//! Saved sign-ins: folders on this machine, each mounted into every
//! workspace whose design gives it to one of its apps, so signing in once
//! covers all of them.
//!
//! - The record: `<state_dir>/signins/<id>.json` ({id, name, app, paths, createdAt}).
//! - The folder: `<signins_dir>/<id>`, the containers' user's, at
//!   /run/wadspaces-signins/<id> inside.
//! - Before a container starts, `prepare` makes any folder it's missing and
//!   writes `<state_dir>/extra/<ws>/signins.json`, which says inside what each
//!   is for: a web sign-in is the WadBrowser profile of the web apps listed,
//!   an app's holds its files (`paths`, under the home folder), which the
//!   image's init links to it.
//!
//! Two running workspaces may share one: whichever writes last wins. Names
//! are unique here (they're what people pick sign-ins by). WadBrowser setups
//! (launched beside the spaces) use web sign-ins too: `usedBy` lists the saved
//! ones, as `browser-<id>`, the workspace each launches as.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use wad_core::signins as core;
use wad_proto::v1::{Event, SignIn, SignInPatch, SignInRequest, Workspace, WorkspaceSignIn};
use wad_proto::{ApiError, ErrorCode};

use crate::events::Bus;
use crate::registry::Registry;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: String,
    name: String,
    app: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    created_at: i64,
}

pub struct SignIns {
    records: PathBuf,
    data: PathBuf,
    state_dir: PathBuf,
    uid: u32,
    registry: Arc<Registry>,
    bus: Bus,
    /// One change at a time.
    lock: Mutex<()>,
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

fn internal(what: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, format!("{what}: {e}"))
}

fn not_found(id: &str) -> ApiError {
    ApiError::new(ErrorCode::NotFound, format!("no sign-in {id:?}"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(bad("a sign-in's name is 1 to 80 characters, on one line"));
    }
    Ok(name.to_string())
}

/// An app's files: its own when the catalog knows them, else the ones given.
fn paths_for(app: &str, given: Vec<String>) -> Result<Vec<String>, ApiError> {
    if app == core::WEB {
        return Ok(vec![]);
    }
    if let Some(known) = core::paths_for(app) {
        return Ok(known.iter().map(|p| p.to_string()).collect());
    }
    let mut out: Vec<String> = Vec::new();
    for p in given {
        let p = p.trim().trim_start_matches("~/").to_string();
        if !core::is_path(&p) {
            return Err(bad(format!("{p:?}: a path under the home folder, like .config/app")));
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    if out.is_empty() {
        return Err(bad("which files hold this app's sign-in? Name at least one path under the home folder"));
    }
    Ok(out)
}

/// `github-k3x9q2`: its name's slug and six random letters.
fn new_id(name: &str) -> String {
    let slug = wad_core::spec::slugify(name);
    let slug = slug.get(..40.min(slug.len())).unwrap_or("").trim_end_matches('-');
    let tail = wad_store::projects::new_id().to_lowercase();
    let tail: String = tail.chars().filter(char::is_ascii_alphanumeric).take(6).collect();
    if slug.is_empty() { format!("signin-{tail}") } else { format!("{slug}-{tail}") }
}

/// Its size in bytes and when anything in it last changed (no links followed).
fn usage(dir: &Path) -> (u64, Option<i64>) {
    let (mut size, mut newest) = (0u64, None::<i64>);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                size += m.len();
            }
            let at = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);
            newest = newest.max(at);
        }
    }
    (size, newest)
}

/// A folder for the containers' user alone (theirs when wadd is root),
/// labelled for containers.
fn make_private_dir(path: &Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    if nix::unistd::geteuid().is_root() {
        nix::unistd::chown(path, Some(uid.into()), Some(uid.into())).map_err(std::io::Error::from)?;
    }
    crate::projects::relabel(path, false);
    Ok(())
}

