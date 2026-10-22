//! A wadspace on the Builder's board: one card, everything on it shown, its
//! name and how it stands, then its customization (white: its desktop's
//! background and projects), its secrets (pink: a title card per key, what
//! signs in with it underneath), its add-ons (green: a module for each app
//! they're for) and its apps (blue: a row of icons). A part flashes when
//! something lands in it.
//!
//! Dragging the card takes all of it; dragging a part takes just that (its
//! customization, its secrets, a key with what signs in with it, one thing
//! off a key, an app's add-ons, an add-on, its apps, an app, the
//! background, a project). A drop copies into another wadspace and selects
//! it; nothing moves around inside its own. A click selects the card
//! (Ctrl+click: more), and a click on a part opens its tab of the sidebar.
//! ✕ takes a thing out; Ctrl+click on a piece picks it, to delete.

use std::cell::{Cell, RefCell};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::b::{self, Ext, Held, KeyEntry, Op, Payload as Bp, Pick, Wadspace};
use wad_board::catalog::{self, KeyLine};
use wad_gtk::Icon;

use super::state::{self, Standing, Tab};
use crate::drag::{self, Mods, Payload, Target, Verdict};
use crate::pages::builder::parts::projects::project_icon;
use crate::pages::builder::parts::{self, img, keys};
use crate::store::{AppState, store};
use crate::ui;

/// A card, wide.
pub const CARD_W: i32 = 280;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ─────────────────────────────── what's dragged ───────────────────────────────

pub fn of(p: &Payload) -> Option<&Bp> {
    match p {
        Payload::B(b) => Some(b),
        _ => None,
    }
}

/// An app or an add-on, one of them.
pub fn appish(p: &Bp) -> bool {
    matches!(p, Bp::App { .. } | Bp::Ext { .. } | Bp::CatalogApp { .. } | Bp::CatalogExt { .. })
}

/// Add-ons: one, or an app's.
fn extish(p: &Bp) -> bool {
    matches!(p, Bp::Ext { .. } | Bp::Exts { .. } | Bp::CatalogExt { .. })
}

/// What goes in the customization.
fn customish(p: &Bp) -> bool {
    matches!(
        p,
        Bp::Customs { .. }
            | Bp::Secrets { .. }
            | Bp::Key { .. }
            | Bp::Holder { .. }
            | Bp::Project { .. }
            | Bp::Background { .. }
            | Bp::Texture { .. }
            | Bp::CatalogProject { .. }
    )
}

fn name_of(ws: &str) -> String {
    state::read(|s| s.board.get(ws).map(|w| w.name().to_string())).unwrap_or_default()
}

/// An edit from a button: its error said.
pub fn act(op: Op) {
    act_all(vec![op]);
}

pub fn act_all(ops: Vec<Op>) {
    if let Err(e) = state::apply_all(ops) {
        crate::toast::error("Couldn't do that", &e);
    }
}

/// A drop into `ws`: done (then it's the one selected, and `part` flashes),
/// or why not said.
fn drop_into(p: &Bp, ws: &str, part: Option<&str>) {
    match state::apply(Op::Drop { payload: p.clone(), into: ws.into() }) {
        Ok(()) => {
            state::select(ws, false);
            if let Some(part) = part {
                state::flash(ws, part);
            }
        }
        Err(e) => crate::toast::error("Couldn't do that", &e),
    }
}

/// What a drop of something for the customization into `ws` says.
fn custom_verdict(p: &Bp, ws: &str) -> Option<Verdict> {
    match p {
        Bp::Texture { .. } => Verdict::yes("Its background"),
        Bp::CatalogProject { .. } => Verdict::yes("Add to its projects"),
        Bp::Customs { .. } => Verdict::yes(format!("Copy its customization to {}", name_of(ws))),
        Bp::Secrets { .. } | Bp::Key { .. } | Bp::Holder { .. } | Bp::Project { .. } | Bp::Background { .. } => {
            Verdict::yes(format!("Copy to {}", name_of(ws)))
        }
        _ => None,
    }
}

// ─────────────────────────────── faces ───────────────────────────────

pub fn app_name(app: &str) -> String {
    catalog::app(app).map(|a| a.name).unwrap_or_else(|| app.to_string())
}

pub fn app_icon(app: &str) -> String {
    catalog::app(app).map(|a| a.icon).unwrap_or_default()
}

/// Its background's texture, if it has one.
pub fn texture_of(w: &Wadspace) -> Option<String> {
    let wp = w.wallpaper()?;
    (s(wp, "type") == "texture").then(|| s(wp, "value").to_string())
}

pub fn texture_name(id: &str) -> String {
    wad_wallpaper::TEXTURES.iter().find(|(t, _)| *t == id).map(|(_, n)| n.to_string()).unwrap_or_else(|| id.into())
}

