use serde_json::{Value, json};

use super::*;

/// IntelligenceQuest on its way to Rust: WadBrowser in Focus with two web
/// apps, VS Code with three extensions, the command line's C++ and Rust, and
/// game art.
fn iq() -> Value {
    json!({
        "version": 5, "id": "iq", "owner": "u1", "machine": "surface", "order": 1,
        "apps": [{"app": "wadbrowser"}, {"app": "vscode"}, {"app": "terminal"}, {"app": "libresprite"}, {"app": "tiled"}],
        "exts": [
            {"app": "wadbrowser", "id": "focus", "name": "Focus"},
            {"app": "wadbrowser", "id": "github", "name": "GitHub", "url": "https://github.com"},
            {"app": "wadbrowser", "id": "learnopengl", "name": "LearnOpenGL", "url": "https://learnopengl.com", "iconUrl": "/i.png"},
            {"app": "vscode", "id": "anthropic.claude-code"},
            {"app": "vscode", "id": "ms-vscode.cpptools"},
            {"app": "vscode", "id": "rust-lang.rust-analyzer"},
            {"app": "terminal", "id": "cpp"},
            {"app": "terminal", "id": "rust"}
        ],
        "customization": customization()
    })
}

fn customization() -> Value {
    json!({
        "name": "IntelligenceQuest", "description": "A game.", "wallpaper": {"type": "texture", "value": "stars"},
        "autostart": ["app:vscode"],
        "projects": [{"id": "project-iq", "kind": "github", "repo": "HungGod/IntelligenceQuest", "as": "IntelligenceQuest"}],
        "secrets": {"bySlot": {"ext:anthropic.claude-code": {"id": "iq-claude", "name": "IQ Claude", "kind": "vscode-claude-code"}},
                    "groups": [{"id": "g1", "members": ["browser", "web:github"], "key": {"id": "hunggod", "name": "hunggod", "kind": "web"}}]}
    })
}

fn with(mut w: Value, f: impl FnOnce(&mut Value)) -> Value {
    f(&mut w);
    w
}

