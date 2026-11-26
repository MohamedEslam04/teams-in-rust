//! The Calls page of the rail (plans/calls.md K1.1 to K1.3): call history with Teams' filters,
//! a "Type a name" search that places a call, the speed dial, and the voicemail list.
//!
//! - History is `calling::history::call_history` (the `48:calllogs` thread), newest first.
//! - Calling someone reuses the 1:1 call path of a chat header's Call button
//!   (`call_direct::CallCmd::DialPerson`), so the ringing screen and the call window follow.
//! - The speed dial is kept in the config, per account (Teams keeps its own list in a place we
//!   have not found yet; plans/calls.md K1.2).
//! - Voicemail is Exchange mail read through Graph (`calling::voicemail`); Play downloads the
//!   recording to the cache and opens it in the desktop's audio player. UNVERIFIED live.
//!
//! The row building is pure ([`rows`], [`duration_text`], [`speed_toggle`]) so it is tested
//! without a network.

use super::Engine;
use super::call_direct::CallCmd as DirectCallCmd;
use super::in_order::{self, Rest};
use crate::data::{Pixels, color_for, initials};
use calling::history::{CallDirection, CallOutcome, CallRecord};
use calling::voicemail::Voicemail;
use session::SpeedDial;
use std::sync::Arc;
use std::time::Duration;

/// How many calls the history shows (Teams' list scrolls through about this many).
const HISTORY_LIMIT: usize = 100;
const VOICEMAIL_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallsCmd {
    /// The page was opened: load history and voicemail (again, when already loaded).
    Open,
    /// `all`, `missed`, `incoming`, `outgoing` or `voicemail`.
    Filter(String),
    Query(String),
    /// Place an audio call to this MRI.
    Call(String),
    /// Open the 1:1 chat with this MRI.
    Chat(String),
    /// Add to the speed dial or take off it.
    SpeedToggle(String),
    /// Move up (`-1`) or down (`1`) on the speed dial.
    SpeedMove(String, i32),
    VoicemailPlay(String),
    VoicemailRead(String),
    VoicemailUnread(String),
    VoicemailDelete(String),
    /// Call the person who left this voicemail.
    VoicemailCallBack(String),
    /// Take a call off the history on this computer (call id).
    Hide(String),
    /// Copy a name or number to the clipboard.
    CopyText(String),
}

impl CallsCmd {
    pub fn parse(name: &str, arg: &str) -> Option<CallsCmd> {
        let a = arg.to_owned();
        let need = |a: &str| (!a.is_empty()).then(|| a.to_owned());
        Some(match name {
            "open" | "refresh" => CallsCmd::Open,
            "filter" if FILTERS.contains(&arg) => CallsCmd::Filter(a),
            "query" => CallsCmd::Query(a),
            "call" => CallsCmd::Call(need(arg)?),
            "chat" => CallsCmd::Chat(need(arg)?),
            "speed-toggle" => CallsCmd::SpeedToggle(need(arg)?),
            "speed-up" => CallsCmd::SpeedMove(need(arg)?, -1),
            "speed-down" => CallsCmd::SpeedMove(need(arg)?, 1),
            "voicemail-play" => CallsCmd::VoicemailPlay(need(arg)?),
            "voicemail-read" => CallsCmd::VoicemailRead(need(arg)?),
            "voicemail-unread" => CallsCmd::VoicemailUnread(need(arg)?),
            "voicemail-delete" => CallsCmd::VoicemailDelete(need(arg)?),
            "voicemail-call" => CallsCmd::VoicemailCallBack(need(arg)?),
            "hide" => CallsCmd::Hide(need(arg)?),
            "copy" => CallsCmd::CopyText(need(arg)?),
            _ => return None,
        })
    }
}

const FILTERS: [&str; 5] = ["all", "missed", "incoming", "outgoing", "voicemail"];

/// A row of the history.
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub id: String,
    /// The other party; empty for a group call (no Call back).
    pub mri: String,
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    /// `incoming` or `outgoing`.
    pub direction: String,
    /// `accepted`, `missed`, `declined` or `voicemail`.
    pub outcome: String,
    /// An incoming call the user missed (shown in red).
    pub missed: bool,
    pub when: String,
    pub duration: String,
    pub group: bool,
    /// The other party is on the speed dial.
    pub speed: bool,
    /// The other party's phone number (a PSTN call), empty for a person.
    pub number: String,
}

