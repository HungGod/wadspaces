//! The Builder's board in the app: the account's version 5 wadspaces on
//! wad_board::b's Board, each edit saved behind it: the library at once,
//! the account after; a change made elsewhere taken in when none of the
//! board's writes are out. What's built comes from this machine's images
//! and the others' heartbeats. One wadspace of the machine shown is always
//! selected (the sidebar's); Ctrl+click selects more.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use serde_json::Value;
use wad_board::b::{Board, Machine, Op, Payload, Pick, Wadspace};
use wad_proto::v1::ImageRow;

use crate::launch;
use crate::pages::machines::THIS;
use crate::store::{AppState, Topic, store};

/// The sidebar's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Apps,
    AddOns,
    Secrets,
    Customization,
}

impl Tab {
    /// The card's part it's for ("<ws>:<part>" flashes).
    pub fn part(self) -> &'static str {
        match self {
            Tab::Apps => "apps",
            Tab::AddOns => "exts",
            Tab::Secrets => "keys",
            Tab::Customization => "custom",
        }
    }
}

#[derive(Default)]
pub struct State {
    pub board: Board,
    /// The board's been read from the library.
    pub loaded: bool,
    /// The documents as last saved (what a save compares to).
    saved: Vec<Wadspace>,
    /// Writes out now.
    pending: u32,
    /// Why the last save failed, until one goes through.
    pub save_error: Option<String>,
    /// After a failed write, until the library's read back.
    stale: bool,
    /// Writes failed in a row (each waits longer before it's tried again).
    fails: u32,
    /// The machine whose wadspaces are shown (None: this one).
    pub shown: Option<String>,
    /// Wadspaces selected, the sidebar's last (one is, whatever this says:
    /// see [`primary`]).
    pub selected: Vec<String>,
    /// The sidebar's tab.
    pub tab: Tab,
    /// Pieces Ctrl+clicked, for Delete selected.
    pub picked: Vec<Pick>,
    /// A card's part something just landed in ("<ws>:<part>"), and which
    /// landing it was: it flashes once.
    pub flash: Option<(String, u32)>,
    /// This machine's images, by layer.
    pub images: HashMap<String, ImageRow>,
    /// A name being typed: the board isn't drawn again under it.
    pub renaming: bool,
    /// The sidebar shows Recently deleted.
    pub trash_open: bool,
    redraw: Vec<Box<dyn Fn()>>,
    scheduled: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub fn read<T>(f: impl FnOnce(&State) -> T) -> T {
    STATE.with(|s| f(&s.borrow()))
}

pub fn write<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// `f` runs whenever the board changes (one pass per GTK idle).
pub fn on_change(f: impl Fn() + 'static) {
    write(|s| s.redraw.push(Box::new(f)));
}

/// The board changed: drawn again once GTK's idle.
pub fn changed() {
    let first = write(|s| !std::mem::replace(&mut s.scheduled, true));
    if first {
        glib::idle_add_local_once(|| {
            let fs: Vec<Box<dyn Fn()>> = write(|s| {
                s.scheduled = false;
                std::mem::take(&mut s.redraw)
            });
            for f in &fs {
                f();
            }
            write(|s| {
                let mut later = std::mem::take(&mut s.redraw);
                s.redraw = fs;
                s.redraw.append(&mut later);
            });
        });
    }
}

/// This machine's id on the board: the account's for it (else "local", as
/// projects from it say).
pub fn here(s: &AppState) -> String {
    s.cloud.as_ref().filter(|c| c.linked).and_then(|c| c.machine_id.clone()).unwrap_or_else(|| THIS.into())
}

/// The machine shown: the one picked, else this one.
pub fn shown(s: &AppState) -> String {
    read(|st| st.shown.clone()).unwrap_or_else(|| here(s))
}

/// The machines, this one first, as the board names them.
fn machines(s: &AppState) -> Vec<Machine> {
    let me = here(s);
    crate::pages::machines::machines(s)
        .into_iter()
        .map(|m| Machine { id: if m.here { me.clone() } else { m.id }, name: m.label })
        .collect()
}

/// Wadspaces from the library, in each machine's order.
fn from_library(s: &AppState) -> Vec<Wadspace> {
    let mut ws: Vec<Wadspace> = s
        .library
        .iter()
        .flat_map(|l| l.wadspaces.iter())
        .filter_map(|d| match serde_json::from_value(d.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!("wadspace {}: not on the board: {e}", d["id"]);
                None
            }
        })
        .collect();
    ws.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    ws
}

