//! The machine on its owner's tailnet (Tailscale; host/TAILNET.md).
//!
//! [`Tailnet`] reads tailscaled's state every `poll_s` (every second while
//! joining) and publishes it as the "tailnet" event; joins and leaves through
//! the `tailscale` CLI; and keeps which of the owner's devices may use the
//! remote API (remote.rs): the ones trusted here (`tailnet-devices.json`),
//! and asks from the others, which the machine shows (the HUD, the WadSpaces
//! Client's Tailnet tab) until they're allowed or put off.
//!
//! Only the machine's own user's untagged devices may even ask:
//! [`Tailnet::identify`] refuses everything else outright.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};
use wad_proto::v1::{Event, TailnetAsk, TailnetDevice, TailnetStatus};
use wad_proto::{ApiError, ErrorCode};
use wad_tailscale::{Cli, LocalApi, Login, Status, WhoIs};

use crate::events::Bus;

/// How long a join waits to be approved.
const LOGIN_WAIT: Duration = Duration::from_secs(600);
/// How long a device's whois answer is kept (per address and port).
const WHOIS_FOR: Duration = Duration::from_secs(30);
/// "Not now": that device's asks are put off for this long.
const QUIET: Duration = Duration::from_secs(600);
/// Asks kept at once (the oldest go).
const MAX_ASKS: usize = 8;

/// A device the remote API is talking to: the owner's, and trusted or not.
#[derive(Debug, Clone)]
pub struct Caller {
    /// Its stable node id.
    pub id: String,
    pub name: String,
    pub os: String,
    pub trusted: bool,
}

/// Why a connection to the remote API was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The machine isn't on the tailnet (or tailscaled isn't answering).
    Down,
    /// Not a device on the tailnet.
    NotOnTailnet,
    /// A tagged device: a server or a service, not a person's.
    Tagged,
    /// Someone else's device, or one shared in from another tailnet.
    NotOwners,
    /// The machine itself.
    ThisMachine,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::Down => "This machine isn't on the tailnet right now.",
            Refusal::NotOnTailnet => "Only devices on the owner's tailnet can use this.",
            Refusal::Tagged => "Tagged devices can't manage this machine.",
            Refusal::NotOwners => "Only the owner's own devices can manage this machine.",
            Refusal::ThisMachine => "That's this machine.",
        }
    }
}

/// tailnet-devices.json: whose tailnet the trust is for, and the devices.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Book {
    /// The machine's Tailscale user and tailnet when they were trusted (0,
    /// "": not known yet).
    #[serde(default)]
    user: i64,
    #[serde(default)]
    tailnet: String,
    #[serde(default)]
    devices: Vec<Trusted>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Trusted {
    id: String,
    name: String,
    #[serde(default)]
    os: String,
    trusted_at: u64,
}

/// A join waiting to be approved.
struct Joining {
    url: Option<String>,
    cancel: Arc<Notify>,
}

#[derive(Default)]
struct Inner {
    status: Option<Status>,
    /// The last read of tailscaled failed.
    unreachable: bool,
    joining: Option<Joining>,
    error: Option<String>,
    book: Book,
    asks: Vec<TailnetAsk>,
    quiet: HashMap<String, Instant>,
    used: HashMap<String, u64>,
    whois: HashMap<SocketAddr, (Instant, Option<WhoIs>)>,
    listening: bool,
    published: Option<TailnetStatus>,
}

