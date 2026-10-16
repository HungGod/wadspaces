//! udev's events, both ways: wadd listens to the machine's (netlink group
//! 2, sent once udevd has written a device's database entry) to see devices
//! come and go, and sends its own in a wadspace's network namespace, in
//! libudev's format, so the apps in there (Chrome's gamepads, WebUSB,
//! WebHID) see a device it hands over come and go as if udevd had sent it.
//!
//! The format (systemd's sd-device monitor): a 40-byte header ("libudev\0",
//! magic 0xfeedcafe big-endian, header size, properties offset and length,
//! then big-endian MurmurHash2 hashes of the subsystem and devtype and a
//! 64-bit bloom filter of the tags, which receivers' socket filters match
//! on), then `KEY=value\0` properties. Receivers take it only from uid 0.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use super::sysfs::{Node, Roots, uevent};

/// NETLINK_KOBJECT_UEVENT's udev group (the kernel's own is 1).
const UDEV_GROUP: u32 = 2;
const MAGIC: u32 = 0xfeed_cafe;
const HEADER: usize = 40;

/// MurmurHash2 (32-bit, seed 0): systemd's string_hash32.
pub fn murmur2(data: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    let mut h = data.len() as u32;
    let (words, tail) = data.as_chunks::<4>();
    for w in words {
        let mut k = u32::from_le_bytes(*w);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    if tail.len() == 3 {
        h ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        h ^= u32::from(tail[1]) << 8;
    }
    if !tail.is_empty() {
        h ^= u32::from(tail[0]);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

/// The tags' bloom filter: four bits per tag, from its hash.
pub fn bloom<'a>(tags: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut bits = 0u64;
    for t in tags {
        let h = murmur2(t.as_bytes());
        for shift in [0, 6, 12, 18] {
            bits |= 1 << ((h >> shift) & 63);
        }
    }
    bits
}

/// A udev event in libudev's format.
pub fn message(props: &[(String, String)]) -> Vec<u8> {
    let get = |k: &str| props.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    let mut body = Vec::new();
    for (k, v) in props {
        body.extend_from_slice(k.as_bytes());
        body.push(b'=');
        body.extend_from_slice(v.as_bytes());
        body.push(0);
    }
    let tags = get("TAGS").unwrap_or("").split(':').filter(|t| !t.is_empty());
    let b = bloom(tags);
    let mut m = Vec::with_capacity(HEADER + body.len());
    m.extend_from_slice(b"libudev\0");
    m.extend_from_slice(&MAGIC.to_be_bytes());
    m.extend_from_slice(&(HEADER as u32).to_ne_bytes());
    m.extend_from_slice(&(HEADER as u32).to_ne_bytes());
    m.extend_from_slice(&(body.len() as u32).to_ne_bytes());
    m.extend_from_slice(&murmur2(get("SUBSYSTEM").unwrap_or("").as_bytes()).to_be_bytes());
    m.extend_from_slice(&get("DEVTYPE").map_or(0, |d| murmur2(d.as_bytes())).to_be_bytes());
    m.extend_from_slice(&((b >> 32) as u32).to_be_bytes());
    m.extend_from_slice(&(b as u32).to_be_bytes());
    m.extend_from_slice(&body);
    m
}

/// A udev event's properties (None: not one of libudev's).
pub fn decode(buf: &[u8]) -> Option<BTreeMap<String, String>> {
    if buf.len() < HEADER || &buf[..8] != b"libudev\0" || buf[8..12] != MAGIC.to_be_bytes() {
        return None;
    }
    let word = |at: usize| u32::from_ne_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize;
    let (off, len) = (word(16), word(20));
    let props = buf.get(off..off.checked_add(len)?)?;
    Some(
        props
            .split(|b| *b == 0)
            .filter_map(|p| std::str::from_utf8(p).ok()?.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

/// What udevd would say about a node: its uevent, then its database entry
/// (properties, tags, when it was set up).
pub fn props_for(roots: &Roots, node: &Node, action: &str, seqnum: u64) -> Vec<(String, String)> {
    let devpath = node.syspath.strip_prefix(&roots.sys).map(|p| format!("/{}", p.display())).unwrap_or_default();
    let mut props = vec![
        ("ACTION".to_string(), action.to_string()),
        ("DEVPATH".into(), devpath),
        ("SUBSYSTEM".into(), node.subsystem.clone()),
        ("MAJOR".into(), node.major.to_string()),
        ("MINOR".into(), node.minor.to_string()),
        ("DEVNAME".into(), node.devname.clone()),
    ];
    if let Some(t) = uevent(&node.syspath).get("DEVTYPE") {
        props.push(("DEVTYPE".into(), t.clone()));
    }
    props.push(("SEQNUM".into(), seqnum.to_string()));
    let db =
        std::fs::read_to_string(roots.udev_data.join(format!("c{}:{}", node.major, node.minor))).unwrap_or_default();
    let mut tags = vec![];
    for line in db.lines() {
        if let Some((k, v)) = line.strip_prefix("E:").and_then(|l| l.split_once('=')) {
            props.push((k.into(), v.into()));
        } else if let Some(t) = line.strip_prefix("G:") {
            tags.push(t);
        } else if let Some(at) = line.strip_prefix("I:") {
            props.push(("USEC_INITIALIZED".into(), at.into()));
        }
    }
    if !tags.is_empty() {
        let t = format!(":{}:", tags.join(":"));
        props.push(("TAGS".into(), t.clone()));
        props.push(("CURRENT_TAGS".into(), t));
    }
    props
}

fn netlink_socket(groups: u32) -> std::io::Result<OwnedFd> {
    // SAFETY: plain socket(2)/bind(2) calls; the fd is owned right away.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = groups;
        let r = libc::bind(
            fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        );
        if r < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd)
    }
}

/// A socket in a process's network namespace (a container's), to send udev
/// events to the apps in there. Made on a thread of its own that joins the
/// namespace (setns is per thread), so wadd's own threads stay where they
/// are; the socket keeps the namespace it was made in.
pub fn socket_in(pid: u32) -> std::io::Result<OwnedFd> {
    let ns = std::fs::File::open(format!("/proc/{pid}/ns/net"))?;
    std::thread::spawn(move || {
        // SAFETY: setns on this short-lived thread only.
        if unsafe { libc::setns(ns.as_raw_fd(), libc::CLONE_NEWNET) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        netlink_socket(0)
    })
    .join()
    .map_err(|_| std::io::Error::other("the namespace thread panicked"))?
}

/// Sends a udev event on `sock` (from socket_in) to whoever listens there.
pub fn send(sock: &OwnedFd, msg: &[u8]) -> std::io::Result<()> {
    // SAFETY: sendto with a zeroed netlink address naming the udev group.
    unsafe {
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as u16;
        addr.nl_groups = UDEV_GROUP;
        let r = libc::sendto(
            sock.as_raw_fd(),
            msg.as_ptr() as *const libc::c_void,
            msg.len(),
            0,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        );
        if r < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }
}

/// The machine's udev events, as they come: (action, subsystem). Ends
/// when the socket can't be read.
pub async fn listen(tx: tokio::sync::mpsc::UnboundedSender<(String, String)>) -> std::io::Result<()> {
    let fd = tokio::io::unix::AsyncFd::new(netlink_socket(UDEV_GROUP)?)?;
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let mut ready = fd.readable().await?;
        // SAFETY: recv into our buffer.
        let n = unsafe { libc::recv(fd.get_ref().as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                ready.clear_ready();
                continue;
            }
            // ENOBUFS: events were lost; a rescan catches up.
            if e.raw_os_error() == Some(libc::ENOBUFS) {
                let _ = tx.send(("change".into(), String::new()));
                continue;
            }
            return Err(e);
        }
        if let Some(p) = decode(&buf[..n as usize]) {
            let get = |k: &str| p.get(k).cloned().unwrap_or_default();
            if tx.send((get("ACTION"), get("SUBSYSTEM"))).is_err() {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_blooms_match_systemds() {
        // From devices-helper.py, whose messages the machine's libudev takes.
        for (s, h) in [
            ("input", 0xc1a28470),
            ("hidraw", 0xc2caf397),
            ("usb", 0x0577c5e5),
            ("usb_device", 0x27f8f50c),
            ("video4linux", 0xe9cf3931),
            ("tty", 0x8afa90c8),
            ("abc", 0x13577c9b),
        ] {
            assert_eq!(murmur2(s.as_bytes()), h, "{s}");
        }
        assert_eq!(bloom(["seat", "uaccess"]), 0x0208_2008_0040_1009);
    }

    #[test]
    fn messages_are_libudevs_byte_for_byte() {
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        let props = vec![
            p("ACTION", "add"),
            p("DEVPATH", "/devices/virtual/input/input9/event9"),
            p("SUBSYSTEM", "input"),
            p("DEVNAME", "/dev/input/event9"),
            p("SEQNUM", "7"),
            p("TAGS", ":seat:uaccess:"),
        ];
        let want = "6c69627564657600feedcafe28000000280000007f000000c1a28470000000000208200800401009\
                    414354494f4e3d61646400444556504154483d2f646576696365732f7669727475616c2f696e707574\
                    2f696e707574392f6576656e74390053554253595354454d3d696e707574004445564e414d453d2f64\
                    65762f696e7075742f6576656e7439005345514e554d3d3700544147533d3a736561743a7561636365\
                    73733a00";
        let got: String = message(&props).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
        let back = decode(&message(&props)).unwrap();
        assert_eq!((back["ACTION"].as_str(), back["SUBSYSTEM"].as_str()), ("add", "input"));
        assert!(decode(b"add@/devices/x\0ACTION=add\0").is_none(), "the kernel's own aren't");
    }

    #[test]
    fn a_nodes_event_carries_its_udev_database() {
        let d = tempfile::tempdir().unwrap();
        let roots = Roots {
            sys: d.path().join("sys"),
            udev_data: d.path().join("udev"),
            mountinfo: d.path().join("mountinfo"),
        };
        let dir = roots.sys.join("devices/virtual/input/input9/event9");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("uevent"), "MAJOR=13\nMINOR=73\nDEVNAME=input/event9\n").unwrap();
        std::fs::create_dir_all(&roots.udev_data).unwrap();
        std::fs::write(
            roots.udev_data.join("c13:73"),
            "I:123\nE:ID_INPUT=1\nE:ID_INPUT_JOYSTICK=1\nG:seat\nG:uaccess\n",
        )
        .unwrap();
        let node = Node {
            devname: "/dev/input/event9".into(),
            major: 13,
            minor: 73,
            subsystem: "input".into(),
            syspath: dir,
            role: super::super::sysfs::Role::Joystick,
        };
        let props = props_for(&roots, &node, "add", 7);
        let keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "ACTION",
                "DEVPATH",
                "SUBSYSTEM",
                "MAJOR",
                "MINOR",
                "DEVNAME",
                "SEQNUM",
                "USEC_INITIALIZED",
                "ID_INPUT",
                "ID_INPUT_JOYSTICK",
                "TAGS",
                "CURRENT_TAGS"
            ]
        );
        assert_eq!(props[1].1, "/devices/virtual/input/input9/event9");
        assert_eq!(props[10].1, ":seat:uaccess:");
    }
}

/// Sends "add" then "remove" for WADD_TRY_NODE (a sysfs folder with a dev)
/// in this process's network namespace, as wadd sends them into a
/// wadspace's: try-devices.sh runs it in an unprivileged user+network
/// namespace with a libudev listener beside it.
#[cfg(test)]
#[test]
#[ignore = "sends udev events; needs a namespace of its own"]
fn sends_to_libudev() {
    let roots = Roots::default();
    let dir = std::path::PathBuf::from(std::env::var("WADD_TRY_NODE").expect("WADD_TRY_NODE"));
    let ev = uevent(&dir);
    let node = Node {
        devname: format!("/dev/{}", ev["DEVNAME"]),
        major: ev["MAJOR"].parse().unwrap(),
        minor: ev["MINOR"].parse().unwrap(),
        subsystem: std::fs::read_link(dir.join("subsystem")).unwrap().file_name().unwrap().to_string_lossy().into(),
        syspath: dir.clone(),
        role: super::sysfs::Role::Other,
    };
    let sock = socket_in(std::process::id()).unwrap();
    for (i, action) in ["add", "remove"].into_iter().enumerate() {
        send(&sock, &message(&props_for(&roots, &node, action, i as u64 + 1))).unwrap();
    }
}
