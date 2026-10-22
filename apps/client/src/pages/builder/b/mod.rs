//! The Builder: one machine's wadspaces in a row, each one card (its
//! customization, secrets, add-ons and apps); the machines along the
//! bottom; the sidebar on the selected wadspace, always there (Recently
//! deleted in its place when that's open).
//!
//! Dragging a card takes all of it, a piece just that: into another
//! wadspace it's copied in; between wadspaces, after the last one or onto a
//! machine it starts a new wadspace (a card dropped on a machine is copied
//! there). One wadspace is always selected; Ctrl+click selects more, or
//! picks a piece. Delete deletes what's selected and picked; Undo steps
//! back.

pub mod card;
pub mod carousel;
pub mod sidebar;
pub mod state;
pub mod trash;

use gtk::{glib, prelude::*};
use wad_board::b::{Op, Pick};
use wad_gtk::Icon;

use self::state::{Standing, Tab};
use crate::drag;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

/// Recently deleted in the sidebar's place, or not.
pub fn show_trash(on: bool) {
    state::write(|s| s.trash_open = on);
    if on {
        sidebar::closed();
    }
    state::changed();
}

/// The Builder on a wadspace (from its details): its machine's row, it
/// selected; or (`copy`) a copy of it made at the row's end, selected.
pub fn show_wadspace(id: &str, copy: bool) {
    crate::shell::go(crate::shell::Page::Builder);
    let Some(w) = state::read(|s| s.board.get(id).cloned()) else { return };
    let mut sel = w.id.clone();
    if copy {
        let before: Vec<String> = state::read(|s| s.board.wadspaces.iter().map(|x| x.id.clone()).collect());
        let op = Op::Fresh {
            payload: wad_board::b::Payload::Wadspace { ws: w.id.clone() },
            machine: w.machine.clone(),
            before: None,
        };
        if state::apply(op).is_ok()
            && let Some(made) =
                state::read(|s| s.board.wadspaces.iter().find(|x| !before.contains(&x.id)).map(|x| x.id.clone()))
        {
            crate::toast::show(&format!("Copied {}", w.name()), None, crate::toast::Tone::Info);
            sel = made;
        }
    }
    let here = store().read_now(state::here);
    state::write(|s| {
        s.shown = (w.machine != here).then(|| w.machine.clone());
        s.selected = vec![sel];
        s.trash_open = false;
    });
    state::changed();
}

struct Page {
    row: gtk::Box,
    machines: gtk::Box,
    side: gtk::Box,
    undo: gtk::Button,
    build_all: gtk::Button,
    build_picked: gtk::Button,
    delete_picked: gtk::Button,
    deleted: gtk::Button,
    not_saved: gtk::Button,
}

pub fn page() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("wb");
    root.add_css_class("wbb");
    let (bar, mut p) = topbar();
    root.append(&bar);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("wbb-row");
    let scroll = gtk::ScrolledWindow::new();
    crate::tour::target("board", &scroll);
    scroll.add_css_class("wbb-board");
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.set_child(Some(&row));
    drag::autoscroll(&scroll);
    main.append(&scroll);
    let machines = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.append(&machines);
    body.append(&main);
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("wbb-side-slot");
    body.append(&side);
    root.append(&body);
    p.row = row;
    p.machines = machines;
    p.side = side;
    let p = std::rc::Rc::new(p);

    // Drawn again when the board changes (the page lives as long as the app).
    let pp = p.clone();
    state::on_change(move || render(&pp));
    let builds_seen: std::rc::Rc<std::cell::RefCell<String>> = Default::default();
    let machines_seen: std::rc::Rc<std::cell::RefCell<String>> = Default::default();
    // Drawn again only when something it shows changed (state::take_library).
    store().watch(&root, Topic::LIBRARY | Topic::ACTIVITY | Topic::WADD | Topic::CLOUD | Topic::MACHINE, move |s| {
        let mut redraw = state::take_library(s);
        // Builds starting or ending change what's built.
        let key: String = s.builds.iter().map(|(b, _)| format!("{}:{:?};", b.id, b.status)).collect();
        if *builds_seen.borrow() != key {
            let ended = !builds_seen.borrow().is_empty();
            builds_seen.replace(key);
            if ended {
                state::refresh_images();
            }
            redraw = true;
        }
        let key = carousel::key(s);
        if *machines_seen.borrow() != key {
            machines_seen.replace(key);
            redraw = true;
        }
        if redraw {
            state::changed();
        }
    });
    root.connect_map(|_| state::refresh_images());
    keys(&root);
    render(&p);
    root.upcast()
}

