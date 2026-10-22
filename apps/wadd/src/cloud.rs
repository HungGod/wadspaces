//! The account link (cloud.py): wadd and WadSpaces Client meet in Firestore,
//! over plain HTTPS. A machine has no inbound port, so wadd polls:
//!
//! - link(code): enrollMachine (a callable function) turns a code the owner
//!   made into a custom token for this machine; signed in with it, wadd keeps
//!   the refresh token (enrollment.json, the Python wadd's format). A machine
//!   that's linked already sends its current ID token along, so linking
//!   again to the same owner keeps its machine entry; another owner's link
//!   forgets the last owner's projects and secrets here first.
//! - run(): a heartbeat (users/{uid}/machines/{mid}) every heartbeat_s, with
//!   just the fields the rules allow; pending commands every poll_s (backing
//!   off when the account can't be reached); the projects synced every 4th
//!   heartbeat or soon after a change here; the account's secrets every 20th
//!   heartbeat (and on sync-secrets).
//! - open_elsewhere(): the owner's other machines, so a launch can warn
//!   when a project is open on one of them.
//!
//! - the owner's GitHub repo list (users/{uid}/github/repos) for the online
//!   app, which has no token: checked every 20th heartbeat, and written only
//!   when it changed; sooner after a repo is made or a sign-in here.
//! - the Builder's commands from the apps: `build` (a wadspace by its id,
//!   read from the account, installed here and its set built), `launch-run`
//!   (anything the launcher starts, maybe as a focus session) and `session`.
//!   The heartbeat carries the images here (the base, sets), the saved
//!   sign-ins' names (never what they hold) and the screen's size.
//! - for the remote API (remote.rs): a wadspace of the owner's
//!   ([`CloudRelay::wadspace_doc`]), and whether a device's sign-in is the
//!   owner's ([`CloudRelay::owner_token`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use wad_firebase::values::{doc_id, doc_path, from_fields, time, to_ms};
use wad_firebase::{Firestore, Functions, Identity};
use wad_proto::v1::{CloudLink, Display, Event, View as V};
use wad_proto::{ApiError, ErrorCode};

use crate::events::Bus;
use crate::launches::Launches;
use crate::projects::Projects;
use crate::registry::Registry;
use crate::secrets::Secrets;
use crate::view::View;

