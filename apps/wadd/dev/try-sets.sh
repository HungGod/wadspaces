#!/usr/bin/env bash
# Version 4 wadspaces' sets for real (sets.rs): a wadd (serve --user) on a
# podman store of its own (yours isn't touched) installs wadspaces through
# its API and builds their sets from packages, one step at a time, each
# timed:
#
#   1 iq        IntelligenceQuest: C++, VS Code with rust-analyzer, Tiled
#               (FULL=1: Rust and LibreSprite too). Its packages come from
#               CI's registry with GITHUB_TOKEN, else wadd builds each here
#               from its recipe, one at a time (long the first time: they're
#               kept between runs). Then stacked as chain images and put
#               together.
#   2 no-tiled  Tiled taken out (in the middle): what's above it is stacked
#               again, nothing below. The tidy-up after the build takes the
#               set Tiled was in and Tiled's package (no wadspace here needs
#               it: the next run builds it again).
#   3 twin      a second wadspace with the same packages: nothing builds.
#
# Then: the set's labels and what's inside it, the extra folder wadd wrote
# (web apps and their icons, layout, WadBrowser's mode, the background), a
# container of the set started with it (the base's init takes the web apps
# and the layout, as at a real start), and the tidy-up and the disk page
# leaving what the wadspaces run on and the packages they need, with
# nothing left over. report.tsv has the times.
#
#   apps/wadd/dev/try-sets.sh
#   GITHUB_TOKEN=<reads hunggod's packages> apps/wadd/dev/try-sets.sh
#   FULL=1 apps/wadd/dev/try-sets.sh
#   FRESH=1 apps/wadd/dev/try-sets.sh     packages too from scratch
#
# The base is yours (localhost/wadspaces-base:trixie, copied in once) with
# this repo's init-wadspaces-workspace, wadspaces-webapp and the extras
# label on it: the base the next images/build.sh makes. The store is
# .build/try-sets (TRY_DIR moves it); each run starts with no sets or chains.
set -euo pipefail
ulimit -c 0
here=$(cd "$(dirname "$0")/../../.." && pwd)
dir=${TRY_DIR:-$here/.build/try-sets}
podman image exists localhost/wadspaces-base:trixie || { echo "no base: images/build.sh --only base" >&2; exit 1; }

P=(podman --root "$dir/storage" --runroot "$dir/run" --storage-driver overlay)
# The store sits in your home, which SELinux doesn't label for containers:
# the trial's podman (its service's builds too) runs them unlabelled.
export CONTAINERS_CONF_OVERRIDE=$dir/containers.conf
svc_pid="" wadd_pid="" box=""
finish() {
  [ -n "$box" ] && "${P[@]}" rm -f "$box" >/dev/null 2>&1
  [ -n "$wadd_pid" ] && kill "$wadd_pid" 2>/dev/null && wait "$wadd_pid" 2>/dev/null
  [ -n "$svc_pid" ] && kill "$svc_pid" 2>/dev/null && wait "$svc_pid" 2>/dev/null
  true
}
trap finish EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

[ -n "${FRESH:-}" ] && [ -d "$dir/storage" ] && podman unshare rm -rf "$dir/storage"
mkdir -p "$dir"
# wadd's own files go each run; the store (its packages) stays.
rm -rf "$dir/state" "$dir/units" "$dir/projects" "$dir/signins" "$dir/logs" "$dir/ctx"
mkdir -p "$dir/state" "$dir/units" "$dir/projects" "$dir/signins" "$dir/logs" "$dir/ctx"
printf '[containers]\nlabel = false\n' >"$CONTAINERS_CONF_OVERRIDE"
cargo build -q -p wadd --manifest-path "$here/Cargo.toml"
report=$dir/report.tsv
printf 'step\tseconds\tchains_built\tnote\n' >"$report"

