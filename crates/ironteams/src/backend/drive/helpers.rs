use super::*;

// -- pure helpers --------------------------------------------------------------------------

/// Local time minus UTC at `ms`, in milliseconds, in the time zone chosen in Settings.
pub(super) fn local_offset(ms: i64) -> i64 {
    appearance::formats::offset_ms(ms)
}

/// Folders of a drive path, outermost first: `/Docs/Reports` gives `Docs`, `Docs/Reports`.
pub(super) fn path_prefixes(parent_path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for seg in parent_path.split('/').filter(|s| !s.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(acc.clone());
    }
    out
}

/// The lines of the Details dialog.
pub(super) fn details_text(i: &DriveItem, favorite: bool) -> String {
    let when = |ms: i64| {
        if ms <= 0 {
            "unknown".to_owned()
        } else {
            chrono::DateTime::from_timestamp_millis(ms + local_offset(ms)).map_or_else(
                String::new,
                |t| {
                    format!(
                        "{} {}",
                        t.format("%Y-%m-%d"),
                        appearance::formats::time_of(&t)
                    )
                },
            )
        }
    };
    let kind = FileKind::of(i).as_str();
    let mut lines = vec![
        format!("Name: {}", i.name),
        format!("Type: {}", if i.is_folder { "Folder" } else { kind }),
    ];
    if i.is_folder {
        lines.push(format!("Items: {}", i.child_count));
    } else {
        lines.push(format!("Size: {}", state::size_label(i.size)));
    }
    lines.push(format!(
        "Location: {}",
        if i.parent_path.is_empty() {
            "/"
        } else {
            &i.parent_path
        }
    ));
    lines.push(format!(
        "Modified: {} by {}",
        when(i.modified_ms),
        non_empty(&i.modified_by)
    ));
    if !i.created_by.is_empty() {
        lines.push(format!("Created by: {}", i.created_by));
    }
    if i.is_remote_shortcut {
        lines.push("Shortcut or shared item".into());
    }
    lines.push(format!("Favorite: {}", if favorite { "yes" } else { "no" }));
    if !i.web_url.is_empty() {
        lines.push(format!("Web address: {}", i.web_url));
    }
    lines.join("\n")
}

pub(super) fn non_empty(s: &str) -> &str {
    if s.is_empty() { "unknown" } else { s }
}

/// Settings > Files and links: where Word, Excel and PowerPoint files open. The browser and the
/// desktop app replace IronTeams's own preview; every other file keeps its preview.
pub(super) fn with_files_pref(
    pref: super::super::settings::OpenFilesIn,
    preview: Preview,
    kind: FileKind,
) -> Preview {
    use super::super::settings::OpenFilesIn;
    if !matches!(
        kind,
        FileKind::Word | FileKind::Excel | FileKind::PowerPoint
    ) {
        return preview;
    }
    match pref {
        OpenFilesIn::IronTeams => preview,
        OpenFilesIn::Browser => Preview::Browser,
        // Downloads the file and hands it to the desktop's default app.
        OpenFilesIn::Desktop => Preview::Player,
    }
}

/// Opens a web address in the browser (only logs in demo mode and tests).
pub(super) fn drive_open_web_url(url: &str) {
    let _ = crate::external::open(url);
}

/// `expirationDateTime` for a link that lasts `days` days.
pub(super) fn link_expiry(days: u32, now: i64) -> String {
    calendar::format_iso_ms(now + i64::from(days) * 86_400_000)
}

pub(super) fn role_str(r: Role) -> &'static str {
    match r {
        Role::Read => "read",
        Role::Write => "write",
    }
}

pub(super) fn parse_role(s: &str) -> Role {
    if s == "read" { Role::Read } else { Role::Write }
}

/// A directory person as the invite call wants them: by e-mail, else by object id.
pub(super) fn recipient_of(p: &PersonData) -> Option<Recipient> {
    match (&p.email, teams_proto::people::oid_of_mri(&p.id)) {
        (Some(e), _) if !e.is_empty() => Some(Recipient::Email(e.clone())),
        (_, Some(oid)) => Some(Recipient::ObjectId(oid.to_owned())),
        _ => None,
    }
}

pub(super) fn perm_data(p: &Permission, now: i64) -> PermData {
    let owner = p.roles.iter().any(|r| r == "owner");
    let role = if owner {
        "owner"
    } else if p.roles.iter().any(|r| r == "write") {
        "write"
    } else {
        "read"
    };
    let link = p.link_scope.is_some();
    let who = if link {
        match p.link_scope.as_deref() {
            Some("anonymous") => "Anyone with the link".to_owned(),
            Some("organization") => "People in your organization with the link".to_owned(),
            _ => "People you chose, with the link".to_owned(),
        }
    } else if p.granted_to.is_empty() {
        "Unknown".to_owned()
    } else {
        p.granted_to.join(", ")
    };
    let mut detail = Vec::new();
    if p.inherited {
        detail.push("inherited".to_owned());
    }
    if p.link_type.as_deref() == Some("blocksDownload") {
        detail.push("download blocked".into());
    }
    if let Some(e) = p.expires_ms {
        detail.push(format!(
            "expires {}",
            state::when_label(e, now, local_offset(e))
        ));
    }
    PermData {
        id: p.id.clone(),
        who,
        role: role.into(),
        link,
        detail: detail.join(", "),
        editable: !owner && !p.inherited,
    }
}

