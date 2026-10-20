#!/usr/bin/env bash
# wadd getting packages for real: CI's index, its private registry (ghcr,
# signed in with a GitHub token) and podman, in a store of its own (yours
# isn't touched). The base is a stand-in: an empty image with the ABI label,
# which is all fetching reads of it. A wadd (serve --user) runs on that
# podman with everything of its own under the trial's folder (state, units,
# socket); then GET /v1/packages, POST /v1/packages/fetch, and what's in the
# store afterwards.
#
#   GITHUB_TOKEN=<a token that reads hunggod's packages> apps/wadd/dev/try-fetch.sh [ids…]
#
# The ids default to electron-deps (the smallest). TRY_DIR moves the trial
# (default .build/try-fetch; it's emptied first).
set -euo pipefail
here=$(cd "$(dirname "$0")/../../.." && pwd)
dir=${TRY_DIR:-$here/.build/try-fetch}
abi=${ABI:-trixie.1}
ids=("$@")
[ ${#ids[@]} -gt 0 ] || ids=(electron-deps)
[ -n "${GITHUB_TOKEN:-}" ] || { echo "GITHUB_TOKEN: a token that reads hunggod's packages" >&2; exit 1; }

P=(podman --root "$dir/storage" --runroot "$dir/run" --storage-driver overlay)
svc_pid="" wadd_pid=""
finish() {
  [ -n "$wadd_pid" ] && kill "$wadd_pid" 2>/dev/null && wait "$wadd_pid" 2>/dev/null
  [ -n "$svc_pid" ] && kill "$svc_pid" 2>/dev/null && wait "$svc_pid" 2>/dev/null
  true
}
trap finish EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

[ -d "$dir/storage" ] && podman unshare rm -rf "$dir/storage"
rm -rf "$dir" && mkdir -p "$dir/logs" "$dir/state" "$dir/units" "$dir/projects" "$dir/signins"
cargo build -q -p wadd --manifest-path "$here/Cargo.toml"

echo "== a store of its own, a stand-in base ($abi), the token as github_token"
printf 'FROM scratch\nLABEL io.wadspaces.abi=%s\n' "$abi" | "${P[@]}" build -q -t localhost/wadspaces-base:trixie -f - "$dir" >/dev/null
printf '%s' "$GITHUB_TOKEN" | "${P[@]}" secret create github_token - >/dev/null
"${P[@]}" system service --time=0 "unix://$dir/podman.sock" >"$dir/logs/service.log" 2>&1 &
svc_pid=$!

cat >"$dir/wadd.toml" <<EOF
[daemon]
state_dir = "$dir/state"
podman_socket = "$dir/podman.sock"
units_dir = "$dir/units"
projects_dir = "$dir/projects"
signins_dir = "$dir/signins"
[display]
enabled = false
[keys]
enabled = false
[streaming]
enabled = false
listen = ["127.0.0.1:0"]
[tailnet]
enabled = false
[cloud]
enabled = false
EOF
"$here/target/debug/wadd" serve --user --config "$dir/wadd.toml" --socket "$dir/wadd.sock" >"$dir/logs/wadd.log" 2>&1 &
wadd_pid=$!
for _ in $(seq 50); do [ -S "$dir/wadd.sock" ] && [ -S "$dir/podman.sock" ] && break; sleep 0.2; done
[ -S "$dir/wadd.sock" ] || fail "wadd didn't start: $(tail -5 "$dir/logs/wadd.log")"
api() { curl -sS --unix-socket "$dir/wadd.sock" "$@"; }

summary() {
  python3 -I -c '
import json, sys
l = json.load(sys.stdin)
if "packages" not in l: sys.exit("not a package list: " + json.dumps(l)[:300])
print("  abi %s, index from %s, %d packages" % (l["abi"], l["indexFrom"], len(l["packages"])))
for p in l["packages"]:
    if p["id"] in sys.argv[1:]:
        print("  %-24s %-8s %8.1f MB  %s" % (p["id"], p["state"], p["sizeMb"], p.get("image") or ""))'  "${ids[@]}"
}

echo "== GET /v1/packages"
api http://wadd/v1/packages | summary

echo "== POST /v1/packages/fetch ${ids[*]}"
body=$(python3 -I -c 'import json, sys; print(json.dumps({"ids": sys.argv[1:]}))' "${ids[@]}")
api -X POST -H 'content-type: application/json' -d "$body" http://wadd/v1/packages/fetch >/dev/null
started=$(date +%s)
while :; do
  f=$(api http://wadd/v1/packages | python3 -I -c 'import json, sys; print(json.dumps(json.load(sys.stdin)["fetch"]))')
  status=$(printf '%s' "$f" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["status"])')
  case "$status" in done | error | cancelled) break ;; esac
  [ $(($(date +%s) - started)) -lt 1800 ] || fail "the fetch took over 30 minutes"
  sleep 1
done
printf '%s' "$f" | python3 -I -c '
import json, sys
f = json.load(sys.stdin)
print("  %s in %.0f s: %s" % (f["status"], f["finished"] - f["started"], ", ".join("%s %s" % (g["id"], g["how"]) for g in f["got"])))
print("  %d of %d bytes" % (f["doneBytes"], f["totalBytes"]))
if f["error"]: print("  error:", f["error"])'
[ "$status" = done ] || fail "the fetch ended $status"

echo "== GET /v1/packages again"
api http://wadd/v1/packages | summary
echo "== the store"
"${P[@]}" images --format '  {{.Repository}}:{{.Tag}} {{.ID}} {{.Size}}' | grep wadspaces-pkg || fail "no package in the store"
for id in "${ids[@]}"; do
  "${P[@]}" images --format '{{.Repository}}' | grep -qx "localhost/wadspaces-pkg-$id" || fail "$id isn't named localhost/wadspaces-pkg-$id:<image id>"
done
echo "ok"
