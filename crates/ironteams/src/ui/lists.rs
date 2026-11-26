//! UI-thread side of the chat list sections, item menus and list options: fills the `ListStore`
//! global (plan 8.1 to 8.3, 8.5).

use super::{UiHandle, chat_row, model};
use crate::lists_data::{ChannelMenuData, ChoiceData, ListOptions, SectionData};
use crate::{AppWindow, ListStore, SectionChoice, SectionRow};
use slint::ComponentHandle;

impl UiHandle {
    fn list_store(&self, f: impl FnOnce(ListStore<'_>) + Send + 'static) {
        self.run(move |w: AppWindow| f(w.global::<ListStore>()));
    }

    /// The sections between Favorites and Chats, and where "Move to section" can put an item.
    pub fn sections(&self, sections: Vec<SectionData>, choices: Vec<ChoiceData>) {
        self.list_store(move |s| {
            s.set_sections(model(
                sections
                    .into_iter()
                    .map(|d| SectionRow {
                        id: d.id.into(),
                        name: d.name.into(),
                        expanded: d.expanded,
                        unread: d.unread,
                        local: d.local,
                        sort: d.sort.into(),
                        items: model(d.items.into_iter().map(chat_row).collect()),
                    })
                    .collect(),
            ));
            s.set_choices(model(
                choices
                    .into_iter()
                    .map(|c| SectionChoice {
                        id: c.id.into(),
                        name: c.name.into(),
                    })
                    .collect(),
            ));
        });
    }

    /// The "Sort by" choice of Favorites, Chats and Teams and channels (`recent`, `alpha`, `unread`).
    pub fn fixed_sorts(
        &self,
        favorites: &'static str,
        chats: &'static str,
        channels: &'static str,
    ) {
        self.list_store(move |s| {
            s.set_favorites_sort(favorites.into());
            s.set_chats_sort(chats.into());
            s.set_channels_sort(channels.into());
        });
    }

    pub fn list_options(&self, o: ListOptions) {
        self.list_store(move |s| {
            s.set_show_previews(o.previews);
            s.set_show_times(o.times);
            s.set_combined(o.combined);
        });
    }

    /// Opens or closes the fixed sections (Favorites, Chats, Meetings, Teams and channels).
    pub fn fixed_sections_open(&self, open: bool) {
        self.list_store(move |s| {
            s.set_favorites_open(open);
            s.set_chats_open(open);
            s.set_meetings_open(open);
            s.set_channels_open(open);
        });
    }

    /// What the open channel menu shows.
    pub fn channel_menu(&self, m: ChannelMenuData) {
        self.list_store(move |s| {
            s.set_channel_level(m.level.into());
            s.set_channel_private(m.private);
            s.set_channel_hidden(m.hidden);
            s.set_channel_pinned(m.pinned);
        });
    }

    /// Switches the left rail to another view (`chat`, `teams`, ...).
    pub fn show_view(&self, view: &'static str) {
        self.run(move |w| w.set_view(view.into()));
    }
}
