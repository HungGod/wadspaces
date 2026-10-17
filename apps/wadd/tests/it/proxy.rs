//! The streaming proxy on real sockets, in front of a made-up sidecar (raw
//! TCP: it asks for a login, echoes what it was sent, and upgrades to a
//! websocket that echoes bytes). What the tunnel would send it, by Host.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::Fake;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wadd::streams::Streams;

const GUID: &str = "0123456789abcdef0123456789abcdef";
const LOGIN: &str = "Basic d2FkLXVzZXI6cGFzc3dvcmQ="; // wad-user:password
/// The stream's sign-in link's key.
const KEY: &str = "fedcba9876543210fedcba9876543210";

/// The made-up sidecar: one request per connection.
async fn sidecar() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let head = read_head(&mut s).await;
                let lower = head.to_ascii_lowercase();
                if lower.contains("upgrade: websocket") {
                    s.write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\n\r\n",
                    )
                    .await
                    .unwrap();
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                } else if !head.contains(LOGIN) {
                    s.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"Login\"\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                } else {
                    let body = head.replace("\r\n", "\n");
                    let r =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Sidecar: yes\r\n\r\n{body}", body.len());
                    s.write_all(r.as_bytes()).await.unwrap();
                }
            });
        }
    });
    port
}

async fn read_head(s: &mut TcpStream) -> String {
    let mut head = vec![];
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).await.unwrap_or(0) == 0 {
            break;
        }
        head.push(b[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// A stream routed to the made-up sidecar, and the proxy in front of it.
async fn proxy() -> (tempfile::TempDir, u16, Arc<Streams>) {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().to_path_buf();
    let port = sidecar().await;
    let fake = Fake::default();
    std::fs::write(
        state.join("streams.json"),
        json!([{"workspace": "a", "guid": GUID, "web_port": port, "mux_port": 1, "user": "wad-user", "started": 1.0}])
            .to_string(),
    )
    .unwrap();
    fake.with(|m| {
        m.secrets.insert("wad_stream_a".into(), b"password".to_vec());
        m.secrets.insert("wad_streamkey_a".into(), KEY.as_bytes().to_vec());
        m.containers.insert("wad-a-display".into(), "running".into());
        m.container_labels.insert(
            "wad-a-display".into(),
            json!({"wadspaces.id": "a", "wadspaces.stream": GUID, "wadspaces.role": "display"})
                .as_object()
                .unwrap()
                .clone(),
        );
    });
    let bus = wadd::events::Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::User,
        started_at: 0,
        screen: None,
    });
    let reg = wadd::registry::Registry::new(
        Arc::new(fake.clone()),
        bus.clone(),
        wadd::registry::Settings {
            ready_timeout: Duration::from_secs(5),
            projects_dir: state.join("p").to_string_lossy().into(),
            signins_dir: state.join("s"),
            state_dir: state.clone(),
            rootless_uid: None,
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    let view = wadd::view::View::new(
        reg.clone(),
        Arc::new(fake.clone()),
        Arc::new(wadd::display::NullDisplay),
        bus.clone(),
        state.clone(),
    );
    let drives = Arc::new(wadd::drives::Drives::new(1000, Arc::new(common::host::FakeHost::new(None)), true));
    let machine: wadd::projects::MachineRef = Arc::new(|| ("local".to_string(), "m".to_string()));
    let copies =
        Arc::new(wadd::copies::Copies::new(&state, vec![], 1000, machine.clone(), drives.clone(), reg.clone()));
    let projects = Arc::new(wadd::projects::Projects::new(
        &state,
        state.join("p"),
        vec![],
        1000,
        machine,
        drives,
        Arc::new(wad_git::Git::default()),
        reg.clone(),
        bus.clone(),
    ));
    let launches = wadd::launches::Launches::new(
        reg.clone(),
        view.clone(),
        Arc::new(fake.clone()),
        projects,
        copies,
        bus.clone(),
        &state,
    );
    let settings = wad_config::Streaming::default();
    let tunnel = Arc::new(wadd::tunnel::Tunnel::new(&settings, "inst-1".into(), false));
    let streams = Streams::new(settings, &state, reg, view, Arc::new(fake), bus, tunnel, launches, "inst-1".into());
    streams.adopt().await;
    let listeners = wadd::proxy::bind(&["127.0.0.1:0".into()]).await;
    let proxy_port = listeners[0].local_addr().unwrap().port();
    let (_stop, stop) = tokio::sync::watch::channel(false);
    let p = wadd::proxy::Proxy::new(streams.clone());
    tokio::spawn(async move {
        let _keep = _stop;
        p.serve(listeners, stop).await
    });
    (d, proxy_port, streams)
}

/// One request as the tunnel would send it; the whole answer.
async fn ask(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = vec![];
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

fn get(host: &str, path: &str, extra: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{extra}\r\n")
}

#[tokio::test]
async fn a_stream_is_passed_through_as_it_came() {
    let (_d, port, streams) = proxy().await;
    let host = format!("ws-{GUID}.wadspaces.com");
    // No login: the sidecar's prompt comes back untouched.
    let r = ask(port, &get(&host, "/", "")).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(
        r.contains("www-authenticate: Basic realm=\"Login\"") || r.contains("WWW-Authenticate: Basic realm=\"Login\""),
        "{r}"
    );
    // The login (and the rest) reach it as sent; hop-by-hop headers don't.
    let extra =
        format!("Authorization: {LOGIN}\r\nCookie: a=b\r\nCf-Connecting-Ip: 1.2.3.4\r\nKeep-Alive: timeout=5\r\n");
    // Any case, with a port: still this stream (the Host goes on as it came).
    let r = ask(port, &get(&format!("WS-{GUID}.WadSpaces.com:443"), "/a/b?c=d", &extra)).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let lower = r.to_ascii_lowercase();
    assert!(r.contains("GET /a/b?c=d HTTP/1.1"), "{r}");
    assert!(r.contains(LOGIN) && lower.contains("cookie: a=b") && lower.contains("cf-connecting-ip: 1.2.3.4"), "{r}");
    assert!(lower.contains(&format!("host: ws-{GUID}.wadspaces.com:443")), "{r}");
    assert!(!lower.contains("keep-alive: timeout"), "{r}");
    // Someone touched it.
    assert!(streams.list()[0].last_active > 1.0);
}

#[tokio::test]
async fn the_sign_in_link_signs_in() {
    let (_d, port, _streams) = proxy().await;
    let host = format!("ws-{GUID}.wadspaces.com");
    // Its page: nothing secret in it (the key is after the `#`, never sent).
    let r = ask(port, &get(&host, "/open", "")).await;
    assert!(r.starts_with("HTTP/1.1 200") && r.contains("Opening the stream") && !r.contains(KEY), "{r}");
    let post = |key: &str| {
        format!(
            "POST /open HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{key}",
            key.len()
        )
    };
    // The wrong key: refused, no cookie.
    let r = ask(port, &post("0123456789abcdef0123456789abcdee")).await;
    assert!(r.starts_with("HTTP/1.1 403") && !r.to_ascii_lowercase().contains("set-cookie"), "{r}");
    // The right one: the cookie, for this stream's site only.
    let r = ask(port, &post(KEY)).await;
    assert!(r.starts_with("HTTP/1.1 204"), "{r}");
    let cookie = r.lines().find(|l| l.to_ascii_lowercase().starts_with("set-cookie:")).unwrap().to_string();
    for part in [&format!("wadspaces_stream={KEY}"), "HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(cookie.contains(part), "{cookie}");
    }
    assert!(!cookie.to_ascii_lowercase().contains("domain="), "{cookie}");
    // With it, the proxy logs in for the viewer; the cookie stays here.
    let r = ask(port, &get(&host, "/", &format!("Cookie: theme=dark; wadspaces_stream={KEY}\r\n"))).await;
    assert!(r.starts_with("HTTP/1.1 200") && r.contains(LOGIN), "{r}");
    assert!(r.to_ascii_lowercase().contains("cookie: theme=dark") && !r.contains("wadspaces_stream"), "{r}");
    // A wrong cookie gets the login prompt, and isn't passed on either.
    let r = ask(port, &get(&host, "/", "Cookie: wadspaces_stream=nope\r\n")).await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    // The websocket too.
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET /api/websockets HTTP/1.1\r\nHost: {host}\r\nCookie: wadspaces_stream={KEY}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    assert!(read_head(&mut s).await.starts_with("HTTP/1.1 101"));
}

#[tokio::test]
async fn websockets_go_through_both_ways() {
    let (_d, port, streams) = proxy().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET /api/websockets HTTP/1.1\r\nHost: ws-{GUID}.wadspaces.com\r\nAuthorization: {LOGIN}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let head = read_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    s.write_all(b"frame one").await.unwrap();
    let mut buf = [0u8; 9];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf, b"frame one");
    for _ in 0..100 {
        if streams.list()[0].viewers == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(streams.list()[0].viewers, 1);
    drop(s);
    for _ in 0..200 {
        if streams.list()[0].viewers == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the viewer never left");
}

#[tokio::test]
async fn anything_else_is_not_found_and_the_selftest_is_answered_here() {
    let (_d, port, _streams) = proxy().await;
    for host in [
        "ws-ffffffffffffffffffffffffffffffff.wadspaces.com", // no such stream
        "ws-0123.wadspaces.com",                             // not a guid
        "www.wadspaces.com",
        "wadspaces.com",
        &format!("ws-{GUID}.example.com"),
        "localhost",
    ] {
        let r = ask(port, &get(host, "/", "")).await;
        assert!(r.starts_with("HTTP/1.1 404"), "{host}: {r}");
    }
    let nonce = "a".repeat(32);
    let r = ask(port, &get("ws-selftest.wadspaces.com", &format!("/?n={nonce}"), "")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.to_ascii_lowercase().contains(&format!("x-wadspaces-selftest: inst-1:{nonce}")), "{r}");
    // Only a nonce that looks like one comes back.
    let r = ask(port, &get("ws-selftest.wadspaces.com", "/?n=%3Cscript%3E", "")).await;
    assert!(r.to_ascii_lowercase().contains("x-wadspaces-selftest: inst-1:\r\n"), "{r}");
}

#[tokio::test]
async fn the_selftest_tells_this_wadd_from_another() {
    let (_d, port, _streams) = proxy().await;
    let s = wad_config::Streaming::default();
    let dir = tempfile::tempdir().unwrap();
    let here = (format!("http://127.0.0.1:{port}/"), Some("ws-selftest.wadspaces.com".to_string()));
    let mine = wadd::tunnel::Tunnel::new(&s, "inst-1".into(), false).for_tests(dir.path().join("token"), here.clone());
    let t = mine.selftest().await;
    assert!(t.ok, "{t:?}");
    assert_eq!(t.status, Some(200));
    let other = wadd::tunnel::Tunnel::new(&s, "inst-2".into(), false).for_tests(dir.path().join("token"), here);
    let t = other.selftest().await;
    assert!(!t.ok && t.detail.contains("Another machine"), "{t:?}");
    let nobody = wadd::tunnel::Tunnel::new(&s, "inst-1".into(), false)
        .for_tests(dir.path().join("token"), ("http://127.0.0.1:9/".into(), Some("ws-selftest.wadspaces.com".into())));
    assert!(!nobody.selftest().await.ok);
}
