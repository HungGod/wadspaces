//! The application: one instance (a second start shows the window), the
//! window (full screen in the kiosk), the theme and fonts, wadd's events
//! into the store.

use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};

use crate::Args;
use crate::store::store;

/// The program's id, as the Tauri app's (its folders), and the window's
/// app_id on Wayland: what sway's rules for the shell name.
const APP_ID: &str = "io.wadspaces.client";
const WINDOW_APP_ID: &str = "client";

const CSS: &str = include_str!("style.css");

pub fn run(args: Args) -> std::process::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        wad_gtk::fonts::install();
        let display = gdk::Display::default().expect("a display");
        wad_gtk::theme::install(&display, CSS);
        // Snapshots and trying things: a made-up machine and account.
        if let Ok(path) = std::env::var("WADSPACES_FAKE") {
            crate::fake::load(std::path::Path::new(&path));
            return;
        }
        follow_wadd();
        crate::account::start();
        crate::account::follow_library();
    });
    let args = Rc::new(args);
    app.connect_activate(move |app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let w = window(app, &args);
        w.present();
        if let Some(dir) = args.snapshot.clone() {
            snapshot(&w, dir);
        }
    });
    // Our own options are read in main.rs; GApplication sees none.
    let code = app.run_with_args::<&str>(&[]);
    std::process::ExitCode::from(code.get())
}

