//! What a wadspace is made of, as its image and desktop see it: its apps
//! (and which open folders), the keys its apps and web
//! apps sign in with and the secrets that fit them, its desktop and Start
//! menu as the image draws them, the layout and WadBrowser settings it
//! starts with, and the workspace wadd installs it as. A wadspace comes
//! here as v5::as_template_setup's (template, setup).
//! Values are JSON, as everywhere in the core.

use std::cmp::Ordering;

use serde_json::{Value, json};

use crate::catalog::{self, App, Recipe};
use crate::js::{self, Obj};
use crate::spec::is_id;
use crate::{signins, url, v5};

/// Where wadd puts a wadspace's wallpaper inside it (its extra folder).
pub const WALLPAPER_DIR: &str = "/run/wadspaces-extra";

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn strs<'a>(v: &'a Value, k: &str) -> Vec<&'a str> {
    js::arr(v, k).iter().filter_map(Value::as_str).collect()
}

/// A name or label on one line: control characters become spaces, so no
/// name can add lines of its own to a Dockerfile.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Google's favicon service for a domain (lib/favicon.ts's).
pub fn favicon(domain: &str) -> String {
    format!("https://www.google.com/s2/favicons?domain={}&sz=128", encode_component(domain))
}

/// A URL's host, or the text itself when it isn't one.
pub fn host_of(u: &str) -> String {
    match url::host(u, "") {
        Some(h) if !h.is_empty() => h,
        _ => u.to_string(),
    }
}

/// A web app's icon: its site's favicon.
pub fn webapp_icon(u: &str) -> String {
    favicon(&host_of(u))
}

// ------------------------------------------------------------------ setups

/// A web app's id from its name: "Google Cloud" → "google-cloud" ("focus"
/// is WadBrowser's Focus: a web app called that is "focus-app").
pub fn webapp_id(name: &str) -> String {
    let lower = name.to_lowercase();
    let id = js::replace_runs(&lower, |c| !(c.is_ascii_lowercase() || c.is_ascii_digit()), "-");
    let id = js::ascii_prefix(js::trim_dashes(&id), 60);
    match js::trim_dashes(id) {
        "" => "app".into(),
        v5::FOCUS => format!("{}-app", v5::FOCUS),
        id => id.into(),
    }
}

// --------------------------------------------------------------- wadspaces

/// What a wadspace is made of: `{chain, template, setup}`: its templates
/// (`{id, name}`, top first), and [`v5::as_template_setup`]'s template and
/// setup (what the image and the desktop are made from).
pub fn resolve_stack(w: &Value) -> Value {
    let (template, setup) = v5::as_template_setup(w);
    let chain: Vec<Value> =
        js::arr(w, "templates").iter().map(|t| json!({ "id": s(t, "id"), "name": s(t, "name") })).collect();
    json!({ "chain": chain, "template": template, "setup": setup })
}

/// The image a wadspace runs: its set's ("set:<key>"), or "base".
pub fn stack_image_key(w: &Value) -> String {
    v5::image_key(w)
}

/// An image on a machine, as [`image_state`] reads it: podman's id for it,
/// the id of the image it was built FROM, and its key's sha.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerImage {
    pub id: String,
    pub from: String,
    pub sha: String,
}

/// A wadspace's image on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// Never built here.
    Never,
    /// Built on an older base: still runs, an update is there.
    Update,
    Built,
}

/// The state of the image for `key` ("set:<key>", or "base"), given the
/// machine's images by key ("base" for the base image): a set is built, or
/// an update when it's on an older base (as [`v5::state`]); "base" is built
/// when it's here.
pub fn image_state(key: &str, images: &std::collections::HashMap<String, LayerImage>) -> ImageState {
    let Some(row) = images.get(key) else { return ImageState::Never };
    let base = images.get("base");
    if key != "base" && base.is_some_and(|b| b.id != row.from) { ImageState::Update } else { ImageState::Built }
}

/// Why a wadspace can't be installed or built yet: it needs only a name (it
/// runs on the base with no apps).
pub fn not_ready(w: &Value) -> Option<&'static str> {
    s(&w["customization"], "name").trim().is_empty().then_some("Its customization needs a name.")
}

