//! The sidebar, always there, on the selected wadspace: its Apps (blue),
//! the apps' Add-ons (green: WadBrowser's web apps and its Focus, VS Code's
//! extensions, command-line tools), its Secrets (pink: a key for each thing
//! that signs in) and its Customization (white: name, description,
//! background, projects). An app it has is lit whole. A click puts an app or
//! add-on in the wadspace, or drag it onto a card; on a machine with none,
//! the first click makes one.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use serde_json::Value;
use wad_board::b::{Ext, Op, Payload as Bp, Wadspace};
use wad_board::catalog::{self, KeyApp, KeyLine};
use wad_gtk::Icon;

use super::card::{act, app_icon, app_name, ghost};
use super::state::{self, Tab};
use crate::drag::{self, Payload};
use crate::pages::builder::parts::img;
use crate::pages::builder::parts::{keys, projects};
use crate::store::AppState;
use crate::ui;

/// The sidebar, wide.
pub(super) const SIDE_W: i32 = 360;

/// What the sidebar's wadspace's parts keep while it stays the sidebar's:
/// their hosts (the parts hold them weakly) and their own state.
struct Open {
    ws: String,
    side: Rc<Side>,
    projects: Rc<projects::Projects>,
    keys: Rc<keys::Keys>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
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

/// The sidebar on `w` (None: the machine has none yet).
pub fn sidebar(s: &AppState, w: Option<&Wadspace>) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("wbb-side");
    let head = gtk::Box::new(gtk::Orientation::Vertical, 10);
    head.add_css_class("wbb-side-head");
    let title = match w {
        Some(w) if !w.name().trim().is_empty() => w.name(),
        Some(_) => "Untitled",
        None => wad_board::b::NEW_NAME,
    };
    let name = gtk::Label::new(Some(title));
    name.add_css_class("wbb-side-name");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    head.append(&name);
    let tab = state::read(|st| st.tab);
    let tabs = ui::segmented(
        &[
            ("apps", Some("layout-grid"), "Apps"),
            ("exts", Some("puzzle"), "Add-ons"),
            ("keys", Some("key-round"), "Secrets"),
            ("custom", Some("palette"), "Customization"),
        ],
        tab.part(),
        |v| {
            let tab = match v {
                "exts" => Tab::AddOns,
                "keys" => Tab::Secrets,
                "custom" => Tab::Customization,
                _ => Tab::Apps,
            };
            state::write(|st| st.tab = tab);
            state::changed();
        },
    );
    tabs.add_css_class("wbb-tabs");
    tabs.set_halign(gtk::Align::Fill);
    tabs.set_homogeneous(true);
    // Four to a sidebar: their names give way rather than widen it.
    let mut b = tabs.first_child();
    while let Some(btn) = b {
        let mut c = btn.downcast_ref::<gtk::Button>().and_then(gtk::Button::child).and_then(|r| r.first_child());
        while let Some(x) = c {
            if let Some(l) = x.downcast_ref::<gtk::Label>() {
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            }
            c = x.next_sibling();
        }
        b = btn.next_sibling();
    }
    head.append(&tabs);
    root.append(&head);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.add_css_class("wbb-side-body");
    match tab {
        Tab::Apps => apps_tab(w, &body),
        Tab::AddOns => exts_tab(s, w, &body),
        Tab::Secrets => keys_tab(w, &body),
        Tab::Customization => custom_tab(w, &body),
    }
    let key = w.map_or("#new".to_string(), |w| w.id.clone());
    root.append(&scroller(&key, tab, &body));
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
    static SCROLLER: RefCell<Option<(String, Tab, gtk::ScrolledWindow, gtk::Viewport)>> =
        const { RefCell::new(None) };
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

/// Something in the sidebar: dragged onto a card, or clicked into the
/// sidebar's wadspace.
fn item(w: &impl IsA<gtk::Widget>, p: Bp, icon: &str, label: &str) {
    let (pd, icon, label) = (p.clone(), icon.to_string(), label.to_string());
    drag::draggable(w, move |_| Some((Payload::B(pd.clone()), ghost(&icon, &label))));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |_, n, _, _| {
        if n == 1 {
            state::add_from_sidebar(p.clone());
        }
    });
    w.add_controller(click);
}

fn group(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let l = gtk::Label::new(Some(&title.to_uppercase()));
    l.add_css_class("group-label");
    l.set_xalign(0.0);
    b.append(&l);
    b
}

fn faint(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("faint");
    l.add_css_class("small");
    l
}

// ─────────────────────────────── Apps ───────────────────────────────

