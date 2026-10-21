//! Launching: each wadspace is installed as its document (wadd draws its
//! background and web app icons), its set built by its id first when it has
//! never been, then wadd starts them all together, the first on screen. What
//! was launched is kept for "Jump back in".

use std::collections::HashMap;

use serde_json::{Value, json};
use wad_core::layers as core;
use wad_proto::v1::ImageRow;
use wad_wadd_client::{Method, Wadd};

use crate::rt;
use crate::store::store;

/// What the launcher starts.
#[derive(Debug, Clone, PartialEq)]
pub enum RunItem {
    Wadspace(String),
    /// A wadspace run once with a background (`{type, value}`) and keys by
    /// slot of its own (WadBrowser's key groups stay its own).
    WadspaceRun {
        id: String,
        wallpaper: Value,
        by_slot: Value,
    },
    Terminal,
    /// One of the machine's own apps as it is (wadd's "browser").
    App(String),
    /// A saved WadBrowser setup (its document).
    Browser(Value),
    /// A WadBrowser set up for one run (`{name, mode, webapps, secrets}`).
    BrowserRun(Value),
    /// One of a wadspace's web apps, in a WadBrowser of its own on the
    /// machine, as that wadspace has it: its sign-ins, its WadBrowser mode.
    WadspaceWebApp {
        from: String,
        webapp: String,
    },
}

