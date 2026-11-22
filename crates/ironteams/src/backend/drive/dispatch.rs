use super::*;

// -- the engine -----------------------------------------------------------------------------

impl Engine {
    pub(super) fn drive_offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    pub(super) fn drive_push(&self) {
        let data = {
            let d = self.drive.lock().unwrap();
            d.s.data(now_ms(), &local_offset)
        };
        self.ui.drive(data);
    }

    pub(super) fn drive_status(&self, text: impl Into<String>) {
        self.drive.lock().unwrap().s.status = text.into();
        self.drive_push();
    }

    pub(super) fn drive_selection(&self) -> Vec<DriveItem> {
        let d = self.drive.lock().unwrap();
        d.s.selected_items().into_iter().cloned().collect()
    }

    /// The one selected item, or a status line saying what is needed.
    pub(super) fn drive_one(&self) -> Option<DriveItem> {
        let sel = self.drive_selection();
        if let [one] = sel.as_slice() {
            return Some(one.clone());
        }
        self.drive_status("Select one item first");
        None
    }

    pub(in super::super) async fn drive_cmd(self: &Arc<Self>, cmd: DriveCmd) -> anyhow::Result<()> {
        use DriveCmd::*;
        match cmd {
            Enter => {
                let first = !std::mem::replace(&mut self.drive.lock().unwrap().entered, true);
                if first {
                    self.drive_start_background();
                    return self.drive_go(Section::Home).await;
                }
                self.drive_push();
            }
            Nav(key) => {
                if let Some(section) = Section::from_key(&key) {
                    self.drive_go(section).await?;
                }
            }
            OpenRow(id) => self.drive_open_row(&id).await?,
            // The first crumb of a person's files is the People list.
            Crumb(0) if matches!(self.drive.lock().unwrap().s.section, Section::Person(_)) => {
                self.drive_go(Section::People).await?;
            }
            Crumb(i) => {
                let g = self.drive.lock().unwrap().s.go_to_crumb(i);
                if let Some(g) = g {
                    self.drive_push();
                    self.drive_load(g).await?;
                }
            }
            Refresh => {
                let g = {
                    let mut d = self.drive.lock().unwrap();
                    d.s.loading = true;
                    d.s.generation += 1;
                    d.s.generation
                };
                self.drive_push();
                if self.drive.lock().unwrap().s.section == Section::Home {
                    let e = self.clone();
                    tokio::spawn(async move { e.drive_load_cards().await });
                }
                self.drive_load(g).await?;
            }
            LoadMore => self.drive_load_more().await?,
            Select(_) | Toggle(_) | SelectAll | ClearSelection | View(_) | Sort(_) | Group(_)
            | Column(_) | Type(_) | Filter(_) => self.drive_instant(cmd),
            Search(q) => {
                let q = q.trim().to_owned();
                let section = if q.is_empty() {
                    Section::Home
                } else {
                    Section::Search(q)
                };
                self.drive_go(section).await?;
            }
            NewFolder(name) => self.drive_new_folder(name.trim()).await?,
            NewDoc(kind) => self.drive_new_doc(kind).await?,
            NewLink(arg) => self.drive_new_link(&arg).await?,
            NewNotebook(name) => self.drive_new_notebook(name.trim()).await?,
            NewForms => self.drive_new_forms(),
            CardOpen(id) => self.drive_card_open(&id).await?,
            UploadFiles => self.drive_upload_pick(false).await?,
            UploadFolder => self.drive_upload_pick(true).await?,
            Drop(path) => self.drive_upload_paths(vec![path]).await?,
            OpenIronTeams => {
                if let Some(i) = self.drive_one() {
                    self.drive_open_item(i).await?;
                }
            }
            OpenBrowser => {
                if let Some(i) = self.drive_one() {
                    self.drive_open_web(&i);
                }
            }
            OpenDesktop => {
                if let Some(i) = self.drive_one() {
                    self.drive_open_external(i).await?;
                }
            }
            OpenLocation => {
                if let Some(i) = self.drive_one() {
                    self.drive_open_location(i).await?;
                }
            }
            Favorite(id) => self.drive_favorite(id).await?,
            Shortcut => self.drive_shortcut().await?,
            Rename(name) => self.drive_rename(name.trim()).await?,
            MoveTo => self.drive_pick_begin("move".into()).await?,
            CopyTo => self.drive_pick_begin("copy".into()).await?,
            Download => self.drive_download().await?,
            Delete => self.drive_delete().await?,
            Restore => self.drive_bin(true).await?,
            DeleteForever => self.drive_bin(false).await?,
            CopyLink => self.drive_copy_link(),
            Versions => self.drive_versions().await?,
            VersionRestore(id) => self.drive_version_restore(&id).await?,
            Details => {
                if let Some(i) = self.drive_one() {
                    let fav = self.drive.lock().unwrap().s.favorites.contains(&i.id);
                    self.ui.drive_info("Details".into(), details_text(&i, fav));
                }
            }
            PreviewPage(which) => self.drive_preview_page(&which).await?,
            Sync => self.ui.drive_info("Sync".into(), onedrive::sync_help()),
            Share => self.drive_share_begin().await?,
            ShareSearch(q) => self.drive_share_search(q.trim()).await?,
            SharePick(id) => self.drive_share_pick(&id),
            ShareRole(r) => {
                if let Some(s) = self.drive.lock().unwrap().share.as_mut() {
                    s.role = parse_role(&r);
                }
                self.drive_share_push();
            }
            ShareSend(msg) => self.drive_share_send(&msg).await?,
            ShareLink(arg) => self.drive_share_link(&arg).await?,
            SharePermRole(id, role) => self.drive_perm_role(&id, parse_role(&role)).await?,
            SharePermRemove(id) => self.drive_perm_remove(&id).await?,
            PickBegin(purpose) => self.drive_pick_begin(purpose).await?,
            PickOpen(id) => self.drive_pick_open(&id).await?,
            PickCrumb(i) => self.drive_pick_crumb(i).await?,
            PickSelect(id) => self.drive_pick_select(&id),
            PickChoose => self.drive_pick_choose().await?,
            PickCancel | DialogClose => {
                let mut d = self.drive.lock().unwrap();
                d.picker = None;
                d.share = None;
                d.versions_of = None;
                d.pages = None;
            }
        }
        Ok(())
    }

