//! A wadspace's details (the web app's WadspaceDrawer), from its menu: its
//! picture; what can be done with it (open it, customize it for a run, edit
//! it or a copy of it on the board); what it's built on and how its image
//! stands; its projects; where it runs and its last runs; and Delete.

use std::collections::HashMap;

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_gtk::Icon;
use wad_proto::v1::{ImageRow, Run};

use crate::launch::{self, ImageState, RunItem};
use crate::modal::Modal;
use crate::pages::machines::{self, get};
use crate::store::{AppState, store};
use crate::ui::{self, Size, Variant};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Opens a wadspace's details.
pub fn open(id: &str) {
    let Some(w) = store().read_now(|st| st.all_wadspaces().into_iter().find(|w| s(w, "id") == id).cloned()) else {
        return;
    };
    let m = Modal::drawer(480);
    let body = m.body();

    let top = gtk::Overlay::new();
    top.add_css_class("drawer-top");
    let thumb = store().read_now(|st| crate::thumb::thumb(st, &w));
    thumb.set_radius(0.0);
    top.set_child(Some(&thumb));
    let fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fade.add_css_class("drawer-fade");
    fade.set_valign(gtk::Align::End);
    fade.set_size_request(-1, 96);
    fade.set_can_target(false);
    top.add_overlay(&fade);
    let x = ui::icon_button("x", "Close", 16);
    x.add_css_class("glass");
    x.set_halign(gtk::Align::End);
    x.set_valign(gtk::Align::Start);
    x.set_margin_top(12);
    x.set_margin_end(12);
    let mm = m.clone();
    x.connect_clicked(move |_| mm.close());
    top.add_overlay(&x);
    body.append(&top);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 28);
    col.add_css_class("drawer-body");
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_vexpand(true);
    sw.set_child(Some(&col));
    body.append(&sw);

    store().read_now(|st| {
        col.append(&heading(st, &w));
        col.append(&actions(st, &w, &m));
        col.append(&built_on(st, &w));
        col.append(&projects(&w));
        col.append(&containers(st, &w));
    });
    let history = section("Container history");
    let runs = gtk::Box::new(gtk::Orientation::Vertical, 4);
    runs.append(&muted("Loading…"));
    history.append(&runs);
    col.append(&history);
    let del = ui::button("Delete", Some("trash"), Variant::Danger, Size::Md);
    let (w2, mm) = (w.clone(), m.clone());
    del.connect_clicked(move |_| confirm_delete(&w2, Some(&mm)));
    del.set_margin_top(4);
    col.append(&del);

    // Its last runs, and its image as this machine has it.
    let (id, rw) = (id.to_string(), runs.downgrade());
    glib::spawn_future_local(async move {
        let list = get::<Vec<Run>>("/v1/runs".into()).await.unwrap_or_default();
        let Some(runs) = rw.upgrade() else { return };
        ui::clear(&runs);
        let mut mine: Vec<Run> = list.into_iter().filter(|r| r.wadspace_id == id).collect();
        mine.sort_by(|a, b| b.started_at.total_cmp(&a.started_at));
        if mine.is_empty() {
            runs.append(&muted("It hasn't run here yet."));
        }
        for r in mine.iter().take(8) {
            runs.append(&run_row(r));
        }
    });
}

fn section(title: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let t = gtk::Label::new(Some(&title.to_uppercase()));
    t.add_css_class("drawer-section");
    t.set_xalign(0.0);
    b.append(&t);
    b
}

fn muted(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("small-muted-14");
    l
}

fn badge(text: &str, mono: bool) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("badge");
    if mono {
        l.add_css_class("mono");
    }
    l
}

fn heading(st: &AppState, w: &Value) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let n = gtk::Label::new(Some(&st.wadspace_name(w)));
    n.add_css_class("drawer-title");
    n.set_xalign(0.0);
    n.set_wrap(true);
    b.append(&n);
    let d = s(&w["customization"], "description");
    if !d.is_empty() {
        b.append(&muted(d));
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_top(8);
    if let Some(ms) = crate::pages::builder::state::iso_ms(s(w, "updatedAt")) {
        row.append(&badge(&format!("Updated {}", crate::pages::builder::state::time_ago(ms)), false));
    }
    row.append(&badge(s(w, "id"), true));
    b.append(&row);
    b.upcast()
}