impl RunItem {
    /// The wadspace it starts, if it starts one.
    pub fn wadspace(&self) -> Option<&str> {
        match self {
            RunItem::Wadspace(id) | RunItem::WadspaceRun { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// A WadBrowser setup's sign-ins as wadd mounts them: each web app's, by slot.
fn browser_signins(secrets: &Value) -> Vec<Value> {
    secrets
        .as_object()
        .into_iter()
        .flatten()
        .map(|(slot, r)| {
            json!({ "id": r["id"], "app": slot.trim_start_matches("web:"), "web": true, "name": r["name"], "kind": r["kind"], "slot": slot })
        })
        .collect()
}

pub use wad_core::layers::{ImageState, not_ready};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A wadspace's image's state (wad-core's image_state, with wadd's image
/// rows).
pub fn image_state(key: &str, images: &HashMap<String, ImageRow>) -> ImageState {
    let rows = images
        .iter()
        .map(|(k, r)| {
            (k.clone(), core::LayerImage { id: r.id.clone(), from: r.from.clone(), sha: r.dockerfile_sha.clone() })
        })
        .collect();
    core::image_state(key, &rows)
}

/// The machine's time zone (what the wadspace's clock shows).
fn time_zone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| p.to_str().and_then(|p| p.split("zoneinfo/").nth(1)).map(String::from))
        .unwrap_or_else(|| "Etc/UTC".into())
}

/// A wallpaper as the file the image shows: a gradient (or a texture, in
/// the app's theme) drawn to a PNG, a picture as a JPEG.
pub fn wallpaper_upload(wall: &Value) -> Result<Value, String> {
    let kind = s(wall, "type");
    let value = s(wall, "value");
    if kind == "image" {
        let bytes = match value.strip_prefix("data:").and_then(|d| d.split_once(";base64,")) {
            Some((_, b64)) => {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).map_err(|e| e.to_string())?
            }
            None => return Err("That wallpaper's picture isn't here.".into()),
        };
        let img = image::load_from_memory(&bytes).map_err(|e| format!("The wallpaper's picture didn't read: {e}"))?;
        let img = if img.width() > 2560 {
            img.resize(2560, img.height() * 2560 / img.width(), image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut out = std::io::Cursor::new(Vec::new());
        img.to_rgb8()
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90))
            .map_err(|e| e.to_string())?;
        return Ok(
            json!({ "ext": "jpg", "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, out.into_inner()) }),
        );
    }
    let kind = if matches!(kind, "color" | "texture") { kind } else { "gradient" };
    let wall: wad_wallpaper::Wallpaper =
        serde_json::from_value(json!({ "type": kind, "value": value })).map_err(|e| e.to_string())?;
    let tone = match wad_gtk::theme::current() {
        wad_gtk::Theme::Dark => wad_wallpaper::Tone::Dark,
        wad_gtk::Theme::Light => wad_wallpaper::Tone::Light,
    };
    let png = wad_wallpaper::draw_png_toned(&wall, tone).ok_or("The wallpaper couldn't be drawn.")?;
    Ok(json!({ "ext": "png", "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png) }))
}

/// What wadd's batch launch takes for each item (the first on screen).
fn launch_items(items: &[RunItem], wadspaces: &[Value]) -> Result<Vec<Value>, String> {
    items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            Ok(match i {
                RunItem::Wadspace(id) | RunItem::App(id) => {
                    json!({ "kind": "workspace", "workspace": id, "background": n > 0 })
                }
                RunItem::WadspaceRun { id, wallpaper, by_slot } => {
                    json!({ "kind": "run", "from": id, "overrides": run_overrides(wadspaces, id, wallpaper, by_slot)?, "background": n > 0 })
                }
                RunItem::Terminal => json!({ "kind": "terminal", "background": n > 0 }),
                RunItem::Browser(b) => json!({
                    "kind": "browser",
                    "setup": { "name": b["name"], "mode": b["mode"], "webapps": b["webapps"], "signins": browser_signins(&b["secrets"]) },
                    "savedId": b["id"],
                    "background": n > 0,
                }),
                RunItem::BrowserRun(c) => json!({
                    "kind": "browser",
                    "setup": { "name": c["name"], "mode": c["mode"], "webapps": c["webapps"], "signins": browser_signins(&c["secrets"]) },
                    "background": n > 0,
                }),
                RunItem::WadspaceWebApp { from, webapp } => {
                    json!({ "kind": "browser", "setup": wadspace_webapp_setup(wadspaces, from, webapp)?, "background": n > 0 })
                }
            })
        })
        .collect()
}

/// A wadspace's web apps (its WadBrowser's extras).
pub fn wadspace_webapps(w: &Value) -> Vec<Value> {
    let (template, setup) = wad_core::v4::as_template_setup(w);
    if !core::has_app(&template, "wadbrowser") {
        return vec![];
    }
    setup["webapps"].as_array().cloned().unwrap_or_default()
}

/// A wadspace's sign-ins as wadd mounts them.
fn signins_of(w: &Value) -> Value {
    wad_core::v4::workspace_for(w, &json!({}))["signins"].clone()
}

/// A WadBrowser setup for one of a wadspace's web apps: that app alone, its
/// sign-ins as the wadspace has them, and the wadspace's WadBrowser mode.
fn wadspace_webapp_setup(wadspaces: &[Value], from: &str, webapp: &str) -> Result<Value, String> {
    let w = wadspaces.iter().find(|x| s(x, "id") == from).ok_or("That wadspace is gone.")?;
    let (_, setup) = wad_core::v4::as_template_setup(w);
    let wa = setup["webapps"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| s(x, "id") == webapp)
        .cloned()
        .ok_or("That web app isn't in the wadspace any more.")?;
    let signins: Vec<Value> = signins_of(w)
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["web"] == true && s(x, "app") == webapp)
        .cloned()
        .collect();
    let mode = if s(&setup, "browserMode") == "focus" { "focus" } else { "browser" };
    let name = format!("{} · {}", s(&w["customization"], "name"), s(&wa, "name"));
    Ok(json!({ "name": name, "mode": mode, "webapps": [wa], "signins": signins }))
}

/// A run's own background (drawn) and sign-ins: its keys by slot in place
/// of the wadspace's, its WadBrowser groups as they are.
fn run_overrides(wadspaces: &[Value], id: &str, wallpaper: &Value, by_slot: &Value) -> Result<Value, String> {
    let mut w = wadspaces.iter().find(|x| s(x, "id") == id).ok_or("That wadspace is gone.")?.clone();
    w["customization"]["secrets"]["bySlot"] = by_slot.clone();
    Ok(json!({ "wallpaper": wallpaper_upload(wallpaper)?, "signins": signins_of(&w) }))
}

/// A wadspace installed here as it is now (wadd draws its wallpaper, in the
/// machine's theme).
async fn install(w: &Wadd, ws: &Value, tz: &str) -> Result<(), String> {
    let id = s(ws, "id");
    if let Some(why) = not_ready(ws) {
        return Err(format!("{}: {why}", s(&ws["customization"], "name")));
    }
    let install = json!({ "wadspace": ws, "tz": tz });
    w.act(Method::PUT, &format!("/v1/wadspaces/{id}"), Some(&install)).await.map_err(|e| e.to_string())
}

/// A wadspace installed here as it is now, without starting it (a stream
/// starts it on its sidecar).
pub async fn install_here(id: String) -> Result<(), String> {
    let ws = store().read_now(|st| {
        let lib = st.library.as_ref().ok_or("Your library isn't here yet.")?;
        lib.wadspaces.iter().find(|x| s(x, "id") == id).cloned().ok_or("That wadspace is gone.")
    })?;
    let tz = time_zone();
    rt::run(async move { install(&Wadd::here(), &ws, &tz).await }).await
}

/// Builds a wadspace here without starting it: installed as it is now,
/// its set built unless that's up to date or already building (wadd
/// decides).
pub async fn build_here(id: String) -> Result<(), String> {
    let ws = store().read_now(|st| {
        let lib = st.library.as_ref().ok_or("Your library isn't here yet.")?;
        lib.wadspaces.iter().find(|x| s(x, "id") == id).cloned().ok_or("That wadspace is gone.")
    })?;
    let tz = time_zone();
    rt::run(async move {
        let w = Wadd::here();
        install(&w, &ws, &tz).await?;
        build_set(&w, &ws).await
    })
    .await
}

/// A wadspace's set built here (POST /v1/wadspaces/{id}/build:
/// nothing when it's built already or building).
async fn build_set(w: &Wadd, ws: &Value) -> Result<(), String> {
    w.act(Method::POST, &format!("/v1/wadspaces/{}/build", s(ws, "id")), None::<&Value>)
        .await
        .map_err(|e| e.to_string())
}

/// Launches `items` (the first on screen), as a focus session of
/// `focus_minutes` if given. The ids wadd started.
pub async fn launch(items: Vec<RunItem>, focus_minutes: Option<u32>) -> Result<Vec<String>, String> {
    let wadspaces =
        store().read_now(|s| s.library.as_ref().map(|l| l.wadspaces.clone())).ok_or("Your library isn't here yet.")?;
    let mut ids: Vec<String> = vec![];
    for id in items.iter().filter_map(RunItem::wadspace) {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    let tz = time_zone();
    let started = rt::run(async move {
        let w = Wadd::here();
        let images: HashMap<String, ImageRow> = w
            .get::<Vec<ImageRow>>("/v1/images")
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.layer.clone(), r))
            .collect();
        for id in &ids {
            let Some(ws) = wadspaces.iter().find(|x| s(x, "id") == id) else { continue };
            install(&w, ws, &tz).await?;
            // Its set on an older base still runs: an update, not a build.
            if image_state(&core::stack_image_key(ws), &images) == ImageState::Never {
                build_set(&w, ws).await?;
            }
        }
        // Drawn off GTK's thread: a run's own background.
        let body_items = launch_items(&items, &wadspaces)?;
        w.post::<Vec<String>>("/v1/launches/batch", &json!({ "items": body_items, "focusMinutes": focus_minutes }))
            .await
            .map_err(|e| e.to_string())
    })
    .await?;
    // Jump back in: what ran, kept here.
    let again: Vec<String> =
        started.iter().filter(|id| !id.starts_with("run-") && !id.starts_with("browser-run-")).cloned().collect();
    if !again.is_empty() {
        save_last(&json!({
            "kind": if focus_minutes.is_some() { "focus" } else { "wadspace" },
            "wadspaceIds": again,
            "machineId": "local",
            "minutes": focus_minutes,
            "at": now_iso(),
        }));
    }
    Ok(started)
}

fn last_file() -> std::path::PathBuf {
    wad_gtk::theme::file().with_file_name("last-session.json")
}

fn save_last(v: &Value) {
    let _ = std::fs::write(last_file(), v.to_string());
    store().update(|_| crate::store::Topic::LIBRARY);
}

/// What was last launched here (else the account's, from another machine).
pub fn last_session() -> Option<Value> {
    std::fs::read(last_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .or_else(|| store().read_now(|s| s.profile().and_then(|p| p.last_session.clone())))
}

pub fn now_iso() -> String {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    wad_firebase_time(secs)
}

/// RFC 3339 for Unix ms (what the web app keeps).
fn wad_firebase_time(ms: i64) -> String {
    wad_account::rfc3339(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gradient_wallpaper_is_a_png_and_a_picture_a_jpeg() {
        let g = wallpaper_upload(&json!({"type": "gradient", "value": wad_wallpaper::BLANK})).unwrap();
        assert_eq!(g["ext"], "png");
        let png = wad_wallpaper::png(&wad_wallpaper::layers("#c6ff1f"), 4, 4).unwrap();
        let data = format!(
            "data:image/png;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png)
        );
        let p = wallpaper_upload(&json!({"type": "image", "value": data})).unwrap();
        assert_eq!(p["ext"], "jpg");
        assert!(wallpaper_upload(&json!({"type": "image", "value": "/wallpapers/x.png"})).is_err());
        // A texture (a version 4 run's): drawn, in the app's theme.
        assert_eq!(wallpaper_upload(&json!({"type": "texture", "value": "stars"})).unwrap()["ext"], "png");
        assert!(wallpaper_upload(&json!({"type": "texture", "value": "nope"})).is_err());
    }

    #[test]
    fn a_wadspace_builds_first_only_when_its_set_was_never_made() {
        let row = |layer: &str, id: &str, from: &str| ImageRow {
            layer: layer.into(),
            image_ref: String::new(),
            id: id.into(),
            dockerfile_sha: String::new(),
            built_at: 0.0,
            size_mb: 0.0,
            from: from.into(),
            pkgs: vec![],
        };
        let mut images = HashMap::from([("base".to_string(), row("base", "b2", ""))]);
        assert_eq!(image_state("set:abc", &images), ImageState::Never);
        images.insert("set:abc".into(), row("set:abc", "s1", "b1"));
        assert_eq!(image_state("set:abc", &images), ImageState::Update);
        images.insert("set:abc".into(), row("set:abc", "s2", "b2"));
        assert_eq!(image_state("set:abc", &images), ImageState::Built);
        assert_eq!(not_ready(&json!({"customization": {"name": " "}})), Some("Its customization needs a name."));
        assert_eq!(not_ready(&json!({"customization": {"name": "W"}})), None);
    }
}