pub struct Tailnet {
    settings: wad_config::Tailnet,
    api: LocalApi,
    cli: Cli,
    hostname: String,
    file: PathBuf,
    bus: Bus,
    inner: Mutex<Inner>,
    addrs: watch::Sender<Vec<SocketAddr>>,
    wake: Notify,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A name Tailscale takes for the machine: letters, digits and dashes.
fn host_label(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(63).collect();
    if out.is_empty() { "wadspaces".into() } else { out.trim_end_matches('-').to_string() }
}

impl Tailnet {
    pub fn new(settings: &wad_config::Tailnet, machine_name: &str, state_dir: &Path, bus: Bus) -> Arc<Self> {
        let file = state_dir.join("tailnet-devices.json");
        let book = match std::fs::read(&file) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!("{}: {e}", file.display());
                Book::default()
            }),
            Err(_) => Book::default(),
        };
        let hostname =
            if settings.hostname.is_empty() { host_label(machine_name) } else { host_label(&settings.hostname) };
        Arc::new(Self {
            api: LocalApi::new(&settings.socket),
            cli: Cli::new(&settings.cli, &settings.socket),
            settings: settings.clone(),
            hostname,
            file,
            bus,
            inner: Mutex::new(Inner { book, ..Default::default() }),
            addrs: watch::channel(vec![]).0,
            wake: Notify::new(),
        })
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// The interface the remote API is bound to ("": none).
    pub fn bind_device(&self) -> &str {
        &self.settings.bind_device
    }

    /// Where the remote API should listen now (it rebinds when this changes).
    pub fn addrs(&self) -> watch::Receiver<Vec<SocketAddr>> {
        self.addrs.subscribe()
    }

    /// The remote API is listening (or stopped).
    pub fn set_listening(&self, on: bool) {
        let changed = {
            let mut i = self.inner.lock().unwrap();
            std::mem::replace(&mut i.listening, on) != on
        };
        if changed {
            self.publish();
        }
    }

    /// Reads tailscaled now, until wadd stops: every `poll_s`, every second
    /// while joining, and at once when something changed here.
    pub async fn run(self: Arc<Self>) {
        if !self.enabled() {
            return;
        }
        loop {
            self.refresh().await;
            let every = if self.inner.lock().unwrap().joining.is_some() {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(self.settings.poll_s.max(1))
            };
            tokio::select! {
                _ = tokio::time::sleep(every) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    /// tailscaled's state read again (and published if it changed).
    pub async fn refresh(&self) {
        if !self.enabled() {
            return;
        }
        let read = self.api.status().await;
        {
            let mut i = self.inner.lock().unwrap();
            match read {
                Ok(s) => {
                    i.unreachable = false;
                    self.check_owner(&mut i, &s);
                    i.status = Some(s);
                }
                Err(e) => {
                    if !i.unreachable {
                        tracing::warn!("tailnet: {e}");
                    }
                    i.unreachable = true;
                    i.status = None;
                }
            }
        }
        let addrs = self.listen_addrs();
        self.addrs.send_if_modified(|now| {
            if *now == addrs {
                return false;
            }
            *now = addrs;
            true
        });
        self.publish();
    }

    /// The trust is for one person's tailnet: logged in as someone else, or
    /// in another tailnet, the machine starts again with none.
    fn check_owner(&self, i: &mut Inner, s: &Status) {
        let (Some(me), true) = (s.me.as_ref(), s.running()) else { return };
        let tailnet = s.tailnet.as_ref().map(|t| t.name.clone()).unwrap_or_default();
        let b = &mut i.book;
        if b.user == me.user_id && b.tailnet == tailnet {
            return;
        }
        if b.user != 0 && !b.devices.is_empty() {
            tracing::warn!("tailnet: the machine's Tailscale login changed; no device is trusted now");
            i.asks.clear();
        }
        *b = Book { user: me.user_id, tailnet, devices: vec![] };
        self.save(b);
    }

    fn save(&self, b: &Book) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = self.file.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            if let Some(d) = self.file.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(b).expect("json"))?;
            std::fs::rename(&tmp, &self.file)
        };
        if let Err(e) = write() {
            tracing::warn!("tailnet: {}: {e}", self.file.display());
        }
    }

    fn listen_addrs(&self) -> Vec<SocketAddr> {
        if !self.settings.listen.is_empty() {
            return self
                .settings
                .listen
                .iter()
                .filter_map(|a| a.parse().inspect_err(|e| tracing::warn!("[tailnet] listen {a:?}: {e}")).ok())
                .collect();
        }
        let i = self.inner.lock().unwrap();
        match &i.status {
            Some(s) if s.running() => s.ips().into_iter().map(|ip| SocketAddr::new(ip, self.settings.port)).collect(),
            _ => vec![],
        }
    }

    fn state(&self, i: &Inner) -> &'static str {
        let Some(s) = i.status.as_ref().filter(|_| self.enabled() && !i.unreachable) else { return "off" };
        match s.backend_state.as_str() {
            "Running" => "running",
            "NeedsMachineAuth" => "needsApproval",
            "Stopped" if i.joining.is_none() => "stopped",
            "Starting" | "NoState" if i.joining.is_none() => "starting",
            _ if i.joining.is_some() => "loggingIn",
            _ => "needsLogin",
        }
    }

    /// GET /v1/tailnet.
    pub fn view(&self) -> TailnetStatus {
        let i = self.inner.lock().unwrap();
        self.view_of(&i)
    }

    fn view_of(&self, i: &Inner) -> TailnetStatus {
        let state = self.state(i);
        let mut v = TailnetStatus {
            available: state != "off",
            state: state.into(),
            error: i.error.clone(),
            port: self.settings.port,
            listening: i.listening,
            ..Default::default()
        };
        let trusted = |id: &str| i.book.devices.iter().find(|d| d.id == id);
        if let Some(s) = i.status.as_ref().filter(|_| v.available) {
            v.login_url = i
                .joining
                .as_ref()
                .and_then(|j| j.url.clone())
                .or_else(|| Some(s.auth_url.clone()).filter(|u| !u.is_empty()))
                .filter(|_| !s.running());
            v.ips = s.tailscale_ips.clone();
            v.tailnet = s.tailnet.as_ref().map(|t| t.name.clone()).unwrap_or_default();
            v.user = s.me_user().map(|u| u.login_name.clone()).unwrap_or_default();
            if let Some(me) = &s.me {
                v.hostname = me.host_name.clone();
                v.dns_name = me.dns().to_string();
                v.key_expiry = me.key_expiry.clone();
                if s.running() {
                    for p in s.peer_list() {
                        if !p.tags.is_empty() || p.user_id != me.user_id || p.sharee_node {
                            continue;
                        }
                        let t = trusted(&p.id);
                        v.devices.push(TailnetDevice {
                            id: p.id.clone(),
                            name: p.host_name.clone(),
                            os: p.os.clone(),
                            online: p.online,
                            trusted: t.is_some(),
                            trusted_at: t.map(|t| t.trusted_at),
                            last_used: i.used.get(&p.id).copied(),
                        });
                    }
                }
            }
        }
        // Trusted devices that aren't on the tailnet now, to remove.
        for t in &i.book.devices {
            if !v.devices.iter().any(|d| d.id == t.id) {
                v.devices.push(TailnetDevice {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    os: t.os.clone(),
                    online: false,
                    trusted: true,
                    trusted_at: Some(t.trusted_at),
                    last_used: i.used.get(&t.id).copied(),
                });
            }
        }
        v.asks = i.asks.iter().filter(|a| trusted(&a.id).is_none()).cloned().collect();
        v
    }

    fn publish(&self) {
        let v = {
            let mut i = self.inner.lock().unwrap();
            let v = self.view_of(&i);
            if i.published.as_ref() == Some(&v) {
                return;
            }
            i.published = Some(v.clone());
            v
        };
        self.bus.publish(Event::Tailnet(v));
    }

    /// What the account's heartbeat says: where the owner's devices find
    /// the remote API. None when the tailnet is off here.
    pub fn heartbeat_value(&self) -> Option<Value> {
        if !self.enabled() {
            return None;
        }
        let i = self.inner.lock().unwrap();
        let v = self.view_of(&i);
        let me = i.status.as_ref().and_then(|s| s.me.as_ref()).map(|m| m.id.clone()).unwrap_or_default();
        Some(json!({
            "state": v.state, "ips": v.ips, "dnsName": v.dns_name, "stableId": me,
            "port": v.port, "listening": v.listening,
        }))
    }

    // ------------------------------------------------------------ joining

    /// Join: `tailscale up`, waiting (up to 10 minutes) for the machine to be
    /// approved at the login link, which is in the answer once tailscaled
    /// has it (a few seconds).
    pub async fn login(self: &Arc<Self>) -> Result<TailnetStatus, ApiError> {
        if !self.enabled() {
            return Err(ApiError::new(ErrorCode::Offline, "Tailscale is turned off on this machine."));
        }
        self.refresh().await;
        {
            let i = self.inner.lock().unwrap();
            if i.status.is_none() {
                return Err(ApiError::new(ErrorCode::Offline, "Tailscale isn't running on this machine."));
            }
            if i.status.as_ref().is_some_and(Status::running) {
                return Err(ApiError::new(ErrorCode::Conflict, "This machine is on the tailnet already."));
            }
            if i.joining.is_some() {
                return Ok(self.view_of(&i));
            }
        }
        let login = self
            .cli
            .up(Some(&self.hostname), LOGIN_WAIT)
            .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
        let cancel = Arc::new(Notify::new());
        {
            let mut i = self.inner.lock().unwrap();
            i.joining = Some(Joining { url: None, cancel: cancel.clone() });
            i.error = None;
        }
        tracing::info!("tailnet: joining as {}", self.hostname);
        tokio::spawn(self.clone().follow(login, cancel));
        // The link, once there is one (or the machine is already approved).
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            self.refresh().await;
            let v = self.view();
            if v.login_url.is_some() || v.state != "loggingIn" {
                return Ok(v);
            }
        }
        Ok(self.view())
    }

    /// The join's `tailscale up`, until it's done: its link, then how it went.
    async fn follow(self: Arc<Self>, mut login: Login, cancel: Arc<Notify>) {
        let cancelled = loop {
            if let Some(u) = login.url() {
                let mut i = self.inner.lock().unwrap();
                if let Some(j) = i.joining.as_mut().filter(|j| j.url.is_none()) {
                    j.url = Some(u);
                }
            }
            if !login.running() {
                break false;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = cancel.notified() => break true,
            }
        };
        let out = if cancelled {
            login.cancel().await;
            Ok(())
        } else {
            login.wait().await
        };
        {
            let mut i = self.inner.lock().unwrap();
            i.joining = None;
            if let Err(e) = out {
                tracing::warn!("tailnet: joining: {e}");
                i.error = Some(e.to_string());
            }
        }
        self.refresh().await;
        self.wake.notify_one();
    }

    /// Leave the tailnet (a join waiting to be approved is given up).
    pub async fn logout(&self) -> Result<TailnetStatus, ApiError> {
        if !self.enabled() {
            return Err(ApiError::new(ErrorCode::Offline, "Tailscale is turned off on this machine."));
        }
        if let Some(j) = self.inner.lock().unwrap().joining.take() {
            j.cancel.notify_one();
        }
        let r = self.cli.logout().await;
        {
            let mut i = self.inner.lock().unwrap();
            i.asks.clear();
            i.whois.clear();
            i.error = r.as_ref().err().map(|e| e.to_string());
        }
        self.refresh().await;
        r.map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
        tracing::info!("tailnet: left");
        Ok(self.view())
    }

    // -------------------------------------------------------------- trust

    /// Trust one of the owner's devices (one on the tailnet now, or one
    /// that asked).
    pub fn trust(&self, id: &str, name: Option<String>) -> Result<TailnetStatus, ApiError> {
        let mut i = self.inner.lock().unwrap();
        if i.book.devices.iter().any(|d| d.id == id) {
            return Ok(self.view_of(&i));
        }
        let found = self
            .view_of(&i)
            .devices
            .into_iter()
            .find(|d| d.id == id)
            .map(|d| (d.name, d.os))
            .or_else(|| i.asks.iter().find(|a| a.id == id).map(|a| (a.name.clone(), a.os.clone())));
        let Some((found, os)) = found else {
            return Err(ApiError::new(ErrorCode::NotFound, "That isn't one of your devices on the tailnet."));
        };
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or(found);
        tracing::info!("tailnet: trusted {name} ({id})");
        i.book.devices.push(Trusted { id: id.into(), name, os, trusted_at: now_ms() });
        i.asks.retain(|a| a.id != id);
        i.quiet.remove(id);
        self.save(&i.book);
        drop(i);
        self.publish();
        Ok(self.view())
    }

    /// It can't manage the machine any more.
    pub fn untrust(&self, id: &str) -> Result<TailnetStatus, ApiError> {
        let mut i = self.inner.lock().unwrap();
        let before = i.book.devices.len();
        i.book.devices.retain(|d| d.id != id);
        if i.book.devices.len() == before {
            return Err(ApiError::new(ErrorCode::NotFound, "That device isn't trusted here."));
        }
        tracing::info!("tailnet: no longer trusts {id}");
        self.save(&i.book);
        drop(i);
        self.publish();
        Ok(self.view())
    }

    /// "Not now": its ask goes, and it can't ask again for a while.
    pub fn dismiss(&self, id: &str) -> Result<TailnetStatus, ApiError> {
        {
            let mut i = self.inner.lock().unwrap();
            i.asks.retain(|a| a.id != id);
            i.quiet.insert(id.into(), Instant::now() + QUIET);
        }
        self.publish();
        Ok(self.view())
    }

    // ----------------------------------------------------------- callers

    async fn whois(&self, peer: SocketAddr) -> Result<Option<WhoIs>, Refusal> {
        // A loopback port is tailscaled's (userspace mode) for one connection
        // at a time: never kept.
        let keep = !peer.ip().is_loopback();
        if keep {
            let mut i = self.inner.lock().unwrap();
            i.whois.retain(|_, (at, _)| at.elapsed() < WHOIS_FOR);
            if let Some((_, w)) = i.whois.get(&peer) {
                return Ok(w.clone());
            }
        }
        let w = self.api.whois(peer).await.map_err(|e| {
            tracing::warn!("tailnet: whois {peer}: {e}");
            Refusal::Down
        })?;
        if keep {
            self.inner.lock().unwrap().whois.insert(peer, (Instant::now(), w.clone()));
        }
        Ok(w)
    }

    /// Who a connection from `peer` is: one of the owner's devices (trusted
    /// or not, in which case it asks), or why it's refused.
    pub async fn identify(&self, peer: SocketAddr) -> Result<Caller, Refusal> {
        let (me, user) = {
            let i = self.inner.lock().unwrap();
            match i.status.as_ref().filter(|s| s.running()).and_then(|s| s.me.as_ref()) {
                Some(m) => (m.id.clone(), m.user_id),
                None => return Err(Refusal::Down),
            }
        };
        let Some(w) = self.whois(peer).await? else { return Err(Refusal::NotOnTailnet) };
        if !w.node.tags.is_empty() {
            return Err(Refusal::Tagged);
        }
        if w.node.stable_id == me {
            return Err(Refusal::ThisMachine);
        }
        if w.node.user != user || w.node.sharer != 0 {
            return Err(Refusal::NotOwners);
        }
        let c = Caller {
            id: w.node.stable_id.clone(),
            name: w.device_name(),
            os: w.node.hostinfo.os.clone(),
            trusted: false,
        };
        let mut i = self.inner.lock().unwrap();
        if i.book.devices.iter().any(|d| d.id == c.id) {
            i.used.insert(c.id.clone(), now_ms());
            return Ok(Caller { trusted: true, ..c });
        }
        let quiet = i.quiet.get(&c.id).is_some_and(|until| Instant::now() < *until);
        if !quiet && !i.asks.iter().any(|a| a.id == c.id) {
            tracing::info!("tailnet: {} ({}) asks to manage this machine", c.name, c.id);
            i.asks.push(TailnetAsk { id: c.id.clone(), name: c.name.clone(), os: c.os.clone(), at: now_ms() });
            if i.asks.len() > MAX_ASKS {
                i.asks.remove(0);
            }
            drop(i);
            self.publish();
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::host_label;

    #[test]
    fn names_tailscale_takes() {
        assert_eq!(host_label("Surface Book 2"), "surface-book-2");
        assert_eq!(host_label("  wad's  machine!! "), "wad-s-machine");
        assert_eq!(host_label("!!!"), "wadspaces");
        assert_eq!(host_label(&"x".repeat(80)).len(), 63);
    }
}
