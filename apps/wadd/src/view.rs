//! What's on screen, and the rules for changing it: switching to a
//! workspace (at once if it's ready; otherwise the screen stays put until it
//! is), WadSpaces Client ("home"), the Super+Tab switcher (most recently used
//! first), Super+number, and sessions.
//!
//! A session is the workspaces picked for a stretch of work. A focus session
//! has a timer: until it runs out, only its picks can be shown (WadSpaces Client
//! and other workspaces wait), and the clock starts the first time a pick is
//! on screen. A free session has no timer. Sessions survive restarts
//! (session.json, the Python wadd's format).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use wad_input::Action;
use wad_proto::v1::{
    Carousel, CarouselItem, Display as Shown, Event, Kind, Phase, Session, SessionMode, View as V, ViewState, Workspace,
};
use wad_proto::{ApiError, ErrorCode};
use wad_sway::Owner;

use crate::backend::Backend;
use crate::display::{Display, WindowSink};
use crate::events::Bus;
use crate::registry::Registry;

pub const SESSION_MAX_MIN: u32 = 12 * 60;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn locked() -> ApiError {
    ApiError::new(ErrorCode::Conflict, "a focus session is on: that waits until its time is up")
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, msg)
}

/// session.json, as the Python wadd wrote it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    workspaces: Vec<String>,
    #[serde(default = "focus")]
    mode: String,
    #[serde(default)]
    started_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    minutes: Option<u32>,
    #[serde(default)]
    ends_at: Option<f64>,
    #[serde(default)]
    expired: bool,
}

fn focus() -> String {
    "focus".into()
}

impl Saved {
    fn is_focus(&self) -> bool {
        self.mode != "free"
    }

    fn to_v1(&self) -> Session {
        Session {
            mode: if self.is_focus() { SessionMode::Focus } else { SessionMode::Free },
            workspaces: self.workspaces.clone(),
            minutes: self.minutes,
            started_at: self.started_at,
            ends_at: self.ends_at,
            expired: self.expired,
        }
    }
}

struct Inner {
    view: V,
    pending: Option<String>,
    /// Most recently shown first: the switcher's order.
    mru: Vec<V>,
    carousel: Option<Carousel>,
    session: Option<Saved>,
    timer: Option<JoinHandle<()>>,
}

pub struct View {
    registry: Arc<Registry>,
    backend: Arc<dyn Backend>,
    display: Arc<dyn Display>,
    bus: Bus,
    session_file: PathBuf,
    inner: Mutex<Inner>,
    me: Weak<View>,
}

impl View {
    pub fn new(
        registry: Arc<Registry>,
        backend: Arc<dyn Backend>,
        display: Arc<dyn Display>,
        bus: Bus,
        state_dir: PathBuf,
    ) -> Arc<Self> {
        let view = Arc::new_cyclic(|me| Self {
            registry: registry.clone(),
            backend,
            display,
            bus,
            session_file: state_dir.join("session.json"),
            inner: Mutex::new(Inner {
                view: V::Home,
                pending: None,
                mru: vec![],
                carousel: None,
                session: None,
                timer: None,
            }),
            me: me.clone(),
        });
        let me = Arc::downgrade(&view);
        registry.on_ready(move |id| {
            if let Some(v) = me.upgrade() {
                v.ready(id);
            }
        });
        view.publish_view();
        view
    }

    fn me(&self) -> Arc<View> {
        self.me.upgrade().expect("the view outlives its calls")
    }

    // ------------------------------------------------------------- state
    /// The screen's logical size, for laying a desktop out at it.
    pub async fn screen(&self) -> Option<wad_proto::v1::Screen> {
        self.display.screen().await
    }

    pub fn state(&self) -> ViewState {
        let i = self.inner.lock().unwrap();
        ViewState {
            view: i.view.clone(),
            pending: i.pending.clone(),
            native_display: self.registry.windows().available(),
        }
    }

