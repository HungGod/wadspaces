//! Version 5 wadspaces: the Builder's cards. A wadspace lives on one machine
//! and is its apps, its add-ons (each for an app: VS Code's extensions,
//! command-line tools, WadBrowser's web apps and its Focus) and its
//! customization (name, wallpaper, projects, keys).
//!
//! ```json
//! {"version": 5, "id": "iq", "owner": "<uid>", "machine": "<machine id>", "order": 2,
//!  "apps": [{"app": "wadbrowser"}, {"app": "vscode"}, {"app": "terminal"}],
//!  "exts": [{"app": "wadbrowser", "id": "focus", "name": "Focus"},
//!           {"app": "wadbrowser", "id": "github", "name": "GitHub", "url": "https://github.com"},
//!           {"app": "vscode", "id": "anthropic.claude-code"}, {"app": "terminal", "id": "rust"}],
//!  "customization": {"name": "IntelligenceQuest", "wallpaper": {"type": "texture", "value": "stars"}, …},
//!  "trash": [{"id": "k1", "at": "…", "kind": "app", "app": {"app": "tiled"}}, …]}
//! ```
//!
//! An add-on whose app isn't in it waits there (the Builder draws it dashed):
//! nothing else reads it until its app is back. WadBrowser's Focus (its web
//! apps alone, no address bar) is the add-on "focus".
//!
//! Its image is the base with its packages copied on (wad_core::packages): a
//! set image, shared by every wadspace with the same packages. A set is
//! named by its packages alone, so every machine, and the client reading a
//! machine's heartbeat, names it the same; a set on an older base is an
//! update, not a different set. Web apps, the desktop's layout and the
//! wallpaper go in when it starts (/run/wadspaces-extra), not into the image.
//!
//! [`as_template_setup`] gives the (template, setup) pair version 3's helpers
//! take (slots_for, desktop_for, layout_json_for, its sign-ins), so they
//! serve every version.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::js::{self, Obj};
use crate::layers::{self, is_extension_id};
use crate::spec::{DEFAULT_BASE_IMAGE, is_id};
use crate::{catalog, packages};

pub const VERSION: u64 = 5;
pub const MAX_APPS: usize = 40;
pub const MAX_EXTS: usize = 120;
/// What its Recently deleted keeps at most (the newest).
pub const KEEP_MOST: usize = 30;
/// WadBrowser's Focus: the add-on that hides its address bar.
pub const FOCUS: &str = "focus";
/// The backgrounds wad-wallpaper draws (each dark and light).
pub const TEXTURES: &[&str] = &[
    "carbon",
    "marble",
    "wood",
    "contour",
    "grid",
    "terrazzo",
    "honeycomb",
    "seigaiha",
    "brushed",
    "camo",
    "stars",
    "halftone",
];
/// The app command-line tools are added to (the base's terminal).
pub const TERMINAL: &str = "terminal";
const SET_PREFIX: &str = "localhost/wadspaces-set-";
/// What its Recently deleted keeps: an app's add-ons, an app, a project, a
/// background.
const KEPT_KINDS: &[&str] = &["exts", "app", "project", "background"];

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn is_v5(w: &Value) -> bool {
    w.get("version").and_then(Value::as_u64) == Some(VERSION)
}

/// A machine's id, as the account names it.
pub fn is_machine_id(m: &str) -> bool {
    !m.is_empty() && m.len() <= 64 && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether an add-on is WadBrowser's Focus.
pub fn is_focus(x: &Value) -> bool {
    s(x, "id") == FOCUS && matches!(s(x, "app"), "wadbrowser" | "")
}

/// WadBrowser's Focus, as an add-on.
pub fn focus() -> Value {
    json!({ "app": "wadbrowser", "id": FOCUS, "name": "Focus" })
}

/// The apps add-ons are for: VS Code, the command line, WadBrowser.
pub fn takes_addons(app: &str) -> bool {
    matches!(app, "vscode" | "wadbrowser" | TERMINAL)
}

fn name_ok(what: &str, name: &str) -> Result<(), String> {
    let n = js::trim(name).chars().count();
    if (1..=80).contains(&n) { Ok(()) } else { Err(format!("{what} needs a name of 1 to 80 characters")) }
}

fn web_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://")) && crate::url::host(u, "").is_some_and(|h| !h.is_empty())
}

