//! Machines (the web app's pages/Machines.tsx): this machine and the
//! account's others, each with what runs there; this one with its run
//! history, what it takes on its disk, its files and its diagnostics too.
//! This machine is wadd's; the others are their heartbeats (read again
//! every 30 s while the page shows), and what's done to them goes through
//! their relays.

mod devices;
mod diagnostics;
mod files;
mod history;
mod storage;
pub mod tailnet;
mod tree;
pub mod wadspaces;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde::de::DeserializeOwned;
use serde_json::Value;
use wad_gtk::Icon;
use wad_wadd_client::{Method, Wadd};

use super::{badge, now, secs_of, time_ago};
use crate::rt;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

/// This machine's id on the page (the web app's THIS_MACHINE).
pub const THIS: &str = "local";
/// Heartbeats older than this: the machine's offline.
const ONLINE_WITHIN: f64 = 90.0;

/// A wadspace's container on a machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub running: bool,
    /// Streamed (not drawn on the machine's screen).
    pub stream: bool,
    /// When its phase last changed (a heartbeat: when it was sent).
    pub since: f64,
    /// idle, starting, waiting, ready, stopping, error.
    pub phase: String,
    pub error: Option<String>,
    pub on_screen: bool,
    pub hotkey: Option<u64>,
}

impl Container {
    /// What it's doing, in words (the web app's phaseLabel).
    pub fn phase_label(&self) -> &'static str {
        if self.on_screen {
            return "On screen";
        }
        match self.phase.as_str() {
            "idle" if self.running => "Running",
            "idle" => "Stopped",
            "starting" => "Starting",
            "waiting" => "Waiting for its desktop",
            "ready" => "Running",
            "stopping" => "Stopping",
            "error" => "Error",
            _ if self.running => "Running",
            _ => "Stopped",
        }
    }

    /// Its dot: error, on, busy or (stopped) none.
    pub fn dot(&self) -> &'static str {
        if self.phase == "error" {
            "error"
        } else if self.running {
            "on"
        } else if self.phase != "idle" {
            "busy"
        } else {
            "off"
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    pub id: String,
    /// Its hostname.
    pub name: String,
    pub label: String,
    pub os: String,
    pub up: bool,
    pub here: bool,
    pub linked: bool,
    /// Load in percent, once it says.
    pub cpu: Option<f64>,
    pub ram: Option<f64>,
    pub gpu: String,
    pub containers: Vec<Container>,
    pub last_seen: Option<f64>,
}

fn phase_name(p: wad_proto::v1::Phase) -> String {
    serde_json::to_value(p).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "idle".into())
}

/// This machine, as wadd says it is.
fn this_machine(s: &AppState) -> Machine {
    let screen = s.view.as_ref().map(|v| v.view.clone());
    let containers = s
        .workspaces
        .iter()
        .filter(|w| w.kind == wad_proto::v1::Kind::Container)
        .map(|w| {
            let st = s.states.get(&w.id);
            Container {
                id: w.id.clone(),
                name: w.name.clone(),
                running: st.is_some_and(|st| st.container == "running"),
                stream: w.display != wad_proto::v1::Display::Host || st.is_some_and(|st| st.streaming),
                since: st.map(|st| st.since).unwrap_or(0.0),
                phase: st.map(|st| phase_name(st.phase)).unwrap_or_else(|| "idle".into()),
                error: st.and_then(|st| st.error.clone()),
                on_screen: screen == Some(wad_proto::v1::View::Workspace(w.id.clone())),
                hotkey: w.hotkey.map(u64::from),
            }
        })
        .collect();
    let m = s.machine.as_ref();
    Machine {
        id: THIS.into(),
        name: m.map(|m| m.name.clone()).unwrap_or_else(|| "this machine".into()),
        label: "This machine".into(),
        os: m.map(|m| format!("WadSpaces · wadd {}", m.version)).unwrap_or_else(|| "WadSpaces".into()),
        up: s.connected,
        here: true,
        linked: s.cloud.as_ref().is_some_and(|c| c.linked),
        cpu: None,
        ram: None,
        gpu: String::new(),
        containers,
        last_seen: None,
    }
}

