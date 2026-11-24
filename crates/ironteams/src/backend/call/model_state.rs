use super::*;

impl CallModel {
    /// Opens the pre-join screen. Refused while a call runs.
    pub fn prejoin(
        &mut self,
        kind: CallKind,
        target: JoinTarget,
        chat_id: String,
        devices: Devices,
    ) -> Result<(), &'static str> {
        if self.phase.active() {
            self.error = ALREADY_IN_CALL.into();
            return Err(ALREADY_IN_CALL);
        }
        *self = CallModel {
            phase: Phase::PreJoin,
            kind,
            target: Some(target),
            chat_id,
            my_name: std::mem::take(&mut self.my_name),
            mic_on: true,
            devices: devices.or_default(),
            ..Default::default()
        };
        Ok(())
    }

    /// Pre-join "Cancel".
    pub fn cancel(&mut self) -> bool {
        self.phase == Phase::PreJoin && {
            self.phase = Phase::Idle;
            true
        }
    }

    /// Pre-join "Join now": what to join and how. `None` unless on the pre-join screen.
    pub fn joining(&mut self) -> Option<(JoinTarget, JoinOptions)> {
        if self.phase != Phase::PreJoin {
            return None;
        }
        let target = self.target.clone()?;
        self.phase = Phase::Connecting;
        self.muted = !self.mic_on;
        self.video_preview = false;
        Some((target, self.options()))
    }

    pub fn options(&self) -> JoinOptions {
        JoinOptions {
            name: self.my_name.clone(),
            mic_on: self.mic_on,
            #[cfg(any(test, feature = "calls"))]
            mic: self.mic_id(),
            #[cfg(any(test, feature = "calls"))]
            speaker: self.speaker_id(),
            #[cfg(any(test, feature = "calls"))]
            camera: self.prejoin_camera,
        }
    }

    pub(super) fn mic_id(&self) -> Option<String> {
        self.devices.mics.get(self.devices.mic)?.id.clone()
    }

    pub(super) fn speaker_id(&self) -> Option<String> {
        self.devices.speakers.get(self.devices.speaker)?.id.clone()
    }

    /// A connected call handed over (a 1:1 call answered or placed elsewhere).
    #[cfg(any(test, feature = "calls"))]
    pub fn adopted(&mut self, kind: CallKind, devices: Devices, now: Instant) -> bool {
        if self.phase.active() {
            return false;
        }
        let chat_id = match &kind {
            CallKind::Direct { chat_id, .. } => chat_id.clone(),
            CallKind::Meeting { .. } => String::new(),
        };
        *self = CallModel {
            phase: Phase::Connected,
            kind,
            chat_id,
            my_name: std::mem::take(&mut self.my_name),
            mic_on: true,
            connected_at: Some(now),
            devices: devices.or_default(),
            ..Default::default()
        };
        true
    }

    /// Applies a driver event. Events for no running call are ignored. Returns whether anything
    /// changed.
    pub fn apply(&mut self, ev: Event, now: Instant) -> bool {
        if !self.phase.active() {
            return false;
        }
        let next = match ev {
            Event::Connecting => {
                if !matches!(self.phase, Phase::Connecting | Phase::Lobby) {
                    return false;
                }
                Phase::Connecting
            }
            Event::Lobby => Phase::Lobby,
            Event::Connected => {
                self.connected_at.get_or_insert(now);
                Phase::Connected
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Reconnecting => {
                if self.phase != Phase::Connected {
                    return false;
                }
                Phase::Reconnecting
            }
            Event::Roster(r) => return replace(&mut self.roster, r),
            Event::Recording(on) => {
                self.noticed(on && !self.recording, !on, RECORDING_NOTICE);
                return replace(&mut self.recording, on);
            }
            Event::Transcribing(on) => {
                self.noticed(on && !self.transcribing, !on, TRANSCRIPTION_NOTICE);
                return replace(&mut self.transcribing, on);
            }
            Event::Captions(on) => return self.set_captions(on),
            Event::Caption(c) => return self.caption(c),
            #[cfg(any(test, feature = "calls"))]
            Event::Hand(h) => return replace(&mut self.hand, h),
            #[cfg(any(test, feature = "calls"))]
            Event::Reaction { mri, name } => return self.reacted(mri, name, now),
            #[cfg(any(test, feature = "calls"))]
            Event::Held(h) => return replace(&mut self.held, h),
            Event::Share { available, sharing } => {
                let a = replace(&mut self.can_share, available);
                return replace(&mut self.sharing, sharing) || a;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Camera { available, on } => {
                let a = replace(&mut self.can_camera, available);
                return replace(&mut self.camera, on) || a;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Presenter(name) => return replace(&mut self.presenter, name),
            #[cfg(any(test, feature = "calls"))]
            Event::Control { mode, name } => {
                let m = replace(&mut self.control_mode, mode.to_owned());
                return replace(&mut self.control_name, name) || m;
            }
            #[cfg(any(test, feature = "calls"))]
            Event::Notice(text) => return replace(&mut self.notice, text),
            Event::Error(e) => return replace(&mut self.error, e),
            Event::Ended(reason) => {
                self.end(reason, now);
                return true;
            }
        };
        replace(&mut self.phase, next)
    }

    pub(super) fn end(&mut self, reason: String, now: Instant) {
        self.phase = Phase::Ended;
        self.ended = reason;
        self.duration = self.connected_at.map(|t| now.saturating_duration_since(t));
        self.hand = false;
        self.can_share = false;
        self.sharing = false;
        self.share_when_ready = false;
        self.can_camera = false;
        self.camera = false;
        self.presenter.clear();
        self.control_mode.clear();
        self.control_name.clear();
        self.recording = false;
        self.transcribing = false;
        self.notice.clear();
        // An error from a call that ran (a camera that would not turn on) is not the ended
        // screen's; one from a join that failed is its reason.
        if self.connected_at.is_some() {
            self.error.clear();
        }
        self.set_captions(false);
        self.held = false;
        self.found.clear();
        self.invite_status.clear();
        self.reactions.clear();
    }

    /// Shows `text` as the notice when `show`, clears it when `hide` and it is showing.
    pub(super) fn noticed(&mut self, show: bool, hide: bool, text: &str) {
        if show {
            self.notice = text.into();
        } else if hide && self.notice == text {
            self.notice.clear();
        }
    }

    /// Captions on or off; off forgets the lines.
    pub fn set_captions(&mut self, on: bool) -> bool {
        if !on {
            self.caption_lines.clear();
        }
        replace(&mut self.captions, on)
    }

    /// A caption while captions are on: it updates its utterance, or comes last.
    pub(super) fn caption(&mut self, c: Caption) -> bool {
        if !self.captions || c.text.trim().is_empty() {
            return false;
        }
        if let Some(known) = self.caption_lines.iter_mut().find(|l| l.id == c.id) {
            return replace(known, c);
        }
        self.caption_lines.push(c);
        if self.caption_lines.len() > CAPTION_LINES {
            self.caption_lines.remove(0);
        }
        true
    }

    /// "Leave": ends the call on our side at once. False when no call runs.
    pub fn leave(&mut self, now: Instant) -> bool {
        if !self.phase.active() {
            return false;
        }
        let reason = match self.kind {
            CallKind::Meeting { .. } => YOU_LEFT_MEETING,
            #[cfg(any(test, feature = "calls"))]
            CallKind::Direct { .. } => YOU_LEFT_CALL,
        };
        self.end(reason.into(), now);
        true
    }

    /// Closes the call-ended screen.
    pub fn close(&mut self) -> bool {
        self.phase == Phase::Ended && {
            self.phase = Phase::Idle;
            true
        }
    }

    /// "Rejoin" on the call-ended screen: back to the pre-join screen for the same meeting.
    pub fn rejoin(&mut self, devices: Devices) -> bool {
        match (self.phase, self.target.clone()) {
            (Phase::Ended, Some(t)) => {
                let (kind, chat) = (self.kind.clone(), self.chat_id.clone());
                self.prejoin(kind, t, chat, devices).is_ok()
            }
            _ => false,
        }
    }

    /// The mute toggle. Returns the new state while a call runs.
    pub fn toggle_mute(&mut self) -> Option<bool> {
        match self.phase {
            Phase::PreJoin => {
                self.mic_on = !self.mic_on;
                None
            }
            p if p.active() => {
                self.muted = !self.muted;
                Some(self.muted)
            }
            _ => None,
        }
    }

    /// The Share toggle: what to ask the driver while a call that can share runs. The state
    /// itself follows the driver's [`Event::Share`] (the picker can be cancelled).
    pub fn toggle_share(&self) -> Option<Control> {
        (self.phase.active() && (self.can_share || self.sharing))
            .then_some(Control::Share(!self.sharing))
    }

    /// A pending "Share screen" (chat header): starts sharing once the call can, then is used up.
    pub fn take_share_when_ready(&mut self) -> Option<Control> {
        if !self.share_when_ready || !self.phase.active() || !self.can_share || self.sharing {
            return None;
        }
        self.share_when_ready = false;
        Some(Control::Share(true))
    }

    /// The Camera toggle: on the pre-join screen it flips the choice; in a call that can send
    /// video it asks the driver. The state itself follows the driver's [`Event::Camera`] (the
    /// portal can refuse).
    pub fn toggle_camera(&mut self) -> Option<Control> {
        if self.phase == Phase::PreJoin {
            self.prejoin_camera = !self.prejoin_camera;
            return None;
        }
        // The camera is about to be opened for sending: the preview lets go of it first.
        if self.phase.active() && !self.camera {
            self.video_preview = false;
        }
        (self.phase.active() && (self.can_camera || self.camera))
            .then_some(Control::Camera(!self.camera))
    }

    /// The self-preview of the Video effects panel and the camera menu, on or off. It runs on
    /// the pre-join screen and in a call whose camera is off (the sending camera is its own
    /// picture); nothing is sent.
    pub fn set_video_preview(&mut self, on: bool) -> bool {
        replace(&mut self.video_preview, on)
    }

    /// "Apply and turn on video": the preview ends, and the camera goes on (the pre-join switch,
    /// or the call's camera when it is off and can send).
    pub fn apply_video(&mut self) -> Option<Control> {
        self.video_preview = false;
        if self.phase == Phase::PreJoin {
            self.prejoin_camera = true;
            return None;
        }
        (self.phase.active() && self.can_camera && !self.camera).then_some(Control::Camera(true))
    }

    /// The devices open again, to pick up what Settings changed (noise suppression).
    pub fn refresh_audio(&self) -> Option<Control> {
        self.phase.active().then(|| Control::Devices {
            mic: self.mic_id(),
            speaker: self.speaker_id(),
        })
    }

    /// A remote-control command while a call runs. "Give control" takes the person's name from
    /// the roster.
    pub fn remote_control(&self, a: ControlAction) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let a = match a {
            ControlAction::Give { mri, .. } => {
                let p = self.roster.iter().find(|p| p.mri == mri && !p.me)?;
                ControlAction::Give {
                    name: p.name.clone(),
                    mri,
                }
            }
            a => a,
        };
        Some(Control::Remote(a))
    }

    /// The hand toggle. Returns the new state while a call runs.
    pub fn toggle_hand(&mut self) -> Option<bool> {
        self.phase.active().then(|| {
            self.hand = !self.hand;
            self.hand
        })
    }

    /// The React menu's pick: it floats here at once and goes to the call, at most one a second
    /// ([`REACTION_EVERY`]). `None` while not connected, or too soon after the last one.
    pub fn react(&mut self, name: &'static str, now: Instant) -> Option<Control> {
        let too_soon = self
            .reacted_at
            .is_some_and(|t| now.saturating_duration_since(t) < REACTION_EVERY);
        if self.phase != Phase::Connected || too_soon {
            return None;
        }
        self.reacted_at = Some(now);
        self.float(name, String::new(), "You".into(), true, now);
        Some(Control::React(name))
    }

    /// Someone's reaction floats with their name. Ours, echoed back by the service, already
    /// floated when we sent it.
    #[cfg(any(test, feature = "calls"))]
    pub(super) fn reacted(&mut self, mri: String, name: &'static str, now: Instant) -> bool {
        let sender = self.roster.iter().find(|p| p.mri == mri);
        if sender.is_some_and(|p| p.me) {
            return false;
        }
        let who = sender
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Someone".into());
        self.float(name, mri, who, false, now);
        true
    }

    pub(super) fn float(
        &mut self,
        name: &'static str,
        mri: String,
        who: String,
        mine: bool,
        now: Instant,
    ) {
        self.reactions
            .retain(|r| now.saturating_duration_since(r.at) < REACTION_SHOWN);
        if self.reactions.len() >= MAX_FLOATING {
            self.reactions.remove(0);
        }
        self.reaction_seq += 1;
        self.reactions.push(Floating {
            id: self.reaction_seq,
            name,
            mri,
            who,
            mine,
            at: now,
        });
    }

    /// Hold / Resume. Returns the new state while the call is connected.
    pub fn toggle_hold(&mut self) -> Option<bool> {
        (self.phase == Phase::Connected).then(|| {
            self.held = !self.held;
            self.held
        })
    }

    /// Opening Transfer puts the call on hold first, as Teams does. Returns `Some(true)` when
    /// that changed the hold.
    pub fn transfer_opened(&mut self) -> Option<bool> {
        self.found.clear();
        if self.held {
            return None;
        }
        self.toggle_hold()
    }

    /// A new people search: its sequence number. Older answers are then dropped.
    pub fn find_started(&mut self) -> u64 {
        self.find_seq += 1;
        self.find_seq
    }

    /// The answer of search `seq`. False when it is stale.
    pub fn found(&mut self, seq: u64, found: Vec<Found>) -> bool {
        seq == self.find_seq && {
            self.found = found;
            true
        }
    }

    /// The person `mri` from the search, as a control `make` builds; the search is done.
    pub(super) fn pick(
        &mut self,
        mri: &str,
        make: fn(String, String) -> Control,
    ) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let f = self.found.iter().find(|f| f.mri == mri)?.clone();
        self.found.clear();
        self.find_seq += 1;
        Some(make(f.mri, f.name))
    }

    /// The invite box's pick: ring that person into the call.
    pub fn invite(&mut self, mri: &str) -> Option<Control> {
        let c = self.pick(mri, |mri, name| Control::AddPerson { mri, name })?;
        if let Control::AddPerson { name, .. } = &c {
            self.invite_status = format!("Calling {name}…");
        }
        Some(c)
    }

    /// The People panel's Admit: let one person in from the lobby. `None` unless a call runs and
    /// the roster has them waiting.
    pub fn admit(&mut self, mri: &str) -> Option<Control> {
        if !self.phase.active() {
            return None;
        }
        let p = self.roster.iter().find(|p| p.lobby && p.mri == mri)?;
        Some(Control::Admit {
            mri: p.mri.clone(),
            name: p.name.clone(),
        })
    }

    /// "Admit all": `None` unless a call runs and somebody waits in the lobby.
    pub fn admit_all(&mut self) -> Option<Control> {
        (self.phase.active() && self.roster.iter().any(|p| p.lobby)).then_some(Control::AdmitAll)
    }

    /// The transfer dialog's "Transfer now".
    pub fn transfer(&mut self, mri: &str) -> Option<Control> {
        self.pick(mri, |mri, name| Control::Transfer { mri, name })
    }

    /// Picks a microphone (`mic`) or speaker. Returns the device ids to switch to while a call
    /// runs.
    pub fn pick_device(&mut self, mic: bool, index: usize) -> Option<Control> {
        let (list, at) = if mic {
            (&self.devices.mics, &mut self.devices.mic)
        } else {
            (&self.devices.speakers, &mut self.devices.speaker)
        };
        if index >= list.len() || *at == index {
            return None;
        }
        *at = index;
        self.phase.active().then(|| Control::Devices {
            mic: self.mic_id(),
            speaker: self.speaker_id(),
        })
    }

    /// Picks a camera. True when the choice changed (it applies the next time the camera opens).
    pub fn pick_camera(&mut self, index: usize) -> bool {
        if index >= self.devices.cameras.len() || self.devices.camera == index {
            return false;
        }
        self.devices.camera = index;
        true
    }

    /// The chosen camera's id (`None`: the default).
    pub fn camera_id(&self) -> Option<String> {
        self.devices.cameras.get(self.devices.camera)?.id.clone()
    }

    /// The meeting's chat, whose "Meeting started" message tells when the meeting began (`None` for
    /// a 1:1 call, or a meeting joined by code).
    pub fn meeting_chat(&self) -> Option<String> {
        if !matches!(self.kind, CallKind::Meeting { .. }) {
            return None;
        }
        Some(self.chat_id.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| match self.target.as_ref()? {
                JoinTarget::Chat(c) => Some(c.clone()),
                JoinTarget::Link(u) => ::calendar::meeting_chat_thread_id(u),
                JoinTarget::Code { .. } => None,
            })
    }

    pub fn data(&self, now: Instant) -> CallData {
        let elapsed = match (self.phase, self.connected_at, self.duration) {
            (Phase::Ended, _, Some(d)) => d + self.head_start,
            (p, Some(t), _) if p.active() => now.saturating_duration_since(t) + self.head_start,
            _ => Duration::ZERO,
        };
        let mut people: Vec<Person> = self
            .roster
            .iter()
            .filter(|p| !p.is_bot())
            .cloned()
            .collect();
        for p in people.iter_mut().filter(|p| p.me) {
            p.muted = self.muted;
            p.hand = self.hand;
        }
        // In the call first, the lobby after; each in roster order.
        people.sort_by_key(|p| p.lobby);
        let in_lobby = people.iter().filter(|p| p.lobby).count();
        let can_admit = self.phase.active() && people.iter().any(|p| p.me && p.role != "attendee");
        let (invited, chat_people) = self.absent_people(&people);
        CallData {
            phase: self.phase.name(),
            kind: self.kind.name(),
            title: self.kind.title().to_owned(),
            my_name: self.my_name.clone(),
            timer: clock(elapsed),
            muted: self.muted,
            hand: self.hand,
            can_share: self.can_share && self.phase.active(),
            sharing: self.sharing,
            can_camera: self.can_camera && self.phase.active(),
            camera: self.camera,
            // While we present, nobody else does (a meeting has one presenter).
            presenter: if self.phase.active() && !self.sharing {
                self.presenter.clone()
            } else {
                String::new()
            },
            remote_control: remote_control_wanted(),
            control_mode: self.control_mode.clone(),
            control_name: self.control_name.clone(),
            prejoin_mic: self.mic_on,
            prejoin_camera: self.prejoin_camera,
            recording: self.recording,
            transcribing: self.transcribing,
            captions: self.captions,
            caption_lines: self.caption_lines.clone(),
            notice: self.notice.clone(),
            error: self.error.clone(),
            ended: self.ended.clone(),
            feedback_eligible: self.phase == Phase::Ended
                && self.duration.is_some_and(call_feedback::long_enough),
            chat_id: self.chat_id.clone(),
            can_rejoin: self.target.is_some(),
            held_name: if self.held
                && self.phase.active()
                && matches!(self.kind, CallKind::Meeting { .. })
            {
                self.held_title()
            } else {
                String::new()
            },
            in_call: people.len() - in_lobby,
            in_lobby,
            can_admit,
            people,
            mics: self.devices.mics.iter().map(|d| d.name.clone()).collect(),
            mic: self.devices.mic,
            mic_id: self.mic_id(),
            speakers: self
                .devices
                .speakers
                .iter()
                .map(|d| d.name.clone())
                .collect(),
            speaker: self.devices.speaker,
            cameras: self
                .devices
                .cameras
                .iter()
                .map(|d| d.name.clone())
                .collect(),
            camera_device: self.devices.camera,
            held: self.held,
            found: self.found.clone(),
            invite_status: self.invite_status.clone(),
            invited,
            chat_people,
            can_share_invite: self.phase.active() && self.join_link().is_some(),
            people_filter: self.people_filter.clone(),
            reactions: self
                .reactions
                .iter()
                .filter(|r| {
                    self.phase.active() && now.saturating_duration_since(r.at) < REACTION_SHOWN
                })
                .cloned()
                .collect(),
        }
    }
}
