use super::*;

impl Engine {
    pub(in super::super) async fn find_cmd(self: &Arc<Self>, cmd: FindCmd) -> anyhow::Result<()> {
        match cmd {
            FindCmd::Search(q) => self.search_start(q).await,
            FindCmd::Suggest(q) => self.suggest_start(q),
            FindCmd::SuggestMessages(q) => self.suggest_messages(q),
            FindCmd::Go(arg) => self.go_to(&arg).await?,
            FindCmd::SearchTab(t) => {
                self.find.lock().unwrap().tab = Tab::parse(&t);
                self.push_search(false);
            }
            FindCmd::Editor(name) => self.editor_open(name).await,
            FindCmd::PickQuery(q) => self.pick_query(q).await,
            FindCmd::Pick(id) => self.pick(&id).await,
            FindCmd::FilterClear(which) => {
                {
                    let mut f = self.find.lock().unwrap();
                    match which.as_str() {
                        "in" => f.in_conv = None,
                        "person" => f.person = None,
                        _ => f.date = Date::Any,
                    }
                }
                self.push_filters();
                self.rerun_search().await;
            }
            FindCmd::Date(d) => {
                {
                    let mut f = self.find.lock().unwrap();
                    f.date = Date::parse(&d);
                    f.editor.clear();
                }
                self.push_filters();
                self.rerun_search().await;
            }
            FindCmd::Open(id) => self.open_hit(&id).await?,
            FindCmd::More => self.more_results().await,
            FindCmd::Quick(kind) => self.quick_open(&kind).await,
            FindCmd::QuickClose => {
                self.find.lock().unwrap().quick = None;
            }
            FindCmd::Unfollow(id) => self.unfollow_row(&id).await?,
            FindCmd::Unsave(id) => self.unsave_row(&id).await?,
            FindCmd::FindOpen => {
                let mut f = self.find.lock().unwrap();
                f.find = ChatFind {
                    open: true,
                    ..Default::default()
                };
            }
            FindCmd::FindClose => self.chat_find_close(),
            FindCmd::FindQuery(q) => self.chat_find_query(q).await,
            FindCmd::FindPrev => self.chat_find_step(false).await,
            FindCmd::FindNext => self.chat_find_step(true).await,
            FindCmd::Tab(t) => {
                self.shared_tab(&t).await;
                if t == "storyline" {
                    self.storyline_open().await;
                }
            }
            FindCmd::ChannelTabOpen(url) => {
                if url.starts_with("https://") {
                    self.clone().handle(super::super::Cmd::OpenUrl(url)).await;
                }
            }
            FindCmd::ChannelTabAdd(arg) => self.add_channel_tab(&arg).await?,
            FindCmd::ChannelTabAddApp(arg) => self.add_channel_app_tab(&arg).await?,
            FindCmd::ChatTabAdd(arg) => self.add_chat_tab(&arg, false).await?,
            FindCmd::ChatTabAddApp(arg) => self.add_chat_tab(&arg, true).await?,
            FindCmd::ChannelTabRename(arg) => self.rename_channel_tab(&arg).await?,
            FindCmd::ChannelTabRemove(id) => self.remove_channel_tab(&id).await?,
            FindCmd::ChannelTabApps => self.load_tab_apps().await,
            FindCmd::SharedFilter(f) => self.shared_filter(&f),
            FindCmd::SharedKeyword(k) => self.shared_keyword(k),
            FindCmd::SharedOpen(url) => {
                self.clone().handle(super::super::Cmd::OpenUrl(url)).await;
            }
            FindCmd::SharedUpload => self.shared_upload().await,
            FindCmd::SharedDownload(url) => self.shared_download(&url).await,
            FindCmd::SharedCopyLink(url) => self.shared_copy_link(&url),
            FindCmd::SharedShare(url) => self.shared_share(&url).await,
            FindCmd::SharedRename(arg) => self.shared_rename(&arg).await,
            FindCmd::SharedDelete(url) => self.shared_delete(&url).await,
            FindCmd::SharedNewFolder(name) => self.shared_new_folder(&name).await,
            FindCmd::SharedNewDoc(kind) => self.shared_new_doc(&kind).await,
            FindCmd::SharedOneDrive => self.shared_onedrive().await,
            FindCmd::Details(open) => self.details(open).await,
            FindCmd::Follow(id) => self.set_follow(&id, true).await?,
            FindCmd::UnfollowMsg(id) => self.set_follow(&id, false).await?,
            FindCmd::MenuFollow(id) => self.menu_follow(&id).await,
        }
        Ok(())
    }

    /// A chat was opened: the find bar, the Shared tab and the quick view close.
    pub(in super::super) fn find_chat_opened(self: &Arc<Self>, conv: &str) {
        {
            let mut f = self.find.lock().unwrap();
            f.find = ChatFind::default();
            f.quick = None;
        }
        self.ui.chat_tab_reset();
        self.ui.quick_open(false);
        self.shared_chat_opened(conv);
    }

    // ---- locations and rows -------------------------------------------------------

    pub(super) fn conv_row(&self, conv: &str) -> Option<ConversationRow> {
        self.store.lock().unwrap().conversation(conv).ok().flatten()
    }

