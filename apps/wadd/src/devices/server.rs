//! The device server: what's plugged in (sysfs, followed through udev's
//! events), what each device remembers (the wadspaces it goes to, kept in
//! `<state_dir>/devices.json`), and the nodes handed to running wadspaces.
//!
//! What's handed out follows one rule, applied whenever anything changes
//! ([`Devices::reconcile`]): a device is in a wadspace exactly when it's
//! plugged in, handed out (not a keyboard, mouse, sound or drive), the
//! wadspace runs, and the device remembers it. Attaching remembers;
//! detaching forgets. A device plugged in for the first time remembers
//! nothing and is `new` until it's asked about (the HUD asks), so it stays
//! on the machine until then. Several wadspaces may share one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wad_proto::v1::{Device, DeviceBus, DeviceKind, Display, Event, Kind};
use wad_proto::{ApiError, ErrorCode};

use super::attach::{Hands, NodeOp};
use super::sysfs::{self, Found, Node, Roots};
use super::uevent;
use crate::backend::Backend;
use crate::events::Bus;
use crate::registry::Registry;

const FILE: &str = "devices.json";
/// Checks between events (a wadspace that started or stopped, an event lost).
const TICK: Duration = Duration::from_secs(3);
/// udev sends several events for one plug (the device, then its parts).
const SETTLE: Duration = Duration::from_millis(300);

/// What wadd keeps about a device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Known {
    /// The name it was given (none: its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Its own name, from when it was last plugged in.
    own_name: String,
    kind: Option<DeviceKind>,
    bus: Option<DeviceBus>,
    /// The wadspaces it goes to.
    #[serde(default)]
    remembered: Vec<String>,
    /// Asked about where it goes (or attached by hand) at least once.
    #[serde(default)]
    asked: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    devices: BTreeMap<String, Known>,
}

/// Nodes handed to one running wadspace (its container's first process).
#[derive(Debug, Clone)]
struct Handed {
    pid: u32,
    nodes: Vec<Node>,
}

#[derive(Default)]
struct State {
    known: BTreeMap<String, Known>,
    present: BTreeMap<String, Found>,
    /// (wadspace, device) → what it was handed.
    handed: BTreeMap<(String, String), Handed>,
    /// Gave an error handing it over: said once, not every tick.
    failed: BTreeSet<(String, String)>,
}

pub struct Devices {
    roots: Roots,
    hands: Arc<dyn Hands>,
    backend: Arc<dyn Backend>,
    registry: Arc<Registry>,
    bus: Bus,
    file: PathBuf,
    /// Devices can be handed out here (rootful podman: wadd is root).
    available: bool,
    state: Mutex<State>,
    one_at_a_time: tokio::sync::Mutex<()>,
    seqnum: AtomicU64,
    poke: tokio::sync::Notify,
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

/// A device key from a URL: what sysfs keys look like.
pub fn key_ok(key: &str) -> bool {
    (key.starts_with("usb:") || key.starts_with("bt:"))
        && key.len() <= 200
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '@' | '.' | '_' | '-'))
}

/// Stops a task when its owner stops.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn same_node(a: &Node, b: &Node) -> bool {
    a.devname == b.devname && a.major == b.major && a.minor == b.minor
}

impl Devices {
    pub fn new(
        state_dir: &std::path::Path,
        roots: Roots,
        hands: Arc<dyn Hands>,
        backend: Arc<dyn Backend>,
        registry: Arc<Registry>,
        bus: Bus,
        available: bool,
    ) -> Arc<Self> {
        let file = state_dir.join(FILE);
        let saved: Saved = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Arc::new(Self {
            roots,
            hands,
            backend,
            registry,
            bus,
            file,
            available,
            state: Mutex::new(State { known: saved.devices, ..State::default() }),
            one_at_a_time: tokio::sync::Mutex::new(()),
            seqnum: AtomicU64::new(1),
            poke: tokio::sync::Notify::new(),
        })
    }

