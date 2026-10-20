#!/usr/bin/env bash
# Packages and their assembly for real, on this laptop, in a podman store of
# its own (yours isn't touched). The trial's base is the built one with this
# repo's helpers on it (wadspaces-pkg, wadspaces-feature,
# wadspaces-vscode-extensions, the ABI file): the base the next
# images/build.sh makes. Packages are captured on it from the Containerfiles
# `cargo xtask packages containerfile` writes (wad-core's packages): by
# default electron-deps, vscode, cpp, rust, libresprite, tiled and two VS Code
# extensions; ALL=1 every package the catalog asks for (`packages waves`). A
# probe package carries a setuid file, a hardlink and a file capability.
#
# Then wadspace images are assembled from them and each case is timed, with
# the steps podman took from its cache and the disk it added:
#
#   s1-cold      the six and the probe, in WadSpaces' order (COPY --link)
#   s2-warm      the same again (podman's cache: it checks every source)
#   s3-remove    tiled (in the middle) taken out
#   s4-order     the six in a person's order (apps first, then tools)
#   s5-prefix    cpp and rust only: the bottom of s1
#   s6-base      s1 again on a changed base, --link
#   s7-nolink    s1 on that base, plain COPY
#   c1-chain     s1 on that base as chain images (wadd's way, wad-core's
#                packages::chain_ref): one image per prefix, tagged by its
#                packages, so a later image starts FROM the longest one and
#                nothing is checked again
#   c2-remove    tiled taken out, from c1's chain
#   c3-add       an extension added on top of c1's chain
#   s8-*         COPY --from by tag, by image id, by ARG
#
# s1 is checked inside (/ /usr /etc /tmp as the base has them, the
# merged-usr links, the probe's setuid bit, hardlink and capability, cargo's
# hardlinks, code, rustc, cmake, dpkg --audit, apt-get check) and c3 seeds its
# extension into a home. Then pulls through podman's API (libpod, libpod
# compatMode, docker's) show which reports bytes as they come.
# report.tsv has every number; captures.tsv each package's.
#
# On this laptop (2026-10-20): capturing the six took 960 s, assembling them
# 110-165 s (3.3 GB); podman's cache costs ~50 s just to check the sources;
# with chain images, taking tiled out took 14 s and adding an extension 7 s.
# --link reused nothing after a base change and copied slower than plain
# COPY, so wadd uses plain COPY.
#
#   apps/wadd/dev/try-packages.sh
#   ALL=1 apps/wadd/dev/try-packages.sh       every catalog package (long)
#   FRESH=1 apps/wadd/dev/try-packages.sh     from an empty store (captures again)
#   GITHUB_TOKEN=… apps/wadd/dev/try-packages.sh
#                                            captures get the token (GitHub's API)
#   PUSH=1 GITHUB_TOKEN=… apps/wadd/dev/try-packages.sh
#                                            also push cpp to ghcr.io/hunggod and
#                                            pull it back through the API with the token
#
# Needs localhost/wadspaces-base:trixie (images/build.sh --only base). The
# store is .build/try-packages/storage, kept between runs: its packages aren't
# captured again unless FRESH=1.
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
dir=$root/.build/try-packages
registry=${REGISTRY:-ghcr.io/hunggod}
podman image exists localhost/wadspaces-base:trixie || { echo "no base: images/build.sh --only base" >&2; exit 1; }
[ -n "${FRESH:-}" ] && [ -d "$dir" ] && podman unshare rm -rf "$dir"
mkdir -p "$dir/ctx" "$dir/logs"
helpers=$root/images/base/root/usr/local/bin
cp "$helpers"/wadspaces-pkg "$helpers"/wadspaces-feature "$helpers"/wadspaces-vscode-extensions "$dir/ctx/"
(cd "$root" && cargo build -q -p xtask)
xt() { "$root/target/debug/xtask" "$@"; }
P=(podman --root "$dir/storage" --runroot "$dir/run" --storage-driver overlay)
# The store sits in your home, which SELinux doesn't label for containers.
SEC=(--security-opt label=disable)
TOKEN=()
[ -n "${GITHUB_TOKEN:-}" ] && TOKEN=(--secret id=github_token,env=GITHUB_TOKEN)
report=$dir/report.tsv
captures=$dir/captures.tsv
printf 'scenario\tseconds\tcached\tsteps\tdisk_mb\tnote\n' >"$report"
printf 'package\timage\tmb\tfiles\tbase_changes\tversion\n' >"$captures"
svc_pid=""
finish() {
  [ -n "$svc_pid" ] && kill "$svc_pid" 2>/dev/null && wait "$svc_pid" 2>/dev/null
  true
}
trap finish EXIT
fail() { echo "FAILED: $*"; exit 1; }
disk() { podman unshare du -sb "$dir/storage/overlay" 2>/dev/null | cut -f1; }
now() { date +%s.%N; }

