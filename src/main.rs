#![windows_subsystem = "windows"]

mod bookmarks;
mod bookmarks_io;
mod downloads;
mod extensions;
mod extinstall;
mod extupdate;
mod favicon;
mod fetch;
mod fileicon;
mod history;
mod icons;
mod import;
mod install;
mod keys;
mod saver;
mod session;
mod settings;
mod source;
mod update;

use bookmarks::Bookmarks;
use downloads::Dl;
use history::History;
use serde_json::{json, Value};
use settings::{Engine, Settings};
use std::{
    borrow::Cow,
    cell::Cell,
    collections::VecDeque,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc, RwLock},
    time::{Duration, Instant},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowExtWindows,
    window::{Fullscreen, Icon, Window, WindowBuilder},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW},
            WindowsAndMessaging::{
                GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, HWND_TOP, STYLESTRUCT, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                WM_STYLECHANGING, WS_CLIPCHILDREN,
            },
        },
    },
};
use wry::{
    dpi::LogicalPosition,
    http::{Request, Response},
    NewWindowResponse, PageLoadEvent, Rect, ScrollBarStyle, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows, WebViewId,
};

const BG: (u8, u8, u8, u8) = (19, 19, 22, 255);
const CHROME_H: f64 = 80.0;
const BM_H: f64 = 34.0;
const ROW_H: f64 = 36.0;
const EXT_ROW_H: f64 = 48.0;
const STORE_JS: &str = include_str!("../assets/rb-store.js");
const PUSH_GAP: Duration = Duration::from_millis(24);
const NEWTAB: &str = "http://rb.localhost/newtab";
const SETTINGS: &str = "http://rb.localhost/settings";
const DOWNLOADS: &str = "http://rb.localhost/downloads";
const HISTORY: &str = "http://rb.localhost/history";
const BOOKMARKS: &str = "http://rb.localhost/bookmarks";
const ONBOARDING: &str = "http://rb.localhost/onboarding";
const PEEK_ZONE: i32 = 6;
const PEEK_MS: f64 = 120.0;

#[derive(Clone, Copy, PartialEq)]
enum Src {
    Chrome,
    Popup,
    Tab(u32),
}

enum Ev {
    Msg(Src, String),
    Key(String),
    Title(u32, String),
    Url(u32, String),
    Source(u32, String),
    Load(u32, bool, String),
    NewWin(u32, String, bool, bool),
    Sugs(String, Vec<String>),
    DlStart(u32, String, PathBuf),
    Fetch(u32, fetch::Msg),
    DlDone(String, Option<PathBuf>, bool),
    DlProgress(String, u64, u64),
    HistSave,
    IconsReady,
    Exts(Vec<extensions::Ext>),
    ExtProgress(String),
    StoreInstall(extinstall::Source),
    PackageLink(u32, String),
    ExtInstalled(Result<extinstall::Done, String>),
    ExtAdded(extinstall::Done, Result<String, String>),
    ExtRemoved(String),
    ExtUpd(Value),
    ExtAuto(Vec<(String, String)>),
    ExtSwap(String, String, Result<extinstall::Done, String>, bool),
    ExtSwapped(String, String, Result<String, String>, bool, extinstall::Done),
    Icon(u32, String),
    BmImport(Vec<bookmarks_io::Entry>),
    ObBrowsers(Value),
    ObOut(import::Out),
    ObExt(extinstall::Done, Option<std::path::PathBuf>, Result<String, String>, std::sync::mpsc::Sender<Result<(), String>>),
    ObDone,
    Spare,
    Suspended(u32, bool),
    Restore,
    SessTick,
    Scroll(u32, f64),
    PopupInit,
    PageFs(u32, bool),
    Update(bool, Result<Option<update::Release>, String>),
    UpdProg(u64, u64),
    UpdDone(Result<PathBuf, String>),
}

struct Tab {
    id: u32,
    wv: WebView,
    title: String,
    url: String,
    loading: bool,
    pinned: bool,
    private: bool,
    icon: String,
    scroll: f64,
    seek: bool,
    hidden: Option<Instant>,
    sus: bool,
}

struct Focus {
    on: bool,
    full: bool,
    page: bool,
    addr: bool,
    to: bool,
    p: f64,
    at: Instant,
    leave: Option<Instant>,
    next: Option<Instant>,
}

type Box4 = (f64, f64, f64, f64);

struct Browser {
    win: Window,
    ctx: WebContext,
    px: EventLoopProxy<Ev>,
    cfg: Arc<RwLock<Settings>>,
    chrome: WebView,
    popup: Option<WebView>,
    popup_kind: &'static str,
    popup_ready: bool,
    popup_wait: Option<(&'static str, f64, f64, f64, Value)>,
    popup_at: Instant,
    spare_at: Instant,
    tabs: Vec<Tab>,
    spare: Option<Tab>,
    rest: VecDeque<(bool, session::Entry)>,
    rest_head: u32,
    rest_tail: u32,
    sess: Vec<u8>,
    active: u32,
    next: u32,
    dls: Vec<Dl>,
    history: History,
    hist_wait: bool,
    closed: Vec<String>,
    tab_menu: u32,
    anchor: (f64, f64),
    dl_anchor: (f64, f64),
    bm: Bookmarks,
    bm_anchor: (f64, f64),
    bm_sel: u32,
    bm_fsel: String,
    bm_cols: Vec<String>,
    bar_on: Cell<bool>,
    push_at: Cell<Instant>,
    push_due: Cell<bool>,
    fs: Focus,
    lay: Cell<(Box4, Box4, u32)>,
    dl_auto: bool,
    dl_want: bool,
    ext_page: bool,
    ext_busy: bool,
    upd_q: Vec<(String, String)>,
    update: Option<update::Release>,
    upd_show: bool,
    upd_cancel: Option<Arc<AtomicBool>>,
    quit: bool,
}

fn serve(_: WebViewId, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    if let Some(host) = req.uri().path().strip_prefix("/favicon/").and_then(|p| p.strip_suffix(".png")) {
        return match icons::read(host) {
            Some((mime, b)) => Response::builder().header("Content-Type", mime).header("Cache-Control", "max-age=600").body(Cow::Owned(b)).unwrap(),
            None => Response::builder().status(404).header("Cache-Control", "no-store").body(Cow::Borrowed(&[][..])).unwrap(),
        };
    }
    if let Some(ext) = req.uri().path().strip_prefix("/fileicon/").and_then(|p| p.strip_suffix(".png")) {
        return Response::builder().header("Content-Type", "image/png").body(Cow::Owned(fileicon::png(ext))).unwrap();
    }
    let (mime, body): (&str, &'static [u8]) = match req.uri().path() {
        "/chrome" => ("text/html", include_bytes!("../ui/chrome.html")),
        "/popup" => ("text/html", include_bytes!("../ui/popup.html")),
        "/bookmarks" => ("text/html", include_bytes!("../ui/bookmarks.html")),
        "/newtab" | "/" => ("text/html", include_bytes!("../ui/newtab.html")),
        "/settings" => ("text/html", include_bytes!("../ui/settings.html")),
        "/downloads" => ("text/html", include_bytes!("../ui/downloads.html")),
        "/history" => ("text/html", include_bytes!("../ui/history.html")),
        "/onboarding" => ("text/html", include_bytes!("../ui/onboarding.html")),
        "/base.css" => ("text/css", include_bytes!("../ui/base.css")),
        "/icons/fav.png" | "/favicon.ico" => ("image/png", include_bytes!("../ui/icons/fav.png")),
        "/icons/app.png" => ("image/png", include_bytes!("../ui/icons/app.png")),
        "/icons/0.png" => ("image/png", include_bytes!("../ui/icons/0.png")),
        "/icons/1.png" => ("image/png", include_bytes!("../ui/icons/1.png")),
        "/icons/2.png" => ("image/png", include_bytes!("../ui/icons/2.png")),
        "/icons/3.png" => ("image/png", include_bytes!("../ui/icons/3.png")),
        "/icons/4.png" => ("image/png", include_bytes!("../ui/icons/4.png")),
        _ => {
            return Response::builder().status(404).body(Cow::Borrowed(&b""[..])).unwrap();
        }
    };
    Response::builder().header("Content-Type", mime).body(Cow::Borrowed(body)).unwrap()
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { position: LogicalPosition::new(x, y).into(), size: wry::dpi::LogicalSize::new(w.max(1.0), h.max(1.0)).into() }
}

fn builder<'a>(ctx: &'a mut WebContext, px: &EventLoopProxy<Ev>, src: Src, hw: bool) -> WebViewBuilder<'a> {
    let px = px.clone();
    let mut args = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,ExtensionManifestV2Unsupported,ExtensionManifestV2Disabled,ExtensionManifestV2DeprecationWarning --enable-features=msEdgeAllowLegacyMV2Extensions --enable-quic --enable-parallel-downloading --enable-tcp-fast-open --disable-background-networking");
    if !hw {
        args.push_str(" --disable-gpu --disable-gpu-compositing");
    }
    let load = src == Src::Chrome;
    let b = WebViewBuilder::new_with_web_context(ctx)
        .with_additional_browser_args(args)
        .with_scroll_bar_style(ScrollBarStyle::FluentOverlay)
        .with_background_color(BG)
        .with_browser_extensions_enabled(true)
        .with_custom_protocol("rb".into(), serve)
        .with_ipc_handler(move |r| {
            let host = r.uri().host().unwrap_or_default().to_owned();
            if host == "rb.localhost" {
                let _ = px.send_event(Ev::Msg(src, r.into_body()));
            } else if let (Src::Tab(_), Some(s)) = (src, extinstall::store_request(&host, r.body())) {
                let _ = px.send_event(Ev::StoreInstall(s));
            }
        });
    if load { b.with_extensions_path(extensions::dir()) } else { b }
}

fn apply_autofill(wv: &WebView, on: bool) {
    use windows::core::Interface;
    unsafe {
        let Ok(core) = wv.webview().Settings() else { return };
        if let Ok(s) = core.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4>() {
            let _ = s.SetIsGeneralAutofillEnabled(on);
            let _ = s.SetIsPasswordAutosaveEnabled(on);
        }
    }
}

fn has_scheme(q: &str) -> bool {
    ["http://", "https://", "file://"].iter().any(|p| q.starts_with(p))
}

fn is_url(q: &str) -> bool {
    let host = q.split(['/', ':', '?']).next().unwrap_or("");
    has_scheme(q) || (!q.contains(' ') && (host == "localhost" || (host.contains('.') && !host.ends_with('.'))))
}

fn js(v: &Value) -> String {
    v.to_string()
}

