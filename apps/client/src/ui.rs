//! The web app's building blocks (components/ui.tsx, AuthCard.tsx) as GTK
//! widgets with its look (style.css): buttons in its variants, icon
//! buttons, text fields, labels, notices, the glass card, panels, tabs
//! (Segmented), meters, menus, empty states.

use gtk::prelude::*;
use wad_gtk::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

/// A button: its text, an icon before it (or after it, for an arrow).
pub fn button(text: &str, icon: Option<&'static str>, variant: Variant, size: Size) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("btn");
    b.add_css_class(match variant {
        Variant::Primary => "primary",
        Variant::Secondary => "secondary",
        Variant::Ghost => "ghost",
        Variant::Danger => "danger",
    });
    b.add_css_class(match size {
        Size::Sm => "sm",
        Size::Md => "md",
        Size::Lg => "lg",
    });
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Center);
    let px = if size == Size::Sm { 14 } else { 16 };
    let after = icon.is_some_and(|i| i.starts_with("arrow-"));
    if let Some(i) = icon.filter(|_| !after) {
        // Play is filled, as lucide-react's fill-current draws it.
        row.append(&if i == "play" { Icon::filled(i, px) } else { Icon::new(i, px) });
    }
    if !text.is_empty() {
        row.append(&gtk::Label::new(Some(text)));
    }
    if let Some(i) = icon.filter(|_| after) {
        row.append(&Icon::new(i, px));
    }
    b.set_child(Some(&row));
    b
}

/// The button's text, changed.
pub fn set_text(b: &gtk::Button, text: &str) {
    if let Some(l) = b.child().and_then(|r| {
        let mut c = r.first_child();
        while let Some(w) = c {
            if let Ok(l) = w.clone().downcast::<gtk::Label>() {
                return Some(l);
            }
            c = w.next_sibling();
        }
        None
    }) {
        l.set_text(text);
    }
}

/// "Sign in with GitHub" with GitHub's mark (drawn here: it works offline).
pub fn github_button(text: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("github");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_halign(gtk::Align::Center);
    let mark = gtk::Image::new();
    mark.set_pixel_size(16);
    mark.add_css_class("github-mark");
    let show = {
        let mark = mark.clone();
        move |t: wad_gtk::Theme| {
            let fill = if t == wad_gtk::Theme::Dark { "#f5f2fa" } else { "#0a0614" };
            let svg = GITHUB_MARK.replace("currentColor", fill);
            if let Some(tex) = wad_gtk::icon::svg_texture(svg.as_bytes(), 32) {
                mark.set_paintable(Some(&tex));
            }
        }
    };
    show(wad_gtk::theme::shown());
    wad_gtk::theme::on_change(show);
    row.append(&mark);
    row.append(&gtk::Label::new(Some(text)));
    b.set_child(Some(&row));
    b
}

const GITHUB_MARK: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg>"#;

/// A text field.
pub fn entry(placeholder: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.add_css_class("input");
    e.set_placeholder_text(Some(placeholder));
    e
}

/// A field's name above it, with a hint at its end.
pub fn field_label(text: &str, hint: Option<&gtk::Label>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("field-label");
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    row.append(&l);
    if let Some(h) = hint {
        h.add_css_class("field-hint");
        row.append(h);
    }
    row.upcast()
}

/// A label that wraps to the width it's given (GTK would otherwise ask for
/// its whole text on one line).
pub fn wrapping(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_max_width_chars(1);
    l.set_hexpand(true);
    l.set_xalign(0.0);
    l
}

/// A message in a tinted box: `error` (the danger colour) or a note (the accent's).
pub fn notice(text: &str, error: bool) -> gtk::Label {
    let l = wrapping(text);
    l.add_css_class(if error { "notice-error" } else { "notice" });
    l
}

/// A spinner and words: something under way.
pub fn busy(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("busy");
    let s = gtk::Spinner::new();
    s.start();
    row.append(&s);
    row.append(&wrapping(text));
    row
}

/// The glass card the sign-in and setup screens hold their content in.
pub fn card(title: &str, subtitle: Option<&str>, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
    c.add_css_class("glass-card");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("card-title");
    t.set_xalign(0.0);
    c.append(&t);
    if let Some(s) = subtitle {
        let l = wrapping(s);
        l.add_css_class("card-subtitle");
        c.append(&l);
    }
    let b = body.upcast_ref::<gtk::Widget>();
    b.add_css_class("card-body");
    c.append(b);
    c
}

