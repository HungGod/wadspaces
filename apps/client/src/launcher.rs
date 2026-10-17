//! The launcher (the web app's components/launcher/Launcher.tsx): the
//! wadspaces, each its own desktop, and what runs on the machine beside
//! them (WadBrowser, saved WadBrowser setups, the terminal). Pick any mix,
//! then launch, or launch in focus mode for a set time. A wadspace or a
//! WadBrowser can be customized first: only what a run changes (a
//! wadspace's background and keys; WadBrowser's web apps and their
//! sign-ins), then saved, or run once and thrown away. WadBrowser's also
//! opens any wadspace's own web apps as that wadspace has them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use wad_core::layers;
use wad_gtk::Icon;
use wad_proto::v1::ImageRow;

use crate::launch::{self, ImageState, RunItem};
use crate::modal::Modal;
use crate::slots::{self, Draft};
use crate::store::{AppState, store};
use crate::thumb::thumb;
use crate::ui::{self, Size, Variant};
use crate::{secret_slots, wallpaper_picker};

const PRESETS: [u32; 4] = [25, 50, 90, 120];

/// Draws a view's part again.
type Redraw = Rc<dyn Fn()>;
const BROWSER_SVG: &[u8] = include_bytes!("../../../assets/public/brand/wadbrowser-icon-dark.svg");

fn fmt_minutes(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn alive(v: &Value) -> bool {
    v.get("deletedAt").is_none_or(Value::is_null)
}

/// Something to pick.
#[derive(Clone)]
struct Entry {
    id: String,
    item: RunItem,
    label: String,
    detail: String,
    /// A space's stack, for its picture and its customizing.
    wadspace: Option<Value>,
    /// A WadBrowser's setup as a run starts from (`{id, name, mode,
    /// webapps, secrets}`), and the saved one's id.
    browser: Option<(Value, Option<String>)>,
    /// A throwaway run: listed until the launcher closes.
    once: bool,
}

fn browser_detail(mode: &str, webapps: &[Value]) -> String {
    let bar = if mode == "focus" { "No address bar" } else { "Address bar" };
    let apps: Vec<&str> = webapps.iter().filter_map(|w| w["name"].as_str()).collect();
    format!("{bar} · {}", if apps.is_empty() { "no web apps".to_string() } else { apps.join(", ") })
}

/// The WadBrowser every machine has. (Focus is how a wadspace's Extensions
/// set WadBrowser up, not another app.)
fn default_browsers() -> [Value; 1] {
    [json!({ "id": "wadbrowser", "name": "WadBrowser", "mode": "browser", "webapps": [], "secrets": {} })]
}

fn entries(st: &AppState, images: &HashMap<String, ImageRow>) -> (Vec<Entry>, Vec<Entry>) {
    let lib = st.library.as_ref().map(|l| l.lib()).unwrap_or_default();
    let spaces = st
        .wadspaces()
        .into_iter()
        .filter(|w| w["customization"].is_object())
        .map(|w| {
            let n = w["customization"]["projects"].as_array().map(Vec::len).unwrap_or(0);
            let count = format!("{n} project{}", if n == 1 { "" } else { "s" });
            let key = layers::stack_image_key(w);
            let resolves = !layers::resolve_stack(&lib, w).is_null();
            let detail = if !resolves {
                "Its template is gone".to_string()
            } else {
                match launch::image_state(&key, &lib, images) {
                    ImageState::Built => count,
                    ImageState::Update => format!("{count} · an update is available"),
                    ImageState::Never | ImageState::Changed => format!("{count} · its image builds first"),
                }
            };
            Entry {
                id: format!("w:{}", s(w, "id")),
                item: RunItem::Wadspace(s(w, "id").into()),
                label: st.wadspace_name(w),
                detail,
                wadspace: Some(w.clone()),
                browser: None,
                once: false,
            }
        })
        .collect();
    let mut apps: Vec<Entry> = default_browsers()
        .into_iter()
        .map(|b| Entry {
            id: format!("b:{}", s(&b, "id")),
            item: RunItem::App("browser".into()),
            label: s(&b, "name").into(),
            detail: browser_detail(s(&b, "mode"), &[]),
            wadspace: None,
            browser: Some((b, None)),
            once: false,
        })
        .collect();
    for b in st.library.iter().flat_map(|l| l.browsers.iter()).filter(|b| alive(b)) {
        let config = json!({ "id": b["id"], "name": b["name"], "mode": b["mode"], "webapps": b["webapps"], "secrets": b["secrets"] });
        apps.push(Entry {
            id: format!("b:{}", s(b, "id")),
            item: RunItem::Browser(b.clone()),
            label: b["name"].as_str().unwrap_or("WadBrowser").into(),
            detail: browser_detail(s(b, "mode"), b["webapps"].as_array().map(Vec::as_slice).unwrap_or(&[])),
            wadspace: None,
            browser: Some((config, Some(s(b, "id").into()))),
            once: false,
        });
    }
    apps.push(Entry {
        id: "terminal".into(),
        item: RunItem::Terminal,
        label: "Terminal".into(),
        detail: "foot, on the machine".into(),
        wadspace: None,
        browser: None,
        once: false,
    });
    (spaces, apps)
}

struct Flow {
    modal: Modal,
    picked: RefCell<Vec<String>>,
    all: RefCell<Vec<Entry>>,
    /// Throwaway runs made here.
    once: RefCell<Vec<Entry>>,
    images: RefCell<HashMap<String, ImageRow>>,
    cards: RefCell<Vec<(String, gtk::Widget, gtk::Box)>>,
    count: gtk::Label,
    launch: gtk::Button,
    focus: gtk::Button,
    minutes: Cell<u32>,
}

/// Opens the launcher.
pub fn open() {
    flow();
}

/// Opens the launcher straight into customizing a wadspace for a run
/// (`w:<id>`) or a WadBrowser (`b:<id>`, saved or one of the two built in).
pub fn customize(entry: &str) {
    let f = flow();
    let e = f.all.borrow().iter().find(|e| e.id == entry).cloned();
    match e {
        Some(Entry { wadspace: Some(w), .. }) => f.wadspace_run(w),
        Some(Entry { browser: Some((base, saved)), .. }) => f.browser_run(base, saved),
        _ => {}
    }
}

fn flow() -> Rc<Flow> {
    let modal = Modal::open("Launch", Some("Pick spaces and apps to open together."), 920);
    let count = gtk::Label::new(Some("Pick one or more"));
    let launch_b = ui::button("Launch", Some("play"), Variant::Primary, Size::Md);
    let focus_b = ui::button("Focus mode…", Some("lock"), Variant::Secondary, Size::Md);
    let f = Rc::new(Flow {
        modal,
        picked: RefCell::default(),
        all: RefCell::default(),
        once: RefCell::default(),
        images: RefCell::default(),
        cards: RefCell::default(),
        count,
        launch: launch_b,
        focus: focus_b,
        minutes: Cell::new(50),
    });
    let me = f.clone();
    f.launch.connect_clicked(move |_| me.go(None));
    let me = f.clone();
    f.focus.connect_clicked(move |_| me.focus_view());
    f.pick_view();
    // The images here say which spaces build first.
    let ff = f.clone();
    glib::spawn_future_local(async move {
        let rows = crate::pages::machines::get::<Vec<ImageRow>>("/v1/images".into()).await.unwrap_or_default();
        ff.images.replace(rows.into_iter().map(|r| (r.layer.clone(), r)).collect());
        // Only while picking (a customize view stays as it is).
        if ff.count.parent().is_some() {
            ff.pick_view();
        }
    });
    f
}

fn heading(title: &str, hint: Option<&str>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("launch-heading");
    let t = gtk::Label::new(Some(&title.to_uppercase()));
    t.add_css_class("launch-heading-title");
    t.set_xalign(0.0);
    b.append(&t);
    if let Some(h) = hint {
        let l = ui::wrapping(h);
        l.add_css_class("launch-heading-hint");
        b.append(&l);
    }
    b.upcast()
}

fn check(on: bool) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
    c.add_css_class("check");
    c.set_valign(gtk::Align::Center);
    if on {
        c.add_css_class("on");
        c.append(&Icon::new("check", 12));
    }
    c
}

