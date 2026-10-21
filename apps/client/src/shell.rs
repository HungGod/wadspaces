//! The app's frame (the web app's components/Shell.tsx): the sidebar (the
//! WadSpaces mark, the nav, background work, Launch, the theme, who's
//! signed in), and the page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use wad_gtk::{Icon, Params, Spring, Theme};

use crate::store::{Topic, store};

/// The sidebar open and narrow, and its spring (motion: 400 / 40).
const WIDE: f64 = 248.0;
const NARROW: f64 = 76.0;
const SIDEBAR_SPRING: Params = Params::new(400.0, 40.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Builder,
    Secrets,
    Machines,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Home, Page::Builder, Page::Secrets, Page::Machines];

    fn label(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Builder => "Builder",
            Page::Secrets => "Secrets",
            Page::Machines => "Machines",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Home => "house",
            Page::Builder => "wand-sparkles",
            Page::Secrets => "key-round",
            Page::Machines => "server",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Builder => "builder",
            Page::Secrets => "secrets",
            Page::Machines => "machines",
        }
    }
}

struct Shell {
    side: gtk::Box,
    side_width: wad_gtk::Width,
    tray: gtk::Box,
    narrow_now: Rc<Cell<bool>>,
    brand: gtk::Box,
    /// What centres when narrow (the nav's rows, the profile).
    centred: Vec<gtk::Widget>,
    stack: gtk::Stack,
    nav: Vec<(Page, gtk::Button, gtk::Label)>,
    labels: Vec<gtk::Widget>,
    brand_text: gtk::Box,
    theme_wide: gtk::Box,
    theme_narrow: gtk::Button,
    profile_text: gtk::Box,
    sign_out: gtk::Button,
    collapse: gtk::Button,
    collapse_icon: Icon,
    launch_label: gtk::Label,
    /// Collapsed by choice (the Builder narrows it without changing that).
    collapsed: Cell<bool>,
    page: Cell<Page>,
    spring: RefCell<Option<Spring>>,
}

