//! Preview (the web app's builder/Preview.tsx): a stack's finished desktop,
//! inside the Builder. Nothing is launched; clicking an icon opens a
//! stand-in window saying what that app comes set up with.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use wad_board::catalog::{self, DesktopIcon};
use wad_board::*;
use wad_gtk::Icon;

use super::{editor, faces, state};
use crate::ui;

thread_local! {
    static OPEN: RefCell<Option<Rc<Preview>>> = const { RefCell::new(None) };
}

struct Preview {
    root: gtk::Overlay,
    stack: Stack,
    window: gtk::Box,
    icons: RefCell<Vec<(String, gtk::Widget)>>,
}

/// Previews a stack.
pub fn open(stack_id: &str) {
    close();
    let Some(stack) = state::read(|s| s.board.stack(stack_id).cloned()) else { return };
    let root = gtk::Overlay::new();
    let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scrim.add_css_class("editor-scrim");
    root.set_child(Some(&scrim));
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| {
        close();
    });
    scrim.add_controller(click);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("wb");
    card.add_css_class("preview");
    card.set_overflow(gtk::Overflow::Hidden);
    card.set_margin_start(24);
    card.set_margin_end(24);
    card.set_margin_top(24);
    card.set_margin_bottom(24);
    root.add_overlay(&card);
    root.connect_get_child_position(|o, _| Some(gtk::gdk::Rectangle::new(0, 0, o.width(), o.height())));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    head.add_css_class("preview-head");
    head.append(&Icon::new("eye", 16));
    let t = gtk::Label::new(Some(&stack_name(&stack)));
    t.add_css_class("preview-title");
    head.append(&t);
    let n = gtk::Label::new(Some("Preview · nothing is launched"));
    n.add_css_class("preview-note");
    n.set_hexpand(true);
    n.set_xalign(0.0);
    head.append(&n);
    let x = ui::icon_button("x", "Close", 16);
    x.connect_clicked(|_| {
        close();
    });
    head.append(&x);
    card.append(&head);

    let desk = gtk::Overlay::new();
    desk.add_css_class("preview-desktop");
    desk.set_vexpand(true);
    desk.set_child(Some(&faces::wallpaper_sized(stack.look().map(|l| &l.wallpaper), true)));
    let window = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.set_halign(gtk::Align::Center);
    window.set_valign(gtk::Align::Center);
    window.set_margin_bottom(40);
    let me = Rc::new(Preview {
        root: root.clone(),
        stack: stack.clone(),
        window: window.clone(),
        icons: RefCell::default(),
    });

    let icons = catalog::desktop_of(&stack);
    let grid = gtk::FlowBox::new();
    grid.add_css_class("pv-icons");
    grid.set_orientation(gtk::Orientation::Vertical);
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);
    grid.set_valign(gtk::Align::Fill);
    grid.set_halign(gtk::Align::Start);
    grid.set_margin_start(18);
    grid.set_margin_top(18);
    grid.set_margin_bottom(62);
    for i in &icons {
        let b = gtk::Button::new();
        b.add_css_class("pv-icon");
        let col = gtk::Box::new(gtk::Orientation::Vertical, 5);
        let im = faces::img(&i.icon, 46);
        im.set_halign(gtk::Align::Center);
        col.append(&im);
        let l = gtk::Label::new(Some(&i.label));
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_max_width_chars(11);
        col.append(&l);
        b.set_child(Some(&col));
        b.set_size_request(88, -1);
        let (w, icon) = (Rc::downgrade(&me), i.clone());
        b.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.show(&icon);
            }
        });
        me.icons.borrow_mut().push((i.id.clone(), b.clone().upcast()));
        grid.insert(&b, -1);
    }
    if icons.is_empty() {
        let e = gtk::Label::new(Some("Nothing on the desktop yet."));
        e.add_css_class("empty-note");
        grid.insert(&e, -1);
    }
    desk.add_overlay(&grid);
    desk.add_overlay(&window);
    desk.add_overlay(&taskbar(&me, &icons));
    card.append(&desk);

    OPEN.with(|o| o.replace(Some(me)));
    editor::layer_show(&root);
    let s = scrim.clone();
    glib::idle_add_local_once(move || s.add_css_class("shown"));
}

