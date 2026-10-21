//! Projects: folders of work that wadspaces mount, kept apart from the images
//! (projects.py). A launch mounts a project read-write at ~/Desktop/<mount>;
//! where its folder is depends on its source:
//!   - git: a GitHub repo, cloned on each machine at <projects_dir>/<id>;
//!   - folder: a directory on one machine (machineId: the linked machine's
//!     id, or "local" before it's linked), inside the folder roots;
//!   - drive: a folder on a filesystem known by its UUID, wherever it's
//!     plugged in.
//!
//! Each is a small JSON document, `<state_dir>/projects/<id>.json`, the same
//! shape as WadSpaces Client's users/{uid}/projects/{id} in Firestore, which the
//! cloud relay syncs both ways (last writer wins on updatedAt):
//!
//! ```text
//! {"id", "name", "mountName",
//!  "source": {"kind": "git", "url": "https://github.com/<owner>/<repo>[.git]", "ref"?}
//!          | {"kind": "folder", "machineId", "machineName", "path"}
//!          | {"kind": "drive", "uuid", "label", "fstype", "subpath"},
//!  "setup": "npm install", "deleted": false,
//!  "createdAt": <ms>, "updatedAt": <ms>, "synced": bool}
//! ```
//!
//! Deleting leaves a tombstone, so the deletion syncs; the folder goes only
//! on request (purge). Documents from before (another source kind, a
//! non-GitHub url) still load, marked legacy: true; they launch while their
//! folder is here, and never go to the cloud. Their Syncthing fields
//! (holders, ignore, folderId) are dropped wherever they turn up.
//!
//! The store works on the documents as JSON, so fields it doesn't know
//! survive a round trip.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use wad_proto::v1::ProjectSource;

use crate::folders::{self, FolderError};
use crate::state::truthy;
use crate::workspaces::host_path_ok;

pub const MAX_SETUP: usize = 4000;
pub const NOT_A_SOURCE: &str = "a project is a GitHub repo, a folder or a drive";
const NOT_GITHUB: &str = "a git project is a GitHub repo";
/// A folder project's machineId on a machine that isn't linked (yet).
pub const LOCAL: &str = "local";
/// Fields WadSpaces Client edits; the store keeps the rest.
const EDITABLE: [&str; 4] = ["name", "mountName", "source", "setup"];
const OLD_FIELDS: [&str; 3] = ["holders", "ignore", "folderId"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProjectError {
    /// A bad document (400).
    #[error("{0}")]
    Invalid(String),
    /// Valid on its own, but clashes with another project or a workspace (409).
    #[error("{0}")]
    Conflict(String),
    #[error("no project {0:?}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
}

type Result<T> = std::result::Result<T, ProjectError>;

fn invalid(msg: impl Into<String>) -> ProjectError {
    ProjectError::Invalid(msg.into())
}

/// Shaped like a Firestore auto-id: 20 letters and digits.
pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(20);
    let mut f = std::fs::File::open("/dev/urandom").expect("the kernel's random numbers");
    while out.len() < 20 {
        let mut b = [0u8; 32];
        std::io::Read::read_exact(&mut f, &mut b).expect("the kernel's random numbers");
        // 248 = 4 * 62: no letter more likely than another.
        out.extend(b.iter().filter(|x| **x < 248).map(|x| ALPHABET[(*x % 62) as usize] as char).take(20 - out.len()));
    }
    out
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The image runs a project's setup once per hash of the command.
pub fn setup_hash(setup: &str) -> String {
    let d = Sha256::digest(setup.as_bytes());
    d.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// A folder name (a mount name) from a repo name: anything else becomes "-".
pub fn mount_for(name: &str) -> String {
    let m: String =
        name.chars().map(|c| if wad_core::projects::is_mount(&c.to_string()) { c } else { '-' }).take(64).collect();
    let m = m.trim_matches('-');
    if m.is_empty() || m == "." || m == ".." { "project".into() } else { m.into() }
}

pub fn check_id(pid: &str) -> Result<()> {
    if wad_core::projects::is_project_id(pid) { Ok(()) } else { Err(invalid(format!("bad project id {pid:?}"))) }
}

/// A text field: missing or null is "", else a string of at most `most`
/// characters on one line.
fn text(src: &Value, key: &str, most: usize) -> Result<String> {
    match src.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(t)) if t.chars().count() <= most && !t.contains('\n') => Ok(t.clone()),
        _ => Err(invalid(format!("source.{key} is text ({most} characters at most)"))),
    }
}