/// The library read (or read again): the board from it, unless the board's
/// own writes are out or it has work not saved yet. Whether the board
/// changed: the store says something changed many times a second while a
/// build's on, and drawing the board again each time takes the hover off
/// what's under the pointer (and a click pressed on one button and released
/// on its new copy is no click).
pub fn take_library(s: &AppState) -> bool {
    if s.library.is_none() {
        return false;
    }
    let machines = machines(s);
    let docs = from_library(s);
    let reserved = s.library.as_ref().map(|l| l.reserved.clone()).unwrap_or_default();
    write(|st| {
        let news = st.board.machines != machines || st.board.reserved != reserved;
        st.board.machines = machines;
        st.board.reserved = reserved;
        if st.pending > 0 || st.stale {
            return news;
        }
        if st.loaded && unsaved(st) {
            return news;
        }
        // What it saved, read back: nothing to take (Undo keeps its steps).
        if st.loaded && by_id(&docs) == by_id(&st.saved) {
            return news;
        }
        st.board.load(docs.clone(), false);
        st.saved = docs;
        st.loaded = true;
        true
    })
}

/// Wadspaces in id order (the library's order isn't the board's).
fn by_id(ws: &[Wadspace]) -> Vec<&Wadspace> {
    let mut v: Vec<&Wadspace> = ws.iter().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// The board has changes not saved yet.
fn unsaved(st: &State) -> bool {
    !st.board.plan(&st.saved, "", "").is_empty()
}

/// An edit; saved and drawn again. Its error, if it couldn't be made.
pub fn apply(op: Op) -> Result<(), String> {
    apply_all(vec![op])
}

/// Edits made as one (one Undo).
pub fn apply_all(ops: Vec<Op>) -> Result<(), String> {
    let now = launch::now_iso();
    let live = |s: &State| s.board.wadspaces.iter().filter(|w| !w.deleted()).count();
    let (r, made) = write(|s| {
        s.board.now = now;
        let before = live(s);
        let r = s.board.apply_all(&ops);
        (r, live(s) > before)
    });
    if r.is_ok() {
        save();
        if made {
            crate::tour::signal(crate::tour::Event::WadspaceMade);
        }
    }
    changed();
    r
}

pub fn undo() {
    if write(|s| s.board.apply(&Op::Undo)).is_ok() {
        save();
    }
    changed();
}

/// Writes what changed: the library at once, the account behind.
pub fn save() {
    if read(|s| s.stale) {
        return;
    }
    let owner = crate::account::uid().unwrap_or_default();
    let now = launch::now_iso();
    let plan = read(|s| s.board.plan(&s.saved, &owner, &now));
    if plan.is_empty() {
        return;
    }
    let docs: Vec<Wadspace> = plan.iter().filter_map(|d| serde_json::from_value(d.clone()).ok()).collect();
    write(|s| {
        for d in &docs {
            match s.saved.iter_mut().find(|x| x.id == d.id) {
                Some(x) => *x = d.clone(),
                None => s.saved.push(d.clone()),
            }
            // The board's as saved: its place, owner and times.
            if let Some(w) = s.board.wadspaces.iter_mut().find(|w| w.id == d.id) {
                w.order = d.order;
                w.owner = d.owner.clone();
                w.created_at = d.created_at.clone();
                w.updated_at = d.updated_at.clone();
            }
        }
        s.pending += 1;
    });
    // The library as it'll be, at once.
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            for d in &plan {
                match l.wadspaces.iter_mut().find(|x| x["id"] == d["id"]) {
                    Some(x) => *x = d.clone(),
                    None => l.wadspaces.push(d.clone()),
                }
            }
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
    // A made-up machine (snapshots): nothing to write to.
    if crate::fake::on() {
        write(|s| s.pending = 0);
        return;
    }
    glib::spawn_future_local(async move {
        let writes = async {
            for d in plan {
                crate::account::write_wadspace(d).await?;
            }
            Ok::<(), String>(())
        };
        match writes.await {
            Ok(()) => {
                // Saved again after it couldn't: the Not saved chip goes.
                let cleared = write(|s| {
                    s.pending = s.pending.saturating_sub(1);
                    if s.pending == 0 {
                        s.fails = 0;
                    }
                    s.pending == 0 && s.save_error.take().is_some()
                });
                if cleared {
                    changed();
                }
            }
            Err(why) => {
                if read(|s| s.save_error.is_none()) {
                    crate::toast::error(
                        "Couldn't save the board",
                        &format!("{why} Your changes are kept here and saved when they can be."),
                    );
                }
                let fails = write(|s| {
                    s.save_error = Some(why);
                    s.stale = true;
                    s.pending = 0;
                    // What's saved is the library's, read back next.
                    s.saved.clear();
                    s.fails += 1;
                    s.fails
                });
                changed();
                crate::account::refresh_library().await;
                write(|s| s.stale = false);
                store().read_now(take_saved);
                // Tried again in a while (5 s, then longer, up to a minute),
                // not at once: a write that's refused (offline, or rules that
                // don't take it) would go round and round, drawing the board
                // each time. Retry, or the next edit, tries it now.
                let secs = (5u32 << (fails - 1).min(4)).min(60);
                glib::timeout_add_seconds_local_once(secs, save);
            }
        }
        changed();
    });
    changed();
}

/// After a failed write: what the account has, as the board's saved.
fn take_saved(s: &AppState) {
    let docs = from_library(s);
    write(|st| st.saved = docs);
}

/// Writes what isn't saved yet again.
pub fn retry() {
    write(|s| s.stale = false);
    save();
}

/// Signed out: nothing of theirs stays.
pub fn reset() {
    write(|s| {
        let redraw = std::mem::take(&mut s.redraw);
        *s = State { redraw, ..State::default() };
    });
    changed();
}

// ─────────────────────────────── built ───────────────────────────────

/// Where a wadspace stands on its machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Built,
    Update,
    Unbuilt,
    Building,
    /// Its machine hasn't said (offline, or never linked).
    Unknown,
}

