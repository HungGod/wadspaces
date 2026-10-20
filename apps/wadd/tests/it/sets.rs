//! Version 4 wadspaces on a machine: installed on their set with what they
//! take at start (web apps and their icons, layout, WadBrowser's mode, the
//! background in the machine's theme), and the set built from packages as
//! chain images, each build starting from the longest chain stored; one
//! set for every wadspace with the same packages; the base they need; and
//! the tidy-up and the disk page leaving packages and chains alone. A fake
//! podman and a fake registry.

use crate::common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{Fake, TOKEN};
use serde_json::{Value, json};
use wad_core::packages as pk;
use wad_proto::ErrorCode;
use wad_proto::v1::{BuildLog, BuildStatus, StorageDelete, Theme};
use wadd::builds::Builds;
use wadd::events::Bus;
use wadd::icons::Icons;
use wadd::packages::{Packages, Settings as PkgSettings};
use wadd::registry::{Registry, Settings};
use wadd::sets::{self, Here};

const BASE: &str = "localhost/wadspaces-base:trixie";
const ABI: &str = "trixie.1";
const REGISTRY: &str = "ghcr.io/hunggod";

fn labels(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// What CI published: two tools, VS Code (on electron-deps), an extension.
fn published() -> Vec<(&'static str, char, Vec<&'static str>)> {
    vec![
        ("cpp", '1', vec![]),
        ("rust", '2', vec![]),
        ("electron-deps", 'e', vec![]),
        ("vscode", 'c', vec!["electron-deps"]),
        ("ext-rust-lang.rust-analyzer", 'a', vec!["vscode", "electron-deps"]),
    ]
}

struct Env {
    _d: tempfile::TempDir,
    state: PathBuf,
    fake: Fake,
    reg: Arc<Registry>,
    builds: Arc<Builds>,
}

impl Env {
    fn here(&self) -> Here<'_> {
        Here { registry: &self.reg, backend: &self.fake, builds: &self.builds, state_dir: &self.state, machine: None }
    }

    fn extra(&self, id: &str, file: &str) -> PathBuf {
        self.state.join("extra").join(id).join(file)
    }

    /// The images podman was asked to build, in order.
    fn built(&self) -> Vec<String> {
        self.fake.0.lock().unwrap().builds.iter().map(|b| b.0.clone()).collect()
    }

    fn label(&self, image: &str, key: &str) -> Option<String> {
        let m = self.fake.0.lock().unwrap();
        m.image_labels.get(image).and_then(|l| l.get(key)).and_then(Value::as_str).map(String::from)
    }

    /// A build's log once it's finished.
    async fn wait(&self, id: &str) -> BuildLog {
        for _ in 0..3000 {
            let l = self.builds.log(id, 0).unwrap();
            if l.build.status.finished() {
                return l;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("build stuck: {:?}", self.builds.log(id, 0).unwrap());
    }

    /// Installs `w` and builds its set to the end; the build's log, if one ran.
    async fn install_and_build(&self, w: &Value) -> Option<BuildLog> {
        let id = w["id"].as_str().unwrap();
        let (_, b) = sets::install_and_build(&self.here(), id, w, Some("Pacific/Fiji")).await.unwrap();
        match b.build {
            Some(build) => Some(self.wait(&build.id).await),
            None => None,
        }
    }
}

async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let fake = Fake::default();
    let mut packages = serde_json::Map::new();
    fake.with(|m| {
        m.images.insert(BASE.into());
        m.image_ids.insert(BASE.into(), "b".repeat(64));
        m.image_labels.insert(
            BASE.into(),
            labels(json!({"io.wadspaces.abi": ABI, "io.wadspaces.extras": "1", "io.wadspaces.projects": "3",
                          "build_version": "WadSpaces base version:- 1"})),
        );
        m.free_gb = 100.0;
        m.secrets.insert("github_token".into(), TOKEN.as_bytes().to_vec());
        for (id, c, needs) in published() {
            let digest = format!("sha256:{}", c.to_string().repeat(8));
            packages.insert(
                id.into(),
                json!({"digest": digest, "imageId": c.to_string().repeat(64), "version": "1.0", "size": 1_000_000,
                       "needs": needs, "content": "x"}),
            );
            m.registry.insert(
                format!("{REGISTRY}/wadspaces-pkg-{id}@{digest}"),
                (
                    c.to_string().repeat(64),
                    labels(
                        json!({"io.wadspaces.pkg": id, "io.wadspaces.pkg.abi": ABI, "io.wadspaces.pkg.version": "1.0"}),
                    ),
                    1_000_000,
                ),
            );
        }
    });
    let index = json!({"version": 1, "registry": REGISTRY, "abis": {ABI: {"base": "sha256:b", "packages": packages}}});
    std::fs::write(d.path().join("index.json"), index.to_string()).unwrap();
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
    // Icons: nothing answers at the web apps' addresses, so each gets its name.
    let builds = Builds::with_icons(Arc::new(fake.clone()), bus.clone(), &state, 8, Icons::for_tests());
    let p = Packages::new(
        Arc::new(fake.clone()),
        bus.clone(),
        PkgSettings {
            pull: true,
            index_url: format!("file://{}", d.path().join("index.json").display()),
            drive_index: d.path().join("drive-index.json"),
            index_max_age: Duration::from_secs(3600),
            state_dir: state.clone(),
            min_free_gb: 8,
        },
    );
    builds.set_packages(p);
    Env { _d: d, state, fake, reg, builds }
}

/// IntelligenceQuest: VS Code with rust-analyzer, C++ and Rust on the
/// command line, a web app in focus mode, and its starry background.
fn iq() -> Value {
    json!({
        "version": 4, "id": "iq", "owner": "u1", "machine": "surface", "order": 1,
        "templates": [
            {"id": "dev", "name": "Dev", "apps": [
                {"app": "wadbrowser", "mode": "focus", "extras": [{"id": "notes", "name": "Notes", "url": "http://127.0.0.1:9/"}]},
                {"app": "vscode", "extras": [{"id": "rust-lang.rust-analyzer"}]},
                {"app": "terminal", "extras": [{"id": "cpp"}, {"id": "rust"}]}]}],
        "customization": {"name": "IntelligenceQuest", "wallpaper": {"type": "texture", "value": "stars"},
                          "projects": [], "secrets": {"bySlot": {}, "groups": []}}
    })
}

fn with(mut w: Value, f: impl FnOnce(&mut Value)) -> Value {
    f(&mut w);
    w
}

fn set_ref(w: &Value) -> String {
    wad_core::v4::image_ref(&wad_core::v4::image_key(w), BASE)
}

#[tokio::test]
async fn a_set_is_its_packages_stacked_on_the_base_then_put_together() {
    let e = env().await;
    let log = e.install_and_build(&iq()).await.expect("a build");
    assert_eq!(log.build.status, BuildStatus::Done, "{:?}", log.lines);
    let set = set_ref(&iq());
    let key = wad_core::v4::image_key(&iq());
    assert_eq!(log.build.ws_id, key);
    // One chain image per package, bottom first, then the set on the top one.
    let want = wad_core::v4::packages_of(&iq());
    assert_eq!(want, ["cpp", "rust", "electron-deps", "vscode", "ext-rust-lang.rust-analyzer"]);
    let built = e.built();
    assert_eq!(built.len(), want.len() + 1, "{built:?}");
    assert!(built[..want.len()].iter().all(|b| b.starts_with("localhost/wadspaces-chain:")), "{built:?}");
    assert_eq!(built.last(), Some(&set));
    // Each package downloaded with the machine's GitHub sign-in.
    let pulls = e.fake.0.lock().unwrap().pulls.clone();
    assert_eq!(pulls.len(), 5);
    assert!(pulls.iter().all(|(_, signed_in)| *signed_in));
    // The set: its layer, the base it's on, its packages (with their image ids).
    assert_eq!(e.label(&set, "io.wadspaces.layer").as_deref(), Some(key.as_str()));
    assert_eq!(e.label(&set, "io.wadspaces.from"), Some("b".repeat(64)));
    assert_eq!(e.label(&set, "io.wadspaces.assembly").as_deref(), Some("1"));
    let pkgs = e.label(&set, "io.wadspaces.pkgs").unwrap();
    assert!(pkgs.starts_with("cpp@111111111111,rust@222222222222,"), "{pkgs}");
    let last_build = e.fake.0.lock().unwrap().builds.last().unwrap().clone();
    let names = common::tar_names(&last_build.2);
    assert_eq!(names, ["Dockerfile"]);
    let file = String::from_utf8_lossy(&last_build.2).to_string();
    assert!(file.contains("RUN wadspaces-pkg assemble"), "{file}");
    assert_eq!(last_build.1, built[want.len() - 1], "the set is built FROM the top chain");
    // /v1/images lists it with its packages; the workspace runs on it.
    let rows = wadd::images::list(&e.fake).await.unwrap();
    let row = rows.iter().find(|r| r.layer == key).expect("the set's row");
    assert_eq!((row.image_ref.as_str(), row.pkgs.len()), (set.as_str(), 5));
    let ws = e.reg.workspace("iq").unwrap();
    assert_eq!(ws.image, set);
    assert_eq!(ws.name, "IntelligenceQuest");
    assert!(wad_core::v4::state(&iq(), &serde_json::to_value(&rows).unwrap().as_array().unwrap()[..]) == "built");
}

#[tokio::test]
async fn what_it_takes_at_start_is_in_its_extra_folder() {
    let e = env().await;
    sets::install(&e.here(), "iq", &iq(), None).await.unwrap();
    let webapps: Value = serde_json::from_slice(&std::fs::read(e.extra("iq", "webapps.json")).unwrap()).unwrap();
    assert_eq!(webapps, json!([{"id": "notes", "name": "Notes", "url": "http://127.0.0.1:9/"}]));
    assert!(e.extra("iq", "icons/notes.png").is_file());
    let layout = std::fs::read_to_string(e.extra("iq", "layout.json")).unwrap();
    assert!(layout.contains("wadspaces-webapp-notes.desktop"), "{layout}");
    assert!(std::fs::read_to_string(e.extra("iq", "wadbrowser.conf")).unwrap().contains("default = focus"));
    let wallpaper = std::fs::read(e.extra("iq", "wallpaper.png")).unwrap();
    assert!(wallpaper.starts_with(b"\x89PNG"));
    // The document stays for its build and the theme.
    assert_eq!(sets::installed_doc(&e.state, "iq").unwrap()["id"], "iq");
    // Without WadBrowser: no web apps, and the base's own wadbrowser.conf.
    let art = with(iq(), |w| w["templates"][0]["apps"] = json!([{"app": "vscode"}]));
    sets::install(&e.here(), "iq", &art, None).await.unwrap();
    assert_eq!(std::fs::read_to_string(e.extra("iq", "webapps.json")).unwrap().trim(), "[]");
    assert!(!e.extra("iq", "wadbrowser.conf").exists());
    assert!(!e.extra("iq", "icons/notes.png").exists(), "a web app taken out takes its icon");
}

#[tokio::test]
async fn the_theme_draws_its_background_and_web_app_icons_again() {
    let e = env().await;
    sets::install(&e.here(), "iq", &iq(), None).await.unwrap();
    let read = |f: &str| std::fs::read(e.extra("iq", f)).unwrap();
    let (dark_wall, dark_icon) = (read("wallpaper.png"), read("icons/notes.png"));
    assert_eq!(sets::theme(&e.state), Theme::Dark);
    assert_eq!(read("theme"), b"dark");
    sets::set_theme(&e.here(), Theme::Light).await.unwrap();
    assert_eq!(sets::theme(&e.state), Theme::Light);
    // The desktop names its icons in black on it.
    assert_eq!(read("theme"), b"light");
    assert_ne!(read("wallpaper.png"), dark_wall);
    assert_ne!(read("icons/notes.png"), dark_icon, "black on white, not acid on violet");
    // Installed again, it keeps the machine's theme.
    sets::install(&e.here(), "iq", &iq(), None).await.unwrap();
    assert_ne!(read("wallpaper.png"), dark_wall);
    sets::set_theme(&e.here(), Theme::Dark).await.unwrap();
    assert_eq!(read("wallpaper.png"), dark_wall);
}

#[tokio::test]
async fn a_second_wadspace_with_the_same_packages_builds_nothing() {
    let e = env().await;
    e.install_and_build(&iq()).await.unwrap();
    let n = e.built().len();
    // Other web apps, another name and background: the same packages.
    let twin = with(iq(), |w| {
        w["id"] = "twin".into();
        w["customization"]["name"] = "Twin".into();
        w["customization"]["wallpaper"] = json!({"type": "texture", "value": "marble"});
        w["templates"][0]["apps"][0]["extras"] = json!([]);
    });
    assert!(e.install_and_build(&twin).await.is_none(), "built already");
    assert_eq!(e.built().len(), n);
    assert_eq!(e.reg.workspace("twin").unwrap().image, set_ref(&iq()));
}

#[tokio::test]
async fn taking_a_package_out_restacks_only_whats_above_it() {
    let e = env().await;
    e.install_and_build(&iq()).await.unwrap();
    let n = e.built().len();
    // The extension, at the top: its chain is there, so only the set builds.
    let no_ext = with(iq(), |w| w["templates"][0]["apps"][1]["extras"] = json!([]));
    let log = e.install_and_build(&no_ext).await.unwrap();
    assert_eq!(log.build.status, BuildStatus::Done, "{:?}", log.lines);
    assert!(log.lines.iter().any(|l| l.contains("4 of them stacked already")), "{:?}", log.lines);
    assert_eq!(e.built()[n..], [set_ref(&no_ext)]);
    // rust, second from the bottom: cpp's chain stays under the rest.
    let n = e.built().len();
    let no_rust = with(iq(), |w| w["templates"][0]["apps"][2]["extras"] = json!([{"id": "cpp"}]));
    let log = e.install_and_build(&no_rust).await.unwrap();
    assert!(log.lines.iter().any(|l| l.contains("1 of them stacked already")), "{:?}", log.lines);
    let after = &e.built()[n..];
    assert_eq!(after.len(), 3 + 1, "electron-deps, vscode and the extension again, then the set: {after:?}");
    let pairs: Vec<(String, String)> = ["cpp", "electron-deps"]
        .iter()
        .map(|p| {
            let c = published().iter().find(|x| x.0 == *p).unwrap().1;
            (p.to_string(), pk::local_ref(p, &c.to_string().repeat(64)))
        })
        .collect();
    assert_eq!(after[0], pk::chain_ref(&"b".repeat(64), &pairs));
}

#[tokio::test]
async fn one_set_builds_once_however_many_ask() {
    let e = env().await;
    e.fake.with(|m| m.build_slow = true);
    sets::install(&e.here(), "iq", &iq(), None).await.unwrap();
    let twin = with(iq(), |w| w["id"] = "twin".into());
    sets::install(&e.here(), "twin", &twin, None).await.unwrap();
    let a = sets::build_installed(&e.here(), "iq").await.unwrap().build.unwrap();
    let b = sets::build_installed(&e.here(), "twin").await.unwrap().build.unwrap();
    assert_eq!(a.id, b.id);
    e.builds.cancel(&a.id).unwrap();
}

#[tokio::test]
async fn a_wadspace_with_no_packages_runs_on_the_base() {
    let e = env().await;
    let web = json!({"version": 4, "id": "web", "machine": "surface", "templates": [{"id": "w", "name": "Web", "apps": [
        {"app": "wadbrowser", "extras": [{"id": "notes", "name": "Notes", "url": "http://127.0.0.1:9/"}]}]}],
        "customization": {"name": "Web"}});
    let (_, b) = sets::install_and_build(&e.here(), "web", &web, None).await.unwrap();
    assert_eq!((b.image_key.as_str(), b.built, b.build.is_none()), ("base", true, true));
    assert_eq!(e.reg.workspace("web").unwrap().image, BASE);
    assert!(e.built().is_empty());
    // No background chosen: plain black.
    assert!(e.extra("web", "wallpaper.png").is_file());
}

#[tokio::test]
async fn an_old_base_another_machine_and_a_bad_wadspace_are_refused() {
    let e = env().await;
    sets::install(&e.here(), "iq", &iq(), None).await.unwrap();
    e.fake.with(|m| {
        m.image_labels.get_mut(BASE).unwrap().remove("io.wadspaces.extras");
    });
    let err = sets::build_installed(&e.here(), "iq").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("from before version 4"), "{}", err.message);
    let elsewhere = Here { machine: Some("laptop".into()), ..e.here() };
    let err = sets::install(&elsewhere, "iq", &iq(), None).await.err().unwrap();
    assert!(err.message.contains("another machine"), "{}", err.message);
    let bad = with(iq(), |w| w["customization"]["wallpaper"] = json!({"type": "image", "value": "/x.png"}));
    assert_eq!(sets::install(&e.here(), "iq", &bad, None).await.err().unwrap().code, ErrorCode::BadRequest);
    let err = sets::build_installed(&e.here(), "nope").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_tidy_up_and_the_disk_page_leave_packages_and_chains_alone() {
    let e = env().await;
    e.install_and_build(&iq()).await.unwrap();
    // A newer base: the old one stays while chains stand on it.
    e.fake.with(|m| {
        let labels = m.image_labels.get(BASE).cloned().unwrap();
        let old = "localhost/old-base".to_string();
        m.images.insert(old.clone());
        m.image_ids.insert(old.clone(), "b".repeat(64));
        m.image_labels.insert(old, labels.clone());
        m.image_ids.insert(BASE.into(), "f".repeat(64));
    });
    let before: Vec<String> = {
        let m = e.fake.0.lock().unwrap();
        let mut v: Vec<String> = m.images.iter().cloned().collect();
        v.sort();
        v
    };
    let freed = wadd::images::prune(&e.fake).await.unwrap();
    assert!(freed.removed.is_empty(), "{freed:?}");
    let after: Vec<String> = {
        let m = e.fake.0.lock().unwrap();
        let mut v: Vec<String> = m.images.iter().cloned().collect();
        v.sort();
        v
    };
    assert_eq!(before, after);
    // Picking a package by hand doesn't take it either.
    let pkg_id = "1".repeat(64);
    let copies = wadd::copies::Copies::new(
        &e.state,
        vec![],
        1000,
        Arc::new(|| ("local".into(), "t".into())),
        Arc::new(wadd::drives::Drives::new(1000, Arc::new(wadd::drives::System), false)),
        e.reg.clone(),
    );
    let pick = StorageDelete { images: vec![pkg_id.clone()], ..Default::default() };
    let freed = wadd::images::delete_selected(&e.reg, &e.builds, &copies, &e.fake, &e.state, &pick).await.unwrap();
    assert!(freed.removed.is_empty() && freed.failed.len() == 1, "{freed:?}");
    assert!(e.fake.0.lock().unwrap().images.iter().any(|i| i.starts_with("localhost/wadspaces-pkg-cpp:")));
    // The set's builds aren't old builds; a set's image is a layer the disk page counts.
    assert!(wadd::images::leftovers(&e.fake).await.unwrap().is_empty());
}

#[test]
fn layer_names_and_images() {
    use wadd::images::{layer_of, layer_ok, layer_ref};
    assert!(layer_ok("set:0123456789ab"));
    assert!(!layer_ok("set:xyz") && !layer_ok("set:0123456789abc"));
    assert_eq!(layer_ref("set:0123456789ab"), "localhost/wadspaces-set-0123456789ab:latest");
    assert_eq!(layer_of("localhost/wadspaces-set-0123456789ab:latest").as_deref(), Some("set:0123456789ab"));
    assert_eq!(layer_of("localhost/wadspaces-template-my-dev:latest").as_deref(), Some("template:my-dev"));
    assert_eq!(layer_of("localhost/wadspaces-custom-dev-plus:latest").as_deref(), Some("custom:dev-plus"));
    assert_eq!(layer_of(BASE), None);
    assert_eq!(layer_of("localhost/wadspaces-chain:0123456789ab"), None);
}
