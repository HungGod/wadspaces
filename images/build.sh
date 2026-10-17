#!/usr/bin/env bash
# Build the base every template is made from, and the stream sidecar.
# Templates and customizations are built by wadd from the WadSpaces Client's
# Builder, on the machine.
#
#   images/build.sh                  the base and the stream sidecar
#   images/build.sh --only base      just one (base or stream)
#   images/build.sh --example vanua-academy
#                                    an example wadspace's layers
#                                    (images/examples/<name>/layers.json):
#                                    its template's image, then its
#                                    customization's, as wadd builds and
#                                    labels them (so wadd counts them built),
#                                    the base first if it's missing
#   images/build.sh --no-cache       without podman's layer cache
#   images/build.sh --push           also tag and push to ghcr.io/hunggod
#
#   base    localhost/wadspaces-base:trixie    the lean desktop every workspace
#           builds on; its window goes on the machine's screen.
#           WadBrowser and the clipboard bridge are built for it first
#           (images/builder/: a Debian container with Rust; the first build
#           takes a while), and Dolphin is staged in it from linuxserver's
#           build (lscr.io/linuxserver/dolphin, pulled once: DOLPHIN_PULL=1
#           gets a newer one)
#   stream  localhost/wadspaces-stream:trixie  the sidecar a streamed wadspace
#           draws on: Selkies (linuxserver's baseimage-selkies, pinned in
#           images/stream/Dockerfile), published by wadd behind its
#           streaming proxy
#
# host/build.sh install (or update --bases) copies them onto a machine's
# drive: the machine builds templates and customizations on the base.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REGISTRY="${REGISTRY:-ghcr.io/hunggod}"
ALL=(base stream)
ONLY=()
EXAMPLES=()
NO_CACHE=false
PUSH=false

usage() { sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --only) [[ $# -gt 1 && " ${ALL[*]} " == *" $2 "* ]] || { echo "--only ${ALL[*]// /|}" >&2; exit 2; }
                ONLY+=("$2"); shift 2 ;;
        --example) [[ $# -gt 1 && -f "${HERE}/examples/$2/layers.json" ]] || {
                       echo "--example: one of $(ls "${HERE}/examples" | tr '\n' ' ')" >&2; exit 2; }
                   EXAMPLES+=("$2"); shift 2 ;;
        --no-cache) NO_CACHE=true; shift ;;
        --push) PUSH=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done
