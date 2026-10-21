//! The open wadspace's sidebar on Builder B: its Apps (the catalog's, each
//! with what it takes: VS Code's extensions, the command line's tools,
//! WadBrowser's web apps), to drag onto a template or a New template field,
//! or click into the wadspace; and its Customization: its name and
//! description, its background (a texture), its projects and its keys (the
//! builder's parts, editing the board).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_board::b::{Extra, Op, Payload as Bp, Wadspace, Where};
use wad_board::catalog::{self, KeyApp, KeyLine};
use wad_gtk::Icon;

use super::buckets::{self, act, extra_face, ghost};
use super::state::{self, Tab};
use crate::drag::{self, Payload};
use crate::pages::builder::editor::extensions::{COMMON, site_webapp};
use crate::pages::builder::faces::img;
use crate::pages::builder::parts::{keys, projects};
use crate::store::AppState;
use crate::ui;

/// The sidebar, wide.
pub(super) const SIDE_W: i32 = 360;

/// What the open wadspace's parts keep while it stays open: their hosts
/// (the parts hold them weakly) and their own state.
struct Open {
    ws: String,
    side: Rc<Side>,
    projects: Rc<projects::Projects>,
    keys: Rc<keys::Keys>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// What the Apps tab's search has typed.
    static QUERY: RefCell<String> = const { RefCell::new(String::new()) };
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn open_parts(ws: &str) -> (Rc<Side>, Rc<projects::Projects>, Rc<keys::Keys>) {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        if o.as_ref().is_none_or(|x| x.ws != ws) {
            *o = Some(Open {
                ws: ws.into(),
                side: Rc::new(Side { ws: ws.into() }),
                projects: Rc::default(),
                keys: Rc::default(),
            });
        }
        let x = o.as_ref().expect("open");
        (x.side.clone(), x.projects.clone(), x.keys.clone())
    })
}

pub fn sidebar(s: &AppState, w: &Wadspace) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("wbb-side");
    let head = gtk::Box::new(gtk::Orientation::Vertical, 10);
    head.add_css_class("wbb-side-head");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(if w.name().trim().is_empty() { "Untitled" } else { w.name() }));
    name.add_css_class("wbb-side-name");
    name.set_hexpand(true);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    top.append(&name);
    let close = ui::icon_button("x", "Close", 14);
    close.connect_clicked(|_| super::close());
    top.append(&close);
    head.append(&top);
    let tab = state::read(|st| st.tab);
    let tabs = ui::segmented(
        &[("apps", Some("layout-grid"), "Apps"), ("custom", Some("palette"), "Customization")],
        if tab == Tab::Apps { "apps" } else { "custom" },
        |v| {
            state::write(|st| st.tab = if v == "apps" { Tab::Apps } else { Tab::Customization });
            state::changed();
        },
    );
    tabs.set_halign(gtk::Align::Fill);
    tabs.set_homogeneous(true);
    head.append(&tabs);
    root.append(&head);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.add_css_class("wbb-side-body");
    match tab {
        Tab::Apps => apps_tab(s, w, &head, &body),
        Tab::Customization => custom_tab(w, &body),
    }
    root.append(&scroller(&w.id, tab, &body));
    let side = wad_gtk::Width::new(&root, SIDE_W);
    // As tall as the page (Width doesn't take its child's expanding).
    side.set_vexpand(true);
    side.upcast()
}

thread_local! {
    /// The sidebar's scroller and its viewport, while it shows this wadspace
    /// and tab: the sidebar's drawn anew with every change (and with the
    /// library's news every half minute), its new content goes into them,
    /// and they stay where they were scrolled to.
    static SCROLLER: std::cell::RefCell<Option<(String, Tab, gtk::ScrolledWindow, gtk::Viewport)>> =
        const { std::cell::RefCell::new(None) };
}

