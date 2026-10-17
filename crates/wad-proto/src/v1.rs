//! wadd's `/v1` API (the Rust wadd, over /run/wadd/wadd.sock): the types its
//! endpoints and events carry. More arrive with each milestone.

use serde::{Deserialize, Serialize};

/// GET /v1/health
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

/// How wadd runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Profile {
    /// A WadSpaces machine.
    System,
    /// A laptop (`wadd serve --user`).
    User,
}

/// GET /v1/machine
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInfo {
    pub name: String,
    pub version: String,
    pub profile: Profile,
    /// When this wadd started (Unix seconds).
    pub started_at: u64,
    /// The screen's logical size (what the Builder lays a desktop out at);
    /// None while there's no compositor to ask.
    #[serde(default)]
    pub screen: Option<Screen>,
}

/// A screen's logical size, and its scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

/// GET /v1/logs: one line of wadd's own log (redacted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Unix milliseconds.
    pub time: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// GET /v1/events: server-sent events, named by `type`, with `data` as JSON.
/// The stream starts with the current state of each kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Machine(MachineInfo),
    /// A workspace's state changed.
    WorkspaceState(WorkspaceState),
    /// What's on screen changed (or what's about to be).
    View(ViewState),
    /// The Super+Tab switcher opened, moved or closed.
    Carousel(Carousel),
    /// A session began, its clock started, its time ran out, or it ended.
    Session(Option<Session>),
    /// Projects were saved or deleted here.
    Projects {
        ids: Vec<String>,
    },
    /// Saved sign-ins were made, renamed, signed out or deleted here.
    SignIns {
        ids: Vec<String>,
    },
    /// A launch moved on: the job, and its log lines since the last event.
    Launch(LaunchLog),
    /// A build moved on: the job, and its log lines since the last event.
    Build(BuildLog),
    /// The account link changed (linked, unlinked, a heartbeat, an error).
    Cloud(CloudLink),
    /// The network changed (joined, left, connectivity).
    Network(NetworkStatus),
    /// The workspace list changed (added, edited, removed, built).
    Workspaces(Vec<Workspace>),
    /// Something to tell whoever is watching.
    Notice {
        text: String,
    },
    /// A Super+key the HUD answers rather than wadd ("clipboard": Super+V,
    /// the clipboard history). Never replayed to a new watcher.
    Shortcut {
        name: String,
    },
    /// A new count of the disk (GET /v1/storage has it).
    Storage {
        #[serde(rename = "measuredAt")]
        measured_at: f64,
    },
    /// The devices changed: plugged in or out, handed to a wadspace or
    /// taken back, renamed (all of them, as GET /v1/devices has them).
    Devices(Vec<Device>),
    /// Bluetooth changed: on or off, found, paired, connected, a prompt.
    Bluetooth(BluetoothStatus),
    /// Streams began, ended or were watched (all of them, as GET /v1/streams
    /// has them: never their passwords).
    Streams(Vec<StreamInfo>),
}

impl Event {
    /// The SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Machine(_) => "machine",
            Event::WorkspaceState(_) => "workspaceState",
            Event::View(_) => "view",
            Event::Carousel(_) => "carousel",
            Event::Session(_) => "session",
            Event::Projects { .. } => "projects",
            Event::SignIns { .. } => "signins",
            Event::Launch(_) => "launch",
            Event::Build(_) => "build",
            Event::Cloud(_) => "cloud",
            Event::Network(_) => "network",
            Event::Workspaces(_) => "workspaces",
            Event::Notice { .. } => "notice",
            Event::Shortcut { .. } => "shortcut",
            Event::Storage { .. } => "storage",
            Event::Devices(_) => "devices",
            Event::Bluetooth(_) => "bluetooth",
            Event::Streams(_) => "streams",
        }
    }
}

/// What the machine shows: WadSpaces Client (the app's own window, "home"), or a
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum View {
    Home,
    Workspace(String),
}

/// GET /v1/view
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub view: View,
    /// A workspace that's coming up and is shown once it's ready (until
    /// then the screen stays where it is).
    pub pending: Option<String>,
    /// There's a compositor to show native workspaces in.
    pub native_display: bool,
}

