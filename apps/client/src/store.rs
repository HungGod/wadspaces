//! What the app knows, in one place: wadd's state as its events tell it
//! (and, later, the account's library). Widgets watch the topics they draw
//! from; an update marks topics and one idle pass redraws what watches them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use wad_proto::v1::{CloudLink, Event, MachineInfo, NetworkStatus, Session, ViewState, Workspace, WorkspaceState};
use wad_wadd_client::Incoming;

/// What changed, as bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Topic(u32);

impl Topic {
    pub const MACHINE: Topic = Topic(1);
    pub const VIEW: Topic = Topic(1 << 1);
    pub const WORKSPACES: Topic = Topic(1 << 2);
    pub const STATES: Topic = Topic(1 << 3);
    pub const SESSION: Topic = Topic(1 << 4);
    pub const NETWORK: Topic = Topic(1 << 5);
    pub const CLOUD: Topic = Topic(1 << 6);
    pub const WADD: Topic = Topic(1 << 7);
    /// Who's signed in, their profile, this machine's GitHub token.
    pub const ACCOUNT: Topic = Topic(1 << 8);
    /// The account's library (templates, Extensions, wadspaces) and machines.
    pub const LIBRARY: Topic = Topic(1 << 9);
    /// Builds and launches under way (the sidebar's tray).
    pub const ACTIVITY: Topic = Topic(1 << 10);
    /// wadd counted the disk again (GET /v1/storage has a new report).
    pub const STORAGE: Topic = Topic(1 << 11);
    /// The machine's saved sign-ins and tokens (GET /v1/signins, /v1/secrets).
    pub const SIGNINS: Topic = Topic(1 << 12);
    /// The devices plugged in, and Bluetooth (GET /v1/devices, /v1/bluetooth).
    pub const DEVICES: Topic = Topic(1 << 13);
    /// Wadspaces streamed to browsers, and the tunnel (GET /v1/streams, /v1/tunnel).
    pub const STREAMS: Topic = Topic(1 << 14);
    /// The tailnet, and the devices trusted or asking (GET /v1/tailnet).
    pub const TAILNET: Topic = Topic(1 << 15);
    pub const NONE: Topic = Topic(0);

    pub fn any(self, other: Topic) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for Topic {
    type Output = Topic;
    fn bitor(self, o: Topic) -> Topic {
        Topic(self.0 | o.0)
    }
}

#[derive(Debug, Default)]
pub struct AppState {
    /// wadd answers (its event stream is up).
    pub connected: bool,
    pub machine: Option<MachineInfo>,
    pub view: Option<ViewState>,
    pub workspaces: Vec<Workspace>,
    pub states: HashMap<String, WorkspaceState>,
    pub session: Option<Session>,
    pub network: Option<NetworkStatus>,
    pub cloud: Option<CloudLink>,
    pub account: AccountState,
    pub library: Option<wad_account::Library>,
    /// Builds since the app started, with their logs (the tray's).
    pub builds: Vec<(wad_proto::v1::Build, Vec<String>)>,
    pub launches: Vec<(wad_proto::v1::Launch, Vec<String>)>,
    /// The saved sign-ins here (None until read).
    pub signins: Option<Vec<wad_proto::v1::SignIn>>,
    /// The machine's tokens, by name (never their values).
    pub secrets: Vec<wad_proto::v1::SecretInfo>,
    /// What's plugged in, and which wadspaces have it.
    pub devices: Vec<wad_proto::v1::Device>,
    pub bluetooth: Option<wad_proto::v1::BluetoothStatus>,
    /// What's streamed from here (never the passwords).
    pub streams: Vec<wad_proto::v1::StreamInfo>,
    /// The Cloudflare tunnel streams go through (None until read).
    pub tunnel: Option<wad_proto::v1::TunnelStatus>,
    /// The machine on its owner's tailnet (None until read).
    pub tailnet: Option<wad_proto::v1::TailnetStatus>,
}

/// A job's log, with lines from `from` on (an event repeats or skips none).
pub(crate) fn take_lines(log: &mut Vec<String>, from: u64, lines: Vec<String>) {
    log.truncate(from as usize);
    log.extend(lines);
}

/// The person signed in, as far as the app knows.
#[derive(Debug, Default, Clone)]
pub struct AccountState {
    /// The last sign-in was looked for (the app restarted: maybe still in).
    pub checked: bool,
    pub user: Option<wad_account::User>,
    pub profile: ProfileState,
    /// This machine's GitHub token works (None: not known yet).
    pub github: Option<bool>,
    /// "Later" on the GitHub step.
    pub github_later: bool,
    /// The account's settings aren't there (no firebase.toml).
    pub unconfigured: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub enum ProfileState {
    #[default]
    Unknown,
    /// Signed in without a username yet.
    Missing,
    Have(wad_account::Profile),
}

impl AppState {
    /// The profile, once there is one.
    pub fn profile(&self) -> Option<&wad_account::Profile> {
        match &self.account.profile {
            ProfileState::Have(p) => Some(p),
            _ => None,
        }
    }

