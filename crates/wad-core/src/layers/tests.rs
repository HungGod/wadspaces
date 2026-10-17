//! The layers' rules, on a sample library: the Writing and
//! Dev starters, a Dev Extensions layer with seven web apps, and a version 2
//! wadspace on each kind of base; then template chains and version 3 stacks.
//! Secret names here are made up.

use serde_json::{Value, json};

use super::*;

fn web(name: &str, url: &str) -> Value {
    json!({ "id": webapp_id(name), "name": name, "url": url })
}

fn writing() -> Value {
    json!({ "id": "writing", "owner": "me", "name": "Writing", "description": "", "apps": ["obsidian"], "createdAt": "2026-01-01", "updatedAt": "2026-01-01" })
}

fn dev() -> Value {
    json!({ "id": "dev", "owner": "me", "name": "Dev", "description": "", "apps": ["wadbrowser", "vscode"], "createdAt": "2026-01-02", "updatedAt": "2026-01-02" })
}

fn academy_dev() -> Value {
    json!({
        "id": "academy-dev", "owner": "me", "name": "Academy Dev", "description": "", "templateId": "dev",
        "browserMode": "browser",
        "webapps": [
            web("GitHub", "https://github.com"),
            web("Claude", "https://claude.ai"),
            web("Google FireBase", "https://firebase.google.com"),
            web("Google Cloud", "https://cloud.google.com"),
            web("Gmail", "https://mail.google.com"),
            web("Google Drive", "https://drive.google.com"),
            web("Google Workspace", "https://workspace.google.com"),
        ],
        "extensions": ["anthropic.claude-code"], "tools": ["firebase"], "autostart": [],
        "createdAt": "2026-02-01", "updatedAt": "2026-02-01",
    })
}

fn r(id: &str, name: &str, kind: &str) -> Value {
    json!({ "id": id, "name": name, "kind": kind })
}

fn wadspaces() -> Vec<Value> {
    let (cloud, mail) = (r("s-cloud", "Work-WadBrowser-Google", "web"), r("s-mail", "Mail-WadBrowser-Google", "web"));
    vec![
        json!({ "version": 2, "id": "novel", "owner": "me", "name": "Novel", "description": "",
                "base": { "kind": "template", "id": "writing" }, "wallpaper": { "type": "image", "value": "/wallpapers/x.png" },
                "projects": ["writing"], "secrets": {}, "createdAt": "2026-03-01", "updatedAt": "2026-03-01" }),
        json!({ "version": 2, "id": "academy", "owner": "me", "name": "Academy", "description": "",
                "base": { "kind": "customization", "id": "academy-dev" }, "wallpaper": { "type": "color", "value": "#000" },
                "projects": ["academy"], "createdAt": "2026-03-02", "updatedAt": "2026-03-02",
                "secrets": {
                    "ext:anthropic.claude-code": r("s-claude", "Me-VSCode-ClaudeCode", "vscode-claude-code"),
                    "web:github": r("s-gh", "Me-WadBrowser-GitHub", "web"),
                    "web:claude": r("s-cl", "Academy-WadBrowser-Claude", "web"),
                    "web:google-firebase": cloud, "web:google-cloud": cloud,
                    "web:gmail": mail, "web:google-drive": mail, "web:google-workspace": mail,
                } }),
    ]
}

fn lib() -> Value {
    json!({ "templates": [writing(), dev()], "customizations": [academy_dev()], "wadspaces": wadspaces() })
}

fn keys(v: &[Value], k: &str) -> Vec<String> {
    v.iter().map(|x| x[k].as_str().unwrap_or("").to_string()).collect()
}

#[test]
fn resolves_a_base_to_its_template_and_setup() {
    let got = resolve(&lib(), &json!({ "kind": "customization", "id": "academy-dev" }));
    assert_eq!(got["template"]["id"], "dev");
    assert_eq!(got["setup"]["tools"], json!(["firebase"]));
    assert_eq!(resolve(&lib(), &json!({ "kind": "template", "id": "writing" }))["setup"], empty_setup("writing"));
    assert!(resolve(&lib(), &json!({ "kind": "template", "id": "gone" })).is_null());
}

