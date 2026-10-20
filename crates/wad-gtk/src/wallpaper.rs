//! A wallpaper filling its widget: gradients and colours drawn as they are,
//! a picture covering it (the Builder's module screens, swatches). Children
//! go on top of it in an Overlay.

use std::cell::RefCell;

use gtk::{gdk, glib, graphene, prelude::*, subclass::prelude::*};
use wad_wallpaper::{Layer, Paint};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Wallpaper {
        pub layers: RefCell<Vec<Layer>>,
        pub picture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Wallpaper {
        const NAME: &'static str = "WadWallpaper";
        type Type = super::Wallpaper;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("wad-wallpaper");
        }
    }

    impl ObjectImpl for Wallpaper {}

    impl WidgetImpl for Wallpaper {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let wid = self.obj();
            let (w, h) = (wid.width() as f32, wid.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            match self.picture.borrow().as_ref() {
                Some(t) => {
                    let (tw, th) = (t.width() as f32, t.height() as f32);
                    let c = (w / tw).max(h / th);
                    let (dw, dh) = (tw * c, th * c);
                    snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, w, h));
                    snapshot.append_texture(t, &graphene::Rect::new((w - dw) / 2.0, (h - dh) / 2.0, dw, dh));
                    snapshot.pop();
                }
                None => crate::desktop::paint_layers(snapshot, &self.layers.borrow(), w, h),
            }
        }
    }
}

glib::wrapper! {
    pub struct Wallpaper(ObjectSubclass<imp::Wallpaper>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Wallpaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Wallpaper {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Gradients and colours drawn here; a picture comes in with
    /// [`set_picture`](Self::set_picture).
    pub fn set_paint(&self, paint: &Paint) {
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
}
