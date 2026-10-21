//! Version 4 wadspaces (wad_core::v4) on this machine: installed, and their
//! images built.
//!
//! A version 4 wadspace's image is a set: the base with its packages copied
//! on, shared by every wadspace with the same packages. A set's build gets
//! them (packages.rs: here, downloaded, or built here), stacks them as chain
//! images, each one package on the chain under it and named by the base and
//! the packages so far (wad_core::packages::chain_ref), then runs
//! `wadspaces-pkg assemble` on the top one. It starts from the longest
//! chain stored with a bottom it shares, so taking an app out copies again
//! only what was above it, and a second wadspace with the same packages
//! builds nothing.
//!
//! A set is labelled as a layer ("set:<key>": /v1/images lists it) with the
//! base it was built on (a newer base makes it an update), its packages
//! (`io.wadspaces.pkgs`) and `io.wadspaces.assembly`; a chain carries
//! `io.wadspaces.chain`. Both inherit the base's labels, so the tidy-up
//! after builds tells them from older bases by these.
//!
//! Installing one: its workspace on its set (or the base, with no
//! packages), and what it takes when it starts in its extra folder
//! (/run/wadspaces-extra, which the base's init-wadspaces-workspace
//! applies): its web apps and their icons, its desktop's layout,
//! WadBrowser's mode and its background, drawn in the machine's theme. So
//! changing those never builds. A copy of the document stays in
//! `<state_dir>/wadspaces/<id>.json` for its build and a change of theme.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use wad_core::packages as pk;
use wad_core::v4;
use wad_proto::v1::{SetBuild, Theme, WorkspaceSaved};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::{Backend, ImageInfo};
use crate::builds::{Builds, FROM_LABEL, LAYER_LABEL, SHA_LABEL};
use crate::icons::IconJob;
use crate::packages::{Got, Watch};
use crate::registry::Registry;

/// A chain image's packages so far (`pkgs_label`'s form).
pub const CHAIN_LABEL: &str = "io.wadspaces.chain";
/// A set's packages, as it stacks them.
pub const PKGS_LABEL: &str = "io.wadspaces.pkgs";
/// A set: packages put together.
pub const ASSEMBLY_LABEL: &str = "io.wadspaces.assembly";
/// A base that takes a wadspace's extras when it starts (images/base).
pub const EXTRAS_LABEL: &str = "io.wadspaces.extras";

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, e.to_string())
}

fn label<'a>(i: &'a ImageInfo, k: &str) -> &'a str {
    i.labels.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// A package's, a chain's or a set's image: never an older base, and
/// never deleted on its own.
pub fn is_assembly_part(labels: &Map<String, Value>) -> bool {
    [crate::packages::PKG_LABEL, CHAIN_LABEL, ASSEMBLY_LABEL].iter().any(|k| labels.contains_key(*k))
}

/// A set to build: `packages` (v4::packages_of) on `base`.
pub struct SetWork {
    /// "set:<key>"
    pub key: String,
    pub packages: Vec<String>,
    pub base: String,
}

/// The base, if sets can be built on it.
async fn base_for_sets(backend: &dyn Backend, base: &str) -> Result<ImageInfo, String> {
    let Some(info) = backend.image_info(base).await? else {
        return Err(format!("{base} is not on this machine — update the drive with host/build.sh update --bases"));
    };
    if !info.labels.contains_key(EXTRAS_LABEL) {
        return Err(format!("{base} is from before sets: update the base (images/build.sh, or the update drive)"));
    }
    Ok(info)
}