/// What follows the pointer: a chip with its name.
pub fn ghost(icon: &str, text: &str) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.add_css_class("wbb-ghost");
    if !icon.is_empty() {
        b.append(&img(icon, 16));
    }
    b.append(&gtk::Label::new(Some(text)));
    parts::ghost(&b)
}

/// Makes `w` dragged as `p`, its ghost `icon` and `text`.
fn drags(w: &impl IsA<gtk::Widget>, p: Bp, icon: &str, text: &str) {
    let (icon, text) = (icon.to_string(), text.to_string());
    drag::draggable(w, move |_| Some((Payload::B(p.clone()), ghost(&icon, &text))));
}

/// Picks `p` on Ctrl+click (and nothing under it hears the click).
fn pickable(w: &impl IsA<gtk::Widget>, p: Pick) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_pressed(move |g, _, _, _| {
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            g.set_state(gtk::EventSequenceState::Claimed);
            state::toggle_pick(p.clone());
        }
    });
    w.add_controller(click);
}

fn picked(p: &Pick) -> bool {
    state::read(|s| s.picked.contains(p))
}

/// A click on a part opens its tab of the sidebar (the card's click then
/// selects the card).
fn opens(w: &impl IsA<gtk::Widget>, tab: Tab) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |g, n, _, _| {
        if n == 1 && !g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            state::write(|s| s.tab = tab);
        }
    });
    w.add_controller(click);
}

/// `w` with a ✕ in its corner (shown on hover) that takes it out.
fn with_x(w: &impl IsA<gtk::Widget>, tip: &str, remove: impl Fn() + 'static) -> gtk::Overlay {
    let o = gtk::Overlay::new();
    o.add_css_class("wbb-x-wrap");
    w.set_margin_top(5);
    w.set_margin_end(5);
    o.set_child(Some(w));
    let x = ui::icon_button("x", tip, 9);
    x.add_css_class("wbb-corner-x");
    x.set_halign(gtk::Align::End);
    x.set_valign(gtk::Align::Start);
    x.connect_clicked(move |_| remove());
    o.add_overlay(&x);
    o
}

/// Chips that wrap.
fn flow() -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_max_children_per_line(12);
    f.set_column_spacing(4);
    f.set_row_spacing(4);
    f.set_homogeneous(false);
    f.set_valign(gtk::Align::Start);
    f
}

fn empty(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("faint");
    l.add_css_class("tiny");
    l.add_css_class("italic");
    l.add_css_class("wbb-sec-empty");
    l
}

pub fn build_tag(st: Standing) -> gtk::Label {
    let (text, class) = match st {
        Standing::Built => ("Ready", "ready"),
        Standing::Update => ("Update", "update"),
        Standing::Unbuilt => ("Not built", "unbuilt"),
        Standing::Building => ("Building", "building"),
        Standing::Unknown => ("Not seen", "unknown"),
    };
    let l = gtk::Label::new(Some(text));
    l.add_css_class("wbb-tag");
    l.add_css_class(class);
    l.set_tooltip_text(Some(match st {
        Standing::Built => "Built on its machine",
        Standing::Update => "Built on an older base: it runs, and a build brings it up to date",
        Standing::Unbuilt => "Changed since it was built, or never built",
        Standing::Building => "Building now",
        Standing::Unknown => "Its machine hasn't said what it has",
    }));
    l
}

thread_local! {
    /// The last landing a part flashed for (each flashes once).
    static FLASHED: Cell<u32> = const { Cell::new(0) };
}

/// A part's head: its icon and name in its colour, how many, what's on its
/// right. It flashes when something just landed in it (`<ws>:<part>`).
fn sec_head(ws: &str, part: &str, icon: &'static str, title: &str, count: Option<usize>) -> gtk::Box {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    h.add_css_class("wbb-sec-head");
    let i = Icon::new(icon, 13);
    i.add_css_class("wbb-sec-icon");
    h.append(&i);
    let t = gtk::Label::new(Some(&title.to_uppercase()));
    t.add_css_class("wbb-sec-title");
    h.append(&t);
    if let Some(n) = count {
        let c = gtk::Label::new(Some(&n.to_string()));
        c.add_css_class("faint");
        c.add_css_class("tiny");
        h.append(&c);
    }
    let key = format!("{ws}:{part}");
    let n = state::read(|s| s.flash.as_ref().filter(|(k, _)| *k == key).map(|(_, n)| *n));
    if let Some(n) = n.filter(|n| *n > FLASHED.with(Cell::get)) {
        FLASHED.with(|f| f.set(n));
        // Lit at once, then fading (the head's transition).
        h.add_css_class("flash");
        let hw = h.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
            if let Some(h) = hw.upgrade() {
                h.remove_css_class("flash");
            }
        });
    }
    h
}