/// One of the account's other machines, from its last heartbeat (the web
/// app's toMachine).
fn from_heartbeat(d: &Value) -> Machine {
    let id = d["id"].as_str().unwrap_or_default().to_string();
    let seen = d["lastSeen"].as_str().and_then(secs_of);
    let up = seen.is_some_and(|t| now() - t < ONLINE_WITHIN);
    let view = d["view"].as_str().unwrap_or_default();
    let containers = d["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| {
            let wid = w["id"].as_str().unwrap_or_default().to_string();
            Container {
                name: w["name"].as_str().unwrap_or(&wid).to_string(),
                running: w["container"] == "running",
                stream: w["port"].as_u64().is_some(),
                since: seen.unwrap_or(0.0),
                phase: w["phase"].as_str().unwrap_or("idle").to_string(),
                error: w["error"].as_str().map(String::from),
                on_screen: view == format!("workspace:{wid}"),
                hotkey: w["hotkey"].as_u64(),
                id: wid,
            }
        })
        .collect();
    let host = d["hostname"].as_str().unwrap_or(&id).to_string();
    Machine {
        label: d["name"].as_str().unwrap_or(&host).to_string(),
        os: d["daemonVersion"].as_str().map(|v| format!("WadSpaces · wadd {v}")).unwrap_or_else(|| "WadSpaces".into()),
        up,
        here: false,
        linked: true,
        cpu: d["metrics"]["cpu"].as_f64(),
        ram: d["metrics"]["mem"].as_f64(),
        gpu: d["metrics"]["gpu"].as_str().unwrap_or_default().to_string(),
        containers,
        last_seen: seen,
        name: host,
        id,
    }
}

/// This machine, then the account's others.
pub fn machines(s: &AppState) -> Vec<Machine> {
    let me = s.cloud.as_ref().and_then(|c| c.machine_id.clone());
    let mut out = vec![this_machine(s)];
    out.extend(
        s.library
            .iter()
            .flat_map(|l| l.machines.iter())
            .filter(|d| d["id"].as_str() != me.as_deref())
            .map(from_heartbeat),
    );
    out
}

fn machine(s: &AppState, id: &str) -> Option<Machine> {
    machines(s).into_iter().find(|m| m.id == id)
}

/// A focus session is on and `id` isn't in it (the web app's isLocked).
pub fn locked(s: &AppState, id: &str) -> bool {
    s.session.as_ref().is_some_and(|x| {
        x.mode == wad_proto::v1::SessionMode::Focus && !x.expired && !x.workspaces.iter().any(|w| w == id)
    })
}

/// GET from wadd: the made-up machine's answer when there is one.
pub(crate) async fn get<T: DeserializeOwned + Send + 'static>(path: String) -> Result<T, String> {
    if crate::fake::on() {
        let v = crate::fake::answer(&path).ok_or("wadd isn't answering.")?;
        return serde_json::from_value(v).map_err(|e| e.to_string());
    }
    rt::run(async move { Wadd::here().get::<T>(&path).await }).await.map_err(|e| e.to_string())
}

/// POST, PUT or DELETE to wadd, its answer as `T`.
pub(crate) async fn send<T: DeserializeOwned + Send + 'static>(
    method: Method,
    path: String,
    body: Option<Value>,
) -> Result<T, String> {
    if crate::fake::on() {
        return Err("Not on a made-up machine.".into());
    }
    rt::run(async move { Wadd::here().send::<T>(method, &path, body.as_ref()).await }).await.map_err(|e| e.to_string())
}

/// A call to wadd here whose answer isn't needed, only that it worked.
pub(crate) async fn act(method: Method, path: String, body: Option<Value>) -> Result<(), String> {
    if crate::fake::on() {
        return Err("Not on a made-up machine.".into());
    }
    rt::run(async move { Wadd::here().act(method, &path, body.as_ref()).await }).await.map_err(|e| e.to_string())
}

/// Start, stop or restart a wadspace on a machine: here through wadd, on
/// another through its relay.
pub fn container_action(m: &Machine, id: &str, action: &'static str, done: impl FnOnce() + 'static) {
    let (machine, label, here, id) = (m.id.clone(), m.label.clone(), m.here, id.to_string());
    glib::spawn_future_local(async move {
        let r = if here {
            let path = format!("/v1/workspaces/{id}/{action}");
            send::<Value>(Method::POST, path, Some(serde_json::json!({}))).await.map(drop)
        } else {
            crate::account::command(machine, action, id).await
        };
        match r {
            Ok(()) if !here => {
                crate::toast::show("Sent", Some(&format!("{action} → {label}")), crate::toast::Tone::Info)
            }
            Ok(()) => {}
            Err(e) => crate::toast::error("Couldn't do that", &e),
        }
        done();
    });
}

