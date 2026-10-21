use serde_json::{Value, json};

use super::*;

fn x(id: &str) -> Extra {
    Extra { id: id.into(), name: None, url: None, icon_url: None }
}

fn web(id: &str, name: &str, url: &str) -> Extra {
    Extra { id: id.into(), name: Some(name.into()), url: Some(url.into()), icon_url: None }
}

fn e(app: &str, extras: Vec<Extra>) -> Entry {
    Entry { app: app.into(), mode: None, extras }
}

fn t(id: &str, name: &str, apps: Vec<Entry>) -> Template {
    Template { id: id.into(), name: name.into(), apps }
}

fn ws(id: &str, machine: &str, name: &str, templates: Vec<Template>, custom: Value) -> Wadspace {
    let mut c = custom.as_object().cloned().unwrap_or_default();
    c.insert("name".into(), name.into());
    Wadspace {
        version: 4,
        id: id.into(),
        owner: "u1".into(),
        shared_with: vec![],
        machine: machine.into(),
        order: 0,
        templates,
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
        vec![
            t(
                "dev",
                "Dev",
                vec![
                    e("wadbrowser", vec![web("github", "GitHub", "https://github.com")]),
                    e("vscode", vec![x("anthropic.claude-code")]),
                ],
            ),
            t("cpp", "C++", vec![e("vscode", vec![x("ms-vscode.cpptools")]), e("terminal", vec![x("cpp")])]),
            t("art", "Game art", vec![e("libresprite", vec![]), e("tiled", vec![])]),
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
    let cosmic = ws(
        "cosmic",
        "surface",
        "Cosmic Bodybuilding",
        vec![t("writing", "Writing", vec![e("obsidian", vec![]), e("wadbrowser", vec![])])],
        json!({}),
    );
    let vanua = ws("vanua", "laptop", "VanuaAcademy", vec![t("dev", "Dev", vec![e("vscode", vec![])])], json!({}));
    let machines = vec![
        Machine { id: "surface".into(), name: "Surface Book 2".into() },
        Machine { id: "laptop".into(), name: "Laptop".into() },
    ];
    Board::new(machines, vec![iq, cosmic, vanua], "2026-10-21T10:00:00Z")
}

fn names(b: &Board, ws: &str) -> Vec<String> {
    b.get(ws).unwrap().templates.iter().map(|t| t.name.clone()).collect()
}

fn ids(b: &Board, ws: &str) -> Vec<String> {
    b.get(ws).unwrap().templates.iter().map(|t| t.id.clone()).collect()
}

fn row(b: &Board, m: &str) -> Vec<String> {
    b.row(m).iter().map(|w| w.name().to_string()).collect()
}

fn drop_op(payload: Payload, into: &str, at: Where) -> Op {
    Op::Drop { payload, into: into.into(), at }
}

#[test]
fn a_template_pile_into_another_wadspace_is_copied_there() {
    let mut b = board();
    let pile = Payload::Templates { ws: "iq".into(), ids: vec!["cpp".into(), "art".into()] };
    b.apply(&drop_op(pile, "cosmic", Where::default())).unwrap();
    assert_eq!(names(&b, "cosmic"), ["Writing", "C++", "Game art"]);
    assert_eq!(names(&b, "iq"), ["Dev", "C++", "Game art"], "the original stays");
    // Into Vanua, whose Dev clashes with IQ's: the copy's id is its own.
    b.apply(&drop_op(
        Payload::Templates { ws: "iq".into(), ids: vec!["dev".into()] },
        "vanua",
        Where { at: Some(0), ..Where::default() },
    ))
    .unwrap();
    assert_eq!(ids(&b, "vanua"), ["dev-2", "dev"]);
}

#[test]
fn a_template_moved_within_its_wadspace_lands_where_it_was_dropped() {
    let mut b = board();
    // Game art to the top.
    b.apply(&drop_op(
        Payload::Templates { ws: "iq".into(), ids: vec!["art".into()] },
        "iq",
        Where { at: Some(0), ..Where::default() },
    ))
    .unwrap();
    assert_eq!(names(&b, "iq"), ["Game art", "Dev", "C++"]);
    // Dev to below C++ (the slot after C++ is 3 as shown; 2 once Dev is out).
    b.apply(&drop_op(
        Payload::Templates { ws: "iq".into(), ids: vec!["dev".into()] },
        "iq",
        Where { at: Some(3), ..Where::default() },
    ))
    .unwrap();
    assert_eq!(names(&b, "iq"), ["Game art", "C++", "Dev"]);
}

#[test]
fn apps_and_extras_go_onto_templates_or_into_new_ones() {
    let mut b = board();
    // Rust from the sidebar: onto C++'s command line, with cpp.
    let rust = Payload::CatalogExtra { app: "terminal".into(), extra: x("rust") };
    b.apply(&drop_op(rust, "iq", Where { template: Some("cpp".into()), ..Where::default() })).unwrap();
    let cpp = &b.get("iq").unwrap().templates[1];
    assert_eq!(cpp.apps[1].extras.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["cpp", "rust"]);
    // A click (no template): an extension goes where its app is.
    let ra = Payload::CatalogExtra { app: "vscode".into(), extra: x("rust-lang.rust-analyzer") };
    b.apply(&drop_op(ra, "iq", Where::default())).unwrap();
    assert!(b.get("iq").unwrap().templates[0].apps[1].extras.iter().any(|x| x.id == "rust-lang.rust-analyzer"));
    // The New template field: a template of its own, named as the app.
    b.apply(&drop_op(
        Payload::CatalogApp { app: "audacity".into() },
        "iq",
        Where { new_template: true, ..Where::default() },
    ))
    .unwrap();
    assert_eq!(names(&b, "iq"), ["Dev", "C++", "Game art", "Audacity"]);
    // An app moved within the wadspace leaves its template; into another it's copied.
    let tiled = Payload::App { ws: "iq".into(), template: "art".into(), app: "tiled".into() };
    b.apply(&drop_op(tiled.clone(), "iq", Where { template: Some("audacity".into()), ..Where::default() })).unwrap();
    assert_eq!(b.get("iq").unwrap().templates[2].apps.len(), 1);
    let tiled = Payload::App { ws: "iq".into(), template: "audacity".into(), app: "tiled".into() };
    b.apply(&drop_op(tiled, "cosmic", Where { template: Some("writing".into()), ..Where::default() })).unwrap();
    assert_eq!(b.get("iq").unwrap().templates[3].apps.len(), 2, "copied, so still in IQ");
    assert_eq!(b.get("cosmic").unwrap().templates[0].apps.len(), 3);
}

#[test]
fn a_customizations_pile_on_the_open_board_is_the_whole_wadspace_again() {
    let mut b = board();
    b.apply(&Op::Fresh {
        payload: Payload::Bucket { ws: "iq".into(), which: Which::Customs, pile: true },
        machine: "surface".into(),
        before: None,
    })
    .unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest", "Cosmic Bodybuilding", "IntelligenceQuest - 1"]);
    let copy = b.get("intelligencequest-1").unwrap();
    assert_eq!(copy.templates.len(), 3);
    assert_eq!(copy.customization["wallpaper"]["value"], "stars");
    assert_eq!(copy.customization["secrets"]["bySlot"]["ext:anthropic.claude-code"]["id"], "iq-claude");
    // Just the bucket (Shift): its customizations, no templates; between IQ and Cosmic.
    b.apply(&Op::Fresh {
        payload: Payload::Bucket { ws: "iq".into(), which: Which::Customs, pile: false },
        machine: "surface".into(),
        before: Some("cosmic".into()),
    })
    .unwrap();
    assert_eq!(row(&b, "surface")[1], "IntelligenceQuest - 2");
    assert!(b.get("intelligencequest-2").unwrap().templates.is_empty());
}

#[test]
fn copying_to_another_machine_keeps_the_name_and_is_one_undo() {
    let mut b = board();
    b.apply(&Op::CopyTo { ids: vec!["iq".into(), "cosmic".into()], machine: "laptop".into() }).unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy", "IntelligenceQuest", "Cosmic Bodybuilding"]);
    let copy = b.row("laptop")[1];
    assert_eq!((copy.id.as_str(), copy.machine.as_str()), ("intelligencequest", "laptop"));
    assert_eq!(copy.templates, b.get("iq").unwrap().templates);
    b.apply(&Op::Undo).unwrap();
    assert_eq!(row(&b, "laptop"), ["VanuaAcademy"]);
    assert!(!b.can_undo());
}

#[test]
fn deleting_is_soft_and_a_wadspace_comes_back() {
    let mut b = board();
    let picks = vec![
        Pick::Wadspace { ws: "cosmic".into() },
        Pick::Template { ws: "iq".into(), id: "art".into() },
        Pick::Project { ws: "iq".into(), id: "project-iq".into() },
        Pick::Background { ws: "iq".into() },
    ];
    b.apply(&Op::Delete { picks }).unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest"]);
    assert_eq!(b.deleted()[0].deleted_at.as_deref(), Some("2026-10-21T10:00:00Z"));
    let iq = b.get("iq").unwrap();
    assert_eq!((iq.templates.len(), iq.customization.get("wallpaper")), (2, None));
    assert_eq!(iq.customization["projects"], json!([]));
    b.apply(&Op::Restore { ws: "cosmic".into() }).unwrap();
    assert_eq!(row(&b, "surface"), ["IntelligenceQuest", "Cosmic Bodybuilding"]);
}

#[test]
fn keys_and_web_apps_in_groups() {
    let mut b = board();
    // Cosmic's WadBrowser has no groups yet: one, with the browser in it.
    assert_eq!(b.groups_of("cosmic"), [json!({"id": "g1", "members": ["browser"]})]);
    // GitHub into a group of its own, then back: the emptied group goes.
    b.apply(&Op::MoveMember { ws: "iq".into(), member: "web:github".into(), to: None }).unwrap();
    assert_eq!(b.groups_of("iq").iter().map(|g| g["id"].as_str().unwrap()).collect::<Vec<_>>(), ["g1", "g2"]);
    b.apply(&Op::MoveMember { ws: "iq".into(), member: "web:github".into(), to: Some("g1".into()) }).unwrap();
    assert_eq!(b.groups_of("iq").len(), 1);
    // New group stays empty until it's filled.
    b.apply(&Op::NewGroup { ws: "iq".into() }).unwrap();
    assert_eq!(b.groups_of("iq")[1], json!({"id": "g2", "members": []}));
    // A new key: its name free, its id from it.
    let key = b.new_key("hunggod", "web");
    assert_eq!(key, json!({"id": "hunggod-2", "name": "hunggod-2", "kind": "web"}));
    b.apply(&Op::SetGroupKey { ws: "iq".into(), group: "g2".into(), key: Some(key) }).unwrap();
    b.apply(&Op::SetKey {
        ws: "cosmic".into(),
        slot: "app:obsidian".into(),
        key: Some(json!({"id": "hunggod", "name": "hunggod", "kind": "obsidian"})),
    })
    .unwrap();
    // Renamed everywhere it's used.
    b.apply(&Op::RenameKey { id: "hunggod".into(), name: "Doug".into() }).unwrap();
    assert_eq!(b.get("iq").unwrap().customization["secrets"]["groups"][0]["key"]["name"], "Doug");
    assert_eq!(b.get("cosmic").unwrap().customization["secrets"]["bySlot"]["app:obsidian"]["name"], "Doug");
    b.apply(&Op::SetKey { ws: "iq".into(), slot: "ext:anthropic.claude-code".into(), key: None }).unwrap();
    assert_eq!(b.get("iq").unwrap().customization["secrets"]["bySlot"], json!({}));
    let slots: Vec<String> = b.slots_of("iq").iter().map(|s| s["key"].as_str().unwrap().into()).collect();
    assert!(slots.contains(&"ext:anthropic.claude-code".to_string()), "{slots:?}");
}

#[test]
fn its_keys_by_app_as_the_key_editor_shows_them() {
    let b = board();
    let map = b.key_map("iq");
    let apps: Vec<&str> = map.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(apps, ["wadbrowser", "vscode"]);
    let web = &map[0].lines[0];
    assert_eq!(web.slot, "group:g1");
    assert_eq!(web.group.as_ref().and_then(|g| g.key.as_ref()).map(|k| k["name"].clone()), Some(json!("hunggod")));
    let members: Vec<&str> = web.members.iter().flatten().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["WadBrowser", "GitHub"]);
    let code: Vec<&str> = map[1].lines.iter().map(|l| l.slot.as_str()).collect();
    assert_eq!(code, ["app:vscode", "ext:anthropic.claude-code"]);
    // Obsidian keeps one; Cosmic's WadBrowser has a group of one, no key.
    let cosmic: Vec<(String, usize)> = b.key_map("cosmic").iter().map(|a| (a.id.clone(), a.lines.len())).collect();
    assert_eq!(cosmic, [("obsidian".to_string(), 1), ("wadbrowser".to_string(), 1)]);
    // The board's keys, each once; a name another key has is taken.
    let names: Vec<Value> = b.keys().iter().map(|k| k["name"].clone()).collect();
    assert_eq!(names, [json!("IQ-claude"), json!("hunggod")]);
    assert_eq!(b.key_clash("iq-claude", "HungGod", &[]).as_deref(), Some("Another key is already called hunggod"));
    assert_eq!(b.key_clash("hunggod", "hunggod", &[]), None);
    let known = [json!({"id": "work", "name": "Work", "kind": "web"})];
    assert!(b.key_clash("hunggod", "work", &known).is_some());
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
fn a_wadspace_from_nothing_has_an_empty_template_to_fill_in() {
    let mut b = board();
    let id = b.free_id(NEW_NAME);
    // Made and given its first app as one step (as the client does it).
    let first = Op::Drop {
        payload: Payload::CatalogApp { app: "obsidian".into() },
        into: id.clone(),
        at: Where { template: Some(BLANK_TEMPLATE.into()), ..Where::default() },
    };
    b.apply_all(&[Op::Create { machine: "laptop".into() }, first]).unwrap();
    let w = b.get(&id).unwrap();
    assert_eq!((w.name(), w.machine.as_str()), ("New wadspace", "laptop"));
    assert_eq!(w.templates.len(), 1);
    assert_eq!((w.templates[0].name.as_str(), w.templates[0].apps[0].app.as_str()), ("Template", "obsidian"));
    assert_eq!(b.row("laptop").iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["vanua", id.as_str()]);
    // A second takes the next id; one Undo takes the first back whole.
    b.apply(&Op::Create { machine: "laptop".into() }).unwrap();
    assert_eq!(b.row("laptop").last().unwrap().id, "new-wadspace-2");
    b.apply(&Op::Undo).unwrap();
    b.apply(&Op::Undo).unwrap();
    assert!(b.get(&id).is_none());
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
    let saved: Vec<Wadspace> = first.iter().map(|d| serde_json::from_value(d.clone()).unwrap()).collect();
    assert!(b.plan(&saved, "u1", "2026-10-21T12:00:00Z").is_empty(), "nothing new");
    b.apply(&Op::Rename { ws: "cosmic".into(), name: "Cosmic".into() }).unwrap();
    let next = b.plan(&saved, "u1", "2026-10-21T12:00:00Z");
    assert_eq!(next.len(), 1);
    assert_eq!((next[0]["id"].as_str(), next[0]["createdAt"].as_str()), (Some("cosmic"), Some("2026-10-20T00:00:00Z")));
    assert_eq!(next[0]["updatedAt"], "2026-10-21T12:00:00Z");
}

#[test]
fn an_edit_that_cant_be_made_changes_nothing() {
    let mut b = board();
    let before = b.wadspaces.clone();
    assert!(b.apply(&Op::SetBackground { ws: "iq".into(), texture: Some("plaid".into()) }).is_err());
    assert!(b.apply(&drop_op(Payload::Templates { ws: "nope".into(), ids: vec![] }, "iq", Where::default())).is_err());
    assert_eq!(b.wadspaces, before);
    assert!(!b.can_undo());
    assert_eq!(b.apply(&Op::Undo).unwrap_err(), "nothing to undo");
}

#[test]
fn ops_are_json() {
    let op: Op = serde_json::from_value(json!({"op": "drop", "payload": {"kind": "catalogApp", "app": "obs"}, "into": "iq", "where": {"newTemplate": true}})).unwrap();
    assert!(matches!(op, Op::Drop { at: Where { new_template: true, .. }, .. }));
    let out = run(&json!({"board": board(), "ops": [{"op": "rename", "ws": "iq", "name": "IQ"}, {"op": "restore", "ws": "nope"}]})).unwrap();
    assert_eq!(out["errors"], json!([null, "no wadspace nope"]));
    assert_eq!(out["wadspaces"][0]["customization"]["name"], "IQ");
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
    let copied = case(&all, "copied to another machine, then undone, then again");
    assert_eq!(names(copied, "laptop"), ["VanuaAcademy", "IntelligenceQuest"]);
    let pile = case(&all, "a customizations pile on the open board, then just the bucket");
    assert_eq!(
        names(pile, "surface"),
        ["IntelligenceQuest", "IntelligenceQuest - 2", "Cosmic Bodybuilding", "IntelligenceQuest - 1"]
    );
    assert_eq!(names(pile, "laptop"), ["VanuaAcademy", "New wadspace"]);
    let cant = case(&all, "edits that can't be made");
    assert_eq!(
        cant["errors"],
        json!(["there's no texture plaid", "no wadspace nope", "WadBrowser has no mode kiosk", "nothing to undo"])
    );
    assert_eq!(cant["plan"].as_array().unwrap().len(), 3, "the first save: every order is new");
    let saved = case(&all, "saving: what changed, each row's order");
    let plan = saved["plan"].as_array().unwrap();
    assert_eq!((plan.len(), plan[0]["id"].as_str()), (1, Some("cosmic")));
    assert_eq!(case(&all, "a new key's name and id"), &json!({"id": "hunggod-2", "name": "hunggod-2", "kind": "web"}));
    let blank = case(&all, "a wadspace from nothing, filled in");
    assert_eq!(names(blank, "laptop"), ["VanuaAcademy", "Notes", "New wadspace"]);
    let notes =
        &blank["wadspaces"].as_array().unwrap().iter().find(|w| w["id"] == "new-wadspace").unwrap()["templates"];
    assert_eq!(notes, &json!([{"id": "template", "name": "Template", "apps": [{"app": "obsidian", "extras": []}]}]));
}

#[test]
fn the_top_template_decides_the_mode() {
    let all = made();
    let out = case(&all, "WadBrowser's mode per template: the top one decides");
    let iq = out["wadspaces"].as_array().unwrap().iter().find(|w| w["id"] == "iq").unwrap();
    let browser = |w: &Value| wad_core::v4::apps_of(w).into_iter().find(|a| a["app"] == "wadbrowser").unwrap();
    // Game art (focus) went on top of Dev (an address bar): focus.
    assert_eq!(browser(iq)["mode"], "focus");
    // Dev back on top: its address bar, Game art's focus kept but not used.
    let mut moved = iq.clone();
    moved["templates"].as_array_mut().unwrap().swap(0, 1);
    assert!(browser(&moved).get("mode").is_none());
    assert_eq!(out["errors"][3], "C++ has no WadBrowser");
}

#[test]
fn whats_taken_out_waits_in_its_wadspace() {
    let all = made();
    let out = case(&all, "Recently deleted: what's taken out waits in its wadspace");
    let iq = out["wadspaces"].as_array().unwrap().iter().find(|w| w["id"] == "iq").unwrap();
    let tpls: Vec<&str> = iq["templates"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(tpls, ["dev", "cpp", "art"], "Game art back where it was");
    assert!(iq["templates"][1]["apps"].as_array().unwrap().iter().any(|e| e["app"] == "terminal"));
    let trash: Vec<(&str, &str)> = iq["trash"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["id"].as_str().unwrap(), k["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(trash, [("k4", "background")], "the project forgotten, the background still waiting");
    assert_eq!(out["errors"][7], "that's not in Recently deleted");
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