/// A card's part: a box in its colour (`class`).
fn section(class: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.add_css_class("wbb-sec");
    b.add_css_class(class);
    b
}

// ─────────────────────────────── the card ───────────────────────────────

pub fn card(s: &AppState, w: &Wadspace, selected: bool, primary: bool) -> gtk::Widget {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 10);
    c.add_css_class("wbb-card");
    if selected {
        c.add_css_class("selected");
    }
    if primary {
        c.add_css_class("primary");
    }
    if picked(&Pick::Wadspace { ws: w.id.clone() }) {
        c.add_css_class("picked");
    }
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("wbb-card-head");
    let grip = Icon::new("grip-vertical", 13);
    grip.add_css_class("faint");
    head.append(&grip);
    let name = gtk::Label::new(Some(if w.name().trim().is_empty() { "Untitled" } else { w.name() }));
    name.add_css_class("wbb-name");
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    head.append(&name);
    head.append(&build_tag(state::standing(s, w)));
    c.append(&head);
    c.append(&customs(s, w));
    c.append(&secrets(w));
    c.append(&add_ons(w));
    c.append(&apps(w));
    c.set_tooltip_text(Some("Drag the card to copy all of it: onto a machine, the open board, or another wadspace"));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let id = w.id.clone();
    click.connect_released(move |g, n, _, _| {
        if n == 1 {
            state::select(&id, g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK));
        }
    });
    c.add_controller(click);
    drags(&c, Bp::Wadspace { ws: w.id.clone() }, "", w.name());
    drag::target(&c, CardTarget { ws: w.id.clone() });
    let fixed = wad_gtk::Width::new(&c, CARD_W);
    fixed.set_widget_name(&w.id);
    fixed.set_valign(gtk::Align::Start);
    fixed.upcast()
}

/// The card: anything from elsewhere, copied in where it goes.
struct CardTarget {
    ws: String,
}

impl Target for CardTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        if b.source() == Some(self.ws.as_str()) {
            return None;
        }
        match b {
            _ if customish(b) => custom_verdict(b, &self.ws),
            Bp::Wadspace { .. } => Verdict::yes(format!("Copy all of it into {}", name_of(&self.ws))),
            Bp::Apps { .. } | Bp::Exts { .. } => Verdict::yes(format!("Copy to {}", name_of(&self.ws))),
            _ if appish(b) => Verdict::yes(format!("Add to {}", name_of(&self.ws))),
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            drop_into(b, &self.ws, None);
        }
    }
}

// ─────────────────────────────── customization ───────────────────────────────

fn customs(app: &AppState, w: &Wadspace) -> gtk::Widget {
    let b = section("customs");
    let projects = w.projects();
    let bg = texture_of(w);
    b.append(&sec_head(&w.id, "custom", "palette", "Customization", Some(projects.len() + usize::from(bg.is_some()))));
    let t = crate::thumb::thumb(app, &w.doc());
    // The card's padding, the part's border and padding in.
    t.set_fixed_width(CARD_W - 20 - 2 - 16 - 2);
    t.set_radius(10.0);
    b.append(&t);
    let chips = flow();
    chips.add_css_class("wbb-chips");
    match &bg {
        Some(tex) => {
            let pick = Pick::Background { ws: w.id.clone() };
            let c = chip("palette", &texture_name(tex), picked(&pick));
            c.set_tooltip_text(Some("The background: drag it to another wadspace · Ctrl+click to pick"));
            drags(&c, Bp::Background { ws: w.id.clone() }, "", &texture_name(tex));
            pickable(&c, pick);
            chips.insert(&c, -1);
        }
        None => {
            let c = chip("palette", "No background", false);
            c.add_css_class("prompt");
            chips.insert(&c, -1);
        }
    }
    for p in projects {
        let pick = Pick::Project { ws: w.id.clone(), id: s(p, "id").into() };
        let c = chip(project_icon(p), s(p, "as"), picked(&pick));
        c.set_tooltip_text(Some(&format!("{}: drag it to another wadspace · Ctrl+click to pick", s(p, "as"))));
        drags(&c, Bp::Project { ws: w.id.clone(), id: s(p, "id").into() }, "", s(p, "as"));
        pickable(&c, pick);
        chips.insert(&c, -1);
    }
    b.append(&chips);
    b.set_tooltip_text(Some(
        "Drag its customization (background, projects, secrets) to another wadspace · click for the sidebar's \
         Customization",
    ));
    opens(&b, Tab::Customization);
    drags(&b, Bp::Customs { ws: w.id.clone() }, "", &format!("{}'s customization", w.name()));
    drag::target(&b, CustomsTarget { ws: w.id.clone() });
    b.upcast()
}

