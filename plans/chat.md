# kelma: plan to Teams chat parity

The MVP in `PLAN.md` is done. This plan covers everything Teams chat does that kelma does not. It is one of the parity plans indexed in `plans/README.md`.

**Source.** A survey of the real Teams web app (`teams.cloud.microsoft`, 2026-11-18). It covered every
menu, hover bar, toolbar, dialog, settings page and the keyboard shortcut list (Ctrl+.). Each item
says where it was seen, so the expected behavior can be checked against Teams.

**Out of scope.** Calls, meetings, Calendar, Copilot and suggested replies, Loop, Notes, Storyline,
Recap, Store, app extensions (Planner, Jira, Polly, Workflows), and team or channel admin. kelma
opens those in the browser.

## How to read an item

| Field | Meaning |
|---|---|
| Impact | 1–5, value for daily use |
| Effort | 1–5, where 5 is large or risky |
| Seen | where in Teams the behavior was observed |
| What | the behavior to build, matching Teams |
| How | the endpoint and the kelma pieces. **known**: the endpoint is in purple-teams or already used by kelma. **discover**: capture it from the Teams web client first (DevTools, then sanitize) and document it in `docs/protocol.md` |
| Done when | the acceptance check |

Every item also needs:
- tests: unit tests for protocol and parsing, and UI tests in `crates/kelma/src/ui_tests.rs` for UI behavior
- a live check against the real account: post only in `48:notes` or a private test channel the user named, and delete the test messages afterwards

Checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo build --workspace --release`.

---

## Phase 5: everyday messaging

### 5.1 Paste and drag-drop
- [x] Impact 5, effort 2. Seen: composer (Ctrl+V, drop zone).
  - **What.**
    - Ctrl+V with an image on the clipboard attaches it as an inline image, with a preview chip above the composer before sending.
    - Dropping files on the chat attaches them: images inline, other files as file cards.
    - Several attachments can wait in the chip row, and each has ✕ to remove it.
  - **How.**
    - Read the clipboard image with `arboard::Clipboard::get_image` and encode it as PNG.
    - Get drops from winit `WindowEvent::DroppedFile` through the `unstable-winit-030` hook. Spike this first; if Slint swallows the event, ship paste only and leave drop as BLOCKED with the reason.
    - Upload with the existing `upload_image` and `send_file`.
    - Add a `pending-attachments` model in the UI.
  - **Done when.**
    - A pasted screenshot shows in 48:notes as an image.
    - A dropped PDF shows as a file card.
    - A UI test covers the chip row and removing an item.

### 5.2 Hover bar like Teams
- [x] Impact 5, effort 2. Seen: message hover.
  - **What.**
    - The bar reads 👍 ❤️ 😆 😮, then "More reactions", "Reply with quote" (❝), Edit (own messages only), "…".
    - More reactions opens a searchable picker with every Teams reaction key.
    - Hovering a reaction chip shows who reacted.
  - **How.**
    - Restyle `MessageRow` in `app.slint`.
    - Map reaction keys (`like`, `heart`, `laugh`, `surprised`, `sad`, `angry`, plus codepoint keys such as `1f525_fire`). `reaction_emoji` already decodes them.
    - Names for the tooltip come from `emotions[].users[].mri`, resolved through the profile cache.
  - **Done when.**
    - A UI test checks the order of the bar and the picker search.
    - The tooltip lists names.

### 5.3 Message "…" menu
- [x] Impact 5, effort 3. Seen: message menu, own and others' messages.
  - **What.** One menu, like Teams:
    - Reply with quote
    - Forward, with Forward options: this message or multiple messages
    - Copy message link
    - Save this message (others' messages)
    - Pin for everyone
    - Mark as unread
    - Translation
    - Delete (own messages)
    - Edit (own messages, also in the bar)

    Right click on a message opens the same menu.
  - **How.**
    - **Forward** (discover): Teams sends a new message with the original embedded as a forwarded quote. The dialog has "Add recipients" (people, chats, channels), "Add a message (optional)" and a preview of the quoted message, then Forward. Recipient search reuses 6.2.
    - **Copy link** (known shape): `https://teams.microsoft.com/l/message/{conv}/{msgId}?context=…`, copied with arboard.
    - **Save** (discover): a message property; it also feeds a "Saved" view (9.8).
    - **Pin** (discover): a thread property. It also feeds the pinned banner (6.6).
    - **Mark as unread** (known): set `consumptionhorizon` to the message before this one, and set the chat's unread flag in the store.
    - **Translation**: 9.6.
    - **Multiple messages**: a checkbox mode in the list, then forward them together.
  - **Done when.** Each action works live in 48:notes or the test channel, and pinning shows in Teams web.

