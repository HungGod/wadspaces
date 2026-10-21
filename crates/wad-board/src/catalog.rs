//! What the board knows of the catalog (wad-core's catalog.json) and works
//! out from it (the client's builder/catalog.ts): keys and WadBrowser's key
//! groups, misconfiguration and its fixes, what a desktop shows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::*;

/// An app of the catalog, as the board shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub category: String,
    pub icon: String,
    pub summary: String,
    /// The customization layer it has ("vscode", "wadbrowser").
    pub layer: Option<String>,
    /// The kind of sign-in it can keep.
    pub secret: Option<String>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Every app, in the catalog's order.
pub fn apps() -> Vec<CatalogApp> {
    wad_core::catalog::catalog()["apps"].as_array().into_iter().flatten().map(to_app).collect()
}

fn to_app(a: &Value) -> CatalogApp {
    CatalogApp {
        id: s(a, "id").into(),
        name: s(a, "name").into(),
        category: s(a, "category").into(),
        icon: s(a, "icon").into(),
        summary: s(a, "summary").into(),
        layer: a.get("options").and_then(Value::as_str).map(String::from),
        secret: a.get("secret").and_then(|x| x.get("kind")).and_then(Value::as_str).map(String::from),
    }
}

pub fn app(id: &str) -> Option<CatalogApp> {
    wad_core::catalog::catalog()["apps"].as_array()?.iter().find(|a| s(a, "id") == id).map(to_app)
}

/// Command-line tools aren't an app on the desktop, but every template has
/// them as a layer.
pub const TERMINAL_ICON: &str = "/icons/terminal.png";

/// An app setup's title, app and icon (APP_SUB_INFO).
pub fn app_sub_info(key: &str) -> (&'static str, &'static str, &'static str) {
    match key {
        "vscode" => ("VS Code extensions", "VS Code", "/apps/vscode/icon.png"),
        "wadbrowser" => ("WadBrowser apps", "WadBrowser", "/brand/wadbrowser-icon-dark.svg"),
        _ => ("Command-line tools", "Command line", TERMINAL_ICON),
    }
}

/// A VS Code extension, as the board shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub secret: Option<String>,
}

/// Extensions with icons of their own.
pub fn extension_icon(id: &str) -> Option<&'static str> {
    match id {
        "anthropic.claude-code" => Some("/icons/claude.png"),
        _ => None,
    }
}

/// The catalog's extensions.
pub fn extensions() -> Vec<Extension> {
    wad_core::catalog::catalog()["extensions"].as_array().into_iter().flatten().map(to_extension).collect()
}

fn to_extension(e: &Value) -> Extension {
    Extension {
        id: s(e, "id").into(),
        name: s(e, "name").into(),
        icon: extension_icon(s(e, "id")).map(String::from),
        secret: e.get("secret").and_then(|x| x.get("kind")).and_then(Value::as_str).map(String::from),
    }
}

/// An extension: the catalog's, or one added by its marketplace id.
pub fn extension(id: &str) -> Extension {
    wad_core::catalog::catalog()["extensions"]
        .as_array()
        .and_then(|l| l.iter().find(|e| s(e, "id") == id))
        .map(to_extension)
        .unwrap_or_else(|| Extension { id: id.into(), name: id.into(), icon: None, secret: None })
}

