//! Quiet notices from the profile avatar (the web app's ProfileNotices): a
//! build that's done (or didn't work), a launch that didn't start. A dot on
//! the avatar while some are unread, a bubble beside it for a few seconds
//! when one comes, and the list on a click.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, prelude::*};
use wad_gtk::Icon;

use crate::store::{Topic, store};
use crate::ui;

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: u64,
    pub title: String,
    pub body: String,
    /// The wadspace it's about (Open launches it).
    pub wadspace: Option<String>,
    /// Unix milliseconds.
    pub at: f64,
    pub read: bool,
}

/// Draws notices (`Some`: a new one came).
type View = Rc<dyn Fn(Option<&Notice>)>;

#[derive(Default)]
struct State {
    list: Vec<Notice>,
    seq: u64,
    /// Builds and launches seen, and whether they'd finished.
    seen: HashMap<String, bool>,
    /// What's drawn from it.
    views: Vec<View>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// A new notice (the newest twenty are kept).
pub fn notify(title: &str, body: &str, wadspace: Option<String>) {
    let n = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.seq += 1;
        let n = Notice { id: s.seq, title: title.into(), body: body.into(), wadspace, at: now_ms(), read: false };
        s.list.insert(0, n.clone());
        s.list.truncate(20);
        n
    });
    redraw(Some(&n));
}

fn redraw(new: Option<&Notice>) {
    let views: Vec<View> = STATE.with(|s| s.borrow().views.clone());
    for v in views {
        v(new);
    }
}

/// Signed out: their notices go.
pub fn reset() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.list.clear();
        s.seen.clear();
    });
    redraw(None);
}

fn read_all() {
    STATE.with(|s| s.borrow_mut().list.iter_mut().for_each(|n| n.read = true));
    redraw(None);
}

fn clear(id: u64) {
    STATE.with(|s| s.borrow_mut().list.retain(|n| n.id != id));
    redraw(None);
}

fn list() -> Vec<Notice> {
    STATE.with(|s| s.borrow().list.clone())
}

/// What a finished job says: Some(title, body, its wadspace).
type Said = Option<(String, String, Option<String>)>;

/// Builds and launches that finish (not those finished before the app
/// saw them) become notices.
fn take(builds: &[wad_proto::v1::Build], launches: &[wad_proto::v1::Launch]) -> Vec<Said> {
    use wad_proto::v1::{BuildStatus, LaunchStatus};
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut out = vec![];
        for b in builds {
            let done = b.status.finished();
            let was = s.seen.insert(format!("b:{}", b.id), done);
            if done && was == Some(false) {
                // A layer's build isn't a wadspace to open.
                let ws = b.layer.is_none().then(|| b.ws_id.clone());
                out.push(match b.status {
                    BuildStatus::Done => {
                        Some((format!("{} is built", b.name), "Its image is ready on the machine".into(), ws))
                    }
                    BuildStatus::Error => Some((
                        format!("{} didn't build", b.name),
                        b.error.clone().unwrap_or_else(|| "See the build log".into()),
                        ws,
                    )),
                    _ => None,
                });
            }
        }
        for l in launches {
            let done = l.status.finished();
            let was = s.seen.insert(format!("l:{}", l.id), done);
            if done && was == Some(false) && l.status == LaunchStatus::Error {
                out.push(Some((
                    format!("{} didn't start", l.name),
                    l.error.clone().unwrap_or_else(|| "See the launch log".into()),
                    Some(l.ws_id.clone()),
                )));
            }
        }
        out
    })
}

