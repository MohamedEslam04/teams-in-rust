//! OneDrive: the Files view of the rail. The pure view state lives in `state`, the commands in
//! `cmd`; this file runs them against `crates/onedrive` (or an in-memory demo tree offline) and
//! pushes the result to the UI.

mod chip;
pub(super) use chip::file_card_labels;
mod cmd;
pub(crate) mod state;

pub use cmd::DriveCmd;
use cmd::LinkSettings;
use state::{Card, Crumb, FileKind, Library, Preview, Section, State};
pub use state::{DriveData, RowData};

use super::Engine;
use crate::data::Pixels;
use onedrive::{
    DriveItem, ItemRef, NewItem, Permission, PersonFiles, Recipient, Role, Version, bin, browse,
    insights, ops, share,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use teams_proto::time::now_ms;

mod payloads;
pub use payloads::*;
mod helpers;
pub use helpers::*;
mod dispatch;
mod edit;
mod load;
mod opening;
mod picker;
mod sharing;

/// Pictures and text bigger than this open in the browser instead of IronTeams's viewer.
const MAX_PREVIEW_BYTES: u64 = 15 * 1024 * 1024;
/// How much of a text file the viewer shows.
const MAX_TEXT_CHARS: usize = 200_000;
/// Files bigger than this are not downloaded to be opened by a desktop app.
const MAX_OPEN_BYTES: u64 = 100 * 1024 * 1024;
/// Uploads are read into memory; refuse what would not fit comfortably.
const MAX_UPLOAD_BYTES: u64 = 250 * 1024 * 1024;
/// Thumbnails are fetched for this many items of a listing.
const MAX_THUMBS: usize = 48;
/// Cards in Home's "For you" row.
const MAX_CARDS: usize = 8;
/// Photos and videos per page of Browse files by Media.
const MEDIA_PAGE: u32 = 50;
const SHARE_HELP: &str = "Choose people and what they can do, or create a link.";

struct Picker {
    purpose: String,
    folders_only: bool,
    trail: Vec<Crumb>,
    items: Vec<DriveItem>,
    selected: Option<String>,
    loading: bool,
    generation: u64,
}

struct Share {
    item: DriveItem,
    role: Role,
    results: Vec<PersonData>,
    picked: Vec<PersonData>,
    perms: Vec<Permission>,
    status: String,
}

/// A document open page by page in the preview dialog.
struct OpenPages {
    doc: Arc<preview::Document>,
    name: String,
    /// The page shown, from 0.
    page: usize,
}

/// How wide a page is rendered, in pixels.
const PAGE_WIDTH: u32 = 1000;

/// The OneDrive part of the engine's state.
#[derive(Default)]
pub struct Drive {
    s: State,
    entered: bool,
    /// Bumped per share-dialog people search; an older, slower answer is dropped.
    share_seq: u64,
    /// Libraries, favorites and the "For you" cards load once, from whichever feature needs
    /// them first (the OneDrive view or the cloud-file picker).
    background_started: bool,
    /// Who shared files with the user, from the last People listing (a person's files open
    /// from it).
    people: Vec<PersonFiles>,
    /// The PDF (or converted Office file) open in the preview dialog.
    pages: Option<OpenPages>,
    my_drive_id: Option<String>,
    picker: Option<Picker>,
    share: Option<Share>,
    versions_of: Option<DriveItem>,
}

/// Where a thumbnail comes from: a listing that carries its address, or the item itself.
#[derive(Clone)]
enum ThumbSource {
    Url(String),
    Item(ItemRef),
}

/// `drive_id` of the picker's "Recent" and "Shared with me" entries: lists, not folders.
const VIRTUAL: &str = "@virtual";

#[cfg(test)]
mod tests;
