#!/usr/bin/env bash
# Tries the Rust wadd's view on this laptop, against a real sway: a headless
# sway (no window on your desktop) with the machine's sway rules, the Rust
# wadd as you, and a real lean workspace whose desktop opens as a window in
# that sway. Checks adoption, switching, Super+Tab (by API), focus rules and
# stop.
#
#   apps/wadd/dev/try-view.sh [image]     default localhost/wadspaces-base:trixie
#
# Needs sway: installed, or unpacked under .build/sway (no root needed):
#   cd .build/sway && dnf download sway sway-config-upstream wlroots libseat libliftoff xcb-util-errors \
#     && for r in *.rpm; do rpm2cpio $r | cpio -idm; done
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
image=${1:-localhost/wadspaces-base:trixie}
dir=$root/.build/wadd-try-view
sock=$dir/wadd.sock
rm -rf "$dir"
mkdir -p "$dir/state" "$dir/projects"
. "$root/apps/wadd/dev/sway.sh"
podman image exists localhost/wadspaces-trytest:latest ||
  podman build -q -t localhost/wadspaces-trytest:latest "$root/apps/wadd/dev/test-image" >/dev/null
start_sway "$dir"

cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try-view"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
ready_timeout_s = 120
[display]
socket = "$ipc"
TOML
cat >"$dir/state/workspaces.json" <<JSON
[{"id":"trynative","name":"Try native","image":"$image","display":"host","hotkey":1,"enabled":true,"autostart":false,
  "containerName":"wad-trynative","containerPort":3000,
  "env":[["PUID","1000"],["PGID","1000"],["WADSPACES_WAYLAND","$rt_name/wayland-1"]],
  "secrets":[],"volumes":[],"devices":[],"shmSize":"1g","projects":[]},
 {"id":"trystream","name":"Try stream","image":"localhost/wadspaces-trytest:latest","display":"stream","port":3198,"hotkey":2,
  "enabled":true,"autostart":false,"containerName":"wad-trystream","containerPort":3000,
  "env":[],"secrets":[],"volumes":[],"devices":[],"projects":[]}]
JSON

(cd "$root" && cargo build -q -p wadd)
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
wadd_pid=$!
finish() {
  kill "$wadd_pid" 2>/dev/null || true
  wait "$wadd_pid" 2>/dev/null || true
  systemctl --user stop wad-trynative.service wad-trystream.service 2>/dev/null || true
  rm -f "$XDG_RUNTIME_DIR"/containers/systemd/wad-try*.container
  systemctl --user daemon-reload
  stop_sway
}
trap finish EXIT
for _ in $(seq 50); do [ -S "$sock" ] && break; sleep 0.1; done

api() { curl -sS --unix-socket "$sock" -X "$1" ${3:+-H 'Content-Type: application/json' -d "$3"} -w ' %{http_code}' "http://wadd$2"; echo; }
view() { curl -sS --unix-socket "$sock" http://wadd/v1/view | sed -n 's/.*"view":\({[^}]*}\).*"pending":\([^,]*\).*/\1 pending=\2/p'; }
until_view() { # substring
  for _ in $(seq 1200); do view | grep -q "$1" && return 0; sleep 0.1; done
  echo "timed out: $(view)"; tail -5 "$dir/wadd.log"; return 1
}
elapsed() { echo "$(( ($(date +%s%N) - t0) / 1000000 )) ms"; }

echo "== sway is up; wadd sees it"
until_view '"kind":"home"'; api GET /v1/view
echo "== switch to the native workspace (cold): the screen stays put until its window is up"
t0=$(date +%s%N); api POST /v1/workspaces/trynative/switch
until_view '"kind":"workspace","id":"trynative"'; echo "shown after $(elapsed)"
where; echo "focused workspace: $(focused)"
echo "== Super+Tab (by API) back to WadSpaces Client"
api POST /v1/carousel/next; api POST /v1/carousel/commit
until_view '"kind":"home"'; echo "focused workspace: $(focused)"
echo "== switch again (warm)"
t0=$(date +%s%N); api POST /v1/workspaces/trynative/switch >/dev/null
until_view '"id":"trynative"'; echo "shown after $(elapsed); focused workspace: $(focused)"
echo "== a focus session on it: WadSpaces Client and the other workspace wait"
api POST /v1/session '{"workspaces":["trynative"],"minutes":5}'
api POST /v1/workspaces/trynative/switch >/dev/null
api POST /v1/view/home
api POST /v1/workspaces/trystream/switch
api GET /v1/session
api DELETE '/v1/session?force=true'
echo "== stop it: its window closes, back to WadSpaces Client"
t0=$(date +%s%N); api POST /v1/workspaces/trynative/stop; echo "stopped after $(elapsed)"
until_view '"kind":"home"'; where; echo "focused workspace: $(focused)"
echo "== wadd's log"
grep -E "sway|window|ready|session|key" "$dir/wadd.log" | tail -12