/// A person in the search results or on the speed dial.
#[derive(Debug, Clone)]
pub struct PersonRow {
    pub mri: String,
    pub name: String,
    pub detail: String,
    pub initials: String,
    pub color: u32,
    pub avatar: Option<Pixels>,
    pub speed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoicemailRow {
    pub id: String,
    pub mri: String,
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub when: String,
    pub duration: String,
    pub transcript: String,
    pub unread: bool,
}

/// Everything the page shows.
#[derive(Debug, Clone, Default)]
pub struct CallsData {
    pub filter: String,
    pub history: Vec<HistoryRow>,
    pub voicemails: Vec<VoicemailRow>,
    pub results: Vec<PersonRow>,
    pub speed: Vec<PersonRow>,
    /// The search box holds a phone number: this is it, tidied. Calling it is not supported.
    pub dial: String,
    pub loading: bool,
    /// Why the history could not be read (empty when it was).
    pub error: String,
    /// Why voicemail could not be read (empty when it was).
    pub voicemail_error: String,
}

#[derive(Default)]
pub struct State {
    records: Vec<CallRecord>,
    voicemails: Vec<Voicemail>,
    filter: String,
    results: Vec<teams_proto::people::Person>,
    /// The phone number the search box holds, if it holds one.
    dial: String,
    /// Bumped per search, so a slow answer to an older query is dropped.
    seq: u64,
    loading: bool,
    error: String,
    voicemail_error: String,
}

/// Whether `r` belongs under `filter`.
pub fn matches(r: &CallRecord, filter: &str) -> bool {
    match filter {
        "missed" => r.is_missed(),
        "incoming" => r.direction == CallDirection::Incoming,
        "outgoing" => r.direction == CallDirection::Outgoing,
        "voicemail" => r.is_voicemail(),
        _ => true,
    }
}

/// "45s", "3m 5s", "1h 2m"; empty when the call never connected.
pub fn duration_text(secs: Option<i64>) -> String {
    match secs {
        None => String::new(),
        Some(s) if s < 60 => format!("{}s", s.max(0)),
        Some(s) if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        Some(s) => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

fn outcome_code(o: CallOutcome) -> &'static str {
    match o {
        CallOutcome::Accepted => "accepted",
        CallOutcome::Missed => "missed",
        CallOutcome::Declined => "declined",
        CallOutcome::Voicemail => "voicemail",
    }
}

/// The history rows under `filter`. `name` resolves an MRI the log did not name; `when` formats
/// a start time; `speed` says whether an MRI is on the speed dial.
pub fn rows(
    records: &[CallRecord],
    filter: &str,
    name: impl Fn(&str) -> String,
    when: impl Fn(i64) -> String,
    speed: impl Fn(&str) -> bool,
) -> Vec<HistoryRow> {
    records
        .iter()
        .filter(|r| matches(r, filter))
        .map(|r| {
            let mri = if r.is_group {
                String::new()
            } else {
                r.other_mri.clone().unwrap_or_default()
            };
            let shown = match (&r.other_name, mri.is_empty()) {
                (Some(n), _) if !r.is_group => n.clone(),
                (_, false) => name(&mri),
                _ => "Group call".to_owned(),
            };
            HistoryRow {
                id: r.id.clone(),
                initials: initials(&shown),
                color: color_for(if mri.is_empty() { &r.id } else { &mri }),
                avatar: None,
                direction: match r.direction {
                    CallDirection::Incoming => "incoming",
                    CallDirection::Outgoing => "outgoing",
                }
                .to_owned(),
                outcome: outcome_code(r.outcome).to_owned(),
                missed: r.is_missed(),
                when: when(r.start_ms),
                duration: duration_text(r.duration_secs),
                group: r.is_group,
                speed: !mri.is_empty() && speed(&mri),
                number: phone_of_mri(&mri),
                name: shown,
                mri,
            }
        })
        .collect()
}

/// The phone number behind a PSTN MRI (`4:+15551230000`), empty for anyone else.
pub fn phone_of_mri(mri: &str) -> String {
    mri.strip_prefix("4:")
        .filter(|n| phone_number(n).is_some())
        .unwrap_or_default()
        .to_owned()
}

/// The search text as a phone number when it is one: digits with an optional leading `+`, and
/// spaces, dots, dashes or brackets between them, 3 to 15 digits. Returns it without the
/// punctuation (`+1 (555) 010-0000` gives `+15550100000`).
pub fn phone_number(query: &str) -> Option<String> {
    let q = query.trim();
    let mut out = String::new();
    for (i, c) in q.chars().enumerate() {
        match c {
            '+' if i == 0 => out.push(c),
            '0'..='9' => out.push(c),
            ' ' | '.' | '-' | '(' | ')' => {}
            _ => return None,
        }
    }
    let digits = out.chars().filter(char::is_ascii_digit).count();
    (3..=15).contains(&digits).then_some(out)
}

/// The key a hidden call is kept under: the account and the call.
fn hidden_key(owner: &str, id: &str) -> String {
    format!("{owner}\u{1f}{id}")
}

/// Takes call `id` off the history for `owner` (once).
pub fn hide_call(list: &mut Vec<String>, owner: &str, id: &str) {
    let key = hidden_key(owner, id);
    if !list.contains(&key) {
        list.push(key);
    }
}

/// The records `owner` has not hidden.
pub fn unhidden(records: &[CallRecord], list: &[String], owner: &str) -> Vec<CallRecord> {
    records
        .iter()
        .filter(|r| !list.contains(&hidden_key(owner, &r.id)))
        .cloned()
        .collect()
}

/// Who to call back for a voicemail from this caller address: a phone number, a person by
/// mail address, or nobody known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallBack {
    Number(String),
    Person(String),
    Unknown,
}

/// A caller address is an email address, or `+number@…` for a call from a phone.
pub fn call_back_target(address: &str) -> CallBack {
    let a = address.trim();
    let local = a.split('@').next().unwrap_or("");
    if let Some(n) = phone_number(local).filter(|_| local.starts_with('+')) {
        CallBack::Number(n)
    } else if a.contains('@') {
        CallBack::Person(a.to_owned())
    } else {
        CallBack::Unknown
    }
}

/// The speed dial with `mri` added at the end, or taken off when already on it.
pub fn speed_toggle(list: &mut Vec<SpeedDial>, owner: &str, mri: &str, name: &str) {
    if let Some(i) = list.iter().position(|s| s.owner == owner && s.mri == mri) {
        list.remove(i);
    } else {
        list.push(SpeedDial {
            owner: owner.to_owned(),
            mri: mri.to_owned(),
            name: name.to_owned(),
        });
    }
}

/// Moves `mri` one place up (`by < 0`) or down among `owner`'s entries; other accounts' entries
/// keep their places.
pub fn speed_move(list: &mut [SpeedDial], owner: &str, mri: &str, by: i32) {
    let mine: Vec<usize> = (0..list.len())
        .filter(|&i| list[i].owner == owner)
        .collect();
    let Some(at) = mine.iter().position(|&i| list[i].mri == mri) else {
        return;
    };
    let to = at as i64 + i64::from(by.signum());
    if to < 0 || to as usize >= mine.len() {
        return;
    }
    list.swap(mine[at], mine[to as usize]);
}

/// The one line a voicemail row shows: the first line of the transcript. The mail's own
/// boilerplate ("You received a voice mail from ...", "Email: ...", "Set Up Voice Mail") is
/// skipped, and so is everything after the first line (a translation, say).
fn transcript_line(preview: &str) -> String {
    const BOILERPLATE: [&str; 6] = [
        "you received a voice mail",
        "you received a voicemail",
        "email:",
        "e-mail:",
        "set up voice mail",
        "set up voicemail",
    ];
    preview
        .lines()
        .map(str::trim)
        .find(|l| {
            let l = l.to_lowercase();
            !l.is_empty() && !BOILERPLATE.iter().any(|b| l.starts_with(b))
        })
        .unwrap_or_default()
        .to_owned()
}

/// The voicemail rows. A voicemail names its caller by mail address, not MRI, so its row has no
/// Call back (`mri` stays empty) until the address is resolved.
fn voicemail_rows(v: &[Voicemail], when: impl Fn(i64) -> String) -> Vec<VoicemailRow> {
    v.iter()
        .map(|m| VoicemailRow {
            id: m.id.clone(),
            mri: String::new(),
            initials: initials(&m.caller_name),
            color: color_for(&m.caller_address),
            name: if m.caller_name.is_empty() {
                "Unknown caller".to_owned()
            } else {
                m.caller_name.clone()
            },
            when: when(m.received_ms),
            duration: duration_text(m.duration_secs),
            transcript: transcript_line(&m.transcript),
            unread: !m.read,
        })
        .collect()
}

impl Engine {
    fn calls_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.calls_page.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn speed_dial(&self) -> Vec<SpeedDial> {
        self.cfg
            .lock()
            .unwrap()
            .speed_dial
            .iter()
            .filter(|s| s.owner == self.me)
            .cloned()
            .collect()
    }

