//! Builder B (design B: ../ui-mockup-pieces/src/spikes/B-Buckets.tsx), the
//! Builder when it's chosen (Try design B on the old board, kept in
//! ~/.config/wadspaces/builder; WADSPACES_BUILDER=b or =old wins) until it
//! takes the old board's place:
//! one machine's version 4 wadspaces in a row, each a Customizations bucket
//! (its desktop: background, projects, keys) over a Templates bucket (its
//! own templates: apps with what they take); the machines along the bottom;
//! the open wadspace's sidebar on the right.
//!
//! A drag takes a thing and what's under it (Shift: just it): into another
//! wadspace it's a copy, within its own a move; between wadspaces, on the
//! open board or onto a machine it starts a new wadspace, and a whole
//! wadspace dropped on a machine is copied there. Ctrl+click picks, for
//! Build selected and Delete selected; Undo steps back.

pub mod buckets;
pub mod carousel;
pub mod sidebar;
pub mod state;

use gtk::{glib, prelude::*};
use wad_board::b::{Op, Pick};
use wad_gtk::Icon;

use self::state::{Standing, Tab};
use crate::drag;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

/// Where the choice is kept (beside the theme).
fn choice_file() -> std::path::PathBuf {
    wad_gtk::theme::file().with_file_name("builder")
}

/// Builder B is the Builder: WADSPACES_BUILDER says (b or old), else the
/// choice kept.
pub fn enabled() -> bool {
    match std::env::var("WADSPACES_BUILDER").as_deref() {
        Ok("b") => true,
        Ok("old") => false,
        _ => std::fs::read_to_string(choice_file()).is_ok_and(|t| t.trim() == "b"),
    }
}

/// Builder B chosen (or the old board again): kept, and shown at once.
pub fn choose(on: bool) {
    let file = choice_file();
    let done = if on {
        file.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&file, "b\n"))
    } else {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    };
    if let Err(e) = done {
        tracing::warn!("{}: {e}", file.display());
    }
    super::show_chosen();
}

/// Opens a wadspace in the sidebar.
pub fn open(id: &str) {
    state::write(|s| s.open = Some(id.to_string()));
    state::changed();
}

/// Opens a wadspace in the sidebar, on a tab.
pub fn open_on(id: &str, tab: Tab) {
    state::write(|s| {
        s.open = Some(id.to_string());
        s.tab = tab;
    });
    state::changed();
}

/// The Builder on a wadspace (from its details): its machine's row, it
/// open in the sidebar; or (`copy`) a copy of it made at the row's end.
pub fn show_wadspace(id: &str, copy: bool) {
    crate::shell::go(crate::shell::Page::Builder);
    let Some(w) = state::read(|s| s.board.get(id).cloned()) else { return };
    let mut open = w.id.clone();
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
            open = made;
        }
    }
    let here = store().read_now(state::here);
    state::write(|s| {
        s.shown = (w.machine != here).then(|| w.machine.clone());
        s.open = Some(open);
    });
    state::changed();
}

