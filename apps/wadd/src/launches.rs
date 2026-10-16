//! Launching a workspace: its image, and any copy it takes in that isn't
//! on this machine yet; then it's brought up.
//!
//! Step 1 does what may take a while, at once:
//!   - the image: it must be here (layer images are built, never
//!     downloaded), and, when the workspace has projects, from a base that
//!     clones and copies them in (labelled io.wadspaces.projects=2).
//!   - each project's copy that isn't here yet (copies.rs): a copy from
//!     before copies were made ahead (a folder or drive project's), made from
//!     where its files are when they're here; a GitHub project wadd cloned on
//!     this machine, moved in so the wadspace starts from it. Neither fails
//!     the launch: a copy that can't be made now is made at a later launch.
//!
//! Step 2 brings it up as a switch does. Projects aren't mounted: the
//! wadspace clones its GitHub projects and copies in the rest itself, in the
//! background, into its own /config (the image's wadspaces-projects).
//!
//! One launch per workspace at a time; different workspaces launch in
//! parallel. Progress goes out as `launch` events: the job, plus the log
//! lines since the last event.
//!
//! What's launched (`create_item`) is a workspace, or:
//!   - a throwaway run of a wadspace: a workspace `run-<from>-<6>` copied
//!     from it with the run's own wallpaper and sign-ins, no /config volume
//!     (its projects are cloned and copied afresh each time), and
//!     `ephemeral` (the registry removes it once it stops);
//!   - a WadBrowser setup: an app `browser-<saved id>` (or an ephemeral
//!     `browser-run-<6>`) running `wadbrowser --instance <id> --setup <file>`,
//!     its files with the sign-ins' folders (the screen user's), so it's a
//!     WadBrowser of its own beside any other;
//!   - the terminal (the machine's own app).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use wad_proto::v1::{
    BrowserSetup, Event, Kind, Launch, LaunchItem, LaunchLog, LaunchPart, LaunchStatus, PartState, Phase, ProjectKind,
    RunOverrides, Workspace, WorkspaceProject, WorkspaceSignIn,
};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::Backend;
use crate::copies::Copies;
use crate::events::Bus;
use crate::joblog::Lines;
use crate::projects::Projects;
use crate::registry::Registry;
use crate::view::View;

const MAX_LINES: usize = 2000;
const PUBLISH_EVERY: Duration = Duration::from_millis(250);
const PROJECTS_LABEL: &str = "io.wadspaces.projects";
/// Of the progress bar; starting the workspace is the rest.
const STEP1_SHARE: f64 = 0.9;
const KEEP_FINISHED: usize = 20;
/// The base's label value from which its wadspaces clone and copy in their
/// projects, and from which they take mounts.
const PROJECTS_IN: u32 = 2;
const MOUNTS_IN: u32 = 3;
pub const OLD_BASE: &str = "this image was built on an older base; rebuild its template to have projects";

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The named volume a workspace keeps /config in (wad-<id>-config), if any.
pub fn config_volume(volumes: &[String]) -> Option<String> {
    volumes.iter().find_map(|v| {
        let parts: Vec<&str> = v.split(':').collect();
        (parts.len() >= 2 && parts[1].trim_end_matches('/') == "/config" && !parts[0].contains('/'))
            .then(|| parts[0].to_string())
    })
}

struct Job {
    launch: Launch,
    log: Lines,
    /// Started, not switched to on the screen: opened in the background
    /// (one of several the UI opens at once).
    background: bool,
}

/// The machine's own terminal app.
pub const TERMINAL: &str = "terminal";
/// The machine's own WadBrowser app (its exec names the program).
pub const BROWSER: &str = "browser";

