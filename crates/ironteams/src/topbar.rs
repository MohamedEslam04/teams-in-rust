//! The top bar's logic without any window: the search box's slash commands, what its dropdown
//! lists, the recent searches kept on this computer, and the back and forward history. The window
//! side (`ui/topbar.rs`) turns these into rows and clicks.

use crate::find_data::HitData;
use crate::suggest::SuggestData;
use std::path::{Path, PathBuf};

/// Where "Help" in the "…" menu and `/help` go.
pub const HELP_URL: &str = "https://support.microsoft.com/teams";
/// "Feedback" in the "…" menu: IronTeams's own issue tracker (IronTeams is not Microsoft's client).
pub const FEEDBACK_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
/// "Get the mobile app": Microsoft's Teams download page.
pub const MOBILE_APP_URL: &str = "https://www.microsoft.com/microsoft-teams/download-app";

/// Recent searches kept.
pub const RECENT_MAX: usize = 8;

/// File in the config directory that holds the recent searches.
pub const RECENT_FILE: &str = "recent-searches.json";

// ---------------------------------------------------------------------------------------------
// Slash commands.

/// One command of the list that opens when the search box starts with "/".
pub struct SlashCommand {
    pub name: &'static str,
    /// What goes after the name, empty when it takes nothing.
    pub args: &'static str,
    pub what: &'static str,
}

pub const COMMANDS: [SlashCommand; 17] = [
    SlashCommand {
        name: "available",
        args: "",
        what: "Set your status to Available",
    },
    SlashCommand {
        name: "busy",
        args: "",
        what: "Set your status to Busy",
    },
    SlashCommand {
        name: "dnd",
        args: "",
        what: "Set your status to Do not disturb",
    },
    SlashCommand {
        name: "brb",
        args: "",
        what: "Set your status to Be right back",
    },
    SlashCommand {
        name: "away",
        args: "",
        what: "Appear away",
    },
    SlashCommand {
        name: "offline",
        args: "",
        what: "Appear offline",
    },
    SlashCommand {
        name: "unread",
        args: "",
        what: "Show only unread chats",
    },
    SlashCommand {
        name: "unreads",
        args: "",
        what: "Open every unread message",
    },
    SlashCommand {
        name: "saved",
        args: "",
        what: "Open your saved messages",
    },
    SlashCommand {
        name: "mentions",
        args: "",
        what: "Open your @mentions",
    },
    SlashCommand {
        name: "activity",
        args: "",
        what: "Open Activity",
    },
    SlashCommand {
        name: "files",
        args: "",
        what: "Open your files",
    },
    SlashCommand {
        name: "goto",
        args: "<team or channel>",
        what: "Go to a team or channel",
    },
    SlashCommand {
        name: "chat",
        args: "<name>",
        what: "Open a chat with someone",
    },
    SlashCommand {
        name: "call",
        args: "<name>",
        what: "Call someone",
    },
    SlashCommand {
        name: "help",
        args: "",
        what: "Open Teams help",
    },
    SlashCommand {
        name: "keys",
        args: "",
        what: "Show keyboard shortcuts",
    },
];

/// What a typed command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Force my status: the value the presence service takes (`Busy`, `DoNotDisturb`, ...).
    Status(&'static str),
    Unread,
    Unreads,
    Saved,
    Mentions,
    Activity,
    Files,
    Help,
    Keys,
    /// `goto`, `chat` or `call`, and the name typed after it.
    Go(&'static str, String),
    /// A command that needs a name and got none.
    NeedsName(&'static str),
    /// Not a command of the list.
    Unknown(String),
}

/// The status a command sets.
fn status_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "available" => "Available",
        "busy" => "Busy",
        "dnd" => "DoNotDisturb",
        "brb" => "BeRightBack",
        "away" => "Away",
        "offline" => "Offline",
        _ => return None,
    })
}

