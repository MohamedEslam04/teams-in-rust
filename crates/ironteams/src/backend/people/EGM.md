# backend/people

## Overview
People and conversation features: `PeopleCmd` dispatch, new chat pane, add-people search, group management helpers, mention candidates and team tags, pinned messages and read receipts, with pure planning helpers in `pure.rs`.

Files:
- `commands.rs`: Engine::people_cmd dispatcher for PeopleCmd, chat-open reset/loader (also stops a running jump), member/name/PersonData helpers, receipt-poll startup (paused while the main window is hidden)
- `members.rs`: Guest/standing lookup, add-people search/pick, group_from_direct, leave group, profile card build and fill (profile_in)
- `mention_flow.rs`: @mention candidate query/picker state, team tags cache, mention pruning and edit-restore
- `model.rs`: PeopleCmd enum + parse, NewChat and people State structs, CardFacts, card fill/badge/facts helpers, CARD_FRESH
- `new_chat.rs`: New chat pane search/add/send, open_with chat planner/creator, direct row ensure, quick_message, rename, push_members
- `pins_and_receipts.rs`: Pinned messages refresh/unpin/jump+flash, history jumps (to the first message, a date, or a quoted message: `begin_nav`, `load_to_start`, `load_back`, `jump_to_first`, `jump_to_date`, `jump_to_quote`, `land_on_row`, pure `day_start`), read-receipt setting/horizons/receipt_marks, unread-mention check
- `pure.rs`: Pure (no I/O) helpers for people/chat: open-chat planning, name matching, mention re-insertion, pin sorting, read receipts (`receipt` for groups, `delivery_states` for 1:1), conv-kind check, pin preview.

