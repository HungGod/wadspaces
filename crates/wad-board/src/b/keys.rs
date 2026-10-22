//! A wadspace's keys on the board: WadBrowser's key groups, every slot
//! that keeps a key (the key editor's lines), each key with what signs in
//! with it (the card's Secrets), and new keys.

use serde::Serialize;
use serde_json::{Map, Value, json};
use wad_core::{layers, spec, v5};

use super::{Board, HolderRef, Payload, Wadspace, free, free_group, s};
use crate::catalog::{self as cat, BrowserItem, KeyApp, KeyLine};
use crate::model::KeyGroup;

/// Something that signs in with a key, as the card shows it: an app, a
/// VS Code extension, WadBrowser or one of its web apps (`member`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyHolder {
    pub icon: String,
    pub name: String,
    /// What it is, in a few words (its hover text).
    pub what: String,
    pub line: KeyLine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

impl KeyHolder {
    /// What a new key for it alone is named after: <Wadspace>-<this>.
    pub fn app_id(&self) -> String {
        match self.member.as_deref() {
            None => self.line.app_id.clone(),
            Some("browser") => "WadBrowser".into(),
            Some(_) => format!("WadBrowser-{}", self.name.chars().filter(|c| !c.is_whitespace()).collect::<String>()),
        }
    }
    pub fn holder_ref(&self) -> HolderRef {
        HolderRef { slot: self.line.slot.clone(), member: self.member.clone() }
    }
}

/// A key in a wadspace, with everything that signs in with it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    /// `{id, name, kind}`.
    pub key: Value,
    /// The app it's for, and its picture: a WadBrowser key WadBrowser's, a
    /// VS Code extension's the extension's (Claude Code: Claude), else the
    /// app's.
    pub app: String,
    pub icon: String,
    pub holders: Vec<KeyHolder>,
}

/// What a dragged key, or one thing off it, brings: what signs in, the
/// key's kind, and what a new key for them is named after.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub holders: Vec<HolderRef>,
    pub kind: String,
    pub app_id: String,
    pub key: String,
}

/// The key a line has (its group's, or its slot's).
pub fn key_of(w: &Wadspace, l: &KeyLine) -> Option<Value> {
    match &l.group {
        Some(g) => g.key.clone(),
        None => w.customization.get("secrets")?.get("bySlot")?.get(&l.slot).filter(|k| k.is_object()).cloned(),
    }
}

impl Board {
    /// The key slots it has (wad_core's: an app's own, each AI extension's,
    /// each web app's).
    pub fn slots_of(&self, ws: &str) -> Vec<Value> {
        let Some(w) = self.get(ws) else { return vec![] };
        let (t, setup) = v5::as_template_setup(&w.doc());
        layers::slots_for(&t, &setup)
    }

    /// WadBrowser's apps (the browser and each web app; none without
    /// WadBrowser) in their key groups, every one in exactly one: those in
    /// none join the first. A group none of its apps are left in goes (its
    /// key with it): a key always has something on it.
    pub fn groups_of(&self, ws: &str) -> Vec<KeyGroup> {
        let Some(w) = self.get(ws) else { return vec![] };
        let apps = v5::apps_of(&w.doc());
        let Some(browser) = apps.iter().find(|e| s(e, "app") == "wadbrowser") else { return vec![] };
        let mut members = vec!["browser".to_string()];
        members.extend(browser["extras"].as_array().into_iter().flatten().map(|x| format!("web:{}", s(x, "id"))));
        let stored = w.customization.get("secrets").and_then(|x| x.get("groups")).cloned().unwrap_or(json!([]));
        let mut groups: Vec<KeyGroup> = serde_json::from_value::<Vec<KeyGroup>>(stored)
            .unwrap_or_default()
            .into_iter()
            .map(|mut g| {
                g.members.retain(|m| members.contains(m));
                g
            })
            .filter(|g| !g.members.is_empty())
            .collect();
        let loose: Vec<String> =
            members.into_iter().filter(|m| !groups.iter().any(|g| g.members.contains(m))).collect();
        if !loose.is_empty() {
            match groups.first_mut() {
                Some(g) => g.members.extend(loose),
                None => groups.push(KeyGroup { id: "g1".into(), members: loose, key: None }),
            }
        }
        groups
    }

    /// Its groups as `f` leaves them (those left empty go), saved.
    pub(super) fn set_groups(&mut self, ws: &str, f: impl FnOnce(&mut Vec<KeyGroup>)) -> Result<(), String> {
        let mut gs = self.groups_of(ws);
        f(&mut gs);
        gs.retain(|g| !g.members.is_empty());
        let gs = serde_json::to_value(gs).unwrap_or(json!([]));
        self.get_mut(ws)?.secrets().insert("groups".into(), gs);
        Ok(())
    }

