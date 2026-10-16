//! Drives on the machine, for drive projects (drives.py's drive half).
//!
//! `lsblk --json` lists them, leaving out the disk the system runs from
//! (whatever holds /, /sysroot, /boot, /var or /etc), swap, loop devices and
//! LUKS/LVM members. A drive that isn't mounted is mounted on demand:
//!   - wadd as root (a machine): `systemd-mount` at /run/wadspaces-drives/<uuid>.
//!     wadd.service runs in a mount namespace of its own (ProtectSystem): a
//!     plain mount there would be invisible to podman, so PID 1 does it.
//!   - wadd as a user (a laptop): `udisksctl mount`, as a file manager would.
//!
//! FAT, exFAT and NTFS have no owners, so they're mounted as the projects
//! user. Commands go through a Runner, which the tests replace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use wad_proto::v1::Drive;

pub const DRIVES_DIR: &str = "/run/wadspaces-drives";
pub const LSBLK: [&str; 5] =
    ["lsblk", "--json", "-b", "-o", "NAME,UUID,LABEL,FSTYPE,SIZE,MOUNTPOINTS,RM,HOTPLUG,MODEL,PKNAME,TYPE"];
const SYSTEM_MOUNTS: [&str; 5] = ["/", "/sysroot", "/boot", "/var", "/etc"];
const NOT_DATA: [&str; 3] = ["swap", "crypto_LUKS", "LVM2_member"];
/// No owners on these: mounted as the projects user.
const OWNERLESS: [&str; 4] = ["vfat", "exfat", "ntfs", "ntfs3"];
const MOUNT_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DriveError {
    #[error("{0}")]
    Failed(String),
    /// Not plugged in here (409).
    #[error("{0}")]
    Missing(String),
    #[error("bad drive id {0:?}")]
    BadId(String),
}

/// Runs a command: (exit code, stdout, stderr).
#[async_trait]
pub trait Runner: Send + Sync + 'static {
    async fn run(&self, argv: &[String]) -> (i32, String, String);
}

/// The real thing, with a 30 s limit.
pub struct System;

#[async_trait]
impl Runner for System {
    async fn run(&self, argv: &[String]) -> (i32, String, String) {
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(std::process::Stdio::null()).kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
            Err(_) => (124, String::new(), format!("{} timed out", argv[0])),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                (127, String::new(), format!("{} is not installed", argv[0]))
            }
            Ok(Err(e)) => (126, String::new(), format!("{}: {e}", argv[0])),
            Ok(Ok(o)) => (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            ),
        }
    }
}

pub fn valid_uuid(u: &str) -> bool {
    (1..=64).contains(&u.len()) && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn flag(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
        || v.and_then(Value::as_i64) == Some(1)
        || matches!(v.and_then(Value::as_str), Some("1" | "true"))
}

fn str_of(d: &Value, k: &str) -> Option<String> {
    d.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn mountpoints(d: &Value) -> Vec<String> {
    match d.get("mountpoints").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).filter(|m| !m.is_empty()).map(String::from).collect(),
        None => str_of(d, "mountpoint").into_iter().collect(),
    }
}

/// lsblk's tree as a list, each with the top device's name and the model
/// of the disk it's on.
fn flatten(devs: &[Value], parent: Option<(&str, Option<&str>)>, out: &mut Vec<(Value, String, Option<String>)>) {
    for d in devs {
        let name = str_of(d, "name").unwrap_or_default();
        let disk = parent.map(|p| p.0.to_string()).unwrap_or_else(|| name.clone());
        let model = str_of(d, "model").or_else(|| parent.and_then(|p| p.1).map(String::from));
        out.push((d.clone(), disk.clone(), model.clone()));
        let children = d.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
        flatten(&children, Some((&disk, model.as_deref())), out);
    }
}

