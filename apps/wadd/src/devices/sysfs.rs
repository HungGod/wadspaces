//! The devices plugged into the machine, read from sysfs: each physical
//! device (a USB device, or a Bluetooth HID one) with the nodes under it
//! (/dev/input/event*, js*, hidraw*, video*, ttyUSB*/ttyACM*, and a USB
//! device's own /dev/bus/usb node), and what kind of device it is, from
//! udev's database (/run/udev/data).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wad_proto::v1::{DeviceBus, DeviceKind};

/// Where sysfs and udev's database are (a tree of their own in tests).
#[derive(Debug, Clone)]
pub struct Roots {
    pub sys: PathBuf,
    pub udev_data: PathBuf,
    /// The machine's mounts (PID 1's: wadd's own namespace is ProtectSystem's).
    pub mountinfo: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self { sys: "/sys".into(), udev_data: "/run/udev/data".into(), mountinfo: "/proc/1/mountinfo".into() }
    }
}

/// What a node is to the machine: keyboards and pointers stay with it
/// (sway has them, and wad-input grabs keyboards).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Keyboard,
    Pointer,
    Joystick,
    Other,
}

/// A device node under a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// /dev/…
    pub devname: String,
    pub major: u32,
    pub minor: u32,
    pub subsystem: String,
    /// Its sysfs folder (what a udev event's DEVPATH names, under /sys).
    pub syspath: PathBuf,
    pub role: Role,
}

/// A physical device and its nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What it's known by, across plugs: `usb:<vid>:<pid>:<serial>`, or
    /// `usb:<vid>:<pid>@<port>` with no serial; `bt:<MAC>`.
    pub key: String,
    pub name: String,
    pub bus: DeviceBus,
    pub kind: DeviceKind,
    pub nodes: Vec<Node>,
}

impl Found {
    /// The nodes a wadspace gets: not a keyboard's or a pointer's (those
    /// stay with the machine), nor anything not handed out.
    pub fn handed_nodes(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| matches!(n.role, Role::Joystick | Role::Other))
            .filter(|n| HANDED_SUBSYSTEMS.contains(&n.subsystem.as_str()))
            .collect()
    }
}

/// The node kinds a wadspace may be handed (super::cgroup_rules' majors).
const HANDED_SUBSYSTEMS: [&str; 5] = ["input", "hidraw", "video4linux", "tty", "usb"];

/// Whether a kind goes to wadspaces (the rest work through the machine).
pub fn attachable(kind: DeviceKind) -> bool {
    matches!(
        kind,
        DeviceKind::Gamepad
            | DeviceKind::Webcam
            | DeviceKind::SecurityKey
            | DeviceKind::Serial
            | DeviceKind::Storage
            | DeviceKind::Other
    )
}

/// Every device plugged in now, by key.
pub fn scan(roots: &Roots) -> BTreeMap<String, Found> {
    let mut out = BTreeMap::new();
    for d in list(&roots.sys.join("bus/usb/devices")) {
        if let Some(f) = usb_device(roots, &d) {
            out.insert(f.key.clone(), f);
        }
    }
    for d in list(&roots.sys.join("bus/hid/devices")) {
        if let Some(f) = bluetooth_hid(roots, &d) {
            // One Bluetooth device can be several HID devices: one entry.
            match out.get_mut(&f.key) {
                Some(have) => have.nodes.extend(f.nodes),
                None => {
                    out.insert(f.key.clone(), f);
                }
            }
        }
    }
    for f in out.values_mut() {
        f.nodes.sort_by(|a, b| a.devname.cmp(&b.devname));
        f.kind = kind_of(f.kind, &f.nodes, roots);
    }
    // The disk the system runs from (the stick, on a machine) is nobody's
    // to hand out or eject.
    let system = system_mounts(&std::fs::read_to_string(&roots.mountinfo).unwrap_or_default());
    out.retain(|_, f| {
        !f.nodes.iter().any(|n| {
            n.subsystem == "block" && system.iter().any(|(mm, src)| *mm == (n.major, n.minor) || *src == n.devname)
        })
    });
    out
}

