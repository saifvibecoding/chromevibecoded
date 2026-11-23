//! Import data from an installed Google Chrome (also Edge / Brave, which use the same format).
//!
//! Bookmarks are stored by Chrome as plain JSON, so they can be read directly.
//! Passwords and cookies are encrypted by Chrome with "app-bound encryption" (Chrome 127+),
//! which deliberately blocks other apps from reading them — passwords therefore come in via
//! Chrome's own "Export passwords" CSV instead (see `Vault::import_csv`).

use crate::storage::{now, Bookmark};
use serde_json::Value;
use std::path::PathBuf;

/// `User Data` directories of Chromium-family browsers, most likely first.
fn user_data_dirs() -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        out.push(("Chrome", local.join("Google/Chrome/User Data")));
        out.push(("Edge", local.join("Microsoft/Edge/User Data")));
        out.push(("Brave", local.join("BraveSoftware/Brave-Browser/User Data")));
    }
    #[cfg(target_os = "macos")]
    if let Some(sup) = dirs::data_dir() {
        out.push(("Chrome", sup.join("Google/Chrome")));
        out.push(("Edge", sup.join("Microsoft Edge")));
        out.push(("Brave", sup.join("BraveSoftware/Brave-Browser")));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(cfg) = dirs::config_dir() {
        out.push(("Chrome", cfg.join("google-chrome")));
        out.push(("Chromium", cfg.join("chromium")));
        out.push(("Edge", cfg.join("microsoft-edge")));
        out.push(("Brave", cfg.join("BraveSoftware/Brave-Browser")));
    }
    out
}

/// Profile folders (`Default`, `Profile 1`, ...) that contain a Bookmarks file.
fn bookmark_files() -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    for (browser, dir) in user_data_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                (n == "Default" || n.starts_with("Profile ")) && p.join("Bookmarks").is_file()
            })
            .collect();
        profiles.sort();
        for p in profiles {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            files.push((format!("{browser} ({name})"), p.join("Bookmarks")));
        }
    }
    files
}

fn walk(node: &Value, folder: &str, out: &mut Vec<Bookmark>) {
    match node.get("type").and_then(Value::as_str) {
        Some("url") => {
            let url = node.get("url").and_then(Value::as_str).unwrap_or("");
            if url.starts_with("http://") || url.starts_with("https://") {
                out.push(Bookmark {
                    title: node
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(url)
                        .to_string(),
                    url: url.to_string(),
                    folder: folder.to_string(),
                    added: now(),
                });
            }
        }
        _ => {
            let name = node.get("name").and_then(Value::as_str).unwrap_or("");
            let path = match (folder.is_empty(), name.is_empty()) {
                (true, _) => name.to_string(),
                (false, true) => folder.to_string(),
                (false, false) => format!("{folder}/{name}"),
            };
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for c in children {
                    walk(c, &path, out);
                }
            }
        }
    }
}

pub fn parse_chrome_bookmarks(json: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return out;
    };
    if let Some(roots) = v.get("roots").and_then(Value::as_object) {
        for key in ["bookmark_bar", "other", "synced"] {
            if let Some(root) = roots.get(key) {
                walk(root, "", &mut out);
            }
        }
    }
    out
}

/// Read bookmarks from every Chrome-family profile found on this machine.
/// Returns the bookmarks and a human-readable list of where they came from.
pub fn chrome_bookmarks() -> (Vec<Bookmark>, Vec<String>) {
    let mut all = Vec::new();
    let mut sources = Vec::new();
    for (label, path) in bookmark_files() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let items = parse_chrome_bookmarks(&text);
            if !items.is_empty() {
                sources.push(format!("{label}: {}", items.len()));
                all.extend(items);
            }
        }
    }
    (all, sources)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_chrome_format() {
        let json = r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bookmarks bar","children":[
            {"type":"url","name":"Rust","url":"https://www.rust-lang.org/"},
            {"type":"folder","name":"Games","children":[{"type":"url","name":"Steam","url":"https://store.steampowered.com/"}]},
            {"type":"url","name":"js","url":"javascript:alert(1)"}]},
            "other":{"type":"folder","name":"Other","children":[]}}}"#;
        let b = super::parse_chrome_bookmarks(json);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].folder, "Bookmarks bar/Games");
    }
}
