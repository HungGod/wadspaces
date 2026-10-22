//! wadd's remote API: what the owner's trusted devices may ask of the
//! machine over the tailnet (host/TAILNET.md). It's its own small router on
//! the machine's tailnet addresses (bound to tailscale0), never the full
//! /v1 the machine's socket serves.
//!
//! Every request is checked first ([`gate`]): tailscaled says which device
//! the connection is from, and only the owner's own untagged devices get
//! anywhere. An untrusted one can only say hello, which asks the machine
//! whether to trust it. Past hello, a request also carries the owner's
//! sign-in (`Authorization: Bearer <Firebase ID token>`), so a trusted
//! phone's other apps can't use the machine.
//!
//! What it does: lists the wadspaces here; builds one of the owner's from
//! the account (wadd reads it, the device sends only its id), opens or
//! stops one on the machine's screen, and streams one (the stream's sign-in
//! link goes back to the device that asked, never anywhere else).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use wad_proto::v1::{
    BuildLog, Event, Kind, Launch, RemoteBuild, RemoteHello, RemoteWadspace, StreamRequest, StreamShare, View,
    Workspace,
};
use wad_proto::{ApiError, ErrorCode};

use crate::api::{AppState, Failure};
use crate::tailnet::Caller;

/// A request body's most.
const BODY: usize = 64 * 1024;
/// The one path an untrusted device may use (the app's).
const HELLO: &str = "/v1/remote/hello";

/// Hello: the app's path, or the bare address (what a person types into a
/// phone's browser, which may capitalise a path).
fn is_hello(path: &str) -> bool {
    path == HELLO || path == "/"
}

/// What acts on the machine: the owner's sign-in too. Anything else that
/// isn't hello is no such endpoint (404), whatever the sign-in.
fn needs_account(path: &str) -> bool {
    path.starts_with("/v1/remote/") && path != HELLO
}

/// A connection to the remote API: where it's from, and where it arrived.
#[derive(Debug, Clone, Copy)]
pub struct Conn {
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

impl Connected<axum::serve::IncomingStream<'_, TcpListener>> for Conn {
    fn connect_info(s: axum::serve::IncomingStream<'_, TcpListener>) -> Self {
        let peer = *s.remote_addr();
        Conn { peer, local: s.io().local_addr().unwrap_or(peer) }
    }
}

/// Who's asking, before anything else: the owner's device, trusted (or only
/// saying hello), as `Extension<Caller>` for the handlers.
pub async fn gate(
    State(app): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<Conn>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let caller = match app.tailnet.identify(conn.peer).await {
        Ok(c) => c,
        Err(r) => {
            tracing::warn!(peer = %conn.peer, %path, "remote: refused ({r:?})");
            return Failure(ApiError::new(ErrorCode::Forbidden, r.message())).into_response();
        }
    };
    if !caller.trusted && !is_hello(&path) {
        let machine = app.bus.machine().name;
        return Failure(ApiError::new(
            ErrorCode::Forbidden,
            format!("Allow {} on {machine} first (WadSpaces asks there).", caller.name),
        ))
        .into_response();
    }
    let token = bearer(&req);
    if needs_account(&path)
        && let Err(f) = owners_sign_in(&app, token).await
    {
        tracing::warn!(device = %caller.name, %path, "remote: {}", f.0.message);
        return f.into_response();
    }
    tracing::debug!(device = %caller.name, %path, "remote");
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// The sign-in a request carries (`Authorization: Bearer <ID token>`).
fn bearer(req: &Request) -> Option<String> {
    let h = req.headers().get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    h.strip_prefix("Bearer ").filter(|t| !t.is_empty()).map(String::from)
}

/// The request carries the machine owner's sign-in.
async fn owners_sign_in(app: &AppState, token: Option<String>) -> Result<(), Failure> {
    let relay = app
        .cloud
        .as_ref()
        .filter(|c| c.linked())
        .ok_or_else(|| Failure(ApiError::new(ErrorCode::Offline, "This machine isn't linked to an account.")))?;
    let token = token.ok_or_else(|| Failure(ApiError::new(ErrorCode::Unauthorized, "Sign in to WadSpaces first.")))?;
    match relay.owner_token(&token).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(Failure(ApiError::new(
            ErrorCode::Unauthorized,
            "That sign-in isn't this machine's owner's (or it ran out): sign in again.",
        ))),
        Err(e) => Err(Failure(ApiError::new(ErrorCode::Upstream, format!("Couldn't check the sign-in: {e}")))),
    }
}