    fn person_row(self: &Arc<Self>, mri: &str, name: &str, detail: &str, speed: bool) -> PersonRow {
        PersonRow {
            mri: mri.to_owned(),
            name: name.to_owned(),
            detail: detail.to_owned(),
            initials: initials(name),
            color: color_for(mri),
            avatar: self.avatar(mri),
            speed,
        }
    }

    fn known_name(&self, mri: &str) -> String {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Unknown".to_owned())
    }

    /// Builds the page from the state and sends it to the UI.
    pub(super) fn push_calls_page(self: &Arc<Self>) {
        let speed = self.speed_dial();
        let on_speed = |mri: &str| speed.iter().any(|s| s.mri == mri);
        let hidden = self.cfg.lock().unwrap().hidden_calls.clone();
        let data = {
            let s = self.calls_state();
            let filter = if s.filter.is_empty() {
                "all".to_owned()
            } else {
                s.filter.clone()
            };
            let shown = unhidden(&s.records, &hidden, &self.me);
            let mut history = rows(
                &shown,
                &filter,
                |m| self.known_name(m),
                super::util::chat_time,
                on_speed,
            );
            for r in &mut history {
                if !r.mri.is_empty() {
                    r.avatar = self.avatar(&r.mri);
                }
            }
            CallsData {
                history,
                voicemails: voicemail_rows(&s.voicemails, super::util::chat_time),
                results: s
                    .results
                    .iter()
                    .map(|p| {
                        let detail = p
                            .job_title
                            .clone()
                            .or_else(|| p.email.clone())
                            .unwrap_or_default();
                        self.person_row(&p.mri, &p.display_name, &detail, on_speed(&p.mri))
                    })
                    .collect(),
                speed: Vec::new(),
                dial: s.dial.clone(),
                loading: s.loading,
                error: s.error.clone(),
                voicemail_error: s.voicemail_error.clone(),
                filter,
            }
        };
        let speed = speed
            .iter()
            .map(|s| {
                // The directory's current name, else the one saved with the entry.
                let known = self.known_name(&s.mri);
                let name = if known == "Unknown" { &s.name } else { &known };
                self.person_row(&s.mri, name, "", true)
            })
            .collect();
        self.ui.calls_page(CallsData { speed, ..data });
    }

