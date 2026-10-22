//! What the board knows of the catalog (wad-core's catalog.json) and works
//! out from it: keys and WadBrowser's key groups, and add-ons (what each is
//! called, its picture, what it does).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::b::Ext;
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
    pub description: Option<String>,
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
        description: e.get("description").and_then(Value::as_str).map(String::from),
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
        .unwrap_or_else(|| Extension { id: id.into(), name: id.into(), description: None, icon: None, secret: None })
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

/// What a command-line tool brings.
pub fn tool_what(id: &str) -> Option<String> {
    wad_core::catalog::tool(id).and_then(|t| t.get("description").and_then(Value::as_str).map(String::from))
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

/// A web app's id from its name (never "focus": that's WadBrowser's Focus).
pub fn webapp_id(name: &str) -> String {
    wad_core::layers::webapp_id(name)
}

/// A site's host ("https://www.notion.so/x" → "www.notion.so").
fn host_of(url: &str) -> String {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or("");
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("");
    host.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// A site typed in ("notion.so"): a web app named after it.
pub fn site_webapp(text: &str) -> Option<Ext> {
    let text = text.trim();
    let url = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let host = host_of(&url);
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host.is_empty() || host.contains(char::is_whitespace) {
        return None;
    }
    let first = host.split('.').next()?;
    let mut name: String = first.chars().take(1).flat_map(char::to_uppercase).collect();
    name.push_str(&first.chars().skip(1).collect::<String>());
    Some(Ext { id: webapp_id(&name), name: Some(name), url: Some(url), ..Ext::plain("wadbrowser", "") })
}

/// Web apps worth offering even when no wadspace has them: (id, name, url).
pub const COMMON_WEBAPPS: [(&str, &str, &str); 4] = [
    ("github", "GitHub", "https://github.com"),
    ("gmail", "Gmail", "https://mail.google.com"),
    ("google-drive", "Google Drive", "https://drive.google.com"),
    ("claude", "Claude", "https://claude.ai"),
];

/// What common web apps are for (any other: its site).
const WEB_WHAT: [(&str, &str); 6] = [
    ("github", "Code, issues and pull requests"),
    ("claude", "Anthropic's AI assistant"),
    ("gmail", "Email"),
    ("google-drive", "Files in the cloud"),
    ("google-firebase", "The Firebase console"),
    ("docs-rs", "Docs for Rust crates"),
];

// ─────────────────────────────── add-ons ───────────────────────────────

/// The apps add-ons are shown by, in this order (then any other's).
pub const EXT_ORDER: [&str; 3] = ["wadbrowser", "vscode", "terminal"];

/// What an app's add-ons are called.
pub fn ext_kind(app: &str) -> &'static str {
    match app {
        "wadbrowser" => "Web apps",
        "vscode" => "VS Code extensions",
        "terminal" => "Command-line tools",
        _ => "Add-ons",
    }
}

/// What an add-on is called, and its own picture: "" when it has none
/// (Focus, VS Code extensions without one, command-line tools).
pub fn ext_face(x: &Ext) -> (String, String) {
    match x.app.as_str() {
        _ if x.is_focus() => ("Focus".into(), String::new()),
        "vscode" => {
            let e = extension(&x.id);
            (e.name, e.icon.unwrap_or_default())
        }
        "terminal" => (tool_name(&x.id), String::new()),
        _ => (
            x.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| x.id.clone()),
            x.icon_url.clone().filter(|u| !u.is_empty()).unwrap_or_else(|| site_icon(x.url.as_deref().unwrap_or(""))),
        ),
    }
}

/// What an add-on does, in a few words.
pub fn ext_what(x: &Ext) -> String {
    match x.app.as_str() {
        _ if x.is_focus() => "Its web apps alone, no address bar".into(),
        "vscode" => extension(&x.id).description.unwrap_or_else(|| "A VS Code extension".into()),
        "terminal" => tool_what(&x.id).unwrap_or_else(|| "A command-line tool".into()),
        _ => match WEB_WHAT.iter().find(|(id, _)| *id == x.id) {
            Some((_, what)) => what.to_string(),
            None => {
                let host = host_of(x.url.as_deref().unwrap_or(""));
                match host.strip_prefix("www.").unwrap_or(&host) {
                    "" => "A web app".into(),
                    h => h.to_string(),
                }
            }
        },
    }
}

/// A key's kind as people say it: "web", "VS Code", "Claude Code"…
pub fn kind_label(kind: &str) -> String {
    match kind {
        "web" => "web".into(),
        "vscode" => "VS Code".into(),
        "vscode-claude-code" => "Claude Code".into(),
        "obsidian" => "Obsidian".into(),
        "steam" => "Steam".into(),
        other => other.into(),
    }
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
