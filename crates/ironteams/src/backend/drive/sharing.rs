use super::*;

impl Engine {
    pub(super) async fn drive_versions(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(item) = self.drive_one() else {
            return Ok(());
        };
        // The dialog's item is set before the list loads, so a Close sent meanwhile ends it.
        self.drive.lock().unwrap().versions_of = Some(item.clone());
        let list = if self.drive_offline() {
            crate::demo::drive_versions(now_ms())
        } else {
            ops::versions(&self.client, &ItemRef::from(&item)).await?
        };
        let still_open = self
            .drive
            .lock()
            .unwrap()
            .versions_of
            .as_ref()
            .is_some_and(|v| v.id == item.id);
        if still_open {
            self.ui
                .drive_versions(item.name, version_data(&list, now_ms()));
        }
        Ok(())
    }

    pub(super) async fn drive_version_restore(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let item = self.drive.lock().unwrap().versions_of.clone();
        let Some(item) = item else {
            return Ok(());
        };
        if self.demo_note() {
            return Ok(());
        }
        ops::restore_version(&self.client, &ItemRef::from(&item), id).await?;
        self.drive_status(format!("Restored an earlier version of {}", item.name));
        Ok(())
    }

    // -- share --------------------------------------------------------------------------------

    pub(super) async fn drive_share_begin(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(item) = self.drive_one() else {
            return Ok(());
        };
        self.drive_share_open(item).await
    }

    /// Opens the Share dialog for `item`, which need not be the OneDrive view's selection
    /// (Meeting Recaps shares a recording this way). The dialog lives in the OneDrive view.
    pub(in super::super) async fn drive_share_open(
        self: &Arc<Self>,
        item: DriveItem,
    ) -> anyhow::Result<()> {
        self.ui.open_view("onedrive".into());
        self.drive.lock().unwrap().share = Some(Share {
            item: item.clone(),
            role: Role::Write,
            results: Vec::new(),
            picked: Vec::new(),
            perms: Vec::new(),
            status: SHARE_HELP.into(),
        });
        self.drive_share_push();
        self.drive_share_reload_perms().await;
        Ok(())
    }

    pub(super) fn drive_share_push(&self) {
        let now = now_ms();
        let data = {
            let d = self.drive.lock().unwrap();
            let Some(s) = d.share.as_ref() else {
                return;
            };
            ShareData {
                name: s.item.name.clone(),
                results: s
                    .results
                    .iter()
                    .map(|p| PersonData {
                        picked: s.picked.iter().any(|x| x.id == p.id),
                        ..p.clone()
                    })
                    .collect(),
                picked: s.picked.clone(),
                perms: s.perms.iter().map(|p| perm_data(p, now)).collect(),
                role: role_str(s.role),
                status: s.status.clone(),
            }
        };
        self.ui.drive_share(data);
    }

