//! Pictures the app shows: desktop icons (wadd draws them: GET
//! /v1/desktop-icon), wallpapers that are pictures, avatars. Fetched and
//! decoded off GTK's loop, once, and kept.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};

use crate::rt;

type Waiting = Vec<Box<dyn FnOnce(gdk::Texture)>>;
/// Thumbnails on a texture: each, its texture's id, how wide it's drawn.
type Textured = Vec<(glib::WeakRef<wad_gtk::DesktopThumb>, String, u32)>;

thread_local! {
    static READY: RefCell<HashMap<String, Option<gdk::Texture>>> = RefCell::default();
    static WAITING: RefCell<HashMap<String, Waiting>> = RefCell::default();
    /// Thumbnails showing a texture, and which: drawn again in the other
    /// tone when the theme changes (one listener for all of them).
    static TEXTURED: RefCell<Option<Textured>> = const { RefCell::new(None) };
}

/// Straight RGBA as a texture.
fn texture(img: image::RgbaImage) -> gdk::Texture {
    let (w, h) = img.dimensions();
    gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(img.into_raw()),
        w as usize * 4,
    )
    .upcast()
}

/// Calls `on` with the picture `key` names once it's there (at once if it
/// is); `fetch` makes it, on tokio, the first time it's asked for.
fn get<F>(key: String, on: impl FnOnce(gdk::Texture) + 'static, fetch: F)
where
    F: std::future::Future<Output = Option<image::RgbaImage>> + Send + 'static,
{
    match READY.with(|r| r.borrow().get(&key).cloned()) {
        Some(Some(t)) => return on(t),
        // Couldn't be had: not asked again this run.
        Some(None) => return,
        None => {}
    }
    let first = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let list = w.entry(key.clone()).or_default();
        list.push(Box::new(on));
        list.len() == 1
    });
    if !first {
        return;
    }
    glib::spawn_future_local(async move {
        let got = rt::run(fetch).await.map(texture);
        READY.with(|r| r.borrow_mut().insert(key.clone(), got.clone()));
        let waiting = WAITING.with(|w| w.borrow_mut().remove(&key)).unwrap_or_default();
        if let Some(t) = got {
            for f in waiting {
                f(t.clone());
            }
        }
    });
}

/// Where WadSpaces' own pictures are: the image's, else (a laptop's build)
/// the repo's assets/public.
fn asset(rel: &str) -> Option<Vec<u8>> {
    let rel = rel.strip_prefix('/')?;
    if rel.split('/').any(|c| c == ".." || c.is_empty()) {
        return None;
    }
    let dirs = [
        std::path::PathBuf::from("/usr/share/wadspaces/assets"),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/public"),
    ];
    dirs.iter().find_map(|d| std::fs::read(d.join(rel)).ok())
}

fn query_value(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A desktop icon's picture (wad-core's desktop_for `iconUrl`).
pub fn desktop_icon(src: &str, on: impl FnOnce(gdk::Texture) + 'static) {
    let src = src.to_string();
    get(format!("desk {src}"), on, async move {
        let w = wad_wadd_client::Wadd::here();
        let bytes = match w.bytes(&format!("/v1/desktop-icon?size=96&src={}", query_value(&src))).await {
            Ok(b) => b,
            // No wadd (a laptop without one): the picture itself.
            Err(_) => asset(&src)?,
        };
        wad_icons::decode::decode(&bytes)
            .map(|img| if img.width() > 96 { image::imageops::thumbnail(&img, 96, 96) } else { img })
    });
}

/// A wallpaper that's a picture, for a thumbnail (cards, the board, the
/// pickers): at most 960 wide (a thumbnail's never more than about half a
/// screen, on a screen twice as dense).
pub fn wallpaper(value: &str, on: impl FnOnce(gdk::Texture) + 'static) {
    wallpaper_at(value, 960, on);
}