fn ids(list: &[Value], k: &str) -> Vec<String> {
    list.iter().map(|x| x[k].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_good_wadspace_checks() {
    assert_eq!(check(&iq()), Ok(()));
    // No apps and no background: an empty wadspace is one.
    let empty =
        json!({"version": 5, "id": "e", "machine": "m", "apps": [], "exts": [], "customization": {"name": "Empty"}});
    assert_eq!(check(&empty), Ok(()));
}

#[test]
fn what_check_refuses() {
    let bad = |f: fn(&mut Value)| check(&with(iq(), f)).unwrap_err();
    assert_eq!(check(&with(iq(), |w| w["version"] = json!(4))).unwrap_err(), "it isn't a version 5 wadspace");
    assert_eq!(bad(|w| w["templates"] = json!([])), "it still has version 4's templates");
    assert_eq!(bad(|w| w["machine"] = json!("")), "it isn't on a machine");
    assert_eq!(bad(|w| w["apps"][3]["app"] = json!("photoshop")), "there's no app \"photoshop\"");
    assert_eq!(bad(|w| w["apps"][4]["app"] = json!("libresprite")), "libresprite is in it twice");
    assert_eq!(bad(|w| w["apps"] = json!(vec![json!({"app": "tiled"}); 41])), "at most 40 apps");
    assert_eq!(bad(|w| w["exts"][0] = json!({"app": "libresprite", "id": "x"})), "nothing is added to libresprite");
    assert_eq!(bad(|w| w["exts"][6]["id"] = json!("java")), "there's no command-line tool \"java\"");
    assert_eq!(bad(|w| w["exts"][3]["id"] = json!("no dot")), "\"no dot\" isn't a VS Code extension's id");
    assert_eq!(bad(|w| w["exts"][1]["url"] = json!("github.com")), "the web app github needs an http(s) address");
    assert_eq!(
        bad(|w| w["exts"][0]["url"] = json!("https://focus.app")),
        "Focus is WadBrowser's own: it has no address"
    );
    assert_eq!(
        bad(|w| w["exts"][4]["id"] = json!("anthropic.claude-code")),
        "vscode's anthropic.claude-code is in it twice"
    );
    assert_eq!(bad(|w| w["exts"] = json!(vec![json!({"app": "terminal", "id": "rust"}); 121])), "at most 120 add-ons");
    assert!(bad(|w| w["customization"]["wallpaper"] = json!({"type": "image", "value": "x.jpg"})).contains("textures"));
    assert_eq!(bad(|w| w["customization"]["name"] = json!(" ")), "a wadspace needs a name of 1 to 80 characters");
    assert_eq!(
        bad(|w| w["trash"] = json!([{"id": "k1", "at": "t", "kind": "template"}])),
        "its Recently deleted can't keep \"template\""
    );
    assert_eq!(
        bad(|w| w["trash"] = json!(vec![json!({"id": "k", "at": "t", "kind": "background"}); 31])),
        "its Recently deleted keeps at most 30"
    );
}

#[test]
fn its_apps_carry_their_add_ons_and_focus_is_wadbrowsers_mode() {
    let apps = apps_of(&iq());
    assert_eq!(ids(&apps, "app"), ["wadbrowser", "vscode", "terminal", "libresprite", "tiled"]);
    assert_eq!(apps[0]["mode"], "focus");
    assert_eq!(ids(apps[0]["extras"].as_array().unwrap(), "id"), ["github", "learnopengl"], "Focus isn't a web app");
    assert_eq!(apps[0]["extras"][0], json!({"id": "github", "name": "GitHub", "url": "https://github.com"}));
    assert_eq!(
        ids(apps[1]["extras"].as_array().unwrap(), "id"),
        ["anthropic.claude-code", "ms-vscode.cpptools", "rust-lang.rust-analyzer"]
    );
    assert!(apps[1].get("mode").is_none() && apps[3]["extras"] == json!([]));
    let plain = with(iq(), |w| {
        w["exts"].as_array_mut().unwrap().remove(0);
    });
    assert!(apps_of(&plain)[0].get("mode").is_none(), "no Focus: the address bar");
}

#[test]
fn an_add_on_without_its_app_waits_and_counts_for_nothing() {
    let no_code = with(iq(), |w| {
        w["apps"].as_array_mut().unwrap().remove(1);
    });
    assert_eq!(check(&no_code), Ok(()));
    assert_eq!(ids(&apps_of(&no_code), "app"), ["wadbrowser", "terminal", "libresprite", "tiled"]);
    assert!(!packages_of(&no_code).iter().any(|p| p.starts_with("ext-")), "{:?}", packages_of(&no_code));
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
    let browser = json!({"version": 5, "apps": [{"app": "wadbrowser"}, {"app": "terminal"}], "exts": [focus()]});
    assert!(packages_of(&browser).is_empty());
    assert_eq!(image_key(&browser), "base");
}

#[test]
fn a_set_is_named_by_its_packages_alone() {
    let key = image_key(&iq());
    assert!(key.starts_with("set:") && key.len() == 16, "{key}");
    // The same packages in another order: the same set.
    let same = with(iq(), |w| {
        w["apps"].as_array_mut().unwrap().reverse();
        w["exts"].as_array_mut().unwrap().reverse();
    });
    assert_eq!(image_key(&same), key);
    let fewer = with(iq(), |w| w["apps"].as_array_mut().unwrap().truncate(3));
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
    // LearnOpenGL is in no group: it joins the first.
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
        &json!({"version": 5, "id": "b", "apps": [], "exts": [], "customization": {"name": "B"}}),
        &json!({"base": "x/base:1"}),
    );
    assert_eq!(bare["image"], "x/base:1");
}

#[test]
fn what_it_brings_when_it_starts() {
    let x = extras(&iq());
    assert_eq!(ids(x["webapps"].as_array().unwrap(), "id"), ["github", "learnopengl"]);
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
    // Without WadBrowser: no web apps (its add-ons wait), and the base's own
    // wadbrowser.conf.
    let art = with(iq(), |w| w["apps"] = json!([{"app": "libresprite"}, {"app": "tiled"}]));
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
    let bare = json!({"version": 5, "apps": [], "exts": []});
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
    let r = layers::resolve_stack(&w);
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