fn chrome_h(bar: bool) -> f64 {
    if bar { CHROME_H + BM_H } else { CHROME_H }
}

fn to_top(wv: &WebView) {
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_ok() {
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

unsafe extern "system" fn clip_children(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize) -> LRESULT {
    unsafe {
        if msg == WM_STYLECHANGING && wp.0 as i32 == GWL_STYLE.0 {
            (*(lp.0 as *mut STYLESTRUCT)).styleNew |= WS_CLIPCHILDREN.0;
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

fn clip_children_on(win: &Window) {
    unsafe {
        let hwnd = HWND(win.hwnd() as _);
        let _ = SetWindowSubclass(hwnd, Some(clip_children), 1, 0);
        SetWindowLongPtrW(hwnd, GWL_STYLE, GetWindowLongPtrW(hwnd, GWL_STYLE) | WS_CLIPCHILDREN.0 as isize);
    }
}

impl Browser {
    fn size(&self) -> (f64, f64) {
        let s = self.win.inner_size().to_logical::<f64>(self.win.scale_factor());
        (s.width, s.height)
    }

    fn bar_for(&self, url: &str) -> bool {
        self.cfg.read().unwrap().bookmarks_bar && matches!(url, NEWTAB | "http://rb.localhost/")
    }

    fn bar(&self) -> bool {
        self.tab(self.active).is_some_and(|t| self.bar_for(&t.url))
    }

    fn layout(&self) {
        let (w, h) = self.size();
        let ch = chrome_h(self.bar());
        let im = self.immersive();
        let (cy, ty) = if im { (-ch * (1.0 - self.fs.p * (2.0 - self.fs.p)), 0.0) } else { (0.0, ch) };
        let (c, t) = ((0.0, cy, w, ch), (0.0, ty, w, h - ty));
        let (pc, pt, pid) = self.lay.get();
        if c != pc {
            let _ = self.chrome.set_bounds(rect(c.0, c.1, c.2, c.3));
        }
        if let Some(tab) = self.tab(self.active) {
            if t != pt || tab.id != pid {
                let _ = tab.wv.set_bounds(rect(t.0, t.1, t.2, t.3));
            }
        }
        self.lay.set((c, t, self.active));
    }

    fn immersive(&self) -> bool {
        self.fs.on || self.fs.page
    }

    fn peek_step(&mut self, p: f64) {
        let (was, is) = (self.fs.p > 0.0, p > 0.0);
        self.fs.p = p;
        if is && !was {
            let _ = self.chrome.set_visible(true);
            to_top(&self.chrome);
        }
        self.layout();
        if was && !is {
            let _ = self.chrome.set_visible(false);
            if let Some(t) = self.tab(self.active) {
                let _ = t.wv.focus();
            }
        }
    }

    fn reveal(&mut self) {
        if self.fs.on {
            self.fs.to = true;
            self.fs.leave = Some(Instant::now() + Duration::from_millis(600));
            if self.fs.p == 0.0 {
                self.peek_step(0.001);
            }
        }
    }

    fn sync_fs(&mut self) {
        let im = self.immersive();
        self.win.set_fullscreen((self.fs.full || self.fs.page).then_some(Fullscreen::Borderless(None)));
        self.fs.to = false;
        self.fs.p = 0.0;
        self.fs.leave = None;
        self.fs.addr = false;
        self.close_popup();
        let _ = self.chrome.set_visible(!im);
        if im {
            if let Some(t) = self.tab(self.active) {
                let _ = t.wv.focus();
            }
        }
        self.lay.set(((0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0), 0));
        self.layout();
    }

    fn fs_tick(&mut self) {
        self.fs.next = None;
        if !self.fs.on {
            return;
        }
        let now = Instant::now();
        let (mut pt, mut r) = (POINT::default(), RECT::default());
        let hwnd = HWND(self.win.hwnd() as _);
        let over = unsafe {
            GetCursorPos(&mut pt).is_ok() && GetWindowRect(hwnd, &mut r).is_ok() && GetForegroundWindow() == hwnd && pt.x >= r.left && pt.x < r.right && {
                let dy = pt.y - r.top;
                dy >= 0 && dy < if self.fs.to { (chrome_h(self.bar()) * self.win.scale_factor()) as i32 } else { PEEK_ZONE }
            }
        };
        if over || self.fs.addr || self.popup_kind != "" {
            self.fs.leave = None;
            self.fs.to = true;
        } else if self.fs.to {
            match self.fs.leave {
                None => self.fs.leave = Some(now + Duration::from_millis(400)),
                Some(t) if now >= t => {
                    self.fs.to = false;
                    self.fs.leave = None;
                }
                _ => {}
            }
        }
        let dt = now.duration_since(self.fs.at).as_secs_f64() * 1000.0 / PEEK_MS;
        self.fs.at = now;
        let goal = if self.fs.to { 1.0 } else { 0.0 };
        if self.fs.p != goal {
            let p = if self.fs.to { (self.fs.p + dt.clamp(0.05, 1.0)).min(1.0) } else { (self.fs.p - dt.clamp(0.05, 1.0)).max(0.0) };
            self.peek_step(p);
        }
        self.fs.next = Some(now + Duration::from_millis(if self.fs.p != goal { 16 } else { 60 }));
    }

    fn settings_json(&self) -> Value {
        let s = self.cfg.read().unwrap();
        json!({
            "engine": s.engine.id(), "engine_name": s.engine.name(),
            "engines": Engine::ALL.iter().map(|e| json!({"id": e.id(), "name": e.name()})).collect::<Vec<_>>(),
            "hardware_accel": s.hardware_accel, "bookmarks_bar": s.bookmarks_bar, "autofill": s.autofill, "links_in_new_tab": s.links_in_new_tab, "clean_urls": s.clean_urls, "focus_mode": s.focus_mode, "auto_update_ext": s.auto_update_ext, "dl_popup": s.dl_popup, "resume_tabs": s.resume_tabs, "memory_saver": s.memory_saver, "version": update::VERSION,
        })
    }

    fn push(&self) {
        let now = Instant::now();
        if now < self.push_at.get() + PUSH_GAP {
            self.push_due.set(true);
            return;
        }
        self.push_at.set(now);
        self.push_due.set(false);
        self.push_now();
    }

    fn push_flush(&self) {
        if self.push_due.get() && Instant::now() >= self.push_at.get() + PUSH_GAP {
            self.push();
        }
    }

    fn push_now(&self) {
        let e = self.cfg.read().unwrap().engine;
        let bar = self.bar();
        if self.bar_on.replace(bar) != bar {
            self.layout();
        }
        let st = json!({
            "tabs": self.tabs.iter().map(|t| json!({"id": t.id, "title": t.title, "url": t.url, "loading": t.loading, "pinned": t.pinned, "private": t.private, "icon": t.icon, "sus": t.sus})).collect::<Vec<_>>(),
            "active": self.active, "engine": {"id": e.id(), "name": e.name()},
            "bar": bar, "clean": self.cfg.read().unwrap().clean_urls, "bm": self.bm.bar(), "starred": self.tab(self.active).is_some_and(|t| self.bm.starred(&t.url)), "update": self.update.as_ref().map(|u| &u.version),
        });
        let _ = self.chrome.evaluate_script(&format!("rb.state({})", js(&st)));
    }

    fn push_settings(&self) {
        let s = js(&self.settings_json());
        for t in &self.tabs {
            let _ = t.wv.evaluate_script(&format!("window.rbSettings&&rbSettings({s})"));
        }
    }

    fn push_bookmarks(&self, only: Option<u32>) {
        let mut pages = self.tabs.iter().filter(|t| only.map_or(t.url == BOOKMARKS, |id| t.id == id)).peekable();
        if pages.peek().is_none() {
            return;
        }
        let script = format!("window.rbBookmarks&&rbBookmarks({})", serde_json::to_string(&self.bm).unwrap_or_default());
        for t in pages {
            let _ = t.wv.evaluate_script(&script);
        }
    }

    fn bm_changed(&self) {
        self.push();
        self.push_bookmarks(None);
        let px = self.px.clone();
        icons::prefetch(self.bm.items.iter().map(|b| icons::host_of(&b.url)).collect(), move || {
            let _ = px.send_event(Ev::IconsReady);
        });
    }

    fn show_bmfolder(&mut self, path: &str, depth: usize) {
        self.bm_cols.truncate(depth);
        self.bm_cols.push(path.to_owned());
        self.render_bmfolder();
    }

    fn render_bmfolder(&mut self) {
        self.bm_cols.retain(|p| p.is_empty() || self.bm.folders.contains(p));
        if self.bm_cols.is_empty() {
            return self.close_popup();
        }
        let cols: Vec<Value> = self
            .bm_cols
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut items = self.bm.children(p);
                if let Some(next) = self.bm_cols.get(i + 1) {
                    items.iter_mut().filter(|it| it["path"] == next.as_str()).for_each(|it| it["on"] = json!(true));
                }
                if items.is_empty() {
                    items.push(json!({"title": "(empty)"}));
                }
                json!({"path": p, "items": items})
            })
            .collect();
        let (x, y) = self.bm_anchor;
        self.show_popup("bmfolder", x, y, 0.0, Value::Array(cols));
    }

    fn show_bm_rename(&mut self, path: &str) {
        let leaf = path.rsplit('/').next().unwrap_or(path);
        let items = json!([{"id": 0, "title": leaf, "folder": path, "folders": [], "mode": "rename"}]);
        let (x, y) = self.bm_anchor;
        self.show_popup("bm", x, y, 0.0, items);
    }

    fn show_bm(&mut self, id: u32, added: bool) {
        let Some(b) = self.bm.items.iter().find(|b| b.id == id) else { return };
        let items = json!([{"id": id, "title": b.title, "folder": b.folder, "folders": self.bm.folders, "added": added}]);
        let (x, y) = self.bm_anchor;
        self.show_popup("bm", x, y, 0.0, items);
    }

    fn show_bm_new(&mut self, folder: bool) {
        let items = json!([{"id": 0, "title": "", "folder": null, "folders": self.bm.folders, "mode": if folder { "folder" } else { "add" }}]);
        let (x, y) = self.bm_anchor;
        self.show_popup("bm", x, y, 0.0, items);
    }

    fn push_downloads(&self) {
        let d = js(&Value::Array(self.dls.iter().rev().map(Dl::view).collect()));
        for t in &self.tabs {
            let _ = t.wv.evaluate_script(&format!("window.rbDownloads&&rbDownloads({d})"));
        }
    }

    fn dl_changed(&mut self) {
        downloads::save(&mut self.dls);
        self.push_downloads();
        if self.popup_kind == "dl" {
            self.show_dl_popup();
        }
    }

    fn to_onboarding(&self, code: &str) {
        for t in self.tabs.iter().filter(|t| t.url == ONBOARDING) {
            let _ = t.wv.evaluate_script(code);
        }
    }

    fn push_history(&self) {
        let h = js(&Value::Array(self.history.recent()));
        for t in &self.tabs {
            let _ = t.wv.evaluate_script(&format!("window.rbHistory&&rbHistory({h})"));
        }
    }

    fn ext_list(&mut self, page: bool) {
        self.ext_page = page;
        let px = self.px.clone();
        extensions::list(&self.chrome, move |l| {
            let _ = px.send_event(Ev::Exts(l));
        });
    }

    fn ext_upd(&self, v: Value) {
        for t in &self.tabs {
            let _ = t.wv.evaluate_script(&format!("window.rbExtUpd&&rbExtUpd({})", js(&v)));
        }
    }

    fn ext_update(&mut self, id: String, ver: String, silent: bool) {
        self.ext_busy = true;
        if !silent {
            self.ext_upd(json!({"id": id, "s": "busy"}));
        }
        let px = self.px.clone();
        std::thread::spawn(move || {
            let r = extupdate::targets().into_iter().find(|t| t.id == id).ok_or_else(|| "Extension cannot be updated".to_owned()).and_then(|t| extupdate::stage(t.src));
            let _ = px.send_event(Ev::ExtSwap(id, ver, r, silent));
        });
    }

    fn ext_next(&mut self) {
        if !self.ext_busy && !self.upd_q.is_empty() {
            let (id, ver) = self.upd_q.remove(0);
            self.ext_update(id, ver, true);
        }
    }

    fn ext_status(&self, v: Value) {
        for t in &self.tabs {
            let _ = t.wv.evaluate_script(&format!("window.rbExtStatus&&rbExtStatus({})", js(&v)));
        }
    }

    fn ext_install(&mut self, src: impl FnOnce() -> Result<extinstall::Source, String> + Send + 'static) {
        if self.ext_busy {
            return self.ext_status(json!({"s": "err", "m": "Another extension is being installed"}));
        }
        self.ext_busy = true;
        self.ext_status(json!({"s": "busy", "m": "Starting"}));
        let px = self.px.clone();
        std::thread::spawn(move || {
            let say = |m: &str| {
                let _ = px.send_event(Ev::ExtProgress(m.to_owned()));
            };
            let _ = px.send_event(Ev::ExtInstalled(src().and_then(|s| extinstall::install(s, &extensions::dir(), &say))));
        });
    }

    fn push_sugs(&self, q: &str, engine: &[String]) {
        let direct = is_url(q.trim()).then(|| self.resolve(q));
        let items = js(&Value::Array(self.history.suggest(q, direct, engine)));
        let _ = self.chrome.evaluate_script(&format!("rb.sugs({items},{},{})", js(&json!(q)), js(&json!(self.history.complete(q)))));
    }

    fn hist_sched(&mut self) {
        if (self.history.dirty || self.bm.dirty) && !std::mem::replace(&mut self.hist_wait, true) {
            let px = self.px.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let _ = px.send_event(Ev::HistSave);
            });
        }
    }

    fn tab(&self, id: u32) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn make_tab(&mut self, url: &str, visible: bool, private: bool) -> Tab {
        let id = self.next;
        self.next += 1;
        let (w, h) = self.size();
        let ch = chrome_h(self.bar_for(url));
        let px = self.px.clone();
        let hw = self.cfg.read().unwrap().hardware_accel;
        let (nav_px, title_px, load_px, new_px, dl_px, done_px) = (px.clone(), px.clone(), px.clone(), px.clone(), px.clone(), px.clone());
        let (icon_px, prog_px) = (px.clone(), px.clone());
        let (key_px, src_px, fs_px) = (px.clone(), px.clone(), px.clone());
        let wv = builder(&mut self.ctx, &px, Src::Tab(id), hw)
            .with_bounds(rect(0.0, ch, w, h - ch))
            .with_visible(visible)
            .with_focused(false)
            .with_incognito(private)
            .with_url(url)
            .with_initialization_script(STORE_JS)
            .with_navigation_handler(move |u| {
                if extinstall::is_package_url(&u) {
                    let _ = nav_px.send_event(Ev::PackageLink(id, u));
                    return false;
                }
                let _ = nav_px.send_event(Ev::Url(id, u));
                true
            })
            .with_document_title_changed_handler(move |t| {
                let _ = title_px.send_event(Ev::Title(id, t));
            })
            .with_on_page_load_handler(move |e, u| {
                let _ = load_px.send_event(Ev::Load(id, matches!(e, PageLoadEvent::Started), u));
            })
            .with_new_window_req_handler(move |u, _| {
                let _ = new_px.send_event(Ev::NewWin(id, u, private, keys::new_tab_click()));
                NewWindowResponse::Deny
            })
            .with_download_started_handler(move |u, p| {
                if extinstall::is_package_url(&u) {
                    let _ = dl_px.send_event(Ev::PackageLink(id, u));
                    return false;
                }
                let own = fetch::own(&u);
                let _ = dl_px.send_event(Ev::DlStart(id, u, p.clone()));
                !own
            })
            .with_download_completed_handler(move |u, p, ok| {
                let _ = done_px.send_event(Ev::DlDone(u, p, ok));
            })
            .build_as_child(&self.win)
            .expect("tab webview");
        apply_autofill(&wv, self.cfg.read().unwrap().autofill);
        if !visible && self.cfg.read().unwrap().memory_saver {
            saver::level(&wv, true);
        }
        keys::watch(&wv, move |k| {
            let _ = key_px.send_event(Ev::Key(k));
        });
        keys::watch_fs(&wv, move |on| {
            let _ = fs_px.send_event(Ev::PageFs(id, on));
        });
        source::watch(&wv, move |u| {
            let _ = src_px.send_event(Ev::Source(id, u));
        });
        favicon::watch(&wv, move |i| {
            let _ = icon_px.send_event(Ev::Icon(id, i));
        });
        downloads::watch(&wv, move |u, r, t| {
            let _ = prog_px.send_event(Ev::DlProgress(u, r, t));
        });
        Tab { id, wv, title: "New Tab".into(), url: url.to_owned(), loading: false, pinned: false, private, icon: String::new(), scroll: 0.0, seek: false, hidden: (!visible).then(Instant::now), sus: false }
    }

    fn new_tab(&mut self, url: &str) {
        self.open_tab(url, false);
    }

    fn open_tab(&mut self, url: &str, private: bool) {
        let t = match self.spare.take() {
            Some(t) if url == NEWTAB && !private => t,
            s => {
                self.spare = s;
                self.make_tab(url, true, private)
            }
        };
        let id = t.id;
        self.tabs.push(t);
        self.select(id);
        if self.spare.is_none() {
            let px = self.px.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let _ = px.send_event(Ev::Spare);
            });
        }
    }

    fn saver_apply(&mut self) {
        let on = self.cfg.read().unwrap().memory_saver;
        for t in &mut self.tabs {
            saver::level(&t.wv, on && t.hidden.is_some());
            if !on && std::mem::take(&mut t.sus) {
                saver::resume(&t.wv);
            }
        }
    }

    fn saver_tick(&mut self) {
        if !self.cfg.read().unwrap().memory_saver {
            return;
        }
        let warm = saver::warm();
        if self.spare.is_some() && self.spare_at.elapsed() >= warm {
            self.spare = None;
        }
        if self.popup_kind.is_empty() && self.popup.is_some() && self.popup_at.elapsed() >= warm {
            self.popup = None;
            self.popup_ready = false;
        }
        let limit = saver::idle();
        let downloading = self.dls.iter().any(|d| d.state == "active");
        for t in &mut self.tabs {
            let hidden_for = t.hidden.map(|h| h.elapsed());
            let st = saver::Tab { hidden_for, suspended: t.sus, pinned: t.pinned, loading: t.loading, restoring: t.seek, audio: hidden_for.is_some_and(|d| d >= limit) && saver::playing_audio(&t.wv) };
            if saver::eligible(&st, limit, downloading) {
                t.hidden = Some(Instant::now());
                let (px, id) = (self.px.clone(), t.id);
                saver::suspend(&t.wv, move |ok| {
                    let _ = px.send_event(Ev::Suspended(id, ok));
                });
            }
        }
    }

    fn resume(&self) -> bool {
        self.cfg.read().unwrap().resume_tabs
    }

    fn scroll_of(&self, id: u32) {
        let Some(t) = self.tab(id).filter(|t| !t.private && !t.seek) else { return };
        let px = self.px.clone();
        let _ = t.wv.evaluate_script_with_callback("window.scrollY", move |v| {
            if let Ok(y) = v.parse::<f64>() {
                let _ = px.send_event(Ev::Scroll(id, y));
            }
        });
    }

    fn sess_save(&mut self, sync: bool) {
        if !self.resume() || !self.rest.is_empty() {
            return;
        }
        let active = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        let s = session::capture(self.tabs.iter().map(|t| (session::Entry { url: t.url.clone(), title: t.title.clone(), pinned: t.pinned, scroll: t.scroll }, t.private)), active);
        if s.tabs.is_empty() {
            return;
        }
        let b = session::encode(&s);
        if b == self.sess {
            return;
        }
        self.sess = b.clone();
        if sync { session::write(&b) } else { session::write_bg(b) }
    }

    fn restore_next(&self, ms: u64) {
        let px = self.px.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            let _ = px.send_event(Ev::Restore);
        });
    }

    fn seed(t: &mut Tab, e: &session::Entry) {
        t.pinned = e.pinned;
        t.scroll = e.scroll;
        t.seek = e.scroll > 0.0;
        if !e.title.is_empty() {
            t.title = e.title.clone();
        }
    }

    fn start(&mut self, home: &str, arg: Option<String>) {
        let Some(s) = session::load().filter(|_| arg.is_none() && self.resume()) else {
            return self.new_tab(&arg.unwrap_or_else(|| home.into()));
        };
        let first = home == ONBOARDING;
        if first {
            self.new_tab(home);
        }
        for (i, e) in s.tabs.into_iter().enumerate() {
            if first || i != s.active {
                self.rest.push_back((first || i > s.active, e));
            } else {
                self.open_tab(&e.url, false);
                if let Some(t) = self.tabs.last_mut() {
                    Self::seed(t, &e);
                }
            }
        }
        self.rest_head = self.active;
        self.rest_tail = self.active;
        self.push();
        self.restore_next(400);
    }

    fn restore_one(&mut self) {
        let Some((after, e)) = self.rest.pop_front() else { return };
        let mut t = self.make_tab(&e.url, false, false);
        Self::seed(&mut t, &e);
        let anchor = if after { self.rest_tail } else { self.rest_head };
        let at = self.tabs.iter().position(|x| x.id == anchor).map_or(self.tabs.len(), |p| p + after as usize);
        if after {
            self.rest_tail = t.id;
        }
        self.tabs.insert(at, t);
        self.push();
        if !self.rest.is_empty() {
            self.restore_next(150);
        }
    }

    fn select(&mut self, id: u32) {
        let prev = std::mem::replace(&mut self.active, id);
        if prev != id && self.resume() {
            self.scroll_of(prev);
        }
        self.layout();
        let saver = self.cfg.read().unwrap().memory_saver;
        for t in &mut self.tabs {
            let show = t.id == id;
            let _ = t.wv.set_visible(show);
            if show {
                t.hidden = None;
                if std::mem::take(&mut t.sus) {
                    saver::resume(&t.wv);
                }
            } else {
                t.hidden.get_or_insert_with(Instant::now);
            }
            if saver && (show || t.id == prev) {
                saver::level(&t.wv, !show);
            }
        }
        if self.fs.p > 0.0 {
            to_top(&self.chrome);
        }
        if self.tab(id).is_some_and(|t| t.url == NEWTAB) {
            self.reveal();
            self.chrome.focus().ok();
            let _ = self.chrome.evaluate_script("q.focus()");
        } else if let Some(t) = self.tab(id) {
            let _ = t.wv.focus();
        }
        self.close_popup();
        self.push();
    }

    fn close_tab(&mut self, id: u32) {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let t = self.tabs.remove(i);
        if !t.private {
            self.closed.push(t.url);
        }
        if self.tabs.is_empty() {
            self.quit = true;
        } else if self.active == id {
            let n = self.tabs[i.min(self.tabs.len() - 1)].id;
            self.select(n);
        } else {
            self.push();
        }
    }

    fn open_after(&mut self, id: u32, url: &str) {
        let at = self.tabs.iter().position(|t| t.id == id).map_or(self.tabs.len(), |i| i + 1);
        self.new_tab(url);
        let t = self.tabs.pop().unwrap();
        self.tabs.insert(at, t);
        self.push();
    }

    fn toggle_pin(&mut self, id: u32) {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let mut t = self.tabs.remove(i);
        t.pinned = !t.pinned;
        let at = self.tabs.iter().take_while(|t| t.pinned).count();
        self.tabs.insert(at, t);
        self.push();
    }

    fn close_matching(&mut self, keep: u32, f: impl Fn(usize, &Tab) -> bool) {
        let ids: Vec<u32> = self.tabs.iter().enumerate().filter(|(i, t)| t.id != keep && f(*i, t)).map(|(_, t)| t.id).collect();
        for id in ids {
            self.close_tab(id);
        }
        self.select(keep);
    }

    fn open_internal(&mut self, url: &str) {
        match self.tabs.iter().find(|t| t.url == url).map(|t| t.id) {
            Some(id) => self.select(id),
            None => self.new_tab(url),
        }
    }

    fn open_update(&mut self) {
        let Some(u) = self.update.as_ref().map(|u| json!({"show": true, "info": u.view()})) else { return };
        let open = self.tabs.iter().any(|t| t.url == SETTINGS);
        self.open_internal(SETTINGS);
        if open {
            self.page_call("rbUpdate", &u);
        } else {
            self.upd_show = true;
        }
    }

    fn page_call(&self, f: &str, v: &Value) {
        let s = js(v);
        for t in self.tabs.iter().filter(|t| t.url == SETTINGS) {
            let _ = t.wv.evaluate_script(&format!("window.{f}&&{f}({s})"));
        }
    }

    fn update_apply(&mut self) {
        if self.upd_cancel.is_some() {
            return;
        }
        let Some(rel) = self.update.clone() else { return };
        let (cancel, px) = (Arc::new(AtomicBool::new(false)), self.px.clone());
        self.upd_cancel = Some(cancel.clone());
        std::thread::spawn(move || {
            let dest = update::staging().join("ferrum.exe");
            let last = Cell::new(Instant::now() - Duration::from_secs(1));
            let r = update::download(&rel, &dest, &cancel, &|got, total| {
                if last.get().elapsed() > Duration::from_millis(120) {
                    last.set(Instant::now());
                    let _ = px.send_event(Ev::UpdProg(got, total));
                }
            });
            let _ = px.send_event(Ev::UpdDone(r.map(|_| dest)));
        });
    }

    fn resolve(&self, q: &str) -> String {
        let q = q.trim();
        if has_scheme(q) {
            q.to_owned()
        } else if is_url(q) {
            format!("{}://{q}", if q.split(['/', ':', '?']).next() == Some("localhost") { "http" } else { "https" })
        } else {
            self.cfg.read().unwrap().engine.url(q)
        }
    }

    fn navigate(&mut self, id: u32, q: &str) {
        if !is_url(q.trim()) && !self.tab(id).is_some_and(|t| t.private) {
            self.history.search(q);
        }
        if let Some(t) = self.tab(id) {
            let _ = t.wv.load_url(&self.resolve(q));
        }
    }

    fn ensure_popup(&mut self) {
        if self.popup.is_none() {
            let hw = self.cfg.read().unwrap().hardware_accel;
            let wv = builder(&mut self.ctx, &self.px, Src::Popup, hw).with_bounds(rect(0.0, 0.0, 1.0, 1.0)).with_visible(false).with_url("http://rb.localhost/popup").build_as_child(&self.win).expect("popup webview");
            self.popup = Some(wv);
            self.popup_ready = false;
        }
    }

    fn close_popup(&mut self) {
        self.popup_wait = None;
        if let Some(p) = &self.popup {
            let _ = p.set_visible(false);
        }
        self.popup_kind = "";
    }

    fn show_popup(&mut self, kind: &'static str, x: f64, y: f64, w: f64, items: Value) {
        let n = items.as_array().map_or(0, Vec::len) as f64;
        if n == 0.0 && !matches!(kind, "dl" | "ext") {
            return self.close_popup();
        }
        self.ensure_popup();
        self.popup_at = Instant::now();
        if !self.popup_ready {
            self.popup_wait = Some((kind, x, y, w, items));
            return;
        }
        let Some(popup) = self.popup.as_ref() else { return };
        let (win_w, win_h) = self.size();
        let w = match kind {
            "eng" => 240.0,
            "menu" => 200.0,
            "tab" => 220.0,
            "ext" => 340.0,
            "dl" => 380.0,
            "bm" => 340.0,
            "bmfolder" => (260.0 * n).min(win_w),
            _ => w,
        };
        let x = if matches!(kind, "menu" | "ext" | "dl" | "bm") { (x - w).max(0.0) } else { x.min(win_w - w).max(0.0) };
        let seps = items.as_array().map_or(0, |a| a.iter().filter(|i| i["sep"] == true).count()) as f64;
        let h = match kind {
            "dl" => n.max(1.0) * 64.0 + 62.0,
            "ext" => ((if n == 0.0 { 64.0 } else { n * EXT_ROW_H }) + 62.0).min(win_h - 16.0),
            "bm" if items[0]["mode"] == "add" => 264.0,
            "bm" if items[0]["mode"] == "rename" => 150.0,
            "bmfolder" => {
                let rows = items.as_array().into_iter().flatten().map(|c| c["items"].as_array().map_or(0, Vec::len)).max().unwrap_or(0);
                (rows as f64 * ROW_H + 13.0).max(120.0).min(win_h - y.min(win_h) - 8.0)
            }
            "bm" => 216.0,
            _ => (n * ROW_H + seps * 9.0 + 13.0).min(win_h - 16.0),
        };
        let y = y.min(win_h - h).max(0.0);
        let _ = popup.set_bounds(rect(x, y, w, h));
        let auto = kind == "dl" && self.dl_auto;
        let _ = popup.evaluate_script(&format!("pop.render({},{},{auto})", js(&json!(kind)), js(&items)));
        if kind == "sug" && self.popup_kind == "sug" {
            return;
        }
        let _ = popup.set_visible(true);
        to_top(popup);
        let fresh = self.popup_kind != kind;
        self.popup_kind = kind;
        if kind == "sug" {
            let _ = self.chrome.focus();
        } else if fresh {
            let _ = popup.focus();
        }
    }

    fn set_dl_anchor(&mut self, x: f64, y: f64) {
        self.dl_anchor = (x, y);
        if self.dl_want || self.popup_kind == "dl" {
            self.dl_want = false;
            self.show_dl_popup();
        }
    }

    fn request_dl_anchor(&mut self) {
        if self.immersive() && self.fs.p == 0.0 {
            let (w, _) = self.size();
            self.set_dl_anchor(w - 8.0, chrome_h(self.bar()));
        } else {
            let _ = self.chrome.evaluate_script("rb.anchor()");
        }
    }

    fn start_fetch(&mut self, id: u32, resume: Option<(u64, Vec<(u64, u64)>)>) {
        let Some(i) = self.dls.iter().position(|d| d.id == id) else { return };
        let cookie = self
            .tab(self.active)
            .and_then(|t| t.wv.cookies_for_url(&self.dls[i].url).ok())
            .map(|c| c.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; "))
            .unwrap_or_default();
        let d = &mut self.dls[i];
        let rt = Arc::new(fetch::Rt::default());
        d.rt = Some(rt.clone());
        d.state = "active".into();
        let (req, dest, px) = (fetch::Req { url: d.url.clone(), cookie, referer: d.referer.clone() }, d.path.clone(), self.px.clone());
        std::thread::spawn(move || {
            fetch::run(&req, &dest, resume, &rt, &|m| {
                let _ = px.send_event(Ev::Fetch(id, m));
            })
        });
    }

    fn fetch_msg(&mut self, id: u32, msg: fetch::Msg) {
        let Some(i) = self.dls.iter().position(|d| d.id == id) else { return };
        let d = &mut self.dls[i];
        match msg {
            fetch::Msg::Info { total, ranged } => {
                d.total = total;
                d.ranged = ranged;
            }
            fetch::Msg::Progress(r, t, chunks) => {
                d.received = r;
                d.total = d.total.max(t);
                if d.ranged {
                    d.chunks = chunks;
                }
                if d.tick() {
                    self.push_downloads();
                    if self.popup_kind == "dl" {
                        self.show_dl_popup();
                    }
                }
            }
            fetch::Msg::Paused => {
                d.state = "paused".into();
                d.speed = 0.0;
                self.dl_changed();
            }
            fetch::Msg::Cancelled => {
                self.dls.remove(i);
                self.dl_changed();
            }
            fetch::Msg::Done => {
                d.state = "done".into();
                d.chunks.clear();
                d.size = std::fs::metadata(&d.path).map_or(0, |m| m.len());
                self.dl_changed();
            }
            fetch::Msg::Failed => {
                d.state = "fail".into();
                self.dl_changed();
            }
        }
    }

    fn show_dl_popup(&mut self) {
        let items: Vec<Value> = self
            .dls
            .iter()
            .rev()
            .take(5)
            .map(Dl::view)
            .collect();
        let (x, y) = self.dl_anchor;
        self.show_popup("dl", x, y, 0.0, Value::Array(items));
    }

    fn set(&mut self, k: &str, v: &Value) {
        {
            let mut s = self.cfg.write().unwrap();
            let b = v.as_bool().unwrap_or(true);
            match k {
                "engine" => {
                    if let Some(e) = v.as_str().and_then(|s| s.parse::<usize>().ok()).and_then(|i| Engine::ALL.get(i)) {
                        s.engine = *e;
                    }
                }
                "hardware_accel" => s.hardware_accel = b,
                "bookmarks_bar" => s.bookmarks_bar = b,
                "autofill" => s.autofill = b,
                "links_in_new_tab" => s.links_in_new_tab = b,
                "clean_urls" => s.clean_urls = b,
                "focus_mode" => s.focus_mode = b,
                "auto_update_ext" => s.auto_update_ext = b,
                "dl_popup" => s.dl_popup = b,
                "resume_tabs" => s.resume_tabs = b,
                "memory_saver" => s.memory_saver = b,
                _ => return,
            }
            s.save();
        }
        if k == "focus_mode" {
            self.fs.on = self.cfg.read().unwrap().focus_mode;
            self.fs.full &= self.fs.on;
            self.sync_fs();
        }
        if k == "resume_tabs" && !self.resume() {
            self.sess.clear();
            session::clear();
        }
        if k == "memory_saver" {
            self.saver_apply();
        }
        if k == "autofill" {
            let on = self.cfg.read().unwrap().autofill;
            for t in self.tabs.iter().chain(self.spare.iter()) {
                apply_autofill(&t.wv, on);
            }
        }
        self.push();
        self.push_settings();
    }

    fn msg(&mut self, src: Src, text: &str) {
        let Ok(m) = serde_json::from_str::<Value>(text) else { return };
        let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
        let n = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let id = match src {
            Src::Tab(id) => id,
            _ => self.active,
        };
        let from_page = matches!(src, Src::Tab(_));
        match s("c") {
            "ready" => {
                self.push();
                if self.tab(self.active).is_some_and(|t| t.url == NEWTAB) {
                    let _ = self.chrome.evaluate_script("q.focus()");
                }
            }
            "nt_text" if from_page && id == self.active => {
                self.chrome.focus().ok();
                let _ = self.chrome.evaluate_script(&format!("rb.seed({})", js(&json!(s("v")))));
            }
            "nav" => self.navigate(id, s("q")),
            "pick" => {
                self.close_popup();
                let _ = self.chrome.evaluate_script(&format!("go({})", js(&json!(s("q")))));
            }
            "settings_get" => self.push_settings(),
            "update_check" if from_page => {
                let px = self.px.clone();
                std::thread::spawn(move || {
                    let _ = px.send_event(Ev::Update(true, update::check()));
                });
            }
            "update_open" if from_page => self.open_update(),
            "update_get" if from_page => {
                if let Some(u) = &self.update {
                    let v = json!({"show": std::mem::take(&mut self.upd_show), "info": u.view()});
                    self.page_call("rbUpdate", &v);
                }
            }
            "update_apply" if from_page => self.update_apply(),
            "update_cancel" if from_page => {
                if let Some(c) = &self.upd_cancel {
                    c.store(true, Ordering::Relaxed);
                }
            }
            "update_release" if from_page => {
                if let Some(u) = self.update.as_ref().map(|u| u.url.clone()).filter(|u| u.starts_with("https://github.com/")) {
                    self.open_tab(&u, false);
                }
            }
            "downloads_get" => self.push_downloads(),
            "bm_get" => self.push_bookmarks(Some(id)),
            "bm_dir" if src == Src::Popup => self.show_bmfolder(s("f"), n("d") as usize),
            "bm_do" if src == Src::Popup => match s("a") {
                "edit" => self.show_bm(n("id") as u32, false),
                "rename" => self.show_bm_rename(s("f")),
                "rm" => {
                    self.bm.remove(n("id") as u32);
                    self.bm_changed();
                    self.render_bmfolder();
                }
                "rmdir" => {
                    self.bm.remove_folder(s("f"));
                    self.bm_changed();
                    self.render_bmfolder();
                }
                _ => {}
            },
            "bm_frename" if src == Src::Popup => {
                self.bm.rename_folder(s("f"), s("name"));
                self.bm_changed();
            }
            "bookmarks" if src == Src::Chrome => self.open_internal(BOOKMARKS),
            "bm_import" if from_page => {
                let px = self.px.clone();
                std::thread::spawn(move || {
                    let Some(p) = rfd::FileDialog::new().add_filter("All bookmark files", &["json", "html", "htm", "csv"]).add_filter("HTML", &["html", "htm"]).add_filter("JSON", &["json"]).add_filter("CSV", &["csv"]).pick_file() else { return };
                    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                    if let Some(list) = std::fs::read(&p).ok().and_then(|d| bookmarks_io::parse(&ext, &d).ok()) {
                        let _ = px.send_event(Ev::BmImport(list));
                    }
                });
            }
            "ob_open" if from_page => self.open_internal(ONBOARDING),
            "ob_detect" if from_page => {
                let px = self.px.clone();
                std::thread::spawn(move || {
                    let _ = px.send_event(Ev::ObBrowsers(import::list()));
                });
            }
            "ob_import" if from_page => {
                let sel: Vec<(String, String)> = m["sel"].as_array().map(|a| a.iter().filter_map(|p| Some((p[0].as_str()?.to_owned(), p[1].as_str()?.to_owned()))).collect()).unwrap_or_default();
                let on = |k: &str| m["what"][k] == true;
                let what = import::What { bookmarks: on("bookmarks"), history: on("history"), cookies: on("cookies"), extensions: on("extensions") };
                let px = self.px.clone();
                std::thread::spawn(move || {
                    import::run(&sel, &what, &mut |o| {
                        let _ = px.send_event(Ev::ObOut(o));
                    });
                    let _ = px.send_event(Ev::ObDone);
                });
            }
            "ob_finish" if from_page => {
                let mut c = self.cfg.write().unwrap();
                c.onboarded = true;
                c.save();
                drop(c);
                if let Some(t) = self.tab(id) {
                    let _ = t.wv.load_url(NEWTAB);
                }
            }
            "bm_export" if from_page && bookmarks_io::FORMATS.contains(&s("f")) => {
                let fmt = s("f").to_owned();
                let data = bookmarks_io::export(&self.bm, &fmt);
                std::thread::spawn(move || {
                    if let Some(p) = rfd::FileDialog::new().set_file_name(format!("bookmarks.{fmt}")).add_filter(&fmt, &[fmt.as_str()]).save_file() {
                        let _ = std::fs::write(p, data);
                    }
                });
            }
            "bm_star" if src == Src::Chrome && self.popup_kind != "bm" => {
                self.bm_anchor = (n("x"), n("y"));
                let Some((url, title)) = self.tab(self.active).map(|t| (t.url.clone(), t.title.clone())) else { return };
                match self.bm.items.iter().find(|b| b.url == url).map(|b| b.id) {
                    Some(id) => self.show_bm(id, false),
                    None => {
                        let id = self.bm.add(&title, &url);
                        self.bm_changed();
                        self.show_bm(id, true);
                    }
                }
            }
            "bm_save" if src == Src::Popup => {
                let mut id = n("id") as u32;
                if id == 0 {
                    let u = s("url").trim();
                    if u.is_empty() || self.bm.starred(u) {
                        return;
                    }
                    id = self.bm.add(s("name"), &if has_scheme(u) { u.to_owned() } else { format!("https://{u}") });
                    self.bm.items.rotate_right(1);
                }
                self.bm.update(id, s("name"), s("folder"));
                self.bm_changed();
            }
            "bm_mkdir" if src == Src::Popup => {
                self.bm.new_folder(s("folder"), s("name"));
                self.bm_changed();
            }
            "bm_move" if from_page => {
                self.bm.update(n("id") as u32, "", s("folder"));
                self.bm_changed();
            }
            "bm_rmdir" if from_page => {
                self.bm.remove_folder(s("f"));
                self.bm_changed();
            }
            "bm_clear" if from_page => {
                self.bm.clear();
                self.bm_changed();
            }
            "bm_remove" if src != Src::Chrome => {
                self.bm.remove(n("id") as u32);
                self.bm_changed();
                self.close_popup();
            }
            "bm_open" if !from_page => {
                if let Some(u) = self.bm.items.iter().find(|b| b.id == n("id") as u32).map(|b| b.url.clone()) {
                    if m["new"] == true {
                        self.new_tab(&u);
                    } else {
                        self.navigate(self.active, &u);
                    }
                }
                self.close_popup();
            }
            "set" if from_page => self.set(s("k"), &m["v"]),
            "history_get" => self.push_history(),
            "history_clear" => {
                self.history.clear();
                self.push_history();
            }
            "show_dl" => {
                match downloads::find(&self.dls, n("id") as u32) {
                    Some(d) => {
                        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", d.path.display())).spawn();
                    }
                    None => self.dl_changed(),
                }
            }
            "dl_open" if from_page || src == Src::Popup => match downloads::find(&self.dls, n("id") as u32) {
                Some(d) => {
                    let p: Vec<u16> = d.path.as_os_str().encode_wide().chain([0]).collect();
                    unsafe { ShellExecuteW(None, w!("open"), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
                    if self.popup_kind == "dl" {
                        self.close_popup();
                    }
                }
                None => self.dl_changed(),
            },
            "dl_delete" if from_page => {
                if let Some(d) = self.dls.iter().find(|d| d.id == n("id") as u32 && d.state == "done") {
                    let _ = std::fs::remove_file(&d.path);
                }
                self.dl_changed();
            }
            "dl_remove" if from_page => {
                let id = n("id") as u32;
                self.dls.iter().filter(|d| d.id == id && d.state == "paused").for_each(Dl::discard);
                self.dls.retain(|d| d.id != id || d.state == "active");
                self.dl_changed();
            }
            "dl_clear" if from_page => {
                self.dls.iter().filter(|d| d.state == "paused").for_each(Dl::discard);
                self.dls.retain(|d| d.state == "active");
                self.dl_changed();
            }
            "dl_pause" if from_page || src == Src::Popup => {
                if let Some(rt) = self.dls.iter().find(|d| d.id == n("id") as u32 && d.ranged).and_then(|d| d.rt.as_ref()) {
                    rt.paused.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            "dl_resume" if from_page || src == Src::Popup => {
                let id = n("id") as u32;
                if let Some(d) = self.dls.iter().find(|d| d.id == id && d.state == "paused") {
                    let r = (d.total, d.chunks.clone());
                    self.start_fetch(id, Some(r));
                    self.dl_changed();
                }
            }
            "dl_cancel" if from_page || src == Src::Popup => {
                let id = n("id") as u32;
                match self.dls.iter().find(|d| d.id == id).map(|d| d.state.as_str()) {
                    Some("paused") => {
                        self.dls.iter().filter(|d| d.id == id).for_each(Dl::discard);
                        self.dls.retain(|d| d.id != id);
                        self.dl_changed();
                    }
                    Some("active") => {
                        if let Some(rt) = self.dls.iter().find(|d| d.id == id).and_then(|d| d.rt.as_ref()) {
                            rt.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
            }
            "back" => self.eval_active("history.back()"),
            "fwd" => self.eval_active("history.forward()"),
            "reload" => match self.tab(self.active) {
                Some(t) if t.loading => self.eval_active("window.stop()"),
                Some(t) => {
                    let _ = t.wv.reload();
                }
                None => {}
            },
            "newtab" => self.new_tab(NEWTAB),
            "close" => self.close_tab(n("id") as u32),
            "select" => self.select(n("id") as u32),
            "settings" => self.open_internal(SETTINGS),
            "downloads" => self.open_internal(DOWNLOADS),
            "ext_get" => self.ext_list(true),
            "ext_add" if from_page => {
                let (pick, q) = (s("pick").to_owned(), s("q").to_owned());
                self.ext_install(move || match pick.as_str() {
                    "file" => rfd::FileDialog::new().add_filter("Extension", &["crx", "xpi", "zip"]).pick_file().map(extinstall::Source::Path).ok_or_else(String::new),
                    "folder" => rfd::FileDialog::new().pick_folder().map(extinstall::Source::Path).ok_or_else(String::new),
                    _ => extinstall::parse(&q),
                });
            }
            "ext_check" if from_page => {
                let (id, px) = (s("id").to_owned(), self.px.clone());
                std::thread::spawn(move || {
                    let send = |v: Value| {
                        let _ = px.send_event(Ev::ExtUpd(v));
                    };
                    for t in extupdate::targets().into_iter().filter(|t| id.is_empty() || t.id == id) {
                        send(json!({"id": t.id, "s": "checking"}));
                        send(match extupdate::check(&t.src, &t.current) {
                            Ok(Some(v)) => json!({"id": t.id, "s": "new", "v": v}),
                            Ok(None) => json!({"id": t.id, "s": "ok"}),
                            Err(e) => json!({"id": t.id, "s": "err", "m": e}),
                        });
                    }
                });
            }
            "ext_update" if from_page => {
                if self.ext_busy {
                    return self.ext_upd(json!({"id": s("id"), "s": "err", "m": "Another extension is being installed"}));
                }
                self.ext_update(s("id").to_owned(), s("v").to_owned(), false);
            }
            "ext_open" if src != Src::Chrome => {
                if let Some(u) = extensions::page_url(s("id"), s("p")) {
                    self.close_popup();
                    self.new_tab(&u);
                }
            }
            "ext_remove" if src != Src::Chrome => {
                let (id, px) = (s("id").to_owned(), self.px.clone());
                extensions::remove(&self.chrome, id.clone(), move |ok| {
                    if ok {
                        let _ = px.send_event(Ev::ExtRemoved(id));
                    }
                });
            }
            "ext_set" => extensions::enable(&self.chrome, s("id").to_owned(), m["on"].as_bool().unwrap_or(true)),
            "min" => self.win.set_minimized(true),
            "max" if self.fs.full => self.key("fullscreen"),
            "max" => {
                self.win.set_maximized(!self.win.is_maximized());
                self.layout();
            }
            "qf" if src == Src::Chrome => self.fs.addr = n("on") > 0.0,
            "quit" => self.quit = true,
            "drag" => {
                let _ = self.win.drag_window();
            }
            "suggest" => {
                let (q, e, px) = (s("q").to_owned(), self.cfg.read().unwrap().engine, self.px.clone());
                self.push_sugs(&q, &[]);
                std::thread::spawn(move || {
                    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(2))).build().into();
                    let list = agent
                        .get(e.suggest_url(&q))
                        .call()
                        .ok()
                        .and_then(|mut r| r.body_mut().read_json::<Value>().ok())
                        .and_then(|v| v.get(1).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).take(6).collect()))
                        .unwrap_or_default();
                    let _ = px.send_event(Ev::Sugs(q, list));
                });
            }
            "popup" if src == Src::Chrome => {
                let e = self.cfg.read().unwrap().engine;
                let (x, y, w) = (n("x"), n("y"), n("w"));
                match s("k") {
                    "sug" => self.show_popup("sug", x, y, w, m["items"].clone()),
                    "eng" if self.popup_kind != "eng" => {
                        let items = Engine::ALL.iter().map(|k| json!({"id": k.id(), "name": k.name(), "on": *k == e})).collect();
                        self.show_popup("eng", x, y, w, Value::Array(items));
                    }
                    "ext" if self.popup_kind != "ext" => {
                        self.anchor = (x, y);
                        self.ext_list(false);
                    }
                    "dl" if self.popup_kind != "dl" => {
                        self.dl_anchor = (x, y);
                        self.dl_auto = false;
                        self.show_dl_popup();
                    }
                    "tab" => {
                        let Some(i) = self.tabs.iter().position(|t| t.id == n("id") as u32) else { return };
                        let (cur, pinned) = (&self.tabs[i], self.tabs[i].pinned);
                        let more = |f: &dyn Fn(usize, &Tab) -> bool| self.tabs.iter().enumerate().any(|(j, t)| t.id != cur.id && !t.pinned && f(j, t));
                        let mut items = vec![
                            json!({"id":"t_new","label":"New tab to the right"}),
                            json!({"id":"t_reload","label":"Reload tab","sep":true}),
                            json!({"id":"t_pin","label":if pinned {"Unpin tab"} else {"Pin tab"}}),
                            json!({"id":"t_dup","label":"Duplicate tab"}),
                            json!({"id":"t_close","label":"Close tab","sep":true}),
                        ];
                        if self.tabs.iter().any(|t| t.id != cur.id && t.url == cur.url) {
                            items.push(json!({"id":"t_dups","label":"Close duplicate tabs"}));
                        }
                        if more(&|j, _| j > i) {
                            items.push(json!({"id":"t_right","label":"Close tabs to the right"}));
                        }
                        if more(&|_, _| true) {
                            items.push(json!({"id":"t_others","label":"Close other tabs"}));
                        }
                        if !self.closed.is_empty() {
                            items.push(json!({"id":"t_reopen","label":"Reopen closed tab"}));
                        }
                        self.tab_menu = cur.id;
                        self.show_popup("tab", x, y, w, Value::Array(items));
                    }
                    "bmfolder" => {
                        self.bm_anchor = (x, y);
                        self.show_bmfolder(s("f"), 0);
                    }
                    "bmfctx" => {
                        self.bm_fsel = s("f").to_owned();
                        self.bm_anchor = (x, y);
                        self.show_popup("tab", x, y, w, json!([{"id":"bm_frename","label":"Rename folder"},{"id":"bm_frm","label":"Delete folder"}]));
                    }
                    "bmbar" => {
                        self.bm_anchor = (x, y);
                        self.show_popup("tab", x, y, w, json!([{"id":"bm_new","label":"New bookmark"},{"id":"bm_newfolder","label":"New folder"},{"id":"bookmarks","label":"Open bookmarks page"}]));
                    }
                    "bmctx" => {
                        self.bm_sel = n("id") as u32;
                        self.bm_anchor = (x, y);
                        self.show_popup("tab", x, y, w, json!([{"id":"bm_edit","label":"Edit..."},{"id":"bm_rm","label":"Delete"}]));
                    }
                    "menu" if self.popup_kind != "menu" => {
                        let mut items = vec![json!({"id":"newtab","label":"New tab"}), json!({"id":"private","label":"New private tab"}), json!({"id":"bookmarks","label":"Bookmarks"}), json!({"id":"downloads","label":"Downloads"}), json!({"id":"history","label":"History"}), json!({"id":"settings","label":"Settings"}), json!({"id":"focus","label":"Focus mode"}), json!({"id":"quit","label":"Quit"})];
                        if let Some(u) = &self.update {
                            items.insert(0, json!({"id":"update","label":format!("Update available — v{}", u.version)}));
                        }
                        self.show_popup("menu", x, y, w, Value::Array(items));
                    }
                    _ => self.close_popup(),
                }
            }
            "popup_ready" if src == Src::Popup => {
                self.popup_ready = true;
                if let Some((k, x, y, w, items)) = self.popup_wait.take() {
                    self.show_popup(k, x, y, w, items);
                }
            }
            "popup_close" => match s("only") {
                "sug" if self.popup_kind == "sug" => self.close_popup(),
                "focus" if self.popup_kind != "sug" => self.close_popup(),
                "auto" if self.popup_kind == "dl" => self.close_popup(),
                "" => self.close_popup(),
                _ => {}
            },
            "anchor" if src == Src::Chrome => self.set_dl_anchor(n("x"), n("y")),
            "hl" => {
                if let Some(p) = &self.popup {
                    let _ = p.evaluate_script(&format!("pop.hl({})", n("i")));
                }
            }
            "dl_all" if src == Src::Popup => {
                self.close_popup();
                self.open_internal(DOWNLOADS);
            }
            "engine" if src == Src::Popup => {
                self.set("engine", &json!((n("id") as usize).to_string()));
                self.close_popup();
            }
            "menuitem" if src == Src::Popup => {
                if !matches!(s("id"), "bm_edit" | "bm_new" | "bm_newfolder" | "bm_frename") {
                    self.close_popup();
                }
                match s("id") {
                    "newtab" => self.new_tab(NEWTAB),
                    "private" => self.open_tab(NEWTAB, true),
                    "bookmarks" => self.open_internal(BOOKMARKS),
                    "bm_edit" => self.show_bm(self.bm_sel, false),
                    "bm_new" => self.show_bm_new(false),
                    "bm_newfolder" => self.show_bm_new(true),
                    "bm_frename" => self.show_bm_rename(&self.bm_fsel.clone()),
                    "bm_frm" => {
                        self.bm.remove_folder(&self.bm_fsel.clone());
                        self.bm_changed();
                    }
                    "bm_rm" => {
                        self.bm.remove(self.bm_sel);
                        self.bm_changed();
                    }
                    "downloads" => self.open_internal(DOWNLOADS),
                    "history" => self.open_internal(HISTORY),
                    "settings" => self.open_internal(SETTINGS),
                    "focus" => self.key("focus"),
                    "update" => self.open_update(),
                    "quit" => self.quit = true,
                    "t_new" => self.open_after(self.tab_menu, NEWTAB),
                    "t_reload" => {
                        if let Some(t) = self.tab(self.tab_menu) {
                            let _ = t.wv.reload();
                        }
                    }
                    "t_pin" => self.toggle_pin(self.tab_menu),
                    "t_dup" => {
                        if let Some(u) = self.tab(self.tab_menu).map(|t| t.url.clone()) {
                            self.open_after(self.tab_menu, &u);
                        }
                    }
                    "t_close" => self.close_tab(self.tab_menu),
                    "t_dups" => {
                        if let Some(u) = self.tab(self.tab_menu).map(|t| t.url.clone()) {
                            self.close_matching(self.tab_menu, |_, t| t.url == u);
                        }
                    }
                    "t_right" => {
                        let at = self.tabs.iter().position(|t| t.id == self.tab_menu).unwrap_or(usize::MAX);
                        self.close_matching(self.tab_menu, |i, t| i > at && !t.pinned);
                    }
                    "t_others" => self.close_matching(self.tab_menu, |_, t| !t.pinned),
                    "t_reopen" => {
                        if let Some(u) = self.closed.pop() {
                            self.new_tab(&u);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, k: &str) {
        let at = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        let len = self.tabs.len();
        match k {
            "newtab" => self.msg(Src::Chrome, r#"{"c":"newtab"}"#),
            "close" => self.close_tab(self.active),
            "private" => self.open_tab(NEWTAB, true),
            "reopen" => {
                if let Some(u) = self.closed.pop() {
                    self.new_tab(&u);
                }
            }
            "next" | "prev" if len > 0 => {
                let to = if k == "next" { (at + 1) % len } else { (at + len - 1) % len };
                self.select(self.tabs[to].id);
            }
            "addr" => {
                self.reveal();
                self.chrome.focus().ok();
                let _ = self.chrome.evaluate_script("q.focus()");
            }
            "reload" => self.msg(Src::Chrome, r#"{"c":"reload"}"#),
            "star" => {
                self.reveal();
                self.chrome.focus().ok();
                let _ = self.chrome.evaluate_script("star()");
            }
            "history" => self.open_internal(HISTORY),
            "downloads" => self.open_internal(DOWNLOADS),
            "bookmarks" => self.open_internal(BOOKMARKS),
            "settings" => self.open_internal(SETTINGS),
            "fullscreen" => {
                self.fs.full = !self.fs.full;
                self.fs.on = self.fs.full;
                self.sync_fs();
            }
            "focus" => {
                self.fs.on = !self.fs.on;
                self.fs.full &= self.fs.on;
                self.sync_fs();
                let mut s = self.cfg.write().unwrap();
                s.focus_mode = self.fs.on;
                s.save();
                drop(s);
                self.push_settings();
            }
            _ => {
                if let Some(i) = k.strip_prefix("tab").and_then(|d| d.parse::<usize>().ok()).filter(|&i| len > 0 && i > 0) {
                    self.select(self.tabs[if i == 9 { len - 1 } else { (i - 1).min(len - 1) }].id);
                }
            }
        }
    }

    fn eval_active(&self, code: &str) {
        if let Some(t) = self.tab(self.active) {
            let _ = t.wv.evaluate_script(code);
        }
    }

    fn event(&mut self, ev: Ev) {
        match ev {
            Ev::Msg(src, text) => self.msg(src, &text),
            Ev::Key(k) => self.key(&k),
            Ev::Title(id, title) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.title = title.clone();
                    if !t.private {
                        self.history.retitle(&t.url, &title);
                    }
                }
                self.push();
            }
            Ev::Url(id, url) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.url = url;
                }
                self.push();
            }
            Ev::Source(id, url) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id).filter(|t| t.url != url) {
                    t.url = url;
                    if !t.loading && !t.private {
                        self.history.visit(&t.url, &t.title);
                    }
                    self.push();
                }
            }
            Ev::Load(id, started, url) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.loading = started;
                    t.url = url.clone();
                    if started && !t.seek {
                        t.scroll = 0.0;
                    }
                    if !started && std::mem::take(&mut t.seek) {
                        let _ = t.wv.evaluate_script(&format!("window.scrollTo(0,{})", t.scroll));
                    }
                    if !started && !t.private {
                        self.history.visit(&url, &t.title);
                    }
                }
                self.push();
            }
            Ev::NewWin(id, url, private, forced) if url.starts_with("http") => {
                if forced || self.cfg.read().unwrap().links_in_new_tab || self.tab(id).is_none() {
                    self.open_tab(&url, private);
                } else {
                    self.navigate(id, &url);
                }
            }
            Ev::NewWin(..) => {}
            Ev::Sugs(q, list) => {
                self.push_sugs(&q, &list);
            }
            Ev::HistSave => {
                self.hist_wait = false;
                if self.history.dirty {
                    self.history.save_bg();
                }
                if self.bm.dirty {
                    self.bm.save();
                }
            }
            Ev::DlProgress(url, received, total) => {
                if let Some(d) = self.dls.iter_mut().rev().find(|d| d.url == url && d.state == "active" && d.rt.is_none()) {
                    d.received = received;
                    d.total = total;
                    if d.tick() {
                        self.push_downloads();
                        if self.popup_kind == "dl" {
                            self.show_dl_popup();
                        }
                    }
                }
            }
            Ev::DlStart(tab, url, path) => {
                if fetch::own(&url) {
                    let path = fetch::unique(path);
                    let mut d = Dl::new(url, path);
                    d.referer = self.tab(tab).map(|t| t.url.clone()).filter(|u| fetch::own(u)).unwrap_or_default();
                    let id = d.id;
                    self.dls.push(d);
                    self.start_fetch(id, None);
                } else {
                    self.dls.push(Dl::new(url, path));
                }
                self.push_downloads();
                if self.cfg.read().unwrap().dl_popup {
                    self.dl_auto = true;
                    self.dl_want = true;
                    self.request_dl_anchor();
                }
            }
            Ev::Fetch(id, msg) => self.fetch_msg(id, msg),
            Ev::DlDone(url, path, ok) => {
                if let Some(d) = self.dls.iter_mut().rev().find(|d| d.url == url && d.state == "active" && d.rt.is_none()) {
                    d.state = if ok { "done" } else { "fail" }.into();
                    if let Some(p) = path {
                        d.size = std::fs::metadata(&p).map_or(0, |m| m.len());
                        d.path = p;
                    }
                }
                self.dl_changed();
            }
            Ev::Icon(id, icon) => {
                let mut fresh = false;
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    if !t.private {
                        fresh = icons::save(&icons::host_of(&t.url), &icon);
                    }
                    t.icon = icon;
                }
                self.push();
                if fresh {
                    self.push_bookmarks(None);
                }
            }
            Ev::IconsReady => {
                self.push();
                self.push_bookmarks(None);
            }
            Ev::BmImport(list) => {
                self.bm.import(list);
                self.bm_changed();
            }
            Ev::ObBrowsers(list) => self.to_onboarding(&format!("rbOb.browsers({})", js(&list))),
            Ev::ObOut(import::Out::Ext { done, store, tx }) => {
                let (px, folder) = (self.px.clone(), done.folder.clone());
                extensions::add(&self.chrome, folder, move |r| {
                    let _ = px.send_event(Ev::ObExt(done, store, r, tx));
                });
            }
            Ev::ObExt(done, store, r, tx) => {
                let _ = tx.send(import::ext_added(&done, store, r));
            }
            Ev::ObOut(out) => {
                let msg = match out {
                    import::Out::Step(p, label) => format!("rbOb.step({})", js(&json!({"p": p, "label": label}))),
                    import::Out::Data(d) => {
                        let (nb, nh) = (self.bm.import(d.bookmarks), self.history.import(d.history));
                        if nb > 0 {
                            self.bm_changed();
                        }
                        format!("rbOb.add({})", js(&json!({"id": d.id, "bookmarks": nb, "history": nh, "error": d.error})))
                    }
                    import::Out::Cookies { id, list, skipped } => format!("rbOb.add({})", js(&json!({"id": id, "cookies": import::add_cookies(&self.chrome, &list), "skipped": skipped}))),
                    import::Out::ExtStats { id, stats } => {
                        self.ext_list(true);
                        format!("rbOb.add({})", js(&json!({"id": id, "ext": {"ok": stats.ok, "failed": stats.failed, "skipped": stats.skipped, "settings": stats.settings}})))
                    }
                    import::Out::Ext { .. } => return,
                };
                self.to_onboarding(&msg);
            }
            Ev::ObDone => self.to_onboarding("rbOb.done()"),
            Ev::PageFs(id, on) => {
                if id == self.active && self.fs.page != on {
                    self.fs.page = on;
                    self.sync_fs();
                }
            }
            Ev::PopupInit => self.ensure_popup(),
            Ev::Restore => self.restore_one(),
            Ev::SessTick => {
                self.saver_tick();
                self.sess_save(false);
                if self.resume() {
                    self.scroll_of(self.active);
                }
            }
            Ev::Scroll(id, y) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && !t.seek) {
                    t.scroll = y.max(0.0);
                }
            }
            Ev::Suspended(id, ok) => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && t.hidden.is_some()) {
                    t.sus = ok;
                    self.push();
                }
            }
            Ev::Spare => {
                if self.spare.is_none() {
                    self.spare = Some(self.make_tab(NEWTAB, false, false));
                    self.spare_at = Instant::now();
                }
            }
            Ev::Update(manual, r) => {
                if let Ok(Some(u)) = &r {
                    let fresh = !self.update.as_ref().is_some_and(|c| c.version == u.version);
                    self.update = Some(u.clone());
                    self.push();
                    if !manual && fresh && update::notify_once(&u.version) {
                        self.open_update();
                    }
                }
                if manual {
                    let v = match &r {
                        Ok(Some(u)) => json!({"show": true, "info": u.view()}),
                        Ok(None) => json!({}),
                        Err(e) => json!({"err": e}),
                    };
                    self.page_call("rbUpdate", &v);
                }
            }
            Ev::UpdProg(got, total) => self.page_call("rbUpdProg", &json!({"got": got, "total": total})),
            Ev::UpdDone(r) => {
                let cancelled = self.upd_cancel.take().is_some_and(|c| c.load(Ordering::Relaxed));
                match r.and_then(|p| if cancelled { Err("Cancelled".into()) } else { install::spawn_apply(&p) }) {
                    Ok(()) => self.quit = true,
                    Err(e) => self.page_call("rbUpdProg", &json!({"err": e})),
                }
            }
            Ev::ExtProgress(m) => self.ext_status(json!({"s": "busy", "m": m})),
            Ev::StoreInstall(src) if extinstall::folder_of(&src).is_some_and(|f| extensions::installed().contains(&f)) => self.ext_status(json!({"s": "ok", "m": "Already installed"})),
            Ev::StoreInstall(src) => self.ext_install(move || Ok(src)),
            Ev::PackageLink(id, url) => {
                let trusted = self.tab(id).is_some_and(|t| t.url.starts_with("https://addons.mozilla.org/"));
                self.ext_install(move || {
                    let host = url.split('/').nth(2).unwrap_or_default().to_owned();
                    let ok = trusted || rfd::MessageDialog::new().set_title("Install extension").set_description(format!("Install the extension from {host}?")).set_buttons(rfd::MessageButtons::YesNo).show() == rfd::MessageDialogResult::Yes;
                    if ok { Ok(extinstall::Source::Url(url)) } else { Err(String::new()) }
                });
            }
            Ev::ExtInstalled(Err(e)) => {
                self.ext_busy = false;
                self.ext_status(json!({"s": if e.is_empty() { "idle" } else { "err" }, "m": e}));
            }
            Ev::ExtInstalled(Ok(d)) => {
                self.ext_status(json!({"s": "busy", "m": "Installing"}));
                let (px, folder) = (self.px.clone(), d.folder.clone());
                extensions::add(&self.chrome, folder, move |r| {
                    let _ = px.send_event(Ev::ExtAdded(d, r));
                });
            }
            Ev::ExtAdded(d, r) => {
                self.ext_busy = false;
                match r {
                    Ok(id) => {
                        extensions::index_set(&id, &d.folder.file_name().unwrap_or_default().to_string_lossy(), d.source);
                        self.ext_status(json!({"s": "ok", "m": format!("Added {}", d.name), "notes": d.notes}));
                        self.ext_list(true);
                    }
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&d.folder);
                        self.ext_status(json!({"s": "err", "m": e}));
                    }
                }
            }
            Ev::ExtUpd(v) => self.ext_upd(v),
            Ev::ExtAuto(list) => {
                self.upd_q = list;
                self.ext_next();
            }
            Ev::ExtSwap(id, _, Err(e), silent) => {
                self.ext_busy = false;
                if !silent {
                    self.ext_upd(json!({"id": id, "s": "err", "m": e}));
                }
                self.ext_next();
            }
            Ev::ExtSwap(id, ver, Ok(d), silent) => {
                let (px, staged, src, d2) = (self.px.clone(), d.folder.clone(), d.source, d.clone());
                extensions::replace(&self.chrome, id.clone(), staged, src, move |r| {
                    let _ = px.send_event(Ev::ExtSwapped(id, ver, r, silent, d2));
                });
            }
            Ev::ExtSwapped(id, ver, r, silent, d) => {
                extupdate::unstage(&d);
                self.ext_busy = false;
                match r {
                    Ok(_) => {
                        self.ext_upd(json!({"id": id, "s": "done", "v": ver}));
                        if silent {
                            self.ext_status(json!({"s": "ok", "m": format!("Updated {} to v{ver}", d.name)}));
                        }
                    }
                    Err(e) if !silent => self.ext_upd(json!({"id": id, "s": "err", "m": e})),
                    Err(_) => {}
                }
                self.ext_list(true);
                self.ext_next();
            }
            Ev::ExtRemoved(id) => {
                if let Some(f) = extensions::index_take(&id).filter(|f| std::path::Path::new(f).file_name().is_some_and(|n| n == f.as_str())) {
                    let _ = std::fs::remove_dir_all(extensions::dir().join(f));
                }
                self.ext_status(json!({"s": "ok", "m": "Extension removed"}));
                self.ext_list(true);
            }
            Ev::Exts(list) => {
                let items = serde_json::to_value(&list).unwrap_or_default();
                for t in &self.tabs {
                    let _ = t.wv.evaluate_script(&format!("window.rbExts&&rbExts({})", js(&items)));
                }
                if !self.ext_page || self.popup_kind == "ext" {
                    let (x, y) = self.anchor;
                    self.show_popup("ext", x, y, 0.0, items);
                }
            }
        }
    }
}

