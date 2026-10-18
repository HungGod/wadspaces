//! tailscaled, for wadd: the machine on its owner's tailnet.
//!
//! [`LocalApi`] reads tailscaled's LocalAPI over its Unix socket
//! (`/run/tailscale/tailscaled.sock`): [`LocalApi::status`] (the machine's
//! state, its addresses, the devices on the tailnet and whose they are) and
//! [`LocalApi::whois`] (which device a connection came from). [`Cli`] changes
//! things through the `tailscale` CLI, as a person would: joining
//! ([`Cli::up`], which hands back the login link), leaving, the name.
//! Tailscale's own CLI sets its settings whole, where a hand-made LocalAPI
//! call could leave some at zero.
//!
//! The types read what wadd uses and ignore the rest; Go writes empty lists
//! and maps as `null`, which read as empty. The LocalAPI isn't a stable
//! interface: the image pins Tailscale's version, and `fixtures/tailscale`
//! has the shapes these read.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Deserializer};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Where tailscaled listens on a machine.
pub const SOCKET: &str = "/run/tailscale/tailscaled.sock";
/// The Host the LocalAPI wants.
const HOST: &str = "local-tailscaled.sock";
/// How long a LocalAPI call may take.
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tailscaled isn't answering: {0}")]
    Unreachable(String),
    #[error("tailscaled answered {status}: {message}")]
    Refused { status: u16, message: String },
    #[error("tailscaled's answer didn't read: {0}")]
    BadAnswer(String),
    /// The `tailscale` CLI failed or couldn't run.
    #[error("{0}")]
    Cli(String),
}

/// A `null` (Go's empty list or map) reads as empty.
fn nullable<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// GET /localapi/v0/status (`ipnstate.Status`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Status {
    #[serde(rename = "Version")]
    pub version: String,
    /// NoState, NeedsLogin, NeedsMachineAuth, Stopped, Starting or Running.
    #[serde(rename = "BackendState")]
    pub backend_state: String,
    /// While logging in: the link to approve the machine at.
    #[serde(rename = "AuthURL")]
    pub auth_url: String,
    #[serde(rename = "TailscaleIPs", deserialize_with = "nullable")]
    pub tailscale_ips: Vec<String>,
    #[serde(rename = "Self")]
    pub me: Option<Peer>,
    #[serde(rename = "Health", deserialize_with = "nullable")]
    pub health: Vec<String>,
    #[serde(rename = "CurrentTailnet")]
    pub tailnet: Option<Tailnet>,
    /// The other devices, by node key.
    #[serde(rename = "Peer", deserialize_with = "nullable")]
    pub peers: HashMap<String, Peer>,
    /// The people the devices are owned by, by user id.
    #[serde(rename = "User", deserialize_with = "nullable")]
    pub users: HashMap<String, UserProfile>,
}

/// A device in the status (`ipnstate.PeerStatus`): the machine itself, or another.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Peer {
    /// Its stable node id (what wadd trusts a device by).
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "HostName")]
    pub host_name: String,
    #[serde(rename = "DNSName")]
    pub dns_name: String,
    #[serde(rename = "OS")]
    pub os: String,
    #[serde(rename = "UserID")]
    pub user_id: i64,
    #[serde(rename = "TailscaleIPs", deserialize_with = "nullable")]
    pub tailscale_ips: Vec<String>,
    #[serde(rename = "Tags", deserialize_with = "nullable")]
    pub tags: Vec<String>,
    #[serde(rename = "Online")]
    pub online: bool,
    /// RFC 3339 ("0001-01-01T00:00:00Z": never).
    #[serde(rename = "LastSeen")]
    pub last_seen: String,
    /// It's here because it was shared with this tailnet's user.
    #[serde(rename = "ShareeNode")]
    pub sharee_node: bool,
    #[serde(rename = "Expired")]
    pub expired: bool,
    /// When its key runs out (RFC 3339); None when it never does.
    #[serde(rename = "KeyExpiry")]
    pub key_expiry: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tailnet {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "MagicDNSSuffix")]
    pub magic_dns_suffix: String,
    #[serde(rename = "MagicDNSEnabled")]
    pub magic_dns_enabled: bool,
}

/// A person (`tailcfg.UserProfile`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserProfile {
    #[serde(rename = "ID")]
    pub id: i64,
    #[serde(rename = "LoginName")]
    pub login_name: String,
    #[serde(rename = "DisplayName")]
    pub display_name: String,
}

