//! UI-thread side of the top bar: the search box's dropdown (suggestions, recent searches, slash
//! commands) and Back and Forward. Fills the `TopBarStore` global from `crate::topbar` and turns
//! what the user picks into the same calls the rest of the window already makes.
//!
//! Commands from `TopBarStore.command(name, argument)`: input, focus, close, clear, activate,
//! submit, back, forward, visit (`view` U+001F `chat`), help, feedback, mobile-app.
//!
//! Names are suggested for every keystroke; message hits once typing has paused for
//! [`MESSAGE_PAUSE`] (the engine adds the server's after a further pause, see
//! `backend/find/top_messages.rs`).

use super::{UiHandle, img, model, rgb, styled};
use crate::find_data::HitData;
use crate::suggest::SuggestData;
use crate::topbar::{self, Action, History, Pick, Recents, Row};
use crate::{AppWindow, FindStore, SettingsStore, TopBarStore, TopRow};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Typing pauses this long before the cache's messages are searched.
const MESSAGE_PAUSE: Duration = Duration::from_millis(150);
/// Heights of the dropdown's rows and of a group's heading, as `topbar.slint` draws them.
const ROW_H: f32 = 36.0;
const MESSAGE_ROW_H: f32 = 52.0;
const HEADING_H: f32 = 24.0;

