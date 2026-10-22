//! Dragging inside the app (the web app's builder/drag.tsx): a grabbed thing
//! follows the pointer as a ghost (in a layer over the app that takes no
//! input), the target under the pointer says whether it fits (a hint by the
//! pointer: green, or red and why), and a drop that lands nowhere snaps
//! back. Right-click or Escape mid-drag cancels. A drag starts 6 px from
//! where it was grabbed; the grabber reads Shift then, and Alt is read live.
//! Lists that scroll scroll by themselves when the pointer nears an edge.
//!
//! Targets are widgets: the deepest one under the pointer that's interested
//! (`accepts` isn't None) decides, else the next one up.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, graphene, prelude::*};

/// What's being dragged.
#[derive(Debug, Clone)]
pub enum Payload {
    /// WadBrowser's apps (one, or several Ctrl+clicked) moving between key
    /// groups (a wadspace's keys).
    Member { members: Vec<String>, group: String },
    /// Something of the Builder's board, or from its sidebar.
    B(wad_board::b::Payload),
}

/// What a drop here does ("Copy to VanuaAcademy"), or why it can't.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub ok: bool,
    pub why: Option<String>,
}

impl Verdict {
    pub fn yes(why: impl Into<String>) -> Option<Verdict> {
        Some(Verdict { ok: true, why: Some(why.into()) })
    }

    pub fn no(why: impl Into<String>) -> Option<Verdict> {
        Some(Verdict { ok: false, why: Some(why.into()) })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    /// Alt is held: move rather than copy.
    pub moving: bool,
}

/// A drop target.
pub trait Target {
    /// None: not interested (a target underneath gets a say). `at`: the
    /// pointer, in the target's own coordinates (NaN: asked whether it
    /// could take it at all, to light up while dragging).
    fn accepts(&self, p: &Payload, m: Mods, at: (f64, f64)) -> Option<Verdict>;
    fn land(&self, p: &Payload, m: Mods, at: (f64, f64));
    /// The pointer came over it (or left).
    fn hover(&self, _on: bool) {}
}

/// The target under the pointer: its widget, itself, what it says.
type Hovered = (gtk::Widget, Rc<dyn Target>, Verdict);

struct Active {
    payload: Payload,
    ghost: gtk::Widget,
    /// Where it came from (the layer's coordinates), to snap back to.
    origin: (f64, f64),
    /// Where in it it was grabbed.
    grab: (f64, f64),
    pointer: Cell<(f64, f64)>,
    mods: Cell<Mods>,
    hover: RefCell<Option<Hovered>>,
    lit: RefCell<Vec<gtk::Widget>>,
    hint: gtk::Label,
    done: Cell<bool>,
    /// The pointer moved since the targets were last asked (they're asked
    /// once a frame, however often it moves).
    stale: Cell<bool>,
}

type Registered = (glib::WeakRef<gtk::Widget>, Rc<dyn Target>);

/// What hears drags start and end.
type Watcher = Rc<dyn Fn(Option<&Payload>)>;

/// A widget's key in the targets: where it is (checked against it, in
/// case another took its place).
fn key(w: &gtk::Widget) -> usize {
    w.as_ptr() as usize
}

thread_local! {
    static LAYER: RefCell<Option<gtk::Fixed>> = const { RefCell::new(None) };
    static TARGETS: RefCell<std::collections::HashMap<usize, Registered>> = RefCell::default();
    static SCROLLERS: RefCell<Vec<glib::WeakRef<gtk::ScrolledWindow>>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: RefCell<Option<Rc<Active>>> = const { RefCell::new(None) };
    static WATCHERS: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
}

/// `f` hears each drag start (what's dragged) and end (None): what shows
/// only while something is dragged.
pub fn watch(f: impl Fn(Option<&Payload>) + 'static) {
    WATCHERS.with(|w| w.borrow_mut().push(Rc::new(f)));
}

fn tell(p: Option<&Payload>) {
    let all: Vec<Watcher> = WATCHERS.with(|w| w.borrow().clone());
    for f in all {
        f(p);
    }
}

/// Where ghosts go: over the app, taking no input.
pub fn layer() -> gtk::Fixed {
    let f = gtk::Fixed::new();
    f.set_can_target(false);
    f.add_css_class("drag-layer");
    LAYER.with(|l| l.replace(Some(f.clone())));
    f
}

fn the_layer() -> Option<gtk::Fixed> {
    LAYER.with(|l| l.borrow().clone())
}

/// A drag's under way.
pub fn active() -> bool {
    ACTIVE.with(|a| a.borrow().is_some())
}

/// Makes `w` a drop target.
pub fn target(w: &impl IsA<gtk::Widget>, t: impl Target + 'static) {
    let w = w.upcast_ref::<gtk::Widget>();
    TARGETS.with(|ts| {
        let mut ts = ts.borrow_mut();
        ts.retain(|_, (w, _)| w.upgrade().is_some());
        ts.insert(key(w), (w.downgrade(), Rc::new(t)));
    });
}

/// Makes a list scroll by itself when a drag nears its edge.
pub fn autoscroll(sw: &gtk::ScrolledWindow) {
    SCROLLERS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|w| w.upgrade().is_some());
        s.push(sw.downgrade());
    });
}

