//! The account link (cloud.py's tests) against a fake Firebase: linking and
//! relinking, heartbeats, commands, project and secret sync, the other
//! machines, and the token's refresh.

use crate::common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::firebase::FakeFirebase;
use common::{Fake, ws};
use serde_json::{Value, json};
use wad_firebase::values::time;
use wad_firebase::{Functions, Identity};
use wad_proto::v1::Display;
use wadd::cloud::{CloudRelay, Endpoints, Parts, Settings};
use wadd::display::NullDisplay;
use wadd::drives::Drives;
use wadd::events::Bus;
use wadd::launches::Launches;
use wadd::projects::Projects;
use wadd::registry::{Registry, Settings as RegSettings};
use wadd::secrets::Secrets;
use wadd::view::View;

struct Env {
    _d: tempfile::TempDir,
    state: PathBuf,
    fb: FakeFirebase,
    _github: Arc<wiremock::MockServer>,
    fake: Fake,
    reg: Arc<Registry>,
    view: Arc<View>,
    projects: Arc<Projects>,
    relay: Arc<CloudRelay>,
    remake: Box<dyn Fn() -> Arc<CloudRelay> + Send + Sync>,
}

impl Env {
    /// A new relay on the same state, as after a restart (no ID token yet).
    fn restart(&self) -> Arc<CloudRelay> {
        (self.remake)()
    }
}

