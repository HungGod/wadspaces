//! The Builder's board: version 5 wadspaces (wad_core::v5), each on one
//! machine and shown as one card: its customization (background,
//! projects), its secrets (its keys, each with what signs in with it), its
//! add-ons (VS Code's extensions, command-line tools, WadBrowser's web apps
//! and its Focus, by the app they're for) and its apps. The edits are the
//! cards':
//!
//! - A drag takes the card (all of it) or one piece: its customization, its
//!   secrets, a key with what signs in with it, one thing that does, its
//!   apps, an app's add-ons, an add-on, an app, the background, a project.
//! - Into another wadspace it's copied in, merged with what's there (an
//!   add-on brings its app); nothing moves around inside its own. Between
//!   wadspaces or on the open board it starts a new one; onto a machine (the
//!   carousel) a new one there, a whole wadspace keeping its name.
//! - What's taken out waits in its wadspace's Recently deleted; deleting a
//!   wadspace is soft (deletedAt), so it can be restored; Undo steps back.
//!
//! Every edit is an [`Op`] ([`Board::apply`]), as JSON: what the goldens
//! (fixtures/board/b.json) and the Android app's port replay. [`Board::plan`]
//! gives the documents to save.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use wad_core::{spec, v5};

use crate::model::KeyGroup;

mod keys;
pub use keys::*;

const UNDO_STEPS: usize = 40;
pub use v5::KEEP_MOST;

/// A new wadspace's name ("New wadspace - 1"… when its row has one).
pub const NEW_NAME: &str = "New wadspace";

/// An add-on: something added to an app (a VS Code extension, a
/// command-line tool, a WadBrowser web app or its Focus).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ext {
    pub app: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

impl Ext {
    /// One with no name or address of its own (an extension, a tool).
    pub fn plain(app: &str, id: &str) -> Self {
        Self { app: app.into(), id: id.into(), name: None, url: None, icon_url: None }
    }
    /// WadBrowser's Focus.
    pub fn focus() -> Self {
        Self { name: Some("Focus".into()), ..Self::plain("wadbrowser", v5::FOCUS) }
    }
    pub fn is_focus(&self) -> bool {
        self.app == "wadbrowser" && self.id == v5::FOCUS
    }
    /// The same add-on (its app's, by id).
    pub fn same(&self, o: &Ext) -> bool {
        self.app == o.app && self.id == o.id
    }
}

/// An app in a wadspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppIn {
    pub app: String,
}

/// Something taken out of a wadspace, kept in it (Recently deleted) until
/// it's put back or forgotten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    /// k1, k2…: its own among the wadspace's.
    pub id: String,
    /// When (the board's clock).
    pub at: String,
    #[serde(flatten)]
    pub piece: Piece,
}

/// What's kept: an app's add-ons, an app, a project, or the background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Piece {
    Exts { app: String, exts: Vec<Ext> },
    App { app: AppIn },
    Project { project: Value },
    Background { wallpaper: Value },
}

/// A version 5 wadspace document (its Firestore fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wadspace {
    #[serde(default = "five")]
    pub version: u64,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default)]
    pub shared_with: Vec<String>,
    pub machine: String,
    #[serde(default)]
    pub order: i64,
    /// Its apps, each once.
    #[serde(default)]
    pub apps: Vec<AppIn>,
    /// Its add-ons, each once.
    #[serde(default)]
    pub exts: Vec<Ext>,
    /// Name, description, wallpaper, titleBars, autostart, projects, secrets.
    #[serde(default)]
    pub customization: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    /// What was taken out of it, oldest first (Recently deleted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trash: Vec<Kept>,
}

fn five() -> u64 {
    v5::VERSION
}

