# support

## Overview
Small shared pure helpers: turning failed-command errors into readable text (`error_text.rs`) and the banner decision for pause, quiet hours, Focus and keyword alerts (`quiet.rs`).

Files:
- `error_text.rs`: Turns failed-command errors and raw 'HTTP nnn' status lines into short banner sentences.
- `quiet.rs`: Pure banner decision (pause, quiet hours, Focus, keyword alerts), Arabic-aware keyword matching, and process-global Focus state with prefs persistence.

## Interface
- `[error_text.rs:shown_error]` pub(crate) fn(&anyhow::Error) -> String; transient teams_proto::Error -> 'Couldn't reach Teams...', Status -> status_text, NotMember -> not-a-member banner, else err.to_string()
- `[error_text.rs:plain_banner]` pub(crate) fn(&str) -> String; replaces ': HTTP ddd...' tail with a status sentence keeping text before the context word, else returns input unchanged
- `[quiet.rs:Incoming]` pub struct<'a> (Debug, Clone, Copy, Default): paused, routed, mentions_me, urgent, priority_sender, call, keyword: Option<&'a str>
- `[quiet.rs:Why]` pub enum: Paused | QuietHours | Focus | Routing
- `[quiet.rs:Decision]` pub enum<'a>: Show{keyword: Option<&'a str>} | Hidden(Why)
- `[quiet.rs:should_banner]` pub fn(&Incoming, NaiveDateTime, &GeneralPrefs, focus: bool) -> Decision; the one pure banner decision (called from live.rs per doc)
- `[quiet.rs:in_quiet_hours]` pub fn(NaiveDateTime, &QuietHoursPrefs) -> bool
- `[quiet.rs:focus_active]` pub fn(Option<i64>, now_ms) -> bool; true while end > now
- `[quiet.rs:focus_end]` pub fn(FocusDuration, now_local, now_ms) -> i64 end timestamp ms (FOCUS_FOREVER for Manual)
- `[quiet.rs:normalise]` pub fn(&str) -> String; lowercase, strips tashkeel/tatweel, unifies alef/yeh/teh-marbuta, collapses whitespace
- `[quiet.rs:keyword_hit]` pub fn<'a>(&str, &'a [String]) -> Option<&'a str>; first keyword matching as whole word/phrase
- `[quiet.rs:init_focus]` pub fn(&GeneralPrefs, now_ms); start-up init of global Focus state, drops ended focus
- `[quiet.rs:set_focus]` pub fn(Option<i64>); sets in-memory Focus end (caller persists)
- `[quiet.rs:focus_until]` pub fn(&GeneralPrefs) -> Option<i64>; in-memory value if initialised else prefs.focus_until_ms
- `[quiet.rs:focus_override]` pub fn() -> Option<Option<i64>>; Some(..) once init/set ran; used by Local::save_general
- `[quiet.rs:persist_focus]` pub fn(Option<i64>); loads general prefs from config dir, sets focus_until_ms, saves
- `[quiet.rs:load_prefs]` pub fn() -> GeneralPrefs; prefs as saved on disk, defaults if no config dir

## Invariants
- `[quiet.rs:should_banner]` Evaluation order is fixed: paused > quiet hours > Focus filter > routing; pause and quiet hours beat keyword hits and Focus exceptions
- `[quiet.rs:should_banner]` In Focus only mentions_me/urgent/priority_sender/call/keyword pass; a message still needs routed (or call, or keyword) to Show, so a mention in a muted chat is Hidden(Routing)
- `[quiet.rs:should_banner]` Show carries keyword only when routed is false and not a call; routed or call messages yield Show{keyword: None}
- `[quiet.rs:in_quiet_hours]` start/end clamped to <=1439; disabled or start==end means no window
- `[quiet.rs:in_quiet_hours]` end<=start window runs over midnight and belongs to the start day (on(day) && minute>=start, or on(day+6) && minute<end, days indexed mod 7 from Monday); same-day window is half-open [start,end)
- `[quiet.rs:focus_active]` Focus is on only while end > now strictly; the end instant itself is over
- `[quiet.rs:focus_end]` Tomorrow = next local day 09:00 (fallback +24h), computed as now_ms plus local wall-clock delta; Manual = FOCUS_FOREVER
- `[quiet.rs:keyword_hit]` Matching on normalised text with alphanumeric word boundaries; blank keywords never match; Arabic prefixed forms (article, wa-) are different words
- `[quiet.rs:FOCUS]` Static Mutex<Option<Option<i64>>>: outer None = not initialised (tests, CLI) so saved pref decides; inner None = Focus off; lock poisoning recovered via into_inner
- `[quiet.rs:init_focus]` Saved focus_until_ms already in the past is dropped at start
- `[quiet.rs:focus_override]` Exists so Local::save_general never undoes a tray Focus change; None when app state uninitialised
- `[error_text.rs:plain_banner]` Only rewrites when text contains ': HTTP ' followed by exactly 3 digits; otherwise returns text unchanged; the original text is logged at warn through `teams_proto::logsafe::text` (sanitised)

## Side Effects
- `[quiet.rs:FOCUS]` process-global static Mutex holding Focus mode end time, written by init_focus/set_focus
- `[quiet.rs:persist_focus]` disk: reads then rewrites general prefs file (appearance::general::FILE_NAME) in session::Paths config_dir; read-modify-write of whole prefs
- `[quiet.rs:persist_focus]` logs tracing::warn on save failure; silently returns if Paths::new fails
- `[quiet.rs:load_prefs]` disk read of the general prefs file via session::Paths config_dir
- `[error_text.rs:plain_banner]` tracing::warn logging of the sanitised error text (`teams_proto::logsafe::text`) when a raw HTTP status is cut out

## Verification
- Run: `cargo test -p ironteams`
