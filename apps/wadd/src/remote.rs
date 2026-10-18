//! wadd's remote API: what the owner's trusted devices may ask of the
//! machine over the tailnet (host/TAILNET.md). It's its own small router on
//! the machine's tailnet addresses (bound to tailscale0), never the full
//! /v1 the machine's socket serves.
//!
//! Every request is checked first ([`gate`]): tailscaled says which device
//! the connection is from, and only the owner's own untagged devices get
//! anywhere. An untrusted one can only say hello, which asks the machine
//! whether to trust it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::watch;
use wad_proto::v1::RemoteHello;
use wad_proto::{ApiError, ErrorCode};

use crate::api::{AppState, Failure};
use crate::tailnet::Caller;

/// A request body's most.
const BODY: usize = 64 * 1024;
/// The one path an untrusted device may use.
const HELLO: &str = "/v1/remote/hello";

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
    if !caller.trusted && path != HELLO {
        let machine = app.bus.machine().name;
        return Failure(ApiError::new(
            ErrorCode::Forbidden,
            format!("Allow {} on {machine} first (WadSpaces asks there).", caller.name),
        ))
        .into_response();
    }
    tracing::debug!(device = %caller.name, %path, "remote");
    req.extensions_mut().insert(caller);
    next.run(req).await
}

pub fn router(app: Arc<AppState>) -> Router {
    Router::new()
        .route(HELLO, get(hello))
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
