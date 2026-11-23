use super::*;

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
    /// "Jump to the first message" of the open chat (its whole history is read first).
    JumpFirst,
    /// "Jump to a date": `YYYY-MM-DD`, or `days:N` for N days ago.
    JumpDate(String),
    /// A click on a reply's quote: the quoted message.
    QuoteJump(String),
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
            "jump-first" => PeopleCmd::JumpFirst,
            "jump-date" => PeopleCmd::JumpDate(a),
            "quote-jump" => PeopleCmd::QuoteJump(a),
            _ => return None,
        })
    }
}

/// The New chat pane's state.
#[derive(Default)]
pub(super) struct NewChat {
    pub(super) to: Vec<Person>,
    pub(super) results: Vec<Person>,
    pub(super) group_name: String,
}

/// Engine state of the people pieces.
#[derive(Default)]
pub(in super::super) struct State {
    /// Member MRIs by chat (the aggregator snapshot or a thread fetch). Includes ourselves.
    pub(super) members: HashMap<String, Vec<String>>,
    /// Candidates of the last @mention query.
    pub(super) candidates: Vec<Entry>,
    /// Mentions picked in the composer for the draft.
    pub(in super::super) entries: Vec<Entry>,
    /// The tags of each team (by team thread id), fetched the first time a channel mentions one.
    pub(super) tags: HashMap<String, Vec<Tag>>,
    /// Bumped by every search; a slow answer for an older one is dropped.
    pub(super) seq: u64,
    /// The same for @mention queries, so typing a mention and a New chat search do not drop each
    /// other's answers.
    pub(super) mention_seq: u64,
    /// The same for the New chat search. Opening, closing and adding a person bump it too, so a
    /// search still on its way does not fill the list again after them.
    pub(super) new_chat_seq: u64,
    pub(super) new_chat: NewChat,
    pub(super) add_history: bool,
    pub(super) add_results: Vec<Person>,
    pub(super) card_email: Option<String>,
    /// The person the profile card shows.
    pub(super) card_mri: Option<String>,
    /// Who a profile lookup found to be a guest (or from another tenant), by person.
    pub(super) standing: HashMap<String, Standing>,
    /// People whose standing lookup was already started (once each per session).
    pub(super) standing_asked: BTreeSet<String>,
    /// What profile cards learned, by person: a second card for them shows it at once.
    pub(super) profiles: HashMap<String, (CardFacts, std::time::Instant)>,
    /// The schedule, manager and reports the card and the organization chart read.
    pub(in super::super) org: super::super::people_card::OrgState,
    pub(super) pins: HashMap<String, Vec<PinnedData>>,
    pub(super) horizons: HashMap<String, Vec<Horizon>>,
    /// `readReceiptsEnabled` of the account, once known.
    pub(super) receipts_server: Option<bool>,
    /// A jump to this message is pending; the next render scrolls there.
    pub(super) jump: Option<String>,
    /// The row the last jump landed on, highlighted for a moment.
    pub(super) flash: Option<String>,
    /// Bumped by every jump and every chat opened: a jump still reading older history stops when
    /// it no longer holds the latest number.
    pub(super) nav_seq: u64,
    /// A jump is reading older history behind the loading line.
    pub(super) nav_loading: bool,
}

/// A person's profile details as a card last read them.
#[derive(Clone, Default)]
pub(super) struct CardFacts {
    pub(super) name: String,
    pub(super) subtitle: String,
    pub(super) job_title: String,
    pub(super) department: String,
    pub(super) email: String,
    pub(super) work: String,
    pub(super) location: String,
    pub(super) phone: String,
    pub(super) mobile: String,
    pub(super) manager: String,
}

/// Details older than this are read again (the card shows the old ones meanwhile).
pub(super) const CARD_FRESH: Duration = Duration::from_secs(15 * 60);

/// Puts what a card read about a person on the card (an empty value keeps what it had).
pub(super) fn fill_facts(card: &mut CardData, f: CardFacts) {
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
pub(super) fn card_badge(mri: &str, email: &str) -> String {
    classify(mri, None, None, is_guest_upn(email))
        .map_or("", Outsider::label)
        .into()
}

/// The card's details from the Delve card and the directory entry: each field from the first
/// source that has it.
pub(super) fn card_facts(
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