/// WadBrowser's apps that keep keys: the browser itself ("browser") and
/// each web app ("web:<id>"), when the template has WadBrowser.
pub fn key_items(template: &Value, setup: &Value) -> Vec<String> {
    if !has_app(template, "wadbrowser") {
        return vec![];
    }
    let web = js::arr(setup, "webapps").iter().map(|w| format!("web:{}", s(w, "id")));
    std::iter::once("browser".to_string()).chain(web).collect()
}

/// WadBrowser's key groups, tidied: members that are gone dropped, and
/// those in no group joining the first group with members (so by default
/// one group: everything shares one profile). An empty group stays (it was
/// made to drag apps into). `[{id, members, key?}]`.
pub fn key_groups_of(template: &Value, setup: &Value, groups: &Value) -> Vec<Value> {
    let items = key_items(template, setup);
    if items.is_empty() {
        return vec![];
    }
    let mut out: Vec<Value> = groups
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            let mut g = g.clone();
            let members: Vec<Value> =
                strs(&g, "members").into_iter().filter(|m| items.iter().any(|i| i == m)).map(Value::from).collect();
            g["members"] = Value::Array(members);
            g
        })
        .collect();
    let placed: Vec<String> = out.iter().flat_map(|g| strs(g, "members")).map(String::from).collect();
    let loose: Vec<Value> = items.iter().filter(|i| !placed.contains(i)).map(|i| Value::from(i.as_str())).collect();
    if loose.is_empty() {
        return out;
    }
    match out.iter().position(|g| !js::arr(g, "members").is_empty()) {
        Some(i) => {
            let mut members = js::arr(&out[i], "members").to_vec();
            members.extend(loose);
            out[i]["members"] = Value::Array(members);
        }
        None => out.insert(0, json!({ "id": "all", "members": loose })),
    }
    out
}

fn apps_of(template: &Value) -> Vec<App> {
    strs(template, "apps").into_iter().filter_map(catalog::app).collect()
}

pub fn has_app(template: &Value, id: &str) -> bool {
    strs(template, "apps").contains(&id)
}

// ----------------------------------------------------------------- secrets

fn slot(key: String, kind: &str, label: String, app_id: String, icon: &str) -> Value {
    json!({ "key": key, "kind": kind, "label": label, "appId": app_id, "iconUrl": icon })
}

/// The id a new secret for a web app is named with: WadBrowser-<Name>.
fn web_app_id(name: &str) -> String {
    format!("WadBrowser-{}", name.chars().filter(char::is_ascii_alphanumeric).collect::<String>())
}

/// The sign-ins a template with this setup can keep, in desktop order: an
/// app's own (Obsidian, Steam), each AI extension's, and each web app's.
pub fn slots_for(template: &Value, setup: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for app in apps_of(template) {
        if let Some((kind, app_id)) = &app.secret {
            out.push(slot(format!("app:{}", app.id), kind, app.name.clone(), app_id.clone(), &app.icon));
        }
        if app.id == "vscode" {
            for ext_id in strs(setup, "extensions") {
                let ext = catalog::extension(ext_id);
                if let Some(sec) = ext.get("secret").filter(|x| x.is_object()) {
                    let label = format!("{} (VS Code)", s(&ext, "name"));
                    out.push(slot(format!("ext:{ext_id}"), s(sec, "kind"), label, s(sec, "id").into(), &app.icon));
                }
            }
        }
        if app.id == "wadbrowser" {
            for w in js::arr(setup, "webapps") {
                let label = format!("{} (web app)", s(w, "name"));
                let key = format!("web:{}", s(w, "id"));
                out.push(slot(key, signins::WEB, label, web_app_id(s(w, "name")), &webapp_icon(s(w, "url"))));
            }
        }
    }
    out
}

/// A WadBrowser setup's sign-ins: one per web app.
pub fn browser_slots(setup: &Value) -> Vec<Value> {
    js::arr(setup, "webapps")
        .iter()
        .map(|w| {
            let key = format!("web:{}", s(w, "id"));
            slot(key, signins::WEB, s(w, "name").into(), web_app_id(s(w, "name")), &webapp_icon(s(w, "url")))
        })
        .collect()
}

