# backend/call/live

## Overview
The real `CallDriver` (`LiveDriver`) built on the `calling` crate: shared calling trouter/NgcClient hub, meeting join loop with audio-only fallback, the live-call select loop, mic chain, share/camera controls, renegotiation and speaker tracking.

Files:
- `audio.rs`: Mic chain (AEC/NS/AGC then Opus), restartable Audio devices struct (applies the saved speaker volume on every device open), mute/hold outgoing filter, incoming-packet forwarder, MediaState to Link mapping.
- `controls.rs`: Call impl for screen share (picker, session, grant timeout) and remote-control actions/messages, plus meeting/1:1 signalling of share line changes.
- `driver.rs`: LiveDriver (CallDriver impl): lists devices from crates/devices+settings (`device_rows`: on Linux the first mic/speaker row is "Default" or "Default (<name>)" with `id: None`, so indices shift by one), spawns run_meeting on join, camera preview; LiveHandle forwards Controls.
- `dump.rs`: IRONTEAMS_CALL_DUMP debug dumping of calling pushes, answer SDPs and conversation-service exchanges to timestamped files.
- `hub.rs`: Hub: shared calling trouter + NgcClient, routes pushes to per-session/invitation/unmatched channels, ICE servers, meeting target resolution.
- `join.rs`: run_meeting join loop with audio-only fallback on video refusal, join_meeting setup, adopt() for connected 1:1 calls, find_blob.
- `lifecycle.rs`: Call impl: keep-alive task, media answer apply, Control dispatch (mute/hand/react/sound/devices/bots/admit/transfer), hold/resume re-offer (refused while a restarted media session is waiting) and refusal rollback.
- `renegotiate.rs`: Call impl: camera on/off control and signalling, ended_text, answering re-offers (an ICE restart is answered on a fresh MediaSession parked in `Call::next`, which takes over once connected; supersedes our pending re-offer).
- `run.rs`: impl Call::run: the live-call select loop (controls, data channel, pushes, media state, roster/standing/share/camera/remote-control updates) and teardown/leave on exit.
- `talk.rs`: Speaker tracking (Talk), media options for a call, caption decoding from data-channel messages, speaker-name lookup, and building the Person list for the UI (incl. `Person.elsewhere`, set when the person has more than one device).