fn target_of(w: &gtk::Widget) -> Option<Rc<dyn Target>> {
    TARGETS.with(|ts| ts.borrow().get(&key(w)).filter(|(x, _)| x.upgrade().as_ref() == Some(w)).map(|(_, t)| t.clone()))
}

/// Makes `w` draggable: `make` runs as the drag starts (with Shift held or
/// not) and gives what's dragged and its ghost.
pub fn draggable(w: &impl IsA<gtk::Widget>, make: impl Fn(bool) -> Option<(Payload, gtk::Widget)> + 'static) {
    draggable_from(w, move |shift| make(shift).map(|(p, g)| (p, g, None)));
}

/// A drag's start: what's dragged, its ghost, and where the ghost starts
/// (None: over the widget grabbed).
pub type Grab = (Payload, gtk::Widget, Option<gtk::Widget>);

/// Makes `w` draggable, its ghost starting over another widget when `make`
/// names one: a module grabbed takes those on top of it too, and their pile
/// starts where they are (the pointer keeps its place on what it grabbed).
pub fn draggable_from(w: &impl IsA<gtk::Widget>, make: impl Fn(bool) -> Option<Grab> + 'static) {
    let g = gtk::GestureDrag::new();
    g.set_button(1);
    let started = Rc::new(Cell::new(false));
    let start = Rc::new(Cell::new((0.0, 0.0)));
    let s = start.clone();
    let st = started.clone();
    g.connect_drag_begin(move |_, x, y| {
        s.set((x, y));
        st.set(false);
    });
    let make = Rc::new(make);
    let st = started.clone();
    g.connect_drag_update(move |g, dx, dy| {
        let Some(layer) = the_layer() else { return };
        let Some(w) = g.widget() else { return };
        let (x0, y0) = start.get();
        let state = g.current_event_state();
        let mods = Mods { moving: state.contains(gtk::gdk::ModifierType::ALT_MASK) };
        if !st.get() {
            if (dx * dx + dy * dy).sqrt() < 6.0 || active() {
                return;
            }
            let Some((payload, ghost, anchor)) = make(state.contains(gtk::gdk::ModifierType::SHIFT_MASK)) else {
                return;
            };
            st.set(true);
            g.set_state(gtk::EventSequenceState::Claimed);
            let at = anchor.filter(|a| a.is_mapped()).unwrap_or_else(|| w.clone());
            let corner = |x: f32, y: f32, of: &gtk::Widget| {
                of.compute_point(&layer, &graphene::Point::new(x, y))
                    .map(|p| (p.x() as f64, p.y() as f64))
                    .unwrap_or_default()
            };
            let origin = corner(0.0, 0.0, &at);
            // Where it was pressed, from the ghost's corner.
            let press = corner(x0 as f32, y0 as f32, &w);
            begin(payload, ghost, origin, (press.0 - origin.0, press.1 - origin.1), at.width());
        }
        let p = w.compute_point(&layer, &graphene::Point::new((x0 + dx) as f32, (y0 + dy) as f32));
        if let Some(p) = p {
            moved((p.x() as f64, p.y() as f64), mods);
        }
    });
    let st = started.clone();
    g.connect_drag_end(move |_, _, _| {
        if st.replace(false) {
            finish(true);
        }
    });
    let st = started.clone();
    g.connect_cancel(move |_, _| {
        if st.replace(false) {
            finish(false);
        }
    });
    w.add_controller(g);
}

