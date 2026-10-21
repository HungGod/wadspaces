# WadBrowser

The web browser in every WadSpaces workspace (formerly KaleBrowser, an
Electron app). It's three things in one program:

- **WadBrowser**: a browser with tabs and an address bar;
- **WadBrowser Focus**: tabs, no address bar; pages arrive by links;
- **every web app's window** (`wadspaces-webapp`): one site, its own name,
  icon and window class (`wadspaces-webapp-<id>`), no address bar.

Links from other apps (`xdg-open`) open in whichever of the first two the
wadspace's customization picked (`/etc/wadspaces/wadbrowser.conf`).

On the machine itself, beside the wadspaces, the launcher starts WadBrowser,
WadBrowser Focus and **WadBrowser setups**: a mode, web apps, and a saved
sign-in for each (or none), saved to launch again or run once. Each runs as
an instance of its own (below), so several can be open at once, each set up
its own way.

## How it's put together

Tauri 2 on WebKitGTK 4.1. Tauri owns each window and its one webview, the
chrome (`ui/`: plain HTML, CSS and JS, embedded in the binary). The pages
are WebKitGTK views WadBrowser makes itself, in a stack under the chrome:
Tauri's own child webviews can't be laid out on Wayland, and pages never get
Tauri's IPC this way. Menus and panels are a transparent view in a popup
surface of their own, which the compositor blends over the page.

- **One process per user and instance** (`ipc.rs`, `instance.rs`): a later
  launch hands its request to the running browser over
  `$XDG_RUNTIME_DIR/wadbrowser[/<instance>]/ctl.sock` and exits,
  so a link opens in the window already there. Tabs move between windows
  (drag one off the strip, or onto another window's chrome); a moved tab's
  page loads again in its new window, history kept: WebKit ties a view's
  GPU drawing to its window, so views never change windows.
- **Memory**: one shared profile (one network process and cache for every
  web app); tabs idle for 15 minutes sleep (their history kept) and wake
  where they were; WebKit sheds caches under memory pressure.
- **Sign-ins** (`profile.rs`): cookies are kept on disk (`cookies.sqlite`
  in the profile), so a sign-in outlasts the browser. A web app given a
  saved sign-in in WadSpaces uses profile `signin-<id>` instead: its data
  is the sign-in's folder that wadd mounts at
  `/run/wadspaces-signins/<id>` (listed in
  `/run/wadspaces-extra/signins.json`), shared with every wadspace whose
  apps use it; its cache stays the wadspace's own. WadBrowser's own windows
  do the same with the sign-in listing `@browser` (the key of the group the
  browser is in, in the Builder's Customization). A web app in a setup
  with no saved sign-in gets an **ephemeral** profile (WebKit's ephemeral
  data manager): nothing of it reaches the disk, and it's forgotten when its
  last window closes.
- **GPU**: on when the container has a usable `/dev/dri` (`gpu.rs`), video
  decoded with VA-API.
- **What WebKitGTK can't do**: DRM (EME) and video calls (WebRTC). Spotify,
  Discord, Zoom and Slack are Chrome web apps in WadSpaces Client for that reason.

## Command line

```
wadbrowser [URL...]                       a browser window
wadbrowser --no-urlbar [URL...]           WadBrowser Focus
wadbrowser --default [URL...]             what the workspace chose (links)
wadbrowser --app <id> --name <name> --url <start> [URL...]
  --new-window  --profile <p>  --gpu auto|on|off  --icon <png>  --config <kalebrowser.json>
wadbrowser --instance <id> [...]          an instance of its own: socket, session and
                                          profiles under wadbrowser/instances/<id>
wadbrowser --instance <id> --setup <file> a setup the launcher wrote: its mode, web apps
                                          and their sign-ins (each app's window opens)
```

wadd writes a setup's file into the sign-ins folder
(`/var/lib/wadspaces-signins/.wadbrowser/<id>.json`, readable by the screen
user) and starts it as a transient unit; WadBrowser Focus on the machine is
the `focus` instance. A setup run once (or with no name) is an ephemeral
instance: it keeps no session.

Settings: `/etc/wadspaces/wadbrowser.conf`, then
`~/.config/wadbrowser/wadbrowser.conf` (`default`, `home`, `search`,
`hibernate_after_minutes`, `gpu`). With no `home`, new tabs and Home show
`wadbrowser://home`: a search box (none in Focus, which has no address
bar), under WadBrowser's logo on the machine's own WadBrowser. `WADBROWSER_DEVTOOLS=1` adds the inspector
(F12); `WADBROWSER_LOG=debug` says more.

## Building and trying it

- `cargo build -p wadbrowser` builds it for this laptop (Fedora's WebKitGTK).
- `images/build.sh --only base` builds it for workspaces, in a Debian trixie
  container (`images/builder/`), and bakes it into the base image.
- `spike/run.sh` runs a scripted session on a headless sway (screenshots of
  its windows, tabs moved between windows, a tab put to sleep and woken, a
  download, find in page): `cargo build -p wadbrowser --features spike` first.
  `WADBROWSER_SPIKE_SITES="https://… …"` checks real sites and what WebKit
  supports; `WADBROWSER_SPIKE_HOME=1` checks the home page and its search box.
- `spike/ipc.sh` checks the hand-off from a second launch, timed.
- `spike/instances.sh` checks two instances side by side, a setup's
  ephemeral profile leaving nothing behind, and a saved sign-in's cookie
  landing in its folder.
- `apps/wadd/dev/try-sets.sh` installs wadspaces with web apps through
  wadd and builds their sets.

## Inside wadspaces, and on the machine

The same program, built twice:
- **Inside wadspaces** (`images/build.sh`, no features): dark in WadSpaces'
  colours, always, with no logo; its own look, nothing of the machine's. Its
  icon is the light WadSpaces mark (`icons/`, from
  `assets/public/brand/wadspaces-icon-light-transparent.svg`).
- **On the machine** (`host/build.sh`, `--features host`): WadBrowser's own
  logo (`assets/public/brand/wadbrowser-logo-{dark,light}.svg`) on its
  home page, and light or dark as WadSpaces is: it follows the WadSpaces
  Client's `~/.config/wadspaces/theme` (src/theme.rs). The pages carry it as
  `data-theme` (WebKit's `prefers-color-scheme` follows the desktop's
  settings portal, not WadBrowser). `WADBROWSER_THEME=light|dark` overrides
  it, for the spike.
