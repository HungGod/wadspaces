#!/usr/bin/env bash
# Streams on this laptop, as you: `wadd serve --user` streams a real
# wadspace (the Writing template's image) on the real stream sidecar
# (localhost/wadspaces-stream:trixie), rootless, behind its proxy on a spare
# port (18085), asked for by Host as the tunnel would. No tunnel: the
# trial's wadd doesn't need one (require_tunnel = false), and its streams
# are ws-<guid>.stream.localhost (a browser treats *.localhost as secure,
# as it does https://ws-<guid>.wadspaces.com, and finds it on this machine).
#
#   apps/wadd/dev/try-stream.sh
#   IDLE=1 apps/wadd/dev/try-stream.sh   also a stream nobody watches,
#                                        ending by itself (about 90 s)
#
# Checks: the sidecar's port is on 127.0.0.1 only; through the proxy the
# login is asked for (401 + Basic) on the page and the websocket, the right
# one gets the page (200) and a websocket (101), a browser sees the desktop
# (a screenshot); unknown hosts get 404 and ws-selftest is answered; wadd
# restarting keeps the stream (same link, same login); ending it takes the
# containers, the units and the password with it, and the link 404s.
#
# Needs podman.socket, the images (images/build.sh, and a Writing template:
# localhost/wadspaces-template-writing:latest) and, for the screenshot,
# playwright-core in .build/uitools. Everything it makes is removed.
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
dir=$root/.build/wadd-try-stream
sock=$dir/wadd.sock
proxy=18085
ws=trystream
image=localhost/wadspaces-template-writing:latest
rm -rf "$dir"
mkdir -p "$dir/state" "$dir/projects" "$dir/signins"
chmod 700 "$dir"
for img in "$image" localhost/wadspaces-stream:trixie; do
  podman image exists "$img" || { echo "needs $img (images/build.sh)" >&2; exit 1; }
done
[ -S "$XDG_RUNTIME_DIR/podman/podman.sock" ] || systemctl --user start podman.socket

idle=30
[ -n "${IDLE:-}" ] && idle=1
cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try-stream"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
signins_dir = "$dir/signins"
vendor_workspaces = "$dir/none.d"
ready_timeout_s = 300
[display]
enabled = false
[cloud]
enabled = false
[streaming]
domain = "stream.localhost"
listen = ["127.0.0.1:$proxy"]
require_tunnel = false
idle_minutes = $idle
TOML
cat >"$dir/state/workspaces.json" <<JSON
[{"id":"$ws","name":"Try Stream","image":"$image","display":"host","port":null,"hotkey":null,"icon":null,
  "enabled":true,"autostart":false,"containerName":"wad-$ws","containerPort":3000,
  "env":[["PUID","1000"],["PGID","1000"],["TZ","Pacific/Fiji"],["TITLE","Try Stream"]],"secrets":[],"volumes":[],
  "devices":["/dev/dri"],"shmSize":"1g","projects":[]}]
JSON

(cd "$root" && cargo build -q -p wadd)
pid=
start_wadd() {
  "$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >>"$dir/wadd.log" 2>&1 &
  pid=$!
  for _ in $(seq 100); do [ -S "$sock" ] && curl -s --unix-socket "$sock" http://wadd/v1/health >/dev/null && break; sleep 0.1; done
}
stop_wadd() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
finish() {
  stop_wadd
  systemctl --user stop "wad-$ws.service" "wad-$ws-display.service" 2>/dev/null || true
  rm -f "$XDG_RUNTIME_DIR"/containers/systemd/wad-$ws.container "$XDG_RUNTIME_DIR"/containers/systemd/wad-$ws-display.*
  systemctl --user daemon-reload
  podman secret rm "wad_stream_$ws" >/dev/null 2>&1 || true
  podman volume rm -f "wad-$ws-display" "wad-$ws-config" >/dev/null 2>&1 || true
}
trap finish EXIT
start_wadd