/// The Super+Tab switcher: WadSpaces, then the spaces running, then what runs
/// on the machine, each in the order it started (Super+N's order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Carousel {
    pub open: bool,
    pub items: Vec<CarouselItem>,
    /// The selected item.
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarouselItem {
    pub view: View,
    pub name: String,
    /// The workspace's icon (a /v1 path), if it has one.
    pub icon: Option<String>,
    /// A wadspace's desktop to show for it: its wallpaper (a /v1 path; 404
    /// when it has none).
    #[serde(default)]
    pub preview: Option<String>,
    pub running: bool,
    /// "home" (WadSpaces), "space" (a wadspace) or "app" (WadBrowser, the
    /// terminal: what runs on the machine itself).
    #[serde(default)]
    pub kind: String,
    /// Its Super+number: its place among what runs (spaces, then apps).
    #[serde(default)]
    pub number: Option<u8>,
    /// A wadspace's desktop icons, drawn on its preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop: Vec<DesktopIcon>,
}

/// POST /v1/session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub workspaces: Vec<String>,
    /// A focus session's length; none: no timer (a free session).
    pub minutes: Option<u32>,
    /// The caller launches the picks itself (POST /v1/launches, with their
    /// projects): the session doesn't start them.
    #[serde(default)]
    pub launched: bool,
}

/// GET /v1/keys: the keyboard proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeysStatus {
    pub enabled: bool,
    /// The keyboards are grabbed (Super never reaches a workspace), not just watched.
    pub grabbing: bool,
    pub keyboards: Vec<String>,
    pub note: Option<String>,
}

/// How a workspace is shown on its machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Display {
    /// A lean image whose desktop is a window on the machine's own screen.
    Host,
    /// An all-in-one Selkies image, streamed (legacy).
    Stream,
}

/// What runs a workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// An image, run as a podman container (a quadlet).
    #[default]
    Container,
    /// A program on the machine itself (the terminal, WadBrowser), run as the
    /// kiosk user in a transient unit: no image, no projects.
    App,
}

impl Kind {
    pub fn is_container(&self) -> bool {
        *self == Kind::Container
    }
}

/// How a project gets into a wadspace, at ~/Desktop/<mount>. A GitHub one
/// or a copy lives inside the wadspace (its /config); a mount is the
/// machine's own folder, there only on that machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectKind {
    /// A GitHub repo the wadspace clones, then fast-forwards at each start
    /// when that's safe.
    Github,
    /// Files from a machine, copied in once.
    Copy,
    /// A folder or file on a machine, mounted read-write: changes land on
    /// the machine. `from` says where; one path.
    Mount,
}

/// A drive a copy came from (a drive project from before copies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveRef {
    pub uuid: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub fstype: String,
}

/// Where a copy's files came from: paths on a machine, or on a drive. wadd
/// copies them here the first time the wadspace starts when it hasn't the
/// copy yet (one made before copies were made ahead: a folder or drive
/// project's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyFrom {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<DriveRef>,
    pub paths: Vec<String>,
}

/// A project in a workspace (from its design).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProject {
    pub id: String,
    pub kind: ProjectKind,
    /// ~/Desktop/<mount>.
    pub mount: String,
    /// A GitHub project's repo: owner/name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// A GitHub project's branch or tag (none: its default branch).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Run once after it's cloned (and again when it changes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup: String,
    /// A copy's source, for one this machine hasn't made yet; a mount's
    /// folder or file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<CopyFrom>,
}

/// POST /v1/copies: files on this machine to copy into wadspaces. One
/// folder's contents, or each path (files or folders) by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyRequest {
    pub paths: Vec<String>,
}

/// A copy made on this machine, ready for any wadspace here to take in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyMade {
    pub id: String,
    pub files: u64,
    pub bytes: u64,
}

/// How a device is connected to the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceBus {
    Usb,
    Bluetooth,
}

/// What a device is. Gamepads, webcams, security keys, serial ports and
/// other USB devices are handed to wadspaces; keyboards, mice, sound and
/// drives work in every wadspace through the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    Gamepad,
    Webcam,
    SecurityKey,
    Serial,
    Other,
    Keyboard,
    Mouse,
    Audio,
    Storage,
}

/// GET /v1/devices: a device the machine has, or has had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// What it's known by across plugs: `usb:<vid>:<pid>:<serial>`,
    /// `usb:<vid>:<pid>@<port>` (no serial), or `bt:<MAC>`.
    pub key: String,
    /// Its own name, or the one it was given.
    pub name: String,
    pub bus: DeviceBus,
    pub kind: DeviceKind,
    /// Handed to wadspaces (the rest work in every wadspace already).
    pub attachable: bool,
    /// Plugged in (Bluetooth: connected) now.
    pub present: bool,
    /// The running wadspaces that have it now.
    #[serde(default)]
    pub attached: Vec<String>,
    /// The wadspaces it goes back to: when it's plugged in, or when they start.
    #[serde(default)]
    pub remembered: Vec<String>,
    /// Never asked about yet: plugging it in asks where it goes.
    #[serde(default)]
    pub new: bool,
    /// A drive that was ejected: safe to unplug.
    #[serde(default)]
    pub ejected: bool,
}

