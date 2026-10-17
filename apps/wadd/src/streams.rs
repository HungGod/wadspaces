//! Streaming wadspaces to browsers: https://ws-<guid>.<domain>/, through the
//! Cloudflare tunnel (tunnel.rs) and wadd's proxy (proxy.rs) to the
//! wadspace's stream sidecar (images/stream: Selkies, its compositor and its
//! web client), which the wadspace draws on instead of the screen.
//!
//! It fails closed:
//! - **a login per stream**: made when it begins (a user and a 20-character
//!   password), asked for by the sidecar's nginx on every path, the page and
//!   the websocket; the sidecar won't start without one. The password is
//!   podman secret `wad_stream_<id>` and nowhere else: not in streams.json,
//!   units, events, logs or the account. GET /v1/streams/{id}/share reads it
//!   back for the person at the machine to hand on with the link.
//! - **a link per stream**: 128 random bits, new each time; the proxy knows
//!   only the streams here, and anything else gets 404.
//! - **it ends**: when it's stopped, when its wadspace stops or is opened at
//!   the machine (the person at the machine wins), or when nobody has
//!   watched it for `idle_minutes`. Its link answers 404 from then on.
//! - **across restarts**: wadd keeps the streams whose containers are still
//!   running and ends the rest; a sidecar it has no stream for is stopped.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wad_proto::v1::{Display, Event, Kind, Phase, StreamInfo, StreamShare};
use wad_proto::{ApiError, ErrorCode};

use crate::backend::Backend;
use crate::events::Bus;
use crate::launches::Launches;
use crate::registry::Registry;
use crate::tunnel::Tunnel;
use crate::view::View;

/// The label wad-core puts on every sidecar, and the one with its stream.
pub const ROLE_LABEL: &str = "wadspaces.role=display";
pub const STREAM_LABEL: &str = "wadspaces.stream";
/// Cloudflare TURN's API token, while WebRTC is on.
pub const TURN_SECRET: &str = "wad_turn_api_token";
/// A stream that never came up is given this long.
const NEVER_UP: f64 = 600.0;

/// A stream's password: its podman secret.
pub fn secret_name(id: &str) -> String {
    format!("wad_stream_{id}")
}

/// `n` random bytes as lowercase hex.
pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the system's randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `len` characters from `alphabet`, uniformly (no modulo bias).
fn random_from(alphabet: &[u8], len: usize) -> String {
    let limit = 256 - 256 % alphabet.len();
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let mut b = [0u8; 64];
        getrandom::fill(&mut b).expect("the system's randomness");
        for x in b.iter().map(|x| *x as usize).filter(|x| *x < limit) {
            if out.len() < len {
                out.push(alphabet[x % alphabet.len()] as char);
            }
        }
    }
    out
}

/// No 0/O, 1/l/I: easy to read out and type on a phone. 56 symbols, so 20
/// of them are about 116 bits.
const PASSWORD_ALPHABET: &[u8] = b"23456789abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

pub fn new_password() -> String {
    random_from(PASSWORD_ALPHABET, 20)
}

