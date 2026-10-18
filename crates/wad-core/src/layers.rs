//! The Builder's layers, and the rules between them (the UI's
//! src/core/layers.ts wraps these). A wadspace is a stack, bottom to top:
//!
//!   Template       the base image plus apps. Its own image,
//!                  localhost/wadspaces-template-<id>:latest, FROM the base,
//!                  or FROM another template's (its `parent`): a template can
//!                  build on one whose apps it all has, adding at least one.
//!   Extensions     a template's apps set up: WadBrowser's mode and web apps,
//!                  VS Code's extensions, tools, what opens at boot. Its own
//!                  image, FROM the template's. (Its image key is still
//!                  "custom:<id>" and its image localhost/wadspaces-custom-
//!                  <id>:latest, from when this layer was "customization".)
//!   Customization  the wadspace's own: name, wallpaper, projects and keys
//!                  (saved sign-ins). No image: wadd hands those over when
//!                  it starts, and the wadspace clones its GitHub projects
//!                  and copies in the rest (nothing is mounted). Kept in the
//!                  wadspace document (version 3).
//!
//! What a wadspace ends up with, which sign-ins it can keep and which secrets
//! fit them, its desktop and Start menu as the image draws them, each layer's
//! Dockerfile and build folder, and turning a version 2 wadspace (its base a
//! template or extensions, its customization's fields its own) into a stack.
//! Values are JSON, as everywhere in the core.

use std::cmp::Ordering;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::catalog::{self, App, Recipe};
use crate::generator::q;
use crate::js::{self, Obj};
use crate::spec::{DEFAULT_BASE_IMAGE, FEATURES, is_id};
use crate::{signins, url};

/// Where wadd puts a wadspace's wallpaper inside it (its extra folder).
pub const WALLPAPER_DIR: &str = "/run/wadspaces-extra";

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn strs<'a>(v: &'a Value, k: &str) -> Vec<&'a str> {
    js::arr(v, k).iter().filter_map(Value::as_str).collect()
}

