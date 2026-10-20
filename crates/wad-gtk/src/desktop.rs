//! A wadspace's desktop, small (the web app's components/desktop/
//! DesktopThumb.tsx): its wallpaper, its icons in selkies-desktop's
//! columns with their names, and the panel along the bottom, laid out as on
//! the machine's screen (drawn as on one `zoom` times smaller, so the icons
//! read at card size) and scaled to cover the widget.

use std::cell::{Cell, RefCell};

use gtk::{gdk, glib, graphene, gsk, pango, prelude::*, subclass::prelude::*};
use wad_wallpaper::{Layer, Paint, Rgba};

/// selkies-desktop's sizes (apps/selkies-desktop, desktop.h), in its pixels.
pub const PANEL_H: f32 = 56.0;
pub const START_W: f32 = 60.0;
pub const CELL_W: f32 = 96.0;
pub const CELL_H: f32 = 104.0;
pub const GRID_PAD: f32 = 10.0;
pub const ICON: f32 = 48.0;

/// How many icons fit in a column above the panel.
pub fn rows_for(screen_h: f32) -> usize {
    (((screen_h - PANEL_H) / CELL_H).floor() as usize).max(1)
}

/// Where the n-th icon goes: down each column, then the next.
pub fn cell_at(n: usize, screen_h: f32) -> (f32, f32) {
    let rows = rows_for(screen_h);
    ((n / rows) as f32 * CELL_W + GRID_PAD, (n % rows) as f32 * CELL_H + GRID_PAD)
}

#[derive(Clone, Default)]
pub struct DesktopIcon {
    pub label: String,
    pub texture: Option<gdk::Texture>,
}

fn rgba(c: Rgba) -> gdk::RGBA {
    gdk::RGBA::new(c.r, c.g, c.b, c.a)
}

fn stops(s: &[wad_wallpaper::Stop]) -> Vec<gsk::ColorStop> {
    // GSK wants them in order, within 0..1 (CSS allows past the ends).
    let mut last = 0.0f32;
    s.iter()
        .map(|st| {
            last = st.at.clamp(last, 1.0);
            gsk::ColorStop::new(last, rgba(st.color))
        })
        .collect()
}

/// A wallpaper's layers drawn over `w`×`h` (from 0,0).
pub fn paint_layers(snapshot: &gtk::Snapshot, layers: &[Layer], w: f32, h: f32) {
    let all = graphene::Rect::new(0.0, 0.0, w, h);
    for l in layers {
        match l {
            Layer::Color(c) => snapshot.append_color(&rgba(*c), &all),
            Layer::Linear { deg, stops: s } => {
                let ((x0, y0), (x1, y1)) = Layer::line(*deg, w, h);
                snapshot.append_linear_gradient(
                    &all,
                    &graphene::Point::new(x0, y0),
                    &graphene::Point::new(x1, y1),
                    &stops(s),
                );
            }
            Layer::Radial { rx, ry, cx, cy, stops: s } => {
                let (hr, vr) = ((rx * w).max(0.01), (ry * h).max(0.01));
                snapshot.append_radial_gradient(
                    &all,
                    &graphene::Point::new(cx * w, cy * h),
                    hr,
                    vr,
                    0.0,
                    1.0,
                    &stops(s),
                );
            }
        }
    }
}

mod imp {
    use super::*;

    pub struct DesktopThumb {
        pub layers: RefCell<Vec<Layer>>,
        pub picture: RefCell<Option<gdk::Texture>>,
        pub icons: RefCell<Vec<DesktopIcon>>,
        /// The machine's screen, in logical pixels.
        pub screen: Cell<(f32, f32)>,
        pub zoom: Cell<f32>,
        /// Rounded corners (a card's).
        pub radius: Cell<f32>,
        /// Exactly this wide (16:9 tall), else as wide as it's given.
        pub width: Cell<Option<i32>>,
        /// Icons' names in black (on a light texture), else white.
        pub dark_labels: Cell<bool>,
    }

