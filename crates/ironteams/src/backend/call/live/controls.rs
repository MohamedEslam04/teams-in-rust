use super::*;

impl Call {
    /// Share / Stop sharing: the picker opens without blocking the call (its answer comes as a
    /// [`ShareMsg`]); stopping tells the service at once.
    pub(super) async fn share_control(
        &mut self,
        on: bool,
        tx: &mpsc::UnboundedSender<ShareMsg>,
        sink: &EventSink,
    ) {
        if on {
            if self.meeting
                && share::meeting_mid(&self.media, &self.declared, share::ROLE)
                    .await
                    .is_none()
            {
                let text = share::unavailable_text(share::ROLE, &self.declared);
                sink.send(Event::Error(text.into()));
                return;
            }
            self.share.pick(tx);
        } else if self.share.stop().await {
            self.tell_share(false, sink).await;
        }
    }

    pub(super) async fn share_msg(
        &mut self,
        m: ShareMsg,
        tx: &mpsc::UnboundedSender<ShareMsg>,
        sink: &EventSink,
    ) {
        match m {
            ShareMsg::Picked(Ok(Some(capture))) => {
                let area = capture.area;
                match self.share.start(*capture, &self.media, tx) {
                    Ok(()) => {
                        self.control.set_share(Some(area));
                        self.share_started(tx, sink).await;
                    }
                    Err(e) => {
                        tracing::warn!(%e, "screen share not started");
                        self.share.stop().await;
                        sink.send(Event::Error(share::capture_error_text(&e.to_string())));
                    }
                }
            }
            ShareMsg::Picked(Ok(None)) => self.share.picked_nothing(),
            ShareMsg::Picked(Err(e)) => {
                self.share.picked_nothing();
                tracing::warn!(%e, "screen capture failed");
                sink.send(Event::Error(share::capture_error_text(&e)));
            }
            ShareMsg::Input(input) => self.input_answered(input, sink).await,
            ShareMsg::Ended => {
                if self.share.stop().await {
                    self.tell_share(false, sink).await;
                }
            }
            ShareMsg::GrantTimeout(correlation) => {
                if share::grant_timed_out(&mut self.share, &correlation) && self.share.sharing() {
                    tracing::warn!(
                        "the meeting granted no content-sharing session: sharing anyway"
                    );
                    self.tell_share(true, sink).await;
                }
            }
        }
    }

    /// Our capture runs: a meeting is asked for its content-sharing session first, and the
    /// share goes out once it is granted (or the call has no session to ask for).
    pub(super) async fn share_started(
        &mut self,
        tx: &mpsc::UnboundedSender<ShareMsg>,
        sink: &EventSink,
    ) {
        if !self.meeting {
            return self.tell_share(true, sink).await;
        }
        match share::start_session(&self.ngc, &self.s, &self.links, &mut self.share, tx).await {
            Ok(true) => self.tell_share(true, sink).await,
            Ok(false) => tracing::info!("waiting for the meeting to grant the sharing session"),
            Err(e) => {
                tracing::warn!("no content-sharing session: {e:#}");
                self.tell_share(true, sink).await;
            }
        }
    }

    /// A callback of the content-sharing session (`addModalitySuccess`, `contentSharingUpdate`,
    /// `contentSharingEnd`, `addModalityFailure`).
    pub(super) async fn session_frame(
        &mut self,
        event: &str,
        body: &serde_json::Value,
        sink: &EventSink,
    ) {
        match self.share.session_frame(event, body) {
            SessionNews::Granted => {
                if self.share.sharing() {
                    self.tell_share(true, sink).await;
                }
            }
            SessionNews::Refused => {
                if self.share.stop().await {
                    sink.send(Event::Error("The meeting didn't let you present.".into()));
                }
            }
            SessionNews::Ended => {
                if self.share.stop().await {
                    self.tell_share(false, sink).await;
                    sink.send(Event::Error(
                        "You stopped presenting: someone else is presenting now.".into(),
                    ));
                }
            }
            SessionNews::Nothing => {}
        }
        if remote_control::wanted()
            && event == SESSION_UPDATE
            && let Some(m) = parse_session_message(body)
        {
            self.control_message(m, sink).await;
        }
    }

    /// What remote control needs to know of the call.
    pub(super) fn control_ctx(&self) -> Ctx {
        let me = &self.s.me.mri;
        Ctx {
            can_give: self.share.sharing(),
            input_allowed: self.control.can_inject(),
            // From the roster: a 1:1 call has none, so control is a meeting feature for now.
            presenter: remote_video::presenter_mri(&self.roster, me, self.share.sharing()).map(
                |mri| {
                    let name = self.person_name(&mri);
                    (mri, name)
                },
            ),
        }
    }

    pub(super) fn person_name(&self, mri: &str) -> String {
        match self.roster.get(mri).map(|p| p.display_name.as_str()) {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => "Someone".into(),
        }
    }

    /// A remote-control command of the call window.
    pub(super) async fn control_action(
        &mut self,
        a: ControlAction,
        tx: &mpsc::UnboundedSender<ShareMsg>,
        sink: &EventSink,
    ) {
        if !remote_control::wanted() {
            return;
        }
        let ctx = self.control_ctx();
        if self.control.machine.needs_input_consent(&a, &ctx) {
            // Granting control is when the desktop is asked to allow input (not at share
            // time); the grant goes ahead once it says yes (`input_answered`).
            if let Some(area) = self.control.share_area()
                && self.control.asking.is_none()
            {
                self.control.asking = Some(a);
                share::ask_input(area, tx);
            }
            return;
        }
        self.run_control_action(a, sink).await;
    }