fn deleted(v: &Value) -> bool {
    v.get("deletedAt").is_some_and(js::truthy)
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

/// A template on its own: WadBrowser with an address bar, nothing added.
pub fn empty_setup(template_id: &str) -> Value {
    json!({ "templateId": template_id, "browserMode": "browser", "webapps": [], "extensions": [], "tools": [], "autostart": [] })
}

/// Just the setup part of a customization (or a template's defaults).
pub fn setup_of(c: &Value) -> Value {
    let or_list = |k: &str| c.get(k).filter(|v| v.is_array()).cloned().unwrap_or(json!([]));
    json!({
        "templateId": s(c, "templateId"),
        "browserMode": if s(c, "browserMode") == "focus" { "focus" } else { "browser" },
        "webapps": or_list("webapps"),
        "extensions": or_list("extensions"),
        "tools": or_list("tools"),
        "autostart": or_list("autostart"),
    })
}

/// Whether two setups set the same things (a customization unchanged from its base).
pub fn same_setup(a: &Value, b: &Value) -> bool {
    setup_of(a) == setup_of(b)
}

/// A web app's id from its name: "Google Cloud" → "google-cloud".
pub fn webapp_id(name: &str) -> String {
    let lower = name.to_lowercase();
    let id = js::replace_runs(&lower, |c| !(c.is_ascii_lowercase() || c.is_ascii_digit()), "-");
    let id = js::ascii_prefix(js::trim_dashes(&id), 60);
    let id = js::trim_dashes(id);
    if id.is_empty() { "app".into() } else { id.into() }
}

/// "template:<id>" or "custom:<id>": the image a base runs on.
pub fn image_key(base: &Value) -> String {
    let prefix = if s(base, "kind") == "customization" { "custom" } else { "template" };
    format!("{prefix}:{}", s(base, "id"))
}

/// The image an image key names.
pub fn image_ref(key: &str) -> String {
    match key.split_once(':') {
        Some(("custom", id)) => format!("localhost/wadspaces-custom-{id}:latest"),
        Some((_, id)) => format!("localhost/wadspaces-template-{id}:latest"),
        None => format!("localhost/wadspaces-template-{key}:latest"),
    }
}

/// What a base (template or customization) has, with its template:
/// `{template, setup, customization?}`, or null when either is missing.
/// Deleted ones still resolve: a wadspace on one keeps working until it's
/// deleted for good.
pub fn resolve(lib: &Value, base: &Value) -> Value {
    let find = |coll: &str, id: &str| js::arr(lib, coll).iter().find(|x| s(x, "id") == id).cloned();
    if s(base, "kind") != "customization" {
        return match find("templates", s(base, "id")) {
            Some(t) => json!({ "template": t, "setup": empty_setup(s(&t, "id")) }),
            None => Value::Null,
        };
    }
    let Some(c) = find("customizations", s(base, "id")) else { return Value::Null };
    match find("templates", s(&c, "templateId")) {
        Some(t) => json!({ "template": t, "setup": setup_of(&c), "customization": c }),
        None => Value::Null,
    }
}

// ------------------------------------------------------- stacks (version 3)

/// How deep templates can stack.
pub const MAX_CHAIN: usize = 8;

/// A template and what it builds on, bottom first: walks `parent` through
/// the library's templates. A missing parent ends the walk (the chain then
/// starts with a template that has a `parent`; [`chain_ok`] says so), and so
/// does a loop or [`MAX_CHAIN`].
pub fn chain_of(lib: &Value, template_id: &str) -> Vec<Value> {
    let find = |id: &str| js::arr(lib, "templates").iter().find(|t| s(t, "id") == id).cloned();
    let mut chain: Vec<Value> = Vec::new();
    let mut next = find(template_id);
    while let Some(t) = next {
        if chain.len() > MAX_CHAIN || chain.iter().any(|c| s(c, "id") == s(&t, "id")) {
            break;
        }
        next = match s(&t, "parent") {
            "" => None,
            pid => find(pid),
        };
        chain.push(t);
    }
    chain.reverse();
    chain
}

/// Why a chain (bottom first) can't be built, or None: each template builds
/// on the one below it, has all of its apps and adds at least one.
pub fn chain_ok(chain: &[Value]) -> Option<String> {
    let first = chain.first()?;
    if !s(first, "parent").is_empty() {
        return Some(format!("{} builds on a template that's gone", one_line(s(first, "name"))));
    }
    if chain.len() > MAX_CHAIN {
        return Some(format!("Templates stack {MAX_CHAIN} deep at most"));
    }
    for pair in chain.windows(2) {
        let (lower, upper) = (&pair[0], &pair[1]);
        let (lname, uname) = (one_line(s(lower, "name")), one_line(s(upper, "name")));
        if s(upper, "parent") != s(lower, "id") {
            return Some(format!("{uname} doesn't build on {lname}"));
        }
        let have = strs(upper, "apps");
        let missing: Vec<&str> = strs(lower, "apps").into_iter().filter(|a| !have.contains(a)).collect();
        if !missing.is_empty() {
            let names: Vec<String> =
                missing.iter().map(|id| catalog::app(id).map(|a| a.name).unwrap_or_else(|| id.to_string())).collect();
            return Some(format!("{uname} can't build on {lname}: it doesn't have {}", names.join(", ")));
        }
        if have.len() <= strs(lower, "apps").len() {
            return Some(format!("{uname} adds no apps to {lname}"));
        }
    }
    None
}

/// What a version 3 wadspace is built from: `{chain, template, setup,
/// extensions?}` (`template` the top of the chain), or null with no template
/// (or one that's gone). `lib`: `{templates, extensions}`. An Extensions
/// layer made on another template doesn't count (the stack needs fixing).
pub fn resolve_stack(lib: &Value, w: &Value) -> Value {
    let chain = chain_of(lib, s(w, "template"));
    let Some(t) = chain.last().cloned() else { return Value::Null };
    let tid = s(&t, "id").to_string();
    let ext = js::arr(lib, "extensions").iter().find(|e| s(e, "id") == s(w, "extensions") && s(e, "templateId") == tid);
    match ext {
        Some(e) => json!({ "chain": chain, "template": t, "setup": setup_of(e), "extensions": e }),
        None => json!({ "chain": chain, "template": t, "setup": empty_setup(&tid) }),
    }
}

/// The image a version 3 wadspace runs: its Extensions layer's, else its
/// template's ("" with neither).
pub fn stack_image_key(w: &Value) -> String {
    match (s(w, "extensions"), s(w, "template")) {
        ("", "") => String::new(),
        ("", t) => format!("template:{t}"),
        (e, _) => format!("custom:{e}"),
    }
}

/// A layer's image on a machine, as [`image_state`] reads it: podman's id
/// for it, the id of the image it was built FROM, and the sha of the build
/// folder it was built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerImage {
    pub id: String,
    pub from: String,
    pub sha: String,
}

/// A layer's image against the library (the web app's imageState).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// Never built here.
    Never,
    /// Its layer (or one under it) changed since.
    Changed,
    /// Built on an older base or parent: still runs, an update is there.
    Update,
    Built,
}