fn chip(icon: &'static str, text: &str, picked: bool) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    c.add_css_class("wbb-chip");
    if picked {
        c.add_css_class("picked");
    }
    c.append(&Icon::new(icon, 11));
    let l = gtk::Label::new(Some(text));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(16);
    c.append(&l);
    c
}

struct CustomsTarget {
    ws: String,
}

impl Target for CustomsTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        if b.source() == Some(self.ws.as_str()) {
            return None;
        }
        custom_verdict(b, &self.ws)
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            drop_into(b, &self.ws, Some("custom"));
        }
    }
}

// ─────────────────────────────── secrets ───────────────────────────────

fn secrets(w: &Wadspace) -> gtk::Widget {
    let b = section("keys");
    let list: Vec<KeyEntry> = state::read(|st| st.board.secrets_by_key(&w.id));
    b.append(&sec_head(&w.id, "keys", "key-round", "Secrets", Some(list.len())));
    if list.is_empty() {
        b.append(&empty("No secrets: give its apps keys in the sidebar's Secrets"));
    }
    for e in &list {
        b.append(&secret(w, e));
    }
    b.append(&new_secret(&w.id));
    b.set_tooltip_text(Some(
        "Its secrets: each key, with what signs in with it · drag them (just them) to another wadspace · click for \
         the sidebar's Secrets",
    ));
    opens(&b, Tab::Secrets);
    drags(&b, Bp::Secrets { ws: w.id.clone() }, "", &format!("{}'s secrets", w.name()));
    drag::target(&b, SecretsTarget { ws: w.id.clone() });
    b.upcast()
}

/// No key for a line any more.
fn unset(ws: &str, l: &KeyLine) -> Op {
    match &l.group {
        Some(g) => Op::SetGroupKey { ws: ws.into(), group: g.id.clone(), key: None },
        None => Op::SetKey { ws: ws.into(), slot: l.slot.clone(), key: None },
    }
}

/// A key as a title card: what it's for and its name on its bar (a
/// double-click renames it), everything that signs in with it underneath.
fn secret(w: &Wadspace, e: &KeyEntry) -> gtk::Widget {
    let (kid, kname) = (s(&e.key, "id").to_string(), s(&e.key, "name").to_string());
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("wbb-cluster");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("wbb-cluster-head");
    head.append(&img(&e.icon, 14));
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.set_hexpand(true);
    let name = gtk::Label::new(Some(&kname));
    name.add_css_class("wbb-cluster-name");
    name.add_css_class("key");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some(&format!("{kname}: double-click to rename it")));
    slot.append(&name);
    let rename = gtk::GestureClick::new();
    rename.set_button(1);
    let (sw, id, was) = (slot.downgrade(), kid.clone(), kname.clone());
    rename.connect_pressed(move |g, n, _, _| {
        if n == 2
            && let Some(slot) = sw.upgrade()
        {
            g.set_state(gtk::EventSequenceState::Claimed);
            rename_key(&slot, &id, &was);
        }
    });
    name.add_controller(rename);
    head.append(&slot);
    let n = gtk::Label::new(Some(&e.holders.len().to_string()));
    n.add_css_class("faint");
    n.add_css_class("tiny");
    head.append(&n);
    let x = ui::icon_button("x", &format!("No {kname} key here any more"), 11);
    x.add_css_class("wbb-x");
    let mut clear: Vec<Op> = vec![];
    for h in &e.holders {
        let op = unset(&w.id, &h.line);
        if !clear.contains(&op) {
            clear.push(op);
        }
    }
    x.connect_clicked(move |_| act_all(clear.clone()));
    head.append(&x);
    card.append(&head);
    let body = flow();
    body.add_css_class("wbb-cluster-body");
    for h in &e.holders {
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        chip.add_css_class("wbb-addon");
        chip.append(&img(&h.icon, 13));
        let l = gtk::Label::new(Some(&h.name));
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_max_width_chars(16);
        chip.append(&l);
        let (ws, line, member) = (w.id.clone(), h.line.clone(), h.member.clone());
        let off = with_x(&chip, &format!("{} doesn't sign in with {kname}", h.name), move || {
            act(match &member {
                Some(m) => Op::MoveMember { ws: ws.clone(), member: m.clone(), to: None },
                None => unset(&ws, &line),
            })
        });
        off.set_tooltip_text(Some(&format!(
            "{}: signs in with {kname} · drag it onto New secret key for a key of its own",
            h.what
        )));
        let p = Bp::Holder { ws: w.id.clone(), key: kid.clone(), slot: h.line.slot.clone(), member: h.member.clone() };
        drags(&off, p, &h.icon, &h.name);
        body.insert(&off, -1);
    }
    card.append(&body);
    card.set_tooltip_text(Some(&format!(
        "{kname}: drag it onto New secret key for a key of their own, onto another key to sign in with that, or to \
         another wadspace · double-click its name to rename it"
    )));
    drags(&card, Bp::Key { ws: w.id.clone(), key: kid.clone() }, &e.icon, &kname);
    drag::target(&card, KeyTarget { ws: w.id.clone(), key: e.key.clone() });
    card.upcast()
}

