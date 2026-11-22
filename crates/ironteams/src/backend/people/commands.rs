use super::*;

impl Engine {
    // ---- plumbing -----------------------------------------------------------

    pub(in super::super) async fn people_cmd(self: &Arc<Self>, c: PeopleCmd) -> anyhow::Result<()> {
        match c {
            PeopleCmd::MentionQuery(q) => self.mention_query(q).await,
            PeopleCmd::MentionPick(id) => {
                self.mention_pick(&id);
                Ok(())
            }
            PeopleCmd::NewChatOpen => {
                let mut p = self.people.lock().unwrap();
                p.new_chat = NewChat::default();
                p.new_chat_seq += 1;
                drop(p);
                self.push_new_chat(true);
                Ok(())
            }
            PeopleCmd::NewChatClose => {
                let mut p = self.people.lock().unwrap();
                p.new_chat = NewChat::default();
                p.new_chat_seq += 1;
                drop(p);
                self.push_new_chat(false);
                Ok(())
            }
            PeopleCmd::NewChatQuery(q) => self.new_chat_query(q).await,
            PeopleCmd::NewChatAdd(id) => {
                self.new_chat_add(&id);
                Ok(())
            }
            PeopleCmd::NewChatRemove(id) => {
                self.people
                    .lock()
                    .unwrap()
                    .new_chat
                    .to
                    .retain(|p| p.mri != id);
                self.push_new_chat(true);
                Ok(())
            }
            PeopleCmd::NewChatName(n) => {
                self.people.lock().unwrap().new_chat.group_name = n;
                Ok(())
            }
            PeopleCmd::NewChatSend(text) => self.new_chat_send(text).await,
            PeopleCmd::Rename(name) => self.rename(name).await,
            PeopleCmd::ParticipantsOpen => self.push_members().await,
            PeopleCmd::AddQuery(q) => self.add_query(q).await,
            PeopleCmd::AddPick(mri) => self.add_pick(mri).await,
            PeopleCmd::AddHistory(on) => {
                self.people.lock().unwrap().add_history = on;
                Ok(())
            }
            PeopleCmd::Leave(id) => self.leave(&id).await,
            PeopleCmd::Profile(mri) => self.profile(mri).await,
            PeopleCmd::ProfileCopyEmail => {
                let email = self.people.lock().unwrap().card_email.clone();
                if let Some(email) = email {
                    arboard::Clipboard::new()?.set_text(email)?;
                    self.ui.banner("Email copied".into());
                }
                Ok(())
            }
            PeopleCmd::ProfileChat(mri) => {
                self.ui.card(false, CardData::default());
                let id = self.open_with(&[mri], "", false).await?;
                tracing::debug!(%id, "opened chat from the profile card");
                Ok(())
            }
            PeopleCmd::ProfileMessage(text) => self.quick_message(text).await,
            PeopleCmd::ProfileModal(mri) => self.profile_in(mri, true).await,
            PeopleCmd::OrgOpen(mri) => self.org_open(mri).await,
            PeopleCmd::OrgFocus(mri) => self.org_focus(mri).await,
            PeopleCmd::OrgBack => self.org_step(-1).await,
            PeopleCmd::OrgForward => self.org_step(1).await,
            PeopleCmd::OrgHome => self.org_step(0).await,
            PeopleCmd::PinJump(id) => self.jump_to(&id).await,
            PeopleCmd::PinUnpin(id) => self.unpin(&id).await,
            PeopleCmd::Receipts(on) => {
                self.set_receipts(on);
                Ok(())
            }
        }
    }

    /// Called when a chat is opened: drops what belonged to the previous one and starts loading
    /// members, pins and read receipts of the new one.
    pub(in super::super) fn on_chat_opened(self: &Arc<Self>, conv: &str) {
        {
            let mut p = self.people.lock().unwrap();
            p.entries.clear();
            p.candidates.clear();
            p.add_results.clear();
        }
        self.ui.card(false, CardData::default());
        self.ui.close_participants();
        self.ui.members(vec![], vec![]);
        self.ui.pinned(
            self.people
                .lock()
                .unwrap()
                .pins
                .get(conv)
                .cloned()
                .unwrap_or_default(),
        );
        let e = self.clone();
        let conv = conv.to_string();
        // Members, tabs and pins do not depend on each other: read together, not one after the
        // other (a channel's tabs waited for its members before).
        tokio::spawn(async move {
            tokio::join!(
                async {
                    let _ = e.ensure_members(&conv).await;
                    e.push_group_header(&conv);
                    e.refresh_horizons(&conv).await;
                },
                e.load_channel_tabs(&conv),
                e.refresh_pins(&conv),
            );
        });
    }