/// The avatar as a button: its dot, the bubble, the list.
pub fn avatar_button(avatar: &impl IsA<gtk::Widget>) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("avatar-button");
    b.set_valign(gtk::Align::Center);
    let o = gtk::Overlay::new();
    o.set_child(Some(avatar));
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("notice-dot");
    dot.set_halign(gtk::Align::End);
    dot.set_valign(gtk::Align::Start);
    dot.set_visible(false);
    o.add_overlay(&dot);
    b.set_child(Some(&o));
    b.set_tooltip_text(Some("Notifications"));

    let menu = gtk::Popover::new();
    menu.add_css_class("wad-menu");
    menu.add_css_class("notices");
    menu.set_has_arrow(false);
    menu.set_position(gtk::PositionType::Right);
    menu.set_parent(&b);
    let bubble = gtk::Popover::new();
    bubble.add_css_class("notice-bubble");
    bubble.set_has_arrow(true);
    bubble.set_autohide(false);
    bubble.set_can_focus(false);
    bubble.set_position(gtk::PositionType::Right);
    bubble.set_parent(&b);
    let hide_at = Rc::new(Cell::new(0u64));

    let (mw, bw) = (menu.downgrade(), bubble.downgrade());
    b.connect_clicked(move |_| {
        if let Some(bubble) = bw.upgrade() {
            bubble.popdown();
        }
        if let Some(m) = mw.upgrade() {
            fill_menu(&m);
            m.popup();
        }
        read_all();
    });
    let (dw, bw, bt) = (dot.downgrade(), b.downgrade(), bubble.downgrade());
    let view: View = Rc::new(move |new| {
        let (Some(dot), Some(b)) = (dw.upgrade(), bw.upgrade()) else { return };
        let unread = list().iter().filter(|n| !n.read).count();
        dot.set_visible(unread > 0);
        let label = match unread {
            0 => "Notifications".to_string(),
            1 => "1 new notification".to_string(),
            n => format!("{n} new notifications"),
        };
        b.set_tooltip_text(Some(&label));
        b.update_property(&[gtk::accessible::Property::Label(&label)]);
        // A bubble slides out for each new one, then tucks away again.
        if let (Some(n), Some(bubble)) = (new, bt.upgrade()) {
            fill_bubble(&bubble, n);
            if b.is_mapped() {
                bubble.popup();
            }
            let token = hide_at.get() + 1;
            hide_at.set(token);
            let (bw, h) = (bubble.downgrade(), hide_at.clone());
            glib::timeout_add_local_once(Duration::from_millis(6500), move || {
                if h.get() == token
                    && let Some(b) = bw.upgrade()
                {
                    b.popdown();
                }
            });
        }
    });
    STATE.with(|s| s.borrow_mut().views.push(view.clone()));
    // Jobs finishing.
    store().watch(&b, Topic::ACTIVITY, |s| {
        let builds: Vec<wad_proto::v1::Build> = s.builds.iter().map(|(b, _)| b.clone()).collect();
        let launches: Vec<wad_proto::v1::Launch> = s.launches.iter().map(|(l, _)| l.clone()).collect();
        let said = take(&builds, &launches);
        // Said after the store's done drawing (a notice draws too).
        if !said.is_empty() {
            glib::idle_add_local_once(move || {
                for (title, body, ws) in said.into_iter().flatten() {
                    notify(&title, &body, ws);
                }
            });
        }
    });
    b
}

fn open(ws: &str) {
    let id = ws.to_string();
    glib::spawn_future_local(async move {
        if let Err(e) = crate::launch::launch(vec![crate::launch::RunItem::Wadspace(id)], None).await {
            crate::toast::error("Couldn't open it", &e);
        }
    });
}

fn fill_bubble(bubble: &gtk::Popover, n: &Notice) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("bubble-row");
    let i = Icon::new("circle-check", 16);
    i.add_css_class("accent");
    row.append(&i);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
    texts.set_hexpand(true);
    let t = gtk::Label::new(Some(&n.title));
    t.add_css_class("notice-title");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_max_width_chars(34);
    texts.append(&t);
    if !n.body.is_empty() {
        let b = gtk::Label::new(Some(&n.body));
        b.add_css_class("notice-body");
        b.set_xalign(0.0);
        b.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.set_max_width_chars(34);
        texts.append(&b);
    }
    row.append(&texts);
    if let Some(ws) = n.wadspace.clone() {
        let o = ui::button("Open", None, ui::Variant::Primary, ui::Size::Sm);
        o.set_valign(gtk::Align::Center);
        let bw = bubble.downgrade();
        o.connect_clicked(move |_| {
            if let Some(b) = bw.upgrade() {
                b.popdown();
            }
            read_all();
            open(&ws);
        });
        row.append(&o);
    }
    let x = ui::icon_button("x", "Hide", 14);
    x.set_valign(gtk::Align::Center);
    let bw = bubble.downgrade();
    x.connect_clicked(move |_| {
        if let Some(b) = bw.upgrade() {
            b.popdown();
        }
    });
    row.append(&x);
    bubble.set_child(Some(&row));
}

