//! The NGC conversation service (`epconv`): place a call, join a meeting, take or decline an
//! incoming call, and every later step by POSTing to a link the service handed back.
//!
//! Request and response bodies are pure functions in [`bodies`]; this module only sends them.
//! Every request carries exactly one credential, the IC3 bearer, and goes through
//! [`endpoints::routed_url`], which refuses hosts outside Microsoft's calling plane.

pub mod bodies;
pub mod call_ops;
pub mod callbacks;
pub mod content_sharing;
pub mod controls;
pub mod endpoints;
pub mod lobby;
pub mod meeting;
pub mod recorder;
pub mod roster;
pub mod video;

pub use bodies::{Attached, Conversation, MeetingInfo};
pub use call_ops::TransferOutcome;
pub use callbacks::CallbackLinks;
pub use controls::{Controls, HandReceipt};
pub use endpoints::CallingEndpoints;
pub use meeting::MeetingTarget;
pub use roster::Roster;

use crate::error::{Error, Result};
use crate::invitation;
use crate::types::{CallInvitation, Links, Modality, Participant, SdpBlob};
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;
use teams_proto::Client;
use teams_proto::broker::Audience;

/// Our endpoint in one call, as `participants.from` / `sender` / `acceptedBy` name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    /// Our MRI (`8:orgid:<oid>`).
    pub mri: String,
    pub display_name: String,
    /// A fresh id per call; also the session id of the call's callback links.
    pub endpoint_id: String,
    pub participant_id: String,
    pub language_id: String,
}

impl LocalEndpoint {
    pub fn wire(&self) -> Value {
        json!({
            "id": self.mri,
            "displayName": self.display_name,
            "endpointId": self.endpoint_id,
            "participantId": self.participant_id,
            "languageId": self.language_id,
        })
    }
}

/// The local state of one call: who we are in it, the callback links we publish for it, the
/// chain id every request of the call shares, and the id of our media leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSession {
    pub me: LocalEndpoint,
    pub callbacks: CallbackLinks,
    pub chain_id: String,
    pub media_leg_id: String,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A media leg id shaped like the web client's: 32 upper-case hex digits.
fn new_media_leg_id() -> String {
    uuid::Uuid::new_v4().simple().to_string().to_uppercase()
}

impl CallSession {
    /// A session for a call we place or a meeting we join. `surl` is the calling trouter's.
    pub fn new(surl: &str, mri: &str, display_name: &str) -> Self {
        let endpoint_id = new_id();
        CallSession {
            callbacks: CallbackLinks::new(surl, &endpoint_id),
            me: LocalEndpoint {
                mri: mri.to_owned(),
                display_name: display_name.to_owned(),
                endpoint_id,
                participant_id: new_id(),
                language_id: "en-US".into(),
            },
            chain_id: new_id(),
            media_leg_id: new_media_leg_id(),
        }
    }

    /// A session to answer `inv`: we take the participant id the invitation gave us, the call id
    /// as chain id, and the caller's media leg id.
    pub fn for_invitation(surl: &str, mri: &str, display_name: &str, inv: &CallInvitation) -> Self {
        let mut s = Self::new(surl, mri, display_name);
        if let Some(p) = invitation::own_participant_id(inv) {
            s.me.participant_id = p.to_owned();
        }
        s.chain_id = inv.call_id.clone();
        if let Some(leg) = invitation::media_leg_id(inv) {
            s.media_leg_id = leg.to_owned();
        }
        s
    }
}

/// A response of the conversation service.
#[derive(Debug, Clone)]
pub struct NgcResponse {
    pub status: u16,
    /// The `Location` header (some answers name the conversation controller only there).
    pub location: Option<String>,
    /// The JSON body, `Null` when empty.
    pub body: Value,
}

/// One request to the conversation service and what came back, for `--dump` /
/// `IRONTEAMS_CALL_DUMP`. The bodies hold real ids and SDPs with addresses: keep them local.
#[derive(Debug)]
pub struct Exchange<'a> {
    pub op: &'a str,
    pub method: &'a str,
    pub url: &'a str,
    pub request: Option<&'a Value>,
    /// The HTTP status; 0 when the request never got an answer.
    pub status: u16,
    /// The answer body, or the transport error.
    pub response: &'a str,
}