fn begin(payload: Payload, ghost: gtk::Widget, origin: (f64, f64), grab: (f64, f64), width: i32) {
    let Some(layer) = the_layer() else { return };
    ghost.add_css_class("drag-ghost");
    ghost.set_size_request(width, -1);
    layer.put(&ghost, origin.0, origin.1);
    let hint = gtk::Label::new(None);
    hint.add_css_class("drag-hint");
    layer.put(&hint, origin.0, origin.1);
    if let Some(win) = layer.root() {
        win.add_css_class("is-dragging");
    }
    let a = Rc::new(Active {
        payload,
        ghost,
        origin,
        grab,
        pointer: Cell::new((origin.0 + grab.0, origin.1 + grab.1)),
        mods: Cell::new(Mods::default()),
        hover: RefCell::default(),
        lit: RefCell::default(),
        hint,
        done: Cell::new(false),
        stale: Cell::new(false),
    });
    ACTIVE.with(|x| x.replace(Some(a.clone())));
    tell(Some(&a.payload));
    // What could take it lights up (asked with no pointer: NaN).
    let all: Vec<Registered> = TARGETS.with(|ts| ts.borrow().values().cloned().collect());
    for (w, t) in all {
        if let Some(w) = w.upgrade()
            && t.accepts(&a.payload, Mods::default(), (f64::NAN, f64::NAN)).is_some_and(|v| v.ok)
        {
            w.add_css_class("drop-could");
            a.lit.borrow_mut().push(w);
        }
    }
    // Escape and right-click cancel.
    if let Some(win) = layer.root() {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.set_name(Some("drag-keys"));
        keys.connect_key_pressed(|_, k, _, mods| {
            if k == gtk::gdk::Key::Escape {
                finish(false);
                return glib::Propagation::Stop;
            }
            if matches!(k, gtk::gdk::Key::Alt_L | gtk::gdk::Key::Alt_R)
                || mods.contains(gtk::gdk::ModifierType::ALT_MASK)
            {
                set_moving(true);
            }
            glib::Propagation::Proceed
        });
        keys.connect_key_released(|_, k, _, _| {
            if matches!(k, gtk::gdk::Key::Alt_L | gtk::gdk::Key::Alt_R) {
                set_moving(false);
            }
        });
        win.add_controller(keys);
        let right = gtk::GestureClick::new();
        right.set_button(3);
        right.set_propagation_phase(gtk::PropagationPhase::Capture);
        right.set_name(Some("drag-right"));
        right.connect_pressed(|g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            finish(false);
        });
        win.add_controller(right);
    }
    // Lists near the pointer's edge scroll.
    glib::timeout_add_local(Duration::from_millis(16), || {
        let Some(a) = ACTIVE.with(|x| x.borrow().clone()) else { return glib::ControlFlow::Break };
        if a.done.get() {
            return glib::ControlFlow::Break;
        }
        scroll_near(a.pointer.get());
        glib::ControlFlow::Continue
    });
    let p = a.pointer.get();
    moved(p, Mods::default());
}

fn set_moving(on: bool) {
    if let Some(a) = ACTIVE.with(|x| x.borrow().clone()) {
        let m = Mods { moving: on };
        if a.mods.get() != m {
            a.mods.set(m);
            update(&a);
        }
    }
}