/// The state of the image for layer `key` ("template:<id>" or
/// "custom:<id>"), given the machine's layer images by key ("base" for the
/// base image). A layer the library doesn't have counts as built.
pub fn image_state(key: &str, lib: &Value, images: &std::collections::HashMap<String, LayerImage>) -> ImageState {
    let Some(row) = images.get(key) else { return ImageState::Never };
    let under = |parent: Option<String>, sha: &str| -> ImageState {
        if row.sha != sha {
            return ImageState::Changed;
        }
        let Some(pk) = parent else {
            let base = images.get("base");
            return if base.is_some_and(|b| !row.from.is_empty() && b.id != row.from) {
                ImageState::Update
            } else {
                ImageState::Built
            };
        };
        match image_state(&pk, lib, images) {
            ImageState::Changed | ImageState::Never => ImageState::Changed,
            p => match images.get(&pk) {
                Some(parent) if parent.id == row.from => p,
                _ => ImageState::Update,
            },
        }
    };
    let find = |coll: &str, id: &str| js::arr(lib, coll).iter().find(|x| s(x, "id") == id).cloned();
    if let Some(tid) = key.strip_prefix("template:") {
        let Some(t) = find("templates", tid) else { return ImageState::Built };
        let chain = chain_of(lib, tid);
        let parent = (chain.len() > 1).then(|| chain[chain.len() - 2].clone());
        let sha = template_layer(&t, parent.as_ref().unwrap_or(&Value::Null))["sha"].as_str().unwrap_or("").to_string();
        return under(parent.map(|p| format!("template:{}", s(&p, "id"))), &sha);
    }
    let Some(e) = key.strip_prefix("custom:").and_then(|id| find("extensions", id)) else { return ImageState::Built };
    let Some(t) = find("templates", s(&e, "templateId")) else { return ImageState::Built };
    let sha = extensions_layer(&e, &t)["sha"].as_str().unwrap_or("").to_string();
    under(Some(format!("template:{}", s(&t, "id"))), &sha)
}

/// Why a stack can't be installed or built yet (the web app's notReady).
pub fn not_ready(w: &Value, lib: &Value) -> Option<&'static str> {
    if s(w, "template").is_empty() {
        return Some("It needs a template with apps.");
    }
    if resolve_stack(lib, w).is_null() {
        return Some("Its template is gone.");
    }
    if !w["customization"].is_object() {
        return Some("It needs a customization.");
    }
    if s(&w["customization"], "name").trim().is_empty() {
        return Some("Its customization needs a name.");
    }
    None
}

