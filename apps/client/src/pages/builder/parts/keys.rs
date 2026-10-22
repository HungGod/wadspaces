//! A wadspace's keys (the Builder sidebar's Secrets), laid out downward:
//! each app that signs in, then a card for each thing of it that keeps a key
//! (WadBrowser: each group of its apps), what signs in on top and its key
//! underneath, full width. A click on the key lists the choices under it: a
//! new key, the keys there are of the same kind, or none. ✎ renames a key,
//! wherever it's kept and on this machine. WadBrowser's apps drag between
//! groups (Ctrl+click picks several to drag together).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::catalog::{BrowserItem, KeyApp, KeyLine};
use wad_gtk::Icon;
use wad_wadd_client::Method;

use super::{ghost, group_label, img};
use crate::drag::{self, Mods, Payload, Target, Verdict};
use crate::pages::machines::send;
use crate::signins;
use crate::store::store;
use crate::ui;

/// The wadspace whose keys these are.
pub trait Host {
    /// Its keys by app, in desktop order (wad-board's key map).
    fn key_map(&self) -> Vec<KeyApp>;
    /// The key a line has.
    fn key_of(&self, line: &KeyLine) -> Option<Value>;
    fn set_key(&self, line: &KeyLine, key: Option<Value>);
    /// WadBrowser's apps into group `to` (None: a new group of their own).
    fn regroup(&self, members: &[String], to: Option<&str>);
    /// A key renamed wherever the board keeps it, or why it can't be.
    fn rename_key(&self, id: &str, to: &str, known: &[Value]) -> Result<(), String>;
    /// The keys of a kind there are: `known` (the machine's) and the board's.
    fn keys_of_kind(&self, known: &[Value], kind: &str) -> Vec<Value>;
    /// A new key's name: <Wadspace>-<what signs in with it>.
    fn new_key_name(&self, app_id: &str) -> String;
    /// The wadspace is on this machine: a new key is made here at once.
    fn on_this_machine(&self) -> bool;
    /// A new key for the wadspace's machine, only in its document (that
    /// machine makes it when the wadspace next starts there).
    fn mint_key(&self, name: &str, kind: &str, known: &[Value]) -> Value;
}

/// The part's own state: WadBrowser's apps Ctrl+clicked to move together.
#[derive(Default)]
pub struct Keys {
    picked: RefCell<Vec<String>>,
    /// The slot whose key's choices are open.
    open: RefCell<Option<String>>,
    view: RefCell<glib::WeakRef<gtk::Box>>,
}

impl Keys {
    /// Where the keys are drawn (picking redraws just them).
    pub fn show_in(&self, b: &gtk::Box) {
        *self.view.borrow_mut() = b.downgrade();
    }

    /// Just the keys, drawn again.
    fn redraw(self: &Rc<Self>, host: &Rc<dyn Host>) {
        if let Some(b) = self.view.borrow().upgrade() {
            ui::clear(&b);
            fill(host, self, &b);
        }
    }
}

fn with(w: &Weak<dyn Host>, f: impl FnOnce(&Rc<dyn Host>)) {
    if let Some(h) = w.upgrade() {
        f(&h);
    }
}

/// The keys there are: the machine's saved sign-ins (and those the
/// library's wadspaces name), as `{id, name, kind}`.
pub fn known() -> Vec<Value> {
    store()
        .read_now(signins::known)
        .into_iter()
        .map(|(id, name, kind)| json!({ "id": id, "name": name, "kind": kind }))
        .collect()
}

fn apps_word(n: usize) -> String {
    if n == 1 { "it".into() } else { format!("these {n}") }
}

fn regroup(host: &Rc<dyn Host>, keys: &Keys, members: &[String], to: Option<&str>) {
    keys.picked.borrow_mut().clear();
    host.regroup(members, to);
}

