use super::*;

impl Engine {
    pub(in super::super) async fn settings_cmd(
        self: &Arc<Self>,
        cmd: SettingsCmd,
    ) -> anyhow::Result<()> {
        match cmd {
            SettingsCmd::Load => self.settings_reload(true).await,
            SettingsCmd::Redraw => {
                self.mark(|d| {
                    d.chats = true;
                    d.messages = true;
                    d.teams = true;
                    d.header = true;
                    d.activity = true;
                });
                // The panels that are not part of the render loop write their times again.
                self.push_calendar();
                self.remind_push();
                self.push_scheduled();
                #[cfg(feature = "calls")]
                self.push_calls_page();
            }
            SettingsCmd::RefreshLicense => {
                let r = self.refresh_license().await.map_err(|e| e.to_string());
                self.ui.settings_license(r);
            }
            SettingsCmd::Apply(req) => {
                // One write at a time, in the order they were made: the settings bag is written
                // whole, so an older write landing last would undo a newer one.
                let turn = self.settings_writes.lock().await;
                let r = if self.offline.load(Ordering::Relaxed) {
                    Err(anyhow::anyhow!("Not available offline."))
                } else {
                    self.client
                        .send_planned(&req)
                        .await
                        .map_err(anyhow::Error::from)
                };
                drop(turn);
                match r {
                    Ok(_) => self.ui.settings_notice("Saved to Teams.".into()),
                    Err(e) => {
                        tracing::warn!(%e, "settings write failed");
                        self.ui
                            .settings_notice(format!("Teams did not take the change: {e}"));
                    }
                }
                self.settings_reload(true).await;
            }
            SettingsCmd::ChangePhoto => self.change_photo().await,
            SettingsCmd::RemovePhoto => self.remove_photo().await,
            SettingsCmd::SetOpenChatAt(at) => {
                self.cfg.lock().unwrap().open_chat_at = at;
                self.save_cfg();
            }
            SettingsCmd::SetStatus(..)
            | SettingsCmd::SetWorkLocation(_)
            | SettingsCmd::SetStatusNote(..) => {
                let turn = self.settings_writes.lock().await;
                let r = self.set_my_presence(&cmd).await;
                drop(turn);
                if let Err(e) = r {
                    tracing::warn!(%e, "could not change my status");
                    let why = format!("Could not change your status: {e}");
                    // The menu showed the picked status at once: it goes back.
                    if matches!(cmd, SettingsCmd::SetStatus(..)) {
                        self.ui.settings_status_failed(why.clone());
                    }
                    self.ui.settings_notice(why);
                }
                self.settings_reload(false).await;
            }
        }
        Ok(())
    }

    /// Reads the Teams-side settings and shows them, unless a newer read started meanwhile (an
    /// older read can see the bag from before the latest write). `remember` keeps the
    /// notification settings for the banners.
    pub(super) async fn settings_reload(self: &Arc<Self>, remember: bool) {
        let ticket = self.settings_reads.fetch_add(1, Ordering::SeqCst) + 1;
        let data = self.settings_read().await;
        if self.settings_reads.load(Ordering::SeqCst) != ticket {
            return;
        }
        if remember {
            remember_notification_settings(data.notifications.clone());
            remember_priority_people(data.priority.iter().map(|(m, _)| m.clone()).collect());
        }
        self.ui.settings_server(data);
    }