/// A scrolling column for a view's body, at most `max` tall.
fn scroller(max: i32) -> (gtk::ScrolledWindow, gtk::Box) {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_max_content_height(max);
    scroll.set_propagate_natural_height(true);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 24);
    col.set_margin_end(4);
    scroll.set_child(Some(&col));
    (scroll, col)
}

impl Flow {
    fn pick_view(self: &Rc<Self>) {
        let body = self.modal.body();
        ui::clear(body);
        let images = self.images.borrow().clone();
        let (mut spaces, mut apps) = store().read_now(|st| entries(st, &images));
        let once = self.once.borrow().clone();
        spaces.extend(once.iter().filter(|e| e.item.wadspace().is_some()).cloned());
        // Throwaway WadBrowsers before the terminal.
        let term = apps.pop();
        apps.extend(once.iter().filter(|e| e.item.wadspace().is_none()).cloned());
        apps.extend(term);
        *self.all.borrow_mut() = spaces.iter().chain(apps.iter()).cloned().collect();
        // Picks that are gone (a deleted setup) go too.
        let all_ids: Vec<String> = self.all.borrow().iter().map(|e| e.id.clone()).collect();
        self.picked.borrow_mut().retain(|p| all_ids.contains(p));
        self.cards.borrow_mut().clear();
        let (scroll, col) = scroller(520);
        let sec = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sec.append(&heading(
            "Wadspaces",
            Some("Each its own desktop. Customize one to change its background or keys for a run."),
        ));
        if spaces.is_empty() {
            let empty = ui::wrapping("No wadspaces yet. Make one in the Builder.");
            empty.add_css_class("launch-empty");
            sec.append(&empty);
        } else {
            let grid = gtk::Grid::new();
            grid.set_column_homogeneous(true);
            grid.set_column_spacing(8);
            grid.set_row_spacing(8);
            for (n, e) in spaces.iter().enumerate() {
                grid.attach(&self.card(e, true), (n % 2) as i32, (n / 2) as i32, 1, 1);
            }
            if spaces.len() == 1 {
                grid.attach(&gtk::Box::new(gtk::Orientation::Vertical, 0), 1, 0, 1, 1);
            }
            sec.append(&grid);
        }
        col.append(&sec);
        let sec = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sec.append(&heading("On this machine", Some("On the machine itself, beside the spaces. Run as many WadBrowsers as you like, each set up its own way.")));
        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_column_spacing(8);
        grid.set_row_spacing(8);
        for (n, e) in apps.iter().enumerate() {
            grid.attach(&self.card(e, false), (n % 3) as i32, (n / 3) as i32, 1, 1);
        }
        sec.append(&grid);
        col.append(&sec);
        body.append(&scroll);

        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        foot.add_css_class("launch-foot");
        self.count.add_css_class("small-muted");
        self.count.set_hexpand(true);
        self.count.set_xalign(0.0);
        // Kept across views: out of the last footer first.
        for w in [self.count.upcast_ref::<gtk::Widget>(), self.focus.upcast_ref(), self.launch.upcast_ref()] {
            if let Some(b) = w.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
                b.remove(w);
            }
        }
        foot.append(&self.count);
        foot.append(&self.focus);
        foot.append(&self.launch);
        body.append(&foot);
        self.refresh();
    }

    /// Leaves the picking view: its footer's kept widgets come out of it.
    fn leave_pick(&self) {
        for w in [self.count.upcast_ref::<gtk::Widget>(), self.focus.upcast_ref(), self.launch.upcast_ref()] {
            if let Some(b) = w.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
                b.remove(w);
            }
        }
    }

    fn card(self: &Rc<Self>, e: &Entry, space: bool) -> gtk::Widget {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        card.add_css_class("pick");
        let b = gtk::Button::new();
        b.add_css_class("pick-main");
        b.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        if space {
            let t = store().read_now(|st| e.wadspace.as_ref().map(|w| thumb(st, w)));
            if let Some(t) = t {
                t.set_fixed_width(96);
                t.set_radius(12.0);
                t.set_screen(1920.0, 1080.0, 2.0);
                t.set_valign(gtk::Align::Center);
                // A run shows its own background.
                if let RunItem::WadspaceRun { wallpaper, .. } = &e.item {
                    let wall: wad_wallpaper::Wallpaper =
                        serde_json::from_value(wallpaper.clone()).unwrap_or_else(|_| wad_wallpaper::Wallpaper {
                            kind: wad_wallpaper::Kind::Gradient,
                            value: wad_wallpaper::BLANK.into(),
                        });
                    let paint = wall.paint();
                    t.set_wallpaper(&paint);
                    if let wad_wallpaper::Paint::Image(src) = &paint {
                        crate::images::wallpaper(src, crate::images::later(&t, |t, tex| t.set_picture(Some(tex))));
                    }
                }
                row.append(&t);
            }
        } else {
            let icon: gtk::Widget = if matches!(e.item, RunItem::Terminal) {
                let img = gtk::Image::new();
                img.set_pixel_size(32);
                crate::images::desktop_icon(
                    "/icons/terminal.png",
                    crate::images::later(&img, |i, t| i.set_paintable(Some(&t))),
                );
                img.upcast()
            } else {
                wad_gtk::Svg::new(BROWSER_SVG, 32, 32).upcast()
            };
            icon.set_valign(gtk::Align::Center);
            row.append(&icon);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let l = gtk::Label::new(Some(&e.label));
        l.add_css_class("pick-label");
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let d = gtk::Label::new(Some(&e.detail));
        d.add_css_class("pick-detail");
        d.set_xalign(0.0);
        d.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&l);
        text.append(&d);
        let saved = e.browser.as_ref().is_some_and(|(_, id)| id.is_some());
        if e.once || saved {
            let badge = crate::pages::badge(if e.once { "Throwaway run" } else { "Saved" }, None, false);
            if e.once {
                badge.add_css_class("pink");
            }
            badge.set_halign(gtk::Align::Start);
            badge.set_margin_top(4);
            text.append(&badge);
        }
        row.append(&text);
        let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        slot.set_valign(gtk::Align::Center);
        slot.append(&check(false));
        row.append(&slot);
        b.set_child(Some(&row));
        let (me, id) = (self.clone(), e.id.clone());
        b.connect_clicked(move |_| me.toggle(&id));
        card.append(&b);

        // Customize: a wadspace (if its template's there) or a WadBrowser.
        if !e.once {
            if let Some(w) = e.wadspace.clone() {
                let ok = store()
                    .read_now(|st| st.library.as_ref().is_some_and(|l| !layers::resolve_stack(&l.lib(), &w).is_null()));
                let c = ui::icon_button("sliders-horizontal", &format!("Customize {}", e.label), 16);
                c.set_sensitive(ok);
                let me = self.clone();
                c.connect_clicked(move |_| me.wadspace_run(w.clone()));
                card.append(&c);
            } else if let Some((base, saved_id)) = e.browser.clone() {
                let c = ui::icon_button("sliders-horizontal", &format!("Customize {}", e.label), 16);
                let me = self.clone();
                c.connect_clicked(move |_| me.browser_run(base.clone(), saved_id.clone()));
                card.append(&c);
            }
        }
        // Remove: a throwaway run, or delete a saved setup.
        if e.once {
            let x = ui::icon_button("x", &format!("Remove {}", e.label), 16);
            let (me, id) = (self.clone(), e.id.clone());
            x.connect_clicked(move |_| {
                me.once.borrow_mut().retain(|o| o.id != id);
                me.picked.borrow_mut().retain(|p| *p != id);
                me.pick_view();
            });
            card.append(&x);
        } else if let Some((_, Some(saved))) = &e.browser {
            let x = ui::icon_button("trash", &format!("Remove {}", e.label), 16);
            let (me, id, label) = (self.clone(), saved.clone(), e.label.clone());
            x.connect_clicked(move |_| me.delete_setup(&id, &label));
            card.append(&x);
        }
        self.cards.borrow_mut().push((e.id.clone(), card.clone().upcast(), slot));
        card.upcast()
    }

    /// Deletes a saved setup (after asking): softly, as the Builder does.
    fn delete_setup(self: &Rc<Self>, id: &str, label: &str) {
        let m = Modal::open(&format!("Delete the saved setup {label}?"), None, 440);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::End);
        let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
        let mm = m.clone();
        cancel.connect_clicked(move |_| mm.close());
        let del = ui::button("Delete", Some("trash"), Variant::Danger, Size::Md);
        let (mm, me, id, label) = (m.clone(), self.clone(), id.to_string(), label.to_string());
        del.connect_clicked(move |b| {
            let label = label.clone();
            b.set_sensitive(false);
            let doc = store()
                .read_now(|st| st.library.as_ref().and_then(|l| l.browsers.iter().find(|x| s_eq(x, &id)).cloned()));
            let (mm, me, b) = (mm.clone(), me.clone(), b.clone());
            glib::spawn_future_local(async move {
                let Some(mut doc) = doc else { return mm.close() };
                let now = launch::now_iso();
                doc["deletedAt"] = now.clone().into();
                doc["updatedAt"] = now.into();
                let kept = doc.clone();
                match crate::account::save_browser(doc).await {
                    Ok(()) => {
                        mm.close();
                        me.pick_view();
                        // Undo brings it back (deleted softly).
                        let me = me.clone();
                        crate::toast::with_action(
                            &format!("Deleted {label}"),
                            crate::toast::Tone::Info,
                            "Undo",
                            move || {
                                let mut doc = kept.clone();
                                if let Some(o) = doc.as_object_mut() {
                                    o.remove("deletedAt");
                                    o.insert("updatedAt".into(), launch::now_iso().into());
                                }
                                let me = me.clone();
                                glib::spawn_future_local(async move {
                                    match crate::account::save_browser(doc).await {
                                        Ok(()) => {
                                            if me.count.parent().is_some() {
                                                me.pick_view();
                                            }
                                        }
                                        Err(e) => crate::toast::error("Couldn't bring it back", &e),
                                    }
                                });
                            },
                        );
                    }
                    Err(e) => {
                        crate::toast::error("Can't delete it", &e);
                        b.set_sensitive(true);
                    }
                }
            });
        });
        row.append(&cancel);
        row.append(&del);
        m.body().append(&row);
    }

    fn toggle(self: &Rc<Self>, id: &str) {
        {
            let mut p = self.picked.borrow_mut();
            match p.iter().position(|x| x == id) {
                Some(i) => {
                    p.remove(i);
                }
                None => p.push(id.into()),
            }
        }
        self.refresh();
    }

    fn refresh(&self) {
        let picked = self.picked.borrow();
        for (id, card, slot) in self.cards.borrow().iter() {
            let on = picked.contains(id);
            if on {
                card.add_css_class("on");
            } else {
                card.remove_css_class("on");
            }
            ui::clear(slot);
            slot.append(&check(on));
        }
        self.count.set_text(&if picked.is_empty() {
            "Pick one or more".to_string()
        } else {
            format!("{} picked", picked.len())
        });
        self.launch.set_sensitive(!picked.is_empty());
        self.focus.set_sensitive(!picked.is_empty());
    }

    /// What's picked, spaces first, then what runs beside them.
    fn items(&self) -> Vec<RunItem> {
        let all = self.all.borrow();
        let picked: Vec<RunItem> = self
            .picked
            .borrow()
            .iter()
            .filter_map(|id| all.iter().find(|e| e.id == *id).map(|e| e.item.clone()))
            .collect();
        let (mut spaces, rest): (Vec<RunItem>, Vec<RunItem>) = picked.into_iter().partition(|i| i.wadspace().is_some());
        spaces.extend(rest);
        spaces
    }

    fn go(self: &Rc<Self>, focus: Option<u32>) {
        let items = self.items();
        if items.is_empty() {
            return;
        }
        self.launch.set_sensitive(false);
        self.focus.set_sensitive(false);
        let me = self.clone();
        glib::spawn_future_local(async move {
            match launch::launch(items, focus).await {
                Ok(_) => {
                    crate::toast::show("Launching", None, crate::toast::Tone::Success);
                    me.modal.close();
                }
                Err(e) => {
                    crate::toast::error("Couldn't launch", &e);
                    me.refresh();
                }
            }
        });
    }

    /// A throwaway run, listed and picked.
    fn add_once(self: &Rc<Self>, mut e: Entry) {
        e.id = format!("once:{}", glib::monotonic_time());
        e.once = true;
        self.picked.borrow_mut().push(e.id.clone());
        self.once.borrow_mut().push(e);
        self.pick_view();
    }

    fn focus_view(self: &Rc<Self>) {
        self.leave_pick();
        let body = self.modal.body();
        ui::clear(body);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 20);
        let hint = gtk::Label::new(None);
        let label = gtk::Box::new(gtk::Orientation::Vertical, 6);
        label.append(&ui::field_label("Focus for how long?", Some(&hint)));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let mut presets = vec![];
        for m in PRESETS {
            let b = gtk::Button::with_label(&fmt_minutes(m));
            b.add_css_class("preset");
            row.append(&b);
            presets.push((m, b));
        }
        let hours = gtk::SpinButton::with_range(0.0, 12.0, 1.0);
        hours.add_css_class("number");
        let mins = gtk::SpinButton::with_range(0.0, 59.0, 5.0);
        mins.add_css_class("number");
        let hl = gtk::Label::new(Some("h"));
        hl.add_css_class("small-muted-14");
        let ml = gtk::Label::new(Some("min"));
        ml.add_css_class("small-muted-14");
        row.append(&hours);
        row.append(&hl);
        row.append(&mins);
        row.append(&ml);
        label.append(&row);
        col.append(&label);
        let note = ui::wrapping(&format!(
            "Only these {} open until the time's up, and there's no ending it early.",
            self.picked.borrow().len()
        ));
        note.add_css_class("small-muted");
        col.append(&note);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        foot.add_css_class("launch-foot");
        let back = ui::button("Back", None, Variant::Ghost, Size::Md);
        back.set_hexpand(true);
        back.set_halign(gtk::Align::Start);
        let start = ui::button("Start focus", Some("timer"), Variant::Primary, Size::Md);
        foot.append(&back);
        foot.append(&start);
        col.append(&foot);
        body.append(&col);

        let presets = Rc::new(presets);
        let show = {
            let (me, hint, start, presets, hours, mins) =
                (self.clone(), hint.clone(), start.clone(), presets.clone(), hours.clone(), mins.clone());
            Rc::new(move || {
                let m = me.minutes.get();
                for (pm, b) in presets.iter() {
                    if *pm == m {
                        b.add_css_class("on");
                    } else {
                        b.remove_css_class("on");
                    }
                }
                hours.set_value((m / 60) as f64);
                mins.set_value((m % 60) as f64);
                let until = glib::DateTime::now_local()
                    .ok()
                    .and_then(|d| d.add_minutes(m as i32).ok())
                    .and_then(|d| d.format("%H:%M").ok());
                hint.set_text(&format!("until about {}", until.map(|g| g.to_string()).unwrap_or_default()));
                ui::set_text(&start, &format!("Start focus for {}", fmt_minutes(m)));
            })
        };
        show();
        for (m, b) in presets.iter() {
            let (me, show, m) = (self.clone(), show.clone(), *m);
            b.connect_clicked(move |_| {
                me.minutes.set(m);
                show();
            });
        }
        let set = {
            let (me, show, hours, mins) = (self.clone(), show.clone(), hours.clone(), mins.clone());
            Rc::new(move || {
                let m = (hours.value() as u32 * 60 + mins.value() as u32).clamp(1, 12 * 60);
                if m != me.minutes.get() {
                    me.minutes.set(m);
                    show();
                }
            })
        };
        let s1 = set.clone();
        hours.connect_value_changed(move |_| s1());
        mins.connect_value_changed(move |_| set());
        let me = self.clone();
        back.connect_clicked(move |_| me.pick_view());
        let me = self.clone();
        start.connect_clicked(move |_| me.go(Some(me.minutes.get())));
    }

    // ------------------------------------------------- customizing runs

    /// The view's head: its icon, "Customize <name>", what it changes.
    fn run_head(icon: gtk::Widget, title: &str, hint: &str) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        icon.set_valign(gtk::Align::Center);
        row.append(&icon);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let t = gtk::Label::new(Some(title));
        t.add_css_class("strong");
        t.set_xalign(0.0);
        texts.append(&t);
        let h = ui::wrapping(hint);
        h.add_css_class("small-muted");
        texts.append(&h);
        row.append(&texts);
        row.upcast()
    }

    /// Back, the problems over the buttons, and the buttons.
    fn run_foot(self: &Rc<Self>, buttons: &[&gtk::Button]) -> (gtk::Box, gtk::Box) {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
        col.add_css_class("launch-foot");
        let problems = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.append(&problems);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let back = ui::button("Back", None, Variant::Ghost, Size::Md);
        back.set_hexpand(true);
        back.set_halign(gtk::Align::Start);
        let me = self.clone();
        back.connect_clicked(move |_| me.pick_view());
        row.append(&back);
        for b in buttons {
            row.append(*b);
        }
        col.append(&row);
        (col, problems)
    }

    /// A wadspace's own options for a run: its background, and its keys by
    /// slot (WadBrowser's key groups stay its own). Saved as a new wadspace
    /// (a copy of its stack, at the board's end), or run once.
    fn wadspace_run(self: &Rc<Self>, w: Value) {
        self.leave_pick();
        let body = self.modal.body();
        ui::clear(body);
        let c = w["customization"].clone();
        let cname = s(&c, "name").to_string();
        let (lib, wadspaces) = store().read_now(|st| {
            (
                st.library.as_ref().map(|l| l.lib()).unwrap_or_default(),
                st.wadspaces().into_iter().cloned().collect::<Vec<_>>(),
            )
        });
        let r = layers::resolve_stack(&lib, &w);
        let slots: Vec<Value> = if r.is_null() {
            vec![]
        } else {
            layers::slots_for(&r["template"], &r["setup"])
                .into_iter()
                .filter(|sl| !s(sl, "key").starts_with("web:"))
                .collect()
        };
        let draft = Rc::new(RefCell::new(Draft {
            name: cname.clone(),
            secrets: slots::as_choices(&c["secrets"]["bySlot"]),
            browser: false,
        }));
        let wallpaper = Rc::new(RefCell::new(c["wallpaper"].clone()));

        let icon = Icon::new("sliders-horizontal", 20);
        icon.add_css_class("accent");
        body.append(&Self::run_head(
            icon.upcast(),
            &format!("Customize {cname}"),
            "Its background and keys for this run. Its apps, projects and image stay as they are.",
        ));
        let (scroll, col) = scroller(460);
        scroll.set_margin_top(20);
        let fields = gtk::Grid::new();
        fields.set_column_homogeneous(true);
        fields.set_column_spacing(12);
        let name_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        name_col.append(&ui::field_label("Name", None));
        let name = ui::entry("Needed to save it as a new wadspace");
        name_col.append(&name);
        let taken_l = gtk::Label::new(Some("There's a wadspace called that already."));
        taken_l.add_css_class("small-danger");
        taken_l.set_xalign(0.0);
        taken_l.set_visible(false);
        name_col.append(&taken_l);
        fields.attach(&name_col, 0, 0, 1, 1);
        let desc_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        desc_col.append(&ui::field_label("Description", None));
        let desc = ui::entry("");
        desc.set_text(s(&c, "description"));
        desc_col.append(&desc);
        fields.attach(&desc_col, 1, 0, 1, 1);
        col.append(&fields);
        let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bg.append(&heading("Background", None));
        let wp = wallpaper.clone();
        let pick = wallpaper_picker::picker(c["wallpaper"].clone(), move |v| {
            wp.replace(v);
        });
        pick.set_size_request(560, -1);
        pick.set_halign(gtk::Align::Start);
        bg.append(&pick);
        col.append(&bg);
        let keys = gtk::Box::new(gtk::Orientation::Vertical, 0);
        keys.append(&heading(
            "Keys",
            Some(
                "New and saved ones are kept on the machine. Don't save: sign in inside the app, and nothing is kept.",
            ),
        ));
        body.append(&scroll);

        let run_b = ui::button("Throwaway run", Some("play"), Variant::Secondary, Size::Md);
        let save_b = ui::button("Save as new wadspace", Some("plus"), Variant::Primary, Size::Md);
        let (foot, problems) = self.run_foot(&[&run_b, &save_b]);
        body.append(&foot);

        // What's wrong now, and what can be done.
        let check = {
            let (draft, slots, name, taken_l, run_b, save_b, problems, wadspaces) = (
                draft.clone(),
                slots.clone(),
                name.clone(),
                taken_l.clone(),
                run_b.clone(),
                save_b.clone(),
                problems.clone(),
                wadspaces.clone(),
            );
            let cname = cname.clone();
            Rc::new(move || {
                let typed = name.text().trim().to_string();
                // New keys are named after the run's name, else the wadspace's.
                draft.borrow_mut().name = if typed.is_empty() { cname.clone() } else { typed.clone() };
                let known = store().read_now(crate::signins::known);
                let res = slots::resolve(&draft.borrow(), &slots, &known);
                let taken = !typed.is_empty()
                    && wadspaces.iter().any(|x| {
                        x["customization"].is_object()
                            && s(&x["customization"], "name").trim().eq_ignore_ascii_case(&typed)
                    });
                taken_l.set_visible(taken);
                ui::clear(&problems);
                for p in &res.problems {
                    let l = ui::wrapping(p);
                    l.add_css_class("small-danger");
                    problems.append(&l);
                }
                run_b.set_sensitive(res.problems.is_empty());
                save_b.set_sensitive(!typed.is_empty() && !taken && res.problems.is_empty());
                save_b.set_tooltip_text(if typed.is_empty() { Some("Name it to save it") } else { None });
            })
        };
        if slots.is_empty() {
            let p = ui::wrapping("Nothing in it keeps a key of its own (WadBrowser's keys are its groups').");
            p.add_css_class("small-muted");
            keys.append(&p);
        } else {
            let ch = check.clone();
            keys.append(&secret_slots::view(
                slots.clone(),
                draft.clone(),
                "Sign in inside the app; nothing is saved on the machine (a throwaway run forgets it when it closes).",
                move || ch(),
            ));
        }
        col.append(&keys);
        let ch = check.clone();
        name.connect_changed(move |_| ch());
        check();

        {
            let (me, draft, slots, wallpaper, w, save_b2, check) = (
                self.clone(),
                draft.clone(),
                slots.clone(),
                wallpaper.clone(),
                w.clone(),
                save_b.clone(),
                check.clone(),
            );
            run_b.connect_clicked(move |b| {
                b.set_sensitive(false);
                save_b2.set_sensitive(false);
                let known = store().read_now(crate::signins::known);
                let res = slots::resolve(&draft.borrow(), &slots, &known);
                let (me, wallpaper, w, check) = (me.clone(), wallpaper.borrow().clone(), w.clone(), check.clone());
                glib::spawn_future_local(async move {
                    match slots::refs(&res).await {
                        Ok(by_slot) => {
                            let id = s(&w, "id").to_string();
                            let label = format!("{} (run)", s(&w["customization"], "name"));
                            me.add_once(Entry {
                                id: String::new(),
                                item: RunItem::WadspaceRun { id, wallpaper, by_slot: Value::Object(by_slot) },
                                label,
                                detail: "Its own background and keys, once".into(),
                                wadspace: Some(w),
                                browser: None,
                                once: true,
                            });
                        }
                        Err(e) => {
                            crate::toast::error("Couldn't start the run", &e);
                            check();
                        }
                    }
                });
            });
        }
        {
            let (me, draft, slots, wallpaper, run_b2, check) =
                (self.clone(), draft.clone(), slots.clone(), wallpaper.clone(), run_b.clone(), check.clone());
            let (name, desc) = (name.clone(), desc.clone());
            save_b.connect_clicked(move |b| {
                b.set_sensitive(false);
                run_b2.set_sensitive(false);
                let known = store().read_now(crate::signins::known);
                let res = slots::resolve(&draft.borrow(), &slots, &known);
                let (me, check) = (me.clone(), check.clone());
                let (n, d, wall, w, wadspaces) = (
                    name.text().trim().to_string(),
                    desc.text().trim().to_string(),
                    wallpaper.borrow().clone(),
                    w.clone(),
                    wadspaces.clone(),
                );
                glib::spawn_future_local(async move {
                    let made = async {
                        let by_slot = slots::refs(&res).await?;
                        let doc = new_wadspace(&w, &wadspaces, &n, &d, wall, Value::Object(by_slot));
                        let id = s(&doc, "id").to_string();
                        crate::account::save_wadspace(doc).await?;
                        Ok::<String, String>(id)
                    };
                    match made.await {
                        Ok(id) => {
                            me.picked.borrow_mut().push(format!("w:{id}"));
                            me.pick_view();
                        }
                        Err(e) => {
                            crate::toast::error("Couldn't save it", &e);
                            check();
                        }
                    }
                });
            });
        }
    }

    /// The wadspaces' own web apps, by wadspace: each opens in one click in
    /// a WadBrowser of its own, signed in as in its wadspace (None: no
    /// wadspace has any).
    fn from_wadspaces(self: &Rc<Self>) -> Option<gtk::Widget> {
        let groups: Vec<(String, String, Vec<Value>)> = store().read_now(|st| {
            let lib = st.library.as_ref().map(|l| l.lib()).unwrap_or_default();
            st.wadspaces()
                .into_iter()
                .filter(|w| w["customization"].is_object())
                .map(|w| (s(w, "id").to_string(), st.wadspace_name(w), launch::wadspace_webapps(&lib, w)))
                .filter(|(_, _, apps)| !apps.is_empty())
                .collect()
        });
        if groups.is_empty() {
            return None;
        }
        let sec = gtk::Box::new(gtk::Orientation::Vertical, 4);
        sec.add_css_class("from-wadspaces");
        sec.append(&heading(
            "From your wadspaces",
            Some("Their web apps, each signed in as it is in its wadspace. Opens right away."),
        ));
        for (id, name, apps) in groups {
            let g = gtk::Label::new(Some(&name));
            g.add_css_class("small-muted");
            g.set_xalign(0.0);
            g.set_margin_top(8);
            sec.append(&g);
            for wa in apps {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.add_css_class("webapp-row");
                let i = gtk::Image::new();
                i.set_pixel_size(16);
                crate::images::desktop_icon(
                    &layers::webapp_icon(s(&wa, "url")),
                    crate::images::later(&i, |i, t| i.set_paintable(Some(&t))),
                );
                row.append(&i);
                let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
                texts.set_hexpand(true);
                let n = gtk::Label::new(Some(s(&wa, "name")));
                n.add_css_class("small-14");
                n.set_xalign(0.0);
                n.set_ellipsize(gtk::pango::EllipsizeMode::End);
                texts.append(&n);
                let u = gtk::Label::new(Some(s(&wa, "url")));
                u.add_css_class("tiny-muted");
                u.set_xalign(0.0);
                u.set_ellipsize(gtk::pango::EllipsizeMode::End);
                texts.append(&u);
                row.append(&texts);
                let open = ui::button("Open", Some("play"), Variant::Secondary, Size::Sm);
                open.set_tooltip_text(Some(&format!("Open {} as {name} has it", s(&wa, "name"))));
                let (me, from, webapp, label) =
                    (self.clone(), id.clone(), s(&wa, "id").to_string(), format!("{name} · {}", s(&wa, "name")));
                open.connect_clicked(move |b| {
                    b.set_sensitive(false);
                    let item = RunItem::WadspaceWebApp { from: from.clone(), webapp: webapp.clone() };
                    let (me, b, label) = (me.clone(), b.clone(), label.clone());
                    glib::spawn_future_local(async move {
                        match launch::launch(vec![item], None).await {
                            Ok(_) => {
                                crate::toast::show(&format!("Opening {label}"), None, crate::toast::Tone::Success);
                                me.modal.close();
                            }
                            Err(e) => {
                                crate::toast::error("Couldn't open it", &e);
                                b.set_sensitive(true);
                            }
                        }
                    });
                });
                row.append(&open);
                sec.append(&row);
            }
        }
        Some(sec.upcast())
    }

    /// A WadBrowser run: web apps to open and their sign-ins (shared or their
    /// own), saved to launch later or run once; and, first, the wadspaces'
    /// own web apps, each opened as its wadspace has it in one click.
    fn browser_run(self: &Rc<Self>, base: Value, saved_id: Option<String>) {
        self.leave_pick();
        let body = self.modal.body();
        ui::clear(body);
        let base_name = s(&base, "name").to_string();
        let mode = Rc::new(RefCell::new(s(&base, "mode").to_string()));
        let webapps = Rc::new(RefCell::new(base["webapps"].as_array().cloned().unwrap_or_default()));
        let draft = Rc::new(RefCell::new(Draft {
            name: if saved_id.is_some() { base_name.clone() } else { String::new() },
            secrets: slots::as_choices(&base["secrets"]),
            browser: true,
        }));
        body.append(&Self::run_head(
            wad_gtk::Svg::new(BROWSER_SVG, 28, 28).upcast(),
            &format!("Customize {base_name}"),
            "Runs on the machine, beside the spaces. Run several at once, each set up its own way.",
        ));
        let (scroll, col) = scroller(460);
        scroll.set_margin_top(20);
        body.append(&scroll);

        let run_b = ui::button("Throwaway run", Some("play"), Variant::Secondary, Size::Md);
        let update_b = ui::button("Update", None, Variant::Secondary, Size::Md);
        update_b.set_visible(saved_id.is_some());
        let save_b = ui::button(
            if saved_id.is_some() { "Save as new setup" } else { "Save setup" },
            Some("plus"),
            Variant::Primary,
            Size::Md,
        );
        let (foot, problems) = self.run_foot(&[&run_b, &update_b, &save_b]);
        body.append(&foot);

        if let Some(from) = self.from_wadspaces() {
            col.append(&from);
        }
        let apps_sec = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.append(&apps_sec);
        let keys_sec = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.append(&keys_sec);
        let name_sec = gtk::Box::new(gtk::Orientation::Vertical, 6);
        name_sec.append(&ui::field_label("Name", None));
        let name = ui::entry("Needed to save this setup");
        if saved_id.is_some() {
            name.set_text(&base_name);
        }
        name_sec.append(&name);
        let taken_l = gtk::Label::new(Some("There's a setup called that already."));
        taken_l.add_css_class("small-danger");
        taken_l.set_xalign(0.0);
        taken_l.set_visible(false);
        name_sec.append(&taken_l);
        col.append(&name_sec);

        let check = {
            let (draft, webapps, name, taken_l, problems) =
                (draft.clone(), webapps.clone(), name.clone(), taken_l.clone(), problems.clone());
            let (run_b, update_b, save_b, saved_id, base_name) =
                (run_b.clone(), update_b.clone(), save_b.clone(), saved_id.clone(), base_name.clone());
            Rc::new(move || {
                let typed = name.text().trim().to_string();
                // New sign-ins are named after the setup; an unnamed one's
                // are WadSpaces-WadBrowser.<app>.
                draft.borrow_mut().name =
                    if typed.is_empty() && saved_id.is_some() { base_name.clone() } else { typed.clone() };
                let slots = layers::browser_slots(&json!({ "webapps": *webapps.borrow() }));
                let known = store().read_now(crate::signins::known);
                let res = slots::resolve(&draft.borrow(), &slots, &known);
                let taken_by = taken_names();
                let taken = |except: Option<&str>| !typed.is_empty() && layers::named(&typed, &taken_by, except);
                taken_l.set_visible(taken(saved_id.as_deref()));
                ui::clear(&problems);
                for p in &res.problems {
                    let l = ui::wrapping(p);
                    l.add_css_class("small-danger");
                    problems.append(&l);
                }
                let ok = res.problems.is_empty();
                run_b.set_sensitive(ok);
                update_b.set_sensitive(ok && !typed.is_empty() && !taken(saved_id.as_deref()));
                save_b.set_sensitive(ok && !typed.is_empty() && !taken(None));
                save_b.set_tooltip_text(if typed.is_empty() { Some("Name it to save it") } else { None });
            })
        };

        // Web apps and their keys, drawn again as the list changes.
        let redraw: Rc<RefCell<Option<Redraw>>> = Rc::default();
        let draw: Rc<dyn Fn()> = {
            let (apps_sec, keys_sec, webapps, draft, check, redraw) =
                (apps_sec.clone(), keys_sec.clone(), webapps.clone(), draft.clone(), check.clone(), redraw.clone());
            Rc::new(move || {
                let again = redraw.borrow().clone();
                draw_webapps(&apps_sec, &webapps, again.clone());
                ui::clear(&keys_sec);
                let slots = layers::browser_slots(&json!({ "webapps": *webapps.borrow() }));
                if !slots.is_empty() {
                    keys_sec.append(&heading(
                        "Sign-ins",
                        Some("Pick the same secret for several to share one cookie profile. Don't save: sign in in the window, and nothing is kept."),
                    ));
                    let ch = check.clone();
                    keys_sec.append(&secret_slots::view(
                        slots,
                        draft.clone(),
                        "Signs in in its window; nothing is saved on the machine, and it's forgotten when WadBrowser closes.",
                        move || ch(),
                    ));
                }
                check();
            })
        };
        redraw.replace(Some(draw.clone()));
        draw();
        let ch = check.clone();
        name.connect_changed(move |_| ch());

        // A config as the launcher runs it, and as it's saved.
        let config = {
            let (mode, webapps) = (mode.clone(), webapps.clone());
            Rc::new(
                move |id: &str, n: &str, secrets: serde_json::Map<String, Value>| json!({ "id": id, "name": n, "mode": *mode.borrow(), "webapps": *webapps.borrow(), "secrets": secrets }),
            )
        };
        let resolved = {
            let (draft, webapps) = (draft.clone(), webapps.clone());
            Rc::new(move || {
                let slots = layers::browser_slots(&json!({ "webapps": *webapps.borrow() }));
                let known = store().read_now(crate::signins::known);
                slots::resolve(&draft.borrow(), &slots, &known)
            })
        };
        {
            let (me, config, resolved, name, check) =
                (self.clone(), config.clone(), resolved.clone(), name.clone(), check.clone());
            run_b.connect_clicked(move |b| {
                b.set_sensitive(false);
                let res = resolved();
                let typed = name.text().trim().to_string();
                let n = if !typed.is_empty() { typed } else { "WadBrowser".to_string() };
                let (me, config, check) = (me.clone(), config.clone(), check.clone());
                glib::spawn_future_local(async move {
                    match slots::refs(&res).await {
                        Ok(secrets) => {
                            let c = config(&format!("run-{}", glib::monotonic_time()), &n, secrets);
                            let detail = browser_detail(
                                s(&c, "mode"),
                                c["webapps"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                            );
                            me.add_once(Entry {
                                id: String::new(),
                                item: RunItem::BrowserRun(c),
                                label: format!("{n} (run)"),
                                detail,
                                wadspace: None,
                                browser: None,
                                once: true,
                            });
                        }
                        Err(e) => {
                            crate::toast::error("Couldn't start the run", &e);
                            check();
                        }
                    }
                });
            });
        }
        let save = {
            let (me, name, check) = (self.clone(), name.clone(), check.clone());
            Rc::new(move |b: &gtk::Button, id: Option<String>| {
                b.set_sensitive(false);
                let res = resolved();
                let n = name.text().trim().to_string();
                let (me, config, check) = (me.clone(), config.clone(), check.clone());
                glib::spawn_future_local(async move {
                    let made = async {
                        let secrets = slots::refs(&res).await?;
                        let (taken, prev, owner) = store().read_now(|st| {
                            let l = st.library.clone().unwrap_or_default();
                            let prev = id.as_ref().and_then(|i| l.browsers.iter().find(|b| s_eq(b, i)).cloned());
                            (l.browsers, prev, st.account.user.as_ref().map(|u| u.uid.clone()).unwrap_or_default())
                        });
                        let id = id.unwrap_or_else(|| free_id(&n, &taken));
                        let mut doc = config(&id, &n, secrets);
                        let now = launch::now_iso();
                        doc["owner"] = owner.into();
                        doc["createdAt"] =
                            prev.as_ref().map(|p| p["createdAt"].clone()).unwrap_or_else(|| now.clone().into());
                        doc["updatedAt"] = now.into();
                        crate::account::save_browser(doc).await?;
                        Ok::<String, String>(id)
                    };
                    match made.await {
                        Ok(id) => {
                            let key = format!("b:{id}");
                            if !me.picked.borrow().contains(&key) {
                                me.picked.borrow_mut().push(key);
                            }
                            me.pick_view();
                        }
                        Err(e) => {
                            crate::toast::error("Couldn't save it", &e);
                            check();
                        }
                    }
                });
            })
        };
        let s2 = save.clone();
        update_b.connect_clicked(move |b| s2(b, saved_id.clone()));
        save_b.connect_clicked(move |b| save(b, None));
    }
}

