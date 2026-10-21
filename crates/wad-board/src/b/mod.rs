//! Design B's board: the Builder's version 4 wadspaces (wad_core::v4). Each
//! wadspace is on one machine, with its Templates bucket (its own templates,
//! each apps with their extras) and its Customizations bucket (background,
//! projects, keys). The edits are the design's spike's
//! (../ui-mockup-pieces/src/spikes/B-Buckets.tsx):
//!
//! - A drag takes a module and what's under it: a template the templates
//!   below it, the Customizations bucket the whole wadspace; Shift takes
//!   just the one (the client makes the payload). Single apps, extras,
//!   projects and the background go on their own.
//! - Into another wadspace it's a copy; within its own it moves. Between
//!   wadspaces or on the open board it starts a new one; onto a machine (the
//!   carousel) a new one there, a whole wadspace keeping its name.
//! - Deleting is soft (deletedAt), so a wadspace can be restored; Undo steps
//!   back.
//!
//! Every edit is an [`Op`] ([`Board::apply`]), as JSON: what the goldens
//! (fixtures/board/b.json) and the Android app's port replay. [`Board::plan`]
//! gives the documents to save.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use wad_core::{catalog, layers, spec, v4};

use crate::board::copy_name;
use crate::catalog::{self as cat, BrowserItem, KeyApp};
use crate::model::KeyGroup;

const UNDO_STEPS: usize = 40;
/// What a wadspace's Recently deleted keeps: the newest this many.
pub const KEEP_MOST: usize = 30;

/// A new wadspace's name, and its one empty template's id and name (made
/// by [`Op::Create`], to fill in).
pub const NEW_NAME: &str = "New wadspace";
pub const BLANK_TEMPLATE: &str = "template";
const BLANK_TEMPLATE_NAME: &str = "Template";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extra {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// An app in a template, with what it takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub app: String,
    /// WadBrowser's: "browser" or "focus".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default)]
    pub extras: Vec<Extra>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub apps: Vec<Entry>,
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

/// What's kept: a template and its place among the templates, an app off a
/// template (with its extras), a project, or the background.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Piece {
    Template { template: Template, place: usize },
    App { template: String, entry: Entry },
    Project { project: Value },
    Background { wallpaper: Value },
}

/// A version 4 wadspace document (its Firestore fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Wadspace {
    #[serde(default = "four")]
    pub version: u64,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default)]
    pub shared_with: Vec<String>,
    pub machine: String,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub templates: Vec<Template>,
    /// Version 3's: name, description, wallpaper, titleBars, autostart,
    /// projects, secrets.
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

fn four() -> u64 {
    v4::VERSION
}

