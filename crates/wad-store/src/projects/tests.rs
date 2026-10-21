//! projects.py's store tests, ported.

use serde_json::{Value, json};

use super::*;

fn gh(repo: &str) -> Value {
    json!({"kind": "git", "url": format!("https://github.com/o/{repo}.git")})
}

fn notes() -> Value {
    json!({"name": "Notes", "mountName": "Notes", "source": gh("notes")})
}

fn vault() -> Value {
    json!({"name": "Writing vault", "mountName": "Writing", "setup": "npm install",
           "source": {"kind": "git", "url": "https://github.com/o/vault.git", "ref": "main"}})
}

fn store(d: &tempfile::TempDir) -> ProjectStore {
    ProjectStore::new(d.path().join("projects"), vec![], || (LOCAL.into(), "wadspaces".into()))
}

fn with(base: &Value, extra: Value) -> Value {
    let mut v = base.clone();
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

#[test]
fn new_ids_look_like_firestore_auto_ids() {
    let ids: std::collections::HashSet<String> = (0..50).map(|_| new_id()).collect();
    assert_eq!(ids.len(), 50);
    assert!(ids.iter().all(|i| i.len() == 20 && i.chars().all(|c| c.is_ascii_alphanumeric())));
}

#[test]
fn put_fills_in_defaults_and_get_list() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let doc = s.put("p1", &notes()).unwrap();
    assert_eq!(
        keys(&doc),
        ["id", "name", "mountName", "source", "setup", "deleted", "createdAt", "updatedAt", "synced"]
    );
    assert_eq!((&doc["source"], &doc["setup"], &doc["deleted"]), (&gh("notes"), &json!(""), &json!(false)));
    assert_eq!(doc["createdAt"], doc["updatedAt"]);
    assert_eq!(doc["synced"], false);
    assert_eq!(s.get("p1").unwrap(), doc);
    assert_eq!(s.list(false), [doc]);
    assert_eq!(s.get("nope"), Err(ProjectError::NotFound("nope".into())));
}

#[test]
fn updates_keep_created_and_move_updated_on() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let first = s.put("p1", &vault()).unwrap();
    let again = s
        .put("p1", &with(&vault(), json!({"setup": "make", "createdAt": 1, "updatedAt": 1, "folderId": "x", "holders": {"m": {}}, "ignore": ["x"]})))
        .unwrap();
    assert_eq!(again["createdAt"], first["createdAt"]);
    assert!(again["updatedAt"].as_i64() > first["updatedAt"].as_i64());
    assert_eq!(again["setup"], "make"); // the store owns the timestamps; old fields are dropped
    assert!(["folderId", "holders", "ignore"].iter().all(|k| again.get(k).is_none()));
    assert_eq!(s.changes(), 2);
}

