//! Secrets (the web app's pages/Secrets.tsx): what this machine keeps for
//! its wadspaces to sign in with. Saved sign-ins (each mounted into the
//! wadspaces and WadBrowser setups whose slots pick it), what each signs in
//! to where, renaming, signing out everywhere and deleting; and the
//! machine's tokens. Beside them, the Cloud Tunnel tab (tunnel.rs): what
//! streams go through.

mod tunnel;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use serde_json::Value;
use wad_gtk::Icon;
use wad_proto::v1::{SecretOrigin, SignIn};

use super::{badge, now, time_ago};
use crate::images;
use crate::modal::Modal;
use crate::signins::{self, KeyItem, KeyUser};
use crate::store::{AppState, Topic, store};
use crate::thumb::thumb;
use crate::ui::{self, Size, Variant};

/// The tabs: (value, icon, label).
const TABS: [(&str, Option<&str>, &str); 2] =
    [("signins", Some("key-round"), "Sign-ins"), ("tunnel", Some("cloud"), "Cloud Tunnel")];

struct Page {
    tab: RefCell<&'static str>,
    bar: gtk::Box,
    stack: gtk::Stack,
}

thread_local! {
    static PAGE: RefCell<std::rc::Weak<Page>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Opens Secrets on one of its tabs ("signins", "tunnel").
pub fn show_tab(tab: &'static str) {
    if let Some(p) = PAGE.with(|p| p.borrow().upgrade()) {
        p.tab.replace(tab);
        p.make_tabs();
    }
    crate::shell::go(crate::shell::Page::Secrets);
}

impl Page {
    fn make_tabs(self: &Rc<Self>) {
        ui::clear(&self.bar);
        let p = self.clone();
        let bar = ui::segmented(&TABS, &self.tab.borrow(), move |v| {
            p.tab.replace(TABS.iter().find(|t| t.0 == v).map(|t| t.0).unwrap_or("signins"));
            p.show();
        });
        self.bar.append(&bar);
        self.show();
    }

    fn show(&self) {
        let tab = *self.tab.borrow();
        if self.stack.child_by_name(tab).is_none() {
            let w = if tab == "tunnel" { tunnel::view() } else { sign_ins() };
            self.stack.add_named(&w, Some(tab));
        }
        self.stack.set_visible_child_name(tab);
    }
}

pub fn page() -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Vertical, 0);
    head.set_margin_bottom(24);
    let title = gtk::Label::new(Some("Secrets"));
    title.add_css_class("page-title");
    title.set_xalign(0.0);
    head.append(&title);
    let sub = ui::wrapping(
        "What your wadspaces sign in with, and the tunnel this machine streams them through. Their values never \
         reach this page.",
    );
    sub.add_css_class("page-subtitle");
    head.append(&sub);
    body.append(&head);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.set_margin_bottom(28);
    body.append(&bar);
    let stack = gtk::Stack::new();
    stack.set_vhomogeneous(false);
    stack.set_hhomogeneous(false);
    body.append(&stack);
    let p = Rc::new(Page { tab: RefCell::new("signins"), bar, stack });
    PAGE.with(|x| x.replace(Rc::downgrade(&p)));
    // The tab bar's buttons hold it from here on.
    p.make_tabs();
    super::page(&body)
}

/// The Sign-ins tab: saved sign-ins, then the machine's tokens.
fn sign_ins() -> gtk::Widget {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // The subtitle's long: the button goes under it (the web app's header wraps so).
    let head = gtk::Box::new(gtk::Orientation::Vertical, 16);
    head.set_margin_bottom(24);
    let sub = ui::wrapping(
        "A saved sign-in is mounted into every wadspace whose apps use it: sign in once, in any of them, and the rest \
         are signed in too.",
    );
    sub.add_css_class("small-muted-14");
    head.append(&sub);
    let new = ui::button("New sign-in", Some("plus"), Variant::Primary, Size::Md);
    new.set_halign(gtk::Align::Start);
    new.connect_clicked(|_| new_sign_in());
    head.append(&new);
    body.append(&head);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&content);
    // Held weakly: the watcher goes with the page.
    let weak = content.downgrade();
    store().watch(&content, Topic::SIGNINS | Topic::LIBRARY | Topic::STATES, move |s| {
        if let Some(c) = weak.upgrade() {
            render(&c, s);
        }
    });
    // Read again each time it's shown (sizes change as people sign in).
    body.connect_map(|_| signins::load());
    body.upcast()
}

fn heading(text: &str) -> gtk::Label {
    let h = gtk::Label::new(Some(text));
    h.add_css_class("subhead");
    h.set_xalign(0.0);
    h
}

