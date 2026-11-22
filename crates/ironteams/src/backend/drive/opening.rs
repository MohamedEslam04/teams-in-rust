use super::*;

impl Engine {
    /// A "For you" card was clicked: open its file like a row of the list.
    pub(super) async fn drive_card_open(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let item = {
            let d = self.drive.lock().unwrap();
            d.s.cards
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.item.clone())
        };
        match item {
            Some(item) => self.drive_open_item(item).await,
            None => Ok(()),
        }
    }

    pub(super) async fn drive_open_row(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let item = self.drive.lock().unwrap().s.item(id).cloned();
        let Some(item) = item else {
            return Ok(());
        };
        let section = self.drive.lock().unwrap().s.section.clone();
        match section {
            // Deleted items cannot be opened; Restore is the way back.
            Section::Bin => return Ok(()),
            Section::People => return self.drive_go(Section::Person(item.name)).await,
            _ => {}
        }
        self.drive_open_item(item).await
    }

    /// A folder opens in the list, a file in IronTeams's viewer or the browser.
    pub(super) async fn drive_open_item(self: &Arc<Self>, item: DriveItem) -> anyhow::Result<()> {
        if item.is_folder {
            let g = {
                let mut d = self.drive.lock().unwrap();
                if d.s.section.is_browsable() {
                    d.s.open_folder(&item)
                } else {
                    // A folder from Home, Shared, Libraries ...: browse it as its own root.
                    d.s.open_foreign_folder(&item)
                }
            };
            self.drive_push();
            return self.drive_load(g).await;
        }
        let preview = with_files_pref(
            super::super::settings::files_prefs().open_office_in,
            Preview::of(&item),
            FileKind::of(&item),
        );
        match preview {
            Preview::Image | Preview::Text | Preview::Markdown
                if item.size <= MAX_PREVIEW_BYTES && !self.drive_offline() =>
            {
                self.drive_status(format!("Opening {}...", item.name));
                let bytes = ops::download(&self.client, &item).await?;
                let data = tokio::task::spawn_blocking(move || preview_of(&item, &bytes)).await?;
                self.drive_status("");
                self.ui.drive_preview(data);
            }
            Preview::Image | Preview::Text | Preview::Markdown if self.drive_offline() => {
                let text = crate::demo::drive_file_text(&item.name);
                self.ui.drive_preview(PreviewData {
                    title: item.name.clone(),
                    kind: if Preview::of(&item) == Preview::Markdown {
                        "markdown"
                    } else {
                        "text"
                    },
                    image: None,
                    text,
                    page: 0,
                    pages: 0,
                });
            }
            Preview::Pages if item.size <= MAX_OPEN_BYTES => {
                if !self.drive_open_pages(&item).await {
                    self.drive_open_browser_preview(&item).await?;
                }
            }
            Preview::Player => self.drive_open_external(item).await?,
            _ => self.drive_open_browser_preview(&item).await?,
        }
        Ok(())
    }

    /// Shows a PDF, or an Office file converted to PDF by Graph (a read-only request), page by
    /// page inside ironteams. False when that is not possible (password, a type Graph cannot convert,
    /// a damaged file): the caller falls back to the browser.
    pub(super) async fn drive_open_pages(self: &Arc<Self>, item: &DriveItem) -> bool {
        self.drive_status(format!("Opening {}...", item.name));
        let bytes: Vec<u8> = if self.drive_offline() {
            crate::demo::drive_pdf(&[(612, 792), (612, 792), (612, 792)])
        } else {
            let fetched = if FileKind::of(item) == FileKind::Pdf {
                ops::download(&self.client, item)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                preview::office_to_pdf(&self.client, &item.drive_id, &item.id, &item.name)
                    .await
                    .map_err(|e| e.to_string())
            };
            match fetched {
                Ok(b) => b,
                Err(err) => {
                    tracing::info!(%err, "no page preview, using the browser");
                    self.drive_status("");
                    return false;
                }
            }
        };
        let doc = match tokio::task::spawn_blocking(move || preview::Document::open(bytes)).await {
            Ok(Ok(doc)) if doc.page_count() > 0 => Arc::new(doc),
            _ => {
                self.drive_status("");
                return false;
            }
        };
        self.drive.lock().unwrap().pages = Some(OpenPages {
            doc,
            name: item.name.clone(),
            page: 0,
        });
        self.drive_status("");
        self.drive_show_page().await
    }

    /// Renders the current page of the open document and shows it.
    pub(super) async fn drive_show_page(self: &Arc<Self>) -> bool {
        let Some((doc, name, page)) = self
            .drive
            .lock()
            .unwrap()
            .pages
            .as_ref()
            .map(|p| (p.doc.clone(), p.name.clone(), p.page))
        else {
            return false;
        };
        let pages = doc.page_count();
        let image = tokio::task::spawn_blocking(move || {
            let r = doc.render_page(page, PAGE_WIDTH).ok()?;
            Some(Pixels::clone_from_slice(&r.pixels, r.width, r.height))
        })
        .await
        .ok()
        .flatten();
        let Some(image) = image else {
            self.drive_status("Couldn't draw this page");
            return false;
        };
        self.ui.drive_preview(PreviewData {
            title: name,
            kind: "pages",
            image: Some(image),
            text: String::new(),
            page: page + 1,
            pages,
        });
        true
    }

    /// Previous or next page of the open document.
    pub(super) async fn drive_preview_page(self: &Arc<Self>, which: &str) -> anyhow::Result<()> {
        {
            let mut d = self.drive.lock().unwrap();
            let Some(p) = d.pages.as_mut() else {
                return Ok(());
            };
            let last = p.doc.page_count().saturating_sub(1);
            p.page = match which {
                "next" => (p.page + 1).min(last),
                "prev" => p.page.saturating_sub(1),
                _ => p.page,
            };
        }
        self.drive_show_page().await;
        Ok(())
    }

    /// Office files and PDFs: the preview URL in the browser, else the web address.
    pub(super) async fn drive_open_browser_preview(&self, item: &DriveItem) -> anyhow::Result<()> {
        if self.drive_offline() {
            self.drive_open_web(item);
            return Ok(());
        }
        match ops::preview_url(&self.client, &ItemRef::from(item)).await {
            Ok(url) if url.starts_with("https://") => {
                let _ = crate::external::open(&url);
                Ok(())
            }
            _ => {
                self.drive_open_web(item);
                Ok(())
            }
        }
    }

    pub(super) fn drive_open_web(&self, item: &DriveItem) {
        if item.web_url.starts_with("https://") {
            drive_open_web_url(&item.web_url);
        } else {
            self.drive_status("This item has no web address");
        }
    }

    /// Downloads to the cache and hands the file to the desktop (`xdg-open`).
    pub(super) async fn drive_open_external(
        self: &Arc<Self>,
        item: DriveItem,
    ) -> anyhow::Result<()> {
        if item.is_folder || self.drive_offline() {
            self.drive_open_web(&item);
            return Ok(());
        }
        if item.size > MAX_OPEN_BYTES {
            self.drive_open_web(&item);
            return Ok(());
        }
        self.drive_status(format!("Downloading {}...", item.name));
        let bytes = ops::download(&self.client, &item).await?;
        let dir = session::Paths::new()?.cache_dir.join("drive-open");
        let name = sanitize_file_name(&item.name);
        let path = tokio::task::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(name);
            std::fs::write(&path, bytes)?;
            Ok(path)
        })
        .await??;
        self.drive_status("");
        let _ = crate::external::open(path);
        Ok(())
    }

    /// Goes to the folder that holds the item. Own files are browsed natively; items of other
    /// drives open the folder in the browser.
    pub(super) async fn drive_open_location(
        self: &Arc<Self>,
        item: DriveItem,
    ) -> anyhow::Result<()> {
        if self.drive_offline() {
            return self.drive_go(Section::MyFiles).await;
        }
        let mine = self.drive_my_drive_id().await;
        if item.parent_path.is_empty() || mine.as_deref() != Some(item.drive_id.as_str()) {
            if let Some((parent, _)) = item.web_url.rsplit_once('/') {
                let _ = crate::external::open(parent);
            }
            return Ok(());
        }
        let g = self.drive.lock().unwrap().s.go(Section::MyFiles);
        for path in path_prefixes(&item.parent_path) {
            let folder = browse::item_at(&self.client, &path).await?;
            self.drive.lock().unwrap().s.trail.push(Crumb {
                name: folder.name,
                drive_id: folder.drive_id,
                item_id: folder.id,
            });
        }
        self.drive_push();
        self.drive_load(g).await
    }

    pub(super) async fn drive_my_drive_id(&self) -> Option<String> {
        if let Some(id) = self.drive.lock().unwrap().my_drive_id.clone() {
            return Some(id);
        }
        let id = browse::my_drive(&self.client).await.ok()?.id;
        self.drive.lock().unwrap().my_drive_id = Some(id.clone());
        Some(id)
    }

    /// Where new files and folders go: the open folder, else the root of My files.
    pub(super) fn drive_target(&self) -> ItemRef {
        let d = self.drive.lock().unwrap();
        match d.s.folder() {
            Some(c) => ItemRef::new(&c.drive_id, &c.item_id),
            None => ItemRef::my_root(),
        }
    }

    /// Reloads the open list after a change and selects `select` when it is there.
    pub(super) async fn drive_refresh_select(
        self: &Arc<Self>,
        select: Option<String>,
    ) -> anyhow::Result<()> {
        let g = {
            let mut d = self.drive.lock().unwrap();
            d.s.generation += 1;
            d.s.generation
        };
        self.drive_load(g).await?;
        if let Some(id) = select {
            self.drive_edit(|s| s.select_only(&id));
        }
        Ok(())
    }

    pub(super) fn demo_note(&self) -> bool {
        if self.drive_offline() {
            self.drive_status("Demo mode: nothing is saved");
            return true;
        }
        false
    }
}
