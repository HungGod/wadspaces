//! The new user's tour (the web app's Tour.tsx and lib/tour.ts): a card
//! beside what each step is about, with that spotlit: the Builder in the
//! sidebar, the board, + Create, the Build bucket, then Launch. Where it got
//! to is kept per person, so a restart resumes it; finishing (or skipping)
//! marks them onboarded so it doesn't run again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::{glib, prelude::*};
use wad_gtk::Icon;

use crate::shell::Page;
use crate::ui::{self, Size, Variant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Nav,
    Board,
    Create,
    Build,
    Launch,
}

/// What the Builder and the launcher tell the tour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    WadspaceMade,
    BuildStarted,
    LauncherOpened,
}

const ORDER: [Step; 5] = [Step::Nav, Step::Board, Step::Create, Step::Build, Step::Launch];
const NUMBERED: [Step; 4] = [Step::Board, Step::Create, Step::Build, Step::Launch];
const PAD: f64 = 8.0;
const CARD_W: f64 = 340.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Right,
    Below,
    Left,
    Above,
}

/// Each step: what it spotlights, where its card goes (by preference),
/// whether the rest is dimmed and blocked, whether it's the Builder's.
fn def(s: Step) -> (&'static str, &'static [Side], bool, bool) {
    match s {
        Step::Nav => ("nav-builder", &[Side::Right], true, false),
        Step::Board => ("board", &[Side::Below, Side::Left], false, true),
        Step::Create => ("create", &[Side::Below, Side::Left], false, true),
        Step::Build => ("build", &[Side::Above, Side::Right], false, true),
        Step::Launch => ("launcher", &[Side::Right, Side::Above], true, false),
    }
}

fn name(s: Step) -> &'static str {
    match s {
        Step::Nav => "nav",
        Step::Board => "board",
        Step::Create => "create",
        Step::Build => "build",
        Step::Launch => "launch",
    }
}

fn copy(s: Step, who: &str) -> (String, &'static str, Option<&'static str>) {
    match s {
        Step::Nav => (
            format!("Welcome to WadSpaces, {who}!"),
            "Wadspaces are made in the Builder: each its Customizations (its background, projects and keys) over its Templates (its apps and what they take), on one machine. Let's make your first one.",
            None,
        ),
        Step::Board => (
            "The board".into(),
            "This machine's wadspaces, your other machines along the bottom. Drag a template (and those under it) onto another wadspace to copy it there, Shift+drag for just it, or onto a machine for a new wadspace there. Click a wadspace to open it in the sidebar.",
            Some("Recently deleted (the clock, top right) keeps what's taken out, to put back."),
        ),
        Step::Create => (
            "Make a wadspace".into(),
            "Click the space at the end of the row for a new wadspace, with an empty template: then pick its apps in the sidebar.",
            None,
        ),
        Step::Build => (
            "Build it".into(),
            "Build all builds every wadspace here that needs it (Ctrl+click picks some, for Build selected). A wadspace says when it's out of date.",
            None,
        ),
        Step::Launch => (
            "Launch it".into(),
            "Launch opens wadspaces, WadBrowsers and the terminal together on your machine, or for a focus session. Click it to finish the tour.",
            None,
        ),
    }
}

