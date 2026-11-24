use super::*;

impl Engine {
    // -- the reusable file picker -------------------------------------------------------------

    /// Opens the picker for `purpose`. `move` and `copy` act on the selection; other purposes
    /// (for example `attach`) are answered through [`Engine::drive_picked`].
    pub(super) async fn drive_pick_begin(self: &Arc<Self>, purpose: String) -> anyhow::Result<()> {
        let folders_only = matches!(purpose.as_str(), "move" | "copy");
        if folders_only && self.drive_selection().is_empty() {
            self.drive_status("Select something first");
            return Ok(());
        }
        // The chat can open the picker before the OneDrive view ever did.
        self.drive_start_background();
        {
            let mut d = self.drive.lock().unwrap();
            let libs = d.s.libraries.clone();
            let mut items = vec![my_files_item()];
            if purpose == "attach" {
                items.extend(virtual_items());
            }
            items.extend(libs.iter().map(library_item));
            d.picker = Some(Picker {
                purpose,
                folders_only,
                trail: Vec::new(),
                items,
                selected: None,
                loading: false,
                generation: 0,
            });
        }
        self.drive_picker_push();
        Ok(())
    }

    pub(super) fn drive_picker_push(&self) {
        let data = {
            let d = self.drive.lock().unwrap();
            d.picker.as_ref().map(|p| picker_data(p, &d.s.libraries))
        };
        if let Some(data) = data {
            self.ui.drive_picker(data);
        }
    }

    pub(super) async fn drive_pick_open(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let (folder, generation, folders_only) = {
            let mut d = self.drive.lock().unwrap();
            let Some(p) = d.picker.as_mut() else {
                return Ok(());
            };
            let Some(item) = p
                .items
                .iter()
                .find(|i| i.id == id && !i.id.is_empty())
                .cloned()
            else {
                return Ok(());
            };
            if !item.is_folder {
                p.selected = Some(item.id);
                drop(d);
                self.drive_picker_push();
                return Ok(());
            }
            p.trail.push(Crumb {
                name: item.name.clone(),
                drive_id: item.drive_id.clone(),
                item_id: item.id.clone(),
            });
            p.items.clear();
            p.selected = None;
            p.loading = true;
            p.generation += 1;
            (
                p.trail.last().cloned().unwrap(),
                p.generation,
                p.folders_only,
            )
        };
        self.drive_picker_push();
        self.drive_pick_list(folder, generation, folders_only).await
    }

    pub(super) async fn drive_pick_crumb(self: &Arc<Self>, i: usize) -> anyhow::Result<()> {
        // Crumb 0 is the OneDrive root; crumb n is trail[n - 1].
        let (folder, generation, folders_only) = {
            let mut d = self.drive.lock().unwrap();
            let libs = d.s.libraries.clone();
            let Some(p) = d.picker.as_mut() else {
                return Ok(());
            };
            p.trail.truncate(i);
            p.selected = None;
            p.generation += 1;
            match p.trail.last().cloned() {
                None => {
                    let mut items = vec![my_files_item()];
                    items.extend(libs.iter().map(library_item));
                    p.items = items;
                    p.loading = false;
                    drop(d);
                    self.drive_picker_push();
                    return Ok(());
                }
                Some(f) => {
                    p.loading = true;
                    (f, p.generation, p.folders_only)
                }
            }
        };
        self.drive_picker_push();
        self.drive_pick_list(folder, generation, folders_only).await
    }

    pub(super) async fn drive_pick_list(
        self: &Arc<Self>,
        folder: Crumb,
        generation: u64,
        folders_only: bool,
    ) -> anyhow::Result<()> {
        let items = if folder.drive_id == VIRTUAL {
            let list = match folder.item_id.as_str() {
                "recent" => browse::recent(&self.client).await,
                _ => browse::shared_with_me(&self.client).await,
            };
            match list {
                Ok(items) => items,
                Err(err) => {
                    self.drive_status(format!("Couldn't list {}: {err}", folder.name));
                    Vec::new()
                }
            }
        } else if self.drive_offline() {
            crate::demo::drive_listing("files", &folder.item_id, now_ms())
        } else {
            match browse::children(&self.client, &folder.drive_id, &folder.item_id, None).await {
                Ok(p) => p.items,
                Err(err) => {
                    self.drive_status(format!("Couldn't list that folder: {err}"));
                    Vec::new()
                }
            }
        };
        {
            let mut d = self.drive.lock().unwrap();
            let Some(p) = d.picker.as_mut().filter(|p| p.generation == generation) else {
                return Ok(());
            };
            p.items = items
                .into_iter()
                .filter(|i| i.is_folder || !folders_only)
                .collect();
            p.loading = false;
        }
        self.drive_picker_push();
        Ok(())
    }

