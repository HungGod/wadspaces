//! wadd's HTTP API, `/v1`, on its Unix socket. JSON in and out; failures are
//! wad_proto's ApiError with a matching status. Every request is checked
//! against the caller's credentials first (access.rs).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use tokio::net::UnixListener;
use tokio_stream::wrappers::BroadcastStream;
use wad_proto::v1::{
    Browse, Build, BuildLog, CloudLink, Drive, Event, Health, ImageRow, KeysStatus, Launch, LaunchBody, LaunchLog,
    LayerBuildRequest, LogLine, MachineInfo, Project, ProjectDeleted, ProjectStatus, Run, Session, SessionRequest,
    SignIn, SignInPatch, SignInRequest, ViewState, WallpaperUpload, Workspace, WorkspaceSaved, WorkspaceState,
};
use wad_proto::{ApiError, ErrorCode};

use crate::access::{Peer, Policy};
use crate::events::Bus;
use crate::logbuf::LogBuffer;
use crate::registry::Registry;
use crate::view::View;

pub struct AppState {
    pub bus: Bus,
    pub registry: Arc<Registry>,
    pub view: Arc<View>,
    pub projects: Arc<crate::projects::Projects>,
    pub copies: Arc<crate::copies::Copies>,
    pub launches: Arc<crate::launches::Launches>,
    pub builds: Arc<crate::builds::Builds>,
    pub secrets: Arc<crate::secrets::Secrets>,
    pub signins: Arc<crate::signins::SignIns>,
    pub github: Arc<crate::github::GithubService>,
    pub network: Arc<crate::network::Network>,
    /// The machine the registry manages (podman), for diagnostics.
    pub registry_backend: Arc<dyn crate::backend::Backend>,
    pub meter: crate::metrics::Meter,
    /// Power off and restart (logind); None without the system bus.
    pub power: Option<wad_systemd::Power>,
    /// The screen's brightness (sysfs).
    pub backlight: crate::screen::Backlight,
    /// The account link; None without cloud settings.
    pub cloud: Option<Arc<crate::cloud::CloudRelay>>,
    /// The keyboard proxy, while it runs.
    pub keys: std::sync::Mutex<Option<wad_input::Proxy>>,
    pub keys_enabled: bool,
    pub logs: LogBuffer,
    pub policy: Policy,
    /// What's on disk (the Python wadd's formats).
    pub store: wad_store::State,
    /// What WadSpaces takes on the disk (GET /v1/storage).
    pub storage: crate::storage::Storage,
    /// Where podman keeps its images (asked once: it doesn't move).
    pub graph_root: std::sync::OnceLock<String>,
    /// What's saved on the disk outside the images (/v1/files).
    pub files: crate::files::Files,
    /// The devices plugged in, and the wadspaces they're handed to.
    pub devices: Arc<crate::devices::Devices>,
}

/// An ApiError as an HTTP response.
pub struct Failure(pub ApiError);

impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

fn no_workspace(id: &str) -> Failure {
    Failure(ApiError::new(ErrorCode::NotFound, format!("no workspace {id:?}")))
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::Offline => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Upstream => StatusCode::BAD_GATEWAY,
            ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::Cancelled => StatusCode::CONFLICT,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

impl Connected<axum::serve::IncomingStream<'_, UnixListener>> for Peer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, UnixListener>) -> Self {
        match stream.io().peer_cred() {
            Ok(c) => Peer { uid: c.uid(), gid: c.gid(), pid: c.pid() },
            Err(_) => Peer::UNKNOWN,
        }
    }
}

async fn check_peer(
    State(app): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    req: Request,
    next: Next,
) -> Response {
    if !app.policy.allows(&peer) {
        tracing::warn!(uid = peer.uid, pid = ?peer.pid, path = %req.uri().path(), "refused a caller");
        return Failure(ApiError::new(ErrorCode::Forbidden, "not allowed to use wadd")).into_response();
    }
    next.run(req).await
}