if [[ ${#EXAMPLES[@]} -gt 0 ]]; then
    # An example needs the base.
    podman image exists localhost/wadspaces-base:trixie || ONLY+=(base)
elif [[ ${#ONLY[@]} -eq 0 ]]; then
    ONLY=("${ALL[@]}")
fi

# Fail before a long build, not at the first push.
if ${PUSH} && ! podman login --get-login "${REGISTRY%%/*}" > /dev/null 2>&1; then
    echo "not logged in to ${REGISTRY%%/*}: podman login ${REGISTRY%%/*}" >&2
    exit 1
fi

ROOT="$(cd "${HERE}/.." && pwd)"
STAGE="${ROOT}/.build/wadbrowser"

# WadBrowser, wadspaces-icon and wadspaces-clipboard (crates/wad-clip: the
# wadspace's clipboard bridged to the machine's), built for trixie in a
# Debian container (a Fedora build would link another glibc and WebKitGTK),
# staged for the base image's `wadbrowser` build context with WadBrowser's
# icons. Cargo's caches
# live in podman volumes, so a rebuild only builds what changed.
build_wadbrowser() {
    echo ">> building localhost/wadspaces-builder:trixie from images/builder/"
    podman build -q -t localhost/wadspaces-builder:trixie "${HERE}/builder" > /dev/null
    echo ">> building WadBrowser for trixie (cargo, in the builder)"
    # label=disable, not :z: relabelling would rewrite the whole checkout.
    podman run --rm --security-opt label=disable \
        -v "${ROOT}":/src -w /src \
        -v wadspaces-cargo-registry:/usr/local/cargo/registry \
        -v wadspaces-target-trixie:/target -e CARGO_TARGET_DIR=/target \
        localhost/wadspaces-builder:trixie \
        sh -c 'cargo build --locked --profile wadbrowser -p wadbrowser -p wad-icons --features wad-icons/cli -p wad-clip &&
               cp /target/wadbrowser/wadbrowser /target/wadbrowser/wadspaces-icon /target/wadbrowser/wadspaces-clipboard \
                  /src/.build/wadbrowser/bin/ &&
               rm -rf /tmp/sd && cp -r /src/apps/selkies-desktop /tmp/sd && make -s -C /tmp/sd &&
               cp /tmp/sd/selkies-desktop /src/.build/wadbrowser/bin/'
    cp -r "${ROOT}/apps/wadbrowser/icons/hicolor/." "${STAGE}/icons/"
}

# Dolphin, as linuxserver's docker-dolphin builds it from source (its image
# is trixie too): only its own files, staged for the base's `dolphin` build
# context. Its feature script (wadspaces-feature dolphin) puts them in place
# with the libraries they need. Machines never pull images, so it has to
# come in the base.
DOLPHIN_IMAGE="${DOLPHIN_IMAGE:-lscr.io/linuxserver/dolphin:latest}"
stage_dolphin() {
    local stage="${ROOT}/.build/dolphin" ctr tmp
    if [[ -n "${DOLPHIN_PULL:-}" ]] || ! podman image exists "${DOLPHIN_IMAGE}"; then
        echo ">> pulling ${DOLPHIN_IMAGE} (Dolphin's build)"
        podman pull -q "${DOLPHIN_IMAGE}" > /dev/null
    fi
    echo ">> staging Dolphin from ${DOLPHIN_IMAGE}"
    rm -rf "${stage}" && mkdir -p "${stage}/usr/local"
    tmp="$(mktemp -d)"
    ctr="$(podman create -q "${DOLPHIN_IMAGE}")"
    podman cp "${ctr}:/usr/local/." "${tmp}/" > /dev/null
    podman rm -f "${ctr}" > /dev/null
    (cd "${tmp}" && find . \( -path ./bin/dolphin-emu -o -path ./bin/dolphin-tool -o -path ./share/dolphin-emu \
        -o -path ./share/applications/dolphin-emu.desktop -o -path './share/icons/*' -name 'dolphin-emu.*' \
        -o -path './share/man/man6/dolphin-emu*' \) -prune -print) | while read -r p; do
        mkdir -p "${stage}/usr/local/$(dirname "${p}")"
        cp -a "${tmp}/${p}" "${stage}/usr/local/${p}"
    done
    rm -rf "${tmp}"
    [[ -x "${stage}/usr/local/bin/dolphin-emu" ]] || { echo "no dolphin-emu in ${DOLPHIN_IMAGE}" >&2; exit 1; }
}

for name in "${ONLY[@]}"; do
    img="localhost/wadspaces-${name}:trixie"
    args=(--build-arg BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" --build-arg VERSION="$(date -u +%Y%m%d)")
    ${NO_CACHE} && args+=(--no-cache)
    if [[ "${name}" == base ]]; then
        rm -rf "${STAGE}" && mkdir -p "${STAGE}/bin" "${STAGE}/icons"
        build_wadbrowser
        stage_dolphin
        args+=(--build-context "wadbrowser=${STAGE}" --build-context "dolphin=${ROOT}/.build/dolphin")
    fi
    echo ">> building ${img} from images/${name}/"
    podman build "${args[@]}" -t "${img}" "${HERE}/${name}"
    if ${PUSH}; then
        podman tag "${img}" "${REGISTRY}/wadspaces-${name}:trixie"
        podman push "${REGISTRY}/wadspaces-${name}:trixie"
    fi
done
for name in "${EXAMPLES[@]}"; do
    # wad-core writes each layer's build folder (cargo xtask layer); built
    # FROM the one before, labelled with its folder's sha and that image's id.
    out="${ROOT}/.build/examples/${name}"
    mkdir -p "${out}"
    (cd "${ROOT}" && cargo run -q -p xtask -- layer "${HERE}/examples/${name}/layers.json" "${out}") > "${out}/layers.txt"
    folders=(template customization)
    i=0
    while read -r key ref from sha; do
        args=(--label "io.wadspaces.layer=${key}" --label "io.wadspaces.dockerfile-sha256=${sha}"
              --label "io.wadspaces.from=$(podman image inspect -f '{{.Id}}' "${from}")")
        ${NO_CACHE} && args+=(--no-cache)
        echo ">> building ${ref} (${key}) from images/examples/${name}/"
        podman build "${args[@]}" -t "${ref}" "${out}/${folders[i]}"
        i=$((i + 1))
    done < "${out}/layers.txt"
done
echo ">> built: ${ONLY[*]} ${EXAMPLES[*]}"