    /// Its keys by app, as the key editor shows them: WadBrowser's groups,
    /// VS Code's settings and its extensions that sign in, each other app
    /// that keeps a key.
    pub fn key_map(&self, ws: &str) -> Vec<KeyApp> {
        let Some(w) = self.get(ws) else { return vec![] };
        let apps = v5::apps_of(&w.doc());
        let ids: Vec<String> = apps.iter().map(|e| s(e, "app").to_string()).collect();
        let extras = |app: &str| -> Vec<Value> {
            apps.iter().find(|e| s(e, "app") == app).and_then(|e| e["extras"].as_array().cloned()).unwrap_or_default()
        };
        let mut items = vec![];
        if ids.iter().any(|a| a == "wadbrowser") {
            items.push(BrowserItem { id: "browser".into(), name: "WadBrowser".into(), icon: cat::BROWSER_ICON.into() });
            for x in extras("wadbrowser") {
                let icon = match s(&x, "iconUrl") {
                    "" => cat::site_icon(s(&x, "url")),
                    i => i.to_string(),
                };
                items.push(BrowserItem { id: format!("web:{}", s(&x, "id")), name: s(&x, "name").into(), icon });
            }
        }
        let extensions: Vec<String> = extras("vscode").iter().map(|x| s(x, "id").to_string()).collect();
        cat::key_map_of(&ids, items, &extensions, self.groups_of(ws))
    }

    /// Its keys, each with everything that signs in with it (the card's
    /// Secrets), in the key editor's order.
    pub fn secrets_by_key(&self, ws: &str) -> Vec<KeyEntry> {
        let Some(w) = self.get(ws) else { return vec![] };
        let mut out: Vec<KeyEntry> = vec![];
        for a in self.key_map(ws) {
            for l in &a.lines {
                let Some(k) = key_of(w, l) else { continue };
                let i = match out.iter().position(|e| s(&e.key, "id") == s(&k, "id")) {
                    Some(i) => i,
                    None => {
                        // What it's for: WadBrowser's group, else the line's own.
                        let icon = if l.group.is_some() { a.icon.clone() } else { l.icon.clone() };
                        out.push(KeyEntry { key: k, app: a.id.clone(), icon, holders: vec![] });
                        out.len() - 1
                    }
                };
                let holders = &mut out[i].holders;
                match &l.members {
                    Some(members) => holders.extend(members.iter().map(|m| KeyHolder {
                        icon: m.icon.clone(),
                        name: m.name.clone(),
                        what: if m.id == "browser" {
                            "WadBrowser itself".into()
                        } else {
                            format!("{}, a web app in WadBrowser", m.name)
                        },
                        line: l.clone(),
                        member: Some(m.id.clone()),
                    })),
                    None if a.id == "vscode" && l.slot != "app:vscode" => holders.push(KeyHolder {
                        icon: l.icon.clone(),
                        name: l.label.clone(),
                        what: format!("{}, a VS Code extension", l.label),
                        line: l.clone(),
                        member: None,
                    }),
                    None => holders.push(KeyHolder {
                        icon: a.icon.clone(),
                        name: a.name.clone(),
                        what: if l.label == "Settings" {
                            format!("{}'s settings", a.name)
                        } else {
                            format!("{} ({})", a.name, l.label)
                        },
                        line: l.clone(),
                        member: None,
                    }),
                }
            }
        }
        out
    }

    /// What a dragged key (all it's on) or one thing off it brings: None
    /// for any other payload, or one that's no longer there.
    pub fn holders_of(&self, p: &Payload) -> Option<Held> {
        let (ws, key, one) = match p {
            Payload::Key { ws, key } => (ws, key, None),
            Payload::Holder { ws, key, slot, member } => (ws, key, Some((slot, member))),
            _ => return None,
        };
        let e = self.secrets_by_key(ws).into_iter().find(|e| s(&e.key, "id") == key)?;
        let hs: Vec<&KeyHolder> = e
            .holders
            .iter()
            .filter(|h| one.is_none_or(|(slot, member)| h.line.slot == *slot && h.member == *member))
            .collect();
        let first = hs.first()?;
        Some(Held {
            holders: hs.iter().map(|h| h.holder_ref()).collect(),
            kind: s(&e.key, "kind").into(),
            app_id: if hs.len() == 1 { first.app_id() } else { first.line.app_id.clone() },
            key: key.clone(),
        })
    }

    /// Key `key`'s slots and WadBrowser's apps with it in `ws` (`one`: just
    /// that slot, or that member of its group), as a drop carries them.
    pub(super) fn keys_out(
        &self,
        ws: &str,
        key: &str,
        one: Option<(&String, Option<&str>)>,
    ) -> Result<(Map<String, Value>, Vec<KeyGroup>), String> {
        let w = self.get(ws).ok_or_else(|| format!("no wadspace {ws}"))?;
        let (mut by, mut groups) = (Map::new(), vec![]);
        for a in self.key_map(ws) {
            for l in a.lines {
                let Some(k) = key_of(w, &l) else { continue };
                if s(&k, "id") != key || one.is_some_and(|(slot, _)| l.slot != *slot) {
                    continue;
                }
                match &l.group {
                    Some(g) => {
                        let members = match one {
                            Some((_, Some(m))) => vec![m.to_string()],
                            _ => g.members.clone(),
                        };
                        groups.push(KeyGroup { id: g.id.clone(), members, key: Some(k) });
                    }
                    None => drop(by.insert(l.slot, k)),
                }
            }
        }
        Ok((by, groups))
    }

