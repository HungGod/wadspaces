use wad_proto::v1::{Display, Kind, ProjectKind, WorkspaceProject};

use super::*;

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

const WRITING: &str = r#"
name = "Writing"
image = "ghcr.io/hunggod/wadspaces-cosmic-bodybuilding:latest"
display = "host"
hotkey = 1
secrets = ["github_token"]
devices = ["/dev/dri"]
[env]
PUID = "1000"
TZ = "Pacific/Fiji"
"#;

#[test]
fn reads_the_images_workspaces_with_the_yaml_defaults() {
    let d = tempfile::tempdir().unwrap();
    assert!(read(&d.path().join("none")).unwrap().is_empty());
    write(d.path(), "writing.toml", WRITING);
    write(d.path(), "notes.txt", "ignored");
    let v = read(d.path()).unwrap();
    let w = &v[0].workspace;
    assert_eq!(
        (w.id.as_str(), w.display, w.hotkey, w.container_name.as_str()),
        ("writing", Display::Host, Some(1), "wad-writing")
    );
    assert_eq!(w.env, [("PUID".to_string(), "1000".to_string()), ("TZ".into(), "Pacific/Fiji".into())]);
    assert_eq!((w.enabled, w.shm_size.as_deref()), (true, Some("1g")));
    // The same workspace, the same digest; a change, another.
    assert_eq!(read(d.path()).unwrap()[0].digest, v[0].digest);
    write(d.path(), "writing.toml", &WRITING.replace("hotkey = 1", "hotkey = 2"));
    assert_ne!(read(d.path()).unwrap()[0].digest, v[0].digest);
}

#[test]
fn refuses_a_bad_set() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "writing.toml", &format!("id = \"other\"\n{WRITING}"));
    assert!(read(d.path()).unwrap_err().to_string().contains("isn't the file's name"));
    write(d.path(), "writing.toml", WRITING);
    // Streamed, with no port: not a workspace.
    write(d.path(), "second.toml", &WRITING.replace("display = \"host\"", "display = \"stream\""));
    assert!(read(d.path()).is_err());
}

#[test]
fn applies_what_is_new_or_changed_and_keeps_the_machines_projects() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "writing.toml", WRITING);
    let vendor = read(d.path()).unwrap();
    let (mut list, mut book) = (vec![], Book::new());
    assert_eq!(apply(&mut list, &mut book, &vendor).changed, ["writing"]);
    assert_eq!(list.len(), 1);
    // Applied once: the machine's changes stay.
    list[0].name = "My writing".into();
    list[0].projects = vec![WorkspaceProject {
        id: "p1".into(),
        kind: ProjectKind::Github,
        mount: "notes".into(),
        repo: "o/notes".into(),
        git_ref: None,
        setup: String::new(),
        from: None,
    }];
    assert_eq!(apply(&mut list, &mut book, &vendor), Applied::default());
    assert_eq!(list[0].name, "My writing");
    // Deleted here: stays deleted.
    let kept = list.clone();
    list.clear();
    assert_eq!(apply(&mut list, &mut book, &vendor), Applied::default());
    assert!(list.is_empty());
    // The image changes it: applied over the machine's, keeping its projects.
    list = kept;
    write(d.path(), "writing.toml", &WRITING.replace("cosmic-bodybuilding:latest", "cosmic-bodybuilding:v2"));
    let vendor = read(d.path()).unwrap();
    assert_eq!(apply(&mut list, &mut book, &vendor).changed, ["writing"]);
    assert_eq!(
        (list[0].name.as_str(), list[0].image.as_str()),
        ("Writing", "ghcr.io/hunggod/wadspaces-cosmic-bodybuilding:v2")
    );
    assert_eq!(list[0].projects[0].id, "p1");
    // Saved and read back.
    write_book(d.path(), &book).unwrap();
    assert_eq!(read_book(d.path()), book);
}

#[test]
fn a_clash_with_the_machines_workspaces_is_left_out_and_retried() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "writing.toml", WRITING);
    let vendor = read(d.path()).unwrap();
    // A workspace built here already has its container's name.
    let mut mine = vendor[0].workspace.clone();
    mine.id = "mine".into();
    mine.name = "Mine".into();
    let (mut list, mut book) = (vec![mine], Book::new());
    let a = apply(&mut list, &mut book, &vendor);
    assert_eq!((a.changed.len(), a.refused.len()), (0, 1));
    assert!(book.is_empty() && list.len() == 1);
    list[0].container_name = "wad-mine".into();
    assert_eq!(apply(&mut list, &mut book, &vendor).changed, ["writing"]);
}

