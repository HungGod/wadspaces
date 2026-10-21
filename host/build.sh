#!/usr/bin/env bash
# Build the WadSpaces host image, and optionally a bootable disk.
#
#   host/build.sh image            container image localhost/wadspaces-host:latest
#   host/build.sh qcow2 | iso      image + disk via bootc-image-builder (sudo)
#   host/build.sh install [/dev/sdX]
#                                  image + `bootc install to-disk` onto that drive
#                                  (WIPES it; sudo; refuses the disk holding /),
#                                  with the base images (--no-bases: without)
#   host/build.sh update [/dev/sdX]
#                                  image, copied onto an already installed drive
#                                  in place: only changed layers are written, and
#                                  the drive switches to it on its next boot (sudo)
#
# With no drive named, install and update use the one USB drive plugged in.
# sudo asks for the password once, at the start, and stays fresh until the
# end (a drive copy takes longer than sudo remembers a password). The drive's
# partitions are unmounted first if the desktop mounted them.
#
#   --bases                        with update: also copy the base images
#                                  (localhost/wadspaces-base:trixie, and the stream
#                                  sidecar localhost/wadspaces-stream:trixie, built
#                                  by images/build.sh) onto the drive; the machine
#                                  builds templates on the base, and streams draw
#                                  on the sidecar. install copies them unless
#                                  --no-bases
#   --stage-only DIR               with update: write what would go onto the
#                                  drive into DIR instead (for testing)
#   --full                         with update: copy every layer of the system
#                                  image, even those the drive already has
#                                  (when an update said a layer was missing)
#
# The image is rechunked after it's built (rpm-ostree: a layer per package
# group, WadSpaces' own files in one of their own), so a drive update copies
# only what changed. RECHUNK=0 skips it (quicker, for trying things).
#
# The WadSpaces Client (apps/client, GTK4: the kiosk's shell) goes into the
# image at /usr/bin/client, its GitHub sign-in window (apps/signin) beside it.
# wadd (apps/wadd, the machine daemon) and the HUD (apps/hud: the bar, power
# and Wi-Fi menus and the switcher above every window) are built here too,
# into /usr/bin/wadd and /usr/libexec/wadspaces/hud. So is WadBrowser
# (apps/wadbrowser), the machine's own browser app, into /usr/bin/wadbrowser.
# They need Rust (rustup), gtk4-devel, gtk4-layer-shell-devel and the
# WebKitGTK dev packages.
#
# Baked-in settings and secrets come from host/secrets/ (gitignored, never in
# the build context directly):
#   firebase.toml          the account's Firebase project, the web app's config
#                          (api_key, project_id; required)
#   admin_password_hash    crypt hash for `admin` (default: the one in bib-config.toml)
#   cloudflare.toml        [cloudflare.turn] token_id, api_token: Cloudflare TURN,
#                          for streams' WebRTC (only that table is baked in)
# and the Cloudflare tunnel's token from this machine's /etc/cloudflared/token
# (read with sudo if need be), handed to the build as a secret: it's baked
# into the image's /etc/cloudflared/token and nowhere else.
# No GitHub token is baked in any more: a machine gets its token when you sign
# in to WadSpaces Client with GitHub, or from the account.
# ADMIN_SSH_KEY (default: the key in bib-config.toml) is admin's SSH key.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-localhost/wadspaces-host:latest}"
SECRETS_DIR="${ROOT}/host/secrets"
BIB_CONFIG="${ROOT}/host/bib-config.toml"
TARGET="${1:-image}"
DISK=""
BASES=""
STAGE_ONLY=""
FULL=""
# The background job keeping sudo fresh, and a drive mounted for a copy:
# cleanup() undoes both however the script ends.
SUDO_KEEPALIVE=""
MNT=""
[[ $# -gt 0 ]] && shift
if [[ "${TARGET}" == "install" || "${TARGET}" == "update" ]] && [[ $# -gt 0 && "$1" != --* ]]; then
    DISK="$1"; shift
fi
while [[ $# -gt 0 ]]; do
    case "$1" in
        --bases) BASES=1; shift ;;
        --no-bases) BASES=0; shift ;;
        --stage-only) STAGE_ONLY="$2"; shift 2 ;;
        --full) FULL=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
STICK_OCI="${ROOT}/.build/stick/oci"
# A wiped drive has no base image: install brings it unless told not to.
if [[ "${TARGET}" == "install" ]]; then BASES="${BASES:-1}"; fi
[[ "${BASES}" == "0" ]] && BASES=""

cleanup() {
    if [[ -n "${MNT}" ]]; then
        sudo umount "${MNT}" 2>/dev/null
        rmdir "${MNT}" 2>/dev/null
    fi
    if [[ -n "${SUDO_KEEPALIVE}" ]]; then
        kill "${SUDO_KEEPALIVE}" 2>/dev/null
    fi
    true
}
trap cleanup EXIT

# Ask for the sudo password now, once, and keep it fresh while this script
# runs: builds and a drive copy outlast sudo's few minutes.
sudo_once() {
    [[ $EUID -eq 0 ]] && return
    echo ">> sudo is needed for ${1}: asking once, now"
    sudo -v
    ( while kill -0 "$$" 2>/dev/null; do sudo -n -v 2>/dev/null; sleep 50; done ) &
    SUDO_KEEPALIVE=$!
}

# A binary into the image without its symbols (the release profile keeps
# them); a copy that has them stays in .build/debug for backtraces.
install_stripped() {
    local name; name="$(basename "$2")"
    mkdir -p "${ROOT}/.build/debug"
    objcopy --only-keep-debug "$1" "${ROOT}/.build/debug/${name}.debug"
    install -s -m 755 "$1" "$2"
}

# Built here rather than in a container: this laptop runs Fedora 43 like the
# image, so the apps link against the same GTK and WebKitGTK. The image build
# runs `client --version` to check that it loads. The account's settings are
# the web app's (public) Firebase config, from host/secrets/firebase.toml.
stage_client() {
    local out="${ROOT}/.build/client" firebase="${SECRETS_DIR}/firebase.toml"
    rm -rf "${out}" && mkdir -p "${out}"
    if ! command -v cargo > /dev/null && [[ -f "${HOME}/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env"
    fi
    command -v cargo > /dev/null || { echo "cargo not found: install Rust with rustup" >&2; exit 1; }
    grep -q '^api_key = ".\+"' "${firebase}" 2> /dev/null || {
        echo "no ${firebase} with an api_key: WadSpaces Client needs the account's Firebase" >&2
        echo "web config (Firebase console > Project settings > Your apps > Web app):" >&2
        echo '  api_key = "..."' >&2
        echo '  project_id = "..."' >&2
        exit 1
    }
    echo ">> building WadSpaces Client (apps/client, apps/signin)"
    (cd "${ROOT}" && cargo build --release --quiet -p client -p client-signin)
    install_stripped "${ROOT}/target/release/client" "${out}/client"
    install_stripped "${ROOT}/target/release/client-signin" "${out}/client-signin"
    install -m 644 "${firebase}" "${out}/firebase.toml"
}

stage_wadd() {
    local out="${ROOT}/.build/wadd"
    rm -rf "${out}" && mkdir -p "${out}"
    echo ">> building wadd (apps/wadd)"
    (cd "${ROOT}" && cargo build --release --quiet -p wadd)
    install_stripped "${ROOT}/target/release/wadd" "${out}/wadd"
}

stage_hud() {
    local out="${ROOT}/.build/hud"
    rm -rf "${out}" && mkdir -p "${out}"
    echo ">> building the HUD (apps/hud)"
    (cd "${ROOT}" && cargo build --release --quiet -p wadspaces-hud)
    install_stripped "${ROOT}/target/release/wadspaces-hud" "${out}/hud"
}

# WadBrowser for the machine itself (an app wadd runs, like the terminal:
# host/usr/lib/wadspaces/workspaces.d/browser.toml). Built here for the same
# reason as the client; the wadspace images get theirs from images/build.sh.
stage_wadbrowser() {
    local out="${ROOT}/.build/wadbrowser"
    rm -rf "${out}" && mkdir -p "${out}"
    echo ">> building WadBrowser (apps/wadbrowser)"
    (cd "${ROOT}" && cargo build --profile wadbrowser --quiet -p wadbrowser --features host)
    install -m 755 "${ROOT}/target/wadbrowser/wadbrowser" "${out}/wadbrowser"
}

# The pictures wadspaces' desktops show (wad-core's catalog icons, the brand,
# the folder) and the client's wallpapers, from assets/public, and the fonts.
stage_assets() {
    local out="${ROOT}/.build/assets"
    rm -rf "${out}" && mkdir -p "${out}"
    cp -r "${ROOT}"/assets/public/. "${out}/"
    mkdir -p "${out}/fonts" && cp "${ROOT}"/assets/fonts/* "${out}/fonts/"
}

# Where host/etc and host/usr's files land in the image: the Containerfile
# marks them (and the binaries) as WadSpaces' own chunk.
stage_ours() {
    (cd "${ROOT}/host" && find etc usr -type f | sed 's|^|/|' | sort) > "${ROOT}/.build/ours.list"
}

# The built image in chunks by package (rpm-ostree), WadSpaces' own files in
# one of their own: an update copies only the chunks that changed. Rootless:
# the image is mounted into a fedora-bootc container that writes an OCI
# directory, decompressed on its way into podman's storage so its layers
# keep the digests a stick update names them by. RECHUNK=0: as built.
rechunk() {
    local raw="${IMAGE%:*}:unchunked" out="${ROOT}/.build/chunked"
    if [[ "${RECHUNK:-1}" == "0" ]]; then
        podman tag "${raw}" "${IMAGE}"
        return
    fi
    echo ">> rechunking ${IMAGE} (a few minutes)"
    rm -rf "${out}" && mkdir -p "${out}/oci/blobs/sha256"
    echo '{"imageLayoutVersion":"1.0.0"}' > "${out}/oci/oci-layout"
    echo '{"schemaVersion":2,"manifests":[]}' > "${out}/oci/index.json"
    podman run --rm --privileged --security-opt label=disable \
        --mount "type=image,source=${raw},destination=/rootfs" -v "${out}:/out" \
        quay.io/fedora/fedora-bootc:43 \
        rpm-ostree compose build-chunked-oci --bootc --format-version=1 --max-layers 64 \
        --rootfs /rootfs --output oci:/out/oci:host > "${out}.log" 2>&1 \
        || { tail -20 "${out}.log" >&2; exit 1; }
    skopeo copy -q --dest-decompress "oci:${out}/oci:host" "dir:${out}/dir"
    skopeo copy -q "dir:${out}/dir" "containers-storage:${IMAGE}"
    rm -rf "${out}"
}

# admin's password hash or SSH key from bib-config.toml ("password" / "key").
bib_admin() {
    python3 - "${BIB_CONFIG}" "$1" <<'EOF'
import sys, tomllib
with open(sys.argv[1], "rb") as f:
    users = tomllib.load(f).get("customizations", {}).get("user", [])
print(next((u.get(sys.argv[2], "") for u in users if u.get("name") == "admin"), ""))
EOF
}

stage_secrets() {
    local out="${ROOT}/.build/secrets"
    rm -rf "${out}" && (umask 077 && mkdir -p "${out}")
    local hash
    if [[ -f "${SECRETS_DIR}/admin_password_hash" ]]; then
        hash="$(< "${SECRETS_DIR}/admin_password_hash")"
    else
        hash="$(bib_admin password)"
    fi
    # bib-config.toml may hold the password itself rather than a crypt hash
    # (bootc-image-builder takes either); usermod -p needs the hash. The salt
    # comes from the password so the same password gives the same hash on
    # every build, and the machine only re-applies it when it changes.
    if [[ -n "${hash}" && "${hash}" != \$*\$*\$* ]]; then
        local salt
        salt="$(printf 'wadspaces-admin:%s' "${hash}" | sha256sum | cut -c1-16)"
        hash="$(printf '%s' "${hash}" | openssl passwd -6 -salt "${salt}" -stdin)"
    fi
    if [[ -n "${hash}" ]]; then
        (umask 077 && printf '%s' "${hash}" > "${out}/admin_password_hash")
    else
        echo ">> WARNING: no admin password hash; admin can only log in with the SSH key"
    fi

    # Streams' WebRTC (off unless wadd.toml says): Cloudflare TURN's key, the
    # [cloudflare.turn] table alone (the file's other tokens stay here).
    local turn="${SECRETS_DIR}/cloudflare.toml"
    if [[ -f "${turn}" ]]; then
        (umask 077 && python3 - "${turn}" > "${out}/cloudflare.toml" <<'PY'
import json, sys, tomllib
t = tomllib.load(open(sys.argv[1], "rb")).get("cloudflare", {}).get("turn", {})
if t.get("token_id") and t.get("api_token"):
    print("[cloudflare.turn]")
    print("token_id = " + json.dumps(t["token_id"]))
    print("api_token = " + json.dumps(t["api_token"]))
PY
        )
        [[ -s "${out}/cloudflare.toml" ]] || { rm -f "${out}/cloudflare.toml"; echo ">> WARNING: ${turn} has no [cloudflare.turn] key"; }
    fi

    local ssh="${ROOT}/.build/ssh"
    rm -rf "${ssh}" && mkdir -p "${ssh}"
    if [[ -n "${ADMIN_SSH_KEY:-}" ]]; then
        cp "${ADMIN_SSH_KEY}" "${ssh}/admin"
    else
        bib_admin key > "${ssh}/admin"
    fi
    [[ -s "${ssh}/admin" ]] || echo ">> WARNING: no admin SSH key"
}

# The whole-disk device behind /, so `install` can refuse it.
root_disk() {
    local src
    src="$(findmnt -no SOURCE -T / | sed 's/\[.*//')"
    lsblk -ndo PKNAME "${src}" 2>/dev/null | sed 's|^|/dev/|' || true
}

# No drive named: the one USB drive plugged in.
pick_usb_disk() {
    local usb
    mapfile -t usb < <(lsblk -dnpo NAME,TRAN,TYPE | awk '$2 == "usb" && $3 == "disk" { print $1 }')
    case ${#usb[@]} in
        0) echo "no USB drive plugged in: plug one in, or name it (host/build.sh ${TARGET} /dev/sdX)" >&2; exit 2 ;;
        1) DISK="${usb[0]}"; echo ">> using the USB drive ${DISK}" ;;
        *) echo "more than one USB drive: name one (host/build.sh ${TARGET} /dev/sdX)" >&2
           lsblk -o NAME,SIZE,MODEL,TRAN "${usb[@]}" >&2; exit 2 ;;
    esac
}

check_disk() {
    [[ -n "${DISK}" ]] || pick_usb_disk
    [[ -b "${DISK}" ]] || { echo "${DISK} is not a block device" >&2; exit 2; }
    [[ "$(lsblk -ndo TYPE "${DISK}")" == "disk" ]] || { echo "${DISK} is not a whole disk" >&2; exit 2; }
    if [[ "$(realpath "${DISK}")" == "$(realpath "$(root_disk)" 2>/dev/null)" ]]; then
        echo "${DISK} holds this system's root filesystem; refusing" >&2; exit 2
    fi
}

# The desktop mounts a drive's partitions when it's plugged in: unmount them
# (needs sudo_once first).
unmount_disk() {
    local p
    for p in $(lsblk -nrpo NAME,MOUNTPOINTS "${DISK}" | awk 'NF > 1 { print $1 }'); do
        echo ">> unmounting ${p}"
        sudo umount --all-targets "${p}"
    done
}

check_install_target() {
    check_disk
    lsblk -o NAME,SIZE,MODEL,TRAN "${DISK}"
    read -r -p "Everything on ${DISK} will be erased. Type the device path to continue: " answer
    [[ "${answer}" == "${DISK}" ]] || { echo "aborted"; exit 1; }
}

# The WadSpaces root partition (label "root", xfs) on an installed drive.
root_partition() {
    lsblk -nrpo NAME,FSTYPE,LABEL "${DISK}" | awk '$2 == "xfs" && $3 == "root" { print $1; exit }'
}

check_update_target() {
    check_disk
    [[ -n "$(root_partition)" ]] || {
        echo "${DISK} has no WadSpaces root partition; use install first" >&2; exit 2; }
    lsblk -o NAME,SIZE,MODEL,TRAN,LABEL "${DISK}"
}

# ------------------------------------------------------------ drive updates
# What goes onto the drive: an OCI directory holding the host image ("host")
# and base images ("base-<name>"), with layers left
# uncompressed, so a file name is its content digest and stays the same in every
# build that didn't change it.
# The system image's layers the drive's system already holds aren't copied
# at all: bootc reads only the layers it lacks (the drive's ostree repo names
# each one it has, by digest). On the drive, wadspaces-import.service
# (host/usr/libexec/wadspaces/import-updates) imports them at the next boot.

stage_stick() {
    rm -rf "${STICK_OCI}" && mkdir -p "${STICK_OCI}"
    : > "${STICK_OCI}/import.list"
    if [[ "${TARGET}" == "update" ]]; then
        echo ">> staging ${IMAGE} for the drive"
        skopeo copy -q --dest-oci-accept-uncompressed-layers "containers-storage:${IMAGE}" "oci:${STICK_OCI}:host"
        podman image inspect --format '{{.Id}}' "${IMAGE}" > "${STICK_OCI}/host.id"
    fi
    # Bases keep their own names: wadd builds on them as they are.
    local b
    local b src
    for b in ${BASES:+base stream}; do
        src="localhost/wadspaces-${b}:trixie"
        podman image exists "${src}" || { echo "${src} is not built: images/build.sh --only ${b}" >&2; exit 1; }
        echo ">> staging ${src}"
        skopeo copy -q --dest-oci-accept-uncompressed-layers "containers-storage:${src}" "oci:${STICK_OCI}:base-${b}"
        echo "base-${b} ${src}" >> "${STICK_OCI}/import.list"
    done
    du -sh "${STICK_OCI}" | awk '{print ">> staged " $1}'
}

# The staged system image's layers the drive (its root mounted at $1)
# already holds, as rsync excludes: ostree keeps a ref per layer it imported,
# ostree/container/blob/sha256_3A_<hex>. A layer another staged image needs
# too is copied all the same (podman reads those whole).
held_layers() {
    local refs="$1/ostree/repo/refs/heads/ostree/container/blob"
    sudo test -d "${refs}" || return 0
    sudo ls "${refs}" | python3 -c '
import json, os, sys
oci = sys.argv[1]
have = {n.strip().removeprefix("sha256_3A_") for n in sys.stdin}
def read(d):
    return json.load(open(os.path.join(oci, "blobs", *d.split(":", 1))))
host, others = set(), set()
for m in json.load(open(os.path.join(oci, "index.json")))["manifests"]:
    layers = {l["digest"] for l in read(m["digest"])["layers"]}
    is_host = m.get("annotations", {}).get("org.opencontainers.image.ref.name") == "host"
    (host if is_host else others).update(layers)
held = sorted(d.split(":", 1)[1] for d in host - others if d.split(":", 1)[1] in have)
print(f">> {len(held)} of {len(host)} system layers already on the drive", file=sys.stderr)
for h in held:
    print(f"/blobs/sha256/{h}")
' "${STICK_OCI}"
}

# Mount the drive's root, mirror the staged directory into its /var, mark it.
push_to_drive() {
    local part var
    part="$(root_partition)"
    MNT="$(mktemp -d)"
    sudo mount "${part}" "${MNT}"
    var="$(sudo sh -c "ls -d '${MNT}'/ostree/deploy/*/var" | head -n1)"
    [[ -n "${var}" ]] || { echo "no ostree deployment on ${part}" >&2; exit 1; }
    local skip
    skip="$(mktemp)"
    if [[ -z "${FULL}" ]]; then
        held_layers "${MNT}" > "${skip}"
    fi
    echo ">> copying changed layers to ${DISK}"
    sudo mkdir -p "${var}/lib/wadspaces/incoming"
    sudo rsync -rt --delete --exclude-from="${skip}" --info=progress2 "${STICK_OCI}/" "${var}/lib/wadspaces/incoming/"
    rm -f "${skip}"
    sudo touch "${var}/lib/wadspaces/incoming/pending"
    echo ">> flushing to the drive"
    sync
    sudo umount "${MNT}" && rmdir "${MNT}"
    MNT=""
    echo ">> ${DISK} is unmounted: safe to unplug"
}

copy_to_root_storage() {
    if [[ $EUID -ne 0 ]]; then
        echo ">> copying ${IMAGE} into root's container storage"
        podman image scp "${USER}@localhost::${IMAGE}" root@localhost::
    fi
}

case "${TARGET}" in
    image) ;;
    qcow2|iso|raw) sudo_once "the disk image" ;;
    install) check_install_target; sudo_once "installing onto ${DISK}"; unmount_disk ;;
    update)
        if [[ -z "${STAGE_ONLY}" ]]; then
            check_update_target; sudo_once "updating ${DISK}"; unmount_disk
        fi
        ;;
    *) echo "usage: host/build.sh [image|qcow2|iso|raw|install [/dev/sdX]|update [/dev/sdX]] [--bases|--no-bases] [--full]" >&2; exit 2 ;;
esac

stage_client
stage_wadd
stage_hud
stage_wadbrowser
stage_assets
stage_ours
stage_secrets
# The tunnel's token: read from /etc/cloudflared/token (it's root's) into a
# private file in the runtime dir, handed to the build as a secret and
# removed straight after. Never in .build/ or the build context.
TOKEN_FILE=/etc/cloudflared/token
token_args=()
if [[ -e "${TOKEN_FILE}" ]]; then
    TOKEN_COPY="$(umask 077 && mktemp -p "${XDG_RUNTIME_DIR:-/tmp}" cloudflared-token.XXXXXX)"
    trap 'rm -f "${TOKEN_COPY}"; cleanup' EXIT
    if [[ -r "${TOKEN_FILE}" ]]; then cat "${TOKEN_FILE}"; else sudo cat "${TOKEN_FILE}"; fi > "${TOKEN_COPY}"
    token_args=(--secret "id=cloudflared_token,src=${TOKEN_COPY}"
        --build-arg "CLOUDFLARED_TOKEN_SHA=$(sha256sum "${TOKEN_COPY}" | cut -c1-16)")
else
    echo ">> WARNING: no ${TOKEN_FILE} here: the image has no tunnel token (streaming waits for one)"
fi
echo ">> building ${IMAGE}"
podman build "${token_args[@]}" -f "${ROOT}/host/Containerfile" -t "${IMAGE%:*}:unchunked" "${ROOT}"
if [[ -n "${TOKEN_COPY:-}" ]]; then rm -f "${TOKEN_COPY}"; fi
rechunk

case "${TARGET}" in
    image)
        echo ">> done: ${IMAGE} is built here (nothing written to a drive; host/build.sh update puts it on one)"
        ;;
    qcow2|iso|raw)
        mkdir -p "${ROOT}/output"
        # bootc-image-builder reads the image from root's storage.
        copy_to_root_storage
        sudo podman run --rm -it --privileged --pull=newer \
            --security-opt label=type:unconfined_t \
            -v "${ROOT}/output:/output" \
            -v /var/lib/containers/storage:/var/lib/containers/storage \
            -v "${BIB_CONFIG}:/config.toml:ro" \
            quay.io/centos-bootc/bootc-image-builder:latest \
            --type "${TARGET}" --rootfs xfs --config /config.toml "${IMAGE}"
        echo ">> disk image in ${ROOT}/output/"
        ;;
    install)
        copy_to_root_storage
        echo ">> installing ${IMAGE} to ${DISK}"
        sudo podman run --rm --privileged --pid=host \
            --security-opt label=type:unconfined_t \
            -v /dev:/dev -v /var/lib/containers:/var/lib/containers \
            "${IMAGE}" bootc install to-disk --wipe "${DISK}"
        if [[ -n "${BASES}" ]]; then
            # The host image is installed already; only the bases go over.
            stage_stick
            push_to_drive
        fi
        echo ">> done; boot the Surface from USB (hold Volume-down, press Power)"
        ;;
    update)
        stage_stick
        if [[ -n "${STAGE_ONLY}" ]]; then
            mkdir -p "${STAGE_ONLY}"
            rsync -rt --delete "${STICK_OCI}/" "${STAGE_ONLY}/"
            touch "${STAGE_ONLY}/pending"
            echo ">> staged into ${STAGE_ONLY}"
        else
            push_to_drive
            echo ">> done; boot ${DISK}: it imports the update, then restarts once into it"
        fi
        ;;
esac
