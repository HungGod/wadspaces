//! The workspaces on this machine and their lives: bringing one up (its
//! image, its unit, until its desktop answers), stopping and restarting it,
//! and checking every few seconds that what wadd thinks matches what podman
//! says (reconcile).
//!
//! One thing at a time per workspace (its lock); images are never
//! downloaded (a workspace's image is a layer built here); run history is
//! opened and closed with the container.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use wad_proto::v1::{Display, Event, Kind, Phase, ProjectKind, Workspace, WorkspaceState};
use wad_proto::{ApiError, ErrorCode};
use wad_store::runs::RunLog;

use crate::backend::Backend;
use crate::events::Bus;

const RUNNING: &str = "running";
/// Under the sign-ins folder (the screen user's): WadBrowser setups' files.
pub const SETUPS_DIR: &str = ".wadbrowser";

pub struct Settings {
    pub ready_timeout: Duration,
    pub projects_dir: String,
    /// Saved sign-ins' folders (`<signins_dir>/<id>`).
    pub signins_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Rootless podman: the uid inside a container the host user maps to.
    pub rootless_uid: Option<u32>,
    /// Between readiness checks.
    pub poll: Duration,
}

pub struct Registry {
    backend: Arc<dyn Backend>,
    bus: Bus,
    settings: Settings,
    workspaces: Mutex<Vec<Workspace>>,
    states: Mutex<HashMap<String, WorkspaceState>>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    runs: Mutex<RunLog>,
    /// Native workspaces' windows (the display keeps this up to date).
    windows: Arc<Windows>,
    /// Told when a bring-up finishes (the view shows a pending switch).
    on_ready: Mutex<Option<ReadyHook>>,
    on_list: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Gets what a container mounts ready, just before it starts.
    on_prepare: Mutex<Option<PrepareHook>>,
    /// Finds a workspace's mounts on this machine, just before it starts.
    on_mounts: Mutex<Option<Arc<MountsHook>>>,
    /// Each workspace's mounts found here: project id → the folder or file.
    /// Its unit mounts these; a mount that isn't here is left out.
    mounts: Mutex<HashMap<String, Mounts>>,
    /// Ephemeral workspaces seen running: once one stops, it goes.
    ran: Mutex<std::collections::HashSet<String>>,
    /// What runs, in the order it started (Super+N and the switcher follow it).
    up_order: Mutex<Vec<String>>,
    /// Itself, for the tasks it starts.
    me: std::sync::Weak<Registry>,
}

type ReadyHook = Box<dyn Fn(&str) + Send + Sync>;
type PrepareHook = Box<dyn Fn(&Workspace) -> Result<(), String> + Send + Sync>;
/// A mount project's id → where it is on this machine.
pub type Mounts = std::collections::BTreeMap<String, String>;
type MountsHook = dyn Fn(Workspace) -> futures_util::future::BoxFuture<'static, Result<Mounts, String>> + Send + Sync;

/// Which native workspaces have a window on screen, and whether there's a
/// compositor to have one in at all.
#[derive(Default)]
pub struct Windows {
    available: std::sync::atomic::AtomicBool,
    present: tokio::sync::watch::Sender<std::collections::HashSet<String>>,
}

