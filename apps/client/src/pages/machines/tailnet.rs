//! Tailnet (this machine): the machine on its owner's Tailscale network
//! (host/TAILNET.md). Joining (a QR code of the link to approve it at, for
//! a phone signed in to Tailscale), leaving, its name and addresses there,
//! and which of the owner's devices may manage it from the WadSpaces app:
//! devices asking (Allow, Not now), and the owner's devices to trust or
//! remove. wadd's tailnet event keeps it current.

use gtk::{glib, prelude::*};
use serde_json::json;
use wad_gtk::Icon;
use wad_proto::v1::{TailnetAsk, TailnetDevice, TailnetStatus};
use wad_wadd_client::Method;

use super::{get, send};
use crate::modal::Modal;
use crate::pages::{now, secs_of, time_ago};
use crate::store::{Topic, store};
use crate::ui::{self, Size, Variant};

/// Reads it again (the events keep it current after that).
pub fn load() {
    glib::spawn_future_local(async {
        if let Ok(t) = get::<TailnetStatus>("/v1/tailnet".into()).await {
            take(t);
        }
    });
}

fn take(t: TailnetStatus) {
    store().update(|s| {
        s.tailnet = Some(t);
        Topic::TAILNET
    });
}

/// A change to the tailnet here (`path`, POST/PUT/DELETE): its answer is the
/// new state; a failure, a toast.
fn change(method: Method, path: String, failed: &'static str) {
    glib::spawn_future_local(async move {
        match send::<TailnetStatus>(method, path, Some(json!({}))).await {
            Ok(t) => take(t),
            Err(e) => crate::toast::error(failed, &e),
        }
    });
}

/// Allow a device that asked (the toast's button, the HUD's card).
pub fn allow(id: &str) {
    change(Method::PUT, format!("/v1/tailnet/devices/{id}"), "Couldn't trust it");
}

pub fn view() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 20);
    let net = ui::panel(
        "Tailnet",
        Some("network"),
        Some(
            "Your Tailscale network. Your own devices on it can build, open and stream this machine's wadspaces from the WadSpaces app, once you trust them here.",
        ),
    );
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("tunnel-rows");
    net.body.append(&rows);
    let join = gtk::Box::new(gtk::Orientation::Vertical, 12);
    join.add_css_class("tailnet-join");
    net.body.append(&join);
    root.append(&net.root);

    let asks = ui::panel("Asking to manage this machine", Some("shield-question-mark"), None);
    asks.body.add_css_class("device-list");
    root.append(&asks.root);

    let devices = ui::panel(
        "Your devices",
        Some("smartphone"),
        Some(
            "Trusted ones can build, open, stop and stream wadspaces here. Other people's devices, tagged ones and shared ones can't even ask.",
        ),
    );
    devices.body.add_css_class("device-list");
    root.append(&devices.root);
    root.connect_map(|_| load());

    let (actions, asks_root, asks_body, dev_root, dev_body) =
        (net.actions.clone(), asks.root.clone(), asks.body.clone(), devices.root.clone(), devices.body.clone());
    store().watch(&root, Topic::TAILNET, move |s| {
        let Some(t) = s.tailnet.as_ref() else {
            ui::clear(&rows);
            rows.append(&ui::busy("Looking…"));
            asks_root.set_visible(false);
            dev_root.set_visible(false);
            return;
        };
        draw_actions(&actions, t);
        draw_facts(&rows, t);
        draw_join(&join, t);
        asks_root.set_visible(!t.asks.is_empty());
        ui::clear(&asks_body);
        for a in &t.asks {
            asks_body.append(&ask_row(a));
        }
        dev_root.set_visible(t.state == "running" || t.devices.iter().any(|d| d.trusted));
        ui::clear(&dev_body);
        if t.devices.is_empty() {
            dev_body.append(&ui::empty_state(
                "smartphone",
                "No other devices yet",
                Some("Sign in to Tailscale on your phone with the same account: it shows up here."),
            ));
        }
        for d in &t.devices {
            dev_body.append(&device_row(d));
        }
    });
    root.upcast()
}

/// Join or Leave, as the machine is.
fn draw_actions(actions: &gtk::Box, t: &TailnetStatus) {
    ui::clear(actions);
    match t.state.as_str() {
        "needsLogin" | "stopped" => {
            let b = ui::button("Join", Some("log-in"), Variant::Primary, Size::Sm);
            b.connect_clicked(|b| {
                b.set_sensitive(false);
                change(Method::POST, "/v1/tailnet/login".into(), "Couldn't join the tailnet");
            });
            actions.append(&b);
        }
        "running" | "needsApproval" | "loggingIn" | "starting" => {
            let b = ui::button("Leave", Some("log-out"), Variant::Ghost, Size::Sm);
            b.connect_clicked(|_| confirm_leave());
            actions.append(&b);
        }
        _ => {}
    }
}

