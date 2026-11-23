//! Rustium — a lightweight, gaming-friendly browser written in Rust.
//!
//! The browser itself (window, tabs, toolbar, password manager, bookmarks, history, tab
//! sleeping, game mode) is Rust. Web pages are rendered by the system web engine through
//! `wry`: on Windows that is Microsoft WebView2 — the Chromium engine (Blink + V8) — so sites
//! behave exactly as in Chrome, but the engine is shared with the OS instead of bundling
//! another 300 MB copy of Chromium.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod import;
mod perf;
mod storage;
mod vault;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};

const UI_HTML: &str = include_str!("ui/chrome.html");
const CONTENT_JS: &str = include_str!("ui/content.js");
const DEFAULT_UI_HEIGHT: f64 = 76.0;

#[derive(Debug)]
enum UserEvent {
    Ui(String),
    Page {
        tab: u32,
        src: String,
        body: String,
    },
    Title {
        tab: u32,
        title: String,
    },
    Load {
        tab: u32,
        finished: bool,
    },
    OpenTab(String),
    DownloadStarted {
        url: String,
        path: PathBuf,
    },
    DownloadDone {
        url: String,
        path: Option<PathBuf>,
        ok: bool,
    },
}

struct Tab {
    id: u32,
    url: String,
    title: String,
    view: Option<WebView>,
    hidden_since: Option<Instant>,
    last_active: Instant,
    frozen: bool,
    playing: bool,
    loading: bool,
}

struct Download {
    url: String,
    name: String,
    path: PathBuf,
    done: bool,
    ok: bool,
}

struct PendingSave {
    origin: String,
    username: String,
    password: String,
}

/// Owns the native window and builds child webviews into it.
struct Host {
    window: Window,
    #[cfg(target_os = "linux")]
    fixed: gtk::Fixed,
}

impl Host {
    fn build(&self, b: WebViewBuilder) -> wry::Result<WebView> {
        #[cfg(target_os = "linux")]
        {
            use wry::WebViewBuilderExtUnix;
            b.build_gtk(&self.fixed)
        }
        #[cfg(not(target_os = "linux"))]
        {
            b.build_as_child(&self.window)
        }
    }

    /// Position a child webview (logical pixels).
    fn place(&self, v: &WebView, x: f64, y: f64, w: f64, h: f64) {
        let _ = v.set_bounds(rect(x, y, w, h));
        // In a GtkFixed, wry's size_allocate is undone by the next GTK layout pass;
        // moving the widget and setting its size request makes the placement stick.
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::*;
            use wry::WebViewExtUnix;
            let widget = v.webview();
            self.fixed.move_(&widget, x as i32, y as i32);
            widget.set_size_request(w.max(1.0) as i32, h.max(1.0) as i32);
        }
    }

    fn size(&self) -> (f64, f64) {
        let s = self
            .window
            .inner_size()
            .to_logical::<f64>(self.window.scale_factor());
        (s.width, s.height)
    }

    /// True when the browser window is the foreground app (used by game mode).
    fn in_foreground(&self) -> bool {
        #[cfg(windows)]
        {
            use tao::platform::windows::WindowExtWindows;
            let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            fg.0 as isize == self.window.hwnd() && !self.window.is_minimized()
        }
        #[cfg(not(windows))]
        {
            self.window.is_focused() && !self.window.is_minimized()
        }
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    }
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Turn omnibox text into a URL: real URLs are opened, everything else is searched.
fn to_url(text: &str, search: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("data:") {
        // never execute script typed/pasted into the address bar
    } else if ["http://", "https://", "file://", "about:", "view-source:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Some(t.to_string());
    } else if !t.contains(char::is_whitespace) {
        let host = t.split(['/', '?', '#']).next().unwrap_or("");
        let hostname = host
            .rsplit_once(':')
            .map(|(h, p)| {
                if p.chars().all(|c| c.is_ascii_digit()) {
                    h
                } else {
                    host
                }
            })
            .unwrap_or(host);
        let looks_like_host = hostname == "localhost"
            || hostname.parse::<std::net::Ipv4Addr>().is_ok()
            || (hostname.contains('.')
                && !hostname.starts_with('.')
                && !hostname.ends_with('.')
                && hostname
                    .rsplit('.')
                    .next()
                    .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_alphabetic())));
        if looks_like_host {
            let scheme =
                if hostname == "localhost" || hostname.parse::<std::net::Ipv4Addr>().is_ok() {
                    "http"
                } else {
                    "https"
                };
            return Some(format!("{scheme}://{t}"));
        }
    }
    let q: String = url::form_urlencoded::byte_serialize(t.as_bytes()).collect();
    Some(search.replace("{}", &q))
}

fn profile_dir() -> PathBuf {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Rustium")
}

fn engine_version() -> String {
    match wry::webview_version() {
        #[cfg(windows)]
        Ok(v) => format!("Chromium / Microsoft WebView2 {v}"),
        #[cfg(not(windows))]
        Ok(v) => format!("WebKitGTK {v} (Linux build; Windows uses Chromium/WebView2)"),
        Err(_) => "WebView2 runtime not found".into(),
    }
}

