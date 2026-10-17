//! The board in the app (the web app's builder/store.ts around wad-board):
//! read from the account's library, each change saved behind it (the
//! library changes at once, the account's documents after), a change made
//! elsewhere taken in when none of the board's own writes are out. The
//! images here say what's built.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::glib;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wad_board::board::Note;
use wad_board::{Board, Deleted, Docs, Layer, Module, Stack};
use wad_proto::v1::ImageRow;
use wad_wadd_client::Method;

use crate::launch::{self, ImageState};
use crate::store::{AppState, Topic, store};

#[derive(Default)]
pub struct State {
    pub board: Board,
    /// The board's been read from the documents.
    pub loaded: bool,
    /// The board while its documents aren't all saved (written again on the
    /// next change, Retry, or the next read); None when they are.
    pub unsaved: Option<Vec<Stack>>,
    /// Why the last save failed, until one goes through.
    pub save_error: Option<String>,
    /// Writes out now.
    pending: u32,
    /// After a failed write, until the library's read back.
    stale: bool,
    /// The layer images here, by layer.
    pub images: HashMap<String, ImageRow>,
    /// The stack just made (to scroll to and light up).
    pub created: Option<String>,
    /// A module's name being typed: the board isn't drawn again under it.
    pub renaming: bool,
    /// The created stack's been brought into view.
    scrolled: bool,
    /// Draws the board again (the page's), when it's changed.
    redraw: Vec<Box<dyn Fn()>>,
    scheduled: bool,
    /// What's kept on this machine, as last written.
    kept: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// What's known now.
pub fn read<T>(f: impl FnOnce(&State) -> T) -> T {
    STATE.with(|s| f(&s.borrow()))
}

fn write<T>(f: impl FnOnce(&mut State) -> T) -> T {
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
            keep();
        });
    }
}

/// What the board keeps on this machine (the web app keeps it in the
/// browser): Recently deleted, and work not saved yet.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    deleted: Vec<Deleted>,
    unsaved: Option<Vec<Stack>>,
}

const KEPT: &str = "builder.json";

/// Written when it's changed (not on a made-up machine).
fn keep() {
    if crate::fake::on() {
        return;
    }
    let Some(path) = crate::account::data_file(KEPT) else { return };
    let text = read(|s| {
        if !s.loaded {
            return None;
        }
        serde_json::to_string(&Kept { deleted: s.board.deleted.clone(), unsaved: s.unsaved.clone() }).ok()
    });
    let Some(text) = text else { return };
    if write(|s| std::mem::replace(&mut s.kept, text.clone()) == text) {
        return;
    }
    if let Err(e) = wad_account::write_private(&path, text.as_bytes()) {
        tracing::warn!("{}: {e}", path.display());
    }
}

