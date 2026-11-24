# IronTeams

IronTeams — an unofficial native Microsoft Teams client written in Rust.

**Website:** [mohamedeslam04.github.io/teams-in-rust](https://mohamedeslam04.github.io/teams-in-rust/) has the downloads for every system and the install guides.

A lightweight native desktop client for Microsoft Teams on Linux, written in Rust with [Slint](https://slint.dev).
Formerly called kelma; the repository was `MohamedEslam04/kelma` before it became `teams-in-rust`.

The Teams web app takes around 1 GB of RAM. IronTeams gives you the same chat experience in a familiar
Teams-like layout and uses a fraction of that memory: see [Memory](#memory).

![Chat](docs/screenshots/chat.png)

> **Unofficial.** Microsoft does not affiliate with, endorse or support IronTeams. "Microsoft Teams"
> is a trademark of Microsoft Corporation. See the [disclaimer](#disclaimer).

## Features

- **Sign-in:** sign in with a device code (works with MFA); the code's address opens in the browser
  while sign-in waits. The refresh token lives in your system keyring (KWallet or GNOME Keyring),
  never in a file. Several accounts and guest organizations.
- **Chat:**
  - 1:1, group, meeting and self-notes chats, with custom sections, filters and list options
  - rich text both ways: a formatting toolbar (bold to tables, colors and code blocks) and Teams' shortcuts
  - @mentions, reply quotes, forward, pin, save, mark unread, mark as important, drafts and an offline outbox
  - new chats and group management, profile cards, read receipts, pinned-message banner, and
    Notify when available for the person of a 1:1 chat
  - image grid and viewer, link previews, translation, voice messages and videos
  - a message's right-click menu also has Copy image, Open in Browser, Download and Share for its
    pictures, and Open file, Download file, Copy file link and Share file for its files
  - bot and workflow cards (Adaptive Cards), and the composer's apps: message extension search,
    action commands (their page opens in the web host) and sending the resulting cards
  - Arabic, English and mixed-direction text, each line of a message in its own direction, kept in
    the sent message so Teams shows it the same way
- **Find:** a search page (messages, files, people, meetings), find in chat, Mentions, Followed
  threads and Saved quick views, the Shared tab and a chat details pane.
- **Activity:** the Teams activity feed with filters, a preview pane, mark all as read and badges
  that follow what you read in other Teams clients.
- **Teams and channels:** posts as cards with replies, announcements, follow thread, channel files,
  plus a management page: discover and join, channels, members, tags, email address, notifications.
- **Calendar:** day, week and month views, event peek and full view, the event form with
  recurrence, the scheduling assistant, rooms, proposals, filters, share and print.
- **OneDrive:** browse, For you, Shared, People, Media and Meetings views, create and upload,
  share dialog, recycle bin and a native PDF and Office preview.
- **Apps:** the app bar, the Store, Planner and To Do, Workflows, OneNote and Lists (read-only),
  a Viva Engage community feed (read-only), and app tabs in a web host window that only gets
  the camera, microphone, location, notifications and links you allowed the app.
- **Calls and meetings:** a native call window with pre-join, meeting controls and the roster,
  1:1 incoming and outgoing call screens (`calls` feature, on by default; `crates/calling`).
  The microphone gets echo cancellation and noise suppression that also removes keyboard and
  mouse clicks; the camera sends 720p and lowers its quality when the machine falls behind;
  background blur and pictures in the Video effects panel. The People panel has a search box, a
  Share invite button (copies the join link), and sections for who is in the meeting, who was
  invited but has not joined (Declined or No response) and who is only in the meeting chat;
  clicking a person there or a participant's tile opens their profile card, and the speaker strip
  scrolls to show everyone. Pre-join offers Computer audio or Don't use audio; when your account is
  also in the call on another device, a banner says so and the call's sound can be turned off or
  back on here.
- **Settings:** theme (light, dark, system), accent, density and text size; app language with
  Arabic for the main screens; date and time format and time zone; notifications; devices;
  privacy; files and links (open Office files in the browser, the desktop app or IronTeams);
  app permissions; accounts; accessibility; an opt-in update check.
- **Desktop:** your status follows your calls and goes Away after five idle minutes; tray icon
  with unread badge, close to tray, autostart, window state, desktop
  notifications, a local redacted problem report (`ironteams-cli diag`).
- **Live:** real-time updates over Teams' push channel, automatic reconnect and catch-up.


## Install

You need Rust (via [rustup](https://rustup.rs)) and the development headers for fontconfig,
xkbcommon and D-Bus.

```sh
# Fedora: sudo dnf install fontconfig-devel libxkbcommon-devel dbus-devel
# Debian/Ubuntu: sudo apt install libfontconfig1-dev libxkbcommon-dev libdbus-1-dev pkg-config
git clone https://github.com/MohamedEslam04/teams-in-rust && cd teams-in-rust
./packaging/install.sh      # builds and installs to ~/.local (binary, .desktop file, icon)
```

### Packages

| Format | Where | Notes |
|---|---|---|
| Flatpak | `packaging/flatpak/` | sandboxed: network, Wayland/X11, PipeWire, notifications and the Secret Service only |
| Arch (AUR) | `packaging/aur/PKGBUILD` | the `-git` package, built from the latest commit: `makepkg -si` |
| RPM (Fedora) | `packaging/rpm/ironteams.spec` | builds offline from vendored crates |

Each folder says how to build the package. None of them is published to a repository yet, and
updates come from your package manager. For these Settings › About has an opt-in check (off by
default) that asks GitHub for the latest release once per start and never downloads anything.

Start **IronTeams** from your launcher. On first start it shows a code: open the link, enter the code
and sign in with your work account.

### Windows, macOS and the AppImage

Tagged releases (`v*`, `.github/workflows/release.yml`) build a Linux AppImage, next to the
tarball, RPM and Flatpak. A Windows `Setup.exe` (a per-user install, no administrator needed) and a
portable zip, and a macOS `.pkg` for Apple Silicon and for Intel, are added to the same release
afterwards (`.github/workflows/release-add.yml`), so a release may have them a little later than
the Linux files, or not at all. To build yourself:
`cargo build --release -p ironteams` with Rust and CMake (Windows: the MSVC build tools; macOS:
the Xcode command line tools). The installers are not signed yet: Windows SmartScreen asks once
("More info › Run anyway"), and on macOS the `.pkg` opens with right click › Open.

### Updates

Copies installed from `Setup.exe`, the `.pkg` or the AppImage update themselves
([Velopack](https://velopack.io)):

- About 45 seconds after the start, then every 6 hours, IronTeams looks at the GitHub releases
  for a newer version. It downloads it in the background, and only the parts that changed (a
  delta package) when it can. Velopack checks the size and SHA-256 of every download, and the
  feed is only read over HTTPS from github.com.
- Nothing is installed while you work: a banner says "IronTeams <version> is ready" with
  **Restart to update**; if you don't click it, the update is installed when you quit. If a
  download or the install fails, the current version keeps running and the next check tries
  again.
- Before a new version first opens its data, the message caches and settings are copied to
  `backup-<old version>` in the data folder (the last two are kept).
- If a new version fails to start three times in a row, automatic updates pause and a banner
  offers to go back to the version before (downloaded only when you click).
- Settings › About › Updates: **Automatic updates** (on), **Get beta updates** (off),
  **Check now**.

Copies from the tarball, RPM, Flatpak, AUR or `cargo` never replace themselves. The first
Velopack release has to be installed by hand once (also by users of an older MSI or `.dmg`);
every release after it arrives on its own.

To ship a release: `git tag vX.Y.Z && git push --tags`. The release workflow runs fmt, clippy and
every test on Linux, builds the Linux packages, starts the packaged AppImage with
`--self-check --packaged`, and only then publishes the release. To add Windows or macOS to it:
`gh workflow run release-add.yml -f tag=vX.Y.Z -f platform=windows` (or `platform=macos`). That
runs every test on that system, builds and smoke-tests its packages from the tag's commit, and
only then uploads that system's files to the existing release; it never creates a release or
replaces a file. Until then, installed Windows and macOS copies simply see no update (Velopack
skips releases without their feed). Setting the repository variable `RELEASE_ADD_PLATFORMS` to
`windows macos` makes every tag add both right after the Linux release. A tag like
`vX.Y.Z-beta.1` is published as a GitHub pre-release: only copies with **Get beta updates** on
take it, so try a beta on your own machine first, then tag the stable version.

Signing is optional in the workflow: it signs when the repository has the secrets listed at the
top of `release.yml` (used by `release-add.yml`), and warns otherwise. Free Windows signing for open-source projects:
[SignPath Foundation](https://signpath.org). macOS notarization needs an Apple Developer ID.

## Platforms

CI (`ci.yml`) runs clippy and the whole test suite on Linux on every push. Windows (x86_64) and
macOS (Apple Silicon and Intel) run the whole test suite on a manual CI run, and before
`release-add.yml` adds that system to a release. The runners have no screen, camera or
microphone, so what needs one is only checked to build there. Daily use is on Linux (KDE Plasma, Wayland); nothing
has been tried on real Windows or macOS hardware yet.

| | Linux | Windows | macOS |
|---|---|---|---|
| Chat, channels, calendar, files, search, settings | works | builds, tests pass | builds, tests pass |
| Sign-in token storage | Secret Service (KWallet, GNOME Keyring) | Credential Manager | login Keychain |
| Desktop notifications | works (actions, markup) | toasts with buttons | banners, no buttons |
| Tray icon with unread badge | StatusNotifierItem + launcher badge | Slint tray icon, compile-only | Slint menu-bar icon, compile-only |
| Start at login | XDG autostart | Run key | LaunchAgent |
| Spell check | system Hunspell | bundled en_US and ar | bundled en_US and ar, `~/Library/Spelling` |
| Voice messages: record, play | works | compile-only (cpal/WASAPI) | compile-only (cpal/CoreAudio) |
| Voice messages: encode | ffmpeg | Media Foundation (tested in CI), else ffmpeg | AudioToolbox (tested in CI) |
| Call audio | works | compile-only | compile-only |
| Camera | Camera portal / V4L2 through GStreamer | compile-only: GStreamer `mfvideosrc`, needs the GStreamer runtime | compile-only: GStreamer `avfvideosrc`, needs the GStreamer runtime |
| Screen sharing | ScreenCast portal through GStreamer | compile-only: main screen via `d3d11screencapturesrc`, no picker, needs GStreamer | compile-only: main screen via `avfvideosrc`, no picker, needs GStreamer |
| Remote control of a share | RemoteDesktop portal | not yet (the call window says so) | not yet (the call window says so) |
| Video clips | ffmpeg | needs ffmpeg installed | needs ffmpeg installed |
| Web apps (web host) | Chromium-family browser | Edge or Chrome, compile-only | Chrome, Edge, Brave… in `/Applications`, compile-only |
| Screen-locked presence (Away) | `org.freedesktop.ScreenSaver` | not detected | not detected |

The AAC encoder: IronTeams is GPL-3.0-or-later and must not link fdk-aac, so Linux runs the system
`ffmpeg`, while Windows and macOS use the encoder that comes with the system (Media Foundation is
loaded at run time, so Windows N editions without the Media Feature Pack still start, and fall
back to ffmpeg). GStreamer is not bundled: install the runtime from
[gstreamer.freedesktop.org](https://gstreamer.freedesktop.org/download/) for camera and screen
sharing on Windows and macOS (`IRONTEAMS_GST_LAUNCH` points at another `gst-launch-1.0`).

### Environment variables

| Variable | Effect |
|---|---|
| `IRONTEAMS_RENDERER=femtovg` | GPU renderer instead of the default software renderer (uses more memory); beats the choice in Settings > General > Performance, see `docs/perf.md` |
| `IRONTEAMS_LOG=ironteams=debug,teams_proto=info` | More detail in the log (`logs/` in the data folder; also printed to the terminal). The default is `ironteams=info,calling=info`, warnings from the rest, with WebRTC's duplicate-packet warnings left out. Tokens are never logged |
| `IRONTEAMS_DEMO=1` | Offline demo with sample data: no account, no network |
| `IRONTEAMS_HOME=<dir>` | Keep config, cache and data under `<dir>` |

The old `KELMA_*` names still work when the `IRONTEAMS_*` one is not set. On the first start after the
rename, the old `kelma` config, data and cache folders are moved to `ironteams`, and a saved sign-in
under the old keyring name is moved to the new one when it is first read, so you stay signed in.

### Report a problem

IronTeams collects no telemetry and never uploads anything. If something breaks, write a local
bundle and attach it to an issue yourself after reading it:

```sh
ironteams --report-problem              # also in the launcher's right-click menu
ironteams-cli diag                      # the same bundle from the terminal; --log FILE adds another log
```

The `.tar.gz` goes to your Downloads folder (`--out PATH` changes that). It holds the IronTeams
version, OS and desktop facts, the non-secret config and the logs (the app keeps its own in the data folder). Tokens, cookies,
e-mail addresses, MRIs, conversation ids, GUIDs (tenant ids included), tenant domains and
user names are replaced by placeholders. The keyring and the message cache are never read.

## Memory

Measured from `/proc/<pid>/smaps_rollup` (PSS, which counts shared memory only once, plus
swapped-out pages where noted), release build, KDE Plasma on Wayland, Fedora 44:

| Client | Memory |
|---|---|
| Teams PWA in a Chromium browser | ~1100 MB |
| Pidgin + purple-teams | ~147 MB |
| **IronTeams**, real work account, chat with images open | **~62 MB** (PSS 8 + swapped 54) |
| **IronTeams**, real work account, 10 min idle, chat list only | **~29 MB** PSS (swap not counted) |
| IronTeams demo mode | 39 MB |

IronTeams keeps the conversation list and messages in an SQLite cache. Only the visible part of the
message list exists in memory (virtualized list), and images are downscaled when they are decoded.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
IRONTEAMS_DEMO=1 cargo run -p ironteams
cargo run -p ironteams-cli -- login | chats | teams | read <id> | send <id> <text> | tail
```

| Crate | Role |
|---|---|
| `crates/teams-proto` | Protocol: auth broker, chat service, channels and teams, search, activity, cards, real-time |
| `crates/render` | Teams HTML to a rich-text model and back, with per-paragraph text direction |
| `crates/store` | SQLite cache, drafts, outbox and saved messages |
| `crates/session` | Keyring, accounts and config persistence |
| `crates/ironteams` | The desktop app (Slint UI and engine) |
| `crates/ironteams-cli` | Command-line client for protocol work |
| `crates/calendar` | Outlook calendar over Microsoft Graph |
| `crates/onedrive` | OneDrive and SharePoint files |
| `crates/apps` | App bar, installed apps, the Store, bots and message extensions |
| `crates/calling` | Native calling: signalling, push, WebRTC media, audio |
| `crates/webhost` | Chromium `--app` window over CDP with a TeamsJS host shim |
| `crates/preview` | PDF pages to images, Office files to PDF |
| `crates/tray` | Tray icon, unread badge, autostart |
| `crates/appearance` | Theme, accent, density, font scale and accessibility preferences |
| `crates/devices` | Audio devices and cameras for settings |
| `crates/diag` | Redaction and the local problem report |
| `crates/emoji` | Emoji table, search, shortcodes and skin tones |

[`docs/protocol.md`](docs/protocol.md) documents every endpoint and what has been verified.
[`docs/mcp.md`](docs/mcp.md) explains `ironteams-cli mcp`, an MCP server that lets Claude Code or Claude
Desktop read your chats and calendar and draft replies you approve. It is not in release builds (`--features mcp`)
and is off until you allow an account.
The ADRs in [`docs/adr/`](docs/adr/) explain the toolkit, the web host and the composer choices.

Parallel builds on a small machine: run cargo through `flock /tmp/ironteams-build.lock cargo …` so only one
build compiles the large `ironteams` crate at a time. GitHub CI runs fmt, clippy and the whole test
suite on Linux on every push to `dev`, `feat/**` and `fix/**` and on pull requests.

Translations: interface strings are wrapped in `@tr()` and compiled in from
`crates/ironteams/translations/<lang>/LC_MESSAGES/ironteams.po`.

## Disclaimer

IronTeams talks to the same internal endpoints the Teams web client uses. They are undocumented and
Microsoft can change them at any time, which would break IronTeams. Your organization's policies may
not allow third-party clients, so check with your IT team before you use IronTeams with a work account.
Use it at your own risk.

On first run IronTeams downloads Microsoft Teams' notification and ringtone sounds from Microsoft's CDN
(the files Teams web plays). They are not part of IronTeams and not shipped with it; turn this off in
Settings › Notifications › "Download Teams' original sounds", and IronTeams plays its own sounds instead.

In the same way, the Video effects panel downloads Microsoft Teams' background pictures (the ones the Teams web
app offers) from Microsoft's CDN into IronTeams' data folder (`backgrounds/teams/`): the small previews after
sign-in, and a full picture when you choose it. They are not part of IronTeams and not shipped with it. The
panel's other backgrounds are drawn by IronTeams itself.

## Credits

- Protocol knowledge from [purple-teams](https://github.com/EionRobb/purple-teams) by Eion Robb (GPL-3.0).
- Color emoji: [Twemoji](https://github.com/mozilla/twemoji-colr) (CC-BY 4.0). See `crates/ironteams/ui/fonts/README.md`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Docs synced to commit 705bbad5 ("Merge fix/ams-asyncgw-fallback: media downloads retry on the asyncgw host when the AMS host fails", 2026-11-24)
