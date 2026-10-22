//! The machines along the bottom of Builder B: click one to show its
//! wadspaces. A wadspace dropped on one (by its name, or its
//! Customizations bar with what's under it) is copied there, those picked
//! with it too; anything else dropped on one starts a new wadspace there.
//! Or pick wadspaces, and press Copy on a machine.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::{glib, prelude::*};
use wad_board::b4::{Op, Payload as Bp};
use wad_gtk::Icon;

use super::buckets::{self, of, source, whole};
use super::state;
use crate::drag::{self, Mods, Payload, Target, Verdict};
use crate::store::AppState;
use crate::ui::{self, Size, Variant};

thread_local! {
    /// Each machine's card, by its board id (the snapshots' drags).
    static CARDS: RefCell<HashMap<String, glib::WeakRef<gtk::Box>>> = RefCell::default();
}

/// A machine's card, wide.
const CARD_W: i32 = 250;

/// A machine's card, if it's drawn.
pub fn card_of(id: &str) -> Option<gtk::Box> {
    CARDS.with(|c| c.borrow().get(id).and_then(|w| w.upgrade()))
}

/// A machine as the carousel shows it.
struct Shown {
    id: String,
    name: String,
    detail: String,
    up: bool,
    here: bool,
}

fn machines(s: &AppState) -> Vec<Shown> {
    let me = state::here(s);
    // This machine by the name the account has for it.
    let my_name = s
        .library
        .iter()
        .flat_map(|l| l.machines.iter())
        .find(|m| m["id"].as_str() == Some(&me))
        .and_then(|m| m["name"].as_str().or(m["hostname"].as_str()))
        .map(String::from);
    crate::pages::machines::machines(s)
        .into_iter()
        .map(|m| Shown {
            name: if m.here { my_name.clone().unwrap_or(m.label.clone()) } else { m.label.clone() },
            detail: if m.here {
                "This machine".into()
            } else if m.up {
                "Online".into()
            } else {
                match m.last_seen {
                    Some(t) => format!("Offline · seen {}", crate::pages::ago_ms(t * 1000.0)),
                    None => "Offline".into(),
                }
            },
            id: if m.here { me.clone() } else { m.id },
            up: m.up || m.here,
            here: m.here,
        })
        .collect()
}

pub fn bar(s: &AppState) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.add_css_class("wbb-machines");
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    track.add_css_class("wbb-track");
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::External);
    sw.set_vscrollbar_policy(gtk::PolicyType::Never);
    sw.set_hexpand(true);
    sw.set_child(Some(&track));
    let prev = ui::icon_button("chevron-left", "Earlier machines", 16);
    let next = ui::icon_button("chevron-right", "More machines", 16);
    let adj = sw.hadjustment();
    for (b, dir) in [(&prev, -1.0), (&next, 1.0)] {
        let adj = adj.clone();
        b.connect_clicked(move |_| {
            let to = adj.value() + dir * adj.page_size() * 0.8;
            adj.set_value(to.clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
        });
    }
    let edges = {
        let (prev, next, adj) = (prev.clone(), next.clone(), adj.clone());
        move || {
            prev.set_sensitive(adj.value() > 2.0);
            next.set_sensitive(adj.value() + adj.page_size() < adj.upper() - 2.0);
        }
    };
    let e = edges.clone();
    adj.connect_value_changed(move |_| e());
    let e = edges.clone();
    adj.connect_changed(move |_| e());
    edges();

    let shown = state::shown(s);
    let picked: Vec<(String, String, String)> = state::read(|st| {
        state::picked_wadspaces(st)
            .into_iter()
            .filter_map(|id| st.board.get(&id).map(|w| (w.id.clone(), w.name().to_string(), w.machine.clone())))
            .collect()
    });
    CARDS.with(|c| c.borrow_mut().clear());
    for m in machines(s) {
        let c = card(&m, m.id == shown, &picked);
        CARDS.with(|cs| cs.borrow_mut().insert(m.id.clone(), c.downgrade()));
        // As wide as a card is, however few there are.
        track.append(&wad_gtk::Width::new(&c, CARD_W));
    }
    root.append(&prev);
    root.append(&sw);
    root.append(&next);
    drag::autoscroll(&sw);
    root.upcast()
}

