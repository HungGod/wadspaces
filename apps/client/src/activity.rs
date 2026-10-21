//! Background work in the sidebar (the web app's ActivityTray): layer image
//! builds and launches getting wadspaces ready, each with its log; a ring
//! when the sidebar's narrow.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use wad_gtk::Icon;
use wad_proto::v1::{Build, BuildStatus, Launch, LaunchStatus, PartState};
use wad_wadd_client::{Method, Wadd};

use crate::modal::Modal;
use crate::rt;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

thread_local! {
    /// Taken off the list (wadd keeps them; the tray doesn't show them).
    static DISMISSED: RefCell<HashSet<String>> = RefCell::default();
}

fn shown_builds(s: &AppState) -> Vec<&(Build, Vec<String>)> {
    DISMISSED.with(|d| {
        let d = d.borrow();
        s.builds.iter().filter(|(b, _)| !d.contains(&b.id)).collect()
    })
}

fn shown_launches(s: &AppState) -> Vec<&(Launch, Vec<String>)> {
    DISMISSED.with(|d| {
        let d = d.borrow();
        s.launches.iter().filter(|(l, _)| !d.contains(&l.id)).collect()
    })
}

fn dismiss(id: &str) {
    DISMISSED.with(|d| d.borrow_mut().insert(id.to_string()));
    store().update(|_| Topic::ACTIVITY);
}

fn build_text(b: &Build) -> String {
    match b.status {
        BuildStatus::Done => "Ready".into(),
        BuildStatus::Error => "Failed".into(),
        BuildStatus::Cancelled => "Cancelled".into(),
        _ => format!("{}%", (b.progress * 100.0).floor()),
    }
}

fn launch_text(l: &Launch) -> String {
    match l.status {
        LaunchStatus::Done => "Open".into(),
        LaunchStatus::Error => "Failed".into(),
        LaunchStatus::Cancelled => "Cancelled".into(),
        _ if l.phase == "starting" => "Starting".into(),
        _ => format!("{}%", (l.progress * 100.0).floor()),
    }
}

/// A thin progress bar (the web app's Progress).
pub fn progress(value: f64) -> gtk::ProgressBar {
    let p = gtk::ProgressBar::new();
    p.add_css_class("thin");
    p.set_fraction(value.clamp(0.0, 1.0));
    p
}

/// Cancels a job on wadd.
fn cancel(path: String) {
    glib::spawn_future_local(async move {
        let w = Wadd::here();
        if let Err(e) = rt::run(async move { w.act(Method::DELETE, &path, None::<&()>).await }).await {
            crate::toast::error("Couldn't cancel", &e.to_string());
        }
    });
}

/// The tray: under the nav, above Launch. `narrow`: rings.
pub fn tray(narrow: Rc<std::cell::Cell<bool>>) -> gtk::Box {
    let tray = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let t = tray.clone();
    store().watch(&tray, Topic::ACTIVITY, move |s| draw(&t, s, narrow.get()));
    tray
}

/// Draws it again (the sidebar changed width).
pub fn redraw(tray: &gtk::Box, narrow: bool) {
    store().read_now(|s| draw(tray, s, narrow));
}