fn window(app: &gtk::Application, args: &Args) -> gtk::ApplicationWindow {
    let w = gtk::ApplicationWindow::builder()
        .application(app)
        .title("WadSpaces")
        .default_width(1280)
        .default_height(800)
        .width_request(800)
        .height_request(560)
        .build();
    w.set_child(Some(&crate::setup::gate(crate::shell::shell())));
    wad_gtk::theme::mark(&w, wad_gtk::theme::shown());
    if args.kiosk {
        w.set_decorated(false);
        w.fullscreen();
        follow_hud(&w);
    }
    // Esc closes the top dialog (or the Builder's editor).
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, _| {
        if key == gtk::gdk::Key::Escape && (crate::modal::escape() || crate::pages::builder::editor::escape()) {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    w.add_controller(keys);
    // sway knows the shell as "client" (host/etc/sway/wadspaces.conf). GTK
    // names the window after the application's id once its toplevel exists
    // (on map); renamed straight after, before its first frame, so sway's
    // rules see "client".
    w.connect_map(|w| {
        if let Some(t) = w.surface().and_then(|s| s.downcast::<gdk_wayland::WaylandToplevel>().ok()) {
            t.set_application_id(WINDOW_APP_ID);
        }
    });
    w
}

thread_local! {
    static HUD_MONITOR: std::cell::RefCell<Option<gtk::gio::FileMonitor>> = const { std::cell::RefCell::new(None) };
}

/// The HUD floats its bar over the bottom right of the machine's screen.
/// While it's out (not tucked behind its arrow; it remembers which in
/// ~/.config/wadspaces/hud.json, no file: out), what sits along the bottom
/// keeps clear of it (the window's `hud-room`).
fn follow_hud(w: &gtk::ApplicationWindow) {
    let path = wad_gtk::theme::file().with_file_name("hud.json");
    let apply = {
        let (w, path) = (w.downgrade(), path.clone());
        move || {
            #[derive(serde::Deserialize)]
            struct Hud {
                #[serde(default)]
                collapsed: bool,
            }
            let open = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Hud>(&b).ok())
                .is_none_or(|h| !h.collapsed);
            if let Some(w) = w.upgrade() {
                if open {
                    w.add_css_class("hud-room");
                } else {
                    w.remove_css_class("hud-room");
                }
            }
        }
    };
    apply();
    let file = gtk::gio::File::for_path(&path);
    match file.monitor_file(gtk::gio::FileMonitorFlags::WATCH_MOVES, gtk::gio::Cancellable::NONE) {
        Ok(m) => {
            m.connect_changed(move |_, _, _, _| apply());
            HUD_MONITOR.with(|h| h.replace(Some(m)));
        }
        Err(e) => tracing::warn!("{}: {e}", path.display()),
    }
}

/// wadd's events, from tokio to the store on GTK's loop.
fn follow_wadd() {
    let (tx, rx) = async_channel::unbounded();
    {
        let _rt = crate::rt::enter();
        wad_wadd_client::Wadd::here().follow(move |i| tx.send_blocking(i).is_ok());
    }
    glib::spawn_future_local(async move {
        let s = store();
        while let Ok(i) = rx.recv().await {
            s.apply(i);
        }
    });
}

/// Draws what's on screen (setup, without a sign-in), then the app, dark
/// and light, to DIR, and quits.
fn snapshot(w: &gtk::ApplicationWindow, dir: std::path::PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let w = w.clone();
    glib::spawn_future_local(async move {
        let display = WidgetExt::display(&w);
        let settle = |ms| glib::timeout_future(std::time::Duration::from_millis(ms));
        // The gate waits a moment for wadd.
        settle(3600).await;
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(700).await;
            shot(&w, &dir.join(format!("setup-{}.png", t.name())));
        }
        crate::setup::show_app();
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(700).await;
            shot(&w, &dir.join(format!("shell-{}.png", t.name())));
        }
        // The tour: its first step on Home, its second on the Builder; a
        // notice on the avatar.
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        crate::tour::start(false);
        settle(900).await;
        shot(&w, &dir.join("tour-nav-dark.png"));
        crate::shell::go(crate::shell::Page::Builder);
        settle(1200).await;
        shot(&w, &dir.join("tour-board-dark.png"));
        crate::tour::finish();
        crate::shell::go(crate::shell::Page::Home);
        crate::notices::notify(
            "Cosmic Bodybuilding is built",
            "Its image is ready on the machine",
            Some("cosmic-bodybuilding".into()),
        );
        settle(700).await;
        shot(&w, &dir.join("notice-dark.png"));
        crate::launcher::open();
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("launcher-{}.png", t.name())));
        }
        crate::modal::escape();
        for (entry, name) in [("w:vanua-academy", "launcher-customize"), ("b:wadbrowser", "launcher-browser")] {
            crate::launcher::customize(entry);
            for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                wad_gtk::theme::set(&display, t);
                settle(900).await;
                shot(&w, &dir.join(format!("{name}-{}.png", t.name())));
            }
            crate::modal::escape();
        }
        crate::shell::go(crate::shell::Page::Machines);
        for tab in ["wadspaces", "history", "storage", "files", "devices", "diagnostics"] {
            crate::pages::machines::show_tab(tab);
            let name = if tab == "wadspaces" { "machines".to_string() } else { format!("machines-{tab}") };
            for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                wad_gtk::theme::set(&display, t);
                settle(800).await;
                shot(&w, &dir.join(format!("{name}-{}.png", t.name())));
            }
        }
        // The long tabs whole, and another machine's.
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        for tab in ["storage", "diagnostics"] {
            crate::pages::machines::show_tab(tab);
            settle(800).await;
            if let Some(tab) = crate::pages::machines::tab_widget() {
                shot(&tab, &dir.join(format!("machines-{}-whole-dark.png", crate::pages::machines::tab_name())));
            }
        }
        crate::pages::machines::pick("m-desk");
        settle(800).await;
        shot(&w, &dir.join("machines-other-dark.png"));
        // A wadspace's details.
        crate::details::open("vanua-academy");
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("details-{}.png", t.name())));
        }
        crate::modal::escape();
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        crate::shell::go(crate::shell::Page::Builder);
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(1000).await;
            shot(&w, &dir.join(format!("builder-{}.png", t.name())));
        }
        // A drag: over another stack, over Delete, then into a new stack.
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        settle(300).await;
        if crate::pages::builder::sim::pick_up("cosmic-bodybuilding") {
            crate::pages::builder::sim::over_stack("vanua-academy");
            settle(400).await;
            shot(&w, &dir.join("builder-drag-stack-dark.png"));
            crate::pages::builder::sim::over_delete();
            settle(400).await;
            shot(&w, &dir.join("builder-drag-delete-dark.png"));
            crate::pages::builder::sim::drop_after_last();
            settle(900).await;
            shot(&w, &dir.join("builder-dropped-dark.png"));
        }
        // The editors: a template's, Extensions' (open on WadBrowser) and a
        // Customization's; a Customization open on its keys; Extensions set
        // up new.
        use crate::pages::builder::editor;
        use wad_board::Layer;
        for (layer, focus, name) in [
            (Layer::Template, None, "builder-template"),
            (Layer::Extensions, Some("wadbrowser"), "builder-extensions"),
            (Layer::Customization, None, "builder-customization"),
        ] {
            if editor::sim::open("vanua-academy", layer, focus) {
                for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                    wad_gtk::theme::set(&display, t);
                    settle(1000).await;
                    shot(&w, &dir.join(format!("{name}-{}.png", t.name())));
                }
                editor::sim::close();
                settle(700).await;
            }
        }
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        for (stack, layer, focus, name) in [
            ("vanua-academy", Layer::Customization, Some("secrets"), "builder-customization-keys"),
            ("cosmic-bodybuilding", Layer::Extensions, None, "builder-extensions-new"),
        ] {
            if editor::sim::open(stack, layer, focus) {
                settle(1100).await;
                shot(&w, &dir.join(format!("{name}-dark.png")));
                editor::sim::close();
                settle(700).await;
            }
        }
        // An app dragged onto a template's desktop, then saved.
        if editor::sim::open("cosmic-bodybuilding", Layer::Template, None) {
            settle(900).await;
            if editor::template::sim::pick_up("vscode") {
                editor::template::sim::over_desktop();
                settle(400).await;
                shot(&w, &dir.join("builder-template-drag-dark.png"));
                crate::drag::sim::end(true);
                settle(600).await;
            }
            editor::sim::save();
            settle(1200).await;
            shot(&w, &dir.join("builder-template-saved-dark.png"));
        }
        // A stack previewed, VS Code's window open.
        crate::pages::builder::preview::open("vanua-academy");
        crate::pages::builder::preview::sim_show("app:vscode");
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("builder-preview-{}.png", t.name())));
        }
        crate::pages::builder::preview::close();
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        // Recently deleted: a customization just deleted, a wadspace deleted before.
        crate::pages::builder::sim::delete_customization("cosmic-bodybuilding");
        crate::pages::builder::sim::show_deleted();
        settle(900).await;
        shot(&w, &dir.join("builder-deleted-dark.png"));
        crate::shell::go(crate::shell::Page::Secrets);
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("secrets-{}.png", t.name())));
        }
        if let Some(app) = w.application() {
            app.quit();
        }
    });
}

fn shot(w: &impl IsA<gtk::Widget>, path: &std::path::Path) {
    match save_png(w, path) {
        Ok(()) => tracing::info!("wrote {}", path.display()),
        Err(e) => tracing::warn!("{}: {e}", path.display()),
    }
}

fn save_png(w: &impl IsA<gtk::Widget>, path: &std::path::Path) -> Result<(), String> {
    let w = w.upcast_ref::<gtk::Widget>();
    let paintable = gtk::WidgetPaintable::new(Some(w));
    let (width, height) = (w.width() as f64, w.height() as f64);
    // At the screen's scale: what it shows, pixel for pixel.
    let k = w.scale_factor() as f64;
    let snap = gtk::Snapshot::new();
    snap.scale(k as f32, k as f32);
    paintable.snapshot(&snap, width, height);
    let node = snap.to_node().ok_or("nothing drawn")?;
    let renderer = w.native().and_then(|n| n.renderer()).ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())
}
