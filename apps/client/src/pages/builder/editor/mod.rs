//! The open editor (the web app's builder/editors/Editor.tsx): it grows out
//! of the module clicked (or the place a new one goes) to fill the window,
//! and shrinks back into the module on Save or Cancel. Its body is the
//! layer's editor (template.rs, extensions.rs, customization.rs), working
//! on a draft: nothing reaches the board until Save. Names are unique per
//! kind, said in the head; a shared template whose apps changed is offered
//! a free name.

pub mod customization;
pub mod extensions;
pub mod template;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use wad_board::*;
use wad_gtk::Icon;

use super::{faces, state};
use crate::ui::{self, Size, Variant};

/// The stack id a module for the board's first wadspace edits under.
pub const NEW: &str = "new";

/// How long growing (and shrinking) takes.
const GROW_US: f64 = 320_000.0;
/// Open, it fills the window but for this much all round.
const INSET: f64 = 16.0;

/// What to edit.
pub struct Editing {
    pub stack_id: String,
    pub module_id: String,
    /// A module not on the board yet: Save adds it (Cancel, or saving it
    /// with nothing set up, drops it).
    pub fresh: Option<Module>,
    /// The section to open on ("secrets", "vscode", …).
    pub focus: Option<String>,
    /// Where it grows out of: the widget clicked.
    pub from: Option<gtk::Widget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Grow,
    Open,
    Shrink,
}

type Redraw = Box<dyn Fn(&Rc<Editor>)>;

/// The editor open now.
pub struct Editor {
    pub stack_id: String,
    pub module_id: String,
    pub fresh: bool,
    pub focus: Option<String>,
    /// The stack as it was when it opened (the board's is read while it's there).
    stack0: Stack,
    draft: RefCell<Module>,
    redraws: RefCell<Vec<Redraw>>,
    /// What the body's parts keep (their state).
    kept: RefCell<Vec<Rc<dyn std::any::Any>>>,
    root: gtk::Overlay,
    scrim: gtk::Box,
    card: gtk::Box,
    /// The head's buttons and notes: hidden while it grows or shrinks.
    head_rest: Vec<gtk::Widget>,
    kind: gtk::Label,
    title: gtk::Label,
    clash: gtk::Label,
    fork: gtk::Box,
    fork_text: gtk::Label,
    fork_button: gtk::Button,
    save: gtk::Button,
    body: gtk::Box,
    phase: Cell<Phase>,
    /// Where it is now (the layer's coordinates): x, y, width, height.
    at: Cell<[f64; 4]>,
    /// Where it grew out of.
    from: Cell<[f64; 4]>,
}

thread_local! {
    static LAYER: RefCell<Option<gtk::Overlay>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Rc<Editor>>> = const { RefCell::new(None) };
    /// A menu open in the editor (Esc closes it, not the editor).
    static MENU: RefCell<glib::WeakRef<gtk::Popover>> = RefCell::default();
}

/// Where the editor opens: over the app, under dialogs, toasts and drags.
pub fn layer() -> gtk::Overlay {
    let o = gtk::Overlay::new();
    o.set_can_target(false);
    o.set_visible(false);
    LAYER.with(|l| l.replace(Some(o.clone())));
    o
}

fn the_layer() -> Option<gtk::Overlay> {
    LAYER.with(|l| l.borrow().clone())
}

/// The open editor, cancelled (its draft goes).
pub fn cancel() {
    if let Some(ed) = current() {
        ed.close(false);
    }
}

/// Something shown over the app in the editor's place (the Preview).
pub fn layer_show(w: &impl IsA<gtk::Widget>) {
    let Some(layer) = the_layer() else { return };
    layer.add_overlay(w);
    layer.set_visible(true);
    layer.set_can_target(true);
}

/// It goes (the layer hides when nothing's left on it).
pub fn layer_hide(w: &impl IsA<gtk::Widget>) {
    let Some(layer) = the_layer() else { return };
    layer.remove_overlay(w);
    if layer.first_child().is_none() {
        layer.set_visible(false);
        layer.set_can_target(false);
    }
}