fn scroll_near((x, y): (f64, f64)) {
    let Some(layer) = the_layer() else { return };
    let list: Vec<gtk::ScrolledWindow> = SCROLLERS.with(|s| s.borrow().iter().filter_map(|w| w.upgrade()).collect());
    for sw in list {
        if !sw.is_mapped() {
            continue;
        }
        let Some(b) = sw.compute_bounds(&layer) else { continue };
        let (l, t, r, bt) = (b.x() as f64, b.y() as f64, (b.x() + b.width()) as f64, (b.y() + b.height()) as f64);
        if x < l || x > r || y < t || y > bt {
            continue;
        }
        let edge = 40.0;
        let v = sw.vadjustment();
        if y > bt - edge {
            v.set_value(v.value() + 14.0);
        } else if y < t + edge {
            v.set_value(v.value() - 14.0);
        }
        let h = sw.hadjustment();
        if x > r - edge {
            h.set_value(h.value() + 14.0);
        } else if x < l + edge {
            h.set_value(h.value() - 14.0);
        }
    }
}

fn moved(p: (f64, f64), mods: Mods) {
    let Some(a) = ACTIVE.with(|x| x.borrow().clone()) else { return };
    if a.done.get() {
        return;
    }
    a.pointer.set(p);
    // Alt as the pointer's event says it now.
    a.mods.set(mods);
    if let Some(layer) = the_layer() {
        layer.move_(&a.ghost, p.0 - a.grab.0, p.1 - a.grab.1);
        layer.move_(&a.hint, p.0 + 18.0, p.1 - 40.0);
    }
    // The ghost follows every move; what's under it is asked once a frame.
    if !a.stale.replace(true) {
        let aa = a.clone();
        a.ghost.add_tick_callback(move |_, _| {
            if aa.stale.replace(false) && !aa.done.get() {
                update(&aa);
            }
            glib::ControlFlow::Break
        });
    }
}

fn update(a: &Rc<Active>) {
    let Some(layer) = the_layer() else { return };
    let Some(frame) = layer.parent() else { return };
    let (x, y) = a.pointer.get();
    let mut found: Option<Hovered> = None;
    let mut w = frame.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(cur) = w {
        if let Some(t) = target_of(&cur) {
            let at = layer
                .compute_point(&cur, &graphene::Point::new(x as f32, y as f32))
                .map(|p| (p.x() as f64, p.y() as f64))
                .unwrap_or_default();
            if let Some(v) = t.accepts(&a.payload, a.mods.get(), at) {
                found = Some((cur, t, v));
                break;
            }
        }
        w = cur.parent();
    }
    let was = a.hover.borrow().as_ref().map(|(w, t, _)| (w.clone(), t.clone()));
    let same = match (&was, &found) {
        (Some((w0, _)), Some((w1, _, _))) => w0 == w1,
        (None, None) => true,
        _ => false,
    };
    if !same {
        if let Some((w0, t0)) = &was {
            w0.remove_css_class("drop-ok");
            w0.remove_css_class("drop-no");
            t0.hover(false);
        }
        if let Some((w1, t1, _)) = &found {
            t1.hover(true);
            let _ = w1;
        }
    }
    if let Some((w1, _, v)) = &found {
        w1.add_css_class(if v.ok { "drop-ok" } else { "drop-no" });
        w1.remove_css_class(if v.ok { "drop-no" } else { "drop-ok" });
    }
    // The hint changes only when what it says does.
    let (tone, text) = match &found {
        Some((_, _, v)) => (Some(v.ok), v.why.as_deref().unwrap_or("")),
        None => (
            None,
            match a.payload {
                Payload::B(_) => "Drop on a wadspace, or on a machine below · right-click to cancel",
                Payload::Member { .. } => "Right-click to cancel",
            },
        ),
    };
    for (class, on) in [("ok", tone == Some(true)), ("no", tone == Some(false))] {
        if on != a.hint.has_css_class(class) {
            if on {
                a.hint.add_css_class(class);
            } else {
                a.hint.remove_css_class(class);
            }
        }
    }
    if a.hint.text() != text {
        a.hint.set_text(text);
    }
    a.hint.set_visible(!text.is_empty());
    a.hover.replace(found);
}

/// A drag made by hand (the snapshots): picked up from `w`, the pointer
/// then put where `to` says (the layer's coordinates), and let go.
pub mod sim {
    use super::*;

