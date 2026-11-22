//! People and conversations: @mention candidates, New chat, group chat management, the profile
//! card, the pinned messages banner and read receipts (plan 6.1, 6.2, 6.3, 6.5, 6.6, 6.7).
//! Presence (6.4) is not built: the presence endpoint answers 401 on the work account.

use super::Engine;
use super::mentions::{self, Entry};
use super::outbox::PENDING_PREFIX;
use super::outsider::{Outsider, Standing, classify, is_guest_upn, tagged};
use super::util::{AVATAR_PX, decode_image, one_on_one_members};
use crate::data::{CardData, PersonData, PinnedData};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use store::{ConvKind, ConversationRow, MessageRow, OutboxKind, UserRow};
use teams_proto::mentions::{Mention, MentionKind, parse_mentions};
use teams_proto::people::{Person, oid_of_mri};
use teams_proto::receipts::{Horizon, seen_by};
use teams_proto::tags::Tag;
use teams_proto::threads::one_on_one_id;
use teams_proto::time::now_ms;

/// Read receipts are shown in 1:1 chats and groups up to this size (like Teams).
const RECEIPT_MAX_MEMBERS: usize = 20;
/// How often the open chat's read horizons are fetched again.
const RECEIPT_POLL: Duration = Duration::from_secs(20);
/// Candidates the @mention picker shows.
const MENTION_LIMIT: usize = 8;
/// Pages of history fetched to find a pinned message that is not cached.
const PIN_PAGES: usize = 3;
/// Pages fetched at most when jumping to a message that is not cached.
const JUMP_PAGES: usize = 10;
/// Key of the local "read receipts" switch in the store (`off` turns the display off).
const RECEIPTS_KEY: &str = "read_receipts";

/// Commands of the people UI (`PeopleStore.command(name, argument)`).
#[derive(Debug, Clone, PartialEq)]
pub enum PeopleCmd {
    MentionQuery(String),
    MentionPick(String),
    NewChatOpen,
    NewChatClose,
    NewChatQuery(String),
    NewChatAdd(String),
    NewChatRemove(String),
    NewChatName(String),
    NewChatSend(String),
    Rename(String),
    ParticipantsOpen,
    AddQuery(String),
    AddPick(String),
    AddHistory(bool),
    Leave(String),
    Profile(String),
    ProfileCopyEmail,
    ProfileChat(String),
    /// "Send a quick message" on the profile card: to the person the card shows.
    ProfileMessage(String),
    /// Loads the full profile for this person (the card data, without the hover card).
    ProfileModal(String),
    /// The organization chart: opened at a person, moved to one, back, forward, home.
    OrgOpen(String),
    OrgFocus(String),
    OrgBack,
    OrgForward,
    OrgHome,
    PinJump(String),
    PinUnpin(String),
    Receipts(bool),
}

impl PeopleCmd {
    pub fn parse(name: &str, arg: &str) -> Option<PeopleCmd> {
        let a = arg.to_string();
        Some(match name {
            "mention-query" => PeopleCmd::MentionQuery(a),
            "mention-pick" => PeopleCmd::MentionPick(a),
            "new-chat-open" => PeopleCmd::NewChatOpen,
            "new-chat-close" => PeopleCmd::NewChatClose,
            "new-chat-query" => PeopleCmd::NewChatQuery(a),
            "new-chat-add" => PeopleCmd::NewChatAdd(a),
            "new-chat-remove" => PeopleCmd::NewChatRemove(a),
            "new-chat-name" => PeopleCmd::NewChatName(a),
            "new-chat-send" => PeopleCmd::NewChatSend(a),
            "rename" => PeopleCmd::Rename(a),
            "participants-open" => PeopleCmd::ParticipantsOpen,
            "add-query" => PeopleCmd::AddQuery(a),
            "add-pick" => PeopleCmd::AddPick(a),
            "add-history" => PeopleCmd::AddHistory(arg == "on"),
            "leave" => PeopleCmd::Leave(a),
            "profile" => PeopleCmd::Profile(a),
            "profile-copy-email" => PeopleCmd::ProfileCopyEmail,
            "profile-chat" => PeopleCmd::ProfileChat(a),
            "profile-message" => PeopleCmd::ProfileMessage(a),
            "profile-modal" => PeopleCmd::ProfileModal(a),
            "org-open" => PeopleCmd::OrgOpen(a),
            "org-focus" => PeopleCmd::OrgFocus(a),
            "org-back" => PeopleCmd::OrgBack,
            "org-forward" => PeopleCmd::OrgForward,
            "org-home" => PeopleCmd::OrgHome,
            "pin-jump" => PeopleCmd::PinJump(a),
            "pin-unpin" => PeopleCmd::PinUnpin(a),
            "receipts" => PeopleCmd::Receipts(arg == "on"),
            _ => return None,
        })
    }
}