impl SignIns {
    pub fn new(state_dir: &Path, signins_dir: PathBuf, uid: u32, registry: Arc<Registry>, bus: Bus) -> Arc<Self> {
        Arc::new(Self {
            records: state_dir.join("signins"),
            data: signins_dir,
            state_dir: state_dir.to_path_buf(),
            uid,
            registry,
            bus,
            lock: Mutex::new(()),
        })
    }

    fn publish(&self, ids: Vec<String>) {
        self.bus.publish(Event::SignIns { ids });
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.records.join(format!("{id}.json"))
    }

    /// Its folder on the host.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.data.join(id)
    }

    fn read(&self, id: &str) -> Option<Record> {
        if !wad_core::spec::is_id(id) {
            return None;
        }
        let text = std::fs::read_to_string(self.record_path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write(&self, r: &Record) -> Result<(), ApiError> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&self.records).map_err(|e| internal("sign-ins", e))?;
        let tmp = self.records.join(format!(".{}.json.tmp", r.id));
        let text = serde_json::to_string_pretty(r).expect("json") + "\n";
        std::fs::write(&tmp, text).map_err(|e| internal("sign-in", e))?;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        std::fs::rename(&tmp, self.record_path(&r.id)).map_err(|e| internal("sign-in", e))
    }

    fn records(&self) -> Vec<Record> {
        let Ok(entries) = std::fs::read_dir(&self.records) else { return vec![] };
        let mut out: Vec<Record> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(String::from))
            .filter_map(|id| self.read(&id))
            .collect();
        out.sort_by_key(|r| r.name.to_lowercase());
        out
    }

    /// Where its folder is kept: `<signins_dir>`.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The workspaces here that mount it (and the saved WadBrowser setups
    /// that use it), and those of them running.
    fn users(&self, id: &str) -> (Vec<String>, Vec<String>) {
        let mut used: Vec<String> = self
            .registry
            .workspaces()
            .into_iter()
            .filter(|w| w.signins.iter().any(|s| s.id == id))
            .map(|w| w.id)
            .collect();
        let setups = wad_store::State::new(&self.state_dir).library("browsers").unwrap_or_default();
        for b in setups.iter().filter(|b| !b.get("deletedAt").is_some_and(wad_store::state::truthy)) {
            let uses = b["secrets"].as_object().is_some_and(|m| m.values().any(|r| r["id"] == id));
            let ws = format!("browser-{}", b["id"].as_str().unwrap_or_default());
            if uses && !used.contains(&ws) {
                used.push(ws);
            }
        }
        let running =
            used.iter().filter(|w| self.registry.state(w).is_some_and(|s| s.container == "running")).cloned().collect();
        (used, running)
    }

    fn view(&self, r: Record) -> SignIn {
        let (size, changed_at) = usage(&self.dir(&r.id));
        let (used_by, in_use) = self.users(&r.id);
        SignIn {
            id: r.id,
            name: r.name,
            app: r.app,
            paths: r.paths,
            created_at: r.created_at,
            size,
            changed_at,
            used_by,
            in_use,
        }
    }

    pub fn list(&self) -> Vec<SignIn> {
        self.records().into_iter().map(|r| self.view(r)).collect()
    }

    pub fn get(&self, id: &str) -> Result<SignIn, ApiError> {
        self.read(id).map(|r| self.view(r)).ok_or_else(|| not_found(id))
    }

    /// Refuses a name another sign-in here has (letter case aside).
    fn name_free(&self, name: &str, except: Option<&str>) -> Result<(), ApiError> {
        let n = name.to_lowercase();
        if self.records().iter().any(|r| Some(r.id.as_str()) != except && r.name.to_lowercase() == n) {
            return Err(ApiError::new(ErrorCode::Conflict, format!("there's a sign-in called {name} already")));
        }
        Ok(())
    }

    pub fn create(&self, req: SignInRequest) -> Result<SignIn, ApiError> {
        let _one = self.lock.lock().unwrap();
        let name = clean_name(&req.name)?;
        self.name_free(&name, None)?;
        if req.app != core::WEB && !wad_core::spec::is_id(&req.app) {
            return Err(bad(format!("{:?} isn't an app id", req.app)));
        }
        let paths = paths_for(&req.app, req.paths)?;
        let mut id = new_id(&name);
        while self.record_path(&id).exists() {
            id = new_id(&name);
        }
        let r = Record { id: id.clone(), name, app: req.app, paths, created_at: now_ms() };
        make_private_dir(&self.dir(&id), self.uid).map_err(|e| internal("its folder", e))?;
        self.write(&r)?;
        self.publish(vec![id]);
        Ok(self.view(r))
    }

    /// Renames it, or (an app the catalog doesn't know) changes its files.
    pub fn patch(&self, id: &str, patch: SignInPatch) -> Result<SignIn, ApiError> {
        let _one = self.lock.lock().unwrap();
        let mut r = self.read(id).ok_or_else(|| not_found(id))?;
        if let Some(name) = patch.name {
            r.name = clean_name(&name)?;
            self.name_free(&r.name, Some(id))?;
        }
        if let Some(paths) = patch.paths {
            if r.app == core::WEB || core::paths_for(&r.app).is_some() {
                return Err(bad("this sign-in's files are its app's own"));
            }
            r.paths = paths_for(&r.app, paths)?;
        }
        self.write(&r)?;
        self.publish(vec![id.into()]);
        Ok(self.view(r))
    }

    fn refuse_while_running(&self, id: &str, what: &str) -> Result<(), ApiError> {
        let (_, running) = self.users(id);
        if running.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = running.iter().map(|w| self.name_of(w)).collect();
        Err(ApiError::new(ErrorCode::Conflict, format!("{} using it; stop it first to {what}", names.join(", "))))
    }

    fn name_of(&self, ws: &str) -> String {
        self.registry.workspaces().into_iter().find(|w| w.id == ws).map(|w| w.name).unwrap_or_else(|| ws.into())
    }

    /// Forgets what it saved (every wadspace using it is signed out), keeping it.
    pub fn sign_out(&self, id: &str) -> Result<SignIn, ApiError> {
        let _one = self.lock.lock().unwrap();
        let r = self.read(id).ok_or_else(|| not_found(id))?;
        self.refuse_while_running(id, "sign out")?;
        let dir = self.dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| internal("signing out", e))?;
        }
        make_private_dir(&dir, self.uid).map_err(|e| internal("its folder", e))?;
        tracing::info!("signed out of {} ({id})", r.name);
        self.publish(vec![id.into()]);
        Ok(self.view(r))
    }

    /// Removes it and what it saved; not while a workspace still mounts it.
    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        let _one = self.lock.lock().unwrap();
        let r = self.read(id).ok_or_else(|| not_found(id))?;
        let (used, _) = self.users(id);
        if !used.is_empty() {
            let names: Vec<String> = used.iter().map(|w| self.name_of(w)).collect();
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("{} still use it; give their apps another sign-in first", names.join(", ")),
            ));
        }
        let dir = self.dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| internal("removing it", e))?;
        }
        std::fs::remove_file(self.record_path(id)).map_err(|e| internal("removing it", e))?;
        tracing::info!("deleted sign-in {} ({id})", r.name);
        self.publish(vec![id.into()]);
        Ok(())
    }

    /// A sign-in a launch names, made here (empty) when this machine hasn't
    /// got it yet: its record and its folder.
    pub fn ensure(&self, s: &WorkspaceSignIn) -> Result<(String, PathBuf), String> {
        let _one = self.lock.lock().unwrap();
        let r = self.ensure_record(s)?;
        make_private_dir(&self.dir(&s.id), self.uid).map_err(|e| format!("sign-in {}: {e}", r.name))?;
        Ok((r.name, self.dir(&s.id)))
    }

    /// Its record, made from what the workspace says when it isn't here.
    /// (Under the lock.)
    fn ensure_record(&self, s: &WorkspaceSignIn) -> Result<Record, String> {
        if let Some(r) = self.read(&s.id) {
            return Ok(r);
        }
        if !wad_core::spec::is_id(&s.id) {
            return Err(format!("{:?} isn't a sign-in id", s.id));
        }
        let app = if !s.kind.is_empty() {
            s.kind.clone()
        } else if s.web {
            core::WEB.to_string()
        } else {
            s.app.clone()
        };
        let paths = paths_for(&app, vec![]).unwrap_or_default();
        let wanted = if s.name.trim().is_empty() { s.app.clone() } else { s.name.trim().to_string() };
        // Another here has its name: keep both, this one marked.
        let mut name = wanted.clone();
        let mut n = 2;
        while self.records().iter().any(|r| r.name.to_lowercase() == name.to_lowercase()) {
            name = format!("{wanted} ({n})");
            n += 1;
        }
        let r = Record { id: s.id.clone(), name, app, paths, created_at: now_ms() };
        self.write(&r).map_err(|e| e.message)?;
        self.publish(vec![s.id.clone()]);
        Ok(r)
    }

    /// The names an installed wadspace's document gives its keys, taken: a
    /// key renamed elsewhere (the phone renames in the documents) is called
    /// that here too, unless another sign-in here is. Only at an install,
    /// when the document is the newest there is.
    pub fn follow_names(&self, signins: &[WorkspaceSignIn]) {
        let _one = self.lock.lock().unwrap();
        for s in signins {
            let (Some(mut r), Ok(name)) = (self.read(&s.id), clean_name(&s.name)) else { continue };
            if name == r.name || self.name_free(&name, Some(&r.id)).is_err() {
                continue;
            }
            tracing::info!("sign-in {}: {} is called {name} now", r.id, r.name);
            r.name = name;
            if self.write(&r).is_ok() {
                self.publish(vec![r.id.clone()]);
            }
        }
    }

    /// Gets a container's sign-ins ready to mount: each one's folder (made
    /// here, empty, when its design came from a machine that has it), and
    /// signins.json for inside.
    pub fn prepare(&self, ws: &Workspace) -> Result<(), String> {
        let manifest = self.state_dir.join("extra").join(&ws.id).join("signins.json");
        if ws.signins.is_empty() {
            let _ = std::fs::remove_file(&manifest);
            return Ok(());
        }
        let _one = self.lock.lock().unwrap();
        let mut entries: Vec<serde_json::Value> = vec![];
        for s in &ws.signins {
            let r = self.ensure_record(s)?;
            make_private_dir(&self.dir(&s.id), self.uid).map_err(|e| format!("sign-in {}: {e}", r.name))?;
            if (r.app == core::WEB) != s.web {
                tracing::warn!("{}: {} is a {} sign-in; {} can't use it", ws.id, r.name, r.app, s.app);
                continue;
            }
            match entries.iter_mut().find(|e| e["id"] == r.id.as_str()) {
                Some(e) => e["apps"].as_array_mut().expect("a list").push(s.app.clone().into()),
                None => entries.push(json!({
                    "id": r.id,
                    "name": r.name,
                    "web": r.app == core::WEB,
                    "apps": [s.app],
                    "dir": format!("{}/{}", core::MOUNT_ROOT, r.id),
                    "paths": r.paths,
                })),
            }
        }
        write_manifest(&manifest, &json!({"version": 1, "signins": entries}))
            .map_err(|e| format!("signins.json: {e}"))?;
        Ok(())
    }
}

fn write_manifest(path: &Path, v: &serde_json::Value) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().expect("a folder");
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
    let tmp = dir.join(".signins.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).expect("json") + "\n")?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, path)
}
