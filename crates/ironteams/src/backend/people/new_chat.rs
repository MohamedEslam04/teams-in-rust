use super::*;

impl Engine {
    // ---- 6.2 New chat --------------------------------------------------------

    pub(super) fn push_new_chat(self: &Arc<Self>, open: bool) {
        let groups = self.groups();
        let (to, results, hint) = {
            let p = self.people.lock().unwrap();
            let mris: Vec<String> = p.new_chat.to.iter().map(|x| x.mri.clone()).collect();
            let hint = match plan(&self.me, &mris, &groups) {
                Plan::Existing(_) => "A chat with these people already exists.".to_string(),
                _ => String::new(),
            };
            (p.new_chat.to.clone(), p.new_chat.results.clone(), hint)
        };
        let to = to.iter().map(|x| self.person_data(x)).collect();
        let results = results.iter().map(|x| self.person_data(x)).collect();
        self.ui.new_chat(open, to, results, hint);
    }

    /// The known group chats with their members.
    pub(super) fn groups(&self) -> Vec<(String, Vec<String>)> {
        let ids: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .conversations(store::ConvFilter::Chats, 1000)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.kind == ConvKind::Group)
            .map(|c| c.id)
            .collect();
        let p = self.people.lock().unwrap();
        ids.into_iter()
            .filter_map(|id| p.members.get(&id).cloned().map(|m| (id, m)))
            .collect()
    }

    pub(super) async fn new_chat_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
        let seq = {
            let mut p = self.people.lock().unwrap();
            p.new_chat_seq += 1;
            p.new_chat_seq
        };
        if q.trim().is_empty() {
            self.people.lock().unwrap().new_chat.results.clear();
            self.push_new_chat(true);
            return Ok(());
        }
        let found = self.client.search_people(&self.me, q.trim()).await?;
        {
            let mut p = self.people.lock().unwrap();
            if p.new_chat_seq != seq {
                return Ok(());
            }
            let picked: Vec<String> = p.new_chat.to.iter().map(|x| x.mri.clone()).collect();
            p.new_chat.results = found
                .into_iter()
                .filter(|f| !picked.contains(&f.mri))
                .collect();
        }
        self.push_new_chat(true);
        Ok(())
    }

    pub(super) fn new_chat_add(self: &Arc<Self>, mri: &str) {
        {
            let mut p = self.people.lock().unwrap();
            if let Some(f) = p.new_chat.results.iter().find(|r| r.mri == mri).cloned()
                && !p.new_chat.to.iter().any(|t| t.mri == mri)
            {
                p.new_chat.to.push(f);
            }
            p.new_chat.results.clear();
            p.new_chat_seq += 1;
        }
        self.push_new_chat(true);
    }

    /// The New chat search's sequence number (tests of the command order).
    #[cfg(test)]
    pub(in super::super) fn new_chat_seq_for_test(&self) -> u64 {
        self.people.lock().unwrap().new_chat_seq
    }

    /// Opens (creating it when needed) the chat with `others` and returns its id. A new group is
    /// created here, so this is only called when the first message is sent or Chat is pressed.
    pub(in super::super) async fn open_with(
        self: &Arc<Self>,
        others: &[String],
        group_name: &str,
        create: bool,
    ) -> anyhow::Result<String> {
        let id = match plan(&self.me, others, &self.groups()) {
            Plan::Notes => "48:notes".to_string(),
            Plan::Existing(id) => id,
            Plan::Direct(id) => {
                let other = others.iter().find(|m| **m != self.me).cloned();
                self.ensure_direct_row(&id, other.as_deref());
                id
            }
            Plan::CreateGroup(members) => {
                if !create {
                    anyhow::bail!("Start a group chat from New chat");
                }
                let topic = (!group_name.trim().is_empty()).then(|| group_name.trim());
                let id = self.client.create_group_chat(&members, topic).await?;
                self.resolve_names(&members).await;
                let title = topic.map(str::to_string).unwrap_or_else(|| {
                    members
                        .iter()
                        .take(4)
                        .map(|m| self.name_of(m))
                        .collect::<Vec<_>>()
                        .join(", ")
                });
                self.store
                    .lock()
                    .unwrap()
                    .upsert_conversation(&ConversationRow {
                        id: id.clone(),
                        kind: ConvKind::Group,
                        title,
                        team_id: None,
                        team_name: None,
                        last_activity_ms: now_ms(),
                        last_preview: String::new(),
                        unread: false,
                        favorite: false,
                        read_horizon_ms: 0,
                    })?;
                let mut all = members;
                all.push(self.me.clone());
                self.people.lock().unwrap().members.insert(id.clone(), all);
                id
            }
            Plan::Unsupported => anyhow::bail!("This person cannot be messaged from here"),
        };
        self.mark(|d| d.chats = true);
        // A brand new thread has no history yet; opening it must not fail the whole action.
        if let Err(err) = self.open(id.clone(), None).await {
            tracing::debug!(%err, "opening the new chat");
        }
        Ok(id)
    }

    /// A 1:1 chat that is not in the list yet gets a row, so it can be opened and shows up.
    pub(super) fn ensure_direct_row(&self, id: &str, other: Option<&str>) {
        let st = self.store.lock().unwrap();
        if st.conversation(id).ok().flatten().is_some() {
            return;
        }
        let title = other
            .and_then(|m| st.user(m).ok().flatten())
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "New chat".into());
        let _ = st.upsert_conversation(&ConversationRow {
            id: id.to_string(),
            kind: ConvKind::OneOnOne,
            title,
            team_id: None,
            team_name: None,
            last_activity_ms: now_ms(),
            last_preview: String::new(),
            unread: false,
            favorite: false,
            read_horizon_ms: 0,
        });
    }

    /// Sends `text` to the 1:1 chat with the person on the profile card, without leaving the
    /// chat that is open.
    pub(super) async fn quick_message(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
        let text = text.trim().to_string();
        let Some(mri) = self.people.lock().unwrap().card_mri.clone() else {
            return Ok(());
        };
        if text.is_empty() || mri == self.me {
            return Ok(());
        }
        let id = match plan(&self.me, std::slice::from_ref(&mri), &self.groups()) {
            Plan::Existing(id) => id,
            Plan::Direct(id) => {
                self.ensure_direct_row(&id, Some(&mri));
                id
            }
            _ => return Ok(()),
        };
        let item = self.new_outbox_item(
            id,
            OutboxKind::Text,
            render::text_to_teams_html(&text),
            None,
            None,
            None,
        );
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        self.mark(|d| d.chats = true);
        Ok(())
    }

    pub(super) async fn new_chat_send(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
        let (picked, name) = {
            let p = self.people.lock().unwrap();
            (
                p.new_chat
                    .to
                    .iter()
                    .map(|x| x.mri.clone())
                    .collect::<Vec<_>>(),
                p.new_chat.group_name.clone(),
            )
        };
        let text = text.trim().to_string();
        if picked.is_empty() || text.is_empty() {
            return Ok(());
        }
        // The people's names are needed for the title of a chat that is not in the list yet.
        {
            let st = self.store.lock().unwrap();
            for x in &self.people.lock().unwrap().new_chat.to {
                if st.user(&x.mri).ok().flatten().is_none() {
                    let _ = st.upsert_user(&UserRow {
                        mri: x.mri.clone(),
                        display_name: x.display_name.clone(),
                        email: x.email.clone(),
                        avatar: None,
                        updated_ms: now_ms(),
                    });
                }
            }
        }
        let id = self.open_with(&picked, &name, true).await?;
        self.people.lock().unwrap().new_chat = NewChat::default();
        self.push_new_chat(false);
        let item = self.new_outbox_item(
            id,
            OutboxKind::Text,
            render::text_to_teams_html(&text),
            None,
            None,
            None,
        );
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        Ok(())
    }

    // ---- 6.3 group chat management -------------------------------------------

    pub(super) async fn rename(self: &Arc<Self>, name: String) -> anyhow::Result<()> {
        let name = name.trim().to_string();
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        if name.is_empty() || self.conv_kind(&conv) != Some(ConvKind::Group) {
            return Ok(());
        }
        self.client.rename_thread(&conv, &name).await?;
        {
            let st = self.store.lock().unwrap();
            if let Some(mut row) = st.conversation(&conv)? {
                row.title = name;
                st.upsert_conversation(&row)?;
            }
        }
        self.mark(|d| {
            d.chats = true;
            d.header = true;
        });
        Ok(())
    }

    /// Fills the participants popover with the open chat's members.
    pub(super) async fn push_members(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let members = self.ensure_members(&conv).await;
        self.resolve_names(&members).await;
        self.resolve_standing(&members).await;
        self.people.lock().unwrap().add_results.clear();
        let data = members.iter().map(|m| self.member_data(m)).collect();
        self.ui.members(data, vec![]);
        Ok(())
    }
}
