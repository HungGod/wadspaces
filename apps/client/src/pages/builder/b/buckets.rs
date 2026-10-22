//! A wadspace on Builder B's board: its Customizations bucket (its desktop:
//! the background and projects on it, and its keys, shown or hidden) over
//! its Templates bucket (its own templates, each its apps with what they
//! take), and its name with how it stands. Both buckets are acid outside;
//! the templates in theirs are blue.
//!
//! A bucket's bar drags the bucket (the Customizations bar the Templates
//! under it too: the whole wadspace; Shift: just the bucket), a template's
//! bar it and the templates below it (Shift: just it), an app it with what
//! it takes, a chip just that. Ctrl+click picks.

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_board::b4::{self as b, Extra, Op, Payload as Bp, Pick, Template, Wadspace, Where, Which};
use wad_board::catalog::{self, KeyLine};
use wad_gtk::Icon;

use super::state::{self, Standing};
use crate::drag::{self, Mods, Payload, Target, Verdict};
use crate::pages::builder::parts::projects::project_icon;
use crate::pages::builder::parts::{self, img};
use crate::store::AppState;
use crate::ui;

/// A wadspace's column, wide.
pub const COL_W: i32 = 280;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ─────────────────────────────── what's dragged ───────────────────────────────

/// The wadspace a payload came out of (None: the sidebar's).
pub fn source(p: &Bp) -> Option<&str> {
    match p {
        Bp::Wadspace { ws }
        | Bp::Templates { ws, .. }
        | Bp::Bucket { ws, .. }
        | Bp::Project { ws, .. }
        | Bp::Background { ws }
        | Bp::App { ws, .. }
        | Bp::Extra { ws, .. } => Some(ws),
        _ => None,
    }
}

/// An app, or what one takes.
pub fn appish(p: &Bp) -> bool {
    matches!(p, Bp::App { .. } | Bp::Extra { .. } | Bp::CatalogApp { .. } | Bp::CatalogExtra { .. })
}

/// A whole wadspace: by its name, or its Customizations bar with the
/// Templates under it.
pub fn whole(p: &Bp) -> bool {
    matches!(p, Bp::Wadspace { .. } | Bp::Bucket { pile: true, .. })
}

pub fn of(p: &Payload) -> Option<&Bp> {
    match p {
        Payload::B(b) => Some(b),
        _ => None,
    }
}

fn name_of(ws: &str) -> String {
    state::read(|s| s.board.get(ws).map(|w| w.name().to_string())).unwrap_or_default()
}

/// An edit from a drop or a button: its error said.
pub fn act(op: Op) {
    if let Err(e) = state::apply(op) {
        crate::toast::error("Couldn't do that", &e);
    }
}

// ─────────────────────────────── faces ───────────────────────────────

pub fn app_name(app: &str) -> String {
    catalog::app(app).map(|a| a.name).unwrap_or_else(|| app.to_string())
}

pub fn app_icon(app: &str) -> String {
    catalog::app(app).map(|a| a.icon).unwrap_or_default()
}

/// What an extra is called, and its picture, by the app it's in.
pub fn extra_face(app: &str, x: &Extra) -> (String, String) {
    match app {
        "vscode" => {
            let e = catalog::extension(&x.id);
            (e.name, e.icon.unwrap_or_else(|| app_icon("vscode")))
        }
        "terminal" => (catalog::tool_name(&x.id), catalog::TERMINAL_ICON.into()),
        _ => (
            x.name.clone().unwrap_or_else(|| x.id.clone()),
            x.icon_url
                .clone()
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| catalog::site_icon(x.url.as_deref().unwrap_or(""))),
        ),
    }
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

/// Templates in a pile, as they'd be carried.
fn pile_ghost(names: &[String]) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("wbb-ghost-pile");
    for n in names {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("wbb-tpl-bar");
        bar.append(&Icon::new("layout-grid", 12));
        bar.append(&gtk::Label::new(Some(n)));
        b.append(&bar);
    }
    parts::ghost(&b)
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

/// A bucket's bar: a grip, its icon and name, how many, and what's on its right.
fn bucket_bar(icon: &'static str, title: &str, count: usize, right: Option<gtk::Widget>) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.add_css_class("wbb-bar");
    let grip = Icon::new("grip-vertical", 13);
    grip.add_css_class("faint");
    bar.append(&grip);
    let i = Icon::new(icon, 13);
    i.add_css_class("kc");
    bar.append(&i);
    let t = gtk::Label::new(Some(&title.to_uppercase()));
    t.add_css_class("wbb-layer");
    bar.append(&t);
    let n = gtk::Label::new(Some(&count.to_string()));
    n.add_css_class("faint");
    n.add_css_class("tiny");
    bar.append(&n);
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.set_hexpand(true);
    bar.append(&fill);
    if let Some(r) = right {
        bar.append(&r);
    }
    bar
}