# One case: the command's time, steps from cache, disk added.
measure() {
  local name=$1 note=$2 log=$dir/logs/$1.log d0 t0 t1 d1 status=0
  shift 2
  d0=$(disk)
  t0=$(now)
  "$@" >"$log" 2>&1 || status=$?
  t1=$(now)
  d1=$(disk)
  local secs cached steps mb
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }')
  cached=$(grep -c "Using cache" "$log" || true)
  steps=$(grep -cE '(^|\] )STEP [0-9]+/' "$log" || true)
  mb=$(awk -v a="$d0" -v b="$d1" 'BEGIN { printf "%.0f", (b - a) / 1e6 }')
  [ "$status" = 0 ] || note="FAILED ($status): $note"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$secs" "$cached" "$steps" "$mb" "$note" >>"$report"
  printf '  %-12s %7s s  %2s/%-2s from cache  %+6s MB  %s\n' "$name" "$secs" "$cached" "$steps" "$mb" "$note"
  return "$status"
}

echo "== the base: the built one, with this repo's helpers"
if ! "${P[@]}" image exists localhost/wadspaces-base:orig; then
  graph=$(podman info --format '{{.Store.GraphRoot}}')
  runroot=$(podman info --format '{{.Store.RunRoot}}')
  podman unshare skopeo copy -q "containers-storage:[overlay@$graph+$runroot]localhost/wadspaces-base:trixie" \
    "containers-storage:[overlay@$dir/storage+$dir/run]localhost/wadspaces-base:orig"
fi
abi=$(sed -n 's/^ARG WADSPACES_ABI=//p' "$root/images/base/Dockerfile")
cat >"$dir/ctx/base.Containerfile" <<EOF
FROM localhost/wadspaces-base:orig
COPY wadspaces-pkg wadspaces-feature wadspaces-vscode-extensions /usr/local/bin/
RUN mkdir -p /usr/share/wadspaces && echo $abi > /usr/share/wadspaces/abi
LABEL io.wadspaces.abi="$abi"
EOF
"${P[@]}" build "${SEC[@]}" -q -t localhost/wadspaces-base:trixie -f "$dir/ctx/base.Containerfile" "$dir/ctx" >/dev/null
"${P[@]}" image inspect -f '  {{.Id}} {{index .Labels "build_version"}} abi {{index .Labels "io.wadspaces.abi"}}' localhost/wadspaces-base:trixie

# Earlier runs' wadspace images, chains and build cache go; packages stay.
"${P[@]}" images --format '{{.Repository}}:{{.Tag}}' | { grep '^localhost/try-' || true; } | xargs -r "${P[@]}" rmi -f >/dev/null
"${P[@]}" image prune -f >/dev/null

pkg() { echo "localhost/wadspaces-pkg-$1:try"; }
ref() { echo "localhost/wadspaces-pkg-$1:$("${P[@]}" image inspect -f '{{.Id}}' "$(pkg "$1")" | cut -c1-12)"; }