fn render(content: &gtk::Box, s: &AppState) {
    ui::clear(content);
    let Some(list) = s.signins.as_ref() else {
        content.append(&ui::busy("Looking…"));
        return;
    };
    content.append(&heading("Saved sign-ins"));
    if list.is_empty() {
        let e = ui::empty_state(
            "key-round",
            "No saved sign-ins yet",
            Some(
                "Construct a wadspace in the Builder and its apps' sign-ins are saved here (or make one now). Then \
                 sign in once in any wadspace that uses it.",
            ),
        );
        let b = ui::button("New sign-in", Some("plus"), Variant::Primary, Size::Md);
        b.set_halign(gtk::Align::Center);
        b.set_margin_top(20);
        b.connect_clicked(|_| new_sign_in());
        e.append(&b);
        content.append(&e);
    } else {
        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_column_spacing(16);
        grid.set_row_spacing(16);
        let uses = signins::key_use(s);
        for (n, x) in list.iter().enumerate() {
            let users = signins::merged(uses.get(&x.id).map(Vec::as_slice).unwrap_or_default());
            grid.attach(&card(s, x, &users), (n % 2) as i32, (n / 2) as i32, 1, 1);
        }
        if list.len() == 1 {
            grid.attach(&gtk::Box::new(gtk::Orientation::Vertical, 0), 1, 0, 1, 1);
        }
        content.append(&grid);
    }
    if !s.secrets.is_empty() {
        let h = heading("Tokens");
        h.set_margin_top(40);
        content.append(&h);
        let p = ui::wrapping(
            "Kept on this machine for its wadspaces (github_token lets git reach your repositories). Their values \
             never reach this page.",
        );
        p.add_css_class("small-muted-14");
        p.set_margin_bottom(12);
        content.append(&p);
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rows.add_css_class("token-list");
        for t in &s.secrets {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("token-row");
            let i = Icon::new("key-round", 16);
            i.add_css_class("muted");
            row.append(&i);
            let n = gtk::Label::new(Some(&t.name));
            n.add_css_class("mono-14");
            n.set_hexpand(true);
            n.set_xalign(0.0);
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&n);
            let (text, accent) = match t.origin {
                SecretOrigin::Account => ("From your account", true),
                SecretOrigin::Local => ("Set on this machine", true),
                SecretOrigin::Placeholder => ("Not set", false),
            };
            row.append(&badge(text, None, accent));
            if t.name == "github_token" {
                let b = ui::button("GitHub sign-in", None, Variant::Ghost, Size::Sm);
                b.connect_clicked(github_again);
                row.append(&b);
            }
            rows.append(&row);
        }
        content.append(&rows);
    }
}

/// Signs in to GitHub again, for this machine's token.
fn github_again(b: &gtk::Button) {
    b.set_sensitive(false);
    let b = b.clone();
    glib::spawn_future_local(async move {
        match crate::account::github_again().await {
            Ok(login) => {
                crate::toast::show("Signed in to GitHub", Some(&format!("as {login}")), crate::toast::Tone::Success);
                signins::load();
            }
            Err(e) => crate::toast::error("Couldn't sign in to GitHub", &e),
        }
        b.set_sensitive(true);
    });
}

/// An app's or a site's icon, `px` square (filled in once it's fetched).
fn icon_image(src: &str, px: i32) -> gtk::Image {
    let i = gtk::Image::new();
    i.set_pixel_size(px);
    images::desktop_icon(src, images::later(&i, |i, t| i.set_paintable(Some(&t))));
    i
}

