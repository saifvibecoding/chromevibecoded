//! Plain (non-secret) browser data: settings, bookmarks, history and the last session.
//! Everything is stored as small JSON files in the profile directory.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY: usize = 5000;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Page opened by new tabs and the home button.
    pub homepage: String,
    /// Search URL template; `{}` is replaced by the query.
    pub search_engine: String,
    /// Hidden tabs are frozen (timers/JS paused, memory trimmed) after this many seconds.
    pub freeze_after_sec: u64,
    /// Hidden tabs are fully unloaded (memory released) after this many minutes.
    pub sleep_after_min: u64,
    /// At most this many tabs keep a live page; the least recently used are unloaded.
    pub max_awake_tabs: usize,
    /// Game mode: aggressive tab sleeping + browser yields CPU to games when unfocused.
    pub game_mode: bool,
    /// Force GPU rasterization / zero-copy (good on gaming GPUs). Applies on restart.
    pub gpu_boost: bool,
    /// Soft cap on renderer processes. Lower = less RAM. Applies on restart.
    pub renderer_process_limit: u32,
    /// Reopen the previous tabs on startup.
    pub restore_session: bool,
    /// Offer to save passwords.
    pub offer_save_passwords: bool,
    /// Sites where the user clicked "Never" on the save-password prompt.
    pub never_save: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            homepage: "https://www.google.com/".into(),
            search_engine: "https://www.google.com/search?q={}".into(),
            freeze_after_sec: 60,
            sleep_after_min: 15,
            max_awake_tabs: 8,
            game_mode: false,
            gpu_boost: true,
            renderer_process_limit: 6,
            restore_session: true,
            offer_save_passwords: true,
            never_save: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub added: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub last_visit: u64,
    pub visits: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Session {
    pub tabs: Vec<String>,
    #[serde(default)]
    pub titles: Vec<String>,
    pub active: usize,
}

pub struct Store {
    dir: PathBuf,
    pub settings: Settings,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryEntry>,
    pub session: Session,
    dirty_settings: bool,
    dirty_bookmarks: bool,
    dirty_history: bool,
    dirty_session: bool,
}

fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn save<T: Serialize>(path: &Path, value: &T) {
    if let Ok(bytes) = serde_json::to_vec(value) {
        if let Err(e) = write_atomic(path, &bytes) {
            eprintln!("rustium: failed to save {}: {e}", path.display());
        }
    }
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            settings: load(&dir.join("settings.json")),
            bookmarks: load(&dir.join("bookmarks.json")),
            history: load(&dir.join("history.json")),
            session: load(&dir.join("session.json")),
            dirty_settings: false,
            dirty_bookmarks: false,
            dirty_history: false,
            dirty_session: false,
        }
    }

    pub fn mark_settings(&mut self) {
        self.dirty_settings = true;
    }
    pub fn mark_bookmarks(&mut self) {
        self.dirty_bookmarks = true;
    }
    pub fn mark_session(&mut self) {
        self.dirty_session = true;
    }

    /// Persist whatever changed since the last flush.
    pub fn flush(&mut self) {
        if std::mem::take(&mut self.dirty_settings) {
            save(&self.dir.join("settings.json"), &self.settings);
        }
        if std::mem::take(&mut self.dirty_bookmarks) {
            save(&self.dir.join("bookmarks.json"), &self.bookmarks);
        }
        if std::mem::take(&mut self.dirty_history) {
            save(&self.dir.join("history.json"), &self.history);
        }
        if std::mem::take(&mut self.dirty_session) {
            save(&self.dir.join("session.json"), &self.session);
        }
    }

    pub fn record_visit(&mut self, url: &str, title: &str) {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return;
        }
        let t = now();
        if let Some(pos) = self.history.iter().position(|h| h.url == url) {
            let mut e = self.history.remove(pos);
            e.visits += 1;
            e.last_visit = t;
            if !title.is_empty() {
                e.title = title.to_string();
            }
            self.history.push(e);
        } else {
            self.history.push(HistoryEntry {
                url: url.to_string(),
                title: title.to_string(),
                last_visit: t,
                visits: 1,
            });
            if self.history.len() > MAX_HISTORY {
                let excess = self.history.len() - MAX_HISTORY;
                self.history.drain(..excess);
            }
        }
        self.dirty_history = true;
    }

    pub fn update_title(&mut self, url: &str, title: &str) {
        if let Some(e) = self.history.iter_mut().rev().find(|h| h.url == url) {
            if e.title != title {
                e.title = title.to_string();
                self.dirty_history = true;
            }
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.dirty_history = true;
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Adds the bookmark, or removes it if it already exists. Returns the new state.
    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        self.dirty_bookmarks = true;
        if let Some(pos) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(pos);
            false
        } else {
            self.bookmarks.push(Bookmark {
                title: if title.is_empty() {
                    url.into()
                } else {
                    title.into()
                },
                url: url.into(),
                folder: String::new(),
                added: now(),
            });
            true
        }
    }

    /// Merge bookmarks, skipping URLs we already have. Returns how many were added.
    pub fn add_bookmarks(&mut self, items: Vec<Bookmark>) -> usize {
        let mut added = 0;
        for b in items {
            if !self.is_bookmarked(&b.url) {
                self.bookmarks.push(b);
                added += 1;
            }
        }
        if added > 0 {
            self.dirty_bookmarks = true;
        }
        added
    }

    /// Omnibox suggestions: bookmarks first, then history by visit count.
    pub fn suggest(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        let q = query.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let matches = |url: &str, title: &str| {
            url.to_lowercase().contains(&q) || title.to_lowercase().contains(&q)
        };
        let mut out: Vec<(String, String)> = self
            .bookmarks
            .iter()
            .filter(|b| matches(&b.url, &b.title))
            .map(|b| (b.url.clone(), b.title.clone()))
            .take(limit)
            .collect();
        let mut hist: Vec<&HistoryEntry> = self
            .history
            .iter()
            .filter(|h| matches(&h.url, &h.title))
            .collect();
        hist.sort_by(|a, b| {
            b.visits
                .cmp(&a.visits)
                .then(b.last_visit.cmp(&a.last_visit))
        });
        for h in hist {
            if out.len() >= limit {
                break;
            }
            if !out.iter().any(|(u, _)| u == &h.url) {
                out.push((h.url.clone(), h.title.clone()));
            }
        }
        out
    }
}
