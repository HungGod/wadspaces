//! The checks a workspace list goes through, and the YAML shape it's given in.

use serde_json::{Value, json};
use wad_proto::v1::Display;

use super::*;

fn one(w: Value) -> Result<Workspace, Error> {
    check_workspaces(vec![w]).map(|mut l| l.remove(0))
}

#[test]
fn a_container_with_its_defaults() {
    let w = one(json!({"id": "writing", "name": "Writing", "image": "localhost/wadspaces-template-writing:latest",
        "display": "host", "env": {"PUID": 1000, "DEBUG": true}, "secrets": ["github_token"],
        "volumes": ["wad-writing-config:/config:z"], "devices": ["/dev/dri"]}))
    .unwrap();
    assert_eq!(w.container_name, "wad-writing");
    assert_eq!((w.container_port, w.shm_size.as_deref(), w.enabled, w.autostart), (3000, Some("1g"), true, false));
    assert_eq!(w.env, [("PUID".to_string(), "1000".to_string()), ("DEBUG".to_string(), "True".to_string())]);
    assert_eq!(w.display, Display::Host);
    // A stream is the default display, and needs a port.
    let s = one(json!({"id": "s", "name": "S", "image": "i", "port": 3100, "shm_size": null})).unwrap();
    assert_eq!((s.display, s.port, s.shm_size), (Display::Stream, Some(3100), None));
}

#[test]
fn workspaces_round_trip_through_the_yaml_shape() {
    let list = check_workspaces(vec![
        json!({"id": "a", "name": "A", "image": "i", "port": 3100, "hotkey": 2, "icon": "/x.png", "autostart": true,
            "env": {"TZ": "Pacific/Fiji"}, "projects": [{"id": "p1", "mount": "Notes"}, {"id": "p2", "mount": "Code", "path": "/home/wad/Code"}],
            "signins": [{"id": "gh", "app": "github", "web": true, "slot": "web:github"}]}),
        json!({"id": "b", "name": "B", "image": "i", "display": "host", "enabled": false, "ephemeral": true}),
        json!({"id": "t", "name": "Terminal", "kind": "app", "exec": ["/usr/bin/foot"]}),
    ])
    .unwrap();
    let again = check_workspaces(list.iter().map(to_yaml).collect()).unwrap();
    assert_eq!(again, list);
    // Checked together: a second workspace on the same port is refused.
    let mut two = to_yaml(&list[0]);
    two["id"] = "c".into();
    two["container_name"] = "wad-c".into();
    let e = check_workspaces(vec![to_yaml(&list[0]), two]).unwrap_err();
    assert!(e.to_string().contains("port 3100 already used"), "{e}");
}