/// GET /v1/bluetooth: the machine's Bluetooth (BlueZ).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothStatus {
    /// There's an adapter.
    pub available: bool,
    pub powered: bool,
    /// Looking for devices to pair.
    pub discovering: bool,
    /// Paired ones first, then what's in range.
    #[serde(default)]
    pub devices: Vec<BluetoothDevice>,
    /// Pairing wants an answer (or something shown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<BluetoothPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothDevice {
    /// AA:BB:CC:DD:EE:FF
    pub address: String,
    pub name: String,
    /// What it is, from its class (headphones are Audio, a pad Gamepad).
    pub kind: DeviceKind,
    pub paired: bool,
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i16>,
    /// Pairing or connecting now.
    #[serde(default)]
    pub busy: bool,
}

/// What pairing asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BluetoothAsk {
    /// Does the device show `code`? (yes/no)
    Confirm,
    /// Type `code` on the device, then Enter (nothing to answer).
    Display,
    /// The PIN the device wants (an old one: often 0000).
    Pin,
    /// Let it connect? (no code)
    Authorize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothPrompt {
    pub id: u64,
    pub address: String,
    pub name: String,
    pub ask: BluetoothAsk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// POST /v1/bluetooth/prompt: the answer to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BluetoothAnswer {
    pub id: u64,
    pub accept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

/// POST /v1/bluetooth/power and /scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    pub on: bool,
}

/// POST /v1/devices/{key}/attach and /detach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTo {
    pub wadspace: String,
}

/// PUT /v1/devices/{key}: a new name ("": its own again), and/or where it
/// goes (the answer when it's new: none keeps it on the machine).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remembered: Option<Vec<String>>,
}

/// GET /v1/workspaces: a workspace on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Container or app (left out for a container, so older lists read the same).
    #[serde(default, skip_serializing_if = "Kind::is_container")]
    pub kind: Kind,
    /// An app's command line (an app has no image: `image` is empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec: Vec<String>,
    pub image: String,
    pub display: Display,
    /// The stream's port (streamed workspaces).
    pub port: Option<u16>,
    /// Super+N.
    pub hotkey: Option<u8>,
    pub icon: Option<String>,
    pub enabled: bool,
    /// Started once the machine is up, instead of on first use.
    pub autostart: bool,
    pub container_name: String,
    pub container_port: u16,
    /// In the order the file has them.
    pub env: Vec<(String, String)>,
    pub secrets: Vec<String>,
    pub volumes: Vec<String>,
    pub devices: Vec<String>,
    pub shm_size: Option<String>,
    /// Its projects (from its design): cloned or copied in by the wadspace.
    #[serde(default)]
    pub projects: Vec<WorkspaceProject>,
    /// The saved sign-ins it mounts (from its design).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signins: Vec<WorkspaceSignIn>,
    /// A throwaway run (or a WadBrowser setup run once): never saved
    /// anywhere, and removed, with its extra folder, once it stops.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    /// A wadspace's desktop icons, in selkies-desktop's order (wad-core's
    /// desktop_for, kept at install): what the Super+Tab switcher draws.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop: Vec<DesktopIcon>,
}

/// One icon on a wadspace's desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIcon {
    pub label: String,
    /// The picture: a WadSpaces asset path ("/apps/vscode/icon.png") or a web
    /// address (a web app's favicon); GET /v1/desktop-icon draws either.
    pub icon: String,
}

/// A saved sign-in a workspace mounts, for one of its apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSignIn {
    /// The sign-in's id.
    pub id: String,
    /// The app that uses it: a web app's id (its WadBrowser `--app`) or a
    /// catalog app's (vscode).
    pub app: String,
    /// A web app's (a WadBrowser profile) rather than an app's files.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web: bool,
    /// Its name, for making it on a machine that hasn't got it yet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Its kind ("web", "obsidian", "steam", "vscode-claude-code", ...), for
    /// making it here: the Builder's slots say (an extension's app is vscode).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The Builder's slot it fills ("web:<id>", "ext:<id>", "app:<id>"): one
    /// sign-in per slot (two extensions of one app may each have one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot: String,
}

