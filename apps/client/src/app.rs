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
        wad_gtk::theme::on_change(tell_wadd_theme);
        // Whether the machine can stream (the Stream buttons go by it).
        crate::stream::load_tunnel();
        crate::stream::load_streams();
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
    // Esc closes the top dialog.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, _| {
        if key == gtk::gdk::Key::Escape && crate::modal::escape() {
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

/// The Builder's snapshots: a wadspace selected, on each tab of the
/// sidebar; an app dragged onto a card's Apps and dropped (its part
/// flashes); a key held over another card (New secret key shows); a card
/// copied onto another machine, that machine's row; an empty machine's card
/// and an app dropped on it; more selected and a piece picked; Recently
/// deleted.
async fn builder_b(w: &gtk::ApplicationWindow, dir: &std::path::Path) {
    use crate::pages::builder::b::{self, carousel, sim, state};
    use wad_board::b::{Op, Pick};
    let display = WidgetExt::display(w);
    let settle = |ms| glib::timeout_future(std::time::Duration::from_millis(ms));
    let iq = "intelligencequest";
    let tab = |t: state::Tab| {
        state::write(|s| s.tab = t);
        state::select(iq, false);
    };
    tab(state::Tab::Apps);
    for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
        wad_gtk::theme::set(&display, t);
        settle(1200).await;
        shot(w, &dir.join(format!("builder-apps-{}.png", t.name())));
    }
    wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
    for (t, name) in [(state::Tab::AddOns, "addons"), (state::Tab::Secrets, "secrets")] {
        tab(t);
        settle(900).await;
        shot(w, &dir.join(format!("builder-{name}-dark.png")));
    }
    // Scrolled down, then the board drawn again (as the library's news does
    // every half minute): the sidebar stays where it was.
    sim::scroll_side_to_end();
    settle(300).await;
    state::changed();
    settle(900).await;
    shot(w, &dir.join("builder-scroll-kept-dark.png"));
    tab(state::Tab::Customization);
    for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
        wad_gtk::theme::set(&display, t);
        settle(1800).await;
        shot(w, &dir.join(format!("builder-custom-{}.png", t.name())));
    }
    wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
    // Its projects: GitHub's repositories to clone in, or mount.
    settle(600).await;
    sim::scroll_side_to_end();
    settle(600).await;
    shot(w, &dir.join("builder-projects-dark.png"));
    tab(state::Tab::Apps);
    settle(600).await;
    if sim::app_over_apps("obsidian", iq) {
        settle(500).await;
        shot(w, &dir.join("builder-drag-app-dark.png"));
        sim::drop_it();
        settle(300).await;
        shot(w, &dir.join("builder-dropped-dark.png"));
    }
    settle(900).await;
    if sim::key_over_secrets("desk-lab", "notes-vault", iq) {
        settle(600).await;
        shot(w, &dir.join("builder-drag-key-dark.png"));
        crate::drag::sim::end(false);
        settle(600).await;
    }
    if sim::wadspace_over_machine(iq, "m-desk") {
        settle(500).await;
        shot(w, &dir.join("builder-drag-machine-dark.png"));
        sim::drop_it();
        settle(900).await;
        shot(w, &dir.join("builder-copied-dark.png"));
    }
    carousel::show("m-desk");
    settle(900).await;
    shot(w, &dir.join("builder-desk-dark.png"));
    // A machine with none: an empty card; an app dropped on it makes it.
    carousel::show("m-laptop");
    settle(900).await;
    shot(w, &dir.join("builder-blank-dark.png"));
    if sim::app_over_blank("obsidian") {
        settle(500).await;
        shot(w, &dir.join("builder-blank-drag-dark.png"));
        sim::drop_it();
        settle(1200).await;
        shot(w, &dir.join("builder-blank-made-dark.png"));
    }
    // The HUD out: the machines keep clear of it.
    w.add_css_class("hud-room");
    settle(500).await;
    shot(w, &dir.join("builder-hud-dark.png"));
    w.remove_css_class("hud-room");
    // Two selected, and a project picked.
    carousel::show("m-surface");
    state::select(iq, false);
    state::select("cosmic-bodybuilding", true);
    state::toggle_pick(Pick::Project { ws: iq.into(), id: "iq".into() });
    for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
        wad_gtk::theme::set(&display, t);
        settle(900).await;
        shot(w, &dir.join(format!("builder-picked-{}.png", t.name())));
    }
    state::write(|s| s.picked.clear());
    // Recently deleted: VS Code's add-ons and a project taken out of
    // IntelligenceQuest, Desk Lab deleted (its parts to drag back out).
    let out =
        vec![Pick::Exts { ws: iq.into(), app: "vscode".into() }, Pick::Project { ws: iq.into(), id: "iq".into() }];
    let _ = state::apply(Op::Delete { picks: out });
    let _ = state::apply(Op::Delete { picks: vec![Pick::Wadspace { ws: "desk-lab".into() }] });
    b::show_trash(true);
    for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
        wad_gtk::theme::set(&display, t);
        settle(900).await;
        shot(w, &dir.join(format!("builder-trash-{}.png", t.name())));
    }
    sim::board_to_start(iq);
    settle(400).await;
    sim::background_from("desk-lab", iq);
    settle(900).await;
    let wallpaper = |id: &str| state::read(|s| s.board.get(id).and_then(|w| w.customization.get("wallpaper").cloned()));
    let took = wallpaper(iq).is_some() && wallpaper(iq) == wallpaper("desk-lab");
    tracing::info!("builder: a deleted wadspace's part drags into another: {took}");
    b::show_trash(false);
    wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
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
            let was = s.read_now(|st| st.connected);
            s.apply(i);
            // wadd up (again: it may have restarted): it hears the theme.
            if !was && s.read_now(|st| st.connected) {
                tell_wadd_theme(wad_gtk::theme::current());
            }
        }
    });
}