pub fn shell() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("backdrop");

    // --- The sidebar.
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("sidebar");
    side.set_hexpand(false);

    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.set_halign(gtk::Align::Fill);
    brand.add_css_class("brand");
    let mark = brand_mark();
    brand.append(&mark);
    let brand_text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    brand_text.add_css_class("lockup");
    brand_text.set_valign(gtk::Align::Center);
    let wad = gtk::Label::new(Some("WAD"));
    wad.add_css_class("lockup-wad");
    wad.set_xalign(0.0);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("lockup-rule");
    let spaces = gtk::Label::new(Some("SPACES"));
    spaces.add_css_class("lockup-spaces");
    spaces.set_xalign(0.0);
    brand_text.append(&wad);
    brand_text.append(&rule);
    brand_text.append(&spaces);
    brand.append(&brand_text);
    side.append(&brand);

    let nav_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    nav_box.add_css_class("nav");
    let mut nav = Vec::new();
    let mut labels: Vec<gtk::Widget> = Vec::new();
    let mut rows: Vec<gtk::Box> = Vec::new();
    for p in Page::ALL {
        let b = gtk::Button::new();
        b.add_css_class("nav-item");
        b.set_tooltip_text(Some(p.label()));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        rows.push(row.clone());
        row.append(&Icon::new(p.icon(), 18));
        let l = gtk::Label::new(Some(p.label()));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&l);
        b.set_child(Some(&row));
        nav_box.append(&b);
        labels.push(l.clone().upcast());
        if p == Page::Builder {
            crate::tour::target("nav-builder", &b);
        }
        nav.push((p, b, l));
    }
    side.append(&nav_box);

    let bottom = gtk::Box::new(gtk::Orientation::Vertical, 12);
    bottom.add_css_class("side-bottom");
    bottom.set_vexpand(true);
    bottom.set_valign(gtk::Align::End);
    let narrow_now = Rc::new(Cell::new(false));
    let tray = crate::activity::tray(narrow_now.clone());
    bottom.append(&tray);

    let launch = gtk::Button::new();
    launch.add_css_class("launch");
    let lrow = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lrow.set_halign(gtk::Align::Center);
    let play = Icon::filled("play", 16);
    lrow.append(&play);
    let launch_label = gtk::Label::new(Some("Launch"));
    lrow.append(&launch_label);
    launch.set_child(Some(&lrow));
    bottom.append(&launch);

    let (theme_wide, theme_narrow) = theme_toggle();
    bottom.append(&theme_wide);
    bottom.append(&theme_narrow);

    let profile = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    profile.add_css_class("profile");
    let avatar = gtk::Label::new(Some("?"));
    avatar.add_css_class("avatar");
    avatar.set_valign(gtk::Align::Center);
    profile.append(&crate::notices::avatar_button(&avatar));
    let profile_text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    profile_text.set_hexpand(true);
    profile_text.set_valign(gtk::Align::Center);
    let who = gtk::Label::new(Some("This machine"));
    who.add_css_class("who");
    who.set_xalign(0.0);
    who.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let sub = gtk::Label::new(None);
    sub.add_css_class("who-sub");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    profile_text.append(&who);
    profile_text.append(&sub);
    profile.append(&profile_text);
    let out = gtk::Button::new();
    out.add_css_class("icon-button");
    out.add_css_class("small");
    out.set_tooltip_text(Some("Sign out"));
    out.update_property(&[gtk::accessible::Property::Label("Sign out")]);
    out.set_child(Some(&Icon::new("log-out", 16)));
    out.set_valign(gtk::Align::Center);
    out.connect_clicked(|_| crate::account::sign_out());
    profile.append(&out);
    bottom.append(&profile);
    side.append(&bottom);

    // Exactly the spring's width, whatever's in it.
    let side_width = wad_gtk::Width::new(&side, WIDE as i32);
    root.append(&side_width);

    // --- The page.
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_hexpand(true);
    content.append(&crate::focus::banner());
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    stack.set_vexpand(true);
    for p in Page::ALL {
        let page = match p {
            Page::Home => crate::pages::home::page(),
            Page::Machines => crate::pages::machines::page(),
            Page::Secrets => crate::pages::secrets::page(),
            Page::Builder => crate::pages::builder::b::page(),
        };
        stack.add_named(&page, Some(p.id()));
    }
    content.append(&stack);
    root.append(&content);

    // The collapse button sits on the sidebar's edge (the spring moves it).
    let frame = gtk::Overlay::new();
    frame.set_child(Some(&root));
    let collapse = gtk::Button::new();
    collapse.add_css_class("collapse");
    collapse.set_halign(gtk::Align::Start);
    collapse.set_valign(gtk::Align::Start);
    let collapse_icon = Icon::new("chevrons-left", 14);
    collapse.set_child(Some(&collapse_icon));
    frame.add_overlay(&collapse);

    let mut centred: Vec<gtk::Widget> = rows.iter().map(|r| r.clone().upcast()).collect();
    centred.push(profile.clone().upcast());
    frame.add_overlay(&crate::modal::layer());
    frame.add_overlay(&crate::toast::host());
    frame.add_overlay(&crate::drag::layer());
    launch.connect_clicked(|_| {
        crate::tour::signal(crate::tour::Event::LauncherOpened);
        crate::launcher::open();
    });
    crate::tour::target("launcher", &launch);
    crate::tour::attach(&frame);
    crate::chrome::attach(&frame);
    let sh = Rc::new(Shell {
        side: side.clone(),
        side_width: side_width.clone(),
        tray,
        narrow_now,
        brand: brand.clone(),
        centred,
        stack,
        nav,
        labels,
        brand_text: brand_text.clone(),
        theme_wide,
        theme_narrow,
        profile_text,
        sign_out: out.clone(),
        collapse: collapse.clone(),
        collapse_icon,
        launch_label,
        collapsed: Cell::new(settings_collapsed()),
        page: Cell::new(Page::Home),
        spring: RefCell::new(None),
    });
    let weak = Rc::downgrade(&sh);
    let spring = Spring::new(&side, SIDEBAR_SPRING, if sh.narrow() { NARROW } else { WIDE }, move |w| {
        if let Some(s) = weak.upgrade() {
            s_width(&s, w);
        }
    });
    *sh.spring.borrow_mut() = Some(spring);

    for (p, b, _) in &sh.nav {
        let (s, p) = (sh.clone(), *p);
        b.connect_clicked(move |_| s.go(p));
    }
    let s = sh.clone();
    collapse.connect_clicked(move |_| {
        s.collapsed.set(!s.collapsed.get());
        save_collapsed(s.collapsed.get());
        s.relayout();
    });
    // Who's signed in: their avatar (initial on their colour), name, @username.
    let colors = gtk::CssProvider::new();
    #[allow(deprecated)]
    avatar.style_context().add_provider(&colors, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    let (out2, side2, who2) = (out.clone(), side.clone(), who.clone());
    store().watch(&who2, Topic::ACCOUNT | Topic::MACHINE | Topic::WADD, move |st| match st.profile() {
        Some(p) => {
            who.set_text(&p.display_name);
            sub.set_text(&format!("@{}", p.username));
            avatar.set_text(&p.display_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
            colors.load_from_string(&format!("label.avatar {{ background: {}; }}", p.color));
            out2.set_visible(!side2.has_css_class("narrow"));
            // New: the tour (resuming where it got to).
            if !p.onboarded {
                gtk::glib::idle_add_local_once(crate::tour::start_once);
            }
        }
        None => {
            who.set_text("This machine");
            sub.set_text(&st.machine.as_ref().map(|m| m.name.clone()).unwrap_or_default());
            avatar.set_text("·");
            out2.set_visible(false);
        }
    });
    sh.go(Page::Home);
    sh.relayout();
    SHELL.with(|s| s.replace(Rc::downgrade(&sh)));
    frame.upcast()
}

thread_local! {
    static SHELL: std::cell::RefCell<std::rc::Weak<Shell>> = const { std::cell::RefCell::new(std::rc::Weak::new()) };
}

/// The page shown.
pub fn page() -> Option<Page> {
    SHELL.with(|s| s.borrow().upgrade()).map(|s| s.page.get())
}

/// Shows a page (the web app's navigate).
pub fn go(p: Page) {
    if let Some(s) = SHELL.with(|s| s.borrow().upgrade()) {
        s.go(p);
    }
}

fn s_width(s: &Shell, w: f64) {
    let w = w.round() as i32;
    s.side_width.set_width(w);
    s.collapse.set_margin_start((w - 12).max(0));
}

impl Shell {
    fn narrow(&self) -> bool {
        self.collapsed.get() || self.page.get() == Page::Builder
    }

    fn go(self: &Rc<Self>, p: Page) {
        let was_narrow = self.narrow();
        self.page.set(p);
        self.stack.set_visible_child_name(p.id());
        for (q, b, _) in &self.nav {
            if *q == p {
                b.add_css_class("active");
            } else {
                b.remove_css_class("active");
            }
        }
        if was_narrow != self.narrow() {
            self.relayout();
        }
        let p2 = p;
        gtk::glib::idle_add_local_once(move || crate::tour::page_changed(p2));
    }

    /// Wide or narrow: the spring moves the width; what only fits wide
    /// goes at once (as the web app's lockup does).
    fn relayout(self: &Rc<Self>) {
        let narrow = self.narrow();
        for l in &self.labels {
            l.set_visible(!narrow);
        }
        self.brand_text.set_visible(!narrow);
        self.theme_wide.set_visible(!narrow);
        self.theme_narrow.set_visible(narrow);
        self.profile_text.set_visible(!narrow);
        self.sign_out.set_visible(!narrow && store().read_now(|s| s.profile().is_some()));
        self.launch_label.set_visible(!narrow);
        self.collapse_icon.set(if narrow { "chevrons-right" } else { "chevrons-left" });
        self.collapse.set_visible(self.page.get() != Page::Builder);
        if narrow {
            self.side.add_css_class("narrow");
        } else {
            self.side.remove_css_class("narrow");
        }
        if self.narrow_now.replace(narrow) != narrow {
            crate::activity::redraw(&self.tray, narrow);
        }
        let along = if narrow { gtk::Align::Center } else { gtk::Align::Fill };
        self.brand.set_halign(if narrow { gtk::Align::Center } else { gtk::Align::Start });
        for w in &self.centred {
            w.set_halign(along);
        }
        if let Some(sp) = self.spring.borrow().as_ref() {
            sp.to(if narrow { NARROW } else { WIDE });
        }
    }
}

/// The WadSpaces mark, in the theme's colours.
fn brand_mark() -> gtk::Widget {
    let svg = |t: Theme| -> &'static [u8] {
        match t {
            Theme::Dark => include_bytes!("../../../assets/public/brand/wadspaces-icon-dark-transparent.svg"),
            Theme::Light => include_bytes!("../../../assets/public/brand/wadspaces-icon-light-transparent.svg"),
        }
    };
    let mark = wad_gtk::Svg::new(svg(wad_gtk::theme::shown()), 44, 44);
    mark.add_css_class("mark");
    let m = mark.clone();
    wad_gtk::theme::on_change(move |t| m.set(svg(t)));
    mark.upcast()
}

