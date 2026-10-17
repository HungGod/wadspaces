//! What the Registry asks of the machine: podman (containers, images),
//! systemd (the workspaces' units, and the apps' transient ones) and
//! a few probes. A trait, so the lifecycle is tested against a fake
//! (tests/registry.rs).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use wad_podman::Podman;
pub use wad_podman::{ContainerInfo, Df, DfImage, DfVolume, ImageInfo};
use wad_proto::v1::Workspace;
use wad_systemd::{Systemd, TransientSpec};

/// Lines from a build, as they come.
pub type OnLine = Box<dyn FnMut(&str) + Send>;

#[async_trait]
pub trait Backend: Send + Sync + 'static {
    /// Podman answers.
    async fn available(&self) -> bool;
    /// A container's status: running, exited, ... ("missing": there's none).
    async fn container(&self, name: &str) -> Result<String, String>;
    async fn image_exists(&self, image: &str) -> Result<bool, String>;
    async fn start(&self, unit: &str) -> Result<(), String>;
    async fn stop(&self, unit: &str) -> Result<(), String>;
    async fn restart(&self, unit: &str) -> Result<(), String>;
    /// Runs an app (a program on the machine itself) as the transient unit
    /// `unit`, as the screen's user and on its screen. Returns once it's
    /// running (its window comes later).
    async fn start_app(&self, _unit: &str, _ws: &Workspace) -> Result<(), String> {
        Err("this machine can't run apps".into())
    }
    /// A unit's state: "active", "inactive", "failed", ...
    async fn unit_state(&self, _unit: &str) -> Result<String, String> {
        Err("no systemd".into())
    }
    /// Makes the units directory hold exactly these wad-*.container files
    /// (name, text), and reloads systemd when anything changed. True if it did.
    async fn install_units(&self, units: Vec<(String, String)>) -> Result<bool, String>;
    /// A stream answers (any status under 500).
    async fn http_ok(&self, url: &str) -> bool;
    /// Free space where images are kept, in GB (None: unknown).
    async fn free_gb(&self) -> Option<f64>;
    /// The workspace a container belongs to (its wadspaces.id label).
    async fn workspace_of(&self, _container: &str) -> Option<String> {
        None
    }
    /// An image's labels; None if it isn't here.
    async fn image_labels(&self, _image: &str) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
        Err("no podman".into())
    }
    /// Where a named volume's files are; None if there's no such volume.
    async fn volume_mountpoint(&self, _name: &str) -> Option<String> {
        None
    }
    /// A running container's first process, on the machine (None: it isn't
    /// running).
    async fn container_pid(&self, _name: &str) -> Result<Option<u32>, String> {
        Err("no podman".into())
    }
    /// The github_token secret (git's credentials for github.com).
    async fn github_token(&self) -> Option<String> {
        None
    }
    /// The names of podman's secrets.
    async fn secret_names(&self) -> Result<Vec<String>, String> {
        Err("no podman".into())
    }
    /// A secret's value, for wadd's own use (a stream's password, handed on
    /// with its link); None if there's no such secret.
    async fn secret_value(&self, _name: &str) -> Result<Option<String>, String> {
        Err("no podman".into())
    }
    /// The running containers that carry a label.
    async fn containers_labelled(&self, _label: &str) -> Result<Vec<ContainerInfo>, String> {
        Err("no podman".into())
    }
    /// Stops a container itself (its unit may be gone). False if there was none.
    async fn stop_container(&self, _name: &str) -> Result<bool, String> {
        Err("no podman".into())
    }
    /// podman's own description.
    async fn podman_info(&self) -> Result<serde_json::Value, String> {
        Err("no podman".into())
    }
    /// A container's last lines of output; None if there's no container.
    async fn container_logs(&self, _name: &str, _lines: usize) -> Result<Option<String>, String> {
        Err("no podman".into())
    }
    /// Creates (replacing) a secret. Its value only ever goes to podman.
    async fn create_secret(&self, _name: &str, _value: &[u8]) -> Result<(), String> {
        Err("no podman".into())
    }
    /// False if there was none.
    async fn delete_secret(&self, _name: &str) -> Result<bool, String> {
        Err("no podman".into())
    }
    /// Builds an image from a tar build context on `base` (its BASE_IMAGE
    /// build argument), with these labels. Dropping the future stops the build.
    async fn build(
        &self,
        _context: Vec<u8>,
        _tag: &str,
        _base: &str,
        _labels: serde_json::Map<String, serde_json::Value>,
        _on_line: OnLine,
    ) -> Result<(), String> {
        Err("no podman".into())
    }
    /// An image's id, age, size and labels; None if it isn't here.
    async fn image_info(&self, _image: &str) -> Result<Option<ImageInfo>, String> {
        Err("no podman".into())
    }
    /// The images that carry a label.
    async fn images_labelled(&self, _label: &str) -> Result<Vec<ImageInfo>, String> {
        Err("no podman".into())
    }
    /// Each named volume's size in bytes.
    async fn volume_sizes(&self) -> Result<Vec<(String, u64)>, String> {
        Err("no podman".into())
    }
    /// Removes an image (just its name, when others are built on it). False
    /// if there was none.
    async fn remove_image(&self, _image: &str) -> Result<bool, String> {
        Err("no podman".into())
    }
    /// Removes an image by id, every name it has; an error while anything
    /// uses it. False if there was none.
    async fn remove_image_id(&self, _id: &str) -> Result<bool, String> {
        Err("no podman".into())
    }
    /// Removes a named volume and its files. False if there was none.
    async fn remove_volume(&self, _name: &str) -> Result<bool, String> {
        Err("no podman".into())
    }
    /// What podman keeps on the disk, image by image.
    async fn system_df(&self) -> Result<Df, String> {
        Err("no podman".into())
    }
    /// Every image here, with its labels and names.
    async fn images_all(&self) -> Result<Vec<ImageInfo>, String> {
        Err("no podman".into())
    }
}

