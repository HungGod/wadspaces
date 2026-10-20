//! Secrets → Cloud Tunnel: the Cloudflare tunnel this machine streams
//! wadspaces through (host/STREAMS.md). Whether cloudflared runs and is
//! connected, the tunnel's id, a self-test (https://ws-selftest.<domain>
//! from here and back), replacing its token (it's never shown), and what's
//! streamed now.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::json;
use wad_proto::v1::{SelfTest, TunnelStatus};
use wad_wadd_client::Method;

use crate::modal::Modal;
use crate::pages::machines::send;
use crate::store::{AppState, Topic, store};
use crate::stream;
use crate::ui::{self, Size, Variant};

pub fn view() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 20);

    let tunnel = ui::panel(
        "Cloudflare Tunnel",
        Some("cloud"),
        Some("Streams reach browsers through it: https://ws-…wadspaces.com, with no port open on this machine."),
    );
    let test = ui::button("Run self-test", Some("activity"), Variant::Secondary, Size::Sm);
    tunnel.actions.append(&test);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.add_css_class("tunnel-rows");
    tunnel.body.append(&rows);
    let result = gtk::Box::new(gtk::Orientation::Vertical, 0);
    result.add_css_class("tunnel-result");
    tunnel.body.append(&result);
    root.append(&tunnel.root);
    let last: Rc<RefCell<Option<SelfTest>>> = Rc::default();
    {
        let (result, last) = (result.clone(), last.clone());
        test.connect_clicked(move |b| {
            b.set_sensitive(false);
            ui::clear(&result);
            result.append(&ui::busy("Fetching ws-selftest through Cloudflare and back…"));
            let (b, result, last) = (b.clone(), result.clone(), last.clone());
            glib::spawn_future_local(async move {
                let t = send::<SelfTest>(Method::POST, "/v1/tunnel/selftest".into(), Some(json!({}))).await;
                ui::clear(&result);
                match t {
                    Ok(t) => {
                        let text = if t.ok { format!("{} ({} ms)", t.detail, t.ms) } else { t.detail.clone() };
                        result.append(&ui::notice(&text, !t.ok));
                        last.replace(Some(t));
                    }
                    Err(e) => result.append(&ui::notice(&e, true)),
                }
                b.set_sensitive(true);
                stream::load_tunnel();
            });
        });
    }
    let weak = rows.downgrade();
    store().watch(&rows, Topic::STREAMS, move |s| {
        if let Some(rows) = weak.upgrade() {
            draw_tunnel(&rows, s);
        }
    });

    let streams = ui::panel(
        "Streams",
        Some("radio"),
        Some(
            "What's streamed from this machine now. Each has its own link and login, and ends by itself when nobody watches.",
        ),
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("stream-list");
    streams.body.append(&list);
    root.append(&streams.root);
    let weak = list.downgrade();
    store().watch(&list, Topic::STREAMS | Topic::LIBRARY | Topic::WORKSPACES, move |s| {
        if let Some(list) = weak.upgrade() {
            draw_streams(&list, s);
        }
    });

    // Read now and every few seconds while it's shown (cloudflared's state
    // has no event).
    ui::every(&root, 5, || {
        stream::load_tunnel();
        stream::load_streams();
    });
    root.upcast()
}

/// One fact: its name, then what it is.
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

