use super::*;

impl Call {
    /// Camera on / off: the portal opens without blocking the call (its answer comes as a
    /// [`CameraMsg`]); turning it off tells a meeting at once. A 1:1 call is told by the run loop
    /// ([`Call::sync_reoffer`]), one re-offer at a time.
    pub(super) async fn camera_control(
        &mut self,
        on: bool,
        tx: &mpsc::UnboundedSender<CameraMsg>,
        sink: &EventSink,
    ) {
        if on {
            if self.meeting
                && share::meeting_mid(&self.media, &self.declared, camera::ROLE)
                    .await
                    .is_none()
            {
                let text = share::unavailable_text(camera::ROLE, &self.declared);
                sink.send(Event::Error(text.into()));
                return;
            }
            self.camera.open(tx);
        } else if self.camera.stop(sink).await && self.meeting {
            self.tell_camera(false, sink).await;
        }
    }

    pub(super) async fn camera_msg(
        &mut self,
        m: CameraMsg,
        tx: &mpsc::UnboundedSender<CameraMsg>,
        sink: &EventSink,
    ) {
        match m {
            // Turned off again while it opened: not started.
            CameraMsg::Opened(Ok(capture)) if !self.camera.opening() => {
                tracing::info!("camera opened after it was turned off: not started");
                (*capture).stop().await;
            }
            CameraMsg::Opened(Ok(capture)) => {
                match self.camera.start(*capture, &self.media, tx, sink) {
                    Ok(()) if self.meeting => self.tell_camera(true, sink).await,
                    Ok(()) => {}
                    Err(e) => {
                        tracing::warn!(%e, "camera not started");
                        self.camera.stop(sink).await;
                        sink.send(Event::Error(camera::open_error_text(&e.to_string())));
                    }
                }
            }
            CameraMsg::Opened(Err(e)) => {
                self.camera.open_failed();
                tracing::warn!(%e, "camera not opened");
                // Another app holds it: a calm notice, the button back off, no retry.
                let text = camera::open_error_text(&e);
                sink.send(if camera::in_use(&e) {
                    Event::Notice(text)
                } else {
                    Event::Error(text)
                });
            }
            CameraMsg::Ended => {
                if self.camera.stop(sink).await && self.meeting {
                    self.tell_camera(false, sink).await;
                }
            }
        }
    }

    /// Tells a meeting our camera sends (or stopped): `UpdateMediaDescriptions`. A 1:1 call is
    /// told by the run loop's re-offer ([`Call::sync_reoffer`]).
    pub(super) async fn tell_camera(&mut self, on: bool, sink: &EventSink) {
        let lines = Sending {
            camera: on,
            share: self.share.sharing(),
        };
        if let Err(e) = self.tell_meeting(camera::ROLE, lines, on).await {
            tracing::warn!("camera not signalled: {e:#}");
            if on {
                self.camera.stop(sink).await;
                sink.send(Event::Error(
                    "Others can't see your video: the call refused it.".into(),
                ));
            }
        }
    }

    pub(super) fn ended_text(&self) -> String {
        if self.meeting {
            "The meeting ended.".into()
        } else {
            "The call ended.".into()
        }
    }