/// The tabs: (value, icon, label).
fn tabs(m: &Machine) -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    let mut t = vec![("wadspaces", Some("activity"), "Wadspaces")];
    if m.here {
        t.push(("history", Some("rotate-ccw-clock"), "History"));
    }
    if m.here && m.up {
        t.push(("storage", Some("hard-drive"), "Storage"));
        t.push(("files", Some("folder-open"), "Files"));
        t.push(("devices", Some("usb"), "Devices"));
        t.push(("tailnet", Some("network"), "Tailnet"));
    }
    t.push(("diagnostics", Some("stethoscope"), "Diagnostics"));
    t
}

thread_local! {
    static PAGE: RefCell<std::rc::Weak<Page>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Picks a machine (snapshots).
pub fn pick(id: &str) {
    let Some(p) = PAGE.with(|p| p.borrow().upgrade()) else { return };
    p.picked.replace(id.to_string());
    store().read_now(|s| p.render(s));
}

/// The tab showing, and its name (snapshots).
pub fn tab_widget() -> Option<gtk::Widget> {
    PAGE.with(|p| p.borrow().upgrade()).and_then(|p| p.tab_stack.visible_child())
}

pub fn tab_name() -> &'static str {
    PAGE.with(|p| p.borrow().upgrade()).map(|p| *p.tab.borrow()).unwrap_or("wadspaces")
}

/// Shows one of the picked machine's tabs (snapshots).
pub fn show_tab(tab: &'static str) {
    let Some(p) = PAGE.with(|p| p.borrow().upgrade()) else { return };
    p.tab.replace(tab);
    let m = store().read_now(|s| machine(s, &p.picked.borrow()));
    if let Some(m) = m {
        p.make_tabs(&m, true);
    }
}

struct Page {
    /// The machine picked (this one to start with).
    picked: RefCell<String>,
    list: gtk::Box,
    head: gtk::Box,
    tab_bar: gtk::Box,
    tab_stack: gtk::Stack,
    /// What the tabs were made for: the machine, here, up.
    made_for: RefCell<Option<(String, bool, bool)>>,
    tab: RefCell<&'static str>,
}

pub fn page() -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title = gtk::Label::new(Some("Machines"));
    title.add_css_class("page-title");
    title.set_xalign(0.0);
    body.append(&title);
    let sub = gtk::Label::new(Some("Your WadSpaces machines and what runs on them."));
    sub.add_css_class("page-subtitle");
    sub.set_xalign(0.0);
    sub.set_margin_bottom(32);
    body.append(&sub);

    let grid = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.set_valign(gtk::Align::Start);
    // 280 wide, whatever its rows ask (their texts expand within it).
    let list_width = wad_gtk::Width::new(&list, 280);
    list_width.set_hexpand(false);
    list_width.set_valign(gtk::Align::Start);
    grid.append(&list_width);
    let detail = gtk::Box::new(gtk::Orientation::Vertical, 20);
    detail.set_hexpand(true);
    let head = gtk::Box::new(gtk::Orientation::Vertical, 0);
    detail.append(&head);
    let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    detail.append(&tab_bar);
    let tab_stack = gtk::Stack::new();
    tab_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    tab_stack.set_transition_duration(120);
    tab_stack.set_vhomogeneous(false);
    tab_stack.set_hhomogeneous(false);
    tab_stack.set_interpolate_size(false);
    detail.append(&tab_stack);
    grid.append(&detail);
    body.append(&grid);

    let pg = Rc::new(Page {
        picked: RefCell::new(THIS.into()),
        list,
        head,
        tab_bar,
        tab_stack,
        made_for: RefCell::default(),
        tab: RefCell::new("wadspaces"),
    });
    PAGE.with(|x| x.replace(Rc::downgrade(&pg)));
    let p = pg.clone();
    store().watch(
        &grid,
        Topic::MACHINE
            | Topic::WORKSPACES
            | Topic::STATES
            | Topic::VIEW
            | Topic::LIBRARY
            | Topic::WADD
            | Topic::CLOUD
            | Topic::ACCOUNT,
        move |s| p.render(s),
    );

    // The other machines' heartbeats, read again while the page shows.
    body.connect_map(|_| {
        glib::spawn_future_local(crate::account::refresh_machines());
    });
    let weak = body.downgrade();
    glib::timeout_add_seconds_local(30, move || {
        let Some(b) = weak.upgrade() else { return glib::ControlFlow::Break };
        if b.is_mapped() {
            glib::spawn_future_local(crate::account::refresh_machines());
        }
        glib::ControlFlow::Continue
    });
    super::page(&body)
}