/// A key's picture: the app it's for; the one app it signs in to; or, for
/// several, a folder of theirs, as on a phone.
fn key_icon(items: &[KeyItem], app: &str) -> gtk::Widget {
    let mut icons: Vec<&KeyItem> = vec![];
    for i in items {
        if !icons.iter().any(|x| x.icon == i.icon) {
            icons.push(i);
        }
    }
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("key-icon");
    tile.set_size_request(44, 44);
    tile.set_valign(gtk::Align::Start);
    let pic = |src: &str, px: i32| icon_image(src, px);
    match icons.len() {
        0 => {
            tile.add_css_class("plain");
            let w: gtk::Widget = match signins::kind_icon(app) {
                Some(src) => pic(&src, 20).upcast(),
                None => Icon::new("globe", 20).upcast(),
            };
            w.set_halign(gtk::Align::Center);
            w.set_valign(gtk::Align::Center);
            w.set_vexpand(true);
            tile.append(&w);
        }
        1 => {
            tile.add_css_class("one");
            tile.set_tooltip_text(Some(&icons[0].name));
            let p = pic(&icons[0].icon, 28);
            p.set_halign(gtk::Align::Center);
            p.set_valign(gtk::Align::Center);
            p.set_vexpand(true);
            tile.append(&p);
        }
        n => {
            tile.add_css_class("folder");
            tile.set_tooltip_text(Some(&icons.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")));
            let g = gtk::Grid::new();
            g.set_row_spacing(2);
            g.set_column_spacing(2);
            g.set_halign(gtk::Align::Center);
            g.set_valign(gtk::Align::Center);
            g.set_vexpand(true);
            let shown = if n > 4 { 3 } else { 4 };
            for (k, i) in icons.iter().take(shown).enumerate() {
                g.attach(&pic(&i.icon, 14), (k % 2) as i32, (k / 2) as i32, 1, 1);
            }
            if n > 4 {
                let more = gtk::Label::new(Some(&format!("+{}", n - 3)));
                more.add_css_class("folder-more");
                g.attach(&more, 1, 1, 1, 1);
            }
            tile.append(&g);
        }
    }
    tile.set_vexpand(false);
    tile.upcast()
}

fn card(s: &AppState, x: &SignIn, users: &[KeyUser]) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("key-card");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let items: Vec<KeyItem> = users.iter().flat_map(|u| u.items.iter().cloned()).collect();
    top.append(&key_icon(&items, &x.app));
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    let name_slot = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let name = gtk::Label::new(Some(&x.name));
    name.add_css_class("key-name");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_hexpand(true);
    name_slot.append(&name);
    texts.append(&name_slot);
    let kind = gtk::Label::new(Some(&signins::kind_label(&x.app)));
    kind.add_css_class("small-muted");
    kind.set_xalign(0.0);
    texts.append(&kind);
    top.append(&texts);
    let rename = ui::icon_button("pencil", "Rename", 16);
    let out = ui::icon_button("log-out", "Sign out everywhere", 16);
    out.set_sensitive(x.size > 0);
    let del = ui::icon_button("trash", "Delete", 16);
    {
        let (slot, rename_b, x) = (name_slot.clone(), rename.clone(), x.clone());
        rename.connect_clicked(move |_| renaming(&slot, &rename_b, &x));
    }
    let xo = x.clone();
    out.connect_clicked(move |_| confirm(&xo, true));
    let xd = x.clone();
    del.connect_clicked(move |_| confirm(&xd, false));
    top.append(&rename);
    top.append(&out);
    top.append(&del);
    card.append(&top);

    let info = gtk::Box::new(gtk::Orientation::Vertical, 4);
    info.set_margin_top(12);
    let saved = if x.size > 0 {
        let mut t = format!("{} saved", ui::bytes(x.size));
        if let Some(at) = x.changed_at {
            t.push_str(&format!(", changed {}", time_ago(now() - at as f64 / 1000.0)));
        }
        t
    } else {
        "Not signed in yet".into()
    };
    let l = gtk::Label::new(Some(&saved));
    l.add_css_class("small-muted");
    l.set_xalign(0.0);
    info.append(&l);
    if !x.paths.is_empty() {
        let paths: Vec<String> = x.paths.iter().map(|p| format!("~/{p}")).collect();
        let p = gtk::Label::new(Some(&paths.join(", ")));
        p.add_css_class("mono-muted");
        p.set_xalign(0.0);
        p.set_ellipsize(gtk::pango::EllipsizeMode::End);
        p.set_tooltip_text(Some(&paths.join("\n")));
        info.append(&p);
    }
    card.append(&info);
    if !users.is_empty() {
        card.append(&used_in(s, users, &x.in_use));
    }
    // Installed here but not (or no longer) in a wadspace you have.
    let others: Vec<&String> = x
        .used_by
        .iter()
        .filter(|id| !users.iter().any(|u| u.id == **id || format!("browser-{}", u.id) == **id))
        .collect();
    if !others.is_empty() || users.is_empty() {
        let row = gtk::FlowBox::new();
        row.add_css_class("badges");
        row.set_selection_mode(gtk::SelectionMode::None);
        row.set_column_spacing(6);
        row.set_row_spacing(6);
        row.set_margin_top(12);
        let live = s.all_wadspaces();
        let tag = |t: &str| {
            let b = badge(t, None, false);
            b.set_halign(gtk::Align::Start);
            b
        };
        for id in &others {
            let name = live.iter().find(|w| w["id"].as_str() == Some(id)).map(|w| s.wadspace_name(w));
            row.insert(&tag(&name.unwrap_or_else(|| (*id).clone())), -1);
        }
        if users.is_empty() && others.is_empty() {
            row.insert(&tag("Not used by a wadspace"), -1);
        }
        card.append(&row);
    }
    card.upcast()
}

/// The name, as a field to rename it (Enter or the check saves; X leaves it).
fn renaming(slot: &gtk::Box, rename: &gtk::Button, x: &SignIn) {
    ui::clear(slot);
    rename.set_visible(false);
    let e = ui::entry("");
    e.add_css_class("small");
    e.set_text(&x.name);
    e.set_hexpand(true);
    slot.append(&e);
    let ok = ui::icon_button("check", "Save", 16);
    let no = ui::icon_button("x", "Cancel", 16);
    slot.append(&ok);
    slot.append(&no);
    let go = {
        let (e, id, old) = (e.clone(), x.id.clone(), x.name.clone());
        move || {
            let n = e.text().trim().to_string();
            if n.is_empty() || n == old {
                // Nothing changes: drawn again as it was.
                signins::load();
                return;
            }
            let id = id.clone();
            glib::spawn_future_local(async move {
                if let Err(err) = signins::rename(id, n).await {
                    crate::toast::error("Couldn't rename it", &err);
                    signins::load();
                }
            });
        }
    };
    let g = go.clone();
    e.connect_activate(move |_| g());
    ok.connect_clicked(move |_| go());
    no.connect_clicked(|_| signins::load());
    e.grab_focus();
}

/// The wadspaces (and WadBrowser setups) a key is in, each wired to what it
/// signs in to there.
fn used_in(s: &AppState, users: &[KeyUser], running: &[String]) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    col.set_margin_top(12);
    let h = gtk::Label::new(Some("USED IN"));
    h.add_css_class("tiny-head");
    h.set_xalign(0.0);
    col.append(&h);
    let live = s.all_wadspaces();
    for u in users {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let on = running.iter().any(|r| *r == u.id || *r == format!("browser-{}", u.id));
        row.set_tooltip_text(Some(&format!("{}: {}{}", u.name, u.what, if on { " (in use now)" } else { "" })));
        let w: Option<&Value> =
            if u.kind == "wadspace" { live.iter().find(|w| w["id"].as_str() == Some(&u.id)).copied() } else { None };
        match w {
            Some(w) => {
                let t = thumb(s, w);
                t.set_fixed_width(48);
                t.set_radius(6.0);
                t.add_css_class("mini-thumb");
                t.set_valign(gtk::Align::Center);
                row.append(&t);
            }
            None => {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.add_css_class("mini-browser");
                b.set_size_request(48, 27);
                let svg = wad_gtk::Svg::new(
                    include_bytes!("../../../../../assets/public/brand/wadbrowser-icon-dark.svg"),
                    16,
                    16,
                );
                svg.set_halign(gtk::Align::Center);
                svg.set_valign(gtk::Align::Center);
                svg.set_vexpand(true);
                b.append(&svg);
                b.set_vexpand(false);
                row.append(&b);
            }
        }
        let name = gtk::Label::new(Some(&u.name));
        name.add_css_class("tiny-strong");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(18);
        row.append(&name);
        if on {
            row.append(&ui::dot("on"));
        }
        let wire = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        wire.add_css_class("wire");
        if on {
            wire.add_css_class("live");
        }
        wire.set_hexpand(true);
        wire.set_valign(gtk::Align::Center);
        wire.set_size_request(16, -1);
        row.append(&wire);
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        chip.add_css_class("icon-chip");
        for i in u.items.iter().take(5) {
            let p = icon_image(&i.icon, 16);
            p.set_tooltip_text(Some(&i.name));
            chip.append(&p);
        }
        if u.items.len() > 5 {
            let more = gtk::Label::new(Some(&format!("+{}", u.items.len() - 5)));
            more.add_css_class("tiny-muted");
            chip.append(&more);
        }
        row.append(&chip);
        col.append(&row);
    }
    col.upcast()
}

