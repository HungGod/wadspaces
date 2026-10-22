use serde_json::{Value, json};

use super::*;

fn apps(ids: &[&str]) -> Vec<AppIn> {
    ids.iter().map(|a| AppIn { app: a.to_string() }).collect()
}

fn web(id: &str, name: &str, url: &str) -> Ext {
    Ext { name: Some(name.into()), url: Some(url.into()), ..Ext::plain("wadbrowser", id) }
}

fn ws(id: &str, machine: &str, name: &str, apps: Vec<AppIn>, exts: Vec<Ext>, custom: Value) -> Wadspace {
    let mut c = custom.as_object().cloned().unwrap_or_default();
    c.insert("name".into(), name.into());
    Wadspace {
        version: 5,
        id: id.into(),
        owner: "u1".into(),
        shared_with: vec![],
        machine: machine.into(),
        order: 0,
        apps,
        exts,
        customization: c,
        created_at: Some("2026-10-20T00:00:00Z".into()),
        updated_at: Some("2026-10-20T00:00:00Z".into()),
        deleted_at: None,
        trash: vec![],
    }
}

/// IntelligenceQuest and Cosmic on the Surface, Vanua on the Laptop.
pub fn board() -> Board {
    let iq = ws(
        "iq",
        "surface",
        "IntelligenceQuest",
        apps(&["wadbrowser", "vscode", "terminal", "libresprite", "tiled"]),
        vec![
            web("github", "GitHub", "https://github.com"),
            Ext::plain("vscode", "anthropic.claude-code"),
            Ext::plain("vscode", "ms-vscode.cpptools"),
            Ext::plain("terminal", "cpp"),
        ],
        json!({
            "description": "A game.",
            "wallpaper": {"type": "texture", "value": "stars"},
            "projects": [{"id": "project-iq", "kind": "github", "repo": "HungGod/IntelligenceQuest", "as": "IntelligenceQuest"}],
            "secrets": {
                "bySlot": {"ext:anthropic.claude-code": {"id": "iq-claude", "name": "IQ-claude", "kind": "vscode-claude-code"}},
                "groups": [{"id": "g1", "members": ["browser", "web:github"], "key": {"id": "hunggod", "name": "hunggod", "kind": "web"}}]
            }
        }),
    );
    let cosmic = ws("cosmic", "surface", "Cosmic Bodybuilding", apps(&["obsidian", "wadbrowser"]), vec![], json!({}));
    let vanua = ws("vanua", "laptop", "VanuaAcademy", apps(&["vscode"]), vec![], json!({}));
    let machines = vec![
        Machine { id: "surface".into(), name: "Surface Book 2".into() },
        Machine { id: "laptop".into(), name: "Laptop".into() },
    ];
    Board::new(machines, vec![iq, cosmic, vanua], "2026-10-21T10:00:00Z")
}

fn app_ids(b: &Board, ws: &str) -> Vec<String> {
    b.get(ws).unwrap().apps.iter().map(|a| a.app.clone()).collect()
}

fn ext_ids(b: &Board, ws: &str) -> Vec<String> {
    b.get(ws).unwrap().exts.iter().map(|x| x.id.clone()).collect()
}

fn row(b: &Board, m: &str) -> Vec<String> {
    b.row(m).iter().map(|w| w.name().to_string()).collect()
}

fn drop_op(payload: Payload, into: &str) -> Op {
    Op::Drop { payload, into: into.into() }
}

fn secrets(b: &Board, ws: &str) -> Value {
    b.get(ws).unwrap().customization["secrets"].clone()
}

#[test]
fn a_card_dropped_on_another_is_merged_into_it() {
    let mut b = board();
    b.apply(&drop_op(Payload::Wadspace { ws: "iq".into() }, "cosmic")).unwrap();
    assert_eq!(app_ids(&b, "cosmic"), ["obsidian", "wadbrowser", "vscode", "terminal", "libresprite", "tiled"]);
    assert_eq!(ext_ids(&b, "cosmic"), ["github", "anthropic.claude-code", "ms-vscode.cpptools", "cpp"]);
    let c = &b.get("cosmic").unwrap().customization;
    assert_eq!((c["name"].as_str(), c["description"].as_str()), (Some("Cosmic Bodybuilding"), Some("A game.")));
    assert_eq!((c["wallpaper"]["value"].as_str(), c["projects"].as_array().unwrap().len()), (Some("stars"), 1));
    assert_eq!(c["secrets"], secrets(&b, "iq"));
    assert_eq!(b.get("iq").unwrap(), &board().wadspaces[0], "the original stays as it was");
}