/// Python's str(x or ""): a string as it is, a number written out.
fn loose(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(v @ Value::Number(_)) if truthy(v) => v.to_string(),
        Some(Value::Bool(true)) => "True".into(),
        _ => String::new(),
    }
}

/// os.path.normpath(p) == p, for an absolute path.
fn is_normal_abs(p: &str) -> bool {
    p.starts_with('/') && !p.ends_with('/') && !p.contains("//") && !p.split('/').skip(1).any(|s| s == "." || s == "..")
}

/// A project's source, checked as a document (what this machine has on disk
/// is put()'s business).
pub fn check_source(source: &Value) -> Result<ProjectSource> {
    let kind = source.as_object().and_then(|o| o.get("kind")).and_then(Value::as_str);
    match kind {
        Some("git") => {
            let url = loose(source.get("url"));
            let url = url.trim().trim_end_matches('/').to_string();
            if !wad_core::projects::is_github_url(&url) {
                return Err(invalid(format!("{NOT_GITHUB}: {url:?} isn't https://github.com/<owner>/<repo>")));
            }
            let git_ref = match source.get("ref").filter(|r| truthy(r)) {
                None => None,
                Some(r) => {
                    let r = loose(Some(r));
                    let ok = (1..=200).contains(&r.len())
                        && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
                        && !r.starts_with('-')
                        && !r.contains("..");
                    if !ok {
                        return Err(invalid(format!("bad git ref {r:?}")));
                    }
                    Some(r)
                }
            };
            Ok(ProjectSource::Git { url, git_ref })
        }
        Some("folder") => {
            let mid = source.get("machineId").and_then(Value::as_str).unwrap_or_default();
            if !(1..=128).contains(&mid.len())
                || !mid.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                return Err(invalid(format!(
                    "bad source.machineId {:?}",
                    source.get("machineId").unwrap_or(&Value::Null)
                )));
            }
            let path = source.get("path").and_then(Value::as_str).unwrap_or_default();
            if path == "/" || path.chars().count() > 4000 || !is_normal_abs(path) || !host_path_ok(path) {
                return Err(invalid(format!(
                    "source.path {path:?} must be an absolute folder path, without ':' or '%'"
                )));
            }
            Ok(ProjectSource::Folder {
                machine_id: mid.into(),
                machine_name: text(source, "machineName", 200)?,
                path: path.into(),
            })
        }
        Some("drive") => {
            let uuid = source.get("uuid").and_then(Value::as_str).unwrap_or_default();
            if !(1..=64).contains(&uuid.len()) || !uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(invalid(format!("bad source.uuid {:?}", source.get("uuid").unwrap_or(&Value::Null))));
            }
            let fstype = text(source, "fstype", 32)?;
            if !fstype.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
                return Err(invalid(format!("bad source.fstype {fstype:?}")));
            }
            let sub = match source.get("subpath") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(_) => return Err(invalid("subpath is a path inside the drive")),
            };
            let sub = folders::clean_subpath(&sub).map_err(|e| invalid(e.to_string()))?;
            if !host_path_ok(&sub) || sub.chars().count() > 4000 {
                return Err(invalid(format!("source.subpath {sub:?} can't have ':' or '%' in it")));
            }
            Ok(ProjectSource::Drive { uuid: uuid.into(), label: text(source, "label", 200)?, fstype, subpath: sub })
        }
        _ => Err(invalid(NOT_A_SOURCE)),
    }
}

