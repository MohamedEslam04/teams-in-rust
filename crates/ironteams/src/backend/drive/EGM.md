# backend/drive

## Overview
OneDrive Files view: pure `Drive` state (sections, breadcrumbs), `DriveCmd` parsing, instant dispatcher, background load, opening, mutating operations, version history and sharing dialog, reusable file/folder picker, and chat file-chip open planning.

Files:
- `chip.rs`: Chat file-chip open plan (browser/desktop/IronTeams), file card labels, card menu actions (copy/web/download/share) and chip click handling.
- `cmd.rs`: DriveCmd enum of OneDrive view commands, string (name,arg) parser, is_instant classifier, and LinkSettings parser for share-link args.
- `dispatch.rs`: Engine drive_cmd match dispatcher, instant view-state commands, drive_edit/drive_push state helpers and one-shot background load start.
- `edit.rs`: Engine OneDrive mutating ops: new folder/doc/link/notebook/forms, upload (picker/drop), favorite, shortcut, rename, delete, recycle bin, copy link, download.
- `helpers.rs`: Pure helpers for the drive view: time offset, details text, files-pref preview mapping, perm/version/picker/people/recording row builders, sanitize_file_name, preview_of.
- `load.rs`: Engine impl: OneDrive background load (libraries, favorites, For-you cards), section listing, paging, people cache, thumbnail fetching, Meetings aggregation.
- `opening.rs`: Engine impl: open folders/files (inline image/text/markdown preview, paged PDF/Office preview, browser, xdg-open), open location, my-drive-id cache, refresh-and-select.
- `payloads.rs`: Plain data structs sent to the UI for the share dialog, permissions, versions, file picker and preview.
- `picker.rs`: Engine impl: reusable file/folder picker (navigation, selection, choose) and purpose dispatch (move/copy/attach), plus move/copy execution.
- `sharing.rs`: Engine impl: version history/restore and the Share dialog (people search, invite, link creation, permission role change/removal).
- `state.rs`: Pure-data OneDrive view state: sections, breadcrumb trail, listing, selection, sort/group/filter, and conversion to UI row/card data (no network or UI types).

