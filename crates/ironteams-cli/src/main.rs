//! `ironteams-cli`: exercise the protocol from a terminal.
//!
//! ```text
//! ironteams-cli login                 sign in with a device code
//! ironteams-cli chats [n]             list recent conversations
//! ironteams-cli teams                 list teams and channels
//! ironteams-cli activity [n]          list the newest Activity feed items
//! ironteams-cli read <conv> [n]       print the last n messages
//! ironteams-cli send <conv> <text>    send a message (use 48:notes for your own notes)
//! ironteams-cli react <conv> <msg> <key>
//! ironteams-cli images <conv> [<msg id>|last] [--out <dir>] [--list]  fetch and save the images of one message
//! ironteams-cli calendar [days]       count upcoming events per day and per show-as (read-only)
//! ironteams-cli calendar <check>      live checks of the calendar data layer (calendars, rooms, mailbox, schedule, ...)
//! ironteams-cli calendar permissions --dry-run <role>  print the sharing writes without sending them
//! ironteams-cli audiences             mint a token for every audience (prints no tokens)
//! ironteams-cli people <query>        search people (prints the count first)
//! ironteams-cli search <query> [--kind messages|files|people|events] [--page N]  global search (read-only)
//! ironteams-cli presence [mri...]     presence of the given people, default yourself
//! ironteams-cli card [mri]            profile card of a person, default yourself
//! ironteams-cli horizons <conv>       who has read up to which message in a chat
//! ironteams-cli drive <ls [path]|search q|recent|shared|favorites|recordings|libraries|sync-help|bin|mkdir name|rm id|selftest>
//! ironteams-cli drive <meetings|for-you|shared-by-me|people|media>   more views (read-only)
//! ironteams-cli drive new <kind> [name] [--dry-run]                    New menu; --dry-run prints the request
//! ironteams-cli drive shape <graph path> | shape-search <kql>            redacted answer shapes (read-only)
//! ironteams-cli tasks [todo <n>|plan <n>]  To Do lists and Planner plans (read-only)
//! ironteams-cli flows [runs <n> [top]]  your Workflows and their run history (read-only)
//! ironteams-cli onenote [<notebook> [<section> [<page>]]]  read OneNote notebooks, sections, pages
//! ironteams-cli lists [<site> [<list>]]  SharePoint lists of your teams' sites (read-only)
//! ironteams-cli store <bar|more|home|search|app|mine|org|settings|install|uninstall|update|pin|unpin|move|probe|bots|extensions|ext-query>  app bar and Store (writes are dry runs)
//! ironteams-cli settings <notifications|quiet|priority|privacy|blocked|shape>  notification and privacy settings (read-only)
//! ironteams-cli settings set <what> ... --dry-run  print the settings write without sending it
//! ironteams-cli team <ls|discover|join|code|new|edit|settings|archive|channel|tag|members|community|…>
//!                                 teams and channels (writes go to the sandbox team, see team.rs)
//! ironteams-cli call <probe|meetings>   native calling checks (see call.rs)
//! ironteams-cli tail                  print live events until Ctrl+C
//! ironteams-cli web <url> [--shim] [--theme T] [--browser P] [--profile D]  open a page in the web host window
//! ironteams-cli diag [--log F] [--out P]  write a local, redacted problem-report bundle (nothing is uploaded)
//! ironteams-cli accounts [tenants|use <label>|add <label>|remove <label>|org <tenant-id|home>]
//!                                 several accounts and organizations (list and tenants are read-only)
//! ironteams-cli preview-file <path>|--find <ext>|--local <file> [--page N] [--width W] [--out F.png]
//!                                 render a PDF or Office page to PNG (read-only; prints sizes, not names)
//! ironteams-cli devices [audio|cameras|test-sound|mic-level]  list sound devices and cameras; test-sound plays a tone, mic-level shows the mic level
//! ironteams-cli appearance [show|watch]  saved appearance, general and accessibility prefs, the desktop portal values and the resolved result (read-only)
//! ironteams-cli tray-demo [--step S|autostart <enable|disable|status>]  show the tray icon, unread badge and launcher badge for a few seconds; autostart writes the login entry
//! ironteams-cli logout                remove the stored refresh token
//! ```

mod accounts;
mod appcard;
mod appearance;
mod apps;
mod call;
mod chatx;
mod delivery;
mod devices;
mod drive;
mod find;
mod images;
mod preview_file;
mod search;
mod settings;
mod store;
mod team;
mod tester;
mod tray;
mod web;

use anyhow::{Context, Result, bail};
use session::Paths;
use teams_proto::trouter::TrouterEvent;
use teams_proto::{Client, model::Event};

fn fmt_time(ms: i64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60)
}

fn preview(html: &str) -> String {
    let t = render::plain_text(&render::parse_teams_html(html));
    let one: String = t.replace('\n', " ");
    if one.chars().count() > 80 {
        format!("{}…", one.chars().take(80).collect::<String>())
    } else {
        one
    }
}

async fn self_mri(client: &Client, cfg: &session::Config) -> Result<String> {
    match &cfg.self_mri {
        Some(m) => Ok(m.clone()),
        None => Ok(client.self_mri().await?),
    }
}

async fn connect(paths: &Paths) -> Result<(Client, session::Config)> {
    let cfg = paths.load_config()?;
    let sess =
        session::stored_session(&cfg)?.context("not signed in: run `ironteams-cli login`")?;
    let client = Client::resume(sess).await?;
    session::persist(&cfg, &client).await?;
    Ok((client, cfg))
}

