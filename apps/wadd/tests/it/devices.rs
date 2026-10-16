//! The device server: devices go to the running wadspaces they remember,
//! while they're plugged in, and come back when either goes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wad_proto::v1::{Device, DeviceKind, Display};
use wadd::devices::Devices;
use wadd::devices::attach::{Hands, NodeOp};
use wadd::devices::sysfs::Roots;
use wadd::devices::uevent;
use wadd::events::Bus;
use wadd::registry::{Registry, Settings};

use crate::common::host::FakeHost;
use crate::common::{self, Fake};
use wadd::drives::Drives;

const PAD: &str = "usb:2dc8:6101:PAD1";
const KB: &str = "usb:04d9:0006@1-6";

/// What the hands did: "give <pid> <nodes>" / "take <pid> <nodes>", and
/// on the machine "bind <src> <dst>", "unbind <dst>[ force]", "off <disk>"
/// (paths under the tempdir as "~").
#[derive(Default, Clone)]
struct Recorder(Arc<Mutex<Vec<String>>>, Arc<Mutex<Option<PathBuf>>>, Arc<Mutex<bool>>);

impl Recorder {
    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
    fn line(&self, what: &str, pid: u32, nodes: &[NodeOp]) {
        let names: Vec<&str> = nodes.iter().map(|n| n.devname.as_str()).collect();
        self.0.lock().unwrap().push(format!("{what} {pid} {}", names.join(" ")));
    }
}

impl Hands for Recorder {
    fn give(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String> {
        for n in nodes {
            let p = uevent::decode(&n.event).unwrap();
            assert_eq!((p["ACTION"].as_str(), p["DEVNAME"].as_str()), ("add", n.devname.as_str()));
        }
        self.line("give", pid, nodes);
        Ok(())
    }
    fn take(&self, pid: u32, nodes: &[NodeOp]) -> Result<(), String> {
        for n in nodes {
            assert_eq!(uevent::decode(&n.event).unwrap()["ACTION"], "remove");
        }
        self.line("take", pid, nodes);
        Ok(())
    }
    fn make_dir(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())
    }
    fn bind(&self, src: &Path, dst: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dst).unwrap();
        let line = format!("bind {} {}", self.short(src), self.short(dst));
        self.0.lock().unwrap().push(line);
        Ok(())
    }
    fn unbind(&self, dst: &Path, force: bool) -> Result<(), String> {
        if *self.2.lock().unwrap() && !force {
            return Err(wadd::devices::attach::BUSY.into());
        }
        let line = format!("unbind {}{}", self.short(dst), if force { " force" } else { "" });
        self.0.lock().unwrap().push(line);
        let _ = std::fs::remove_dir(dst);
        Ok(())
    }
    fn power_off(&self, disk: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("off {disk}"));
        Ok(())
    }
}

impl Recorder {
    /// A path, with the tempdir as "~".
    fn short(&self, p: &Path) -> String {
        let root = self.1.lock().unwrap().clone().unwrap_or_default();
        match p.strip_prefix(&root) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }
    /// Unmounting finds files open (until it's set back).
    fn busy(&self, yes: bool) {
        *self.2.lock().unwrap() = yes;
    }
}

/// A machine's sysfs and udev database, in a tempdir.
struct Tree {
    roots: Roots,
}

impl Tree {
    fn new(dir: &Path) -> Self {
        let roots = Roots { sys: dir.join("sys"), udev_data: dir.join("udev"), mountinfo: dir.join("mountinfo") };
        for p in ["bus/usb/devices", "bus/hid/devices", "class"] {
            std::fs::create_dir_all(roots.sys.join(p)).unwrap();
        }
        std::fs::create_dir_all(&roots.udev_data).unwrap();
        Self { roots }
    }

