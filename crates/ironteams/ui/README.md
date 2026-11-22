# IronTeams UI (Slint)

`build.rs` compiles `app.slint`; everything else is imported from it. The files are grouped by
layer, from the smallest pieces up:

| Layer | Folder | Holds |
|---|---|---|
| tokens | `tokens/` | `Theme` (colours, presence, scrim, text sizes), `Motion`, shared structs (`types.slint`) |
| stores | `stores/` | globals Rust fills and listens to (`CallStore`, `DriveStore`, `FindStore`, …) and their structs |
| atoms | `atoms/` | the building blocks: buttons, fields, checks, avatars, pills, menus, the search box |
| molecules | `molecules/` | small composites of atoms (cards, pickers, a screen's own widgets) |
| organisms | `organisms/` | the parts of a screen (chat list, composer, call stage, OneDrive items, …) |
| pages | `pages/` | whole screens (Settings, Calendar, OneDrive, Search, the call window, …) |
| app | `app.slint` | the window: state, callbacks and wiring |

## Rules

`bash scripts/check-ui-layers.sh` checks both of these; CI runs it in the clippy job.

1. **No upward imports.** A file imports only from its own layer or a lower one:
   `tokens`, `stores` ← `atoms` ← `molecules` ← `organisms` ← `pages` ← `app.slint`.
   When an organism needs something from a page, the shared piece moves down (a struct or global
   to `stores/`, a widget to an organism or lower).
2. **Colours live in `tokens/` and `atoms/`.** Elsewhere use a `Theme` token or an atom variant.
   The colours screens still draw themselves are listed in `scripts/ui-hex-allowlist.txt`
   (the composer's colour palette, drop shadows, brand colours, the always-dark call surfaces,
   and the rest of what the screens drew before this layout); the list may only shrink. When you
   move a colour into a token, delete its line.
3. **Use the atom, don't copy it.** A control that looks a little different is a new variant of
   the atom (an `enum` value), not a new component. Callback and property names that Rust binds
   (`src/ui/*.rs`) and the component and element names the UI tests look for
   (`src/ui_tests/`) are part of the contract: rename them only together with their users.

Not policed: the `px` literals. A length used in many places becomes a token when it earns it.

## Atom catalog

Every atom draws each variant exactly as the copy it replaced. The old names are listed so a
search for them lands here.

### `atoms/button.slint`: `Button`, `FlexButton`, `ButtonVariant`

`Button` is as wide as its label (and icon); `FlexButton` is the same button sized by the
layout around it (its label width is only the preferred and minimum width). Properties:
`label`, `icon`, `enabled`, `primary`, `danger`, `active`, `chevron`, `tip`, `checkable`, and the
sizing overrides `base-width`, `pad`, `text-size`. Callback: `clicked()`. Out: `has-hover`.

| Variant | Was | Look |
|---|---|---|
| `dialog` | `TButton` (Teams dialogs) | 32px, bold, filled; `danger` red with a hover |
| `settings` | `SButton` | follows `Theme.font-body`, grows with the text |
| `toolbar` | `CalButton` (use `FlexButton`) | 28px; `active` marks an open menu; `danger` is a red outline |
| `kit` | `KitButton` (app screens) | optional icon and chevron, outlined, transparent |
| `command` | `CmdButton` (OneDrive command bar) | as `kit`, no outline |
| `accent-outline` | `CardBtn` (adaptive cards) | accent outline |
| `card` | `CardButton` (meeting cards) | 28px, small bold label |
| `flat` | `DialogButton` (OneDrive dialogs) | flat, bold only when primary |
| `toggle` | `ReaderButton` (Immersive Reader) | `active` fills it |
| `subtle` | `RowButton` (reminders) | small outlined label |
| `solid` | `HeldButton`, `RowButton` (call roster) | 26px, small bold label, filled |
| `action` | `CallAction` (call screens) | 34px text button |
| `call` | `CallButton` (call top bar) | icon over label, hover tooltip (`tip`) |

### `atoms/field.slint`: `Field`, `FieldVariant`

Properties: `label`, `placeholder`, `text` (in-out; a binding is the starting text), `multiline`,
`field-height`, `autofocus`. Callbacks: `edited(string)`, `accepted(string)` (Enter),
`committed(string)` (Enter or focus leaving). Function: `focus-input()`.

| Variant | Was |
|---|---|
| `dialog` | `TField` (Teams dialogs; `initial` became `text`) |
| `kit` | `KitField` (app screens' dialogs) |
| `settings` | `SField` (`initial` became `text`) |
| `bare` | `CalField` (calendar; no label, `placeholder` is the accessible name) |

### `atoms/check.slint`: `Check`, `CheckBox`, `Switch`

- `Check` (`CheckVariant.dialog` was `TCheck`, `.toolbar` was `CalCheck`): a box and its label;
  flips `checked` itself, then `toggled(bool)`.
- `CheckBox` (`.square` was `LoopCheck`, `.round` was `TaskCheck`, `.drive` was OneDrive's
  `CheckBox`): the box alone; its owner keeps the state, `toggled(bool)` asks for the other one.
- `Switch` (`.settings` was `SSwitch`, `.panel` was `FxSwitch`, `.row` was the call roster's
  `SwitchRow`, `.icon` was the pre-join `MicSwitch`): state in `checked` (was `on` on Settings);
  `settings` flips it itself, the others leave it to their owner. The `icon` variant's owner
  sets `width: self.natural-width`.

### `atoms/avatar.slint`: `Avatar`, `PresenceDot`, `GlyphAvatar`

`Avatar`: a photo or initials on `tone`, with an optional `presence` dot. `corner` squares it
(team pictures), `text-size`/`text-weight` size the initials, `tone-under-img` keeps the tone
behind the photo, `photo-label` names the photo. It replaced the hand-drawn circles of the
Seen-by list, the readers under a message (`ReaderAvatar` keeps its slide-in animation around
it), the chat list's channel and team pictures and the Teams page's team tiles.
`PresenceDot` is the availability dot.

### `atoms/chip.slint`: `Pill`, `Badge`

- `Pill` (`.filter` was `Pill`, `.toggle` was `KitPill`; the toggle's owner sets
  `width: self.natural-width`).
- `Badge` (`.status` was `KitBadge`, tone "", ok, bad, warn; `.tag` was the Teams page's `Chip`,
  tone "", warn, accent).

### `atoms/menu.slint`: `MenuItem`, `MenuFrame`, `MenuColumn`

`MenuItem` variants: `plain` (chat list `MenuItem`), `dialog` (`TMenuItem`), `share`
(`ShareMenuItem`), `spell` (`SpellMenuRow`), `file` (`FileMenuItem`), `card` (`CardMenuRow`),
`format` (`FmtItem`; `hint` is its accessible name), `icon` (top bar `MenuItem`), `header`
(chat header `MenuItem`), `parity` (`PMenuItem`), `message` (message menu `MenuItem`),
`calendar` (`CalMenuItem`), `kit` (`KitMenuRow`), `drive` (OneDrive `MenuRow`), `list` (chat
list `MenuRow`). The row carries the accessible name; its texts are hidden from screen readers.
`MenuFrame` (`.parity` was `PMenuCard`, `.card` was `CalCard`) is the card a menu sits on;
`MenuColumn` (was `KitMenuFrame`) also lays the rows out and fits their height.

### Other atoms

- `atoms/search_box.slint`: `SearchBox` (was the Teams page's and OneDrive's `SearchBox` and
  `KitSearch`; `clip-placeholder` elides the placeholder, as OneDrive did).
- `atoms/scrim.slint`: `Scrim` (`.clear` was `CalScrim`, `.dim` was the calendar's `Modal`).
- `atoms/dialog_frame.slint`: `DialogFrame` (OneDrive's titled panel) and `ModalDialog`
  (was the Teams `TFrame`: dims the window, centres the panel, shows an error band).
- `atoms/file_kind.slint`: `KindBadge` and `DriveLook` (a file kind's tag and colour).
- `atoms/widgets.slint`: `Icon`, `IconButton`, `SectionHeader`, `ReactionChip`,
  `AddReactionChip`. `atoms/reaction_hover.slint`: the `ReactionHover` global.
- `atoms/heading.slint`: `Heading` (`.title` was Recaps' `Heading`, `.section` the App store's
  shelf title, `.group` the shortcut list's `Heading`, `.caption` the video effects' `Heading` and
  the call's More panel `SectionTitle`).

## The window and the composer

`app.slint` declares what Rust binds (the `AppWindow` properties, callbacks and functions) and
composes the window from organisms; their state goes in through properties (`<=>` when they
write it) and callbacks:

- `AppKeys` (`organisms/app_keys.slint`): the keyboard shortcuts and Esc, around the content.
- `AppTopBar`, `AppRail`: the top bar and the app bar.
- `ChatPaneHeader`, `MessagePane`, `NoticeBanner`: the conversation's header, messages and notice.
- `organisms/chat_overlays.slint`: the message menu, Seen by, reaction picker, Forward,
  participants and person-card layers.
- `organisms/app_dialogs.slint`: the small dialogs over a scrim (leave, remove history, add a
  tab, leave a channel, reminders, section name, the OneDrive picker).

`Composer` keeps the text box, its keys and the popups' state; `ComposerToolbar`,
`ComposerAttachments`, `ImportanceTag` and `organisms/composer_popups.slint` (emoji suggestions,
the emoji panel, the "…" menu, the Attach menu) draw the rest.

Element ids the UI tests use are `Component::id`: an element that moves into an organism is
found as `Organism::id` (for example `MessagePane::msg-list`).

## Not merged yet

Copies that keep their own component, because merging them would change how they look or
behave, or because they carry screen logic. Each has a name of its own:

- Chips with screen logic: Find's `SearchFilterChip` (with a clear button), the send menu's
  `QuickChip`, the video effects' `FilterChip`, Settings' `SChoicePill` and `DayChip`, `TChoice`.
- Tabs: the Teams page's `TabButton` and Recaps' `RecapsTabButton` (different sizes, and the
  Teams tab's label is a text for screen readers).
- Menu rows with their own behaviour: the profile menu's `StatusMenuRow` (keyboard focus,
  presence glyph), the call window's `TileMenuRow` (list items with a focus ring), the send
  menu's `SendOptionRow`, the chat list's `OptionRow`, the activity menu's `MenuEntry`.
- Screen pieces that only shared a name: `CalendarMoreMenu` and the top bar's `MoreMenu`,
  `CalendarShareDialog` and `DriveShareDialog`, the Teams dialogs' `MemberLine` and the Calls
  page's `PersonLine`, Viva Engage's `EngagePostCard` and the channel's `PostCard`.
- The desktop notification card's `CardButton` (`organisms/message_toast.slint`).