echo "== the base: yours, with this repo's start-time extras"
mine=$(podman image inspect -f '{{.Id}}' localhost/wadspaces-base:trixie)
have=$("${P[@]}" image inspect -f '{{index .Labels "io.wadspaces.try-from"}}' localhost/wadspaces-base:trixie 2>/dev/null || true)
if [ "$have" != "$mine" ]; then
  graph=$(podman info --format '{{.Store.GraphRoot}}')
  runroot=$(podman info --format '{{.Store.RunRoot}}')
  podman unshare skopeo copy -q "containers-storage:[overlay@$graph+$runroot]localhost/wadspaces-base:trixie" \
    "containers-storage:[overlay@$dir/storage+$dir/run]localhost/wadspaces-base:orig"
  root=$here/images/base/root
  cp "$root/etc/s6-overlay/s6-rc.d/init-wadspaces-workspace/run" "$dir/ctx/init-run"
  cp "$root/usr/local/bin/wadspaces-webapp" "$dir/ctx/wadspaces-webapp"
  cat >"$dir/ctx/base.Containerfile" <<EOF
FROM localhost/wadspaces-base:orig
COPY --chmod=0755 init-run /etc/s6-overlay/s6-rc.d/init-wadspaces-workspace/run
COPY --chmod=0755 wadspaces-webapp /usr/local/bin/wadspaces-webapp
LABEL io.wadspaces.extras="1" io.wadspaces.try-from="$mine"
EOF
  "${P[@]}" build -q -t localhost/wadspaces-base:trixie -f "$dir/ctx/base.Containerfile" "$dir/ctx" >/dev/null
  "${P[@]}" rmi localhost/wadspaces-base:orig >/dev/null
fi
"${P[@]}" image inspect -f '  {{.Id}} abi {{index .Labels "io.wadspaces.abi"}} extras {{index .Labels "io.wadspaces.extras"}}' localhost/wadspaces-base:trixie

echo "== earlier runs' sets and chains go (packages stay)"
"${P[@]}" ps -aq | xargs -r "${P[@]}" rm -f >/dev/null
"${P[@]}" images --format '{{.Repository}}:{{.Tag}}' | { grep -E '^localhost/wadspaces-(set|chain)' || true; } |
  xargs -r "${P[@]}" rmi -f >/dev/null
"${P[@]}" image prune -f >/dev/null
"${P[@]}" images --format '  {{.Repository}}:{{.Tag}} {{.Size}}' | { grep wadspaces-pkg || echo "  no packages yet"; }

pull=false
if [ -n "${GITHUB_TOKEN:-}" ]; then
  pull=true
  "${P[@]}" secret rm github_token >/dev/null 2>&1 || true
  printf '%s' "$GITHUB_TOKEN" | "${P[@]}" secret create github_token - >/dev/null
fi
"${P[@]}" system service --time=0 "unix://$dir/podman.sock" >"$dir/logs/service.log" 2>&1 &
svc_pid=$!
cat >"$dir/wadd.toml" <<EOF
[machine]
name = "wadd-try-sets"
[daemon]
state_dir = "$dir/state"
vendor_workspaces = "$dir/none.d"
podman_socket = "$dir/podman.sock"
units_dir = "$dir/units"
projects_dir = "$dir/projects"
signins_dir = "$dir/signins"
build_min_free_gb = 8
[packages]
pull = $pull
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
api() { curl -sS --fail-with-body --unix-socket "$dir/wadd.sock" "$@"; }
echo "  packages: $([ "$pull" = true ] && echo "downloaded with GITHUB_TOKEN" || echo "built here (no GITHUB_TOKEN)")"

# A version 4 wadspace: id, name, Tiled or not, the extra tools.
doc() {
  python3 -I - "$@" <<'EOF'
import json, sys
wid, name, tiled, full = sys.argv[1], sys.argv[2], sys.argv[3] == "1", sys.argv[4] == "1"
tools = [{"id": "cpp"}] + ([{"id": "rust"}] if full else [])
art = ([{"app": "tiled"}] if tiled else []) + ([{"app": "libresprite"}] if full else [])
templates = [
    {"id": "dev", "name": "Dev", "apps": [
        {"app": "wadbrowser", "mode": "focus", "extras": [
            {"id": "github", "name": "GitHub", "url": "https://github.com"},
            {"id": "docs-rs", "name": "Docs.rs", "url": "https://docs.rs"}]},
        {"app": "vscode", "extras": [{"id": "rust-lang.rust-analyzer"}]},
        {"app": "terminal", "extras": tools}]}]
if art:
    templates.append({"id": "art", "name": "Game art", "apps": art})
print(json.dumps({"wadspace": {
    "version": 4, "id": wid, "owner": "try", "machine": "try", "order": 1, "templates": templates,
    "customization": {"name": name, "wallpaper": {"type": "texture", "value": "stars"}, "autostart": ["app:vscode"],
                      "projects": [], "secrets": {"bySlot": {}, "groups": []}}},
    "tz": "America/Denver"}))
EOF
}

