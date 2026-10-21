//! What the board knows of the catalog (wad-core's catalog.json) and works
//! out from it (the client's builder/catalog.ts): keys and WadBrowser's key
//! groups, misconfiguration and its fixes, what a desktop shows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::KeyGroup;

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
