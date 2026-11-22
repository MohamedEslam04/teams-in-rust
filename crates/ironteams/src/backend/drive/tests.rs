use super::*;

#[test]
fn the_files_setting_decides_where_office_files_open() {
    use super::super::settings::OpenFilesIn::*;
    for kind in [FileKind::Word, FileKind::Excel, FileKind::PowerPoint] {
        assert_eq!(
            with_files_pref(IronTeams, Preview::Pages, kind),
            Preview::Pages
        );
        assert_eq!(
            with_files_pref(Browser, Preview::Pages, kind),
            Preview::Browser
        );
        assert_eq!(
            with_files_pref(Desktop, Preview::Pages, kind),
            Preview::Player
        );
    }
    // Pictures, text and PDFs are not Office files: the setting leaves them alone.
    for kind in [FileKind::Image, FileKind::Pdf, FileKind::Text] {
        assert_eq!(
            with_files_pref(Desktop, Preview::Image, kind),
            Preview::Image
        );
        assert_eq!(
            with_files_pref(Browser, Preview::Pages, kind),
            Preview::Pages
        );
    }
}

#[test]
fn path_prefixes_walk_down_a_drive_path() {
    assert_eq!(
        path_prefixes("/Docs/Reports/2026"),
        ["Docs", "Docs/Reports", "Docs/Reports/2026"]
    );
    assert!(path_prefixes("").is_empty());
    assert!(path_prefixes("/").is_empty());
}

#[test]
fn people_are_invited_by_email_else_by_object_id() {
    let p = |mri: &str, email: Option<&str>| PersonData {
        id: mri.into(),
        name: "x".into(),
        detail: String::new(),
        picked: true,
        email: email.map(str::to_owned),
    };
    assert_eq!(
        recipient_of(&p("8:orgid:aaa", Some("a@x.com"))),
        Some(Recipient::Email("a@x.com".into()))
    );
    assert_eq!(
        recipient_of(&p("8:orgid:aaa", None)),
        Some(Recipient::ObjectId("aaa".into()))
    );
    assert_eq!(recipient_of(&p("28:bot", None)), None);
}

#[test]
fn permissions_read_as_people_and_links() {
    let now = 1_795_000_000_000;
    let person = Permission {
        id: "p1".into(),
        roles: vec!["write".into()],
        granted_to: vec!["Ada".into(), "Grace".into()],
        ..Default::default()
    };
    let d = perm_data(&person, now);
    assert_eq!(
        (d.who.as_str(), d.role.as_str(), d.link, d.editable),
        ("Ada, Grace", "write", false, true)
    );
    let link = Permission {
        id: "p2".into(),
        roles: vec!["read".into()],
        link_scope: Some("organization".into()),
        link_type: Some("blocksDownload".into()),
        expires_ms: Some(now + 86_400_000),
        ..Default::default()
    };
    let d = perm_data(&link, now);
    assert!(d.link && d.who.starts_with("People in your organization"));
    assert!(d.detail.contains("download blocked") && d.detail.contains("expires"));
    let owner = Permission {
        id: "p3".into(),
        roles: vec!["owner".into()],
        granted_to: vec!["Me".into()],
        ..Default::default()
    };
    assert!(
        !perm_data(&owner, now).editable,
        "the owner cannot be removed"
    );
    let inherited = Permission {
        inherited: true,
        ..person
    };
    assert!(!perm_data(&inherited, now).editable);
}

#[test]
fn versions_mark_the_newest_as_current() {
    let v = |id: &str, ms| Version {
        id: id.into(),
        modified_ms: ms,
        modified_by: "Ada".into(),
        size: 2048,
    };
    let d = version_data(
        &[v("3.0", 3_000_000_000_000), v("2.0", 2_000_000_000_000)],
        3_000_000_000_000,
    );
    assert!(d[0].current && !d[1].current);
    assert_eq!(d[1].size, "2 KB");
}

#[test]
fn link_expiry_is_an_iso_time_days_ahead() {
    assert_eq!(link_expiry(1, 0), "1970-01-02T00:00:00.000Z");
}

