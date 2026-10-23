//! wadd's API on a real socket: the right callers get answers, others 403,
//! and events stream.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use wad_config::{Config, Profile};
use wad_proto::v1::{Display, Workspace};
use wadd::logbuf::LogBuffer;
use wadd::{Listen, Server};

async fn start(allow_self: bool) -> (tempfile::TempDir, std::path::PathBuf, tokio::sync::oneshot::Sender<()>) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("wadd.sock");
    let mut cfg = Config::defaults(Profile::User);
    cfg.machine.name = "test-machine".into();
    cfg.daemon.state_dir = dir.path().join("state");
    cfg.display.enabled = false; // never a real sway
    cfg.streaming.listen = vec!["127.0.0.1:0".into()]; // never the real proxy's port
    cfg.daemon.projects_dir = dir.path().join("projects");
    std::fs::create_dir_all(dir.path().join("home/wad/Notes")).unwrap();
    cfg.daemon.folder_roots = vec![wad_store::folders::realpath(&dir.path().join("home"))];
    std::fs::create_dir_all(&cfg.daemon.state_dir).unwrap();
    let list = vec![workspace("writing")];
    std::fs::write(cfg.daemon.state_dir.join("workspaces.json"), serde_json::to_string(&list).unwrap()).unwrap();
    let offline = Arc::new(wadd::backend::Offline("no machine in tests".into()));
    let mut server =
        Server::new(&cfg, Profile::User, Listen::Path(sock.clone()), LogBuffer::new(100), offline).unwrap();
    let state = Arc::get_mut(&mut server.state).unwrap();
    // Never the laptop's own screen.
    state.backlight = wadd::screen::Backlight::new(dir.path().join("backlight"));
    if !allow_self {
        // As if wadd ran as someone else: this test's user isn't allowed.
        state.policy.own_uid = u32::MAX - 1;
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.run(async move {
        let _ = rx.await;
    }));
    (dir, sock, tx)
}

fn workspace(id: &str) -> Workspace {
    Workspace {
        id: id.into(),
        name: "Writing".into(),
        kind: Default::default(),
        exec: vec![],
        image: "ghcr.io/o/writing:1".into(),
        display: Display::Host,
        port: None,
        hotkey: None,
        icon: None,
        enabled: true,
        autostart: false,
        container_name: format!("wad-{id}"),
        container_port: 3000,
        env: vec![],
        secrets: vec![],
        volumes: vec![],
        devices: vec![],
        shm_size: None,
        projects: vec![],
        signins: vec![],
        ephemeral: false,
        desktop: vec![],
    }
}

/// One HTTP/1.1 request over the socket; the status and body.
async fn request(sock: &std::path::Path, method: &str, path: &str) -> (u16, String) {
    send(sock, method, path, "").await
}