// ─────────────────────────────── the column ───────────────────────────────

pub fn column(s: &AppState, w: &Wadspace) -> gtk::Widget {
    let (open, whole_picked) = state::read(|st| {
        (st.open.as_deref() == Some(w.id.as_str()), st.picked.contains(&Pick::Wadspace { ws: w.id.clone() }))
    });
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.add_css_class("wbb-col");
    if open {
        col.add_css_class("open");
    }
    if whole_picked {
        col.add_css_class("picked");
    }
    col.append(&customs(s, w));
    col.append(&templates(w));
    col.append(&name_row(s, w));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let id = w.id.clone();
    click.connect_released(move |g, n, _, _| {
        if n != 1 {
            return;
        }
        if g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            state::toggle_pick(Pick::Wadspace { ws: id.clone() });
        } else {
            super::open(&id);
        }
    });
    col.add_controller(click);
    let fixed = wad_gtk::Width::new(&col, COL_W);
    fixed.set_widget_name(&w.id);
    fixed.set_valign(gtk::Align::Start);
    fixed.upcast()
}

fn name_row(s: &AppState, w: &Wadspace) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wbb-name-row");
    let name = gtk::Label::new(Some(if w.name().trim().is_empty() { "Untitled" } else { w.name() }));
    name.add_css_class("wbb-name");
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some("Drag it onto a machine below to copy it there"));
    let (id, label) = (w.id.clone(), w.name().to_string());
    drag::draggable(&name, move |_| Some((Payload::B(Bp::Wadspace { ws: id.clone() }), ghost("", &label))));
    row.append(&name);
    row.append(&build_tag(state::standing(s, w)));
    row
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

// ─────────────────────────────── Customizations ───────────────────────────────

/// The key a line has (a group's, or its slot's).
pub fn key_of(w: &Wadspace, l: &KeyLine) -> Option<Value> {
    match &l.group {
        Some(g) => g.key.clone(),
        None => {
            w.customization.get("secrets").and_then(|x| x["bySlot"].get(&l.slot)).filter(|k| k.is_object()).cloned()
        }
    }
}

/// Its keys that are set: (picture, what signs in, the key's name).
fn key_lines(w: &Wadspace) -> Vec<(String, String, String)> {
    let map = state::read(|st| st.board.key_map(&w.id));
    let mut out = vec![];
    for a in map {
        for l in &a.lines {
            let Some(k) = key_of(w, l) else { continue };
            let what = match &l.members {
                Some(m) => m.iter().map(|x| x.name.clone()).collect::<Vec<_>>().join(", "),
                None if l.label == "Settings" => a.name.clone(),
                None => l.label.clone(),
            };
            out.push((l.icon.clone(), what, s(&k, "name").to_string()));
        }
    }
    out
}

