# Rustium: Technical Audit

**Audited revision:** `8af00e9` on branch `claude/chrome-rust-implementation-370f3e` of `saifvibecoding/chromevibecoded`
**Audit date:** 2026-11-26
**Released build:** GitHub Release `v0.1.0`, asset `rustium.exe` (1,317,376 bytes, built by CI on `windows-latest`)
**Scope:** every source file in the repository, the dependency tree (`Cargo.lock`), the CI workflow, and the relevant internals of the `wry` 0.57.3 library the project is built on. No code was changed for this audit.

This report is meant to stand alone. Another engineer or AI should be able to plan the next phase from it without access to the original conversation.

---

## 0. How to read this report

### Feature status labels

| Label | Meaning |
|---|---|
| **FULL** | Implemented end to end in code. Every step the feature needs exists. |
| **PARTIAL** | Works in a reduced form, or important sub-features are missing. |
| **BROKEN** | Code exists but is wrong, or fails in an identifiable situation. |
| **MISSING** | No implementation. |
| **PLATFORM** | Not implemented by Rustium. Provided, or possibly provided, by the WebView2 / Chromium engine on its own. |

### Verification labels

| Label | Meaning |
|---|---|
| **T-LINUX** | Manually exercised in this project's Linux build (WebKitGTK engine) under a virtual X display (Xvfb), with screenshots checked. Network access in that container was restricted, so only `localhost` pages were loaded. |
| **T-CI** | Covered by an automated unit test that passed in CI on both `windows-latest` and `ubuntu-24.04`. |
| **UNVERIFIED** | Never executed. The status comes from reading code only. |

> **The most important fact in this report:** the Windows build, which is the only build that uses the Chromium engine, **has never been launched with a GUI.** CI compiled it and ran 4 unit tests. Every Windows-specific runtime behaviour in this report is **UNVERIFIED**: WebView2 rendering, tab freezing, memory targets, extensions, game-mode priority changes, downloads, DevTools and popups.

---

## 1. Executive summary

- Rustium is a **browser shell**: about 3,200 lines of Rust, HTML and JavaScript. It wraps the operating system's web view through the `wry` and `tao` libraries.
  - **Windows:** pages render in **Microsoft WebView2**, which is Chromium (Blink for layout, V8 for JavaScript).
  - **Linux:** pages render in **WebKitGTK** (WebKit for layout, JavaScriptCore for JavaScript). That is **not** Chromium.
  - **macOS:** the code paths exist, but the project has never been compiled for macOS.
- The browser's own interface (tab strip, toolbar, settings panels) is **an HTML page in a separate web view**. It talks to Rust through JSON messages.
- **Solid core:** tabs, the address bar, an encrypted password vault with autofill and save prompts, Chrome CSV password import, Chrome bookmark import, history, session restore with lazy-loaded tabs, two-stage tab sleeping, and a "game mode" that changes CPU priority.
- **Major gaps compared with Chrome:**
  - No incognito mode, no profiles, no sync.
  - No tab groups, no pinned tabs, no dragging tabs, no multiple windows.
  - No bookmark folders in the UI, no bookmarks bar, no bookmark export.
  - No find bar or print menu of its own (WebView2 may provide these through built-in shortcuts).
  - No site permissions UI.
  - No crash recovery.
  - No full-screen video handling.
  - Extensions: no store, and no toolbar or popup UI for them.
  - No Safe Browsing (it is deliberately switched off).
  - No auto-update.
- **Security issues found:**
  - Every website is silently granted clipboard **read** permission on Windows.
  - Background pages can send browser commands, including closing or switching the user's active tab.
  - Autofilled passwords can be read by any script on the page.
  - Native popup windows have no address bar.
  - Phishing and malware protection (SmartScreen) is turned off.
- **Confirmed bugs:**
  - The tab strip falls apart when many tabs are open (live-tested with 40 tabs).
  - Saving settings can overwrite newer settings changes.
  - With game mode on at startup, browser processes stay at low priority even while you use the browser.
  - A homepage typed without `https://` is never fixed up and produces a blank tab.
  - Running two copies of the browser at once can lose data.
- **Can this architecture become a complete Chrome-like browser?**
  - Most of the *shell* features listed above can be added without a rewrite.
  - Full Chrome parity cannot: full Chrome-extension compatibility, Chrome Web Store installs, Google account sync, Safe Browsing parity, deep process control and cross-platform Chromium would all require replacing the engine layer, for example with CEF (Chromium Embedded Framework) or a Chromium fork. See §20.

---

## 2. Languages, frameworks and dependencies

### 2.1 Languages and lines of code

| File | Language | Lines | Role |
|---|---|---|---|
| `src/main.rs` | Rust | 1,449 | App state, window, tabs, event loop, message handling, tab sleeping, game mode, downloads |
| `src/vault.rs` | Rust | 402 | Encrypted password vault, CSV import and export, DPAPI key protection, 2 tests |
| `src/storage.rs` | Rust | 281 | Settings, bookmarks, history and session saved as JSON files |
| `src/perf.rs` | Rust | 168 | Engine flags, memory accounting, process priority and Efficiency Mode |
| `src/import.rs` | Rust | 139 | Chrome / Edge / Brave bookmark import, 1 test |
| `src/ui/chrome.html` | HTML / CSS / JS | 452 | Browser interface: tab strip, toolbar, infobar, full-window panels |
| `src/ui/content.js` | JavaScript | 147 | Injected into every page: shortcuts, address tracking, media detection, password autofill and capture |
| `.github/workflows/build.yml` | YAML | 68 | CI for Windows and Linux, test, release build, artifact upload, GitHub Release publishing |

Both HTML/JS files are compiled into the executable with `include_str!` (`src/main.rs:25-26`). There is no separate frontend build step and no npm.

### 2.2 Rust dependencies (direct, versions from `Cargo.lock`)

| Crate | Version | Purpose | Platforms |
|---|---|---|---|
| `wry` | 0.57.3 (feature `devtools`) | Cross-platform web view: WebView2, WebKitGTK, WKWebView | all |
| `tao` | 0.37.3 | Window creation and event loop | all |
| `serde`, `serde_json` | 1.0.229, 1.0.154 | Saving data as JSON, UI messages | all |
| `dirs` | 6.0.0 | Profile, Downloads and Chrome profile paths | all |
| `url` | 2.5.9 | Origin parsing, address-bar text encoding | all |
| `chacha20poly1305` | 0.10.1 | Vault encryption | all |
| `rand` | 0.8.8 | Key and nonce generation | all |
| `csv` | 1.4.0 | Password CSV import and export | all |
| `sysinfo` | 0.40.4 (feature `system` only) | Listing processes for memory accounting and priority changes | all |
| `webview2-com` | 0.39.1 | Raw WebView2 COM calls (`TrySuspend`) | Windows |
| `windows` | 0.62.2 | Win32 calls: DPAPI, priority, Efficiency Mode, process memory, foreground window, MessageBox | Windows |
| `gtk` | 0.18.2 | GTK container for multiple web views | Linux |