#[test]
fn nothing_moves_around_inside_its_own_wadspace() {
    let mut b = board();
    for p in [
        Payload::App { ws: "iq".into(), app: "tiled".into() },
        Payload::Customs { ws: "iq".into() },
        Payload::Key { ws: "iq".into(), key: "hunggod".into() },
    ] {
        b.apply(&drop_op(p, "iq")).unwrap();
    }
    assert_eq!(b.wadspaces, board().wadspaces);
    assert!(!b.can_undo());
}

#[test]
fn an_add_on_brings_its_app_along() {
    let mut b = board();
    b.apply(&drop_op(
        Payload::Ext { ws: "iq".into(), app: "vscode".into(), id: "ms-vscode.cpptools".into() },
        "cosmic",
    ))
    .unwrap();
    assert_eq!(app_ids(&b, "cosmic"), ["obsidian", "wadbrowser", "vscode"]);
    assert_eq!(ext_ids(&b, "cosmic"), ["ms-vscode.cpptools"]);
    // WadBrowser's Focus from the sidebar: WadBrowser's mode.
    b.apply(&drop_op(Payload::CatalogExt { ext: Ext::focus() }, "cosmic")).unwrap();
    let browser = v5::apps_of(&b.get("cosmic").unwrap().doc()).into_iter().find(|e| e["app"] == "wadbrowser");
    assert_eq!(browser.unwrap()["mode"], "focus");
    // All of an app's add-ons; all its apps (each once).
    b.apply(&drop_op(Payload::Exts { ws: "iq".into(), app: "vscode".into() }, "vanua")).unwrap();
    b.apply(&drop_op(Payload::Apps { ws: "iq".into() }, "vanua")).unwrap();
    assert_eq!(ext_ids(&b, "vanua"), ["anthropic.claude-code", "ms-vscode.cpptools"]);
    assert_eq!(app_ids(&b, "vanua"), ["vscode", "wadbrowser", "terminal", "libresprite", "tiled"]);
    // Again: nothing twice.
    b.apply(&drop_op(Payload::Exts { ws: "iq".into(), app: "vscode".into() }, "vanua")).unwrap();
    assert_eq!(ext_ids(&b, "vanua").len(), 2);
}

#[test]
fn customization_secrets_background_and_projects_go_on_their_own() {
    let mut b = board();
    b.apply(&drop_op(Payload::Customs { ws: "iq".into() }, "vanua")).unwrap();
    let v = &b.get("vanua").unwrap();
    assert_eq!(v.apps, apps(&["vscode"]), "no apps with it");
    assert_eq!((v.wallpaper().unwrap()["value"].as_str(), v.projects().len()), (Some("stars"), 1));
    assert_eq!(v.customization["secrets"], secrets(&b, "iq"));
    b.apply(&drop_op(Payload::Secrets { ws: "iq".into() }, "cosmic")).unwrap();
    assert_eq!(secrets(&b, "cosmic"), secrets(&b, "iq"));
    assert_eq!(b.get("cosmic").unwrap().wallpaper(), None);
    b.apply(&drop_op(Payload::Background { ws: "iq".into() }, "cosmic")).unwrap();
    b.apply(&drop_op(Payload::Project { ws: "iq".into(), id: "project-iq".into() }, "cosmic")).unwrap();
    b.apply(&drop_op(Payload::Texture { value: "grid".into() }, "vanua")).unwrap();
    let c = b.get("cosmic").unwrap();
    assert_eq!((c.wallpaper().unwrap()["value"].as_str(), c.projects().len()), (Some("stars"), 1));
    assert_eq!(b.get("vanua").unwrap().wallpaper().unwrap()["value"], "grid");
}

