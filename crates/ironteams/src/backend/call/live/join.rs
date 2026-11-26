use super::*;

pub(super) async fn run_meeting(
    hub: Arc<Hub>,
    target: JoinTarget,
    mut opts: JoinOptions,
    sink: EventSink,
    mut ctl: mpsc::UnboundedReceiver<Control>,
) {
    sink.send(Event::Connecting);
    // The join declares its video lines unless a refusal was seen in this session; a refusal
    // makes one more, audio-only attempt (see [`fallback`]).
    let mut video = hub.video.video_allowed() && (camera::wanted() || share::wanted());
    loop {
        let setup = join_meeting(hub.clone(), target.clone(), opts.clone(), video);
        tokio::pin!(setup);
        // Controls pressed while joining wait; Leave (or closing the window) gives up the join.
        let mut early = Vec::new();
        let joined = loop {
            tokio::select! {
                r = &mut setup => break r,
                c = ctl.recv() => match c {
                    None | Some(Control::Leave) => return,
                    Some(c) => early.push(c),
                },
            }
        };
        match joined {
            // `run` is true when the service ended the call refusing the video lines.
            Ok((call, events, pushes)) => {
                let refused = call
                    .run(events, pushes, opts.clone(), sink.clone(), &mut ctl, early)
                    .await;
                if !refused {
                    return;
                }
            }
            Err(e) => {
                let text = format!("{e:#}");
                if !fallback::join_refused_video(video, &text) {
                    tracing::warn!("join failed: {}", teams_proto::logsafe::text(&text));
                    sink.send(Event::Error(format!("Couldn't join: {text}")));
                    sink.send(Event::Ended("Couldn't join the meeting.".into()));
                    return;
                }
                tracing::info!(refusal = %text, "the meeting refused the join's video lines");
            }
        }
        hub.video.remember_refusal();
        video = false;
        // Nothing to turn on in an audio-only call.
        opts.camera = false;
        sink.send(Event::Connecting);
        sink.send(Event::Notice(fallback::NOTE.into()));
    }
}

pub(super) type Joined = (Call, MediaEvents, mpsc::UnboundedReceiver<CallPush>);

/// Discover, TURN, trouter, media offer, join (the first half of `call join`).
pub(super) async fn join_meeting(
    hub: Arc<Hub>,
    target: JoinTarget,
    opts: JoinOptions,
    with_video: bool,
) -> anyhow::Result<Joined> {
    let target = hub.resolve(target).await?;
    let ngc = hub.ngc_client().await?;
    let surl = hub
        .connected_surl()
        .await
        .context("the calling connection didn't come up")?;
    let ice = hub.ice_servers(&ngc.endpoints().region).await;
    // A meeting join also offers the gallery's receive-only lines (UNVERIFIED), unless `with_video`
    // is off: the audio-only rejoin after the media servers refused the video lines.
    let options = media_options(true, with_video);
    let (media, events) = MediaSession::with_options(ice.clone(), options).await?;
    let offer = media.create_offer().await?;
    let modalities = remote_video::join_modalities(with_video);
    let declared = declared_video_mids(&offer, modalities);
    tracing::info!(?declared, "meeting join declares its video lines");
    let me = hub.client.self_mri().await?;
    let s = CallSession::new(&surl, &me, &opts.name);
    // Callbacks can arrive before the join answers.
    let pushes = hub.add_session(s.callbacks.session_id());
    let conv = match ngc
        .join(&s, &target, &SdpBlob::ngc(offer), modalities)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            hub.remove_session(s.callbacks.session_id());
            let _ = media.close().await;
            return Err(e.into());
        }
    };
    let mut roster = Roster::default();
    let mut bots = Bots::new(target.thread_id.clone(), true);
    if let Some(snapshot) = roster_snapshot(&conv.raw) {
        roster.apply_snapshot(snapshot);
        bots.roster(snapshot);
    }
    let mut answered = false;
    let mut video = RemoteVideo::default();
    if let Some(ans) = find_blob(&conv.raw) {
        match ans.decode() {
            Ok(sdp) => match media.set_answer(&sdp).await {
                Ok(()) => {
                    answered = true;
                    video
                        .negotiated(&media, &sdp, remote_video::Phase::Join)
                        .await;
                }
                Err(e) => tracing::warn!(%e, "inline media answer refused"),
            },
            Err(e) => tracing::warn!(%e, "undecodable inline answer"),
        }
    }
    let call = Call {
        hub,
        ngc,
        s,
        links: conv.links.clone(),
        controller: conv.controller.clone(),
        media,
        ice,
        answered,
        roster,
        controls: Controls::default(),
        meeting: true,
        video_lines: with_video,
        declared,
        keep_alive: None,
        reoffers: Reoffers::default(),
        next: None,
        transferring: None,
        link: Link::New,
        share: Share::default(),
        bots,
        camera: Camera::default(),
        video,
        control: RemoteControl::default(),
    };
    Ok((call, events, pushes))
}

/// Runs a connected direct call handed over by the ringing flows.
pub fn adopt(
    hub: Arc<Hub>,
    session: CallSession,
    links: Links,
    media: MediaSession,
    events: MediaEvents,
    opts: JoinOptions,
    sink: EventSink,
) -> Box<dyn CallHandle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let pushes = hub.add_session(session.callbacks.session_id());
    tokio::spawn(async move {
        let ngc = match hub.ngc_client().await {
            Ok(n) => n,
            Err(e) => {
                hub.remove_session(session.callbacks.session_id());
                let _ = media.close().await;
                sink.send(Event::Error(format!("{e:#}")));
                sink.send(Event::Ended("The call ended.".into()));
                return;
            }
        };
        // An answered 1:1 call has its controller from the attach answer; a placed one may learn
        // it from its conversation push.
        let controller = links
            .get(calling::signaling::bodies::CONVERSATION_CONTROLLER)
            .unwrap_or_default()
            .to_owned();
        let mut call = Call {
            hub,
            ngc,
            s: session,
            links,
            controller,
            media,
            ice: vec![],
            answered: true,
            roster: Roster::default(),
            controls: Controls::default(),
            meeting: false,
            video_lines: true,
            declared: Vec::new(),
            keep_alive: None,
            reoffers: Reoffers::default(),
            next: None,
            transferring: None,
            link: Link::Connected,
            share: Share::default(),
            bots: Bots::new(None, false),
            camera: Camera::default(),
            video: RemoteVideo::default(),
            control: RemoteControl::default(),
        };
        // A direct call's sharing line shows the other side's screen (no subscription).
        call.video
            .negotiated(&call.media, "", remote_video::Phase::Direct)
            .await;
        call.start_keep_alive(None);
        let mut rx = rx;
        call.run(events, pushes, opts, sink, &mut rx, vec![]).await;
    });
    Box::new(LiveHandle(tx))
}

/// The first `{contentType, blob}` object anywhere in `v` (a join answer may carry the media
/// answer inline). As in `ironteams-cli`.
pub(super) fn find_blob(v: &serde_json::Value) -> Option<SdpBlob> {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            if let (Some(ct), Some(b)) = (
                m.get("contentType").and_then(Value::as_str),
                m.get("blob").and_then(Value::as_str),
            ) && !b.is_empty()
            {
                return Some(SdpBlob {
                    content_type: ct.to_owned(),
                    blob: b.to_owned(),
                });
            }
            m.values().find_map(find_blob)
        }
        Value::Array(a) => a.iter().find_map(find_blob),
        _ => None,
    }
}