fn draw(tray: &gtk::Box, s: &AppState, narrow: bool) {
    ui::clear(tray);
    let builds = shown_builds(s);
    let launches = shown_launches(s);
    if narrow {
        if !builds.is_empty() {
            let active: Vec<_> = builds.iter().filter(|(b, _)| !b.status.finished()).collect();
            let avg = if active.is_empty() {
                1.0
            } else {
                active.iter().map(|(b, _)| b.progress).sum::<f64>() / active.len() as f64
            };
            let first = active
                .first()
                .map(|(b, _)| b.id.clone())
                .or(builds.last().map(|(b, _)| b.id.clone()))
                .unwrap_or_default();
            let tip =
                builds.iter().map(|(b, _)| format!("{} · {}", b.name, build_text(b))).collect::<Vec<_>>().join("\n");
            tray.append(&ring(avg, "hammer", builds.len(), active.is_empty(), &tip, move || build_log(&first)));
        }
        if !launches.is_empty() {
            let active: Vec<_> = launches.iter().filter(|(l, _)| !l.status.finished()).collect();
            let avg = if active.is_empty() {
                1.0
            } else {
                active.iter().map(|(l, _)| l.progress).sum::<f64>() / active.len() as f64
            };
            let first = active
                .first()
                .map(|(l, _)| l.id.clone())
                .or(launches.last().map(|(l, _)| l.id.clone()))
                .unwrap_or_default();
            let tip =
                launches.iter().map(|(l, _)| format!("{} · {}", l.name, launch_text(l))).collect::<Vec<_>>().join("\n");
            tray.append(&ring(avg, "rocket", launches.len(), active.is_empty(), &tip, move || launch_log(&first)));
        }
        return;
    }
    if !builds.is_empty() {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("tray-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.add_css_class("tray-head");
        let i = Icon::new("hammer", 14);
        i.add_css_class("accent");
        head.append(&i);
        let t = gtk::Label::new(Some("BUILDS"));
        t.set_hexpand(true);
        t.set_xalign(0.0);
        head.append(&t);
        if builds.iter().any(|(b, _)| b.status.finished()) {
            let clear = gtk::Button::with_label("Clear");
            clear.add_css_class("link");
            clear.set_tooltip_text(Some("Take every finished build off the list"));
            let done: Vec<String> =
                builds.iter().filter(|(b, _)| b.status.finished()).map(|(b, _)| b.id.clone()).collect();
            clear.connect_clicked(move |_| {
                for id in &done {
                    dismiss(id);
                }
            });
            head.append(&clear);
        }
        card.append(&head);
        for (b, _) in &builds {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            row.add_css_class("tray-row");
            let open = gtk::Button::new();
            open.add_css_class("tray-open");
            open.set_hexpand(true);
            open.set_tooltip_text(Some("Show build log"));
            let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
            names.set_hexpand(true);
            let n = gtk::Label::new(Some(&b.name));
            n.add_css_class("tray-name");
            n.set_xalign(0.0);
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            names.append(&n);
            if let Some(layer) = &b.layer {
                let r = gtk::Label::new(Some(&wad_core::v4::image_ref(layer, wad_core::spec::DEFAULT_BASE_IMAGE)));
                r.add_css_class("tray-ref");
                r.set_xalign(0.0);
                r.set_ellipsize(gtk::pango::EllipsizeMode::End);
                names.append(&r);
            }
            line.append(&names);
            let st = gtk::Label::new(Some(&build_text(b)));
            st.add_css_class(match b.status {
                BuildStatus::Done => "tray-done",
                BuildStatus::Error => "tray-error",
                _ => "tray-pct",
            });
            st.set_valign(gtk::Align::Start);
            line.append(&st);
            col.append(&line);
            if !b.status.finished() {
                let p = progress(b.progress);
                p.set_margin_top(6);
                col.append(&p);
            }
            open.set_child(Some(&col));
            let id = b.id.clone();
            open.connect_clicked(move |_| build_log(&id));
            row.append(&open);
            let x = gtk::Button::new();
            x.add_css_class("tray-x");
            x.set_child(Some(&Icon::new("x", 14)));
            x.set_valign(gtk::Align::Center);
            let (id, going) = (b.id.clone(), !b.status.finished());
            x.set_tooltip_text(Some(if going { "Stop it, and take it off the list" } else { "Dismiss" }));
            x.connect_clicked(move |_| {
                if going {
                    cancel(format!("/v1/builds/{id}"));
                }
                dismiss(&id);
            });
            row.append(&x);
            card.append(&row);
        }
        tray.append(&card);
    }
    if !launches.is_empty() {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("tray-card");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.add_css_class("tray-head");
        let i = Icon::new("rocket", 14);
        i.add_css_class("accent");
        head.append(&i);
        head.append(&gtk::Label::new(Some("OPENING")));
        card.append(&head);
        for (l, _) in &launches {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            row.add_css_class("tray-row");
            let open = gtk::Button::new();
            open.add_css_class("tray-open");
            open.set_hexpand(true);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let n = gtk::Label::new(Some(&l.name));
            n.add_css_class("tray-name");
            n.set_hexpand(true);
            n.set_xalign(0.0);
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.append(&n);
            let st = gtk::Label::new(Some(&launch_text(l)));
            st.add_css_class(match l.status {
                LaunchStatus::Done => "tray-done",
                LaunchStatus::Error => "tray-error",
                _ => "tray-pct",
            });
            line.append(&st);
            col.append(&line);
            if !l.status.finished() {
                for p in &l.parts {
                    let pr = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    pr.add_css_class("tray-part");
                    pr.append(&part_icon(p.state, &p.kind, 12));
                    let pn = gtk::Label::new(Some(if p.kind == "image" { "Image" } else { &p.name }));
                    pn.set_hexpand(true);
                    pn.set_xalign(0.0);
                    pn.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    pr.append(&pn);
                    if p.state == PartState::Working
                        && let Some(f) = p.progress
                    {
                        pr.append(&gtk::Label::new(Some(&format!("{}%", (f * 100.0).floor()))));
                    }
                    col.append(&pr);
                }
                let pb = progress(l.progress);
                pb.set_margin_top(6);
                col.append(&pb);
            }
            open.set_child(Some(&col));
            let id = l.id.clone();
            open.connect_clicked(move |_| launch_log(&id));
            row.append(&open);
            if l.status.finished() {
                let x = gtk::Button::new();
                x.add_css_class("tray-x");
                x.set_child(Some(&Icon::new("x", 14)));
                x.set_tooltip_text(Some("Dismiss"));
                let id = l.id.clone();
                x.connect_clicked(move |_| dismiss(&id));
                row.append(&x);
            }
            card.append(&row);
        }
        tray.append(&card);
    }
}

fn part_icon(state: PartState, kind: &str, px: i32) -> gtk::Widget {
    match state {
        PartState::Done => {
            let i = Icon::new("check", px);
            i.add_css_class("accent");
            i.upcast()
        }
        PartState::Error => {
            let i = Icon::new("circle-alert", px);
            i.add_css_class("accent-2");
            i.upcast()
        }
        PartState::Working => {
            let s = gtk::Spinner::new();
            s.start();
            s.set_size_request(px, px);
            s.upcast()
        }
        PartState::Waiting => {
            let i = Icon::new(if kind == "image" { "hard-drive" } else { "folder" }, px);
            i.add_css_class("faint");
            i.upcast()
        }
    }
}

/// A progress ring for the narrow sidebar.
fn ring(value: f64, icon: &'static str, count: usize, done: bool, tip: &str, on: impl Fn() + 'static) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("ring");
    b.set_halign(gtk::Align::Center);
    b.set_tooltip_text(Some(tip));
    let o = gtk::Overlay::new();
    let area = gtk::DrawingArea::new();
    area.set_content_width(40);
    area.set_content_height(40);
    area.set_draw_func(move |a, cr, w, h| {
        let (cx, cy, r) = (w as f64 / 2.0, h as f64 / 2.0, 15.0);
        let track = a.color();
        cr.set_line_width(2.5);
        cr.set_source_rgba(track.red() as f64, track.green() as f64, track.blue() as f64, 0.15);
        cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
        cr.set_source_rgba(track.red() as f64, track.green() as f64, track.blue() as f64, 1.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        let start = -std::f64::consts::FRAC_PI_2;
        cr.arc(cx, cy, r, start, start + std::f64::consts::TAU * value.clamp(0.0, 1.0));
        let _ = cr.stroke();
    });
    o.set_child(Some(&area));
    let i = Icon::new(if done { "check" } else { icon }, 16);
    o.add_overlay(&i);
    if count > 1 {
        let n = gtk::Label::new(Some(&count.to_string()));
        n.add_css_class("ring-count");
        n.set_halign(gtk::Align::End);
        n.set_valign(gtk::Align::Start);
        o.add_overlay(&n);
    }
    b.set_child(Some(&o));
    b.connect_clicked(move |_| on());
    b.upcast()
}

/// A log view (monospace, dark), following its end.
fn log_view(lines: &[String]) -> (gtk::ScrolledWindow, gtk::TextBuffer) {
    let view = gtk::TextView::new();
    view.add_css_class("log");
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    let buf = view.buffer();
    buf.set_text(&lines.join("\n"));
    let scroll = gtk::ScrolledWindow::new();
    scroll.add_css_class("log-frame");
    scroll.set_child(Some(&view));
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    (scroll, buf)
}

fn follow_end(scroll: &gtk::ScrolledWindow) {
    let s = scroll.clone();
    glib::idle_add_local_once(move || {
        let adj = s.vadjustment();
        adj.set_value(adj.upper());
    });
}

/// One build's log. Closing it never stops the build.
pub fn build_log(id: &str) {
    let Some((b, lines)) = store().read_now(|s| s.builds.iter().find(|(b, _)| b.id == id).cloned()) else { return };
    let modal = Modal::open(&b.name, None, 580);
    let body = modal.body().clone();
    let (scroll, buf) = log_view(&lines);
    scroll.set_size_request(-1, 256);
    body.append(&scroll);
    let bar = progress(b.progress);
    bar.set_margin_top(16);
    body.append(&bar);
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.set_halign(gtk::Align::End);
    foot.set_margin_top(20);
    body.append(&foot);
    follow_end(&scroll);
    let id = id.to_string();
    let m = modal.clone();
    store().watch(&body, Topic::ACTIVITY, move |s| {
        let Some((b, lines)) = s.builds.iter().find(|(b, _)| b.id == id) else { return };
        let text = lines.join("\n");
        if buf.text(&buf.start_iter(), &buf.end_iter(), false) != text {
            buf.set_text(&text);
            follow_end(&scroll);
        }
        bar.set_fraction(b.progress);
        ui::clear(&foot);
        if b.status.finished() {
            let d = ui::button("Dismiss", None, Variant::Ghost, Size::Md);
            let (did, dm) = (id.clone(), m.clone());
            d.connect_clicked(move |_| {
                dismiss(&did);
                dm.close();
            });
            foot.append(&d);
            if b.status == BuildStatus::Done && b.layer.is_some() {
                let go = ui::button("Open the Builder", None, Variant::Secondary, Size::Md);
                let m = m.clone();
                go.connect_clicked(move |_| {
                    m.close();
                    crate::shell::go(crate::shell::Page::Builder);
                });
                foot.append(&go);
            }
        } else {
            let c = ui::button("Cancel build", None, Variant::Ghost, Size::Md);
            let cid = id.clone();
            c.connect_clicked(move |_| cancel(format!("/v1/builds/{cid}")));
            foot.append(&c);
            let h = ui::button("Hide", None, Variant::Secondary, Size::Md);
            let m = m.clone();
            h.connect_clicked(move |_| m.close());
            foot.append(&h);
        }
    });
}

/// One launch, followed: the image and each project's folder getting ready,
/// then the start, with its log.
pub fn launch_log(id: &str) {
    let Some((l, lines)) = store().read_now(|s| s.launches.iter().find(|(l, _)| l.id == id).cloned()) else { return };
    let modal = Modal::open(
        &format!("Opening {}", l.name),
        Some("You can close this: it keeps going and the sidebar tracks it."),
        580,
    );
    let body = modal.body().clone();
    let parts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.append(&parts);
    let (scroll, buf) = log_view(&lines);
    scroll.set_size_request(-1, 192);
    scroll.set_margin_top(12);
    body.append(&scroll);
    let bar = progress(l.progress);
    bar.set_margin_top(16);
    body.append(&bar);
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.set_halign(gtk::Align::End);
    foot.set_margin_top(20);
    body.append(&foot);
    let id = id.to_string();
    let m = modal.clone();
    store().watch(&body, Topic::ACTIVITY, move |s| {
        let Some((l, lines)) = s.launches.iter().find(|(l, _)| l.id == id) else { return };
        ui::clear(&parts);
        for p in &l.parts {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("launch-part");
            row.append(&part_icon(p.state, &p.kind, 16));
            let n = gtk::Label::new(Some(if p.kind == "image" { "Its image" } else { &p.name }));
            n.set_hexpand(true);
            n.set_xalign(0.0);
            row.append(&n);
            let note = p.message.clone().or_else(|| {
                (p.state == PartState::Working)
                    .then_some(p.progress)
                    .flatten()
                    .map(|f| format!("{}%", (f * 100.0).floor()))
            });
            if let Some(t) = note {
                let l = gtk::Label::new(Some(&t));
                l.add_css_class("small-muted");
                row.append(&l);
            }
            parts.append(&row);
        }
        let text = lines.join("\n");
        if buf.text(&buf.start_iter(), &buf.end_iter(), false) != text {
            buf.set_text(&text);
            follow_end(&scroll);
        }
        bar.set_fraction(l.progress);
        ui::clear(&foot);
        if l.status.finished() {
            let d = ui::button("Dismiss", None, Variant::Ghost, Size::Md);
            let (id, m) = (id.clone(), m.clone());
            d.connect_clicked(move |_| {
                dismiss(&id);
                m.close();
            });
            foot.append(&d);
        } else {
            let c = ui::button("Cancel", None, Variant::Ghost, Size::Md);
            let cid = id.clone();
            c.connect_clicked(move |_| cancel(format!("/v1/launches/{cid}")));
            foot.append(&c);
            let h = ui::button("Hide", None, Variant::Secondary, Size::Md);
            let m = m.clone();
            h.connect_clicked(move |_| m.close());
            foot.append(&h);
        }
    });
}