/// The keys, by app, downward: each app that signs in, then a card for
/// each thing of it that keeps a key (WadBrowser: each group of its apps),
/// what signs in on top and its key underneath, full width.
pub fn fill(host: &Rc<dyn Host>, keys: &Rc<Keys>, b: &gtk::Box) {
    let apps = host.key_map();
    if apps.is_empty() {
        let e = gtk::Label::new(Some("Nothing in this wadspace keeps a key."));
        e.add_css_class("empty-note");
        e.set_xalign(0.0);
        b.append(&e);
        return;
    }
    for a in apps {
        let app = gtk::Box::new(gtk::Orientation::Vertical, 6);
        app.add_css_class("key-app");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("key-app-head");
        head.append(&img(&a.icon, 18));
        head.append(&gtk::Label::new(Some(&a.name)));
        app.append(&head);
        for l in &a.lines {
            let value = host.key_of(l);
            let line = gtk::Box::new(gtk::Orientation::Vertical, 6);
            line.add_css_class("key-line");
            let (w, l2) = (Rc::downgrade(host), l.clone());
            let on_set = move |k: Option<Value>| with(&w, |h| h.set_key(&l2, k));
            match (&l.group, &l.members) {
                (Some(g), Some(members)) => {
                    line.add_css_class("group");
                    let src = gtk::FlowBox::new();
                    src.add_css_class("members");
                    src.set_selection_mode(gtk::SelectionMode::None);
                    src.set_max_children_per_line(8);
                    src.set_column_spacing(4);
                    src.set_row_spacing(4);
                    for m in members {
                        src.insert(&member(host, keys, m, &g.id), -1);
                    }
                    line.append(&src);
                    line.append(&key_control(host, keys, &l.slot, "web", value.clone(), l.app_id.clone(), on_set));
                    drag::target(
                        &line,
                        GroupTarget {
                            host: Rc::downgrade(host),
                            keys: keys.clone(),
                            id: g.id.clone(),
                            members: g.members.clone(),
                            key: value.as_ref().map(|k| k["name"].as_str().unwrap_or_default().to_string()),
                        },
                    );
                }
                _ => {
                    let src = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    src.add_css_class("key-src");
                    src.append(&img(&l.icon, 16));
                    let n = gtk::Label::new(Some(&l.label));
                    n.set_wrap(true);
                    n.set_xalign(0.0);
                    src.append(&n);
                    line.append(&src);
                    line.append(&key_control(host, keys, &l.slot, &l.kind, value, l.app_id.clone(), on_set));
                }
            }
            app.append(&line);
        }
        if a.id == "wadbrowser" {
            // A key always has something on it: a group is made by dragging
            // apps into it.
            let ng = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            ng.add_css_class("key-new-group");
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            r.set_halign(gtk::Align::Center);
            r.set_hexpand(true);
            r.append(&Icon::new("plus", 13));
            r.append(&gtk::Label::new(Some("Drag apps here for a key of their own")));
            ng.append(&r);
            ng.set_tooltip_text(Some("Drag WadBrowser's apps here to give them a key of their own"));
            drag::target(&ng, NewGroupTarget { host: Rc::downgrade(host), keys: keys.clone() });
            app.append(&ng);
        }
        b.append(&app);
    }
    let n = gtk::Label::new(Some("No key: that's signed out when the wadspace stops."));
    n.add_css_class("faint");
    n.add_css_class("small");
    n.set_xalign(0.0);
    n.set_wrap(true);
    b.append(&n);
}

fn member_face(m: &BrowserItem, picked: bool, n: usize) -> gtk::Box {
    let f = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    f.add_css_class("member");
    if picked {
        f.add_css_class("picked");
    }
    f.append(&img(&m.icon, 15));
    f.append(&gtk::Label::new(Some(&m.name)));
    if n > 1 {
        let more = gtk::Label::new(Some(&format!("+{}", n - 1)));
        more.add_css_class("member-more");
        f.append(&more);
    }
    f
}