    pub fn begin(w: &impl IsA<gtk::Widget>, payload: Payload, ghost: gtk::Widget) {
        let Some(layer) = the_layer() else { return };
        let w = w.upcast_ref::<gtk::Widget>();
        let origin = w
            .compute_point(&layer, &graphene::Point::new(0.0, 0.0))
            .map(|p| (p.x() as f64, p.y() as f64))
            .unwrap_or_default();
        super::begin(payload, ghost, origin, (40.0, 20.0), w.width());
    }

    /// The pointer over the middle of `w`, `dy` down from it.
    pub fn over(w: &impl IsA<gtk::Widget>, dx: f64, dy: f64) {
        let Some(layer) = the_layer() else { return };
        let w = w.upcast_ref::<gtk::Widget>();
        if let Some(p) = w.compute_point(&layer, &graphene::Point::new(w.width() as f32 / 2.0 + dx as f32, dy as f32)) {
            moved((p.x() as f64, p.y() as f64), Mods::default());
        }
    }

    pub fn end(drop: bool) {
        finish(drop);
    }
}

/// Ends the drag: dropped where it fits, else snapped back.
fn finish(drop: bool) {
    let Some(a) = ACTIVE.with(|x| x.borrow().clone()) else { return };
    // Let go before the frame asked what's under it: ask now.
    if a.stale.replace(false) && !a.done.get() {
        update(&a);
    }
    if a.done.replace(true) {
        return;
    }
    let layer = the_layer();
    if let Some(win) = layer.as_ref().and_then(|l| l.root()) {
        win.remove_css_class("is-dragging");
        // Its Escape and right-click go.
        let ctrls: Vec<gtk::EventController> = win
            .observe_controllers()
            .into_iter()
            .filter_map(|c| c.ok().and_downcast::<gtk::EventController>())
            .filter(|c| matches!(c.name().as_deref(), Some("drag-keys" | "drag-right")))
            .collect();
        for c in ctrls {
            win.remove_controller(&c);
        }
    }
    for w in a.lit.borrow().iter() {
        w.remove_css_class("drop-could");
    }
    let hover = a.hover.borrow_mut().take();
    tell(None);
    if let Some((w, t, _)) = &hover {
        w.remove_css_class("drop-ok");
        w.remove_css_class("drop-no");
        t.hover(false);
    }
    a.hint.set_visible(false);
    let landed = match (&hover, drop) {
        (Some((w, t, v)), true) if v.ok => {
            let (x, y) = a.pointer.get();
            let at = layer
                .as_ref()
                .and_then(|l| l.compute_point(w, &graphene::Point::new(x as f32, y as f32)))
                .map(|p| (p.x() as f64, p.y() as f64))
                .unwrap_or_default();
            // After the drag's done (the drop may draw the board again).
            let (t, payload, mods) = (t.clone(), a.payload.clone(), a.mods.get());
            glib::idle_add_local_once(move || t.land(&payload, mods, at));
            true
        }
        _ => false,
    };
    if landed {
        end(&a);
        return;
    }
    // Snap back to where it came from.
    let from = a.pointer.get();
    let from = (from.0 - a.grab.0, from.1 - a.grab.1);
    let t0 = glib::monotonic_time();
    let aa = a.clone();
    a.ghost.add_tick_callback(move |_, _| {
        let k = ((glib::monotonic_time() - t0) as f64 / 230_000.0).min(1.0);
        let e = 1.0 - (1.0 - k).powi(3);
        if let Some(l) = the_layer() {
            l.move_(&aa.ghost, from.0 + (aa.origin.0 - from.0) * e, from.1 + (aa.origin.1 - from.1) * e);
        }
        if k >= 1.0 {
            end(&aa);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn end(a: &Rc<Active>) {
    if let Some(l) = the_layer() {
        if a.ghost.parent().is_some() {
            l.remove(&a.ghost);
        }
        if a.hint.parent().is_some() {
            l.remove(&a.hint);
        }
    }
    ACTIVE.with(|x| {
        if x.borrow().as_ref().is_some_and(|b| Rc::ptr_eq(b, a)) {
            x.replace(None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_say_yes_or_no_and_why() {
        assert_eq!(Verdict::yes("Copy"), Some(Verdict { ok: true, why: Some("Copy".into()) }));
        assert!(!Verdict::no("Nope").unwrap().ok);
        assert!(!active());
    }
}