struct Ctl {
    recents: Recents,
    /// Where the recent searches are kept; none keeps them in memory only.
    dir: Option<PathBuf>,
    history: History,
    rows: Vec<Row>,
    suggestions: Vec<SuggestData>,
    /// The message hits shown, and the text they were found for.
    messages: Vec<HitData>,
    messages_for: String,
    /// Waits for typing to pause before the messages are searched.
    message_timer: Timer,
    /// A `FindStore` command to send once the controller is free again: an engine that answers
    /// at once (tests) calls back into it.
    pending: Option<(&'static str, String)>,
    ui: UiHandle,
}

type Shared = Rc<RefCell<Ctl>>;

thread_local! {
    static CTL: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Shared> {
    CTL.with(|c| c.borrow().clone())
}

fn s(v: impl Into<SharedString>) -> SharedString {
    v.into()
}

/// Wires the top bar. `config_dir` is where the recent searches live (`None` for tests that do
/// not need them kept).
pub fn install(window: &AppWindow, config_dir: Option<PathBuf>) {
    let recents = config_dir.as_deref().map(Recents::load).unwrap_or_default();
    let shared: Shared = Rc::new(RefCell::new(Ctl {
        recents,
        dir: config_dir,
        history: History::default(),
        rows: Vec::new(),
        suggestions: Vec::new(),
        messages: Vec::new(),
        messages_for: String::new(),
        message_timer: Timer::default(),
        pending: None,
        ui: UiHandle::new(window.as_weak()),
    }));
    CTL.with(|c| *c.borrow_mut() = Some(shared.clone()));
    let weak = window.as_weak();
    window.global::<TopBarStore>().on_command(move |name, arg| {
        let Some(w) = weak.upgrade() else { return };
        // A call from inside a call (a callback fired while a command still runs) is dropped.
        let pending = match shared.try_borrow_mut() {
            Ok(mut c) => {
                command(&mut c, &w, &name, &arg);
                c.pending.take()
            }
            Err(_) => None,
        };
        if let Some((cmd, a)) = pending {
            w.global::<FindStore>().invoke_command(s(cmd), s(a));
        }
    });
}

fn to_row(r: &Row, top: f32, messages: &[HitData]) -> TopRow {
    let (avatar, has_avatar) = messages
        .iter()
        .find(|m| r.kind == "message" && m.id == r.id)
        .map(|m| img(&m.avatar))
        .unwrap_or_default();
    TopRow {
        kind: s(r.kind),
        id: s(r.id.as_str()),
        title: s(r.title.as_str()),
        subtitle: s(r.subtitle.as_str()),
        initials: s(r.initials.as_str()),
        color: rgb(r.color),
        section: s(r.section),
        insert: s(r.insert.as_str()),
        snippet: styled(&r.snippet),
        snippet_plain: s(r.snippet.replace("**", "")),
        time: s(r.time.as_str()),
        rtl: r.rtl,
        person: s(r.person.as_str()),
        avatar,
        has_avatar,
        top,
    }
}

/// Shows the rows for what is in the box; the arrow keys' choice starts over.
fn rebuild(w: &AppWindow, c: &mut Ctl) {
    show_rows(w, c);
    w.global::<TopBarStore>().set_selected(-1);
}

/// Shows the rows for what is in the box and keeps the arrow keys' choice where it is (more
/// results came while the user was choosing).
fn show_rows(w: &AppWindow, c: &mut Ctl) {
    let store = w.global::<TopBarStore>();
    c.rows = topbar::rows(
        &store.get_input(),
        c.recents.items(),
        &c.suggestions,
        &c.messages,
    );
    // Where each row's body starts, so the dropdown can scroll the chosen one into view.
    let mut y = 0.0;
    let rows: Vec<TopRow> = c
        .rows
        .iter()
        .map(|r| {
            if !r.section.is_empty() {
                y += HEADING_H;
            }
            let row = to_row(r, y, &c.messages);
            y += if r.kind == "message" {
                MESSAGE_ROW_H
            } else {
                ROW_H
            };
            row
        })
        .collect();
    store.set_rows(model(rows));
    if store.get_selected() >= c.rows.len() as i32 {
        store.set_selected(c.rows.len() as i32 - 1);
    }
}

/// Forgets the message hits and any search of them still waiting.
fn drop_messages(c: &mut Ctl) {
    c.message_timer.stop();
    c.messages.clear();
    c.messages_for.clear();
}

/// Searches the messages for what is in the box once typing has paused.
fn ask_messages_later(w: &AppWindow, c: &mut Ctl) {
    let weak = w.as_weak();
    c.message_timer
        .start(TimerMode::SingleShot, MESSAGE_PAUSE, move || {
            let Some(w) = weak.upgrade() else { return };
            let q = w.global::<TopBarStore>().get_input();
            let q = q.trim();
            if !q.is_empty() && !q.starts_with('/') {
                w.global::<FindStore>()
                    .invoke_command(s("suggest-messages"), s(q));
            }
        });
}

fn save_recents(c: &Ctl) {
    if let Some(dir) = &c.dir
        && let Err(e) = c.recents.save(dir)
    {
        tracing::warn!(%e, "recent searches not saved");
    }
}

/// Empties the box, closes the dropdown and lifts the chat list filter.
fn clear_input(w: &AppWindow, c: &mut Ctl) {
    let store = w.global::<TopBarStore>();
    store.set_input(s(""));
    store.set_search_open(false);
    c.suggestions.clear();
    drop_messages(c);
    rebuild(w, c);
    w.invoke_search(s(""));
}

fn show_flags(w: &AppWindow, c: &Ctl) {
    let store = w.global::<TopBarStore>();
    store.set_can_back(c.history.can_back());
    store.set_can_forward(c.history.can_forward());
}

fn status_label(wire: &str) -> &'static str {
    match wire {
        "Available" => "Available",
        "Busy" => "Busy",
        "DoNotDisturb" => "Do not disturb",
        "BeRightBack" => "Be right back",
        "Away" => "Appear away",
        _ => "Appear offline",
    }
}

/// Opens a quick view (`saved`, `mentions`) over the chat list's conversation pane.
fn open_quick(w: &AppWindow, kind: &str) {
    w.set_view(s("chat"));
    w.global::<FindStore>().invoke_command(s("quick"), s(kind));
}

/// Runs a typed command. `true` when it did something, so the box can be emptied.
fn run(w: &AppWindow, c: &Ctl, action: Action) -> bool {
    match action {
        Action::Status(wire) => {
            let g = w.global::<SettingsStore>();
            // A command sets the status until it is changed, not for the last chosen duration.
            g.set_status_duration(s("never"));
            g.invoke_command(s("status"), s(wire));
            c.ui.info(format!("Status: {}", status_label(wire)));
        }
        Action::Unread => {
            w.set_view(s("chat"));
            w.set_filter(s("unread"));
            w.invoke_set_filter(s("unread"));
        }
        Action::Saved => open_quick(w, "saved"),
        Action::Mentions => open_quick(w, "mentions"),
        Action::Activity => w.invoke_navigate(s("activity")),
        Action::Files => w.invoke_navigate(s("onedrive")),
        Action::Help => w.invoke_open_url(s(topbar::HELP_URL)),
        Action::Keys => w.set_shortcuts_open(true),
        Action::Go(kind, name) => {
            w.global::<FindStore>()
                .invoke_command(s("go"), s(format!("{kind}\u{1f}{name}")));
        }
        Action::NeedsName(kind) => {
            c.ui.info(format!("Type a name after /{kind}."));
            return false;
        }
        Action::Unknown(word) => {
            c.ui.info(format!(
                "There is no command /{word}. Type / to see the list."
            ));
            return false;
        }
    }
    true
}

/// Opens the Search page for `text`.
fn search(w: &AppWindow, c: &mut Ctl, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    c.recents.add(text);
    save_recents(c);
    // The Search page has the messages now.
    c.message_timer.stop();
    let f = w.global::<FindStore>();
    f.set_query(s(text));
    w.set_view(s("search"));
    f.invoke_command(s("search"), s(text));
    w.global::<TopBarStore>().set_search_open(false);
    rebuild(w, c);
}

/// Shows the person or chat that was chosen in the dropdown (its header from the row, a loading
/// state, no composer) while the engine switches to it, which may take a moment: until then the
/// previous chat stayed on screen with its composer, and what was typed went into its draft.
/// The row's id is already the conversation's (the engine suggests from the cached chats), so
/// nothing is looked up or created first.
fn show_opening(w: &AppWindow, c: &Ctl, kind: &str, id: &str) {
    // The chat already open: the engine has nothing to switch, so nothing would end the wait.
    if w.get_current_id() == id && w.get_thread_root().is_empty() {
        return;
    }
    let Some(row) = c.rows.iter().find(|r| r.kind == kind && r.id == id) else {
        return;
    };
    let shown = if kind == "person" { "chat" } else { "group" };
    super::show_opening(
        w,
        id,
        &row.title,
        &row.initials,
        row.color,
        shown,
        &row.person,
    );
}

fn apply(w: &AppWindow, c: &mut Ctl, pick: Pick, input: &str) {
    match pick {
        Pick::Insert(text) => {
            w.global::<TopBarStore>().set_input(s(text.as_str()));
            c.suggestions.clear();
            rebuild(w, c);
        }
        Pick::Search(text) => search(w, c, &text),
        Pick::Open(kind, id, title) => {
            c.recents.add(input);
            save_recents(c);
            w.global::<FindStore>().set_quick_open(false);
            if kind == "channel" {
                w.set_view(s("teams"));
                w.invoke_select_channel(s(id), s(title));
            } else {
                w.set_view(s("chat"));
                show_opening(w, c, kind, &id);
                w.invoke_select_chat(s(id));
            }
            clear_input(w, c);
        }
        Pick::OpenMessage(id) => {
            c.recents.add(input);
            save_recents(c);
            clear_input(w, c);
            // The engine opens the chat and lands on the message, as for a Search page hit.
            c.pending = Some(("open", id));
        }
        Pick::Run(action) => {
            if run(w, c, action) {
                clear_input(w, c);
            }
        }
        Pick::ClearRecents => {
            c.recents.clear();
            save_recents(c);
            rebuild(w, c);
        }
    }
}

/// Goes to a place of the history.
fn go(w: &AppWindow, place: &topbar::Place) {
    w.invoke_navigate(s(place.view.as_str()));
    if !place.chat.is_empty() && w.get_current_id() != place.chat.as_str() {
        w.invoke_select_chat(s(place.chat.as_str()));
    }
}

/// Runs one command; a `FindStore` command to send after it goes in `c.pending`.
fn command(c: &mut Ctl, w: &AppWindow, name: &str, arg: &str) {
    let store = w.global::<TopBarStore>();
    match name {
        "input" => {
            let q = arg.trim();
            // As in Teams, the box searches (its dropdown, then the Search page) and never
            // filters the chat list: a word left in the box once hid most chats and left the
            // user's sections empty with nothing saying why.
            c.suggestions.clear();
            if q.is_empty() || q.starts_with('/') {
                drop_messages(c);
            } else {
                c.pending = Some(("suggest", q.to_string()));
                // The hits of a query this one grew from stay until the new ones come, so the
                // list does not flicker; any other text drops them.
                if !q.to_lowercase().starts_with(&c.messages_for.to_lowercase()) {
                    c.messages.clear();
                }
                ask_messages_later(w, c);
            }
            store.set_search_open(true);
            rebuild(w, c);
        }
        // The box's clear button, and Escape with text in the box.
        "clear" => clear_input(w, c),
        "focus" => {
            store.set_search_open(true);
            rebuild(w, c);
        }
        "close" => store.set_search_open(false),
        "help" => w.invoke_open_url(s(topbar::HELP_URL)),
        "feedback" => w.invoke_open_url(s(topbar::FEEDBACK_URL)),
        "mobile-app" => w.invoke_open_url(s(topbar::MOBILE_APP_URL)),
        "activate" => {
            let input = store.get_input().to_string();
            if let Some(row) = arg
                .parse::<usize>()
                .ok()
                .and_then(|i| c.rows.get(i))
                .cloned()
            {
                let pick = topbar::pick(&row, &input);
                apply(w, c, pick, &input);
            }
        }
        // Enter with nothing chosen in the list: a command runs, anything else is searched.
        "submit" => {
            let text = arg.trim();
            match topbar::parse(text) {
                Some(action) => apply(w, c, Pick::Run(action), text),
                None => search(w, c, text),
            }
        }
        "back" | "forward" => {
            let chat = w.get_current_id().to_string();
            let place = if name == "back" {
                c.history.back(&chat)
            } else {
                c.history.forward(&chat)
            };
            if let Some(p) = place {
                go(w, &p);
            }
            show_flags(w, c);
        }
        "visit" => {
            let (view, chat) = arg.split_once('\u{1f}').unwrap_or((arg, ""));
            c.history.visit(view, chat);
            show_flags(w, c);
        }
        other => tracing::debug!(command = other, "topbar: unknown command"),
    }
}

impl UiHandle {
    /// The engine's suggestions for `query`; dropped when the box has moved on.
    pub fn topbar_suggestions(&self, query: String, rows: Vec<SuggestData>) {
        self.run(move |w| {
            let Some(shared) = ctl() else { return };
            let Ok(mut c) = shared.try_borrow_mut() else {
                return;
            };
            if w.global::<TopBarStore>().get_input().trim() == query.trim() {
                c.suggestions = rows;
                rebuild(&w, &mut c);
            }
        });
    }

    /// The server is (or has stopped being) asked for message hits: the dropdown holds grey rows
    /// for them meanwhile.
    pub fn topbar_searching(&self, on: bool) {
        self.run(move |w| w.global::<TopBarStore>().set_searching(on));
    }

    /// The message hits for `query`; dropped when the box has moved on.
    pub fn topbar_messages(&self, query: String, hits: Vec<HitData>) {
        self.run(move |w| {
            let Some(shared) = ctl() else { return };
            let Ok(mut c) = shared.try_borrow_mut() else {
                return;
            };
            if w.global::<TopBarStore>().get_input().trim() == query.trim() {
                c.messages = hits;
                c.messages_for = query.trim().to_string();
                show_rows(&w, &mut c);
            }
        });
    }
}