async fn send(sock: &std::path::Path, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = UnixStream::connect(sock).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: wadd\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    let status = buf[9..12].parse().unwrap();
    let body = buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

async fn get(sock: &std::path::Path, path: &str) -> (u16, String) {
    request(sock, "GET", path).await
}

#[tokio::test]
async fn allowed_callers_get_answers() {
    let (_d, sock, stop) = start(true).await;
    let (status, body) = get(&sock, "/v1/health").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""ok":true"#), "{body}");
    let (status, body) = get(&sock, "/v1/machine").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""name":"test-machine""#) && body.contains(r#""profile":"user""#), "{body}");
    let (status, body) = get(&sock, "/v1/nope").await;
    assert_eq!(status, 404);
    assert!(body.contains(r#""code":"not_found""#), "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn others_are_refused() {
    let (_d, sock, stop) = start(false).await;
    let (status, body) = get(&sock, "/v1/health").await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains(r#""code":"forbidden""#), "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn events_start_with_the_current_state() {
    let (_d, sock, stop) = start(true).await;
    let mut s = UnixStream::connect(&sock).await.unwrap();
    s.write_all(b"GET /v1/events HTTP/1.1\r\nHost: wadd\r\n\r\n").await.unwrap();
    let mut got = String::new();
    let mut buf = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !got.contains("\n\n") || !got.contains("event: machine") {
        let n = tokio::time::timeout_at(deadline, s.read(&mut buf)).await.expect("no event in time").unwrap();
        assert!(n > 0, "closed: {got}");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(got.contains("text/event-stream"), "{got}");
    assert!(got.contains(r#"data: {"name":"test-machine""#), "{got}");
    while !got.contains("event: workspaceState") {
        let n = tokio::time::timeout_at(deadline, s.read(&mut buf)).await.expect("no state in time").unwrap();
        assert!(n > 0, "closed: {got}");
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(got.contains(r#""id":"writing""#), "{got}");
    let _ = stop.send(());
}

#[tokio::test]
async fn workspaces_start_and_say_why_they_failed() {
    let (_d, sock, stop) = start(true).await;
    let (status, body) = get(&sock, "/v1/workspaces").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""id":"writing""#), "{body}");
    let (status, body) = get(&sock, "/v1/states").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""phase":"idle""#), "{body}");
    let (status, body) = request(&sock, "POST", "/v1/workspaces/nope/start").await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = request(&sock, "POST", "/v1/workspaces/writing/start").await;
    assert_eq!(status, 202, "{body}");
    // No machine here: the bring-up fails, and the state says so.
    for _ in 0..100 {
        let (_, body) = get(&sock, "/v1/workspaces/writing/state").await;
        if body.contains(r#""phase":"error""#) {
            assert!(body.contains("no machine in tests"), "{body}");
            let _ = stop.send(());
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never failed");
}

#[tokio::test]
async fn the_socket_is_private_on_a_laptop_and_removed_on_exit() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, sock, stop) = start(true).await;
    let mode = std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let _ = stop.send(());
    for _ in 0..50 {
        if !sock.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("socket left behind");
}

#[tokio::test]
async fn the_view_and_sessions() {
    let (_d, sock, stop) = start(true).await;
    let (status, body) = get(&sock, "/v1/view").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""view":{"kind":"home"}"#) && body.contains(r#""pending":null"#), "{body}");
    let (status, body) = send(&sock, "POST", "/v1/session", r#"{"workspaces":["writing"],"minutes":25}"#).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""mode":"focus""#), "{body}");
    // Locked: WadSpaces Client waits, and ending needs ?force.
    let (status, body) = request(&sock, "POST", "/v1/view/home").await;
    assert_eq!(status, 409, "{body}");
    let (status, _) = request(&sock, "DELETE", "/v1/session").await;
    assert_eq!(status, 409);
    let (status, _) = request(&sock, "DELETE", "/v1/session?force=true").await;
    assert_eq!(status, 204);
    let (_, body) = get(&sock, "/v1/session").await;
    assert_eq!(body, "null");
    let (status, body) = send(&sock, "POST", "/v1/session", r#"{"workspaces":[],"minutes":null}"#).await;
    assert_eq!(status, 400, "{body}");
    let (status, _) = request(&sock, "POST", "/v1/carousel/next").await;
    assert_eq!(status, 204);
    let (status, _) = request(&sock, "POST", "/v1/carousel/sideways").await;
    assert_eq!(status, 404);
    // On a laptop the keyboard stays yours.
    let (_, body) = get(&sock, "/v1/keys").await;
    assert!(body.contains(r#""enabled":false"#) && body.contains(r#""grabbing":false"#), "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn projects() {
    let (d, sock, stop) = start(true).await;
    let notes =
        r#"{"name":"Notes","mountName":"Notes","source":{"kind":"git","url":"https://github.com/o/notes.git"}}"#;
    let (status, body) = send(&sock, "POST", "/v1/projects", notes).await;
    assert_eq!(status, 201, "{body}");
    let made: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 20);
    assert_eq!(made["source"], serde_json::json!({"kind": "git", "url": "https://github.com/o/notes.git"}));
    let (status, body) =
        send(&sock, "PUT", "/v1/projects/x2", &notes.replace(r#""name":"Notes""#, r#""name":"Clash""#)).await;
    assert_eq!(status, 409, "{body}"); // the same folder name
    let (status, body) = send(&sock, "PUT", "/v1/projects/x2", r#"{"name":"Bad"}"#).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("mountName"), "{body}");
    // A folder project: this machine's, symlinks resolved.
    let home = wad_store::folders::realpath(&d.path().join("home"));
    let folder = format!(
        r#"{{"name":"Mine","mountName":"Mine","source":{{"kind":"folder","path":"{}/wad/Notes"}}}}"#,
        home.display()
    );
    let (status, body) = send(&sock, "PUT", "/v1/projects/f1", &folder).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""machineId":"local""#) && body.contains(r#""machineName":"test-machine""#), "{body}");
    let (_, body) = get(&sock, "/v1/projects/f1/status").await;
    assert!(
        body.contains(r#""existsOnDisk":true"#)
            && body.contains(r#""available":true"#)
            && body.contains(r#""git":null"#),
        "{body}"
    );
    let (_, body) = get(&sock, &format!("/v1/projects/{id}/status")).await;
    assert!(body.contains(r#""existsOnDisk":false"#) && body.contains(r#""available":true"#), "{body}");
    // Deleting leaves a tombstone; purge removes the folder.
    std::fs::create_dir_all(d.path().join(format!("projects/{id}"))).unwrap();
    let (status, body) = request(&sock, "DELETE", &format!("/v1/projects/{id}?purge=true")).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""deleted":true"#) && body.contains(r#""purged":true"#), "{body}");
    assert!(!d.path().join(format!("projects/{id}")).exists());
    let (_, body) = get(&sock, "/v1/projects").await;
    assert!(!body.contains(&id) && body.contains("f1"), "{body}");
    let (_, body) = get(&sock, "/v1/projects?deleted=true").await;
    assert!(body.contains(&id), "{body}");
    assert_eq!(get(&sock, "/v1/projects/nope").await.0, 404);
    // The folder picker stays inside the roots.
    let (status, body) = get(&sock, &format!("/v1/browse?path={}/wad", home.display())).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"Notes""#), "{body}");
    assert_eq!(get(&sock, "/v1/browse?path=/etc").await.0, 403);
    assert_eq!(get(&sock, &format!("/v1/browse?path={}/nope", home.display())).await.0, 404);
    assert_eq!(get(&sock, "/v1/browse?drive=x%20y").await.0, 400);
    // Launches: the workspace's machine is offline here, so it fails, and says so.
    let (status, body) = send(&sock, "POST", "/v1/launches", r#"{"workspace":"writing","projects":["f1"]}"#).await;
    assert_eq!(status, 201, "{body}");
    let launch: serde_json::Value = serde_json::from_str(&body).unwrap();
    for _ in 0..100 {
        let (_, body) = get(&sock, &format!("/v1/launches/{}", launch["id"].as_str().unwrap())).await;
        if body.contains(r#""status":"error""#) {
            assert!(body.contains("no machine in tests"), "{body}");
            let _ = stop.send(());
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the launch never failed");
}

#[tokio::test]
async fn builds() {
    let (_d, sock, stop) = start(true).await;
    // The old Builder's layers aren't built any more, nor designs: a
    // wadspace's set is (POST /v1/wadspaces/{id}/build, sets.rs).
    assert_eq!(send(&sock, "POST", "/v1/images/build", r#"{"layer":"template:tryb"}"#).await.0, 405);
    assert_eq!(send(&sock, "POST", "/v1/builds", r#"{"design":{"id":"x"}}"#).await.0, 405);
    // Only a version 5 wadspace installs (not the templates' version 4).
    let (status, body) =
        send(&sock, "PUT", "/v1/wadspaces/old", r#"{"wadspace":{"version":4,"id":"old","templates":[]}}"#).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("version 5"), "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn packages() {
    let (_d, sock, stop) = start(true).await;
    // No podman here: no base to read its ABI from.
    let (status, body) = get(&sock, "/v1/packages").await;
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("no machine in tests"), "{body}");
    assert_eq!(request(&sock, "DELETE", "/v1/packages/fetch").await.0, 404, "no fetch to stop");
    let (status, body) = send(&sock, "POST", "/v1/packages/fetch", r#"{"ids":["no-such-package"]}"#).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = send(&sock, "POST", "/v1/packages/fetch", r#"{"ids":["vscode"]}"#).await;
    assert_eq!(status, 202, "{body}");
    assert!(body.contains(r#""status":"building""#), "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn github_without_a_token() {
    let (_d, sock, stop) = start(true).await;
    let (status, body) = get(&sock, "/v1/github").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""token":false"#), "{body}");
    let (status, body) = get(&sock, "/v1/github/repos").await;
    assert_eq!(status, 409, "{body}");
    let _ = stop.send(());
}

#[tokio::test]
async fn network_and_power_without_the_system_bus() {
    let (_d, sock, stop) = start(true).await;
    let (status, body) = get(&sock, "/v1/network").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""available":false"#), "{body}");
    assert_eq!(get(&sock, "/v1/network/wifi").await.0, 503);
    let (status, body) =
        send(&sock, "POST", "/v1/network/wifi/connect", r#"{"ssid":"Home","password":"correct horse"}"#).await;
    assert_eq!(status, 503, "{body}");
    assert!(!body.contains("correct horse"), "{body}");
    let (status, _) = send(&sock, "POST", "/v1/power", r#"{"action":"reboot"}"#).await;
    assert_eq!(status, 503); // and nothing was asked of the real logind
    let (status, _) = send(&sock, "POST", "/v1/power", r#"{"action":"explode"}"#).await;
    assert_eq!(status, 422);
    let _ = stop.send(());
}

#[tokio::test]
async fn the_screens_brightness() {
    let (d, sock, stop) = start(true).await;
    // No backlight (an external monitor): says so.
    assert_eq!(get(&sock, "/v1/screen/brightness").await.1, r#"{"available":false,"percent":0}"#);
    assert_eq!(send(&sock, "PUT", "/v1/screen/brightness", r#"{"percent":50}"#).await.0, 404);
    let dev = d.path().join("backlight/intel_backlight");
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("type"), "raw\n").unwrap();
    std::fs::write(dev.join("max_brightness"), "7500\n").unwrap();
    std::fs::write(dev.join("brightness"), "3750\n").unwrap();
    assert_eq!(get(&sock, "/v1/screen/brightness").await.1, r#"{"available":true,"percent":50}"#);
    let (status, body) = send(&sock, "PUT", "/v1/screen/brightness", r#"{"percent":80}"#).await;
    assert_eq!((status, body.as_str()), (200, r#"{"available":true,"percent":80}"#));
    assert_eq!(std::fs::read_to_string(dev.join("brightness")).unwrap(), "6000");
    assert_eq!(send(&sock, "PUT", "/v1/screen/brightness", r#"{"percent":"lots"}"#).await.0, 422);
    let _ = stop.send(());
}

#[tokio::test]
async fn editing_workspaces_the_library_and_diagnostics() {
    let (_d, sock, stop) = start(true).await;
    let mut w = workspace("notes");
    w.name = "Notes".into();
    w.hotkey = Some(3);
    let (status, body) = send(&sock, "POST", "/v1/workspaces", &serde_json::to_string(&w).unwrap()).await;
    assert_eq!(status, 201, "{body}");
    let (status, _) = send(&sock, "POST", "/v1/workspaces", &serde_json::to_string(&w).unwrap()).await;
    assert_eq!(status, 409); // it exists
    w.name = "Notes 2".into();
    let (status, body) = send(&sock, "PUT", "/v1/workspaces/notes", &serde_json::to_string(&w).unwrap()).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"Notes 2""#) && body.contains(r#""restartRequired":false"#), "{body}");
    // Checked with the rest: writing has no hotkey 3, but two workspaces can't share a container name.
    let mut clash = workspace("other");
    clash.container_name = "wad-notes".into();
    let (status, body) = send(&sock, "POST", "/v1/workspaces", &serde_json::to_string(&clash).unwrap()).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(request(&sock, "DELETE", "/v1/workspaces/notes").await.0, 204);
    assert_eq!(request(&sock, "DELETE", "/v1/workspaces/notes").await.0, 404);
    // The library.
    let (status, _) = send(&sock, "PUT", "/v1/library/browsers/b1", r#"{"name":"Mail"}"#).await;
    assert_eq!(status, 200);
    assert!(get(&sock, "/v1/library/browsers/b1").await.1.contains("Mail"));
    assert!(get(&sock, "/v1/library/browsers").await.1.contains("Mail"));
    assert_eq!(send(&sock, "PUT", "/v1/library/secrets/x", "{}").await.0, 404);
    // The old Builder's are gone.
    for c in ["drafts", "templates", "extensions", "wadspaces"] {
        assert_eq!(send(&sock, "PUT", &format!("/v1/library/{c}/d1"), "{}").await.0, 404);
    }
    assert_eq!(send(&sock, "PUT", "/v1/library/browsers/..", "{}").await.0, 400);
    assert_eq!(request(&sock, "DELETE", "/v1/library/browsers/b1").await.0, 204);
    // Load, diagnostics, logs.
    let (status, body) = get(&sock, "/v1/metrics").await;
    assert_eq!(status, 200);
    assert!(body.contains(r#""memTotal""#), "{body}");
    let (status, body) = get(&sock, "/v1/diagnostics").await;
    assert_eq!(status, 200);
    assert!(
        body.contains(r#""logUnits":["wadd","greetd","wad-writing"]"#) && body.contains("no machine in tests"),
        "{body}"
    );
    assert_eq!(get(&sock, "/v1/logs/unit/sshd").await.0, 404); // not on the list
    let (status, body) = get(&sock, "/v1/logs/workspace/writing").await;
    assert_eq!(status, 503, "{body}");
    let _ = stop.send(());
}