pub(super) fn version_data(versions: &[Version], now: i64) -> Vec<VersionData> {
    versions
        .iter()
        .enumerate()
        .map(|(n, v)| VersionData {
            id: v.id.clone(),
            when: state::when_label(v.modified_ms, now, local_offset(v.modified_ms)),
            by: v.modified_by.clone(),
            size: state::size_label(v.size),
            current: n == 0,
        })
        .collect()
}

pub(super) fn bin_item(b: bin::BinItem) -> DriveItem {
    DriveItem {
        id: b.id,
        name: b.name,
        size: b.size,
        is_folder: b.is_folder,
        modified_ms: b.deleted_ms,
        modified_by: b.deleted_by,
        parent_path: format!("/{}", b.dir.trim_matches('/')),
        ..Default::default()
    }
}

pub(super) fn library_item(l: &Library) -> DriveItem {
    DriveItem {
        id: "root".into(),
        drive_id: l.drive_id.clone(),
        name: l.name.clone(),
        is_folder: true,
        ..Default::default()
    }
}

pub(super) fn my_files_item() -> DriveItem {
    DriveItem {
        id: "root".into(),
        name: "My files".into(),
        is_folder: true,
        ..Default::default()
    }
}

/// What the People list calls a person.
pub(super) fn person_label(p: &onedrive::insights::Person) -> String {
    [&p.name, &p.address]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "Unknown".into())
}

/// The People list as rows: one per person, opening to their files.
pub(super) fn people_items(people: &[PersonFiles]) -> Vec<DriveItem> {
    people
        .iter()
        .enumerate()
        .map(|(n, p)| DriveItem {
            id: format!("person:{n}"),
            name: person_label(&p.person),
            is_folder: true,
            child_count: u32::try_from(p.files.len()).unwrap_or(u32::MAX),
            modified_ms: p.latest_ms,
            ..Default::default()
        })
        .collect()
}

/// "Shared by you" as rows. Only drive items can be acted on, so shares of other things (a
/// page, a mail attachment) are left out; the sharer is the user, so no name is shown.
pub(super) fn shared_by_you_items(shared: Vec<insights::Insight>) -> Vec<DriveItem> {
    shared
        .iter()
        .map(insights::Insight::to_drive_item)
        .filter(|i| !i.id.is_empty() && !i.drive_id.is_empty())
        .map(|i| DriveItem {
            modified_by: String::new(),
            ..i
        })
        .collect()
}

/// Meeting recordings as rows, with the place (`My files` or `<team> › <channel>`) as their
/// location. Recordings in My files keep their folder path so Open location still works.
pub(super) fn recording_items(recs: Vec<onedrive::Recording>) -> Vec<DriveItem> {
    recs.into_iter()
        .map(|r| {
            if r.place == onedrive::around::MY_FILES {
                r.item
            } else {
                DriveItem {
                    parent_path: r.place,
                    ..r.item
                }
            }
        })
        .collect()
}

/// The extra top-level entries of the picker when attaching files: Teams offers Recent and
/// Shared with me next to My files and the team libraries.
pub(super) fn virtual_items() -> Vec<DriveItem> {
    [("Recent", "recent"), ("Shared with me", "shared")]
        .into_iter()
        .map(|(name, id)| DriveItem {
            id: id.into(),
            drive_id: VIRTUAL.into(),
            name: name.into(),
            is_folder: true,
            ..Default::default()
        })
        .collect()
}

pub(super) fn picker_data(p: &Picker, libs: &[Library]) -> PickerData {
    let s = State {
        items: p.items.clone(),
        selected: p.selected.iter().cloned().collect(),
        libraries: libs.to_vec(),
        ..State::default()
    };
    PickerData {
        purpose: p.purpose.clone(),
        title: match p.purpose.as_str() {
            "move" => "Move to",
            "copy" => "Copy to",
            _ => "Choose a file",
        }
        .into(),
        folders_only: p.folders_only,
        // The top level is a fixed list (My files first); inside a folder, folders come first.
        rows: if p.trail.is_empty() {
            s.plain_rows(now_ms(), &local_offset)
        } else {
            s.rows(now_ms(), &local_offset)
        },
        crumbs: std::iter::once("OneDrive".to_owned())
            .chain(p.trail.iter().map(|c| c.name.clone()))
            .collect(),
        loading: p.loading,
        can_choose: if p.folders_only {
            !p.trail.is_empty()
        } else {
            p.selected.is_some()
        },
    }
}

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A file name that is safe to create in a directory of ours.
pub(in super::super) fn sanitize_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim_matches('.').trim().to_owned();
    if s.is_empty() { "download".into() } else { s }
}

/// What the viewer shows for a downloaded file.
pub(super) fn preview_of(item: &DriveItem, bytes: &[u8]) -> PreviewData {
    match Preview::of(item) {
        Preview::Image => PreviewData {
            title: item.name.clone(),
            kind: "image",
            image: super::super::util::decode_image(bytes, 1600, 1100),
            text: String::new(),
            page: 0,
            pages: 0,
        },
        p => PreviewData {
            title: item.name.clone(),
            kind: if p == Preview::Markdown {
                "markdown"
            } else {
                "text"
            },
            image: None,
            text: String::from_utf8_lossy(bytes)
                .chars()
                .take(MAX_TEXT_CHARS)
                .collect(),
            page: 0,
            pages: 0,
        },
    }
}
