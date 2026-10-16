//! The workspace lifecycle against a fake machine, with time paused.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, ws};
use wad_proto::v1::{Display, Event, Phase, Workspace};
use wadd::events::Bus;
use wadd::registry::{Registry, Settings};

struct Setup {
    reg: Arc<Registry>,
    fake: Fake,
    bus: Bus,
    _dir: tempfile::TempDir,
}

async fn setup(list: Vec<Workspace>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
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
            ready_timeout: Duration::from_secs(30),
            projects_dir: "/p".into(),
            signins_dir: "/s".into(),
            state_dir: dir.path().into(),
            rootless_uid: Some(1000),
            device_rules: vec![],
            poll: Duration::from_secs(1),
        },
    );
    reg.load(list).await.unwrap();
    Setup { reg, fake, bus, _dir: dir }
}

fn phase(s: &Setup, id: &str) -> Phase {
    s.reg.state(id).unwrap().phase
}

#[tokio::test(start_paused = true)]
async fn a_cold_start_starts_and_waits_for_the_stream() {
    let s = setup(vec![ws("a", "ghcr.io/o/a:1", Display::Stream, Some(3100))]).await;
    s.fake.with(|m| {
        m.images.insert("ghcr.io/o/a:1".into());
        m.answering.insert("http://127.0.0.1:3100/".into());
        m.answer_after = 2;
    });
    let (_, mut rx) = s.bus.subscribe();
    s.reg.start("a").unwrap();
    s.reg.settled("a").await;
    let st = s.reg.state("a").unwrap();
    assert_eq!(st.phase, Phase::Ready, "{st:?}");
    assert_eq!(st.container, "running");
    assert_eq!(st.image_present, Some(true));
    assert_eq!(s.fake.calls(), ["start wad-a.service"]);
    let mut phases = vec![];
    while let Ok(Event::WorkspaceState(e)) = rx.try_recv() {
        if phases.last() != Some(&e.phase) {
            phases.push(e.phase);
        }
    }
    assert_eq!(phases, [Phase::Starting, Phase::Waiting, Phase::Ready]);
    // A run opened with the container.
    let runs = wad_store::State::new(s._dir.path()).runs(None, 10);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, "stream");
}

#[tokio::test(start_paused = true)]
async fn a_warm_start_starts_nothing() {
    let s = setup(vec![ws("b", "ghcr.io/o/b:1", Display::Host, None)]).await;
    s.fake.with(|m| {
        m.images.insert("ghcr.io/o/b:1".into());
        m.containers.insert("wad-b".into(), "running".into());
    });
    s.reg.start("b").unwrap();
    s.reg.settled("b").await;
    assert_eq!(phase(&s, "b"), Phase::Ready);
    assert!(s.fake.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn failures_say_why() {
    let s = setup(vec![
        ws("local", "localhost/wadspaces-local:latest", Display::Host, None),
        ws("slow", "ghcr.io/o/slow:1", Display::Stream, Some(3101)),
    ])
    .await;
    s.reg.start("local").unwrap();
    s.reg.settled("local").await;
    let st = s.reg.state("local").unwrap();
    assert_eq!(st.phase, Phase::Error);
    assert!(st.error.unwrap().contains("build it in WadSpaces first"));
    assert_eq!(st.image_present, Some(false));
    assert!(s.fake.calls().is_empty(), "nothing is downloaded");

    s.fake.with(|m| {
        m.images.insert("ghcr.io/o/slow:1".into());
    });
    s.reg.start("slow").unwrap();
    s.reg.settled("slow").await;
    let st = s.reg.state("slow").unwrap();
    assert_eq!(st.phase, Phase::Error);
    assert_eq!(st.error.as_deref(), Some("SLOW didn't answer within 30 s"));
    // A new phase clears the old error.
    s.fake.with(|m| {
        m.answering.insert("http://127.0.0.1:3101/".into());
    });
    s.reg.start("slow").unwrap();
    s.reg.settled("slow").await;
    let st = s.reg.state("slow").unwrap();
    assert_eq!((st.phase, st.error), (Phase::Ready, None));
}

#[tokio::test(start_paused = true)]
async fn stop_and_restart() {
    let s = setup(vec![ws("c", "ghcr.io/o/c:1", Display::Host, None)]).await;
    s.fake.with(|m| {
        m.images.insert("ghcr.io/o/c:1".into());
    });
    s.reg.start("c").unwrap();
    s.reg.settled("c").await;
    s.reg.stop("c").await.unwrap();
    let st = s.reg.state("c").unwrap();
    assert_eq!((st.phase, st.container.as_str()), (Phase::Idle, "missing"));
    s.reg.restart("c").await.unwrap();
    s.reg.settled("c").await;
    assert_eq!(phase(&s, "c"), Phase::Ready);
    assert_eq!(s.fake.calls(), ["start wad-c.service", "stop wad-c.service", "restart wad-c.service"]);
    let runs = wad_store::State::new(s._dir.path()).runs(None, 10);
    assert_eq!(runs.len(), 2); // stopped, then running again
    assert!(s.reg.stop("nope").await.is_err());
}

#[tokio::test(start_paused = true)]
async fn reconcile_sees_what_happened_elsewhere() {
    let s = setup(vec![ws("d", "ghcr.io/o/d:1", Display::Stream, Some(3102))]).await;
    s.fake.with(|m| {
        m.images.insert("ghcr.io/o/d:1".into());
        m.containers.insert("wad-d".into(), "running".into());
        m.answering.insert("http://127.0.0.1:3102/".into());
    });
    s.reg.reconcile().await;
    let st = s.reg.state("d").unwrap();
    assert_eq!((st.phase, st.image_present), (Phase::Ready, Some(true)));
    s.fake.with(|m| {
        m.containers.insert("wad-d".into(), "exited".into());
    });
    s.reg.reconcile().await;
    let st = s.reg.state("d").unwrap();
    assert_eq!((st.phase, st.container.as_str()), (Phase::Idle, "exited"));
}

#[tokio::test(start_paused = true)]
async fn units_are_written_for_every_workspace() {
    let s = setup(vec![ws("g", "ghcr.io/o/g:1", Display::Host, None)]).await;
    let units = s.fake.0.lock().unwrap().units.clone();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].0, "wad-g.container");
    assert!(
        units[0].1.contains("UIDMap=1000:0:1") && units[0].1.contains("Volume=/run/user/1000:/run/wadspaces-display")
    );
}

#[tokio::test(start_paused = true)]
async fn autostart_starts_what_starts_at_boot_and_retries_it() {
    let mut boot = ws("boot", "localhost/wadspaces-template-boot:latest", Display::Host, None);
    boot.autostart = true;
    let s = setup(vec![ws("other", "localhost/wadspaces-template-other:latest", Display::Host, None), boot]).await;
    // Its image isn't built yet: it fails, then starts once it's there.
    let task = tokio::spawn(s.reg.clone().autostart(Duration::from_secs(300)));
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(phase(&s, "boot"), Phase::Error);
    s.fake.with(|m| {
        m.images.insert("localhost/wadspaces-template-boot:latest".into());
    });
    task.await.unwrap();
    assert_eq!(phase(&s, "boot"), Phase::Ready);
    assert_eq!(s.fake.calls(), ["start wad-boot.service"]);
    assert_eq!(phase(&s, "other"), Phase::Idle);
}