/// The machine as wadd's config describes it: podman's socket, systemd (the
/// system manager, or yours with --user), where units go and the screen
/// apps draw on.
pub async fn connect(config: &wad_config::Config, profile: wad_config::Profile) -> Result<Real, String> {
    let daemon = &config.daemon;
    let systemd = match profile {
        wad_config::Profile::System => Systemd::system().await,
        wad_config::Profile::User => Systemd::user().await,
    }
    .map_err(|e| format!("systemd: {e}"))?;
    Ok(Real {
        podman: Podman::new(&daemon.podman_socket),
        systemd,
        units_dir: daemon.units_dir.clone(),
        http: reqwest::Client::new(),
        start_timeout: Duration::from_secs(daemon.ready_timeout_s),
        sway: config.display.enabled.then(|| wad_sway::Sway::new(crate::sway_locate(&config.display))),
        as_screen_user: profile == wad_config::Profile::System,
    })
}

/// Whose screen it is, for an app the system manager runs (a user manager
/// runs it as you): name, primary group and home, from the password database.
pub struct ScreenUser {
    pub name: String,
    pub group: String,
    pub home: String,
}

/// The transient unit an app runs in: its program, as the screen's user (on
/// a machine), with what sway gives its own children: the runtime dir, the
/// Wayland socket, the session bus, language and cursor. The workspace's
/// own env comes last, so it wins.
pub fn app_spec(ws: &Workspace, session: &wad_sway::Session, user: Option<&ScreenUser>) -> TransientSpec {
    let runtime = session.runtime_dir.to_string_lossy().into_owned();
    let mut environment =
        vec![format!("XDG_RUNTIME_DIR={runtime}"), format!("WAYLAND_DISPLAY={}", session.wayland_display)];
    if let Some(u) = user {
        environment.push(format!("HOME={}", u.home));
        environment.push(format!("USER={}", u.name));
        environment.push(format!("LOGNAME={}", u.name));
    }
    environment.extend(session.env.iter().map(|(k, v)| format!("{k}={v}")));
    if !session.env.iter().any(|(k, _)| k == "DBUS_SESSION_BUS_ADDRESS") && session.runtime_dir.join("bus").exists() {
        environment.push(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={runtime}/bus"));
    }
    environment.extend(ws.env.iter().map(|(k, v)| format!("{k}={v}")));
    TransientSpec {
        description: format!("WadSpaces app: {}", ws.name),
        exec: ws.exec.clone(),
        user: user.map(|u| u.name.clone()),
        group: user.map(|u| u.group.clone()),
        working_directory: user.map(|u| u.home.clone()),
        environment,
    }
}

fn screen_user(uid: u32) -> Result<ScreenUser, String> {
    use nix::unistd::{Gid, Group, Uid, User};
    let u = User::from_uid(Uid::from_raw(uid)).ok().flatten().ok_or_else(|| format!("no user with uid {uid}"))?;
    let group = Group::from_gid(Gid::from_raw(u.gid.as_raw()))
        .ok()
        .flatten()
        .map(|g| g.name)
        .unwrap_or_else(|| u.gid.to_string());
    Ok(ScreenUser { name: u.name, group, home: u.dir.to_string_lossy().into_owned() })
}

/// No machine to manage (systemd unreachable, or a test): everything
/// workspaces need fails with why, and the rest of wadd still answers.
pub struct Offline(pub String);

#[async_trait]
impl Backend for Offline {
    async fn available(&self) -> bool {
        false
    }
    async fn container(&self, _: &str) -> Result<String, String> {
        Err(self.0.clone())
    }
    async fn image_exists(&self, _: &str) -> Result<bool, String> {
        Err(self.0.clone())
    }
    async fn start(&self, _: &str) -> Result<(), String> {
        Err(self.0.clone())
    }
    async fn stop(&self, _: &str) -> Result<(), String> {
        Err(self.0.clone())
    }
    async fn restart(&self, _: &str) -> Result<(), String> {
        Err(self.0.clone())
    }
    async fn install_units(&self, _: Vec<(String, String)>) -> Result<bool, String> {
        Err(self.0.clone())
    }
    async fn http_ok(&self, _: &str) -> bool {
        false
    }
    async fn free_gb(&self) -> Option<f64> {
        None
    }
    async fn image_labels(&self, _: &str) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
        Err(self.0.clone())
    }
    async fn secret_names(&self) -> Result<Vec<String>, String> {
        Err(self.0.clone())
    }
    async fn create_secret(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err(self.0.clone())
    }
    async fn delete_secret(&self, _: &str) -> Result<bool, String> {
        Err(self.0.clone())
    }
    async fn podman_info(&self) -> Result<serde_json::Value, String> {
        Err(self.0.clone())
    }
    async fn container_logs(&self, _: &str, _: usize) -> Result<Option<String>, String> {
        Err(self.0.clone())
    }
}

