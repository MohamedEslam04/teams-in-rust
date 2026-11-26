use super::*;

impl Engine {
    pub(super) async fn drive_new_folder(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        if self.drive_offline() {
            let item = crate::demo::drive_new_folder(name);
            let id = item.id.clone();
            self.drive.lock().unwrap().s.items.push(item);
            self.drive_edit(|s| s.select_only(&id));
            return Ok(());
        }
        let parent = self.drive_target();
        let made = ops::create_folder(&self.client, &parent, name).await?;
        self.drive_refresh_select(Some(made.id)).await
    }

    pub(super) async fn drive_new_doc(
        self: &Arc<Self>,
        kind: onedrive::DocKind,
    ) -> anyhow::Result<()> {
        if self.demo_note() {
            return Ok(());
        }
        let parent = self.drive_target();
        self.drive_status(format!("Creating {}...", kind.default_name()));
        let made = ops::create_office_doc(&self.client, &parent, kind, None).await?;
        self.drive_refresh_select(Some(made.id)).await?;
        self.drive_status("");
        Ok(())
    }

    /// New > Link: a `.url` file that opens `address`. The argument is `name, a newline, then the address`.
    pub(super) async fn drive_new_link(self: &Arc<Self>, arg: &str) -> anyhow::Result<()> {
        let (name, url) = arg.split_once('\n').unwrap_or((arg, ""));
        let (name, url) = (name.trim(), url.trim());
        if name.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            self.drive_status("A link needs a name and a web address that starts with https://");
            return Ok(());
        }
        if self.demo_note() {
            return Ok(());
        }
        let parent = self.drive_target();
        let what = NewItem::Link {
            name: name.to_owned(),
            url: url.to_owned(),
        };
        match onedrive::create::create(&self.client, &parent, &what).await? {
            onedrive::Created::Item(made) => self.drive_refresh_select(Some(made.id)).await,
            onedrive::Created::Notebook(_) => Ok(()),
        }
    }

    /// New > OneNote notebook: OneNote keeps notebooks in its own place, not in the open folder,
    /// so the notebook opens in the browser once it exists.
    pub(super) async fn drive_new_notebook(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        if name.is_empty() || self.demo_note() {
            return Ok(());
        }
        let what = NewItem::Notebook {
            name: name.to_owned(),
        };
        self.drive_status(format!("Creating notebook {name}..."));
        let made = onedrive::create::create(&self.client, &self.drive_target(), &what).await?;
        if let onedrive::Created::Notebook(nb) = made {
            if nb.web_url.starts_with("https://") {
                let _ = crate::external::open(&nb.web_url);
            }
            self.drive_status(format!("Created notebook {}", nb.name));
        }
        Ok(())
    }

    /// New > Forms survey: Forms has no API for this client, so it opens in the browser.
    pub(super) fn drive_new_forms(&self) {
        if self.drive_offline() {
            self.drive_status("Demo mode: Microsoft Forms would open in the browser");
            return;
        }
        let _ = crate::external::open(onedrive::FORMS_NEW_URL);
    }

    pub(super) async fn drive_upload_pick(self: &Arc<Self>, folder: bool) -> anyhow::Result<()> {
        // Demo mode never shows a file dialog (tests and screenshots have nobody to answer it).
        if self.demo_note() {
            return Ok(());
        }
        let paths: Vec<std::path::PathBuf> = if folder {
            rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        } else {
            rfd::AsyncFileDialog::new()
                .pick_files()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        };
        self.drive_upload_paths(paths).await
    }

    /// Uploads files and folders (the picker, or dropped on the window).
    pub(super) async fn drive_upload_paths(
        self: &Arc<Self>,
        paths: Vec<std::path::PathBuf>,
    ) -> anyhow::Result<()> {
        if paths.is_empty() || self.demo_note() {
            return Ok(());
        }
        // An account switch asks before cutting it short.
        let _upload = crate::backend::inflight::UploadGuard::start();
        let parent = self.drive_target();
        let (mut files, mut failed) = (0usize, Vec::new());
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.drive_status(format!("Uploading {name}..."));
            let r: anyhow::Result<usize> = async {
                if path.is_dir() {
                    let report = ops::upload_folder(&self.client, &parent, &path).await?;
                    Ok(report.files)
                } else {
                    let p = path.clone();
                    let data = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
                        let meta = std::fs::metadata(&p)?;
                        anyhow::ensure!(
                            meta.len() <= MAX_UPLOAD_BYTES,
                            "{} is too big to upload here (250 MB limit)",
                            p.display()
                        );
                        Ok(std::fs::read(&p)?)
                    })
                    .await??;
                    ops::upload_file(&self.client, &parent, &name, data).await?;
                    Ok(1)
                }
            }
            .await;
            match r {
                Ok(n) => files += n,
                Err(err) => failed.push(format!("{name}: {err}")),
            }
        }
        self.drive_refresh_select(None).await?;
        let mut status = format!("Uploaded {files} file{}", if files == 1 { "" } else { "s" });
        if !failed.is_empty() {
            status.push_str(&format!(". Failed: {}", failed.join("; ")));
        }
        self.drive_status(status);
        Ok(())
    }

    pub(super) async fn drive_favorite(self: &Arc<Self>, id: String) -> anyhow::Result<()> {
        let items: Vec<DriveItem> = if id.is_empty() {
            self.drive_selection()
        } else {
            self.drive
                .lock()
                .unwrap()
                .s
                .item(&id)
                .cloned()
                .into_iter()
                .collect()
        };
        if items.is_empty() {
            return Ok(());
        }
        let make_favorite = {
            let d = self.drive.lock().unwrap();
            !items.iter().all(|i| d.s.favorites.contains(&i.id))
        };
        for i in &items {
            {
                let mut d = self.drive.lock().unwrap();
                if make_favorite {
                    d.s.favorites.insert(i.id.clone());
                } else {
                    d.s.favorites.remove(&i.id);
                }
            }
            if self.drive_offline() {
                continue;
            }
            let r = ItemRef::from(i);
            let done = if make_favorite {
                ops::follow(&self.client, &r).await
            } else {
                ops::unfollow(&self.client, &r).await
            };
            if let Err(err) = done {
                // Undo the optimistic star.
                let mut d = self.drive.lock().unwrap();
                if make_favorite {
                    d.s.favorites.remove(&i.id);
                } else {
                    d.s.favorites.insert(i.id.clone());
                }
                drop(d);
                self.drive_status(format!("Couldn't change the favorite: {err}"));
                return Ok(());
            }
        }
        {
            // Un-favoriting inside Favorites removes the row.
            let mut d = self.drive.lock().unwrap();
            if !make_favorite && d.s.section == Section::Favorites {
                let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                d.s.items.retain(|i| !ids.contains(&i.id));
                d.s.selected.retain(|s| !ids.contains(s));
            }
        }
        self.drive_push();
        Ok(())
    }

    /// Add shortcut: Graph refuses every body we tried (docs/protocol.md), so the failure is
    /// expected and shown as a plain sentence.
    pub(super) async fn drive_shortcut(self: &Arc<Self>) -> anyhow::Result<()> {
        let sel = self.drive_selection();
        let Some(target) = sel.first() else {
            self.drive_status("Select a folder to add as a shortcut");
            return Ok(());
        };
        if self.drive_offline() {
            self.drive_status("Demo mode: nothing is saved");
            return Ok(());
        }
        match ops::add_shortcut(&self.client, target, &ItemRef::my_root(), None).await {
            Ok(_) => {
                self.drive_status(format!("Added a shortcut to {} in My files", target.name));
            }
            Err(err) => {
                tracing::warn!(%err, "add shortcut failed");
                self.drive_status(
                    "OneDrive did not accept the shortcut, so none was added. \
                     Open the item in the browser to add it there.",
                );
            }
        }
        Ok(())
    }

    pub(super) async fn drive_rename(self: &Arc<Self>, name: &str) -> anyhow::Result<()> {
        let Some(item) = self.drive_one() else {
            return Ok(());
        };
        if name.is_empty() || name == item.name {
            return Ok(());
        }
        if self.drive_offline() {
            self.drive_edit(|s| {
                if let Some(i) = s.items.iter_mut().find(|i| i.id == item.id) {
                    i.name = name.to_owned();
                }
            });
            return Ok(());
        }
        let renamed = ops::rename(&self.client, &ItemRef::own(&item), name).await?;
        self.drive_edit(|s| {
            if let Some(i) = s.items.iter_mut().find(|i| i.id == item.id) {
                i.name = renamed.name.clone();
            }
        });
        Ok(())
    }

    pub(super) async fn drive_delete(self: &Arc<Self>) -> anyhow::Result<()> {
        let sel = self.drive_selection();
        if sel.is_empty() {
            return Ok(());
        }
        let mut gone = Vec::new();
        for i in &sel {
            if self.drive_offline() || ops::delete(&self.client, &ItemRef::own(i)).await.is_ok() {
                gone.push(i.id.clone());
            }
        }
        let failed = sel.len() - gone.len();
        {
            let mut d = self.drive.lock().unwrap();
            d.s.items.retain(|i| !gone.contains(&i.id));
            d.s.selected.retain(|s| !gone.contains(s));
            d.s.status = match (gone.len(), failed) {
                (n, 0) => format!("Deleted {n} item{} (in the recycle bin)", plural(n)),
                (n, f) => format!("Deleted {n}, couldn't delete {f}"),
            };
        }
        self.drive_push();
        Ok(())
    }

    /// Recycle bin: Restore (`true`) or Delete forever (`false`) for the selection.
    pub(super) async fn drive_bin(self: &Arc<Self>, restore: bool) -> anyhow::Result<()> {
        let sel = self.drive_selection();
        if sel.is_empty() {
            return Ok(());
        }
        let mut gone = Vec::new();
        for i in &sel {
            let ok = self.drive_offline()
                || if restore {
                    bin::restore(&self.client, &i.id).await.is_ok()
                } else {
                    bin::delete_forever(&self.client, &i.id).await.is_ok()
                };
            if ok {
                gone.push(i.id.clone());
            }
        }
        {
            let mut d = self.drive.lock().unwrap();
            d.s.items.retain(|i| !gone.contains(&i.id));
            d.s.selected.retain(|s| !gone.contains(s));
            let n = gone.len();
            d.s.status = if n == sel.len() {
                format!(
                    "{} {n} item{}",
                    if restore {
                        "Restored"
                    } else {
                        "Deleted forever"
                    },
                    plural(n)
                )
            } else {
                format!("Only {n} of {} items could be changed", sel.len())
            };
        }
        self.drive_push();
        Ok(())
    }

    pub(super) fn drive_copy_link(&self) {
        let sel = self.drive_selection();
        let Some(i) = sel.first().filter(|i| !i.web_url.is_empty()) else {
            self.drive_status("Select an item with a web address first");
            return;
        };
        // The item's own address works for everyone who already has access, which is what
        // Teams' "Copy link" gives. A new sharing link is a decision for the Share dialog.
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(i.web_url.clone())) {
            Ok(()) => self.drive_status("Link copied"),
            Err(err) => self.drive_status(format!("Couldn't copy the link: {err}")),
        }
    }

    pub(super) async fn drive_download(self: &Arc<Self>) -> anyhow::Result<()> {
        let sel = self.drive_selection();
        if sel.is_empty() {
            return Ok(());
        }
        if self.demo_note() {
            return Ok(());
        }
        let file_name = |i: &DriveItem| {
            if i.is_folder {
                format!("{}.zip", i.name)
            } else {
                i.name.clone()
            }
        };
        let targets: Vec<(DriveItem, std::path::PathBuf)> = if let [one] = sel.as_slice() {
            let Some(f) = rfd::AsyncFileDialog::new()
                .set_file_name(sanitize_file_name(&file_name(one)))
                .save_file()
                .await
            else {
                return Ok(());
            };
            vec![(one.clone(), f.path().to_path_buf())]
        } else {
            let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return Ok(());
            };
            sel.iter()
                .map(|i| {
                    (
                        i.clone(),
                        dir.path().join(sanitize_file_name(&file_name(i))),
                    )
                })
                .collect()
        };
        let mut done = 0;
        for (item, path) in targets {
            self.drive_status(format!("Downloading {}...", item.name));
            let bytes = ops::download(&self.client, &item).await?;
            tokio::task::spawn_blocking(move || std::fs::write(path, bytes)).await??;
            done += 1;
        }
        self.drive_status(format!("Downloaded {done} item{}", plural(done)));
        Ok(())
    }
}