#[test]
fn a_new_wadspace_between_cards_after_them_or_on_another_machine() {
    let mut b = board();
    // An app between IQ and Cosmic: a copy of IQ's name.
    b.apply(&Op::Fresh {
        payload: Payload::App { ws: "iq".into(), app: "tiled".into() },
        machine: "surface".into(),
        before: Some("cosmic".into()),
    })
    .unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest", "IntelligenceQuest - 1", "Cosmic Bodybuilding"]);
    assert_eq!(app_ids(&b, "intelligencequest-1"), ["tiled"]);
    // From the sidebar, after the last: "New wadspace".
    b.apply(&Op::Fresh {
        payload: Payload::CatalogApp { app: "krita".into() },
        machine: "surface".into(),
        before: None,
    })
    .unwrap();
    assert_eq!(row(&b, "surface")[3], "New wadspace");
    // The whole card onto the Laptop: its name kept.
    b.apply(&Op::Fresh { payload: Payload::Wadspace { ws: "iq".into() }, machine: "laptop".into(), before: None })
        .unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy", "IntelligenceQuest"]);
    let copy = b.row("laptop")[1];
    assert_eq!((copy.id.as_str(), copy.exts.len(), copy.owner.as_str()), ("intelligencequest", 4, "u1"));
}

#[test]
fn a_wadspace_from_nothing_is_empty_and_named_new() {
    let mut b = board();
    b.apply(&Op::Create { machine: "laptop".into() }).unwrap();
    b.apply(&Op::Create { machine: "laptop".into() }).unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy", "New wadspace", "New wadspace - 1"]);
    let w = b.row("laptop")[2];
    assert_eq!((w.id.as_str(), w.apps.len(), w.exts.len()), ("new-wadspace-1", 0, 0));
    assert_eq!(b.new_name("laptop"), "New wadspace - 2");
    // Made and given its first app as one step (as the client does it).
    let id = b.free_id(&b.new_name("surface"));
    let first = drop_op(Payload::CatalogApp { app: "obsidian".into() }, &id);
    b.apply_all(&[Op::Create { machine: "surface".into() }, first]).unwrap();
    assert_eq!(app_ids(&b, &id), ["obsidian"]);
    b.apply(&Op::Undo).unwrap();
    assert!(b.get(&id).is_none());
}

#[test]
fn copying_to_another_machine_keeps_the_name_and_is_one_undo() {
    let mut b = board();
    b.apply(&Op::CopyTo { ids: vec!["iq".into(), "cosmic".into()], machine: "laptop".into() }).unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy", "IntelligenceQuest", "Cosmic Bodybuilding"]);
    let copy = b.row("laptop")[1];
    assert_eq!((copy.id.as_str(), copy.machine.as_str()), ("intelligencequest", "laptop"));
    assert_eq!(copy.exts, b.get("iq").unwrap().exts);
    b.apply(&Op::Undo).unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy"]);
    assert!(!b.can_undo());
}

#[test]
fn whats_taken_out_waits_and_comes_back() {
    let mut b = board();
    let picks = vec![
        Pick::Wadspace { ws: "cosmic".into() },
        Pick::Exts { ws: "iq".into(), app: "vscode".into() },
        Pick::Project { ws: "iq".into(), id: "project-iq".into() },
        Pick::Background { ws: "iq".into() },
    ];
    b.apply(&Op::Delete { picks }).unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest"]);
    assert_eq!(b.deleted()[0].deleted_at.as_deref(), Some("2026-10-21T10:00:00Z"));
    let iq = b.get("iq").unwrap();
    assert_eq!(
        (ext_ids(&b, "iq"), iq.wallpaper(), iq.projects().len()),
        (vec!["github".into(), "cpp".into()], None, 0)
    );
    let kinds: Vec<(&str, &str)> = iq
        .trash
        .iter()
        .map(|k| {
            let kind = match &k.piece {
                Piece::Exts { .. } => "exts",
                Piece::App { .. } => "app",
                Piece::Project { .. } => "project",
                Piece::Background { .. } => "background",
            };
            (k.id.as_str(), kind)
        })
        .collect();
    assert_eq!(kinds, [("k1", "exts"), ("k2", "project"), ("k3", "background")]);
    b.apply(&Op::PutBack { ws: "iq".into(), kept: "k1".into() }).unwrap();
    b.apply(&Op::PutBack { ws: "iq".into(), kept: "k3".into() }).unwrap();
    b.apply(&Op::Forget { ws: "iq".into(), kept: Some("k2".into()) }).unwrap();
    let iq = b.get("iq").unwrap();
    assert_eq!(ext_ids(&b, "iq"), ["github", "cpp", "anthropic.claude-code", "ms-vscode.cpptools"]);
    assert_eq!((iq.wallpaper().is_some(), iq.trash.len()), (true, 0));
    assert_eq!(
        b.apply(&Op::PutBack { ws: "iq".into(), kept: "k2".into() }).unwrap_err(),
        "that's not in Recently deleted"
    );
    b.apply(&Op::Restore { ws: "cosmic".into() }).unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest", "Cosmic Bodybuilding"]);
}