#[test]
fn offers_a_slot_per_web_app_ai_extension_and_app_that_keeps_a_sign_in() {
    let slots = slots_for(&dev(), &academy_dev());
    // In desktop order: WadBrowser's web apps, then VS Code's own (its
    // settings and accounts) and its extension's.
    let mut want: Vec<String> =
        js::arr(&academy_dev(), "webapps").iter().map(|w| format!("web:{}", w["id"].as_str().unwrap())).collect();
    want.extend(["app:vscode".into(), "ext:anthropic.claude-code".into()]);
    assert_eq!(keys(&slots, "key"), want);
    let w = slots_for(&writing(), &empty_setup("writing"));
    assert_eq!((keys(&w, "key"), keys(&w, "kind")), (vec!["app:obsidian".into()], vec!["obsidian".into()]));
    // An extension that keeps nothing, or Copilot (signs in inside each wadspace), has no slot.
    let mut plain = academy_dev();
    plain["webapps"] = json!([]);
    plain["extensions"] = json!(["ms-python.python", "github.copilot-chat"]);
    assert_eq!(keys(&slots_for(&dev(), &plain), "key"), ["app:vscode"]);
    let steam = json!({ "id": "games", "name": "Games", "apps": ["steam", "dolphin"] });
    assert_eq!(keys(&slots_for(&steam, &empty_setup("games")), "kind"), ["steam"]);
}

#[test]
fn names_new_secrets_after_the_wadspace_or_the_setup() {
    let slots = slots_for(&dev(), &academy_dev());
    let slot = |k: &str| slots.iter().find(|s| s["key"] == k).unwrap().clone();
    assert_eq!(new_secret_name("Academy", &slot("ext:anthropic.claude-code")), "Academy-VSCode-ClaudeCode");
    assert_eq!(new_secret_name("Vanua Academy", &slot("web:github")), "VanuaAcademy-WadBrowser-GitHub");
    assert_eq!(new_secret_name("", &slot("web:github")), "Wadspace-WadBrowser-GitHub");
    let b = browser_slots(&json!({ "webapps": [web("GitHub", "https://github.com")] }));
    assert_eq!(b[0]["label"], "GitHub");
    assert_eq!(default_browser_secret_name("Cloud work", &b[0]), "Cloudwork-WadBrowser-GitHub");
    assert_eq!(default_browser_secret_name("  ", &b[0]), "WadSpaces-WadBrowser.GitHub");
}

#[test]
fn only_mounts_a_secret_where_its_kind_fits() {
    let slots = slots_for(&dev(), &academy_dev());
    let slot = |k: &str| slots.iter().find(|s| s["key"] == k).unwrap().clone();
    assert!(fits(&json!({ "kind": "web" }), &slot("web:github")));
    assert!(!fits(&json!({ "kind": "web" }), &slot("ext:anthropic.claude-code")));
    assert!(!fits(&json!({ "kind": "obsidian" }), &slot("web:github")));
    assert_eq!(kind_of_slot_key("ext:openai.chatgpt").as_deref(), Some("vscode-codex"));
    assert_eq!(kind_of_slot_key("app:steam").as_deref(), Some("steam"));
    assert_eq!(kind_of_slot_key("web:x").as_deref(), Some("web"));
    assert_eq!(kind_of_slot_key("app:vscode").as_deref(), Some("vscode"));
    assert_eq!(kind_of_slot_key("app:krita"), None);
}

#[test]
fn shares_secrets_as_the_slots_say() {
    let browsers =
        vec![json!({ "id": "mail", "name": "Mail", "secrets": { "web:gmail": r("s-mail", "Mail", "web") } })];
    let use_ = secret_use(&wadspaces(), &browsers);
    let slots = |id: &str| {
        let mut v: Vec<String> =
            use_[id].as_array().unwrap().iter().map(|u| u["slot"].as_str().unwrap().into()).collect();
        v.sort();
        v
    };
    assert_eq!(slots("s-cloud"), ["web:google-cloud", "web:google-firebase"]);
    assert_eq!(slots("s-mail"), ["web:gmail", "web:gmail", "web:google-drive", "web:google-workspace"]);
    assert_eq!(use_["s-mail"].as_array().unwrap().last().unwrap()["kind"], "browser");
}