/// What the text means as a command; `None` when it does not start with `/`.
pub fn parse(input: &str) -> Option<Action> {
    let t = input.trim_start().strip_prefix('/')?;
    let (word, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let word = word.to_lowercase();
    let rest = rest.trim();
    if let Some(s) = status_of(&word) {
        return Some(Action::Status(s));
    }
    Some(match word.as_str() {
        "unread" => Action::Unread,
        "unreads" => Action::Unreads,
        "saved" => Action::Saved,
        "mentions" => Action::Mentions,
        "activity" => Action::Activity,
        "files" => Action::Files,
        "help" => Action::Help,
        "keys" => Action::Keys,
        "goto" | "chat" | "call" => {
            let kind = COMMANDS
                .iter()
                .find(|c| c.name == word)
                .map_or("goto", |c| c.name);
            if rest.is_empty() {
                Action::NeedsName(kind)
            } else {
                Action::Go(kind, rest.to_string())
            }
        }
        _ => Action::Unknown(word),
    })
}

/// The commands the list shows for the text typed: every one for "/", those that start with the
/// word typed so far, and just the one when the word is complete and has more after it.
pub fn matching(input: &str) -> Vec<&'static SlashCommand> {
    let Some(t) = input.trim_start().strip_prefix('/') else {
        return Vec::new();
    };
    let (word, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let word = word.to_lowercase();
    if !rest.is_empty() || t.ends_with(char::is_whitespace) {
        return COMMANDS.iter().filter(|c| c.name == word).collect();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&word))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The dropdown.

/// One row of the dropdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `command`, `person`, `chat`, `channel`, `message`, `search`, `recent` or `clear`.
    pub kind: &'static str,
    /// The command's name, the conversation id or the id of a message hit.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub initials: String,
    pub color: u32,
    /// Non-empty on the first row of a group.
    pub section: &'static str,
    /// What a command row puts in the box when it needs a name.
    pub insert: String,
    /// A message hit: Markdown of the text around the match, the matching words in bold.
    pub snippet: String,
    /// A message hit: when it was sent.
    pub time: String,
    /// A message hit whose text runs right to left.
    pub rtl: bool,
    /// A person row: who the photo's hover card is for ("" for the rest).
    pub person: String,
}

impl Row {
    fn plain(kind: &'static str, id: &str, title: String) -> Row {
        Row {
            kind,
            id: id.to_string(),
            title,
            subtitle: String::new(),
            initials: String::new(),
            color: 0,
            section: "",
            insert: String::new(),
            snippet: String::new(),
            time: String::new(),
            rtl: false,
            person: String::new(),
        }
    }
}