/// The image's own workspaces (host/usr/lib/wadspaces/workspaces.d) read
/// and check: the machine's apps alone (wadspaces are the Builder's now).
#[test]
fn the_images_workspaces_are_valid() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../host/usr/lib/wadspaces/workspaces.d");
    let v = read(&dir).unwrap();
    let ids: Vec<&str> = v.iter().map(|w| w.workspace.id.as_str()).collect();
    assert_eq!(ids, ["browser-focus", "browser", "terminal"]);
    for w in &v {
        let w = &w.workspace;
        assert_eq!(w.display, Display::Host, "{}", w.id);
        // The apps: no Super+number, and a program to run.
        if w.kind == Kind::App {
            assert_eq!(w.hotkey, None, "{}", w.id);
            assert!(w.exec[0].starts_with('/'), "{}", w.id);
        }
        let icon = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../host")
            .join(w.icon.as_deref().unwrap().trim_start_matches('/'));
        assert!(icon.is_file(), "{}", icon.display());
    }
}

#[test]
fn a_line_break_in_a_value_is_refused() {
    for raw in [
        serde_json::json!({"id": "w", "name": "W\n[Service]", "image": "i", "display": "host"}),
        serde_json::json!({"id": "w", "name": "W", "image": "i", "display": "host", "env": {"A": "1\nB=2"}}),
        serde_json::json!({"id": "w", "name": "W", "image": "i", "display": "host", "volumes": ["a:/b\r"]}),
    ] {
        let e = crate::workspaces::check_workspaces(vec![raw]).unwrap_err().to_string();
        assert!(e.contains("control character"), "{e}");
    }
    let ok = serde_json::json!({"id": "w", "name": "Wad Creator – dev", "image": "i", "display": "host"});
    assert!(crate::workspaces::check_workspaces(vec![ok]).is_ok());
}

#[test]
fn the_machines_own_apps_come_back_when_lost_and_wadspaces_dont() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "terminal.toml", "name = \"WadTerminal\"\nkind = \"app\"\nexec = [\"/usr/bin/foot\"]\n");
    write(d.path(), "writing.toml", WRITING);
    let v = read(d.path()).unwrap();
    let (mut list, mut book) = (vec![], Book::default());
    apply(&mut list, &mut book, &v);
    assert_eq!(list.len(), 2);
    // Both gone from the machine's list (however that happened).
    list.clear();
    let a = apply(&mut list, &mut book, &v);
    // The terminal is the system's: back. A wadspace removed stays removed.
    assert_eq!(a.changed, ["terminal"]);
    assert_eq!(list.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["terminal"]);
    // And then it's there: nothing to do.
    assert!(apply(&mut list, &mut book, &v).changed.is_empty());
}

#[test]
fn an_app_the_image_stops_shipping_goes_and_a_wadspace_stays() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "terminal.toml", "name = \"WadTerminal\"\nkind = \"app\"\nexec = [\"/usr/bin/foot\"]\n");
    write(d.path(), "browser-focus.toml", "name = \"Focus\"\nkind = \"app\"\nexec = [\"/usr/bin/wadbrowser\"]\n");
    write(d.path(), "writing.toml", WRITING);
    let (mut list, mut book) = (vec![], Book::default());
    apply(&mut list, &mut book, &read(d.path()).unwrap());
    assert_eq!(list.len(), 3);
    // The next image has neither the Focus app nor the writing wadspace.
    std::fs::remove_file(d.path().join("browser-focus.toml")).unwrap();
    std::fs::remove_file(d.path().join("writing.toml")).unwrap();
    let a = apply(&mut list, &mut book, &read(d.path()).unwrap());
    assert_eq!(a.changed, ["browser-focus"]);
    let ids: Vec<&str> = list.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(!ids.contains(&"browser-focus") && ids.contains(&"writing") && ids.contains(&"terminal"));
    assert!(!book.contains_key("browser-focus"));
}