    /// Follows the machine's devices and wadspaces until it's stopped.
    pub async fn run(self: Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _listener = AbortOnDrop(tokio::spawn(async move {
            if let Err(e) = uevent::listen(tx).await {
                tracing::warn!("devices: udev's events: {e}; checking every {}s instead", TICK.as_secs());
            }
        }));
        let (_, mut events) = self.bus.subscribe();
        self.rescan();
        self.reconcile().await;
        loop {
            let go = tokio::select! {
                _ = tokio::time::sleep(TICK) => true,
                _ = self.poke.notified() => true,
                Some(_) = rx.recv() => {
                    tokio::time::sleep(SETTLE).await;
                    while rx.try_recv().is_ok() {}
                    true
                }
                // A wadspace started or stopped: hand out (or forget) now.
                e = events.recv() => matches!(e, Ok(Event::WorkspaceState(_)) | Err(_)),
            };
            if go {
                self.rescan();
                self.reconcile().await;
            }
        }
    }

    /// Reads what's plugged in now; a new device handed out is `new` (asked
    /// about) until it's answered.
    pub fn rescan(&self) {
        let found = sysfs::scan(&self.roots);
        let mut s = self.state.lock().unwrap();
        let mut changed = false;
        for f in found.values() {
            let k = s.known.entry(f.key.clone()).or_default();
            let before = k.clone();
            k.own_name = f.name.clone();
            k.kind = Some(f.kind);
            k.bus = Some(f.bus);
            changed |= *k != before;
        }
        // Devices that stay with the machine aren't remembered once gone.
        s.known.retain(|key, k| {
            found.contains_key(key)
                || k.asked
                || k.name.is_some()
                || k.kind.is_some_and(sysfs::attachable) && !k.remembered.is_empty()
        });
        let present_changed = s.present.keys().ne(found.keys())
            || s.present.iter().any(|(k, f)| found.get(k).is_none_or(|g| g.nodes != f.nodes || g.name != f.name));
        s.present = found;
        if changed {
            self.save(&s);
        }
        drop(s);
        if changed || present_changed {
            self.publish();
        }
    }

    fn save(&self, s: &State) {
        let saved = Saved { devices: s.known.clone() };
        let tmp = self.file.with_extension("json.tmp");
        let r = std::fs::write(&tmp, serde_json::to_vec_pretty(&saved).expect("json"))
            .and_then(|_| std::fs::rename(&tmp, &self.file));
        if let Err(e) = r {
            tracing::warn!("devices: {}: {e}", self.file.display());
        }
    }

    fn publish(&self) {
        self.bus.publish(Event::Devices(self.list()));
    }

    /// Every device: plugged in now, or remembered from before.
    pub fn list(&self) -> Vec<Device> {
        let s = self.state.lock().unwrap();
        let mut out: Vec<Device> = s
            .known
            .iter()
            .filter_map(|(key, k)| {
                let f = s.present.get(key);
                let kind = f.map(|f| f.kind).or(k.kind)?;
                let attachable = sysfs::attachable(kind);
                let present = f.is_some();
                Some(Device {
                    key: key.clone(),
                    name: k.name.clone().unwrap_or_else(|| f.map_or(k.own_name.clone(), |f| f.name.clone())),
                    bus: f.map(|f| f.bus).or(k.bus).unwrap_or(DeviceBus::Usb),
                    kind,
                    attachable,
                    present,
                    attached: s.handed.keys().filter(|(_, d)| d == key).map(|(w, _)| w.clone()).collect(),
                    remembered: k.remembered.clone(),
                    new: attachable && present && !k.asked,
                })
            })
            .collect();
        out.sort_by(|a, b| (!a.present, !a.attachable, &a.name).cmp(&(!b.present, !b.attachable, &b.name)));
        out
    }