/// A new saved sign-in (the web app's NewSignIn).
pub fn new_sign_in() {
    let m =
        Modal::open("New saved sign-in", Some("Kept on this machine and shared by every wadspace that picks it."), 500);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    col.append(&ui::field_label("Name", None));
    let name = ui::entry("e.g. Work GitHub");
    col.append(&name);
    let hint = gtk::Label::new(Some("Only a slot of the same kind can use it."));
    let kl = ui::field_label("Kind", Some(&hint));
    kl.set_margin_top(10);
    col.append(&kl);
    let kinds: Vec<&str> = wad_core::signins::SLOT_KINDS.to_vec();
    let labels: Vec<String> = kinds.iter().map(|k| signins::kind_label(k)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let pick = gtk::DropDown::from_strings(&refs);
    pick.add_css_class("select");
    col.append(&pick);
    let keeps = ui::wrapping("");
    keeps.add_css_class("small-muted");
    keeps.set_use_markup(true);
    col.append(&keeps);
    let show = {
        let (keeps, kinds) = (keeps.clone(), kinds.clone());
        move |at: u32| {
            let k = kinds.get(at as usize).copied().unwrap_or("web");
            let files = if k == "web" { None } else { wad_core::signins::paths_for(k) };
            let mut t = glib::markup_escape_text(signins::kind_keeps(k)).to_string();
            if let Some(f) = files {
                let list: Vec<String> = f.iter().map(|p| format!("~/{p}")).collect();
                t.push_str(&format!(": <tt>{}</tt>", glib::markup_escape_text(&list.join(", "))));
            }
            keeps.set_markup(&t);
        }
    };
    show(0);
    pick.connect_selected_notify(move |p| show(p.selected()));
    let err = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.append(&err);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(16);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let save = ui::button("Save sign-in", None, Variant::Primary, Size::Md);
    save.set_sensitive(false);
    let s2 = save.clone();
    name.connect_changed(move |e| s2.set_sensitive(!e.text().trim().is_empty()));
    let go = {
        let (m, name, pick, err, save) = (m.clone(), name.clone(), pick.clone(), err.clone(), save.clone());
        move || {
            let n = name.text().trim().to_string();
            if n.is_empty() {
                return;
            }
            let kind = kinds.get(pick.selected() as usize).copied().unwrap_or("web").to_string();
            let (m, err, save) = (m.clone(), err.clone(), save.clone());
            save.set_sensitive(false);
            glib::spawn_future_local(async move {
                match signins::create(n, kind).await {
                    Ok(s) => {
                        crate::toast::show(
                            "Sign-in saved",
                            Some(&format!(
                                "{}: pick it for a wadspace's slot in the Builder, then sign in once.",
                                s.name
                            )),
                            crate::toast::Tone::Info,
                        );
                        m.close();
                    }
                    // 409: the name is taken.
                    Err(e) => {
                        ui::clear(&err);
                        err.append(&ui::notice(&e, true));
                        save.set_sensitive(true);
                    }
                }
            });
        }
    };
    let g = go.clone();
    name.connect_activate(move |_| g());
    save.connect_clicked(move |_| go());
    row.append(&cancel);
    row.append(&save);
    col.append(&row);
    m.body().append(&col);
    name.grab_focus();
}

/// Sign out everywhere (keeps it) or delete it: both refused while it's in
/// use (the web app's ConfirmSignIn).
fn confirm(x: &SignIn, sign_out: bool) {
    let title = if sign_out { format!("Sign out of {}?", x.name) } else { format!("Delete {}?", x.name) };
    let m = Modal::open(&title, None, 460);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 16);
    let p = ui::wrapping(if sign_out {
        "What it saved (cookies, tokens) is forgotten, so every wadspace using it is signed out. It stays, for signing in again."
    } else {
        "It's forgotten, with what it saved. Pick another for the wadspaces using it in the Builder first."
    });
    p.add_css_class("small-muted-14");
    col.append(&p);
    let err = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.append(&err);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let go = ui::button(
        if sign_out { "Sign out everywhere" } else { "Delete sign-in" },
        Some(if sign_out { "log-out" } else { "trash" }),
        Variant::Danger,
        Size::Md,
    );
    let (mm, id, name) = (m.clone(), x.id.clone(), x.name.clone());
    go.connect_clicked(move |b| {
        b.set_sensitive(false);
        let (mm, id, name, b, err) = (mm.clone(), id.clone(), name.clone(), b.clone(), err.clone());
        glib::spawn_future_local(async move {
            let r = if sign_out { signins::sign_out(id).await } else { signins::delete(id).await };
            match r {
                Ok(()) => {
                    crate::toast::show(
                        if sign_out { "Signed out" } else { "Sign-in deleted" },
                        Some(&name),
                        crate::toast::Tone::Info,
                    );
                    mm.close();
                }
                // 409: a wadspace is running with it, or (delete) still uses it.
                Err(e) => {
                    ui::clear(&err);
                    let n = ui::notice(&e, true);
                    n.set_margin_top(0);
                    err.append(&n);
                    b.set_sensitive(true);
                }
            }
        });
    });
    row.append(&cancel);
    row.append(&go);
    col.append(&row);
    m.body().append(&col);
}
