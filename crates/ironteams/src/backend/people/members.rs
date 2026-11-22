use super::*;

impl Engine {
    /// What a profile lookup said about `mri`, once it was asked.
    pub(in super::super) fn standing_of(&self, mri: &str) -> Option<Standing> {
        self.people.lock().unwrap().standing.get(mri).cloned()
    }

    /// Looks up, once per session, whether the organization people among `mris` are guests: the
    /// stored profile has no user type and a guest's display name and MRI look like a
    /// colleague's. Returns whether somebody was found to be a guest.
    pub(in super::super) async fn resolve_standing(&self, mris: &[String]) -> bool {
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let todo: Vec<String> = {
            let mut p = self.people.lock().unwrap();
            mris.iter()
                .filter(|m| m.starts_with("8:orgid:") && **m != self.me)
                .filter(|m| p.standing_asked.insert((*m).clone()))
                .cloned()
                .collect()
        };
        if todo.is_empty() {
            return false;
        }
        let Ok(found) = self.client.profiles(&todo).await else {
            return false;
        };
        let mut guest = false;
        let mut p = self.people.lock().unwrap();
        for f in &found {
            let s = Standing::from(f);
            guest |= s.guest;
            p.standing.insert(f.mri.clone(), s);
        }
        guest
    }

    /// Starts the lookup for the people of the chat list's 1:1 chats; the list and the header
    /// are drawn again when somebody turns out to be a guest.
    pub(in super::super) fn want_standing(self: &Arc<Self>, mris: Vec<String>) {
        let any_new = {
            let p = self.people.lock().unwrap();
            mris.iter()
                .any(|m| m.starts_with("8:orgid:") && !p.standing_asked.contains(m))
        };
        if !any_new || self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            if e.resolve_standing(&mris).await {
                e.mark(|d| {
                    d.chats = true;
                    d.header = true;
                });
            }
        });
    }

    pub(super) async fn add_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let seq = {
            let mut p = self.people.lock().unwrap();
            p.seq += 1;
            p.seq
        };
        let found = if q.trim().is_empty() {
            Vec::new()
        } else {
            self.client.search_people(&self.me, q.trim()).await?
        };
        let members = self.members_known(&conv).unwrap_or_default();
        let found: Vec<Person> = found
            .into_iter()
            .filter(|f| !members.contains(&f.mri))
            .collect();
        {
            let mut p = self.people.lock().unwrap();
            if p.seq != seq {
                return Ok(());
            }
            p.add_results = found.clone();
        }
        let data = members.iter().map(|m| self.member_data(m)).collect();
        let add = found.iter().map(|f| self.person_data(f)).collect();
        self.ui.members(data, add);
        Ok(())
    }

    pub(super) async fn add_pick(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let (person, history) = {
            let p = self.people.lock().unwrap();
            (
                p.add_results.iter().find(|r| r.mri == mri).cloned(),
                p.add_history,
            )
        };
        if self.conv_kind(&conv) == Some(ConvKind::OneOnOne) {
            return self.group_from_direct(&conv, person);
        }
        // Whole history: 0. No history: from now on.
        let from = if history { 0 } else { now_ms() };
        self.client.add_member(&conv, &mri, from).await?;
        if let Some(p) = &person {
            let st = self.store.lock().unwrap();
            if st.user(&p.mri).ok().flatten().is_none() {
                let _ = st.upsert_user(&UserRow {
                    mri: p.mri.clone(),
                    display_name: p.display_name.clone(),
                    email: p.email.clone(),
                    avatar: None,
                    updated_ms: now_ms(),
                });
            }
        }
        {
            let mut p = self.people.lock().unwrap();
            let members = p.members.entry(conv.clone()).or_default();
            if !members.contains(&mri) {
                members.push(mri.clone());
            }
            p.add_results.clear();
        }
        self.push_group_header(&conv);
        self.push_members().await?;
        self.ui.banner(format!(
            "Added {}",
            person.map_or_else(|| "a person".into(), |p| p.display_name)
        ));
        Ok(())
    }

    /// "Add people" in a 1:1 chat: a 1:1 chat takes no members, so, as in Teams, a group chat
    /// with the other person and the one picked is started instead. The New chat pane opens
    /// with both in To:; its first message creates the group (or opens one that exists).
    pub(super) fn group_from_direct(
        self: &Arc<Self>,
        conv: &str,
        picked: Option<Person>,
    ) -> anyhow::Result<()> {
        let mut to: Vec<Person> = one_on_one_members(conv)
            .into_iter()
            .filter(|m| *m != self.me)
            .map(|m| Person {
                display_name: self.name_of(&m),
                mri: m,
                email: None,
                job_title: None,
                department: None,
            })
            .collect();
        to.extend(picked);
        {
            let mut p = self.people.lock().unwrap();
            p.new_chat = NewChat {
                to,
                ..NewChat::default()
            };
            p.add_results.clear();
        }
        self.ui.close_participants();
        self.push_new_chat(true);
        Ok(())
    }

    /// Leaves a group chat and forgets it locally.
    pub(super) async fn leave(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
        self.ui.leave_confirm(String::new());
        if self.conv_kind(conv) != Some(ConvKind::Group) {
            return Ok(());
        }
        self.client.leave_thread(conv, &self.me).await?;
        self.store.lock().unwrap().delete_conversation(conv)?;
        self.people.lock().unwrap().members.remove(conv);
        let was_open = {
            let mut v = self.view.lock().unwrap();
            let open = v.current.as_deref() == Some(conv);
            if open {
                v.current = None;
                v.thread_root = None;
            }
            open
        };
        if was_open {
            self.ui.group_header(false, 0);
            self.ui.pinned(vec![]);
        }
        self.mark(|d| {
            d.chats = true;
            d.header = true;
            d.messages = true;
        });
        self.ui.banner("You left the chat".into());
        Ok(())
    }

    // ---- 6.5 profile card ----------------------------------------------------

    /// The card's photo: the stored one at the avatar size, else the cached avatar.
    pub(super) fn card_photo(self: &Arc<Self>, mri: &str) -> Option<crate::data::Pixels> {
        let stored = self
            .store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .and_then(|u| u.avatar);
        stored
            .and_then(|b| decode_image(&b, AVATAR_PX, AVATAR_PX))
            .or_else(|| self.avatar(mri))
    }

    /// Opens the profile card: at once with everything already known (the stored name, email and
    /// photo, the last presence, the details an earlier card read), then fills in the rest. The
    /// details and the presence are read together.
    pub(super) async fn profile(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
        self.profile_in(mri, false).await
    }

    /// [`Self::profile`] for the hover or click card (`modal` false), or for the full profile
    /// window (`modal` true: the card data is filled without opening the card).
    pub(in super::super) async fn profile_in(
        self: &Arc<Self>,
        mri: String,
        modal: bool,
    ) -> anyhow::Result<()> {
        let known = self.store.lock().unwrap().user(&mri)?;
        let name = if mri == self.me {
            self.my_name.clone()
        } else {
            known
                .as_ref()
                .map(|u| u.display_name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| self.org_known_name(&mri))
                .unwrap_or_else(|| "Someone".into())
        };
        let cached = self.people.lock().unwrap().profiles.get(&mri).cloned();
        let presence = super::super::people_presence::want(&mri);
        let mut card = CardData {
            person: PersonData {
                id: mri.clone(),
                name,
                subtitle: String::new(),
                avatar: self.card_photo(&mri),
                kind: "person",
                presence: String::new(),
            },
            loading: cached.is_none(),
            email: known.and_then(|u| u.email).unwrap_or_default(),
            presence_label: super::super::people_presence::status_label(
                &presence,
                &super::super::people_presence::known_activity(&mri),
            )
            .into(),
            presence,
            ..Default::default()
        };
        if let Some((f, _)) = &cached {
            fill_facts(&mut card, f.clone());
        }
        card.badge = card_badge(&mri, &card.email);
        {
            let mut p = self.people.lock().unwrap();
            p.card_email = (!card.email.is_empty()).then(|| card.email.clone());
            p.card_mri = Some(mri.clone());
        }
        // What an earlier card read about their schedule, manager and reports shows at once.
        let (avail, org) = self.cached_card_extras(&mri);
        self.fill_card_extras(&mut card, avail.as_ref(), org.as_ref());
        if modal {
            self.ui.card_fill(card.clone());
        } else {
            self.ui.card(true, card.clone());
        }
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            card.loading = false;
            self.ui.card_details(card);
            return Ok(());
        }
        // Details an earlier card read recently are not read again; the presence always is.
        let stale = cached.is_none_or(|(_, at)| at.elapsed() > CARD_FRESH);
        let details = async {
            if stale {
                Some(self.client.profile_card(&mri).await)
            } else {
                None
            }
        };
        let directory = async {
            if stale {
                Some(self.client.directory_details(&mri).await)
            } else {
                None
            }
        };
        let email0 = card.email.clone();
        let schedule = async {
            if email0.is_empty() {
                None
            } else {
                self.avail_of(&mri, &email0).await
            }
        };
        let (fetched, dir, presence, org, mut avail) = tokio::join!(
            details,
            directory,
            self.client.get_presence(std::slice::from_ref(&mri)),
            self.org_of(&mri),
            schedule
        );
        card.loading = false;
        let fetched = match fetched {
            Some(Ok(c)) => Some(c),
            Some(Err(err)) => {
                tracing::debug!(%err, "profile card");
                None
            }
            None => None,
        };
        let dir = match dir {
            Some(Ok(d)) => Some(d),
            Some(Err(err)) => {
                tracing::debug!(%err, "profile card directory");
                None
            }
            None => None,
        };
        if fetched.is_some() || dir.is_some() {
            let facts = card_facts(fetched, dir);
            self.people
                .lock()
                .unwrap()
                .profiles
                .insert(mri.clone(), (facts.clone(), std::time::Instant::now()));
            fill_facts(&mut card, facts);
            card.badge = card_badge(&mri, &card.email);
            card.person.avatar = self.card_photo(&mri);
        }
        if avail.is_none() && email0.is_empty() && !card.email.is_empty() {
            // The email only came with the details: read the schedule now.
            avail = self.avail_of(&mri, &card.email).await;
        }
        self.fill_card_extras(&mut card, avail.as_ref(), org.as_ref());
        match presence {
            Ok(found) => {
                if let Some(p) = found.into_iter().next() {
                    self.apply_presence(&mri, &p.availability, &p.activity);
                    card.presence_label =
                        super::super::people_presence::status_label(&p.availability, &p.activity)
                            .into();
                    card.presence = p.availability;
                    card.work_location = super::super::people_presence::work_location_label(
                        p.work_location.as_deref(),
                    )
                    .into();
                    let kind = super::super::people_card::location_kind(p.work_location.as_deref());
                    card.work_location_kind = kind.into();
                    self.people.lock().unwrap().org.set_kind(&mri, kind);
                    card.note = p.note.map(|n| n.text).unwrap_or_default();
                    card.ooo = if p.ooo {
                        "Out of office".into()
                    } else {
                        String::new()
                    };
                }
            }
            Err(err) => tracing::debug!(%err, "profile card presence"),
        }
        {
            let mut p = self.people.lock().unwrap();
            if p.card_mri.as_deref() != Some(mri.as_str()) {
                // Another card opened meanwhile.
                return Ok(());
            }
            p.card_email = (!card.email.is_empty()).then(|| card.email.clone());
        }
        self.ui.card_details(card);
        Ok(())
    }
}