    /// The network part of the status and work location commands.
    pub(super) async fn set_my_presence(self: &Arc<Self>, cmd: &SettingsCmd) -> anyhow::Result<()> {
        if self.offline.load(Ordering::Relaxed) {
            anyhow::bail!("Not available offline.");
        }
        let c = &self.client;
        match cmd {
            SettingsCmd::SetStatus(s, _) if s.is_empty() => c.reset_presence().await?,
            SettingsCmd::SetStatus(s, until) => {
                let a = teams_proto::presence::Availability::from_wire(s)
                    .ok_or_else(|| anyhow::anyhow!("unknown status {s}"))?;
                c.set_presence(a, until.as_deref()).await?;
            }
            SettingsCmd::SetStatusNote(text, until, show) => {
                c.set_status_note(text.trim(), until.as_deref(), *show)
                    .await?
            }
            SettingsCmd::SetWorkLocation(w) => {
                let pin = work_location_pin(w)
                    .ok_or_else(|| anyhow::anyhow!("unknown work location {w}"))?;
                ::calendar::set_work_location(c, pin, end_of_today_ms()).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The network part of "Refresh license and restart". Never runs offline (demo mode has no
    /// credentials, and the tests have no engine).
    pub(super) async fn refresh_license(self: &Arc<Self>) -> anyhow::Result<()> {
        if self.offline.load(Ordering::Relaxed) {
            anyhow::bail!("Not available offline.");
        }
        self.client.refresh_all().await?;
        let cfg = self.cfg.lock().unwrap().clone();
        session::persist(&cfg, &self.client).await?;
        Ok(())
    }

    /// Keeps the notification settings up to date for the desktop notifications: read at start
    /// and every ten minutes (a read of the properties bag, no write).
    pub(in super::super) fn start_notify_prefs(self: &Arc<Self>) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                match us::properties(&e.client).await {
                    Ok(p) => remember_notification_settings(Some(
                        NotificationSettings::from_properties(&p),
                    )),
                    Err(err) => tracing::debug!(%err, "notification settings not read"),
                }
                if let Ok(v) = us::contacts(&e.client, ContactList::Priority).await {
                    remember_priority_people(v);
                }
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            }
        });
    }

    pub(super) fn display_name(&self, mri: &str) -> String {
        self.store
            .lock()
            .unwrap()
            .user(mri)
            .ok()
            .flatten()
            .map(|u| u.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| mri.rsplit(':').next().unwrap_or(mri).to_string())
    }

    pub(super) async fn settings_read(self: &Arc<Self>) -> ServerData {
        let c = &self.client;
        let mut d = ServerData {
            offline: self.offline.load(Ordering::Relaxed),
            chat_service: c.chat_service().await,
            middle_tier: c.middle_tier().await,
            me_name: self.my_name.clone(),
            ..Default::default()
        };
        if d.offline {
            return d;
        }
        let me = vec![self.me.clone()];
        let (props, blocked, priority, tenants, profile, presence, installed) = tokio::join!(
            us::properties(c),
            us::contacts(c, ContactList::Blocked),
            us::contacts(c, ContactList::Priority),
            c.tenants(),
            c.profiles(&me),
            c.get_presence(&me),
            app_store::installed_apps(c),
        );
        let mut err = |what: &str, e: &dyn std::fmt::Display| {
            tracing::warn!(%e, "settings: could not read {what}");
            d.errors.push(format!("Could not read {what}."));
        };
        match props {
            Ok(p) => {
                d.notifications = Some(NotificationSettings::from_properties(&p));
                d.privacy = Some(us::parse_privacy(&p));
            }
            Err(e) => err("your Teams settings", &e),
        }
        let named = |v: Vec<String>| -> Vec<(String, String)> {
            v.into_iter()
                .map(|m| {
                    let n = self.display_name(&m);
                    (m, n)
                })
                .collect()
        };
        match blocked {
            Ok(v) => d.blocked = named(v),
            Err(e) => err("blocked contacts", &e),
        }
        match priority {
            Ok(v) => d.priority = named(v),
            Err(e) => err("priority access", &e),
        }
        match tenants {
            Ok(t) => d.tenants = Some(t),
            Err(e) => err("your organizations", &e),
        }
        match profile {
            Ok(p) => d.me_email = p.into_iter().next().and_then(|p| p.email),
            Err(e) => err("your profile", &e),
        }
        match presence {
            Ok(p) => {
                if let Some(p) = p.into_iter().next() {
                    d.me_status = p.note.map(|n| n.text).filter(|t| !t.trim().is_empty());
                    d.me_presence = Some(p.availability);
                    d.me_activity = Some(p.activity);
                    d.me_work_location = p.work_location;
                }
            }
            Err(e) => err("your status", &e),
        }
        match installed {
            Ok(apps) => {
                let mut asks: Vec<AppAsk> = apps
                    .definitions
                    .values()
                    .filter(|a| !a.device_permissions.is_empty())
                    .map(|a| AppAsk {
                        id: a.id.clone(),
                        name: a.name.clone(),
                        requested: a.device_permissions.clone(),
                    })
                    .collect();
                asks.sort_by(|a, b| a.name.cmp(&b.name));
                d.apps = asks;
            }
            Err(e) => err("your apps", &e),
        }
        if let Some(p) = d.privacy.as_mut() {
            p.blocked = d.blocked.iter().map(|(m, _)| m.clone()).collect();
        }
        d
    }
}