impl Wadspace {
    pub fn name(&self) -> &str {
        self.customization.get("name").and_then(Value::as_str).unwrap_or("")
    }
    pub fn deleted(&self) -> bool {
        self.deleted_at.as_deref().is_some_and(|d| !d.is_empty())
    }
    /// Its document, as wad_core::v5 reads it.
    pub fn doc(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
    pub fn has_app(&self, app: &str) -> bool {
        self.apps.iter().any(|a| a.app == app)
    }
    /// Its add-ons by the app they're for: WadBrowser's, VS Code's, the
    /// command line's, then any other's.
    pub fn exts_by_app(&self) -> Vec<(String, Vec<&Ext>)> {
        let mut apps: Vec<&str> = vec![];
        for x in &self.exts {
            if !apps.contains(&x.app.as_str()) {
                apps.push(&x.app);
            }
        }
        let rank = |a: &str| crate::catalog::EXT_ORDER.iter().position(|o| *o == a).unwrap_or(99);
        apps.sort_by_key(|a| rank(a));
        apps.into_iter().map(|a| (a.to_string(), self.exts.iter().filter(|x| x.app == a).collect())).collect()
    }
    pub fn wallpaper(&self) -> Option<&Value> {
        self.customization.get("wallpaper").filter(|w| !w.is_null())
    }
    pub fn projects(&self) -> &[Value] {
        self.customization.get("projects").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }
    fn secrets(&mut self) -> &mut Map<String, Value> {
        let s = self.customization.entry("secrets").or_insert_with(|| json!({"bySlot": {}, "groups": []}));
        if !s.is_object() {
            *s = json!({"bySlot": {}, "groups": []});
        }
        let s = s.as_object_mut().unwrap();
        for (k, empty) in [("bySlot", json!({})), ("groups", json!([]))] {
            if !s.get(k).is_some_and(|v| v.is_object() || v.is_array()) {
                s.insert(k.into(), empty);
            }
        }
        s
    }
    fn by_slot(&mut self) -> &mut Map<String, Value> {
        self.secrets()["bySlot"].as_object_mut().expect("an object")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// What a drag carries (the client makes it from what was grabbed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Payload {
    /// The whole card.
    Wadspace {
        ws: String,
    },
    /// Its customization: background, projects, keys.
    Customs {
        ws: String,
    },
    /// Just its keys.
    Secrets {
        ws: String,
    },
    /// One key, with what signs in with it.
    Key {
        ws: String,
        key: String,
    },
    /// One thing that signs in with a key: its slot, or (WadBrowser's) its
    /// member.
    Holder {
        ws: String,
        key: String,
        slot: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member: Option<String>,
    },
    /// All its apps.
    Apps {
        ws: String,
    },
    /// All of an app's add-ons.
    Exts {
        ws: String,
        app: String,
    },
    Project {
        ws: String,
        id: String,
    },
    Background {
        ws: String,
    },
    App {
        ws: String,
        app: String,
    },
    Ext {
        ws: String,
        app: String,
        id: String,
    },
    /// From the sidebar.
    CatalogApp {
        app: String,
    },
    CatalogExt {
        ext: Ext,
    },
    Texture {
        value: String,
    },
    CatalogProject {
        project: Value,
    },
}

impl Payload {
    /// The wadspace it's out of (none: the sidebar's).
    pub fn source(&self) -> Option<&str> {
        match self {
            Payload::Wadspace { ws }
            | Payload::Customs { ws }
            | Payload::Secrets { ws }
            | Payload::Key { ws, .. }
            | Payload::Holder { ws, .. }
            | Payload::Apps { ws }
            | Payload::Exts { ws, .. }
            | Payload::Project { ws, .. }
            | Payload::Background { ws }
            | Payload::App { ws, .. }
            | Payload::Ext { ws, .. } => Some(ws),
            _ => None,
        }
    }
}

/// Something picked (Ctrl+click), for Delete selected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pick {
    Wadspace { ws: String },
    Exts { ws: String, app: String },
    Project { ws: String, id: String },
    Background { ws: String },
}

/// One thing that signs in with a key: a slot ("app:vscode",
/// "ext:<id>"…), or one of WadBrowser's apps (its member, in group slot
/// "group:<id>").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HolderRef {
    pub slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// An edit to the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// What's dragged copied into a wadspace (from its own: nothing).
    Drop {
        payload: Payload,
        into: String,
    },
    /// A new wadspace on `machine` from what's dragged, before `before`
    /// (none: after the row's last).
    Fresh {
        payload: Payload,
        machine: String,
        #[serde(default)]
        before: Option<String>,
    },
    /// A new wadspace on `machine` from nothing, to fill in.
    Create {
        machine: String,
    },
    /// Wadspaces copied to a machine, in one step.
    CopyTo {
        ids: Vec<String>,
        machine: String,
    },
    Delete {
        picks: Vec<Pick>,
    },
    Restore {
        ws: String,
    },
    /// Something from a wadspace's Recently deleted back in it.
    PutBack {
        ws: String,
        kept: String,
    },
    /// Something in a wadspace's Recently deleted gone for good (none: all of it).
    Forget {
        ws: String,
        #[serde(default)]
        kept: Option<String>,
    },
    Rename {
        ws: String,
        name: String,
    },
    Describe {
        ws: String,
        description: String,
    },
    /// An app out (kept); its add-ons stay, waiting for it.
    RemoveApp {
        ws: String,
        app: String,
    },
    RemoveExt {
        ws: String,
        app: String,
        id: String,
    },
    SetBackground {
        ws: String,
        texture: Option<String>,
    },
    /// A project in (or out, when it's in already, by id).
    ToggleProject {
        ws: String,
        project: Value,
    },
    /// A slot's key (`{id, name, kind}`; none: no key).
    SetKey {
        ws: String,
        slot: String,
        key: Option<Value>,
    },
    SetGroupKey {
        ws: String,
        group: String,
        key: Option<Value>,
    },
    /// A WadBrowser app into a group (none: a new group of its own).
    MoveMember {
        ws: String,
        member: String,
        to: Option<String>,
    },
    /// A key's name, everywhere it's used.
    RenameKey {
        id: String,
        name: String,
    },
    /// These sign in with `key` (a new one, or one it has; none: no key):
    /// slots set, WadBrowser's apps into the group with it.
    AssignKey {
        ws: String,
        holders: Vec<HolderRef>,
        key: Option<Value>,
    },
    Undo,
}