pub fn new_user() -> String {
    format!("wad-{}", random_from(b"abcdefghijkmnpqrstuvwxyz", 6))
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Someone watching: open websockets, and when the stream last heard from a
/// viewer (a request, or a byte on a websocket: Selkies' page reports its
/// frame rate every few seconds while it's open, so a phone asleep with the
/// page open goes quiet).
#[derive(Default)]
pub struct Activity {
    open: AtomicU32,
    last: AtomicU64,
}

impl Activity {
    fn new(at: f64) -> Arc<Self> {
        let a = Arc::new(Self::default());
        a.last.store(at as u64, Ordering::Relaxed);
        a
    }
    pub fn touch(&self) {
        self.last.store(now() as u64, Ordering::Relaxed);
    }
    pub fn opened(&self) {
        self.open.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }
    pub fn closed(&self) {
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn viewers(&self) -> u32 {
        self.open.load(Ordering::Relaxed)
    }
    pub fn last(&self) -> f64 {
        self.last.load(Ordering::Relaxed) as f64
    }
}

/// A stream as streams.json keeps it (never its password).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    workspace: String,
    guid: String,
    web_port: u16,
    mux_port: u16,
    user: String,
    started: f64,
}

struct Live {
    entry: Entry,
    activity: Arc<Activity>,
    /// Its sidecar is known to be running with this stream: the proxy
    /// routes to it. (Not yet, for one from before a restart podman hasn't
    /// confirmed.)
    routed: bool,
    /// Its wadspace has been seen running since the stream began.
    was_up: bool,
}

pub struct Streams {
    settings: wad_config::Streaming,
    file: PathBuf,
    registry: Arc<Registry>,
    view: Arc<View>,
    backend: Arc<dyn Backend>,
    bus: Bus,
    pub tunnel: Arc<Tunnel>,
    launches: Arc<Launches>,
    live: Mutex<Vec<Live>>,
    /// One start or end at a time.
    one: tokio::sync::Mutex<()>,
    /// This run of wadd: the proxy's answer to the self-test carries it.
    pub instance: String,
}

impl Streams {
    /// The streams from before (streams.json), drawn on their sidecars from
    /// the registry's first load (before it, so running ones aren't
    /// rewritten under them).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: wad_config::Streaming,
        state_dir: &std::path::Path,
        registry: Arc<Registry>,
        view: Arc<View>,
        backend: Arc<dyn Backend>,
        bus: Bus,
        tunnel: Arc<Tunnel>,
        launches: Arc<Launches>,
        instance: String,
    ) -> Arc<Self> {
        let file = state_dir.join("streams.json");
        let entries: Vec<Entry> =
            std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let me = Arc::new(Self {
            settings,
            file,
            registry,
            view,
            backend,
            bus,
            tunnel,
            launches,
            live: Mutex::new(
                entries
                    .into_iter()
                    .map(|e| Live { activity: Activity::new(now()), entry: e, routed: false, was_up: false })
                    .collect(),
            ),
            one: tokio::sync::Mutex::new(()),
            instance,
        });
        let seeds: HashMap<String, Value> =
            me.live.lock().unwrap().iter().map(|l| (l.entry.workspace.clone(), me.spec(&l.entry))).collect();
        me.registry.seed_streams(seeds);
        let weak = Arc::downgrade(&me);
        me.registry.on_unstream(move |id| {
            if let Some(s) = weak.upgrade() {
                s.forget(id);
            }
        });
        me
    }

    pub fn domain(&self) -> &str {
        &self.settings.domain
    }

    /// The sidecar's part of a workspace's units (wad_core's stream_quadlets).
    fn spec(&self, e: &Entry) -> Value {
        json!({
            "guid": e.guid, "web_port": e.web_port, "mux_port": e.mux_port, "user": e.user,
            "image": self.settings.image, "webrtc": self.settings.webrtc,
            "turn_token_id": if self.settings.webrtc { self.turn().map(|t| t.0).unwrap_or_default() } else { String::new() },
        })
    }

    /// Cloudflare TURN's key ([cloudflare.turn] token_id, api_token).
    fn turn(&self) -> Result<(String, String), String> {
        let path = &self.settings.turn;
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let t: toml::Table = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        let turn = t.get("cloudflare").and_then(|c| c.get("turn"));
        let field = |k: &str| turn.and_then(|t| t.get(k)).and_then(toml::Value::as_str).map(String::from);
        match (field("token_id"), field("api_token")) {
            (Some(id), Some(key)) if !id.is_empty() && !key.is_empty() => Ok((id, key)),
            _ => Err(format!("{}: no [cloudflare.turn] token_id and api_token", path.display())),
        }
    }

    fn save(&self) {
        let entries: Vec<Entry> = self.live.lock().unwrap().iter().map(|l| l.entry.clone()).collect();
        let r = (|| {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let tmp = self.file.with_extension("json.tmp");
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(&entries).expect("json"))?;
            std::fs::rename(&tmp, &self.file)
        })();
        if let Err(e) = r {
            tracing::warn!("{}: {e}", self.file.display());
        }
    }

    fn publish(&self) {
        self.bus.publish(Event::Streams(self.list()));
    }

    fn info(&self, l: &Live) -> StreamInfo {
        let host = format!("ws-{}.{}", l.entry.guid, self.settings.domain);
        StreamInfo {
            workspace: l.entry.workspace.clone(),
            link: format!("https://{host}/"),
            host,
            user: l.entry.user.clone(),
            started: l.entry.started,
            last_active: l.activity.last(),
            viewers: l.activity.viewers(),
            live: l.routed,
        }
    }

    pub fn list(&self) -> Vec<StreamInfo> {
        self.live.lock().unwrap().iter().map(|l| self.info(l)).collect()
    }

    /// Where the proxy sends ws-<guid>: the sidecar's port, and its activity.
    pub fn route(&self, guid: &str) -> Option<(u16, Arc<Activity>)> {
        let live = self.live.lock().unwrap();
        live.iter().find(|l| l.routed && l.entry.guid == guid).map(|l| (l.entry.web_port, l.activity.clone()))
    }

    /// Tests only: as if nobody had watched `id`'s stream for `secs`.
    #[doc(hidden)]
    pub fn unwatched_for_tests(&self, id: &str, secs: u64) {
        if let Some(l) = self.live.lock().unwrap().iter().find(|l| l.entry.workspace == id) {
            l.activity.last.store((now() as u64).saturating_sub(secs), Ordering::Relaxed);
        }
    }

    /// Someone began or stopped watching.
    pub fn viewers_changed(&self) {
        self.publish();
    }

    /// The link and its login, to hand on.
    pub async fn share(&self, id: &str) -> Result<StreamShare, ApiError> {
        let info = self
            .list()
            .into_iter()
            .find(|s| s.workspace == id)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("{id} isn't streamed")))?;
        let password = self
            .backend
            .secret_value(&secret_name(id))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Offline, e))?
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("{id}'s stream has no password")))?;
        Ok(StreamShare { workspace: id.into(), link: info.link, user: info.user, password })
    }

    /// Two free ports from the ranges: the sidecar's web port (127.0.0.1)
    /// and its WebRTC mux port.
    fn ports(&self) -> Result<(u16, u16), ApiError> {
        let used: Vec<u16> =
            self.live.lock().unwrap().iter().flat_map(|l| [l.entry.web_port, l.entry.mux_port]).collect();
        let s = &self.settings;
        let free = |first: u16, udp: bool| {
            (first..first.saturating_add(s.ports)).find(|p| {
                !used.contains(p)
                    && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()
                    && (!udp || std::net::UdpSocket::bind(("0.0.0.0", *p)).is_ok())
            })
        };
        let full = || ApiError::new(ErrorCode::Conflict, "no free port for another stream");
        Ok((free(s.first_web_port, false).ok_or_else(full)?, free(s.first_mux_port, true).ok_or_else(full)?))
    }

    /// Streams a wadspace: a new link and login, its units drawn on a
    /// sidecar, and it started in the background. If it's open on the
    /// screen, `takeover` moves it off (else that's refused).
    pub async fn start(&self, id: &str, takeover: bool) -> Result<StreamShare, ApiError> {
        let _one = self.one.lock().await;
        if !self.settings.enabled {
            return Err(ApiError::new(ErrorCode::Forbidden, "streaming is off on this machine"));
        }
        let ws = self.registry.workspace(id)?;
        if ws.kind != Kind::Container || ws.ephemeral || ws.display != Display::Host {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("{} can't be streamed", ws.name)));
        }
        if self.live.lock().unwrap().iter().any(|l| l.entry.workspace == id) {
            drop(_one);
            return self.share(id).await;
        }
        let sidecar = format!("wad-{id}-display");
        if self.registry.workspaces().iter().any(|w| w.container_name == sidecar) {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("{} can't be streamed: {sidecar} is taken", ws.name),
            ));
        }
        if self.settings.require_tunnel && !self.tunnel.status().await.ready {
            return Err(ApiError::new(
                ErrorCode::Offline,
                "cloud streaming isn't set up on this machine: WadSpaces → Secrets → Cloud Tunnel",
            ));
        }
        let here = |image: &str| {
            let backend = self.backend.clone();
            let image = image.to_string();
            async move { backend.image_exists(&image).await.map_err(|e| ApiError::new(ErrorCode::Offline, e)) }
        };
        if !here(&self.settings.image).await? {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("the stream image ({}) isn't on this machine: update it with --bases", self.settings.image),
            ));
        }
        if !here(&ws.image).await? {
            return Err(ApiError::new(ErrorCode::Conflict, format!("{} isn't built on this machine yet", ws.name)));
        }
        let running = self.registry.state(id).is_some_and(|s| s.container == "running");
        if running && !takeover {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("{} is open on this machine: streaming takes it off the screen", ws.name),
            ));
        }
        let (web_port, mux_port) = self.ports()?;
        let entry =
            Entry { workspace: id.into(), guid: random_hex(16), web_port, mux_port, user: new_user(), started: now() };
        let password = new_password();
        let upstream = |e: String| ApiError::new(ErrorCode::Upstream, e);
        self.backend.create_secret(&secret_name(id), password.as_bytes()).await.map_err(upstream)?;
        if self.settings.webrtc {
            let (_, key) =
                self.turn().map_err(|e| ApiError::new(ErrorCode::Conflict, format!("WebRTC is on, but {e}")))?;
            self.backend.create_secret(TURN_SECRET, key.as_bytes()).await.map_err(upstream)?;
        }
        if running {
            tracing::info!("{id}: off the screen, to be streamed");
            self.view.stop(id).await?;
        }
        let spec = self.spec(&entry);
        let share = StreamShare {
            workspace: id.into(),
            link: format!("https://ws-{}.{}/", entry.guid, self.settings.domain),
            user: entry.user.clone(),
            password,
        };
        self.live.lock().unwrap().push(Live {
            activity: Activity::new(entry.started),
            entry,
            routed: true,
            was_up: false,
        });
        self.save();
        if let Err(e) = self.registry.set_stream(id, Some(spec)).await {
            self.forget(id);
            return Err(ApiError::new(ErrorCode::Internal, e));
        }
        if let Err(e) = self.launches.create_as(id, false, true) {
            let _ = self.registry.unstream(id).await;
            return Err(e);
        }
        tracing::info!("{id}: streamed at {}", share.link);
        self.publish();
        Ok(share)
    }

    /// Ends a stream (its wadspace stops; it's drawn on the screen next time).
    pub async fn stop(&self, id: &str) -> Result<(), ApiError> {
        let _one = self.one.lock().await;
        if !self.live.lock().unwrap().iter().any(|l| l.entry.workspace == id) {
            return Err(ApiError::new(ErrorCode::NotFound, format!("{id} isn't streamed")));
        }
        self.registry.unstream(id).await
    }

    /// Ends this stream, if it's still the one there (a late tick mustn't
    /// end the next one, or a wadspace back on the screen).
    async fn end(&self, id: &str, guid: &str, why: &str) {
        let _one = self.one.lock().await;
        if !self.live.lock().unwrap().iter().any(|l| l.entry.workspace == id && l.entry.guid == guid) {
            return;
        }
        tracing::info!("{id}: its stream ends ({why})");
        if let Err(e) = self.registry.unstream(id).await {
            tracing::warn!("ending {id}'s stream: {}", e.message);
        }
    }

    /// A stream that's over: forgotten, its password with it.
    fn forget(&self, id: &str) {
        let had = {
            let mut live = self.live.lock().unwrap();
            let before = live.len();
            live.retain(|l| l.entry.workspace != id);
            live.len() != before
        };
        if !had {
            return;
        }
        self.save();
        self.publish();
        let backend = self.backend.clone();
        let name = secret_name(id);
        tokio::spawn(async move {
            if let Err(e) = backend.delete_secret(&name).await {
                tracing::warn!("removing {name}: {e}");
            }
        });
    }

    /// At start, before the registry's first load: sidecars wadd has no
    /// stream for are stopped (their units are still there), and the
    /// streams whose sidecar is running are routed again; the rest end.
    /// Podman not answering within a few seconds: the loop checks later.
    pub async fn adopt(&self) {
        // Waited for only when there are streams to keep (a sidecar left
        // without one is stopped by the loop too).
        let tries = if self.live.lock().unwrap().is_empty() { 1 } else { 10 };
        let mut up = false;
        for i in 0..tries {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if self.backend.available().await {
                up = true;
                break;
            }
        }
        if !up {
            if !self.live.lock().unwrap().is_empty() {
                tracing::info!("streams: podman isn't answering yet; checked again later");
            }
            return;
        }
        let sidecars = match self.backend.containers_labelled(ROLE_LABEL).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("streams: listing sidecars: {e}");
                return;
            }
        };
        let label = |c: &crate::backend::ContainerInfo, k: &str| {
            c.labels.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        let mut dead = vec![];
        {
            let mut live = self.live.lock().unwrap();
            for l in live.iter_mut() {
                l.routed = sidecars
                    .iter()
                    .any(|c| label(c, "wadspaces.id") == l.entry.workspace && label(c, STREAM_LABEL) == l.entry.guid);
                if !l.routed {
                    dead.push(l.entry.workspace.clone());
                }
            }
        }
        self.stop_orphans(&sidecars).await;
        // Their sidecars are gone: those streams are over (drawn on the
        // screen from the first load).
        for id in dead {
            tracing::info!("streams: {id}'s stream ended while wadd was away");
            let mut seeds: HashMap<String, Value> = HashMap::new();
            for l in self.live.lock().unwrap().iter().filter(|l| l.entry.workspace != id) {
                seeds.insert(l.entry.workspace.clone(), self.spec(&l.entry));
            }
            self.registry.seed_streams(seeds);
            self.forget(&id);
        }
        let n = self.live.lock().unwrap().len();
        if n > 0 {
            tracing::info!("streams: {n} still running");
        }
        self.publish();
    }

    /// Sidecars running with no stream here (wadd stopped before it could
    /// stop them): stopped. Nothing routes to them, but they don't stay.
    async fn stop_orphans(&self, sidecars: &[crate::backend::ContainerInfo]) {
        for c in sidecars {
            let label = |k: &str| c.labels.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let (id, guid) = (label("wadspaces.id"), label(STREAM_LABEL));
            if self.live.lock().unwrap().iter().any(|l| l.entry.workspace == id && l.entry.guid == guid) {
                continue;
            }
            tracing::warn!("streams: {} has no stream here: stopping it", c.name);
            let unit = format!("{}.service", c.name);
            if self.backend.stop(&unit).await.is_err() {
                let _ = self.backend.stop_container(&c.name).await;
            }
        }
    }

    /// Every few seconds, for as long as wadd runs: streams podman hadn't
    /// confirmed are checked, and streams whose wadspace stopped, or that
    /// nobody watched for `idle_minutes`, end.
    pub async fn run(self: Arc<Self>, every: Duration) {
        loop {
            tokio::time::sleep(every).await;
            self.tick().await;
        }
    }

    pub async fn tick(&self) {
        let idle = self.settings.idle_minutes.max(1) as f64 * 60.0;
        let t = now();
        let mut ends: Vec<(String, String, String)> = vec![];
        if let Ok(sidecars) = self.backend.containers_labelled(ROLE_LABEL).await {
            self.stop_orphans(&sidecars).await;
            let mut live = self.live.lock().unwrap();
            for l in live.iter_mut().filter(|l| !l.routed) {
                l.routed = sidecars.iter().any(|c| {
                    c.labels.get("wadspaces.id").and_then(Value::as_str) == Some(l.entry.workspace.as_str())
                        && c.labels.get(STREAM_LABEL).and_then(Value::as_str) == Some(l.entry.guid.as_str())
                });
                if !l.routed && t - l.entry.started > NEVER_UP {
                    ends.push((l.entry.workspace.clone(), l.entry.guid.clone(), "its sidecar isn't running".into()));
                }
            }
        }
        {
            let mut live = self.live.lock().unwrap();
            for l in live.iter_mut() {
                let st = self.registry.state(&l.entry.workspace);
                let running = st.as_ref().is_some_and(|s| s.container == "running");
                // Stopped by podman's word: "unknown" (it didn't answer) isn't.
                let stopped = st.as_ref().is_some_and(|s| s.container != "running" && s.container != "unknown");
                let busy = st.as_ref().is_some_and(|s| s.phase.busy());
                let failed = st.as_ref().is_some_and(|s| s.phase == Phase::Error);
                l.was_up |= running;
                let (id, guid) = (l.entry.workspace.clone(), l.entry.guid.clone());
                if failed {
                    ends.push((id, guid, "it didn't start".into()));
                } else if stopped && !busy && (l.was_up || t - l.entry.started > NEVER_UP) {
                    ends.push((id, guid, "its wadspace stopped".into()));
                } else if t - l.activity.last() > idle {
                    let mins = self.settings.idle_minutes;
                    ends.push((id, guid, format!("nobody watched it for {mins} minutes")));
                }
            }
        }
        for (id, guid, why) in ends {
            self.end(&id, &guid, &why).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_are_long_random_and_easy_to_type() {
        let p = new_password();
        assert_eq!(p.len(), 20);
        assert!(p.bytes().all(|b| PASSWORD_ALPHABET.contains(&b)));
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        assert_ne!(new_password(), p);
        let u = new_user();
        assert!(u.starts_with("wad-") && u.len() == 10 && u[4..].bytes().all(|b| b.is_ascii_lowercase()));
        let g = random_hex(16);
        assert!(g.len() == 32 && g.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(PASSWORD_ALPHABET.len(), 56);
    }

    #[test]
    fn every_symbol_comes_up() {
        let s = random_from(b"abc", 3000);
        for c in ['a', 'b', 'c'] {
            let n = s.matches(c).count();
            assert!((800..1200).contains(&n), "{c}: {n}");
        }
    }
}