fn main() {
    if !install::startup() {
        return;
    }
    extensions::drop_local();
    let mut settings = Settings::load();
    let hw = settings.hardware_accel;
    let home = if settings.onboarded { NEWTAB } else { ONBOARDING };
    if !settings.onboarded {
        settings.onboarded = true;
        settings.save();
    }
    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let px = event_loop.create_proxy();
    let win = WindowBuilder::new()
        .with_title("Ferrum")
        .with_window_icon(png::Decoder::new(std::io::Cursor::new(&include_bytes!("../ui/icons/app.png")[..])).read_info().ok().and_then(|mut d| {
            let mut buf = vec![0; d.output_buffer_size()?];
            let i = d.next_frame(&mut buf).ok()?;
            Icon::from_rgba(buf[..i.buffer_size()].to_vec(), i.width, i.height).ok()
        }))
        .with_decorations(false)
        .with_background_color(BG)
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(480.0, 320.0))
        .build(&event_loop)
        .expect("window");
    clip_children_on(&win);
    let mut ctx = WebContext::new(Some(settings::dir().join("webview")));
    let s = win.inner_size().to_logical::<f64>(win.scale_factor());
    let chrome = builder(&mut ctx, &px, Src::Chrome, hw)
        .with_bounds(rect(0.0, 0.0, s.width, chrome_h(false)))
        .with_url("http://rb.localhost/chrome")
        .build_as_child(&win)
        .expect("chrome webview");
    let key_px = px.clone();
    keys::watch(&chrome, move |k| {
        let _ = key_px.send_event(Ev::Key(k));
    });
    let mut b = Browser {
        win,
        ctx,
        px,
        cfg: Arc::new(RwLock::new(settings)),
        chrome,
        popup: None,
        popup_kind: "",
        popup_ready: false,
        popup_wait: None,
        popup_at: Instant::now(),
        spare_at: Instant::now(),
        tabs: vec![],
        spare: None,
        rest: VecDeque::new(),
        rest_head: 0,
        rest_tail: 0,
        sess: vec![],
        active: 0,
        next: 1,
        dls: downloads::load(),
        history: History::load(),
        hist_wait: false,
        closed: vec![],
        tab_menu: 0,
        anchor: (0.0, 0.0),
        dl_anchor: (0.0, 0.0),
        bm: Bookmarks::load(),
        bm_anchor: (0.0, 0.0),
        bm_sel: 0,
        bm_fsel: String::new(),
        bm_cols: vec![],
        bar_on: Cell::new(false),
        push_at: Cell::new(Instant::now()),
        push_due: Cell::new(false),
        fs: Focus { on: false, full: false, page: false, addr: false, to: false, p: 0.0, at: Instant::now(), leave: None, next: None },
        lay: Cell::new(((0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0), 0)),
        dl_auto: false,
        dl_want: false,
        ext_page: false,
        ext_busy: false,
        upd_q: vec![],
        update: None,
        upd_show: false,
        upd_cancel: None,
        quit: false,
    };
    let up_px = b.px.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        update::cleanup();
        let _ = up_px.send_event(Ev::Update(false, update::check()));
    });
    extensions::forget_local(&b.chrome);
    let (auto_px, auto_cfg) = (b.px.clone(), b.cfg.clone());
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(45));
        if !auto_cfg.read().unwrap().auto_update_ext || !extupdate::due() {
            return;
        }
        extupdate::mark();
        extupdate::sweep();
        let ups: Vec<_> = extupdate::targets().into_iter().filter_map(|t| extupdate::check(&t.src, &t.current).ok().flatten().map(|v| (t.id, v))).collect();
        if !ups.is_empty() {
            let _ = auto_px.send_event(Ev::ExtAuto(ups));
        }
    });
    let pre_px = b.px.clone();
    std::thread::spawn(move || {
        for (key, src) in extensions::preset_pending() {
            let say = |m: &str| {
                let _ = pre_px.send_event(Ev::ExtProgress(format!("{key}: {m}")));
            };
            match extinstall::install(src, &extensions::dir(), &say) {
                Ok(d) => {
                    extensions::preset_mark(key);
                    let _ = pre_px.send_event(Ev::ExtInstalled(Ok(d)));
                }
                Err(e) => {
                    let _ = pre_px.send_event(Ev::ExtInstalled(Err(format!("Could not install {key}: {e}"))));
                }
            }
        }
    });
    b.start(home, install::url_arg());
    let tick_px = b.px.clone();
    std::thread::spawn(move || {
        while tick_px.send_event(Ev::SessTick).is_ok() {
            std::thread::sleep(Duration::from_secs(4));
        }
    });
    let _ = b.px.send_event(Ev::PopupInit);
    let icon_px = b.px.clone();
    icons::prefetch(b.bm.items.iter().map(|i| icons::host_of(&i.url)).collect(), move || {
        let _ = icon_px.send_event(Ev::IconsReady);
    });
    if b.cfg.read().unwrap().focus_mode {
        b.fs.on = true;
        b.sync_fs();
    }
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(ev) => {
                b.event(ev);
                b.hist_sched();
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                b.layout();
                if b.popup_kind == "dl" {
                    b.request_dl_anchor();
                }
            }
            Event::MainEventsCleared => {
                b.fs_tick();
                b.push_flush();
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => b.quit = true,
            _ => {}
        }
        if b.quit {
            b.sess_save(true);
            b.history.save();
            downloads::save(&mut b.dls);
            if b.bm.dirty {
                b.bm.save();
            }
            *flow = ControlFlow::Exit;
        } else if let Some(t) = [b.fs.next, b.push_due.get().then(|| b.push_at.get() + PUSH_GAP)].into_iter().flatten().min() {
            *flow = ControlFlow::WaitUntil(t);
        }
    });
}