impl Windows {
    pub fn available(&self) -> bool {
        self.available.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_available(&self, yes: bool) {
        self.available.store(yes, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn has(&self, id: &str) -> bool {
        self.present.borrow().contains(id)
    }

    fn set(&self, id: &str, present: bool) {
        self.present.send_modify(|p| {
            if present {
                p.insert(id.into());
            } else {
                p.remove(id);
            }
        });
    }
}

fn wall() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn not_found(id: &str) -> ApiError {
    ApiError::new(ErrorCode::NotFound, format!("no workspace {id:?}"))
}

/// A workspace as wad-core's unit renderer takes it (wadd's YAML shape), its
/// sign-ins at their folders in `signins_dir` and its mounts where `mounts`
/// found them.
pub fn wadd_spec(ws: &Workspace, signins_dir: &std::path::Path, mounts: &Mounts) -> Value {
    let env: serde_json::Map<String, Value> =
        ws.env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    let projects: Vec<Value> = ws
        .projects
        .iter()
        .map(|p| match p.kind {
            ProjectKind::Mount => json!({"id": p.id, "kind": "mount", "mount": p.mount, "path": mounts.get(&p.id)}),
            _ => json!({"id": p.id, "mount": p.mount}),
        })
        .collect();
    let signins: Vec<Value> = ws
        .signins
        .iter()
        .map(|s| json!({"id": s.id, "app": s.app, "path": signins_dir.join(&s.id).to_string_lossy()}))
        .collect();
    json!({
        "id": ws.id, "name": ws.name, "image": ws.image,
        "display": if ws.display == Display::Host { "host" } else { "stream" },
        "port": ws.port, "container_name": ws.container_name, "container_port": ws.container_port,
        "env": env, "secrets": ws.secrets, "volumes": ws.volumes, "devices": ws.devices,
        "shm_size": ws.shm_size, "projects": projects, "signins": signins,
    })
}

pub fn unit_name(ws: &Workspace) -> String {
    format!("wad-{}.service", ws.id)
}

fn stream_url(ws: &Workspace) -> Option<String> {
    (ws.display == Display::Stream).then(|| ws.port.map(|p| format!("http://127.0.0.1:{p}/"))).flatten()
}

impl Registry {
    pub fn new(backend: Arc<dyn Backend>, bus: Bus, settings: Settings) -> Arc<Self> {
        let runs = RunLog::new(settings.state_dir.join("runs.jsonl"));
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            backend,
            bus,
            settings,
            workspaces: Mutex::default(),
            states: Mutex::default(),
            locks: Mutex::default(),
            tasks: Mutex::default(),
            runs: Mutex::new(runs),
            windows: Arc::default(),
            on_ready: Mutex::new(None),
            on_list: Mutex::new(None),
            on_prepare: Mutex::new(None),
            on_mounts: Mutex::new(None),
            mounts: Mutex::default(),
            ran: Mutex::default(),
            up_order: Mutex::default(),
        })
    }

    /// The unit wadd writes for a workspace.
    pub fn unit_text(&self, ws: &Workspace) -> String {
        wad_core::generator::quadlet(
            &wadd_spec(ws, &self.settings.signins_dir, &self.mounts_of(&ws.id)),
            &self.settings.state_dir.to_string_lossy(),
            self.settings.rootless_uid,
        )
    }

    /// The mounts found for a workspace when it last started.
    pub fn mounts_of(&self, id: &str) -> Mounts {
        self.mounts.lock().unwrap().get(id).cloned().unwrap_or_default()
    }

    /// The units for these workspaces: one per container (an app's unit is
    /// made when it starts).
    fn units(&self, list: &[Workspace]) -> Vec<(String, String)> {
        list.iter()
            .filter(|w| w.kind == Kind::Container)
            .map(|w| (format!("wad-{}.container", w.id), self.unit_text(w)))
            .collect()
    }

    /// An app's status, in a container's words: "running" while its unit
    /// is up, "exited" once it's gone.
    async fn app_status(&self, ws: &Workspace) -> String {
        match self.backend.unit_state(&unit_name(ws)).await.as_deref() {
            Ok("active" | "activating" | "reloading" | "deactivating") => RUNNING.into(),
            Ok(_) => "exited".into(),
            Err(_) => "unknown".into(),
        }
    }

    /// Takes on a list of workspaces: writes their units (and reloads systemd
    /// if any changed) and starts tracking their state.
    pub async fn load(&self, list: Vec<Workspace>) -> Result<(), String> {
        let units = self.units(&list);
        {
            let mut states = self.states.lock().unwrap();
            states.retain(|id, _| list.iter().any(|w| &w.id == id));
            for w in &list {
                states.entry(w.id.clone()).or_insert_with(|| WorkspaceState {
                    id: w.id.clone(),
                    container: "unknown".into(),
                    phase: Phase::Idle,
                    message: None,
                    error: None,
                    // An app has no image.
                    image_present: (w.kind == Kind::App).then_some(true),
                    since: wall(),
                });
            }
        }
        *self.workspaces.lock().unwrap() = list.clone();
        self.bus.set_states(self.states());
        self.bus.publish(Event::Workspaces(list));
        self.backend.install_units(units).await.map(drop)
    }

    /// A new workspace list (a build added or changed one): saved, units
    /// rewritten, and whoever follows the list told.
    /// The list is what counts: units that can't be written now (podman or
    /// systemd down) are logged, and starting the workspace says why.
    pub async fn set_workspaces(&self, list: Vec<Workspace>) -> Result<(), String> {
        self.save(&list)?;
        if let Err(e) = self.load(list).await {
            tracing::warn!("workspace units: {e}");
        }
        if let Some(f) = self.on_list.lock().unwrap().as_ref() {
            f();
        }
        Ok(())
    }

    /// Adds a workspace, or replaces the one with its id. The list is
    /// checked whole. Whether it's running with the old settings.
    pub async fn put_workspace(&self, ws: Workspace) -> Result<bool, ApiError> {
        let bad = |e: String| ApiError::new(ErrorCode::BadRequest, e);
        let mut list = self.workspaces();
        let old = list.iter().position(|w| w.id == ws.id);
        let changed = old.is_none_or(|i| list[i] != ws);
        match old {
            Some(i) => list[i] = ws.clone(),
            None => list.push(ws.clone()),
        }
        let checked =
            wad_store::workspaces::check_workspaces(list.iter().map(wad_store::workspaces::to_yaml).collect())
                .map_err(|e| bad(e.to_string()))?;
        let running = self.state(&ws.id).is_some_and(|s| s.container == RUNNING);
        self.set_workspaces(checked).await.map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
        Ok(old.is_some() && changed && running)
    }

    /// Removes a workspace from the list (stop it first).
    pub async fn remove_workspace(&self, id: &str) -> Result<(), ApiError> {
        let mut list = self.workspaces();
        let before = list.len();
        list.retain(|w| w.id != id);
        if list.len() == before {
            return Err(not_found(id));
        }
        self.set_workspaces(list).await.map_err(|e| ApiError::new(ErrorCode::Internal, e))
    }

    /// Calls `f` before a container starts, to get what it mounts ready (its
    /// sign-ins). An error stops the start.
    pub fn on_prepare(&self, f: impl Fn(&Workspace) -> Result<(), String> + Send + Sync + 'static) {
        *self.on_prepare.lock().unwrap() = Some(Box::new(f));
    }

    /// Calls `f` before a container starts, to find its mounts on this
    /// machine (project id → folder or file). An error stops the start.
    pub fn on_mounts(
        &self,
        f: impl Fn(Workspace) -> futures_util::future::BoxFuture<'static, Result<Mounts, String>> + Send + Sync + 'static,
    ) {
        *self.on_mounts.lock().unwrap() = Some(Arc::new(f));
    }

    /// Finds a workspace's mounts here and, when they changed, rewrites the
    /// units so its unit mounts them.
    async fn find_mounts(&self, ws: &Workspace) -> Result<(), String> {
        let hook = self.on_mounts.lock().unwrap().clone();
        let found = match hook {
            Some(f) if ws.projects.iter().any(|p| p.kind == ProjectKind::Mount) => f(ws.clone()).await?,
            _ => Mounts::new(),
        };
        let changed = {
            let mut all = self.mounts.lock().unwrap();
            let old = all.get(&ws.id);
            if old.is_none_or(|m| *m != found) && !(old.is_none() && found.is_empty()) {
                all.insert(ws.id.clone(), found);
                true
            } else {
                false
            }
        };
        if changed {
            self.backend.install_units(self.units(&self.workspaces())).await?;
        }
        Ok(())
    }

    /// Calls `f` whenever set_workspaces changes the list (hotkeys follow it).
    pub fn on_list_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_list.lock().unwrap() = Some(Box::new(f));
    }