/// Chains stored on this base made of the packages as `got` has them: each
/// one's packages, bottom first.
async fn stored_chains(backend: &dyn Backend, base_id: &str, got: &[Got]) -> Result<Vec<Vec<String>>, String> {
    let mut out = vec![];
    for i in backend.images_labelled(CHAIN_LABEL).await? {
        // A set inherits its top chain's label.
        if i.labels.contains_key(LAYER_LABEL) {
            continue;
        }
        let pairs: Option<Vec<(String, String)>> = label(&i, CHAIN_LABEL)
            .split(',')
            .map(|p| {
                let (id, tag) = p.split_once('@')?;
                let r = pk::local_ref(id, tag);
                got.iter().any(|g| g.id == id && g.image == r).then(|| (id.to_string(), r))
            })
            .collect();
        let Some(pairs) = pairs.filter(|p| !p.is_empty()) else { continue };
        if i.tags.contains(&pk::chain_ref(base_id, &pairs)) {
            out.push(pairs.into_iter().map(|(id, _)| id).collect());
        }
    }
    Ok(out)
}

/// A set's packages as they come: the first part of its job's bar.
struct SetWatch<'a> {
    b: &'a Builds,
    id: &'a str,
    n: usize,
    done: usize,
    groups: u32,
}

impl Watch for SetWatch<'_> {
    fn line(&mut self, line: &str) {
        self.b.line(self.id, line);
    }
    fn bytes(&mut self, _id: &str, done: u64, total: u64) {
        let part = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        self.b.at(self.id, 0, self.groups, (self.done as f64 + part) / self.n.max(1) as f64);
    }
    fn got(&mut self, _got: &Got) {
        self.done += 1;
        self.b.at(self.id, 0, self.groups, self.done as f64 / self.n.max(1) as f64);
    }
}

/// One image from a Containerfile, its log lines in the job's.
async fn podman(
    b: &Builds,
    id: &str,
    file: &str,
    tag: &str,
    from: &str,
    labels: Map<String, Value>,
) -> Result<(), String> {
    let context = wad_core::tar::tar(&[wad_core::tar::Entry { path: "Dockerfile", content: file.as_bytes() }])?;
    let me = b.me().ok_or("wadd is stopping")?;
    let line_id = id.to_string();
    b.backend().build(context, tag, from, labels, Box::new(move |l| me.line(&line_id, l))).await
}

/// A set's build (a job of builds.rs): its tag when it's done.
pub(crate) async fn build(b: &Builds, id: &str, w: SetWork) -> Result<String, String> {
    let backend = b.backend().clone();
    let tag = v4::image_ref(&w.key, &w.base);
    let lock = b.image_lock(&tag);
    let _held = lock.lock().await;
    let base = base_for_sets(backend.as_ref(), &w.base).await?;
    if backend.image_info(&tag).await?.is_some_and(|i| label(&i, FROM_LABEL) == base.id) {
        b.line(id, "» built already, on this base");
        return Ok(tag);
    }
    let packages = b.packages().ok_or("packages aren't set up on this machine")?;
    let n = w.packages.len();
    b.line(id, &format!("» {n} package{}: {}", if n == 1 { "" } else { "s" }, w.packages.join(", ")));
    let groups = n as u32 + 2;
    let mut watch = SetWatch { b, id, n, done: 0, groups };
    let got = packages.ensure(&w.packages, &w.base, &mut watch).await.map_err(|e| e.message)?;

    // The order that copies least: the longest chain here first.
    let stored = stored_chains(backend.as_ref(), &base.id, &got).await?;
    let ids: Vec<String> = got.iter().map(|g| g.id.clone()).collect();
    let pairs: Vec<(String, String)> = pk::stack_order(&ids, &stored)
        .into_iter()
        .filter_map(|p| got.iter().find(|g| g.id == p).map(|g| (g.id.clone(), g.image.clone())))
        .collect();
    let mut have = 0;
    for i in (0..pairs.len()).rev() {
        if backend.image_exists(&pk::chain_ref(&base.id, &pairs[..=i])).await? {
            have = i + 1;
            break;
        }
    }
    let groups = (pairs.len() - have) as u32 + 2;
    if have > 0 {
        b.line(id, &format!("» {have} of them stacked already, by an earlier build"));
    }
    let (mut from, mut from_id) = if have == 0 {
        (w.base.clone(), base.id.clone())
    } else {
        let top = pk::chain_ref(&base.id, &pairs[..have]);
        let top_id = backend.image_info(&top).await?.map(|i| i.id).ok_or(format!("{top} went away"))?;
        (top, top_id)
    };
    for i in have..pairs.len() {
        b.at(id, 1 + (i - have) as u32, groups, 0.0);
        let chain = &pairs[..=i];
        let ctag = pk::chain_ref(&base.id, chain);
        let (pid, pref) = &pairs[i];
        b.line(id, &format!("» stacking {pid}"));
        let mut labels = Map::new();
        labels.insert(FROM_LABEL.into(), from_id.clone().into());
        labels.insert(CHAIN_LABEL.into(), pk::pkgs_label(chain).into());
        podman(b, id, &pk::chain_step(&from, pref, chain), &ctag, &from, labels).await?;
        from_id = backend.image_info(&ctag).await?.map(|i| i.id).ok_or(format!("{ctag} isn't here after its build"))?;
        from = ctag;
    }
    b.at(id, groups - 1, groups, 0.0);
    b.line(id, "» putting the packages together (wadspaces-pkg assemble)");
    let mut labels = Map::new();
    labels.insert(LAYER_LABEL.into(), w.key.clone().into());
    labels.insert(SHA_LABEL.into(), w.key.trim_start_matches("set:").into());
    labels.insert(FROM_LABEL.into(), base.id.clone().into());
    labels.insert(PKGS_LABEL.into(), pk::pkgs_label(&pairs).into());
    labels.insert(ASSEMBLY_LABEL.into(), "1".into());
    let file = pk::assembled(&from, true).join("\n") + "\n";
    podman(b, id, &file, &tag, &from, labels).await?;
    Ok(tag)
}

