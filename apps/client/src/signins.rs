//! Saved sign-ins (the web app's SignIns.tsx, keyUse and the store's
//! sign-in parts): the machine's (wadd's /v1/signins) and its tokens, what
//! each kind is called and keeps, who uses each one, and making, renaming,
//! signing out of and deleting them. A rename reaches the wadspaces that
//! keep the key by name too.

use std::collections::HashMap;

use gtk::glib;
use serde_json::{Value, json};
use wad_core::{catalog, layers};
use wad_proto::v1::{SecretInfo, SignIn};
use wad_wadd_client::Method;

use crate::pages::machines::{get, send};
use crate::store::{AppState, Topic, store};

/// What a sign-in of each kind is called, and what it keeps (catalog.ts's
/// SECRET_KINDS).
pub fn kind_label(kind: &str) -> String {
    match kind {
        "web" => "WadBrowser sign-in",
        "vscode" => "VS Code settings",
        "obsidian" => "Obsidian sign-in",
        "steam" => "Steam sign-in",
        "vscode-claude-code" => "Claude Code sign-in",
        other => return other.to_string(),
    }
    .into()
}

pub fn kind_keeps(kind: &str) -> &'static str {
    match kind {
        "web" => "A cookie profile: one or more web apps can share it",
        "vscode" => "VS Code's accounts, settings, keybindings and snippets",
        "obsidian" => "Obsidian's account (Sync, Publish)",
        "steam" => "Steam's login and each account's settings",
        "vscode-claude-code" => "Claude Code's login and settings (~/.claude)",
        _ => "",
    }
}

/// A kind's picture: the app (or extension) it's for; None: a globe.
pub fn kind_icon(kind: &str) -> Option<String> {
    match kind {
        "obsidian" | "steam" => catalog::app(kind).map(|a| a.icon),
        "vscode-claude-code" => Some("/icons/claude.png".into()),
        _ => None,
    }
}

/// Extensions with icons of their own (catalog.ts's EXTENSION_ICONS).
fn extension_icon(id: &str) -> Option<&'static str> {
    match id {
        "anthropic.claude-code" => Some("/icons/claude.png"),
        _ => None,
    }
}

/// Something a key signs in to: its name and icon.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyItem {
    pub name: String,
    pub icon: String,
}

/// Where a key is used: a wadspace or a WadBrowser setup, and what it signs
/// in to there.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyUser {
    /// "wadspace" or "browser".
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub what: String,
    pub items: Vec<KeyItem>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn alive(v: &Value) -> bool {
    v.get("deletedAt").is_none_or(Value::is_null)
}