    /// `Team › channel` for channels, the chat's title for chats.
    pub(in super::super) fn location_of(&self, conv: &str, topic: &str, team: &str) -> String {
        match self.conv_row(conv) {
            Some(c) if c.kind == ConvKind::Channel => match &c.team_name {
                Some(t) => format!("{t} › {}", c.title),
                None => c.title,
            },
            Some(c) if c.kind == ConvKind::Notes => "Notes".into(),
            Some(c) => c.title,
            None if !team.is_empty() => format!("{team} › {topic}"),
            None => topic.to_string(),
        }
    }

    pub(super) fn user_name(&self, mri: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
    }

    pub(super) fn hit_from_message(&self, h: &MessageHit) -> Hit {
        let name = if h.sender_name.is_empty() {
            self.user_name(&h.sender_mri).unwrap_or_default()
        } else {
            h.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: h.conversation_id.clone(),
            message: h.message_id.clone(),
            root: h.root_id.clone(),
            time_ms: h.time_ms,
            mri: h.sender_mri.clone(),
            name,
            place: self.location_of(&h.conversation_id, &h.topic, &h.team_name),
            text: h.snippet.clone(),
            marked: String::new(),
            url: String::new(),
        }
    }

    pub(super) fn hit_from_row(&self, m: &MessageRow) -> Hit {
        let name = if m.sender_name.is_empty() {
            self.user_name(&m.sender_mri).unwrap_or_default()
        } else {
            m.sender_name.clone()
        };
        Hit {
            kind: "message",
            conv: m.conversation_id.clone(),
            message: m.id.clone(),
            root: m.parent_id.clone().unwrap_or_default(),
            time_ms: m.compose_ms,
            mri: m.sender_mri.clone(),
            name,
            place: self.location_of(&m.conversation_id, "", ""),
            text: plain_of(m),
            marked: String::new(),
            url: String::new(),
        }
    }

    pub(super) fn row_data(
        self: &Arc<Self>,
        h: &Hit,
        section: String,
        terms: &[String],
    ) -> HitData {
        let name = if h.name.is_empty() {
            "Someone".to_string()
        } else {
            h.name.clone()
        };
        let (title, subtitle) = match h.kind {
            "message" => (name.clone(), h.place.clone()),
            "thread" => (h.place.clone(), h.name.clone()),
            _ => (name.clone(), h.place.clone()),
        };
        let snippet = match h.kind {
            "message" | "thread" if !h.marked.is_empty() => marked_md(&h.marked),
            "message" | "thread" => highlight_md(&h.text, terms, 160),
            _ => String::new(),
        };
        let avatar_of = if h.kind == "message" || h.kind == "person" {
            h.mri.as_str()
        } else {
            ""
        };
        HitData {
            id: h.id(),
            kind: h.kind,
            section,
            title: title.clone(),
            subtitle,
            snippet,
            time: chat_time(h.time_ms),
            initials: initials(&title),
            color: color_for(if h.mri.is_empty() { &h.conv } else { &h.mri }),
            avatar: self.avatar(avatar_of),
            person: avatar_of.to_string(),
            rtl: ::render::base_direction(&h.text) == ::render::Dir::Rtl,
        }
    }

    // ---- 7.1 Search page -----------------------------------------------------------

    pub(super) fn filters_active(f: &State) -> bool {
        f.in_conv.is_some() || f.person.is_some() || f.date != Date::Any
    }

    pub(super) fn push_filters(&self) {
        let f = self.find.lock().unwrap();
        self.ui.search_filters(
            f.in_conv
                .as_ref()
                .map(|(_, t)| t.clone())
                .unwrap_or_default(),
            f.person
                .as_ref()
                .map(|(_, n)| n.clone())
                .unwrap_or_default(),
            f.date.label(),
            f.editor.clone(),
        );
    }

    /// Sends the stored results to the Search page.
    pub(super) fn push_search(self: &Arc<Self>, searching: bool) {
        let (rows, terms, searched, more) = {
            let f = self.find.lock().unwrap();
            (
                rows_for(f.tab, Self::filters_active(&f), &f.hits),
                terms_of(&f.query),
                f.searched,
                (f.more && matches!(f.tab, Tab::Messages | Tab::All))
                    || global::tab_has_more(f.tab, &f.global_more),
            )
        };
        let rows: Vec<HitData> = rows
            .iter()
            .map(|(h, section)| self.row_data(h, section.clone(), &terms))
            .collect();
        self.ui.search_results(rows, searching, searched, more);
    }

    /// Starts a search for `q` (Enter on the Search page or in the top bar).
    pub(super) async fn search_start(self: &Arc<Self>, q: String) {
        {
            let mut f = self.find.lock().unwrap();
            f.query = q.clone();
            f.editor.clear();
        }
        self.ui.show_view("search");
        self.push_filters();
        self.run_search().await;
    }

    pub(super) async fn rerun_search(self: &Arc<Self>) {
        if !self.find.lock().unwrap().query.trim().is_empty() {
            self.run_search().await;
        }
    }
}