// ------------------------------------------------------------- installing

/// What installing and building one needs of the machine.
pub struct Here<'a> {
    pub registry: &'a Registry,
    pub backend: &'a dyn Backend,
    pub builds: &'a Builds,
    pub state_dir: &'a Path,
    /// This machine's id in the account, when it's linked: a wadspace
    /// installs only on its own machine.
    pub machine: Option<String>,
}

/// Installed: the workspace, and what didn't go as it should.
pub struct Installed {
    pub saved: WorkspaceSaved,
    pub warnings: Vec<String>,
}

fn doc_path(state_dir: &Path, id: &str) -> PathBuf {
    state_dir.join("wadspaces").join(format!("{id}.json"))
}

/// The version 4 document a wadspace here was installed from.
pub fn installed_doc(state_dir: &Path, id: &str) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(doc_path(state_dir, id)).ok()?).ok()
}

/// The machine's theme (PUT /v1/machine/theme): dark unless it's light.
pub fn theme(state_dir: &Path) -> Theme {
    match std::fs::read_to_string(state_dir.join("theme")) {
        Ok(t) if t.trim() == "light" => Theme::Light,
        _ => Theme::Dark,
    }
}

/// Writes `bytes` to `dir/name` whole, readable inside.
fn put(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, dir.join(name))
}