- 288 packages in the lock file in total. `dirs` appears twice, at 6.0.0 and at 7.0.0 (pulled in by another dependency). That is harmless.
- **Removed during development:** `rfd` (native file dialogs) conflicted with wry's GTK version. It was replaced by an HTML `<input type=file>`.
- Release build settings (`Cargo.toml`): `opt-level=3`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`.
- The release binary is 1.58 MB on Linux and 1.32 MB on Windows.

### 2.3 Build and CI (`.github/workflows/build.yml`)

- **Windows job:** `cargo test --locked`, then `cargo build --release --locked`, then upload `rustium.exe` as an artifact, then read the version from `Cargo.toml` and publish a GitHub Release with `softprops/action-gh-release@v2`.
  - It publishes on `v*` tags, on pushes to `main` or the development branch, and on manual runs.
  - Tags are created by the workflow, because this development environment cannot push tags.
- **Linux job:** installs `libwebkit2gtk-4.1-dev` and `libgtk-3-dev`, then runs tests and a release build.
- **Status:** run #3 passed every step on both jobs, and Release `v0.1.0` exists. Run #1 failed because the DevTools functions only exist in release builds when wry's `devtools` feature is on; fixed in `0882c93`.
- CI has no GUI tests, no linting (`clippy`) and no formatting check.
- The executable is **not code-signed**, so Windows SmartScreen will warn. There is no installer and no auto-updater.

---

## 3. Rendering engine and JavaScript engine

| Platform | Engine | JavaScript engine | How it's embedded | Evidence |
|---|---|---|---|---|
| Windows 10/11 | Microsoft **WebView2** runtime (evergreen Chromium, updated by Windows Update / Edge) | **V8** | `wry` → WebView2 COM. Each tab is an `ICoreWebView2Controller` hosted as a child window (`build_as_child`, `src/main.rs:97-100`) | `engine_version()` `src/main.rs:208-216`; WebView2-only code `src/main.rs:1156-1180, 1224-1255` |
| Linux | **WebKitGTK** 4.1 (2.52.6 in the test container) | **JavaScriptCore** | `wry` → `webkit2gtk`, placed inside a `gtk::Fixed` container (`src/main.rs:91-96, 1311-1320`) | The Performance panel showed "WebKitGTK 2.52.6" (T-LINUX) |
| macOS | WKWebView (WebKit) in theory | JavaScriptCore | `build_as_child` | **Never compiled or run** |

What this means:

- The project **does not ship or control** its rendering engine. The engine version is whatever WebView2 runtime is installed on the PC.
- Browser engine flags are passed through `with_additional_browser_args` (`src/perf.rs:9-27`). They are fixed when the browser starts.
- **The user agent is WebView2's default**, which identifies itself as Edge ("Edg/…"), not Chrome. Rustium never changes it.
- On Linux the browser is effectively a WebKit browser. Chromium-specific compatibility claims do not apply there.

---

## 4. Architecture and process model

### 4.1 Diagram

```
┌───────────────────────────── rustium.exe (one process, one UI thread) ─────────────────────────────┐
│ tao event loop (src/main.rs:1393)                                                                   │
│   ├─ UserEvent queue ←── IPC / title / page-load / new-window / download callbacks (EventLoopProxy) │
│   ├─ wakes every 2 s: check_foreground() + tick()  (tab sleeping, memory sample, save to disk)       │
│   └─ App state: Vec<Tab>, active index, Store (JSON), Vault (decrypted, in memory), downloads list   │
│                                                                                                     │
│  Native top-level window (tao)                                                                      │
│   ├─ UI web view (HTML: src/ui/chrome.html) — y=0, height 76px (grows for infobar / full-window)    │
│   │     window.ipc.postMessage(JSON) ──► on_ui()      Rust ──► evaluate_script("R.state(...)")       │
│   └─ Content web views, one per *loaded* tab, stacked at y=ui_height; only the active one visible    │
│         content.js injected (main frame only) ──► on_page(tab, source URL, JSON)                    │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘
          │ (Windows) all web views share one WebView2 profile folder: %LOCALAPPDATA%\Rustium\Engine
          ▼
  msedgewebview2.exe browser process ─┬─ GPU process
                                      ├─ utility / network processes
                                      └─ renderer processes (sandboxed by Chromium; --process-per-site,
                                         --renderer-process-limit=N)
```

### 4.2 Key facts

- **Single-threaded shell.** All state lives in `App` (`src/main.rs:218-243`) and is changed only on the event-loop thread. Callbacks from web views are turned into `UserEvent`s (`src/main.rs:30-55`) through `EventLoopProxy`. There are no locks.
- **The UI is itself a web view.**
  - It is built with `with_html` (on Windows wry calls `NavigateToString`).
  - It is the only web view that has the extensions path set, and it uses a navigation handler that blocks any `http…` or `file…` address so the privileged UI can never be steered to a website (`src/main.rs:254-275`).
  - Rust pushes the entire UI state as JSON with `evaluate_script("window.R&&R.state({...})")` (`src/main.rs:507-537`).
  - The UI sends commands back as `{cmd:…}` JSON (`src/main.rs:855-1083`).
- **One web view per tab, created only when needed.** `Tab.view: Option<WebView>`, where `None` means sleeping or not yet loaded (`src/main.rs:57-67`).
  - A web view is created when a tab is first activated (`activate`, `src/main.rs:396-400`).
  - It is destroyed on sleep (`discard`, `src/main.rs:471-483`) or close (`src/main.rs:438`).
- **Layout.** Only the active tab is shown (`set_visible`). `Host::place` positions web views and adds a GTK workaround on Linux (`src/main.rs:104-116`). When a full-window panel opens, the UI web view is stretched to the whole window and the active tab is hidden (`src/main.rs:494-505`).
- **Engine process model (Windows).**
  - All web views share one profile folder (`WebContext::new(Some(engine_dir))`, `src/main.rs:1328`).
  - The engine flags are identical for every web view, as WebView2 requires.
  - wry creates a WebView2 environment per web view. With the same folder and options, they should share one engine browser process (UNVERIFIED).
  - Renderer sandboxing is Chromium's own; Rustium adds nothing to it.
- **Profile folder** (`profile_dir()`, `src/main.rs:201-206`): `%LOCALAPPDATA%\Rustium\` on Windows, `$XDG_DATA_HOME/Rustium` on Linux.
  - Files: `settings.json`, `bookmarks.json`, `history.json`, `session.json`, `passwords.vault`, `vault.key`, `Engine\`, `Extensions\`.
- **Linux specifics.** `GDK_BACKEND=x11` is forced, because child web views do not work on native Wayland (`src/main.rs:1295-1301`). GTK containers undo wry's resizing, hence the `Host::place` workaround (T-LINUX bug, fixed).
- **No logging, no crash reporting.**
  - Release builds use `windows_subsystem = "windows"` (`src/main.rs:9`), so every `eprintln!` is lost.
  - Fatal startup errors show a MessageBox (`src/main.rs:1257-1271`).

---

## 5. Feature inventory: shell and navigation

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 5.1 | Main window with native decorations, 1280×820, minimum 420×300 | FULL | T-LINUX | `src/main.rs:1304-1309` |
| 5.2 | Address bar: URL or search detection | FULL | T-CI (`tests::omnibox`) + T-LINUX | `to_url` `src/main.rs:153-199`. Blocks `javascript:` and `data:` (treated as searches). Allows `http`, `https`, `file`, `about:` and `view-source:`. Adds `https://` for dotted hosts and `http://` for localhost and IPv4. **Gaps:** no IPv6, no public-suffix list (`email@host.com` becomes a URL), no `chrome://` pages |
| 5.3 | Address bar suggestions | PARTIAL | UNVERIFIED in UI | `Store::suggest` `src/storage.rs:246` (bookmarks first, then history by visit count, up to 8), shown through an HTML `<datalist>` (`src/ui/chrome.html:114, 427`). No inline autocomplete, no search-engine suggestions, no keyboard-highlighted results like Chrome's dropdown. Whether the datalist popup draws correctly in WebView2 is unverified |
| 5.4 | Search engine choice (Google, Bing, DuckDuckGo, Brave, Startpage) | FULL | UNVERIFIED | Settings panel `src/ui/chrome.html:342+`; template `{}` |
| 5.5 | Back, forward, reload, home buttons and their enabled state | FULL | T-LINUX (buttons drawn; back/forward disabled correctly) | `src/main.rs:689-708`; `can_go_back`/`can_go_forward` in `push_state` |
| 5.6 | Stop loading | MISSING | — | No stop command; the reload button never becomes a stop button |
| 5.7 | Keyboard shortcuts in pages | FULL in code | UNVERIFIED on Windows | `src/ui/content.js:10-31`: Ctrl+T, Ctrl+W/Ctrl+F4, Ctrl+Shift+T, Ctrl+L/Alt+D/F6, Ctrl+Tab/Ctrl+Shift+Tab/Ctrl+PgUp/Ctrl+PgDn, Ctrl+1…9, Ctrl+D, Ctrl+H, Ctrl+J, Ctrl+Shift+O, Alt+←/→, Alt+Home, F5/Ctrl+R, F12/Ctrl+Shift+I, Ctrl+Shift+G. They only work after the page script has run, and pages can override them. **Risk:** WebView2's own shortcuts (F5, F12) may also fire, giving a double reload or DevTools opening and immediately closing (§16) |
| 5.8 | Keyboard shortcuts while the toolbar has focus | PARTIAL | UNVERIFIED | `src/ui/chrome.html` keydown handler: a subset of the above |
| 5.9 | Mouse back/forward buttons | FULL in code | UNVERIFIED | `src/ui/content.js:35-38`. Could navigate twice if WebView2 also handles these buttons |
| 5.10 | Zoom (Ctrl+±, Ctrl+wheel) | PLATFORM | UNVERIFIED | `with_hotkeys_zoom(true)` `src/main.rs:291`. No zoom indicator, no reset button, no per-site zoom memory |
| 5.11 | Find in page | PLATFORM / MISSING | UNVERIFIED | Not implemented. WebView2 browser shortcuts are enabled for tabs (`src/main.rs:1168`); WebView2's documentation lists Ctrl+F among them. Nothing on Linux |
| 5.12 | Print | PLATFORM | UNVERIFIED | Only through WebView2's Ctrl+P. No menu item |
| 5.13 | Right-click menu | PLATFORM | UNVERIFIED | WebView2 / WebKit default menu. Nothing custom ("Open link in new tab", "Search Google for…" and similar are missing) |
| 5.14 | Pages opened with `target=_blank` or `window.open` | FULL | T-LINUX (a `target=_blank` link opened a new tab) | `src/main.rs:309-318`. Requests that specify a window size are allowed as **native popups with no Rustium toolbar**; everything else becomes a new tab. Popups are not tracked as tabs |
| 5.15 | Opening a URL from the command line | FULL | UNVERIFIED | `std::env::args().nth(1)` `src/main.rs:1359-1361`. Not registered as a protocol or file handler; cannot be the default browser |
| 5.16 | Single instance / opening links in the running browser | MISSING | — | No lock or IPC between instances. Two instances share and overwrite the same JSON files and vault (§18 B7) |
| 5.17 | Full-screen video (e.g. YouTube) | MISSING / likely BROKEN | UNVERIFIED | Neither Rustium nor wry handles WebView2's full-screen event (`grep FullScreen` in the wry source finds nothing). Expect full-screen video to fill only the tab area, with the toolbar still visible and the window not going full screen |
| 5.18 | Crashed-tab recovery | MISSING | — | WebView2's process-failure event is not handled (also not in wry). A crashed renderer leaves a dead tab with no "reload" message |
| 5.19 | Favicons | PARTIAL | T-LINUX (fallback letter shown) | `src/ui/chrome.html:157` loads `<origin>/favicon.ico` directly. Ignores `<link rel=icon>`. The fallback code is broken for titles starting with `'` or `\`. The whole tab strip is rebuilt with `innerHTML` on every state update, which re-creates the image elements |
| 5.20 | Loading indicator | FULL | T-LINUX | `loading` flag from page-load events `src/main.rs:1097-1112`; spinning icon in CSS |
| 5.21 | Security indicator (padlock, certificate viewer) | MISSING | — | The address bar shows the raw URL only |
| 5.22 | Internal pages (`chrome://settings`, `chrome://flags` and so on) | MISSING | — | Settings are an HTML panel instead |

