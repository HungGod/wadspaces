//! The WadSpaces HUD: what floats above every window on a machine.
//!
//! - **The bar**, bottom-right: Home (WadSpaces Client on screen, as Super+0),
//!   Wi-Fi, brightness and volume, power and the clock. During a focus
//!   session Home, Wi-Fi and power give way to a timer icon (hover or tap it
//!   for the time left). Its › tucks it away, leaving only an arrow that
//!   brings it back (remembered, prefs.rs).
//! - **Menus** over a dimmed screen: power (restart, shut down) and Wi-Fi
//!   (networks in range, joining one with its password). Brightness and
//!   volume are a panel over the corner, the screen left undimmed.
//! - **The switcher** (Super+Tab), from wadd's `carousel` events.
//! - **The clipboard history** (Super+V, wadd's `shortcut` event): the
//!   machine's copies from every wadspace (wad-clip), one picked to paste.
//!   The HUD also keeps the latest copy alive when whoever copied it goes.
//! - **WadNotes** (notes/): notes, tasks and a calendar side by side at the
//!   top right, for the wadspace on screen (wadd's `view`); reminders at the
//!   top of the screen. One button on the bar puts them away or brings them
//!   back; its menu (hover, or a long press) turns each on or off and picks
//!   the notespace.
//!
//! All are wlr-layer-shell overlays, which sway stacks above normal and
//! fullscreen windows, so they show over whatever wadspace is on screen.
//! Only the Wi-Fi menu (for a password), the clipboard history (arrows,
//! Enter, Esc) and WadNotes (once you click into it) take the keyboard. Without layer-shell
//! (a desktop session) they're plain windows.
//!
//! WADSPACES_HUD_SNAPSHOT=<dir> renders each surface to a PNG there and exits.

mod audio;
mod clips;
mod devices;
mod model;
mod notes;
mod power;
mod prefs;
mod theme;
mod thumb;
mod wadd;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use model::{Carousel, Pick, State, WifiNetwork};
use theme::Theme;
use wad_clip::{Change, Content};
use wadd::{Incoming, Wadd};

/// The WadSpaces logo mark for each theme (assets/public/brand), 128 px.
const LOGO_DARK: &[u8] = include_bytes!("../assets/logo-dark.png");
const LOGO_LIGHT: &[u8] = include_bytes!("../assets/logo-light.png");
/// WadNotes' icon (64 px) and logo (96 px tall), each theme's
/// (assets/public/brand/wadnotes-*.svg).
const NOTES_LOGO_DARK: &[u8] = include_bytes!("../assets/wadnotes-logo-dark.png");
const NOTES_LOGO_LIGHT: &[u8] = include_bytes!("../assets/wadnotes-logo-light.png");

fn texture(bytes: &'static [u8]) -> Option<gdk::Texture> {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)).ok()
}

fn logo(t: Theme) -> Option<gdk::Texture> {
    texture(if t == Theme::Light { LOGO_LIGHT } else { LOGO_DARK })
}

/// A square image drawn at `px` logical pixels (whatever its own size),
/// rounded by the `class` CSS (the box clips it).
fn square_image(texture: &gdk::Texture, px: i32, class: &str) -> gtk::Widget {
    let image = gtk::Image::from_paintable(Some(texture));
    image.set_pixel_size(px);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_size_request(px, px);
    frame.set_halign(gtk::Align::Center);
    frame.set_valign(gtk::Align::Center);
    if !class.is_empty() {
        frame.add_css_class(class);
    }
    frame.append(&image);
    frame.upcast()
}

/// A wadspace's preview in the switcher: its desktop, screen-shaped.
const PREVIEW_W: i32 = 144;
const PREVIEW_H: i32 = 96;

/// The desktop as selkies-desktop lays it out (apps/selkies-desktop,
/// desktop.h; the client's components/desktop/geometry.ts), in its pixels.
const PANEL_H: f64 = 56.0;
const START_W: f64 = 60.0;
const CELL_W: f64 = 96.0;
const CELL_H: f64 = 104.0;
const GRID_PAD: f64 = 10.0;
const DESKTOP_ICON: f64 = 48.0;
/// The preview draws that desktop this much smaller, so its icons show.
const DESKTOP_SCALE: f64 = 0.25;
/// Desktop icons are fetched this big (sharp at 2×).
const DESKTOP_ICON_PX: u32 = 32;

/// A texture as a cairo surface (GDK's default download format is cairo's
/// ARGB32, premultiplied BGRA).
fn surface_of(texture: &gdk::Texture) -> Option<gtk::cairo::ImageSurface> {
    let (tw, th) = (texture.width(), texture.height());
    let stride = tw as usize * 4;
    let mut pixels = vec![0u8; stride * th as usize];
    texture.download(&mut pixels, stride);
    gtk::cairo::ImageSurface::create_for_data(pixels, gtk::cairo::Format::ARgb32, tw, th, stride as i32).ok()
}

/// Straight RGBA as a texture (no PNG on the way).
fn rgba_texture((w, h, pixels): thumb::Rgba) -> gdk::Texture {
    gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(pixels),
        w as usize * 4,
    )
    .upcast()
}

/// A wadspace's desktop, small: its wallpaper (else the image's dark blue)
/// with its icons in selkies-desktop's columns and the panel along the
/// bottom. An icon still on its way is a faint square.
fn desktop_card(wallpaper: Option<&gdk::Texture>, icons: &[Option<gdk::Texture>]) -> gtk::Widget {
    let wall = wallpaper.and_then(surface_of);
    let icons: Vec<Option<gtk::cairo::ImageSurface>> = icons.iter().map(|t| t.as_ref().and_then(surface_of)).collect();
    let area = gtk::DrawingArea::new();
    area.set_content_width(PREVIEW_W);
    area.set_content_height(PREVIEW_H);
    area.set_halign(gtk::Align::Center);
    area.add_css_class("preview");
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // Rounded like the switcher's icons.
        let r = 12.0;
        cr.new_sub_path();
        cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(w - r, h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(r, h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(r, r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.clip();
        match &wall {
            Some(s) => {
                let _ = cr.save();
                cr.scale(w / s.width() as f64, h / s.height() as f64);
                let _ = cr.set_source_surface(s, 0.0, 0.0);
                let _ = cr.paint();
                let _ = cr.restore();
            }
            None => {
                cr.set_source_rgb(11.0 / 255.0, 11.0 / 255.0, 20.0 / 255.0);
                let _ = cr.paint();
            }
        }
        let k = DESKTOP_SCALE;
        let rows = (((h / k - PANEL_H) / CELL_H).floor() as usize).max(1);
        let px = DESKTOP_ICON * k;
        for (n, icon) in icons.iter().enumerate() {
            let x = ((n / rows) as f64 * CELL_W + GRID_PAD + (CELL_W - DESKTOP_ICON) / 2.0) * k;
            let y = ((n % rows) as f64 * CELL_H + GRID_PAD + 5.0) * k;
            if x + px > w {
                break;
            }
            match icon {
                Some(s) => {
                    let _ = cr.save();
                    let side = s.width().max(s.height()) as f64;
                    let (iw, ih) = (s.width() as f64 * px / side, s.height() as f64 * px / side);
                    cr.translate(x + (px - iw) / 2.0, y + (px - ih) / 2.0);
                    cr.scale(px / side, px / side);
                    let _ = cr.set_source_surface(s, 0.0, 0.0);
                    let _ = cr.paint();
                    let _ = cr.restore();
                }
                None => {
                    cr.rectangle(x, y, px, px);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.3);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                }
            }
        }
        // The panel, and its Start button.
        let panel = PANEL_H * k;
        cr.set_source_rgb(26.0 / 255.0, 26.0 / 255.0, 26.0 / 255.0);
        cr.rectangle(0.0, h - panel, w, panel);
        let _ = cr.fill();
        cr.set_source_rgb(64.0 / 255.0, 64.0 / 255.0, 64.0 / 255.0);
        cr.rectangle(4.0 * k, h - panel + 4.0 * k, (START_W - 8.0) * k, panel - 8.0 * k);
        let _ = cr.fill();
    });
    area.upcast()
}

/// What the HUD has of a picture it fetches from wadd.
#[derive(Clone)]
enum Cached {
    Loading,
    /// Not there (or not a picture): asked again after a while.
    Failed(std::time::Instant),
    Ready(gdk::Texture),
}

/// How long a picture wadd didn't give is left alone.
const RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// A query value, percent-encoded.
fn query_value(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The bar sits in the middle of a wadspace's tray (selkies-desktop's
/// panel, 56 px: apps/selkies-desktop/desktop.h): 28 px of pills, 14 px up.
const BAR_BOTTOM: i32 = 14;
/// The bar's icons.
const BAR_ICON: i32 = 14;

/// wadd never sets the backlight lower (apps/wadd/src/screen.rs).
const BRIGHTNESS_FLOOR: f64 = 5.0;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Runs `work` on a thread, then `done` with its result on the GTK loop.
fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    glib::spawn_future_local(async move {
        if let Ok(v) = rx.recv().await {
            done(v);
        }
    });
}

/// Makes `win` an overlay surface above every window (when the compositor can).
fn layer(win: &gtk::Window, namespace: &str, anchors: &[Edge], fill: bool, keyboard: KeyboardMode) {
    if !gtk4_layer_shell::is_supported() {
        return;
    }
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some(namespace));
    win.set_keyboard_mode(keyboard);
    for &edge in anchors {
        win.set_anchor(edge, true);
        if !fill {
            win.set_margin(edge, 12);
        }
    }
    if fill {
        win.set_exclusive_zone(-1);
    }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}

fn button(text: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// Centred over a dimmed screen (a menu).
    Centre,
    /// Over the bar's corner, the screen left as it is (brightness must be
    /// judged by eye).
    Corner,
}

/// A full-screen overlay: a backdrop (a click on it closes) under a card.
fn dimmed(
    app: &gtk::Application,
    title: &str,
    namespace: &str,
    keyboard: KeyboardMode,
    card: &gtk::Box,
    place: Place,
) -> (gtk::Window, gtk::Button) {
    let win = gtk::Window::builder().application(app).title(title).build();
    layer(&win, namespace, &[Edge::Top, Edge::Bottom, Edge::Left, Edge::Right], true, keyboard);
    if !gtk4_layer_shell::is_supported() {
        win.set_default_size(900, 600);
    }
    let overlay = gtk::Overlay::new();
    let dim = gtk::Button::new();
    dim.add_css_class(if place == Place::Corner { "catcher" } else { "dim" });
    dim.set_hexpand(true);
    dim.set_vexpand(true);
    overlay.set_child(Some(&dim));
    card.add_css_class("card");
    match place {
        Place::Centre => {
            card.set_halign(gtk::Align::Center);
            card.set_valign(gtk::Align::Center);
        }
        Place::Corner => {
            card.set_halign(gtk::Align::End);
            card.set_valign(gtk::Align::End);
            card.set_margin_end(12);
            card.set_margin_bottom(56);
        }
    }
    overlay.add_overlay(card);
    win.set_child(Some(&overlay));
    (win, dim)
}

/// Slider moves, sent one at a time: while one is on its way, only the
/// latest waits behind it.
#[derive(Default)]
struct Throttle {
    pending: Cell<Option<u8>>,
    busy: Cell<bool>,
}

