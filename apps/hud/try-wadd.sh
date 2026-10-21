#!/usr/bin/env bash
# The HUD against a real wadd, on a headless sway with the machine's rules:
# WadNotes' cards follow the wadspace on screen (Default, a wadspace, then
# Default again for the terminal). Screenshots go to .build/hud-try-wadd/shots.
#
#   apps/hud/try-wadd.sh
#
# Needs: sway, foot and grim (installed, or unpacked under .build/sway, as
# apps/wadd/dev/sway.sh says), `systemctl --user start podman.socket`, and
# the HUD and wadd built (cargo build -p wadspaces-hud -p wadd).
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../.." && pwd)
dir=$root/.build/hud-try-wadd
out=$dir/shots
rm -rf "$dir"; mkdir -p "$dir/state" "$dir/projects" "$dir/config/wadspaces" "$out"
. "$root/apps/wadd/dev/sway.sh"
podman image exists localhost/wadspaces-trytest:latest ||
  podman build -q -t localhost/wadspaces-trytest:latest "$root/apps/wadd/dev/test-image" >/dev/null
start_sway "$dir"
if command -v foot >/dev/null; then foot=$(command -v foot); foot_libs=
else foot=$root/.build/sway/usr/bin/foot; foot_libs=$root/.build/sway/usr/lib64; fi
grim=$(command -v grim || echo "$root/.build/sway/usr/bin/grim")
sock=$dir/wadd.sock
cat >"$dir/wadd.toml" <<TOML
[machine]
name = "hud-try-wadd"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
vendor_workspaces = "$dir/none.d"
ready_timeout_s = 60
[display]
socket = "$ipc"
TOML
cat >"$dir/state/workspaces.json" <<JSON
[{"id":"terminal","name":"Terminal","kind":"app","exec":["$foot"],"image":"","display":"host","hotkey":null,
  "enabled":true,"autostart":false,"containerName":"wad-terminal","containerPort":3000,
  "env":[["LD_LIBRARY_PATH","$foot_libs"]],"secrets":[],"volumes":[],"devices":[],"shmSize":null,"projects":[]},
 {"id":"novel","name":"Novel","image":"localhost/wadspaces-trytest:latest","display":"stream","port":3197,"hotkey":2,
  "enabled":true,"autostart":false,"containerName":"wad-novel","containerPort":3000,
  "env":[],"secrets":[],"volumes":[],"devices":[],"projects":[]}]
JSON
echo '{"notes":true,"tasks":true,"calendar":true}' > "$dir/config/wadspaces/hud.json"
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
wadd_pid=$!
for _ in $(seq 50); do [ -S "$sock" ] && break; sleep 0.1; done
# Its own session bus, without your desktop's display (what the bus starts
# must not attach to it).
env -u DISPLAY -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$rt" XDG_CONFIG_HOME="$dir/config" \
  WADD_SOCKET="$sock" WADNOTES_DIR="$dir/notes" GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- env WAYLAND_DISPLAY=wayland-1 "$root/target/debug/wadspaces-hud" >"$dir/hud.log" 2>&1 &
hud_pid=$!
finish() {
  kill "$hud_pid" "$wadd_pid" 2>/dev/null || true
  systemctl --user stop wad-terminal.service wad-novel.service 2>/dev/null || true
  systemctl --user reset-failed wad-novel.service 2>/dev/null || true
  rm -f "$XDG_RUNTIME_DIR"/containers/systemd/wad-novel.container; systemctl --user daemon-reload
  for p in $(pgrep -u "$(id -u)" -f "^/usr/libexec/xdg-"); do
    tr "\0" "\n" < /proc/$p/environ 2>/dev/null | grep -qx "XDG_RUNTIME_DIR=$rt" && kill "$p"
  done
  sleep 1
  fusermount3 -u "$rt/doc" 2>/dev/null || true
  stop_sway
}
trap finish EXIT
api() { curl -sS --unix-socket "$sock" -X "$1" -o /dev/null -w '%{http_code}\n' "http://wadd$2"; }
shot() { sleep 1.5; env XDG_RUNTIME_DIR="$rt" WAYLAND_DISPLAY=wayland-1 LD_LIBRARY_PATH=$root/.build/sway/usr/lib64 "$grim" "$out/$1.png"; echo "shot $out/$1.png"; }
until_view() { for _ in $(seq 150); do curl -sS --unix-socket "$sock" http://wadd/v1/view | grep -q "$1" && return 0; sleep 0.2; done; echo "timed out: $1"; }
sleep 3
shot 1-home
api POST /v1/workspaces/novel/switch >/dev/null; until_view '"id":"novel"'
shot 2-novel
api POST /v1/workspaces/terminal/switch >/dev/null; until_view '"id":"terminal"'
shot 3-terminal
grep -iE "warn|critical|panic" "$dir/hud.log" | grep -v "MESA\|portal" | head -10 || true