fn source_json(s: &ProjectSource) -> Value {
    serde_json::to_value(s).expect("sources serialize")
}

/// What goes in the file: no old fields, and no legacy (worked out on read).
fn stored(doc: &Value) -> Value {
    let mut d = doc.clone();
    if let Value::Object(o) = &mut d {
        for f in OLD_FIELDS {
            o.remove(f);
        }
        o.remove("legacy");
    }
    d
}

/// A stored document as the API shows it: old fields dropped, and legacy:
/// true when its source is from before.
pub fn clean(doc: &Value) -> Value {
    let mut d = stored(doc);
    let current = d.get("source").is_some_and(|s| check_source(s).is_ok());
    if !current && let Value::Object(o) = &mut d {
        o.insert("legacy".into(), Value::Bool(true));
    }
    d
}

pub fn is_legacy(doc: &Value) -> bool {
    doc.get("legacy").is_some_and(truthy)
}

fn int(doc: &Value, key: &str) -> i64 {
    doc.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0)
}

/// The editable fields of a project, checked and with defaults filled in.
pub fn validate(pid: &str, body: &Value) -> Result<Map<String, Value>> {
    check_id(pid)?;
    if !body.is_object() {
        return Err(invalid("a project is a JSON object"));
    }
    let name = loose(body.get("name")).trim().to_string();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(invalid("a project needs a name (200 characters at most)"));
    }
    let mount = loose(body.get("mountName"));
    if !wad_core::projects::is_mount(&mount) || mount == "." || mount == ".." {
        return Err(invalid(format!("mountName {mount:?} must be a folder name (^[A-Za-z0-9._-]{{1,64}}$)")));
    }
    let src = check_source(body.get("source").unwrap_or(&Value::Null))?;
    let setup = match body.get("setup") {
        None | Some(Value::Null) => String::new(),
        Some(v) if !truthy(v) => String::new(),
        Some(Value::String(s)) if s.chars().count() <= MAX_SETUP => s.clone(),
        _ => return Err(invalid(format!("setup is a shell command ({MAX_SETUP} characters at most)"))),
    };
    let mut m = Map::new();
    m.insert("name".into(), name.into());
    m.insert("mountName".into(), mount.into());
    m.insert("source".into(), source_json(&src));
    m.insert("setup".into(), setup.into());
    Ok(m)
}

type MachineRef = Box<dyn Fn() -> (String, String) + Send + Sync>;

/// The project documents, one JSON file each.
pub struct ProjectStore {
    root: PathBuf,
    folder_roots: Vec<PathBuf>,
    /// (id, name) of this machine, for the folder projects made here.
    machine: MachineRef,
    /// Bumped on every local change (not on what a sync brings in), so the
    /// cloud relay knows there's something to push.
    changes: AtomicU64,
}

