use std::os::unix::fs::PermissionsExt;

use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn cfg() -> Config {
    Config {
        api_key: "k".into(),
        project_id: "wad-spaces".into(),
        auth_domain: "wad-spaces.firebaseapp.com".into(),
        functions_region: String::new(),
        emulator: None,
    }
}

async fn account(s: &MockServer, file: Option<PathBuf>) -> Account {
    let http = reqwest::Client::new();
    Account::with(
        cfg(),
        http.clone(),
        Identity::with_base(http.clone(), &s.uri()),
        Firestore::with_base(http, &s.uri(), "wad-spaces").unwrap(),
        file,
    )
}

async fn signed_in(s: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/identitytoolkit/accounts:signInWithIdp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "localId": "u1", "idToken": "id1", "refreshToken": "rt1", "expiresIn": "3600",
            "screenName": "Octo-Cat", "displayName": "Octo", "rawUserInfo": "{\"id\": 42}"
        })))
        .mount(s)
        .await;
}

const DOCS: &str = "/v1/projects/wad-spaces/databases/(default)/documents";

#[tokio::test]
async fn signs_in_keeps_the_session_privately_and_comes_back() {
    let s = MockServer::start().await;
    signed_in(&s).await;
    Mock::given(method("POST"))
        .and(path("/securetoken/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id_token": "id2", "refresh_token": "rt2", "expires_in": "3600"})),
        )
        .mount(&s)
        .await;
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("io.wadspaces.client/session.json");
    let a = account(&s, Some(file.clone())).await;
    let u = a.sign_in_github("gho_x").await.unwrap();
    assert_eq!((u.uid.as_str(), u.github_id.as_str(), u.login.as_deref()), ("u1", "42", Some("Octo-Cat")));
    assert_eq!(a.suggested_username(), "octo-cat");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(a.id_token().await.unwrap(), "id1");
    // The app restarts: back in from the file, with a fresh token.
    let b = account(&s, Some(file.clone())).await;
    assert_eq!(b.restore().await.map(|u| u.uid), Some("u1".into()));
    assert_eq!(b.id_token().await.unwrap(), "id2");
    b.sign_out();
    assert!(!file.exists() && b.user().is_none());
    assert!(matches!(b.id_token().await, Err(Error::SignedOut)));
}

#[tokio::test]
async fn closed_sign_ups_say_so() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "ADMIN_ONLY_OPERATION"}})))
        .mount(&s)
        .await;
    let e = account(&s, None).await.sign_in_github("t").await.unwrap_err();
    assert!(matches!(e, Error::Closed), "{e:?}");
}

#[tokio::test]
async fn a_free_github_login_becomes_the_username() {
    let s = MockServer::start().await;
    signed_in(&s).await;
    Mock::given(method("GET"))
        .and(path(format!("{DOCS}/profiles/u1")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{DOCS}/usernames/octo-cat")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{DOCS}:commit")))
        .and(body_partial_json(json!({"writes": [
            {"update": {"name": "projects/wad-spaces/databases/(default)/documents/usernames/octo-cat", "fields": {"uid": {"stringValue": "u1"}}}},
            {"update": {"fields": {"username": {"stringValue": "octo-cat"}, "onboarded": {"booleanValue": false}}},
             "updateTransforms": [{"fieldPath": "createdAt", "setToServerValue": "REQUEST_TIME"}]}
        ]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&s)
        .await;
    let a = account(&s, None).await;
    a.sign_in_github("t").await.unwrap();
    let p = a.ensure_profile().await.unwrap().unwrap();
    assert_eq!((p.username.as_str(), p.display_name.as_str(), p.onboarded), ("octo-cat", "Octo", false));
}

#[tokio::test]
async fn someone_elses_username_is_taken() {
    let s = MockServer::start().await;
    signed_in(&s).await;
    Mock::given(method("GET"))
        .and(path(format!("{DOCS}/usernames/octo")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"fields": {"uid": {"stringValue": "someone"}}})))
        .mount(&s)
        .await;
    let a = account(&s, None).await;
    a.sign_in_github("t").await.unwrap();
    assert!(a.username_taken("octo").await.unwrap());
    assert!(matches!(a.claim_username("octo", "O").await, Err(Error::Taken)));
    assert!(matches!(a.claim_username("No Spaces", "O").await, Err(Error::BadUsername)));
}

#[tokio::test]
async fn links_codes_and_the_github_token_go_to_the_account() {
    let s = MockServer::start().await;
    signed_in(&s).await;
    Mock::given(method("POST"))
        .and(path(format!("{DOCS}:commit")))
        .and(body_partial_json(json!({"writes": [{"update": {"fields": {"uid": {"stringValue": "u1"}, "used": {"booleanValue": false}, "machineName": {"stringValue": "Surface"}}}}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&s)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{DOCS}/users/u1/secrets/github_token")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&s)
        .await;
    Mock::given(path_regex(".*")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
    let a = account(&s, None).await;
    a.sign_in_github("t").await.unwrap();
    let code = a.create_enroll_code("Surface").await.unwrap();
    assert!(code.len() == 8 && code.chars().all(|c| "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(c)), "{code}");
    a.save_github_token("gho_x").await.unwrap();
}

#[tokio::test]
async fn the_library_is_read_as_them_and_kept() {
    let s = MockServer::start().await;
    signed_in(&s).await;
    let doc = |name: &str, fields: serde_json::Value| json!({"name": format!("projects/wad-spaces/databases/(default)/documents/{name}"), "fields": fields});
    Mock::given(method("GET"))
        .and(path(format!("{DOCS}/users/u1/browsers")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"documents": [doc("users/u1/browsers/b1", json!({"name": {"stringValue": "Mail"}}))]}),
        ))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{DOCS}:runQuery")))
        .and(body_partial_json(json!({"structuredQuery": {"where": {"fieldFilter": {"field": {"fieldPath": "owner"}, "value": {"stringValue": "u1"}}}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"document": doc("wadspaces/w1", json!({"version": {"integerValue": "3"}, "template": {"stringValue": "dev"}}))},
            {"document": doc("wadspaces/w2", json!({"version": {"integerValue": "4"}, "machine": {"stringValue": "m1"}}))},
        ])))
        .mount(&s)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&s).await;
    let d = tempfile::tempdir().unwrap();
    // SAFETY: tests in this binary don't read XDG_DATA_HOME elsewhere.
    unsafe { std::env::set_var("XDG_DATA_HOME", d.path()) };
    let a = account(&s, None).await;
    a.sign_in_github("t").await.unwrap();
    let lib = a.library().await.unwrap();
    assert_eq!(lib.browsers, [json!({"name": "Mail", "id": "b1"})]);
    // An old (version 3) wadspace is never shown.
    assert_eq!(lib.wadspaces, [json!({"version": 4, "machine": "m1", "id": "w2"})]);
    assert_eq!(a.cached_library(), Some(lib.clone()));
}