fn kbd(key: &str, text: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    b.add_css_class("hint");
    let k = gtk::Label::new(Some(key));
    k.add_css_class("kbd");
    b.append(&k);
    b.append(&gtk::Label::new(Some(text)));
    b
}

fn topbar() -> (gtk::Box, Page) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bar.add_css_class("topbar");
    let t = gtk::Label::new(Some("Builder"));
    t.add_css_class("wb-title");
    bar.append(&t);
    let hints = gtk::FlowBox::new();
    hints.add_css_class("hints");
    hints.set_selection_mode(gtk::SelectionMode::None);
    hints.set_max_children_per_line(2);
    hints.set_column_spacing(14);
    hints.set_row_spacing(4);
    hints.set_hexpand(false);
    hints.set_halign(gtk::Align::Start);
    hints.set_valign(gtk::Align::Center);
    for (k, text) in [
        ("drag", "a card: all of it · a piece: just it"),
        ("Ctrl+click", "selects more"),
        ("click", "an app or add-on adds it"),
        ("Delete", "deletes what's selected"),
    ] {
        let h = kbd(k, text);
        h.set_halign(gtk::Align::Start);
        hints.insert(&h, -1);
    }
    bar.append(&hints);
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    bar.append(&fill);

    let not_saved = ui::button("Not saved · Retry", Some("cloud-off"), Variant::Ghost, Size::Sm);
    not_saved.add_css_class("not-saved");
    not_saved.connect_clicked(|_| state::retry());
    bar.append(&not_saved);

    let new = gtk::Button::new();
    new.add_css_class("wbb-deleted");
    new.set_child(Some(&Icon::new("plus", 15)));
    new.set_tooltip_text(Some("New wadspace: an empty one on this machine, to build from"));
    new.update_property(&[gtk::accessible::Property::Label("New wadspace")]);
    new.connect_clicked(|_| new_wadspace());
    crate::tour::target("create", &new);
    bar.append(&new);

    let deleted = gtk::Button::new();
    deleted.add_css_class("wbb-deleted");
    deleted.set_child(Some(&Icon::new("rotate-ccw-clock", 15)));
    deleted.set_tooltip_text(Some(
        "Recently deleted: wadspaces, and what was taken out of them, to bring back or delete for good",
    ));
    deleted.update_property(&[gtk::accessible::Property::Label("Recently deleted")]);
    deleted.connect_clicked(|_| show_trash(!state::read(|s| s.trash_open)));
    bar.append(&deleted);

    let build_all = ui::button("Build all", Some("hammer"), Variant::Ghost, Size::Sm);
    crate::tour::target("build", &build_all);
    build_all.add_css_class("wb-ghost");
    build_all.connect_clicked(|_| {
        let ids = store().read_now(|s| {
            state::read(|st| {
                st.board
                    .wadspaces
                    .iter()
                    .filter(|w| !w.deleted())
                    .filter(|w| matches!(state::standing(s, w), Standing::Unbuilt | Standing::Update))
                    .map(|w| w.id.clone())
                    .collect::<Vec<_>>()
            })
        });
        state::build(ids);
    });
    bar.append(&build_all);
    let build_picked = ui::button("Build selected", Some("hammer"), Variant::Ghost, Size::Sm);
    build_picked.add_css_class("wb-ghost");
    build_picked.connect_clicked(|_| {
        let shown = store().read_now(state::shown);
        state::build(state::read(|st| state::selected_ids(st, &shown)));
    });
    bar.append(&build_picked);
    let delete_picked = ui::button("Delete selected", Some("trash"), Variant::Ghost, Size::Sm);
    delete_picked.add_css_class("wb-ghost");
    delete_picked.add_css_class("wbb-delete");
    delete_picked.connect_clicked(|_| delete_selected());
    bar.append(&delete_picked);
    let undo = ui::button("Undo", Some("undo-2"), Variant::Ghost, Size::Sm);
    undo.add_css_class("wb-ghost");
    undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
    undo.connect_clicked(|_| state::undo());
    bar.append(&undo);
    let p = Page {
        row: gtk::Box::new(gtk::Orientation::Horizontal, 0),
        machines: gtk::Box::new(gtk::Orientation::Vertical, 0),
        side: gtk::Box::new(gtk::Orientation::Vertical, 0),
        undo,
        build_all,
        build_picked,
        delete_picked,
        deleted,
        not_saved,
    };
    (bar, p)
}