impl Throttle {
    fn send(self: &Rc<Self>, wadd: &Wadd, value: u8, work: fn(&Wadd, u8) -> Result<(), String>) {
        self.pending.set(Some(value));
        if self.busy.get() {
            return;
        }
        let Some(v) = self.pending.take() else { return };
        self.busy.set(true);
        let (w, me, again) = (wadd.clone(), self.clone(), wadd.clone());
        background(
            move || work(&w, v),
            move |r| {
                report(r, "slider");
                me.busy.set(false);
                if let Some(next) = me.pending.take() {
                    me.send(&again, next, work);
                }
            },
        );
    }
}

/// One of Adwaita's symbolic icons, coloured by the theme like the text.
fn icon(name: &str, px: i32) -> gtk::Image {
    let i = gtk::Image::from_icon_name(name);
    i.set_pixel_size(px);
    i
}

const BRIGHTNESS_ICON: &str = "display-brightness-symbolic";

/// A bubble on the bar holding just an icon.
fn icon_pill(image: &gtk::Image, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(image));
    b.add_css_class("pill");
    b.add_css_class("icon-pill");
    b.set_tooltip_text(Some(tooltip));
    b
}

/// A slider panel over the corner: brightness, or volume.
struct Level {
    win: gtk::Window,
    row: gtk::Box,
    scale: gtk::Scale,
    value: gtk::Label,
    /// Why it can't be set here (shown instead of the slider).
    why: gtk::Label,
    send: Rc<Throttle>,
}

impl Level {
    /// `lead`: the icon before the slider (the mute button, for volume).
    fn build(app: &gtk::Application, namespace: &str, lead: &gtk::Widget, min: f64) -> (Level, gtk::Button) {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("level");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("slider-row");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_size_request(220, -1);
        let value = label("", "value");
        value.set_width_chars(4);
        value.set_xalign(1.0);
        row.append(lead);
        row.append(&scale);
        row.append(&value);
        let why = label("", "sub");
        why.set_wrap(true);
        why.set_max_width_chars(34);
        why.set_xalign(0.0);
        why.set_visible(false);
        card.append(&row);
        card.append(&why);
        let (win, dim) = dimmed(app, namespace, namespace, KeyboardMode::None, &card, Place::Corner);
        (Level { win, row, scale, value, why, send: Rc::default() }, dim)
    }

    /// What it is now, or why it can't be set.
    fn show(&self, now: Result<u8, String>, syncing: &Cell<bool>) {
        self.row.set_visible(now.is_ok());
        self.why.set_visible(now.is_err());
        match now {
            Ok(p) => {
                syncing.set(true);
                self.scale.set_value(p as f64);
                syncing.set(false);
                self.value.set_label(&format!("{p}%"));
            }
            Err(why) => self.why.set_label(&why),
        }
    }
}

/// The bubble a brightness or volume key shows for a moment.
struct Osd {
    win: gtk::Window,
    icon: gtk::Image,
    bar: gtk::ProgressBar,
    value: gtk::Label,
    hide: RefCell<Option<glib::SourceId>>,
}

/// The bar's tooltips, drawn above it. GTK's own open below their widget,
/// which for a bar on the bottom edge is off the screen (sway doesn't flip
/// a layer surface's popups), so the bar's widgets have no GTK tooltip: their
/// text is the HUD's (`Hud::set_tip`), which this shows.
struct Tip {
    win: gtk::Window,
    card: gtk::Box,
    text: gtk::Label,
    /// The hover's delay.
    pending: RefCell<Option<glib::SourceId>>,
    /// Whose tip is on screen.
    showing: Cell<Option<usize>>,
}

fn tip_key(w: &impl IsA<gtk::Widget>) -> usize {
    w.as_ref().as_ptr() as usize
}

/// Key presses, run one at a time: presses that come meanwhile add up.
#[derive(Default)]
struct Steps {
    pending: Cell<i32>,
    busy: Cell<bool>,
}

/// Why brightness or volume can't be set, in words.
const NO_BRIGHTNESS: &str = "This screen's brightness can't be set from here.";
const NO_SOUND: &str = "No sound output found.";

struct Clip {
    win: gtk::Window,
    list: gtk::ListBox,
    empty: gtk::Label,
    clear: gtk::Button,
    history: RefCell<clips::History>,
    /// sway's clipboard (None: no wlr-data-control, or a snapshot).
    board: RefCell<Option<wad_clip::Clipboard>>,
    /// The clipboard now is the history's latest (not a password).
    current_kept: Cell<bool>,
}

/// From the clipboard's threads to the GTK loop.
enum ClipEvent {
    Copied(Content, Option<Vec<u8>>),
    Cleared,
    Closed(String),
}

struct Wifi {
    win: gtk::Window,
    status: gtk::Label,
    disconnect: gtk::Button,
    list: gtk::Box,
    scanning: gtk::Label,
    ask: gtk::Box,
    ask_label: gtk::Label,
    password: gtk::PasswordEntry,
    asking: RefCell<Option<String>>,
    error: gtk::Label,
    busy: Cell<bool>,
}

/// The WadNotes button's menu, above the bar: each card on or off, and the
/// notespace. Open while the pointer is over the button or the menu (or
/// after a long press, for touch).
struct NotesMenu {
    win: gtk::Window,
    logo: gtk::Picture,
    switches: Vec<(notes::Section, gtk::Switch)>,
    /// The pointer left: it closes soon unless it comes back.
    closing: RefCell<Option<glib::SourceId>>,
    opening: RefCell<Option<glib::SourceId>>,
    /// Set while the HUD moves a switch itself.
    syncing: Cell<bool>,
}

struct Hud {
    wadd: Wadd,
    state: RefCell<State>,
    show_left: Cell<bool>,
    bar: gtk::Window,
    row: gtk::Box,
    done: gtk::Label,
    timer: gtk::Button,
    home: gtk::Button,
    net: gtk::Button,
    bright_button: gtk::Button,
    sound_button: gtk::Button,
    sound_icon: gtk::Image,
    /// What's plugged in, and which wadspaces have it.
    devices_button: gtk::Button,
    devices: Rc<devices::Panel>,
    power_button: gtk::Button,
    clock: gtk::Label,
    battery: gtk::Box,
    battery_icon: gtk::Image,
    battery_text: gtk::Label,
    /// The arrow left when the bar is tucked away.
    expand: gtk::Button,
    power: gtk::Window,
    wifi: Wifi,
    bright: Level,
    sound: Level,
    mute: gtk::Button,
    mute_icon: gtk::Image,
    /// The volume as last heard (None: no sound output).
    volume: Cell<Option<audio::Volume>>,
    /// Set while the HUD moves a slider itself (not a person's change).
    syncing: Cell<bool>,
    osd: Osd,
    tip: Tip,
    /// The bar's tooltips, by widget (they're not GTK's: see Tip).
    tips: RefCell<HashMap<usize, String>>,
    bright_steps: Rc<Steps>,
    sound_steps: Rc<Steps>,
    clip: Clip,
    /// WadNotes, its button on the bar, and the button's menu.
    notes: Rc<notes::Panel>,
    notes_button: gtk::Button,
    notes_menu: NotesMenu,
    switcher: gtk::Window,
    switcher_box: gtk::Box,
    carousel: RefCell<Carousel>,
    icons: RefCell<HashMap<String, Cached>>,
    /// The switcher's cells, for the items they were built for.
    cells: RefCell<Vec<gtk::Box>>,
    built: RefCell<Vec<model::CarouselItem>>,
    /// A picture arrived: the cells are built again.
    cells_stale: Cell<bool>,
    /// The site's colours, dark or light as WadSpaces Client is.
    theme: Cell<Theme>,
    css: gtk::CssProvider,
    logo: RefCell<Option<gdk::Texture>>,
}

