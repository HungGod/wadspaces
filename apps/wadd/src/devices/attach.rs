//! Handing a device's nodes to a running wadspace, and taking them back.
//!
//! wadd (root on the machine) does it from a thread of its own that joins
//! the container's mount namespace (after leaving the filesystem state it
//! shares with wadd's other threads), so every path resolves inside the
//! container, as `nsenter -m` would: a symlink the container planted (its
//! root is anyone with sudo in there) leads nowhere but into the container.
//! The same thread joins its network namespace to tell the apps in there,
//! as udevd would (super::uevent). Its device cgroup rules let it open
//! what's made, never make anything itself (super::cgroup_rules).

use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

/// One node to make (or take away) in a wadspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOp {
    /// /dev/…
    pub devname: String,
    pub major: u32,
    pub minor: u32,
    /// Its udev database entry (/run/udev/data/c<major>:<minor>), copied in
    /// so libudev in there knows what it is.
    pub udev_db: Option<String>,
    /// The udev event that says it came (or went), in libudev's format.
    pub event: Vec<u8>,
}

/// What makes and takes nodes (the machine's; a fake in tests).
pub trait Hands: Send + Sync {
    /// Makes the nodes in the container whose first process is `pid`, then
    /// tells it they came.
    fn give(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String>;
    /// Takes the nodes away, then tells it they went.
    fn take(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String>;
}

/// The owner of what's made inside: abc (PUID 1000), the wadspace's user.
const OWNER: u32 = 1000;

/// The machine's hands: root, joining the container's namespaces.
pub struct HostHands;

impl Hands for HostHands {
    fn give(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String> {
        let made = nodes.to_vec();
        inside(pid, move || {
            for n in &made {
                make_node(n)?;
            }
            Ok(())
        })?;
        tell(pid, nodes.iter().map(|n| &n.event))
    }

    fn take(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String> {
        let gone = nodes.to_vec();
        inside(pid, move || {
            for n in &gone {
                remove_node(n);
            }
            Ok(())
        })?;
        tell(pid, nodes.iter().map(|n| &n.event))
    }
}

/// Runs `f` on a thread in `pid`'s mount namespace.
fn inside(pid: u32, f: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), String> {
    let ns = std::fs::File::open(format!("/proc/{pid}/ns/mnt")).map_err(|e| format!("its namespace: {e}"))?;
    std::thread::spawn(move || {
        // SAFETY: unshare/setns affect this short-lived thread only (it
        // stops sharing its root and cwd with wadd's other threads first,
        // which setns into a mount namespace needs).
        unsafe {
            if libc::unshare(libc::CLONE_FS) < 0 {
                return Err(format!("unshare: {}", std::io::Error::last_os_error()));
            }
            if libc::setns(ns.as_raw_fd(), libc::CLONE_NEWNS) < 0 {
                return Err(format!("setns: {}", std::io::Error::last_os_error()));
            }
        }
        f()
    })
    .join()
    .map_err(|_| "the namespace thread panicked".to_string())?
}

/// Sends udev events in `pid`'s network namespace.
fn tell<'a>(pid: u32, events: impl Iterator<Item = &'a Vec<u8>>) -> Result<(), String> {
    let sock = super::uevent::socket_in(pid).map_err(|e| format!("its network: {e}"))?;
    for e in events {
        super::uevent::send(&sock, e).map_err(|e| format!("telling it: {e}"))?;
    }
    Ok(())
}

/// A /dev path wadd makes: under /dev, plain names only.
fn dev_path_ok(p: &str) -> bool {
    p.strip_prefix("/dev/").is_some_and(|rest| {
        !rest.is_empty()
            && rest.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(char::is_control))
    })
}

/// Makes a folder and its parents, refusing a symlink on the way (inside
/// the container's namespace, a symlink only leads around in there, but
/// the node should be where the event says).
fn make_dirs(dir: &Path) -> Result<(), String> {
    let mut at = std::path::PathBuf::from("/");
    for c in dir.components().skip(1) {
        at.push(c);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("{} isn't a folder", at.display())),
            Err(_) => std::fs::create_dir(&at).map_err(|e| format!("{}: {e}", at.display()))?,
        }
    }
    Ok(())
}