fn apps_tab(w: Option<&Wadspace>, body: &gtk::Box) {
    let apps = catalog::apps();
    let mut cats: Vec<String> = vec![];
    for a in &apps {
        if !cats.contains(&a.category) {
            cats.push(a.category.clone());
        }
    }
    for cat in cats {
        let section = group(&cat);
        for a in apps.iter().filter(|a| a.category == cat) {
            let has = w.is_some_and(|w| w.has_app(&a.id));
            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("wbb-app-card");
            if has {
                card.add_css_class("in");
            }
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("wbb-app-row");
            row.append(&img(&a.icon, 22));
            let n = gtk::Label::new(Some(&a.name));
            n.set_hexpand(true);
            n.set_xalign(0.0);
            row.append(&n);
            if has {
                let c = Icon::new("check", 13);
                c.add_css_class("wbb-in");
                row.append(&c);
            }
            let grip = Icon::new("grip-vertical", 13);
            grip.add_css_class("faint");
            row.append(&grip);
            row.set_tooltip_text(Some(&format!("{}: click to add · or drag onto a card", a.name)));
            item(&row, Bp::CatalogApp { app: a.id.clone() }, &a.icon, &a.name);
            card.append(&row);
            section.append(&card);
        }
        body.append(&section);
    }
}

// ─────────────────────────────── Add-ons ───────────────────────────────

/// The web apps on offer: those the board's wadspaces and saved WadBrowser
/// setups have, and a few common ones.
fn web_apps(app: &AppState) -> Vec<Ext> {
    let mut out: Vec<Ext> = vec![];
    let mut add = |x: Ext| {
        if !out.iter().any(|y| y.id == x.id) {
            out.push(x);
        }
    };
    let board: Vec<Ext> = state::read(|st| {
        st.board
            .wadspaces
            .iter()
            .flat_map(|w| w.exts.iter())
            .filter(|x| x.app == "wadbrowser" && !x.is_focus())
            .cloned()
            .collect()
    });
    board.into_iter().for_each(&mut add);
    for b in app.library.iter().flat_map(|l| l.browsers.iter()) {
        for x in b["webapps"].as_array().into_iter().flatten() {
            let web = Ext {
                name: Some(s(x, "name").into()),
                url: Some(s(x, "url").into()),
                ..Ext::plain("wadbrowser", s(x, "id"))
            };
            add(web);
        }
    }
    for (id, name, url) in catalog::COMMON_WEBAPPS {
        add(Ext { name: Some(name.into()), url: Some(url.into()), ..Ext::plain("wadbrowser", id) });
    }
    out
}

/// What each app takes, as the sidebar offers it.
fn offered(st: &AppState, app: &str) -> Vec<Ext> {
    match app {
        "vscode" => catalog::extensions().into_iter().map(|e| Ext::plain(app, &e.id)).collect(),
        "terminal" => catalog::tools().into_iter().map(|(id, _)| Ext::plain(app, &id)).collect(),
        _ => web_apps(st),
    }
}

/// An add-on on offer: lit when the wadspace has it.
fn offer(x: Ext, has: bool) -> gtk::Box {
    let (label, icon) = catalog::ext_face(&x);
    let c = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    c.add_css_class("wbb-offer-chip");
    if has {
        c.add_css_class("in");
    }
    if !icon.is_empty() {
        c.append(&img(&icon, 14));
    }
    let l = gtk::Label::new(Some(&label));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(18);
    c.append(&l);
    c.set_tooltip_text(Some(&format!("{label}: {} · click to add · or drag onto a card", catalog::ext_what(&x))));
    item(&c, Bp::CatalogExt { ext: x }, &icon, &label);
    c
}

fn exts_tab(st: &AppState, w: Option<&Wadspace>, body: &gtk::Box) {
    let has = |x: &Ext| w.is_some_and(|w| w.exts.iter().any(|y| y.same(x)));
    for app in catalog::EXT_ORDER {
        let section = group(catalog::ext_kind(app));
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("wbb-app-card");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.add_css_class("wbb-for");
        line.append(&img(&app_icon(app), 14));
        let mut text = format!("for {}", app_name(app));
        if !w.is_some_and(|w| w.has_app(app)) {
            let ws = w.map_or("it", |w| w.name());
            text.push_str(&format!(" · not in {ws} yet: it comes along"));
        }
        let l = gtk::Label::new(Some(&text));
        l.add_css_class("muted");
        l.add_css_class("tiny");
        l.set_wrap(true);
        l.set_xalign(0.0);
        line.append(&l);
        card.append(&line);
        if app == "wadbrowser" {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("wbb-focus-row");
            let focus = offer(Ext::focus(), has(&Ext::focus()));
            focus.set_tooltip_text(Some(
                "Focus: WadBrowser shows its web apps alone, no address bar · click to add · or drag onto a card",
            ));
            row.append(&focus);
            let t = gtk::Label::new(Some("its web apps alone, no address bar"));
            t.add_css_class("muted");
            t.add_css_class("tiny");
            row.append(&t);
            card.append(&row);
        }
        let chips = gtk::FlowBox::new();
        chips.add_css_class("wbb-offer");
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_max_children_per_line(4);
        chips.set_column_spacing(4);
        chips.set_row_spacing(4);
        for x in offered(st, app) {
            let on = has(&x);
            chips.insert(&offer(x, on), -1);
        }
        card.append(&chips);
        if app == "wadbrowser" {
            card.append(&new_web_app());
        }
        section.append(&card);
        body.append(&section);
    }
}