pub struct Real {
    pub podman: Podman,
    pub systemd: Systemd,
    pub units_dir: PathBuf,
    pub http: reqwest::Client,
    pub start_timeout: Duration,
    /// The screen apps draw on (none: apps can't run).
    pub sway: Option<wad_sway::Sway>,
    /// Apps run as the screen's user (the system manager), not as wadd.
    pub as_screen_user: bool,
}

#[async_trait]
impl Backend for Real {
    async fn available(&self) -> bool {
        // Socket-activated podman stops its API service after a few idle
        // seconds and starts it again on the next call; a call that lands
        // just as it stops goes unanswered. Asked again before giving up.
        if self.podman.ping().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.podman.ping().await
    }

    async fn container(&self, name: &str) -> Result<String, String> {
        self.podman.container_status(name).await.map_err(|e| e.to_string())
    }

    async fn image_exists(&self, image: &str) -> Result<bool, String> {
        self.podman.image_exists(image).await.map_err(|e| e.to_string())
    }

    async fn start(&self, unit: &str) -> Result<(), String> {
        self.systemd.start(unit, self.start_timeout).await.map_err(|e| e.to_string())
    }

    async fn stop(&self, unit: &str) -> Result<(), String> {
        self.systemd.stop(unit, Duration::from_secs(120)).await.map_err(|e| e.to_string())
    }

    async fn restart(&self, unit: &str) -> Result<(), String> {
        self.systemd.restart(unit, self.start_timeout).await.map_err(|e| e.to_string())
    }