/// A new secret's name: <wadspace name without spaces>-<slot's app id>.
pub fn new_secret_name(wadspace_name: &str, slot: &Value) -> String {
    let name: String = wadspace_name.chars().filter(|c| !js::is_space(*c)).collect();
    format!("{}-{}", if name.is_empty() { "Wadspace" } else { &name }, s(slot, "appId"))
}

/// A new secret's name in a WadBrowser setup: as [`new_secret_name`] after
/// the setup's name, or WadSpaces-WadBrowser.<App> when it has none.
pub fn default_browser_secret_name(setup_name: &str, slot: &Value) -> String {
    if js::trim(setup_name).is_empty() {
        let app = s(slot, "appId");
        format!("WadSpaces-WadBrowser.{}", app.strip_prefix("WadBrowser-").unwrap_or(app))
    } else {
        new_secret_name(setup_name, slot)
    }
}

/// Whether a secret can go in a slot: only one of its kind (a web sign-in
/// in a web app, Obsidian's in Obsidian...).
pub fn fits(secret: &Value, slot: &Value) -> bool {
    let kind = |v: &Value| match s(v, "kind") {
        "wadbrowser" => signins::WEB.to_string(),
        k => k.to_string(),
    };
    kind(secret) == kind(slot)
}

/// The kind of sign-in a slot key takes ("web:<id>", "ext:<id>", "app:<id>").
pub fn kind_of_slot_key(key: &str) -> Option<String> {
    let (prefix, id) = key.split_once(':')?;
    match prefix {
        "web" => Some(signins::WEB.into()),
        "ext" => {
            catalog::extension(id).get("secret").and_then(|x| x.get("kind")).and_then(Value::as_str).map(String::from)
        }
        "app" => catalog::app(id).and_then(|a| a.secret).map(|(k, _)| k),
        _ => None,
    }
}

// ---------------------------------------------------------- desktop, menu

/// Whether anything in the template opens a folder in a window. Without one,
/// a project's desktop icon would only open a terminal there, so projects are
/// mounted with no icon (the image's wadspaces-projects does the same).
pub fn opens_folders(template: &Value) -> bool {
    apps_of(template).iter().any(|a| a.opens_folders)
}

/// An app's name on the desktop. WadBrowser is one app whichever way it
/// opens (focus is its Extensions' setting, not another app).
fn app_label(app: &App, _setup: &Value) -> String {
    app.name.clone()
}

/// What the image puts on the desktop, in selkies-desktop's order: the
/// template's apps, then the web apps, then a folder per mounted project
/// (only where an app opens folders). `{id, label, iconUrl, project?, added?}`;
/// `added`: VS Code's extensions, which the mock images mark with a +.
pub fn desktop_for(template: &Value, setup: &Value, projects: &[Value]) -> Vec<Value> {
    let mut items = Vec::new();
    for app in apps_of(template) {
        let mut item = json!({ "id": format!("app:{}", app.id), "label": app_label(&app, setup), "iconUrl": app.icon });
        let n = js::arr(setup, "extensions").len();
        if app.id == "vscode" && n > 0 {
            item["added"] = n.into();
        }
        items.push(item);
    }
    if has_app(template, "wadbrowser") {
        for w in js::arr(setup, "webapps") {
            items.push(json!({ "id": format!("web:{}", s(w, "id")), "label": s(w, "name"), "iconUrl": webapp_icon(s(w, "url")) }));
        }
    }
    if opens_folders(template) {
        for p in projects {
            items.push(json!({ "id": format!("project:{}", s(p, "id")), "label": s(p, "name"), "iconUrl": "/folder.svg", "project": true }));
        }
    }
    items
}