/// A site typed in, as a web app of its own.
fn new_web_app() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("wbb-new-web");
    let e = ui::entry("Add a site: notion.so");
    e.add_css_class("small");
    e.set_hexpand(true);
    row.append(&e);
    let add = ui::icon_button("plus", "Add it", 13);
    row.append(&add);
    let go = {
        let e = e.downgrade();
        move || {
            let Some(e) = e.upgrade() else { return };
            let Some(ext) = catalog::site_webapp(&e.text()) else {
                crate::toast::show(
                    "That isn't a site",
                    Some("Type its address, like notion.so"),
                    crate::toast::Tone::Info,
                );
                return;
            };
            state::add_from_sidebar(Bp::CatalogExt { ext });
        }
    };
    let g = go.clone();
    e.connect_activate(move |_| g());
    add.connect_clicked(move |_| go());
    row
}

// ─────────────────────────────── Secrets ───────────────────────────────

fn keys_tab(w: Option<&Wadspace>, body: &gtk::Box) {
    let Some(w) = w else {
        body.append(&faint("Its secrets: once it's made (click an app that signs in)."));
        return;
    };
    let (side, _, k) = open_parts(&w.id);
    let kb = gtk::Box::new(gtk::Orientation::Vertical, 10);
    kb.add_css_class("keys");
    let kh: Rc<dyn keys::Host> = side;
    keys::fill(&kh, &k, &kb);
    k.show_in(&kb);
    body.append(&kb);
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

fn custom_tab(w: Option<&Wadspace>, body: &gtk::Box) {
    let name = w.map(|w| w.name().to_string()).unwrap_or_default();
    body.append(&field("Title", &name, "Name this wadspace", move |name| {
        state::edit_selected(|ws| vec![Op::Rename { ws: ws.into(), name }]);
    }));
    let desc = w.and_then(|w| w.customization.get("description")).and_then(Value::as_str).unwrap_or_default();
    body.append(&field("Description", desc, "What it's for", move |description| {
        state::edit_selected(|ws| vec![Op::Describe { ws: ws.into(), description }]);
    }));

    let bg = group("Background");
    let current = w.and_then(|w| w.wallpaper()).cloned().unwrap_or(Value::Null);
    let picker = crate::wallpaper_picker::texture_picker(current, move |v| {
        let texture = v["value"].as_str().map(String::from);
        if let Some(ws) = state::edit_selected(|ws| vec![Op::SetBackground { ws: ws.into(), texture }]) {
            state::flash(&ws, "custom");
        }
    });
    bg.append(&picker);
    body.append(&bg);

    let proj = group("Projects");
    match w {
        Some(w) => {
            let (side, p, _) = open_parts(&w.id);
            let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
            list.add_css_class("cz-list");
            let ph: Rc<dyn projects::Host> = side;
            projects::fill(&ph, &p, &list);
            projects::load_repos_once(&ph);
            proj.append(&list);
        }
        None => proj.append(&faint("Its projects: once it's made (pick a background, or click an app).")),
    }
    body.append(&proj);
}

/// The sidebar's wadspace, as the parts edit it: each change an edit of the
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
        self.wadspace().map(|w| w.projects().to_vec()).unwrap_or_default()
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
                .flat_map(|w| w.projects().to_vec())
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
        wad_board::b::key_of(&self.wadspace()?, line)
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
            let ids: Vec<String> = state::read(|s| s.board.groups_of(&self.ws)).into_iter().map(|g| g.id).collect();
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

/// The sidebar's wadspace is gone, or another's: its parts let go.
pub fn closed() {
    OPEN.with(|o| o.replace(None));
    SCROLLER.with(|s| s.replace(None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidebar_offers_each_apps_add_ons() {
        let st = AppState::default();
        assert!(offered(&st, "vscode").iter().any(|x| x.id == "anthropic.claude-code"));
        assert!(offered(&st, "terminal").iter().any(|x| x.id == "rust"));
        let web: Vec<String> = offered(&st, "wadbrowser").into_iter().map(|x| x.id).collect();
        assert_eq!(web, ["github", "gmail", "google-drive", "claude"], "no YouTube");
        assert_eq!(app_name("vscode"), "VS Code");
        assert!(!app_icon("vscode").is_empty());
    }
}