struct App {
    host: Host,
    ctx: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    ui: Option<WebView>,
    ui_height: f64,
    panel_open: bool,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u32,
    closed: Vec<String>,
    store: storage::Store,
    vault: Option<vault::Vault>,
    vault_error: Option<String>,
    pending_save: Option<PendingSave>,
    downloads: Vec<Download>,
    monitor: perf::Monitor,
    mem: u64,
    last_mem_sample: Instant,
    backgrounded: bool,
    engine_args: String,
    ext_dir: PathBuf,
    dir: PathBuf,
    last_page_newtab: Instant,
    quit: bool,
}

impl App {
    // ------------------------------------------------------------------ webviews

    fn eval_ui(&self, js: &str) {
        if let Some(ui) = &self.ui {
            let _ = ui.evaluate_script(js);
        }
    }

    fn build_ui(&mut self) {
        let proxy = self.proxy.clone();
        let (w, _) = self.host.size();
        let b = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_html(UI_HTML)
            .with_bounds(rect(0.0, 0.0, w, self.ui_height))
            .with_background_color((32, 33, 36, 255))
            .with_clipboard(true)
            .with_devtools(cfg!(debug_assertions))
            .with_hotkeys_zoom(false)
            // The toolbar must never navigate anywhere (it has privileged IPC access).
            .with_navigation_handler(|url| !(url.starts_with("http") || url.starts_with("file")))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(UserEvent::Ui(req.into_body()));
            });
        let b = platform_opts(b, &self.engine_args, &self.ext_dir, true);
        match self.host.build(b) {
            Ok(v) => self.ui = Some(v),
            Err(e) => fatal(&format!("Could not start the browser engine: {e}\n\nOn Windows, install the Microsoft WebView2 Runtime (it ships with Windows 11).")),
        }
    }

    fn spawn_view(&mut self, id: u32, url: &str) -> Option<WebView> {
        let (w, h) = self.host.size();
        let bounds = rect(0.0, self.ui_height, w, h - self.ui_height);
        let p = self.proxy.clone();
        let (p1, p2, p3, p4, p5, p6) = (p.clone(), p.clone(), p.clone(), p.clone(), p.clone(), p);
        let downloads = dirs::download_dir().unwrap_or_else(|| self.dir.join("Downloads"));
        let b = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url(url)
            .with_bounds(bounds)
            .with_visible(false)
            .with_background_color((255, 255, 255, 255))
            .with_initialization_script_for_main_only(CONTENT_JS, true)
            .with_devtools(true)
            .with_clipboard(true)
            .with_hotkeys_zoom(true)
            .with_ipc_handler(move |req| {
                let src = req.uri().to_string();
                let _ = p1.send_event(UserEvent::Page {
                    tab: id,
                    src,
                    body: req.into_body(),
                });
            })
            .with_document_title_changed_handler(move |title| {
                let _ = p2.send_event(UserEvent::Title { tab: id, title });
            })
            .with_on_page_load_handler(move |ev, _url| {
                let _ = p3.send_event(UserEvent::Load {
                    tab: id,
                    finished: matches!(ev, PageLoadEvent::Finished),
                });
            })
            .with_new_window_req_handler(move |url, features| {
                // Sized popups (OAuth "Sign in with ..." windows) stay real popups so
                // `window.opener` keeps working; ordinary target=_blank links become tabs.
                if features.size.is_some() {
                    NewWindowResponse::Allow
                } else {
                    let _ = p4.send_event(UserEvent::OpenTab(url));
                    NewWindowResponse::Deny
                }
            })
            .with_download_started_handler(move |url, path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .or_else(|| {
                        url.split(['?', '#'])
                            .next()?
                            .rsplit('/')
                            .next()
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "download".into());
                let _ = std::fs::create_dir_all(&downloads);
                *path = unique_path(&downloads, &name);
                let _ = p5.send_event(UserEvent::DownloadStarted {
                    url,
                    path: path.clone(),
                });
                true
            })
            .with_download_completed_handler(move |url, path, ok| {
                let _ = p6.send_event(UserEvent::DownloadDone { url, path, ok });
            });
        let b = platform_opts(b, &self.engine_args, &self.ext_dir, false);
        match self.host.build(b) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("rustium: failed to create tab: {e}");
                None
            }
        }
    }

    // ------------------------------------------------------------------ tabs

    fn tab_index(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn new_tab(&mut self, url: &str, activate: bool) {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            url: url.to_string(),
            title: String::new(),
            view: None,
            hidden_since: Some(Instant::now()),
            last_active: Instant::now(),
            frozen: false,
            playing: false,
            loading: false,
        });
        let idx = self.tabs.len() - 1;
        if activate || self.tabs.len() == 1 {
            self.activate(idx);
        } else {
            self.push_state();
        }
        self.save_session();
    }

    fn activate(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if idx != self.active {
            if let Some(prev) = self.tabs.get_mut(self.active) {
                if let Some(v) = &prev.view {
                    let _ = v.set_visible(false);
                    set_memory_low(v, true);
                }
                prev.hidden_since = Some(Instant::now());
            }
        }
        self.active = idx;
        if self.tabs[idx].view.is_none() {
            let (id, url) = (self.tabs[idx].id, self.tabs[idx].url.clone());
            self.tabs[idx].view = self.spawn_view(id, &url);
            self.tabs[idx].loading = true;
        }
        let (w, h) = self.host.size();
        let ui_h = self.ui_height;
        let panel_open = self.panel_open;
        let t = &mut self.tabs[idx];
        t.hidden_since = None;
        t.frozen = false;
        t.last_active = Instant::now();
        if let Some(v) = &t.view {
            set_memory_low(v, false);
            self.host.place(v, 0.0, ui_h, w, h - ui_h);
            if !panel_open {
                let _ = v.set_visible(true);
                let _ = v.focus();
            }
        }
        self.store.session.active = idx;
        self.store.mark_session();
        self.push_state();
    }

    fn close_tab(&mut self, id: u32) {
        let Some(idx) = self.tab_index(id) else {
            return;
        };
        if self.tabs.len() == 1 {
            // Closing the last tab closes the browser, like Chrome. Session is kept as-is
            // so "continue where you left off" brings it back.
            self.quit = true;
            return;
        }
        let tab = self.tabs.remove(idx);
        if tab.url.starts_with("http") {
            self.closed.push(tab.url.clone());
            if self.closed.len() > 25 {
                self.closed.remove(0);
            }
        }
        drop(tab); // destroys the webview and frees its memory
        if idx == self.active {
            // The visible tab was closed: show its neighbour (nothing left to hide).
            self.active = usize::MAX;
            self.activate(idx.min(self.tabs.len() - 1));
        } else if idx < self.active {
            self.active -= 1;
        }
        self.save_session();
        self.push_state();
    }

    fn active_view(&self) -> Option<&WebView> {
        self.tabs.get(self.active).and_then(|t| t.view.as_ref())
    }

    fn navigate_active(&mut self, url: &str) {
        if self.tabs.is_empty() {
            self.new_tab(url, true);
            return;
        }
        let idx = self.active;
        self.tabs[idx].url = url.to_string();
        match &self.tabs[idx].view {
            Some(v) => {
                let _ = v.load_url(url);
                let _ = v.focus();
            }
            None => self.activate(idx),
        }
        self.push_state();
    }

    fn discard(&mut self, idx: usize) {
        let t = &mut self.tabs[idx];
        if let Some(v) = t.view.take() {
            if let Ok(u) = v.url() {
                if !u.is_empty() && u != "about:blank" {
                    t.url = u;
                }
            }
        }
        t.frozen = false;
        t.playing = false;
        t.loading = false;
    }

    fn save_session(&mut self) {
        self.store.session.tabs = self.tabs.iter().map(|t| t.url.clone()).collect();
        self.store.session.titles = self.tabs.iter().map(|t| t.title.clone()).collect();
        self.store.session.active = self.active;
        self.store.mark_session();
    }

    // ------------------------------------------------------------------ layout & state

    fn layout(&mut self) {
        let (w, h) = self.host.size();
        if let Some(ui) = &self.ui {
            let ui_h = if self.panel_open { h } else { self.ui_height };
            self.host.place(ui, 0.0, 0.0, w, ui_h);
        }
        if let Some(v) = self.active_view() {
            self.host
                .place(v, 0.0, self.ui_height, w, h - self.ui_height);
            let _ = v.set_visible(!self.panel_open);
        }
    }

    fn push_state(&self) {
        let (can_back, can_fwd) = self
            .active_view()
            .map(|v| {
                (
                    v.can_go_back().unwrap_or(false),
                    v.can_go_forward().unwrap_or(false),
                )
            })
            .unwrap_or((false, false));
        let url = self
            .tabs
            .get(self.active)
            .map(|t| t.url.clone())
            .unwrap_or_default();
        let state = json!({
            "tabs": self.tabs.iter().enumerate().map(|(i, t)| json!({
                "id": t.id, "title": t.title, "url": t.url, "active": i == self.active,
                "sleeping": t.view.is_none(), "frozen": t.frozen, "playing": t.playing, "loading": t.loading,
            })).collect::<Vec<_>>(),
            "url": url,
            "bookmarked": self.store.is_bookmarked(&url),
            "canBack": can_back, "canForward": can_fwd,
            "game": self.store.settings.game_mode,
            "mem": self.mem,
            "downloads": self.downloads.iter().map(|d| json!({
                "name": d.name, "path": d.path.to_string_lossy(), "done": d.done, "ok": d.ok,
            })).collect::<Vec<_>>(),
        });
        self.eval_ui(&format!("window.R&&R.state({state})"));
    }

    fn reply(&self, kind: &str, data: Value) {
        self.eval_ui(&format!("window.R&&R.reply({},{data})", js_str(kind)));
    }

    fn toast(&self, msg: &str) {
        self.eval_ui(&format!("window.R&&R.toast({})", js_str(msg)));
    }

    // ------------------------------------------------------------------ game mode & memory

    fn set_game_mode(&mut self, on: bool) {
        self.store.settings.game_mode = on;
        self.store.mark_settings();
        self.tick(true);
        if !on && self.backgrounded {
            self.monitor.set_background(false);
            self.backgrounded = false;
        }
        self.check_foreground();
        self.toast(if on {
            "🎮 Game mode on — background tabs sleep, and the browser yields the CPU to your game when it's not focused."
        } else {
            "Game mode off"
        });
        self.push_state();
    }

    fn check_foreground(&mut self) {
        let want_bg = self.store.settings.game_mode && !self.host.in_foreground();
        if want_bg != self.backgrounded {
            self.backgrounded = want_bg;
            self.monitor.set_background(want_bg);
            if let Some(v) = self.active_view() {
                set_memory_low(v, want_bg);
            }
        }
    }

    /// Periodic housekeeping: freeze → unload background tabs, enforce the loaded-tab cap,
    /// sample memory, flush data to disk.
    fn tick(&mut self, force_mem: bool) {
        let s = &self.store.settings;
        let game = s.game_mode;
        let freeze_after = Duration::from_secs(if game { 0 } else { s.freeze_after_sec });
        let sleep_after = Duration::from_secs(if game { 180 } else { s.sleep_after_min * 60 });
        let cap = if game { 3 } else { s.max_awake_tabs.max(1) };
        let mut changed = false;

        for i in 0..self.tabs.len() {
            if i == self.active || self.tabs[i].playing || self.tabs[i].view.is_none() {
                continue;
            }
            let Some(since) = self.tabs[i].hidden_since else {
                continue;
            };
            let idle = since.elapsed();
            if idle >= sleep_after {
                self.discard(i);
                changed = true;
            } else if !self.tabs[i].frozen && idle >= freeze_after {
                if let Some(v) = &self.tabs[i].view {
                    freeze(v);
                }
                self.tabs[i].frozen = true;
                changed = true;
            }
        }
        loop {
            let awake = self.tabs.iter().filter(|t| t.view.is_some()).count();
            if awake <= cap {
                break;
            }
            let victim = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, t)| *i != self.active && t.view.is_some() && !t.playing)
                .min_by_key(|(_, t)| t.last_active)
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    self.discard(i);
                    changed = true;
                }
                None => break,
            }
        }

        let mem_every = Duration::from_secs(if game { 30 } else { 5 });
        if force_mem || self.last_mem_sample.elapsed() >= mem_every {
            self.mem = self.monitor.memory();
            self.last_mem_sample = Instant::now();
            changed = true;
        }
        self.store.flush();
        if changed {
            self.push_state();
        }
    }

    // ------------------------------------------------------------------ events

    fn shortcut(&mut self, k: &str, from_page: bool) {
        let n = self.tabs.len();
        match k {
            "newtab" => {
                if from_page && self.last_page_newtab.elapsed() < Duration::from_millis(400) {
                    return; // pages can't spam-open tabs
                }
                self.last_page_newtab = Instant::now();
                let home = self.store.settings.homepage.clone();
                self.new_tab(&home, true);
                self.eval_ui("window.R&&R.focusOmni()");
                if let Some(ui) = &self.ui {
                    let _ = ui.focus();
                }
            }
            "reopen" => {
                if let Some(u) = self.closed.pop() {
                    self.new_tab(&u, true);
                }
            }
            "close" => {
                if let Some(t) = self.tabs.get(self.active) {
                    self.close_tab(t.id);
                }
            }
            "focus" => {
                if let Some(ui) = &self.ui {
                    let _ = ui.focus();
                }
                self.eval_ui("window.R&&R.focusOmni()");
            }
            "next" if n > 0 => self.activate((self.active + 1) % n),
            "prev" if n > 0 => self.activate((self.active + n - 1) % n),
            "tab9" if n > 0 => self.activate(n - 1),
            k if k.starts_with("tab") => {
                if let Ok(i) = k[3..].parse::<usize>() {
                    if i >= 1 && i <= n {
                        self.activate(i - 1);
                    }
                }
            }
            "bookmark" => self.toggle_bookmark(),
            "history" | "downloads" | "bookmarks" | "passwords" | "settings" => {
                if let Some(ui) = &self.ui {
                    let _ = ui.focus();
                }
                self.eval_ui(&format!("window.R&&R.openPanel({})", js_str(k)));
            }
            "back" => {
                if let Some(v) = self.active_view() {
                    let _ = v.go_back();
                }
            }
            "forward" => {
                if let Some(v) = self.active_view() {
                    let _ = v.go_forward();
                }
            }
            "reload" => match self.active_view() {
                Some(v) => {
                    let _ = v.reload();
                }
                None => self.activate(self.active),
            },
            "home" => {
                let home = self.store.settings.homepage.clone();
                self.navigate_active(&home);
            }
            "devtools" => {
                if let Some(v) = self.active_view() {
                    if v.is_devtools_open() {
                        v.close_devtools();
                    } else {
                        v.open_devtools();
                    }
                }
            }
            "gamemode" => {
                let on = !self.store.settings.game_mode;
                self.set_game_mode(on);
            }
            _ => {}
        }
    }

    fn toggle_bookmark(&mut self) {
        let Some(t) = self.tabs.get(self.active) else {
            return;
        };
        let (url, title) = (t.url.clone(), t.title.clone());
        if !url.starts_with("http") {
            return;
        }
        let on = self.store.toggle_bookmark(&url, &title);
        self.toast(if on {
            "★ Bookmarked"
        } else {
            "Bookmark removed"
        });
        self.push_state();
    }

    fn on_page(&mut self, tab: u32, src: String, body: String) {
        let Some(idx) = self.tab_index(tab) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return;
        };
        match v["t"].as_str().unwrap_or("") {
            "key" => {
                let k = v["k"].as_str().unwrap_or("").to_string();
                self.shortcut(&k, true);
            }
            "url" => {
                if let Some(u) = self.tabs[idx].view.as_ref().and_then(|w| w.url().ok()) {
                    let title = self.tabs[idx].title.clone();
                    self.store.record_visit(&u, &title);
                    self.tabs[idx].url = u;
                    if idx == self.active {
                        self.push_state();
                    }
                }
            }
            "media" => {
                self.tabs[idx].playing = v["on"].as_bool().unwrap_or(false);
                self.push_state();
            }
            "pw_query" => self.autofill(idx, &src),
            "pw_save" => {
                let u = v["u"].as_str().unwrap_or("").to_string();
                let p = v["p"].as_str().unwrap_or("").to_string();
                self.offer_save(&src, u, p);
            }
            _ => {}
        }
    }

    fn autofill(&mut self, idx: usize, src: &str) {
        let Some(origin) = vault::origin_of(src) else {
            return;
        };
        let secure = origin.starts_with("https://")
            || origin.starts_with("http://localhost")
            || origin.starts_with("http://127.0.0.1");
        if !secure {
            return;
        }
        let Some(vault) = &mut self.vault else { return };
        let creds: Vec<Value> = vault
            .for_origin(&origin)
            .iter()
            .map(|c| json!({ "u": c.username, "p": c.password }))
            .collect();
        if creds.is_empty() {
            return;
        }
        vault.mark_used(&origin);
        // The origin check runs inside the page at fill time, so if the tab navigated to a
        // different site in the meantime, nothing is filled. `window.location` is unforgeable.
        let js = format!(
            "(function(){{if(window.location.origin!=={o})return;if(window.__rsFill)window.__rsFill({c});}})();",
            o = js_str(&origin),
            c = Value::Array(creds)
        );
        if let Some(v) = &self.tabs[idx].view {
            let _ = v.evaluate_script(&js);
        }
    }

    fn offer_save(&mut self, src: &str, username: String, password: String) {
        if !self.store.settings.offer_save_passwords || password.is_empty() || password.len() > 1024
        {
            return;
        }
        let Some(origin) = vault::origin_of(src) else {
            return;
        };
        if self.store.settings.never_save.contains(&origin) {
            return;
        }
        let Some(vault) = &self.vault else { return };
        let update = match vault.classify(&origin, &username, &password) {
            vault::Upsert::Unchanged => return,
            vault::Upsert::Updated => true,
            vault::Upsert::Added => false,
        };
        let prompt = json!({ "origin": origin, "username": username, "update": update });
        self.pending_save = Some(PendingSave {
            origin,
            username,
            password,
        });
        self.eval_ui(&format!("window.R&&R.savePrompt({prompt})"));
    }

    fn password_list(&self) {
        let list: Vec<Value> = self
            .vault
            .as_ref()
            .map(|v| {
                let mut l: Vec<&vault::Credential> = v.all().iter().collect();
                l.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.username.cmp(&b.username)));
                l.iter()
                    .map(|c| json!({ "id": c.id, "origin": c.origin, "username": c.username }))
                    .collect()
            })
            .unwrap_or_default();
        self.reply("passwords", json!({ "list": list, "dpapi": cfg!(windows) }));
        if let Some(e) = &self.vault_error {
            self.toast(&format!("Password vault unavailable: {e}"));
        }
    }

    fn on_ui(&mut self, body: String) {
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return;
        };
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        match v["cmd"].as_str().unwrap_or("") {
            "ready" => {
                self.push_state();
                if let Some(e) = &self.vault_error {
                    self.toast(&format!("Password vault unavailable: {e}"));
                }
            }
            "height" => {
                let h = v["h"]
                    .as_f64()
                    .unwrap_or(DEFAULT_UI_HEIGHT)
                    .clamp(40.0, 400.0);
                if (h - self.ui_height).abs() > 0.5 {
                    self.ui_height = h;
                    self.layout();
                }
            }
            "panel" => {
                self.panel_open = v["open"].as_bool().unwrap_or(false);
                self.layout();
                if !self.panel_open {
                    if let Some(v) = self.active_view() {
                        let _ = v.focus();
                    }
                }
            }
            "newtab" => self.shortcut("newtab", false),
            "key" => self.shortcut(&s("k"), false),
            "close" => self.close_tab(v["id"].as_u64().unwrap_or(0) as u32),
            "activate" => {
                if let Some(i) = self.tab_index(v["id"].as_u64().unwrap_or(0) as u32) {
                    self.activate(i);
                }
            }
            "navigate" => {
                if let Some(url) = to_url(&s("text"), &self.store.settings.search_engine) {
                    self.navigate_active(&url);
                }
            }
            "open_url" => {
                let url = s("url");
                if url.starts_with("http") {
                    self.new_tab(&url, true);
                }
            }
            "focus_page" => {
                if let Some(v) = self.active_view() {
                    let _ = v.focus();
                }
            }
            "back" | "forward" | "reload" | "home" => {
                self.shortcut(v["cmd"].as_str().unwrap(), false)
            }
            "bookmark_toggle" => self.toggle_bookmark(),
            "game_mode" => self.set_game_mode(v["on"].as_bool().unwrap_or(false)),
            "suggest" => {
                let list = self.store.suggest(&s("q"), 8);
                self.reply("suggest", json!(list));
            }

            // passwords
            "pw_list" => self.password_list(),
            "pw_reveal" => {
                let id = v["id"].as_u64().unwrap_or(0);
                if let Some(c) = self.vault.as_ref().and_then(|vl| vl.get(id)) {
                    self.reply("password", json!({ "id": id, "password": c.password, "copy": v["copy"].as_bool().unwrap_or(false) }));
                }
            }
            "pw_delete" => {
                if let Some(vl) = &mut self.vault {
                    vl.delete(v["id"].as_u64().unwrap_or(0));
                }
                self.password_list();
            }
            "pw_import_csv" => {
                let res = match &mut self.vault {
                    Some(vl) => vl.import_csv(&s("csv")),
                    None => Err(self.vault_error.clone().unwrap_or_default()),
                };
                match res {
                    Ok((a, u, sk)) => self.toast(&format!(
                        "Imported {a} passwords{}{}. Now delete “{}” — it holds your passwords in plain text.",
                        if u > 0 { format!(", updated {u}") } else { String::new() },
                        if sk > 0 { format!(" ({sk} skipped: duplicates or app-only entries)") } else { String::new() },
                        s("name")
                    )),
                    Err(e) => self.toast(&format!("Import failed: {e}")),
                }
                self.password_list();
            }
            "pw_export" => {
                if let Some(vl) = &self.vault {
                    let dir = dirs::download_dir().unwrap_or_else(|| self.dir.clone());
                    let path = unique_path(&dir, "Rustium Passwords.csv");
                    match storage::write_atomic(&path, vl.export_csv().as_bytes()) {
                        Ok(()) => self.toast(&format!(
                            "Exported to {} — delete it when you're done.",
                            path.display()
                        )),
                        Err(e) => self.toast(&format!("Export failed: {e}")),
                    }
                }
            }
            "pw_save_answer" => {
                if let Some(p) = self.pending_save.take() {
                    match s("answer").as_str() {
                        "save" => {
                            if let Some(vl) = &mut self.vault {
                                vl.upsert(&p.origin, &p.username, &p.password);
                                self.toast("🔑 Password saved");
                            }
                        }
                        "never" => {
                            self.store.settings.never_save.push(p.origin);
                            self.store.mark_settings();
                        }
                        _ => {}
                    }
                }
            }

            // bookmarks & history
            "bm_list" => self.reply("bookmarks", json!(self.store.bookmarks)),
            "bm_delete" => {
                let i = v["i"].as_u64().unwrap_or(u64::MAX) as usize;
                if i < self.store.bookmarks.len() {
                    self.store.bookmarks.remove(i);
                    self.store.mark_bookmarks();
                }
                self.reply("bookmarks", json!(self.store.bookmarks));
                self.push_state();
            }
            "bm_import_chrome" => {
                let (items, sources) = import::chrome_bookmarks();
                if sources.is_empty() {
                    self.toast("No Chrome, Edge or Brave bookmarks were found on this PC.");
                } else {
                    let added = self.store.add_bookmarks(items);
                    self.toast(&format!(
                        "Imported {added} new bookmarks from {}",
                        sources.join(", ")
                    ));
                }
                self.reply("bookmarks", json!(self.store.bookmarks));
                self.push_state();
            }
            "hist_list" => {
                let q = s("q").to_lowercase();
                let list: Vec<&storage::HistoryEntry> = self
                    .store
                    .history
                    .iter()
                    .rev()
                    .filter(|h| {
                        q.is_empty()
                            || h.url.to_lowercase().contains(&q)
                            || h.title.to_lowercase().contains(&q)
                    })
                    .take(500)
                    .collect();
                self.reply("history", json!(list));
            }
            "hist_clear" => {
                self.store.clear_history();
                self.reply("history", json!([]));
                self.toast("History cleared");
            }
            "clear_data" => {
                if let Some(v) = self.active_view().or(self.ui.as_ref()) {
                    let _ = v.clear_all_browsing_data();
                }
                self.toast("Cookies, cache and site data cleared");
            }
            "show_file" => show_in_folder(Path::new(&s("path"))),

            // settings & performance
            "settings_get" => self.reply(
                "settings",
                json!({
                    "settings": self.store.settings,
                    "version": env!("CARGO_PKG_VERSION"),
                    "engine": engine_version(),
                    "data_dir": self.dir.to_string_lossy(),
                    "ext_dir": self.ext_dir.to_string_lossy(),
                }),
            ),
            "settings_set" => {
                if let Ok(new) = serde_json::from_value::<storage::Settings>(v["settings"].clone())
                {
                    let game_changed = new.game_mode != self.store.settings.game_mode;
                    self.store.settings = new;
                    self.store.mark_settings();
                    if game_changed {
                        let on = self.store.settings.game_mode;
                        self.set_game_mode(on);
                    }
                    self.tick(false);
                }
            }
            "perf" => {
                self.tick(true);
                self.reply(
                    "perf",
                    json!({
                        "mem": self.mem,
                        "awake": self.tabs.iter().filter(|t| t.view.is_some()).count(),
                        "total": self.tabs.len(),
                        "engine": engine_version(),
                    }),
                );
            }
            "sleep_all" => {
                for i in 0..self.tabs.len() {
                    if i != self.active && !self.tabs[i].playing {
                        self.discard(i);
                    }
                }
                self.tick(true);
                self.toast("All background tabs are asleep");
                self.on_ui(r#"{"cmd":"perf"}"#.into());
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: UserEvent) {
        match ev {
            UserEvent::Ui(body) => self.on_ui(body),
            UserEvent::Page { tab, src, body } => self.on_page(tab, src, body),
            UserEvent::Title { tab, title } => {
                if let Some(i) = self.tab_index(tab) {
                    let url = self.tabs[i].url.clone();
                    self.store.update_title(&url, &title);
                    self.tabs[i].title = title;
                    self.push_state();
                }
            }
            UserEvent::Load { tab, finished } => {
                if let Some(i) = self.tab_index(tab) {
                    self.tabs[i].loading = !finished;
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() {
                            self.tabs[i].url = u;
                        }
                    }
                    if finished {
                        let (u, t) = (self.tabs[i].url.clone(), self.tabs[i].title.clone());
                        self.store.record_visit(&u, &t);
                        self.save_session();
                    }
                    self.push_state();
                }
            }
            UserEvent::OpenTab(url) => self.new_tab(&url, true),
            UserEvent::DownloadStarted { url, path } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.toast(&format!("⬇ Downloading {name}…"));
                self.downloads.push(Download {
                    url,
                    name,
                    path,
                    done: false,
                    ok: false,
                });
                self.push_state();
            }
            UserEvent::DownloadDone { url, path, ok } => {
                if let Some(d) = self
                    .downloads
                    .iter_mut()
                    .rev()
                    .find(|d| d.url == url && !d.done)
                {
                    d.done = true;
                    d.ok = ok;
                    if let Some(p) = path {
                        d.path = p;
                    }
                    let msg = if ok {
                        format!("✔ Downloaded {}", d.name)
                    } else {
                        format!("✖ Download failed: {}", d.name)
                    };
                    self.toast(&msg);
                }
                self.push_state();
            }
        }
    }
}