/// A setup's web apps: each with Remove; those used elsewhere to add with a
/// click; and a name and address to add one.
fn draw_webapps(sec: &gtk::Box, webapps: &Rc<RefCell<Vec<Value>>>, again: Option<Redraw>) {
    ui::clear(sec);
    sec.append(&heading("Web apps", Some("Each opens in its own window.")));
    let redo = move || {
        if let Some(f) = &again {
            f();
        }
    };
    let redo = Rc::new(redo);
    let icon = |url: &str, px: i32| {
        let i = gtk::Image::new();
        i.set_pixel_size(px);
        crate::images::desktop_icon(
            &layers::webapp_icon(url),
            crate::images::later(&i, |i, t| i.set_paintable(Some(&t))),
        );
        i
    };
    for wa in webapps.borrow().iter() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("webapp-row");
        row.append(&icon(s(wa, "url"), 16));
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        texts.set_hexpand(true);
        let n = gtk::Label::new(Some(s(wa, "name")));
        n.add_css_class("small-14");
        n.set_xalign(0.0);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        texts.append(&n);
        let u = gtk::Label::new(Some(s(wa, "url")));
        u.add_css_class("tiny-muted");
        u.set_xalign(0.0);
        u.set_ellipsize(gtk::pango::EllipsizeMode::End);
        texts.append(&u);
        row.append(&texts);
        let x = ui::icon_button("trash", &format!("Remove {}", s(wa, "name")), 14);
        let (list, id, redo) = (webapps.clone(), s(wa, "id").to_string(), redo.clone());
        x.connect_clicked(move |_| {
            list.borrow_mut().retain(|w| s(w, "id") != id);
            redo();
        });
        row.append(&x);
        sec.append(&row);
    }
    // Web apps used elsewhere, to add with a click.
    let known: Vec<Value> = store().read_now(|st| {
        let lib = st.library.clone().unwrap_or_default();
        let ext: Vec<Value> = lib.extensions.iter().filter(|x| alive(x)).cloned().collect();
        let br: Vec<Value> = lib.browsers.iter().filter(|x| alive(x)).cloned().collect();
        layers::known_webapps(&ext, &br)
    });
    let have: Vec<String> = webapps.borrow().iter().map(|w| s(w, "url").to_string()).collect();
    let known: Vec<Value> = known.into_iter().filter(|w| !have.iter().any(|u| u == s(w, "url"))).collect();
    if !known.is_empty() {
        let chips = gtk::FlowBox::new();
        chips.add_css_class("chips");
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_column_spacing(6);
        chips.set_row_spacing(6);
        chips.set_max_children_per_line(12);
        for w in known {
            let b = gtk::Button::new();
            b.add_css_class("chip");
            b.set_tooltip_text(Some(&format!("Add {}", s(&w, "name"))));
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            r.append(&icon(s(&w, "url"), 14));
            r.append(&gtk::Label::new(Some(s(&w, "name"))));
            let plus = Icon::new("plus", 12);
            plus.add_css_class("faint");
            r.append(&plus);
            b.set_child(Some(&r));
            let (list, redo) = (webapps.clone(), redo.clone());
            b.connect_clicked(move |_| {
                add_webapp(&list, s(&w, "name"), s(&w, "url"));
                redo();
            });
            b.set_halign(gtk::Align::Start);
            chips.insert(&b, -1);
        }
        sec.append(&chips);
    }
    // A typed one.
    let form = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    form.set_margin_top(4);
    let an = ui::entry("Name");
    an.add_css_class("small");
    an.set_size_request(144, -1);
    let au = ui::entry("https://…");
    au.add_css_class("small");
    au.set_hexpand(true);
    let add = ui::button("Add", Some("plus"), Variant::Secondary, Size::Sm);
    add.set_sensitive(false);
    let enable = {
        let (an, au, add) = (an.clone(), au.clone(), add.clone());
        move || add.set_sensitive(!an.text().trim().is_empty() && !au.text().trim().is_empty())
    };
    let e2 = enable.clone();
    an.connect_changed(move |_| e2());
    au.connect_changed(move |_| enable());
    let typed = {
        let (an, au, list, redo) = (an.clone(), au.clone(), webapps.clone(), redo.clone());
        move || {
            let (n, mut u) = (an.text().trim().to_string(), au.text().trim().to_string());
            if n.is_empty() || u.is_empty() {
                return;
            }
            if !u.starts_with("http://") && !u.starts_with("https://") {
                u = format!("https://{u}");
            }
            add_webapp(&list, &n, &u);
            redo();
        }
    };
    let t2 = typed.clone();
    au.connect_activate(move |_| t2());
    add.connect_clicked(move |_| typed());
    form.append(&an);
    form.append(&au);
    form.append(&add);
    sec.append(&form);
}