async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let fb = FakeFirebase::default();
    let base = fb.serve().await;
    let fake = Fake::default();
    fake.with(|m| {
        m.images.insert("ghcr.io/o/a:1".into());
        m.answering.insert("http://127.0.0.1:3100/".into());
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
        RegSettings {
            ready_timeout: Duration::from_secs(10),
            projects_dir: "/p".into(),
            signins_dir: "/s".into(),
            state_dir: state.clone(),
            rootless_uid: Some(1000),
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    let mut a = ws("a", "ghcr.io/o/a:1", Display::Stream, Some(3100));
    a.secrets = vec!["github_token".into()];
    reg.load(vec![a]).await.unwrap();
    let view = View::new(reg.clone(), Arc::new(fake.clone()), Arc::new(NullDisplay), bus.clone(), state.clone());
    let projects = Arc::new(Projects::new(
        &state,
        d.path().join("projects"),
        vec![],
        1000,
        Arc::new(|| ("local".into(), "Surface".into())),
        Arc::new(Drives::new(1000, Arc::new(wadd::drives::System), false)),
        Arc::new(wad_git::Git::default()),
        reg.clone(),
        bus.clone(),
    ));
    let copies = Arc::new(wadd::copies::Copies::new(
        &state,
        vec![],
        1000,
        Arc::new(|| ("local".into(), "Surface".into())),
        Arc::new(Drives::new(1000, Arc::new(wadd::drives::System), false)),
        reg.clone(),
    ));
    let launches =
        Launches::new(reg.clone(), view.clone(), Arc::new(fake.clone()), projects.clone(), copies, bus.clone(), &state);
    let secrets = Arc::new(Secrets::new(Arc::new(fake.clone()), &state));
    // GitHub: the owner and two repos.
    let gh_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/api/user"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({"login": "HungGod"})))
        .mount(&gh_server)
        .await;
    wiremock::Mock::given(wiremock::matchers::path("/api/user/repos"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([
            {"full_name": "HungGod/notes", "name": "notes", "private": true, "clone_url": "https://github.com/HungGod/notes.git", "default_branch": "main"},
            {"full_name": "HungGod/site", "name": "site", "private": false, "clone_url": "https://github.com/HungGod/site.git", "default_branch": "main"}])))
        .mount(&gh_server)
        .await;
    let github = wadd::github::GithubService::new(
        wad_github::Github::with_base(reqwest::Client::new(), &format!("{}/api", gh_server.uri())),
        wadd::github::Parts { backend: Arc::new(fake.clone()), secrets: secrets.clone(), projects: projects.clone() },
    );
    let gh_keep = Arc::new(gh_server);
    let builds = wadd::builds::Builds::new(Arc::new(fake.clone()), bus.clone(), &state, 1);
    let signins = wadd::signins::SignIns::new(&state, state.join("signins-data"), 1000, reg.clone(), bus.clone());
    let make = {
        let (builds, signins, backend) = (builds.clone(), signins.clone(), Arc::new(fake.clone()));
        let (reg, view, projects, state) = (reg.clone(), view.clone(), projects.clone(), state.clone());
        move || {
            let http = reqwest::Client::new();
            let ends = Endpoints {
                identity: Identity::with_base(http.clone(), &base),
                functions: Functions::new(http.clone(), &format!("{base}/functions")),
                firestore_base: base.clone(),
            };
            let settings = Settings {
                project_id: "demo".into(),
                api_key: "key".into(),
                heartbeat: Duration::from_secs(30),
                poll: Duration::from_millis(50),
                machine_name: "Surface".into(),
                state_dir: state.clone(),
            };
            let parts = Parts {
                registry: reg.clone(),
                view: view.clone(),
                projects: projects.clone(),
                secrets: secrets.clone(),
                launches: launches.clone(),
                github: github.clone(),
                bus: bus.clone(),
                builds: builds.clone(),
                signins: signins.clone(),
                backend: backend.clone(),
                tailnet: None,
            };
            CloudRelay::new(settings, http, ends, parts)
        }
    };
    let relay = make();
    Env { _d: d, state, fb, _github: gh_keep, fake, reg, view, projects, relay, remake: Box::new(make) }
}

async fn linked() -> Env {
    let e = env().await;
    e.fb.code("ABC123", "u1");
    e.relay.link("abc123").await.unwrap();
    e
}

#[tokio::test]
async fn linking_keeps_the_refresh_token_and_no_more() {
    let e = env().await;
    e.fb.code("ABC123", "u1");
    assert!(e.relay.link("nope!").await.is_err()); // not a code
    let err = e.relay.link("ZZZ999").await.unwrap_err();
    assert_eq!(err.message, "Unknown enrollment code.");
    let link = e.relay.link(" abc123 ").await.unwrap();
    assert!(link.linked);
    assert_eq!(
        (link.machine_id.as_deref(), link.owner_uid.as_deref(), link.project_id.as_deref()),
        (Some("m1"), Some("u1"), Some("demo"))
    );
    // enrollment.json: the Python wadd's fields, private.
    let file = e.state.join("enrollment.json");
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        (saved["machine_id"].as_str(), saved["refresh_token"].as_str(), saved["api_key"].as_str()),
        (Some("m1"), Some("rt:u1:m1:0"), Some("key"))
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let sent = e.fb.0.lock().unwrap().enrolls[1].clone();
    assert_eq!((sent["machineName"].as_str(), sent.get("previousIdToken")), (Some("Surface"), None));
    e.relay.unlink().await.unwrap();
    assert!(!e.relay.linked() && !file.exists());
}

#[tokio::test]
async fn heartbeats_say_only_what_the_rules_allow() {
    let e = linked().await;
    e.view.switch("a").unwrap();
    e.reg.settled("a").await;
    e.relay.heartbeat().await.unwrap();
    let doc = e.fb.doc("users/u1/machines/m1").unwrap();
    let mut keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
    keys.sort();
    // name: the function's. images, signins and screen: what the Builder reads.
    assert_eq!(
        keys,
        ["daemonVersion", "hostname", "images", "lastSeen", "name", "screen", "signins", "view", "workspaces"]
    );
    assert_eq!(
        (doc["images"].clone(), doc["signins"].clone(), doc["screen"].clone()),
        (json!([]), json!([]), Value::Null)
    );
    assert_eq!(doc["view"], "workspace:a");
    assert_eq!(doc["workspaces"][0]["phase"], "ready");
    assert_eq!(doc["workspaces"][0]["container"], "running");
    assert!(doc["lastSeen"].as_str().unwrap().ends_with('Z'));
    assert!(e.relay.link_state().last_heartbeat.is_some());
    e.view.home(true).unwrap();
    assert_eq!(e.relay.heartbeat_fields()["view"], "launcher"); // as the web app expects
}

#[tokio::test]
async fn commands_run_and_report_back() {
    let e = linked().await;
    let cmds = "users/u1/machines/m1/commands";
    e.fb.put(
        &format!("{cmds}/c2"),
        json!({"type": "start", "wsId": "a", "status": "pending", "createdAt": time(2000)}),
    );
    e.fb.put(&format!("{cmds}/c1"), json!({"type": "teleport", "status": "pending", "createdAt": time(1000)}));
    e.fb.put(&format!("{cmds}/c3"), json!({"type": "stop", "status": "pending", "createdAt": time(3000)})); // no wsId
    e.fb.put(&format!("{cmds}/c0"), json!({"type": "start", "wsId": "a", "status": "done"}));
    e.relay.poll_once().await.unwrap();
    let c = |id: &str| e.fb.doc(&format!("{cmds}/{id}")).unwrap();
    assert_eq!((c("c2")["status"].as_str(), &c("c2")["result"]), (Some("done"), &json!({"ok": true})));
    assert!(c("c2")["finishedAt"].is_string() && c("c2")["startedAt"].is_string());
    assert_eq!(
        (c("c1")["status"].as_str(), c("c1")["result"]["error"].as_str()),
        (Some("error"), Some("unknown command type \"teleport\""))
    );
    assert_eq!(c("c3")["result"]["error"], "stop needs wsId");
    assert!(c("c0").get("finishedAt").is_none()); // not pending: left alone
    e.reg.settled("a").await;
    assert_eq!(e.reg.state("a").unwrap().container, "running");
}

#[tokio::test]
async fn projects_sync_both_ways() {
    let e = linked().await;
    let base = "users/u1/projects";
    e.projects.save("mine", &json!({"name": "Mine", "mountName": "Mine", "source": {"kind": "git", "url": "https://github.com/o/mine"}})).unwrap();
    e.fb.put(
        &format!("{base}/theirs"),
        json!({"name": "Theirs", "mountName": "Theirs", "source": {"kind": "git", "url": "https://github.com/o/theirs"},
        "setup": "", "deleted": false, "createdAt": time(1000), "updatedAt": time(2000), "holders": {"old": true}}),
    );
    let (pulled, pushed) = e.relay.sync_projects().await.unwrap();
    assert_eq!((pulled, pushed), (1, 1));
    let up = e.fb.doc(&format!("{base}/mine")).unwrap();
    assert_eq!(up["name"], "Mine");
    assert!(up["updatedAt"].as_str().unwrap().ends_with('Z')); // a timestamp, as the rules want
    assert!(up.get("id").is_none() && up.get("synced").is_none());
    let here = e.projects.store.get("theirs").unwrap();
    assert_eq!((here["updatedAt"].as_i64(), here["synced"].as_bool()), (Some(2000), Some(true)));
    assert!(e.projects.store.get("mine").unwrap()["synced"].as_bool().unwrap());
    // A change online comes down; nothing goes back up.
    e.fb.put(&format!("{base}/theirs"), json!({"name": "Renamed", "mountName": "Theirs", "source": {"kind": "git", "url": "https://github.com/o/theirs"},
        "deleted": false, "createdAt": time(1000), "updatedAt": time(9000)}));
    assert_eq!(e.relay.sync_projects().await.unwrap(), (1, 0));
    assert_eq!(e.projects.store.get("theirs").unwrap()["name"], "Renamed");
}

#[tokio::test]
async fn secrets_come_from_the_account() {
    let e = linked().await;
    e.fb.put("users/u1/secrets/github_token", json!({"value": "ghp_account"}));
    e.fb.put("users/u1/secrets/tailscale_authkey", json!({"value": "tskey"}));
    let r = e.relay.sync_secrets().await.unwrap();
    assert_eq!(r.added, ["github_token"]);
    assert_eq!(e.fake.0.lock().unwrap().secrets.get("github_token").map(Vec::as_slice), Some(&b"ghp_account"[..]));
    assert!(!e.fake.0.lock().unwrap().secrets.contains_key("tailscale_authkey")); // Tailscale is parked
    let r = e.relay.execute(&json!({"type": "sync-secrets"})).await.unwrap();
    assert_eq!(r["added"], json!([]));
}

#[tokio::test]
async fn relinking_keeps_the_machine_for_its_owner_and_forgets_for_another() {
    let e = linked().await;
    e.projects
        .save("p1", &json!({"name": "P", "mountName": "P", "source": {"kind": "git", "url": "https://github.com/o/p"}}))
        .unwrap();
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"u1's".to_vec());
    });
    // The same owner: the same machine, and it said who it was.
    e.fb.code("SAME11", "u1");
    let l = e.relay.link("SAME11").await.unwrap();
    assert_eq!(l.machine_id.as_deref(), Some("m1"));
    assert!(
        e.fb.0.lock().unwrap().enrolls.last().unwrap()["previousIdToken"].as_str().unwrap().starts_with("id:u1:m1")
    );
    assert!(e.projects.store.get("p1").is_ok() && e.fake.0.lock().unwrap().secrets.contains_key("github_token"));
    // Another owner: a new machine, and none of u1's things.
    e.fb.code("OTHER2", "u2");
    let l = e.relay.link("OTHER2").await.unwrap();
    assert_eq!((l.owner_uid.as_deref(), l.machine_id.as_deref()), (Some("u2"), Some("m2")));
    assert!(e.projects.store.get("p1").is_err());
    assert!(
        std::fs::read_dir(&e.state).unwrap().any(|f| f
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("projects.u1."))
    );
    assert!(!e.fake.0.lock().unwrap().secrets.contains_key("github_token"));
    assert!(e.fb.doc("users/u1/machines/m1").is_none()); // u1's entry went with it
}