impl Page {
    fn render(self: &Rc<Self>, s: &AppState) {
        let all = machines(s);
        let picked = self.picked.borrow().clone();
        let m = all.iter().find(|m| m.id == picked).or(all.first()).cloned();
        self.render_list(&all, m.as_ref().map(|m| m.id.as_str()).unwrap_or(THIS));
        let Some(m) = m else { return };
        ui::clear(&self.head);
        self.head.append(&header(&m));
        let made = (m.id.clone(), m.here, m.up);
        if self.made_for.borrow().as_ref() != Some(&made) {
            let same_machine = self.made_for.borrow().as_ref().is_some_and(|(id, _, _)| *id == m.id);
            *self.made_for.borrow_mut() = Some(made);
            self.make_tabs(&m, same_machine);
        }
    }

    fn render_list(self: &Rc<Self>, all: &[Machine], picked: &str) {
        ui::clear(&self.list);
        for m in all {
            let b = gtk::Button::new();
            b.add_css_class("machine-item");
            if m.id == picked {
                b.add_css_class("on");
            }
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.append(&tile(if m.here { "laptop" } else { "server" }, m.up, 40));
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_valign(gtk::Align::Center);
            text.set_hexpand(true);
            let t = gtk::Label::new(Some(&m.label));
            t.add_css_class("item-title");
            t.set_xalign(0.0);
            t.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&t);
            let sub = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            sub.append(&ui::dot(if m.up { "on" } else { "off" }));
            let running = m.containers.iter().filter(|c| c.running).count();
            let state = if m.up {
                format!("{running} running")
            } else if let Some(t) = m.last_seen {
                format!("seen {}", time_ago(now() - t))
            } else {
                "offline".into()
            };
            let l = gtk::Label::new(Some(&format!("{state} · {}", m.name)));
            l.add_css_class("small-muted");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            sub.append(&l);
            text.append(&sub);
            row.append(&text);
            b.set_child(Some(&row));
            let (p, id) = (self.clone(), m.id.clone());
            b.connect_clicked(move |_| {
                p.picked.replace(id.clone());
                store().read_now(|s| p.render(s));
            });
            self.list.append(&b);
        }
    }

    /// The tab bar and the tabs (made when first shown); the Wadspaces tab
    /// again on another machine.
    fn make_tabs(self: &Rc<Self>, m: &Machine, same_machine: bool) {
        ui::clear(&self.tab_bar);
        while let Some(c) = self.tab_stack.first_child() {
            self.tab_stack.remove(&c);
        }
        let options = tabs(m);
        if !same_machine || !options.iter().any(|(v, _, _)| *v == *self.tab.borrow()) {
            self.tab.replace("wadspaces");
        }
        let p = self.clone();
        let mid = m.id.clone();
        let values: Vec<&'static str> = options.iter().map(|(v, _, _)| *v).collect();
        let bar = ui::segmented(&options, &self.tab.borrow(), move |v| {
            let v = values.iter().find(|o| **o == v).copied().unwrap_or("wadspaces");
            p.tab.replace(v);
            p.show_tab(&mid);
        });
        self.tab_bar.append(&bar);
        self.show_tab(&m.id);
    }

    fn show_tab(&self, machine: &str) {
        let tab = *self.tab.borrow();
        if self.tab_stack.child_by_name(tab).is_none() {
            let w = match tab {
                "history" => history::view(),
                "storage" => storage::view(),
                "files" => files::view(),
                "devices" => devices::view(),
                "tailnet" => tailnet::view(),
                "diagnostics" => diagnostics::view(machine.to_string()),
                _ => wadspaces::view(machine.to_string()),
            };
            self.tab_stack.add_named(&w, Some(tab));
        }
        self.tab_stack.set_visible_child_name(tab);
    }
}

/// An icon on its tile: the accent's when the machine's up.
fn tile(icon: &'static str, live: bool, px: i32) -> gtk::Box {
    let t = gtk::Box::new(gtk::Orientation::Vertical, 0);
    t.add_css_class("machine-tile");
    t.add_css_class(if live { "live" } else { "dead" });
    t.set_size_request(px, px);
    t.set_valign(gtk::Align::Center);
    // The icon takes the tile's height; the tile takes none of the page's.
    t.set_vexpand(false);
    let i = Icon::new(icon, 20);
    i.set_halign(gtk::Align::Center);
    i.set_valign(gtk::Align::Center);
    i.set_vexpand(true);
    t.append(&i);
    t
}

