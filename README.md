# WadSpaces

The software that runs a WadSpaces machine, with the WadSpaces Client, the machine's app for designing and running wadspaces. The site (`wadspaces.com`, a landing page) and the Firebase backend the machines use (Firestore's rules, `enrollMachine`, the GitHub sign-in page) are in their own repo, `wadspaces-client`, checked out beside this one (`../wadspaces-client`).

| Path | What it is |
|---|---|
| `apps/wadd/` | wadd, the machine daemon: workspaces, the Builder's layer images and installs, launches (wadspaces, throwaway runs, WadBrowser setups, the terminal), projects, saved sign-ins, the account link, and streams (a wadspace in any browser at `https://ws-<guid>.wadspaces.com`, through its proxy and the Cloudflare tunnel: `host/STREAMS.md`), and the tailnet (the owner's trusted devices manage the machine through its small remote API on the tailnet: `host/TAILNET.md`). Its API is `/v1` over `/run/wadd/wadd.sock`. |
| `apps/client/` | The WadSpaces Client (installed as `/usr/bin/client`, the kiosk's shell): GTK4, Rust. Home, the launcher, Activity, Machines, Secrets and the Builder (a board of stacks: templates, Extensions, Customizations), signed in to the account as you. Built on `wad-gtk`, `wad-board` and `wad-account`. |
| `apps/signin/` | `client-signin`, the client's GitHub sign-in window (WebKitGTK): it opens GitHub's page, prints what GitHub hands back and exits. |
| `apps/hud/` | The HUD: the bar above every wadspace (Home, Wi-Fi, brightness and volume, power, the clock; it tucks away behind an arrow), and the switcher (what runs, spaces then apps, each with its Super+number), Wi-Fi, power and clipboard-history (Super+V) overlays. WadNotes is built into it (`src/notes/`): notes and tasks for the wadspace on screen (or the machine's own, "Default": WadSpaces Client, WadTerminal, WadBrowser), a calendar shared by all of them, and reminders. They sit side by side at the top right; the bar's WadNotes button puts them away and brings them back, and its menu (hover, or a long press) turns each on or off and picks the notespace. |
| `apps/wadbrowser/` | WadBrowser, the WadSpaces web browser (Tauri chrome, WebKitGTK tabs). It's in the lean base image, and on the machine itself as an app, with instances for saved WadBrowser setups. |
| `crates/` | Shared Rust crates:<br>• `wad-proto`: types shared by wadd, the client and the HUD<br>• `wad-core`: the Builder's layers (`layers.rs`), its catalog and starter templates (`data/`), sign-in kinds, and the generators; also compiled to wasm (`cargo xtask wasm`, kept in budget by `xtask ci`)<br>• `wad-board`: the Builder's board (stacks, saves, keys)<br>• `wad-account`: the account in Firestore, as the signed-in user<br>• `wad-gtk`: the client's and the HUD's widgets and theme<br>• `wad-wadd-client`: wadd's `/v1` from the client and the HUD<br>• `wad-config`<br>• `wad-store`: wadd's state files<br>• `wad-podman`, `wad-systemd`, `wad-sway`<br>• `wad-input`: the keyboard proxy<br>• `wad-git`: project clones and fast-forwards<br>• `wad-net`: Wi-Fi through NetworkManager<br>• `wad-github`: GitHub's REST calls (whose token, repos, new repos)<br>• `wad-firebase`: Firestore from Rust<br>• `wad-clip`: one clipboard for the machine (each wadspace's `wadspaces-clipboard` bridge, and the HUD's history)<br>• `wad-notes`: WadNotes' files: markdown notes and tasks per wadspace, the calendar's events, reminders and each wadspace's colour |
| `host/` | The machine's OS image (Fedora bootc, sway, WadSpaces Client as the shell, cloudflared for streams), and `host/build.sh` to build it and write it to a drive. Two apps run on the machine itself (`usr/lib/wadspaces/workspaces.d/{terminal,browser}.toml`: workspaces of kind `app`): WadTerminal (foot) and WadBrowser. wadd opens and closes them like wadspaces, each in a transient unit `wad-<id>.service` as the kiosk user, full screen and on Super+Tab, but with no Super+number. |
| `images/` | The base image every template is built on (`base`: the lean desktop, WadBrowser, and a feature script per catalog app), and the stream sidecar a streamed wadspace draws on (`stream`: Selkies). `images/build.sh` builds them; templates (one on another) and Extensions layers are built from the base by wadd. `examples/` has two real wadspaces' layers (Cosmic Bodybuilding, VanuaAcademy). |
| `fixtures/` | What every renderer must reproduce:<br>• unit files (`quadlet/`)<br>• the core's and the board's goldens (`core/`, `board/`), which wad-core and wad-board check<br>• the native client's made-up machines (`desktop/`) and the old web app's pages as pictures (`client-shots/`) |

## Tests

```bash
cargo xtask ci        # fmt, clippy, tests, and the wasm build in budget
cargo test --workspace
cargo xtask jscheck   # WadBrowser's chrome, type-checked (tsc through npx)
```

The goldens (`fixtures/core`, `fixtures/board`) are what wad-core and the Builder's board return: after a deliberate change, `UPDATE_GOLDENS=1 cargo test -p wad-core goldens -- --ignored` (or `-p wad-board`) rewrites them.

## The client on a laptop

Try it in a window, without the host image. Run wadd as you, then the client; it finds wadd's socket by itself.

```bash
cargo run -p wadd -- serve --user &
cargo run -p client                     # --kiosk for fullscreen
```

You sign in with your real account (Firebase): its web config goes in `host/secrets/firebase.toml` (`api_key`, `project_id`; gitignored), which `host/build.sh` puts on the image too. Wi-Fi and Power in the client act on the laptop for real.

`apps/client/dev/headless.sh` runs it on a screen of its own (a headless KWin) against made-up machines (`fixtures/desktop/`): `snapshot` writes every page, dark and light, to PNGs; `measure` prints its memory.

The Rust toolchain is pinned in `rust-toolchain.toml`. On Fedora the apps also need these packages:
- `gtk4-devel` and `gtk4-layer-shell-devel`, for the client and the HUD;
- `webkit2gtk4.1-devel`, `javascriptcoregtk4.1-devel`, `libsoup3-devel`, `gtk3-devel`, `librsvg2-devel`, for WadBrowser and the sign-in window;
- `binaryen`, for the wasm build.

## wadd on a laptop

`wadd serve --user` runs it as you, on a laptop:
- rootless podman and your systemd;
- a socket at `$XDG_RUNTIME_DIR/wadd/wadd.sock` (`curl --unix-socket … http://wadd/v1/states`).

The trial scripts use their own state under `.build/`, and need `systemctl --user start podman.socket`.

```bash
apps/wadd/dev/try.sh           # workspaces: cold and warm start, stop, restart, a missing image
apps/wadd/dev/try-view.sh      # the screen: a headless sway with the machine's rules and a real lean
                               # workspace's window: switching, Super+Tab, focus sessions
apps/wadd/dev/try-apps.sh      # the machine's own apps (foot, WadBrowser) in the headless sway:
                               # full screen, Super+Tab, restart, stop, and exiting by themselves
apps/wadd/dev/try-projects.sh  # projects in a wadspace: GitHub repos cloned (and fast-forwarded)
                               # and files copied in by the wadspace itself; a throwaway run
apps/wadd/dev/try-layers.sh    # the Builder's layers: Extensions built (their template first), a
                               # template on a template, a stack installed, opened, a throwaway run
apps/wadd/dev/try-cloud.sh     # the account link, against the Firebase emulators (real rules and
                               # enrollMachine, from ../wadspaces-client): link, heartbeat, commands,
                               # project and secret sync
apps/wadd/dev/try-network.sh   # NetworkManager for real: status and networks in range
                               # (--join SSID / --forget SSID change things)
apps/wadd/dev/try-stream.sh    # a wadspace streamed on its sidecar, behind wadd's proxy:
                               # logins, websockets, a browser's screenshot, a restart
apps/wadd/dev/try-tunnel.sh    # the same through the real Cloudflare tunnel, for a phone
                               # (the laptop's cloudflared started first)
sudo target/debug/wadd keys    # the keyboard proxy for 20 s (prints what Super chords would do)
```

A few scripts need extra pieces:
- `try-view.sh`, `try-layers.sh` and `try-apps.sh` need sway. Unpacked under `.build/sway` is enough (`dev/sway.sh` says how). `try-apps.sh` also needs foot (unpacked the same way) and WadBrowser built (`cargo build --profile wadbrowser -p wadbrowser`).

The HUD's surfaces, WadNotes' included, render to PNGs with `apps/hud/snapshot.sh` (made-up notes, tasks and events). `apps/hud/try-wadd.sh` runs the HUD against a real wadd in the headless sway and screenshots WadNotes following the screen.
- `try-layers.sh` needs the base image: `images/build.sh`.

## History

This repo was made on 2026-10-02 from older repos. Their history is kept as the second parent of the import merges:
- `WadCreator` → `apps/wadcreator/` (renamed `apps/client/` on 2026-10-07; split out to `wadspaces-client`, with its history, on 2026-10-16, when the native client took over `apps/client/`);
- `Wadspaces-Tools` (`Workspace-Switcher`) → the Python wadd (removed after the cutover to the Rust one) and `host/`.

`images/` came from `Wadspaces-David`'s `_common` and `_stream` on 2026-10-04, without their history. That repo's hand-written workspaces aren't needed any more: the Builder makes workspaces now.