## Interface
- `[audio.rs:processing_wanted]` pub(super) fn() -> ProcessingOptions; reads settings devices prefs + IRONTEAMS_NO_AEC
- `[run.rs:Call::run]` pub(super) async fn run(self, events: MediaEvents, pushes: UnboundedReceiver<CallPush>, opts: JoinOptions, sink: EventSink, ctl: &mut UnboundedReceiver<Control>, early: Vec<Control>) -> bool; returns true only when the service ended the call refusing our video lines (caller rejoins audio-only)
- `[audio.rs:processing_from]` pub(super) fn(&DevicePrefs, Option<&OsStr>) -> ProcessingOptions
- `[talk.rs:Talk]` pub(super) struct {now: HashSet<String>, last: HashMap<String,u64>, tick: u64}, Default; who is speaking now and the tick each MRI last spoke at
- `[audio.rs:aec_wanted_by]` pub(super) fn(Option<&OsStr>) -> bool; true when unset/empty/"0"
- `[talk.rs:Talk::heard]` pub(super) fn heard(&mut self, roster: &Roster, sources: &[u32]) -> bool; maps audio source ids to participant MRIs, true when the speaking set changed
- `[audio.rs:conversation_controller]` pub(super) fn(&Value) -> Option<String>; https-only conversationController URL
- `[talk.rs:media_options]` pub(super) fn media_options(meeting: bool, video: bool) -> MediaOptions; data_channel = meeting, video lines from share::media_options() or default
- `[audio.rs:mic_chain]` pub(super) fn(S: FrameSource, ProcessingOptions) -> (Receiver<OpusPacket>, Option<EchoReference>)
- `[talk.rs:captions_of]` pub(super) fn captions_of(msg: DataMessage, roster: &Roster) -> Vec<Caption>; empty unless msg.data_id == CAPTIONS_DATA_ID, fills missing speaker from roster
- `[audio.rs:Audio]` pub(super) struct {sender: watch::Receiver<AudioSender>, muted, held: Arc<AtomicBool>, play: watch::Sender<Option<mpsc::Sender<OpusPacket>>>, send_task, playback}
- `[talk.rs:speaker_name]` pub(super) fn speaker_name(roster: &Roster, user_id: &str) -> String; roster name by MRI or by id after the MRI prefix (case-insensitive), empty if unknown
- `[audio.rs:Audio::start]` pub(super) fn(&mut self, mic: Option<&str>, speaker: Option<&str>) -> anyhow::Result<()>
- `[audio.rs:apply_saved_volume]` pub(super) fn(); reads Settings `devices.speaker_volume` and calls `calling::audio::set_output_volume`
- `[talk.rs:people]` pub(super) fn people(roster, me, own_tenant: Option<&str>, standing: &HashMap<String,Standing>, talk: &Talk, video: &RemoteVideo) -> Vec<Person>; builds UI person list (speaking, muted, hand, lobby, role, video slot, outsider class)
- `[audio.rs:Audio::play_into]` pub(super) fn(&mut self, Playback)
- `[audio.rs:Audio::stop]` pub(super) fn(&mut self)
- `[audio.rs:outgoing]` pub(super) fn(OpusPacket, muted, held) -> Option<OpusPacket>
- `[audio.rs:forward]` pub(super) fn(watch::Receiver<..>, Arc<AtomicBool>, mpsc::Receiver<OpusPacket>) -> JoinHandle<()>
- `[audio.rs:link_of]` pub(super) fn(MediaState) -> Link
- `[controls.rs:Call::share_control]` pub(super) async fn(&mut self, on, &UnboundedSender<ShareMsg>, &EventSink)
- `[controls.rs:Call::share_msg]` pub(super) async fn(&mut self, ShareMsg, tx, sink)
- `[controls.rs:Call::share_started]` pub(super) async fn(&mut self, tx, sink)
- `[controls.rs:Call::session_frame]` pub(super) async fn(&mut self, event:&str, body:&Value, sink)
- `[controls.rs:Call::control_action]` pub(super) async fn(&mut self, ControlAction, tx, sink); no-op unless remote_control::wanted()
- `[controls.rs:Call::control_message]` pub(super) async fn(&mut self, ControlMessage, sink)
- `[controls.rs:Call::input_answered]` pub(super) async fn(&mut self, Result<Option<Box<InputInjector>>,String>, sink)
- `[controls.rs:Call::tell_meeting]` pub(super) async fn(&mut self, VideoRole, Sending, sending:bool) -> anyhow::Result<()>
- `[controls.rs:Call::tell_share]` pub(super) async fn(&mut self, sending:bool, sink)
- `[controls.rs:Call::declared_lines]` pub(super) fn(&self) -> Option<&[String]>; Some only in meetings
- `[driver.rs:LiveDriver]` pub struct {hub: Arc<Hub>}; pub fn new(Arc<Hub>); impl CallDriver (devices, join, preview)
- `[driver.rs:list_devices]` pub(super) fn() -> Devices; rows built by `device_rows`
- `[driver.rs:device_rows]` pub(super) fn(list: &[AudioDevice], chosen: Option<&AudioDevice>) -> (Vec<Device>, usize); on Linux a leading "Default" row with `id: None`, selected unless a still-present device was chosen; elsewhere the system default device is selected when none was chosen
- `[hub.rs:Hub]` pub struct; pub fn new(Client) -> Arc<Hub>; pub fn start(self:&Arc); pub fn ngc(); pub async fn ngc_client(); pub fn surl(); pub fn route(CallPush); pub fn subscribe_invitations(); pub fn subscribe_unmatched()
- `[hub.rs:Hub::add_session/remove_session]` pub(super) fn(&self, session_id:&str); add returns UnboundedReceiver<CallPush>
- `[hub.rs:Hub::resolve]` pub(super) async fn(&self, JoinTarget) -> anyhow::Result<MeetingTarget>
- `[join.rs:run_meeting]` pub(super) async fn(Arc<Hub>, JoinTarget, JoinOptions, EventSink, UnboundedReceiver<Control>)
- `[join.rs:join_meeting]` pub(super) async fn(hub, target, opts, with_video) -> anyhow::Result<Joined>
- `[join.rs:adopt]` pub fn(hub, CallSession, Links, MediaSession, MediaEvents, JoinOptions, EventSink) -> Box<dyn CallHandle>
- `[dump.rs:dump_answer]` pub(crate) fn(name:&str, text:&str); writes to IRONTEAMS_CALL_DUMP dir

