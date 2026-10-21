//! The app's pages (the web app's src/pages).

pub mod builder;
pub mod home;
pub mod machines;
pub mod secrets;

use gtk::prelude::*;

/// A page's frame: scrolls, centred at most 1440 wide, padded (the web
/// app's Page).
pub fn page(content: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.add_css_class("page");
    inner.set_halign(gtk::Align::Fill);
    inner.append(content);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&inner));
    scroll.upcast()
}

/// A section's heading, with a link at its end.
pub fn section_head(title: &str, cta: &str, go: crate::shell::Page) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("section-head");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("section-title");
    t.set_hexpand(true);
    t.set_xalign(0.0);
    row.append(&t);
    let link = gtk::Button::new();
    link.add_css_class("cta");
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    r.append(&gtk::Label::new(Some(cta)));
    r.append(&wad_gtk::Icon::new("arrow-right", 14));
    link.set_child(Some(&r));
    link.connect_clicked(move |_| crate::shell::go(go));
    row.append(&link);
    row.upcast()
}

/// A small rounded tag (the web app's Badge).
pub fn badge(text: &str, icon: Option<&'static str>, accent: bool) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    b.add_css_class("badge");
    if accent {
        b.add_css_class("accent");
    }
    if let Some(i) = icon {
        b.append(&wad_gtk::Icon::new(i, 12));
    }
    b.append(&gtk::Label::new(Some(text)));
    b.set_valign(gtk::Align::Center);
    b.upcast()
}

/// "5m ago" (the web app's timeAgo).
pub fn time_ago(secs_ago: f64) -> String {
    let s = secs_ago.max(0.0);
    if s < 60.0 {
        return "just now".into();
    }
    let m = s / 60.0;
    if m < 60.0 {
        return format!("{}m ago", m.floor());
    }
    let h = m / 60.0;
    if h < 24.0 {
        return format!("{}h ago", h.floor());
    }
    format!("{}d ago", (h / 24.0).floor())
}

/// "5m ago" from Unix milliseconds (its date once it's a month old).
pub fn ago_ms(ms: f64) -> String {
    let secs = now() - ms / 1000.0;
    if secs < 30.0 * 86_400.0 {
        return time_ago(secs);
    }
    gtk::glib::DateTime::from_unix_local((ms / 1000.0) as i64)
        .ok()
        .and_then(|t| t.format("%x").ok())
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// An ISO time as Unix milliseconds.
pub fn iso_ms(iso: &str) -> Option<f64> {
    gtk::glib::DateTime::from_iso8601(iso, None).ok().map(|t| t.to_unix() as f64 * 1000.0)
}

/// "2h 5m" (the web app's uptime).
pub fn uptime(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Unix seconds from RFC 3339 (what the documents keep).
pub fn secs_of(rfc: &str) -> Option<f64> {
    let (date, time) = rfc.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let (time, _) = time.split_once(['+', '-']).unwrap_or((time, ""));
    let mut t = time.split(':');
    let (h, mi): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: f64 = t.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    // Days from the civil date (Howard Hinnant's).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + h * 3600 + mi * 60) as f64 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_and_say_as_the_web_app_does() {
        assert_eq!(secs_of("1970-01-01T00:00:00.000Z"), Some(0.0));
        assert_eq!(secs_of("2026-10-01T00:00:00Z"), Some(1790812800.0));
        assert_eq!(time_ago(30.0), "just now");
        assert_eq!(time_ago(125.0), "2m ago");
        assert_eq!(time_ago(7300.0), "2h ago");
        assert_eq!(time_ago(3.0 * 86400.0), "3d ago");
        assert_eq!(uptime(7500.0), "2h 5m");
        assert_eq!(uptime(59.0), "0m");
    }
}
