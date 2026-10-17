//! Diagnostics (the web app's Resources and MachineDiagnostics): a
//! machine's CPU, memory and GPU; and on this machine, when a wadspace won't
//! start, where it is and what broke, without a shell: wadd's health, each
//! workspace's phase and last error, logs (wadd's own, a unit's journal, a
//! container's output) and recent problems. Everything wadd returns here is
//! redacted. Polled only while it shows.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::json;
use wad_gtk::Icon;
use wad_proto::v1::{Diagnostics, LogLine, LogText, Metrics};

use super::{Container, THIS, container_action, get, machine};
use crate::store::{Topic, store};
use crate::ui::{self, Size, Variant};

/// Where a log comes from.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Daemon,
    Unit(String),
    Workspace(String),
}

const LINES: [u32; 4] = [100, 200, 500, 1000];

fn hms(ms: u64) -> String {
    glib::DateTime::from_unix_local((ms / 1000) as i64)
        .ok()
        .and_then(|d| d.format("%H:%M:%S").ok())
        .map(String::from)
        .unwrap_or_default()
}

fn daemon_text(lines: &[LogLine]) -> String {
    lines
        .iter()
        .map(|l| format!("{} {:<7} {}: {}", hms(l.time), l.level, l.target, l.message))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn view(id: String) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 20);
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    card.add_css_class("machine-card");
    card.set_homogeneous(true);
    root.append(&card);
    if id == THIS {
        // Its metrics come from wadd, every few seconds while they show.
        let metrics: Rc<RefCell<Option<Metrics>>> = Rc::default();
        let draw = {
            let (card, metrics) = (card.clone(), metrics.clone());
            move || {
                let m = metrics.borrow();
                resources(
                    &card,
                    m.as_ref().map(|m| m.cpu),
                    m.as_ref().map(|m| m.mem),
                    m.as_ref().map(|m| m.gpu.as_str()),
                );
            }
        };
        draw();
        ui::every(&root, 4, move || {
            let (metrics, draw) = (metrics.clone(), draw.clone());
            glib::spawn_future_local(async move {
                if let Ok(m) = get::<Metrics>("/v1/metrics".into()).await {
                    metrics.replace(Some(m));
                    draw();
                }
            });
        });
        root.append(&this_machine());
    } else {
        let c = card.clone();
        store().watch(&root, Topic::LIBRARY, move |s| {
            if let Some(m) = machine(s, &id) {
                resources(&c, m.cpu, m.ram, Some(m.gpu.as_str()).filter(|g| !g.is_empty()));
            }
        });
    }
    root.upcast()
}

/// CPU, memory and GPU, each on its card.
fn resources(card: &gtk::Box, cpu: Option<f64>, ram: Option<f64>, gpu: Option<&str>) {
    ui::clear(card);
    card.append(&resource("cpu", "CPU", cpu, None, false));
    card.append(&resource("memory-stick", "Memory", ram, None, true));
    card.append(&resource("monitor-smartphone", "GPU", None, Some(gpu.unwrap_or("—")), false));
}

fn resource(icon: &'static str, label: &str, value: Option<f64>, text: Option<&str>, pink: bool) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.add_css_class("resource");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("resource-head");
    head.append(&Icon::new(icon, 16));
    let l = gtk::Label::new(Some(label));
    l.set_hexpand(true);
    l.set_xalign(0.0);
    head.append(&l);
    let v = gtk::Label::new(Some(&match (value, text) {
        (Some(v), _) => format!("{}%", v.round()),
        (None, Some(t)) => t.to_string(),
        _ => "—".into(),
    }));
    v.add_css_class("resource-value");
    v.set_ellipsize(gtk::pango::EllipsizeMode::End);
    head.append(&v);
    b.append(&head);
    if text.is_none() {
        b.append(&ui::meter(value.unwrap_or(0.0), pink));
        if value.is_none() {
            let n = gtk::Label::new(Some("Reported by newer versions of wadd"));
            n.add_css_class("tiny-faint");
            n.set_xalign(0.0);
            b.append(&n);
        }
    }
    b.upcast()
}

