use super::*;

/// Call waiting: a second call while one runs.
#[cfg(any(test, feature = "calls"))]
impl CallManager {
    /// A call runs (joined or joining), shown in the call window.
    pub fn in_call(&self) -> bool {
        self.shared.model.lock().unwrap().phase.active()
    }

    /// Another call is on hold behind the shown one.
    pub fn has_held(&self) -> bool {
        self.shared.held.lock().unwrap().is_some()
    }

    /// Makes room for a call being answered while another runs: holds or leaves the current one.
    pub fn make_room(&self, current: CurrentCall) {
        match current {
            CurrentCall::Hold => {
                self.shared.hold_current();
            }
            CurrentCall::End => self.shared.end_current(),
        }
    }

    /// Starts the window on a call that is already connected, run by `handle`. False while
    /// another call runs.
    pub(super) fn adopt_handle(
        &self,
        kind: CallKind,
        start: impl FnOnce(EventSink) -> Box<dyn CallHandle>,
    ) -> bool {
        let devices = self.driver().devices();
        if !self
            .shared
            .model
            .lock()
            .unwrap()
            .adopted(kind, devices, Instant::now())
        {
            return false;
        }
        let generation = self.shared.new_generation();
        let handle = start(EventSink {
            shared: Arc::downgrade(&self.shared),
            generation,
        });
        *self.shared.handle.lock().unwrap() = Some(Arc::from(handle));
        self.shared.publish_presence();
        self.shared.push();
        self.shared.tick();
        true
    }

    /// A connected 1:1 call with no engine behind it (the fake call flows of the demo and the
    /// tests), after making room as `current` says.
    pub fn adopt_without_engine(&self, kind: CallKind, current: Option<CurrentCall>) -> bool {
        if let Some(c) = current {
            self.make_room(c);
        }
        self.adopt_handle(kind, |_| Box::new(NoHandle))
    }
}

/// The running manager, once IronTeams is signed in ([`CallManager::start`]); the 1:1 call flows
/// (`call_direct`) reach it here.
#[cfg(feature = "calls")]
pub(super) static MANAGER: Mutex<Option<Arc<CallManager>>> = Mutex::new(None);

#[cfg(feature = "calls")]
pub fn manager() -> Option<Arc<CallManager>> {
    MANAGER.lock().unwrap().clone()
}

/// The entry points of the 1:1 call flows (`call_direct`).
#[cfg(feature = "calls")]
impl CallManager {
    /// The chat header's "Share screen": the adopted call shares (the screen and window picker
    /// opens) now if it can, else as soon as its sharing line is ready.
    pub fn share_when_ready(&self) {
        let control = {
            let mut m = self.shared.model.lock().unwrap();
            m.share_when_ready = true;
            m.take_share_when_ready()
        };
        if let Some(c) = control {
            self.shared.control(c);
            self.shared.push();
        }
    }

    /// The entry point for a connected 1:1 call (answered or placed by the ringing flows): the
    /// manager runs it from here on (keepAlive, roster, controls, audio through cpal with echo
    /// cancellation, leave). Needs a tokio runtime. `current`: what happens to the call in
    /// progress (call waiting). Errors while another call runs.
    pub fn adopt(
        &self,
        session: calling::signaling::CallSession,
        links: calling::Links,
        media: calling::media::MediaSession,
        events: calling::media::MediaEvents,
        kind: CallKind,
        current: Option<CurrentCall>,
    ) -> anyhow::Result<()> {
        // Call waiting: the call in progress goes on hold (or ends) only now that the new one is
        // answered, so a failed answer leaves it untouched.
        if let Some(c) = current {
            self.make_room(c);
        }
        let (hub, shared) = (self.hub.clone(), self.shared.clone());
        let mut parts = Some((session, links, media, events));
        let adopted = self.adopt_handle(kind, |sink| {
            let (session, links, media, events) = parts.take().expect("taken once");
            let mut opts = shared.model.lock().unwrap().options();
            opts.camera = camera::take_accept_with_video();
            live::adopt(hub, session, links, media, events, opts, sink)
        });
        if !adopted {
            if let Some((_, _, media, _)) = parts {
                tokio::spawn(async move {
                    let _ = media.close().await;
                });
            }
            anyhow::bail!("{ALREADY_IN_CALL}");
        }
        Ok(())
    }

    /// A connected 1:1 call run by `fake` (UI tests).
    #[cfg(test)]
    pub fn adopt_fake(&self, fake: &FakeDriver, kind: CallKind) -> bool {
        let fake = fake.clone();
        self.adopt_handle(kind, move |sink| fake.adopt(sink))
    }