/// The catalog's command-line tools: (id, name).
pub fn tools() -> Vec<(String, String)> {
    wad_core::catalog::catalog()["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| (s(t, "id").to_string(), s(t, "name").to_string()))
        .collect()
}

pub fn tool_name(id: &str) -> String {
    tools().into_iter().find(|(t, _)| t == id).map(|(_, n)| n).unwrap_or_else(|| id.to_string())
}

/// A web app's icon (the client's siteIcon): one the catalog brings for its
/// site, else the site's favicon.
pub fn site_icon(url: &str) -> String {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    let Some(rest) = rest else { return "/favicon.svg".into() };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    if host.is_empty() {
        return "/favicon.svg".into();
    }
    match host.as_str() {
        "claude.ai" => "/icons/claude.png".into(),
        "chatgpt.com" => "/icons/codex.png".into(),
        _ => wad_core::layers::webapp_icon(url),
    }
}

/// A web app's id from its name.
pub fn webapp_id(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() { "app".into() } else { out.to_string() }
}

fn no_spaces(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

// ─────────────────────────────── keys ───────────────────────────────

/// One of WadBrowser's apps, for its key groups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserItem {
    pub id: String,
    pub name: String,
    pub icon: String,
}

pub const BROWSER_ICON: &str = "/brand/wadbrowser-icon-dark.svg";

/// WadBrowser and its web apps, when the template has WadBrowser.
pub fn browser_items(s: &Stack) -> Vec<BrowserItem> {
    if !s.has_app("wadbrowser") {
        return vec![];
    }
    let mut out = vec![BrowserItem { id: "browser".into(), name: "WadBrowser".into(), icon: BROWSER_ICON.into() }];
    for w in s.browser().map(|b| b.webapps.as_slice()).unwrap_or(&[]) {
        out.push(BrowserItem { id: format!("web:{}", w.id), name: w.name.clone(), icon: site_icon(&w.url) });
    }
    out
}

/// WadBrowser's key groups, tidied: members that are gone dropped, and
/// anything not in a group joining the first group that has apps (by
/// default one group). An empty group stays, waiting for apps.
pub fn key_groups_of(s: &Stack, sub: Option<&SecretsSub>) -> Vec<KeyGroup> {
    let items: Vec<String> = browser_items(s).into_iter().map(|i| i.id).collect();
    if items.is_empty() {
        return vec![];
    }
    let mut groups: Vec<KeyGroup> = sub
        .and_then(|x| x.groups.as_ref())
        .into_iter()
        .flatten()
        .map(|g| KeyGroup { members: g.members.iter().filter(|m| items.contains(m)).cloned().collect(), ..g.clone() })
        .collect();
    let placed: Vec<&String> = groups.iter().flat_map(|g| g.members.iter()).collect();
    let loose: Vec<String> = items.iter().filter(|i| !placed.contains(i)).cloned().collect();
    if loose.is_empty() {
        return groups;
    }
    match groups.iter().position(|g| !g.members.is_empty()) {
        None => {
            groups.insert(0, KeyGroup { id: "all".into(), members: loose, key: None });
            groups
        }
        Some(first) => {
            groups[first].members.extend(loose);
            groups
        }
    }
}

/// One key a stack could have: what signs in with it, and where it's kept.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyLine {
    /// "app:vscode", "ext:<id>", "app:obsidian", "app:steam", or
    /// "group:<id>" for a WadBrowser group.
    pub slot: String,
    pub kind: String,
    pub label: String,
    pub icon: String,
    /// Names a new key: <Wadspace>-<this>.
    pub app_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<KeyGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<BrowserItem>>,
}

/// An app with keys, and its key lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyApp {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub lines: Vec<KeyLine>,
}

/// The keys a stack could have, by app, in desktop order.
pub fn key_map(s: &Stack, sub: Option<&SecretsSub>) -> Vec<KeyApp> {
    let extensions = s.vscode().map(|v| v.extensions.clone()).unwrap_or_default();
    key_map_of(s.apps(), browser_items(s), &extensions, key_groups_of(s, sub))
}

/// The keys apps could have, by app in their order: WadBrowser's groups of
/// `items` (`groups`, tidied), VS Code's settings and its extensions that
/// sign in, and each other app that keeps a key (either board's wadspaces).
pub fn key_map_of(
    apps: &[String],
    items: Vec<BrowserItem>,
    extensions: &[String],
    groups: Vec<KeyGroup>,
) -> Vec<KeyApp> {
    let mut out = vec![];
    for id in apps {
        let Some(a) = app(id) else { continue };
        if id == "wadbrowser" {
            let lines = groups
                .iter()
                .cloned()
                .map(|g| {
                    let members: Vec<BrowserItem> =
                        g.members.iter().filter_map(|m| items.iter().find(|i| i.id == *m).cloned()).collect();
                    let only = if members.len() == 1 { members.first().cloned() } else { None };
                    KeyLine {
                        slot: format!("group:{}", g.id),
                        kind: "web".into(),
                        label: match (&only, members.len()) {
                            (Some(o), _) => o.name.clone(),
                            (None, 0) => "Empty group".into(),
                            (None, n) => format!("{n} apps"),
                        },
                        icon: only.as_ref().map(|o| o.icon.clone()).unwrap_or_else(|| a.icon.clone()),
                        app_id: match &only {
                            Some(o) if o.id != "browser" => format!("WadBrowser-{}", no_spaces(&o.name)),
                            _ => "WadBrowser".into(),
                        },
                        group: Some(g),
                        members: Some(members),
                    }
                })
                .collect();
            out.push(KeyApp { id: id.clone(), name: a.name, icon: a.icon, lines });
        } else if id == "vscode" {
            let mut lines = vec![KeyLine {
                slot: "app:vscode".into(),
                kind: "vscode".into(),
                label: "Settings".into(),
                icon: a.icon.clone(),
                app_id: "VSCode".into(),
                group: None,
                members: None,
            }];
            for x in extensions {
                let e = extension(x);
                if let Some(kind) = e.secret {
                    lines.push(KeyLine {
                        slot: format!("ext:{x}"),
                        kind,
                        label: e.name.clone(),
                        icon: e.icon.unwrap_or_else(|| a.icon.clone()),
                        app_id: format!("VSCode-{}", no_spaces(&e.name)),
                        group: None,
                        members: None,
                    });
                }
            }
            out.push(KeyApp { id: id.clone(), name: a.name, icon: a.icon, lines });
        } else if let Some(kind) = a.secret.clone() {
            out.push(KeyApp {
                id: id.clone(),
                name: a.name.clone(),
                icon: a.icon.clone(),
                lines: vec![KeyLine {
                    slot: format!("app:{id}"),
                    kind,
                    label: if id == "steam" { "Account" } else { "Vault sync" }.into(),
                    icon: a.icon.clone(),
                    app_id: no_spaces(&a.name),
                    group: None,
                    members: None,
                }],
            });
        }
    }
    out
}