/// lsblk --json (a tree or --list): the filesystems that could hold projects.
pub fn parse_lsblk(text: &str) -> Vec<Drive> {
    let tree: Value = serde_json::from_str(if text.trim().is_empty() { "{}" } else { text }).unwrap_or(Value::Null);
    let mut devs = vec![];
    flatten(tree.get("blockdevices").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default(), None, &mut devs);
    let by_name: HashMap<String, usize> =
        devs.iter().enumerate().map(|(i, (d, _, _))| (str_of(d, "name").unwrap_or_default(), i)).collect();
    let disk_of = |i: usize| -> String {
        let mut i = i;
        let mut seen = std::collections::HashSet::new();
        while let Some(pk) = str_of(&devs[i].0, "pkname") {
            match by_name.get(&pk) {
                Some(&j) if seen.insert(pk.clone()) => i = j,
                _ => break,
            }
        }
        match str_of(&devs[i].0, "pkname") {
            None => devs[i].1.clone(),
            Some(_) => str_of(&devs[i].0, "name").unwrap_or_default(),
        }
    };
    let system_mount = |m: &str| SYSTEM_MOUNTS.contains(&m) || m.starts_with("/boot/");
    let system: std::collections::HashSet<String> =
        (0..devs.len()).filter(|&i| mountpoints(&devs[i].0).iter().any(|m| system_mount(m))).map(disk_of).collect();
    let mut out: Vec<Drive> = vec![];
    for (i, (d, _, model)) in devs.iter().enumerate() {
        let (Some(uuid), Some(fstype)) = (str_of(d, "uuid"), str_of(d, "fstype")) else { continue };
        if NOT_DATA.contains(&fstype.as_str()) || str_of(d, "type").as_deref() == Some("loop") {
            continue;
        }
        if system.contains(&disk_of(i)) || out.iter().any(|o| o.uuid == uuid) {
            continue;
        }
        let size =
            d.get("size").and_then(|s| s.as_u64().or_else(|| s.as_str().and_then(|t| t.parse().ok()))).unwrap_or(0);
        out.push(Drive {
            uuid,
            label: str_of(d, "label").unwrap_or_default(),
            fstype,
            size,
            mountpoint: mountpoints(d).into_iter().next(),
            removable: flag(d.get("rm")) || flag(d.get("hotplug")),
            model: model.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(String::from),
            disk: Some(disk_of(i)).filter(|d| !d.is_empty()),
        });
    }
    out
}

pub fn mount_options(fstype: &str, uid: u32) -> Option<String> {
    OWNERLESS.contains(&fstype).then(|| format!("uid={uid},gid={uid},umask=022"))
}

pub struct Drives {
    uid: u32,
    runner: Arc<dyn Runner>,
    root: bool,
    mountinfo: PathBuf,
    by_uuid: PathBuf,
    /// One mount at a time.
    lock: tokio::sync::Mutex<()>,
    poll: Duration,
}

impl Drives {
    pub fn new(uid: u32, runner: Arc<dyn Runner>, root: bool) -> Self {
        Self {
            uid,
            runner,
            root,
            mountinfo: "/proc/self/mountinfo".into(),
            by_uuid: "/dev/disk/by-uuid".into(),
            lock: tokio::sync::Mutex::new(()),
            poll: Duration::from_millis(300),
        }
    }

    /// Reads mounts from elsewhere (tests).
    pub fn with_paths(mut self, mountinfo: PathBuf, by_uuid: PathBuf) -> Self {
        self.mountinfo = mountinfo;
        self.by_uuid = by_uuid;
        self
    }

    async fn run(&self, argv: &[&str]) -> (i32, String, String) {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        self.runner.run(&argv).await
    }

    pub async fn list(&self) -> Result<Vec<Drive>, DriveError> {
        let (code, out, err) = self.run(&LSBLK).await;
        if code != 0 {
            let why = if err.trim().is_empty() { format!("exit {code}") } else { err.trim().to_string() };
            return Err(DriveError::Failed(format!("lsblk: {why}")));
        }
        Ok(parse_lsblk(&out))
    }