/// Who uses each saved sign-in, by its id (the web app's keyUse).
pub fn key_use(st: &AppState) -> HashMap<String, Vec<KeyUser>> {
    let mut out: HashMap<String, Vec<KeyUser>> = HashMap::new();
    let Some(lib) = st.library.as_ref() else { return out };
    let vscode = catalog::app("vscode").map(|a| a.icon).unwrap_or_else(|| "/apps/vscode/icon.png".into());
    let slot_item = |slot: &str| {
        let (kind, id) = slot.split_once(':').unwrap_or((slot, ""));
        if kind == "ext" {
            let e = catalog::extension(id);
            return KeyItem {
                name: s(&e, "name").to_string(),
                icon: extension_icon(id).map(String::from).unwrap_or_else(|| vscode.clone()),
            };
        }
        match catalog::app(id) {
            Some(a) => KeyItem { name: a.name, icon: a.icon },
            None => KeyItem { name: id.to_string(), icon: "/favicon.svg".into() },
        }
    };
    let browser_item = KeyItem {
        name: "WadBrowser".into(),
        icon: catalog::app("wadbrowser").map(|a| a.icon).unwrap_or_else(|| "/brand/wadbrowser-icon-dark.svg".into()),
    };
    let web_item =
        |webapps: &[Value], member: &str| match webapps.iter().find(|x| format!("web:{}", s(x, "id")) == member) {
            Some(w) => KeyItem { name: s(w, "name").to_string(), icon: layers::webapp_icon(s(w, "url")) },
            None => KeyItem { name: member.trim_start_matches("web:").to_string(), icon: "/favicon.svg".into() },
        };
    let what = |items: &[KeyItem]| items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ");
    let mut add = |id: &str, u: KeyUser| out.entry(id.to_string()).or_default().push(u);
    for w in st.all_wadspaces() {
        if !w["customization"].is_object() {
            continue;
        }
        let keys = &w["customization"]["secrets"];
        let name = st.wadspace_name(w);
        for (slot, r) in keys["bySlot"].as_object().into_iter().flatten() {
            let items = vec![slot_item(slot)];
            add(
                s(r, "id"),
                KeyUser { kind: "wadspace", id: s(w, "id").into(), name: name.clone(), what: what(&items), items },
            );
        }
        let webapps = wad_core::v4::as_template_setup(w).1["webapps"].as_array().cloned().unwrap_or_default();
        for g in keys["groups"].as_array().into_iter().flatten() {
            let members = g["members"].as_array().cloned().unwrap_or_default();
            if !g["key"].is_object() || members.is_empty() {
                continue;
            }
            let items: Vec<KeyItem> = members
                .iter()
                .filter_map(Value::as_str)
                .map(|m| if m == "browser" { browser_item.clone() } else { web_item(&webapps, m) })
                .collect();
            add(
                s(&g["key"], "id"),
                KeyUser { kind: "wadspace", id: s(w, "id").into(), name: name.clone(), what: what(&items), items },
            );
        }
    }
    for b in lib.browsers.iter().filter(|b| alive(b)) {
        let webapps = b["webapps"].as_array().cloned().unwrap_or_default();
        for (slot, r) in b["secrets"].as_object().into_iter().flatten() {
            let items = vec![web_item(&webapps, slot)];
            add(
                s(r, "id"),
                KeyUser {
                    kind: "browser",
                    id: s(b, "id").into(),
                    name: s(b, "name").into(),
                    what: what(&items),
                    items,
                },
            );
        }
    }
    out
}

/// One row per wadspace (or setup), with every app the key signs in there.
pub fn merged(users: &[KeyUser]) -> Vec<KeyUser> {
    let mut out: Vec<KeyUser> = vec![];
    for u in users {
        match out.iter_mut().find(|x| x.kind == u.kind && x.id == u.id) {
            Some(x) => {
                x.what = format!("{}, {}", x.what, u.what);
                x.items.extend(u.items.iter().cloned());
            }
            None => out.push(u.clone()),
        }
    }
    out
}

/// The sign-ins and tokens, read again.
pub fn load() {
    glib::spawn_future_local(async {
        let signins = get::<Vec<SignIn>>("/v1/signins".into()).await;
        let secrets = get::<Vec<SecretInfo>>("/v1/secrets".into()).await.unwrap_or_default();
        store().update(|s| {
            s.signins = Some(signins.unwrap_or_default());
            s.secrets = secrets;
            Topic::SIGNINS
        });
    });
}

/// A new saved sign-in of a kind.
pub async fn create(name: String, kind: String) -> Result<SignIn, String> {
    let made = send::<SignIn>(Method::POST, "/v1/signins".into(), Some(json!({ "name": name, "app": kind }))).await?;
    load();
    Ok(made)
}

pub async fn sign_out(id: String) -> Result<(), String> {
    send::<Value>(Method::POST, format!("/v1/signins/{id}/signout"), Some(json!({}))).await?;
    load();
    Ok(())
}

pub async fn delete(id: String) -> Result<(), String> {
    send::<Value>(Method::DELETE, format!("/v1/signins/{id}"), None).await?;
    load();
    Ok(())
}

/// Renames one, and the name the wadspaces using it keep for it.
pub async fn rename(id: String, name: String) -> Result<(), String> {
    send::<Value>(Method::PUT, format!("/v1/signins/{id}"), Some(json!({ "name": name }))).await?;
    load();
    let changed: Vec<Value> =
        store().read_now(|s| s.all_wadspaces().into_iter().filter_map(|w| renamed(w, &id, &name)).collect());
    for w in changed {
        crate::account::save_wadspace(w).await?;
    }
    Ok(())
}

