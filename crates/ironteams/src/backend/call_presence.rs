//! "In a call" presence: while a call or meeting is connected, this device publishes
//! Busy / `InACall` (`InAConferenceCall` for a meeting) to the presence service, and Available
//! when it ends, as Teams does. The device's presence is aggregated with the user's other
//! devices, so a status the user forced stays in charge.
//!
//! [`install`] is called once with the signed-in client; [`update`] from the call manager on
//! every call state change. Repeated updates with the same state send nothing. Each publish also
//! asks for our own presence to be read again, so the status shown follows at once.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use teams_proto::Client;
use teams_proto::presence::Availability;

static CLIENT: OnceLock<Client> = OnceLock::new();
/// How long after publishing our own presence is read a second time.
const SETTLE: Duration = Duration::from_secs(3);
/// The activity last published: `None` for no call (Available).
static PUBLISHED: Mutex<Option<&'static str>> = Mutex::new(None);

/// Whether a call is connected now (as last published).
pub fn in_call() -> bool {
    PUBLISHED.lock().unwrap().is_some()
}

/// Remembers the client presence updates go through. Tests never install one (no network).
#[cfg_attr(test, allow(dead_code))]
pub fn install(client: Client) {
    let _ = CLIENT.set(client);
}

/// The activity for a call state, `None` when no call is connected.
pub fn activity(connected: bool, meeting: bool) -> Option<&'static str> {
    match (connected, meeting) {
        (false, _) => None,
        (true, true) => Some("InAConferenceCall"),
        (true, false) => Some("InACall"),
    }
}

/// Whether `want` differs from what was last published (and records it).
fn changed(last: &Mutex<Option<&'static str>>, want: Option<&'static str>) -> bool {
    let mut last = last.lock().unwrap();
    if *last == want {
        return false;
    }
    *last = want;
    true
}

/// Publishes the presence for the current call state, in the background.
pub fn update(connected: bool, meeting: bool) {
    let want = activity(connected, meeting);
    if !changed(&PUBLISHED, want) {
        return;
    }
    // A short cue as the call connects and as it ends.
    super::sounds::call_cue(want.is_some());
    let (Some(client), Ok(rt)) = (CLIENT.get(), tokio::runtime::Handle::try_current()) else {
        return;
    };
    let client = client.clone();
    rt.spawn(async move {
        let r = match want {
            Some(a) => client.publish_endpoint(Availability::Busy, a).await,
            None => {
                client
                    .publish_endpoint(Availability::Available, "Available")
                    .await
            }
        };
        if let Err(e) = r {
            tracing::warn!("in-call presence: {e}");
        }
        // The status shown to us follows the service: read it now, and once more shortly after
        // in case the service had not combined the devices yet, instead of at the next minute.
        super::people_presence::refresh_mine();
        tokio::time::sleep(SETTLE).await;
        super::people_presence::refresh_mine();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connected_call_or_meeting_is_in_a_call() {
        assert_eq!(activity(false, false), None);
        assert_eq!(activity(false, true), None);
        assert_eq!(activity(true, false), Some("InACall"));
        assert_eq!(activity(true, true), Some("InAConferenceCall"));
    }

    #[test]
    fn only_a_change_is_published() {
        let last = Mutex::new(None);
        assert!(!changed(&last, None));
        assert!(changed(&last, Some("InACall")));
        assert!(!changed(&last, Some("InACall")));
        assert!(changed(&last, None));
    }
}