    fn plug(&self, port: &str, vid: &str, pid: &str, serial: &str, product: &str) -> PathBuf {
        let dir = self.roots.sys.join("devices/pci0000:00/usb1").join(port);
        std::fs::create_dir_all(&dir).unwrap();
        for (k, v) in
            [("idVendor", vid), ("idProduct", pid), ("serial", serial), ("product", product), ("bDeviceClass", "00")]
        {
            if !v.is_empty() {
                std::fs::write(dir.join(k), format!("{v}\n")).unwrap();
            }
        }
        let _ = std::os::unix::fs::symlink(&dir, self.roots.sys.join("bus/usb/devices").join(port));
        dir
    }

    fn unplug(&self, port: &str) {
        let _ = std::fs::remove_file(self.roots.sys.join("bus/usb/devices").join(port));
        let _ = std::fs::remove_dir_all(self.roots.sys.join("devices/pci0000:00/usb1").join(port));
    }

    fn node(&self, under: &Path, rel: &str, subsystem: &str, devname: &str, dev: (u32, u32), udev: &[&str]) {
        let dir = under.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dev"), format!("{}:{}\n", dev.0, dev.1)).unwrap();
        std::fs::write(dir.join("uevent"), format!("MAJOR={}\nMINOR={}\nDEVNAME={devname}\n", dev.0, dev.1)).unwrap();
        let class = self.roots.sys.join("class").join(subsystem);
        std::fs::create_dir_all(&class).unwrap();
        let _ = std::os::unix::fs::symlink(&class, dir.join("subsystem"));
        let db: String = udev.iter().map(|l| format!("E:{l}\n")).collect();
        std::fs::write(self.roots.udev_data.join(format!("c{}:{}", dev.0, dev.1)), db).unwrap();
    }

    /// A gamepad at 1-2, its event node numbered `n`.
    fn gamepad(&self, n: u32) {
        let d = self.plug("1-2", "2dc8", "6101", "PAD1", "Pro 2");
        self.node(&d, "", "usb", &format!("bus/usb/001/{:03}", n), (189, n), &[]);
        self.node(
            &d,
            &format!("1-2:1.0/input/input{n}/event{n}"),
            "input",
            &format!("input/event{n}"),
            (13, 64 + n),
            &["ID_INPUT_JOYSTICK=1"],
        );
    }

    /// A USB drive at 1-4: the disk sdb, with the fixture's sdb1 (STICK, exFAT).
    fn drive(&self) {
        let d = self.plug("1-4", "0480", "0820", "TOSH1", "EXTERNAL_USB");
        self.node(&d, "", "usb", "bus/usb/002/004", (189, 131), &[]);
        let blk = "1-4:1.0/host1/target1:0:0/1:0:0:0/block/sdb";
        self.node(&d, blk, "block", "sdb", (8, 16), &[]);
        self.node(&d, &format!("{blk}/sdb1"), "block", "sdb1", (8, 17), &[]);
    }

    /// The drive after ejecting: its disk gone, the USB device still there.
    fn drive_off(&self) {
        let d = self.roots.sys.join("devices/pci0000:00/usb1/1-4");
        std::fs::remove_dir_all(d.join("1-4:1.0/host1")).unwrap();
    }

    fn keyboard(&self) {
        let d = self.plug("1-6", "04d9", "0006", "", "Keyboard");
        self.node(&d, "1-6:1.0/input/input20/event20", "input", "input/event20", (13, 84), &["ID_INPUT_KEYBOARD=1"]);
    }
}

struct Env {
    _d: tempfile::TempDir,
    tree: Tree,
    fake: Fake,
    reg: Arc<Registry>,
    hands: Recorder,
    devices: Arc<Devices>,
    state: PathBuf,
    bus: Bus,
    host: FakeHost,
}