struct Diag {
    diag: RefCell<Option<Diagnostics>>,
    error: RefCell<Option<String>>,
    source: RefCell<Source>,
    sources: Rc<RefCell<Vec<(Source, String)>>>,
    lines: Cell<u32>,
    follow: Cell<bool>,
    error_slot: gtk::Box,
    health: gtk::Box,
    rows: gtk::Box,
    picker: gtk::DropDown,
    log: gtk::TextBuffer,
    log_view: gtk::ScrolledWindow,
    log_error: gtk::Box,
    problems: gtk::Box,
}

fn this_machine() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 20);
    let intro = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let l = ui::wrapping("Errors and logs from wadd on this machine. Tokens and passwords are redacted.");
    l.add_css_class("small-muted-14");
    intro.append(&l);
    let copy = ui::button("Copy report", Some("clipboard-copy"), Variant::Secondary, Size::Md);
    copy.set_valign(gtk::Align::Center);
    intro.append(&copy);
    root.append(&intro);
    let error_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&error_slot);
    let health = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    health.set_homogeneous(true);
    root.append(&health);

    let ws = ui::panel("Workspaces", None, Some("Phase and the last error for each workspace."));
    ws.body.add_css_class("panel-pad");
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("diag-rows");
    ws.body.append(&rows);
    root.append(&ws.root);

    let logs =
        ui::panel("Logs", None, Some("wadd's own log, a systemd unit's journal, or a workspace container's output."));
    logs.body.add_css_class("panel-pad");
    let picker = gtk::DropDown::from_strings(&["wadd (live)"]);
    picker.add_css_class("select");
    logs.actions.append(&picker);
    let line_labels: Vec<String> = LINES.iter().map(|n| format!("last {n}")).collect();
    let line_refs: Vec<&str> = line_labels.iter().map(String::as_str).collect();
    let lines = gtk::DropDown::from_strings(&line_refs);
    lines.add_css_class("select");
    lines.set_selected(1);
    logs.actions.append(&lines);
    let follow = gtk::CheckButton::with_label("Follow");
    follow.add_css_class("check-label");
    follow.set_active(true);
    logs.actions.append(&follow);
    let refresh = ui::button("Refresh", Some("refresh-cw"), Variant::Ghost, Size::Sm);
    logs.actions.append(&refresh);
    let log_error = gtk::Box::new(gtk::Orientation::Vertical, 0);
    logs.body.append(&log_error);
    let tv = gtk::TextView::new();
    tv.add_css_class("log");
    tv.set_editable(false);
    tv.set_cursor_visible(false);
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    let log_view = gtk::ScrolledWindow::new();
    log_view.add_css_class("log-frame");
    log_view.set_hscrollbar_policy(gtk::PolicyType::Never);
    log_view.set_min_content_height(160);
    log_view.set_max_content_height(448);
    log_view.set_propagate_natural_height(true);
    log_view.set_child(Some(&tv));
    logs.body.append(&log_view);
    root.append(&logs.root);

    let pr = ui::panel("Recent problems", None, Some("Warnings and errors wadd logged."));
    pr.body.add_css_class("panel-pad");
    let problems = gtk::Box::new(gtk::Orientation::Vertical, 6);
    pr.body.append(&problems);
    root.append(&pr.root);

    let d = Rc::new(Diag {
        diag: RefCell::default(),
        error: RefCell::default(),
        source: RefCell::new(Source::Daemon),
        sources: Rc::new(RefCell::new(vec![(Source::Daemon, "wadd (live)".into())])),
        lines: Cell::new(200),
        follow: Cell::new(true),
        error_slot,
        health,
        rows,
        picker: picker.clone(),
        log: tv.buffer(),
        log_view,
        log_error,
        problems,
    });

    let dd = d.clone();
    picker.connect_selected_notify(move |p| {
        let pick = dd.sources.borrow().get(p.selected() as usize).map(|(s, _)| s.clone());
        if let Some(s) = pick.filter(|s| *s != *dd.source.borrow()) {
            dd.source.replace(s);
            dd.clone().load_log();
        }
    });
    let dd = d.clone();
    lines.connect_selected_notify(move |l| {
        dd.lines.set(LINES.get(l.selected() as usize).copied().unwrap_or(200));
        dd.clone().load_log();
    });
    let dd = d.clone();
    follow.connect_toggled(move |c| dd.follow.set(c.is_active()));
    let dd = d.clone();
    refresh.connect_clicked(move |_| dd.clone().load_log());
    let dd = d.clone();
    copy.connect_clicked(move |b| dd.clone().copy_report(b));
    // The live phases follow wadd's events.
    let dd = d.clone();
    store().watch(&root, Topic::STATES | Topic::WORKSPACES, move |_| dd.render());

    let dd = d.clone();
    ui::every(&root, 5, move || dd.clone().load());
    let dd = d.clone();
    ui::every(&root, 3, move || {
        if dd.follow.get() {
            dd.clone().load_log();
        }
    });
    root.upcast()
}

