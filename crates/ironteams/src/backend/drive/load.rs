use super::*;

impl Engine {
    /// Libraries (Quick access), favorites and the "For you" cards, which the list needs but
    /// should not wait for.
    pub(super) async fn drive_background_load(self: Arc<Self>) {
        if self.drive_offline() {
            let libs = crate::demo::drive_libraries()
                .into_iter()
                .map(|(drive_id, name)| Library { drive_id, name })
                .collect();
            let favs = crate::demo::drive_favorites();
            {
                let mut d = self.drive.lock().unwrap();
                d.s.libraries = libs;
                d.s.favorites = favs;
            }
            self.drive_push();
            self.drive_load_cards().await;
            return;
        }
        let cards = self.clone();
        tokio::spawn(async move { cards.drive_load_cards().await });
        if let Ok(p) = browse::following(&self.client).await {
            self.drive.lock().unwrap().s.favorites = p.items.into_iter().map(|i| i.id).collect();
            self.drive_push();
        }
        let mut libs = Vec::new();
        if let Ok(sites) = browse::followed_sites(&self.client).await {
            for site in sites.into_iter().take(12) {
                if let Ok(drives) = browse::site_drives(&self.client, &site.id).await
                    && let Some(d) = drives.into_iter().next()
                {
                    libs.push(Library {
                        drive_id: d.id,
                        name: site.name,
                    });
                }
            }
        }
        self.drive.lock().unwrap().s.libraries = libs;
        self.drive_push();
    }

    pub(super) async fn drive_go(self: &Arc<Self>, section: Section) -> anyhow::Result<()> {
        // Libraries come from the background load; wait for it on first use.
        let g = {
            let mut d = self.drive.lock().unwrap();
            // Photos and videos are for looking at: start them as tiles.
            if section == Section::Media {
                d.s.grid = true;
            }
            d.s.go(section)
        };
        self.drive_push();
        self.drive_load(g).await
    }

    /// Lists the current section and folder, and shows the result unless the user moved on.
    pub(super) async fn drive_load(self: &Arc<Self>, generation: u64) -> anyhow::Result<()> {
        let (section, trail, libs) = {
            let d = self.drive.lock().unwrap();
            (
                d.s.section.clone(),
                d.s.trail.clone(),
                d.s.libraries.clone(),
            )
        };
        let result = self.drive_fetch(&section, &trail, &libs, None).await;
        let used = {
            let mut d = self.drive.lock().unwrap();
            match result {
                Ok((items, next)) => d.s.set_listing(generation, items, next),
                Err(err) => {
                    if generation == d.s.generation {
                        d.s.loading = false;
                        d.s.status = format!("Couldn't load this list: {err}");
                    }
                    false
                }
            }
        };
        self.drive_push();
        if used {
            self.drive_thumbs();
            if section == Section::Meetings {
                let e = self.clone();
                tokio::spawn(async move { e.drive_meetings_everywhere(generation).await });
            }
        }
        Ok(())
    }

    /// Meetings first lists the `Recordings` folder of My files (one request); this adds the
    /// channel libraries' recordings, which take a request per channel, and swaps the list.
    pub(super) async fn drive_meetings_everywhere(self: Arc<Self>, generation: u64) {
        let items: Vec<DriveItem> = if self.drive_offline() {
            let mut v = crate::demo::drive_listing("meetings", "", now_ms());
            v.extend(crate::demo::drive_channel_recordings(now_ms()));
            v.sort_by_key(|i| std::cmp::Reverse(i.modified_ms));
            v
        } else {
            match onedrive::meeting_recordings(&self.client).await {
                Ok(r) => recording_items(r),
                Err(err) => {
                    tracing::warn!(%err, "meeting recordings failed");
                    return;
                }
            }
        };
        let used = self
            .drive
            .lock()
            .unwrap()
            .s
            .set_listing(generation, items, None);
        if used {
            self.drive_push();
        }
    }

    pub(super) async fn drive_load_more(self: &Arc<Self>) -> anyhow::Result<()> {
        let (generation, trail, next, section) = {
            let d = self.drive.lock().unwrap();
            (
                d.s.generation,
                d.s.trail.clone(),
                d.s.next_link.clone(),
                d.s.section.clone(),
            )
        };
        if section == Section::Media {
            // The "next link" of Media is the offset of the next page.
            let Some(from) = next.and_then(|n| n.parse::<u32>().ok()) else {
                return Ok(());
            };
            let page = browse::media(&self.client, from, MEDIA_PAGE).await?;
            let next = page.more.then(|| (from + MEDIA_PAGE).to_string());
            self.drive
                .lock()
                .unwrap()
                .s
                .append_listing(generation, page.items, next);
            self.drive_push();
            self.drive_thumbs();
            return Ok(());
        }
        let (Some(next), Some(folder)) = (next, trail.last()) else {
            return Ok(());
        };
        let page =
            browse::children(&self.client, &folder.drive_id, &folder.item_id, Some(&next)).await?;
        self.drive
            .lock()
            .unwrap()
            .s
            .append_listing(generation, page.items, page.next_link);
        self.drive_push();
        self.drive_thumbs();
        Ok(())
    }

    /// Who shared files with the user: fetched by the People list, then kept for the person
    /// lists that open from it.
    pub(super) async fn drive_people(&self) -> anyhow::Result<Vec<PersonFiles>> {
        let (listing, cached) = {
            let d = self.drive.lock().unwrap();
            (d.s.section == Section::People, d.people.clone())
        };
        let people = if self.drive_offline() {
            crate::demo::drive_people_files(now_ms())
        } else if listing || cached.is_empty() {
            insights::people(&self.client).await?
        } else {
            cached
        };
        self.drive.lock().unwrap().people = people.clone();
        Ok(people)
    }

