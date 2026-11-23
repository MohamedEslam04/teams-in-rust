//! UI-thread side of the header's live-call strip: fills the `OngoingCallStore` global.

use super::{UiHandle, img, model, rgb};
use crate::data::OngoingCallData;
use crate::{AppWindow, OngoingCallStore, OngoingPerson};
use slint::ComponentHandle;

impl UiHandle {
    /// The open conversation's call going on (`live: false` hides the strip).
    pub fn ongoing_call(&self, data: OngoingCallData) {
        self.run(move |w: AppWindow| {
            let s = w.global::<OngoingCallStore>();
            s.set_chat_id(data.chat_id.into());
            s.set_live(data.live);
            s.set_elapsed(data.elapsed.into());
            s.set_people(model(
                data.people
                    .into_iter()
                    .map(|p| {
                        let (photo, has_photo) = img(&p.avatar);
                        OngoingPerson {
                            id: p.mri.into(),
                            name: p.name.into(),
                            initials: p.initials.into(),
                            color: rgb(p.color),
                            photo,
                            has_photo,
                        }
                    })
                    .collect(),
            ));
            s.set_more(data.more);
            s.set_names(data.names.into());
        });
    }

    /// The strip's timer, once a second while `chat_id`'s call goes on.
    pub fn ongoing_call_elapsed(&self, chat_id: &str, elapsed: String) {
        let chat_id = chat_id.to_owned();
        self.run(move |w: AppWindow| {
            let s = w.global::<OngoingCallStore>();
            if s.get_chat_id() == chat_id.as_str() {
                s.set_elapsed(elapsed.into());
            }
        });
    }
}