/// One of WadBrowser's apps in a key group: drag it to another group;
/// Ctrl+click picks several to drag together.
fn member(host: &Rc<dyn Host>, keys: &Rc<Keys>, m: &BrowserItem, group: &str) -> gtk::Widget {
    let on = keys.picked.borrow().contains(&m.id);
    let f = member_face(m, on, 1);
    f.set_tooltip_text(Some("Drag to another group · Ctrl+click to pick several"));
    let (k, mm, g) = (keys.clone(), m.clone(), group.to_string());
    drag::draggable(&f, move |_| {
        let picked = k.picked.borrow().clone();
        let members = if picked.contains(&mm.id) { picked } else { vec![mm.id.clone()] };
        let ghost = ghost(&member_face(&mm, true, members.len()));
        Some((Payload::Member { members, group: g.clone() }, ghost))
    });
    let click = gtk::GestureClick::new();
    let (w, k, id) = (Rc::downgrade(host), keys.clone(), m.id.clone());
    click.connect_released(move |g, _, _, _| {
        if !g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        {
            let mut p = k.picked.borrow_mut();
            if p.contains(&id) {
                p.retain(|x| *x != id);
            } else {
                p.push(id.clone());
            }
        }
        with(&w, |h| k.redraw(h));
    });
    f.add_controller(click);
    f.upcast()
}

/// A thing's key, full width: its name (whole), ✎ to rename it; a click
/// lists the choices underneath (a new key, the keys of its kind there
/// are, or none).
fn key_control(
    host: &Rc<dyn Host>,
    keys: &Rc<Keys>,
    slot: &str,
    kind: &str,
    value: Option<Value>,
    app_id: String,
    on_set: impl Fn(Option<Value>) + 'static,
) -> gtk::Widget {
    let ctl = gtk::Box::new(gtk::Orientation::Vertical, 4);
    ctl.add_css_class("key-ctl");
    let known = known();
    let id = value.as_ref().and_then(|v| v["id"].as_str()).map(String::from);
    // The machine's name for it (renamed on the Secrets page, a wadspace may keep the old one).
    let shown = id
        .as_ref()
        .and_then(|id| known.iter().find(|k| k["id"] == id.as_str()))
        .or(value.as_ref())
        .and_then(|k| k["name"].as_str())
        .unwrap_or_default()
        .to_string();
    let open = keys.open.borrow().as_deref() == Some(slot);
    let current = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    current.add_css_class("key-current");
    if value.is_some() {
        current.add_css_class("set");
    }
    if open {
        current.add_css_class("open");
    }
    let pick = gtk::Button::new();
    pick.add_css_class("key-pick");
    pick.set_hexpand(true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&Icon::new("key-round", 13));
    let name = gtk::Label::new(Some(if value.is_some() { &shown } else { "No key" }));
    name.add_css_class("key-name");
    name.set_wrap(true);
    name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    name.set_xalign(0.0);
    name.set_hexpand(true);
    row.append(&name);
    row.append(&Icon::new(if open { "chevron-up" } else { "chevron-down" }, 13));
    pick.set_child(Some(&row));
    pick.set_tooltip_text(Some(if value.is_some() { "Another key, a new one, or none" } else { "Give it a key" }));
    let (w, k, s) = (Rc::downgrade(host), keys.clone(), slot.to_string());
    pick.connect_clicked(move |_| {
        {
            let mut o = k.open.borrow_mut();
            *o = if o.as_deref() == Some(s.as_str()) { None } else { Some(s.clone()) };
        }
        with(&w, |h| k.redraw(h));
    });
    current.append(&pick);
    ctl.append(&current);
    if let Some(id) = id.clone() {
        let rename = ui::icon_button("pencil", "Rename this key (everywhere it's kept)", 13);
        rename.add_css_class("key-rename-btn");
        let (w, cw, ctlw) = (Rc::downgrade(host), current.downgrade(), ctl.downgrade());
        rename.connect_clicked(move |_| {
            if let (Some(host), Some(current), Some(ctl)) = (w.upgrade(), cw.upgrade(), ctlw.upgrade()) {
                renaming(&host, &ctl, current.upcast_ref(), &id, &shown);
            }
        });
        current.append(&rename);
    }
    if open {
        ctl.append(&choices(host, keys, kind, id.as_deref(), value.is_some(), &app_id, Rc::new(on_set)));
    }
    ctl.upcast()
}