pub fn projects_of(w: &Wadspace) -> Vec<Value> {
    w.customization.get("projects").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Its background's texture, if it has one.
pub fn texture_of(w: &Wadspace) -> Option<String> {
    let wp = w.customization.get("wallpaper")?;
    (s(wp, "type") == "texture").then(|| s(wp, "value").to_string())
}

pub fn texture_name(id: &str) -> String {
    wad_wallpaper::TEXTURES.iter().find(|(t, _)| *t == id).map(|(_, n)| n.to_string()).unwrap_or_else(|| id.into())
}

fn customs(app: &AppState, w: &Wadspace) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("wbb-bucket");
    b.add_css_class("customs");
    let lines = key_lines(w);
    let projects = projects_of(w);
    let bg = texture_of(w);
    let shown = state::read(|st| st.secrets_shown.contains(&w.id));

    // Its keys, shown or hidden.
    let toggle = gtk::Button::new();
    toggle.add_css_class("wbb-keys-toggle");
    if shown {
        toggle.add_css_class("on");
    }
    let tr = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tr.append(&Icon::new("key-round", 12));
    tr.append(&gtk::Label::new(Some(&lines.len().to_string())));
    tr.append(&Icon::new(if shown { "eye-off" } else { "eye" }, 12));
    toggle.set_child(Some(&tr));
    toggle.set_tooltip_text(Some(if shown { "Hide its secrets" } else { "Show its secrets" }));
    let id = w.id.clone();
    toggle.connect_clicked(move |_| {
        state::write(|st| match st.secrets_shown.iter().position(|x| *x == id) {
            Some(i) => drop(st.secrets_shown.remove(i)),
            None => st.secrets_shown.push(id.clone()),
        });
        state::changed();
    });
    let count = projects.len() + usize::from(bg.is_some()) + lines.len();
    let bar = bucket_bar("palette", "Customizations", count, Some(toggle.upcast()));
    bar.set_tooltip_text(Some("Drag it with the Templates under it · Shift+drag for just this bucket"));
    let (ws, name) = (w.id.clone(), w.name().to_string());
    drag::draggable(&bar, move |shift| {
        let p = Bp::Bucket { ws: ws.clone(), which: Which::Customs, pile: !shift };
        let what = if shift { format!("{name}'s Customizations") } else { name.clone() };
        Some((Payload::B(p), ghost("", &what)))
    });
    b.append(&bar);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.add_css_class("wbb-bucket-body");
    let t = crate::thumb::thumb(app, &w.doc());
    // The column's padding, the bucket's border and its body's padding in.
    t.set_fixed_width(COL_W - 16 - 2 - 16);
    t.set_radius(10.0);
    body.append(&t);

    // The background and the projects, each to drag or pick.
    let chips = gtk::FlowBox::new();
    chips.add_css_class("wbb-chips");
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_max_children_per_line(6);
    chips.set_column_spacing(4);
    chips.set_row_spacing(4);
    match &bg {
        Some(tex) => {
            let pick = Pick::Background { ws: w.id.clone() };
            let c = chip("palette", &texture_name(tex), picked(&pick));
            c.set_tooltip_text(Some("The background: drag it to another wadspace · Ctrl+click to pick"));
            let (ws, label) = (w.id.clone(), texture_name(tex));
            drag::draggable(&c, move |_| Some((Payload::B(Bp::Background { ws: ws.clone() }), ghost("", &label))));
            pickable(&c, pick);
            chips.insert(&c, -1);
        }
        None => {
            let c = chip("palette", "No background", false);
            c.add_css_class("prompt");
            chips.insert(&c, -1);
        }
    }
    for p in &projects {
        let pick = Pick::Project { ws: w.id.clone(), id: s(p, "id").into() };
        let c = chip(project_icon(p), s(p, "as"), picked(&pick));
        c.set_tooltip_text(Some(&format!("{}: drag it to another wadspace · Ctrl+click to pick", s(p, "as"))));
        let (ws, pid, label) = (w.id.clone(), s(p, "id").to_string(), s(p, "as").to_string());
        drag::draggable(&c, move |_| {
            Some((Payload::B(Bp::Project { ws: ws.clone(), id: pid.clone() }), ghost("", &label)))
        });
        pickable(&c, pick);
        chips.insert(&c, -1);
    }
    body.append(&chips);

    if shown {
        let keys = gtk::Box::new(gtk::Orientation::Vertical, 3);
        keys.add_css_class("wbb-keys");
        for (icon, what, key) in &lines {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&img(icon, 13));
            let l = gtk::Label::new(Some(what));
            l.add_css_class("muted");
            l.set_hexpand(true);
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&l);
            let pill = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            pill.add_css_class("wbb-key-pill");
            pill.append(&Icon::new("key-round", 11));
            pill.append(&gtk::Label::new(Some(key)));
            row.append(&pill);
            keys.append(&row);
        }
        if lines.is_empty() {
            let l = ui::wrapping("No secrets: open it, then Customization");
            l.add_css_class("faint");
            l.add_css_class("tiny");
            keys.append(&l);
        }
        body.append(&keys);
    }
    b.append(&body);
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

/// The Customizations bucket: a background, projects, or another
/// wadspace's Customizations dropped on it.
struct CustomsTarget {
    ws: String,
}

impl Target for CustomsTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let other = |src: &str| src != self.ws;
        match of(p)? {
            Bp::Texture { .. } => Verdict::yes("Its background"),
            Bp::CatalogProject { .. } => Verdict::yes("Add to its projects"),
            Bp::Project { ws, .. } | Bp::Background { ws } if other(ws) => {
                Verdict::yes(format!("Copy to {}", name_of(&self.ws)))
            }
            Bp::Bucket { ws, which: Which::Customs, .. } if other(ws) => {
                Verdict::yes(format!("Copy to {}", name_of(&self.ws)))
            }
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Drop { payload: b.clone(), into: self.ws.clone(), at: Where::default() });
        }
    }
}

// ─────────────────────────────── Templates ───────────────────────────────

