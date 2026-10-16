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
//!
//! A drive isn't nodes but files: each of its filesystems is mounted on the
//! machine (drives.rs, at /run/wadspaces-drives/<uuid>) and bound into
//! `<attached>/<wadspace>/<name>`, which every native wadspace has at
//! ~/Drives with the machine's mounts followed (rslave): there whether the
//! wadspace runs or not, so it's there as it starts. Ejecting takes it out
//! of every wadspace (refused while something there has files open),
//! unmounts it and turns the disk off: safe to unplug.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wad_proto::v1::{BluetoothDevice, Device, DeviceBus, DeviceKind, Display, Event, Kind};
use wad_proto::{ApiError, ErrorCode};

use super::attach::{BUSY, Hands, NodeOp};
use super::sysfs::{self, Found, Node, Roots};
use super::uevent;
use crate::backend::Backend;
use crate::drives::{DRIVES_DIR, Drives};
use crate::events::Bus;
use crate::registry::Registry;

const FILE: &str = "devices.json";
/// Checks between events (a wadspace that started or stopped, an event lost).
const TICK: Duration = Duration::from_secs(3);
/// udev sends several events for one plug (the device, then its parts).
const SETTLE: Duration = Duration::from_millis(300);
/// Where wadspaces' drives are bound, a folder per wadspace (its ~/Drives).
pub const ATTACHED: &str = "/run/wadspaces-drives/attached";

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

/// A drive's filesystem bound into a wadspace's ~/Drives.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    /// Where it's mounted on the machine.
    src: String,
    dst: PathBuf,
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
    /// Paired Bluetooth devices (BlueZ's), connected or not.
    bluetooth: Vec<BluetoothDevice>,
    /// (wadspace, drive) → its filesystems bound into the wadspace.
    bound: BTreeMap<(String, String), Vec<Bound>>,
    /// Drives ejected and still plugged in: safe to unplug.
    ejected: BTreeSet<String>,
}

impl State {
    /// What a device is: a drive stays one once it's ejected (its disk is
    /// gone; the USB device it was on is still there).
    fn kind(&self, key: &str) -> Option<DeviceKind> {
        if self.ejected.contains(key) {
            return Some(DeviceKind::Storage);
        }
        self.present.get(key).map(|f| f.kind).or(self.known.get(key).and_then(|k| k.kind))
    }
}

pub struct Devices {
    roots: Roots,
    hands: Arc<dyn Hands>,
    drives: Arc<Drives>,
    /// ATTACHED (a tempdir in tests).
    attached: PathBuf,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state_dir: &std::path::Path,
        roots: Roots,
        hands: Arc<dyn Hands>,
        drives: Arc<Drives>,
        attached: PathBuf,
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
            drives,
            attached,
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
        // An ejected drive unplugged: a drive like any other next time.
        let gone: Vec<String> = s.ejected.iter().filter(|k| !found.contains_key(*k)).cloned().collect();
        for k in &gone {
            s.ejected.remove(k);
        }
        let ejected = s.ejected.clone();
        for f in found.values() {
            let k = s.known.entry(f.key.clone()).or_default();
            let before = k.clone();
            k.own_name = f.name.clone();
            if !ejected.contains(&f.key) {
                k.kind = Some(f.kind);
            }
            k.bus = Some(f.bus);
            changed |= *k != before;
        }
        changed |= !gone.is_empty();
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

    /// The paired Bluetooth devices, from BlueZ (the Bluetooth service).
    pub fn set_bluetooth(&self, devices: &[BluetoothDevice]) {
        let paired: Vec<BluetoothDevice> = devices.iter().filter(|d| d.paired).cloned().collect();
        let changed = {
            let mut s = self.state.lock().unwrap();
            let changed = s.bluetooth != paired;
            s.bluetooth = paired;
            changed
        };
        if changed {
            self.publish();
        }
    }

