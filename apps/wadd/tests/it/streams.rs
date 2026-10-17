//! Streams on a fake machine: a wadspace drawn on its sidecar and back,
//! the login only in podman, the screen's consent, the person at the
//! machine winning, wadd restarting, and streams ending by themselves.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, ws};
use serde_json::json;
use wad_proto::ErrorCode;
use wad_proto::v1::{Display, Phase};
use wadd::copies::Copies;
use wadd::display::NullDisplay;
use wadd::drives::Drives;
use wadd::events::Bus;
use wadd::launches::Launches;
use wadd::projects::Projects;
use wadd::registry::{Registry, Settings};
use wadd::streams::Streams;
use wadd::tunnel::Tunnel;
use wadd::view::View;

const IMAGE: &str = "localhost/wadspaces-template-a:latest";
const STREAM_IMAGE: &str = "localhost/wadspaces-stream:trixie";

struct Env {
    _d: tempfile::TempDir,
    state: std::path::PathBuf,
    fake: Fake,
    reg: Arc<Registry>,
    view: Arc<View>,
    launches: Arc<Launches>,
    bus: Bus,
}

fn settings(first: u16) -> wad_config::Streaming {
    wad_config::Streaming {
        require_tunnel: false,
        first_web_port: first,
        first_mux_port: first + 50,
        ports: 50,
        metrics: "127.0.0.1:9".into(),
        ..Default::default()
    }
}

/// A machine with wadspaces a and b (on the screen) and the stream image.
async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let tmp = wad_store::folders::realpath(d.path());
    let state = tmp.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let fake = Fake::default();
    fake.with(|m| {
        m.images.insert(IMAGE.into());
        m.images.insert(STREAM_IMAGE.into());
    });
    let bus = Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::User,
        started_at: 0,
        screen: None,
    });
    let reg = Registry::new(
        Arc::new(fake.clone()),
        bus.clone(),
        Settings {
            ready_timeout: Duration::from_secs(10),
            projects_dir: tmp.join("projects").to_string_lossy().into(),
            signins_dir: tmp.join("signins"),
            state_dir: state.clone(),
            rootless_uid: None,
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    let view = View::new(reg.clone(), Arc::new(fake.clone()), Arc::new(NullDisplay), bus.clone(), state.clone());
    let drives = Arc::new(Drives::new(1000, Arc::new(common::host::FakeHost::new(None)), true));
    let machine: wadd::projects::MachineRef = Arc::new(|| ("local".to_string(), "Surface".to_string()));
    let copies = Arc::new(Copies::new(&state, vec![], 1000, machine.clone(), drives.clone(), reg.clone()));
    let projects = Arc::new(Projects::new(
        &state,
        tmp.join("projects"),
        vec![],
        1000,
        machine,
        drives,
        Arc::new(wad_git::Git::default()),
        reg.clone(),
        bus.clone(),
    ));
    let launches =
        Launches::new(reg.clone(), view.clone(), Arc::new(fake.clone()), projects, copies, bus.clone(), &state);
    Env { _d: d, state, fake, reg, view, launches, bus }
}

fn streams(e: &Env, s: wad_config::Streaming) -> Arc<Streams> {
    let tunnel = Arc::new(Tunnel::new(&s, "inst".into(), false));
    Streams::new(
        s,
        &e.state,
        e.reg.clone(),
        e.view.clone(),
        Arc::new(e.fake.clone()),
        e.bus.clone(),
        tunnel,
        e.launches.clone(),
        "inst".into(),
    )
}

async fn load(e: &Env) {
    e.reg.load(vec![ws("a", IMAGE, Display::Host, None), ws("b", IMAGE, Display::Host, None)]).await.unwrap();
}

fn unit_names(e: &Env) -> Vec<String> {
    let mut n: Vec<String> = e.fake.0.lock().unwrap().units.iter().map(|u| u.0.clone()).collect();
    n.sort();
    n
}

fn unit(e: &Env, name: &str) -> String {
    e.fake.0.lock().unwrap().units.iter().find(|(n, _)| n == name).map(|u| u.1.clone()).unwrap_or_default()
}

/// The port a streamed wadspace's sidecar got (the first free one in the
/// range: one another program holds is skipped).
fn port_of(e: &Env, id: &str) -> u64 {
    e.reg.stream_of(id).unwrap()["web_port"].as_u64().unwrap()
}

/// A streamed wadspace's sidecar answers (its nginx's login counts).
fn answering(e: &Env, id: &str) {
    let port = port_of(e, id);
    e.fake.with(|m| {
        m.answering.insert(format!("http://127.0.0.1:{port}/"));
    });
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what}: never");
}

fn guid_of(link: &str) -> String {
    link.strip_prefix("https://ws-").and_then(|l| l.strip_suffix(".wadspaces.com/")).unwrap().to_string()
}

