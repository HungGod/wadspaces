//! Devices plugged into the machine, handed to the wadspaces that should
//! have them while they run: wadd is the device server.
//!
//! A per-device volume can't work: a unit's devices are fixed when the
//! container starts, and a device plugged in later (or into another port)
//! is a new node. So every native wadspace is started allowed to *open*
//! nodes of the kinds wadd hands out ([`cgroup_rules`]: USB, input,
//! hidraw, webcams, serial), but never to make one; wadd makes the nodes of
//! the devices it attaches, inside the running container, and takes them
//! away again.
//!
//! Keyboards, mice, sound and drives aren't handed out: they already work
//! in every wadspace through the machine (sway, PipeWire, copies and
//! mounts).

pub mod sysfs;
pub mod uevent;

/// Character device majors wadd hands out that never change: input
/// (evdev, joysticks), video4linux (webcams), ttyACM and ttyUSB (serial),
/// usb_device (raw USB, /dev/bus/usb).
const FIXED_MAJORS: [u32; 5] = [13, 81, 166, 188, 189];
/// Majors the kernel picks at boot, by their name in /proc/devices.
const DYNAMIC: [&str; 1] = ["hidraw"];

/// A container's device cgroup rules for the nodes wadd may hand it, from
/// /proc/devices: `c <major>:* rw` (open, never make: no "m").
pub fn cgroup_rules(proc_devices: &str) -> Vec<String> {
    let mut majors: Vec<u32> = FIXED_MAJORS.to_vec();
    let mut chars = false;
    for line in proc_devices.lines() {
        let line = line.trim();
        if line.ends_with(':') {
            chars = line == "Character devices:";
            continue;
        }
        let Some((major, name)) = line.split_once(char::is_whitespace) else { continue };
        if chars
            && DYNAMIC.contains(&name.trim())
            && let Ok(m) = major.parse()
        {
            majors.push(m);
        }
    }
    majors.sort_unstable();
    majors.dedup();
    majors.iter().map(|m| format!("c {m}:* rw")).collect()
}

/// [`cgroup_rules`] for this machine.
pub fn machine_cgroup_rules() -> Vec<String> {
    cgroup_rules(&std::fs::read_to_string("/proc/devices").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_open_the_kinds_wadd_hands_out_and_hidraw_wherever_it_is() {
        let proc = "Character devices:\n  1 mem\n 10 misc\n 13 input\n189 usb_device\n241 hidraw\n\nBlock devices:\n242 hidraw\n";
        assert_eq!(
            cgroup_rules(proc),
            ["c 13:* rw", "c 81:* rw", "c 166:* rw", "c 188:* rw", "c 189:* rw", "c 241:* rw"]
        );
        assert_eq!(cgroup_rules("").len(), 5, "the fixed ones without /proc/devices");
    }
}
