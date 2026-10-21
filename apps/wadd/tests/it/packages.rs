//! Packages on a machine: what a build copies from comes from here, else
//! CI's registry (signed in with the machine's GitHub token), else an older
//! one here, else a build here; CI's index from the network, the last one
//! saved or the update drive's; what GET /v1/packages lists; and fetching
//! ahead of time as a job. A fake podman and a fake registry.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, TOKEN};
use serde_json::{Value, json};
use wad_proto::ErrorCode;
use wad_proto::v1::{BuildStatus, PackageFetchRequest, PackageHow, PackageState};
use wadd::events::Bus;
use wadd::packages::{Got, Packages, Settings, Watch};

const BASE: &str = "localhost/wadspaces-base:trixie";
const ABI: &str = "trixie.1";
const REGISTRY: &str = "ghcr.io/hunggod";

/// An image id: 64 of one hex digit.
fn iid(c: char) -> String {
    c.to_string().repeat(64)
}

fn labels(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// What CI published: electron-deps, vscode on it, and an extension on that.
fn published() -> Vec<(&'static str, char, u64, Vec<&'static str>)> {
    vec![
        ("electron-deps", 'e', 100_000_000, vec![]),
        ("vscode", 'c', 1_000_000_000, vec!["electron-deps"]),
        ("ext-rust-lang.rust-analyzer", 'a', 50_000_000, vec!["vscode", "electron-deps"]),
    ]
}

struct Env {
    d: tempfile::TempDir,
    fake: Fake,
    bus: Bus,
}

impl Env {
    fn index_path(&self) -> std::path::PathBuf {
        self.d.path().join("index.json")
    }

    fn settings(&self) -> Settings {
        Settings {
            pull: true,
            index_url: format!("file://{}", self.index_path().display()),
            index_max_age: Duration::from_secs(3600),
            state_dir: self.d.path().join("state"),
            min_free_gb: 8,
        }
    }

    fn packages(&self) -> Arc<Packages> {
        Packages::new(Arc::new(self.fake.clone()), self.bus.clone(), self.settings())
    }
}

fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let mut packages = serde_json::Map::new();
    fake.with(|m| {
        m.images.insert(BASE.into());
        m.image_labels.insert(BASE.into(), labels(json!({"io.wadspaces.abi": ABI})));
        m.free_gb = 100.0;
        m.secrets.insert("github_token".into(), TOKEN.as_bytes().to_vec());
        for (id, c, size, needs) in published() {
            let digest = format!("sha256:{}", c.to_string().repeat(8));
            packages.insert(
                id.into(),
                json!({"digest": digest, "imageId": iid(c), "version": "1.0", "size": size, "needs": needs, "content": "x"}),
            );
            m.registry.insert(
                format!("{REGISTRY}/wadspaces-pkg-{id}@{digest}"),
                (
                    iid(c),
                    labels(json!({"io.wadspaces.pkg": id, "io.wadspaces.pkg.abi": ABI, "io.wadspaces.pkg.version": "1.0"})),
                    size,
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
    Env { d, fake, bus }
}

/// What ensure said.
#[derive(Default)]
struct Said {
    lines: Vec<String>,
    bytes: Vec<(String, u64, u64)>,
}

impl Watch for Said {
    fn line(&mut self, line: &str) {
        self.lines.push(line.into());
    }
    fn bytes(&mut self, id: &str, done: u64, total: u64) {
        self.bytes.push((id.into(), done, total));
    }
}

fn hows(got: &[Got]) -> Vec<(String, PackageHow)> {
    got.iter().map(|g| (g.id.clone(), g.how)).collect()
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn whats_missing_is_downloaded_signed_in_and_named_by_its_image_id() {
    let e = env();
    let p = e.packages();
    let mut said = Said::default();
    let got = p.ensure(&ids(&["ext-rust-lang.rust-analyzer"]), BASE, &mut said).await.unwrap();
    // What it needs comes first: vscode, and electron-deps under that.
    assert_eq!(
        hows(&got),
        [
            ("electron-deps".into(), PackageHow::Fetched),
            ("vscode".into(), PackageHow::Fetched),
            ("ext-rust-lang.rust-analyzer".into(), PackageHow::Fetched)
        ]
    );
    assert_eq!(got[1].image, format!("localhost/wadspaces-pkg-vscode:{}", "c".repeat(12)));
    let pulls = e.fake.0.lock().unwrap().pulls.clone();
    assert_eq!(pulls[1], (format!("{REGISTRY}/wadspaces-pkg-vscode@sha256:cccccccc"), true), "by digest, signed in");
    assert!(e.fake.0.lock().unwrap().images.contains(&got[1].image));
    assert!(said.lines.contains(&"» vscode: downloading 1.0 GB".to_string()));
    assert!(said.bytes.contains(&("vscode".into(), 1_000_000_000, 1_000_000_000)));
    // Again: everything's here, nothing's downloaded.
    let mut again = Said::default();
    let got = p.ensure(&ids(&["ext-rust-lang.rust-analyzer"]), BASE, &mut again).await.unwrap();
    assert!(got.iter().all(|g| g.how == PackageHow::Here));
    assert_eq!(e.fake.0.lock().unwrap().pulls.len(), 3);
    assert!(again.lines.contains(&"» vscode: on this machine".to_string()));
}

#[tokio::test]
async fn a_package_ci_hasnt_published_is_built_here_on_what_it_needs() {
    let e = env();
    let p = e.packages();
    let mut said = Said::default();
    // rust needs cpp; CI published neither.
    let got = p.ensure(&ids(&["rust"]), BASE, &mut said).await.unwrap();
    assert_eq!(hows(&got), [("cpp".into(), PackageHow::Built), ("rust".into(), PackageHow::Built)]);
    let builds = e.fake.0.lock().unwrap().builds.clone();
    assert_eq!(builds.len(), 2);
    assert_eq!((builds[1].0.as_str(), builds[1].1.as_str()), ("localhost/wadspaces-pkg-rust:building", BASE));
    let file = String::from_utf8_lossy(&builds[1].2).into_owned();
    assert!(file.contains(&format!("COPY --from={} / /", got[0].image)), "rust's build copies cpp in: {file}");
    {
        let m = e.fake.0.lock().unwrap();
        assert!(!m.images.contains("localhost/wadspaces-pkg-rust:building"), "the build's own name goes");
        assert_eq!(m.image_labels[&got[1].image]["io.wadspaces.pkg.built-here"], "true");
    }
    assert!(said.lines.contains(&"» rust: building it here".to_string()));
    let list = p.list(BASE).await.unwrap();
    let rust = list.packages.iter().find(|r| r.id == "rust").unwrap();
    assert_eq!((rust.state, rust.image.as_deref()), (PackageState::Built, Some(got[1].image.as_str())));
}

#[tokio::test]
async fn without_a_sign_in_an_older_one_here_does_else_its_built() {
    let e = env();
    e.fake.with(|m| {
        m.secrets.remove("github_token");
        // vscode from an earlier CI run.
        let old = format!("localhost/wadspaces-pkg-vscode:{}", "0".repeat(12));
        m.images.insert(old.clone());
        m.image_ids.insert(old.clone(), iid('0'));
        m.image_labels.insert(old, labels(json!({"io.wadspaces.pkg": "vscode", "io.wadspaces.pkg.abi": ABI})));
    });
    let p = e.packages();
    let mut said = Said::default();
    let got = p.ensure(&ids(&["vscode"]), BASE, &mut said).await.unwrap();
    assert_eq!(hows(&got), [("electron-deps".into(), PackageHow::Built), ("vscode".into(), PackageHow::Here)]);
    assert_eq!(got[1].image, format!("localhost/wadspaces-pkg-vscode:{}", "0".repeat(12)));
    assert!(e.fake.0.lock().unwrap().pulls.is_empty());
    assert!(said.lines.iter().any(|l| l.contains("no GitHub sign-in")), "{:?}", said.lines);
    assert!(said.lines.contains(&"» vscode: on this machine (not CI's latest: a download gets that)".to_string()));
    let list = p.list(BASE).await.unwrap();
    let state = |id: &str| list.packages.iter().find(|r| r.id == id).unwrap().state;
    // Both are older than what CI published: electron-deps was built here.
    assert_eq!(
        (state("vscode"), state("electron-deps"), state("ext-rust-lang.rust-analyzer")),
        (PackageState::Older, PackageState::Older, PackageState::Missing)
    );
}

#[tokio::test]
async fn a_download_leaves_the_free_space_builds_keep() {
    let e = env();
    e.fake.with(|m| m.free_gb = 8.5);
    let p = e.packages();
    let mut said = Said::default();
    let got = p.ensure(&ids(&["vscode"]), BASE, &mut said).await.unwrap();
    // electron-deps (0.1 GB) fits; vscode (1 GB) doesn't, so it's built.
    assert_eq!(hows(&got), [("electron-deps".into(), PackageHow::Fetched), ("vscode".into(), PackageHow::Built)]);
    assert!(
        said.lines.iter().any(|l| l.starts_with("» vscode: couldn't download it: only 8.5 GB free")),
        "{:?}",
        said.lines
    );
}

#[tokio::test]
async fn a_base_from_before_packages_is_refused() {
    let e = env();
    e.fake.with(|m| {
        m.image_labels.insert(BASE.into(), serde_json::Map::new());
    });
    let p = e.packages();
    let err = p.ensure(&ids(&["vscode"]), BASE, &mut Said::default()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("update the base"), "{}", err.message);
    let list = p.list(BASE).await.unwrap();
    assert_eq!((list.abi, list.packages.len()), (None, 0));
    let err = p.ensure(&ids(&["no-such-thing"]), BASE, &mut Said::default()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn the_index_comes_from_the_network_else_the_last_one_saved() {
    let e = env();
    let list = e.packages().list(BASE).await.unwrap();
    assert_eq!((list.index_from.as_str(), list.abi.as_deref()), ("network", Some(ABI)));
    assert_eq!(
        list.packages.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["electron-deps", "vscode", "ext-rust-lang.rust-analyzer"]
    );
    assert!(list.packages.iter().all(|r| r.state == PackageState::Missing));
    assert_eq!(list.packages[1].size_mb, 1000.0);
    // The network's out: the copy it saved.
    std::fs::remove_file(e.index_path()).unwrap();
    let list = e.packages().list(BASE).await.unwrap();
    assert_eq!((list.index_from.as_str(), list.packages.len()), ("saved", 3));
    // Nothing saved either: none.
    std::fs::remove_dir_all(e.d.path().join("state")).unwrap();
    let list = e.packages().list(BASE).await.unwrap();
    assert_eq!((list.index_from.as_str(), list.index_at, list.packages.len()), ("none", None, 0));
}

/// Waits for the fetch to finish.
async fn fetched(p: &Packages) -> wad_proto::v1::PackageFetch {
    for _ in 0..200 {
        let f = p.list(BASE).await.unwrap().fetch.unwrap();
        if f.status.finished() {
            return f;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the fetch didn't finish");
}

#[tokio::test]
async fn a_fetch_gets_everything_published_and_says_how_each_came() {
    let e = env();
    let p = e.packages();
    let (_, mut events) = e.bus.subscribe();
    let job = p.fetch(PackageFetchRequest::default()).unwrap();
    assert_eq!(job.status, BuildStatus::Building);
    let f = fetched(&p).await;
    assert_eq!((f.status, f.error.as_deref()), (BuildStatus::Done, None));
    assert_eq!(f.ids, ["electron-deps", "vscode", "ext-rust-lang.rust-analyzer"]);
    assert_eq!(
        f.got.iter().map(|g| (g.id.as_str(), g.how)).collect::<Vec<_>>(),
        [
            ("electron-deps", PackageHow::Fetched),
            ("vscode", PackageHow::Fetched),
            ("ext-rust-lang.rust-analyzer", PackageHow::Fetched),
        ]
    );
    assert_eq!((f.done_bytes, f.total_bytes), (1_150_000_000, 1_150_000_000));
    assert!(matches!(events.try_recv(), Ok(wad_proto::v1::Event::Packages(_))), "packages events");
    let list = p.list(BASE).await.unwrap();
    assert!(list.packages.iter().all(|r| r.state == PackageState::Here));
}

#[tokio::test]
async fn one_fetch_at_a_time_and_one_can_be_stopped() {
    let e = env();
    e.fake.with(|m| m.build_slow = true);
    let p = e.packages();
    // cpp isn't published: it's built, slowly.
    p.fetch(PackageFetchRequest { ids: ids(&["cpp"]), base: None }).unwrap();
    let err = p.fetch(PackageFetchRequest { ids: ids(&["vscode"]), base: None }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(p.cancel_fetch());
    let f = p.list(BASE).await.unwrap().fetch.unwrap();
    assert_eq!(f.status, BuildStatus::Cancelled);
    assert!(!p.cancel_fetch(), "nothing left to stop");
    assert_eq!(
        p.fetch(PackageFetchRequest { ids: ids(&["fake-id"]), base: None }).unwrap_err().code,
        ErrorCode::BadRequest
    );
}