/// This machine's images, read again.
pub fn refresh_images() {
    glib::spawn_future_local(async {
        if let Ok(rows) = crate::pages::machines::get::<Vec<ImageRow>>("/v1/images".into()).await {
            write(|s| s.images = rows.into_iter().map(|r| (r.layer.clone(), r)).collect());
            changed();
        }
    });
}

/// A machine's images, as wad_core::v5::state reads them: this one's from
/// wadd, another's from its last heartbeat.
fn images_of(s: &AppState, machine: &str) -> Option<Vec<Value>> {
    if machine == here(s) {
        return Some(read(|st| st.images.values().filter_map(|r| serde_json::to_value(r).ok()).collect()));
    }
    let beat = s.library.as_ref()?.machines.iter().find(|m| m["id"].as_str() == Some(machine))?;
    beat["images"].as_array().cloned()
}

pub fn standing(s: &AppState, w: &Wadspace) -> Standing {
    let doc = w.doc();
    let key = wad_core::v5::image_key(&doc);
    if w.machine == here(s) && s.builds.iter().any(|(b, _)| !b.status.finished() && b.layer.as_deref() == Some(&key)) {
        return Standing::Building;
    }
    let Some(images) = images_of(s, &w.machine) else { return Standing::Unknown };
    match wad_core::v5::state(&doc, &images) {
        "built" => Standing::Built,
        "update" => Standing::Update,
        _ => Standing::Unbuilt,
    }
}

/// Builds wadspaces where they live: here through wadd, on another machine
/// as a command its relay runs (it reads the saved document, so after the
/// board's writes).
pub fn build(ids: Vec<String>) {
    crate::tour::signal(crate::tour::Event::BuildStarted);
    glib::spawn_future_local(async move {
        for _ in 0..50 {
            if read(|s| s.pending == 0) {
                break;
            }
            glib::timeout_future(std::time::Duration::from_millis(200)).await;
        }
        let (here, list) = store().read_now(|s| {
            let list: Vec<(String, String, String)> = read(|st| {
                ids.iter()
                    .filter_map(|id| st.board.get(id))
                    .map(|w| (w.id.clone(), w.machine.clone(), w.name().to_string()))
                    .collect()
            });
            (here(s), list)
        });
        for (id, machine, name) in list {
            let r = if machine == here {
                launch::build_here(id).await
            } else {
                crate::account::command(machine.clone(), "build", id).await
            };
            match r {
                Ok(()) if machine == here => {
                    crate::toast::show(&format!("Building {name}"), None, crate::toast::Tone::Info)
                }
                Ok(()) => crate::toast::show(
                    &format!("{name} builds on its machine"),
                    Some("Its relay starts the build within a few seconds when the machine's on."),
                    crate::toast::Tone::Info,
                ),
                Err(e) => crate::toast::error(&format!("Couldn't build {name}"), &e),
            }
        }
        refresh_images();
    });
}