pub fn router(app: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/machine", get(machine))
        .route("/v1/logs", get(logs))
        .route("/v1/events", get(events))
        .route("/v1/workspaces", get(workspaces).post(workspace_create))
        .route("/v1/workspaces/{id}", get(workspace).put(workspace_put).delete(workspace_delete))
        .route("/v1/workspaces/{id}/state", get(workspace_state))
        .route("/v1/workspaces/{id}/icon", get(workspace_icon))
        .route("/v1/webapp-icon", get(webapp_icon))
        .route("/v1/desktop-icon", get(desktop_icon))
        .route(
            "/v1/workspaces/{id}/wallpaper",
            get(workspace_wallpaper_get)
                .put(workspace_wallpaper)
                .layer(axum::extract::DefaultBodyLimit::max(WALLPAPER_BODY)),
        )
        .route("/v1/wadspaces/{id}", axum::routing::put(wadspace_install))
        .route("/v1/images", get(images))
        .route("/v1/storage", get(storage))
        .route("/v1/storage/prune", post(storage_prune))
        .route("/v1/storage/delete", post(storage_delete))
        .route("/v1/files/roots", get(file_roots))
        .route("/v1/files", get(file_list).delete(file_delete))
        .route("/v1/files/text", get(file_text).put(file_text_put))
        .route("/v1/images/build", post(image_build))
        .route("/v1/images/{layer}", delete(image_delete))
        .route("/v1/workspaces/{id}/start", post(start))
        .route("/v1/workspaces/{id}/stop", post(stop))
        .route("/v1/workspaces/{id}/restart", post(restart))
        .route("/v1/workspaces/{id}/switch", post(switch))
        .route("/v1/states", get(states))
        .route("/v1/view", get(view))
        .route("/v1/view/home", post(home))
        .route("/v1/carousel/{step}", post(carousel))
        .route("/v1/keys", get(keys))
        .route("/v1/projects", get(projects).post(project_create))
        .route("/v1/projects/{id}", get(project).put(project_put).delete(project_delete))
        .route("/v1/projects/{id}/status", get(project_status))
        .route("/v1/signins", get(signins).post(signin_create))
        .route("/v1/signins/{id}", get(signin).put(signin_put).delete(signin_delete))
        .route("/v1/signins/{id}/signout", post(signin_sign_out))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/{key}", axum::routing::put(device_put).delete(device_forget))
        .route("/v1/devices/{key}/attach", post(device_attach))
        .route("/v1/devices/{key}/detach", post(device_detach))
        .route("/v1/drives", get(drives))
        .route("/v1/browse", get(browse))
        .route("/v1/copies", post(copy_create))
        .route("/v1/copies/{id}", delete(copy_delete))
        .route(
            "/v1/launches",
            get(launches).post(launch_create).layer(axum::extract::DefaultBodyLimit::max(WALLPAPER_BODY)),
        )
        .route("/v1/launches/batch", post(launch_batch).layer(axum::extract::DefaultBodyLimit::max(WALLPAPER_BODY)))
        .route("/v1/launches/{id}", get(launch).delete(launch_cancel))
        .route("/v1/builds", get(builds))
        .route("/v1/builds/{id}", get(build).delete(build_cancel))
        .route("/v1/runs", get(runs))
        .route("/v1/session", get(session).post(session_begin))
        .route("/v1/session", delete(session_end))
        .route("/v1/cloud", get(cloud).delete(cloud_unlink))
        .route("/v1/cloud/link", post(cloud_link))
        .route("/v1/github", get(github).delete(github_sign_out))
        .route("/v1/github/repos", get(github_repos).post(github_repo_create))
        .route("/v1/network", get(network))
        .route("/v1/network/wifi", get(wifi))
        .route("/v1/network/wifi/connect", post(wifi_connect))
        .route("/v1/network/wifi/disconnect", post(wifi_disconnect))
        .route("/v1/network/wifi/forget", post(wifi_forget))
        .route("/v1/power", post(power))
        .route("/v1/screen/brightness", get(brightness).put(brightness_put))
        .route("/v1/secrets", get(secrets))
        .route("/v1/secrets/{name}", axum::routing::put(secret_put).delete(secret_delete))
        .route("/v1/library/{collection}", get(library))
        .route("/v1/library/{collection}/{id}", get(library_get).put(library_put).delete(library_delete))
        .route("/v1/metrics", get(metrics))
        .route("/v1/diagnostics", get(diagnostics))
        .route("/v1/logs/unit/{unit}", get(unit_log))
        .route("/v1/logs/workspace/{id}", get(workspace_log))
        .fallback(|| async { Failure(ApiError::new(ErrorCode::NotFound, "no such endpoint")) })
        .layer(middleware::from_fn_with_state(app.clone(), check_peer))
        .with_state(app)
}

/// A wallpaper (16 MB) as base64, and the rest of its request.
const WALLPAPER_BODY: usize = 24 * 1024 * 1024;

async fn health() -> Json<Health> {
    Json(Health { ok: true, version: env!("CARGO_PKG_VERSION").into() })
}

async fn machine(State(app): State<Arc<AppState>>) -> Json<MachineInfo> {
    let mut info = app.bus.machine();
    info.screen = app.view.screen().await;
    Json(info)
}

#[derive(Deserialize)]
struct LogsQuery {
    lines: Option<usize>,
}

async fn logs(State(app): State<Arc<AppState>>, Query(q): Query<LogsQuery>) -> Json<Vec<LogLine>> {
    Json(app.logs.tail(q.lines.unwrap_or(200).clamp(1, 2000)))
}

async fn workspaces(State(app): State<Arc<AppState>>) -> Json<Vec<Workspace>> {
    Json(app.registry.workspaces())
}

async fn workspace(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Workspace>, Failure> {
    app.registry.workspaces().into_iter().find(|w| w.id == id).map(Json).ok_or_else(|| no_workspace(&id))
}

/// Where a workspace's icon may be: the image's icons, and wadd's own.
pub const IMAGE_ICONS: &str = "/usr/share/wadspaces/icons";
const ICON_MAX: u64 = 8 << 20;

#[derive(Deserialize)]
struct IconQuery {
    /// "dark" or "light": the icon's variant for that theme, when there is
    /// one beside it (`browser-dark.png` for `browser.png`).
    theme: Option<String>,
}

/// A workspace's icon, as a picture. Only from the icon folders (an icon is
/// a path anyone who may edit workspaces can set, so not any file).
async fn workspace_icon(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<IconQuery>,
) -> Result<Response, Failure> {
    let ws = app.registry.workspaces().into_iter().find(|w| w.id == id).ok_or_else(|| no_workspace(&id))?;
    let none = || Failure(ApiError::new(ErrorCode::NotFound, format!("workspace {id:?} has no icon")));
    let path = std::path::PathBuf::from(ws.icon.ok_or_else(none)?);
    let own = app.store.dir.join("icons");
    let themed = q.theme.as_deref().filter(|t| matches!(*t, "dark" | "light")).and_then(|t| themed_icon(&path, t));
    let (mime, bytes) = themed.and_then(|p| icon_file(&p, &own)).or_else(|| icon_file(&path, &own)).ok_or_else(none)?;
    Ok(([(axum::http::header::CONTENT_TYPE, mime), (axum::http::header::CACHE_CONTROL, "max-age=3600")], bytes)
        .into_response())
}

#[derive(Deserialize)]
struct SiteQuery {
    site: String,
}

/// A web app's icon as WadSpaces Client shows it: the site's own, in colour,
/// fetched once and kept (`{"png": base64}`); 404 when the site has none.
async fn webapp_icon(
    State(app): State<Arc<AppState>>,
    Query(q): Query<SiteQuery>,
) -> Result<Json<serde_json::Value>, Failure> {
    let png = app
        .builds
        .icons()
        .favicon(&q.site)
        .await
        .ok_or_else(|| Failure(ApiError::new(ErrorCode::NotFound, format!("{} has no icon to show", q.site))))?;
    Ok(Json(serde_json::json!({ "png": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png) })))
}

