//! The Builder (the web app's builder/BoardPage.tsx and Board.tsx): every
//! wadspace as a stack of modules (template, Extensions, customization) in
//! one row, stepped through with arrows. Modules rename in place; Ctrl+click
//! picks them for Build or Delete along the bottom; a stack whose parts
//! don't add up says so, with fixes. What's on the board is the account's
//! documents (state.rs): changes are saved as they're made, and Undo takes
//! them back.

pub mod b;
pub mod drops;
pub mod editor;
pub mod faces;
pub mod parts;
pub mod preview;
pub mod state;

use std::cell::Cell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use wad_board::catalog::{self, Fix};
use wad_board::*;
use wad_gtk::Icon;

use self::faces::Face;
use self::state::Standing;
use crate::drag;
use crate::modal::Modal;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

const STACK_W: i32 = faces::MOD_W + 18;

thread_local! {
    /// The row of stacks and the Delete bucket (for the snapshots' drags).
    static WIDGETS: std::cell::RefCell<(glib::WeakRef<gtk::Box>, glib::WeakRef<gtk::Button>)> = Default::default();
    /// Recently deleted's panel (for the snapshots).
    static PANEL: glib::WeakRef<gtk::Box> = Default::default();
}

thread_local! {
    /// A stack asked for from elsewhere (and whether to copy it), handled
    /// once the board has it.
    static ASKED: std::cell::RefCell<Option<(String, bool)>> = const { std::cell::RefCell::new(None) };
}

/// Shows the Builder on a stack (the web app's ?stack=<id>), or (`copy`)
/// on a copy of it, made beside it.
pub fn show_stack(id: &str, copy: bool) {
    ASKED.with(|a| a.replace(Some((id.to_string(), copy))));
    crate::shell::go(crate::shell::Page::Builder);
    state::changed();
}

fn take_asked() {
    let Some((id, copy)) = ASKED.with(|a| a.borrow().clone()) else { return };
    // Not on the board (yet: the documents may still be loading).
    let Some(at) = state::read(|s| s.board.stacks.iter().position(|x| x.id == id)) else { return };
    ASKED.with(|a| a.replace(None));
    if !copy {
        state::set_created(id);
        return;
    }
    let (modules, name) = state::read(|s| (s.board.stacks[at].modules(), stack_name(&s.board.stacks[at])));
    let from = wad_board::board::From::Stack { stack_id: id };
    let made = state::edit(|b| b.new_stack(&modules, &from, at + 1, false, &[]));
    state::tell(Some(wad_board::board::Note { text: format!("Copied {name}"), undo: true }));
    state::set_created(made);
}