/// Projects sync every 4th heartbeat (2 min): each sync reads every project
/// document, and Firestore bills per read. A change here goes up sooner.
const PROJECTS_EVERY: u64 = 4;
/// The account's secrets every 20th heartbeat (10 min), and on sync-secrets.
const SECRETS_EVERY: u64 = 20;
/// A machine whose last heartbeat is older than this many heartbeats is off.
const OFFLINE_AFTER_BEATS: u64 = 3;
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// What a machine writes to a project document. The Syncthing-era fields go
/// in the update mask without a value, which removes them.
const PROJECT_FIELDS: [&str; 5] = ["name", "mountName", "source", "setup", "deleted"];
const OLD_FIELDS: [&str; 3] = ["holders", "ignore", "folderId"];
/// Joins a tailnet rather than being a container's secret (Tailscale is parked).
const TAILSCALE_AUTHKEY: &str = "tailscale_authkey";

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn hostname() -> String {
    nix::unistd::gethostname().ok().and_then(|h| h.into_string().ok()).unwrap_or_else(|| "wadspaces".into())
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Where the Firebase APIs are: Google's, the emulators', or a test server's.
#[derive(Clone)]
pub struct Endpoints {
    pub identity: Identity,
    pub functions: Functions,
    pub firestore_base: String,
}

impl Endpoints {
    pub fn google(http: reqwest::Client, project: &str, region: &str) -> Self {
        Self {
            identity: Identity::new(http.clone()),
            functions: Functions::for_project(http, project, region),
            firestore_base: "https://firestore.googleapis.com".into(),
        }
    }

    /// The emulators on `host`, at firebase.json's ports.
    pub fn emulator(http: reqwest::Client, host: &str, project: &str, region: &str) -> Self {
        Self {
            identity: Identity::emulator(http.clone(), &format!("{host}:9099")),
            functions: Functions::emulator(http, &format!("{host}:5001"), project, region),
            firestore_base: format!("http://{host}:8090"),
        }
    }
}

/// enrollment.json, as the Python wadd wrote it (0600: it holds the refresh
/// token). Fields it doesn't know are kept.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Enrollment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enrolled_at: Option<String>,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

impl Enrollment {
    fn linked(&self) -> bool {
        self.refresh_token.as_deref().is_some_and(|t| !t.is_empty())
            && self.machine_id.as_deref().is_some_and(|m| !m.is_empty())
    }
}

/// The settings the relay needs.
#[derive(Debug, Clone)]
pub struct Settings {
    pub project_id: String,
    pub api_key: String,
    pub heartbeat: Duration,
    pub poll: Duration,
    pub machine_name: String,
    pub state_dir: PathBuf,
}

/// Another of the owner's machines.
#[derive(Debug, Clone, PartialEq)]
pub struct Sibling {
    pub id: String,
    pub name: String,
    pub online: bool,
}

pub struct Parts {
    pub registry: Arc<Registry>,
    pub view: Arc<View>,
    pub projects: Arc<Projects>,
    pub secrets: Arc<Secrets>,
    pub launches: Arc<Launches>,
    pub github: Arc<crate::github::GithubService>,
    pub bus: Bus,
    pub builds: Arc<crate::builds::Builds>,
    pub signins: Arc<crate::signins::SignIns>,
    /// podman, for the images.
    pub backend: Arc<dyn crate::backend::Backend>,
    /// The tailnet, when it's on here: where the owner's devices find the
    /// remote API.
    pub tailnet: Option<Arc<crate::tailnet::Tailnet>>,
}

/// How long an ID token's check is kept.
const OWNER_CHECK_FOR: Duration = Duration::from_secs(300);

pub struct CloudRelay {
    settings: Settings,
    http: reqwest::Client,
    ends: Endpoints,
    parts: Parts,
    state: Mutex<Enrollment>,
    /// The current ID token and when it stops working.
    token: tokio::sync::Mutex<Option<(String, Instant)>>,
    beats: AtomicU64,
    /// The project store's change count at the last sync; -1: never synced.
    projects_seen: AtomicI64,
    secrets_due: std::sync::atomic::AtomicBool,
    siblings: Mutex<Vec<Sibling>>,
    /// The repo list as last written to the account.
    repos_written: Mutex<Option<wad_proto::github::Repos>>,
    status: Mutex<(Option<u64>, Option<String>)>,
    kick: Notify,
    /// ID tokens checked (by their sha-256): the owner's or not, until when.
    owners: Mutex<HashMap<[u8; 32], (bool, Instant)>>,
}

impl CloudRelay {
    pub fn new(settings: Settings, http: reqwest::Client, ends: Endpoints, parts: Parts) -> Arc<Self> {
        let state = std::fs::read(settings.state_dir.join("enrollment.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Arc::new(Self {
            settings,
            http,
            ends,
            parts,
            state: Mutex::new(state),
            token: tokio::sync::Mutex::new(None),
            beats: AtomicU64::new(0),
            projects_seen: AtomicI64::new(-1),
            secrets_due: std::sync::atomic::AtomicBool::new(true),
            siblings: Mutex::default(),
            repos_written: Mutex::default(),
            status: Mutex::default(),
            kick: Notify::new(),
            owners: Mutex::default(),
        })
    }

    fn file(&self) -> PathBuf {
        self.settings.state_dir.join("enrollment.json")
    }

    fn save(&self, e: &Enrollment) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.file();
        let tmp = path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&self.settings.state_dir)?;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(e).expect("json"))?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn linked(&self) -> bool {
        self.state.lock().unwrap().linked()
    }

    /// What installing and building wadspaces needs.
    fn here(&self) -> crate::sets::Here<'_> {
        crate::sets::Here {
            registry: &self.parts.registry,
            backend: self.parts.backend.as_ref(),
            builds: &self.parts.builds,
            state_dir: &self.settings.state_dir,
            machine: self.ids().map(|(_, m)| m),
            signins: Some(&self.parts.signins),
        }
    }

    fn ids(&self) -> Option<(String, String)> {
        let st = self.state.lock().unwrap();
        st.linked().then(|| (st.owner_uid.clone().unwrap_or_default(), st.machine_id.clone().unwrap_or_default()))
    }

    /// The link as the API shows it (no tokens).
    pub fn link_state(&self) -> CloudLink {
        let st = self.state.lock().unwrap();
        let (beat, err) = self.status.lock().unwrap().clone();
        CloudLink {
            linked: st.linked(),
            machine_id: st.machine_id.clone(),
            owner_uid: st.owner_uid.clone(),
            project_id: st.project_id.clone(),
            linked_at: st.enrolled_at.clone(),
            last_heartbeat: beat,
            last_error: err,
        }
    }

    fn publish(&self) {
        self.parts.bus.publish(Event::Cloud(self.link_state()));
    }

    fn api_key(&self) -> String {
        self.state
            .lock()
            .unwrap()
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| self.settings.api_key.clone())
    }

    fn firestore(&self) -> Result<Firestore, String> {
        let project = self
            .state
            .lock()
            .unwrap()
            .project_id
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| self.settings.project_id.clone());
        Firestore::with_base(self.http.clone(), &self.ends.firestore_base, &project).map_err(|e| e.to_string())
    }

    /// An ID token for the machine, refreshed a while before it runs out.
    pub async fn id_token(&self) -> Result<String, String> {
        let mut cached = self.token.lock().await;
        if let Some((t, until)) = cached.as_ref()
            && Instant::now() + Duration::from_secs(300) < *until
        {
            return Ok(t.clone());
        }
        let refresh = self.state.lock().unwrap().refresh_token.clone().filter(|t| !t.is_empty()).ok_or("not linked")?;
        let t =
            self.ends.identity.refresh(&self.api_key(), &refresh).await.map_err(|e| format!("token refresh: {e}"))?;
        if t.refresh_token != refresh {
            let mut st = self.state.lock().unwrap().clone();
            st.refresh_token = Some(t.refresh_token.clone());
            self.save(&st)?;
            *self.state.lock().unwrap() = st;
        }
        *cached = Some((t.id_token.clone(), Instant::now() + Duration::from_secs(t.expires_in)));
        Ok(t.id_token)
    }

    // -------------------------------------------------------------- link
    /// Links this machine with a code the owner made.
    pub async fn link(&self, code: &str) -> Result<CloudLink, ApiError> {
        let code = code.trim().to_uppercase();
        if !(6..=12).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
            return Err(ApiError::new(ErrorCode::BadRequest, "a link code is 6 to 12 letters and digits"));
        }
        // Linked already: say who this machine is, so the same owner keeps its entry.
        let previous = if self.linked() { self.id_token().await.ok() } else { None };
        let mut data = json!({
            "code": code,
            "hostname": hostname(),
            "machineName": self.settings.machine_name,
            "daemonVersion": env!("CARGO_PKG_VERSION"),
        });
        if let Some(t) = &previous {
            data["previousIdToken"] = t.clone().into();
        }
        let res = self.ends.functions.call("enrollMachine", data).await.map_err(|e| match &e {
            wad_firebase::Error::Callable { status, message } => {
                let code = match status.as_str() {
                    "NOT_FOUND" | "INVALID_ARGUMENT" => ErrorCode::BadRequest,
                    "FAILED_PRECONDITION" | "DEADLINE_EXCEEDED" => ErrorCode::Conflict,
                    _ => ErrorCode::Upstream,
                };
                ApiError::new(code, message.clone())
            }
            _ => ApiError::new(ErrorCode::Offline, format!("couldn't reach the account: {e}")),
        })?;
        let (machine_id, owner) = (s(&res, "machineId"), s(&res, "ownerUid"));
        if machine_id.is_empty() || owner.is_empty() {
            return Err(ApiError::new(ErrorCode::Upstream, "the account's answer had no machine"));
        }
        let api_key =
            Some(s(&res, "apiKey")).filter(|k| !k.is_empty()).unwrap_or_else(|| self.settings.api_key.clone());
        let tokens = self
            .ends
            .identity
            .sign_in_with_custom_token(&api_key, &s(&res, "customToken"))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Upstream, format!("signing in: {e}")))?;
        let before = self.state.lock().unwrap().owner_uid.clone();
        if let Some(prev) = before.filter(|p| *p != owner) {
            self.forget_owner(&prev).await;
        }
        let st = Enrollment {
            machine_id: Some(machine_id.clone()),
            owner_uid: Some(owner),
            project_id: Some(s(&res, "projectId")).filter(|p| !p.is_empty()).or(Some(self.settings.project_id.clone())),
            api_key: Some(api_key),
            refresh_token: Some(tokens.refresh_token),
            enrolled_at: Some(wad_firebase::values::rfc3339(now_ms())),
            rest: Map::new(),
        };
        self.save(&st).map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
        *self.state.lock().unwrap() = st;
        *self.token.lock().await = Some((tokens.id_token, Instant::now() + Duration::from_secs(tokens.expires_in)));
        // Folder projects made before linking become this machine's.
        self.parts.projects.store.claim_local(&machine_id, &self.settings.machine_name);
        self.secrets_due.store(true, Ordering::Relaxed);
        *self.status.lock().unwrap() = (None, None);
        let how = s(&res, "relink");
        tracing::info!(
            "linked as machine {machine_id}{}",
            if how.is_empty() { String::new() } else { format!(" ({how})") }
        );
        self.publish();
        self.kick.notify_one();
        Ok(self.link_state())
    }

    /// Linked to someone else now: the last owner's projects and secrets
    /// mustn't become theirs. The project documents move aside (the relay
    /// would push them to the new account); clones stay on disk, unused.
    async fn forget_owner(&self, uid: &str) {
        let root = self.parts.projects.store.root().to_path_buf();
        if std::fs::read_dir(&root).is_ok_and(|mut d| d.next().is_some()) {
            let aside = root.with_file_name(format!("projects.{uid}.{}", now_ms() / 1000));
            match std::fs::rename(&root, &aside) {
                Ok(()) => tracing::info!("linked to a new owner: {uid}'s projects moved to {}", aside.display()),
                Err(e) => tracing::warn!("moving {uid}'s projects aside: {e}"),
            }
        }
        match self.parts.secrets.forget_account().await {
            Ok(gone) if !gone.is_empty() => {
                tracing::info!("linked to a new owner: removed {uid}'s secrets {}", gone.join(", "))
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("couldn't remove the last owner's secrets: {e}"),
        }
        self.projects_seen.store(-1, Ordering::Relaxed);
        self.siblings.lock().unwrap().clear();
        *self.repos_written.lock().unwrap() = None;
    }

    /// Forgets the link here (the account keeps the machine's entry until the
    /// owner removes it).
    pub async fn unlink(&self) -> Result<(), ApiError> {
        match std::fs::remove_file(self.file()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(ApiError::new(ErrorCode::Internal, e.to_string()));
            }
            _ => {}
        }
        *self.state.lock().unwrap() = Enrollment::default();
        *self.token.lock().await = None;
        self.siblings.lock().unwrap().clear();
        *self.status.lock().unwrap() = (None, None);
        tracing::info!("unlinked");
        self.publish();
        Ok(())
    }

    // --------------------------------------------------------- heartbeat
    fn machine_doc(&self) -> Result<String, String> {
        let (uid, mid) = self.ids().ok_or("not linked")?;
        Ok(format!("users/{uid}/machines/{mid}"))
    }

    /// What the account is told about this machine: only these fields (the
    /// rules allow no others).
    pub fn heartbeat_fields(&self) -> Map<String, Value> {
        let r = &self.parts.registry;
        let view = match self.parts.view.current() {
            V::Home => "launcher".to_string(),
            V::Workspace(id) => format!("workspace:{id}"),
        };
        let states = r.states();
        let workspaces: Vec<Value> = r
            .workspaces()
            .into_iter()
            .filter(|w| w.enabled)
            .map(|w| {
                let st = states.iter().find(|s| s.id == w.id);
                json!({
                    "id": w.id, "name": w.name, "port": w.port, "hotkey": w.hotkey,
                    "display": if w.display == Display::Host { "host" } else { "stream" },
                    "container": st.map(|s| s.container.clone()).unwrap_or_else(|| "unknown".into()),
                    "phase": st.map(|s| serde_json::to_value(s.phase).unwrap_or(Value::Null)).unwrap_or(Value::Null),
                    "error": st.and_then(|s| s.error.clone()),
                })
            })
            .collect();
        let mut m = Map::new();
        m.insert("lastSeen".into(), time(now_ms()));
        m.insert("hostname".into(), hostname().into());
        m.insert("daemonVersion".into(), env!("CARGO_PKG_VERSION").into());
        m.insert("view".into(), view.into());
        m.insert("workspaces".into(), workspaces.into());
        m
    }

    /// What the Builder reads from the heartbeat: the layer images, the
    /// saved sign-ins (ids, names, kinds) and the screen's size.
    pub async fn builder_fields(&self) -> Map<String, Value> {
        let p = &self.parts;
        let mut m = Map::new();
        if let Ok(images) = crate::images::list(p.backend.as_ref()).await {
            m.insert("images".into(), serde_json::to_value(images).unwrap_or(Value::Null));
        }
        let signins: Vec<Value> =
            p.signins.list().into_iter().map(|s| json!({"id": s.id, "name": s.name, "app": s.app})).collect();
        m.insert("signins".into(), signins.into());
        m.insert("screen".into(), serde_json::to_value(p.view.screen().await).unwrap_or(Value::Null));
        if let Some(t) = p.tailnet.as_ref().and_then(|t| t.heartbeat_value()) {
            m.insert("tailnet".into(), t);
        }
        m
    }

    pub async fn heartbeat(&self) -> Result<(), String> {
        let path = self.machine_doc()?;
        let tok = self.id_token().await?;
        let mut fields = self.heartbeat_fields();
        fields.extend(self.builder_fields().await);
        self.firestore()?.patch(&tok, &path, &fields, &[]).await.map_err(|e| e.to_string())?;
        self.status.lock().unwrap().0 = Some((now_ms() / 1000) as u64);
        Ok(())
    }

    // ---------------------------------------------------------- commands
    async fn pending_commands(&self) -> Result<Vec<(String, Value)>, String> {
        let path = self.machine_doc()?;
        let tok = self.id_token().await?;
        let q = json!({
            "from": [{"collectionId": "commands"}],
            "where": {"fieldFilter": {"field": {"fieldPath": "status"}, "op": "EQUAL", "value": {"stringValue": "pending"}}},
            "limit": 10,
        });
        let docs = self.firestore()?.query(&tok, &path, q).await.map_err(|e| e.to_string())?;
        let mut out: Vec<(String, Value)> =
            docs.iter().map(|d| (doc_path(d), from_fields(d.get("fields").unwrap_or(&Value::Null)))).collect();
        out.sort_by_key(|(_, c)| to_ms(c.get("createdAt").unwrap_or(&Value::Null)));
        Ok(out)
    }

    /// One command from WadSpaces Client; its result.
    pub async fn execute(&self, cmd: &Value) -> Result<Value, String> {
        let p = &self.parts;
        let ws = s(cmd, "wsId");
        let need_ws = |kind: &str| if ws.is_empty() { Err(format!("{kind} needs wsId")) } else { Ok(()) };
        let msg = |e: ApiError| e.message;
        match s(cmd, "type").as_str() {
            "start" => {
                need_ws("start")?;
                p.registry.start(&ws).map_err(msg)?;
            }
            "stop" => {
                need_ws("stop")?;
                p.view.stop(&ws).await.map_err(msg)?;
            }
            "restart" => {
                need_ws("restart")?;
                p.view.restart(&ws).await.map_err(msg)?;
            }
            "sync-secrets" => {
                let r = self.sync_secrets().await?;
                return Ok(json!({"ok": true, "added": r.added, "updated": r.updated, "removed": r.removed}));
            }
            "projects-sync" => {
                let (pulled, pushed) = self.sync_projects().await?;
                // The online app asks this when it wants the repo list now too.
                let repos = match self.sync_repos(true).await {
                    Ok(Some(written)) => json!({"written": written}),
                    Ok(None) => Value::Null,
                    Err(e) => json!({"error": e}),
                };
                return Ok(json!({"ok": true, "pulled": pulled, "pushed": pushed, "repos": repos}));
            }
            // A wadspace by its id: read from the account, installed here
            // and its set built.
            "build" => {
                need_ws("build")?;
                let w = self.wadspace_doc(&ws).await.map_err(msg)?;
                let tz = s(cmd, "tz");
                let (_, b) = crate::sets::install_and_build(
                    &self.here(),
                    &ws,
                    &w,
                    Some(&tz).filter(|t| !t.is_empty()).map(|t| t.as_str()),
                )
                .await
                .map_err(msg)?;
                return Ok(
                    json!({"ok": true, "imageKey": b.image_key, "built": b.built, "buildId": b.build.map(|b| b.id)}),
                );
            }
            "launch-run" => {
                let items: Vec<wad_proto::v1::LaunchItem> =
                    serde_json::from_value(cmd.get("items").cloned().unwrap_or(json!([])))
                        .map_err(|e| e.to_string())?;
                let minutes = cmd.get("focusMinutes").and_then(Value::as_u64).map(|m| m as u32);
                let ids = p.launches.batch(items, minutes).await.map_err(msg)?;
                return Ok(json!({"ok": true, "workspaces": ids}));
            }
            other => return Err(format!("unknown command type {other:?}")),
        }
        Ok(json!({"ok": true}))
    }

    /// A document by its path, with its id; None when it isn't there.
    async fn doc_at(&self, path: &str, id: &str) -> Result<Option<Value>, String> {
        let tok = self.id_token().await?;
        let doc = self.firestore()?.get(&tok, path).await.map_err(|e| e.to_string())?;
        Ok(doc.map(|d| {
            let mut v = from_fields(d.get("fields").unwrap_or(&Value::Null));
            v["id"] = id.into();
            v
        }))
    }

    /// One of the owner's wadspaces (wadspaces/{id}): what the remote API and
    /// the relay build from. Only the owner's own (the rules let a machine
    /// read wadspaces shared with its owner too), a version 5 one (a version
    /// 4 one upgraded), and not one that's deleted.
    pub async fn wadspace_doc(&self, ws_id: &str) -> Result<Value, ApiError> {
        let (uid, _) =
            self.ids().ok_or_else(|| ApiError::new(ErrorCode::Offline, "This machine isn't linked to an account."))?;
        if !wad_core::spec::is_id(ws_id) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("{ws_id:?} isn't a wadspace's id")));
        }
        let up = |e: String| ApiError::new(ErrorCode::Upstream, format!("The account: {e}"));
        let w = self.doc_at(&format!("wadspaces/{ws_id}"), ws_id).await.map_err(up)?;
        let w = w.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That wadspace isn't in the account."))?;
        if s(&w, "owner") != uid {
            return Err(ApiError::new(ErrorCode::Forbidden, "That wadspace isn't the machine owner's."));
        }
        if w.get("deletedAt").is_some_and(|d| !d.is_null()) {
            return Err(ApiError::new(ErrorCode::NotFound, "That wadspace is deleted."));
        }
        if !(wad_core::v5::is_v5(&w) || wad_core::v5::is_v4(&w)) {
            return Err(ApiError::new(ErrorCode::BadRequest, "That wadspace is from the old Builder: make it again."));
        }
        Ok(wad_core::v5::upgrade(&w))
    }

    /// Whether an ID token is the machine owner's: Firebase is asked (it
    /// checks the signature and that it hasn't expired), and the answer is
    /// kept for a few minutes.
    pub async fn owner_token(&self, id_token: &str) -> Result<bool, String> {
        let (uid, _) = self.ids().ok_or("not linked")?;
        let key: [u8; 32] = Sha256::digest(id_token.as_bytes()).into();
        {
            let mut known = self.owners.lock().unwrap();
            known.retain(|_, (_, until)| *until > Instant::now());
            if let Some((ok, _)) = known.get(&key) {
                return Ok(*ok);
            }
        }
        let ok = match self.ends.identity.lookup(&self.api_key(), id_token).await {
            Ok(who) => who == uid,
            Err(wad_firebase::Error::Auth { message, .. }) => {
                tracing::debug!("remote: a sign-in that didn't check out: {message}");
                false
            }
            Err(e) => return Err(e.to_string()),
        };
        self.owners.lock().unwrap().insert(key, (ok, Instant::now() + OWNER_CHECK_FOR));
        Ok(ok)
    }

    pub async fn poll_once(&self) -> Result<(), String> {
        let cmds = self.pending_commands().await?;
        for (path, cmd) in cmds {
            let tok = self.id_token().await?;
            let fs = self.firestore()?;
            let mut running = Map::new();
            running.insert("status".into(), "running".into());
            running.insert("startedAt".into(), time(now_ms()));
            fs.patch(&tok, &path, &running, &[]).await.map_err(|e| e.to_string())?;
            let (status, result) = match self.execute(&cmd).await {
                Ok(r) => ("done", r),
                Err(e) => {
                    tracing::warn!("command {path} ({}) failed: {e}", s(&cmd, "type"));
                    ("error", json!({"error": e}))
                }
            };
            let mut done = Map::new();
            done.insert("status".into(), status.into());
            done.insert("result".into(), result);
            done.insert("finishedAt".into(), time(now_ms()));
            fs.patch(&tok, &path, &done, &[]).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // -------------------------------------------------------------- sync
    /// Two-way sync of the owner's projects: the newer updatedAt wins; what's
    /// newer here (or new) is written up. Machines never delete a project
    /// document: a deletion is a tombstone. (pulled, pushed).
    pub async fn sync_projects(&self) -> Result<(usize, usize), String> {
        let (uid, mid) = self.ids().ok_or("not linked")?;
        let store = &self.parts.projects.store;
        store.claim_local(&mid, &self.settings.machine_name);
        let changes = store.changes();
        let base = format!("users/{uid}/projects");
        let tok = self.id_token().await?;
        let fs = self.firestore()?;
        let remote: Vec<Value> = fs
            .list(&tok, &base)
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .map(|d| {
                let mut p = from_fields(d.get("fields").unwrap_or(&Value::Null));
                p["id"] = doc_id(d).into();
                p["createdAt"] = to_ms(&p["createdAt"]).into();
                p["updatedAt"] = to_ms(&p["updatedAt"]).into();
                p
            })
            .collect();
        let stamp = |d: &Value| (s(d, "id"), d.get("updatedAt").cloned().unwrap_or(Value::Null));
        let before: Vec<(String, Value)> = store.list(true).iter().map(stamp).collect();
        let push = store.merge(&remote);
        let mut sent = vec![];
        for doc in &push {
            let mut fields = Map::new();
            for k in PROJECT_FIELDS {
                fields.insert(k.into(), doc.get(k).cloned().unwrap_or(Value::Null));
            }
            for k in ["createdAt", "updatedAt"] {
                fields.insert(k.into(), time(doc.get(k).and_then(Value::as_i64).unwrap_or(0)));
            }
            let id = s(doc, "id");
            match fs.patch(&tok, &format!("{base}/{id}"), &fields, &OLD_FIELDS).await {
                Ok(()) => sent.push(id),
                // One refused (the rules) shouldn't hold the rest up.
                Err(e) => tracing::warn!("project {id} not sent: {e}"),
            }
        }
        store.mark_synced(&sent);
        self.projects_seen.store(changes as i64, Ordering::Relaxed);
        let pulled: Vec<String> =
            store.list(true).iter().map(stamp).filter(|x| !before.contains(x)).map(|(id, _)| id).collect();
        if !pulled.is_empty() {
            self.parts.bus.publish(Event::Projects { ids: pulled.clone() });
        }
        Ok((pulled.len(), sent.len()))
    }

    /// The account's secrets into podman (only what changed).
    pub async fn sync_secrets(&self) -> Result<wad_proto::v1::SecretsSynced, String> {
        let (uid, _) = self.ids().ok_or("not linked")?;
        let tok = self.id_token().await?;
        let docs = self.firestore()?.list(&tok, &format!("users/{uid}/secrets")).await.map_err(|e| e.to_string())?;
        let account: Vec<(String, String)> = docs
            .iter()
            .map(|d| (doc_id(d), s(&from_fields(d.get("fields").unwrap_or(&Value::Null)), "value")))
            .filter(|(n, v)| n != TAILSCALE_AUTHKEY && !v.is_empty())
            .collect();
        let r = self.parts.secrets.sync_account(&account).await?;
        self.secrets_due.store(false, Ordering::Relaxed);
        if !(r.added.is_empty() && r.updated.is_empty() && r.removed.is_empty()) {
            tracing::info!(
                "secrets from the account: added {:?}, updated {:?}, removed {:?}",
                r.added,
                r.updated,
                r.removed
            );
        }
        Ok(r)
    }

    /// Writes the owner's repo list to the account when it differs from what
    /// was last written: Some(written?), None without a token.
    pub async fn sync_repos(&self, fresh: bool) -> Result<Option<bool>, String> {
        let (uid, _) = self.ids().ok_or("not linked")?;
        self.parts.github.repos_due.store(false, Ordering::Relaxed);
        let Some(list) = self.parts.github.repos(fresh).await.map_err(|e| e.message)? else { return Ok(None) };
        if self.repos_written.lock().unwrap().as_ref() == Some(&list) {
            return Ok(Some(false));
        }
        let mut data = Map::new();
        data.insert("login".into(), list.login.clone().into());
        data.insert("repos".into(), serde_json::to_value(&list.repos).expect("json"));
        data.insert("updatedAt".into(), time(now_ms()));
        let tok = self.id_token().await?;
        self.firestore()?
            .patch(&tok, &format!("users/{uid}/github/repos"), &data, &[])
            .await
            .map_err(|e| e.to_string())?;
        *self.repos_written.lock().unwrap() = Some(list);
        Ok(Some(true))
    }

    /// The owner's other machines.
    pub async fn load_siblings(&self) -> Result<Vec<Sibling>, String> {
        let (uid, mid) = self.ids().ok_or("not linked")?;
        let tok = self.id_token().await?;
        let docs = self.firestore()?.list(&tok, &format!("users/{uid}/machines")).await.map_err(|e| e.to_string())?;
        let online_ms = (OFFLINE_AFTER_BEATS * self.settings.heartbeat.as_millis() as u64) as i64;
        let now = now_ms();
        let out: Vec<Sibling> = docs
            .iter()
            .filter(|d| doc_id(d) != mid)
            .map(|d| {
                let f = from_fields(d.get("fields").unwrap_or(&Value::Null));
                let seen = to_ms(f.get("lastSeen").unwrap_or(&Value::Null));
                let name = [s(&f, "name"), s(&f, "hostname"), doc_id(d)]
                    .into_iter()
                    .find(|n| !n.is_empty())
                    .unwrap_or_default();
                Sibling { id: doc_id(d), name, online: seen > 0 && now - seen < online_ms }
            })
            .collect();
        *self.siblings.lock().unwrap() = out.clone();
        Ok(out)
    }

    // --------------------------------------------------------------- run
    async fn turn(&self, beat: bool) -> Result<(), String> {
        if beat {
            self.heartbeat().await?;
            self.beats.fetch_add(1, Ordering::Relaxed);
        }
        let beats = self.beats.load(Ordering::Relaxed);
        let changed = self.parts.projects.store.changes() as i64 != self.projects_seen.load(Ordering::Relaxed);
        if (beat && beats % PROJECTS_EVERY == 1) || changed {
            // A failure (rules not allowing it yet) waits for the next turn
            // without slowing the command polling down.
            if let Err(e) = self.sync_projects().await {
                self.projects_seen.store(self.parts.projects.store.changes() as i64, Ordering::Relaxed);
                tracing::warn!("projects sync: {e}");
            }
        }
        if ((beat && beats % SECRETS_EVERY == 1) || self.secrets_due.load(Ordering::Relaxed))
            && let Err(e) = self.sync_secrets().await
        {
            self.secrets_due.store(false, Ordering::Relaxed);
            tracing::warn!("secrets sync: {e}");
        }
        if ((beat && beats % SECRETS_EVERY == 1) || self.parts.github.repos_due.load(Ordering::Relaxed))
            && let Err(e) = self.sync_repos(false).await
        {
            tracing::warn!("GitHub repo list: {e}");
        }
        self.poll_once().await
    }

    /// For as long as wadd runs: heartbeats, syncs and commands, backing off
    /// (up to a minute) while the account can't be reached.
    pub async fn run(self: Arc<Self>) {
        let mut last_beat: Option<Instant> = None;
        let mut backoff = self.settings.poll;
        loop {
            if !self.linked() {
                last_beat = None;
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = self.kick.notified() => {}
                }
                continue;
            }
            let beat = last_beat.is_none_or(|t| t.elapsed() >= self.settings.heartbeat);
            match self.turn(beat).await {
                Ok(()) => {
                    if beat {
                        last_beat = Some(Instant::now());
                    }
                    backoff = self.settings.poll;
                    let had_error = self.status.lock().unwrap().1.take().is_some();
                    if beat || had_error {
                        self.publish();
                    }
                }
                Err(e) => {
                    tracing::warn!("cloud relay: {e}");
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    let changed = self.status.lock().unwrap().1.replace(e.clone()).as_deref() != Some(e.as_str());
                    if changed {
                        self.publish();
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = self.kick.notified() => {}
            }
        }
    }

    /// Look at the account now (after a change here worth sending).
    pub fn kick(&self) {
        self.kick.notify_one();
    }
}