    /// Its image is here now (a build made it).
    pub fn image_here(&self, id: &str) {
        if let Ok(ws) = self.workspace(id) {
            self.update(&ws, |st| st.image_present = Some(true));
        }
    }

    /// The workspace list (workspaces.json).
    fn save(&self, list: &[Workspace]) -> Result<(), String> {
        save_list(&self.settings.state_dir, list)
    }

    pub fn workspaces(&self) -> Vec<Workspace> {
        self.workspaces.lock().unwrap().clone()
    }

    pub fn states(&self) -> Vec<WorkspaceState> {
        let order = self.workspaces();
        let states = self.states.lock().unwrap();
        order.iter().filter_map(|w| states.get(&w.id).cloned()).collect()
    }

    pub fn state(&self, id: &str) -> Option<WorkspaceState> {
        self.states.lock().unwrap().get(id).cloned()
    }

    pub fn windows(&self) -> Arc<Windows> {
        self.windows.clone()
    }

    /// Calls `f` with a workspace's id each time one finishes coming up.
    pub fn on_ready(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.on_ready.lock().unwrap() = Some(Box::new(f));
    }

    /// An enabled workspace.
    pub fn workspace(&self, id: &str) -> Result<Workspace, ApiError> {
        self.workspaces.lock().unwrap().iter().find(|w| w.id == id && w.enabled).cloned().ok_or_else(|| not_found(id))
    }