impl Hud {
    fn build(
        app: &gtk::Application,
        css: gtk::CssProvider,
        theme: Theme,
        notes_store: wad_notes::Store,
        reminders: wad_notes::ReminderFile,
    ) -> Rc<Hud> {
        let wadd = Wadd::from_env();

        // The bar.
        let bar = gtk::Window::builder().application(app).title("wadspaces-hud").build();
        layer(&bar, "wadspaces-hud", &[Edge::Bottom, Edge::Right], false, KeyboardMode::None);
        bar.set_margin(Edge::Bottom, BAR_BOTTOM);
        // Measured from the screen's edge, over any panel's reserved space
        // (a wadspace's tray is inside its window; this keeps it so anywhere).
        if gtk4_layer_shell::is_supported() {
            bar.set_exclusive_zone(-1);
        }
        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let done = label("Time's up", "pill");
        done.add_css_class("done");
        let timer = button("⏱", &["pill", "focus"]);
        // Home (WadSpaces Client) and WadNotes: plain icons, the theme's colours.
        let home = gtk::Button::new();
        home.set_child(Some(&icon("go-home-symbolic", BAR_ICON)));
        home.add_css_class("pill");
        home.add_css_class("icon-pill");
        home.set_tooltip_text(Some("Back to WadSpaces (Super+0)"));
        let net = button("Wi-Fi", &["pill"]);
        net.set_tooltip_text(Some("Networks"));
        let bright_button = icon_pill(&icon(BRIGHTNESS_ICON, BAR_ICON), "Brightness");
        let sound_icon = icon(audio::icon(None), BAR_ICON);
        let sound_button = icon_pill(&sound_icon, "Volume");
        let devices_button = icon_pill(&icon("media-removable-symbolic", BAR_ICON), "Devices");
        let power_button = icon_pill(&icon("system-shutdown-symbolic", BAR_ICON), "Power");
        // WadNotes: away or back; its menu does the rest.
        let notes_button = gtk::Button::new();
        notes_button.set_child(Some(&icon("document-edit-symbolic", BAR_ICON)));
        notes_button.add_css_class("pill");
        notes_button.add_css_class("icon-pill");
        let clock = label("", "pill");
        clock.add_css_class("clock");
        // The battery: hidden until there is one (power.rs), as on a desktop.
        let battery = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        battery.add_css_class("pill");
        battery.add_css_class("battery");
        let battery_icon = icon("battery-missing-symbolic", BAR_ICON);
        let battery_text = label("", "");
        battery.append(&battery_icon);
        battery.append(&battery_text);
        battery.set_visible(false);
        let collapse = button("›", &["pill", "arrow"]);
        collapse.set_tooltip_text(Some("Hide the bar"));
        for w in [
            done.upcast_ref::<gtk::Widget>(),
            timer.upcast_ref(),
            home.upcast_ref(),
            net.upcast_ref(),
            bright_button.upcast_ref(),
            sound_button.upcast_ref(),
            devices_button.upcast_ref(),
        ] {
            row.append(w);
        }
        row.append(&notes_button);
        for w in
            [power_button.upcast_ref::<gtk::Widget>(), battery.upcast_ref(), clock.upcast_ref(), collapse.upcast_ref()]
        {
            row.append(w);
        }
        let expand = button("‹", &["pill", "arrow"]);
        expand.set_tooltip_text(Some("Show the bar"));
        expand.set_visible(false);
        outer.append(&row);
        outer.append(&expand);
        bar.set_child(Some(&outer));

        // Power.
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.append(&label("Power", "title"));
        card.append(&label("Your wadspaces stop; their files stay.", "sub"));
        let restart = button("↻  Restart", &["menu-button"]);
        let shutdown = button("⏻  Shut down", &["menu-button", "danger"]);
        let cancel = button("Cancel", &["menu-button", "quiet"]);
        card.append(&restart);
        card.append(&shutdown);
        card.append(&cancel);
        for l in [&card.first_child().unwrap(), &card.first_child().unwrap().next_sibling().unwrap()] {
            l.set_halign(gtk::Align::Start);
        }
        let (power, power_dim) =
            dimmed(app, "wadspaces-power", "wadspaces-power", KeyboardMode::None, &card, Place::Centre);

        // Wi-Fi.
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.set_size_request(420, -1);
        let title = label("Wi-Fi", "title");
        title.set_halign(gtk::Align::Start);
        card.append(&title);
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_row.add_css_class("status");
        let status = label("", "");
        status.set_hexpand(true);
        status.set_halign(gtk::Align::Start);
        let disconnect = button("Disconnect", &["small-button"]);
        status_row.append(&status);
        status_row.append(&disconnect);
        card.append(&status_row);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let section = label("NETWORKS", "section");
        section.set_hexpand(true);
        section.set_halign(gtk::Align::Start);
        let scan = button("Scan", &["small-button"]);
        head.append(&section);
        head.append(&scan);
        card.append(&head);
        let scanning = label("Looking for networks…", "sub");
        card.append(&scanning);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(80)
            .max_content_height(300)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        card.append(&scroller);
        let ask = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let ask_label = label("", "sub");
        ask_label.set_halign(gtk::Align::Start);
        let ask_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let password = gtk::PasswordEntry::new();
        password.set_show_peek_icon(true);
        password.set_hexpand(true);
        let join = button("Join", &["small-button", "primary"]);
        ask_row.append(&password);
        ask_row.append(&join);
        ask.append(&ask_label);
        ask.append(&ask_row);
        ask.set_visible(false);
        card.append(&ask);
        let error = label("", "error");
        error.set_wrap(true);
        error.set_halign(gtk::Align::Start);
        error.set_visible(false);
        card.append(&error);
        let close = button("Done", &["menu-button", "quiet"]);
        card.append(&close);
        // The password needs the keyboard: on demand, only while this is open.
        let (wifi_win, wifi_dim) =
            dimmed(app, "wadspaces-wifi", "wadspaces-wifi", KeyboardMode::OnDemand, &card, Place::Centre);

        // Brightness and volume: a panel each, over the corner.
        let sun = icon(BRIGHTNESS_ICON, 18);
        sun.add_css_class("slider-icon");
        let (bright, bright_dim) = Level::build(app, "wadspaces-brightness", sun.upcast_ref(), BRIGHTNESS_FLOOR);
        let mute_icon = icon(audio::icon(None), 18);
        let mute = gtk::Button::new();
        mute.set_child(Some(&mute_icon));
        mute.add_css_class("small-button");
        mute.add_css_class("slider-icon");
        mute.set_tooltip_text(Some("Mute"));
        let (sound, sound_dim) = Level::build(app, "wadspaces-volume", mute.upcast_ref(), 0.0);

        // What a brightness or volume key shows: the level, for a moment.
        let osd_win = gtk::Window::builder().application(app).title("wadspaces-level").build();
        layer(&osd_win, "wadspaces-level", &[Edge::Bottom, Edge::Right], false, KeyboardMode::None);
        if gtk4_layer_shell::is_supported() {
            osd_win.set_margin(Edge::Bottom, 56);
        }
        let osd_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        osd_box.add_css_class("card");
        osd_box.add_css_class("osd");
        let osd_icon = icon(BRIGHTNESS_ICON, 20);
        let osd_bar = gtk::ProgressBar::new();
        osd_bar.set_size_request(180, -1);
        osd_bar.set_valign(gtk::Align::Center);
        let osd_value = label("", "value");
        osd_value.set_width_chars(4);
        osd_value.set_xalign(1.0);
        osd_box.append(&osd_icon);
        osd_box.append(&osd_bar);
        osd_box.append(&osd_value);
        osd_win.set_child(Some(&osd_box));

        // The clipboard history.
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.set_size_request(460, -1);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = label("Clipboard", "title");
        title.set_hexpand(true);
        title.set_halign(gtk::Align::Start);
        let clear = button("Clear all", &["small-button"]);
        head.append(&title);
        head.append(&clear);
        card.append(&head);
        let sub = label("Copies from every wadspace. Pick one, then paste it with Ctrl+V.", "sub");
        sub.set_halign(gtk::Align::Start);
        sub.set_wrap(true);
        card.append(&sub);
        let empty = label("Nothing copied yet.", "sub");
        empty.add_css_class("empty");
        card.append(&empty);
        let clip_list = gtk::ListBox::new();
        clip_list.add_css_class("clips");
        clip_list.set_selection_mode(gtk::SelectionMode::Browse);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(440)
            .propagate_natural_height(true)
            .child(&clip_list)
            .build();
        card.append(&scroller);
        let clip_done = button("Done", &["menu-button", "quiet"]);
        card.append(&clip_done);
        // The keyboard while it's open: arrows, Enter, Delete, Esc.
        let (clip_win, clip_dim) =
            dimmed(app, "wadspaces-clipboard", "wadspaces-clipboard", KeyboardMode::Exclusive, &card, Place::Centre);

        // The switcher.
        let switcher = gtk::Window::builder().application(app).title("wadspaces-switcher").build();
        layer(&switcher, "wadspaces-switcher", &[], false, KeyboardMode::None);
        let switcher_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        switcher_box.add_css_class("switcher");
        switcher.set_child(Some(&switcher_box));

        // WadNotes: the cards at the top right, the reminders, and the
        // button's menu.
        let notes = notes::Panel::build(app, notes_store, reminders);
        let menu_win = gtk::Window::builder().application(app).title("wadspaces-notes-menu").build();
        layer(&menu_win, "wadspaces-notes-menu", &[Edge::Bottom, Edge::Right], false, KeyboardMode::None);
        let menu_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        menu_card.add_css_class("card");
        menu_card.add_css_class("notes-menu");
        menu_card.set_size_request(290, -1);
        // WadNotes' logo (the theme's, apply_theme).
        let notes_logo = gtk::Picture::new();
        notes_logo.set_can_shrink(true);
        notes_logo.set_content_fit(gtk::ContentFit::Contain);
        notes_logo.set_size_request(104, 48);
        notes_logo.set_halign(gtk::Align::Start);
        notes_logo.set_alternative_text(Some("WadNotes"));
        menu_card.append(&notes_logo);
        let hint = label("Tap the button to put them away and bring them back.", "sub");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        menu_card.append(&hint);
        let mut switches = vec![];
        for section in notes::Section::ALL {
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let name = label(section.label(), "row-title");
            name.set_hexpand(true);
            name.set_xalign(0.0);
            let sw = gtk::Switch::new();
            sw.set_valign(gtk::Align::Center);
            r.append(&name);
            r.append(&sw);
            menu_card.append(&r);
            switches.push((section, sw));
        }
        let space = label("NOTESPACE", "section");
        space.set_xalign(0.0);
        space.set_margin_top(6);
        menu_card.append(&space);
        menu_card.append(&notes.scope_box);
        menu_win.set_child(Some(&menu_card));
        // The notespace dropdown opens and closes: the menu follows its size.
        let w = menu_win.downgrade();
        notes.picker.connect_child_revealed_notify(move |_| {
            if let Some(w) = w.upgrade() {
                w.set_default_size(1, 1);
            }
        });

        // The bar's tooltips, above it: made last, so sway stacks them over
        // every other overlay.
        let tip_win = gtk::Window::builder().application(app).title("wadspaces-tip").build();
        layer(&tip_win, "wadspaces-tip", &[Edge::Bottom, Edge::Right], false, KeyboardMode::None);
        let tip_card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tip_card.add_css_class("tip");
        // One line (they're short): its natural width is its width, so the
        // centring in tip_show() is exact.
        let tip_text = label("", "");
        tip_card.append(&tip_text);
        tip_win.set_child(Some(&tip_card));

        let devices = devices::Panel::build(app, wadd.clone());
        let hud = Rc::new(Hud {
            devices,
            devices_button,
            wadd,
            state: RefCell::default(),
            show_left: Cell::new(false),
            bar,
            row,
            done,
            timer,
            home,
            net,
            bright_button,
            sound_button,
            sound_icon,
            power_button,
            clock,
            battery,
            battery_icon,
            battery_text,
            expand,
            power,
            wifi: Wifi {
                win: wifi_win,
                status,
                disconnect,
                list,
                scanning,
                ask,
                ask_label,
                password,
                asking: RefCell::default(),
                error,
                busy: Cell::new(false),
            },
            bright,
            sound,
            mute,
            mute_icon,
            volume: Cell::new(None),
            syncing: Cell::new(false),
            osd: Osd { win: osd_win, icon: osd_icon, bar: osd_bar, value: osd_value, hide: RefCell::default() },
            tip: Tip {
                win: tip_win,
                card: tip_card,
                text: tip_text,
                pending: RefCell::default(),
                showing: Cell::new(None),
            },
            tips: RefCell::default(),
            bright_steps: Rc::default(),
            sound_steps: Rc::default(),
            clip: Clip {
                win: clip_win,
                list: clip_list,
                empty,
                clear,
                history: RefCell::default(),
                board: RefCell::default(),
                current_kept: Cell::new(false),
            },
            notes,
            notes_button,
            notes_menu: NotesMenu {
                win: menu_win,
                logo: notes_logo,
                switches,
                closing: RefCell::default(),
                opening: RefCell::default(),
                syncing: Cell::new(false),
            },
            switcher,
            switcher_box,
            carousel: RefCell::default(),
            icons: RefCell::default(),
            cells: RefCell::default(),
            built: RefCell::default(),
            cells_stale: Cell::default(),
            theme: Cell::new(theme),
            css,
            logo: RefCell::default(),
        });
        hud.apply_theme(theme);

        // Wiring.
        let h = hud.clone();
        hud.timer.connect_clicked(move |_| {
            h.show_left.set(!h.show_left.get());
            h.update_bar();
        });
        let h = hud.clone();
        hud.home.connect_clicked(move |_| {
            let w = h.wadd.clone();
            background(move || w.home(), |r| report(r, "Home"));
        });
        let h = hud.clone();
        hud.net.connect_clicked(move |_| h.open_wifi());
        let h = hud.clone();
        hud.power_button.connect_clicked(move |_| h.power.present());
        let h = hud.clone();
        hud.devices_button.connect_clicked(move |_| h.devices.open());
        for (b, action) in [(&restart, "reboot"), (&shutdown, "poweroff")] {
            let h = hud.clone();
            b.connect_clicked(move |_| {
                h.power.set_visible(false);
                let w = h.wadd.clone();
                background(move || w.power(action), |r| report(r, "Power"));
            });
        }
        for b in [&cancel, &power_dim] {
            let h = hud.clone();
            b.connect_clicked(move |_| h.power.set_visible(false));
        }
        for b in [&close, &wifi_dim] {
            let h = hud.clone();
            b.connect_clicked(move |_| h.close_wifi());
        }
        let h = hud.clone();
        scan.connect_clicked(move |_| h.scan());
        let h = hud.clone();
        hud.wifi.disconnect.connect_clicked(move |_| {
            let w = h.wadd.clone();
            let h2 = h.clone();
            background(
                move || w.wifi_disconnect(),
                move |r| match r {
                    Ok(()) => h2.scan(),
                    Err(e) => h2.wifi_error(&e),
                },
            );
        });
        let h = hud.clone();
        join.connect_clicked(move |_| h.join_asked());
        let h = hud.clone();
        hud.wifi.password.connect_activate(move |_| h.join_asked());

        for w in [
            hud.timer.upcast_ref::<gtk::Widget>(),
            hud.home.upcast_ref(),
            hud.net.upcast_ref(),
            hud.bright_button.upcast_ref(),
            hud.sound_button.upcast_ref(),
            hud.devices_button.upcast_ref(),
            hud.power_button.upcast_ref(),
            hud.battery.upcast_ref(),
            hud.clock.upcast_ref(),
            collapse.upcast_ref(),
            hud.expand.upcast_ref(),
        ] {
            hud.hover_tip(w);
        }
        hud.wire_notes();
        let h = hud.clone();
        collapse.connect_clicked(move |_| h.set_collapsed(true, true));
        let h = hud.clone();
        hud.expand.connect_clicked(move |_| h.set_collapsed(false, true));

        let h = hud.clone();
        hud.bright_button.connect_clicked(move |_| h.open_brightness());
        let h = hud.clone();
        hud.sound_button.connect_clicked(move |_| h.open_volume());
        let h = hud.clone();
        bright_dim.connect_clicked(move |_| h.bright.win.set_visible(false));
        let h = hud.clone();
        sound_dim.connect_clicked(move |_| h.sound.win.set_visible(false));
        let h = hud.clone();
        hud.bright.scale.connect_value_changed(move |s| {
            let v = s.value().round() as u8;
            h.bright.value.set_label(&format!("{v}%"));
            if !h.syncing.get() {
                h.bright.send.send(&h.wadd, v, |w, p| w.set_brightness(p).map(|_| ()));
            }
        });
        let h = hud.clone();
        hud.sound.scale.connect_value_changed(move |s| {
            let v = s.value().round() as u8;
            h.sound.value.set_label(&format!("{v}%"));
            if !h.syncing.get() {
                // Moving it unmutes (audio::set).
                h.show_volume(Some(audio::Volume { percent: v, muted: false }));
                h.sound.send.send(&h.wadd, v, |_, p| audio::set(p));
            }
        });
        let h = hud.clone();
        hud.mute.connect_clicked(move |_| {
            let Some(v) = h.volume.get() else { return };
            let muted = !v.muted;
            let h2 = h.clone();
            background(
                move || audio::set_muted(muted),
                move |r| match r {
                    Ok(()) => h2.show_volume(Some(audio::Volume { muted, ..v })),
                    Err(e) => eprintln!("hud: mute: {e}"),
                },
            );
        });

        for b in [&clip_done, &clip_dim] {
            let h = hud.clone();
            b.connect_clicked(move |_| h.close_clips());
        }
        let h = hud.clone();
        hud.clip.clear.connect_clicked(move |_| {
            h.clip.history.borrow_mut().clear();
            // What's copied now stays copied, but isn't kept alive any more.
            h.clip.current_kept.set(false);
            h.fill_clips();
        });
        let h = hud.clone();
        hud.clip.list.connect_row_activated(move |_, row| {
            if let Ok(id) = row.widget_name().parse() {
                h.pick_clip(id);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let h = hud.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                h.close_clips();
                glib::Propagation::Stop
            }
            gdk::Key::Delete | gdk::Key::BackSpace => {
                if let Some(id) = h.clip.list.selected_row().and_then(|r| r.widget_name().parse().ok()) {
                    h.forget_clip(id);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        hud.clip.win.add_controller(keys);

        hud.update_bar();
        hud.tick();
        hud.bar.present();
        hud
    }

    // -------------------------------------------------------------- theme
    /// Colours and logos for `t`.
    fn apply_theme(self: &Rc<Self>, t: Theme) {
        self.theme.set(t);
        self.css.load_from_string(&theme::css(t));
        // The switcher's WadSpaces entry keeps the logo.
        self.logo.replace(logo(t));
        // WadNotes' mark in its menu, in the theme's colours.
        let light = t == Theme::Light;
        self.notes_menu.logo.set_paintable(texture(if light { NOTES_LOGO_LIGHT } else { NOTES_LOGO_DARK }).as_ref());
        if self.switcher.is_visible() {
            self.show_switcher();
        }
    }

    /// Follows WadSpaces Client's theme file (it rewrites it when you switch).
    fn follow_theme(self: &Rc<Self>) -> Option<gtk::gio::FileMonitor> {
        let path = theme::file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let monitor = gtk::gio::File::for_path(&path)
            .monitor_file(gtk::gio::FileMonitorFlags::WATCH_MOVES, None::<&gtk::gio::Cancellable>)
            .ok()?;
        let h = self.clone();
        monitor.connect_changed(move |_, _, _, _| {
            let t = theme::current();
            if t != h.theme.get() {
                eprintln!("hud: theme {t:?}");
                h.apply_theme(t);
            }
        });
        Some(monitor)
    }

    // ---------------------------------------------------------------- bar
    fn update_bar(&self) {
        let st = self.state.borrow();
        let session = st.session.as_ref();
        let focus = model::locked(session);
        self.timer.set_visible(focus);
        for w in [&self.home, &self.power_button] {
            w.set_visible(!focus);
        }
        let expired = session.is_some_and(|s| s.expired);
        self.done.set_visible(expired);
        // Tucked away, the arrow says time's up.
        if expired {
            self.expand.add_css_class("attention")
        } else {
            self.expand.remove_css_class("attention")
        }
        if let Some(s) = session.filter(|_| focus) {
            let text = model::focus_left(s, now());
            self.set_tip(&self.timer, &text);
            self.timer.set_label(&if self.show_left.get() { format!("⏱  {text}") } else { "⏱".into() });
        }
        match model::network_label(st.network.as_ref()) {
            Some((text, online)) if !focus => {
                self.net.set_visible(true);
                self.net.set_label(&text);
                if online { self.net.remove_css_class("offline") } else { self.net.add_css_class("offline") }
            }
            _ => self.net.set_visible(false),
        }
        if let Some(n) = &st.network {
            let text = match (&n.ssid, n.connectivity.as_deref()) {
                (Some(s), Some("full")) if !s.is_empty() => format!("Connected to {s}"),
                (Some(s), _) if !s.is_empty() => format!("{s}: no internet yet"),
                (_, Some("full")) => "Connected (wired)".into(),
                _ => "Not connected: pick a network".into(),
            };
            self.wifi.status.set_label(&text);
            self.wifi.disconnect.set_visible(n.ssid.as_ref().is_some_and(|s| !s.is_empty()));
        }
    }

    /// The clock (each second; the label only changes each minute).
    fn tick(&self) {
        let Ok(t) = glib::DateTime::now_local() else { return };
        let text = t.format("%-l:%M %p").map(|s| s.trim().to_string()).unwrap_or_default();
        if self.clock.label() != text {
            self.clock.set_label(&text);
            if let Ok(day) = t.format("%A %-e %B %Y") {
                self.set_tip(&self.clock, &day);
            }
        }
    }

    /// `w`'s tooltip, shown above the bar instead of GTK's below it.
    fn hover_tip(self: &Rc<Self>, w: &gtk::Widget) {
        // Its GTK tooltip becomes ours (GTK would show its own below too).
        if let Some(t) = w.tooltip_text() {
            self.set_tip(w, &t);
        }
        w.set_tooltip_text(None);
        w.set_has_tooltip(false);
        let motion = gtk::EventControllerMotion::new();
        let (h, weak) = (self.clone(), w.downgrade());
        motion.connect_enter(move |_, _, _| {
            if let Some(w) = weak.upgrade() {
                h.tip_soon(&w);
            }
        });
        let h = self.clone();
        motion.connect_leave(move |_| h.tip_hide());
        w.add_controller(motion);
        // A press hides it (the button still gets the click).
        let press = gtk::GestureClick::new();
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        let h = self.clone();
        press.connect_pressed(move |_, _, _, _| h.tip_hide());
        w.add_controller(press);
    }

    /// A bar widget's tooltip (live, if it's on screen now).
    fn set_tip(&self, w: &impl IsA<gtk::Widget>, text: &str) {
        let key = tip_key(w);
        self.tips.borrow_mut().insert(key, text.to_string());
        if self.tip.showing.get() == Some(key) && self.tip.win.is_visible() {
            self.tip.text.set_label(text);
        }
    }

    fn tip_soon(self: &Rc<Self>, w: &gtk::Widget) {
        self.tip_hide();
        let (h, weak) = (self.clone(), w.downgrade());
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
            h.tip.pending.replace(None);
            if let Some(w) = weak.upgrade() {
                h.tip_show(&w);
            }
        });
        self.tip.pending.replace(Some(id));
    }

    fn tip_hide(&self) {
        if let Some(id) = self.tip.pending.take() {
            id.remove();
        }
        self.tip.win.set_visible(false);
        self.tip.showing.set(None);
    }

    /// Centred over `w`, just above the bar.
    fn tip_show(&self, w: &gtk::Widget) {
        let key = tip_key(w);
        let Some(text) = self.tips.borrow().get(&key).filter(|t| !t.is_empty()).cloned() else { return };
        self.tip.text.set_label(&text);
        self.tip.showing.set(Some(key));
        if gtk4_layer_shell::is_supported() {
            let at = w.compute_point(&self.bar, &gtk::graphene::Point::new(0.0, 0.0));
            let (_, natural, _, _) = self.tip.card.measure(gtk::Orientation::Horizontal, -1);
            let centre = at.map(|p| p.x() + w.width() as f32 / 2.0).unwrap_or(self.bar.width() as f32 / 2.0);
            // The bar sits 12 px from the right and bottom edges (layer()).
            let right = 12.0 + self.bar.width() as f32 - centre - natural as f32 / 2.0;
            self.tip.win.set_margin(Edge::Right, right.max(6.0) as i32);
            self.tip.win.set_margin(Edge::Bottom, BAR_BOTTOM + self.bar.height() + 6);
        }
        self.tip.win.set_default_size(1, 1);
        self.tip.win.present();
    }

    /// The battery bubble (every few seconds; sysfs is cheap to read).
    fn show_battery(&self, b: Option<power::Battery>) {
        self.battery.set_visible(b.is_some());
        let Some(b) = b else { return };
        self.battery_icon.set_icon_name(Some(&power::icon(&b)));
        self.battery_text.set_label(&format!("{}%", b.percent));
        self.set_tip(&self.battery, &power::describe(&b));
        if power::low(&b) { self.battery.add_css_class("low") } else { self.battery.remove_css_class("low") }
    }

    /// Tucks the bar away behind its arrow, or brings it back. WadNotes goes
    /// and comes back with it (the same sections as before).
    fn set_collapsed(&self, collapsed: bool, remember: bool) {
        self.tip_hide();
        self.row.set_visible(!collapsed);
        self.expand.set_visible(collapsed);
        // Shrink to what's left, so the empty corner takes no clicks.
        self.bar.set_default_size(1, 1);
        self.notes.set_hidden(collapsed);
        if collapsed {
            self.notes_menu.win.set_visible(false);
        }
        if remember {
            prefs::update(|p| p.collapsed = collapsed);
        }
    }

    // ------------------------------------------------------------ WadNotes
    fn wire_notes(self: &Rc<Self>) {
        // A tap: away, or back (with none on, the menu, to turn one on).
        let h = self.clone();
        self.notes_button.connect_clicked(move |_| {
            if !h.notes.any_on() {
                h.open_notes_menu();
                return;
            }
            let away = !h.notes.is_put_away();
            h.notes.set_put_away(away);
            prefs::update(|p| p.notes_away = away);
            h.update_notes_button();
        });
        // Hover opens the menu; leaving the button and the menu closes it.
        for w in [self.notes_button.upcast_ref::<gtk::Widget>(), self.notes_menu.win.upcast_ref()] {
            let motion = gtk::EventControllerMotion::new();
            let h = self.clone();
            let is_button = w.is::<gtk::Button>();
            motion.connect_enter(move |_, _, _| {
                h.cancel_notes_menu_close();
                if is_button {
                    h.open_notes_menu_soon();
                }
            });
            let h = self.clone();
            motion.connect_leave(move |_| h.close_notes_menu_soon());
            w.add_controller(motion);
        }
        // Touch has no hover: a long press opens it.
        let long = gtk::GestureLongPress::new();
        long.set_touch_only(true);
        let h = self.clone();
        long.connect_pressed(move |g, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            h.open_notes_menu();
        });
        self.notes_button.add_controller(long);
        for (section, sw) in &self.notes_menu.switches {
            let (h, section) = (self.clone(), *section);
            sw.connect_active_notify(move |sw| {
                if !h.notes_menu.syncing.get() {
                    h.show_section(section, sw.is_active());
                    let on = sw.is_active();
                    prefs::update(|p| {
                        match section {
                            notes::Section::Notes => p.notes = on,
                            notes::Section::Tasks => p.tasks = on,
                            notes::Section::Calendar => p.calendar = on,
                        }
                        p.notes_away = false;
                    });
                }
            });
        }
        self.update_notes_button();
    }

    /// A section on or off: its card, its switch, and the button.
    fn show_section(&self, s: notes::Section, on: bool) {
        self.notes.set_section(s, on);
        self.notes_menu.syncing.set(true);
        for (section, sw) in &self.notes_menu.switches {
            if *section == s {
                sw.set_active(on);
            }
        }
        self.notes_menu.syncing.set(false);
        self.update_notes_button();
    }

    /// Lit while WadNotes is on screen.
    fn update_notes_button(&self) {
        if self.notes.showing() {
            self.notes_button.add_css_class("on")
        } else {
            self.notes_button.remove_css_class("on")
        }
    }

    fn open_notes_menu_soon(self: &Rc<Self>) {
        if self.notes_menu.win.is_visible() || self.notes_menu.opening.borrow().is_some() {
            return;
        }
        let h = self.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            h.notes_menu.opening.replace(None);
            h.open_notes_menu();
        });
        self.notes_menu.opening.replace(Some(id));
    }

    fn cancel_notes_menu_close(&self) {
        if let Some(id) = self.notes_menu.closing.take() {
            id.remove();
        }
    }

    fn close_notes_menu_soon(self: &Rc<Self>) {
        if let Some(id) = self.notes_menu.opening.take() {
            id.remove();
        }
        self.cancel_notes_menu_close();
        let h = self.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
            h.notes_menu.closing.replace(None);
            h.notes_menu.win.set_visible(false);
            h.notes.picker.set_reveal_child(false);
        });
        self.notes_menu.closing.replace(Some(id));
    }

    /// The menu, right-aligned over the button, just above the bar.
    fn open_notes_menu(self: &Rc<Self>) {
        self.cancel_notes_menu_close();
        self.notes.fill_picker();
        let win = &self.notes_menu.win;
        if gtk4_layer_shell::is_supported() {
            let b = &self.notes_button;
            let at = b.compute_point(&self.bar, &gtk::graphene::Point::new(0.0, 0.0));
            let right_edge = at.map(|p| p.x() + b.width() as f32).unwrap_or(self.bar.width() as f32);
            // The bar sits 12 px from the right and bottom edges (layer()).
            win.set_margin(Edge::Right, (12.0 + self.bar.width() as f32 - right_edge).max(6.0) as i32);
            win.set_margin(Edge::Bottom, BAR_BOTTOM + self.bar.height() + 6);
        }
        win.set_default_size(1, 1);
        win.present();
    }

    /// WadNotes follows the screen: the wadspace on it has its notespace.
    fn follow_view(&self) {
        let st = self.state.borrow();
        let scope = model::scope_for(st.view.as_deref(), &st.workspaces);
        let spaces = model::notespaces(&st.workspaces);
        drop(st);
        self.notes.set_view(scope, spaces);
    }

    // ------------------------------------------------- brightness, volume
    fn open_brightness(self: &Rc<Self>) {
        self.sound.win.set_visible(false);
        self.bright.win.present();
        let w = self.wadd.clone();
        let h = self.clone();
        background(
            move || w.brightness(),
            move |r| {
                let now = match r {
                    Ok(b) if b.available => Ok(b.percent),
                    Ok(_) => Err(NO_BRIGHTNESS.to_string()),
                    Err(e) => {
                        eprintln!("hud: brightness: {e}");
                        Err(NO_BRIGHTNESS.to_string())
                    }
                };
                h.bright.show(now, &h.syncing);
            },
        );
    }

    fn open_volume(self: &Rc<Self>) {
        self.bright.win.set_visible(false);
        self.sound.win.present();
        let h = self.clone();
        background(audio::get, move |r| h.heard_volume(r));
    }

    fn heard_volume(&self, r: Result<audio::Volume, String>) {
        if let Err(e) = &r {
            eprintln!("hud: volume: {e}");
        }
        self.show_volume(r.as_ref().ok().copied());
        self.sound.show(r.map(|v| v.percent).map_err(|_| NO_SOUND.to_string()), &self.syncing);
    }

    /// The volume's icons (the bubble's and the mute button's).
    fn show_volume(&self, v: Option<audio::Volume>) {
        self.volume.set(v);
        let name = audio::icon(v);
        self.sound_icon.set_icon_name(Some(name));
        self.mute_icon.set_icon_name(Some(name));
        let muted = v.is_some_and(|v| v.muted);
        self.mute.set_tooltip_text(Some(if muted { "Unmute" } else { "Mute" }));
        if muted { self.sound.row.add_css_class("muted") } else { self.sound.row.remove_css_class("muted") }
    }

    /// A brightness or volume key (sway runs `hud --key <name>`, which hands
    /// it to this HUD): the change, then its level for a moment.
    fn on_key(self: &Rc<Self>, key: &str) {
        match key {
            "brightness-up" => self.step_brightness(10),
            "brightness-down" => self.step_brightness(-10),
            "volume-up" => self.step_volume(5),
            "volume-down" => self.step_volume(-5),
            "volume-mute" => {
                let h = self.clone();
                background(audio::toggle_mute, move |r| h.volume_stepped(r));
            }
            other => eprintln!("hud: no such key {other:?}"),
        }
    }

    fn step_brightness(self: &Rc<Self>, delta: i32) {
        let steps = self.bright_steps.clone();
        steps.pending.set(steps.pending.get() + delta);
        if steps.busy.replace(true) {
            return;
        }
        let delta = steps.pending.replace(0);
        let w = self.wadd.clone();
        let h = self.clone();
        background(
            move || {
                let b = w.brightness()?;
                if !b.available {
                    return Ok(b);
                }
                w.set_brightness((b.percent as i32 + delta).clamp(0, 100) as u8)
            },
            move |r| {
                h.bright_steps.busy.set(false);
                let now = match r {
                    Ok(b) if b.available => Ok(b.percent),
                    _ => Err(NO_BRIGHTNESS.to_string()),
                };
                if h.bright.win.is_visible() {
                    h.bright.show(now.clone(), &h.syncing);
                }
                h.flash(BRIGHTNESS_ICON, now);
                if h.bright_steps.pending.get() != 0 {
                    h.step_brightness(0);
                }
            },
        );
    }

    fn step_volume(self: &Rc<Self>, delta: i32) {
        let steps = self.sound_steps.clone();
        steps.pending.set(steps.pending.get() + delta);
        if steps.busy.replace(true) {
            return;
        }
        let delta = steps.pending.replace(0);
        let h = self.clone();
        background(
            move || audio::step(delta),
            move |r| {
                h.sound_steps.busy.set(false);
                h.volume_stepped(r);
                if h.sound_steps.pending.get() != 0 {
                    h.step_volume(0);
                }
            },
        );
    }

    fn volume_stepped(self: &Rc<Self>, r: Result<audio::Volume, String>) {
        let v = r.as_ref().ok().copied();
        if self.sound.win.is_visible() {
            self.heard_volume(r);
        } else {
            self.show_volume(v);
        }
        let now = match v {
            Some(v) if v.muted => Ok(0),
            Some(v) => Ok(v.percent),
            None => Err(NO_SOUND.to_string()),
        };
        self.flash(audio::icon(v), now);
    }

    /// The level bubble, for a moment.
    fn flash(self: &Rc<Self>, icon_name: &str, now: Result<u8, String>) {
        self.osd.icon.set_icon_name(Some(icon_name));
        match now {
            Ok(p) => {
                self.osd.bar.set_visible(true);
                self.osd.bar.set_fraction(p as f64 / 100.0);
                self.osd.value.set_label(&format!("{p}%"));
                self.osd.value.set_width_chars(4);
            }
            Err(why) => {
                self.osd.bar.set_visible(false);
                self.osd.value.set_label(&why);
                self.osd.value.set_width_chars(-1);
            }
        }
        self.osd.win.set_default_size(1, 1);
        self.osd.win.present();
        if let Some(old) = self.osd.hide.take() {
            old.remove();
        }
        let h = self.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            h.osd.hide.replace(None);
            h.osd.win.set_visible(false);
        });
        self.osd.hide.replace(Some(id));
    }

    // ------------------------------------------------------------ clipboard
    /// Follows sway's clipboard: every copy into the history.
    fn start_clipboard(self: &Rc<Self>) {
        let (tx, rx) = async_channel::unbounded();
        let socket = wad_clip::wl::env_socket();
        let watched = wad_clip::wl::watch(&socket, "hud", move |c| {
            let ev = match c {
                Change::Copied(c) => {
                    // Made here, off the GTK loop: decoding a screenshot takes a moment.
                    let thumb = c.image().and_then(|(_, bytes)| thumb::fit(bytes, 192, 96));
                    ClipEvent::Copied(c, thumb)
                }
                Change::Cleared => ClipEvent::Cleared,
                Change::Closed(why) => ClipEvent::Closed(why),
            };
            let _ = tx.send_blocking(ev);
        });
        match watched {
            Ok(board) => self.clip.board.replace(Some(board)),
            Err(e) => {
                eprintln!("hud: no clipboard history: {e}");
                return;
            }
        };
        let h = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                h.on_clip(ev);
            }
        });
    }

    fn on_clip(self: &Rc<Self>, ev: ClipEvent) {
        match ev {
            ClipEvent::Copied(c, thumb) => {
                let kept = self.clip.history.borrow_mut().add(c, thumb, now());
                self.clip.current_kept.set(kept);
                if self.clip.win.is_visible() {
                    self.fill_clips();
                }
            }
            // Whoever copied it went away (a wadspace stopped, an app
            // closed): copy it again, so it can still be pasted.
            ClipEvent::Cleared => {
                if self.clip.current_kept.get()
                    && let (Some(e), Some(board)) = (self.clip.history.borrow().latest(), &*self.clip.board.borrow())
                {
                    board.set(e.content.clone());
                }
            }
            ClipEvent::Closed(why) => {
                eprintln!("hud: the clipboard went away: {why}");
                self.clip.board.replace(None);
            }
        }
    }

    fn toggle_clips(self: &Rc<Self>) {
        if self.clip.win.is_visible() {
            self.close_clips();
            return;
        }
        self.fill_clips();
        self.clip.win.present();
        if let Some(first) = self.clip.list.row_at_index(0) {
            first.grab_focus();
        }
    }

    fn close_clips(&self) {
        self.clip.win.set_visible(false);
    }

    fn fill_clips(self: &Rc<Self>) {
        while let Some(row) = self.clip.list.first_child() {
            self.clip.list.remove(&row);
        }
        let history = self.clip.history.borrow();
        let none = history.entries().is_empty();
        self.clip.empty.set_visible(none);
        self.clip.list.set_visible(!none);
        self.clip.clear.set_sensitive(!none);
        let t = now();
        for e in history.entries() {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("clip");
            row.set_widget_name(&e.id.to_string());
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let texture = e.thumb.as_ref().and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from(b)).ok());
            let face: gtk::Widget = match texture {
                Some(tex) => {
                    // At its own (thumbnail) size, never stretched to the row.
                    let pic = gtk::Picture::for_paintable(&tex);
                    pic.set_can_shrink(false);
                    pic.set_content_fit(gtk::ContentFit::ScaleDown);
                    pic.set_size_request(tex.width(), tex.height());
                    pic.set_halign(gtk::Align::Start);
                    pic.set_valign(gtk::Align::Center);
                    pic.add_css_class("thumb");
                    pic.upcast()
                }
                None => {
                    let text = label(&e.label(), "text");
                    text.set_wrap(true);
                    text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    text.set_lines(3);
                    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    text.set_max_width_chars(42);
                    text.set_xalign(0.0);
                    text.upcast()
                }
            };
            face.set_hexpand(true);
            line.append(&face);
            let side = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let when = label(&clips::ago(t - e.at), "when");
            when.set_halign(gtk::Align::End);
            side.append(&when);
            let forget = button("✕", &["forget"]);
            forget.set_tooltip_text(Some("Remove from the history"));
            forget.set_halign(gtk::Align::End);
            let (h, id) = (self.clone(), e.id);
            forget.connect_clicked(move |_| h.forget_clip(id));
            side.append(&forget);
            line.append(&side);
            row.set_child(Some(&line));
            self.clip.list.append(&row);
        }
        drop(history);
        if let Some(first) = self.clip.list.row_at_index(0) {
            self.clip.list.select_row(Some(&first));
        }
    }

    fn forget_clip(self: &Rc<Self>, id: u64) {
        let was_latest = self.clip.history.borrow().latest().is_some_and(|e| e.id == id);
        self.clip.history.borrow_mut().remove(id);
        if was_latest {
            self.clip.current_kept.set(false);
        }
        self.fill_clips();
        if let Some(first) = self.clip.list.row_at_index(0) {
            first.grab_focus();
        }
    }

    /// The pick becomes the clipboard, in every wadspace (their bridges copy it in).
    fn pick_clip(self: &Rc<Self>, id: u64) {
        let picked = self.clip.history.borrow_mut().promote(id);
        if let (Some(c), Some(board)) = (picked, &*self.clip.board.borrow()) {
            board.set(c);
            self.clip.current_kept.set(true);
        }
        self.close_clips();
    }

    // --------------------------------------------------------------- Wi-Fi
    fn open_wifi(self: &Rc<Self>) {
        self.wifi.error.set_visible(false);
        self.wifi.ask.set_visible(false);
        self.wifi.asking.replace(None);
        self.wifi.win.present();
        self.scan();
    }

    fn close_wifi(&self) {
        self.wifi.password.set_text("");
        self.wifi.win.set_visible(false);
    }

    fn wifi_error(&self, e: &str) {
        self.wifi.error.set_label(&model::join_error(e));
        self.wifi.error.set_visible(true);
    }

    fn scan(self: &Rc<Self>) {
        self.wifi.scanning.set_visible(true);
        let w = self.wadd.clone();
        let h = self.clone();
        background(
            move || w.wifi_scan(),
            move |r| {
                h.wifi.scanning.set_visible(false);
                match r {
                    Ok(list) => h.show_networks(&list),
                    Err(e) => h.wifi_error(&e),
                }
            },
        );
    }

    fn show_networks(self: &Rc<Self>, list: &[WifiNetwork]) {
        while let Some(c) = self.wifi.list.first_child() {
            self.wifi.list.remove(&c);
        }
        if list.is_empty() {
            self.wifi.list.append(&label("No networks in range.", "sub"));
        }
        for n in list {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let bars = label(model::bars(n.signal), "bars");
            bars.set_xalign(0.0);
            row.append(&bars);
            let name = label(&n.ssid, "");
            name.set_hexpand(true);
            name.set_halign(gtk::Align::Start);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&name);
            let tag = if n.active {
                "connected"
            } else if !n.supported {
                "can't join"
            } else if n.known {
                "saved"
            } else if n.secure {
                "secured"
            } else {
                "open"
            };
            row.append(&label(tag, "tag"));
            let b = gtk::Button::new();
            b.set_child(Some(&row));
            b.add_css_class("net-row");
            if n.active {
                b.add_css_class("active");
            }
            let h = self.clone();
            let net = n.clone();
            b.connect_clicked(move |_| h.pick(&net));
            self.wifi.list.append(&b);
        }
    }

    fn pick(self: &Rc<Self>, n: &WifiNetwork) {
        if self.wifi.busy.get() {
            return;
        }
        self.wifi.error.set_visible(false);
        match model::pick(n) {
            Pick::Nothing => {}
            Pick::Refuse(why) => self.wifi_error(&why),
            Pick::AskPassword => {
                self.wifi.asking.replace(Some(n.ssid.clone()));
                self.wifi.ask_label.set_label(&format!("Password for {}", n.ssid));
                self.wifi.password.set_text("");
                self.wifi.ask.set_visible(true);
                self.wifi.password.grab_focus();
            }
            Pick::Join => self.join(&n.ssid, None),
        }
    }

    fn join_asked(self: &Rc<Self>) {
        let Some(ssid) = self.wifi.asking.borrow().clone() else { return };
        let pw = self.wifi.password.text().to_string();
        if pw.len() < 8 {
            return self.wifi_error("Wi-Fi passwords are at least 8 characters.");
        }
        self.join(&ssid, Some(pw));
    }

    fn join(self: &Rc<Self>, ssid: &str, password: Option<String>) {
        self.wifi.busy.set(true);
        self.wifi.scanning.set_label(&format!("Joining {ssid}…"));
        self.wifi.scanning.set_visible(true);
        let w = self.wadd.clone();
        let h = self.clone();
        let ssid = ssid.to_string();
        background(
            move || w.wifi_connect(&ssid, password.as_deref()),
            move |r| {
                h.wifi.busy.set(false);
                h.wifi.scanning.set_label("Looking for networks…");
                h.wifi.scanning.set_visible(false);
                match r {
                    Ok(()) => {
                        h.wifi.ask.set_visible(false);
                        h.wifi.asking.replace(None);
                        h.wifi.password.set_text("");
                        h.scan();
                    }
                    Err(e) if e.to_lowercase().contains("password required") => {
                        h.wifi.ask.set_visible(true);
                        h.wifi.password.grab_focus();
                    }
                    Err(e) => h.wifi_error(&e),
                }
            },
        );
    }

    // ------------------------------------------------------------ switcher
    /// Shows the switcher at once with what's here; a Tab only moves the
    /// highlight (the cells are built again when the list changes, or a
    /// picture arrives).
    fn show_switcher(self: &Rc<Self>) {
        let c = self.carousel.borrow().clone();
        if !c.open || c.items.is_empty() {
            self.switcher.set_visible(false);
            return;
        }
        if self.cells_stale.replace(false) || *self.built.borrow() != c.items {
            self.build_cells(&c.items);
        }
        for (i, cell) in self.cells.borrow().iter().enumerate() {
            if i == c.index {
                cell.add_css_class("selected");
            } else {
                cell.remove_css_class("selected");
            }
        }
        self.switcher.present();
    }

    fn build_cells(self: &Rc<Self>, items: &[model::CarouselItem]) {
        while let Some(child) = self.switcher_box.first_child() {
            self.switcher_box.remove(&child);
        }
        let mut cells = Vec::with_capacity(items.len());
        let divider = model::divider_at(items);
        for (i, item) in items.iter().enumerate() {
            // The spaces, then what runs on the machine itself.
            if divider == Some(i) {
                let line = gtk::Separator::new(gtk::Orientation::Vertical);
                line.add_css_class("divider");
                self.switcher_box.append(&line);
            }
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
            cell.add_css_class("item");
            // A wadspace is its desktop: its wallpaper with its icons.
            let wallpaper =
                if item.kind == "space" { item.preview.as_ref().and_then(|p| self.wallpaper(p)) } else { None };
            // WadSpaces Client (wadd's "home" view) is the WadSpaces logo.
            let texture = if item.view == "home" {
                self.logo.borrow().clone()
            } else {
                item.icon.as_ref().and_then(|p| self.icon(p))
            };
            let face: gtk::Widget = if item.kind == "space" && (wallpaper.is_some() || !item.desktop.is_empty()) {
                let icons: Vec<Option<gdk::Texture>> =
                    item.desktop.iter().map(|d| self.desktop_icon(&d.icon)).collect();
                desktop_card(wallpaper.as_ref(), &icons)
            } else if let Some(t) = texture {
                square_image(&t, 64, "icon")
            } else {
                let initial =
                    label(&item.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), "icon");
                initial.set_halign(gtk::Align::Center);
                initial.upcast()
            };
            cell.append(&face);
            let name = label(&item.name, "name");
            name.set_max_width_chars(16);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            cell.append(&name);
            if !item.running {
                cell.append(&label("not running", "state"));
            } else if let Some(keys) = item.shortcut() {
                cell.append(&label(&keys, "state"));
            }
            self.switcher_box.append(&cell);
            cells.push(cell);
        }
        self.cells.replace(cells);
        self.built.replace(items.to_vec());
    }

    /// A picture from wadd, made off the GTK loop: from the cache, else
    /// fetched (the switcher redraws when it arrives). One wadd didn't give
    /// is asked for again after a while.
    fn fetched(
        self: &Rc<Self>,
        key: String,
        work: impl FnOnce() -> Option<thumb::Rgba> + Send + 'static,
    ) -> Option<gdk::Texture> {
        match self.icons.borrow().get(&key) {
            Some(Cached::Ready(t)) => return Some(t.clone()),
            Some(Cached::Loading) => return None,
            Some(Cached::Failed(at)) if at.elapsed() < RETRY => return None,
            _ => {}
        }
        self.icons.borrow_mut().insert(key.clone(), Cached::Loading);
        let h = self.clone();
        background(work, move |r| {
            let got = match r {
                Some(px) => Cached::Ready(rgba_texture(px)),
                None => Cached::Failed(std::time::Instant::now()),
            };
            let ready = matches!(got, Cached::Ready(_));
            h.icons.borrow_mut().insert(key, got);
            if ready && h.switcher.is_visible() {
                h.cells_stale.set(true);
                h.show_switcher();
            }
        });
        None
    }

    /// A wadspace's wallpaper, made a small desktop-shaped picture.
    fn wallpaper(self: &Rc<Self>, path: &str) -> Option<gdk::Texture> {
        let w = self.wadd.clone();
        let p = path.to_string();
        self.fetched(format!("desktop {path}"), move || {
            w.bytes(&p).and_then(|b| thumb::cover_rgba(&b, PREVIEW_W as u32 * 2, PREVIEW_H as u32 * 2))
        })
    }

    /// One of a wadspace's desktop icons (GET /v1/desktop-icon draws it).
    fn desktop_icon(self: &Rc<Self>, src: &str) -> Option<gdk::Texture> {
        let w = self.wadd.clone();
        let path = format!("/v1/desktop-icon?size={DESKTOP_ICON_PX}&src={}", query_value(src));
        self.fetched(format!("desk {src}"), move || w.bytes(&path).and_then(|b| thumb::rgba(&b, DESKTOP_ICON_PX)))
    }

    /// A workspace's icon, in the theme's variant when it has one (WadBrowser
    /// does). Icons are often whole wallpapers: shrunk off the GTK loop.
    fn icon(self: &Rc<Self>, path: &str) -> Option<gdk::Texture> {
        // One put in the cache by hand (the snapshots).
        if let Some(Cached::Ready(t)) = self.icons.borrow().get(path) {
            return Some(t.clone());
        }
        let theme = if self.theme.get() == Theme::Light { "light" } else { "dark" };
        let themed = format!("{path}?theme={theme}");
        let w = self.wadd.clone();
        let p = themed.clone();
        self.fetched(themed, move || w.bytes(&p).and_then(|b| thumb::cover_rgba(&b, 128, 128)))
    }

    /// Fetches what the switcher will show for each wadspace, so it opens
    /// with its pictures.
    fn prefetch(self: &Rc<Self>) {
        let spaces = self.state.borrow().workspaces.clone();
        for s in spaces.iter().filter(|s| !s.is_app() && s.enabled) {
            self.wallpaper(&format!("/v1/workspaces/{}/wallpaper", s.id));
            for d in &s.desktop {
                self.desktop_icon(&d.icon);
            }
        }
    }

    // -------------------------------------------------------------- events
    fn on_event(self: &Rc<Self>, ev: Incoming) {
        match ev {
            Incoming::Event(name, data) if name == "session" => {
                if let Ok(session) = serde_json::from_value::<Option<model::Session>>(data) {
                    if !model::locked(session.as_ref()) {
                        self.show_left.set(false);
                    }
                    self.state.borrow_mut().session = session;
                    self.update_bar();
                }
            }
            Incoming::Event(name, data) if name == "network" => {
                if let Ok(network) = serde_json::from_value::<model::Network>(data) {
                    self.state.borrow_mut().network = Some(network);
                    self.update_bar();
                }
            }
            Incoming::Event(name, data) if name == "shortcut" => {
                if data.get("name").and_then(|n| n.as_str()) == Some("clipboard") {
                    self.toggle_clips();
                }
            }
            Incoming::Event(name, data) if name == "view" => {
                if let Ok(v) = serde_json::from_value::<model::ViewState>(data) {
                    self.state.borrow_mut().view = Some(v.view);
                    self.follow_view();
                }
            }
            Incoming::Event(name, data) if name == "devices" => {
                if let Ok(list) = serde_json::from_value(data) {
                    self.devices.on_devices(list);
                }
            }
            Incoming::Event(name, data) if name == "bluetooth" => {
                if let Ok(s) = serde_json::from_value(data) {
                    self.devices.on_bluetooth(s);
                }
            }
            Incoming::Event(name, data) if name == "workspaceState" => {
                if let (Some(id), Some(c)) = (data["id"].as_str(), data["container"].as_str()) {
                    self.devices.on_state(id, c == "running");
                }
            }
            Incoming::Event(name, data) if name == "workspaces" => {
                if let Ok(list) = serde_json::from_value::<Vec<model::Space>>(data) {
                    self.devices.on_spaces(&list);
                    self.state.borrow_mut().workspaces = list;
                    self.follow_view();
                    self.prefetch();
                }
            }
            Incoming::Event(name, data) if name == "carousel" => {
                self.carousel.replace(serde_json::from_value(data).unwrap_or_default());
                self.show_switcher();
            }
            // The old way the bar opened WadSpaces Client's menus: only power
            // and Wi-Fi asked, and both are here now.
            Incoming::Event(..) => {}
            Incoming::Disconnected => {
                self.carousel.replace(Carousel::default());
                self.switcher.set_visible(false);
            }
        }
    }
}