    /// Incoming call invitations, for the ringing flows.
    pub fn subscribe_invitations(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Box<calling::CallInvitation>> {
        self.hub.subscribe_invitations()
    }

    /// Call callbacks whose session is not the active call (a ringing call's cancel, an outgoing
    /// call's progress and acceptance) and other pushes.
    pub fn subscribe_unmatched(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<calling::ngc_trouter::CallPush> {
        self.hub.subscribe_unmatched()
    }

    /// The calling trouter's surl (callback links are built on it), while connected.
    pub fn surl(&self) -> Option<String> {
        self.hub.surl()
    }

    /// The conversation service client (discovered at sign-in, else now).
    pub async fn ngc(&self) -> anyhow::Result<calling::signaling::NgcClient> {
        self.hub.ngc_client().await
    }
}

impl Engine {
    /// The ordered part of a call window command (see `in_order`); a people search's answer
    /// comes back as the rest.
    pub(in super::super) fn call_cmd(self: &Arc<Self>, c: CallCmd) -> anyhow::Result<Option<Rest>> {
        match c {
            CallCmd::JoinChat(thread) => {
                let title = {
                    let v = self.view.lock().unwrap();
                    if v.current.as_deref() == Some(thread.as_str()) {
                        v.current_title.clone()
                    } else {
                        String::new()
                    }
                };
                let title = if title.is_empty() {
                    "Meeting".to_owned()
                } else {
                    title
                };
                self.calls
                    .prejoin(title, JoinTarget::Chat(thread.clone()), thread);
            }
            CallCmd::JoinOngoing(thread) => self.join_ongoing(&thread)?,
            CallCmd::Find(q) => return Ok(self.call_find(&q)),
            CallCmd::PersonCard(mri) => return Ok(self.call_person_card(mri)),
            CallCmd::PeopleOpen => {
                self.calls.command(CallCmd::PeopleOpen);
                return Ok(self.call_people_open());
            }
            CallCmd::ShareInvite => self.call_share_invite(),
            #[cfg(all(feature = "calls", not(test)))]
            CallCmd::Fx(video_fx::FxCmd::AddBackground) => {
                let e = self.clone();
                return Ok(in_order::rest(async move {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("Pictures", &["jpg", "jpeg", "png", "webp"])
                        .pick_file()
                        .await;
                    if let Some(file) = file {
                        let path = file.path().to_path_buf();
                        let calls = e.calls.clone();
                        tokio::task::spawn_blocking(move || calls.add_background(&path)).await?;
                    }
                    Ok(())
                }));
            }
            other => self.calls.command(other),
        }
        Ok(None)
    }

    /// A tile's click: the person's profile card, the one the chat opens, shows over the call
    /// window. A bot has no card.
    pub(super) fn call_person_card(self: &Arc<Self>, mri: String) -> Option<Rest> {
        if mri.is_empty() || mri.starts_with(BOT_PREFIX) {
            return None;
        }
        let e = self.clone();
        in_order::rest(async move { e.profile_in(mri, false).await })
    }

    /// Add people and Transfer's search: it starts here, in order with the commands around it, and
    /// the answer is shown when it comes (unless a newer search started).
    pub(super) fn call_find(self: &Arc<Self>, q: &str) -> Option<Rest> {
        let seq = self.calls.find_started();
        let q = q.trim().to_owned();
        if q.is_empty() {
            self.calls.found(seq, Vec::new());
            return None;
        }
        // Tests answer with `CallManager::test_found`.
        if cfg!(test) {
            return None;
        }
        let e = self.clone();
        in_order::rest(async move {
            match e.client.search_people(&e.me, &q).await {
                Ok(people) => {
                    let found = people
                        .into_iter()
                        .filter(|p| p.mri != e.me)
                        .map(|p| Found {
                            subtitle: p.job_title.or(p.email).unwrap_or_default(),
                            mri: p.mri,
                            name: p.display_name,
                        })
                        .collect();
                    e.calls.found(seq, found);
                }
                // The box keeps what it showed.
                Err(err) => tracing::warn!(%err, "people search for the call failed"),
            }
            Ok(())
        })
    }

    /// The calendar's Join for a meeting link: the pre-join screen. False when native calls are
    /// off, so the caller opens the link in the browser.
    pub(in super::super) fn call_prejoin_link(&self, title: &str, url: &str) -> bool {
        if !self.calls.native() {
            return false;
        }
        let chat = ::calendar::meeting_chat_thread_id(url).unwrap_or_default();
        self.calls
            .prejoin(title.to_owned(), JoinTarget::Link(url.to_owned()), chat);
        true
    }

    /// "Join with an ID": the pre-join screen for that meeting ID and passcode.
    pub(in super::super) fn call_prejoin_code(&self, id: &str, passcode: &str) -> bool {
        if !self.calls.native() {
            return false;
        }
        let id: String = id.chars().filter(|c| !c.is_whitespace()).collect();
        self.calls.prejoin(
            format!("Meeting {id}"),
            JoinTarget::Code {
                id,
                passcode: passcode.trim().to_owned(),
            },
            String::new(),
        );
        true
    }
}
