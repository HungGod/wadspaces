//! Launches: a fake machine and fake drives, and real files for copies.
//! Nothing is mounted for a project: a launch checks the image and makes the
//! copies a wadspace takes in that aren't here yet; the wadspace clones and
//! copies the rest itself (the image's wadspaces-projects).

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::host::FakeHost;
use common::{Fake, ws};
use serde_json::{Value, json};
use wad_git::Update;
use wad_proto::ErrorCode;
use wad_proto::v1::{
    CopyFrom, Display, DriveRef, GitStatus, LaunchLog, LaunchStatus, PartState, Phase, ProjectKind, View as V,
    WorkspaceProject,
};
use wadd::copies::Copies;
use wadd::display::NullDisplay;
use wadd::drives::Drives;
use wadd::events::Bus;
use wadd::launches::{Launches, OLD_BASE};
use wadd::projects::{ProjectGit, Projects};
use wadd::registry::{Registry, Settings};
use wadd::view::View;

const IMAGE: &str = "localhost/wadspaces-a:latest";
const STICK: &str = "5E3F-1A2B";

/// wadd clones nothing now; the project service still wants a git.
struct NoGit;

#[async_trait]
impl ProjectGit for NoGit {
    async fn clone_repo(
        &self,
        _: &str,
        _: Option<&str>,
        _: &Path,
        _: Option<&str>,
        _: &mut (dyn for<'s> FnMut(&'s str, Option<f64>) + Send),
        _: u32,
    ) -> Result<(), String> {
        panic!("wadd doesn't clone projects")
    }
    async fn update(&self, _: &Path, _: Option<&str>, _: u32) -> Update {
        panic!("wadd doesn't fetch projects")
    }
    async fn status(&self, _: &Path, _: u32) -> Option<GitStatus> {
        None
    }
}

struct Env {
    d: tempfile::TempDir,
    tmp: PathBuf,
    fake: Fake,
    host: FakeHost,
    reg: Arc<Registry>,
    view: Arc<View>,
    copies: Arc<Copies>,
    launches: Arc<Launches>,
    bus: Bus,
}

impl Env {
    fn home(&self) -> PathBuf {
        self.tmp.join("home")
    }
}

fn github(id: &str, mount: &str) -> WorkspaceProject {
    WorkspaceProject {
        id: id.into(),
        kind: ProjectKind::Github,
        mount: mount.into(),
        repo: format!("o/{id}"),
        git_ref: Some("main".into()),
        setup: "npm install".into(),
        from: None,
    }
}

fn copy(id: &str, mount: &str, from: Option<CopyFrom>) -> WorkspaceProject {
    WorkspaceProject {
        id: id.into(),
        kind: ProjectKind::Copy,
        mount: mount.into(),
        repo: String::new(),
        git_ref: None,
        setup: String::new(),
        from,
    }
}

fn mount(id: &str, as_: &str, from: CopyFrom) -> WorkspaceProject {
    WorkspaceProject { kind: ProjectKind::Mount, ..copy(id, as_, Some(from)) }
}

fn mounts_in(e: &Env) {
    e.fake.with(|m| {
        m.image_labels.insert(IMAGE.into(), json!({"io.wadspaces.projects": "3"}).as_object().unwrap().clone());
    });
}

