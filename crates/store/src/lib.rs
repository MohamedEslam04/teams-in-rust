//! On-disk SQLite cache for ironteams.
//!
//! The UI only keeps the visible window of messages in memory; everything
//! else lives here. Rows are plain structs so this crate does not depend on
//! the protocol crates; the app maps protocol types to rows.

mod fts;
mod migrations;
mod model;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};

pub use fts::{MARK_END, MARK_START};
pub use model::{
    ConvFilter, ConvKind, ConversationRow, Draft, HistoryGap, LocalFilters, LocalHit, MessageRow,
    OutboxItem, OutboxKind, OutboxState, SavedMessage, TeamRow, UserRow,
};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown conversation kind in database: {0:?}")]
    UnknownKind(String),
    #[error("unknown outbox kind or state in database: {0:?}")]
    UnknownOutbox(String),
    #[error("database schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Handle to the on-disk cache.
pub struct Store {
    conn: Connection,
}

const CONV_COLS: &str = "id, kind, title, team_id, team_name, last_activity_ms, last_preview, \
                         unread, favorite, read_horizon_ms";
const MSG_COLS: &str = "id, conversation_id, parent_id, sender_mri, sender_name, compose_ms, \
                        edited_ms, deleted, message_type, content_html, client_message_id, \
                        reactions_json";

fn conv_from_row(r: &Row<'_>) -> Result<ConversationRow> {
    let kind: String = r.get(1)?;
    Ok(ConversationRow {
        id: r.get(0)?,
        kind: ConvKind::from_db(&kind).ok_or(Error::UnknownKind(kind))?,
        title: r.get(2)?,
        team_id: r.get(3)?,
        team_name: r.get(4)?,
        last_activity_ms: r.get(5)?,
        last_preview: r.get(6)?,
        unread: r.get(7)?,
        favorite: r.get(8)?,
        read_horizon_ms: r.get(9)?,
    })
}

fn msg_from_row(r: &Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        parent_id: r.get(2)?,
        sender_mri: r.get(3)?,
        sender_name: r.get(4)?,
        compose_ms: r.get(5)?,
        edited_ms: r.get(6)?,
        deleted: r.get(7)?,
        message_type: r.get(8)?,
        content_html: r.get(9)?,
        client_message_id: r.get(10)?,
        reactions_json: r.get(11)?,
    })
}

fn user_from_row(r: &Row<'_>) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        mri: r.get(0)?,
        display_name: r.get(1)?,
        email: r.get(2)?,
        avatar: r.get(3)?,
        updated_ms: r.get(4)?,
    })
}

const OUTBOX_COLS: &str = "client_message_id, conversation_id, html, subject, reply_to, kind, \
                           payload_path, created_ms, attempts, state, last_error, mentions, importance, cards";

fn draft_from_row(r: &Row<'_>) -> rusqlite::Result<Draft> {
    Ok(Draft {
        conversation_id: r.get(0)?,
        text: r.get(1)?,
        reply_to: r.get(2)?,
        updated_ms: r.get(3)?,
    })
}

