//! wadd's configuration, layered:
//!
//! - **system** (a WadSpaces machine): `/usr/lib/wadspaces/wadd.toml` from
//!   the image, then `/etc/wadspaces/wadd.toml` if the admin has one. wadd
//!   never writes either (what it changes at run time is state, under
//!   `state_dir`), so an image update can't be held back by an edited copy.
//! - **user** (`wadd serve --user`, a laptop): the same vendor file if there
//!   is one, then `~/.config/wadspaces/wadd.toml`; rootless podman, paths under
//!   your home and runtime dir.
//!
//! Later files override earlier ones key by key (tables merge). Unknown keys
//! are ignored, so an older wadd reads a newer file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const VENDOR: &str = "/usr/lib/wadspaces/wadd.toml";
pub const ADMIN: &str = "/etc/wadspaces/wadd.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// A WadSpaces machine: root, rootful podman, system paths.
    System,
    /// A laptop or desktop: your user, rootless podman, your paths.
    User,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub machine: Machine,
    pub daemon: Daemon,
    pub log: Log,
    pub keys: Keys,
    pub display: Display,
    pub cloud: Cloud,
    pub streaming: Streaming,
    pub tailnet: Tailnet,
}

/// The account link (the cloud relay). An empty project_id: no account to
/// link to.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Cloud {
    pub enabled: bool,
    pub project_id: String,
    /// Where enrollMachine runs.
    pub functions_region: String,
    /// The web API key (linking hands one over too).
    pub api_key: String,
    pub heartbeat_s: u64,
    /// How often pending commands are looked for.
    pub poll_s: u64,
    /// Use the Firebase emulators on this host (firebase.json's ports)
    /// instead of Google's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emulator: Option<String>,
}

impl Default for Cloud {
    fn default() -> Self {
        Self {
            enabled: true,
            project_id: String::new(),
            functions_region: "australia-southeast2".into(),
            api_key: String::new(),
            heartbeat_s: 30,
            poll_s: 3,
            emulator: None,
        }
    }
}

/// Streaming wadspaces to a browser: each streamed wadspace draws on its
/// Selkies sidecar, wadd's proxy maps https://ws-<guid>.<domain> to that
/// sidecar, and the Cloudflare tunnel (cloudflared, its token at
/// /etc/cloudflared/token) carries it from the internet to the proxy.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Streaming {
    pub enabled: bool,
    /// A stream is https://ws-<guid>.<domain>/ (the tunnel's wildcard).
    pub domain: String,
    /// Where the proxy listens: the tunnel's origin, on loopback only (both
    /// families: `localhost` may resolve to ::1 first). One that can't be
    /// bound is skipped.
    pub listen: Vec<String>,
    /// The sidecar (images/stream).
    pub image: String,
    /// Each stream's sidecar web port (on 127.0.0.1) and WebRTC mux port
    /// come from these ranges: `ports` ports from each first one.
    pub first_web_port: u16,
    pub first_mux_port: u16,
    pub ports: u16,
    /// A stream nobody has watched or touched for this long is stopped.
    pub idle_minutes: u64,
    /// WebRTC beside WebSockets (STUN and Cloudflare's TURN): off, so a
    /// stream is WebSockets end to end.
    pub webrtc: bool,
    /// The TURN key ([cloudflare.turn] token_id, api_token), read only when
    /// `webrtc` is on.
    pub turn: PathBuf,
    /// cloudflared's metrics server (its /ready says whether it's connected).
    pub metrics: String,
    /// Stream only when the tunnel is up (the trials turn this off).
    pub require_tunnel: bool,
}

impl Default for Streaming {
    fn default() -> Self {
        Self {
            enabled: true,
            domain: "wadspaces.com".into(),
            listen: vec!["127.0.0.1:8085".into(), "[::1]:8085".into()],
            image: "localhost/wadspaces-stream:trixie".into(),
            first_web_port: 48100,
            first_mux_port: 48200,
            ports: 100,
            idle_minutes: 30,
            webrtc: false,
            turn: "/usr/lib/wadspaces/secrets/cloudflare.toml".into(),
            metrics: "127.0.0.1:20241".into(),
            require_tunnel: true,
        }
    }
}