pub fn router(app: Arc<AppState>) -> Router {
    Router::new()
        .route(HELLO, get(hello))
        .route("/", get(hello))
        .route("/v1/remote/wadspaces", get(wadspaces))
        .route("/v1/remote/wadspaces/{id}/build", post(build))
        .route("/v1/remote/wadspaces/{id}/open", post(open))
        .route("/v1/remote/wadspaces/{id}/stop", post(stop))
        .route("/v1/remote/wadspaces/{id}/stream", post(stream).delete(stream_stop))
        .route("/v1/remote/builds/{id}", get(build_log))
        .fallback(|| async { Failure(ApiError::new(ErrorCode::NotFound, "no such endpoint")) })
        .layer(axum::extract::DefaultBodyLimit::max(BODY))
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app)
}

/// The machine, and whether it trusts the device asking (if not, it's
/// asking its owner now).
async fn hello(State(app): State<Arc<AppState>>, Extension(c): Extension<Caller>) -> Json<RemoteHello> {
    let link = app.store.cloud();
    Json(RemoteHello {
        machine: app.bus.machine().name,
        machine_id: link.machine_id.filter(|_| link.linked),
        version: env!("CARGO_PKG_VERSION").into(),
        device: c.name,
        trust: if c.trusted { "trusted" } else { "asked" }.into(),
    })
}

/// Something on the machine's screen to say what a device did.
fn notice(app: &AppState, text: String) {
    tracing::info!("remote: {text}");
    app.bus.publish(Event::Notice { text });
}

/// A wadspace here (not one of the machine's own apps).
fn wadspace(app: &AppState, id: &str) -> Result<Workspace, Failure> {
    match app.registry.workspace(id) {
        Ok(w) if w.kind == Kind::Container => Ok(w),
        _ => Err(Failure(ApiError::new(
            ErrorCode::NotFound,
            "That wadspace isn't on this machine: build it here first.",
        ))),
    }
}

/// The layer an installed wadspace runs: one of the Builder's, "base" (a
/// wadspace with no packages), or "" for none.
fn layer_of(image: &str) -> String {
    match crate::images::layer_of(image) {
        Some(l) => l,
        None if image == wad_core::spec::DEFAULT_BASE_IMAGE => "base".into(),
        None => String::new(),
    }
}

/// The build making `layer`'s image now.
fn building(app: &AppState, layer: &str) -> Option<String> {
    app.builds.list().into_iter().find(|b| b.ws_id == layer && !b.status.finished()).map(|b| b.id)
}

/// The wadspaces on the machine: built or building, running, on the
/// screen, streamed.
async fn wadspaces(State(app): State<Arc<AppState>>) -> Result<Json<Vec<RemoteWadspace>>, Failure> {
    let images = crate::images::list(app.registry_backend.as_ref()).await.unwrap_or_default();
    let streams = app.streams.list();
    let on_screen = match app.view.current() {
        View::Workspace(id) => Some(id),
        View::Home => None,
    };
    let list = app
        .registry
        .workspaces()
        .into_iter()
        .filter(|w| w.kind == Kind::Container && w.enabled && !w.ephemeral)
        .map(|w| {
            let layer = layer_of(&w.image);
            let st = app.registry.state(&w.id);
            let stream = streams.iter().find(|s| s.workspace == w.id);
            RemoteWadspace {
                built: images.iter().any(|r| r.layer == layer),
                building: building(&app, &layer),
                container: st.as_ref().map(|s| s.container.clone()).unwrap_or_else(|| "unknown".into()),
                phase: st.map(|s| s.phase),
                on_screen: on_screen.as_deref() == Some(w.id.as_str()),
                streamed: stream.is_some(),
                live: stream.is_some_and(|s| s.live),
                id: w.id,
                name: w.name,
                layer,
            }
        })
        .collect();
    Ok(Json(list))
}

/// The machine's time zone (what a wadspace's clock shows).
fn time_zone() -> Option<String> {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| p.to_str().and_then(|p| p.split("zoneinfo/").nth(1)).map(String::from))
}

