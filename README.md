# Rustium

A lightweight, gaming-friendly web browser written in **Rust**.

The browser itself is written in Rust: the window, tabs, address bar, password manager, bookmarks, history, downloads, tab sleeping and game mode. Web pages are drawn by the **Chromium engine** (Blink + V8), using the copy Windows already ships as **Microsoft WebView2**. That means:

- Sites look and behave the same as in Chrome: YouTube, Discord, Twitch, Google Docs, banking sites and so on.
- Rustium doesn't bundle its own ~300 MB copy of Chromium. The download is a single small `.exe`.
- Chromium security updates arrive through Windows Update.

> **Why not rewrite Chromium in Rust?** Chromium is about 35 million lines of C++, built by thousands of engineers over 15+ years. Nobody can rewrite that. Servo, the most serious Rust browser engine, has been in development for over a decade and still can't run the whole web. The realistic way to get a Rust browser that works on every site is a Rust browser shell around the Chromium engine. Brave, Edge, Opera and Vivaldi also build on Chromium.

## Features

| | |
|---|---|
| **Tabs** | Chrome-style tab strip. Middle-click to close, `Ctrl+T` / `Ctrl+W` / `Ctrl+Tab` / `Ctrl+1…9` / `Ctrl+Shift+T` |
| **Address bar** | Typing a URL opens it, anything else is searched (Google, Bing, DuckDuckGo, Brave, Startpage). Suggestions come from history and bookmarks |
| **Password manager** | Offers to save logins, autofills them, and can show, copy or delete them. Encrypted on disk (ChaCha20-Poly1305, with the key protected by **Windows DPAPI**, which is also what Chrome uses) |
| **Chrome import** | Passwords: import the CSV that Chrome exports. Bookmarks: one click reads them straight from your Chrome, Edge or Brave profiles |
| **Low memory** | Restored tabs load only when you click them. Background tabs are *frozen* after 1 minute and *unloaded* after 15. There's a cap on how many tabs stay loaded, and pages from the same site share one process. A live RAM counter sits in the toolbar |
| **Game mode** 🎮 | When the browser isn't the focused window, every browser process drops to below-normal priority and Windows **Efficiency Mode**. Background tabs freeze immediately and at most 3 tabs stay loaded. Music and video keep playing. `Ctrl+Shift+G` |
| **GPU boost** | Turns on GPU rasterization and zero-copy, and ignores Chromium's GPU blocklist. Gaming GPUs handle this fine |
| **Session restore** | Your tabs come back next time you open the browser |
| **Downloads** | Saved to your Downloads folder, with a list and "Show in folder" |
| **Extensions** | Put *unpacked* Chrome extensions (for example uBlock Origin) in `%LOCALAPPDATA%\Rustium\Extensions\<name>\` and restart |
| **DevTools** | `F12` |

## Getting your passwords and bookmarks from Chrome

**Passwords.** Since version 127, Chrome encrypts saved passwords and cookies with *app-bound encryption*, which stops every other app from reading them. Malware is the main target of that protection, but it blocks Rustium too, so passwords come across through Chrome's own export:

1. In Chrome, open `chrome://password-manager/settings`, click **Export passwords** and save the `.csv` file.
2. In Rustium, click 🔑, then **Import CSV**, and pick that file.
3. **Delete the CSV.** It contains every password in plain text.

Exports from Edge, Brave, Firefox and Bitwarden in the same format also work.

**Bookmarks.** In Rustium, go to ⋮ → Bookmarks → **Import from Chrome**. It reads every Chrome, Edge and Brave profile on the PC.

**Logged-in sessions (cookies)** can't be copied out of Chrome, for the same app-bound encryption reason. Sign in to each site once and Rustium will remember you. Google sometimes refuses sign-in from embedded browser engines ("This browser or app may not be secure"). If that happens, sign in to the specific Google service you need (Gmail, YouTube) instead of the account page.

## Download / build

**Prebuilt:** every push builds `rustium.exe` on GitHub Actions. Open the *Actions* tab, choose the latest run and download the **rustium-windows-x64** artifact. Pushing a `v*` tag publishes a Release.

**From source (Windows):**

```powershell
# needs Rust (https://rustup.rs) — WebView2 is already part of Windows 10/11
cargo build --release
.\target\release\rustium.exe
```

**Linux** builds work too, but there the engine is WebKitGTK rather than Chromium:

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev
cargo run --release
```

## Where your data lives

`%LOCALAPPDATA%\Rustium\`:

| File | Contents |
|---|---|
| `passwords.vault` | Your passwords, encrypted |
| `vault.key` | Encryption key, protected with DPAPI so only your Windows account can unlock it |
| `bookmarks.json`, `history.json`, `session.json`, `settings.json` | Plain JSON |
| `Engine\` | Chromium profile: cookies, cache, site storage |
| `Extensions\` | Unpacked extensions |

On Linux the vault key is a file readable only by you (`0600`). It isn't protected by an OS keychain yet.

## Project layout

```
src/main.rs        window, tabs, event loop, IPC, tab sleeping, game mode
src/vault.rs       encrypted password vault + Chrome/Edge/Firefox CSV import/export
src/import.rs      Chrome/Edge/Brave bookmark import
src/storage.rs     settings, bookmarks, history, session (JSON)
src/perf.rs        engine flags, process priority / EcoQoS, memory accounting
src/ui/chrome.html toolbar, tab strip and settings/password panels
src/ui/content.js  injected into pages: shortcuts, autofill, save-password, media detection
```

## Honest limitations

- It isn't Google Chrome. There's no Google account sync and no Chrome Web Store; extensions are loaded unpacked, as described above.
- Windows only for the Chromium engine. Linux uses WebKitGTK, and macOS would use WebKit.
- Popups that set a window size, such as "Sign in with…" windows, open as separate windows so the login can finish. Every other new-window link opens as a tab.
- Revealing a saved password doesn't ask for your Windows PIN or password yet.
