//! The Builder's parts both boards use: a wadspace's projects and its keys
//! (the old board's Customization editor, and Builder B's sidebar), each
//! editing what its host keeps.

pub mod keys;
pub mod projects;

use gtk::prelude::*;

use crate::ui;

/// Small print, faint.
pub fn faint(text: &str) -> gtk::Label {
    let l = ui::wrapping(text);
    l.add_css_class("faint");
    l.add_css_class("small");
    l
}