    impl Default for DesktopThumb {
        fn default() -> Self {
            Self {
                layers: RefCell::default(),
                picture: RefCell::default(),
                icons: RefCell::default(),
                screen: Cell::new((1920.0, 1080.0)),
                zoom: Cell::new(1.6),
                radius: Cell::new(0.0),
                width: Cell::new(None),
                dark_labels: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DesktopThumb {
        const NAME: &'static str = "WadDesktopThumb";
        type Type = super::DesktopThumb;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("wad-desktop");
        }
    }

    impl ObjectImpl for DesktopThumb {}

    impl WidgetImpl for DesktopThumb {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        /// 16:9, as the web app's `aspect-video`.
        fn measure(&self, o: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(w) = self.width.get() {
                let s = if o == gtk::Orientation::Horizontal { w } else { (w as f32 * 9.0 / 16.0).round() as i32 };
                return (s, s, -1, -1);
            }
            match o {
                gtk::Orientation::Horizontal => (0, 160, -1, -1),
                _ => {
                    let h = if for_size > 0 { (for_size as f32 * 9.0 / 16.0).round() as i32 } else { 90 };
                    (h, h, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let wid = self.obj();
            let (w, h) = (wid.width() as f32, wid.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let (sw, sh) = self.screen.get();
            let z = self.zoom.get().max(0.1);
            let (vw, vh) = (sw / z, sh / z);
            let k = (w / vw).max(h / vh);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(
                graphene::Rect::new(0.0, 0.0, w, h),
                self.radius.get(),
            ));
            snapshot.append_color(
                &gdk::RGBA::new(11.0 / 255.0, 11.0 / 255.0, 20.0 / 255.0, 1.0),
                &graphene::Rect::new(0.0, 0.0, w, h),
            );
            snapshot.save();
            snapshot.scale(k, k);
            // The wallpaper fills the screen.
            match self.picture.borrow().as_ref() {
                Some(t) => {
                    let (tw, th) = (t.width() as f32, t.height() as f32);
                    let c = (vw / tw).max(vh / th);
                    let (dw, dh) = (tw * c, th * c);
                    snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, vw, vh));
                    snapshot.append_texture(t, &graphene::Rect::new((vw - dw) / 2.0, (vh - dh) / 2.0, dw, dh));
                    snapshot.pop();
                }
                None => paint_layers(snapshot, &self.layers.borrow(), vw, vh),
            }
            // The icons: a 48 px picture 5 px into its 96×104 cell, its name
            // under it in white 12 px (black on a light texture), on up to
            // two lines.
            let ctx = wid.pango_context();
            let mut font = pango::FontDescription::from_string("Sans 12px");
            font.set_absolute_size(12.0 * pango::SCALE as f64);
            let ink = if self.dark_labels.get() {
                gdk::RGBA::new(22.0 / 255.0, 22.0 / 255.0, 22.0 / 255.0, 1.0)
            } else {
                gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)
            };
            for (n, icon) in self.icons.borrow().iter().enumerate() {
                let (x, y) = cell_at(n, vh);
                if x > vw {
                    break;
                }
                if let Some(t) = &icon.texture {
                    snapshot.append_texture(t, &graphene::Rect::new(x + (CELL_W - ICON) / 2.0, y + 5.0, ICON, ICON));
                }
                let layout = pango::Layout::new(&ctx);
                layout.set_font_description(Some(&font));
                layout.set_text(&icon.label);
                layout.set_width(92 * pango::SCALE);
                layout.set_alignment(pango::Alignment::Center);
                layout.set_wrap(pango::WrapMode::WordChar);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                layout.set_height(-2);
                snapshot.save();
                snapshot.translate(&graphene::Point::new(x + 2.0, y + ICON + 11.0));
                snapshot.append_layout(&layout, &ink);
                snapshot.restore();
            }
            // The panel, and its Start button.
            snapshot.append_color(
                &gdk::RGBA::new(26.0 / 255.0, 26.0 / 255.0, 26.0 / 255.0, 1.0),
                &graphene::Rect::new(0.0, vh - PANEL_H, vw, PANEL_H),
            );
            let start = gsk::RoundedRect::from_rect(
                graphene::Rect::new(4.0, vh - PANEL_H + 4.0, START_W - 8.0, PANEL_H - 8.0),
                4.0,
            );
            snapshot.push_rounded_clip(&start);
            snapshot.append_color(&gdk::RGBA::new(64.0 / 255.0, 64.0 / 255.0, 64.0 / 255.0, 1.0), start.bounds());
            snapshot.pop();
            snapshot.restore();
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct DesktopThumb(ObjectSubclass<imp::DesktopThumb>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for DesktopThumb {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopThumb {
    pub fn new() -> Self {
        let me: Self = glib::Object::new();
        me.set_overflow(gtk::Overflow::Hidden);
        me
    }

    /// The wallpaper: gradients and colours drawn here; a picture comes in
    /// with [`set_picture`](Self::set_picture).
    pub fn set_wallpaper(&self, paint: &Paint) {
        match paint {
            Paint::Layers(l) => {
                *self.imp().layers.borrow_mut() = l.clone();
                self.imp().picture.borrow_mut().take();
            }
            // A picture, or a texture drawn as one: set_picture brings it.
            Paint::Image(_) | Paint::Texture(_) => self.imp().layers.borrow_mut().clear(),
        }
        self.queue_draw();
    }

    pub fn set_picture(&self, t: Option<gdk::Texture>) {
        *self.imp().picture.borrow_mut() = t;
        self.queue_draw();
    }

    /// Icons' names in black, for a light texture (the wadspace's desktop
    /// draws them so in the light theme).
    pub fn set_dark_labels(&self, dark: bool) {
        self.imp().dark_labels.set(dark);
        self.queue_draw();
    }

    pub fn set_icons(&self, icons: Vec<DesktopIcon>) {
        *self.imp().icons.borrow_mut() = icons;
        self.queue_draw();
    }

    /// One icon's picture arrived.
    pub fn set_icon_texture(&self, n: usize, t: gdk::Texture) {
        if let Some(i) = self.imp().icons.borrow_mut().get_mut(n) {
            i.texture = Some(t);
        }
        self.queue_draw();
    }

    /// Exactly `w` wide (and 16:9 tall).
    pub fn set_fixed_width(&self, w: i32) {
        self.imp().width.set(Some(w));
        self.queue_resize();
    }

    pub fn set_radius(&self, r: f32) {
        self.imp().radius.set(r);
        self.queue_draw();
    }

    /// The machine's screen (logical pixels) and how much smaller to draw it.
    pub fn set_screen(&self, w: f32, h: f32, zoom: f32) {
        self.imp().screen.set((w.max(320.0), h.max(240.0)));
        self.imp().zoom.set(zoom);
        self.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_fill_columns_down_then_across() {
        // 1080 tall: (1080 - 56) / 104 = 9 rows.
        assert_eq!(rows_for(1080.0), 9);
        assert_eq!(cell_at(0, 1080.0), (10.0, 10.0));
        assert_eq!(cell_at(8, 1080.0), (10.0, 842.0));
        assert_eq!(cell_at(9, 1080.0), (106.0, 10.0));
        assert_eq!(rows_for(100.0), 1);
    }
}