/// What kind of sign-in: a WadBrowser profile, for any web app.
pub const SIGNIN_WEB: &str = "web";

/// GET /v1/signins: a sign-in kept on this machine, mounted into each
/// workspace that uses it, so signing in once covers all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub id: String,
    pub name: String,
    /// "web" (a WadBrowser profile: any web app can use it), or the app
    /// whose files it keeps (vscode).
    pub app: String,
    /// Those files, under the home folder (none for "web").
    pub paths: Vec<String>,
    /// Epoch milliseconds.
    pub created_at: i64,
    /// What's saved, in bytes.
    pub size: u64,
    /// When anything in it last changed (epoch milliseconds); None while empty.
    pub changed_at: Option<i64>,
    /// The workspaces here that mount it, and the WadBrowser setups that use
    /// it (as `browser-<id>`, the workspace each launches as).
    pub used_by: Vec<String>,
    /// Those of them running now.
    pub in_use: Vec<String>,
}

/// POST /v1/signins
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInRequest {
    pub name: String,
    pub app: String,
    /// For an app with no known files: which to keep.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// PUT /v1/signins/{id}: what changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

/// Where a project's files are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ProjectSource {
    /// A GitHub repository, cloned on each machine that opens it.
    Git {
        url: String,
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
    },
    /// A directory on one machine.
    Folder { machine_id: String, machine_name: String, path: String },
    /// A filesystem by UUID, on whichever machine it's plugged into.
    Drive { uuid: String, label: String, fstype: String, subpath: String },
}

/// GET /v1/projects
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub mount_name: String,
    /// None for a project from before sources (legacy).
    pub source: Option<ProjectSource>,
    pub setup: String,
    /// A tombstone: deletions sync.
    pub deleted: bool,
    /// Epoch milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
    /// The account has seen it (local bookkeeping).
    pub synced: bool,
    /// Made before projects had sources: opens where its folder is, can't be edited.
    pub legacy: bool,
}

/// GET /v1/runs: when a workspace ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub wadspace_id: String,
    pub wadspace_name: String,
    /// "local" or "stream".
    pub mode: String,
    pub user: String,
    pub projects: Vec<String>,
    /// Unix seconds.
    pub started_at: f64,
    pub ended_at: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionMode {
    /// Locked to its workspaces until the time is up.
    Focus,
    /// No timer.
    Free,
}

/// GET /v1/session: the session in progress, if any.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub mode: SessionMode,
    pub workspaces: Vec<String>,
    pub minutes: Option<u32>,
    /// Unix seconds.
    pub started_at: f64,
    /// When the focus time ends; None until it starts (or with no timer).
    pub ends_at: Option<f64>,
    pub expired: bool,
}

/// GET /v1/cloud: whether (and to whom) this machine is linked. No tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLink {
    pub linked: bool,
    pub machine_id: Option<String>,
    pub owner_uid: Option<String>,
    pub project_id: Option<String>,
    pub linked_at: Option<String>,
    /// The last heartbeat the account took (Unix seconds).
    #[serde(default)]
    pub last_heartbeat: Option<u64>,
    /// Why the relay is stuck, if it is.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Where a workspace is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Not running (or unknown); nothing in progress.
    Idle,
    /// Its unit is starting.
    Starting,
    /// Up, waiting for the desktop (its window, or its stream answering).
    Waiting,
    Ready,
    Stopping,
    Error,
}

impl Phase {
    /// Something is in progress.
    pub fn busy(self) -> bool {
        matches!(self, Phase::Starting | Phase::Waiting | Phase::Stopping)
    }
}

/// A workspace's state on this machine (GET /v1/states, `workspaceState` events).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub id: String,
    /// The container: running, exited, missing, ... ("unknown": podman didn't say).
    pub container: String,
    pub phase: Phase,
    pub message: Option<String>,
    pub error: Option<String>,
    /// Is its image here? None: unknown.
    pub image_present: Option<bool>,
    /// When the phase last changed (Unix seconds).
    pub since: f64,
    /// It's streamed: drawn on its stream sidecar, not on the screen.
    #[serde(default)]
    pub streaming: bool,
}

/// GET /v1/streams (and `streams` events): a wadspace streamed to browsers,
/// at https://ws-<guid>.<domain>/ behind its login. Never the password.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub workspace: String,
    /// The hostname (ws-<guid>.<domain>).
    pub host: String,
    pub link: String,
    pub user: String,
    /// When it began, and when someone last watched or touched it (Unix
    /// seconds).
    pub started: f64,
    pub last_active: f64,
    /// Browsers watching now (open websockets).
    pub viewers: u32,
    /// The proxy routes to it (its sidecar was found running).
    pub live: bool,
}