/// What was kept, read at the first start of the board.
fn kept() -> Kept {
    if crate::fake::on() {
        return Kept::default();
    }
    crate::account::data_file(KEPT)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// An edit of the board; saved, drawn again, and its note shown.
pub fn edit<T>(f: impl FnOnce(&mut Board) -> T) -> T {
    let (out, moved) = write(|s| {
        let v = s.board.version;
        let out = f(&mut s.board);
        (out, s.board.version != v)
    });
    if moved {
        save();
    }
    changed();
    out
}

/// A note from an edit, as a toast (with Undo when it has one).
pub fn tell(n: Option<Note>) {
    let Some(n) = n else { return };
    if n.undo {
        crate::toast::with_action(&n.text, crate::toast::Tone::Info, "Undo", || {
            edit(|b| b.undo());
        });
    } else {
        crate::toast::show(&n.text, None, crate::toast::Tone::Info);
    }
}

/// Signed out: nothing of theirs stays (the next person's board is read
/// afresh, with what this machine kept for them).
pub fn reset() {
    write(|s| {
        s.board = Board::default();
        s.loaded = false;
        s.unsaved = None;
        s.save_error = None;
        s.stale = false;
        s.created = None;
        s.renaming = false;
        s.kept.clear();
    });
    changed();
}

/// The documents as the library has them.
pub fn docs(s: &AppState) -> Option<Docs> {
    let l = s.library.as_ref()?;
    let mut extensions = l.extensions.clone();
    extensions.extend(l.customizations.iter().filter(|c| !l.extensions.iter().any(|e| e["id"] == c["id"])).cloned());
    Some(Docs { templates: l.templates.clone(), extensions, wadspaces: l.wadspaces.clone() })
}

/// A stack just made: it lights up and comes into view.
pub fn set_created(id: String) {
    write(|s| {
        s.created = Some(id.clone());
        s.scrolled = false;
    });
    changed();
    glib::timeout_add_local_once(std::time::Duration::from_millis(1800), move || {
        if write(|s| s.created.take_if(|c| *c == id).is_some()) {
            changed();
        }
    });
}

/// The stack to bring into view, once.
pub fn write_created_take() -> Option<String> {
    write(|s| s.created.clone().filter(|_| !std::mem::replace(&mut s.scrolled, true)))
}

/// A name's being typed (or not any more).
pub fn set_renaming(on: bool) {
    write(|s| s.renaming = on);
}

/// The library read (or read again): the board from it, unless the board's
/// own writes are out, or it has work not saved yet.
pub fn take_library(s: &AppState) {
    let Some(docs) = docs(s) else { return };
    // The first read: what this machine kept (Recently deleted, unsaved work).
    if read(|st| !st.loaded) {
        let k = kept();
        write(|st| {
            st.board.deleted = k.deleted;
            if st.unsaved.is_none() {
                st.unsaved = k.unsaved;
            }
        });
    }
    let retry = write(|st| {
        if let Some(unsaved) = st.unsaved.clone() {
            if !st.loaded {
                st.board.load(unsaved, false);
                st.loaded = true;
                return true;
            }
            return false;
        }
        if st.pending > 0 || st.stale {
            return false;
        }
        if st.loaded && !wad_board::plan(&st.board.stacks, &docs, "", "").writes() {
            return false;
        }
        let keep = st.loaded;
        st.board.load(wad_board::to_board(&docs), keep);
        st.loaded = true;
        false
    });
    if retry {
        glib::idle_add_local_once(save);
    }
    changed();
}

/// Writes the board's documents: the library at once, the account behind.
/// Until every write is in, the board is kept (`unsaved`).
pub fn save() {
    if read(|s| s.stale) {
        write(|s| s.unsaved = Some(s.board.stacks.clone()));
        return;
    }
    let Some(docs) = store().read_now(docs) else { return };
    let owner = crate::account::uid()
        .unwrap_or_else(|| store().read_now(|s| s.profile().map(|p| p.username.clone())).unwrap_or_default());
    let stacks = read(|s| s.board.stacks.clone());
    let p = wad_board::plan(&stacks, &docs, &owner, &launch::now_iso());
    if !p.writes() {
        write(|s| {
            if s.pending == 0 {
                s.unsaved = None;
                s.save_error = None;
            }
        });
        changed();
        return;
    }
    // The library as it'll be, at once; the board with its documents noted.
    let docs = p.docs.clone();
    store().update(|s| match s.library.as_mut() {
        Some(l) => {
            l.templates = docs.templates.clone();
            l.extensions = docs.extensions.clone();
            l.customizations.retain(|c| !docs.extensions.iter().any(|e| e["id"] == c["id"]));
            l.wadspaces = docs.wadspaces.clone();
            Topic::LIBRARY
        }
        None => Topic::NONE,
    });
    let refs: HashMap<String, String> =
        p.refs.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect();
    write(|s| {
        s.board.stacks = wad_board::with_refs(&s.board.stacks, &refs);
        s.unsaved = Some(s.board.stacks.clone());
        s.pending += 1;
    });
    // A made-up machine (snapshots): nothing to write to.
    if crate::fake::on() {
        write(|s| {
            s.pending = 0;
            s.unsaved = None;
        });
        return;
    }
    glib::spawn_future_local(async move {
        let deleted: Vec<Value> = p
            .deleted
            .iter()
            .filter_map(|id| p.docs.wadspaces.iter().find(|w| w["id"].as_str() == Some(id)).cloned())
            .collect();
        let writes = async {
            for t in &p.templates {
                crate::account::write_doc("templates", t.clone()).await?;
            }
            for e in &p.extensions {
                crate::account::write_doc("extensions", e.clone()).await?;
            }
            for w in p.wadspaces.iter().chain(deleted.iter()) {
                crate::account::write_doc("wadspaces", w.clone()).await?;
            }
            Ok::<(), String>(())
        };
        match writes.await {
            Ok(()) => write(|s| {
                s.pending = s.pending.saturating_sub(1);
                if s.pending == 0 && !s.stale {
                    s.unsaved = None;
                    s.save_error = None;
                }
            }),
            Err(why) => {
                let first = read(|s| s.save_error.is_none());
                if first {
                    crate::toast::error(
                        "Couldn't save the board",
                        &format!("{why} Your changes are kept here and saved when they can be."),
                    );
                }
                write(|s| {
                    s.save_error = Some(why);
                    s.unsaved = Some(s.board.stacks.clone());
                    s.stale = true;
                    s.pending = 0;
                });
                // The library back to what's saved; the board keeps its work.
                crate::account::refresh_library().await;
                write(|s| s.stale = false);
            }
        }
        changed();
    });
    changed();
}

/// Writes what isn't saved yet again.
pub fn retry() {
    if read(|s| s.pending > 0 || s.stale || s.unsaved.is_none()) {
        return;
    }
    save();
}

/// The images here, read again (what's built).
pub fn refresh_images() {
    glib::spawn_future_local(async {
        if let Ok(rows) = crate::pages::machines::get::<Vec<ImageRow>>("/v1/images".into()).await {
            write(|s| s.images = rows.into_iter().map(|r| (r.layer.clone(), r)).collect());
            changed();
        }
    });
}

/// Where a module stands here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Built,
    Update,
    Unbuilt,
    Building,
}

