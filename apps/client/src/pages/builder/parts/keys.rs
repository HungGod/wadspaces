//! A wadspace's keys (the Customization editor's Secrets, and Builder B's
//! sidebar's): one per app that signs in, or per group of WadBrowser's apps,
//! new or picked from the keys there are of the same kind. A key's name
//! renames it, wherever it's kept and on this machine.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_board::catalog::{BrowserItem, KeyApp, KeyLine};
use wad_gtk::Icon;
use wad_wadd_client::Method;

use super::{ghost, group_label, img, menu_opened};
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
    fn new_group(&self);
    fn remove_group(&self, id: &str);
    /// A key renamed wherever the board keeps it, or why it can't be.
    fn rename_key(&self, id: &str, to: &str, known: &[Value]) -> Result<(), String>;
    /// The keys of a kind there are: `known` (the machine's) and the board's.
    fn keys_of_kind(&self, known: &[Value], kind: &str) -> Vec<Value>;
    /// A new key's name: <Wadspace>-<what signs in with it>.
    fn new_key_name(&self, app_id: &str) -> String;
}

/// The part's own state: WadBrowser's apps Ctrl+clicked to move together.
#[derive(Default)]
pub struct Keys {
    picked: RefCell<Vec<String>>,
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
fn known() -> Vec<Value> {
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

fn wire(on: bool) -> gtk::Box {
    let w = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    w.add_css_class("wire");
    if on {
        w.add_css_class("on");
    }
    w.set_hexpand(true);
    w.set_valign(gtk::Align::Center);
    w
}

/// The keys, by app: each line what signs in, a wire, and its key.
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
        let app = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        app.add_css_class("key-app");
        let head = gtk::Box::new(gtk::Orientation::Vertical, 5);
        head.add_css_class("key-app-head");
        let i = img(&a.icon, 34);
        i.set_halign(gtk::Align::Center);
        head.append(&i);
        let n = gtk::Label::new(Some(&a.name));
        n.set_wrap(true);
        n.set_justify(gtk::Justification::Center);
        head.append(&n);
        app.append(&wad_gtk::Width::new(&head, 96));
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 6);
        lines.add_css_class("key-lines");
        lines.set_hexpand(true);
        for l in &a.lines {
            let value = host.key_of(l);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.add_css_class("key-line");
            let (w, l2) = (Rc::downgrade(host), l.clone());
            let on_set = move |k: Option<Value>| with(&w, |h| h.set_key(&l2, k));
            if let (Some(g), Some(members)) = (&l.group, &l.members) {
                line.add_css_class("group");
                let src = gtk::FlowBox::new();
                src.add_css_class("members");
                src.set_selection_mode(gtk::SelectionMode::None);
                src.set_max_children_per_line(8);
                src.set_column_spacing(4);
                src.set_row_spacing(4);
                src.set_hexpand(false);
                src.set_valign(gtk::Align::Center);
                for m in members {
                    src.insert(&member(host, keys, m, &g.id), -1);
                }
                if members.is_empty() {
                    line.add_css_class("empty");
                    let h = gtk::Label::new(Some("Drag apps here"));
                    h.add_css_class("drop-here");
                    src.insert(&h, -1);
                }
                line.append(&src);
                line.append(&wire(value.is_some()));
                line.append(&key_picker(host, "web", value.clone(), l.app_id.clone(), on_set));
                if members.is_empty() {
                    let x = ui::icon_button("x", "Remove this empty group", 13);
                    let (w, id) = (Rc::downgrade(host), g.id.clone());
                    x.connect_clicked(move |_| with(&w, |h| h.remove_group(&id)));
                    line.append(&x);
                }
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
            } else {
                let src = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                src.add_css_class("key-src");
                src.append(&img(&l.icon, 18));
                src.append(&gtk::Label::new(Some(&l.label)));
                line.append(&src);
                line.append(&wire(value.is_some()));
                line.append(&key_picker(host, &l.kind, value, l.app_id.clone(), on_set));
            }
            lines.append(&line);
        }
        if a.id == "wadbrowser" {
            let ng = gtk::Button::new();
            ng.add_css_class("key-line");
            ng.add_css_class("new-group");
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            r.set_halign(gtk::Align::Center);
            r.append(&Icon::new("plus", 13));
            r.append(&gtk::Label::new(Some("New group")));
            ng.set_child(Some(&r));
            let w = Rc::downgrade(host);
            ng.connect_clicked(move |_| with(&w, |h| h.new_group()));
            drag::target(&ng, NewGroupTarget { host: Rc::downgrade(host), keys: keys.clone() });
            lines.append(&ng);
        }
        app.append(&lines);
        b.append(&app);
    }
    let n = gtk::Label::new(Some("No key: that app is signed out when the wadspace stops."));
    n.add_css_class("faint");
    n.add_css_class("small");
    n.set_xalign(0.0);
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