/// A key's name as a field, in its title: Enter or clicking away renames
/// it (wherever it's kept); Esc lets go.
fn rename_key(slot: &gtk::Box, id: &str, was: &str) {
    let Some(label) = slot.first_child() else { return };
    label.set_visible(false);
    let e = gtk::Entry::new();
    e.add_css_class("wbb-key-rename");
    e.add_css_class("esc-local");
    e.set_text(was);
    e.set_hexpand(true);
    slot.append(&e);
    state::write(|s| s.renaming = true);
    e.grab_focus();
    let done = std::rc::Rc::new(Cell::new(false));
    let finish = {
        let (id, was, done) = (id.to_string(), was.to_string(), done.clone());
        move |e: &gtk::Entry, keep: bool| {
            if done.replace(true) {
                return;
            }
            state::write(|s| s.renaming = false);
            let to = e.text().trim().to_string();
            if keep
                && !to.is_empty()
                && to != was
                && let Err(why) = rename_everywhere(&id, &to)
            {
                crate::toast::error("That name's taken", &why);
            }
            state::changed();
        }
    };
    let f = finish.clone();
    e.connect_activate(move |e| f(e, true));
    let k = gtk::EventControllerKey::new();
    let (f, ew) = (finish.clone(), e.downgrade());
    k.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape
            && let Some(e) = ew.upgrade()
        {
            f(&e, false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    e.add_controller(k);
    let focus = gtk::EventControllerFocus::new();
    let ew = e.downgrade();
    focus.connect_leave(move |_| {
        if let Some(e) = ew.upgrade() {
            let f = finish.clone();
            glib::idle_add_local_once(move || f(&e, true));
        }
    });
    e.add_controller(focus);
}

/// A key renamed wherever the board keeps it, and on this machine when it's
/// one of its sign-ins; or why it can't be (another key has the name).
pub fn rename_everywhere(id: &str, to: &str) -> Result<(), String> {
    let known = keys::known();
    if let Some(why) = state::read(|s| s.board.key_clash(id, to, &known)) {
        return Err(why);
    }
    state::apply(Op::RenameKey { id: id.into(), name: to.trim().into() })?;
    let mine = store().read_now(|s| s.signins.iter().flatten().any(|x| x.id == id));
    if mine && !crate::fake::on() {
        let (id, to) = (id.to_string(), to.trim().to_string());
        glib::spawn_future_local(async move {
            let path = format!("/v1/signins/{id}");
            let r =
                crate::pages::machines::send::<Value>(wad_wadd_client::Method::PUT, path, Some(json!({ "name": to })));
            match r.await {
                Ok(_) => crate::signins::load(),
                Err(e) => crate::toast::error("Couldn't rename it on the machine", &e),
            }
        });
    }
    Ok(())
}

thread_local! {
    /// The cards' New secret key fields: shown while a key, or a thing off
    /// one, is dragged.
    static NEW_SECRETS: RefCell<Vec<glib::WeakRef<gtk::Box>>> = const { RefCell::new(Vec::new()) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn keyish(p: Option<&Payload>) -> bool {
    matches!(p.and_then(of), Some(Bp::Key { .. } | Bp::Holder { .. }))
}

/// Drop a key, or a thing off one, here for a key of its own.
fn new_secret(ws: &str) -> gtk::Box {
    if !WATCHING.with(|w| w.replace(true)) {
        drag::watch(|p| {
            let on = keyish(p);
            NEW_SECRETS.with(|n| {
                n.borrow_mut().retain(|w| w.upgrade().is_some());
                for b in n.borrow().iter().filter_map(glib::WeakRef::upgrade) {
                    b.set_visible(on);
                }
            });
        });
    }
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.add_css_class("wbb-newsecret");
    b.set_halign(gtk::Align::Fill);
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    inner.set_halign(gtk::Align::Center);
    inner.set_hexpand(true);
    inner.append(&Icon::new("plus", 13));
    inner.append(&gtk::Label::new(Some("New secret key")));
    b.append(&inner);
    b.set_visible(drag::active() && keyish(None));
    drag::target(&b, NewSecretTarget { ws: ws.into() });
    NEW_SECRETS.with(|n| n.borrow_mut().push(b.downgrade()));
    b
}

/// A new key for what's dragged, named after it and the wadspace (made
/// here when the wadspace is on this machine, else at its next start).
fn new_key_for(ws: &str, held: Held) {
    let known = keys::known();
    let (key, machine) = state::read(|st| {
        (st.board.new_key_for(ws, &held.app_id, &held.kind, &known), st.board.get(ws).map(|w| w.machine.clone()))
    });
    let here = store().read_now(state::here);
    let ws = ws.to_string();
    glib::spawn_future_local(async move {
        let key = if machine.as_deref() == Some(here.as_str()) && !crate::fake::on() {
            match crate::signins::create(s(&key, "name").into(), held.kind.clone()).await {
                Ok(made) => json!({ "id": made.id, "name": made.name, "kind": held.kind }),
                Err(e) => return crate::toast::error("Couldn't make the key", &e),
            }
        } else {
            key
        };
        match state::apply(Op::AssignKey { ws: ws.clone(), holders: held.holders, key: Some(key) }) {
            Ok(()) => {
                state::select(&ws, false);
                state::flash(&ws, "keys");
            }
            Err(e) => crate::toast::error("Couldn't do that", &e),
        }
    });
}

struct NewSecretTarget {
    ws: String,
}

impl Target for NewSecretTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        state::read(|st| st.board.holders_of(b)).map(|_| Verdict { ok: true, why: Some("A new key for it".into()) })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(held) = of(p).and_then(|b| state::read(|st| st.board.holders_of(b))) {
            new_key_for(&self.ws, held);
        }
    }
}

/// A key's card: what's dragged off a key signs in with this one (of its
/// kind only).
struct KeyTarget {
    ws: String,
    key: Value,
}

impl Target for KeyTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        let held = state::read(|st| st.board.holders_of(b))?;
        if b.source() == Some(self.ws.as_str()) && held.key == s(&self.key, "id") {
            return None;
        }
        let (name, kind) = (s(&self.key, "name"), s(&self.key, "kind"));
        if held.kind != kind {
            return Verdict::no(format!("{name} is a {} key: that doesn't sign in with it", catalog::kind_label(kind)));
        }
        Verdict::yes(format!("Sign in with {name}"))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        let Some(held) = of(p).and_then(|b| state::read(|st| st.board.holders_of(b))) else { return };
        let op = Op::AssignKey { ws: self.ws.clone(), holders: held.holders, key: Some(self.key.clone()) };
        match state::apply(op) {
            Ok(()) => {
                state::select(&self.ws, false);
                state::flash(&self.ws, "keys");
            }
            Err(e) => crate::toast::error("Couldn't do that", &e),
        }
    }
}