    pub fn current(&self) -> V {
        self.inner.lock().unwrap().view.clone()
    }

    fn publish_view(&self) {
        self.bus.publish(Event::View(self.state()));
    }

    fn native(&self, id: &str) -> bool {
        self.registry.workspaces().iter().any(|w| w.id == id && w.display == Shown::Host)
    }

    fn running(&self, id: &str) -> bool {
        self.registry.state(id).is_some_and(|s| s.container == "running" || s.phase == Phase::Ready)
    }

    /// Brings the window for a view forward: a native workspace's own, else
    /// the app's (the shell).
    pub async fn place(&self, v: &V) {
        if let V::Workspace(id) = v
            && self.native(id)
            && self.display.show_native(id).await
        {
            return;
        }
        self.display.show_shell().await;
    }

    fn spawn_place(&self, v: V) {
        let me = self.me();
        tokio::spawn(async move { me.place(&v).await });
    }

    /// Puts a view on screen.
    fn show(&self, v: V) {
        self.start_focus_clock(&v);
        {
            let mut i = self.inner.lock().unwrap();
            i.view = v.clone();
            i.mru.retain(|m| *m != v);
            i.mru.insert(0, v.clone());
        }
        self.publish_view();
        self.spawn_place(v);
    }

    /// A bring-up finished: show it if it was waited for.
    fn ready(&self, id: &str) {
        let due = {
            let mut i = self.inner.lock().unwrap();
            if i.pending.as_deref() == Some(id) {
                i.pending = None;
                true
            } else {
                false
            }
        };
        if due {
            self.show(V::Workspace(id.into()));
        }
    }

    // ---------------------------------------------------------- commands
    /// Shows a workspace: at once if it's ready, else once it is (the
    /// screen stays where it is meanwhile, with no loading screen).
    pub fn switch(&self, id: &str) -> Result<(), ApiError> {
        self.registry.workspace(id)?;
        if self.locked() && !self.session_ids().iter().any(|w| w == id) {
            return Err(locked());
        }
        // Streamed to a browser: back on the screen (its stream ends).
        if self.registry.is_streamed(id) {
            self.inner.lock().unwrap().pending = Some(id.into());
            self.publish_view();
            let (registry, id) = (self.registry.clone(), id.to_string());
            tokio::spawn(async move {
                if let Err(e) = registry.unstream(&id).await.and_then(|_| registry.start(&id)) {
                    tracing::warn!("{id} back on the screen: {}", e.message);
                }
            });
            return Ok(());
        }
        let st = self.registry.state(id);
        if st.is_some_and(|s| s.phase == Phase::Ready && s.container == "running") {
            self.inner.lock().unwrap().pending = None;
            self.show(V::Workspace(id.into()));
            return Ok(());
        }
        self.inner.lock().unwrap().pending = Some(id.into());
        self.publish_view();
        self.registry.start(id)
    }

    /// WadSpaces Client. Refused during a focus session's time unless `force`
    /// (nothing else is left to show).
    pub fn home(&self, force: bool) -> Result<(), ApiError> {
        if self.locked() && !force {
            return Err(locked());
        }
        self.inner.lock().unwrap().pending = None;
        self.show(V::Home);
        Ok(())
    }

    /// Stops a workspace; if it was on screen, shows the next running
    /// session pick, or (with none) WadSpaces Client.
    pub async fn stop(&self, id: &str) -> Result<(), ApiError> {
        {
            let mut i = self.inner.lock().unwrap();
            if i.pending.as_deref() == Some(id) {
                i.pending = None;
            }
        }
        self.registry.stop(id).await?;
        let shown = V::Workspace(id.into());
        let was_shown = {
            let mut i = self.inner.lock().unwrap();
            i.mru.retain(|m| *m != shown);
            i.view == shown
        };
        if was_shown {
            let next = if self.locked() {
                self.session_ids()
                    .into_iter()
                    .find(|w| w != id && self.registry.state(w).is_some_and(|s| s.container == "running"))
            } else {
                None
            };
            match next {
                Some(w) => self.switch(&w)?,
                None => self.home(true)?,
            }
        }
        Ok(())
    }

