//! What a machine keeps beside its wadspaces: the disk report, counted
//! again behind the last count; a workspace list that keeps the machine's
//! own apps' ids; and the file browser (what's saved, secrets kept out).
//! A fake podman; the real registry.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, ws};
use serde_json::json;
use wad_proto::ErrorCode;
use wad_proto::v1::Display;
use wadd::events::Bus;
use wadd::registry::{Registry, Settings};

struct Env {
    _d: tempfile::TempDir,
    state: std::path::PathBuf,
    fake: Fake,
    reg: Arc<Registry>,
    bus: Bus,
}

async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let fake = Fake::default();
    fake.with(|m| m.free_gb = 100.0);
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
            projects_dir: "/p".into(),
            signins_dir: d.path().join("signins"),
            state_dir: state.clone(),
            rootless_uid: Some(1000),
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    reg.load(vec![]).await.unwrap();
    Env { _d: d, state, fake, reg, bus }
}

#[tokio::test]
async fn storage_hands_out_its_last_count_and_counts_again_behind() {
    let e = env().await;
    let backend: Arc<dyn wadd::backend::Backend> = Arc::new(e.fake.clone());
    let s = wadd::storage::Storage::new(&e.state, false, backend, e.reg.clone(), e.bus.clone());
    let (_, mut rx) = e.bus.subscribe();
    let counted = async |rx: &mut tokio::sync::broadcast::Receiver<wad_proto::v1::Event>| loop {
        if let wad_proto::v1::Event::Storage { measured_at } = rx.recv().await.unwrap() {
            return measured_at;
        }
    };
    // None yet: counted now.
    let first = s.report(false).await;
    assert!(!first.measuring);
    assert_eq!(counted(&mut rx).await, first.measured_at);
    // Something went: the last count at once, a new one on its way.
    s.forget();
    let old = s.report(false).await;
    assert_eq!((old.measured_at, old.measuring), (first.measured_at, true));
    let next = tokio::time::timeout(Duration::from_secs(5), counted(&mut rx)).await.unwrap();
    assert!(next > first.measured_at);
    let now = s.report(false).await;
    assert_eq!((now.measured_at, now.measuring), (next, false));
}

#[test]
fn a_wadspace_cannot_take_an_apps_id() {
    let current = vec![common::app("terminal", "/usr/bin/foot"), ws("w", "i", Display::Host, None)];
    let fresh =
        json!({"id": "terminal", "name": "T", "image": "localhost/wadspaces-terminal:latest", "display": "host"});
    let e = wadd::builds::merged(&current, &fresh).unwrap_err();
    assert!(e.contains("one of this machine's own apps"), "{e}");
    let other = json!({"id": "w", "name": "W2", "image": "localhost/wadspaces-w:latest", "display": "host"});
    assert!(wadd::builds::merged(&current, &other).unwrap().1);
}

#[tokio::test]
async fn files_show_whats_saved_and_keep_secrets_out() {
    let e = env().await;
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    for (p, body) in [
        ("Downloads/report.md", "# Report\n"),
        ("Downloads/photo.bin", "\0\0\0binary"),
        (".ssh/id_ed25519", "KEY"),
        (".local/share/wadbrowser/profiles/github/Cookies", "SESSION"),
        (".local/share/notes.txt", "kept"),
    ] {
        std::fs::create_dir_all(home.join(p).parent().unwrap()).unwrap();
        std::fs::write(home.join(p), body).unwrap();
    }
    let files = wadd::files::Files::with_home(&e.state, &d.path().join("signins"), Some(home.clone()));
    let b = e.fake.clone();
    let top: Vec<String> =
        files.list("home", "", &e.reg, &b).await.unwrap().entries.into_iter().map(|x| x.name).collect();
    assert!(top.contains(&"Downloads".to_string()) && !top.contains(&".ssh".to_string()), "{top:?}");
    let downloads = files.list("home", "Downloads", &e.reg, &b).await.unwrap().entries;
    let shown: Vec<(&str, bool)> = downloads.iter().map(|x| (x.name.as_str(), x.text)).collect();
    assert_eq!(shown, [("photo.bin", false), ("report.md", true)]);
    // WadBrowser's profiles (signed-in sessions) stay out; the rest of .local is there.
    let names = |list: wadd::files::FileList| list.entries.into_iter().map(|x| x.name).collect::<Vec<_>>();
    assert_eq!(names(files.list("home", ".local/share", &e.reg, &b).await.unwrap()), ["wadbrowser", "notes.txt"]);
    assert!(names(files.list("home", ".local/share/wadbrowser", &e.reg, &b).await.unwrap()).is_empty());
    assert_eq!(
        files.list("home", ".local/share/wadbrowser/profiles", &e.reg, &b).await.unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(files.read_text("home", ".ssh/id_ed25519", &e.reg, &b).await.unwrap_err().code, ErrorCode::Forbidden);
    assert_eq!(files.list("home", "../", &e.reg, &b).await.unwrap_err().code, ErrorCode::BadRequest);
    // Read, edit, delete.
    assert_eq!(files.read_text("home", "Downloads/report.md", &e.reg, &b).await.unwrap(), "# Report\n");
    files.write_text("home", "Downloads/report.md", "# Report v2\n", &e.reg, &b).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("Downloads/report.md")).unwrap(), "# Report v2\n");
    files.delete("home", "Downloads", &e.reg, &b).await.unwrap();
    assert!(!home.join("Downloads").exists());
    assert_eq!(files.delete("home", "", &e.reg, &b).await.unwrap_err().code, ErrorCode::BadRequest);
    assert!(files.roots(&e.reg).await.iter().any(|r| r.id == "home"));
}