/// A new empty wadspace on the machine shown (one Undo): selected, the
/// sidebar on Apps.
fn new_wadspace() {
    let machine = store().read_now(state::shown);
    let id = state::read(|s| s.board.free_id(&s.board.new_name(&machine)));
    if let Err(e) = state::apply(Op::Create { machine }) {
        return crate::toast::error("Couldn't make it", &e);
    }
    state::write(|s| {
        s.selected = vec![id];
        s.tab = Tab::Apps;
        s.trash_open = false;
    });
    state::changed();
}

/// Deletes the selected wadspaces (to Recently deleted) and the pieces
/// picked, with Undo; the next one along is selected.
fn delete_selected() {
    let shown = store().read_now(state::shown);
    let (ids, picks, row, primary) = state::read(|st| {
        let ids = state::selected_ids(st, &shown);
        let row: Vec<String> = st.board.row(&shown).iter().map(|w| w.id.clone()).collect();
        (ids, st.picked.clone(), row, state::primary(st, &shown))
    });
    let mut all: Vec<Pick> = ids.iter().map(|ws| Pick::Wadspace { ws: ws.clone() }).collect();
    all.extend(picks.iter().cloned());
    if all.is_empty() {
        return;
    }
    let at = row.iter().position(|w| Some(w) == primary.as_ref()).unwrap_or(0);
    let next = row[at + 1..].iter().chain(row[..at].iter().rev()).find(|w| !ids.contains(w)).cloned();
    let one = (ids.len() == 1 && picks.is_empty())
        .then(|| state::read(|st| st.board.get(&ids[0]).map(|w| w.name().to_string())));
    let n = all.len();
    if let Err(e) = state::apply(Op::Delete { picks: all }) {
        return crate::toast::error("Couldn't delete it", &e);
    }
    state::write(|s| {
        s.selected = next.into_iter().collect();
        s.picked.clear();
    });
    state::changed();
    let text = match one.flatten() {
        Some(name) => format!("Deleted {name}"),
        None => format!("Deleted {n} selected"),
    };
    crate::toast::with_action(&text, crate::toast::Tone::Info, "Undo", state::undo);
}