#[test]
fn puts_apps_then_web_apps_then_project_folders_on_the_desktop() {
    let projects = [json!({ "id": "academy", "name": "Academy" })];
    let items = desktop_for(&dev(), &academy_dev(), &projects);
    let mut want = vec!["WadBrowser".to_string(), "VS Code".into()];
    want.extend(js::arr(&academy_dev(), "webapps").iter().map(|w| w["name"].as_str().unwrap().to_string()));
    want.push("Academy".into());
    assert_eq!(keys(&items, "label"), want);
    let mut focus = academy_dev();
    focus["browserMode"] = "focus".into();
    // Focus is how WadBrowser opens (wadbrowser.conf), not another app.
    assert_eq!(desktop_for(&dev(), &focus, &[])[0]["label"], "WadBrowser");
    // Nothing in Writing opens a folder in a window: its projects are mounted with no icon.
    let w = desktop_for(&writing(), &empty_setup("writing"), &[json!({ "id": "writing", "name": "Writing" })]);
    assert_eq!(keys(&w, "label"), ["Obsidian"]);
    // Bundled icons for sites the catalog knows, so they draw offline.
    assert_eq!(items[0]["iconUrl"], "/brand/wadbrowser-icon-dark.svg");
}

#[test]
fn marks_vs_code_with_extensions_added() {
    let added = |setup: &Value| {
        desktop_for(&dev(), setup, &[]).into_iter().find(|i| i["id"] == "app:vscode").unwrap()["added"].clone()
    };
    assert_eq!(added(&academy_dev()), 1);
    assert!(added(&empty_setup("dev")).is_null());
}

#[test]
fn lists_every_launcher_in_the_start_menu_by_category() {
    let menu = start_menu_for(&dev(), &academy_dev());
    assert_eq!(keys(&menu, "name"), ["Development", "Internet", "System"]);
    let internet = keys(menu[1]["apps"].as_array().unwrap(), "name");
    for n in ["WadBrowser", "GitHub", "Gmail"] {
        assert!(internet.contains(&n.to_string()), "{n}");
    }
    let mut sorted = internet.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(internet, sorted);
    assert!(!internet.contains(&"WadBrowser Focus".to_string()), "one WadBrowser");
    // A template without WadBrowser has none in its menu.
    let writing_menu = start_menu_for(&writing(), &empty_setup("writing"));
    let all: Vec<String> = writing_menu.iter().flat_map(|c| keys(c["apps"].as_array().unwrap(), "name")).collect();
    assert!(!all.iter().any(|n| n.starts_with("WadBrowser")), "{all:?}");
}

#[test]
fn builds_a_template_on_the_base_and_extensions_on_its_template() {
    let t = template_dockerfile(&dev(), &Value::Null);
    assert!(t.contains("ARG BASE_IMAGE=localhost/wadspaces-base:trixie\nFROM ${BASE_IMAGE}"), "{t}");
    assert!(t.contains("RUN wadspaces-feature vscode\n"), "{t}");
    assert!(t.contains("LABEL io.wadspaces.layer=\"template:dev\" io.wadspaces.apps=\"wadbrowser,vscode\""), "{t}");
    assert!(t.contains("ENV TITLE=\"Dev\""), "{t}");
    let apt =
        template_dockerfile(&json!({ "id": "m", "name": "Music", "apps": ["lmms", "obs", "audacity"] }), &Value::Null);
    assert!(apt.contains("RUN wadspaces-feature audacity\n"), "{apt}");
    assert!(
        apt.contains("RUN wadspaces-apt lmms --desktop lmms.desktop lmms && \\\n    wadspaces-apt obs --desktop com.obsproject.Studio.desktop obs-studio && \\\n    wadspaces-apt --clean"),
        "{apt}"
    );
    let d = extensions_dockerfile(&academy_dev(), &dev());
    assert!(d.contains("ARG BASE_IMAGE=localhost/wadspaces-template-dev:latest\nFROM ${BASE_IMAGE}"), "{d}");
    assert!(d.contains("LABEL io.wadspaces.layer=\"custom:academy-dev\""), "{d}");
    assert!(d.contains("RUN wadspaces-feature firebase\n"), "{d}");
    assert!(d.contains("RUN wadspaces-vscode-extensions anthropic.claude-code\n"), "{d}");
    assert!(
        d.contains("wadspaces-webapp --id google-firebase \"Google FireBase\" \"https://firebase.google.com\""),
        "{d}"
    );
    assert!(d.ends_with("COPY root/ /\n"));
    let mut boot = academy_dev();
    boot["autostart"] = json!(["app:vscode", "web:gmail"]);
    let layout: Value = serde_json::from_str(&layout_json_for(&dev(), &boot)).unwrap();
    assert_eq!(
        layout["icons"][1],
        json!({ "app": "vscode", "desktop": "wadspaces-vscode.desktop", "label": "VS Code", "autostart": true })
    );
    assert_eq!(
        layout["icons"][6],
        json!({ "app": "webapp-gmail", "desktop": "wadspaces-webapp-gmail.desktop", "label": "Gmail", "autostart": true })
    );
}