#[test]
fn an_app_out_leaves_its_add_ons_waiting_for_it() {
    let mut b = board();
    b.apply(&Op::RemoveApp { ws: "iq".into(), app: "vscode".into() }).unwrap();
    let iq = b.get("iq").unwrap();
    assert!(!iq.has_app("vscode") && iq.exts.iter().any(|x| x.app == "vscode"));
    let made: Vec<String> = v5::apps_of(&iq.doc()).iter().map(|e| e["app"].as_str().unwrap().into()).collect();
    assert_eq!(made, ["wadbrowser", "terminal", "libresprite", "tiled"]);
    assert_eq!(iq.trash[0].piece, Piece::App { app: AppIn { app: "vscode".into() } });
    // Its key isn't on the card while it's out.
    assert!(!b.secrets_by_key("iq").iter().any(|e| e.key["id"] == "iq-claude"));
    b.apply(&Op::PutBack { ws: "iq".into(), kept: "k1".into() }).unwrap();
    assert_eq!(app_ids(&b, "iq"), ["wadbrowser", "terminal", "libresprite", "tiled", "vscode"]);
    // One add-on out: not kept (Undo brings it back).
    b.apply(&Op::RemoveExt { ws: "iq".into(), app: "terminal".into(), id: "cpp".into() }).unwrap();
    assert!(!ext_ids(&b, "iq").contains(&"cpp".to_string()) && b.get("iq").unwrap().trash.is_empty());
}

#[test]
fn recently_deleted_keeps_the_newest() {
    let mut b = board();
    b.now = "2026-10-21T00:00:00Z".into();
    let p = |n: usize| json!({"id": format!("p{n}"), "kind": "github", "repo": "o/r", "as": format!("r{n}")});
    for n in 0..KEEP_MOST + 5 {
        let op = Op::ToggleProject { ws: "iq".into(), project: p(n) };
        b.apply(&op).unwrap();
        b.apply(&op).unwrap();
    }
    let trash = &b.get("iq").unwrap().trash;
    assert_eq!(trash.len(), KEEP_MOST);
    assert_eq!(trash[0].piece, Piece::Project { project: p(5) });
    assert_eq!(trash[KEEP_MOST - 1].piece, Piece::Project { project: p(KEEP_MOST + 4) });
}

#[test]
fn keys_and_web_apps_in_groups() {
    let mut b = board();
    // Cosmic's WadBrowser has no groups yet: one, with the browser in it.
    assert_eq!(serde_json::to_value(b.groups_of("cosmic")).unwrap(), json!([{"id": "g1", "members": ["browser"]}]));
    // GitHub into a group of its own, then back: the emptied group goes.
    b.apply(&Op::MoveMember { ws: "iq".into(), member: "web:github".into(), to: None }).unwrap();
    assert_eq!(b.groups_of("iq").iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), ["g1", "g2"]);
    b.apply(&Op::MoveMember { ws: "iq".into(), member: "web:github".into(), to: Some("g1".into()) }).unwrap();
    assert_eq!(b.groups_of("iq").len(), 1);
    b.apply(&Op::SetGroupKey { ws: "iq".into(), group: "g1".into(), key: None }).unwrap();
    assert_eq!(b.groups_of("iq")[0].key, None);
    b.apply(&Op::SetKey {
        ws: "cosmic".into(),
        slot: "app:obsidian".into(),
        key: Some(json!({"id": "hunggod", "name": "hunggod", "kind": "obsidian"})),
    })
    .unwrap();
    // Renamed everywhere it's used.
    b.apply(&Op::RenameKey { id: "hunggod".into(), name: "Doug".into() }).unwrap();
    assert_eq!(secrets(&b, "cosmic")["bySlot"]["app:obsidian"]["name"], "Doug");
    b.apply(&Op::SetKey { ws: "iq".into(), slot: "ext:anthropic.claude-code".into(), key: None }).unwrap();
    assert_eq!(secrets(&b, "iq")["bySlot"], json!({}));
    let slots: Vec<String> = b.slots_of("iq").iter().map(|s| s["key"].as_str().unwrap().into()).collect();
    assert!(slots.contains(&"ext:anthropic.claude-code".to_string()), "{slots:?}");
}