now() { date +%s.%N; }
chains() { "${P[@]}" images --format '{{.Repository}}' | grep -c '^localhost/wadspaces-chain$' || true; }

# One step: install, build, wait; its time and the chains it stacked.
step() {
  local name=$1 wid=$2 note=$3 t0 t1 c0 b id status=""
  shift 3
  c0=$(chains)
  t0=$(now)
  doc "$wid" "$@" | api -X PUT -H 'content-type: application/json' -d @- "http://wadd/v1/wadspaces/$wid" >/dev/null
  b=$(api -X POST "http://wadd/v1/wadspaces/$wid/build")
  id=$(printf '%s' "$b" | python3 -I -c 'import json, sys; b = json.load(sys.stdin)["build"]; print(b["id"] if b else "")')
  if [ -n "$id" ]; then
    while :; do
      status=$(api "http://wadd/v1/builds/$id" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["build"]["status"])')
      case "$status" in done | error | cancelled) break ;; esac
      sleep 2
    done
    api "http://wadd/v1/builds/$id" | python3 -I -c '
import json, sys
l = json.load(sys.stdin)
for x in l["lines"]:
    if x.startswith(("»", "✓", "✗")): print("    " + x)' >"$dir/logs/$name.lines"
    api "http://wadd/v1/builds/$id" | python3 -I -c 'import json, sys; print("\n".join(json.load(sys.stdin)["lines"]))' >"$dir/logs/$name.log"
    cat "$dir/logs/$name.lines"
  else
    echo "    built already: $(printf '%s' "$b" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["imageKey"])')"
  fi
  t1=$(now)
  local secs n
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }')
  n=$(($(chains) - c0))
  printf '%s\t%s\t%s\t%s\n' "$name" "$secs" "$n" "$note" >>"$report"
  printf '  %-9s %8s s  %s chain image(s) built  %s\n' "$name" "$secs" "$n" "$note"
  [ -z "$id" ] || [ "$status" = done ] || fail "$name: the build ended $status (logs/$name.log)"
}

