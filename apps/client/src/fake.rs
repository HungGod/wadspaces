//! A made-up machine and account (WADSPACES_FAKE=fixtures/desktop/sample.json):
//! the app's pages with something in them, for snapshots and trying the UI
//! without a wadd or a sign-in.

use std::cell::RefCell;

use serde_json::Value;

use crate::store::{ProfileState, Topic, store};

thread_local! {
    /// What the made-up machine's wadd answers, by path (its "wadd").
    static WADD: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// The app runs on a made-up machine.
pub fn on() -> bool {
    WADD.with(|w| w.borrow().is_some())
}

/// The made-up wadd's answer to a GET: by the whole path, else without its
/// query.
pub fn answer(path: &str) -> Option<Value> {
    WADD.with(|w| {
        let w = w.borrow();
        let w = w.as_ref()?;
        w.get(path).or_else(|| w.get(path.split('?').next().unwrap_or(path))).cloned()
    })
}

pub fn load(path: &std::path::Path) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            return;
        }
    };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    WADD.with(|w| w.replace(Some(v.get("wadd").cloned().unwrap_or_else(|| serde_json::json!({})))));
    fn take<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Result<T, serde_json::Error> {
        serde_json::from_value(v[k].clone())
    }
    store().update(|s| {
        s.connected = true;
        s.machine = take(&v, "machine").ok();
        s.workspaces = take(&v, "workspaces").unwrap_or_default();
        let states: Vec<wad_proto::v1::WorkspaceState> = take(&v, "states").unwrap_or_default();
        s.states = states.into_iter().map(|st| (st.id.clone(), st)).collect();
        s.library = take(&v, "library").ok();
        s.session = take(&v, "session").ok().flatten();
        s.view = take(&v, "view").ok();
        s.cloud = take(&v, "cloud").ok();
        #[derive(serde::Deserialize)]
        struct B {
            build: wad_proto::v1::Build,
            lines: Vec<String>,
        }
        #[derive(serde::Deserialize)]
        struct L {
            launch: wad_proto::v1::Launch,
            lines: Vec<String>,
        }
        s.builds = take::<Vec<B>>(&v, "builds").unwrap_or_default().into_iter().map(|b| (b.build, b.lines)).collect();
        s.launches =
            take::<Vec<L>>(&v, "launches").unwrap_or_default().into_iter().map(|l| (l.launch, l.lines)).collect();
        let wadd = &v["wadd"];
        s.streams = take(wadd, "/v1/streams").unwrap_or_default();
        s.tunnel = take(wadd, "/v1/tunnel").ok();
        s.tailnet = take(wadd, "/v1/tailnet").ok();
        s.account.checked = true;
        s.account.user = take(&v, "user").ok();
        s.account.github = Some(true);
        let p = &v["profile"];
        s.account.profile = ProfileState::Have(wad_account::Profile {
            username: p["username"].as_str().unwrap_or("me").into(),
            display_name: p["displayName"].as_str().unwrap_or("Me").into(),
            color: p["color"].as_str().unwrap_or("#c6ff1f").into(),
            photo_url: None,
            onboarded: true,
            starters_created: true,
            last_session: v.get("lastSession").cloned(),
        });
        Topic::ACCOUNT
            | Topic::LIBRARY
            | Topic::MACHINE
            | Topic::WORKSPACES
            | Topic::STATES
            | Topic::WADD
            | Topic::SESSION
            | Topic::ACTIVITY
            | Topic::CLOUD
            | Topic::STREAMS
            | Topic::TAILNET
    });
}