/// The editor open now.
pub fn current() -> Option<Rc<Editor>> {
    OPEN.with(|o| o.borrow().clone())
}

/// A menu opened in the editor: Esc closes it first.
pub fn menu_opened(p: &gtk::Popover) {
    MENU.with(|m| m.borrow().set(Some(p)));
}

/// Esc: a menu closes, else the editor (as Cancel), unless something's
/// being dragged or typed somewhere that takes Esc itself.
pub fn escape() -> bool {
    if let Some(p) = MENU.with(|m| m.borrow().upgrade()).filter(|p| p.is_visible()) {
        p.popdown();
        return true;
    }
    if super::preview::close() {
        return true;
    }
    let Some(ed) = current() else { return false };
    if crate::drag::active() || ed.phase.get() != Phase::Open {
        return ed.phase.get() != Phase::Open;
    }
    let typing_here = ed
        .root
        .root()
        .and_then(|w| w.focus())
        .is_some_and(|f| f.ancestor(gtk::Entry::static_type()).is_some_and(|e| e.has_css_class("esc-local")));
    if typing_here {
        return false;
    }
    ed.close(false);
    true
}

/// An empty module to set up in its editor.
pub fn blank(layer: Layer) -> Module {
    let id = uid("new");
    match layer {
        Layer::Template => Module::Template(TemplateModule::new(id, "New template", vec![])),
        Layer::Extensions => Module::Extensions(ExtensionsModule {
            layer: ExtensionsTag,
            id,
            doc: None,
            name: "New extensions".into(),
            subs: vec![],
            autostart: None,
        }),
        Layer::Customization => Module::Customization(CustomizationModule {
            layer: CustomizationTag,
            id,
            subs: vec![CustomSub::Look(blank_look())],
        }),
    }
}

/// A look with nothing set.
pub fn blank_look() -> LookSub {
    LookSub {
        id: uid("look"),
        origin: String::new(),
        name: String::new(),
        description: String::new(),
        wallpaper: blank_wallpaper(),
        title_bars: false,
    }
}

/// `base`, or "<base> - n" for the first n that's free (`taken`: lower case).
fn free(base: &str, taken: &[String]) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{base} - {n}");
        n += 1;
    }
    name
}

/// Opens the editor on a new module for a stack without that layer (or a
/// template more), named so it's free.
pub fn add(layer: Layer, stack_id: &str, from: Option<gtk::Widget>, focus: Option<&str>) {
    let mut fresh = blank(layer);
    let stacks = state::read(|s| s.board.stacks.clone());
    let stack = stacks.iter().find(|s| s.id == stack_id);
    match &mut fresh {
        Module::Extensions(e) => {
            let taken: Vec<String> = stacks
                .iter()
                .map(|s| s.extensions.as_ref().map(|e| e.name.to_lowercase()).unwrap_or_default())
                .collect();
            let base = format!("{} Extensions", stack.map(stack_name).unwrap_or_else(|| "New".into()));
            e.name = free(&base, &taken);
        }
        Module::Template(t) => {
            let taken: Vec<String> =
                stacks.iter().flat_map(|s| s.templates.iter().map(|t| t.name.to_lowercase())).collect();
            t.name = free("New template", &taken);
        }
        Module::Customization(_) => {}
    }
    open(Editing {
        stack_id: stack_id.to_string(),
        module_id: fresh.id().to_string(),
        fresh: Some(fresh),
        focus: focus.map(String::from),
        from,
    });
}

/// Opens the editor on a module of a stack.
pub fn edit(stack_id: &str, module_id: &str, focus: Option<&str>, from: Option<gtk::Widget>) {
    open(Editing {
        stack_id: stack_id.to_string(),
        module_id: module_id.to_string(),
        fresh: None,
        focus: focus.map(String::from),
        from,
    });
}