fn outbox_from_row(r: &Row<'_>) -> Result<OutboxItem> {
    let kind: String = r.get(5)?;
    let state: String = r.get(9)?;
    Ok(OutboxItem {
        client_message_id: r.get(0)?,
        conversation_id: r.get(1)?,
        html: r.get(2)?,
        subject: r.get(3)?,
        reply_to: r.get(4)?,
        kind: OutboxKind::from_db(&kind).ok_or(Error::UnknownOutbox(kind))?,
        payload_path: r.get(6)?,
        created_ms: r.get(7)?,
        attempts: r.get(8)?,
        state: OutboxState::from_db(&state).ok_or(Error::UnknownOutbox(state))?,
        last_error: r.get(10)?,
        mentions: r.get(11)?,
        importance: r.get(12)?,
        cards: r.get(13)?,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The kv key of the time of the last `Store::maintain`.
const MAINTAINED_KEY: &str = "maintained_ms";
const WEEK_MS: i64 = 7 * 24 * 3600 * 1000;

fn unread_mark_key(conversation_id: &str) -> String {
    format!("unread_mark:{conversation_id}")
}

fn limit_i64(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

fn insert_conversation(conn: &Connection, c: &ConversationRow) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO conversations (id, kind, title, team_id, team_name, \
             last_activity_ms, last_preview, unread, favorite, read_horizon_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(id) DO UPDATE SET \
             kind = excluded.kind, title = excluded.title, \
             team_id = excluded.team_id, team_name = excluded.team_name, \
             last_activity_ms = excluded.last_activity_ms, \
             last_preview = excluded.last_preview, unread = excluded.unread, \
             read_horizon_ms = excluded.read_horizon_ms",
    )?
    .execute(params![
        c.id,
        c.kind.as_db(),
        c.title,
        c.team_id,
        c.team_name,
        c.last_activity_ms,
        c.last_preview,
        c.unread,
        c.favorite,
        c.read_horizon_ms
    ])?;
    Ok(())
}

fn insert_message(conn: &Connection, m: &MessageRow) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO messages (id, conversation_id, parent_id, sender_mri, sender_name, \
             compose_ms, edited_ms, deleted, message_type, content_html, \
             client_message_id, reactions_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT(id) DO UPDATE SET \
             content_html = excluded.content_html, edited_ms = excluded.edited_ms, \
             deleted = excluded.deleted, reactions_json = excluded.reactions_json",
    )?
    .execute(params![
        m.id,
        m.conversation_id,
        m.parent_id,
        m.sender_mri,
        m.sender_name,
        m.compose_ms,
        m.edited_ms,
        m.deleted,
        m.message_type,
        m.content_html,
        m.client_message_id,
        m.reactions_json
    ])?;
    Ok(())
}

impl Store {
    /// Opens (creating if needed) the database at `path`, creating parent
    /// directories, enabling WAL and foreign keys, and running migrations.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "foreign_keys", true)?;
        fts::register(&conn)?;
        let mut store = Store { conn };
        migrations::run(&mut store.conn)?;
        // A row still `sending` means the app died mid-send; send it again.
        store.conn.execute(
            "UPDATE outbox SET state = 'queued' WHERE state = 'sending'",
            [],
        )?;
        Ok(store)
    }

    // ---- conversations -------------------------------------------------

    /// Inserts or updates a conversation. On update every field except
    /// `favorite` is overwritten (favorite is a local user choice).
    pub fn upsert_conversation(&self, c: &ConversationRow) -> Result<()> {
        insert_conversation(&self.conn, c)
    }

    /// [`Store::upsert_conversation`] for a list inside one transaction: one commit instead of
    /// one per row, so the store lock is held for far less time.
    pub fn upsert_conversations(&mut self, rows: &[ConversationRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for c in rows {
            insert_conversation(&tx, c)?;
        }
        Ok(tx.commit()?)
    }

    /// Forgets a conversation (after leaving a group chat): its row, cached messages, draft and
    /// unread mark. Queued outbox items are left alone.
    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        for sql in [
            "DELETE FROM messages WHERE conversation_id = ?1",
            "DELETE FROM drafts WHERE conversation_id = ?1",
            "DELETE FROM history_gaps WHERE conversation_id = ?1",
            "DELETE FROM conversations WHERE id = ?1",
        ] {
            self.conn.prepare_cached(sql)?.execute([id])?;
        }
        self.conn
            .prepare_cached("DELETE FROM kv WHERE key = ?1")?
            .execute([unread_mark_key(id)])?;
        Ok(())
    }

    pub fn set_favorite(&self, id: &str, fav: bool) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE conversations SET favorite = ?2 WHERE id = ?1")?
            .execute(params![id, fav])?;
        Ok(())
    }

    /// Raises the read horizon to `max(old, horizon_ms)`, clears `unread` and
    /// any manual unread mark.
    pub fn mark_read(&self, id: &str, horizon_ms: i64) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE conversations SET unread = 0, \
                     read_horizon_ms = max(read_horizon_ms, ?2) WHERE id = ?1",
            )?
            .execute(params![id, horizon_ms])?;
        self.conn
            .prepare_cached("DELETE FROM kv WHERE key = ?1")?
            .execute([unread_mark_key(id)])?;
        Ok(())
    }

    /// Manual "Mark as unread": sets `unread` and moves the read horizon back
    /// to `horizon_ms` (the message before the first unread one). The mark is
    /// remembered so a sync does not undo it; [`Store::mark_read`] clears it.
    /// Does nothing for an unknown conversation.
    pub fn mark_unread(&self, id: &str, horizon_ms: i64) -> Result<()> {
        let changed = self
            .conn
            .prepare_cached(
                "UPDATE conversations SET unread = 1, read_horizon_ms = ?2 WHERE id = ?1",
            )?
            .execute(params![id, horizon_ms])?;
        if changed > 0 {
            self.set_kv(&unread_mark_key(id), &horizon_ms.to_string())?;
        }
        Ok(())
    }

    /// The horizon of a manual unread mark, if the conversation has one.
    pub fn unread_mark(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .get_kv(&unread_mark_key(id))?
            .and_then(|v| v.parse().ok()))
    }

    /// Conversations ordered by `last_activity_ms` descending (id breaks ties).
    pub fn conversations(&self, filter: ConvFilter, limit: usize) -> Result<Vec<ConversationRow>> {
        let wher = match filter {
            ConvFilter::All => "1",
            ConvFilter::Unread => "unread = 1",
            ConvFilter::Chats => "kind <> 'channel'",
            ConvFilter::Channels => "kind = 'channel'",
            ConvFilter::Favorites => "favorite = 1",
        };
        let sql = format!(
            "SELECT {CONV_COLS} FROM conversations WHERE {wher} \
             ORDER BY last_activity_ms DESC, id LIMIT ?1"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut rows = stmt.query([limit_i64(limit)])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(conv_from_row(r)?);
        }
        Ok(out)
    }

    pub fn conversation(&self, id: &str) -> Result<Option<ConversationRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {CONV_COLS} FROM conversations WHERE id = ?1"
        ))?;
        let mut rows = stmt.query([id])?;
        rows.next()?.map(conv_from_row).transpose()
    }

    /// Case-insensitive substring match on the title, newest activity first.
    /// Folding is done with Unicode lowercasing (SQLite's `LIKE` only folds
    /// ASCII); conversation lists are small enough to scan.
    pub fn search_conversations(&self, query: &str, limit: usize) -> Result<Vec<ConversationRow>> {
        let needle = query.to_lowercase();
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {CONV_COLS} FROM conversations ORDER BY last_activity_ms DESC, id"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while out.len() < limit {
            let Some(r) = rows.next()? else { break };
            let title: String = r.get(2)?;
            if title.to_lowercase().contains(&needle) {
                out.push(conv_from_row(r)?);
            }
        }
        Ok(out)
    }

    /// Moves a conversation's activity forward. Does nothing unless
    /// `activity_ms` is newer than the stored value. When it is, the preview
    /// is replaced and `unread` is only ever raised (use [`Store::mark_read`]
    /// to clear it).
    pub fn touch_conversation(
        &self,
        id: &str,
        activity_ms: i64,
        preview: &str,
        unread: bool,
    ) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE conversations SET last_activity_ms = ?2, last_preview = ?3, \
                     unread = (unread OR ?4) \
                 WHERE id = ?1 AND ?2 > last_activity_ms",
            )?
            .execute(params![id, activity_ms, preview, unread])?;
        Ok(())
    }

    // ---- messages ------------------------------------------------------

    /// Inserts a message, or on id conflict updates content, edit time,
    /// deleted flag and reactions in place. Does not touch the conversation
    /// row; call [`Store::touch_conversation`] for that.
    pub fn upsert_message(&self, m: &MessageRow) -> Result<()> {
        insert_message(&self.conn, m)
    }

    /// [`Store::upsert_message`] for a batch inside one transaction: a history page is up to
    /// hundreds of rows (each firing the FTS trigger), so it costs one commit, not one per row.
    pub fn upsert_messages(&mut self, rows: &[MessageRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for m in rows {
            insert_message(&tx, m)?;
        }
        Ok(tx.commit()?)
    }

    /// Marks a message deleted and drops its content, keeping the row so
    /// threads and paging stay stable.
    pub fn delete_message_local(&self, id: &str) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE messages SET deleted = 1, content_html = '' WHERE id = ?1")?
            .execute([id])?;
        Ok(())
    }

    /// Top-level messages (no `parent_id`): the newest `limit` strictly older
    /// than `before_ms` (or the newest overall when `None`), returned in
    /// ascending compose order.
    pub fn messages_before(
        &self,
        conversation_id: &str,
        before_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {MSG_COLS} FROM messages \
             WHERE conversation_id = ?1 AND parent_id IS NULL AND compose_ms < ?2 \
             ORDER BY compose_ms DESC, id DESC LIMIT ?3"
        ))?;
        let mut out = stmt
            .query_map(
                params![
                    conversation_id,
                    before_ms.unwrap_or(i64::MAX),
                    limit_i64(limit)
                ],
                msg_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.reverse();
        Ok(out)
    }

    /// Messages (replies included, deleted ones not) whose HTML contains `needle`, newest first,
    /// in one conversation or in all of them. SQLite's `LIKE` only folds ASCII case, so this is
    /// a prefilter: the caller confirms the match on the plain text.
    pub fn messages_containing(
        &self,
        conversation_id: Option<&str>,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<MessageRow>> {
        let pattern = format!(
            "%{}%",
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {MSG_COLS} FROM messages \
             WHERE deleted = 0 AND content_html LIKE ?1 ESCAPE '\\' \
               AND (?2 IS NULL OR conversation_id = ?2) \
             ORDER BY compose_ms DESC, id DESC LIMIT ?3"
        ))?;
        let out = stmt
            .query_map(
                params![pattern, conversation_id, limit_i64(limit)],
                msg_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Offline full-text search of the cached messages (deleted and `Control/` ones excluded):
    /// every word of `query` must start a word of the message's text or of its sender's name,
    /// matched without case, Latin accents, tashkeel or Arabic letter variants (see `fts`). The
    /// query is never read as FTS syntax. Newest first, like the server results it is merged
    /// with and like Teams lists them; among messages of the same millisecond the better bm25
    /// rank comes first. A query without a word finds nothing.
    pub fn search_messages_local(
        &self,
        query: &str,
        filters: &LocalFilters<'_>,
        limit: usize,
    ) -> Result<Vec<LocalHit>> {
        let Some(fts_query) = fts::match_query(query) else {
            return Ok(vec![]);
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.conversation_id, m.parent_id, m.sender_mri, m.sender_name, \
                 m.compose_ms, m.content_html, \
                 snippet(messages_fts, 0, char(2), char(3), '', 16) \
             FROM messages_fts JOIN messages m ON m.rowid = messages_fts.rowid \
             WHERE messages_fts MATCH ?1 AND m.deleted = 0 \
               AND m.message_type NOT LIKE 'ThreadActivity/%' \
               AND m.message_type NOT LIKE 'Event/%' \
               AND m.message_type NOT LIKE 'Control/%' \
               AND (?2 IS NULL OR m.conversation_id = ?2) \
               AND (?3 IS NULL OR m.sender_mri = ?3) \
               AND m.compose_ms >= ?4 \
             ORDER BY m.compose_ms DESC, bm25(messages_fts), m.id DESC LIMIT ?5",
        )?;
        let out = stmt
            .query_map(
                params![
                    fts_query,
                    filters.conversation,
                    filters.sender,
                    filters.since_ms.unwrap_or(i64::MIN),
                    limit_i64(limit)
                ],
                |r| {
                    let html: String = r.get(6)?;
                    let snippet: String = r.get(7)?;
                    Ok(LocalHit {
                        message_id: r.get(0)?,
                        conversation_id: r.get(1)?,
                        parent_id: r.get(2)?,
                        sender_mri: r.get(3)?,
                        sender_name: r.get(4)?,
                        compose_ms: r.get(5)?,
                        snippet: fts::restore_snippet(&snippet, &html),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Replies to a channel thread root, oldest first, at most `limit`.
    pub fn thread_replies(&self, root_id: &str, limit: usize) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE parent_id = ?1 \
             ORDER BY compose_ms, id LIMIT ?2"
        ))?;
        let out = stmt
            .query_map(params![root_id, limit_i64(limit)], msg_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    pub fn reply_count(&self, root_id: &str) -> Result<usize> {
        let n: i64 = self
            .conn
            .prepare_cached("SELECT COUNT(*) FROM messages WHERE parent_id = ?1")?
            .query_row([root_id], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Newest top-level compose time in a conversation (for sync cursors).
    pub fn latest_compose_ms(&self, conversation_id: &str) -> Result<Option<i64>> {
        let v: Option<i64> = self
            .conn
            .prepare_cached(
                "SELECT MAX(compose_ms) FROM messages \
                 WHERE conversation_id = ?1 AND parent_id IS NULL",
            )?
            .query_row([conversation_id], |r| r.get(0))?;
        Ok(v)
    }

    pub fn message(&self, id: &str) -> Result<Option<MessageRow>> {
        Ok(self
            .conn
            .prepare_cached(&format!("SELECT {MSG_COLS} FROM messages WHERE id = ?1"))?
            .query_row([id], msg_from_row)
            .optional()?)
    }

    /// Id of the top-level message right before the one composed at `compose_ms`
    /// (what "Mark as unread" bookmarks), or `None` for the first message.
    pub fn message_before(&self, conversation_id: &str, compose_ms: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT id FROM messages \
                 WHERE conversation_id = ?1 AND parent_id IS NULL AND compose_ms < ?2 \
                 ORDER BY compose_ms DESC, id DESC LIMIT 1",
            )?
            .query_row(params![conversation_id, compose_ms], |r| r.get(0))
            .optional()?)
    }

    // ---- history gaps ----------------------------------------------------

    /// Records that messages composed between `older_ms` and `newer_ms` may be missing from the
    /// cache. Recording the same gap again changes nothing; an empty range is no gap.
    pub fn add_history_gap(
        &self,
        conversation_id: &str,
        newer_ms: i64,
        older_ms: i64,
    ) -> Result<()> {
        if newer_ms <= older_ms {
            return Ok(());
        }
        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO history_gaps (conversation_id, newer_ms, older_ms) \
                 VALUES (?1, ?2, ?3)",
            )?
            .execute(params![conversation_id, newer_ms, older_ms])?;
        Ok(())
    }

    /// A conversation's gaps, newest first.
    pub fn history_gaps(&self, conversation_id: &str) -> Result<Vec<HistoryGap>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT newer_ms, older_ms FROM history_gaps \
             WHERE conversation_id = ?1 ORDER BY newer_ms DESC",
        )?;
        Ok(stmt
            .query_map([conversation_id], |r| {
                Ok(HistoryGap {
                    newer_ms: r.get(0)?,
                    older_ms: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Moves the newer edge of the gap that ends at `newer_ms` down to `to_ms` (messages down to
    /// there were fetched), or removes the gap once `to_ms` reaches its older edge. A `to_ms`
    /// that is not below the current edge changes nothing.
    pub fn shrink_history_gap(
        &self,
        conversation_id: &str,
        newer_ms: i64,
        to_ms: i64,
    ) -> Result<()> {
        if to_ms >= newer_ms {
            return Ok(());
        }
        let n = self
            .conn
            .prepare_cached(
                "UPDATE OR REPLACE history_gaps SET newer_ms = ?3 \
                 WHERE conversation_id = ?1 AND newer_ms = ?2 AND older_ms < ?3",
            )?
            .execute(params![conversation_id, newer_ms, to_ms])?;
        if n == 0 {
            // Closed (or no such gap): drop it when it exists and `to_ms` is past its older edge.
            self.conn
                .prepare_cached(
                    "DELETE FROM history_gaps \
                     WHERE conversation_id = ?1 AND newer_ms = ?2 AND older_ms >= ?3",
                )?
                .execute(params![conversation_id, newer_ms, to_ms])?;
        }
        Ok(())
    }

    /// Forgets the gap that ends at `newer_ms` (it was filled, or the server has nothing in it).
    pub fn remove_history_gap(&self, conversation_id: &str, newer_ms: i64) -> Result<()> {
        self.conn
            .prepare_cached(
                "DELETE FROM history_gaps WHERE conversation_id = ?1 AND newer_ms = ?2",
            )?
            .execute(params![conversation_id, newer_ms])?;
        Ok(())
    }

    // ---- saved messages --------------------------------------------------

    /// Saves a message, replacing the row when it is already saved.
    pub fn save_message(&self, m: &SavedMessage) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO saved_messages \
                     (conversation_id, message_id, root_id, sender_name, preview, saved_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                m.conversation_id,
                m.message_id,
                m.root_id,
                m.sender_name,
                m.preview,
                m.saved_ms
            ])?;
        Ok(())
    }

    pub fn unsave_message(&self, conversation_id: &str, message_id: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "DELETE FROM saved_messages WHERE conversation_id = ?1 AND message_id = ?2",
            )?
            .execute(params![conversation_id, message_id])?;
        Ok(())
    }

    pub fn is_saved(&self, conversation_id: &str, message_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT 1 FROM saved_messages WHERE conversation_id = ?1 AND message_id = ?2",
            )?
            .exists(params![conversation_id, message_id])?)
    }

    /// Ids of the saved messages of one conversation.
    pub fn saved_ids(&self, conversation_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT message_id FROM saved_messages WHERE conversation_id = ?1 \
             ORDER BY saved_ms DESC, message_id",
        )?;
        let out = stmt
            .query_map([conversation_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Every saved message, newest first (the Saved view).
    pub fn saved_messages(&self) -> Result<Vec<SavedMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT conversation_id, message_id, root_id, sender_name, preview, saved_ms \
             FROM saved_messages ORDER BY saved_ms DESC, message_id",
        )?;
        let out = stmt
            .query_map([], |r| {
                Ok(SavedMessage {
                    conversation_id: r.get(0)?,
                    message_id: r.get(1)?,
                    root_id: r.get(2)?,
                    sender_name: r.get(3)?,
                    preview: r.get(4)?,
                    saved_ms: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    // ---- users ---------------------------------------------------------

    /// Inserts or updates a user. An existing avatar is kept when the new
    /// row has none.
    pub fn upsert_user(&self, u: &UserRow) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO users (mri, display_name, email, avatar, updated_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(mri) DO UPDATE SET \
                     display_name = excluded.display_name, email = excluded.email, \
                     avatar = COALESCE(excluded.avatar, users.avatar), \
                     updated_ms = excluded.updated_ms",
            )?
            .execute(params![
                u.mri,
                u.display_name,
                u.email,
                u.avatar,
                u.updated_ms
            ])?;
        Ok(())
    }

    /// Drops a user's stored avatar (`upsert_user` never does: it keeps the old one).
    pub fn clear_user_avatar(&self, mri: &str) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE users SET avatar = NULL WHERE mri = ?1")?
            .execute([mri])?;
        Ok(())
    }

    pub fn user(&self, mri: &str) -> Result<Option<UserRow>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT mri, display_name, email, avatar, updated_ms FROM users WHERE mri = ?1",
            )?
            .query_row([mri], user_from_row)
            .optional()?)
    }

    /// The subset of `mris` with no cached profile (no row, or an empty
    /// display name), deduplicated, in input order.
    pub fn users_missing_profile(&self, mris: &[String]) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT display_name <> '' FROM users WHERE mri = ?1")?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for mri in mris {
            if !seen.insert(mri.as_str()) {
                continue;
            }
            let has: Option<bool> = stmt.query_row([mri], |r| r.get(0)).optional()?;
            if !has.unwrap_or(false) {
                out.push(mri.clone());
            }
        }
        Ok(out)
    }

    // ---- teams / channels ---------------------------------------------

    pub fn upsert_team(&self, t: &TeamRow) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO teams (id, name) VALUES (?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            )?
            .execute(params![t.id, t.name])?;
        Ok(())
    }

    /// All teams, ordered by name.
    pub fn teams(&self) -> Result<Vec<TeamRow>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name FROM teams ORDER BY name COLLATE NOCASE, id")?;
        let out = stmt
            .query_map([], |r| {
                Ok(TeamRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Channel conversations of a team, ordered by title.
    pub fn channels(&self, team_id: &str) -> Result<Vec<ConversationRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {CONV_COLS} FROM conversations \
             WHERE kind = 'channel' AND team_id = ?1 ORDER BY title COLLATE NOCASE, id"
        ))?;
        let mut rows = stmt.query([team_id])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(conv_from_row(r)?);
        }
        Ok(out)
    }

    // ---- drafts --------------------------------------------------------

    /// Saves the unsent text (and reply target) of a conversation. Empty
    /// text deletes the draft.
    pub fn set_draft(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        self.set_draft_at(conversation_id, text, reply_to, now_ms())
    }

    fn set_draft_at(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to: Option<&str>,
        updated_ms: i64,
    ) -> Result<()> {
        if text.is_empty() {
            self.conn
                .prepare_cached("DELETE FROM drafts WHERE conversation_id = ?1")?
                .execute([conversation_id])?;
        } else {
            self.conn
                .prepare_cached(
                    "INSERT INTO drafts (conversation_id, text, reply_to, updated_ms) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(conversation_id) DO UPDATE SET \
                         text = excluded.text, reply_to = excluded.reply_to, \
                         updated_ms = excluded.updated_ms",
                )?
                .execute(params![conversation_id, text, reply_to, updated_ms])?;
        }
        Ok(())
    }

    pub fn draft(&self, conversation_id: &str) -> Result<Option<Draft>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT conversation_id, text, reply_to, updated_ms FROM drafts \
                 WHERE conversation_id = ?1",
            )?
            .query_row([conversation_id], draft_from_row)
            .optional()?)
    }

    /// All drafts, most recently edited first.
    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT conversation_id, text, reply_to, updated_ms FROM drafts \
             ORDER BY updated_ms DESC, conversation_id",
        )?;
        let out = stmt
            .query_map([], draft_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    // ---- outbox --------------------------------------------------------

    /// Queues a message for sending. Whatever state, attempts and error the
    /// item carries, it is stored as freshly `Queued` with no attempts.
    pub fn enqueue(&self, item: &OutboxItem) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO outbox (client_message_id, conversation_id, html, subject, \
                     reply_to, kind, payload_path, created_ms, attempts, state, last_error, \
                     mentions, importance, cards) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, NULL, ?10, ?11, ?12)",
            )?
            .execute(params![
                item.client_message_id,
                item.conversation_id,
                item.html,
                item.subject,
                item.reply_to,
                item.kind.as_db(),
                item.payload_path,
                item.created_ms,
                OutboxState::Queued.as_db(),
                item.mentions,
                item.importance,
                item.cards
            ])?;
        Ok(())
    }

    /// Items waiting to be sent (`Queued`), oldest first.
    pub fn outbox_due(&self) -> Result<Vec<OutboxItem>> {
        self.query_outbox(
            &format!(
                "SELECT {OUTBOX_COLS} FROM outbox WHERE state = 'queued' \
                 ORDER BY created_ms, client_message_id"
            ),
            [],
        )
    }

    /// Every queued, sending and failed item of a conversation, oldest
    /// first, for rendering their rows.
    pub fn outbox_for(&self, conversation_id: &str) -> Result<Vec<OutboxItem>> {
        self.query_outbox(
            &format!(
                "SELECT {OUTBOX_COLS} FROM outbox WHERE conversation_id = ?1 \
                 ORDER BY created_ms, client_message_id"
            ),
            [conversation_id],
        )
    }

    fn query_outbox(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<OutboxItem>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(params)?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(outbox_from_row(r)?);
        }
        Ok(out)
    }

    /// `Queued` to `Sending`. Other states are left alone.
    pub fn mark_sending(&self, client_message_id: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE outbox SET state = 'sending' \
                 WHERE client_message_id = ?1 AND state = 'queued'",
            )?
            .execute([client_message_id])?;
        Ok(())
    }

    /// `Sending` back to `Queued` after a network error: the send did not fail, it never
    /// happened, so the attempt is not counted. Other states are left alone.
    pub fn requeue(&self, client_message_id: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE outbox SET state = 'queued' \
                 WHERE client_message_id = ?1 AND state = 'sending'",
            )?
            .execute([client_message_id])?;
        Ok(())
    }

    /// Marks an item `Failed`, counts the attempt and records why.
    pub fn mark_failed(&self, client_message_id: &str, error: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE outbox SET state = 'failed', attempts = attempts + 1, last_error = ?2 \
                 WHERE client_message_id = ?1",
            )?
            .execute(params![client_message_id, error])?;
        Ok(())
    }

    /// `Failed` back to `Queued`; the attempt count and last error are kept.
    pub fn retry(&self, client_message_id: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE outbox SET state = 'queued' \
                 WHERE client_message_id = ?1 AND state = 'failed'",
            )?
            .execute([client_message_id])?;
        Ok(())
    }

    /// Drops an item: it was sent, or the user deleted it.
    pub fn remove(&self, client_message_id: &str) -> Result<()> {
        self.conn
            .prepare_cached("DELETE FROM outbox WHERE client_message_id = ?1")?
            .execute([client_message_id])?;
        Ok(())
    }

    // ---- clearing and maintenance --------------------------------------

    /// Drops every stored profile picture (clearing the media cache); names and emails stay.
    pub fn clear_avatars(&self) -> Result<()> {
        self.conn.execute("UPDATE users SET avatar = NULL", [])?;
        Ok(())
    }

    /// Drops everything that is only a copy of what the service has: the messages (and their
    /// search index), users and teams, the `kv_keys` entries of the kv table and the
    /// conversations' previews, then gives the space back (`VACUUM`).
    ///
    /// Kept, because they are the user's own or are not recoverable: the conversation rows
    /// (favorites, local mark-unread and the order of the list; their previews are blanked
    /// because they were copied from messages), drafts, the outbox, saved messages (they carry
    /// their own copy of sender and preview, no join to `messages`) and every other kv entry.
    pub fn clear_cache(&self, kv_keys: &[&str]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM messages", [])?;
        // The delete trigger emptied the index already; this keeps it consistent whatever the
        // triggers missed.
        tx.execute(
            "INSERT INTO messages_fts (messages_fts) VALUES ('rebuild')",
            [],
        )?;
        // The gaps describe holes in message history that is now gone.
        tx.execute("DELETE FROM history_gaps", [])?;
        tx.execute("DELETE FROM users", [])?;
        tx.execute("DELETE FROM teams", [])?;
        tx.execute("UPDATE conversations SET last_preview = ''", [])?;
        for key in kv_keys {
            tx.execute("DELETE FROM kv WHERE key = ?1", [key])?;
        }
        tx.commit()?;
        // Not in the transaction (`VACUUM` cannot run in one). The messages are gone, so the
        // rowids the search index is tied to cannot be renumbered under it.
        self.conn.execute_batch("VACUUM")?;
        self.checkpoint()
    }

    /// Runs the weekly upkeep (`PRAGMA optimize`, then a WAL checkpoint that also truncates the
    /// log) when a week has passed since the last time. True when it ran.
    pub fn maintain(&self) -> Result<bool> {
        self.maintain_at(now_ms(), WEEK_MS)
    }

    fn maintain_at(&self, now_ms: i64, interval_ms: i64) -> Result<bool> {
        let last = self
            .get_kv(MAINTAINED_KEY)?
            .and_then(|v| v.parse::<i64>().ok());
        if last.is_some_and(|t| now_ms - t < interval_ms) {
            return Ok(false);
        }
        self.conn.execute_batch("PRAGMA optimize")?;
        // The stamp first: written after the checkpoint it would fill the log again.
        self.set_kv(MAINTAINED_KEY, &now_ms.to_string())?;
        self.checkpoint()?;
        Ok(true)
    }

    /// Moves the write-ahead log into the database file and empties it.
    fn checkpoint(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    // ---- kv ------------------------------------------------------------

    /// Reads a small setting (epid, last sync time, ...).
    ///
    /// This table is plain, unencrypted SQLite. NEVER store tokens or other
    /// secrets here; those belong in the OS keyring.
    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM kv WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    /// Writes a small setting. NEVER use for tokens or other secrets; see
    /// [`Store::get_kv`].
    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO kv (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, value])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
