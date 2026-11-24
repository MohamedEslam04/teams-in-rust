# desktop_cards

## Overview
Teams-style desktop notification cards. The app spawns its own executable as a card host child (`client.rs`), talks to it over a line-delimited JSON protocol (`protocol.rs`), and the host (`host.rs`) stacks and animates cards using pure geometry (`geometry.rs`) and a backend chosen by `select.rs` (Wayland wlr-layer-shell in `layer.rs` on Linux).

Files:
- `mod.rs`: Module docs and declarations for the Teams-style desktop notification cards drawn by a separate host process; defines HOST_ARG ("--toast-host").
- `client.rs`: App side of the card host: spawns the same exe as a child, writes JSON lines to its stdin, reads replies on a thread and dispatches per-card action handlers; falls back (false) when the host cannot draw.
- `geometry.rs`: Pure pixel arithmetic: Rect, MAX_CARDS/MARGIN, work_area per OS (taskbar allowance), stack of cards from the bottom-right corner, and corner placement of one window.
- `host.rs`: Host process logic: Core card model, stack plan/apply (replace, evict, keep ringing calls), Card->ToastCard conversion, callback wiring to FromHost actions, backend dispatch (run) and the winit window backend (X11/Windows/macOS).
- `layer.rs`: Linux Wayland wlr-layer-shell backend: probe for zwlr_layer_shell_v1, custom Slint platform rendering software buffers into wl_shm, SCTK seat/pointer/keyboard events mapped to Slint, calloop loop fed by Wayland and a stdin channel.
- `protocol.rs`: Line-delimited JSON wire protocol between app and card host: Card, Kind, ToHost, FromHost, Action, Backend, plus line()/parse() helpers.
- `select.rs`: Pure backend selection (choose) from OS, session capabilities and optional IRONTEAMS_CARD_HOST override; detect() reads Wayland/X11 env.