    pub async fn restart(&self, id: &str) -> Result<(), ApiError> {
        let shown = self.current() == V::Workspace(id.into());
        if shown {
            // Shown again once it answers.
            self.inner.lock().unwrap().pending = Some(id.into());
        }
        let r = self.registry.restart(id).await;
        if r.is_err() && shown {
            self.inner.lock().unwrap().pending = None;
        }
        r
    }

    // ---------------------------------------------------------- switcher
    /// What runs, in Super+N's order: the spaces (container workspaces) in
    /// the order they started, then what runs on the machine itself (apps:
    /// WadBrowsers, the terminal), likewise. During a focus session only its
    /// picks (the only ones allowed on screen).
    pub fn running_items(&self) -> Vec<Workspace> {
        let locked = self.locked();
        let picks = self.session_ids();
        let list = self.registry.workspaces();
        let up: Vec<Workspace> = self
            .registry
            .running_order()
            .into_iter()
            .filter_map(|id| list.iter().find(|w| w.id == id && w.enabled).cloned())
            // A streamed one isn't on this screen: it's in a browser.
            .filter(|w| !self.registry.is_streamed(&w.id))
            .filter(|w| self.running(&w.id) && (!locked || picks.contains(&w.id)))
            .collect();
        let (spaces, apps): (Vec<Workspace>, Vec<Workspace>) = up.into_iter().partition(|w| w.kind == Kind::Container);
        spaces.into_iter().chain(apps).collect()
    }

    /// What Super+Tab offers: WadSpaces Client (not during a focus session's
    /// time), then what runs as `running_items` orders it, each with its
    /// Super+number. A fixed strip: Super+Tab steps along it from what's on
    /// screen.
    pub fn carousel_items(&self) -> Vec<CarouselItem> {
        let mut items: Vec<CarouselItem> = vec![];
        if !self.locked() {
            items.push(CarouselItem {
                view: V::Home,
                name: "WadSpaces".into(),
                icon: None,
                preview: None,
                running: true,
                kind: "home".into(),
                number: None,
                desktop: vec![],
            });
        }
        for (n, ws) in self.running_items().into_iter().enumerate() {
            items.push(CarouselItem {
                view: V::Workspace(ws.id.clone()),
                name: ws.name.clone(),
                icon: ws.icon.as_ref().map(|_| format!("/v1/workspaces/{}/icon", ws.id)),
                preview: (ws.kind != Kind::App).then(|| format!("/v1/workspaces/{}/wallpaper", ws.id)),
                running: true,
                kind: if ws.kind == Kind::App { "app" } else { "space" }.into(),
                number: (n < 9).then_some(n as u8 + 1),
                desktop: ws.desktop.clone(),
            });
        }
        items
    }

    fn publish_carousel(&self) {
        let c =
            self.inner.lock().unwrap().carousel.clone().unwrap_or(Carousel { open: false, items: vec![], index: 0 });
        self.bus.publish(Event::Carousel(c));
    }

    /// Opens the switcher (the first step lands on the view before) or moves.
    pub fn carousel_step(&self, forward: bool) {
        let items = self.carousel_items();
        {
            let mut i = self.inner.lock().unwrap();
            match &mut i.carousel {
                Some(c) => {
                    let n = c.items.len() as u32;
                    c.index = if forward { (c.index + 1) % n } else { (c.index + n - 1) % n };
                }
                None => {
                    if items.len() < 2 {
                        return;
                    }
                    // From what's on screen, one along.
                    let n = items.len();
                    let at = items.iter().position(|it| it.view == i.view).unwrap_or(0);
                    let index = (if forward { (at + 1) % n } else { (at + n - 1) % n }) as u32;
                    // The HUD draws it above whatever is on screen.
                    i.carousel = Some(Carousel { open: true, items, index });
                }
            }
        }
        self.publish_carousel();
    }

