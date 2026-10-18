#!/usr/bin/env bash
# This laptop's wadd on your tailnet, rootless: the remote API the owner's
# trusted devices use (host/TAILNET.md), for a phone running Tailscale.
#
#   apps/wadd/dev/try-tailnet.sh               # join (once), then ask/allow
#   apps/wadd/dev/try-tailnet.sh --link CODE   # also link to the account
#   apps/wadd/dev/try-tailnet.sh --leave       # leave the tailnet at the end
#   apps/wadd/dev/try-tailnet.sh --fresh       # a new node, nothing trusted
#
# tailscaled runs in userspace mode (Tailscale's static build, pinned, in
# .build/tailscale): it leaves the laptop's own network and DNS alone, and
# hands the tailnet's connections to :8087 on to 127.0.0.1:8087, where this
# wadd's remote API listens; whois still says which device each is from.
# The first run prints a login link (a QR code too, with qrencode): approve
# "wadd-try-tailnet" in your tailnet. Its state is kept in
# .build/wadd-try-tailnet, so later runs are already on the tailnet.
#
# Then, on the phone (Tailscale on): http://<the address it shows>:8087/
# says "asked"; the script asks here whether to allow it; after a reload it
# says "trusted". Enter ends; it stops only what it started.
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
version=1.104.1
sha=108d1d96ecf410d305571e173516f27038f870919a9b89c914a167c6d33a4528
bin=$root/.build/tailscale
dir=$root/.build/wadd-try-tailnet
sock=$XDG_RUNTIME_DIR/wadd-try-tailnet.sock
tsock=$XDG_RUNTIME_DIR/wadd-try-ts.sock
port=8087
link= leave= fresh=
while [ $# -gt 0 ]; do
  case $1 in
    --link) link=$2; shift ;;
    --leave) leave=1 ;;
    --fresh) fresh=1 ;;
    *) echo "unknown: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ ! -x "$bin/tailscaled" ] || ! "$bin/tailscale" version | grep -qx "$version"; then
  echo "== Tailscale $version (static build)"
  tmp=$(mktemp -d)
  curl -fsSLo "$tmp/t.tgz" "https://pkgs.tailscale.com/stable/tailscale_${version}_amd64.tgz"
  echo "$sha  $tmp/t.tgz" | sha256sum -c --quiet -
  tar -xzf "$tmp/t.tgz" -C "$tmp"
  mkdir -p "$bin"
  mv "$tmp/tailscale_${version}_amd64/tailscale" "$tmp/tailscale_${version}_amd64/tailscaled" "$bin/"
  rm -rf "$tmp"
fi
[ -n "$fresh" ] && rm -rf "$dir"
mkdir -p "$dir/ts" "$dir/state" "$dir/projects" "$dir/signins"
chmod 700 "$dir"
cloud="enabled = false"
[ -n "$link" ] && cloud=$'project_id = "wad-spaces"\nfunctions_region = "australia-southeast2"'
cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try-tailnet"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
signins_dir = "$dir/signins"
vendor_workspaces = "$dir/none.d"
[display]
enabled = false
[cloud]
$cloud
[streaming]
domain = "wadspaces.com"
[tailnet]
enabled = true
socket = "$tsock"
cli = "$bin/tailscale"
port = $port
listen = ["127.0.0.1:$port"]
bind_device = ""
hostname = "wadd-try-tailnet"
poll_s = 3
TOML

pids=()
finish() {
  if [ -n "$leave" ] && [ -S "$tsock" ]; then
    "$bin/tailscale" --socket="$tsock" logout && echo "left the tailnet"
  fi
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
  rm -f "$sock"
  echo "stopped (logs in $dir)"
}
trap finish EXIT
trap 'exit 130' INT TERM

TS_LOGS_DIR=$dir/ts "$bin/tailscaled" --tun=userspace-networking --socket="$tsock" --statedir="$dir/ts" \
  --port=0 --no-logs-no-support >"$dir/tailscaled.log" 2>&1 &
pids+=($!)
for _ in $(seq 100); do [ -S "$tsock" ] && break; sleep 0.1; done
[ -S "$tsock" ] || { echo "tailscaled didn't start:" >&2; tail -5 "$dir/tailscaled.log" >&2; exit 1; }

(cd "$root" && cargo build -q -p wadd)
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
pids+=($!)
for _ in $(seq 100); do [ -S "$sock" ] && curl -s --unix-socket "$sock" http://wadd/v1/health >/dev/null && break; sleep 0.1; done

api() { curl -sS --unix-socket "$sock" -X "$1" ${3:+-H 'content-type: application/json' -d "$3"} "http://wadd$2"; }
field() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
state() { api GET /v1/tailnet | field 'd["state"]'; }

if [ -n "$link" ]; then
  echo "== linking to the account"
  api POST /v1/cloud/link "{\"code\":\"$link\"}" | field '"  linked: %s (%s)" % (d.get("linked"), d.get("machineId"))'
fi

for _ in $(seq 30); do s=$(state); [ "$s" = starting ] || [ "$s" = off ] || break; sleep 0.5; done
if [ "$(state)" != running ]; then
  echo "== joining the tailnet"
  out=$(api POST /v1/tailnet/login)
  url=$(echo "$out" | field 'd.get("loginUrl") or ""')
  if [ -n "$url" ]; then
    echo "  approve this machine: $url"
    command -v qrencode >/dev/null && qrencode -t ANSIUTF8 "$url"
  else
    echo "  $out"
  fi
  for _ in $(seq 600); do [ "$(state)" = running ] && break; sleep 1; done
fi
t=$(api GET /v1/tailnet)
[ "$(echo "$t" | field 'd["state"]')" = running ] || { echo "not on the tailnet: $t"; exit 1; }
ip=$(echo "$t" | field '([a for a in d["ips"] if "." in a] or d["ips"])[0]')
echo "== on the tailnet as $(echo "$t" | field 'd["dnsName"]') ($(echo "$t" | field 'd["user"]'))"
echo "  remote API listening: $(echo "$t" | field 'd["listening"]')"
echo "  your devices: $(echo "$t" | field '", ".join("%s%s" % (x["name"], " (trusted)" if x["trusted"] else "") for x in d["devices"]) or "none"')"
echo
echo "On the phone (Tailscale on), open:  http://$ip:$port/"
echo "It says \"asked\"; allow it here; reload: \"trusted\". Enter ends."
asked=" "
while true; do
  if read -r -t 2 _; then break; fi
  for a in $(api GET /v1/tailnet | field '" ".join(x["id"] for x in d["asks"])'); do
    case "$asked" in *" $a "*) continue ;; esac
    asked="$asked$a "
    name=$(api GET /v1/tailnet | field "next((x['name'] + ' (' + x['os'] + ')') for x in d['asks'] if x['id'] == '$a')")
    read -r -p "Allow $name to manage this machine? [y/N] " yn
    if [ "$yn" = y ]; then
      api PUT "/v1/tailnet/devices/$a" >/dev/null && echo "  trusted: reload the page on it"
    else
      api POST "/v1/tailnet/asks/$a/dismiss" >/dev/null && echo "  not now"
    fi
  done
done
grep -E 'remote|tailnet' "$dir/wadd.log" | tail -8 || true
