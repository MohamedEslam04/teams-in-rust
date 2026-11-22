//! UI-thread side of the people and conversation pieces: fills the `PeopleStore` global.

use super::{UiHandle, img, model, rgb};
use crate::data::{CardData, OrgData, PersonData, PinnedData, color_for, initials};
use crate::{AppWindow, PeopleStore, PersonItem, PinnedItem, SettingsStore};
use slint::{ComponentHandle, ModelRc};

fn person(d: PersonData) -> PersonItem {
    let (avatar, has_avatar) = img(&d.avatar);
    PersonItem {
        initials: initials(&d.name).into(),
        color: rgb(color_for(&d.id)),
        id: d.id.into(),
        name: d.name.into(),
        subtitle: d.subtitle.into(),
        avatar,
        has_avatar,
        kind: d.kind.into(),
        presence: d.presence.into(),
    }
}

fn fill_card(s: &PeopleStore<'_>, c: CardData) {
    s.set_card_loading(c.loading);
    s.set_card(person(c.person));
    s.set_card_email(c.email.into());
    s.set_card_work(c.work.into());
    s.set_card_location(c.location.into());
    s.set_card_time(c.time.into());
    s.set_card_presence(c.presence.into());
    s.set_card_presence_label(c.presence_label.into());
    s.set_card_work_location(c.work_location.into());
    s.set_card_note(c.note.into());
    s.set_card_ooo(c.ooo.into());
    s.set_card_phone(c.phone.into());
    s.set_card_mobile(c.mobile.into());
    s.set_card_manager(c.manager.into());
    s.set_card_badge(c.badge.into());
    s.set_card_free_at(c.free_at.into());
    s.set_card_work_hours(c.work_hours.into());
    s.set_card_work_location_kind(c.work_location_kind.into());
    s.set_card_job_title(c.job_title.into());
    s.set_card_department(c.department.into());
    s.set_card_reports(people(c.reports));
    s.set_card_reports_count(c.reports_count as i32);
    s.set_card_manager_person(person(c.manager_person.unwrap_or_default()));
}

fn fill_org(s: &PeopleStore<'_>, o: OrgData) {
    s.set_org_loading(o.loading);
    s.set_org_focus(person(o.focus));
    s.set_org_location_kind(o.location_kind.into());
    s.set_org_manager(person(o.manager.unwrap_or_default()));
    s.set_org_reports(people(o.reports));
    s.set_org_reports_count(o.reports_count);
    s.set_org_can_back(o.can_back);
    s.set_org_can_forward(o.can_forward);
}

fn people(v: Vec<PersonData>) -> ModelRc<PersonItem> {
    model(v.into_iter().map(person).collect())
}

impl UiHandle {
    fn store(&self, f: impl FnOnce(PeopleStore<'_>) + Send + 'static) {
        self.run(move |w: AppWindow| f(w.global::<PeopleStore>()));
    }

    /// Candidates of the @mention picker.
    pub fn mention_items(&self, items: Vec<PersonData>) {
        self.store(move |s| s.set_mention_items(people(items)));
    }

    /// The New chat pane: open or closed, who is addressed, the suggestions and a hint.
    pub fn new_chat(
        &self,
        open: bool,
        to: Vec<PersonData>,
        results: Vec<PersonData>,
        hint: String,
    ) {
        self.store(move |s| {
            s.set_new_chat_open(open);
            s.set_new_chat_to(people(to));
            s.set_new_chat_results(people(results));
            s.set_new_chat_hint(hint.into());
        });
    }

    /// Chat header state: whether the pencil shows (group chats), and the count of the
    /// participants button (group and 1:1 chats; 0 hides it).
    pub fn group_header(&self, group: bool, count: usize) {
        self.store(move |s| {
            s.set_header_group(group);
            s.set_member_count(count as i32);
            if count == 0 {
                s.set_participants_open(false);
            }
        });
    }

    pub fn close_participants(&self) {
        self.store(|s| s.set_participants_open(false));
    }

    /// The participants popover: members, and the suggestions of "Add people".
    pub fn members(&self, members: Vec<PersonData>, add_results: Vec<PersonData>) {
        self.store(move |s| {
            s.set_member_count(members.len() as i32);
            s.set_members(people(members));
            s.set_add_results(people(add_results));
        });
    }

    pub fn leave_confirm(&self, chat: String) {
        self.store(move |s| s.set_leave_confirm(chat.into()));
    }

    /// Opens the profile card (or fills the open one).
    pub fn card(&self, open: bool, c: CardData) {
        self.store(move |s| {
            s.set_card_open(open);
            if !open {
                // Closing the card closes the full profile too (a chat opened from it).
                s.set_profile_open(false);
            }
            fill_card(&s, c);
        });
    }

    /// Fills the card data without opening or closing the card: the full profile of a person
    /// the card was not opened for.
    pub fn card_fill(&self, c: CardData) {
        self.store(move |s| fill_card(&s, c));
    }

    /// The organization chart of the full profile.
    pub fn org(&self, o: OrgData) {
        self.store(move |s| fill_org(&s, o));
    }

    /// Fills in the card of `c.person` with what arrived, if it is still the open card (a hover
    /// card may have closed, or moved to someone else, while the details were on their way).
    pub fn card_details(&self, c: CardData) {
        self.store(move |s| {
            if (s.get_card_open() || s.get_profile_open())
                && s.get_card().id == c.person.id.as_str()
            {
                fill_card(&s, c);
            }
        });
    }

    /// Our own MRI: the photo at the top right opens our own card.
    pub fn me_id(&self, mri: String) {
        self.store(move |s| {
            if s.get_me() != mri.as_str() {
                s.set_me(mri.into());
            }
        });
    }

    /// Our own availability, for the dot on the photo at the top right.
    pub fn me_presence(&self, availability: String) {
        self.run(move |w: AppWindow| {
            w.global::<SettingsStore>()
                .set_me_presence(availability.into())
        });
    }

    /// The pins of the open chat for the banner.
    pub fn pinned(&self, items: Vec<PinnedData>) {
        self.store(move |s| {
            let n = items.len() as i32;
            if s.get_pinned_index() >= n {
                s.set_pinned_index(0);
            }
            s.set_pinned(model(
                items
                    .into_iter()
                    .map(|p| PinnedItem {
                        id: p.id.into(),
                        sender: p.sender.into(),
                        preview: p.preview.into(),
                    })
                    .collect(),
            ));
        });
    }

    pub fn receipts_on(&self, on: bool) {
        self.store(move |s| s.set_receipts_on(on));
    }
}
