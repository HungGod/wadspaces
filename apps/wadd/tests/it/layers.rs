//! The Builder's layers on a machine: templates' (one on another) and an
//! Extensions layer's images (what each builds on built first when it's
//! missing or out of date), their
//! labels and listing, deleting one, and wadspaces installed on them with
//! their wallpaper; and what any build does: one at a time, cancelling, a
//! missing base or too little disk, and web apps' icons made first. A fake
//! podman builds; the real registry installs.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::{Fake, ws};
use serde_json::{Value, json};
use wad_proto::ErrorCode;
use wad_proto::v1::{BuildLog, BuildStatus, Display, LayerBuildRequest, WallpaperUpload};
use wadd::builds::{Builds, FROM_LABEL, SHA_LABEL};
use wadd::events::Bus;
use wadd::icons::Icons;
use wadd::images::{self, Install};
use wadd::registry::{Registry, Settings};

const BASE: &str = "localhost/wadspaces-base:trixie";
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

struct Env {
    _d: tempfile::TempDir,
    state: std::path::PathBuf,
    fake: Fake,
    reg: Arc<Registry>,
    builds: Arc<Builds>,
    bus: Bus,
}

async fn env() -> Env {
    env_with(None).await
}

async fn env_with(icons: Option<Arc<Icons>>) -> Env {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let fake = Fake::default();
    fake.with(|m| {
        m.images.insert(BASE.into());
        m.image_labels.insert(BASE.into(), json!({"io.wadspaces.projects": "1"}).as_object().unwrap().clone());
        m.free_gb = 100.0;
    });
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
            ready_timeout: Duration::from_secs(10),
            projects_dir: "/p".into(),
            signins_dir: d.path().join("signins"),
            state_dir: state.clone(),
            rootless_uid: Some(1000),
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    reg.load(vec![]).await.unwrap();
    let builds = match icons {
        Some(i) => Builds::with_icons(Arc::new(fake.clone()), bus.clone(), &state, 8, i),
        None => Builds::new(Arc::new(fake.clone()), bus.clone(), &state, 8),
    };
    Env { _d: d, state, fake, reg, builds, bus }
}

fn dev() -> Value {
    json!({"id": "dev", "owner": "me", "name": "Dev", "description": "", "apps": ["wadbrowser", "vscode"], "createdAt": "t", "updatedAt": "t"})
}

fn plus() -> Value {
    json!({"id": "dev-plus", "owner": "me", "name": "Dev plus", "description": "", "templateId": "dev", "browserMode": "focus",
           "webapps": [{"id": "github", "name": "GitHub", "url": "https://github.com"}],
           "extensions": ["anthropic.claude-code"], "tools": ["go"], "autostart": [], "createdAt": "t", "updatedAt": "t"})
}

fn custom(t: Value, c: Value) -> LayerBuildRequest {
    LayerBuildRequest {
        layer: format!("custom:{}", c["id"].as_str().unwrap()),
        template: t,
        parents: vec![],
        extensions: Some(c),
    }
}