---

## 6. Tabs, tab groups and session restore

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 6.1 | Open, close and switch tabs (click, middle-click close, double-click empty strip for a new tab) | FULL | T-LINUX | `new_tab` `src/main.rs:359`, `activate` `:382`, `close_tab` `:421`; UI `src/ui/chrome.html` mousedown handler |
| 6.2 | Closing the last tab quits the browser (like Chrome) | FULL | UNVERIFIED | `src/main.rs:425-430`; the session still contains that tab, so it comes back next launch |
| 6.3 | Reopen closed tab (Ctrl+Shift+T) | PARTIAL | UNVERIFIED | `closed: Vec<String>`, up to 25 entries, `src/main.rs:432-437, 656-660`. Restores the URL only (no back/forward history, no position); lost on restart; no "recently closed" menu |
| 6.4 | Tab strip with many tabs | **BROKEN** | **T-LINUX (confirmed 2026-11-26, 40 tabs)** | CSS `src/ui/chrome.html:23-24` (`.tab{flex:0 1 220px;min-width:36px}`, no overflow handling). With 40 tabs, tabs overlap, close buttons and sleep badges cover the icons, tabs run past the "+" button and off the window, and there is no scrolling. Tabs out of view can only be reached by keyboard |
| 6.5 | Drag to reorder, drag out into a new window | MISSING | — | |
| 6.6 | Pinned tabs | MISSING | — | |
| 6.7 | Tab groups | MISSING | — | No data model for groups |
| 6.8 | Multiple windows | MISSING | — | One `Host` / window per process |
| 6.9 | Tab search, tab hover preview | MISSING | — | Only a native tooltip with title, URL and sleep state |
| 6.10 | Mute tab / speaker icon | PARTIAL | UNVERIFIED | 🔊 badge from `content.js` media detection (`src/ui/content.js:56-61`). No mute control. Web Audio, iframes and muted video aren't detected |
| 6.11 | Session restore | FULL | **T-LINUX** (restart restored the tab and active index) | `session.json` holds `tabs`, `titles` and `active` (`src/storage.rs:81-86`). Saved on tab open/close and page load (`src/main.rs:485-490`). Restored at startup (`src/main.rs:1357-1388`), controlled by the `restore_session` setting |
| 6.12 | Lazy session restore (only the active tab loads) | FULL | T-LINUX (background restored tab showed 💤) | Restored tabs start with `view: None` |
| 6.13 | Restoring back/forward history, scroll position and form contents per tab | MISSING | — | URL and title only |
| 6.14 | Crash-safe saving | PARTIAL | code review | Writes use temp file + rename (`write_atomic`, `src/storage.rs:108`), but saving happens only every 2 seconds and at quit. A crash loses up to 2 s of changes. `panic="abort"` skips the quit path |

---

## 7. Bookmarks, folders, import and export

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 7.1 | Add or remove a bookmark (star / Ctrl+D) | FULL | UNVERIFIED | `Store::toggle_bookmark` `src/storage.rs:210`; `src/main.rs:726-741`. Star reflects an exact URL match only |
| 7.2 | Storage | FULL | code review | Flat `Vec<Bookmark{title,url,folder,added}>` in `bookmarks.json` |
| 7.3 | Folders | **PARTIAL (display only)** | code review | `folder` is a plain text path (e.g. `"Bookmarks bar/Games"`), set **only** by import. The panel shows it as grey text. You cannot create, rename, move between or browse folders; there is no tree |
| 7.4 | Edit a bookmark (title or URL) | MISSING | — | |
| 7.5 | Delete a bookmark | PARTIAL / race bug | UNVERIFIED | Deleted **by list position** (`bm_delete`, `src/main.rs:983-991`; UI `src/ui/chrome.html:397`). If the list changes between rendering and clicking, the wrong bookmark is deleted |
| 7.6 | Bookmarks bar under the toolbar | MISSING | — | |
| 7.7 | Bookmark manager (search, sort, drag) | MISSING | — | The panel is a plain list with no search |
| 7.8 | Import from Chrome, Edge, Brave and Chromium | FULL (parser) / UNVERIFIED (finding the files) | T-CI (`import::tests::parses_chrome_format`) | `src/import.rs`. Looks in Chrome/Edge/Brave `User Data` folders on Windows, macOS and Linux (`:13-35`); `Default` and `Profile N` folders (`:38-58`); reads `roots.bookmark_bar/other/synced` (`:100`); keeps only http(s); skips URLs already bookmarked. Never run against a real installed Chrome profile |
| 7.9 | Import from Firefox or an HTML bookmarks file | MISSING | — | |
| 7.10 | Bookmark export (HTML/JSON) | MISSING | — | |