impl Diag {
    fn load(self: Rc<Self>) {
        glib::spawn_future_local(async move {
            match get::<Diagnostics>("/v1/diagnostics".into()).await {
                Ok(d) => {
                    self.diag.replace(Some(d));
                    self.error.replace(None);
                }
                Err(e) => {
                    self.error.replace(Some(e));
                }
            }
            self.render();
        });
    }

    fn load_log(self: Rc<Self>) {
        let n = self.lines.get();
        let src = self.source.borrow().clone();
        glib::spawn_future_local(async move {
            let text = match &src {
                Source::Daemon => get::<Vec<LogLine>>(format!("/v1/logs?lines={n}")).await.map(|l| daemon_text(&l)),
                Source::Unit(u) => {
                    get::<LogText>(format!("/v1/logs/unit/{}?lines={n}", ui::enc(u))).await.map(|t| t.text)
                }
                Source::Workspace(w) => {
                    get::<LogText>(format!("/v1/logs/workspace/{}?lines={n}", ui::enc(w))).await.map(|t| t.text)
                }
            };
            ui::clear(&self.log_error);
            match text {
                Ok(t) => {
                    let t = if t.is_empty() { "(empty)".to_string() } else { t };
                    let (s, e) = self.log.bounds();
                    if self.log.text(&s, &e, false) != t {
                        self.log.set_text(&t);
                        // Keep the newest lines in view while following.
                        if self.follow.get() {
                            let adj = self.log_view.vadjustment();
                            glib::idle_add_local_once(move || adj.set_value(adj.upper()));
                        }
                    }
                }
                Err(e) => {
                    let n = ui::notice(&e, true);
                    n.set_margin_top(0);
                    n.set_margin_bottom(8);
                    self.log_error.append(&n);
                }
            }
        });
    }

    fn copy_report(self: Rc<Self>, b: &gtk::Button) {
        let b = b.clone();
        glib::spawn_future_local(async move {
            let known = self.diag.borrow().clone();
            let diag = match known {
                Some(d) => serde_json::to_value(d).unwrap_or_default(),
                None => match get::<Diagnostics>("/v1/diagnostics".into()).await {
                    Ok(d) => serde_json::to_value(d).unwrap_or_default(),
                    Err(e) => json!({ "error": e }),
                },
            };
            let log =
                get::<Vec<LogLine>>("/v1/logs?lines=200".into()).await.map(|l| daemon_text(&l)).unwrap_or_else(|e| e);
            let generated = glib::DateTime::now_utc()
                .ok()
                .and_then(|d| d.format_iso8601().ok())
                .map(String::from)
                .unwrap_or_default();
            let report = json!({ "generated": generated, "diagnostics": diag, "daemon_log": log });
            let text = serde_json::to_string_pretty(&report).unwrap_or_default();
            WidgetExt::display(&b).clipboard().set_text(&text);
            ui::set_text(&b, "Copied");
            glib::timeout_add_seconds_local_once(2, move || ui::set_text(&b, "Copy report"));
        });
    }

