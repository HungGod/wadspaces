//! Against a fake podman on a temporary socket; and (ignored by default) the
//! real rootless one: cargo test -p wad-podman -- --ignored

use axum::Router;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::json;

use super::*;

async fn fake() -> (tempfile::TempDir, Podman) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("podman.sock");
    let app = Router::new()
        .route("/v5.0.0/libpod/_ping", get(|| async { "OK" }))
        .route("/v5.0.0/libpod/info", get(|| async { axum::Json(json!({"store": {"graphRoot": "/var/lib/containers/storage"}})) }))
        .route(
            "/v5.0.0/libpod/images/{name}/exists",
            get(|Path(name): Path<String>| async move { if name == "ghcr.io/o/here:latest" { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND } }),
        )
        .route(
            "/v5.0.0/libpod/images/{name}/json",
            get(|Path(name): Path<String>| async move {
                if name == "ghcr.io/o/here:latest" {
                    axum::Json(json!({"Labels": {"io.wadspaces.projects": "1"}})).into_response()
                } else {
                    (StatusCode::NOT_FOUND, axum::Json(json!({"message": "no such image"}))).into_response()
                }
            }),
        )
        .route(
            "/v5.0.0/libpod/containers/{name}/json",
            get(|Path(name): Path<String>| async move {
                if name == "wad-up" { axum::Json(json!({"State": {"Status": "Running"}})).into_response() } else { StatusCode::NOT_FOUND.into_response() }
            }),
        )
        .route(
            "/v5.0.0/libpod/containers/json",
            get(|Query(q): Query<std::collections::HashMap<String, String>>| async move {
                let f: serde_json::Value = serde_json::from_str(q.get("filters").map(String::as_str).unwrap_or("{}")).unwrap();
                assert_eq!(f, json!({"label": ["wadspaces.role=display"]}));
                axum::Json(json!([{"Names": ["wad-a-display"], "Labels": {"wadspaces.id": "a", "wadspaces.stream": "g1"}}]))
            }),
        )
        .route(
            "/v5.0.0/libpod/containers/{name}/stop",
            post(|Path(name): Path<String>| async move {
                match name.as_str() {
                    "wad-up" => StatusCode::NO_CONTENT,
                    "wad-stopped" => StatusCode::NOT_MODIFIED,
                    _ => StatusCode::NOT_FOUND,
                }
            }),
        )
        .route(
            "/v5.0.0/libpod/volumes/{name}/json",
            get(|Path(name): Path<String>| async move {
                if name == "wad-a-config" { axum::Json(json!({"Mountpoint": "/vol/_data"})).into_response() } else { StatusCode::NOT_FOUND.into_response() }
            }),
        )
        .route(
            "/v5.0.0/libpod/secrets/{name}/json",
            get(|Path(name): Path<String>, Query(q): Query<std::collections::HashMap<String, String>>| async move {
                match (name.as_str(), q.get("showsecret").map(String::as_str)) {
                    ("github_token", Some("true")) => axum::Json(json!({"Spec": {"Name": "github_token"}, "SecretData": "ghp_x"})).into_response(),
                    ("github_token", _) => axum::Json(json!({"Spec": {"Name": "github_token"}})).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }),
        )
        .route(
            "/v5.0.0/libpod/build",
            post(|Query(q): Query<std::collections::HashMap<String, String>>, body: axum::body::Bytes| async move {
                let args: serde_json::Value = serde_json::from_str(q.get("buildargs").map(String::as_str).unwrap_or("{}")).unwrap();
                assert_eq!(&body[..], b"tar", "the build context arrives as the body");
                if q.get("t").map(String::as_str) == Some("localhost/wadspaces-bad:latest") {
                    return "{\"stream\":\"STEP 1/2: FROM base\\n\"}\n{\"error\":\"exit status 1\",\"errorDetail\":{\"message\":\"RUN failed: exit status 1\"}}\n".to_string();
                }
                format!(
                    "{{\"stream\":\"STEP 1/2: FROM {}\\n\"}}\n{{\"stream\":\"STEP 2/2: COPY root/ /\\n--> abc\\n\"}}\n{{\"stream\":\"{}\\n\"}}\n",
                    args["BASE_IMAGE"].as_str().unwrap(),
                    "f".repeat(64)
                )
            }),
        );
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (dir, Podman::new(sock))
}