fn confirm_leave() {
    let m = Modal::open(
        "Leave the tailnet?",
        Some("Your devices can't reach this machine until it joins again. The devices you trust stay trusted."),
        440,
    );
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let leave = ui::button("Leave", Some("log-out"), Variant::Danger, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let mm = m.clone();
    leave.connect_clicked(move |_| {
        change(Method::POST, "/v1/tailnet/logout".into(), "Couldn't leave the tailnet");
        mm.close();
    });
    row.append(&cancel);
    row.append(&leave);
    m.body().append(&row);
}

/// How the machine is on the tailnet, in a word and a dot.
pub fn status(t: &TailnetStatus) -> (&'static str, String) {
    match t.state.as_str() {
        "off" => ("error", "Tailscale isn't running on this machine".into()),
        "needsLogin" => ("off", "Not on a tailnet".into()),
        "loggingIn" => ("busy", "Waiting for you to approve it…".into()),
        "needsApproval" => ("busy", "Waiting for the tailnet's admin to approve it".into()),
        "stopped" => ("off", "Joined, but turned off: Join turns it on".into()),
        "starting" => ("busy", "Connecting…".into()),
        "running" if t.listening => ("on", "Connected · your trusted devices can reach it".into()),
        "running" => ("busy", "Connected · the remote API isn't listening yet".into()),
        other => ("off", other.to_string()),
    }
}

fn text(t: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

fn fact(rows: &gtk::Box, name: &str, value: &impl IsA<gtk::Widget>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("tunnel-row");
    let l = gtk::Label::new(Some(name));
    l.add_css_class("tunnel-key");
    l.set_xalign(0.0);
    l.set_width_chars(11);
    l.set_valign(gtk::Align::Center);
    row.append(&l);
    let v = value.upcast_ref::<gtk::Widget>();
    v.set_hexpand(true);
    row.append(v);
    rows.append(&row);
}

/// "Logs in again by 16 Apr 2027", or nothing when it never has to.
pub fn key_note(expiry: Option<&str>) -> Option<String> {
    let e = expiry?;
    let secs = secs_of(e)?;
    let days = ((secs - now()) / 86400.0).floor();
    let date = e.split('T').next().unwrap_or(e);
    Some(if days < 0.0 {
        "Its key ran out: join again".into()
    } else if days < 14.0 {
        format!("Must log in again by {date} ({days} days): turn key expiry off in Tailscale's admin console")
    } else {
        format!("Logs in again by {date} (or turn key expiry off in Tailscale's admin console)")
    })
}

fn draw_facts(rows: &gtk::Box, t: &TailnetStatus) {
    ui::clear(rows);
    let (dot, word) = status(t);
    let st = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    st.append(&ui::dot(dot));
    st.append(&text(&word, "row-title"));
    fact(rows, "Status", &st);
    if let Some(e) = &t.error {
        fact(rows, "Last try", &text(e, "small-muted-14"));
    }
    if t.state != "running" {
        return;
    }
    let name = if t.dns_name.is_empty() { t.hostname.clone() } else { t.dns_name.clone() };
    fact(rows, "Name", &text(&name, "mono-14"));
    fact(rows, "Address", &text(&format!("{} · port {}", t.ips.join(", "), t.port), "mono-14"));
    let who = match (t.tailnet.is_empty(), t.user.is_empty()) {
        (false, false) if t.tailnet != t.user => format!("{} · as {}", t.tailnet, t.user),
        (false, _) => t.tailnet.clone(),
        (true, false) => t.user.clone(),
        (true, true) => "—".into(),
    };
    fact(rows, "Tailnet", &text(&who, "small-muted-14"));
    if let Some(n) = key_note(t.key_expiry.as_deref()) {
        fact(rows, "Key", &text(&n, "small-muted-14"));
    }
}

/// While joining: the link to approve the machine at, as a QR code.
fn draw_join(join: &gtk::Box, t: &TailnetStatus) {
    ui::clear(join);
    let Some(url) = t.login_url.as_ref().filter(|_| t.state == "loggingIn") else {
        join.set_visible(t.state == "loggingIn");
        if t.state == "loggingIn" {
            join.append(&ui::busy("Asking Tailscale for the link…"));
        }
        return;
    };
    join.set_visible(true);
    let qr = gtk::Image::new();
    qr.add_css_class("qr");
    qr.set_pixel_size(176);
    qr.set_halign(gtk::Align::Center);
    if let Some(tex) = wad_gtk::icon::svg_texture(crate::stream::qr_svg(url).as_bytes(), 352) {
        qr.set_paintable(Some(&tex));
    }
    join.append(&qr);
    let note = ui::wrapping(
        "Scan it with your phone (signed in to Tailscale) or open the link, and approve this machine. It joins as soon as you do.",
    );
    note.add_css_class("small-muted");
    join.append(&note);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
    rows.add_css_class("copy-rows");
    rows.append(&ui::copy_row("Link", url));
    join.append(&rows);
}

/// A lucide icon for a device's OS.
pub fn os_icon(os: &str) -> &'static str {
    match os.to_ascii_lowercase().as_str() {
        "android" | "ios" => "smartphone",
        "macos" | "windows" | "linux" => "laptop",
        _ => "monitor-smartphone",
    }
}

fn head(icon: &'static str, name: &str, sub: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("device-row");
    let i = Icon::new(icon, 18);
    i.add_css_class("muted");
    i.set_valign(gtk::Align::Center);
    row.append(&i);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    let n = gtk::Label::new(Some(name));
    n.add_css_class("row-name");
    n.set_xalign(0.0);
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let w = gtk::Label::new(Some(sub));
    w.add_css_class("small-muted");
    w.set_xalign(0.0);
    w.set_ellipsize(gtk::pango::EllipsizeMode::End);
    texts.append(&n);
    texts.append(&w);
    row.append(&texts);
    row
}

fn ask_row(a: &TailnetAsk) -> gtk::Box {
    let row = head(os_icon(&a.os), &a.name, &format!("Asked {}", time_ago(now() - a.at as f64 / 1000.0)));
    let later = ui::button("Not now", None, Variant::Ghost, Size::Sm);
    later.set_valign(gtk::Align::Center);
    let id = a.id.clone();
    later.connect_clicked(move |_| {
        change(Method::POST, format!("/v1/tailnet/asks/{id}/dismiss"), "Couldn't put it off");
    });
    row.append(&later);
    let ok = ui::button("Allow", Some("shield-check"), Variant::Primary, Size::Sm);
    ok.set_valign(gtk::Align::Center);
    let id = a.id.clone();
    ok.connect_clicked(move |_| allow(&id));
    row.append(&ok);
    row
}

/// What's known of a device, in words.
pub fn device_sub(d: &TailnetDevice) -> String {
    let mut parts = vec![if d.online { "Online" } else { "Offline" }.to_string()];
    if d.trusted {
        parts.push(match d.last_used {
            Some(t) => format!("trusted · last used {}", time_ago(now() - t as f64 / 1000.0)),
            None => "trusted".into(),
        });
    } else {
        parts.push("can't manage this machine".into());
    }
    parts.join(" · ")
}

fn device_row(d: &TailnetDevice) -> gtk::Box {
    let row = head(os_icon(&d.os), &d.name, &device_sub(d));
    if !d.online {
        row.add_css_class("dim");
    }
    let id = d.id.clone();
    let b = if d.trusted {
        let b = ui::button("Remove", Some("shield-off"), Variant::Ghost, Size::Sm);
        b.set_tooltip_text(Some("It can't manage this machine any more"));
        b.connect_clicked(move |_| {
            change(Method::DELETE, format!("/v1/tailnet/devices/{id}"), "Couldn't remove it");
        });
        b
    } else {
        let b = ui::button("Trust", Some("shield-check"), Variant::Secondary, Size::Sm);
        b.set_tooltip_text(Some("It can build, open, stop and stream wadspaces here"));
        b.connect_clicked(move |_| allow(&id));
        b
    };
    b.set_valign(gtk::Align::Center);
    row.append(&b);
    row
}

/// A new ask (one wadd hadn't sent before): a toast with its Allow.
pub fn toast_asks(before: Option<&TailnetStatus>, now: &TailnetStatus) {
    let Some(before) = before else { return };
    for a in now.asks.iter().filter(|a| !before.asks.iter().any(|b| b.id == a.id)) {
        let (id, name) = (a.id.clone(), a.name.clone());
        glib::idle_add_local_once(move || {
            crate::toast::with_action(
                &format!("{name} asks to manage this machine"),
                crate::toast::Tone::Info,
                "Allow",
                move || allow(&id),
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(online: bool, trusted: bool) -> TailnetDevice {
        TailnetDevice {
            id: "n1".into(),
            name: "Pixel 9".into(),
            os: "android".into(),
            online,
            trusted,
            trusted_at: None,
            last_used: None,
        }
    }

    #[test]
    fn says_how_devices_are() {
        assert_eq!(device_sub(&device(true, false)), "Online · can't manage this machine");
        assert_eq!(device_sub(&device(false, true)), "Offline · trusted");
        assert_eq!(os_icon("android"), "smartphone");
        assert_eq!(os_icon("macOS"), "laptop");
        assert_eq!(os_icon("plan9"), "monitor-smartphone");
    }

    #[test]
    fn key_notes() {
        assert!(key_note(None).is_none());
        assert!(key_note(Some("2000-01-01T00:00:00Z")).unwrap().contains("ran out"));
        assert!(key_note(Some("2999-04-16T03:00:00Z")).unwrap().starts_with("Logs in again by 2999-04-16"));
    }
}