    fn render(&self) {
        ui::clear(&self.error_slot);
        if let Some(e) = self.error.borrow().as_ref() {
            let n = ui::notice(e, true);
            n.set_margin_top(0);
            self.error_slot.append(&n);
        }
        let diag = self.diag.borrow();
        let Some(d) = diag.as_ref() else {
            ui::clear(&self.rows);
            if self.error.borrow().is_none() {
                let l = gtk::Label::new(Some("Loading…"));
                l.add_css_class("small-muted-14");
                l.set_xalign(0.0);
                self.rows.append(&l);
            }
            return;
        };
        self.render_health(d);
        self.render_sources(d);
        ui::clear(&self.rows);
        let (workspaces, states) = store().read_now(|s| (s.workspaces.clone(), s.states.clone()));
        for st in &d.workspaces {
            let w = workspaces.iter().find(|w| w.id == st.id);
            let live = states.get(&st.id).unwrap_or(st);
            self.rows.append(&self.workspace_row(st, live, w));
        }
        ui::clear(&self.problems);
        if d.recent_problems.is_empty() {
            let l = gtk::Label::new(Some("Nothing recent."));
            l.add_css_class("small-muted-14");
            l.set_xalign(0.0);
            self.problems.append(&l);
        }
        for p in d.recent_problems.iter().rev() {
            let l = ui::wrapping(&format!("{} {} {}: {}", hms(p.time), p.level, p.target, p.message));
            l.add_css_class("problem");
            l.add_css_class(if p.level.to_ascii_lowercase().starts_with("warn") { "warn" } else { "bad" });
            self.problems.append(&l);
        }
    }

    fn render_health(&self, d: &Diagnostics) {
        ui::clear(&self.health);
        let low = d.disk.free_bytes < 5_000_000_000;
        let net_ok = !d.network.available || d.network.connectivity == "full";
        let cards = [
            (
                "wadd",
                true,
                false,
                format!("{} · up {} · podman backend", d.wadd.version, ui::duration(d.wadd.uptime_s as f64)),
            ),
            (
                "Podman",
                d.podman.connected && d.podman.error.is_none(),
                false,
                d.podman.error.clone().unwrap_or_else(|| {
                    format!(
                        "{} · {} images",
                        d.podman.version.as_deref().unwrap_or("?"),
                        d.podman.images.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
                    )
                }),
            ),
            (
                "Network",
                net_ok,
                !d.network.available,
                if d.network.available {
                    format!("{} · {}", d.network.ssid.as_deref().unwrap_or("no Wi-Fi"), d.network.connectivity)
                } else {
                    "NetworkManager not available".into()
                },
            ),
            (
                "Disk",
                d.disk.free_bytes > 1_000_000_000,
                low,
                format!(
                    "{} free of {}{}",
                    ui::bytes(d.disk.free_bytes),
                    ui::bytes(d.disk.total_bytes),
                    if low { " — images may not fit" } else { "" }
                ),
            ),
        ];
        for (title, ok, warn, text) in cards {
            let c = gtk::Box::new(gtk::Orientation::Vertical, 6);
            c.add_css_class("health");
            c.add_css_class(if !ok {
                "bad"
            } else if warn {
                "warn"
            } else {
                "good"
            });
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            head.append(&ui::dot(if !ok {
                "error"
            } else if warn {
                "busy"
            } else {
                "on-flat"
            }));
            let t = gtk::Label::new(Some(title));
            t.add_css_class("strong");
            head.append(&t);
            c.append(&head);
            let l = ui::wrapping(&text);
            l.add_css_class("small-muted");
            c.append(&l);
            self.health.append(&c);
        }
    }

