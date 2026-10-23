//! Reading what's in the state directory, as the Python wadd wrote it.

use std::path::{Path, PathBuf};

use serde_json::Value;
use wad_proto::v1::{CloudLink, Project, ProjectSource, Run, Session, SessionMode};

/// JS/Python-ish truthiness for JSON values read from the files.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Fields projects had when they were Syncthing folders: dropped on sight.
const OLD_FIELDS: &[&str] = &["holders", "ignore", "folderId"];
/// The library's collections here: saved WadBrowser setups. (Wadspaces live
/// in the account.)
pub const COLLECTIONS: &[&str] = &["browsers"];

#[derive(Debug, Clone)]
pub struct State {
    pub dir: PathBuf,
    /// runs.jsonl as read so far: only what's appended is read again.
    runs: std::sync::Arc<std::sync::Mutex<RunsSeen>>,
}

/// The run history read so far, and how far into which file.
#[derive(Debug, Default)]
struct RunsSeen {
    ino: u64,
    read: u64,
    order: Vec<String>,
    runs: std::collections::HashMap<String, Run>,
}

impl RunsSeen {
    fn take(&mut self, line: &str) {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { return };
        match rec.get("event").and_then(Value::as_str) {
            Some("start") => {
                let id = s(&rec, "id");
                if !self.runs.contains_key(&id) {
                    self.order.push(id.clone());
                }
                self.runs.insert(
                    id.clone(),
                    Run {
                        id,
                        wadspace_id: s(&rec, "wadspaceId"),
                        wadspace_name: s(&rec, "wadspaceName"),
                        mode: s(&rec, "mode"),
                        user: s(&rec, "user"),
                        projects: rec
                            .get("projects")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
                            .unwrap_or_default(),
                        started_at: rec.get("startedAt").and_then(Value::as_f64).unwrap_or(0.0),
                        ended_at: rec.get("endedAt").and_then(Value::as_f64),
                    },
                );
            }
            Some("end") => {
                if let Some(r) = self.runs.get_mut(&s(&rec, "id")) {
                    r.ended_at = rec.get("endedAt").and_then(Value::as_f64);
                }
            }
            _ => {}
        }
    }

    /// Reads what was appended since last time (all of it when the file is
    /// another one, or shorter than what was read).
    fn catch_up(&mut self, path: &Path) {
        use std::io::{Read, Seek, SeekFrom};
        use std::os::unix::fs::MetadataExt;
        let Ok(mut f) = std::fs::File::open(path) else {
            *self = Self::default();
            return;
        };
        let Ok(m) = f.metadata() else { return };
        if m.ino() != self.ino || m.len() < self.read {
            *self = Self { ino: m.ino(), ..Self::default() };
        }
        if m.len() == self.read || f.seek(SeekFrom::Start(self.read)).is_err() {
            return;
        }
        let mut more = Vec::new();
        if f.read_to_end(&mut more).is_err() {
            return;
        }
        // Whole lines only: one being written is read next time.
        let Some(end) = more.iter().rposition(|b| *b == b'\n') else { return };
        for line in String::from_utf8_lossy(&more[..end]).lines() {
            self.take(line);
        }
        self.read += end as u64 + 1;
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `*.json` in a directory, sorted by name (a broken file is skipped, as Python does).
fn json_files(dir: &Path) -> Vec<(String, Value)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut names: Vec<PathBuf> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    names.sort();
    names.into_iter().filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), read_json(&p)?))).collect()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// `os.path.normpath(p) == p` for an absolute path.
/// A project's source, if it's one wadd knows today (projects.py check_source).
pub fn current_source(src: &Value) -> Option<ProjectSource> {
    crate::projects::check_source(src).ok()
}

/// A stored project document as the API shows it.
pub fn project_of(id: &str, d: &Value) -> Project {
    let source = d.get("source").and_then(current_source);
    let ms = |k: &str| d.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0);
    Project {
        id: d.get("id").and_then(Value::as_str).unwrap_or(id).to_string(),
        name: s(d, "name"),
        mount_name: s(d, "mountName"),
        legacy: source.is_none(),
        source,
        setup: s(d, "setup"),
        deleted: d.get("deleted").is_some_and(truthy),
        created_at: ms("createdAt"),
        updated_at: ms("updatedAt"),
        synced: d.get("synced").is_some_and(truthy),
    }
}

