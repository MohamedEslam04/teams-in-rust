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
  never in a file. Several accounts and guest organizations. When a Conditional Access or
  security policy blocks the sign-in (non-compliant device, approved apps only, security
  defaults), the error says so by name instead of showing a bare code.
- **Accounts:** the profile menu lists your accounts (photo or initials, name, sign-in name and the
  unread count each one had when you last used it, a check on the active one); the active
  account's organizations sit under it, and Add account and Manage accounts open Settings ›
  Accounts. Picking another account switches by restarting IronTeams on it, which takes a moment
  because each account is a separate session. If the switch would cut something short (a call
  that is on or ringing, a voice message being recorded, messages still waiting in the outbox,
  uploads in progress), a dialog lists it and Cancel keeps everything as it was; your drafts are
  saved either way. Each account reopens on its own last chat, channel and collapsed sections and
  keeps its own media cache; window size, sidebar width and the call filmstrip position are shared.
  IronTeams starts from the saved chats at once and signs in in the background, and shows
  "Offline — showing saved messages" only when Teams cannot be reached. A photo or team picture
  asked for before the first sign-in finishes is asked for again instead of leaving initials until
  the next start (people without a photo are not asked again). Starting IronTeams a second
  time raises the running window instead of opening another one. `--account label[@tenant]` starts
  on a given account or organization (see [Command line](#command-line)).
- **Chat:**
  - 1:1, group, meeting and self-notes chats, with custom sections (each sortable by recent activity, A to Z or unread first), filters and list options
  - rich text both ways: a formatting toolbar (bold to tables, colors and code blocks) and Teams' shortcuts
  - @mentions, reply quotes, forward, pin, save, mark unread, mark as important, drafts and an offline outbox;
    editing a message changes only your own text and keeps its forwarded or quoted part
  - new chats and group management, profile cards, read receipts (in a group, every older message
    of yours that someone read shows an eye mark with the reader count, and a click lists the
    readers), pinned-message banner, and Notify when available for the person of a 1:1 chat; apps
    and bots do not count in a chat's derived title or member count
  - a chat with a "New messages" divider opens at the divider, and the "New messages" pill no
    longer covers the newest message; the in-app message card shows the whole message (up to 4000
    characters) in a box you can scroll with the wheel, also on the desktop card on Wayland, and
    the wheel over a short card scrolls the list behind it
  - image grid and viewer, link previews, translation, voice messages and videos; a forwarded
    picture or GIF shows as a picture
  - the text and every attached image go out as one message: paste (Ctrl+V), drop or attach an image
    while you type and it appears in the text at the cursor, on a line of its own with an X
    (Backspace or Delete on it removes it), and the message is sent in that order. Files stay as
    chips above the text, and an edit takes no images. A pasted image stays with its chat while
    IronTeams runs, not across a restart
  - hover one of your own messages and press E (or choose Edit) to edit it in place: the message
    turns into an editor that is the full composer (formatting, @mentions, emoji, snippets, spell
    check), opened with the message's formatting. Enter saves (Ctrl+Enter when "Enter sends" is
    off), Esc cancels. E only edits when the focus is outside text boxes or in an empty composer;
    otherwise it types an "e"
  - unread chats: Alt+Shift+Down and Alt+Shift+Up open the next and previous unread chat in list
    order and wrap around, never landing on the open chat (Alt+Down and Alt+Up stop at the ends); the
    Unreads view (first row of the list's Quick views, `/unreads` in the top bar, Alt+Shift+U) lists
    every unread message IronTeams has stored, grouped by chat with Mark as read, Mark all as read,
    Up and Down or K and J to move, Enter to open the message and R to mark its chat read
  - mute a chat for 1 hour, 8 hours, until tomorrow 8:00, 1 week or until you turn it off. Teams has no
    end time for a mute, so IronTeams keeps it on this computer and unmutes when it passes (at start
    and while it runs); the menu shows "Unmute (muted until 14:30)"
  - Later: one page for your pending reminders and saved messages (from the Later pill in the Activity
    header or Open Later in the command palette), overdue or soon-due reminders first. Open jumps to the
    message, Mark done cancels the reminder or unsaves the message, and reminders can be snoozed for an
    hour or until tomorrow 9:00
  - a private note per chat, in the chat details pane under "Private note": plain text, each line in its
    own direction, kept on this computer and never sent to Teams (a note icon shows in the chat header
    while the chat has one)
  - split view: More chat options (…) and the chat list's right-click menu have Open in split view, which
    shows a second chat right of the open one with its own header, messages and message box. Drag the
    divider, click the pane to make it active (Esc closes it, Alt+Shift+R goes to its box). The pane
    has the pop-out window's features (reactions, links, pictures, a plain-text box; no message menu,
    attachments or formatting), takes chats only, hides when the window is narrow and is not kept across
    a restart
  - emoji follow Teams' own catalog (about 1,650 emoticons with skin tones): reactions such as
    `muscle-tone1`, emoticons in messages, previews and notifications show as Teams shows them,
    and the reaction picker sends the same ids as the official client and finds emoji by name or
    shortcut (`flex`, `(flex)`)
  - a message's right-click menu also has Copy image, Open in Browser, Download and Share for its
    pictures, and Open file, Download file, Copy file link and Share file for its files
  - bot and workflow cards (Adaptive Cards), and the composer's apps: message extension search,
    action commands (their page opens in the web host) and sending the resulting cards
  - Arabic, English and mixed-direction text, each line of a message in its own direction, kept in
    the sent message so Teams shows it the same way; in the composer, the arrow keys move visually
    in an Arabic line, Home and End go to the start and end of the line and Ctrl+Home and Ctrl+End
    to the start and end of the text, the cursor shows at the start of an Arabic line, and Ctrl+A
    selects every line. Reply quotes name the quoted author instead of "Display Name"
- **Find:** a search page (messages, files, people, meetings), find in chat, Mentions, Followed
  threads and Saved quick views, the Shared tab and a chat details pane. Choosing a person or chat
  in the top search opens it at once, and what you type cannot reach the previous chat's draft.
- **Activity:** the Teams activity feed with filters, a preview pane, mark all as read and badges
  that follow what you read in other Teams clients.
- **Teams and channels:** posts as full-width cards (a title, headings, bullet and numbered lists,
  each line in its own direction) with replies, announcements, follow thread, channel files,
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
  - The call screen puts a shared screen on its own content stage and the people in a filmstrip
    beside it, below it, or hidden (View menu; your choice is kept). A camera never takes the
    share's place. Pin a person to keep them on the stage. Full screen is immersive: the control
    bar gives way to a floating bar (Mute, Camera, Share, Exit, Leave) and Esc leaves it, even
    while you control a shared screen. In a call, Ctrl+Shift+M mutes, Ctrl+Shift+O turns the
    camera on or off, Ctrl+Shift+K raises or lowers your hand, Ctrl+Shift+E starts or stops
    sharing and Ctrl+Shift+H or Ctrl+Shift+B leaves (listed under "During a call" in Shortcuts).
  - Remote video is drawn sharper (area-averaged downscale, up to 1280 pixels wide) and asks for a
    new keyframe after packet loss. In a meeting, the video IronTeams asks for follows each tile's
    role: 720p for the stage, 360p for the gallery, 180p for the filmstrip, and nothing for
    people who are off screen. If the service refuses a size, or a resized picture stays empty,
    the call falls back to 360p for everyone for the rest of the call.
    `IRONTEAMS_VIDEO_SUBSCRIBE=static` keeps the older latest-speaker selection.
  - 1:1 calls: Opus first, then G.711 (PCMU and PCMA) and telephone events, so voicemail and
    phone-style endpoints, which do not take Opus, are offered a codec they know; a call from a native Teams desktop client is answered
    correctly; a peer's shared screen shows in 1:1 calls that the media server hosts and when a
    share starts again on a new line, and stopping your own share no longer leaves the peer's
    share waiting. Meeting gallery slots survive a media restart (lobby admission) and a camera
    picture shows only in its sender's tile. Quitting IronTeams leaves the call on the service,
    and the call window's microphone meter can no longer freeze the window. In a 1:1 call the
    camera stays on from the click until you turn it off, a media update lost to a dropped
    connection is sent again, and a camera another app holds says so instead of failing quietly.
    Hold, resume, camera and screen-share changes in a 1:1 call go out one at a time and their answers
    are matched to the right change (an answer could land on the wrong one and turn the camera off),
    a hold during a media restart waits instead of failing, and sending one no longer stalls the call.
- **Settings:** theme: a gallery of ten built-in themes (IronTeams Dark and Light, Dusk, Harbor,
  Moss, Sandstone, Ember, Glacier, Graphite, Orchard), shown as one theme, a light and a dark theme
  that follow the system, or a light and a dark theme on a schedule (switched within a minute of
  each time); the accent colour (for IronTeams Dark and Light), density and text size; Reset theme
  in the command palette (Ctrl+Shift+P) or `--reset-theme`; a theme for each account
  (Accounts page) and one for calls and meetings (Appearance page); app language with
  Arabic for the main screens; date and time format and time zone; notifications; devices;
  privacy; files and links (open Office files in the browser, the desktop app or IronTeams);
  app permissions; accounts; storage (the active account's media and the total over all accounts,
  with Clear media for the active account only); accessibility; updates (always on).
- **Make it yours:**
  - Settings › Chats: a Chat list section with the list's view options (message previews, time
    stamps, chats and channels together, New section; with previews off, rows are one line) and Quick
    reactions, six slots for the message hover bar that you can replace or add to (Reset to default
    brings back like, heart, laugh and surprised)
  - Settings › General › Start: open Activity, Chat (the default), Teams, Calendar or where you left off
    when IronTeams starts
  - Settings › Appearance › Messages: Message style (Bubbles, Flat with a line per message like a
    chat app, or Compact with tighter spacing and a smaller photo) and switches for Show avatars, Show
    time stamps (the time shows again while the pointer is on the message) and Show reactions (the
    reactions stay on the message and you can still react from the hover bar)
  - Settings › Keyboard shortcuts: 27 window shortcuts with their keys; click the keys of a row, press
    the new combination, Esc cancels, Reset puts one back and Reset all shortcuts all of them. A key is
    refused, with the reason, when another shortcut has it, when it is a plain letter or Shift+letter,
    or when it belongs to text editing, the call keys, Find in Settings or the app bar (Ctrl+Shift+1 to 9)
  - the app bar: Settings › Appearance › App bar order moves and hides the built-in entries (Activity,
    Chat, Teams, Calendar, Calls, OneDrive; Chat cannot be hidden), also from a right click on an icon;
    Auto collapse app bar now shows icons only while the window is narrower than 800 px
  - your status message has an Emoji button, a 30 minutes option next to 1 hour, 4 hours, Today and This
    week, and shows "Clears at 14:30" (a message that has expired is hidden)