/// The key a line has, if any.
pub fn key_of<'a>(sub: Option<&'a SecretsSub>, line: &'a KeyLine) -> Option<&'a Value> {
    match &line.group {
        Some(g) => g.key.as_ref(),
        None => sub.and_then(|x| x.by_slot.get(&line.slot)),
    }
}

/// How many keys a stack could have, and how many it has: (all, set).
pub fn key_count(s: &Stack) -> (usize, usize) {
    let sub = s.secrets();
    // An empty WadBrowser group signs nothing in: it doesn't count.
    let lines: Vec<KeyLine> = key_map(s, sub)
        .into_iter()
        .flat_map(|a| a.lines)
        .filter(|l| l.group.as_ref().is_none_or(|g| !g.members.is_empty()))
        .collect();
    let set = lines.iter().filter(|l| key_of(sub, l).is_some()).count();
    (lines.len(), set)
}

/// The key a web app (or the browser) signs in with: its group's.
pub fn web_key(s: &Stack, member: &str) -> Option<Value> {
    key_groups_of(s, s.secrets()).into_iter().find(|g| g.members.iter().any(|m| m == member)).and_then(|g| g.key)
}

// ─────────────────────────────── misconfiguration ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Fix {
    #[serde(rename_all = "camelCase")]
    AddApp {
        app_id: String,
        label: String,
    },
    #[serde(rename_all = "camelCase")]
    RemoveApp {
        app_id: String,
        label: String,
    },
    RemoveKey {
        slot: String,
        label: String,
    },
}