/// The theme wadspaces take at their next start: wadd draws
/// their backgrounds and web app icons in it.
fn tell_wadd_theme(t: wad_gtk::Theme) {
    let theme = match t {
        wad_gtk::Theme::Dark => wad_proto::v1::Theme::Dark,
        wad_gtk::Theme::Light => wad_proto::v1::Theme::Light,
    };
    let body = wad_proto::v1::MachineTheme { theme };
    glib::spawn_future_local(async move {
        let put = async move {
            let w = wad_wadd_client::Wadd::here();
            w.act(wad_wadd_client::Method::PUT, "/v1/machine/theme", Some(&body)).await.map_err(|e| e.to_string())
        };
        if let Err(e) = crate::rt::run(put).await {
            tracing::debug!("the theme to wadd: {e}");
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
            "IntelligenceQuest is built",
            "Its image is ready on the machine",
            Some("intelligencequest".into()),
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
        // (A version 4 one's picker draws a dozen textures, in each theme.)
        for (entry, name, ms) in
            [("w:intelligencequest", "launcher-customize", 2500), ("b:wadbrowser", "launcher-browser", 900)]
        {
            crate::launcher::customize(entry);
            for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                wad_gtk::theme::set(&display, t);
                settle(ms).await;
                shot(&w, &dir.join(format!("{name}-{}.png", t.name())));
            }
            crate::modal::escape();
        }
        crate::shell::go(crate::shell::Page::Machines);
        for tab in ["wadspaces", "history", "storage", "files", "devices", "tailnet", "diagnostics"] {
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
        for tab in ["storage", "diagnostics", "tailnet"] {
            crate::pages::machines::show_tab(tab);
            settle(800).await;
            if let Some(tab) = crate::pages::machines::tab_widget() {
                shot(&tab, &dir.join(format!("machines-{}-whole-dark.png", crate::pages::machines::tab_name())));
            }
        }
        // Storage's packages, shown.
        crate::pages::machines::show_tab("storage");
        crate::pages::machines::show_packages();
        settle(800).await;
        if let Some(tab) = crate::pages::machines::tab_widget() {
            shot(&tab, &dir.join("machines-storage-packages-dark.png"));
        }
        crate::pages::machines::pick("m-desk");
        settle(800).await;
        shot(&w, &dir.join("machines-other-dark.png"));
        // A wadspace's details.
        for (id, name) in [("intelligencequest", "details")] {
            crate::details::open(id);
            for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                wad_gtk::theme::set(&display, t);
                settle(900).await;
                shot(&w, &dir.join(format!("{name}-{}.png", t.name())));
            }
            crate::modal::escape();
        }
        wad_gtk::theme::set(&display, wad_gtk::Theme::Dark);
        crate::shell::go(crate::shell::Page::Builder);
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(1000).await;
            shot(&w, &dir.join(format!("builder-{}.png", t.name())));
        }
        builder_b(&w, &dir).await;
        crate::shell::go(crate::shell::Page::Secrets);
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("secrets-{}.png", t.name())));
        }
        // Cloud Tunnel: the tunnel, and what's streamed.
        crate::pages::secrets::show_tab("tunnel");
        for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
            wad_gtk::theme::set(&display, t);
            settle(900).await;
            shot(&w, &dir.join(format!("secrets-tunnel-{}.png", t.name())));
        }
        // A stream's link and login, to hand on.
        if let Some(share) = crate::fake::answer("/v1/streams/vanua-academy/share")
            .and_then(|v| serde_json::from_value::<wad_proto::v1::StreamShare>(v).ok())
        {
            crate::stream::share_dialog(share, "Vanua Academy".into());
            for t in [wad_gtk::Theme::Dark, wad_gtk::Theme::Light] {
                wad_gtk::theme::set(&display, t);
                settle(900).await;
                shot(&w, &dir.join(format!("stream-share-{}.png", t.name())));
            }
            crate::modal::escape();
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
