//! Launching (the web app's data/local/layers.ts launch and data/layers.ts):
//! each wadspace is installed as it is now (with its wallpaper drawn), its
//! layer built first when it has to be, then wadd starts them all
//! together, the first on screen. A version 4 wadspace is installed as its
//! document (wadd draws its background and web app icons) and its set
//! built by its id. What was launched is kept for "Jump back in".

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

/// A layer image's state against the library (wad-core's image_state, with
/// wadd's image rows).
pub fn image_state(key: &str, lib: &Value, images: &HashMap<String, ImageRow>) -> ImageState {
    let rows = images
        .iter()
        .map(|(k, r)| {
            (k.clone(), core::LayerImage { id: r.id.clone(), from: r.from.clone(), sha: r.dockerfile_sha.clone() })
        })
        .collect();
    core::image_state(key, lib, &rows)
}

fn layer_request(key: &str, lib: &Value) -> Result<Value, String> {
    core::layer_request(key, lib)
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
fn launch_items(items: &[RunItem], lib: &Value, wadspaces: &[Value]) -> Result<Vec<Value>, String> {
    items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            Ok(match i {
                RunItem::Wadspace(id) | RunItem::App(id) => {
                    json!({ "kind": "workspace", "workspace": id, "background": n > 0 })
                }
                RunItem::WadspaceRun { id, wallpaper, by_slot } => {
                    json!({ "kind": "run", "from": id, "overrides": run_overrides(lib, wadspaces, id, wallpaper, by_slot)?, "background": n > 0 })
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
                    json!({ "kind": "browser", "setup": wadspace_webapp_setup(lib, wadspaces, from, webapp)?, "background": n > 0 })
                }
            })
        })
        .collect()
}

/// A wadspace's web apps (its Extensions' WadBrowser ones), as it resolves.
pub fn wadspace_webapps(lib: &Value, w: &Value) -> Vec<Value> {
    let r = core::resolve_stack(lib, w);
    if r.is_null() || !core::has_app(&r["template"], "wadbrowser") {
        return vec![];
    }
    r["setup"]["webapps"].as_array().cloned().unwrap_or_default()
}

