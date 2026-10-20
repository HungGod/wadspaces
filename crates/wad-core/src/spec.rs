//! Workspaces' common rules: the base image's features in install order,
//! ids, a spec's defaults (the presets from before the layers are specs),
//! and the base images.

use serde_json::{Value, json};

use crate::js;

/// Features in install order (images/base's feature scripts).
pub const FEATURES: &[&str] = &[
    "git",
    "cpp",
    "python",
    "nodejs",
    "firebase",
    "gcloud",
    "rust",
    "go",
    "electron-deps",
    "vscode",
    "claude-code",
    "chrome",
    "tiled",
    "android-studio",
    "hplip",
    "obsidian",
    "libresprite",
    "audacity",
    "krita",
    "openshot",
    "steam",
    "dolphin",
];

/// The features each feature's script needs first (its `need` lines): a
/// package carries only its own files, so these come along as packages.
pub const FEATURE_NEEDS: &[(&str, &[&str])] = &[
    ("chrome", &["electron-deps"]),
    ("claude-code", &["nodejs"]),
    ("firebase", &["nodejs"]),
    ("obsidian", &["electron-deps"]),
    ("rust", &["cpp"]),
    ("vscode", &["electron-deps"]),
];

pub const DEFAULT_BASE_IMAGE: &str = "localhost/wadspaces-base:trixie";
/// The streamed desktop's base, which specs from before the layers name.
pub const SELKIES_BASE_IMAGE: &str = "localhost/wadspaces-selkies:trixie";
pub const DEFAULT_IMAGE_PREFIX: &str = "localhost/wadspaces-";

/// The base image for a display kind (anything but "host" is streamed).
pub fn base_image_for(display: Option<&str>) -> &'static str {
    if display == Some("host") { DEFAULT_BASE_IMAGE } else { SELKIES_BASE_IMAGE }
}

/// `/^[a-z0-9][a-z0-9-]{0,62}$/`
pub fn is_id(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.len() <= 63
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `newSpec(partial)`: the defaults, then whatever `partial` has.
pub fn new_spec(partial: &Value) -> Value {
    let id = js::present(partial, "id").map(js::string).unwrap_or_default();
    let display = js::present(partial, "display").map(js::string).unwrap_or_else(|| "host".into());
    let image = if id.is_empty() { String::new() } else { format!("{DEFAULT_IMAGE_PREFIX}{id}:latest") };
    let mut out = match json!({
        "id": id,
        "name": "",
        "display": display,
        "baseImage": base_image_for(Some(&display)),
        "features": ["git", "vscode", "claude-code"],
        "webapps": [],
        "image": image,
        "port": 3160,
        "hotkey": null,
        "env": { "PUID": "1000", "PGID": "1000", "TZ": "Pacific/Fiji" },
        "secrets": ["github_token"],
        "persistConfig": true,
        "devices": ["/dev/dri"],
        "shmSize": "1g",
        "autostart": false,
    }) {
        Value::Object(o) => o,
        _ => unreachable!(),
    };
    for (k, v) in js::obj(partial) {
        out.insert(k, v);
    }
    Value::Object(out)
}

/// `slugify(s)`: lowercase letters, digits and dashes, 63 at most.
pub fn slugify(s: &str) -> String {
    let lower = s.to_lowercase();
    let slug = js::replace_runs(&lower, |c| !(c.is_ascii_lowercase() || c.is_ascii_digit()), "-");
    js::ascii_prefix(js::trim_dashes(&slug), 63).to_string()
}