/// What to hand on with a stream: the link and its login. Only from POST
/// /v1/workspaces/{id}/stream and GET /v1/streams/{id}/share.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamShare {
    pub workspace: String,
    pub link: String,
    pub user: String,
    pub password: String,
}

// Never the password in a log line.
impl std::fmt::Debug for StreamShare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamShare")
            .field("workspace", &self.workspace)
            .field("link", &self.link)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// POST /v1/workspaces/{id}/stream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRequest {
    /// It's open on the screen: take it off (else that's refused).
    #[serde(default)]
    pub takeover: bool,
}

/// GET /v1/tunnel: the Cloudflare tunnel streams reach browsers through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub cloudflared_installed: bool,
    /// /etc/cloudflared/token is there (its contents never leave wadd).
    pub token_present: bool,
    /// The tunnel's id, from the token (None: no token, or wadd can't read it).
    pub tunnel_id: Option<String>,
    /// cloudflared.service: active, inactive, failed, ... ("unknown": systemd
    /// didn't say).
    pub service: String,
    /// Connected to Cloudflare, and over how many links.
    pub connected: bool,
    pub connections: u32,
    /// Streams are https://ws-<guid>.<domain>/.
    pub domain: String,
    /// WebRTC beside WebSockets (wadd.toml's [streaming] webrtc).
    pub webrtc: bool,
    /// Streaming can start: a token, cloudflared running and connected.
    pub ready: bool,
}

/// POST /v1/tunnel/selftest: https://ws-selftest.<domain>/ fetched from
/// here, through Cloudflare and the tunnel back to this proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTest {
    pub ok: bool,
    /// The last HTTP status (None: no answer).
    pub status: Option<u16>,
    /// The slowest of the tries, in milliseconds.
    pub ms: u64,
    /// What happened, for people.
    pub detail: String,
}

/// PUT /v1/tunnel/token: a new tunnel token (or the `cloudflared service
/// install <token>` line Cloudflare shows).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelToken {
    pub token: String,
}

// Never the token in a log line.
impl std::fmt::Debug for TunnelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelToken").finish_non_exhaustive()
    }
}

/// GET /v1/browse: the folders in a folder (folder projects' picker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Browse {
    /// None: the list is the roots themselves. Inside a drive, relative to
    /// its root ("" is the root).
    pub path: Option<String>,
    /// Where "up" goes; None at a root.
    pub parent: Option<String>,
    pub dirs: Vec<Dir>,
    /// Its files, when they're asked for (`files=true`: to copy some in).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<Dir>,
    /// There were more than shown.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dir {
    pub name: String,
    pub path: String,
}

/// A project folder's git state, from the refs already there (no fetch).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// None: a detached HEAD.
    pub branch: Option<String>,
    /// Uncommitted changes, untracked files included.
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// None: no upstream branch (or one that's gone).
    pub upstream: Option<String>,
}

/// GET /v1/drives: a filesystem a drive project could be on (not the
/// system's own disk).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    pub uuid: String,
    pub label: String,
    pub fstype: String,
    pub size: u64,
    /// Where it's mounted, if it is.
    pub mountpoint: Option<String>,
    pub removable: bool,
    pub model: Option<String>,
    /// The disk it's on (the kernel's name: "sdb").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
}

/// GET /v1/projects/{id}/status: where its folder is, whether a launch can
/// have it, who mounts it, and its git state (no fetch, no mounting).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub exists_on_disk: bool,
    pub path: Option<String>,
    /// Workspaces here it's a project of.
    pub mounted_in: Vec<String>,
    /// None: not a git repository (or git can't tell).
    pub git: Option<GitStatus>,
    pub available: bool,
    /// Why it isn't available.
    pub reason: Option<String>,
}

/// DELETE /v1/projects/{id}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDeleted {
    pub project: Project,
    /// Its folder was removed too (?purge=true).
    pub purged: bool,
}

