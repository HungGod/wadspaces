//! wadd, the WadSpaces machine daemon, in Rust (stage 2 of the plan). It
//! serves `/v1` on a Unix socket to the callers its policy allows, and runs
//! the machine's workspaces (registry.rs); the rest (projects, the account,
//! ...) arrives milestone by milestone.

pub mod access;
pub mod api;
pub mod backend;
pub mod bluetooth;
pub mod builds;
pub mod cloud;
pub mod copies;
pub mod devices;
pub mod diagnostics;
pub mod display;
pub mod drives;
pub mod events;
pub mod files;
pub mod github;
pub mod icons;
pub mod images;
pub mod joblog;
pub mod launches;
pub mod logbuf;
pub mod metrics;
pub mod network;
pub mod packages;
pub mod projects;
pub mod proxy;
pub mod registry;
pub mod remote;
pub mod screen;
pub mod secrets;
pub mod sets;
pub mod signins;
pub mod storage;
pub mod streams;
pub mod systemd;
pub mod tailnet;
pub mod tidy;
pub mod tunnel;
pub mod view;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::UnixListener;
use wad_config::{Config, Profile};
use wad_proto::v1::{self, MachineInfo, Workspace};

use crate::access::{Peer, Policy};
use crate::api::AppState;
use crate::backend::Backend;
use crate::display::{Display, NullDisplay, SwayDisplay, WindowSink};
use crate::events::Bus;
use crate::logbuf::LogBuffer;
use crate::registry::{Registry, Settings};
use crate::view::View;

/// Where the API listens.
pub enum Listen {
    /// The socket systemd handed over (wadd.socket).
    Activated(std::os::unix::net::UnixListener),
    /// A path to bind (and remove on exit).
    Path(PathBuf),
}

pub struct Server {
    pub listener: UnixListener,
    /// The path we bound, to clean up; None when systemd owns the socket.
    pub bound: Option<PathBuf>,
    pub state: Arc<AppState>,
    /// The workspaces to take on when serving starts.
    workspaces: Vec<Workspace>,
    reconcile_every: Duration,
    sway: Option<Arc<SwayDisplay>>,
    keys: wad_config::Keys,
    /// Where the streaming proxy listens (the tunnel's origin).
    proxy_listen: Vec<String>,
}

/// Placeholders for the secrets the workspaces name that aren't here, so
/// their containers can start.
async fn placeholders(app: Arc<AppState>) {
    let mut needed: Vec<String> =
        app.registry.workspaces().into_iter().filter(|w| w.enabled).flat_map(|w| w.secrets).collect();
    needed.sort();
    needed.dedup();
    if let Err(e) = app.secrets.ensure(&needed).await {
        tracing::debug!("placeholder secrets: {e}");
    }
}

/// The relay's settings (`[cloud]`). None: no account to link to.
fn cloud_settings(config: &Config) -> Option<(cloud::Settings, cloud::Endpoints)> {
    let c = &config.cloud;
    if !c.enabled {
        return None;
    }
    if c.project_id.is_empty() {
        return None;
    }
    let (project, region, api_key) = (c.project_id.clone(), c.functions_region.clone(), c.api_key.clone());
    let http = reqwest::Client::new();
    let ends = match &c.emulator {
        Some(host) => cloud::Endpoints::emulator(http, host, &project, &region),
        None => cloud::Endpoints::google(http, &project, &region),
    };
    let settings = cloud::Settings {
        project_id: project,
        api_key,
        heartbeat: Duration::from_secs(c.heartbeat_s.max(5)),
        poll: Duration::from_secs(c.poll_s.max(1)),
        machine_name: config.machine.name.clone(),
        state_dir: config.daemon.state_dir.clone(),
    };
    Some((settings, ends))
}

/// Where sway is: the configured socket, else the newest live one.
pub fn sway_locate(display: &wad_config::Display) -> wad_sway::Locate {
    match &display.socket {
        Some(s) => wad_sway::Locate::Socket(s.clone()),
        None => wad_sway::Locate::RuntimeDir(display.runtime_dir.clone()),
    }
}