fn card(m: &Shown, shown: bool, picked: &[(String, String, String)]) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 8);
    c.add_css_class("wbb-machine");
    if shown {
        c.add_css_class("shown");
    }
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icon.add_css_class("wbb-machine-icon");
    icon.append(&Icon::new(if m.here { "laptop" } else { "monitor" }, 16));
    top.append(&icon);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    let n = gtk::Label::new(Some(&m.name));
    n.add_css_class("wbb-machine-name");
    n.set_xalign(0.0);
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    texts.append(&n);
    let d = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("wbb-dot");
    if m.up {
        dot.add_css_class("up");
    }
    dot.set_valign(gtk::Align::Center);
    d.append(&dot);
    let dl = gtk::Label::new(Some(&m.detail));
    dl.add_css_class("muted");
    dl.add_css_class("tiny");
    dl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    d.append(&dl);
    texts.append(&d);
    top.append(&texts);
    let row: Vec<(String, Option<String>)> =
        state::read(|st| st.board.row(&m.id).iter().map(|w| (w.name().to_string(), buckets::texture_of(w))).collect());
    let count = gtk::Label::new(Some(&row.len().to_string()));
    count.add_css_class("faint");
    count.add_css_class("tiny");
    count.set_tooltip_text(Some(&format!("{} wadspace{}", row.len(), if row.len() == 1 { "" } else { "s" })));
    top.append(&count);
    c.append(&top);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom.add_css_class("wbb-machine-row");
    let copy: Vec<&(String, String, String)> = picked.iter().filter(|(_, _, on)| *on != m.id).collect();
    if !copy.is_empty() && !shown {
        let text = if copy.len() == 1 {
            format!("Copy {} here", copy[0].1)
        } else {
            format!("Copy {} wadspaces here", copy.len())
        };
        let b = ui::button(&text, Some("copy"), Variant::Secondary, Size::Sm);
        b.add_css_class("wbb-copy-here");
        b.set_hexpand(true);
        let (ids, machine, name) =
            (copy.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>(), m.id.clone(), m.name.clone());
        b.connect_clicked(move |_| copy_to(ids.clone(), &machine, &name));
        bottom.append(&b);
    } else if !row.is_empty() {
        for (name, _) in row.iter().take(2) {
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            chip.add_css_class("wbb-machine-ws");
            let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            swatch.add_css_class("wbb-swatch");
            chip.append(&swatch);
            let l = gtk::Label::new(Some(name));
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_max_width_chars(11);
            chip.append(&l);
            chip.set_tooltip_text(Some(name));
            bottom.append(&chip);
        }
        if row.len() > 2 {
            let more = gtk::Label::new(Some(&format!("+{}", row.len() - 2)));
            more.add_css_class("faint");
            more.add_css_class("tiny");
            more.set_tooltip_text(Some(&row[2..].iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")));
            bottom.append(&more);
        }
    } else {
        let l = gtk::Label::new(Some("No wadspaces yet"));
        l.add_css_class("faint");
        l.add_css_class("tiny");
        bottom.append(&l);
    }
    c.append(&bottom);
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let id = m.id.clone();
    click.connect_released(move |_, _, _, _| show(&id));
    c.add_controller(click);
    c.set_tooltip_text(Some(&format!("Show {}'s wadspaces · drop a wadspace here to copy it", m.name)));
    drag::target(&c, MachineTarget { id: m.id.clone(), name: m.name.clone() });
    c
}

/// The board on another machine's wadspaces (the sidebar closes).
pub fn show(id: &str) {
    let here = crate::store::store().read_now(state::here);
    state::write(|s| {
        s.shown = (id != here).then(|| id.to_string());
        s.open = None;
    });
    state::changed();
}

/// Wadspaces copied to a machine, in one step (one Undo); a toast says so,
/// with Show.
fn copy_to(ids: Vec<String>, machine: &str, name: &str) {
    let first = state::read(|s| ids.first().and_then(|i| s.board.get(i)).map(|w| w.name().to_string()));
    let n = ids.len();
    if state::apply(Op::CopyTo { ids, machine: machine.into() }).is_err() {
        return;
    }
    state::write(|s| s.picked.clear());
    state::changed();
    let text = match (n, first) {
        (1, Some(one)) => format!("Copied {one} to {name}"),
        _ => format!("Copied {n} wadspaces to {name}"),
    };
    told(&text, machine);
}

/// A toast about a machine, with Show when it isn't the one shown.
fn told(text: &str, machine: &str) {
    let shown = crate::store::store().read_now(state::shown);
    if shown == machine {
        crate::toast::show(text, None, crate::toast::Tone::Info);
    } else {
        let m = machine.to_string();
        crate::toast::with_action(text, crate::toast::Tone::Info, "Show", move || show(&m));
    }
}

struct MachineTarget {
    id: String,
    name: String,
}

impl MachineTarget {
    /// The wadspaces a whole-wadspace drop copies: those picked, when it's
    /// one of them, else it alone (none already on this machine).
    fn copies(&self, ws: &str) -> Vec<String> {
        state::read(|st| {
            let picked = state::picked_wadspaces(st);
            let ids = if picked.iter().any(|p| p == ws) { picked } else { vec![ws.to_string()] };
            ids.into_iter().filter(|id| st.board.get(id).is_some_and(|w| w.machine != self.id)).collect()
        })
    }
}

impl Target for MachineTarget {
    fn accepts(&self, p: &Payload, _m: Mods, _at: (f64, f64)) -> Option<Verdict> {
        let b = of(p)?;
        let ws = source(b)?;
        if whole(b) {
            let ids = self.copies(ws);
            return match ids.len() {
                0 => Verdict::no(format!("It's on {} already", self.name)),
                1 => Verdict::yes(format!(
                    "Copy {} to {}",
                    state::read(|s| s.board.get(&ids[0]).map(|w| w.name().to_string())).unwrap_or_default(),
                    self.name
                )),
                n => Verdict::yes(format!("Copy {n} wadspaces to {}", self.name)),
            };
        }
        Verdict::yes(format!("A new wadspace on {}", self.name))
    }

    fn land(&self, p: &Payload, _m: Mods, _at: (f64, f64)) {
        let Some(b) = of(p) else { return };
        let Some(ws) = source(b) else { return };
        if whole(b) {
            copy_to(self.copies(ws), &self.id, &self.name);
            return;
        }
        if state::apply(Op::Fresh { payload: b.clone(), machine: self.id.clone(), before: None }).is_ok() {
            told(&format!("A new wadspace on {}", self.name), &self.id);
        }
    }
}

/// What a drop here would carry, for the snapshots: a wadspace by its name.
pub fn wadspace_payload(ws: &str) -> Payload {
    Payload::B(Bp::Wadspace { ws: ws.into() })
}