    pub(super) async fn run_control_action(&mut self, a: ControlAction, sink: &EventSink) {
        let ctx = self.control_ctx();
        let step = self
            .control
            .machine
            .local(&a, &self.s.me.mri, &ctx, Instant::now());
        self.apply_control(step, sink).await;
        self.control_changed(sink);
    }

    /// The desktop answered the input-consent question: carry the waiting grant out, or turn it
    /// down.
    pub(super) async fn input_answered(
        &mut self,
        input: Result<Option<Box<InputInjector>>, String>,
        sink: &EventSink,
    ) {
        let waiting = self.control.asking.take();
        match (input, waiting) {
            (Ok(Some(injector)), Some(waiting)) if self.share.sharing() => {
                self.control.set_injector(Some(*injector));
                self.run_control_action(waiting, sink).await;
            }
            (input, waiting) => {
                match input {
                    Ok(Some(injector)) => injector.close().await,
                    Ok(None) => {}
                    Err(e) => tracing::warn!(%e, "remote control not allowed"),
                }
                if let Some(waiting) = waiting {
                    sink.send(Event::Error(
                        "IronTeams needs permission to control your screen.".into(),
                    ));
                    // A viewer who asked is told no.
                    if matches!(waiting, ControlAction::Answer(true)) {
                        self.run_control_action(ControlAction::Answer(false), sink)
                            .await;
                    }
                }
            }
        }
    }

    /// A remote-control message from the other side.
    pub(super) async fn control_message(&mut self, m: ControlMessage, sink: &EventSink) {
        if !remote_control::wanted() {
            return;
        }
        let ctx = self.control_ctx();
        let me = self.s.me.mri.clone();
        let step = {
            let roster = &self.roster;
            let name_of = |mri: &str| match roster.get(mri).map(|p| p.display_name.as_str()) {
                Some(n) if !n.is_empty() => n.to_owned(),
                _ => "Someone".into(),
            };
            self.control
                .machine
                .remote(m, &me, &ctx, name_of, Instant::now())
        };
        self.apply_control(step, sink).await;
        self.control_changed(sink);
    }

    /// Tells the call window what changed in control, and shows or removes the controller's
    /// cursor.
    pub(super) fn control_changed(&mut self, sink: &EventSink) {
        self.control
            .sync_overlay(|mri| sink.photo(mri).map(remote_control::photo));
        if let Some(ev) = self.control.changed() {
            sink.send(ev);
        }
    }

    /// Carries a remote-control step out over the best-guess transport.
    pub(super) async fn apply_control(&mut self, step: Step, sink: &EventSink) {
        if step == Step::default() {
            return;
        }
        let presenter_links =
            remote_video::presenter_mri(&self.roster, &self.s.me.mri, self.share.sharing())
                .and_then(|mri| self.roster.get(&mri).map(|p| p.sharing_links.clone()))
                .unwrap_or_default();
        let channel = NgcControlChannel {
            ngc: self.ngc.clone(),
            s: self.s.clone(),
            own: self.share.session.clone(),
            presenter_links,
            data: self.media.data_sender(),
            sequence: self.control.next_sequence(),
        };
        for ev in self.control.apply(step, &channel).await {
            sink.send(ev);
        }
    }

    /// In a meeting, the video lines its join declared (what [`share::available`] needs); `None`
    /// in a 1:1 call.
    pub(super) fn declared_lines(&self) -> Option<&[String]> {
        self.meeting.then_some(self.declared.as_slice())
    }

    /// A meeting's `UpdateMediaDescriptions` for a change of `role`'s line (now `sending`, the
    /// others as `lines` say). Nothing is posted for a line the join did not declare
    /// ([`share::meeting_update`]): the service ends the call after refusing such an update.
    pub(super) async fn tell_meeting(
        &mut self,
        role: calling::video::VideoRole,
        lines: Sending,
        sending: bool,
    ) -> anyhow::Result<()> {
        let update = share::meeting_update(&self.media, &self.declared, role, lines).await?;
        let request_id = self.share.next_request_id();
        share::tell_meeting(
            &self.ngc,
            &self.s,
            &self.links,
            &update,
            role,
            sending,
            request_id,
        )
        .await
    }

    /// Tells the service our sharing line sends (or stopped): `UpdateMediaDescriptions` in a
    /// meeting, a re-offer in a 1:1 call.
    pub(super) async fn tell_share(&mut self, sending: bool, sink: &EventSink) {
        let r = if self.meeting {
            let lines = Sending {
                camera: self.camera.on(),
                share: sending,
            };
            self.tell_meeting(share::ROLE, lines, sending).await
        } else {
            // The answer comes inline or on `call/mediaAnswer/`, which `apply_answer` takes.
            self.answered = false;
            let camera_on = self.camera.on();
            match share::reoffer_direct(
                &self.ngc,
                &self.s,
                &self.links,
                &self.media,
                sending,
                camera_on,
            )
            .await
            {
                Ok(Some(a)) => {
                    self.apply_answer(&a).await;
                    Ok(())
                }
                Ok(None) => {
                    self.reoffer = Some(Reoffer::Share(sending));
                    Ok(())
                }
                Err(e) => {
                    self.answered = true;
                    if let Err(e) = self.media.rollback_offer().await {
                        tracing::warn!(%e, "unsent re-offer not rolled back");
                    }
                    Err(e)
                }
            }
        };
        if !sending {
            share::leave_session(&self.ngc, &self.s, &mut self.share).await;
        }
        if let Err(e) = r {
            tracing::warn!("share not signalled: {e:#}");
            if sending {
                self.share.stop().await;
                share::leave_session(&self.ngc, &self.s, &mut self.share).await;
                sink.send(Event::Error(
                    "Others can't see your screen: the call refused the share.".into(),
                ));
            }
        }
    }
}