#[tokio::test]
async fn images_containers_and_builds() {
    let (_d, p) = fake().await;
    assert!(p.ping().await);
    assert_eq!(p.graph_root().await.unwrap().as_deref(), Some("/var/lib/containers/storage"));
    assert!(p.image_exists("ghcr.io/o/here:latest").await.unwrap());
    assert!(!p.image_exists("ghcr.io/o/gone:latest").await.unwrap());
    assert_eq!(p.image_labels("ghcr.io/o/here:latest").await.unwrap().unwrap()["io.wadspaces.projects"], "1");
    assert!(p.image_labels("nope").await.unwrap().is_none());
    assert_eq!(p.container_status("wad-up").await.unwrap(), "running");
    assert_eq!(p.container_status("wad-gone").await.unwrap(), "missing");
    let sidecars = p.containers_labelled("wadspaces.role=display").await.unwrap();
    assert_eq!(sidecars.len(), 1);
    assert_eq!((sidecars[0].name.as_str(), &sidecars[0].labels["wadspaces.stream"]), ("wad-a-display", &json!("g1")));
    assert!(p.stop_container("wad-up", 10).await.unwrap());
    assert!(p.stop_container("wad-stopped", 10).await.unwrap());
    assert!(!p.stop_container("wad-gone", 10).await.unwrap());
    assert_eq!(p.volume_mountpoint("wad-a-config").await.unwrap().as_deref(), Some("/vol/_data"));
    assert_eq!(p.volume_mountpoint("nope").await.unwrap(), None);
    assert_eq!(p.secret_value("github_token").await.unwrap().as_deref(), Some("ghp_x"));
    assert_eq!(p.secret_value("nope").await.unwrap(), None);
    let mut out = vec![];
    let args = serde_json::json!({"BASE_IMAGE": "localhost/wadspaces-base:trixie"}).as_object().unwrap().clone();
    let labels = serde_json::json!({"io.wadspaces.layer": "template:x"}).as_object().unwrap().clone();
    let id = p
        .build(b"tar".to_vec(), "localhost/wadspaces-ok:latest", &args, &labels, |l| out.push(l.to_string()))
        .await
        .unwrap();
    assert_eq!(id, "f".repeat(64));
    assert_eq!(out, ["STEP 1/2: FROM localhost/wadspaces-base:trixie", "STEP 2/2: COPY root/ /", "--> abc"]);
    let e = p.build(b"tar".to_vec(), "localhost/wadspaces-bad:latest", &args, &labels, |_| {}).await.unwrap_err();
    assert_eq!(e.to_string(), "RUN failed: exit status 1");
}

#[tokio::test]
async fn no_podman() {
    let p = Podman::new("/nonexistent/podman.sock");
    assert!(!p.ping().await);
    assert!(matches!(p.image_exists("x").await, Err(Error::Connect(..))));
}

#[test]
fn segments_are_encoded() {
    assert_eq!(segment("ghcr.io/o/n:1@sha256:ab"), "ghcr.io%2Fo%2Fn%3A1%40sha256%3Aab");
}

/// The real rootless podman (start its socket: systemctl --user start podman.socket).
#[tokio::test]
#[ignore]
async fn real_rootless_podman() {
    let sock = format!("{}/podman/podman.sock", std::env::var("XDG_RUNTIME_DIR").unwrap());
    let p = Podman::new(sock);
    assert!(p.ping().await, "podman.socket isn't running");
    assert!(p.graph_root().await.unwrap().is_some());
    assert!(!p.image_exists("localhost/wadspaces-does-not-exist:latest").await.unwrap());
    assert_eq!(p.container_status("wad-does-not-exist").await.unwrap(), "missing");
}

#[test]
fn container_output_frames() {
    let mut framed = vec![1, 0, 0, 0, 0, 0, 0, 6];
    framed.extend_from_slice(b"hello\n");
    framed.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 4]);
    framed.extend_from_slice(b"oops");
    assert_eq!(demux(&framed), "hello\noops");
    assert_eq!(demux(b"plain text\n"), "plain text\n");
}

#[test]
fn image_times_are_read_as_unix_seconds() {
    assert_eq!(crate::rfc3339_secs("1970-01-01T00:00:00Z"), Some(0.0));
    assert_eq!(crate::rfc3339_secs("2026-10-11T19:05:00Z"), Some(1_791_745_500.0));
    assert_eq!(crate::rfc3339_secs("2026-10-11T19:05:00.5+12:00"), Some(1_791_702_300.5));
    assert_eq!(crate::rfc3339_secs("nonsense"), None);
}