/// Whether an add-on is one its app takes: an extension for VS Code, a tool
/// for the command line, a web app (or Focus) for WadBrowser.
fn addon_ok(x: &Value) -> Result<(), String> {
    let (app, id) = (s(x, "app"), s(x, "id"));
    match app {
        "vscode" if is_extension_id(id) => Ok(()),
        "vscode" => Err(format!("{id:?} isn't a VS Code extension's id")),
        TERMINAL if catalog::tool(id).is_some() => Ok(()),
        TERMINAL => Err(format!("there's no command-line tool {id:?}")),
        "wadbrowser" if id == FOCUS && s(x, "url").is_empty() => Ok(()),
        "wadbrowser" if id == FOCUS => Err("Focus is WadBrowser's own: it has no address".into()),
        "wadbrowser" if !is_id(id) => Err(format!("a web app's id {id:?} isn't one")),
        "wadbrowser" if !web_url(s(x, "url")) => Err(format!("the web app {id} needs an http(s) address")),
        "wadbrowser" => name_ok("a web app", s(x, "name")),
        _ => Err(format!("nothing is added to {app}")),
    }
}

/// What's wrong with a version 5 wadspace, if anything.
pub fn check(w: &Value) -> Result<(), String> {
    if !is_v5(w) {
        return Err("it isn't a version 5 wadspace".into());
    }
    if w.get("templates").is_some() {
        return Err("it still has version 4's templates".into());
    }
    if !is_id(s(w, "id")) {
        return Err(format!("{:?} isn't an id", s(w, "id")));
    }
    if !is_machine_id(s(w, "machine")) {
        return Err("it isn't on a machine".into());
    }
    let apps = js::arr(w, "apps");
    if apps.len() > MAX_APPS {
        return Err(format!("at most {MAX_APPS} apps"));
    }
    let mut seen: Vec<&str> = vec![];
    for a in apps {
        let app = s(a, "app");
        if catalog::app(app).is_none() {
            return Err(format!("there's no app {app:?}"));
        }
        if seen.contains(&app) {
            return Err(format!("{app} is in it twice"));
        }
        seen.push(app);
    }
    let exts = js::arr(w, "exts");
    if exts.len() > MAX_EXTS {
        return Err(format!("at most {MAX_EXTS} add-ons"));
    }
    let mut seen: Vec<(&str, &str)> = vec![];
    for x in exts {
        addon_ok(x)?;
        let key = (s(x, "app"), s(x, "id"));
        if seen.contains(&key) {
            return Err(format!("{}'s {} is in it twice", key.0, key.1));
        }
        seen.push(key);
    }
    let c = &w["customization"];
    name_ok("a wadspace", s(c, "name"))?;
    let wp = &c["wallpaper"];
    if !wp.is_null() && !(s(wp, "type") == "texture" && TEXTURES.contains(&s(wp, "value"))) {
        return Err(format!("its background is one of WadSpaces' textures, not {wp}"));
    }
    let trash = js::arr(w, "trash");
    if trash.len() > KEEP_MOST {
        return Err(format!("its Recently deleted keeps at most {KEEP_MOST}"));
    }
    if let Some(k) = trash.iter().find(|k| !KEPT_KINDS.contains(&s(k, "kind"))) {
        return Err(format!("its Recently deleted can't keep {:?}", s(k, "kind")));
    }
    Ok(())
}

