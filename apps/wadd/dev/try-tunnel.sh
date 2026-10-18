#!/usr/bin/env bash
# A real stream through the real Cloudflare tunnel, from this laptop: for a
# phone on cellular. wadd serve --user streams the Writing template behind
# its proxy on :8085 (the tunnel's route), at https://ws-<guid>.wadspaces.com.
#
#   sudo systemctl start cloudflared     # the laptop's connector, first
#   apps/wadd/dev/try-tunnel.sh
#   sudo systemctl stop cloudflared      # after: never beside the Surface's
#
# It shows the tunnel's state and runs the self-test (through Cloudflare and
# back), then the sign-in link (a QR code of it too, with qrencode) and
# the link with its login, and waits while you open them on the phone: the
# link with no login first (the login prompt), then the sign-in link (the
# desktop, no login asked). Enter ends the stream; everything it made is
# removed. Needs podman.socket and the images (see try-stream.sh).
#
#   PHONE=1 apps/wadd/dev/try-tunnel.sh  also opens the sign-in link in the
#                                        WadSpaces app's stream view, on the
#                                        phone on adb (a debug build)
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
dir=$root/.build/wadd-try-tunnel
sock=$XDG_RUNTIME_DIR/wadd-try-tunnel.sock
ws=trytunnel
image=localhost/wadspaces-template-writing:latest
rm -rf "$dir"
mkdir -p "$dir/state" "$dir/projects" "$dir/signins"
chmod 700 "$dir"
for img in "$image" localhost/wadspaces-stream:trixie; do
  podman image exists "$img" || { echo "needs $img (images/build.sh)" >&2; exit 1; }
done
[ -S "$XDG_RUNTIME_DIR/podman/podman.sock" ] || systemctl --user start podman.socket
if [ "$(systemctl is-active cloudflared 2>/dev/null)" != active ]; then
  echo "cloudflared isn't running: sudo systemctl start cloudflared" >&2
  exit 1
fi

cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try-tunnel"
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
domain = "wadspaces.com"
TOML
cat >"$dir/state/workspaces.json" <<JSON
[{"id":"$ws","name":"Try Tunnel","image":"$image","display":"host","port":null,"hotkey":null,"icon":null,
  "enabled":true,"autostart":false,"containerName":"wad-$ws","containerPort":3000,
  "env":[["PUID","1000"],["PGID","1000"],["TZ","Pacific/Fiji"],["TITLE","Try Tunnel"]],"secrets":[],"volumes":[],
  "devices":["/dev/dri"],"shmSize":"1g","projects":[]}]
JSON

(cd "$root" && cargo build -q -p wadd)
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
pid=$!
finish() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  systemctl --user stop "wad-$ws.service" "wad-$ws-display.service" 2>/dev/null || true
  rm -f "$XDG_RUNTIME_DIR"/containers/systemd/wad-$ws.container "$XDG_RUNTIME_DIR"/containers/systemd/wad-$ws-display.*
  systemctl --user daemon-reload
  podman secret rm "wad_stream_$ws" >/dev/null 2>&1 || true
  podman volume rm -f "wad-$ws-display" "wad-$ws-config" >/dev/null 2>&1 || true
  rm -f "$sock"
  echo "cleaned up; now: sudo systemctl stop cloudflared"
}
trap finish EXIT
for _ in $(seq 100); do [ -S "$sock" ] && curl -s --unix-socket "$sock" http://wadd/v1/health >/dev/null && break; sleep 0.1; done

api() { curl -sS --unix-socket "$sock" -X "$1" ${3:+-H 'content-type: application/json' -d "$3"} "http://wadd$2"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

echo "== the tunnel"
api GET /v1/tunnel | field '"\n".join(f"  {k}: {v}" for k, v in d.items())'
echo "== the self-test (https://ws-selftest.wadspaces.com from here, through Cloudflare)"
api POST /v1/tunnel/selftest | field '"  ok: %s, %s ms: %s" % (d["ok"], d["ms"], d["detail"])'

echo "== streaming"
share=$(api POST /v1/workspaces/$ws/stream '{}')
echo "$share" | grep -q '"link"' || { echo "  refused: $share"; exit 1; }
link=$(echo "$share" | field 'd["link"]')
user=$(echo "$share" | field 'd["user"]')
pw=$(echo "$share" | field 'd["password"]')
open=$(echo "$share" | field 'd["openLink"]')
for _ in $(seq 300); do
  phase=$(api GET /v1/workspaces/$ws/state | field 'd["phase"]')
  [ "$phase" = ready ] || [ "$phase" = error ] && break
  sleep 1
done
echo "  $phase"
echo
echo "  sign-in link: $open"
echo
echo "  link:     $link"
echo "  user:     $user"
echo "  password: $pw"
echo
command -v qrencode >/dev/null && qrencode -t ANSIUTF8 "$open"
echo "On the phone (Wi-Fi off): open the plain link and cancel the login (the"
echo "prompt should stay, no desktop). Then open the sign-in link (or scan the"
echo "code): the desktop, with no login asked."
if [ -n "${PHONE:-}" ]; then
  adb shell "am start -S -n io.wadspaces.android/.MainActivity --es stream '$open'" >/dev/null &&
    echo "Opened in the WadSpaces app on the phone." || echo "adb couldn't open it in the app."
fi
read -r -p "Enter ends the stream... " _
out=$(api DELETE /v1/workspaces/$ws/stream)
if [ -z "$out" ]; then echo "ended"; else echo "  it had ended already: $out"; fi
echo "the link now: $(curl -s -o /dev/null -m 10 -w '%{http_code}' "$link")"
grep -iE 'warn|error' "$dir/wadd.log" | tail -5 || true