pub fn open(e: Editing) {
    if current().is_some() {
        return;
    }
    let Some(layer) = the_layer() else { return };
    let stack = if e.stack_id == NEW {
        Stack::new(NEW)
    } else {
        match state::read(|s| s.board.stack(&e.stack_id).cloned()) {
            Some(s) => s,
            None => return,
        }
    };
    let Some(module) = e.fresh.clone().or_else(|| stack.modules().into_iter().find(|m| m.id() == e.module_id)) else {
        return;
    };
    let layer_kind = module.layer();

    let root = gtk::Overlay::new();
    let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scrim.add_css_class("editor-scrim");
    root.set_child(Some(&scrim));
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("wb");
    card.add_css_class("editor");
    card.add_css_class(match layer_kind {
        Layer::Template => "editor-template",
        Layer::Extensions => "editor-extensions",
        Layer::Customization => "editor-customization",
    });
    card.set_overflow(gtk::Overflow::Hidden);
    card.set_halign(gtk::Align::Fill);
    card.set_valign(gtk::Align::Fill);
    root.add_overlay(&card);
    // Exactly the layer's box (its margins place it), whatever it'd ask for.
    root.connect_get_child_position(|o, _| Some(gtk::gdk::Rectangle::new(0, 0, o.width(), o.height())));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.add_css_class("editor-head");
    let icon = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon.add_css_class("editor-icon");
    icon.set_valign(gtk::Align::Center);
    let i = Icon::new(faces::layer_icon(layer_kind), 20);
    i.set_halign(gtk::Align::Center);
    i.set_margin_top(9);
    icon.append(&i);
    head.append(&icon);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 0);
    titles.set_valign(gtk::Align::Center);
    titles.set_hexpand(true);
    let where_ = if e.stack_id == NEW { "Your first wadspace".to_string() } else { stack_name(&stack) };
    let kind = gtk::Label::new(Some(&format!("{} · {where_}", layer_kind.label()).to_uppercase()));
    kind.add_css_class("editor-kind");
    kind.set_xalign(0.0);
    kind.set_ellipsize(gtk::pango::EllipsizeMode::End);
    titles.append(&kind);
    let title = gtk::Label::new(None);
    title.add_css_class("editor-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    titles.append(&title);
    head.append(&titles);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Sm);
    head.append(&cancel);
    let clash = gtk::Label::new(None);
    clash.add_css_class("name-clash");
    clash.set_wrap(true);
    clash.set_max_width_chars(36);
    head.append(&clash);
    let fork = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    fork.add_css_class("fork-note");
    let fork_text = gtk::Label::new(None);
    fork_text.set_wrap(true);
    fork_text.set_max_width_chars(44);
    fork_text.set_xalign(0.0);
    fork.append(&fork_text);
    let fork_button = ui::button("", None, Variant::Ghost, Size::Sm);
    fork_button.add_css_class("btn-mini");
    fork_button.set_valign(gtk::Align::Center);
    fork.append(&fork_button);
    head.append(&fork);
    let save = ui::button(if e.fresh.is_some() { "Add" } else { "Save and shrink" }, None, Variant::Primary, Size::Sm);
    head.append(&save);
    let close = ui::icon_button("x", "Close", 16);
    head.append(&close);
    for w in [
        cancel.upcast_ref::<gtk::Widget>(),
        clash.upcast_ref(),
        fork.upcast_ref(),
        save.upcast_ref(),
        close.upcast_ref(),
    ] {
        w.set_valign(gtk::Align::Center);
    }
    card.append(&head);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("editor-body");
    body.set_vexpand(true);
    card.append(&body);

    let ed = Rc::new(Editor {
        stack_id: e.stack_id.clone(),
        module_id: e.module_id.clone(),
        fresh: e.fresh.is_some(),
        focus: e.focus.clone(),
        stack0: stack,
        draft: RefCell::new(module),
        redraws: RefCell::default(),
        kept: RefCell::default(),
        root: root.clone(),
        scrim: scrim.clone(),
        card: card.clone(),
        head_rest: vec![
            cancel.clone().upcast(),
            clash.clone().upcast(),
            fork.clone().upcast(),
            save.clone().upcast(),
            close.clone().upcast(),
        ],
        kind,
        title,
        clash,
        fork,
        fork_text,
        fork_button: fork_button.clone(),
        save: save.clone(),
        body,
        phase: Cell::new(Phase::Grow),
        at: Cell::new([0.0; 4]),
        from: Cell::new([0.0; 4]),
    });
    let w = Rc::downgrade(&ed);
    cancel.connect_clicked(move |_| with(&w, |ed| ed.close(false)));
    let w = Rc::downgrade(&ed);
    close.connect_clicked(move |_| with(&w, |ed| ed.close(false)));
    let w = Rc::downgrade(&ed);
    save.connect_clicked(move |_| with(&w, |ed| ed.close(true)));
    let w = Rc::downgrade(&ed);
    fork_button.connect_clicked(move |_| {
        with(&w, |ed| {
            let name = ed.free_name();
            if let Module::Template(mut t) = ed.draft() {
                t.name = name;
                ed.set(Module::Template(t));
            }
        })
    });
    // A click on the dim around it is Cancel.
    let click = gtk::GestureClick::new();
    let w = Rc::downgrade(&ed);
    click.connect_released(move |_, _, _, _| with(&w, |ed| ed.close(false)));
    scrim.add_controller(click);

    let body: gtk::Widget = match layer_kind {
        Layer::Template => template::build(&ed),
        Layer::Extensions => extensions::build(&ed),
        Layer::Customization => customization::build(&ed),
    };
    body.set_vexpand(true);
    ed.body.append(&body);
    ed.update_head();
    for w in &ed.head_rest {
        w.set_visible(false);
    }
    ed.kind.set_visible(false);
    ed.body.set_visible(false);

    let from = e
        .from
        .as_ref()
        .and_then(|w| w.compute_bounds(&layer))
        .filter(|b| b.width() > 1.0)
        .map(|b| [b.x() as f64, b.y() as f64, b.width() as f64, b.height() as f64])
        .unwrap_or_else(|| {
            let (lw, lh) = (layer.width() as f64, layer.height() as f64);
            [lw / 2.0 - 150.0, lh / 2.0 - 90.0, 300.0, 180.0]
        });
    ed.from.set(from);
    ed.place(from);
    layer_show(&root);
    OPEN.with(|o| o.replace(Some(ed.clone())));
    let s = scrim.clone();
    glib::idle_add_local_once(move || s.add_css_class("shown"));
    ed.animate(Phase::Grow);
}

