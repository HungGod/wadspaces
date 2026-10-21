use serde_json::{Value, json};

use super::*;

/// IntelligenceQuest on its way to Rust: a Dev template, a C++/Rust one (an
/// app in two templates), and game art.
fn iq() -> Value {
    json!({
        "version": 4, "id": "iq", "owner": "u1", "machine": "surface", "order": 1,
        "templates": [
            {"id": "dev", "name": "Dev", "apps": [
                {"app": "wadbrowser", "mode": "focus", "extras": [{"id": "github", "name": "GitHub", "url": "https://github.com"}]},
                {"app": "vscode", "extras": [{"id": "anthropic.claude-code"}]}]},
            {"id": "lang", "name": "C++ and Rust", "apps": [
                {"app": "vscode", "extras": [{"id": "ms-vscode.cpptools"}, {"id": "rust-lang.rust-analyzer"}, {"id": "anthropic.claude-code"}]},
                {"app": "terminal", "extras": [{"id": "cpp"}, {"id": "rust"}]},
                {"app": "wadbrowser", "extras": [{"id": "learnopengl", "name": "LearnOpenGL", "url": "https://learnopengl.com", "iconUrl": "/i.png"}]}]},
            {"id": "art", "name": "Game art", "apps": [{"app": "libresprite"}, {"app": "tiled"}]}
        ],
        "customization": {
            "name": "IntelligenceQuest", "description": "A game.", "wallpaper": {"type": "texture", "value": "stars"},
            "autostart": ["app:vscode"],
            "projects": [{"id": "project-iq", "kind": "github", "repo": "HungGod/IntelligenceQuest", "as": "IntelligenceQuest"}],
            "secrets": {"bySlot": {"ext:anthropic.claude-code": {"id": "iq-claude", "name": "IQ Claude", "kind": "vscode-claude-code"}},
                        "groups": [{"id": "g1", "members": ["browser", "web:github"], "key": {"id": "hunggod", "name": "hunggod", "kind": "web"}}]}
        }
    })
}

fn with(mut w: Value, f: impl FnOnce(&mut Value)) -> Value {
    f(&mut w);
    w
}

#[test]
fn a_good_wadspace_checks() {
    assert_eq!(check(&iq()), Ok(()));
    // No templates and no background: an empty wadspace is one.
    let empty = json!({"version": 4, "id": "e", "machine": "m", "templates": [], "customization": {"name": "Empty"}});
    assert_eq!(check(&empty), Ok(()));
}

#[test]
fn what_check_refuses() {
    let bad = |f: fn(&mut Value)| check(&with(iq(), f)).unwrap_err();
    assert_eq!(bad(|w| w["version"] = json!(3)), "it isn't a version 4 wadspace");
    assert_eq!(bad(|w| w["machine"] = json!("")), "it isn't on a machine");
    assert_eq!(bad(|w| w["templates"][1]["id"] = json!("dev")), "a template's id \"dev\" is missing or used twice");
    assert_eq!(bad(|w| w["templates"][2]["apps"][0]["app"] = json!("photoshop")), "there's no app \"photoshop\"");
    assert_eq!(bad(|w| w["templates"][2]["apps"][1]["app"] = json!("libresprite")), "libresprite is in Game art twice");
    assert_eq!(
        bad(|w| w["templates"][2]["apps"][0]["extras"] = json!([{"id": "x"}])),
        "nothing is added to libresprite"
    );
    assert_eq!(
        bad(|w| w["templates"][1]["apps"][1]["extras"][0]["id"] = json!("java")),
        "there's no command-line tool \"java\""
    );
    assert_eq!(
        bad(|w| w["templates"][0]["apps"][1]["extras"][0]["id"] = json!("no dot")),
        "\"no dot\" isn't a VS Code extension's id"
    );
    assert_eq!(
        bad(|w| w["templates"][0]["apps"][0]["extras"][0]["url"] = json!("github.com")),
        "the web app github needs an http(s) address"
    );
    assert_eq!(bad(|w| w["templates"][0]["apps"][1]["mode"] = json!("focus")), "vscode has no mode \"focus\"");
    assert!(bad(|w| w["customization"]["wallpaper"] = json!({"type": "image", "value": "x.jpg"})).contains("textures"));
    assert_eq!(bad(|w| w["customization"]["name"] = json!(" ")), "a wadspace needs a name of 1 to 80 characters");
}

