use serde_json::json;
use wad_proto::v1::{ProjectSource, SessionMode};

use super::*;

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn state() -> (tempfile::TempDir, State) {
    let d = tempfile::tempdir().unwrap();
    let st = State::new(d.path());
    (d, st)
}

#[test]
fn projects_current_legacy_and_tombstones() {
    let (d, st) = state();
    let dir = d.path();
    write(dir, "projects/web.json", &json!({"id": "web", "name": "web", "mountName": "Web", "source": {"kind": "git", "url": "https://github.com/o/web/", "ref": "main"}, "setup": "npm ci", "deleted": false, "createdAt": 1, "updatedAt": 2, "synced": true, "holders": {}}).to_string());
    write(dir, "projects/notes.json", &json!({"id": "notes", "name": "Notes", "mountName": "Notes", "source": {"kind": "drive", "uuid": "1234-AB", "label": "L", "fstype": "exfat", "subpath": "/a//./b/"}, "deleted": true}).to_string());
    write(
        dir,
        "projects/old.json",
        &json!({"id": "old", "name": "Old", "mountName": "Old", "source": {"kind": "local"}}).to_string(),
    );
    write(dir, "projects/here.json", &json!({"id": "here", "name": "Here", "mountName": "Here", "source": {"kind": "folder", "machineId": "local", "machineName": "", "path": "/var/home/wad/x"}}).to_string());
    write(dir, "projects/bad.json", "{half");
    write(dir, "projects/skip.json.tmp", "{}");
    let ps = st.projects();
    assert_eq!(ps.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["here", "notes", "old", "web"]);
    let web = &ps[3];
    assert_eq!(
        web.source,
        Some(ProjectSource::Git { url: "https://github.com/o/web".into(), git_ref: Some("main".into()) })
    );
    assert!(web.synced && !web.legacy && web.updated_at == 2);
    assert_eq!(
        ps[1].source,
        Some(ProjectSource::Drive {
            uuid: "1234-AB".into(),
            label: "L".into(),
            fstype: "exfat".into(),
            subpath: "a/b".into()
        })
    );
    assert!(ps[1].deleted);
    assert!(ps[2].legacy && ps[2].source.is_none());
    assert!(matches!(&ps[0].source, Some(ProjectSource::Folder { machine_id, .. }) if machine_id == "local"));
}

#[test]
fn folder_paths_must_be_plain() {
    for bad in ["relative", "/", "/a/../b", "/a/", "/a//b", "/a:b", "/50%"] {
        let src = json!({"kind": "folder", "machineId": "m", "path": bad});
        assert_eq!(current_source(&src), None, "{bad}");
    }
}

#[test]
fn runs_newest_first_with_unfinished_ones_closed() {
    let (d, st) = state();
    write(
        d.path(),
        "runs.jsonl",
        &[
            json!({"event": "start", "id": "r1", "wadspaceId": "a", "wadspaceName": "A", "mode": "local", "user": "local", "projects": ["p"], "startedAt": 10.0, "endedAt": null}),
            json!({"event": "end", "id": "r1", "endedAt": 20.0}),
            json!({"event": "start", "id": "r2", "wadspaceId": "b", "wadspaceName": "B", "mode": "local", "user": "local", "startedAt": 30.0, "endedAt": null}),
        ]
        .iter()
        .map(|l| l.to_string() + "\n")
        .chain(["not json\n".to_string()])
        .collect::<String>(),
    );
    let runs = st.runs(None, 10);
    assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r2", "r1"]);
    assert_eq!(runs[0].ended_at, Some(30.0)); // wadd went away mid-run
    assert_eq!(runs[1].ended_at, Some(20.0));
    assert_eq!(runs[1].projects, ["p"]);
    assert_eq!(st.runs(Some("a"), 10).len(), 1);
    assert_eq!(st.runs(None, 1).len(), 1);
}