    pub fn carousel_commit(&self) {
        let Some(picked) = self.inner.lock().unwrap().carousel.take() else { return };
        self.publish_carousel();
        let Some(item) = picked.items.get(picked.index as usize) else { return };
        let r = match &item.view {
            V::Home => self.home(false),
            V::Workspace(id) => self.switch(id),
        };
        if let Err(e) = r {
            tracing::debug!("switcher: {}", e.message);
        }
        let current = self.current();
        if current != item.view && matches!(&current, V::Workspace(id) if self.native(id)) {
            self.spawn_place(current); // still starting: back to where we were
        }
    }

    pub fn carousel_cancel(&self) {
        if self.inner.lock().unwrap().carousel.take().is_none() {
            return;
        }
        self.publish_carousel();
        let current = self.current();
        if matches!(&current, V::Workspace(id) if self.native(id)) {
            self.spawn_place(current);
        }
    }

    /// A key action from the keyboard proxy.
    pub fn on_key(&self, action: Action) {
        let r = match action {
            Action::CarouselNext => {
                self.carousel_step(true);
                Ok(())
            }
            Action::CarouselPrev => {
                self.carousel_step(false);
                Ok(())
            }
            Action::CarouselCommit => {
                self.carousel_commit();
                Ok(())
            }
            Action::CarouselCancel => {
                self.carousel_cancel();
                Ok(())
            }
            Action::Bound(b) if b == "home" => self.home(false),
            // The HUD's to show (it holds the history), during focus time too.
            Action::Bound(b) if b == "clipboard" => {
                self.bus.publish(Event::Shortcut { name: b });
                Ok(())
            }
            // Super+N: the n-th of what runs, if there's one there.
            Action::Bound(b) => match b.strip_prefix("switch:running:").and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => match self.running_items().get(n.wrapping_sub(1)) {
                    Some(ws) => self.switch(&ws.id),
                    None => Ok(()),
                },
                None => Ok(()),
            },
        };
        if let Err(e) = r {
            tracing::debug!("key: {}", e.message);
        }
    }

    // ----------------------------------------------------------- session
    /// A focus session whose time isn't up: only its picks can be shown.
    pub fn locked(&self) -> bool {
        self.inner.lock().unwrap().session.as_ref().is_some_and(|s| s.is_focus() && !s.expired)
    }

    fn session_ids(&self) -> Vec<String> {
        self.inner.lock().unwrap().session.as_ref().map(|s| s.workspaces.clone()).unwrap_or_default()
    }

    pub fn session(&self) -> Option<Session> {
        self.inner.lock().unwrap().session.as_ref().map(Saved::to_v1)
    }

    fn save_session(&self) {
        let s = self.inner.lock().unwrap().session.clone();
        let r = match s {
            None => match std::fs::remove_file(&self.session_file) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                r => r,
            },
            Some(s) => (|| {
                if let Some(d) = self.session_file.parent() {
                    std::fs::create_dir_all(d)?;
                }
                let tmp = self.session_file.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec(&s)?)?;
                std::fs::rename(&tmp, &self.session_file)
            })(),
        };
        if let Err(e) = r {
            tracing::warn!("couldn't save the session: {e}");
        }
    }

    fn publish_session(&self) {
        self.bus.publish(Event::Session(self.session()));
    }

    /// Starts a session: brings up the picks and shows WadSpaces Client's landing
    /// page (the picks are a Super+Tab away). With `minutes`, a focus session.
    pub fn session_begin(&self, ids: &[String], minutes: Option<u32>) -> Result<Session, ApiError> {
        self.session_start(ids, minutes, true)
    }

    /// session_begin; without `start` the picks are left to the caller,
    /// which launches them with their projects.
    pub fn session_start(&self, ids: &[String], minutes: Option<u32>, start: bool) -> Result<Session, ApiError> {
        if self.locked() {
            return Err(locked());
        }
        let mut picks: Vec<String> = vec![];
        for id in ids {
            if !picks.contains(id) {
                picks.push(id.clone());
            }
        }
        if picks.is_empty() {
            return Err(bad("pick at least one workspace"));
        }
        for id in &picks {
            self.registry.workspace(id)?;
        }
        if let Some(m) = minutes
            && !(1..=SESSION_MAX_MIN).contains(&m)
        {
            return Err(bad(format!("minutes must be 1..{SESSION_MAX_MIN}")));
        }
        let mode = if minutes.is_some() { "focus" } else { "free" };
        tracing::info!(
            "{mode} session: {}{}",
            picks.join(", "),
            minutes.map(|m| format!(" for {m} min, from when one is first shown")).unwrap_or_default()
        );
        {
            let mut i = self.inner.lock().unwrap();
            if let Some(t) = i.timer.take() {
                t.abort();
            }
            i.session = Some(Saved {
                workspaces: picks.clone(),
                mode: mode.into(),
                started_at: now(),
                minutes,
                ends_at: None,
                expired: false,
            });
        }
        self.save_session();
        self.publish_session();
        if start {
            for id in &picks {
                self.registry.start(id)?;
            }
        }
        self.home(true)?;
        Ok(self.session().expect("just begun"))
    }

    /// The clock starts the first time one of a focus session's picks is on
    /// screen, not when it was set up.
    fn start_focus_clock(&self, v: &V) {
        let started = {
            let mut i = self.inner.lock().unwrap();
            match (&mut i.session, v) {
                (Some(s), V::Workspace(id)) if s.is_focus() && s.ends_at.is_none() && s.workspaces.contains(id) => {
                    let minutes = s.minutes.unwrap_or(25);
                    s.ends_at = Some(now() + f64::from(minutes) * 60.0);
                    Some(minutes)
                }
                _ => None,
            }
        };
        if let Some(m) = started {
            tracing::info!("focus session: the clock is running ({m} min)");
            self.save_session();
            self.publish_session();
            self.arm_timer();
        }
    }

    fn arm_timer(&self) {
        let me = self.me.clone();
        let task = tokio::spawn(async move {
            loop {
                let Some(v) = me.upgrade() else { return };
                let Some(left) = v.tick_session() else { return };
                drop(v);
                tokio::time::sleep(left.min(Duration::from_secs(60))).await;
            }
        });
        if let Some(old) = self.inner.lock().unwrap().timer.replace(task) {
            old.abort();
        }
    }

    /// Expires the session if its time is up; else the time left (None:
    /// nothing to time).
    pub fn tick_session(&self) -> Option<Duration> {
        let ends = {
            let i = self.inner.lock().unwrap();
            let s = i.session.as_ref()?;
            if s.expired {
                return None;
            }
            s.ends_at?
        };
        let left = ends - now();
        if left <= 0.0 {
            self.expire_session();
            return None;
        }
        Some(Duration::from_secs_f64(left))
    }

    /// Time's up: WadSpaces Client joins Super+Tab. The picks stay until a new
    /// session begins.
    pub fn expire_session(&self) {
        {
            let mut i = self.inner.lock().unwrap();
            match &mut i.session {
                Some(s) if !s.expired => s.expired = true,
                _ => return,
            }
        }
        tracing::info!("focus session: time is up");
        self.save_session();
        self.publish_session();
        self.bus.publish(Event::Notice { text: "Time's up: WadSpaces is back in Super+Tab".into() });
    }

    /// Ends the session. Not during focus time unless `force` (the user chose
    /// to end it early).
    pub fn session_end(&self, force: bool) -> Result<(), ApiError> {
        if self.locked() {
            if !force {
                return Err(locked());
            }
            tracing::info!("focus session ended early");
        }
        {
            let mut i = self.inner.lock().unwrap();
            if let Some(t) = i.timer.take() {
                t.abort();
            }
            i.session = None;
        }
        self.save_session();
        self.publish_session();
        Ok(())
    }

    /// After a restart (or reboot) mid-session: picks up where it was.
    pub fn restore_session(&self) {
        let text = match std::fs::read_to_string(&self.session_file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!("ignoring the saved session: {e}");
                return;
            }
        };
        let mut saved: Saved = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("ignoring the saved session: {e}");
                return;
            }
        };
        let known: Vec<String> = self.registry.workspaces().into_iter().map(|w| w.id).collect();
        saved.workspaces.retain(|w| known.contains(w));
        if saved.workspaces.is_empty() {
            self.inner.lock().unwrap().session = None;
            self.save_session();
            return;
        }
        let how = match saved.ends_at {
            None if !saved.is_focus() => "no timer".to_string(),
            None => "clock not started yet".to_string(),
            Some(e) if now() >= e => {
                saved.expired = true;
                "time is up".to_string()
            }
            Some(e) => format!("{} min left", ((e - now()) / 60.0) as u64),
        };
        tracing::info!("resumed session: {} ({how})", saved.workspaces.join(", "));
        let timed = saved.ends_at.is_some() && !saved.expired;
        self.inner.lock().unwrap().session = Some(saved);
        if timed {
            self.arm_timer();
        }
        self.publish_session();
    }

    /// Super+key bindings: WadSpaces Client's keys, the clipboard history's, and
    /// Super+1..9 for the 1st..9th of what runs (resolved when pressed).
    pub fn bindings(&self, keys: &wad_config::Keys) -> std::collections::HashMap<u16, String> {
        let mut b = std::collections::HashMap::new();
        for (list, name) in [(&keys.home, "home"), (&keys.clipboard, "clipboard")] {
            for k in list {
                match wad_input::keys::code(k) {
                    Ok(c) => {
                        b.insert(c, name.to_string());
                    }
                    Err(e) => tracing::warn!("keys.{name}: {e}"),
                }
            }
        }
        for n in 1..=9u8 {
            if let Ok(c) = wad_input::keys::code(&n.to_string()) {
                b.insert(c, format!("switch:running:{n}"));
            }
        }
        b
    }
}

