# backend/composer_apps

## Overview
Composer "+" picker for the open chat's installed apps: debounced composeExtension queries, result rows and card previews, with demo fixtures.

Files:
- `card_preview.rs`: Derives a result-list row (title, subtitle, thumbnail) from a messaging-extension ResultItem, falling back to an Adaptive Card's first two TextBlocks and first Image.
- `fixtures.rs`: Synthetic demo/test data for the composer apps picker: fake chatentitlements, search results, task/submit answers and an app manifest, parsed by the real app_store::bots parsers.

## Interface
- `[card_preview.rs:Preview]` pub struct {title: String, subtitle: String, image: Option<String>}; derives Debug, Clone, PartialEq, Eq
- `[card_preview.rs:preview_of]` pub fn preview_of(item: &app_store::bots::ResultItem) -> Preview; explicit title/subtitle/image win, else Adaptive Card texts/image; empty title becomes "Card"
- `[fixtures.rs:GIF_APP]` pub const &str "demo-gifs"
- `[fixtures.rs:TICKET_APP]` pub const &str "demo-tickets"
- `[fixtures.rs:TASK_URL]` pub const &str "https://tickets.example.invalid/ironteams-demo-task"
- `[fixtures.rs:place_apps]` pub fn place_apps() -> serde_json::Value; chatentitlements-shaped JSON (5 entitlements, 5 definitions; one of them Featured/not installed)
- `[fixtures.rs:query]` pub fn query(app_id: &str, command_id: &str, text: &str) -> QueryOutcome; case-insensitive substring match on canned gifs (GIF_APP/searchGifs) or tickets (TICKET_APP/findTicket); other pairs give an empty result list
- `[fixtures.rs:definition]` pub fn definition(app_id: &str) -> Option<AppDefinition>; manifest only for TICKET_APP (validDomains, webApplicationInfo), None otherwise
- `[fixtures.rs:task]` pub fn task(app_id: &str, command_id: &str) -> TaskOutcome; only (TICKET_APP, reportProblem) gives a continue task, else TaskOutcome::Empty
- `[fixtures.rs:submit]` pub fn submit(app_id: &str, command_id: &str, data: &Value) -> QueryOutcome; only TICKET_APP with createTicket/reportProblem returns an adaptive card "TCK-200 <data.title|Untitled>", else QueryOutcome::Empty

## Invariants
- `[card_preview.rs:MAX_CHARS]` titles/subtitles are at most 120 chars; longer text is cut to 119 chars plus an ellipsis
- `[card_preview.rs:tidy]` whitespace is collapsed to single spaces so every shown line is one line
- `[card_preview.rs:walk]` only TextBlock texts that are non-blank and the first Image with a non-empty url are collected, in document order, recursing through body/items/columns
- `[card_preview.rs:preview_of]` when the item's title is blank the subtitle takes texts[1] (the title consumed texts[0]), otherwise texts[0]
- `[card_preview.rs:preview_of]` item.image takes precedence over the card's image; empty image addresses become None; empty title becomes "Card"
- `[fixtures.rs:module doc]` all data is synthetic (example.invalid hosts) and goes through the real parse_query_response/parse_task_response/AppDefinition::parse
- `[fixtures.rs:query]` an empty or whitespace-only query matches every canned item

## Side Effects
- None

## Verification
- Run: `cargo test -p ironteams`