/// Dark | Light (wide), or a sun/moon button (narrow).
pub fn theme_toggle() -> (gtk::Box, gtk::Button) {
    let wide = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    wide.add_css_class("theme-toggle");
    wide.set_homogeneous(true);
    let mut buttons = vec![];
    for (t, icon, label) in [(Theme::Dark, "moon", "Dark"), (Theme::Light, "sun", "Light")] {
        let b = gtk::Button::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::Center);
        row.append(&Icon::new(icon, 14));
        row.append(&gtk::Label::new(Some(label)));
        b.set_child(Some(&row));
        b.connect_clicked(move |b| wad_gtk::theme::choose(&WidgetExt::display(b), t));
        wide.append(&b);
        buttons.push((t, b));
    }
    let narrow = gtk::Button::new();
    narrow.add_css_class("icon-button");
    narrow.set_halign(gtk::Align::Center);
    narrow.set_tooltip_text(Some("Toggle theme"));
    let sun_moon = Icon::new("sun", 16);
    narrow.set_child(Some(&sun_moon));
    narrow.connect_clicked(|b| wad_gtk::theme::choose(&WidgetExt::display(b), wad_gtk::theme::shown().other()));
    let show = move |t: Theme| {
        for (bt, b) in &buttons {
            if *bt == t {
                b.add_css_class("on");
            } else {
                b.remove_css_class("on");
            }
        }
        sun_moon.set(if t == Theme::Dark { "sun" } else { "moon" });
    };
    show(wad_gtk::theme::shown());
    wad_gtk::theme::on_change(show);
    (wide, narrow)
}

/// The sidebar's state, kept (the web app's `ws-sidebar`).
fn settings_file() -> std::path::PathBuf {
    wad_gtk::theme::file().with_file_name("sidebar")
}

fn settings_collapsed() -> bool {
    std::fs::read_to_string(settings_file()).is_ok_and(|s| s.trim() == "collapsed")
}

fn save_collapsed(c: bool) {
    let _ = std::fs::write(settings_file(), if c { "collapsed" } else { "open" });
}