- **Desktop:** your status follows your calls and goes Away after five idle minutes (also on
  Wayland, where IronTeams watches the compositor's idle notification); tray icon
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
updates come from your package manager. For these Settings › About checks GitHub for the latest
release once per start, shows a banner when there is one, and never downloads anything. That check
cannot be turned off.

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
- Updates cannot be turned off: a self-updating copy always downloads new versions and installs
  them when you restart or quit. Settings › About › Updates has **Get beta updates** (off) and
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
| `IRONTEAMS_VIDEO_SUBSCRIBE=static` | In meetings, ask for video the older way (the latest speakers at one size) instead of by tile role |

The old `KELMA_*` names still work when the `IRONTEAMS_*` one is not set. On the first start after the
rename, the old `kelma` config, data and cache folders are moved to `ironteams`, and a saved sign-in
under the old keyring name is moved to the new one when it is first read, so you stay signed in.

### Command line

| Flag | Effect |
|---|---|
| `--account label[@tenant]` | Start on that account, or on an organization of it, and keep it as the active one. An unknown label falls back to the saved choice |
| `--hidden` | Start in the tray without showing the window |
| `--reset-theme` | Put the theme back to the default (IronTeams Light and Dark following the system), drop the active account's own theme, the call theme and the fonts, then start as usual. Custom themes stay |
| `--report-problem` | Write the local problem report (see below) |

Switching accounts and Settings › Restart use these flags to start IronTeams again. A second launch
of the same data folder (`IRONTEAMS_HOME` or the default) hands over to the running instance and
raises its window. The window's position is restored on every system except Wayland, where
a client cannot read or set it, so only the size and the maximized state come back there.

Media, notification pictures, opened OneDrive files and voicemail are cached per account in
`cache/accounts/<account>/` of the cache folder. The first start after upgrading moves the shared
folders into the active account's folder (nothing is deleted); the 500 MB budget is shared and
evicts the oldest files across all accounts; removing an account deletes only its own folders.

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

UI colours: every colour on screen comes from region tokens (`Theme.content`, `Theme.nav`, `Theme.call`;
`ui/tokens/`), and `scripts/check-ui-layers.sh` refuses colour literals elsewhere. A pull request that touches
the UI is drawn with its base in CI (`ui-compare.yml`) and the pictures are compared pixel by pixel; a
deliberate change needs the `visual-change` label. The theme model (tokens, OKLCH derivation, `themes.json`)
is in `crates/appearance`.

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
- Protocol knowledge, and some adapted code, from [rusty-teams](https://github.com/danieldaeschle/rusty-teams) by Daniel Däschle (MIT). The MIT notice is in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
- Color emoji: [Twemoji](https://github.com/mozilla/twemoji-colr) (CC-BY 4.0). See `crates/ironteams/ui/fonts/README.md`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Docs synced to commit 5997752a ("Merge pull request #97 from MohamedEslam04/feat/theme-gallery", 2026-11-26)