pub struct Launches {
    me: Weak<Launches>,
    signins: Mutex<Option<Weak<crate::signins::SignIns>>>,
    builds: Mutex<Option<Weak<crate::builds::Builds>>>,
    registry: Arc<Registry>,
    view: Arc<View>,
    backend: Arc<dyn Backend>,
    projects: Arc<Projects>,
    copies: Arc<Copies>,
    bus: Bus,
    log_dir: PathBuf,
    state_dir: PathBuf,
    jobs: Mutex<Vec<Job>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    /// Lines each job's events have carried so far.
    sent: Mutex<HashMap<String, u64>>,
    last_publish: Mutex<Option<Instant>>,
}

impl Launches {
    pub fn new(
        registry: Arc<Registry>,
        view: Arc<View>,
        backend: Arc<dyn Backend>,
        projects: Arc<Projects>,
        copies: Arc<Copies>,
        bus: Bus,
        state_dir: &Path,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            signins: Mutex::default(),
            builds: Mutex::default(),
            registry,
            view,
            backend,
            projects,
            copies,
            bus,
            log_dir: state_dir.join("launches"),
            state_dir: state_dir.into(),
            jobs: Mutex::default(),
            tasks: Mutex::default(),
            sent: Mutex::default(),
            last_publish: Mutex::default(),
        })
    }

    /// The saved sign-ins (a WadBrowser setup's are made when missing).
    pub fn set_signins(&self, s: Weak<crate::signins::SignIns>) {
        *self.signins.lock().unwrap() = Some(s);
    }

    /// The layer builds (a batch waits for the images it needs).
    pub fn set_builds(&self, b: Weak<crate::builds::Builds>) {
        *self.builds.lock().unwrap() = Some(b);
    }

    // ------------------------------------------------------------- jobs
    /// Launches what the launcher asked for (any LaunchItem).
    pub async fn create_item(&self, item: LaunchItem) -> Result<Launch, ApiError> {
        let background = match &item {
            LaunchItem::Workspace { background, .. }
            | LaunchItem::Run { background, .. }
            | LaunchItem::Browser { background, .. }
            | LaunchItem::Terminal { background } => *background,
        };
        let p = self.prepare_item(&item).await?;
        self.create_as(&p.workspace, p.restart, background)
    }

    /// What an item launches as: a workspace here (a run's and a setup's
    /// are made now).
    pub async fn prepare_item(&self, item: &LaunchItem) -> Result<Prepared, ApiError> {
        Ok(match item {
            LaunchItem::Workspace { workspace, restart, .. } => {
                self.registry.workspace(workspace)?;
                Prepared { workspace: workspace.clone(), restart: *restart }
            }
            LaunchItem::Terminal { .. } => {
                self.registry.workspace(TERMINAL)?;
                Prepared { workspace: TERMINAL.into(), restart: false }
            }
            LaunchItem::Run { from, overrides, .. } => {
                let id = self.make_run(from, overrides).await?;
                Prepared { workspace: id, restart: false }
            }
            LaunchItem::Browser { setup, saved_id, .. } => {
                let id = self.make_browser(setup, saved_id.as_deref()).await?;
                Prepared { workspace: id, restart: false }
            }
        })
    }

    /// The launcher's pick, together: each item made ready, a focus session
    /// over them when `minutes` says, then each launched (the first switched
    /// to, the rest in the background), once any layer image one needs has
    /// finished building. The workspaces' ids, at once; the launches follow
    /// as launch events.
    pub async fn batch(
        self: &Arc<Self>,
        items: Vec<LaunchItem>,
        minutes: Option<u32>,
    ) -> Result<Vec<String>, ApiError> {
        if items.is_empty() {
            return Err(bad("pick something to launch"));
        }
        let mut prepared = vec![];
        for it in &items {
            prepared.push(self.prepare_item(it).await?);
        }
        let ids: Vec<String> = prepared.iter().map(|p| p.workspace.clone()).collect();
        let me = self.clone();
        let session_ids = ids.clone();
        tokio::spawn(async move {
            me.wait_for_images(&session_ids).await;
            if minutes.is_some()
                && let Err(e) = me.view.session_start(&session_ids, minutes, false)
            {
                tracing::warn!("the focus session: {}", e.message);
            }
            for (i, p) in prepared.into_iter().enumerate() {
                if let Err(e) = me.create_as(&p.workspace, p.restart, i > 0) {
                    tracing::warn!("launching {}: {}", p.workspace, e.message);
                    me.bus.publish(Event::Notice { text: format!("{} didn't launch: {}", p.workspace, e.message) });
                }
            }
        });
        Ok(ids)
    }

    /// Waits while a layer image these workspaces run on is building here.
    async fn wait_for_images(&self, ids: &[String]) {
        let Some(builds) = self.builds.lock().unwrap().as_ref().and_then(Weak::upgrade) else { return };
        for id in ids {
            let Ok(ws) = self.registry.workspace(id) else { continue };
            let layer = layer_of(&ws.image);
            let Some(layer) = layer else { continue };
            for _ in 0..(3 * 3600) {
                if !builds.building(&layer) {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    /// A throwaway run's workspace, from its wadspace's: its id.
    async fn make_run(&self, from: &str, o: &RunOverrides) -> Result<String, ApiError> {
        let src = self.registry.workspace(from)?;
        if src.kind != Kind::Container {
            return Err(bad(format!("{} is an app: run it as it is", src.name)));
        }
        if src.ephemeral {
            return Err(bad(format!("{} is a run already", src.name)));
        }
        if src.display != wad_proto::v1::Display::Host {
            return Err(bad(format!("{} is streamed (from before the Builder): it can't be run once", src.name)));
        }
        let base = from.get(..from.len().min(48)).unwrap_or(from).trim_end_matches('-');
        let id = format!("run-{base}-{}", tail());
        let mut ws = src.clone();
        ws.id = id.clone();
        ws.name = format!("{} (run)", src.name);
        ws.container_name = format!("wad-{id}");
        // No /config of its own: nothing it does is kept.
        let config = config_volume(&src.volumes);
        ws.volumes.retain(|v| config.as_deref() != v.split(':').next());
        ws.signins = slot_signins(&o.signins)?;
        ws.ephemeral = true;
        ws.hotkey = None;
        ws.autostart = false;
        crate::images::ensure_extra(&self.state_dir, &id).map_err(|e| internal(format!("its extra folder: {e}")))?;
        match &o.wallpaper {
            Some(up) => {
                let bytes = crate::images::decode_wallpaper(up)?;
                crate::images::write_wallpaper(&self.state_dir, &id, &up.ext, &bytes).map_err(internal)?;
                set_env(&mut ws.env, "WALLPAPER", &format!("{}/wallpaper.{}", wad_core::layers::WALLPAPER_DIR, up.ext));
                set_env(&mut ws.env, "WALLPAPER_MODE", "fill");
            }
            None => {
                // Its wadspace's own runtime wallpaper comes along.
                for ext in ["png", "jpg"] {
                    let own = crate::images::extra_dir(&self.state_dir, from).join(format!("wallpaper.{ext}"));
                    if let Ok(bytes) = std::fs::read(&own) {
                        crate::images::write_wallpaper(&self.state_dir, &id, ext, &bytes).map_err(internal)?;
                    }
                }
            }
        }
        self.registry.put_workspace(ws).await?;
        Ok(id)
    }

    /// A WadBrowser setup's app workspace and files: its id.
    async fn make_browser(&self, setup: &BrowserSetup, saved: Option<&str>) -> Result<String, ApiError> {
        if !matches!(setup.mode.as_str(), "browser" | "focus") {
            return Err(bad("a setup's mode is browser or focus"));
        }
        let name = setup.name.trim();
        if name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(bad("a setup's name is at most 80 characters, on one line"));
        }
        let mut seen = vec![];
        for w in &setup.webapps {
            let url_ok = (w.url.starts_with("https://") || w.url.starts_with("http://"))
                && w.url.len() < 2048
                && !w.url.contains(char::is_control);
            if !wad_core::spec::is_id(&w.id) || seen.contains(&w.id) || !url_ok || w.name.contains(char::is_control) {
                return Err(bad(format!("{}: a web app's id, name and address", w.name)));
            }
            seen.push(w.id.clone());
        }
        let id = match saved {
            Some(s) if wad_core::spec::is_id(s) => format!("browser-{}", s.get(..s.len().min(54)).unwrap_or(s)),
            Some(s) => return Err(bad(format!("{s:?} isn't a setup id"))),
            None => format!("browser-run-{}", tail()),
        };
        let signins =
            self.signins.lock().unwrap().as_ref().and_then(Weak::upgrade).ok_or_else(|| internal("no sign-ins"))?;
        // Each web app's sign-in, made here (empty) if it's new to this machine.
        let mut ws_signins: Vec<WorkspaceSignIn> = vec![];
        let mut manifest: Vec<Value> = vec![];
        let mut by_app = serde_json::Map::new();
        for s in &setup.signins {
            if !seen.contains(&s.app) || ws_signins.iter().any(|x| x.app == s.app) {
                return Err(bad(format!("a sign-in for {:?}, which isn't one of the setup's web apps", s.app)));
            }
            let si = WorkspaceSignIn {
                web: true,
                kind: wad_core::signins::WEB.into(),
                slot: format!("web:{}", s.app),
                ..s.clone()
            };
            let (sname, dir) = signins.ensure(&si).map_err(internal)?;
            by_app.insert(s.app.clone(), s.id.clone().into());
            match manifest.iter_mut().find(|e| e["id"] == s.id.as_str()) {
                Some(e) => e["apps"].as_array_mut().expect("a list").push(s.app.clone().into()),
                None => manifest.push(serde_json::json!({
                    "id": s.id, "name": sname, "web": true, "apps": [s.app], "dir": dir.to_string_lossy(),
                })),
            }
            ws_signins.push(si);
        }
        let dir = signins.data_dir().join(crate::registry::SETUPS_DIR);
        let file = dir.join(format!("{id}.json"));
        let manifest_file = dir.join(format!("{id}.signins.json"));
        let setup_json = serde_json::json!({
            "mode": setup.mode, "webapps": setup.webapps, "signins": by_app, "ephemeral": saved.is_none(),
        });
        write_for_screen(&dir, &file, &setup_json, self.projects.uid).map_err(internal)?;
        write_for_screen(
            &dir,
            &manifest_file,
            &serde_json::json!({"version": 1, "signins": manifest}),
            self.projects.uid,
        )
        .map_err(internal)?;
        let own = self.registry.workspace(BROWSER).ok();
        let program =
            own.as_ref().and_then(|w| w.exec.first().cloned()).unwrap_or_else(|| "/usr/bin/wadbrowser".into());
        let ws = Workspace {
            id: id.clone(),
            name: if name.is_empty() { "WadBrowser".into() } else { name.into() },
            kind: Kind::App,
            exec: vec![program, "--instance".into(), id.clone(), "--setup".into(), file.to_string_lossy().into()],
            image: String::new(),
            display: wad_proto::v1::Display::Host,
            port: None,
            hotkey: None,
            icon: own.and_then(|w| w.icon),
            enabled: true,
            autostart: false,
            container_name: format!("wad-{id}"),
            container_port: 3000,
            env: vec![("WADBROWSER_SIGNINS".into(), manifest_file.to_string_lossy().into())],
            secrets: vec![],
            volumes: vec![],
            devices: vec![],
            shm_size: None,
            projects: vec![],
            signins: ws_signins,
            ephemeral: saved.is_none(),
            desktop: vec![],
        };
        self.registry.put_workspace(ws).await?;
        Ok(id)
    }

    /// Checks what can be checked up front, then starts.
    pub fn create(&self, ws_id: &str, restart: bool) -> Result<Launch, ApiError> {
        self.create_as(ws_id, restart, false)
    }

    /// `background`: started, not switched to.
    pub fn create_as(&self, ws_id: &str, restart: bool, background: bool) -> Result<Launch, ApiError> {
        let ws = self.registry.workspace(ws_id)?;
        let busy = |j: &Job| j.launch.ws_id == ws.id && !j.launch.status.finished();
        if self.jobs.lock().unwrap().iter().any(busy) {
            return Err(ApiError::new(ErrorCode::Conflict, format!("{} is already launching", ws.name)));
        }
        let app = ws.kind == wad_proto::v1::Kind::App;
        // An app has no image to get: it just starts.
        let mut parts = if app {
            vec![]
        } else {
            vec![LaunchPart {
                key: "image".into(),
                kind: "image".into(),
                name: ws.image.clone(),
                state: PartState::Waiting,
                progress: None,
                message: None,
            }]
        };
        for p in ws.projects.iter().filter(|p| self.to_stage(p).is_some()) {
            parts.push(LaunchPart {
                key: p.id.clone(),
                kind: "project".into(),
                name: p.mount.clone(),
                state: PartState::Waiting,
                progress: None,
                message: None,
            });
        }
        let id = wad_store::projects::new_id()[..12].to_lowercase();
        let launch = Launch {
            id: id.clone(),
            ws_id: ws.id.clone(),
            name: ws.name.clone(),
            restart,
            status: LaunchStatus::Queued,
            progress: 0.0,
            phase: "waiting to start".into(),
            error: None,
            parts,
            created: now(),
            started: None,
            finished: None,
            line_count: 0,
        };
        let log = Lines::new(&self.log_dir, &launch.id, MAX_LINES);
        self.jobs.lock().unwrap().push(Job { launch: launch.clone(), log, background });
        self.trim();
        self.publish(&id, true);
        let me = self.me.upgrade().expect("launches outlive their calls");
        let task_id = id.clone();
        self.tasks.lock().unwrap().insert(id, tokio::spawn(async move { me.run(task_id).await }));
        Ok(launch)
    }

    /// What a project needs made here before its wadspace starts, if
    /// anything: a copy's files, or a GitHub project's clone from when wadd
    /// kept them here.
    fn to_stage(&self, p: &WorkspaceProject) -> Option<Stage> {
        if self.copies.ready(&p.id) {
            return None;
        }
        match p.kind {
            ProjectKind::Copy => p.from.as_ref().map(|f| Stage::From(f.clone())),
            ProjectKind::Github => {
                let clone = self.projects.projects_dir.join(&p.id);
                clone.join(".git").exists().then_some(Stage::Clone(clone))
            }
            // Found when the wadspace starts (Copies::find_mounts).
            ProjectKind::Mount => None,
        }
    }

    /// Keeps the last KEEP_FINISHED finished jobs.
    fn trim(&self) {
        let mut jobs = self.jobs.lock().unwrap();
        let finished: Vec<String> =
            jobs.iter().filter(|j| j.launch.status.finished()).map(|j| j.launch.id.clone()).collect();
        let cut = finished.len().saturating_sub(KEEP_FINISHED);
        for id in &finished[..cut] {
            jobs.retain(|j| &j.launch.id != id);
            self.sent.lock().unwrap().remove(id);
        }
    }

    fn launch_of(&self, j: &Job) -> Launch {
        Launch { line_count: j.log.total(), ..j.launch.clone() }
    }

    pub fn list(&self) -> Vec<Launch> {
        let jobs = self.jobs.lock().unwrap();
        jobs.iter().rev().map(|j| self.launch_of(j)).collect()
    }

    pub fn log(&self, id: &str, since: u64) -> Result<LaunchLog, ApiError> {
        let jobs = self.jobs.lock().unwrap();
        let j = jobs.iter().find(|j| j.launch.id == id).ok_or_else(|| no_launch(id))?;
        Ok(LaunchLog { launch: self.launch_of(j), from: since, lines: j.log.since(since) })
    }

    pub fn cancel(&self, id: &str) -> Result<Launch, ApiError> {
        let running = {
            let jobs = self.jobs.lock().unwrap();
            let j = jobs.iter().find(|j| j.launch.id == id).ok_or_else(|| no_launch(id))?;
            !j.launch.status.finished()
        };
        if running {
            // Dropping the work stops a copy in progress (its .part is redone next time).
            if let Some(t) = self.tasks.lock().unwrap().remove(id) {
                t.abort();
            }
            self.with(id, |l| {
                l.status = LaunchStatus::Cancelled;
                l.finished = Some(now());
            });
            self.line(id, "✗ cancelled");
            self.publish(id, true);
        }
        self.log(id, 0).map(|l| l.launch)
    }

    // ------------------------------------------------------------ output
    fn with<T>(&self, id: &str, f: impl FnOnce(&mut Launch) -> T) -> Option<T> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.iter_mut().find(|j| j.launch.id == id).map(|j| f(&mut j.launch))
    }

    fn line(&self, id: &str, text: &str) {
        if let Some(j) = self.jobs.lock().unwrap().iter_mut().find(|j| j.launch.id == id) {
            j.log.push(text);
        }
        self.publish(id, false);
    }

    fn part(&self, id: &str, key: &str, state: Option<PartState>, progress: Option<f64>, message: Option<&str>) {
        self.with(id, |l| {
            if let Some(p) = l.parts.iter_mut().find(|p| p.key == key) {
                if let Some(s) = state {
                    p.state = s;
                }
                if progress.is_some() {
                    p.progress = progress;
                }
                if let Some(m) = message {
                    p.message = Some(m.into());
                }
            }
            let done: f64 =
                l.parts.iter().map(|p| p.progress.unwrap_or(if p.state == PartState::Done { 1.0 } else { 0.0 })).sum();
            l.progress = l.progress.max(STEP1_SHARE * done / l.parts.len() as f64);
        });
        self.publish(id, state.is_some());
    }

    /// Events for this job (and any other with lines not sent yet), at most
    /// every PUBLISH_EVERY unless `force`.
    fn publish(&self, id: &str, force: bool) {
        {
            let mut last = self.last_publish.lock().unwrap();
            let now = Instant::now();
            if !force && last.is_some_and(|t| now - t < PUBLISH_EVERY) {
                return;
            }
            *last = Some(now);
        }
        let out: Vec<LaunchLog> = {
            let jobs = self.jobs.lock().unwrap();
            let mut sent = self.sent.lock().unwrap();
            jobs.iter()
                .filter_map(|j| {
                    let from = sent.get(&j.launch.id).copied().unwrap_or(0);
                    if j.launch.id != id && from == j.log.total() {
                        return None;
                    }
                    sent.insert(j.launch.id.clone(), j.log.total());
                    Some(LaunchLog { launch: self.launch_of(j), from, lines: j.log.since(from) })
                })
                .collect()
        };
        for l in out {
            self.bus.publish(Event::Launch(l));
        }
    }

    // ----------------------------------------------------------- running
    async fn run(self: Arc<Self>, id: String) {
        let r = self.go(&id).await;
        match r {
            Ok(name) => {
                self.with(&id, |l| {
                    l.progress = 1.0;
                    l.status = LaunchStatus::Done;
                    l.phase = "running".into();
                });
                self.line(&id, &format!("✓ {name} is up"));
            }
            Err(e) => {
                tracing::warn!("launch {id} failed: {e}");
                self.with(&id, |l| {
                    l.status = LaunchStatus::Error;
                    l.error = Some(e.clone());
                });
                self.line(&id, &format!("✗ {e}"));
            }
        }
        self.with(&id, |l| l.finished = Some(now()));
        self.tasks.lock().unwrap().remove(&id);
        self.publish(&id, true);
    }

    /// The whole launch; its workspace's name when it's up.
    async fn go(&self, id: &str) -> Result<String, String> {
        let ws_id = self
            .with(id, |l| {
                l.status = LaunchStatus::Running;
                l.started = Some(now());
                l.phase = "getting it ready".into();
                l.ws_id.clone()
            })
            .ok_or("the launch is gone")?;
        let ws = self.registry.workspace(&ws_id).map_err(|e| e.message)?;
        let with = if ws.projects.is_empty() {
            String::new()
        } else {
            format!(" with {}", ws.projects.iter().map(|p| p.mount.as_str()).collect::<Vec<_>>().join(", "))
        };
        self.line(id, &format!("» launching {}{with}", ws.name));
        for p in ws.projects.iter().filter(|p| p.kind == ProjectKind::Mount) {
            let Some(from) = &p.from else { continue };
            let line = match (&from.drive, from.paths.first()) {
                (Some(d), _) => {
                    format!(
                        "{}: mounted from the drive {}",
                        p.mount,
                        if d.label.is_empty() { &d.uuid } else { &d.label }
                    )
                }
                (None, Some(path)) if self.copies.is_here(&from.machine_id) => {
                    format!("{}: mounted from {path}; changes save to this machine", p.mount)
                }
                _ => format!("{} is mounted only on the machine it's from, so it isn't in this one", p.mount),
            };
            self.line(id, &line);
        }
        let mut step1: Vec<BoxFuture<'_, Result<(), String>>> = vec![];
        if ws.kind == wad_proto::v1::Kind::Container {
            step1.push(self.image(id, &ws).boxed());
        }
        for p in &ws.projects {
            if let Some(stage) = self.to_stage(p) {
                step1.push(self.stage(id, p, stage).map(Ok).boxed());
            }
        }
        futures_util::future::try_join_all(step1).await?;
        self.with(id, |l| {
            l.progress = l.progress.max(STEP1_SHARE);
            l.phase = "starting".into();
        });
        self.publish(id, true);
        self.start(id, &ws).await?;
        Ok(ws.name)
    }

    async fn image(&self, id: &str, ws: &Workspace) -> Result<(), String> {
        self.part(id, "image", Some(PartState::Working), None, Some("checking the image"));
        let r = async {
            let Some(labels) = self.backend.image_labels(&ws.image).await? else {
                return Err(format!("{} isn't on this machine; build {} in WadSpaces first", ws.image, ws.name));
            };
            let level = labels.get(PROJECTS_LABEL).and_then(Value::as_str).and_then(|v| v.parse::<u32>().ok());
            let needs = if ws.projects.iter().any(|p| p.kind == ProjectKind::Mount) {
                MOUNTS_IN
            } else if ws.projects.is_empty() {
                0
            } else {
                PROJECTS_IN
            };
            if needs > 0 && level.is_none_or(|l| l < needs) {
                return Err(OLD_BASE.to_string());
            }
            Ok(())
        }
        .await;
        match &r {
            Ok(()) => self.part(id, "image", Some(PartState::Done), Some(1.0), Some("ready")),
            Err(e) => self.part(id, "image", Some(PartState::Error), None, Some(e)),
        }
        r
    }

    /// Makes what a project needs here. Not making it doesn't stop the
    /// launch: the wadspace starts without it, and a later launch tries again.
    async fn stage(&self, id: &str, p: &WorkspaceProject, stage: Stage) {
        let mount = &p.mount;
        let r = match stage {
            Stage::From(from) => {
                self.part(id, &p.id, Some(PartState::Working), None, Some("copying"));
                self.copies
                    .stage_from(&p.id, &from)
                    .await
                    .map(|m| format!("{mount}: copied {} files ({} MB) to take in", m.files, m.bytes >> 20))
            }
            Stage::Clone(clone) => {
                self.part(id, &p.id, Some(PartState::Working), None, Some("moving its clone"));
                self.copies.seed_from_clone(&p.id, &clone).await.map(|_| {
                    format!("{mount}: the clone kept on this machine ({}) moves into the wadspace", clone.display())
                })
            }
        };
        match r {
            Ok(line) => {
                self.line(id, &line);
                self.part(id, &p.id, Some(PartState::Done), Some(1.0), Some("ready"));
            }
            Err(e) => {
                self.line(id, &format!("⚠ {mount}: {e}"));
                self.part(id, &p.id, Some(PartState::Done), Some(1.0), Some("not this time"));
            }
        }
    }

    async fn start(&self, id: &str, ws: &Workspace) -> Result<(), String> {
        let running = self.registry.state(&ws.id).is_some_and(|s| s.container == "running");
        if running && self.with(id, |l| l.restart).unwrap_or(false) {
            self.line(id, &format!("restarting {}", ws.name));
            self.view.stop(&ws.id).await.map_err(|e| e.message)?;
        }
        let background = self.jobs.lock().unwrap().iter().any(|j| j.launch.id == id && j.background);
        if background {
            self.line(id, &format!("starting {} in the background", ws.name));
            self.registry.start(&ws.id).map_err(|e| e.message)?;
        } else {
            self.line(id, &format!("starting {}", ws.name));
            self.view.switch(&ws.id).map_err(|e| e.message)?;
        }
        self.registry.settled(&ws.id).await;
        match self.registry.state(&ws.id) {
            Some(st) if st.phase == Phase::Error => {
                Err(st.error.unwrap_or_else(|| format!("{} didn't start", ws.name)))
            }
            _ => Ok(()),
        }
    }
}

/// What a project needs made here before its wadspace starts.
enum Stage {
    /// A copy, from where its files are.
    From(wad_proto::v1::CopyFrom),
    /// A GitHub project's clone wadd kept here.
    Clone(PathBuf),
}

/// A launch item made ready: the workspace it launches.
pub struct Prepared {
    pub workspace: String,
    pub restart: bool,
}

/// The layer an image belongs to ("template:<id>" / "custom:<id>"), if it's one.
fn layer_of(image: &str) -> Option<String> {
    let rest = image.strip_prefix("localhost/wadspaces-")?.strip_suffix(":latest")?;
    if let Some(id) = rest.strip_prefix("template-") {
        return Some(format!("template:{id}"));
    }
    rest.strip_prefix("custom-").map(|id| format!("custom:{id}"))
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, e.to_string())
}

