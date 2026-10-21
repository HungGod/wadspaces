//! Builder B's board in the app: the account's version 4 wadspaces (the
//! library's `v4`) on wad_board::b's Board, each edit saved behind it as the
//! old board's are (../state.rs): the library at once, the account after; a
//! change made elsewhere taken in when none of the board's writes are out.
//! What's built comes from this machine's images and the others'
//! heartbeats.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use serde_json::Value;
use wad_board::b::{Board, Machine, Op, Pick, Wadspace};
use wad_proto::v1::ImageRow;

use crate::launch;
use crate::pages::machines::THIS;
use crate::store::{AppState, Topic, store};

/// The sidebar's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Apps,
    Customization,
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
    /// The machine whose wadspaces are shown (None: this one).
    pub shown: Option<String>,
    /// The wadspace open in the sidebar, and its tab.
    pub open: Option<String>,
    pub tab: Tab,
    /// Ctrl+clicked, for Build selected and Delete selected.
    pub picked: Vec<Pick>,
    /// Templates opened to their apps' windows: (wadspace, template).
    pub opened: Vec<(String, String)>,
    /// Secrets shown under a wadspace's desktop.
    pub secrets_shown: Vec<String>,
    /// This machine's images, by layer.
    pub images: HashMap<String, ImageRow>,
    /// A name being typed: the board isn't drawn again under it.
    pub renaming: bool,
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
    let mut ws: Vec<Wadspace> =
        s.library.iter().flat_map(|l| l.v4.iter()).filter_map(|d| serde_json::from_value(d.clone()).ok()).collect();
    ws.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    ws
}

/// The library read (or read again): the board from it, unless the board's
/// own writes are out or it has work not saved yet.
pub fn take_library(s: &AppState) {
    if s.library.is_none() {
        return;
    }
    let machines = machines(s);
    let docs = from_library(s);
    let reserved: Vec<String> =
        s.library.iter().flat_map(|l| l.wadspaces.iter()).filter_map(|w| w["id"].as_str().map(String::from)).collect();
    write(|st| {
        st.board.machines = machines;
        st.board.reserved = reserved;
        if st.pending > 0 || st.stale {
            return;
        }
        if st.loaded && unsaved(st) {
            return;
        }
        // What it saved, read back: nothing to take (Undo keeps its steps).
        if st.loaded && by_id(&docs) == by_id(&st.saved) {
            return;
        }
        st.board.load(docs.clone(), false);
        st.saved = docs;
        st.loaded = true;
    });
    changed();
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
    let r = write(|s| {
        s.board.now = now;
        s.board.apply_all(&ops)
    });
    if r.is_ok() {
        save();
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
                match l.v4.iter_mut().find(|x| x["id"] == d["id"]) {
                    Some(x) => *x = d.clone(),
                    None => l.v4.push(d.clone()),
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
                crate::account::write_doc("wadspaces", d).await?;
            }
            Ok::<(), String>(())
        };
        match writes.await {
            Ok(()) => write(|s| {
                s.pending = s.pending.saturating_sub(1);
                if s.pending == 0 {
                    s.save_error = None;
                }
            }),
            Err(why) => {
                if read(|s| s.save_error.is_none()) {
                    crate::toast::error(
                        "Couldn't save the board",
                        &format!("{why} Your changes are kept here and saved when they can be."),
                    );
                }
                write(|s| {
                    s.save_error = Some(why);
                    s.stale = true;
                    s.pending = 0;
                    // What's saved is the library's, read back next.
                    s.saved.clear();
                });
                crate::account::refresh_library().await;
                write(|s| s.stale = false);
                store().read_now(take_saved);
                save();
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

/// A machine's images, as wad_core::v4::state reads them: this one's from
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
    let key = wad_core::v4::image_key(&doc);
    if w.machine == here(s) && s.builds.iter().any(|(b, _)| !b.status.finished() && b.layer.as_deref() == Some(&key)) {
        return Standing::Building;
    }
    let Some(images) = images_of(s, &w.machine) else { return Standing::Unknown };
    match wad_core::v4::state(&doc, &images) {
        "built" => Standing::Built,
        "update" => Standing::Update,
        _ => Standing::Unbuilt,
    }
}

/// Builds wadspaces where they live: here through wadd, on another machine
/// as a command its relay runs (it reads the saved document, so after the
/// board's writes).
pub fn build(ids: Vec<String>) {
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
        if let Err(e) = crate::account::delete_doc("wadspaces", id).await {
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

/// The wadspaces picked: those picked whole, and those something in is.
pub fn picked_wadspaces(st: &State) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for p in &st.picked {
        let ws = match p {
            Pick::Wadspace { ws } | Pick::Template { ws, .. } | Pick::Project { ws, .. } | Pick::Background { ws } => {
                ws
            }
        };
        if !out.contains(ws) {
            out.push(ws.clone());
        }
    }
    out
}
