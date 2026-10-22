//! The site's colours (the web app's src/styles/globals.css), dark or light:
//! as GTK CSS custom properties on every window, so a stylesheet says
//! `var(--accent)`. The choice lives in ~/.config/wadspaces/theme, shared
//! with the HUD; switching swaps the tokens, nothing else reloads.

use std::cell::RefCell;
use std::path::PathBuf;

use gtk::{gdk, gio, prelude::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn parse(s: &str) -> Theme {
        if s.trim() == "light" { Theme::Light } else { Theme::Dark }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn other(self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

/// Where the theme is kept (the HUD reads it too).
pub fn file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("wadspaces/theme")
}

/// The theme now (dark until someone picks). WADSPACES_THEME wins (snapshots).
pub fn current() -> Theme {
    if let Ok(t) = std::env::var("WADSPACES_THEME") {
        return Theme::parse(&t);
    }
    std::fs::read_to_string(file()).map(|s| Theme::parse(&s)).unwrap_or_default()
}

/// Keeps the choice, atomically (the HUD follows the file).
pub fn save(t: Theme) -> std::io::Result<()> {
    let path = file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, t.name())?;
    std::fs::rename(tmp, path)
}

/// globals.css's tokens, for one theme.
pub fn tokens(t: Theme) -> &'static str {
    match t {
        Theme::Dark => {
            "window, window.background {
  --bg: #0a0614;
  --bg-2: #0e0919;
  --surface: #140d22;
  --surface-2: #1b132d;
  --surface-3: #241a39;
  --line: rgb(240 234 250 / 0.09);
  --line-strong: rgb(240 234 250 / 0.18);
  --fg: #f5f2fa;
  --muted: rgb(245 242 250 / 0.62);
  --faint: rgb(245 242 250 / 0.4);
  --accent: #c6ff1f;
  --accent-fg: #0a0614;
  --accent-soft: rgb(198 255 31 / 0.2);
  --accent-2: #ff3d81;
  --accent-2-fg: #0a0614;
  --accent-2-soft: rgb(255 61 129 / 0.24);
  --danger: #ff5d7a;
  --brand: #c6ff1f;
  --brand-rule: #ff3d81;
  --layer-template: #7fd3ff;
  --layer-extensions: #c6ff1f;
  --layer-customization: #ff3d81;
  --c-custom: #ffffff;
  --c-key: #ff3d81;
  --c-ext: #c6ff1f;
  --c-app: #7fd3ff;
  --key-set-fg: #ffc1d6;
  --tpl: #7fd3ff;
  --unbuilt: #ffb36b;
  --glow-ring: rgb(198 255 31 / 0.35);
  --glow-haze: rgb(198 255 31 / 0.25);
  --shadow: rgb(3 2 8 / 0.85);
  --dim: rgb(3 2 8 / 0.62);
}
"
        }
        // Strictly greyscale.
        Theme::Light => {
            "window, window.background {
  --bg: #f4f4f4;
  --bg-2: #ededed;
  --surface: #ffffff;
  --surface-2: #f6f6f6;
  --surface-3: #ebebeb;
  --line: rgb(0 0 0 / 0.09);
  --line-strong: rgb(0 0 0 / 0.18);
  --fg: #0a0a0a;
  --muted: #5f5f5f;
  --faint: #9a9a9a;
  --accent: #0a0a0a;
  --accent-fg: #ffffff;
  --accent-soft: rgb(0 0 0 / 0.07);
  --accent-2: #d9d9d9;
  --accent-2-fg: #0a0a0a;
  --accent-2-soft: rgb(0 0 0 / 0.05);
  --danger: #3a3a3a;
  --brand: #0a0a0a;
  --brand-rule: #0a0a0a;
  --layer-template: #3d3d3d;
  --layer-extensions: #0a0a0a;
  --layer-customization: #7a7a7a;
  --c-custom: #1f1f1f;
  --c-key: #7a7a7a;
  --c-ext: #0a0a0a;
  --c-app: #3d3d3d;
  --key-set-fg: #7a7a7a;
  --tpl: #0a0a0a;
  --unbuilt: #6e6e6e;
  --glow-ring: rgb(0 0 0 / 0.25);
  --glow-haze: rgb(0 0 0 / 0.25);
  --shadow: rgb(0 0 0 / 0.28);
  --dim: rgb(0 0 0 / 0.35);
}
"
        }
    }
}