# A package: its Containerfile from wad-core, on the packages it needs.
package() {
  local id=$1 n args=()
  for n in $(xt packages needs "$id"); do args+=(--need "$n=$(ref "$n")"); done
  [ ${#TOKEN[@]} -gt 0 ] && args+=(--token)
  xt packages containerfile "$id" --base localhost/wadspaces-base:trixie "${args[@]}" >"$dir/ctx/pkg-$id.Containerfile"
  "${P[@]}" build "${SEC[@]}" "${TOKEN[@]}" --layers -t "$(pkg "$id")" -f "$dir/ctx/pkg-$id.Containerfile" "$dir/ctx"
}

# The probe: what packages must carry as they are, made by hand.
probe() {
  cat >"$dir/ctx/pkg-probe.Containerfile" <<'EOF'
FROM localhost/wadspaces-base:trixie AS build
RUN ["wadspaces-pkg", "capture", "probe", "--out", "/out", "--", "sh", "-c", "mkdir -p /opt/probe && cp /usr/bin/true /opt/probe/suid && chmod 4755 /opt/probe/suid && ln /opt/probe/suid /opt/probe/link && cp /usr/bin/true /opt/probe/cap && python3 -c 'import os; os.setxattr(\"/opt/probe/cap\", \"security.capability\", bytes.fromhex(\"0100000200200000000000000000000000000000\"))'"]
FROM scratch
COPY --from=build /out/ /
LABEL io.wadspaces.pkg="probe"
EOF
  "${P[@]}" build "${SEC[@]}" --layers -t "$(pkg probe)" -f "$dir/ctx/pkg-probe.Containerfile" "$dir/ctx"
}

if [ -n "${ALL:-}" ]; then
  mapfile -t ids < <(xt packages waves | python3 -I -c 'import json, sys; print("\n".join(i for w in json.load(sys.stdin) for i in w))')
else
  ids=(electron-deps cpp rust vscode libresprite tiled ext-rust-lang.rust-analyzer ext-ms-vscode.cmake-tools)
fi
ids+=(probe)
echo "== packages (captured once; FRESH=1 captures again)"
failed=()
for id in "${ids[@]}"; do
  if ! "${P[@]}" image exists "$(pkg "$id")"; then
    how=package
    [ "$id" = probe ] && how=probe
    if ! measure "pkg-$id" "captured" "$how" "$id"; then
      grep '\[wadspaces-pkg\]' "$dir/logs/pkg-$id.log" | tail -8 | sed 's/^/    /'
      failed+=("$id")
      [ -n "${ALL:-}" ] || fail "packaging $id"
      continue
    fi
    grep '\[wadspaces-pkg\]' "$dir/logs/pkg-$id.log" >"$dir/capture-$id.txt" || true
  fi
  pid=$("${P[@]}" image inspect -f '{{.Id}}' "$(pkg "$id")")
  "${P[@]}" tag "$(pkg "$id")" "$(ref "$id")"
  size=$("${P[@]}" image inspect -f '{{.Size}}' "$(pkg "$id")")
  files=$(sed -n 's/.*: \([0-9]*\) files, .*/\1/p' "$dir/capture-$id.txt" 2>/dev/null | tail -1)
  changes=$(grep -c 'WARN' "$dir/capture-$id.txt" 2>/dev/null || true)
  version=$(sed -n 's/.*, version \(.*\)$/\1/p' "$dir/capture-$id.txt" 2>/dev/null | tail -1)
  mb=$(awk -v s="$size" 'BEGIN { printf "%.0f", s / 1e6 }')
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "${pid:0:12}" "$mb" "${files:-?}" "$changes" "$version" >>"$captures"
  printf '  %-30s %s %6s MB %6s files  %s\n' "$id" "${pid:0:12}" "$mb" "${files:-?}" "$version"
done
[ ${#failed[@]} -eq 0 ] || echo "  FAILED to package: ${failed[*]} (logs/pkg-<id>.log)"

# An assembly's Containerfile: the base, each package (in the order given),
# then assemble.
assembly_file() {
  local link=$1 p
  shift
  echo "ARG BASE_IMAGE=localhost/wadspaces-base:trixie"
  echo 'FROM ${BASE_IMAGE}'
  for p in "$@"; do echo "COPY ${link:+--link }--from=$(ref "$p") / /"; done
  echo "RUN wadspaces-pkg assemble"
}
assemble() {
  local name=$1 note=$2 link=$3
  shift 3
  assembly_file "$link" "$@" >"$dir/ctx/$name.Containerfile"
  measure "$name" "$note" "${P[@]}" build "${SEC[@]}" --layers -t "localhost/try-$name" -f "$dir/ctx/$name.Containerfile" "$dir/ctx"
}

# Chain images: one per prefix of packages, tagged by the base and the
# packages' image ids, so an image starts FROM the longest stored one and
# copies only what's left; then assemble on top.
chain_tag() {
  local key p
  key=$("${P[@]}" image inspect -f '{{.Id}}' localhost/wadspaces-base:trixie)
  for p in "$@"; do key+="|$(ref "$p")"; done
  echo "localhost/try-chain:$(printf '%s' "$key" | sha256sum | cut -c1-12)"
}
chain() {
  local name=$1 from=localhost/wadspaces-base:trixie p tag prefix=()
  shift
  for p in "$@"; do
    prefix+=("$p")
    tag=$(chain_tag "${prefix[@]}")
    if "${P[@]}" image exists "$tag"; then
      echo "--> Using cache: chain to $p"
    else
      echo "STEP ${#prefix[@]}/$#: chain to $p"
      printf 'FROM %s\nCOPY --from=%s / /\n' "$from" "$(ref "$p")" >"$dir/ctx/chain.Containerfile"
      "${P[@]}" build "${SEC[@]}" --layers=false -q -t "$tag" -f "$dir/ctx/chain.Containerfile" "$dir/ctx" || return 1
    fi
    from=$tag
  done
  printf 'FROM %s\nRUN wadspaces-pkg assemble\n' "$from" >"$dir/ctx/$name.Containerfile"
  "${P[@]}" build "${SEC[@]}" --layers=false -t "localhost/try-$name" -f "$dir/ctx/$name.Containerfile" "$dir/ctx"
}

canonical=(cpp rust electron-deps vscode tiled libresprite)
person=(electron-deps vscode libresprite tiled cpp rust)
echo "== assemblies"
assemble s1-cold "the six and the probe, WadSpaces' order" 1 "${canonical[@]}" probe
assemble s2-warm "the same again" 1 "${canonical[@]}" probe
assemble s3-remove "tiled taken out" 1 cpp rust electron-deps vscode libresprite probe
assemble s4-order "a person's order" 1 "${person[@]}"
assemble s5-prefix "cpp + rust: s1's bottom" 1 cpp rust

echo "== s1, inside"
"${P[@]}" run "${SEC[@]}" --rm --entrypoint sh localhost/wadspaces-base:trixie -c 'stat -c "%n %a %u %g" / /usr /etc /tmp /var/tmp' >"$dir/base-stat.txt"
"${P[@]}" run "${SEC[@]}" --rm --entrypoint sh localhost/try-s1-cold -c 'stat -c "%n %a %u %g" / /usr /etc /tmp /var/tmp' >"$dir/s1-stat.txt"
diff "$dir/base-stat.txt" "$dir/s1-stat.txt" && echo "  / /usr /etc /tmp /var/tmp as the base has them" || fail "the packages changed the base's folders"
"${P[@]}" run "${SEC[@]}" --rm --entrypoint bash localhost/try-s1-cold -c '
  set -e
  for d in /bin /lib /lib64 /sbin; do [ -L "$d" ] || { echo "$d is not a link any more"; exit 1; }; done
  echo "  merged-usr links kept"
  [ "$(stat -c %a /opt/probe/suid)" = 4755 ] || { echo "the probe lost its setuid bit ($(stat -c %a /opt/probe/suid))"; exit 1; }
  [ "$(stat -c %h /opt/probe/suid)" = 2 ] || { echo "the probe lost its hardlink"; exit 1; }
  python3 -c "import os; os.getxattr(\"/opt/probe/cap\", \"security.capability\")" || { echo "the probe lost its capability"; exit 1; }
  echo "  setuid bit, hardlink and file capability kept"
  [ "$(stat -c %h /usr/local/cargo/bin/cargo)" -gt 1 ] && echo "  cargo hardlinked $(stat -c %h /usr/local/cargo/bin/cargo)x" || echo "  cargo: no hardlinks (rustup chose copies)"
  echo "  code $(code --version --no-sandbox --user-data-dir /tmp/v | head -1)"
  echo "  $(rustc --version)"
  echo "  $(cmake --version | head -1)"
  [ -x /opt/tiled/AppRun ] && [ -x /opt/libresprite/AppRun ] && echo "  tiled + libresprite there"
  [ -z "$(dpkg --audit)" ] || { dpkg --audit; exit 1; }
  echo "  dpkg --audit: clean"
  apt-get check -q >/dev/null && echo "  apt-get check: ok"
  [ "$(dpkg-query -W -f="\${Status}" cmake)" = "install ok installed" ] || { echo "dpkg has no cmake"; exit 1; }
  echo "  dpkg knows what the packages installed (cmake: installed)"
' || fail "s1 isn't right inside"

echo "== a changed base"
printf 'FROM localhost/wadspaces-base:trixie\nRUN date > /etc/wadspaces-try-base\n' >"$dir/ctx/base2.Containerfile"
"${P[@]}" build "${SEC[@]}" -q -t localhost/wadspaces-base:trixie -f "$dir/ctx/base2.Containerfile" "$dir/ctx" >/dev/null
assemble s6-base "s1 on a changed base, --link" 1 "${canonical[@]}" probe
assemble s7-nolink "s1 on that base, plain COPY" "" "${canonical[@]}" probe
measure c1-chain "s1 on that base, as chain images" chain c1-chain "${canonical[@]}" probe
measure c2-remove "tiled taken out, from c1's chain" chain c2-remove cpp rust electron-deps vscode libresprite probe
measure c3-add "an extension on c1's chain" chain c3-add "${canonical[@]}" probe ext-rust-lang.rust-analyzer
"${P[@]}" run "${SEC[@]}" --rm --entrypoint bash localhost/try-c3-add -c '
  set -e
  mkdir -p /tmp/h && wadspaces-vscode-extensions --seed /tmp/h >/dev/null
  ls /tmp/h/.vscode/extensions | grep -qi "^rust-lang.rust-analyzer-" || { ls /tmp/h/.vscode/extensions; exit 1; }
  echo "  c3 seeds rust-analyzer into a home"' || fail "c3 didn't seed its extension"

echo "== COPY --from forms"
cpp=$(ref cpp)
cpp_id=$("${P[@]}" image inspect -f '{{.Id}}' "$cpp")
from_case() {
  local name=$1 note=$2
  measure "$name" "$note" "${P[@]}" build "${SEC[@]}" --layers -t "localhost/try-$name" -f "$dir/ctx/$name.Containerfile" "$dir/ctx" || true
}
printf 'FROM localhost/wadspaces-base:trixie\nCOPY --link --from=%s / /\n' "$cpp" >"$dir/ctx/s8-tag.Containerfile"
from_case s8-tag "by tag"
printf 'FROM localhost/wadspaces-base:trixie\nCOPY --link --from=%s / /\n' "$cpp_id" >"$dir/ctx/s8-id.Containerfile"
from_case s8-id "by image id"
printf 'ARG PKG_CPP=%s\nFROM localhost/wadspaces-base:trixie\nARG PKG_CPP\nCOPY --link --from=${PKG_CPP} / /\n' "$cpp" >"$dir/ctx/s8-arg.Containerfile"
from_case s8-arg "by ARG"

echo "== disk"
"${P[@]}" system df | sed 's/^/  /'

echo "== pulls through the API"
"${P[@]}" system service --time=0 "unix://$dir/podman.sock" >"$dir/logs/service.log" 2>&1 &
svc_pid=$!
for _ in $(seq 50); do [ -S "$dir/podman.sock" ] && break; sleep 0.1; done
pull_case() {
  local name=$1 url=$2 auth=${3:-}
  "${P[@]}" rmi -f docker.io/library/debian:trixie-slim >/dev/null 2>&1 || true
  curl -sS --unix-socket "$dir/podman.sock" -X POST ${auth:+-H "X-Registry-Auth: $auth"} "http://d$url" >"$dir/pull-$name.json"
  python3 -I - "$dir/pull-$name.json" "$name" <<'PY'
import json, sys
text = open(sys.argv[1]).read()
events = []
dec = json.JSONDecoder()
i = 0
while i < len(text):
    while i < len(text) and text[i].isspace():
        i += 1
    if i >= len(text):
        break
    obj, i = dec.raw_decode(text, i)
    events.append(obj)
keys = sorted({k for e in events if isinstance(e, dict) for k in e})
detail = [e for e in events if isinstance(e, dict) and (e.get("progressDetail") or {}).get("total")]
print(f"  {sys.argv[2]:<12} {len(events)} events, keys {keys}; {len(detail)} with bytes")
PY
}
pull_case libpod "/v5.0.0/libpod/images/pull?reference=docker.io/library/debian:trixie-slim&policy=always"
pull_case compat-mode "/v5.0.0/libpod/images/pull?reference=docker.io/library/debian:trixie-slim&policy=always&compatMode=true"
pull_case docker "/v1.41/images/create?fromImage=docker.io/library/debian&tag=trixie-slim"

if [ -n "${PUSH:-}" ]; then
  [ -n "${GITHUB_TOKEN:-}" ] || fail "PUSH=1 needs GITHUB_TOKEN"
  echo "== push cpp to $registry, pull it back with the token"
  remote=$registry/wadspaces-pkg-try-cpp:try
  "${P[@]}" push -q --creds "wadspaces:$GITHUB_TOKEN" "$cpp" "docker://$remote"
  "${P[@]}" rmi -f "$remote" >/dev/null 2>&1 || true
  auth=$(python3 -I -c 'import base64, json, os, sys
print(base64.urlsafe_b64encode(json.dumps({"username": "wadspaces", "password": os.environ["GITHUB_TOKEN"], "serveraddress": sys.argv[1]}).encode()).decode())' "${registry%%/*}")
  curl -sS --unix-socket "$dir/podman.sock" -X POST -H "X-Registry-Auth: $auth" \
    "http://d/v5.0.0/libpod/images/pull?reference=$remote&policy=always&compatMode=true" >"$dir/pull-ghcr.json"
  "${P[@]}" image inspect -f '  pulled {{.Id}}' "$remote" || fail "the pull with the token didn't work: $(tail -c 300 "$dir/pull-ghcr.json")"
  [ "$("${P[@]}" image inspect -f '{{.Id}}' "$remote")" = "$cpp_id" ] && echo "  same image id as here (pin by id works)"
fi

echo "== packages: $captures"
column -t -s $'\t' "$captures" | sed 's/^/  /'
echo "== report: $report"
column -t -s $'\t' "$report" | sed 's/^/  /'
[ ${#failed[@]} -eq 0 ] || fail "packaging ${failed[*]}"
echo ok