    /// The ordered part of a Calls page command (see `in_order`); what waits on the network comes
    /// back as the rest.
    pub(super) fn calls_page_cmd(self: &Arc<Self>, c: CallsCmd) -> anyhow::Result<Option<Rest>> {
        let e = self.clone();
        match c {
            CallsCmd::Open => {
                return Ok(in_order::rest(async move {
                    e.calls_page_load().await;
                    Ok(())
                }));
            }
            CallsCmd::Filter(f) => {
                self.calls_state().filter = f;
                self.push_calls_page();
            }
            CallsCmd::Query(q) => return Ok(self.calls_page_search(&q)),
            CallsCmd::Call(mri) => {
                self.calls_state().results.clear();
                self.push_calls_page();
                self.direct_call_cmd(DirectCallCmd::DialPerson(mri))?;
            }
            CallsCmd::Chat(mri) => {
                return Ok(in_order::rest(async move {
                    e.open_with(&[mri], "", false).await?;
                    Ok(())
                }));
            }
            CallsCmd::SpeedToggle(mri) => {
                let found = {
                    let s = self.calls_state();
                    s.results
                        .iter()
                        .find(|p| p.mri == mri)
                        .map(|p| p.display_name.clone())
                        .or_else(|| {
                            s.records
                                .iter()
                                .find(|r| r.other_mri.as_deref() == Some(mri.as_str()))
                                .and_then(|r| r.other_name.clone())
                        })
                };
                let name = found.unwrap_or_else(|| self.known_name(&mri));
                speed_toggle(
                    &mut self.cfg.lock().unwrap().speed_dial,
                    &self.me,
                    &mri,
                    &name,
                );
                self.save_cfg();
                self.push_calls_page();
            }
            CallsCmd::SpeedMove(mri, by) => {
                speed_move(&mut self.cfg.lock().unwrap().speed_dial, &self.me, &mri, by);
                self.save_cfg();
                self.push_calls_page();
            }
            CallsCmd::VoicemailPlay(id) => {
                return Ok(in_order::rest(async move { e.voicemail_play(&id).await }));
            }
            CallsCmd::VoicemailRead(id) => {
                return Ok(in_order::rest(async move {
                    e.voicemail_set_read(&id, true).await
                }));
            }
            CallsCmd::VoicemailUnread(id) => {
                return Ok(in_order::rest(async move {
                    e.voicemail_set_read(&id, false).await
                }));
            }
            CallsCmd::VoicemailDelete(id) => {
                return Ok(in_order::rest(async move {
                    calling::voicemail::delete(&e.client, &id).await?;
                    e.calls_state().voicemails.retain(|v| v.id != id);
                    e.push_calls_page();
                    e.ui.banner("Voicemail deleted".into());
                    Ok(())
                }));
            }
            CallsCmd::VoicemailCallBack(id) => {
                return Ok(in_order::rest(
                    async move { e.voicemail_call_back(&id).await },
                ));
            }
            CallsCmd::Hide(id) => {
                hide_call(&mut self.cfg.lock().unwrap().hidden_calls, &self.me, &id);
                self.save_cfg();
                self.push_calls_page();
                self.ui
                    .banner("Removed from this list. The call stays in Teams.".into());
            }
            CallsCmd::CopyText(text) => {
                match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
                    Ok(()) => self.ui.banner("Copied".into()),
                    Err(err) => self.ui.banner(format!("Couldn't copy: {err}")),
                }
            }
        }
        Ok(None)
    }