/// What a drag brings to where it lands.
#[derive(Default)]
struct Got {
    background: Option<Value>,
    projects: Vec<Value>,
    secrets: Option<Value>,
    description: Option<String>,
    apps: Vec<AppIn>,
    exts: Vec<Ext>,
    /// Keys merged in: slots set, WadBrowser's apps in groups with them.
    keys: Option<(Map<String, Value>, Vec<KeyGroup>)>,
}

/// The board: the account's machines and version 5 wadspaces (deleted
/// ones too, for Recently deleted), in board order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Board {
    #[serde(default)]
    pub machines: Vec<Machine>,
    #[serde(default)]
    pub wadspaces: Vec<Wadspace>,
    /// What a delete stamps (the caller's clock).
    #[serde(default)]
    pub now: String,
    /// Ids taken by other documents in the same collection (ones the board
    /// doesn't show): a new wadspace never takes one.
    #[serde(default)]
    pub reserved: Vec<String>,
    #[serde(skip)]
    past: Vec<Vec<Wadspace>>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A copy's name: the original's with " - n", n counting the copies.
pub fn copy_name(name: &str, taken: &[String]) -> String {
    let base = strip_counter(name);
    if base.trim().is_empty() {
        return name.to_string();
    }
    let prefix = format!("{base} - ");
    let n = taken
        .iter()
        .filter_map(|t| t.strip_prefix(&prefix))
        .filter(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|rest| rest.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{base} - {}", n + 1)
}

/// `name` without a " - <digits>" end.
fn strip_counter(name: &str) -> &str {
    if let Some(i) = name.rfind(" - ") {
        let rest = &name[i + 3..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return &name[..i];
        }
    }
    name
}

/// `base`, or base-2, base-3… if that's taken.
fn free(base: &str, taken: &[String]) -> String {
    let base = if base.is_empty() { "x" } else { base };
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).unwrap()
}

