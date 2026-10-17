//! The streaming proxy: what the Cloudflare tunnel delivers to this machine
//! (http://127.0.0.1:8085, the tunnel's route for *.<domain>). It looks at
//! the Host only:
//!
//! - `ws-<guid>.<domain>`, a stream here: passed to its sidecar's web port
//!   as it came, the login included (the sidecar's nginx asks for it), and
//!   websockets passed through both ways;
//! - `ws-selftest.<domain>`: answered here, so the self-test (tunnel.rs)
//!   can tell the whole way works and that it reached this wadd;
//! - anything else: 404.
//!
//! It never logs anyone in itself, and listens on loopback only: the
//! tunnel is the way in from outside.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use crate::streams::{Activity, Streams};
use crate::tunnel::{SELFTEST_HEADER, SELFTEST_HOST};

type Body = BoxBody<Bytes, hyper::Error>;

const CONNECT: Duration = Duration::from_secs(5);

/// Binds each address (a loopback one only); one that can't be bound is
/// logged and skipped.
pub async fn bind(addresses: &[String]) -> Vec<TcpListener> {
    let mut out = vec![];
    for a in addresses {
        let addr: SocketAddr = match a.parse() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::warn!("streaming proxy: {a:?}: {e}");
                continue;
            }
        };
        if !addr.ip().is_loopback() {
            tracing::warn!("streaming proxy: {a} isn't a loopback address; not listening there");
            continue;
        }
        match TcpListener::bind(addr).await {
            Ok(l) => {
                tracing::info!("streaming proxy on {}", l.local_addr().map(|x| x.to_string()).unwrap_or_default());
                out.push(l);
            }
            Err(e) => tracing::warn!("streaming proxy: can't listen on {a}: {e}"),
        }
    }
    out
}

pub struct Proxy {
    streams: Arc<Streams>,
}

impl Proxy {
    pub fn new(streams: Arc<Streams>) -> Arc<Self> {
        Arc::new(Self { streams })
    }

    /// Serves these listeners until `stop` turns true.
    pub async fn serve(self: Arc<Self>, listeners: Vec<TcpListener>, stop: tokio::sync::watch::Receiver<bool>) {
        let tasks: Vec<_> = listeners
            .into_iter()
            .map(|l| {
                let (me, mut stop) = (self.clone(), stop.clone());
                tokio::spawn(async move {
                    loop {
                        let conn = tokio::select! {
                            c = l.accept() => c,
                            _ = stop.wait_for(|s| *s) => break,
                        };
                        let Ok((tcp, _)) = conn else { continue };
                        let me = me.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| me.clone().handle(req));
                            let conn = hyper::server::conn::http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(Duration::from_secs(30))
                                .serve_connection(TokioIo::new(tcp), service)
                                .with_upgrades();
                            let _ = conn.await;
                        });
                    }
                })
            })
            .collect();
        for t in tasks {
            let _ = t.await;
        }
    }

    async fn handle(self: Arc<Self>, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
        let domain = self.streams.domain().to_ascii_lowercase();
        let host = host_of(&req);
        let Some(name) = host.strip_suffix(&format!(".{domain}")) else { return Ok(not_found()) };
        if name == SELFTEST_HOST {
            return Ok(self.selftest(&req));
        }
        let Some(guid) = name.strip_prefix("ws-").filter(|g| is_guid(g)) else { return Ok(not_found()) };
        let Some((port, activity)) = self.streams.route(guid) else { return Ok(not_found()) };
        match self.forward(req, port, activity).await {
            Ok(r) => Ok(r),
            Err(e) => {
                tracing::debug!("stream {guid}: {e}");
                Ok(text(StatusCode::BAD_GATEWAY, "This stream isn't answering yet: try again in a moment.\n"))
            }
        }
    }

    fn selftest(&self, req: &Request<Incoming>) -> Response<Body> {
        // The nonce comes back only when it's what the self-test sends.
        let nonce = req
            .uri()
            .query()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("n=")))
            .filter(|n| n.len() == 32 && is_guid(n))
            .unwrap_or_default();
        let mut r = text(StatusCode::OK, "wadspaces stream proxy\n");
        if let Ok(v) = HeaderValue::from_str(&format!("{}:{nonce}", self.streams.instance)) {
            r.headers_mut().insert(HeaderName::from_static(SELFTEST_HEADER), v);
        }
        r
    }

    /// Passes a request to the sidecar on `port` unchanged (but for the
    /// hop-by-hop headers), and a websocket through both ways.
    async fn forward(
        &self,
        mut req: Request<Incoming>,
        port: u16,
        activity: Arc<Activity>,
    ) -> Result<Response<Body>, String> {
        activity.touch();
        let upgrade = wants_upgrade(req.headers()).then(|| hyper::upgrade::on(&mut req));
        let (mut parts, body) = req.into_parts();
        strip_hop_by_hop(&mut parts.headers, upgrade.is_some());
        let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/").to_string();
        parts.uri = pq.parse().map_err(|e: hyper::http::uri::InvalidUri| e.to_string())?;
        let tcp = tokio::time::timeout(CONNECT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| "connect: timed out".to_string())?
            .map_err(|e| format!("connect: {e}"))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await.map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = conn.with_upgrades().await;
        });
        let mut resp = sender.send_request(Request::from_parts(parts, body)).await.map_err(|e| e.to_string())?;
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS
            && let Some(client) = upgrade
        {
            let upstream = hyper::upgrade::on(&mut resp);
            let streams = self.streams.clone();
            tokio::spawn(async move {
                let (Ok(a), Ok(b)) = (client.await, upstream.await) else { return };
                activity.opened();
                streams.viewers_changed();
                let mut a = Heard { io: TokioIo::new(a), activity: activity.clone() };
                let mut b = TokioIo::new(b);
                let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                activity.closed();
                streams.viewers_changed();
            });
            let (p, _) = resp.into_parts();
            return Ok(Response::from_parts(p, Empty::new().map_err(|e| match e {}).boxed()));
        }
        strip_hop_by_hop(resp.headers_mut(), false);
        Ok(resp.map(|b| b.boxed()))
    }
}