/// Every app in it once, with its add-ons as extras (Focus as WadBrowser's
/// mode): `[{app, mode?, extras}]`. Add-ons whose app isn't in it are left
/// out.
pub fn apps_of(w: &Value) -> Vec<Value> {
    let exts = js::arr(w, "exts");
    let mut out: Vec<Value> = vec![];
    for a in js::arr(w, "apps") {
        let app = s(a, "app");
        if app.is_empty() || out.iter().any(|e| s(e, "app") == app) {
            continue;
        }
        let mine: Vec<&Value> = exts.iter().filter(|x| s(x, "app") == app).collect();
        let mut extras: Vec<Value> = vec![];
        for x in mine.iter().filter(|x| !is_focus(x)) {
            if !extras.iter().any(|y| s(y, "id") == s(x, "id")) {
                let mut e = js::obj(x);
                e.shift_remove("app");
                extras.push(Value::Object(e));
            }
        }
        let mut o = Obj::new();
        o.insert("app".into(), app.into());
        if app == "wadbrowser" && mine.iter().any(|x| is_focus(x)) {
            o.insert("mode".into(), FOCUS.into());
        }
        o.insert("extras".into(), Value::Array(extras));
        out.push(Value::Object(o));
    }
    out
}

/// The packages its image is made of: each app's, tool's and extension's,
/// with what they need, in WadSpaces' order.
pub fn packages_of(w: &Value) -> Vec<String> {
    let mut ids = vec![];
    for e in apps_of(w) {
        let app = s(&e, "app");
        ids.extend(packages::of_app(app));
        for x in js::arr(&e, "extras") {
            ids.extend(match app {
                "vscode" => packages::of_extension(s(x, "id")),
                TERMINAL => packages::of_tool(s(x, "id")),
                _ => None,
            });
        }
    }
    packages::closure(&ids)
}