#[test]
fn validation() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let a = |src: Value| json!({"name": "A", "mountName": "A", "source": src});
    let cases: Vec<(Value, &str)> = vec![
        (json!({"mountName": "A", "source": gh("a")}), "name"),
        (json!({"name": "A", "mountName": "a/b", "source": gh("a")}), "mountName"),
        (json!({"name": "A", "mountName": "..", "source": gh("a")}), "mountName"),
        (json!({"name": "A", "mountName": "A"}), NOT_A_SOURCE),
        (a(json!({"kind": "empty"})), NOT_A_SOURCE),
        (a(json!({"kind": "local"})), NOT_A_SOURCE),
        (a(json!("https://github.com/o/r")), NOT_A_SOURCE),
        (a(json!({"kind": "git", "url": "http://github.com/o/r"})), "GitHub repo"),
        (a(json!({"kind": "git", "url": "git@github.com:o/r.git"})), "GitHub repo"),
        (a(json!({"kind": "git", "url": "https://gitlab.com/o/r"})), "GitHub repo"),
        (a(json!({"kind": "git", "url": "https://github.com/o"})), "GitHub repo"),
        (a(json!({"kind": "git", "url": "https://github.com/o/r/tree/x"})), "GitHub"),
        (a(json!({"kind": "git", "url": "file:///etc"})), "GitHub repo"),
        (a(with(&gh("r"), json!({"ref": "--upload-pack=x"}))), "ref"),
        (with(&a(gh("r")), json!({"setup": ["make"]})), "setup"),
    ];
    for (body, msg) in cases {
        match s.put("p1", &body) {
            Err(ProjectError::Invalid(m)) => assert!(m.contains(msg), "{body}: {m}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}

#[test]
fn github_urls() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    for (url, kept) in [
        ("https://github.com/o/r.git", "https://github.com/o/r.git"),
        ("https://github.com/o-x/r.y_z", "https://github.com/o-x/r.y_z"),
        ("https://github.com/o/r/", "https://github.com/o/r"), // a trailing slash goes
    ] {
        let doc = s.put("p1", &with(&notes(), json!({"source": {"kind": "git", "url": url}}))).unwrap();
        assert_eq!(doc["source"]["url"], kept);
    }
    assert!(s.put("bad id", &notes()).is_err());
}

#[test]
fn old_documents_still_load_marked_legacy() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let root = d.path().join("projects");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("old.json"), json!({"id": "old", "name": "Old", "mountName": "Old", "source": {"kind": "empty"}, "setup": "",
        "ignore": ["node_modules"], "folderId": "wad-old", "holders": {"m1": {}}, "deleted": false, "createdAt": 1, "updatedAt": 2, "synced": true}).to_string()).unwrap();
    std::fs::write(
        root.join("ssh.json"),
        json!({"id": "ssh", "name": "Ssh", "mountName": "Ssh", "source": {"kind": "git", "url": "git@github.com:o/r"},
        "deleted": false, "createdAt": 1, "updatedAt": 2})
        .to_string(),
    )
    .unwrap();
    s.put("new", &notes()).unwrap();
    let old = s.get("old").unwrap();
    assert_eq!(old["legacy"], true);
    assert!(["ignore", "folderId", "holders"].iter().all(|k| old.get(k).is_none()));
    assert_eq!(s.get("ssh").unwrap()["legacy"], true);
    assert!(s.get("new").unwrap().get("legacy").is_none());
    let flags: Vec<bool> = s.list(false).iter().map(is_legacy).collect();
    assert_eq!(flags, [false, true, true]); // Notes, Old, Ssh
    // Saving one makes it a GitHub repo like any other.
    assert!(s.put("old", &json!({"name": "Old", "mountName": "Old", "source": {"kind": "empty"}})).is_err());
    assert!(
        s.put("old", &json!({"name": "Old", "mountName": "Old", "source": gh("old")})).unwrap().get("legacy").is_none()
    );
    assert!(!std::fs::read_to_string(root.join("old.json")).unwrap().contains("legacy"));
}

#[test]
fn mount_names_from_repo_names() {
    assert_eq!(mount_for("my-repo.v2"), "my-repo.v2");
    assert_eq!(mount_for("a b/c"), "a-b-c");
    assert_eq!((mount_for(".."), mount_for("")), ("project".into(), "project".into()));
    assert_eq!(mount_for(&"x".repeat(100)).len(), 64);
}

#[test]
fn mount_names_are_unique_among_live_projects() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    s.put("p1", &notes()).unwrap();
    assert!(
        matches!(s.put("p2", &with(&notes(), json!({"name": "Other"}))), Err(ProjectError::Conflict(m)) if m.contains("Notes"))
    );
    s.put("p1", &with(&notes(), json!({"name": "Renamed"}))).unwrap(); // itself is fine
    s.delete("p1").unwrap();
    assert_eq!(s.put("p2", &with(&notes(), json!({"name": "Other"}))).unwrap()["mountName"], "Notes");
}

#[test]
fn deleting_leaves_a_tombstone() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let doc = s.put("p1", &notes()).unwrap();
    let gone = s.delete("p1").unwrap();
    assert_eq!(gone["deleted"], true);
    assert!(gone["updatedAt"].as_i64() > doc["updatedAt"].as_i64());
    assert!(s.list(false).is_empty());
    assert_eq!(s.list(true), std::slice::from_ref(&gone));
    assert_eq!(s.delete("p1").unwrap(), gone); // again: nothing changes
    assert_eq!(s.put("p1", &notes()).unwrap()["deleted"], false); // saving brings it back
}

#[test]
fn merging_newest_wins_both_ways() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    s.put("mine", &notes()).unwrap();
    let older = s.put("older", &json!({"name": "Old here", "mountName": "Old", "source": gh("old")})).unwrap();
    let newer = s.put("newer", &json!({"name": "New here", "mountName": "New", "source": gh("new")})).unwrap();
    let t = |v: &Value| v["updatedAt"].as_i64().unwrap();
    let remote = vec![
        with(
            &older,
            json!({"name": "Changed online", "updatedAt": t(&older) + 1000, "holders": {"m2": {}}, "ignore": ["x"], "folderId": "wad-older"}),
        ),
        with(&newer, json!({"name": "Stale online", "updatedAt": t(&newer) - 1000})),
        json!({"id": "theirs", "name": "Made online", "mountName": "Online", "source": gh("online"), "updatedAt": 5, "createdAt": 4}),
        json!({"id": "empty", "name": "Old kind", "mountName": "Empty", "source": {"kind": "empty"}, "updatedAt": 5}),
    ];
    let mut push: Vec<String> = s.merge(&remote).iter().map(|d| d["id"].as_str().unwrap().to_string()).collect();
    push.sort();
    assert_eq!(push, ["mine", "newer"]); // never sent + newer here
    let o = s.get("older").unwrap();
    assert_eq!((&o["name"], &o["synced"]), (&json!("Changed online"), &json!(true)));
    assert!(["holders", "ignore", "folderId"].iter().all(|k| o.get(k).is_none())); // dropped on the way in
    assert_eq!(s.get("newer").unwrap()["name"], "New here");
    let theirs = s.get("theirs").unwrap();
    assert_eq!((&theirs["synced"], &theirs["source"], &theirs["updatedAt"]), (&json!(true), &gh("online"), &json!(5)));
    assert!(s.get_or_none("empty").is_none()); // not a GitHub repo: not taken in
    assert_eq!(s.changes(), 3); // what a sync brings in isn't a local change
    s.mark_synced(&["mine".into(), "newer".into()]);
    assert_eq!(s.get("mine").unwrap()["synced"], true);
}