full=${FULL:+1}
full=${full:-0}
echo "== 1 iq"
step iq try-iq "the set from nothing (packages: $([ "$pull" = true ] && echo fetched || echo here or built here))" IntelligenceQuest 1 "$full"
iq_key=$(api http://wadd/v1/workspaces/try-iq | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["image"])')
echo "== 2 no-tiled"
step no-tiled try-iq "Tiled taken out of the middle" IntelligenceQuest 0 "$full"
set_ref=$(api http://wadd/v1/workspaces/try-iq | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["image"])')
[ "$set_ref" != "$iq_key" ] || fail "taking Tiled out didn't change the set"
# The tidy-up runs as the last build ends.
for _ in $(seq 60); do "${P[@]}" image exists "$iq_key" || break; sleep 1; done
! "${P[@]}" image exists "$iq_key" || fail "the set Tiled was in is still here"
[ "$("${P[@]}" images --format '{{.Repository}}' | grep -c '^localhost/wadspaces-pkg-tiled$')" = 0 ] ||
  fail "Tiled's package is still here"
echo "  the tidy-up took the set Tiled was in, and Tiled's package"
echo "== 3 twin"
step twin try-twin "the same packages: nothing to build" Twin 0 "$full"
[ "$(api http://wadd/v1/workspaces/try-twin | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["image"])')" = "$set_ref" ] ||
  fail "the twin isn't on iq's set"
[ "$(tail -1 "$report" | cut -f3)" = 0 ] || fail "the twin stacked chains"

echo "== the set: $set_ref"
"${P[@]}" image inspect -f '  layer {{index .Labels "io.wadspaces.layer"}}  pkgs {{index .Labels "io.wadspaces.pkgs"}}' "$set_ref"
api http://wadd/v1/images | python3 -I -c '
import json, sys
for r in json.load(sys.stdin):
    print("  /v1/images: %-18s %6.0f MB %s" % (r["layer"], r["sizeMb"], ",".join(p.split("@")[0] for p in r.get("pkgs", []))))'
"${P[@]}" run --rm --memory=1g --entrypoint bash "$set_ref" -c '
  set -e
  echo "  code $(code --version --no-sandbox --user-data-dir /tmp/v | head -1)"
  echo "  $(cmake --version | head -1)"
  [ -x /opt/tiled/AppRun ] && { echo "Tiled is still in the set"; exit 1; } || echo "  no Tiled"
  ls /usr/share/wadspaces/vscode-extensions | grep -qi rust-analyzer && echo "  rust-analyzer in the image"
  [ -z "$(dpkg --audit)" ] && echo "  dpkg --audit: clean"' || fail "the set isn't right inside"

echo "== try-iq's extra folder"
ex=$dir/state/extra/try-iq
(cd "$ex" && find . -type f | sort | sed 's/^/  /')
for f in webapps.json layout.json wadbrowser.conf wallpaper.png icons/github.png icons/docs-rs.png; do
  [ -s "$ex/$f" ] || fail "no $f in the extra folder"
done

echo "== a container of the set, started with it"
box=try-sets-iq
"${P[@]}" run -d --name "$box" --memory=1g -v "$ex:/run/wadspaces-extra:ro" "$set_ref" >/dev/null
for _ in $(seq 60); do "${P[@]}" logs "$box" 2>&1 | grep -q 'menu rebuilt\|menu generation' && break; sleep 1; done
"${P[@]}" logs "$box" 2>&1 | grep -E '\[wadspaces-(workspace|layout)\] (web apps|desktop laid|placed)' | sed 's/^/  /'
"${P[@]}" exec "$box" sh -c 'ls /config/Desktop' | sed 's/^/  Desktop: /'
"${P[@]}" exec "$box" sh -c 'test -f /usr/share/applications/wadspaces-webapp-github.desktop && cmp -s /run/wadspaces-extra/icons/github.png /usr/share/icons/hicolor/512x512/apps/wadspaces-webapp-github.png' ||
  fail "the web app's launcher or icon isn't as wadd made it"
echo "  GitHub's launcher, with the icon wadd made"
"${P[@]}" rm -f "$box" >/dev/null
box=""

echo "== the tidy-up and the disk page"
before=$("${P[@]}" images -q | sort | md5sum)
api -X POST http://wadd/v1/storage/prune | python3 -I -c 'import json, sys; f = json.load(sys.stdin); print("  prune removed %d image(s)" % len(f["removed"]))'
[ "$("${P[@]}" images --format '{{.Repository}}' | grep -c '^localhost/wadspaces-chain$')" -gt 0 ] || fail "the prune took the chains"
[ "$("${P[@]}" images --format '{{.Repository}}' | grep -c '^localhost/wadspaces-pkg-')" -gt 0 ] || fail "the prune took the packages"
[ "$before" = "$("${P[@]}" images -q | sort | md5sum)" ] && echo "  nothing it shouldn't"
api 'http://wadd/v1/storage?fresh=true' | python3 -I -c '
import json, sys
r = json.load(sys.stdin)
if r["leftovers"]: sys.exit("left over: %s" % [l["layer"] for l in r["leftovers"]])
for p in r["packages"]:
    print("  package %-22s %7.0f MB  for %d set(s)" % (p["name"], p["bytes"] / 1e6, len(p["sets"])))
for p in r["other"]:
    for i in p.get("items", []):
        if "wadspaces-" in i["label"] or i["label"].startswith(("The ", "Packages stacked")):
            sys.exit("a WadSpaces image under other images: " + i["label"])
print("  WadSpaces images %.2f GB (each layer once); nothing left over" % (r["imagesBytes"] / 1e9))'

echo "== disk"
"${P[@]}" system df | sed 's/^/  /'
echo "== report ($report)"
column -t -s $'\t' "$report" | sed 's/^/  /'
echo "ok"
