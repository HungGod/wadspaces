//! A wadspace's background: one of the textures, in the app's theme.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use serde_json::{Value, json};
use wad_gtk::DesktopThumb;
use wad_wallpaper::{Paint, Wallpaper};

use crate::images;

/// The picker for `value`; `on` hears each pick (`{type: "texture", value}`).
pub fn texture_picker(value: Value, on: impl Fn(Value) + 'static) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.add_css_class("wallpapers");
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(8);
    grid.set_row_spacing(8);
    let me = Rc::new(Picker { grid: grid.clone(), value: RefCell::new(value), on: Box::new(on) });
    me.render();
    grid
}

struct Picker {
    grid: gtk::Grid,
    value: RefCell<Value>,
    on: Box<dyn Fn(Value)>,
}

fn swatch(w: &Value, label: &str, on: bool) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("swatch-button");
    if on {
        b.add_css_class("on");
    }
    b.set_tooltip_text(Some(label));
    let t = DesktopThumb::new();
    t.set_radius(12.0);
    let wall: Wallpaper = serde_json::from_value(w.clone())
        .unwrap_or_else(|_| Wallpaper { kind: wad_wallpaper::Kind::Gradient, value: wad_wallpaper::BLANK.into() });
    let paint = wall.paint();
    t.set_wallpaper(&paint);
    match &paint {
        Paint::Image(src) => images::wallpaper(src, images::later(&t, |t, tex| t.set_picture(Some(tex)))),
        Paint::Texture(id) => images::textured(&t, id, 480),
        Paint::Layers(_) => {}
    }
    let o = gtk::Overlay::new();
    o.set_child(Some(&t));
    let l = gtk::Label::new(Some(label));
    l.add_css_class("swatch-name");
    l.set_halign(gtk::Align::Fill);
    l.set_valign(gtk::Align::End);
    l.set_xalign(0.0);
    o.add_overlay(&l);
    b.set_child(Some(&o));
    b
}

impl Picker {
    fn pick(self: &Rc<Self>, w: Value) {
        self.value.replace(w.clone());
        self.render();
        (self.on)(w);
    }

    fn render(self: &Rc<Self>) {
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        let cur = self.value.borrow().clone();
        let mut n = 0;
        let mut place = |w: &gtk::Widget| {
            self.grid.attach(w, n % 3, n / 3, 1, 1);
            n += 1;
        };
        for (id, name) in wad_wallpaper::TEXTURES {
            let w = json!({ "type": "texture", "value": id });
            let b = swatch(&w, name, cur["type"] == "texture" && cur["value"] == *id);
            let me = self.clone();
            b.connect_clicked(move |_| me.pick(w.clone()));
            place(b.upcast_ref());
        }
    }
}