fn with(w: &Weak<Editor>, f: impl FnOnce(&Rc<Editor>)) {
    if let Some(ed) = w.upgrade() {
        f(&ed);
    }
}

/// CSS's cubic-bezier(x1, y1, x2, y2) at `t`.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| 3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3);
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut s = t;
    for _ in 0..30 {
        let x = at(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    at(y1, y2, s)
}

/// The editor's growing and shrinking: quick, then settling.
fn ease(t: f64) -> f64 {
    bezier(0.2, 0.9, 0.25, 1.0, t.clamp(0.0, 1.0))
}

impl Editor {
    /// The draft as it is now.
    pub fn draft(&self) -> Module {
        self.draft.borrow().clone()
    }

    /// The draft changed: the head and the body say so.
    pub fn set(self: &Rc<Self>, m: Module) {
        self.draft.replace(m);
        self.update_head();
        self.redraw();
    }

    /// The draft changed where it's typed: the head says so (the body
    /// doesn't need drawing again).
    pub fn set_quiet(self: &Rc<Self>, m: Module) {
        self.draft.replace(m);
        self.update_head();
    }

    /// `f` runs whenever the draft changes.
    pub fn on_redraw(&self, f: impl Fn(&Rc<Editor>) + 'static) {
        self.redraws.borrow_mut().push(Box::new(f));
    }

    /// Something the body keeps while it's open.
    pub fn keep(&self, x: Rc<dyn std::any::Any>) {
        self.kept.borrow_mut().push(x);
    }

    pub fn redraw(self: &Rc<Self>) {
        let fs = std::mem::take(&mut *self.redraws.borrow_mut());
        for f in &fs {
            f(self);
        }
        let mut later = std::mem::take(&mut *self.redraws.borrow_mut());
        let mut all = fs;
        all.append(&mut later);
        self.redraws.replace(all);
    }

    /// Its stack as the board has it now.
    pub fn stack(&self) -> Stack {
        if self.stack_id == NEW {
            return Stack::new(NEW);
        }
        state::read(|s| s.board.stack(&self.stack_id).cloned()).unwrap_or_else(|| self.stack0.clone())
    }

    /// The module as its stack has it (None: a new one).
    pub fn original(&self) -> Option<Module> {
        if self.fresh {
            return None;
        }
        self.stack().modules().into_iter().find(|m| m.id() == self.module_id)
    }

    /// The stack as it would be with the draft saved.
    pub fn live(&self) -> Stack {
        let mut s = self.stack();
        match self.draft() {
            Module::Template(t) => {
                if self.fresh {
                    s.templates.push(t);
                } else if let Some(x) = s.templates.iter_mut().find(|x| x.id == t.id) {
                    *x = t;
                }
            }
            Module::Extensions(e) => s.extensions = Some(e),
            Module::Customization(c) => s.customization = Some(c),
        }
        s
    }

    /// Why Save can't (the name's another's), or None.
    fn clash_now(&self) -> Option<String> {
        let d = self.draft();
        let name = module_name(&d);
        let name = if name == "Untitled" { String::new() } else { name };
        state::read(|s| board::name_clash(&s.board.stacks, &self.stack_id, &d, &name))
    }

    /// A shared template whose apps changed: it's a different template now.
    fn forked(&self, clash: bool) -> bool {
        match (self.draft(), self.original()) {
            (Module::Template(d), Some(Module::Template(o))) => clash && o.name == d.name,
            _ => false,
        }
    }

    /// A free name for a template that's forked: "<name> - n".
    fn free_name(&self) -> String {
        let Module::Template(d) = self.draft() else { return String::new() };
        let taken: Vec<String> = state::read(|s| {
            s.board.stacks.iter().flat_map(|st| st.templates.iter().map(|t| t.name.to_lowercase())).collect()
        });
        let base = strip_counter(&d.name);
        let mut n = 1;
        while taken.contains(&format!("{base} - {n}").to_lowercase()) {
            n += 1;
        }
        format!("{base} - {n}")
    }

    fn update_head(&self) {
        let d = self.draft();
        self.title.set_text(&module_name(&d));
        if self.phase.get() != Phase::Open {
            return;
        }
        let clash = self.clash_now();
        let forked = self.forked(clash.is_some());
        self.clash.set_text(clash.as_deref().unwrap_or(""));
        self.clash.set_visible(clash.is_some() && !forked);
        self.fork.set_visible(forked);
        if forked {
            let name = match &d {
                Module::Template(t) => t.name.clone(),
                _ => String::new(),
            };
            self.fork_text
                .set_text(&format!("Its apps changed, so it's no longer the {name} the other wadspaces share."));
            ui::set_text(&self.fork_button, &format!("Name it {}", self.free_name()));
        }
        self.save.set_sensitive(clash.is_none());
        self.save.set_tooltip_text(clash.as_deref());
    }

    /// Where it is: a box in the layer's coordinates.
    fn place(&self, b: [f64; 4]) {
        self.at.set(b);
        let Some(layer) = the_layer() else { return };
        let (lw, lh) = (layer.width() as f64, layer.height() as f64);
        let m = |v: f64| v.max(0.0).round() as i32;
        self.card.set_margin_start(m(b[0]));
        self.card.set_margin_top(m(b[1]));
        self.card.set_margin_end(m(lw - b[0] - b[2]));
        self.card.set_margin_bottom(m(lh - b[1] - b[3]));
    }

    /// The whole window, but for the inset.
    fn full() -> [f64; 4] {
        let (lw, lh) = the_layer().map(|l| (l.width() as f64, l.height() as f64)).unwrap_or((1280.0, 800.0));
        [INSET, INSET, lw - 2.0 * INSET, lh - 2.0 * INSET]
    }

    /// Grows to fill the window (then opens), or shrinks into the module.
    fn animate(self: &Rc<Self>, to: Phase) {
        self.phase.set(to);
        let start = self.at.get();
        let t0 = Cell::new(None::<i64>);
        let w = Rc::downgrade(self);
        self.card.add_tick_callback(move |_, clock| {
            let Some(ed) = w.upgrade() else { return glib::ControlFlow::Break };
            let now = clock.frame_time();
            let t0 = match t0.get() {
                Some(t) => t,
                None => {
                    t0.set(Some(now));
                    now
                }
            };
            let k = ((now - t0) as f64 / GROW_US).min(1.0);
            let end = if to == Phase::Grow { Editor::full() } else { ed.shrink_target() };
            let e = ease(k);
            let b: Vec<f64> = (0..4).map(|i| start[i] + (end[i] - start[i]) * e).collect();
            ed.place([b[0], b[1], b[2], b[3]]);
            if k < 1.0 {
                return glib::ControlFlow::Continue;
            }
            if to == Phase::Grow {
                ed.opened();
            } else {
                ed.gone();
            }
            glib::ControlFlow::Break
        });
    }

    fn opened(self: &Rc<Self>) {
        self.phase.set(Phase::Open);
        // Open, it follows the window: margins all round.
        let m = INSET as i32;
        self.card.set_margin_start(m);
        self.card.set_margin_top(m);
        self.card.set_margin_end(m);
        self.card.set_margin_bottom(m);
        self.at.set(Editor::full());
        self.kind.set_visible(true);
        for w in &self.head_rest {
            w.set_visible(true);
        }
        self.body.set_visible(true);
        self.card.add_css_class("open");
        self.update_head();
        self.redraw();
    }

    /// Where it shrinks to: the module where the board has it now, else
    /// where it came from.
    fn shrink_target(&self) -> [f64; 4] {
        let Some(layer) = the_layer() else { return self.from.get() };
        super::tile_widget(&self.module_id)
            .filter(|w| w.is_mapped())
            .and_then(|w| w.compute_bounds(&layer))
            .filter(|b| b.width() > 1.0)
            .map(|b| [b.x() as f64, b.y() as f64, b.width() as f64, b.height() as f64])
            .unwrap_or_else(|| self.from.get())
    }

    /// Saves (when asked and the name's free) and shrinks away.
    pub fn close(self: &Rc<Self>, save: bool) {
        if self.phase.get() != Phase::Open {
            return;
        }
        if save {
            if self.clash_now().is_some() {
                return;
            }
            let (id, fresh, d) = (self.stack_id.clone(), self.fresh, self.draft());
            let n = state::edit(|b| b.save_module(&id, fresh, id == NEW, d));
            state::tell(n);
            if id == NEW {
                crate::tour::signal(crate::tour::Event::StackCreated);
            }
        }
        if let Some(p) = MENU.with(|m| m.borrow().upgrade()) {
            p.popdown();
        }
        self.card.remove_css_class("open");
        self.kind.set_visible(false);
        for w in &self.head_rest {
            w.set_visible(false);
        }
        self.body.set_visible(false);
        self.scrim.remove_css_class("shown");
        self.animate(Phase::Shrink);
    }

    fn gone(&self) {
        layer_hide(&self.root);
        self.redraws.borrow_mut().clear();
        self.kept.borrow_mut().clear();
        OPEN.with(|o| o.replace(None));
    }
}

/// "Dev - 2" → "Dev".
fn strip_counter(name: &str) -> &str {
    match name.rsplit_once(" - ") {
        Some((base, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => base,
        _ => name,
    }
}

// ─────────────────────────────── shared pieces ───────────────────────────────

/// A field: its name above it (the web app's `.field`).
pub fn field(label: &str, input: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 5);
    b.add_css_class("field");
    let l = gtk::Label::new(Some(&label.to_uppercase()));
    l.add_css_class("field-name");
    l.set_xalign(0.0);
    b.append(&l);
    b.append(input);
    b
}

/// A panel's heading.
pub fn panel_title(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("panel-title-sm");
    l.set_xalign(0.0);
    l
}

/// A small heading over a group.
pub fn group_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(&text.to_uppercase()));
    l.add_css_class("group-label");
    l.set_xalign(0.0);
    l
}