impl Wadspace {
    pub fn name(&self) -> &str {
        self.customization.get("name").and_then(Value::as_str).unwrap_or("")
    }
    pub fn deleted(&self) -> bool {
        self.deleted_at.as_deref().is_some_and(|d| !d.is_empty())
    }
    /// Its document, as wad_core::v4 reads it.
    pub fn doc(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
    fn secrets(&mut self) -> &mut Map<String, Value> {
        let s = self.customization.entry("secrets").or_insert_with(|| json!({"bySlot": {}, "groups": []}));
        if !s.is_object() {
            *s = json!({"bySlot": {}, "groups": []});
        }
        s.as_object_mut().unwrap()
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
    /// A whole wadspace, by its name (or the carousel's picks).
    Wadspace {
        ws: String,
    },
    /// Templates of one wadspace: a pile, or just one.
    #[serde(rename_all = "camelCase")]
    Templates {
        ws: String,
        ids: Vec<String>,
    },
    /// A bucket by its bar: the Templates bucket's templates, or the
    /// Customizations bucket (with the Templates under it when `pile`).
    Bucket {
        ws: String,
        which: Which,
        #[serde(default)]
        pile: bool,
    },
    Project {
        ws: String,
        id: String,
    },
    Background {
        ws: String,
    },
    /// An app off a template, with its extras.
    App {
        ws: String,
        template: String,
        app: String,
    },
    /// One extra off an app.
    Extra {
        ws: String,
        template: String,
        app: String,
        id: String,
    },
    /// From the sidebar.
    #[serde(rename_all = "camelCase")]
    CatalogApp {
        app: String,
    },
    #[serde(rename_all = "camelCase")]
    CatalogExtra {
        app: String,
        extra: Extra,
    },
    Texture {
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    CatalogProject {
        project: Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Which {
    Templates,
    Customs,
}

/// Where in a wadspace a drop lands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Where {
    /// Templates: at this place in its Templates bucket (none: the end).
    #[serde(default)]
    pub at: Option<usize>,
    /// An app or extra: onto this template.
    #[serde(default)]
    pub template: Option<String>,
    /// An app or extra: into a new template of its own.
    #[serde(default)]
    pub new_template: bool,
}

/// Something picked (Ctrl+click), for Delete selected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pick {
    Wadspace { ws: String },
    Template { ws: String, id: String },
    Project { ws: String, id: String },
    Background { ws: String },
}

/// An edit to the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    Drop {
        payload: Payload,
        into: String,
        #[serde(default, rename = "where")]
        at: Where,
    },
    /// A new wadspace on `machine` from what's dragged, before `before`
    /// (none: after the row's last).
    Fresh {
        payload: Payload,
        machine: String,
        #[serde(default)]
        before: Option<String>,
    },
    /// A new wadspace on `machine` from nothing, to fill in: one empty
    /// template ([`BLANK_TEMPLATE`]) and a customization with only its name.
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
    /// Something from a wadspace's Recently deleted back where it was.
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
    RenameTemplate {
        ws: String,
        template: String,
        name: String,
    },
    RemoveApp {
        ws: String,
        template: String,
        app: String,
    },
    RemoveExtra {
        ws: String,
        template: String,
        app: String,
        id: String,
    },
    /// WadBrowser's mode in `template` (none: wherever it is in the
    /// wadspace). The top template with WadBrowser decides (v4::apps_of).
    SetMode {
        ws: String,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
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
    NewGroup {
        ws: String,
    },
    RemoveGroup {
        ws: String,
        group: String,
    },
    /// A key's name, everywhere it's used.
    RenameKey {
        id: String,
        name: String,
    },
    Undo,
}

/// What a drag brings to where it lands.
#[derive(Default)]
struct Got {
    templates: Option<Vec<Template>>,
    background: Option<Value>,
    projects: Vec<Value>,
    secrets: Option<Value>,
    description: Option<String>,
    entry: Option<Entry>,
}

/// The board: the account's machines and version 4 wadspaces (deleted
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
    /// Ids taken by other documents in the same collection (version 3
    /// wadspaces): a new wadspace never takes one.
    #[serde(default)]
    pub reserved: Vec<String>,
    #[serde(skip)]
    past: Vec<Vec<Wadspace>>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
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

fn app_name(app: &str) -> String {
    catalog::app(app).map(|a| a.name).unwrap_or_else(|| app.to_string())
}

/// An app's extras, with these added (once each, by id).
fn add_to(t: &mut Template, app: &str, extras: &[Extra]) {
    let i = match t.apps.iter().position(|e| e.app == app) {
        Some(i) => i,
        None => {
            t.apps.push(Entry { app: app.into(), mode: None, extras: vec![] });
            t.apps.len() - 1
        }
    };
    for x in extras {
        if !t.apps[i].extras.iter().any(|y| y.id == x.id) {
            t.apps[i].extras.push(x.clone());
        }
    }
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

    /// Edits made together, one step to undo (WadBrowser's apps moved as
    /// one); if one can't be made, none is.
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
            Op::Drop { payload, into, at } => self.drop(payload, into, at),
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
                let (got, place, onto) = match w.trash.remove(i).piece {
                    Piece::Template { template, place } => {
                        (Got { templates: Some(vec![template]), ..Got::default() }, Some(place), Where::default())
                    }
                    Piece::App { template, entry } => (
                        Got { entry: Some(entry), ..Got::default() },
                        None,
                        Where { template: Some(template), ..Where::default() },
                    ),
                    Piece::Project { project } => {
                        (Got { projects: vec![project], ..Got::default() }, None, Where::default())
                    }
                    Piece::Background { wallpaper } => {
                        (Got { background: Some(wallpaper), ..Got::default() }, None, Where::default())
                    }
                };
                Self::take(w, got, place, &onto);
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
            Op::RenameTemplate { ws, template, name } => {
                let w = self.get_mut(ws)?;
                let t = w.templates.iter_mut().find(|t| t.id == *template).ok_or("no such template")?;
                t.name = name.clone();
                Ok(())
            }
            Op::RemoveApp { ws, template, app } => {
                let w = self.get_mut(ws)?;
                let t = w.templates.iter_mut().find(|t| t.id == *template).ok_or("no such template")?;
                let Some(i) = t.apps.iter().position(|e| e.app == *app) else { return Ok(()) };
                let entry = t.apps.remove(i);
                self.keep(ws, Piece::App { template: template.clone(), entry })
            }
            Op::RemoveExtra { ws, template, app, id } => {
                let w = self.get_mut(ws)?;
                let t = w.templates.iter_mut().find(|t| t.id == *template).ok_or("no such template")?;
                if let Some(e) = t.apps.iter_mut().find(|e| e.app == *app) {
                    e.extras.retain(|x| x.id != *id);
                }
                Ok(())
            }
            Op::SetMode { ws, mode, template } => {
                if !matches!(mode.as_str(), "browser" | "focus") {
                    return Err(format!("WadBrowser has no mode {mode}"));
                }
                let w = self.get_mut(ws)?;
                if let Some(id) = template {
                    let t = w.templates.iter().find(|t| t.id == *id).ok_or("no such template")?;
                    if !t.apps.iter().any(|e| e.app == "wadbrowser") {
                        return Err(format!("{} has no WadBrowser", t.name));
                    }
                }
                for t in w.templates.iter_mut().filter(|t| template.as_ref().is_none_or(|id| t.id == *id)) {
                    for e in t.apps.iter_mut().filter(|e| e.app == "wadbrowser") {
                        e.mode = (mode == "focus").then(|| mode.clone());
                    }
                }
                Ok(())
            }
            Op::SetBackground { ws, texture } => {
                if let Some(t) = texture.as_deref().filter(|t| !v4::TEXTURES.contains(t)) {
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
                let secrets = self.get_mut(ws)?.secrets();
                let by = secrets.entry("bySlot").or_insert_with(|| json!({}));
                let by = by.as_object_mut().ok_or("bySlot isn't an object")?;
                match key {
                    Some(k) => by.insert(slot.clone(), k.clone()),
                    None => by.remove(slot),
                };
                Ok(())
            }
            Op::SetGroupKey { ws, group, key } => self.set_groups(ws, |gs| {
                for g in gs.iter_mut().filter(|g| s(g, "id") == group) {
                    match key {
                        Some(k) => drop(g.as_object_mut().map(|o| o.insert("key".into(), k.clone()))),
                        None => drop(g.as_object_mut().map(|o| o.remove("key"))),
                    }
                }
            }),
            Op::MoveMember { ws, member, to } => {
                let ids: Vec<String> = self.groups_of(ws).iter().map(|g| s(g, "id").to_string()).collect();
                let fresh = free_group(&ids);
                self.set_groups(ws, |gs| {
                    let was: Vec<bool> =
                        gs.iter().map(|g| g["members"].as_array().is_some_and(|m| m.is_empty())).collect();
                    for g in gs.iter_mut() {
                        if let Some(m) = g["members"].as_array_mut() {
                            m.retain(|x| x.as_str() != Some(member));
                        }
                    }
                    match to.as_deref().and_then(|t| gs.iter_mut().find(|g| s(g, "id") == t)) {
                        Some(g) => g["members"].as_array_mut().unwrap().push(member.clone().into()),
                        None => gs.push(json!({"id": fresh, "members": [member]})),
                    }
                    // A group its last app left goes; one made empty with New group stays.
                    let mut i = 0;
                    gs.retain(|g| {
                        let keep = !g["members"].as_array().is_some_and(|m| m.is_empty())
                            || was.get(i).copied().unwrap_or(true);
                        i += 1;
                        keep
                    });
                })
            }
            Op::NewGroup { ws } => {
                let ids: Vec<String> = self.groups_of(ws).iter().map(|g| s(g, "id").to_string()).collect();
                let id = free_group(&ids);
                self.set_groups(ws, |gs| gs.push(json!({"id": id, "members": []})))
            }
            Op::RemoveGroup { ws, group } => self.set_groups(ws, |gs| gs.retain(|g| s(g, "id") != group)),
            Op::RenameKey { id, name } => {
                for w in &mut self.wadspaces {
                    let sec = w.secrets();
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
            Op::Undo => unreachable!(),
        }
    }

    /// What `p` brings into wadspace `into`: moved out of its own wadspace
    /// when that's `into`, else copied.
    fn carry(&mut self, p: &Payload, into: &str) -> Result<Got, String> {
        let src = |b: &Self, ws: &str| b.get(ws).cloned().ok_or_else(|| format!("no wadspace {ws}"));
        let moving = |ws: &str| ws == into;
        let whole = |w: &Wadspace| Got {
            templates: Some(w.templates.clone()),
            background: w.customization.get("wallpaper").cloned(),
            projects: w.customization.get("projects").and_then(Value::as_array).cloned().unwrap_or_default(),
            secrets: w.customization.get("secrets").cloned(),
            description: w.customization.get("description").and_then(Value::as_str).map(String::from),
            entry: None,
        };
        Ok(match p {
            Payload::Wadspace { ws } => whole(&src(self, ws)?),
            Payload::Templates { ws, ids } => {
                let w = src(self, ws)?;
                let tpls: Vec<Template> = w.templates.iter().filter(|t| ids.contains(&t.id)).cloned().collect();
                if moving(ws) {
                    self.get_mut(ws)?.templates.retain(|t| !ids.contains(&t.id));
                }
                Got { templates: Some(tpls), ..Got::default() }
            }
            Payload::Bucket { ws, which, pile } => {
                let w = src(self, ws)?;
                match which {
                    Which::Templates => Got { templates: Some(w.templates.clone()), ..Got::default() },
                    Which::Customs if *pile => whole(&w),
                    Which::Customs => Got { templates: None, description: None, ..whole(&w) },
                }
            }
            Payload::Project { ws, id } => {
                let w = src(self, ws)?;
                let projects = w.customization.get("projects").and_then(Value::as_array).cloned().unwrap_or_default();
                Got { projects: projects.into_iter().filter(|p| s(p, "id") == id).collect(), ..Got::default() }
            }
            Payload::Background { ws } => {
                Got { background: src(self, ws)?.customization.get("wallpaper").cloned(), ..Got::default() }
            }
            Payload::App { ws, template, app } | Payload::Extra { ws, template, app, .. } => {
                let w = src(self, ws)?;
                let t = w.templates.iter().find(|t| t.id == *template).ok_or("no such template")?;
                let e = t.apps.iter().find(|e| e.app == *app).ok_or("no such app")?.clone();
                let extras = match p {
                    Payload::Extra { id, .. } => e.extras.iter().filter(|x| x.id == *id).cloned().collect(),
                    _ => e.extras.clone(),
                };
                if moving(ws) {
                    let wm = self.get_mut(ws)?;
                    let t = wm.templates.iter_mut().find(|t| t.id == *template).unwrap();
                    match p {
                        Payload::Extra { id, .. } => {
                            if let Some(e) = t.apps.iter_mut().find(|e| e.app == *app) {
                                e.extras.retain(|x| x.id != *id);
                            }
                        }
                        _ => t.apps.retain(|e| e.app != *app),
                    }
                }
                Got { entry: Some(Entry { app: e.app, mode: e.mode, extras }), ..Got::default() }
            }
            Payload::CatalogApp { app } => {
                Got { entry: Some(Entry { app: app.clone(), mode: None, extras: vec![] }), ..Got::default() }
            }
            Payload::CatalogExtra { app, extra } => Got {
                entry: Some(Entry { app: app.clone(), mode: None, extras: vec![extra.clone()] }),
                ..Got::default()
            },
            Payload::Texture { value } => {
                Got { background: Some(json!({"type": "texture", "value": value})), ..Got::default() }
            }
            Payload::CatalogProject { project } => Got { projects: vec![project.clone()], ..Got::default() },
        })
    }

    /// Templates in, their ids made their own in `w`.
    fn insert_templates(w: &mut Wadspace, at: usize, tpls: Vec<Template>) {
        let mut taken: Vec<String> = w.templates.iter().map(|t| t.id.clone()).collect();
        let tpls: Vec<Template> = tpls
            .into_iter()
            .map(|mut t| {
                t.id = free(&t.id, &taken);
                taken.push(t.id.clone());
                t
            })
            .collect();
        let at = at.min(w.templates.len());
        w.templates.splice(at..at, tpls);
    }

    /// A new template in `w` for an app, named as the app.
    fn new_template(w: &mut Wadspace, app: &str) -> usize {
        let name = app_name(app);
        let taken: Vec<String> = w.templates.iter().map(|t| t.id.clone()).collect();
        let id = free(&spec::slugify(&name), &taken);
        w.templates.push(Template { id, name, apps: vec![] });
        w.templates.len() - 1
    }

    fn take(w: &mut Wadspace, got: Got, at: Option<usize>, onto: &Where) {
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
        if let Some(t) = got.templates {
            let at = at.unwrap_or(w.templates.len());
            Self::insert_templates(w, at, t);
        }
        if let Some(e) = got.entry {
            let mut i = if onto.new_template {
                None
            } else {
                onto.template.as_ref().and_then(|t| w.templates.iter().position(|x| x.id == *t))
            };
            // A click in the sidebar: an extension goes where its app is.
            if i.is_none() && !onto.new_template && !e.extras.is_empty() {
                i = w.templates.iter().position(|t| t.apps.iter().any(|x| x.app == e.app));
            }
            let i = i.unwrap_or_else(|| Self::new_template(w, &e.app));
            add_to(&mut w.templates[i], &e.app, &e.extras);
            if let Some(m) = e.mode
                && let Some(x) = w.templates[i].apps.iter_mut().find(|x| x.app == e.app && x.mode.is_none())
            {
                x.mode = Some(m);
            }
        }
    }

    fn drop(&mut self, p: &Payload, into: &str, onto: &Where) -> Result<(), String> {
        self.get(into).ok_or_else(|| format!("no wadspace {into}"))?;
        // Templates moving within their own bucket: where they land once they're out.
        let mut at = onto.at;
        if let (Payload::Templates { ws, ids }, Some(a)) = (p, at)
            && ws == into
        {
            let w = self.get(into).unwrap();
            at = Some(w.templates.iter().take(a).filter(|t| !ids.contains(&t.id)).count());
        }
        let got = self.carry(p, into)?;
        let w = self.get_mut(into)?;
        Self::take(w, got, at, onto);
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

    fn blank(id: String, machine: &str, name: &str, owner: String, templates: Vec<Template>) -> Wadspace {
        let mut c = Map::new();
        c.insert("name".into(), name.into());
        Wadspace {
            version: v4::VERSION,
            id,
            owner,
            shared_with: vec![],
            machine: machine.into(),
            order: 0,
            templates,
            customization: c,
            created_at: None,
            updated_at: None,
            deleted_at: None,
            trash: vec![],
        }
    }

    /// A new wadspace on `machine` from nothing; its id.
    fn create(&mut self, machine: &str) -> String {
        let id = self.free_id(NEW_NAME);
        let at = self.place(machine, None);
        let t = Template { id: BLANK_TEMPLATE.into(), name: BLANK_TEMPLATE_NAME.into(), apps: vec![] };
        self.wadspaces.insert(at, Self::blank(id.clone(), machine, NEW_NAME, String::new(), vec![t]));
        id
    }

    /// A new wadspace on `machine` from what's dragged; its id.
    fn fresh(&mut self, p: &Payload, machine: &str, before: Option<&str>) -> Result<String, String> {
        let from = match p {
            Payload::Wadspace { ws }
            | Payload::Templates { ws, .. }
            | Payload::Bucket { ws, .. }
            | Payload::Project { ws, .. }
            | Payload::Background { ws }
            | Payload::App { ws, .. }
            | Payload::Extra { ws, .. } => Some(self.get(ws).cloned().ok_or_else(|| format!("no wadspace {ws}"))?),
            _ => None,
        };
        let whole = matches!(p, Payload::Wadspace { .. } | Payload::Bucket { pile: true, .. });
        let name = match &from {
            None => NEW_NAME.to_string(),
            Some(w) if whole && w.machine != machine => w.name().to_string(),
            Some(w) => {
                let taken: Vec<String> = self.row(machine).iter().map(|x| x.name().to_string()).collect();
                copy_name(w.name(), &taken)
            }
        };
        let id = self.free_id(&name);
        let at = self.place(machine, before);
        let owner = from.as_ref().map(|w| w.owner.clone()).unwrap_or_default();
        self.wadspaces.insert(at, Self::blank(id.clone(), machine, &name, owner, vec![]));
        let got = self.carry(p, &id)?;
        let w = self.get_mut(&id)?;
        Self::take(w, got, None, &Where { new_template: true, ..Where::default() });
        Ok(id)
    }

    /// Wadspaces deleted (they wait, deleted); templates, projects and
    /// backgrounds taken out (they wait in their wadspace's Recently deleted).
    fn delete(&mut self, picks: &[Pick]) -> Result<(), String> {
        let now = self.now.clone();
        for pick in picks {
            let (ws, piece) = match pick {
                Pick::Wadspace { ws } => {
                    self.get_mut(ws)?.deleted_at = Some(now.clone());
                    continue;
                }
                Pick::Template { ws, id } => {
                    let w = self.get_mut(ws)?;
                    let Some(place) = w.templates.iter().position(|t| t.id == *id) else { continue };
                    (ws, Piece::Template { template: w.templates.remove(place), place })
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

    // ------------------------------------------------------------ keys

    /// The key slots it has (wad_core's: an app's own, each AI extension's,
    /// each web app's).
    pub fn slots_of(&self, ws: &str) -> Vec<Value> {
        let Some(w) = self.get(ws) else { return vec![] };
        let (t, setup) = v4::as_template_setup(&w.doc());
        layers::slots_for(&t, &setup)
    }

    /// WadBrowser's apps in their groups, every one in exactly one: the
    /// browser and each web app (none without WadBrowser).
    pub fn groups_of(&self, ws: &str) -> Vec<Value> {
        let Some(w) = self.get(ws) else { return vec![] };
        let apps = v4::apps_of(&w.doc());
        let Some(browser) = apps.iter().find(|e| s(e, "app") == "wadbrowser") else { return vec![] };
        let mut members = vec!["browser".to_string()];
        members.extend(browser["extras"].as_array().into_iter().flatten().map(|x| format!("web:{}", s(x, "id"))));
        let stored = w.customization.get("secrets").and_then(|x| x.get("groups")).and_then(Value::as_array);
        let mut groups: Vec<Value> = stored
            .into_iter()
            .flatten()
            .map(|g| {
                let mut g = g.clone();
                let kept: Vec<Value> = g["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|m| m.as_str().is_some_and(|m| members.iter().any(|x| x == m)))
                    .cloned()
                    .collect();
                g["members"] = Value::Array(kept);
                g
            })
            .collect();
        let loose: Vec<Value> = members
            .iter()
            .filter(|m| !groups.iter().any(|g| g["members"].as_array().is_some_and(|l| l.iter().any(|x| x == *m))))
            .map(|m| Value::from(m.as_str()))
            .collect();
        if !loose.is_empty() {
            match groups.first_mut() {
                Some(g) => g["members"].as_array_mut().unwrap().extend(loose),
                None => groups.push(json!({"id": "g1", "members": loose})),
            }
        }
        groups
    }

    /// Its keys by app, as the client's key editor shows them (the old
    /// board's [`cat::KeyApp`]s): WadBrowser's groups, VS Code's settings and
    /// its extensions that sign in, each other app that keeps a key.
    pub fn key_map(&self, ws: &str) -> Vec<KeyApp> {
        let Some(w) = self.get(ws) else { return vec![] };
        let apps = v4::apps_of(&w.doc());
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
        let groups: Vec<KeyGroup> =
            self.groups_of(ws).into_iter().filter_map(|g| serde_json::from_value(g).ok()).collect();
        cat::key_map_of(&ids, items, &extensions, groups)
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

    /// Why key `id` can't be called `name` (another key is), or None.
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

    fn set_groups(&mut self, ws: &str, f: impl FnOnce(&mut Vec<Value>)) -> Result<(), String> {
        let mut gs = self.groups_of(ws);
        f(&mut gs);
        self.get_mut(ws)?.secrets().insert("groups".into(), Value::Array(gs));
        Ok(())
    }

    /// A new key `{id, name, kind}`: its name `name` unless that's taken
    /// (then name-2…), its id from it, unique among the board's keys.
    pub fn new_key(&self, name: &str, kind: &str) -> Value {
        let mut names = vec![];
        let mut ids = vec![];
        for w in &self.wadspaces {
            let sec = w.customization.get("secrets");
            let refs = sec
                .and_then(|x| x.get("bySlot"))
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|o| o.values())
                .chain(
                    sec.and_then(|x| x.get("groups"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|g| g.get("key")),
                );
            for k in refs {
                names.push(s(k, "name").to_string());
                ids.push(s(k, "id").to_string());
            }
        }
        let name = free(name, &names);
        let id = free(&spec::slugify(&name), &ids);
        json!({"id": id, "name": name, "kind": kind})
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