    /// The wadspaces this machine can start, not deleted, in the board's
    /// order: version 3's (any machine takes them) and the version 4 ones
    /// that live here.
    pub fn wadspaces(&self) -> Vec<&serde_json::Value> {
        self.all_wadspaces().into_iter().filter(|w| self.lives_here(w)).collect()
    }

    /// Every wadspace that isn't deleted, on any machine, in the board's order.
    pub fn all_wadspaces(&self) -> Vec<&serde_json::Value> {
        let mut ws: Vec<&serde_json::Value> = self
            .library
            .iter()
            .flat_map(|l| l.all_wadspaces())
            .filter(|w| w.get("deletedAt").is_none_or(|d| d.is_null()))
            .collect();
        ws.sort_by(|a, b| {
            let o = |w: &serde_json::Value| w.get("order").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            o(a).total_cmp(&o(b))
        });
        ws
    }

    /// Whether a wadspace runs on this machine: a version 4 one lives on
    /// one machine (any, while this one isn't linked to the account: wadd
    /// takes it then); version 3's go anywhere.
    pub fn lives_here(&self, w: &serde_json::Value) -> bool {
        if !wad_account::library::is_v4(w) {
            return true;
        }
        let here = self.cloud.as_ref().filter(|c| c.linked).and_then(|c| c.machine_id.as_deref());
        here.is_none_or(|m| w.get("machine").and_then(serde_json::Value::as_str) == Some(m))
    }

    /// A wadspace's name (the web app's wadspaceName).
    pub fn wadspace_name(&self, w: &serde_json::Value) -> String {
        let s = |v: &serde_json::Value, k: &str| {
            v.get(k).and_then(serde_json::Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(String::from)
        };
        if let Some(n) = w.get("customization").and_then(|c| s(c, "name")) {
            return n;
        }
        let lib = self.library.as_ref();
        let find = |coll: &[serde_json::Value], id: Option<String>| {
            id.and_then(|id| coll.iter().find(|d| s(d, "id").as_deref() == Some(&id)).and_then(|d| s(d, "name")))
        };
        lib.and_then(|l| find(&l.extensions, s(w, "extensions")))
            .or_else(|| lib.and_then(|l| find(&l.templates, s(w, "template"))))
            .unwrap_or_else(|| "Untitled".into())
    }

    /// NetworkManager says the machine's online.
    pub fn online(&self) -> bool {
        self.network.as_ref().is_some_and(|n| n.connectivity == "full")
    }
}

type Draw = Rc<dyn Fn(&AppState)>;
type Watcher = (Topic, glib::WeakRef<gtk::Widget>, Draw);

#[derive(Default)]
pub struct Store {
    state: RefCell<AppState>,
    dirty: Cell<Topic>,
    scheduled: Cell<bool>,
    watchers: RefCell<Vec<Watcher>>,
}

thread_local! {
    static STORE: Rc<Store> = Rc::new(Store::default());
}

/// The app's store (GTK's thread only).
pub fn store() -> Rc<Store> {
    STORE.with(Rc::clone)
}

impl Store {
    /// Changes the state; `f` says which topics changed. Watchers of those
    /// run once, together, when GTK is next idle.
    pub fn update(self: &Rc<Self>, f: impl FnOnce(&mut AppState) -> Topic) {
        let t = f(&mut self.state.borrow_mut());
        if t == Topic::NONE {
            return;
        }
        self.dirty.set(self.dirty.get() | t);
        if !self.scheduled.replace(true) {
            let me = self.clone();
            glib::idle_add_local_once(move || me.flush());
        }
    }

    fn flush(&self) {
        self.scheduled.set(false);
        let dirty = self.dirty.replace(Topic::NONE);
        // Watchers whose widget is gone go too.
        self.watchers.borrow_mut().retain(|(_, w, _)| w.upgrade().is_some());
        // Taken out first: a watcher may make widgets that watch too.
        let due: Vec<(glib::WeakRef<gtk::Widget>, Draw)> = self
            .watchers
            .borrow()
            .iter()
            .filter(|(t, _, _)| t.any(dirty))
            .map(|(_, w, f)| (w.clone(), f.clone()))
            .collect();
        let st = self.state.borrow();
        for (w, f) in due {
            if w.upgrade().is_some() {
                f(&st);
            }
        }
    }

    /// What's known now.
    pub fn read_now<T>(&self, f: impl FnOnce(&AppState) -> T) -> T {
        f(&self.state.borrow())
    }

    /// Runs `f` now and whenever `topics` change, while `widget` lives.
    pub fn watch(&self, widget: &impl IsA<gtk::Widget>, topics: Topic, f: impl Fn(&AppState) + 'static) {
        f(&self.state.borrow());
        self.watchers.borrow_mut().push((topics, widget.upcast_ref::<gtk::Widget>().downgrade(), Rc::new(f)));
    }