/// The owner's tailnet (Tailscale): the machine joins it, and the devices
/// the owner trusts reach wadd's remote API over it (host/TAILNET.md).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Tailnet {
    pub enabled: bool,
    /// tailscaled's LocalAPI.
    pub socket: PathBuf,
    /// The `tailscale` CLI (joining, leaving, the name).
    pub cli: PathBuf,
    /// The remote API's port, on the machine's tailnet addresses.
    pub port: u16,
    /// Listen here instead (the laptop trial: tailscaled in userspace mode
    /// forwards the tailnet's connections to loopback). Empty: the machine's
    /// tailnet addresses, with `port`.
    pub listen: Vec<String>,
    /// The interface the remote API is bound to, so nothing from another
    /// network reaches it. Empty: none.
    pub bind_device: String,
    /// The machine's name on the tailnet. Empty: from [machine] name.
    pub hostname: String,
    /// How often tailscaled's status is read.
    pub poll_s: u64,
}

impl Tailnet {
    pub fn for_profile(profile: Profile) -> Self {
        Self {
            // A laptop has its own Tailscale, if any: the trial turns it on.
            enabled: profile == Profile::System,
            socket: "/run/tailscale/tailscaled.sock".into(),
            cli: "/usr/bin/tailscale".into(),
            port: 8087,
            listen: vec![],
            bind_device: "tailscale0".into(),
            hostname: String::new(),
            poll_s: 10,
        }
    }
}

impl Default for Tailnet {
    fn default() -> Self {
        Self::for_profile(Profile::System)
    }
}

/// The keyboard proxy: wadd grabs the keyboards and keeps Super for itself
/// (Super+Tab switcher, Super+1..9 workspaces, Super+`home` WadSpaces Client).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Keys {
    /// Off on a laptop: your keyboard stays yours.
    pub enabled: bool,
    /// False: only watch for chords, filter nothing.
    pub grab: bool,
    /// With Super: back to WadSpaces Client.
    /// (The Python wadd called it `launcher`; that's read too.)
    pub home: Vec<String>,
    /// With Super: the clipboard history (the HUD shows it).
    pub clipboard: Vec<String>,
    /// Chords dropped before they reach a workspace (modifiers must match exactly).
    pub block: Vec<String>,
    /// Also send other Super chords on to the workspace.
    pub pass_super: bool,
}

/// Where the screen is: the kiosk session's sway.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Display {
    pub enabled: bool,
    /// Where sway's IPC socket is looked for (the newest live one).
    pub runtime_dir: PathBuf,
    /// A fixed socket instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
}

impl Keys {
    pub fn for_profile(profile: Profile) -> Self {
        Self {
            enabled: profile == Profile::System,
            grab: true,
            home: vec!["KEY_0".into(), "KEY_SPACE".into()],
            clipboard: vec!["KEY_V".into()],
            block: vec!["alt+f4".into(), "ctrl+shift+q".into(), "ctrl+alt+backspace".into()],
            pass_super: false,
        }
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self::for_profile(Profile::System)
    }
}