struct SecretsTarget {
    ws: String,
}

impl Target for SecretsTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        if b.source() == Some(self.ws.as_str()) || !matches!(b, Bp::Secrets { .. } | Bp::Key { .. } | Bp::Holder { .. })
        {
            return None;
        }
        Verdict::yes(format!("Copy to {}", name_of(&self.ws)))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            drop_into(b, &self.ws, Some("keys"));
        }
    }
}

// ─────────────────────────────── add-ons ───────────────────────────────

fn add_ons(w: &Wadspace) -> gtk::Widget {
    let b = section("exts");
    b.append(&sec_head(&w.id, "exts", "puzzle", "Add-ons", Some(w.exts.len())));
    let by_app = w.exts_by_app();
    if by_app.is_empty() {
        b.append(&empty("No add-ons: click one in the sidebar's Add-ons"));
    }
    for (app, exts) in &by_app {
        b.append(&cluster(w, app, exts));
    }
    b.set_tooltip_text(Some("Its add-ons, by the app they're for: click for the sidebar's Add-ons"));
    opens(&b, Tab::AddOns);
    drag::target(&b, AddOnsTarget { ws: w.id.clone() });
    b.upcast()
}

/// One app's add-ons, as a module: a bar with the app's icon and name, a
/// chip for each underneath; dragged together, or one by one.
fn cluster(w: &Wadspace, app: &str, exts: &[&Ext]) -> gtk::Widget {
    let pick = Pick::Exts { ws: w.id.clone(), app: app.into() };
    let what = catalog::exts_of(app);
    let orphan = !w.has_app(app);
    let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
    c.add_css_class("wbb-cluster");
    if picked(&pick) {
        c.add_css_class("picked");
    }
    if orphan {
        c.add_css_class("orphan");
    }
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("wbb-cluster-head");
    head.append(&img(&app_icon(app), 14));
    let n = gtk::Label::new(Some(&app_name(app).to_uppercase()));
    n.add_css_class("wbb-cluster-name");
    n.set_hexpand(true);
    n.set_xalign(0.0);
    head.append(&n);
    let count = gtk::Label::new(Some(&exts.len().to_string()));
    count.add_css_class("faint");
    count.add_css_class("tiny");
    head.append(&count);
    let x = ui::icon_button("x", &format!("Take {what} out"), 11);
    x.add_css_class("wbb-x");
    let p = pick.clone();
    x.connect_clicked(move |_| act(Op::Delete { picks: vec![p.clone()] }));
    head.append(&x);
    c.append(&head);
    let body = flow();
    body.add_css_class("wbb-cluster-body");
    for x in exts {
        body.insert(&add_on(&w.id, x), -1);
    }
    c.append(&body);
    c.set_tooltip_text(Some(&if orphan {
        format!("{what}: needs {}: add it for these to work", app_name(app))
    } else {
        format!("{what}: drag them together to another wadspace · Ctrl+click to pick")
    }));
    pickable(&c, pick);
    drags(&c, Bp::Exts { ws: w.id.clone(), app: app.into() }, &app_icon(app), &what);
    c.upcast()
}

