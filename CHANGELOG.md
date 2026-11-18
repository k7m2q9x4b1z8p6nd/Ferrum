# Changelog

## Unreleased

### Added
- **Memory saver** (Settings > General, on by default). Tabs you have not looked at for 5 minutes are put to sleep with WebView2 `TrySuspend` and their renderer memory is released; the tab title is dimmed in the strip and the tab wakes when you select it. Hidden tabs are also asked to use less memory (`MemoryUsageTargetLevel` low). Tabs that are pinned, playing audio, loading or being restored stay awake, and nothing is suspended while a download is running.
- With Memory saver on, the preloaded new tab and the popup webview are released after a minute without use and recreated on demand: the first new tab after that takes about 65 ms instead of 15 ms and the first popup about 100 ms instead of 20 ms.

## 1.0.1

### Fixed
- **Focus mode with a fullscreen page** (for example a fullscreen video): moving the pointer to the top edge, or focusing the address bar, now brings back the tabs and toolbar. They previously stayed hidden until fullscreen ended.
- **Firefox add-ons with a non-persistent background page** (`"persistent": false`) were rejected with "The browser rejected the extension: Class not registered". They are now converted so the browser accepts them.

### Added
- **Add Violentmonkey** button under Settings > Extensions > userscripts. Violentmonkey installs from addons.mozilla.org and runs userscripts; checked by installing a script and loading a page it matches.

### Known limitations
- **Tampermonkey does not run scripts in Ferrum.** The Chrome Web Store build is Manifest V3 and needs `chrome.userScripts`, which WebView2 leaves undefined even with the `userScripts` permission granted. The addons.mozilla.org build is refused (it needs Firefox container tabs) and relies on Firefox's own `userScripts` API. Use Violentmonkey.

## 1.0.0

First public release. Ferrum is a lightweight Windows browser: a Rust shell around the system WebView2 (Chromium) runtime, with the browser UI written as plain HTML.

### Browsing
- Tabs with pinning, duplicate, close-others and reopen-closed-tab
- **Private tabs** that keep no history and are not restored on the next start
- **Resume last tabs** on startup (off by default)
- **Focus mode** and borderless fullscreen: tabs and toolbar hide, move the pointer to the top edge to bring them back
- Clean address bar option (hides `http://` and `https://`, shows internal pages by name)
- Address bar suggestions from your history and the search engine of your choice (DuckDuckGo, Brave Search, Google, Startpage, Kagi)
- Keyboard shortcuts for tabs, address bar, history, downloads, bookmarks, settings, fullscreen and focus mode

### Bookmarks and history
- Bookmarks bar with **nested folders** and a cascading folder popup
- Bookmarks page with search, drag and drop, and **import/export as JSON, HTML or CSV**
- Large imports stay fast (merging 20,000 bookmarks is a single pass)
- History page and address bar history matching

### Import from other browsers
- Detects installed Chromium and Firefox based browsers
- Imports **bookmarks, history, cookies and extensions**, merging with what you already have and skipping duplicates
- First-run onboarding walks through it; it can be re-run from Settings

### Extensions
- Install Chrome extensions and **Firefox add-ons** (converted automatically) from a store link, an extension ID, a name, or a `.crx`, `.xpi`, `.zip` or folder
- One-click **Add to Ferrum** buttons on Chrome Web Store and addons.mozilla.org pages
- **Automatic extension updates**, with a manual check in Settings
- **uBlock Origin**, **SponsorBlock** and **Spotify Adblock** are installed on first run (downloaded then, not bundled in the exe)
- Enable, disable and remove extensions from the toolbar popup or Settings

### Downloads
- **Chunked parallel downloads** with pause and resume, and a downloads popup and page
- Downloads that cannot be split fall back to a single stream

### Installing and updating
- Single `ferrum.exe`: first run offers to install to `C:\Program Files\Ferrum` (UAC prompt), adds a Start Menu shortcut and an uninstall entry
- Declining the prompt runs it portably from wherever it is
- **Settings > About & updates > Check for updates** shows the release notes with Cancel and Update; the new exe is verified against its published SHA-256 before it replaces the old one
- Settings, bookmarks, history and the browser profile live in `%APPDATA%\Ferrum`

### Known limitations
- Passwords cannot be imported
- Extensions do not run in private tabs
- Firefox extension support is partial: add-ons that need Firefox-only features are refused
- Manifest V2 extensions depend on WebView2 flags that Microsoft may remove in a future runtime
- WebView2 has no extension toolbar buttons, so extension actions are opened from the Ferrum extensions popup
- Windows only; requires the WebView2 runtime (included with current Windows 10 and 11)