/// One of the owner's wadspaces installed here as the account has it now,
/// its wallpaper made, and its image built unless that's up to date or
/// building already.
async fn build(
    State(app): State<Arc<AppState>>,
    Extension(c): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<RemoteBuild>, Failure> {
    let relay = app
        .cloud
        .as_ref()
        .ok_or_else(|| Failure(ApiError::new(ErrorCode::Offline, "This machine isn't linked to an account.")))?;
    let w = relay.wadspace_doc(&id).await?;
    let name = w["customization"]["name"].as_str().unwrap_or(&id).to_string();
    let (i, b) = crate::sets::install_and_build(&app.here(), &id, &w, time_zone().as_deref()).await?;
    if b.build.is_some() {
        notice(&app, format!("{} is building {name}", c.name));
    }
    Ok(Json(RemoteBuild {
        workspace: i.saved.workspace.id,
        layer: b.image_key,
        build_id: b.build.map(|b| b.id),
        built: b.built,
        restart_required: i.saved.restart_required,
        warnings: i.warnings,
    }))
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: u64,
}

/// A build's progress: its state, and its log's lines from `since` on.
async fn build_log(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<Since>,
) -> Result<Json<BuildLog>, Failure> {
    Ok(Json(app.builds.log(&id, q.since)?))
}

/// Opens a wadspace on the machine's screen.
async fn open(
    State(app): State<Arc<AppState>>,
    Extension(c): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Launch>), Failure> {
    let w = wadspace(&app, &id)?;
    let launch = app.launches.create(&id, false)?;
    notice(&app, format!("{} opened {}", c.name, w.name));
    Ok((StatusCode::CREATED, Json(launch)))
}

/// Stops a wadspace (its stream too).
async fn stop(
    State(app): State<Arc<AppState>>,
    Extension(c): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let w = wadspace(&app, &id)?;
    app.view.stop(&id).await?;
    notice(&app, format!("{} stopped {}", c.name, w.name));
    Ok(StatusCode::NO_CONTENT)
}

/// Streams a wadspace (built here): its sign-in link, for the device that
/// asked. Open on the screen, it's only taken off with `takeover`.
async fn stream(
    State(app): State<Arc<AppState>>,
    Extension(c): Extension<Caller>,
    Path(id): Path<String>,
    body: Option<Json<StreamRequest>>,
) -> Result<Json<StreamShare>, Failure> {
    let w = wadspace(&app, &id)?;
    let takeover = body.is_some_and(|Json(b)| b.takeover);
    let share = app.streams.start(&id, takeover).await?;
    notice(&app, format!("{} is streaming {}", c.name, w.name));
    Ok(Json(share))
}

async fn stream_stop(State(app): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    wadspace(&app, &id)?;
    app.streams.stop(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A listener on `addr`, bound to `device` when there is one.
fn bind(addr: SocketAddr, device: &str) -> std::io::Result<TcpListener> {
    let s = if addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
    s.set_reuseaddr(true)?;
    if !device.is_empty() {
        s.bind_device(Some(device.as_bytes()))?;
    }
    s.bind(addr)?;
    s.listen(128)
}

/// Serves the remote API on the tailnet's addresses until `stop`, listening
/// again whenever they change (joined, left, a new address); addresses that
/// can't be bound yet are tried again shortly.
pub async fn serve(app: Arc<AppState>, mut stop: watch::Receiver<bool>) {
    if !app.tailnet.enabled() {
        return;
    }
    let mut addrs = app.tailnet.addrs();
    loop {
        let want = addrs.borrow_and_update().clone();
        let mut serving = vec![];
        let mut missed = false;
        for a in &want {
            match bind(*a, app.tailnet.bind_device()) {
                Ok(l) => {
                    tracing::info!("remote API on {a}");
                    let svc = router(app.clone()).into_make_service_with_connect_info::<Conn>();
                    serving.push(tokio::spawn(async move {
                        if let Err(e) = axum::serve(l, svc).await {
                            tracing::warn!("remote API: {e}");
                        }
                    }));
                }
                Err(e) => {
                    tracing::warn!("remote API: {a}: {e}");
                    missed = true;
                }
            }
        }
        app.tailnet.set_listening(!serving.is_empty());
        let retry = async {
            if missed { tokio::time::sleep(Duration::from_secs(5)).await } else { std::future::pending().await }
        };
        let done = tokio::select! {
            _ = addrs.changed() => false,
            _ = retry => false,
            _ = stop.changed() => true,
        };
        for s in serving {
            s.abort();
        }
        app.tailnet.set_listening(false);
        if done {
            return;
        }
    }
}