    /// Every device: plugged in (Bluetooth: paired) now, or remembered
    /// from before.
    pub fn list(&self) -> Vec<Device> {
        let s = self.state.lock().unwrap();
        let mut keys: BTreeSet<String> = s.known.keys().cloned().collect();
        keys.extend(s.bluetooth.iter().map(|b| format!("bt:{}", b.address.to_uppercase())));
        let mut out: Vec<Device> = keys
            .iter()
            .filter_map(|key| {
                let none = Known::default();
                let k = s.known.get(key).unwrap_or(&none);
                let f = s.present.get(key);
                let bt = key
                    .strip_prefix("bt:")
                    .and_then(|a| s.bluetooth.iter().find(|b| b.address.eq_ignore_ascii_case(a)));
                let kind = s.kind(key).or(bt.map(|b| b.kind))?;
                let ejected = s.ejected.contains(key);
                // A Bluetooth device is handed out only for nodes it has
                // (a pad's); the rest work through the machine.
                let attachable =
                    sysfs::attachable(kind) && (f.is_some() || bt.is_none() || kind == DeviceKind::Gamepad);
                let present = f.is_some() || bt.is_some_and(|b| b.connected);
                let own = f.map(|f| f.name.clone()).or(bt.map(|b| b.name.clone())).unwrap_or(k.own_name.clone());
                Some(Device {
                    key: key.clone(),
                    name: k.name.clone().unwrap_or(own),
                    bus: f.map(|f| f.bus).or(k.bus).unwrap_or(if bt.is_some() {
                        DeviceBus::Bluetooth
                    } else {
                        DeviceBus::Usb
                    }),
                    kind,
                    attachable,
                    present,
                    attached: s
                        .handed
                        .keys()
                        .chain(s.bound.keys())
                        .filter(|(_, d)| d == key)
                        .map(|(w, _)| w.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    remembered: k.remembered.clone(),
                    new: attachable && f.is_some() && !k.asked && !ejected,
                    ejected,
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
            // A paired Bluetooth device wadd hasn't seen plugged in yet.
            if !s.known.contains_key(key)
                && let Some(b) =
                    key.strip_prefix("bt:").and_then(|a| s.bluetooth.iter().find(|b| b.address.eq_ignore_ascii_case(a)))
            {
                let k = Known {
                    own_name: b.name.clone(),
                    kind: Some(b.kind),
                    bus: Some(DeviceBus::Bluetooth),
                    ..Known::default()
                };
                s.known.insert(key.to_string(), k);
            }
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
        let bt = key.strip_prefix("bt:").and_then(|a| s.bluetooth.iter().find(|b| b.address.eq_ignore_ascii_case(a)));
        let kind = s.kind(key).or(bt.map(|b| b.kind));
        let bt_only = bt.is_some() && !s.present.contains_key(key) && kind != Some(DeviceKind::Gamepad);
        if kind.is_some_and(|k| !sysfs::attachable(k)) || bt_only {
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
        let (wanted, mut todo) =
            {
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
                    // Drives go in as files (reconcile_drives), not as nodes.
                    let Some(f) = s.present.get(key).filter(|f| {
                        sysfs::attachable(f.kind) && f.kind != DeviceKind::Storage && !s.ejected.contains(key)
                    }) else {
                        continue;
                    };
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
        changed |= self.reconcile_drives(&list).await;
        if changed {
            self.publish();
        }
    }

    /// A wadspace's ~/Drives on the machine.
    fn drives_dir(&self, ws: &str) -> PathBuf {
        self.attached.join(ws)
    }

    /// Before a native wadspace starts: its ~/Drives folder on the machine
    /// (its unit mounts it), with the drives it has bound in already.
    pub fn prepare(&self, ws: &str) -> Result<(), String> {
        if !self.available {
            return Ok(());
        }
        self.hands.make_dir(&self.drives_dir(ws))
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn Hands) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let hands = self.hands.clone();
        tokio::task::spawn_blocking(move || f(hands.as_ref())).await.map_err(|e| e.to_string())?
    }

    fn notice_once(&self, at: &(String, String), text: String) {
        if self.state.lock().unwrap().failed.insert(at.clone()) {
            tracing::warn!("devices: {text}");
            self.bus.publish(Event::Notice { text });
        }
    }

    fn name_of(&self, key: &str) -> String {
        let s = self.state.lock().unwrap();
        s.known.get(key).map_or(key.to_string(), |k| k.name.clone().unwrap_or(k.own_name.clone()))
    }

    /// The rule for drives: each one bound into every native wadspace it
    /// remembers while it's plugged in (and not ejected), running or not.
    /// Whether anything changed.
    async fn reconcile_drives(&self, list: &[wad_proto::v1::Workspace]) -> bool {
        let (wanted, have) = {
            let s = self.state.lock().unwrap();
            let native =
                |ws: &str| list.iter().any(|w| w.id == ws && w.kind == Kind::Container && w.display == Display::Host);
            let mut wanted: BTreeSet<(String, String)> = BTreeSet::new();
            for (key, k) in &s.known {
                if s.ejected.contains(key) || s.present.get(key).is_none_or(|f| f.kind != DeviceKind::Storage) {
                    continue;
                }
                for ws in k.remembered.iter().filter(|w| native(w)) {
                    wanted.insert((ws.clone(), key.clone()));
                }
            }
            (wanted, s.bound.clone())
        };
        let mut changed = false;
        // Taken out: no longer wanted, or the drive's gone (then detached:
        // its files are gone with it).
        for (at, binds) in have.iter().filter(|(at, _)| !wanted.contains(*at)) {
            let force = !self.state.lock().unwrap().present.contains_key(&at.1);
            let dsts: Vec<PathBuf> = binds.iter().map(|b| b.dst.clone()).collect();
            let r = self.blocking(move |h| dsts.iter().try_for_each(|d| h.unbind(d, force))).await;
            match r {
                Ok(()) => {
                    let mut s = self.state.lock().unwrap();
                    s.bound.remove(at);
                    s.failed.remove(at);
                    changed = true;
                }
                Err(e) => {
                    let text = format!("Couldn't take {} out of {}: {e}", self.name_of(&at.1), at.0);
                    self.notice_once(at, text);
                }
            }
        }
        let to_bind: Vec<&(String, String)> = wanted.iter().filter(|at| !have.contains_key(*at)).collect();
        if to_bind.is_empty() {
            return changed;
        }
        let all = match self.drives.list().await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("devices: drives: {e}");
                return changed;
            }
        };
        for at in to_bind {
            let parts = self.filesystems_of(&at.1, &all);
            let mut binds: Vec<Bound> = vec![];
            let mut err = None;
            for d in &parts {
                let src = match self.drives.mount(&d.uuid, &d.label, &d.fstype).await {
                    Ok(mp) => mp,
                    Err(e) => {
                        err = Some(e.to_string());
                        break;
                    }
                };
                let name = folder_name(d, &binds, &self.name_of(&at.1));
                let dst = self.drives_dir(&at.0).join(&name);
                let (s2, d2) = (PathBuf::from(&src), dst.clone());
                match self.blocking(move |h| h.bind(&s2, &d2)).await {
                    Ok(()) => binds.push(Bound { src, dst }),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = err {
                // What got in comes out again; tried again later.
                let dsts: Vec<PathBuf> = binds.iter().map(|b| b.dst.clone()).collect();
                let _ = self.blocking(move |h| dsts.iter().try_for_each(|d| h.unbind(d, true))).await;
                let text = format!("Couldn't put {} in {}: {e}", self.name_of(&at.1), at.0);
                self.notice_once(at, text);
                continue;
            }
            let mut s = self.state.lock().unwrap();
            s.failed.remove(at);
            s.bound.insert(at.clone(), binds);
            changed = true;
        }
        changed
    }

    /// A drive's filesystems (the partitions lsblk lists on its disks).
    fn filesystems_of(&self, key: &str, all: &[wad_proto::v1::Drive]) -> Vec<wad_proto::v1::Drive> {
        let disks: BTreeSet<String> = {
            let s = self.state.lock().unwrap();
            s.present
                .get(key)
                .map(|f| {
                    f.nodes
                        .iter()
                        .filter(|n| n.subsystem == "block")
                        .filter_map(|n| n.devname.rsplit('/').next().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        all.iter().filter(|d| d.disk.as_ref().is_some_and(|k| disks.contains(k))).cloned().collect()
    }

    /// Takes a drive out of every wadspace, unmounts it and turns its disk
    /// off: then it's safe to unplug. Refused while something in a wadspace
    /// has its files open (or one runs with a folder on it as a project).
    pub async fn eject(&self, key: &str) -> Result<Device, ApiError> {
        if !self.available {
            return Err(bad("this machine can't eject drives here (wadd isn't running as root)"));
        }
        let busy = |msg: String| ApiError::new(ErrorCode::Conflict, msg);
        let name = self.name_of(key);
        let _one = self.one_at_a_time.lock().await;
        {
            let s = self.state.lock().unwrap();
            if s.present.get(key).is_none_or(|f| f.kind != DeviceKind::Storage) || s.ejected.contains(key) {
                return Err(bad(format!("{name} isn't a drive that's plugged in")));
            }
        }
        let all = self.drives.list().await.map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
        let parts = self.filesystems_of(key, &all);
        let mounted: Vec<String> = parts.iter().filter_map(|d| d.mountpoint.clone()).collect();
        // A running wadspace with a folder on it mounted (a project).
        for w in self.registry.workspaces() {
            let running = self.registry.state(&w.id).is_some_and(|s| s.container == "running");
            let uses =
                self.registry.mounts_of(&w.id).values().any(|p| mounted.iter().any(|m| Path::new(p).starts_with(m)));
            if running && uses {
                return Err(busy(format!("{} has a folder on {name} as a project: stop it first", w.name)));
            }
        }
        // Out of every wadspace first (so none re-binds it meanwhile).
        let binds: Vec<((String, String), Vec<Bound>)> = {
            let mut s = self.state.lock().unwrap();
            s.ejected.insert(key.to_string());
            s.bound.iter().filter(|((_, k), _)| k == key).map(|(a, b)| (a.clone(), b.clone())).collect()
        };
        for (at, b) in binds {
            let dsts: Vec<PathBuf> = b.iter().map(|x| x.dst.clone()).collect();
            if let Err(e) = self.blocking(move |h| dsts.iter().try_for_each(|d| h.unbind(d, false))).await {
                self.state.lock().unwrap().ejected.remove(key);
                self.poke();
                let ws = self.registry.workspace(&at.0).map(|w| w.name).unwrap_or(at.0.clone());
                return Err(if e == BUSY {
                    busy(format!("{name} is in use in {ws}: close what has its files open, then eject it again"))
                } else {
                    ApiError::new(ErrorCode::Internal, e)
                });
            }
            self.state.lock().unwrap().bound.remove(&at);
        }
        // Then off the machine: its own mounts (drives.rs's), and the disk.
        for m in &mounted {
            if !m.starts_with(DRIVES_DIR) {
                self.state.lock().unwrap().ejected.remove(key);
                return Err(busy(format!("{name} is mounted at {m}, not by WadSpaces: unmount it there first")));
            }
            let at = PathBuf::from(m);
            if let Err(e) = self.blocking(move |h| h.unbind(&at, false)).await {
                self.state.lock().unwrap().ejected.remove(key);
                self.poke();
                return Err(busy(format!("{name}: {e}")));
            }
        }
        let disks: BTreeSet<String> = parts.iter().filter_map(|d| d.disk.clone()).collect();
        for d in disks {
            self.blocking(move |h| h.power_off(&d)).await.map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
        }
        tracing::info!("devices: ejected {name}");
        self.publish();
        self.list()
            .into_iter()
            .find(|d| d.key == key)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such device"))
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

/// A drive's folder in ~/Drives: its filesystem's label, else the
/// device's name, made safe and unique among what's bound already.
fn folder_name(d: &wad_proto::v1::Drive, taken: &[Bound], device: &str) -> String {
    let base = [d.label.as_str(), device, d.uuid.as_str()]
        .into_iter()
        .map(|s| {
            s.chars()
                .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '-' })
                .collect::<String>()
                .trim()
                .trim_matches('.')
                .to_string()
        })
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "Drive".into());
    let used = |n: &str| taken.iter().any(|b| b.dst.file_name().is_some_and(|f| f == n));
    let mut name = base.clone();
    let mut i = 2;
    while used(&name) {
        name = format!("{base} {i}");
        i += 1;
    }
    name
}
