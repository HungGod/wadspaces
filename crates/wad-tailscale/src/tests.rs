use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;

use axum::Router;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;

use super::*;

const RUNNING: &str = include_str!("../../../fixtures/tailscale/status-running.json");
const NEEDS_LOGIN: &str = include_str!("../../../fixtures/tailscale/status-needs-login.json");
const PHONE: &str = include_str!("../../../fixtures/tailscale/whois-phone.json");
const TAGGED: &str = include_str!("../../../fixtures/tailscale/whois-tagged.json");

/// A tailscaled that answers from the fixtures; `status` is the one it reports.
async fn fake(status: &'static str) -> (tempfile::TempDir, LocalApi) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("tailscaled.sock");
    let app = Router::new()
        .route(
            "/localapi/v0/status",
            get(move |h: HeaderMap| async move {
                assert_eq!(h["host"], HOST, "the LocalAPI wants its own Host");
                assert!(h.get("origin").is_none() && h.get("referer").is_none(), "it refuses browsers");
                status
            }),
        )
        .route(
            "/localapi/v0/whois",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q.get("proto").map(String::as_str), Some("tcp"));
                match q.get("addr").map(String::as_str) {
                    Some("100.64.0.7:50123") | Some("127.0.0.1:41000") => (StatusCode::OK, PHONE),
                    Some("100.64.0.9:50124") => (StatusCode::OK, TAGGED),
                    Some(_) => (StatusCode::NOT_FOUND, "no match for IP:port"),
                    None => (StatusCode::BAD_REQUEST, "missing 'addr' parameter"),
                }
            }),
        );
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (dir, LocalApi::new(sock))
}

#[tokio::test]
async fn reads_a_running_machine() {
    let (_d, api) = fake(RUNNING).await;
    let s = api.status().await.unwrap();
    assert!(s.running());
    assert_eq!(s.ips(), ["100.101.102.103".parse::<IpAddr>().unwrap(), "fd7a:115c:a1e0::a01:2b03".parse().unwrap()]);
    let me = s.me.as_ref().unwrap();
    assert_eq!((me.id.as_str(), me.dns()), ("nSurf1CNTRL", "wadspaces-surface.tail1234.ts.net"));
    assert_eq!(me.key_expiry.as_deref(), Some("2027-04-16T03:00:00Z"));
    assert_eq!(s.me_user().unwrap().login_name, "owner@example.com");
    assert_eq!(s.tailnet.as_ref().unwrap().name, "owner@example.com");
    let names: Vec<_> = s.peer_list().iter().map(|p| p.host_name.as_str()).collect();
    assert_eq!(names, ["build-box", "friends-laptop", "Pixel 9"]);
    let phone = s.peer_list().into_iter().find(|p| p.os == "android").unwrap();
    assert_eq!((phone.user_id, phone.online, phone.tags.is_empty()), (2001, true, true));
    let server = s.peers.values().find(|p| p.host_name == "build-box").unwrap();
    assert_eq!(server.tags, ["tag:server"]);
    assert!(s.peers.values().any(|p| p.sharee_node));
}

#[tokio::test]
async fn reads_a_machine_waiting_to_log_in() {
    let (_d, api) = fake(NEEDS_LOGIN).await;
    let s = api.status().await.unwrap();
    assert!(!s.running());
    assert_eq!(s.backend_state, "NeedsLogin");
    assert_eq!(s.auth_url, "https://login.tailscale.com/a/1a2b3c4d5e6f");
    // Go's nulls read as empty.
    assert!(s.ips().is_empty() && s.peers.is_empty() && s.users.is_empty() && s.tailnet.is_none());
    assert!(s.me_user().is_none());
    assert_eq!(s.health, ["You are logged out."]);
}