async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let tmp = wad_store::folders::realpath(d.path());
    let fake = Fake::default();
    let mut a = ws("a", IMAGE, Display::Stream, Some(3100));
    a.volumes = vec!["wad-a-config:/config:z".into()];
    let b = ws("b", IMAGE, Display::Stream, Some(3101));
    fake.with(|m| {
        m.images.insert(IMAGE.into());
        m.image_labels.insert(IMAGE.into(), json!({"io.wadspaces.projects": "2"}).as_object().unwrap().clone());
        m.answering.insert("http://127.0.0.1:3100/".into());
        m.answering.insert("http://127.0.0.1:3101/".into());
    });
    let bus = Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::User,
        started_at: 0,
        screen: None,
    });
    let state = tmp.join("state");
    let reg = Registry::new(
        Arc::new(fake.clone()),
        bus.clone(),
        Settings {
            ready_timeout: Duration::from_secs(10),
            projects_dir: tmp.join("projects").to_string_lossy().into(),
            signins_dir: tmp.join("signins"),
            state_dir: state.clone(),
            rootless_uid: Some(1000),
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    reg.load(vec![a, b, common::app("t", "/usr/bin/foot")]).await.unwrap();
    let view = View::new(reg.clone(), Arc::new(fake.clone()), Arc::new(NullDisplay), bus.clone(), state.clone());
    let home = tmp.join("home");
    std::fs::create_dir_all(home.join("Notes/drafts")).unwrap();
    std::fs::write(home.join("Notes/todo.md"), "- copy me").unwrap();
    std::fs::write(home.join("Notes/drafts/one.md"), "one").unwrap();
    std::fs::write(home.join("photo.jpg"), "jpg").unwrap();
    std::fs::create_dir_all(tmp.join("stick/Books")).unwrap();
    std::fs::write(tmp.join("stick/Books/book.epub"), "epub").unwrap();
    let host = FakeHost::new(Some(&tmp.join("stick").to_string_lossy()));
    let drives = Arc::new(Drives::new(1000, Arc::new(host.clone()), true));
    let machine: wadd::projects::MachineRef = Arc::new(|| ("local".to_string(), "Surface".to_string()));
    let copies = Arc::new(Copies::new(&state, vec![home.clone()], 1000, machine.clone(), drives.clone(), reg.clone()));
    {
        let found = copies.clone();
        reg.on_mounts(move |ws| {
            let copies = found.clone();
            Box::pin(async move { copies.find_mounts(&ws).await })
        });
        let (copies, state, r) = (copies.clone(), state.clone(), Arc::downgrade(&reg));
        reg.on_prepare(move |ws| {
            copies.prepare(ws)?;
            let here = wadd::copies::projects_here(ws, &r.upgrade().unwrap().mounts_of(&ws.id));
            wad_store::projects::write_manifest(&state, &ws.id, &here).map(drop).map_err(|e| e.to_string())
        });
    }
    let projects = Arc::new(Projects::new(
        &state,
        tmp.join("projects"),
        vec![home],
        1000,
        machine,
        drives,
        Arc::new(NoGit),
        reg.clone(),
        bus.clone(),
    ));
    let launches =
        Launches::new(reg.clone(), view.clone(), Arc::new(fake.clone()), projects, copies.clone(), bus.clone(), &state);
    Env { d, tmp, fake, host, reg, view, copies, launches, bus }
}

/// Workspace "a" with these projects (as an install gives them).
async fn with_projects(e: &Env, projects: Vec<WorkspaceProject>) {
    let mut a = e.reg.workspace("a").unwrap();
    a.projects = projects;
    e.reg.put_workspace(a).await.unwrap();
}

async fn wait(e: &Env, id: &str) -> LaunchLog {
    for _ in 0..500 {
        let l = e.launches.log(id, 0).unwrap();
        if l.launch.status.finished() {
            return l;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("launch stuck: {:?}", e.launches.log(id, 0).unwrap());
}

async fn launch(e: &Env, ws: &str, restart: bool) -> LaunchLog {
    let l = e.launches.create(ws, restart).unwrap();
    wait(e, &l.id).await
}

fn starts(e: &Env, unit: &str) -> usize {
    e.fake.calls().iter().filter(|c| **c == format!("start {unit}")).count()
}

fn unit(e: &Env, name: &str) -> String {
    e.fake.0.lock().unwrap().units.iter().find(|(n, _)| n == name).unwrap().1.clone()
}

#[tokio::test]
async fn a_background_launch_starts_it_without_switching() {
    let e = env().await;
    let l = e.launches.create_as("a", false, true).unwrap();
    let l = wait(&e, &l.id).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    assert_eq!(e.reg.state("a").unwrap().phase, Phase::Ready);
    assert_eq!(e.view.current(), V::Home);
    assert!(l.lines.iter().any(|x| x.contains("in the background")), "{:?}", l.lines);
}

#[tokio::test]
async fn an_app_launches_without_an_image() {
    let e = env().await;
    let l = launch(&e, "t", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    assert!(l.launch.parts.is_empty(), "{:?}", l.launch.parts);
    assert_eq!(e.fake.calls(), ["app wad-t.service /usr/bin/foot"]);
    assert_eq!(e.view.current(), V::Workspace("t".into()));
}

#[tokio::test]
async fn a_wadspace_with_projects_takes_them_in_itself() {
    let e = env().await;
    let notes = e.copies.make(&[e.home().join("Notes").to_string_lossy().into()]).await.unwrap();
    with_projects(&e, vec![github("vault", "Writing"), copy(&notes.id, "Notes", None)]).await;
    let (_, mut rx) = e.bus.subscribe();
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    assert_eq!(l.launch.progress, 1.0);
    // Nothing to make here: the copy is made, the repo is the wadspace's to clone.
    let parts: Vec<&str> = l.launch.parts.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(parts, ["image"]);
    assert!(l.lines[0].contains("with Writing, Notes"), "{:?}", l.lines);
    // What the wadspace reads at start.
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(e.tmp.join("state/extra/a/projects.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], 2);
    assert_eq!(
        manifest["projects"][0],
        json!({"id": "vault", "kind": "github", "mount": "Writing", "name": "Writing", "repo": "o/vault", "ref": "main",
               "setup": "npm install", "setupHash": wad_store::projects::setup_hash("npm install")})
    );
    assert_eq!(manifest["projects"][1], json!({"id": notes.id, "kind": "copy", "mount": "Notes", "name": "Notes"}));
    // Its unit mounts no project: only the copies to take in, read-only.
    let u = unit(&e, "wad-a.container");
    let state = e.tmp.join("state");
    assert!(u.contains(&format!("Volume={}/copies/vault:/run/wadspaces-copies/vault:ro,z", state.display())), "{u}");
    assert!(
        u.contains(&format!("Volume={0}/copies/{1}:/run/wadspaces-copies/{1}:ro,z", state.display(), notes.id)),
        "{u}"
    );
    assert!(!u.contains("/config/Desktop"), "{u}");
    assert!(state.join("copies/vault").is_dir(), "an empty folder until there's a copy");
    assert_eq!(e.view.current(), V::Workspace("a".into()));
    let mut done_event = false;
    while let Ok(ev) = rx.try_recv() {
        done_event |= matches!(ev, wad_proto::v1::Event::Launch(ref l) if l.launch.status == LaunchStatus::Done);
    }
    assert!(done_event);
}

#[tokio::test]
async fn copies_are_made_whole_inside_the_folder_roots() {
    let e = env().await;
    let home = e.home().to_string_lossy().into_owned();
    // One folder: its contents.
    let notes = e.copies.make(&[format!("{home}/Notes")]).await.unwrap();
    assert_eq!((notes.files, notes.bytes), (2, 12));
    let files = e.copies.path(&notes.id).join("files");
    assert_eq!(std::fs::read_to_string(files.join("drafts/one.md")).unwrap(), "one");
    assert!(e.copies.ready(&notes.id) && !e.tmp.join(format!("state/copies/{}.part", notes.id)).exists());
    // Several: each by its name.
    let both = e.copies.make(&[format!("{home}/photo.jpg"), format!("{home}/Notes")]).await.unwrap();
    let files = e.copies.path(&both.id).join("files");
    assert!(files.join("photo.jpg").is_file() && files.join("Notes/todo.md").is_file());
    assert_eq!(both.files, 3);
    // Outside the roots, or not there.
    for bad in ["/etc/passwd", &format!("{home}/nope"), &home] {
        assert_eq!(e.copies.make(&[bad.to_string()]).await.unwrap_err().code, ErrorCode::BadRequest, "{bad}");
    }
    assert_eq!(e.copies.make(&[]).await.unwrap_err().code, ErrorCode::BadRequest);
    // Deleted once nothing here has it.
    with_projects(&e, vec![copy(&notes.id, "Notes", None)]).await;
    assert_eq!(e.copies.delete(&notes.id).unwrap_err().code, ErrorCode::Conflict);
    with_projects(&e, vec![]).await;
    e.copies.delete(&notes.id).unwrap();
    assert!(!e.copies.path(&notes.id).exists());
    e.copies.delete(&notes.id).unwrap();
}

#[tokio::test]
async fn a_copy_from_before_is_made_where_its_files_are() {
    let e = env().await;
    let home = e.home().to_string_lossy().into_owned();
    let from = |m: &str| Some(CopyFrom { machine_id: m.into(), drive: None, paths: vec![format!("{home}/Notes")] });
    let stick = Some(CopyFrom {
        machine_id: String::new(),
        drive: Some(DriveRef { uuid: STICK.into(), label: "STICK".into(), fstype: "exfat".into() }),
        paths: vec!["Books".into()],
    });
    with_projects(
        &e,
        vec![copy("folder", "Notes", from("local")), copy("desk", "Desk", from("m2")), copy("books", "Books", stick)],
    )
    .await;
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    let parts: Vec<(&str, Option<&str>)> =
        l.launch.parts.iter().map(|p| (p.key.as_str(), p.message.as_deref())).collect();
    assert_eq!(
        parts,
        [
            ("image", Some("ready")),
            ("folder", Some("ready")),
            ("desk", Some("not this time")),
            ("books", Some("ready"))
        ]
    );
    assert!(e.copies.ready("folder") && e.copies.ready("books") && !e.copies.ready("desk"));
    assert_eq!(std::fs::read_to_string(e.copies.path("books").join("files/book.epub")).unwrap(), "epub");
    assert!(e.host.calls().iter().any(|c| c[0] == "systemd-mount"), "the stick was mounted to copy it");
    assert!(l.lines.iter().any(|x| x.starts_with("⚠ Desk: its files are on another machine")), "{:?}", l.lines);
    // Made: the next launch has nothing to make (the one elsewhere still waits).
    let again = launch(&e, "a", false).await;
    let keys: Vec<&str> = again.launch.parts.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["image", "desk"]);
    let _ = &e.d;
}

#[tokio::test]
async fn a_clone_wadd_kept_here_moves_into_the_wadspace() {
    let e = env().await;
    let clone = e.tmp.join("projects/vault");
    std::fs::create_dir_all(clone.join(".git")).unwrap();
    std::fs::write(clone.join("draft.md"), "uncommitted work").unwrap();
    with_projects(&e, vec![github("vault", "Writing")]).await;
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    assert_eq!(l.launch.parts[1].key, "vault");
    assert_eq!(std::fs::read_to_string(e.copies.path("vault").join("files/draft.md")).unwrap(), "uncommitted work");
    assert!(!clone.exists(), "moved, not copied");
    assert!(l.lines.iter().any(|x| x.contains("moves into the wadspace")), "{:?}", l.lines);
}

#[tokio::test]
async fn an_image_from_an_older_base_is_refused_for_projects() {
    let e = env().await;
    e.fake.with(|m| {
        m.image_labels.insert(IMAGE.into(), json!({"io.wadspaces.projects": "1"}).as_object().unwrap().clone());
    });
    with_projects(&e, vec![github("vault", "Writing")]).await;
    let l = launch(&e, "a", false).await;
    assert_eq!((l.launch.status, l.launch.error.as_deref()), (LaunchStatus::Error, Some(OLD_BASE)));
    assert_eq!(starts(&e, "wad-a.service"), 0);
    // Without projects that image is fine.
    with_projects(&e, vec![]).await;
    assert_eq!(launch(&e, "a", false).await.launch.status, LaunchStatus::Done);
}

#[tokio::test]
async fn a_mount_is_the_machines_own_folder_read_write() {
    let e = env().await;
    mounts_in(&e);
    let home = e.home().to_string_lossy().into_owned();
    let here = CopyFrom { machine_id: "local".into(), drive: None, paths: vec![format!("{home}/Notes")] };
    let away = CopyFrom { machine_id: "m2".into(), drive: None, paths: vec!["/var/home/wad/Desk".into()] };
    let stick = CopyFrom {
        machine_id: String::new(),
        drive: Some(DriveRef { uuid: STICK.into(), label: "STICK".into(), fstype: "exfat".into() }),
        paths: vec!["Books".into()],
    };
    with_projects(
        &e,
        vec![
            github("vault", "Writing"),
            mount("m1", "Notes", here),
            mount("m2", "Desk", away),
            mount("m3", "Books", stick),
        ],
    )
    .await;
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    let parts: Vec<&str> = l.launch.parts.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(parts, ["image"], "nothing to copy for a mount");
    for want in [
        format!("Notes: mounted from {home}/Notes; changes save to this machine"),
        "Desk is mounted only on the machine it's from, so it isn't in this one".into(),
        "Books: mounted from the drive STICK".into(),
    ] {
        assert!(l.lines.contains(&want), "{want} in {:?}", l.lines);
    }
    let u = unit(&e, "wad-a.container");
    assert!(u.contains(&format!("Volume={home}/Notes:/config/Desktop/Notes:rw\n")), "{u}");
    assert!(u.contains(&format!("Volume={}/stick/Books:/config/Desktop/Books:rw\n", e.tmp.display())), "{u}");
    assert!(!u.contains("Desktop/Desk:") && !u.contains("copies/m1"), "{u}");
    assert!(u.contains("SecurityLabelDisable=true"), "a mount is never relabelled: {u}");
    assert!(!e.tmp.join("state/copies/m1").exists(), "a mount has no copies folder");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(e.tmp.join("state/extra/a/projects.json")).unwrap()).unwrap();
    let listed: Vec<(&str, &str)> = manifest["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["mount"].as_str().unwrap(), p["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(listed, [("Writing", "github"), ("Notes", "mount"), ("Books", "mount")]);
}

#[tokio::test]
async fn a_mount_that_isnt_there_stops_the_start() {
    let e = env().await;
    mounts_in(&e);
    let home = e.home().to_string_lossy().into_owned();
    let gone = CopyFrom { machine_id: "local".into(), drive: None, paths: vec![format!("{home}/Gone")] };
    with_projects(&e, vec![mount("m1", "Gone", gone)]).await;
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Error, "{l:?}");
    assert!(l.launch.error.as_deref().unwrap_or("").contains("isn't there"), "{l:?}");
    assert_eq!(starts(&e, "wad-a.service"), 0, "never with an empty folder in its place");
    // Outside the folder roots: refused too.
    let etc = CopyFrom { machine_id: "local".into(), drive: None, paths: vec!["/etc".into()] };
    with_projects(&e, vec![mount("m1", "Etc", etc)]).await;
    let l = launch(&e, "a", false).await;
    assert!(l.launch.error.as_deref().unwrap_or("").contains("isn't inside"), "{l:?}");
}

#[tokio::test]
async fn a_mount_needs_a_base_that_takes_mounts() {
    let e = env().await;
    let home = e.home().to_string_lossy().into_owned();
    let here = CopyFrom { machine_id: "local".into(), drive: None, paths: vec![format!("{home}/Notes")] };
    with_projects(&e, vec![mount("m1", "Notes", here)]).await;
    let l = launch(&e, "a", false).await;
    assert_eq!((l.launch.status, l.launch.error.as_deref()), (LaunchStatus::Error, Some(OLD_BASE)));
    // A newer base still has GitHub projects and copies.
    mounts_in(&e);
    with_projects(&e, vec![github("vault", "Writing")]).await;
    assert_eq!(launch(&e, "a", false).await.launch.status, LaunchStatus::Done);
}

#[tokio::test]
async fn a_missing_local_image_says_build_it() {
    let e = env().await;
    e.fake.with(|m| m.images.clear());
    let l = launch(&e, "a", false).await;
    assert_eq!(l.launch.status, LaunchStatus::Error);
    assert!(l.launch.error.as_deref().unwrap().contains("build A in WadSpaces"), "{l:?}");
    assert_eq!(l.launch.parts[0].state, PartState::Error);
}

#[tokio::test]
async fn restart_restarts_it() {
    let e = env().await;
    e.view.switch("a").unwrap();
    e.reg.settled("a").await;
    let l = launch(&e, "a", true).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    let calls = e.fake.calls();
    let stop = calls.iter().position(|c| c == "stop wad-a.service").unwrap();
    let start = calls.iter().rposition(|c| c == "start wad-a.service").unwrap();
    assert!(stop < start, "{calls:?}");
}

#[tokio::test]
async fn bad_requests() {
    let e = env().await;
    assert_eq!(e.launches.create("nope", false).unwrap_err().code, ErrorCode::NotFound);
    let first = e.launches.create("a", false).unwrap();
    assert_eq!(e.launches.create("a", false).unwrap_err().code, ErrorCode::Conflict); // one at a time
    assert!(e.launches.create("b", false).is_ok()); // others may
    wait(&e, &first.id).await;
    assert_eq!(e.launches.log("nope", 0).unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(e.launches.list().len(), 2);
}

// ---------------------------------------------- runs, WadBrowser setups, the terminal

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn signins(e: &Env) -> Arc<wadd::signins::SignIns> {
    let s =
        wadd::signins::SignIns::new(&e.tmp.join("state"), e.tmp.join("signins"), 1000, e.reg.clone(), e.bus.clone());
    e.launches.set_signins(Arc::downgrade(&s));
    s
}

async fn item(e: &Env, it: wad_proto::v1::LaunchItem) -> LaunchLog {
    let l = e.launches.create_item(it).await.unwrap();
    wait(e, &l.id).await
}

#[tokio::test]
async fn a_throwaway_run_is_a_copy_that_goes_once_it_stops() {
    let e = env().await;
    // A native wadspace (a streamed one has a port of its own: it isn't run once).
    let mut n = ws("n", IMAGE, Display::Host, None);
    n.name = "N".into();
    n.volumes = vec!["wad-n-config:/config:z".into()];
    e.reg.put_workspace(n).await.unwrap();
    let streamed = e.launches.create_item(wad_proto::v1::LaunchItem::Run {
        from: "a".into(),
        overrides: Default::default(),
        background: true,
    });
    assert_eq!(streamed.await.unwrap_err().code, ErrorCode::BadRequest);
    let mut n = e.reg.workspace("n").unwrap();
    n.projects = vec![github("notes", "Notes")];
    e.reg.put_workspace(n).await.unwrap();
    let overrides = wad_proto::v1::RunOverrides {
        wallpaper: Some(wad_proto::v1::WallpaperUpload { ext: "png".into(), data: PNG.into() }),
        signins: vec![],
    };
    let l = item(&e, wad_proto::v1::LaunchItem::Run { from: "n".into(), overrides, background: true }).await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    let id = l.launch.ws_id.clone();
    assert!(id.starts_with("run-n-") && id.len() == "run-n-".len() + 6, "{id}");
    let run = e.reg.workspace(&id).unwrap();
    assert!(run.ephemeral);
    assert_eq!((run.name.as_str(), run.image.as_str()), ("N (run)", IMAGE));
    assert!(run.volumes.is_empty(), "no /config of its own: {:?}", run.volumes);
    // Its wadspace's projects, cloned afresh in it.
    assert_eq!(run.projects.iter().map(|p| p.mount.as_str()).collect::<Vec<_>>(), ["Notes"]);
    assert!(run.env.contains(&("WALLPAPER".into(), "/run/wadspaces-extra/wallpaper.png".into())));
    let extra = e.tmp.join("state/extra").join(&id);
    assert!(extra.join("wallpaper.png").is_file());
    // Its wadspace is untouched.
    assert!(e.reg.workspace("n").unwrap().volumes.contains(&"wad-n-config:/config:z".to_string()));
    // Stopped: gone, and its extra folder too.
    e.view.stop(&id).await.unwrap();
    e.reg.reconcile().await;
    for _ in 0..200 {
        if e.reg.workspace(&id).is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(e.reg.workspace(&id).is_err(), "the run is still listed");
    assert!(!extra.exists());
    // A run of a run, or of an app, isn't one.
    let refuse = e.launches.create_item(wad_proto::v1::LaunchItem::Run {
        from: "t".into(),
        overrides: Default::default(),
        background: true,
    });
    assert_eq!(refuse.await.unwrap_err().code, ErrorCode::BadRequest);
}

fn setup(name: &str, mode: &str, apps: &[(&str, &str)], signin: Option<(&str, &str)>) -> wad_proto::v1::BrowserSetup {
    wad_proto::v1::BrowserSetup {
        name: name.into(),
        mode: mode.into(),
        webapps: apps
            .iter()
            .map(|(id, url)| wad_proto::v1::SetupWebApp {
                id: (*id).into(),
                name: id.to_uppercase(),
                url: (*url).into(),
            })
            .collect(),
        signins: signin
            .map(|(app, id)| wad_proto::v1::WorkspaceSignIn {
                id: id.into(),
                app: app.into(),
                web: true,
                name: "Work-WadBrowser-Google".into(),
                kind: String::new(),
                slot: String::new(),
            })
            .into_iter()
            .collect(),
    }
}

#[tokio::test]
async fn wadbrowser_setups_run_as_instances_of_their_own() {
    let e = env().await;
    let si = signins(&e);
    let mail = setup(
        "Mail",
        "focus",
        &[("gmail", "https://mail.google.com"), ("drive", "https://drive.google.com")],
        Some(("gmail", "google-abc123")),
    );
    let l =
        item(&e, wad_proto::v1::LaunchItem::Browser { setup: mail, saved_id: Some("mail".into()), background: true })
            .await;
    assert_eq!(l.launch.status, LaunchStatus::Done, "{l:?}");
    let ws = e.reg.workspace("browser-mail").unwrap();
    let file = e.tmp.join("signins/.wadbrowser/browser-mail.json");
    assert_eq!(ws.exec, ["/usr/bin/wadbrowser", "--instance", "browser-mail", "--setup", file.to_str().unwrap()]);
    assert!(!ws.ephemeral, "a saved setup stays listed");
    // Its files: the setup, and its sign-ins at their folders here (made now).
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["signins"], json!({"gmail": "google-abc123"}));
    assert_eq!(written["mode"], "focus");
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(e.tmp.join("signins/.wadbrowser/browser-mail.signins.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["signins"][0]["dir"], e.tmp.join("signins/google-abc123").to_str().unwrap());
    assert_eq!(manifest["signins"][0]["apps"], json!(["gmail"]));
    assert!(e.tmp.join("signins/google-abc123").is_dir());
    let made = si.get("google-abc123").unwrap();
    assert_eq!((made.app.as_str(), made.name.as_str()), ("web", "Work-WadBrowser-Google"));
    assert_eq!(made.used_by, ["browser-mail"]);
    // A second, set up otherwise, beside it: a unit of its own, run once.
    let scratch = setup("", "browser", &[], None);
    let l = item(&e, wad_proto::v1::LaunchItem::Browser { setup: scratch, saved_id: None, background: true }).await;
    let run = e.reg.workspace(&l.launch.ws_id).unwrap();
    assert!(run.id.starts_with("browser-run-") && run.ephemeral, "{}", run.id);
    let units: Vec<String> = e.fake.0.lock().unwrap().apps.keys().cloned().collect();
    assert!(
        units.contains(&"wad-browser-mail.service".to_string()) && units.contains(&format!("wad-{}.service", run.id)),
        "{units:?}"
    );
    // Its last window closed: the app ends, and the run goes with its files.
    e.fake.with(|m| {
        m.apps.remove(&format!("wad-{}.service", run.id));
    });
    e.reg.reconcile().await;
    for _ in 0..200 {
        if e.reg.workspace(&run.id).is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(e.reg.workspace(&run.id).is_err());
    assert!(!e.tmp.join(format!("signins/.wadbrowser/{}.json", run.id)).exists());
    // A sign-in for a web app the setup hasn't is refused.
    let wrong = setup("X", "browser", &[("a", "https://a.example")], Some(("b", "google-abc123")));
    let r =
        e.launches.create_item(wad_proto::v1::LaunchItem::Browser { setup: wrong, saved_id: None, background: true });
    assert_eq!(r.await.unwrap_err().code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn the_terminal_and_older_requests_launch_and_sessions_take_apps() {
    let e = env().await;
    e.reg.put_workspace(common::app("terminal", "/usr/bin/foot")).await.unwrap();
    let l = item(&e, wad_proto::v1::LaunchItem::Terminal { background: true }).await;
    assert_eq!((l.launch.status, l.launch.ws_id.as_str()), (LaunchStatus::Done, "terminal"));
    // The older shape (no kind) is a workspace's launch (projects it names
    // are ignored: a wadspace has its own); the new one is tagged.
    let old: wad_proto::v1::LaunchBody =
        serde_json::from_value(json!({"workspace": "a", "projects": ["notes"]})).unwrap();
    assert!(matches!(old.item(), wad_proto::v1::LaunchItem::Workspace { workspace, .. } if workspace == "a"));
    let tagged: wad_proto::v1::LaunchBody = serde_json::from_value(json!({"kind": "terminal"})).unwrap();
    assert_eq!(tagged.item(), wad_proto::v1::LaunchItem::Terminal { background: false });
    let run: wad_proto::v1::LaunchBody =
        serde_json::from_value(json!({"kind": "run", "from": "a", "overrides": {"projects": []}})).unwrap();
    assert!(matches!(run.item(), wad_proto::v1::LaunchItem::Run { from, .. } if from == "a"));
    // A focus session over spaces and apps together.
    let s = e.view.session_start(&["a".into(), "terminal".into()], Some(25), false).unwrap();
    assert_eq!(s.workspaces, ["a", "terminal"]);
}