impl State {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), runs: Default::default() }
    }

    /// The account link. The refresh token stays in the file: only whether
    /// there is one counts (linked).
    pub fn cloud(&self) -> CloudLink {
        let e = read_json(&self.dir.join("enrollment.json")).unwrap_or(Value::Null);
        let opt = |k: &str| e.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
        CloudLink {
            linked: e.get("refresh_token").is_some_and(truthy) && e.get("machine_id").is_some_and(truthy),
            machine_id: opt("machine_id"),
            owner_uid: opt("owner_uid"),
            project_id: opt("project_id"),
            linked_at: opt("enrolled_at"),
            last_heartbeat: None,
            last_error: None,
        }
    }

    /// Every project document, tombstones too, by name then id.
    pub fn projects(&self) -> Vec<Project> {
        let mut out: Vec<Project> = json_files(&self.dir.join("projects"))
            .into_iter()
            .filter(|(_, d)| d.is_object())
            .map(|(id, mut d)| {
                if let Value::Object(o) = &mut d {
                    for f in OLD_FIELDS {
                        o.remove(*f);
                    }
                }
                project_of(&id, &d)
            })
            .collect();
        out.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
        out
    }

    /// Runs, newest first (runs.py list): a run whose end was never written
    /// ends when it started (wadd went away mid-run).
    pub fn runs(&self, workspace: Option<&str>, limit: usize) -> Vec<Run> {
        let mut seen = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        seen.catch_up(&self.dir.join("runs.jsonl"));
        let mut out: Vec<&Run> = seen
            .order
            .iter()
            .filter_map(|id| seen.runs.get(id))
            .filter(|r| workspace.is_none_or(|w| r.wadspace_id == w))
            .collect();
        out.sort_by(|a, b| b.started_at.total_cmp(&a.started_at));
        out.into_iter().take(limit).map(|r| Run { ended_at: r.ended_at.or(Some(r.started_at)), ..r.clone() }).collect()
    }

    /// The session in progress (session.json), if any. `known`: the
    /// workspaces there are now; others drop out, and with none left, no session.
    pub fn session(&self, known: &[String]) -> Option<Session> {
        let d = read_json(&self.dir.join("session.json"))?;
        let ids: Vec<String> = d
            .get("workspaces")?
            .as_array()?
            .iter()
            .filter_map(|w| w.as_str().map(String::from))
            .filter(|w| known.contains(w))
            .collect();
        if ids.is_empty() {
            return None;
        }
        let mode = match d.get("mode").and_then(Value::as_str) {
            Some("free") => SessionMode::Free,
            _ => SessionMode::Focus,
        };
        let ends_at = d.get("ends_at").and_then(Value::as_f64);
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs_f64()).unwrap_or(0.0);
        Some(Session {
            mode,
            workspaces: ids,
            minutes: d.get("minutes").and_then(Value::as_u64).map(|m| m as u32),
            started_at: d.get("started_at").and_then(Value::as_f64).unwrap_or(0.0),
            ends_at,
            expired: d.get("expired").is_some_and(truthy) || ends_at.is_some_and(|e| now >= e),
        })
    }

    /// WadSpaces Client's library: a collection's documents (opaque to wadd).
    pub fn library(&self, collection: &str) -> Option<Vec<Value>> {
        COLLECTIONS
            .contains(&collection)
            .then(|| json_files(&self.dir.join("library").join(collection)).into_iter().map(|(_, d)| d).collect())
    }

    fn library_file(&self, collection: &str, id: &str) -> Result<PathBuf, String> {
        if !COLLECTIONS.contains(&collection) {
            return Err(format!("no collection {collection:?}"));
        }
        // ^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$
        let ok = (1..=128).contains(&id.len())
            && id.as_bytes()[0].is_ascii_alphanumeric()
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !ok {
            return Err(format!("bad id {id:?}"));
        }
        Ok(self.dir.join("library").join(collection).join(format!("{id}.json")))
    }

    /// One library document; Ok(None) when there's none.
    pub fn library_get(&self, collection: &str, id: &str) -> Result<Option<Value>, String> {
        Ok(read_json(&self.library_file(collection, id)?))
    }

    /// Saves a library document (a JSON object, 24 MB at most: wallpapers
    /// ride along as data URLs).
    pub fn library_put(&self, collection: &str, id: &str, doc: &Value) -> Result<(), String> {
        let path = self.library_file(collection, id)?;
        if !doc.is_object() {
            return Err("a document is a JSON object".into());
        }
        let data = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
        if data.len() > 24 * 1024 * 1024 {
            return Err("document too big (24 MB max)".into());
        }
        let dir = path.parent().expect("a folder");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// False if there was none.
    pub fn library_delete(&self, collection: &str, id: &str) -> Result<bool, String> {
        match std::fs::remove_file(self.library_file(collection, id)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    /// What a launch mounted into a workspace (extra/<ws>/projects.json).
    pub fn manifest(&self, ws: &str) -> Option<Value> {
        read_json(&self.dir.join("extra").join(ws).join("projects.json"))
    }

    /// Secrets seeded from the image, by name (seeded-secrets.json).
    pub fn seeded_secrets(&self) -> Vec<String> {
        read_json(&self.dir.join("seeded-secrets.json"))
            .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default()
    }

    /// The digest (sha256, hex) of the value a secret was last seeded with.
    pub fn seeded_digest(&self, name: &str) -> Option<String> {
        read_json(&self.dir.join("seeded-secrets.json"))
            .and_then(|v| v.get(name).and_then(|d| d.as_str()).map(String::from))
    }
}

#[cfg(test)]
mod tests;
