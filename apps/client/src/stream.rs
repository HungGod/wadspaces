//! Streaming a wadspace to a browser (wadd's streams, host/STREAMS.md): the
//! one way the launcher and Home start a stream, the share dialog (the link
//! and its login, a QR code for a phone), stopping one, and the tunnel's
//! state the Stream buttons go by.

use gtk::{glib, prelude::*};
use serde_json::json;
use wad_proto::v1::{Phase, StreamInfo, StreamShare, TunnelStatus};
use wad_wadd_client::Method;

use crate::modal::Modal;
use crate::pages::machines::{act, get, send};
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

/// The tunnel's state, read again.
pub fn load_tunnel() {
    glib::spawn_future_local(async {
        if let Ok(t) = get::<TunnelStatus>("/v1/tunnel".into()).await {
            store().update(|s| {
                s.tunnel = Some(t);
                Topic::STREAMS
            });
        }
    });
}

/// What's streamed, read again (events keep it fresh after that).
pub fn load_streams() {
    glib::spawn_future_local(async {
        if let Ok(list) = get::<Vec<StreamInfo>>("/v1/streams".into()).await {
            store().update(|s| {
                s.streams = list;
                Topic::STREAMS
            });
        }
    });
}

/// The machine can stream (a token, cloudflared connected).
pub fn ready(s: &AppState) -> bool {
    s.tunnel.as_ref().is_some_and(|t| t.ready)
}

/// The stream a wadspace is on, if it is.
pub fn of<'a>(s: &'a AppState, id: &str) -> Option<&'a StreamInfo> {
    s.streams.iter().find(|x| x.workspace == id)
}

/// Streams a wadspace (the launcher's Stream…, Home's Stream): to the
/// Cloud Tunnel tab first if the machine can't stream yet; asked first if
/// it's open here; then its link and login to hand on.
pub fn begin(id: String, name: String) {
    glib::spawn_future_local(async move {
        // The tunnel as it is now, not as it was when the page was drawn.
        if let Ok(t) = get::<TunnelStatus>("/v1/tunnel".into()).await {
            store().update(|s| {
                s.tunnel = Some(t);
                Topic::STREAMS
            });
        }
        let (can, open, streamed) = store().read_now(|s| {
            let open = s.states.get(&id).is_some_and(|st| st.container == "running" && !st.streaming);
            (ready(s), open, of(s, &id).is_some())
        });
        if streamed {
            share(id, name);
            return;
        }
        if !can {
            crate::toast::show(
                "Set up cloud streaming first",
                Some("The machine streams through a Cloudflare tunnel: Secrets → Cloud Tunnel."),
                crate::toast::Tone::Info,
            );
            crate::pages::secrets::show_tab("tunnel");
            return;
        }
        if open {
            take_off_screen(id, name);
        } else {
            start(id, name, false).await;
        }
    });
}

/// It's open on this machine: streaming takes it off the screen (it
/// restarts on its stream). Asked first.
fn take_off_screen(id: String, name: String) {
    let m = Modal::open(&format!("Stream {name}?"), None, 460);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 16);
    let p = ui::wrapping(&format!(
        "{name} is open on this machine. Streaming moves it off this screen: it restarts on its stream, and \
         opening it here again ends the stream."
    ));
    p.add_css_class("small-muted-14");
    col.append(&p);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let go = ui::button("Stream it", Some("radio"), Variant::Primary, Size::Md);
    let mm = m.clone();
    go.connect_clicked(move |_| {
        mm.close();
        let (id, name) = (id.clone(), name.clone());
        glib::spawn_future_local(async move { start(id, name, true).await });
    });
    row.append(&cancel);
    row.append(&go);
    col.append(&row);
    m.body().append(&col);
}

async fn start(id: String, name: String, takeover: bool) {
    let r = async {
        crate::launch::install_here(id.clone()).await?;
        send::<StreamShare>(Method::POST, format!("/v1/workspaces/{id}/stream"), Some(json!({ "takeover": takeover })))
            .await
    }
    .await;
    match r {
        Ok(share) => share_dialog(share, name),
        Err(e) => crate::toast::error(&format!("Couldn't stream {name}"), &e),
    }
}

/// A stream's link and login again (wadd hands the password to this machine
/// only).
pub fn share(id: String, name: String) {
    glib::spawn_future_local(async move {
        match get::<StreamShare>(format!("/v1/streams/{id}/share")).await {
            Ok(share) => share_dialog(share, name),
            Err(e) => crate::toast::error("Couldn't show the stream's login", &e),
        }
    });
}