/// A wadspace deleted for good (from Recently deleted): its document goes.
pub fn delete_forever(id: String) {
    write(|s| {
        s.board.wadspaces.retain(|w| w.id != id);
        s.saved.retain(|w| w.id != id);
    });
    changed();
    if crate::fake::on() {
        return;
    }
    glib::spawn_future_local(async move {
        if let Err(e) = crate::account::delete_wadspace(id).await {
            crate::toast::error("Couldn't delete it for good", &e);
        }
    });
}

/// A thing is picked (Ctrl+click), or not any more.
pub fn toggle_pick(p: Pick) {
    write(|s| match s.picked.iter().position(|x| *x == p) {
        Some(i) => drop(s.picked.remove(i)),
        None => s.picked.push(p),
    });
    changed();
}

// ─────────────────────────────── selecting ───────────────────────────────

/// The sidebar's wadspace: the last selected that's in the machine's row,
/// else the row's first (None: the row's empty).
pub fn primary(st: &State, machine: &str) -> Option<String> {
    let row = st.board.row(machine);
    st.selected
        .iter()
        .rev()
        .find(|id| row.iter().any(|w| w.id == **id))
        .cloned()
        .or_else(|| row.first().map(|w| w.id.clone()))
}

/// Every wadspace selected (at least the sidebar's).
pub fn selected_ids(st: &State, machine: &str) -> Vec<String> {
    let row = st.board.row(machine);
    let ok: Vec<String> = st.selected.iter().filter(|id| row.iter().any(|w| w.id == **id)).cloned().collect();
    if ok.is_empty() { primary(st, machine).into_iter().collect() } else { ok }
}

/// A click selects it alone; Ctrl+click (`more`) adds it, or takes it out
/// (never the last one).
pub fn select(id: &str, more: bool) {
    let machine = store().read_now(shown);
    write(|st| {
        let now = selected_ids(st, &machine);
        if !more {
            st.selected = vec![id.to_string()];
            st.trash_open = false;
        } else if now.iter().any(|x| x == id) {
            if now.len() > 1 {
                st.selected = now.into_iter().filter(|x| x != id).collect();
            }
        } else {
            st.selected = now.into_iter().chain([id.to_string()]).collect();
            st.trash_open = false;
        }
    });
    changed();
}

/// A card's part flashes: something landed in it.
pub fn flash(ws: &str, part: &str) {
    write(|st| {
        let n = st.flash.as_ref().map_or(1, |(_, n)| n + 1);
        st.flash = Some((format!("{ws}:{part}"), n));
    });
    changed();
}

/// Edits of the sidebar's wadspace (on an empty machine, a new one made
/// first, the same Undo): `ops` for its id. Then it's the one selected.
pub fn edit_selected(ops: impl FnOnce(&str) -> Vec<Op>) -> Option<String> {
    let machine = store().read_now(shown);
    let (ws, mut all) = read(|st| match primary(st, &machine) {
        Some(ws) => (ws, vec![]),
        None => (st.board.free_id(&st.board.new_name(&machine)), vec![Op::Create { machine: machine.clone() }]),
    });
    all.extend(ops(&ws));
    match apply_all(all) {
        Ok(()) => {
            write(|st| st.selected = vec![ws.clone()]);
            changed();
            Some(ws)
        }
        Err(e) => {
            crate::toast::error("Couldn't do that", &e);
            None
        }
    }
}

/// An app or add-on from the sidebar into the sidebar's wadspace; its part
/// flashes.
pub fn add_from_sidebar(p: Payload) {
    let part = if matches!(p, Payload::CatalogExt { .. }) { "exts" } else { "apps" };
    if let Some(ws) = edit_selected(|ws| vec![Op::Drop { payload: p, into: ws.to_string() }]) {
        flash(&ws, part);
    }
}