#[test]
fn an_app_in_two_templates_is_one_with_both_templates_extras() {
    let apps = apps_of(&iq());
    let ids: Vec<&str> = apps.iter().map(|a| a["app"].as_str().unwrap()).collect();
    assert_eq!(ids, ["wadbrowser", "vscode", "terminal", "libresprite", "tiled"]);
    assert_eq!(apps[0]["mode"], "focus");
    assert_eq!(apps[0]["extras"].as_array().unwrap().len(), 2, "GitHub and LearnOpenGL");
    let code: Vec<&str> = apps[1]["extras"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(code, ["anthropic.claude-code", "ms-vscode.cpptools", "rust-lang.rust-analyzer"], "Claude once");
}

#[test]
fn its_packages_are_its_apps_tools_and_extensions_with_what_they_need() {
    let p = packages_of(&iq());
    assert_eq!(
        p,
        [
            "cpp",
            "rust",
            "electron-deps",
            "vscode",
            "tiled",
            "libresprite",
            "ext-anthropic.claude-code",
            "ext-ms-vscode.cpptools",
            "ext-rust-lang.rust-analyzer"
        ]
    );
    // WadBrowser and the command line are in the base: no packages.
    let browser = json!({"version": 4, "templates": [{"apps": [{"app": "wadbrowser"}, {"app": "terminal"}]}]});
    assert!(packages_of(&browser).is_empty());
    assert_eq!(image_key(&browser), "base");
}

#[test]
fn a_set_is_named_by_its_packages_alone() {
    let key = image_key(&iq());
    assert!(key.starts_with("set:") && key.len() == 16, "{key}");
    // The same packages from other templates: the same set.
    let same = with(iq(), |w| {
        let t = w["templates"].as_array_mut().unwrap();
        t.reverse();
    });
    assert_eq!(image_key(&same), key);
    let fewer = with(iq(), |w| w["templates"].as_array_mut().unwrap().truncate(2));
    assert_ne!(image_key(&fewer), key);
    assert_eq!(image_ref(&key, "b"), format!("localhost/wadspaces-set-{}:latest", &key[4..]));
    assert_eq!(image_ref("base", "localhost/wadspaces-base:trixie"), "localhost/wadspaces-base:trixie");
}

#[test]
fn version_3s_helpers_read_it_as_a_template_and_a_setup() {
    let (t, setup) = as_template_setup(&iq());
    assert_eq!(t["apps"], json!(["wadbrowser", "vscode", "terminal", "libresprite", "tiled"]));
    assert_eq!(setup["browserMode"], "focus");
    assert_eq!(setup["tools"], json!(["cpp", "rust"]));
    assert_eq!(
        setup["webapps"][1],
        json!({"id": "learnopengl", "name": "LearnOpenGL", "url": "https://learnopengl.com", "iconUrl": "/i.png"})
    );
    assert_eq!(setup["autostart"], json!(["app:vscode"]));
    let slots: Vec<String> =
        layers::slots_for(&t, &setup).iter().map(|s| s["key"].as_str().unwrap().to_string()).collect();
    assert!(slots.contains(&"ext:anthropic.claude-code".to_string()), "{slots:?}");
    let desktop: Vec<String> =
        layers::desktop_for(&t, &setup, &[]).iter().map(|d| d["id"].as_str().unwrap().to_string()).collect();
    assert!(
        desktop.contains(&"app:terminal".to_string()) && desktop.contains(&"web:github".to_string()),
        "{desktop:?}"
    );
}

#[test]
fn it_installs_on_its_set_with_its_name_sign_ins_and_projects() {
    let ws = workspace_for(&iq(), &json!({"tz": "Pacific/Fiji"}));
    assert_eq!(ws["image"], format!("localhost/wadspaces-set-{}:latest", &image_key(&iq())[4..]));
    assert_eq!(
        (ws["name"].as_str(), ws["env"]["TITLE"].as_str()),
        (Some("IntelligenceQuest"), Some("IntelligenceQuest"))
    );
    assert_eq!(ws["env"]["WALLPAPER"], "/run/wadspaces-extra/wallpaper.png");
    assert_eq!(ws["volumes"], json!(["wad-iq-config:/config:z"]));
    let signins: Vec<(&str, &str)> = ws["signins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["slot"].as_str().unwrap(), s["id"].as_str().unwrap()))
        .collect();
    // LearnOpenGL is in no group: version 3's rule puts it in the first.
    assert_eq!(
        signins,
        [
            ("ext:anthropic.claude-code", "iq-claude"),
            ("app:wadbrowser", "hunggod"),
            ("web:github", "hunggod"),
            ("web:learnopengl", "hunggod")
        ]
    );
    assert_eq!(ws["projects"][0]["repo"], "HungGod/IntelligenceQuest");
    let bare = workspace_for(
        &json!({"version": 4, "id": "b", "templates": [], "customization": {"name": "B"}}),
        &json!({"base": "x/base:1"}),
    );
    assert_eq!(bare["image"], "x/base:1");
}

#[test]
fn what_it_brings_when_it_starts() {
    let x = extras(&iq());
    let ids: Vec<&str> = x["webapps"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["github", "learnopengl"]);
    let layout: Value = serde_json::from_str(x["layout"].as_str().unwrap()).unwrap();
    let icons: Vec<(&str, Option<&str>)> = layout["icons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["app"].as_str().unwrap(), i["desktop"].as_str()))
        .collect();
    assert_eq!(icons[0], ("wadbrowser", Some("wadbrowser.desktop")));
    assert!(icons.contains(&("webapp-github", Some("wadspaces-webapp-github.desktop"))), "{icons:?}");
    assert_eq!(layout["icons"][1]["autostart"], true, "VS Code opens at start");
    assert!(x["wadbrowserConf"].as_str().unwrap().contains("default = focus"));
    // Without WadBrowser: no web apps, and the base's own wadbrowser.conf.
    let art = with(iq(), |w| w["templates"] = json!([w["templates"][2].clone()]));
    let x = extras(&art);
    assert_eq!((x["webapps"].clone(), x["wadbrowserConf"].clone()), (json!([]), Value::Null));
    let desktop: Vec<String> = desktop(&iq()).iter().map(|d| d["id"].as_str().unwrap().to_string()).collect();
    assert!(desktop.contains(&"project:project-iq".to_string()), "{desktop:?}");
}

#[test]
fn whether_its_built_comes_from_the_machines_images() {
    let key = image_key(&iq());
    let base = json!({"layer": "base", "id": "b2"});
    assert_eq!(state(&iq(), std::slice::from_ref(&base)), "never");
    assert_eq!(state(&iq(), &[base.clone(), json!({"layer": key, "from": "b2"})]), "built");
    assert_eq!(state(&iq(), &[base.clone(), json!({"layer": key, "from": "b1"})]), "update");
    let bare = json!({"version": 4, "templates": []});
    assert_eq!(state(&bare, &[base]), "built");
}

#[test]
fn what_the_board_shows() {
    let v = view(&iq());
    assert_eq!(
        (v["name"].as_str(), v["machine"].as_str(), v["deleted"].as_bool()),
        (Some("IntelligenceQuest"), Some("surface"), Some(false))
    );
    assert_eq!(v["imageKey"], json!(image_key(&iq())));
    assert_eq!(v["wallpaper"]["value"], "stars");
}

#[test]
fn the_readers_of_what_a_wadspace_is_made_of_read_it() {
    use std::collections::HashMap;
    let w = iq();
    // Its templates are the chain (the details page's "Built on").
    let r = layers::resolve_stack(&w);
    let names: Vec<&str> = js::arr(&r, "chain").iter().map(|t| s(t, "name")).collect();
    assert_eq!(names, ["Dev", "C++ and Rust", "Game art"]);
    assert_eq!(r["setup"]["browserMode"], "focus");
    assert_eq!(layers::stack_image_key(&w), image_key(&w));
    assert_eq!(layers::not_ready(&w), None);
    let unnamed = with(iq(), |w| w["customization"]["name"] = json!(" "));
    assert_eq!(layers::not_ready(&unnamed), Some("Its customization needs a name."));

    let key = image_key(&w);
    let row = |id: &str, from: &str| layers::LayerImage { id: id.into(), from: from.into(), sha: String::new() };
    let mut images = HashMap::from([("base".to_string(), row("b2", ""))]);
    assert_eq!(layers::image_state(&key, &images), layers::ImageState::Never);
    images.insert(key.clone(), row("s1", "b1"));
    assert_eq!(layers::image_state(&key, &images), layers::ImageState::Update);
    images.insert(key.clone(), row("s2", "b2"));
    assert_eq!(layers::image_state(&key, &images), layers::ImageState::Built);
    assert_eq!(layers::image_state("base", &images), layers::ImageState::Built);
}
