Docs synced to commit 931bf3b9 ("fix(settings): a radio row's focus ring clears its dot; the landing shot is taken once the opened page's rows exist", 2026-11-23)

# IronTeams UI (Slint)

`build.rs` compiles `app.slint`; everything else is imported from it. The files are grouped by
layer, from the smallest pieces up:

| Layer | Folder | Holds |
|---|---|---|
| tokens | `tokens/` | `Theme` (region colours, text sizes), `Fixed` (colours that do not follow the theme), `Motion`, shared structs (`types.slint`) |
| stores | `stores/` | globals Rust fills and listens to (`CallStore`, `DriveStore`, `FindStore`, …) and their structs |
| atoms | `atoms/` | the building blocks: buttons, fields, checks, avatars, pills, menus, the search box |
| molecules | `molecules/` | small composites of atoms (cards, pickers, a screen's own widgets) |
| organisms | `organisms/` | the parts of a screen (chat list, composer, call stage, OneDrive items, …) |
| pages | `pages/` | whole screens (Settings, Calendar, OneDrive, Search, the call window, …) |
| app | `app.slint` | the window: state, callbacks and wiring |

## Rules

`bash scripts/check-ui-layers.sh` checks rules 1 and 2, and the region colours rule (see Theme regions); CI runs it in the clippy job.

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

## Theme regions

The window has three colour regions, each a full set of colours: the content area, the nav (the
rail, the sidebar, the chat list) and the call screen. A set is a `Tokens` struct
(`tokens/types.slint`: 49 colours from `bg` to `shimmer`, and `dark`), and `Theme` holds one per
region: `Theme.content`, `Theme.nav` and `Theme.call`. Rust sets all three, with the non-colour
properties (`font-scale`, `compact`, `high-contrast`, `reduced-motion`, `h24`), through one
function, `apply_look` (`src/ui/settings/push.rs`), for the main window and each chat popout.
In the four built-in palettes the nav is the content set on the content's `panel`, and the call
screen is the content set.

- **The `c` input.** A component that draws colour declares `in property <Tokens> c:
  Theme.content;` (the nav set defaults to `Theme.nav`, the call set to `Theme.call`) and reads
  every colour from `root.c`. Inside such a component, every instance of another component that
  takes `c` gets `c: root.c`. Popups opened from the nav float over the content area and use
  `Theme.content`; menus and panels of the call screen use `Theme.call`.
- **Flat aliases.** `Theme.main` (`content.bg`), `Theme.sidebar` and `Theme.card`
  (`content.panel`), `Theme.dark` and one read-only alias per other token (`Theme.fg`,
  `Theme.danger-hover`, …) stay for the pages outside the nav and the call screen.
- **`Fixed`** (`tokens/fixed.slint`): named colours that do not follow the theme, such as
  `Fixed.black` for the video surface, the file-kind colours, the call buttons, the calendar
  slots, the composer's text and highlight colours and the accent choices.
- **The nav set and the call set** are lists at the top of `scripts/check-ui-layers.sh`
  (`nav_set`, `call_set`): the nav is `app_rail`, `apps_rail`, `sidebar`, `lists`,
  `chat_list_extras`, `communities_section` and `web_parity`; the call set is
  `pages/call_window.slint` and the `call_*` organisms with `video_effects` and `video_preview`.
- **Region colours rule** (rule 3 of the script): the files of an area named in `rule3_areas`
  may not read the flat colour aliases or `Theme.dark`; they read their `c` input. The names
  are read from `tokens/theme.slint`. Areas: `atoms` (with `tokens/motion.slint`), `molecules`,
  `nav`, `call`. The rule is switched on area by area as the components take `c`; today no area
  is on.

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
- `SwitchTrack`: the track and knob of a settings switch alone (no label, no click target, hidden
  from screen readers); `SettingsSwitchRow` is the switch it belongs to.

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
- `atoms/radio_row.slint`: `RadioRow` (a radio choice of the call's camera and audio menus and the
  Video effects panel). `atoms/link_row.slint`: `LinkRow` (a menu row that opens more).
  `atoms/range_slider.slint`: `RangeSlider` (0 to 100; the call's speaker volume).
- `atoms/widgets.slint`: `Icon`, `IconButton`, `SectionHeader`, `ReactionChip`,
  `AddReactionChip`. `atoms/reaction_hover.slint`: the `ReactionHover` global.
- `atoms/heading.slint`: `Heading` (`.title` was Recaps' `Heading`, `.section` the App store's
  shelf title, `.group` the shortcut list's `Heading`, `.caption` the video effects' `Heading` and
  the call's More panel `SectionTitle`).

## The Settings kit

`molecules/settings_kit.slint` builds every Settings page; `tokens/space.slint` holds the spacing
scale (`Space.xxs` 2px to `Space.xxl` 32px) and `SettingsMetrics` the kit's sizes (section gap,
row padding, smallest row height, content width), both exported to Rust for the UI tests.

- `SettingsSections`: a page's column of sections, `SettingsMetrics.section-gap` apart.
- `SettingsSection` (`title`, `description`, `danger`): a heading over a card (`Theme.card`, a
  border, rounded); `danger` draws the heading and border in `Theme.danger`.
- `SettingsRow` (`label`, `description`, `enabled`, `stacked`, `divider`): text on the start side,
  the children (the control) on the end side, mirrored for a right-to-left language; `stacked`
  puts the control under the text. A line above every row but the first of its card.
- `SettingsSwitchRow` (`label`, `description`, `checked`, `enabled`, `toggled(bool)`): the whole
  row is the switch: click target, hover tint, Tab focus with a ring, Space or Enter flips it,
  and it carries the accessible name and description (its texts are hidden from screen readers).
- `SettingsChoiceRow` (`options`, `value`, `picked(string)`): pills under the text (`SChoice`).
- `SettingsChoiceRow`'s pills (`SChoice`) list the first option on the start side, also in a
  right-to-left language (the row lists them backwards there; stacked, the first is on top).
- `SettingsButtonRow` (`button-label`, `primary`, `danger`, `button-enabled`, `clicked()`),
  `SPersonRow` (`name`, `action`, `act()`; the button reads "<action> <name>").
- `SettingsLinkRow` (`label`, `description`, `a11y-label`, `clicked()`): a row that goes somewhere,
  with a chevron on the end side; Find in Settings lists its results with it, in one card.
- `SettingsBlock` (free content in a card, stacked) and `SettingsNote` (a muted paragraph).
- `SRadio`: a radio row whose dot sits on the row's start edge, in line with the row labels.

Keyboard: a switch row, a link row, each pill, each radio row and a `settings` button are reached
with Tab and act on Space or Enter. A ring (`focus-ring`) shows only when Tab put the focus there,
never after a click.

Every row with a `label` answers Find in Settings: it reports where it is when its label is the
target, and is tinted (`Theme.selected`) for a moment after the page scrolls to it. Element ids
the tests use: `SettingsSection::card`, `SettingsRow::divider-line`, `SettingsSwitchRow::focus-ring`,
`SChoicePill::focus-ring`, `ButtonBody::focus-ring`.

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

Two things stay larger than the plan's target on purpose: `app.slint` (about 1290 lines, roughly 410 of
them the property, callback and function declarations Rust binds by name) and `Composer` (the text box
and its key handling). Moving them would pass most of their state through new properties and put the
UI tests at risk, so the user accepted them as exceptions.

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
