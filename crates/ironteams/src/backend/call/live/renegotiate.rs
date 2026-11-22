use super::*;

impl Call {
    /// Camera on / off: the portal opens without blocking the call (its answer comes as a
    /// [`CameraMsg`]); turning it off tells the service at once.
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
        } else if self.camera.stop(sink).await {
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
            CameraMsg::Opened(Ok(capture)) => {
                match self.camera.start(*capture, &self.media, tx, sink) {
                    Ok(()) => self.tell_camera(true, sink).await,
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
                sink.send(Event::Error(camera::open_error_text(&e)));
            }
            CameraMsg::Ended => {
                if self.camera.stop(sink).await {
                    self.tell_camera(false, sink).await;
                }
            }
        }
    }

    /// Tells the service our camera sends (or stopped): `UpdateMediaDescriptions` in a meeting,
    /// a re-offer in a 1:1 call.
    pub(super) async fn tell_camera(&mut self, on: bool, sink: &EventSink) {
        let r = if self.meeting {
            let lines = Sending {
                camera: on,
                share: self.share.sharing(),
            };
            self.tell_meeting(camera::ROLE, lines, on).await
        } else {
            // The answer comes inline or on `call/mediaAnswer/`, which `apply_answer` takes.
            self.answered = false;
            let sharing = self.share.sharing();
            match camera::reoffer_direct(&self.ngc, &self.s, &self.links, &self.media, on, sharing)
                .await
            {
                Ok(Some(a)) => {
                    self.apply_answer(&a).await;
                    Ok(())
                }
                Ok(None) => {
                    self.reoffer = Some(Reoffer::Camera(on));
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
        if let Err(e) = r {
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

    /// Answers a re-offer. One that restarts ICE (the lobby's admission: new mids and ufrags) is
    /// refused by the connected session and answered on a fresh one, which replaces it; its
    /// events are returned so the caller follows them.
    pub(super) async fn renegotiate(
        &mut self,
        offer: &SdpBlob,
        links: &Links,
        sender: &watch::Sender<AudioSender>,
        sink: &EventSink,
    ) -> Option<MediaEvents> {
        let sdp = match offer.decode() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%e, "undecodable re-offer");
                return None;
            }
        };
        let mut fresh_events = None;
        let answer = match self.media.accept_offer(&sdp).await {
            Ok(a) => Ok(a),
            Err(e) => {
                tracing::info!(%e, "re-offer answered on a new media session");
                if self.ice.is_empty() {
                    self.ice = self.hub.ice_servers(&self.ngc.endpoints().region).await;
                }
                match MediaSession::with_options(
                    self.ice.clone(),
                    media_options(self.meeting, self.video_lines),
                )
                .await
                {
                    Ok((fresh, ev)) => match fresh.accept_offer(&sdp).await {
                        Ok(ans) => {
                            let old = std::mem::replace(&mut self.media, fresh);
                            let _ = old.close().await;
                            sender.send_replace(self.media.audio_sender());
                            self.link = Link::New;
                            fresh_events = Some(ev);
                            Ok(ans)
                        }
                        Err(e) => {
                            let _ = fresh.close().await;
                            Err(e)
                        }
                    },
                    Err(e) => Err(e),
                }
            }
        };
        match answer {
            Ok(ans) => {
                // The media server's offer names its lines' stream ids.
                self.video.negotiated(&self.media, &sdp).await;
                if let Some(url) = links.get("mediaAnswer") {
                    match self
                        .ngc
                        .send_media_answer(&self.s, url, &MODALITIES, &SdpBlob::ngc(ans))
                        .await
                    {
                        // Teams ignored the first state update after admission (mautrix-teams),
                        // so publish the mute state again.
                        Ok(()) => {
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
            }
            Err(e) => {
                tracing::warn!(%e, "re-offer not answered");
                sink.send(Event::Error("Couldn't update the call's media.".into()));
            }
        }
        fresh_events
    }
}