fn templates(w: &Wadspace) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("wbb-bucket");
    b.add_css_class("templates");
    let bar = bucket_bar("layout-grid", "Templates", w.templates.len(), None);
    bar.set_tooltip_text(Some("Drag the whole bucket"));
    let (ws, name) = (w.id.clone(), w.name().to_string());
    drag::draggable(&bar, move |_| {
        let p = Bp::Bucket { ws: ws.clone(), which: Which::Templates, pile: false };
        Some((Payload::B(p), ghost("", &format!("{name}'s Templates"))))
    });
    b.append(&bar);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("wbb-bucket-body");
    body.append(&slot(&w.id, 0));
    for (i, t) in w.templates.iter().enumerate() {
        body.append(&template(w, t, i));
        body.append(&slot(&w.id, i + 1));
    }
    if w.templates.is_empty() {
        let l = gtk::Label::new(Some("No templates: open it, then drag apps in"));
        l.add_css_class("faint");
        l.add_css_class("tiny");
        l.add_css_class("italic");
        l.set_xalign(0.0);
        body.append(&l);
    }
    body.append(&new_template(&w.id));
    b.append(&body);
    drag::target(&b, TemplatesTarget { ws: w.id.clone() });
    b.upcast()
}

fn template(w: &Wadspace, t: &Template, i: usize) -> gtk::Widget {
    let open = state::read(|s| s.opened.contains(&(w.id.clone(), t.id.clone())));
    let pick = Pick::Template { ws: w.id.clone(), id: t.id.clone() };
    let m = gtk::Box::new(gtk::Orientation::Vertical, 0);
    m.add_css_class("wbb-tpl");
    if picked(&pick) {
        m.add_css_class("picked");
    }

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    bar.add_css_class("wbb-tpl-bar");
    let grip = Icon::new("grip-vertical", 12);
    grip.add_css_class("faint");
    bar.append(&grip);
    let name_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let name = gtk::Label::new(Some(&t.name));
    name.add_css_class("wbb-tpl-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some("Double-click to rename"));
    name_slot.append(&name);
    bar.append(&name_slot);
    let inside: usize = t.apps.iter().map(|e| e.extras.len()).sum();
    let meta = gtk::Label::new(Some(&format!(
        "{} app{}{}",
        t.apps.len(),
        if t.apps.len() == 1 { "" } else { "s" },
        if inside > 0 { format!(" · {inside} inside") } else { String::new() }
    )));
    meta.add_css_class("muted");
    meta.add_css_class("tiny");
    meta.set_hexpand(true);
    meta.set_xalign(0.0);
    bar.append(&meta);
    let x = ui::icon_button("x", &format!("Take {} out", t.name), 12);
    x.add_css_class("wbb-x");
    let (ws, tid) = (w.id.clone(), t.id.clone());
    x.connect_clicked(move |_| act(Op::Delete { picks: vec![Pick::Template { ws: ws.clone(), id: tid.clone() }] }));
    bar.append(&x);
    let chev = ui::icon_button(
        if open { "chevron-up" } else { "chevron-down" },
        &if open { format!("Hide what's in {}", t.name) } else { format!("Show what's in {}", t.name) },
        14,
    );
    chev.add_css_class("wbb-chev");
    let key = (w.id.clone(), t.id.clone());
    chev.connect_clicked(move |_| {
        state::write(|s| match s.opened.iter().position(|x| *x == key) {
            Some(i) => drop(s.opened.remove(i)),
            None => s.opened.push(key.clone()),
        });
        state::changed();
    });
    bar.append(&chev);
    bar.set_tooltip_text(Some(
        "Drag it, with the templates under it, to another wadspace (a copy) or another place in this one · Shift+drag for just it",
    ));
    // The templates from it down (Shift: it alone).
    let below: Vec<(String, String)> = w.templates[i..].iter().map(|x| (x.id.clone(), x.name.clone())).collect();
    let ws = w.id.clone();
    drag::draggable(&bar, move |shift| {
        let pile: Vec<(String, String)> = if shift { below[..1].to_vec() } else { below.clone() };
        let ids = pile.iter().map(|(id, _)| id.clone()).collect();
        let names: Vec<String> = pile.into_iter().map(|(_, n)| n).collect();
        Some((Payload::B(Bp::Templates { ws: ws.clone(), ids }), pile_ghost(&names)))
    });
    renamable(&name, &name_slot, &w.id, &t.id, &t.name);
    m.append(&bar);

    if open {
        let wins = gtk::Box::new(gtk::Orientation::Vertical, 5);
        wins.add_css_class("wbb-tpl-body");
        for e in &t.apps {
            let win = gtk::Box::new(gtk::Orientation::Vertical, 0);
            win.add_css_class("wbb-win");
            let wb = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            wb.add_css_class("wbb-win-bar");
            wb.append(&img(&app_icon(&e.app), 12));
            let n = gtk::Label::new(Some(&app_name(&e.app)));
            n.set_hexpand(true);
            n.set_xalign(0.0);
            wb.append(&n);
            if e.mode.as_deref() == Some("focus") {
                // The top template with the app decides its mode.
                let above =
                    w.templates.iter().take_while(|x| x.id != t.id).find(|x| x.apps.iter().any(|a| a.app == e.app));
                let tag = gtk::Label::new(Some(if above.is_some() { "focus, not used" } else { "focus" }));
                tag.add_css_class("faint");
                tag.add_css_class("tiny");
                if let Some(a) = above {
                    tag.set_tooltip_text(Some(&format!("{} is higher up, so its setting is the one used", a.name)));
                }
                wb.append(&tag);
            }
            let x = ui::icon_button("x", "Take it out", 11);
            x.add_css_class("wbb-x");
            let (ws, tid, app) = (w.id.clone(), t.id.clone(), e.app.clone());
            x.connect_clicked(move |_| {
                act(Op::RemoveApp { ws: ws.clone(), template: tid.clone(), app: app.clone() });
            });
            wb.append(&x);
            app_draggable(&wb, &w.id, &t.id, &e.app);
            win.append(&wb);
            if !e.extras.is_empty() {
                let xs = gtk::FlowBox::new();
                xs.add_css_class("wbb-extras");
                xs.set_selection_mode(gtk::SelectionMode::None);
                xs.set_max_children_per_line(4);
                xs.set_column_spacing(4);
                xs.set_row_spacing(4);
                for x in &e.extras {
                    let (label, icon) = extra_face(&e.app, x);
                    let c = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    c.add_css_class("wbb-xchip");
                    c.append(&img(&icon, 11));
                    let l = gtk::Label::new(Some(&label));
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    l.set_max_width_chars(14);
                    c.append(&l);
                    let rm = ui::icon_button("x", "Take it out", 10);
                    rm.add_css_class("wbb-x");
                    let (ws, tid, app, id) = (w.id.clone(), t.id.clone(), e.app.clone(), x.id.clone());
                    rm.connect_clicked(move |_| {
                        act(Op::RemoveExtra {
                            ws: ws.clone(),
                            template: tid.clone(),
                            app: app.clone(),
                            id: id.clone(),
                        });
                    });
                    c.append(&rm);
                    let (ws, tid, app, id) = (w.id.clone(), t.id.clone(), e.app.clone(), x.id.clone());
                    drag::draggable(&c, move |_| {
                        let p = Bp::Extra { ws: ws.clone(), template: tid.clone(), app: app.clone(), id: id.clone() };
                        Some((Payload::B(p), ghost(&icon, &label)))
                    });
                    xs.insert(&c, -1);
                }
                win.append(&xs);
            }
            wins.append(&win);
        }
        if t.apps.is_empty() {
            let l = gtk::Label::new(Some("Drag apps in from the sidebar"));
            l.add_css_class("faint");
            l.add_css_class("tiny");
            l.set_xalign(0.0);
            wins.append(&l);
        }
        m.append(&wins);
    } else {
        let icons = gtk::FlowBox::new();
        icons.add_css_class("wbb-tpl-icons");
        icons.set_selection_mode(gtk::SelectionMode::None);
        icons.set_max_children_per_line(6);
        icons.set_min_children_per_line(5);
        icons.set_column_spacing(6);
        icons.set_row_spacing(6);
        for e in &t.apps {
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
            tile.add_css_class("wbb-app");
            let o = gtk::Overlay::new();
            let i = img(&app_icon(&e.app), 24);
            i.set_halign(gtk::Align::Center);
            o.set_child(Some(&i));
            if !e.extras.is_empty() {
                let more = gtk::Label::new(Some(&format!("+{}", e.extras.len())));
                more.add_css_class("wbb-more");
                more.set_halign(gtk::Align::End);
                more.set_valign(gtk::Align::Start);
                o.add_overlay(&more);
            }
            tile.append(&o);
            let n = gtk::Label::new(Some(&app_name(&e.app)));
            n.add_css_class("wbb-app-name");
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            n.set_max_width_chars(7);
            tile.append(&n);
            tile.set_tooltip_text(Some(&format!(
                "{}: drag it (with what it takes) to another template",
                app_name(&e.app)
            )));
            app_draggable(&tile, &w.id, &t.id, &e.app);
            icons.insert(&tile, -1);
        }
        m.append(&icons);
    }
    pickable(&m, pick);
    drag::target(&m, TemplateTarget { ws: w.id.clone(), template: t.id.clone(), name: t.name.clone() });
    m.upcast()
}

fn app_draggable(w: &impl IsA<gtk::Widget>, ws: &str, template: &str, app: &str) {
    let (ws, template, app) = (ws.to_string(), template.to_string(), app.to_string());
    drag::draggable(w, move |_| {
        let p = Bp::App { ws: ws.clone(), template: template.clone(), app: app.clone() };
        Some((Payload::B(p), ghost(&app_icon(&app), &app_name(&app))))
    });
}

/// A template's name: double-click to type a new one (Enter or leaving
/// keeps it, Escape doesn't).
fn renamable(name: &gtk::Label, slot: &gtk::Box, ws: &str, template: &str, was: &str) {
    let click = gtk::GestureClick::new();
    let (ls, nw) = (slot.downgrade(), name.downgrade());
    let (ws, template, was) = (ws.to_string(), template.to_string(), was.to_string());
    click.connect_pressed(move |g, n, _, _| {
        if n != 2 {
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        let (Some(slot), Some(name)) = (ls.upgrade(), nw.upgrade()) else { return };
        rename(&name, &slot, &ws, &template, &was);
    });
    name.add_controller(click);
}

/// The name typed over: Enter keeps it; Escape, or a click anywhere else,
/// doesn't. The click that would cancel it, for the snapshots.
pub fn rename(name: &gtk::Label, slot: &gtk::Box, ws: &str, template: &str, was: &str) -> Option<gtk::GestureClick> {
    let (ws, template, was) = (ws.to_string(), template.to_string(), was.to_string());
    {
        state::write(|s| s.renaming = true);
        name.set_visible(false);
        let e = gtk::Entry::new();
        e.add_css_class("wbb-rename");
        e.set_text(&was);
        e.set_width_chars(12);
        slot.append(&e);
        e.grab_focus();
        let done = std::rc::Rc::new(std::cell::Cell::new(false));
        // A click anywhere but the name cancels: most of the board takes no
        // focus, so the entry wouldn't lose it. Gone once it's done.
        let outside: std::rc::Rc<std::cell::RefCell<Option<(gtk::Widget, gtk::GestureClick)>>> = Default::default();
        let finish = {
            let (ws, template, was, done, outside) =
                (ws.clone(), template.clone(), was.clone(), done.clone(), outside.clone());
            std::rc::Rc::new(move |e: &gtk::Entry, keep: bool| {
                if done.replace(true) {
                    return;
                }
                // Not inside the click's own handling.
                if let Some((w, c)) = outside.borrow_mut().take() {
                    glib::idle_add_local_once(move || w.remove_controller(&c));
                }
                state::write(|s| s.renaming = false);
                let to = e.text().trim().to_string();
                if keep && !to.is_empty() && to != was {
                    act(Op::RenameTemplate { ws: ws.clone(), template: template.clone(), name: to });
                } else {
                    state::changed();
                }
            })
        };
        let f = finish.clone();
        e.connect_activate(move |e| f(e, true));
        let keys = gtk::EventControllerKey::new();
        let (f, ew) = (finish.clone(), e.downgrade());
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                if let Some(e) = ew.upgrade() {
                    f(&e, false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        e.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let (f, ew) = (finish.clone(), e.downgrade());
        focus.connect_leave(move |_| {
            if let Some(e) = ew.upgrade() {
                f(&e, true);
            }
        });
        e.add_controller(focus);
        if let Some(root) = slot.root() {
            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            let (f, ew) = (finish.clone(), e.downgrade());
            click.connect_pressed(move |g, _, x, y| {
                let (Some(e), Some(w)) = (ew.upgrade(), g.widget()) else { return };
                let on_it = w.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|p| p == e || p.is_ancestor(&e));
                if !on_it {
                    f(&e, false);
                }
            });
            let root: gtk::Widget = root.upcast();
            root.add_controller(click.clone());
            outside.replace(Some((root, click.clone())));
            return Some(click);
        }
        None
    }
}

/// Between templates: templates dropped here land at that place.
fn slot(ws: &str, at: usize) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("wbb-slot");
    drag::target(&b, SlotTarget { ws: ws.into(), at });
    b
}

/// A New template field: an app (or what one takes) dropped on it makes a
/// template of its own; a click opens the sidebar's apps.
fn new_template(ws: &str) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("wbb-newtpl");
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    r.set_halign(gtk::Align::Center);
    r.append(&Icon::new("plus", 13));
    r.append(&gtk::Label::new(Some("New template")));
    b.set_child(Some(&r));
    b.set_tooltip_text(Some("Drag an app here for a template of its own"));
    b.set_widget_name(&format!("newtpl:{ws}"));
    let id = ws.to_string();
    b.connect_clicked(move |_| super::open_on(&id, state::Tab::Apps));
    drag::target(&b, NewTemplateTarget { ws: ws.into() });
    b.upcast()
}

struct TemplatesTarget {
    ws: String,
}

impl Target for TemplatesTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        match of(p)? {
            Bp::Templates { ws, ids } if *ws != self.ws => Verdict::yes(format!(
                "Copy {} to {}",
                if ids.len() == 1 { "it".to_string() } else { format!("these {}", ids.len()) },
                name_of(&self.ws)
            )),
            Bp::Bucket { ws, which: Which::Templates, .. } if *ws != self.ws => {
                Verdict::yes(format!("Copy its templates to {}", name_of(&self.ws)))
            }
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Drop { payload: b.clone(), into: self.ws.clone(), at: Where::default() });
        }
    }
}

