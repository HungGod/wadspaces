# Images

What wadspaces run on. `base` and `stream` are built here; the Builder's
layers are built on the base by wadd, on the machine.

| Image | What it is |
|---|---|
| `localhost/wadspaces-base:trixie` (`base/`) | The lean desktop (labwc, selkies-desktop's icons and panel with the WadSpaces logo, Xwayland), WadBrowser (the browser, every web app's window, and what links open in), and the helpers layers build with: `wadspaces-feature` (one script per app or tool, `base/root/usr/local/lib/wadspaces/features/`), `wadspaces-apt`, `wadspaces-webapp`, `wadspaces-vscode-extensions`, `wadspaces-layout`. Its window goes on whatever compositor is mounted at `/run/wadspaces-display`. `build.sh` builds WadBrowser for it first, in `builder/` (a Debian container with Rust and WebKitGTK's headers), and stages Dolphin from linuxserver's image. |
| `localhost/wadspaces-stream:trixie` (`stream/`) | The stream sidecar: linuxserver's Selkies (pinned), its compositor, encoder and web client behind nginx's login. A streamed wadspace draws on it through a display volume they share (the same `/run/wadspaces-display`), and plays its sound through it; wadd publishes it behind its proxy (`host/STREAMS.md`). It won't start without `CUSTOM_USER` and `PASSWORD`. |
| `localhost/wadspaces-template-<id>:latest` | A template: `FROM` the base, with its apps. |
| `localhost/wadspaces-template-<id>:latest` built on another | A template that builds on another (its `parent`): `FROM` that template's image, adding its own apps. |
| `localhost/wadspaces-custom-<id>:latest` | An Extensions layer: `FROM` its template's image, with tools, VS Code extensions and web apps. |
| `examples/` | Two real wadspaces' layers as documents (Cosmic Bodybuilding on Writing; VanuaAcademy on Extensions on Dev), which `--example` builds as wadd would. |
| `localhost/wadspaces-<id>:latest` | A wadspace's own image from before the layers; a migrated one runs it until its layers are built. |
| `localhost/wadspaces-pkg-<id>:<image id>` | A package: one app's, tool's or VS Code extension's files on their own, `FROM scratch` (`crates/wad-core/src/packages.rs`). CI builds them on the base (`.github/workflows/packages.yml`) and publishes them, privately, as `ghcr.io/hunggod/wadspaces-pkg-<id>:<abi>` with an index (`index.json` on the `packages` release); wadd gets one when a build needs it, or ahead of time (`GET /v1/packages`, `POST /v1/packages/fetch`): the one here, else downloaded with the machine's GitHub token, else built here. Named by image id, which the drive's copies keep. |

A wadspace's own layer (its Customization) has no image: its wallpaper and
saved sign-ins are handed over when it starts, and its projects aren't mounted
at all: the base's `wadspaces-projects` (with `svc-wadspaces-projects`, in the
background) clones its GitHub repositories into `~/Desktop`, fast-forwards
them at each start when that's safe, and copies in what wadd copied from the
machine; the base says so with `io.wadspaces.projects="2"`. Its start-up log
opens with the WadSpaces banner (`base/root/etc/s6-overlay/s6-rc.d/
init-adduser/branding`). Layer images carry labels wadd reads:
`io.wadspaces.layer`, `io.wadspaces.dockerfile-sha256` (the build folder's
sha, so a changed layer shows as changed) and `io.wadspaces.from` (the image
it was built on). wad-core writes their build folders
(`crates/wad-core/src/layers.rs`), and wadd builds them
(`POST /v1/images/build`), what a layer builds on first.

```bash
images/build.sh                    # the base and the stream sidecar
images/build.sh --only stream      # just the sidecar
images/build.sh --example vanua-academy  # an example's layers (the base first, if missing)
images/base/try-apps.sh            # every catalog app over the base, opened in a headless sway
images/base/try-signins.sh         # each sign-in kind's files kept across runs
apps/wadd/dev/try-layers.sh        # Extensions (their template first) and a template on a template built by wadd, a stack installed on them
apps/wadd/dev/try-projects.sh      # projects cloned and copied into a wadspace, fast-forwarded, a throwaway run
apps/wadd/dev/try-packages.sh      # packages captured and assembled, timed, in a podman store of its own
GITHUB_TOKEN=… apps/wadd/dev/try-fetch.sh [ids]  # wadd downloading packages from CI's registry, in a store of its own
```

A machine gets the base and the stream sidecar from its drive: `host/build.sh
install` brings them, and `host/build.sh update --bases` updates them. Layer images are built on the
machine the first time something needs them.