#[derive(Default)]
struct State {
    step: Option<Step>,
    /// The step whose action was done (a wadspace made).
    created: bool,
    /// The tour's been started this run (it starts once by itself).
    started: bool,
    targets: Vec<(&'static str, glib::WeakRef<gtk::Widget>)>,
    frame: glib::WeakRef<gtk::Overlay>,
    /// What's on screen: blockers, ring, card.
    parts: Vec<gtk::Widget>,
    card: Option<gtk::Box>,
    /// The card's for this (step, lost).
    shown: Option<(Step, bool)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Where the tour draws: the app's frame (its overlays).
pub fn attach(frame: &gtk::Overlay) {
    with(|s| s.frame.set(Some(frame)));
}

/// Something a step spotlights.
pub fn target(name: &'static str, w: &impl IsA<gtk::Widget>) {
    with(|s| {
        s.targets.retain(|(n, _)| *n != name);
        s.targets.push((name, w.upcast_ref::<gtk::Widget>().downgrade()));
    });
}

fn saved_file() -> Option<std::path::PathBuf> {
    crate::account::data_file("tour")
}

/// Starts the tour (or, `resume`, carries on where it got to).
pub fn start(resume: bool) {
    let mut step = Step::Nav;
    if resume
        && let Some(saved) = saved_file().and_then(|p| std::fs::read_to_string(p).ok())
        && let Some(s) = ORDER.iter().find(|s| name(**s) == saved.trim())
    {
        step = *s;
    }
    with(|s| {
        s.step = Some(step);
        s.created = false;
        s.started = true;
    });
    save();
    // Already on the Builder: the first step's done.
    if step == Step::Nav && crate::shell::page() == Some(Page::Builder) {
        next();
        return;
    }
    show();
}

/// Starts it by itself, once, for someone not onboarded yet.
pub fn start_once() {
    if !with(|s| s.started) {
        start(true);
    }
}

/// Signed out: the tour stops (it starts again for whoever's next, if
/// they're new).
pub fn reset() {
    with(|s| {
        s.step = None;
        s.started = false;
        s.created = false;
    });
    clear();
}

pub fn running() -> bool {
    with(|s| s.step.is_some())
}

fn save() {
    if crate::fake::on() {
        return;
    }
    let (Some(path), step) = (saved_file(), with(|s| s.step)) else { return };
    match step {
        Some(s) => {
            let _ = wad_account::write_private(&path, name(s).as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn next() {
    let Some(step) = with(|s| s.step) else { return };
    let at = ORDER.iter().position(|s| *s == step).unwrap_or(0);
    if at + 1 >= ORDER.len() {
        return finish();
    }
    with(|s| s.step = Some(ORDER[at + 1]));
    save();
    show();
}

/// Finished, or skipped: they're onboarded.
pub fn finish() {
    with(|s| s.step = None);
    save();
    clear();
    if crate::fake::on() {
        return;
    }
    glib::spawn_future_local(async {
        if let Err(e) = crate::account::set_onboarded().await {
            tracing::warn!("onboarded: {e}");
        }
    });
}

/// The Builder made a wadspace, started a build, the launcher opened.
pub fn signal(e: Event) {
    match (with(|s| s.step), e) {
        (Some(Step::Create), Event::WadspaceMade) => {
            with(|s| s.created = true);
            next();
        }
        (Some(Step::Build), Event::BuildStarted) => next(),
        (Some(Step::Launch), Event::LauncherOpened) => finish(),
        _ => {}
    }
}

/// A page shown: opening the Builder finishes the first step.
pub fn page_changed(p: Page) {
    match with(|s| s.step) {
        Some(Step::Nav) if p == Page::Builder => next(),
        Some(_) => show(),
        None => {}
    }
}

fn clear() {
    let (frame, parts) = with(|s| {
        s.shown = None;
        s.card = None;
        (s.frame.upgrade(), std::mem::take(&mut s.parts))
    });
    if let Some(f) = frame {
        for p in parts {
            f.remove_overlay(&p);
        }
    }
}

/// A box placed in the frame: `(x, y, w, h)`.
fn place(w: &gtk::Widget, x: f64, y: f64, width: f64, height: f64) {
    w.set_halign(gtk::Align::Start);
    w.set_valign(gtk::Align::Start);
    w.set_margin_start(x.max(0.0).round() as i32);
    w.set_margin_top(y.max(0.0).round() as i32);
    w.set_size_request(width.max(0.0).round() as i32, height.max(0.0).round() as i32);
}

/// Draws the step: blockers and the ring round what it's about (followed
/// every frame), and its card.
fn show() {
    let Some(step) = with(|s| s.step) else { return };
    let Some(frame) = with(|s| s.frame.upgrade()) else { return };
    let (_, _, blocking, builder) = def(step);
    let lost = builder && crate::shell::page() != Some(Page::Builder);
    if with(|s| s.shown == Some((step, lost))) {
        return;
    }
    clear();
    let mut parts: Vec<gtk::Widget> = vec![];
    let mut blockers = vec![];
    if blocking && !lost {
        for _ in 0..4 {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("tour-dim");
            b.set_visible(false);
            // Swallows clicks outside the spotlight.
            b.add_controller(gtk::GestureClick::new());
            frame.add_overlay(&b);
            parts.push(b.clone().upcast());
            blockers.push(b);
        }
    }
    let ring = gtk::Box::new(gtk::Orientation::Vertical, 0);
    ring.add_css_class("tour-ring");
    ring.set_can_target(false);
    ring.set_visible(false);
    frame.add_overlay(&ring);
    parts.push(ring.clone().upcast());
    let card = card(step, lost);
    card.set_visible(false);
    frame.add_overlay(&card);
    parts.push(card.clone().upcast());
    let c2 = card.clone();
    glib::idle_add_local_once(move || c2.add_css_class("shown"));
    with(|s| {
        s.parts = parts;
        s.card = Some(card.clone());
        s.shown = Some((step, lost));
    });

    // Follows its target every frame (it moves with layout, scrolling,
    // resizes): ticked by the frame (the card's hidden until it's placed).
    let lost_since = Rc::new(Cell::new(None::<i64>));
    frame.add_tick_callback(move |frame, clock| {
        if with(|s| s.shown != Some((step, lost))) {
            return glib::ControlFlow::Break;
        }
        let frame = frame.upcast_ref::<gtk::Widget>();
        let (fw, fh) = (frame.width() as f64, frame.height() as f64);
        let hole = if lost { None } else { hole_of(step, frame) };
        // Only docked when the target stays missing (not while a page loads).
        let now = clock.frame_time();
        match hole {
            Some(_) => lost_since.set(None),
            None if lost_since.get().is_none() => lost_since.set(Some(now)),
            None => {}
        }
        let missing = lost_since.get().is_some_and(|t| now - t > 700_000);
        let (_, sides, _, _) = def(step);
        let ch = card.height().max(200) as f64;
        let (x, y) = match hole {
            Some(h) if !lost => card_at(h, sides, (fw, fh), ch),
            _ => (fw - CARD_W - 24.0, fh - ch - 24.0),
        };
        place(card.upcast_ref(), x, y, CARD_W, -1.0);
        card.set_visible(hole.is_some() || missing || lost);
        ring.set_visible(hole.is_some());
        if let Some((t, l, r, b)) = hole {
            place(ring.upcast_ref(), l, t, r - l, b - t);
            if let [top, bottom, left, right] = &blockers[..] {
                place(top.upcast_ref(), 0.0, 0.0, fw, t);
                place(bottom.upcast_ref(), 0.0, b, fw, fh - b);
                place(left.upcast_ref(), 0.0, t, l, b - t);
                place(right.upcast_ref(), r, t, fw - r, b - t);
            }
        }
        for b in &blockers {
            b.set_visible(hole.is_some());
        }
        glib::ControlFlow::Continue
    });
}

/// The spotlight: the target's box in the frame, padded (top, left, right, bottom).
fn hole_of(step: Step, frame: &gtk::Widget) -> Option<(f64, f64, f64, f64)> {
    let (name, ..) = def(step);
    let w = with(|s| s.targets.iter().find(|(n, _)| *n == name).and_then(|(_, w)| w.upgrade()))?;
    if !w.is_mapped() {
        return None;
    }
    let b = w.compute_bounds(frame)?;
    if b.width() < 1.0 {
        return None;
    }
    let (x, y, bw, bh) = (b.x() as f64, b.y() as f64, b.width() as f64, b.height() as f64);
    Some((y - PAD, x - PAD, x + bw + PAD, y + bh + PAD))
}

/// Where the card goes beside the hole: the first side it fits on.
fn card_at(h: (f64, f64, f64, f64), sides: &[Side], (vw, vh): (f64, f64), ch: f64) -> (f64, f64) {
    let (top, left, right, bottom) = h;
    let fits = |s: &Side| match s {
        Side::Right => right + 16.0 + CARD_W <= vw - 16.0,
        Side::Left => left - 16.0 - CARD_W >= 16.0,
        Side::Below => bottom + 16.0 + ch <= vh - 16.0,
        Side::Above => top - 16.0 - ch >= 16.0,
    };
    let clamp_y = |y: f64| y.min(vh - ch - 16.0).max(16.0);
    let clamp_x = |x: f64| x.min(vw - CARD_W - 16.0).max(16.0);
    match sides.iter().find(|s| fits(s)) {
        Some(Side::Right) => (right + 16.0, clamp_y(top)),
        Some(Side::Left) => (left - 16.0 - CARD_W, clamp_y(top)),
        Some(Side::Below) => (clamp_x(right - CARD_W), bottom + 16.0),
        Some(Side::Above) => (clamp_x(right - CARD_W), top - 16.0 - ch),
        // A big spotlight with no room beside it: docked at the right, over the dim.
        None => (vw - CARD_W - 16.0, clamp_y(top + 24.0)),
    }
}

fn card(step: Step, lost: bool) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("tour-card");
    card.update_property(&[gtk::accessible::Property::Label("Tutorial")]);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.set_margin_bottom(12);
    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    badge.add_css_class("tour-badge");
    badge.append(&Icon::new("sparkles", 12));
    let index = NUMBERED.iter().position(|s| *s == step);
    let text = match index {
        Some(i) => format!("STEP {} OF {}", i + 1, NUMBERED.len()),
        None => "WELCOME".into(),
    };
    badge.append(&gtk::Label::new(Some(&text)));
    badge.set_hexpand(false);
    head.append(&badge);
    let spring = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spring.set_hexpand(true);
    head.append(&spring);
    let x = ui::icon_button("x", "Skip tour", 16);
    x.connect_clicked(|_| finish());
    head.append(&x);
    card.append(&head);

    let skip = gtk::Button::with_label("Skip tour");
    skip.add_css_class("link");
    skip.add_css_class("tour-skip");
    skip.connect_clicked(|_| finish());
    if lost {
        card.append(&title("Continue in the Builder"));
        card.append(&body("The rest of the tour happens in the Builder."));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(20);
        skip.set_hexpand(true);
        skip.set_halign(gtk::Align::Start);
        row.append(&skip);
        let go = ui::button("Open the Builder", Some("arrow-right"), Variant::Primary, Size::Sm);
        go.connect_clicked(|_| crate::shell::go(Page::Builder));
        row.append(&go);
        card.append(&row);
        return card;
    }
    let who = crate::store::store().read_now(|s| s.profile().map(|p| p.display_name.clone())).unwrap_or("there".into());
    let (t, b, tip) = copy(step, &who);
    card.append(&title(&t));
    card.append(&body(b));
    if let Some(tip) = tip {
        let l = ui::wrapping(tip);
        l.add_css_class("tour-tip");
        card.append(&l);
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_top(20);
    for (i, _) in NUMBERED.iter().enumerate() {
        let d = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        d.add_css_class("tour-dot");
        match index {
            Some(at) if at == i => d.add_css_class("on"),
            Some(at) if i < at => d.add_css_class("done"),
            _ => {}
        }
        d.set_valign(gtk::Align::Center);
        row.append(&d);
    }
    skip.set_margin_start(6);
    skip.set_hexpand(true);
    skip.set_halign(gtk::Align::Start);
    row.append(&skip);
    let primary: Option<(&str, bool, fn())> = match step {
        Step::Nav => Some(("Open the Builder", true, || crate::shell::go(Page::Builder))),
        Step::Board => Some(("Next step", true, next)),
        Step::Create | Step::Build => {
            let done = step == Step::Create && with(|s| s.created);
            Some((if done { "Next step" } else { "Skip this step" }, done, next))
        }
        Step::Launch => None,
    };
    if let Some((label, ready, f)) = primary {
        let b =
            ui::button(label, Some("arrow-right"), if ready { Variant::Primary } else { Variant::Secondary }, Size::Sm);
        b.connect_clicked(move |_| f());
        row.append(&b);
    }
    card.append(&row);
    card
}

fn title(t: &str) -> gtk::Label {
    let l = ui::wrapping(t);
    l.add_css_class("tour-title");
    l
}

fn body(t: &str) -> gtk::Label {
    let l = ui::wrapping(t);
    l.add_css_class("tour-body");
    l.set_margin_top(6);
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_goes_beside_the_spotlight_where_it_fits() {
        let view = (1280.0, 800.0);
        // The sidebar's Builder row: to its right.
        assert_eq!(card_at((100.0, 10.0, 70.0, 140.0), &[Side::Right], view, 220.0), (86.0, 100.0));
        // The board, nearly the whole page: no room beside it, so docked.
        assert_eq!(card_at((60.0, 80.0, 1270.0, 700.0), &[Side::Below, Side::Left], view, 220.0), (924.0, 84.0));
        // The Build bucket, along the bottom: above it.
        assert_eq!(card_at((730.0, 260.0, 660.0, 790.0), &[Side::Above, Side::Right], view, 220.0), (320.0, 494.0));
    }
}