### 5.4 Composer formatting
- [x] Impact 4, effort 4. Seen: composer toolbar, overflow, shortcuts.
  - **What.**
    - Toolbar: Bold, Italic, Underline, Strikethrough, bullet and numbered lists, text highlight, font color, font size, Quote, Insert link, Code block.
    - Overflow: Paragraph and headings, inline Code, Clear formatting, indent and outdent, horizontal rule, table, undo and redo.
    - Shortcuts: Ctrl+B, Ctrl+I, Ctrl+U, Ctrl+Alt+X (strike), Ctrl+K (link), Ctrl+Alt+Shift+C (inline code), Ctrl+Alt+Shift+B (code block), Ctrl+Alt+1/2/3 (headings), Ctrl+Alt+P (paragraph), Ctrl+Alt+4 (quote), Ctrl+Shift+7/8 (lists), Ctrl+Z, Ctrl+Y.
    - "Show formatting options" expands the composer, like Teams.
  - **How.**
    - Slint's `TextInput` is plain text, so the composer edits **Markdown-style source with a live preview strip**. Toolbar buttons and shortcuts wrap the selection in markers.
    - On send, `render::markdown_to_teams_html` (new) converts the source to Teams HTML: `<b> <i> <u> <s> <ul> <ol> <blockquote> <pre> <code> <h1–3> <table> <span style="color|background|font-size">`.
    - Write an ADR that records why it is not WYSIWYG.
  - **Done when.**
    - Every toolbar action round-trips: the sent HTML renders correctly in Teams web, and kelma's renderer renders it identically.
    - Unit tests cover each construct.
  - **Evidence (UI, 2026-11-19).** The composer edits Markdown-style source (ADR `docs/adr/0010-composer-markdown-source.md`, why it is not WYSIWYG). `crates/render/src/edit.rs` (pure, 37 unit tests): `apply(action, text, anchor, cursor)` for bold, italic, underline, strike, inline code, `color:`/`bg:`/`size:` spans, `h1`-`h3`, paragraph, quote, bullet and numbered lists, indent and outdent, code block with a language (toggles and changes language), link (address validated, only http, https, mailto), table (`table:RxC`), rule and clear formatting, each tested together with the HTML it produces; `shortcut()` has the Teams chords (Ctrl+B/I/U, Ctrl+Alt+X, Ctrl+K, Ctrl+Alt+Shift+C and B, Ctrl+Alt+1/2/3/P/4, Ctrl+Shift+7/8, Ctrl+Z/Y); `History` is undo and redo for toolbar edits (the input forgets its own stack when text is set from code). `crates/kelma/ui/composer_format.slint` is the toolbar (B I U S, font color, highlight with Teams' 8-color palette, size, lists, quote, link dialog, code block language menu, an overflow menu with paragraph and headings, inline code, clear, indent, rule, table, undo, redo), the preview strip and the "Aa" Show formatting options button; `src/composer_format.rs` is its Rust side. Send goes through `render::markdown_to_composer_html` for new messages, replies (`reply_quote_html` plus the body) and edits, so the outbox is unchanged; plain text converts to the same HTML as before (tested). Incoming: `markdown::upgrade_rich` draws headings, tables, colors, highlights and underline for messages that have them; Slint's `StyledText` has no sizes, tables or backgrounds, so a heading is a bold line, a table aligned monospace rows and a highlight a text color. Live in 48:notes (`cargo test -p kelma live_format -- --ignored --nocapture`, messages deleted again): one message per construct (inline styles, color/highlight/size, h1-h3, quote, nested list, ordered list, rust code block, table, link) was sent through the real send path and read back, and the server's HTML decodes to exactly the model that was sent (it adds `<thead>` to tables and turns the highlight into the `marigold` palette entry). Not done: looking at the messages in Teams web (needs a person); editing a message that was sent with formatting shows its plain text, not its source. Tests: `cargo test -p render edit`, `cargo test -p kelma ui_tests::format`.
  - **Earlier evidence (data layer).** `crates/render/src/rich/` and `crates/render/tests/rich_format.rs`. `parse_rich_html` decodes into a richer model than `RichText` (`RichDoc`: headings 1-3 with `h4`-`h6` folded to 3, nested lists with `<ol start>`, code blocks with language, quotes, rules, tables with header cells and `colspan`/`rowspan`, plus `Style.color`, `highlight`, `size` next to bold, italic, underline, strike, code, links, mentions and emoji); `RichDoc::to_html` writes what the Teams web composer sends; `markdown_to_rich` and `markdown_to_composer_html` take the Markdown-style source (the existing `markdown_to_teams_html` syntax plus a language after the code fence) through the same model. The old `RichText` API is unchanged. Real shapes from the Teams web bundle (CKEditor 5): `strong`, `i`, `u`, `s`, `code`; `h1`-`h3` (`h4`-`h6` upcast to heading 3); code block `<pre class="language-<lang> skipProofing" spellcheck="false"><code>` with `<br>` between lines (a stored message echoes `class="language-plaintext" itemid="…"`, no `skipProofing`); table `<figure class="table"><table class="copy-paste-table">`, an empty cell gets `&nbsp;` (the `itemprop` form exists only for Kusto tables); text color `<span style="color:<css>">` and font size `x-large`/`xx-small`/`inherit`, both passed through from the editor; highlight from a fixed 8-color palette as `<span itemtype="http://schema.skype.com/HighlightColor" itemid="<slug>" style="background-color:#RRGGBB">` (any other color is a plain `background-color` span). Lists, quotes and `ol start` follow rusty-teams `draft.rs` (MIT). Read-only against 48:notes (tag shapes only, nothing stored): links carry `target`, `rel`, `title`, `itemtype=…/HyperLink`; code blocks use `<br>`. Colors are validated (`#rgb`, `#rrggbb`, `rgb()`, `rgba()` only), links to http, https, mailto and tel only, and the encoder re-validates the model, so script, event attributes, unknown styles and `javascript:` do not survive. Not run live: nothing was sent, so how Teams web renders each encoded construct is still unchecked. Still to do: the toolbar and shortcuts, the live preview strip, and the ADR (this box stays unticked until then). Tests: `cargo test -p render`.

### 5.5 Emoji like Teams
- [x] Impact 4, effort 2. Seen: emoji panel.
  - **What.**
    - The panel has an Emoji tab with categories, recent emoji and search ("Find something fun").
    - Typing `:name:` autocompletes, for example `:heart:` or `:like:`. A popup appears after `:` plus two letters.
    - A skin tone preference.
    - GIFs and stickers are out (see Later).
  - **How.**
    - Generate a static emoji table (name, keywords, category) from Unicode CLDR data at build time, with no runtime network.
    - Store recent emoji and the skin tone in config.
  - **Done when.** UI tests cover search, `:heart` autocomplete and inserting at the cursor.

  - **Evidence (GIFs and named reactions, 2026-11-19).** The composer has a GIF button: trending GIFs, a search box
    and a three-column grid; a pick is sent as Teams sends a GIF (`<img itemtype="http://schema.skype.com/Giphy">`,
    `teams_proto::gifs`, search through the middle tier's Giphy extension; live, read only: `kelma-cli gifs happy`
    returned 27 results). UI test `the_gif_button_shows_trending_gifs_and_a_pick_is_sent`. Reactions Teams sends by
    their Skype name (`loudlycrying`, `cwl`, `yes`, …) now show as emoji (`markdown::reaction_emoji`, test
    `named_teams_reactions_become_emoji`). Stickers stay out.
  - **Evidence (composer gaps, 2026-11-20).** Spell check: misspelled words get a red underline once typing pauses
    (500 ms) and a right click offers up to five suggestions and "Add to dictionary" (`src/spell.rs`,
    `ui/spelling.slint`). Slint's TextInput has no spell-check hook, so the words are checked in Rust (`spellbook`,
    the system's Hunspell English and Arabic dictionaries, none installed means nothing is flagged) and each word's
    position comes from a hidden, disabled copy of the input whose cursor is moved to the word's ends
    (`cursor-position-changed`). Settings › General › Writing › "Check spelling" turns it off. Added words are kept in
    `spelling-words.txt` in the config dir. The composer has one Attach button with "Attach cloud files" and "Upload
    from this device"; the "+" menu has "Schedule a meeting" (the calendar form with the chat's members) and "Meet now"
    in chats; the GIF picker has a Stickers tab (Teams' sticker catalogue, see docs/protocol.md); the @ picker lists a
    channel's team tags. Tests: `cargo test -p kelma --bin kelma spell composer_gaps stickers`.
### 5.6 Unread handling
- [x] Impact 4, effort 2. Seen: settings › Chats ("When I open a chat, take me to"), chat menu, shortcuts.
  - **What.**
    - A "New messages" divider sits at the read marker.
    - On opening a chat, jump to the last read message or the newest one (a setting).
    - A floating "↓ Jump to newest" button appears when scrolled up, with the count of new messages.
    - Mark all as read: Shift+Esc, and an item in the chat list "…".
    - Mark as unread from the chat item menu.
  - **How.**
    - The read marker comes from the conversation `consumptionhorizon`, which the store already reads.
    - Mark all as read PUTs the horizon on every unread conversation, throttled.
  - **Done when.** A UI test covers the divider position and the jump button; the live check marks all read and Teams web agrees.
  - **Status.** Built and unit/UI tested. The setting is `open_chat_at` in `config.json` (`first_unread`, the default, or `newest`); there is no settings page for it yet (9.x). A manual "Mark as unread" is remembered locally (`kv unread_mark:<conv>`) because the conversations list does not return the bookmark. Not run live: Mark all as read (it changes the real read state of every chat), so "Teams web agrees" is still open.

### 5.7 Drafts
- [x] Impact 4, effort 2. Seen: quick view "Drafts".
  - **What.**
    - Each chat keeps its unsent text (and reply target) when you switch chats or restart.
    - The chat list shows "Draft:" in the preview.
    - A "Drafts" quick view lists them.
  - **How.**
    - Store the draft in the store's `kv` as `draft:<conv>`, saved with a debounce from `edited`.
    - Teams syncs drafts server-side. Only discover that if it turns out to be cheap; local is enough.
  - **Done when.** A UI test types, switches chat and comes back, and the draft is still there.

### 5.8 Failed sends and offline queue
- [x] Impact 4, effort 2.
  - **What.**
    - Optimistic messages that fail show "Failed to send · Retry · Delete".
    - While disconnected, sends wait in a queue and go out when the connection comes back.
  - **How.**
    - Add a `pending` state on the row (it exists), a failure state, and a retry command.
    - The queue lives in the store so it survives a restart.
  - **Done when.** Freezing the network (as in the reconnect test) and sending queues the message, and it goes out on reconnect.
  - **Status.** Every new message (text, image, file) goes through the store's outbox; files are copied to `<cache>/outbox/<id>/` and removed once sent. A network error keeps the item queued (flushed in order on Connected, Resync, startup and every 30 s); a refusal shows "Failed to send · Retry · Delete". Teams does not deduplicate a repeated `clientmessageid`, so flushes after a restart or a lost answer read the newest history first and drop items whose echo is already there. Checked live in `48:notes` (`cargo test -p kelma live_outbox -- --ignored`); the offline path is covered by unit and engine tests, not by freezing the real network.

### 5.9 Keyboard shortcuts like Teams
- [x] Impact 4, effort 2. Seen: the Ctrl+. dialog.
  - **What.**

    | Shortcut | Action |
    |---|---|
    | Ctrl+. | shortcut list dialog |
    | Ctrl+Alt+E | search |
    | Ctrl+Alt+G | go to chat or channel |
    | Alt+Shift+N | new chat |
    | Ctrl+Shift+F | open filter |
    | Ctrl+Alt+U | Unread filter |
    | Ctrl+Alt+C | Chats filter |
    | Ctrl+Alt+A | Channels filter |
    | Ctrl+Alt+Z | clear filters |
    | Alt+Shift+R | focus the compose box |
    | Alt+Shift+M | focus the message pane |
    | Shift+Esc | mark all as read |
    | Alt+Q | collapse all sections |
    | Ctrl+Shift+X | expand the composer |
    | Alt+Shift+O | attach a file |
    | Ctrl+Shift+I | mark important |
    | ↑ in an empty composer | edit your last message |
    | Esc | close the thread, popup or reply |

    In the message menu: R for reply, F for forward, S for save, P for pin, U for unread.
  - **How.** Add a window-level `FocusScope` in `AppWindow` that routes keys to commands.
  - **Done when.** A UI test covers each shortcut's effect.
  - **Status (2026-11-19).** Done, with a UI test each (`cargo test -p kelma ui_tests::shortcuts`, `shortcuts::`): Ctrl+. (the dialog, `shortcuts.slint`; Esc or Ctrl+. closes it), Ctrl+Alt+G (Search page on the Chats tab), Ctrl+Shift+F (the chat list's filter box), Ctrl+Alt+U, C, A (toggle the filter like the pill) and Z (clear), Alt+Shift+R (compose box), Alt+Shift+M (message pane: arrows, Page Up and Down, Home, End scroll it, Esc gives the focus back), Ctrl+Shift+X (taller composer), Alt+Shift+O (attach a file), Up in an empty composer (edit my last message), Esc (the dialog, the open message menu, the thread, then the reply), and R, F, S, P, U in the message menu. Ctrl+Alt+E, Alt+Shift+N, Shift+Esc and Alt+Q are untouched (Shift+Esc and Ctrl+Alt+E are re-tested). The mapping is in `src/shortcuts.rs`, the routing in `app.slint`. Ctrl+Shift+I (mark important) is done too: it toggles "Mark as important" for the next message, shown as a red IMPORTANT! chip in the composer (also in the formatting overflow menu "Mark as important"; the chip's ✕ removes it; the send uses it up). Store migration v5 adds `outbox.importance TEXT NOT NULL DEFAULT ''` (`OutboxItem.importance`, kept through failure, retry and requeue, old rows read as standard); a message with it set is sent with `teams_proto::delivery` (`outgoing_delivery_body`, `Client::send_outgoing_delivery_as`, `apply_delivery` Important) under the same client message id on every retry, with its mentions and quote reply; the pending and incoming rows show a red IMPORTANT! label (URGENT! for urgent) from `Message.importance`. Live in 48:notes (`cargo test -p kelma live_important -- --ignored --nocapture`, messages deleted again): one message sent through the real send path and outbox with the flag read back as `Important` (`high`), the next message without it read back as `Standard`. Tests: `cargo test -p store importance`, `cargo test -p teams-proto delivery`, `cargo test -p kelma ui_tests::format` (chip, overflow entry, flag used up by one send, outbox row, labels).

### 5.10 Enter behavior setting
- [x] Impact 2, effort 1. Seen: settings › Chats ("When writing a message, press Enter to").
  - **What.** "Send the message" (default), or "Start a new line", where Ctrl+Enter sends.
  - **Done when.** A UI test covers both modes.

### 5.11 What a message shows and offers
- [x] Impact 3, effort 3. Seen: Teams chat (code blocks, mentions, message menu, drop zone).
  - **What.**
    - Code in a message is a box of its own (monospace, `Theme.code-bg`, wraps long lines) with a Copy button that shows while the pointer is over it.
    - A mention is bold text in `Theme.mention`.
    - The message menu has "Copy text" (the message's plain text).
    - While a file is dragged over the window, a chat that takes attachments shows a dashed frame and "Drop to attach".
  - **How.**
    - `markdown::Rendered.segments` splits a body into text and code (empty when there is no code block; `markdown` stays the whole body). `MsgData.segments` becomes `Msg.parts` (`BodyPart`); a row with parts draws each text run as `StyledText` and each block as `molecules/code_block.slint`. The row fingerprint hashes `segments`.
    - The mention color is the palette's `mention`, set into the markdown path by `ui/settings/push.rs` (`markdown::set_mention_color`); `StyledText` has no background span, so it is a colored bold, not a chip. The memo version of a row includes the color.
    - `Copy text` and the block's Copy button both send `MediaStore.command("text-copy", text)` (`MediaCmd::CopyText`, banner "Text copied").
    - `src/file_hover.rs` turns winit's `HoveredFile`, `HoveredFileCancelled` and `DroppedFile` into `AppWindow.file-hover`; `app.slint` shows `molecules/drop_zone.slint` when `accepts-files` (a chat or channel with its composer showing).
  - **Done when.** UI tests cover each part.
  - **Status.** Written without a local build (CI is the compile gate). Tests: `cargo test -p ironteams markdown::` (segments, mention color), `ui_tests::message_ui` (block in its row, taller with more lines, Copy button, "Copy text" and where it hides, drop zone), `ui::row_fingerprint`, `backend::media` (`text-copy`), `backend::settings` (code box color). Not done: a language label on the block (`Block::Code` carries no language), selecting text inside a block (use Copy), a mention chip with a background and a stronger look for a mention of me per mention (the bubble of a message that mentions me is already tinted), a keyboard shortcut for Copy text (the menu's letter keys are Teams' own, not one per entry), the drop zone on Wayland (winit 0.30 sends no drag events) and the OneDrive page (a drop uploads there).

---

## Phase 6: people and conversations

### 6.1 @mentions
- [x] Impact 5, effort 3. Seen: messages, Activity, settings › Notifications (personal, Everyone, channel and team, tag).
  - **What.**
    - Typing `@` opens a picker:
      - group chats: members
      - 1:1: the other person
      - channels: org people plus "team" and "channel" entries
    - Choosing a person inserts a chip-like name token that Backspace removes as one unit.
    - @Everyone in group chats.
    - Incoming mentions of you are highlighted, and the chat shows an @ marker in the list.
  - **How (known).** Send `<span itemscope itemtype="http://schema.skype.com/Mention" itemid="N">Name</span>` plus a `properties.mentions` JSON string: `[{"@type":"http://schema.skype.com/Mention","itemid":N,"mri":"8:orgid:…","mentionType":"person","displayName":"Name"}]`. Team and channel mentions use `mentionType: "team"` or `"channel"` (verify).
  - **Done when.** A mention sent from kelma notifies the person in Teams, which is verified by mentioning yourself in a test channel.
  - **Status.** Built and tested. `@` in the composer opens a picker (group: members and Everyone; 1:1: the other person; channel: channel, team and org people; notes: you). A picked name is plain `@Name` text that one Backspace removes whole; the engine keeps who each token is and turns them into the mention spans and `properties.mentions` when sent. Mentions go through the store outbox (migration v4, `outbox.mentions`) and an edit resends them. Incoming mentions of you (or `@Everyone`) tint the bubble and put `@` in the chat list. Live: a self-mention sent through the outbox to the sandbox channel came back parsed as a mention of me and one new Mention item reached the Activity feed (`cargo test -p kelma live_people_mention -- --ignored`, message deleted afterwards). The chip is text, not a drawn pill: Slint's `TextInput` cannot style a span. A restored draft or a new chat loses the picked mentions, so they go out as plain text.

### 6.2 New chat
- [ ] DEFERRED: needs test tenant (the remaining live checks message a colleague's 1:1; built and tested offline). Impact 5, effort 3. Seen: Alt+Shift+N, "New message".
  - **What.**
    - The "To:" field searches people, chats, channels, emails and tags, with suggestions as you type.
    - Adding several people creates a group chat, with "Add group name".
    - If a chat with exactly those people exists, open it instead.
    - The thread is created when the first message is sent.
  - **How (known).**
    - People search: `GET <middleTier>/beta/users/searchV2?includeDLs=true&includeBots=true&enableGuest=true&source=newChat&skypeTeamsInfo=true` with the query.
    - Create: `POST <chatService>/v1/threads` with `{members:[{id,role}], properties:{threadType:"chat", topic?}}`. The new id is in the `Location` header.
    - A 1:1 uses the deterministic id `19:<guidA>_<guidB>@unq.gbl.spaces` and needs no create.
  - **Done when.** Starting a 1:1 with yourself and a group with one colleague works. Only do the group test with that colleague's OK; otherwise test it with the user only.
  - **Status.** Built, not ticked. Alt+Shift+N or the compose button opens the pane: To: suggestions (people search), chips, "Add group name" from two people, and a hint when a group with exactly that roster exists (it is opened instead of creating one). One person gives the deterministic 1:1 id, you alone gives `48:notes`, and the thread is created when the first message is sent. Tested offline for existing roster, new roster, 1:1 and notes. Live, with the one allowed test group, only `create_group_chat` with one colleague was run; sending to a never-used 1:1 id (which would message a colleague) and the UI path for a new 2+ person group were not. Matching the roster needs the members of every group, which come from the aggregator snapshot at sync.

### 6.3 Group chat management
- [ ] DEFERRED: needs test tenant (Add people would add a real person to a chat; built and tested offline, group photo endpoint unknown). Impact 4, effort 3. Seen: chat header (pencil, photo), the participants button ("20").
  - **What.**
    - Rename with the pencil.
    - Change the group photo.
    - The participants popover lists people with presence and work location, and has "Add people" (optionally with chat history) and "Leave".
    - The chat item menu also has Leave.
  - **How.**
    - Rename (known): `PUT /v1/threads/{id}/properties?name=topic`.
    - Add (known): `PUT /v1/threads/{id}/members/{mri}` with `{role:"User"}`.
    - Leave (known): `DELETE /v1/threads/{id}/members/{selfMri}`.
    - Group photo (discover): probably an AMS upload plus a thread property.
  - **Done when.** Rename and add/leave work on a test group created with the user only.
  - **Status.** Built, not ticked. Pencil renames in place; the people button opens the participants popover (members, Add people with an "Include chat history" switch, Leave with a confirmation); Leave is also in the chat item menu of groups. Live in the one test group: rename and leave worked. Add people was not run live (it would add a person to a chat). The group photo is not built: no endpoint is known, it needs a capture from Teams web first.

### 6.4 Presence
- [ ] DEFERRED: needs test tenant (the Done-when compares dots with Teams web for 5 people; dots now show in the chat list and on profile cards, see Status; the participants list and people picker do not show them yet). Impact 5, effort 3. Seen: profile menu, participants list, avatars everywhere.
  - **What.**
    - Status dots on every avatar: green Available, red Busy or Do not disturb, yellow Away or Be right back, grey Offline, with an out-of-office marker.
    - The profile menu changes your status: Available, Busy, Do not disturb, Be right back, Appear away, Appear offline, Reset status.
    - "Set status message", with an expiry.
    - "Pause notifications".
    - Automatic Away when idle.
    - "Notify me when this person is available" (settings › Presence).
  - **How (known).**
    - Read: `POST <presence service>/presence/getpresence/` with `[{mri}]`, where the presence service is the regional `regionGtms.presenceUPS` + `/v1` (for example `teams.microsoft.com/ups/noam/v1`), batched over visible people, plus presence change events from trouter (already registered).
    - Set: `PUT /v1/me/forceavailability/` with `{availability}`.
    - Note: `PUT /v1/me/publishnote`.
    - Idle: `POST /v1/me/reportmyactivity/`.
  - **Done when.** Dots match Teams web for 5 people, and setting Busy in kelma shows Busy in Teams web.
  - **Status.** The profile menu has "Set status" (Available, Busy, Do not disturb, Be right back, Appear away,
    Appear offline, Reset status) and today's work location; the engine sets them (`SettingsCmd::SetStatus`,
    `SetWorkLocation`) and reads the menu again, which shows the status, the location and a presence dot on my
    avatar (`Avatar.presence`, usable for any avatar). kelma registers this device at sign-in (`my_presence.rs`),
    removes it while the screen is locked (`org.freedesktop.ScreenSaver.GetActive`, checked each minute) and on
    quit; a call publishes "In a call" (`call_presence.rs`). UI test
    `the_profile_menu_sets_my_status_and_work_location_through_the_engine`. The chat list draws a dot on each
    1:1 chat's avatar (`people_presence.rs`: the people drawn are looked up once a minute in batches, and
    trouter presence events update them). A profile card shows the dot, the availability in words and today's
    work location. The status options also edit the status message (Save and Clear message). Evidence
    (2026-11-19, live): `kelma-cli presence note kelma test` read back `note="kelma test"`, and `presence note`
    cleared it. Not built: dots in the participants list and people picker, "Pause notifications" and
    "Notify me when available".
  - **Evidence (2026-11-19, live, work account):** the global `presence.teams.microsoft.com` host answered 401 to
    the presence token. A capture of Teams web showed it uses the regional unified presence service from the sign-in
    answer (`regionGtms.presenceUPS`) with the same token audience. With that address `kelma-cli presence` answered
    200 and read the user's own availability (Away / Away). `kelma-cli presence set Busy|DoNotDisturb` (ten-minute
    expiry) and `presence reset` then read back Busy, DoNotDisturb and Available. A forced status only shows while
    some Teams client of the user is active: with none, the service kept answering Away; with Teams web open it
    showed the forced value at once. So the GUI has to register its endpoint (`PUT me/endpoints/`) and report
    activity for a status set in kelma to show. The status was reset at the end.

### 6.5 Profile card
- [ ] DEFERRED: needs test tenant (comparing the card needs a second account; built and tested offline). Impact 4, effort 3. Seen: avatars and names.
  - **What.** Clicking an avatar or name opens a card with:
    - a large photo, name and title
    - presence and status message
    - email (click to copy), local time and work location
    - buttons: Chat, and Email (mailto)
  - **How.** Use `fetchShortProfile`, which already exists, and add `jobTitle` and `email`. Presence comes from 6.4.
  - **Done when.** The card matches the Teams card for 3 colleagues.
  - **Status.** Built, not ticked. Clicking an avatar or a sender name opens a card with photo, name, title and department, email (click to copy), company, office, and the Chat and Email buttons; Chat opens the 1:1. Live (read-only): 3 cards read, all with an email, none with a job title, so the title row stays empty on this tenant until Delve answers. Local time is not shown (no source for a person's time zone), and presence needs 6.4. The comparison with Teams for 3 colleagues was not done.

  - **Evidence (hover card, 2026-11-19).** Resting on a sender's photo or name for 0.6 s opens the card as a hover
    card that closes when the pointer leaves it; it shows the person's availability row with its dot, today's work
    location, and "Send a quick message", which queues the text for the 1:1 chat without leaving the open chat. UI
    test `resting_on_a_photo_opens_a_hover_card_whose_quick_message_goes_to_their_chat`. Hover in the chat list and
    the participants list is not built.
### 6.6 Pinned messages banner
- [ ] DEFERRED: needs test tenant (comparing the banner needs a real group with a second account; built and tested offline). Impact 3, effort 2. Seen: the group chat banner ("📌 <sender> … 9/6" with "…").
  - **What.**
    - A strip under the header shows the current pinned message.
    - It cycles through all of them with a counter and click-to-jump.
    - The "…" menu has Unpin.
  - **How.** Discover the thread property; pinning itself is 5.3.
  - **Done when.** It shows the same pinned messages as Teams for a real group chat.
  - **Status.** Built, not ticked. A strip under the header shows the newest pin with `k/N`, arrows to cycle, a click jumps to the message (loading older history when needed) and moves to the next pin, and "…" has Unpin. Pins come from `Client::pinned` and are resolved from the store or up to 3 older pages. Live: in the test group, pin then unpin worked (1 then 0 pins) and 1 of 8 real chats has pins (read-only). Not compared with the Teams banner on a real group.

### 6.7 Read receipts
- [ ] DEFERRED: needs test tenant (Seen needs a second account that reads; built and tested offline). Impact 3, effort 2. Seen: settings › Chats "Read receipts".
  - **What.** On your newest message in 1:1 and small groups: ✓ for sent, an eye icon for "Seen by …". The setting turns it off, which also stops sending your receipts.
  - **How.** Probably `GET /v1/threads/{id}/consumptionhorizons` (discover or verify) to compare each member's horizon with your message.
  - **Done when.** "Seen" appears after a colleague reads (observed naturally).
  - **Status.** Built, not ticked. My newest message in a 1:1 or a group of up to 20 shows a check once sent and an eye with "Seen by …" when members' consumption horizons reach it; the horizons are polled every 20 s for the open chat. The setting is the composer "…" menu, "Read receipts". It honors the account's `readReceiptsEnabled` (read: true) and a local off switch; it does not write the account setting (no setter is known, and writing it is a change to the real account). Live: horizons read for 5 of 8 chats; "Seen" was not observed (the colleague in the test group had not read).

---

## Phase 7: finding things

### 7.1 Search page
- [x] Impact 5, effort 4. Seen: Search (Ctrl+Alt+E).
  - **What.**
    - Results tabs: All, Messages, Files, People, Channels, Group chats, Meetings (and 2 more).
    - Filters: In (chat or channel), Person, Date.
    - Message results show a highlighted snippet; clicking jumps to the message in its chat.
    - "We couldn't find any results" empty state.
  - **How (discover).**
    - `substrate.office.com/search/api/v2/query`, with a substrate token (`https://substrate.office.com/.default`; check that the refresh grants it).
    - People results reuse `searchV2`.
    - Chats and channels can be searched locally.
  - **Done when.** Searching a phrase from the cache finds it, and the jump lands on it.
  - **Status.**
    - Built in `ui/find.slint` (`SearchView`), `backend/find.rs` and `teams_proto::search`. Tabs: All, Messages, Files, People, Channels, Group chats, Meetings (the two unnamed extra tabs are not built). Filters In, Person and Date apply to messages. A hit shows the sender, the place, the time and a snippet with the matching words in bold.
    - The cache is searched first (so results show at once and offline), then Substrate adds older messages. Files use Graph search (`onedrive::search`) because the Substrate `File` entity answers 500; people reuse `search_people`; channels, group chats and meetings are matched locally by name.
    - The top-bar box opens the page on Enter. Clicking a message hit opens its chat and lands on the message (a channel reply opens its thread).
  - **Evidence.** UI test `ctrl_alt_e_opens_search_and_a_phrase_from_the_cache_is_found_and_jumped_to`, plus `the_in_and_date_filters_narrow_the_messages` and `search_tabs_split_the_results_and_an_empty_search_says_so`. Live read check: `kelma-cli find search` returned hits whose ids were all found in the chat service (counts in `docs/protocol.md`), and `find mentions` flagged 50 of 50 hits as mentions.

### 7.2 Find in chat
- [x] Impact 4, effort 3. Seen: chat header 🔍 ("Find in chat"), and "Find in channel".
  - **What.** A search box above the conversation with ↑ and ↓ through matches; the list scrolls to each match and highlights it.
  - **How.** Add SQLite FTS5 over cached messages. When a match is older than the cache, page backward with the existing history paging.
  - **Done when.** A UI test plus a live check on a long chat.
  - **Status.**
    - Deviation from "How": the cache is scanned with a SQL `LIKE` prefilter (`Store::messages_containing`) and confirmed on the plain text, not FTS5, so there is no migration (other work is adding migrations in parallel). Matches older than the cache come from the server search `clientthreadid:<chat>`; moving to one pages the history back with `jump_to` (10 pages).
    - The 🔍 button in the chat or channel header opens the bar ("Find in chat" or "Find in channel"). ↑ goes to the older match, ↓ to the newer one, Enter and Shift+Enter do the same, Esc closes. The list scrolls to each match and keeps it highlighted. A channel reply match opens its thread.
  - **Evidence.** UI tests `find_in_chat_steps_through_the_matches_and_highlights_each` and `find_in_channel_has_its_own_label_and_finds_posts`. Live read check: `kelma-cli find search <word> in <sandbox channel>` found the 2 sandbox messages (a post and its reply) and both ids were found in the chat service.

### 7.3 Activity feed
Moved to its own plan: `plans/activity.md`.

### 7.4 Quick views: Mentions and Followed threads
- [ ] Impact 4, effort 3. Seen: chat list "Quick views", post menu "Follow thread", settings › Chats (auto-follow). DEFERRED: needs test tenant (Teams' Followed threads only lists a thread after another user replies in it; a second user is needed to see a kelma follow arrive there).
  - **What.**
    - Mentions lists every message that mentions you.
    - Followed threads lists the channel threads you follow, with new replies on top.
    - Channel posts have "Follow thread" and "Unfollow thread" in their menu.
    - Auto-follow covers threads you started, threads you replied to and threads where you are @mentioned (settings).
  - **How.** Mentions is a filtered Activity. Followed threads needs discovery (a thread subscription property).
  - **Done when.** Following a test post in kelma shows it in Teams' Followed threads.
  - **Status.** Not ticked: the Done-when needs a reply from a second user.
    - Teams web check (2026-11-19, sandbox `tests` post "kelma follow test", read with `kelma-cli find props` and `find feed 48:threads`): Teams' "Follow thread" stores the `follow` emotion as `"1"` and "Unfollow thread" as `"0"`. kelma had them reversed; `follow_body` now writes `"1"` to follow. A new post is followed automatically in Teams, yet Teams' Followed threads view did not list it, after either Teams' own explicit follow or kelma's, and `48:threads` got no row: the view only shows threads that have had replies. Teams also kept showing "Follow thread" after kelma wrote `"1"` until it was clicked there, so Teams caches the state locally.
    - Built: the chat list "…" menu has Quick views > Mentions and Followed threads (`QuickView` in `find.slint`, `backend/find.rs`). Channel posts have "Follow thread" and "Unfollow thread" in the "…" menu. Unfollow is also a button on each Followed threads row, and the list is ordered by the newest reply.
    - Mentions is not a filtered Activity (Activity's feed rows carry no text for `48:mentions`): it uses the Substrate query `mentions:<oid>` plus mentioning messages in the cache, which listed 50 of 50 hits as mentions in the live check.
    - Followed threads reads `48:threads` (newest row per thread, `deleted` rows drop it). Following writes the `follow` emotion (`"1"` follow, `"0"` unfollow, corrected after the Teams web check below), checked on a sandbox post: the service stores it and reads it back. `48:threads` did not change within 30 s of an explicit follow or unfollow (it did get an `ImplicitFollow` row when the sandbox post was replied to), so it is unknown whether Teams' view reads that flag.
    - Auto-follow is a Teams account setting and no Settings screen exists yet (`plans/settings-and-platform.md`), so it is not built.
  - **Evidence.** UI tests `mentions_lists_the_messages_that_mention_me_and_a_click_opens_one`, `followed_threads_list_new_replies_on_top_and_unfollow_removes_one` and `channel_posts_offer_follow_and_unfollow_in_their_menu`.

### 7.5 Shared and Files tabs
- [x] Impact 4, effort 3. Seen: chat tab "Shared", channel tab "Shared".
  - **What.**
    - A table of Name, Date shared and Shared by, filtered by Recent, Files or Links.
    - "Filter by keyword".
    - "Upload" (same flow as 5.1 and send_file).
    - Clicking opens the file or link.
  - **How.**
    - Chats: collect from cached messages (`properties.files` and links in the content).
    - Channels: Graph `GET /teams/{groupId}/channels/{channelId}/filesFolder`, then its children. `groupId` comes from CSA `teamSiteInformation` (verify).
  - **Done when.** It lists the same files as Teams for 48:notes and the test channel.
  - **Status.** Ticked 2026-11-19 after the Teams web comparison: Teams' Shared tab for 48:notes lists 27 rows (5 files, 22 links, all shared by me, newest 11/10, oldest 1/22) to the end of the list, the same 5 files and 22 links `find shared 48:notes` returns; the sandbox `tests` channel's Shared tab says "This folder is empty", as `find folder` found (0 items).
    - Built: the header tab "Shared" (chats) or "Files" (channels) shows `SharedView` with Recent, Files and Links, a keyword filter, and Upload (the file picker adds the file to the composer, then the chat shows again with the chip). Rows open the file or link in the browser.
    - Chats read the Substrate `AllShared` list, the same call the Teams tab uses, and add what cached messages carry. Channels also list their SharePoint folder (Graph `filesFolder` then its children, `teams_proto::views::channel_files_folder`, reusable by 8.6).
    - Live read check: `find shared 48:notes` returned 5 files and 22 links, all with a sender and a date. `find folder` on the sandbox channel found its folder (0 items), and `AllShared` for that channel returned none.
  - **Evidence.** UI tests `the_shared_tab_lists_files_and_links_filters_them_and_opens_one` and `a_channel_shows_its_files_tab_and_a_chat_list_of_links`.

### 7.6 Chat details pane
- [x] Impact 2, effort 2. Seen: header button "Open chat details".
  - **What.** A right panel "In this chat" with the recent shared items and "See all", which goes to 7.5.
  - **Done when.** It toggles, and its items open.
  - **Evidence.** UI test `the_details_pane_toggles_lists_recent_items_and_see_all_opens_shared` (the "i" button toggles it, it lists the five newest shared items, a row opens, "See all" goes to the Shared tab). The panel overlays the right edge of the conversation instead of pushing it aside.

---

## Phase 8: chat list and channels

### 8.1 Custom sections
- [ ] BLOCKED: section writes (create, move, rename) are built in the web client's GraphQL layer and are not in its public JS; probing would change the user's real sections. Reading and local sections work. Impact 4, effort 3. Seen: Favorites, Groups, DevOps, Operation and Chats in the list; "New section"; "Move to section" and "Remove from section".
  - **What.**
    - User sections that mix chats and channels, each collapsible.
    - "Collapse all sections" (Alt+Q).
    - New section from the new menu.
    - Move and remove from the item menu.
    - Sections sync with Teams.
  - **How.** Discover the CSA folders API (the sections already show in Teams' list). Replace the local favorites star with the real Favorites section.
  - **Done when.** kelma shows the same sections and order as Teams, and a move shows up in Teams.
  - **Status.** Read side and local sections done; the write side is BLOCKED.
    - Done: kelma reads the account's sections (`GET csa/api/v1/teams/users/me/conversationFolders`, checked live: the 5 sections and their order match Teams) and shows them between Favorites and Chats, each collapsible. New section, Move to section, Remove from section, rename, delete and Alt+Q work. UI tests: `ui_tests/lists.rs`.
    - BLOCKED: a move does not reach Teams. The web client defines the write calls (`POST`, `PUT /<id>`, `DELETE` on the same URL) but builds their bodies inside its GraphQL layer, which is not in its public JavaScript. A probe would change the real account's sections, which the run rules forbid. Needs a DevTools capture of the create and move requests, then a test on a test account. Until then, changes stay on this computer and win over what Teams has (`docs/protocol.md`, "Chat list sections").

### 8.2 Chat item menu
- [ ] DEFERRED: needs test tenant (48:notes refuses mute and real chats may not be muted; menu built and UI-tested). Impact 4, effort 2. Seen: right click on a chat.
  - **What.** Mark as unread, Move to section, Remove from section, Mute, Copy link, Leave (group), Hide.
  - **How.**
    - Mute and Hide: discover (conversation properties).
    - Leave: 6.3.
    - Copy link: `https://teams.microsoft.com/l/chat/{id}/0`.
  - **Done when.** Mute in kelma silences notifications both in kelma and in Teams web.
  - **Status.** Built, with a DEFERRED live check.
    - Done: the right-click menu has Mark as unread, Move to section, Remove from section, Mute or Unmute, Copy link, Leave (group chats) and Hide. Mute writes the `alerts` property, reads the state back from the aggregator and silences desktop notifications; Hide writes `unpinnedTime` and keeps the chat out of the list until its next message. UI tests: `ui_tests/lists.rs`.
    - DEFERRED: needs test tenant (the Done when needs a chat that can be muted and a Teams web session to look at it. `48:notes` refuses the property with `The Thread Id is invalid`, and the run rules forbid muting real chats).

### 8.3 Filters and list options
- [x] Impact 3, effort 1. Seen: filter pills, "…" › Customize view, settings › Chats.
  - **What.**
    - Pills: Unread, Channels, Chats, Meeting chats, plus the filter text box.
    - Options: show message previews, show time stamps, chats and channels combined or separate.
  - **Done when.** UI tests cover each pill and toggle.
  - **Evidence.** `ui_tests::lists` clicks each pill (Unread, Channels, Chats, Meeting chats, Drafts) and each option (previews, time stamps, chats and channels together) in the "…" menu and checks the rows. The filter text box is the search field above the list.

### 8.4 Channel posts like Teams
- [ ] DEFERRED: needs test tenant ("Also send to channel" and post pin write calls need a capture; side-by-side check with Teams pending; post cards, replies, follow and announcements built). Impact 4, effort 3. Seen: channel Posts tab.
  - **What.**
    - Posts are cards with replies shown inline under each post (collapsed after 2, with "See N more replies"), and "Reply in thread" opens an inline reply box.
    - The "Post in channel" button opens a composer with "Add a subject", formatting, and the Announcement type (headline and background banner).
    - Thread replies have "Also send to channel".
    - The menu has "See new posts at top" and "In post channels, show recent activity at the top or bottom".
    - The post menu has: Follow thread, Copy post link, Save, Pin, Mark as unread.
  - **How.**
    - Replies come from the existing `;messageid=` threads, and subjects already work.
    - Announcement and "also send to channel" need discovery.
  - **Done when.** The test channel looks and behaves like Teams.
  - **Status.** Mostly built; not ticked because two entries have no known call and the test channel was not compared with Teams side by side.
    - Done: post cards with the first two replies inline and "See N more replies" (fetches the thread), the inline reply box (sent through the outbox into the thread), "Post in channel" with subject and Announcement (headline and banner color; the post was stored and read back in the test channel), "See new posts at top", and Follow thread / Unfollow thread (`rcmetadata` `{"f":1}`, read back 1 then 0). Copy post link, Save and Mark as unread are the existing message menu. UI tests: `ui_tests/channels.rs`.
    - Not built: "Also send to channel" (no such call or property in the web client's public JavaScript) and Pin for a post (the write call was not captured).

### 8.5 Channel menu
- [x] Impact 3, effort 2. Seen: right click on a channel.
  - **What.** Manage notifications (all activity, off, custom), Hide, Copy link, Leave channel (private channels), Go to team. Admin items stay in the browser.
  - **Done when.** Hiding and per-channel notifications work and sync.
  - **Evidence.** Live on the sandbox channel: hide flipped the aggregator's `isFavorite` to false and show back to true; the three notification levels were written and read back, then restored. The menu (Manage notifications, Hide or Show, Copy link, Go to team, Leave channel for private channels after a confirmation) is in the chat list and the Teams view; UI tests in `ui_tests/lists.rs`. Leaving a private channel is unit-tested only (it would remove a real member).

### 8.6 Files in channels
- [ ] DEFERRED: needs test tenant (a second account must open the uploaded file; upload, post and delete verified in the sandbox). Impact 4, effort 3.
  - **What.** Attach in a channel uploads to the channel's SharePoint folder and posts the file card.
  - **How.** Graph `PUT /drives/{driveId}/items/{folderId}:/{name}:/content`, where the folder comes from `filesFolder` (7.5), then `properties.files` as for chats. No member invite is needed.
  - **Done when.** A test file posts in the test channel and opens for others. Delete it afterwards.
  - **Status.** Built and run live; DEFERRED for the last clause.
    - Done: attaching in a channel uploads to the channel's files folder (`filesFolder`, own call in `teams_proto::channel_files`) and posts the file card; a 25-byte file was posted in the test channel, read back as a file card, then the post and the file were deleted.
    - DEFERRED: needs test tenant (a second account must open the file; the folder is the one every channel member already sees).

### 8.7 Attach cloud files
- [x] Impact 3, effort 3. Seen: attach menu "Attach cloud files".
  - **What.** A picker with Recent, My files, Shared with me and team libraries; choose files, then send them as file cards.
  - **How.** Graph `/me/drive/recent`, `/me/drive/root/children` and `/me/drive/sharedWithMe`, then share with members and send as in `files.rs`.
  - **Done when.** Attaching an existing OneDrive file works in 48:notes.
  - **Evidence.** Live: an existing OneDrive file (from `me/drive/recent`) was posted to 48:notes as a file card with its own name, then the post was deleted. The picker (the composer's "…" menu › Attach cloud files) lists My files, Recent, Shared with me and the team libraries; UI test `attach_cloud_files_opens_the_picker_with_recent_and_shared_and_attaches_the_pick`. The invite to chat members is unit-tested only.

### 8.8 Adaptive cards like Teams
- [ ] DEFERRED: needs test tenant (no Workflows bot in the sandbox to compare cards with Teams; model and drawing built and tested). Impact 4, effort 3. Seen: Workflows posts (Release readiness, run reports).
  - **What.**
    - Container styles (the colored "AT RISK" band).
    - FactSet as a two-column table.
    - TextBlock sizes, weights and colors.
    - ColumnSet.
    - ActionSet as real buttons (`Action.OpenUrl` opens the browser).
    - Images.
  - **How.** Add a structured card model in `render` (instead of today's flattened text) and a `CardView` Slint component.
  - **Done when.** The Workflows cards in the test channel match Teams side by side.
  - **Status.** Built; DEFERRED for the side-by-side check.
    - Done: `teams_proto::chat_card` keeps the card structure (container styles as colored bands, fact sets as two columns, text size, weight, color and subtle, column sets with auto and stretch widths, images, `ActionSet` and card actions, hero cards, the announcement banner) and `CardView` draws it. `Action.OpenUrl` opens the browser; other actions are drawn disabled. The rows, colors and buttons are checked in `ui_tests/channels.rs`.
    - DEFERRED: needs test tenant (a Workflows bot posting into a test channel; the sandbox has none, so there was nothing to compare with Teams).

---

## Phase 9: notifications, settings, polish

### 9.1 Notification settings
- [x] Impact 4, effort 2. Seen: settings › Notifications and activity.
  - **What.**
    - Mute all.
    - Sounds on or off.
    - "Show message and content previews".
    - Test notification.
    - Per type: chat messages; @mentions (personal, Everyone, channel and team, tag); replies to followed threads; likes and reactions; posts in shown channels (with replies). Each is off, Activity only, or Activity and banner.
    - Per-chat mute (8.2) and per-channel settings (8.5).
  - **How.** Store the settings locally in config and route them through `should_notify`.
  - **Done when.** Unit tests cover the routing, and the test notification button works.
  - **Evidence (data layer only, UI missing, box stays open).** `crates/teams-proto/src/user_settings.rs`. The per-type settings are not local: they are the `userPersonalSettings` JSON string in the `GET /v1/users/ME/properties` bag. `toastSettings.<Type>` is the banner flag and `activityGenerationSettings.<Type>` the Activity-feed flag, so banner and feed is both true, feed only is the feed flag alone, off is neither (`emailSettings` mirrors the keys for mail). Types (`Kind`): `Chat`, `MentionInChat_Person_v2`, `MentionInChat_Everyone`, `Mention_Person`, `Mention_Channel`, `Mention_Team`, `Mention_Tag`, `Reply`, `ReplyToReply`, `Like`, `LikeInChat_v2`, `Follow_ChannelNewMessage`, `Follow_ChannelReplyMessage`, `Visible_ChannelNewMessage_v2`, `Visible_ChannelReplyMessage_v2`, `Visible_ChannelMention_v2`, `MeetingWentLiveNotification` (meeting started), `Priority`, `VIP`. `selectedPreset` is 1 all activity, 2 my @mentions, 3 custom (any per-type change sets 3). "Pause notifications" is `toastSettings.temporarilyPauseUntil` (ISO time). Sounds are `soundsSettings.{alertSound,incomingCallsAndMeetingStartSound,showNotificationDuringCallsAndMeetings}` and `toastSettings.{alertSoundType,alertUrgentSoundType,PlayNotificationSoundsWhenBusyOrInMeetings,PlayNotificationSoundsForUrgentAndPriority,GlobalSoundAndPreview,ShowPreviewInToasts}`. Meeting-chat notifications are `chatSettings.meetingChatNotification` (`MeetingsUserParticipatesIn`, `MeetingsUserIsInvitedTo`, `MeetingsManuallyUnmuted`) and `meetingChatNotificationRSVPv2{Accepted,Tentative,Follow,NoResponse,Declined}`. Writing is `PUT /v1/users/ME/properties?name=userPersonalSettings` with `{"userPersonalSettings":"<whole object as a string>"}`; `NotificationSettings` keeps every key it does not know, and `write_request` only builds the request (never sent). Channel settings stay with NSS (§3.8, `channel_notifications`). Quiet hours: the web client has no such server setting (no `quietHours`, `dndSchedule` or similar in the bundle), so `QuietHours` (days, start and end, overnight windows, `mobile`) is a local schedule the app must store itself. Not run live: the keyring failed twice (DBus, no reply), so the real answer shape of `userPersonalSettings` is unverified; `kelma-cli settings shape` prints key names and types only, to check it. Sources: Teams web client JS bundle (facts only), rusty-teams `chatsvc/src/settings.rs` (MIT, property-bag access adapted). Tests: `cargo test -p teams-proto user_settings`, `cargo test -p kelma-cli settings`. CLI: `settings notifications|quiet|shape` and `settings set notify|pause|sound|meeting-chat|meeting-rsvp ... --dry-run`.
  - **Evidence (UI, box stays open).** 2026-11-19: Settings › Notifications and activity lists the 19 types in three groups (Chat; Teams and channels; Meetings and people), each with Banner and feed, Only show in feed, Off, read from `userPersonalSettings` by the engine; Pause notifications; and a Test notification button (a local desktop notification through `notify-rust`). Changes are dry runs: `NotificationSettings::write_request` on a copy, logged and listed as planned, the page keeps showing the Teams value. Verified: `privacy_toggle_plans_a_dry_run_and_changes_nothing` (Chat messages: Off gives the `userPersonalSettings` PUT), `notify_rows_group_and_read_delivery`. Still open: sending the write, sounds and meeting-chat options in the UI, `should_notify` routing, per-chat and per-channel settings. Tests: `cargo test -p kelma settings`.
  - **Evidence (routing and test button; 2026-11-19).** The two Done-when parts are met locally: unit tests cover the routing and the Test notification button works. The routing is `backend/settings.rs`: `incoming_kind(conversation id, is reply, mentions, me)` gives the notification type of a new message (chat: message, @mention of me, Everyone; channel: post, reply, @mention of me, channel, team or tag mention), and the pure `banner_decision(settings, kind, tray_muted, now)` says `Show { preview }` or `Hidden(Muted | Paused | Delivery)`: the tray mute first, then "Pause notifications" while its end time is in the future, then the type's own switch (banner and feed shows; feed only and off hide; no value from Teams shows, as before), and `toastSettings.ShowPreviewInToasts = false` replaces the text with "New message". `live.rs::notify()` calls `settings::banner_for` and `banner_body` (two lines); the engine keeps the settings fresh (`start_notify_prefs`: a read of the properties bag at start and every ten minutes, and again whenever Settings reads them), until then every message notifies as before. Per-chat mute and per-channel Off still apply first (`lists_silenced`). Verified: `incoming_messages_map_to_the_notification_types`, `without_settings_every_event_notifies_as_before`, `each_types_own_switch_decides`, `mute_and_pause_beat_every_type`, `the_preview_switch_only_changes_the_text` (`cargo test -p kelma settings`); `the_test_notification_button_raises_a_desktop_notification` presses the real button in the window (the hook is recorded in the test; on the desktop it is a `notify-rust` notification). Not done, and why the box stays open: the Teams-side writes (DEFERRED on the checkbox line), the sound switches, a "Mute all" switch in Settings (the tray's Mute exists), and the "Show message and content previews" setting is read by the routing but has no switch on the page (it is a Teams setting too).

  - **Evidence (writes, 2026-11-19).** The user allowed the writes on the work account: a change on the page is sent (`SettingsCmd::Apply`) and the page reads Teams again. Live: `kelma-cli settings set notify like-in-chat feed --apply` read back "feed only", then `banner` read back "banner and feed" (the whole notification settings are written back from what was read).
  - **Evidence (sounds and meeting chats, 2026-11-19).** Settings › Notifications also has Sounds and previews (Show message preview, sounds for calls and meetings starting, sounds when busy, sounds for urgent and priority, notifications during calls) and Meeting chats (all, participated, unmuted); each change is sent like the others. "Mute all" is Pause notifications. UI test `sound_switches_and_meeting_chats_build_the_teams_write`; live: `settings set sound show-in-calls On --apply` read back On, then Off read back Off (its value before). Routing: `banner_for` applies the Teams types, pause and previews to desktop notifications (unit tests in `backend/settings.rs`).
### 9.2 Tray icon and badge
- [x] Impact 4, effort 3.
  - **What.** A tray icon with the unread count, a menu (Open, Status, Quit), close to tray, and optional autostart. The taskbar badge shows the unread count.
  - **How.** Use the `ksni` crate (StatusNotifierItem), an autostart `.desktop` file, and the Unity launcher entry API for the badge.
  - **Done when.** It works on KDE Plasma.
  - **Evidence (tray crate only, app wiring missing, box stays open; same work as X3.3 in `plans/settings-and-platform.md`).** 2026-11-19: new crate `crates/tray` (`ksni` 0.3.7 over zbus 5, pure Rust, no new system package). `Tray::start(TrayOptions) -> TrayHandle` runs on its own thread with a private runtime, so it sits beside the Slint loop. `TrayHandle` (cheap to clone): `set_unread(n)` (overlay badge pixmap in 4 sizes drawn in code, title `kelma (12 unread)`, tooltip), `set_muted(bool)`, `set_launcher_count(n)`, `events()` (a cloneable crossbeam `Receiver<TrayEvent {Open, ToggleMute(bool), Quit}>`), `tray_error()`, `launcher_ready()`, `shutdown()` (removes the icon and clears the launcher badge). The icon is the `kelma` theme name with a pixmap fallback drawn from the `packaging/kelma.svg` shapes. Menu: Open kelma, Mute notifications (checkmark), separator, Quit; left click is `Open`. The badge is 0 hidden, 1 to 99 as is, `99+` above. `launcher::launcher_update` builds the `com.canonical.Unity.LauncherEntry.Update` body (`application://kelma.desktop`, `count` int64, `count-visible`), and `set_unread` sends it too unless `sync_launcher_badge` is off. `autostart::enable(exec, hidden_flag)`/`disable()`/`is_enabled()` write `$XDG_CONFIG_HOME/autostart/kelma.desktop` from the fields of `packaging/kelma.desktop` plus `X-GNOME-Autostart-enabled=true`. Live on KDE Plasma Wayland (`kelma-cli tray-demo`): the item showed up in `org.kde.StatusNotifierWatcher.RegisteredStatusNotifierItems` as `org.kde.StatusNotifierItem-<pid>-1/StatusNotifierItem` and left it on exit; `Activate` produced `Open`, the dbusmenu `Event` on Mute produced `ToggleMute(true)`; the item's Title, ToolTip and `OverlayIconPixmap` (4 sizes) followed 3, 12, 120 and 0; `dbus-monitor` saw LauncherEntry updates 3, 12, 120, 0 and a final 0 on shutdown. Autostart enabled, validated with `desktop-file-validate` and disabled under a scratch `XDG_CONFIG_HOME`. Not verified: how Plasma draws the overlay (no screenshot taken), the task manager badge on a pinned launcher, GNOME (needs the AppIndicator extension). Not done: the app has to call `Tray::start`, handle the events (show and focus the window, mute, quit), pass a `--hidden` flag it does not parse yet, close to tray, and a Settings toggle for autostart; the Flatpak uses the Background portal for autostart instead of the file. Tests: `cargo test -p tray -p kelma-cli`.
  - **Evidence (app wiring done, box ticked for the KDE Plasma tray; 2026-11-19).** `crates/kelma/src/tray_wiring.rs` starts `Tray::start` before the event loop and replaces `window.run()` with `show` (or not) plus `slint::run_event_loop_until_quit()`. `--hidden` starts with the tray only (it shows the window anyway when the tray did not register, so the user is never locked out). Closing hides the window when the item registered, and quits as before when it did not. Tray Open shows, un-minimizes and focuses the window, Quit quits, and Mute toggles an in-process flag that `backend/live.rs` `notify()` checks (there was no notification setting; the flag is not saved and a Settings toggle can take it over). The unread total (Activity items plus unread chats, the same number as the window title) goes from `UiHandle::badges` through a 300 ms debounce to `TrayHandle::set_unread`, which also drives the launcher badge. Tests: `cargo test -p kelma tray_wiring` (close decision, hidden start, unread total, debounce). Live on KDE Plasma Wayland with `KELMA_DEMO=1 KELMA_HOME=<tmp> kelma --hidden`: `RegisteredStatusNotifierItems` gained `org.kde.StatusNotifierItem-<pid>-1/StatusNotifierItem` and lost it when the process was killed; no kelma window existed while hidden; the item's `Activate` (left click) showed the window, active, titled `(5) kelma` (the demo's unread count); Alt+F4 hid the window and left the process and the tray item alive; `Activate` again showed it. Not verified: the Quit and Mute menu entries inside this app (the `tray` crate's own live check covers the events), the launcher badge on a pinned task manager entry, GNOME. Not done here: the Settings toggle for autostart (belongs to Settings; `tray::autostart` and the `--hidden` flag it writes are ready).

  - **Evidence (notification look, 2026-11-19).** Message notifications follow Teams: the chat as the title, the
    sender in bold in a group, "📷 Image" for a picture, the sender's photo, kelma's icon and desktop entry, and Open
    (click), Reply and Like actions (`backend/banner.rs`, tests for the text). Settings › Notifications has "Show the
    sender's photo" and "Show Reply and Like buttons". Not compared side by side with a Teams notification yet.
### 9.3 Appearance
- [x] Impact 3, effort 3. Seen: settings › Appearance and Chats.
  - **What.** Theme (light, dark, follow the OS), message density (Comfy or Compact), skin tone, font size.
  - **How.** Make the `Theme` global switchable at runtime and read the OS theme through the XDG desktop portal.
  - **Done when.** Screenshots of both themes are in `docs/screenshots`.
  - **Evidence (data side only, UI missing, box stays open).** `crates/appearance`. `AppearancePrefs` (theme Light, Dark or System; optional accent, `None` follows the portal; density Comfy or Compact; `font_scale` clamped 0.75 to 2.0) lives in `appearance.json` in the app config dir (same `directories` strings as `session`, `KELMA_HOME` honored), written atomically; a missing or corrupt file gives defaults and a bad single field falls back alone. `portal::read` and `portal::watch` use zbus over the XDG portal (`org.freedesktop.portal.Settings` `ReadOne`, falling back to `Read`; `SettingChanged` as a `PortalChange` stream) for `org.freedesktop.appearance` `color-scheme` and `accent-color`; with no session bus they return `Ok(None)`. `resolve_theme`, `resolve_accent` and `resolve` are pure. Live, read-only, on this KDE desktop: color-scheme Dark, accent #5294e2. Skin tone is not covered here (it is a Teams server setting). Still to do: the runtime-switchable `Theme` global and the Settings page in the GUI, then the screenshots. Try it with `kelma-cli appearance [show|watch]`. Tests: `cargo test -p appearance` (live: `-- --ignored`).
  - **Evidence (UI, box stays open).** 2026-11-19: `ui/theme.slint` properties are now `in-out`, plus `dark`, `font-scale`, `compact`, `high-contrast` and `reduced-motion`. Settings › Appearance: theme (Light, Dark, Use system setting), accent (desktop accent or six colours), density, font size (5% steps, 75% to 200%) and skin tone (the existing chat preference). `src/ui/settings.rs` applies them live: `appearance::resolve` with the portal state read at start (`portal::read`, 400 ms limit) and followed by `portal::watch()` on a thread, giving a dark, light or high-contrast palette with the accent mixed in; the window's `default-font-size` is `14px * Theme.font-scale`. Saved to `appearance.json`. Verified: `theme_choice_changes_the_theme_global_live` (Light sets `Theme.main` #ffffff, Dark #141414, Blue sets `accent-strong`), `font_size_and_larger_text_scale_the_theme_font`, round-trip in a temp home. Theme sweep: the other views (chat, OneDrive, calendar, dialogs, apps, teams) now use `Theme` tokens for backgrounds and borders and scale their `font-size` values by `Theme.font-scale`; written, compile pending (build pause). Still open: density is only exposed as `Theme.compact`, and no screenshots are in `docs/screenshots`. Tests: `cargo test -p kelma settings`.
  - **Evidence (screenshots; box ticked 2026-11-19).** Built from this branch and run in demo mode (`KELMA_DEMO=1 KELMA_VIEW=chat KELMA_DEMO_OPEN=19:backend@thread.v2`, no network, nothing opened outside the window) with a temporary `KELMA_HOME` whose `appearance.json` set the theme, taken with `spectacle -b -n -a` on KDE Plasma Wayland, the process killed by its PID after each shot: `docs/screenshots/appearance-light.png` (`{"theme":"light"}`), `docs/screenshots/appearance-dark.png` (`{"theme":"dark"}`, the comparison) and `docs/screenshots/appearance-150.png` (`{"theme":"light","font_scale":1.5}`). Light mode turns the top bar, rail, chat list, message pane, bubbles, composer and OneDrive (a light OneDrive Home was also looked at) light with readable text. At 150% every text in the chat list, header, messages and composer is larger; long names and the rail labels elide (`Workflo...`) because their boxes are fixed width (sidebar and rail files, not Settings). Leftover dark hard-coded areas in light mode, in files that are not part of Settings: (1) the Join banner (`ui/calendar.slint` `JoinBanner`: background `#1d3a2a`, border `#2f6b4a`) keeps its dark green with `Theme.fg` text, which is almost unreadable in light mode; (2) the selected filter pill on OneDrive Home (`ui/onedrive.slint`, line 688: `#34365c`), dark with dark text. Not covered by this screenshot: message density (it is only exposed as `Theme.compact`, the views do not use it yet), and the Settings window itself in light mode.

### 9.4 Delivery options
- [x] Impact 2, effort 1. Seen: composer "+" › "Set delivery options", Ctrl+Shift+I.
  - **What.** Standard, Important (a red "IMPORTANT!" label) and Urgent (repeated notifications on the receiving side), sent and shown.
  - **How.** Discover the exact property; purple-teams notes `importance`.
  - **Done when.** An Important message shows as important in Teams web.
  - **Evidence (data layer and CLI; UI missing).** `crates/teams-proto/src/delivery.rs`: `Importance` (`""`, `high`, `urgent`), `Delivery`, `delivery_body`/`apply_delivery`, `Announcement` (headline plus colour or picture banner card) and the receive parsers, now on `model::Message` as `importance` and `announcement`. Live in `48:notes` (2026-11-19): an Important and an Urgent message were sent with `kelma-cli deliver send` and read back as `high` and `urgent`. Still needs a person to confirm the red IMPORTANT! label in Teams web. UI side (2026-11-19): the composer's "Mark as important" (Ctrl+Shift+I and the formatting overflow menu) and the red IMPORTANT!/URGENT! label on messages exist, and an important message sent from the GUI path was read back as `high` in 48:notes (see 5.9); Urgent is not offered in the composer yet. Announcements are channel-only and were not posted live; the shape comes from the web bundle (see docs/protocol.md). Tests: `cargo test -p teams-proto delivery`.

### 9.5 Image viewer and grid
- [x] Impact 3, effort 3. Seen: multi-image messages (2-column grid), link preview cards.
  - **What.**
    - Several images in one message form a grid.
    - Clicking an image opens a viewer with zoom, next and previous, Save as, and Copy image.
    - Videos and voice play inside kelma, or keep the system player as a fallback.
  - **Done when.** A UI test covers the viewer navigation.
  - **Evidence (UI, 2026-11-19).** `crates/kelma/ui/media.slint` (`MediaStore`, `ImageViewer`), `crates/kelma/src/backend/media.rs`, `ui/message_row.slint`. A message with several pictures draws the `grid_layout` cells (4 cells, "+K" on the last one when there are more) and a click opens the viewer on that picture. The viewer is a full-window overlay with previous and next (buttons and the arrow keys, no wrap), zoom in and out (buttons, `+`, `-`, `0`, drag to pan), Save (into the Downloads folder, never overwriting, via `save_image`), Copy (the picture goes to the clipboard) and Close (button, Escape or a click beside the picture). The list is `message_images` (inline pictures, then attached image files), the full size comes from `images::fetch`, and an answer for a picture the viewer has left is dropped. Tests (Slint testing backend): `cargo test -p kelma ui_tests::media` (`several_pictures_form_a_grid_with_a_more_badge`, `the_viewer_opens_on_the_clicked_picture_and_steps_through_them`, `a_downloaded_picture_shows_and_an_answer_for_a_left_picture_is_dropped`, `save_writes_the_picture_and_never_overwrites`) and `cargo test -p kelma backend::media`. Not done: videos and voice playing inside kelma (the system player is still the fallback through the file card), and no screenshot of the viewer was taken. The data layer is described next.
  - **Data layer.** `crates/teams-proto/src/images.rs`. `image_refs(html)` lists every inline image in order with its `width`/`height` (fractions rounded; emoji, `data:` and non-https sources skipped; an `itemid` stands in for a missing `src`) and `message_images(html, files)` adds attached image files, which is the viewer's next/previous list. `ImageRef::candidates()` gives the AMS views `imgpsh_fullsize` (the original) then `imgo` (downscaled), and `fetch(client, &ImageRef)` returns `FetchedImage { bytes, content_type, url }` with the right credential per host: skypetoken for AMS, Graph `shares/u!…/driveItem/content` (then a SharePoint bearer) for OneDrive/SharePoint, none for other https hosts; the content type is sniffed when the server's is generic. `grid_layout(sizes, width)` gives cell rectangles: 1 image keeps its aspect (at most 420x320, never upscaled), 2 are side by side squares, 3 are one wide 2:1 cell over two squares, 4 are 2x2, and more than 4 show 4 with `overflow: Some(n - 4)` on the last cell ("+K more"); the 2-column rule is from the plan, the exact shapes are our choice and were not compared with Teams pixel by pixel. `save_image(target, name, content_type, bytes)` writes a safe file name with an extension from the content type and never overwrites (`name-1.png`). Live, read-only, in the own notes chat: `kelma-cli images 48:notes last --out <dir>` fetched a 4-image message and single PNG and JPEG images; the full-size view returned the real pixel size (2160x2880) where `imgo` is downscaled (see `docs/protocol.md` § AMS). The OneDrive/SharePoint path was not run live (no image file in the chat). Still to do: the grid and the viewer UI (zoom, next/previous, Save as, Copy image), then videos and voice inside kelma. Tests: `cargo test -p teams-proto images`.

### 9.6 Translate
- [ ] DEFERRED: needs test tenant (an Arabic or German message sent by another account, and Teams web to compare with; no real chat content may be translated on the work account) Impact 2, effort 3. Seen: "Translate" link on messages in another language, the message menu "Translation", settings.
  - **What.** "Translate" under a message in another language replaces its text, with "See original".
  - **How.** Discover the Teams translation endpoint.
  - **Done when.** An Arabic message translates to English like in Teams.
  - **Evidence (UI built, not checked against a real translation; 2026-11-19).** `Message.language_stamp` is read from `properties.languageStamp` and stored as the language code (`g` in the message properties). `crates/kelma/src/backend/media.rs`: a message from someone else whose language differs from the user's (`Settings::should_translate`, the settings are fetched once, defaults apply until then) shows a "Translate" link under it; the message menu has "Translate" for every text message. Clicking calls `translate_message` (on demand, the chat or channel chain scope), shows "Translating...", then replaces the body with the translated HTML and "Translated from <language>" plus "See original". The translation stays cached, so Translate after See original needs no call, and a failure shows "Could not translate" with Retry. Tests: `cargo test -p kelma ui_tests::media` (`a_message_in_another_language_offers_translate_and_shows_the_original_again`, `translate_that_cannot_reach_the_service_says_so_and_can_be_retried`) and `cargo test -p kelma backend::media` (state rules, language names, stamp parsing); `cargo test -p teams-proto model` for the new field. Not done: the Settings control for the translation language and behaviour (Settings is another session's area), automatic translation on arrival, and the live check that an Arabic message translates to English like in Teams.
  - **Data layer.** `crates/teams-proto/src/translate.rs`. Teams translates by message id, not by text: `POST CSA/api/v1/chats/<chat>/messages/languages/<to>` (channel chains: `teams/<team>/channels/<channel>/<root>/messages/languages/<to>`) with body `[{"id":"<messageId>","version":"<optional>"}]` (up to 30), headers `x-ms-use-acs-translation: true`, `x-ms-request-id` and either `x-ms-on-demand-translation: true` (the user clicked) or `x-ms-knownLanguages: en,ar` (automatic; optional `x-ms-sourceLanguage`). The answer lists `{id, version, resultCode, translatedContent, translatedSubject, translatedTitle}`. The middle tier `beta/translate` (plain text) answers 404 today, so there is no text-only call. Languages: `GET CSA/api/v1/translator/languages?locale=<ui>`. The user setting is the `userLanguageAccountSettings` key of `POST MT/beta/users/useraggregatesettings` (`defaultTranslationLanguage`, `translationPreferences.translationBehavior` Yes, No or Ask, `authoringLanguages`); when unset Teams translates to the UI language and suggests. A message's detected language is in its property `languageStamp` (`languages=de:100;length:82;&detector=Bling`); `Settings::should_translate` says when to offer the Translate link. Live, read-only: `kelma-cli translate languages [locale]` (the supported list, named in the UI locale) and `translate settings` work; the translate route accepts a request and answers 200 in the documented shape, but only a made-up message id was sent (it answers `UnknownError`), so a real translation was not checked (no real chat content translated). Still to do: expose `properties.languageStamp` on `model::Message`, then the UI link, "See original" and the Settings control. Sources: Teams web client JS bundle, rusty-teams `translate.rs` (MIT). Tests: `cargo test -p teams-proto translate`.

### 9.7 Link previews
- [ ] DEFERRED: needs test tenant (a message with a link sent by another account, to compare its card with Teams web; no message with a `links` property was looked at live) Impact 3, effort 2. Seen: a link preview card in a group chat.
  - **What.** URLs in messages show a preview card (image, title, description, domain), and sent links get the same preview, with a dismiss ✕ in the composer.
  - **How.** Discover the Teams URL preview service and the message property it uses. Render from the property when present; never fetch arbitrary URLs with credentials.
  - **Done when.** The same link shows the same card as Teams.
  - **Evidence (cards built, not compared with Teams; 2026-11-19).** `Message.links` is read from `properties.links` (`parse_links_property`, so only public links with at least two of image, title and description) and stored (`l` in the message properties). `ui/media.slint` `LinkCardView` draws the card under the message: thumbnail, domain, title and description, up to three per message, and a click opens the link. The thumbnails come from the preview service through `urlpreview::fetch_image` (service hosts only, so a link can never receive the token), one at a time with a pause after each and one retry, because the service rate-limits; the card shows without its picture until it arrives. kelma never asks the service about a link in a received message (that would send the URL to Microsoft), it only draws what the sender's Teams attached. Tests: `cargo test -p kelma ui_tests::media a_message_with_a_preview_shows_a_card_that_opens_the_link`, `cargo test -p kelma backend::media` (store round trip), `cargo test -p teams-proto model` (`properties.links`). Not done: previews for links in messages kelma sends (the dismiss in the composer and `set_message_preview` on send) and the live comparison with Teams.
  - **Data layer.** `crates/teams-proto/src/urlpreview.rs`. `GET https://urlp.asm.skype.com/v1/url/info?url=<url>` (the web client also uses `v1/url/richInfo?url=` and the `urlp`/`urlpV2` host from `regionGtms`; `https://<geo>-prod.asyncgw.teams.microsoft.com/urlp/v1/url/info` answers the same), skypetoken auth through the existing `Ams` header policy, headers `RequestId`, `CorrelationId` and `TransactionId`. The answer has `target_url` (after redirects), `title`, `description`, `site`, `favicon`, `thumbnail` (240 px) and `largethumbnail` (480 px) with `*_meta {width,height}`, and `status_code` (a page it cannot reach answers 200 with `status_code` 502 and nothing to show). The image URLs are on the same service and return raw JPEG or PNG bytes with the skypetoken; `fetch_image` only sends the token to service hosts. A sent link keeps its card in the message property `links` (`PUT …/messages/<id>/properties?name=links`, body `{"links":"[{\"@type\":\"http://schema.skype.com/HyperLink\",\"url\",\"preview\":{previewurl,previewmeta,title,description},\"previewenabled\":true}]"}`; `urlpreviews` is the newer variant); `parse_links_property` reads it and `set_message_preview` writes it (the write was not run live). Live, read-only: `kelma-cli preview https://www.rust-lang.org --image` returns title, description, site, a 240x240 thumbnail (8811 bytes downloaded) and favicon. Still to do: expose `properties.links` on `model::Message`, then render the card, the dismiss ✕ in the composer and the call on send. Sources: Teams web client JS bundle, rusty-teams `links.rs` (MIT). Tests: `cargo test -p teams-proto urlpreview`.

### 9.8 Saved messages
- [x] Impact 2, effort 2. Seen: "Save this message".
  - **What.** A "Saved" view lists saved messages and jumps to them.
  - **How.** Needs 5.3's save property plus a query for saved messages (discover).
  - **Evidence (2026-11-19).** The Saved view is a quick view like Mentions: the chat list's "…" menu › Quick views › Saved (`FindCmd::Quick("saved")` in `backend/find.rs`). It lists the local `saved_messages` table (store v3) newest saved first, with sender, place (`Team › channel` or the chat's title) and the preview; a click opens the chat and lands on the message (a channel reply opens its thread first), and "Unsave" removes the row from the list and the store and clears the server flag best effort, like the message menu's toggle. The local list stays the source of truth: the server's `48:saved` list is not available for every tenant, so no server query was added. Tests: `cargo test -p kelma ui_tests::saved` (newest first, empty state, click lands on the message, unsave). Not run live: nothing is written to the server except the existing save flag.

### 9.9 Profile menu
- [x] Impact 2, effort 2. Seen: profile menu.
  - **What.** Name and email, presence (6.4), Pause notifications, status message, Change profile photo, View account (browser), Sign out.
  - **Evidence (UI).** 2026-11-19: clicking the avatar in the top bar opens the profile menu (`ProfileMenu` in `ui/settings.slint`): name and email (from Teams when read, else the account in `config.json`), presence and status message shown read-only (the engine reads them with `get_presence`, nothing is set), Settings, View account (opens myaccount.microsoft.com), and Sign out with a confirmation, which removes the account's refresh tokens through `session::forget_account_tokens` and closes kelma. Pause notifications is on the Notifications page (dry run). DEFERRED: Change profile photo (shown disabled), setting presence and the status message (not allowed on the work account). Verified: `profile_menu_shows_who_is_signed_in_and_opens_settings`, `sign_out_removes_the_tokens_after_confirming` (in-memory token store; no real sign-out was run). Tests: `cargo test -p kelma settings`.

### 9.10 Window state
- [x] Impact 2, effort 1.
  - **What.** Restore the size, position, last open chat and collapsed sections on start.
  - **Evidence (data side done, UI missing, box stays open).** Data side done (session::window_state): `crates/session/src/window_state.rs`, stored as `window-state.json` in the config dir (next to `config.json`, `KELMA_HOME` applies). Fields `width`, `height`, `x`, `y` (both `None` when the window manager places it), `maximized`, `last_chat`, `collapsed_sections` (set of section ids), all `#[serde(default)]`, so old and partial files load. API: `WindowState::load()` / `load_from(path)` (never fails: a missing or corrupt file gives the defaults, values are clamped), `save()` / `save_to(path)` (atomic: temp file plus rename), setters `set_size`, `set_position`, `set_maximized`, `set_last_chat`, `toggle_section`, `set_section_collapsed`, `is_collapsed`. `Config` is untouched. `Debug` hides the chat id and the module never logs. Still to do: the GUI reads it at start, writes it on resize, move, chat change and section toggle (debounced), and applies it. Tests: `cargo test -p session window_state`.
  - **Evidence (UI wiring done, 2026-11-19).** `crates/kelma/src/window_state.rs`. Start: `WindowState::load()` is applied before the window shows (size, position only when `Some`, `maximized`; a maximized state only calls `set_maximized(true)` and keeps the saved geometry; a state still at the defaults leaves the UI's own size; a position on a monitor that is gone is moved back on screen after the first show). The last chat reopens once sign-in is done and the window is visible, if the sidebar models have it (chat, favorite, pinned channel or team channel), and is skipped silently if not. Save: one UI timer every 500 ms folds size, position, maximized and the open chat into the state and writes only when it changed (so at most one write per 500 ms), plus a final write when the window closes (including close to tray) and on quit. Position is never read or stored where the platform reports none (Wayland). Collapsed teams: `View.collapsed_teams` is seeded from the state and written back on every toggle (section ids `team:<id>`), as a Rust-only hook in `backend/mod.rs` (no sidebar.slint change). Demo mode (`KELMA_DEMO`) neither restores nor saves. Tests: `cargo test -p kelma window_state` (restore plan, observe/maximized rules, Wayland, write spacing, off-screen, last-chat lookup, collapsed teams) and `ui_tests/window_state.rs` on the Slint testing backend (size applied, first run untouched, chat lookup in the real models). The testing backend has no window manager, so a maximized restore is checked as a plan, not as `is_maximized()`. Not verified live: restore across a real restart with a signed-in session (needs the user's account; the logic is covered by the tests above).

  - **Evidence (chat list width, 2026-11-19).** The divider beside the chat and teams list drags it between 220 and
    600 px (also a keyboard slider); the width is saved with the window state (`sidebar_width`) and restored. A
    channel in the chat list shows its own name first, its team on the second line.
---

## Later ⏭️ (dropped for now, with the reason)

- GIFs and stickers: need Giphy and Microsoft's sticker service, and add little value for work chat.
- Loop components, Notes, Storyline, Recap, custom tabs ("Add a tab"): web apps, opened in the browser.
- Record video clip, Share to Outlook, Immersive Reader, Report message: rarely used, or they need other services.
- Message extensions (Create Planner task, Jira, Polly, Workflows): third-party apps.
- Team and channel admin: new team, join team, new channel, Edit, Manage, tags, email address, Archive, Delete, Add members.
- Communities, Copilot, Meeting Recaps, work location, suggested replies, priority and blocked contacts, missed-activity emails.
- Spell check (Slint has no hook), and Home/End in RTL lines (upstream Slint, see ADR 0001).
- Schedule send. BLOCKED: not offered to this user (2026-11-19): Teams web and the Teams mobile app show no "Schedule message" on the Send button, and the chat service answers `GET /v1/users/ME/drafts` with 404 `LocationLookupFailed` (the user's `teamsstream_drafts` thread does not exist) and `POST` with 404 `ThreadNotFound`. The client is built and unit-tested (`teams_proto::scheduled`: `schedule_message`, `scheduled_messages`, `cancel_scheduled`; `kelma-cli schedule <conversation> <minutes> <text> | list | cancel <id>`). 2026-11-20: Teams web now offers "Schedule message" on this account, so the composer has it (right-click Send or its chevron: "Schedule message" with a day and time picker, the "Select Send to deliver on …" banner, and "Sending on …" bubbles with Edit and Delete; `backend/scheduled.rs`). It uses the same server drafts API and is UNVERIFIED live; a refused schedule puts the text back in the composer with the error.

## Order

1. Phase 5, in order. It is daily friction and mostly local work.
2. 6.1 @mentions, 6.2 new chat and 6.4 presence, which is what a daily user misses most.
3. 7.3 Activity and 7.1 search, which need endpoint discovery.
4. The rest of phases 6–9.

## Endpoints already documented by open-source clients

These items no longer need discovery. Details, bodies and sources are in
`docs/research/open-source.md` §3. Licensing:
- rusty-teams (MIT): code may be reused with attribution.
- mautrix-teams (AGPL): use the facts, not the code.
- better-teams ("MIT + Microsoft exclusion") and teams-lite (no license): facts only.

| Item | Endpoint (chat service `CS` unless noted) | Source |
|---|---|---|
| 5.2 reactions catalog | `emotions` keys `like heart laugh surprised sad angry`, others `<hex>_<name>`; catalog JSON on `statics.teams.cdn.office.net/evergreen-assets/personal-expressions/…`; custom emoji `CSA/api/v1/customemoji/metadata` | rusty, Maxim, m0nkmaster |
| 5.3 forward | `POST /v1/users/ME/conversations/<src>/messages/forward` `{targetThreadIds, messageIds (≤5), additionalMessage, templateId:"basic_forward_message_template"}` | rusty `forward.rs` |
| 5.3 save | `PUT /v1/users/ME/conversations/<conv>/rcmetadata/<rootId>` `{"s":1,"mid":<msgId>}`; list `48:saved` | rusty `saved.rs`, m0nkmaster |
| 5.3 pin | CSA pinbar: `GET/POST/DELETE CSA/api/v1/chats/<chatId>/pinbar` (`pinbarItemType:"pinmsg"`); channel posts `…/channels/<c>/posts/<id>/pin` | rusty `pins/` |
| 5.3 / 5.6 mark unread | `PUT …/properties?name=consumptionHorizonBookmark` `"<id-1>;<now>;<cmid>"`; on read also reset it to `"0;<now>;0"` | rusty, Jezza (verified) |
| 5.4 reply with quote | `<blockquote itemtype=…/Reply itemid=…>` plus `properties.qtdMsgs` JSON string | Jezza, rusty |
| 5.7 drafts | `GET/POST/PUT/DELETE /v1/users/ME/drafts` (`draftType`, `innerThreadId`), `48:drafts` | rusty `drafts.rs` |
| 6.1 mentions | `<readonly …><span itemtype=…/Mention itemid=N>Name</span></readonly>` plus `properties.mentions` JSON string; `mentionType` `person`/`channel`/`team`; tags use `tag:<tagId>`; an edit replaces `properties`, so resend the mentions | xteams, Jezza |
| 6.2 new chat | people: substrate `POST /search/api/v1/suggestions?scenario=peoplepicker.newChat` (needs `x-anchormailbox: Oid:<oid>@<tid>`); create: `POST /v1/threads` with `threadType:"chat"`, `fixedRoster` and `uniquerosterthread` for 1:1 | better-teams, purple |
| 6.3 members | add `PUT /v1/threads/<id>/members/<mri>` `{"role":"Admin","shareHistoryTime":<ms>}`; leave `DELETE` on your own MRI; rename `PUT …/properties?name=topic` | better-teams, purple |
| 6.4 presence | read `POST presence/v1/presence/getpresence/`; push `POST …/pubsub/subscriptions/<epid>`; set `PUT …/me/forceavailability/` (an empty body clears it); note `PUT …/me/publishnote` (`<pinnednote></pinnednote>` suffix = show when messaged); idle `POST …/me/reportmyactivity/`. Writes need the **presence** token | rusty, teams-lite, purple |
| 6.5 profile card | `fetchShortProfile` (body array starting with `""`, SPACES + `x-skypetoken`); Loki `POST …/api/v2/person?smtp=` | teams-lite, m0nkmaster |
| 6.6 pinned banner | CSA pinbar, as for 5.3 | rusty |
| 6.7 read receipts | `GET /v1/threads/<id>/consumptionhorizons`; trouter `ThreadActivity/MemberConsumptionHorizonUpdate`; setting `readReceiptsEnabled` in `/v1/users/ME/properties` | teams-lite, rusty |
| 7.1 search | substrate `POST /searchservice/api/v2/query`, `entityType` Message/People/File/Chat/TeamsChannel; needs `x-anchormailbox`, otherwise it returns 200 with empty results | rusty, m0nkmaster, xteams |
| 7.2 find in chat | the same query with KQL `clientthreadid:<id>` | rusty |
| 7.4 followed threads | `48:threads`; follow and unfollow through the `emotions` key `follow` | m0nkmaster, Maxim |
| 7.5 shared tab | substrate `GET /AllFiles/api/users('OID:<oid>@<tid>')/AllShared?ThreadId=<conv>&ItemTypes=File&ItemTypes=Link` | m0nkmaster |
| 8.1 sections | `GET/POST CSA/api/v1/teams/users/me/conversationFolders` (actions AddItem, RemoveItem, ReorderItems, AddFolder; 412 means a version conflict, so retry) | rusty, better-teams |
| 8.2 mute and hide | mute `PUT …/properties?name=alerts` `{"alerts":"false"}`; hide `unpinnedTime` then `historyHiddenTime` | teams-lite, Jezza (verified) |
| 8.5 channel notifications and hide | `POST /api/nss/<geo>/v1/me/notificationSettings/channels/batch` (audience `uis.teams.microsoft.com`), `PUT …/properties?name=channelNotificationSettings`; hide a channel with `favorite` plus the member `interest` | rusty, Jezza |
| 8.6 channel files | `GET MT/beta/channels/<id>/documentlibraries` or Graph `filesFolder` | Squads |
| 8.8 card actions | `POST CS/agents/<botMri>/invoke` for `Action.Submit` | rusty `cards.rs` |
| 9.4 delivery options | `properties.importance` `high` or `urgent` on send | better-teams, Maxim |
| 9.6 translate | `POST CSA/api/v1/chats/<chatId>/messages/languages/<to>` with `x-ms-use-acs-translation: true`; languages `GET CSA/api/v1/translator/languages` | rusty `translate.rs` |
| 9.7 link previews | `GET <geo>-prod.asyncgw.teams.microsoft.com/urlp/v1/url/info?url=`; attach with `PUT …/messages/<id>/properties?name=links` | rusty `links.rs` |
| 9.8 saved view | `48:saved` (see 5.3) | rusty |
