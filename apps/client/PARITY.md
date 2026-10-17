# Native client: parity with the Tauri one

The native WadSpaces Client (`apps/client`, GTK4) replaced the Tauri one
(React in WebKitGTK) on the machine on 2026-10-16, after a soak on the
Surface. It's `/usr/bin/client`, and its log is in the journal:
`journalctl -b -t client`.

On its first start, `client-session` removes what the Tauri app left in
`~/.local/share/io.wadspaces.client/` (WebKit's storage and caches; the
native client's `account/` folder stays). Board edits the Tauri app hadn't
saved were in that storage, and aren't carried over. `bootc rollback` goes
back to the image before.

## What matches the web app

| Area | Web app | Native |
| --- | --- | --- |
| Setup | MachineGate: Wi-Fi, sign-in, Welcome (username), link, relink, GitHub | setup.rs, the `client-signin` window |
| Home | greeting, numbers, Jump back in, Running now, Your wadspaces, the tour | pages/home.rs |
| Launcher | pick, focus mode, customize a run, WadBrowser runs, saved setups (deleted with Undo) | launcher.rs, slots.rs |
| Activity | builds and launches, their logs | activity.rs |
| Focus | the banner and countdown; going back after a restart | focus.rs, chrome.rs |
| Machines | the list; Wadspaces, History, Storage, Files and Diagnostics tabs; other machines through their relay | pages/machines/ |
| A wadspace's details | WadspaceDrawer; the row menu's Details, Customize, Edit in Builder, New from it, Delete | details.rs |
| Secrets | saved sign-ins (make, rename, sign out, delete), tokens, GitHub | pages/secrets/mod.rs, signins.rs |
| Secrets → Cloud Tunnel | cloudflared's state, the tunnel's id, self-test, replacing the token, what's streamed (share, stop) | pages/secrets/tunnel.rs, stream.rs |
| Streams | Stream… in the launcher, Stream on Home's cards, streamed rows in Running now, the share dialog (QR, link, login) | stream.rs, launcher.rs, pages/home.rs |
| Builder | the board, drags, the three editors, Preview, Fix, Recently deleted (and for good), Undo, Build all, unsaved work kept | pages/builder/ |
| Tour, notices | Tour.tsx, ProfileNotices | tour.rs, notices.rs |
| Machine chrome | wadd's notices, "Starting …", the focus prompt | store.rs, chrome.rs |
| The HUD's room | `--hud-room` | `window.hud-room` (hud.json, watched) |

Two differences are deliberate:

- **Drags are in-app only.** As in the web app, nothing is dragged in from
  outside.
- **The key menu is a popover.** It renders as a separate surface, so
  snapshots don't show it.

## Not tried yet

These have only run in the headless snapshots (`dev/headless.sh`), against
the made-up machine in `fixtures/desktop/`:

- **Input:** real pointer drags (only scripted ones so far), touch on the
  Surface, and keyboard focus order.
- **Account writes:**
  - the Builder's saves;
  - Recently deleted's "for good";
  - restoring a wadspace;
  - deleting from the details drawer;
  - `set_onboarded`.
- **Data kept across a restart:** `builder.json` (Recently deleted, unsaved
  work) and the tour's step.
- **Copying in from the machine:** a folder or files, through `/v1/browse`
  and `/v1/copies`.
- **Sign-out:** signing out, then in as someone else (the board, tour and
  notices start afresh).

## Still to watch on the Surface

- Memory: about 60 MB idle (80 MB with the Builder open). In the headless
  harness on the laptop (KWin, nothing else sharing GTK), it's 62 MB for an
  empty account and 86 MB for the sample's two wadspaces, each with a
  picture wallpaper. On the Surface:
  `grep Pss /proc/$(pidof client)/smaps_rollup`. On a laptop:
  `dev/headless.sh measure`.
- 0% CPU while a wadspace is on screen.
- The renderer: GTK's default so far. `GSK_RENDERER=ngl`, `vulkan` and
  `cairo` (in `client-session`) are still to be timed and watched for
  glitches.