/// A layer image is building now.
fn building(key: &str, s: &AppState) -> bool {
    s.builds.iter().any(|(b, _)| !b.status.finished() && b.layer.as_deref() == Some(key))
}

/// Where a module stands (a customization has no image: built; a blank
/// one has nothing to build: built).
pub fn built(m: &Module, s: &AppState) -> Standing {
    let blank = match m {
        Module::Customization(_) => return Standing::Built,
        Module::Extensions(e) => e.subs.is_empty(),
        Module::Template(t) => t.apps.is_empty(),
    };
    if blank {
        return Standing::Built;
    }
    let Some(key) = wad_board::board::image_of(m) else { return Standing::Unbuilt };
    if building(&key, s) {
        return Standing::Building;
    }
    let lib = s.library.as_ref().map(|l| l.lib()).unwrap_or_default();
    read(|st| match launch::image_state(&key, &lib, &st.images) {
        ImageState::Built => Standing::Built,
        ImageState::Update => Standing::Update,
        _ => Standing::Unbuilt,
    })
}

/// A stack's image is building.
pub fn stack_building(stack: &Stack, s: &AppState) -> bool {
    stack.modules().iter().any(|m| wad_board::board::image_of(m).is_some_and(|k| building(&k, s)))
}

/// Its modules not built, and those with an update.
pub fn counts(stack: &Stack, s: &AppState) -> (usize, usize) {
    let states: Vec<Standing> = stack.modules().iter().map(|m| built(m, s)).collect();
    (
        states.iter().filter(|b| matches!(b, Standing::Unbuilt | Standing::Building)).count(),
        states.iter().filter(|b| **b == Standing::Update).count(),
    )
}

/// An image in words: its module's name and what it is ("Dev (template)").
fn image_label(m: &Module) -> String {
    let kind = match m {
        Module::Template(_) => "template",
        Module::Extensions(_) => "Extensions",
        Module::Customization(_) => "Customization",
    };
    format!("{} ({kind})", wad_board::module_name(m))
}

/// The images a stack's build makes or brings up to date, as (layer key,
/// words), template first.
pub fn images_to_build(stack: &Stack, s: &AppState) -> Vec<(String, String)> {
    stack
        .modules()
        .iter()
        .filter(|m| matches!(built(m, s), Standing::Unbuilt | Standing::Update))
        .filter_map(|m| wad_board::board::image_of(m).map(|k| (k, image_label(m))))
        .collect()
}

/// The images building now, in words (each once, though stacks share them).
pub fn images_building(stacks: &[Stack], s: &AppState) -> Vec<String> {
    let mut out: Vec<(String, String)> = vec![];
    for st in stacks {
        for m in &st.modules() {
            if let Some(k) = wad_board::board::image_of(m)
                && building(&k, s)
                && !out.iter().any(|(x, _)| *x == k)
            {
                out.push((k, image_label(m)));
            }
        }
    }
    out.into_iter().map(|(_, l)| l).collect()
}