    async fn calls_page_load(self: &Arc<Self>) {
        self.calls_state().loading = true;
        self.push_calls_page();
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.calls_state().loading = false;
            self.push_calls_page();
            return;
        }
        let (history, voicemail) = tokio::join!(
            calling::history::call_history(&self.client, HISTORY_LIMIT),
            calling::voicemail::voicemails(&self.client, VOICEMAIL_LIMIT),
        );
        {
            let mut s = self.calls_state();
            s.loading = false;
            match history {
                Ok(r) => {
                    s.records = r;
                    s.error.clear();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "call history");
                    s.error = "Your call history could not be loaded.".into();
                }
            }
            match voicemail {
                Ok(v) => {
                    s.voicemails = v;
                    s.voicemail_error.clear();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "voicemail");
                    s.voicemail_error = "Voicemail could not be loaded.".into();
                }
            }
        }
        self.push_calls_page();
        // Photos fetched in the background land in the cache; show them once they are in.
        let e = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            e.push_calls_page();
        });
    }

    async fn voicemail_set_read(self: &Arc<Self>, id: &str, read: bool) -> anyhow::Result<()> {
        calling::voicemail::mark_read(&self.client, id, read).await?;
        if let Some(v) = self
            .calls_state()
            .voicemails
            .iter_mut()
            .find(|v| v.id == id)
        {
            v.read = read;
        }
        self.push_calls_page();
        Ok(())
    }

    /// Call back: a voicemail names its caller by mail address, so the directory finds the person
    /// and the 1:1 call path dials them. A call from a phone cannot be called back from ironteams.
    async fn voicemail_call_back(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let address = self
            .calls_state()
            .voicemails
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.caller_address.clone())
            .unwrap_or_default();
        match call_back_target(&address) {
            CallBack::Number(_) => self.ui.banner(
                "This voicemail came from a phone. IronTeams can't call phone numbers yet.".into(),
            ),
            CallBack::Unknown => self
                .ui
                .banner("This voicemail doesn't say who called.".into()),
            CallBack::Person(email) => {
                let found = self.client.search_people(&self.me, &email).await?;
                let person = found.into_iter().find(|p| {
                    p.email
                        .as_deref()
                        .is_some_and(|e| e.eq_ignore_ascii_case(&email))
                });
                match person {
                    Some(p) => self.direct_call_cmd(DirectCallCmd::DialPerson(p.mri))?,
                    None => self
                        .ui
                        .banner("Couldn't find the person who left this voicemail.".into()),
                }
            }
        }
        Ok(())
    }

    /// A search starts here, in order with the commands around it; its answer is dropped when a
    /// newer search started meanwhile.
    fn calls_page_search(self: &Arc<Self>, q: &str) -> Option<Rest> {
        let dial = phone_number(q);
        let seq = {
            let mut s = self.calls_state();
            s.seq += 1;
            s.dial = dial.clone().unwrap_or_default();
            if q.trim().is_empty() || dial.is_some() {
                s.results.clear();
            }
            s.seq
        };
        if q.trim().is_empty() || dial.is_some() {
            self.push_calls_page();
            return None;
        }
        let (e, q) = (self.clone(), q.trim().to_owned());
        in_order::rest(async move {
            let found = e.client.search_people(&e.me, &q).await?;
            {
                let mut s = e.calls_state();
                if s.seq != seq {
                    return Ok(());
                }
                s.results = found.into_iter().filter(|p| p.mri != e.me).collect();
            }
            e.push_calls_page();
            Ok(())
        })
    }

    async fn voicemail_play(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let (name, data) = calling::voicemail::recording(&self.client, id).await?;
        let dir = super::cache::current_account_dir()
            .ok_or_else(|| anyhow::anyhow!("no cache directory"))?
            .join("voicemail");
        std::fs::create_dir_all(&dir)?;
        // The message id is not a safe file name; a hash of it is.
        let ext = std::path::Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("mp3");
        let path = dir.join(format!("{:016x}.{ext}", fxhash(id)));
        std::fs::write(&path, data)?;
        crate::external::open(&path)?;
        if let Err(e) = calling::voicemail::mark_read(&self.client, id, true).await {
            tracing::warn!(error = %e, "voicemail: mark read after play");
        } else if let Some(v) = self
            .calls_state()
            .voicemails
            .iter_mut()
            .find(|v| v.id == id)
        {
            v.read = true;
        }
        self.push_calls_page();
        Ok(())
    }
}