/// A set's name: its packages', whatever base it's built on.
pub fn set_key(packages: &[String]) -> String {
    let mut h = Sha256::new();
    h.update(b"wadspaces-set 1\n");
    for p in packages {
        h.update(p.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// "set:<key>", or "base" for a wadspace with no packages: what wadd's image
/// list (and a machine's heartbeat) calls its image.
pub fn image_key(w: &Value) -> String {
    match packages_of(w) {
        p if p.is_empty() => "base".into(),
        p => format!("set:{}", set_key(&p)),
    }
}

/// The image an image key names (`base`: the machine's base).
pub fn image_ref(key: &str, base: &str) -> String {
    match key.strip_prefix("set:") {
        Some(k) => format!("{SET_PREFIX}{k}:latest"),
        None => base.to_string(),
    }
}

/// Version 3's (template, setup) for it: the template its apps, the setup
/// its web apps, extensions, tools and what opens at start.
pub fn as_template_setup(w: &Value) -> (Value, Value) {
    let id = s(w, "id");
    let apps = apps_of(w);
    let extras = |app: &str| -> Vec<Value> {
        apps.iter().find(|e| s(e, "app") == app).map(|e| js::arr(e, "extras").to_vec()).unwrap_or_default()
    };
    let ids = |app: &str| -> Vec<Value> { extras(app).iter().map(|x| Value::from(s(x, "id"))).collect() };
    let browser = apps.iter().find(|e| s(e, "app") == "wadbrowser");
    let mode = if browser.is_some_and(|b| s(b, "mode") == FOCUS) { "focus" } else { "browser" };
    let webapps: Vec<Value> = extras("wadbrowser")
        .iter()
        .map(|x| {
            let mut o = Obj::new();
            for k in ["id", "name", "url"] {
                o.insert(k.into(), s(x, k).into());
            }
            if !s(x, "iconUrl").is_empty() {
                o.insert("iconUrl".into(), s(x, "iconUrl").into());
            }
            Value::Object(o)
        })
        .collect();
    let template = json!({
        "id": id,
        "name": s(&w["customization"], "name"),
        "apps": apps.iter().map(|e| s(e, "app")).collect::<Vec<_>>(),
    });
    let autostart = w["customization"].get("autostart").filter(|a| a.is_array()).cloned().unwrap_or(json!([]));
    let setup = json!({
        "templateId": id,
        "browserMode": mode,
        "webapps": webapps,
        "extensions": ids("vscode"),
        "tools": ids(TERMINAL),
        "autostart": autostart,
    });
    (template, setup)
}

/// The wadd workspace it installs as on its machine: its set image (or the
/// base), its name and wallpaper, a /config of its own, its sign-ins and its
/// projects. `opts`: `{tz, secrets?: [podman secret names], base?}`.
pub fn workspace_for(w: &Value, opts: &Value) -> Value {
    let (template, setup) = as_template_setup(w);
    let own = &w["customization"];
    let base = match s(opts, "base") {
        "" => DEFAULT_BASE_IMAGE,
        b => b,
    };
    let image = image_ref(&image_key(w), base);
    let signins = layers::signins_v3(w, &template, &setup);
    layers::workspace_value(s(w, "id"), own, image, signins, Some(layers::workspace_projects(own)), opts)
}

/// What wadd puts in its /run/wadspaces-extra when it's installed, for the
/// base's init-wadspaces-workspace to take at start: its web apps
/// (`[{id, name, url, iconUrl?}]`: their launchers; wadd makes the icons),
/// the desktop's layout.json, and WadBrowser's wadbrowser.conf (null
/// without WadBrowser). `{webapps, layout, wadbrowserConf}`.
pub fn extras(w: &Value) -> Value {
    let (template, setup) = as_template_setup(w);
    json!({
        "webapps": setup["webapps"],
        "layout": layers::layout_json_for(&template, &setup),
        "wadbrowserConf": layers::wadbrowser_conf(&template, &setup),
    })
}

/// Its desktop's icons, as its image draws them (what the switcher shows):
/// version 3's, from its apps, web apps and projects.
pub fn desktop(w: &Value) -> Vec<Value> {
    let (template, setup) = as_template_setup(w);
    let projects: Vec<Value> =
        js::arr(&w["customization"], "projects").iter().map(|p| json!({ "id": p["id"], "name": p["as"] })).collect();
    layers::desktop_for(&template, &setup, &projects)
}

/// Whether its image is on a machine, from the machine's image list (GET
/// /v1/images' rows, or a heartbeat's `images`): "never" (not built),
/// "update" (built on a base since replaced: it runs, and a build brings it
/// up to date) or "built".
pub fn state(w: &Value, images: &[Value]) -> &'static str {
    let key = image_key(w);
    let base = images.iter().find(|r| s(r, "layer") == "base");
    match images.iter().find(|r| s(r, "layer") == key) {
        None => "never",
        Some(_) if key == "base" => "built",
        Some(r) if base.is_some_and(|b| s(b, "id") != s(r, "from")) => "update",
        Some(_) => "built",
    }
}

/// What the client and the phone show of a wadspace.
pub fn view(w: &Value) -> Value {
    let c = &w["customization"];
    let mut o = Obj::new();
    for k in ["id", "machine"] {
        o.insert(k.into(), s(w, k).into());
    }
    o.insert("name".into(), s(c, "name").into());
    o.insert("description".into(), s(c, "description").into());
    o.insert("order".into(), w.get("order").cloned().unwrap_or(json!(0)));
    o.insert("deleted".into(), w.get("deletedAt").is_some_and(js::truthy).into());
    o.insert("apps".into(), apps_of(w).iter().map(|e| s(e, "app").to_string()).collect::<Vec<_>>().into());
    o.insert("packages".into(), packages_of(w).into());
    o.insert("imageKey".into(), image_key(w).into());
    o.insert("wallpaper".into(), c.get("wallpaper").cloned().unwrap_or(Value::Null));
    Value::Object(o)
}

#[cfg(test)]
mod tests;
