# Images

What wadspaces run on. `base` and `stream` are built here; a wadspace's
apps are put together on the base by wadd, on the machine, from packages CI
builds.

| Image | What it is |
|---|---|
| `localhost/wadspaces-base:trixie` (`base/`) | The lean desktop (labwc, selkies-desktop's icons and panel with the WadSpaces logo, Xwayland), WadBrowser (the browser, every web app's window, and what links open in), and the helpers packages are captured with: `wadspaces-feature` (one script per app or tool, `base/root/usr/local/lib/wadspaces/features/`), `wadspaces-apt`, `wadspaces-webapp`, `wadspaces-vscode-extensions`, `wadspaces-layout`. Its window goes on whatever compositor is mounted at `/run/wadspaces-display`. `build.sh` builds WadBrowser for it first, in `builder/` (a Debian container with Rust and WebKitGTK's headers), and stages Dolphin from linuxserver's image. |
| `localhost/wadspaces-stream:trixie` (`stream/`) | The stream sidecar: linuxserver's Selkies (pinned), its compositor, encoder and web client behind nginx's login. A streamed wadspace draws on it through a display volume they share (the same `/run/wadspaces-display`), and plays its sound through it; wadd publishes it behind its proxy (`host/STREAMS.md`). It won't start without `CUSTOM_USER` and `PASSWORD`. |
| `localhost/wadspaces-pkg-<id>:<image id>` | A package: one app's, tool's or VS Code extension's files on their own, `FROM scratch` (`crates/wad-core/src/packages.rs`). CI builds them on the base (`.github/workflows/packages.yml`) and publishes them, privately, as `ghcr.io/hunggod/wadspaces-pkg-<id>:<abi>` with an index (`index.json` on the `packages` release); wadd gets one when a build needs it, or ahead of time (`GET /v1/packages`, `POST /v1/packages/fetch`): the one here, else downloaded with the machine's GitHub token, else built here. Named by image id, which the drive's copies keep. |
| `localhost/wadspaces-chain:<sha12>` | Packages stacked on the base, one per image, each `FROM` the one under it and named by the base and the packages so far (`packages::chain_ref`, labelled `io.wadspaces.chain`): a set build starts from the longest one stored, so taking an app out stacks again only what was above it. |
| `localhost/wadspaces-set-<key>:latest` | A wadspace's image, its set: its packages on the base (the top chain, then `wadspaces-pkg assemble`), named by its packages alone (`v4::set_key`), so every wadspace with the same apps, tools and extensions shares it. Labelled `io.wadspaces.layer="set:<key>"`, `io.wadspaces.from` (the base it was built on: a newer base makes it an update) and `io.wadspaces.pkgs`. A wadspace with no packages runs on the base. |

A wadspace's customization has no image: its saved sign-ins are handed over
when it starts, and its projects aren't mounted at all (but for the ones
picked to mount): the base's `wadspaces-projects` (with
`svc-wadspaces-projects`, in the background) clones its GitHub repositories
into `~/Desktop`, fast-forwards them at each start when that's safe, and
copies in what wadd copied from the machine; the base says so with
`io.wadspaces.projects="2"`. Its start-up log opens with the WadSpaces banner
(`base/root/etc/s6-overlay/s6-rc.d/init-adduser/branding`).

A wadspace (a version 4 document: `crates/wad-core/src/v4.rs`) is
installed with `PUT /v1/wadspaces/{id}` and its set built with
`POST /v1/wadspaces/{id}/build` (`apps/wadd/src/sets.rs`). Its web apps (with
the icons wadd made), desktop layout, WadBrowser's mode and background aren't
in any image: wadd writes them to its extra folder, and the base's
`init-wadspaces-workspace` takes them from `/run/wadspaces-extra` at each
start, so changing them never builds. The base says it does with
`io.wadspaces.extras="1"`. The background and the web apps' icons are drawn
in the machine's theme (`PUT /v1/machine/theme`): black and white textures,
icons acid on violet in the dark, black on white in the light.

```bash
images/build.sh                    # the base and the stream sidecar
images/build.sh --only stream      # just the sidecar
images/base/try-apps.sh            # every catalog app over the base, opened in a headless sway
images/base/try-signins.sh         # each sign-in kind's files kept across runs
apps/wadd/dev/try-projects.sh      # projects cloned and copied into a wadspace, fast-forwarded, a throwaway run
apps/wadd/dev/try-packages.sh      # packages captured and assembled, timed, in a podman store of its own
GITHUB_TOKEN=… apps/wadd/dev/try-fetch.sh [ids]  # wadd downloading packages from CI's registry, in a store of its own
apps/wadd/dev/try-sets.sh          # version 4 wadspaces' sets built by wadd, timed, in a store of its own
```

A machine gets the base and the stream sidecar from its drive: `host/build.sh
install` brings them, and `host/build.sh update --bases` updates them. Sets are built on the
machine the first time a wadspace needs one.