#[async_trait]
impl WindowSink for View {
    async fn resolve(&self, owner: Owner) -> Option<String> {
        let id = match owner {
            Owner::Workspace(id) => id,
            Owner::Container(c) => self.backend.workspace_of(&c).await?,
            // The app's window (and anything else run in the session) stays
            // where sway put it.
            Owner::Exe(_) => return None,
        };
        self.registry.workspace(&id).ok().map(|w| w.id)
    }

    fn on_window(&self, id: &str, present: bool) {
        // Only an app that was up: while it starts, stops or restarts, its
        // window coming and going is already in hand.
        let was_ready = self.registry.state(id).is_some_and(|s| s.phase == Phase::Ready);
        self.registry.window(id, present);
        if present && self.current() == V::Workspace(id.into()) {
            self.spawn_place(V::Workspace(id.into())); // it restarted while on screen
        }
        // An app whose last window closed (`exit` in the terminal) is done:
        // stopped, and off the screen if it was on it (a workspace's desktop
        // may come back, an app's doesn't).
        let app = self.registry.workspace(id).is_ok_and(|w| w.kind == wad_proto::v1::Kind::App);
        if !present && app && was_ready {
            let (me, id) = (self.me(), id.to_string());
            tokio::spawn(async move {
                if let Err(e) = me.stop(&id).await {
                    tracing::warn!("stopping {id} after its window closed: {e}");
                }
            });
        }
        self.publish_view();
    }
}