fn report(r: Result<(), String>, what: &str) {
    if let Err(e) = r {
        eprintln!("hud: {what}: {e}");
    }
}

/// Renders a window's contents to a PNG.
fn save_png(win: &gtk::Window, path: &std::path::Path) {
    let (w, h) = (win.width(), win.height());
    let paintable = gtk::WidgetPaintable::new(Some(win));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, w as f64, h as f64);
    let (Some(node), Some(native)) = (snap.to_node(), win.native()) else { return };
    let texture = native.renderer().map(|r| r.render_texture(node, None));
    if let Some(t) = texture
        && t.save_to_png(path).is_ok()
    {
        println!("hud: wrote {}", path.display());
    }
}

/// Made-up WadNotes for the snapshots: a few notes, tasks and events around now.
fn snapshot_notes(root: &std::path::Path) {
    use chrono::{Duration, Timelike};
    use wad_notes::{Event, Note, Priority, Status, Store, Task};
    let _ = std::fs::remove_dir_all(root);
    let s = Store::new(root);
    let t = notes::now().with_second(0).unwrap();
    let mut notes = vec![
        (
            "writing",
            "Chapter 3 outline",
            "# Chapter 3 outline\n\nThe **storm** scene moves earlier.\n\n- Open on the harbour\n- [x] Cut the flashback\n- [ ] Ana's letter arrives\n\n> Keep it under 4,000 words.\n",
        ),
        ("writing", "Research links", "Tide tables for *Suva*, and the old ferry routes."),
        ("default", "Machine setup", "Remember to update the stick before Friday."),
    ];
    for (i, (scope, title, body)) in notes.drain(..).enumerate() {
        let mut n = Note { title: title.into(), body: body.into(), ..Note::new(t) };
        let _ = s.save_note(scope, &mut n, t - Duration::hours(3 * i as i64));
    }
    let today = t.date();
    for (title, status, priority, due) in [
        ("Draft chapter 3", Status::Doing, Priority::High, Some(today + Duration::days(1))),
        ("Proofread chapter 2", Status::Todo, Priority::Medium, Some(today - Duration::days(1))),
        ("Pick a cover photo", Status::Todo, Priority::Low, None),
        ("Export chapter 1 to PDF", Status::Done, Priority::Medium, None),
    ] {
        let mut k = Task { title: title.into(), status, priority, due, ..Task::new(t) };
        let _ = s.save_task("writing", &mut k);
    }
    let soon = t + Duration::minutes(10);
    for (title, start, scope, all_day, remind) in [
        ("Writing group", soon, Some("writing"), false, Some(15)),
        ("Editor call", (today + Duration::days(1)).and_hms_opt(9, 30, 0).unwrap(), Some("writing"), false, Some(15)),
        ("Sprint review", (today + Duration::days(2)).and_hms_opt(14, 0, 0).unwrap(), Some("iq-dev"), false, None),
        ("Back up the stick", (today + Duration::days(3)).and_hms_opt(0, 0, 0).unwrap(), None, true, Some(60)),
    ] {
        let mut e = Event { title: title.into(), all_day, remind, ..Event::new(start, scope.map(String::from)) };
        if !all_day {
            e.end = Some(start + Duration::minutes(45));
        }
        let _ = s.save_event(&mut e, t);
    }
}