fn cmp_names(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// selkies-desktop's Start menu: every launcher in the image, by category
/// A–Z (Misc last), each A–Z. `[{name, apps: [{key, name, iconUrl}]}]`.
pub fn start_menu_for(template: &Value, setup: &Value) -> Vec<Value> {
    const BROWSER: &str = "/brand/wadbrowser-icon-dark.svg";
    const FOOT: &str = "/catalog/terminal.png";
    let mut entries: Vec<(String, String, String, String)> = vec![
        ("foot".into(), "Foot".into(), FOOT.into(), "System".into()),
        ("footclient".into(), "Foot Client".into(), FOOT.into(), "System".into()),
        ("foot-server".into(), "Foot Server".into(), FOOT.into(), "System".into()),
    ];
    // WadBrowser is in the base, but in the menu only when the template has it.
    if has_app(template, "wadbrowser") {
        entries.push(("app:wadbrowser".into(), "WadBrowser".into(), BROWSER.into(), "Internet".into()));
    }
    for app in apps_of(template) {
        // The base's own (WadBrowser, the terminal) are listed already.
        if !matches!(app.recipe, Recipe::Builtin { .. }) {
            entries.push((format!("app:{}", app.id), app.name.clone(), app.icon.clone(), app.category.clone()));
        }
    }
    if has_app(template, "wadbrowser") {
        for w in js::arr(setup, "webapps") {
            entries.push((
                format!("web:{}", s(w, "id")),
                s(w, "name").into(),
                webapp_icon(s(w, "url")),
                "Internet".into(),
            ));
        }
    }
    let mut cats: Vec<(String, Vec<Value>)> = Vec::new();
    for (key, name, icon, cat) in entries {
        let app = json!({ "key": key, "name": name, "iconUrl": icon });
        match cats.iter_mut().find(|(c, _)| *c == cat) {
            Some((_, apps)) => apps.push(app),
            None => cats.push((cat, vec![app])),
        }
    }
    for (_, apps) in cats.iter_mut() {
        apps.sort_by(|a, b| cmp_names(s(a, "name"), s(b, "name")));
    }
    cats.sort_by(|(a, _), (b, _)| match (a.as_str(), b.as_str()) {
        ("Misc", "Misc") => Ordering::Equal,
        ("Misc", _) => Ordering::Greater,
        (_, "Misc") => Ordering::Less,
        _ => cmp_names(a, b),
    });
    cats.into_iter().map(|(name, apps)| json!({ "name": name, "apps": apps })).collect()
}

// ----------------------------------------------------- what it starts with

/// The launcher an app's desktop icon copies.
fn launcher(app: &App, setup: &Value) -> Option<String> {
    // One launcher, full or focus as the image's wadbrowser.conf says.
    if app.id == "wadbrowser" {
        let _ = setup;
        return Some("wadbrowser.desktop".into());
    }
    match &app.recipe {
        Recipe::Feature { desktop, .. } => desktop.first().map(|d| d.to_string()),
        Recipe::Apt { desktop, .. } => desktop.map(String::from),
        Recipe::Builtin { desktop } => Some(desktop.to_string()),
    }
}

/// /etc/wadspaces/layout.json: the desktop's icons in order (the template's
/// apps, then the web apps) and which open at boot; wadspaces-layout reads it.
pub fn layout_json_for(template: &Value, setup: &Value) -> String {
    let boot = strs(setup, "autostart");
    let mut icons = Vec::new();
    for app in apps_of(template) {
        let mut e = Obj::new();
        e.insert("app".into(), app.id.clone().into());
        if let Some(d) = launcher(&app, setup) {
            e.insert("desktop".into(), d.into());
        }
        e.insert("label".into(), app_label(&app, setup).into());
        if boot.contains(&format!("app:{}", app.id).as_str()) {
            e.insert("autostart".into(), true.into());
        }
        icons.push(Value::Object(e));
    }
    if has_app(template, "wadbrowser") {
        for w in js::arr(setup, "webapps").iter().filter(|w| is_id(s(w, "id"))) {
            let id = s(w, "id");
            let mut e = Obj::new();
            e.insert("app".into(), format!("webapp-{id}").into());
            e.insert("desktop".into(), format!("wadspaces-webapp-{id}.desktop").into());
            e.insert("label".into(), s(w, "name").into());
            if boot.contains(&format!("web:{id}").as_str()) {
                e.insert("autostart".into(), true.into());
            }
            icons.push(Value::Object(e));
        }
    }
    js::stringify_pretty(&json!({ "version": 1, "icons": icons }), 2) + "\n"
}

/// A marketplace id: publisher.name, nothing a shell would read.
pub fn is_extension_id(id: &str) -> bool {
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    id.len() <= 120 && id.split_once('.').is_some_and(|(a, b)| ok(a) && b.split('.').all(ok))
}

pub(crate) fn wadbrowser_conf(template: &Value, setup: &Value) -> Option<String> {
    let mode = if s(setup, "browserMode") == "focus" { "focus" } else { "full" };
    has_app(template, "wadbrowser").then(|| crate::generator::wadbrowser_conf(&json!({ "defaultBrowser": mode })))?
}

/// The extension a wallpaper is stored with, as WadSpaces draws it
/// (lib/wallpaperRender.ts): a picture is a JPEG, a gradient or colour a PNG.
pub fn wallpaper_ext(wallpaper: &Value) -> &'static str {
    if s(wallpaper, "type") == "image" { "jpg" } else { "png" }
}