## Invariants
- `[audio.rs:outgoing]` held => nothing sent; muted => OPUS_SILENCE replaces captured packet so RTP keeps flowing (no RTP => service ends call with MediaConnectivityFailure)
- `[run.rs:Call::run loop]` single tokio::select! loop owns all call state; per-iteration flags roster_changed/standing_changed drive the post-select refresh of roster, presenter, share, remote control, camera and phase events
- `[audio.rs:Audio::start]` always stop() first; if mic fails the speaker still starts, then returns the mic error
- `[run.rs:Call::run phase]` Event::Lobby/Connected/Reconnecting/Connecting are sent only when live_phase(link, in_lobby && meeting, was_in_call) differs from the last shown phase
- `[audio.rs:Audio::start]` with AEC or NS on, the speaker must feed played frames to the EchoReference (start_with_tap)
- `[run.rs:Call::run teardown]` leave (hang_up via links 'leave'/'callLeg', else leave_controller) runs only when ended.is_none() || hang_up; teardown order: keep_alive abort, share, control, camera, video, audio.stop, `calling::audio::set_output_off(false)`, media.close, close of a waiting `self.next` session, forwarder abort, hub.remove_session
- `[audio.rs:conversation_controller]` only https:// URLs accepted as controller
- `[run.rs:video_refused]` Event::Ended is suppressed and run returns true when the service ended the call refusing declared video lines (fallback::call_refused_video); otherwise returns false
- `[hub.rs:Hub::start]` trouter spawned once (slot.is_some() guard); event task holds only Weak<Hub>
- `[run.rs:CallEvent::MediaRenegotiation]` the handler only calls `renegotiate(&offer, &links, held, &sink)` and sets in_lobby=false and roster_changed=true; the swap happens in the `next_state(&mut self.next)` branch when the restarted session reports `MediaState::Connected`: share stopped (and left, with an error notice), camera stopped but re-wanted via camera_wanted, old session closed, video/state/speakers/data/audio receivers replaced, depacketizer reset, link set to `Link::Connected`, roster_changed=true; on `Failed|Closed` there the new session is dropped and the old one kept
- `[hub.rs:Hub::route]` Call pushes go to sessions[session_id]; no session or Other => unmatched subscribers; closed senders pruned via retain
- `[run.rs:CallEvent::Rejected guard]` a Rejected while `self.reoffer.is_some() || self.superseded.is_some()` is a refused re-offer, not a call end; `refused_reoffer` picks `Refused::Pending` (rolled back via `Call::refused`) or `Refused::Superseded` (nothing undone); a matching `MediaAnswer` for a superseded re-offer is ignored; any other Rejected/Ended breaks the loop
- `[run.rs:state_rx]` the state branch is `state_rx.recv(), if state_open`; while a restarted session waits in `self.next`, `Failed|Closed|None` sets `state_open = false` and `Link::Disconnected` instead of ending the call; the call ends with "The connection was lost." only if the waiting session also fails (or none waits)
- `[hub.rs:Hub::connected_surl]` waits at most 30s for the trouter surl
- `[run.rs:standing_asked]` profile lookups happen once per `8:orgid:` non-guest MRI (insert into standing_asked gates the lookup); results return via standing_rx
- `[join.rs:join_meeting]` session registered with hub.add_session before ngc.join (callbacks can precede join answer); removed + media closed on join failure
- `[run.rs:camera_wanted]` camera is opened only when can_camera, then camera_wanted is taken (cleared) so it opens once per want
- `[join.rs:run_meeting]` video lines declared only if hub.video.video_allowed() and camera/share wanted; a video refusal triggers exactly one audio-only retry (opts.camera=false)
- `[run.rs:remote_control::wanted]` control machine is stepped each iteration only when remote_control::wanted(); control share is cleared when not sharing; control data id handling is marked UNVERIFIED
- `[controls.rs:share_control/lifecycle.rs:camera_control]` in meetings, share/camera start needs a mid declared at join (share::meeting_mid), else error; undeclared update would make the service end the call
- `[run.rs:speakers_open]` speakers_rx.changed() branch is disabled after the sender closes (Err sets speakers_open=false); re-enabled after renegotiation
- `[lifecycle.rs:Call::refused]` after a refused re-offer: rollback_offer, pending_offer=None, answered=true, then undo hold/camera/share
- `[lifecycle.rs:Call::hold]` refuses while `self.next.is_some()`: sends `Event::Held(!on)` and an error "The call is reconnecting its media", no re-offer
- `[run.rs:Call::run mic]` audio.muted starts as !opts.mic_on and Control::Mute(true) is applied before early controls
- `[renegotiate.rs:Call::renegotiate]` signature `(&mut self, offer, links, held: bool, sink)` returning `()`; ICE-restart offers refused by the connected session are answered on a fresh MediaSession via `accept_or_restart`, parked in `Call::next` (`NextMedia`) and replacing the old session only after it connects (the link is set to `Link::Connected` at takeover); a newer restart replaces the waiting one; 1:1 answers are rewritten by `direct_reanswer` to reflect camera/share/hold; a pending re-offer is superseded (`supersede`, glare: theirs wins)
- `[run.rs:Call::run]` global speaker state: `Control::Sound(off)` calls `calling::audio::set_output_off` (lifecycle.rs), and run sets it from `opts.sound_off` at start; it is reset to false on teardown
- `[talk.rs:Talk::heard]` tick increments only when the speaking set changes; each newly present MRI gets last[mri]=tick; people() uses spoke=0 for never-heard MRIs and name 'Unknown' for empty display names