/// Closes the preview; whether one was open.
pub fn close() -> bool {
    let Some(me) = OPEN.with(|o| o.borrow_mut().take()) else { return false };
    editor::layer_hide(&me.root);
    true
}

fn taskbar(me: &Rc<Preview>, icons: &[DesktopIcon]) -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.add_css_class("pv-taskbar");
    bar.set_valign(gtk::Align::End);
    for i in icons.iter().filter(|i| i.id.starts_with("app:")) {
        let b = gtk::Button::new();
        b.add_css_class("pv-tb-app");
        b.set_child(Some(&faces::img(&i.icon, 24)));
        b.set_tooltip_text(Some(&i.label));
        let (w, icon) = (Rc::downgrade(me), i.clone());
        b.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.show(&icon);
            }
        });
        bar.append(&b);
    }
    let spring = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spring.set_hexpand(true);
    bar.append(&spring);
    let folders = me.stack.folders().map(|f| f.mounts.clone()).unwrap_or_default();
    if !folders.is_empty() {
        let t = tray("folder", &folders.len().to_string());
        let names: Vec<&str> = folders.iter().filter_map(|m| m["as"].as_str()).collect();
        t.set_tooltip_text(Some(&names.join(", ")));
        bar.append(&t);
    }
    let (all, set) = catalog::key_count(&me.stack);
    if all > 0 {
        let t = tray("key-round", &format!("{set}/{all}"));
        t.set_tooltip_text(Some("Saved sign-ins"));
        bar.append(&t);
    }
    let clock = gtk::Label::new(Some("9:41"));
    clock.add_css_class("pv-clock");
    bar.append(&clock);
    bar.upcast()
}

fn tray(icon: &'static str, text: &str) -> gtk::Box {
    let t = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    t.add_css_class("pv-tray");
    t.append(&Icon::new(icon, 12));
    t.append(&gtk::Label::new(Some(text)));
    t
}

fn p(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("mw-p");
    l
}

fn faint(text: &str) -> gtk::Label {
    let l = p(text);
    l.add_css_class("faint");
    l
}

