# apps

## Overview
Teams apps for IronTeams: the app bar and installed apps, the Store catalog (home, search, definitions, your apps, org catalog), app details, install/uninstall/update requests, app policy and device-permission grants, and messaging-extension bots (compose-extension query, fetch-task, submit-action). Most parsing is pure and lenient (`serde_json::Value` in, typed rows out). Network calls go through `teams_proto::Client`; `reqwest` is used only for `reqwest::Method`. Request builders (`install`, `bar::pin_request`) only build a `Request`; sending is explicit via `api::send`.

## Interface
- Modules (all `pub`): `api`, `bar`, `bots`, `install`, `model`, `settings`, `store` [lib.rs]. `bots` items are not re-exported at the root (use `apps::bots::*`).
- Root re-exports [lib.rs]: `Entitlement, EntitlementState, InstalledApps, PinChange, installed_apps, parse_installed, pin_request`; `Request, Target, install_request, uninstall_request, update_request`; `AppDefinition, DevicePermission, StaticTab, WorksIn, parse_definitions`; `AppPolicy, Grants, app_policy, parse_app_policy`; `AddTarget, AppDetails, CatalogApp, Category, Certification, Distribution, Filter, Price, Rating, Scope, SearchHit, StoreEntry, StoreHome, YourApp, catalog_apps, definitions, org_apps, parse_search, parse_store_home, parse_your_apps, search, store_home, store_home_body, your_apps`.
- `api`: `CALLER = "ironteams"`, `base(&Client) -> String`, `send(&Client, context, &Request) -> Result<Value>`; `read` is crate-private [api.rs].
- `install`: `Request {method, url, body}` with `describe` and `dry_run`; `Target {Me, Team, Chat, Meeting}`; `install_request`, `uninstall_request`, `update_request` [install.rs].
- `bar`: `EntitlementState`, `Entitlement` (`has_update`), `InstalledApps` (`definition`, `app_bar`, `more_apps`), `parse_installed`, `installed_apps`, `PinChange {Pin, Unpin, Move}`, `pin_request` [bar.rs].
- `model`: `WorksIn`, `DevicePermission` (`ALL`, `parse`, `as_str`), `StaticTab`, `AppDefinition` (`parse`, `personal_tabs`, `open_link`, `share_link`), `parse_definitions` [model.rs].
- `store`: `Rating`, `StoreEntry`, `Category`, `StoreHome`, `Price`, `Certification`, `SearchHit`, `YourApp`, `Scope`, `Filter`, `AddTarget`, `AppDetails`, `Distribution`, `CatalogApp`, parsers and async fetchers listed above [store.rs].
- `settings`: `AppPolicy`, `parse_app_policy`, `app_policy`, `Grants` (`allowed`, `set`, `for_app`, `to_json`, `from_json`) [settings.rs].
- `bots`: `Place`, `entitlements_path`, `Bot`, `BotCommand`, `bot_mri`, `CommandKind`, `CommandContext`, `Input`, `CommandParameter`, `TaskInfo`, `ExtensionCommand`, `Extension`, `PlaceApp`, `parse_place_apps`, `place_apps`, `QueryRequest`, `ActionRequest`, `query_request`, `fetch_task_request`, `submit_action_request`, `TaskOutcome`, `parse_task_response`, `fetch_task`, `submit_action`, `query_extension`, `ResultKind`, `ResultItem`, `QueryResults`, `QueryOutcome`, `parse_query_response` [bots.rs].

## Invariants
- `api::send` and `read`: status outside 200..300 gives `Error::status(...)`; an empty body gives `Value::Null`; otherwise JSON-parsed. The "URL must be on the app service host" rule is documented but not checked in code [api.rs:send, read].
- Grants are deny-by-default; `Grants::from_json` silently drops unknown permissions and non-bool values [settings.rs:Grants].
- `parse_app_policy`: missing `isSideloadingAllowed` is false, missing `isUserPinningAllowed` is true [settings.rs:parse_app_policy].
- `Target::Meeting` and `Target::Chat` both map to `chats/{id}`; ids are percent-encoded [install.rs:Target::scope].
- `pin_request` rebuilds the whole pinned list from `app_bar()`, renumbers `appBarOrder` from 1 and is a dry-run builder only [bar.rs:pin_request].
- `is_installed` is true only for `Installed` and `InstalledAndPermanent` [bar.rs:EntitlementState::is_installed].
- `AppDefinition::parse` needs `id`; scope, capability and permission fields are lenient with defaults [model.rs:AppDefinition::parse].
- `store::definitions` with empty ids returns `Ok(vec![])` without a network call [store.rs:definitions].
- `query_extension` refuses commands whose kind is not `Query`; `fetch_task` and `submit_action` do not check kind, so `submit_action` can cause a bot post [bots.rs:query_extension, submit_action].
- `parse_place_apps` keeps definitions with bots or input extensions. The installed filter applies only when the answer carries `value.entitlements`; without it every such definition is kept [bots.rs:parse_place_apps].
- `query_extension` always requests `skip: 0, count: 25` [bots.rs:query_extension].
- Parsers do not panic and default missing fields; some counts use truncating `as u32` casts [store.rs:u32_of, bar.rs:Entitlement::parse].
- Doc note: install, uninstall, update and pin request shapes were taken from web client code and are documented as not verified against a real account [install.rs header].

## Side Effects
- Network through `teams_proto::Client`: `POST beta/users/apps/store`, `POST beta/users/apps/search`, `POST beta/users/apps/batchedDefinitions`, `GET beta/users/apps/store/entitlements`, `POST beta/users/apps/aggregatedEntitlements?appbarview=userpinned`, `GET beta/me/settings/appPolicy`, Graph `appCatalogs/teamsApps` for `org_apps`, per-place `GET` of entitlements, and chat-service `POST /v1/agents/28:<botId>/invoke` for fetch-task, submit-action and query [store.rs, bar.rs, settings.rs, bots.rs]. Calls through `api::send` and `api::read` (app service) add header `x-ms-client-caller: ironteams`; the chat-service bot calls (`fetch_task`, `submit_action`, `query_extension`) send no extra headers [api.rs, bots.rs].
- `api::send` performs install, uninstall, update or pin only when called by the caller [api.rs:send].
- No disk, locks, global state or logging [all files].

## Verification
- Run: `cargo test -p apps`