## Side Effects
- `[hub.rs:Hub::start]` spawns tokio tasks: trouter event loop and NGC endpoint discovery; NgcTrouter::spawn opens network connection
- `[run.rs:Call::run]` starts audio capture/playback via audio.start (mic/speaker devices); sends Event::Error on failure; audio.stop at end
- `[hub.rs:Hub::ngc_client/ice_servers/resolve]` HTTP to conversation service, TURN relay resolve, CSA /me chats
- `[run.rs:Call::run]` spawns tokio tasks: audio forwarders (forward(...), aborted at end) and per-roster-change profile lookup via hub.client.profiles
- `[audio.rs:Audio::start]` reads Settings `devices.speaker_volume` from disk on every open (apply_saved_volume) and sets the global output volume; opens mic/speaker devices; spawns capture-encoder, send task and decoder-playback tasks (aborted/closed in stop)
- `[run.rs:Call::run]` network: hub.client.tenant_id(), ngc.hang_up / ngc.leave_controller on exit, share::leave_session, media.close()
- `[audio.rs:processing_wanted/driver.rs:list_devices]` reads settings Local config from disk and IRONTEAMS_NO_AEC env
- `[lifecycle.rs:Call::control/run.rs:Call::run]` set the global speaker state (`calling::audio::set_output_off`) on Control::Sound and at start/teardown of a call
- `[run.rs:Call::run]` sends UI Events through EventSink (Roster, Caption, Reaction, Recording, Hand, Share, Camera, Presenter, Error, Lobby/Connected/Reconnecting/Connecting, Ended)
- `[dump.rs:dump_dir]` reads IRONTEAMS_CALL_DUMP, creates the dir, OnceLock-cached; static AtomicU64 SEQ for file names
- `[run.rs:Call::run]` start_keep_alive spawns keep-alive task (self.keep_alive), aborted on exit
- `[dump.rs:write_push/dump_answer/dump_exchange]` write push JSON, SDP and exchange files (real ids/names) to disk
- `[run.rs:Call::run]` hub.remove_session(callbacks.session_id()) mutates the shared hub's session registry on exit
- `[lifecycle.rs:Call::start_keep_alive]` spawns keepAlive task via ngc.run_keep_alive
- `[run.rs:Call::run]` starts/stops screen share, camera, remote control and bot interactions through self.share/self.camera/self.control/self.bots; tracing logs (warn/info/debug incl. conversation message bodies and summaries of SDP directions/transport for 1:1 hold/answer and media state; the join-failed warning goes through `teams_proto::logsafe::text`)
- `[lifecycle.rs:Call::control, controls.rs:tell_share, renegotiate.rs:tell_camera]` network calls to the call service (mute, hand, reaction, add/admit/transfer, hold, UpdateMediaDescriptions, re-offer, media answer/ack)
- `[talk.rs:media_options]` None (pure)
- `[controls.rs:Call::control_action/apply_control]` shows desktop input-consent prompt, injects remote input, sends control over NGC/data channel; screen capture picker/camera portal
- `[driver.rs:LiveDriver::join / join.rs:adopt]` tokio::spawn the call task; events sent to EventSink

## Verification
- Run: `cargo test -p ironteams`