impl Preview {
    /// A stand-in window for an app, web app or folder.
    fn show(self: &Rc<Self>, icon: &DesktopIcon) {
        for (id, w) in self.icons.borrow().iter() {
            if *id == icon.id {
                w.add_css_class("on");
            } else {
                w.remove_css_class("on");
            }
        }
        ui::clear(&self.window);
        let win = gtk::Box::new(gtk::Orientation::Vertical, 0);
        win.add_css_class("mock-window");
        win.set_size_request(460, -1);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("mw-bar");
        bar.append(&faces::img(&icon.icon, 16));
        let t = gtk::Label::new(Some(&icon.label));
        t.set_hexpand(true);
        t.set_xalign(0.0);
        bar.append(&t);
        let x = ui::icon_button("x", "Close", 12);
        let w = Rc::downgrade(self);
        x.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                ui::clear(&me.window);
                for (_, w) in me.icons.borrow().iter() {
                    w.remove_css_class("on");
                }
            }
        });
        bar.append(&x);
        win.append(&bar);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.add_css_class("mw-body");
        for w in self.lines(icon) {
            body.append(&w);
        }
        win.append(&body);
        self.window.append(&win);
    }

    fn lines(&self, icon: &DesktopIcon) -> Vec<gtk::Widget> {
        let s = &self.stack;
        let (kind, id) = icon.id.split_once(':').unwrap_or(("app", icon.id.as_str()));
        let by_slot = s.secrets().map(|x| x.by_slot.clone()).unwrap_or_default();
        let key_name = |k: &serde_json::Value| k["name"].as_str().unwrap_or_default().to_string();
        match (kind, id) {
            ("web", id) => {
                let w = s.browser().and_then(|b| b.webapps.iter().find(|x| x.id == id).cloned());
                let url = gtk::Label::new(w.as_ref().map(|w| w.url.as_str()));
                url.add_css_class("mw-url");
                url.set_xalign(0.0);
                url.set_ellipsize(gtk::pango::EllipsizeMode::End);
                // A web app signs in with its key group's key.
                let k = catalog::web_key(s, &format!("web:{id}"));
                vec![
                    url.upcast(),
                    p(&match k {
                        Some(k) => format!("Signed in with {}", key_name(&k)),
                        None => "Not signed in: this sign-in isn't saved, so it ends when the wadspace stops.".into(),
                    })
                    .upcast(),
                ]
            }
            ("dir", id) => {
                let m = s.folders().and_then(|f| f.mounts.iter().find(|m| m["id"] == id).cloned()).unwrap_or_default();
                let as_ = m["as"].as_str().unwrap_or_default();
                let text = match m["kind"].as_str() {
                    Some("github") => format!(
                        "~/Desktop/{as_}: cloned from {}. The files live in this wadspace.",
                        m["repo"].as_str().unwrap_or_default()
                    ),
                    Some("mount") => format!(
                        "~/Desktop/{as_}: {} on its machine, mounted. Changes save there, and only there has it.",
                        m["source"]["paths"][0].as_str().unwrap_or_default()
                    ),
                    _ => {
                        let files = m["files"].as_u64().unwrap_or(0);
                        format!(
                            "~/Desktop/{as_}: copied from this machine{}. The files live in this wadspace.",
                            if files > 0 { format!(" ({files} files)") } else { String::new() }
                        )
                    }
                };
                vec![p(&text).upcast()]
            }
            (_, "vscode") => {
                let exts = s.vscode().map(|v| v.extensions.clone()).unwrap_or_default();
                let tools = s.extensions.as_ref().and_then(tools_of).map(|t| t.tools.clone()).unwrap_or_default();
                let mut out: Vec<gtk::Widget> =
                    vec![p(if exts.is_empty() { "No extensions." } else { "Extensions:" }).upcast()];
                for e in &exts {
                    let x = catalog::extension(e);
                    let signed = x.secret.as_ref().map(|_| match by_slot.get(&format!("ext:{e}")) {
                        Some(k) => format!(" · signed in ({})", key_name(k)),
                        None => " · not signed in".into(),
                    });
                    let l = p(&format!("•  {}{}", x.name, signed.unwrap_or_default()));
                    l.add_css_class("mw-li");
                    out.push(l.upcast());
                }
                if !tools.is_empty() {
                    let names: Vec<String> = tools.iter().map(|t| catalog::tool_name(t)).collect();
                    out.push(faint(&format!("Terminal tools: {}", names.join(", "))).upcast());
                }
                out
            }
            (_, "wadbrowser") => {
                let b = s.browser();
                let k = catalog::web_key(s, "browser");
                vec![
                    p(&match b {
                        Some(b) => format!(
                            "{}, {} web app{}.",
                            if b.mode == "focus" {
                                "Focus mode (web apps only)"
                            } else {
                                "Browser with an address bar"
                            },
                            b.webapps.len(),
                            if b.webapps.len() == 1 { "" } else { "s" }
                        ),
                        None => "A plain browser.".into(),
                    })
                    .upcast(),
                    faint(&match k {
                        Some(k) => format!("Profile: {}", key_name(&k)),
                        None => "No key: signed out when the wadspace stops.".into(),
                    })
                    .upcast(),
                ]
            }
            (_, id) => {
                let a = catalog::app(id);
                let mut out: Vec<gtk::Widget> =
                    vec![p(&a.as_ref().map(|a| a.summary.clone()).unwrap_or_default()).upcast()];
                if let Some(kind) = a.as_ref().and_then(|a| a.secret.clone()) {
                    let label = crate::signins::kind_label(&kind);
                    out.push(
                        faint(&match by_slot.get(&format!("app:{id}")) {
                            Some(k) => format!("{label}: {}", key_name(k)),
                            None => format!("{label} isn't saved."),
                        })
                        .upcast(),
                    );
                }
                out
            }
        }
    }
}

/// For the snapshots: an icon opened.
pub fn sim_show(icon_id: &str) {
    let Some(me) = OPEN.with(|o| o.borrow().clone()) else { return };
    if let Some(i) = catalog::desktop_of(&me.stack).into_iter().find(|i| i.id == icon_id) {
        me.show(&i);
    }
}
