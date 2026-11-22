use super::*;

impl Engine {
    // ---- 6.1 @mentions -------------------------------------------------------

    pub(super) async fn mention_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        let row = self.store.lock().unwrap().conversation(&conv)?;
        let kind = row.as_ref().map_or(ConvKind::Group, |r| r.kind);
        let seq = {
            let mut p = self.people.lock().unwrap();
            p.mention_seq += 1;
            p.mention_seq
        };
        let mut list: Vec<Entry> = Vec::new();
        let person = |mri: &str, name: String| Entry {
            mri: mri.to_string(),
            name,
            kind: MentionKind::Person,
        };
        match kind {
            ConvKind::Channel => {
                if let Some(r) = &row {
                    list.push(Entry {
                        mri: conv.clone(),
                        name: r.title.clone(),
                        kind: MentionKind::Channel,
                    });
                    if let (Some(team), Some(name)) = (&r.team_id, &r.team_name) {
                        list.push(Entry {
                            mri: team.clone(),
                            name: name.clone(),
                            kind: MentionKind::Team,
                        });
                    }
                    // @tags: the team's groups of people, mentioned as `tag:<id>`.
                    if let Some(team) = &r.team_id {
                        list.extend(self.team_tags(team).await.iter().map(|t| Entry {
                            mri: t.mri(),
                            name: t.name.clone(),
                            kind: MentionKind::Tag,
                        }));
                    }
                }
            }
            ConvKind::Notes => list.push(person(&self.me, self.my_name.clone())),
            ConvKind::Group | ConvKind::OneOnOne => {
                let members = self.ensure_members(&conv).await;
                self.resolve_names(&members).await;
                if kind == ConvKind::Group {
                    list.push(Entry {
                        mri: conv.clone(),
                        name: "Everyone".into(),
                        kind: MentionKind::Everyone,
                    });
                }
                for m in members {
                    // In a 1:1 chat only the other person is worth mentioning.
                    if kind == ConvKind::OneOnOne && m == self.me {
                        continue;
                    }
                    let name = if m == self.me {
                        self.my_name.clone()
                    } else {
                        self.name_of(&m)
                    };
                    list.push(person(&m, name));
                }
            }
            ConvKind::Meeting => {}
        }
        list.retain(|e| matches_query(&e.name, &q));
        list.truncate(MENTION_LIMIT);
        let wants_search = matches!(kind, ConvKind::Channel | ConvKind::Meeting)
            && !q.trim().is_empty()
            && !self.offline.load(std::sync::atomic::Ordering::Relaxed);
        // Loading the members waits on the network: a newer query may have answered meanwhile.
        if self.people.lock().unwrap().mention_seq != seq {
            return Ok(());
        }
        self.set_candidates(list.clone());
        if wants_search && let Ok(found) = self.client.search_people(&self.me, q.trim()).await {
            let p = self.people.lock().unwrap();
            if p.mention_seq == seq {
                for f in found {
                    if list.len() >= MENTION_LIMIT {
                        break;
                    }
                    if !list.iter().any(|e| e.mri == f.mri) {
                        list.push(person(&f.mri, f.display_name));
                    }
                }
                drop(p);
                self.set_candidates(list);
            }
        }
        Ok(())
    }

    /// Stores the candidate list and shows it in the picker.
    pub(super) fn set_candidates(self: &Arc<Self>, list: Vec<Entry>) {
        let data: Vec<PersonData> = list
            .iter()
            .map(|e| PersonData {
                id: e.mri.clone(),
                name: e.name.clone(),
                subtitle: match e.kind {
                    MentionKind::Everyone => "Everyone in this chat".into(),
                    MentionKind::Team => "Team".into(),
                    MentionKind::Channel => "Channel".into(),
                    MentionKind::Tag => self.tag_subtitle(&e.mri),
                    _ => String::new(),
                },
                avatar: matches!(e.kind, MentionKind::Person)
                    .then(|| self.avatar(&e.mri))
                    .flatten(),
                kind: match e.kind {
                    MentionKind::Everyone => "everyone",
                    MentionKind::Team => "team",
                    MentionKind::Channel => "channel",
                    MentionKind::Tag => "tag",
                    _ => "person",
                },
                presence: String::new(),
            })
            .collect();
        self.people.lock().unwrap().candidates = list;
        self.ui.mention_items(data);
    }

    /// The tags of a team, fetched once (none offline or when the fetch fails).
    pub(super) async fn team_tags(&self, team: &str) -> Vec<Tag> {
        if let Some(t) = self.people.lock().unwrap().tags.get(team) {
            return t.clone();
        }
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            return Vec::new();
        }
        match self.client.tags(team).await {
            Ok(t) => {
                self.people
                    .lock()
                    .unwrap()
                    .tags
                    .insert(team.to_string(), t.clone());
                t
            }
            Err(e) => {
                tracing::debug!(error = %e, "team tags for mentions");
                Vec::new()
            }
        }
    }

    /// "Tag · 3 people" under a tag in the mention picker.
    pub(super) fn tag_subtitle(&self, mri: &str) -> String {
        let count = self
            .people
            .lock()
            .unwrap()
            .tags
            .values()
            .flatten()
            .find(|t| t.mri() == mri)
            .map_or(0, |t| t.member_count);
        match count {
            0 => "Tag".into(),
            1 => "Tag · 1 person".into(),
            n => format!("Tag · {n} people"),
        }
    }

    pub(super) fn mention_pick(&self, id: &str) {
        let mut p = self.people.lock().unwrap();
        if let Some(e) = p.candidates.iter().find(|e| e.mri == id).cloned() {
            p.entries.push(e);
        }
    }

    /// Drops picked mentions whose `@Name` is no longer in the composer text.
    pub(in super::super) fn prune_mentions(&self, text: &str) {
        let mut p = self.people.lock().unwrap();
        if p.entries.is_empty() {
            return;
        }
        let kept = mentions::prepare(text, &p.entries).mentions.len();
        if kept == 0 {
            p.entries.clear();
        }
    }

    /// Restores the picked mentions of a message that is being edited, and returns the text with
    /// `@` before each name.
    pub(in super::super) fn restore_edit_mentions(&self, row: &MessageRow, text: String) -> String {
        let props: Value = serde_json::from_str(&row.reactions_json).unwrap_or_else(|_| json!({}));
        let found = props
            .get("m")
            .map(|m| parse_mentions(Some(&json!({ "mentions": m }))))
            .unwrap_or_default();
        let (text, entries) = with_at_signs(&text, &found);
        self.people.lock().unwrap().entries = entries;
        text
    }
}
