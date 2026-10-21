#!/usr/bin/env bash
# Tries projects for real on this laptop, as you: the wadspace clones and
# copies them in itself, but for one the user chose to mount. wadd (with your
# rootless podman) installs a wadspace whose projects are:
#   - a GitHub repo (octocat/Hello-World: public and tiny), cloned inside;
#   - a GitHub repo wadd had cloned on the machine (octocat/Spoon-Knife, with
#     an uncommitted file), which moves in instead of being cloned again;
#   - a copy made ahead (POST /v1/copies) of a folder here;
#   - a copy from before copies (a folder project's), made at launch;
#   - a folder here, mounted read-write (what's written inside lands here).
# Then: a restart fast-forwards a clean clone, leaves a changed one alone,
# and copies nothing again; a throwaway run clones and copies afresh.
#
#   apps/wadd/dev/try-projects.sh
#
# Its image is the base as it is (localhost/wadspaces-base:trixie, from
# images/build.sh --only base) with this checkout's images/base/root/ laid
# over it, so the image's side (wadspaces-projects) is the one here. Needs
# podman.socket and sway (see sway.sh); everything is removed afterwards.
set -euo pipefail
ulimit -c 0
root=$(cd "$(dirname "$0")/../../.." && pwd)
dir=$root/.build/wadd-try-projects
sock=$dir/wadd.sock
image=localhost/wadspaces-try-projects:latest
rm -rf "$dir"
mkdir -p "$dir/state" "$dir/projects" "$dir/signins" "$dir/home/Notes/drafts" "$dir/home/Photos" "$dir/home/Thesis"
echo "my notes" >"$dir/home/Notes/todo.md"
echo "chapter one" >"$dir/home/Notes/drafts/one.md"
echo "a photo" >"$dir/home/Photos/beach.jpg"
echo "chapter one" >"$dir/home/Thesis/one.md"
podman image exists localhost/wadspaces-base:trixie || { echo "no base: images/build.sh --only base" >&2; exit 1; }
[ -S "$XDG_RUNTIME_DIR/podman/podman.sock" ] || systemctl --user start podman.socket

echo "== the image: the base with this checkout's root/ over it"
cat >"$dir/Containerfile" <<'EOF'
FROM localhost/wadspaces-base:trixie
COPY root/ /
RUN chmod +x /usr/local/bin/* /etc/s6-overlay/s6-rc.d/*/run
LABEL io.wadspaces.projects="3"
EOF
podman build -q -t "$image" -f "$dir/Containerfile" "$root/images/base" >/dev/null

echo "== the clone wadd kept on this machine (from before), with work not committed"
git clone -q https://github.com/octocat/Spoon-Knife.git "$dir/projects/spoon"
echo "not committed yet" >"$dir/projects/spoon/wip.md"

. "$root/apps/wadd/dev/sway.sh"
start_sway "$dir"
cat >"$dir/wadd.toml" <<TOML
[machine]
name = "wadd-try-projects"
[daemon]
socket = "$sock"
state_dir = "$dir/state"
projects_dir = "$dir/projects"
signins_dir = "$dir/signins"
folder_roots = ["$dir/home"]
vendor_workspaces = "$dir/none.d"
ready_timeout_s = 120
[display]
socket = "$ipc"
TOML
echo '[]' >"$dir/state/workspaces.json"
(cd "$root" && cargo build -q -p wadd)
"$root/target/debug/wadd" serve --user --config "$dir/wadd.toml" >"$dir/wadd.log" 2>&1 &
wadd_pid=$!
finish() {
  kill "$wadd_pid" 2>/dev/null || true
  wait "$wadd_pid" 2>/dev/null || true
  for u in "$XDG_RUNTIME_DIR"/containers/systemd/wad-trypj*.container "$XDG_RUNTIME_DIR"/containers/systemd/wad-run-trypj*.container; do
    [ -e "$u" ] || continue
    systemctl --user stop "$(basename "$u" .container).service" 2>/dev/null || true
    rm -f "$u"
  done
  systemctl --user daemon-reload
  podman volume rm -f wad-trypj-config >/dev/null 2>&1 || true
  podman rmi -i "$image" >/dev/null 2>&1 || true
  stop_sway
}
trap finish EXIT
for _ in $(seq 50); do [ -S "$sock" ] && break; sleep 0.1; done
api() { curl -sS --unix-socket "$sock" -X "$1" "http://wadd$2"; echo; }
send() { curl -sS --unix-socket "$sock" -X "$1" -H 'Content-Type: application/json' --data-binary @- "http://wadd$2"; echo; }
fail() { echo "FAILED: $*"; grep -E "warn|error" "$dir/wadd.log" | tail -5; exit 1; }
inside() { podman exec -u abc "$1" sh -c "$2"; }
# The wadspace's own log lines about its projects, once it's done with them.
projects_log() {
  for _ in $(seq 120); do
    podman logs "$1" 2>&1 | grep -q "\[wadspaces-projects\].*$2" && break
    sleep 1
  done
  podman logs "$1" 2>&1 | grep "\[wadspaces-projects\]" | sed 's/^/  | /'
}