    /// The list's filter text (tests of the command order).
    #[cfg(test)]
    pub(in super::super) fn drive_filter_for_test(&self) -> String {
        self.drive.lock().unwrap().s.filter.clone()
    }

    /// The commands that only change the view state (see `DriveCmd::is_instant`).
    pub(in super::super) fn drive_instant(self: &Arc<Self>, cmd: DriveCmd) {
        match cmd {
            DriveCmd::Select(id) => self.drive_edit(|s| s.select_only(&id)),
            DriveCmd::Toggle(id) => self.drive_edit(|s| s.toggle(&id)),
            DriveCmd::SelectAll => self.drive_edit(State::select_all),
            DriveCmd::ClearSelection => self.drive_edit(State::clear_selection),
            DriveCmd::View(v) => {
                let grid = v == "grid";
                self.drive_edit(|s| s.grid = grid);
                if grid {
                    self.drive_thumbs();
                }
            }
            DriveCmd::Sort(k) => {
                if let Some(k) = state::parse_sort_key(&k) {
                    self.drive_edit(|s| s.set_sort(k));
                }
            }
            DriveCmd::Group(k) => {
                let key = state::parse_sort_key(&k);
                self.drive_edit(|s| s.group = key);
            }
            DriveCmd::Column(c) => self.drive_edit(|s| s.columns.toggle(&c)),
            DriveCmd::Type(t) => {
                self.drive_edit(|s| s.type_filter = state::TypeFilter::parse(&t));
            }
            DriveCmd::Filter(q) => self.drive_edit(|s| s.filter = q),
            _ => {}
        }
    }

    /// Changes the view state and pushes it; for the commands that need no network.
    pub(super) fn drive_edit(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.drive.lock().unwrap().s);
        self.drive_push();
    }

    /// Starts the background load (libraries, favorites, cards) unless it already started.
    pub(super) fn drive_start_background(self: &Arc<Self>) {
        if !std::mem::replace(&mut self.drive.lock().unwrap().background_started, true) {
            let e = self.clone();
            tokio::spawn(async move { e.drive_background_load().await });
        }
    }
}
