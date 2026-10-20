//! A wadspace's background (the web app's WallpaperPicker): one of the
//! shaded gradients, or a picture from this machine (kept in the document,
//! so made small: at most 1280 wide and 400 KB). A version 4 wadspace's is
//! one of the textures (no pictures).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gio, glib, prelude::*};
use serde_json::{Value, json};
use wad_gtk::{DesktopThumb, Icon};
use wad_wallpaper::{Paint, Wallpaper};

use crate::{images, rt, ui};

const MAX_WIDTH: u32 = 1280;
const MAX_BYTES: usize = 400 * 1024;

/// The picker for `value`; `on` hears each pick (`{type, value}`).
pub fn picker(value: Value, on: impl Fn(Value) + 'static) -> gtk::Grid {
    make(value, false, on)
}

/// The textures (in the app's theme), for a version 4 wadspace.
pub fn texture_picker(value: Value, on: impl Fn(Value) + 'static) -> gtk::Grid {
    make(value, true, on)
}

fn make(value: Value, textures: bool, on: impl Fn(Value) + 'static) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.add_css_class("wallpapers");
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(8);
    grid.set_row_spacing(8);
    let me = Rc::new(Picker { grid: grid.clone(), value: RefCell::new(value), textures, on: Box::new(on) });
    me.render();
    grid
}

struct Picker {
    grid: gtk::Grid,
    value: RefCell<Value>,
    textures: bool,
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
        if self.textures {
            for (id, name) in wad_wallpaper::TEXTURES {
                let w = json!({ "type": "texture", "value": id });
                let b = swatch(&w, name, cur["type"] == "texture" && cur["value"] == *id);
                let me = self.clone();
                b.connect_clicked(move |_| me.pick(w.clone()));
                place(b.upcast_ref());
            }
            return;
        }
        for (name, value) in wad_wallpaper::PRESETS {
            let w = json!({ "type": "gradient", "value": value });
            let b = swatch(&w, name, cur["type"] == "gradient" && cur["value"] == *value);
            let me = self.clone();
            b.connect_clicked(move |_| me.pick(w.clone()));
            place(b.upcast_ref());
        }
        if cur["type"] == "image" {
            place(swatch(&cur, "Your picture", true).upcast_ref());
        }
        let choose = gtk::Button::new();
        choose.add_css_class("swatch-choose");
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_valign(gtk::Align::Center);
        col.set_halign(gtk::Align::Center);
        col.append(&Icon::new("image-plus", 16));
        col.append(&gtk::Label::new(Some("Choose a picture…")));
        choose.set_child(Some(&col));
        let me = self.clone();
        choose.connect_clicked(move |b| me.clone().choose(b));
        place(choose.upcast_ref());
    }

    fn choose(self: Rc<Self>, b: &gtk::Button) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Pictures"));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Choose a picture").modal(true).filters(&filters).build();
        let window = b.root().and_downcast::<gtk::Window>();
        let b = b.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.open_future(window.as_ref()).await else { return };
            let Some(path) = file.path() else { return };
            ui::set_text(&b, "Loading…");
            b.set_sensitive(false);
            match rt::run(async move { shrink(&path) }).await {
                Ok(url) => self.pick(json!({ "type": "image", "value": url })),
                Err(e) => crate::toast::error("Couldn't use that picture", &e),
            }
        });
    }
}

/// A picture as a small JPEG data: URL (the web app's imageDataUrl).
fn shrink(path: &std::path::Path) -> Result<String, String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let img = if img.width() > MAX_WIDTH {
        img.resize(MAX_WIDTH, img.height() * MAX_WIDTH / img.width(), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    for quality in [75u8, 60, 45, 30] {
        let mut out = std::io::Cursor::new(Vec::new());
        rgb.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality))
            .map_err(|e| e.to_string())?;
        let bytes = out.into_inner();
        if bytes.len() <= MAX_BYTES {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
            return Ok(format!("data:image/jpeg;base64,{b64}"));
        }
    }
    Err("It's too big, even made small.".into())
}