/// A wallpaper that's a picture: a data: URL (the Builder keeps uploads so)
/// or one of WadSpaces' own (/wallpapers/...), kept at most `max` wide.
fn wallpaper_at(value: &str, max: u32, on: impl FnOnce(gdk::Texture) + 'static) {
    let v = value.to_string();
    let key = format!("wall{max} {}", if v.len() > 96 { format!("{}…{}", &v[..48], v.len()) } else { v.clone() });
    get(key, on, async move {
        let bytes = if let Some(data) = v.strip_prefix("data:") {
            let b64 = data.split_once(";base64,")?.1;
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?
        } else {
            asset(&v)?
        };
        let img = wad_icons::decode::decode(&bytes)?;
        Some(if img.width() > max {
            // (thumbnail: integer area sampling, nothing allocated but the result)
            image::imageops::thumbnail(&img, max, img.height() * max / img.width())
        } else {
            img
        })
    });
}

/// The tone textures are drawn in: the app's theme (black for dark, white
/// for light), as wadd draws them in the wadspaces.
pub fn tone() -> wad_wallpaper::Tone {
    match wad_gtk::theme::shown() {
        wad_gtk::Theme::Dark => wad_wallpaper::Tone::Dark,
        wad_gtk::Theme::Light => wad_wallpaper::Tone::Light,
    }
}

/// A texture background (a wadspace's), drawn `wide` pixels wide (16:9) in
/// a theme's tone.
fn texture_wallpaper(id: &str, wide: u32, tone: wad_wallpaper::Tone, on: impl FnOnce(gdk::Texture) + 'static) {
    let id = id.to_string();
    get(format!("texture{wide} {id} {tone:?}"), on, async move {
        // Opaque: its premultiplied pixels are straight ones.
        let h = wide * 9 / 16;
        image::RgbaImage::from_raw(wide, h, wad_wallpaper::texture::pixmap(&id, tone, wide, h)?.take())
    });
}

/// A thumbnail on texture `id`, drawn `wide` pixels wide (960 for a card or
/// the details' header, half that for a picker's swatch), in the theme's
/// tone now and whenever it changes.
pub fn textured(t: &wad_gtk::DesktopThumb, id: &str, wide: u32) {
    textured_now(t, id, wide);
    TEXTURED.with(|l| {
        let mut l = l.borrow_mut();
        let list = l.get_or_insert_with(|| {
            wad_gtk::theme::on_change(|_| {
                let live: Vec<_> = TEXTURED.with(|l| {
                    let mut l = l.borrow_mut();
                    let list = l.as_mut().expect("listed");
                    list.retain(|(w, ..)| w.upgrade().is_some());
                    list.iter().filter_map(|(w, id, wide)| Some((w.upgrade()?, id.clone(), *wide))).collect()
                });
                for (t, id, wide) in live {
                    textured_now(&t, &id, wide);
                }
            });
            vec![]
        });
        list.retain(|(w, ..)| w.upgrade().is_some());
        list.push((t.downgrade(), id.to_string(), wide));
    });
}

/// Its texture in the theme's tone now (and its icons' names in the ink
/// that reads on it). One drawn for a theme since left comes too late:
/// the next one's on its way.
fn textured_now(t: &wad_gtk::DesktopThumb, id: &str, wide: u32) {
    let asked = tone();
    texture_wallpaper(
        id,
        wide,
        asked,
        later(t, move |t, tex| {
            if tone() == asked {
                t.set_picture(Some(tex));
                t.set_dark_labels(asked == wad_wallpaper::Tone::Light);
            }
        }),
    );
}

/// A widget that fills in when its picture arrives.
pub fn later<W: IsA<gtk::Widget>>(
    w: &W,
    set: impl Fn(&W, gdk::Texture) + 'static,
) -> impl FnOnce(gdk::Texture) + 'static {
    let weak = w.downgrade();
    let set = Rc::new(set);
    move |t| {
        if let Some(w) = weak.upgrade() {
            set(&w, t);
        }
    }
}
