# WadSpaces stream sidecar.
#
# Runs next to a lean wadspace (see ../base/) while it's streamed: this
# container is only the Selkies stream (pixelflux's compositor, the encoder,
# and the web client behind nginx on :3000). It runs no desktop of its own;
# its Wayland socket goes in a volume shared with the wadspace at
# /run/wadspaces-display, and the wadspace's labwc draws on it.
#
# wadd starts it (wad-core's stream_quadlets) with CUSTOM_USER and PASSWORD
# (nginx's basic auth on every path, the page and the websocket) and
# publishes :3000 on 127.0.0.1 only: wadd's streaming proxy is the way in,
# behind the Cloudflare tunnel, which ends TLS. It refuses to start without
# a user and password (init-stream-display).
#
# One image serves every wadspace. Pinned: images/build.sh --only stream.
FROM ghcr.io/linuxserver/baseimage-selkies:debiantrixie@sha256:5abd49f48a1966f7c715b7f70ab1b143bf131726ee1961b1dfe6efdb630f4290

ARG BUILD_DATE
ARG VERSION
LABEL build_version="WadSpaces stream version:- ${VERSION} Build-date:- ${BUILD_DATE}"
LABEL maintainer="dwadsworth"
LABEL org.opencontainers.image.source="https://github.com/HungGod/WadSpaces"

# No Docker inside, no gamepads or webcam (their device nodes would land in
# the sidecar, not the wadspace), no file transfers (same), and a failed
# start-up step halts the container rather than serving half set up. Sound:
# pulseaudio opens a second socket in the shared volume (root/etc/pulse/
# default.pa.d/wadspaces.pa), where the wadspace's apps play to it
# (PULSE_SERVER), and Selkies sends it to the viewer.
ENV TITLE="WadSpaces" \
    PIXELFLUX_WAYLAND=true \
    START_DOCKER=false \
    NO_GAMEPAD=true \
    NO_WEBCAM=true \
    SELKIES_FILE_TRANSFERS=none \
    S6_BEHAVIOUR_IF_STAGE2_FAILS=2

COPY root/ /
RUN chmod +x /etc/s6-overlay/s6-rc.d/*/run

EXPOSE 3000