/// PUT /v1/wadspaces/{id} with a version 4 wadspace: installed, or
/// updated, as a workspace on its set, with what it takes at start.
pub async fn install(h: &Here<'_>, id: &str, w: &Value, tz: Option<&str>) -> Result<Installed, ApiError> {
    if w.get("id").and_then(Value::as_str) != Some(id) {
        return Err(bad(format!("{id:?}: the wadspace sent is another")));
    }
    v4::check(w).map_err(bad)?;
    if let Some(m) = &h.machine
        && w["machine"].as_str() != Some(m)
    {
        return Err(ApiError::new(ErrorCode::Conflict, "that wadspace is on another machine: copy it to this one"));
    }
    if w.get("deletedAt").is_some_and(|d| !d.is_null()) {
        return Err(bad("it's deleted: restore it first"));
    }
    // Git's credentials, when the machine has them.
    let secrets: Vec<String> =
        h.backend.secret_names().await.unwrap_or_default().into_iter().filter(|n| n == "github_token").collect();
    let tz = tz.filter(|t| !t.is_empty() && t.len() < 64 && !t.contains(char::is_control)).unwrap_or("Etc/UTC");
    let opts = json!({ "tz": tz, "secrets": secrets, "base": wad_core::spec::DEFAULT_BASE_IMAGE });
    let mut ws = v4::workspace_for(w, &opts);
    ws["desktop"] = v4::desktop(w).iter().map(|i| json!({ "label": i["label"], "icon": i["iconUrl"] })).collect();
    // Its extras before the workspace: a start finds them there.
    let warnings = write_extras(h, id, w, theme(h.state_dir)).await.map_err(internal)?;
    let docs = h.state_dir.join("wadspaces");
    std::fs::create_dir_all(&docs).map_err(internal)?;
    put(&docs, &format!("{id}.json"), &serde_json::to_vec_pretty(w).map_err(internal)?).map_err(internal)?;
    let (list, updated) = crate::builds::merged(&h.registry.workspaces(), &ws).map_err(bad)?;
    let running = h.registry.state(id).is_some_and(|s| s.container == "running");
    h.registry.set_workspaces(list).await.map_err(internal)?;
    let workspace = h.registry.workspace(id)?;
    if h.backend.image_exists(&workspace.image).await.unwrap_or(false) {
        h.registry.image_here(id);
    }
    for w in &warnings {
        tracing::info!("{id}: {w}");
    }
    Ok(Installed { saved: WorkspaceSaved { workspace, restart_required: updated && running }, warnings })
}

/// Its extra folder: web apps with their icons, layout.json,
/// wadbrowser.conf and the background, in the theme. Warnings back.
async fn write_extras(h: &Here<'_>, id: &str, w: &Value, theme: Theme) -> std::io::Result<Vec<String>> {
    crate::images::ensure_extra(h.state_dir, id)?;
    let dir = crate::images::extra_dir(h.state_dir, id);
    let x = v4::extras(w);
    let webapps: Vec<Value> = x["webapps"].as_array().cloned().unwrap_or_default();
    let list: Vec<Value> =
        webapps.iter().map(|a| json!({ "id": a["id"], "name": a["name"], "url": a["url"] })).collect();
    put(&dir, "webapps.json", &serde_json::to_vec_pretty(&list)?)?;
    put(&dir, "layout.json", x["layout"].as_str().unwrap_or_default().as_bytes())?;
    match x["wadbrowserConf"].as_str() {
        Some(conf) => put(&dir, "wadbrowser.conf", conf.as_bytes())?,
        None => match std::fs::remove_file(dir.join("wadbrowser.conf")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        },
    }
    let mut warnings = vec![];
    // Web apps' icons: the site's own, the picture chosen, or the name.
    let jobs: Vec<IconJob> = webapps
        .iter()
        .filter_map(|a| {
            Some(IconJob {
                id: a["id"].as_str().filter(|i| wad_core::spec::is_id(i))?.to_string(),
                site: a["url"].as_str()?.to_string(),
                custom: a["iconUrl"].as_str().filter(|u| !u.is_empty()).map(String::from),
            })
        })
        .collect();
    let icons = dir.join("icons");
    std::fs::create_dir_all(&icons)?;
    let (made, how) = h.builds.icons().themed(&jobs, theme == Theme::Dark).await;
    if how.fallback > 0 {
        warnings.push(format!("{} web app{} got a card with its name for an icon", how.fallback, plural(how.fallback)));
    }
    for e in std::fs::read_dir(&icons)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !made.iter().any(|(i, _)| format!("{i}.png") == name) {
            std::fs::remove_file(e.path())?;
        }
    }
    for (i, png) in &made {
        put(&icons, &format!("{i}.png"), png)?;
    }
    // The background, in the theme's tone; none chosen: plain black or white.
    // The desktop (selkies-desktop) names its icons in black on a light one.
    put(&dir, "theme", if theme == Theme::Light { b"light" } else { b"dark" })?;
    let tone = if theme == Theme::Light { wad_wallpaper::Tone::Light } else { wad_wallpaper::Tone::Dark };
    let chosen = &w["customization"]["wallpaper"];
    let wp = match serde_json::from_value::<wad_wallpaper::Wallpaper>(chosen.clone()) {
        Ok(wp) => wp,
        Err(_) => wad_wallpaper::Wallpaper {
            kind: wad_wallpaper::Kind::Color,
            value: if theme == Theme::Light { "#ffffff" } else { "#000000" }.into(),
        },
    };
    match wad_wallpaper::draw_png_toned(&wp, tone) {
        Some(png) => {
            crate::images::write_wallpaper(h.state_dir, id, "png", &png)?;
        }
        None => warnings.push(format!("its background {chosen} didn't draw")),
    }
    crate::projects::relabel(&dir, true);
    Ok(warnings)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// POST /v1/wadspaces/{id}/build: its set built, unless it's built on
