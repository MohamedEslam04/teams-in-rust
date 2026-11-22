use super::*;

impl Engine {
    /// Runs `f` on the store (tests seed and inspect it).
    #[cfg(test)]
    pub(crate) fn test_store<R>(&self, f: impl FnOnce(&store::Store) -> R) -> R {
        f(&self.store.lock().unwrap())
    }

    /// Search results the New chat pane would have received.
    #[cfg(test)]
    pub(crate) fn test_new_chat_results(self: &Arc<Self>, found: Vec<Person>) {
        self.people.lock().unwrap().new_chat.results = found;
        self.push_new_chat(true);
    }

    /// Search results the participants popover's "Add people" would have received.
    #[cfg(test)]
    pub(crate) fn test_add_results(self: &Arc<Self>, found: Vec<Person>) {
        let conv = self.current_chat().unwrap_or_default();
        let members = self.members_known(&conv).unwrap_or_default();
        self.people.lock().unwrap().add_results = found.clone();
        let data = members.iter().map(|m| self.member_data(m)).collect();
        let add = found.iter().map(|f| self.person_data(f)).collect();
        self.ui.members(data, add);
    }

    #[cfg(test)]
    pub(crate) fn test_member_name(self: &Arc<Self>, mri: &str) -> String {
        self.member_data(mri).name
    }

    #[cfg(test)]
    pub(crate) fn test_set_standing(&self, mri: &str, standing: Standing) {
        self.people
            .lock()
            .unwrap()
            .standing
            .insert(mri.into(), standing);
    }

    #[cfg(test)]
    pub(crate) fn test_set_members(&self, conv: &str, members: &[&str]) {
        self.people
            .lock()
            .unwrap()
            .members
            .insert(conv.into(), members.iter().map(|m| m.to_string()).collect());
    }

    #[cfg(test)]
    pub(crate) fn test_set_tags(&self, team: &str, tags: Vec<Tag>) {
        self.people.lock().unwrap().tags.insert(team.into(), tags);
    }

    /// Remembers a person's card details as if a card had read them.
    #[cfg(test)]
    pub(crate) fn test_cache_profile(&self, mri: &str, subtitle: &str, work: &str, phone: &str) {
        let (job, dept) = subtitle.split_once(" · ").unwrap_or((subtitle, ""));
        let facts = CardFacts {
            subtitle: subtitle.into(),
            job_title: job.into(),
            department: dept.into(),
            work: work.into(),
            phone: phone.into(),
            ..CardFacts::default()
        };
        self.people
            .lock()
            .unwrap()
            .profiles
            .insert(mri.to_string(), (facts, std::time::Instant::now()));
    }

    #[cfg(test)]
    pub(crate) fn test_set_pins(&self, conv: &str, pins: Vec<PinnedData>) {
        self.people
            .lock()
            .unwrap()
            .pins
            .insert(conv.into(), pins.clone());
        self.ui.pinned(pins);
    }

    #[cfg(test)]
    pub(crate) fn test_set_horizons(&self, conv: &str, h: Vec<Horizon>) {
        self.people.lock().unwrap().horizons.insert(conv.into(), h);
    }
}