// ------------------------------------------------------- installing one

/// One sign-in a wadspace mounts, as wadd takes it.
fn signin_entry(r: &Value, app: &str, web: bool, kind: &str, slot: &str) -> Value {
    let mut e = Obj::new();
    e.insert("id".into(), s(r, "id").into());
    e.insert("app".into(), app.into());
    if web {
        e.insert("web".into(), true.into());
    }
    e.insert("name".into(), s(r, "name").into());
    e.insert("kind".into(), kind.into());
    e.insert("slot".into(), slot.into());
    Value::Object(e)
}

/// What WadBrowser's key group member signs in: the browser itself
/// ("@browser", its windows that aren't a web app's) or a web app (its id).
pub const BROWSER_APP: &str = "@browser";

/// A version 3 wadspace's sign-ins: its keys by slot (apps and VS Code's
/// extensions), then WadBrowser's groups, each member signing in with its
/// group's key. Keys for what the stack doesn't have are left out.
pub(crate) fn signins_v3(w: &Value, template: &Value, setup: &Value) -> Vec<Value> {
    let secrets = &w["customization"]["secrets"];
    let mut out = Vec::new();
    for sl in slots_for(template, setup) {
        let key = s(&sl, "key");
        if key.starts_with("web:") {
            continue;
        }
        let Some(r) = secrets.get("bySlot").and_then(|x| x.get(key)).filter(|r| !s(r, "id").is_empty()) else {
            continue;
        };
        let app = if key.starts_with("ext:") { "vscode" } else { key.split_once(':').map_or(key, |(_, a)| a) };
        out.push(signin_entry(r, app, false, s(&sl, "kind"), key));
    }
    for g in key_groups_of(template, setup, &secrets["groups"]) {
        let Some(r) = g.get("key").filter(|r| !s(r, "id").is_empty()) else { continue };
        // Each member its own slot (a web app's as in version 2), all with
        // the group's key: wadd mounts it once, for each of them.
        for m in strs(&g, "members") {
            let (app, slot) = match m {
                "browser" => (BROWSER_APP, "app:wadbrowser"),
                web => (web.strip_prefix("web:").unwrap_or(web), web),
            };
            out.push(signin_entry(r, app, true, signins::WEB, slot));
        }
    }
    out
}

