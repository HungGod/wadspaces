#!/usr/bin/python3
"""Helpers for try-devices.sh (the device server's trial): the steps wadd
takes to hand a device to a running wadspace, by hand.

    devices-helper.py gamepad                 a virtual gamepad (uinput) until killed;
                                              prints its event node's name first
    devices-helper.py send PID ACTION SYSPATH  a udev "add"/"remove" for a device, in
                                              PID's network namespace (as udevd sends it)
    devices-helper.py monitor SUBSYSTEM SECS   inside a container: what libudev lists, and
                                              the udev events it hears for SECS seconds

Run `gamepad` and `send` as root on the machine; `monitor` inside the
container as abc.
"""
import ctypes
import fcntl
import os
import socket
import struct
import sys
import time

# ------------------------------------------------------------------ gamepad
UI_SET_EVBIT = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_SET_ABSBIT = 0x40045567
UI_DEV_CREATE = 0x5501
UI_DEV_DESTROY = 0x5502
UI_DEV_SETUP = 0x405C5503
UI_ABS_SETUP = 0x401C5504
EV_KEY, EV_ABS = 0x01, 0x03
BTN_SOUTH, BTN_EAST = 0x130, 0x131
ABS_X, ABS_Y = 0x00, 0x01
NAME = b"WadSpaces trial gamepad"


def gamepad() -> int:
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    for b in (BTN_SOUTH, BTN_EAST):
        fcntl.ioctl(fd, UI_SET_KEYBIT, b)
    for a in (ABS_X, ABS_Y):
        fcntl.ioctl(fd, UI_SET_ABSBIT, a)
        # struct uinput_abs_setup { __u16 code; struct input_absinfo { value, min, max, fuzz, flat, res } }
        fcntl.ioctl(fd, UI_ABS_SETUP, struct.pack("HxxiiiiiI", a, 0, -32768, 32767, 16, 128, 0))
    # struct uinput_setup { struct input_id { bustype, vendor, product, version }; char name[80]; __u32 ff_effects_max }
    fcntl.ioctl(fd, UI_DEV_SETUP, struct.pack("HHHH80sI", 0x03, 0x1209, 0x0001, 1, NAME, 0))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    # Its event node: the newest input device with our name.
    node = None
    for _ in range(50):
        for d in os.listdir("/sys/class/input"):
            p = f"/sys/class/input/{d}/device/name"
            if d.startswith("event") and os.path.exists(p) and open(p, "rb").read().strip() == NAME:
                node = d
        if node:
            break
        time.sleep(0.1)
    print(node or "none", flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass
    finally:
        fcntl.ioctl(fd, UI_DEV_DESTROY)
    return 0


# --------------------------------------------------------------------- send
def murmur2(data: bytes, seed: int = 0) -> int:
    """MurmurHash2 (32 bit), as systemd's string_hash32 takes it."""
    m, r = 0x5BD1E995, 24
    h = (seed ^ len(data)) & 0xFFFFFFFF
    n = len(data) // 4
    for i in range(n):
        k = struct.unpack_from("<I", data, i * 4)[0]
        k = (k * m) & 0xFFFFFFFF
        k ^= k >> r
        k = (k * m) & 0xFFFFFFFF
        h = (h * m) & 0xFFFFFFFF
        h ^= k
    tail = data[n * 4:]
    if len(tail) == 3:
        h ^= tail[2] << 16
    if len(tail) >= 2:
        h ^= tail[1] << 8
    if len(tail) >= 1:
        h ^= tail[0]
        h = (h * m) & 0xFFFFFFFF
    h ^= h >> 13
    h = (h * m) & 0xFFFFFFFF
    h ^= h >> 15
    return h


def bloom(tags: list[str]) -> int:
    bits = 0
    for t in tags:
        h = murmur2(t.encode())
        for shift in (0, 6, 12, 18):
            bits |= 1 << ((h >> shift) & 63)
    return bits


def udev_props(syspath: str, action: str) -> list[tuple[str, str]]:
    """What udevd says about a device: the kernel's uevent, and its udev
    database entry (/run/udev/data/c<major>:<minor>)."""
    uevent = dict(l.split("=", 1) for l in open(f"{syspath}/uevent").read().splitlines() if "=" in l)
    subsystem = os.path.basename(os.readlink(f"{syspath}/subsystem"))
    props = [("ACTION", action), ("DEVPATH", syspath.removeprefix("/sys")), ("SUBSYSTEM", subsystem)]
    for k in ("MAJOR", "MINOR", "DEVNAME", "DEVTYPE"):
        if k in uevent:
            v = uevent[k]
            props.append((k, "/dev/" + v if k == "DEVNAME" else v))
    props.append(("SEQNUM", str(int(time.time() * 1000) % 10**9)))
    tags: list[str] = []
    db = f"/run/udev/data/c{uevent.get('MAJOR')}:{uevent.get('MINOR')}"
    try:
        for line in open(db).read().splitlines():
            if line.startswith("E:"):
                props.append(tuple(line[2:].split("=", 1)))
            elif line.startswith("G:"):
                tags.append(line[2:])
            elif line.startswith("I:"):
                props.append(("USEC_INITIALIZED", line[2:]))
    except FileNotFoundError:
        pass
    if tags:
        props += [("TAGS", ":" + ":".join(tags) + ":"), ("CURRENT_TAGS", ":" + ":".join(tags) + ":")]
    return props


def message(props: list[tuple[str, str]]) -> bytes:
    p = dict(props)
    body = b"".join(f"{k}={v}".encode() + b"\0" for k, v in props)
    tags = [t for t in p.get("TAGS", "").split(":") if t]
    b = bloom(tags)
    hdr = struct.pack(
        "=8sIIIIIIII",
        b"libudev\0",
        socket.htonl(0xFEEDCAFE),
        40,
        40,
        len(body),
        socket.htonl(murmur2(p["SUBSYSTEM"].encode())),
        socket.htonl(murmur2(p["DEVTYPE"].encode()) if "DEVTYPE" in p else 0),
        socket.htonl(b >> 32),
        socket.htonl(b & 0xFFFFFFFF),
    )
    return hdr + body


def send(pid: str, action: str, syspath: str) -> int:
    msg = message(udev_props(syspath, action))
    with open(f"/proc/{pid}/ns/net") as ns:
        os.setns(ns.fileno(), os.CLONE_NEWNET)
    s = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 15)  # NETLINK_KOBJECT_UEVENT
    s.bind((0, 0))
    s.sendto(msg, (0, 2))  # group 2: udev
    print(f"sent {action} {syspath} ({len(msg)} bytes)")
    return 0