echo "== a copy made ahead, of Notes"
copy=$(printf '{"paths":["%s/home/Notes"]}' "$dir" | send POST /v1/copies)
echo "  $copy"
copy_id=$(python3 -I -c 'import json,sys; d=json.loads(sys.argv[1]); print(d.get("id") or sys.exit(str(d)))' "$copy")
printf '{"paths":["/etc/passwd"]}' | send POST /v1/copies | grep -q "isn't inside" || fail "a copy from outside the roots"

echo "== install the wadspace"
stack=$(cat <<JSON
{"version":4,"id":"trypj","owner":"me","sharedWith":[],"machine":"local","order":1,"templates":[],
 "customization":{"name":"Try projects","description":"","wallpaper":{"type":"texture","value":"stars"},"secrets":{"bySlot":{}},
  "projects":[
   {"id":"hello","kind":"github","repo":"octocat/Hello-World","as":"Hello"},
   {"id":"spoon","kind":"github","repo":"octocat/Spoon-Knife","as":"Spoon","setup":"echo set up > .setup-ran"},
   {"id":"$copy_id","kind":"copy","source":{"machineId":"local","paths":["$dir/home/Notes"]},"as":"Notes","files":2,"bytes":21},
   {"id":"photos","kind":"copy","source":{"machineId":"local","paths":["$dir/home/Photos"]},"as":"Photos"},
   {"id":"mount-thesis","kind":"mount","source":{"machineId":"local","paths":["$dir/home/Thesis"]},"as":"Thesis"}]},
 "createdAt":"t","updatedAt":"t"}
JSON
)
printf '{"wadspace":%s,"tz":"Etc/UTC"}' "$stack" | send PUT /v1/wadspaces/trypj >/dev/null
# Draw on the headless sway, not your session's display; on this image (it
# has no apps: the base's).
api GET /v1/workspaces/trypj | python3 -I -c '
import json, sys
w = json.load(sys.stdin)
w["env"] = [e for e in w["env"] if e[0] != "WADSPACES_WAYLAND"] + [["WADSPACES_WAYLAND", sys.argv[1] + "/wayland-1"]]
w["image"] = sys.argv[2]
print(json.dumps(w))' "$rt_name" "$image" | send PUT /v1/workspaces/trypj >/dev/null
grep -q "copies/hello:/run/wadspaces-copies/hello:ro" "$XDG_RUNTIME_DIR/containers/systemd/wad-trypj.container" ||
  fail "the unit doesn't mount the copies"
grep -q "/config/Desktop" "$XDG_RUNTIME_DIR/containers/systemd/wad-trypj.container" &&
  fail "the unit mounts a project before it starts"