#[test]
fn what_signs_in_with_a_key_goes_onto_another() {
    let mut b = board();
    let work = json!({"id": "work", "name": "Work", "kind": "web"});
    let code = json!({"id": "code", "name": "Code", "kind": "vscode"});
    let gh = HolderRef { slot: "group:g1".into(), member: Some("web:github".into()) };
    // GitHub onto a key the wadspace hasn't got: a group of its own with it.
    b.apply(&Op::AssignKey { ws: "iq".into(), holders: vec![gh.clone()], key: Some(work.clone()) }).unwrap();
    let groups = b.groups_of("iq");
    assert_eq!(groups.iter().map(|g| (g.id.as_str(), g.members.len())).collect::<Vec<_>>(), [("g1", 1), ("g2", 1)]);
    assert_eq!(groups[1].key, Some(work.clone()));
    // The browser onto Work too: into that group.
    let browser = HolderRef { slot: "group:g1".into(), member: Some("browser".into()) };
    b.apply(&Op::AssignKey { ws: "iq".into(), holders: vec![browser], key: Some(work.clone()) }).unwrap();
    assert_eq!(serde_json::to_value(b.groups_of("iq")).unwrap()[0]["members"], json!(["web:github", "browser"]));
    // No key: into the key-less group (a new one).
    b.apply(&Op::AssignKey { ws: "iq".into(), holders: vec![gh], key: None }).unwrap();
    assert_eq!(b.groups_of("iq").iter().filter(|g| g.key.is_none()).count(), 1);
    // VS Code's settings take a VS Code key; a web key signs nothing in there.
    let settings = HolderRef { slot: "app:vscode".into(), member: None };
    b.apply(&Op::AssignKey { ws: "iq".into(), holders: vec![settings.clone()], key: Some(code.clone()) }).unwrap();
    assert_eq!(secrets(&b, "iq")["bySlot"]["app:vscode"], code);
    assert_eq!(
        b.apply(&Op::AssignKey { ws: "iq".into(), holders: vec![settings], key: Some(work) }).unwrap_err(),
        "Work is a web key: that doesn't sign in with it"
    );
}

#[test]
fn a_key_dropped_on_another_wadspace_brings_what_signs_in_with_it() {
    let mut b = board();
    b.apply(&drop_op(Payload::Key { ws: "iq".into(), key: "hunggod".into() }, "cosmic")).unwrap();
    let g = b.groups_of("cosmic");
    assert_eq!(
        (g.len(), g[0].members.as_slice(), g[0].key.as_ref().map(|k| k["id"].clone())),
        (1, &["browser".to_string()][..], Some(json!("hunggod")))
    );
    // One thing off a key: Claude Code's.
    let claude = Payload::Holder {
        ws: "iq".into(),
        key: "iq-claude".into(),
        slot: "ext:anthropic.claude-code".into(),
        member: None,
    };
    b.apply(&drop_op(claude, "vanua")).unwrap();
    assert_eq!(secrets(&b, "vanua")["bySlot"]["ext:anthropic.claude-code"]["id"], "iq-claude");
}

#[test]
fn a_cards_secrets_each_key_with_what_signs_in() {
    let b = board();
    let keys = b.secrets_by_key("iq");
    let shown: Vec<(&str, &str, Vec<&str>)> = keys
        .iter()
        .map(|e| {
            (e.key["name"].as_str().unwrap(), e.icon.as_str(), e.holders.iter().map(|h| h.name.as_str()).collect())
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("hunggod", "/brand/wadbrowser-icon-dark.svg", vec!["WadBrowser", "GitHub"]),
            ("IQ-claude", "/icons/claude.png", vec!["Claude Code"])
        ]
    );
    assert_eq!(keys[0].holders[1].what, "GitHub, a web app in WadBrowser");
    let held = b.holders_of(&Payload::Key { ws: "iq".into(), key: "hunggod".into() }).unwrap();
    assert_eq!((held.holders.len(), held.kind.as_str(), held.app_id.as_str()), (2, "web", "WadBrowser"));
    let one = Payload::Holder {
        ws: "iq".into(),
        key: "hunggod".into(),
        slot: "group:g1".into(),
        member: Some("web:github".into()),
    };
    assert_eq!(b.holders_of(&one).unwrap().app_id, "WadBrowser-GitHub");
    assert!(b.holders_of(&Payload::Wadspace { ws: "iq".into() }).is_none());
}