/// Ctrl+Z undoes; Delete deletes what's selected and picked; Escape lets
/// go of the extra selections and picks.
fn keys(root: &gtk::Box) {
    let k = gtk::EventControllerKey::new();
    let r = root.downgrade();
    k.connect_key_pressed(move |_, key, _, mods| {
        let Some(root) = r.upgrade() else { return glib::Propagation::Proceed };
        let typing =
            root.root().and_then(|w| w.focus()).is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::TextView>());
        if typing || !root.is_mapped() {
            return glib::Propagation::Proceed;
        }
        let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        match key {
            gtk::gdk::Key::z | gtk::gdk::Key::Z if ctrl => {
                state::undo();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Delete => {
                delete_selected();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Escape if state::read(|s| s.selected.len() > 1 || !s.picked.is_empty()) => {
                let shown = store().read_now(state::shown);
                state::write(|s| {
                    s.selected = state::primary(s, &shown).into_iter().collect();
                    s.picked.clear();
                });
                state::changed();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    root.add_controller(k);
}

/// Everything drawn again (not while a name's typed or a drag's on: then
/// once it's done).
fn render(p: &Page) {
    if state::read(|s| s.renaming) {
        return;
    }
    if drag::active() {
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), state::changed);
        return;
    }
    store().read_now(|s| {
        let shown = state::shown(s);
        draw_row(s, &p.row);
        ui::clear(&p.machines);
        p.machines.append(&carousel::bar(s));
        draw_side(s, p);
        let (can_undo, selected, picked, err, gone) = state::read(|st| {
            (
                st.board.can_undo(),
                state::selected_ids(st, &shown).len(),
                st.picked.len(),
                st.save_error.clone(),
                st.board.deleted().len() + st.board.wadspaces.iter().map(|w| w.trash.len()).sum::<usize>(),
            )
        });
        let unbuilt: Vec<String> = state::read(|st| {
            st.board
                .wadspaces
                .iter()
                .filter(|w| !w.deleted() && matches!(state::standing(s, w), Standing::Unbuilt | Standing::Update))
                .map(|w| w.name().to_string())
                .collect()
        });
        p.undo.set_sensitive(can_undo);
        p.build_all.set_sensitive(!unbuilt.is_empty());
        p.build_all.set_tooltip_text(Some(&if unbuilt.is_empty() {
            "Everything's built".to_string()
        } else {
            format!("Not built: {}", unbuilt.join(", "))
        }));
        p.build_picked.set_sensitive(selected > 0);
        ui::set_text(&p.build_picked, &format!("Build selected ({selected})"));
        let n = selected + picked;
        p.delete_picked.set_sensitive(n > 0);
        if picked > 0 {
            p.delete_picked.add_css_class("armed");
        } else {
            p.delete_picked.remove_css_class("armed");
        }
        ui::set_text(&p.delete_picked, &format!("Delete selected ({n})"));
        let trash_open = state::read(|st| st.trash_open);
        p.deleted.set_sensitive(gone > 0 || trash_open);
        if trash_open {
            p.deleted.add_css_class("on");
        } else {
            p.deleted.remove_css_class("on");
        }
        p.not_saved.set_visible(err.is_some());
        if let Some(e) = err {
            p.not_saved.set_tooltip_text(Some(&format!("{e} Your changes are kept here. Click to try again.")));
        }
    });
}

/// The sidebar on the selected wadspace (or on the empty machine's), or
/// Recently deleted in its place.
fn draw_side(s: &AppState, p: &Page) {
    ui::clear(&p.side);
    if state::read(|st| st.trash_open) {
        p.side.append(&trash::panel(s));
        return;
    }
    let shown = state::shown(s);
    let w = state::read(|st| state::primary(st, &shown).and_then(|id| st.board.get(&id).cloned()));
    p.side.append(&sidebar::sidebar(s, w.as_ref()));
}

fn draw_row(s: &AppState, row: &gtk::Box) {
    ui::clear(row);
    let shown = state::shown(s);
    if !state::read(|st| st.loaded) {
        let l = gtk::Label::new(Some("Reading your wadspaces…"));
        l.add_css_class("faint");
        l.set_margin_start(24);
        l.set_margin_top(24);
        l.set_valign(gtk::Align::Start);
        row.append(&l);
        return;
    }
    let (list, selected, primary) = state::read(|st| {
        let list: Vec<wad_board::b::Wadspace> = st.board.row(&shown).into_iter().cloned().collect();
        (list, state::selected_ids(st, &shown), state::primary(st, &shown))
    });
    // A machine with none: an empty one to build from.
    if list.is_empty() {
        row.append(&card::blank(&shown));
        row.append(&card::gap(&shown, None));
        return;
    }
    for w in &list {
        row.append(&card::gap(&shown, Some(&w.id)));
        let on = selected.contains(&w.id);
        row.append(&card::card(s, w, on, primary.as_deref() == Some(w.id.as_str())));
    }
    row.append(&card::gap(&shown, None));
}

/// Drags made by hand (the snapshots).
pub mod sim {
    use super::*;
    use wad_board::b::Payload as Bp;

    fn find(w: &gtk::Widget, id: &str) -> Option<gtk::Widget> {
        if w.widget_name() == id {
            return Some(w.clone());
        }
        let mut c = w.first_child();
        while let Some(x) = c {
            if let Some(f) = find(&x, id) {
                return Some(f);
            }
            c = x.next_sibling();
        }
        None
    }

    /// A widget by its name (a card's is its wadspace's id).
    fn named(id: &str) -> Option<gtk::Widget> {
        let root = gtk::Window::list_toplevels().into_iter().next()?;
        find(&root, id)
    }

    /// The first widget under `w` with CSS class `class`.
    fn with_class(w: &gtk::Widget, class: &str) -> Option<gtk::Widget> {
        if w.has_css_class(class) {
            return Some(w.clone());
        }
        let mut c = w.first_child();
        while let Some(x) = c {
            if let Some(f) = with_class(&x, class) {
                return Some(f);
            }
            c = x.next_sibling();
        }
        None
    }

    /// A card picked up and held over a machine's card.
    pub fn wadspace_over_machine(ws: &str, machine: &str) -> bool {
        let (Some(c), Some(m)) = (named(ws), carousel::card_of(machine)) else { return false };
        crate::drag::sim::begin(&c, carousel::wadspace_payload(ws), card::ghost("", ws));
        crate::drag::sim::over(&m, 0.0, 20.0);
        true
    }

    /// An app from the sidebar held over a card's Apps.
    pub fn app_over_apps(app: &str, ws: &str) -> bool {
        let Some(apps) = named(ws).and_then(|c| with_class(&c, "apps")) else { return false };
        let p = crate::drag::Payload::B(Bp::CatalogApp { app: app.into() });
        crate::drag::sim::begin(&apps, p, card::ghost(&card::app_icon(app), &card::app_name(app)));
        crate::drag::sim::over(&apps, 0.0, 10.0);
        true
    }

    /// A key off one card held over another's Secrets: New secret key shows.
    pub fn key_over_secrets(from: &str, key: &str, ws: &str) -> bool {
        let Some(keys) = named(ws).and_then(|c| with_class(&c, "keys")) else { return false };
        let p = crate::drag::Payload::B(Bp::Key { ws: from.into(), key: key.into() });
        crate::drag::sim::begin(&keys, p, card::ghost("", key));
        crate::drag::sim::over(&keys, 0.0, 10.0);
        true
    }

    /// An app from the sidebar held over the empty machine's card.
    pub fn app_over_blank(app: &str) -> bool {
        let Some(b) = named("blank-card") else { return false };
        let p = crate::drag::Payload::B(Bp::CatalogApp { app: app.into() });
        crate::drag::sim::begin(&b, p, card::ghost(&card::app_icon(app), &card::app_name(app)));
        crate::drag::sim::over(&b, 0.0, 60.0);
        true
    }

    /// The board scrolled to its start.
    pub fn board_to_start(ws: &str) {
        if let Some(sw) =
            named(ws).and_then(|c| c.ancestor(gtk::ScrolledWindow::static_type())).and_downcast::<gtk::ScrolledWindow>()
        {
            sw.hadjustment().set_value(0.0);
            sw.vadjustment().set_value(0.0);
        }
    }

    /// A deleted wadspace's background (Recently deleted's part) dropped on
    /// another's customization (it lands once the drop's drawn).
    pub fn background_from(deleted: &str, ws: &str) -> bool {
        let Some(customs) = named(ws).and_then(|c| with_class(&c, "customs")) else { return false };
        let p = crate::drag::Payload::B(Bp::Background { ws: deleted.into() });
        crate::drag::sim::begin(&customs, p, card::ghost("", "background"));
        crate::drag::sim::over(&customs, 20.0, 20.0);
        crate::drag::sim::end(true);
        true
    }

    /// The sidebar scrolled to its end.
    pub fn scroll_side_to_end() -> bool {
        let Some(root) = gtk::Window::list_toplevels().into_iter().next() else { return false };
        let Some(side) = with_class(&root, "wbb-side") else { return false };
        let Some(sw) = with_class(&side, "wbb-side-scroll").and_downcast::<gtk::ScrolledWindow>() else { return false };
        let a = sw.vadjustment();
        a.set_value(a.upper() - a.page_size());
        true
    }

    /// Let go (dropped where it is).
    pub fn drop_it() {
        crate::drag::sim::end(true);
    }
}