/// Empties a box.
pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// A square button with only an icon (the web app's IconButton); its label
/// is its tooltip, and what a screen reader says.
pub fn icon_button(icon: &'static str, label: &str, px: i32) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("icon-button");
    b.set_tooltip_text(Some(label));
    b.update_property(&[gtk::accessible::Property::Label(label)]);
    b.set_child(Some(&if icon == "play" || icon == "square" { Icon::filled(icon, px) } else { Icon::new(icon, px) }));
    b.set_valign(gtk::Align::Center);
    b
}

/// The icon button's icon, changed (a spinner's turn, say).
pub fn set_icon(b: &gtk::Button, icon: &'static str) {
    if let Some(i) = b.child().and_downcast::<Icon>() {
        i.set(icon);
    }
}

/// 512 B, 12 KB, 3.4 MB, 1.2 GB (the web app's formatBytes).
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n < 1e3 {
        format!("{} B", n.round())
    } else if n < 1e6 {
        format!("{} KB", (n / 1e3).round())
    } else if n < 1e9 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.1} GB", n / 1e9)
    }
}

/// 45 s, 12 min, 1.5 h (the web app's formatDuration).
pub fn duration(secs: f64) -> String {
    if secs < 90.0 {
        format!("{} s", secs.round().max(1.0))
    } else if secs < 5400.0 {
        format!("{} min", (secs / 60.0).round())
    } else {
        format!("{:.1} h", secs / 3600.0)
    }
}