/// The machine's card: what it is, whether it's up, Launch here; this one
/// linked to the account.
fn header(m: &Machine) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("machine-card");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let t = tile(if m.here { "laptop" } else { "server" }, m.up, 44);
    t.set_valign(gtk::Align::Start);
    row.append(&t);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_hexpand(true);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let name = gtk::Label::new(Some(&m.label));
    name.add_css_class("machine-title");
    title.append(&name);
    title.append(&badge(if m.up { "online" } else { "offline" }, None, m.up));
    text.append(&title);
    let facts = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    facts.add_css_class("mono-facts");
    let mut list = vec![m.name.clone(), m.os.clone()];
    if let Some(t) = m.last_seen.filter(|_| !m.up) {
        list.push(format!("last seen {}", time_ago(now() - t)));
    }
    for f in list {
        let l = gtk::Label::new(Some(&f));
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        facts.append(&l);
    }
    text.append(&facts);
    row.append(&text);
    // The launcher starts things here: only this machine's.
    if m.here {
        let launch = ui::button("Launch here", Some("play"), Variant::Primary, Size::Md);
        launch.set_valign(gtk::Align::Start);
        launch.set_sensitive(m.up);
        launch.connect_clicked(|_| crate::launcher::open());
        row.append(&launch);
    }
    card.append(&row);
    if m.here && m.linked {
        let linked = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        linked.add_css_class("linked-row");
        linked.append(&tile("shield-check", true, 40));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        let t = gtk::Label::new(Some("Linked to your account"));
        t.add_css_class("strong");
        t.set_xalign(0.0);
        text.append(&t);
        let d = ui::wrapping("You can start, stop and show wadspaces on this machine from WadSpaces online.");
        d.add_css_class("small-muted");
        text.append(&d);
        linked.append(&text);
        card.append(&linked);
    }
    card.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn phases_read_as_the_web_app_says_them() {
        let c = |phase: &str, running: bool, on_screen: bool| Container {
            id: "a".into(),
            name: "A".into(),
            running,
            stream: false,
            since: 0.0,
            phase: phase.into(),
            error: None,
            on_screen,
            hotkey: None,
        };
        assert_eq!(c("ready", true, true).phase_label(), "On screen");
        assert_eq!(c("idle", true, false).phase_label(), "Running");
        assert_eq!(c("idle", false, false).phase_label(), "Stopped");
        assert_eq!(c("waiting", true, false).phase_label(), "Waiting for its desktop");
        assert_eq!(c("error", false, false).dot(), "error");
        assert_eq!(c("starting", false, false).dot(), "busy");
        assert_eq!(c("ready", true, false).dot(), "on");
    }

    #[test]
    fn other_machines_come_from_their_heartbeats_and_this_one_isnt_twice() {
        let mut s = AppState {
            cloud: Some(wad_proto::v1::CloudLink {
                linked: true,
                machine_id: Some("m-here".into()),
                owner_uid: Some("u".into()),
                project_id: None,
                linked_at: None,
                last_heartbeat: None,
                last_error: None,
            }),
            ..Default::default()
        };
        let fresh = super::super::now() - 10.0;
        let iso = wad_account::rfc3339((fresh * 1000.0) as i64);
        s.library = Some(wad_account::Library {
            machines: vec![
                json!({ "id": "m-here", "hostname": "surface" }),
                json!({
                    "id": "m-desk", "hostname": "desk", "name": "Desk", "daemonVersion": "0.2.0", "lastSeen": iso,
                    "view": "workspace:w1", "metrics": { "cpu": 12.0, "mem": 40.0, "gpu": "AMD" },
                    "workspaces": [{ "id": "w1", "container": "running", "phase": "ready", "hotkey": 2, "port": 3001 }],
                }),
            ],
            ..Default::default()
        });
        let all = machines(&s);
        assert_eq!(all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec![THIS, "m-desk"]);
        let desk = &all[1];
        assert!(desk.up && !desk.here);
        assert_eq!((desk.label.as_str(), desk.os.as_str()), ("Desk", "WadSpaces · wadd 0.2.0"));
        assert_eq!((desk.cpu, desk.ram, desk.gpu.as_str()), (Some(12.0), Some(40.0), "AMD"));
        let c = &desk.containers[0];
        assert!(c.running && c.on_screen && c.stream);
        assert_eq!(c.hotkey, Some(2));
    }
}