/// The rows for the text in the box: commands after "/", the recent searches for an empty box, and
/// otherwise the suggestions, the messages found, and "Search messages for" at the end.
pub fn rows(
    input: &str,
    recents: &[String],
    suggestions: &[SuggestData],
    messages: &[HitData],
) -> Vec<Row> {
    if input.trim_start().starts_with('/') {
        return matching(input)
            .into_iter()
            .enumerate()
            .map(|(i, c)| Row {
                subtitle: c.what.to_string(),
                section: if i == 0 { "Commands" } else { "" },
                insert: if c.args.is_empty() {
                    String::new()
                } else {
                    format!("/{} ", c.name)
                },
                ..Row::plain(
                    "command",
                    c.name,
                    if c.args.is_empty() {
                        format!("/{}", c.name)
                    } else {
                        format!("/{} {}", c.name, c.args)
                    },
                )
            })
            .collect();
    }
    let q = input.trim();
    if q.is_empty() {
        let mut out: Vec<Row> = recents
            .iter()
            .enumerate()
            .map(|(i, r)| Row {
                section: if i == 0 { "Recent searches" } else { "" },
                ..Row::plain("recent", r, r.clone())
            })
            .collect();
        if !out.is_empty() {
            out.push(Row::plain("clear", "", "Clear recent searches".into()));
        }
        return out;
    }
    let mut out: Vec<Row> = suggestions
        .iter()
        .enumerate()
        .map(|(i, s)| Row {
            subtitle: s.subtitle.clone(),
            initials: s.initials.clone(),
            color: s.color,
            person: s.person.clone(),
            section: if i == 0 { "Suggestions" } else { "" },
            ..Row::plain(s.kind, &s.id, s.title.clone())
        })
        .collect();
    out.extend(messages.iter().enumerate().map(|(i, m)| Row {
        subtitle: m.subtitle.clone(),
        initials: m.initials.clone(),
        color: m.color,
        section: if i == 0 { "Messages" } else { "" },
        snippet: m.snippet.clone(),
        time: m.time.clone(),
        rtl: m.rtl,
        ..Row::plain("message", &m.id, m.title.clone())
    }));
    let mut search = Row::plain("search", "", format!("Search messages for \"{q}\""));
    if !out.is_empty() {
        search.section = "Search";
    }
    out.push(search);
    out
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Run(Action),
    /// Put this in the box and keep typing.
    Insert(String),
    /// Open the Search page for this text.
    Search(String),
    /// Open a person, a chat or a channel: kind, id, title.
    Open(&'static str, String, String),
    /// Open the chat of a message hit at that message: the hit's id.
    OpenMessage(String),
    ClearRecents,
}

/// What choosing `row` means with `input` in the box.
pub fn pick(row: &Row, input: &str) -> Pick {
    match row.kind {
        "command" => {
            // Typed with its name already ("/goto Platf"): run what is typed.
            if let Some(a) = parse(input).filter(|a| matches!(a, Action::Go(k, _) if *k == row.id))
            {
                return Pick::Run(a);
            }
            if row.insert.is_empty() {
                Pick::Run(parse(&format!("/{}", row.id)).unwrap_or(Action::Unknown(row.id.clone())))
            } else {
                Pick::Insert(row.insert.clone())
            }
        }
        "person" => Pick::Open("person", row.id.clone(), row.title.clone()),
        "chat" => Pick::Open("chat", row.id.clone(), row.title.clone()),
        "channel" => Pick::Open("channel", row.id.clone(), row.title.clone()),
        "message" => Pick::OpenMessage(row.id.clone()),
        "recent" => Pick::Search(row.title.clone()),
        "clear" => Pick::ClearRecents,
        _ => Pick::Search(input.trim().to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Recent searches.

/// The searches typed before, newest first, kept in a file on this computer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recents {
    items: Vec<String>,
}

impl Recents {
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Puts `query` first. Empty text and commands are not kept; the same search again moves up.
    pub fn add(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() || q.starts_with('/') {
            return;
        }
        let lower = q.to_lowercase();
        self.items.retain(|r| r.to_lowercase() != lower);
        self.items.insert(0, q.to_string());
        self.items.truncate(RECENT_MAX);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Reads the file in `dir`; a missing or broken one gives an empty list.
    pub fn load(dir: &Path) -> Recents {
        let items = std::fs::read(dir.join(RECENT_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default();
        let mut r = Recents::default();
        // Oldest first, so the file's order is kept.
        for q in items.iter().rev() {
            r.add(q);
        }
        r
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path: PathBuf = dir.join(RECENT_FILE);
        let tmp = dir.join(format!(".{RECENT_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&self.items)?)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Back and forward.

/// A place in the app: the page and the chat that was open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub view: String,
    pub chat: String,
}

/// Most places kept.
const HISTORY_MAX: usize = 50;

/// The pages and chats visited, with a position that Back and Forward move.
#[derive(Debug, Default)]
pub struct History {
    places: Vec<Place>,
    at: usize,
    /// The place Back or Forward just went to, and the chat that was open before: the window
    /// reports a visit for each step on the way there, and those are not new places.
    arriving: Option<(Place, String)>,
}

impl History {
    pub fn can_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.places.len()
    }

    pub fn current(&self) -> Option<&Place> {
        self.places.get(self.at)
    }

    /// The window now shows `view` with `chat` open.
    pub fn visit(&mut self, view: &str, chat: &str) {
        let place = Place {
            view: view.to_string(),
            chat: chat.to_string(),
        };
        if let Some((target, from_chat)) = self.arriving.take() {
            // The way to a place Back chose: the page first (with the old chat), then the chat.
            if place == target {
                return;
            }
            if place.view == target.view && place.chat == from_chat {
                self.arriving = Some((target, from_chat));
                return;
            }
        }
        if self.current() == Some(&place) {
            return;
        }
        // A chat that is not open yet is not a place of its own.
        if view == "chat" && chat.is_empty() && self.places.is_empty() {
            return;
        }
        self.places
            .truncate(self.at + usize::from(!self.places.is_empty()));
        self.places.push(place);
        if self.places.len() > HISTORY_MAX {
            self.places.remove(0);
        }
        self.at = self.places.len() - 1;
    }

    fn step(&mut self, delta: isize, current_chat: &str) -> Option<Place> {
        let to = self.at.checked_add_signed(delta)?;
        let target = self.places.get(to)?.clone();
        self.at = to;
        self.arriving = Some((target.clone(), current_chat.to_string()));
        Some(target)
    }

    /// Moves back; the place to show, if there is one. `current_chat` is the chat open now.
    pub fn back(&mut self, current_chat: &str) -> Option<Place> {
        self.step(-1, current_chat)
    }

    pub fn forward(&mut self, current_chat: &str) -> Option<Place> {
        self.step(1, current_chat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(kind: &'static str, id: &str, title: &str) -> SuggestData {
        SuggestData {
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: "Person".into(),
            initials: "SH".into(),
            color: 1,
            person: String::new(),
        }
    }

    #[test]
    fn every_status_command_names_the_status_the_service_takes() {
        for (cmd, wire) in [
            ("/available", "Available"),
            ("/busy", "Busy"),
            ("/dnd", "DoNotDisturb"),
            ("/brb", "BeRightBack"),
            ("/away", "Away"),
            ("/offline", "Offline"),
        ] {
            assert_eq!(parse(cmd), Some(Action::Status(wire)), "{cmd}");
            assert!(
                teams_proto::presence::Availability::from_wire(wire).is_some(),
                "{wire} is a status of the presence service"
            );
        }
    }

    #[test]
    fn the_other_commands_parse_with_or_without_a_name() {
        assert_eq!(parse("/unread"), Some(Action::Unread));
        assert_eq!(parse("/unreads"), Some(Action::Unreads));
        assert_eq!(parse("  /SAVED "), Some(Action::Saved));
        assert_eq!(parse("/mentions"), Some(Action::Mentions));
        assert_eq!(parse("/activity"), Some(Action::Activity));
        assert_eq!(parse("/files"), Some(Action::Files));
        assert_eq!(parse("/help"), Some(Action::Help));
        assert_eq!(parse("/keys"), Some(Action::Keys));
        assert_eq!(
            parse("/goto Platform  General "),
            Some(Action::Go("goto", "Platform  General".into()))
        );
        assert_eq!(parse("/chat sara"), Some(Action::Go("chat", "sara".into())));
        assert_eq!(parse("/call Sara"), Some(Action::Go("call", "Sara".into())));
        assert_eq!(parse("/call"), Some(Action::NeedsName("call")));
        assert_eq!(parse("/goto   "), Some(Action::NeedsName("goto")));
        assert_eq!(parse("/nope"), Some(Action::Unknown("nope".into())));
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn every_command_of_the_list_parses_and_none_is_unknown() {
        assert_eq!(COMMANDS.len(), 17);
        for c in &COMMANDS {
            let typed = if c.args.is_empty() {
                format!("/{}", c.name)
            } else {
                format!("/{} x", c.name)
            };
            assert!(
                !matches!(parse(&typed), Some(Action::Unknown(_)) | None),
                "{typed}"
            );
        }
    }

    #[test]
    fn the_list_narrows_while_typing() {
        assert_eq!(matching("/").len(), COMMANDS.len());
        let names = |i: &str| matching(i).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("/b"), ["busy", "brb"]);
        assert_eq!(names("/BUS"), ["busy"]);
        assert_eq!(names("/goto Platform"), ["goto"]);
        assert_eq!(names("/goto "), ["goto"]);
        assert!(names("/zzz").is_empty());
        assert!(matching("busy").is_empty());
    }

    #[test]
    fn a_slash_shows_commands_with_the_ones_that_need_a_name_inserting_it() {
        let r = rows("/", &[], &[], &[]);
        assert_eq!(r.len(), COMMANDS.len());
        assert_eq!(r[0].section, "Commands");
        assert_eq!(r[1].section, "");
        let goto = r.iter().find(|r| r.id == "goto").unwrap();
        assert_eq!(goto.title, "/goto <team or channel>");
        assert_eq!(goto.insert, "/goto ");
        let busy = r.iter().find(|r| r.id == "busy").unwrap();
        assert_eq!(busy.title, "/busy");
        assert_eq!(busy.insert, "");
    }

    #[test]
    fn an_empty_box_lists_recent_searches_with_a_clear_row() {
        assert!(rows("", &[], &[], &[]).is_empty());
        let r = rows("  ", &["budget".into(), "launch".into()], &[], &[]);
        let kinds: Vec<_> = r.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, ["recent", "recent", "clear"]);
        assert_eq!(r[0].section, "Recent searches");
        assert_eq!(r[2].title, "Clear recent searches");
    }

    #[test]
    fn typing_lists_suggestions_then_the_message_search() {
        let sug = [
            s("person", "p1", "Sara Hassan"),
            s("channel", "c1", "A › B"),
        ];
        let r = rows("sara", &[], &sug, &[]);
        let kinds: Vec<_> = r.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, ["person", "channel", "search"]);
        assert_eq!(r[2].title, "Search messages for \"sara\"");
        // Alone, the search row has no heading.
        let alone = rows("sara", &[], &[], &[]);
        assert_eq!(alone.len(), 1);
        assert_eq!(alone[0].section, "");
    }

    fn msg(id: &str, sender: &str, rtl: bool) -> HitData {
        HitData {
            id: id.into(),
            kind: "message",
            section: String::new(),
            title: sender.into(),
            subtitle: "Backend Team".into(),
            snippet: "it was **deployed** to staging".into(),
            time: "10:30".into(),
            initials: "SH".into(),
            color: 2,
            avatar: None,
            person: String::new(),
            rtl,
            count: 0,
        }
    }

    #[test]
    fn message_hits_come_after_the_suggestions_in_their_own_group() {
        let sug = [s("person", "p1", "Sara Hassan")];
        let msgs = [msg("m1", "Sara Hassan", false), msg("m2", "Omar", true)];
        let r = rows("dep", &[], &sug, &msgs);
        let kinds: Vec<_> = r.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, ["person", "message", "message", "search"]);
        assert_eq!(r[1].section, "Messages");
        assert_eq!(r[2].section, "");
        assert_eq!(r[3].section, "Search");
        assert_eq!(r[1].title, "Sara Hassan");
        assert_eq!(r[1].subtitle, "Backend Team");
        assert_eq!(r[1].snippet, "it was **deployed** to staging");
        assert_eq!(r[1].time, "10:30");
        assert!(r[2].rtl);
        assert_eq!(pick(&r[1], "dep"), Pick::OpenMessage("m1".into()));
        // Messages alone still head their group.
        let r = rows("dep", &[], &[], &msgs[..1]);
        assert_eq!(r[0].section, "Messages");
        // A command or an empty box shows none.
        assert!(
            rows("/b", &[], &[], &msgs)
                .iter()
                .all(|r| r.kind == "command")
        );
        assert!(rows("", &[], &[], &msgs).is_empty());
    }

    #[test]
    fn choosing_a_row_does_what_its_kind_says() {
        let sug = [s("person", "p1", "Sara Hassan")];
        let r = rows("sa", &[], &sug, &[]);
        assert_eq!(
            pick(&r[0], "sa"),
            Pick::Open("person", "p1".into(), "Sara Hassan".into())
        );
        assert_eq!(pick(&r[1], " sa "), Pick::Search("sa".into()));

        let recent = rows("", &["budget".into()], &[], &[]);
        assert_eq!(pick(&recent[0], ""), Pick::Search("budget".into()));
        assert_eq!(pick(&recent[1], ""), Pick::ClearRecents);

        let cmds = rows("/", &[], &[], &[]);
        let row = |id: &str| cmds.iter().find(|r| r.id == id).unwrap().clone();
        assert_eq!(pick(&row("busy"), "/"), Pick::Run(Action::Status("Busy")));
        assert_eq!(pick(&row("goto"), "/g"), Pick::Insert("/goto ".into()));
        assert_eq!(
            pick(&row("goto"), "/goto Plat"),
            Pick::Run(Action::Go("goto", "Plat".into()))
        );
    }

    #[test]
    fn recent_searches_keep_the_newest_first_without_repeats_or_commands() {
        let mut r = Recents::default();
        for q in ["a", "b", "c", "A", "  ", "/busy"] {
            r.add(q);
        }
        assert_eq!(r.items(), ["A", "c", "b"]);
        for i in 0..20 {
            r.add(&format!("q{i}"));
        }
        assert_eq!(r.items().len(), RECENT_MAX);
        assert_eq!(r.items()[0], "q19");
        r.clear();
        assert!(r.items().is_empty());
    }

    #[test]
    fn recent_searches_survive_a_restart_and_a_broken_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("ironteams-recents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut r = Recents::default();
        r.add("first");
        r.add("second");
        r.save(&dir).unwrap();
        assert_eq!(Recents::load(&dir).items(), ["second", "first"]);
        std::fs::write(dir.join(RECENT_FILE), b"{not json").unwrap();
        assert!(Recents::load(&dir).items().is_empty());
        assert!(Recents::load(&dir.join("missing")).items().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn back_and_forward_walk_the_places_visited() {
        let mut h = History::default();
        assert!(!h.can_back() && !h.can_forward());
        h.visit("chat", "a");
        h.visit("chat", "b");
        h.visit("calendar", "b");
        assert!(h.can_back() && !h.can_forward());
        // The same place twice is one place.
        h.visit("calendar", "b");
        let back = h.back("b").unwrap();
        assert_eq!(
            back,
            Place {
                view: "chat".into(),
                chat: "b".into()
            }
        );
        let back = h.back("b").unwrap();
        assert_eq!(back.chat, "a");
        assert!(!h.can_back() && h.can_forward());
        assert!(h.back("a").is_none());
        assert_eq!(h.forward("a").unwrap().chat, "b");
        assert_eq!(h.forward("b").unwrap().view, "calendar");
        assert!(h.forward("b").is_none());
    }

    #[test]
    fn the_steps_on_the_way_back_are_not_new_places_and_a_new_visit_drops_the_forward_ones() {
        let mut h = History::default();
        h.visit("chat", "a");
        h.visit("chat", "b");
        h.visit("activity", "b");
        // Back to (chat, b): one step, the page changes first.
        assert_eq!(h.back("b").unwrap().view, "chat");
        h.visit("chat", "b");
        assert!(h.can_forward(), "arriving did not cut the forward places");
        // Back to (chat, a) from (chat, b): the page is the same, only the chat changes.
        assert_eq!(h.back("b").unwrap().chat, "a");
        h.visit("chat", "a");
        assert!(!h.can_back() && h.can_forward());
        // From (activity, b) back to (chat, a): the page reports first with the old chat.
        let mut h = History::default();
        h.visit("chat", "a");
        h.visit("activity", "a");
        h.visit("activity", "b");
        assert_eq!(
            h.back("b").unwrap(),
            Place {
                view: "activity".into(),
                chat: "a".into()
            }
        );
        h.visit("activity", "a");
        assert_eq!(h.back("a").unwrap().view, "chat");
        h.visit("chat", "a");
        assert!(!h.can_back());
        // Somewhere new from the middle forgets the way forward.
        h.visit("calendar", "a");
        assert!(!h.can_forward());
        assert!(h.can_back());
    }

    #[test]
    fn a_step_that_never_arrives_does_not_swallow_the_next_real_visit() {
        let mut h = History::default();
        h.visit("chat", "a");
        h.visit("chat", "b");
        assert_eq!(h.back("b").unwrap().chat, "a");
        // The chat never opened; the user goes to the calendar instead.
        h.visit("calendar", "b");
        assert_eq!(h.current().unwrap().view, "calendar");
    }

    #[test]
    fn only_the_last_fifty_places_are_kept() {
        let mut h = History::default();
        for i in 0..80 {
            h.visit("chat", &format!("c{i}"));
        }
        let mut steps = 0;
        let mut chat = "c79".to_string();
        while let Some(p) = h.back(&chat) {
            chat = p.chat;
            steps += 1;
        }
        assert_eq!(steps, HISTORY_MAX - 1);
    }
}