## Interface
- `[mod.rs:HOST_ARG]` pub const &str = "--toast-host": argument that starts the executable as the card host
- `[protocol.rs:Kind]` pub enum {Message(default), Meeting, Reminder, Call}; serde lowercase; Copy
- `[mod.rs:client/geometry/host/protocol/select]` pub submodules; `layer` is private and linux-only (#[cfg(target_os = "linux")])
- `[protocol.rs:Kind::as_str]` pub fn(self) -> &'static str; the value of Slint ToastCard.kind
- `[client.rs:Handler]` pub type Box<dyn Fn(Action) + Send + Sync>: per-card action callback, called on the host reader thread
- `[protocol.rs:Card]` pub struct (serde default): id u32, kind, title, sender, text, initials, color u32 (0xRRGGBB), avatar Option<String>, meeting, can_reply, can_mute, can_join
- `[client.rs:START_TIMEOUT]` pub const Duration = 5s: how long the first card waits for the host
- `[protocol.rs:ToHost]` pub enum (snake_case): Show(Card) | Dismiss{id:u32} | Quit; app -> host stdin
- `[client.rs:MAX_STARTS]` pub const u32 = 3: host starts per app run
- `[protocol.rs:Action]` pub enum (tag="type", snake_case): Reply{text}, React{key}, Open, Join, Snooze{minutes}, Dismiss, Accept, Decline, Settings, Mute, Closed
- `[client.rs:next_id]` pub fn() -> u32: new card number (global AtomicU32 starting at 1)
- `[protocol.rs:Backend]` pub enum (kebab-case, Copy): LayerShell | X11 | Native
- `[client.rs:show]` pub fn(card: Card, on_action: Handler) -> bool: shows/replaces a card, starts host on first use and may block up to START_TIMEOUT (call via spawn_blocking); false means use system notification; always false under cfg!(test)
- `[protocol.rs:FromHost]` pub enum (snake_case): Ready{backend} | Unavailable{reason} | Action{id,action}; host -> app stdout
- `[client.rs:dismiss]` pub fn(id: u32): removes a card (sends ToHost::Dismiss) only if phase Ready and a handler exists
- `[protocol.rs:line]` pub fn line<T: Serialize>(&T) -> String; one JSON line without newline, empty string on serialize error
- `[geometry.rs:Rect]` pub struct {x:i32,y:i32,w:u32,h:u32} physical pixels
- `[protocol.rs:parse]` pub fn parse<'a, T: Deserialize<'a>>(&'a str) -> Option<T>; trims, None on unknown/invalid
- `[geometry.rs:MAX_CARDS]` pub const usize = 3
- `[select.rs:Os]` pub enum {Linux, Windows, Mac, Other}; Os::current() -> Os via cfg!
- `[geometry.rs:MARGIN]` pub const u32 = 16 logical px from work-area edges
- `[select.rs:Caps]` pub struct (Default): wayland, layer_shell, x11 bools
- `[geometry.rs:work_area]` pub fn(monitor: Rect, scale: f64, os: select::Os) -> Rect: subtracts taskbar/menu bar/dock (Win 0/48, Mac 25/70, Linux/Other 0/44 logical px)
- `[select.rs:Force]` pub enum {LayerShell, X11, Off}; Force::parse(&str) -> Option<Force> (case-insensitive, trimmed; aliases layer/layer-shell, x11/xwayland, off/none/system)
- `[geometry.rs:stack]` pub fn(area, width, heights: &[u32], margin, spacing) -> Vec<(i32,i32)>: top-left corners oldest first, stacked upwards from bottom-right
- `[select.rs:choose]` pub fn choose(Os, &Caps, Option<Force>) -> Option<Backend>; pure; None means fall back to system notifications
- `[geometry.rs:corner]` pub fn(area, size: (u32,u32), margin) -> (i32,i32): one window in bottom-right corner
- `[select.rs:detect]` pub fn detect(probe: impl FnOnce() -> bool) -> Caps; reads WAYLAND_DISPLAY/WAYLAND_SOCKET/DISPLAY, probe only run in Wayland session
- `[host.rs:emit]` pub fn(&FromHost): writes one JSON line to stdout and flushes
- `[select.rs:forced]` pub fn forced() -> Option<Force>; reads IRONTEAMS_CARD_HOST via compat::var
- `[host.rs:Core]` pub struct {model: Rc<VecModel<ToastCard>>, out: Rc<dyn Fn(FromHost)>, relayout: Rc<dyn Fn()>}; Core::new(out, relayout)
- `[host.rs:Plan]` pub enum {Replace(usize), Append{evict: Vec<usize>}}
- `[host.rs:plan]` pub fn(stack: &[(u32,Kind)], id: u32, max: usize) -> Plan: replace known id, else evict oldest non-call cards first, calls only as last resort
- `[host.rs:parse_act]` pub fn(&str) -> Option<Action>: maps button strings (join/dismiss/accept/decline/open/settings/snooze:N, N>0)
- `[host.rs:to_toast]` pub fn(&Card) -> ToastCard: builds Slint card; card id string stands in for conversation/target/message; loads avatar from path; computes RTL flags
- `[host.rs:apply]` pub fn(core: &Core, msg: ToHost) -> bool: applies Show/Dismiss/Quit; true means host should quit
- `[host.rs:wanted_size]` pub fn(&CardHost) -> (f32,f32): ensures repeater rows instantiated, then returns wanted width/height
- `[host.rs:wire]` pub fn(host: &CardHost, core: &Rc<Core>): binds MessageToastStore model and dismiss/reply/react/open/mute/act callbacks to FromHost::Action
- `[host.rs:apply_language]` pub(super) fn(): applies language from loaded prefs (crate::language::apply)
- `[host.rs:run]` pub fn() -> anyhow::Result<()>: host entry point called from main for --toast-host; detects capabilities, chooses backend, runs it, emits Unavailable on error
- `[layer.rs:probe]` pub fn() -> bool (linux): connects to Wayland and checks for zwlr_layer_shell_v1 in the registry
- `[layer.rs:run]` pub fn() -> anyhow::Result<()> (linux): runs the layer-shell host loop until stdin closes

## Invariants
- `[client.rs:client]` one process-wide Client (OnceLock) with Mutex<Inner> + Condvar; lock() recovers from poisoning via into_inner
- `[protocol.rs:FromHost::Ready]` sent once, first, by the host (doc comment)
- `[client.rs:Phase]` state machine Stopped/Starting/Ready/Failed; Failed is terminal for the run (reader does not reset it to Stopped)
- `[protocol.rs:Action::Closed]` always the last word about a card, for any close reason (doc comment)
- `[client.rs:start]` g.starts incremented each start; beyond MAX_STARTS phase becomes Failed (no more restarts)
- `[protocol.rs:Card::id]` chosen by the app (client::next_id); a Show with a known id replaces that card in place
- `[client.rs:show]` Handler runs on the host reader thread; Closed action removes the handler, other actions only clone it; a handler is registered before ToHost::Show is written and removed on write failure
- `[protocol.rs:Card]` #[serde(default)]: only id/title required in practice; missing fields default
- `[client.rs:show]` waits on the Condvar only while phase==Starting up to START_TIMEOUT, then kills the child and sets Failed; under cfg!(test) returns false without spawning
- `[protocol.rs]` wire format: one JSON object per line, no ids of chat/event/call cross the pipe; host knows cards only by the number
- `[client.rs:read]` when host stdout ends, handlers cleared, stdin dropped, child reaped, phase -> Stopped unless Failed
- `[protocol.rs:ToHost::Quit]` serializes as the bare string "quit"; Backend serializes kebab-case ("layer-shell")
- `[geometry.rs:MAX_CARDS]` stack holds at most 3 cards; geometry is in physical pixels, MARGIN in logical px, work_area uses saturating_sub so no underflow
- `[protocol.rs:parse]` unknown or junk messages yield None (ignored); surrounding whitespace/CRLF tolerated
- `[host.rs:plan]` a ringing Kind::Call is evicted only when no non-call card remains; evicts until len < max (max.max(1)); indices ascending
- `[protocol.rs:Action::Snooze]` minutes picked are 5, 10 or 15 (doc comment, not enforced)
- `[host.rs:apply]` eviction removes rows in reverse index order and emits Action::Closed for each; Core::remove emits Closed before relayout
- `[select.rs:choose]` on Linux LayerShell requires wayland && layer_shell; X11 requires x11; layer shell preferred over X11 when no force
- `[host.rs:Core::relayout]` called on the UI thread, possibly inside a card callback, so backends must only mark/schedule work (run_window uses Timer::single_shot(ZERO))
- `[select.rs:choose]` Force::Off returns None on every OS; forcing an unavailable backend yields None rather than a fallback
- `[host.rs:wanted_size]` must call ensure_tree_instantiated before reading wanted-height, else a newly added card is not counted
- `[select.rs:choose]` Windows/macOS always Native (force other than Off is ignored); Os::Other always None
- `[host.rs:run]` WAYLAND_DISPLAY/WAYLAND_SOCKET removed (unsafe) for the X11 backend only while the host has a single thread; Slint/Core/WINDOW state is UI-thread-only (thread_local WINDOW, Rc); stdin thread posts via invoke_from_event_loop
- `[select.rs:detect]` layer_shell is false unless wayland is true (probe not called otherwise); empty env vars count as unset
- `[layer.rs:State::relayout]` surface is recreated when the last card goes and made on next; after set_size, configured=false so no drawing until the compositor configures (bottom-right anchor growth); checked every loop turn
- `[layer.rs:draw]` pixels converted to wl_shm ARGB8888 premultiplied little-endian as [B,G,R,A]; input region excludes shadow pad; layer margin = MARGIN - pad; keyboard interactivity OnDemand, layer Overlay, namespace "ironteams-notifications"

## Side Effects
- `[client.rs:start]` spawns child process std::env::current_exe() --toast-host with piped stdin/stdout and inherited stderr, plus a reader thread
- `[select.rs:set]` reads process environment variables (WAYLAND_DISPLAY, WAYLAND_SOCKET, DISPLAY)
- `[client.rs:show]` writes JSON lines to child stdin; kills the child on start timeout; blocks the calling thread on a Condvar up to 5s
- `[select.rs:forced]` reads env IRONTEAMS_CARD_HOST via compat::var
- `[client.rs:client]` global static OnceLock<Client> and static AtomicU32 card id counter; tracing logs
- `[select.rs:detect]` may invoke caller-supplied probe closure (the Wayland compositor global query lives in the caller)
- `[client.rs:read]` invokes user handlers on the reader thread; waits on the child at exit
- `[host.rs:emit]` writes JSON lines to the process stdout and flushes
- `[host.rs:run_window]` spawns a stdin reader thread; creates a winit/software Slint window (always on top, no decorations, X11 Notification type, skip taskbar on Windows, accessory policy on macOS); unsafe env var removal; quits the event loop when stdin closes
- `[host.rs:place_window]` sets window size/position and shows/hides the window based on the stack; queries monitors
- `[host.rs:to_toast]` reads avatar image file from a path given by the app (slint::Image::load_from_path)
- `[host.rs:apply_language]` reads prefs from disk via crate::support::quiet::load_prefs and applies the language globally
- `[layer.rs:run]` connects to the Wayland socket, installs a global Slint platform (set_platform), creates wl_shm buffers/pool, layer surface, pointer cursor surface, spawns a stdin reader thread, and loops on calloop until exit

## Verification
- Run: `cargo test -p ironteams`