/// An add-on: a chip around its own icon (when it has one) and its name;
/// what it does on hover; dragged on its own.
fn add_on(ws: &str, x: &Ext) -> gtk::Widget {
    let (name, icon) = catalog::ext_face(x);
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    chip.add_css_class("wbb-addon");
    if !icon.is_empty() {
        chip.append(&img(&icon, 14));
    }
    let l = gtk::Label::new(Some(&name));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(18);
    chip.append(&l);
    let (w, app, id) = (ws.to_string(), x.app.clone(), x.id.clone());
    let o = with_x(&chip, &format!("Take {name} out"), move || {
        act(Op::RemoveExt { ws: w.clone(), app: app.clone(), id: id.clone() })
    });
    o.set_tooltip_text(Some(&format!("{name}: {} · drag it to another wadspace", catalog::ext_what(x))));
    drags(&o, Bp::Ext { ws: ws.into(), app: x.app.clone(), id: x.id.clone() }, &icon, &name);
    o.upcast()
}

struct AddOnsTarget {
    ws: String,
}

impl Target for AddOnsTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        if !extish(b) || b.source() == Some(self.ws.as_str()) {
            return None;
        }
        Verdict::yes(format!("Add to {}", name_of(&self.ws)))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            drop_into(b, &self.ws, Some("exts"));
        }
    }
}

// ─────────────────────────────── apps ───────────────────────────────

/// Its apps, together: a row of icons, each dragged on its own (the part:
/// all of them).
fn apps(w: &Wadspace) -> gtk::Widget {
    let b = section("apps");
    b.append(&sec_head(&w.id, "apps", "layout-grid", "Apps", Some(w.apps.len())));
    if w.apps.is_empty() {
        b.append(&empty("No apps: click one in the sidebar's Apps"));
    } else {
        let row = flow();
        row.add_css_class("wbb-app-icons");
        for a in &w.apps {
            let (name, icon) = (app_name(&a.app), app_icon(&a.app));
            let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            tile.add_css_class("wbb-tile");
            let i = img(&icon, 20);
            i.set_halign(gtk::Align::Center);
            i.set_valign(gtk::Align::Center);
            i.set_hexpand(true);
            tile.append(&i);
            tile.set_size_request(30, 30);
            let (ws, app) = (w.id.clone(), a.app.clone());
            let o = with_x(&tile, &format!("Take {name} out"), move || {
                act(Op::RemoveApp { ws: ws.clone(), app: app.clone() })
            });
            o.set_tooltip_text(Some(&format!("{name}: drag it to another wadspace")));
            drags(&o, Bp::App { ws: w.id.clone(), app: a.app.clone() }, &icon, &name);
            row.insert(&o, -1);
        }
        b.append(&row);
        let first = app_icon(&w.apps[0].app);
        drags(&b, Bp::Apps { ws: w.id.clone() }, &first, &format!("{}'s apps", w.name()));
    }
    b.set_tooltip_text(Some("Its apps: drag them (just them) to another wadspace · click for the sidebar's Apps"));
    opens(&b, Tab::Apps);
    drag::target(&b, AppsTarget { ws: w.id.clone() });
    b.upcast()
}

struct AppsTarget {
    ws: String,
}

impl Target for AppsTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        if b.source() == Some(self.ws.as_str()) {
            return None;
        }
        match b {
            Bp::Apps { .. } => Verdict::yes(format!("Copy to {}", name_of(&self.ws))),
            Bp::App { .. } | Bp::CatalogApp { .. } => Verdict::yes(format!("Add to {}", name_of(&self.ws))),
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            drop_into(b, &self.ws, Some("apps"));
        }
    }
}

// ─────────────────────────────── the open board ───────────────────────────────

/// Between two wadspaces, or (`before` None) all the room after the last
/// one: what's dropped there starts a new wadspace, selected.
pub fn gap(machine: &str, before: Option<&str>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("wbb-gap");
    if before.is_none() {
        b.add_css_class("end");
        b.set_hexpand(true);
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        strip.add_css_class("wbb-gap-strip");
        b.append(&strip);
    }
    drag::target(&b, GapTarget { machine: machine.into(), before: before.map(String::from) });
    b
}