/// Ends a stream: its wadspace stops, and its link answers nothing.
pub fn stop(id: String, name: String, done: impl FnOnce() + 'static) {
    glib::spawn_future_local(async move {
        match act(Method::DELETE, format!("/v1/workspaces/{id}/stream"), None).await {
            Ok(()) => crate::toast::show("Stream ended", Some(&name), crate::toast::Tone::Info),
            Err(e) => crate::toast::error("Couldn't end the stream", &e),
        }
        done();
    });
}

/// The link and its login, to hand on: a QR code for a phone, each to copy,
/// and how the stream's doing.
pub fn share_dialog(share: StreamShare, name: String) {
    let m = Modal::open(
        &format!("Streaming {name}"),
        Some("Hand these on: the link opens it in any browser, which asks for the username and password."),
        480,
    );
    let col = gtk::Box::new(gtk::Orientation::Vertical, 14);
    col.add_css_class("share");
    let qr = gtk::Image::new();
    qr.add_css_class("qr");
    qr.set_pixel_size(176);
    qr.set_halign(gtk::Align::Center);
    if let Some(t) = wad_gtk::icon::svg_texture(qr_svg(&share.link).as_bytes(), 352) {
        qr.set_paintable(Some(&t));
    }
    col.append(&qr);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
    rows.add_css_class("copy-rows");
    rows.append(&ui::copy_row("Link", &share.link));
    rows.append(&ui::copy_row("Username", &share.user));
    rows.append(&ui::copy_row("Password", &share.password));
    col.append(&rows);
    let state = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    state.add_css_class("share-state");
    col.append(&state);
    let id = share.workspace.clone();
    let weak = state.downgrade();
    store().watch(&state, Topic::STATES | Topic::STREAMS, move |s| {
        let Some(state) = weak.upgrade() else { return };
        ui::clear(&state);
        let up = s.states.get(&id).is_some_and(|st| st.phase == Phase::Ready && st.streaming);
        let stream = of(s, &id);
        if stream.is_none() && !crate::fake::on() {
            state.append(&ui::dot("off"));
            state.append(&gtk::Label::new(Some("Ended")));
        } else if up || crate::fake::on() {
            state.append(&ui::dot("on"));
            let n = stream.map(|x| x.viewers).unwrap_or(0);
            let text = match n {
                0 => "Streaming: nobody watching yet".to_string(),
                1 => "Streaming: 1 watching".to_string(),
                n => format!("Streaming: {n} watching"),
            };
            state.append(&gtk::Label::new(Some(&text)));
        } else {
            state.append(&ui::busy("Starting… the link works once it's up."));
        }
    });
    let idle = store().read_now(|s| s.tunnel.as_ref().map(|t| t.idle_minutes).filter(|m| *m > 0).unwrap_or(30));
    let note = ui::wrapping(&format!(
        "Anyone with the link and password can use this wadspace. It stops after {idle} minutes with nobody watching."
    ));
    note.add_css_class("small-muted");
    col.append(&note);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(4);
    let copy_all = ui::button("Copy all", Some("clipboard-copy"), Variant::Secondary, Size::Md);
    let all = format!("{}\nUsername: {}\nPassword: {}", share.link, share.user, share.password);
    copy_all.connect_clicked(move |b| {
        ui::copy(b, &all);
        ui::set_text(b, "Copied");
        let b = b.clone();
        glib::timeout_add_seconds_local_once(2, move || ui::set_text(&b, "Copy all"));
    });
    row.append(&copy_all);
    let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gap.set_hexpand(true);
    row.append(&gap);
    let end = ui::button("Stop streaming", Some("square"), Variant::Danger, Size::Md);
    let (mm, id, nm) = (m.clone(), share.workspace.clone(), name.clone());
    end.connect_clicked(move |b| {
        b.set_sensitive(false);
        let mm = mm.clone();
        stop(id.clone(), nm.clone(), move || mm.close());
    });
    row.append(&end);
    let done = ui::button("Done", None, Variant::Primary, Size::Md);
    let mm = m.clone();
    done.connect_clicked(move |_| mm.close());
    row.append(&done);
    col.append(&row);
    m.body().append(&col);
}

/// A QR code of `text` (a stream's link), as an SVG document: dark on
/// white whatever the theme, so phones read it.
fn qr_svg(text: &str) -> String {
    use qrcode::render::svg;
    qrcode::QrCode::new(text.as_bytes())
        .map(|q| q.render::<svg::Color>().min_dimensions(176, 176).quiet_zone(true).build())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_links_qr_code_is_an_svg() {
        let svg = super::qr_svg("https://ws-0123456789abcdef0123456789abcdef.wadspaces.com/");
        assert!(svg.starts_with("<?xml") && svg.contains("<svg") && svg.contains("<path"), "{svg}");
    }
}
