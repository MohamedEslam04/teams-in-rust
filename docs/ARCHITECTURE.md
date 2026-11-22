Docs synced to commit df8c757f ("Merge remote-tracking branch 'origin/dev' into feat/arch-split-files", 2026-11-22)

# Architecture

How the `ironteams` GUI crate is layered, what the checks enforce, and where the numbers started.
The layering is enforced by `crates/ironteams/src/architecture_tests.rs` (runs in `cargo test`).

## Rust layers

```
        ui  ─────────────►  ui_api  ◄─────────────  backend
         │                    ▲                        ▲
         │   support ◄────────┴────────────────────────┤   (both use support)
         └─────────── backend::Cmd (the one allowed edge) ─┘
```

- `ui_api` (`src/ui_api.rs`): what the engine needs from the UI thread, the `UiHandle` and the view
  data its updates carry. The handle's methods are written next to the screens they fill
  (`impl UiHandle` blocks in `ui/*.rs`).
- `support` (`src/support/`): helpers both sides use without depending on each other (error
  wording, quiet rules).
- `backend`: the engine. It reaches the UI only through `crate::ui_api`. It never names `crate::ui`.
- `ui`: the Slint glue. It uses `ui_api`, `support` and `backend::Cmd`, and sends commands to the
  engine.

`Cmd` stays in `backend`. It has about 25 variants, and each one wraps a command enum owned by a
backend feature (`CallCmd`, `DriveCmd`, `FindCmd`, `PeopleCmd`, `SettingsCmd`, ...) whose payloads
use backend types. Moving it would drag those types, and then the whole backend, into a neutral
module, so the UI-to-backend edge is kept narrow and explicit instead: `ui` depends on `Cmd` and
nothing else of the engine.

### What the test enforces

`crates/ironteams/src/architecture_tests.rs` is plain std (a unit test of the crate, so no second
build of the binary):

1. No file under `src/backend/**` contains `crate::ui` unless it is followed by an identifier
   character, that is `crate::ui_api` passes and `crate::ui::x` or `crate::ui;` fails.
2. `src/ui_api.rs`, `src/support.rs` and `src/support/**` contain neither `crate::backend` nor
   `crate::ui::`.

### Baseline

| Measure | Before | After |
|---|---|---|
| backend files that name `crate::ui` | 14 files, 17 references | 0 |

`cargo machete` runs in the Linux clippy job of `.github/workflows/ci.yml` and finds no unused
dependencies.

## File size

The god-files were split by responsibility, as pure moves: every submodule is re-exported from its
parent (`pub use child::*`), so outside paths did not change, and `#[cfg(test)]` blocks moved into a
`tests` submodule of the same file. Target: a file under about 600 lines.

| File | Before | Parent after | New files | Largest new file |
|---|---|---|---|---|
| `backend/call.rs` | 3742 | 65 | 14 | 584 (`model_state.rs`) |
| `backend/drive.rs` | 2648 | 114 | 9 | 403 (`edit.rs`) |
| `backend/call/live.rs` | 2495 | 158 | 11 | 390 (`run.rs`) |
| `backend/cal/actions.rs` | 2492 | 29 | 9 | 435 (`event_ops.rs`) |
| `ui/cal.rs` | 2473 | 34 | 10 | 309 (`peek.rs`) |
| `ui/settings.rs` | 2121 | 123 | 6 | 784 (`command.rs`) |
| `backend/settings.rs` | 2308 | 56 | 8 | 529 (`tests.rs`) |
| `backend/people.rs` | 2079 | 48 | 9 | 389 (`members.rs`) |
| `backend/find.rs` | 2014 | 46 | 7 | 453 (`quick_lists.rs`) |

The one file over the target is `ui/settings/command.rs`: it is a single `command` function (one
`match` over the setting names), and a pure move cannot make it smaller. Splitting its arms into
functions is a follow-up that edits code, so it is left alone here.

## Slint layers

The `.slint` files under `crates/ironteams/ui/` import in one direction only:

```
tokens, stores  ◄─  atoms  ◄─  molecules  ◄─  organisms  ◄─  pages  ◄─  app
```

A layer imports from the layers to its left and never from the right. Stores (`*_store.slint`) hold
state and callbacks and sit beside the tokens, so any layer can read them; atoms are the smallest
controls, molecules combine atoms, organisms are whole panels, pages are screens, and `app.slint`
assembles the window.

The rule is checked by `scripts/check-ui-layers.sh` (a CI step) and written up in
`crates/ironteams/ui/README.md`.