    /// These sign in with `key` (none: no key): slots set, WadBrowser's apps
    /// out of their groups and into the one with that key (else a new one).
    /// A key signs in only what's its kind.
    pub(super) fn assign_key(&mut self, ws: &str, holders: &[HolderRef], key: Option<&Value>) -> Result<(), String> {
        if let Some(k) = key {
            let lines: Vec<KeyLine> = self.key_map(ws).into_iter().flat_map(|a| a.lines).collect();
            for h in holders {
                if let Some(l) = lines.iter().find(|l| l.slot == h.slot)
                    && l.kind != s(k, "kind")
                {
                    let kind = cat::kind_label(s(k, "kind"));
                    return Err(format!("{} is a {kind} key: that doesn't sign in with it", s(k, "name")));
                }
            }
        }
        let members: Vec<String> = holders.iter().filter_map(|h| h.member.clone()).collect();
        if !members.is_empty() {
            let id = key.map(|k| s(k, "id").to_string());
            self.set_groups(ws, |gs| {
                let ids: Vec<String> = gs.iter().map(|g| g.id.clone()).collect();
                for g in gs.iter_mut() {
                    g.members.retain(|m| !members.contains(m));
                }
                let with = |g: &&mut KeyGroup| match &id {
                    Some(id) => g.key.as_ref().is_some_and(|k| s(k, "id") == id),
                    None => g.key.is_none(),
                };
                match gs.iter_mut().find(with) {
                    Some(g) => g.members.extend(members.clone()),
                    None => gs.push(KeyGroup { id: free_group(&ids), members: members.clone(), key: key.cloned() }),
                }
            })?;
        }
        let by = self.get_mut(ws)?.by_slot();
        for h in holders.iter().filter(|h| h.member.is_none()) {
            match key {
                Some(k) => by.insert(h.slot.clone(), k.clone()),
                None => by.remove(&h.slot),
            };
        }
        Ok(())
    }

    /// Every key the board's wadspaces keep, each once (`{id, name, kind}`).
    pub fn keys(&self) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        for w in &self.wadspaces {
            let sec = w.customization.get("secrets");
            let by = sec.and_then(|x| x.get("bySlot")).and_then(Value::as_object).into_iter().flat_map(|o| o.values());
            let groups = sec.and_then(|x| x.get("groups")).and_then(Value::as_array).into_iter().flatten();
            for k in by.chain(groups.filter_map(|g| g.get("key"))) {
                if k.is_object() && !out.iter().any(|o| s(o, "id") == s(k, "id")) {
                    out.push(k.clone());
                }
            }
        }
        out
    }

    /// Why key `id` can't be called `name`, or None. A machine keeps one
    /// sign-in by a name, whatever its kind and however it's written.
    pub fn key_clash(&self, id: &str, name: &str, known: &[Value]) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return Some("Give it a name".into());
        }
        let used = self.keys();
        known
            .iter()
            .chain(&used)
            .find(|k| s(k, "id") != id && s(k, "name").eq_ignore_ascii_case(name))
            .map(|o| format!("Another key is already called {}", s(o, "name")))
    }

    /// A new key `{id, name, kind}`: called `name` unless a key is (the
    /// board's, or `known`: the machine's), then name-2…; its id from its
    /// name, its own.
    pub fn new_key(&self, name: &str, kind: &str, known: &[Value]) -> Value {
        let all: Vec<Value> = self.keys().into_iter().chain(known.iter().cloned()).collect();
        let names: Vec<String> = all.iter().map(|k| s(k, "name").to_lowercase()).collect();
        let ids: Vec<String> = all.iter().map(|k| s(k, "id").to_string()).collect();
        let base = name.trim();
        let name = std::iter::once(base.to_string())
            .chain((2..).map(|n| format!("{base}-{n}")))
            .find(|n| !names.contains(&n.to_lowercase()))
            .unwrap();
        let id = free(&spec::slugify(&name), &ids);
        json!({"id": id, "name": name, "kind": kind})
    }

    /// A new key for something in `ws` named after what's in it:
    /// <Wadspace>-<app_id> (the wadspace's name without spaces).
    pub fn new_key_for(&self, ws: &str, app_id: &str, kind: &str, known: &[Value]) -> Value {
        let w: String = self.get(ws).map(|w| w.name()).unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
        let w = if w.is_empty() { "Wadspace".to_string() } else { w };
        self.new_key(&format!("{w}-{app_id}"), kind, known)
    }
}