struct SlotTarget {
    ws: String,
    at: usize,
}

impl Target for SlotTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        match of(p)? {
            Bp::Templates { ws, .. } if *ws == self.ws => Verdict::yes("Move here"),
            Bp::Templates { .. } => Verdict::yes("Copy here"),
            Bp::Bucket { ws, which: Which::Templates, .. } if *ws != self.ws => Verdict::yes("Copy its templates here"),
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Drop {
                payload: b.clone(),
                into: self.ws.clone(),
                at: Where { at: Some(self.at), ..Where::default() },
            });
        }
    }
}

struct TemplateTarget {
    ws: String,
    template: String,
    name: String,
}

impl Target for TemplateTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        match of(p)? {
            Bp::CatalogApp { .. } | Bp::CatalogExtra { .. } => Verdict::yes(format!("Add to {}", self.name)),
            Bp::App { ws, template, .. } | Bp::Extra { ws, template, .. } => {
                if *ws == self.ws && *template == self.template {
                    None
                } else if *ws == self.ws {
                    Verdict::yes(format!("Move to {}", self.name))
                } else {
                    Verdict::yes(format!("Add to {}", self.name))
                }
            }
            _ => None,
        }
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Drop {
                payload: b.clone(),
                into: self.ws.clone(),
                at: Where { template: Some(self.template.clone()), ..Where::default() },
            });
        }
    }
}

