use super::*;

/// A calling-trouter endpoint id: stable per install and distinct from the chat trouter's (as
/// `ironteams-cli`'s `calling_epid`).
pub(super) fn calling_epid(chat_epid: &str) -> String {
    let tail = chat_epid.get(8..).unwrap_or(chat_epid);
    format!("ca11ca11{tail}")
}

/// The calling trouter and the conversation service client, shared by every call of the app.
pub struct Hub {
    pub(super) client: Client,
    pub(super) trouter: Mutex<Option<NgcTrouter>>,
    pub(super) ngc: Mutex<Option<NgcClient>>,
    /// Where each live call's callbacks go, by its session id (the shown call and the one on
    /// hold).
    pub(super) sessions: Mutex<HashMap<String, mpsc::UnboundedSender<CallPush>>>,
    pub(super) invitations: Mutex<Vec<mpsc::UnboundedSender<Box<CallInvitation>>>>,
    pub(super) unmatched: Mutex<Vec<mpsc::UnboundedSender<CallPush>>>,
    /// Whether the media servers refused a meeting's video lines in this session.
    pub(super) video: fallback::Memory,
}

impl Hub {
    pub fn new(client: Client) -> Arc<Hub> {
        Arc::new(Hub {
            client,
            trouter: Mutex::new(None),
            ngc: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            invitations: Mutex::new(Vec::new()),
            unmatched: Mutex::new(Vec::new()),
            video: fallback::Memory::default(),
        })
    }

    /// Starts the calling trouter (once) and discovers the conversation service. Needs a tokio
    /// runtime.
    pub fn start(self: &Arc<Self>) {
        let mut slot = self.trouter.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let (trouter, mut rx) =
            NgcTrouter::spawn(self.client.clone(), calling_epid(self.client.epid()));
        *slot = Some(trouter);
        drop(slot);
        let hub = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let Some(h) = hub.upgrade() else { return };
                match ev {
                    NgcEvent::Push(p) => h.route(p),
                    NgcEvent::Connected { .. } => tracing::debug!("calling trouter registered"),
                    // A reconnect changes the surl, which a running call's links were built on.
                    NgcEvent::Disconnected(r) => tracing::warn!(%r, "calling trouter disconnected"),
                }
            }
        });
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(e) = hub.ngc_client().await {
                tracing::warn!(%e, "calling endpoints not discovered");
            }
        });
    }

    pub fn ngc(&self) -> Option<NgcClient> {
        self.ngc.lock().unwrap().clone()
    }

    pub async fn ngc_client(&self) -> anyhow::Result<NgcClient> {
        if let Some(n) = self.ngc() {
            return Ok(n);
        }
        let mut n = NgcClient::discover(self.client.clone())
            .await
            .context("calling endpoints")?;
        if dump_dir().is_some() {
            n = n.with_observer(Arc::new(dump_exchange));
        }
        *self.ngc.lock().unwrap() = Some(n.clone());
        Ok(n)
    }

    pub fn surl(&self) -> Option<String> {
        self.trouter.lock().unwrap().as_ref()?.surl()
    }

    pub(super) async fn connected_surl(self: &Arc<Self>) -> Option<String> {
        self.start();
        let mut w = self.trouter.lock().unwrap().as_ref()?.surl_watch();
        tokio::time::timeout(Duration::from_secs(30), w.wait_for(Option::is_some))
            .await
            .ok()?
            .ok()?
            .clone()
    }

    /// Sends one push where it belongs.
    pub fn route(&self, p: CallPush) {
        dump_push(&p);
        match classify(&p.url, &p.body) {
            NgcPush::Invitation(inv) => self
                .invitations
                .lock()
                .unwrap()
                .retain(|tx| tx.send(inv.clone()).is_ok()),
            NgcPush::Call { session_id, .. } => {
                let call = self.sessions.lock().unwrap().get(&session_id).cloned();
                match call {
                    Some(tx) => {
                        let _ = tx.send(p);
                    }
                    None => self.unmatched(p),
                }
            }
            NgcPush::Other { .. } => self.unmatched(p),
        }
    }

    pub(super) fn unmatched(&self, p: CallPush) {
        self.unmatched
            .lock()
            .unwrap()
            .retain(|tx| tx.send(p.clone()).is_ok());
    }

    pub fn subscribe_invitations(&self) -> mpsc::UnboundedReceiver<Box<CallInvitation>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.invitations.lock().unwrap().push(tx);
        rx
    }

    pub fn subscribe_unmatched(&self) -> mpsc::UnboundedReceiver<CallPush> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.unmatched.lock().unwrap().push(tx);
        rx
    }

    /// Routes the callbacks of `session_id` to the returned receiver from now on, next to every
    /// other live call's.
    pub(super) fn add_session(&self, session_id: &str) -> mpsc::UnboundedReceiver<CallPush> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), tx);
        rx
    }

    pub(super) fn remove_session(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
    }

    pub(super) async fn ice_servers(&self, region: &str) -> Vec<IceServer> {
        match calling::relay::resolve_ice_servers(&self.client, region, None).await {
            Ok((ice, _)) => ice,
            Err(e) => {
                tracing::warn!(%e, "TURN unavailable; host candidates only");
                vec![]
            }
        }
    }

    /// The meeting a call window target names.
    pub(super) async fn resolve(&self, target: JoinTarget) -> anyhow::Result<MeetingTarget> {
        match target {
            JoinTarget::Link(url) => {
                meeting_from_join_url(&url).context("This isn't a Teams meeting link.")
            }
            JoinTarget::Code { id, passcode } => ::calendar::join_by_id_url(&id, &passcode)
                .and_then(|u| meeting_from_join_url(&u))
                .context("That doesn't look like a meeting ID."),
            JoinTarget::Chat(thread) => {
                let v = self
                    .client
                    .json("csa", reqwest::Method::GET, CSA_ME, None)
                    .await?;
                v["chats"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| c["id"].as_str() == Some(thread.as_str()))
                    .and_then(meeting_from_csa_chat)
                    .context("This chat has no meeting to join.")
            }
        }
    }
}