#[test]
fn merging_takes_tombstones_and_outright_deletes() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let a = s.put("a", &notes()).unwrap();
    s.put("b", &json!({"name": "B", "mountName": "B", "source": gh("b")})).unwrap();
    s.mark_synced(&["a".into(), "b".into()]);
    s.put("c", &json!({"name": "C", "mountName": "C", "source": gh("c")})).unwrap(); // never synced: goes up
    let push = s.merge(&[json!({"id": "a", "deleted": true, "updatedAt": a["updatedAt"].as_i64().unwrap() + 1})]);
    let a = s.get("a").unwrap();
    assert_eq!((&a["deleted"], &a["name"]), (&json!(true), &json!("Notes")));
    assert_eq!(s.get("b").unwrap()["deleted"], true); // the cloud knew it and dropped it
    assert_eq!(push.iter().map(|d| d["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c"]);
}

#[test]
fn legacy_projects_are_never_pushed() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    std::fs::create_dir_all(d.path().join("projects")).unwrap();
    std::fs::write(
        d.path().join("projects/old.json"),
        json!({"id": "old", "name": "Old", "mountName": "Old", "source": {"kind": "local"},
        "holders": {}, "deleted": false, "createdAt": 1, "updatedAt": 2})
        .to_string(),
    )
    .unwrap();
    s.put("new", &notes()).unwrap();
    assert_eq!(s.merge(&[]).iter().map(|d| d["id"].as_str().unwrap()).collect::<Vec<_>>(), ["new"]);
}

#[test]
fn manifests_are_written_atomically_with_setup_hashes() {
    use wad_proto::v1::{CopyFrom, ProjectKind, WorkspaceProject};
    let d = tempfile::tempdir().unwrap();
    let github = WorkspaceProject {
        id: "p1".into(),
        kind: ProjectKind::Github,
        mount: "Writing".into(),
        repo: "HungGod/Writing".into(),
        git_ref: Some("main".into()),
        setup: "npm install".into(),
        from: None,
        mounted: false,
    };
    let copy = WorkspaceProject {
        id: "c1".into(),
        kind: ProjectKind::Copy,
        mount: "Notes".into(),
        repo: String::new(),
        git_ref: None,
        setup: String::new(),
        from: Some(CopyFrom { machine_id: "m1".into(), drive: None, paths: vec!["/home/wad/Notes".into()] }),
        mounted: false,
    };
    let path = write_manifest(d.path(), "writing", &[github, copy]).unwrap();
    assert_eq!(path, d.path().join("extra/writing/projects.json"));
    let data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        data,
        json!({"version": 2, "projects": [
        {"id": "p1", "kind": "github", "mount": "Writing", "name": "Writing", "repo": "HungGod/Writing", "ref": "main",
         "setup": "npm install", "setupHash": setup_hash("npm install")},
        {"id": "c1", "kind": "copy", "mount": "Notes", "name": "Notes"}]})
    );
    // Python's sha256 hex, first 12.
    assert_eq!(setup_hash(""), "e3b0c44298fc");
    assert_ne!(setup_hash("a"), setup_hash("b"));
    let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["projects.json"]); // no temp file left
}

// ------------------------------------------------------ folders and drives
const STICK: &str = "5E3F-1A2B";

fn folders() -> (tempfile::TempDir, ProjectStore, String) {
    let d = tempfile::tempdir().unwrap();
    let home = crate::folders::realpath(d.path()).join("home");
    std::fs::create_dir_all(home.join("wad/Notes")).unwrap();
    std::os::unix::fs::symlink(home.join("wad/Notes"), home.join("wad/link")).unwrap();
    let s = ProjectStore::new(d.path().join("projects"), vec![home.clone()], || ("m1".into(), "Surface".into()));
    (d, s, home.to_string_lossy().into_owned())
}

