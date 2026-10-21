//! The Builder's parts: a wadspace's projects and its keys (each editing
//! what its host keeps), and the small things they and the board share.

pub mod keys;
pub mod projects;

use std::cell::RefCell;

use gtk::{glib, prelude::*};

use crate::images;
use crate::ui;

thread_local! {
    /// A menu open in the Builder (Esc closes it first).
    static MENU: RefCell<glib::WeakRef<gtk::Popover>> = RefCell::default();
}

/// Small print, faint.
pub fn faint(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("faint");
    l.add_css_class("small");
    l
}

/// A small heading over a group.
pub fn group_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(&text.to_uppercase()));
    l.add_css_class("group-label");
    l.set_xalign(0.0);
    l
}

/// A note in the small print.
pub fn note(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("rail-note");
    l
}

/// A picture by path or URL, `px` square (WadSpaces' own or fetched).
pub fn img(src: &str, px: i32) -> gtk::Image {
    let i = gtk::Image::new();
    i.set_pixel_size(px);
    if !src.is_empty() {
        images::desktop_icon(src, images::later(&i, |i, t| i.set_paintable(Some(&t))));
    }
    i
}

/// A drag's ghost, in the Builder's look (its styles are the Builder's).
pub fn ghost(w: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("wb");
    b.append(w);
    b.upcast()
}

/// A menu opened in the Builder: Esc closes it first.
pub fn menu_opened(p: &gtk::Popover) {
    MENU.with(|m| m.borrow().set(Some(p)));
}

/// Esc: a menu open in the Builder closes (whether it did).
pub fn escape() -> bool {
    match MENU.with(|m| m.borrow().upgrade()).filter(|p| p.is_visible()) {
        Some(p) => {
            p.popdown();
            true
        }
        None => false,
    }
}
