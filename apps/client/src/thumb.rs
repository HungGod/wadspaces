//! A wadspace's picture on cards (the web app's components/Thumb.tsx): the
//! desktop its layers give, on its wallpaper, with its projects' folders.

use gtk::prelude::*;
use serde_json::{Value, json};
use wad_gtk::{DesktopIcon, DesktopThumb};
use wad_wallpaper::{Paint, Wallpaper};

use crate::images;
use crate::store::AppState;

/// Drawn as on a screen this many times smaller, so icons read at card size.
pub const ZOOM: f32 = 1.6;

/// The desktop's icons: wad-core's desktop_for for the stack (a version 4
/// wadspace resolves to its own).
pub fn desktop_items(st: &AppState, w: &Value) -> Vec<Value> {
    let Some(lib) = st.library.as_ref() else { return vec![] };
    let r = wad_core::layers::resolve_stack(&lib.lib(), w);
    if r.is_null() {
        return vec![];
    }
    let projects: Vec<Value> = w["customization"]["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| json!({ "id": p["id"], "name": p["as"] }))
        .collect();
    wad_core::layers::desktop_for(&r["template"], &r["setup"], &projects)
}

pub fn thumb(st: &AppState, w: &Value) -> DesktopThumb {
    let t = DesktopThumb::new();
    t.add_css_class("thumb");
    let (sw, sh) = st
        .machine
        .as_ref()
        .and_then(|m| m.screen)
        .map(|s| (s.width as f32, s.height as f32))
        .unwrap_or((1920.0, 1080.0));
    t.set_screen(sw, sh, ZOOM);
    let wall: Wallpaper = serde_json::from_value(w["customization"]["wallpaper"].clone())
        .unwrap_or_else(|_| Wallpaper { kind: wad_wallpaper::Kind::Gradient, value: wad_wallpaper::BLANK.into() });
    let paint = wall.paint();
    t.set_wallpaper(&paint);
    match &paint {
        Paint::Image(src) => images::wallpaper(src, images::later(&t, |t, tex| t.set_picture(Some(tex)))),
        Paint::Texture(id) => images::textured(&t, id, 960),
        Paint::Layers(_) => {}
    }
    let items = desktop_items(st, w);
    t.set_icons(
        items
            .iter()
            .map(|i| DesktopIcon { label: i["label"].as_str().unwrap_or_default().to_string(), texture: None })
            .collect(),
    );
    for (n, i) in items.iter().enumerate() {
        if let Some(src) = i["iconUrl"].as_str() {
            images::desktop_icon(src, images::later(&t, move |t, tex| t.set_icon_texture(n, tex)));
        }
    }
    t
}