#[tokio::test]
async fn a_stream_draws_on_its_sidecar_and_ends() {
    let e = env().await;
    let s = streams(&e, settings(58100));
    load(&e).await;
    let share = s.start("a", false).await.unwrap();
    let guid = guid_of(&share.link);
    assert!(guid.len() == 32 && guid.bytes().all(|b| b.is_ascii_hexdigit()), "{}", share.link);
    assert!(share.user.starts_with("wad-") && share.password.len() == 20);
    // The password is podman's, and nowhere in wadd's own files.
    assert_eq!(e.fake.0.lock().unwrap().secrets.get("wad_stream_a").unwrap(), share.password.as_bytes());
    let book = std::fs::read_to_string(e.state.join("streams.json")).unwrap();
    assert!(book.contains(&guid) && !book.contains(&share.password), "{book}");
    // Its units: the wadspace on the sidecar's display, the sidecar, their volume.
    assert_eq!(
        unit_names(&e),
        ["wad-a-display.container", "wad-a-display.volume", "wad-a.container", "wad-b.container"]
    );
    assert!(unit(&e, "wad-a.container").contains("BindsTo=wad-a-display.service"));
    let display = unit(&e, "wad-a-display.container");
    let port = port_of(&e, "a");
    assert!((58100..58150).contains(&port));
    assert!(display.contains(&format!("PublishPort=127.0.0.1:{port}:3000")), "{display}");
    assert!(display.contains(&format!("Label=wadspaces.stream={guid}")));
    assert!(!display.contains(&share.password) && !display.contains("STUN"));
    // It comes up on its sidecar: that first, ready when it answers.
    answering(&e, "a");
    until("ready", || e.reg.state("a").is_some_and(|s| s.phase == Phase::Ready)).await;
    let calls = e.fake.calls();
    let at = |c: &str| calls.iter().position(|x| x == c).unwrap_or_else(|| panic!("{c}: {calls:?}"));
    assert!(at("start wad-a-display.service") < at("start wad-a.service"));
    assert!(e.reg.state("a").unwrap().streaming);
    assert_eq!(s.route(&guid).map(|r| u64::from(r.0)), Some(port));
    assert!(s.list()[0].live && s.list()[0].link == share.link);
    // It's off Super+Tab.
    assert!(!e.view.running_items().iter().any(|w| w.id == "a"));
    // Starting it again hands back the same stream.
    assert_eq!(s.start("a", false).await.unwrap(), share);
    assert_eq!(s.share("a").await.unwrap(), share);
    // Stopped: both units stop, it's on the screen next time, the link 404s.
    s.stop("a").await.unwrap();
    assert!(e.fake.calls().contains(&"stop wad-a-display.service".to_string()));
    assert_eq!(unit_names(&e), ["wad-a.container", "wad-b.container"]);
    assert!(!e.fake.0.lock().unwrap().containers.contains_key("wad-a"));
    until("password gone", || !e.fake.0.lock().unwrap().secrets.contains_key("wad_stream_a")).await;
    assert!(s.route(&guid).is_none() && s.list().is_empty());
    assert!(!e.reg.state("a").unwrap().streaming);
    assert_eq!(s.stop("a").await.unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn on_the_screen_it_asks_first_and_the_person_at_the_machine_wins() {
    let e = env().await;
    let s = streams(&e, settings(58200));
    load(&e).await;
    e.reg.start("a").unwrap();
    e.reg.settled("a").await;
    assert_eq!(e.reg.state("a").unwrap().container, "running");
    let refused = s.start("a", false).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert!(s.list().is_empty() && !e.fake.0.lock().unwrap().secrets.contains_key("wad_stream_a"));
    // Taken off the screen, then streamed.
    let share = s.start("a", true).await.unwrap();
    assert!(e.fake.calls().contains(&"stop wad-a.service".to_string()));
    answering(&e, "a");
    until("streamed", || e.reg.state("a").is_some_and(|s| s.phase == Phase::Ready && s.streaming)).await;
    // Switched to at the machine: its stream ends and it's on the screen.
    e.view.switch("a").unwrap();
    until("back on the screen", || !e.reg.is_streamed("a") && s.list().is_empty()).await;
    until("running again", || e.reg.state("a").is_some_and(|s| s.container == "running")).await;
    assert_eq!(unit_names(&e), ["wad-a.container", "wad-b.container"]);
    assert!(s.route(&guid_of(&share.link)).is_none());
}

#[tokio::test]
async fn wadd_restarting_keeps_running_streams_and_stops_the_rest() {
    let e = env().await;
    // Before the restart: streams for a and b; only a's sidecar still runs,
    // and a sidecar for c that wadd has no stream for.
    let book = json!([
        {"workspace": "a", "guid": "a".repeat(32), "web_port": 58300, "mux_port": 58350, "user": "wad-aaaaaa", "started": 1.0},
        {"workspace": "b", "guid": "b".repeat(32), "web_port": 58301, "mux_port": 58351, "user": "wad-bbbbbb", "started": 1.0},
    ]);
    std::fs::write(e.state.join("streams.json"), book.to_string()).unwrap();
    e.fake.with(|m| {
        m.secrets.insert("wad_stream_a".into(), b"pa".to_vec());
        m.secrets.insert("wad_stream_b".into(), b"pb".to_vec());
        for (name, id, guid) in [("wad-a-display", "a", "a".repeat(32)), ("wad-c-display", "c", "c".repeat(32))] {
            m.containers.insert(name.into(), "running".into());
            m.container_labels.insert(
                name.into(),
                json!({"wadspaces.id": id, "wadspaces.role": "display", "wadspaces.stream": guid})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        m.containers.insert("wad-a".into(), "running".into());
    });
    let s = streams(&e, settings(58300));
    s.adopt().await;
    load(&e).await;
    // a: routed again, drawn on its sidecar from the first load.
    assert_eq!(s.route(&"a".repeat(32)).map(|r| r.0), Some(58300));
    assert!(unit_names(&e).contains(&"wad-a-display.container".to_string()));
    // b: over, its password gone; c's sidecar: stopped.
    assert!(s.route(&"b".repeat(32)).is_none() && !e.reg.is_streamed("b"));
    assert!(!unit_names(&e).contains(&"wad-b-display.container".to_string()));
    until("b's password gone", || !e.fake.0.lock().unwrap().secrets.contains_key("wad_stream_b")).await;
    assert!(e.fake.calls().contains(&"stop wad-c-display.service".to_string()), "{:?}", e.fake.calls());
    assert_eq!(s.list().len(), 1);
}

#[tokio::test]
async fn streams_end_when_their_wadspace_stops_or_nobody_watches() {
    let e = env().await;
    let s = streams(&e, wad_config::Streaming { idle_minutes: 1, ..settings(58400) });
    load(&e).await;
    s.start("a", false).await.unwrap();
    s.start("b", false).await.unwrap();
    answering(&e, "a");
    answering(&e, "b");
    until("both up", || ["a", "b"].iter().all(|w| e.reg.state(w).is_some_and(|s| s.phase == Phase::Ready))).await;
    s.tick().await;
    assert_eq!(s.list().len(), 2);
    // Podman not answering for a moment (its API service restarting) isn't
    // a wadspace stopping: both streams stay, still ready.
    e.fake.with(|m| m.podman_down = true);
    e.reg.reconcile().await;
    s.tick().await;
    e.fake.with(|m| m.podman_down = false);
    assert_eq!(s.list().len(), 2);
    assert!(["a", "b"].iter().all(|w| e.reg.state(w).is_some_and(|s| s.phase == Phase::Ready)));
    // a's wadspace stopped by itself: its stream ends.
    e.fake.with(|m| {
        m.containers.remove("wad-a");
    });
    e.reg.reconcile().await;
    s.tick().await;
    assert!(!e.reg.is_streamed("a"));
    // Nobody watched b for longer than idle_minutes: it ends.
    s.unwatched_for_tests("b", 30);
    s.tick().await;
    assert!(e.reg.is_streamed("b"));
    s.unwatched_for_tests("b", 61);
    s.tick().await;
    assert!(!e.reg.is_streamed("b") && s.list().is_empty());
}

#[tokio::test]
async fn not_without_the_tunnel_or_the_stream_image() {
    let e = env().await;
    // cloudflared isn't connected (nothing answers its metrics).
    let s = streams(&e, wad_config::Streaming { require_tunnel: true, ..settings(58500) });
    load(&e).await;
    let r = s.start("a", false).await.unwrap_err();
    assert_eq!(r.code, ErrorCode::Offline, "{}", r.message);
    assert!(r.message.contains("Cloud Tunnel"));
    let s = streams(&e, settings(58500));
    e.fake.with(|m| {
        m.images.remove(STREAM_IMAGE);
    });
    let r = s.start("a", false).await.unwrap_err();
    assert_eq!(r.code, ErrorCode::Conflict, "{}", r.message);
    assert!(e.fake.0.lock().unwrap().secrets.is_empty());
}

#[tokio::test]
async fn a_streams_password_is_out_of_everyone_elses_reach() {
    let fake = Fake::default();
    let d = tempfile::tempdir().unwrap();
    let secrets = wadd::secrets::Secrets::new(Arc::new(fake.clone()), d.path());
    fake.with(|m| {
        m.secrets.insert("wad_stream_a".into(), b"pw".to_vec());
    });
    assert_eq!(secrets.set("wad_stream_a", b"mine").await.unwrap_err().code, ErrorCode::BadRequest);
    assert_eq!(secrets.delete("wad_stream_a").await.unwrap_err().code, ErrorCode::BadRequest);
    secrets.sync_account(&[("wad_stream_a".into(), "account".into())]).await.unwrap();
    secrets.ensure(&["wad_turn_api_token".into()]).await.unwrap();
    assert!(secrets.list().await.unwrap().iter().all(|s| !s.name.starts_with("wad_")));
    assert_eq!(fake.0.lock().unwrap().secrets.get("wad_stream_a").unwrap(), b"pw");
    assert!(!fake.0.lock().unwrap().secrets.contains_key("wad_turn_api_token"));
    // Nor may a design mount one.
    let mut w = ws("x", IMAGE, Display::Host, None);
    w.secrets = vec!["wad_stream_a".into()];
    let e = wad_store::workspaces::check_workspaces(vec![wad_store::workspaces::to_yaml(&w)]).unwrap_err();
    assert!(e.to_string().contains("wadd's own"), "{e}");
}