impl Display {
    pub fn for_profile(profile: Profile) -> Self {
        Self {
            enabled: true,
            // The kiosk user (uid 1000, host/etc/sysusers.d); on a laptop, yours.
            runtime_dir: match profile {
                Profile::System => "/run/user/1000".into(),
                Profile::User => runtime_dir(),
            },
            socket: None,
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Self::for_profile(Profile::System)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Machine {
    /// What the machine is called (the kiosk and the account show it).
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Daemon {
    /// The API's Unix socket (when systemd doesn't hand one over).
    pub socket: PathBuf,
    /// Where wadd keeps its state (enrollment, projects, runs, ...).
    pub state_dir: PathBuf,
    /// Who may use the API, besides root and wadd's own user: these users...
    pub allow_users: Vec<String>,
    /// ...and members of these groups.
    pub allow_groups: Vec<String>,
    /// The image's workspaces, one `<id>.toml` each: applied to the
    /// machine's list when they're new or the image changes them.
    pub vendor_workspaces: PathBuf,
    /// Podman's API socket.
    pub podman_socket: PathBuf,
    /// Where wadd writes the workspaces' quadlet units (rewritten at every
    /// start, so they're never stale; quadlet reads this directory).
    pub units_dir: PathBuf,
    /// Project folders, one per project.
    pub projects_dir: PathBuf,
    /// Saved sign-ins, one folder each, mounted into the wadspaces that use them.
    pub signins_dir: PathBuf,
    /// How long a workspace may take to come up.
    pub ready_timeout_s: u64,
    /// How often wadd checks the workspaces against podman.
    pub reconcile_s: u64,
    /// Who owns project folders: the user inside the images (abc). git runs
    /// as them when wadd is root.
    pub projects_uid: u32,
    /// Folder projects and the folder browser stay inside these.
    pub folder_roots: Vec<PathBuf>,
    /// A build won't start with less free disk than this.
    pub build_min_free_gb: u64,
}

/// Home folders, and where drives get mounted.
pub const FOLDER_ROOTS: [&str; 6] = ["/var/home", "/home", "/mnt", "/media", "/run/media", "/run/wadspaces-drives"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Log {
    /// `tracing` filter, e.g. "info" or "wadd=debug,info".
    pub level: String,
    /// Lines kept in memory for /v1/logs.
    pub buffer_lines: usize,
}

impl Default for Machine {
    fn default() -> Self {
        Self { name: hostname().unwrap_or_else(|| "wadspaces".into()) }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self { level: "info".into(), buffer_lines: 2000 }
    }
}

impl Default for Daemon {
    fn default() -> Self {
        Self::for_profile(Profile::System)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::defaults(Profile::System)
    }
}

impl Daemon {
    pub fn for_profile(profile: Profile) -> Self {
        match profile {
            Profile::System => Self {
                socket: "/run/wadd/wadd.sock".into(),
                state_dir: "/var/lib/wadspaces".into(),
                // The kiosk user, and anyone an admin adds to group wad.
                allow_users: vec!["wad".into()],
                allow_groups: vec!["wad".into(), "wheel".into()],
                vendor_workspaces: "/usr/lib/wadspaces/workspaces.d".into(),
                podman_socket: "/run/podman/podman.sock".into(),
                units_dir: "/run/containers/systemd".into(),
                projects_dir: "/var/lib/wadspaces-projects".into(),
                signins_dir: "/var/lib/wadspaces-signins".into(),
                ready_timeout_s: 900,
                reconcile_s: 5,
                projects_uid: 1000,
                folder_roots: FOLDER_ROOTS.iter().map(PathBuf::from).collect(),
                build_min_free_gb: 8,
            },
            Profile::User => Self {
                socket: runtime_dir().join("wadd/wadd.sock"),
                state_dir: home().join(".local/state/wadspaces"),
                allow_users: vec![],
                allow_groups: vec![],
                vendor_workspaces: "/usr/lib/wadspaces/workspaces.d".into(),
                podman_socket: runtime_dir().join("podman/podman.sock"),
                units_dir: runtime_dir().join("containers/systemd"),
                projects_dir: home().join(".local/share/wadspaces-projects"),
                signins_dir: home().join(".local/share/wadspaces-signins"),
                ready_timeout_s: 900,
                reconcile_s: 5,
                projects_uid: 1000,
                folder_roots: FOLDER_ROOTS.iter().map(PathBuf::from).collect(),
                build_min_free_gb: 8,
            },
        }
    }
}

impl Config {
    pub fn defaults(profile: Profile) -> Self {
        Self {
            machine: Machine::default(),
            daemon: Daemon::for_profile(profile),
            log: Log::default(),
            keys: Keys::for_profile(profile),
            display: Display::for_profile(profile),
            cloud: Cloud::default(),
            streaming: Streaming::default(),
            tailnet: Tailnet::for_profile(profile),
        }
    }

    /// The files read for a profile, in order (missing ones are skipped).
    pub fn files(profile: Profile) -> Vec<PathBuf> {
        match profile {
            Profile::System => vec![VENDOR.into(), ADMIN.into()],
            Profile::User => vec![VENDOR.into(), config_home().join("wadspaces/wadd.toml")],
        }
    }

    /// The profile's defaults, then each file that exists, merged in order.
    pub fn load(profile: Profile, files: &[PathBuf]) -> Result<Self, Error> {
        let mut merged = toml::Table::new();
        for f in files {
            match std::fs::read_to_string(f) {
                Ok(text) => {
                    let t: toml::Table = text.parse().map_err(|e| Error::Parse(f.clone(), Box::new(e)))?;
                    merge(&mut merged, t);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::Read(f.clone(), e)),
            }
        }
        Self::from_table(profile, merged).map_err(|e| Error::Invalid(Box::new(e)))
    }

    /// Defaults overlaid with a (merged) table.
    pub fn from_table(profile: Profile, mut table: toml::Table) -> Result<Self, toml::de::Error> {
        if let Some(toml::Value::Table(keys)) = table.get_mut("keys")
            && let Some(old) = keys.remove("launcher")
        {
            keys.entry("home").or_insert(old);
        }
        let defaults = Self::defaults(profile);
        let mut base = toml::Table::try_from(Raw::from(&defaults)).expect("defaults serialize");
        merge(&mut base, table);
        base.try_into()
    }
}

/// `over` into `base`: tables merge, anything else replaces.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// The defaults as a table to merge onto (Config itself only deserializes).
#[derive(serde::Serialize)]
struct Raw {
    machine: RawMachine,
    daemon: RawDaemon,
    log: RawLog,
    keys: Keys,
    display: Display,
    cloud: Cloud,
    streaming: Streaming,
    tailnet: Tailnet,
}
#[derive(serde::Serialize)]
struct RawMachine {
    name: String,
}
#[derive(serde::Serialize)]
struct RawDaemon {
    socket: PathBuf,
    state_dir: PathBuf,
    allow_users: Vec<String>,
    allow_groups: Vec<String>,
    podman_socket: PathBuf,
    units_dir: PathBuf,
    projects_dir: PathBuf,
    signins_dir: PathBuf,
    ready_timeout_s: u64,
    reconcile_s: u64,
    projects_uid: u32,
    folder_roots: Vec<PathBuf>,
    build_min_free_gb: u64,
}
#[derive(serde::Serialize)]
struct RawLog {
    level: String,
    buffer_lines: usize,
}

impl From<&Config> for Raw {
    fn from(c: &Config) -> Self {
        Self {
            machine: RawMachine { name: c.machine.name.clone() },
            daemon: RawDaemon {
                socket: c.daemon.socket.clone(),
                state_dir: c.daemon.state_dir.clone(),
                allow_users: c.daemon.allow_users.clone(),
                allow_groups: c.daemon.allow_groups.clone(),
                podman_socket: c.daemon.podman_socket.clone(),
                units_dir: c.daemon.units_dir.clone(),
                projects_dir: c.daemon.projects_dir.clone(),
                signins_dir: c.daemon.signins_dir.clone(),
                ready_timeout_s: c.daemon.ready_timeout_s,
                reconcile_s: c.daemon.reconcile_s,
                projects_uid: c.daemon.projects_uid,
                folder_roots: c.daemon.folder_roots.clone(),
                build_min_free_gb: c.daemon.build_min_free_gb,
            },
            log: RawLog { level: c.log.level.clone(), buffer_lines: c.log.buffer_lines },
            keys: c.keys.clone(),
            display: c.display.clone(),
            cloud: c.cloud.clone(),
            streaming: c.streaming.clone(),
            tailnet: c.tailnet.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}: {1}")]
    Read(PathBuf, #[source] std::io::Error),
    #[error("{0}: {1}")]
    Parse(PathBuf, #[source] Box<toml::de::Error>),
    #[error("configuration: {0}")]
    Invalid(#[source] Box<toml::de::Error>),
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| "/".into())
}

fn runtime_dir() -> PathBuf {
    env_path("XDG_RUNTIME_DIR").unwrap_or_else(std::env::temp_dir)
}

fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

/// The configured hostname, else the kernel's (a transient one has no file).
fn hostname() -> Option<String> {
    ["/etc/hostname", "/proc/sys/kernel/hostname"]
        .iter()
        .find_map(|f| std::fs::read_to_string(f).ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The image's own file (host/usr/lib/wadspaces/wadd.toml).
    #[test]
    fn the_images_settings() {
        let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../host/usr/lib/wadspaces/wadd.toml");
        let c = Config::load(Profile::System, &[f]).unwrap();
        assert_eq!(c.machine.name, "wadspaces");
        assert_eq!(c.daemon.socket, Path::new("/run/wadd/wadd.sock"));
        assert_eq!(c.daemon.vendor_workspaces, Path::new("/usr/lib/wadspaces/workspaces.d"));
        assert_eq!(
            (c.cloud.project_id.as_str(), c.cloud.functions_region.as_str()),
            ("wad-spaces", "australia-southeast2")
        );
        assert!(c.keys.enabled && c.keys.home == ["KEY_0", "KEY_SPACE"]);
        // Streams are WebSockets only until WebRTC is turned on here.
        assert!(c.streaming.enabled && !c.streaming.webrtc && c.streaming.require_tunnel);
        assert_eq!(c.streaming.domain, "wadspaces.com");
        assert_eq!(c.streaming.listen, ["127.0.0.1:8085", "[::1]:8085"]);
        // The remote API: on the tailnet's interface only.
        assert!(c.tailnet.enabled && c.tailnet.listen.is_empty());
        assert_eq!((c.tailnet.port, c.tailnet.bind_device.as_str()), (8087, "tailscale0"));
        assert_eq!(c.tailnet.socket, Path::new("/run/tailscale/tailscaled.sock"));
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn defaults_per_profile() {
        let c = Config::load(Profile::System, &[]).unwrap();
        assert_eq!(c.daemon.socket, PathBuf::from("/run/wadd/wadd.sock"));
        assert_eq!(c.daemon.allow_groups, ["wad", "wheel"]);
        let u = Config::load(Profile::User, &[]).unwrap();
        assert!(u.daemon.socket.ends_with("wadd/wadd.sock"));
        assert!(u.daemon.allow_users.is_empty());
        assert!(c.tailnet.enabled && !u.tailnet.enabled);
    }

    #[test]
    fn later_files_override_key_by_key_and_unknown_keys_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let vendor = write(
            d.path(),
            "vendor.toml",
            "[machine]\nname = \"Surface\"\n[log]\nlevel = \"debug\"\nbuffer_lines = 10\n[future]\nthing = 1\n",
        );
        let admin = write(d.path(), "admin.toml", "[log]\nlevel = \"warn\"\n[daemon]\nnew_key = true\n");
        let missing = d.path().join("nope.toml");
        let c = Config::load(Profile::System, &[vendor, admin, missing]).unwrap();
        assert_eq!(c.machine.name, "Surface");
        assert_eq!(c.log.level, "warn");
        assert_eq!(c.log.buffer_lines, 10);
        assert_eq!(c.daemon.state_dir, PathBuf::from("/var/lib/wadspaces"));
        assert!(c.keys.enabled && c.keys.home == ["KEY_0", "KEY_SPACE"]);
        assert!(!Config::load(Profile::User, &[]).unwrap().keys.enabled);
        let old = write(d.path(), "old.toml", "[keys]\nlauncher = [\"KEY_HOME\"]\n[display]\nsocket = \"/tmp/s\"\n");
        let c = Config::load(Profile::System, &[old]).unwrap();
        assert_eq!((c.keys.home, c.keys.grab), (vec!["KEY_HOME".to_string()], true));
        assert_eq!(c.display.socket, Some(PathBuf::from("/tmp/s")));
        let s = write(d.path(), "streams.toml", "[streaming]\nwebrtc = true\nidle_minutes = 5\n");
        let c = Config::load(Profile::System, &[s]).unwrap();
        assert!(c.streaming.webrtc && c.streaming.enabled);
        assert_eq!((c.streaming.idle_minutes, c.streaming.first_web_port), (5, 48100));
        let bad = write(d.path(), "soon.toml", "[daemon]\nready_timeout_s = \"soon\"\n");
        assert!(Config::load(Profile::System, &[bad]).is_err());
    }

    #[test]
    fn bad_files_say_which() {
        let d = tempfile::tempdir().unwrap();
        let bad = write(d.path(), "bad.toml", "[log\n");
        let e = Config::load(Profile::System, std::slice::from_ref(&bad)).unwrap_err();
        assert!(e.to_string().starts_with(&bad.display().to_string()), "{e}");
        let wrong = write(d.path(), "wrong.toml", "[log]\nbuffer_lines = \"many\"\n");
        assert!(matches!(Config::load(Profile::System, &[wrong]), Err(Error::Invalid(_))));
    }
}