/// `ironteams-cli calendar <check>`: the calendar data layer against the live service. Prints
/// counts and key names only, never titles, people or addresses. `write-check` and
/// `calendar-check` are the only writes: a private appointment (and a calendar) made and
/// removed again.
async fn calendar_check(client: &Client, args: &[String]) -> Result<()> {
    use calendar as cal;
    let now = teams_proto::time::now_ms();
    let day = 86_400_000;
    match args.first().map(String::as_str) {
        Some("calendars") => {
            let list = cal::calendars(client).await?;
            let groups = cal::calendar_groups(client).await?;
            println!(
                "{} calendars ({} default, {} editable, {} with a hex color), {} groups",
                list.len(),
                list.iter().filter(|c| c.is_default).count(),
                list.iter().filter(|c| c.can_edit).count(),
                list.iter().filter(|c| c.hex_color.is_some()).count(),
                groups.len()
            );
            if let Some(g) = groups.first() {
                let n = cal::calendars_in_group(client, &g.id).await?.len();
                println!("first group holds {n} calendars");
            }
            if let Some(c) = list.iter().find(|c| c.is_default) {
                let perms = cal::calendar_permissions(client, &c.id).await?;
                println!("default calendar has {} permission entries", perms.len());
            }
        }
        Some("rooms") => {
            let lists = cal::room_lists(client).await?;
            println!("{} room lists", lists.len());
            let all = cal::rooms(client, "").await?;
            println!("{} rooms in the tenant", all.len());
            if !all.is_empty() {
                let start = now - now.rem_euclid(day) + day;
                let s = cal::room_schedules(client, &all, start, start + day, 30).await?;
                println!(
                    "room availability tomorrow: {} schedules, {} readable, {} free 10:00-11:00 UTC",
                    s.len(),
                    s.iter().filter(|s| s.error.is_none()).count(),
                    s.iter()
                        .filter(|s| cal::free_between(
                            s,
                            start + 10 * 3_600_000,
                            start + 11 * 3_600_000
                        ) == Some(true))
                        .count()
                );
            }
            if let Some(l) = lists.first() {
                let rooms = cal::rooms(client, &l.email).await?;
                println!(
                    "first list: {} rooms, {} with capacity, {} with a building",
                    rooms.len(),
                    rooms.iter().filter(|r| r.capacity.is_some()).count(),
                    rooms.iter().filter(|r| r.building.is_some()).count()
                );
            }
        }
        Some("mailbox") => {
            let (status, text) = client
                .request_as(
                    &teams_proto::broker::Audience::Graph,
                    reqwest::Method::GET,
                    &format!(
                        "https://graph.microsoft.com/v1.0{}",
                        cal::GRAPH_MAILBOX_SETTINGS_PATH
                    ),
                    None,
                    &[],
                )
                .await?;
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            println!(
                "Graph mailboxSettings: HTTP {status}, work hours zone custom: {}, date format set: {}",
                v["workingHours"]["timeZone"]["bias"].is_number(),
                v["dateFormat"].as_str().is_some_and(|f| !f.is_empty())
            );
            let s = cal::mailbox_settings(client).await?;
            println!(
                "time zone set: {}, work days: {}, work hours {:02}:{:02}-{:02}:{:02}, date format set: {}, time format set: {}",
                !s.time_zone.is_empty(),
                s.working_hours.days.len(),
                s.working_hours.start_min / 60,
                s.working_hours.start_min % 60,
                s.working_hours.end_min / 60,
                s.working_hours.end_min % 60,
                !s.date_format.is_empty(),
                !s.time_format.is_empty()
            );
        }
        Some("categories") => {
            let c = cal::categories(client).await?;
            println!(
                "{} categories, {} with a color",
                c.len(),
                c.iter().filter(|c| c.color_preset.is_some()).count()
            );
        }
        Some("schedule") => {
            let me = client
                .json_as(
                    "me",
                    &teams_proto::broker::Audience::Graph,
                    reqwest::Method::GET,
                    "https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName",
                    None,
                )
                .await?;
            let email = me["mail"]
                .as_str()
                .or(me["userPrincipalName"].as_str())
                .context("no mail address on /me")?
                .to_string();
            let start = now - now.rem_euclid(day);
            let schedules =
                cal::get_schedule(client, std::slice::from_ref(&email), start, start + day, 30)
                    .await?;
            for s in &schedules {
                println!(
                    "my schedule today: {} slots, {} busy, {} items, error: {}",
                    s.availability.len(),
                    s.availability
                        .iter()
                        .filter(|a| **a == cal::Availability::Busy)
                        .count(),
                    s.items.len(),
                    s.error.is_some()
                );
            }
            match cal::person_view(client, &email, start, start + day, 30).await? {
                cal::PersonView::Events(e) => {
                    println!("my calendar via /users/{{upn}}: {} events", e.len())
                }
                cal::PersonView::FreeBusyOnly(_) => {
                    println!("my calendar via /users/{{upn}}: free/busy fallback")
                }
            }
            let found = cal::find_meeting_times(
                client,
                &cal::FindTimes {
                    required: vec![cal::Person::new("", &email)],
                    optional: vec![],
                    duration_minutes: 30,
                    window_start_ms: start + 2 * day,
                    window_end_ms: start + 3 * day,
                    max_candidates: 5,
                    min_attendee_percentage: 100,
                    work_hours_only: true,
                },
            )
            .await?;
            println!(
                "find meeting times (only me, in 2 days): {} suggestions, empty reason set: {}",
                found.suggestions.len(),
                found.empty_reason.is_some()
            );
            if let Some(h) = schedules.first().and_then(|s| s.working_hours.as_ref()) {
                let slots = cal::work_slots(h, start, 30, 48);
                println!(
                    "my work hours from getSchedule: custom zone {}, zone maps to IANA {}, {} of 48 half-hours today are work time",
                    h.custom_zone.is_some(),
                    cal::iana_name(&h.time_zone).is_some(),
                    slots.iter().filter(|w| **w).count()
                );
            }
        }
        Some("timezones") => {
            let windows = cal::supported_time_zones(client, false).await?;
            let unmapped: Vec<&str> = windows
                .iter()
                .filter(|z| cal::iana_name(&z.alias).is_none())
                .map(|z| z.alias.as_str())
                .collect();
            println!(
                "{} Windows zones offered, {} map to IANA, unmapped: {unmapped:?}",
                windows.len(),
                windows.len() - unmapped.len()
            );
            let iana = cal::supported_time_zones(client, true).await?;
            println!(
                "{} IANA zones offered, {} known to chrono-tz",
                iana.len(),
                iana.iter()
                    .filter(|z| cal::iana_name(&z.alias).is_some())
                    .count()
            );
            let s = cal::mailbox_settings(client).await?;
            println!(
                "mailbox zone maps to IANA: {}, offset now known: {}",
                cal::iana_name(&s.time_zone).is_some(),
                cal::utc_offset_min(&s.time_zone, now).is_some()
            );
        }
        Some("proposals") => {
            // Read-only: the organizer's view of proposed times.
            let msgs = cal::proposal_messages(client, 25).await?;
            println!(
                "{} proposals in the newest tentative/decline answers, {} linked to an event",
                msgs.len(),
                msgs.iter().filter(|p| p.event_id.is_some()).count()
            );
            let events = cal::Calendar::view(client, now - 7 * day, now + 30 * day).await?;
            let mine: Vec<_> = events
                .iter()
                .filter(|e| e.response == cal::Response::Organizer && !e.attendees.is_empty())
                .take(5)
                .collect();
            let mut found = 0;
            for e in &mine {
                found += cal::event_proposals(client, &e.id).await?.len();
            }
            println!(
                "{} meetings I organize read, {found} proposals on their attendees",
                mine.len()
            );
        }
        Some("permissions") => {
            // Reads the default calendar's sharing; `--dry-run <role> [address]` prints the
            // add/update/remove requests without sending them.
            let list = cal::calendars(client).await?;
            let c = list
                .iter()
                .find(|c| c.is_default)
                .context("no default calendar")?;
            let perms = cal::calendar_permissions(client, &c.id).await?;
            println!(
                "default calendar: {} permission entries, {} removable, {} with allowed roles",
                perms.len(),
                perms.iter().filter(|p| p.is_removable).count(),
                perms.iter().filter(|p| !p.allowed_roles.is_empty()).count()
            );
            if args.get(1).map(String::as_str) == Some("--dry-run") {
                let role = cal::PermissionRole::from_graph(
                    args.get(2).map_or("freeBusyRead", String::as_str),
                );
                let who = cal::Person::new(
                    "",
                    args.get(3)
                        .map_or("someone@contoso.example", String::as_str),
                );
                let id = "{calendar-id}";
                for r in [
                    cal::add_permission_request(id, &who, role, true),
                    cal::update_permission_request(id, "{permission-id}", role),
                    cal::remove_permission_request(id, "{permission-id}"),
                ] {
                    println!(
                        "DRY RUN (not sent): {} {} {}",
                        r.method,
                        r.path,
                        r.body.map(|b| b.to_string()).unwrap_or_default()
                    );
                }
            }
        }
        Some(probe @ ("probe-outlook" | "probe-graph")) => {
            // GET only, prints the status and the top-level key names, never values.
            let (host, aud) = if probe == "probe-graph" {
                (
                    "https://graph.microsoft.com",
                    teams_proto::broker::Audience::Graph,
                )
            } else {
                (
                    "https://outlook.office.com",
                    teams_proto::broker::Audience::Outlook,
                )
            };
            let path = args
                .get(1)
                .with_context(|| format!("{probe} <path under {host}/>"))?;
            let url = format!("{host}/{}", path.trim_start_matches('/'));
            let (status, text) = client
                .request_as(&aud, reqwest::Method::GET, &url, None, &[])
                .await?;
            let v = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            let names = |v: &serde_json::Value| -> Vec<String> {
                v.as_object()
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default()
            };
            let rows = v["value"].as_array();
            println!(
                "HTTP {status}, {} bytes, keys {:?}, error code {:?}, {} rows, first row keys {:?}",
                text.len(),
                names(&v),
                v["error"]["code"].as_str(),
                rows.map_or(0, Vec::len),
                rows.and_then(|r| r.first()).map(names).unwrap_or_default()
            );
        }
        Some("worklocation") => {
            // `worklocation [office|remote|clear]`: a pin set here lapses after one hour.
            let pin = match args.get(1).map(String::as_str) {
                None => None,
                Some("office") => Some(cal::WorkLocation::Office),
                Some("remote") => Some(cal::WorkLocation::Remote),
                Some("clear") => Some(cal::WorkLocation::Unset),
                Some(_) => bail!("usage: calendar worklocation [office|remote|clear]"),
            };
            if let Some(pin) = pin {
                cal::set_work_location(client, pin, now + 3_600_000).await?;
                println!("set work location: {pin:?}");
            }
            let w = cal::work_location(client).await?;
            println!("work location: {w:?}");
        }
        Some("detail") => {
            let events = cal::Calendar::view(client, now, now + 7 * day).await?;
            let Some(e) = events.iter().find(|e| !e.attendees.is_empty()) else {
                println!("no upcoming event with attendees to read");
                return Ok(());
            };
            let d = cal::event_detail(client, &e.id).await?;
            println!(
                "detail: body {} bytes, {} attachments, counts {:?}, conference id set: {}, thread id from join url: {}",
                d.body.len(),
                d.attachments.len(),
                d.counts,
                d.dial_in.conference_id.is_some(),
                e.join_url
                    .as_deref()
                    .and_then(cal::meeting_chat_thread_id)
                    .is_some()
            );
            if let Some(master) = &e.series_master_id {
                let inst = cal::instances(client, master, now, now + 30 * day).await?;
                println!("series has {} occurrences in 30 days", inst.len());
            }
        }
        Some("ics") => {
            let events = cal::Calendar::view(client, now, now + 7 * day).await?;
            let ics = cal::to_ics(&events, now);
            println!(
                "{} events exported to {} bytes of .ics ({} VEVENT)",
                events.len(),
                ics.len(),
                ics.matches("BEGIN:VEVENT").count()
            );
        }
        Some("write-check") => {
            let start = now + 30 * day;
            let start = start - start.rem_euclid(day) + 12 * 3_600_000;
            let wall = |ms: i64| cal::format_iso_ms(ms)[..19].to_string();
            let new = cal::NewEvent {
                subject: "ironteams test".into(),
                start: wall(start),
                end: wall(start + 1_800_000),
                is_teams_meeting: false,
                sensitivity: cal::Sensitivity::Private,
                ..cal::NewEvent::default()
            };
            let created = cal::create_event(client, &new).await?;
            println!(
                "created: subject ok {}, private {}, attendees {}, online {}",
                created.subject == "ironteams test",
                created.sensitivity_private,
                created.attendees.len(),
                created.is_online_meeting
            );
            let mut changed = new.clone();
            changed.subject = "ironteams test (updated)".into();
            let outcome = cal::update_event(client, &created.id, &changed).await;
            match &outcome {
                Ok(u) => println!(
                    "updated: subject changed {}",
                    u.subject == "ironteams test (updated)"
                ),
                Err(e) => println!("update failed: {e}"),
            }
            cal::delete_event(client, &created.id).await?;
            println!("deleted");
            let left = cal::Calendar::view(client, start - day, start + day)
                .await?
                .iter()
                .filter(|e| e.subject.starts_with("ironteams test"))
                .count();
            println!("cleanup proof: {left} events named 'ironteams test*' left in the window");
            outcome?;
        }
        Some("tester") => tester::run(client, &args[1..]).await?,
        Some("calendar-check") => {
            let made = cal::create_calendar(client, "ironteams-test", "lightBlue").await?;
            println!("created calendar, color {}", made.color);
            cal::delete_calendar(client, &made.id).await?;
            let left = cal::calendars(client)
                .await?
                .iter()
                .filter(|c| c.name == "ironteams-test")
                .count();
            println!("cleanup proof: {left} calendars named 'ironteams-test' left");
        }
        other => bail!(
            "unknown calendar check {other:?}; try calendars, rooms, mailbox, schedule, timezones, proposals, permissions, probe-graph, probe-outlook, worklocation, detail, ics, write-check, calendar-check, tester"
        ),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // reqwest has no TLS provider of its own; install the app's (ring) before any client.
    teams_proto::client::install_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            compat::var("IRONTEAMS_LOG").unwrap_or_default(),
        ))
        .with_writer(std::io::stderr)
        .init();
    compat::migrate_legacy_dirs();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::new()?;
    match args.first().map(String::as_str) {
        Some("login") => {
            let mut cfg = paths.load_config()?;
            let client = Client::sign_in_device_code(&cfg.tenant, &cfg.epid, |dc| {
                println!(
                    "Open {} and enter the code: {}",
                    dc.verification_url, dc.user_code
                );
            })
            .await?;
            session::persist(&cfg, &client).await?;
            let me = client.self_mri().await?;
            println!(
                "Signed in as {me} (chat service: {})",
                client.chat_service().await
            );
            cfg.self_mri = Some(me);
            paths.save_config(&cfg)?;
        }
        Some("diag") => {
            let out = diag::report_from_args(&paths, env!("CARGO_PKG_VERSION"), &args[1..])?;
            println!(
                "Wrote {}\nIt was made locally and nothing was uploaded. Tokens, emails, ids and \
                 tenant details are redacted; skim it before you share it.",
                out.display()
            );
        }
        Some("accounts") => accounts::run(&paths, &args[1..]).await?,
        Some("web") => web::run(&paths, &args[1..]).await?,
        Some("devices") => devices::run(&args[1..])?,
        Some("appearance") => appearance::run(&args[1..]).await?,
        Some("tray-demo") => tray::run(&args[1..])?,
        Some("logout") => {
            let cfg = paths.load_config()?;
            session::delete_refresh_token(&cfg.account)?;
            println!("Signed out.");
        }
        Some("chats") => {
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
            let (client, _) = connect(&paths).await?;
            let convs = client.conversations(0, 100).await?;
            for c in convs.iter().take(n) {
                let title = c.topic.clone().unwrap_or_else(|| c.id.clone());
                let last = c
                    .last_message
                    .as_ref()
                    .map(|m| format!("{}: {}", m.sender_name, preview(&m.content)))
                    .unwrap_or_default();
                let unread = if c.last_activity_ms > c.read_horizon_ms {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{unread} {:<9?} {:<40} {}\n    {}",
                    c.kind,
                    title,
                    fmt_time(c.last_activity_ms),
                    last
                );
                println!("    id: {}", c.id);
            }
        }
        Some("teams") => {
            let (client, _) = connect(&paths).await?;
            let snap = client.snapshot().await?;
            for t in &snap.teams {
                println!("{}", t.name);
                for ch in &t.channels {
                    println!("  # {:<30} {}", ch.name, ch.id);
                }
            }
            println!("({} chats in aggregator)", snap.chats.len());
        }
        Some("activity") => {
            let n: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
            let (client, _) = connect(&paths).await?;
            let items = client.activity(n).await?;
            for i in &items {
                println!(
                    "{} {:<28} {} {}\n    in: {}  id: {}",
                    if i.is_read { " " } else { "*" },
                    format!("{:?}", i.kind),
                    fmt_time(i.timestamp_ms),
                    i.actor_name,
                    i.thread_topic,
                    i.id
                );
                if !i.preview.is_empty() {
                    println!("    {}", preview(&i.preview));
                }
            }
            println!("({} items)", items.len());
        }
        Some("read") => {
            let conv = args.get(1).context("usage: read <conversation id> [n]")?;
            let n: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
            let (client, _) = connect(&paths).await?;
            let page = client.messages(conv, None, n).await?;
            for m in page.messages.iter().filter(|m| m.is_chat_content()) {
                let flags = format!(
                    "{}{}",
                    if m.edited_ms.is_some() {
                        " (edited)"
                    } else {
                        ""
                    },
                    if m.deleted { " (deleted)" } else { "" }
                );
                let reacts: Vec<String> = m
                    .reactions
                    .iter()
                    .map(|r| format!("{}×{}", r.key, r.users.len()))
                    .collect();
                println!(
                    "[{}] {}{}: {}  {}",
                    fmt_time(m.compose_ms),
                    m.sender_name,
                    flags,
                    preview(&m.content),
                    reacts.join(" ")
                );
                for f in &m.files {
                    println!("      📎 {} {}", f.name, f.url);
                }
            }
            println!(
                "(id of last: {})",
                page.messages.last().map(|m| m.id.as_str()).unwrap_or("-")
            );
        }
        Some("raw") => {
            // Prints raw message JSON (structure discovery). Contains message content, never tokens.
            let conv = args.get(1).context("usage: raw <conversation id> [n]")?;
            let n: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            let (client, _) = connect(&paths).await?;
            let url = format!(
                "{}/v1/users/ME/conversations/{}/messages?view=msnp24Equivalent|supportsMessageProperties&pageSize={n}&startTime=1",
                client.chat_service().await,
                teams_proto::chatsvc::encode_conversation(conv)
            );
            let v = client.json("raw", reqwest::Method::GET, &url, None).await?;
            println!("{}", serde_json::to_string_pretty(&v["messages"])?);
        }
        Some("csa-raw") => {
            // Prints the aggregator entry of one chat (structure discovery).
            let id = args.get(1).context("usage: csa-raw <conversation id>")?;
            let (client, _) = connect(&paths).await?;
            let url = "https://teams.microsoft.com/api/csa/api/v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true";
            let v = client.json("csa", reqwest::Method::GET, url, None).await?;
            let chat = v["chats"]
                .as_array()
                .and_then(|a| a.iter().find(|c| c["id"] == id.as_str()))
                .cloned();
            let mut c = chat.unwrap_or_default();
            if let Some(o) = c.as_object_mut() {
                o.remove("lastMessage");
            }
            println!("{}", serde_json::to_string_pretty(&c)?);
        }
        Some("img") => {
            let url = args.get(1).context("usage: img <ams url>")?;
            let (client, _) = connect(&paths).await?;
            match client.download_image(url).await {
                Ok(b) => println!("ok {} bytes", b.len()),
                Err(e) => println!("error: {e}"),
            }
        }
        Some("ams-meta") => {
            // Prints the metadata of AMS objects (structure discovery): type, views, filename.
            // Usage: ams-meta <host, e.g. us-api.asm.skype.com> <object id>...
            let host = args
                .get(1)
                .context("usage: ams-meta <host> <object id>...")?;
            let (client, _) = connect(&paths).await?;
            for id in &args[2..] {
                let url = format!("https://{host}/v1/objects/{id}");
                match client.download_media(&url, 1 << 20).await {
                    Ok(b) => println!("{id}: {}", String::from_utf8_lossy(&b)),
                    Err(e) => println!("{id}: error: {e}"),
                }
            }
        }
        Some("send") => {
            let (Some(conv), Some(text)) = (args.get(1), args.get(2)) else {
                bail!("usage: send <conversation id> <text>")
            };
            let (client, cfg) = connect(&paths).await?;
            let name = cfg.display_name.clone().unwrap_or_default();
            let cmid = client
                .send_message(conv, &render::text_to_teams_html(text), &name, None)
                .await?;
            println!("sent (clientmessageid {cmid})");
        }
        Some("gifs") => {
            // `gifs [query]`: GIF search, read only (an empty query gives the trending ones).
            let (client, _) = connect(&paths).await?;
            let q = args[1..].join(" ");
            let found = client.search_gifs(&q, "en-us").await?;
            println!("{} GIF(s)", found.len());
            for g in found.iter().take(5) {
                println!("  {} {}x{}", g.title, g.width, g.height);
            }
        }
        Some("schedule") => {
            // `schedule <conversation id> <minutes> <text>`: a scheduled send (live checks use
            // 48:notes). `schedule list` and `schedule cancel <id>`.
            let (client, cfg) = connect(&paths).await?;
            match args.get(1).map(String::as_str) {
                Some("list") => {
                    let list = client.scheduled_messages().await?;
                    println!("{} scheduled message(s)", list.len());
                    for m in &list {
                        println!(
                            "  {} in {} at {} ({} bytes)",
                            m.id,
                            if m.conversation_id == "48:notes" {
                                "48:notes"
                            } else {
                                "a chat"
                            },
                            calendar::format_iso_ms(m.send_at_ms),
                            m.html.len()
                        );
                    }
                }
                Some("cancel") => {
                    let id = args.get(2).context("usage: schedule cancel <id>")?;
                    client.cancel_scheduled(id).await?;
                    println!("cancelled");
                }
                Some(conv) => {
                    let (Some(mins), Some(text)) = (args.get(2), args.get(3)) else {
                        bail!("usage: schedule <conversation id> <minutes> <text>")
                    };
                    let mins: i64 = mins.parse().context("minutes")?;
                    let at = teams_proto::time::now_ms() + mins * 60_000;
                    let name = cfg.display_name.clone().unwrap_or_default();
                    let cmid = client
                        .schedule_message(conv, &render::text_to_teams_html(text), &name, at)
                        .await?;
                    println!(
                        "scheduled for {} (clientmessageid {cmid})",
                        calendar::format_iso_ms(at)
                    );
                }
                None => {
                    bail!("usage: schedule <conversation id> <minutes> <text> | list | cancel <id>")
                }
            }
        }
        Some("reply") => {
            let (Some(conv), Some(msg), Some(text)) = (args.get(1), args.get(2), args.get(3))
            else {
                bail!("usage: reply <conv> <msg id> <text>")
            };
            let (client, cfg) = connect(&paths).await?;
            let page = client.messages(conv, None, 50).await?;
            let orig = page
                .messages
                .iter()
                .find(|m| &m.id == msg)
                .context("message not in the last 50")?;
            let preview = render::plain_text(&render::parse_teams_html(&orig.content));
            let html = render::reply_html(
                &orig.id,
                &orig.sender_mri,
                &orig.sender_name,
                orig.compose_ms,
                &preview,
                text,
            );
            let qtd = teams_proto::chatsvc::quoted_messages_property(
                &orig.id,
                &orig.sender_mri,
                orig.compose_ms,
            );
            client
                .send_quote_reply(conv, &html, &cfg.display_name.unwrap_or_default(), &qtd)
                .await?;
            println!("replied");
        }
        Some("edit") => {
            let (Some(conv), Some(msg), Some(text)) = (args.get(1), args.get(2), args.get(3))
            else {
                bail!("usage: edit <conv> <msg id> <text>")
            };
            let (client, cfg) = connect(&paths).await?;
            client
                .edit_message(
                    conv,
                    msg,
                    &render::text_to_teams_html(text),
                    &cfg.display_name.unwrap_or_default(),
                )
                .await?;
            println!("edited");
        }
        Some("forward") => {
            let (Some(src), Some(msg), Some(target)) = (args.get(1), args.get(2), args.get(3))
            else {
                bail!("usage: forward <source conv> <message id> <target conv> [note]")
            };
            let (client, cfg) = connect(&paths).await?;
            let note = args.get(4).map(|t| render::text_to_teams_html(t));
            let r = client
                .forward(
                    src,
                    std::slice::from_ref(msg),
                    std::slice::from_ref(target),
                    note.as_deref(),
                    &cfg.display_name.unwrap_or_default(),
                )
                .await?;
            for f in r {
                println!(
                    "forwarded to {}: {} {:?}",
                    f.thread_id, f.status, f.message_id
                );
            }
        }
        Some(
            cmd @ ("save" | "unsave" | "pin" | "unpin" | "unread" | "clear-unread" | "mute"
            | "unmute" | "hide" | "unhide"),
        ) => {
            let conv = args.get(1).context("usage: <action> <conv> [message id]")?;
            let msg = args.get(2).cloned().unwrap_or_default();
            let (client, _) = connect(&paths).await?;
            match cmd {
                "save" | "unsave" => client.set_saved(conv, &msg, &msg, cmd == "save").await?,
                "pin" | "unpin" => client.set_pinned(conv, &msg, cmd == "pin").await?,
                "unread" => client.mark_unread(conv, &msg, "").await?,
                "clear-unread" => client.clear_unread_mark(conv).await?,
                "mute" | "unmute" => client.set_muted(conv, cmd == "mute").await?,
                _ => client.set_hidden(conv, cmd == "hide").await?,
            }
            println!("{cmd}: ok");
        }
        Some("saved") => {
            let (client, _) = connect(&paths).await?;
            for s in client.saved_messages().await? {
                println!(
                    "{} {} {}: {}",
                    s.conversation_id, s.message_id, s.sender_name, s.preview
                );
            }
        }
        Some("pins") => {
            let conv = args.get(1).context("usage: pins <chat id>")?;
            let (client, _) = connect(&paths).await?;
            for p in client.pinned(conv).await? {
                println!(
                    "{} folder={:?} at {}",
                    p.message_id, p.folder_id, p.pinned_at
                );
            }
        }
        Some("delete") => {
            let (Some(conv), Some(msg)) = (args.get(1), args.get(2)) else {
                bail!("usage: delete <conv> <msg id>")
            };
            let (client, _) = connect(&paths).await?;
            client.delete_message(conv, msg).await?;
            println!("deleted");
        }
        Some("unreact") => {
            let (Some(conv), Some(msg), Some(key)) = (args.get(1), args.get(2), args.get(3)) else {
                bail!("usage: unreact <conv> <msg id> <key>")
            };
            let (client, _) = connect(&paths).await?;
            client.remove_reaction(conv, msg, key).await?;
            println!("unreacted");
        }
        Some("send-image") => {
            let (Some(conv), Some(file)) = (args.get(1), args.get(2)) else {
                bail!("usage: send-image <conv> <file>")
            };
            let (client, cfg) = connect(&paths).await?;
            let data = std::fs::read(file)?;
            let name = std::path::Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image.png")
                .to_string();
            let id = client.upload_image(conv, &name, data).await?;
            client
                .send_message(
                    conv,
                    &teams_proto::ams::image_html(&id, 320, 200),
                    &cfg.display_name.unwrap_or_default(),
                    None,
                )
                .await?;
            println!("image sent ({id})");
        }
        Some("post") => {
            let (Some(channel), Some(subject), Some(text)) =
                (args.get(1), args.get(2), args.get(3))
            else {
                bail!("usage: post <channel id> <subject> <text>")
            };
            let (client, cfg) = connect(&paths).await?;
            let cmid = client
                .send_message(
                    channel,
                    &render::text_to_teams_html(text),
                    &cfg.display_name.unwrap_or_default(),
                    Some(subject),
                )
                .await?;
            println!("posted (clientmessageid {cmid})");
        }
        Some("send-file") => {
            let (Some(conv), Some(file)) = (args.get(1), args.get(2)) else {
                bail!("usage: send-file <conv> <file>")
            };
            let (client, cfg) = connect(&paths).await?;
            let data = std::fs::read(file)?;
            let name = std::path::Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let f = client
                .send_file(conv, &name, data, &cfg.display_name.unwrap_or_default())
                .await?;
            println!("file sent ({}, drive item {})", f.name, f.drive_item_id);
        }
        Some("react") => {
            let (Some(conv), Some(msg), Some(key)) = (args.get(1), args.get(2), args.get(3)) else {
                bail!("usage: react <conversation id> <message id> <key>")
            };
            let (client, _) = connect(&paths).await?;
            client.add_reaction(conv, msg, key).await?;
            println!("reacted");
        }
        Some("people") => {
            let q = args.get(1).context("usage: people <query>")?;
            let (client, cfg) = connect(&paths).await?;
            let me = self_mri(&client, &cfg).await?;
            let found = client.search_people(&me, q).await?;
            println!("{} result(s)", found.len());
            for p in &found {
                println!(
                    "{}  {}  {}  {}",
                    p.mri,
                    p.display_name,
                    p.email.as_deref().unwrap_or("-"),
                    p.job_title.as_deref().unwrap_or("-")
                );
            }
        }
        Some("presence") => {
            let (client, cfg) = connect(&paths).await?;
            // `presence set <status>` forces my status for ten minutes; `presence reset` clears it.
            let mut mris: Vec<String> = args[1..].to_vec();
            match mris.first().map(String::as_str) {
                Some("set") => {
                    let want = mris.get(1).map(String::as_str).unwrap_or_default();
                    let a = teams_proto::presence::Availability::from_wire(want).context(
                        "usage: presence set Available|Busy|DoNotDisturb|BeRightBack|Away|Offline",
                    )?;
                    let until = calendar::format_iso_ms(teams_proto::time::now_ms() + 600_000);
                    client.set_presence(a, Some(&until)).await?;
                    println!("set {} until {until}", a.as_str());
                    mris.clear();
                }
                Some("reset") => {
                    client.reset_presence().await?;
                    println!("reset");
                    mris.clear();
                }
                Some("note") => {
                    // `presence note [text…]`: my status message for one hour; no text clears it.
                    let text = mris[1..].join(" ");
                    let until = calendar::format_iso_ms(teams_proto::time::now_ms() + 3_600_000);
                    let expiry = (!text.is_empty()).then_some(until.as_str());
                    client.set_status_note(&text, expiry, false).await?;
                    println!(
                        "status message {}",
                        if text.is_empty() { "cleared" } else { "set" }
                    );
                    mris.clear();
                }
                Some("idle") => {
                    client.report_activity(false).await?;
                    println!("reported idle");
                    mris.clear();
                }
                Some("remove-endpoint") => {
                    client.remove_endpoint().await?;
                    println!("removed this device's endpoint");
                    mris.clear();
                }
                Some("endpoint") => {
                    // `presence endpoint <availability> <activity>`: this device's own presence.
                    let a = mris
                        .get(1)
                        .and_then(|w| teams_proto::presence::Availability::from_wire(w))
                        .context("usage: presence endpoint <availability> <activity>")?;
                    let activity = mris.get(2).map(String::as_str).unwrap_or(a.as_str());
                    client.publish_endpoint(a, activity).await?;
                    println!("endpoint {} / {activity}", a.as_str());
                    mris.clear();
                }
                _ => {}
            }
            if mris.is_empty() {
                mris.push(self_mri(&client, &cfg).await?);
            }
            let found = client.get_presence(&mris).await?;
            println!("{} presence record(s)", found.len());
            for p in &found {
                println!(
                    "{} {} / {}{}{}{}",
                    p.mri,
                    p.availability,
                    p.activity,
                    if p.ooo { " ooo" } else { "" },
                    p.note
                        .as_ref()
                        .map(|n| format!(" note={:?}", n.text))
                        .unwrap_or_default(),
                    p.work_location
                        .as_ref()
                        .map(|w| format!(" work location={w}"))
                        .unwrap_or_default()
                );
            }
        }
        Some("card") => {
            let (client, cfg) = connect(&paths).await?;
            let mri = match args.get(1) {
                Some(m) => m.clone(),
                None => self_mri(&client, &cfg).await?,
            };
            let c = client.profile_card(&mri).await?;
            println!("{c:#?}");
        }
        Some("horizons") => {
            let conv = args.get(1).context("usage: horizons <conv>")?;
            let (client, _) = connect(&paths).await?;
            let hs = client.consumption_horizons(conv).await?;
            println!("{} horizon(s)", hs.len());
            for h in &hs {
                println!("{} read up to {} at {}", h.mri, h.last_read_id, h.read_ms);
            }
            println!(
                "read receipts enabled: {}",
                client.read_receipts_enabled().await?
            );
        }
        Some("team") => {
            let (client, cfg) = connect(&paths).await?;
            let r = team::run(&client, &args[1..]).await;
            // A busy keyring must not fail (and so invite a retry of) a write that already ran.
            if let Err(e) = session::persist(&cfg, &client).await {
                eprintln!("warning: could not store the refreshed session: {e}");
            }
            r?;
        }
        Some("find") => {
            let (client, cfg) = connect(&paths).await?;
            let r = find::run(&client, &cfg, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("translate") => {
            let (client, cfg) = connect(&paths).await?;
            let r = chatx::translate(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("appcard") => {
            let (client, cfg) = connect(&paths).await?;
            let name = cfg.display_name.clone().unwrap_or_default();
            let r = appcard::run(&client, &name, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("deliver") => {
            let (client, cfg) = connect(&paths).await?;
            let name = cfg.display_name.clone().unwrap_or_default();
            let r = delivery::run(&client, &name, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("images") => {
            let (client, cfg) = connect(&paths).await?;
            let r = images::run(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("preview") => {
            let (client, cfg) = connect(&paths).await?;
            let r = chatx::preview(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("settings") if settings::is_offline(&args[1..]) => settings::quiet(&args[2..])?,
        Some("settings") => {
            // A `set` is a dry run: the settings are read, never written.
            let (client, cfg) = connect(&paths).await?;
            let r = settings::run(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("preview-file") if preview_file::is_offline(&args[1..]) => {
            preview_file::offline(&args[1..])?
        }
        Some("preview-file") => {
            let (client, cfg) = connect(&paths).await?;
            let r = preview_file::run(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("search") => {
            let (client, cfg) = connect(&paths).await?;
            let me = self_mri(&client, &cfg).await?;
            let r = search::run(&client, &me, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("store") => {
            let (client, cfg) = connect(&paths).await?;
            let r = store::run(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("drive") if drive::is_offline(&args[1..]) => drive::offline(&args[1..])?,
        Some("call") => {
            let (client, cfg) = connect(&paths).await?;
            let r = call::run(&client, &args).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("drive") => {
            let (client, cfg) = connect(&paths).await?;
            let r = drive::run(&client, &args[1..]).await;
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("tasks" | "flows" | "onenote" | "lists") => {
            let (client, cfg) = connect(&paths).await?;
            let rest = &args[1..];
            let r = match args[0].as_str() {
                "tasks" => apps::tasks(&client, rest).await,
                "flows" => apps::flows(&client, rest).await,
                "onenote" => apps::onenote(&client, rest).await,
                _ => apps::lists(&client, rest).await,
            };
            session::persist(&cfg, &client).await?;
            r?;
        }
        Some("audiences") => {
            // Mints a token for every audience through the auth broker. Prints no token values.
            let (client, cfg) = connect(&paths).await?;
            for aud in teams_proto::broker::Audience::FIXED {
                match client.token_for(&aud).await {
                    Ok(_) => println!("ok    {}", aud.resource()),
                    Err(e) => println!("fail  {}: {e}", aud.resource()),
                }
            }
            // SharePoint is per host: use the host of the user's own OneDrive.
            let drive = client
                .json_as(
                    "drive",
                    &teams_proto::broker::Audience::Graph,
                    reqwest::Method::GET,
                    "https://graph.microsoft.com/v1.0/me/drive?$select=webUrl",
                    None,
                )
                .await?;
            let host = drive["webUrl"]
                .as_str()
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            match teams_proto::broker::Audience::sharepoint(&host) {
                Some(sp) => match client.token_for(&sp).await {
                    Ok(_) => println!("ok    SharePoint (OneDrive host)"),
                    Err(e) => println!("fail  SharePoint (OneDrive host): {e}"),
                },
                None => println!("skip  SharePoint: OneDrive host not found"),
            }
            session::persist(&cfg, &client).await?;
        }
        Some("calendar") if args.get(1).is_some_and(|a| a.parse::<u32>().is_err()) => {
            let (client, cfg) = connect(&paths).await?;
            calendar_check(&client, &args[1..]).await?;
            session::persist(&cfg, &client).await?;
        }
        Some("calendar") => {
            // Read-only: prints only counts (per UTC day and per show-as), never event details.
            let days: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(7);
            let (client, cfg) = connect(&paths).await?;
            let today = calendar::Date::from_ms(teams_proto::time::now_ms());
            let (start, end) =
                calendar::week_range(today, calendar::View::Day(days), calendar::Weekday::Monday);
            let events = calendar::Calendar::view(&client, start, end).await?;
            let mut per_day = std::collections::BTreeMap::new();
            let mut per_show_as = std::collections::BTreeMap::new();
            for e in &events {
                *per_day
                    .entry(calendar::Date::from_ms(e.start_ms))
                    .or_insert(0usize) += 1;
                *per_show_as
                    .entry(format!("{:?}", e.show_as))
                    .or_insert(0usize) += 1;
            }
            println!("{} events from {today} for {days} day(s)", events.len());
            for (day, n) in &per_day {
                println!("  {day}  {n}");
            }
            for (show_as, n) in &per_show_as {
                println!("  {show_as:<18} {n}");
            }
            session::persist(&cfg, &client).await?;
        }
        Some("tail") => {
            let (client, _) = connect(&paths).await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(teams_proto::trouter::run(client, tx));
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(TrouterEvent::Connected) => println!("-- connected"),
                        Some(TrouterEvent::Resync) => println!("-- resync"),
                        Some(TrouterEvent::Disconnected(r)) => println!("-- disconnected: {r}"),
                        Some(TrouterEvent::Event(ev)) => match *ev {
                            Event::NewMessage(m) if m.is_chat_content() => println!("[{}] {} in {}: {}", fmt_time(m.compose_ms), m.sender_name, m.conversation_id, preview(&m.content)),
                            Event::MessageUpdate(m) => println!("~ update {} in {} (reactions {})", m.id, m.conversation_id, m.reactions.len()),
                            Event::Typing { conversation_id, from } => println!("… {from} typing in {conversation_id}"),
                            other => println!("{other:?}"),
                        },
                        None => break,
                    },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
        }
        _ => {
            println!(
                "usage: ironteams-cli <command>\n\
                   login\n\
                   logout\n\
                   diag [--log FILE] [--out PATH]\n\
                   chats [n]\n\
                   teams\n\
                   calendar [days]\n\
                   calendar calendars|rooms|mailbox|schedule|worklocation|detail|ics\n\
                   calendar timezones|proposals\n\
                   calendar permissions [--dry-run <role> [address]]\n\
                   calendar probe-graph|probe-outlook <path>\n\
                   activity [n]\n\
                   read <conv> [n]\n\
                   send <conv> <text>\n\
                   send-image <conv> <file>\n\
                   send-file <conv> <file>\n\
                   post <channel> <subject> <text>\n\
                   react <conv> <msg> <key>\n\
                   forward <src> <msg> <target> [note]\n\
                   save/unsave/pin/unpin/unread <conv> <msg>\n\
                   mute/unmute/hide/unhide <conv>\n\
                   saved\n\
                   pins <chat>\n\
                   audiences\n\
                   people <q>\n\
                   search <q> [--kind messages|files|people|events] [--page N]\n\
                   presence [mri..]\n\
                   card [mri]\n\
                   horizons <conv>\n\
                   find <search|mentions|followed|shared|folder|feed> (search, quick views and shared files; counts only)\n\
                   drive <ls|search|recent|shared|favorites|recordings|libraries|sync-help|bin|mkdir|rm|selftest|purge>\n\
                   drive <meetings|for-you|shared-by-me|people|media|new|shape|shape-search>\n\
                   tasks [todo <n>|plan <n>]\n\
                   flows [runs <n> [top]]\n\
                   onenote [<notebook> [<section> [<page>]]]\n\
                   lists [<site> [<list>]]\n\
                   team <ls|discover|join|join-code|code|new|rm|edit|settings|archive|show|hide|channel|tag|members|community|storyline|sandbox|probe|raw>\n\
                   translate <languages|settings|message|channel>\n\
                   images <conv> [<msg id>|last] [--out <dir>] [--list]\n\
                   preview <url> [--image]\n\
                   settings <notifications|quiet|priority|privacy|blocked|shape>\n\
                   settings set <notify|pause|sound|meeting-chat|meeting-rsvp|privacy|block|unblock|priority-add|priority-remove> ... --dry-run\n\
                   preview-file <path>|--find <ext>|--local <file> [--page N] [--width W] [--out F]\n\
                   store <bar|more|home|search|app|mine|org|settings|install|uninstall|update|pin|unpin|move|probe|bots|extensions|ext-query>\n\
                   tail"
            );
        }
    }
    Ok(())
}
