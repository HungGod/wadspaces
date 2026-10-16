//! Files copied into wadspaces. Nothing on the machine is mounted into a
//! wadspace for it to work in: wadd makes a copy of the files here, at
//! `<state_dir>/copies/<id>/` (`files/`, and `done` once it's whole), and
//! each wadspace that has it copies it into its own /config the first time it
//! starts (the image's wadspaces-projects), from a read-only mount of that
//! folder (wad-core's generator).
//!
//! - `POST /v1/copies`: a copy of paths on this machine, inside the folder
//!   roots: one folder's contents, or each path (file or folder) by its name.
//! - A copy this machine hasn't made yet (a folder or drive project's, from
//!   before copies) is made when a wadspace that has it launches here, from
//!   where its files are, when they're here ([`Copies::stage_from`]).
//! - A GitHub project wadd cloned on this machine (before wadspaces cloned
//!   their own) moves in the same way ([`Copies::seed_from_clone`]): the
//!   wadspace starts from that clone, uncommitted work and all, instead of
//!   cloning afresh.
//! - `DELETE /v1/copies/{id}`: when no wadspace uses it any more (WadSpaces
//!   knows); refused while one here does.
//!
//! A mount (a project the user chose to mount rather than copy) is the one
//! exception: the machine's own folder or file, mounted read-write, found
//! here by [`Copies::find_mounts`] just before its wadspace starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wad_proto::v1::{CopyFrom, CopyMade, DriveRef, ProjectKind, Workspace, WorkspaceProject};
use wad_proto::{ApiError, ErrorCode};

use crate::drives::{self, Drives};
use crate::projects::MachineRef;
use crate::registry::{Mounts, Registry};

/// Room left on the disk after a copy.
const MARGIN_BYTES: u64 = 1 << 30;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, e.to_string())
}

/// A workspace's projects that are here: every one but a mount that wasn't
/// found on this machine (what its projects.json lists).
pub fn projects_here(ws: &Workspace, mounts: &Mounts) -> Vec<WorkspaceProject> {
    ws.projects.iter().filter(|p| p.kind != ProjectKind::Mount || mounts.contains_key(&p.id)).cloned().collect()
}

pub struct Copies {
    dir: PathBuf,
    roots: Vec<PathBuf>,
    uid: u32,
    machine: MachineRef,
    drives: Arc<Drives>,
    registry: Arc<Registry>,
}

impl Copies {
    pub fn new(
        state_dir: &Path,
        roots: Vec<PathBuf>,
        uid: u32,
        machine: MachineRef,
        drives: Arc<Drives>,
        registry: Arc<Registry>,
    ) -> Self {
        Self { dir: state_dir.join("copies"), roots, uid, machine, drives, registry }
    }

    /// `<state_dir>/copies/<id>`: what a wadspace sees at /run/wadspaces-copies/<id>.
    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Whether the copy is here, whole.
    pub fn ready(&self, id: &str) -> bool {
        self.path(id).join("done").is_file()
    }