/// The Host a request is for: lowercased, without its port.
fn host_of(req: &Request<Incoming>) -> String {
    let h = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| req.uri().host().map(String::from))
        .unwrap_or_default();
    let h = h.trim().trim_end_matches('.');
    let h = match h.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => h,
    };
    h.to_ascii_lowercase()
}

/// A stream's guid: 32 lowercase hex.
fn is_guid(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn wants_upgrade(h: &HeaderMap) -> bool {
    h.contains_key(header::UPGRADE)
        && h.get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
}

/// RFC 9110 §7.6.1: what's only for this hop (and what Connection names)
/// isn't passed on; an upgrade keeps Connection and Upgrade.
fn strip_hop_by_hop(h: &mut HeaderMap, upgrade: bool) {
    let named: Vec<HeaderName> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .filter(|n| !(upgrade && *n == header::UPGRADE))
        .collect();
    for n in named {
        h.remove(n);
    }
    for n in [
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
    ] {
        h.remove(n);
    }
    h.remove("keep-alive");
    h.remove("proxy-connection");
    if upgrade {
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    } else {
        h.remove(header::CONNECTION);
        h.remove(header::UPGRADE);
    }
}

fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut r = Response::new(Full::new(Bytes::from_static(body.as_bytes())).map_err(|e| match e {}).boxed());
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn not_found() -> Response<Body> {
    text(StatusCode::NOT_FOUND, "Not found\n")
}

/// A viewer's side of a websocket: whatever it sends counts as someone
/// watching (the idle stop's clock).
struct Heard<T> {
    io: T,
    activity: Arc<Activity>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Heard<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.io).poll_read(cx, buf);
        if matches!(r, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.activity.touch();
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Heard<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_headers_stay_on_their_hop() {
        let mut h = HeaderMap::new();
        h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, x-secret"));
        h.insert("x-secret", HeaderValue::from_static("1"));
        h.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic d2FkOnB3"));
        h.insert(header::COOKIE, HeaderValue::from_static("a=b"));
        strip_hop_by_hop(&mut h, false);
        assert!(h.get("x-secret").is_none() && h.get("keep-alive").is_none() && h.get(header::CONNECTION).is_none());
        // The login and the rest go through as they came.
        assert_eq!(h[header::AUTHORIZATION], "Basic d2FkOnB3");
        assert_eq!(h[header::COOKIE], "a=b");

        let mut ws = HeaderMap::new();
        ws.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        ws.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        assert!(wants_upgrade(&ws));
        strip_hop_by_hop(&mut ws, true);
        assert_eq!(
            (ws[header::CONNECTION].to_str().unwrap(), ws[header::UPGRADE].to_str().unwrap()),
            ("upgrade", "websocket")
        );
    }

    #[test]
    fn guids_are_exactly_32_lowercase_hex() {
        assert!(is_guid("0123456789abcdef0123456789abcdef"));
        assert!(!is_guid("0123456789ABCDEF0123456789abcdef"));
        assert!(!is_guid("0123456789abcdef"));
        assert!(!is_guid("0123456789abcdef0123456789abcdeg"));
    }
}