---

## 8. History and downloads

### 8.1 History

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 8.1.1 | Recording visits | FULL | T-LINUX (`history.json` written) | `Store::record_visit` `src/storage.rs:163`; called on page load and on in-page address changes (`content.js` hooks `pushState`/`replaceState`/`popstate`/`hashchange`, `src/ui/content.js:40-53`). Only http(s) |
| 8.1.2 | Data model | PARTIAL | code review | **One entry per URL** (last visit time and visit count). No per-visit timeline, no grouping by day, no "journeys", no referrers |
| 8.1.3 | Limit | FULL | code review | `MAX_HISTORY = 5000` (`src/storage.rs:8`); oldest are dropped |
| 8.1.4 | History panel with search | FULL | UNVERIFIED | `hist_list` `src/main.rs:1006-1021` (newest first, top 500, text match) |
| 8.1.5 | Clear all history | FULL | UNVERIFIED | `src/main.rs:1022-1026` |
| 8.1.6 | Delete a single entry, or a time range | MISSING | — | |
| 8.1.7 | Performance | PARTIAL | code review | The whole `history.json` (up to 5,000 entries) is rewritten on every 2-second flush while it has changes. Fine at this size, but it isn't a database |

### 8.2 Downloads

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 8.2.1 | Saving into the Downloads folder without the built-in WebView2 download UI | FULL in code | UNVERIFIED | `src/main.rs:319-339`. wry sets WebView2's result file path and marks the download handled, which hides WebView2's own download bubble (wry `webview2/mod.rs:955-970`). File names are cleaned of `\/:*?"<>\|` and control characters; duplicates get " (n)" (`unique_path`, `src/main.rs:1182-1204`) |
| 8.2.2 | Notices when a download starts and finishes | FULL | UNVERIFIED | `src/main.rs:1114-1149` |
| 8.2.3 | Downloads panel ("Show in folder") | PARTIAL | UNVERIFIED | `src/ui/chrome.html:320`; `show_in_folder` `src/main.rs:1206-1221` |
| 8.2.4 | Progress, speed, pause/resume, cancel, retry, open file | MISSING | — | No progress events are wired up |
| 8.2.5 | Download list survives restart | MISSING | — | `App.downloads` lives in memory only |
| 8.2.6 | Ask where to save | MISSING | — | Always the Downloads folder |
| 8.2.7 | Dangerous-file warnings / SmartScreen | MISSING | — | Turned off (§15) |
| 8.2.8 | Matching the finish event to the right download | PARTIAL | code review | Matched by URL (`src/main.rs:1130-1135`). Two downloads of the same URL can be confused. Two simultaneous downloads with the same name can pick the same "unique" path, because uniqueness is checked before either file exists |

---

## 9. Password management and autofill

