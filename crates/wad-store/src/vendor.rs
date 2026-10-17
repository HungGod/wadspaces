//! The workspaces that come with the image: one TOML file per workspace in
//! `/usr/lib/wadspaces/workspaces.d/<id>.toml`, with workspaces.yaml's keys
//! (and its defaults: workspaces.rs checks them the same way).
//!
//! The machine's list is state (`<state_dir>/workspaces.json`): it changes
//! as you build, edit and delete workspaces, and an image update can't
//! overwrite it the way bootc's /etc merge kept an old workspaces.yaml. So
//! the image's workspaces go in as updates: each one is applied when it's
//! new or the image changed it since it was last applied (a digest per id
//! in `<state_dir>/workspaces-vendor.json`), and otherwise left alone. One
//! you deleted stays deleted until the image changes it. Applying keeps the
//! projects the machine mounts in it.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use wad_proto::v1::{Kind, Workspace};

use crate::Error;
use crate::workspaces;

pub const BOOK: &str = "workspaces-vendor.json";

/// A workspace from the image, and a digest of what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorWorkspace {
    pub workspace: Workspace,
    pub digest: String,
}

/// id -> digest of the image's workspace when it was last applied.
pub type Book = BTreeMap<String, String>;

fn digest(w: &Workspace) -> String {
    let text = serde_json::to_string(w).expect("workspaces serialize");
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The image's workspaces, in file-name order; none when `dir` is missing.
/// The id is the file's name unless the file says it (then they must match).
pub fn read(dir: &Path) -> Result<Vec<VendorWorkspace>, Error> {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(Error::Io(dir.display().to_string(), e)),
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut raw = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).map_err(|e| Error::Io(f.display().to_string(), e))?;
        let mut v: Value = toml::from_str(&text).map_err(|e| Error::Yaml(f.display().to_string(), e.to_string()))?;
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Value::Object(m) = &mut v else { unreachable!("a TOML document is a table") };
        match m.get("id").and_then(Value::as_str) {
            None => {
                m.insert("id".into(), stem.into());
            }
            Some(id) if id == stem => {}
            Some(id) => {
                return Err(Error::Config(format!("{}: id {id:?} isn't the file's name", f.display())));
            }
        }
        raw.push(v);
    }
    // Checked together, as one workspaces.yaml would be.
    let list = workspaces::check_workspaces(raw).map_err(|e| Error::Config(format!("{}: {e}", dir.display())))?;
    Ok(list.into_iter().map(|w| VendorWorkspace { digest: digest(&w), workspace: w }).collect())
}

pub fn read_book(state_dir: &Path) -> Book {
    std::fs::read(state_dir.join(BOOK)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn write_book(state_dir: &Path, book: &Book) -> Result<(), Error> {
    let path = state_dir.join(BOOK);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(book).expect("json") + "\n";
    std::fs::create_dir_all(state_dir)
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| Error::Io(path.display().to_string(), e))
}

/// What applying the image's workspaces did.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// Added or updated from the image.
    pub changed: Vec<String>,
    /// Image workspaces that clash with the machine's (a hotkey, a port, a
    /// container name): left out, and tried again next time.
    pub refused: Vec<(String, String)>,
}

/// Applies the image's workspaces that are new or changed to `list`,
/// recording them in `book`. The machine's own apps (WadBrowser, the
/// terminal) are the system's: one missing from the list comes back, even
/// unchanged, and one the image doesn't ship any more goes.
pub fn apply(list: &mut Vec<Workspace>, book: &mut Book, vendor: &[VendorWorkspace]) -> Applied {
    let mut out = Applied::default();
    let shipped = |id: &str| vendor.iter().any(|v| v.workspace.id == id);
    let dropped: Vec<String> = book
        .keys()
        .filter(|id| !shipped(id))
        .filter(|id| list.iter().any(|w| &w.id == *id && w.kind == Kind::App))
        .cloned()
        .collect();
    for id in dropped {
        list.retain(|w| w.id != id);
        book.remove(&id);
        out.changed.push(id);
    }
    for v in vendor {
        let id = &v.workspace.id;
        let lost_app = v.workspace.kind == Kind::App && !list.iter().any(|x| &x.id == id);
        if book.get(id) == Some(&v.digest) && !lost_app {
            continue;
        }
        let mut next = list.clone();
        let mut w = v.workspace.clone();
        match next.iter_mut().find(|x| &x.id == id) {
            Some(old) => {
                w.projects = std::mem::take(&mut old.projects);
                *old = w;
            }
            None => next.push(w),
        }
        match workspaces::check_workspaces(next.iter().map(workspaces::to_yaml).collect()) {
            Ok(checked) => {
                *list = checked;
                book.insert(id.clone(), v.digest.clone());
                out.changed.push(id.clone());
            }
            Err(e) => out.refused.push((id.clone(), e.to_string())),
        }
    }
    out
}

#[cfg(test)]
mod tests;