impl ProjectStore {
    pub fn new(
        root: impl Into<PathBuf>,
        folder_roots: Vec<PathBuf>,
        machine: impl Fn() -> (String, String) + Send + Sync + 'static,
    ) -> Self {
        Self { root: root.into(), folder_roots, machine: Box::new(machine), changes: AtomicU64::new(0) }
    }

    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Relaxed)
    }

    /// Where the documents are (a new owner's link moves it aside).
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn folder_roots(&self) -> &[PathBuf] {
        &self.folder_roots
    }

    fn path(&self, pid: &str) -> Result<PathBuf> {
        check_id(pid)?;
        Ok(self.root.join(format!("{pid}.json")))
    }

    fn write(&self, doc: &Value) -> Result<Value> {
        let doc = stored(doc);
        let id = doc.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let path = self.path(&id)?;
        let io = |e: std::io::Error| ProjectError::Io(format!("{}: {e}", path.display()));
        std::fs::create_dir_all(&self.root).map_err(io)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&doc).expect("documents serialize")).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        Ok(clean(&doc))
    }

    pub fn list(&self, include_deleted: bool) -> Vec<Value> {
        let mut out: Vec<Value> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
            .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok()) // a broken file shouldn't hide the rest
            .filter(|d| d.is_object() && (include_deleted || !d.get("deleted").is_some_and(truthy)))
            .map(|d| clean(&d))
            .collect();
        let key = |d: &Value| (loose(d.get("name")).to_lowercase(), loose(d.get("id")));
        out.sort_by_key(key);
        out
    }

    pub fn get(&self, pid: &str) -> Result<Value> {
        let path = self.path(pid).map_err(|_| ProjectError::NotFound(pid.into()))?;
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(|d: Value| clean(&d))
                .map_err(|e| ProjectError::Io(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ProjectError::NotFound(pid.into())),
            Err(e) => Err(ProjectError::Io(format!("{}: {e}", path.display()))),
        }
    }

    pub fn get_or_none(&self, pid: &str) -> Option<Value> {
        self.get(pid).ok()
    }

    fn check_mount(&self, pid: &str, mount: &str) -> Result<()> {
        for other in self.list(false) {
            if other["id"] != pid && other.get("mountName").and_then(Value::as_str) == Some(mount) {
                return Err(ProjectError::Conflict(format!(
                    "{:?} already uses the folder name {mount:?}",
                    loose(other.get("name"))
                )));
            }
        }
        Ok(())
    }

    /// A folder source as this machine's: an existing folder inside the
    /// folder roots (symlinks resolved), with this machine's id and name. An
    /// unchanged one (same machine and path) is kept as it is: another
    /// machine's folder project can still be renamed here.
    fn here(&self, source: Option<&Value>, old: &Value) -> Result<Option<Value>> {
        let Some(source) = source else { return Ok(None) };
        if source.get("kind").and_then(Value::as_str) != Some("folder") {
            return Ok(Some(source.clone()));
        }
        let prev = old.get("source").cloned().unwrap_or(Value::Null);
        let given_mid = source.get("machineId");
        let mid_unchanged = match given_mid {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) if s.is_empty() => true,
            Some(m) => Some(m) == prev.get("machineId"),
        };
        if prev.get("kind").and_then(Value::as_str) == Some("folder")
            && source.get("path") == prev.get("path")
            && mid_unchanged
        {
            return Ok(Some(prev));
        }
        let path = source.get("path").and_then(Value::as_str).unwrap_or_default();
        let real = folders::resolve_folder(path, &self.folder_roots, true).map_err(|e| invalid(e.to_string()))?;
        if !Path::new(&real).is_dir() {
            return Err(invalid(format!("folder {path} doesn't exist on this machine")));
        }
        let (mid, name) = (self.machine)();
        Ok(Some(json!({"kind": "folder", "machineId": mid, "machineName": name, "path": real})))
    }

    /// Whether put() would take this (nothing is saved): to check before
    /// doing what can't be undone (making a GitHub repo).
    pub fn check(&self, pid: &str, body: &Value) -> Result<()> {
        let fields = validate(pid, body)?;
        self.check_mount(pid, fields["mountName"].as_str().unwrap_or_default())
    }

    /// Creates or updates (and undeletes). The store sets the timestamps.
    pub fn put(&self, pid: &str, body: &Value) -> Result<Value> {
        let old = self.get_or_none(pid).unwrap_or_else(|| json!({}));
        let mut body = body.clone();
        if let Value::Object(o) = &mut body
            && let Some(src) = self.here(o.get("source"), &old)?
        {
            o.insert("source".into(), src);
        }
        let fields = validate(pid, &body)?;
        self.check_mount(pid, fields["mountName"].as_str().unwrap_or_default())?;
        // Always newer than what it replaces.
        let t = now_ms().max(int(&old, "updatedAt") + 1);
        let created = Some(int(&old, "createdAt")).filter(|c| *c != 0).unwrap_or(t);
        let mut doc = Map::new();
        doc.insert("id".into(), pid.into());
        doc.extend(fields);
        doc.insert("deleted".into(), false.into());
        doc.insert("createdAt".into(), created.into());
        doc.insert("updatedAt".into(), t.into());
        doc.insert("synced".into(), old.get("synced").is_some_and(truthy).into());
        self.changes.fetch_add(1, Ordering::Relaxed);
        self.write(&Value::Object(doc))
    }

    /// Tombstones it (the folder stays; see purge_dir).
    pub fn delete(&self, pid: &str) -> Result<Value> {
        let mut doc = self.get(pid)?;
        if doc.get("deleted").is_some_and(truthy) {
            return Ok(doc);
        }
        let t = now_ms().max(int(&doc, "updatedAt") + 1);
        doc["deleted"] = true.into();
        doc["updatedAt"] = t.into();
        self.changes.fetch_add(1, Ordering::Relaxed);
        self.write(&doc)
    }

    /// Brings in the cloud's documents, the newest updatedAt winning. Returns
    /// the local documents the cloud should get (newer here, or never sent;
    /// never a legacy one). A project the cloud knew but no longer has was
    /// deleted outright there: it becomes a tombstone here too.
    pub fn merge(&self, remote: &[Value]) -> Vec<Value> {
        let local: Vec<Value> = self.list(true);
        let mine_of = |pid: &str| local.iter().find(|d| d["id"] == pid);
        let mut push: Vec<Value> = vec![];
        let mut seen: Vec<String> = vec![];
        for r in remote {
            let pid = loose(r.get("id"));
            if check_id(&pid).is_err() {
                tracing::warn!("ignoring a cloud project with a bad id {pid:?}");
                continue;
            }
            seen.push(pid.clone());
            let mine = mine_of(&pid);
            let theirs = int(r, "updatedAt");
            let ours = mine.map(|m| int(m, "updatedAt")).unwrap_or(0);
            if mine.is_none() || theirs > ours {
                let deleted = r.get("deleted").is_some_and(truthy);
                let fields = if deleted { Ok(tombstone_fields(r, mine)) } else { validate(&pid, r) };
                let fields = match fields {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!("ignoring cloud project {pid}: {e}");
                        continue;
                    }
                };
                let mut doc = Map::new();
                doc.insert("id".into(), pid.clone().into());
                doc.extend(fields);
                doc.insert("deleted".into(), deleted.into());
                let created = Some(int(r, "createdAt")).filter(|c| *c != 0).unwrap_or(theirs);
                doc.insert("createdAt".into(), created.into());
                doc.insert("updatedAt".into(), theirs.into());
                doc.insert("synced".into(), true.into());
                let _ = self.write(&Value::Object(doc));
            } else if theirs < ours {
                push.extend(mine.cloned());
            } else if let Some(m) = mine.filter(|m| !m.get("synced").is_some_and(truthy)) {
                let mut m = m.clone();
                m["synced"] = true.into();
                let _ = self.write(&m);
            }
        }
        for mine in &local {
            let pid = loose(mine.get("id"));
            if seen.contains(&pid) {
                continue;
            }
            if mine.get("synced").is_some_and(truthy) {
                if !mine.get("deleted").is_some_and(truthy) {
                    tracing::info!("project {pid} is gone from the cloud; marking it deleted here");
                    let mut m = mine.clone();
                    m["deleted"] = true.into();
                    let _ = self.write(&m);
                }
            } else {
                push.push(mine.clone());
            }
        }
        let legacy: Vec<String> = push.iter().filter(|d| is_legacy(d)).map(|d| loose(d.get("id"))).collect();
        if !legacy.is_empty() {
            tracing::info!("not sending projects that aren't GitHub repos to the cloud: {}", legacy.join(", "));
        }
        push.retain(|d| !is_legacy(d));
        push
    }

    /// Folder projects made before this machine was linked (machineId
    /// "local") become this machine's, before they go to the cloud.
    pub fn claim_local(&self, machine_id: &str, machine_name: &str) -> Vec<String> {
        let mut out = vec![];
        for mut doc in self.list(false) {
            let src = doc.get("source").cloned().unwrap_or(Value::Null);
            if src.get("kind").and_then(Value::as_str) == Some("folder")
                && src.get("machineId").and_then(Value::as_str) == Some(LOCAL)
            {
                let t = now_ms().max(int(&doc, "updatedAt") + 1);
                doc["source"]["machineId"] = machine_id.into();
                doc["source"]["machineName"] = machine_name.into();
                doc["updatedAt"] = t.into();
                if self.write(&doc).is_ok() {
                    out.push(loose(doc.get("id")));
                }
            }
        }
        if !out.is_empty() {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
        out
    }

    pub fn mark_synced(&self, ids: &[String]) {
        for pid in ids {
            if let Some(mut doc) = self.get_or_none(pid)
                && !doc.get("synced").is_some_and(truthy)
            {
                doc["synced"] = true.into();
                let _ = self.write(&doc);
            }
        }
    }
}

