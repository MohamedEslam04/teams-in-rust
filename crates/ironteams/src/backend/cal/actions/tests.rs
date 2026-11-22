use super::*;

// ---- the engine end to end, offline (nothing is sent; the cache changes locally) ----

use crate::ui_api::UiHandle;
use crate::{AppWindow, CalForm, CalStore};
use i_slint_backend_testing as testing;
use slint::{ComponentHandle, Model};

mod flows;
mod joins;
mod unit;

struct Rig {
    app: AppWindow,
    engine: Arc<Engine>,
    rt: tokio::runtime::Runtime,
}

impl Rig {
    fn new() -> Rig {
        thread_local! { static INIT: std::sync::Once = const { std::sync::Once::new() }; }
        INIT.with(|o| o.call_once(testing::init_no_event_loop));
        let app = AppWindow::new().unwrap();
        app.set_login_state("ready".into());
        app.set_view("calendar".into());
        app.show().unwrap();
        let store = store::Store::open_in_memory().unwrap();
        for (mri, name, mail) in [
            ("8:orgid:aaa", "Me", "me@contoso.com"),
            ("8:orgid:bbb", "Bob Lee", "bob@contoso.com"),
        ] {
            store
                .upsert_user(&store::UserRow {
                    mri: mri.into(),
                    display_name: name.into(),
                    email: Some(mail.into()),
                    avatar: None,
                    updated_ms: 0,
                })
                .unwrap();
        }
        let engine = Engine::new(
            teams_proto::Client::offline("test").unwrap(),
            store,
            session::Config::default(),
            UiHandle::new(app.as_weak()),
            "8:orgid:aaa".into(),
            "Me".into(),
        );
        engine.offline.store(true, Ordering::Relaxed);
        {
            let mut s = engine.cal.lock();
            s.date = Date::new(2026, 11, 18);
            s.me_email = "me@contoso.com".into();
        }
        Rig {
            app,
            engine,
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    fn seed(&self, evs: serde_json::Value) {
        let changes = ::calendar::parse_events(&evs)
            .into_iter()
            .map(EventChange::Upsert)
            .collect();
        self.engine.cal.lock().cache.apply(changes);
        self.engine.push_calendar();
    }

    fn run(&self, cmd: CalCmd) {
        self.rt
            .block_on(self.engine.calendar_cmd(cmd))
            .expect("command");
    }

    fn ids(&self) -> Vec<String> {
        self.app
            .global::<CalStore>()
            .get_blocks()
            .iter()
            .map(|b| b.id.to_string())
            .collect()
    }
}

fn ev_json(id: &str, extra: Value) -> Value {
    let mut v = json!({"id": id, "subject": format!("Subject {id}"),
        "start": {"dateTime": "2026-11-18T09:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-11-18T10:00:00.0000000", "timeZone": "UTC"}});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    v
}

fn meeting_extra(response: &str) -> Value {
    json!({
        "responseStatus": {"response": response},
        "organizer": {"emailAddress": {"name": "Alice", "address": "alice@contoso.com"}},
        "attendees": [{"emailAddress": {"name": "Bob", "address": "bob@contoso.com"}, "type": "required", "status": {"response": "none"}}]
    })
}

/// A join went to the pre-join screen, or (native calls off) to the browser.
fn was_joined(r: &Rig, link: &str) -> bool {
    r.engine.calls.is_in_meeting(link)
        || crate::external::suppressed_targets()
            .iter()
            .any(|t| t == link)
}