/// Sees every [`Exchange`] of an [`NgcClient`].
pub type ExchangeObserver = Arc<dyn Fn(&Exchange<'_>) + Send + Sync>;

/// Client for the NGC conversation service, on top of a signed-in [`teams_proto::Client`].
#[derive(Clone)]
pub struct NgcClient {
    client: Client,
    endpoints: CallingEndpoints,
    observer: Option<ExchangeObserver>,
}

impl NgcClient {
    pub fn new(client: Client, endpoints: CallingEndpoints) -> Self {
        NgcClient {
            client,
            endpoints,
            observer: None,
        }
    }

    /// Reports every request and its answer to `observer` (protocol debugging).
    pub fn with_observer(mut self, observer: ExchangeObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    fn observe(
        &self,
        op: &str,
        method: &Method,
        url: &str,
        request: Option<&Value>,
        status: u16,
        response: &str,
    ) {
        if let Some(o) = &self.observer {
            o(&Exchange {
                op,
                method: method.as_str(),
                url,
                request,
                status,
                response,
            });
        }
    }

    /// Reads the calling endpoints from authz (teams-proto keeps only the chat ones), then builds
    /// the client.
    pub async fn discover(client: Client) -> Result<Self> {
        let spaces = client.token_for(&Audience::Spaces).await?;
        let resp = client
            .http()
            .post(endpoints::AUTHZ_URL)
            .header("User-Agent", teams_proto::client_info::USER_AGENT)
            .header("Authorization", format!("Bearer {spaces}"))
            .header("Accept", "application/json; ver=1.0")
            .header("Content-Length", "0")
            .send()
            .await
            .map_err(teams_proto::Error::from)?;
        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(teams_proto::Error::from)?;
        if !(200..300).contains(&status) {
            return Err(status_error("authz", status, &body));
        }
        Ok(Self::new(client, endpoints::parse_authz(&body)?))
    }

    pub fn endpoints(&self) -> &CallingEndpoints {
        &self.endpoints
    }

    /// Sends one call-control request to `url` (routed through the flightproxy when it names a
    /// backend). The IC3 token is minted here and sent by hand: `Client::request_as` only attaches
    /// a bearer for the audience's own host.
    pub async fn send(
        &self,
        op: &'static str,
        s: &CallSession,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<NgcResponse> {
        self.send_with(op, s, method, url, body, &[]).await
    }

    /// [`Self::send`] with `extra` headers (a content-sharing session's correlation id).
    pub async fn send_with(
        &self,
        op: &'static str,
        s: &CallSession,
        method: Method,
        url: &str,
        body: Option<&Value>,
        extra: &[(&'static str, &str)],
    ) -> Result<NgcResponse> {
        let target = endpoints::routed_url(url)?;
        let ic3 = self.client.token_for(&Audience::Ic3).await?;
        let message_id = new_id();
        let mut req = self.client.http().request(method.clone(), &target);
        for (k, v) in endpoints::call_headers(&self.endpoints, &s.chain_id, &message_id, &ic3) {
            req = req.header(k, v);
        }
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        req = match body {
            Some(b) => req.body(serde_json::to_vec(b)?),
            None => req.header("Content-Length", "0"),
        };
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                self.observe(op, &method, &target, body, 0, &e.to_string());
                return Err(teams_proto::Error::from(e).into());
            }
        };
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = resp.text().await.map_err(teams_proto::Error::from)?;
        self.observe(op, &method, &target, body, status, &text);
        // The media negotiation's requests are what a live log of hold, resume or a media restart
        // needs; the rest stay at debug.
        if op.starts_with("media_") {
            tracing::info!(op, %method, status, "ngc request");
        } else {
            tracing::debug!(op, %method, status, "ngc request");
        }
        if !(200..300).contains(&status) {
            return Err(status_error(op, status, &text));
        }
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        };
        Ok(NgcResponse {
            status,
            location,
            body,
        })
    }

    async fn post(
        &self,
        op: &'static str,
        s: &CallSession,
        url: &str,
        body: &Value,
    ) -> Result<NgcResponse> {
        self.send(op, s, Method::POST, url, Some(body)).await
    }

    fn link<'a>(links: &'a Links, name: &str) -> Result<&'a str> {
        links
            .get(name)
            .ok_or_else(|| Error::Protocol(format!("no '{name}' link")))
    }

    /// Places a call to `to` (one person: a direct 1:1 call). The answer comes later on the
    /// `call/acceptance` callback (see [`crate::invitation::CallEvent::Accepted`]).
    pub async fn place_call(
        &self,
        s: &CallSession,
        to: &[Participant],
        thread_id: Option<&str>,
        offer: &SdpBlob,
        modalities: &[Modality],
    ) -> Result<Conversation> {
        let body = bodies::place_call_body(s, to, thread_id, offer, modalities);
        let r = self
            .post(
                "place_call",
                s,
                &self.endpoints.conversation_service_url,
                &body,
            )
            .await?;
        bodies::parse_conversation(&r.body, r.location.as_deref())
    }

    /// Joins a meeting by thread + `meetingInfo`, in one POST. The media answer comes later on the
    /// `call/mediaAnswer` or `call/acceptance` callback.
    pub async fn join_meeting(
        &self,
        s: &CallSession,
        meeting: &MeetingInfo,
        offer: &SdpBlob,
        modalities: &[Modality],
    ) -> Result<Conversation> {
        let body = bodies::join_meeting_body(s, meeting, offer, modalities);
        let r = self
            .post(
                "join_meeting",
                s,
                &self.endpoints.conversation_service_url,
                &body,
            )
            .await?;
        bodies::parse_conversation(&r.body, r.location.as_deref())
    }

    /// Joins a meeting by code (mautrix-teams' two steps): open the conversation by
    /// `meetingData`, then post the media to its controller with the `meetingData` the answer
    /// returned (it carries the short passcode the media step needs). Works for a meeting outside
    /// our tenant, such as a meet-now hosted by a personal account.
    pub async fn join_meeting_by_code(
        &self,
        s: &CallSession,
        meeting_data: &Value,
        offer: &SdpBlob,
        modalities: &[Modality],
    ) -> Result<Conversation> {
        let open = bodies::code_join_body(s, meeting_data, None);
        let r = self
            .post(
                "join_by_code",
                s,
                &self.endpoints.conversation_service_url,
                &open,
            )
            .await?;
        let opened = bodies::parse_conversation(&r.body, r.location.as_deref())?;
        let data = opened
            .raw
            .get("meetingData")
            .filter(|d| d.as_object().is_some_and(|m| !m.is_empty()))
            .unwrap_or(meeting_data);
        let media = bodies::code_join_body(s, data, Some((offer, modalities)));
        let r = self
            .post("join_by_code_media", s, &opened.controller, &media)
            .await?;
        Ok(merge(opened, &r))
    }

    /// Joins a running conversation by its URL (`activeMeetup.conversationUrl`), posting the media
    /// join to it the way a conversation controller takes it. UNVERIFIED: no reference joins an
    /// active meetup by its URL; prefer [`Self::join_meeting_by_code`] when a code is known.
    pub async fn join_conversation_url(
        &self,
        s: &CallSession,
        conversation_url: &str,
        meeting: &MeetingTarget,
        offer: &SdpBlob,
        modalities: &[Modality],
    ) -> Result<Conversation> {
        let body = match (meeting.meeting_data(), meeting.meeting_info()) {
            (Some(data), _) => bodies::code_join_body(s, &data, Some((offer, modalities))),
            (None, Some(info)) => bodies::join_meeting_body(s, &info, offer, modalities),
            (None, None) => {
                return Err(Error::Protocol(
                    "meeting has neither a code nor a thread context".into(),
                ));
            }
        };
        let r = self
            .post("join_conversation_url", s, conversation_url, &body)
            .await?;
        bodies::parse_conversation(&r.body, r.location.as_deref()).or_else(|_| {
            Ok(Conversation {
                controller: conversation_url.to_owned(),
                links: bodies::parse_links(&r.body["links"]),
                raw: r.body,
            })
        })
    }

    /// Joins `meeting` the best-supported way: by code when it has one (works across tenants),
    /// else by thread + `meetingInfo`.
    pub async fn join(
        &self,
        s: &CallSession,
        meeting: &MeetingTarget,
        offer: &SdpBlob,
        modalities: &[Modality],
    ) -> Result<Conversation> {
        if let Some(data) = meeting.meeting_data() {
            return self.join_meeting_by_code(s, &data, offer, modalities).await;
        }
        let info = meeting.meeting_info().ok_or_else(|| {
            Error::Protocol("meeting has neither a code nor a thread context".into())
        })?;
        self.join_meeting(s, &info, offer, modalities).await
    }

    /// Claims a forked incoming call (`attach`) and joins its conversation. Returns the leg's links
    /// (`acceptance`, `callLeg`, `mediaAnswer`, `progress`, ...).
    pub async fn attach(&self, s: &CallSession, inv: &CallInvitation) -> Result<Links> {
        self.attach_with(s, inv, false).await.map(|a| a.links)
    }

    /// [`Self::attach`], asking for the caller's offer when `require_media` (a ring that came
    /// without `mediaContent`). The offer, if the service sends it, is in [`Attached::offer`].
    pub async fn attach_with(
        &self,
        s: &CallSession,
        inv: &CallInvitation,
        require_media: bool,
    ) -> Result<Attached> {
        let url = Self::link(&inv.links, "attach")?;
        let body = bodies::attach_body(s, invitation::conversation_controller(inv), require_media);
        let r = self.post("attach", s, url, &body).await?;
        bodies::parse_attach(&r.body)
    }

    /// [`Self::attach`] with OUR offer in it (see [`bodies::attach_offer_body`]; UNVERIFIED).
    pub async fn attach_with_offer(
        &self,
        s: &CallSession,
        inv: &CallInvitation,
        offer: &SdpBlob,
    ) -> Result<Attached> {
        let url = Self::link(&inv.links, "attach")?;
        let body = bodies::attach_offer_body(s, invitation::conversation_controller(inv), offer);
        let r = self.post("attach", s, url, &body).await?;
        bodies::parse_attach(&r.body)
    }

    /// Tells the caller we ring (`progress`).
    pub async fn send_ringing(&self, s: &CallSession, links: &Links) -> Result<()> {
        let url = Self::link(links, "progress")?;
        self.post("progress", s, url, &bodies::progress_body(s))
            .await
            .map(drop)
    }

    /// Accepts a call on its `acceptance` link with our SDP answer. Returns the established
    /// leg's links (`callLeg`, `mediaRenegotiation`, ...).
    pub async fn accept(
        &self,
        s: &CallSession,
        links: &Links,
        modalities: &[Modality],
        answer: &SdpBlob,
    ) -> Result<Links> {
        let url = Self::link(links, "acceptance")?;
        let r = self
            .post(
                "accept",
                s,
                url,
                &bodies::acceptance_body(s, modalities, answer),
            )
            .await?;
        Ok(bodies::parse_acceptance_links(&r.body))
    }

    /// Accepts a call that came without an offer, with OUR offer (the offerer path, UNVERIFIED
    /// live; see [`bodies::offer_acceptance_body`]). Returns the leg's links and the caller's
    /// answer when the acknowledgement carries it inline; otherwise it arrives on our
    /// `call/mediaAnswer/` callback.
    pub async fn accept_with_offer(
        &self,
        s: &CallSession,
        links: &Links,
        modalities: &[Modality],
        offer: &SdpBlob,
    ) -> Result<(Links, Option<SdpBlob>)> {
        let url = Self::link(links, "acceptance")?;
        let r = self
            .post(
                "accept_with_offer",
                s,
                url,
                &bodies::offer_acceptance_body(s, modalities, offer),
            )
            .await?;
        Ok((
            bodies::parse_acceptance_links(&r.body),
            bodies::parse_inline_answer(&r.body),
        ))
    }

    /// Accepts after our offer went out on its own (`mediaContent: null`; UNVERIFIED). Returns
    /// the leg's links and any answer the acknowledgement carries inline.
    pub async fn accept_without_media(
        &self,
        s: &CallSession,
        links: &Links,
        modalities: &[Modality],
    ) -> Result<(Links, Option<SdpBlob>)> {
        let url = Self::link(links, "acceptance")?;
        let r = self
            .post(
                "accept_without_media",
                s,
                url,
                &bodies::mediafree_acceptance_body(s, modalities),
            )
            .await?;
        Ok((
            bodies::parse_acceptance_links(&r.body),
            bodies::parse_inline_answer(&r.body),
        ))
    }

    /// Posts OUR offer as a `mediaNegotiation` to `url` (the attach answer's `newOffer`; see
    /// [`bodies::negotiation_body`]). Returns the answer when it comes inline; otherwise it
    /// arrives on our `call/mediaAnswer/` callback.
    pub async fn send_offer(
        &self,
        s: &CallSession,
        url: &str,
        modalities: &[Modality],
        offer: &SdpBlob,
    ) -> Result<Option<SdpBlob>> {
        let body = bodies::negotiation_body(s, modalities, offer);
        let r = self.post("new_offer", s, url, &body).await?;
        Ok(bodies::parse_inline_answer(&r.body))
    }

    /// The whole incoming path: attach when the invitation has no `acceptance` of its own (a
    /// forked invite), report ringing, then accept. Returns the established leg's links merged
    /// over the attach links.
    pub async fn accept_invitation(
        &self,
        s: &CallSession,
        inv: &CallInvitation,
        modalities: &[Modality],
        answer: &SdpBlob,
    ) -> Result<Links> {
        let mut links = if inv.links.get("acceptance").is_some() {
            inv.links.clone()
        } else {
            self.attach(s, inv).await?
        };
        if links.get("progress").is_some()
            && let Err(e) = self.send_ringing(s, &links).await
        {
            tracing::warn!(%e, "reporting the call as ringing failed");
        }
        let accepted = self.accept(s, &links, modalities, answer).await?;
        links.0.extend(accepted.0);
        Ok(links)
    }

    /// Declines a ringing call (603) on its `reject`/`rejection` link, else its `callLeg`.
    pub async fn reject(&self, s: &CallSession, links: &Links) -> Result<()> {
        let (method, url, body) = bodies::reject_request(links)
            .ok_or_else(|| Error::Protocol("no link to decline the call on".into()))?;
        self.send("reject", s, method, &url, Some(&body))
            .await
            .map(drop)
    }

    /// Sends our answer to a media offer on its `mediaAnswer` link; returns the call leg links its
    /// response names (none for most answers).
    pub async fn send_media_answer(
        &self,
        s: &CallSession,
        url: &str,
        modalities: &[Modality],
        answer: &SdpBlob,
    ) -> Result<Links> {
        let body = bodies::media_answer_body(s, modalities, answer);
        let r = self.post("media_answer", s, url, &body).await?;
        // After a media server's restart its answer may name the new leg's links (rusty-teams
        // absorbs them as the leg's); later re-offers go there.
        Ok(crate::invitation::nested_links(&r.body))
    }

    /// Leaves the call: the conversation's `leave` link when there is one, else ends the leg
    /// (`DELETE callLeg`, the only leg-shaped link an incoming 1:1 call hands out).
    pub async fn hang_up(&self, s: &CallSession, links: &Links) -> Result<()> {
        if let Some(url) = links.get("leave") {
            return self
                .post("leave", s, url, &bodies::leave_body(s))
                .await
                .map(drop);
        }
        let url = Self::link(links, "callLeg")?;
        self.send(
            "end_call_leg",
            s,
            Method::DELETE,
            url,
            Some(&bodies::call_leg_end_body()),
        )
        .await
        .map(drop)
    }

    /// Leaves via the conversation controller (`<controller>/leave`), for a conversation whose
    /// answer named no `leave` link (mautrix-teams does this).
    pub async fn leave_controller(&self, s: &CallSession, controller: &str) -> Result<()> {
        let url = insert_path(controller, "/leave");
        self.post("leave", s, &url, &bodies::leave_body(s))
            .await
            .map(drop)
    }

    /// Mutes or unmutes us as the service sees it (`updateEndpointState`; see [`controls`]).
    /// The caller stops or resumes sending microphone audio itself (sending
    /// [`crate::audio::OPUS_SILENCE`] in its place: a call sending no RTP is ended by the service).
    pub async fn set_muted(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        controls: &mut Controls,
        muted: bool,
    ) -> Result<()> {
        let (url, body) = controls.mute_request(s, links, controller, muted);
        self.post("update_endpoint_state", s, &url, &body)
            .await
            .map(drop)
    }

    /// Publishes our current mute state again (after joining, and after the lobby admits us).
    pub async fn repeat_muted(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        controls: &mut Controls,
    ) -> Result<()> {
        let (url, body) = controls.repeat_mute_request(s, links, controller);
        self.post("update_endpoint_state", s, &url, &body)
            .await
            .map(drop)
    }

    /// Raises our hand (`publishState`). Returns the service's receipt, also kept in `controls`.
    pub async fn raise_hand(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        controls: &mut Controls,
    ) -> Result<HandReceipt> {
        let (url, body) = controls.raise_hand_request(s, links, controller);
        let r = self.post("publish_state", s, &url, &body).await?;
        let receipt = controls::parse_hand_receipt(&r.body);
        controls.hand_raised(&receipt);
        Ok(receipt)
    }

    /// Lowers our hand (`removeState`): the one we raised, else `roster_hand` (the `stateId` the
    /// roster shows for us). Returns false when there was no hand to lower.
    pub async fn lower_hand(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        controls: &mut Controls,
        roster_hand: Option<&str>,
    ) -> Result<bool> {
        let Some((url, body)) = controls.lower_hand_request(s, links, controller, roster_hand)
        else {
            return Ok(false);
        };
        self.post("remove_state", s, &url, &body)
            .await
            .map(|_| true)
    }

    /// Sends a live reaction everyone in the meeting sees float up (`POST
    /// <conversation>/sendMessage`, as Teams web does; see [`controls`]). Each send is logged at
    /// info with the service's status, the URL's path (never its query or a token), the start of
    /// the answer and the ids `from` named, so a live run shows where it went, whether the service
    /// took it and as whom (the receiver's `receiveMessage` names the same ids); a refusal comes
    /// back as the error.
    pub async fn send_reaction(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        reaction: controls::Reaction,
    ) -> Result<()> {
        let (url, body) = controls::reaction_request(s, links, controller, reaction);
        let to = url.split('?').next().unwrap_or_default().to_owned();
        let r = self
            .post("send_message", s, &url, &body)
            .await
            .inspect_err(|e| {
                let controller_known = !controller.is_empty();
                tracing::info!(%to, controller_known, %e, "reaction refused");
            })?;
        let answer: String = r.body.to_string().chars().take(200).collect();
        tracing::info!(
            reaction = reaction.wire_name(),
            status = r.status,
            %to,
            %answer,
            participant = %s.me.participant_id,
            endpoint = %s.me.endpoint_id,
            "reaction sent"
        );
        Ok(())
    }

    /// Rings `mri` into the call (`addParticipant`; see [`call_ops`]). The outcome comes on the
    /// `conversation/addParticipantSuccess` or `addParticipantFailure` callback.
    pub async fn add_participant(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        mri: &str,
        display_name: &str,
    ) -> Result<()> {
        let (url, body) =
            call_ops::add_participant_request(s, links, controller, mri, display_name);
        self.post("add_participant", s, &url, &body).await.map(drop)
    }

    /// Lets `mri` in from the lobby (`admit`; see [`lobby`]). The outcome comes on the
    /// `conversation/admitParticipantSuccess` or `admitParticipantFailure` callback.
    pub async fn admit(
        &self,
        s: &CallSession,
        links: &Links,
        controller: &str,
        mri: &str,
        display_name: &str,
    ) -> Result<()> {
        let (url, body) = lobby::admit_request(s, links, controller, mri, display_name);
        self.post("admit", s, &url, &body).await.map(drop)
    }

    /// Re-offers our media on the same peer connection (hold and resume; see [`call_ops`]) on
    /// the leg's `mediaRenegotiation` link. Returns the answer when it comes inline; otherwise
    /// it arrives on our `call/mediaAnswer/` callback.
    pub async fn renegotiate(
        &self,
        s: &CallSession,
        links: &Links,
        modalities: &[Modality],
        offer: &SdpBlob,
    ) -> Result<Option<SdpBlob>> {
        let url = Self::link(links, "mediaRenegotiation")?;
        let body = bodies::renegotiation_body(s, modalities, offer, false);
        let r = self.post("media_renegotiation", s, url, &body).await?;
        Ok(bodies::parse_inline_answer(&r.body))
    }

    /// Acknowledges a media answer we applied (an empty POST on its `mediaAcknowledgement` link,
    /// as rusty-teams does).
    pub async fn acknowledge_media(&self, s: &CallSession, url: &str) -> Result<()> {
        self.send("media_acknowledgement", s, Method::POST, url, None)
            .await
            .map(drop)
    }

    /// Transfers the call to `target` (an MRI; blind, "Transfer now"). The outcome comes on
    /// `call/transferCompletion` ([`call_ops::parse_transfer_completion`]).
    pub async fn transfer(&self, s: &CallSession, links: &Links, target: &str) -> Result<()> {
        let (url, body) = call_ops::transfer_request(s, links, target)
            .ok_or_else(|| Error::Protocol("this call has no 'transfer' link".into()))?;
        self.post("transfer", s, &url, &body).await.map(drop)
    }

    /// One keep-alive (see [`bodies::keep_alive_request`]).
    /// Asks the meeting to make us the presenter of its content-sharing session (`POST
    /// <addModality>`, UNVERIFIED). The grant may be in the answer or come on a callback
    /// ([`content_sharing::SharingSession::read`] reads either).
    pub async fn start_content_sharing(
        &self,
        s: &CallSession,
        add_modality: &str,
        correlation_id: &str,
        identifier: &str,
    ) -> Result<NgcResponse> {
        let body = content_sharing::start_body(s, identifier);
        self.send_with(
            "contentSharingStart",
            s,
            Method::POST,
            add_modality,
            Some(&body),
            &[(content_sharing::CORRELATION_HEADER, correlation_id)],
        )
        .await
    }

    /// Gives the content-sharing session back on ITS `leave` link (never the call's).
    pub async fn leave_content_sharing(
        &self,
        s: &CallSession,
        session: &content_sharing::SharingSession,
    ) -> Result<()> {
        let url = session
            .leave()
            .ok_or_else(|| Error::Protocol("the sharing session has no 'leave' link".into()))?;
        let body = content_sharing::leave_body(s);
        self.post_content_sharing("contentSharingLeave", s, url, session, &body)
            .await?;
        Ok(())
    }

    /// One POST of a content-sharing session, with its correlation header.
    pub async fn post_content_sharing(
        &self,
        op: &'static str,
        s: &CallSession,
        url: &str,
        session: &content_sharing::SharingSession,
        body: &Value,
    ) -> Result<NgcResponse> {
        self.send_with(
            op,
            s,
            Method::POST,
            url,
            Some(body),
            &[(content_sharing::CORRELATION_HEADER, &session.correlation_id)],
        )
        .await
    }

    pub async fn keep_alive(&self, s: &CallSession, links: &Links) -> Result<()> {
        let (url, body) = bodies::keep_alive_request(links)
            .ok_or_else(|| Error::Protocol("no keep-alive link".into()))?;
        self.post("keep_alive", s, &url, &body).await.map(drop)
    }

    /// Keeps the call alive until a keep-alive fails; abort the task to stop it.
    pub async fn run_keep_alive(
        &self,
        s: &CallSession,
        links: &Links,
        interval_s: Option<u64>,
    ) -> Result<()> {
        let period = bodies::keep_alive_period(interval_s);
        loop {
            tokio::time::sleep(period).await;
            self.keep_alive(s, links).await?;
        }
    }
}