/// A WadBrowser setup for one of a wadspace's web apps: that app alone, its
/// sign-ins as the wadspace has them, and the wadspace's WadBrowser mode.
fn wadspace_webapp_setup(lib: &Value, wadspaces: &[Value], from: &str, webapp: &str) -> Result<Value, String> {
    let w = wadspaces.iter().find(|x| s(x, "id") == from).ok_or("That wadspace is gone.")?;
    let r = core::resolve_stack(lib, w);
    if r.is_null() {
        return Err("That wadspace's template is gone.".into());
    }
    let wa = r["setup"]["webapps"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| s(x, "id") == webapp)
        .cloned()
        .ok_or("That web app isn't in the wadspace any more.")?;
    let signins: Vec<Value> = core::workspace_for(w, &r["template"], &r["setup"], &json!({}))["signins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["web"] == true && s(x, "app") == webapp)
        .cloned()
        .collect();
    let mode = if s(&r["setup"], "browserMode") == "focus" { "focus" } else { "browser" };
    let name = format!("{} · {}", s(&w["customization"], "name"), s(&wa, "name"));
    Ok(json!({ "name": name, "mode": mode, "webapps": [wa], "signins": signins }))
}

/// A run's own background (drawn) and sign-ins: its keys by slot in place
/// of the wadspace's, its WadBrowser groups as they are.
fn run_overrides(
    lib: &Value,
    wadspaces: &[Value],
    id: &str,
    wallpaper: &Value,
    by_slot: &Value,
) -> Result<Value, String> {
    let w = wadspaces.iter().find(|x| s(x, "id") == id).ok_or("That wadspace is gone.")?;
    let r = core::resolve_stack(lib, w);
    let mut w = w.clone();
    w["customization"]["secrets"]["bySlot"] = by_slot.clone();
    let signins = if r.is_null() {
        json!([])
    } else {
        core::workspace_for(&w, &r["template"], &r["setup"], &json!({}))["signins"].clone()
    };
    Ok(json!({ "wallpaper": wallpaper_upload(wallpaper)?, "signins": signins }))
}

/// A wadspace installed here as it is now, with its wallpaper drawn (a
/// version 4 one's wadd draws, in the machine's theme).
async fn install(w: &Wadd, ws: &Value, lib: &Value, tz: &str) -> Result<(), String> {
    let id = s(ws, "id");
    if let Some(why) = not_ready(ws, lib) {
        return Err(format!("{}: {why}", s(&ws["customization"], "name")));
    }
    if wad_core::v4::is_v4(ws) {
        let install = json!({ "wadspace": ws, "tz": tz });
        return w.act(Method::PUT, &format!("/v1/wadspaces/{id}"), Some(&install)).await.map_err(|e| e.to_string());
    }
    let r = core::resolve_stack(lib, ws);
    let install = json!({ "wadspace": ws, "template": r["template"], "extensions": r.get("extensions").cloned().unwrap_or(Value::Null), "tz": tz });
    w.act(Method::PUT, &format!("/v1/wadspaces/{id}"), Some(&install)).await.map_err(|e| e.to_string())?;
    let wall = wallpaper_upload(&ws["customization"]["wallpaper"])?;
    w.act(Method::PUT, &format!("/v1/workspaces/{id}/wallpaper"), Some(&wall)).await.map_err(|e| e.to_string())
}

/// A wadspace installed here as it is now, without starting it (a stream
/// starts it on its sidecar).
pub async fn install_here(id: String) -> Result<(), String> {
    let (lib, ws) = store().read_now(|st| {
        let ws = st.library.as_ref().and_then(|l| l.all_wadspaces().find(|x| s(x, "id") == id).cloned());
        (st.library.as_ref().map(|l| l.lib()), ws)
    });
    let lib = lib.ok_or("Your library isn't here yet.")?;
    let ws = ws.ok_or("That wadspace is gone.")?;
    let tz = time_zone();
    rt::run(async move { install(&Wadd::here(), &ws, &lib, &tz).await }).await
}

/// Builds a wadspace here without starting it (the web app's buildStack):
/// installed as it is now, its layer built unless that's up to date or
/// already building (a version 4 one's set: wadd decides).
pub async fn build_here(id: String) -> Result<(), String> {
    let (lib, ws, building) = store().read_now(|st| {
        let ws = st.library.as_ref().and_then(|l| l.all_wadspaces().find(|x| s(x, "id") == id).cloned());
        let building: Vec<String> =
            st.builds.iter().filter(|(b, _)| !b.status.finished()).filter_map(|(b, _)| b.layer.clone()).collect();
        (st.library.as_ref().map(|l| l.lib()), ws, building)
    });
    let lib = lib.ok_or("Your library isn't here yet.")?;
    let ws = ws.ok_or("That wadspace is gone.")?;
    let tz = time_zone();
    rt::run(async move {
        let w = Wadd::here();
        install(&w, &ws, &lib, &tz).await?;
        if wad_core::v4::is_v4(&ws) {
            return build_set(&w, &ws).await;
        }
        let key = core::stack_image_key(&ws);
        if key.is_empty() || building.contains(&key) {
            return Ok(());
        }
        let images: HashMap<String, ImageRow> = w
            .get::<Vec<ImageRow>>("/v1/images")
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.layer.clone(), r))
            .collect();
        if image_state(&key, &lib, &images) != ImageState::Built {
            w.act(Method::POST, "/v1/images/build", Some(&layer_request(&key, &lib)?))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
}

/// A version 4 wadspace's set built here (POST /v1/wadspaces/{id}/build:
/// nothing when it's built already or building).
async fn build_set(w: &Wadd, ws: &Value) -> Result<(), String> {
    w.act(Method::POST, &format!("/v1/wadspaces/{}/build", s(ws, "id")), None::<&Value>)
        .await
        .map_err(|e| e.to_string())
}

/// Launches `items` (the first on screen), as a focus session of
/// `focus_minutes` if given. The ids wadd started.
pub async fn launch(items: Vec<RunItem>, focus_minutes: Option<u32>) -> Result<Vec<String>, String> {
    let (lib, wadspaces) = store().read_now(|s| {
        let all = s.library.as_ref().map(|l| l.all_wadspaces().cloned().collect::<Vec<_>>());
        (s.library.as_ref().map(|l| l.lib()), all.unwrap_or_default())
    });
    let lib = lib.ok_or("Your library isn't here yet.")?;
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
            install(&w, ws, &lib, &tz).await?;
            let key = core::stack_image_key(ws);
            if wad_core::v4::is_v4(ws) {
                // Its set on an older base still runs: an update, not a build.
                if image_state(&key, &lib, &images) == ImageState::Never {
                    build_set(&w, ws).await?;
                }
                continue;
            }
            if !key.is_empty() && matches!(image_state(&key, &lib, &images), ImageState::Never | ImageState::Changed) {
                w.act(Method::POST, "/v1/images/build", Some(&layer_request(&key, &lib)?))
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        // Drawn off GTK's thread: a run's own background.
        let body_items = launch_items(&items, &lib, &wadspaces)?;
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
    fn images_need_building_when_never_made_or_changed() {
        let t = json!({"id": "dev", "name": "Dev", "apps": ["wadbrowser"]});
        let lib = json!({"templates": [t], "extensions": []});
        let mut images = HashMap::new();
        assert_eq!(image_state("template:dev", &lib, &images), ImageState::Never);
        let sha = core::template_layer(&t, &Value::Null)["sha"].as_str().unwrap().to_string();
        let row = |sha: &str| ImageRow {
            layer: "template:dev".into(),
            image_ref: String::new(),
            id: "i1".into(),
            dockerfile_sha: sha.into(),
            built_at: 0.0,
            size_mb: 0.0,
            from: String::new(),
            pkgs: vec![],
        };
        images.insert("template:dev".into(), row("old"));
        assert_eq!(image_state("template:dev", &lib, &images), ImageState::Changed);
        images.insert("template:dev".into(), row(&sha));
        assert_eq!(image_state("template:dev", &lib, &images), ImageState::Built);
    }

    #[test]
    fn a_stack_without_a_customization_isnt_ready() {
        let lib = json!({"templates": [{"id": "dev", "apps": []}], "extensions": []});
        assert_eq!(not_ready(&json!({"template": ""}), &lib), Some("It needs a template with apps."));
        assert_eq!(not_ready(&json!({"template": "gone"}), &lib), Some("Its template is gone."));
        assert_eq!(not_ready(&json!({"template": "dev"}), &lib), Some("It needs a customization."));
        assert_eq!(
            not_ready(&json!({"template": "dev", "customization": {"name": " "}}), &lib),
            Some("Its customization needs a name.")
        );
        assert_eq!(not_ready(&json!({"template": "dev", "customization": {"name": "W"}}), &lib), None);
    }
}