pub fn close() {
    state::write(|s| s.open = None);
    sidebar::closed();
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
    deleted: gtk::MenuButton,
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
    row.set_valign(gtk::Align::Start);
    row.set_halign(gtk::Align::Start);
    let scroll = gtk::ScrolledWindow::new();
    scroll.add_css_class("wbb-board");
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.set_child(Some(&row));
    drag::autoscroll(&scroll);
    // A click on the open board (not on a wadspace) closes the sidebar.
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let sw = scroll.downgrade();
    click.connect_released(move |g, _, x, y| {
        let Some(sw) = sw.upgrade() else { return };
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        let mut at = sw.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(w) = at {
            if w.has_css_class("wbb-col") || w.has_css_class("wbb-gap") || w.has_css_class("wbb-open") {
                return;
            }
            at = w.parent();
        }
        close();
    });
    scroll.add_controller(click);
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
    store().watch(&root, Topic::LIBRARY | Topic::ACTIVITY | Topic::WADD | Topic::CLOUD | Topic::MACHINE, move |s| {
        state::take_library(s);
        // Builds starting or ending change what's built.
        let key: String = s.builds.iter().map(|(b, _)| format!("{}:{:?};", b.id, b.status)).collect();
        if *builds_seen.borrow() != key {
            let ended = !builds_seen.borrow().is_empty();
            builds_seen.replace(key);
            if ended {
                state::refresh_images();
            }
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
        ("drag", "copies or moves, with what's under it"),
        ("Shift", "just one"),
        ("drag", "a name onto a machine"),
        ("Ctrl+click", "picks"),
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

    let deleted = gtk::MenuButton::new();
    deleted.add_css_class("wbb-deleted");
    deleted.set_child(Some(&Icon::new("rotate-ccw-clock", 15)));
    deleted.set_tooltip_text(Some("Recently deleted: bring them back, or delete them for good"));
    deleted.update_property(&[gtk::accessible::Property::Label("Recently deleted")]);
    let pop = gtk::Popover::new();
    pop.add_css_class("wad-menu");
    pop.set_has_arrow(false);
    pop.connect_show(fill_deleted);
    deleted.set_popover(Some(&pop));
    bar.append(&deleted);

    let build_all = ui::button("Build all", Some("hammer"), Variant::Ghost, Size::Sm);
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
    build_picked.connect_clicked(|_| state::build(state::read(state::picked_wadspaces)));
    bar.append(&build_picked);
    let delete_picked = ui::button("Delete selected", Some("trash"), Variant::Ghost, Size::Sm);
    delete_picked.add_css_class("wb-ghost");
    delete_picked.add_css_class("wbb-delete");
    delete_picked.connect_clicked(|_| delete_picked_now());
    bar.append(&delete_picked);
    let undo = ui::button("Undo", Some("undo-2"), Variant::Ghost, Size::Sm);
    undo.add_css_class("wb-ghost");
    undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
    undo.connect_clicked(|_| state::undo());
    bar.append(&undo);
    let old = ui::button("Old Builder", None, Variant::Ghost, Size::Sm);
    old.set_tooltip_text(Some("Back to the board of templates and Extensions (your version 3 wadspaces)"));
    old.connect_clicked(|_| choose(false));
    bar.append(&old);
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

/// Recently deleted, newest first: each to bring back or delete for good.
fn fill_deleted(pop: &gtk::Popover) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.add_css_class("wbb-deleted-list");
    let gone: Vec<(String, String, String)> = store().read_now(|s| {
        let names: Vec<(String, String)> = crate::pages::machines::machines(s)
            .into_iter()
            .map(|m| (if m.here { state::here(s) } else { m.id }, m.label))
            .collect();
        state::read(|st| {
            st.board
                .deleted()
                .iter()
                .map(|w| {
                    let on = names.iter().find(|(id, _)| *id == w.machine).map(|(_, n)| n.clone()).unwrap_or_default();
                    (w.id.clone(), w.name().to_string(), on)
                })
                .collect()
        })
    });
    if gone.is_empty() {
        let l = gtk::Label::new(Some("Nothing deleted lately."));
        l.add_css_class("faint");
        l.set_margin_top(6);
        l.set_margin_bottom(6);
        l.set_margin_start(8);
        l.set_margin_end(8);
        col.append(&l);
    }
    for (id, name, on) in gone {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("wbb-deleted-row");
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        texts.set_hexpand(true);
        let n = gtk::Label::new(Some(&name));
        n.set_xalign(0.0);
        texts.append(&n);
        let m = gtk::Label::new(Some(&on));
        m.add_css_class("faint");
        m.add_css_class("tiny");
        m.set_xalign(0.0);
        texts.append(&m);
        row.append(&texts);
        let back = ui::button("Restore", None, Variant::Secondary, Size::Sm);
        let (i, pw) = (id.clone(), pop.downgrade());
        back.connect_clicked(move |_| {
            buckets::act(Op::Restore { ws: i.clone() });
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
        });
        row.append(&back);
        let gone = ui::icon_button("trash", "Delete it for good", 13);
        let pw = pop.downgrade();
        gone.connect_clicked(move |_| {
            state::delete_forever(id.clone());
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
        });
        row.append(&gone);
        col.append(&row);
    }
    pop.set_child(Some(&col));
}

/// Deletes what's picked (wadspaces go to Recently deleted), with Undo.
fn delete_picked_now() {
    let (picks, names) = state::read(|st| {
        let names: Vec<String> = st
            .picked
            .iter()
            .filter_map(|p| match p {
                Pick::Wadspace { ws } => st.board.get(ws).map(|w| w.name().to_string()),
                _ => None,
            })
            .collect();
        (st.picked.clone(), names)
    });
    if picks.is_empty() {
        return;
    }
    let n = picks.len();
    if state::apply(Op::Delete { picks }).is_err() {
        return;
    }
    state::write(|s| {
        if s.open.as_ref().is_some_and(|o| s.board.get(o).is_none_or(|w| w.deleted())) {
            s.open = None;
        }
        s.picked.clear();
    });
    state::changed();
    let text = match names.as_slice() {
        [one] if n == 1 => format!("Deleted {one}"),
        _ => format!("Deleted {n} picked"),
    };
    crate::toast::with_action(&text, crate::toast::Tone::Info, "Undo", state::undo);
}

/// Ctrl+Z undoes; Delete deletes what's picked; Escape lets go of it, then
/// closes the sidebar.
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
        let picked = state::read(|s| !s.picked.is_empty());
        match key {
            gtk::gdk::Key::z | gtk::gdk::Key::Z if ctrl => {
                state::undo();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Delete | gtk::gdk::Key::BackSpace if picked => {
                delete_picked_now();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Escape if picked => {
                state::write(|s| s.picked.clear());
                state::changed();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Escape if state::read(|s| s.open.is_some()) => {
                close();
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
        draw_row(s, &p.row);
        ui::clear(&p.machines);
        p.machines.append(&carousel::bar(s));
        ui::clear(&p.side);
        let open = state::read(|st| st.open.as_ref().and_then(|o| st.board.get(o)).filter(|w| !w.deleted()).cloned());
        match open {
            Some(w) => {
                p.side.append(&sidebar::sidebar(s, &w));
                p.side.set_visible(true);
            }
            None => {
                p.side.set_visible(false);
                if state::read(|st| st.open.is_some()) {
                    state::write(|st| st.open = None);
                }
                sidebar::closed();
            }
        }
        let (can_undo, picked, picked_ws, err, gone) = state::read(|st| {
            (
                st.board.can_undo(),
                st.picked.len(),
                state::picked_wadspaces(st).len(),
                st.save_error.clone(),
                st.board.deleted().len(),
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
        p.build_picked.set_sensitive(picked_ws > 0);
        ui::set_text(
            &p.build_picked,
            &if picked_ws > 0 { format!("Build selected ({picked_ws})") } else { "Build selected".into() },
        );
        p.delete_picked.set_sensitive(picked > 0);
        if picked > 0 {
            p.delete_picked.add_css_class("armed");
        } else {
            p.delete_picked.remove_css_class("armed");
        }
        ui::set_text(
            &p.delete_picked,
            &if picked > 0 { format!("Delete selected ({picked})") } else { "Delete selected".into() },
        );
        p.deleted.set_sensitive(gone > 0);
        p.not_saved.set_visible(err.is_some());
        if let Some(e) = err {
            p.not_saved.set_tooltip_text(Some(&format!("{e} Your changes are kept here. Click to try again.")));
        }
    });
}

fn draw_row(s: &AppState, row: &gtk::Box) {
    ui::clear(row);
    let shown = state::shown(s);
    let list: Vec<wad_board::b::Wadspace> = state::read(|st| st.board.row(&shown).into_iter().cloned().collect());
    if !state::read(|st| st.loaded) {
        let l = gtk::Label::new(Some("Reading your wadspaces…"));
        l.add_css_class("faint");
        l.set_margin_start(24);
        l.set_margin_top(24);
        row.append(&l);
        return;
    }
    // A machine with none: a blank one to fill in.
    if list.is_empty() {
        row.append(&buckets::blank(&shown));
        return;
    }
    for w in &list {
        row.append(&buckets::gap(&shown, &w.id));
        row.append(&buckets::column(s, w));
    }
    row.append(&buckets::open_board(&shown));
}

/// Drags made by hand (the snapshots).
pub mod sim {
    use super::*;
    use wad_board::b::Payload as Bp;

    fn column(id: &str) -> Option<gtk::Widget> {
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
        let root = gtk::Window::list_toplevels().into_iter().next()?;
        find(&root, id)
    }

    /// A wadspace picked up by its name and held over a machine's card.
    pub fn wadspace_over_machine(ws: &str, machine: &str) -> bool {
        let (Some(col), Some(card)) = (column(ws), carousel::card_of(machine)) else { return false };
        crate::drag::sim::begin(&col, carousel::wadspace_payload(ws), buckets::ghost("", ws));
        crate::drag::sim::over(&card, 0.0, 20.0);
        true
    }

    /// A wadspace's New template field brought into view.
    pub fn reveal_new_template(ws: &str) {
        let (Some(col), Some(field)) = (column(ws), column(&format!("newtpl:{ws}"))) else { return };
        if let Some(sw) = col.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>()
            && let Some(p) = field.compute_point(&col, &gtk::graphene::Point::new(0.0, 0.0))
        {
            let adj = sw.vadjustment();
            adj.set_value((f64::from(p.y()) - adj.page_size() / 2.0).clamp(0.0, adj.upper()));
        }
    }

    /// An app from the sidebar held over a wadspace's New template field.
    pub fn app_over_new_template(app: &str, ws: &str) -> bool {
        let (Some(col), Some(field)) = (column(ws), column(&format!("newtpl:{ws}"))) else { return false };
        let p = crate::drag::Payload::B(Bp::CatalogApp { app: app.into() });
        crate::drag::sim::begin(&col, p, buckets::ghost(&buckets::app_icon(app), &buckets::app_name(app)));
        crate::drag::sim::over(&field, 0.0, 10.0);
        true
    }

    /// An app from the sidebar held over a blank wadspace's template.
    pub fn app_over_blank(app: &str) -> bool {
        let Some(t) = column("blank-template") else { return false };
        let p = crate::drag::Payload::B(Bp::CatalogApp { app: app.into() });
        crate::drag::sim::begin(&t, p, buckets::ghost(&buckets::app_icon(app), &buckets::app_name(app)));
        crate::drag::sim::over(&t, 0.0, 20.0);
        true
    }

    /// Let go (dropped where it is).
    pub fn drop_it() {
        crate::drag::sim::end(true);
    }
}