/// `body` in the sidebar's scroller: the one it had, on the same wadspace
/// and tab, else a new one at the top.
pub(super) fn scroller(ws: &str, tab: Tab, body: &gtk::Box) -> gtk::ScrolledWindow {
    let kept = SCROLLER.with(|s| {
        s.borrow().as_ref().filter(|(w, t, ..)| w == ws && *t == tab).map(|(_, _, sw, vp)| (sw.clone(), vp.clone()))
    });
    if let Some((sw, vp)) = kept {
        if let Some(b) = sw.parent().and_downcast::<gtk::Box>() {
            b.remove(&sw);
        }
        vp.set_child(Some(body));
        return sw;
    }
    let sw = gtk::ScrolledWindow::new();
    sw.add_css_class("wbb-side-scroll");
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_vexpand(true);
    let vp = gtk::Viewport::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    vp.set_child(Some(body));
    sw.set_child(Some(&vp));
    drag::autoscroll(&sw);
    SCROLLER.with(|s| s.replace(Some((ws.to_string(), tab, sw.clone(), vp))));
    sw
}

// ─────────────────────────────── Apps ───────────────────────────────

/// What an app's extras are called in its card.
fn what(app: &str) -> &'static str {
    match app {
        "vscode" => "Extensions",
        "terminal" => "Command-line tools",
        _ => "Web apps",
    }
}

/// The web apps on offer: those the board's wadspaces and saved WadBrowser
/// setups have, and a few common ones.
fn web_apps(app: &AppState) -> Vec<Extra> {
    let mut out: Vec<Extra> = vec![];
    let mut add = |x: Extra| {
        if !out.iter().any(|y| y.id == x.id) {
            out.push(x);
        }
    };
    let board: Vec<Extra> = state::read(|st| {
        st.board
            .wadspaces
            .iter()
            .flat_map(|w| w.templates.iter())
            .flat_map(|t| t.apps.iter())
            .filter(|e| e.app == "wadbrowser")
            .flat_map(|e| e.extras.clone())
            .collect()
    });
    board.into_iter().for_each(&mut add);
    for b in app.library.iter().flat_map(|l| l.browsers.iter()) {
        for x in b["webapps"].as_array().into_iter().flatten() {
            add(Extra {
                id: s(x, "id").into(),
                name: Some(s(x, "name").into()),
                url: Some(s(x, "url").into()),
                icon_url: None,
            });
        }
    }
    for (id, name, url) in COMMON {
        add(Extra { id: id.into(), name: Some(name.into()), url: Some(url.into()), icon_url: None });
    }
    out
}

/// What each app can take, as the sidebar offers it.
fn offered(st: &AppState, app: &str) -> Vec<Extra> {
    let plain = |id: String| Extra { id, name: None, url: None, icon_url: None };
    match app {
        "vscode" => catalog::extensions().into_iter().map(|e| plain(e.id)).collect(),
        "terminal" => catalog::tools().into_iter().map(|(id, _)| plain(id)).collect(),
        "wadbrowser" => web_apps(st),
        _ => vec![],
    }
}

fn apps_tab(s: &AppState, w: &Wadspace, head: &gtk::Box, body: &gtk::Box) {
    let search = gtk::SearchEntry::new();
    search.add_css_class("wbb-search");
    search.set_placeholder_text(Some("Find an app or extension"));
    search.set_text(&QUERY.with(|q| q.borrow().clone()));
    head.append(&search);
    let hint = ui::wrapping(&format!(
        "Drag onto a template, or a bucket's New template field. A click puts it in {}.",
        w.name()
    ));
    hint.add_css_class("muted");
    hint.add_css_class("tiny");
    head.append(&hint);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.append(&list);
    let (ws, bw) = (w.id.clone(), list.downgrade());
    let fill = Rc::new(move || {
        let Some(list) = bw.upgrade() else { return };
        ui::clear(&list);
        crate::store::store().read_now(|s| fill_apps(s, &ws, &list));
    });
    fill();
    let f = fill.clone();
    search.connect_search_changed(move |e| {
        QUERY.with(|q| q.replace(e.text().to_string()));
        f();
    });
    let _ = s;
}