fn status_error(op: &'static str, status: u16, body: &str) -> Error {
    Error::Status {
        op,
        status,
        body: body.chars().take(400).collect(),
    }
}

/// `<base><suffix>?<query>`, as mautrix-teams addresses controller operations.
fn insert_path(url: &str, suffix: &str) -> String {
    match url.split_once('?') {
        Some((base, query)) => format!("{}{suffix}?{query}", base.trim_end_matches('/')),
        None => format!("{}{suffix}", url.trim_end_matches('/')),
    }
}

/// A join's second answer can name a new controller and more links.
fn merge(opened: Conversation, joined: &NgcResponse) -> Conversation {
    match bodies::parse_conversation(&joined.body, joined.location.as_deref()) {
        Ok(j) => {
            let mut links = opened.links;
            links.0.extend(j.links.0);
            Conversation {
                controller: j.controller,
                links,
                raw: j.raw,
            }
        }
        Err(_) => {
            let mut c = opened;
            c.links
                .0
                .extend(bodies::parse_links(&joined.body["links"]).0);
            c
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_operations_keep_the_query() {
        assert_eq!(
            insert_path("https://c/conv/1?i=3", "/leave"),
            "https://c/conv/1/leave?i=3"
        );
        assert_eq!(
            insert_path("https://c/conv/1/", "/leave"),
            "https://c/conv/1/leave"
        );
    }
}
