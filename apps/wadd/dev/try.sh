#!/usr/bin/env bash
# Tries the Rust wadd's workspace lifecycle on this laptop, as you: rootless
# podman, your systemd, a socket and state of its own under .build/wadd-try.
#
#   apps/wadd/dev/try.sh            start, wait, stop, restart, a missing image
#   KEEP=1 apps/wadd/dev/try.sh     leave wadd running afterwards
#
# Needs podman.socket (systemctl --user enable --now podman.socket). Builds
# the test image (busybox serving a page on 3000) if it's missing.
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
dir=$root/.build/wadd-try
sock=$dir/wadd.sock
port=3199
mkdir -p "$dir/state" "$dir/projects"

podman image exists localhost/wadspaces-trytest:latest ||
  podman build -q -t localhost/wadspaces-trytest:latest "$root/apps/wadd/dev/test-image" >/dev/null

cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
ready_timeout_s = 60
TOML
ws() { # id name image display port
  printf '{"id":"%s","name":"%s","image":"%s","display":"%s",%s"enabled":true,"autostart":false,"containerName":"wad-%s","containerPort":3000,"env":[],"secrets":[],"volumes":[],"devices":[],"projects":[]}' \
    "$1" "$2" "$3" "$4" "${5:+\"port\":$5,}" "$1"
}
echo "[$(ws trystream 'Try stream' localhost/wadspaces-trytest:latest stream $port),$(ws trymissing 'Try missing' localhost/wadspaces-trymissing:latest host)]" >"$dir/state/workspaces.json"

(cd "$root" && cargo build -q -p wadd)
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
pid=$!
finish() {
  if [ -z "${KEEP:-}" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    systemctl --user stop wad-trystream.service 2>/dev/null || true
    rm -f "$XDG_RUNTIME_DIR"/containers/systemd/wad-try*.container
    systemctl --user daemon-reload
  else
    echo "wadd still running (pid $pid): curl --unix-socket $sock http://wadd/v1/states"
  fi
}
trap finish EXIT
for _ in $(seq 50); do [ -S "$sock" ] && break; sleep 0.1; done

api() { curl -sS --unix-socket "$sock" -X "$1" "http://wadd$2"; echo; }
phase() { api GET "/v1/workspaces/$1/state" | sed -n 's/.*"phase":"\([a-z]*\)".*/\1/p'; }
until_phase() { # id phase...
  local id=$1; shift
  for _ in $(seq 600); do
    p=$(phase "$id")
    for want; do [ "$p" = "$want" ] && return 0; done
    sleep 0.1
  done
  echo "timed out in phase $p"; return 1
}
elapsed() { echo "$(( ($(date +%s%N) - t0) / 1000000 )) ms"; }

echo "== units"
ls "$XDG_RUNTIME_DIR/containers/systemd/"
systemctl --user cat wad-trystream.service >/dev/null && echo "quadlet made wad-trystream.service"
echo "== start (cold: no container yet)"
t0=$(date +%s%N); api POST /v1/workspaces/trystream/start >/dev/null
until_phase trystream ready error; echo "$(phase trystream) after $(elapsed)"
curl -sS "http://127.0.0.1:$port/" | head -1
echo "== start again (warm)"
t0=$(date +%s%N); api POST /v1/workspaces/trystream/start >/dev/null
until_phase trystream ready error; echo "$(phase trystream) after $(elapsed)"
echo "== stop"
t0=$(date +%s%N); api POST /v1/workspaces/trystream/stop; echo "after $(elapsed)"
echo "== restart"
t0=$(date +%s%N); api POST /v1/workspaces/trystream/restart >/dev/null
until_phase trystream ready error; echo "$(phase trystream) after $(elapsed)"
echo "== podman stops it behind wadd's back; reconcile notices"
podman stop -t 1 wad-trystream >/dev/null
until_phase trystream idle; api GET /v1/workspaces/trystream/state
echo "== an image that isn't here: never downloaded, it says to build it"
api POST /v1/workspaces/trymissing/start >/dev/null
until_phase trymissing error; api GET /v1/workspaces/trymissing/state
echo "== runs"
api GET '/v1/runs?workspace=trystream'
echo "== wadd's log"
tail -5 "$dir/wadd.log"