/// this base already or building (that build is the answer).
pub async fn build_installed(h: &Here<'_>, id: &str) -> Result<SetBuild, ApiError> {
    let w = installed_doc(h.state_dir, id)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("{id:?} isn't installed here")))?;
    h.registry.workspace(id)?;
    let base = wad_core::spec::DEFAULT_BASE_IMAGE;
    let conflict = |e: String| ApiError::new(ErrorCode::Conflict, e);
    let info = base_for_sets(h.backend, base).await.map_err(conflict)?;
    let image_key = v4::image_key(&w);
    if image_key == "base" {
        return Ok(SetBuild { image_key, built: true, build: None });
    }
    if let Some(b) = h.builds.list().into_iter().find(|b| b.ws_id == image_key && !b.status.finished()) {
        return Ok(SetBuild { image_key, built: false, build: Some(b) });
    }
    let tag = v4::image_ref(&image_key, base);
    if h.backend.image_info(&tag).await.map_err(internal)?.is_some_and(|i| label(&i, FROM_LABEL) == info.id) {
        return Ok(SetBuild { image_key, built: true, build: None });
    }
    let name = w["customization"]["name"].as_str().unwrap_or(id).to_string();
    let b = h.builds.create_set(&name, v4::packages_of(&w), base)?;
    Ok(SetBuild { image_key, built: false, build: Some(b) })
}

/// Installed and built from one go (the relay's `build` and the remote
/// API's): the workspace, its image key, and the build, if one runs.
pub async fn install_and_build(
    h: &Here<'_>,
    id: &str,
    w: &Value,
    tz: Option<&str>,
) -> Result<(Installed, SetBuild), ApiError> {
    let installed = install(h, id, w, tz).await?;
    let built = build_installed(h, id).await?;
    Ok((installed, built))
}

/// PUT /v1/machine/theme: kept, and each version 4 wadspace's icons and
/// background made again in it (each takes them at its next start).
pub async fn set_theme(h: &Here<'_>, theme: Theme) -> Result<(), ApiError> {
    let name = if theme == Theme::Light { "light" } else { "dark" };
    let was = self::theme(h.state_dir);
    std::fs::create_dir_all(h.state_dir).map_err(internal)?;
    put(h.state_dir, "theme", format!("{name}\n").as_bytes()).map_err(internal)?;
    if was == theme {
        return Ok(());
    }
    for ws in h.registry.workspaces() {
        let Some(w) = installed_doc(h.state_dir, &ws.id) else { continue };
        if let Err(e) = write_extras(h, &ws.id, &w, theme).await {
            tracing::warn!("{}: its extras in the {name} theme: {e}", ws.id);
        }
    }
    tracing::info!("the theme is {name}");
    Ok(())
}
