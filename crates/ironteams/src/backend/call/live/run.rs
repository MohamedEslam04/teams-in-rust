use super::*;

impl Call {
    /// The rest of `call join`: audio, then controls, media state and callbacks until the call
    /// ends or we leave.
    pub(super) async fn run(
        mut self,
        events: MediaEvents,
        mut pushes: mpsc::UnboundedReceiver<CallPush>,
        opts: JoinOptions,
        sink: EventSink,
        ctl: &mut mpsc::UnboundedReceiver<Control>,
        early: Vec<Control>,
    ) -> bool {
        let (sender_tx, sender_rx) = watch::channel(self.media.audio_sender());
        let (play_tx, play_rx) = watch::channel(None);
        let mut audio = Audio {
            sender: sender_rx,
            muted: Arc::new(AtomicBool::new(!opts.mic_on)),
            held: Arc::new(AtomicBool::new(false)),
            play: play_tx,
            send_task: None,
            playback: None,
        };
        if let Err(e) = audio.start(opts.mic.as_deref(), opts.speaker.as_deref()) {
            sink.send(Event::Error(format!("Audio device: {e:#}")));
        }
        // "Add this device" joins playing no sound; any other join plays it.
        calling::audio::set_output_off(opts.sound_off);
        let mut forwarders = vec![forward(play_rx.clone(), audio.held.clone(), events.audio)];
        let mut state_rx = events.state;
        let mut speakers_rx = events.speakers;
        let mut data_rx = events.data;
        let mut depacketizer = Depacketizer::default();
        self.video.show(events.video, &sink);
        let mut talk = Talk::default();
        let mut speakers_open = true;
        // The current session's state channel; shut once it failed while a restarted one connects.
        let mut state_open = true;
        if !opts.mic_on {
            self.control(Control::Mute(true), &mut audio, &sink).await;
        }
        for c in early {
            self.control(c, &mut audio, &sink).await;
        }
        let me = self.s.me.mri.clone();
        // Ours, to tell who is from another organization (the call's own roster entry names it
        // too, when the service sends tenant ids).
        let own_tenant = match self.hub.client.tenant_id().await {
            Ok(t) => Some(t),
            Err(_) => self.roster.get(&me).and_then(|p| p.tenant_id.clone()),
        };
        let started = Instant::now();
        // The service ended the call refusing the video lines we declared: the caller rejoins
        // audio only.
        let mut video_refused = false;
        let (mut in_lobby, mut was_in_call) = (false, false);
        let mut shown: Option<Phase> = None;
        let mut own_hand: Option<bool> = None;
        // A finished transfer ends the call, and we still leave our leg.
        let mut hang_up = false;
        let (share_tx, mut share_rx) = mpsc::unbounded_channel();
        let (camera_tx, mut camera_rx) = mpsc::unbounded_channel();
        // Who the roster leaves unlabelled is looked up once per person (profile lookups run
        // off the loop and come back here).
        let (standing_tx, mut standing_rx) = mpsc::unbounded_channel::<Vec<Profile>>();
        let mut standing: HashMap<String, Standing> = HashMap::new();
        let mut standing_asked: HashSet<String> = HashSet::new();
        // Joined with the camera on: it starts once the call can send it.
        let mut camera_wanted = opts.camera;
        let ended: Option<String> = loop {
            let mut roster_changed = false;
            let mut standing_changed = false;
            tokio::select! {
                c = ctl.recv() => match c {
                    None | Some(Control::Leave) => break None,
                    Some(Control::Share(on)) => self.share_control(on, &share_tx, &sink).await,
                    Some(Control::Camera(on)) => self.camera_control(on, &camera_tx, &sink).await,
                    Some(Control::Remote(a)) => self.control_action(a, &share_tx, &sink).await,
                    Some(c) => self.control(c, &mut audio, &sink).await,
                },
                Some(m) = share_rx.recv() => self.share_msg(m, &share_tx, &sink).await,
                Some(frame) = data_rx.recv() => {
                    if let Some(msg) = depacketizer.push(&frame) {
                        if msg.data_id == CONTROL_DATA_ID {
                            // Remote control (UNVERIFIED data id and payload).
                            let m = serde_json::from_slice(&msg.payload).ok().and_then(|v| decode(&v));
                            if let Some(m) = m {
                                self.control_message(m, &sink).await;
                            }
                        } else {
                            for c in captions_of(msg, &self.roster) {
                                sink.send(Event::Caption(c));
                            }
                        }
                    }
                }
                Some(m) = camera_rx.recv() => self.camera_msg(m, &camera_tx, &sink).await,
                Some(found) = standing_rx.recv() => {
                    for p in &found {
                        standing.insert(p.mri.clone(), Standing::from(p));
                    }
                    standing_changed = true;
                }
                heard = speakers_rx.changed(), if speakers_open => match heard {
                    Ok(()) => {
                        let sources = speakers_rx.borrow_and_update().clone();
                        roster_changed = talk.heard(&self.roster, &sources);
                    }
                    Err(_) => speakers_open = false,
                },
                st = state_rx.recv(), if state_open => match st {
                    // The session answering a media restart may still take over.
                    Some(MediaState::Failed | MediaState::Closed) | None if self.next.is_some() => {
                        tracing::info!(state = ?st, "media lost; waiting for the restarted session");
                        state_open = false;
                        self.link = Link::Disconnected;
                    }
                    Some(MediaState::Failed | MediaState::Closed) | None => {
                        tracing::info!(state = ?st, "media lost");
                        break Some("The connection was lost.".into());
                    }
                    Some(st) => {
                        tracing::info!(state = ?st, "media state");
                        self.link = link_of(st);
                    }
                },
                st = next_state(&mut self.next), if self.next.is_some() => match st {
                    // Connected (ICE and DTLS): it takes over the call's media.
                    Some(MediaState::Connected) => {
                        if let Some(NextMedia { media, events: ev, offer, links, since }) = self.next.take() {
                            let old = std::mem::replace(&mut self.media, media);
                            sender_tx.send_replace(self.media.audio_sender());
                            self.links.0.extend(links.0);
                            tracing::info!(
                                after_ms = since.elapsed().as_millis() as u64,
                                "the restarted media session connected and took over"
                            );
                            // The share was sent on the old session's line.
                            if self.share.stop().await {
                                share::leave_session(&self.ngc, &self.s, &mut self.share).await;
                                sink.send(Event::Error("Screen sharing stopped: the call's media restarted.".into()));
                            }
                            // So was the camera: it comes back on the new session's line.
                            camera_wanted |= self.camera.stop(&sink).await;
                            let _ = old.close().await;
                            self.video.negotiated(&self.media, &offer, remote_video::Phase::Restart).await;
                            self.video.show(ev.video, &sink);
                            state_rx = ev.state;
                            state_open = true;
                            speakers_rx = ev.speakers;
                            speakers_open = true;
                            data_rx = ev.data;
                            depacketizer = Depacketizer::default();
                            forwarders.push(forward(play_rx.clone(), audio.held.clone(), ev.audio));
                            self.link = Link::Connected;
                            roster_changed = true;
                        }
                    }
                    // It never connected: the call stays on its current session, if that is
                    // still up.
                    Some(MediaState::Failed | MediaState::Closed) | None => {
                        if let Some(n) = self.next.take() {
                            tracing::warn!(
                                state = ?st,
                                after_ms = n.since.elapsed().as_millis() as u64,
                                "the restarted media session failed; the call stays on its current one"
                            );
                            let _ = n.media.close().await;
                        }
                        if !state_open {
                            break Some("The connection was lost.".into());
                        }
                    }
                    Some(st) => tracing::info!(state = ?st, "restarted media session state"),
                },
                p = pushes.recv() => {
                    let Some(p) = p else { break Some(self.ended_text()) };
                    if self.controller.is_empty()
                        && let Some(c) = conversation_controller(&p.body)
                    {
                        self.controller = c;
                    }
                    let NgcPush::Call { event, .. } = classify(&p.url, &p.body) else { continue };
                    // A 1:1 call's callbacks are few: each one is logged, so a call the service
                    // ends right after the answer shows what came before the end.
                    if !self.meeting {
                        tracing::info!(event = %event.summary(), "1:1 call: callback");
                    }
                    match event {
                        CallEvent::Accepted { answer, links, keep_alive_interval_s, .. } => {
                            self.links.0.extend(links.0);
                            if let Some(a) = answer {
                                // New lines: the subscriptions follow.
                                roster_changed |= self.apply_answer(&a).await;
                            }
                            self.start_keep_alive(keep_alive_interval_s);
                        }
                        // The answer to a re-offer their offer superseded: its session moved on.
                        CallEvent::MediaAnswer { links, .. }
                            if self.reoffer.is_none() && self.superseded.is_some() =>
                        {
                            let superseded = self.superseded.take();
                            tracing::info!(?superseded, "answer to a superseded re-offer ignored");
                            self.links.0.extend(links.0);
                        }
                        CallEvent::MediaAnswer { answer, links } => {
                            self.reoffer = None;
                            match self.pending_offer.take() {
                                Some(ours) => self.apply_reoffer_answer(&answer, &ours, &links, &sink).await,
                                None => roster_changed |= self.apply_answer(&answer).await,
                            }
                            self.links.0.extend(links.0);
                        }
                        CallEvent::MediaRenegotiation { offer, links } => {
                            let held = audio.held.load(Ordering::Relaxed);
                            // A restart's session takes over once connected (`next_state` above);
                            // `renegotiate` keeps the links.
                            self.renegotiate(&offer, &links, held, &sink).await;
                            // Admission re-offers the media; we are out of the lobby.
                            in_lobby = false;
                            // The lines may have changed: the subscriptions start over.
                            roster_changed = true;
                        }
                        // Our re-offer refused (a browser that can't answer it: 488/3102
                        // `CallEndReasonMediaAnswerError`), not the call: it goes on, and the
                        // other side's recovery offer comes as `call/mediaRenegotiation`. One their
                        // offer superseded (live 2026-11-23: a resume met a media restart, then was
                        // refused) puts nothing back: the answer to theirs said what it asked for.
                        CallEvent::Rejected(r) if self.reoffer.is_some() || self.superseded.is_some() => {
                            tracing::warn!(code = r.code, sub = r.sub_code, phrase = %r.phrase, "re-offer refused");
                            match refused_reoffer(&mut self.superseded, &mut self.reoffer) {
                                Some(Refused::Pending(reoffer)) => self.refused(reoffer, &audio, &sink).await,
                                Some(Refused::Superseded(reoffer)) => tracing::info!(
                                    ?reoffer,
                                    "the refused re-offer was for the session the media restart replaced: nothing undone"
                                ),
                                None => {}
                            }
                        }
                        CallEvent::Rejected(r) | CallEvent::Ended(r) => {
                            tracing::info!(
                                code = r.code,
                                sub = r.sub_code,
                                phrase = %r.phrase,
                                "call ended by the service"
                            );
                            video_refused = fallback::call_refused_video(
                                self.video_lines && !self.declared.is_empty(),
                                &r,
                                started.elapsed(),
                            );
                            break Some(self.ended_text());
                        }
                        CallEvent::RosterUpdate(delta) => {
                            roster_changed = self.roster.apply(&delta);
                            self.bots.roster(&delta);
                            self.step_bots(&sink).await;
                        }
                        CallEvent::ConversationUpdate(body) => {
                            if let Some(snapshot) = roster_snapshot(&body) {
                                roster_changed = self.roster.apply_snapshot(snapshot);
                                self.bots.roster(snapshot);
                            }
                            if let Some(on) = recording_state(&body) {
                                sink.send(Event::Recording(on));
                            }
                            for ev in self.bots.conversation(&body) {
                                sink.send(ev);
                            }
                            self.step_bots(&sink).await;
                        }
                        // Live reactions (verified live 2026-11-21: a Teams web reaction floats
                        // here with its sender's name; see `controls::parse_reactions`).
                        CallEvent::Other { scope, event, body }
                            if scope == "conversation" && event == "receiveMessage" =>
                        {
                            tracing::debug!(%body, "conversation message");
                            let reactions = parse_reactions(&body);
                            // Our own reaction coming back: the conversation took it and fans it
                            // out (a live check that the other side was sent it too).
                            for r in reactions.iter().filter(|r| r.mri == me) {
                                let reaction = r.reaction.wire_name();
                                tracing::info!(reaction, "own reaction echoed by the conversation");
                            }
                            for r in reactions.into_iter().filter(|r| r.mri != me) {
                                if let Some(name) = reaction_named(r.reaction.wire_name()) {
                                    sink.send(Event::Reaction { mri: r.mri, name });
                                }
                            }
                        }
                        CallEvent::Other { scope, event, body }
                            if scope == "conversation" && is_session_event(&event) =>
                        {
                            self.session_frame(&event, &body, &sink).await;
                        }
                        CallEvent::Other { event, body, .. } if event == "addParticipantFailure" => {
                            // The recorder bot's invitation when one is pending, else a person
                            // from the invite box.
                            let refused = self.bots.bot_refused(&body);
                            if refused.is_empty() {
                                tracing::warn!("adding a person failed");
                                sink.send(Event::Error("Couldn't add that person to the call.".into()));
                            }
                            for ev in refused {
                                sink.send(ev);
                            }
                        }
                        CallEvent::Other { scope, event, body }
                            if scope == "call" && event == "transferCompletion" =>
                        {
                            match parse_transfer_completion(&body) {
                                Some(TransferOutcome::Done) => {
                                    let to = self.transferring.take().unwrap_or_default();
                                    hang_up = true;
                                    break Some(format!("You transferred the call to {to}."));
                                }
                                Some(TransferOutcome::Failed { code, sub_code }) => {
                                    tracing::warn!(code, sub_code, "transfer failed");
                                    self.transferring = None;
                                    sink.send(Event::Error("The transfer didn't go through. The call is still on hold.".into()));
                                }
                                None => {}
                            }
                        }
                        CallEvent::MediaAcknowledgement { links } => self.links.0.extend(links.0),
                        CallEvent::Other { event, body, .. } if event == "controlVideoStreaming" => {
                            self.share.sender_control(&body);
                            self.camera.sender_control(&body);
                        }
                        CallEvent::Progress { .. } | CallEvent::Other { .. } => {}
                    }
                }
            }
            if standing_changed && !roster_changed {
                sink.send(Event::Roster(people(
                    &self.roster,
                    &me,
                    own_tenant.as_deref(),
                    &standing,
                    &talk,
                    &self.video,
                )));
            }
            // Our own share changes who the stage may show (never our own screen).
            let own_share_changed = self.video.set_own_share(self.share.sharing());
            if roster_changed || own_share_changed {
                // A meeting, or a 1:1 call whose media a server took over: its lines carry
                // only what we subscribe to.
                if self.video.subscribes() {
                    let spoke = |m: &str| talk.last.get(m).copied().unwrap_or(0);
                    self.video.reselect(
                        &self.ngc,
                        &self.s,
                        &self.links,
                        &self.roster,
                        spoke,
                        &sink,
                    );
                }
                if let Some(name) = self.video.presenter_changed(&self.roster, &me) {
                    sink.send(Event::Presenter(name));
                }
                sink.send(Event::Roster(people(
                    &self.roster,
                    &me,
                    own_tenant.as_deref(),
                    &standing,
                    &talk,
                    &self.video,
                )));
                let unknown: Vec<String> = self
                    .roster
                    .participants()
                    .iter()
                    .filter(|p| p.mri != me && p.mri.starts_with("8:orgid:") && !p.guest)
                    .filter(|p| standing_asked.insert(p.mri.clone()))
                    .map(|p| p.mri.clone())
                    .collect();
                if !unknown.is_empty() {
                    let (hub, tx) = (self.hub.clone(), standing_tx.clone());
                    tokio::spawn(async move {
                        match hub.client.profiles(&unknown).await {
                            Ok(found) => {
                                let _ = tx.send(found);
                            }
                            Err(err) => tracing::debug!(%err, "no profiles for the call's people"),
                        }
                    });
                }
                if let Some(p) = self.roster.get(&me) {
                    in_lobby = p.presence == Presence::Lobby;
                    let hand = p.hand.is_some();
                    if own_hand.replace(hand).is_some_and(|h| h != hand) {
                        sink.send(Event::Hand(hand));
                    }
                }
            }
            let can_share = self.link == Link::Connected
                && share::available(&self.media, &self.links, self.declared_lines()).await;
            if let Some((available, sharing)) = self.share.changed(can_share) {
                sink.send(Event::Share { available, sharing });
            }
            if remote_control::wanted() {
                if !self.share.sharing() {
                    // The desktop's input consent was for that share.
                    self.control.set_share(None);
                }
                // Control ends with our share, or with the presenter's.
                let step = self.control.machine.context(&self.control_ctx());
                self.apply_control(step, &sink).await;
                self.control_changed(&sink);
            }
            let can_camera = self.link == Link::Connected
                && camera::available(&self.media, &self.links, self.declared_lines()).await;
            if can_camera && std::mem::take(&mut camera_wanted) {
                self.camera.open(&camera_tx);
            }
            if let Some((available, on)) = self.camera.changed(can_camera) {
                sink.send(Event::Camera { available, on });
            }
            let phase = live_phase(self.link, in_lobby && self.meeting, was_in_call);
            was_in_call |= phase == Phase::Connected;
            if shown != Some(phase) {
                shown = Some(phase);
                sink.send(match phase {
                    Phase::Lobby => Event::Lobby,
                    Phase::Connected => Event::Connected,
                    Phase::Reconnecting => Event::Reconnecting,
                    _ => Event::Connecting,
                });
            }
        };

        if ended.is_none() || hang_up {
            let leave = if self.links.get("leave").is_some() || self.links.get("callLeg").is_some()
            {
                self.ngc.hang_up(&self.s, &self.links).await
            } else {
                self.ngc.leave_controller(&self.s, &self.controller).await
            };
            if let Err(e) = leave {
                tracing::warn!(%e, "leave failed");
            }
        }
        if let Some(k) = self.keep_alive.take() {
            k.abort();
        }
        self.share.stop().await;
        self.control.stop();
        self.camera.stop(&sink).await;
        self.video.stop(&sink);
        audio.stop();
        // The next call (a 1:1 one too) plays its sound.
        calling::audio::set_output_off(false);
        let _ = self.media.close().await;
        if let Some(n) = self.next.take() {
            let _ = n.media.close().await;
        }
        for f in forwarders {
            f.abort();
        }
        self.hub.remove_session(self.s.callbacks.session_id());
        match ended {
            Some(_) if video_refused => true,
            Some(reason) => {
                sink.send(Event::Ended(reason));
                false
            }
            None => false,
        }
    }
}
