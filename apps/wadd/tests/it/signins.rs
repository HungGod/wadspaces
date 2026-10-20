//! Saved sign-ins: made, renamed, mounted into the workspaces that use
//! them, and kept while one runs or uses them.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, ws};
use wad_proto::ErrorCode;
use wad_proto::v1::{Display, SignInPatch, SignInRequest, Workspace, WorkspaceSignIn};
use wadd::events::Bus;
use wadd::registry::{Registry, Settings};
use wadd::signins::SignIns;

struct Setup {
    reg: Arc<Registry>,
    fake: Fake,
    signins: Arc<SignIns>,
    dir: tempfile::TempDir,
}

async fn setup(list: Vec<Workspace>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let bus = Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::User,
        started_at: 0,
        screen: None,
    });
    let reg = Registry::new(
        Arc::new(fake.clone()),
        bus.clone(),
        Settings {
            ready_timeout: Duration::from_secs(30),
            projects_dir: "/p".into(),
            signins_dir: dir.path().join("signins"),
            state_dir: dir.path().join("state"),
            rootless_uid: None,
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_secs(1),
        },
    );
    let signins = SignIns::new(
        &dir.path().join("state"),
        dir.path().join("signins"),
        nix::unistd::getuid().as_raw(),
        reg.clone(),
        bus,
    );
    let weak = Arc::downgrade(&signins);
    reg.on_prepare(move |w| weak.upgrade().map_or(Ok(()), |s| s.prepare(w)));
    reg.load(list).await.unwrap();
    Setup { reg, fake, signins, dir }
}

fn web(name: &str) -> SignInRequest {
    SignInRequest { name: name.into(), app: "web".into(), paths: vec![] }
}

fn using(id: &str, signins: &[(&str, &str, bool)]) -> Workspace {
    Workspace {
        signins: signins
            .iter()
            .map(|(s, app, web)| WorkspaceSignIn {
                id: (*s).into(),
                app: (*app).into(),
                web: *web,
                name: String::new(),
                kind: String::new(),
                slot: String::new(),
            })
            .collect(),
        ..ws(id, "ghcr.io/o/x:1", Display::Host, None)
    }
}

#[tokio::test]
async fn made_renamed_and_listed() {
    let s = setup(vec![]).await;
    let gh = s.signins.create(web("GitHub")).unwrap();
    assert!(gh.id.starts_with("github-"), "{}", gh.id);
    assert!(s.dir.path().join("signins").join(&gh.id).is_dir());
    let code = s.signins.create(SignInRequest { name: "VS Code".into(), app: "vscode".into(), paths: vec![] }).unwrap();
    assert_eq!(code.paths, wad_core::signins::paths_for("vscode").unwrap());
    // An app the catalog doesn't know names its files.
    let err = s.signins.create(SignInRequest { name: "Tool".into(), app: "tool".into(), paths: vec![] }).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
    let tool = s
        .signins
        .create(SignInRequest { name: "Tool".into(), app: "tool".into(), paths: vec!["~/.config/tool".into()] })
        .unwrap();
    assert_eq!(tool.paths, [".config/tool"]);
    assert!(
        s.signins.create(SignInRequest { name: "Bad".into(), app: "tool".into(), paths: vec!["../x".into()] }).is_err()
    );

    let renamed = s.signins.patch(&gh.id, SignInPatch { name: Some("Work GitHub".into()), paths: None }).unwrap();
    assert_eq!(renamed.name, "Work GitHub");
    assert!(s.signins.patch(&gh.id, SignInPatch { name: None, paths: Some(vec![".x".into()]) }).is_err());
    let names: Vec<String> = s.signins.list().into_iter().map(|x| x.name).collect();
    assert_eq!(names, ["Tool", "VS Code", "Work GitHub"]);
}