fn text(t: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

/// How the tunnel is, in a word and a dot.
fn status(t: &TunnelStatus) -> (&'static str, String) {
    if !t.cloudflared_installed {
        ("error", "cloudflared isn't on this machine (the image brings it)".into())
    } else if !t.token_present {
        ("error", "No token: replace it below".into())
    } else if t.service != "active" {
        ("error", format!("cloudflared isn't running ({})", t.service))
    } else if !t.connected {
        ("busy", "cloudflared runs, but isn't connected to Cloudflare yet".into())
    } else {
        let links = if t.connections == 1 { "1 link".to_string() } else { format!("{} links", t.connections) };
        ("on", format!("Connected · {links}"))
    }
}

fn draw_tunnel(rows: &gtk::Box, s: &AppState) {
    ui::clear(rows);
    let Some(t) = s.tunnel.as_ref() else {
        rows.append(&ui::busy("Looking…"));
        return;
    };
    let (dot, word) = status(t);
    let st = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    st.append(&ui::dot(dot));
    st.append(&text(&word, "row-title"));
    fact(rows, "Status", &st);
    let id = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    match &t.tunnel_id {
        Some(uuid) => {
            id.append(&text(uuid, "mono-14"));
            let c = ui::icon_button("copy", "Copy the tunnel's id", 14);
            let u = uuid.clone();
            c.connect_clicked(move |b| {
                ui::copy(b, &u);
                ui::set_icon(b, "check");
            });
            id.append(&c);
        }
        None if t.token_present => {
            id.append(&text("In the token (only the machine's wadd can read it)", "small-muted"))
        }
        None => id.append(&text("—", "small-muted")),
    }
    fact(rows, "Tunnel", &id);
    fact(rows, "Streams at", &text(&format!("https://ws-<link>.{}", t.domain), "mono-14"));
    let token = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    token.append(&text(
        if t.token_present { "/etc/cloudflared/token (never shown)" } else { "/etc/cloudflared/token is missing" },
        "small-muted-14",
    ));
    let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gap.set_hexpand(true);
    token.append(&gap);
    let replace = ui::button("Replace token…", Some("key-round"), Variant::Ghost, Size::Sm);
    replace.connect_clicked(|_| replace_token());
    token.append(&replace);
    fact(rows, "Token", &token);
    fact(
        rows,
        "WebRTC",
        &text(
            if t.webrtc {
                "On: WebRTC beside WebSockets (STUN, Cloudflare's TURN)"
            } else {
                "Off: streams use WebSockets ([streaming] webrtc in wadd.toml)"
            },
            "small-muted-14",
        ),
    );
}

fn draw_streams(list: &gtk::Box, s: &AppState) {
    ui::clear(list);
    if s.streams.is_empty() {
        list.append(&text(
            "Nothing is streamed. Stream a wadspace from Launch (Stream…) or from its card on Home.",
            "small-muted-14",
        ));
        return;
    }
    for x in &s.streams {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("stream-row");
        row.append(&ui::dot(if x.live { "on" } else { "busy" }));
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.set_hexpand(true);
        let name = name_of(s, &x.workspace);
        texts.append(&text(&name, "row-title"));
        let watching = match x.viewers {
            0 => "nobody watching".to_string(),
            1 => "1 watching".to_string(),
            n => format!("{n} watching"),
        };
        let sub = text(&format!("{} · {watching}", x.link), "small-muted");
        sub.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        texts.append(&sub);
        row.append(&texts);
        let share = ui::button("Share", Some("link-2"), Variant::Secondary, Size::Sm);
        share.set_valign(gtk::Align::Center);
        let (id, nm) = (x.workspace.clone(), name.clone());
        share.connect_clicked(move |_| stream::share(id.clone(), nm.clone()));
        row.append(&share);
        let stop = ui::button("Stop", Some("square"), Variant::Ghost, Size::Sm);
        stop.set_valign(gtk::Align::Center);
        let (id, nm) = (x.workspace.clone(), name);
        stop.connect_clicked(move |b| {
            b.set_sensitive(false);
            let b = b.clone();
            stream::stop(id.clone(), nm.clone(), move || b.set_sensitive(true));
        });
        row.append(&stop);
        list.append(&row);
    }
}

/// A wadspace's name, from the library (else the machine's).
fn name_of(s: &AppState, id: &str) -> String {
    s.all_wadspaces()
        .into_iter()
        .find(|w| w["id"].as_str() == Some(id))
        .map(|w| s.wadspace_name(w))
        .or_else(|| s.workspaces.iter().find(|w| w.id == id).map(|w| w.name.clone()))
        .unwrap_or_else(|| id.to_string())
}

/// A new tunnel token, pasted from Cloudflare's dashboard.
fn replace_token() {
    let m = Modal::open(
        "Replace the tunnel's token",
        Some(
            "Paste it from Cloudflare (Networks → Tunnels → your tunnel). The whole “cloudflared service install …” \
             line works too. cloudflared restarts with it.",
        ),
        520,
    );
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    col.append(&ui::field_label("Token", None));
    let entry = gtk::PasswordEntry::new();
    entry.add_css_class("input");
    entry.set_show_peek_icon(true);
    entry.set_placeholder_text(Some("eyJhIjoi…"));
    col.append(&entry);
    let err = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.append(&err);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(16);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let save = ui::button("Replace token", None, Variant::Primary, Size::Md);
    save.set_sensitive(false);
    let s2 = save.clone();
    entry.connect_changed(move |e| s2.set_sensitive(!e.text().trim().is_empty()));
    let (mm, e2, err2) = (m.clone(), entry.clone(), err.clone());
    save.connect_clicked(move |b| {
        let token = e2.text().trim().to_string();
        b.set_sensitive(false);
        let (mm, err, b) = (mm.clone(), err2.clone(), b.clone());
        glib::spawn_future_local(async move {
            match send::<TunnelStatus>(Method::PUT, "/v1/tunnel/token".into(), Some(json!({ "token": token }))).await {
                Ok(t) => {
                    store().update(|s| {
                        s.tunnel = Some(t);
                        Topic::STREAMS
                    });
                    crate::toast::show(
                        "Token replaced",
                        Some("cloudflared restarted with it"),
                        crate::toast::Tone::Success,
                    );
                    mm.close();
                }
                Err(e) => {
                    ui::clear(&err);
                    err.append(&ui::notice(&e, true));
                    b.set_sensitive(true);
                }
            }
        });
    });
    row.append(&cancel);
    row.append(&save);
    col.append(&row);
    m.body().append(&col);
    entry.grab_focus();
}