/// A list in words: "a, b and c".
fn and_list(xs: &[String]) -> String {
    match xs {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Builds the images a stack doesn't have yet (installing it first).
pub fn build_stack(id: &str) {
    let (stack, why, counts) = store().read_now(|s| {
        let stack = read(|st| st.board.stack(id).cloned());
        let Some(stack) = stack else { return (None, None, vec![]) };
        let name = wad_board::stack_name(&stack);
        let why = if stack_building(&stack, s) {
            Some(String::new())
        } else if stack.apps().is_empty() {
            Some(format!("{name} needs a template to build on"))
        } else if !wad_board::catalog::problems_of(&stack).is_empty() {
            Some(format!("{name} needs fixing first"))
        } else if stack.look().is_none_or(|l| l.name.trim().is_empty()) {
            Some(format!("{name} needs a named Customization to be built"))
        } else {
            None
        };
        let c = images_to_build(&stack, s);
        (Some(stack), why, c)
    });
    let Some(stack) = stack else { return };
    match why {
        Some(w) if w.is_empty() => {}
        Some(w) => crate::toast::show(&w, None, crate::toast::Tone::Info),
        None => {
            let name = wad_board::stack_name(&stack);
            let images: Vec<String> = counts.into_iter().map(|(_, l)| l).collect();
            let id = id.to_string();
            crate::tour::signal(crate::tour::Event::BuildStarted);
            glib::spawn_future_local(async move {
                match launch::build_here(id).await {
                    Ok(()) => crate::toast::show(
                        &if images.is_empty() {
                            format!("{name} is installed, its images built already")
                        } else {
                            format!("Building {} for {name}", and_list(&images))
                        },
                        None,
                        crate::toast::Tone::Info,
                    ),
                    Err(e) => crate::toast::error(&format!("Couldn't build {name}"), &e),
                }
                refresh_images();
            });
        }
    }
}

/// Builds every stack that needs it and can be; says what it skipped.
pub fn build_all(only: Option<Vec<String>>) {
    let stacks = read(|s| s.board.stacks.clone());
    let (waiting, any_building) = store().read_now(|s| {
        let waiting: Vec<Stack> = stacks
            .iter()
            .filter(|st| {
                let (todo, up) = counts(st, s);
                todo + up > 0 && !stack_building(st, s) && only.as_ref().is_none_or(|o| o.contains(&st.id))
            })
            .cloned()
            .collect();
        (waiting, stacks.iter().any(|st| stack_building(st, s)))
    });
    if only.is_some() {
        for st in &waiting {
            build_stack(&st.id);
        }
        return;
    }
    if waiting.is_empty() {
        let t =
            if any_building { "Everything else is built and up to date" } else { "Everything's built and up to date" };
        crate::toast::show(t, None, crate::toast::Tone::Info);
        return;
    }
    let named = |st: &Stack| st.look().is_some_and(|l| !l.name.trim().is_empty());
    let fixed = |st: &Stack| wad_board::catalog::problems_of(st).is_empty();
    let no_template: Vec<&Stack> = waiting.iter().filter(|st| st.apps().is_empty()).collect();
    let broken: Vec<&Stack> = waiting.iter().filter(|st| !st.apps().is_empty() && !fixed(st)).collect();
    let unnamed: Vec<&Stack> = waiting.iter().filter(|st| !st.apps().is_empty() && fixed(st) && !named(st)).collect();
    let ready: Vec<&Stack> = waiting.iter().filter(|st| !st.apps().is_empty() && fixed(st) && named(st)).collect();
    for st in &ready {
        build_stack(&st.id);
    }
    let list = |xs: &[&Stack], what: &str| {
        if xs.is_empty() {
            None
        } else {
            Some(format!(
                "{} need{} {what}",
                xs.iter().map(|s| wad_board::stack_name(s)).collect::<Vec<_>>().join(", "),
                if xs.len() == 1 { "s" } else { "" }
            ))
        }
    };
    let skipped: Vec<String> =
        [list(&broken, "fixing"), list(&no_template, "a template"), list(&unnamed, "a named Customization")]
            .into_iter()
            .flatten()
            .collect();
    // Exactly the images: each once, though stacks share them.
    let images: Vec<String> = store().read_now(|s| {
        let mut seen: Vec<(String, String)> = vec![];
        for st in &ready {
            for (k, l) in images_to_build(st, s) {
                if !seen.iter().any(|(x, _)| *x == k) {
                    seen.push((k, l));
                }
            }
        }
        seen.into_iter().map(|(_, l)| l).collect()
    });
    crate::toast::show(
        &format!(
            "Building {} image{}: {}{}",
            images.len(),
            if images.len() == 1 { "" } else { "s" },
            and_list(&images),
            if skipped.is_empty() { String::new() } else { format!("; skipped: {}", skipped.join("; ")) }
        ),
        None,
        crate::toast::Tone::Info,
    );
}

// ─────────────────────────── gone for good ───────────────────────────

fn s_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn deleted_doc(v: &Value) -> bool {
    !s_of(v, "deletedAt").is_empty()
}

/// What's built on a document (`kind`: "template", "extensions"): their
/// names (`with_deleted`: those deleted too, said so).
fn dependents(s: &AppState, kind: &str, id: &str, with_deleted: bool) -> Vec<String> {
    let Some(d) = docs(s) else { return vec![] };
    let name = |v: &Value, n: String| if deleted_doc(v) { format!("{n} (recently deleted)") } else { n };
    let pick = |v: &&Value| with_deleted || !deleted_doc(v);
    match kind {
        "template" => d
            .templates
            .iter()
            .filter(pick)
            .filter(|t| s_of(t, "parent") == id)
            .map(|t| name(t, s_of(t, "name").into()))
            .chain(
                d.extensions
                    .iter()
                    .filter(pick)
                    .filter(|e| s_of(e, "templateId") == id)
                    .map(|e| name(e, s_of(e, "name").into())),
            )
            .chain(
                d.wadspaces
                    .iter()
                    .filter(pick)
                    .filter(|w| s_of(w, "template") == id)
                    .map(|w| name(w, s.wadspace_name(w))),
            )
            .collect(),
        "extensions" => d
            .wadspaces
            .iter()
            .filter(pick)
            .filter(|w| s_of(w, "extensions") == id)
            .map(|w| name(w, s.wadspace_name(w)))
            .collect(),
        _ => vec![],
    }
}

/// A document gone for good (`kind`: "template", "extensions",
/// "wadspace"), with its image here; refused while anything (deleted or
/// not) is built on it.
async fn purge(kind: &'static str, id: String) -> Result<(), String> {
    let deps = store().read_now(|s| dependents(s, kind, &id, true));
    if !deps.is_empty() {
        let one = deps.len() == 1;
        return Err(format!(
            "{} {} built on it: delete {} for good first.",
            deps.join(", "),
            if one { "is" } else { "are" },
            if one { "that" } else { "those" }
        ));
    }
    let (collection, image) = match kind {
        "template" => ("templates", Some(format!("template:{id}"))),
        "extensions" => ("extensions", Some(format!("custom:{id}"))),
        _ => ("wadspaces", None),
    };
    if let Some(layer) = &image {
        let path = format!("/v1/images/{}", crate::ui::enc(layer));
        if let Err(e) = crate::pages::machines::send::<Value>(Method::DELETE, path, None).await {
            tracing::debug!("{layer}: {e}");
        }
    }
    crate::account::delete_doc(collection, id).await?;
    if image.is_some() {
        refresh_images();
    }
    Ok(())
}

/// Wadspaces deleted with no entry in Recently deleted (before, or
/// elsewhere): restored or deleted for good from there too.
pub fn orphans(s: &AppState) -> Vec<Value> {
    let listed: Vec<String> = read(|st| {
        st.board
            .deleted
            .iter()
            .filter_map(|d| match d {
                Deleted::Module { module: Module::Customization(c), .. } => c.id.split('@').nth(1).map(String::from),
                _ => None,
            })
            .collect()
    });
    let on_board: Vec<String> = read(|st| st.board.stacks.iter().map(|x| x.id.clone()).collect());
    s.library
        .iter()
        .flat_map(|l| l.wadspaces.iter())
        .filter(|w| {
            deleted_doc(w) && !listed.iter().any(|l| l == s_of(w, "id")) && !on_board.iter().any(|b| b == s_of(w, "id"))
        })
        .cloned()
        .collect()
}

/// Recently deleted entries gone for good, with the documents behind them
/// that nothing else has (and their images here), and wadspaces left over.
pub fn delete_forever(ids: Vec<String>, orphans: Vec<String>) {
    let (going, staying, stacks) = read(|s| {
        let (g, k): (Vec<Deleted>, Vec<Deleted>) =
            s.board.deleted.iter().cloned().partition(|d| ids.iter().any(|i| i == d.id()));
        (g, k, s.board.stacks.clone())
    });
    write(|s| s.board.deleted = staying.clone());
    changed();
    // A document something still has (a stack, an entry left) stays.
    let kept_doc = |layer: Layer, doc: &str| {
        let on_board = stacks.iter().any(|st| match layer {
            Layer::Template => st.templates.iter().any(|t| t.doc.as_deref() == Some(doc)),
            _ => st.extensions.as_ref().and_then(|e| e.doc.as_deref()) == Some(doc),
        });
        on_board
            || staying.iter().any(
                |d| matches!(d, Deleted::Module { module, .. } if module.layer() == layer && module.doc() == Some(doc)),
            )
    };
    let (templates, wadspaces) = store().read_now(|s| {
        let d = docs(s).unwrap_or_default();
        (d.templates, d.wadspaces)
    });
    let mut purge_list: Vec<(&'static str, String, i32)> = vec![];
    for d in &going {
        let Deleted::Module { module, .. } = d else { continue };
        match module {
            Module::Customization(c) => {
                let stack_id = c.id.split('@').nth(1).unwrap_or_default();
                let gone = wadspaces.iter().any(|w| s_of(w, "id") == stack_id && deleted_doc(w));
                if gone && !stacks.iter().any(|st| st.id == stack_id) {
                    purge_list.push(("wadspace", stack_id.to_string(), 0));
                }
            }
            m => {
                let Some(doc) = m.doc().filter(|d| !kept_doc(m.layer(), d)) else { continue };
                if m.layer() == Layer::Template {
                    // Templates at the top of a chain first: one under another goes after it.
                    let mut depth = 0;
                    let mut t = templates.iter().find(|x| s_of(x, "id") == doc);
                    while let Some(p) = t.map(|x| s_of(x, "parent")).filter(|p| !p.is_empty()) {
                        depth -= 1;
                        t = templates.iter().find(|x| s_of(x, "id") == p);
                    }
                    purge_list.push(("template", doc.to_string(), 10 + depth));
                } else {
                    purge_list.push(("extensions", doc.to_string(), 1));
                }
            }
        }
    }
    let n = going.len() + orphans.len();
    for w in orphans {
        purge_list.push(("wadspace", w, 0));
    }
    purge_list.sort_by_key(|p| p.2);
    let mut seen: Vec<String> = vec![];
    purge_list.retain(|(k, id, _)| {
        let key = format!("{k}:{id}");
        !seen.contains(&key) && {
            seen.push(key);
            true
        }
    });
    glib::spawn_future_local(async move {
        let mut failed: Vec<String> = vec![];
        for (kind, id, _) in purge_list {
            if crate::fake::on() {
                continue;
            }
            if let Err(e) = purge(kind, id).await {
                failed.push(e);
            }
        }
        let text = if !failed.is_empty() {
            format!("Deleted for good, except: {}", failed.join(" "))
        } else if n == 1 {
            "Deleted it for good".to_string()
        } else {
            format!("Deleted {n} for good")
        };
        crate::toast::show(&text, None, crate::toast::Tone::Info);
    });
}

/// A wadspace deleted elsewhere (or before), back on the board: with the
/// Extensions and templates it's built on, if they went too.
pub fn restore_wadspace(id: String) {
    glib::spawn_future_local(async move {
        let (w, docs) = store().read_now(|s| {
            let w = s.library.iter().flat_map(|l| l.wadspaces.iter()).find(|w| s_of(w, "id") == id).cloned();
            (w, docs(s))
        });
        let (Some(w), Some(docs)) = (w, docs) else { return };
        let mut back: Vec<(&'static str, Value)> = vec![];
        if let Some(e) = docs.extensions.iter().find(|e| s_of(e, "id") == s_of(&w, "extensions") && deleted_doc(e)) {
            back.push(("extensions", e.clone()));
        }
        let mut t = docs.templates.iter().find(|t| s_of(t, "id") == s_of(&w, "template"));
        while let Some(x) = t {
            if deleted_doc(x) {
                back.push(("templates", x.clone()));
            }
            let parent = s_of(x, "parent");
            t = (!parent.is_empty()).then(|| docs.templates.iter().find(|y| s_of(y, "id") == parent)).flatten();
        }
        back.push(("wadspaces", w.clone()));
        let now = launch::now_iso();
        for (collection, mut doc) in back {
            if let Some(o) = doc.as_object_mut() {
                o.remove("deletedAt");
                o.insert("updatedAt".into(), now.clone().into());
            }
            if let Err(e) = crate::account::write_doc(collection, doc.clone()).await {
                crate::toast::error("Couldn't restore it", &e);
                return;
            }
            store().update(|s| match s.library.as_mut() {
                Some(l) => {
                    let list = match collection {
                        "templates" => &mut l.templates,
                        "extensions" => &mut l.extensions,
                        _ => &mut l.wadspaces,
                    };
                    match list.iter_mut().find(|x| x["id"] == doc["id"]) {
                        Some(x) => *x = doc.clone(),
                        None => list.push(doc.clone()),
                    }
                    Topic::LIBRARY
                }
                None => Topic::NONE,
            });
        }
        crate::toast::show(
            &format!("Restored {}", store().read_now(|s| s.wadspace_name(&w))),
            None,
            crate::toast::Tone::Info,
        );
    });
}

/// "5m ago" (the web app's timeAgo), from Unix milliseconds.
pub fn time_ago(ms: f64) -> String {
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
    let s = ((now - ms) / 1000.0).max(0.0);
    if s < 60.0 {
        return "just now".into();
    }
    let m = s / 60.0;
    if m < 60.0 {
        return format!("{}m ago", m.floor());
    }
    let h = m / 60.0;
    if h < 24.0 {
        return format!("{}h ago", h.floor());
    }
    let d = h / 24.0;
    if d < 30.0 {
        return format!("{}d ago", d.floor());
    }
    glib::DateTime::from_unix_local((ms / 1000.0) as i64)
        .ok()
        .and_then(|t| t.format("%x").ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// An ISO time as Unix milliseconds.
pub fn iso_ms(iso: &str) -> Option<f64> {
    glib::DateTime::from_iso8601(iso, None).ok().map(|t| t.to_unix() as f64 * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whats_built_on_a_document_is_named_deleted_or_not() {
        let lib = wad_account::Library {
            templates: vec![
                json!({ "id": "base", "name": "Base", "apps": ["vscode"] }),
                json!({ "id": "dev", "name": "Dev", "parent": "base", "apps": ["vscode", "wadbrowser"], "deletedAt": "2026-10-01T00:00:00Z" }),
            ],
            extensions: vec![json!({ "id": "e1", "name": "Dev setup", "templateId": "base" })],
            wadspaces: vec![
                json!({ "id": "w1", "template": "base", "extensions": "e1", "customization": { "name": "Site" } }),
            ],
            ..Default::default()
        };
        let s = AppState { library: Some(lib), ..Default::default() };
        assert_eq!(dependents(&s, "template", "base", false), vec!["Dev setup", "Site"]);
        assert_eq!(dependents(&s, "template", "base", true), vec!["Dev (recently deleted)", "Dev setup", "Site"]);
        assert_eq!(dependents(&s, "extensions", "e1", true), vec!["Site"]);
        assert!(dependents(&s, "template", "dev", true).is_empty());
    }

    #[test]
    fn signing_out_leaves_nothing_of_theirs() {
        write(|s| {
            s.board = Board::new(vec![Stack::new("theirs")], vec![]);
            s.loaded = true;
            s.unsaved = Some(vec![Stack::new("theirs")]);
        });
        write(|s| s.redraw.clear());
        reset();
        read(|s| {
            assert!(s.board.stacks.is_empty() && !s.board.can_undo());
            assert!(!s.loaded && s.unsaved.is_none());
        });
    }

    #[test]
    fn times_read_as_how_long_ago() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
        assert_eq!(time_ago(now - 10_000.0), "just now");
        assert_eq!(time_ago(now - 5.5 * 60_000.0), "5m ago");
        assert_eq!(time_ago(now - 3.0 * 3_600_000.0), "3h ago");
        assert_eq!(time_ago(now - 2.0 * 86_400_000.0), "2d ago");
        assert_eq!(iso_ms("1970-01-01T00:00:01Z"), Some(1000.0));
    }
}