api() { curl -sS --unix-socket "$sock" -X "$1" ${3:+-H 'content-type: application/json' -d "$3"} "http://wadd$2"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
status() { curl -s -o /dev/null -m 10 -w '%{http_code}' "$@"; }
upgrade=(--http1.1 -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13'
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==')
# A websocket's answer: its status line only (curl would wait on the socket).
ws_status() { curl -s -i -m 3 "${upgrade[@]}" "$@" 2>/dev/null | head -1 | tr -d '\r' || true; }

echo "== streaming it"
share=$(api POST /v1/workspaces/$ws/stream '{}')
link=$(echo "$share" | field 'd["link"]')
user=$(echo "$share" | field 'd["user"]')
pw=$(echo "$share" | field 'd["password"]')
host=${link#https://}; host=${host%/}
echo "link $link, user $user, a password of ${#pw} characters"
for _ in $(seq 300); do
  phase=$(api GET /v1/workspaces/$ws/state | field 'd["phase"]')
  [ "$phase" = ready ] || [ "$phase" = error ] && break
  sleep 1
done
api GET /v1/workspaces/$ws/state | field '("phase:", d["phase"], "streaming:", d["streaming"], "error:", d["error"])'
web=$(podman port "wad-$ws-display" 3000/tcp)
echo "the sidecar's web port: $web (127.0.0.1 only: $([[ $web == 127.0.0.1:* ]] && echo yes || echo NO))"
echo "streams.json has no password: $(grep -q "$pw" "$dir/state/streams.json" && echo NO || echo yes)"
echo "units have no password: $(grep -lq "$pw" "$XDG_RUNTIME_DIR"/containers/systemd/wad-$ws* 2>/dev/null && echo NO || echo yes)"

at=http://127.0.0.1:$proxy
echo "== through the proxy, as the tunnel delivers it"
echo "no login:    page $(status -H "Host: $host" $at/), websocket '$(ws_status -H "Host: $host" $at/api/websockets)'"
echo "             $(curl -s -D - -o /dev/null -m 10 -H "Host: $host" $at/ | grep -i '^www-authenticate' | tr -d '\r')"
echo "wrong login: page $(status -u "$user:nope-nope-nope" -H "Host: $host" $at/)"
# Selkies' websocket comes up a few seconds after its page does.
for _ in $(seq 30); do
  [[ $(ws_status -u "$user:$pw" -H "Host: $host" $at/api/websockets) == *" 101 "* ]] && break
  sleep 1
done
echo "right login: page $(status -u "$user:$pw" -H "Host: $host" $at/), websocket '$(ws_status -u "$user:$pw" -H "Host: $host" $at/api/websockets)'"
echo "unknown:     $(status -H 'Host: ws-ffffffffffffffffffffffffffffffff.stream.localhost' $at/) $(status -H 'Host: www.stream.localhost' $at/) $(status $at/)"
echo "self-test:   $(status -H 'Host: ws-selftest.stream.localhost' "$at/?n=$(printf 'a%.0s' $(seq 32))")"

shot=$dir/desktop.png
if [ -d "$root/.build/uitools/node_modules/playwright-core" ]; then
  echo "== in a browser"
  node "$root/apps/wadd/dev/stream-probe.mjs" "http://$host:$proxy/" "$user" "$pw" "$shot" 2>&1 | tail -3 || true
  echo "screenshot: $shot"
fi

echo "== wadd restarts: the stream stays"
stop_wadd
start_wadd
for _ in $(seq 30); do [ "$(status -u "$user:$pw" -H "Host: $host" $at/)" = 200 ] && break; sleep 1; done
echo "same link, same login: page $(status -u "$user:$pw" -H "Host: $host" $at/)"
api GET /v1/streams | field '[(s["workspace"], s["live"], s["link"] == "'"$link"'") for s in d]'

echo "== ending it"
out=$(api DELETE /v1/workspaces/$ws/stream)
if [ -z "$out" ]; then echo "ended"; else echo "  it had ended already: $out"; fi
sleep 2
echo "containers left: $(podman ps --format '{{.Names}}' | grep -c "^wad-$ws" || true)"
echo "units now: $(ls "$XDG_RUNTIME_DIR"/containers/systemd/ | grep "^wad-$ws" | tr '\n' ' ')"
echo "password: $(podman secret exists "wad_stream_$ws" && echo "STILL THERE" || echo gone)"
echo "the link: $(status -u "$user:$pw" -H "Host: $host" $at/)"

if [ -n "${IDLE:-}" ]; then
  echo "== nobody watching (idle_minutes = 1)"
  api POST /v1/workspaces/$ws/stream '{}' >/dev/null
  for _ in $(seq 150); do
    n=$(api GET /v1/streams | field 'len(d)')
    [ "$n" = 0 ] && break
    sleep 1
  done
  echo "streams left: $(api GET /v1/streams | field 'len(d)')"
fi

echo "== wadd's warnings"
grep -iE 'warn|error' "$dir/wadd.log" | tail -8 || true