fn snapshot(hud: Rc<Hud>, app: gtk::Application, dir: std::path::PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    // WADSPACES_HUD_SNAPSHOT_ICON: an image file to show as Writing's icon.
    if let Some(bytes) = std::env::var_os("WADSPACES_HUD_SNAPSHOT_ICON").and_then(|p| std::fs::read(p).ok())
        && let Some(t) =
            thumb::thumbnail(&bytes, 128).and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from(&b)).ok())
    {
        hud.icons.borrow_mut().insert("/v1/workspaces/writing/icon".into(), Cached::Ready(t));
    }
    // WADSPACES_HUD_SNAPSHOT_WALLPAPER: a picture to show as Writing's desktop (its preview).
    if let Some(bytes) = std::env::var_os("WADSPACES_HUD_SNAPSHOT_WALLPAPER").and_then(|p| std::fs::read(p).ok())
        && let Some(t) = thumb::cover(&bytes, PREVIEW_W as u32 * 2, PREVIEW_H as u32 * 2)
            .and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from(&b)).ok())
    {
        hud.icons.borrow_mut().insert("desktop /v1/workspaces/writing/wallpaper".into(), Cached::Ready(t));
    }
    hud.state.replace(State {
        session: None,
        network: Some(model::Network {
            available: true,
            connectivity: Some("full".into()),
            ssid: Some("Home Network".into()),
        }),
        ..State::default()
    });
    hud.update_bar();
    hud.show_battery(Some(power::Battery { percent: 76, state: power::State::Discharging, minutes: Some(250) }));
    hud.power.present();
    hud.wifi.win.present();
    hud.show_networks(&[
        WifiNetwork {
            ssid: "Home Network".into(),
            signal: 82,
            security: "WPA2".into(),
            secure: true,
            supported: true,
            active: true,
            known: true,
        },
        WifiNetwork {
            ssid: "Cafe Guest".into(),
            signal: 54,
            security: String::new(),
            secure: false,
            supported: true,
            active: false,
            known: false,
        },
        WifiNetwork {
            ssid: "Neighbours".into(),
            signal: 30,
            security: "WPA2".into(),
            secure: true,
            supported: true,
            active: false,
            known: false,
        },
        WifiNetwork {
            ssid: "Office 802.1X".into(),
            signal: 40,
            security: "WPA2 802.1X".into(),
            secure: true,
            supported: false,
            active: false,
            known: false,
        },
    ]);
    hud.wifi.scanning.set_visible(false);
    hud.pick(&WifiNetwork {
        ssid: "Neighbours".into(),
        signal: 30,
        security: "WPA2".into(),
        secure: true,
        supported: true,
        active: false,
        known: false,
    });
    hud.carousel.replace(Carousel {
        open: true,
        index: 1,
        items: vec![
            model::CarouselItem {
                view: "home".into(),
                name: "WadSpaces".into(),
                icon: None,
                preview: None,
                running: true,
                kind: "home".into(),
                number: None,
                desktop: vec![],
            },
            model::CarouselItem {
                view: "workspace:writing".into(),
                name: "Writing".into(),
                icon: Some("/v1/workspaces/writing/icon".into()),
                preview: Some("/v1/workspaces/writing/wallpaper".into()),
                running: true,
                kind: "space".into(),
                number: Some(1),
                desktop: ["VS Code", "WadBrowser", "Obsidian", "GitHub", "Docs", "Notes"]
                    .iter()
                    .map(|l| model::DesktopIcon { label: l.to_string(), icon: format!("/apps/{l}.png") })
                    .collect(),
            },
            model::CarouselItem {
                view: "workspace:iq-dev".into(),
                name: "IntelligenceQuest Dev".into(),
                icon: None,
                preview: None,
                running: true,
                kind: "space".into(),
                number: Some(2),
                desktop: vec![],
            },
            model::CarouselItem {
                view: "workspace:terminal".into(),
                name: "WadTerminal".into(),
                icon: None,
                preview: None,
                running: true,
                kind: "app".into(),
                number: Some(3),
                desktop: vec![],
            },
        ],
    });
    hud.show_switcher();
    // Brightness and volume, as if both answered.
    hud.bright.win.present();
    hud.bright.show(Ok(70), &hud.syncing);
    hud.sound.win.present();
    hud.show_volume(Some(audio::Volume { percent: 40, muted: false }));
    hud.sound.show(Ok(40), &hud.syncing);
    hud.flash(audio::icon(Some(audio::Volume { percent: 80, muted: false })), Ok(80));
    // A tooltip, over the battery.
    let h = hud.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || h.tip_show(h.battery.upcast_ref()));
    // The clipboard history: text, code, and an image (the switcher's icon).
    let text = |t: &str| Content {
        items: vec![wad_clip::Item { mime: "text/plain;charset=utf-8".into(), data: t.as_bytes().to_vec() }],
    };
    {
        let mut h = hud.clip.history.borrow_mut();
        h.add(text("fn main() {\n    println!(\"hello from Writing\");\n}"), None, now() - 7200.0);
        if let Some(bytes) = std::env::var_os("WADSPACES_HUD_SNAPSHOT_ICON").and_then(|p| std::fs::read(p).ok()) {
            let thumb = thumb::fit(&bytes, 192, 96);
            h.add(
                Content { items: vec![wad_clip::Item { mime: "image/png".into(), data: bytes }] },
                thumb,
                now() - 300.0,
            );
        }
        h.add(text("https://github.com/HungGod/WadSpaces/pull/42"), None, now() - 20.0);
    }
    hud.toggle_clips();
    // WadNotes: on Writing, every section on, and the reminder due.
    hud.state.borrow_mut().view = Some("writing".into());
    hud.state.borrow_mut().workspaces = [
        ("writing", "Writing", None),
        ("iq-dev", "IntelligenceQuest Dev", None),
        ("terminal", "Terminal", Some("app")),
    ]
    .into_iter()
    .map(|(id, name, kind)| model::Space {
        id: id.into(),
        name: name.into(),
        kind: kind.map(String::from),
        enabled: true,
        display: None,
        desktop: vec![],
    })
    .collect();
    hud.follow_view();
    for s in [notes::Section::Notes, notes::Section::Tasks, notes::Section::Calendar] {
        hud.show_section(s, true);
    }
    // Devices: a pad in Writing, a webcam waiting for IQ Dev, a security
    // key just plugged in (its question up), headphones and Bluetooth.
    {
        use wad_proto::v1::{BluetoothDevice, BluetoothStatus, Device, DeviceBus, DeviceKind};
        let spaces = hud.state.borrow().workspaces.clone();
        hud.devices.on_spaces(&spaces);
        hud.devices.on_state("writing", true);
        let dev =
            |key: &str, name: &str, kind, attachable, present, attached: &[&str], remembered: &[&str], new| Device {
                key: key.into(),
                name: name.into(),
                bus: if key.starts_with("bt:") { DeviceBus::Bluetooth } else { DeviceBus::Usb },
                kind,
                attachable,
                present,
                attached: attached.iter().map(|s| s.to_string()).collect(),
                remembered: remembered.iter().map(|s| s.to_string()).collect(),
                new,
                ejected: false,
            };
        hud.devices.win.present();
        hud.devices.on_bluetooth(BluetoothStatus {
            available: true,
            powered: true,
            discovering: true,
            devices: vec![
                BluetoothDevice {
                    address: "11:22:33:44:55:66".into(),
                    name: "WH-1000XM5".into(),
                    kind: DeviceKind::Audio,
                    paired: true,
                    connected: true,
                    rssi: None,
                    busy: false,
                },
                BluetoothDevice {
                    address: "A0:AB:51:12:34:56".into(),
                    name: "DualSense Wireless Controller".into(),
                    kind: DeviceKind::Gamepad,
                    paired: false,
                    connected: false,
                    rssi: Some(-48),
                    busy: false,
                },
            ],
            prompt: None,
        });
        hud.devices.on_devices(vec![
            dev("usb:2dc8:6101:1", "8BitDo Pro 2", DeviceKind::Gamepad, true, true, &["writing"], &["writing"], false),
            dev("usb:046d:085b:2", "Logitech C925e", DeviceKind::Webcam, true, true, &[], &["iq-dev"], false),
            dev(
                "usb:0480:0820:4",
                "TOSHIBA External",
                DeviceKind::Storage,
                true,
                true,
                &["writing"],
                &["writing"],
                false,
            ),
            dev("usb:1050:0407:3", "YubiKey 5 NFC", DeviceKind::SecurityKey, true, true, &[], &[], true),
            dev("bt:11:22:33:44:55:66", "WH-1000XM5", DeviceKind::Audio, false, true, &[], &[], false),
            dev("usb:045e:0922@1-1", "Surface Keyboard", DeviceKind::Keyboard, false, true, &[], &[], false),
        ]);
    }
    hud.notes.tick();
    hud.open_notes_menu();
    glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
        for (win, name) in [
            (&hud.bar, "bar"),
            (&hud.power, "power"),
            (&hud.wifi.win, "wifi"),
            (&hud.switcher, "switcher"),
            (&hud.bright.win, "brightness"),
            (&hud.sound.win, "volume"),
            (&hud.osd.win, "level"),
            (&hud.tip.win, "tip"),
            (&hud.clip.win, "clipboard"),
            (&hud.notes.win, "notes"),
            (&hud.notes.reminder.win, "reminder"),
            (&hud.notes_menu.win, "notes-menu"),
            (&hud.devices.win, "devices"),
            (&hud.devices.ask_win, "device-ask"),
        ] {
            save_png(win, &dir.join(format!("{name}.png")));
        }
        hud.notes_menu.win.set_visible(false);
        // A note open in its preview (the card grows leftwards) and an event
        // open to edit (it grows over the tasks, drawn on top of them).
        hud.notes.notes.open_first(true);
        hud.notes.calendar.open_first();
        hud.state.borrow_mut().session = Some(model::Session {
            mode: "focus".into(),
            minutes: Some(25.0),
            ends_at: Some(now() + 1500.0),
            expired: false,
        });
        hud.show_left.set(true);
        hud.update_bar();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            save_png(&hud.bar, &dir.join("bar-focus.png"));
            save_png(&hud.notes.win, &dir.join("notes-editors.png"));
            // Put away with the button: the cards go, the button unlit.
            hud.notes.set_put_away(true);
            hud.update_notes_button();
            glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                save_png(&hud.bar, &dir.join("bar-notes-away.png"));
                println!("hud: notes visible when put away: {}", hud.notes.win.is_visible());
                hud.notes.set_put_away(false);
                // Tucked away, after time ran out: only the arrow, lit (and
                // WadNotes gone with the bar).
                hud.state.borrow_mut().session.as_mut().unwrap().expired = true;
                hud.update_bar();
                hud.set_collapsed(true, false);
                glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                    save_png(&hud.bar, &dir.join("bar-collapsed.png"));
                    println!("hud: notes visible when collapsed: {}", hud.notes.win.is_visible());
                    app.quit();
                });
            });
        });
    });
}