fn actions(st: &AppState, w: &Value, m: &Modal) -> gtk::Widget {
    let id = s(w, "id").to_string();
    let locked = machines::locked(st, &id);
    let grid = gtk::Grid::new();
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(8);
    grid.set_row_spacing(8);
    let open = ui::button("Open", Some(if locked { "lock" } else { "play" }), Variant::Primary, Size::Md);
    let customize = ui::button("Customize for a run", Some("sliders-horizontal"), Variant::Secondary, Size::Md);
    customize.set_tooltip_text(Some("Its background or keys, for one run or as a new wadspace"));
    let edit = ui::button("Edit on the board", Some("pencil"), Variant::Secondary, Size::Md);
    let copy = ui::button("Copy it", Some("copy"), Variant::Secondary, Size::Md);
    if locked {
        open.set_sensitive(false);
        customize.set_sensitive(false);
        open.set_tooltip_text(Some("Focus mode: locked until it ends"));
    }
    let (i, mm) = (id.clone(), m.clone());
    open.connect_clicked(move |_| {
        mm.close();
        let i = i.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = launch::launch(vec![RunItem::Wadspace(i)], None).await {
                crate::toast::error("Couldn't launch", &e);
            }
        });
    });
    let (i, mm) = (format!("w:{id}"), m.clone());
    customize.connect_clicked(move |_| {
        mm.close();
        crate::launcher::customize(&i);
    });
    let v4 = wad_core::v4::is_v4(w);
    let (i, mm) = (id.clone(), m.clone());
    edit.connect_clicked(move |_| {
        mm.close();
        if v4 {
            crate::pages::builder::b::show_wadspace(&i, false);
        } else {
            crate::pages::builder::show_stack(&i, false);
        }
    });
    let (i, mm) = (id, m.clone());
    copy.connect_clicked(move |_| {
        mm.close();
        if v4 {
            crate::pages::builder::b::show_wadspace(&i, true);
        } else {
            crate::pages::builder::show_stack(&i, true);
        }
    });
    grid.attach(&open, 0, 0, 1, 1);
    grid.attach(&customize, 1, 0, 1, 1);
    // A version 4 wadspace is edited and copied on design B's board (the
    // old one has version 3's).
    if !v4 || crate::pages::builder::b::enabled() {
        grid.attach(&edit, 0, 1, 1, 1);
        grid.attach(&copy, 1, 1, 1, 1);
    }
    grid.upcast()
}

/// Its templates (bottom first) and Extensions, and how its image stands.
fn built_on(st: &AppState, w: &Value) -> gtk::Widget {
    let sec = section("Built on");
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card.add_css_class("drawer-card");
    let i = Icon::new("layers", 16);
    i.add_css_class("muted");
    card.append(&i);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    texts.set_hexpand(true);
    let lib = st.library.as_ref().map(|l| l.lib()).unwrap_or_default();
    let r = wad_core::layers::resolve_stack(&lib, w);
    let names = if r.is_null() {
        "No template yet".to_string()
    } else {
        let mut n: Vec<String> =
            r["chain"].as_array().into_iter().flatten().map(|t| s(t, "name").to_string()).collect();
        if r["extensions"].is_object() {
            n.push(s(&r["extensions"], "name").to_string());
        }
        n.join(" › ")
    };
    let l = gtk::Label::new(Some(&names));
    l.add_css_class("strong");
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    texts.append(&l);
    let state_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    texts.append(&state_row);
    card.append(&texts);
    let build = ui::button("Build", Some("hammer"), Variant::Ghost, Size::Sm);
    build.set_valign(gtk::Align::Center);
    build.set_visible(false);
    let id = s(w, "id").to_string();
    build.connect_clicked(move |b| {
        b.set_sensitive(false);
        let (b, id) = (b.clone(), id.clone());
        glib::spawn_future_local(async move {
            if let Err(e) = launch::build_here(id).await {
                crate::toast::error("Couldn't build", &e);
            }
            b.set_sensitive(true);
        });
    });
    card.append(&build);
    sec.append(&card);

    let key = wad_core::layers::stack_image_key(w);
    if !r.is_null() && !key.is_empty() {
        let building = st.builds.iter().any(|(b, _)| !b.status.finished() && b.layer.as_deref() == Some(key.as_str()));
        let (rw, bw) = (state_row.downgrade(), build.downgrade());
        glib::spawn_future_local(async move {
            let rows = get::<Vec<ImageRow>>("/v1/images".into()).await.unwrap_or_default();
            let images: HashMap<String, ImageRow> = rows.into_iter().map(|r| (r.layer.clone(), r)).collect();
            let (Some(row), Some(build)) = (rw.upgrade(), bw.upgrade()) else { return };
            let state = launch::image_state(&key, &lib, &images);
            let text = if building {
                "Building".to_string()
            } else {
                match state {
                    ImageState::Built => match images.get(&key) {
                        Some(i) if i.built_at > 0.0 => {
                            format!("Built {}", crate::pages::builder::state::time_ago(i.built_at * 1000.0))
                        }
                        _ => "Built".into(),
                    },
                    ImageState::Update => "Update available".into(),
                    ImageState::Changed => "Changed since built".into(),
                    ImageState::Never => "Not built".into(),
                }
            };
            let b = badge(&text, false);
            if !building && state != ImageState::Built {
                b.add_css_class("pink");
            }
            row.append(&b);
            build.set_visible(!building && state != ImageState::Built);
        });
    }
    sec.upcast()
}