    pub(super) fn drive_pick_select(&self, id: &str) {
        if let Some(p) = self.drive.lock().unwrap().picker.as_mut()
            && p.items.iter().any(|i| i.id == id && !i.is_folder)
        {
            p.selected = Some(id.to_owned());
        }
        self.drive_picker_push();
    }

    pub(super) async fn drive_pick_choose(self: &Arc<Self>) -> anyhow::Result<()> {
        let (purpose, dest, file) = {
            let mut d = self.drive.lock().unwrap();
            let Some(p) = d.picker.take() else {
                return Ok(());
            };
            let file = p
                .selected
                .as_ref()
                .and_then(|id| p.items.iter().find(|i| &i.id == id).cloned());
            (p.purpose, p.trail.last().cloned(), file)
        };
        self.ui.drive_dialog("");
        self.drive_picked(&purpose, dest, file).await
    }

    /// What happens when the user confirmed the picker. Features that attach cloud files add
    /// their purpose here (`file` is the chosen file, `dest` the folder it was in).
    pub(in super::super) async fn drive_picked(
        self: &Arc<Self>,
        purpose: &str,
        dest: Option<Crumb>,
        file: Option<DriveItem>,
    ) -> anyhow::Result<()> {
        match (purpose, dest) {
            ("move", Some(dest)) => self.drive_move_copy(true, dest).await,
            ("copy", Some(dest)) => self.drive_move_copy(false, dest).await,
            ("attach", _) => {
                let Some(f) = file else {
                    return Ok(());
                };
                let drive_id = if f.drive_id.is_empty() {
                    self.drive_my_drive_id().await.unwrap_or_default()
                } else {
                    f.drive_id.clone()
                };
                self.attach_cloud_file(&drive_id, &f.id, &f.name).await
            }
            (other, _) => {
                tracing::debug!(purpose = other, picked = ?file.map(|f| f.name), "file picked with no handler");
                Ok(())
            }
        }
    }

    pub(super) async fn drive_move_copy(
        self: &Arc<Self>,
        moving: bool,
        dest: Crumb,
    ) -> anyhow::Result<()> {
        let sel = self.drive_selection();
        if sel.is_empty() {
            return Ok(());
        }
        if self.demo_note() {
            return Ok(());
        }
        let mine = self.drive_my_drive_id().await.unwrap_or_default();
        let norm = |d: &str| {
            if d.is_empty() {
                mine.clone()
            } else {
                d.to_owned()
            }
        };
        let target = ItemRef::new(&dest.drive_id, &dest.item_id);
        let (mut done, mut failed) = (0usize, Vec::new());
        for i in &sel {
            let r = if moving {
                if norm(&i.drive_id) != norm(&dest.drive_id) {
                    failed.push(format!(
                        "{}: moving between libraries is not supported, use Copy to",
                        i.name
                    ));
                    continue;
                }
                ops::move_to(&self.client, &ItemRef::own(i), &target, None).await
            } else {
                self.drive_status(format!("Copying {}...", i.name));
                ops::copy_to(&self.client, &ItemRef::from(i), &target, None).await
            };
            match r {
                Ok(_) => done += 1,
                Err(err) => failed.push(format!("{}: {err}", i.name)),
            }
        }
        if moving && done > 0 {
            // Moved items are no longer in this folder.
            self.drive_refresh_select(None).await?;
        }
        let verb = if moving { "Moved" } else { "Copied" };
        let mut status = format!("{verb} {done} item{} to {}", plural(done), dest.name);
        if !failed.is_empty() {
            status.push_str(&format!(". Failed: {}", failed.join("; ")));
        }
        self.drive_status(status);
        Ok(())
    }
}