/// The devices the system's own mounts are on: (major, minor) and source,
/// from a mountinfo (btrfs gives anonymous numbers: its source tells).
pub fn system_mounts(mountinfo: &str) -> Vec<((u32, u32), String)> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let (pre, post) = line.split_once(" - ")?;
            let f: Vec<&str> = pre.split_whitespace().collect();
            let at = *f.get(4)?;
            let system = matches!(at, "/" | "/sysroot" | "/var" | "/etc" | "/usr") || at.starts_with("/boot");
            if !system {
                return None;
            }
            let (ma, mi) = f.get(2)?.split_once(':')?;
            let src = post.split_whitespace().nth(1).unwrap_or("").to_string();
            Some(((ma.parse().ok()?, mi.parse().ok()?), src))
        })
        .collect()
}

/// A folder's entries, resolved (sysfs's bus lists are symlinks).
fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::canonicalize(e.path()).ok())
        .collect();
    v.sort();
    v
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// A sysfs uevent file as key=value pairs.
pub fn uevent(dir: &Path) -> BTreeMap<String, String> {
    read(&dir.join("uevent"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// udev's database entry for a node: its properties (`E:` lines).
pub fn udev_props(roots: &Roots, major: u32, minor: u32) -> BTreeMap<String, String> {
    read(&roots.udev_data.join(format!("c{major}:{minor}")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("E:"))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn usb_device(roots: &Roots, dir: &Path) -> Option<Found> {
    let vid = read(&dir.join("idVendor"))?;
    let pid = read(&dir.join("idProduct"))?;
    // Hubs (root hubs too) aren't devices anyone plugs in to use, and a
    // wireless controller (class e0: the machine's Bluetooth radio) is how
    // Bluetooth devices reach it.
    if matches!(read(&dir.join("bDeviceClass")).as_deref(), Some("09" | "e0")) || has_interface_class(dir, "e0") {
        return None;
    }
    let port = dir.file_name()?.to_string_lossy().into_owned();
    let key = match read(&dir.join("serial")).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic())) {
        Some(serial) => format!("usb:{vid}:{pid}:{serial}"),
        None => format!("usb:{vid}:{pid}@{port}"),
    };
    let product = read(&dir.join("product")).unwrap_or_default();
    let maker = read(&dir.join("manufacturer")).unwrap_or_default();
    let name = if product.is_empty() && maker.is_empty() {
        format!("USB device {vid}:{pid}")
    } else if product.is_empty() {
        maker
    } else if maker.is_empty() || product.starts_with(&maker) {
        product
    } else {
        format!("{maker} {product}")
    };
    let mut nodes = vec![];
    walk(roots, dir, true, &mut nodes);
    let kind = if nodes.iter().any(|n| n.subsystem == "block") {
        DeviceKind::Storage
    } else if dir_has_class(dir, "sound") {
        DeviceKind::Audio
    } else {
        DeviceKind::Other
    };
    Some(Found { key, name, bus: DeviceBus::Usb, kind, nodes })
}

/// A HID device on Bluetooth (BlueZ's: bus 0005, under uhid), keyed by the
/// remote's address (HID_UNIQ).
fn bluetooth_hid(roots: &Roots, dir: &Path) -> Option<Found> {
    let ev = uevent(dir);
    let bus = ev.get("HID_ID")?.split(':').next()?.to_string();
    if bus != "0005" {
        return None;
    }
    let mac = ev.get("HID_UNIQ").map(|m| m.to_uppercase()).filter(|m| is_mac(m))?;
    let name = ev.get("HID_NAME").cloned().unwrap_or_else(|| format!("Bluetooth device {mac}"));
    let mut nodes = vec![];
    walk(roots, dir, false, &mut nodes);
    Some(Found { key: format!("bt:{mac}"), name, bus: DeviceBus::Bluetooth, kind: DeviceKind::Other, nodes })
}

/// Whether one of a USB device's interfaces is of this class.
fn has_interface_class(dir: &Path, class: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| read(&e.path().join("bInterfaceClass")).as_deref() == Some(class))
}

pub fn is_mac(s: &str) -> bool {
    s.len() == 17
        && s.split(':').count() == 6
        && s.split(':').all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether any folder under `dir` is in sysfs class `class` (a sound card's
/// `sound/card0`).
fn dir_has_class(dir: &Path, class: &str) -> bool {
    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        is_dir && (e.file_name() == class || (!p.join("idVendor").exists() && dir_has_class(&p, class)))
    })
}

/// The nodes under a device's folder: every folder with a `dev`, not going
/// into another USB device (a hub's) or a symlink.
fn walk(roots: &Roots, dir: &Path, top: bool, out: &mut Vec<Node>) {
    if !top && dir.join("idVendor").exists() {
        return;
    }
    if let Some(node) = node_at(roots, dir) {
        out.push(node);
    }
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    kids.sort();
    for k in kids {
        walk(roots, &k, false, out);
    }
}

fn node_at(roots: &Roots, dir: &Path) -> Option<Node> {
    let (major, minor) =
        read(&dir.join("dev"))?.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))?;
    let ev = uevent(dir);
    let devname = ev.get("DEVNAME")?;
    if devname.contains("..") || devname.starts_with('/') {
        return None;
    }
    let subsystem = std::fs::read_link(dir.join("subsystem"))
        .ok()
        .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let role = if subsystem == "input" {
        let p = udev_props(roots, major, minor);
        let yes = |k: &str| p.get(k).is_some_and(|v| v == "1");
        if yes("ID_INPUT_KEYBOARD") {
            Role::Keyboard
        } else if yes("ID_INPUT_MOUSE") || yes("ID_INPUT_TOUCHPAD") || yes("ID_INPUT_TABLET") {
            Role::Pointer
        } else if yes("ID_INPUT_JOYSTICK") {
            Role::Joystick
        } else {
            Role::Other
        }
    } else {
        Role::Other
    };
    Some(Node { devname: format!("/dev/{devname}"), major, minor, subsystem, syspath: dir.to_path_buf(), role })
}

