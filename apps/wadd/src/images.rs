//! The images on this machine (the base, and sets: sets.rs), and what
//! wadspaces start with.
//!
//! - `GET /v1/images`: each set here, from its labels: its key and the id of
//!   the image it was built on. WadSpaces compares those with the wadspace
//!   as it is now to show Built, Update, or Not built.
//! - `DELETE /v1/images/{layer}`: a set nothing needs. Refused while a
//!   workspace here runs on it.
//! - `PUT /v1/wadspaces/{id}`: a version 4 wadspace installed, or updated,
//!   as a workspace on its set (sets.rs).
//! - `PUT /v1/workspaces/{id}/wallpaper`: the wallpaper it's started with,
//!   kept in its extra folder (mounted at /run/wadspaces-extra).

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use wad_proto::v1::{Freed, ImageRow, StorageDelete, WallpaperUpload, Workspace};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::{Backend, ImageInfo};
use crate::builds::{Builds, FROM_LABEL, LAYER_LABEL, SHA_LABEL};
use crate::registry::Registry;

/// A wallpaper's size limit.
pub const MAX_WALLPAPER: usize = 16 << 20;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, e.to_string())
}

/// A set's "set:<key>".
pub fn layer_ok(layer: &str) -> bool {
    layer
        .strip_prefix("set:")
        .is_some_and(|key| key.len() == 12 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The image a layer's name points at.
pub fn layer_ref(layer: &str) -> String {
    wad_core::v4::image_ref(layer, wad_core::spec::DEFAULT_BASE_IMAGE)
}

/// The set a workspace's image is ("set:<key>"), if it's one.
pub fn layer_of(image: &str) -> Option<String> {
    let key = image.strip_prefix("localhost/wadspaces-set-")?.strip_suffix(":latest")?;
    Some(format!("set:{key}")).filter(|l| layer_ok(l))
}

/// The layer images here.
pub async fn list(backend: &dyn Backend) -> Result<Vec<ImageRow>, ApiError> {
    let found = backend.images_labelled(SHA_LABEL).await.map_err(internal)?;
    let mut rows: Vec<ImageRow> = found
        .into_iter()
        .filter_map(|i| {
            let layer = i.labels.get(LAYER_LABEL).and_then(Value::as_str).filter(|l| layer_ok(l))?.to_string();
            let image_ref = layer_ref(&layer);
            let pkgs = i.labels.get(crate::sets::PKGS_LABEL).and_then(Value::as_str).unwrap_or_default();
            // Only the image the layer's name points at (not an older build).
            i.tags.iter().any(|t| t == &image_ref).then(|| ImageRow {
                dockerfile_sha: i.labels.get(SHA_LABEL).and_then(Value::as_str).unwrap_or_default().into(),
                from: i.labels.get(FROM_LABEL).and_then(Value::as_str).unwrap_or_default().into(),
                pkgs: pkgs.split(',').filter(|p| !p.is_empty()).map(String::from).collect(),
                layer,
                image_ref,
                id: i.id,
                built_at: i.created,
                size_mb: (i.size as f64 / 1_000_000.0).round(),
            })
        })
        .collect();
    rows.sort_by(|a, b| a.layer.cmp(&b.layer));
    // The base too (layer "base"): a template built on an older one has an update.
    let base = wad_core::spec::DEFAULT_BASE_IMAGE;
    if let Ok(Some(i)) = backend.image_info(base).await {
        rows.push(ImageRow {
            layer: "base".into(),
            image_ref: base.into(),
            id: i.id,
            dockerfile_sha: String::new(),
            built_at: i.created,
            size_mb: (i.size as f64 / 1_000_000.0).round(),
            from: String::new(),
            pkgs: vec![],
        });
    }
    Ok(rows)
}

/// Removes a layer's image; not while a workspace runs on it, or it builds.
pub async fn delete(registry: &Registry, builds: &Builds, backend: &dyn Backend, layer: &str) -> Result<(), ApiError> {
    if !layer_ok(layer) {
        return Err(bad(format!("{layer:?} isn't a layer")));
    }
    let image = layer_ref(layer);
    let users: Vec<String> = registry.workspaces().into_iter().filter(|w| w.image == image).map(|w| w.name).collect();
    if !users.is_empty() {
        return Err(ApiError::new(ErrorCode::Conflict, format!("{} run on it; delete them first", users.join(", "))));
    }
    if builds.building(layer) {
        return Err(ApiError::new(ErrorCode::Conflict, "it's building"));
    }
    if !backend.remove_image(&image).await.map_err(internal)? {
        return Err(ApiError::new(ErrorCode::NotFound, format!("no image {image}")));
    }
    tracing::info!("removed the image {image}");
    Ok(())
}

/// Every layer image here, old builds too, with its layer (from its label).
async fn layer_images(backend: &dyn Backend) -> Result<Vec<(String, ImageInfo)>, ApiError> {
    Ok(backend
        .images_labelled(SHA_LABEL)
        .await
        .map_err(internal)?
        .into_iter()
        .filter_map(|i| {
            Some((i.labels.get(LAYER_LABEL).and_then(Value::as_str).filter(|l| layer_ok(l))?.to_string(), i))
        })
        .collect())
}

fn from_of(i: &ImageInfo) -> &str {
    i.labels.get(FROM_LABEL).and_then(Value::as_str).unwrap_or_default()
}

/// Whether it's the build its layer's name points at (else an older one).
fn current(layer: &str, i: &ImageInfo) -> bool {
    i.tags.contains(&layer_ref(layer))
}

/// What an image adds to the one it was built on, in bytes.
async fn own_bytes(backend: &dyn Backend, i: &ImageInfo) -> u64 {
    let under = match backend.image_info(from_of(i)).await {
        Ok(Some(u)) => u.size,
        _ => 0,
    };
    i.size.saturating_sub(under)
}

/// What's picked, off this machine's disk (POST /v1/storage/delete):
/// wadspaces first (each taken off this machine with its files: its home
/// and the copies only it had), then each layer's image and its older
/// builds, the ones built on others first. A layer that something else
/// here is built on loses its name only: its files stay until that goes
/// too (`kept`). Not while a picked wadspace runs, a picked layer builds,
/// or a wadspace that isn't picked runs on a picked layer's image. The base
/// isn't a layer: it stays.
pub async fn delete_selected(
    registry: &Registry,
    builds: &Builds,
    copies: &crate::copies::Copies,
    backend: &dyn Backend,
    state_dir: &Path,
    pick: &StorageDelete,
) -> Result<Freed, ApiError> {
    if let Some(l) = pick.layers.iter().find(|l| !layer_ok(l)) {
        return Err(bad(format!("{l:?} isn't a layer")));
    }
    let installed = registry.workspaces();
    let picked: Vec<&Workspace> = pick
        .wadspaces
        .iter()
        .map(|id| {
            installed
                .iter()
                .find(|w| &w.id == id)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("{id:?} isn't on this machine")))
        })
        .collect::<Result<_, _>>()?;
    let runs = |w: &Workspace| registry.state(&w.id).is_some_and(|s| s.container == "running");
    if let Some(w) = picked.iter().find(|w| runs(w)) {
        return Err(ApiError::new(ErrorCode::Conflict, format!("{} is running: stop it first", w.name)));
    }
    if let Some(l) = pick.layers.iter().find(|l| builds.building(l)) {
        return Err(ApiError::new(ErrorCode::Conflict, format!("{l} is building")));
    }
    for l in &pick.layers {
        let image = layer_ref(l);
        let on: Vec<&str> = installed
            .iter()
            .filter(|w| w.image == image && !pick.wadspaces.contains(&w.id))
            .map(|w| w.name.as_str())
            .collect();
        if !on.is_empty() {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!(
                    "{} {} on {l}: pick {} too",
                    on.join(", "),
                    if on.len() == 1 { "runs" } else { "run" },
                    if on.len() == 1 { "it" } else { "them" }
                ),
            ));
        }
    }
    let mut freed = Freed::default();
    // Wadspaces: off the machine, their home and the copies no one else has.
    let homes = backend.volume_sizes().await.unwrap_or_default();
    for w in &picked {
        registry.remove_workspace(&w.id).await?;
        let volume = format!("wad-{}-config", w.id);
        freed.bytes += homes.iter().find(|(n, _)| *n == volume).map(|(_, b)| *b).unwrap_or(0);
        if let Err(e) = backend.remove_volume(&volume).await {
            tracing::warn!("{volume} stays: {e}");
        }
        for p in &w.projects {
            if copies.delete(&p.id).is_ok() {
                tracing::info!("removed the copy {} ({} had it)", p.id, w.name);
            }
        }
        freed.uninstalled.push(w.name.clone());
    }
    // Layers: each one's builds, those built on others first.
    let all = layer_images(backend).await?;
    let depth = |l: &str| {
        let mut n = 0;
        let mut at = all.iter().find(|(x, i)| x == l && current(x, i)).map(|(_, i)| from_of(i).to_string());
        while let Some(id) = at {
            n += 1;
            at = all.iter().find(|(_, i)| i.id == id).map(|(_, i)| from_of(i).to_string());
        }
        n
    };
    let mut layers: Vec<&String> = pick.layers.iter().collect();
    layers.sort_by_key(|l| std::cmp::Reverse(depth(l)));
    for l in layers {
        let mut builds_of: Vec<&ImageInfo> = all.iter().filter(|(x, _)| x == l).map(|(_, i)| i).collect();
        // The older builds first, then the one its name points at.
        builds_of.sort_by_key(|i| current(l, i));
        for i in builds_of {
            let bytes = own_bytes(backend, i).await;
            match backend.remove_image_id(&i.id).await {
                Ok(true) => {
                    freed.bytes += bytes;
                    freed.removed.push(if current(l, i) { l.clone() } else { i.id.clone() });
                }
                Ok(false) => {}
                // Something here is built on it: the name goes, the files stay with that.
                Err(_) if current(l, i) => {
                    backend.remove_image(&layer_ref(l)).await.map_err(internal)?;
                    freed.kept.push(l.clone());
                }
                Err(e) => tracing::info!("an old build of {l} stays ({}): {e}", i.id),
            }
        }
    }
    // Other images: each on its own (podman refuses one something needs).
    // Packages and the chains sets are built from stay (they're cleared
    // up with the sets that use them).
    let sizes = backend.system_df().await.unwrap_or_default();
    let all_images = backend.images_all().await.unwrap_or_default();
    let short = |id: &str| id.trim_start_matches("sha256:").to_string();
    for id in &pick.images {
        if all_images.iter().any(|i| short(&i.id) == short(id) && crate::sets::is_assembly_part(&i.labels)) {
            freed.failed.push(format!(
                "image {}: a package's, or a step sets are built from: it stays",
                &id[..id.len().min(12)]
            ));
            continue;
        }
        let bytes = sizes
            .images
            .iter()
            .find(|i| i.id.trim_start_matches("sha256:") == id.trim_start_matches("sha256:"))
            .map(|i| i.unique)
            .unwrap_or(0);
        match backend.remove_image_id(id).await {
            Ok(true) => {
                freed.bytes += bytes;
                freed.removed.push(id.clone());
            }
            Ok(false) => {}
            // A step another image is built from stays with that image.
            Err(e) if e.contains("dependent child") => freed.kept.push(format!("image {}", &id[..id.len().min(12)])),
            Err(e) => freed.failed.push(format!("image {}: {e}", &id[..id.len().min(12)])),
        }
    }
    // Volumes no wadspace here has.
    for name in &pick.volumes {
        if installed.iter().any(|w| format!("wad-{}-config", w.id) == *name) {
            freed.failed.push(format!("{name} is a wadspace's home here: delete the wadspace instead"));
            continue;
        }
        let bytes = sizes.volumes.iter().find(|v| v.name == *name).map(|v| v.size).unwrap_or(0);
        match backend.remove_volume(name).await {
            Ok(true) => {
                freed.bytes += bytes;
                freed.removed.push(name.clone());
            }
            Ok(false) => {}
            Err(e) => freed.failed.push(format!("{name}: {e}")),
        }
    }
    // The copy a stick update left, once it's applied.
    if pick.update_copy {
        let dir = state_dir.join(crate::storage::UPDATE_COPY);
        if dir.join("pending").exists() {
            freed.failed.push("the stick update's copy is waiting to be applied at the next start".into());
        } else {
            let bytes = crate::storage::dir_bytes(&dir);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    freed.bytes += bytes;
                    freed.removed.push("the last stick update's copy".into());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => freed.failed.push(format!("the update copy: {e}")),
            }
        }
    }
    tracing::info!(?freed, "removed from the disk");
    Ok(freed)
}