/// GET /localapi/v0/whois (`apitype.WhoIsResponse`): the device a connection
/// came from, and whose it is.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WhoIs {
    #[serde(rename = "Node")]
    pub node: Node,
    #[serde(rename = "UserProfile")]
    pub user: UserProfile,
}

/// A device as the control plane knows it (`tailcfg.Node`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Node {
    #[serde(rename = "StableID")]
    pub stable_id: String,
    /// Its MagicDNS name, with a dot at the end.
    #[serde(rename = "Name")]
    pub name: String,
    /// Whose it is (for a tagged device, the tags' own "user").
    #[serde(rename = "User")]
    pub user: i64,
    /// Who shared it in, from another tailnet (0: nobody).
    #[serde(rename = "Sharer")]
    pub sharer: i64,
    #[serde(rename = "Tags", deserialize_with = "nullable")]
    pub tags: Vec<String>,
    #[serde(rename = "Hostinfo")]
    pub hostinfo: Hostinfo,
    #[serde(rename = "ComputedName")]
    pub computed_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Hostinfo {
    #[serde(rename = "OS")]
    pub os: String,
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "DeviceModel")]
    pub device_model: String,
}

impl Status {
    /// Joined and up.
    pub fn running(&self) -> bool {
        self.backend_state == "Running"
    }

    /// The machine's tailnet addresses.
    pub fn ips(&self) -> Vec<IpAddr> {
        self.tailscale_ips.iter().filter_map(|a| a.parse().ok()).collect()
    }

    /// The person the machine is logged in as.
    pub fn me_user(&self) -> Option<&UserProfile> {
        self.users.get(&self.me.as_ref()?.user_id.to_string())
    }

    /// The other devices, by name.
    pub fn peer_list(&self) -> Vec<&Peer> {
        let mut v: Vec<&Peer> = self.peers.values().collect();
        v.sort_by(|a, b| a.host_name.to_lowercase().cmp(&b.host_name.to_lowercase()).then(a.id.cmp(&b.id)));
        v
    }
}

impl Peer {
    /// Its MagicDNS name without the dot at the end.
    pub fn dns(&self) -> &str {
        self.dns_name.trim_end_matches('.')
    }
}

impl WhoIs {
    /// What to call the device: its own name for itself, else its MagicDNS
    /// name's first label.
    pub fn device_name(&self) -> String {
        let h = &self.node.hostinfo.hostname;
        if !h.is_empty() {
            return h.clone();
        }
        if !self.node.computed_name.is_empty() {
            return self.node.computed_name.clone();
        }
        self.node.name.split('.').next().unwrap_or_default().to_string()
    }
}

/// tailscaled's LocalAPI.
#[derive(Clone)]
pub struct LocalApi {
    http: reqwest::Client,
    sock: PathBuf,
}

impl LocalApi {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        let sock = sock.into();
        let http = reqwest::Client::builder().unix_socket(sock.clone()).timeout(TIMEOUT).build().expect("a client");
        Self { http, sock }
    }

    pub fn socket(&self) -> &Path {
        &self.sock
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response, Error> {
        let res = self
            .http
            .get(format!("http://{HOST}/localapi/v0/{path}"))
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Unreachable(e.to_string()))?;
        Ok(res)
    }

    async fn read<T: for<'de> Deserialize<'de>>(res: reqwest::Response) -> Result<T, Error> {
        let status = res.status().as_u16();
        let body = res.bytes().await.map_err(|e| Error::Unreachable(e.to_string()))?;
        if !(200..300).contains(&status) {
            let message = String::from_utf8_lossy(&body).trim().to_string();
            return Err(Error::Refused { status, message });
        }
        serde_json::from_slice(&body).map_err(|e| Error::BadAnswer(e.to_string()))
    }

    pub async fn status(&self) -> Result<Status, Error> {
        Self::read(self.get("status", &[]).await?).await
    }

    /// Which device the TCP connection from `addr` is from; None when it isn't
    /// one on the tailnet. (With tailscaled in userspace mode, a connection it
    /// forwarded arrives from a loopback port, which tailscaled maps back.)
    pub async fn whois(&self, addr: SocketAddr) -> Result<Option<WhoIs>, Error> {
        let res = self.get("whois", &[("addr", &addr.to_string()), ("proto", "tcp")]).await?;
        if res.status().as_u16() == 404 {
            return Ok(None);
        }
        Self::read(res).await.map(Some)
    }
}

