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
#     (vcs, 7:0; not misc: podman always allows /dev/net/tun) can't be opened;
#   - then wadd's own hands (devices::attach::HostHands, from its test
#     binary) hand the gamepad over and take it back the same way;
#   - a drive (a tmpfs standing in for one) bound by wadd on the machine
#     after the wadspace started shows up in its ~/Drives (rslave), and
#     can't be taken out while a file in there is open.
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
drive_dir=/run/wadspaces-drives/attached/$name
drive_src=/run/wadspaces-drives/try-$$
finish() {
  umount -l "$drive_dir/TRY" 2>/dev/null || true
  umount -l "$drive_src" 2>/dev/null || true
  rm -rf "$drive_dir" "$drive_src"
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
mkdir -p "$drive_dir"
podman run -d --rm --name "$name" --security-opt label=disable --entrypoint sleep "${rules[@]}" \
  -v "$drive_dir:/config/Drives:rslave" "$image" infinity >/dev/null
pid=$(podman inspect -f '{{.State.Pid}}' "$name")
cr=/proc/$pid/root
podman cp "$helper" "$name:/tmp/devices-helper.py"
# What the base's init-desktop-config does at start, before any app watches.
podman exec "$name" sh -c 'mkdir -p /run/udev/data && touch /run/udev/control'
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

echo "== handed to the wadspace (to a libudev watching from before)"
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
mknod -m 0660 "$cr/dev/tryvcs" c 7 0
chown 1000:1000 "$cr/dev/tryvcs"
refuse "a kind not handed out (vcs) can't be opened" inside python3 -c "import os; os.open('/dev/tryvcs', os.O_RDONLY)"
refuse "not even by root inside" podman exec "$name" python3 -c "import os; os.open('/dev/tryvcs', os.O_RDONLY)"

echo "== wadd's own hands"
bin=${WADD_TEST_BIN:-}
if [ -z "$bin" ]; then
  bin=$(sudo -u "${SUDO_USER:-root}" bash -lc "cd '$root' && cargo test -q -p wadd --lib --no-run --message-format=json 2>/dev/null" |
    python3 -I -c 'import json,sys
for l in sys.stdin:
    d = json.loads(l)
    if d.get("reason") == "compiler-artifact" and d.get("executable") and d["target"]["name"] == "wadd" and d["profile"]["test"]:
        print(d["executable"])' | tail -1)
fi
if [ -x "$bin" ]; then
  inside python3 /tmp/devices-helper.py monitor input 5 >"$tmp/mon2" 2>&1 &
  mon=$!
  for _ in $(seq 50); do grep -q ready "$tmp/mon2" 2>/dev/null && break; sleep 0.1; done
  WADD_TRY_PID=$pid WADD_TRY_NODE=$sys "$bin" devices::attach::hands_a_node_over --ignored -q >"$tmp/give" 2>&1 ||
    { bad "wadd gives it"; sed 's/^/    /' "$tmp/give"; }
  check "abc opens what wadd gave" inside python3 -c "import os; os.close(os.open('/dev/input/$ev', os.O_RDONLY))"
  [ "$(stat -c '%u %a' "$cr/dev/input/$ev")" = "1000 660" ] && ok "abc's, 0660" || bad "abc's, 0660"
  sleep 1
  WADD_TRY_PID=$pid WADD_TRY_NODE=$sys WADD_TRY_TAKE=1 "$bin" devices::attach::hands_a_node_over --ignored -q >"$tmp/take" 2>&1 ||
    { bad "wadd takes it"; sed 's/^/    /' "$tmp/take"; }
  wait "$mon" || true
  grep -q "heard add /dev/input/$ev joystick=1" "$tmp/mon2" && ok "libudev heard wadd's add" || bad "libudev heard wadd's add"
  grep -q "heard remove /dev/input/$ev" "$tmp/mon2" && ok "libudev heard wadd's remove" || bad "libudev heard wadd's remove"
  refuse "wadd took it away" inside test -e "/dev/input/$ev"
  # A symlink the container plants leads only into the container.
  podman exec "$name" sh -c 'rm -rf /run/udev/data && ln -s /etc /run/udev/data'
  WADD_TRY_PID=$pid WADD_TRY_NODE=$sys "$bin" devices::attach::hands_a_node_over --ignored -q >/dev/null 2>&1 || true
  [ -e "/etc/c$maj:$min" ] && bad "a planted symlink led out of the container" || ok "a planted symlink stays inside"
  echo "== a drive, into a wadspace that's running"
  mkdir -p "$drive_src"
  mount -t tmpfs -o size=1m,uid=1000,gid=1000 trydrive "$drive_src"
  echo "on the drive" >"$drive_src/hello.txt"
  WADD_TRY_BIND="$drive_src:$drive_dir/TRY" "$bin" devices::attach::binds_a_drive_on_the_machine --ignored -q >/dev/null 2>&1 ||
    bad "wadd binds it"
  check "abc reads it at ~/Drives/TRY" inside sh -c 'grep -q "on the drive" /config/Drives/TRY/hello.txt'
  check "abc writes to it" inside sh -c 'echo back > /config/Drives/TRY/back.txt'
  [ -f "$drive_src/back.txt" ] && ok "what abc wrote is on the drive" || bad "what abc wrote is on the drive"
  inside sh -c 'exec 3</config/Drives/TRY/hello.txt; sleep 4' &
  holder=$!
  sleep 1
  out=$(WADD_TRY_UNBIND="$drive_dir/TRY" "$bin" devices::attach::binds_a_drive_on_the_machine --ignored --nocapture -q 2>&1)
  grep -q busy <<<"$out" && ok "not taken out while a file is open" || { bad "not taken out while a file is open"; echo "$out" | sed 's/^/    /'; }
  wait "$holder" 2>/dev/null || true
  out=$(WADD_TRY_UNBIND="$drive_dir/TRY" "$bin" devices::attach::binds_a_drive_on_the_machine --ignored --nocapture -q 2>&1)
  grep -q unbound <<<"$out" && ok "taken out once it's closed" || { bad "taken out once it's closed"; echo "$out" | sed 's/^/    /'; }
  refuse "gone from ~/Drives" inside test -e /config/Drives/TRY/hello.txt
else
  bad "no wadd test binary (set WADD_TEST_BIN)"
fi

if [ "$failed" = 0 ]; then echo ok; else echo FAILED; exit 1; fi