#[test]
fn file_names_cannot_escape_the_target_folder() {
    assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(sanitize_file_name("a/b.txt"), "a_b.txt");
    assert_eq!(sanitize_file_name(".."), "download");
    assert_eq!(sanitize_file_name("Q4 plan.xlsx"), "Q4 plan.xlsx");
}

#[test]
fn details_list_the_facts() {
    let i = DriveItem {
        name: "Plan.docx".into(),
        size: 20_480,
        modified_by: "Ada".into(),
        modified_ms: 1_795_000_000_000,
        parent_path: "/Docs".into(),
        web_url: "https://x/Plan.docx".into(),
        ..Default::default()
    };
    let t = details_text(&i, true);
    for want in [
        "Name: Plan.docx",
        "Type: word",
        "Size: 20 KB",
        "Location: /Docs",
        "by Ada",
        "Favorite: yes",
        "https://x/Plan.docx",
    ] {
        assert!(t.contains(want), "{want} in {t}");
    }
}

#[test]
fn a_text_file_is_cut_and_a_picture_is_decoded() {
    let big = "a".repeat(MAX_TEXT_CHARS + 50);
    let item = DriveItem {
        name: "x.txt".into(),
        ..Default::default()
    };
    assert_eq!(
        preview_of(&item, big.as_bytes()).text.chars().count(),
        MAX_TEXT_CHARS
    );
    let md = DriveItem {
        name: "x.md".into(),
        ..Default::default()
    };
    assert_eq!(preview_of(&md, b"# hi").kind, "markdown");
    let img = DriveItem {
        name: "x.png".into(),
        ..Default::default()
    };
    let mut png = Vec::new();
    image::RgbaImage::new(2, 2)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let p = preview_of(&img, &png);
    assert_eq!(p.kind, "image");
    assert!(p.image.is_some());
}
#[test]
fn people_become_rows_that_count_their_files() {
    let people = crate::demo::drive_people_files(1_795_000_000_000);
    let rows = people_items(&people);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "person:0");
    assert_eq!(rows[0].name, "Sara Hassan");
    assert_eq!(rows[0].child_count, 2);
    assert!(rows[0].is_folder);
    assert_eq!(rows[0].modified_ms, people[0].latest_ms);
    // A person with no name is shown by address, then as Unknown.
    let nameless = onedrive::Person {
        address: "a@example.test".into(),
        ..Default::default()
    };
    assert_eq!(person_label(&nameless), "a@example.test");
    assert_eq!(person_label(&onedrive::Person::default()), "Unknown");
}

#[test]
fn shared_by_you_keeps_drive_items_and_drops_the_sharer_name() {
    let me = onedrive::Person {
        name: "Me".into(),
        ..Default::default()
    };
    let with_item = insights::Insight {
        title: "Plan.docx".into(),
        item: Some(ItemRef::new("d1", "i1")),
        shared_by: Some(me.clone()),
        ..Default::default()
    };
    let page = insights::Insight {
        title: "A page".into(),
        shared_by: Some(me),
        ..Default::default()
    };
    let rows = shared_by_you_items(vec![with_item, page]);
    assert_eq!(rows.len(), 1, "only drive items can be acted on");
    assert_eq!(
        (rows[0].name.as_str(), rows[0].id.as_str()),
        ("Plan.docx", "i1")
    );
    assert_eq!(rows[0].modified_by, "");
}

#[test]
fn recordings_show_their_place_and_my_files_keeps_its_path() {
    let item = |name: &str, path: &str| DriveItem {
        id: name.into(),
        name: name.into(),
        parent_path: path.into(),
        ..Default::default()
    };
    let rows = recording_items(vec![
        onedrive::Recording {
            item: item("mine.mp4", "/Recordings"),
            place: "My files".into(),
        },
        onedrive::Recording {
            item: item("team.mp4", "/General/Recordings"),
            place: "Sales \u{203a} General".into(),
        },
    ]);
    assert_eq!(rows[0].parent_path, "/Recordings");
    assert_eq!(rows[1].parent_path, "Sales \u{203a} General");
}