struct NewTemplateTarget {
    ws: String,
}

impl Target for NewTemplateTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        appish(of(p)?).then(|| Verdict { ok: true, why: Some("A new template with it".into()) })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Drop {
                payload: b.clone(),
                into: self.ws.clone(),
                at: Where { new_template: true, ..Where::default() },
            });
        }
    }
}

// ─────────────────────────────── the open board ───────────────────────────────

/// Between two wadspaces: what's dropped there starts a new one, of copies.
pub fn gap(machine: &str, before: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("wbb-gap");
    drag::target(&b, GapTarget { machine: machine.into(), before: Some(before.into()) });
    b
}

/// After the last wadspace: a new one, from anything dropped here, or (a
/// click) a blank one.
pub fn open_board(machine: &str) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.add_css_class("wbb-open");
    b.set_valign(gtk::Align::Start);
    let i = Icon::new("plus", 20);
    i.set_halign(gtk::Align::Center);
    b.append(&i);
    let l = gtk::Label::new(Some("Click for a new wadspace, or drop something here"));
    l.set_wrap(true);
    l.set_justify(gtk::Justification::Center);
    b.append(&l);
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let m = machine.to_string();
    click.connect_released(move |_, n, _, _| {
        if n == 1 {
            make_blank(&m, None);
        }
    });
    b.add_controller(click);
    drag::target(&b, GapTarget { machine: machine.into(), before: None });
    b.upcast()
}