#[test]
fn refusals_say_where() {
    let a = |extra: Value| {
        let mut w = json!({"id": "a", "name": "A", "image": "i", "display": "host"});
        w.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        w
    };
    for (list, why) in [
        (vec![a(json!({"colour": "red"}))], "workspaces[0]: unknown keys [\"colour\"]"),
        (vec![a(json!({"display": "stream"}))], "workspaces[0]: missing port"),
        (vec![a(json!({"display": "tv"}))], "workspaces[0]: display must be one of"),
        (vec![a(json!({"id": "A_1"}))], "workspace 'A_1': id must match"),
        (vec![a(json!({})), a(json!({}))], "workspace 'a': duplicate id"),
        (
            vec![a(json!({"id": "b"})), a(json!({"id": "c", "container_name": "wad-b"}))],
            "workspace 'c': duplicate container_name",
        ),
        (vec![a(json!({"display": "stream", "port": 70000}))], "workspace 'a': port out of range"),
        (vec![a(json!({"hotkey": 12}))], "workspace 'a': hotkey must be 1..9"),
        (vec![a(json!({"projects": [{"id": "p"}]}))], "workspace 'a': a project is {id, kind, mount"),
        (
            vec![a(json!({"projects": [{"id": "p", "kind": "github", "mount": "M", "repo": "nope"}]}))],
            "isn't owner/name",
        ),
        (
            vec![a(json!({"projects": [{"id": "p", "kind": "copy", "mount": "M", "from": {"paths": ["rel/x"]}}]}))],
            "comes from paths that aren't",
        ),
        (vec![a(json!({"projects": [{"id": "p", "kind": "mount", "mount": "M"}]}))], "M mounts one folder or file"),
        (
            vec![a(
                json!({"projects": [{"id": "p", "kind": "mount", "mount": "M", "from": {"paths": ["/var/home/a", "/var/home/b"]}}]}),
            )],
            "M mounts one folder or file",
        ),
        (
            vec![a(json!({"projects": [{"id": "p", "kind": "mount", "mount": "M", "from": {"paths": ["rel/x"]}}]}))],
            "comes from paths that aren't",
        ),
        (
            vec![a(
                json!({"projects": [{"id": "p", "kind": "copy", "mount": "M"}, {"id": "p", "kind": "copy", "mount": "N"}]}),
            )],
            "project p is listed twice",
        ),
        (
            vec![a(
                json!({"projects": [{"id": "p", "kind": "copy", "mount": "M"}, {"id": "q", "kind": "copy", "mount": "M"}]}),
            )],
            "two projects are at Desktop/M",
        ),
        (
            vec![a(json!({"signins": [{"id": "x", "app": "github"}, {"id": "y", "app": "github"}]}))],
            "github has two sign-ins",
        ),
        (vec![a(json!({"name": "Line\nbreak"}))], "name has a line break"),
    ] {
        let e = check_workspaces(list).unwrap_err().to_string();
        assert!(e.contains(why), "{e} (wanted {why})");
    }
    // What a launch used to mount ({id, mount, path?}) is dropped, not refused.
    assert!(
        check_workspaces(vec![a(json!({"projects": [{"id": "p", "mount": "M", "path": "/x"}]}))]).unwrap()[0]
            .projects
            .is_empty()
    );
    // A mount: one path on a machine, or on a drive.
    let m = |from: Value| a(json!({"projects": [{"id": "p", "kind": "mount", "mount": "M", "from": from}]}));
    let ok = check_workspaces(vec![m(json!({"machineId": "m1", "paths": ["/var/home/wad/Thesis"]}))]).unwrap();
    assert_eq!(ok[0].projects[0].kind, wad_proto::v1::ProjectKind::Mount);
    assert!(check_workspaces(vec![m(json!({"drive": {"uuid": "1234-ABCD"}, "paths": ["2026"]}))]).is_ok());
    // Two may name the same hotkey: Super+N follows what runs.
    assert!(check_workspaces(vec![a(json!({"hotkey": 1})), a(json!({"id": "b", "hotkey": 1}))]).is_ok());
}

#[test]
fn an_app_is_a_program_not_an_image() {
    let app = json!({"id": "terminal", "name": "Terminal", "kind": "app", "exec": ["/usr/bin/foot"]});
    let w = &check_workspaces(vec![app.clone()]).unwrap()[0];
    assert_eq!(w.kind, wad_proto::v1::Kind::App);
    assert_eq!(w.exec, ["/usr/bin/foot"]);
    assert_eq!(w.display, Display::Host);
    assert_eq!(w.image, "");
    // Round trip, and a container's shape is unchanged (no kind, no exec).
    assert_eq!(&check_workspaces(vec![to_yaml(w)]).unwrap()[0], w);
    let c = &check_workspaces(vec![json!({"id": "c", "name": "C", "image": "i", "display": "host"})]).unwrap()[0];
    assert!(to_yaml(c).get("kind").is_none());
    assert!(!serde_json::to_string(c).unwrap().contains("exec"));

    for (bad, why) in [
        (json!({"id": "t", "name": "T", "kind": "app"}), "missing exec"),
        (json!({"id": "t", "name": "T", "kind": "app", "exec": []}), "exec must name a program"),
        (json!({"id": "t", "name": "T", "kind": "app", "exec": ["x"], "image": "i"}), "an app has no image"),
        (json!({"id": "t", "name": "T", "kind": "app", "exec": ["x"], "volumes": ["a:/b"]}), "an app has no volumes"),
        (
            json!({"id": "t", "name": "T", "kind": "app", "exec": ["x"], "display": "stream", "port": 3100}),
            "display must be 'host'",
        ),
        (json!({"id": "t", "name": "T", "kind": "vm", "exec": ["x"]}), "kind must be one of"),
        (json!({"id": "t", "name": "T", "image": "i", "exec": ["x"]}), "only an app has exec"),
        (json!({"id": "t", "name": "T", "kind": "app", "exec": ["x\ny"]}), "control character"),
    ] {
        let e = check_workspaces(vec![bad]).unwrap_err().to_string();
        assert!(e.contains(why), "{e} (wanted {why})");
    }
}
