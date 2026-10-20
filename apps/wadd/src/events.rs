//! What changed, for everyone watching /v1/events: a broadcast of events,
//! plus the latest of each kind (and each workspace's state) so a new
//! watcher starts from the present.

use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use wad_proto::v1::{Event, MachineInfo, WorkspaceState};

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Event>,
    latest: Arc<Mutex<Latest>>,
}

struct Latest {
    machine: MachineInfo,
    view: Option<Event>,
    carousel: Option<Event>,
    session: Option<Event>,
    cloud: Option<Event>,
    network: Option<Event>,
    workspaces: Option<Event>,
    devices: Option<Event>,
    bluetooth: Option<Event>,
    streams: Option<Event>,
    tailnet: Option<Event>,
    states: Vec<WorkspaceState>,
}

impl Bus {
    pub fn new(machine: MachineInfo) -> Self {
        let latest = Latest {
            machine,
            view: None,
            carousel: None,
            session: None,
            cloud: None,
            network: None,
            workspaces: None,
            devices: None,
            bluetooth: None,
            streams: None,
            tailnet: None,
            states: vec![],
        };
        Self { tx: broadcast::channel(256).0, latest: Arc::new(Mutex::new(latest)) }
    }

    pub fn publish(&self, e: Event) {
        {
            let mut l = self.latest.lock().unwrap();
            match &e {
                Event::Machine(m) => l.machine = m.clone(),
                Event::WorkspaceState(st) => match l.states.iter_mut().find(|s| s.id == st.id) {
                    Some(s) => *s = st.clone(),
                    None => l.states.push(st.clone()),
                },
                Event::View(_) => l.view = Some(e.clone()),
                Event::Carousel(_) => l.carousel = Some(e.clone()),
                Event::Session(_) => l.session = Some(e.clone()),
                Event::Cloud(_) => l.cloud = Some(e.clone()),
                Event::Network(_) => l.network = Some(e.clone()),
                Event::Workspaces(_) => l.workspaces = Some(e.clone()),
                Event::Devices(_) => l.devices = Some(e.clone()),
                Event::Bluetooth(_) => l.bluetooth = Some(e.clone()),
                Event::Streams(_) => l.streams = Some(e.clone()),
                Event::Tailnet(_) => l.tailnet = Some(e.clone()),
                Event::Notice { .. }
                | Event::Shortcut { .. }
                | Event::Projects { .. }
                | Event::SignIns { .. }
                | Event::Launch(_)
                | Event::Build(_)
                | Event::Packages(_)
                | Event::Storage { .. } => {}
            }
        }
        let _ = self.tx.send(e);
    }

    pub fn machine(&self) -> MachineInfo {
        self.latest.lock().unwrap().machine.clone()
    }

    /// The workspaces there are now, in order (a new list replaces the old,
    /// without an event for each).
    pub fn set_states(&self, states: Vec<WorkspaceState>) {
        self.latest.lock().unwrap().states = states;
    }

    /// The current state (the machine, what's on screen, the session, each
    /// workspace's), then whatever happens next.
    pub fn subscribe(&self) -> (Vec<Event>, broadcast::Receiver<Event>) {
        let rx = self.tx.subscribe();
        let l = self.latest.lock().unwrap();
        let mut now = vec![Event::Machine(l.machine.clone())];
        now.extend(
            [
                &l.workspaces,
                &l.view,
                &l.session,
                &l.carousel,
                &l.cloud,
                &l.network,
                &l.devices,
                &l.bluetooth,
                &l.streams,
                &l.tailnet,
            ]
            .into_iter()
            .flatten()
            .cloned(),
        );
        now.extend(l.states.iter().cloned().map(Event::WorkspaceState));
        (now, rx)
    }
}