// ─────────────────────────────── a blank one ───────────────────────────────

/// A blank wadspace made on `machine` (one empty template and a
/// customization with its name), with what was dropped in it, as one Undo;
/// then open in the sidebar.
fn make_blank(machine: &str, then: Option<(Bp, Where)>) {
    let id = state::read(|s| s.board.free_id(b::NEW_NAME));
    let tab = match &then {
        Some((p, _)) if !appish(p) => state::Tab::Customization,
        _ => state::Tab::Apps,
    };
    let mut ops = vec![Op::Create { machine: machine.into() }];
    if let Some((payload, at)) = then {
        ops.push(Op::Drop { payload, into: id.clone(), at });
    }
    if let Err(e) = state::apply_all(ops) {
        crate::toast::error("Couldn't make it", &e);
        return;
    }
    super::open_on(&id, tab);
}

/// A machine with no wadspaces: a blank one to fill in (an empty template
/// and an empty customization). It's made, and saved, when it's first used:
/// clicked, or an app dropped on its template, or a background or project
/// on its Customizations.
pub fn blank(machine: &str) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.add_css_class("wbb-col");
    col.add_css_class("blank");

    let customs = gtk::Box::new(gtk::Orientation::Vertical, 0);
    customs.add_css_class("wbb-bucket");
    customs.add_css_class("customs");
    customs.append(&bucket_bar("palette", "Customizations", 0, None));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.add_css_class("wbb-bucket-body");
    let desk = gtk::Box::new(gtk::Orientation::Vertical, 0);
    desk.add_css_class("wbb-blank-desk");
    desk.set_size_request(-1, 136);
    let l = ui::wrapping("Its background, projects and keys");
    l.add_css_class("faint");
    l.add_css_class("tiny");
    l.set_justify(gtk::Justification::Center);
    l.set_xalign(0.5);
    l.set_vexpand(true);
    l.set_valign(gtk::Align::Center);
    desk.append(&l);
    body.append(&desk);
    customs.append(&body);
    drag::target(&customs, BlankTarget { machine: machine.into(), template: false });
    col.append(&customs);

    let templates = gtk::Box::new(gtk::Orientation::Vertical, 0);
    templates.add_css_class("wbb-bucket");
    templates.add_css_class("templates");
    templates.append(&bucket_bar("layout-grid", "Templates", 1, None));
    let tbody = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tbody.add_css_class("wbb-bucket-body");
    let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
    t.add_css_class("wbb-tpl");
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    bar.add_css_class("wbb-tpl-bar");
    let name = gtk::Label::new(Some("Template"));
    name.add_css_class("wbb-tpl-name");
    bar.append(&name);
    let meta = gtk::Label::new(Some("0 apps"));
    meta.add_css_class("muted");
    meta.add_css_class("tiny");
    bar.append(&meta);
    t.append(&bar);
    let hint = ui::wrapping("Drag apps here from the sidebar, or click to pick them");
    hint.add_css_class("faint");
    hint.add_css_class("tiny");
    hint.set_margin_start(8);
    hint.set_margin_end(8);
    hint.set_margin_bottom(10);
    hint.set_xalign(0.0);
    t.append(&hint);
    t.set_widget_name("blank-template");
    drag::target(&t, BlankTarget { machine: machine.into(), template: true });
    tbody.append(&t);
    templates.append(&tbody);
    col.append(&templates);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wbb-name-row");
    let n = gtk::Label::new(Some(b::NEW_NAME));
    n.add_css_class("wbb-name");
    n.set_hexpand(true);
    n.set_xalign(0.0);
    row.append(&n);
    let tag = gtk::Label::new(Some("Not made yet"));
    tag.add_css_class("wbb-tag");
    tag.add_css_class("unknown");
    tag.set_tooltip_text(Some("It's made, and saved, once you put something in it"));
    row.append(&tag);
    col.append(&row);

    let click = gtk::GestureClick::new();
    click.set_button(1);
    let m = machine.to_string();
    click.connect_released(move |_, n, _, _| {
        if n == 1 {
            make_blank(&m, None);
        }
    });
    col.add_controller(click);
    col.set_tooltip_text(Some("A new wadspace: click it, or drag apps into its template"));
    let fixed = wad_gtk::Width::new(&col, COL_W);
    fixed.set_valign(gtk::Align::Start);
    fixed.upcast()
}