/// A deleted project from the cloud may be partial; keep what we had.
fn tombstone_fields(remote: &Value, mine: Option<&Value>) -> Map<String, Value> {
    let mut m = Map::new();
    for k in EDITABLE {
        let v = remote.get(k).filter(|v| !v.is_null()).or_else(|| mine.and_then(|m| m.get(k))).cloned();
        m.insert(k.into(), v.unwrap_or(Value::Null));
    }
    if m["name"].is_null() || m["name"] == "" {
        m.insert("name".into(), remote.get("id").cloned().unwrap_or_default());
    }
    for (k, default) in [("mountName", json!("")), ("source", json!({})), ("setup", json!(""))] {
        if m[k].is_null() {
            m.insert(k.into(), default);
        }
    }
    m
}

// ------------------------------------------------------- folders on disk
pub fn project_dir(projects_dir: &Path, pid: &str) -> Result<PathBuf> {
    check_id(pid)?;
    Ok(projects_dir.join(pid))
}

/// Removes a project's folder and everything in it. False if there was none.
pub fn purge_dir(projects_dir: &Path, pid: &str) -> Result<bool> {
    let path = project_dir(projects_dir, pid)?;
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&path).map_err(|e| ProjectError::Io(format!("{}: {e}", path.display())))?;
    Ok(true)
}

