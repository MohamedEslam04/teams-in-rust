use super::*;

/// Calendars shown in demo mode.
/// A few zones to pick from when the mailbox's list has not loaded (demo mode, offline).
pub(super) fn demo_zones() -> Vec<(String, String)> {
    [
        ("UTC", "(UTC) Coordinated Universal Time"),
        ("Egypt Standard Time", "(UTC+02:00) Cairo"),
        (
            "W. Europe Standard Time",
            "(UTC+01:00) Amsterdam, Berlin, Rome, Stockholm",
        ),
        (
            "Eastern Standard Time",
            "(UTC-05:00) Eastern Time (US & Canada)",
        ),
        (
            "Pacific Standard Time",
            "(UTC-08:00) Pacific Time (US & Canada)",
        ),
        (
            "India Standard Time",
            "(UTC+05:30) Chennai, Kolkata, Mumbai, New Delhi",
        ),
        ("Tokyo Standard Time", "(UTC+09:00) Osaka, Sapporo, Tokyo"),
    ]
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .to_vec()
}

pub(in super::super) fn demo_calendars() -> Vec<CalendarInfo> {
    let c =
        |id: &str, name: &str, color: &str, default: bool, owner: &str, email: &str| CalendarInfo {
            id: id.into(),
            name: name.into(),
            color: color.into(),
            hex_color: None,
            is_default: default,
            can_edit: default,
            can_share: default,
            owner_name: owner.into(),
            owner_email: email.into(),
        };
    vec![
        c(
            "demo-1",
            "Calendar",
            "auto",
            true,
            "Demo",
            "demo@contoso.com",
        ),
        c(
            "demo-2",
            "Birthdays",
            "lightPink",
            false,
            "Demo",
            "demo@contoso.com",
        ),
        c(
            "demo-3",
            "Design team",
            "lightGreen",
            false,
            "Dana Reyes",
            "dana@contoso.com",
        ),
    ]
}

/// The first and last day of the visible range, for file names and agendas.
pub(super) fn visible_days(s: &Inner) -> (Date, Date) {
    let (a, b) = s.range();
    (
        grid::local_date(a, &local_offset),
        grid::local_date(b - 1, &local_offset),
    )
}

/// `2026-12-18T10:00:00` in UTC from Unix milliseconds.
pub(super) fn wall_utc(ms: i64) -> String {
    ::calendar::format_iso_ms(ms)[..19].to_string()
}

/// A local-day-aligned slot for a new event now: the next half hour of the day the view shows
/// (09:00 when that is not today).
pub(super) fn default_slot(s: &Inner) -> (Date, u32) {
    let now = now_ms();
    let today_local = grid::local_date(now, &local_offset);
    if s.date != today_local {
        return (s.date, 9 * 60);
    }
    let mins = (grid::local_minutes(now, &local_offset) as u32 / 30 + 1) * 30;
    if mins >= 24 * 60 {
        (today_local.add_days(1), 0)
    } else {
        (today_local, mins)
    }
}

/// A file name with only letters, digits and `. - _ space`, so it cannot leave its folder.
pub(super) fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Saves bytes under the system temp dir with a file-system-safe name and opens them.
pub(super) fn open_temp_file(name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let safe = safe_name(name);
    let dir = std::env::temp_dir().join("ironteams-attachments");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(if safe.trim_matches('.').is_empty() {
        "attachment".to_string()
    } else {
        safe
    });
    std::fs::write(&path, bytes)?;
    crate::external::open(path)
}

/// A demo-mode event made from the form, so a new event shows up without a network.
pub(super) fn synth_event(f: &FormFields, id: &str, existing: Option<&Event>) -> Option<Event> {
    let (start, end) = if f.all_day {
        let s = forms::local_ms(&f.start_date, "00:00", &local_offset)?;
        let e = forms::local_ms(&f.end_date, "00:00", &local_offset)? + DAY_MS;
        (s, e)
    } else {
        (
            forms::local_ms(&f.start_date, &f.start_time, &local_offset)?,
            forms::local_ms(&f.end_date, &f.end_time, &local_offset)?,
        )
    };
    let people = |s: &str, kind: &str| -> Vec<Value> {
        forms::people_lenient(s)
            .into_iter()
            .map(|p| {
                json!({"emailAddress": {"name": p.name, "address": p.email}, "type": kind,
                    "status": {"response": "none"}})
            })
            .collect()
    };
    let mut attendees = people(&f.required, "required");
    attendees.extend(people(&f.optional, "optional"));
    let t = |ms: i64| json!({"dateTime": ::calendar::format_iso_ms(ms), "timeZone": "UTC"});
    let mut v = json!({
        "id": id, "subject": f.subject,
        "start": t(start), "end": t(end), "isAllDay": f.all_day,
        "showAs": forms::show_as_graph(&f.show_as),
        "sensitivity": if f.private { "private" } else { "normal" },
        "categories": if f.category.is_empty() { vec![] } else { vec![f.category.clone()] },
        "location": {"displayName": f.location},
        "isOnlineMeeting": f.teams_meeting,
        "responseStatus": {"response": "organizer"},
        "organizer": {"emailAddress": {"name": "You", "address": "demo@contoso.com"}},
        "attendees": attendees,
        "type": if f.repeat == "none" || f.repeat == "keep" { "singleInstance" } else { "occurrence" },
    });
    if f.teams_meeting {
        v["onlineMeeting"] = json!({"joinUrl": "https://teams.example/join"});
    }
    let mut e = ::calendar::parse_events(&json!({ "value": [v] }))
        .into_iter()
        .next()?;
    if let Some(old) = existing {
        e.is_recurring = old.is_recurring;
    }
    Some(e)
}