## Interface
- `[model.rs:PeopleCmd]` pub enum of people UI commands (Mention*, NewChat*, Rename, Participants, Add*, Leave, Profile*, Org*, Pin*, Receipts, JumpFirst, JumpDate(String), QuoteJump(String); UI names "jump-first", "jump-date", "quote-jump")
- `[pure.rs:Plan]` pub(in backend) enum {Notes, Direct(String), Existing(String), CreateGroup(Vec<String>), Unsupported}; Debug+PartialEq; outcome of picking people in To:
- `[model.rs:PeopleCmd::parse]` pub fn parse(name:&str,arg:&str)->Option<PeopleCmd>; maps kebab-case UI command names, add-history/receipts take arg=="on"
- `[pure.rs:plan]` pub(in backend) fn plan(me:&str, picked:&[String], groups:&[(String,Vec<String>)]) -> Plan; 0 others=Notes, 1=Direct(one_on_one_id) or Unsupported, 2+=Existing/CreateGroup
- `[model.rs:State]` pub(in backend) Default struct of engine people state (members, candidates, entries, tags, seq counters, new_chat, pins, horizons, standing, profiles, org, jump/flash, `nav_seq` and `nav_loading` for history jumps)
- `[pure.rs:matches_query]` pub(in backend) fn matches_query(name:&str, query:&str) -> bool; every query word is a prefix of some alphanumeric-split word of name, case-insensitive
- `[commands.rs:Engine::people_cmd]` pub(in backend) async fn(self:&Arc<Self>, PeopleCmd)->anyhow::Result<()>; main command entry
- `[pure.rs:with_at_signs]` pub(in backend) fn with_at_signs(text:&str, mentions:&[Mention]) -> (String, Vec<Entry>); prefixes '@' to each mention's display name found in order, returns composer entries
- `[commands.rs:Engine::on_chat_opened]` pub(in backend) fn(&Arc<Self>, conv:&str); clears per-chat state, spawns members+tabs+pins load
- `[pure.rs:sort_pins]` pub(in backend) fn sort_pins(pins:&mut [teams_proto::actions::PinnedMessage]); newest first by pinned_at
- `[commands.rs:Engine::remember_members]` pub(in backend) fn(&self,&[ChatMeta]); caches aggregator member lists
- `[pure.rs:ReceiptMark]` pub(in backend) struct {msg_id, state("sent"|"seen"), names(first three +N), readers:Vec<String>}; `ReceiptMark::plain(msg_id, state)` makes one with no names
- `[pure.rs:delivery_states]` pub(in backend) fn(horizons:&[Horizon], me, mine:&[(&str,i64)]) -> Vec<&'static str>; for my messages in a 1:1 chat (oldest first): the newest one the other person has read is "seen", ones before it "", ones after it "sent"; all "sent" when none is read
- `[commands.rs:Engine::start_people]` pub(in backend) fn(&Arc<Self>); sets receipts UI, spawns receipt poll loop unless offline
- `[pure.rs:receipt]` pub(in backend) fn receipt(horizons:&[Horizon], me, msg_id, msg_ms:i64, one_on_one:bool, name_of:impl Fn(&str)->String) -> (state, line, readers)
- `[commands.rs:Engine::members_known]` pub(in backend) fn(&self,conv)->Option<Vec<String>>; 1:1 derived from id, else cached
- `[pure.rs:is_chat]` pub(super) fn is_chat(kind:ConvKind) -> bool; true for OneOnOne, Group, Meeting
- `[commands.rs:Engine::push_group_header]` pub(in backend) fn(&self,conv); shows rename/participants controls and count
- `[pure.rs:pin_preview]` pub(super) fn pin_preview(r:&MessageRow) -> String; plain text of content_html, newlines to spaces, first 100 chars
- `[commands.rs:Engine::ensure_members]` pub(super) async; returns members, fetching Group roster via client.thread_members if unknown
- `[commands.rs:Engine::resolve_names]` pub(in backend) async; fetches+stores profiles for MRIs lacking a name
- `[commands.rs:Engine::person_data/member_data]` pub(super) PersonData builders for UI rows
- `[commands.rs:Engine::conv_kind]` pub(in backend) fn(&self, conv) -> Option<ConvKind>; store kind
- `[commands.rs:Engine::current_chat/name_of]` pub(super) small lookups (view.current, display name or "Someone")
- `[members.rs:Engine::standing_of]` pub(in backend) fn(&self,mri)->Option<Standing>
- `[members.rs:Engine::resolve_standing]` pub(in backend) async fn(&self,&[String])->bool; guest lookup once per session per orgid MRI
- `[members.rs:Engine::want_standing]` pub(in backend) fn(&Arc<Self>,Vec<String>); spawns lookup, marks chats+header dirty on guest
- `[members.rs:Engine::profile_in]` pub(in backend) async fn(&Arc<Self>,mri,modal:bool); builds and fills profile card/modal
- `[members.rs:Engine::add_query/add_pick/group_from_direct/leave/profile/card_photo]` pub(super) add-people, leave and card flows
- `[mention_flow.rs:Engine::prune_mentions]` pub(in backend) fn(&self,text); drops picked entries when none remain in text
- `[mention_flow.rs:Engine::restore_edit_mentions]` pub(in backend) fn(&self,&MessageRow,String)->String; restores entries and adds @ signs for an edit
- `[mention_flow.rs:Engine::mention_query/set_candidates/team_tags/tag_subtitle/mention_pick]` pub(super) mention picker flow
- `[new_chat.rs:Engine::open_with]` pub(in backend) async fn(&Arc<Self>,&[String],group_name:&str,create:bool)->Result<String>; open or create chat via plan()
- `[new_chat.rs:Engine::push_new_chat/groups/new_chat_query/new_chat_add/new_chat_send/quick_message/rename/push_members/ensure_direct_row]` pub(super) New chat and group management
- `[new_chat.rs:Engine::new_chat_seq_for_test]` cfg(test) only; excluded
- `[pins_and_receipts.rs:Engine::refresh_pins]` pub(in backend) async fn(&Arc<Self>,conv); fetch/resolve pins and push to UI
- `[pins_and_receipts.rs:Engine::is_pinned]` pub(in backend) fn(&self,conv,id)->bool
- `[pins_and_receipts.rs:Engine::jump_to_with/flash_row/clear_flash/take_jump]` pub(in backend) jump+highlight API; take_jump(&mut [MsgData])->Option<usize>
- `[pins_and_receipts.rs:Engine::receipts_enabled/receipt_marks/has_unread_mention]` pub(in backend) receipt setting, `receipt_marks(conv, rows) -> Vec<ReceiptMark>` (a group: one mark for my newest message; a 1:1 chat: a mark for each sent message of mine whose `delivery_states` entry is non-empty, built with `ReceiptMark::plain`), unread-mention check (asks the store for `mention_candidates(conv, read_horizon_ms, me, 30)` instead of parsing full messages)
- `[pins_and_receipts.rs:Engine::refresh_horizons]` pub(in backend) async fn(&Arc<Self>, conv); also used from live.rs
- `[pins_and_receipts.rs:Engine::load_until_found/unpin/jump_to/set_receipts/receipts_apply]` pub(super) helpers; `load_until_found` returns at once when offline and pages by `oldest_compose_ms`
- `[pins_and_receipts.rs:Engine::begin_nav/load_to_start/jump_to_first/jump_to_date/jump_to_quote]` pub(super) history-jump API; `load_back`, `nav_live`, `nav_pill`, `end_nav`, `first_row_from`, `land_from`, `land_on_row`, `jump_if_found` are private; `day_start(text, zone, now_ms)` reads `YYYY-MM-DD` or `days:N` in the Settings zone

