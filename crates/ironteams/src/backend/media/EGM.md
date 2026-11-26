# backend/media

## Overview
Engine message-menu extras for chat media, including a second chat next to the open one: a pop-out window or the main window's split pane (`extras.rs`). The main media engine is `../media.rs`.

Files:
- `extras.rs`: Engine message-menu extras: second chat in the pop-out window or the split pane (open/render/send), Share to Outlook draft, and Report this message (unverified, always refuses).

## Interface
- `[extras.rs:outlook_draft]` pub(super) fn(sender,place,when,subject,html:&str)->(String,String): HTML-escapes sender/place/when, falls back to "Message from {sender}" for blank subject, prefixes body with a byline paragraph
- `[extras.rs:Second]` pub(super) enum { PopOut, Split }: where a second chat shows (own window or the main window's split pane)
- `[extras.rs:Engine::second_open]` pub(super) async fn(self:&Arc<Self>, conv:&str, to:Second)->anyhow::Result<()>: opens chat in own window or the split pane; channels refused with banner; fetches first page if no local messages and online
- `[extras.rs:Engine::popout_is_open]` pub(in crate::backend) fn(&self)->bool: true when media.popout is Some
- `[extras.rs:Engine::render_popout]` pub(in crate::backend) fn(self:&Arc<Self>): pushes newest <=200 stored messages plus queued outbox rows to the pop-out window
- `[extras.rs:Engine::render_split]` pub(in crate::backend) fn(self:&Arc<Self>): the same for the split pane (media.split)
- `[extras.rs:Engine::popout_send]` pub(super) async fn(self:&Arc<Self>, conv:&str, text:&str)->anyhow::Result<()>: trims text, converts markdown to composer HTML, enqueues Text outbox item, flushes outbox
- `[extras.rs:Engine::share_to_outlook]` pub(super) async fn(self:&Arc<Self>, id:&str)->anyhow::Result<()>: creates an Outlook mail draft from a message and opens the web link; nothing is sent
- `[extras.rs:Engine::report_message]` pub(super) async fn(self:&Arc<Self>, id:&str)->anyhow::Result<()>: always returns an error (not available yet)

## Invariants
- `[extras.rs:POPOUT_ROWS]` const 200: pop-out window and split pane show at most the newest 200 messages
- `[extras.rs:Engine::second_open]` only non-Channel conversations may be popped out or split; Channel kind returns early with a banner and sets no state
- `[extras.rs:Engine::second_open]` media.popout (media.split) is set before ui.popout_open (ui.split_open) and the render; the network fetch only happens when the store has no messages and offline is false
- `[extras.rs:Engine::render_popout]` / `render_split` no-op when media.popout / media.split is None; store errors are swallowed (unwrap_or_default); build_msgs gets default MsgMarks and empty maps
- `[extras.rs:Engine::popout_send]` empty trimmed text or empty conv id is a silent no-op; sends go through the outbox (new_outbox_item/enqueue/flush_outbox(false)), not directly
- `[extras.rs:Engine::share_to_outlook]` unknown message id returns Ok(()) silently; offline shows a banner and returns Ok without calling the API
- `[extras.rs:Engine::share_to_outlook]` post subject is read from key "s" of reactions_json; missing/invalid JSON gives empty subject
- `[extras.rs:Engine::share_to_outlook]` link is only opened externally if it starts with "https://"; otherwise reports saved to Drafts folder
- `[extras.rs:report_message_unverified]` UNVERIFIED: Teams report call never captured (docs/protocol.md), so it always bails and nothing is sent
- `[extras.rs:escape]` escapes & < > " only (applied to sender, place, when, not to the message html)
- `[extras.rs:Engine::*]` store and media are std Mutexes locked briefly via lock().unwrap(); locks are not held across .await (dropped before client calls)

## Side Effects
- `[extras.rs:Engine::second_open]` sets media.popout or media.split state; calls ui.popout_open or ui.split_open and ui.banner; network client.messages page fetch; store_newest_page writes to the store (records a history gap when a full page lands above a hole, calls store_messages, then keeps the newest message time in kv key `newest_page:<conv>`; sync.rs); marks dirty messages flag
- `[extras.rs:Engine::render_popout]` / `render_split` push message models to the UI via ui.popout_messages / ui.split_messages; reads store
- `[extras.rs:Engine::popout_send]` enqueues an outbox item (store write) and triggers flush_outbox (network send)
- `[extras.rs:Engine::share_to_outlook]` network call client.create_mail_draft; opens a URL in the system browser via crate::external::open; ui.info/ui.banner notifications

## Verification
- Run: `cargo test -p ironteams`