fn fill_apps(st: &AppState, ws: &str, list: &gtk::Box) {
    let q = QUERY.with(|q| q.borrow().trim().to_lowercase());
    let hit = |t: &str| q.is_empty() || t.to_lowercase().contains(&q);
    let has: Vec<(String, Vec<String>)> = state::read(|b| {
        b.board
            .get(ws)
            .map(|w| {
                wad_core::v4::apps_of(&w.doc())
                    .iter()
                    .map(|e| {
                        let xs = e["extras"].as_array().into_iter().flatten().map(|x| s(x, "id").to_string());
                        (s(e, "app").to_string(), xs.collect())
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    let apps = catalog::apps();
    let mut cats: Vec<String> = vec![];
    for a in &apps {
        if !cats.contains(&a.category) {
            cats.push(a.category.clone());
        }
    }
    let mut any = false;
    for cat in cats {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let h = gtk::Label::new(Some(&cat.to_uppercase()));
        h.add_css_class("group-label");
        h.set_xalign(0.0);
        section.append(&h);
        let mut shown = 0;
        for a in apps.iter().filter(|a| a.category == cat) {
            let extras: Vec<(Extra, String, String)> = offered(st, &a.id)
                .into_iter()
                .map(|x| {
                    let (name, icon) = extra_face(&a.id, &x);
                    (x, name, icon)
                })
                .filter(|(_, name, _)| hit(name) || hit(&a.name))
                .collect();
            if !hit(&a.name) && extras.is_empty() {
                continue;
            }
            shown += 1;
            let mine = has.iter().find(|(x, _)| *x == a.id);
            section.append(&app_card(ws, &a.id, &a.name, &a.icon, mine.map(|(_, xs)| xs.as_slice()), extras));
        }
        if shown > 0 {
            any = true;
            list.append(&section);
        }
    }
    if !any {
        let l = gtk::Label::new(Some("Nothing by that name."));
        l.add_css_class("faint");
        l.set_xalign(0.0);
        list.append(&l);
    }
}

/// One app: its row to drag or click in, and what it takes, each to drag or
/// click in (`has`: what the wadspace has of it, when it has it).
fn app_card(
    ws: &str,
    app: &str,
    name: &str,
    icon: &str,
    has: Option<&[String]>,
    extras: Vec<(Extra, String, String)>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("wbb-app-card");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wbb-app-row");
    if has.is_some() {
        row.add_css_class("in");
    }
    row.append(&img(icon, 22));
    let n = gtk::Label::new(Some(name));
    n.set_hexpand(true);
    n.set_xalign(0.0);
    row.append(&n);
    if has.is_some() {
        let c = Icon::new("check", 13);
        c.add_css_class("kc");
        row.append(&c);
    }
    let grip = Icon::new("grip-vertical", 13);
    grip.add_css_class("faint");
    row.append(&grip);
    row.set_tooltip_text(Some(&format!("Drag {name} onto a template · click for a template of its own")));
    let payload = Bp::CatalogApp { app: app.into() };
    item(&row, ws, payload, (icon.to_string(), name.to_string()));
    card.append(&row);

    if app == "wadbrowser" && has.is_some() {
        card.append(&focus_switch(ws));
    }
    if !extras.is_empty() || app == "wadbrowser" {
        let h = gtk::Label::new(Some(&what(app).to_uppercase()));
        h.add_css_class("wbb-what");
        h.set_xalign(0.0);
        card.append(&h);
        let chips = gtk::FlowBox::new();
        chips.add_css_class("wbb-offer");
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_max_children_per_line(4);
        chips.set_column_spacing(4);
        chips.set_row_spacing(4);
        for (x, label, icon) in extras {
            let c = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            c.add_css_class("wbb-offer-chip");
            if has.is_some_and(|h| h.contains(&x.id)) {
                c.add_css_class("in");
            }
            c.append(&img(&icon, 14));
            let l = gtk::Label::new(Some(&label));
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_max_width_chars(16);
            c.append(&l);
            item(&c, ws, Bp::CatalogExtra { app: app.into(), extra: x }, (icon, label));
            chips.insert(&c, -1);
        }
        card.append(&chips);
        if app == "wadbrowser" {
            card.append(&new_web_app(ws));
        }
    }
    card
}

/// Something in the sidebar: dragged out, or clicked into the wadspace.
fn item(w: &impl IsA<gtk::Widget>, ws: &str, p: Bp, (icon, label): (String, String)) {
    let pd = p.clone();
    drag::draggable(w, move |_| Some((Payload::B(pd.clone()), ghost(&icon, &label))));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let ws = ws.to_string();
    click.connect_released(move |_, n, _, _| {
        if n == 1 {
            act(Op::Drop { payload: p.clone(), into: ws.clone(), at: Where::default() });
        }
    });
    w.add_controller(click);
}

/// WadBrowser's mode: an address bar, or Focus (its web apps alone). The
/// top template with WadBrowser decides, so the switch shows and sets its.
fn focus_switch(ws: &str) -> gtk::Box {
    let (top, focus) = state::read(|st| {
        st.board
            .get(ws)
            .and_then(|w| {
                w.templates.iter().find_map(|t| Some((t.id.clone(), t.apps.iter().find(|e| e.app == "wadbrowser")?)))
            })
            .map(|(t, e)| (Some(t), e.mode.as_deref() == Some("focus")))
            .unwrap_or((None, false))
    });
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wbb-mode");
    let l = gtk::Label::new(Some("Focus mode: its web apps, no address bar"));
    l.add_css_class("muted");
    l.add_css_class("tiny");
    l.set_hexpand(true);
    l.set_xalign(0.0);
    l.set_wrap(true);
    row.append(&l);
    let sw = gtk::Switch::new();
    sw.set_active(focus);
    sw.set_valign(gtk::Align::Center);
    let ws = ws.to_string();
    sw.connect_state_set(move |_, on| {
        act(Op::SetMode { ws: ws.clone(), mode: if on { "focus" } else { "browser" }.into(), template: top.clone() });
        glib::Propagation::Proceed
    });
    row.append(&sw);
    row
}

/// A site typed in, as a web app of its own.
fn new_web_app(ws: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("wbb-new-web");
    let e = ui::entry("Add a site: notion.so");
    e.add_css_class("small");
    e.set_hexpand(true);
    row.append(&e);
    let add = ui::icon_button("plus", "Add it", 13);
    row.append(&add);
    let go = {
        let (ws, e) = (ws.to_string(), e.downgrade());
        move || {
            let Some(e) = e.upgrade() else { return };
            let Some(w) = site_webapp(&e.text()) else {
                crate::toast::show(
                    "That isn't a site",
                    Some("Type its address, like notion.so"),
                    crate::toast::Tone::Info,
                );
                return;
            };
            let extra = Extra { id: w.id, name: Some(w.name), url: Some(w.url), icon_url: None };
            act(Op::Drop {
                payload: Bp::CatalogExtra { app: "wadbrowser".into(), extra },
                into: ws.clone(),
                at: Where::default(),
            });
        }
    };
    let g = go.clone();
    e.connect_activate(move |_| g());
    add.connect_clicked(move |_| go());
    row
}

// ─────────────────────────────── Customization ───────────────────────────────

/// A text field that changes the board when it's done (Enter, or leaving
/// it): the board isn't drawn again while it's typed in.
fn field(label: &str, value: &str, placeholder: &str, done: impl Fn(String) + 'static) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 5);
    let l = gtk::Label::new(Some(&label.to_uppercase()));
    l.add_css_class("group-label");
    l.set_xalign(0.0);
    b.append(&l);
    let e = ui::entry(placeholder);
    e.set_text(value);
    b.append(&e);
    let done = Rc::new(done);
    let was = value.to_string();
    let commit = {
        let (done, was) = (done.clone(), was.clone());
        move |e: &gtk::Entry| {
            state::write(|s| s.renaming = false);
            let to = e.text().trim().to_string();
            if to != was {
                done(to);
            } else {
                state::changed();
            }
        }
    };
    let c = commit.clone();
    e.connect_activate(move |e| c(e));
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(|_| state::write(|s| s.renaming = true));
    let ew = e.downgrade();
    focus.connect_leave(move |_| {
        if let Some(e) = ew.upgrade() {
            commit(&e);
        }
    });
    e.add_controller(focus);
    b
}

fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let l = gtk::Label::new(Some(&title.to_uppercase()));
    l.add_css_class("group-label");
    l.set_xalign(0.0);
    b.append(&l);
    b
}

fn custom_tab(w: &Wadspace, body: &gtk::Box) {
    let ws = w.id.clone();
    body.append(&field("Title", w.name(), "Name this wadspace", move |name| act(Op::Rename { ws: ws.clone(), name })));
    let ws = w.id.clone();
    let desc = w.customization.get("description").and_then(Value::as_str).unwrap_or_default();
    body.append(&field("Description", desc, "What it's for", move |description| {
        act(Op::Describe { ws: ws.clone(), description })
    }));

    let bg = section("Background");
    let ws = w.id.clone();
    let current = w.customization.get("wallpaper").cloned().unwrap_or(Value::Null);
    let picker = crate::wallpaper_picker::texture_picker(current, move |v| {
        act(Op::SetBackground { ws: ws.clone(), texture: v["value"].as_str().map(String::from) });
    });
    bg.append(&picker);
    body.append(&bg);

    let (side, p, k) = open_parts(&w.id);
    let proj = section("Projects");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list.add_css_class("cz-list");
    let ph: Rc<dyn projects::Host> = side.clone();
    projects::fill(&ph, &p, &list);
    projects::load_repos_once(&ph);
    proj.append(&list);
    body.append(&proj);

    let sec = section("Secrets");
    let kb = gtk::Box::new(gtk::Orientation::Vertical, 10);
    kb.add_css_class("keys");
    let kh: Rc<dyn keys::Host> = side;
    keys::fill(&kh, &k, &kb);
    k.show_in(&kb);
    sec.append(&kb);
    body.append(&sec);
}

/// The open wadspace, as the parts edit it: each change an edit of the
/// board.
struct Side {
    ws: String,
}

impl Side {
    fn wadspace(&self) -> Option<Wadspace> {
        state::read(|s| s.board.get(&self.ws).cloned())
    }
}

impl projects::Host for Side {
    fn projects(&self) -> Vec<Value> {
        self.wadspace().map(|w| buckets::projects_of(&w)).unwrap_or_default()
    }

    /// What changed, as projects in and out (one Undo).
    fn set_projects(&self, list: Vec<Value>) {
        let now = self.projects();
        let id = |p: &Value| s(p, "id").to_string();
        let mut ops: Vec<Op> = now
            .iter()
            .filter(|p| !list.iter().any(|q| id(q) == id(p)))
            .map(|p| Op::ToggleProject { ws: self.ws.clone(), project: p.clone() })
            .collect();
        ops.extend(
            list.iter()
                .filter(|q| !now.iter().any(|p| id(p) == id(q)))
                .map(|q| Op::ToggleProject { ws: self.ws.clone(), project: q.clone() }),
        );
        if let Err(e) = state::apply_all(ops) {
            crate::toast::error("Couldn't change its projects", &e);
        }
    }

    fn elsewhere(&self) -> Vec<Value> {
        state::read(|s| {
            s.board
                .wadspaces
                .iter()
                .filter(|w| w.id != self.ws && !w.deleted())
                .flat_map(buckets::projects_of)
                .collect()
        })
    }

    fn redraw(&self) {
        state::changed();
    }
}

impl keys::Host for Side {
    fn key_map(&self) -> Vec<KeyApp> {
        state::read(|s| s.board.key_map(&self.ws))
    }

    fn key_of(&self, line: &KeyLine) -> Option<Value> {
        buckets::key_of(&self.wadspace()?, line)
    }

    fn set_key(&self, line: &KeyLine, key: Option<Value>) {
        let ws = self.ws.clone();
        act(match &line.group {
            Some(g) => Op::SetGroupKey { ws, group: g.id.clone(), key },
            None => Op::SetKey { ws, slot: line.slot.clone(), key },
        });
    }

    /// Moved together (one Undo): into a new group, the first makes it and
    /// the rest join it.
    fn regroup(&self, members: &[String], to: Option<&str>) {
        let Some((first, rest)) = members.split_first() else { return };
        let to = to.map(String::from);
        let fresh = to.clone().unwrap_or_else(|| {
            let ids: Vec<String> =
                state::read(|s| s.board.groups_of(&self.ws)).iter().map(|g| s(g, "id").to_string()).collect();
            (1..).map(|n| format!("g{n}")).find(|g| !ids.contains(g)).unwrap_or_default()
        });
        let mut ops = vec![Op::MoveMember { ws: self.ws.clone(), member: first.clone(), to }];
        ops.extend(rest.iter().map(|m| Op::MoveMember {
            ws: self.ws.clone(),
            member: m.clone(),
            to: Some(fresh.clone()),
        }));
        if let Err(e) = state::apply_all(ops) {
            crate::toast::error("Couldn't move them", &e);
        }
    }

    fn new_group(&self) {
        act(Op::NewGroup { ws: self.ws.clone() });
    }

    fn remove_group(&self, id: &str) {
        act(Op::RemoveGroup { ws: self.ws.clone(), group: id.into() });
    }

    fn rename_key(&self, id: &str, to: &str, known: &[Value]) -> Result<(), String> {
        if let Some(why) = state::read(|s| s.board.key_clash(id, to, known)) {
            return Err(why);
        }
        state::apply(Op::RenameKey { id: id.into(), name: to.trim().into() })
    }

    fn keys_of_kind(&self, known: &[Value], kind: &str) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        for k in known.iter().cloned().chain(state::read(|s| s.board.keys())) {
            if s(&k, "kind") != kind {
                continue;
            }
            match out.iter_mut().find(|o| s(o, "id") == s(&k, "id")) {
                Some(o) => *o = k,
                None => out.push(k),
            }
        }
        out
    }

    fn new_key_name(&self, app_id: &str) -> String {
        let name = self.wadspace().map(|w| w.name().to_string()).unwrap_or_default();
        let base: String = name.chars().filter(|c| !c.is_whitespace()).collect();
        format!("{}-{app_id}", if base.is_empty() { "Wadspace" } else { &base })
    }
}

/// What the sidebar's open on is gone, or closed: its parts let go.
pub fn closed() {
    OPEN.with(|o| o.replace(None));
    SCROLLER.with(|s| s.replace(None));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::builder::b::buckets::{app_icon, app_name};

    #[test]
    fn each_app_says_what_it_takes() {
        assert_eq!(
            (what("vscode"), what("terminal"), what("wadbrowser")),
            ("Extensions", "Command-line tools", "Web apps")
        );
        let x = Extra {
            id: "github".into(),
            name: Some("GitHub".into()),
            url: Some("https://github.com".into()),
            icon_url: None,
        };
        assert_eq!(extra_face("wadbrowser", &x).0, "GitHub");
        assert_eq!(app_name("vscode"), "VS Code");
        assert!(!app_icon("vscode").is_empty());
    }
}
