# backend/shared

## Overview
Engine actions for the Files tab of the shared-items list (`files.rs`); the Shared tab builder is `../shared.rs`.

Files:
- `files.rs`: Engine actions for the Files tab: row menu (download, copy link, share, rename, delete) and toolbar new folder/document, via onedrive crate Graph calls on a channel's SharePoint folder.

## Interface
- `[files.rs:clean_name]` pub(super) fn(&str) -> Option<String>; trims and validates a SharePoint file/folder name
- `[files.rs:is_sharepoint]` pub(super) fn(&str) -> bool; true for https URLs whose host ends with .sharepoint.com
- `[files.rs:doc_kind]` pub(super) fn(&str) -> Option<DocKind>; maps word/excel/powerpoint to DocKind
- `[files.rs:Engine::shared_show_created]` pub(in crate::backend) fn(self: &Arc<Self>, DriveItem); adds item to the shared list, switches filter to files, highlights it for HIGHLIGHT_MS
- `[files.rs:Engine::shared_copy_link]` pub(in crate::backend) fn(&self, url: &str); copies the URL to the clipboard and sets a note
- `[files.rs:Engine::shared_download]` pub(in crate::backend) async fn(self: &Arc<Self>, url: &str); save dialog then writes file (folder as zip)
- `[files.rs:Engine::shared_share]` pub(in crate::backend) async fn(self: &Arc<Self>, url: &str); creates org-scope view link and copies it
- `[files.rs:Engine::shared_rename]` pub(in crate::backend) async fn(self: &Arc<Self>, arg: &str); arg is `url` U+001F `new name`; optimistic rename with rollback
- `[files.rs:Engine::shared_delete]` pub(in crate::backend) async fn(self: &Arc<Self>, url: &str); deletes to the site recycle bin with optimistic busy state
- `[files.rs:Engine::shared_new_folder]` pub(in crate::backend) async fn(self: &Arc<Self>, name: &str); creates folder in channel files folder, then reloads list
- `[files.rs:Engine::shared_new_doc]` pub(in crate::backend) async fn(self: &Arc<Self>, kind: &str); creates Office doc in channel files folder, then reloads list

## Invariants
- `[files.rs:BAD_NAME_CHARS]` names must not contain " * : < > ? / \ |
- `[files.rs:clean_name]` name is trimmed, non-empty, not starting or ending with '.', at most 255 chars
- `[files.rs:is_sharepoint]` only https scheme accepted; host compared case-insensitively against suffix .sharepoint.com
- `[files.rs:shared_rename]` arg must contain U+001F separator, else silently ignored
- `[files.rs:shared_rename]` no-op when new name equals current name or when offline
- `[files.rs:editable_row]` rows without a drive item (from chat messages) cannot be edited; user gets a note
- `[files.rs:shared_online]` network actions are gated on self.offline (Relaxed load); offline gives note, no request
- `[files.rs:shared_rename]` new name is applied optimistically via rename_now and rolled back via rename_done on server error
- `[files.rs:shared_delete]` row marked busy 'Deleting…' before request; delete_done resolves it by success flag
- `[files.rs:shared_folder]` create actions require shared.folder to be known; otherwise note and abort
- `[files.rs:shared_show_created]` highlight cleared after delay only if shared.highlight still equals the created url
- `[files.rs:find lock usage]` self.find mutex is locked in short scopes and released before push_shared or any await

## Side Effects
- `[files.rs:shared_copy_link]` writes URL to system clipboard via arboard
- `[files.rs:shared_share]` network: onedrive::create_link Graph call; copies resulting link to clipboard
- `[files.rs:shared_download]` opens native save dialog (rfd) and network download via ops::download
- `[files.rs:shared_download]` writes file to chosen path via std::fs::write in spawn_blocking
- `[files.rs:shared_rename]` network: ops::rename on the drive item
- `[files.rs:shared_delete]` network: ops::delete moves item to SharePoint recycle bin
- `[files.rs:shared_new_folder]` network: ops::create_folder, then load_shared reloads the list
- `[files.rs:shared_new_doc]` network: ops::create_office_doc, then load_shared reloads the list
- `[files.rs:shared_show_created]` spawns a tokio task that sleeps HIGHLIGHT_MS then clears highlight and pushes UI
- `[files.rs:shared_note]` mutates self.find shared note state and calls push_shared / ui.shared_filter to update the UI

## Verification
- Run: `cargo test -p ironteams`