#[test]
fn new_keys_are_named_after_what_they_sign_in() {
    let b = board();
    assert_eq!(
        b.new_key_for("iq", "VSCode-ClaudeCode", "vscode-claude-code", &[]),
        json!({"id": "intelligencequest-vscode-claudecode", "name": "IntelligenceQuest-VSCode-ClaudeCode", "kind": "vscode-claude-code"})
    );
    // A name a key has (any kind, however it's written) is taken.
    assert_eq!(b.new_key("HungGod", "obsidian", &[])["name"], "HungGod-2");
    let known = [json!({"id": "work", "name": "Work", "kind": "web"})];
    assert_eq!(b.new_key("work", "web", &known), json!({"id": "work-2", "name": "work-2", "kind": "web"}));
    assert_eq!(b.key_clash("iq-claude", "HungGod", &[]).as_deref(), Some("Another key is already called hunggod"));
    assert_eq!(b.key_clash("hunggod", "hunggod", &[]), None);
    assert!(b.key_clash("hunggod", "work", &known).is_some());
    assert_eq!(b.key_clash("x", " ", &[]).as_deref(), Some("Give it a name"));
}

#[test]
fn its_keys_by_app_as_the_key_editor_shows_them() {
    let b = board();
    let map = b.key_map("iq");
    let ids: Vec<&str> = map.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["wadbrowser", "vscode"]);
    let web = &map[0].lines[0];
    assert_eq!(web.slot, "group:g1");
    let members: Vec<&str> = web.members.iter().flatten().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["WadBrowser", "GitHub"]);
    let code: Vec<&str> = map[1].lines.iter().map(|l| l.slot.as_str()).collect();
    assert_eq!(code, ["app:vscode", "ext:anthropic.claude-code"]);
    let cosmic: Vec<(String, usize)> = b.key_map("cosmic").iter().map(|a| (a.id.clone(), a.lines.len())).collect();
    assert_eq!(cosmic, [("obsidian".to_string(), 1), ("wadbrowser".to_string(), 1)]);
    let names: Vec<Value> = b.keys().iter().map(|k| k["name"].clone()).collect();
    assert_eq!(names, [json!("IQ-claude"), json!("hunggod")]);
}

#[test]
fn a_new_wadspace_never_takes_another_documents_id() {
    let mut b = board();
    b.reserved = vec!["new-wadspace".into()];
    b.apply(&Op::Fresh {
        payload: Payload::CatalogApp { app: "obsidian".into() },
        machine: "surface".into(),
        before: None,
    })
    .unwrap();
    assert!(b.get("new-wadspace-2").is_some(), "{:?}", b.wadspaces.iter().map(|w| &w.id).collect::<Vec<_>>());
}

#[test]
fn edits_made_together_undo_as_one() {
    let mut b = board();
    let both = [
        Op::Rename { ws: "iq".into(), name: "IQ".into() },
        Op::Describe { ws: "iq".into(), description: "Rust".into() },
    ];
    b.apply_all(&both).unwrap();
    assert_eq!((b.get("iq").unwrap().name(), b.can_undo()), ("IQ", true));
    b.apply(&Op::Undo).unwrap();
    assert_eq!((b.get("iq").unwrap().name(), b.can_undo()), ("IntelligenceQuest", false));
    // One that can't be made: neither is.
    let bad = [Op::Rename { ws: "iq".into(), name: "IQ".into() }, Op::Rename { ws: "gone".into(), name: "x".into() }];
    assert!(b.apply_all(&bad).is_err());
    assert_eq!(b.get("iq").unwrap().name(), "IntelligenceQuest");
}

