//! Recently deleted, in the sidebar (the old Builder's panel, for design
//! B): what was taken out of each wadspace (a template, an app off one, a
//! project, the background), each to put back where it was or forget; and
//! the deleted wadspaces, each to restore whole or delete for good, with
//! its parts in it to drag into any wadspace (just those). Undo takes back
//! any of it.

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_board::b4::{Kept, Op, Payload as Bp, Piece, Wadspace};
use wad_gtk::Icon;

use super::buckets::{self, act, app_icon, app_name, projects_of, texture_name, texture_of};
use super::state;
use crate::drag::{self, Payload};
use crate::pages::builder::parts::img;
use crate::pages::builder::parts::projects::{project_icon, project_text};
use crate::store::AppState;
use crate::ui::{self, Size, Variant};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// How long ago an ISO time was ("" when it isn't one).
fn ago(iso: &str) -> String {
    glib::DateTime::from_iso8601(iso, None)
        .ok()
        .map(|t| crate::pages::ago_ms(t.to_unix() as f64 * 1000.0))
        .unwrap_or_default()
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A kept piece as it looks: its picture, what it is, and a line under it.
fn face(w: &Wadspace, piece: &Piece) -> gtk::Box {
    let (pic, title, detail): (gtk::Widget, String, String) = match piece {
        Piece::Template { template, .. } => (
            Icon::new("layout-grid", 14).upcast(),
            template.name.clone(),
            template.apps.iter().map(|e| app_name(&e.app)).collect::<Vec<_>>().join(", "),
        ),
        Piece::App { template, entry } => {
            let from = w.templates.iter().find(|t| t.id == *template).map_or(template.as_str(), |t| t.name.as_str());
            let n = entry.extras.len();
            let more = if n > 0 { format!(" · {n} inside") } else { String::new() };
            (img(&app_icon(&entry.app), 16).upcast(), app_name(&entry.app), format!("off {from}{more}"))
        }
        Piece::Project { project } => {
            (Icon::new(project_icon(project), 14).upcast(), s(project, "as").to_string(), project_text(project))
        }
        Piece::Background { wallpaper } => (
            Icon::new("palette", 14).upcast(),
            "Background".to_string(),
            match s(wallpaper, "type") {
                "texture" => texture_name(s(wallpaper, "value")),
                _ => String::new(),
            },
        ),
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wbb-kept-face");
    pic.set_valign(gtk::Align::Start);
    row.append(&pic);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.append(&label(&title, &["wbb-kept-name"]));
    if !detail.is_empty() {
        texts.append(&label(&detail, &["faint", "tiny"]));
    }
    row.append(&texts);
    row
}

/// A kept piece: Put back (where it was), or forget it.
fn kept_row(w: &Wadspace, k: &Kept) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("wbb-kept");
    let f = face(w, &k.piece);
    f.set_hexpand(true);
    f.set_tooltip_text(Some(&format!("Taken out {}", ago(&k.at))));
    row.append(&f);
    let back = ui::button("Put back", None, Variant::Secondary, Size::Sm);
    back.set_valign(gtk::Align::Center);
    back.set_tooltip_text(Some(match &k.piece {
        Piece::Template { .. } => "Back where it was among the templates",
        Piece::App { .. } => "Back into its template (a template of its own if that's gone)",
        Piece::Project { .. } => "Back among its projects",
        Piece::Background { .. } => "Back as its background",
    }));
    let (ws, id) = (w.id.clone(), k.id.clone());
    back.connect_clicked(move |_| act(Op::PutBack { ws: ws.clone(), kept: id.clone() }));
    row.append(&back);
    let bin = ui::icon_button("trash", "Forget it (Undo still brings it back)", 13);
    bin.set_valign(gtk::Align::Center);
    let (ws, id) = (w.id.clone(), k.id.clone());
    bin.connect_clicked(move |_| act(Op::Forget { ws: ws.clone(), kept: Some(id.clone()) }));
    row.append(&bin);
    row
}

/// A part of a deleted wadspace, to drag into any wadspace.
fn part(icon: &'static str, text: &str, payload: Bp, ghost_icon: String) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    c.add_css_class("wbb-chip");
    c.add_css_class("wbb-kept-part");
    c.append(&Icon::new(icon, 11));
    let l = gtk::Label::new(Some(text));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(22);
    c.append(&l);
    c.set_halign(gtk::Align::Start);
    c.set_tooltip_text(Some("Drag it onto a wadspace to bring just it back there"));
    let text = text.to_string();
    drag::draggable(&c, move |_| Some((Payload::B(payload.clone()), buckets::ghost(&ghost_icon, &text))));
    c
}

/// A deleted wadspace: Restore it whole, delete it for good, or drag its
/// parts out; what was taken out of it before, to put back into it.
fn deleted_card(machine: &str, w: &Wadspace) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("wbb-kept-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.append(&label(if w.name().trim().is_empty() { "Untitled" } else { w.name() }, &["wbb-kept-name"]));
    let when = w.deleted_at.as_deref().map(ago).unwrap_or_default();
    texts.append(&label(&format!("on {machine} · deleted {when}"), &["faint", "tiny"]));
    head.append(&texts);
    let back = ui::button("Restore", None, Variant::Secondary, Size::Sm);
    back.set_valign(gtk::Align::Center);
    back.set_tooltip_text(Some("All of it, back on its machine"));
    let ws = w.id.clone();
    back.connect_clicked(move |_| act(Op::Restore { ws: ws.clone() }));
    head.append(&back);
    let gone = ui::icon_button("trash", "Delete it for good", 13);
    gone.set_valign(gtk::Align::Center);
    let ws = w.id.clone();
    gone.connect_clicked(move |_| state::delete_forever(ws.clone()));
    head.append(&gone);
    card.append(&head);

    let parts = gtk::FlowBox::new();
    parts.add_css_class("wbb-kept-parts");
    parts.set_selection_mode(gtk::SelectionMode::None);
    parts.set_max_children_per_line(2);
    parts.set_column_spacing(4);
    parts.set_row_spacing(4);
    if let Some(t) = texture_of(w) {
        parts.insert(&part("palette", &texture_name(&t), Bp::Background { ws: w.id.clone() }, String::new()), -1);
    }
    for p in projects_of(w) {
        let payload = Bp::Project { ws: w.id.clone(), id: s(&p, "id").into() };
        parts.insert(&part(project_icon(&p), s(&p, "as"), payload, String::new()), -1);
    }
    for t in &w.templates {
        let first = t.apps.first().map(|e| app_icon(&e.app)).unwrap_or_default();
        let payload = Bp::Templates { ws: w.id.clone(), ids: vec![t.id.clone()] };
        parts.insert(&part("layout-grid", &t.name, payload, first), -1);
    }
    if parts.first_child().is_some() {
        card.append(&parts);
    }
    if !w.trash.is_empty() {
        card.append(&label("Taken out of it before (put back, it's there when it's restored):", &["faint", "tiny"]));
        for k in w.trash.iter().rev() {
            card.append(&kept_row(w, k));
        }
    }
    card
}

/// The sidebar's Recently deleted.
pub fn panel(s: &AppState) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("wbb-side");
    let head = gtk::Box::new(gtk::Orientation::Vertical, 6);
    head.add_css_class("wbb-side-head");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some("Recently deleted"));
    name.add_css_class("wbb-side-name");
    name.set_hexpand(true);
    name.set_xalign(0.0);
    top.append(&name);
    let close = ui::icon_button("x", "Close", 14);
    close.connect_clicked(|_| super::close());
    top.append(&close);
    head.append(&top);
    head.append(&label(
        "Put back returns a piece to where it was. A deleted wadspace's parts drag onto any wadspace, just \
         them. Undo takes any of it back.",
        &["faint", "tiny"],
    ));
    root.append(&head);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("wbb-side-body");
    let machines: Vec<(String, String)> = crate::pages::machines::machines(s)
        .into_iter()
        .map(|m| (if m.here { state::here(s) } else { m.id }, m.label))
        .collect();
    let machine = |id: &str| machines.iter().find(|(m, _)| m == id).map(|(_, n)| n.clone()).unwrap_or_default();
    let (living, deleted): (Vec<Wadspace>, Vec<Wadspace>) = state::read(|st| {
        let living = st.board.wadspaces.iter().filter(|w| !w.deleted() && !w.trash.is_empty()).cloned().collect();
        (living, st.board.deleted().into_iter().cloned().collect())
    });
    if living.is_empty() && deleted.is_empty() {
        body.append(&label("Nothing deleted lately.", &["faint"]));
    }
    if !living.is_empty() {
        body.append(&label("TAKEN OUT", &["wbb-what"]));
    }
    // Newest first: by what was last taken out of each.
    let mut living = living;
    living.sort_by(|a, b| b.trash.last().map(|k| &k.at).cmp(&a.trash.last().map(|k| &k.at)));
    for w in &living {
        let group = gtk::Box::new(gtk::Orientation::Vertical, 4);
        group.add_css_class("wbb-kept-card");
        let on = machine(&w.machine);
        let from = if on.is_empty() { format!("From {}", w.name()) } else { format!("From {} · on {on}", w.name()) };
        group.append(&label(&from, &["wbb-kept-from"]));
        for k in w.trash.iter().rev() {
            group.append(&kept_row(w, k));
        }
        body.append(&group);
    }
    if !deleted.is_empty() {
        body.append(&label("DELETED WADSPACES", &["wbb-what"]));
    }
    for w in &deleted {
        body.append(&deleted_card(&machine(&w.machine), w));
    }
    root.append(&super::sidebar::scroller("#recently-deleted", state::Tab::Apps, &body));
    let side = wad_gtk::Width::new(&root, super::sidebar::SIDE_W);
    side.set_vexpand(true);
    side.upcast()
}