/// What a device is, from its nodes: what it's handed out for first, then
/// what keeps it with the machine.
fn kind_of(seen: DeviceKind, nodes: &[Node], roots: &Roots) -> DeviceKind {
    let has = |f: &dyn Fn(&Node) -> bool| nodes.iter().any(f);
    if has(&|n| n.role == Role::Joystick) {
        DeviceKind::Gamepad
    } else if has(&|n| n.subsystem == "video4linux") {
        DeviceKind::Webcam
    } else if has(&|n| {
        n.subsystem == "hidraw" && {
            let p = udev_props(roots, n.major, n.minor);
            p.get("ID_FIDO_TOKEN").or(p.get("ID_SECURITY_TOKEN")).is_some_and(|v| v == "1")
        }
    }) {
        DeviceKind::SecurityKey
    } else if has(&|n| n.subsystem == "tty") {
        DeviceKind::Serial
    } else if seen == DeviceKind::Storage {
        DeviceKind::Storage
    } else if has(&|n| n.role == Role::Keyboard) {
        DeviceKind::Keyboard
    } else if has(&|n| n.role == Role::Pointer) {
        DeviceKind::Mouse
    } else {
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sysfs tree in a tempdir: devices under /sys/devices, the bus lists
    /// as symlinks, nodes as folders with dev/uevent and a subsystem link.
    struct Tree {
        _d: tempfile::TempDir,
        roots: Roots,
    }

    impl Tree {
        fn new() -> Self {
            let d = tempfile::tempdir().unwrap();
            let roots = Roots {
                sys: d.path().join("sys"),
                udev_data: d.path().join("udev"),
                mountinfo: d.path().join("mountinfo"),
            };
            for p in ["bus/usb/devices", "bus/hid/devices", "class"] {
                std::fs::create_dir_all(roots.sys.join(p)).unwrap();
            }
            std::fs::create_dir_all(&roots.udev_data).unwrap();
            Self { _d: d, roots }
        }

        fn usb(&self, port: &str, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.roots.sys.join("devices/pci0000:00/usb1").join(port);
            std::fs::create_dir_all(&dir).unwrap();
            for (k, v) in files {
                std::fs::write(dir.join(k), format!("{v}\n")).unwrap();
            }
            std::os::unix::fs::symlink(&dir, self.roots.sys.join("bus/usb/devices").join(port)).unwrap();
            dir
        }

        fn node(&self, under: &Path, rel: &str, subsystem: &str, devname: &str, dev: (u32, u32), udev: &[&str]) {
            let dir = under.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("dev"), format!("{}:{}\n", dev.0, dev.1)).unwrap();
            std::fs::write(dir.join("uevent"), format!("MAJOR={}\nMINOR={}\nDEVNAME={devname}\n", dev.0, dev.1))
                .unwrap();
            let class = self.roots.sys.join("class").join(subsystem);
            std::fs::create_dir_all(&class).unwrap();
            std::os::unix::fs::symlink(&class, dir.join("subsystem")).unwrap();
            let db: String = udev.iter().map(|l| format!("E:{l}\n")).collect();
            std::fs::write(self.roots.udev_data.join(format!("c{}:{}", dev.0, dev.1)), db).unwrap();
        }
    }

    fn usb_files<'a>(vid: &'a str, pid: &'a str, product: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![("idVendor", vid), ("idProduct", pid), ("product", product), ("bDeviceClass", "00")]
    }

    #[test]
    fn a_gamepad_hands_out_its_pad_but_not_its_keyboard_part() {
        let t = Tree::new();
        let mut files = usb_files("2dc8", "6101", "8BitDo Pro 2");
        files.push(("manufacturer", "8BitDo"));
        files.push(("serial", "E4:17:D8:00:11:22"));
        let pad = t.usb("1-2", &files);
        t.node(&pad, "", "usb", "bus/usb/001/005", (189, 4), &[]);
        t.node(
            &pad,
            "1-2:1.0/0003:2DC8:6101.0001/input/input20/event20",
            "input",
            "input/event20",
            (13, 84),
            &["ID_INPUT=1", "ID_INPUT_JOYSTICK=1"],
        );
        t.node(
            &pad,
            "1-2:1.0/0003:2DC8:6101.0001/input/input20/js0",
            "input",
            "input/js0",
            (13, 0),
            &["ID_INPUT_JOYSTICK=1"],
        );
        t.node(&pad, "1-2:1.0/0003:2DC8:6101.0001/hidraw/hidraw3", "hidraw", "hidraw3", (241, 3), &[]);
        t.node(
            &pad,
            "1-2:1.1/0003:2DC8:6101.0002/input/input21/event21",
            "input",
            "input/event21",
            (13, 85),
            &["ID_INPUT_KEYBOARD=1"],
        );
        let all = scan(&t.roots);
        let f = &all["usb:2dc8:6101:E4:17:D8:00:11:22"];
        assert_eq!((f.name.as_str(), f.kind, f.bus), ("8BitDo Pro 2", DeviceKind::Gamepad, DeviceBus::Usb));
        let handed: Vec<&str> = f.handed_nodes().iter().map(|n| n.devname.as_str()).collect();
        assert_eq!(handed, ["/dev/bus/usb/001/005", "/dev/hidraw3", "/dev/input/event20", "/dev/input/js0"]);
        assert!(attachable(f.kind));
    }

    #[test]
    fn kinds_come_from_what_is_under_a_device() {
        let t = Tree::new();
        let cam = t.usb("1-3", &usb_files("046d", "085b", "C925e"));
        t.node(&cam, "1-3:1.0/video4linux/video0", "video4linux", "video0", (81, 0), &[]);
        t.node(&cam, "1-3:1.2/sound/card1/controlC1", "sound", "snd/controlC1", (116, 9), &[]);
        let key = t.usb("1-4", &usb_files("1050", "0407", "YubiKey OTP+FIDO+CCID"));
        t.node(&key, "1-4:1.1/0003:1050:0407.0003/hidraw/hidraw1", "hidraw", "hidraw1", (241, 1), &["ID_FIDO_TOKEN=1"]);
        let ard = t.usb("1-5", &usb_files("2341", "0043", ""));
        t.node(&ard, "1-5:1.0/tty/ttyACM0", "tty", "ttyACM0", (166, 0), &[]);
        let kb = t.usb("1-6", &usb_files("04d9", "0006", "Keyboard"));
        t.node(&kb, "1-6:1.0/input/input5/event5", "input", "input/event5", (13, 69), &["ID_INPUT_KEYBOARD=1"]);
        let stick = t.usb("1-7", &usb_files("0781", "5581", "Ultra"));
        t.node(&stick, "1-7:1.0/host0/target0:0:0/0:0:0:0/block/sda", "block", "sda", (8, 0), &[]);
        let head = t.usb("1-8", &usb_files("0d8c", "0014", "Headset"));
        std::fs::create_dir_all(head.join("1-8:1.0/sound/card2")).unwrap();
        t.usb("usb1", &[("idVendor", "1d6b"), ("idProduct", "0002"), ("bDeviceClass", "09")]);
        let radio = t.usb("1-9", &usb_files("1286", "204c", "Bluetooth and Wireless LAN"));
        std::fs::create_dir_all(radio.join("1-9:1.0")).unwrap();
        std::fs::write(radio.join("1-9:1.0/bInterfaceClass"), "e0\n").unwrap();
        let kinds: Vec<(String, DeviceKind)> = scan(&t.roots).into_values().map(|f| (f.name, f.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("C925e".into(), DeviceKind::Webcam),
                ("Keyboard".into(), DeviceKind::Keyboard),
                ("Ultra".into(), DeviceKind::Storage),
                ("Headset".into(), DeviceKind::Audio),
                ("YubiKey OTP+FIDO+CCID".into(), DeviceKind::SecurityKey),
                ("USB device 2341:0043".into(), DeviceKind::Serial),
            ]
        );
        // No serial: keyed by its port.
        assert!(scan(&t.roots).contains_key("usb:2341:0043@1-5"));
    }

    #[test]
    fn the_disk_the_system_runs_from_isnt_listed() {
        let t = Tree::new();
        let stick = t.usb("2-1", &usb_files("18a5", "0243", "STORE N GO"));
        t.node(&stick, "2-1:1.0/host0/target0:0:0/0:0:0:0/block/sda", "block", "sda", (8, 0), &[]);
        t.node(&stick, "2-1:1.0/host0/target0:0:0/0:0:0:0/block/sda/sda3", "block", "sda3", (8, 3), &[]);
        let disk = t.usb("2-2", &usb_files("0480", "0820", "EXTERNAL_USB"));
        t.node(&disk, "2-2:1.0/host1/target1:0:0/1:0:0:0/block/sdb", "block", "sdb", (8, 16), &[]);
        std::fs::write(
            &t.roots.mountinfo,
            "1 0 8:3 / /sysroot rw - xfs /dev/sda3 rw\n2 1 8:3 /ostree/deploy/x/var /var rw - xfs /dev/sda3 rw\n",
        )
        .unwrap();
        let names: Vec<String> = scan(&t.roots).into_values().map(|f| f.name).collect();
        assert_eq!(names, ["EXTERNAL_USB"]);
        // btrfs: anonymous numbers, its source tells.
        let btrfs = "30 1 0:35 /root / rw - btrfs /dev/sdb rw\n";
        assert_eq!(system_mounts(btrfs), [((0, 35), "/dev/sdb".to_string())]);
    }

    #[test]
    fn a_bluetooth_gamepad_is_its_address() {
        let t = Tree::new();
        let hid = t.roots.sys.join("devices/virtual/misc/uhid/0005:054C:0CE6.0007");
        std::fs::create_dir_all(&hid).unwrap();
        std::fs::write(
            hid.join("uevent"),
            "HID_ID=0005:0000054C:00000CE6\nHID_NAME=DualSense Wireless Controller\nHID_UNIQ=a0:ab:51:12:34:56\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&hid, t.roots.sys.join("bus/hid/devices/0005:054C:0CE6.0007")).unwrap();
        t.node(&hid, "input/input30/event30", "input", "input/event30", (13, 94), &["ID_INPUT_JOYSTICK=1"]);
        t.node(&hid, "input/input31/event31", "input", "input/event31", (13, 95), &["ID_INPUT_TOUCHPAD=1"]);
        let all = scan(&t.roots);
        let f = &all["bt:A0:AB:51:12:34:56"];
        assert_eq!(
            (f.kind, f.bus, f.name.as_str()),
            (DeviceKind::Gamepad, DeviceBus::Bluetooth, "DualSense Wireless Controller")
        );
        assert_eq!(f.handed_nodes().len(), 1, "its touchpad stays a pointer on the machine");
    }
}

/// `cargo test -p wadd --lib this_machines_devices -- --ignored --nocapture`:
/// what this machine has plugged in.
#[cfg(test)]
#[test]
#[ignore = "reads this machine's sysfs"]
fn this_machines_devices() {
    for f in scan(&Roots::default()).values() {
        println!("{} | {} | {:?} {:?}", f.key, f.name, f.bus, f.kind);
        for n in &f.nodes {
            println!("    {} {}:{} {} {:?}", n.devname, n.major, n.minor, n.subsystem, n.role);
        }
    }
}