fn projects(w: &Value) -> gtk::Widget {
    let sec = section("Projects");
    let list: Vec<Value> = w["customization"]["projects"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        sec.append(&muted("None."));
    }
    for p in list {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("drawer-row");
        let i = Icon::new(
            match s(&p, "kind") {
                "github" => "folder-git-2",
                "mount" => "folder-symlink",
                _ => "hard-drive-download",
            },
            16,
        );
        i.add_css_class("muted");
        row.append(&i);
        let l = gtk::Label::new(Some(&format!("~/Desktop/{}", s(&p, "as"))));
        l.set_hexpand(true);
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&l);
        let from = match s(&p, "kind") {
            "github" => format!("cloned from {}", s(&p, "repo")),
            "mount" => format!("mounted from {}", p["source"]["paths"][0].as_str().unwrap_or_default()),
            _ => "copied in".into(),
        };
        let f = gtk::Label::new(Some(&from));
        f.add_css_class("small-muted");
        f.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&f);
        sec.append(&row);
    }
    sec.upcast()
}

/// The machines it's on, and how it's doing on each.
fn containers(st: &AppState, w: &Value) -> gtk::Widget {
    let sec = section("Containers");
    let id = s(w, "id");
    let mut any = false;
    for m in machines::machines(st) {
        for c in m.containers.iter().filter(|c| c.id == id) {
            any = true;
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("drawer-card");
            let i = Icon::new("server", 16);
            i.add_css_class("muted");
            row.append(&i);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
            texts.set_hexpand(true);
            let n = gtk::Label::new(Some(&m.label));
            n.add_css_class("strong");
            n.set_xalign(0.0);
            texts.append(&n);
            texts.append(&crate::pages::machines::wadspaces::status_line(c));
            row.append(&texts);
            row.append(&ui::dot(if c.running { "on" } else { "off" }));
            sec.append(&row);
        }
    }
    if !any {
        sec.append(&muted("Not on a machine yet: it's installed when it's first launched."));
    }
    sec.upcast()
}

fn run_row(r: &Run) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("drawer-run");
    row.append(&ui::dot(if r.ended_at.is_none() { "on" } else { "off" }));
    let when = glib::DateTime::from_unix_local(r.started_at as i64)
        .ok()
        .and_then(|d| d.format("%a, %b %-d · %-I:%M %p").ok())
        .map(String::from)
        .unwrap_or_default();
    let l = gtk::Label::new(Some(&when));
    l.set_hexpand(true);
    l.set_xalign(0.0);
    row.append(&l);
    let end = r.ended_at.unwrap_or_else(crate::pages::now);
    let mins = ((end - r.started_at) / 60.0).floor() as u64;
    let len = match (r.ended_at, mins) {
        (None, _) => "running".to_string(),
        (_, 0) => "<1m".into(),
        (_, m) if m >= 60 => format!("{}h {}m", m / 60, m % 60),
        (_, m) => format!("{m}m"),
    };
    let d = gtk::Label::new(Some(&len));
    d.add_css_class("small-muted");
    row.append(&d);
    row.upcast()
}

/// "Delete X?" for a wadspace by its id (a row's menu).
pub fn ask_delete(id: &str) {
    if let Some(w) = store().read_now(|st| st.all_wadspaces().into_iter().find(|w| s(w, "id") == id).cloned()) {
        confirm_delete(&w, None);
    }
}

/// "Delete X?": it's deleted softly (Recently deleted in the Builder has
/// it, and Undo brings it back).
fn confirm_delete(w: &Value, drawer: Option<&Modal>) {
    let name = store().read_now(|st| st.wadspace_name(w));
    let m = Modal::open(
        &format!("Delete {name}?"),
        Some("It waits in the Builder's Recently deleted; Undo brings it back."),
        420,
    );
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(8);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let del = ui::button("Delete", Some("trash"), Variant::Danger, Size::Md);
    let (mm, dm, w) = (m.clone(), drawer.cloned(), w.clone());
    del.connect_clicked(move |_| {
        mm.close();
        if let Some(d) = &dm {
            d.close();
        }
        delete(w.clone(), name.clone());
    });
    row.append(&cancel);
    row.append(&del);
    m.body().append(&row);
}

fn delete(mut w: Value, name: String) {
    let id = s(&w, "id").to_string();
    let now = launch::now_iso();
    w["deletedAt"] = now.clone().into();
    w["updatedAt"] = now.into();
    glib::spawn_future_local(async move {
        if let Err(e) = crate::account::write_doc("wadspaces", w.clone()).await {
            crate::toast::error("Can't delete it", &e);
            return;
        }
        crate::account::put_in_library("wadspaces", w);
        crate::toast::with_action(&format!("Deleted {name}"), crate::toast::Tone::Info, "Undo", move || {
            crate::pages::builder::state::restore_wadspace(id.clone());
        });
    });
}
