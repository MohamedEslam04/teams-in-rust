# Composer: images between the lines

Status: implemented on `feat/composer-inline-images` (stacked on `fix/one-message-images`, PR #67).

## Problem

When the user pastes, drops or attaches several images while typing, IronTeams shows them as
chips above the text box. PR #67 makes Send put the text and every image into one message, but
the images always come after the text. Teams' own composer is a rich editor: an image lands where
the cursor is, between two lines of text, and the message keeps that order. A message like
"Before: [screenshot] After: [screenshot]" cannot be written in IronTeams today.

## The composer today

- `ui/organisms/composer.slint` holds one `LineEditor` (`ui/molecules/line_editor.slint`), the
  formatting bar (`composer_format.slint`), the chips (`composer_attachments.slint`) and the
  pickers (mentions, emoji, snippets), all working on one string: the draft (`root.draft`), with
  the cursor and the selection as byte offsets into it.
- `LineEditor` draws one `TextInput` per line (so each line takes its own direction), but keeps
  the text one string; `src/composer_lines.rs` (`LineApi`) maps offsets to lines and back.
- Rust keeps the chips per chat in `Engine::view.attachments` (`backend/attach.rs`). Ctrl+V, a drop
  and the attach menu all end in `Engine::add_attachment`.
- On Send, `backend/send.rs` turns the Markdown-style draft into HTML
  (`render::markdown_to_composer_html`, mentions on top), and with #67 the images ride along in
  one `OutboxKind::Images` outbox item: `backend/outbox.rs` uploads each one (AMS) and sends the
  text's HTML followed by one `ams::image_html` block per image.

## Chosen approach: an image is a line of the draft

Instead of a new block model, the draft string stays the single source of truth, and an image is
a line holding exactly one marker character: `U+E100 + slot` (Private Use Area). Everything that
already works on the draft (cursor offsets, undo of a line, mentions, emoji, snippets, drafts per
chat, the send path) keeps working, and the order of text and images is simply the order of the
lines.

- **Insert.** `add_attachment` gives an image a slot and asks the UI to insert its marker line at
  the composer's cursor (`UiHandle::insert_inline_image`). The cursor is mirrored into
  `InlineImageStore.caret` by the composer. The line the cursor is in is split there: `ab|cd`
  becomes `ab`, image, `cd`; at the end of a line the image goes after it and an empty line
  follows for typing. The cursor ends after the image, so several images pasted in a row keep
  their order.
- **Draw.** `LineEditor` asks `LineApi.image-slot(line)` for every line. A line that is a marker of
  a live image (`InlineImageStore.images[slot].live`) is drawn as the thumbnail with an X button.
  Its `TextInput` stays (so the cursor can rest on it and arrows walk over it) but is read-only
  and its text transparent; the thumbnail is outlined in the accent colour while the cursor is on
  it.
- **Edit.** On an image line, Backspace, Delete and the X remove the image line; Up and Down (and
  Enter) move to the line before or after. Backspace at the start of the line after an image, or
  Delete at the end of the line before one, removes the image too, as in Teams.
- **Files** keep their chips. So do images added while a sent message is being edited (an edit
  cannot add images).

### Data model

`attach::Attachment` gets `slot: Option<u32>`. An image with a slot is not a chip; it is drawn in
the text. `push_attachments` sends the chips (attachments without a slot) and the inline images
(`[InlineImage { preview, live }]`, indexed by slot) to the UI. Slots come from a counter
that runs while the app runs and skips the slots the chat still uses, so an image put back after
a failed send never meets a newer image with the same slot.

The marker range is `U+E100..U+F0FF` (4096 slots). It stays clear of `U+E000` and `U+E001`,
which the mention code uses around its placeholders, and of `U+F700..U+F7FF`, where
Slint encodes special keys.

### Send format

1. The draft's image lines whose image is gone (removed, or from before a restart) are dropped,
   and images no line refers to any more are forgotten.
2. Each remaining image line becomes a placeholder line (`U+FFFC`), the images are put in the
   order of their lines, and image chips (if any) follow.
3. The Markdown conversion gives each placeholder line its own `<p>`. In `outbox.rs`
   (`inline_images::fill_placeholders`) the k-th placeholder paragraph is replaced by the k-th
   `ams::image_html` block after upload. A placeholder that is not a whole paragraph (inside a
   list or a code block) becomes the bare `<img>`; images without a placeholder go at the end,
   which is exactly #67's format for items queued before this change.

So the message is one `OutboxKind::Images` item, one send under one client message id:
`<p>before</p><p><img …></p><p>after</p><p><img …></p>`. The pending row shows "🖼 name" lines
in the same places. A message with app cards cannot hold images, so there (as in #67) the images
go first as a message of their own and the placeholders are dropped from the card message.

## Edge cases

- **Drafts.** The draft text (with markers) is saved as before. The images themselves live in
  memory only, per chat: switching chats keeps them, a restart drops their lines when the draft
  comes back (`restore_draft`). The chat list shows a marker as "🖼".
- **Failed queue.** The draft (with its markers) and the images (with their slots) go back to the
  composer, so the images reappear in place.
- **Copy and paste of the text** copies markers. A marker line that duplicates an image already
  placed is dropped at send; a marker pasted inside a line is not an image line and is dropped at
  send.
- **Spell check** draws its underlines from one hidden input of the whole text, which does not
  know an image line is taller; while the draft has images the underlines are off.
- **Formatting preview** shows the marker as an unknown character.
- **Select all** (Ctrl+A) then Backspace or typing removes the images with the text.

## Non-goals

- A rich text editor (images inside a line, resizing, captions).
- Editing a sent message's images, or adding images to an edit.
- Keeping pasted images across a restart (that would mean copying them to the cache on paste).
- Files placed between lines (they stay chips, as in Teams).

## Trade-offs

A real block model (a list of text blocks and image blocks, one `LineEditor` per text block) was
the alternative. It would make the pickers, the formatting toolbar, spell check, undo and the
cursor plumbing work across several editors, a large change to a composer that works. The marker
line keeps one string and costs a reserved character range, a few key rules in `LineEditor` and
the two limits above (spell underlines, preview).

## Test plan

- `support/inline_images.rs` unit tests: insertion at the start, middle and end of a line and in
  an empty draft; image-line detection; placeholders in order with dead and duplicate lines
  dropped; filling placeholders (whole paragraphs, inline, extra images appended, none left).
- `backend/outbox.rs`: the message HTML is text, image, text, image for a draft with two images,
  through the real Markdown conversion; the pending row shows the names in place.
- UI tests (`ui_tests/composer_inline_images.rs`): an image added while the cursor is inside a line
  splits it and is drawn as an image line; typing goes after it; the X and Backspace remove it;
  Send queues one message whose pending row has text and image names in order; files stay chips.