    async fn start_app(&self, unit: &str, ws: &Workspace) -> Result<(), String> {
        let sway = self.sway.as_ref().ok_or("there's no screen to show it on")?;
        let session = sway.session().map_err(|e| e.to_string())?;
        let user = if self.as_screen_user { Some(screen_user(session.uid)?) } else { None };
        let spec = app_spec(ws, &session, user.as_ref());
        self.systemd.start_transient(unit, &spec, self.start_timeout).await.map_err(|e| e.to_string())
    }

    async fn unit_state(&self, unit: &str) -> Result<String, String> {
        self.systemd.active_state(unit).await.map_err(|e| e.to_string())
    }

    async fn install_units(&self, units: Vec<(String, String)>) -> Result<bool, String> {
        let dir = &self.units_dir;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut changed = false;
        for (name, text) in &units {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                let tmp = dir.join(format!(".{name}.tmp"));
                std::fs::write(&tmp, text)
                    .and_then(|_| std::fs::rename(&tmp, &path))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                changed = true;
            }
        }
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ours = name.starts_with("wad-") && (name.ends_with(".container") || name.ends_with(".volume"));
            if ours && !units.iter().any(|(n, _)| *n == name) {
                std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                changed = true;
            }
        }
        if changed {
            self.systemd.reload().await.map_err(|e| e.to_string())?;
        }
        Ok(changed)
    }

    async fn http_ok(&self, url: &str) -> bool {
        matches!(self.http.get(url).timeout(Duration::from_secs(2)).send().await, Ok(r) if r.status().as_u16() < 500)
    }

    async fn image_labels(&self, image: &str) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
        self.podman.image_labels(image).await.map_err(|e| e.to_string())
    }

    async fn container_pid(&self, name: &str) -> Result<Option<u32>, String> {
        let info = self.podman.inspect_container(name).await.map_err(|e| e.to_string())?;
        Ok(info
            .and_then(|i| i.pointer("/State/Pid").and_then(serde_json::Value::as_u64))
            .filter(|p| *p > 0)
            .map(|p| p as u32))
    }

    async fn volume_mountpoint(&self, name: &str) -> Option<String> {
        match self.podman.volume_mountpoint(name).await {
            Ok(m) => m,
            Err(e) => {
                tracing::info!("looking in volume {name}: {e}");
                None
            }
        }
    }

    async fn secret_names(&self) -> Result<Vec<String>, String> {
        self.podman.secret_names().await.map_err(|e| e.to_string())
    }

    async fn secret_value(&self, name: &str) -> Result<Option<String>, String> {
        self.podman.secret_value(name).await.map_err(|e| e.to_string())
    }

    async fn containers_labelled(&self, label: &str) -> Result<Vec<ContainerInfo>, String> {
        self.podman.containers_labelled(label).await.map_err(|e| e.to_string())
    }

    async fn stop_container(&self, name: &str) -> Result<bool, String> {
        self.podman.stop_container(name, 10).await.map_err(|e| e.to_string())
    }

    async fn podman_info(&self) -> Result<serde_json::Value, String> {
        self.podman.info().await.map_err(|e| e.to_string())
    }

    async fn container_logs(&self, name: &str, lines: usize) -> Result<Option<String>, String> {
        self.podman.container_logs(name, lines).await.map_err(|e| e.to_string())
    }

    async fn create_secret(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.podman.create_secret(name, value).await.map_err(|e| e.to_string())
    }

    async fn delete_secret(&self, name: &str) -> Result<bool, String> {
        self.podman.delete_secret(name).await.map_err(|e| e.to_string())
    }

    async fn build(
        &self,
        context: Vec<u8>,
        tag: &str,
        base: &str,
        labels: serde_json::Map<String, serde_json::Value>,
        mut on_line: OnLine,
    ) -> Result<(), String> {
        let mut args = serde_json::Map::new();
        args.insert("BASE_IMAGE".into(), base.into());
        self.podman.build(context, tag, &args, &labels, |l| on_line(l)).await.map(drop).map_err(|e| e.to_string())
    }

    async fn image_info(&self, image: &str) -> Result<Option<ImageInfo>, String> {
        self.podman.image_info(image).await.map_err(|e| e.to_string())
    }

    async fn images_labelled(&self, label: &str) -> Result<Vec<ImageInfo>, String> {
        self.podman.images_labelled(label).await.map_err(|e| e.to_string())
    }

    async fn volume_sizes(&self) -> Result<Vec<(String, u64)>, String> {
        self.podman.volume_sizes().await.map_err(|e| e.to_string())
    }

    async fn remove_image(&self, image: &str) -> Result<bool, String> {
        self.podman.remove_image(image).await.map_err(|e| e.to_string())
    }

    async fn remove_image_id(&self, id: &str) -> Result<bool, String> {
        self.podman.remove_image_id(id).await.map_err(|e| e.to_string())
    }

    async fn remove_volume(&self, name: &str) -> Result<bool, String> {
        self.podman.remove_volume(name).await.map_err(|e| e.to_string())
    }

    async fn system_df(&self) -> Result<Df, String> {
        self.podman.system_df().await.map_err(|e| e.to_string())
    }

    async fn images_all(&self) -> Result<Vec<ImageInfo>, String> {
        self.podman.images_all().await.map_err(|e| e.to_string())
    }

    async fn github_token(&self) -> Option<String> {
        match self.podman.secret_value("github_token").await {
            Ok(v) => v.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
            Err(e) => {
                tracing::debug!("github_token from podman: {e}");
                None
            }
        }
    }

    async fn workspace_of(&self, container: &str) -> Option<String> {
        let info = self.podman.inspect_container(container).await.ok()??;
        info.pointer("/Config/Labels/wadspaces.id").and_then(|v| v.as_str()).map(String::from)
    }

    async fn free_gb(&self) -> Option<f64> {
        let root = self.podman.graph_root().await.ok().flatten().map(PathBuf::from);
        let path = root.filter(|r| r.is_dir()).unwrap_or_else(|| "/".into());
        let st = nix::sys::statvfs::statvfs(&path).ok()?;
        Some(st.blocks_available() as f64 * st.fragment_size() as f64 / 1e9)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_runs_as_the_screens_user_with_what_sway_gives_its_children() {
        let ws = Workspace {
            id: "terminal".into(),
            name: "Terminal".into(),
            kind: wad_proto::v1::Kind::App,
            exec: vec!["/usr/bin/foot".into(), "--title".into(), "Terminal".into()],
            image: String::new(),
            display: wad_proto::v1::Display::Host,
            port: None,
            hotkey: None,
            icon: None,
            enabled: true,
            autostart: false,
            container_name: "wad-terminal".into(),
            container_port: 3000,
            env: vec![("TERM".into(), "foot".into())],
            secrets: vec![],
            volumes: vec![],
            devices: vec![],
            shm_size: None,
            projects: vec![],
            signins: vec![],
            ephemeral: false,
            desktop: vec![],
        };
        let session = wad_sway::Session {
            uid: 1000,
            runtime_dir: "/run/user/1000".into(),
            wayland_display: "wayland-1".into(),
            env: vec![
                ("DBUS_SESSION_BUS_ADDRESS".into(), "unix:path=/tmp/dbus-x".into()),
                ("LANG".into(), "C.UTF-8".into()),
            ],
        };
        let wad = ScreenUser { name: "wad".into(), group: "wad".into(), home: "/var/home/wad".into() };
        let spec = app_spec(&ws, &session, Some(&wad));
        assert_eq!(spec.exec, ws.exec);
        assert_eq!((spec.user.as_deref(), spec.group.as_deref()), (Some("wad"), Some("wad")));
        assert_eq!(spec.working_directory.as_deref(), Some("/var/home/wad"));
        assert_eq!(
            spec.environment,
            [
                "XDG_RUNTIME_DIR=/run/user/1000",
                "WAYLAND_DISPLAY=wayland-1",
                "HOME=/var/home/wad",
                "USER=wad",
                "LOGNAME=wad",
                "DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/dbus-x",
                "LANG=C.UTF-8",
                "TERM=foot",
            ]
        );
        // Your user manager runs it as you (a laptop): no user, no HOME.
        let mine = app_spec(&ws, &session, None);
        assert_eq!((mine.user, mine.working_directory), (None, None));
        assert!(!mine.environment.iter().any(|e| e.starts_with("HOME=")));
    }
}