/// The New chat pane's state.
#[derive(Default)]
struct NewChat {
    to: Vec<Person>,
    results: Vec<Person>,
    group_name: String,
}

/// Engine state of the people pieces.
#[derive(Default)]
pub(super) struct State {
    /// Member MRIs by chat (the aggregator snapshot or a thread fetch). Includes ourselves.
    members: HashMap<String, Vec<String>>,
    /// Candidates of the last @mention query.
    candidates: Vec<Entry>,
    /// Mentions picked in the composer for the draft.
    pub(super) entries: Vec<Entry>,
    /// The tags of each team (by team thread id), fetched the first time a channel mentions one.
    tags: HashMap<String, Vec<Tag>>,
    /// Bumped by every search; a slow answer for an older one is dropped.
    seq: u64,
    /// The same for @mention queries, so typing a mention and a New chat search do not drop each
    /// other's answers.
    mention_seq: u64,
    /// The same for the New chat search. Opening, closing and adding a person bump it too, so a
    /// search still on its way does not fill the list again after them.
    new_chat_seq: u64,
    new_chat: NewChat,
    add_history: bool,
    add_results: Vec<Person>,
    card_email: Option<String>,
    /// The person the profile card shows.
    card_mri: Option<String>,
    /// Who a profile lookup found to be a guest (or from another tenant), by person.
    standing: HashMap<String, Standing>,
    /// People whose standing lookup was already started (once each per session).
    standing_asked: BTreeSet<String>,
    /// What profile cards learned, by person: a second card for them shows it at once.
    profiles: HashMap<String, (CardFacts, std::time::Instant)>,
    /// The schedule, manager and reports the card and the organization chart read.
    pub(super) org: super::people_card::OrgState,
    pins: HashMap<String, Vec<PinnedData>>,
    horizons: HashMap<String, Vec<Horizon>>,
    /// `readReceiptsEnabled` of the account, once known.
    receipts_server: Option<bool>,
    /// A jump to this message is pending; the next render scrolls there.
    jump: Option<String>,
    /// The row the last jump landed on, highlighted for a moment.
    flash: Option<String>,
}

/// A person's profile details as a card last read them.
#[derive(Clone, Default)]
struct CardFacts {
    name: String,
    subtitle: String,
    job_title: String,
    department: String,
    email: String,
    work: String,
    location: String,
    phone: String,
    mobile: String,
    manager: String,
}

/// Details older than this are read again (the card shows the old ones meanwhile).
const CARD_FRESH: Duration = Duration::from_secs(15 * 60);

/// Puts what a card read about a person on the card (an empty value keeps what it had).
fn fill_facts(card: &mut CardData, f: CardFacts) {
    if !f.name.is_empty() {
        card.person.name = f.name;
    }
    card.person.subtitle = f.subtitle;
    card.job_title = f.job_title;
    card.department = f.department;
    if !f.email.is_empty() {
        card.email = f.email;
    }
    card.work = f.work;
    card.location = f.location;
    card.phone = f.phone;
    card.mobile = f.mobile;
    card.manager = f.manager;
}

/// `External` or `Guest` for a person outside our organization, `""` for a colleague (from their
/// MRI and sign-in name: a profile has no tenant id).
fn card_badge(mri: &str, email: &str) -> String {
    classify(mri, None, None, is_guest_upn(email))
        .map_or("", Outsider::label)
        .into()
}

/// The card's details from the Delve card and the directory entry: each field from the first
/// source that has it.
fn card_facts(
    card: Option<teams_proto::card::ProfileCard>,
    dir: Option<teams_proto::card::DirectoryDetails>,
) -> CardFacts {
    let (c, d) = (card.unwrap_or_default(), dir.unwrap_or_default());
    let pick = |a: Option<String>, b: Option<String>| {
        a.filter(|s| !s.is_empty()).or(b).unwrap_or_default()
    };
    let job = pick(c.job_title, d.job_title);
    let dept = pick(c.department, d.department);
    CardFacts {
        name: c.display_name,
        job_title: job.clone(),
        department: dept.clone(),
        subtitle: [job, dept]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        email: c.email.unwrap_or_default(),
        work: pick(c.company, d.company),
        location: pick(c.office_location, d.office_location),
        phone: d.work_phone.unwrap_or_default(),
        mobile: d.mobile_phone.unwrap_or_default(),
        manager: d.manager.unwrap_or_default(),
    }
}

/// Where a New chat or "Chat" button leads.
#[derive(Debug, PartialEq)]
pub(super) enum Plan {
    /// Only ourselves: the notes chat.
    Notes,
    /// A 1:1 chat, by its deterministic id (it exists once the first message is sent).
    Direct(String),
    /// A group chat with exactly these people already exists.
    Existing(String),
    /// A new group chat with these members (not including us).
    CreateGroup(Vec<String>),
    /// Somebody without a directory id (a guest): no deterministic 1:1 id.
    Unsupported,
}