    /// Remembers the members the aggregator snapshot lists for each chat.
    pub(in super::super) fn remember_members(&self, chats: &[teams_proto::csa::ChatMeta]) {
        let mut p = self.people.lock().unwrap();
        for c in chats.iter().filter(|c| !c.members.is_empty()) {
            p.members.insert(c.id.clone(), c.members.clone());
        }
    }

    /// Background work of the people pieces: the receipt setting and the receipt poll.
    pub(in super::super) fn start_people(self: &Arc<Self>) {
        self.ui.receipts_on(self.receipts_enabled());
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            if let Ok(on) = e.client.read_receipts_enabled().await {
                e.people.lock().unwrap().receipts_server = Some(on);
                e.ui.receipts_on(e.receipts_enabled());
                e.mark(|d| d.messages = true);
            }
            loop {
                // Paused while the window is out of sight; shown again, it reads at once.
                e.visible_tick(RECEIPT_POLL).await;
                let cur = e.view.lock().unwrap().current.clone();
                if let Some(c) = cur {
                    e.refresh_horizons(&c).await;
                }
            }
        });
    }

    pub(super) fn conv_kind(&self, conv: &str) -> Option<ConvKind> {
        self.store
            .lock()
            .unwrap()
            .conversation(conv)
            .ok()
            .flatten()
            .map(|c| c.kind)
    }

    pub(super) fn current_chat(&self) -> Option<String> {
        self.view.lock().unwrap().current.clone()
    }

    pub(super) fn name_of(&self, mri: &str) -> String {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Someone".into())
    }

    /// Fetches and stores the profiles of members we have no name for yet.
    pub(super) async fn resolve_names(&self, mris: &[String]) {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let missing = self
            .store
            .lock()
            .unwrap()
            .users_missing_profile(mris)
            .unwrap_or_default();
        if missing.is_empty() {
            return;
        }
        if let Ok(profiles) = self.client.profiles(&missing).await {
            let st = self.store.lock().unwrap();
            for p in profiles {
                let _ = st.upsert_user(&UserRow {
                    avatar: st.user(&p.mri).ok().flatten().and_then(|u| u.avatar),
                    mri: p.mri,
                    display_name: p.display_name,
                    email: p.email,
                    updated_ms: now_ms(),
                });
            }
        }
    }

    pub(super) fn person_data(self: &Arc<Self>, p: &Person) -> PersonData {
        PersonData {
            id: p.mri.clone(),
            name: p.display_name.clone(),
            subtitle: p
                .job_title
                .clone()
                .or_else(|| p.email.clone())
                .unwrap_or_default(),
            avatar: self.avatar(&p.mri),
            kind: "person",
            presence: super::super::people_presence::want(&p.mri),
        }
    }

    pub(super) fn member_data(self: &Arc<Self>, mri: &str) -> PersonData {
        let st = self.store.lock().unwrap();
        let user = st.user(mri).ok().flatten();
        drop(st);
        PersonData {
            id: mri.to_string(),
            name: if mri == self.me {
                format!("{} (You)", self.my_name)
            } else {
                let name = user
                    .as_ref()
                    .map(|u| u.display_name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Someone".into());
                tagged(&name, self.outsider_of(mri))
            },
            subtitle: user.and_then(|u| u.email).unwrap_or_default(),
            avatar: self.avatar(mri),
            kind: "person",
            presence: super::super::people_presence::want(mri),
        }
    }

    /// The members of a chat, fetching a group's roster when it is not known yet.
    pub(super) async fn ensure_members(&self, conv: &str) -> Vec<String> {
        if let Some(m) = self.members_known(conv) {
            return m;
        }
        if self.conv_kind(conv) == Some(ConvKind::Group)
            && !self.offline.load(std::sync::atomic::Ordering::Relaxed)
            && let Ok((members, _)) = self.client.thread_members(conv).await
            && !members.is_empty()
        {
            self.people
                .lock()
                .unwrap()
                .members
                .insert(conv.to_string(), members.clone());
            return members;
        }
        Vec::new()
    }

    pub(in super::super) fn members_known(&self, conv: &str) -> Option<Vec<String>> {
        if self.conv_kind(conv) == Some(ConvKind::OneOnOne) {
            return Some(one_on_one_members(conv));
        }
        self.people.lock().unwrap().members.get(conv).cloned()
    }

    /// Shows or hides the rename pencil and participants button for the open chat.
    pub(in super::super) fn push_group_header(&self, conv: &str) {
        let group = self.conv_kind(conv) == Some(ConvKind::Group);
        let count = self.members_known(conv).map_or(0, |m| m.len());
        self.ui.group_header(group, count);
    }
}