/// Images nothing here needs, removed (POST /v1/storage/prune, and after
/// the builds): the tidy-up's (tidy.rs), when `wanted` says what's needed
/// (None while builds run), and older bases. One a container still runs on
/// stays (for next time).
pub async fn prune(backend: &dyn Backend, wanted: Option<&crate::tidy::Wanted>) -> Result<Freed, ApiError> {
    let mut freed = match wanted {
        Some(w) => crate::tidy::run(backend, w).await,
        None => Freed::default(),
    };
    // Older bases (a stick update brought a newer one) nothing stands on any
    // more, and steps a build left on the base (they carry its labels). Each
    // frees what only it had: a step's layers are its image's, so next to
    // nothing.
    let olds = old_bases(backend).await;
    let df = if olds.is_empty() { Default::default() } else { backend.system_df().await.unwrap_or_default() };
    let short = |id: &str| id.trim_start_matches("sha256:").to_string();
    for i in olds {
        let bytes = df.images.iter().find(|d| short(&d.id) == short(&i.id)).map_or(i.size, |d| d.unique);
        match backend.remove_image_id(&i.id).await {
            Ok(true) => {
                freed.bytes += bytes;
                freed.removed.push(i.id);
            }
            Ok(false) => {}
            Err(e) => tracing::info!("an older base stays for now ({}): {e}", i.id),
        }
    }
    Ok(freed)
}