launch() { # body; restart
  local id out
  id=$(send POST /v1/launches <<<"$1" | python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(d.get("id") or sys.exit(str(d)))')
  for _ in $(seq 1200); do
    out=$(curl -sS --unix-socket "$sock" "http://wadd/v1/launches/$id")
    grep -q '"status":"\(done\|error\|cancelled\)"' <<<"$out" && break
    sleep 0.1
  done
  python3 -I -c '
import json, sys
l = json.load(sys.stdin)
print("  status:", l["launch"]["status"], l["launch"]["error"] or "", "| ws:", l["launch"]["wsId"])
for p in l["launch"]["parts"]: print("  part", p["key"], p["state"], p["message"])
for line in l["lines"]: print("  |", line)' <<<"$out"
}

echo "== open it: the copy from before is made now; the kept clone moves in"
launch '{"kind":"workspace","workspace":"trypj"}'
[ -d "$dir/projects/spoon" ] && fail "the kept clone is still on the machine"
projects_log wad-trypj "Photos: copied in"
echo "== inside"
inside wad-trypj 'ls ~/Desktop; echo; head -1 ~/Desktop/Hello/README; cat ~/Desktop/Spoon/wip.md ~/Desktop/Notes/drafts/one.md ~/Desktop/Photos/beach.jpg'
inside wad-trypj 'cd ~/Desktop/Hello && git status --short --branch | head -1' || fail "Hello isn't a clone"
inside wad-trypj 'stat -c "%U %n" ~/Desktop/Hello/README ~/Desktop/Notes/todo.md'
grep -q "home/Thesis:/config/Desktop/Thesis:rw" "$XDG_RUNTIME_DIR/containers/systemd/wad-trypj.container" ||
  fail "the unit doesn't mount Thesis"
echo "== the mount: the machine's own folder, read-write both ways"
inside wad-trypj 'cat ~/Desktop/Thesis/one.md && echo "written inside" > ~/Desktop/Thesis/two.md'
[ "$(cat "$dir/home/Thesis/two.md")" = "written inside" ] || fail "what's written inside isn't on the machine"
[ "$(stat -c %u "$dir/home/Thesis/two.md")" = "$(id -u)" ] || fail "two.md isn't yours"
echo "edited here" >>"$dir/home/Thesis/one.md"
inside wad-trypj 'grep -q "edited here" ~/Desktop/Thesis/one.md' || fail "the machine's change isn't inside"
inside wad-trypj 'grep -qx Thesis ~/.wadspaces/mounted-projects' || fail "Thesis isn't remembered as a mount"
echo "  both ways"
for _ in $(seq 30); do inside wad-trypj 'test -f ~/Desktop/Spoon/.setup-ran' && break; sleep 1; done
inside wad-trypj 'test -f ~/Desktop/Spoon/.setup-ran' || fail "Spoon's setup didn't run"
echo "  Spoon's setup ran once"

echo "== restart: Hello a commit behind is fast-forwarded; Spoon (changed) is left; nothing is copied again"
inside wad-trypj 'cd ~/Desktop/Hello && git reset -q --hard HEAD~1 && rm ~/Desktop/Notes/todo.md'
api POST /v1/workspaces/trypj/stop >/dev/null
for _ in $(seq 60); do podman container exists wad-trypj || break; sleep 0.5; done
launch '{"kind":"workspace","workspace":"trypj"}' | grep -E "status|part"
projects_log wad-trypj "Spoon has uncommitted changes" | tail -6
inside wad-trypj 'cd ~/Desktop/Hello && git status --short --branch | head -1'
inside wad-trypj 'test ! -e ~/Desktop/Notes/todo.md' || fail "Notes was copied in again"
[ "$(stat -c %u "$dir/home/Thesis")" = "$(id -u)" ] || fail "Thesis was chowned at start"

echo "== a throwaway run: cloned and copied afresh, then gone"
run=$(launch '{"kind":"run","from":"trypj","overrides":{}}' | sed -n 's/.*| ws: \(run-trypj-[a-z0-9]*\).*/\1/p')
echo "  $run"
projects_log "wad-$run" "Photos: copied in" | tail -4
inside "wad-$run" 'ls ~/Desktop'
api POST "/v1/workspaces/$run/stop" >/dev/null

echo "== a copy in use can't be removed"
api DELETE "/v1/copies/$copy_id" | grep -q "still has it" || fail "a copy in use was removed"
api POST /v1/workspaces/trypj/stop >/dev/null
api DELETE /v1/workspaces/trypj >/dev/null
echo "== wadd's log"
grep -iE "warn|error" "$dir/wadd.log" | tail -5 || true
echo ok
