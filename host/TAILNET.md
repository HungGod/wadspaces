# Tailnet

A machine joins its owner's Tailscale network (the tailnet), so the owner's
own devices can manage it from anywhere: build a wadspace from the account's
library, open it on the machine's screen, stop it, or stream it to the
device. The WadSpaces Android app does this; the WadSpaces Client on the
machine says who may (Machines → this machine → Tailnet).

```
phone (Tailscale app) ──WireGuard──▶ tailscaled ──▶ wadd's remote API ──▶ wadd
   the WadSpaces app                  tailscale0      100.x:8087 (tailnet only)
```

- **tailscaled** (`tailscaled.service`, Tailscale's RPM) keeps the machine's
  key and login in `/var/lib/tailscale`, which bootc keeps across updates.
- **Joining**: Machines → this machine → Tailnet → Join. wadd runs
  `tailscale login`; the tab shows the login link as a QR code. Scan it with
  a phone that's signed in to Tailscale and approve the machine.
- **wadd's remote API** (`apps/wadd/src/remote.rs`) listens on the machine's
  tailnet addresses only (port 8087, `[tailnet]` in wadd.toml), bound to the
  `tailscale0` interface. It's plain HTTP inside WireGuard, which already
  encrypts the traffic and proves which device sent it. It's a small API of
  its own, never the machine's full one.

## What keeps it closed

Every connection is checked before anything is answered:

1. **Which device is it?** wadd asks tailscaled (`whois` on the connection's
   address). Anything that isn't a device on the tailnet is refused.
2. **Tagged devices are refused**: servers and shared services, not a
   person's own device.
3. **It must belong to the machine's own Tailscale user.** Other people in
   the tailnet, and devices shared in from other tailnets, are refused.
4. **It must be trusted on this machine.** The first time one of your devices
   asks, the machine asks you: "Allow <device> to control this machine?" (the
   HUD, and the Tailnet tab). Trusted devices are kept in
   `/var/lib/wadspaces/tailnet-devices.json` (root's, 0600); removing one in
   the tab takes it off. The list is cleared if the machine joins another
   user's tailnet.
5. **And the account**: apart from asking whether it's trusted, every call
   carries the owner's sign-in (a Firebase ID token), which wadd checks with
   Firebase against the machine's owner. A trusted phone's other apps can't
   use it without the owner's account.

What a trusted device can do is only: see the machine's wadspaces, build one
of the **owner's** wadspaces from the account (wadd reads it from the
library itself; the device sends only its id), open or stop it on the
machine, and stream it. It can't read secrets or sign-ins, change Wi-Fi,
power, the tunnel, storage or devices.

- A stream still goes through the Cloudflare tunnel (host/STREAMS.md); the
  tailnet only carries the request and hands the device the stream's
  sign-in link.
- The machine never listens on its LAN or Wi-Fi address for this.

## Things to know

- **Key expiry**: a device Tailscale logged in interactively must log in again
  after 180 days. In the Tailscale admin console, Machines → the machine →
  Disable key expiry keeps it joined.
- **Device approval**: if the tailnet requires an admin to approve new
  devices, the tab says the machine is waiting; approve it in the admin
  console.
- **Tailscale's version** is pinned in `host/Containerfile`, since wadd reads
  tailscaled's LocalAPI, which can change between versions.

## Trying it

- `apps/wadd/dev/try-tailnet.sh`: a laptop wadd on the tailnet, rootless
  (tailscaled in userspace mode, which leaves the laptop's own network
  alone).
