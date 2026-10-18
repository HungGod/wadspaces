//! The tailnet and the remote API on a fake tailscaled: who may ask, asks
//! the machine shows, trusting and removing, joining and leaving, the trust
//! starting over when the machine's Tailscale login changes; and, linked to
//! a fake Firebase on a fake podman, the owner's sign-in past hello and a
//! wadspace built from the account, opened, stopped and streamed.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use wad_config::{Config, Profile};
use wad_firebase::{Functions, Identity};
use wadd::cloud::{CloudRelay, Endpoints, Parts, Settings};
use wadd::logbuf::LogBuffer;
use wadd::{Listen, Server};

use crate::common::Fake;
use crate::common::firebase::FakeFirebase;

const RUNNING: &str = include_str!("../../../../fixtures/tailscale/status-running.json");
const NEEDS_LOGIN: &str = include_str!("../../../../fixtures/tailscale/status-needs-login.json");
const PHONE: &str = include_str!("../../../../fixtures/tailscale/whois-phone.json");
const TAGGED: &str = include_str!("../../../../fixtures/tailscale/whois-tagged.json");
const SHARED: &str = include_str!("../../../../fixtures/tailscale/whois-shared.json");

/// What the fake tailscaled says: its status, and which device each
/// connection (by its address and port) is from.
#[derive(Clone, Default)]
struct Ts {
    status: Arc<Mutex<String>>,
    whois: Arc<Mutex<HashMap<String, String>>>,
}

async fn tailscaled(dir: &Path, status: &str) -> (PathBuf, Ts) {
    let sock = dir.join("tailscaled.sock");
    let ts = Ts::default();
    *ts.status.lock().unwrap() = status.into();
    let (s, w) = (ts.status.clone(), ts.whois.clone());
    let app = Router::new().route("/localapi/v0/status", get(move || async move { s.lock().unwrap().clone() })).route(
        "/localapi/v0/whois",
        get(move |Query(q): Query<HashMap<String, String>>| async move {
            match w.lock().unwrap().get(q.get("addr").map(String::as_str).unwrap_or_default()) {
                Some(j) => (StatusCode::OK, j.clone()),
                None => (StatusCode::NOT_FOUND, "no match for IP:port".to_string()),
            }
        }),
    );
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (sock, ts)
}

