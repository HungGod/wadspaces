//! The Builder's catalog (data/catalog.json): the apps a template is made of,
//! the VS Code extensions and command-line tools an Extensions layer adds, and
//! for each app what the Builder shows (summary, screenshots, its Start menu
//! category) and how it gets into an image. The starter templates every
//! library begins with are in data/starters.json.

use std::sync::OnceLock;

use serde_json::{Value, json};

/// How an app gets into a template's image.
pub enum Recipe {
    /// A `wadspaces-feature` script; `desktop`: the launchers it writes.
    Feature { feature: &'static str, desktop: &'static [&'static str] },
    /// Debian packages (`wadspaces-apt`); `desktop`: the launcher to use.
    Apt { packages: &'static [&'static str], desktop: Option<&'static str> },
    /// In the base image already.
    Builtin { desktop: &'static str },
}

pub const CATALOG_JSON: &str = include_str!("../data/catalog.json");
pub const STARTERS_JSON: &str = include_str!("../data/starters.json");

/// The whole catalog, as the Builder shows it: `{apps, extensions, tools}`.
pub fn catalog() -> &'static Value {
    static C: OnceLock<Value> = OnceLock::new();
    C.get_or_init(|| serde_json::from_str(CATALOG_JSON).expect("data/catalog.json is valid"))
}

/// The starter templates (Writing, Dev): `[{id, name, description, apps}]`.
pub fn starters() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(|| serde_json::from_str(STARTERS_JSON).expect("data/starters.json is valid"))
}

/// How a catalog app is installed, read once from the catalog (which lives
/// for the whole run), so recipes can hand out `&'static` slices.
struct Install {
    id: &'static str,
    kind: &'static str,
    feature: Option<&'static str>,
    desktop: Vec<&'static str>,
    packages: Vec<&'static str>,
}

fn installs() -> &'static [Install] {
    static I: OnceLock<Vec<Install>> = OnceLock::new();
    I.get_or_init(|| {
        let strs = |v: &'static Value, k: &str| -> Vec<&'static str> {
            v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
        };
        catalog()["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| {
                let i = &a["install"];
                Install {
                    id: a["id"].as_str().unwrap_or(""),
                    kind: i["kind"].as_str().unwrap_or(""),
                    feature: i["feature"].as_str(),
                    desktop: strs(i, "desktop"),
                    packages: strs(i, "packages"),
                }
            })
            .collect()
    })
}

/// The recipe catalog.json gives an app, if it lists it.
pub fn catalog_recipe(id: &str) -> Option<Recipe> {
    let i = installs().iter().find(|e| e.id == id)?;
    Some(match i.kind {
        "feature" => Recipe::Feature { feature: i.feature?, desktop: &i.desktop },
        "apt" => Recipe::Apt { packages: &i.packages, desktop: i.desktop.first().copied() },
        "builtin" => Recipe::Builtin { desktop: i.desktop.first()? },
        _ => return None,
    })
}

/// An app in catalog.json.
pub fn catalog_app(id: &str) -> Option<&'static Value> {
    catalog()["apps"].as_array()?.iter().find(|a| a["id"] == id)
}

/// An extension: the catalog's, else one the user added by its id.
pub fn extension(id: &str) -> Value {
    catalog()["extensions"]
        .as_array()
        .and_then(|l| l.iter().find(|e| e["id"] == id))
        .cloned()
        .unwrap_or_else(|| json!({ "id": id, "name": id, "description": "From the marketplace", "secret": null }))
}

/// A command-line tool's catalog entry.
pub fn tool(id: &str) -> Option<&'static Value> {
    catalog()["tools"].as_array()?.iter().find(|t| t["id"] == id)
}

/// What the layers need to know about an app in a template.
pub struct App {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub category: String,
    pub opens_folders: bool,
    /// Its sign-in: (kind, id that names a new secret).
    pub secret: Option<(String, String)>,
    /// How it's installed.
    pub recipe: Recipe,
}

fn str_at<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// An app of a template (catalog.json).
pub fn app(id: &str) -> Option<App> {
    let a = catalog_app(id)?;
    let secret = a.get("secret").filter(|s| s.is_object()).map(|s| (str_at(s, "kind").into(), str_at(s, "id").into()));
    Some(App {
        id: id.into(),
        name: str_at(a, "name").into(),
        icon: str_at(a, "icon").into(),
        category: str_at(a, "category").into(),
        opens_folders: a["opensFolders"] == true,
        secret,
        recipe: catalog_recipe(id)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_app_has_a_recipe_and_its_pictures() {
        for a in catalog()["apps"].as_array().unwrap() {
            let id = a["id"].as_str().unwrap();
            assert!(catalog_recipe(id).is_some(), "no recipe for {id}");
            assert!(app(id).is_some(), "{id}");
            assert!(a["description"].as_str().unwrap().len() > 40, "{id}");
            assert!(!a["icon"].as_str().unwrap().is_empty(), "{id}");
            // The command line is the base's own terminal: no pictures of it yet.
            if id != "terminal" {
                assert!(!a["screenshots"].as_array().unwrap().is_empty(), "{id}");
            }
            assert!(matches!(a["category"].as_str(), Some(c) if !c.is_empty()), "{id}");
        }
    }

    #[test]
    fn the_requested_apps_with_wadbrowser_once() {
        let names: Vec<&str> =
            catalog()["apps"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "WadBrowser",
                "VS Code",
                "Obsidian",
                "Tiled",
                "LibreSprite",
                "Audacity",
                "LMMS",
                "Krita",
                "OpenShot",
                "Steam",
                "Dolphin",
                "OBS Studio",
                "Command line"
            ]
        );
        assert_eq!(catalog_app("wadbrowser").unwrap()["icon"], "/brand/wadbrowser-icon-dark.svg");
    }

    #[test]
    fn features_and_tools_are_known_features() {
        for a in catalog()["apps"].as_array().unwrap() {
            if let Some(Recipe::Feature { feature, .. }) = catalog_recipe(a["id"].as_str().unwrap()) {
                assert!(crate::spec::FEATURES.contains(&feature), "{feature}");
            }
        }
        for t in catalog()["tools"].as_array().unwrap() {
            assert!(crate::spec::FEATURES.contains(&t["id"].as_str().unwrap()), "{}", t["id"]);
        }
    }

    #[test]
    fn secret_kinds_have_sign_in_paths() {
        let kinds = catalog()["apps"]
            .as_array()
            .unwrap()
            .iter()
            .chain(catalog()["extensions"].as_array().unwrap())
            .filter_map(|x| x["secret"]["kind"].as_str());
        for k in kinds {
            assert!(crate::signins::paths_for(k).is_some(), "{k}");
        }
    }

    #[test]
    fn only_catalog_apps_are_in_templates() {
        assert!(app("chrome").is_none() && app("github").is_none());
    }
}