/// Decides what to open for the people picked in the To: field. `groups` are the known group
/// chats with their members. Several people give a group; if one with exactly that roster
/// exists it is opened instead of creating a second one.
pub(super) fn plan(me: &str, picked: &[String], groups: &[(String, Vec<String>)]) -> Plan {
    let others: BTreeSet<&str> = picked
        .iter()
        .map(String::as_str)
        .filter(|m| *m != me)
        .collect();
    match others.len() {
        0 => Plan::Notes,
        1 => {
            let other = others.iter().next().copied().unwrap_or_default();
            match (oid_of_mri(me), oid_of_mri(other)) {
                (Some(a), Some(b)) => Plan::Direct(one_on_one_id(a, b)),
                _ => Plan::Unsupported,
            }
        }
        _ => {
            let roster = |members: &[String]| -> BTreeSet<String> {
                members.iter().filter(|m| *m != me).cloned().collect()
            };
            let want: BTreeSet<String> = others.iter().map(|m| m.to_string()).collect();
            match groups.iter().find(|(_, m)| roster(m) == want) {
                Some((id, _)) => Plan::Existing(id.clone()),
                None => Plan::CreateGroup(want.into_iter().collect()),
            }
        }
    }
}

/// True when every word of `query` starts a word of `name` (case-insensitive).
pub(super) fn matches_query(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    query.to_lowercase().split_whitespace().all(|q| {
        name.split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(q))
    })
}

/// The text of an edited message with `@` put in front of each mention, and the entries so the
/// composer treats them as picked mentions again. Names are looked up in order.
pub(super) fn with_at_signs(text: &str, mentions: &[Mention]) -> (String, Vec<Entry>) {
    let mut out = String::with_capacity(text.len() + mentions.len());
    let mut entries = Vec::new();
    let mut at = 0;
    for m in mentions {
        if m.display_name.is_empty() {
            continue;
        }
        if let Some(rel) = text[at..].find(&m.display_name) {
            let start = at + rel;
            out.push_str(&text[at..start]);
            out.push('@');
            out.push_str(&m.display_name);
            at = start + m.display_name.len();
            entries.push(Entry {
                mri: m.mri.clone(),
                name: m.display_name.clone(),
                kind: m.kind,
            });
        }
    }
    out.push_str(&text[at..]);
    (out, entries)
}

/// Pins newest first. `pinned_at` is an ISO timestamp, so text order is time order.
pub(super) fn sort_pins(pins: &mut [teams_proto::actions::PinnedMessage]) {
    pins.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at));
}

/// My newest message and its read receipt, as `receipt` works it out.
pub(super) struct ReceiptMark {
    pub msg_id: String,
    /// `"sent"` or `"seen"`.
    pub state: String,
    /// The "Seen by" line: the first three names and `+N`.
    pub names: String,
    /// Every reader's name.
    pub readers: Vec<String>,
}

/// The receipt state of my newest message: `("sent", "", [])` until someone has read it, then
/// `("seen", "Name, Name +N", [every reader's name])`. The line names the first three; the list
/// is what the "Seen by" popup shows. `name_of` gives the display name of a member.
pub(super) fn receipt(
    horizons: &[Horizon],
    me: &str,
    msg_id: &str,
    msg_ms: i64,
    one_on_one: bool,
    name_of: impl Fn(&str) -> String,
) -> (String, String, Vec<String>) {
    let seen = seen_by(horizons, me, msg_id, msg_ms);
    if seen.is_empty() {
        return ("sent".into(), String::new(), Vec::new());
    }
    if one_on_one {
        return ("seen".into(), String::new(), Vec::new());
    }
    let all: Vec<String> = seen.iter().map(|m| name_of(m)).collect();
    let line = match all.len() - all.len().min(3) {
        0 => all.join(", "),
        n => format!("{} +{n}", all[..3].join(", ")),
    };
    ("seen".into(), line, all)
}

fn is_chat(kind: ConvKind) -> bool {
    matches!(
        kind,
        ConvKind::OneOnOne | ConvKind::Group | ConvKind::Meeting
    )
}

impl Engine {
    // ---- plumbing -----------------------------------------------------------