/// WadSpaces' own pictures (apps' icons, its brand, the folder), as the
/// image installs them; `WADD_ASSETS` names another folder (a laptop's).
pub const ASSETS: &str = "/usr/share/wadspaces/assets";

#[derive(Deserialize)]
struct DesktopIconQuery {
    /// A desktop icon's picture (Workspace.desktop): an asset path or a
    /// web app's favicon address.
    src: String,
    #[serde(default)]
    size: Option<u32>,
}

/// A desktop icon's picture as a square PNG (`size` px, 96 by default).
async fn desktop_icon(
    State(app): State<Arc<AppState>>,
    Query(q): Query<DesktopIconQuery>,
) -> Result<Response, Failure> {
    let none = || Failure(ApiError::new(ErrorCode::NotFound, format!("no picture for {:?}", q.src)));
    let px = q.size.unwrap_or(96).clamp(16, 256);
    let bytes = if let Some(domain) = q.src.strip_prefix(FAVICON) {
        let domain = percent_decoded(domain.split('&').next().unwrap_or_default());
        app.builds.icons().favicon(&format!("https://{domain}")).await.ok_or_else(none)?
    } else {
        let dir = std::env::var_os("WADD_ASSETS").map(std::path::PathBuf::from).unwrap_or_else(|| ASSETS.into());
        let path = asset_path(&dir, &q.src).ok_or_else(none)?;
        tokio::fs::read(path).await.map_err(|_| none())?
    };
    let png = tokio::task::spawn_blocking(move || {
        wad_icons::decode::decode(&bytes).map(|img| wad_icons::png(&wad_icons::style::fit(&img, px)))
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(none)?;
    Ok(([(axum::http::header::CONTENT_TYPE, "image/png"), (axum::http::header::CACHE_CONTROL, "max-age=86400")], png)
        .into_response())
}

/// How wad-core names a web app's icon (layers::favicon): its domain, encoded.
const FAVICON: &str = "https://www.google.com/s2/favicons?domain=";

fn percent_decoded(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(c) => {
                out.push(c);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An asset's file in `dir`, if `src` names one ("/apps/vscode/icon.png";
/// nothing outside it, links included).
fn asset_path(dir: &std::path::Path, src: &str) -> Option<std::path::PathBuf> {
    let rel = src.strip_prefix('/')?;
    if rel.is_empty() || !rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..") {
        return None;
    }
    let real = std::fs::canonicalize(dir.join(rel)).ok()?;
    (real.starts_with(std::fs::canonicalize(dir).ok()?) && real.is_file()).then_some(real)
}

#[cfg(test)]
mod asset_tests {
    use super::asset_path;

    #[test]
    fn assets_come_from_their_folder_only() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("assets");
        std::fs::create_dir_all(dir.join("apps/vscode")).unwrap();
        std::fs::write(dir.join("apps/vscode/icon.png"), b"png").unwrap();
        std::fs::write(d.path().join("secret"), b"no").unwrap();
        std::os::unix::fs::symlink(d.path().join("secret"), dir.join("link")).unwrap();
        assert!(asset_path(&dir, "/apps/vscode/icon.png").is_some());
        assert_eq!(super::percent_decoded("xn--bcher-kva.example%2Ecom%"), "xn--bcher-kva.example.com%");
        for bad in [
            "apps/vscode/icon.png",
            "/",
            "/apps",
            "/../secret",
            "/apps/../../secret",
            "/link",
            "//etc/passwd",
            "/nope.png",
        ] {
            assert!(asset_path(&dir, bad).is_none(), "{bad}");
        }
    }
}

/// `dir/name-<theme>.ext` for `dir/name.ext`.
pub fn themed_icon(path: &std::path::Path, theme: &str) -> Option<std::path::PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    Some(path.with_file_name(format!("{stem}-{theme}.{ext}")))
}

/// The icon at `path` (its type and bytes), if it's a picture in an icon folder.
pub fn icon_file(path: &std::path::Path, own_icons: &std::path::Path) -> Option<(&'static str, Vec<u8>)> {
    let real = std::fs::canonicalize(path).ok()?;
    let inside = |dir: &std::path::Path| std::fs::canonicalize(dir).is_ok_and(|d| real.starts_with(d));
    if !(inside(std::path::Path::new(IMAGE_ICONS)) || inside(own_icons)) {
        return None;
    }
    let ext = real.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let meta = std::fs::metadata(&real).ok()?;
    (meta.is_file() && meta.len() <= ICON_MAX).then_some(())?;
    Some((mime, std::fs::read(&real).ok()?))
}

/// Adds a workspace (refused if one has that id).
async fn workspace_create(
    State(app): State<Arc<AppState>>,
    Json(ws): Json<Workspace>,
) -> Result<(StatusCode, Json<Workspace>), Failure> {
    if app.registry.workspaces().iter().any(|w| w.id == ws.id) {
        return Err(Failure(ApiError::new(ErrorCode::Conflict, format!("workspace {:?} already exists", ws.id))));
    }
    let id = ws.id.clone();
    app.registry.put_workspace(ws).await?;
    Ok((
        StatusCode::CREATED,
        Json(app.registry.workspaces().into_iter().find(|w| w.id == id).ok_or_else(|| no_workspace(&id))?),
    ))
}

/// Replaces a workspace's settings (its projects stay: a launch sets those).
async fn workspace_put(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut ws): Json<Workspace>,
) -> Result<Json<wad_proto::v1::WorkspaceSaved>, Failure> {
    if !app.registry.workspaces().iter().any(|w| w.id == id) {
        return Err(no_workspace(&id));
    }
    ws.id = id.clone();
    let restart_required = app.registry.put_workspace(ws).await?;
    let workspace = app.registry.workspaces().into_iter().find(|w| w.id == id).ok_or_else(|| no_workspace(&id))?;
    Ok(Json(wad_proto::v1::WorkspaceSaved { workspace, restart_required }))
}