/// A layer's build request (wadd's POST /v1/images/build): a template's,
/// with those it builds on, or an Extensions layer's, with its template and
/// those under it.
pub fn layer_request(key: &str, lib: &Value) -> Result<Value, String> {
    let on = |tid: &str| -> Result<(Value, Vec<Value>), String> {
        let chain = chain_of(lib, tid);
        let (t, parents) = chain.split_last().ok_or("That template is gone.")?;
        Ok((t.clone(), parents.to_vec()))
    };
    if let Some(tid) = key.strip_prefix("template:") {
        let (template, parents) = on(tid)?;
        return Ok(json!({ "layer": key, "template": template, "parents": parents, "extensions": null }));
    }
    let id = key.strip_prefix("custom:").unwrap_or(key);
    let e =
        js::arr(lib, "extensions").iter().find(|x| s(x, "id") == id).cloned().ok_or("Those Extensions are gone.")?;
    let (template, parents) = on(s(&e, "templateId"))?;
    Ok(json!({ "layer": key, "template": template, "parents": parents, "extensions": e }))
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

/// Who uses each saved sign-in: `{<sign-in id>: [{kind, id, name, slot}]}`
/// over wadspaces and WadBrowser setups (not deleted ones).
pub fn secret_use(wadspaces: &[Value], browsers: &[Value]) -> Value {
    let mut out = Obj::new();
    let lists = [("wadspace", wadspaces), ("browser", browsers)];
    for (kind, list) in lists {
        for x in list.iter().filter(|x| !deleted(x)) {
            for (slot, r) in x.get("secrets").and_then(Value::as_object).into_iter().flatten() {
                let Some(id) = r.get("id").and_then(Value::as_str) else { continue };
                let e = out.entry(id.to_string()).or_insert_with(|| json!([]));
                e.as_array_mut().expect("a list").push(json!({
                    "kind": kind, "id": s(x, "id"), "name": s(x, "name"), "slot": slot,
                }));
            }
        }
    }
    Value::Object(out)
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

/// What can open at boot: the template's apps and the web apps. `[{key, name, iconUrl}]`.
pub fn bootables(template: &Value, setup: &Value) -> Vec<Value> {
    desktop_for(template, setup, &[])
        .into_iter()
        .map(|i| json!({ "key": i["id"], "name": i["label"], "iconUrl": i["iconUrl"] }))
        .collect()
}

// ------------------------------------------------------------ Dockerfiles

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

/// Features in install order, from these names (unknown ones are dropped).
fn in_order<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let want: Vec<&str> = names.into_iter().collect();
    FEATURES.iter().copied().filter(|f| want.contains(f)).collect()
}

/// A marketplace id: publisher.name, nothing a shell would read.
pub fn is_extension_id(id: &str) -> bool {
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    id.len() <= 120 && id.split_once('.').is_some_and(|(a, b)| ok(a) && b.split('.').all(ok))
}

/// The image a template builds FROM: its parent template's, else the base.
fn template_from(parent: &Value) -> String {
    match s(parent, "id") {
        "" => DEFAULT_BASE_IMAGE.into(),
        id => image_ref(&format!("template:{id}")),
    }
}

/// The template's image: the base plus its apps, or (with a `parent`, null
/// for none) the parent template's image plus the apps it adds.
pub fn template_dockerfile(t: &Value, parent: &Value) -> String {
    let all = apps_of(t);
    let had = strs(parent, "apps");
    let apps: Vec<&App> = all.iter().filter(|a| !had.contains(&a.id.as_str())).collect();
    let name = one_line(s(t, "name"));
    let mut label = format!(
        "LABEL io.wadspaces.layer={} io.wadspaces.apps={}",
        q(&format!("template:{}", s(t, "id"))),
        q(&all.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(","))
    );
    let first = match s(parent, "id") {
        "" => format!("# {name} template. Generated by WadSpaces Client."),
        pid => {
            label.push_str(&format!(" io.wadspaces.parent={}", q(&format!("template:{pid}"))));
            format!("# {name} template, built on {}. Generated by WadSpaces Client.", one_line(s(parent, "name")))
        }
    };
    let mut lines = vec![
        first,
        format!("ARG BASE_IMAGE={}", template_from(parent)),
        "FROM ${BASE_IMAGE}".into(),
        String::new(),
        format!("ENV TITLE={}", q(&name)),
        label,
        String::new(),
    ];
    // A parent's features are installed already (wadspaces-feature leaves a
    // marker for each, which this image inherits), so only the new apps'.
    let features = in_order(apps.iter().filter_map(|a| match a.recipe {
        Recipe::Feature { feature, .. } => Some(feature),
        _ => None,
    }));
    if !features.is_empty() {
        lines.push(format!("RUN wadspaces-feature {}", features.join(" ")));
        lines.push(String::new());
    }
    let apt: Vec<String> = apps
        .iter()
        .filter_map(|a| match a.recipe {
            Recipe::Apt { packages, desktop } => {
                let mut parts = vec!["wadspaces-apt".to_string(), a.id.clone()];
                if let Some(d) = desktop {
                    parts.extend(["--desktop".into(), d.to_string()]);
                }
                parts.extend(packages.iter().map(|p| p.to_string()));
                Some(parts.join(" "))
            }
            _ => None,
        })
        .collect();
    if !apt.is_empty() {
        // One RUN: apt's lists are fetched once, and --clean drops them from the layer.
        let mut cmds = apt;
        cmds.push("wadspaces-apt --clean".into());
        lines.push(format!("RUN {}", cmds.join(" && \\\n    ")));
        lines.push(String::new());
    }
    lines.push("COPY root/ /".into());
    lines.join("\n") + "\n"
}

/// An Extensions layer's image: its template's, with the apps set up.
pub fn extensions_dockerfile(c: &Value, t: &Value) -> String {
    let name = one_line(s(c, "name"));
    let mut lines = vec![
        format!("# {name}: {}'s extensions. Generated by WadSpaces Client.", one_line(s(t, "name"))),
        format!("ARG BASE_IMAGE={}", image_ref(&format!("template:{}", s(t, "id")))),
        "FROM ${BASE_IMAGE}".into(),
        String::new(),
        format!("LABEL io.wadspaces.layer={}", q(&format!("custom:{}", s(c, "id")))),
        String::new(),
    ];
    let tools = in_order(strs(c, "tools"));
    if !tools.is_empty() {
        lines.push(format!("RUN wadspaces-feature {}", tools.join(" ")));
        lines.push(String::new());
    }
    let exts: Vec<&str> = strs(c, "extensions").into_iter().filter(|e| is_extension_id(e)).collect();
    if has_app(t, "vscode") && !exts.is_empty() {
        // In the image, so they're there on first start; the image's init
        // copies them into ~/.vscode/extensions, where they can be updated.
        lines.push(format!("RUN wadspaces-vscode-extensions {}", exts.join(" ")));
        lines.push(String::new());
    }
    let webapps: Vec<String> = js::arr(c, "webapps")
        .iter()
        .filter(|w| is_id(s(w, "id")))
        .map(|w| {
            format!("wadspaces-webapp --id {} {} {}", s(w, "id"), q(&one_line(s(w, "name"))), q(&one_line(s(w, "url"))))
        })
        .collect();
    if has_app(t, "wadbrowser") && !webapps.is_empty() {
        lines.push(format!("RUN {}", webapps.join(" && \\\n    ")));
        lines.push(String::new());
    }
    // After the web apps: root/ holds their icons (made by wadd before the build).
    lines.push("COPY root/ /".into());
    lines.join("\n") + "\n"
}

fn wadbrowser_conf(template: &Value, setup: &Value) -> Option<String> {
    let mode = if s(setup, "browserMode") == "focus" { "focus" } else { "full" };
    has_app(template, "wadbrowser").then(|| crate::generator::wadbrowser_conf(&json!({ "defaultBrowser": mode })))?
}

fn layer(key: String, from: String, files: Vec<(String, String)>) -> Value {
    let sha = layer_sha(&files);
    let files: Vec<Value> = files.into_iter().map(|(p, c)| json!({ "path": p, "content": c })).collect();
    json!({ "key": key, "ref": image_ref(&key), "from": from, "files": files, "sha": sha })
}

/// A template's build folder: `{key, ref, from, files: [{path, content}], sha}`.
/// `sha` labels the built image, so a later Dockerfile can be told apart.
/// `parent`: the template it builds on, or null. The desktop's layout is the
/// whole template's (its parent's apps too), in its order.
pub fn template_layer(t: &Value, parent: &Value) -> Value {
    let setup = empty_setup(s(t, "id"));
    let mut files = vec![
        ("Dockerfile".to_string(), template_dockerfile(t, parent)),
        ("root/etc/wadspaces/layout.json".into(), layout_json_for(t, &setup)),
    ];
    if let Some(conf) = wadbrowser_conf(t, &setup) {
        files.push(("root/etc/wadspaces/wadbrowser.conf".into(), conf));
    }
    layer(format!("template:{}", s(t, "id")), template_from(parent), files)
}

/// An Extensions layer's build folder, FROM its template's image. (wadd
/// adds the web apps' icons under root/; they follow from the web apps'
/// sites, which the Dockerfile already has, so they aren't in the sha.)
pub fn extensions_layer(c: &Value, t: &Value) -> Value {
    let mut files = vec![
        ("Dockerfile".to_string(), extensions_dockerfile(c, t)),
        ("root/etc/wadspaces/layout.json".into(), layout_json_for(t, c)),
    ];
    if let Some(conf) = wadbrowser_conf(t, c) {
        files.push(("root/etc/wadspaces/wadbrowser.conf".into(), conf));
    }
    layer(format!("custom:{}", s(c, "id")), image_ref(&format!("template:{}", s(t, "id"))), files)
}

/// SHA-256 of a build folder's files (path, then content), as hex.
pub fn layer_sha(files: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    for (path, content) in files {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(content.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The extension a wallpaper is stored with, as WadSpaces draws it
/// (lib/wallpaperRender.ts): a picture is a JPEG, a gradient or colour a PNG.
pub fn wallpaper_ext(wallpaper: &Value) -> &'static str {
    if s(wallpaper, "type") == "image" { "jpg" } else { "png" }
}

/// What wadd mounts when the wadspace starts (none of it is in an image), for
/// the Image tab: its wallpaper, project folders and secrets.
pub fn run_summary(w: &Value, projects: &[Value], slots: &[Value]) -> Vec<String> {
    let wp = w.get("wallpaper").unwrap_or(&Value::Null);
    let mut out = vec![format!(
        "wallpaper  → {WALLPAPER_DIR}/wallpaper.{} ({})",
        wallpaper_ext(wp),
        if s(wp, "type").is_empty() { "color" } else { s(wp, "type") }
    )];
    for id in strs(w, "projects") {
        if let Some(p) = projects.iter().find(|p| s(p, "id") == id) {
            out.push(format!("project    {} → ~/Desktop/{}", s(p, "name"), s(p, "mountName")));
        }
    }
    let mut by_id: Vec<(String, String, Vec<String>)> = Vec::new();
    for sl in slots {
        let Some(r) = w.get("secrets").and_then(|x| x.get(s(sl, "key"))) else { continue };
        let label = s(sl, "label").to_string();
        match by_id.iter_mut().find(|(id, ..)| id == s(r, "id")) {
            Some((.., users)) => users.push(label),
            None => by_id.push((s(r, "id").into(), s(r, "name").into(), vec![label])),
        }
    }
    for (_, name, users) in by_id {
        out.push(format!("secret     {name} → {}", users.join(", ")));
    }
    out
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

/// A version 2 wadspace's sign-ins: one per slot it keeps one in.
fn signins_v2(w: &Value, template: &Value, setup: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for sl in slots_for(template, setup) {
        let key = s(&sl, "key");
        let Some(r) = w.get("secrets").and_then(|x| x.get(key)).filter(|r| !s(r, "id").is_empty()) else { continue };
        let (prefix, rest) = key.split_once(':').unwrap_or(("", key));
        // The app the sign-in is for: a web app's id (its WadBrowser window is
        // wadspaces-webapp-<id>), VS Code for an extension's, else the app.
        let app = if prefix == "ext" { "vscode" } else { rest };
        out.push(signin_entry(r, app, prefix == "web", s(&sl, "kind"), key));
    }
    out
}

/// A version 3 wadspace's sign-ins: its keys by slot (apps and VS Code's
/// extensions), then WadBrowser's groups, each member signing in with its
/// group's key. Keys for what the stack doesn't have are left out.
fn signins_v3(w: &Value, template: &Value, setup: &Value) -> Vec<Value> {
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

/// The wadd workspace a wadspace installs as on a machine: its layers' image,
/// its name and wallpaper at run time, a /config of its own, and its secrets. No
/// Super+number (that follows what's running) and no start with the machine.
/// `template` and `setup` are what it's built from (the top of its chain, and
/// its Extensions' setup or the template's). `opts`: `{tz, secrets?: [podman
/// secret names]}`.
pub fn workspace_for(w: &Value, template: &Value, setup: &Value, opts: &Value) -> Value {
    let id = s(w, "id");
    let v3 = w.get("version").and_then(Value::as_u64) == Some(3);
    // Version 3 keeps its own in its customization.
    let own = if v3 { &w["customization"] } else { w };
    let image = if v3 {
        image_ref(&stack_image_key(w))
    } else {
        image_ref(&image_key(&w.get("base").cloned().unwrap_or(Value::Null)))
    };
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
    let signins = if v3 { signins_v3(w, template, setup) } else { signins_v2(w, template, setup) };
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
    if v3 {
        out.insert("projects".into(), Value::Array(workspace_projects(own)));
    }
    out.insert("hotkey".into(), Value::Null);
    out.insert("autostart".into(), false.into());
    Value::Object(out)
}

/// A stack's projects as wadd takes them: `[{id, kind, mount, repo?, ref?,
/// setup?, from?}]`. A GitHub one is cloned by the wadspace; a copy is
/// copied in once (`from`: where its files are, for a machine that hasn't
/// copied them yet); a mount is the machine's folder or file at `from`,
/// mounted read-write. Anything else is left out.
fn workspace_projects(custom: &Value) -> Vec<Value> {
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

// ------------------------------------------------------ version 2 → 3

/// A version 2 wadspace as a version 3 stack (anything else comes back as
/// it is). `lib`: `{templates, customizations}` as they were; `projects`: the
/// project documents its `projects` named.
///
/// - `base` becomes `template` (and `extensions`, for a customization).
/// - Name, description, wallpaper, projects and secrets move into its
///   `customization`.
/// - Projects become the stack's own: a GitHub one is cloned into the
///   wadspace (`{kind: "github", repo, ref?, as, setup?}`); a folder or a
///   drive's is copied in once (`{kind: "copy", source, as}`, no size yet:
///   wadd copies it the first time the wadspace starts where it is).
/// - Web apps' secrets become WadBrowser's key groups: one group per sign-in,
///   and first a group without a key for the browser and the web apps that
///   kept none. Every other secret stays by its slot.
///
/// `order` is left to the caller (it orders all of them).
pub fn migrate_v2(w: &Value, lib: &Value, projects: &[Value]) -> Value {
    if w.get("version").and_then(Value::as_u64) != Some(2) {
        return w.clone();
    }
    let base = &w["base"];
    let custom = s(base, "kind") == "customization";
    let found = resolve(lib, base);
    let template_id = match (&found, custom) {
        (Value::Object(_), _) => s(&found["template"], "id").to_string(),
        (_, false) => s(base, "id").to_string(),
        _ => js::arr(lib, "customizations")
            .iter()
            .find(|c| s(c, "id") == s(base, "id"))
            .map(|c| s(c, "templateId").to_string())
            .unwrap_or_default(),
    };
    let mut out = Obj::new();
    out.insert("version".into(), 3.into());
    for k in ["id", "owner", "sharedWith"] {
        if let Some(v) = w.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    if !template_id.is_empty() {
        out.insert("template".into(), template_id.into());
    }
    if custom {
        out.insert("extensions".into(), s(base, "id").into());
    }

    let mut moved = Vec::new();
    for pid in strs(w, "projects") {
        let Some(p) = projects.iter().find(|p| s(p, "id") == pid) else { continue };
        let src = &p["source"];
        let as_ = s(p, "mountName");
        let mut e = Obj::new();
        e.insert("id".into(), pid.into());
        match s(src, "kind") {
            "git" => {
                e.insert("kind".into(), "github".into());
                e.insert("repo".into(), crate::projects::repo_full_name(s(src, "url")).into());
                if !s(src, "ref").is_empty() {
                    e.insert("ref".into(), s(src, "ref").into());
                }
            }
            "folder" => {
                e.insert("kind".into(), "copy".into());
                e.insert("source".into(), json!({ "machineId": s(src, "machineId"), "paths": [s(src, "path")] }));
            }
            "drive" => {
                e.insert("kind".into(), "copy".into());
                let drive = json!({ "uuid": s(src, "uuid"), "label": s(src, "label"), "fstype": s(src, "fstype") });
                e.insert("source".into(), json!({ "drive": drive, "paths": [s(src, "subpath")] }));
            }
            _ => continue,
        }
        e.insert("as".into(), as_.into());
        if !s(p, "setup").is_empty() && s(src, "kind") == "git" {
            e.insert("setup".into(), s(p, "setup").into());
        }
        moved.push(Value::Object(e));
    }

    let mut by_slot = Obj::new();
    let mut groups: Vec<Value> = Vec::new();
    let mut keyless: Vec<Value> = Vec::new();
    let setup = found.get("setup").cloned().unwrap_or(Value::Null);
    let template = found.get("template").cloned().unwrap_or(Value::Null);
    let secrets = w.get("secrets").and_then(Value::as_object);
    for (slot, r) in secrets.into_iter().flatten() {
        if slot.starts_with("web:") {
            continue;
        }
        by_slot.insert(slot.clone(), r.clone());
    }
    for item in key_items(&template, &setup) {
        match secrets.and_then(|x| x.get(&item)).filter(|r| !s(r, "id").is_empty()) {
            Some(r) => match groups.iter_mut().find(|g| s(&g["key"], "id") == s(r, "id")) {
                Some(g) => g["members"].as_array_mut().expect("a list").push(item.into()),
                None => groups.push(json!({ "id": format!("g-{}", s(r, "id")), "members": [item], "key": r })),
            },
            None => keyless.push(item.into()),
        }
    }
    if !groups.is_empty() && !keyless.is_empty() {
        groups.insert(0, json!({ "id": "all", "members": keyless }));
    }
    let mut keys = json!({ "bySlot": by_slot });
    if !groups.is_empty() {
        keys["groups"] = Value::Array(groups);
    }

    out.insert(
        "customization".into(),
        json!({
            "name": s(w, "name"),
            "description": s(w, "description"),
            "wallpaper": w.get("wallpaper").cloned().unwrap_or(Value::Null),
            "projects": moved,
            "secrets": keys,
        }),
    );
    for k in ["createdAt", "updatedAt", "deletedAt"] {
        if let Some(v) = w.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    Value::Object(out)
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

// ------------------------------------------------------------------ stacks

fn by_time(mut xs: Vec<Value>) -> Vec<Value> {
    xs.sort_by(|a, b| s(a, "createdAt").cmp(s(b, "createdAt")));
    xs
}

/// Each template with what's been built on it, for Step 1's stacks (deleted
/// ones are left out: they're under "Recently deleted"). `[{template,
/// customizations: [{customization, wadspaces}], wadspaces}]`, oldest first.
pub fn stacks(lib: &Value) -> Vec<Value> {
    let live = |k: &str| js::arr(lib, k).iter().filter(|x| !deleted(x)).cloned().collect::<Vec<_>>();
    let (customizations, wadspaces) = (live("customizations"), live("wadspaces"));
    let on = |kind: &str, id: &str| {
        by_time(
            wadspaces.iter().filter(|w| s(&w["base"], "kind") == kind && s(&w["base"], "id") == id).cloned().collect(),
        )
    };
    live("templates")
        .into_iter()
        .map(|t| {
            let tid = s(&t, "id").to_string();
            let cs: Vec<Value> =
                by_time(customizations.iter().filter(|c| s(c, "templateId") == tid).cloned().collect())
                    .into_iter()
                    .map(|c| {
                        let ws = on("customization", s(&c, "id"));
                        json!({ "customization": c, "wadspaces": ws })
                    })
                    .collect();
            json!({ "template": t, "customizations": cs, "wadspaces": on("template", &tid) })
        })
        .collect()
}

/// Whether a template or customization name is in use (deleted ones still
/// hold theirs: they can be restored). `except`: the one being saved.
pub fn name_taken(name: &str, lib: &Value, except: Option<&str>) -> bool {
    let all: Vec<Value> = js::arr(lib, "templates").iter().chain(js::arr(lib, "customizations")).cloned().collect();
    named(name, &all, except)
}

// --------------------------------------------------------------- importing

/// The parts of a customization another can import.
pub const IMPORT_PARTS: &[&str] = &["webapps", "extensions", "tools", "autostart", "browserMode"];

/// What another setup has to bring into this template: how much of each
/// part, whether it fits, and why not. `{<part>: {count, ok, why?}}`.
pub fn importable(from: &Value, template: &Value) -> Value {
    let browser = has_app(template, "wadbrowser");
    let code = has_app(template, "vscode");
    let tname = s(template, "name");
    let no_browser = format!("{tname} has no WadBrowser");
    let part = |count: usize, ok: bool, why: Option<String>| {
        let mut o = json!({ "count": count, "ok": ok });
        if let Some(w) = why {
            o["why"] = w.into();
        }
        o
    };
    let n = |k: &str| js::arr(from, k).len();
    let focus = s(from, "browserMode") == "focus";
    json!({
        "webapps": part(n("webapps"), browser && n("webapps") > 0, (!browser).then(|| no_browser.clone())),
        "extensions": part(n("extensions"), code && n("extensions") > 0, (!code).then(|| format!("{tname} has no VS Code"))),
        "tools": part(n("tools"), n("tools") > 0, None),
        "autostart": part(n("autostart"), n("autostart") > 0, None),
        "browserMode": part(usize::from(focus), browser && focus, (!browser).then_some(no_browser)),
    })
}

fn same_url(a: &str, b: &str) -> bool {
    a.strip_suffix('/').unwrap_or(a) == b.strip_suffix('/').unwrap_or(b)
}

fn push_unique(list: &mut Vec<Value>, more: &[Value]) {
    for v in more {
        if !list.contains(v) {
            list.push(v.clone());
        }
    }
}

/// `into` with the chosen parts of `from` added: lists merged without
/// repeats (a web app with the same address is kept once, and boot keys
/// follow it), boot entries only for apps this template has.
pub fn import_into(into: &Value, from: &Value, template: &Value, parts: &[&str]) -> Value {
    let can = importable(from, template);
    let want = |p: &str| parts.contains(&p) && can[p]["ok"] == true;
    let mut next = setup_of(into);
    let mut renamed: Vec<(String, String)> = Vec::new();
    if want("webapps") {
        let mut webapps = js::arr(&next, "webapps").to_vec();
        for w in js::arr(from, "webapps") {
            if let Some(same) = webapps.iter().find(|x| same_url(s(x, "url"), s(w, "url"))) {
                renamed.push((s(w, "id").into(), s(same, "id").into()));
                continue;
            }
            let base = s(w, "id").to_string();
            let mut id = base.clone();
            let mut n = 2;
            while webapps.iter().any(|x| s(x, "id") == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            renamed.push((base, id.clone()));
            let mut nw = w.clone();
            nw["id"] = id.into();
            webapps.push(nw);
        }
        next["webapps"] = Value::Array(webapps);
    }
    for p in ["extensions", "tools"] {
        if want(p) {
            let mut l = js::arr(&next, p).to_vec();
            push_unique(&mut l, js::arr(from, p));
            next[p] = Value::Array(l);
        }
    }
    if want("browserMode") {
        next["browserMode"] = from["browserMode"].clone();
    }
    if want("autostart") {
        let here: Vec<String> = bootables(template, &next).iter().map(|b| s(b, "key").to_string()).collect();
        let keys: Vec<Value> = strs(from, "autostart")
            .into_iter()
            .map(|k| match k.strip_prefix("web:").and_then(|id| renamed.iter().find(|(a, _)| a == id)) {
                Some((_, to)) => format!("web:{to}"),
                None => k.to_string(),
            })
            .filter(|k| here.contains(k))
            .map(Value::from)
            .collect();
        let mut l = js::arr(&next, "autostart").to_vec();
        push_unique(&mut l, &keys);
        next["autostart"] = Value::Array(l);
    }
    next
}

#[cfg(test)]
mod tests;