/// A build's log once it's finished (in 30 s at most: a loaded CI machine is slow).
async fn wait(e: &Env, id: &str) -> BuildLog {
    for _ in 0..3000 {
        let l = e.builds.log(id, 0).unwrap();
        if l.build.status.finished() {
            return l;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("build stuck: {:?}", e.builds.log(id, 0).unwrap());
}

fn label(e: &Env, image: &str, key: &str) -> Option<String> {
    let m = e.fake.0.lock().unwrap();
    m.image_labels.get(image).and_then(|l| l.get(key)).and_then(Value::as_str).map(String::from)
}

const T_REF: &str = "localhost/wadspaces-template-dev:latest";
const C_REF: &str = "localhost/wadspaces-custom-dev-plus:latest";

#[tokio::test]
async fn an_extensions_layer_builds_its_template_first_then_itself() {
    let e = env().await;
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap();
    assert_eq!((b.layer.as_deref(), b.image.as_str(), b.base_image.as_str()), (Some("custom:dev-plus"), C_REF, BASE));
    let l = wait(&e, &b.id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{:?}", l.lines);
    assert!(l.lines.iter().any(|x| x == "» Building the Dev template first"), "{:?}", l.lines);
    let built: Vec<(String, String)> =
        e.fake.0.lock().unwrap().builds.iter().map(|(t, b, _)| (t.clone(), b.clone())).collect();
    assert_eq!(built, [(T_REF.into(), BASE.into()), (C_REF.into(), T_REF.into())]);
    // Labelled with their folders' shas and what they were built on.
    let t_layer = wad_core::layers::template_layer(&dev(), &serde_json::Value::Null);
    assert_eq!(label(&e, T_REF, SHA_LABEL).as_deref(), t_layer["sha"].as_str());
    let template_id = e.fake.0.lock().unwrap().image_ids[T_REF].clone();
    assert_eq!(label(&e, C_REF, FROM_LABEL), Some(template_id));
    // They keep the base's: projects can be mounted into them.
    assert_eq!(label(&e, C_REF, "io.wadspaces.projects").as_deref(), Some("1"));
    // The customization's folder has its web app's icon (made here first).
    let tar = e.fake.0.lock().unwrap().builds[1].2.clone();
    assert!(String::from_utf8_lossy(&tar).contains("wadspaces-webapp-github.png"));
    assert!(l.build.progress >= 1.0);
}

#[tokio::test]
async fn a_built_template_isnt_built_again_until_it_changes() {
    let e = env().await;
    let b = e
        .builds
        .create_layer(&LayerBuildRequest {
            layer: "template:dev".into(),
            template: dev(),
            parents: vec![],
            extensions: None,
        })
        .unwrap();
    wait(&e, &b.id).await;
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap();
    let l = wait(&e, &b.id).await;
    assert!(l.lines.iter().any(|x| x == "» the Dev template is built already"), "{:?}", l.lines);
    assert_eq!(e.fake.0.lock().unwrap().builds.len(), 2);
    // The template changes: built again, under its customization.
    let mut changed = dev();
    changed["apps"] = json!(["wadbrowser", "vscode", "tiled"]);
    let b = e.builds.create_layer(&custom(changed, plus())).unwrap();
    let l = wait(&e, &b.id).await;
    assert!(l.lines.iter().any(|x| x == "» Building the Dev template first"), "{:?}", l.lines);
    assert_eq!(e.fake.0.lock().unwrap().builds.len(), 4);
}

#[tokio::test]
async fn images_list_with_their_shas_and_a_rebuilt_template_shows() {
    let e = env().await;
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap();
    wait(&e, &b.id).await;
    let rows = images::list(&e.fake).await.unwrap();
    let layers: Vec<&str> = rows.iter().map(|r| r.layer.as_str()).collect();
    // The base last: a template built on an older one has an update.
    assert_eq!(layers, ["custom:dev-plus", "template:dev", "base"]);
    let (c, t) = (&rows[0], &rows[1]);
    assert_eq!(rows[1].from, rows[2].id);
    assert_eq!(c.image_ref, C_REF);
    assert_eq!(c.dockerfile_sha, wad_core::layers::extensions_layer(&plus(), &dev())["sha"].as_str().unwrap());
    // Built on the template's image as it is: unchanged.
    assert_eq!(c.from, t.id);
    // The template rebuilt: the customization's `from` is an older image now.
    let b = e.builds.create_layer(&LayerBuildRequest {
        layer: "template:dev".into(),
        template: json!({"id": "dev", "name": "Dev", "apps": ["wadbrowser", "vscode", "lmms"]}),
        parents: vec![],
        extensions: None,
    });
    wait(&e, &b.unwrap().id).await;
    let rows = images::list(&e.fake).await.unwrap();
    assert_ne!(rows[0].from, rows[1].id, "a customization on an older template build");
}

#[tokio::test]
async fn layer_requests_are_checked() {
    let e = env().await;
    let bad = |r: LayerBuildRequest| e.builds.create_layer(&r).unwrap_err().code;
    assert_eq!(
        bad(LayerBuildRequest { layer: "template:other".into(), template: dev(), parents: vec![], extensions: None }),
        ErrorCode::BadRequest
    );
    assert_eq!(
        bad(LayerBuildRequest { layer: "custom:dev-plus".into(), template: dev(), parents: vec![], extensions: None }),
        ErrorCode::BadRequest
    );
    let mut elsewhere = plus();
    elsewhere["templateId"] = "writing".into();
    assert_eq!(bad(custom(dev(), elsewhere)), ErrorCode::BadRequest);
    let mut nothing = dev();
    nothing["apps"] = json!(["brave"]);
    assert_eq!(
        bad(LayerBuildRequest { layer: "template:dev".into(), template: nothing, parents: vec![], extensions: None }),
        ErrorCode::BadRequest
    );
    // No base here: the build says so.
    e.fake.with(|m| {
        m.images.remove(BASE);
    });
    let b = e
        .builds
        .create_layer(&LayerBuildRequest {
            layer: "template:dev".into(),
            template: dev(),
            parents: vec![],
            extensions: None,
        })
        .unwrap();
    let l = wait(&e, &b.id).await;
    assert_eq!(l.build.status, BuildStatus::Error);
    assert!(l.build.error.unwrap().contains("not on this machine"));
}

fn wadspace(id: &str) -> Value {
    json!({"version": 2, "id": id, "owner": "me", "name": "Academy", "description": "",
           "base": {"kind": "customization", "id": "dev-plus"},
           "wallpaper": {"type": "image", "value": "data:image/jpeg;base64,/9j/"},
           "projects": ["p1"],
           "secrets": {"web:github": {"id": "gh-abc123", "name": "GitHub", "kind": "web"},
                       "ext:anthropic.claude-code": {"id": "cc-abc123", "name": "Claude", "kind": "vscode-claude-code"}},
           "createdAt": "t", "updatedAt": "t"})
}

#[tokio::test]
async fn a_wadspace_installs_on_its_layer_with_its_wallpaper_and_sign_ins() {
    let e = env().await;
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"t".to_vec());
    });
    let req = Install {
        wadspace: wadspace("academy"),
        template: dev(),
        extensions: Some(plus()),
        tz: Some("Pacific/Fiji".into()),
    };
    let saved = images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    let ws = saved.workspace;
    assert_eq!((ws.image.as_str(), ws.hotkey, ws.autostart), (C_REF, None, false));
    let env: Vec<(&str, &str)> = ws.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert!(env.contains(&("WALLPAPER", "/run/wadspaces-extra/wallpaper.jpg")), "{env:?}");
    assert!(env.contains(&("TITLE", "Academy")) && env.contains(&("TZ", "Pacific/Fiji")), "{env:?}");
    assert_eq!(ws.secrets, ["github_token"]);
    assert_eq!(ws.volumes, ["wad-academy-config:/config:z"]);
    let s: Vec<(&str, &str, &str, bool)> =
        ws.signins.iter().map(|s| (s.id.as_str(), s.app.as_str(), s.slot.as_str(), s.web)).collect();
    assert_eq!(
        s,
        [("gh-abc123", "github", "web:github", true), ("cc-abc123", "vscode", "ext:anthropic.claude-code", false)]
    );
    // Its unit mounts the extra folder, where the wallpaper goes.
    let unit = e.reg.unit_text(&ws);
    assert!(unit.contains("/extra/academy:/run/wadspaces-extra:ro,z"), "{unit}");
    images::put_wallpaper(&e.state, &ws, &WallpaperUpload { ext: "png".into(), data: PNG.into() }).unwrap();
    assert!(e.state.join("extra/academy/wallpaper.png").is_file());
    let refused = images::put_wallpaper(&e.state, &ws, &WallpaperUpload { ext: "jpg".into(), data: PNG.into() });
    assert_eq!(refused.unwrap_err().code, ErrorCode::BadRequest, "a PNG isn't a jpg");
    // Installed again (renamed): the same workspace, updated.
    let mut renamed = wadspace("academy");
    renamed["name"] = "Academy 2".into();
    let req = Install { wadspace: renamed, ..req };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    assert_eq!(
        e.reg.workspaces().iter().filter(|w| w.id == "academy").map(|w| w.name.clone()).collect::<Vec<_>>(),
        ["Academy 2"]
    );
}