    /// Wakes the loop: something to apply now.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    fn check_ws(&self, ws: &str) -> Result<(), ApiError> {
        let w = self.registry.workspace(ws)?;
        if w.kind != Kind::Container || w.display != Display::Host {
            return Err(bad(format!("{} can't have devices", w.name)));
        }
        Ok(())
    }

    fn change(&self, key: &str, f: impl FnOnce(&mut Known) -> Result<(), ApiError>) -> Result<Device, ApiError> {
        if !key_ok(key) {
            return Err(bad(format!("{key:?} isn't a device")));
        }
        {
            let mut s = self.state.lock().unwrap();
            let k = s.known.get_mut(key).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such device"))?;
            f(k)?;
            k.asked = true;
            self.save(&s);
        }
        self.publish();
        self.poke();
        self.list()
            .into_iter()
            .find(|d| d.key == key)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such device"))
    }

    fn handed_out(&self, key: &str) -> Result<(), ApiError> {
        if !self.available {
            return Err(bad("this machine can't hand devices to wadspaces (wadd isn't running as root)"));
        }
        let s = self.state.lock().unwrap();
        let kind = s.present.get(key).map(|f| f.kind).or(s.known.get(key).and_then(|k| k.kind));
        if kind.is_some_and(|k| !sysfs::attachable(k)) {
            return Err(bad("it works in every wadspace already, through the machine"));
        }
        Ok(())
    }

    /// Gives a device to a wadspace (now if it runs, and whenever it does).
    pub fn attach(&self, key: &str, ws: &str) -> Result<Device, ApiError> {
        self.handed_out(key)?;
        self.check_ws(ws)?;
        self.change(key, |k| {
            if !k.remembered.iter().any(|w| w == ws) {
                k.remembered.push(ws.into());
            }
            Ok(())
        })
    }

    /// Takes a device back from a wadspace, for good.
    pub fn detach(&self, key: &str, ws: &str) -> Result<Device, ApiError> {
        self.change(key, |k| {
            k.remembered.retain(|w| w != ws);
            Ok(())
        })
    }

    /// Renames a device (None: its own name again), or sets where it goes
    /// (the answer to "where does it go?": none keeps it on the machine).
    pub fn set(
        &self,
        key: &str,
        name: Option<Option<String>>,
        remembered: Option<Vec<String>>,
    ) -> Result<Device, ApiError> {
        if let Some(list) = &remembered {
            if !list.is_empty() {
                self.handed_out(key)?;
            }
            for ws in list {
                self.check_ws(ws)?;
            }
        }
        if let Some(Some(n)) = &name
            && (n.trim().is_empty() || n.len() > 80 || n.contains(char::is_control))
        {
            return Err(bad("a device's name is 1 to 80 characters on one line"));
        }
        self.change(key, |k| {
            if let Some(n) = name {
                k.name = n.map(|n| n.trim().to_string());
            }
            if let Some(mut list) = remembered {
                list.dedup();
                k.remembered = list;
            }
            Ok(())
        })
    }

    /// Forgets a device that isn't plugged in.
    pub fn forget(&self, key: &str) -> Result<(), ApiError> {
        {
            let mut s = self.state.lock().unwrap();
            if s.present.contains_key(key) {
                return Err(bad("it's plugged in: unplug it first"));
            }
            if s.known.remove(key).is_none() {
                return Err(ApiError::new(ErrorCode::NotFound, "no such device"));
            }
            self.save(&s);
        }
        self.publish();
        Ok(())
    }

    /// Applies the rule: each device in exactly the running wadspaces it
    /// remembers, while it's plugged in.
    pub async fn reconcile(&self) {
        if !self.available {
            return;
        }
        let _one = self.one_at_a_time.lock().await;
        // The running native wadspaces, by their container's first process.
        let list = self.registry.workspaces();
        let mut pids: BTreeMap<String, u32> = BTreeMap::new();
        for w in list.iter().filter(|w| w.kind == Kind::Container && w.display == Display::Host) {
            if self.registry.state(&w.id).is_some_and(|s| s.container == "running")
                && let Ok(Some(pid)) = self.backend.container_pid(&w.container_name).await
            {
                pids.insert(w.id.clone(), pid);
            }
        }
        // What should be where, and what to do about what is.
        let (wanted, mut todo) = {
            let mut s = self.state.lock().unwrap();
            // A wadspace that's gone is forgotten.
            let mut pruned = false;
            for k in s.known.values_mut() {
                let n = k.remembered.len();
                k.remembered.retain(|w| list.iter().any(|x| &x.id == w));
                pruned |= k.remembered.len() != n;
            }
            if pruned {
                self.save(&s);
            }
            let mut wanted: BTreeMap<(String, String), (u32, Vec<Node>)> = BTreeMap::new();
            for (key, k) in &s.known {
                let Some(f) = s.present.get(key).filter(|f| sysfs::attachable(f.kind)) else { continue };
                for ws in &k.remembered {
                    if let Some(pid) = pids.get(ws) {
                        let nodes = f.handed_nodes().into_iter().cloned().collect();
                        wanted.insert((ws.clone(), key.clone()), (*pid, nodes));
                    }
                }
            }
            // A container that stopped (or started again) took its nodes with it.
            s.handed.retain(|(ws, _), h| pids.get(ws) == Some(&h.pid));
            let todo: Vec<((String, String), Option<Handed>)> =
                s.handed.iter().map(|(at, h)| (at.clone(), Some(h.clone()))).collect();
            (wanted, todo)
        };
        for at in wanted.keys() {
            if !todo.iter().any(|(t, _)| t == at) {
                todo.push((at.clone(), None));
            }
        }
        let mut changed = false;
        for (at, had) in todo {
            let want = wanted.get(&at);
            let (pid, had_nodes) = match (&had, want) {
                (Some(h), _) => (h.pid, h.nodes.clone()),
                (None, Some((pid, _))) => (*pid, vec![]),
                (None, None) => continue,
            };
            let want_nodes = want.map(|(_, n)| n.clone()).unwrap_or_default();
            let give: Vec<Node> =
                want_nodes.iter().filter(|n| !had_nodes.iter().any(|h| same_node(h, n))).cloned().collect();
            let take: Vec<Node> =
                had_nodes.iter().filter(|h| !want_nodes.iter().any(|n| same_node(h, n))).cloned().collect();
            if give.is_empty() && take.is_empty() && had.is_some() {
                continue;
            }
            let r = self.apply(pid, &give, &take).await;
            let mut s = self.state.lock().unwrap();
            match r {
                Ok(()) => {
                    s.failed.remove(&at);
                    if want.is_some() {
                        s.handed.insert(at.clone(), Handed { pid, nodes: want_nodes });
                    } else {
                        s.handed.remove(&at);
                    }
                    changed = true;
                }
                Err(e) => {
                    if s.failed.insert(at.clone()) {
                        let (ws, key) = &at;
                        let name = s.known.get(key).map(|k| k.name.clone().unwrap_or(k.own_name.clone()));
                        let text = format!("Couldn't hand {} to {ws}: {e}", name.unwrap_or_else(|| key.clone()));
                        tracing::warn!("devices: {text}");
                        self.bus.publish(Event::Notice { text });
                    }
                }
            }
        }
        if changed {
            self.publish();
        }
    }

    async fn apply(&self, pid: u32, give: &[Node], take: &[Node]) -> Result<(), String> {
        let op = |n: &Node, action: &str| NodeOp {
            devname: n.devname.clone(),
            major: n.major,
            minor: n.minor,
            udev_db: std::fs::read_to_string(self.roots.udev_data.join(format!("c{}:{}", n.major, n.minor))).ok(),
            event: uevent::message(&uevent::props_for(
                &self.roots,
                n,
                action,
                self.seqnum.fetch_add(1, Ordering::Relaxed),
            )),
        };
        let take: Vec<NodeOp> = take.iter().map(|n| op(n, "remove")).collect();
        let give: Vec<NodeOp> = give.iter().map(|n| op(n, "add")).collect();
        let hands = self.hands.clone();
        tokio::task::spawn_blocking(move || {
            if !take.is_empty() {
                hands.take(pid, &take)?;
            }
            if !give.is_empty() {
                hands.give(pid, &give)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