#[test]
fn saving_writes_what_changed_with_each_rows_order() {
    let saved = board().wadspaces;
    let mut b = board();
    // Nothing changed but the orders the saved board never had.
    let first = b.plan(&saved, "u1", "2026-10-21T11:00:00Z");
    assert_eq!(first.len(), 3);
    assert_eq!(first.iter().map(|d| d["order"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2, 1]);
    assert_eq!(
        (first[0]["version"].as_u64(), first[1]["exts"].clone()),
        (Some(5), json!([])),
        "apps and add-ons always there"
    );
    let saved: Vec<Wadspace> = first.iter().map(|d| serde_json::from_value(d.clone()).unwrap()).collect();
    assert!(b.plan(&saved, "u1", "2026-10-21T12:00:00Z").is_empty(), "nothing new");
    b.apply(&Op::Rename { ws: "cosmic".into(), name: "Cosmic".into() }).unwrap();
    let next = b.plan(&saved, "u1", "2026-10-21T12:00:00Z");
    assert_eq!(next.len(), 1);
    assert_eq!((next[0]["id"].as_str(), next[0]["createdAt"].as_str()), (Some("cosmic"), Some("2026-10-20T00:00:00Z")));
    assert_eq!(next[0]["updatedAt"], "2026-10-21T12:00:00Z");
    for d in &first {
        assert_eq!(v5::check(d), Ok(()), "{d}");
    }
}

#[test]
fn an_edit_that_cant_be_made_changes_nothing() {
    let mut b = board();
    let before = b.wadspaces.clone();
    assert!(b.apply(&Op::SetBackground { ws: "iq".into(), texture: Some("plaid".into()) }).is_err());
    assert!(b.apply(&drop_op(Payload::Apps { ws: "nope".into() }, "iq")).is_err());
    assert!(b.apply(&drop_op(Payload::App { ws: "iq".into(), app: "krita".into() }, "cosmic")).is_err());
    assert_eq!(b.wadspaces, before);
    assert!(!b.can_undo());
    assert_eq!(b.apply(&Op::Undo).unwrap_err(), "nothing to undo");
}

#[test]
fn ops_are_json() {
    let op: Op = serde_json::from_value(
        json!({"op": "assignKey", "ws": "iq", "holders": [{"slot": "group:g1", "member": "browser"}], "key": null}),
    )
    .unwrap();
    assert!(
        matches!(op, Op::AssignKey { ref holders, key: None, .. } if holders[0].member.as_deref() == Some("browser"))
    );
    let p: Payload = serde_json::from_value(
        json!({"kind": "catalogExt", "ext": {"app": "wadbrowser", "id": "focus", "name": "Focus"}}),
    )
    .unwrap();
    assert_eq!(p, Payload::CatalogExt { ext: Ext::focus() });
    let out = run(&json!({"board": board(), "ops": [{"op": "rename", "ws": "iq", "name": "IQ"}, {"op": "restore", "ws": "nope"}]})).unwrap();
    assert_eq!(out["errors"], json!([null, "no wadspace nope"]));
    assert_eq!(out["wadspaces"][0]["customization"]["name"], "IQ");
    let kept = Kept { id: "k1".into(), at: "t".into(), piece: Piece::Exts { app: "vscode".into(), exts: vec![] } };
    assert_eq!(
        serde_json::to_value(kept).unwrap(),
        json!({"id": "k1", "at": "t", "kind": "exts", "app": "vscode", "exts": []})
    );
}

#[test]
fn add_ons_by_their_app_with_their_names_and_pictures() {
    let mut w = board().wadspaces[0].clone();
    w.exts.insert(0, Ext::plain("terminal", "rust"));
    w.exts.push(Ext::focus());
    let order: Vec<(String, usize)> = w.exts_by_app().iter().map(|(a, xs)| (a.clone(), xs.len())).collect();
    assert_eq!(order, [("wadbrowser".to_string(), 2), ("vscode".to_string(), 2), ("terminal".to_string(), 2)]);
    use crate::catalog::{ext_face, ext_what, exts_of, site_webapp};
    assert_eq!(
        (exts_of("vscode"), exts_of("terminal")),
        ("VS Code's extensions".into(), "Command line's tools".into())
    );
    assert_eq!(ext_face(&Ext::focus()), ("Focus".to_string(), String::new()));
    assert_eq!(ext_face(&Ext::plain("vscode", "anthropic.claude-code")).1, "/icons/claude.png");
    assert_eq!(ext_face(&Ext::plain("terminal", "rust")), ("Rust".to_string(), String::new()));
    assert_eq!(ext_what(&Ext::plain("terminal", "rust")), "rustup, cargo and the stable toolchain");
    assert_eq!(ext_what(&web("notion", "Notion", "https://www.notion.so")), "notion.so");
    assert_eq!(ext_what(&web("github", "GitHub", "https://github.com")), "Code, issues and pull requests");
    let site = site_webapp("www.focus.app").unwrap();
    assert_eq!((site.id.as_str(), site.name.as_deref()), ("focus-app", Some("Focus")), "never Focus's id");
    assert!(site_webapp("not a site").is_none());
}

// ------------------------------------------------------------ goldens
// fixtures/board/b-inputs.json: named scenarios; b.json: what they return,
// which the Android app's port replays. These keep b.json current and say
// what the scenarios must show, so a rewrite can't quietly change them.

const INPUTS: &str = include_str!("../../../../fixtures/board/b-inputs.json");
const B: &str = include_str!("../../../../fixtures/board/b.json");

fn made() -> Vec<Value> {
    let mut cases: Vec<Value> = serde_json::from_str(INPUTS).unwrap();
    for c in cases.iter_mut() {
        let args = c["args"].as_array().unwrap().clone();
        c["out"] = crate::call(c["fn"].as_str().unwrap(), &args).unwrap();
    }
    cases
}

fn case<'a>(cases: &'a [Value], name: &str) -> &'a Value {
    &cases.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no case {name:?}"))["out"]
}