/// The stream sidecar's image (images/stream; wad-config's default).
const STREAM_IMAGE: &str = "localhost/wadspaces-stream:trixie";

/// Bases that aren't the base any more (localhost/wadspaces-base:trixie
/// names a newer one) and that no image here is built on, and stream
/// sidecars a newer one replaced. A layer's image is never one: it inherits
/// its base's build_version label, but it has its own layer label (counting
/// it made the tidy-up after a build delete the very images it built, those
/// no other image is built on); nor is a chain, a set or a package
/// (sets.rs's labels).
async fn old_bases(backend: &dyn Backend) -> Vec<ImageInfo> {
    let base = wad_core::spec::DEFAULT_BASE_IMAGE;
    let Ok(all) = backend.images_all().await else { return vec![] };
    let short = |id: &str| id.trim_start_matches("sha256:").to_string();
    let under: std::collections::HashSet<String> = all.iter().map(|i| short(from_of(i))).collect();
    let version = |i: &ImageInfo| i.labels.get("build_version").and_then(Value::as_str).unwrap_or_default().to_string();
    all.into_iter()
        .filter(|i| {
            let is_base = version(i).contains("WadSpaces base");
            // An older stream sidecar (a newer one took its name).
            let old_stream = version(i).contains("WadSpaces stream") && !i.tags.iter().any(|t| t == STREAM_IMAGE);
            let layer = i.labels.contains_key(LAYER_LABEL)
                || i.labels.contains_key(SHA_LABEL)
                || crate::sets::is_assembly_part(&i.labels);
            ((is_base && !i.tags.iter().any(|t| t == base)) || old_stream) && !layer && !under.contains(&short(&i.id))
        })
        .collect()
}