/// A new wadspace on `machine` from what's dragged (before `before`): made,
/// then selected.
fn fresh(p: &Bp, machine: &str, before: Option<String>) {
    let had: Vec<String> = state::read(|s| s.board.wadspaces.iter().map(|w| w.id.clone()).collect());
    if let Err(e) = state::apply(Op::Fresh { payload: p.clone(), machine: machine.into(), before }) {
        return crate::toast::error("Couldn't do that", &e);
    }
    if let Some(made) = state::read(|s| s.board.wadspaces.iter().find(|w| !had.contains(&w.id)).map(|w| w.id.clone())) {
        state::select(&made, false);
    }
}

struct GapTarget {
    machine: String,
    before: Option<String>,
}

impl Target for GapTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        let ok = b.source().is_some()
            || (self.before.is_none() && (appish(b) || matches!(b, Bp::Texture { .. } | Bp::CatalogProject { .. })));
        let why = if matches!(b, Bp::Wadspace { .. }) { "A copy of it here" } else { "A new wadspace with it" };
        ok.then(|| Verdict { ok: true, why: Some(why.into()) })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            fresh(b, &self.machine, self.before.clone());
        }
    }
}

// ─────────────────────────────── an empty one ───────────────────────────────

/// A new empty wadspace on `machine`, with what was dropped in it (one
/// Undo), selected.
fn make_blank(machine: &str, then: Option<Bp>) {
    let id = state::read(|s| s.board.free_id(&s.board.new_name(machine)));
    let mut ops = vec![Op::Create { machine: machine.into() }];
    if let Some(payload) = then {
        ops.push(Op::Drop { payload, into: id.clone() });
    }
    match state::apply_all(ops) {
        Ok(()) => state::select(&id, false),
        Err(e) => crate::toast::error("Couldn't make it", &e),
    }
}

/// An empty part of the blank card: what goes there, its tab on a click.
fn blank_sec(class: &str, icon: &'static str, title: &str, text: &str, tab: Tab) -> gtk::Box {
    let b = section(class);
    b.append(&sec_head("", class, icon, title, Some(0)));
    b.append(&empty(text));
    opens(&b, tab);
    b
}

/// A machine with no wadspaces: an empty one to build from, made (and
/// saved) once something's put in it.
pub fn blank(machine: &str) -> gtk::Widget {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 10);
    c.add_css_class("wbb-card");
    c.add_css_class("selected");
    c.add_css_class("primary");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("wbb-card-head");
    let n = gtk::Label::new(Some(b::NEW_NAME));
    n.add_css_class("wbb-name");
    n.set_xalign(0.0);
    n.set_hexpand(true);
    head.append(&n);
    let tag = gtk::Label::new(Some("Empty"));
    tag.add_css_class("wbb-tag");
    tag.add_css_class("unknown");
    tag.set_tooltip_text(Some("Nothing in it yet"));
    head.append(&tag);
    c.append(&head);
    let customs = section("customs");
    customs.append(&sec_head("", "custom", "palette", "Customization", Some(0)));
    let desk = gtk::Box::new(gtk::Orientation::Vertical, 0);
    desk.add_css_class("wbb-blank-desk");
    desk.set_size_request(-1, 135);
    let l = gtk::Label::new(Some("Its background and projects"));
    l.add_css_class("faint");
    l.add_css_class("tiny");
    l.set_vexpand(true);
    l.set_valign(gtk::Align::Center);
    desk.append(&l);
    customs.append(&desk);
    opens(&customs, Tab::Customization);
    c.append(&customs);
    c.append(&blank_sec("keys", "key-round", "Secrets", "No secrets: they come with apps that sign in", Tab::Secrets));
    c.append(&blank_sec("exts", "puzzle", "Add-ons", "No add-ons: click one in the sidebar's Add-ons", Tab::AddOns));
    c.append(&blank_sec("apps", "layout-grid", "Apps", "No apps: click one in the sidebar's Apps", Tab::Apps));
    c.set_tooltip_text(Some(
        "An empty wadspace: add apps, add-ons or a background from the sidebar, or drop something here",
    ));
    c.set_widget_name("blank-card");
    drag::target(&c, BlankTarget { machine: machine.into() });
    let fixed = wad_gtk::Width::new(&c, CARD_W);
    fixed.set_valign(gtk::Align::Start);
    fixed.upcast()
}

/// The blank card: anything dropped makes it.
struct BlankTarget {
    machine: String,
}

impl Target for BlankTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let why = if matches!(of(p)?, Bp::Wadspace { .. }) { "A copy of it here" } else { "A new wadspace with it" };
        Verdict::yes(why)
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        match of(p) {
            Some(b @ Bp::Wadspace { .. }) => fresh(b, &self.machine, None),
            Some(b) => make_blank(&self.machine, Some(b.clone())),
            None => {}
        }
    }
}
