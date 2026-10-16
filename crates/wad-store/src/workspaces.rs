//! A workspace list checked before wadd takes it on (a build's, a launch's,
//! the image's): every workspace valid, ids, ports and container names
//! unique. The workspaces are given in their YAML shape (to_yaml), the one
//! the machine's workspaces.yaml had, which keeps env values, hotkeys and
//! the like as loose as that file was.

use serde_json::{Map, Value};
use wad_proto::v1::{Display, Kind, ProjectKind, Workspace, WorkspaceProject, WorkspaceSignIn};

use crate::Error;

const WORKSPACE_KEYS: &[&str] = &[
    "id",
    "name",
    "image",
    "port",
    "hotkey",
    "icon",
    "enabled",
    "container_name",
    "container_port",
    "env",
    "secrets",
    "volumes",
    "devices",
    "shm_size",
    "autostart",
    "display",
    "projects",
    "kind",
    "exec",
    "signins",
    "ephemeral",
    "desktop",
];
fn err(msg: impl Into<String>) -> Error {
    Error::Config(msg.into())
}

/// Where a value (or a key) holds a control character, if anywhere: the
/// key's name, or "a key".
fn control_chars(v: &Value) -> Option<String> {
    let bad = |s: &str| s.chars().any(char::is_control);
    match v {
        Value::String(s) if bad(s) => Some("a value".into()),
        Value::Array(a) => a.iter().find_map(control_chars),
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            if bad(k) {
                Some("a key".into())
            } else {
                control_chars(x).map(|w| if w == "a value" { k.clone() } else { w })
            }
        }),
        _ => None,
    }
}

/// A value as text: env values written as numbers or booleans (True and
/// False, as they always were).
fn as_text(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn check_known(map: &Map<String, Value>, known: &[&str], name: &str) -> Result<(), Error> {
    let mut unknown: Vec<&String> = map.keys().filter(|k| !known.contains(&k.as_str())).collect();
    unknown.sort();
    if unknown.is_empty() { Ok(()) } else { Err(err(format!("{name}: unknown keys {unknown:?}"))) }
}

fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().map(as_text).collect()).unwrap_or_default()
}

/// `/[:%\\"'\x00-\x1f\x7f]/`: what a host path may not contain.
pub fn host_path_ok(p: &str) -> bool {
    !p.chars().any(|c| matches!(c, ':' | '%' | '\\' | '"' | '\'') || (c as u32) < 0x20 || c as u32 == 0x7f)
}