    /// Before a container starts: a folder (empty until there's a copy) for
    /// each of its projects, which its unit mounts (a mount has none).
    pub fn prepare(&self, ws: &Workspace) -> Result<(), String> {
        for p in ws.projects.iter().filter(|p| p.kind != ProjectKind::Mount) {
            let path = self.path(&p.id);
            if !path.exists() {
                std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        Ok(())
    }

    /// Where a workspace's mounts are on this machine: a drive's folder (the
    /// drive mounted first), or a path on this machine inside the folder
    /// roots. One from another machine is left out; one that should be here
    /// but isn't stops the start, so the wadspace never gets an empty folder
    /// in its place.
    pub async fn find_mounts(&self, ws: &Workspace) -> Result<Mounts, String> {
        let mut found = Mounts::new();
        for p in ws.projects.iter().filter(|p| p.kind == ProjectKind::Mount) {
            let Some(path) = self.mount_of(p).await? else { continue };
            found.insert(p.id.clone(), path.to_string_lossy().into_owned());
        }
        Ok(found)
    }

    /// Where one mount is here (None: on another machine).
    async fn mount_of(&self, p: &WorkspaceProject) -> Result<Option<PathBuf>, String> {
        let as_ = &p.mount;
        let from = p.from.as_ref().ok_or_else(|| format!("{as_} says nothing about what it mounts"))?;
        let first = from.paths.first().map(String::as_str).unwrap_or("");
        // A folder on a drive wadd mounted when it was picked: the drive's
        // mounted again first, wherever the folder was picked.
        let in_drive = first
            .strip_prefix(drives::DRIVES_DIR)
            .and_then(|r| r.strip_prefix('/'))
            .map(|r| r.split_once('/').unwrap_or((r, "")))
            .filter(|(uuid, _)| wad_core::projects::is_drive_uuid(uuid));
        let drive = from.drive.clone().or_else(|| {
            in_drive.map(|(uuid, _)| DriveRef { uuid: uuid.into(), label: String::new(), fstype: String::new() })
        });
        let first = match (&from.drive, in_drive) {
            (None, Some((_, sub))) => sub,
            _ => first,
        };
        let path = match &drive {
            Some(d) => {
                let name = if d.label.is_empty() { &d.uuid } else { &d.label };
                let mp = self
                    .drives
                    .mount(&d.uuid, &d.label, &d.fstype)
                    .await
                    .map_err(|e| format!("{as_} is on the drive {name}: plug it in ({e})"))?;
                let sub = wad_store::folders::clean_subpath(first).map_err(|e| e.to_string())?;
                drives::folder_on(&mp, &sub).ok_or_else(|| format!("{as_}: {sub} is outside the drive {name}"))?
            }
            None => {
                if !self.is_here(&from.machine_id) {
                    return Ok(None);
                }
                PathBuf::from(
                    wad_store::folders::resolve_folder(first, &self.roots, true).map_err(|e| format!("{as_}: {e}"))?,
                )
            }
        };
        if std::fs::metadata(&path).is_err() {
            return Err(format!("{as_} mounts {}, which isn't there", path.display()));
        }
        Ok(Some(path))
    }

    /// Whether a machine id is this machine.
    pub fn is_here(&self, machine_id: &str) -> bool {
        let (here, _) = (self.machine)();
        machine_id == here || (here == wad_store::projects::LOCAL && machine_id.is_empty())
    }

    /// A copy of these paths on this machine (POST /v1/copies).
    pub async fn make(&self, paths: &[String]) -> Result<CopyMade, ApiError> {
        if paths.is_empty() || paths.len() > 64 {
            return Err(bad("copy one folder, or up to 64 files and folders"));
        }
        let mut real = vec![];
        for p in paths {
            let r = wad_store::folders::resolve_folder(p, &self.roots, true).map_err(|e| bad(e.to_string()))?;
            if std::fs::symlink_metadata(&r).is_err() {
                return Err(bad(format!("{p} isn't there")));
            }
            real.push(PathBuf::from(r));
        }
        let id = format!("copy-{}", wad_store::projects::new_id()[..12].to_lowercase());
        self.copy_into(&id, &real).await
    }

    /// Makes a copy this machine hasn't got from where its files are, when
    /// they're here: its line for the launch's log.
    pub async fn stage_from(&self, id: &str, from: &CopyFrom) -> Result<CopyMade, String> {
        let source: PathBuf = match &from.drive {
            Some(d) => {
                let name = if d.label.is_empty() { &d.uuid } else { &d.label };
                let mp = self.drives.mount(&d.uuid, &d.label, &d.fstype).await.map_err(|e| e.to_string())?;
                let sub = from.paths.first().map(String::as_str).unwrap_or("");
                let sub = wad_store::folders::clean_subpath(sub).map_err(|e| e.to_string())?;
                drives::folder_on(&mp, &sub).ok_or_else(|| format!("{sub} is outside the drive {name}"))?
            }
            None => {
                if !self.is_here(&from.machine_id) {
                    return Err("its files are on another machine: it's copied in when it starts there".into());
                }
                let p = from.paths.first().ok_or("it names no files")?;
                PathBuf::from(wad_store::folders::resolve_folder(p, &self.roots, true).map_err(|e| e.to_string())?)
            }
        };
        if !source.is_dir() {
            return Err(format!("{} isn't there", source.display()));
        }
        self.copy_into(id, &[source]).await.map_err(|e| e.message)
    }

    /// A GitHub project's clone that wadd kept on this machine, moved in as
    /// its copy (so the wadspace starts from it). Whether there was one.
    pub async fn seed_from_clone(&self, id: &str, clone: &Path) -> Result<bool, String> {
        if self.ready(id) || !clone.join(".git").exists() {
            return Ok(false);
        }
        let part = self.dir.join(format!("{id}.part"));
        let _ = std::fs::remove_dir_all(&part);
        std::fs::create_dir_all(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let files = part.join("files");
        if std::fs::rename(clone, &files).is_err() {
            // Another filesystem: copied, and the clone left where it was.
            run_cp(&[clone.join(".")], &files).await?;
        }
        self.finish(id, &part).await.map(|_| true)
    }

    /// Removes a copy (DELETE /v1/copies/{id}), unless a wadspace here has it.
    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        if !wad_core::projects::is_project_id(id) {
            return Err(bad(format!("{id:?} isn't a copy")));
        }
        let users: Vec<String> = self
            .registry
            .workspaces()
            .into_iter()
            .filter(|w| w.projects.iter().any(|p| p.id == id))
            .map(|w| w.name)
            .collect();
        if !users.is_empty() {
            return Err(ApiError::new(ErrorCode::Conflict, format!("{} still has it", users.join(", "))));
        }
        match std::fs::remove_dir_all(self.path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }

    /// Copies `sources` into copy `id`: one folder's contents, else each by
    /// its name. Made beside it, then put in place whole.
    async fn copy_into(&self, id: &str, sources: &[PathBuf]) -> Result<CopyMade, ApiError> {
        let need: u64 = {
            let s = sources.to_vec();
            tokio::task::spawn_blocking(move || s.iter().map(|p| size_of(p).1).sum()).await.map_err(internal)?
        };
        std::fs::create_dir_all(&self.dir).map_err(internal)?;
        if let Some(free) = free_bytes(&self.dir)
            && need + MARGIN_BYTES > free
        {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("not enough room: that's {} MB, and {} MB are free", need >> 20, free >> 20),
            ));
        }
        let part = self.dir.join(format!("{id}.part"));
        let _ = std::fs::remove_dir_all(&part);
        let files = part.join("files");
        std::fs::create_dir_all(&files).map_err(internal)?;
        let from: Vec<PathBuf> = match sources {
            [one] if one.is_dir() => vec![one.join(".")],
            many => many.to_vec(),
        };
        if let Err(e) = run_cp(&from, &files).await {
            let _ = std::fs::remove_dir_all(&part);
            return Err(internal(e));
        }
        self.finish(id, &part).await.map_err(internal)
    }

    /// A copy made beside its place: counted, the projects user's, and put
    /// in place with its `done`.
    async fn finish(&self, id: &str, part: &Path) -> Result<CopyMade, String> {
        let files = part.join("files");
        let f = files.clone();
        let (count, bytes) = tokio::task::spawn_blocking(move || size_of(&f)).await.map_err(|e| e.to_string())?;
        let made = CopyMade { id: id.into(), files: count, bytes };
        std::fs::write(part.join("done"), serde_json::to_string(&made).expect("json") + "\n")
            .map_err(|e| e.to_string())?;
        if nix::unistd::geteuid().is_root() {
            let out = tokio::process::Command::new("chown")
                .args(["-R", "--no-dereference", &format!("{0}:{0}", self.uid), "--"])
                .arg(part)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("chown: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
        }
        let dest = self.path(id);
        // The empty folder a unit mounted before there was a copy.
        let _ = std::fs::remove_dir(&dest);
        std::fs::rename(part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        tracing::info!("copy {id}: {} files, {} MB", made.files, made.bytes >> 20);
        Ok(made)
    }
}

/// cp -a (reflinked where the filesystem can) of `from` into `to`.
async fn run_cp(from: &[PathBuf], to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let out = tokio::process::Command::new("cp")
        .args(["-a", "--reflink=auto", "--"])
        .args(from)
        .arg(to)
        .output()
        .await
        .map_err(|e| format!("cp: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("copying: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// Files (anything not a folder) and their bytes under `path`, links not followed.
fn size_of(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return (0, 0) };
    if !meta.is_dir() {
        return (1, meta.len());
    }
    let mut out = (0, 0);
    for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
        let (f, b) = size_of(&e.path());
        out = (out.0 + f, out.1 + b);
    }
    out
}

fn free_bytes(path: &Path) -> Option<u64> {
    let s = nix::sys::statvfs::statvfs(path).ok()?;
    Some(s.blocks_available() as u64 * s.fragment_size() as u64)
}