/// Six lowercase letters and digits, for a run's id.
fn tail() -> String {
    wad_store::projects::new_id().to_lowercase().chars().filter(char::is_ascii_alphanumeric).take(6).collect()
}

/// A run's sign-ins, by slot: each slot at most once, with its kind.
fn slot_signins(list: &[WorkspaceSignIn]) -> Result<Vec<WorkspaceSignIn>, ApiError> {
    let mut out: Vec<WorkspaceSignIn> = vec![];
    for s in list {
        let key = if s.slot.is_empty() { &s.app } else { &s.slot };
        if !wad_core::spec::is_id(&s.id)
            || out.iter().any(|x| (if x.slot.is_empty() { &x.app } else { &x.slot }) == key)
        {
            return Err(bad(format!("sign-in {:?} for {key}", s.id)));
        }
        out.push(s.clone());
    }
    Ok(out)
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    match env.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = value.into(),
        None => env.push((key.into(), value.into())),
    }
}

/// A WadBrowser setup's file, where the screen's user (who runs it) can read
/// it: beside the sign-ins' folders, theirs too.
fn write_for_screen(dir: &Path, file: &Path, v: &Value, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).expect("json") + "\n")?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    if nix::unistd::geteuid().is_root() {
        nix::unistd::chown(dir, Some(uid.into()), Some(uid.into())).map_err(std::io::Error::from)?;
        nix::unistd::chown(&tmp, Some(uid.into()), Some(uid.into())).map_err(std::io::Error::from)?;
    }
    std::fs::rename(&tmp, file)
}

fn no_launch(id: &str) -> ApiError {
    ApiError::new(ErrorCode::NotFound, format!("no launch {id:?}"))
}
