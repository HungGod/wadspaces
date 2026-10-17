//! A wadspace, streamed: three units that fit together.

use serde_json::{Value, json};
use wad_core::generator::{quadlet, stream_quadlets};

fn writing() -> Value {
    json!({"id": "writing", "name": "Cosmic Writing", "image": "localhost/wadspaces-template-writing:latest",
        "display": "host", "env": {"PUID": "1000", "PGID": "1000", "TZ": "Pacific/Fiji", "GIT_USER_NAME": "x"},
        "secrets": ["github_token"], "devices": ["/dev/dri"], "shm_size": "1g",
        "projects": [{"id": "p1", "kind": "copy", "mount": "notes"}]})
}

fn stream(webrtc: bool) -> Value {
    json!({"guid": "0123456789abcdef0123456789abcdef", "web_port": 48100, "mux_port": 48200,
        "user": "wad-abcdef", "image": "localhost/wadspaces-stream:trixie", "webrtc": webrtc,
        "turn_token_id": "turn-id"})
}

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/quadlet").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn a_streamed_wadspace_draws_on_its_sidecar() {
    let units = stream_quadlets(&writing(), &stream(false), "/var/lib/wadspaces", None);
    let names: Vec<&str> = units.iter().map(|u| u.0.as_str()).collect();
    assert_eq!(names, ["wad-writing.container", "wad-writing-display.container", "wad-writing-display.volume"]);
    assert_eq!(units[0].1, fixture("wad-writing-stream.container"));
    assert_eq!(units[1].1, fixture("wad-writing-stream-display.container"));
    assert_eq!(units[2].1, fixture("wad-writing-stream-display.volume"));
    let (ws, display) = (&units[0].1, &units[1].1);

    // The wadspace: on the sidecar's display, never the screen, going with
    // it; everything else as on the screen.
    assert!(ws.contains("BindsTo=wad-writing-display.service\nAfter=wad-writing-display.service"));
    assert!(!ws.contains("/run/user/1000") && !ws.contains("SecurityLabelDisable") && !ws.contains("PublishPort"));
    let screen = quadlet(&writing(), "/var/lib/wadspaces", None);
    let strip = |u: &str| {
        u.lines()
            .filter(|l| !l.contains("display") && !l.contains("SecurityLabel"))
            .map(String::from)
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(ws), strip(&screen));

    // The sidecar: on loopback only, behind a login it can't start without,
    // WebSockets only.
    assert_eq!(display.matches("PublishPort").count(), 1);
    assert!(display.contains("PublishPort=127.0.0.1:48100:3000"));
    assert!(display.contains("Secret=wad_stream_writing,type=env,target=PASSWORD"));
    assert!(display.contains("Environment=\"TITLE=Cosmic Writing\""));
    assert!(!display.contains("GIT_USER_NAME") && !display.contains("github_token"));
    assert!(!display.contains("STUN") && !display.contains("TURN") && !display.contains("MUX"));
    assert!(!display.contains("PUBLIC_IP") && !display.contains("ICE_LITE"));

    // Rootless: both containers mapped the same way.
    let units = stream_quadlets(&writing(), &stream(false), "/s", Some(1000));
    assert!(units[0].1.contains("UIDMap=") && units[1].1.contains("UIDMap="));
}

#[test]
fn webrtc_only_when_asked() {
    let units = stream_quadlets(&writing(), &stream(true), "/var/lib/wadspaces", None);
    assert_eq!(units[1].1, fixture("wad-writing-stream-display-webrtc.container"));
    let display = &units[1].1;
    for l in [
        "Environment=SELKIES_MODE=websockets",
        "Environment=SELKIES_ENABLE_DUAL_MODE=true",
        "Environment=SELKIES_WEBRTC_UDP_MUX_PORT=48200",
        "Environment=SELKIES_WEBRTC_TCP_MUX_PORT=48200",
        "Environment=SELKIES_STUN_HOST=stun.cloudflare.com",
        "Environment=SELKIES_STUN_PORT=3478",
        "Environment=SELKIES_ENABLE_CLOUDFLARE_TURN=true",
        "Environment=SELKIES_CLOUDFLARE_TURN_TOKEN_ID=turn-id",
        "Secret=wad_turn_api_token,type=env,target=SELKIES_CLOUDFLARE_TURN_API_TOKEN",
        "PublishPort=48200:48200/udp",
        "PublishPort=48200:48200/tcp",
    ] {
        assert!(display.contains(l), "{l}\n{display}");
    }
    assert!(!display.contains("PUBLIC_IP") && !display.contains("ICE_LITE"));
}

/// A value can't add lines to a unit (a name from someone else's design).
#[test]
fn values_stay_on_their_line() {
    let evil = "Writing\n[Service]\nExecStartPre=/bin/sh -c 'id > /tmp/owned'";
    let w = json!({"id": "w", "name": evil, "image": "i", "display": "host",
        "env": {"TZ": "x\r\nExecStart=/bin/false"}, "devices": ["/dev/dri"]});
    let mut s = stream(true);
    s["user"] = json!("u\nExecStart=/bin/false");
    let mut units = vec![("q".to_string(), quadlet(&w, "/s", None))];
    units.extend(stream_quadlets(&w, &s, "/s", None));
    for (name, text) in units {
        assert!(!text.lines().any(|l| l.starts_with("ExecStart")), "{name}:\n{text}");
        assert_eq!(
            text.lines().filter(|l| *l == "[Service]").count(),
            usize::from(!name.ends_with(".volume")),
            "{name}:\n{text}"
        );
    }
}
