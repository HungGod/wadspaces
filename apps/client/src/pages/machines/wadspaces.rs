//! A machine's wadspaces (the web app's Containers, ContainerControls and
//! WadspaceActions): what's installed there, each with its state and
//! Restart / Stop / Start; on this machine also your wadspaces it doesn't
//! have yet, to build here.

use gtk::{glib, prelude::*};
use serde_json::Value;

use super::{Container, Machine, container_action, locked, machine};
use crate::launch::{self, RunItem};
use crate::pages::{badge, now, uptime};
use crate::shell::Page;
use crate::store::{AppState, Topic, store};
use crate::thumb::thumb;
use crate::ui::{self, Size, Variant};

pub fn view(id: String) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("panel");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.add_css_class("panel-head");
    let title = gtk::Label::new(Some("On this machine"));
    title.add_css_class("panel-title");
    title.set_hexpand(true);
    title.set_xalign(0.0);
    head.append(&title);
    let counts = gtk::Label::new(None);
    counts.add_css_class("small-muted");
    head.append(&counts);
    root.append(&head);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&rows);
    store().watch(
        &root,
        Topic::MACHINE
            | Topic::WORKSPACES
            | Topic::STATES
            | Topic::VIEW
            | Topic::LIBRARY
            | Topic::WADD
            | Topic::SESSION
            | Topic::CLOUD,
        move |s| {
            let Some(m) = machine(s, &id) else { return };
            render(&rows, &counts, s, &m);
        },
    );
    root.upcast()
}

/// Your wadspaces this machine doesn't have yet (they're installed when
/// they're built or launched here).
fn not_here<'a>(s: &'a AppState, m: &Machine) -> Vec<&'a Value> {
    if !m.here {
        return vec![];
    }
    s.wadspaces()
        .into_iter()
        .filter(|w| wad_core::v5::is_v5(w) && w["customization"].is_object())
        .filter(|w| !m.containers.iter().any(|c| Some(c.id.as_str()) == w["id"].as_str()))
        .collect()
}

fn render(rows: &gtk::Box, counts: &gtk::Label, s: &AppState, m: &Machine) {
    ui::clear(rows);
    let missing = not_here(s, m);
    let running = m.containers.iter().filter(|c| c.running).count();
    let mut c = format!("{running} running · {} installed", m.containers.len());
    if !missing.is_empty() {
        c.push_str(&format!(" · {} not built here", missing.len()));
    }
    counts.set_text(&c);
    if m.containers.is_empty() && missing.is_empty() {
        let pad = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pad.add_css_class("panel-pad");
        let body = if m.up {
            "Make a wadspace in the Builder: build or launch it and it's installed here.".to_string()
        } else {
            format!("{} is offline.", m.label)
        };
        pad.append(&ui::empty_state("power", "No wadspaces yet", Some(&body)));
        rows.append(&pad);
        return;
    }
    let live = s.wadspaces();
    for c in &m.containers {
        let w = live.iter().find(|w| w["id"].as_str() == Some(&c.id)).copied();
        rows.append(&container_row(s, m, c, w));
    }
    for w in missing {
        rows.append(&missing_row(s, m, w));
    }
}

fn picture(s: &AppState, w: Option<&Value>, dim: bool) -> gtk::Widget {
    match w {
        Some(w) => {
            let t = thumb(s, w);
            t.set_fixed_width(112);
            t.set_radius(12.0);
            t.set_valign(gtk::Align::Center);
            if dim {
                t.set_opacity(0.7);
            }
            t.upcast()
        }
        None => {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("thumb-blank");
            b.set_size_request(112, 63);
            b.set_valign(gtk::Align::Center);
            b.upcast()
        }
    }
}

fn container_row(s: &AppState, m: &Machine, c: &Container, w: Option<&Value>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("ws-row");
    row.append(&picture(s, w, false));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&w.map(|w| s.wadspace_name(w)).unwrap_or_else(|| c.name.clone())));
    name.add_css_class("ws-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_xalign(0.0);
    top.append(&name);
    if let Some(k) = c.hotkey {
        top.append(&badge(&format!("Super+{k}"), Some("keyboard"), false));
    }
    text.append(&top);
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status.append(&status_line(c));
    let mode = if w.is_none() {
        "On this machine"
    } else if c.stream {
        "Streamed display"
    } else {
        "On screen"
    };
    status.append(&badge(mode, None, false));
    text.append(&status);
    if let Some(e) = c.error.as_deref().filter(|_| c.phase == "error") {
        let l = ui::wrapping(e);
        l.add_css_class("small-danger");
        text.append(&l);
    }
    row.append(&text);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    controls.set_valign(gtk::Align::Center);
    controls_for(&controls, s, m, c);
    if let Some(w) = w {
        actions_for(&controls, s, w);
    }
    row.append(&controls);
    row.upcast()
}