/// POST /v1/launches: what to launch. A workspace, a throwaway run of one, a
/// WadBrowser setup, or the terminal. A body with no
/// `kind` is the older shape: a workspace ([`LaunchRequest`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LaunchItem {
    Workspace {
        workspace: String,
        #[serde(default)]
        restart: bool,
        #[serde(default)]
        background: bool,
    },
    /// A wadspace launched once with other construction options, saved nowhere.
    Run {
        /// The wadspace it's a run of.
        from: String,
        overrides: RunOverrides,
        #[serde(default)]
        background: bool,
    },
    /// WadBrowser set up as the launcher says, on the machine beside the spaces.
    Browser {
        setup: BrowserSetup,
        /// A saved setup's id: it launches as `browser-<id>`; none: a run of its own.
        #[serde(default)]
        saved_id: Option<String>,
        #[serde(default)]
        background: bool,
    },
    Terminal {
        #[serde(default)]
        background: bool,
    },
}

/// POST /v1/launches takes either shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LaunchBody {
    Item(LaunchItem),
    Workspace(LaunchRequest),
}

impl LaunchBody {
    pub fn item(self) -> LaunchItem {
        match self {
            LaunchBody::Item(i) => i,
            LaunchBody::Workspace(r) => {
                LaunchItem::Workspace { workspace: r.workspace, restart: r.restart, background: r.background }
            }
        }
    }
}

/// What a throwaway run has instead of its wadspace's own.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOverrides {
    /// The wallpaper (none: the wadspace's own).
    #[serde(default)]
    pub wallpaper: Option<WallpaperUpload>,
    /// Its sign-ins, by slot; a slot left out keeps nothing.
    #[serde(default)]
    pub signins: Vec<WorkspaceSignIn>,
}

/// PUT /v1/workspaces/{id}/wallpaper: a picture (WadSpaces draws gradients
/// and colours to a PNG), mounted in at /run/wadspaces-extra/wallpaper.<ext>.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperUpload {
    /// "png" or "jpg".
    pub ext: String,
    /// The image, base64.
    pub data: String,
}

/// A WadBrowser setup: WadBrowser or Focus, and web apps, each in a window of
/// its own with its saved sign-in (or none: nothing kept).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSetup {
    pub name: String,
    /// "browser" or "focus".
    pub mode: String,
    #[serde(default)]
    pub webapps: Vec<SetupWebApp>,
    /// Sign-ins by web app (`app`: the web app's id; `web` is implied).
    #[serde(default)]
    pub signins: Vec<WorkspaceSignIn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupWebApp {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// The older POST /v1/launches body: open a workspace with these projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub workspace: String,
    /// Restarted if it's running (the UI asks first).
    #[serde(default)]
    pub restart: bool,
    /// Started, not switched to (the screen stays put): one of several
    /// opened at once, the UI shows the first that's ready.
    #[serde(default)]
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchStatus {
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}

impl LaunchStatus {
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Error | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartState {
    Waiting,
    Working,
    Done,
    Error,
}

/// What a launch's first step is getting: the image, or a copy this
/// machine hasn't made yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPart {
    /// "image", or the project's id.
    pub key: String,
    /// "image" or "project".
    pub kind: String,
    pub name: String,
    pub state: PartState,
    /// 0..1, when known.
    pub progress: Option<f64>,
    pub message: Option<String>,
}

/// A launch: step 1 checks the image and makes any copy the wadspace takes
/// in that isn't here yet, step 2 brings it up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub id: String,
    pub ws_id: String,
    pub name: String,
    pub restart: bool,
    pub status: LaunchStatus,
    /// 0..1
    pub progress: f64,
    pub phase: String,
    pub error: Option<String>,
    pub parts: Vec<LaunchPart>,
    /// Unix seconds.
    pub created: f64,
    pub started: Option<f64>,
    pub finished: Option<f64>,
    /// Log lines so far.
    pub line_count: u64,
}

/// GET /v1/launches/{id}?since=n, and launch events: the job, and its log
/// from line `from`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchLog {
    pub launch: Launch,
    pub from: u64,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildStatus {
    Queued,
    Building,
    Done,
    Error,
    Cancelled,
}

impl BuildStatus {
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Error | Self::Cancelled)
    }
}

/// POST /v1/images/build: build a layer's image here: a template's (on the
/// templates it builds on), or an Extensions layer's (on its template). What
/// it's built on is built first when it's missing or out of date. The
/// documents are the account's (wad-core's layers), which wadd reads with
/// wad-core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerBuildRequest {
    /// "template:<id>" or "custom:<id>" (an Extensions layer).
    pub layer: String,
    /// The template: the layer's own, or the one its Extensions are made on.
    pub template: serde_json::Value,
    /// The templates `template` builds on, bottom first (none: the base).
    #[serde(default)]
    pub parents: Vec<serde_json::Value>,
    /// An Extensions layer's build: the layer. ("customization" from before
    /// the layer was called Extensions.)
    #[serde(default, alias = "customization")]
    pub extensions: Option<serde_json::Value>,
}