# ------------------------------------------------------------------ monitor
def monitor(subsystem: str, secs: str) -> int:
    lib = ctypes.CDLL("libudev.so.1")
    vp, cp = ctypes.c_void_p, ctypes.c_char_p
    for name, res, args in [
        ("udev_new", vp, []),
        ("udev_enumerate_new", vp, [vp]),
        ("udev_enumerate_add_match_subsystem", ctypes.c_int, [vp, cp]),
        ("udev_enumerate_scan_devices", ctypes.c_int, [vp]),
        ("udev_enumerate_get_list_entry", vp, [vp]),
        ("udev_list_entry_get_next", vp, [vp]),
        ("udev_list_entry_get_name", cp, [vp]),
        ("udev_device_new_from_syspath", vp, [vp, cp]),
        ("udev_device_get_devnode", cp, [vp]),
        ("udev_device_get_property_value", cp, [vp, cp]),
        ("udev_device_get_action", cp, [vp]),
        ("udev_device_get_syspath", cp, [vp]),
        ("udev_monitor_new_from_netlink", vp, [vp, cp]),
        ("udev_monitor_filter_add_match_subsystem_devtype", ctypes.c_int, [vp, cp, cp]),
        ("udev_monitor_enable_receiving", ctypes.c_int, [vp]),
        ("udev_monitor_get_fd", ctypes.c_int, [vp]),
        ("udev_monitor_receive_device", vp, [vp]),
    ]:
        getattr(lib, name).restype = res
        getattr(lib, name).argtypes = args
    udev = lib.udev_new()
    sub = subsystem.encode()
    mon = lib.udev_monitor_new_from_netlink(udev, b"udev")
    lib.udev_monitor_filter_add_match_subsystem_devtype(mon, sub, None)
    lib.udev_monitor_enable_receiving(mon)
    print("ready", flush=True)
    en = lib.udev_enumerate_new(udev)
    lib.udev_enumerate_add_match_subsystem(en, sub)
    lib.udev_enumerate_scan_devices(en)
    e = lib.udev_enumerate_get_list_entry(en)
    while e:
        d = lib.udev_device_new_from_syspath(udev, lib.udev_list_entry_get_name(e))
        node = lib.udev_device_get_devnode(d)
        if node and os.path.exists(node.decode()):
            joy = lib.udev_device_get_property_value(d, b"ID_INPUT_JOYSTICK")
            print(f"listed {node.decode()} joystick={joy.decode() if joy else '-'}", flush=True)
        e = lib.udev_list_entry_get_next(e)
    import select
    fd = lib.udev_monitor_get_fd(mon)
    end = time.time() + float(secs)
    while (left := end - time.time()) > 0:
        if select.select([fd], [], [], left)[0]:
            d = lib.udev_monitor_receive_device(mon)
            if d:
                joy = lib.udev_device_get_property_value(d, b"ID_INPUT_JOYSTICK")
                node = lib.udev_device_get_devnode(d)
                print(
                    f"heard {lib.udev_device_get_action(d).decode()} {node.decode() if node else '-'} "
                    f"joystick={joy.decode() if joy else '-'}",
                    flush=True,
                )
    return 0


if __name__ == "__main__":
    cmd, args = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
    if cmd == "gamepad" and not args:
        sys.exit(gamepad())
    if cmd == "send" and len(args) == 3:
        sys.exit(send(*args))
    if cmd == "monitor" and len(args) == 2:
        sys.exit(monitor(*args))
    print(__doc__, file=sys.stderr)
    sys.exit(2)