    pub(super) async fn drive_fetch(
        &self,
        section: &Section,
        trail: &[Crumb],
        libs: &[Library],
        next: Option<&str>,
    ) -> anyhow::Result<(Vec<DriveItem>, Option<String>)> {
        // The People lists work the same online and in the demo (`drive_people` knows which).
        match section {
            Section::People => return Ok((people_items(&self.drive_people().await?), None)),
            Section::Person(name) => {
                let files = self
                    .drive_people()
                    .await?
                    .into_iter()
                    .find(|p| &person_label(&p.person) == name)
                    .map(|p| p.files);
                return Ok((files.unwrap_or_default(), None));
            }
            _ => {}
        }
        if self.drive_offline() {
            let folder = trail.last().map_or("", |c| c.item_id.as_str());
            return Ok((
                crate::demo::drive_listing(&section.key(), folder, now_ms()),
                None,
            ));
        }
        let c = &self.client;
        Ok(match section {
            Section::Home => (browse::recent(c).await?, None),
            Section::Shared => (browse::shared_with_me(c).await?, None),
            Section::Favorites => (browse::following(c).await?.items, None),
            Section::Meetings => (onedrive::recordings(c).await?, None),
            Section::SharedByYou => (shared_by_you_items(insights::shared_by_me(c).await?), None),
            Section::Media => {
                let page = browse::media(c, 0, MEDIA_PAGE).await?;
                (page.items, page.more.then(|| MEDIA_PAGE.to_string()))
            }
            // Answered above, before the demo check.
            Section::People | Section::Person(_) => (Vec::new(), None),
            Section::Search(q) => (browse::search(c, q).await?, None),
            Section::Libraries => (libs.iter().map(library_item).collect(), None),
            Section::Bin => (
                bin::recycle_bin(c)
                    .await?
                    .into_iter()
                    .map(bin_item)
                    .collect(),
                None,
            ),
            Section::MyFiles | Section::Library(_) => {
                let f = trail
                    .last()
                    .ok_or_else(|| anyhow::anyhow!("no folder to list"))?;
                let page = browse::children(c, &f.drive_id, &f.item_id, next).await?;
                (page.items, page.next_link)
            }
        })
    }

    /// Fetches thumbnails in the background: the "For you" cards on Home, and the tiles of the
    /// grid view. Tiles whose listing has no thumbnail address (Search hits, so Media) ask for
    /// it per item.
    pub(super) fn drive_thumbs(self: &Arc<Self>) {
        if self.drive_offline() {
            return;
        }
        let (generation, wanted): (u64, Vec<(String, ThumbSource)>) = {
            let d = self.drive.lock().unwrap();
            let mut wanted = Vec::new();
            if d.s.section == Section::Home {
                wanted.extend(
                    d.s.cards
                        .iter()
                        .filter(|c| !d.s.thumbs.contains_key(&c.id))
                        .map(|c| (c.id.clone(), ThumbSource::Item(ItemRef::from(&c.item)))),
                );
            }
            if d.s.grid {
                let per_item = d.s.section == Section::Media;
                wanted.extend(
                    d.s.items
                        .iter()
                        .filter(|i| !d.s.thumbs.contains_key(&i.id))
                        .filter_map(|i| {
                            let src = match &i.thumbnail_url {
                                Some(u) => ThumbSource::Url(u.clone()),
                                None if per_item && !i.drive_id.is_empty() => {
                                    ThumbSource::Item(ItemRef::from(i))
                                }
                                None => return None,
                            };
                            Some((i.id.clone(), src))
                        })
                        .take(MAX_THUMBS),
                );
            }
            (d.s.generation, wanted)
        };
        if wanted.is_empty() {
            return;
        }
        let e = self.clone();
        tokio::spawn(async move {
            for batch in wanted.chunks(4) {
                let mut set = tokio::task::JoinSet::new();
                for (id, src) in batch.iter().cloned() {
                    let client = e.client.clone();
                    set.spawn(async move {
                        let url = match src {
                            ThumbSource::Url(u) => u,
                            ThumbSource::Item(r) => {
                                let t = browse::thumbnails(&client, &r).await.ok()?;
                                t.large.or(t.medium).or(t.small)?
                            }
                        };
                        let bytes = client.bytes(&url, &[], 3 * 1024 * 1024).await.ok()?;
                        let px = tokio::task::spawn_blocking(move || {
                            super::super::util::decode_image(&bytes, 480, 320)
                        })
                        .await
                        .ok()??;
                        Some((id, px))
                    });
                }
                while let Some(r) = set.join_next().await {
                    if let Ok(Some((id, px))) = r {
                        let mut d = e.drive.lock().unwrap();
                        if d.s.generation != generation {
                            return;
                        }
                        d.s.thumbs.insert(id, px);
                    }
                }
                e.drive_push();
            }
        });
    }

    /// Loads Home's "For you" cards and fetches their thumbnails.
    pub(super) async fn drive_load_cards(self: Arc<Self>) {
        let found = if self.drive_offline() {
            crate::demo::drive_for_you(now_ms())
        } else {
            match insights::for_you(&self.client, MAX_CARDS).await {
                Ok(c) => c,
                Err(err) => {
                    tracing::warn!(%err, "for you failed");
                    return;
                }
            }
        };
        let cards: Vec<Card> = found
            .into_iter()
            .map(|c| {
                let item = c.insight.to_drive_item();
                Card {
                    id: format!("card:{}/{}", item.drive_id, item.id),
                    item,
                    reason: c.reason,
                }
            })
            .collect();
        self.drive.lock().unwrap().s.cards = cards;
        self.drive_push();
        self.drive_thumbs();
    }
}