#[test]
fn b_goldens_are_current() {
    let now: Vec<Value> = serde_json::from_str(B).unwrap();
    assert!(
        now == made(),
        "fixtures/board/b.json is out of date: UPDATE_GOLDENS=1 cargo test -p wad-board b_goldens -- --ignored"
    );
}

/// `UPDATE_GOLDENS=1 cargo test -p wad-board b_goldens -- --ignored` rewrites
/// fixtures/board/b.json. Look at the diff before committing it.
#[test]
#[ignore = "rewrites fixtures/board/b.json; run on purpose"]
fn rewrite_b_goldens() {
    if std::env::var("UPDATE_GOLDENS").as_deref() != Ok("1") {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/board/b.json");
    std::fs::write(path, serde_json::to_string_pretty(&made()).unwrap() + "\n").unwrap();
}

#[test]
fn what_the_scenarios_show() {
    let all = made();
    let names = |out: &Value, machine: &str| -> Vec<String> {
        out["wadspaces"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["machine"] == machine && w.get("deletedAt").is_none())
            .map(|w| w["customization"]["name"].as_str().unwrap().to_string())
            .collect()
    };
    let of =
        |out: &Value, id: &str| out["wadspaces"].as_array().unwrap().iter().find(|w| w["id"] == id).unwrap().clone();
    let card = case(&all, "a card copied into another, and into its own");
    let cosmic = of(card, "cosmic");
    assert_eq!(cosmic["apps"].as_array().unwrap().len(), 6);
    assert_eq!(card["errors"], json!([null, null]));
    let ext = case(&all, "an add-on brings its app; apps and add-ons on their own");
    assert!(
        of(ext, "vanua")["apps"].as_array().unwrap().iter().any(|a| a["app"] == "wadbrowser"),
        "Focus brought WadBrowser"
    );
    let copied = case(&all, "copied to another machine, then undone, then again");
    assert_eq!(names(copied, "laptop"), ["VanuaAcademy", "IntelligenceQuest"]);
    let fresh = case(&all, "new wadspaces between cards, after them and from nothing");
    assert_eq!(
        names(fresh, "surface"),
        ["IntelligenceQuest", "IntelligenceQuest - 1", "Cosmic Bodybuilding", "New wadspace", "New wadspace - 1"]
    );
    let kept = case(&all, "Recently deleted: what's taken out waits in its wadspace");
    let iq = of(kept, "iq");
    let trash: Vec<&str> = iq["trash"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
    assert_eq!(trash, ["background"], "the add-ons and the app back, the project forgotten");
    let keys = case(&all, "keys onto things, things onto keys");
    assert_eq!(keys["errors"].as_array().unwrap().last().unwrap(), "Work is a web key: that doesn't sign in with it");
    let cant = case(&all, "edits that can't be made");
    assert_eq!(
        cant["errors"],
        json!(["there's no texture plaid", "no wadspace nope", "no such add-on", "nothing to undo"])
    );
    assert_eq!(cant["plan"].as_array().unwrap().len(), 3, "the first save: every order is new");
    let saved = case(&all, "saving: what changed, each row's order");
    let plan = saved["plan"].as_array().unwrap();
    assert_eq!((plan.len(), plan[0]["id"].as_str()), (1, Some("cosmic")));
    assert_eq!(
        case(&all, "a new key's name and id"),
        &json!({"id": "intelligencequest-wadbrowser-2", "name": "IntelligenceQuest-WadBrowser-2", "kind": "web"})
    );
}