/// `hud --key <name>`: a brightness or volume key (sway's bindings).
fn key_arg(args: &[String]) -> Option<&str> {
    let i = args.iter().position(|a| a == "--key")?;
    args.get(i + 1).map(String::as_str)
}

fn main() -> glib::ExitCode {
    // One HUD per session: running it again with `--key` hands the key to
    // the one that's running (over the session bus) and exits.
    let app = gtk::Application::builder()
        .application_id("io.wadspaces.hud")
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let running: Rc<RefCell<Option<Rc<Hud>>>> = Rc::default();
    let r = running.clone();
    app.connect_command_line(move |app, cmd| {
        let args: Vec<String> = cmd.arguments().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        if r.borrow().is_none() {
            app.activate();
        }
        if let (Some(key), Some(hud)) = (key_arg(&args), r.borrow().as_ref()) {
            hud.on_key(key);
        }
        glib::ExitCode::SUCCESS
    });
    let r = running.clone();
    app.connect_activate(move |app| {
        if r.borrow().is_some() {
            return;
        }
        // Adwaita's icons, whatever the desktop's theme (KDE's Breeze lacks
        // the battery-level ones): the HUD names them (icon(), power::icon).
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_icon_theme_name(Some("Adwaita"));
        }
        let css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        // WadNotes' files (a snapshot draws made-up ones, in a folder of its own).
        let snapshot_dir = std::env::var_os("WADSPACES_HUD_SNAPSHOT").map(std::path::PathBuf::from);
        let (store, reminders) = match &snapshot_dir {
            Some(dir) => {
                let root = dir.join("wadnotes");
                (wad_notes::Store::new(&root), wad_notes::ReminderFile(root.join(".reminders.json")))
            }
            None => (
                wad_notes::Store::new(wad_notes::Store::default_root()),
                wad_notes::ReminderFile(wad_notes::ReminderFile::default_path()),
            ),
        };
        if let Some(dir) = &snapshot_dir {
            snapshot_notes(&dir.join("wadnotes"));
        }
        let hud = Hud::build(app, css, theme::current(), store, reminders);
        r.replace(Some(hud.clone()));
        if let Some(dir) = snapshot_dir {
            let _hold = app.hold();
            snapshot(hud, app.clone(), dir);
            std::mem::forget(_hold);
            return;
        }
        // Its volume icon, from the start.
        let h = hud.clone();
        background(audio::get, move |r| h.show_volume(r.ok()));
        // As it was left: tucked away first, so WadNotes doesn't flash up.
        let p = prefs::load_from(&prefs::file());
        if p.collapsed {
            hud.set_collapsed(true, false);
        }
        for (s, on) in
            [(notes::Section::Notes, p.notes), (notes::Section::Tasks, p.tasks), (notes::Section::Calendar, p.calendar)]
        {
            if on {
                hud.show_section(s, true);
            }
        }
        if p.notes_away {
            hud.notes.set_put_away(true);
            hud.update_notes_button();
        }
        // Reminders due, now and each half minute.
        hud.notes.tick();
        let h = hud.clone();
        glib::timeout_add_seconds_local(30, move || {
            h.notes.tick();
            glib::ControlFlow::Continue
        });
        hud.start_clipboard();
        // Kept for as long as the HUD runs.
        std::mem::forget(hud.follow_theme());
        let (tx, rx) = async_channel::unbounded();
        hud.wadd.follow(tx);
        let h = hud.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                h.on_event(ev);
            }
        });
        // The battery, now and every 10 s.
        let h = hud.clone();
        let check = move || h.show_battery(power::combine(&power::read(&power::dir())));
        check();
        glib::timeout_add_seconds_local(10, move || {
            check();
            glib::ControlFlow::Continue
        });
        // The clock, and the focus timer counting down.
        let h = hud.clone();
        glib::timeout_add_seconds_local(1, move || {
            h.tick();
            if model::locked(h.state.borrow().session.as_ref()) {
                h.update_bar();
            }
            glib::ControlFlow::Continue
        });
    });
    app.run()
}