/// <state_dir>/extra/<ws>/projects.json: what the image's wadspaces-projects
/// reads at /run/wadspaces-extra (version 2): each project's kind and folder,
/// a GitHub one's repo, branch and setup (with its hash: a changed setup
/// runs again). The wadspace clones and copies them in; a mount (and a
/// mounted GitHub project, listed as one) is already there.
pub fn write_manifest(
    state_dir: &Path,
    ws: &str,
    projects: &[wad_proto::v1::WorkspaceProject],
) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let d = state_dir.join("extra").join(ws);
    std::fs::create_dir_all(&d)?;
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755))?;
    let list: Vec<Value> = projects
        .iter()
        .map(|p| {
            let kind = if p.is_mounted() { wad_proto::v1::ProjectKind::Mount } else { p.kind };
            let mut e = json!({"id": p.id, "kind": kind, "mount": p.mount, "name": p.mount});
            if kind == wad_proto::v1::ProjectKind::Github {
                e["repo"] = p.repo.clone().into();
                if let Some(r) = &p.git_ref {
                    e["ref"] = r.clone().into();
                }
                e["setup"] = p.setup.clone().into();
                e["setupHash"] = setup_hash(&p.setup).into();
            }
            e
        })
        .collect();
    let path = d.join("projects.json");
    let tmp = d.join(".projects.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&json!({"version": 2, "projects": list})).expect("json") + "\n")?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

impl From<FolderError> for ProjectError {
    fn from(e: FolderError) -> Self {
        invalid(e.to_string())
    }
}

#[cfg(test)]
mod tests;