/// The tokens' provider, the theme shown, and who follows it.
type Shown = (gtk::CssProvider, Theme, Vec<Box<dyn Fn(Theme)>>);

thread_local! {
    static STATE: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// Puts the tokens (and `css`, the app's stylesheet) on the display, in
/// the theme now, and follows the theme file from then on.
pub fn install(display: &gdk::Display, css: &str) {
    let tokens = gtk::CssProvider::new();
    let theme = current();
    tokens.load_from_string(tokens_for(theme));
    gtk::style_context_add_provider_for_display(display, &tokens, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    let app = gtk::CssProvider::new();
    app.load_from_string(css);
    gtk::style_context_add_provider_for_display(display, &app, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    scheme(display, theme);
    STATE.with(|st| *st.borrow_mut() = Some((tokens, theme, vec![])));
    // The HUD or another WadSpaces window may switch it too.
    let f = gio::File::for_path(file());
    if let Ok(m) = f.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        let display = display.clone();
        m.connect_changed(move |_, _, _, _| set(&display, current()));
        // Kept for as long as the app runs.
        std::mem::forget(m);
    }
}

fn tokens_for(t: Theme) -> &'static str {
    tokens(t)
}

/// The theme now, as shown.
pub fn shown() -> Theme {
    STATE.with(|st| st.borrow().as_ref().map(|s| s.1).unwrap_or_default())
}

/// Shows `t` (and tells whoever follows it).
pub fn set(display: &gdk::Display, t: Theme) {
    let changed = STATE.with(|st| {
        let mut st = st.borrow_mut();
        let Some(s) = st.as_mut() else { return false };
        if s.1 == t {
            return false;
        }
        s.0.load_from_string(tokens_for(t));
        s.1 = t;
        true
    });
    if !changed {
        return;
    }
    scheme(display, t);
    STATE.with(|st| {
        if let Some(s) = st.borrow().as_ref() {
            for f in &s.2 {
                f(t);
            }
        }
    });
}

/// Picks `t`: shown, and kept for next time (and the HUD).
pub fn choose(display: &gdk::Display, t: Theme) {
    if let Err(e) = save(t) {
        tracing::warn!("theme: {e}");
    }
    set(display, t);
}

/// Calls `f` whenever the theme changes.
pub fn on_change(f: impl Fn(Theme) + 'static) {
    STATE.with(|st| {
        if let Some(s) = st.borrow_mut().as_mut() {
            s.2.push(Box::new(f));
        }
    });
}

/// GTK's own parts (selections, pickers) in the same scheme, and every
/// window classed `theme-dark` or `theme-light` (for what one theme only has).
fn scheme(display: &gdk::Display, t: Theme) {
    for w in gtk::Window::list_toplevels() {
        mark(&w, t);
    }
    let s = match t {
        Theme::Dark => gtk::InterfaceColorScheme::Dark,
        Theme::Light => gtk::InterfaceColorScheme::Light,
    };
    gtk::Settings::for_display(display).set_gtk_interface_color_scheme(s);
}

/// Classes a window with the theme (and keeps it so for new windows: call
/// it on each new toplevel).
pub fn mark(w: &impl IsA<gtk::Widget>, t: Theme) {
    let w = w.upcast_ref::<gtk::Widget>();
    w.remove_css_class(t.other().class());
    w.add_css_class(t.class());
}

impl Theme {
    /// The CSS class a window has in this theme.
    pub fn class(self) -> &'static str {
        match self {
            Theme::Dark => "theme-dark",
            Theme::Light => "theme-light",
        }
    }
}