## Invariants
- `[model.rs:State.seq/mention_seq/new_chat_seq]` separate sequence counters; a slow answer whose seq changed is dropped (add_query, mention_query, new_chat_query); open/close/add bump new_chat_seq
- `[pure.rs:plan]` self (`me`) is filtered out of picked before counting, so picking only yourself yields Plan::Notes
- `[model.rs:State.members]` member MRIs by chat include ourselves
- `[pure.rs:plan]` 1:1 id needs directory oids for both parties (oid_of_mri); a guest without one gives Plan::Unsupported
- `[model.rs:CARD_FRESH]` profile details older than 15 minutes are re-read; presence always re-read
- `[pure.rs:plan]` group matching compares rosters as BTreeSet excluding me; an existing group with exactly that roster is reused, never duplicated; CreateGroup members exclude me and are sorted
- `[members.rs:resolve_standing]` standing_asked set ensures one lookup per orgid MRI per session; excludes self and non-8:orgid: MRIs
- `[pure.rs:Plan::Direct]` the 1:1 chat id is deterministic and the chat only exists after the first message is sent
- `[members.rs:profile_in]` card_mri checked after async fetches; if another card opened, final card_details push skipped
- `[pure.rs:with_at_signs]` mentions are located in order via a moving cursor `at`; empty display names and names not found after the cursor are skipped (no entry emitted)
- `[members.rs:add_pick]` 1:1 chats take no members: delegates to group_from_direct; history flag selects from=0 vs now_ms()
- `[pure.rs:sort_pins]` relies on pinned_at being an ISO timestamp so lexicographic order equals time order
- `[commands.rs:on_chat_opened]` clears entries/candidates/add_results and closes card/participants before loading new chat; members, tabs, pins loaded concurrently via tokio::join!
- `[pure.rs:receipt]` 'sent' with empty line/readers until seen_by is non-empty; for one_on_one the state is 'seen' with an empty line and empty readers (only tests exercise that: `receipt_marks` calls `receipt(..., false, ..)` for groups and `delivery_states` for 1:1)
- `[model.rs:State.nav_seq]` `begin_nav` bumps it; a jump whose number is no longer the latest, or whose chat is no longer open, stops (`nav_live`); `on_chat_opened` calls `begin_nav()` to stop a running jump; jump-to-first has no page limit
- `[new_chat.rs:open_with]` CreateGroup only when create==true, else bails; group created at first send/Chat press
- `[pure.rs:receipt]` the line lists the first three reader names plus ' +N' for the rest; the readers vec holds all names
- `[new_chat.rs:new_chat_send]` no-op when To: empty or text blank; resets new_chat before enqueue
- `[pure.rs:matches_query]` an empty/whitespace query matches everything (all() over no words)
- `[pins_and_receipts.rs:receipts_enabled]` enabled only if local KV RECEIPTS_KEY != "off" and server value (default true)
- `[pure.rs:pin_preview]` truncation is by chars (100), not bytes
- `[pins_and_receipts.rs:receipts_apply]` receipts only for 1:1 or Group with members <= RECEIPT_MAX_MEMBERS
- `[pins_and_receipts.rs:take_jump]` jump is consumed once (take); flash cleared by 3s timer or clear_flash, jump_to_with(fade=false) keeps flash

## Side Effects
- `[commands.rs:start_people]` spawns tokio task: read_receipts_enabled fetch then a loop waiting on `e.visible_tick(RECEIPT_POLL)` (20 s; paused while the main window is hidden, reads at once on show) and calling refresh_horizons
- `[commands.rs:on_chat_opened]` spawns tokio task loading members, channel tabs, pins; network calls
- `[commands.rs:people_cmd ProfileCopyEmail]` writes email to system clipboard via arboard and shows banner
- `[commands.rs:resolve_names]` network profiles fetch; upserts user rows into store
- `[members.rs:add_pick/leave]` network add_member/leave_thread; store delete_conversation/upsert_user
- `[members.rs:profile_in]` network profile_card, directory_details, get_presence, schedule/org reads; applies presence
- `[new_chat.rs:open_with]` network create_group_chat; upsert conversation row; opens chat
- `[new_chat.rs:quick_message/new_chat_send]` enqueue outbox item and flush_outbox (network send)
- `[new_chat.rs:rename]` network rename_thread; store upsert_conversation
- `[pins_and_receipts.rs:load_back]` network `client.messages` pages back for a jump, storing messages and driving the loading line (`ui.loading_older`, `ui.loading_progress`)
- `[pins_and_receipts.rs:set_receipts/jump_to_with/refresh_pins]` writes KV RECEIPTS_KEY; spawns 3s flash timer; network pinned/set_pinned/messages fetch storing messages

## Verification
- Run: `cargo test -p ironteams`