    /// Where the kernel says /dev/disk/by-uuid/<uuid> is mounted, when lsblk
    /// didn't say.
    fn mounted_by_mountinfo(&self, uuid: &str) -> Option<String> {
        let dev = std::fs::canonicalize(self.by_uuid.join(uuid)).ok()?;
        let text = std::fs::read_to_string(&self.mountinfo).ok()?;
        text.lines().find_map(|line| {
            let (pre, post) = line.split_once(" - ")?;
            let fields: Vec<&str> = pre.split_whitespace().collect();
            let src: Vec<&str> = post.split_whitespace().collect();
            (fields.len() > 4 && src.len() > 1 && std::fs::canonicalize(src[1]).ok()? == dev)
                .then(|| fields[4].replace("\\040", " "))
        })
    }

    /// The drive and where it's mounted: (None, None) when it isn't here.
    pub async fn locate(&self, uuid: &str) -> Result<(Option<Drive>, Option<String>), DriveError> {
        let Some(drive) = self.list().await?.into_iter().find(|d| d.uuid == uuid) else { return Ok((None, None)) };
        let mp = drive.mountpoint.clone().or_else(|| self.mounted_by_mountinfo(uuid));
        Ok((Some(drive), mp))
    }

    /// Where the drive is mounted, mounting it first when it isn't.
    pub async fn mount(&self, uuid: &str, label: &str, fstype: &str) -> Result<String, DriveError> {
        if !valid_uuid(uuid) {
            return Err(DriveError::BadId(uuid.into()));
        }
        let _one = self.lock.lock().await;
        let name = if label.is_empty() { uuid } else { label };
        let (drive, mp) = self.locate(uuid).await?;
        let Some(drive) = drive else { return Err(DriveError::Missing(format!("plug in the drive {name}"))) };
        if let Some(mp) = mp {
            return Ok(mp);
        }
        let dev = format!("{}/{uuid}", self.by_uuid.display());
        let failed = |out: &str, err: &str, code: i32| {
            let why = [err.trim(), out.trim()]
                .into_iter()
                .find(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("exit {code}"));
            DriveError::Failed(format!("mounting {name}: {why}"))
        };
        if self.root {
            let target = format!("{DRIVES_DIR}/{uuid}");
            let fs = if drive.fstype.is_empty() { fstype } else { &drive.fstype };
            let mut argv = vec!["systemd-mount", "--no-block", "--collect"];
            let opts = mount_options(fs, self.uid);
            if let Some(o) = &opts {
                argv.extend(["-o", o.as_str()]);
            }
            argv.extend([dev.as_str(), target.as_str()]);
            let (code, out, err) = self.run(&argv).await;
            if code != 0 {
                return Err(failed(&out, &err, code));
            }
            return self.wait(uuid, name).await;
        }
        let (code, out, err) = self.run(&["udisksctl", "mount", "-b", &dev, "--no-user-interaction"]).await;
        if code != 0 {
            return Err(failed(&out, &err, code));
        }
        // "Mounted /dev/sdb1 at /run/media/wad/STICK."
        if let Some(at) = out.lines().find_map(|l| {
            l.strip_prefix("Mounted ")
                .and_then(|r| r.split_once(" at "))
                .map(|(_, at)| at.trim_end_matches('.').to_string())
        }) {
            return Ok(at);
        }
        self.wait(uuid, name).await
    }

    /// systemd-mount --no-block returns before the mount is there.
    async fn wait(&self, uuid: &str, name: &str) -> Result<String, DriveError> {
        let end = tokio::time::Instant::now() + MOUNT_WAIT;
        loop {
            if let (_, Some(mp)) = self.locate(uuid).await? {
                tracing::info!("mounted drive {name} at {mp}");
                return Ok(mp);
            }
            if tokio::time::Instant::now() > end {
                return Err(DriveError::Failed(format!("{name} didn't get mounted")));
            }
            tokio::time::sleep(self.poll).await;
        }
    }
}

/// A drive project's folder: inside the mount, or not at all.
pub fn folder_on(mountpoint: &str, subpath: &str) -> Option<PathBuf> {
    let root = wad_store::folders::realpath(Path::new(mountpoint));
    let path = if subpath.is_empty() { root.clone() } else { wad_store::folders::realpath(&root.join(subpath)) };
    (path == root || path.starts_with(&root)).then_some(path)
}