/// A query string's value, percent-encoded.
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Choices side by side, one of them picked (the web app's Segmented):
/// `options` are (value, icon, label).
pub fn segmented(options: &[(&str, Option<&'static str>, &str)], value: &str, on: impl Fn(&str) + 'static) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("segmented");
    root.set_halign(gtk::Align::Start);
    let on = std::rc::Rc::new(on);
    let buttons: std::rc::Rc<std::cell::RefCell<Vec<(String, gtk::Button)>>> = Default::default();
    for (v, icon, label) in options {
        let b = gtk::Button::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::Center);
        if let Some(i) = icon {
            row.append(&Icon::new(i, 14));
        }
        row.append(&gtk::Label::new(Some(label)));
        b.set_child(Some(&row));
        if *v == value {
            b.add_css_class("on");
        }
        let (v2, on, all) = (v.to_string(), on.clone(), buttons.clone());
        b.connect_clicked(move |_| {
            for (bv, bb) in all.borrow().iter() {
                if *bv == v2 {
                    bb.add_css_class("on");
                } else {
                    bb.remove_css_class("on");
                }
            }
            on(&v2);
        });
        root.append(&b);
        buttons.borrow_mut().push((v.to_string(), b));
    }
    root
}

/// A dashed box saying there's nothing here (the web app's EmptyState).
pub fn empty_state(icon: &'static str, title: &str, body: Option<&str>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("empty-state");
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("icon-tile");
    tile.add_css_class("big");
    tile.set_halign(gtk::Align::Center);
    tile.append(&Icon::new(icon, 24));
    b.append(&tile);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("card-heading");
    b.append(&t);
    if let Some(body) = body {
        let l = gtk::Label::new(Some(body));
        l.add_css_class("small-muted-14");
        l.set_wrap(true);
        l.set_max_width_chars(48);
        l.set_justify(gtk::Justification::Center);
        b.append(&l);
    }
    b
}

/// A bar filled to a percentage (the web app's Meter); `pink`: accent-2's.
pub fn meter(percent: f64, pink: bool) -> gtk::ProgressBar {
    let p = gtk::ProgressBar::new();
    p.add_css_class("meter");
    if pink {
        p.add_css_class("pink");
    }
    p.set_fraction((percent / 100.0).clamp(0.0, 1.0));
    p
}

/// A card with a heading (its title, an icon before it, a hint under it,
/// room for buttons at its end) over its body: the web app's
/// `rounded-3xl border bg-surface` panels.
pub struct Panel {
    pub root: gtk::Box,
    pub actions: gtk::Box,
    pub body: gtk::Box,
}

pub fn panel(title: &str, icon: Option<&'static str>, hint: Option<&str>) -> Panel {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("panel");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    head.add_css_class("panel-head");
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some(i) = icon {
        let ic = Icon::new(i, 16);
        ic.add_css_class("accent");
        row.append(&ic);
    }
    let t = gtk::Label::new(Some(title));
    t.add_css_class("panel-title");
    t.set_xalign(0.0);
    row.append(&t);
    texts.append(&row);
    let h = wrapping(hint.unwrap_or(""));
    h.add_css_class("small-muted");
    h.set_visible(hint.is_some());
    texts.append(&h);
    head.append(&texts);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_valign(gtk::Align::Center);
    head.append(&actions);
    root.append(&head);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("panel-body");
    root.append(&body);
    Panel { root, actions, body }
}

/// A menu behind a button (the web app's Dropdown): (label, icon, danger,
/// what it does).
pub type MenuItem = (&'static str, &'static str, bool, Box<dyn Fn()>);

pub fn menu(icon: &'static str, label: &str, items: Vec<MenuItem>) -> gtk::MenuButton {
    let m = gtk::MenuButton::new();
    m.add_css_class("menu-button");
    m.set_tooltip_text(Some(label));
    m.update_property(&[gtk::accessible::Property::Label(label)]);
    m.set_child(Some(&Icon::new(icon, 16)));
    m.set_valign(gtk::Align::Center);
    let pop = gtk::Popover::new();
    pop.add_css_class("wad-menu");
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for (text, icon, danger, f) in items {
        let b = gtk::Button::new();
        b.add_css_class("menu-item");
        if danger {
            b.add_css_class("danger");
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&Icon::new(icon, 16));
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        row.append(&l);
        b.set_child(Some(&row));
        let p = pop.downgrade();
        b.connect_clicked(move |_| {
            if let Some(p) = p.upgrade() {
                p.popdown();
            }
            f();
        });
        col.append(&b);
    }
    pop.set_child(Some(&col));
    m.set_popover(Some(&pop));
    m
}

/// Runs `f` when `w` comes on screen, and every `secs` while it's there.
pub fn every(w: &impl IsA<gtk::Widget>, secs: u32, f: impl Fn() + 'static) {
    let f = std::rc::Rc::new(f);
    let ff = f.clone();
    w.connect_map(move |_| ff());
    let weak = w.upcast_ref::<gtk::Widget>().downgrade();
    gtk::glib::timeout_add_seconds_local(secs, move || {
        let Some(w) = weak.upgrade() else { return gtk::glib::ControlFlow::Break };
        if w.is_mapped() {
            f();
        }
        gtk::glib::ControlFlow::Continue
    });
}

/// Puts `text` on the clipboard.
pub fn copy(w: &impl IsA<gtk::Widget>, text: &str) {
    WidgetExt::display(w.upcast_ref::<gtk::Widget>()).clipboard().set_text(text);
}

/// A value to hand on: its name, the value (selectable, in mono), and a
/// copy button that says it copied.
pub fn copy_row(label: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("copy-row");
    let l = gtk::Label::new(Some(label));
    l.add_css_class("copy-label");
    l.set_xalign(0.0);
    l.set_width_chars(9);
    row.append(&l);
    let v = gtk::Label::new(Some(value));
    v.add_css_class("mono-14");
    v.set_selectable(true);
    v.set_hexpand(true);
    v.set_xalign(0.0);
    v.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    // A long link mustn't widen whatever holds it: its middle goes.
    v.set_max_width_chars(34);
    v.set_tooltip_text(Some(value));
    row.append(&v);
    let b = icon_button("copy", &format!("Copy the {}", label.to_lowercase()), 16);
    let text = value.to_string();
    b.connect_clicked(move |b| {
        copy(b, &text);
        set_icon(b, "check");
        let b = b.clone();
        gtk::glib::timeout_add_seconds_local_once(2, move || set_icon(&b, "copy"));
    });
    row.append(&b);
    row
}

/// A small round dot: "on" (the accent, glowing), "busy" (accent-2),
/// "error" (danger), else faint.
pub fn dot(state: &str) -> gtk::Box {
    let d = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    d.add_css_class("dot");
    d.add_css_class(state);
    d.set_valign(gtk::Align::Center);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_read_as_the_web_app_says_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(12_400), "12 KB");
        assert_eq!(bytes(3_420_000), "3.4 MB");
        assert_eq!(bytes(8_600_000_000), "8.6 GB");
        assert_eq!(duration(0.2), "1 s");
        assert_eq!(duration(600.0), "10 min");
        assert_eq!(duration(5400.0), "1.5 h");
        assert_eq!(enc("ws:a b/c"), "ws%3Aa%20b%2Fc");
    }
}
