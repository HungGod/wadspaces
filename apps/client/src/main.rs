//! WadSpaces Client, native (GTK4): the machine's shell. It talks to wadd
//! on its socket (crates/wad-wadd-client), and draws the site's design
//! (crates/wad-gtk) without a web view.
//!
//!   client [--kiosk] [--snapshot DIR] [--version]
//!
//! --kiosk     full screen, no window frame (the machine's session)
//! --snapshot  draws each page, dark and light, to DIR/*.png and exits

mod account;
mod activity;
mod app;
mod chrome;
mod details;
mod drag;
mod fake;
mod focus;
mod images;
mod launch;
mod launcher;
mod modal;
mod notices;
mod pages;
mod rt;
mod secret_slots;
mod setup;
mod shell;
mod signins;
mod slots;
mod store;
mod stream;
mod thumb;
mod toast;
mod tour;
mod ui;
mod wallpaper_picker;

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub kiosk: bool,
    pub snapshot: Option<std::path::PathBuf>,
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "client=info,wad_gtk=info".into()),
        )
        .with_target(false)
        .init();
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--kiosk" => args.kiosk = true,
            "--snapshot" => args.snapshot = it.next().map(Into::into),
            "--version" => {
                println!("client {}", env!("CARGO_PKG_VERSION"));
                return std::process::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown option {other:?}: client [--kiosk] [--snapshot DIR] [--version]");
                return std::process::ExitCode::from(2);
            }
        }
    }
    app::run(args)
}