fn s_eq(v: &Value, id: &str) -> bool {
    v.get("id").and_then(Value::as_str) == Some(id)
}

/// The names a setup can't take: saved ones' and the two built in.
fn taken_names() -> Vec<Value> {
    let mut out: Vec<Value> = store()
        .read_now(|st| st.library.as_ref().map(|l| l.browsers.iter().filter(|b| alive(b)).cloned().collect::<Vec<_>>()))
        .unwrap_or_default();
    out.extend(default_browsers());
    out
}

/// An id not taken, from a name (the web app's freeId).
fn free_id(name: &str, taken: &[Value]) -> String {
    let base = wad_core::spec::slugify(name);
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|x| s_eq(x, &id)) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A web app added to a setup's list (its id unique in it).
fn add_webapp(list: &Rc<RefCell<Vec<Value>>>, name: &str, url: &str) {
    let base = layers::webapp_id(name);
    let mut id = base.clone();
    let mut n = 2;
    while list.borrow().iter().any(|w| s(w, "id") == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    list.borrow_mut().push(json!({ "id": id, "name": name, "url": url }));
}

/// A copy of a stack with a customization of its own, at the board's end
/// (the web app's WadspaceRun save).
fn new_wadspace(
    w: &Value,
    wadspaces: &[Value],
    name: &str,
    description: &str,
    wallpaper: Value,
    by_slot: Value,
) -> Value {
    let mut doc = w.clone();
    let mut c = w["customization"].clone();
    c["name"] = name.into();
    c["description"] = description.into();
    c["wallpaper"] = wallpaper;
    c["secrets"]["bySlot"] = by_slot;
    // Every stack's id is taken, deleted ones too.
    let mut taken = wadspaces.to_vec();
    if let Some(l) = store().read_now(|st| st.library.as_ref().map(|l| l.wadspaces.clone())) {
        taken.extend(l);
    }
    let order = wadspaces.iter().filter_map(|x| x["order"].as_f64()).fold(0.0f64, f64::max) + 1.0;
    let now = launch::now_iso();
    if let Some(o) = doc.as_object_mut() {
        for k in ["owner", "deletedAt"] {
            o.remove(k);
        }
        o.insert("customization".into(), c);
        o.insert("id".into(), free_id(name, &taken).into());
        o.insert("version".into(), 3.into());
        o.insert("order".into(), order.into());
        o.insert("createdAt".into(), now.clone().into());
        o.insert("updatedAt".into(), now.into());
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_run_is_a_new_stack_at_the_boards_end() {
        let w = json!({ "id": "vanua", "version": 3, "order": 2, "owner": "me", "template": "dev",
            "customization": { "name": "Vanua", "secrets": { "bySlot": {}, "groups": [] }, "projects": [] } });
        let others = vec![w.clone(), json!({ "id": "cosmic", "order": 5 })];
        let doc = new_wadspace(
            &w,
            &others,
            "Vanua Late",
            "Nights",
            json!({ "type": "gradient", "value": "x" }),
            json!({ "k": 1 }),
        );
        assert_eq!(doc["id"], "vanua-late");
        assert_eq!(doc["order"], 6.0);
        assert_eq!(doc["template"], "dev");
        assert_eq!(doc["customization"]["name"], "Vanua Late");
        assert_eq!(doc["customization"]["secrets"]["bySlot"]["k"], 1);
        assert!(doc.get("owner").is_none());
    }

    #[test]
    fn ids_and_web_apps_stay_unique() {
        let taken = vec![json!({ "id": "mail" }), json!({ "id": "mail-2" })];
        assert_eq!(free_id("Mail", &taken), "mail-3");
        let list = Rc::new(RefCell::new(vec![]));
        add_webapp(&list, "Gmail", "https://mail.google.com");
        add_webapp(&list, "Gmail", "https://mail.google.com/u/1");
        let ids: Vec<String> = list.borrow().iter().map(|w| s(w, "id").to_string()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }
}