/// The first group id g1, g2… not taken.
fn free_group(taken: &[String]) -> String {
    (1..).map(|n| format!("g{n}")).find(|g| !taken.contains(g)).unwrap()
}

impl Board {
    pub fn new(machines: Vec<Machine>, wadspaces: Vec<Wadspace>, now: &str) -> Self {
        Self { machines, wadspaces, now: now.into(), reserved: vec![], past: vec![] }
    }

    /// The wadspaces as read (again); Undo keeps its steps when `keep`
    /// (the board caught up with what it saved), else starts afresh.
    pub fn load(&mut self, wadspaces: Vec<Wadspace>, keep: bool) {
        self.wadspaces = wadspaces;
        if !keep {
            self.past.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Wadspace> {
        self.wadspaces.iter().find(|w| w.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Result<&mut Wadspace, String> {
        self.wadspaces.iter_mut().find(|w| w.id == id).ok_or_else(|| format!("no wadspace {id}"))
    }

    /// A machine's row: its wadspaces that aren't deleted, in board order.
    pub fn row(&self, machine: &str) -> Vec<&Wadspace> {
        self.wadspaces.iter().filter(|w| w.machine == machine && !w.deleted()).collect()
    }

    /// Recently deleted, newest first.
    pub fn deleted(&self) -> Vec<&Wadspace> {
        let mut d: Vec<&Wadspace> = self.wadspaces.iter().filter(|w| w.deleted()).collect();
        d.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        d
    }

    /// Applies an edit; an edit that can't be made changes nothing.
    pub fn apply(&mut self, op: &Op) -> Result<(), String> {
        if let Op::Undo = op {
            let last = self.past.pop().ok_or("nothing to undo")?;
            self.wadspaces = last;
            return Ok(());
        }
        self.apply_all(std::slice::from_ref(op))
    }

    /// Edits made together, one step to undo (a new wadspace with what
    /// went into it); if one can't be made, none is.
    pub fn apply_all(&mut self, ops: &[Op]) -> Result<(), String> {
        let before = self.wadspaces.clone();
        for op in ops {
            let done = match op {
                Op::Undo => Err("undo goes on its own".to_string()),
                op => self.edit(op),
            };
            if let Err(e) = done {
                self.wadspaces = before;
                return Err(e);
            }
        }
        if self.wadspaces != before {
            self.past.push(before);
            if self.past.len() > UNDO_STEPS {
                self.past.remove(0);
            }
        }
        Ok(())
    }

    fn edit(&mut self, op: &Op) -> Result<(), String> {
        match op {
            Op::Drop { payload, into } => self.drop(payload, into),
            Op::Fresh { payload, machine, before } => self.fresh(payload, machine, before.as_deref()).map(drop),
            Op::Create { machine } => {
                self.create(machine);
                Ok(())
            }
            Op::CopyTo { ids, machine } => {
                for id in ids {
                    self.fresh(&Payload::Wadspace { ws: id.clone() }, machine, None)?;
                }
                Ok(())
            }
            Op::Delete { picks } => self.delete(picks),
            Op::Restore { ws } => {
                self.get_mut(ws)?.deleted_at = None;
                Ok(())
            }
            Op::PutBack { ws, kept } => {
                let w = self.get_mut(ws)?;
                let i = w.trash.iter().position(|k| k.id == *kept).ok_or("that's not in Recently deleted")?;
                let got = match w.trash.remove(i).piece {
                    Piece::Exts { exts, .. } => Got { exts, ..Got::default() },
                    Piece::App { app } => Got { apps: vec![app], ..Got::default() },
                    Piece::Project { project } => Got { projects: vec![project], ..Got::default() },
                    Piece::Background { wallpaper } => Got { background: Some(wallpaper), ..Got::default() },
                };
                Self::take(w, got);
                Ok(())
            }
            Op::Forget { ws, kept } => {
                let w = self.get_mut(ws)?;
                match kept {
                    Some(k) => w.trash.retain(|x| x.id != *k),
                    None => w.trash.clear(),
                }
                Ok(())
            }
            Op::Rename { ws, name } => {
                self.get_mut(ws)?.customization.insert("name".into(), name.clone().into());
                Ok(())
            }
            Op::Describe { ws, description } => {
                self.get_mut(ws)?.customization.insert("description".into(), description.clone().into());
                Ok(())
            }
            Op::RemoveApp { ws, app } => {
                let w = self.get_mut(ws)?;
                let Some(i) = w.apps.iter().position(|a| a.app == *app) else { return Ok(()) };
                let app = w.apps.remove(i);
                self.keep(ws, Piece::App { app })
            }
            Op::RemoveExt { ws, app, id } => {
                self.get_mut(ws)?.exts.retain(|x| !(x.app == *app && x.id == *id));
                Ok(())
            }
            Op::SetBackground { ws, texture } => {
                if let Some(t) = texture.as_deref().filter(|t| !v5::TEXTURES.contains(t)) {
                    return Err(format!("there's no texture {t}"));
                }
                let w = self.get_mut(ws)?;
                match texture {
                    Some(t) => drop(w.customization.insert("wallpaper".into(), json!({"type": "texture", "value": t}))),
                    None => {
                        if let Some(wallpaper) = w.customization.remove("wallpaper") {
                            self.keep(ws, Piece::Background { wallpaper })?;
                        }
                    }
                };
                Ok(())
            }
            Op::ToggleProject { ws, project } => {
                let w = self.get_mut(ws)?;
                let list = w.customization.entry("projects").or_insert_with(|| json!([]));
                let list = list.as_array_mut().ok_or("projects isn't a list")?;
                match list.iter().position(|p| s(p, "id") == s(project, "id")) {
                    Some(i) => {
                        let project = list.remove(i);
                        self.keep(ws, Piece::Project { project })?;
                    }
                    None => list.push(project.clone()),
                }
                Ok(())
            }
            Op::SetKey { ws, slot, key } => {
                let by = self.get_mut(ws)?.by_slot();
                match key {
                    Some(k) => by.insert(slot.clone(), k.clone()),
                    None => by.remove(slot),
                };
                Ok(())
            }
            Op::SetGroupKey { ws, group, key } => self.set_groups(ws, |gs| {
                for g in gs.iter_mut().filter(|g| g.id == *group) {
                    g.key = key.clone();
                }
            }),
            Op::MoveMember { ws, member, to } => self.set_groups(ws, |gs| {
                let ids: Vec<String> = gs.iter().map(|g| g.id.clone()).collect();
                for g in gs.iter_mut() {
                    g.members.retain(|m| m != member);
                }
                match to.as_deref().and_then(|t| gs.iter_mut().find(|g| g.id == t)) {
                    Some(g) => g.members.push(member.clone()),
                    None => gs.push(KeyGroup { id: free_group(&ids), members: vec![member.clone()], key: None }),
                }
            }),
            Op::RenameKey { id, name } => {
                for w in &mut self.wadspaces {
                    let Some(sec) = w.customization.get_mut("secrets") else { continue };
                    if let Some(by) = sec.get_mut("bySlot").and_then(Value::as_object_mut) {
                        for k in by.values_mut().filter(|k| s(k, "id") == id) {
                            k["name"] = name.clone().into();
                        }
                    }
                    if let Some(gs) = sec.get_mut("groups").and_then(Value::as_array_mut) {
                        for k in gs.iter_mut().filter_map(|g| g.get_mut("key")).filter(|k| s(k, "id") == id) {
                            k["name"] = name.clone().into();
                        }
                    }
                }
                Ok(())
            }
            Op::AssignKey { ws, holders, key } => self.assign_key(ws, holders, key.as_ref()),
            Op::Undo => unreachable!(),
        }
    }

    /// What `p` brings: copies, whatever it came out of.
    fn carry(&self, p: &Payload) -> Result<Got, String> {
        let src = |ws: &str| self.get(ws).ok_or_else(|| format!("no wadspace {ws}"));
        let c = |w: &Wadspace, k: &str| w.customization.get(k).filter(|v| !v.is_null()).cloned();
        let customs = |w: &Wadspace| Got {
            background: c(w, "wallpaper"),
            projects: w.projects().to_vec(),
            secrets: c(w, "secrets"),
            ..Got::default()
        };
        Ok(match p {
            Payload::Wadspace { ws } => {
                let w = src(ws)?;
                Got {
                    apps: w.apps.clone(),
                    exts: w.exts.clone(),
                    description: w.customization.get("description").and_then(Value::as_str).map(String::from),
                    ..customs(w)
                }
            }
            Payload::Customs { ws } => customs(src(ws)?),
            Payload::Secrets { ws } => Got { secrets: c(src(ws)?, "secrets"), ..Got::default() },
            Payload::Key { ws, key } => Got { keys: Some(self.keys_out(ws, key, None)?), ..Got::default() },
            Payload::Holder { ws, key, slot, member } => {
                Got { keys: Some(self.keys_out(ws, key, Some((slot, member.as_deref())))?), ..Got::default() }
            }
            Payload::Apps { ws } => Got { apps: src(ws)?.apps.clone(), ..Got::default() },
            Payload::Exts { ws, app } => {
                Got { exts: src(ws)?.exts.iter().filter(|x| x.app == *app).cloned().collect(), ..Got::default() }
            }
            Payload::Project { ws, id } => Got {
                projects: src(ws)?.projects().iter().filter(|p| s(p, "id") == id).cloned().collect(),
                ..Got::default()
            },
            Payload::Background { ws } => Got { background: c(src(ws)?, "wallpaper"), ..Got::default() },
            Payload::App { ws, app } => {
                let a = src(ws)?.apps.iter().find(|a| a.app == *app).ok_or("no such app")?;
                Got { apps: vec![a.clone()], ..Got::default() }
            }
            Payload::Ext { ws, app, id } => {
                let x = src(ws)?.exts.iter().find(|x| x.app == *app && x.id == *id).ok_or("no such add-on")?;
                Got { exts: vec![x.clone()], ..Got::default() }
            }
            Payload::CatalogApp { app } => Got { apps: vec![AppIn { app: app.clone() }], ..Got::default() },
            Payload::CatalogExt { ext } => Got { exts: vec![ext.clone()], ..Got::default() },
            Payload::Texture { value } => {
                Got { background: Some(json!({"type": "texture", "value": value})), ..Got::default() }
            }
            Payload::CatalogProject { project } => Got { projects: vec![project.clone()], ..Got::default() },
        })
    }

    /// What `got` brings, merged into `w`: the background and secrets
    /// replaced, projects, apps and add-ons added when they aren't in it (an
    /// add-on's app with it), keys' slots set and WadBrowser's apps that
    /// come with a key signing in with it together.
    fn take(w: &mut Wadspace, got: Got) {
        if let Some(d) = got.description {
            w.customization.insert("description".into(), d.into());
        }
        if let Some(b) = got.background {
            w.customization.insert("wallpaper".into(), b);
        }
        if !got.projects.is_empty() {
            let list = w.customization.entry("projects").or_insert_with(|| json!([]));
            if let Some(list) = list.as_array_mut() {
                for p in got.projects {
                    if !list.iter().any(|q| s(q, "id") == s(&p, "id")) {
                        list.push(p);
                    }
                }
            }
        }
        if let Some(sec) = got.secrets {
            w.customization.insert("secrets".into(), sec);
        }
        if let Some((by, groups)) = got.keys {
            w.by_slot().extend(by);
            let mut gs: Vec<KeyGroup> = serde_json::from_value(w.secrets()["groups"].clone()).unwrap_or_default();
            for g in groups {
                for o in gs.iter_mut() {
                    o.members.retain(|m| !g.members.contains(m));
                }
                gs.retain(|o| !o.members.is_empty());
                let ids: Vec<String> = gs.iter().map(|o| o.id.clone()).collect();
                gs.push(KeyGroup { id: free_group(&ids), ..g });
            }
            w.secrets().insert("groups".into(), serde_json::to_value(gs).unwrap_or(json!([])));
        }
        for a in got.apps {
            if !w.has_app(&a.app) {
                w.apps.push(a);
            }
        }
        for x in got.exts {
            // An add-on needs its app: it comes along when the wadspace hasn't got it.
            if !w.has_app(&x.app) {
                w.apps.push(AppIn { app: x.app.clone() });
            }
            if !w.exts.iter().any(|y| y.same(&x)) {
                w.exts.push(x);
            }
        }
    }

    fn drop(&mut self, p: &Payload, into: &str) -> Result<(), String> {
        self.get(into).ok_or_else(|| format!("no wadspace {into}"))?;
        // Nothing moves around inside its own wadspace.
        if p.source() == Some(into) {
            return Ok(());
        }
        let got = self.carry(p)?;
        Self::take(self.get_mut(into)?, got);
        Ok(())
    }

    /// The id a new wadspace called `name` gets: free among the board's
    /// (deleted ones too) and the reserved.
    pub fn free_id(&self, name: &str) -> String {
        let taken: Vec<String> = self.wadspaces.iter().map(|w| w.id.clone()).chain(self.reserved.clone()).collect();
        free(&spec::slugify(name), &taken)
    }

    /// Where a new wadspace goes: before `before`, else after its machine's last.
    fn place(&self, machine: &str, before: Option<&str>) -> usize {
        match before.and_then(|b| self.wadspaces.iter().position(|w| w.id == b)) {
            Some(i) => i,
            None => self.wadspaces.iter().rposition(|w| w.machine == machine).map_or(self.wadspaces.len(), |i| i + 1),
        }
    }

    fn blank(id: String, machine: &str, name: &str, owner: String) -> Wadspace {
        let mut c = Map::new();
        c.insert("name".into(), name.into());
        Wadspace {
            version: v5::VERSION,
            id,
            owner,
            shared_with: vec![],
            machine: machine.into(),
            order: 0,
            apps: vec![],
            exts: vec![],
            customization: c,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            trash: vec![],
        }
    }

    /// A new wadspace's name on `machine`: "New wadspace", or "New
    /// wadspace - 1"… when its row has one.
    pub fn new_name(&self, machine: &str) -> String {
        let names: Vec<String> = self.row(machine).iter().map(|w| w.name().to_string()).collect();
        if names.iter().any(|n| n == NEW_NAME) { copy_name(NEW_NAME, &names) } else { NEW_NAME.into() }
    }

    /// A new wadspace on `machine` from nothing; its id.
    fn create(&mut self, machine: &str) -> String {
        let name = self.new_name(machine);
        let id = self.free_id(&name);
        let at = self.place(machine, None);
        self.wadspaces.insert(at, Self::blank(id.clone(), machine, &name, String::new()));
        id
    }

    /// A new wadspace on `machine` from what's dragged; its id.
    fn fresh(&mut self, p: &Payload, machine: &str, before: Option<&str>) -> Result<String, String> {
        let from = match p.source() {
            Some(ws) => Some(self.get(ws).cloned().ok_or_else(|| format!("no wadspace {ws}"))?),
            None => None,
        };
        let name = match &from {
            None => self.new_name(machine),
            Some(w) if matches!(p, Payload::Wadspace { .. }) && w.machine != machine => w.name().to_string(),
            Some(w) => {
                let taken: Vec<String> = self.row(machine).iter().map(|x| x.name().to_string()).collect();
                copy_name(w.name(), &taken)
            }
        };
        let got = self.carry(p)?;
        let id = self.free_id(&name);
        let at = self.place(machine, before);
        let owner = from.map(|w| w.owner).unwrap_or_default();
        self.wadspaces.insert(at, Self::blank(id.clone(), machine, &name, owner));
        Self::take(self.get_mut(&id)?, got);
        Ok(id)
    }

    /// Wadspaces deleted (they wait, deleted); an app's add-ons, projects
    /// and backgrounds taken out (they wait in their wadspace's Recently
    /// deleted).
    fn delete(&mut self, picks: &[Pick]) -> Result<(), String> {
        let now = self.now.clone();
        for pick in picks {
            let (ws, piece) = match pick {
                Pick::Wadspace { ws } => {
                    self.get_mut(ws)?.deleted_at = Some(now.clone());
                    continue;
                }
                Pick::Exts { ws, app } => {
                    let w = self.get_mut(ws)?;
                    let (exts, rest): (Vec<Ext>, Vec<Ext>) = w.exts.drain(..).partition(|x| x.app == *app);
                    w.exts = rest;
                    if exts.is_empty() {
                        continue;
                    }
                    (ws, Piece::Exts { app: app.clone(), exts })
                }
                Pick::Project { ws, id } => {
                    let list = self.get_mut(ws)?.customization.get_mut("projects").and_then(Value::as_array_mut);
                    let Some(list) = list else { continue };
                    let Some(i) = list.iter().position(|p| s(p, "id") == id) else { continue };
                    (ws, Piece::Project { project: list.remove(i) })
                }
                Pick::Background { ws } => {
                    let Some(wallpaper) = self.get_mut(ws)?.customization.remove("wallpaper") else { continue };
                    (ws, Piece::Background { wallpaper })
                }
            };
            self.keep(ws, piece)?;
        }
        Ok(())
    }

    /// `piece` into `ws`'s Recently deleted (the newest KEEP_MOST stay).
    fn keep(&mut self, ws: &str, piece: Piece) -> Result<(), String> {
        let at = self.now.clone();
        let w = self.get_mut(ws)?;
        let id = (1..).map(|n| format!("k{n}")).find(|k| !w.trash.iter().any(|x| x.id == *k)).unwrap();
        w.trash.push(Kept { id, at, piece });
        if w.trash.len() > KEEP_MOST {
            w.trash.remove(0);
        }
        Ok(())
    }

    // ------------------------------------------------------------ saving

    /// The documents to save (new, changed, deleted) with `owner` and
    /// times set, each machine's row ordered 1, 2, 3… as it's shown.
    pub fn plan(&self, saved: &[Wadspace], owner: &str, now: &str) -> Vec<Value> {
        let mut order = std::collections::HashMap::<String, i64>::new();
        let mut out = vec![];
        for w in &self.wadspaces {
            let mut w = w.clone();
            if !w.deleted() {
                let n = order.entry(w.machine.clone()).or_insert(0);
                *n += 1;
                w.order = *n;
            }
            if w.owner.is_empty() {
                w.owner = owner.into();
            }
            let was = saved.iter().find(|x| x.id == w.id);
            let same = was.is_some_and(|x| {
                let mut x = x.clone();
                x.updated_at = w.updated_at.clone();
                x.created_at = w.created_at.clone();
                x == w
            });
            if same {
                continue;
            }
            w.created_at = was.and_then(|x| x.created_at.clone()).or_else(|| Some(now.into()));
            w.updated_at = Some(now.into());
            out.push(w.doc());
        }
        out
    }
}

/// Edits applied to a board, in turn: `{board, ops, saved?, owner?, now?}` →
/// `{wadspaces, plan, errors}` (each op's error, or null). What the goldens hold.
pub fn run(input: &Value) -> Result<Value, String> {
    let mut b: Board = serde_json::from_value(input["board"].clone()).map_err(|e| format!("board: {e}"))?;
    let ops: Vec<Op> = serde_json::from_value(input["ops"].clone()).map_err(|e| format!("ops: {e}"))?;
    let saved: Vec<Wadspace> =
        serde_json::from_value(input.get("saved").cloned().unwrap_or(json!([]))).map_err(|e| format!("saved: {e}"))?;
    let errors: Vec<Value> = ops.iter().map(|op| b.apply(op).err().map(Value::from).unwrap_or(Value::Null)).collect();
    let now = input.get("now").and_then(Value::as_str).unwrap_or("2026-10-21T00:00:00Z");
    let owner = input.get("owner").and_then(Value::as_str).unwrap_or("u1");
    Ok(json!({"wadspaces": b.wadspaces, "plan": b.plan(&saved, owner, now), "errors": errors}))
}

#[cfg(test)]
mod tests;