    pub(super) async fn people_cmd(self: &Arc<Self>, c: PeopleCmd) -> anyhow::Result<()> {
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
    pub(super) fn on_chat_opened(self: &Arc<Self>, conv: &str) {
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
    pub(super) fn remember_members(&self, chats: &[teams_proto::csa::ChatMeta]) {
        let mut p = self.people.lock().unwrap();
        for c in chats.iter().filter(|c| !c.members.is_empty()) {
            p.members.insert(c.id.clone(), c.members.clone());
        }
    }

    /// Background work of the people pieces: the receipt setting and the receipt poll.
    pub(super) fn start_people(self: &Arc<Self>) {
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
                tokio::time::sleep(RECEIPT_POLL).await;
                let cur = e.view.lock().unwrap().current.clone();
                if let Some(c) = cur {
                    e.refresh_horizons(&c).await;
                }
            }
        });
    }

    fn conv_kind(&self, conv: &str) -> Option<ConvKind> {
        self.store
            .lock()
            .unwrap()
            .conversation(conv)
            .ok()
            .flatten()
            .map(|c| c.kind)
    }

    fn current_chat(&self) -> Option<String> {
        self.view.lock().unwrap().current.clone()
    }

    fn name_of(&self, mri: &str) -> String {
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
    async fn resolve_names(&self, mris: &[String]) {
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

    fn person_data(self: &Arc<Self>, p: &Person) -> PersonData {
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
            presence: super::people_presence::want(&p.mri),
        }
    }

    fn member_data(self: &Arc<Self>, mri: &str) -> PersonData {
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
            presence: super::people_presence::want(mri),
        }
    }

    /// The members of a chat, fetching a group's roster when it is not known yet.
    async fn ensure_members(&self, conv: &str) -> Vec<String> {
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

    pub(super) fn members_known(&self, conv: &str) -> Option<Vec<String>> {
        if self.conv_kind(conv) == Some(ConvKind::OneOnOne) {
            return Some(one_on_one_members(conv));
        }
        self.people.lock().unwrap().members.get(conv).cloned()
    }

    /// Shows or hides the rename pencil and participants button for the open chat.
    pub(super) fn push_group_header(&self, conv: &str) {
        let group = self.conv_kind(conv) == Some(ConvKind::Group);
        let count = self.members_known(conv).map_or(0, |m| m.len());
        self.ui.group_header(group, count);
    }

    // ---- 6.1 @mentions -------------------------------------------------------

    async fn mention_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
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
    fn set_candidates(self: &Arc<Self>, list: Vec<Entry>) {
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
    async fn team_tags(&self, team: &str) -> Vec<Tag> {
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
    fn tag_subtitle(&self, mri: &str) -> String {
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

    fn mention_pick(&self, id: &str) {
        let mut p = self.people.lock().unwrap();
        if let Some(e) = p.candidates.iter().find(|e| e.mri == id).cloned() {
            p.entries.push(e);
        }
    }

    /// Drops picked mentions whose `@Name` is no longer in the composer text.
    pub(super) fn prune_mentions(&self, text: &str) {
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
    pub(super) fn restore_edit_mentions(&self, row: &MessageRow, text: String) -> String {
        let props: Value = serde_json::from_str(&row.reactions_json).unwrap_or_else(|_| json!({}));
        let found = props
            .get("m")
            .map(|m| parse_mentions(Some(&json!({ "mentions": m }))))
            .unwrap_or_default();
        let (text, entries) = with_at_signs(&text, &found);
        self.people.lock().unwrap().entries = entries;
        text
    }

    // ---- 6.2 New chat --------------------------------------------------------

    fn push_new_chat(self: &Arc<Self>, open: bool) {
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
    fn groups(&self) -> Vec<(String, Vec<String>)> {
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

    async fn new_chat_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
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

    fn new_chat_add(self: &Arc<Self>, mri: &str) {
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
    pub(super) fn new_chat_seq_for_test(&self) -> u64 {
        self.people.lock().unwrap().new_chat_seq
    }

    /// Opens (creating it when needed) the chat with `others` and returns its id. A new group is
    /// created here, so this is only called when the first message is sent or Chat is pressed.
    pub(super) async fn open_with(
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
    fn ensure_direct_row(&self, id: &str, other: Option<&str>) {
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
    async fn quick_message(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
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

    async fn new_chat_send(self: &Arc<Self>, text: String) -> anyhow::Result<()> {
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

    async fn rename(self: &Arc<Self>, name: String) -> anyhow::Result<()> {
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
    async fn push_members(self: &Arc<Self>) -> anyhow::Result<()> {
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

    /// What a profile lookup said about `mri`, once it was asked.
    pub(super) fn standing_of(&self, mri: &str) -> Option<Standing> {
        self.people.lock().unwrap().standing.get(mri).cloned()
    }

    /// Looks up, once per session, whether the organization people among `mris` are guests: the
    /// stored profile has no user type and a guest's display name and MRI look like a
    /// colleague's. Returns whether somebody was found to be a guest.
    pub(super) async fn resolve_standing(&self, mris: &[String]) -> bool {
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
    pub(super) fn want_standing(self: &Arc<Self>, mris: Vec<String>) {
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

    async fn add_query(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
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

    async fn add_pick(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
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
    fn group_from_direct(
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
    async fn leave(self: &Arc<Self>, conv: &str) -> anyhow::Result<()> {
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
    fn card_photo(self: &Arc<Self>, mri: &str) -> Option<crate::data::Pixels> {
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
    async fn profile(self: &Arc<Self>, mri: String) -> anyhow::Result<()> {
        self.profile_in(mri, false).await
    }

    /// [`Self::profile`] for the hover or click card (`modal` false), or for the full profile
    /// window (`modal` true: the card data is filled without opening the card).
    pub(super) async fn profile_in(
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
        let presence = super::people_presence::want(&mri);
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
            presence_label: super::people_presence::status_label(
                &presence,
                &super::people_presence::known_activity(&mri),
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
                        super::people_presence::status_label(&p.availability, &p.activity).into();
                    card.presence = p.availability;
                    card.work_location =
                        super::people_presence::work_location_label(p.work_location.as_deref())
                            .into();
                    let kind = super::people_card::location_kind(p.work_location.as_deref());
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

    // ---- 6.6 pinned messages -------------------------------------------------

    /// Fetches the chat's pins and resolves each to its sender and text from the store (or
    /// older history, a few pages at most).
    pub(super) async fn refresh_pins(self: &Arc<Self>, conv: &str) {
        if !self.conv_kind(conv).is_some_and(is_chat)
            || self.offline.load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let Ok(mut pins) = self.client.pinned(conv).await else {
            return;
        };
        sort_pins(&mut pins);
        let ids: Vec<String> = pins.iter().map(|p| p.message_id.clone()).collect();
        self.load_until_found(conv, &ids, PIN_PAGES).await;
        let data: Vec<PinnedData> = ids
            .iter()
            .map(|id| {
                let row = self.store.lock().unwrap().message(id).ok().flatten();
                match row {
                    Some(r) => PinnedData {
                        id: r.id.clone(),
                        sender: if r.sender_name.is_empty() {
                            self.name_of(&r.sender_mri)
                        } else {
                            r.sender_name.clone()
                        },
                        preview: pin_preview(&r),
                    },
                    None => PinnedData {
                        id: id.clone(),
                        sender: String::new(),
                        preview: "Pinned message".into(),
                    },
                }
            })
            .collect();
        self.people
            .lock()
            .unwrap()
            .pins
            .insert(conv.to_string(), data.clone());
        if self.current_chat().as_deref() == Some(conv) {
            self.ui.pinned(data);
        }
        // The message menu offers Unpin on pinned rows.
        self.mark(|d| d.messages = true);
    }

    /// Whether a message is pinned for everyone in its chat (as last read).
    pub(super) fn is_pinned(&self, conv: &str, id: &str) -> bool {
        self.people
            .lock()
            .unwrap()
            .pins
            .get(conv)
            .is_some_and(|p| p.iter().any(|x| x.id == id))
    }

    /// Loads older history of `conv` until all `ids` are stored (at most `pages` pages).
    async fn load_until_found(&self, conv: &str, ids: &[String], pages: usize) {
        for _ in 0..pages {
            let (missing, oldest) = {
                let st = self.store.lock().unwrap();
                let missing = ids.iter().any(|id| st.message(id).ok().flatten().is_none());
                let oldest = st
                    .messages_before(conv, None, 1)
                    .ok()
                    .and_then(|v| v.first().map(|m| m.compose_ms));
                (missing, oldest)
            };
            let Some(oldest) = oldest.filter(|_| missing) else {
                return;
            };
            let Ok(page) = self
                .client
                .messages(conv, Some(oldest), super::PAGE as u32)
                .await
            else {
                return;
            };
            if page.messages.is_empty() {
                return;
            }
            self.store_messages(&page.messages, None);
        }
    }

    async fn unpin(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        self.client.set_pinned(&conv, id, false).await?;
        self.refresh_pins(&conv).await;
        self.ui.banner("Message unpinned".into());
        Ok(())
    }

    /// Scrolls the open chat to a message and highlights it, loading older history when it is
    /// not in the list yet.
    async fn jump_to(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.jump_to_with(id, true).await
    }

    /// [`Self::jump_to`]; with `fade` false the highlight stays until [`Self::clear_flash`]
    /// (Find in chat keeps its current match marked).
    pub(super) async fn jump_to_with(self: &Arc<Self>, id: &str, fade: bool) -> anyhow::Result<()> {
        let Some(conv) = self.current_chat() else {
            return Ok(());
        };
        self.load_until_found(&conv, &[id.to_string()], JUMP_PAGES)
            .await;
        let needed = {
            let st = self.store.lock().unwrap();
            let rows = st.messages_before(&conv, None, 5000)?;
            rows.iter().position(|r| r.id == id).map(|i| rows.len() - i)
        };
        let Some(needed) = needed else {
            self.ui
                .banner("That message is no longer in the chat".into());
            return Ok(());
        };
        {
            let mut v = self.view.lock().unwrap();
            v.thread_root = None;
            v.limit = v.limit.max(needed + 5);
            v.at_bottom = false;
        }
        {
            let mut p = self.people.lock().unwrap();
            p.jump = Some(id.to_string());
            p.flash = Some(id.to_string());
        }
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
        if !fade {
            return Ok(());
        }
        let e = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut p = e.people.lock().unwrap();
            if p.flash.as_deref() == Some(id.as_str()) {
                p.flash = None;
                drop(p);
                e.mark(|d| d.messages = true);
            }
        });
        Ok(())
    }

    /// Marks a row of the open list as the jump target without loading anything (a thread reply
    /// that is already stored); the highlight stays until [`Self::clear_flash`].
    pub(super) fn flash_row(&self, id: &str) {
        {
            let mut p = self.people.lock().unwrap();
            p.jump = Some(id.to_string());
            p.flash = Some(id.to_string());
        }
        self.mark(|d| {
            d.messages = true;
            d.scroll = true;
        });
    }

    pub(super) fn clear_flash(&self) {
        let mut p = self.people.lock().unwrap();
        if p.flash.take().is_some() {
            drop(p);
            self.mark(|d| d.messages = true);
        }
    }

    /// Row index of a pending jump in `out`, which the render scrolls to; marks the flashed row.
    pub(super) fn take_jump(&self, out: &mut [crate::data::MsgData]) -> Option<usize> {
        let mut p = self.people.lock().unwrap();
        if let Some(f) = p.flash.clone()
            && let Some(m) = out.iter_mut().find(|m| m.id == f)
        {
            m.flash = true;
        }
        let target = p.jump.take()?;
        out.iter().position(|m| m.id == target)
    }

    // ---- 6.7 read receipts ---------------------------------------------------

    pub(super) fn receipts_enabled(&self) -> bool {
        let local_off = self
            .store
            .lock()
            .unwrap()
            .get_kv(RECEIPTS_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("off");
        !local_off && self.people.lock().unwrap().receipts_server.unwrap_or(true)
    }

    fn set_receipts(&self, on: bool) {
        let _ = self
            .store
            .lock()
            .unwrap()
            .set_kv(RECEIPTS_KEY, if on { "on" } else { "off" });
        self.ui.receipts_on(self.receipts_enabled());
        self.mark(|d| d.messages = true);
    }

    /// Whether `conv` shows read receipts: a 1:1 chat or a small group.
    fn receipts_apply(&self, conv: &str) -> bool {
        match self.conv_kind(conv) {
            Some(ConvKind::OneOnOne) => true,
            Some(ConvKind::Group) => self
                .members_known(conv)
                .is_some_and(|m| m.len() <= RECEIPT_MAX_MEMBERS),
            _ => false,
        }
    }

    async fn refresh_horizons(self: &Arc<Self>, conv: &str) {
        if !self.receipts_apply(conv)
            || !self.receipts_enabled()
            || self.offline.load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let Ok(h) = self.client.consumption_horizons(conv).await else {
            return;
        };
        let changed = {
            let mut p = self.people.lock().unwrap();
            let changed = p.horizons.get(conv) != Some(&h);
            p.horizons.insert(conv.to_string(), h);
            changed
        };
        if changed && self.current_chat().as_deref() == Some(conv) {
            self.mark(|d| d.messages = true);
        }
    }

    /// The receipt of my newest message in `conv`, or `None` when receipts are off, do not apply
    /// to this chat, or the newest message is still being sent.
    pub(super) fn receipt_mark(&self, conv: &str, rows: &[&MessageRow]) -> Option<ReceiptMark> {
        if !self.receipts_enabled() || !self.receipts_apply(conv) {
            return None;
        }
        let mine = rows
            .iter()
            .rev()
            .find(|r| r.sender_mri == self.me && super::util::system_text(r).is_none())?;
        if mine.id.starts_with(PENDING_PREFIX) {
            return None;
        }
        let horizons = self
            .people
            .lock()
            .unwrap()
            .horizons
            .get(conv)
            .cloned()
            .unwrap_or_default();
        let one = self.conv_kind(conv) == Some(ConvKind::OneOnOne);
        let (state, names, readers) =
            receipt(&horizons, &self.me, &mine.id, mine.compose_ms, one, |m| {
                self.name_of(m)
            });
        Some(ReceiptMark {
            msg_id: mine.id.clone(),
            state,
            names,
            readers,
        })
    }

    // ---- chat list marker ----------------------------------------------------

    /// True when an unread message of `conv` (newer than the read horizon) mentions me.
    pub(super) fn has_unread_mention(&self, conv: &str, read_horizon_ms: i64) -> bool {
        let rows = self
            .store
            .lock()
            .unwrap()
            .messages_before(conv, None, 30)
            .unwrap_or_default();
        rows.iter().any(|r| {
            r.compose_ms > read_horizon_ms
                && r.sender_mri != self.me
                && serde_json::from_str::<Value>(&r.reactions_json)
                    .is_ok_and(|p| mentions::mentions_me(&p, &self.me))
        })
    }

    /// Runs `f` on the store (tests seed and inspect it).
    #[cfg(test)]
    pub(crate) fn test_store<R>(&self, f: impl FnOnce(&store::Store) -> R) -> R {
        f(&self.store.lock().unwrap())
    }

    /// Search results the New chat pane would have received.
    #[cfg(test)]
    pub(crate) fn test_new_chat_results(self: &Arc<Self>, found: Vec<Person>) {
        self.people.lock().unwrap().new_chat.results = found;
        self.push_new_chat(true);
    }

    /// Search results the participants popover's "Add people" would have received.
    #[cfg(test)]
    pub(crate) fn test_add_results(self: &Arc<Self>, found: Vec<Person>) {
        let conv = self.current_chat().unwrap_or_default();
        let members = self.members_known(&conv).unwrap_or_default();
        self.people.lock().unwrap().add_results = found.clone();
        let data = members.iter().map(|m| self.member_data(m)).collect();
        let add = found.iter().map(|f| self.person_data(f)).collect();
        self.ui.members(data, add);
    }

    #[cfg(test)]
    pub(crate) fn test_member_name(self: &Arc<Self>, mri: &str) -> String {
        self.member_data(mri).name
    }

    #[cfg(test)]
    pub(crate) fn test_set_standing(&self, mri: &str, standing: Standing) {
        self.people
            .lock()
            .unwrap()
            .standing
            .insert(mri.into(), standing);
    }

    #[cfg(test)]
    pub(crate) fn test_set_members(&self, conv: &str, members: &[&str]) {
        self.people
            .lock()
            .unwrap()
            .members
            .insert(conv.into(), members.iter().map(|m| m.to_string()).collect());
    }

    #[cfg(test)]
    pub(crate) fn test_set_tags(&self, team: &str, tags: Vec<Tag>) {
        self.people.lock().unwrap().tags.insert(team.into(), tags);
    }

    /// Remembers a person's card details as if a card had read them.
    #[cfg(test)]
    pub(crate) fn test_cache_profile(&self, mri: &str, subtitle: &str, work: &str, phone: &str) {
        let (job, dept) = subtitle.split_once(" · ").unwrap_or((subtitle, ""));
        let facts = CardFacts {
            subtitle: subtitle.into(),
            job_title: job.into(),
            department: dept.into(),
            work: work.into(),
            phone: phone.into(),
            ..CardFacts::default()
        };
        self.people
            .lock()
            .unwrap()
            .profiles
            .insert(mri.to_string(), (facts, std::time::Instant::now()));
    }

    #[cfg(test)]
    pub(crate) fn test_set_pins(&self, conv: &str, pins: Vec<PinnedData>) {
        self.people
            .lock()
            .unwrap()
            .pins
            .insert(conv.into(), pins.clone());
        self.ui.pinned(pins);
    }

    #[cfg(test)]
    pub(crate) fn test_set_horizons(&self, conv: &str, h: Vec<Horizon>) {
        self.people.lock().unwrap().horizons.insert(conv.into(), h);
    }
}

/// One line of a pinned message for the banner.
fn pin_preview(r: &MessageRow) -> String {
    let text =
        ::render::plain_text(&::render::parse_teams_html(&r.content_html)).replace('\n', " ");
    text.chars().take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "8:orgid:00000000-0000-0000-0000-00000000000a";
    const A: &str = "8:orgid:00000000-0000-0000-0000-00000000000b";
    const B: &str = "8:orgid:00000000-0000-0000-0000-00000000000c";
    const C: &str = "8:orgid:00000000-0000-0000-0000-00000000000d";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn commands_parse_by_name() {
        assert_eq!(
            PeopleCmd::parse("mention-query", "sa"),
            Some(PeopleCmd::MentionQuery("sa".into()))
        );
        assert_eq!(
            PeopleCmd::parse("add-history", "on"),
            Some(PeopleCmd::AddHistory(true))
        );
        assert_eq!(
            PeopleCmd::parse("receipts", "off"),
            Some(PeopleCmd::Receipts(false))
        );
        assert_eq!(PeopleCmd::parse("nope", ""), None);
    }

    #[test]
    fn one_person_is_a_direct_chat_with_the_deterministic_id() {
        let want = one_on_one_id(oid_of_mri(ME).unwrap(), oid_of_mri(A).unwrap());
        assert_eq!(plan(ME, &s(&[A]), &[]), Plan::Direct(want.clone()));
        // Adding ourselves changes nothing.
        assert_eq!(plan(ME, &s(&[ME, A]), &[]), Plan::Direct(want));
    }

    #[test]
    fn only_ourselves_is_the_notes_chat() {
        assert_eq!(plan(ME, &s(&[ME]), &[]), Plan::Notes);
        assert_eq!(plan(ME, &[], &[]), Plan::Notes);
    }

    #[test]
    fn several_people_open_the_group_with_exactly_that_roster_or_create_one() {
        let groups = vec![
            ("19:ab@thread.v2".to_string(), s(&[ME, A, B])),
            ("19:abc@thread.v2".to_string(), s(&[ME, A, B, C])),
        ];
        // Order and our own entry do not matter.
        assert_eq!(
            plan(ME, &s(&[B, A]), &groups),
            Plan::Existing("19:ab@thread.v2".into())
        );
        assert_eq!(
            plan(ME, &s(&[ME, C, B, A]), &groups),
            Plan::Existing("19:abc@thread.v2".into())
        );
        // A subset or a different set is a new group.
        assert_eq!(
            plan(ME, &s(&[A, C]), &groups),
            Plan::CreateGroup(s(&[A, C]))
        );
        assert_eq!(
            plan(ME, &s(&[A, B, C, "8:orgid:x"]), &groups),
            Plan::CreateGroup(s(&[A, B, C, "8:orgid:x"]))
        );
    }

    #[test]
    fn a_guest_has_no_direct_chat_id() {
        assert_eq!(plan(ME, &s(&["8:live:guest"]), &[]), Plan::Unsupported);
    }

    #[test]
    fn query_words_must_start_words_of_the_name() {
        assert!(matches_query("Sara Hassan", "sa"));
        assert!(matches_query("Sara Hassan", "ha sa"));
        assert!(matches_query("Sara Hassan", ""));
        assert!(!matches_query("Sara Hassan", "ra"));
        assert!(matches_query("سارة حسن", "حس"));
    }

    #[test]
    fn an_edited_message_gets_its_at_signs_back() {
        let m = |name: &str, mri: &str| Mention {
            item_id: 0,
            mri: mri.into(),
            display_name: name.into(),
            kind: MentionKind::Person,
        };
        let (text, entries) = with_at_signs(
            "Sara Hassan please ask Omar",
            &[m("Sara Hassan", "8:orgid:a"), m("Omar", "8:orgid:b")],
        );
        assert_eq!(text, "@Sara Hassan please ask @Omar");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].mri, "8:orgid:b");
        // A name that is gone from the text is skipped.
        let (text, entries) = with_at_signs("hello", &[m("Sara", "8:orgid:a")]);
        assert_eq!(text, "hello");
        assert!(entries.is_empty());
    }

    #[test]
    fn pins_are_newest_first() {
        let pin = |id: &str, at: &str| teams_proto::actions::PinnedMessage {
            message_id: id.into(),
            folder_id: String::new(),
            pinned_at: at.into(),
        };
        let mut v = vec![
            pin("1", "2026-11-01T10:00:00Z"),
            pin("3", "2026-11-03T10:00:00Z"),
            pin("2", "2026-11-02T10:00:00Z"),
        ];
        sort_pins(&mut v);
        let ids: Vec<&str> = v.iter().map(|p| p.message_id.as_str()).collect();
        assert_eq!(ids, ["3", "2", "1"]);
    }

    fn horizon(mri: &str, id: &str) -> Horizon {
        Horizon {
            mri: mri.into(),
            last_read_id: id.into(),
            read_ms: 0,
        }
    }

    #[test]
    fn receipt_is_sent_until_someone_reads_it_then_seen_with_names() {
        let name = |m: &str| if m == A { "Sara".into() } else { "Omar".into() };
        let none = receipt(&[], ME, "100", 100, false, name);
        assert_eq!(none, ("sent".into(), String::new(), vec![]));
        let older = receipt(&[horizon(A, "90")], ME, "100", 100, false, name);
        assert_eq!(older.0, "sent", "read up to an older message only");
        let seen = receipt(&[horizon(A, "100")], ME, "100", 100, false, name);
        assert_eq!(
            seen,
            ("seen".into(), "Sara".into(), vec!["Sara".to_string()])
        );
        let two = receipt(
            &[horizon(A, "120"), horizon(B, "100"), horizon(ME, "999")],
            ME,
            "100",
            100,
            false,
            name,
        );
        assert_eq!(two.1, "Sara, Omar", "our own horizon is not a reader");
        // 1:1 says just "Seen".
        let one = receipt(&[horizon(A, "100")], ME, "100", 100, true, name);
        assert_eq!(one, ("seen".into(), String::new(), vec![]));
    }

    #[test]
    fn many_readers_are_cut_to_three_names() {
        let hs: Vec<Horizon> = (0..5).map(|i| horizon(&format!("m{i}"), "100")).collect();
        let r = receipt(&hs, ME, "100", 100, false, |m| m.to_uppercase());
        assert_eq!(r.1, "M0, M1, M2 +2");
        assert_eq!(
            r.2,
            ["M0", "M1", "M2", "M3", "M4"],
            "the popup lists every reader, not just the three in the line"
        );
    }
}