impl Fix {
    pub fn label(&self) -> &str {
        match self {
            Fix::AddApp { label, .. } | Fix::RemoveApp { label, .. } | Fix::RemoveKey { label, .. } => label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub id: String,
    pub text: String,
    pub fixes: Vec<Fix>,
}

/// What doesn't add up in a stack: app setups or keys for an app its
/// template doesn't have, and keys for extensions that aren't there any
/// more. Each with fixes: add the app, or take out what needs it.
pub fn problems_of(s: &Stack) -> Vec<Problem> {
    let apps = s.apps();
    if apps.is_empty() {
        return vec![];
    }
    let sub = s.secrets();
    let empty = serde_json::Map::new();
    let keys = sub.map(|x| &x.by_slot).unwrap_or(&empty);
    let mut out = vec![];
    let parts = |app_id: &str| {
        let mut p: Vec<&str> = vec![];
        if app_id == "vscode" && s.vscode().is_some() {
            p.push("its extensions are set up");
        }
        if app_id == "wadbrowser" && s.browser().is_some() {
            p.push("its web apps are set up");
        }
        let has_keys = match app_id {
            "vscode" => keys.keys().any(|k| k == "app:vscode" || k.starts_with("ext:")),
            "wadbrowser" => sub.and_then(|x| x.groups.as_ref()).is_some_and(|g| g.iter().any(|g| g.key.is_some())),
            _ => keys.contains_key(&format!("app:{app_id}")),
        };
        if has_keys {
            p.push("keys are kept for it");
        }
        p
    };
    for app_id in ["wadbrowser", "vscode", "obsidian", "steam"] {
        if apps.iter().any(|a| a == app_id) {
            continue;
        }
        let p = parts(app_id);
        if p.is_empty() {
            continue;
        }
        let name = app(app_id).map(|a| a.name).unwrap_or_else(|| app_id.into());
        let what = if p.len() == 2 {
            "setup and keys"
        } else if p[0].starts_with("keys") {
            "keys"
        } else {
            "setup"
        };
        out.push(Problem {
            id: format!("missing:{app_id}"),
            text: format!("{name} isn't in the template, but {}.", p.join(" and ")),
            fixes: vec![
                Fix::AddApp { app_id: app_id.into(), label: format!("Add {name} to the template") },
                Fix::RemoveApp { app_id: app_id.into(), label: format!("Remove {name}'s {what}") },
            ],
        });
    }
    if apps.iter().any(|a| a == "vscode") {
        let installed = s.vscode().map(|v| v.extensions.clone()).unwrap_or_default();
        for k in keys.keys().filter(|k| k.starts_with("ext:") && !installed.iter().any(|i| *i == k[4..])) {
            let e = extension(&k[4..]);
            out.push(Problem {
                id: format!("stray:{k}"),
                text: format!("A key is kept for {}, but it isn't one of VS Code's extensions.", e.name),
                fixes: vec![Fix::RemoveKey { slot: k.clone(), label: format!("Remove the {} key", e.name) }],
            });
        }
    }
    out
}

/// A sign-in slot of a stack.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSlot {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub app_id: String,
    pub icon: String,
}

/// The sign-ins a stack can keep, in desktop order: apps, extensions,
/// then web apps.
pub fn secret_slots(s: &Stack) -> Vec<SecretSlot> {
    let mut out = vec![];
    for id in s.apps() {
        if let Some(a) = app(id)
            && let Some(kind) = a.secret.clone()
        {
            out.push(SecretSlot {
                key: format!("app:{id}"),
                kind,
                label: a.name.clone(),
                app_id: no_spaces(&a.name),
                icon: a.icon,
            });
        }
    }
    if s.has_app("vscode") {
        for id in s.vscode().map(|v| v.extensions.as_slice()).unwrap_or(&[]) {
            let e = extension(id);
            if let Some(kind) = e.secret.clone() {
                out.push(SecretSlot {
                    key: format!("ext:{id}"),
                    kind,
                    label: format!("{} (VS Code)", e.name),
                    app_id: format!("VSCode-{}", no_spaces(&e.name)),
                    icon: e.icon.unwrap_or_else(|| "/apps/vscode/icon.png".into()),
                });
            }
        }
    }
    if s.has_app("wadbrowser") {
        for w in s.browser().map(|b| b.webapps.as_slice()).unwrap_or(&[]) {
            out.push(SecretSlot {
                key: format!("web:{}", w.id),
                kind: "web".into(),
                label: format!("{} (web app)", w.name),
                app_id: format!("WadBrowser-{}", no_spaces(&w.name)),
                icon: site_icon(&w.url),
            });
        }
    }
    out
}

/// One icon on the wadspace's desktop, and the layer it comes from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesktopIcon {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub from: Layer,
}

/// What the wadspace's desktop shows: template apps, then web apps, then
/// projects' folders.
pub fn desktop_of(s: &Stack) -> Vec<DesktopIcon> {
    let mut out = vec![];
    for id in s.apps() {
        if let Some(a) = app(id) {
            out.push(DesktopIcon { id: format!("app:{id}"), label: a.name, icon: a.icon, from: Layer::Template });
        }
    }
    if s.has_app("wadbrowser") {
        for w in s.browser().map(|b| b.webapps.as_slice()).unwrap_or(&[]) {
            out.push(DesktopIcon {
                id: format!("web:{}", w.id),
                label: w.name.clone(),
                icon: site_icon(&w.url),
                from: Layer::Extensions,
            });
        }
    }
    if s.has_app("vscode") {
        for m in s.folders().map(|f| f.mounts.as_slice()).unwrap_or(&[]) {
            out.push(DesktopIcon {
                id: format!("dir:{}", s_of(m, "id")),
                label: s_of(m, "as").into(),
                icon: "/folder.svg".into(),
                from: Layer::Customization,
            });
        }
    }
    out
}

fn s_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The app setups a stack could have: WadBrowser's and VS Code's when its
/// template has them, the command line's always.
pub fn setups_for(apps: &[String]) -> Vec<&'static str> {
    let mut out = vec![];
    if apps.iter().any(|a| a == "wadbrowser") {
        out.push("wadbrowser");
    }
    if apps.iter().any(|a| a == "vscode") {
        out.push("vscode");
    }
    out.push("tools");
    out
}

/// Whether a submodule's app is in the template (else it does nothing).
pub fn sub_fits(key: &str, apps: Option<&[String]>) -> Result<(), String> {
    let Some(apps) = apps else { return Ok(()) };
    let has = |a: &str| apps.iter().any(|x| x == a);
    if key == "vscode" && !has("vscode") {
        return Err("VS Code isn't in this template".into());
    }
    if key == "wadbrowser" && !has("wadbrowser") {
        return Err("WadBrowser isn't in this template".into());
    }
    Ok(())
}