/// GET /v1/images: a layer's image here, from its labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRow {
    /// "template:<id>" or "custom:<id>".
    pub layer: String,
    #[serde(rename = "ref")]
    pub image_ref: String,
    /// podman's id for it.
    pub id: String,
    /// The sha of the build folder it was built from (wad-core's layers).
    pub dockerfile_sha: String,
    /// Unix seconds.
    pub built_at: f64,
    pub size_mb: f64,
    /// The id of the image it was built FROM, as it was then (a template's
    /// image rebuilt since makes its customizations "changed").
    pub from: String,
}

/// Something in the design the build leaves out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub label: String,
    pub reason: String,
}

/// A build: the design's image, made here, then added as a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub id: String,
    pub ws_id: String,
    pub name: String,
    pub status: BuildStatus,
    /// 0..1
    pub progress: f64,
    pub error: Option<String>,
    /// localhost/wadspaces-<id>:latest
    pub image: String,
    pub base_image: String,
    /// Unix seconds.
    pub created: f64,
    pub started: Option<f64>,
    pub finished: Option<f64>,
    /// It replaced a workspace that was already here.
    pub updated: bool,
    /// It's running the old image: a restart picks up the new one.
    pub restart_required: bool,
    pub skipped: Vec<Skipped>,
    pub line_count: u64,
    /// A layer's build ("template:<id>" or "custom:<id>"); none: a design's.
    #[serde(default)]
    pub layer: Option<String>,
}

/// GET /v1/builds/{id}?since=n, and build events: the job, and its log from
/// line `from`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildLog {
    pub build: Build,
    pub from: u64,
    pub lines: Vec<String>,
}

/// Where a secret on the machine came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretOrigin {
    /// The owner's account (synced).
    Account,
    /// Set on this machine.
    Local,
    /// Made so a workspace that names it can start; it has no value yet.
    Placeholder,
}

/// GET /v1/secrets: the names (never the values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretInfo {
    pub name: String,
    pub origin: SecretOrigin,
}

/// What a sync of the account's secrets did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSynced {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub removed: Vec<String>,
}

/// GET /v1/network
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    /// NetworkManager answers.
    pub available: bool,
    /// connected, connecting, disconnected, ...
    pub state: String,
    /// full | limited | portal | none | unknown
    pub connectivity: String,
    pub wifi_enabled: bool,
    pub wifi_device: Option<String>,
    /// The Wi-Fi network joined, and how strong it is (0..100).
    pub ssid: Option<String>,
    pub signal: Option<u8>,
}

/// GET /v1/network/wifi: a network in range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,
    /// "WPA2", "WPA3", "WPA2 802.1X", "" (open), ...
    pub security: String,
    pub secure: bool,
    /// Can be joined here (not enterprise or WEP).
    pub supported: bool,
    pub active: bool,
    /// Saved: joins without its password.
    pub known: bool,
}

/// POST /v1/network/wifi/connect
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiJoin {
    pub ssid: String,
    /// None: a saved network (its saved password), or an open one.
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PowerAction {
    Poweroff,
    Reboot,
}

/// POST /v1/power
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerRequest {
    pub action: PowerAction,
}

/// GET and PUT /v1/screen/brightness: the screen's backlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brightness {
    /// False: no backlight wadd can set (an external monitor, a VM).
    pub available: bool,
    /// 0 to 100 (wadd never sets it below a dim but readable floor).
    pub percent: u8,
}

/// PUT /v1/screen/brightness
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrightnessRequest {
    pub percent: u8,
}

/// GET /v1/metrics: how busy the machine is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// Percent busy since the last look.
    pub cpu: f64,
    /// Percent of memory used.
    pub mem: f64,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_free: u64,
    pub disk_total: u64,
    /// The 1-minute load average.
    pub load: f64,
    /// The first GPU, e.g. "Intel (i915)".
    pub gpu: String,
}

/// PUT /v1/workspaces/{id}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSaved {
    pub workspace: Workspace,
    /// It's running with the old settings: a restart picks up the new ones.
    pub restart_required: bool,
}