/// A key: "No key", or its name. Click it for a new key or one of the same
/// kind; click its name to rename it (everywhere it's kept).
fn key_picker(
    host: &Rc<dyn Host>,
    kind: &str,
    value: Option<Value>,
    app_id: String,
    on_set: impl Fn(Option<Value>) + 'static,
) -> gtk::Widget {
    let slot = gtk::Box::new(gtk::Orientation::Vertical, 2);
    slot.add_css_class("key-pick");
    slot.set_valign(gtk::Align::Center);
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
    let btn = gtk::MenuButton::new();
    btn.add_css_class("key-btn");
    if value.is_some() {
        btn.add_css_class("set");
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&Icon::new("key-round", 13));
    let name = gtk::Label::new(Some(if value.is_some() { &shown } else { "No key" }));
    name.add_css_class("secret-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(22);
    row.append(&name);
    btn.set_child(Some(&row));
    btn.set_always_show_arrow(false);
    btn.set_tooltip_text(Some(&if value.is_some() {
        format!("{} · click the name to rename it, the key for more", signins::kind_label(kind))
    } else {
        signins::kind_label(kind)
    }));
    let on_set: Rc<dyn Fn(Option<Value>)> = Rc::new(on_set);

    // The menu: a new key, the keys there are of this kind, no key (made
    // as it opens: names and keys as they are then).
    let pop = gtk::Popover::new();
    pop.add_css_class("wad-menu");
    pop.add_css_class("key-menu");
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    let (w, kind_s, id2, has) = (Rc::downgrade(host), kind.to_string(), id.clone(), value.is_some());
    pop.connect_show(move |p| {
        menu_opened(p);
        with(&w, |h| fill_menu(p, h, &kind_s, id2.as_deref(), has, &app_id, &on_set));
    });
    btn.set_popover(Some(&pop));
    slot.append(&btn);

    // Its name renames it, in place.
    if let Some(id) = id {
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (w, sw, bw) = (Rc::downgrade(host), slot.downgrade(), btn.downgrade());
        let nw = name.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(host), Some(slot), Some(btn), Some(name)) =
                (w.upgrade(), sw.upgrade(), bw.upgrade(), nw.upgrade())
            else {
                return;
            };
            // Only on the name (the key icon opens the menu).
            let on_name = btn
                .compute_point(&name, &gtk::graphene::Point::new(x as f32, y as f32))
                .is_some_and(|p| name.contains(p.x() as f64, p.y() as f64));
            if !on_name {
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            renaming(&host, &slot, &btn, &id, &shown);
        });
        btn.add_controller(click);
    }
    slot.upcast()
}

/// The key menu's choices, as they are now.
fn fill_menu(
    pop: &gtk::Popover,
    host: &Rc<dyn Host>,
    kind: &str,
    id: Option<&str>,
    has: bool,
    app_id: &str,
    on_set: &Rc<dyn Fn(Option<Value>)>,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let item = |icon: &'static str, text: &str, extra: Option<&str>| {
        let b = gtk::Button::new();
        b.add_css_class("menu-item");
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        r.append(&Icon::new(icon, 12));
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        r.append(&l);
        if let Some(x) = extra {
            let f = gtk::Label::new(Some(x));
            f.add_css_class("faint");
            f.set_ellipsize(gtk::pango::EllipsizeMode::End);
            r.append(&f);
        }
        b.set_child(Some(&r));
        b
    };
    let default = host.new_key_name(app_id);
    let new = item("plus", "New key", Some(&default));
    let (os, pw, kind_s, w) = (on_set.clone(), pop.downgrade(), kind.to_string(), Rc::downgrade(host));
    new.connect_clicked(move |_| {
        if let Some(p) = pw.upgrade() {
            p.popdown();
        }
        let (os, kind, def, w) = (os.clone(), kind_s.clone(), default.clone(), w.clone());
        glib::spawn_future_local(async move {
            match create_key(&kind, &def, &w).await {
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
        g.set_margin_top(6);
        g.set_margin_start(8);
        col.append(&g);
    }
    for k in existing {
        let b = item("key-round", k["name"].as_str().unwrap_or_default(), None);
        let (os, pw) = (on_set.clone(), pop.downgrade());
        b.connect_clicked(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
            os(Some(k.clone()));
        });
        col.append(&b);
    }
    if has {
        let b = item("x", "No key", None);
        b.add_css_class("danger");
        let (os, pw) = (on_set.clone(), pop.downgrade());
        b.connect_clicked(move |_| {
            if let Some(p) = pw.upgrade() {
                p.popdown();
            }
            os(None);
        });
        col.append(&b);
    }
    pop.set_child(Some(&col));
}

/// A new key (a saved sign-in on this machine) of a kind, named after
/// `base` (made free).
async fn create_key(kind: &str, base: &str, host: &Weak<dyn Host>) -> Result<Value, String> {
    let known = known();
    let mut taken: Vec<String> = known.iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase).collect();
    if let Some(h) = host.upgrade() {
        taken.extend(h.keys_of_kind(&[], kind).iter().filter_map(|k| k["name"].as_str()).map(str::to_lowercase));
    }
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
fn renaming(host: &Rc<dyn Host>, slot: &gtk::Box, btn: &gtk::MenuButton, id: &str, shown: &str) {
    btn.set_visible(false);
    let edit = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    edit.add_css_class("key-edit");
    edit.append(&Icon::new("key-round", 13));
    let e = gtk::Entry::new();
    e.add_css_class("esc-local");
    e.set_text(shown);
    e.set_width_chars(22);
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
                    // On this machine too, where it's one.
                    let (id2, to2) = (id.clone(), to.clone());
                    glib::spawn_future_local(async move {
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