/// The blank wadspace's template (apps) or Customizations (a background, a
/// project, another's Customizations): what's dropped makes it.
struct BlankTarget {
    machine: String,
    template: bool,
}

impl Target for BlankTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        let ok = if self.template {
            appish(b)
        } else {
            matches!(
                b,
                Bp::Texture { .. }
                    | Bp::CatalogProject { .. }
                    | Bp::Project { .. }
                    | Bp::Background { .. }
                    | Bp::Bucket { which: Which::Customs, .. }
            )
        };
        ok.then(|| Verdict { ok: true, why: Some("A new wadspace with it".into()) })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        let Some(b) = of(p) else { return };
        let at = if self.template {
            Where { template: Some(b::BLANK_TEMPLATE.into()), ..Where::default() }
        } else {
            Where::default()
        };
        make_blank(&self.machine, Some((b.clone(), at)));
    }
}

struct GapTarget {
    machine: String,
    before: Option<String>,
}

impl Target for GapTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        let ok = source(b).is_some()
            || (self.before.is_none() && (appish(b) || matches!(b, Bp::Texture { .. } | Bp::CatalogProject { .. })));
        ok.then(|| Verdict { ok: true, why: Some("A new wadspace here".into()) })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Some(b) = of(p) {
            act(Op::Fresh { payload: b.clone(), machine: self.machine.clone(), before: self.before.clone() });
        }
    }
}