fn make_node(n: &NodeOp) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if !dev_path_ok(&n.devname) {
        return Err(format!("{} isn't a node wadd makes", n.devname));
    }
    let path = Path::new(&n.devname);
    make_dirs(path.parent().unwrap_or(Path::new("/dev")))?;
    let rdev = libc::makedev(n.major, n.minor);
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_char_device() && m.rdev() == rdev => {}
        Ok(_) => {
            // Something else by that name (in its own /dev): replaced.
            std::fs::remove_file(path).map_err(|e| format!("{}: {e}", n.devname))?;
            mknod(path, rdev)?;
        }
        Err(_) => mknod(path, rdev)?,
    }
    std::os::unix::fs::lchown(path, Some(OWNER), Some(OWNER)).map_err(|e| format!("{}: {e}", n.devname))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("{}: {e}", n.devname))?;
    if let Some(db) = &n.udev_db {
        make_dirs(Path::new("/run/udev/data"))?;
        // libudev listens to udevd's events only where udevd seems to run.
        if std::fs::symlink_metadata("/run/udev/control").is_err() {
            let _ = std::fs::write("/run/udev/control", "");
        }
        let file = Path::new("/run/udev/data").join(format!("c{}:{}", n.major, n.minor));
        let tmp = Path::new("/run/udev/data").join(format!(".c{}:{}.wadd", n.major, n.minor));
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(&tmp, db)
            .and_then(|_| std::fs::rename(&tmp, &file))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

fn mknod(path: &Path, rdev: libc::dev_t) -> Result<(), String> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: mknod with a valid C string.
    if unsafe { libc::mknod(c.as_ptr(), libc::S_IFCHR | 0o660, rdev) } < 0 {
        return Err(format!("{}: {}", path.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Takes a node away (only ours: the char device it names) with its udev
/// database entry.
fn remove_node(n: &NodeOp) {
    if !dev_path_ok(&n.devname) {
        return;
    }
    let rdev = libc::makedev(n.major, n.minor);
    if std::fs::symlink_metadata(&n.devname).is_ok_and(|m| m.file_type().is_char_device() && m.rdev() == rdev) {
        let _ = std::fs::remove_file(&n.devname);
    }
    let _ = std::fs::remove_file(format!("/run/udev/data/c{}:{}", n.major, n.minor));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_dev_paths_are_made() {
        for ok in ["/dev/input/event3", "/dev/bus/usb/001/005", "/dev/hidraw0"] {
            assert!(dev_path_ok(ok), "{ok}");
        }
        for bad in ["/dev/", "/dev/../etc/shadow", "/etc/x", "dev/x", "/dev//x", "/dev/a/./b", "/dev/a\n"] {
            assert!(!dev_path_ok(bad), "{bad}");
        }
    }
}

/// Gives WADD_TRY_NODE (a sysfs folder with a dev) to the container whose
/// first process is WADD_TRY_PID, then (with WADD_TRY_TAKE set) takes it
/// back: try-devices.sh runs it as root against its trial container.
#[cfg(test)]
#[test]
#[ignore = "root: hands a node to a running container"]
fn hands_a_node_over() {
    use super::sysfs::{Roots, uevent as read_uevent};
    let roots = Roots::default();
    let dir = std::path::PathBuf::from(std::env::var("WADD_TRY_NODE").expect("WADD_TRY_NODE"));
    let pid: u32 = std::env::var("WADD_TRY_PID").expect("WADD_TRY_PID").parse().unwrap();
    let ev = read_uevent(&dir);
    let node = super::sysfs::Node {
        devname: format!("/dev/{}", ev["DEVNAME"]),
        major: ev["MAJOR"].parse().unwrap(),
        minor: ev["MINOR"].parse().unwrap(),
        subsystem: std::fs::read_link(dir.join("subsystem")).unwrap().file_name().unwrap().to_string_lossy().into(),
        syspath: dir.clone(),
        role: super::sysfs::Role::Other,
    };
    let take = std::env::var_os("WADD_TRY_TAKE").is_some();
    let action = if take { "remove" } else { "add" };
    let op = NodeOp {
        devname: node.devname.clone(),
        major: node.major,
        minor: node.minor,
        udev_db: std::fs::read_to_string(roots.udev_data.join(format!("c{}:{}", node.major, node.minor))).ok(),
        event: super::uevent::message(&super::uevent::props_for(&roots, &node, action, 99)),
    };
    let r = if take { HostHands.take(pid, &[op]) } else { HostHands.give(pid, &[op]) };
    r.unwrap();
}