/// The page.
pub fn page() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("wb");
    root.append(&topbar());

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("stacks");
    row.set_valign(gtk::Align::Fill);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::External);
    scroll.set_vscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&row));
    let board = gtk::Overlay::new();
    board.add_css_class("board");
    board.set_vexpand(true);
    board.set_child(Some(&scroll));
    let (prev, next) = (nav_button("chevron-left", "Previous wadspace"), nav_button("chevron-right", "Next wadspace"));
    prev.set_halign(gtk::Align::Start);
    next.set_halign(gtk::Align::End);
    board.add_overlay(&prev);
    board.add_overlay(&next);
    let adj = scroll.hadjustment();
    let edges = {
        let (prev, next, adj) = (prev.clone(), next.clone(), adj.clone());
        move || {
            prev.set_visible(adj.value() > 2.0);
            next.set_visible(adj.value() + adj.page_size() < adj.upper() - 2.0);
        }
    };
    let e = edges.clone();
    adj.connect_value_changed(move |_| e());
    let e = edges.clone();
    adj.connect_changed(move |_| e());
    let spring = wad_gtk::Spring::new(&scroll, wad_gtk::Params::new(300.0, 40.0), 0.0, {
        let adj = adj.clone();
        move |v| adj.set_value(v)
    });
    let spring = Rc::new(spring);
    for (b, dir) in [(&prev, -1.0), (&next, 1.0)] {
        let (row, adj, spring) = (row.clone(), adj.clone(), spring.clone());
        b.connect_clicked(move |_| step(&row, &adj, &spring, dir));
    }
    crate::tour::target("board", &board);
    root.append(&board);
    let panel = drops::deleted_panel();
    PANEL.with(|p| p.set(Some(&panel)));
    root.append(&buckets(&panel));
    drag::target(&row, drops::GapsTarget::new(&row));
    WIDGETS.with(|w| w.borrow_mut().0 = row.downgrade());
    drag::autoscroll(&scroll);
    let page = gtk::Overlay::new();
    page.set_child(Some(&root));
    page.add_overlay(&panel);
    let p = panel.downgrade();
    state::on_change(move || {
        if let Some(p) = p.upgrade().filter(|p| p.is_visible()) {
            drops::fill_deleted(&p);
        }
    });

    // A stack asked for, once it's there.
    state::on_change(|| {
        if ASKED.with(|a| a.borrow().is_some()) {
            glib::idle_add_local_once(take_asked);
        }
    });

    // Drawn again when the board changes; the library read into it.
    let r = row.downgrade();
    let (adj2, spring2) = (adj.clone(), spring.clone());
    state::on_change(move || {
        if let Some(row) = r.upgrade() {
            if state::read(|s| s.renaming) {
                return;
            }
            render(&row);
            // A stack just made: brought into view.
            if let Some(id) = state::write_created_take() {
                let (adj, spring, row) = (adj2.clone(), spring2.clone(), row.clone());
                glib::idle_add_local_once(move || scroll_to(&row, &adj, &spring, &id));
            }
        }
    });
    let builds_seen: Rc<std::cell::RefCell<String>> = Rc::default();
    store().watch(&root, Topic::LIBRARY | Topic::ACTIVITY | Topic::WADD, move |s| {
        state::take_library(s);
        // Builds starting or ending (not their progress) change what's built.
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
    render(&row);
    page.upcast()
}

fn nav_button(icon: &'static str, tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("stacks-nav");
    b.set_child(Some(&Icon::new(icon, 20)));
    b.set_tooltip_text(Some(tip));
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b.set_valign(gtk::Align::Center);
    b.set_margin_start(6);
    b.set_margin_end(6);
    b
}

/// One wadspace along: to the next (or previous) stack's left edge.
fn step(row: &gtk::Box, adj: &gtk::Adjustment, spring: &wad_gtk::Spring, dir: f64) {
    let lefts: Vec<f64> =
        children(row).filter(|c| c.has_css_class("stack-col")).map(|c| left_of(&c, row) - 50.0).collect();
    let at = adj.value();
    let to = if dir > 0.0 {
        lefts.iter().copied().find(|x| *x > at + 2.0).unwrap_or(adj.upper())
    } else {
        lefts.iter().rev().copied().find(|x| *x < at - 2.0).unwrap_or(0.0)
    };
    spring.jump(at);
    spring.to(to.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
}

fn left_of(w: &gtk::Widget, row: &gtk::Box) -> f64 {
    w.compute_point(row, &gtk::graphene::Point::new(0.0, 0.0)).map(|p| p.x() as f64).unwrap_or(0.0)
}

fn scroll_to(row: &gtk::Box, adj: &gtk::Adjustment, spring: &wad_gtk::Spring, id: &str) {
    let target = children(row).find(|c| c.widget_name() == id);
    let to = target.map(|t| left_of(&t, row) - 50.0).unwrap_or(adj.upper());
    spring.jump(adj.value());
    spring.to(to.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
}

fn children(b: &impl IsA<gtk::Widget>) -> impl Iterator<Item = gtk::Widget> {
    std::iter::successors(b.as_ref().first_child(), |w| w.next_sibling())
}

/// Ctrl+Z undoes; Delete deletes what's picked; Escape lets go.
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
                state::edit(|b| b.undo());
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Delete | gtk::gdk::Key::BackSpace if state::read(|s| !s.board.selected.is_empty()) => {
                let n = state::edit(|b| b.delete_selected());
                state::tell(n);
                confirm_if_asked();
                glib::Propagation::Stop
            }
            gtk::gdk::Key::Escape if state::read(|s| !s.board.selected.is_empty()) => {
                state::edit(|b| b.selected.clear());
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    root.add_controller(k);
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

fn topbar() -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    bar.add_css_class("topbar");
    let t = gtk::Label::new(Some("Builder"));
    t.add_css_class("wb-title");
    bar.append(&t);
    let hints = gtk::FlowBox::new();
    hints.add_css_class("hints");
    hints.set_selection_mode(gtk::SelectionMode::None);
    hints.set_max_children_per_line(4);
    hints.set_column_spacing(14);
    hints.set_row_spacing(4);
    // Packed to the left at their own widths (the web app's wrapping row),
    // not spread over the bar.
    hints.set_hexpand(false);
    hints.set_halign(gtk::Align::Start);
    hints.set_valign(gtk::Align::Center);
    for (k, text) in [
        ("drag", "a module (and what's on top) to copy it"),
        ("Shift", "+drag one module"),
        ("right-click", "mid-drag to cancel"),
        ("click", "to edit"),
        ("Ctrl+click", "select, then Delete"),
    ] {
        let h = kbd(k, text);
        h.set_halign(gtk::Align::Start);
        hints.insert(&h, -1);
    }
    bar.append(&hints);
    let spring = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spring.set_hexpand(true);
    bar.append(&spring);
    let not_saved = ui::button("Not saved · Retry", Some("cloud-off"), Variant::Ghost, Size::Sm);
    not_saved.add_css_class("not-saved");
    not_saved.connect_clicked(|_| state::retry());
    bar.append(&not_saved);
    let undo = ui::button("Undo", Some("undo-2"), Variant::Ghost, Size::Sm);
    undo.add_css_class("wb-ghost");
    undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
    undo.connect_clicked(|_| {
        state::edit(|b| b.undo());
    });
    bar.append(&undo);
    let create = ui::button("Create", Some("plus"), Variant::Primary, Size::Sm);
    create.set_tooltip_text(Some("A new stack on the right, with a blank template"));
    create.connect_clicked(|_| {
        let id = state::edit(|b| b.create_stack());
        state::set_created(id);
        crate::tour::signal(crate::tour::Event::StackCreated);
    });
    crate::tour::target("create", &create);
    bar.append(&create);
    let (u, ns) = (undo.clone(), not_saved.clone());
    let show = move || {
        let (can, err) = state::read(|s| (s.board.can_undo(), s.save_error.clone()));
        u.set_sensitive(can);
        ns.set_visible(err.is_some());
        if let Some(e) = err {
            ns.set_tooltip_text(Some(&format!("{e} Your changes are kept here. Click to try again.")));
        }
    };
    show();
    state::on_change(show);
    bar.upcast()
}

/// The stacks, drawn again.
fn render(row: &gtk::Box) {
    ui::clear(row);
    let stacks = state::read(|s| s.board.stacks.clone());
    store().read_now(|s| {
        if stacks.is_empty() {
            row.append(&fixed_width(first_wadspace(s), "new"));
        }
        for st in &stacks {
            let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            gap.add_css_class("gap");
            gap.set_size_request(22, -1);
            row.append(&gap);
            row.append(&fixed_width(column(s, st), &st.id));
        }
    });
    // Room after the last stack (new stacks are dropped there).
    let open = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    open.add_css_class("open-space");
    open.set_size_request(240, -1);
    row.append(&open);
}

/// A stack's column, exactly its width whatever's in it.
fn fixed_width(col: gtk::Widget, id: &str) -> gtk::Widget {
    let w = wad_gtk::Width::new(&col, STACK_W);
    w.add_css_class("stack-col");
    w.set_widget_name(id);
    w.set_hexpand(false);
    w.set_valign(gtk::Align::End);
    w.upcast()
}

/// An empty board: a stack to start the first wadspace from.
fn first_wadspace(s: &AppState) -> gtk::Widget {
    let empty = Stack::new("new");
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.add_css_class("stack");
    col.add_css_class("first-ws");
    col.set_valign(gtk::Align::End);
    let intro = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let t = gtk::Label::new(Some("Your first wadspace"));
    t.add_css_class("panel-title-sm");
    t.set_xalign(0.0);
    intro.append(&t);
    let p = ui::wrapping(
        "Set up any layer to start it: a template of apps, the extensions they come with, or its name, background and projects.",
    );
    p.add_css_class("rail-note");
    intro.append(&p);
    col.append(&intro);
    col.append(&blank_tile(faces::blank_customization(&empty), Layer::Customization, editor::NEW));
    col.append(&blank_tile(faces::blank_extensions(&empty), Layer::Extensions, editor::NEW));
    col.append(&add_button(Layer::Template, editor::NEW));
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.add_css_class("stack-foot");
    let n = gtk::Label::new(Some("No wadspaces yet"));
    n.add_css_class("stack-name");
    n.add_css_class("faint");
    foot.append(&n);
    col.append(&foot);
    let _ = s;
    col.upcast()
}

fn add_button(layer: Layer, stack_id: &str) -> gtk::Button {
    let text = match layer {
        Layer::Template => "Add a template",
        Layer::Extensions => "Add extensions",
        Layer::Customization => "Add a customization",
    };
    let b = ui::button(text, Some("plus"), Variant::Ghost, Size::Sm);
    b.add_css_class("add-btn");
    b.add_css_class(&format!(
        "add-{}",
        match layer {
            Layer::Template => "template",
            Layer::Extensions => "extensions",
            Layer::Customization => "customization",
        }
    ));
    let id = stack_id.to_string();
    b.connect_clicked(move |b| editor::add(layer, &id, Some(b.clone().upcast()), None));
    b
}

/// A click (not Ctrl+click: that selects) runs `f` with what was clicked.
fn on_click(w: &impl IsA<gtk::Widget>, f: impl Fn(gtk::Widget) + 'static) {
    let click = gtk::GestureClick::new();
    click.connect_released(move |g, _, _, _| {
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(w) = g.widget() {
            f(w);
        }
    });
    w.add_controller(click);
}

/// A layer a stack hasn't got: a click sets one up (a blank window or a
/// prompt opens the editor on its part).
fn blank_tile(face: Face, layer: Layer, stack_id: &str) -> gtk::Widget {
    let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
    t.add_css_class("mod-tile");
    t.add_css_class("blank");
    t.append(&face.root);
    let tip = match layer {
        Layer::Extensions => "Set up this wadspace's Extensions",
        _ => "Set up this wadspace's Customization",
    };
    t.set_tooltip_text(Some(tip));
    for (focus, w) in face.blanks.iter().map(|(k, w)| (*k, w)).chain(face.prompts.iter().map(|(k, w)| (*k, w))) {
        let id = stack_id.to_string();
        let tile = t.downgrade();
        on_click(w, move |_| editor::add(layer, &id, tile.upgrade().map(|t| t.upcast()), Some(focus)));
    }
    let id = stack_id.to_string();
    on_click(&t, move |t| editor::add(layer, &id, Some(t), None));
    t.upcast()
}

/// A module's tile, by its id (to shrink the editor back into).
pub(super) fn tile_widget(module_id: &str) -> Option<gtk::Widget> {
    let row = WIDGETS.with(|w| w.borrow().0.upgrade())?;
    let name = format!("mod:{module_id}");
    let mut all = vec![];
    sim::descendants(row.upcast_ref(), &mut all);
    all.into_iter().find(|w| w.widget_name() == name)
}

/// What the stack's chip says: (class, words).
fn chip_of(s: &AppState, stack: &Stack) -> (&'static str, String) {
    let (unbuilt, updates) = state::counts(stack, s);
    if !catalog::problems_of(stack).is_empty() {
        ("status-broken", "Needs fixing".into())
    } else if status_of(stack) == "no-template" {
        ("status-no-template", "Needs a template".into())
    } else if status_of(stack) == "no-customization" {
        ("status-no-customization", "Needs a customization".into())
    } else if state::stack_building(stack, s) {
        ("status-building", "Building…".into())
    } else if unbuilt > 0 {
        ("status-unbuilt", format!("Not built · {unbuilt}"))
    } else if stack.look().is_none_or(|l| l.name.trim().is_empty()) {
        ("status-no-customization", "Needs a name".into())
    } else if updates > 0 {
        ("status-update", "Update available".into())
    } else {
        ("status-ready", "Ready".into())
    }
}

fn column(s: &AppState, stack: &Stack) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.add_css_class("stack");
    col.set_valign(gtk::Align::End);
    let problems = catalog::problems_of(stack);
    if !problems.is_empty() {
        col.add_css_class("broken");
        let b = gtk::Button::new();
        b.add_css_class("broken-banner");
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        r.append(&Icon::new("triangle-alert", 14));
        let l = gtk::Label::new(Some("Incorrect configuration"));
        l.set_hexpand(true);
        l.set_xalign(0.0);
        r.append(&l);
        let f = gtk::Label::new(Some("Fix"));
        f.add_css_class("broken-fix");
        r.append(&f);
        b.set_child(Some(&r));
        b.set_tooltip_text(Some("See what's wrong and fix it"));
        let id = stack.id.clone();
        b.connect_clicked(move |_| fix_dialog(&id));
        col.append(&b);
    }
    if state::read(|st| st.created.as_deref() == Some(&stack.id)) {
        col.add_css_class("fresh");
    }
    let mods = gtk::Box::new(gtk::Orientation::Vertical, 8);
    mods.add_css_class("stack-modules");
    match &stack.customization {
        Some(c) => mods.append(&tile(s, stack, &Module::Customization(c.clone()), false)),
        None => mods.append(&blank_tile(faces::blank_customization(stack), Layer::Customization, &stack.id)),
    }
    match &stack.extensions {
        Some(e) => mods.append(&tile(s, stack, &Module::Extensions(e.clone()), false)),
        None => mods.append(&blank_tile(faces::blank_extensions(stack), Layer::Extensions, &stack.id)),
    }
    let n = stack.templates.len();
    for (i, t) in stack.templates.iter().enumerate().rev() {
        mods.append(&tile(s, stack, &Module::Template(t.clone()), i + 1 < n));
    }
    if stack.templates.is_empty() {
        mods.append(&add_button(Layer::Template, &stack.id));
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_vscrollbar_policy(gtk::PolicyType::External);
    scroll.set_propagate_natural_height(true);
    scroll.set_child(Some(&mods));
    drag::autoscroll(&scroll);
    col.append(&scroll);
    drag::target(&col, drops::StackTarget(stack.id.clone()));

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.add_css_class("stack-foot");
    let name = gtk::Label::new(Some(&stack_name(stack)));
    name.add_css_class("stack-name");
    name.set_hexpand(true);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    foot.append(&name);
    let (class, text) = chip_of(s, stack);
    if class == "status-update" {
        let b = gtk::Button::new();
        b.add_css_class("status");
        b.add_css_class(class);
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        r.append(&Icon::new("refresh-cw", 11));
        r.append(&gtk::Label::new(Some(&text)));
        b.set_child(Some(&r));
        b.set_tooltip_text(Some(
            "A newer base (or a template under it) since its images were built. It runs as it is; click to build again on what's there now.",
        ));
        let id = stack.id.clone();
        b.connect_clicked(move |_| state::build_stack(&id));
        foot.append(&b);
    } else {
        let l = gtk::Label::new(Some(&text));
        l.add_css_class("status");
        l.add_css_class(class);
        if class == "status-ready" {
            l.set_tooltip_text(Some("Built and named: ready to deploy"));
        }
        foot.append(&l);
    }
    let preview = ui::icon_button("eye", "Preview: see the finished wadspace (nothing is launched)", 14);
    preview.add_css_class("btn-preview");
    let id = stack.id.clone();
    preview.connect_clicked(move |_| preview::open(&id));
    foot.append(&preview);
    col.append(&foot);
    col.upcast()
}

fn tile(s: &AppState, stack: &Stack, m: &Module, compact: bool) -> gtk::Widget {
    let building = state::stack_building(stack, s);
    let built = match state::built(m, s) {
        Standing::Unbuilt if building => Standing::Building,
        b => b,
    };
    let face = faces::module(stack, m, compact, built);
    let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
    t.add_css_class("mod-tile");
    t.set_widget_name(&format!("mod:{}", m.id()));
    let selected = state::read(|st| st.board.selected.iter().any(|x| x == m.id()));
    if selected {
        t.add_css_class("selected");
    }
    t.set_tooltip_text(Some("Click to edit · Ctrl+click to select · drag to copy onto another stack"));
    let o = gtk::Overlay::new();
    o.set_child(Some(&face.root));
    if selected {
        let c = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        c.add_css_class("picked-check");
        c.set_halign(gtk::Align::End);
        c.set_valign(gtk::Align::Start);
        c.set_margin_top(6);
        c.set_margin_end(6);
        c.append(&Icon::new("check", 10));
        o.add_overlay(&c);
    }
    t.append(&o);
    // The name renames in place.
    ui::clear(&face.name_slot);
    face.name_slot.append(&name_field(stack, m));
    // A drag takes it and what's on top (Shift: just it).
    let (st, id) = (stack.clone(), m.id().to_string());
    drag::draggable_from(&t, move |shift| {
        let modules = wad_board::board::grab(&st, &id, shift);
        // The pile starts where its top module is (it's drawn top first).
        let top = modules.last().and_then(|m| tile_widget(m.id()));
        let ghost = faces::ghost(&faces::pile(&st, &modules));
        (!modules.is_empty()).then(|| {
            (
                drag::Payload::Modules { modules, from: wad_board::board::From::Stack { stack_id: st.id.clone() } },
                ghost,
                top,
            )
        })
    });
    drag::target(&t, drops::ModuleTarget { stack_id: stack.id.clone(), module: m.clone() });
    // Its submodules drag out of it.
    let subs: Vec<AnySub> = match m {
        Module::Extensions(e) => e.subs.iter().cloned().map(AnySub::App).collect(),
        Module::Customization(c) => c.subs.iter().cloned().map(AnySub::Custom).collect(),
        Module::Template(_) => vec![],
    };
    // A submodule, a blank window or a prompt opens the editor on its part.
    let parts = face
        .subs
        .iter()
        .filter_map(|(id, w)| subs.iter().find(|x| x.id() == id).map(|x| (x.key(), w)))
        .chain(face.blanks.iter().map(|(k, w)| (*k, w)))
        .chain(face.prompts.iter().map(|(k, w)| (*k, w)));
    for (focus, w) in parts {
        let (stack_id, module_id, tile) = (stack.id.clone(), m.id().to_string(), t.downgrade());
        on_click(w, move |_| editor::edit(&stack_id, &module_id, Some(focus), tile.upgrade().map(|t| t.upcast())));
    }
    for (sub_id, w) in &face.subs {
        let Some(sub) = subs.iter().find(|x| x.id() == sub_id).cloned() else { continue };
        let (stack_id, layer, st) = (stack.id.clone(), m.layer(), stack.clone());
        drag::draggable(w, move |_| {
            let ghost: gtk::Widget = match &sub {
                AnySub::App(a) => faces::ghost(&faces::app_window(a, None)),
                AnySub::Custom(c) => faces::ghost(&faces::chip(&AnySub::Custom(c.clone()), Some(&st))),
            };
            Some((
                drag::Payload::Sub {
                    sub: sub.clone(),
                    from: wad_board::board::SubFrom::Module { stack_id: stack_id.clone(), layer },
                },
                ghost,
            ))
        });
    }
    // A click opens it; Ctrl+click picks it out (for Build and Delete).
    let click = gtk::GestureClick::new();
    let (id, stack_id) = (m.id().to_string(), stack.id.clone());
    click.connect_released(move |g, _, _, _| {
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            state::edit(|b| b.toggle_select(&id));
        } else {
            editor::edit(&stack_id, &id, None, g.widget());
        }
    });
    t.add_controller(click);
    t.upcast()
}

/// A module's name: click it to rename it in place (Enter or clicking away
/// saves, Escape lets go). Names clash per kind, said as it's typed.
fn name_field(stack: &Stack, m: &Module) -> gtk::Widget {
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.set_hexpand(true);
    let name = module_name(m);
    let l = gtk::Label::new(Some(&name));
    l.add_css_class("mod-name");
    l.add_css_class("renamable");
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_tooltip_text(Some("Click to rename"));
    let click = gtk::GestureClick::new();
    let (slot2, l2, stack_id, module_id, m2) =
        (slot.clone(), l.clone(), stack.id.clone(), m.id().to_string(), m.clone());
    click.connect_released(move |g, _, _, _| {
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        renaming(&slot2, &l2, &stack_id, &module_id, &m2);
    });
    l.add_controller(click);
    slot.append(&l);
    slot.upcast()
}

fn renaming(slot: &gtk::Box, label: &gtk::Label, stack_id: &str, module_id: &str, m: &Module) {
    let name = module_name(m);
    let e = gtk::Entry::new();
    e.add_css_class("mod-name-input");
    e.set_hexpand(true);
    e.set_placeholder_text(Some("Name"));
    e.set_text(if matches!(m, Module::Customization(_)) && name == "Untitled" { "" } else { &name });
    label.set_visible(false);
    slot.append(&e);
    e.grab_focus();
    state::set_renaming(true);
    let done = Rc::new(Cell::new(false));
    let finish = {
        let (e, label, done, stack_id, module_id, name) =
            (e.clone(), label.clone(), done.clone(), stack_id.to_string(), module_id.to_string(), name.clone());
        Rc::new(move |save: bool| {
            if done.replace(true) {
                return;
            }
            let v = e.text().trim().to_string();
            state::set_renaming(false);
            if save && !v.is_empty() && v != name {
                match state::edit(|b| b.rename(&stack_id, &module_id, &v)) {
                    Ok(n) => state::tell(n),
                    Err(why) => crate::toast::show(&why, None, crate::toast::Tone::Error),
                }
            } else {
                if let Some(p) = e.parent().and_downcast::<gtk::Box>() {
                    p.remove(&e);
                }
                label.set_visible(true);
                state::changed();
            }
        })
    };
    // Said as it's typed: a clash.
    let (stack_id2, m2, name2) = (stack_id.to_string(), m.clone(), name.clone());
    e.connect_changed(move |e| {
        let v = e.text().to_string();
        let clash = if v.trim() != name2 {
            state::read(|s| wad_board::board::name_clash(&s.board.stacks, &stack_id2, &m2, &v))
        } else {
            None
        };
        if clash.is_some() {
            e.add_css_class("clash");
        } else {
            e.remove_css_class("clash");
        }
        e.set_tooltip_text(clash.as_deref());
    });
    let f = finish.clone();
    e.connect_activate(move |_| f(true));
    let keys = gtk::EventControllerKey::new();
    let f = finish.clone();
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape {
            f(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    e.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    let f = finish.clone();
    let e2 = e.downgrade();
    focus.connect_leave(move |_| {
        // Clicking away with a name that's taken lets it go instead.
        let clash = e2.upgrade().is_some_and(|e| e.has_css_class("clash"));
        f(!clash);
    });
    e.add_controller(focus);
}

/// What's wrong with a stack's configuration, and a fix for each.
fn fix_dialog(stack_id: &str) {
    let Some(stack) = state::read(|s| s.board.stack(stack_id).cloned()) else { return };
    let m = Modal::open(
        &format!("{} isn't configured right", stack_name(&stack)),
        Some("Fix each problem: give the template the app, or remove what needs it."),
        520,
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    fill_fixes(&list, &m, stack_id);
    m.body().append(&list);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(16);
    let later = ui::button("Later", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    later.connect_clicked(move |_| mm.close());
    row.append(&later);
    m.body().append(&row);
}

fn fill_fixes(list: &gtk::Box, m: &Modal, stack_id: &str) {
    ui::clear(list);
    let problems = state::read(|s| s.board.stack(stack_id).map(catalog::problems_of)).unwrap_or_default();
    if problems.is_empty() {
        m.close();
        return;
    }
    for p in problems {
        let item = gtk::Box::new(gtk::Orientation::Vertical, 8);
        item.add_css_class("fix-item");
        let t = ui::wrapping(&p.text);
        t.add_css_class("small-14");
        item.append(&t);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for f in p.fixes {
            let ghost = !matches!(f, Fix::AddApp { .. });
            let b = ui::button(
                f.label(),
                Some("wrench"),
                if ghost { Variant::Ghost } else { Variant::Secondary },
                Size::Sm,
            );
            if ghost {
                b.add_css_class("danger-text");
            }
            let (list, m, id) = (list.clone(), m.clone(), stack_id.to_string());
            b.connect_clicked(move |_| {
                let n = state::edit(|bd| bd.fix(&id, &f));
                state::tell(n);
                fill_fixes(&list, &m, &id);
            });
            actions.append(&b);
        }
        item.append(&actions);
        list.append(&item);
    }
}

/// "Are you sure?" before a whole stack is deleted.
pub(super) fn confirm_if_asked() {
    let Some(ids) = state::read(|s| s.board.confirming.clone()) else { return };
    let stacks = state::read(|s| s.board.stacks.clone());
    let whole: Vec<&Stack> = stacks
        .iter()
        .filter(|st| !st.modules().is_empty() && st.modules().iter().all(|m| ids.iter().any(|i| i == m.id())))
        .collect();
    let others = ids.len() - whole.iter().map(|st| st.modules().len()).sum::<usize>();
    let names: Vec<String> = whole.iter().map(|s| stack_name(s)).collect();
    let one = whole.len() == 1 && others == 0;
    let title = if one { format!("Delete {}?", names[0]) } else { format!("Delete {} modules?", ids.len()) };
    let mut text = if whole.len() == 1 {
        format!("The whole of {} goes", names.join(", "))
    } else {
        format!("Whole stacks go: {}", names.join(", "))
    };
    if others > 0 {
        text.push_str(&format!(", plus {others} more module{}", if others == 1 { "" } else { "s" }));
    }
    text.push_str(". They wait in Recently deleted, and Undo brings them back.");
    let m = Modal::open(&title, Some(&text), 460);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| {
        state::edit(|b| b.confirming = None);
        mm.close();
    });
    let del = ui::button(if one { "Delete stack" } else { "Delete them" }, Some("trash"), Variant::Danger, Size::Md);
    let mm = m.clone();
    del.connect_clicked(move |_| {
        let n = state::edit(|b| b.delete_ids(&ids));
        state::tell(n);
        mm.close();
    });
    row.append(&cancel);
    row.append(&del);
    m.body().append(&row);
}

// ─────────────────────────────── the buckets ───────────────────────────────

fn bucket(class: &str, icon: &'static str) -> (gtk::Button, gtk::Label, gtk::Label, gtk::Label) {
    let b = gtk::Button::new();
    b.add_css_class("bucket");
    b.add_css_class(class);
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    r.set_halign(gtk::Align::Center);
    let i = Icon::new(icon, 18);
    i.add_css_class("bucket-icon");
    i.set_valign(gtk::Align::Center);
    r.append(&i);
    let name = gtk::Label::new(None);
    name.add_css_class("bucket-name");
    r.append(&name);
    let count = gtk::Label::new(None);
    count.add_css_class("bucket-count");
    count.set_valign(gtk::Align::Center);
    r.append(&count);
    let small = gtk::Label::new(None);
    small.add_css_class("bucket-small");
    small.set_wrap(true);
    small.set_lines(2);
    small.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // On one line when it fits (two at most).
    small.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    small.set_max_width_chars(40);
    small.set_xalign(0.0);
    small.set_valign(gtk::Align::Center);
    r.append(&small);
    b.set_child(Some(&r));
    (b, name, count, small)
}

/// Build and Delete, side by side along the bottom.
fn buckets(panel: &gtk::Box) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outer.add_css_class("bottom-buckets");
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bar.set_halign(gtk::Align::Center);
    bar.set_hexpand(true);
    outer.append(&bar);
    let (build, b_name, b_count, b_small) = bucket("bucket-build", "hammer");
    build.connect_clicked(|_| {
        let sel = state::read(|s| (!s.board.selected.is_empty()).then(|| s.board.selected_stacks()));
        if sel.is_some() {
            state::edit(|b| b.selected.clear());
        }
        state::build_all(sel);
    });
    drag::target(&build, drops::BuildTarget);
    crate::tour::target("build", &build);
    bar.append(&build);
    let (del, d_name, d_count, d_small) = bucket("bucket-delete", "trash");
    let p = panel.clone();
    del.connect_clicked(move |_| {
        if state::read(|s| !s.board.selected.is_empty()) {
            let n = state::edit(|b| b.delete_selected());
            state::tell(n);
            confirm_if_asked();
        } else if p.is_visible() {
            p.set_visible(false);
        } else {
            drops::fill_deleted(&p);
            p.set_visible(true);
        }
    });
    drag::target(&del, drops::DeleteTarget);
    WIDGETS.with(|w| w.borrow_mut().1 = del.downgrade());
    bar.append(&del);
    let clear = ui::button("Clear selection", None, Variant::Ghost, Size::Sm);
    clear.add_css_class("wb-ghost");
    clear.set_valign(gtk::Align::Center);
    clear.connect_clicked(|_| {
        state::edit(|b| b.selected.clear());
    });
    bar.append(&clear);
    let show = {
        let (build, del, clear) = (build.clone(), del.clone(), clear.clone());
        move || {
            let (picked, deleted, stacks) =
                state::read(|s| (s.board.selected.clone(), s.board.deleted.len(), s.board.stacks.clone()));
            let (count, skipped, building) = store().read_now(|s| {
                let building = state::images_building(&stacks, s);
                let waiting: Vec<&Stack> = stacks
                    .iter()
                    .filter(|st| {
                        let (a, b) = state::counts(st, s);
                        a + b > 0 && !state::stack_building(st, s)
                    })
                    .collect();
                let buildable: Vec<&&Stack> = waiting
                    .iter()
                    .filter(|st| {
                        !st.apps().is_empty()
                            && catalog::problems_of(st).is_empty()
                            && st.look().is_some_and(|l| !l.name.trim().is_empty())
                    })
                    .collect();
                let count = if picked.is_empty() {
                    buildable.len()
                } else {
                    buildable
                        .iter()
                        .filter(|st| st.modules().iter().any(|m| picked.iter().any(|p| p == m.id())))
                        .count()
                };
                (count, waiting.len() - buildable.len(), building)
            });
            let sel = !picked.is_empty();
            for (b, on) in [(&build, sel), (&del, sel)] {
                if on {
                    b.add_css_class("armed");
                } else {
                    b.remove_css_class("armed");
                }
            }
            b_name.set_text(if sel { "Build selected" } else { "Build all" });
            b_count.set_text(&count.to_string());
            b_count.set_visible(count > 0);
            b_small.set_text(&if !building.is_empty() {
                format!("Building {}…", building.join(", "))
            } else if count > 0 {
                if skipped > 0 && !sel {
                    format!("{skipped} more need{} fixing or a template", if skipped == 1 { "s" } else { "" })
                } else {
                    "Or drop a wadspace to build just it".into()
                }
            } else if skipped > 0 {
                format!("{skipped} need{} fixing or a template first", if skipped == 1 { "s" } else { "" })
            } else {
                "Everything's built and up to date".into()
            });
            build.set_tooltip_text(Some(if sel {
                "Build the stacks of what's selected"
            } else {
                "Click to build everything not built or with an update, or drop a wadspace here to build just it"
            }));
            d_name.set_text(&if sel { format!("Delete {} selected", picked.len()) } else { "Delete".into() });
            d_small.set_text(&if sel {
                "Click, or press Delete · Esc to clear".into()
            } else if deleted > 0 {
                format!("{deleted} recently deleted · click to see")
            } else {
                "Drop a module here, or Ctrl+click modules to select them".into()
            });
            d_count.set_text(&deleted.to_string());
            d_count.set_visible(deleted > 0 && !sel);
            d_count.add_css_class("deleted-count");
            clear.set_visible(sel);
        }
    };
    show();
    state::on_change(show);
    outer.upcast()
}

/// Drags made by hand, for the snapshots: a stack's customization picked
/// up, held over another stack, then over Delete, then dropped after the
/// last stack.
pub mod sim {
    use super::*;

    pub(super) fn descendants(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        let mut c = w.first_child();
        while let Some(x) = c {
            out.push(x.clone());
            descendants(&x, out);
            c = x.next_sibling();
        }
    }

    fn column(id: &str) -> Option<gtk::Widget> {
        let row = WIDGETS.with(|w| w.borrow().0.upgrade())?;
        children(&row).find(|c| c.widget_name() == id)
    }

    pub fn pick_up(stack_id: &str) -> bool {
        let Some(col) = column(stack_id) else { return false };
        let mut all = vec![];
        descendants(&col, &mut all);
        let Some(tile) = all.into_iter().find(|w| w.has_css_class("mod-tile") && !w.has_css_class("blank")) else {
            return false;
        };
        let Some(stack) = state::read(|s| s.board.stack(stack_id).cloned()) else { return false };
        let Some(c) = stack.customization.clone() else { return false };
        let modules = vec![Module::Customization(c)];
        let ghost = faces::ghost(&faces::pile(&stack, &modules));
        drag::sim::begin(
            &tile,
            drag::Payload::Modules { modules, from: wad_board::board::From::Stack { stack_id: stack_id.into() } },
            ghost,
        );
        true
    }

    pub fn over_stack(stack_id: &str) {
        if let Some(col) = column(stack_id) {
            drag::sim::over(&col, 0.0, 320.0);
        }
    }

    pub fn over_delete() {
        if let Some(d) = WIDGETS.with(|w| w.borrow().1.upgrade()) {
            drag::sim::over(&d, 0.0, 20.0);
        }
    }

    /// A stack's customization deleted (into Recently deleted).
    pub fn delete_customization(stack_id: &str) {
        let Some(c) = state::read(|s| s.board.stack(stack_id).and_then(|st| st.customization.clone())) else { return };
        let from = wad_board::board::From::Stack { stack_id: stack_id.into() };
        state::edit(|b| b.delete_modules(&[Module::Customization(c)], &from));
    }

    /// Recently deleted, open.
    pub fn show_deleted() {
        if let Some(p) = PANEL.with(|p| p.upgrade()) {
            drops::fill_deleted(&p);
            p.set_visible(true);
        }
    }

    pub fn drop_after_last() {
        if let Some(row) = WIDGETS.with(|w| w.borrow().0.upgrade())
            && let Some(open) =
                std::iter::successors(row.last_child(), |w| w.prev_sibling()).find(|w| w.has_css_class("open-space"))
        {
            drag::sim::over(&open, 0.0, 300.0);
        }
        drag::sim::end(true);
    }
}