/// A `tailscale` that writes down what it's asked; `up` prints a login link.
fn cli(dir: &Path) -> (PathBuf, PathBuf) {
    let log = dir.join("cli-args");
    let bin = dir.join("tailscale");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\ncase \"$2\" in\n  up) printf 'To authenticate, visit:\\n\\n\\thttps://login.example/a/77\\n' >&2; sleep 2 ;;\nesac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A file just written can be "busy" for a moment (ETXTBSY) if another
    // test forked meanwhile: run it once first.
    for _ in 0..50 {
        if std::process::Command::new(&bin).arg("--probe").status().is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(&log);
    (bin, log)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Env {
    dir: tempfile::TempDir,
    sock: PathBuf,
    port: u16,
    ts: Ts,
    log: PathBuf,
    /// Linked: the account, and the machine's podman.
    fb: FakeFirebase,
    fake: Fake,
    _stop: tokio::sync::oneshot::Sender<()>,
}

const BASE: &str = "localhost/wadspaces-base:trixie";

async fn start(status: &str) -> Env {
    start_with(status, false).await
}

/// `linked`: to the account (owner u1) as machine m1, on a fake podman
/// with the base and stream images; else with no account and no podman.
async fn start_with(status: &str, linked: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let (ts_sock, ts) = tailscaled(dir.path(), status).await;
    let (bin, log) = cli(dir.path());
    let sock = dir.path().join("wadd.sock");
    let port = free_port();
    let mut cfg = Config::defaults(Profile::User);
    cfg.machine.name = "Test Machine".into();
    cfg.daemon.state_dir = dir.path().join("state");
    cfg.display.enabled = false;
    cfg.streaming.listen = vec![];
    cfg.daemon.projects_dir = dir.path().join("projects");
    cfg.tailnet = wad_config::Tailnet {
        enabled: true,
        socket: ts_sock,
        cli: bin,
        port,
        // As in the laptop trial: tailscaled (userspace) forwards to loopback.
        listen: vec![format!("127.0.0.1:{port}")],
        bind_device: String::new(),
        hostname: String::new(),
        poll_s: 1,
    };
    cfg.streaming.require_tunnel = false;
    // One stream's ports, the web one answering (its sidecar's nginx).
    let web = free_port();
    (cfg.streaming.first_web_port, cfg.streaming.first_mux_port, cfg.streaming.ports) = (web, free_port(), 1);
    // A wadspace opened here waits this long for its window (there's no screen).
    cfg.daemon.ready_timeout_s = 3;
    std::fs::create_dir_all(&cfg.daemon.state_dir).unwrap();
    let fake = Fake::default();
    fake.with(|m| {
        m.images.insert(BASE.into());
        m.images.insert("localhost/wadspaces-stream:trixie".into());
        m.answering.insert(format!("http://127.0.0.1:{web}/"));
        m.free_gb = 100.0;
    });
    let backend: Arc<dyn wadd::backend::Backend> =
        if linked { Arc::new(fake.clone()) } else { Arc::new(wadd::backend::Offline("no machine in tests".into())) };
    let mut server =
        Server::new(&cfg, Profile::User, Listen::Path(sock.clone()), LogBuffer::new(100), backend).unwrap();
    let st = Arc::get_mut(&mut server.state).unwrap();
    st.backlight = wadd::screen::Backlight::new(dir.path().join("backlight"));
    let fb = FakeFirebase::default();
    if linked {
        let base = fb.serve().await;
        fb.code("ABC123", "u1");
        let http = reqwest::Client::new();
        let ends = Endpoints {
            identity: Identity::with_base(http.clone(), &base),
            functions: Functions::new(http.clone(), &format!("{base}/functions")),
            firestore_base: base.clone(),
        };
        let settings = Settings {
            project_id: "demo".into(),
            api_key: "key".into(),
            heartbeat: Duration::from_secs(30),
            poll: Duration::from_secs(30),
            machine_name: "Test Machine".into(),
            state_dir: cfg.daemon.state_dir.clone(),
        };
        let parts = Parts {
            registry: st.registry.clone(),
            view: st.view.clone(),
            projects: st.projects.clone(),
            secrets: st.secrets.clone(),
            launches: st.launches.clone(),
            github: st.github.clone(),
            bus: st.bus.clone(),
            builds: st.builds.clone(),
            signins: st.signins.clone(),
            backend: st.registry_backend.clone(),
            tailnet: None,
        };
        let relay = CloudRelay::new(settings, http, ends, parts);
        relay.link("abc123").await.unwrap();
        st.cloud = Some(relay);
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.run(async move {
        let _ = rx.await;
    }));
    let env = Env { dir, sock, port, ts, log, fb, fake, _stop: tx };
    env.until(|t| t["listening"] == true).await;
    env
}

async fn http(s: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin, method: &str, path: &str) -> (u16, Value) {
    send(s, method, path, None, "").await
}

/// One request (with the owner's sign-in when there's a `token`).
async fn send(
    mut s: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let kind = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: wadd\r\n{auth}{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    let status = buf[9..12].parse().unwrap();
    let body = buf.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or_default();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

impl Env {
    /// The machine's own API, over its socket.
    async fn local(&self, method: &str, path: &str) -> (u16, Value) {
        http(UnixStream::connect(&self.sock).await.unwrap(), method, path).await
    }

    async fn tailnet(&self) -> Value {
        self.local("GET", "/v1/tailnet").await.1
    }

    /// GET /v1/tailnet until `ok` says so (tailscaled is read each second).
    async fn until(&self, ok: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..80 {
            let t = self.tailnet().await;
            if ok(&t) {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("never: {}", self.tailnet().await);
    }

    /// The remote API, from the device `who` (a whois answer; None: not a
    /// device on the tailnet).
    async fn remote(&self, path: &str, who: Option<&str>) -> (u16, Value) {
        self.call("GET", path, who, None, "").await
    }

    /// The remote API from `who`, signed in as `token` (`user:<uid>`).
    async fn call(&self, method: &str, path: &str, who: Option<&str>, token: Option<&str>, body: &str) -> (u16, Value) {
        let s = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        if let Some(w) = who {
            self.ts.whois.lock().unwrap().insert(s.local_addr().unwrap().to_string(), w.into());
        }
        send(s, method, path, token, body).await
    }

    /// The owner's trusted phone, signed in as the owner.
    async fn owner(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        self.call(method, path, Some(PHONE), Some("user:u1"), body).await
    }

    async fn trust_the_phone(&self) {
        self.remote("/v1/remote/hello", Some(PHONE)).await;
        let (status, t) = self.local("PUT", "/v1/tailnet/devices/nPhone1CNTRL").await;
        assert_eq!(status, 200, "{t}");
    }
}

#[tokio::test]
async fn only_the_owners_trusted_devices_get_in() {
    let e = start(RUNNING).await;
    let t = e.tailnet().await;
    assert_eq!(
        (t["state"].as_str(), t["dnsName"].as_str()),
        (Some("running"), Some("wadspaces-surface.tail1234.ts.net"))
    );
    assert_eq!(t["user"], "owner@example.com");
    // Only the owner's own, untagged, not-shared-in devices are listed.
    let names: Vec<_> = t["devices"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Pixel 9"]);
    assert_eq!(t["devices"][0]["trusted"], false);

    // Not on the tailnet, tagged, someone else's: refused, and no ask.
    for (who, says) in [(None, "on the owner's tailnet"), (Some(TAGGED), "Tagged"), (Some(SHARED), "owner's own")] {
        let (status, body) = e.remote("/v1/remote/hello", who).await;
        assert_eq!(status, 403, "{body}");
        assert!(body["message"].as_str().unwrap().contains(says), "{body}");
    }
    assert_eq!(e.tailnet().await["asks"], json!([]));

    // The owner's phone says hello: the machine asks whether to trust it.
    let (status, hello) = e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(status, 200, "{hello}");
    assert_eq!((hello["device"].as_str(), hello["trust"].as_str()), (Some("Pixel 9"), Some("asked")));
    assert_eq!(hello["machine"], "Test Machine");
    let t = e.tailnet().await;
    assert_eq!(t["asks"][0]["id"], "nPhone1CNTRL");
    assert_eq!(t["asks"][0]["os"], "android");
    // ... and it can't do anything else yet.
    let (status, body) = e.remote("/v1/remote/wadspaces", Some(PHONE)).await;
    assert_eq!(status, 403, "{body}");
    assert!(body["message"].as_str().unwrap().contains("Allow Pixel 9 on Test Machine first"), "{body}");

    // Allowed on the machine: trusted, remembered, the ask gone.
    let (status, t) = e.local("PUT", "/v1/tailnet/devices/nPhone1CNTRL").await;
    assert_eq!(status, 200, "{t}");
    assert_eq!((t["devices"][0]["trusted"].clone(), t["asks"].clone()), (json!(true), json!([])));
    let file = e.dir.path().join("state/tailnet-devices.json");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let book: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!((book["user"].clone(), book["devices"][0]["name"].clone()), (json!(2001), json!("Pixel 9")));
    let (_, hello) = e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(hello["trust"], "trusted");
    // The bare address says hello too (what a person types).
    let (status, hello) = e.remote("/", Some(PHONE)).await;
    assert_eq!((status, hello["trust"].as_str()), (200, Some("trusted")));
    // The machine's full API isn't there, nor a capitalised path, nor a
    // browser's favicon: no such endpoint, not "sign in".
    for path in ["/v1/secrets", "/V1/remote/hello", "/favicon.ico"] {
        let (status, body) = e.remote(path, Some(PHONE)).await;
        assert_eq!(status, 404, "{path}: {body}");
    }
    assert!(e.tailnet().await["devices"][0]["lastUsed"].is_u64());

    // Removed: it's asking again.
    let (status, _) = e.local("DELETE", "/v1/tailnet/devices/nPhone1CNTRL").await;
    assert_eq!(status, 200);
    let (_, hello) = e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(hello["trust"], "asked");
    // "Not now": the ask goes, and asking again doesn't bring it back for a while.
    let (_, t) = e.local("POST", "/v1/tailnet/asks/nPhone1CNTRL/dismiss").await;
    assert_eq!(t["asks"], json!([]));
    e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(e.tailnet().await["asks"], json!([]));
    // A device that isn't here can't be trusted by its id.
    let (status, _) = e.local("PUT", "/v1/tailnet/devices/nSomeoneElse").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn joining_shows_the_link_and_leaving_lets_go() {
    let e = start(NEEDS_LOGIN).await;
    let t = e.tailnet().await;
    assert_eq!(t["state"], "needsLogin");
    // Nothing gets in while the machine isn't on the tailnet.
    let (status, body) = e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(status, 403);
    assert!(body["message"].as_str().unwrap().contains("isn't on the tailnet"), "{body}");

    let (status, t) = e.local("POST", "/v1/tailnet/login").await;
    assert_eq!(status, 200, "{t}");
    assert_eq!(t["state"], "loggingIn");
    assert!(t["loginUrl"].as_str().unwrap().starts_with("https://"), "{t}");
    // Joining twice is the same join.
    let (status, again) = e.local("POST", "/v1/tailnet/login").await;
    assert_eq!((status, again["state"].clone()), (200, json!("loggingIn")));
    // Approved: tailscaled says it's running.
    *e.ts.status.lock().unwrap() = RUNNING.into();
    let t = e.until(|t| t["state"] == "running").await;
    assert!(t["loginUrl"].is_null());
    let (status, _) = e.local("POST", "/v1/tailnet/login").await;
    assert_eq!(status, 409, "already on the tailnet");

    let (status, _) = e.local("POST", "/v1/tailnet/logout").await;
    assert_eq!(status, 200);
    let args = std::fs::read_to_string(&e.log).unwrap();
    let args: Vec<_> = args.lines().map(|l| l.split_once(' ').unwrap().1).collect();
    assert_eq!(
        args,
        ["up --reset --accept-dns=false --timeout=600s --hostname=test-machine", "logout"],
        "one join for both asks, then leaving"
    );
}

#[tokio::test]
async fn a_new_tailscale_login_starts_trust_over() {
    let e = start(RUNNING).await;
    e.remote("/v1/remote/hello", Some(PHONE)).await;
    e.local("PUT", "/v1/tailnet/devices/nPhone1CNTRL").await;
    assert_eq!(e.tailnet().await["devices"][0]["trusted"], true);
    // The machine is now logged in as someone else.
    let mut other: Value = serde_json::from_str(RUNNING).unwrap();
    other["Self"]["UserID"] = json!(3001);
    *e.ts.status.lock().unwrap() = other.to_string();
    let t = e.until(|t| t["user"] == "friend@example.com").await;
    assert!(t["devices"].as_array().unwrap().iter().all(|d| d["trusted"] == false), "{t}");
    let book: Value =
        serde_json::from_slice(&std::fs::read(e.dir.path().join("state/tailnet-devices.json")).unwrap()).unwrap();
    assert_eq!((book["user"].clone(), book["devices"].clone()), (json!(3001), json!([])));
    // The owner's phone is now someone else's: refused.
    let (status, _) = e.remote("/v1/remote/hello", Some(PHONE)).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn the_heartbeat_says_where_the_remote_api_is() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, _ts) = tailscaled(dir.path(), RUNNING).await;
    let bus = wadd::events::Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::User,
        started_at: 0,
        screen: None,
    });
    let settings = wad_config::Tailnet { socket: sock, ..wad_config::Tailnet::for_profile(Profile::System) };
    let t = wadd::tailnet::Tailnet::new(&settings, "Surface", dir.path(), bus);
    t.refresh().await;
    assert_eq!(
        t.heartbeat_value().unwrap(),
        json!({
            "state": "running", "ips": ["100.101.102.103", "fd7a:115c:a1e0::a01:2b03"],
            "dnsName": "wadspaces-surface.tail1234.ts.net", "stableId": "nSurf1CNTRL",
            "port": 8087, "listening": false,
        })
    );
    // Off here: nothing in the heartbeat.
    let off = wadd::tailnet::Tailnet::new(
        &wad_config::Tailnet::for_profile(Profile::User),
        "x",
        dir.path(),
        wadd::events::Bus::new(wad_proto::v1::MachineInfo {
            name: "t".into(),
            version: "0".into(),
            profile: wad_proto::v1::Profile::User,
            started_at: 0,
            screen: None,
        }),
    );
    assert!(off.heartbeat_value().is_none());
}

#[tokio::test]
async fn past_hello_the_owners_sign_in_is_needed_too() {
    let e = start_with(RUNNING, true).await;
    e.trust_the_phone().await;
    let path = "/v1/remote/wadspaces";
    let (status, body) = e.call("GET", path, Some(PHONE), None, "").await;
    assert_eq!(status, 401, "{body}");
    assert!(body["message"].as_str().unwrap().contains("Sign in"), "{body}");
    for wrong in ["user:u2", "not-a-token"] {
        let (status, body) = e.call("GET", path, Some(PHONE), Some(wrong), "").await;
        assert_eq!(status, 401, "{wrong}: {body}");
    }
    let (status, list) = e.owner("GET", path, "").await;
    assert_eq!((status, list), (200, json!([])));
    // Checked once, then kept for a while.
    e.owner("GET", path, "").await;
    assert_eq!(e.fb.0.lock().unwrap().lookups, 3);
    // The owner's sign-in on a device that isn't trusted: still refused.
    e.local("DELETE", "/v1/tailnet/devices/nPhone1CNTRL").await;
    let (status, _) = e.owner("GET", path, "").await;
    assert_eq!(status, 403);
}

fn put_library(fb: &FakeFirebase) {
    fb.put(
        "users/u1/templates/dev",
        json!({"owner": "u1", "name": "Dev", "description": "", "apps": ["wadbrowser", "vscode"], "createdAt": "t", "updatedAt": "t"}),
    );
    fb.put(
        "users/u1/extensions/dev-plus",
        json!({"owner": "u1", "name": "Dev plus", "description": "", "templateId": "dev", "browserMode": "browser",
               "webapps": [], "extensions": ["anthropic.claude-code"], "tools": [], "autostart": [], "createdAt": "t", "updatedAt": "t"}),
    );
    let stack = |owner: &str, name: &str| {
        json!({"version": 3, "owner": owner, "sharedWith": [], "order": 1, "template": "dev", "extensions": "dev-plus",
               "customization": {"name": name, "description": "", "wallpaper": {"type": "gradient", "value": wad_wallpaper::PRESETS[0].1},
                                 "projects": [], "secrets": {"bySlot": {}}},
               "createdAt": "t", "updatedAt": "t"})
    };
    fb.put("wadspaces/cosmic", stack("u1", "Cosmic"));
    fb.put("wadspaces/theirs", stack("u2", "Theirs"));
    let mut gone = stack("u1", "Gone");
    gone["deletedAt"] = json!("2026-10-01T00:00:00.000Z");
    fb.put("wadspaces/gone", gone);
    fb.put("wadspaces/unnamed", stack("u1", " "));
}

#[tokio::test]
async fn a_trusted_device_builds_opens_and_streams_the_owners_wadspace() {
    let e = start_with(RUNNING, true).await;
    e.trust_the_phone().await;
    put_library(&e.fb);
    for (id, status) in [("theirs", 403), ("gone", 404), ("nope", 404), ("unnamed", 409)] {
        let (got, body) = e.owner("POST", &format!("/v1/remote/wadspaces/{id}/build"), "").await;
        assert_eq!(got, status, "{id}: {body}");
    }

    // Built from the account: installed, its wallpaper drawn, its layers built.
    let (status, b) = e.owner("POST", "/v1/remote/wadspaces/cosmic/build", "").await;
    assert_eq!(status, 200, "{b}");
    assert_eq!(
        (b["workspace"].as_str(), b["layer"].as_str(), b["built"].as_bool()),
        (Some("cosmic"), Some("custom:dev-plus"), Some(false))
    );
    let id = b["buildId"].as_str().unwrap().to_string();
    let mut log = Value::Null;
    for _ in 0..600 {
        (_, log) = e.owner("GET", &format!("/v1/remote/builds/{id}?since=0"), "").await;
        if log["build"]["status"] != "queued" && log["build"]["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(log["build"]["status"], "done", "{log}");
    let wall = std::fs::read(e.dir.path().join("state/extra/cosmic/wallpaper.png")).unwrap();
    assert!(wall.starts_with(b"\x89PNG"));
    let built = e.fake.0.lock().unwrap().builds.len();
    assert_eq!(built, 2, "the template, then the Extensions layer");
    // Asked again: it's built, nothing to do.
    let (_, b) = e.owner("POST", "/v1/remote/wadspaces/cosmic/build", "").await;
    assert_eq!((b["built"].as_bool(), b["buildId"].is_null()), (Some(true), true), "{b}");
    assert_eq!(e.fake.0.lock().unwrap().builds.len(), built);
    let (_, list) = e.owner("GET", "/v1/remote/wadspaces", "").await;
    assert_eq!(list[0]["id"], "cosmic");
    assert_eq!((list[0]["built"].as_bool(), list[0]["streamed"].as_bool()), (Some(true), Some(false)));

    // Streamed: the sign-in link comes back to the phone.
    let (status, share) = e.owner("POST", "/v1/remote/wadspaces/cosmic/stream", "{}").await;
    assert_eq!(status, 200, "{share}");
    let link = share["openLink"].as_str().unwrap();
    assert!(link.starts_with("https://ws-") && link.contains(".wadspaces.com/open#"), "{link}");
    let (_, list) = e.owner("GET", "/v1/remote/wadspaces", "").await;
    assert_eq!(list[0]["streamed"], true);
    let (status, _) = e.owner("DELETE", "/v1/remote/wadspaces/cosmic/stream", "").await;
    assert_eq!(status, 204);

    // Opened on the machine's screen, and stopped.
    let (status, launch) = e.owner("POST", "/v1/remote/wadspaces/cosmic/open", "").await;
    assert_eq!(status, 201, "{launch}");
    let (status, _) = e.owner("POST", "/v1/remote/wadspaces/cosmic/stop", "").await;
    assert_eq!(status, 204);
    // Only wadspaces that are here.
    let (status, _) = e.owner("POST", "/v1/remote/wadspaces/nope/open", "").await;
    assert_eq!(status, 404);
}