/// Removes a workspace, stopping it first if it runs.
async fn workspace_delete(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    // The machine's own apps are the system's: they stay.
    if let Some(w) = app.registry.workspaces().into_iter().find(|w| w.id == id && w.kind == wad_proto::v1::Kind::App) {
        return Err(Failure(ApiError::new(
            ErrorCode::Conflict,
            format!("{} is one of this machine's own apps: it stays", w.name),
        )));
    }
    if app.registry.state(&id).is_some_and(|s| s.container == "running") {
        app.view.stop(&id).await?;
    }
    app.registry.remove_workspace(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn states(State(app): State<Arc<AppState>>) -> Json<Vec<WorkspaceState>> {
    Json(app.registry.states())
}

fn state_of(app: &AppState, id: &str) -> Result<Json<WorkspaceState>, Failure> {
    app.registry.state(id).map(Json).ok_or_else(|| no_workspace(id))
}

async fn workspace_state(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceState>, Failure> {
    state_of(&app, &id)
}

/// Starts bringing a workspace up; its progress arrives as workspaceState
/// events.
async fn start(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<WorkspaceState>), Failure> {
    app.registry.start(&id)?;
    Ok((StatusCode::ACCEPTED, state_of(&app, &id)?))
}

/// Stops a workspace; if it was on screen, the next session pick (or Wad
/// Creator) is shown.
async fn stop(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<WorkspaceState>, Failure> {
    app.view.stop(&id).await?;
    state_of(&app, &id)
}

async fn restart(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<WorkspaceState>), Failure> {
    app.view.restart(&id).await?;
    Ok((StatusCode::ACCEPTED, state_of(&app, &id)?))
}

#[derive(Deserialize)]
struct DeletedQuery {
    #[serde(default)]
    deleted: bool,
}

/// ?deleted=true includes tombstones.
async fn projects(State(app): State<Arc<AppState>>, Query(q): Query<DeletedQuery>) -> Json<Vec<Project>> {
    Json(app.projects.list(q.deleted))
}

async fn project(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Project>, Failure> {
    Ok(Json(app.projects.get(&id)?))
}

async fn project_create(
    State(app): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<Project>), Failure> {
    let id = wad_store::projects::new_id();
    Ok((StatusCode::CREATED, Json(app.projects.save(&id, &body)?)))
}

async fn project_put(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Project>, Failure> {
    Ok(Json(app.projects.save(&id, &body)?))
}

#[derive(Deserialize)]
struct PurgeQuery {
    #[serde(default)]
    purge: bool,
}

/// Leaves a tombstone (it syncs); ?purge=true also removes the folder.
async fn project_delete(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<PurgeQuery>,
) -> Result<Json<ProjectDeleted>, Failure> {
    Ok(Json(app.projects.delete(&id, q.purge)?))
}

async fn project_status(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ProjectStatus>, Failure> {
    Ok(Json(app.projects.status(&id).await?))
}

async fn devices(State(app): State<Arc<AppState>>) -> Json<Vec<wad_proto::v1::Device>> {
    Json(app.devices.list())
}

/// Renames a device, or says where it goes.
async fn device_put(
    State(app): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(p): Json<wad_proto::v1::DevicePatch>,
) -> Result<Json<wad_proto::v1::Device>, Failure> {
    let name = p.name.map(|n| Some(n).filter(|n| !n.trim().is_empty()));
    Ok(Json(app.devices.set(&key, name, p.remembered)?))
}

/// Forgets a device that isn't plugged in.
async fn device_forget(State(app): State<Arc<AppState>>, Path(key): Path<String>) -> Result<StatusCode, Failure> {
    app.devices.forget(&key)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hands a device to a wadspace (now when it runs, and whenever it does).
async fn device_attach(
    State(app): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(to): Json<wad_proto::v1::DeviceTo>,
) -> Result<Json<wad_proto::v1::Device>, Failure> {
    Ok(Json(app.devices.attach(&key, &to.wadspace)?))
}

/// Takes a device back from a wadspace, for good.
async fn device_detach(
    State(app): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(to): Json<wad_proto::v1::DeviceTo>,
) -> Result<Json<wad_proto::v1::Device>, Failure> {
    Ok(Json(app.devices.detach(&key, &to.wadspace)?))
}

async fn signins(State(app): State<Arc<AppState>>) -> Json<Vec<SignIn>> {
    Json(app.signins.list())
}

async fn signin(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<SignIn>, Failure> {
    Ok(Json(app.signins.get(&id)?))
}

async fn signin_create(
    State(app): State<Arc<AppState>>,
    Json(req): Json<SignInRequest>,
) -> Result<(StatusCode, Json<SignIn>), Failure> {
    Ok((StatusCode::CREATED, Json(app.signins.create(req)?)))
}

/// A new name, or (an app the catalog doesn't know) new files.
async fn signin_put(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(patch): Json<SignInPatch>,
) -> Result<Json<SignIn>, Failure> {
    Ok(Json(app.signins.patch(&id, patch)?))
}

/// Forgets what it saved: each wadspace using it is signed out.
async fn signin_sign_out(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<SignIn>, Failure> {
    Ok(Json(app.signins.sign_out(&id)?))
}

async fn signin_delete(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    app.signins.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

fn drive_failure(e: crate::drives::DriveError) -> Failure {
    use crate::drives::DriveError as D;
    let code = match &e {
        D::Missing(_) => ErrorCode::Conflict,
        D::BadId(_) => ErrorCode::BadRequest,
        D::Failed(_) => ErrorCode::Offline,
    };
    Failure(ApiError::new(code, e.to_string()))
}

/// Filesystems a drive project could be on (not the system's own disk).
async fn drives(State(app): State<Arc<AppState>>) -> Result<Json<Vec<Drive>>, Failure> {
    app.projects.drives.list().await.map(Json).map_err(drive_failure)
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
    drive: Option<String>,
    /// Its files too (not inside a drive).
    #[serde(default)]
    files: bool,
}

/// The folders in `path`, inside the folder roots (no path: those roots),
/// and with `files`, its files. With `drive`, `path` is inside that drive
/// (mounted first if need be).
async fn browse(State(app): State<Arc<AppState>>, Query(q): Query<BrowseQuery>) -> Result<Json<Browse>, Failure> {
    use wad_store::folders::{self, FolderError, MAX_DIRS};
    let with_files = q.files;
    let r = match q.drive.filter(|d| !d.is_empty()) {
        Some(drive) => {
            let mp = app.projects.drives.mount(&drive, "", "").await.map_err(drive_failure)?;
            let path = q.path.unwrap_or_default();
            tokio::task::spawn_blocking(move || folders::browse_inside(&mp, &path, MAX_DIRS)).await
        }
        None => {
            let roots = app.projects.store.folder_roots().to_vec();
            tokio::task::spawn_blocking(move || {
                let mut b = folders::browse(q.path.as_deref(), &roots, MAX_DIRS)?;
                if let (true, Some(p)) = (with_files, &b.path) {
                    let (files, more) = folders::files(std::path::Path::new(p), MAX_DIRS);
                    b.files = files;
                    b.truncated |= more;
                }
                Ok(b)
            })
            .await
        }
    };
    match r.map_err(|e| Failure(ApiError::new(ErrorCode::Internal, e.to_string())))? {
        Ok(b) => Ok(Json(b)),
        Err(e @ FolderError::Outside(_)) => Err(Failure(ApiError::new(ErrorCode::Forbidden, e.to_string()))),
        Err(e @ FolderError::Missing(_)) => Err(Failure(ApiError::new(ErrorCode::NotFound, e.to_string()))),
    }
}

/// A copy of files on this machine, for wadspaces to take in (copies.rs).
async fn copy_create(
    State(app): State<Arc<AppState>>,
    Json(req): Json<wad_proto::v1::CopyRequest>,
) -> Result<(StatusCode, Json<wad_proto::v1::CopyMade>), Failure> {
    Ok((StatusCode::CREATED, Json(app.copies.make(&req.paths).await?)))
}

/// Removes a copy no wadspace here has.
async fn copy_delete(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    app.copies.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn launches(State(app): State<Arc<AppState>>) -> Json<Vec<Launch>> {
    Json(app.launches.list())
}

/// Opens a workspace, a throwaway run of one, a WadBrowser setup or the
/// terminal; its progress arrives as launch events.
async fn launch_create(
    State(app): State<Arc<AppState>>,
    Json(req): Json<LaunchBody>,
) -> Result<(StatusCode, Json<Launch>), Failure> {
    Ok((StatusCode::CREATED, Json(app.launches.create_item(req.item()).await?)))
}

/// POST /v1/launches/batch: the launcher's pick, launched together.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Batch {
    items: Vec<wad_proto::v1::LaunchItem>,
    #[serde(default)]
    focus_minutes: Option<u32>,
}

/// Launches the launcher's pick (optionally as a focus session): the
/// workspaces' ids; the launches arrive as launch events.
async fn launch_batch(
    State(app): State<Arc<AppState>>,
    Json(b): Json<Batch>,
) -> Result<(StatusCode, Json<Vec<String>>), Failure> {
    Ok((StatusCode::ACCEPTED, Json(app.launches.batch(b.items, b.focus_minutes).await?)))
}

/// The layer images here.
async fn images(State(app): State<Arc<AppState>>) -> Result<Json<Vec<ImageRow>>, Failure> {
    Ok(Json(crate::images::list(app.registry_backend.as_ref()).await?))
}

#[derive(Deserialize)]
struct StorageQuery {
    #[serde(default)]
    fresh: bool,
}

/// What WadSpaces takes on this machine's disk: the last count at once
/// (`?fresh=true` counts again now and waits).
async fn storage(State(app): State<Arc<AppState>>, Query(q): Query<StorageQuery>) -> Json<wad_proto::v1::Storage> {
    Json(app.storage.report(q.fresh).await)
}

/// Builds a layer's image (what it builds on first, when needed).
async fn image_build(
    State(app): State<Arc<AppState>>,
    Json(req): Json<LayerBuildRequest>,
) -> Result<(StatusCode, Json<Build>), Failure> {
    Ok((StatusCode::CREATED, Json(app.builds.create_layer(&req)?)))
}

async fn image_delete(State(app): State<Arc<AppState>>, Path(layer): Path<String>) -> Result<StatusCode, Failure> {
    crate::images::delete(&app.registry, &app.builds, app.registry_backend.as_ref(), &layer).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FileQuery {
    root: String,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct FileText {
    root: String,
    path: String,
    text: String,
}

/// The places Files looks through: your files, each wadspace's home, copies.
async fn file_roots(State(app): State<Arc<AppState>>) -> Json<Vec<crate::files::FileRoot>> {
    Json(app.files.roots(&app.registry).await)
}

async fn file_list(
    State(app): State<Arc<AppState>>,
    Query(q): Query<FileQuery>,
) -> Result<Json<crate::files::FileList>, Failure> {
    Ok(Json(app.files.list(&q.root, &q.path, &app.registry, app.registry_backend.as_ref()).await?))
}

async fn file_text(
    State(app): State<Arc<AppState>>,
    Query(q): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, Failure> {
    let text = app.files.read_text(&q.root, &q.path, &app.registry, app.registry_backend.as_ref()).await?;
    Ok(Json(serde_json::json!({ "text": text })))
}

async fn file_text_put(State(app): State<Arc<AppState>>, Json(t): Json<FileText>) -> Result<StatusCode, Failure> {
    app.files.write_text(&t.root, &t.path, &t.text, &app.registry, app.registry_backend.as_ref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn file_delete(State(app): State<Arc<AppState>>, Query(q): Query<FileQuery>) -> Result<StatusCode, Failure> {
    app.files.delete(&q.root, &q.path, &app.registry, app.registry_backend.as_ref()).await?;
    app.storage.forget();
    Ok(StatusCode::NO_CONTENT)
}

/// What's picked (layers, wadspaces), off this machine's disk.
async fn storage_delete(
    State(app): State<Arc<AppState>>,
    Json(pick): Json<wad_proto::v1::StorageDelete>,
) -> Result<Json<wad_proto::v1::Freed>, Failure> {
    let backend = app.registry_backend.as_ref();
    let freed =
        crate::images::delete_selected(&app.registry, &app.builds, &app.copies, backend, &app.store.dir, &pick).await?;
    app.storage.forget();
    Ok(Json(freed))
}

/// Older builds of layers that nothing needs, removed.
async fn storage_prune(State(app): State<Arc<AppState>>) -> Result<Json<wad_proto::v1::Freed>, Failure> {
    let freed = crate::images::prune(app.registry_backend.as_ref()).await?;
    app.storage.forget();
    Ok(Json(freed))
}

/// Installs a wadspace as a workspace on its layer's image (or updates it).
async fn wadspace_install(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<crate::images::Install>,
) -> Result<Json<WorkspaceSaved>, Failure> {
    Ok(Json(crate::images::install(&app.registry, app.registry_backend.as_ref(), &id, &req).await?))
}

/// The wallpaper a wadspace starts with, as a picture (the switcher shows it).
async fn workspace_wallpaper_get(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let ws = app.registry.workspaces().into_iter().find(|w| w.id == id).ok_or_else(|| no_workspace(&id))?;
    let dir = crate::images::extra_dir(&app.store.dir, &ws.id);
    for (ext, mime) in [("png", "image/png"), ("jpg", "image/jpeg")] {
        if let Ok(bytes) = std::fs::read(dir.join(format!("wallpaper.{ext}"))) {
            return Ok((
                [(axum::http::header::CONTENT_TYPE, mime), (axum::http::header::CACHE_CONTROL, "max-age=60")],
                bytes,
            )
                .into_response());
        }
    }
    Err(Failure(ApiError::new(ErrorCode::NotFound, format!("{} has no wallpaper", ws.name))))
}

/// The wallpaper a wadspace starts with.
async fn workspace_wallpaper(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(up): Json<WallpaperUpload>,
) -> Result<StatusCode, Failure> {
    let ws = app.registry.workspace(&id)?;
    crate::images::put_wallpaper(&app.store.dir, &ws, &up)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SinceQuery {
    #[serde(default)]
    since: u64,
}

/// A launch, and its log from line `since`.
async fn launch(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<SinceQuery>,
) -> Result<Json<LaunchLog>, Failure> {
    Ok(Json(app.launches.log(&id, q.since)?))
}

async fn builds(State(app): State<Arc<AppState>>) -> Json<Vec<Build>> {
    Json(app.builds.list())
}

/// A build, and its log from line `since`.
async fn build(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<SinceQuery>,
) -> Result<Json<BuildLog>, Failure> {
    Ok(Json(app.builds.log(&id, q.since)?))
}

async fn build_cancel(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Build>, Failure> {
    Ok(Json(app.builds.cancel(&id)?))
}

async fn launch_cancel(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Launch>, Failure> {
    Ok(Json(app.launches.cancel(&id)?))
}

#[derive(Deserialize)]
struct RunsQuery {
    workspace: Option<String>,
    limit: Option<usize>,
}

async fn runs(State(app): State<Arc<AppState>>, Query(q): Query<RunsQuery>) -> Json<Vec<Run>> {
    // Off the async threads: the first read of a long history takes a moment.
    let store = app.store.clone();
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    Json(tokio::task::spawn_blocking(move || store.runs(q.workspace.as_deref(), limit)).await.unwrap_or_default())
}

async fn view(State(app): State<Arc<AppState>>) -> Json<ViewState> {
    Json(app.view.state())
}

/// Shows a workspace: now if it's ready, else once it is.
async fn switch(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<ViewState>, Failure> {
    app.view.switch(&id)?;
    Ok(Json(app.view.state()))
}

/// WadSpaces Client (refused during a focus session's time).
async fn home(State(app): State<Arc<AppState>>) -> Result<Json<ViewState>, Failure> {
    app.view.home(false)?;
    Ok(Json(app.view.state()))
}

/// The Super+Tab switcher, by hand: next, prev, commit or cancel.
async fn carousel(State(app): State<Arc<AppState>>, Path(step): Path<String>) -> Result<StatusCode, Failure> {
    match step.as_str() {
        "next" => app.view.carousel_step(true),
        "prev" => app.view.carousel_step(false),
        "commit" => app.view.carousel_commit(),
        "cancel" => app.view.carousel_cancel(),
        _ => return Err(Failure(ApiError::new(ErrorCode::NotFound, "next, prev, commit or cancel"))),
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn keys(State(app): State<Arc<AppState>>) -> Json<KeysStatus> {
    let status = app.keys.lock().unwrap().as_ref().map(|k| k.status()).unwrap_or_default();
    Json(KeysStatus {
        enabled: app.keys_enabled,
        grabbing: status.grabbing,
        keyboards: status.keyboards,
        note: status.note,
    })
}

async fn session(State(app): State<Arc<AppState>>) -> Json<Option<Session>> {
    Json(app.view.session())
}

async fn session_begin(
    State(app): State<Arc<AppState>>,
    Json(req): Json<SessionRequest>,
) -> Result<Json<Session>, Failure> {
    Ok(Json(app.view.session_start(&req.workspaces, req.minutes, !req.launched)?))
}

#[derive(Deserialize)]
struct EndQuery {
    #[serde(default)]
    force: bool,
}

/// Ends the session; during a focus session's time only with ?force=true
/// (the user chose to end it early).
async fn session_end(State(app): State<Arc<AppState>>, Query(q): Query<EndQuery>) -> Result<StatusCode, Failure> {
    app.view.session_end(q.force)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cloud(State(app): State<Arc<AppState>>) -> Json<CloudLink> {
    Json(app.cloud.as_ref().map(|c| c.link_state()).unwrap_or_else(|| app.store.cloud()))
}

fn relay(app: &AppState) -> Result<&Arc<crate::cloud::CloudRelay>, Failure> {
    app.cloud
        .as_ref()
        .ok_or_else(|| Failure(ApiError::new(ErrorCode::Conflict, "this machine has no account settings to link with")))
}

#[derive(Deserialize)]
struct LinkBody {
    code: String,
}

/// Links this machine to the account that made the code.
async fn cloud_link(State(app): State<Arc<AppState>>, Json(b): Json<LinkBody>) -> Result<Json<CloudLink>, Failure> {
    Ok(Json(relay(&app)?.link(&b.code).await?))
}

async fn cloud_unlink(State(app): State<Arc<AppState>>) -> Result<StatusCode, Failure> {
    relay(&app)?.unlink().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn github(State(app): State<Arc<AppState>>) -> Json<wad_proto::github::GithubStatus> {
    Json(app.github.status().await)
}

/// Forgets the GitHub token here (the account keeps its copy).
async fn github_sign_out(State(app): State<Arc<AppState>>) -> Result<StatusCode, Failure> {
    app.github.sign_out().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FreshQuery {
    #[serde(default)]
    fresh: bool,
}

/// The repos the owner can use (from the last minute's unless ?fresh=true).
async fn github_repos(
    State(app): State<Arc<AppState>>,
    Query(q): Query<FreshQuery>,
) -> Result<Json<wad_proto::github::Repos>, Failure> {
    match app.github.repos(q.fresh).await? {
        Some(r) => Ok(Json(r)),
        None => Err(Failure(ApiError::new(
            ErrorCode::Conflict,
            "no GitHub token on this machine (sign in to GitHub first)",
        ))),
    }
}

/// A new repo on GitHub, and a project for it.
async fn github_repo_create(
    State(app): State<Arc<AppState>>,
    Json(req): Json<wad_proto::github::NewRepo>,
) -> Result<(StatusCode, Json<Project>), Failure> {
    Ok((StatusCode::CREATED, Json(app.github.create_repo(&req).await?)))
}

async fn network(State(app): State<Arc<AppState>>) -> Json<wad_proto::v1::NetworkStatus> {
    Json(app.network.status().await)
}

#[derive(Deserialize)]
struct RescanQuery {
    #[serde(default)]
    rescan: bool,
}

/// The networks in range (?rescan=true looks again first).
async fn wifi(
    State(app): State<Arc<AppState>>,
    Query(q): Query<RescanQuery>,
) -> Result<Json<Vec<wad_proto::v1::WifiNetwork>>, Failure> {
    Ok(Json(app.network.wifi(q.rescan).await?))
}

/// Joins a network; the password goes to NetworkManager over D-Bus only.
async fn wifi_connect(
    State(app): State<Arc<AppState>>,
    Json(b): Json<wad_proto::v1::WifiJoin>,
) -> Result<Json<wad_proto::v1::NetworkStatus>, Failure> {
    Ok(Json(app.network.connect(&b.ssid, b.password.as_deref()).await?))
}

async fn wifi_disconnect(State(app): State<Arc<AppState>>) -> Result<Json<wad_proto::v1::NetworkStatus>, Failure> {
    Ok(Json(app.network.disconnect().await?))
}

#[derive(Deserialize)]
struct ForgetBody {
    ssid: String,
}

async fn wifi_forget(State(app): State<Arc<AppState>>, Json(b): Json<ForgetBody>) -> Result<StatusCode, Failure> {
    if app.network.forget(&b.ssid).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Failure(ApiError::new(ErrorCode::NotFound, format!("no saved network {:?}", b.ssid))))
    }
}

/// Powers the machine off or restarts it.
async fn power(
    State(app): State<Arc<AppState>>,
    Json(b): Json<wad_proto::v1::PowerRequest>,
) -> Result<StatusCode, Failure> {
    let p = app.power.as_ref().ok_or_else(|| Failure(ApiError::new(ErrorCode::Offline, "logind isn't reachable")))?;
    tracing::info!("{:?} asked for", b.action);
    let r = match b.action {
        wad_proto::v1::PowerAction::Poweroff => p.power_off().await,
        wad_proto::v1::PowerAction::Reboot => p.reboot().await,
    };
    r.map_err(|e| Failure(ApiError::new(ErrorCode::Internal, e.to_string())))?;
    Ok(StatusCode::ACCEPTED)
}

async fn brightness(State(app): State<Arc<AppState>>) -> Json<wad_proto::v1::Brightness> {
    Json(app.backlight.get())
}

async fn brightness_put(
    State(app): State<Arc<AppState>>,
    Json(b): Json<wad_proto::v1::BrightnessRequest>,
) -> Result<Json<wad_proto::v1::Brightness>, Failure> {
    let code = if app.backlight.get().available { ErrorCode::Internal } else { ErrorCode::NotFound };
    app.backlight.set(b.percent).map(Json).map_err(|e| Failure(ApiError::new(code, e)))
}

/// The secrets' names and where they came from; never their values.
async fn secrets(State(app): State<Arc<AppState>>) -> Result<Json<Vec<wad_proto::v1::SecretInfo>>, Failure> {
    Ok(Json(app.secrets.list().await?))
}

#[derive(Deserialize)]
struct SecretBody {
    value: String,
}

async fn secret_put(
    State(app): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(b): Json<SecretBody>,
) -> Result<StatusCode, Failure> {
    app.secrets.set(&name, b.value.as_bytes()).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn secret_delete(State(app): State<Arc<AppState>>, Path(name): Path<String>) -> Result<StatusCode, Failure> {
    if app.secrets.delete(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Failure(ApiError::new(ErrorCode::NotFound, format!("no secret {name:?}"))))
    }
}

fn lib_failure(e: String) -> Failure {
    Failure(ApiError::new(if e.starts_with("no collection") { ErrorCode::NotFound } else { ErrorCode::BadRequest }, e))
}

async fn library_get(
    State(app): State<Arc<AppState>>,
    Path((collection, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, Failure> {
    match app.store.library_get(&collection, &id).map_err(lib_failure)? {
        Some(d) => Ok(Json(d)),
        None => Err(Failure(ApiError::new(ErrorCode::NotFound, format!("no {id:?} in {collection}")))),
    }
}

/// Saves one of WadSpaces Client's documents (opaque to wadd).
async fn library_put(
    State(app): State<Arc<AppState>>,
    Path((collection, id)): Path<(String, String)>,
    Json(doc): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, Failure> {
    app.store.library_put(&collection, &id, &doc).map_err(lib_failure)?;
    Ok(Json(doc))
}

async fn library_delete(
    State(app): State<Arc<AppState>>,
    Path((collection, id)): Path<(String, String)>,
) -> Result<StatusCode, Failure> {
    if app.store.library_delete(&collection, &id).map_err(lib_failure)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Failure(ApiError::new(ErrorCode::NotFound, format!("no {id:?} in {collection}"))))
    }
}

async fn metrics(State(app): State<Arc<AppState>>) -> Json<wad_proto::v1::Metrics> {
    let root = match app.graph_root.get() {
        Some(r) => Some(r.clone()),
        None => {
            let r = app
                .registry_backend
                .podman_info()
                .await
                .ok()
                .and_then(|i| i.pointer("/store/graphRoot").and_then(|v| v.as_str()).map(String::from));
            r.inspect(|r| _ = app.graph_root.set(r.clone()))
        }
    };
    Json(app.meter.snapshot(std::path::Path::new(root.as_deref().unwrap_or("/"))).await)
}

async fn diagnostics(State(app): State<Arc<AppState>>) -> Json<wad_proto::v1::Diagnostics> {
    Json(crate::diagnostics::diagnostics(&app).await)
}

#[derive(Deserialize)]
struct LinesQuery {
    lines: Option<usize>,
}

/// A unit's journal (wadd, greetd, a workspace's), redacted.
async fn unit_log(
    State(app): State<Arc<AppState>>,
    Path(unit): Path<String>,
    Query(q): Query<LinesQuery>,
) -> Result<Json<wad_proto::v1::LogText>, Failure> {
    Ok(Json(crate::diagnostics::unit_log(&app, &unit, q.lines.unwrap_or(200).clamp(1, 2000)).await?))
}

/// A workspace container's output, redacted.
async fn workspace_log(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<LinesQuery>,
) -> Result<Json<wad_proto::v1::LogText>, Failure> {
    Ok(Json(crate::diagnostics::workspace_log(&app, &id, q.lines.unwrap_or(200).clamp(1, 2000)).await?))
}

async fn library(
    State(app): State<Arc<AppState>>,
    Path(collection): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, Failure> {
    app.store
        .library(&collection)
        .map(Json)
        .ok_or_else(|| Failure(ApiError::new(ErrorCode::NotFound, format!("no collection {collection:?}"))))
}

fn sse(e: &Event) -> SseEvent {
    let data = match serde_json::to_value(e) {
        Ok(serde_json::Value::Object(mut o)) => o.remove("data").unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    };
    SseEvent::default().event(e.name()).data(data.to_string())
}

async fn events(State(app): State<Arc<AppState>>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (current, rx) = app.bus.subscribe();
    let first = stream::iter(current.into_iter().map(|e| Ok(sse(&e))));
    // A watcher that falls behind skips ahead (it gets the next events).
    let next = BroadcastStream::new(rx).filter_map(|e| async move { e.ok().map(|e| Ok(sse(&e))) });
    Sse::new(first.chain(next)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