    /// Answers a re-offer. One that restarts ICE (the lobby's admission, a 1:1 call moved to a
    /// media server: new mids and ufrags) is refused by the connected session and answered on a
    /// fresh one, which waits in [`Call::next`] and takes over once connected (the run loop); a
    /// follow-up offer for that one is answered on it. It supersedes our pending re-offer (hold,
    /// resume, camera, share): the answer says the hold, camera and share as they are now
    /// ([`Reoffers::told`]), and that re-offer's own answer or rejection, should one still come,
    /// changes nothing.
    pub(super) async fn renegotiate(
        &mut self,
        offer: &SdpBlob,
        links: &Links,
        held: bool,
        sink: &EventSink,
    ) {
        let sdp = match offer.decode() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%e, "undecodable re-offer");
                self.links.0.extend(links.0.clone());
                return;
            }
        };
        let for_next = match &self.next {
            Some(n) => !n.media.offer_restarts(&sdp).await,
            None => false,
        };
        let restarts = !for_next && self.media.offer_restarts(&sdp).await;
        // Their offer met our pending re-offer (glare): theirs wins, and our answer says what
        // ours asked for (the hold, camera and share as they are now). A restart leaves ours on
        // the old session; otherwise ours is withdrawn so the session can answer.
        if self.reoffers.pending.is_some() {
            if !restarts
                && !for_next
                && let Err(e) = self.media.rollback_offer().await
            {
                tracing::warn!(%e, "pending re-offer not rolled back");
            }
            self.reoffers.supersede();
            tracing::info!(
                superseded = ?self.reoffers.superseded.as_ref().map(|s| s.to),
                restarts,
                "their offer superseded our pending re-offer"
            );
        }
        // The answer, and whether it is the waiting session's.
        let answer = if for_next && let Some(n) = &self.next {
            n.media.accept_offer(&sdp).await.map(|a| (a, true))
        } else {
            self.accept_or_restart(&sdp).await
        };
        // A 1:1 answer says what we send: the camera line sends only while the camera is on,
        // the sharing line only while we share (webrtc-rs would say both send).
        let answer = answer.map(|(ans, to_next)| {
            if self.meeting {
                self.reoffers.told.held = held;
                return (ans, to_next);
            }
            let ans = direct_reanswer(&ans, &sdp, self.camera.on(), self.share.sharing(), held);
            // Our answer says the hold, camera and share as they are: no re-offer needed for them.
            self.reoffers.told = Said {
                held,
                camera: self.camera.on(),
                share: self.share.sharing(),
            };
            tracing::info!(
                held,
                to_next,
                theirs = %calling::media::sdp::directions(&sdp),
                ours = %calling::media::sdp::directions(&ans),
                "1:1 re-offer answered"
            );
            (ans, to_next)
        });
        match answer {
            Ok((ans, to_next)) => {
                // The media server's offer names its lines' stream ids (the waiting session's
                // are read when it takes over).
                match self.next.as_mut().filter(|_| to_next) {
                    Some(n) => n.offer = sdp.clone(),
                    None => {
                        self.video
                            .negotiated(&self.media, &sdp, remote_video::Phase::Reoffer)
                            .await
                    }
                }
                let mut leg = links.clone();
                if let Some(url) = links.get("mediaAnswer") {
                    match self
                        .ngc
                        .send_media_answer(&self.s, url, &MODALITIES, &SdpBlob::ngc(ans))
                        .await
                    {
                        // Teams ignored the first state update after admission (mautrix-teams),
                        // so publish the mute state again.
                        Ok(answered) => {
                            if !answered.0.is_empty() {
                                let names: Vec<&str> =
                                    answered.0.keys().map(String::as_str).collect();
                                tracing::info!(links = %names.join(","), "media answer named leg links");
                            }
                            leg.0.extend(answered.0);
                            let r = self
                                .ngc
                                .repeat_muted(
                                    &self.s,
                                    &self.links,
                                    &self.controller,
                                    &mut self.controls,
                                )
                                .await;
                            if let Err(e) = r {
                                tracing::warn!(%e, "mute state not repeated");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(%e, "re-offer answer refused");
                            sink.send(Event::Error("The media update was refused.".into()));
                        }
                    }
                }
                // The new leg's links are the call's once its session takes over.
                match self.next.as_mut().filter(|_| to_next) {
                    Some(n) => n.links.0.extend(leg.0),
                    None => self.links.0.extend(leg.0),
                }
            }
            Err(e) => {
                tracing::warn!(%e, "re-offer not answered");
                sink.send(Event::Error("Couldn't update the call's media.".into()));
                self.links.0.extend(links.0.clone());
            }
        }
    }

    /// Answers `sdp` on the current session, or, when it restarts the media, on a fresh session
    /// that waits in [`Call::next`] (replacing one already waiting). `true`: the fresh one.
    async fn accept_or_restart(&mut self, sdp: &str) -> calling::Result<(String, bool)> {
        match self.media.accept_offer(sdp).await {
            Ok(a) => Ok((a, false)),
            Err(e) => {
                tracing::info!(%e, "re-offer answered on a new media session");
                if self.ice.is_empty() {
                    self.ice = self.hub.ice_servers(&self.ngc.endpoints().region).await;
                }
                // None: host and server-reflexive candidates only, no relay to fall back on.
                tracing::info!(ice_servers = self.ice.len(), "new media session's TURN");
                // A meeting's fresh session has the gallery lines too, as its join had.
                match MediaSession::with_options(
                    self.ice.clone(),
                    restart_options(self.meeting, self.video_lines, sdp),
                )
                .await
                {
                    Ok((fresh, events)) => match fresh.accept_offer(sdp).await {
                        Ok(ans) => {
                            tracing::info!(
                                theirs = %calling::media::sdp::transport_summary(sdp),
                                ours = %calling::media::sdp::transport_summary(&ans),
                                "media restart answered on a new session; it takes over once connected"
                            );
                            let waiting = NextMedia {
                                media: fresh,
                                events,
                                offer: sdp.to_owned(),
                                links: Links::default(),
                                since: Instant::now(),
                            };
                            if let Some(replaced) = self.next.replace(waiting) {
                                tracing::info!(
                                    "a newer restart replaced the session still connecting"
                                );
                                let _ = replaced.media.close().await;
                            }
                            Ok((ans, true))
                        }
                        Err(e) => {
                            let _ = fresh.close().await;
                            Err(e)
                        }
                    },
                    Err(e) => Err(e),
                }
            }
        }
    }
}
