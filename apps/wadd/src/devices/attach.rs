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

    // Drives, on the machine itself (PID 1's mounts, which wadspaces'
    // ~/Drives follow: super::server).

    /// Makes a folder (and its parents) where wadspaces' drives go.
    fn make_dir(&self, dir: &Path) -> Result<(), String>;
    /// Mounts `src` (a drive's mount) at `dst` too, making `dst`.
    fn bind(&self, src: &Path, dst: &Path) -> Result<(), String>;
    /// Unmounts `dst` and removes its folder. Busy (files open on it) is an
    /// error, unless `force`: then it's detached, and goes once they close.
    fn unbind(&self, dst: &Path, force: bool) -> Result<(), String>;
    /// Writes a disk's cache out and turns it off (`sdb`): safe to unplug.
    fn power_off(&self, disk: &str) -> Result<(), String>;
}

/// "busy" in words, for a mount something still uses.
pub const BUSY: &str = "something still has files open on it";

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

    fn make_dir(&self, dir: &Path) -> Result<(), String> {
        let dir = dir.to_path_buf();
        inside(1, move || std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display())))
    }

    fn bind(&self, src: &Path, dst: &Path) -> Result<(), String> {
        let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
        inside(1, move || {
            std::fs::create_dir_all(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string());
            let (s, d) = (c(&src)?, c(&dst)?);
            // SAFETY: mount(2) with valid C strings.
            let r = unsafe {
                libc::mount(s.as_ptr(), d.as_ptr(), std::ptr::null(), libc::MS_BIND | libc::MS_REC, std::ptr::null())
            };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                let _ = std::fs::remove_dir(&dst);
                return Err(format!("{}: {e}", dst.display()));
            }
            Ok(())
        })
    }

    fn unbind(&self, dst: &Path, force: bool) -> Result<(), String> {
        let dst = dst.to_path_buf();
        inside(1, move || {
            let d = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
            let flags = if force { libc::MNT_DETACH } else { 0 };
            // SAFETY: umount2(2) with a valid C string.
            if unsafe { libc::umount2(d.as_ptr(), flags) } < 0 {
                let e = std::io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(libc::EBUSY) => return Err(BUSY.into()),
                    // Not mounted (any more): only its folder to go.
                    Some(libc::EINVAL) | Some(libc::ENOENT) => {}
                    _ => return Err(format!("{}: {e}", dst.display())),
                }
            }
            let _ = std::fs::remove_dir(&dst);
            Ok(())
        })
    }

    fn power_off(&self, disk: &str) -> Result<(), String> {
        if disk.is_empty() || !disk.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("{disk:?} isn't a disk"));
        }
        // SAFETY: sync(2) has no arguments.
        unsafe { libc::sync() };
        // Removing the SCSI disk flushes its cache and stops it; the USB
        // device stays until it's pulled.
        let p = format!("/sys/block/{disk}/device/delete");
        std::fs::write(&p, "1").map_err(|e| format!("turning {disk} off: {e}"))
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

/// WADD_TRY_BIND=<src>:<dst> binds on the machine; WADD_TRY_UNBIND=<dst>
/// unbinds (prints "busy" when files are open): try-devices.sh's drive part.
#[cfg(test)]
#[test]
#[ignore = "root: mounts on the machine"]
fn binds_a_drive_on_the_machine() {
    if let Ok(v) = std::env::var("WADD_TRY_BIND") {
        let (src, dst) = v.split_once(':').unwrap();
        HostHands.bind(Path::new(src), Path::new(dst)).unwrap();
    }
    if let Ok(dst) = std::env::var("WADD_TRY_UNBIND") {
        match HostHands.unbind(Path::new(&dst), false) {
            Ok(()) => println!("unbound"),
            Err(e) if e == BUSY => println!("busy"),
            Err(e) => panic!("{e}"),
        }
    }
}