#[test]
fn nothing_a_shell_reads_gets_into_a_dockerfile() {
    let mut c = academy_dev();
    c["extensions"] = json!(["ok.ext", "bad;rm -rf /", "$(x).y"]);
    c["webapps"] = json!([{ "id": "x", "name": "A \"quote\" $HOME", "url": "https://a.b/$(x)" }, { "id": "Bad Id", "name": "n", "url": "https://c" }]);
    c["tools"] = json!(["go", "not-a-feature"]);
    let d = extensions_dockerfile(&c, &dev());
    assert!(d.contains("RUN wadspaces-vscode-extensions ok.ext\n"), "{d}");
    assert!(d.contains(r#"wadspaces-webapp --id x "A \"quote\" \$HOME" "https://a.b/\$(x)""#), "{d}");
    assert!(!d.contains("Bad Id") && !d.contains("not-a-feature"), "{d}");
    let t = template_dockerfile(&json!({ "id": "t", "name": "Two\nlines", "apps": [] }), &Value::Null);
    assert!(t.starts_with("# Two lines template."), "{t}");
}

#[test]
fn layers_carry_their_files_and_a_sha_that_follows_them() {
    let t = template_layer(&dev(), &Value::Null);
    assert_eq!(
        (t["key"].as_str(), t["ref"].as_str()),
        (Some("template:dev"), Some("localhost/wadspaces-template-dev:latest"))
    );
    assert_eq!(t["from"], DEFAULT_BASE_IMAGE);
    let paths: Vec<&str> = t["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["Dockerfile", "root/etc/wadspaces/layout.json", "root/etc/wadspaces/wadbrowser.conf"]);
    let c = extensions_layer(&academy_dev(), &dev());
    assert_eq!(c["from"], "localhost/wadspaces-template-dev:latest");
    let mut focus = academy_dev();
    focus["browserMode"] = "focus".into();
    let f = extensions_layer(&focus, &dev());
    assert_ne!(f["sha"], c["sha"]);
    assert!(f["files"][2]["content"].as_str().unwrap().contains("default = focus"));
    assert_eq!(c["sha"], extensions_layer(&academy_dev(), &dev())["sha"]);
    assert_eq!(c["sha"].as_str().unwrap().len(), 64);
    let w = template_layer(&writing(), &Value::Null);
    assert_eq!(w["files"].as_array().unwrap().len(), 2, "no WadBrowser, no wadbrowser.conf");
}

#[test]
fn tells_a_changed_setup_from_its_base() {
    assert!(same_setup(&academy_dev(), &academy_dev()));
    let mut changed = academy_dev();
    changed["tools"] = json!([]);
    assert!(!same_setup(&academy_dev(), &changed));
    assert!(same_setup(&json!({ "templateId": "dev" }), &empty_setup("dev")));
}

#[test]
fn piles_each_templates_customizations_and_wadspaces_on_it() {
    let st = stacks(&lib());
    assert_eq!(st[0]["template"]["id"], "writing");
    assert_eq!(keys(st[0]["wadspaces"].as_array().unwrap(), "name"), ["Novel"]);
    assert_eq!(st[0]["customizations"], json!([]));
    assert_eq!(st[1]["wadspaces"], json!([]));
    assert_eq!(st[1]["customizations"][0]["customization"]["name"], "Academy Dev");
    assert_eq!(keys(st[1]["customizations"][0]["wadspaces"].as_array().unwrap(), "name"), ["Academy"]);
    // Deleted ones leave the stacks.
    let mut l = lib();
    l["wadspaces"][0]["deletedAt"] = "2026-04-01".into();
    l["templates"][1]["deletedAt"] = "2026-04-01".into();
    let st = stacks(&l);
    assert_eq!(st.len(), 1);
    assert_eq!(st[0]["wadspaces"], json!([]));
}

#[test]
fn wont_take_a_name_already_in_use() {
    assert!(name_taken("dev", &lib(), None));
    assert!(name_taken(" Academy Dev ", &lib(), None));
    assert!(!name_taken("Academy Dev", &lib(), Some("academy-dev")));
    assert!(!name_taken("Night Writing", &lib(), None));
    assert!(named("novel", &wadspaces(), None));
    assert!(!named("Novel", &wadspaces(), Some("novel")));
}

#[test]
fn brings_what_fits_into_another_template_and_says_why_the_rest_doesnt() {
    let can = importable(&academy_dev(), &writing());
    assert_eq!(can["tools"]["ok"], true);
    assert_eq!(can["webapps"], json!({ "count": 7, "ok": false, "why": "Writing has no WadBrowser" }));
    assert_eq!(can["extensions"]["why"], "Writing has no VS Code");
    let next = import_into(&empty_setup("writing"), &academy_dev(), &writing(), &["webapps", "extensions", "tools"]);
    assert_eq!(next["templateId"], "writing");
    assert_eq!(
        (next["webapps"].clone(), next["extensions"].clone(), next["tools"].clone()),
        (json!([]), json!([]), json!(["firebase"]))
    );
}

#[test]
fn merges_without_repeats_keeping_boot_apps_that_exist_here() {
    let mut into = empty_setup("dev");
    into["webapps"] = json!([{ "id": "github", "name": "GitHub", "url": "https://github.com/" }]);
    into["tools"] = json!(["firebase"]);
    let mut from = academy_dev();
    from["autostart"] = json!(["web:claude", "app:obsidian"]);
    let next = import_into(&into, &from, &dev(), &["webapps", "extensions", "tools", "autostart"]);
    let mut want = vec!["GitHub".to_string()];
    want.extend(js::arr(&academy_dev(), "webapps")[1..].iter().map(|w| w["name"].as_str().unwrap().to_string()));
    assert_eq!(keys(next["webapps"].as_array().unwrap(), "name"), want);
    assert_eq!(next["tools"], json!(["firebase"]));
    assert_eq!(next["extensions"], json!(["anthropic.claude-code"]));
    // Obsidian isn't in Dev, so it can't open at boot here.
    assert_eq!(next["autostart"], json!(["web:claude"]));
}

#[test]
fn a_web_app_kept_once_brings_its_boot_key_along() {
    let mut into = empty_setup("dev");
    into["webapps"] = json!([{ "id": "hub", "name": "Hub", "url": "https://github.com" }]);
    let from =
        json!({ "templateId": "dev", "webapps": [web("GitHub", "https://github.com/")], "autostart": ["web:github"] });
    let next = import_into(&into, &from, &dev(), &["webapps", "autostart"]);
    assert_eq!(next["autostart"], json!(["web:hub"]));
}

#[test]
fn summarises_what_is_mounted_at_start() {
    let w = &wadspaces()[1];
    let projects = [json!({ "id": "academy", "name": "Academy", "mountName": "Academy" })];
    let lines = run_summary(w, &projects, &slots_for(&dev(), &academy_dev()));
    assert_eq!(lines[0], "wallpaper  → /run/wadspaces-extra/wallpaper.png (color)");
    assert_eq!(lines[1], "project    Academy → ~/Desktop/Academy");
    assert!(
        lines.contains(
            &"secret     Mail-WadBrowser-Google → Gmail (web app), Google Drive (web app), Google Workspace (web app)"
                .to_string()
        )
    );
    // As WadSpaces draws them: a picture is a JPEG (whatever it was), the rest PNGs.
    assert_eq!(wallpaper_ext(&json!({ "type": "image", "value": "/wallpapers/x.png" })), "jpg");
    assert_eq!(wallpaper_ext(&json!({ "type": "gradient", "value": "linear-gradient(#000, #fff)" })), "png");
}

#[test]
fn extension_ids_are_plain_marketplace_ids() {
    for ok in ["anthropic.claude-code", "rust-lang.rust-analyzer", "ms-vscode.cpptools", "a.b.c"] {
        assert!(is_extension_id(ok), "{ok}");
    }
    for bad in ["", "noperiod", ".x", "x.", "a b.c", "a.$(x)", "a;b.c"] {
        assert!(!is_extension_id(bad), "{bad}");
    }
}

#[test]
fn starters_are_writing_and_dev_from_catalog_apps() {
    let st = catalog::starters().as_array().unwrap();
    assert_eq!(keys(st, "name"), ["Writing", "Dev"]);
    for t in st {
        for a in strs(t, "apps") {
            assert!(catalog::catalog_app(a).is_some(), "{a}");
        }
    }
}

// ------------------------------------------------- chains and stacks (v3)

/// Dev + Tiled + LibreSprite, built on Dev.
fn gamedev() -> Value {
    json!({ "id": "gamedev", "owner": "me", "name": "Game Dev", "description": "", "parent": "dev",
            "apps": ["wadbrowser", "vscode", "tiled", "libresprite"], "createdAt": "2026-01-03", "updatedAt": "2026-01-03" })
}

/// Game Dev + LMMS, on Game Dev.
fn music() -> Value {
    json!({ "id": "music", "owner": "me", "name": "Game Music", "description": "", "parent": "gamedev",
            "apps": ["wadbrowser", "vscode", "tiled", "libresprite", "lmms"], "createdAt": "2026-01-04", "updatedAt": "2026-01-04" })
}

fn lib3() -> Value {
    json!({ "templates": [writing(), dev(), gamedev(), music()], "extensions": [academy_dev()] })
}

#[test]
fn a_template_builds_on_its_parents_image_adding_only_its_own_apps() {
    let d = template_dockerfile(&gamedev(), &dev());
    assert!(d.starts_with("# Game Dev template, built on Dev."), "{d}");
    assert!(d.contains("ARG BASE_IMAGE=localhost/wadspaces-template-dev:latest\nFROM ${BASE_IMAGE}"), "{d}");
    assert!(
        d.contains("io.wadspaces.apps=\"wadbrowser,vscode,tiled,libresprite\" io.wadspaces.parent=\"template:dev\""),
        "{d}"
    );
    assert!(d.contains("RUN wadspaces-feature tiled libresprite\n"), "only the new apps: {d}");
    assert!(!d.contains("feature vscode"), "{d}");
    let l = template_layer(&gamedev(), &dev());
    assert_eq!(l["from"], "localhost/wadspaces-template-dev:latest");
    // The desktop is the whole template's, the parent's apps first.
    let layout: Value = serde_json::from_str(l["files"][1]["content"].as_str().unwrap()).unwrap();
    let apps: Vec<&str> = layout["icons"].as_array().unwrap().iter().map(|i| i["app"].as_str().unwrap()).collect();
    assert_eq!(apps, ["wadbrowser", "vscode", "tiled", "libresprite"]);
    // Built on something else, it's another image.
    assert_ne!(l["sha"], template_layer(&gamedev(), &Value::Null)["sha"]);
}

#[test]
fn walks_a_chain_bottom_first_and_says_what_breaks_it() {
    let ids = |c: &[Value]| c.iter().map(|t| t["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&chain_of(&lib3(), "music")), ["dev", "gamedev", "music"]);
    assert_eq!(ids(&chain_of(&lib3(), "dev")), ["dev"]);
    assert!(chain_of(&lib3(), "nope").is_empty());
    assert_eq!(chain_ok(&chain_of(&lib3(), "music")), None);
    // A parent that's gone, a loop, an app dropped, nothing added.
    let mut orphan = gamedev();
    orphan["parent"] = "gone".into();
    assert_eq!(chain_ok(&[orphan]).unwrap(), "Game Dev builds on a template that's gone");
    let mut looped = lib3();
    looped["templates"][1]["parent"] = "music".into();
    assert!(chain_of(&looped, "music").len() <= 3);
    let mut dropped = gamedev();
    dropped["apps"] = json!(["wadbrowser", "tiled", "libresprite"]);
    assert_eq!(chain_ok(&[dev(), dropped]).unwrap(), "Game Dev can't build on Dev: it doesn't have VS Code");
    let mut same = gamedev();
    same["apps"] = json!(["wadbrowser", "vscode"]);
    assert_eq!(chain_ok(&[dev(), same]).unwrap(), "Game Dev adds no apps to Dev");
}

fn stack(template: &str, extensions: Option<&str>, customization: Value) -> Value {
    let mut w = json!({ "version": 3, "id": "s1", "owner": "me", "order": 1, "template": template,
                        "customization": customization, "createdAt": "2026-03-01", "updatedAt": "2026-03-01" });
    if let Some(e) = extensions {
        w["extensions"] = e.into();
    }
    w
}

#[test]
fn resolves_a_stack_to_its_chain_and_setup() {
    let custom =
        json!({ "name": "Academy", "description": "", "wallpaper": null, "projects": [], "secrets": { "bySlot": {} } });
    let r = resolve_stack(&lib3(), &stack("dev", Some("academy-dev"), custom.clone()));
    assert_eq!(r["template"]["id"], "dev");
    assert_eq!(r["extensions"]["id"], "academy-dev");
    assert_eq!(r["setup"]["webapps"].as_array().unwrap().len(), 7);
    assert_eq!(stack_image_key(&stack("dev", Some("academy-dev"), custom.clone())), "custom:academy-dev");
    // Extensions made on another template don't count there.
    let other = resolve_stack(&lib3(), &stack("music", Some("academy-dev"), custom.clone()));
    assert_eq!(other["chain"].as_array().unwrap().len(), 3);
    assert!(other.get("extensions").is_none());
    assert_eq!(stack_image_key(&stack("music", None, custom.clone())), "template:music");
    assert!(resolve_stack(&lib3(), &json!({ "version": 3, "id": "x" })).is_null());
}

#[test]
fn wadbrowser_key_groups_start_as_one_and_keep_empty_ones() {
    let items = key_items(&dev(), &academy_dev());
    assert_eq!(items.len(), 8);
    assert_eq!(items[0], "browser");
    let one = key_groups_of(&dev(), &academy_dev(), &Value::Null);
    assert_eq!(one, [json!({ "id": "all", "members": items })]);
    // Loose apps join the first group with members; gone ones drop; an empty group stays.
    let groups = json!([
        { "id": "empty", "members": ["web:gone"] },
        { "id": "mail", "members": ["web:gmail", "web:google-drive"], "key": r("s-mail", "Mail", "web") },
        { "id": "gh", "members": ["web:github"] },
    ]);
    let g = key_groups_of(&dev(), &academy_dev(), &groups);
    assert_eq!(g[0]["members"], json!([]));
    assert_eq!(g[1]["members"].as_array().unwrap().len(), 7, "mail takes the 5 loose ones");
    assert_eq!(g[2]["members"], json!(["web:github"]));
    assert!(key_groups_of(&writing(), &empty_setup("writing"), &groups).is_empty());
}

#[test]
fn a_stack_installs_with_its_keys_and_each_group_member_signing_in_with_its_groups() {
    let custom = json!({
        "name": "Vanua Academy", "description": "", "wallpaper": { "type": "gradient", "value": "linear-gradient(#000,#111)" },
        "projects": [],
        "secrets": {
            "bySlot": {
                "app:vscode": r("s-code", "Code", "vscode"),
                "ext:anthropic.claude-code": r("s-claude", "Claude", "vscode-claude-code"),
                "app:obsidian": r("s-obs", "Stray", "obsidian"),
            },
            // The keyless group is first: the web apps in no group join it.
            "groups": [
                { "id": "rest", "members": ["web:github"] },
                { "id": "mail", "members": ["browser", "web:gmail"], "key": r("s-mail", "Mail", "web") },
            ],
        },
    });
    let w = stack("dev", Some("academy-dev"), custom);
    let ws = workspace_for(&w, &dev(), &setup_of(&academy_dev()), &json!({ "tz": "Pacific/Fiji" }));
    assert_eq!(ws["image"], "localhost/wadspaces-custom-academy-dev:latest");
    assert_eq!(ws["name"], "Vanua Academy");
    assert_eq!(ws["env"]["WALLPAPER"], "/run/wadspaces-extra/wallpaper.png");
    // Title bars: only when its Look turns them on.
    assert!(ws["env"].get("WADSPACES_TITLEBARS").is_none());
    let mut barred = w.clone();
    barred["customization"]["titleBars"] = true.into();
    let ws_b = workspace_for(&barred, &dev(), &setup_of(&academy_dev()), &json!({}));
    assert_eq!(ws_b["env"]["WADSPACES_TITLEBARS"], "1");
    let signins: Vec<(String, String)> = ws["signins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["id"].as_str().unwrap().into(), x["app"].as_str().unwrap().into()))
        .collect();
    // Obsidian isn't in Dev: its key stays out. The keyless group signs nothing in.
    assert_eq!(
        signins,
        [("s-code", "vscode"), ("s-claude", "vscode"), ("s-mail", BROWSER_APP), ("s-mail", "gmail")]
            .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert_eq!(ws["signins"][0]["kind"], "vscode");
    assert_eq!(
        ws["signins"][2],
        json!({ "id": "s-mail", "app": "@browser", "web": true, "name": "Mail", "kind": "web", "slot": "app:wadbrowser" })
    );
}

#[test]
fn a_version_2_wadspace_becomes_a_stack() {
    let projects = [
        json!({ "id": "academy", "name": "Academy", "mountName": "Academy", "setup": "npm ci",
                "source": { "kind": "git", "url": "https://github.com/HungGod/VanuaAcademy.git", "ref": "main" } }),
        json!({ "id": "notes", "name": "Notes", "mountName": "Notes", "setup": "",
                "source": { "kind": "folder", "machineId": "m1", "machineName": "Surface", "path": "/var/home/wad/Notes" } }),
        json!({ "id": "photos", "name": "Photos", "mountName": "Photos", "setup": "",
                "source": { "kind": "drive", "uuid": "1234-ABCD", "label": "Photos", "fstype": "exfat", "subpath": "2026" } }),
    ];
    let mut v2 = wadspaces()[1].clone();
    v2["projects"] = json!(["academy", "notes", "photos", "gone"]);
    v2["sharedWith"] = json!([]);
    let v3 = migrate_v2(&v2, &lib(), &projects);
    assert_eq!(v3["version"], 3);
    assert_eq!((v3["template"].as_str(), v3["extensions"].as_str()), (Some("dev"), Some("academy-dev")));
    assert!(v3.get("base").is_none() && v3.get("name").is_none());
    let c = &v3["customization"];
    assert_eq!(c["name"], "Academy");
    assert_eq!(c["wallpaper"], json!({ "type": "color", "value": "#000" }));
    assert_eq!(
        c["projects"],
        json!([
            { "id": "academy", "kind": "github", "repo": "HungGod/VanuaAcademy", "ref": "main", "as": "Academy", "setup": "npm ci" },
            { "id": "notes", "kind": "copy", "source": { "machineId": "m1", "paths": ["/var/home/wad/Notes"] }, "as": "Notes" },
            { "id": "photos", "kind": "copy", "source": { "drive": { "uuid": "1234-ABCD", "label": "Photos", "fstype": "exfat" }, "paths": ["2026"] }, "as": "Photos" },
        ])
    );
    let keys = &c["secrets"];
    assert_eq!(
        keys["bySlot"],
        json!({ "ext:anthropic.claude-code": r("s-claude", "Me-VSCode-ClaudeCode", "vscode-claude-code") })
    );
    let groups: Vec<(String, Vec<String>)> = keys["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| (g["id"].as_str().unwrap().into(), strs(g, "members").into_iter().map(String::from).collect()))
        .collect();
    let g = |id: &str, m: &[&str]| (id.to_string(), m.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    assert_eq!(
        groups,
        [
            g("all", &["browser"]),
            g("g-s-gh", &["web:github"]),
            g("g-s-cl", &["web:claude"]),
            g("g-s-cloud", &["web:google-firebase", "web:google-cloud"]),
            g("g-s-mail", &["web:gmail", "web:google-drive", "web:google-workspace"]),
        ]
    );
    // Installed as before: the same sign-ins for the same web apps.
    let ws2 = workspace_for(&v2, &dev(), &setup_of(&academy_dev()), &json!({}));
    let ws3 = workspace_for(&v3, &dev(), &setup_of(&academy_dev()), &json!({}));
    let pairs = |ws: &Value| {
        let mut p: Vec<String> =
            ws["signins"].as_array().unwrap().iter().map(|x| format!("{}={}", x["app"], x["id"])).collect();
        p.sort();
        p
    };
    assert_eq!(pairs(&ws2), pairs(&ws3));
    assert_eq!(ws3["image"], ws2["image"]);
    // Running it again changes nothing.
    assert_eq!(migrate_v2(&v3, &lib(), &projects), v3);
    // On a template, with no web app keys: no groups.
    let novel = migrate_v2(&wadspaces()[0], &lib(), &[]);
    assert_eq!((novel["template"].as_str(), novel.get("extensions")), (Some("writing"), None));
    assert!(novel["customization"]["secrets"].get("groups").is_none());
}

#[test]
fn a_mount_goes_to_wadd_with_where_it_is() {
    let custom = json!({ "projects": [
        { "id": "mount-a1", "kind": "mount", "source": { "machineId": "m1", "paths": ["/var/home/wad/Thesis"] }, "as": "Thesis" },
        { "id": "x", "kind": "later", "as": "X" },
    ] });
    assert_eq!(
        super::workspace_projects(&custom),
        [json!({ "id": "mount-a1", "kind": "mount", "mount": "Thesis",
                 "from": { "machineId": "m1", "paths": ["/var/home/wad/Thesis"] } })]
    );
}