/// Engine options shared by every webview. Free function (not a method) because the builder
/// already holds a mutable borrow of `App::ctx`.
fn platform_opts<'a>(
    b: WebViewBuilder<'a>,
    engine_args: &str,
    ext_dir: &Path,
    is_ui: bool,
) -> WebViewBuilder<'a> {
    #[cfg(windows)]
    {
        use wry::WebViewBuilderExtWindows;
        let mut b = b
            .with_additional_browser_args(engine_args)
            .with_browser_extensions_enabled(true)
            .with_browser_accelerator_keys(!is_ui);
        if is_ui {
            // Extensions are installed into the shared profile once, via the toolbar webview.
            b = b.with_extensions_path(ext_dir);
        }
        b
    }
    #[cfg(not(windows))]
    {
        let _ = (engine_args, ext_dir, is_ui);
        b
    }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name: String = name
        .chars()
        .map(|c| {
            if r#"\/:*?"<>|"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut p = dir.join(&name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut n = 1;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    p
}

fn show_in_folder(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer")
        .arg("/select,")
        .arg(path)
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Ask the engine to trim memory for a hidden tab (Windows / WebView2 only).
fn set_memory_low(v: &WebView, low: bool) {
    #[cfg(windows)]
    {
        use wry::{MemoryUsageLevel, WebViewExtWindows};
        let _ = v.set_memory_usage_level(if low {
            MemoryUsageLevel::Low
        } else {
            MemoryUsageLevel::Normal
        });
    }
    #[cfg(not(windows))]
    let _ = (v, low);
}

/// Freeze a hidden tab: its JS, timers and rendering stop until it's shown again. Tabs
/// playing audio refuse to suspend, so music keeps going. (WebKitGTK throttles hidden views
/// on its own.)
fn freeze(v: &WebView) {
    #[cfg(windows)]
    unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
        use webview2_com::TrySuspendCompletedHandler;
        use windows::core::Interface;
        use wry::WebViewExtWindows;
        if let Ok(wv3) = v.webview().cast::<ICoreWebView2_3>() {
            let handler = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
            let _ = wv3.TrySuspend(&handler);
        }
    }
    #[cfg(not(windows))]
    let _ = v;
}

fn fatal(msg: &str) -> ! {
    eprintln!("rustium: {msg}");
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR};
        MessageBoxW(
            None,
            &HSTRING::from(msg),
            windows::core::w!("Rustium"),
            MB_ICONERROR,
        );
    }
    std::process::exit(1);
}