#[test]
fn a_folder_project_is_this_machines() {
    let (_d, s, home) = folders();
    let doc = s.put("f1", &json!({"name": "Notes", "mountName": "Notes", "source": {"kind": "folder", "path": format!("{home}/wad/link"), "machineId": ""}})).unwrap();
    // wadd says which machine, and where the folder really is.
    assert_eq!(
        doc["source"],
        json!({"kind": "folder", "machineId": "m1", "machineName": "Surface", "path": format!("{home}/wad/Notes")})
    );
    assert!(doc.get("legacy").is_none());
    for (path, msg) in [
        (format!("{home}/wad/nope"), "doesn't exist"),
        ("/etc".into(), "isn't inside"),
        (home.clone(), "isn't inside"),
        ("wad/Notes".into(), "absolute"),
    ] {
        match s.put("f2", &json!({"name": "X", "mountName": "X", "source": {"kind": "folder", "path": path}})) {
            Err(ProjectError::Invalid(m)) => assert!(m.contains(msg), "{path}: {m}"),
            other => panic!("{path}: {other:?}"),
        }
    }
}

#[test]
fn another_machines_folder_project_can_still_be_edited_here() {
    let (_d, s, _) = folders();
    let theirs = json!({"kind": "folder", "machineId": "m2", "machineName": "Desk", "path": "/var/home/wad/Elsewhere"});
    s.merge(&[json!({"id": "f9", "name": "Theirs", "mountName": "Theirs", "source": theirs, "updatedAt": 5})]);
    assert_eq!(s.get("f9").unwrap()["source"], theirs);
    let renamed = with(&s.get("f9").unwrap(), json!({"name": "Renamed"}));
    assert_eq!(s.put("f9", &renamed).unwrap()["source"], theirs);
    let resent = with(&theirs, json!({"machineId": "", "machineName": ""})); // as WadSpaces Client may send it back
    assert_eq!(s.put("f9", &with(&s.get("f9").unwrap(), json!({"source": resent}))).unwrap()["source"], theirs);
    let elsewhere = with(&theirs, json!({"path": "/var/home/wad/Other"})); // a new folder here
    assert!(
        matches!(s.put("f9", &with(&s.get("f9").unwrap(), json!({"source": elsewhere}))), Err(ProjectError::Invalid(m)) if m.contains("isn't inside"))
    );
}

#[test]
fn drive_sources() {
    let d = tempfile::tempdir().unwrap();
    let s = store(&d);
    let doc = s
        .put(
            "d1",
            &json!({"name": "Novel", "mountName": "Novel", "source": {
        "kind": "drive", "uuid": STICK, "label": "STICK", "fstype": "exfat", "subpath": "/Books//Novel/"}}),
        )
        .unwrap();
    assert_eq!(
        doc["source"],
        json!({"kind": "drive", "uuid": STICK, "label": "STICK", "fstype": "exfat", "subpath": "Books/Novel"})
    );
    for (src, msg) in [
        (json!({"uuid": "a/b"}), "uuid"),
        (json!({"uuid": STICK, "subpath": "../x"}), "inside the drive"),
        (json!({"uuid": STICK, "subpath": "a:b"}), "':'"),
        (json!({"uuid": STICK, "fstype": "ext 4"}), "fstype"),
        (json!({"uuid": STICK, "label": ["x"]}), "label"),
    ] {
        let mut src = src;
        src["kind"] = "drive".into();
        match s.put("d2", &json!({"name": "X", "mountName": "X", "source": src})) {
            Err(ProjectError::Invalid(m)) => assert!(m.contains(msg), "{msg}: {m}"),
            other => panic!("{msg}: {other:?}"),
        }
    }
}

#[test]
fn folder_projects_made_before_linking_become_this_machines() {
    let d = tempfile::tempdir().unwrap();
    let home = crate::folders::realpath(d.path()).join("home");
    std::fs::create_dir_all(home.join("Notes")).unwrap();
    let s = ProjectStore::new(d.path().join("projects"), vec![home.clone()], || (LOCAL.into(), "wadspaces".into()));
    let doc = s.put("f1", &json!({"name": "Notes", "mountName": "Notes", "source": {"kind": "folder", "path": format!("{}/Notes", home.display())}})).unwrap();
    assert_eq!(doc["source"]["machineId"], "local");
    let changes = s.changes();
    assert_eq!(s.claim_local("m1", "Surface"), ["f1"]);
    let after = s.get("f1").unwrap();
    assert_eq!((&after["source"]["machineId"], &after["source"]["machineName"]), (&json!("m1"), &json!("Surface")));
    assert!(s.changes() > changes && after["updatedAt"].as_i64() > doc["updatedAt"].as_i64());
    assert!(s.claim_local("m1", "Surface").is_empty());
}