/// A wadspace with key `id` named `name` wherever it keeps it; None when
/// nothing changes.
fn renamed(w: &Value, id: &str, name: &str) -> Option<Value> {
    let mut w = w.clone();
    let mut changed = false;
    let mut swap = |k: &mut Value| {
        if k.is_object() && k["id"] == id && k["name"] != name {
            k["name"] = name.into();
            changed = true;
        }
    };
    let sec = &mut w["customization"]["secrets"];
    if let Some(by) = sec.get_mut("bySlot").and_then(Value::as_object_mut) {
        by.values_mut().for_each(&mut swap);
    }
    if let Some(groups) = sec.get_mut("groups").and_then(Value::as_array_mut) {
        for g in groups {
            if let Some(k) = g.get_mut("key") {
                swap(k);
            }
        }
    }
    if !changed {
        return None;
    }
    w["updatedAt"] = crate::launch::now_iso().into();
    Some(w)
}

/// The sign-ins a slot can pick: the machine's, and those the library's
/// wadspaces and setups name (online, perhaps made on another machine).
/// (id, name, kind); the web app's knownSignIns.
pub fn known(st: &AppState) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = st
        .signins
        .iter()
        .flatten()
        .map(|x| (x.id.clone(), x.name.clone(), if x.app == "wadbrowser" { "web".into() } else { x.app.clone() }))
        .collect();
    let mut add = |k: &Value| {
        let id = s(k, "id");
        if !id.is_empty() && !out.iter().any(|(i, _, _)| i == id) {
            out.push((id.into(), s(k, "name").into(), s(k, "kind").into()));
        }
    };
    for w in st.all_wadspaces() {
        let sec = &w["customization"]["secrets"];
        sec["bySlot"].as_object().into_iter().flatten().for_each(|(_, k)| add(k));
        sec["groups"].as_array().into_iter().flatten().for_each(|g| add(&g["key"]));
    }
    for b in st.library.iter().flat_map(|l| l.browsers.iter()).filter(|b| alive(b)) {
        b["secrets"].as_object().into_iter().flatten().for_each(|(_, k)| add(k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let lib = wad_account::Library {
            wadspaces: vec![json!({
                "version": 4, "id": "vanua", "order": 1,
                "templates": [{ "id": "dev", "name": "Dev", "apps": [
                    { "app": "wadbrowser", "extras": [] },
                    { "app": "vscode", "extras": [{ "id": "anthropic.claude-code" }] },
                ] }],
                "customization": { "name": "Vanua", "secrets": {
                    "bySlot": { "ext:anthropic.claude-code": { "id": "k1", "name": "Claude", "kind": "vscode-claude-code" } },
                    "groups": [{ "members": ["browser"], "key": { "id": "k2", "name": "Work", "kind": "web" } }],
                } },
            })],
            browsers: vec![json!({
                "id": "b1", "name": "Mail", "webapps": [{ "id": "gmail", "name": "Gmail", "url": "https://mail.google.com" }],
                "secrets": { "web:gmail": { "id": "k2", "name": "Work", "kind": "web" } },
            })],
            ..Default::default()
        };
        AppState { library: Some(lib), ..Default::default() }
    }

    #[test]
    fn keys_say_where_theyre_used_and_what_for() {
        let st = state();
        let u = key_use(&st);
        assert_eq!(u["k1"].len(), 1);
        assert_eq!((u["k1"][0].kind, u["k1"][0].what.as_str()), ("wadspace", "Claude Code"));
        assert_eq!(u["k1"][0].items[0].icon, "/icons/claude.png");
        let k2: Vec<(&str, &str)> = u["k2"].iter().map(|x| (x.kind, x.what.as_str())).collect();
        assert_eq!(k2, vec![("wadspace", "WadBrowser"), ("browser", "Gmail")]);
        let known = known(&st);
        assert_eq!(
            known.iter().map(|(i, _, k)| (i.as_str(), k.as_str())).collect::<Vec<_>>(),
            vec![("k1", "vscode-claude-code"), ("k2", "web")]
        );
    }

    #[test]
    fn a_rename_reaches_the_wadspaces_that_keep_its_name() {
        let st = state();
        let w = st.wadspaces()[0].clone();
        let r = renamed(&w, "k2", "Home").expect("changed");
        assert_eq!(r["customization"]["secrets"]["groups"][0]["key"]["name"], "Home");
        assert_eq!(r["customization"]["secrets"]["bySlot"]["ext:anthropic.claude-code"]["name"], "Claude");
        assert!(renamed(&w, "k9", "Home").is_none());
    }
}