async fn env(available: bool) -> Env {
    let d = tempfile::tempdir().unwrap();
    let tmp = wad_store::folders::realpath(d.path());
    let tree = Tree::new(&tmp);
    let fake = Fake::default();
    let bus = Bus::new(wad_proto::v1::MachineInfo {
        name: "t".into(),
        version: "0".into(),
        profile: wad_proto::v1::Profile::System,
        started_at: 0,
        screen: None,
    });
    let state = tmp.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let reg = Registry::new(
        Arc::new(fake.clone()),
        bus.clone(),
        Settings {
            ready_timeout: Duration::from_secs(10),
            projects_dir: tmp.join("projects").to_string_lossy().into(),
            signins_dir: tmp.join("signins"),
            state_dir: state.clone(),
            rootless_uid: None,
            device_rules: vec![],
            drives_dir: None,
            poll: Duration::from_millis(10),
        },
    );
    let stream = common::ws("s", "i", Display::Stream, Some(3100));
    reg.load(vec![common::ws("a", "i", Display::Host, None), common::ws("b", "i", Display::Host, None), stream])
        .await
        .unwrap();
    let hands = Recorder::default();
    *hands.1.lock().unwrap() = Some(tmp.clone());
    let host = FakeHost::new(None);
    let drives = Arc::new(Drives::new(1000, Arc::new(host.clone()), true));
    let devices = Devices::new(
        &state,
        tree.roots.clone(),
        Arc::new(hands.clone()),
        drives,
        tmp.join("attached"),
        Arc::new(fake.clone()),
        reg.clone(),
        bus.clone(),
        available,
    );
    Env { _d: d, tree, fake, reg, hands, devices, state, bus, host }
}

impl Env {
    async fn run(&self, ids: &[&str]) {
        self.fake.with(|m| {
            for id in ids {
                m.next_pid += 1;
                m.pids.insert(format!("wad-{id}"), 4000 + m.next_pid);
                m.containers.insert(format!("wad-{id}"), "running".into());
            }
        });
        self.reg.reconcile().await;
    }

    async fn stop(&self, id: &str) {
        self.fake.with(|m| {
            m.containers.remove(&format!("wad-{id}"));
            m.pids.remove(&format!("wad-{id}"));
        });
        self.reg.reconcile().await;
    }

    fn pid(&self, id: &str) -> u32 {
        self.fake.0.lock().unwrap().pids[&format!("wad-{id}")]
    }

    /// What wadd notices: what's plugged in, then what goes where.
    async fn tick(&self) {
        self.devices.rescan();
        self.devices.reconcile().await;
    }

    fn device(&self, key: &str) -> Device {
        self.devices.list().into_iter().find(|d| d.key == key).unwrap()
    }
}

#[tokio::test]
async fn a_new_device_stays_on_the_machine_until_its_asked_about() {
    let e = env(true).await;
    e.run(&["a"]).await;
    e.tree.gamepad(5);
    e.tree.keyboard();
    let (_, mut rx) = e.bus.subscribe();
    e.tick().await;
    let pad = e.device(PAD);
    assert_eq!((pad.kind, pad.attachable, pad.present, pad.new), (DeviceKind::Gamepad, true, true, true));
    assert!(pad.attached.is_empty() && pad.remembered.is_empty());
    let kb = e.device(KB);
    assert_eq!((kb.kind, kb.attachable, kb.new), (DeviceKind::Keyboard, false, false));
    assert!(e.hands.calls().is_empty());
    assert!(matches!(rx.try_recv(), Ok(wad_proto::v1::Event::Devices(list)) if list.len() == 2));
    // "Keep it on the machine": asked, nowhere.
    e.devices.set(PAD, None, Some(vec![])).unwrap();
    assert!(!e.device(PAD).new);
    // Keyboards work everywhere already.
    let err = e.devices.attach(KB, "a").unwrap_err();
    assert!(err.message.contains("every wadspace already"), "{err:?}");
}

