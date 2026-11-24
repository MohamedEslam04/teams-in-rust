# onedrive

## Overview
Library crate (package `onedrive`, `publish = false`) giving IronTeams access to OneDrive and SharePoint files: browsing, meeting recordings, "For you" / "Shared by you" / People insight views, file actions, sharing, and the recycle bin. It is a thin async layer over `teams_proto::Client`. Graph calls use `Audience::Graph` against `https://graph.microsoft.com/v1.0` (`[src/api.rs:graph]`, `graph_raw`); recycle-bin calls use SharePoint REST on the user's `-my.sharepoint.com` host with a SharePoint-audience token (`[src/bin.rs]`), because a token only reaches the host it was minted for. Request builders and parsers are pure; the async functions send them. `src/lib.rs` declares `pub mod api, around, bin, browse, create, insights, model, ops, package, share` and re-exports the main items at the crate root. Dependencies: `futures-util`, `reqwest` (json), `serde_json`, `teams-proto`, `tokio` (time), `url`, `zip` (deflate).

## Interface
- Item addressing (`[src/api.rs]`): `GRAPH`, `ItemRef { drive_id, item_id }` (`my_root`, `new`, `own`, `url`, `child_url`), `graph`, `graph_raw`, `is_status`.
- Model (`[src/model.rs]`): `DriveItem`, `Drive`, `Site`, `Thumbnails`, `Page<T>`, `SearchPage`, `SortKey`, `Group`, `parse_item`, `parse_page`, `parse_search`, `parse_drive`, `parse_site`, `parse_thumbnails`, `sort_items`, `group_items`.
- Browse (`[src/browse.rs]`): `root_children`, `children`, `item_at`, `item_from_share`, `following`, `followed_sites`, `site_drives`, `group_drive`, `my_drive`, `search`, `search_from`, `recent`, `shared_with_me`, `media`, `thumbnails`, `is_media`, `MEDIA_EXTENSIONS`, `search_body`, `path_url`, `media_query`.
- Recordings (`[src/around.rs]`): `recordings`, `channel_recordings`, `meeting_recordings`, `Recording`, `sync_help`, `RECORDINGS_FOLDER`, `MY_FILES`.
- Recycle bin (`[src/bin.rs]`): `recycle_bin`, `restore`, `delete_forever`, `my_site`, `BinItem`, `MySite`.
- Create (`[src/create.rs]`): `create`, `plan` (pure description of the request), `NewItem::{Doc, Link, Notebook}`, `Planned`, `Created`, `Notebook`, `FORMS_NEW_URL`.
- File actions (`[src/ops.rs]`): `create_folder`, `upload_file`, `upload_folder` (`FolderUpload`), `create_office_doc`, `rename`, `move_to`, `copy_to`, `delete`, `download`, `follow`, `unfollow`, `add_shortcut`, `versions` (`Version`), `restore_version`, `preview_url`, and pure body/URL builders and parsers (`folder_body`, `move_body`, `copy_body`, `upload_url`, `parse_monitor`, ...).
- Packaging (`[src/package.rs]`): `DocKind::{Word, Excel, PowerPoint, Markdown, Text, Visio}` (`blank`, `file_name`, `extension`, `default_name`), `zip_bytes`, `link_file`, `link_file_name`.
- Insights (`[src/insights.rs]`): `for_you`, `shared_by_me`, `people`, `insights`, `Insight`, `InsightKind`, `ForYouCard`, `Person`, `PersonFiles`, and pure `for_you_cards`, `shared_by`, `group_by_person`, `parse_*`.
- Sharing (`[src/share.rs]`): `invite`, `create_link`, `permissions`, `update_permission`, `remove_permission`, `Permission`, `Recipient`, `Role`, `LinkScope`, `LinkType`, plus `invite_body`, `link_body`, `parse_permission`.

## Invariants
- `[src/api.rs:ItemRef]` an empty `drive_id` means the signed-in user's own drive (`/me/drive`); `item_id == "root"` maps to `<drive>/root`; `ItemRef::own` uses a shortcut stub's target ids so the target is never touched.
- `[src/api.rs:graph_raw]` any non-2xx status becomes `Error::status`.
- `[src/model.rs:parse_item]` returns `None` without an id or a name; for remote items id, size and URL come from the target while name and modification data prefer the stub.
- `[src/model.rs:sort_items]` folders always sort first regardless of direction; `[src/model.rs:group_items]` groups only consecutive equal labels, so callers sort first.
- `[src/browse.rs]` listings use `$expand=thumbnails&$top=200`; `search` returns only the first 50 hits; `recent` and `shared_with_me` fall back to a search query when the Graph list is empty or errors; `followed_sites` falls back to team sites only on 403; `media` keeps only non-folder items whose extension is in `MEDIA_EXTENSIONS`.
- `[src/around.rs]` a missing Recordings folder (404) yields an empty list; the channel crawl runs at most 8 requests at once (`CRAWL_CONCURRENCY`) and skips failing channels; results are sorted newest first.
- `[src/bin.rs:parse_my_site]` yields `None` unless the SharePoint host has a SharePoint audience; `recycle_bin` is limited to `$top=500` first-stage items; every bin call first resolves the site with an extra Graph call (`my_site`).
- `[src/ops.rs]` uploads never overwrite (`RENAME_ON_CONFLICT`, a numbered name on clash); `upload_file` uses a single PUT for small data and an upload session otherwise; `move_to` stays on the same drive; `copy_to` polls its monitor URL every 1 s at most 120 times and does not follow redirects on the pre-authenticated monitor client; `upload_folder` skips symbolic links, sorts entries, and aborts on any I/O error; `download` of a folder builds a zip in memory.
- `[src/package.rs:zip_bytes]` every entry is deflated; `link_file` strips control characters from the URL; `DocKind::blank` returns an empty `Vec` for Markdown and Text.
- `[src/share.rs:link_body]` an editable link cannot block download; `invite_body` always sets `requireSignIn = true`.
- `[src/insights.rs:insights]` Graph errors are swallowed and yield an empty list (the insight endpoints are documented as deprecated); `for_you_cards` excludes shares by the user, requires a drive item, de-duplicates and stops at `max`; `people` falls back to `/me/drive/sharedWithMe` only when no insight sharers exist.
- `[src/create.rs:plan]` is pure; the Forms case is only a constant URL because Forms has no Graph API for this client.

## Side Effects
- Graph reads and writes on `https://graph.microsoft.com/v1.0`: drive and item listing, search (`POST /search/query`), `/me/insights/*`, `/me/followedSites`, `/me/joinedTeams` and per-team channel/files-folder reads, thumbnails, versions.
- Remote mutations: folder creation, uploads (`PUT .../content`, `createUploadSession`), rename, move, copy (then polling a server-chosen monitor URL with an unauthenticated client), delete (to the recycle bin), follow/unfollow, shortcuts, restore version, `POST /me/onenote/notebooks`, and sharing changes (`invite` may email recipients; `createLink`; permission update and removal).
- SharePoint REST on `{site_url}/_api/web/RecycleBin`: `restore` and `delete_forever` (permanent).
- Disk: `upload_folder` reads a local directory tree into memory (`std::fs::read_dir`, `std::fs::read`); nothing else touches the disk; no environment reads.
- Process-global: `copy_to` calls `teams_proto::client::install_crypto_provider()`; time use is limited to the 1 s poll sleep.

## Verification
- Run: `cargo test -p onedrive`

Docs synced to commit 480e5184 ("fix(media): avatar lookups outside the runtime read the cache only; test helper ignores pending entries", 2026-11-22)