#[test]
fn runs_read_what_was_appended_and_start_over_on_a_new_file() {
    use std::io::Write;
    let (d, st) = state();
    let path = d.path().join("runs.jsonl");
    let line = |v: serde_json::Value| v.to_string() + "\n";
    std::fs::write(&path, line(json!({"event": "start", "id": "r1", "wadspaceId": "a", "startedAt": 10.0}))).unwrap();
    assert_eq!(st.runs(None, 10).len(), 1);
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    // Its end, and half of the next line (still being written).
    let half = r#"{"event": "start", "id""#;
    write!(f, "{}{half}", line(json!({"event": "end", "id": "r1", "endedAt": 15.0}))).unwrap();
    let runs = st.runs(None, 10);
    assert_eq!((runs.len(), runs[0].ended_at), (1, Some(15.0)));
    writeln!(f, r#": "r2", "wadspaceId": "b", "startedAt": 20.0}}"#).unwrap();
    assert_eq!(st.runs(None, 10).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r2", "r1"]);
    // Replaced by a shorter file: read again from the top.
    std::fs::write(&path, line(json!({"event": "start", "id": "r9", "wadspaceId": "c", "startedAt": 1.0}))).unwrap();
    assert_eq!(st.runs(None, 10).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r9"]);
    std::fs::remove_file(&path).unwrap();
    assert!(st.runs(None, 10).is_empty());
}

#[test]
fn session_link_library_and_secrets() {
    let (d, st) = state();
    let dir = d.path();
    assert!(st.session(&["a".into()]).is_none());
    write(
        dir,
        "session.json",
        &json!({"workspaces": ["a", "gone"], "started_at": 5.0, "minutes": 25, "ends_at": null, "expired": false})
            .to_string(),
    );
    let s = st.session(&["a".into()]).unwrap();
    assert_eq!(s.mode, SessionMode::Focus);
    assert_eq!(s.workspaces, ["a"]);
    assert!(!s.expired);
    write(
        dir,
        "session.json",
        &json!({"workspaces": ["a"], "mode": "focus", "started_at": 5.0, "minutes": 1, "ends_at": 6.0}).to_string(),
    );
    assert!(st.session(&["a".into()]).unwrap().expired); // its time passed while wadd was off
    assert!(st.session(&[]).is_none());

    assert!(!st.cloud().linked);
    write(dir, "enrollment.json", &json!({"machine_id": "m1", "owner_uid": "u1", "project_id": "wad-spaces", "refresh_token": "SECRET", "enrolled_at": "2026-10-01T00:00:00"}).to_string());
    let link = st.cloud();
    assert!(link.linked && link.machine_id.as_deref() == Some("m1"));
    assert!(!serde_json::to_string(&link).unwrap().contains("SECRET"));

    write(dir, "library/browsers/x.json", &json!({"id": "x", "name": "Mail"}).to_string());
    assert_eq!(st.library("browsers").unwrap().len(), 1);
    // The old Builder's: not collections now, and removed.
    write(dir, "library/drafts/x.json", &json!({"id": "x", "name": "Draft"}).to_string());
    write(dir, "library/templates/dev.json", &json!({"id": "dev", "name": "Dev"}).to_string());
    write(dir, "library/wadspaces/w.json", &json!({"version": 3, "id": "w"}).to_string());
    assert!(st.library("drafts").is_none() && st.library("templates").is_none() && st.library("wadspaces").is_none());
    assert_eq!(st.discard_old_builder().unwrap(), 3);
    assert!(!dir.join("library/drafts").exists() && !dir.join("library/templates").exists());
    assert!(dir.join("library/browsers/x.json").exists());
    assert_eq!(st.discard_old_builder().unwrap(), 0);
    assert!(st.library("../etc").is_none());

    write(dir, "seeded-secrets.json", &json!({"github_token": "abc"}).to_string());
    assert_eq!(st.seeded_secrets(), ["github_token"]);
}