#[tokio::test]
async fn a_device_goes_to_the_running_wadspaces_it_remembers_shared() {
    let e = env(true).await;
    e.run(&["a", "b"]).await;
    e.tree.gamepad(5);
    e.tick().await;
    e.devices.attach(PAD, "a").unwrap();
    e.devices.reconcile().await;
    let (a, b) = (e.pid("a"), e.pid("b"));
    assert_eq!(e.hands.calls(), [format!("give {a} /dev/bus/usb/001/005 /dev/input/event5")]);
    // Shared: b too.
    e.devices.attach(PAD, "b").unwrap();
    e.devices.reconcile().await;
    assert_eq!(e.hands.calls(), [format!("give {b} /dev/bus/usb/001/005 /dev/input/event5")]);
    assert_eq!(e.device(PAD).attached, ["a", "b"]);
    // Nothing changed: nothing done.
    e.tick().await;
    assert!(e.hands.calls().is_empty());
    // Remembered on disk.
    let saved = std::fs::read_to_string(e.state.join("devices.json")).unwrap();
    assert!(saved.contains("\"remembered\": [\n        \"a\",\n        \"b\"\n      ]"), "{saved}");
    // Detached from a: taken back, b keeps it.
    e.devices.detach(PAD, "a").unwrap();
    e.devices.reconcile().await;
    assert_eq!(e.hands.calls(), [format!("take {a} /dev/bus/usb/001/005 /dev/input/event5")]);
    assert_eq!(e.device(PAD).remembered, ["b"]);
}

#[tokio::test]
async fn unplugged_and_plugged_in_again_it_goes_back() {
    let e = env(true).await;
    e.run(&["a"]).await;
    e.tree.gamepad(5);
    e.tick().await;
    e.devices.attach(PAD, "a").unwrap();
    e.devices.reconcile().await;
    e.hands.calls();
    let a = e.pid("a");
    e.tree.unplug("1-2");
    e.tick().await;
    assert_eq!(e.hands.calls(), [format!("take {a} /dev/bus/usb/001/005 /dev/input/event5")]);
    let gone = e.device(PAD);
    assert_eq!((gone.present, gone.attached.len(), gone.remembered.len()), (false, 0, 1));
    // Another port, other numbers: the same device, back where it was.
    e.tree.gamepad(9);
    e.tick().await;
    assert_eq!(e.hands.calls(), [format!("give {a} /dev/bus/usb/001/009 /dev/input/event9")]);
}

#[tokio::test]
async fn a_wadspace_that_starts_gets_its_devices_and_one_that_stops_lets_them_go() {
    let e = env(true).await;
    e.tree.gamepad(5);
    e.tick().await;
    e.devices.attach(PAD, "a").unwrap();
    e.devices.reconcile().await;
    assert!(e.hands.calls().is_empty(), "a isn't running");
    e.run(&["a"]).await;
    e.devices.reconcile().await;
    let first = e.pid("a");
    assert_eq!(e.hands.calls(), [format!("give {first} /dev/bus/usb/001/005 /dev/input/event5")]);
    // Stopped: its nodes went with it; nothing to take.
    e.stop("a").await;
    e.devices.reconcile().await;
    assert!(e.hands.calls().is_empty());
    assert!(e.device(PAD).attached.is_empty());
    // Started again: handed again.
    e.run(&["a"]).await;
    e.devices.reconcile().await;
    assert_eq!(e.hands.calls(), [format!("give {} /dev/bus/usb/001/005 /dev/input/event5", e.pid("a"))]);
}

#[tokio::test]
async fn a_part_that_appears_later_is_handed_on_its_own() {
    let e = env(true).await;
    e.run(&["a"]).await;
    let d = e.tree.plug("1-2", "2dc8", "6101", "PAD1", "Pro 2");
    e.tree.node(&d, "", "usb", "bus/usb/001/005", (189, 5), &[]);
    e.tick().await;
    e.devices.attach(PAD, "a").unwrap();
    e.devices.reconcile().await;
    assert_eq!(e.hands.calls(), [format!("give {} /dev/bus/usb/001/005", e.pid("a"))]);
    // Its pad's node, once udev has it.
    e.tree.node(&d, "1-2:1.0/input/input5/event5", "input", "input/event5", (13, 69), &["ID_INPUT_JOYSTICK=1"]);
    e.tick().await;
    assert_eq!(e.hands.calls(), [format!("give {} /dev/input/event5", e.pid("a"))]);
}