/// GET /v1/diagnostics: enough to see what's wrong without a shell. Redacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub wadd: DiagWadd,
    pub podman: DiagPodman,
    pub disk: DiagDisk,
    pub network: NetworkStatus,
    pub view: ViewState,
    pub keys: KeysStatus,
    pub secrets: Vec<SecretInfo>,
    /// The units whose logs can be read (GET /v1/logs/unit/{unit}).
    pub log_units: Vec<String>,
    pub workspaces: Vec<WorkspaceState>,
    /// The last warnings and errors in wadd's log.
    pub recent_problems: Vec<LogLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagWadd {
    pub version: String,
    pub uptime_s: u64,
    pub pid: u32,
    pub profile: Profile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagPodman {
    pub connected: bool,
    pub version: Option<String>,
    pub graph_root: Option<String>,
    pub storage_driver: Option<String>,
    pub images: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagDisk {
    pub path: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// GET /v1/storage: what WadSpaces takes on this machine's disk, measured
/// at most once a minute (a wadspace's files take a while to count).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    /// The disk the images are on.
    pub disk: DiagDisk,
    /// The base every template builds on; None if it isn't here.
    pub base: Option<ImageBytes>,
    /// Each layer's image: all of it, and its own share (what it adds to the
    /// image it's built on).
    pub layers: Vec<LayerBytes>,
    /// Older builds of layers, kept by nothing that still needs them
    /// (`layer` is the layer they were; `id` the image).
    pub leftovers: Vec<LayerBytes>,
    /// Each wadspace's home (its /config volume).
    pub homes: Vec<HomeBytes>,
    /// Files copied in for wadspaces (POST /v1/copies).
    pub copies: Vec<CopyBytes>,
    /// Everything else on the disk, part by part (so none of it is a mystery).
    pub other: Vec<OtherPart>,
    /// Unix seconds.
    pub measured_at: f64,
    /// A newer count is on its way (a `storage` event says when it's ready).
    #[serde(default)]
    pub measuring: bool,
}

/// POST /v1/storage/delete: what to take off this machine's disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageDelete {
    /// Layers ("template:<id>", "custom:<id>"): each one's image and its
    /// older builds. What's built on it stays (and keeps its files).
    #[serde(default)]
    pub layers: Vec<String>,
    /// Wadspaces: off this machine, with their files here (home and the
    /// copies only they had). Their designs stay in the library.
    #[serde(default)]
    pub wadspaces: Vec<String>,
    /// Other images, by id (older bases, images from before layers, build
    /// leftovers): not while a container or an image here needs one.
    #[serde(default)]
    pub images: Vec<String>,
    /// Volumes no wadspace here has (homes of wadspaces long gone).
    #[serde(default)]
    pub volumes: Vec<String>,
    /// The copy the last stick update left (not while it waits to be applied).
    #[serde(default)]
    pub update_copy: bool,
}

/// POST /v1/storage/delete and /v1/storage/prune: what went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Freed {
    /// The images removed: a layer's ("template:<id>"), or an old build's id.
    pub removed: Vec<String>,
    /// Layers whose name went but whose files stay: an image here is built
    /// on them (they go with it).
    pub kept: Vec<String>,
    /// The wadspaces taken off this machine, their files with them.
    pub uninstalled: Vec<String>,
    /// What couldn't go, and why.
    #[serde(default)]
    pub failed: Vec<String>,
    /// About how much of the disk came back.
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageBytes {
    #[serde(rename = "ref")]
    pub image_ref: String,
    pub id: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerBytes {
    /// "template:<id>" or "custom:<id>".
    pub layer: String,
    pub id: String,
    /// The image it was built on.
    pub from: String,
    pub bytes: u64,
    pub own_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeBytes {
    pub workspace_id: String,
    pub volume: String,
    pub bytes: u64,
}

/// A part of the disk that isn't the wadspaces' layers, homes or copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherPart {
    /// "system", "update-copy", "images", "shared", "containers", "volumes",
    /// "logs", "wadd", "people", "unaccounted".
    pub key: String,
    pub label: String,
    /// What it is, and whether it can go.
    pub detail: String,
    pub bytes: u64,
    /// Its pieces, when it has some to show (each image, each volume).
    pub items: Vec<OtherItem>,
    /// Whether the whole part can be deleted (the update copy).
    pub deletable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherItem {
    /// An image's id, or a volume's name.
    pub id: String,
    pub label: String,
    pub detail: String,
    pub bytes: u64,
    pub deletable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyBytes {
    pub id: String,
    pub bytes: u64,
}

/// GET /v1/logs/unit/{unit} and /v1/logs/workspace/{id}: redacted text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogText {
    pub text: String,
}