/// The `tailscale` CLI, pointed at one tailscaled.
#[derive(Debug, Clone)]
pub struct Cli {
    bin: PathBuf,
    sock: PathBuf,
}

/// A `tailscale up` that's waiting for the machine to be approved. Its
/// output is kept, for the login link when tailscaled's status hasn't got it.
pub struct Login {
    child: tokio::process::Child,
    out: Arc<Mutex<String>>,
}

impl Cli {
    pub fn new(bin: impl Into<PathBuf>, sock: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into(), sock: sock.into() }
    }

    fn command(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(&self.bin);
        c.arg(format!("--socket={}", self.sock.display())).stdin(Stdio::null()).kill_on_drop(true);
        c
    }

    /// Join (or log in again): `tailscale up`, with every setting at its
    /// default but the name (`--reset`) and the machine's DNS left alone
    /// (wadd and the devices use addresses, not MagicDNS names). It waits for
    /// approval for up to `wait`; [`Login::url`] (or the status's
    /// `AuthURL`) has the link to approve it at.
    pub fn up(&self, hostname: Option<&str>, wait: Duration) -> Result<Login, Error> {
        let mut c = self.command();
        c.args(["up", "--reset", "--accept-dns=false"]).arg(format!("--timeout={}s", wait.as_secs()));
        if let Some(h) = hostname.filter(|h| !h.is_empty()) {
            c.arg(format!("--hostname={h}"));
        }
        let mut child = c
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Cli(format!("tailscale didn't run: {e}")))?;
        let out = Arc::new(Mutex::new(String::new()));
        if let Some(o) = child.stdout.take() {
            tokio::spawn(collect(o, out.clone()));
        }
        if let Some(e) = child.stderr.take() {
            tokio::spawn(collect(e, out.clone()));
        }
        Ok(Login { child, out })
    }

    /// Leave: the machine's key is let go of, and it's off the tailnet.
    pub async fn logout(&self) -> Result<(), Error> {
        self.run(&["logout"]).await
    }

    /// Its name on the tailnet.
    pub async fn set_hostname(&self, hostname: &str) -> Result<(), Error> {
        self.run(&["set", &format!("--hostname={hostname}")]).await
    }

    async fn run(&self, args: &[&str]) -> Result<(), Error> {
        let out = tokio::time::timeout(Duration::from_secs(30), self.command().args(args).output())
            .await
            .map_err(|_| Error::Cli(format!("tailscale {} took too long", args[0])))?
            .map_err(|e| Error::Cli(format!("tailscale didn't run: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&out.stderr);
        let text = text.trim();
        Err(Error::Cli(if text.is_empty() { format!("tailscale {} failed", args[0]) } else { text.to_string() }))
    }
}

async fn collect(from: impl AsyncRead + Unpin, into: Arc<Mutex<String>>) {
    let mut lines = BufReader::new(from).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut o = into.lock().unwrap();
        o.push_str(&line);
        o.push('\n');
    }
}

impl Login {
    /// The login link it printed ("To authenticate, visit: <url>").
    pub fn url(&self) -> Option<String> {
        url_in(&self.out.lock().unwrap())
    }

    /// What it printed.
    pub fn output(&self) -> String {
        self.out.lock().unwrap().clone()
    }

    /// Still waiting.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for it: Ok once the machine is up, or its complaint.
    pub async fn wait(mut self) -> Result<(), Error> {
        let status = self.child.wait().await.map_err(|e| Error::Cli(e.to_string()))?;
        // Its last lines have been read once it's gone.
        tokio::time::sleep(Duration::from_millis(50)).await;
        if status.success() {
            return Ok(());
        }
        let out = self.output();
        let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("tailscale up failed");
        Err(Error::Cli(last.trim().to_string()))
    }

    /// Give up waiting.
    pub async fn cancel(mut self) {
        let _ = self.child.kill().await;
    }
}

/// The first https link in `text`.
pub fn url_in(text: &str) -> Option<String> {
    text.split_whitespace().find(|w| w.starts_with("https://") && w.len() > "https://".len()).map(String::from)
}

#[cfg(test)]
mod tests;