/// Workspaces (in to_yaml's shape) checked together: ids, ports and names
/// unique, and each one valid.
pub fn check_workspaces(list: Vec<Value>) -> Result<Vec<Workspace>, Error> {
    let mut workspaces = Vec::new();
    let (mut ids, mut ports, mut names) = (Vec::new(), Vec::new(), Vec::new());
    for (i, raw) in list.iter().enumerate() {
        let where_ = format!("workspaces[{i}]");
        let raw = raw.as_object().cloned().ok_or_else(|| err(format!("{where_}: not a mapping")))?;
        check_known(&raw, WORKSPACE_KEYS, &where_)?;
        // Each value becomes a line of a systemd unit: a line break would add
        // lines of its own (a shared design's name running a command as root).
        if let Some(key) = control_chars(&Value::Object(raw.clone())) {
            return Err(err(format!("{where_}: {key} has a line break or another control character")));
        }
        let kind = match raw.get("kind").map(as_text).as_deref() {
            None | Some("container") => Kind::Container,
            Some("app") => Kind::App,
            Some(_) => return Err(err(format!("{where_}: kind must be one of ('container', 'app')"))),
        };
        let required: &[&str] = if kind == Kind::App { &["id", "name", "exec"] } else { &["id", "name", "image"] };
        for req in required {
            if !raw.contains_key(*req) {
                return Err(err(format!("{where_}: missing {req}")));
            }
        }
        let exec = strings(raw.get("exec"));
        if kind == Kind::App {
            // A program on the machine, drawing on its screen: nothing of a
            // container's applies to it.
            if exec.first().is_none_or(|p| p.is_empty()) {
                return Err(err(format!("{where_}: exec must name a program")));
            }
            if raw.get("display").is_some_and(|d| as_text(d) != "host") {
                return Err(err(format!("{where_}: an app is shown on this screen (display must be 'host')")));
            }
            for key in ["image", "port", "secrets", "volumes", "devices", "projects"] {
                if raw.get(key).is_some_and(crate::state::truthy) {
                    return Err(err(format!("{where_}: an app has no {key}")));
                }
            }
            // A WadBrowser setup's web apps keep web sign-ins; nothing else does.
            let web_only = |s: &Value| s.get("web").and_then(Value::as_bool) == Some(true);
            if raw.get("signins").and_then(Value::as_array).is_some_and(|l| !l.iter().all(web_only)) {
                return Err(err(format!("{where_}: an app keeps only web sign-ins (a WadBrowser's)")));
            }
        } else if !exec.is_empty() {
            return Err(err(format!("{where_}: only an app has exec")));
        }
        let default_display = if kind == Kind::App { "host" } else { "stream" };
        let display = raw.get("display").map(as_text).unwrap_or_else(|| default_display.into());
        let display = match display.as_str() {
            "host" => Display::Host,
            "stream" => Display::Stream,
            _ => return Err(err(format!("{where_}: display must be one of ('stream', 'host')"))),
        };
        let port = int(raw.get("port").filter(|p| !p.is_null()));
        if display == Display::Stream && port.is_none() {
            return Err(err(format!("{where_}: missing port (a streamed workspace needs one)")));
        }
        let id = as_text(&raw["id"]);
        let where_ = format!("workspace '{id}'");
        if !wad_core::spec::is_id(&id) {
            return Err(err(format!("{where_}: id must match ^[a-z0-9][a-z0-9-]{{0,62}}$")));
        }
        if ids.contains(&id) {
            return Err(err(format!("{where_}: duplicate id")));
        }
        if let Some(p) = port {
            if !(1..=65535).contains(&p) {
                return Err(err(format!("{where_}: port out of range")));
            }
            if ports.contains(&p) {
                return Err(err(format!("{where_}: port {p} already used")));
            }
            ports.push(p);
        }
        let container_name =
            raw.get("container_name").map(as_text).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("wad-{id}"));
        if names.contains(&container_name) {
            return Err(err(format!("{where_}: duplicate container_name")));
        }
        let projects = check_projects(raw.get("projects"), &where_)?;
        let signins = check_signins(raw.get("signins"), &where_)?;
        let enabled = raw.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let hotkey = int(raw.get("hotkey").filter(|h| !h.is_null()));
        // Never bound: Super+N follows what runs.
        if let Some(h) = hotkey
            && !(1..=9).contains(&h)
        {
            return Err(err(format!("{where_}: hotkey must be 1..9")));
        }
        let env = raw
            .get("env")
            .and_then(Value::as_object)
            .map(|e| e.iter().map(|(k, v)| (k.clone(), as_text(v))).collect())
            .unwrap_or_default();
        let shm_size = match raw.get("shm_size") {
            None => Some("1g".to_string()),
            Some(Value::Null) => None,
            Some(v) => Some(as_text(v)),
        };
        ids.push(id.clone());
        names.push(container_name.clone());
        workspaces.push(Workspace {
            id,
            name: as_text(&raw["name"]),
            kind,
            exec,
            image: raw.get("image").map(as_text).unwrap_or_default(),
            display,
            port: port.map(|p| p as u16),
            hotkey: hotkey.map(|h| h as u8),
            icon: raw.get("icon").filter(|i| !i.is_null()).map(as_text),
            enabled,
            autostart: raw.get("autostart").and_then(Value::as_bool).unwrap_or(false),
            container_name,
            container_port: int(raw.get("container_port")).unwrap_or(3000) as u16,
            env,
            secrets: strings(raw.get("secrets")),
            volumes: strings(raw.get("volumes")),
            devices: strings(raw.get("devices")),
            shm_size,
            projects,
            signins,
            ephemeral: raw.get("ephemeral").and_then(Value::as_bool).unwrap_or(false),
            // Only for the switcher's picture: one that doesn't read is left out.
            desktop: raw.get("desktop").and_then(|d| serde_json::from_value(d.clone()).ok()).unwrap_or_default(),
        });
    }
    Ok(workspaces)
}