#[tokio::test]
async fn devices_are_refused_where_they_cant_go() {
    let e = env(true).await;
    e.tree.gamepad(5);
    e.tick().await;
    let err = e.devices.attach(PAD, "s").unwrap_err();
    assert!(err.message.contains("can't have devices"), "a streamed one: {err:?}");
    assert!(e.devices.attach(PAD, "nope").is_err());
    assert!(e.devices.attach("usb:../x", "a").is_err());
    assert!(e.devices.set(PAD, Some(Some("\n".into())), None).is_err());
    // A wadspace that's deleted is forgotten.
    e.devices.attach(PAD, "b").unwrap();
    let list: Vec<_> = e.reg.workspaces().into_iter().filter(|w| w.id != "b").collect();
    e.reg.load(list).await.unwrap();
    e.devices.reconcile().await;
    assert!(e.device(PAD).remembered.is_empty());
    // Renamed, then its own name again.
    assert_eq!(e.devices.set(PAD, Some(Some(" Player 1 ".into())), None).unwrap().name, "Player 1");
    assert_eq!(e.devices.set(PAD, Some(None), None).unwrap().name, "Pro 2");
    // Forgotten only once it's unplugged.
    assert!(e.devices.forget(PAD).is_err());
    e.tree.unplug("1-2");
    e.tick().await;
    e.devices.forget(PAD).unwrap();
    assert!(e.devices.list().is_empty());
}

#[tokio::test]
async fn without_root_nothing_is_handed_out() {
    let e = env(false).await;
    e.run(&["a"]).await;
    e.tree.gamepad(5);
    e.tick().await;
    let err = e.devices.attach(PAD, "a").unwrap_err();
    assert!(err.message.contains("isn't running as root"), "{err:?}");
    assert!(e.devices.list()[0].present, "still listed");
}

#[tokio::test]
async fn paired_bluetooth_devices_are_listed_and_a_pad_is_handed_out() {
    use wad_proto::v1::{BluetoothDevice, DeviceBus};
    let e = env(true).await;
    e.run(&["a"]).await;
    let bt = |address: &str, name: &str, kind, connected| BluetoothDevice {
        address: address.into(),
        name: name.into(),
        kind,
        paired: true,
        connected,
        rssi: None,
        busy: false,
    };
    let buds = "bt:11:22:33:44:55:66";
    e.devices.set_bluetooth(&[
        bt("11:22:33:44:55:66", "Buds", DeviceKind::Audio, true),
        bt("A0:AB:51:12:34:56", "DualSense", DeviceKind::Gamepad, false),
        BluetoothDevice { paired: false, ..bt("77:77:77:77:77:77", "Nearby", DeviceKind::Other, false) },
    ]);
    let list = e.devices.list();
    let keys: Vec<&str> = list.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, [buds, "bt:A0:AB:51:12:34:56"], "paired only; present first");
    let b = e.device(buds);
    assert_eq!((b.bus, b.present, b.attachable), (DeviceBus::Bluetooth, true, false));
    assert!(e.devices.attach(buds, "a").is_err(), "sound goes through the machine");
    assert_eq!(e.devices.set(buds, Some(Some("My buds".into())), None).unwrap().name, "My buds");
    // The pad: remembered while it's off, handed out once it connects (its HID nodes).
    e.devices.attach("bt:A0:AB:51:12:34:56", "a").unwrap();
    let hid = e.tree.roots.sys.join("devices/virtual/misc/uhid/0005:054C:0CE6.0007");
    std::fs::create_dir_all(&hid).unwrap();
    std::fs::write(
        hid.join("uevent"),
        "HID_ID=0005:0000054C:00000CE6\nHID_NAME=DualSense\nHID_UNIQ=a0:ab:51:12:34:56\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&hid, e.tree.roots.sys.join("bus/hid/devices/0005:054C:0CE6.0007")).unwrap();
    e.tree.node(&hid, "input/input30/event30", "input", "input/event30", (13, 94), &["ID_INPUT_JOYSTICK=1"]);
    e.tick().await;
    assert_eq!(e.hands.calls(), [format!("give {} /dev/input/event30", e.pid("a"))]);
    assert!(e.device("bt:A0:AB:51:12:34:56").present);
}

