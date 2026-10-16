//! GitHub in wadd (github.py's tests): a mock GitHub and the fake machine's
//! secrets.

use crate::common;

use std::sync::Arc;
use std::time::Duration;

use common::Fake;
use serde_json::json;
use wad_proto::ErrorCode;
use wad_proto::github::NewRepo;
use wadd::drives::Drives;
use wadd::events::Bus;
use wadd::github::{GithubService, Parts};
use wadd::projects::Projects;
use wadd::registry::{Registry, Settings};
use wadd::secrets::Secrets;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Env {
    _d: tempfile::TempDir,
    gh: MockServer,
    fake: Fake,
    projects: Arc<Projects>,
    svc: Arc<GithubService>,
}

async fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let gh = MockServer::start().await;
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
            ready_timeout: Duration::from_secs(5),
            projects_dir: "/p".into(),
            signins_dir: "/s".into(),
            state_dir: state.clone(),
            rootless_uid: None,
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    let projects = Arc::new(Projects::new(
        &state,
        d.path().join("projects"),
        vec![],
        1000,
        Arc::new(|| ("local".into(), "Surface".into())),
        Arc::new(Drives::new(1000, Arc::new(wadd::drives::System), false)),
        Arc::new(wad_git::Git::default()),
        reg,
        bus.clone(),
    ));
    let secrets = Arc::new(Secrets::new(Arc::new(fake.clone()), &state));
    let svc = GithubService::new(
        wad_github::Github::with_base(reqwest::Client::new(), &format!("{}/api", gh.uri())),
        Parts { backend: Arc::new(fake.clone()), secrets, projects: projects.clone() },
    );
    Mock::given(path("/api/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "HungGod"})))
        .mount(&gh)
        .await;
    Env { _d: d, gh, fake, projects, svc }
}

fn token_here(e: &Env) -> Option<String> {
    e.fake.0.lock().unwrap().secrets.get("github_token").map(|v| String::from_utf8_lossy(v).into_owned())
}

#[tokio::test]
async fn whose_token_and_signing_out() {
    let e = env().await;
    assert!(!e.svc.status().await.token);
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"gho_new".to_vec());
    });
    let st = e.svc.status().await;
    assert_eq!((st.token, st.login.as_deref()), (true, Some("HungGod")));
    e.svc.sign_out().await.unwrap();
    assert!(token_here(&e).is_none() && !e.svc.status().await.token);
}

#[tokio::test]
async fn repos_are_kept_a_minute_per_token() {
    let e = env().await;
    assert!(e.svc.repos(false).await.unwrap().is_none()); // no token
    Mock::given(path("/api/user/repos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"full_name": "HungGod/notes", "name": "notes", "clone_url": "https://github.com/HungGod/notes.git"}])))
        .mount(&e.gh)
        .await;
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"gho_a".to_vec());
    });
    let calls = || async {
        e.gh.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/api/user/repos").count()
    };
    let r = e.svc.repos(false).await.unwrap().unwrap();
    assert_eq!((r.login.as_str(), r.repos[0].name.as_str()), ("HungGod", "notes"));
    e.svc.repos(false).await.unwrap();
    assert_eq!(calls().await, 1); // from the last minute's
    e.svc.repos(true).await.unwrap();
    assert_eq!(calls().await, 2); // fresh when asked
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"gho_b".to_vec());
    });
    e.svc.repos(false).await.unwrap();
    assert_eq!(calls().await, 3); // another token: its own list
}

#[tokio::test]
async fn a_new_repo_and_its_project() {
    let e = env().await;
    e.fake.with(|m| {
        m.secrets.insert("github_token".into(), b"gho_a".to_vec());
    });
    Mock::given(method("POST"))
        .and(path("/api/user/repos"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"full_name": "HungGod/notes", "name": "notes", "private": true, "clone_url": "https://github.com/HungGod/notes.git", "default_branch": "main"})))
        .mount(&e.gh)
        .await;
    let req = |name: &str, mount: Option<&str>| NewRepo {
        name: name.into(),
        private: true,
        description: String::new(),
        mount_name: mount.map(String::from),
        setup: String::new(),
    };
    let p = e.svc.create_repo(&req("notes", None)).await.unwrap();
    assert_eq!(p.mount_name, "notes");
    assert!(
        matches!(&p.source, Some(wad_proto::v1::ProjectSource::Git { url, .. }) if url == "https://github.com/HungGod/notes.git")
    );
    // Refused before anything is made on GitHub: a bad name, a folder name in use.
    assert_eq!(e.svc.create_repo(&req("no spaces", None)).await.unwrap_err().code, ErrorCode::BadRequest);
    assert_eq!(e.svc.create_repo(&req("other", Some("notes"))).await.unwrap_err().code, ErrorCode::Conflict);
    let posts = e.gh.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "POST").count();
    assert_eq!(posts, 1);
    assert_eq!(e.projects.list(false).len(), 1);
    // No token: say so.
    e.fake.with(|m| {
        m.secrets.clear();
    });
    assert_eq!(e.svc.create_repo(&req("third", None)).await.unwrap_err().code, ErrorCode::Conflict);
}
