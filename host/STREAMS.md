# Streams

A wadspace can be streamed to any browser: a link like
`https://ws-<guid>.wadspaces.com` and a username and password to hand on with
it. The WadSpaces Client makes one (Launch → Stream…, or Stream on a Home
card); Secrets → Cloud Tunnel shows whether the machine can.

```
browser ──https──▶ Cloudflare ──tunnel──▶ cloudflared ──▶ wadd's proxy ──▶ the wadspace's sidecar
  ws-<guid>.wadspaces.com      (*.wadspaces.com)      127.0.0.1:8085      127.0.0.1:<port> (Selkies)
```

- **The sidecar** (`images/stream`, `localhost/wadspaces-stream:trixie`):
  linuxserver's Selkies, its compositor, encoder and web client behind nginx.
  The streamed wadspace draws on its compositor (a display volume they
  share) instead of the screen, and plays its sound through it. wad-core's
  `stream_quadlets` writes the units; the wadspace's is bound to its sidecar.
- **wadd's proxy** (`apps/wadd/src/proxy.rs`) listens on loopback only
  (127.0.0.1 and ::1, port 8085). It looks at the Host: a stream here is
  passed to its sidecar as it came, websockets included; `ws-selftest.*` is
  answered by the proxy itself; anything else is 404. It logs no one in.
- **The tunnel**: `cloudflared.service`, from Cloudflare's RPM, with the
  token in `/etc/cloudflared/token`. The tunnel routes `*.wadspaces.com` to
  `http://127.0.0.1:8085`; the DNS has a `*.wadspaces.com` CNAME to it.
- **WebSockets only** for now. `[streaming] webrtc = true` in
  `/etc/wadspaces/wadd.toml` adds WebRTC (STUN and Cloudflare's TURN, the key
  from `/usr/lib/wadspaces/secrets/cloudflare.toml`), untried.

## What keeps it closed

- Each stream gets a new link (128 random bits) and its own login (a user and
  a 20-character password), asked for by the sidecar's nginx on every path,
  the page and the websocket. The sidecar refuses to start without one.
- The password is podman secret `wad_stream_<id>` only: not in units,
  `streams.json`, events, logs, the heartbeat or the account. Only the
  machine's own socket hands it out (GET /v1/streams/{id}/share). `wad_`
  secrets are wadd's: not listed, set, synced, deleted or mounted by a design.
- A stream ends when it's stopped, when its wadspace stops or is opened at the
  machine (the person at the machine wins), or after 30 minutes (`idle_minutes`)
  with nothing from a viewer. Its link then answers 404.
- Restarting wadd keeps the streams whose sidecars still run; a sidecar wadd
  has no stream for is stopped.
- The sidecar's port is published on 127.0.0.1 only; the tunnel is the only
  way in from outside, and the machine needs no open port.
- The tunnel's token is only ever `/etc/cloudflared/token` (root's, 0600):
  baked in by `host/build.sh` from the build machine's own file (a build
  secret, never in the build context), replaced through the client.
- Cloudflare ends TLS, so it sees the traffic, the login included.
- **Anyone with the token can run a second connector**, and Cloudflare would
  send them some of the traffic. Keep it to this machine: don't run the
  laptop's cloudflared while the machine runs (the self-test notices
  another connector), and treat the stick and `.build/stick` as secret.

## Setting it up

1. On the build machine, Cloudflare's RPM and the token: `/etc/cloudflared/token`.
2. In Cloudflare: the tunnel's public hostname `*.wadspaces.com` →
   `http://127.0.0.1:8085`; a DNS CNAME `*.wadspaces.com` to the tunnel; a
   cache rule "bypass cache" for `*.wadspaces.com`; Bot Fight mode not
   challenging `ws-*` (it would break the websocket). `www` needs its own
   record (Firebase), or the wildcard sends it to the proxy (404).
3. `images/build.sh --only stream`, then `host/build.sh update --bases`.
4. On the machine: Secrets → Cloud Tunnel says Connected; Run self-test.

## Trying it

- `apps/wadd/dev/try-stream.sh`: a real stream on a laptop, rootless,
  without the tunnel (logins, websockets, a screenshot, a restart, the end).
- `apps/wadd/dev/try-tunnel.sh`: through the real tunnel from the laptop,
  for a phone on cellular (start the laptop's cloudflared first, and stop it
  after).
- On the machine: Stream a wadspace, open the link on a phone on cellular,
  log in: the desktop. The link without the login: the login prompt.
