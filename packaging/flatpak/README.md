# Flatpak

`io.github.mohamedeslam04.ironteams.yml` builds IronTeams against the Freedesktop 26.08 runtime with the
`rust-stable` SDK extension.

## Permissions

| Permission | Why |
|---|---|
| `--share=network` | Teams, Graph and OneDrive |
| `--socket=wayland`, `--socket=x11`, `--share=ipc` | window system; X11 also for the notification cards through XWayland where the compositor has no layer shell (GNOME) |
| `--device=dri` | optional GPU renderer (`IRONTEAMS_RENDERER=femtovg`) |
| `--socket=pulseaudio`, `--filesystem=xdg-run/pipewire-0` | call audio, and the PipeWire streams of the Camera and ScreenCast portals |
| `--talk-name=org.freedesktop.Notifications` | desktop notifications |
| `--talk-name=org.kde.StatusNotifierWatcher`, `--own-name=org.kde.StatusNotifierItem-2-1` | the tray icon (the item is named after the pid, 2 in the sandbox) |
| `--talk-name=org.freedesktop.ScreenSaver` | the screen-lock check that shows you Away while the screen is locked |
| `--talk-name=org.freedesktop.secrets` | the refresh token is kept in the Secret Service |
| `--filesystem=xdg-download` | saved attachments and problem reports |

The file chooser, open-URI, camera and screen-cast portals need no permission. Inside the sandbox the
portal names IronTeams by its Flatpak id, so the app's own registration (`register_host_app`) is
skipped there.

## Known gaps

- Recording voice messages runs `ffmpeg` to encode the recording to AAC, and the
  freedesktop runtime has no `ffmpeg` program, so recording says "Install ffmpeg" inside the
  sandbox. Playing voice messages works (it is pure Rust). Bundling an ffmpeg module built with
  only its native `aac` encoder would close the gap.

## Build

The sandbox has no network during the build, so the crates are vendored from `Cargo.lock`.
`build.sh` does all of it: it generates `cargo-sources.json` (git-ignored) with
`flatpak-cargo-generator.py` from `flatpak/flatpak-builder-tools` (pinned to a commit), then runs
`flatpak-builder` (or the `org.flatpak.Builder` Flatpak) and pulls in the runtime, SDK and
`rust-stable` extension when they are missing.

```sh
packaging/flatpak/build.sh /data/ironteams-flatpak --install   # work dir on a disk with free space
flatpak run io.github.mohamedeslam04.ironteams
```

By hand, from this folder:

```sh
flatpak install flathub org.freedesktop.Platform//26.08 org.freedesktop.Sdk//26.08 \
  org.freedesktop.Sdk.Extension.rust-stable//26.08
python3 flatpak-cargo-generator.py ../../Cargo.lock -o cargo-sources.json
flatpak-builder --user --install --force-clean --state-dir=/data/ironteams-flatpak/state \
  /data/ironteams-flatpak/build-dir io.github.mohamedeslam04.ironteams.yml
```

Keep the build directory outside the source tree: the manifest copies the whole repository.
Check the metadata with `appstreamcli validate --no-net ../io.github.mohamedeslam04.ironteams.metainfo.xml`
(the desktop file, icon and metainfo are shared with the other packages, one folder up).