/// The key's choices, under it: a new key, the keys of its kind there are,
/// no key.
fn choices(
    host: &Rc<dyn Host>,
    keys: &Rc<Keys>,
    kind: &str,
    id: Option<&str>,
    has: bool,
    app_id: &str,
    on_set: Rc<dyn Fn(Option<Value>)>,
) -> gtk::Box {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.add_css_class("key-choices");
    let item = |icon: &'static str, text: &str, sub: Option<&str>| {
        let b = gtk::Button::new();
        b.add_css_class("key-choice");
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        r.append(&Icon::new(icon, 13));
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        texts.append(&l);
        if let Some(x) = sub {
            let f = gtk::Label::new(Some(x));
            f.add_css_class("faint");
            f.add_css_class("small");
            f.set_xalign(0.0);
            f.set_wrap(true);
            f.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            texts.append(&f);
        }
        r.append(&texts);
        b.set_child(Some(&r));
        b
    };
    // Each choice closes them.
    let close = {
        let (w, k) = (Rc::downgrade(host), keys.clone());
        move || {
            k.open.replace(None);
            with(&w, |h| k.redraw(h));
        }
    };
    let fresh = host.new_key_name(app_id);
    let new = item("plus", "New key", Some(&fresh));
    let (os, kind_s, w, c) = (on_set.clone(), kind.to_string(), Rc::downgrade(host), close.clone());
    new.connect_clicked(move |_| {
        c();
        let (os, kind, fresh, w) = (os.clone(), kind_s.clone(), fresh.clone(), w.clone());
        glib::spawn_future_local(async move {
            match create_key(&kind, &fresh, &w).await {
                Ok(k) => os(Some(k)),
                Err(e) => crate::toast::error("Couldn't make the key", &e),
            }
        });
    });
    col.append(&new);
    let existing: Vec<Value> =
        host.keys_of_kind(&known(), kind).into_iter().filter(|k| id.is_none_or(|i| k["id"] != i)).collect();
    if !existing.is_empty() {
        let g = group_label(&format!("Your {} keys", signins::kind_label(kind)));
        g.set_margin_top(4);
        g.set_margin_start(6);
        col.append(&g);
    }
    for k in existing {
        let b = item("key-round", k["name"].as_str().unwrap_or_default(), None);
        let (os, c) = (on_set.clone(), close.clone());
        b.connect_clicked(move |_| {
            c();
            os(Some(k.clone()));
        });
        col.append(&b);
    }
    if has {
        let b = item("x", "No key", None);
        b.add_css_class("danger");
        let (os, c) = (on_set.clone(), close.clone());
        b.connect_clicked(move |_| {
            c();
            os(None);
        });
        col.append(&b);
    }
    col
}

/// A new key of a kind, named after `base` (made free): a saved sign-in on
/// this machine when the wadspace is here, else only in its document.
async fn create_key(kind: &str, base: &str, host: &Weak<dyn Host>) -> Result<Value, String> {
    let known = known();
    let Some(h) = host.upgrade() else { return Err("it's gone".into()) };
    if !h.on_this_machine() || crate::fake::on() {
        return Ok(h.mint_key(base, kind, &known));
    }
    let mut taken: Vec<String> = known.iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase).collect();
    taken.extend(h.keys_of_kind(&[], kind).iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase));
    let mut name = base.to_string();
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{base}-{n}");
        n += 1;
    }
    let made = signins::create(name, kind.to_string()).await?;
    Ok(json!({ "id": made.id, "name": made.name, "kind": kind }))
}