#[tokio::test]
async fn the_token_is_refreshed_and_a_new_refresh_token_kept() {
    let e = linked().await;
    // The sign-in's ID token is good for an hour: no refresh yet.
    e.relay.heartbeat().await.unwrap();
    assert_eq!(e.fb.0.lock().unwrap().refreshes, 0);
    // After a restart there's only the refresh token.
    let fresh = e.restart();
    assert!(fresh.linked());
    fresh.heartbeat().await.unwrap();
    fresh.heartbeat().await.unwrap();
    assert_eq!(e.fb.0.lock().unwrap().refreshes, 1); // then it's cached
    // Google gave a new refresh token: it's the one kept.
    let saved: Value = serde_json::from_slice(&std::fs::read(e.state.join("enrollment.json")).unwrap()).unwrap();
    assert_eq!(saved["refresh_token"], "rt:u1:m1:1");
}

#[tokio::test]
async fn the_account_being_unreachable_is_reported() {
    let e = linked().await;
    e.fb.0.lock().unwrap().down = Some(503);
    let err = e.relay.heartbeat().await.unwrap_err();
    assert!(err.contains("503"), "{err}");
    let relay = e.relay.clone();
    let task = tokio::spawn(relay.run());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(e.relay.link_state().last_error.unwrap_or_default().contains("503"));
    e.fb.0.lock().unwrap().down = None;
    e.relay.kick();
    for _ in 0..100 {
        if e.relay.link_state().last_error.is_none()
            && e.fb.doc("users/u1/machines/m1").unwrap().get("lastSeen").is_some()
        {
            task.abort();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never recovered: {:?}", e.relay.link_state());
}

#[tokio::test]
async fn the_repo_list_goes_to_the_account_when_it_changes() {
    let e = linked().await;
    assert_eq!(e.relay.sync_repos(false).await.unwrap(), None); // no token: nothing to say
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"gho_x".to_vec());
    });
    assert_eq!(e.relay.sync_repos(false).await.unwrap(), Some(true));
    let doc = e.fb.doc("users/u1/github/repos").unwrap();
    assert_eq!(doc["login"], "HungGod");
    assert_eq!(doc["repos"][0]["fullName"], "HungGod/notes");
    assert!(doc["updatedAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(e.relay.sync_repos(true).await.unwrap(), Some(false)); // unchanged: not written again
    let r = e.relay.execute(&json!({"type": "projects-sync"})).await.unwrap();
    assert_eq!(r["repos"], json!({"written": false}));
}

#[tokio::test]
async fn the_builders_commands_build_install_and_launch() {
    let e = linked().await;
    e.fake.with(|m| {
        m.images.insert("localhost/wadspaces-base:trixie".into());
        m.free_gb = 100.0;
    });
    let template = json!({"owner": "u1", "name": "Dev", "description": "", "apps": ["wadbrowser", "vscode"], "createdAt": "t", "updatedAt": "t"});
    let custom = json!({"owner": "u1", "name": "Dev plus", "description": "", "templateId": "dev", "browserMode": "browser",
        "webapps": [], "extensions": [], "tools": ["go"], "autostart": [], "createdAt": "t", "updatedAt": "t"});
    // The documents are in the account: the machine reads them.
    e.fb.put("users/u1/templates/dev", template.clone());
    e.fb.put("users/u1/customizations/dev-plus", custom.clone());
    let cmds = "users/u1/machines/m1/commands";
    e.fb.put(
        &format!("{cmds}/b1"),
        json!({"type": "build", "layer": "custom:dev-plus", "status": "pending", "createdAt": time(1000)}),
    );
    e.relay.poll_once().await.unwrap();
    let b1 = e.fb.doc(&format!("{cmds}/b1")).unwrap();
    assert_eq!(b1["status"], "done", "{b1}");
    assert!(b1["result"]["buildId"].is_string());
    for _ in 0..300 {
        if e.fake.0.lock().unwrap().images.contains("localhost/wadspaces-custom-dev-plus:latest") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let built: Vec<String> = e.fake.0.lock().unwrap().builds.iter().map(|b| b.0.clone()).collect();
    assert_eq!(built, ["localhost/wadspaces-template-dev:latest", "localhost/wadspaces-custom-dev-plus:latest"]);
    // A wadspace installed with its documents and the wallpaper the app drew,
    // then launched with the terminal as a focus session.
    let mut t = template.clone();
    t["id"] = "dev".into();
    let mut c = custom.clone();
    c["id"] = "dev-plus".into();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    e.fb.put(
        &format!("{cmds}/i1"),
        json!({"type": "install", "wsId": "academy", "status": "pending", "createdAt": time(2000), "tz": "Pacific/Fiji",
               "wadspace": {"version": 2, "id": "academy", "owner": "u1", "name": "Academy", "description": "",
                            "base": {"kind": "customization", "id": "dev-plus"}, "wallpaper": {"type": "color", "value": "#000"},
                            "projects": [], "secrets": {}, "createdAt": "t", "updatedAt": "t"},
               "template": t, "customization": c, "wallpaper": {"ext": "png", "data": png}}),
    );
    e.reg.put_workspace(common::app("terminal", "/usr/bin/foot")).await.unwrap();
    e.fb.put(
        &format!("{cmds}/l1"),
        json!({"type": "launch-run", "status": "pending", "createdAt": time(3000), "focusMinutes": 25,
               "items": [{"kind": "workspace", "workspace": "academy"}, {"kind": "terminal"}]}),
    );
    e.relay.poll_once().await.unwrap();
    let i1 = e.fb.doc(&format!("{cmds}/i1")).unwrap();
    assert_eq!(i1["status"], "done", "{i1}");
    assert_eq!(e.reg.workspace("academy").unwrap().image, "localhost/wadspaces-custom-dev-plus:latest");
    assert!(e.state.join("extra/academy/wallpaper.png").is_file());
    let l1 = e.fb.doc(&format!("{cmds}/l1")).unwrap();
    assert_eq!(l1["result"]["workspaces"], json!(["academy", "terminal"]), "{l1}");
    for _ in 0..300 {
        if e.view.session().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(e.view.session().unwrap().workspaces, ["academy", "terminal"]);
}