/// The workspace JSON every version installs as (workspace_for's, and
/// version 4's): `own` has its name, wallpaper and title bars.
pub(crate) fn workspace_value(
    id: &str,
    own: &Value,
    image: String,
    signins: Vec<Value>,
    projects: Option<Vec<Value>>,
    opts: &Value,
) -> Value {
    let tz = match s(opts, "tz") {
        "" => "Etc/UTC",
        tz => tz,
    };
    let mut env = Obj::new();
    for (k, v) in [("PUID", "1000"), ("PGID", "1000"), ("TZ", tz)] {
        env.insert(k.into(), v.into());
    }
    env.insert("TITLE".into(), one_line(s(own, "name")).into());
    let wp = own.get("wallpaper").unwrap_or(&Value::Null);
    env.insert("WALLPAPER".into(), format!("{WALLPAPER_DIR}/wallpaper.{}", wallpaper_ext(wp)).into());
    // Filling the screen, as the Builder's preview shows it (the base centres).
    env.insert("WALLPAPER_MODE".into(), "fill".into());
    // Title bars on its windows (its Look's switch; the base has none).
    if own.get("titleBars").and_then(Value::as_bool) == Some(true) {
        env.insert("WADSPACES_TITLEBARS".into(), "1".into());
    }
    let mut out = Obj::new();
    out.insert("id".into(), id.into());
    out.insert("name".into(), one_line(s(own, "name")).into());
    out.insert("image".into(), image.into());
    out.insert("display".into(), "host".into());
    out.insert("enabled".into(), true.into());
    out.insert("env".into(), Value::Object(env));
    out.insert("secrets".into(), opts.get("secrets").filter(|x| x.is_array()).cloned().unwrap_or(json!([])));
    out.insert("volumes".into(), json!([format!("wad-{id}-config:/config:z")]));
    out.insert("devices".into(), json!(["/dev/dri"]));
    out.insert("shm_size".into(), "1g".into());
    out.insert("signins".into(), Value::Array(signins));
    if let Some(p) = projects {
        out.insert("projects".into(), Value::Array(p));
    }
    out.insert("hotkey".into(), Value::Null);
    out.insert("autostart".into(), false.into());
    Value::Object(out)
}

/// A stack's projects as wadd takes them: `[{id, kind, mount, repo?, ref?,
/// setup?, from?, mounted?}]`. A GitHub one is cloned by the wadspace (or,
/// `mounted`, on the machine by wadd, and mounted in); a copy is copied in
/// once (`from`: where its files are, for a machine that hasn't
/// copied them yet); a mount is the machine's folder or file at `from`,
/// mounted read-write. Anything else is left out.
pub(crate) fn workspace_projects(custom: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for p in js::arr(custom, "projects") {
        let mut e = Obj::new();
        e.insert("id".into(), s(p, "id").into());
        match s(p, "kind") {
            "github" => {
                e.insert("kind".into(), "github".into());
                e.insert("mount".into(), s(p, "as").into());
                e.insert("repo".into(), s(p, "repo").into());
                if !s(p, "ref").is_empty() {
                    e.insert("ref".into(), s(p, "ref").into());
                }
                if !s(p, "setup").is_empty() {
                    e.insert("setup".into(), s(p, "setup").into());
                }
                if p["mounted"] == Value::Bool(true) {
                    e.insert("mounted".into(), true.into());
                }
            }
            k @ ("copy" | "mount") => {
                e.insert("kind".into(), k.into());
                e.insert("mount".into(), s(p, "as").into());
                let src = &p["source"];
                let mut from = Obj::new();
                if !s(src, "machineId").is_empty() {
                    from.insert("machineId".into(), s(src, "machineId").into());
                }
                if src.get("drive").is_some_and(Value::is_object) {
                    from.insert("drive".into(), src["drive"].clone());
                }
                from.insert("paths".into(), json!(strs(src, "paths")));
                e.insert("from".into(), Value::Object(from));
            }
            _ => continue,
        }
        out.push(Value::Object(e));
    }
    out
}

// -------------------------------------------------------- WadBrowser runs

/// Web apps used anywhere already, to add to a WadBrowser setup in a click.
pub fn known_webapps(customizations: &[Value], browsers: &[Value]) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    for w in customizations.iter().chain(browsers).flat_map(|x| js::arr(x, "webapps")) {
        if !seen.iter().any(|x| x["url"] == w["url"]) {
            seen.push(w.clone());
        }
    }
    seen
}

/// Whether a name is already taken in this list (but `except`, the one being renamed).
pub fn named(name: &str, taken: &[Value], except: Option<&str>) -> bool {
    let n = js::trim(name).to_lowercase();
    taken.iter().any(|x| Some(s(x, "id")) != except && js::trim(s(x, "name")).to_lowercase() == n)
}

#[cfg(test)]
mod tests;