/// The dot, what it's doing and how long it's been up (the web app's
/// ContainerStatus).
pub fn status_line(c: &Container) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.append(&ui::dot(c.dot()));
    let mut t = c.phase_label().to_string();
    if c.running && c.phase != "error" {
        t.push_str(&format!(" · up {}", uptime(now() - c.since)));
    }
    let l = gtk::Label::new(Some(&t));
    l.add_css_class("small-muted");
    b.append(&l);
    b.upcast()
}

/// Restart and Stop while it runs; Start in the background when it doesn't.
fn controls_for(row: &gtk::Box, s: &AppState, m: &Machine, c: &Container) {
    let act = |icon: &'static str, label: &str, action: &'static str, ok: bool| {
        let b = ui::icon_button(icon, label, if icon == "square" { 14 } else { 16 });
        b.set_sensitive(ok);
        let (m, id) = (m.clone(), c.id.clone());
        b.connect_clicked(move |b| {
            b.set_sensitive(false);
            let b = b.clone();
            container_action(&m, &id, action, move || b.set_sensitive(true));
        });
        row.append(&b);
    };
    if c.running {
        act("rotate-cw", "Restart", "restart", m.up);
        act("square", "Stop", "stop", m.up);
    } else {
        act("power", "Start in the background", "start", m.up && !locked(s, &c.id));
    }
}

/// Open, and the rest in a menu (the web app's WadspaceActions).
fn actions_for(row: &gtk::Box, s: &AppState, w: &Value) {
    let id = w["id"].as_str().unwrap_or_default().to_string();
    let is_locked = locked(s, &id);
    let open = ui::icon_button(if is_locked { "lock" } else { "play" }, "Open", 16);
    if is_locked {
        open.set_tooltip_text(Some("Focus mode: locked until it ends"));
        open.set_sensitive(false);
    }
    open.connect_clicked(move |b| {
        b.set_sensitive(false);
        let (b, id) = (b.clone(), id.clone());
        glib::spawn_future_local(async move {
            if let Err(e) = launch::launch(vec![RunItem::Wadspace(id)], None).await {
                crate::toast::error("Couldn't launch", &e);
            }
            b.set_sensitive(true);
        });
    });
    row.append(&open);
    row.append(&ui::menu(
        "ellipsis",
        "More",
        vec![
            ("Details", "info", false, {
                let id = w["id"].as_str().unwrap_or_default().to_string();
                Box::new(move || crate::details::open(&id))
            }),
            ("Customize for a run", "sliders-horizontal", false, {
                let id = format!("w:{}", w["id"].as_str().unwrap_or_default());
                Box::new(move || crate::launcher::customize(&id))
            }),
            ("Edit in Builder", "pencil", false, {
                let id = w["id"].as_str().unwrap_or_default().to_string();
                Box::new(move || crate::pages::builder::b::show_wadspace(&id, false))
            }),
            ("New from it", "copy", false, {
                let id = w["id"].as_str().unwrap_or_default().to_string();
                Box::new(move || crate::pages::builder::b::show_wadspace(&id, true))
            }),
            ("Delete", "trash", true, {
                let id = w["id"].as_str().unwrap_or_default().to_string();
                Box::new(move || crate::details::ask_delete(&id))
            }),
        ],
    ));
}

fn missing_row(s: &AppState, m: &Machine, w: &Value) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("ws-row");
    row.append(&picture(s, Some(w), true));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(Some(&s.wadspace_name(w)));
    name.add_css_class("ws-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_xalign(0.0);
    text.append(&name);
    let tag = badge("Not built here", None, false);
    tag.set_halign(gtk::Align::Start);
    text.append(&tag);
    let l = ui::wrapping("Saved in your library; installed here when you build or launch it.");
    l.add_css_class("small-muted");
    text.append(&l);
    row.append(&text);
    let id = w["id"].as_str().unwrap_or_default().to_string();
    let build = ui::button("Build here", Some("hammer"), Variant::Secondary, Size::Sm);
    build.set_valign(gtk::Align::Center);
    build.set_sensitive(m.up);
    let name = s.wadspace_name(w);
    build.connect_clicked(move |b| {
        b.set_sensitive(false);
        let (b, id, name) = (b.clone(), id.clone(), name.clone());
        glib::spawn_future_local(async move {
            if let Err(e) = launch::build_here(id).await {
                crate::toast::error(&format!("Couldn't build {name}"), &e);
            }
            b.set_sensitive(true);
        });
    });
    row.append(&build);
    let open = ui::button("Open in Builder", None, Variant::Ghost, Size::Sm);
    open.set_valign(gtk::Align::Center);
    open.connect_clicked(|_| crate::shell::go(Page::Builder));
    row.append(&open);
    row.upcast()
}