/// FNV-1a: a stable, dependency-free file name for a message id.
fn fxhash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, dir: CallDirection, outcome: CallOutcome) -> CallRecord {
        CallRecord {
            id: id.into(),
            start_ms: 1_000,
            end_ms: Some(70_000),
            duration_secs: (outcome == CallOutcome::Accepted).then_some(65),
            direction: dir,
            outcome,
            other_mri: Some(format!("8:orgid:{id}")),
            other_name: (dir == CallDirection::Incoming).then(|| format!("Person {id}")),
            is_group: false,
            thread_id: None,
            participants: Vec::new(),
            voicemail_message_id: None,
        }
    }

    fn sample() -> Vec<CallRecord> {
        let mut group = rec("g", CallDirection::Outgoing, CallOutcome::Accepted);
        group.is_group = true;
        group.other_mri = None;
        vec![
            rec("a", CallDirection::Incoming, CallOutcome::Missed),
            rec("b", CallDirection::Outgoing, CallOutcome::Accepted),
            rec("c", CallDirection::Incoming, CallOutcome::Voicemail),
            rec("d", CallDirection::Outgoing, CallOutcome::Missed),
            group,
        ]
    }

    fn ids(filter: &str) -> Vec<String> {
        rows(
            &sample(),
            filter,
            |m| m.to_owned(),
            |_| String::new(),
            |_| false,
        )
        .into_iter()
        .map(|r| r.id)
        .collect()
    }

    #[test]
    fn the_filters_pick_what_teams_shows_under_them() {
        assert_eq!(ids("all"), ["a", "b", "c", "d", "g"]);
        assert_eq!(
            ids("missed"),
            ["a"],
            "an unanswered outgoing call is not missed"
        );
        assert_eq!(ids("incoming"), ["a", "c"]);
        assert_eq!(ids("outgoing"), ["b", "d", "g"]);
        assert_eq!(ids("voicemail"), ["c"]);
    }

    #[test]
    fn rows_name_the_other_party_and_flag_missed_and_speed_dial() {
        let r = rows(
            &sample(),
            "all",
            |m| format!("looked up {m}"),
            |ms| format!("t{ms}"),
            |m| m == "8:orgid:b",
        );
        assert_eq!(r[0].name, "Person a", "an incoming call names its caller");
        assert!(r[0].missed);
        assert_eq!(r[0].outcome, "missed");
        assert_eq!(
            r[1].name, "looked up 8:orgid:b",
            "an outgoing one is looked up"
        );
        assert!(r[1].speed);
        assert_eq!(r[1].duration, "1m 5s");
        assert_eq!(r[1].when, "t1000");
        assert_eq!(r[1].direction, "outgoing");
        assert_eq!(r[2].outcome, "voicemail");
        assert_eq!(r[4].name, "Group call");
        assert!(r[4].mri.is_empty(), "no Call back for a group call");
        assert!(!r[4].speed);
    }

    #[test]
    fn durations_read_like_teams() {
        assert_eq!(duration_text(None), "");
        assert_eq!(duration_text(Some(45)), "45s");
        assert_eq!(duration_text(Some(185)), "3m 5s");
        assert_eq!(duration_text(Some(3720)), "1h 2m");
    }

    #[test]
    fn the_speed_dial_toggles_and_moves_within_one_account() {
        let mut list = Vec::new();
        speed_toggle(&mut list, "me", "8:x", "X");
        speed_toggle(&mut list, "other", "8:o", "O");
        speed_toggle(&mut list, "me", "8:y", "Y");
        speed_move(&mut list, "me", "8:y", -1);
        let order: Vec<_> = list.iter().map(|s| s.mri.as_str()).collect();
        assert_eq!(
            order,
            ["8:y", "8:o", "8:x"],
            "the other account's entry stays put"
        );
        speed_move(&mut list, "me", "8:y", -1);
        assert_eq!(list[0].mri, "8:y", "the top stays the top");
        speed_toggle(&mut list, "me", "8:y", "Y");
        assert!(!list.iter().any(|s| s.mri == "8:y"));
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn commands_parse_and_reject_missing_arguments() {
        assert_eq!(CallsCmd::parse("open", ""), Some(CallsCmd::Open));
        assert_eq!(
            CallsCmd::parse("filter", "missed"),
            Some(CallsCmd::Filter("missed".into()))
        );
        assert_eq!(CallsCmd::parse("filter", "spam"), None);
        assert_eq!(
            CallsCmd::parse("call", "8:orgid:x"),
            Some(CallsCmd::Call("8:orgid:x".into()))
        );
        assert_eq!(CallsCmd::parse("call", ""), None);
        assert_eq!(
            CallsCmd::parse("speed-down", "8:x"),
            Some(CallsCmd::SpeedMove("8:x".into(), 1))
        );
        assert_eq!(CallsCmd::parse("dial", "8:x"), None);
    }

    #[test]
    fn a_phone_number_is_told_from_a_name() {
        assert_eq!(
            phone_number("+1 (555) 010-0000"),
            Some("+15550100000".into())
        );
        assert_eq!(phone_number(" 0100 123 4567 "), Some("01001234567".into()));
        assert_eq!(phone_number("555.0100"), Some("5550100".into()));
        assert_eq!(phone_number("12"), None, "too short for a number");
        assert_eq!(
            phone_number("1234567890123456"),
            None,
            "too long for a number"
        );
        assert_eq!(phone_number("sam 555"), None);
        assert_eq!(phone_number("555+0100"), None, "a plus only leads");
        assert_eq!(phone_number("sam@example.com"), None);
        assert_eq!(phone_number(""), None);
    }

    #[test]
    fn a_pstn_mri_carries_its_number_and_a_person_does_not() {
        assert_eq!(phone_of_mri("4:+15550100000"), "+15550100000");
        assert_eq!(phone_of_mri("8:orgid:abc"), "");
        assert_eq!(phone_of_mri(""), "");
        let mut r = rec("p", CallDirection::Outgoing, CallOutcome::Accepted);
        r.other_mri = Some("4:+15550100000".into());
        let rows = rows(&[r], "all", |m| m.to_owned(), |_| String::new(), |_| false);
        assert_eq!(rows[0].number, "+15550100000");
    }

    #[test]
    fn a_hidden_call_stays_off_the_history_for_that_account_only() {
        let mut list = Vec::new();
        hide_call(&mut list, "me", "b");
        hide_call(&mut list, "me", "b");
        assert_eq!(list.len(), 1, "hiding twice keeps one entry");
        hide_call(&mut list, "other", "a");
        let kept: Vec<_> = unhidden(&sample(), &list, "me")
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(kept, ["a", "c", "d", "g"]);
        let kept: Vec<_> = unhidden(&sample(), &list, "other")
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(kept, ["b", "c", "d", "g"]);
    }

    #[test]
    fn a_voicemail_is_called_back_by_mail_address_unless_it_came_from_a_phone() {
        assert_eq!(
            call_back_target("pat@example.com"),
            CallBack::Person("pat@example.com".into())
        );
        assert_eq!(
            call_back_target("+1 555 010 0000@example.com"),
            CallBack::Number("+15550100000".into())
        );
        assert_eq!(call_back_target(""), CallBack::Unknown);
        assert_eq!(call_back_target("not an address"), CallBack::Unknown);
    }

    #[test]
    fn the_new_commands_parse_and_need_their_argument() {
        assert_eq!(
            CallsCmd::parse("hide", "c1"),
            Some(CallsCmd::Hide("c1".into()))
        );
        assert_eq!(CallsCmd::parse("hide", ""), None);
        assert_eq!(
            CallsCmd::parse("copy", "Sam"),
            Some(CallsCmd::CopyText("Sam".into()))
        );
        assert_eq!(
            CallsCmd::parse("voicemail-unread", "m"),
            Some(CallsCmd::VoicemailUnread("m".into()))
        );
        assert_eq!(
            CallsCmd::parse("voicemail-delete", "m"),
            Some(CallsCmd::VoicemailDelete("m".into()))
        );
        assert_eq!(
            CallsCmd::parse("voicemail-call", "m"),
            Some(CallsCmd::VoicemailCallBack("m".into()))
        );
        assert_eq!(CallsCmd::parse("voicemail-delete", ""), None);
    }

    #[test]
    fn voicemail_rows_show_unread_and_unknown_callers() {
        let v = vec![Voicemail {
            id: "m1".into(),
            internet_message_id: None,
            caller_name: String::new(),
            caller_address: String::new(),
            received_ms: 5,
            read: false,
            duration_secs: Some(12),
            transcript: "hello".into(),
        }];
        let r = voicemail_rows(&v, |ms| format!("t{ms}"));
        assert_eq!(r[0].name, "Unknown caller");
        assert!(r[0].unread);
        assert_eq!(r[0].duration, "12s");
        assert_eq!(r[0].when, "t5");
    }

    #[test]
    fn a_voicemail_row_shows_the_first_line_of_the_transcript() {
        // The transcript, a blank line, the language, then its English translation.
        assert_eq!(
            transcript_line(
                "Hi Sam, it's Lee. Please call me back about Friday.\n\nEnglish\nHi Sam, it's Lee. Please call me back about Friday."
            ),
            "Hi Sam, it's Lee. Please call me back about Friday."
        );
        assert_eq!(transcript_line("  Call me.  \r\n\r\nMore."), "Call me.");
        assert_eq!(transcript_line(""), "");
        assert_eq!(transcript_line("\n \n"), "");
    }

    #[test]
    fn a_voicemail_row_drops_the_mail_boilerplate() {
        // Only boilerplate: no transcript.
        assert_eq!(
            transcript_line(
                "You received a voice mail from Pat Example (+1 555 0100).\nEmail: pat@example.com\nSet Up Voice Mail"
            ),
            ""
        );
        // Boilerplate around the transcript.
        assert_eq!(
            transcript_line(
                "You received a voice mail from Pat Example.\nEmail: pat@example.com\n\nCall me.\nSet Up Voice Mail"
            ),
            "Call me."
        );
        let v = Voicemail {
            id: "m".into(),
            internet_message_id: None,
            caller_name: "Pat Example".into(),
            caller_address: "pat@example.com".into(),
            received_ms: 1,
            read: true,
            duration_secs: None,
            transcript: "Call me.\n\nEnglish\nCall me.\nSet Up Voice Mail".into(),
        };
        assert_eq!(
            voicemail_rows(&[v], |_| String::new())[0].transcript,
            "Call me."
        );
    }
}
