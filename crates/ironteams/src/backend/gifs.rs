//! The composer's GIF picker: search (trending when empty) through Teams' Giphy extension,
//! small previews for the grid, and the picked GIF sent to the open chat, thread or channel.
//! Its Stickers tab lists Teams' stickers (`stickers.rs`) the same way.

use super::Engine;
use super::stickers::{self, Sticker};
use super::util::{decode_image, decode_limited};
use crate::data::Pixels;
use std::sync::{Arc, Mutex};
use store::OutboxKind;
use teams_proto::gifs::{Gif, gif_html};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GifCmd {
    /// Searches; an empty query shows the trending GIFs.
    Search(String),
    /// Lists the stickers matching a query (popular ones when empty).
    Stickers(String),
    /// Sends the result at this index.
    Pick(usize),
}

impl GifCmd {
    pub fn parse(name: &str, arg: &str) -> Option<GifCmd> {
        Some(match name {
            "search" => GifCmd::Search(arg.to_string()),
            "stickers" => GifCmd::Stickers(arg.to_string()),
            "pick" => GifCmd::Pick(arg.parse().ok()?),
            _ => return None,
        })
    }
}

/// What the picker shows: one row per result.
#[derive(Clone, Default)]
pub struct GifRow {
    pub title: String,
    pub preview: Option<Pixels>,
}

/// Most stickers the tab shows at once.
const STICKER_LIMIT: usize = 48;

#[derive(Default)]
struct State {
    /// Bumped by every search; a slower earlier answer is dropped.
    seq: u64,
    results: Vec<Gif>,
    /// The Stickers tab's results; when set, the grid shows these instead of `results`.
    stickers: Option<Vec<&'static Sticker>>,
    previews: Vec<Option<Pixels>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(State::default))
}

fn rows() -> Vec<GifRow> {
    with(|s| {
        let titles: Vec<String> = match &s.stickers {
            Some(found) => found.iter().map(|st| st.name.to_string()).collect(),
            None => s.results.iter().map(|g| g.title.clone()).collect(),
        };
        titles
            .into_iter()
            .zip(s.previews.iter())
            .map(|(title, p)| GifRow {
                title,
                preview: p.clone(),
            })
            .collect()
    })
}

/// Where a picked GIF goes: the open thread, else the open chat or channel.
pub fn target(conv: &str, thread_root: Option<&str>) -> String {
    match thread_root {
        Some(root) => format!("{conv};messageid={root}"),
        None => conv.to_string(),
    }
}

impl Engine {
    pub(super) async fn gif_cmd(self: &Arc<Self>, cmd: GifCmd) -> anyhow::Result<()> {
        match cmd {
            GifCmd::Search(q) => self.gif_search(q).await,
            GifCmd::Stickers(q) => {
                self.sticker_search(&q);
                Ok(())
            }
            GifCmd::Pick(i) => self.gif_pick(i).await,
        }
    }

    async fn gif_search(self: &Arc<Self>, q: String) -> anyhow::Result<()> {
        let seq = with(|s| {
            s.seq += 1;
            if s.stickers.take().is_some() {
                s.results.clear();
                s.previews.clear();
            }
            s.seq
        });
        if self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.ui.gifs(vec![], "GIFs need a connection.".into());
            return Ok(());
        }
        self.ui.gifs(rows(), "Searching…".into());
        let found = match self.client.search_gifs(q.trim(), "en-us").await {
            Ok(f) => f,
            Err(e) => {
                self.ui.gifs(vec![], format!("Could not search GIFs: {e}"));
                return Ok(());
            }
        };
        let urls: Vec<String> = found.iter().map(|g| g.preview_url.clone()).collect();
        let fresh = with(|s| {
            if s.seq != seq {
                return false;
            }
            s.previews = vec![None; found.len()];
            s.results = found;
            s.stickers = None;
            true
        });
        if !fresh {
            return Ok(());
        }
        let status = if urls.is_empty() {
            "No GIFs found."
        } else {
            ""
        };
        self.ui.gifs(rows(), status.into());
        self.load_previews(seq, urls);
        Ok(())
    }

    /// The Stickers tab: the catalogue matches, their previews from the CDN as they arrive.
    fn sticker_search(self: &Arc<Self>, q: &str) {
        let found = stickers::find(q, STICKER_LIMIT);
        let urls: Vec<String> = found.iter().map(|s| s.url()).collect();
        let seq = with(|s| {
            s.seq += 1;
            s.previews = vec![None; found.len()];
            s.stickers = Some(found);
            s.seq
        });
        let status = if urls.is_empty() {
            "No stickers found."
        } else {
            ""
        };
        self.ui.gifs(rows(), status.into());
        if !self.offline.load(std::sync::atomic::Ordering::Relaxed) {
            self.load_previews(seq, urls);
        }
    }

    /// Downloads the grid's previews; each one redraws the grid as it arrives.
    fn load_previews(self: &Arc<Self>, seq: u64, urls: Vec<String>) {
        for (i, url) in urls.into_iter().enumerate() {
            let e = self.clone();
            tokio::spawn(async move {
                let Ok(bytes) = e.client.bytes(&url, &[], 4 * 1024 * 1024).await else {
                    return;
                };
                let px = decode_limited(move || decode_image(&bytes, 320, 240))
                    .await
                    .flatten();
                let current = with(|s| {
                    if s.seq != seq {
                        return false;
                    }
                    if let Some(p) = s.previews.get_mut(i) {
                        *p = px;
                    }
                    true
                });
                if current {
                    e.ui.gifs(rows(), String::new());
                }
            });
        }
    }

    async fn gif_pick(self: &Arc<Self>, i: usize) -> anyhow::Result<()> {
        let html = with(|s| match &s.stickers {
            Some(found) => found.get(i).map(|st| stickers::sticker_html(st)),
            None => s.results.get(i).map(gif_html),
        });
        let Some(html) = html else {
            return Ok(());
        };
        let (conv, root) = {
            let v = self.view.lock().unwrap();
            (v.current.clone(), v.thread_root.clone())
        };
        let Some(conv) = conv else {
            return Ok(());
        };
        let item = self.new_outbox_item(
            target(&conv, root.as_deref()),
            OutboxKind::Text,
            html,
            None,
            None,
            None,
        );
        self.enqueue(item)?;
        self.flush_outbox(false).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        assert_eq!(
            GifCmd::parse("search", "cats"),
            Some(GifCmd::Search("cats".into()))
        );
        assert_eq!(GifCmd::parse("pick", "3"), Some(GifCmd::Pick(3)));
        assert_eq!(
            GifCmd::parse("stickers", "cat"),
            Some(GifCmd::Stickers("cat".into()))
        );
        assert_eq!(GifCmd::parse("pick", "x"), None);
    }

    #[test]
    fn a_gif_in_a_thread_goes_to_the_thread() {
        assert_eq!(target("19:c@thread.tacv2", None), "19:c@thread.tacv2");
        assert_eq!(
            target("19:c@thread.tacv2", Some("17000")),
            "19:c@thread.tacv2;messageid=17000"
        );
    }
}