/// A note in the rail's small print.
pub fn note(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("rail-note");
    l
}

/// A line with an icon before it (chain notes, warnings).
pub fn icon_note(icon: &'static str, text: &str, class: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.add_css_class(class);
    let i = Icon::new(icon, 12);
    i.set_valign(gtk::Align::Start);
    i.set_margin_top(2);
    b.append(&i);
    b.append(&ui::wrapping(text));
    b
}

/// A list that scrolls up and down (and scrolls itself near its edges
/// while something's dragged).
pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_vexpand(true);
    sw.set_child(Some(child));
    crate::drag::autoscroll(&sw);
    sw
}

/// Brings `w` into view in `sw`, once the editor's open.
pub fn scroll_into_view(sw: &gtk::ScrolledWindow, w: &impl IsA<gtk::Widget>) {
    let (sw, w) = (sw.downgrade(), w.upcast_ref::<gtk::Widget>().downgrade());
    glib::timeout_add_local_once(std::time::Duration::from_millis(360), move || {
        let (Some(sw), Some(w)) = (sw.upgrade(), w.upgrade()) else { return };
        let Some(child) = sw.child() else { return };
        if let Some(p) = w.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)) {
            let adj = sw.vadjustment();
            let y = p.y() as f64;
            let h = w.height() as f64;
            if y < adj.value() || y + h > adj.value() + adj.page_size() {
                adj.set_value((y - 12.0).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
            }
        }
    });
}