fn fill_menu(menu: &gtk::Popover) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.set_size_request(320, -1);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("notices-head");
    let i = Icon::new("bell", 16);
    i.add_css_class("accent");
    head.append(&i);
    head.append(&gtk::Label::new(Some("Notifications")));
    col.append(&head);
    let all = list();
    if all.is_empty() {
        let e = gtk::Label::new(Some("You're all caught up."));
        e.add_css_class("small-muted-14");
        e.set_margin_top(20);
        e.set_margin_bottom(20);
        col.append(&e);
    }
    let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for n in all {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("notice-row");
        let i = Icon::new("circle-check", 16);
        i.add_css_class("accent");
        row.append(&i);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        texts.set_hexpand(true);
        let t = gtk::Label::new(Some(&n.title));
        t.add_css_class("notice-title");
        t.set_xalign(0.0);
        t.set_ellipsize(gtk::pango::EllipsizeMode::End);
        texts.append(&t);
        let b = gtk::Label::new(Some(&format!("{} · {}", n.body, crate::pages::ago_ms(n.at))));
        b.add_css_class("notice-body");
        b.set_xalign(0.0);
        b.set_ellipsize(gtk::pango::EllipsizeMode::End);
        texts.append(&b);
        row.append(&texts);
        if let Some(ws) = n.wadspace.clone() {
            let o = ui::icon_button("play", &format!("Open {}", n.title), 14);
            let mw = menu.downgrade();
            o.connect_clicked(move |_| {
                if let Some(m) = mw.upgrade() {
                    m.popdown();
                }
                open(&ws);
            });
            row.append(&o);
        }
        let x = ui::icon_button("x", "Clear", 14);
        x.add_css_class("notice-clear");
        let (mw, id) = (menu.downgrade(), n.id);
        x.connect_clicked(move |_| {
            clear(id);
            if let Some(m) = mw.upgrade() {
                fill_menu(&m);
            }
        });
        row.append(&x);
        items.append(&row);
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_max_content_height(320);
    sw.set_propagate_natural_height(true);
    sw.set_child(Some(&items));
    col.append(&sw);
    menu.set_child(Some(&col));
}

#[cfg(test)]
mod tests {
    use super::*;
    use wad_proto::v1::{Build, BuildStatus};

    fn build(id: &str, status: BuildStatus, layer: Option<&str>) -> Build {
        serde_json::from_value(serde_json::json!({
            "id": id, "wsId": "w1", "name": "Site", "status": status, "progress": 0.0, "error": null,
            "image": "", "baseImage": "", "created": 0.0, "started": null, "finished": null,
            "updated": false, "restartRequired": false, "skipped": [], "lineCount": 0, "layer": layer,
        }))
        .unwrap()
    }

    #[test]
    fn builds_that_finish_are_told_once() {
        // Finished before it was seen: nothing.
        assert!(take(&[build("old", BuildStatus::Done, None)], &[]).is_empty());
        assert!(take(&[build("b1", BuildStatus::Building, None)], &[]).is_empty());
        let said = take(&[build("b1", BuildStatus::Done, None)], &[]);
        assert_eq!(
            said,
            vec![Some(("Site is built".into(), "Its image is ready on the machine".into(), Some("w1".into())))]
        );
        assert!(take(&[build("b1", BuildStatus::Done, None)], &[]).is_empty());
        // A layer's build: nothing to open.
        take(&[build("b2", BuildStatus::Building, Some("template:dev"))], &[]);
        let said = take(&[build("b2", BuildStatus::Error, Some("template:dev"))], &[]);
        assert_eq!(said[0].as_ref().map(|s| (s.0.as_str(), s.2.is_none())), Some(("Site didn't build", true)));
    }
}