/// `[{id, kind, mount, repo?, ref?, setup?, from?}]`: a workspace's projects,
/// each at its own Desktop/<mount>. Entries from before (`{id, mount,
/// path?}`, what a launch mounted) are dropped; a mount is `kind: "mount"`.
fn check_projects(v: Option<&Value>, where_: &str) -> Result<Vec<WorkspaceProject>, Error> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let list = v.as_array().ok_or_else(|| err(format!("{where_}: projects must be a list")))?;
    let mut out: Vec<WorkspaceProject> = Vec::new();
    for p in list {
        if p.get("kind").is_none() && p.get("mount").is_some() {
            continue;
        }
        let parsed: WorkspaceProject = serde_json::from_value(p.clone()).map_err(|e| {
            err(format!("{where_}: a project is {{id, kind, mount, repo?, ref?, setup?, from?}} ({e})"))
        })?;
        let (id, mount) = (&parsed.id, &parsed.mount);
        if !wad_core::projects::is_project_id(id) {
            return Err(err(format!("{where_}: project id '{id}' must match ^[A-Za-z0-9_-]{{1,64}}$")));
        }
        if !wad_core::projects::is_mount(mount) || mount == "." || mount == ".." {
            return Err(err(format!("{where_}: project folder '{mount}' must match ^[A-Za-z0-9._-]{{1,64}}$")));
        }
        match parsed.kind {
            ProjectKind::Github => {
                let repo_ok = parsed.repo.split_once('/').is_some_and(|(o, n)| {
                    wad_core::projects::is_repo_name(o) && wad_core::projects::is_repo_name(n) && !n.contains('/')
                });
                if !repo_ok {
                    return Err(err(format!("{where_}: {mount}'s repo '{}' isn't owner/name", parsed.repo)));
                }
                if parsed.git_ref.as_deref().is_some_and(|r| !wad_core::projects::is_ref(r) || r.contains("..")) {
                    return Err(err(format!("{where_}: {mount}'s branch isn't one")));
                }
                if parsed.setup.len() > 4000 || parsed.setup.contains('\0') {
                    return Err(err(format!("{where_}: {mount}'s setup command is too long")));
                }
            }
            ProjectKind::Copy | ProjectKind::Mount => {
                if parsed.kind == ProjectKind::Mount && parsed.from.as_ref().is_none_or(|f| f.paths.len() != 1) {
                    return Err(err(format!("{where_}: {mount} mounts one folder or file")));
                }
                if let Some(f) = &parsed.from {
                    let on_drive = f.drive.is_some();
                    let ok = f.paths.len() <= 64
                        && f.paths.iter().all(|x| {
                            !x.contains(char::is_control)
                                && x.len() <= 4096
                                && if on_drive {
                                    !x.starts_with('/') && !x.split('/').any(|part| part == "..")
                                } else {
                                    x.starts_with('/') && host_path_ok(x)
                                }
                        })
                        && f.drive.as_ref().is_none_or(|d| wad_core::projects::is_drive_uuid(&d.uuid))
                        && f.machine_id.len() <= 128
                        && !f.machine_id.contains(char::is_control);
                    if !ok {
                        return Err(err(format!("{where_}: {mount} comes from paths that aren't")));
                    }
                }
            }
        }
        if out.iter().any(|x| &x.id == id) {
            return Err(err(format!("{where_}: project {id} is listed twice")));
        }
        if out.iter().any(|x| &x.mount == mount) {
            return Err(err(format!("{where_}: two projects are at Desktop/{mount}")));
        }
        out.push(parsed);
    }
    Ok(out)
}