    fn lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(id.into()).or_default().clone()
    }

    /// Changes a workspace's state: a new phase clears the error, and leaving
    /// the busy phases clears the message; a container that starts or stops
    /// opens or closes a run.
    fn update(&self, ws: &Workspace, f: impl FnOnce(&mut WorkspaceState)) {
        let state = {
            let mut states = self.states.lock().unwrap();
            let Some(st) = states.get_mut(&ws.id) else { return };
            let before = st.clone();
            f(st);
            if st.phase != before.phase {
                st.since = wall();
                if st.phase != Phase::Error {
                    st.error = None;
                }
                if !st.phase.busy() {
                    st.message = None;
                }
            }
            if st.container != before.container {
                self.track_run(ws, &st.container);
                self.note_ephemeral(ws, &st.container);
                self.note_order(ws, &st.container);
            }
            if *st == before {
                return;
            }
            st.clone()
        };
        self.bus.publish(Event::WorkspaceState(state));
    }

    /// Run history: a run opens when a container starts and closes when it
    /// stops ("unknown" means podman didn't answer: the run stays open).
    fn track_run(&self, ws: &Workspace, container: &str) {
        let mut runs = self.runs.lock().unwrap();
        let r = if container == RUNNING && !runs.is_open(&ws.id) {
            let mode = if ws.display == Display::Host { "local" } else { "stream" };
            let projects: Vec<String> = ws.projects.iter().map(|p| p.id.clone()).collect();
            runs.start(&ws.id, &ws.name, mode, &projects)
        } else if container != RUNNING && container != "unknown" && runs.is_open(&ws.id) {
            runs.end(&ws.id)
        } else {
            Ok(())
        };
        if let Err(e) = r {
            tracing::debug!("run history {}: {e}", ws.id);
        }
    }

    /// Keeps the start order: one that starts goes last, one that stops goes.
    fn note_order(&self, ws: &Workspace, container: &str) {
        let mut order = self.up_order.lock().unwrap();
        if container == RUNNING {
            if !order.contains(&ws.id) {
                order.push(ws.id.clone());
            }
        } else if container != "unknown" {
            order.retain(|id| id != &ws.id);
        }
    }

    /// The workspaces running, in the order they started.
    pub fn running_order(&self) -> Vec<String> {
        let order = self.up_order.lock().unwrap().clone();
        order.into_iter().filter(|id| self.state(id).is_some_and(|s| s.container == RUNNING)).collect()
    }

    /// A throwaway run (or a WadBrowser setup run once) that was up and has
    /// stopped is removed: its workspace, extra folder and setup files.
    fn note_ephemeral(&self, ws: &Workspace, container: &str) {
        if !ws.ephemeral {
            return;
        }
        if container == RUNNING {
            self.ran.lock().unwrap().insert(ws.id.clone());
            return;
        }
        if container == "unknown" || !self.ran.lock().unwrap().remove(&ws.id) {
            return;
        }
        if let Some(me) = self.me.upgrade() {
            let id = ws.id.clone();
            tokio::spawn(async move { me.forget_ephemeral(&id).await });
        }
    }

    /// Removes an ephemeral workspace and what it left on disk.
    pub async fn forget_ephemeral(&self, id: &str) {
        let mut list = self.workspaces();
        let before = list.len();
        list.retain(|w| !(w.id == id && w.ephemeral));
        if list.len() == before {
            return;
        }
        // Its files first: removing the workspace reloads systemd, and a
        // wadd stopped meanwhile would leave them behind for good.
        let _ = std::fs::remove_dir_all(self.settings.state_dir.join("extra").join(id));
        for f in [format!("{id}.json"), format!("{id}.signins.json")] {
            let _ = std::fs::remove_file(self.settings.signins_dir.join(SETUPS_DIR).join(f));
        }
        if let Err(e) = self.set_workspaces(list).await {
            tracing::warn!("removing the run {id}: {e}");
            return;
        }
        tracing::info!("{id}, run once, stopped: removed");
    }

    /// Ephemeral workspaces left from before (wadd restarted, the machine
    /// too) that aren't running: removed. Call after a reconcile.
    pub async fn sweep_ephemeral(&self) {
        let gone: Vec<String> = self
            .workspaces()
            .into_iter()
            .filter(|w| w.ephemeral)
            .filter(|w| self.state(&w.id).is_some_and(|s| s.container != RUNNING && s.container != "unknown"))
            .map(|w| w.id)
            .collect();
        for id in gone {
            self.forget_ephemeral(&id).await;
        }
    }

    /// The wadspaces from before the Builder's layers (a container whose
    /// image isn't a layer's: the old presets and designs), and then their
    /// images, removed. The wadspaces that replace them install on their
    /// layers when they're first launched; their volumes stay, so one with
    /// the same id keeps its home. Not one that's running, and nothing
    /// while podman doesn't answer.
    pub async fn sweep_old(&self) {
        let list = self.workspaces();
        let candidates: Vec<&Workspace> =
            list.iter().filter(|w| w.kind == Kind::Container && !is_layer_image(&w.image)).collect();
        if candidates.is_empty() || !self.backend.available().await {
            return; // podman not answering: tried again at the next start
        }
        let mut old = vec![];
        for w in candidates {
            // Podman's word, not the state's (that's "unknown" until reconcile).
            if matches!(self.backend.container(&w.container_name).await, Ok(s) if s != RUNNING) {
                old.push(w);
            }
        }
        if old.is_empty() {
            return;
        }
        let ids: Vec<String> = old.iter().map(|w| w.id.clone()).collect();
        let keep: Vec<Workspace> = list.iter().filter(|w| !ids.contains(&w.id)).cloned().collect();
        if let Err(e) = self.set_workspaces(keep.clone()).await {
            tracing::warn!("removing the wadspaces from before the layers: {e}");
            return;
        }
        tracing::info!("removed the wadspaces from before the layers: {}", ids.join(", "));
        let mut images: Vec<&str> = old.iter().map(|w| w.image.as_str()).filter(|i| !i.is_empty()).collect();
        images.sort_unstable();
        images.dedup();
        for image in images.into_iter().filter(|i| !keep.iter().any(|w| w.image == *i)) {
            match self.backend.remove_image(image).await {
                Ok(true) => tracing::info!("removed its image {image}"),
                Ok(false) => {}
                Err(e) => tracing::warn!("removing {image}: {e}"),
            }
        }
    }

    // ----------------------------------------------------------- bring up
    /// Brings a workspace up in the background (a no-op while it's already coming up).
    pub fn start(self: &Arc<Self>, id: &str) -> Result<(), ApiError> {
        let ws = self.workspace(id)?;
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.get(id).is_some_and(|t| !t.is_finished()) {
            return Ok(());
        }
        let me = self.clone();
        tasks.insert(id.into(), tokio::spawn(async move { me.bring_up(ws).await }));
        Ok(())
    }

    /// Waits for a workspace's bring-up (if one is running).
    pub async fn settled(&self, id: &str) {
        let task = self.tasks.lock().unwrap().remove(id);
        if let Some(t) = task {
            let _ = t.await;
        }
    }

    async fn bring_up(self: Arc<Self>, ws: Workspace) {
        let lock = self.lock_for(&ws.id);
        let _held = lock.lock().await;
        if let Err(e) = self.bring_up_locked(&ws).await {
            tracing::warn!("bringing up {} failed: {e}", ws.id);
            self.update(&ws, |st| {
                st.phase = Phase::Error;
                st.error = Some(e);
            });
            return;
        }
        drop(_held);
        if let Some(f) = self.on_ready.lock().unwrap().as_ref() {
            f(&ws.id);
        }
    }

    /// A native workspace's window came or went (from the display).
    pub fn window(&self, id: &str, present: bool) {
        self.windows.set(id, present);
        let Ok(ws) = self.workspace(id) else { return };
        let busy = self.lock_for(id).try_lock().is_err();
        self.update(&ws, |st| {
            if present && matches!(st.phase, Phase::Idle | Phase::Waiting) && !busy {
                st.phase = Phase::Ready;
                st.container = RUNNING.into();
            } else if !present && st.phase == Phase::Ready {
                st.phase = Phase::Waiting;
                st.message = Some("the desktop closed; waiting for it to come back".into());
            }
        });
    }

    /// An app's program ended: its window is as good as gone, though sway
    /// may say so a moment later. So a new start waits for a new window
    /// rather than taking the closing one for it.
    fn forget_window(&self, id: &str) {
        self.windows.set(id, false);
    }

    /// A native workspace is ready when its desktop's window appears.
    async fn wait_window(&self, ws: &Workspace) -> Result<(), String> {
        if !self.windows.available() {
            tracing::warn!("{} draws on the machine's screen, but there's no sway to show it in", ws.id);
            return Ok(());
        }
        let mut rx = self.windows.present.subscribe();
        let wait = rx.wait_for(|p| p.contains(&ws.id));
        match tokio::time::timeout(self.settings.ready_timeout, wait).await {
            Ok(_) => Ok(()),
            Err(_) => Err(format!(
                "{}'s desktop didn't open a window within {} s",
                ws.name,
                self.settings.ready_timeout.as_secs()
            )),
        }
    }

    async fn bring_up_locked(&self, ws: &Workspace) -> Result<(), String> {
        if ws.kind == Kind::App {
            return self.bring_up_app(ws).await;
        }
        self.check_image(ws).await?;
        if self.backend.container(&ws.container_name).await.unwrap_or_default() != RUNNING {
            // Its unit mounts the extra folder (projects, sign-ins, wallpaper).
            crate::images::ensure_extra(&self.settings.state_dir, &ws.id)
                .map_err(|e| format!("its extra folder: {e}"))?;
            self.find_mounts(ws).await?;
            if let Some(f) = self.on_prepare.lock().unwrap().as_ref() {
                f(ws)?;
            }
            self.update(ws, |st| {
                st.phase = Phase::Starting;
                st.message = Some("starting container".into());
            });
            self.backend.start(&unit_name(ws)).await?;
        }
        self.update(ws, |st| {
            st.phase = Phase::Waiting;
            st.container = RUNNING.into();
            st.message = Some("waiting for the desktop".into());
        });
        match stream_url(ws) {
            Some(url) => self.wait_http(ws, &url).await?,
            None if ws.display == Display::Host => self.wait_window(ws).await?,
            None => {}
        }
        self.update(ws, |st| st.phase = Phase::Ready);
        tracing::info!("{} ready", ws.id);
        Ok(())
    }

    /// An app: its transient unit started (unless it's up), then ready when
    /// its window shows.
    async fn bring_up_app(&self, ws: &Workspace) -> Result<(), String> {
        if self.app_status(ws).await != RUNNING {
            self.update(ws, |st| {
                st.phase = Phase::Starting;
                st.message = Some(format!("starting {}", ws.name));
            });
            self.backend.start_app(&unit_name(ws), ws).await?;
        }
        self.update(ws, |st| {
            st.phase = Phase::Waiting;
            st.container = RUNNING.into();
            st.message = Some("waiting for its window".into());
        });
        self.wait_window(ws).await?;
        self.update(ws, |st| st.phase = Phase::Ready);
        tracing::info!("{} ready", ws.id);
        Ok(())
    }

    async fn wait_http(&self, ws: &Workspace, url: &str) -> Result<(), String> {
        let deadline = Instant::now() + self.settings.ready_timeout;
        loop {
            if self.backend.http_ok(url).await {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("{} didn't answer within {} s", ws.name, self.settings.ready_timeout.as_secs()));
            }
            tokio::time::sleep(self.settings.poll).await;
        }
    }

    /// Makes sure the image is here: it's never downloaded. Callers hold
    /// the workspace's lock.
    async fn check_image(&self, ws: &Workspace) -> Result<(), String> {
        self.update(ws, |st| {
            st.phase = Phase::Starting;
            st.message = Some("checking image".into());
        });
        let present = self.backend.image_exists(&ws.image).await?;
        self.update(ws, |st| st.image_present = Some(present));
        if present {
            Ok(())
        } else {
            Err(format!("its image {} isn't on this machine: build it in WadSpaces first", ws.image))
        }
    }

    // ------------------------------------------------------- stop, restart
    pub async fn stop(self: &Arc<Self>, id: &str) -> Result<(), ApiError> {
        let ws = self.workspace(id)?;
        if let Some(t) = self.tasks.lock().unwrap().remove(id) {
            t.abort();
        }
        let lock = self.lock_for(id);
        let _held = lock.lock().await;
        self.update(&ws, |st| {
            st.phase = Phase::Stopping;
            st.message = Some("stopping".into());
        });
        // An app that already ended has no unit left to stop.
        let gone = ws.kind == Kind::App && self.app_status(&ws).await != RUNNING;
        let mut stopped = if gone { Ok(()) } else { self.backend.stop(&unit_name(&ws)).await };
        // It ended (and its unit went) between the look and the stop.
        if ws.kind == Kind::App && stopped.is_err() && self.app_status(&ws).await != RUNNING {
            stopped = Ok(());
        }
        if ws.kind == Kind::App {
            self.forget_window(&ws.id);
        }
        match stopped {
            Ok(()) => {
                let container = match ws.kind {
                    Kind::App => self.app_status(&ws).await,
                    Kind::Container => {
                        self.backend.container(&ws.container_name).await.unwrap_or_else(|_| "unknown".into())
                    }
                };
                self.update(&ws, |st| {
                    st.phase = Phase::Idle;
                    st.container = container;
                });
                Ok(())
            }
            Err(e) => {
                self.update(&ws, |st| {
                    st.phase = Phase::Error;
                    st.error = Some(e.clone());
                });
                Err(ApiError::new(ErrorCode::Internal, e))
            }
        }
    }

    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<(), ApiError> {
        let ws = self.workspace(id)?;
        {
            let lock = self.lock_for(id);
            let _held = lock.lock().await;
            self.update(&ws, |st| {
                st.phase = Phase::Starting;
                st.message = Some("restarting".into());
            });
            // An app's unit is gone once stopped, so it's stopped here and
            // made afresh by start below.
            let restarted = match ws.kind {
                Kind::App => {
                    let r = match self.app_status(&ws).await.as_str() {
                        RUNNING => self.backend.stop(&unit_name(&ws)).await,
                        _ => Ok(()),
                    };
                    self.forget_window(&ws.id);
                    r
                }
                Kind::Container => self.backend.restart(&unit_name(&ws)).await,
            };
            if let Err(e) = restarted {
                self.update(&ws, |st| {
                    st.phase = Phase::Error;
                    st.error = Some(e.clone());
                });
                return Err(ApiError::new(ErrorCode::Internal, e));
            }
            self.update(&ws, |st| st.phase = Phase::Idle);
        }
        self.start(id)
    }

    // ----------------------------------------------------------- reconcile
    /// What podman says now, against what wadd thinks (manager.py refresh):
    /// containers started or stopped elsewhere, images that came or went.
    pub async fn reconcile(&self) {
        let up = self.backend.available().await;
        for ws in self.workspaces().into_iter().filter(|w| w.enabled) {
            let lock = self.lock_for(&ws.id);
            let Ok(_held) = lock.try_lock() else { continue }; // busy: it knows
            if ws.kind == Kind::App {
                // Its unit says, podman or not.
                let container = self.app_status(&ws).await;
                self.update(&ws, |st| {
                    if container != RUNNING && matches!(st.phase, Phase::Ready | Phase::Waiting) {
                        st.phase = Phase::Idle;
                    }
                    st.container = container;
                });
                continue;
            }
            let container = if up {
                self.backend.container(&ws.container_name).await.unwrap_or_else(|_| "unknown".into())
            } else {
                "unknown".into()
            };
            let present = if up { self.backend.image_exists(&ws.image).await.ok() } else { None };
            let phase = self.state(&ws.id).map(|s| s.phase).unwrap_or(Phase::Idle);
            let ready = if container == RUNNING && phase == Phase::Idle {
                match stream_url(&ws) {
                    Some(url) => self.backend.http_ok(&url).await,
                    None => true,
                }
            } else {
                false
            };
            self.update(&ws, |st| {
                st.container = container.clone();
                st.image_present = present;
                // WAITING too: a native workspace whose window closed stops its own container.
                if container != RUNNING && matches!(st.phase, Phase::Ready | Phase::Waiting) {
                    st.phase = Phase::Idle;
                } else if ready {
                    st.phase = Phase::Ready;
                }
            });
        }
    }

    // ----------------------------------------------------------- autostart
    /// Once podman answers: starts the autostart workspaces, retrying the
    /// ones that fail every `retry`.
    pub async fn autostart(self: Arc<Self>, retry: Duration) {
        let mut pending: Vec<Workspace> = self.workspaces().into_iter().filter(|w| w.enabled && w.autostart).collect();
        if pending.is_empty() {
            return;
        }
        let mut delay = Duration::from_secs(2);
        while !self.backend.available().await {
            tracing::info!("autostart: waiting for podman");
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(60));
        }
        while !pending.is_empty() {
            let mut failed = vec![];
            for ws in pending {
                if self.workspace(&ws.id).is_err() {
                    continue; // removed or disabled meanwhile
                }
                let ok = self.start(&ws.id).is_ok() && {
                    self.settled(&ws.id).await;
                    self.state(&ws.id).is_some_and(|s| s.phase != Phase::Error)
                };
                if !ok {
                    failed.push(ws);
                }
            }
            pending = failed;
            if !pending.is_empty() {
                let ids: Vec<&str> = pending.iter().map(|w| w.id.as_str()).collect();
                tracing::info!("autostart: retrying {ids:?} in {} s", retry.as_secs());
                tokio::time::sleep(retry).await;
            }
        }
        tracing::info!("autostart: done");
    }

    /// Reconciles every `every`, for as long as wadd runs.
    pub fn spawn_reconcile(self: &Arc<Self>, every: Duration) -> JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                me.reconcile().await;
                tokio::time::sleep(every).await;
            }
        })
    }
}

/// Writes the workspace list (`<state_dir>/workspaces.json`).
pub fn save_list(state_dir: &std::path::Path, list: &[Workspace]) -> Result<(), String> {
    let path = state_dir.join("workspaces.json");
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())? + "\n";
    std::fs::create_dir_all(state_dir)
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A Builder layer's image (localhost/wadspaces-template-<id>, -custom-<id>):
/// what every wadspace runs on.
pub fn is_layer_image(image: &str) -> bool {
    image.starts_with("localhost/wadspaces-template-") || image.starts_with("localhost/wadspaces-custom-")
}
