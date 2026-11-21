# Packaging

| Format | Folder | Status |
|---|---|---|
| Flatpak | `flatpak/` | see the status table at the end |
| RPM (Fedora) | `rpm/` | see the status table at the end |
| AUR (`ironteams-git`) | `aur/` | see the status table at the end |
| Per-user install script | `install.sh` | builds with cargo, installs to `~/.local` |

Shared files, all named after the app id `io.github.mohamedeslam04.ironteams`: the desktop file (with a
"Report a problem" action), the icon (`.svg`) and the AppStream metadata (`.metainfo.xml`). Every
package installs them under that name.

## The app id

IronTeams uses one reverse-DNS id everywhere: the desktop file, icon and AppStream id, the Wayland app
id and X11 window class (`StartupWMClass`), the launcher-badge URI, the notification
`desktop-entry` hint, and the id it registers with the desktop portal
(`org.freedesktop.host.portal.Registry`) before the ScreenCast, RemoteDesktop and Camera
dialogs. The constant is `tray::APP_ID`; a test in `crates/ironteams/src/main.rs` checks the Flatpak
manifest, the desktop file and the metainfo against it.

The portal looks the id up with GLib (`GDesktopAppInfo`), which skips a desktop file whose `Exec`
program is not on the portal's `PATH`. `xdg-desktop-portal` runs as a systemd user service with
`PATH=/usr/local/bin:/usr/bin:/bin`, so a desktop file saying `Exec=ironteams` for a binary in
`~/.local/bin` is "not found" and the registration fails with `App info not found for
'io.github.mohamedeslam04.ironteams'` (KDE then names the dialogs after the terminal). The packages
install to `/usr/bin`, which is on that `PATH`. `install.sh` writes the full path of
`~/.local/bin/ironteams` into the per-user desktop file for the same reason.

## Native build dependencies

| Need | Why | Fedora | Arch | Freedesktop SDK |
|---|---|---|---|---|
| Rust 1.95+ (slint 1.19) | everything | `cargo rust` | `cargo` | `rust-stable` extension |
| C compiler, C++ compiler | bundled C code (aws-lc) | `gcc gcc-c++` | `gcc` | included |
| cmake | bundled C libraries (aws-lc, and libopus for the meeting host) | `cmake` | `cmake` | included |
| fontconfig | text rendering (`yeslogic-fontconfig-sys`) | `fontconfig-devel` | `fontconfig` | included |
| xkbcommon | keyboard input (Slint winit backend) | `libxkbcommon-devel` | `libxkbcommon` | included |
| dbus | Secret Service, notifications | `dbus-devel` | `dbus` | included |
| alsa-lib | meeting host audio (not linked yet) | `alsa-lib-devel` | `alsa-lib` | included |
| pkg-config | finds the above | `pkgconf-pkg-config` | `pkgconf` | included |

At run time the app needs a Secret Service provider (GNOME Keyring or KWallet) to keep the
sign-in, and benefits from `xdg-desktop-portal` and an emoji font.

## Flatpak

Needs `flatpak`, plus either `flatpak-builder` or the Flatpak `org.flatpak.Builder`
(`flatpak install --user flathub org.flatpak.Builder`). The runtime, SDK and `rust-stable` extension
(Freedesktop 26.08) are installed by the script when missing, roughly 2 to 3 GB.

```sh
packaging/flatpak/build.sh /data/ironteams-flatpak          # build only
packaging/flatpak/build.sh /data/ironteams-flatpak --install # build and install for this user
flatpak run io.github.mohamedeslam04.ironteams
```

The work directory holds the builder cache and the build tree, so keep it on a disk with free
space, not on tmpfs. The script generates `cargo-sources.json` (the vendored crate list, git-ignored)
from `Cargo.lock` with `flatpak-cargo-generator.py` pinned to a commit of
`flatpak/flatpak-builder-tools`, then runs the builder with no network inside the sandbox. See
`flatpak/README.md` for the permissions and why each one is there.

## RPM

`rpm/ironteams.spec` builds from a source tarball plus a tarball of vendored crates, so the build runs
without network (as Koji and COPR do). To build locally without root:

```sh
top=/data/ironteams-rpm; v=0.1.0
mkdir -p $top/{SOURCES,BUILD,RPMS,SRPMS,BUILDROOT} /data/ironteams-vendor
git archive --prefix=ironteams-$v/ -o $top/SOURCES/ironteams-$v.tar.gz HEAD
cargo vendor --locked --versioned-dirs /data/ironteams-vendor/vendor
tar -C /data/ironteams-vendor -c vendor | xz -T0 > $top/SOURCES/ironteams-$v-vendor.tar.xz
rpmbuild -bb --define "_topdir $top" packaging/rpm/ironteams.spec
```

The RPM lands in `$top/RPMS/x86_64/`. `rpmbuild` refuses to start when a `BuildRequires` package is
missing (`sudo dnf builddep packaging/rpm/ironteams.spec` installs them). If you use rustup instead of
the Fedora `cargo` and `rust` packages, add `--nodeps`. The `%check` section runs
`desktop-file-validate` and the `diag` crate tests.

## AUR

`aur/PKGBUILD` builds `ironteams-git` from the repository head. On Arch:

```sh
cd packaging/aur && makepkg -si
```

Regenerate `.SRCINFO` after changing the PKGBUILD with `makepkg --printsrcinfo > .SRCINFO`.
On other distributions, `packaging/aur/` can be checked in an Arch container, see below.

## Checking the AUR package without Arch

With rootless podman (no sudo), build `ironteams-git` from a local clone of the repository:

```sh
git clone --bare . /data/ironteams-src.git
podman run --rm -v /data/ironteams-src.git:/src.git:ro,Z -v $PWD/packaging/aur:/pkg:ro,Z \
  docker.io/library/archlinux:base-devel bash -c '
    pacman -Syu --noconfirm --needed git rust cmake alsa-lib fontconfig libxkbcommon \
      libxkbcommon-x11 dbus wayland libx11 libxcursor libxi libxrender libglvnd hicolor-icon-theme namcap
    useradd -m builder; mkdir /build; cp /pkg/PKGBUILD /pkg/.SRCINFO /build; chown builder /build; cd /build
    sed -i "s|git+\${url}.git|git+file:///src.git#branch=<your branch>|" PKGBUILD
    git config --system --add safe.directory "*"
    su builder -c "makepkg -f --noconfirm" && namcap PKGBUILD *.pkg.tar.zst'
```

It takes about 30 minutes and several GB of memory (limit cargo with `CARGO_BUILD_JOBS` if the OOM killer steps in). The container's `makepkg` also confirms `.SRCINFO`
(`makepkg --printsrcinfo | diff - .SRCINFO`, after undoing the `sed`).

## CI

`.github/workflows/packaging.yml` builds the RPM (Fedora container), the Flatpak (Flathub's
`flatpak-github-actions` image, Freedesktop 26.08) and the AUR package (Arch container). It never
runs on pull requests. It runs when started by hand (Actions tab), when `packaging/`, `Cargo.lock`
or the workflow change on `main`, and on a push to any branch named `ci/packaging-*`. A manual run
only works once the workflow is on the default branch, so to test a change to it from a branch:

```sh
git switch -c ci/packaging-test
date > packaging/.ci-trigger && git add -f packaging/.ci-trigger && git commit -m "ci: run packaging"
git push -u origin ci/packaging-test
gh run watch "$(gh run list --workflow=packaging.yml --branch ci/packaging-test --limit 1 --json databaseId -q '.[0].databaseId')"
gh run view <id> --log-failed        # when a job failed
git push origin --delete ci/packaging-test
```

The throwaway commit matters: the `paths` filter only sees files changed by commits that are new to
the repository, so pushing a branch whose commits are all on GitHub already starts nothing.

## Verification status

Last checked on 2026-11-19 (Fedora 44 host, Freedesktop 26.08, Arch container, GitHub Actions). Local
builds from commit `d31bc90`, CI from the `ci/packaging-test` run
[44516901149](https://github.com/MohamedEslam04/teams-in-rust/actions/runs/44516901149) (all three jobs green).

| Package | Result |
|---|---|
| Flatpak | Built with `build.sh` at `d31bc90` (Freedesktop 25.08, offline crate build, 1485 vendored sources). The manifest is now on 26.08 (`flatpak-builder-lint manifest` is clean) and builds in CI on the `freedesktop-26.08` image, with one job (`CARGO_BUILD_JOBS`). The metainfo has three screenshots (`docs/screenshots/appearance-*.png`, demo data); `appstreamcli validate --pedantic` passes offline and warns only that the images are not at the public URL yet (they resolve once the branch is on `main`). Local 26.08 build, 2026-11-19 (`build.sh`, wave-2 `4f4b35c`, runtime, SDK and `rust-stable` 26.08 installed): **BLOCKED on this host, not built, not launched.** The sources download and the dependency crates compile, then `earlyoom` sends SIGTERM to the `rustc` of the `ironteams` crate (6.9 GB resident) because the other work on the machine leaves it about 6 GB free and the swap is full. Three attempts: a crate download timeout, the same OOM kill, and the OOM kill again with `CARGO_PROFILE_RELEASE_LTO=off` (LTO is not the main cost). So `flatpak-builder-lint builddir` and the demo-mode launch were not run. Run them on a machine with about 10 GB free for the compile: `flatpak run --user --env=IRONTEAMS_DEMO=1 io.github.mohamedeslam04.ironteams`, then `flatpak-builder-lint builddir <work>/build-dir`. |
| RPM | `rpmbuild -bb` without `--nodeps` passes in CI (Fedora container with the `cargo` and `rust` RPMs, so the `BuildRequires` are real). Locally the `BuildRequires` are installed (`dnf builddep`, Fedora cargo/rust 1.99), but the local build was paused. At `d31bc90` the build with `--nodeps` gave a 15 MB `ironteams-0.1.0-1.fc44.x86_64.rpm` and the 27 `diag` tests passed. The first CI run was killed by the OOM killer (`rustc` SIGKILL): Fedora's `RUSTFLAGS` has `-Cdebuginfo=2`, so the spec now sets it to 0 (no `-debuginfo` package is built anyway). `rpmlint` on the spec: no errors, one warning (`invalid-url Source1`, the vendor tarball is a local file by design). Local `rpmbuild -bb` without `--nodeps` (Fedora 44, `/usr/bin/cargo` 1.99, `CARGO_BUILD_JOBS=2`), 2026-11-19, wave-2 `4f4b35c`: `%prep` (source and vendor tarballs recreated as above) and `BuildRequires` pass, but **the `%build` was killed three times by `earlyoom`** (SIGTERM to the `ironteams` crate's `rustc`, 5.6 to 6.4 GB resident, the host had about 6 GB free and a full swap). No RPM was produced locally, so `rpmlint` on the built package, `%check` and the install step are still open; CI is the passing evidence. `rpmlint packaging/rpm/ironteams.spec` (rpmlint 2.8.0) again: 0 errors, 1 warning (`invalid-url Source1`, expected). Retry with about 10 GB free. |
| AUR | `ironteams-git` built in an `archlinux:base-devel` podman container with `makepkg` (17 MiB package, 27 `diag` tests passed) and in CI (`makepkg` plus the `.SRCINFO` check). `namcap PKGBUILD` is clean. The `depends` entries for the dlopen-loaded libraries show as "may not be needed" in `namcap`, and are intentional. |