#[tokio::test]
async fn says_which_device_a_connection_is_from() {
    let (_d, api) = fake(RUNNING).await;
    let w = api.whois("100.64.0.7:50123".parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(
        (w.node.stable_id.as_str(), w.node.user, w.user.login_name.as_str()),
        ("nPhone1CNTRL", 2001, "owner@example.com")
    );
    assert!(w.node.tags.is_empty());
    assert_eq!(w.device_name(), "Pixel 9");
    assert_eq!(w.node.hostinfo.os, "android");
    // Userspace tailscaled: a forwarded connection's loopback port.
    assert!(api.whois("127.0.0.1:41000".parse().unwrap()).await.unwrap().is_some());
    let t = api.whois("100.64.0.9:50124".parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(t.node.tags, ["tag:server"]);
    // Not a device on the tailnet.
    assert!(api.whois("192.168.1.5:40000".parse().unwrap()).await.unwrap().is_none());
    assert!(api.whois("[fd7a:115c:a1e0::99]:443".parse().unwrap()).await.unwrap().is_none());
}

#[tokio::test]
async fn no_tailscaled() {
    let api = LocalApi::new("/nonexistent/tailscaled.sock");
    assert!(matches!(api.status().await, Err(Error::Unreachable(_))));
}

#[test]
fn device_names_fall_back() {
    let mut w: WhoIs = serde_json::from_str(PHONE).unwrap();
    w.node.hostinfo.hostname.clear();
    assert_eq!(w.device_name(), "pixel-9");
    w.node.computed_name.clear();
    assert_eq!(w.device_name(), "pixel-9");
}

#[test]
fn finds_the_login_link() {
    let out = "\nTo authenticate, visit:\n\n\thttps://login.tailscale.com/a/5f6e7d\n\n";
    assert_eq!(url_in(out).as_deref(), Some("https://login.tailscale.com/a/5f6e7d"));
    assert_eq!(url_in("Success."), None);
    assert_eq!(url_in("https://"), None);
}

/// A `tailscale` that writes its arguments down; `up` prints a login link and
/// waits a moment, `logout` fails with a message.
fn fake_cli(dir: &Path) -> (Cli, PathBuf) {
    let log = dir.join("args");
    let bin = dir.join("tailscale");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\ncase \"$2\" in\n  up) printf '\\nTo authenticate, visit:\\n\\n\\thttps://login.example/a/77\\n\\n' >&2; sleep 0.3; echo Success. ;;\n  logout) echo 'not logged in' >&2; exit 1 ;;\nesac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A file just written can still be "busy" (ETXTBSY) for a moment if
    // another test thread forked meanwhile: run it once before the test does.
    for _ in 0..50 {
        match std::process::Command::new(&bin).arg("--probe").status() {
            Ok(_) => break,
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let _ = std::fs::remove_file(&log);
    (Cli::new(&bin, "/run/ts.sock"), log)
}

#[tokio::test]
async fn joins_and_leaves_with_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (cli, log) = fake_cli(dir.path());
    let mut login = cli.up(Some("wadspaces-surface"), Duration::from_secs(600)).unwrap();
    let mut url = None;
    for _ in 0..100 {
        url = login.url();
        if url.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(url.as_deref(), Some("https://login.example/a/77"));
    assert!(login.running());
    login.wait().await.unwrap();
    let e = cli.logout().await.unwrap_err();
    assert_eq!(e.to_string(), "not logged in");
    cli.set_hostname("surface-2").await.unwrap();
    let args = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "--socket=/run/ts.sock up --reset --accept-dns=false --timeout=600s --hostname=wadspaces-surface",
            "--socket=/run/ts.sock logout",
            "--socket=/run/ts.sock set --hostname=surface-2",
        ]
    );
}

#[tokio::test]
async fn a_cli_that_isnt_there() {
    let cli = Cli::new("/nonexistent/tailscale", "/run/ts.sock");
    assert!(matches!(cli.up(None, Duration::from_secs(1)), Err(Error::Cli(_))));
    assert!(cli.logout().await.is_err());
}