#[tokio::test]
async fn a_wadspace_on_layers_it_doesnt_have_is_refused() {
    let e = env().await;
    // A wadspace that isn't on what's sent is refused.
    let wrong = Install { wadspace: wadspace("academy"), template: dev(), extensions: None, tz: None };
    assert_eq!(images::install(&e.reg, &e.fake, "academy", &wrong).await.unwrap_err().code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn wadspaces_from_before_the_layers_go_with_their_images() {
    let e = env().await;
    let req = Install { wadspace: wadspace("academy"), template: dev(), extensions: Some(plus()), tz: None };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    let mut old = common::ws(
        "writing",
        "ghcr.io/hunggod/wadspaces-cosmic-bodybuilding:latest",
        wad_proto::v1::Display::Host,
        None,
    );
    old.volumes = vec!["wad-writing-obsidian:/config/.config/obsidian:z".into()];
    let mut list = e.reg.workspaces();
    list.push(old);
    e.reg.set_workspaces(list).await.unwrap();
    e.fake.with(|m| {
        m.images.insert("ghcr.io/hunggod/wadspaces-cosmic-bodybuilding:latest".into());
    });
    e.reg.sweep_old().await;
    let ids: Vec<String> = e.reg.workspaces().into_iter().map(|w| w.id).collect();
    assert_eq!(ids, ["academy"]);
    assert!(!e.fake.0.lock().unwrap().images.contains("ghcr.io/hunggod/wadspaces-cosmic-bodybuilding:latest"));
    // Nothing left to do the next time.
    e.reg.sweep_old().await;
    assert_eq!(e.reg.workspaces().len(), 1);
}

#[tokio::test]
async fn an_image_isnt_deleted_while_a_wadspace_runs_on_it() {
    let e = env().await;
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap();
    wait(&e, &b.id).await;
    let req = Install { wadspace: wadspace("academy"), template: dev(), extensions: Some(plus()), tz: None };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    let refused = images::delete(&e.reg, &e.builds, &e.fake, "custom:dev-plus").await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(refused.message.contains("Academy"), "{}", refused.message);
    // The template's image has no workspace of its own: it can go.
    images::delete(&e.reg, &e.builds, &e.fake, "template:dev").await.unwrap();
    e.reg.remove_workspace("academy").await.unwrap();
    images::delete(&e.reg, &e.builds, &e.fake, "custom:dev-plus").await.unwrap();
    assert!(e.fake.calls().contains(&format!("rmi {C_REF}")));
    assert_eq!(
        images::delete(&e.reg, &e.builds, &e.fake, "custom:dev-plus").await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(images::delete(&e.reg, &e.builds, &e.fake, "../x").await.unwrap_err().code, ErrorCode::BadRequest);
}

fn template(t: Value) -> LayerBuildRequest {
    LayerBuildRequest {
        layer: format!("template:{}", t["id"].as_str().unwrap()),
        template: t,
        parents: vec![],
        extensions: None,
    }
}

#[tokio::test]
async fn a_missing_base_says_how_to_get_it() {
    let e = env().await;
    e.fake.with(|m| m.images.clear());
    let l = wait(&e, &e.builds.create_layer(&template(dev())).unwrap().id).await;
    assert_eq!(l.build.status, BuildStatus::Error);
    assert_eq!(
        l.build.error.as_deref(),
        Some(
            "localhost/wadspaces-base:trixie is not on this machine — update the drive with host/build.sh update --bases"
        )
    );
    assert!(e.fake.0.lock().unwrap().builds.is_empty());
}

#[tokio::test]
async fn short_on_disk_and_failed_builds() {
    let e = env().await;
    e.fake.with(|m| m.free_gb = 3.0);
    let l = wait(&e, &e.builds.create_layer(&template(dev())).unwrap().id).await;
    assert_eq!(l.build.error.as_deref(), Some("only 3.0 GB free; building needs at least 8 GB"));
    e.fake.with(|m| {
        m.free_gb = 100.0;
        m.build_fails = Some("RUN apt-get install: exit status 100".into());
    });
    let l = wait(&e, &e.builds.create_layer(&template(dev())).unwrap().id).await;
    assert_eq!(
        (l.build.status, l.build.error.as_deref()),
        (BuildStatus::Error, Some("RUN apt-get install: exit status 100"))
    );
    assert!(l.build.progress < 1.0); // STEP n/m counts what came before n
}

#[tokio::test]
async fn three_builds_at_once_and_cancelling() {
    let e = env().await;
    e.fake.with(|m| m.build_slow = true);
    let t = |id: &str| {
        let mut x = dev();
        x["id"] = id.into();
        x["apps"] = json!(["obsidian"]);
        template(x)
    };
    let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|id| e.builds.create_layer(&t(id)).unwrap().id).collect();
    let err = e.builds.create_layer(&t("a")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict); // the same one: once
    tokio::time::sleep(Duration::from_millis(150)).await;
    let status = |id: &str| e.builds.log(id, 0).unwrap().build.status;
    // Three side by side; the fourth waits its turn.
    assert_eq!(
        ids.iter().map(|i| status(i)).collect::<Vec<_>>(),
        [BuildStatus::Building, BuildStatus::Building, BuildStatus::Building, BuildStatus::Queued]
    );
    assert_eq!(e.builds.cancel(&ids[0]).unwrap().status, BuildStatus::Cancelled);
    assert_eq!(e.builds.log(&ids[0], 0).unwrap().lines.last().map(String::as_str), Some("✗ cancelled"));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(status(&ids[3]), BuildStatus::Building); // its turn now
    for id in &ids[1..] {
        e.builds.cancel(id).unwrap();
    }
    assert_eq!(e.builds.list().len(), 4);
    assert_eq!(e.builds.log("nope", 0).unwrap_err().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn two_builds_needing_one_template_build_it_once() {
    let e = env().await;
    // The template, and an Extensions layer on it, asked for together.
    let a = e.builds.create_layer(&template(dev())).unwrap().id;
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap().id;
    assert_eq!(wait(&e, &a).await.build.status, BuildStatus::Done);
    let l = wait(&e, &b).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    let built: Vec<String> = e.fake.0.lock().unwrap().builds.iter().map(|(t, _, _)| t.clone()).collect();
    assert_eq!(built, [T_REF, C_REF], "Dev built once");
}

const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[tokio::test]
async fn web_apps_get_their_icons_before_the_build() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let site = MockServer::start().await;
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><head></head></html>"))
        .mount(&site)
        .await;
    let favicon = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, DOT).unwrap();
    Mock::given(path("/favicon.ico"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(favicon))
        .mount(&site)
        .await;

    let e = env_with(Some(Icons::for_tests())).await;
    let mut c = plus();
    c["webapps"] = json!([
        {"id": "site", "name": "Site", "url": site.uri()},
        // Nobody answers there: a card with its name.
        {"id": "gone", "name": "Gone", "url": "http://127.0.0.1:9/"},
    ]);
    let l = wait(&e, &e.builds.create_layer(&custom(dev(), c)).unwrap().id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    assert!(
        l.lines.iter().any(|x| x == "» icons for 2 web apps: 1 from their sites, 0 chosen, 1 with their names"),
        "{:?}",
        l.lines
    );
    let builds = e.fake.0.lock().unwrap().builds.clone();
    let (_, _, context) = builds.last().unwrap().clone();
    let files = common::tar_names(&context);
    for id in ["site", "gone"] {
        let want = format!("root/usr/share/icons/hicolor/512x512/apps/wadspaces-webapp-{id}.png");
        assert!(files.contains(&want), "{want} not in {files:?}");
    }
}

#[tokio::test]
async fn web_apps_show_in_the_client_with_their_sites_own_icons() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let site = MockServer::start().await;
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><head></head></html>"))
        .mount(&site)
        .await;
    let favicon = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, DOT).unwrap();
    Mock::given(path("/favicon.ico"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(favicon))
        .expect(1) // and kept: asked once
        .mount(&site)
        .await;
    let d = tempfile::tempdir().unwrap();
    let icons = Icons::for_tests_kept(d.path());
    for _ in 0..2 {
        let png = icons.favicon(&site.uri()).await.expect("the site's icon");
        // A PNG's size is in its header.
        let side = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        assert_eq!((&png[1..4], side(16), side(20)), (&b"PNG"[..], 128, 128));
    }
    // Nobody answers there: nothing to show (the client shows its own).
    assert!(icons.favicon("http://127.0.0.1:9/").await.is_none());
}

#[tokio::test]
async fn the_disk_report_counts_each_layers_own_share_and_each_home() {
    let e = env().await;
    let l = wait(&e, &e.builds.create_layer(&custom(dev(), plus())).unwrap().id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    e.fake.with(|m| {
        m.image_sizes.insert(BASE.into(), 3_000);
        m.image_sizes.insert(T_REF.into(), 5_000);
        m.image_sizes.insert(C_REF.into(), 5_600);
        m.volume_bytes.insert("wad-academy-config".into(), 700);
        m.volume_bytes.insert("someone-elses".into(), 9);
    });
    let req = Install { wadspace: wadspace("academy"), template: dev(), extensions: Some(plus()), tz: None };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    std::fs::create_dir_all(e.state.join("copies/c1/files/a")).unwrap();
    std::fs::write(e.state.join("copies/c1/files/a/f"), vec![0u8; 42]).unwrap();

    // Its desktop, kept for the switcher: the template's apps, then the web apps.
    let desktop: Vec<String> = e.reg.workspace("academy").unwrap().desktop.iter().map(|d| d.label.clone()).collect();
    assert_eq!(desktop, ["WadBrowser", "VS Code", "GitHub"]);

    let backend: Arc<dyn wadd::backend::Backend> = Arc::new(e.fake.clone());
    let r = wadd::storage::Storage::new(&e.state, false, backend, e.reg.clone(), e.bus.clone()).report(true).await;
    assert_eq!(r.base.as_ref().map(|b| b.bytes), Some(3_000));
    let own: Vec<(String, u64)> = r.layers.iter().map(|l| (l.layer.clone(), l.own_bytes)).collect();
    // Each layer: what it adds to the one under it.
    assert_eq!(own, vec![("custom:dev-plus".into(), 600), ("template:dev".into(), 2_000)]);
    assert_eq!(r.homes.iter().map(|h| (h.workspace_id.as_str(), h.bytes)).collect::<Vec<_>>(), vec![("academy", 700)]);
    assert_eq!(r.copies.iter().map(|c| (c.id.as_str(), c.bytes)).collect::<Vec<_>>(), vec![("c1", 42)]);
    // And the base in the images list, so a template on an older one shows an update.
    let rows = images::list(&e.fake).await.unwrap();
    assert!(rows.iter().any(|r| r.layer == "base" && r.image_ref == BASE), "{rows:?}");
}

#[tokio::test]
async fn storage_hands_out_its_last_count_and_counts_again_behind() {
    let e = env().await;
    let backend: Arc<dyn wadd::backend::Backend> = Arc::new(e.fake.clone());
    let s = wadd::storage::Storage::new(&e.state, false, backend, e.reg.clone(), e.bus.clone());
    let (_, mut rx) = e.bus.subscribe();
    let counted = async |rx: &mut tokio::sync::broadcast::Receiver<wad_proto::v1::Event>| loop {
        if let wad_proto::v1::Event::Storage { measured_at } = rx.recv().await.unwrap() {
            return measured_at;
        }
    };
    // None yet: counted now.
    let first = s.report(false).await;
    assert!(!first.measuring);
    assert_eq!(counted(&mut rx).await, first.measured_at);
    // Something went: the last count at once, a new one on its way.
    s.forget();
    let old = s.report(false).await;
    assert_eq!((old.measured_at, old.measuring), (first.measured_at, true));
    let next = tokio::time::timeout(Duration::from_secs(5), counted(&mut rx)).await.unwrap();
    assert!(next > first.measured_at);
    let now = s.report(false).await;
    assert_eq!((now.measured_at, now.measuring), (next, false));
}

#[tokio::test]
async fn whats_picked_comes_off_the_disk_and_nothing_else() {
    use wad_proto::v1::StorageDelete;
    let e = env().await;
    let l = wait(&e, &e.builds.create_layer(&custom(dev(), plus())).unwrap().id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    let req = Install { wadspace: wadspace("academy"), template: dev(), extensions: Some(plus()), tz: None };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    e.fake.with(|m| {
        m.volume_bytes.insert("wad-academy-config".into(), 700);
        m.image_sizes.insert(T_REF.into(), 5_000);
        m.image_sizes.insert(C_REF.into(), 5_600);
        m.image_sizes.insert(BASE.into(), 3_000);
    });
    let copies = wadd::copies::Copies::new(
        &e.state,
        vec![],
        1000,
        Arc::new(|| ("local".into(), "Surface".into())),
        Arc::new(wadd::drives::Drives::new(1000, Arc::new(wadd::drives::System), false)),
        e.reg.clone(),
    );
    let pick = |layers: &[&str], wadspaces: &[&str]| StorageDelete {
        images: vec![],
        volumes: vec![],
        update_copy: false,
        layers: layers.iter().map(|x| x.to_string()).collect(),
        wadspaces: wadspaces.iter().map(|x| x.to_string()).collect(),
    };
    let images_here = || e.fake.0.lock().unwrap().images.clone();

    // The template alone: its name goes; its files stay, the Extensions are built on it.
    let f = images::delete_selected(&e.reg, &e.builds, &copies, &e.fake, &e.state, &pick(&["template:dev"], &[]))
        .await
        .unwrap();
    assert_eq!((f.removed.len(), f.kept.clone(), f.bytes), (0, vec!["template:dev".to_string()], 0));
    assert!(!images_here().contains(T_REF) && images_here().contains(C_REF));
    assert!(e.reg.workspaces().iter().any(|w| w.id == "academy"));

    // The Extensions, with a wadspace on them that isn't picked: refused.
    let err = images::delete_selected(&e.reg, &e.builds, &copies, &e.fake, &e.state, &pick(&["custom:dev-plus"], &[]))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("pick it too"), "{}", err.message);

    // With it: the wadspace off the machine, its files gone; the image gone.
    let f = images::delete_selected(
        &e.reg,
        &e.builds,
        &copies,
        &e.fake,
        &e.state,
        &pick(&["custom:dev-plus"], &["academy"]),
    )
    .await
    .unwrap();
    assert_eq!(f.removed, vec!["custom:dev-plus".to_string()]);
    assert_eq!(f.uninstalled.len(), 1);
    assert_eq!(f.bytes, 700 + 600);
    assert!(e.reg.workspaces().iter().all(|w| w.id != "academy"));
    assert!(e.fake.0.lock().unwrap().calls.contains(&"volume rm wad-academy-config".to_string()));

    // The template's old files are needed by nothing now: an old build, pruned.
    let left = images::leftovers(&e.fake).await.unwrap();
    assert_eq!(left.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["template:dev"]);
    let f = images::prune(&e.fake).await.unwrap();
    assert_eq!((f.removed.len(), f.bytes), (1, 2_000));
    assert!(images_here().iter().all(|i| i == BASE));
}

#[tokio::test]
async fn everything_else_on_the_disk_is_named_and_what_can_go_goes() {
    use wad_proto::v1::StorageDelete;
    let e = env().await;
    let l = wait(&e, &e.builds.create_layer(&custom(dev(), plus())).unwrap().id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    let req = Install { wadspace: wadspace("academy"), template: dev(), extensions: Some(plus()), tz: None };
    images::install(&e.reg, &e.fake, "academy", &req).await.unwrap();
    e.fake.with(|m| {
        for (n, b) in [(BASE, 3_000), (T_REF, 5_000), (C_REF, 5_600)] {
            m.image_sizes.insert(n.into(), b);
        }
        // An image from before layers, and a step a build left.
        m.images.insert("localhost/wadspaces-kale-b:latest".into());
        m.image_unique.insert("localhost/wadspaces-kale-b:latest".into(), 900);
        m.images.insert("<none>@step".into());
        m.image_unique.insert("<none>@step".into(), 40);
        // All the layers: 3000 + 2000 + 600 ours, 940 theirs, 460 shared by older images.
        m.images_size = Some(7_000);
        m.volume_bytes.insert("wad-academy-config".into(), 700);
        m.volume_bytes.insert("wad-iq-dev-config".into(), 1_200);
    });
    std::fs::create_dir_all(e.state.join("incoming/blobs")).unwrap();
    std::fs::write(e.state.join("incoming/blobs/a"), vec![0u8; 8192]).unwrap();

    let backend: Arc<dyn wadd::backend::Backend> = Arc::new(e.fake.clone());
    let r = wadd::storage::Storage::new(&e.state, false, backend, e.reg.clone(), e.bus.clone()).report(true).await;
    let part = |k: &str| r.other.iter().find(|p| p.key == k).unwrap_or_else(|| panic!("no {k} in {:?}", r.other));
    let images = part("images");
    assert_eq!(images.bytes, 940);
    let named: Vec<(&str, bool)> = images.items.iter().map(|i| (i.label.as_str(), i.deletable)).collect();
    assert_eq!(named, [("kale-b", true), ("A build step", true)]);
    assert_eq!(part("shared").bytes, 460);
    let vols = part("volumes");
    assert_eq!(vols.items.iter().map(|i| (i.label.as_str(), i.bytes)).collect::<Vec<_>>(), [("iq-dev's home", 1_200)]);
    let copy = part("update-copy");
    assert!(copy.deletable && copy.bytes >= 8192, "{copy:?}");

    // Gone: the old image, the build step, the home left behind, the update's copy.
    let copies = wadd::copies::Copies::new(
        &e.state,
        vec![],
        1000,
        Arc::new(|| ("local".into(), "Surface".into())),
        Arc::new(wadd::drives::Drives::new(1000, Arc::new(wadd::drives::System), false)),
        e.reg.clone(),
    );
    let pick = StorageDelete {
        images: images.items.iter().map(|i| i.id.clone()).collect(),
        volumes: vec!["wad-iq-dev-config".into(), "wad-academy-config".into()],
        update_copy: true,
        ..Default::default()
    };
    let f = images::delete_selected(&e.reg, &e.builds, &copies, &e.fake, &e.state, &pick).await.unwrap();
    assert_eq!(f.removed.len(), 4, "{f:?}");
    // A wadspace's home here goes with the wadspace, not on its own.
    assert_eq!(f.failed.len(), 1, "{f:?}");
    assert!(!e.state.join("incoming").exists());
    assert!(e.fake.0.lock().unwrap().volume_bytes.contains_key("wad-academy-config"));
}

#[test]
fn a_wadspace_cannot_take_an_apps_id() {
    let current = vec![common::app("terminal", "/usr/bin/foot"), ws("w", "i", Display::Host, None)];
    let fresh =
        json!({"id": "terminal", "name": "T", "image": "localhost/wadspaces-terminal:latest", "display": "host"});
    let e = wadd::builds::merged(&current, &fresh).unwrap_err();
    assert!(e.contains("one of this machine's own apps"), "{e}");
    let other = json!({"id": "w", "name": "W2", "image": "localhost/wadspaces-w:latest", "display": "host"});
    assert!(wadd::builds::merged(&current, &other).unwrap().1);
}

fn gamedev() -> Value {
    json!({"id": "gamedev", "owner": "me", "name": "Game Dev", "description": "", "parent": "dev",
           "apps": ["wadbrowser", "vscode", "tiled", "libresprite"], "createdAt": "t", "updatedAt": "t"})
}

fn music() -> Value {
    json!({"id": "music", "owner": "me", "name": "Game Music", "description": "", "parent": "gamedev",
           "apps": ["wadbrowser", "vscode", "tiled", "libresprite", "lmms"], "createdAt": "t", "updatedAt": "t"})
}

const GD_REF: &str = "localhost/wadspaces-template-gamedev:latest";
const M_REF: &str = "localhost/wadspaces-template-music:latest";

fn top(parents: Vec<Value>, t: Value) -> LayerBuildRequest {
    LayerBuildRequest {
        layer: format!("template:{}", t["id"].as_str().unwrap()),
        template: t,
        parents,
        extensions: None,
    }
}

fn built(e: &Env) -> Vec<(String, String)> {
    e.fake.0.lock().unwrap().builds.iter().map(|(t, b, _)| (t.clone(), b.clone())).collect()
}

#[tokio::test]
async fn templates_build_on_templates_each_on_the_one_below() {
    let e = env().await;
    let b = e.builds.create_layer(&top(vec![dev(), gamedev()], music())).unwrap();
    assert_eq!((b.image.as_str(), b.base_image.as_str()), (M_REF, BASE));
    let l = wait(&e, &b.id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{:?}", l.lines);
    assert_eq!(built(&e), [(T_REF.into(), BASE.into()), (GD_REF.into(), T_REF.into()), (M_REF.into(), GD_REF.into())]);
    // Each installs only what it adds.
    let tar = String::from_utf8_lossy(&e.fake.0.lock().unwrap().builds[2].2).to_string();
    assert!(tar.contains("RUN wadspaces-apt lmms") && !tar.contains("wadspaces-feature"), "{tar}");
    let gd_id = e.fake.0.lock().unwrap().image_ids[GD_REF].clone();
    assert_eq!(label(&e, M_REF, FROM_LABEL), Some(gd_id));

    // Built again: nothing below needs building.
    let b = e.builds.create_layer(&top(vec![dev(), gamedev()], music())).unwrap();
    let l = wait(&e, &b.id).await;
    assert!(l.lines.iter().any(|x| x == "» the Dev template is built already"), "{:?}", l.lines);
    assert!(l.lines.iter().any(|x| x == "» the Game Dev template is built already"), "{:?}", l.lines);
    assert_eq!(built(&e).len(), 4);

    // The middle one rebuilt on its own: the top one builds again on it, even
    // though its build folder is the same.
    let b = e.builds.create_layer(&top(vec![dev()], gamedev())).unwrap();
    wait(&e, &b.id).await;
    assert_eq!(built(&e).len(), 5);
    let b = e.builds.create_layer(&top(vec![dev(), gamedev()], music())).unwrap();
    let l = wait(&e, &b.id).await;
    assert!(l.lines.iter().any(|x| x == "» the Game Dev template is built already"), "{:?}", l.lines);
    assert_eq!(built(&e).len(), 6, "{:?}", l.lines);
    assert_eq!(built(&e)[5], (M_REF.into(), GD_REF.into()));
}

#[tokio::test]
async fn a_chain_that_doesnt_build_is_refused() {
    let e = env().await;
    let bad = |r: LayerBuildRequest| e.builds.create_layer(&r).unwrap_err().message;
    // Its parents not sent, one that drops an app, one in the wrong order.
    assert_eq!(bad(top(vec![], gamedev())), "Game Dev builds on a template that's gone");
    let mut dropped = music();
    dropped["apps"] = json!(["wadbrowser", "tiled", "libresprite", "lmms"]);
    assert!(bad(top(vec![dev(), gamedev()], dropped)).contains("doesn't have VS Code"));
    assert_eq!(bad(top(vec![gamedev(), dev()], music())), "Game Dev builds on a template that's gone");
    // Extensions on a chained template build the whole chain under them.
    let mut on_music = plus();
    on_music["templateId"] = "music".into();
    let r = LayerBuildRequest {
        layer: "custom:dev-plus".into(),
        template: music(),
        parents: vec![dev(), gamedev()],
        extensions: Some(on_music),
    };
    let b = e.builds.create_layer(&r).unwrap();
    wait(&e, &b.id).await;
    assert_eq!(built(&e).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), [T_REF, GD_REF, M_REF, C_REF]);
}

fn stack(id: &str) -> Value {
    json!({"version": 3, "id": id, "owner": "me", "order": 2, "template": "dev", "extensions": "dev-plus",
           "customization": {
               "name": "Academy", "description": "",
               "wallpaper": {"type": "gradient", "value": "linear-gradient(#000,#111)"},
               "projects": [],
               "secrets": {
                   "bySlot": {"app:vscode": {"id": "code-abc123", "name": "Code", "kind": "vscode"}},
                   "groups": [{"id": "all", "members": ["browser", "web:github"], "key": {"id": "gh-abc123", "name": "GitHub", "kind": "web"}}],
               },
           },
           "createdAt": "t", "updatedAt": "t"})
}

#[tokio::test]
async fn a_stack_installs_on_its_extensions_with_its_keys() {
    let e = env().await;
    let req = Install { wadspace: stack("academy"), template: dev(), extensions: Some(plus()), tz: None };
    let ws = images::install(&e.reg, &e.fake, "academy", &req).await.unwrap().workspace;
    assert_eq!(ws.image, C_REF);
    assert!(ws.env.iter().any(|(k, v)| k == "WALLPAPER" && v == "/run/wadspaces-extra/wallpaper.png"));
    let signins: Vec<(&str, &str)> = ws.signins.iter().map(|s| (s.id.as_str(), s.app.as_str())).collect();
    assert_eq!(signins, [("code-abc123", "vscode"), ("gh-abc123", "@browser"), ("gh-abc123", "github")]);
    // Only what it's built from, and only with a customization.
    let wrong = Install { wadspace: stack("academy"), template: dev(), extensions: None, tz: None };
    assert_eq!(images::install(&e.reg, &e.fake, "academy", &wrong).await.unwrap_err().code, ErrorCode::BadRequest);
    let mut bare = stack("academy");
    bare.as_object_mut().unwrap().remove("customization");
    let bare = Install { wadspace: bare, template: dev(), extensions: Some(plus()), tz: None };
    assert!(images::install(&e.reg, &e.fake, "academy", &bare).await.unwrap_err().message.contains("customization"));
}

#[tokio::test]
async fn files_show_whats_saved_and_keep_secrets_out() {
    let e = env().await;
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    for (p, body) in [
        ("Downloads/report.md", "# Report\n"),
        ("Downloads/photo.bin", "\0\0\0binary"),
        (".ssh/id_ed25519", "KEY"),
        (".local/share/wadbrowser/profiles/github/Cookies", "SESSION"),
        (".local/share/notes.txt", "kept"),
    ] {
        std::fs::create_dir_all(home.join(p).parent().unwrap()).unwrap();
        std::fs::write(home.join(p), body).unwrap();
    }
    let files = wadd::files::Files::with_home(&e.state, &d.path().join("signins"), Some(home.clone()));
    let b = e.fake.clone();
    let top: Vec<String> =
        files.list("home", "", &e.reg, &b).await.unwrap().entries.into_iter().map(|x| x.name).collect();
    assert!(top.contains(&"Downloads".to_string()) && !top.contains(&".ssh".to_string()), "{top:?}");
    let downloads = files.list("home", "Downloads", &e.reg, &b).await.unwrap().entries;
    let shown: Vec<(&str, bool)> = downloads.iter().map(|x| (x.name.as_str(), x.text)).collect();
    assert_eq!(shown, [("photo.bin", false), ("report.md", true)]);
    // WadBrowser's profiles (signed-in sessions) stay out; the rest of .local is there.
    let names = |list: wadd::files::FileList| list.entries.into_iter().map(|x| x.name).collect::<Vec<_>>();
    assert_eq!(names(files.list("home", ".local/share", &e.reg, &b).await.unwrap()), ["wadbrowser", "notes.txt"]);
    assert!(names(files.list("home", ".local/share/wadbrowser", &e.reg, &b).await.unwrap()).is_empty());
    assert_eq!(
        files.list("home", ".local/share/wadbrowser/profiles", &e.reg, &b).await.unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(files.read_text("home", ".ssh/id_ed25519", &e.reg, &b).await.unwrap_err().code, ErrorCode::Forbidden);
    assert_eq!(files.list("home", "../", &e.reg, &b).await.unwrap_err().code, ErrorCode::BadRequest);
    // Read, edit, delete.
    assert_eq!(files.read_text("home", "Downloads/report.md", &e.reg, &b).await.unwrap(), "# Report\n");
    files.write_text("home", "Downloads/report.md", "# Report v2\n", &e.reg, &b).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("Downloads/report.md")).unwrap(), "# Report v2\n");
    files.delete("home", "Downloads", &e.reg, &b).await.unwrap();
    assert!(!home.join("Downloads").exists());
    assert_eq!(files.delete("home", "", &e.reg, &b).await.unwrap_err().code, ErrorCode::BadRequest);
    assert!(files.roots(&e.reg).await.iter().any(|r| r.id == "home"));
}

#[tokio::test]
async fn what_a_build_makes_stays_after_the_tidy_up() {
    // A real base carries build_version, and every image built on it
    // inherits that label: those images aren't old bases.
    let e = env().await;
    e.fake.with(|m| {
        let labels = json!({"io.wadspaces.projects": "3", "build_version": "WadSpaces base version:- 20261017"});
        m.image_labels.insert(BASE.into(), labels.as_object().unwrap().clone());
    });
    let b = e.builds.create_layer(&custom(dev(), plus())).unwrap();
    let l = wait(&e, &b.id).await;
    assert_eq!(l.build.status, BuildStatus::Done, "{l:?}");
    let f = images::prune(&e.fake).await.unwrap();
    assert!(f.removed.is_empty(), "removed {:?}", f.removed);
    let here: Vec<String> = e.fake.0.lock().unwrap().images.iter().cloned().collect();
    for want in [BASE, T_REF, C_REF] {
        assert!(here.iter().any(|i| i == want), "{want} gone: {here:?}");
    }
    // An older base (not tagged as the base, nothing built on it) still goes.
    e.fake.with(|m| {
        let old = "localhost/old-base@1";
        m.images.insert(old.into());
        let labels = json!({"build_version": "WadSpaces base version:- 20261001"});
        m.image_labels.insert(old.into(), labels.as_object().unwrap().clone());
    });
    let f = images::prune(&e.fake).await.unwrap();
    assert_eq!(f.removed.len(), 1, "{:?}", f.removed);
}