### 9.1 Storage and encryption (`src/vault.rs`)

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 9.1.1 | Encrypted vault | FULL | **T-CI** (roundtrip test, checks there's no plaintext on disk) + **T-LINUX** (the saved password wasn't in the file) | ChaCha20-Poly1305 with a random 96-bit nonce per write. File = nonce ‖ ciphertext of JSON `Vec<Credential>` (`src/vault.rs:60-102`) |
| 9.1.2 | Key protection on Windows | FULL | **T-CI on windows-latest** (the vault tests passed, so the DPAPI encrypt/decrypt roundtrip works) | `vault.key` holds the 32-byte key protected by DPAPI for the current Windows user (`CryptProtectData`, `src/vault.rs:299-336`). This protects against someone copying the files to another account or machine; **it does not protect against malware running as the same user.** That matches Chrome before app-bound encryption |
| 9.1.3 | Key protection on Linux/macOS | **PARTIAL / weak** | code review | The key file is stored **unencrypted** with permission `0600` (`src/vault.rs:284, 339-345`), right next to the vault. The encryption adds almost nothing against anyone who can read the folder. No Secret Service or Keychain integration |
| 9.1.4 | Corrupt vault | FULL (fails safe) | code review | If decryption fails, `vault=None` and a "vault unavailable" notice is shown. Nothing is overwritten |
| 9.1.5 | Matching rules | FULL | T-CI | Same scheme, host and port; a leading `www.` is ignored; **no subdomain matching** (deliberate: there's no public-suffix list) (`site_key`, `src/vault.rs:55`). Tests confirm that `evil-github.com` and `http://` versions don't match |

### 9.2 Import and export

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 9.2.1 | CSV import (Chrome, Edge, Brave, Firefox, Bitwarden-style headers) | FULL (parser) / UNVERIFIED (UI) | T-CI | `import_csv` `src/vault.rs:185-250`. Recognises column-name variants, strips the byte-order mark, handles quoting, skips `android://` and blank rows, updates changed passwords, returns added/updated/skipped counts. The UI reads the file with an HTML file picker and sends its full text over IPC (`src/ui/chrome.html:406-412`; 20 MB cap) |
| 9.2.2 | CSV export | FULL | UNVERIFIED | `export_csv` `src/vault.rs:252` (Chrome's format), written as plaintext into Downloads after a JavaScript `confirm()` (`src/main.rs:950-962`) |
| 9.2.3 | Reading Chrome's saved passwords directly | MISSING (blocked) | — | Chrome 127+ app-bound encryption prevents it by design (documented in the README) |

### 9.3 Autofill and saving (`src/ui/content.js:63-146`, `src/main.rs:779-835`)

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 9.3.1 | Offer to save on login | FULL | **T-LINUX** (Enter-submit showed "Save password for gamer42 on localhost:8765?"; Save stored it) | Triggered by form submit, Enter in a password field, or **any button click while a password field is visible** (`content.js:129-133`). Rust checks whether the login is new, changed or the same, and shows an infobar: Save / Never for this site / Not now (`src/main.rs:811-835`; `src/ui/chrome.html:435`) |
| 9.3.2 | Successful-login detection | MISSING | — | The prompt appears even for wrong passwords, or when clicking a "show password" eye button. Chrome waits for a successful navigation |
| 9.3.3 | Autofill on page load | FULL | **T-LINUX** (after a restart, username and password filled with a blue tint) | `content.js` finds the fields (a password input plus the nearest text/email field before it, or common name/id patterns) and asks Rust once per page or address change. Rust checks the origin is https or localhost and sends all saved logins for that origin. The injected fill script re-checks `window.location.origin` before filling (`src/main.rs:799-808`). React-style inputs are handled with the native value setter plus `input`/`change` events |
| 9.3.4 | Two-step logins (username page, then password page) | PARTIAL | UNVERIFIED | The typed username is remembered in the **site's own** `sessionStorage` key `__rs_user` (`content.js:91-97`) |
| 9.3.5 | Choosing between multiple accounts | MISSING | — | Picks a login matching the typed or remembered username, otherwise the first. No dropdown |
| 9.3.6 | Login forms in iframes or shadow DOM | MISSING | — | The script runs in the main frame only (`with_initialization_script_for_main_only(...,true)`, `src/main.rs:288`); `querySelectorAll` doesn't search shadow roots |
| 9.3.7 | Hiding filled values from page scripts | **MISSING (security)** | code review | Filled values are written straight into the DOM, so **any script on the page** (third-party, ads, or injected through an XSS hole) can read them immediately. Chrome hides autofilled values from JavaScript until the user interacts with the page |
| 9.3.8 | Password manager panel (search, show, copy, delete) | FULL | **T-LINUX** (listing shown) | `src/ui/chrome.html:278-299`; show/copy through `pw_reveal` (`src/main.rs:922-927`) |
| 9.3.9 | Asking for Windows Hello / PIN before showing a password | MISSING | — | Show and Copy work with no re-authentication |
| 9.3.10 | Password generator, leaked-password check, weak/reused audit, passkeys/WebAuthn manager | MISSING | — | WebAuthn itself may work through the engine (UNVERIFIED) |
| 9.3.11 | Addresses and payment-card autofill | MISSING (Rustium) | — | `with_general_autofill_enabled` is not used. WebView2 has its own form autofill, default unknown (UNVERIFIED) |
| 9.3.12 | "Never save" list | PARTIAL | code review | Exact origin match (`src/main.rs:819`): `www.` and non-`www` are treated as different. No UI to view or remove entries. **Can be silently erased by the settings race (B2)** |

---

## 10. Profiles, sync and incognito

| # | Feature | Status | Verified | Evidence |
|---|---|---|---|---|
| 10.1 | Multiple user profiles | MISSING | — | One hard-coded profile folder (`src/main.rs:201-206`). wry does offer `with_profile_name` (WebView2 multi-profile), so this is buildable |
| 10.2 | Incognito / private window | MISSING | — | Nothing in the code. wry offers `with_incognito(true)` (WebView2 InPrivate); a separate window or profile would be needed |
| 10.3 | Sync (Google account or other) | MISSING | — | Chrome Sync is not available to third-party apps. Would need a self-built service (end-to-end encrypted) or a file-sync approach |
| 10.4 | Signing in to the browser | MISSING | — | Google sign-in to websites may also be refused in embedded engines (UNVERIFIED) |
| 10.5 | Guest mode | MISSING | — | |

---

## 11. Extensions

| # | Item | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 11.1 | Loading unpacked Chrome extensions (Windows) | PARTIAL | UNVERIFIED | `with_browser_extensions_enabled(true)` on every web view; `with_extensions_path(%LOCALAPPDATA%\Rustium\Extensions)` on the UI web view only (`src/main.rs:1165-1172`). wry calls WebView2 `AddBrowserExtension` for **every entry** in that folder each time the browser starts (wry `webview2/mod.rs:1525-1541`) |
| 11.2 | Supported extension APIs | PLATFORM-defined, unknown | UNVERIFIED | Rustium provides **no** `chrome.*` APIs. Support is whatever WebView2's extension feature offers, which is a subset of Chrome's: content scripts and network-rule extensions (ad blockers) are the usual success cases. **The toolbar button and popup page (browser action) have no UI** in a WebView2 host, so popup-based extensions (password managers, many utilities) can't be used. The `tabs`, `windows`, `bookmarks`, `history` and `downloads` APIs would refer to WebView2's idea of tabs, not Rustium's |
| 11.3 | Chrome Web Store installs, `.crx` files, auto-update | MISSING | — | |
| 11.4 | Extension management (list, enable/disable, remove, permissions, options page) | MISSING | — | Deleting a folder doesn't remove an extension already registered in the WebView2 profile (UNVERIFIED) |
| 11.5 | Startup risk from the Extensions folder | **POSSIBLE BUG** | UNVERIFIED | wry turns any synchronous `AddBrowserExtension` error into a web-view build error, which reaches `fatal()` (`src/main.rs:271-274`). If a stray file (`.crx`, `.zip`, `desktop.ini`) in the folder fails synchronously, **the browser may refuse to start.** Needs testing on Windows |
| 11.6 | Linux and macOS | MISSING | — | WebKitGTK and WKWebView don't support Chrome extensions |

---

## 12. Developer tools

| # | Feature | Status | Verified | Evidence |
|---|---|---|---|---|
| 12.1 | DevTools for pages (F12 / Ctrl+Shift+I) | FULL in code | UNVERIFIED (compile fixed in `0882c93`) | `with_devtools(true)` `src/main.rs:289`; toggle `src/main.rs:709-717`. WebView2 opens the Edge DevTools window; Linux opens the WebKit inspector. **Possible double-trigger:** WebView2's own F12 shortcut plus the JavaScript handler can open and then immediately close it (§16) |
| 12.2 | DevTools for the browser UI | Debug builds only | — | `with_devtools(cfg!(debug_assertions))` `src/main.rs:262` |
| 12.3 | View source | PARTIAL | UNVERIFIED | Typing `view-source:` is allowed (`src/main.rs:161`); no menu item |
| 12.4 | Task manager (per tab / per process) | MISSING | — | Only a total memory figure (§17) |

---

## 13. Settings

| # | Feature | Status | Verified | Evidence |
|---|---|---|---|---|
| 13.1 | Settings panel: homepage, search engine, restore session, offer to save passwords, freeze seconds, unload minutes, max loaded tabs, renderer limit, GPU boost, game mode, clear data, clear history, about | FULL | UNVERIFIED (panel drew in T-LINUX only for Performance and Passwords) | `src/storage.rs:19-61`; `src/ui/chrome.html:342-373`; `settings_set` `src/main.rs:1046-1058` |
| 13.2 | Saving settings | **BROKEN (race)** | code review | The UI sends back its **entire** cached settings object (`src/ui/chrome.html:400-404`) and Rust replaces the whole thing. Anything that changed since the panel loaded (game mode toggled from the toolbar, a "Never save" added) is **reverted** (B2) |
| 13.3 | Homepage check | **BROKEN** | code review | The homepage is used as-is (`new_tab(&home)`, `src/main.rs:649-650, 706-707`). A value like `example.com` (no scheme) is never passed through `to_url`, so loading fails and the tab stays blank (B4) |
| 13.4 | Engine flags (GPU boost, renderer limit) apply after restart | FULL | code review | `engine_args` is computed once (`src/main.rs:1337`) |
| 13.5 | Clear cookies, cache and site data | FULL in code | UNVERIFIED | `clear_all_browsing_data()` on one web view clears the whole shared profile (`src/main.rs:1027-1032`). No time range, no per-site option, no cookie viewer |
| 13.6 | Site settings (camera, microphone, location, notifications, pop-ups, JavaScript, cookies per site) | MISSING | — | No permission handler, so WebView2 / WebKit **default prompts** are used (UNVERIFIED). Clipboard read is auto-allowed (§15) |
| 13.7 | Theme | MISSING | — | Hard-coded dark toolbar; pages follow the OS colour scheme |

---

## 14. Website compatibility

| Area | Assessment | Verified |
|---|---|---|
| General rendering (Windows) | Same Blink/V8 as Edge, which is very close to Chrome for standard sites | UNVERIFIED (never launched on Windows) |
| General rendering (Linux) | WebKit: Safari-class compatibility, **not** Chromium; some sites behave differently | T-LINUX for a local test page only. External sites were not loaded because of the container's network restrictions |
| User agent | Edge's (WebView2 default). Sites that sniff for "Chrome" usually accept Edge | code review |
| Google sign-in | Google is known to block sign-in from embedded web views ("This browser or app may not be secure") | UNVERIFIED (documented in the README as a risk) |
| OAuth popups ("Sign in with…") | Kept as native popups so `window.opener` works (`src/main.rs:312-314`) | UNVERIFIED |
| DRM video (Netflix, Disney+, Spotify) | Depends on what DRM the WebView2 runtime supports; uncertain | UNVERIFIED (high priority to test) |
| Autoplay | `--autoplay-policy=user-gesture-required` (`src/perf.rs:17`) is **stricter than Chrome's default** and may block autoplay on video sites | UNVERIFIED |
| Full-screen video | Not handled (5.17) | UNVERIFIED, likely broken |
| Web notifications, push, installable web apps, file system access, WebUSB/HID/Serial, WebXR | No host-side handling; whatever WebView2 provides by default | UNVERIFIED |
| `--enable-features=CanvasOopRasterization` | Chromium may honour only the **last** `--enable-features` switch; if WebView2 adds its own, one would override the other | UNVERIFIED risk |
| History-API patching | `content.js` replaces `history.pushState`/`replaceState` on every page (`src/ui/content.js:48-51`) — visible to site scripts, could break rare sites | code review |

---

## 15. Security, sandboxing and permissions

### 15.1 What is sound

- Chromium's renderer sandbox and site isolation are inherited from WebView2 (Windows).
- The UI web view can't be navigated to any web page (navigation handler, `src/main.rs:265`), and new windows from it are blocked (`:266`).
- The address bar rejects `javascript:` and `data:` (`src/main.rs:159`).
- Autofill:
  - Only on https or localhost.
  - Rust takes the origin from the URL the browser reports for the page, not from the page's message.
  - The fill script re-checks `window.location.origin`, which pages cannot fake.
  - The fill hooks are defined as non-configurable, so pages can't redefine them.
  - Exact-origin matching.
- Download file names are cleaned, so HTML can't be injected into the UI through a download name.
- The vault uses authenticated encryption; on Windows the key is protected by DPAPI.

### 15.2 Findings (ordered by severity)

| ID | Severity | Finding | Evidence |
|---|---|---|---|
| S1 | **High** | **Every website gets clipboard read access on Windows without a prompt.** Tabs set `with_clipboard(true)` (`src/main.rs:290`), and wry then auto-grants clipboard read in WebView2 (wry `webview2/mod.rs:545-562`). Any page can call `navigator.clipboard.readText()`, including right after the user copies a password with Rustium's own "Copy" button | code + wry source |
| S2 | **High** | **Pages can send browser commands.** Any script on any page, including background tabs that aren't frozen yet, can post `{t:'key',k:…}`, which reaches `shortcut()` with no check of which tab sent it or whether a real key was pressed (`src/main.rs:751-754`). It can close or switch the **active** tab, go back, open DevTools, toggle game mode or open panels. Only "new tab" is rate-limited (`src/main.rs:645`). This allows UI spoofing / tab-switch tricks and annoyance attacks | code |
| S3 | **High** | **SmartScreen (phishing and malware protection) is off.** The engine flags keep wry's `msSmartScreenProtection` disable (`src/perf.rs:12`). There is no Safe Browsing equivalent and no dangerous-download warnings | code |
| S4 | Medium | **Autofilled passwords are readable by page JavaScript** (9.3.7) | code |
| S5 | Medium | **Native popups have no address bar.** Allowed popups (`src/main.rs:312-314`) open as bare WebView2 windows with no URL or security indicator, which helps phishing | code |
| S6 | Medium | **Pages can trigger save-password prompts at will** (`pw_save` from any page script), including "Update password" prompts with values the page chose, for its own origin. Needs a user click, so low impact, but enables prompt spam | code |
| S7 | Medium | **Vault key stored unencrypted on Linux/macOS** (9.1.3) | code |
| S8 | Medium | **Passwords can be shown or copied without re-authentication**; exported CSV is plaintext | code |
| S9 | Low | `--ignore-gpu-blocklist` turns on GPU features Chromium has blocklisted for driver bugs, which can cause instability or expose security bugs | `src/perf.rs:23` |
| S10 | Low | The UI uses `innerHTML` for notices (`src/ui/chrome.html:237, 245`). Current inputs are cleaned or trusted, but it's fragile: any future notice containing page-controlled text becomes UI script injection inside the privileged web view | code |
| S11 | Low | `content.js` writes `__rs_user` into each site's `sessionStorage`, so the site can see the username the user typed | `src/ui/content.js:91` |
| S12 | Info | The shell process runs with the user's full rights, unsandboxed, and keeps all decrypted passwords in memory for the whole session (no memory locking or zeroing) | design |
| S13 | Info | Unsigned executable, no auto-update. Shell security fixes need manual re-download (the engine still updates through Windows) | CI |
| S14 | Info | **Permissions:** no permission UI and no saved per-site decisions; relies on engine defaults | §13.6 |

---

## 16. Performance: RAM and CPU

### 16.1 Memory-saving features

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 16.1.1 | Lazy session restore | FULL | T-LINUX | §6.12 |
| 16.1.2 | Stage 1: freeze hidden tabs after `freeze_after_sec` (60 s; 0 s in game mode) | FULL in code (Windows) / **no-op on Linux** | UNVERIFIED | `freeze()` calls WebView2 `TrySuspend` (`src/main.rs:1241-1255`). **The result is ignored:** the tab is marked `frozen=true` even when suspend fails (e.g. playing audio) and always on Linux, so the "frozen" tooltip can be wrong |
| 16.1.3 | Lower memory target for hidden tabs | FULL in code (Windows) | UNVERIFIED | `set_memory_low` → wry `set_memory_usage_level(Low)` (`src/main.rs:1224-1236`) |
| 16.1.4 | Stage 2: unload hidden tabs after `sleep_after_min` (15 min; 3 min in game mode) | FULL | code review (Linux restored-tab case seen) | `tick()` `src/main.rs:587-605`; `discard` drops the web view and keeps the URL |
| 16.1.5 | Limit on loaded tabs (`max_awake_tabs` 8; 3 in game mode), least recently used unloaded first | FULL | UNVERIFIED | `src/main.rs:606-625` |
| 16.1.6 | Tabs playing audio are never put to sleep | PARTIAL | UNVERIFIED | Uses the `playing` flag from `content.js` (only unmuted `<audio>`/`<video>` in the main frame). Web Audio, iframe players (embedded YouTube, Spotify embeds) and WebRTC calls (Discord) are **not detected** and can be unloaded mid-call |
| 16.1.7 | Engine flags: `--process-per-site`, `--renderer-process-limit=6` | FULL in code | UNVERIFIED | `src/perf.rs:13-15`. Real effect alongside WebView2's site isolation is unmeasured |
| 16.1.8 | "Sleep all background tabs" button | FULL | UNVERIFIED | `src/main.rs:1071-1080` |
| 16.1.9 | Live RAM counter | FULL | **T-LINUX** (after fixing a thread over-count bug) | `perf::Monitor::memory` `src/perf.rs:73`. Sums this process and all its descendants. Windows uses `PrivateUsage` (commit size, `src/perf.rs:107`), which **differs from Task Manager's "Memory" column** and from Chrome's own memory figure; Linux uses resident memory, which double-counts shared pages. Assumes the WebView2 processes are children of `rustium.exe` (UNVERIFIED) |

### 16.2 Game mode

| # | Feature | Status | Verified | Evidence and notes |
|---|---|---|---|---|
| 16.2.1 | Detecting when the browser isn't the foreground window | FULL in code | UNVERIFIED | Windows: `GetForegroundWindow() == hwnd` (`src/main.rs:127-133`). Chosen deliberately because tao reports focus loss whenever focus moves into a page. Checked every 2 s and on focus events |
| 16.2.2 | Lower priority + Efficiency Mode for every browser process when in background | FULL in code (Windows) / no-op elsewhere | UNVERIFIED | `Monitor::set_background` → `SetPriorityClass(BELOW_NORMAL)` plus EcoQoS execution-speed throttling per process (`src/perf.rs:85-92, 130-166`). Also applies to the GPU process, which **may hurt video on a second monitor** |
| 16.2.3 | Lower priority at startup when game mode was on at exit | **BROKEN** | code review | `lower_own_priority_at_startup()` (`src/main.rs:1286-1289`; `src/perf.rs:94-101`) sets this process to below-normal so the engine inherits it. But `backgrounded` starts `false`, and `check_foreground()` only acts on *changes* (`src/main.rs:566-575`). If the browser starts in the foreground, priority is **never restored to normal** until the window first goes to the background and comes back (B3) |
| 16.2.4 | Aggressive tab sleeping in game mode | FULL | UNVERIFIED | `tick()` thresholds (`src/main.rs:582-584`) |
| 16.2.5 | Simpler UI animations in game mode | FULL | T-LINUX (game mode styling seen) | `body.game *{transition:none;animation:none}` |

### 16.3 CPU and idle behaviour

- The event loop wakes **every 2 seconds** (`src/main.rs:1390-1400`) even when idle: a foreground check plus housekeeping.
- Every 5 s (30 s in game mode) it lists **all processes on the system** with `sysinfo` to update the RAM counter. That's cheap but not free, and the descendant search walks the whole process list for each family member (`src/perf.rs:53-70`).
- Every memory sample triggers `push_state`, which **rebuilds the whole tab strip** in the UI with `innerHTML`, including favicon images: unnecessary work in the UI web view every 5 s.
- `content.js` cost per page: one throttled DOM watcher for 30 s after load, then disconnected; capture-phase listeners for keys, clicks, submits and media events; history-API wrappers. Small.
- The UI web view is a full renderer process that is always running: a fixed overhead of very roughly 30–80 MB on Windows (UNVERIFIED).

### 16.4 Measurements

| Scenario | Value | Notes |
|---|---|---|
| Linux debug build, 1 local page + UI, software rendering (Xvfb) | 486 MB → 637 MB with 2 tabs | T-LINUX. Includes WebKitGTK web, network and GPU processes. **Not representative of Windows** |
| Linux release build, 40 restored tabs (1 loaded, page not reachable) + UI | 469 MB | T-LINUX, 2026-11-26 |
| Windows | **not measured** | No comparison with Chrome exists. Claims of "really low memory" are **unverified** |
| Startup time | Linux: UI drawn after more than 7 s and within 15 s on a fresh profile in the container (software rendering) | Observed once; cause not investigated; probably the environment |
| Binary size | 1.32 MB (Windows), 1.58 MB (Linux) | The engine is not bundled |

---

## 17. Known bugs

| ID | Status | Bug | Evidence |
|---|---|---|---|
| B1 | **Confirmed (T-LINUX)** | The tab strip overlaps and overflows with many tabs; no scrolling (6.4) | screenshot test, 40 tabs |
| B2 | Confirmed by code | Saving settings sends the whole cached object, reverting newer changes (game mode, never-save list) | `src/ui/chrome.html:400-404`, `src/main.rs:1046-1058` |
| B3 | Confirmed by code | Game mode at startup leaves browser processes at below-normal priority while the browser is in use | `src/main.rs:566-575, 1286-1289` |
| B4 | Confirmed by code | A homepage without a scheme is never fixed up → blank tab | `src/main.rs:649-650, 706-707` |
| B5 | Confirmed by code | Deleting a bookmark by list position can delete the wrong one | `src/main.rs:983-991` |
| B6 | Confirmed by code | The `frozen` flag is set even when suspending failed or isn't supported | `src/main.rs:598-603, 1241-1255` |
| B7 | Confirmed by code | No single-instance lock: two instances overwrite each other's `session.json`, `history.json`, `settings.json` and **`passwords.vault`** (whoever writes last wins, so passwords saved in the other instance can be lost) | design |
| B8 | Likely (UNVERIFIED) | Full-screen video doesn't go full screen | not handled anywhere |
| B9 | Likely (UNVERIFIED) | F5, F12, Ctrl+R and mouse back/forward may fire twice (WebView2's own shortcut plus `content.js`); F12 may open and immediately close DevTools | `src/ui/content.js:28-31`, `src/main.rs:1168` |
| B10 | Possible (UNVERIFIED) | A stray file in `Extensions\` could stop the browser from starting | §11.5 |
| B11 | Confirmed by code | Favicon fallback has a JavaScript error for titles starting with `'` or `\` (cosmetic) | `src/ui/chrome.html:157` |
| B12 | Confirmed by code | Two simultaneous downloads with the same file name can be given the same target path | `unique_path` checks existence before either download starts |
| B13 | Confirmed by code | Notices take over the infobar; a new notice is suppressed while a save prompt is showing, but a save prompt arriving later replaces a notice (cosmetic) | `src/ui/chrome.html:243-252` |
| B14 | Design | Media detection misses Web Audio, iframe players and WebRTC, so calls and embedded players can be unloaded | `src/ui/content.js:56-61` |
| B15 | Design | A crashed renderer leaves a dead tab (no recovery) | §5.18 |

**Bugs found and fixed earlier in development** (useful history): a memory over-count (Linux threads summed as processes, showing 5.4 GB), GTK panel sizing (panels only drew their top 76 px), notices that stayed until clicked, and the release-build failure from the DevTools functions.

---

## 18. Architectural limitations

1. **The engine is borrowed, not owned.** WebView2 offers only a curated API. Not available, or only partly:
   - The real Chrome extension platform: action UI, Web Store, most `chrome.*` APIs tied to Rustium's own tabs.
   - Chrome Sync and Google account sign-in.
   - Safe Browsing parity.
   - Per-tab process control or custom process models.
   - Custom network-stack features (interception exists, but there's no way to build things like a full proxy chain or HTTP-level features).
   - Choosing or pinning the engine version.
   - Patching Blink or V8.
2. **Different engines per platform.** Windows uses Chromium, Linux and macOS use WebKit. Behaviour, extensions, DRM and DevTools all differ; "Chrome-like" holds only on Windows.
3. **An HTML UI in a separate web view.** It's flexible and fast to build, but:
   - It can't draw over page content (no Chrome-style address-bar dropdown, which is why `<datalist>` is used; no floating bubbles over pages).
   - Every UI change crosses the message boundary.
   - It costs a whole renderer process.
   - Anything needing overlays (permission bubbles, download shelf over content, find bar) needs either changes to the page area's size or extra top-level windows.
4. **One window, one thread, one global `App` struct.** Multiple windows, incognito windows and profiles all need `App` split into per-window and per-profile state, with shared services behind it.
5. **Persistence is whole-file JSON.** That's fine at the current scale but unsuitable for large histories, per-visit records, sync or concurrent writers. SQLite would be the natural next step.
6. **Security split.** Pages and the shell talk through one untyped JSON channel per tab, with no capability checks (S2). Any new feature added to `on_page` is reachable by every website.
7. **No test harness for the GUI.** There are 4 unit tests; nothing launches the app in CI; nothing runs on a Windows desktop.

---

## 19. Features missing compared with Google Chrome (checklist)

**Browsing UI:** multiple windows; dragging tabs; tab groups; pinned tabs; tab search; tab hover cards; working tab-strip overflow; bookmarks bar; address-bar dropdown with inline autocomplete and search suggestions; security padlock and certificate viewer; site information panel; page zoom indicator; find bar UI; print/save/cast menu items; custom right-click menu; reading list; side panel; split view; reader mode; translate; Lens / image search; screenshots; QR code sharing; "Send tab to device".
**Data:** bookmark folders, editing, manager, export, HTML import; per-visit history, grouping and per-entry delete; downloads with progress, pause/resume, persistence and danger checks; addresses and payment cards; password generator, leak check and passkey manager; re-authentication before revealing passwords.
**Accounts:** profiles; incognito; guest; sync; browser sign-in.
**Extensions:** Web Store; `.crx`; extension toolbar, popups and management page; full `chrome.*` API surface; Linux/macOS support.
**Platform integration:** default-browser registration; protocol and file handlers; single instance; jump lists; auto-update; installer; code signing; crash reporting; crash recovery ("Aw, Snap!" plus reload); full-screen handling; picture-in-picture controls (engine may provide); media hub; notification management; installable web apps.
**Security:** Safe Browsing / SmartScreen; per-site permission settings and saved decisions; HTTPS-only mode / upgrade switch; cookie controls (third-party blocking UI); site data viewer; clipboard permission prompt.
**Developer:** task manager; `chrome://` diagnostic pages (`net-internals`, `gpu`, `flags`).

---

## 20. Can this architecture support a complete Chrome-like browser without a major rewrite?

**Short answer:** it can become a **good, lightweight Chromium-based browser for Windows** without a rewrite. It **cannot** reach **complete Chrome parity**, especially full extension compatibility, Google sync, Safe Browsing parity and cross-platform Chromium, without replacing the engine layer.

### 20.1 Achievable on the current architecture (evolution, not rewrite)

| Area | How | Effort |
|---|---|---|
| Tab-strip overflow, pinned tabs, tab groups, drag reorder, tab search | Pure UI/HTML and `App` model changes | Low–medium |
| Multiple windows | Change `App` to hold `Vec<WindowState>`; tao supports several windows | Medium (substantial refactor, not a rewrite) |
| Profiles and incognito | wry `with_profile_name` / `with_incognito` (WebView2 multi-profile / InPrivate), with per-profile `Store` and `Vault` | Medium |
| Bookmark folders and bar, history improvements, download manager | Data model changes (consider SQLite) plus UI. WebView2 offers download operation objects for progress, pause and cancel (would need direct `webview2-com` calls) | Medium |
| Find bar, print, save page, favicons, full screen, crash recovery, permission prompts | Direct WebView2 calls through `webview2-com` (`ShowPrintUI`, favicon events, full-screen events, process-failed events, permission events) | Medium |
| Security fixes S1–S6, S8 | Per-tab clipboard handling (don't use wry's auto-grant), message whitelist and origin/user-gesture checks, re-enable SmartScreen, popup host window with an address bar, Windows Hello check (`UserConsentVerifier`) | Low–medium |
| Sync (self-hosted, end-to-end encrypted) | New service plus a client; independent of the engine | High, but no rewrite |
| Auto-update, installer, code signing, default-browser registration, single instance | Packaging and OS integration | Medium |

### 20.2 Blocked by the WebView2 / wry architecture

| Need | Why blocked | Required change |
|---|---|---|
| Full Chrome extension compatibility (action popups, Web Store, `chrome.tabs` mapped to Rustium tabs, extension update) | WebView2's extension support is a limited subset with no extension UI hooks | Engine layer replaced with **CEF** (Chromium Embedded Framework, e.g. through Rust CEF bindings), which has more complete extension support (scope must be checked), or a **Chromium fork** (the Brave/Vivaldi approach, C++, very large effort) |
| Google account sync and Chrome sign-in | Google doesn't offer Chrome Sync to third-party browsers | Not realistically achievable by anyone; replace with your own sync |
| Safe Browsing parity | Google Safe Browsing API licensing and integration; WebView2 offers SmartScreen instead | Re-enable SmartScreen (Windows) or integrate the Safe Browsing API separately |
| Same Chromium on Linux and macOS | wry uses the OS web view per platform | CEF or a Chromium fork |
| Deep engine control (process model, network stack, engine version pinning, Blink patches) | WebView2 is a closed, evergreen runtime | CEF (bundled Chromium, ~150–300 MB) or a fork |
| Pure-Rust engine | Servo is not yet able to run the whole web reliably | Long-term research option only |

### 20.3 Rewrite cost estimate if the engine is swapped

- **Survives mostly as-is:** `vault.rs`, `storage.rs`, `import.rs`, most of `perf.rs` (minus the WebView2-specific parts), the HTML UI and its message protocol, `content.js` (with adjustments). That's roughly 40–50% of the code.
- **Must be rewritten:** everything in `main.rs` that creates, places and controls web views (`Host`, `spawn_view`, `platform_opts`, `freeze`, `set_memory_low`, the callback wiring), and the build and packaging (CEF requires shipping Chromium binaries, which gives up the "tiny exe" advantage and much of the memory story).

### 20.4 Recommendation for planning

1. **Stay on WebView2 for now.** First fix the security issues (S1, S2, S3, S5) and bugs B1–B4 and B7.
2. **Test on real Windows** (§21) before building new features.
3. **Then add the shell features WebView2 supports:**
   - multiple windows, profiles and incognito;
   - tab groups and pinned tabs;
   - bookmark folders and bar;
   - a download manager;
   - find, print, full screen, crash recovery and permissions;
   - auto-update and default-browser registration.
4. **Only if full Chrome-extension compatibility or cross-platform Chromium becomes a hard requirement**, plan an engine abstraction layer (a trait wrapping "create web view, navigate, events"), then migrate to CEF. Starting that abstraction early in the refactor for multiple windows keeps that option open cheaply.

---

## 21. Recommended Windows verification checklist (none of this has been run yet)

1. The app starts; the toolbar draws; the WebView2 version shows in Settings → About.
2. Load YouTube, Discord (web), Twitch, Google Docs, a banking site and Netflix (DRM); test full-screen video.
3. Google sign-in at accounts.google.com.
4. Password: save → restart → autofill; Chrome CSV import with a real export; Chrome bookmark import from a real profile.
5. Shortcuts: F5, F12, Ctrl+Tab, Ctrl+W, Ctrl+Shift+T, mouse back button (check for double triggers).
6. Tab sleeping: open 15 tabs and wait 60 s → check suspension in Task Manager; wait 15 min → unloaded.
7. Game mode: start a game and check browser process priority and Efficiency Mode leaf icons in Task Manager; music keeps playing; check B3 by restarting with game mode on.
8. Memory: compare total private memory with Chrome for the same 5 / 20 tabs.
9. Downloads: a single file, two at once with the same name, "Show in folder".
10. Put an unpacked uBlock Origin into `Extensions\`, check it works; put a stray `.zip` there, check the browser still starts (B10).
11. Clipboard: check whether a test page can read the clipboard without a prompt (S1).
12. Background tab sends `{t:'key',k:'close'}`, check whether the active tab closes (S2).

---

## Appendix A: Data files

| File | Format | Contents |
|---|---|---|
| `settings.json` | JSON (`storage::Settings`) | homepage, search_engine, freeze_after_sec, sleep_after_min, max_awake_tabs, game_mode, gpu_boost, renderer_process_limit, restore_session, offer_save_passwords, never_save[] |
| `bookmarks.json` | JSON array | {title, url, folder, added} |
| `history.json` | JSON array, oldest first | {url, title, last_visit, visits}, at most 5000 |
| `session.json` | JSON | {tabs[], titles[], active} |
| `passwords.vault` | binary | 12-byte nonce ‖ ChaCha20-Poly1305(JSON [{id, origin, username, password, note, created, last_used}]) |
| `vault.key` | binary | Windows: DPAPI-protected 32-byte key. Others: the 32-byte key unencrypted, permission 0600 |
| `Engine\` | WebView2 / WebKit profile | cookies, cache, local storage, IndexedDB, extension registrations |
| `Extensions\` | folders | unpacked extensions (Windows only) |

## Appendix B: Message protocol

**UI → Rust** (`window.ipc.postMessage(JSON)`, handled in `on_ui` at `src/main.rs:855`):
`ready`, `height{h}`, `panel{open}`, `newtab`, `key{k}`, `close{id}`, `activate{id}`, `navigate{text}`, `open_url{url}`, `focus_page`, `back`, `forward`, `reload`, `home`, `bookmark_toggle`, `game_mode{on}`, `suggest{q}`, `pw_list`, `pw_reveal{id,copy}`, `pw_delete{id}`, `pw_import_csv{csv,name}`, `pw_export`, `pw_save_answer{answer}`, `bm_list`, `bm_delete{i}`, `bm_import_chrome`, `hist_list{q}`, `hist_clear`, `clear_data`, `show_file{path}`, `settings_get`, `settings_set{settings}`, `perf`, `sleep_all`.

**Rust → UI** (`evaluate_script`): `R.state(state)`, `R.reply(kind, data)` with kind in {password, suggest, passwords, settings, bookmarks, history, perf}, `R.toast(msg)`, `R.savePrompt(p|null)`, `R.focusOmni()`, `R.openPanel(name)`.

**Page → Rust** (any web page, through `content.js` or directly; handled in `on_page` at `src/main.rs:743`): `{t:'key',k}`, `{t:'url'}`, `{t:'media',on}`, `{t:'pw_query'}`, `{t:'pw_save',u,p}`. The source URL comes from the engine and can be trusted; the message body cannot (S2).

**Rust → page:** a fill script calling `window.__rsFill([{u,p}])` after checking the origin.

## Appendix C: Test inventory

| Test | File | Covers |
|---|---|---|
| `tests::omnibox` | `src/main.rs:1421-1449` | URL vs search detection, `javascript:` blocking |
| `vault::tests::roundtrip_and_chrome_csv_import` | `src/vault.rs:358` | CSV parsing (quoting, `android://` skip, `www.` normalisation), encrypted save and reload, no plaintext on disk, origin isolation |
| `vault::tests::upsert_updates_existing` | `src/vault.rs:386` | Add / unchanged / update logic |
| `import::tests::parses_chrome_format` | `src/import.rs:128` | Chrome Bookmarks JSON, nested folders, non-http filtered |

All 4 pass on Linux and on `windows-latest`. On Windows they exercise the real DPAPI path.