/// Drawn again whenever the draft changes: `fill` puts what's there now
/// into `b`.
pub fn live_box(ed: &Rc<Editor>, b: &gtk::Box, fill: impl Fn(&Rc<Editor>, &gtk::Box) + 'static) {
    fill(ed, b);
    let w = b.downgrade();
    ed.on_redraw(move |ed| {
        if let Some(b) = w.upgrade() {
            ui::clear(&b);
            fill(ed, &b);
        }
    });
}

/// For the snapshots: an editor opened by hand, and closed.
pub mod sim {
    use super::*;

    pub fn open(stack_id: &str, layer: Layer, focus: Option<&str>) -> bool {
        let Some(stack) = state::read(|s| s.board.stack(stack_id).cloned()) else { return false };
        let module = match layer {
            Layer::Template => stack.top().map(|t| t.id.clone()),
            Layer::Extensions => stack.extensions.as_ref().map(|e| e.id.clone()),
            Layer::Customization => stack.customization.as_ref().map(|c| c.id.clone()),
        };
        match module {
            Some(id) => edit(stack_id, &id, focus, super::super::tile_widget(&id)),
            None => add(layer, stack_id, None, focus),
        }
        current().is_some()
    }

    pub fn close() {
        cancel();
    }

    pub fn save() {
        if let Some(ed) = current() {
            ed.close(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_eases_quickly_then_settles() {
        assert!(ease(0.0).abs() < 1e-3);
        assert!((ease(1.0) - 1.0).abs() < 1e-3);
        assert!(ease(0.3) > 0.6);
        assert!(ease(0.5) < ease(0.6));
    }

    #[test]
    fn names_are_made_free() {
        assert_eq!(free("New template", &[]), "New template");
        let taken = vec!["new template".to_string(), "new template - 1".to_string()];
        assert_eq!(free("New template", &taken), "New template - 2");
        assert_eq!(strip_counter("Dev - 2"), "Dev");
        assert_eq!(strip_counter("Dev - x"), "Dev - x");
        assert_eq!(strip_counter("Dev"), "Dev");
    }
}