/// A read-only look: events of other calendars cannot be answered or changed from here.
/// The meeting link pasted into the Join with an ID field, with `https://` added when only the
/// host and path were copied. `None` for a meeting ID.
pub(super) fn pasted_link(field: &str) -> Option<String> {
    let t = field.trim();
    if t.contains("://") {
        return Some(t.to_owned());
    }
    [
        "teams.microsoft.com/",
        "teams.cloud.microsoft/",
        "teams.live.com/",
    ]
    .iter()
    .any(|h| t.to_ascii_lowercase().starts_with(h))
    .then(|| format!("https://{t}"))
}

pub(super) fn read_only(mut p: PeekData) -> PeekData {
    p.can_respond = false;
    p.can_edit = false;
    p.can_cancel = false;
    p.can_delete = false;
    p
}

pub(super) fn friendly(err: &dyn std::fmt::Display) -> String {
    let t = err.to_string();
    if t.chars().count() > 160 {
        format!("{}…", t.chars().take(160).collect::<String>())
    } else {
        t
    }
}

/// `https://graph.microsoft.com/v1.0/me/events/{id}`
pub(super) fn event_url(id: &str) -> String {
    reminder_url(id, "").trim_end_matches('/').to_string()
}

/// A colleague who only shares free/busy shows as plain "Busy" blocks.
pub(super) fn free_busy_events(s: &::calendar::Schedule) -> Vec<Event> {
    let t = |ms: i64| json!({"dateTime": ::calendar::format_iso_ms(ms), "timeZone": "UTC"});
    let items: Vec<Value> = s
        .items
        .iter()
        .map(|i| {
            json!({
                "id": format!("fb:{}:{}", s.email, i.start_ms),
                "subject": i.subject.clone().unwrap_or_else(|| "Busy".into()),
                "start": t(i.start_ms), "end": t(i.end_ms),
                "showAs": match i.status {
                    ShowAs::Free => "free", ShowAs::Tentative => "tentative",
                    ShowAs::Oof => "oof", ShowAs::WorkingElsewhere => "workingElsewhere",
                    _ => "busy",
                },
                "sensitivity": if i.is_private { "private" } else { "normal" },
                "location": {"displayName": i.location.clone().unwrap_or_default()},
            })
        })
        .collect();
    ::calendar::parse_events(&json!({ "value": items }))
}

/// Reads the settings dialog's `first|start|end|weekends|weeks|tz`.
pub(in super::super) fn parse_settings(arg: &str) -> Result<session::CalendarSettings, String> {
    let p: Vec<&str> = arg.splitn(6, '|').collect();
    if p.len() < 6 {
        return Err("The settings could not be read.".into());
    }
    let first_day: u8 = p[0]
        .parse()
        .ok()
        .filter(|d| *d < 7)
        .ok_or("Pick a first day of the week.")?;
    let (Some(start), Some(end)) = (forms::parse_hm(p[1]), forms::parse_hm(p[2])) else {
        return Err("Write the work hours like 09:00 and 17:00.".into());
    };
    if end <= start {
        return Err("Work hours end after they start.".into());
    }
    // A zone name, or the old fixed offset in minutes (kept as it was until a zone is picked).
    let (tz, zone) = match p[5].trim() {
        "" => (None, None),
        t => match t.parse::<i32>() {
            Ok(m) if (-12 * 60..=14 * 60).contains(&m) => (Some(m), None),
            Ok(_) => return Err("The second time zone is out of range.".into()),
            Err(_) if ::calendar::utc_offset_min(t, 0).is_some() => (None, Some(t.to_string())),
            Err(_) => return Err("Pick the second time zone from the list.".into()),
        },
    };
    Ok(session::CalendarSettings {
        first_day,
        work_start_min: Some(start),
        work_end_min: Some(end),
        show_weekends: p[3] == "1",
        week_numbers: p[4] == "1",
        second_tz_min: tz,
        second_tz: zone,
    })
}