/// The machine's own list: `workspaces.json` in the state directory (none:
/// no workspaces).
pub fn own_workspaces(daemon: &wad_config::Daemon) -> Result<Vec<Workspace>, String> {
    let own = daemon.state_dir.join("workspaces.json");
    match std::fs::read_to_string(&own) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", own.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{}: {e}", own.display())),
    }
}

/// The machine's workspaces at start: its own list, with the image's
/// workspaces that are new or changed applied (wad_store::vendor), and
/// saved when that changed something.
pub fn read_workspaces(daemon: &wad_config::Daemon) -> Result<Vec<Workspace>, String> {
    let mut list = own_workspaces(daemon)?;
    let vendor = wad_store::vendor::read(&daemon.vendor_workspaces).unwrap_or_else(|e| {
        tracing::warn!("the image's workspaces: {e}");
        vec![]
    });
    let mut book = wad_store::vendor::read_book(&daemon.state_dir);
    let applied = wad_store::vendor::apply(&mut list, &mut book, &vendor);
    for (id, why) in &applied.refused {
        tracing::warn!("the image's workspace {id} wasn't applied: {why}");
    }
    if !applied.changed.is_empty() {
        tracing::info!("from the image: {}", applied.changed.join(", "));
    }
    if !applied.changed.is_empty() {
        registry::save_list(&daemon.state_dir, &list)?;
        wad_store::vendor::write_book(&daemon.state_dir, &book).map_err(|e| e.to_string())?;
    }
    Ok(list)
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Binds `path`: its directory made if needed, a stale socket replaced. On a
/// machine the socket is 0660 and group `wad`'s (so the kiosk can connect);
/// on a laptop, yours alone.
fn bind(path: &Path, profile: Profile) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let l = UnixListener::bind(path)?;
    let mode = if profile == Profile::System { 0o660 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    if profile == Profile::System
        && let Ok(Some(g)) = nix::unistd::Group::from_name("wad")
    {
        nix::unistd::chown(path, None, Some(g.gid))?;
    }
    Ok(l)
}

impl Server {
    /// A server for `config`, managing workspaces through `backend`.
    pub fn new(
        config: &Config,
        profile: Profile,
        listen: Listen,
        logs: LogBuffer,
        backend: Arc<dyn Backend>,
    ) -> std::io::Result<Self> {
        let (listener, bound) = match listen {
            Listen::Activated(std) => {
                std.set_nonblocking(true)?;
                (UnixListener::from_std(std)?, None)
            }
            Listen::Path(p) => (bind(&p, profile)?, Some(p)),
        };
        let machine = MachineInfo {
            name: config.machine.name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            profile: match profile {
                Profile::System => v1::Profile::System,
                Profile::User => v1::Profile::User,
            },
            started_at: now_secs(),
            screen: None,
        };
        let policy = Policy {
            own_uid: nix::unistd::getuid().as_raw(),
            users: config.daemon.allow_users.clone(),
            groups: config.daemon.allow_groups.clone(),
            directory: Box::new(access::System),
        };
        let d = &config.daemon;
        let bus = Bus::new(machine);
        let uid = nix::unistd::getuid().as_raw();
        let registry = Registry::new(
            backend.clone(),
            bus.clone(),
            Settings {
                ready_timeout: Duration::from_secs(d.ready_timeout_s),
                projects_dir: d.projects_dir.to_string_lossy().into_owned(),
                signins_dir: d.signins_dir.clone(),
                state_dir: d.state_dir.clone(),
                // Rootless podman maps you to this uid inside (PUID 1000).
                rootless_uid: (uid != 0).then_some(uid),
                device_rules: if uid == 0 { devices::machine_cgroup_rules() } else { vec![] },
                drives_dir: (uid == 0).then(|| devices::server::ATTACHED.into()),
                poll: Duration::from_secs(1),
            },
        );
        let workspaces = read_workspaces(d).unwrap_or_else(|e| {
            tracing::error!("no workspaces: {e}");
            vec![]
        });
        let sway = config
            .display
            .enabled
            .then(|| Arc::new(SwayDisplay::new(wad_sway::Sway::new(sway_locate(&config.display)), registry.windows())));
        let display: Arc<dyn Display> = match &sway {
            Some(s) => s.clone(),
            None => Arc::new(NullDisplay),
        };
        let view = View::new(registry.clone(), backend.clone(), display, bus.clone(), d.state_dir.clone());
        // Folder projects say which machine they're on: its linked id, or
        // "local" until it's linked.
        let machine: projects::MachineRef = {
            let (store, name) = (wad_store::State::new(&d.state_dir), config.machine.name.clone());
            Arc::new(move || {
                let link = store.cloud();
                let id = link.machine_id.filter(|_| link.linked).unwrap_or_else(|| wad_store::projects::LOCAL.into());
                (id, name.clone())
            })
        };
        let drives = Arc::new(drives::Drives::new(d.projects_uid, Arc::new(drives::System), uid == 0));
        let devices_drives = drives.clone();
        // The device server, once it's made (before a wadspace starts: its ~/Drives).
        let devices_cell: Arc<std::sync::OnceLock<std::sync::Weak<devices::Devices>>> = Arc::default();
        let copies = Arc::new(copies::Copies::new(
            &d.state_dir,
            d.folder_roots.clone(),
            d.projects_uid,
            machine.clone(),
            drives.clone(),
            registry.clone(),
        ));
        let projects = Arc::new(projects::Projects::new(
            &d.state_dir,
            d.projects_dir.clone(),
            d.folder_roots.clone(),
            d.projects_uid,
            machine,
            drives,
            Arc::new(wad_git::Git::default()),
            registry.clone(),
            bus.clone(),
        ));
        let launches = launches::Launches::new(
            registry.clone(),
            view.clone(),
            backend.clone(),
            projects.clone(),
            copies.clone(),
            bus.clone(),
            &d.state_dir,
        );
        let builds = builds::Builds::new(backend.clone(), bus.clone(), &d.state_dir, d.build_min_free_gb);
        let packages = packages::Packages::new(backend.clone(), bus.clone(), packages::Settings::new(config));
        builds.set_packages(packages.clone());
        builds.set_registry(registry.clone());
        // Streams: each run of wadd is told apart by the self-test.
        let instance = streams::random_hex(8);
        let tunnel = Arc::new(tunnel::Tunnel::new(
            &config.streaming,
            instance.clone(),
            profile == Profile::System && nix::unistd::Uid::effective().is_root(),
        ));
        let streams = streams::Streams::new(
            config.streaming.clone(),
            &d.state_dir,
            registry.clone(),
            view.clone(),
            backend.clone(),
            bus.clone(),
            tunnel,
            launches.clone(),
            instance,
        );
        let secrets = Arc::new(secrets::Secrets::new(backend.clone(), &d.state_dir));
        let signins =
            signins::SignIns::new(&d.state_dir, d.signins_dir.clone(), d.projects_uid, registry.clone(), bus.clone());
        {
            // Before a container starts: its mounts found here, then its
            // sign-ins, its projects.json (no mount that isn't here), and the
            // folders its copies are mounted from.
            let found = copies.clone();
            registry.on_mounts(move |ws| {
                let copies = found.clone();
                Box::pin(async move { copies.find_mounts(&ws).await })
            });
            let (signins, copies, state_dir) = (Arc::downgrade(&signins), copies.clone(), d.state_dir.clone());
            let reg = Arc::downgrade(&registry);
            let devs = devices_cell.clone();
            registry.on_prepare(move |ws| {
                if ws.display == wad_proto::v1::Display::Host
                    && !reg.upgrade().is_some_and(|r| r.is_streamed(&ws.id))
                    && let Some(d) = devs.get().and_then(std::sync::Weak::upgrade)
                {
                    d.prepare(&ws.id).map_err(|e| format!("its ~/Drives: {e}"))?;
                }
                signins.upgrade().map_or(Ok(()), |s| s.prepare(ws))?;
                copies.prepare(ws)?;
                let mounts = reg.upgrade().map(|r| r.mounts_of(&ws.id)).unwrap_or_default();
                let here = copies::projects_here(ws, &mounts);
                wad_store::projects::write_manifest(&state_dir, &ws.id, &here)
                    .map(drop)
                    .map_err(|e| format!("projects.json: {e}"))
            });
        }
        launches.set_signins(Arc::downgrade(&signins));
        launches.set_builds(Arc::downgrade(&builds));
        let registry_backend = backend.clone();
        let tailnet = tailnet::Tailnet::new(&config.tailnet, &config.machine.name, &d.state_dir, bus.clone());
        let cloud_cfg = cloud_settings(config);
        let github = github::GithubService::new(
            wad_github::Github::new(reqwest::Client::new()),
            github::Parts { backend, secrets: secrets.clone(), projects: projects.clone() },
        );
        let cloud = cloud_cfg.map(|(cs, ends)| {
            cloud::CloudRelay::new(
                cs,
                reqwest::Client::new(),
                ends,
                cloud::Parts {
                    registry: registry.clone(),
                    view: view.clone(),
                    projects: projects.clone(),
                    secrets: secrets.clone(),
                    launches: launches.clone(),
                    github: github.clone(),
                    bus: bus.clone(),
                    builds: builds.clone(),
                    signins: signins.clone(),
                    backend: registry_backend.clone(),
                    tailnet: tailnet.enabled().then(|| tailnet.clone()),
                },
            )
        });
        let storage = storage::Storage::new(
            &config.daemon.state_dir,
            profile == Profile::System,
            registry_backend.clone(),
            registry.clone(),
            bus.clone(),
        );
        // Devices are handed to wadspaces only by root (rootful podman).
        let devices = devices::Devices::new(
            &config.daemon.state_dir,
            devices::sysfs::Roots::default(),
            Arc::new(devices::attach::HostHands),
            devices_drives,
            devices::server::ATTACHED.into(),
            registry_backend.clone(),
            registry.clone(),
            bus.clone(),
            nix::unistd::Uid::effective().is_root(),
        );
        let _ = devices_cell.set(Arc::downgrade(&devices));
        let state = Arc::new(AppState {
            bluetooth: bluetooth::Bluetooth::new(None, None, bus.clone()),
            devices,
            registry_backend,
            meter: metrics::Meter::default(),
            network: network::Network::new(None, bus.clone()),
            power: None,
            backlight: screen::Backlight::default(),
            github,
            secrets,
            signins,
            cloud,
            bus,
            registry,
            view,
            projects,
            copies,
            launches,
            builds,
            packages,
            keys: std::sync::Mutex::new(None),
            keys_enabled: config.keys.enabled,
            logs,
            policy,
            store: wad_store::State::new(&config.daemon.state_dir),
            storage,
            graph_root: Default::default(),
            files: files::Files::new(&config.daemon.state_dir, &config.daemon.signins_dir, profile == Profile::System),
            streams,
            tailnet,
        });
        Ok(Self {
            listener,
            bound,
            state,
            workspaces,
            reconcile_every: Duration::from_secs(d.reconcile_s.max(1)),
            sway,
            keys: config.keys.clone(),
            proxy_listen: if config.streaming.enabled { config.streaming.listen.clone() } else { vec![] },
        })
    }

    /// The machine's network and power (NetworkManager and logind over the
    /// system bus); before serving.
    pub fn set_machine(
        &mut self,
        nm: Option<wad_net::NetworkManager>,
        power: Option<wad_systemd::Power>,
        bt: Option<(wad_bt::Bluez, tokio::sync::mpsc::UnboundedReceiver<wad_bt::AgentEvent>)>,
    ) {
        let st = Arc::get_mut(&mut self.state).expect("set before serving");
        st.network = network::Network::new(nm, st.bus.clone());
        st.power = power;
        let (bz, agent) = bt.map_or((None, None), |(b, a)| (Some(b), Some(a)));
        st.bluetooth = bluetooth::Bluetooth::new(bz, agent, st.bus.clone());
    }

    /// The keyboard proxy, its actions handed to the view.
    fn start_keys(&self) -> Option<wad_input::Proxy> {
        if !self.keys.enabled {
            return None;
        }
        let view = self.state.view.clone();
        let block = self
            .keys
            .block
            .iter()
            .filter_map(|c| wad_input::Chord::parse(c).inspect_err(|e| tracing::warn!("keys.block: {e}")).ok())
            .collect();
        let router = wad_input::KeyRouter::new(view.bindings(&self.keys), block, self.keys.pass_super);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(action) = rx.recv().await {
                view.on_key(action);
            }
        });
        Some(wad_input::Proxy::start(router, self.keys.grab, move |a| {
            let _ = tx.send(a);
        }))
    }

    /// Serves until `shutdown` resolves.
    pub async fn run(mut self, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
        let registry = self.state.registry.clone();
        // Streams from before first: their sidecars' units must survive the
        // first load, and sidecars with no stream are stopped while their
        // units are still there.
        self.state.streams.adopt().await;
        if let Err(e) = registry.load(std::mem::take(&mut self.workspaces)).await {
            tracing::warn!("workspace units: {e}");
        }
        self.state.view.restore_session();
        // The old Builder's documents kept here go.
        match self.state.store.discard_old_builder() {
            Ok(0) => {}
            Ok(n) => tracing::info!("removed {n} of the old Builder's document(s)"),
            Err(e) => tracing::warn!("library: {e}"),
        }
        // Wadspaces installed as version 4 are version 5's now.
        match crate::sets::upgrade_installed(&self.state.store.dir) {
            Ok(0) => {}
            Ok(n) => tracing::info!("upgraded {n} installed wadspace(s) to version 5"),
            Err(e) => tracing::warn!("installed wadspaces: {e}"),
        }
        // Runs left over from before (wadd or the machine restarted).
        let sweep = registry.clone();
        tokio::spawn(async move {
            sweep.reconcile().await;
            sweep.sweep_ephemeral().await;
        });
        self.state.storage.start();
        let watcher = self.sway.clone().map(|d| tokio::spawn(d.run(self.state.view.clone() as Arc<dyn WindowSink>)));
        *self.state.keys.lock().unwrap() = self.start_keys();
        // A build can add a workspace (its hotkey, the secrets it names).
        let (app, keys) = (Arc::downgrade(&self.state), self.keys.clone());
        registry.on_list_change(move || {
            let Some(app) = app.upgrade() else { return };
            if let Some(proxy) = app.keys.lock().unwrap().as_ref() {
                proxy.router().lock().unwrap().set_bindings(app.view.bindings(&keys));
            }
            tokio::spawn(placeholders(app));
        });
        tokio::spawn(placeholders(self.state.clone()));
        let relay = self.state.cloud.clone().map(|c| tokio::spawn(c.run()));
        let reconcile = registry.spawn_reconcile(self.reconcile_every);
        let netwatch = tokio::spawn(self.state.network.clone().run());
        let devwatch = tokio::spawn(self.state.devices.clone().run());
        self.state.bluetooth.set_devices(&self.state.devices);
        let btwatch = tokio::spawn(self.state.bluetooth.clone().run());
        let autostart = tokio::spawn(registry.clone().autostart(Duration::from_secs(300)));
        let streamwatch = tokio::spawn(self.state.streams.clone().run(Duration::from_secs(10)));
        let (stop_proxy, proxy_stop) = tokio::sync::watch::channel(false);
        let listeners = proxy::bind(&self.proxy_listen).await;
        let proxy = (!listeners.is_empty())
            .then(|| tokio::spawn(proxy::Proxy::new(self.state.streams.clone()).serve(listeners, proxy_stop)));
        // The tailnet, and the remote API on its addresses.
        let tailwatch = tokio::spawn(self.state.tailnet.clone().run());
        let (stop_remote, remote_stop) = tokio::sync::watch::channel(false);
        let remote = tokio::spawn(remote::serve(self.state.clone(), remote_stop));
        let app = api::router(self.state.clone()).into_make_service_with_connect_info::<Peer>();
        let res = axum::serve(self.listener, app).with_graceful_shutdown(shutdown).await;
        let _ = stop_proxy.send(true);
        if let Some(p) = proxy {
            p.abort();
        }
        let _ = stop_remote.send(true);
        remote.abort();
        tailwatch.abort();
        streamwatch.abort();
        reconcile.abort();
        netwatch.abort();
        devwatch.abort();
        btwatch.abort();
        autostart.abort();
        if let Some(r) = relay {
            r.abort();
        }
        if let Some(w) = watcher {
            w.abort();
        }
        // Lets every key go and ungrabs (the thread notices within 0.5 s).
        let keys = self.state.keys.lock().unwrap().take();
        if let Some(k) = keys {
            let _ = tokio::task::spawn_blocking(move || drop(k)).await;
        }
        if let Some(p) = &self.bound {
            let _ = std::fs::remove_file(p);
        }
        res
    }
}