#[tokio::test]
async fn mounted_where_used_and_kept_while_in_use() {
    let s = setup(vec![]).await;
    let gh = s.signins.create(web("GitHub")).unwrap();
    let a = using("a", &[(&gh.id, "github", true), (&gh.id, "tailscale", true), ("vscode-zz", "vscode", false)]);
    s.reg.load(vec![a.clone(), ws("b", "ghcr.io/o/b:1", Display::Host, None)]).await.unwrap();

    let unit = s.reg.unit_text(&a);
    let host = s.dir.path().join("signins");
    assert!(
        unit.contains(&format!("Volume={}/{}:/run/wadspaces-signins/{}:rw,z", host.display(), gh.id, gh.id)),
        "{unit}"
    );
    assert!(unit.contains("/run/wadspaces-extra:ro,z"), "{unit}");

    s.fake.with(|m| {
        m.images.insert(a.image.clone());
    });
    s.reg.start("a").unwrap();
    s.reg.settled("a").await;
    // The one it didn't have yet was made, named after its app.
    let made = s.signins.get("vscode-zz").unwrap();
    assert_eq!((made.name.as_str(), made.app.as_str()), ("vscode", "vscode"));
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(s.dir.path().join("state/extra/a/signins.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["signins"],
        serde_json::json!([
            {"id": gh.id, "name": "GitHub", "web": true, "apps": ["github", "tailscale"],
             "dir": format!("/run/wadspaces-signins/{}", gh.id), "paths": []},
            {"id": "vscode-zz", "name": "vscode", "web": false, "apps": ["vscode"],
             "dir": "/run/wadspaces-signins/vscode-zz", "paths": wad_core::signins::paths_for("vscode").unwrap()},
        ])
    );

    let now = s.signins.get(&gh.id).unwrap();
    assert_eq!((now.used_by.as_slice(), now.in_use.as_slice()), (&["a".to_string()][..], &["a".to_string()][..]));
    // Not signed out or deleted from under a running wadspace.
    std::fs::write(host.join(&gh.id).join("cookies.sqlite"), "x").unwrap();
    assert_eq!(s.signins.sign_out(&gh.id).unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(s.signins.delete(&gh.id).unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(s.signins.get(&gh.id).unwrap().size, 1);

    s.reg.stop("a").await.unwrap();
    let out = s.signins.sign_out(&gh.id).unwrap();
    assert_eq!(out.size, 0);
    assert!(host.join(&gh.id).is_dir());
    // Still used by a's design.
    assert_eq!(s.signins.delete(&gh.id).unwrap_err().code, ErrorCode::Conflict);
    s.reg.load(vec![ws("a", "ghcr.io/o/x:1", Display::Host, None)]).await.unwrap();
    s.signins.delete(&gh.id).unwrap();
    assert!(!host.join(&gh.id).exists());
    assert_eq!(s.signins.get(&gh.id).unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_rebuild_takes_the_designs_signins() {
    let old = using("a", &[("github-aa", "github", true)]);
    let fresh = serde_json::json!({"id": "a", "name": "A", "image": "localhost/wadspaces-a:latest", "display": "host"});
    let (list, _) = wadd::builds::merged(std::slice::from_ref(&old), &fresh).unwrap();
    assert!(list[0].signins.is_empty());
    let mut with = fresh.clone();
    with["signins"] = serde_json::json!([{"id": "github-bb", "app": "github", "web": true}]);
    let (list, _) = wadd::builds::merged(&[old], &with).unwrap();
    assert_eq!(list[0].signins[0].id, "github-bb");
}

#[tokio::test]
async fn the_builders_kinds_unique_names_and_setups_that_use_one() {
    let s = setup(vec![]).await;
    let steam = s.signins.create(SignInRequest { name: "Steam".into(), app: "steam".into(), paths: vec![] }).unwrap();
    assert_eq!(steam.paths, [".steam/", ".local/share/Steam/config/", ".local/share/Steam/userdata/"]);
    let claude = s
        .signins
        .create(SignInRequest { name: "Me-VSCode-ClaudeCode".into(), app: "vscode-claude-code".into(), paths: vec![] })
        .unwrap();
    assert_eq!(claude.paths, [".claude/", ".claude.json"]);
    // Names are what sign-ins are picked by: one each, letter case aside.
    let dup = s.signins.create(SignInRequest { name: "steam".into(), app: "steam".into(), paths: vec![] }).unwrap_err();
    assert_eq!(dup.code, ErrorCode::Conflict);
    let rename = s.signins.patch(&claude.id, SignInPatch { name: Some("STEAM".into()), paths: None }).unwrap_err();
    assert_eq!(rename.code, ErrorCode::Conflict);
    s.signins.patch(&claude.id, SignInPatch { name: Some("Claude".into()), paths: None }).unwrap();
    // A saved WadBrowser setup that uses one counts as a user (and keeps it).
    let google = s.signins.create(web("Google")).unwrap();
    let lib = s.dir.path().join("state/library/browsers");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("mail.json"),
        serde_json::json!({"id": "mail", "name": "Mail", "secrets": {"web:gmail": {"id": google.id, "name": "Google", "kind": "web"}}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(s.signins.get(&google.id).unwrap().used_by, ["browser-mail"]);
    assert_eq!(s.signins.delete(&google.id).unwrap_err().code, ErrorCode::Conflict);
    // A slot's kind makes one a wadspace names on a machine that hasn't it.
    let ws = Workspace {
        signins: vec![WorkspaceSignIn {
            id: "claude-abc123".into(),
            app: "vscode".into(),
            web: false,
            name: "Claude Code".into(),
            kind: "vscode-claude-code".into(),
            slot: "ext:anthropic.claude-code".into(),
        }],
        ..ws("w", "ghcr.io/o/x:1", Display::Host, None)
    };
    s.signins.prepare(&ws).unwrap();
    let made = s.signins.get("claude-abc123").unwrap();
    assert_eq!(
        (made.app.as_str(), made.paths.clone()),
        ("vscode-claude-code", vec![".claude/".to_string(), ".claude.json".to_string()])
    );
    // An app and its extension each keep their own (a slot each).
    let two = serde_json::json!([{"id": "w", "name": "W", "image": "i", "display": "host", "signins": [
        {"id": "a1", "app": "vscode", "kind": "vscode", "slot": "app:vscode"},
        {"id": "a2", "app": "vscode", "kind": "vscode-claude-code", "slot": "ext:anthropic.claude-code"}]}]);
    assert_eq!(wad_store::workspaces::check_workspaces(two.as_array().unwrap().clone()).unwrap()[0].signins.len(), 2);
    let same = serde_json::json!([{"id": "w", "name": "W", "image": "i", "display": "host", "signins": [
        {"id": "a1", "app": "vscode", "slot": "ext:anthropic.claude-code"}, {"id": "a2", "app": "vscode", "slot": "ext:anthropic.claude-code"}]}]);
    assert!(wad_store::workspaces::check_workspaces(same.as_array().unwrap().clone()).is_err());
}