/// A Builder slot: "web:<id>", "ext:<marketplace id>" or "app:<id>".
fn slot_ok(slot: &str) -> bool {
    let Some((kind, rest)) = slot.split_once(':') else { return false };
    matches!(kind, "web" | "ext" | "app")
        && !rest.is_empty()
        && rest.len() <= 120
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `[{id, app, web?, name?, kind?, slot?}]`: one sign-in per slot (or per
/// app, without one), ids, apps and kinds as ids are.
fn check_signins(v: Option<&Value>, where_: &str) -> Result<Vec<WorkspaceSignIn>, Error> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let list = v.as_array().ok_or_else(|| err(format!("{where_}: signins must be a list")))?;
    let mut out: Vec<WorkspaceSignIn> = Vec::new();
    for s in list {
        let o = s.as_object();
        let shape_ok = o.is_some_and(|o| {
            o.contains_key("id")
                && o.contains_key("app")
                && o.iter().all(|(k, v)| match k.as_str() {
                    "id" | "app" | "name" | "kind" | "slot" => v.is_string(),
                    "web" => v.is_boolean(),
                    _ => false,
                })
        });
        if !shape_ok {
            return Err(err(format!("{where_}: each sign-in is {{id, app, web?, name?, kind?, slot?}}, not {s}")));
        }
        let o = o.unwrap();
        let (id, app) = (as_text(&o["id"]), as_text(&o["app"]));
        let name = o.get("name").map(as_text).unwrap_or_default();
        let web = o.get("web").and_then(Value::as_bool).unwrap_or(false);
        let kind = o.get("kind").map(as_text).unwrap_or_default();
        let slot = o.get("slot").map(as_text).unwrap_or_default();
        // WadBrowser itself (its windows that aren't a web app's) is "@browser".
        let browser = web && app == wad_core::layers::BROWSER_APP;
        if !wad_core::spec::is_id(&id) || !(browser || wad_core::spec::is_id(&app)) {
            return Err(err(format!(
                "{where_}: sign-in {id:?} for {app:?}: both must match ^[a-z0-9][a-z0-9-]{{0,62}}$ (or the app be @browser, a web sign-in)"
            )));
        }
        if !kind.is_empty() && !wad_core::spec::is_id(&kind) {
            return Err(err(format!("{where_}: sign-in {id:?}: {kind:?} isn't a kind")));
        }
        if !slot.is_empty() && !slot_ok(&slot) {
            return Err(err(format!("{where_}: sign-in {id:?}: {slot:?} isn't a slot")));
        }
        let key = |x: &WorkspaceSignIn| if x.slot.is_empty() { x.app.clone() } else { x.slot.clone() };
        let this = if slot.is_empty() { app.clone() } else { slot.clone() };
        if out.iter().any(|x| key(x) == this) {
            return Err(err(format!("{where_}: {this} has two sign-ins")));
        }
        out.push(WorkspaceSignIn { id, app, web, name, kind, slot });
    }
    Ok(out)
}

/// A workspace in its YAML shape: what check_workspaces() reads back.
pub fn to_yaml(w: &Workspace) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), w.id.clone().into());
    o.insert("name".into(), w.name.clone().into());
    if w.kind == Kind::App {
        o.insert("kind".into(), "app".into());
        o.insert("exec".into(), w.exec.clone().into());
    } else {
        o.insert("image".into(), w.image.clone().into());
    }
    o.insert("display".into(), if w.display == Display::Host { "host" } else { "stream" }.into());
    if let Some(p) = w.port {
        o.insert("port".into(), p.into());
    }
    if let Some(h) = w.hotkey {
        o.insert("hotkey".into(), h.into());
    }
    if let Some(i) = &w.icon {
        o.insert("icon".into(), i.clone().into());
    }
    o.insert("enabled".into(), w.enabled.into());
    o.insert("autostart".into(), w.autostart.into());
    o.insert("container_name".into(), w.container_name.clone().into());
    o.insert("container_port".into(), w.container_port.into());
    let env: Map<String, Value> = w.env.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
    o.insert("env".into(), Value::Object(env));
    o.insert("secrets".into(), w.secrets.clone().into());
    o.insert("volumes".into(), w.volumes.clone().into());
    o.insert("devices".into(), w.devices.clone().into());
    o.insert("shm_size".into(), w.shm_size.clone().map(Value::from).unwrap_or(Value::Null));
    let projects: Vec<Value> = w.projects.iter().map(|p| serde_json::to_value(p).expect("a project is JSON")).collect();
    o.insert("projects".into(), projects.into());
    if !w.signins.is_empty() {
        let signins: Vec<Value> = w
            .signins
            .iter()
            .map(|s| {
                let mut m = Map::new();
                m.insert("id".into(), s.id.clone().into());
                m.insert("app".into(), s.app.clone().into());
                if s.web {
                    m.insert("web".into(), true.into());
                }
                if !s.name.is_empty() {
                    m.insert("name".into(), s.name.clone().into());
                }
                if !s.kind.is_empty() {
                    m.insert("kind".into(), s.kind.clone().into());
                }
                if !s.slot.is_empty() {
                    m.insert("slot".into(), s.slot.clone().into());
                }
                Value::Object(m)
            })
            .collect();
        o.insert("signins".into(), signins.into());
    }
    if w.ephemeral {
        o.insert("ephemeral".into(), true.into());
    }
    if !w.desktop.is_empty() {
        o.insert("desktop".into(), serde_json::to_value(&w.desktop).expect("icons are JSON"));
    }
    Value::Object(o)
}

#[cfg(test)]
mod tests;