    pub(super) fn drive_share_status(&self, text: impl Into<String>) {
        if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
            s.status = text.into();
        }
        self.drive_share_push();
    }

    pub(super) async fn drive_share_reload_perms(&self) {
        let item = {
            let d = self.drive.lock().unwrap();
            d.share.as_ref().map(|s| s.item.clone())
        };
        let Some(item) = item else {
            return;
        };
        let perms = if self.drive_offline() {
            crate::demo::drive_permissions()
        } else {
            match share::permissions(&self.client, &ItemRef::from(&item)).await {
                Ok(p) => p,
                Err(err) => {
                    self.drive_share_status(format!("Couldn't list who has access: {err}"));
                    return;
                }
            }
        };
        if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
            s.perms = perms;
        }
        self.drive_share_push();
    }

    pub(super) async fn drive_share_search(self: &Arc<Self>, q: &str) -> anyhow::Result<()> {
        let seq = {
            let mut d = self.drive.lock().unwrap();
            d.share_seq += 1;
            d.share_seq
        };
        if q.chars().count() < 2 {
            if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
                s.results.clear();
            }
            self.drive_share_push();
            return Ok(());
        }
        let people = if self.drive_offline() {
            crate::demo::drive_people(q)
        } else {
            self.client.search_people(&self.me, q).await?
        };
        let results = people
            .into_iter()
            .map(|p| PersonData {
                id: p.mri,
                name: p.display_name,
                detail: p.job_title.or(p.email.clone()).unwrap_or_default(),
                picked: false,
                email: p.email,
            })
            .collect();
        {
            let mut d = self.drive.lock().unwrap();
            // A slower answer to an older query never replaces a newer one.
            if d.share_seq != seq {
                return Ok(());
            }
            if let Some(s) = d.share.as_mut() {
                s.results = results;
            }
        }
        self.drive_share_push();
        Ok(())
    }

    pub(super) fn drive_share_pick(&self, id: &str) {
        if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
            match s.picked.iter().position(|p| p.id == id) {
                Some(i) => {
                    s.picked.remove(i);
                }
                None => {
                    if let Some(p) = s.results.iter().find(|p| p.id == id) {
                        s.picked.push(p.clone());
                    }
                }
            }
        }
        self.drive_share_push();
    }

    pub(super) async fn drive_share_send(self: &Arc<Self>, message: &str) -> anyhow::Result<()> {
        let (item, role, people) = {
            let d = self.drive.lock().unwrap();
            let Some(s) = d.share.as_ref() else {
                return Ok(());
            };
            (s.item.clone(), s.role, s.picked.clone())
        };
        let recipients: Vec<Recipient> = people.iter().filter_map(recipient_of).collect();
        if recipients.is_empty() {
            self.drive_share_status("Pick at least one person first");
            return Ok(());
        }
        if self.drive_offline() {
            self.drive_share_status("Demo mode: nothing is shared");
            return Ok(());
        }
        self.drive_share_status("Sharing...");
        match share::invite(
            &self.client,
            &ItemRef::from(&item),
            &recipients,
            role,
            message,
            true,
        )
        .await
        {
            Ok(_) => {
                if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
                    s.picked.clear();
                    s.results.clear();
                    s.status = format!(
                        "Shared with {} {}",
                        recipients.len(),
                        if recipients.len() == 1 {
                            "person"
                        } else {
                            "people"
                        }
                    );
                }
                self.drive_share_push();
                self.drive_share_reload_perms().await;
            }
            Err(err) => self.drive_share_status(format!("Couldn't share: {err}")),
        }
        Ok(())
    }

    pub(super) async fn drive_share_link(self: &Arc<Self>, arg: &str) -> anyhow::Result<()> {
        let Some(settings) = LinkSettings::parse(arg) else {
            return Ok(());
        };
        let item = self
            .drive
            .lock()
            .unwrap()
            .share
            .as_ref()
            .map(|s| s.item.clone());
        let Some(item) = item else {
            return Ok(());
        };
        if self.drive_offline() {
            self.drive_share_status("Demo mode: no link was created");
            return Ok(());
        }
        let expiry = settings.expiry_days.map(|d| link_expiry(d, now_ms()));
        let made = share::create_link(
            &self.client,
            &ItemRef::from(&item),
            settings.scope,
            settings.kind,
            expiry.as_deref(),
            settings.block_download,
        )
        .await;
        match made {
            Ok(p) => {
                let copied = p
                    .link_url
                    .as_deref()
                    .map(|u| {
                        arboard::Clipboard::new()
                            .and_then(|mut c| c.set_text(u))
                            .is_ok()
                    })
                    .unwrap_or(false);
                self.drive_share_status(if copied {
                    "Link created and copied"
                } else {
                    "Link created"
                });
                self.drive_share_reload_perms().await;
            }
            Err(err) => self.drive_share_status(format!("Couldn't create the link: {err}")),
        }
        Ok(())
    }

    pub(super) async fn drive_perm_role(
        self: &Arc<Self>,
        id: &str,
        role: Role,
    ) -> anyhow::Result<()> {
        let item = self
            .drive
            .lock()
            .unwrap()
            .share
            .as_ref()
            .map(|s| s.item.clone());
        let Some(item) = item else {
            return Ok(());
        };
        if self.drive_offline() {
            self.drive_share_status("Demo mode: nothing is changed");
            return Ok(());
        }
        match share::update_permission(&self.client, &ItemRef::from(&item), id, role).await {
            Ok(()) => self.drive_share_reload_perms().await,
            Err(err) => self.drive_share_status(format!("Couldn't change access: {err}")),
        }
        Ok(())
    }

    pub(super) async fn drive_perm_remove(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let item = self
            .drive
            .lock()
            .unwrap()
            .share
            .as_ref()
            .map(|s| s.item.clone());
        let Some(item) = item else {
            return Ok(());
        };
        if self.drive_offline() {
            self.drive_share_status("Demo mode: nothing is changed");
            return Ok(());
        }
        match share::remove_permission(&self.client, &ItemRef::from(&item), id).await {
            Ok(()) => self.drive_share_reload_perms().await,
            Err(err) => self.drive_share_status(format!("Couldn't remove access: {err}")),
        }
        Ok(())
    }
}