/// PUT /v1/wadspaces/{id}: a version 4 wadspace (sets.rs installs it).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Install {
    pub wadspace: Value,
    /// The time zone its clock shows (the client's).
    #[serde(default)]
    pub tz: Option<String>,
}

/// `<state_dir>/extra/<id>`: what's mounted in at /run/wadspaces-extra.
pub fn extra_dir(state_dir: &Path, id: &str) -> PathBuf {
    state_dir.join("extra").join(id)
}

/// Keeps a picture as the wadspace's wallpaper.
pub fn put_wallpaper(state_dir: &Path, ws: &Workspace, up: &WallpaperUpload) -> Result<String, ApiError> {
    if ws.kind != wad_proto::v1::Kind::Container {
        return Err(bad(format!("{} is an app: it has no wallpaper", ws.name)));
    }
    let bytes = decode_wallpaper(up)?;
    write_wallpaper(state_dir, &ws.id, &up.ext, &bytes).map_err(internal)
}

/// A wallpaper's picture: a png or a jpg, 16 MB at most.
pub fn decode_wallpaper(up: &WallpaperUpload) -> Result<Vec<u8>, ApiError> {
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(&up.data).map_err(|e| bad(format!("wallpaper: {e}")))?;
    if bytes.len() > MAX_WALLPAPER {
        return Err(bad("the wallpaper is too big (16 MB max)"));
    }
    let ok = match up.ext.as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        _ => return Err(bad("a wallpaper is a png or a jpg")),
    };
    if !ok {
        return Err(bad(format!("that isn't a {} picture", up.ext)));
    }
    Ok(bytes)
}

/// Writes it (and drops the other kind), readable inside.
pub fn write_wallpaper(state_dir: &Path, id: &str, ext: &str, bytes: &[u8]) -> std::io::Result<String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = extra_dir(state_dir, id);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))?;
    let path = dir.join(format!("wallpaper.{ext}"));
    let tmp = dir.join(format!(".wallpaper.{ext}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, &path)?;
    for other in ["png", "jpg"].iter().filter(|e| **e != ext) {
        let _ = std::fs::remove_file(dir.join(format!("wallpaper.{other}")));
    }
    crate::projects::relabel(&dir, false);
    Ok(path.to_string_lossy().into_owned())
}

/// Makes sure the extra folder is there (the unit mounts it).
pub fn ensure_extra(state_dir: &Path, id: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = extra_dir(state_dir, id);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
}