    /// The log picker's choices: wadd's, the units', the containers'.
    fn render_sources(&self, d: &Diagnostics) {
        let names = store().read_now(|s| s.workspaces.clone());
        let mut list = vec![(Source::Daemon, "wadd (live)".to_string())];
        list.extend(d.log_units.iter().map(|u| (Source::Unit(u.clone()), format!("journal: {u}"))));
        list.extend(d.workspaces.iter().map(|w| {
            let name = names.iter().find(|x| x.id == w.id).map(|x| x.name.clone()).unwrap_or_else(|| w.id.clone());
            (Source::Workspace(w.id.clone()), format!("container: {name}"))
        }));
        if *self.sources.borrow() == list {
            return;
        }
        let labels: Vec<&str> = list.iter().map(|(_, l)| l.as_str()).collect();
        let at = list.iter().position(|(s, _)| *s == *self.source.borrow()).unwrap_or(0);
        self.sources.replace(list.clone());
        self.picker.set_model(Some(&gtk::StringList::new(&labels)));
        self.picker.set_selected(at as u32);
    }

    fn workspace_row(
        &self,
        st: &wad_proto::v1::WorkspaceState,
        live: &wad_proto::v1::WorkspaceState,
        w: Option<&wad_proto::v1::Workspace>,
    ) -> gtk::Widget {
        let running = live.container == "running";
        let phase = super::phase_name(live.phase);
        let c = Container {
            id: st.id.clone(),
            name: w.map(|w| w.name.clone()).unwrap_or_else(|| st.id.clone()),
            running,
            stream: false,
            since: live.since,
            phase: phase.clone(),
            error: live.error.clone(),
            on_screen: false,
            hotkey: None,
        };
        let enabled = w.is_none_or(|w| w.enabled);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        row.add_css_class("diag-row");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.append(&ui::dot(c.dot()));
        let name = gtk::Label::new(Some(&c.name));
        name.add_css_class("medium");
        top.append(&name);
        let ph = gtk::Label::new(Some(if enabled { phase.as_str() } else { "disabled" }));
        ph.add_css_class("small-faint");
        top.append(&ph);
        text.append(&top);
        if let Some(w) = w.filter(|w| !w.image.is_empty()) {
            let img = gtk::Label::new(Some(&w.image));
            img.add_css_class("mono-faint");
            img.set_xalign(0.0);
            img.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&img);
        }
        let built = match st.image_present {
            None => "unknown",
            Some(true) => "built",
            Some(false) => "not built",
        };
        let facts = gtk::Label::new(Some(&format!("container {} · image {built}", st.container)));
        facts.add_css_class("small-muted");
        facts.set_xalign(0.0);
        text.append(&facts);
        if let Some(m) = &live.message {
            let l = ui::wrapping(m);
            l.add_css_class("small-muted");
            text.append(&l);
        }
        if let Some(e) = &live.error {
            let l = ui::wrapping(e);
            l.add_css_class("small-danger");
            text.append(&l);
        }
        row.append(&text);
        if enabled {
            let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            buttons.set_valign(gtk::Align::Start);
            let here = store().read_now(|s| machine(s, THIS));
            let act = |label: &str, action: &'static str| {
                let b = ui::button(label, None, Variant::Secondary, Size::Sm);
                if let Some(m) = here.clone() {
                    let id = c.id.clone();
                    b.connect_clicked(move |b| {
                        b.set_sensitive(false);
                        let b = b.clone();
                        container_action(&m, &id, action, move || b.set_sensitive(true));
                    });
                }
                buttons.append(&b);
            };
            if running {
                act("Restart", "restart");
                act("Stop", "stop");
            } else {
                act("Start", "start");
            }
            let logs = ui::button("Logs", None, Variant::Ghost, Size::Sm);
            let (id, sources, picker) = (c.id.clone(), self.sources.clone(), self.picker.clone());
            logs.connect_clicked(move |_| {
                if let Some(at) = sources.borrow().iter().position(|(s, _)| *s == Source::Workspace(id.clone())) {
                    picker.set_selected(at as u32);
                }
            });
            buttons.append(&logs);
            row.append(&buttons);
        }
        row.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wadd_log_lines_read_as_the_web_app_shows_them() {
        let l = LogLine { time: 0, level: "warn".into(), target: "wadd::api".into(), message: "slow".into() };
        let t = daemon_text(&[l]);
        assert!(t.ends_with(" warn    wadd::api: slow"), "{t}");
    }
}