    /// One of wadd's events.
    pub fn apply(self: &Rc<Self>, i: Incoming) {
        self.update(|s| match i {
            Incoming::Disconnected => {
                let was = s.connected;
                s.connected = false;
                if was { Topic::WADD } else { Topic::NONE }
            }
            Incoming::Other(..) => Topic::NONE,
            Incoming::Event(e) => {
                let wadd = if s.connected { Topic::NONE } else { Topic::WADD };
                s.connected = true;
                wadd | match e {
                    Event::Machine(m) => {
                        s.machine = Some(m);
                        Topic::MACHINE
                    }
                    Event::View(v) => {
                        s.view = Some(v);
                        Topic::VIEW
                    }
                    Event::Workspaces(w) => {
                        s.workspaces = w;
                        Topic::WORKSPACES
                    }
                    Event::WorkspaceState(st) => {
                        s.states.insert(st.id.clone(), st);
                        Topic::STATES
                    }
                    Event::Session(x) => {
                        s.session = x;
                        Topic::SESSION
                    }
                    Event::Network(n) => {
                        let changed = s.network.as_ref() != Some(&n);
                        s.network = Some(n);
                        if changed { Topic::NETWORK } else { Topic::NONE }
                    }
                    Event::Cloud(c) => {
                        // A heartbeat alone changes nothing shown.
                        let same = s.cloud.as_ref().is_some_and(|o| {
                            o.linked == c.linked && o.owner_uid == c.owner_uid && o.last_error == c.last_error
                        });
                        s.cloud = Some(c);
                        if same { Topic::NONE } else { Topic::CLOUD }
                    }
                    Event::Build(b) => {
                        let id = b.build.id.clone();
                        match s.builds.iter_mut().find(|(x, _)| x.id == id) {
                            Some((x, log)) => {
                                *x = b.build;
                                take_lines(log, b.from, b.lines);
                            }
                            None => s.builds.push((b.build, b.lines)),
                        }
                        Topic::ACTIVITY
                    }
                    Event::Launch(l) => {
                        let id = l.launch.id.clone();
                        match s.launches.iter_mut().find(|(x, _)| x.id == id) {
                            Some((x, log)) => {
                                *x = l.launch;
                                take_lines(log, l.from, l.lines);
                            }
                            None => s.launches.push((l.launch, l.lines)),
                        }
                        Topic::ACTIVITY
                    }
                    Event::Storage { .. } => Topic::STORAGE,
                    Event::Devices(d) => {
                        s.devices = d;
                        Topic::DEVICES
                    }
                    Event::Bluetooth(b) => {
                        let changed = s.bluetooth.as_ref() != Some(&b);
                        s.bluetooth = Some(b);
                        if changed { Topic::DEVICES } else { Topic::NONE }
                    }
                    Event::Streams(list) => {
                        s.streams = list;
                        Topic::STREAMS
                    }
                    Event::Tailnet(t) => {
                        // A device asking for the first time: a toast with Allow.
                        crate::pages::machines::tailnet::toast_asks(s.tailnet.as_ref(), &t);
                        s.tailnet = Some(t);
                        Topic::TAILNET
                    }
                    // Something wadd tells whoever's watching: a toast.
                    Event::Notice { text } => {
                        gtk::glib::idle_add_local_once(move || {
                            crate::toast::show(&text, None, crate::toast::Tone::Info)
                        });
                        Topic::NONE
                    }
                    _ => Topic::NONE,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_version_4_wadspace_shows_on_its_own_machine() {
        let v4 = |id: &str, machine: &str, order: u32| json!({ "version": 4, "id": id, "machine": machine, "order": order, "templates": [], "customization": { "name": id } });
        let lib = wad_account::Library {
            wadspaces: vec![json!({ "version": 3, "id": "old", "order": 1, "template": "dev" })],
            v4: vec![v4("here", "m-here", 2), v4("there", "m-there", 3)],
            ..Default::default()
        };
        let mut s = AppState { library: Some(lib), ..Default::default() };
        fn ids(ws: Vec<&serde_json::Value>) -> Vec<&str> {
            ws.into_iter().map(|w| w["id"].as_str().unwrap_or("")).collect()
        }
        // Not linked yet: wadd takes any (as it does then).
        assert_eq!(ids(s.wadspaces()), ["old", "here", "there"]);
        s.cloud = Some(CloudLink {
            linked: true,
            machine_id: Some("m-here".into()),
            owner_uid: Some("me".into()),
            project_id: None,
            linked_at: None,
            last_heartbeat: None,
            last_error: None,
        });
        assert_eq!(ids(s.wadspaces()), ["old", "here"]);
        assert_eq!(ids(s.all_wadspaces()), ["old", "here", "there"]);
    }
}
