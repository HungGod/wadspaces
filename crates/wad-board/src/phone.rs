//! The WadSpaces Android app's builder adds one stack at a time
//! ([`crate::sync::plan_new`]), and its own copy (Kotlin) is checked against
//! fixtures/board/phone.json: the scenarios in phone-inputs.json with what
//! they return here. These keep that file current, and say what the cases
//! must show, so a rewrite can't quietly change them.

use serde_json::Value;

const INPUTS: &str = include_str!("../../../fixtures/board/phone-inputs.json");
const PHONE: &str = include_str!("../../../fixtures/board/phone.json");

/// The scenarios with what they return now.
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

fn ids(list: &Value) -> Vec<&str> {
    list.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect()
}

#[test]
fn phone_goldens_are_current() {
    let now: Vec<Value> = serde_json::from_str(PHONE).unwrap();
    assert!(
        now == made(),
        "fixtures/board/phone.json is out of date: UPDATE_GOLDENS=1 cargo test -p wad-board phone -- --ignored"
    );
}

#[test]
fn what_the_cases_show() {
    let all = made();
    let new = case(&all, "catalog apps on an empty library");
    assert_eq!(ids(&new["templates"]), ["game-dev"]);
    let w = &new["wadspaces"][0];
    assert_eq!(
        (w["template"].as_str(), w["order"].as_i64(), w["version"].as_i64()),
        (Some("game-dev"), Some(1), Some(3))
    );
    assert_eq!(w["customization"]["name"], "Pixel Quest");

    // A twin is taken as it is: no writes to it, its name kept.
    for name in ["a starter that's in the library already (its twin)", "a twin renamed in the library keeps its name"] {
        let p = case(&all, name);
        assert_eq!(p["templates"], serde_json::json!([]), "{name}");
        assert_eq!(p["wadspaces"][0]["template"], "dev", "{name}");
    }
    assert_eq!(
        case(&all, "a twin renamed in the library keeps its name")["docs"]["templates"][0]["name"],
        "My Dev Box"
    );

    assert_eq!(ids(&case(&all, "a starter whose id another template has")["templates"]), ["writing-2"]);
    let chained = case(&all, "your own chained template");
    assert_eq!(
        (chained["templates"].clone(), chained["wadspaces"][0]["template"].clone()),
        (serde_json::json!([]), "dev-art".into())
    );
    let on_yours = case(&all, "a new template on one of yours");
    assert_eq!(on_yours["templates"][0]["parent"], "dev");

    let yours = case(&all, "your own Extensions");
    assert_eq!(
        (yours["extensions"].clone(), yours["wadspaces"][0]["extensions"].clone()),
        (serde_json::json!([]), "dev-plus".into())
    );
    let fresh = case(&all, "new Extensions");
    assert_eq!(ids(&fresh["extensions"]), ["study-tools"]);
    assert_eq!(fresh["extensions"][0]["browserMode"], "focus");
    assert_eq!(fresh["extensions"][0]["templateId"], "dev");
    let twin = case(&all, "new Extensions just like ones in the library");
    assert_eq!(
        (twin["extensions"].clone(), twin["wadspaces"][0]["extensions"].clone()),
        (serde_json::json!([]), "dev-plus".into())
    );
    let both = case(&all, "new Extensions on a new template");
    assert_eq!((ids(&both["templates"]), ids(&both["extensions"])), (vec!["py-lab"], vec!["python"]));

    let cosmic = &case(&all, "GitHub projects and a picked wallpaper")["wadspaces"][0]["customization"];
    assert_eq!(cosmic["projects"].as_array().unwrap().len(), 2);
    assert_eq!((cosmic["wallpaper"]["type"].as_str(), cosmic["titleBars"].as_bool()), (Some("color"), Some(true)));

    // The others: nothing written, nothing deleted; this one after the last.
    let others = case(&all, "other wadspaces stay as they are (orders with gaps, one deleted)");
    assert_eq!(ids(&others["wadspaces"]), ["stack-mgv1x2y3-1"]);
    assert_eq!(others["wadspaces"][0]["order"], 6);
    assert_eq!(others["deleted"], serde_json::json!([]));
    let kept = others["docs"]["wadspaces"].as_array().unwrap().iter().find(|w| w["id"] == "first").unwrap();
    assert_eq!(kept["extra"], "kept");

    assert_eq!(ids(&case(&all, "a deleted twin isn't used")["templates"]), ["dev-2"]);
}

/// After a deliberate change: `UPDATE_GOLDENS=1 cargo test -p wad-board
/// phone -- --ignored` writes phone.json from phone-inputs.json.
#[test]
#[ignore = "rewrites fixtures/board/phone.json; run on purpose"]
fn rewrite_phone_goldens() {
    if std::env::var("UPDATE_GOLDENS").as_deref() != Ok("1") {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/board/phone.json");
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b" "));
    serde::Serialize::serialize(&made(), &mut ser).unwrap();
    out.push(b'\n');
    std::fs::write(path, out).unwrap();
}