## Interface
- `[chip.rs:ChipOpen]` pub enum {Browser, Desktop, IronTeams}
- `[payloads.rs:PersonData]` pub struct {id (MRI), name, detail, picked, pub(super) email}
- `[state.rs:Section]` pub enum Home|MyFiles|Shared|Favorites|Libraries|Bin|Meetings|SharedByYou|People|Person(String)|Media|Library(String)|Search(String); methods key(), from_key(&str)->Option, title(&[Library])->String, is_browsable()
- `[chip.rs:parse_chip]` pub fn(&str) -> Option<(&str,&str)>; splits `<TYPE>:<url>`, accepts only http(s) urls
- `[payloads.rs:PermData]` pub struct Default {id, who, role(read|write|owner), link, detail, editable}
- `[state.rs:Library]` pub struct {drive_id, name}
- `[chip.rs:chip_open_plan]` pub fn(url, ext, OpenFilesIn) -> ChipOpen; only Office files on SharePoint urls follow the pref, else Browser
- `[payloads.rs:ShareData]` pub struct Default {name, results, picked, perms, role:&'static str, status}
- `[state.rs:Crumb]` pub struct {name, drive_id, item_id} breadcrumb folder
- `[chip.rs:file_card_labels]` pub fn(url, ext, OpenFilesIn) -> (&'static str place, &'static str detail); place OneDrive|SharePoint|""
- `[payloads.rs:VersionData]` pub struct {id, when, by, size, current}
- `[state.rs:FileKind]` pub enum Folder|Word|Excel|PowerPoint|Pdf|Image|Video|Audio|Markdown|Text|Code|Archive|Other; of(&DriveItem) (by extension, then mime major type), as_str()
- `[chip.rs:Engine::file_card_action]` pub(in crate::backend) async fn(&Arc<Self>, action: &str, url: &str) -> Result<()>; actions copy|web|download|share
- `[payloads.rs:PickerData]` pub struct Default {purpose, title, folders_only, rows:Vec<RowData>, crumbs, loading, can_choose}
- `[state.rs:Preview]` pub enum Image|Text|Markdown|Pages|Browser|Player; of(&DriveItem)->Preview (uses preview::can_convert for Office files)
- `[chip.rs:Engine::open_file_chip]` pub(in crate::backend) async fn(&Arc<Self>, arg: &str); falls back to browser on error
- `[payloads.rs:PreviewData]` pub struct Default {title, kind:&'static str (image|text|markdown|pages), image:Option<Pixels>, text, page, pages}
- `[state.rs:TypeFilter]` pub enum All|Word|Excel|PowerPoint|Pdf|More (Default All); parse(&str) (unknown -> All), as_str()
- `[cmd.rs:DriveCmd]` pub enum, ~65 variants (Enter, Nav, OpenRow, Crumb, Refresh, LoadMore, Select..Filter, Search, New*, Upload*, Drop, Open*, Favorite, Rename, MoveTo, CopyTo, Download, Delete, Restore, Share*, Pick*, DialogClose)
- `[picker.rs:Engine::drive_picked]` pub(in backend) async fn(&Arc<Self>, purpose:&str, dest:Option<Crumb>, file:Option<DriveItem>) -> Result<()>; handles move/copy/attach, others logged
- `[state.rs:Columns]` pub struct {modified, modified_by, size, location, activity} (Copy, Default); toggle(&mut self, name) (unknown name no-op)
- `[cmd.rs:DriveCmd::is_instant]` pub fn(&self) -> bool; true for Select/Toggle/SelectAll/ClearSelection/View/Sort/Group/Column/Type/Filter
- `[sharing.rs:Engine::drive_share_open]` pub(in backend) async fn(&Arc<Self>, item:DriveItem) -> Result<()>; opens Share dialog for any item (switches to onedrive view)
- `[state.rs:parse_sort_key]` pub fn(&str)->Option<SortKey> (name|modified|modified-by|size)
- `[cmd.rs:DriveCmd::parse]` pub fn(name, arg) -> Option<DriveCmd>; None for unknown name, bad number (crumb/pick-crumb), bad doc kind, share-perm-role without '|'
- `[load.rs:Engine::drive_background_load]` pub(super) async fn(Arc<Self>); libraries/favorites/cards
- `[state.rs:sort_key_name]` pub fn(SortKey)->&'static str
- `[cmd.rs:LinkSettings]` pub struct {scope: LinkScope, kind: LinkType, block_download: bool, expiry_days: Option<u32>}
- `[load.rs:Engine::drive_go]` pub(super) async fn(&Arc<Self>, Section) -> Result<()>; switch section and load
- `[state.rs:size_label]` pub fn(u64)->String (B / KB ceil / MB,GB one decimal)
- `[cmd.rs:LinkSettings::parse]` pub fn(&str) -> Option<LinkSettings>; `scope,type,block,days`
- `[load.rs:Engine::drive_load]` pub(super) async fn(&Arc<Self>, generation:u64) -> Result<()>; list current section/folder
- `[state.rs:when_label]` pub fn(ms, now_ms, off_ms)->String (time today, Yesterday, 'Nov 10', 'Nov 10, 2025'; empty if ms<=0)
- `[dispatch.rs:Engine::drive_cmd]` pub(in super::super) async fn(&Arc<Self>, DriveCmd) -> Result<()>; main command dispatcher
- `[load.rs:Engine::drive_load_more]` pub(super) async fn(&Arc<Self>) -> Result<()>; next page (Media offset or Graph next link)
- `[state.rs:RowData]` pub struct list row {id,name,kind,modified,modified_by,size,location,activity,selected,favorite,is_folder,shortcut,header,index,thumb:Option<Pixels>} (Clone, Default)
- `[dispatch.rs:Engine::drive_instant]` pub(in super::super) fn(&Arc<Self>, DriveCmd); applies view-state-only commands
- `[load.rs:Engine::drive_meetings_everywhere]` pub(super) async fn(Arc<Self>, generation); channel-library recordings added to Meetings
- `[state.rs:CardData]` pub struct Home 'For you' card {id,title,reason,kind,thumb}
- `[dispatch.rs:Engine::drive_edit]` pub(super) fn(&self, FnOnce(&mut State)); mutate state then push
- `[load.rs:Engine::drive_people]` pub(super) async fn(&self) -> Result<Vec<PersonFiles>>; fetch/cached people list
- `[state.rs:SelectionInfo]` pub struct {count,name,has_folder,all_favorite} (PartialEq)
- `[dispatch.rs:Engine::drive_push]` pub(super) fn(&self); sends State::data to ui.drive
- `[load.rs:Engine::drive_fetch]` pub(super) async fn(&self, &Section, &[Crumb], &[Library], Option<&str>) -> Result<(Vec<DriveItem>, Option<String>)>
- `[state.rs:DriveData]` pub struct everything pushed to UI: section,title,rows,cards,crumbs,quick,grid,sort,ascending,group,columns,type_filter,filter,status,loading,more,selection
- `[dispatch.rs:Engine::drive_status]` pub(super) fn(&self, impl Into<String>); sets status line and pushes
- `[load.rs:Engine::drive_thumbs]` pub(super) fn(&Arc<Self>); background thumbnail fetch for cards and grid tiles
- `[state.rs:State]` pub struct with all pub fields: section, trail, items, next_link, libraries, favorites:HashSet<String>, selected:Vec<String>, grid, sort, ascending, group:Option<SortKey>, columns, type_filter, filter, status, loading, generation:u64, thumbs:HashMap<String,Pixels>, cards:Vec<Card>; Default = Home, Name asc
- `[dispatch.rs:Engine::drive_selection]` pub(super) fn(&self) -> Vec<DriveItem>
- `[load.rs:Engine::drive_load_cards]` pub(super) async fn(Arc<Self>); For-you cards
- `[state.rs:Card]` pub struct {id, item:DriveItem, reason}
- `[dispatch.rs:Engine::drive_one]` pub(super) fn(&self) -> Option<DriveItem>; sets 'Select one item first' status on miss
- `[opening.rs:Engine::drive_card_open]` pub(super) async fn(&Arc<Self>, id:&str) -> Result<()>
- `[state.rs:State::go]` pub fn(&mut self, Section)->u64 sets trail root for MyFiles/Library, resets listing, returns new generation
- `[dispatch.rs:Engine::drive_offline]` pub(super) fn(&self) -> bool; reads self.offline
- `[opening.rs:Engine::drive_open_row]` pub(super) async fn(&Arc<Self>, id:&str) -> Result<()>; Bin rows no-op, People row drills into Person
- `[state.rs:State::open_folder]` pub fn(&mut self,&DriveItem)->u64 pushes crumb and resets listing
- `[dispatch.rs:Engine::drive_start_background]` pub(super) fn(&Arc<Self>); spawns drive_background_load once
- `[opening.rs:Engine::drive_open_item]` pub(super) async fn(&Arc<Self>, DriveItem) -> Result<()>; folder navigation or file preview routing
- `[state.rs:State::open_foreign_folder]` pub fn(&mut self,&DriveItem)->u64 makes folder the root of a Section::Library view

## Invariants
- `[chip.rs:parse_chip]` only http:// or https:// urls accepted; other schemes (javascript:) yield None
- `[load.rs:drive_load/set_listing/drive_thumbs]` Stale-result guard: results apply only if the generation still matches d.s.generation (set_listing returns used; thumb task aborts if generation changed)
- `[state.rs:State::generation]` bumped on every navigation via reset_listing; set_listing/append_listing drop results whose generation differs
- `[chip.rs:chip_open_plan]` non-SharePoint urls and non-Office types always get ChipOpen::Browser regardless of pref
- `[load.rs:drive_load_more]` Media's next_link is a u32 offset string advanced by MEDIA_PAGE (50); other sections use Graph next link with the last trail folder
- `[state.rs:State::reset_listing]` clears items, next_link, selected, filter, status and sets loading=true on every go/open_folder/open_foreign_folder/go_to_crumb
- `[chip.rs:chip_ext]` generic type "FILE"/empty falls back to the extension of the url's last path segment (query/fragment stripped)
- `[load.rs:drive_thumbs]` At most MAX_THUMBS (48) grid tiles, fetched in batches of 4, 3 MiB download cap, decoded to 480x320; skipped entirely in demo mode
- `[state.rs:State::set_listing]` after replacing items, selected is retained only for ids still present
- `[chip.rs:free_path]` never overwrites: tries `name (n).ext` for n in 1..1000, falls back to the original path after that
- `[load.rs:drive_background_load]` At most 12 followed sites are scanned, first drive of each becomes a Library; cards load in a separate spawned task
- `[state.rs:State::selectable]` Section::People is never selectable; select_only/toggle/select_all do nothing there
- `[chip.rs:open_file_chip]` demo/offline mode always uses the browser path; any failure of open_chip_item falls back to the browser
- `[load.rs:drive_fetch]` People/Person sections are handled before the offline check so they behave the same online and in demo
- `[state.rs:State::select_only/toggle]` ignore ids not present in items
- `[chip.rs:open_chip_item]` folders are rejected (bail); IronTeams plan first runs DriveCmd::Enter and show_view("onedrive") because the dialogs live in the Files view
- `[opening.rs:drive_open_item]` Inline Image/Text/Markdown preview only when size <= MAX_PREVIEW_BYTES (15 MiB) and online; Pages preview and external open only when size <= MAX_OPEN_BYTES (100 MiB)
- `[state.rs:State::folder]` returns a crumb only when section.is_browsable() (MyFiles or Library)
- `[cmd.rs:DriveCmd::is_instant]` instant commands do no network and run inline in order; DriveCmd::drive_instant must handle exactly that set
- `[opening.rs:drive_open_row]` Items in Section::Bin are never opened (Restore is the only way back)
- `[state.rs:State::go_to_crumb]` index >= trail.len() changes nothing and returns None
- `[dispatch.rs:drive_cmd Enter]` `entered` flag flips once; only first Enter starts background load and goes to Section::Home
- `[opening.rs:drive_open_browser_preview/drive_open_web]` Only URLs starting with https:// are opened externally
- `[state.rs:Section::newest_first]` Bin/People/Person/Media/Meetings/SharedByYou sort newest first (Reverse modified_ms) while group is None and sort is Name
- `[dispatch.rs:drive_cmd Crumb(0)]` in a Section::Person the first crumb returns to Section::People
- `[opening.rs:drive_preview_page]` Page index clamped to [0, page_count-1]; drive_open_pages requires page_count() > 0 else returns false
- `[state.rs:State::visible]` when grouped by a key other than Size, uses sort_flat so groups stay contiguous (folders not floated); Size grouping uses sort_items
- `[dispatch.rs:drive_cmd Refresh]` bumps `generation` and sets `loading` before loading; Home also spawns drive_load_cards
- `[picker.rs:Picker.generation]` Each navigation bumps p.generation; drive_pick_list applies results only if it still matches and the picker still exists
- `[state.rs:State::rows]` group labels computed on local wall-clock time (modified_ms and now shifted by off); header rows have id 'group:<label>' and header=true; index counts item rows only

## Side Effects
- `[chip.rs:file_card_action]` copy writes the url to the system clipboard via arboard and shows a banner
- `[load.rs:drive_background_load/drive_load_cards]` Microsoft Graph/insights requests via self.client; tokio::spawn for cards task
- `[chip.rs:file_card_action]` download fetches via Graph (browse::item_from_share, ops::download) and writes a file into diag::default_output_dir() via spawn_blocking
- `[load.rs:drive_thumbs]` tokio::spawn task with JoinSet of up to 4 concurrent HTTP downloads plus spawn_blocking image decode; writes thumbs into drive state and pushes UI
- `[chip.rs:drive_open_web_url / helpers.rs:drive_open_web_url]` opens the url in the default browser through crate::external::open
- `[load.rs:drive_load]` tokio::spawn of drive_meetings_everywhere for the Meetings section
- `[dispatch.rs:drive_push]` pushes drive view data to the UI (self.ui.drive); ui.drive_info dialogs for Details and Sync
- `[opening.rs:drive_open_external]` downloads file, creates <cache_dir>/drive-open and writes sanitized file name there, then crate::external::open (xdg-open)
- `[dispatch.rs:drive_start_background]` tokio::spawn of drive_background_load; Refresh on Home tokio::spawn of drive_load_cards
- `[opening.rs:drive_open_browser_preview/drive_open_web/drive_open_location]` opens URLs in the desktop browser via crate::external::open
- `[edit.rs:drive_upload_pick/drive_download]` opens native file dialogs (rfd AsyncFileDialog)
- `[opening.rs:drive_open_pages/drive_show_page]` Office-to-PDF conversion request via Graph; PDF parse and page render on spawn_blocking threads
- `[edit.rs:drive_upload_paths]` reads local files/folders and uploads to OneDrive over Graph, then refreshes the listing
- `[picker.rs:drive_move_copy]` Graph move/copy requests that mutate OneDrive (skipped in demo mode)
- `[edit.rs:drive_new_*/drive_favorite/drive_rename/drive_delete/drive_bin/drive_shortcut]` Graph network mutations (create, follow/unfollow, rename, delete, restore, add shortcut); shortcut failure logged with tracing::warn
- `[sharing.rs:drive_share_send/drive_share_link/drive_perm_role/drive_perm_remove/drive_version_restore]` Graph requests that change sharing or restore versions
- `[edit.rs:drive_new_notebook/drive_new_forms]` opens the notebook web_url / FORMS_NEW_URL in the browser
- `[sharing.rs:drive_share_link]` copies the created link URL to the system clipboard via arboard
- `[edit.rs:drive_copy_link]` writes the item's web_url to the system clipboard via arboard
- `[sharing.rs:drive_share_open]` ui.open_view("onedrive") plus UI pushes (ui.drive_preview/drive_picker/drive_share/drive_versions/drive_dialog) and tracing logs across files

## Verification
- Run: `cargo test -p ironteams`