/// The key's name as a field: Enter saves; clicking away saves too, unless
/// the name's taken; Esc lets go.
fn renaming(host: &Rc<dyn Host>, slot: &gtk::Box, btn: &gtk::Widget, id: &str, shown: &str) {
    btn.set_visible(false);
    let edit = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    edit.add_css_class("key-edit");
    edit.append(&Icon::new("key-round", 13));
    let e = gtk::Entry::new();
    e.add_css_class("esc-local");
    e.set_text(shown);
    e.set_hexpand(true);
    e.set_width_chars(8);
    edit.append(&e);
    slot.append(&edit);
    let problem = gtk::Label::new(None);
    problem.add_css_class("name-clash");
    problem.add_css_class("small");
    problem.set_visible(false);
    slot.append(&problem);
    e.grab_focus();
    let done = Rc::new(Cell::new(false));
    let stop = {
        let (btn, edit, problem, done) = (btn.clone(), edit.clone(), problem.clone(), done.clone());
        Rc::new(move || {
            if done.replace(true) {
                return;
            }
            if let Some(p) = edit.parent().and_downcast::<gtk::Box>() {
                p.remove(&edit);
                p.remove(&problem);
            }
            btn.set_visible(true);
        })
    };
    let rename = {
        let (w, e, id, shown, stop, problem, done) = (
            Rc::downgrade(host),
            e.clone(),
            id.to_string(),
            shown.to_string(),
            stop.clone(),
            problem.clone(),
            done.clone(),
        );
        Rc::new(move |keep_on_clash: bool| {
            if done.get() {
                return;
            }
            let to = e.text().trim().to_string();
            if to.is_empty() || to == shown {
                return stop();
            }
            let Some(host) = w.upgrade() else { return stop() };
            match host.rename_key(&id, &to, &known()) {
                Err(why) if keep_on_clash => {
                    problem.set_text(&why);
                    problem.set_visible(true);
                    e.add_css_class("clash");
                }
                Err(_) => stop(),
                Ok(()) => {
                    // On this machine too, where it's one of its sign-ins.
                    let (id2, to2) = (id.clone(), to.clone());
                    let mine = store().read_now(|s| s.signins.iter().flatten().any(|x| x.id == id2));
                    glib::spawn_future_local(async move {
                        if !mine {
                            return;
                        }
                        let r = send::<Value>(Method::PUT, format!("/v1/signins/{id2}"), Some(json!({ "name": to2 })));
                        match r.await {
                            Ok(_) => signins::load(),
                            Err(e) if crate::fake::on() => tracing::debug!("rename on the machine: {e}"),
                            Err(e) => crate::toast::error("Couldn't rename it on the machine", &e),
                        }
                    });
                    stop();
                }
            }
        })
    };
    let r = rename.clone();
    e.connect_activate(move |_| r(true));
    let pw = problem.downgrade();
    e.connect_changed(move |e| {
        e.remove_css_class("clash");
        if let Some(p) = pw.upgrade() {
            p.set_visible(false);
        }
    });
    let keys = gtk::EventControllerKey::new();
    let s2 = stop.clone();
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape {
            s2();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    e.add_controller(keys);
    let focus = gtk::EventControllerFocus::new();
    let r = rename.clone();
    focus.connect_leave(move |_| {
        let r = r.clone();
        glib::idle_add_local_once(move || r(false));
    });
    e.add_controller(focus);
}

/// A WadBrowser key group: apps dropped on it join it (and share its key).
struct GroupTarget {
    host: Weak<dyn Host>,
    keys: Rc<Keys>,
    id: String,
    members: Vec<String>,
    key: Option<String>,
}

impl Target for GroupTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let Payload::Member { members, group } = p else { return None };
        if *group == self.id && members.iter().all(|m| self.members.contains(m)) {
            return None;
        }
        let n = apps_word(members.len());
        Verdict::yes(match &self.key {
            Some(k) => format!("Share {k} ({n})"),
            None => format!("Join this group ({n})"),
        })
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Payload::Member { members, .. } = p {
            with(&self.host, |h| regroup(h, &self.keys, members, Some(&self.id)));
        }
    }
}

/// New group: apps dropped on it start a group of their own.
struct NewGroupTarget {
    host: Weak<dyn Host>,
    keys: Rc<Keys>,
}

impl Target for NewGroupTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let Payload::Member { members, .. } = p else { return None };
        Verdict::yes(format!("A new group, with a key of its own ({})", apps_word(members.len())))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        if let Payload::Member { members, .. } = p {
            with(&self.host, |h| regroup(h, &self.keys, members, None));
        }
    }
}
