# Ferrum

A lightweight Windows browser: a Rust shell around the system **WebView2 (Chromium)** runtime, with the browser UI (tabs, address bar, settings, bookmarks, history, downloads, onboarding) written as plain HTML, CSS and JavaScript.

![Web page with tabs](docs/screenshots/04-web-page-tabs.png)

Ferrum is Windows only. It does not ship its own rendering engine: pages are rendered by the WebView2 runtime installed on your PC.

## Features

- **Tabs**: pin, duplicate, close others, reopen closed tab, **private tabs** (WebView2 InPrivate; no history, not restored), optional **resume last tabs**
- **Clean address bar** option, suggestions from your history and your search engine (DuckDuckGo, Brave Search, Google, Startpage, Kagi)
- **Bookmarks** with nested folders, a cascading folder popup in the bookmarks bar, search and drag and drop; **import and export as JSON, HTML or CSV**
- **Import from other browsers** (Chromium and Firefox based): bookmarks, history, cookies and extensions, merged without duplicates
- **Extensions**: install Chrome extensions and Firefox add-ons (converted to Chromium extensions) from a store link, ID, name, `.crx`/`.xpi`/`.zip` or folder; one-click **Add to Ferrum** buttons on the Chrome Web Store and addons.mozilla.org; **automatic updates**
- **uBlock Origin, SponsorBlock and Spotify Adblock** are installed on first run (downloaded then, not bundled in the exe)
- **Downloads**: chunked parallel downloads with pause and resume, popup and page
- **Focus mode** and borderless fullscreen: tabs and toolbar hide; they slide back in when you bring the pointer to the top edge
- **Single-file installer and updater**: see [Installing](#installing) and [Updating](#updating)

### Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+T / Ctrl+W | New tab / close tab |
| Ctrl+Shift+T | Reopen closed tab |
| Ctrl+Shift+N | New private tab |
| Ctrl+Tab, Ctrl+PgDn / Ctrl+Shift+Tab, Ctrl+PgUp | Next / previous tab |
| Ctrl+1 to Ctrl+8, Ctrl+9 | Go to tab N / last tab |
| Ctrl+L, F6 | Focus the address bar |
| Ctrl+R, F5 | Reload |
| Ctrl+D | Bookmark this page |
| Ctrl+B / Ctrl+H / Ctrl+J | Bookmarks / history / downloads |
| Ctrl+, | Settings |
| F11 | Fullscreen |
| Ctrl+Shift+F | Focus mode |

### Settings

General (downloads popup, resume last tabs, open links in new tab, hardware acceleration), search engine, clean address bar, focus mode, autofill, bookmarks bar, extensions, import, and About and updates.

## Screenshots

All screenshots were taken from the real app against a throw-away profile filled with demo data (see [Notes on the screenshots](#notes-on-the-screenshots)).

| | |
| --- | --- |
| ![New tab](docs/screenshots/01-new-tab.png) New tab with the bookmarks bar | ![Address bar suggestions](docs/screenshots/02-address-bar-suggestions.png) Address bar suggestions |
| ![Bookmark folders](docs/screenshots/03-bookmarks-folder-popup.png) Cascading bookmark folders | ![Bookmarks page](docs/screenshots/05-bookmarks-page.png) Bookmarks page with nested folders |
| ![History](docs/screenshots/06-history-page.png) History | ![Downloads](docs/screenshots/07-downloads-page.png) Downloads page |
| ![Downloads popup](docs/screenshots/08-downloads-popup.png) Downloads popup | ![Extensions popup](docs/screenshots/09-extensions-popup.png) Extensions popup |
| ![Menu](docs/screenshots/10-menu.png) Menu | ![Private tab](docs/screenshots/15-private-tab.png) Private tab |
| ![Settings](docs/screenshots/11-settings-general.png) Settings: general | ![Settings search](docs/screenshots/12-settings-search-privacy.png) Settings: search, privacy, bookmarks |
| ![Settings extensions](docs/screenshots/13-settings-extensions.png) Settings: extensions | ![Settings about](docs/screenshots/14-settings-about.png) Settings: import, about |
| ![Focus mode](docs/screenshots/16-focus-mode-peek.png) Focus mode, toolbar revealed | ![Update dialog](docs/screenshots/22-update-dialog.png) Update dialog (example release) |
| ![Onboarding welcome](docs/screenshots/17-onboarding-welcome.png) Onboarding: welcome | ![Onboarding browsers](docs/screenshots/18-onboarding-browsers.png) Onboarding: browser picker |
| ![Onboarding options](docs/screenshots/19-onboarding-import-options.png) Onboarding: what to import | ![Onboarding progress](docs/screenshots/20-onboarding-importing.png) Onboarding: importing |
| ![Onboarding done](docs/screenshots/21-onboarding-done.png) Onboarding: done (illustrative numbers) | |

## How it works

```
                         ferrum.exe (Rust)
 +-------------------------------------------------------------+
 |  tao window (borderless)       wry/webview2-com glue        |
 |                                                             |
 |  +---------------------------------------------------+      |
 |  | chrome  WebView2: ui/chrome.html (tabs + toolbar) |      |
 |  +---------------------------------------------------+      |
 |  | tab WebView2 per tab (web pages, rb:// pages)     |      |
 |  |   ... one controller per tab, only one visible    |      |
 |  +---------------------------------------------------+      |
 |  | popup WebView2: ui/popup.html (menus, suggestions,|      |
 |  |   downloads, extensions, bookmark folders)        |      |
 |  +---------------------------------------------------+      |
 |  | onboarding overlay (ui/onboarding.html)           |      |
 |  +---------------------------------------------------+      |
 |                                                             |
 |  rb protocol: http://rb.localhost/<page> -> ui/*.html       |
 |    (compiled into the exe with include_bytes!)              |
 |  IPC: page -> JSON via ipc.postMessage -> Rust handler      |
 |       Rust -> page via evaluate_script                      |
 |  Extensions: AddBrowserExtension + Firefox -> Chromium      |
 |    converter (src/extinstall.rs)                            |
 |  fetch.rs: chunked downloader   install.rs / update.rs      |
 +-------------------------------------------------------------+
                              |
                  WebView2 runtime (msedgewebview2.exe)
```

- **Window**: [tao](https://github.com/tauri-apps/tao) creates a borderless window; [wry](https://github.com/tauri-apps/wry) and `webview2-com` create WebView2 controllers as children of it.
- **Chrome / popup / tab controllers**: the toolbar and tab strip, the popup surface, and each tab are separate WebView2 controllers laid out by the Rust code (`src/main.rs`).
- **`rb` protocol**: `http://rb.localhost/` is served from the exe itself. The pages in `ui/` are embedded at compile time, so the exe is self-contained.
- **IPC rules**: UI pages talk to Rust with `ipc.postMessage(JSON)`. The handler checks which surface a message came from (chrome, popup, internal page, or ordinary web page) and only honours the commands that surface is allowed to send; ordinary web pages cannot drive it.
- **Extensions**: installed with WebView2's `AddBrowserExtension`. Firefox add-ons are converted to Chromium (manifest and API shims, `assets/browser-polyfill.min.js`, `assets/rb-sw-shim.js`); add-ons that need Firefox-only features are refused.
- **Installer and updater**: see below.

### Installer and updater flow

1. First run of a `ferrum.exe` that is not already in `C:\Program Files\Ferrum` asks to install. Accepting relaunches elevated (UAC), copies the exe, and creates the Start Menu shortcut and an uninstall entry.
2. Declining runs the exe portably from where it is; the choice is remembered for that version.
3. Updating: **Settings > About & updates > Check for updates** queries the GitHub releases API for this repository, shows the release notes, and on **Update** downloads `ferrum.exe` and `ferrum.exe.sha256` from the release, checks the SHA-256, and swaps the exe.

## Installing

1. Download `ferrum.exe` (and, optionally, `ferrum.exe.sha256`) from the [Releases](https://github.com/k7m2q9x4b1z8p6nd/Ferrum/releases) page.
2. Double-click `ferrum.exe`. On first run it offers to install to `C:\Program Files\Ferrum` (UAC prompt), adds a Start Menu shortcut so you can find it by searching for "Ferrum", and registers an uninstall entry.
3. If you decline, Ferrum runs as a portable exe from its current location.

Check the download with PowerShell: `Get-FileHash .\ferrum.exe -Algorithm SHA256` and compare with `ferrum.exe.sha256`.

Requires Windows with the Microsoft Edge WebView2 runtime.

## Updating

**Settings > About & updates > Check for updates.** If a newer release exists, a dialog shows its release notes with **Cancel** and **Update**.

## Where data lives

Settings, bookmarks, history, downloads list, installed extensions and the WebView2 profile are stored in `%APPDATA%\Ferrum`, which is created automatically. The `RB_DATA_DIR` environment variable overrides the location.

## Building from source

Requirements: Windows, a recent stable Rust toolchain, and the WebView2 runtime.

```
cargo build --release
```

The exe is `target\release\ferrum.exe`. `cargo test` runs the unit tests (92 pass, 3 network tests are ignored). Set `FERRUM_NO_INSTALL=1` when running a development build so it does not offer to install itself.

## Performance

Measured on one machine with the release exe, an isolated profile, and the Chrome DevTools Protocol for timing and screenshots. Numbers are medians with min and max in brackets. They are single-machine measurements, not benchmarks of other browsers.

Machine: Intel Core i7-11700F (8 cores, 16 threads), 31.8 GB RAM, NVIDIA GeForce RTX 5060 Ti, Windows 11 Home build 26100, WebView2 runtime 157.0.4341.46.

| Metric | Result | Notes |
| --- | --- | --- |
| Startup to first paint of the browser UI | 298 ms [272 to 369], n=7 | `scripts/perf.mjs startup`; process spawn to first meaningful paint of the toolbar page; warm profile with extensions already installed |
| Startup to first tab (new tab page DOMContentLoaded) | 376 ms [371 to 453], n=7 | same run |
| Memory, 1 tab, idle | 868 MB [860 to 900], n=5 | sum of working sets of `ferrum.exe` and all its descendant `msedgewebview2.exe` processes, 12 s after launch; includes the extension background processes (uBlock Origin, SponsorBlock, Spotify Adblock) |
| Memory, 10 tabs on a local page | 1382 MB [1379 to 1394], n=5 | same method |
| Exe size | 7,180,800 bytes (7.18 MB) | `target\release\ferrum.exe` |
| Profile size after first run | about 62 MB | `%APPDATA%\Ferrum` equivalent with the three preinstalled extensions and the WebView2 profile |
| Installed footprint | not measured | the installer needs a UAC prompt; it copies the single exe |
| Speedometer 3.1 | 29.0 ± 0.32 (one run) | `scripts/perf.mjs speedo`, score read from the page screenshot; a single run, so no spread across runs. An earlier run on this project reported 13.4 ± 0.59 and was not reproduced; the cause was not investigated |
| Bookmark import, 20,000 bookmarks | parse 24 / 12 / 22 ms (JSON / HTML / CSV), merge 4 / 5 / 5 ms | `cargo test --release large_import_perf -- --nocapture`, one run, in-process, no UI |
| Chunked vs single-connection download | not re-measured for this release | an earlier local run against a server throttled to about 2 MB/s per connection saw about 8.6 MB/s chunked vs about 2 MB/s for one connection; treat as an earlier, unrepeated measurement |

Caveats: the memory figure sums working sets, which counts pages shared between the WebView2 processes more than once, so it overstates real memory use. Startup was measured with the DevTools port enabled and without the installer path (`FERRUM_NO_INSTALL=1`). Everything is one machine, a handful of runs, and background load was not controlled.


## Known limitations

- Passwords cannot be imported.
- Extensions do not run in private tabs.
- Firefox extension compatibility is partial: add-ons that need Firefox-only features are refused.
- Manifest V2 extensions rely on WebView2 flags that Microsoft may remove in a future runtime.
- WebView2 has no extension toolbar buttons; extension popups and options are opened from Ferrum's extensions popup.
- Windows only.
- No license has been chosen for this repository yet, so by default all rights are reserved.

## Notes on the screenshots

- Taken over the DevTools protocol (no desktop capture) from a window of 1280 x 800, with a profile containing only demo bookmarks, history and downloads.
- The window is assembled from separate captures of the toolbar, the active page and any open popup. Popups are re-rendered from the live popup page because WebView2 reported that surface as hidden to the DevTools protocol.
- The scroll bars of the bookmark folder popup were hidden in that capture.
- The onboarding browser list is the real list of browsers detected on the capture machine, but the import progress and result numbers are illustrative: the import was not run.
- The update dialog was produced against a local fake release server; the release shown is an example, not a real version.

## Project layout

```
Cargo.toml, Cargo.lock, build.rs   crate and Windows resource (icon) setup
src/main.rs                        window, webview controllers, IPC, tab logic
src/bookmarks*.rs, history.rs      bookmarks (+ import/export), history
src/downloads.rs, fetch.rs         download tracking, chunked downloader
src/extensions.rs, extinstall.rs,  extension management, installer + Firefox
    extupdate.rs                   converter, auto-update
src/import.rs                      import from other browsers
src/install.rs, update.rs          self-install and updater
src/settings.rs, session.rs        settings, resume-last-tabs session
ui/                                HTML/CSS for every browser surface
assets/                            icon, extension polyfill and shims
scripts/                           make_icons.py, perf.mjs, perf_assets.py
docs/screenshots/                  README images
```

## Contributing

Issues and pull requests are welcome. Please run `cargo test` before sending a change, keep the UI as plain HTML without build steps, and describe how you verified behaviour that tests cannot cover (anything involving WebView2).