fn main() {
    let dir = profile_dir();
    let ext_dir = dir.join("Extensions");
    let engine_dir = dir.join("Engine");
    for d in [&dir, &ext_dir, &engine_dir] {
        if let Err(e) = std::fs::create_dir_all(d) {
            fatal(&format!(
                "cannot create profile folder {}: {e}",
                d.display()
            ));
        }
    }
    let store = storage::Store::open(&dir);
    if store.settings.game_mode {
        // Engine processes inherit this priority when they are spawned.
        perf::lower_own_priority_at_startup();
    }
    let (vault, vault_error) = match vault::Vault::open(&dir) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };

    #[cfg(target_os = "linux")]
    {
        // Wayland can't host multiple child webviews in one window; use XWayland instead.
        if std::env::var_os("GDK_BACKEND").is_none() {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Rustium")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(420.0, 300.0))
        .build(&event_loop)
        .unwrap_or_else(|e| fatal(&format!("cannot open window: {e}")));

    #[cfg(target_os = "linux")]
    let fixed = {
        use gtk::prelude::*;
        use tao::platform::unix::WindowExtUnix;
        let fixed = gtk::Fixed::new();
        let vbox = window.default_vbox().expect("tao gtk window has a vbox");
        vbox.pack_start(&fixed, true, true, 0);
        fixed.show_all();
        fixed
    };

    let mut app = App {
        host: Host {
            window,
            #[cfg(target_os = "linux")]
            fixed,
        },
        ctx: WebContext::new(Some(engine_dir)),
        proxy: event_loop.create_proxy(),
        ui: None,
        ui_height: DEFAULT_UI_HEIGHT,
        panel_open: false,
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        closed: Vec::new(),
        engine_args: perf::engine_args(&store.settings),
        store,
        vault,
        vault_error,
        pending_save: None,
        downloads: Vec::new(),
        monitor: perf::Monitor::new(),
        mem: 0,
        last_mem_sample: Instant::now() - Duration::from_secs(3600),
        backgrounded: false,
        ext_dir,
        dir,
        last_page_newtab: Instant::now() - Duration::from_secs(10),
        quit: false,
    };

    app.build_ui();

    // Restore the previous session lazily: only the active tab loads now, the rest load
    // when you click them — so 30 restored tabs cost the RAM of one.
    let session = app.store.session.clone();
    let restore = app.store.settings.restore_session && !session.tabs.is_empty();
    let cli_url = std::env::args()
        .nth(1)
        .and_then(|a| to_url(&a, &app.store.settings.search_engine));
    if restore {
        for (i, u) in session.tabs.iter().enumerate() {
            let id = app.next_id;
            app.next_id += 1;
            app.tabs.push(Tab {
                id,
                url: u.clone(),
                title: session.titles.get(i).cloned().unwrap_or_default(),
                view: None,
                hidden_since: Some(Instant::now()),
                last_active: Instant::now(),
                frozen: false,
                playing: false,
                loading: false,
            });
        }
        app.active = usize::MAX;
        app.activate(session.active.min(app.tabs.len() - 1));
    }
    match cli_url {
        Some(u) => app.new_tab(&u, true),
        None if !restore => {
            let home = app.store.settings.homepage.clone();
            app.new_tab(&home, true);
        }
        None => {}
    }

    let tick_every = Duration::from_secs(2);
    let mut next_tick = Instant::now() + tick_every;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_tick);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                app.check_foreground();
                app.tick(false);
                next_tick = Instant::now() + tick_every;
                *control_flow = ControlFlow::WaitUntil(next_tick);
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => app.quit = true,
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => app.layout(),
                WindowEvent::Focused(_) => app.check_foreground(),
                _ => {}
            },
            Event::UserEvent(ev) => app.on_event(ev),
            _ => {}
        }
        if app.quit {
            app.save_session();
            app.store.flush();
            app.tabs.clear();
            app.ui = None;
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::to_url;
    const S: &str = "https://s.test/?q={}";

    #[test]
    fn omnibox() {
        assert_eq!(to_url("example.com", S).unwrap(), "https://example.com");
        assert_eq!(
            to_url("example.com/a b", S).unwrap(),
            "https://s.test/?q=example.com%2Fa+b"
        );
        assert_eq!(
            to_url("localhost:3000/x", S).unwrap(),
            "http://localhost:3000/x"
        );
        assert_eq!(to_url("192.168.1.1", S).unwrap(), "http://192.168.1.1");
        assert_eq!(to_url("https://a.b/c", S).unwrap(), "https://a.b/c");
        assert_eq!(
            to_url("best gaming mouse", S).unwrap(),
            "https://s.test/?q=best+gaming+mouse"
        );
        assert_eq!(to_url("v1.2", S).unwrap(), "https://s.test/?q=v1.2");
        assert!(to_url("javascript:alert(1)", S)
            .unwrap()
            .starts_with("https://s.test/"));
        assert!(to_url("   ", S).is_none());
    }
}
