#!/usr/bin/env bash
# Tries the device server's way of handing a device to a running wadspace,
# for real, with rootful podman (as wadd runs on a machine), before wadd
# does it itself:
#   - the container is started allowed to open nodes of the kinds wadd
#     hands out (device cgroup rules "c <major>:* rw"), never to make one;
#   - a virtual gamepad (uinput) is plugged in; its node is made inside the
#     running container from the host (/proc/<pid>/root), with its udev
#     database entry, and a udev "add" is sent in the container's network
#     namespace: libudev in there lists it as a joystick and hears it come
#     and go, as Chrome would;
#   - raw USB (/dev/bus/usb/001/001, a root hub) opens the same way;
#   - inside, root can't make a node, and a node of a kind not handed out
#     (misc, 10:200) can't be opened.
#
#   sudo apps/wadd/dev/try-devices.sh
#
# Its image is the base (localhost/wadspaces-base:trixie, copied from your
# podman into root's when root hasn't it, and removed again afterwards).
set -euo pipefail
ulimit -c 0
[ "$(id -u)" = 0 ] || { echo "run it as root: sudo $0" >&2; exit 1; }
root=$(cd "$(dirname "$0")/../../.." && pwd)
helper=$root/apps/wadd/dev/devices-helper.py
image=localhost/wadspaces-base:trixie
name=wadtry-devices
tmp=$(mktemp -d)
loaded=0
pad=
finish() {
  [ -n "$pad" ] && kill "$pad" 2>/dev/null && wait "$pad" 2>/dev/null || true
  podman rm -f -t 0 "$name" >/dev/null 2>&1 || true
  [ "$loaded" = 1 ] && podman rmi -i "$image" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap finish EXIT
failed=0
ok() { echo "  ✓ $*"; }
bad() { echo "  ✗ $*"; failed=1; }
check() { # what; command...
  local what=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$what"; else bad "$what"; fi
}
refuse() { # what; command... (must fail)
  local what=$1; shift
  if "$@" >/dev/null 2>&1; then bad "$what"; else ok "$what"; fi
}

echo "== the base image, in root's podman"
if ! podman image exists "$image"; then
  [ -n "${SUDO_USER:-}" ] || { echo "no $image here, and no SUDO_USER to copy it from" >&2; exit 1; }
  sudo -u "$SUDO_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$SUDO_USER")" podman save "$image" | podman load -q >/dev/null
  loaded=1
fi
rules=()
for m in 13 81 166 188 189 $(awk '/^Character/{c=1} /^Block/{c=0} c && $2=="hidraw"{print $1}' /proc/devices); do
  rules+=("--device-cgroup-rule=c $m:* rw")
done
echo "  rules: ${rules[*]}"

echo "== a running wadspace (as native ones run: no SELinux separation)"
podman run -d --rm --name "$name" --security-opt label=disable --entrypoint sleep "${rules[@]}" "$image" infinity >/dev/null
pid=$(podman inspect -f '{{.State.Pid}}' "$name")
cr=/proc/$pid/root
podman cp "$helper" "$name:/tmp/devices-helper.py"
inside() { podman exec -u 1000 "$name" "$@"; }
check "libudev is in the image" inside sh -c 'ls /usr/lib/*/libudev.so.1'

echo "== a gamepad plugged in"
python3 -I "$helper" gamepad >"$tmp/pad" &
pad=$!
for _ in $(seq 50); do [ -s "$tmp/pad" ] && break; sleep 0.1; done
ev=$(head -1 "$tmp/pad")
[ "$ev" != none ] && [ -n "$ev" ] || { echo "no gamepad (is uinput loaded?)" >&2; exit 1; }
udevadm settle
sys=$(readlink -f "/sys/class/input/$ev")
read -r maj min < <(awk -F= '/^MAJOR/{a=$2} /^MINOR/{b=$2} END{print a, b}' "$sys/uevent")
echo "  $ev ($maj:$min) at $sys"
grep -q "ID_INPUT_JOYSTICK=1" "/run/udev/data/c$maj:$min" && ok "udev calls it a joystick" || bad "udev calls it a joystick"
refuse "not in the wadspace yet" inside test -e "/dev/input/$ev"

echo "== handed to the wadspace"
inside python3 /tmp/devices-helper.py monitor input 6 >"$tmp/mon" 2>&1 &
mon=$!
for _ in $(seq 50); do grep -q ready "$tmp/mon" 2>/dev/null && break; sleep 0.1; done
mkdir -p "$cr/dev/input" "$cr/run/udev/data"
[ -e "$cr/run/udev/control" ] || touch "$cr/run/udev/control"
mknod -m 0660 "$cr/dev/input/$ev" c "$maj" "$min"
chown 1000:1000 "$cr/dev/input/$ev"
cp "/run/udev/data/c$maj:$min" "$cr/run/udev/data/c$maj:$min"
python3 -I "$helper" send "$pid" add "$sys" | sed 's/^/  /'
check "abc opens it" inside python3 -c "import os; os.close(os.open('/dev/input/$ev', os.O_RDONLY))"
inside python3 /tmp/devices-helper.py monitor input 0 >"$tmp/listed" 2>&1 || true
grep -q "listed /dev/input/$ev joystick=1" "$tmp/listed" && ok "libudev lists it as a joystick" ||
  { bad "libudev lists it as a joystick"; sed 's/^/    /' "$tmp/listed"; }

echo "== taken away again"
sleep 1
rm -f "$cr/dev/input/$ev" "$cr/run/udev/data/c$maj:$min"
python3 -I "$helper" send "$pid" remove "$sys" | sed 's/^/  /'
wait "$mon" || true
grep -q "heard add /dev/input/$ev joystick=1" "$tmp/mon" && ok "libudev heard it come" || bad "libudev heard it come"
grep -q "heard remove /dev/input/$ev" "$tmp/mon" && ok "libudev heard it go" || bad "libudev heard it go"
grep heard "$tmp/mon" | sed 's/^/    /' || sed 's/^/    /' "$tmp/mon"
refuse "gone from the wadspace" inside test -e "/dev/input/$ev"

echo "== raw USB"
mkdir -p "$cr/dev/bus/usb/001"
mknod -m 0660 "$cr/dev/bus/usb/001/001" c 189 0
chown 1000:1000 "$cr/dev/bus/usb/001/001"
check "abc reads a root hub's descriptor" inside python3 -c "
import os; f = os.open('/dev/bus/usb/001/001', os.O_RDWR); d = os.read(f, 18); assert d[1] == 1, d"

echo "== what the wadspace can't do"
refuse "root inside can't make a node" podman exec "$name" mknod /tmp/made c 13 0
mknod -m 0660 "$cr/dev/trymisc" c 10 200
chown 1000:1000 "$cr/dev/trymisc"
refuse "a kind not handed out (misc) can't be opened" inside python3 -c "import os; os.open('/dev/trymisc', os.O_RDWR)"
refuse "not even by root inside" podman exec "$name" python3 -c "import os; os.open('/dev/trymisc', os.O_RDWR)"

if [ "$failed" = 0 ]; then echo ok; else echo FAILED; exit 1; fi