const DRIVE: &str = "usb:0480:0820:TOSH1";

#[tokio::test]
async fn a_drive_goes_into_its_wadspaces_drives_running_or_not() {
    let e = env(true).await;
    e.run(&["a"]).await;
    e.tree.drive();
    e.tick().await;
    let d = e.device(DRIVE);
    assert_eq!((d.kind, d.attachable, d.new), (DeviceKind::Storage, true, true), "asked about, like any device");
    // a runs, b doesn't: both get it, at ~/Drives/<its label>.
    e.devices.set(DRIVE, None, Some(vec!["a".into(), "b".into()])).unwrap();
    e.devices.reconcile().await;
    assert_eq!(
        e.hands.calls(),
        [
            "bind /run/wadspaces-drives/5E3F-1A2B ~/attached/a/STICK",
            "bind /run/wadspaces-drives/5E3F-1A2B ~/attached/b/STICK",
        ],
        "mounted on the machine once, no USB node handed out"
    );
    assert!(e.host.calls().iter().any(|c| c[0] == "systemd-mount"));
    assert_eq!(e.device(DRIVE).attached, ["a", "b"]);
    // Out of b only.
    e.devices.detach(DRIVE, "b").unwrap();
    e.devices.reconcile().await;
    assert_eq!(e.hands.calls(), ["unbind ~/attached/b/STICK"]);
}

#[tokio::test]
async fn ejecting_takes_a_drive_out_unmounts_it_and_turns_it_off() {
    let e = env(true).await;
    e.tree.drive();
    e.tick().await;
    e.devices.attach(DRIVE, "a").unwrap();
    e.devices.reconcile().await;
    e.hands.calls();
    // Something in a has files open: refused, and it stays.
    e.hands.busy(true);
    let err = e.devices.eject(DRIVE).await.unwrap_err();
    assert!(err.message.contains("in use in A"), "{err:?}");
    assert!(!e.device(DRIVE).ejected);
    e.devices.reconcile().await;
    assert!(e.hands.calls().is_empty(), "still bound, nothing redone");
    e.hands.busy(false);
    let d = e.devices.eject(DRIVE).await.unwrap();
    assert!(d.ejected && d.attached.is_empty());
    assert_eq!(e.hands.calls(), ["unbind ~/attached/a/STICK", "unbind /run/wadspaces-drives/5E3F-1A2B", "off sdb"]);
    // The disk's gone; the USB device stays until it's pulled: still a
    // drive (safe to unplug), never handed out as raw USB.
    e.tree.drive_off();
    e.tick().await;
    let d = e.device(DRIVE);
    assert_eq!((d.kind, d.ejected, d.present), (DeviceKind::Storage, true, true));
    assert!(e.hands.calls().is_empty());
    // Unplugged, then plugged in again: back in a.
    e.tree.unplug("1-4");
    e.tick().await;
    assert!(!e.device(DRIVE).ejected);
    e.tree.drive();
    e.tick().await;
    assert_eq!(e.hands.calls(), ["bind /run/wadspaces-drives/5E3F-1A2B ~/attached/a/STICK"]);
}

#[tokio::test]
async fn a_drive_pulled_out_without_ejecting_is_let_go() {
    let e = env(true).await;
    e.tree.drive();
    e.tick().await;
    e.devices.attach(DRIVE, "a").unwrap();
    e.devices.reconcile().await;
    e.hands.calls();
    e.tree.unplug("1-4");
    e.tick().await;
    assert_eq!(e.hands.calls(), ["unbind ~/attached/a/STICK force"]);
    assert!(e.device(DRIVE).attached.is_empty());
}
