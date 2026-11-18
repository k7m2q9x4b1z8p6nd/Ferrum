use crate::{bookmarks_io::{self, Entry}, extensions, extinstall, fileicon, history::Visit};
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Read,
    mem::size_of,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{atomic::{AtomicU32, Ordering}, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_2, COREWEBVIEW2_COOKIE_SAME_SITE_KIND};
use windows::{
    core::{Interface, BOOL, HSTRING, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HLOCAL, HWND, LPARAM, WPARAM},
        Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB},
        System::{
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE},
        },
        UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
    },
};
use wry::{WebView, WebViewExtWindows};

const WEBKIT_EPOCH: i64 = 11_644_473_600;
const COOKIE_CHUNK: usize = 500;
const SHARE_ALL: u32 = 7;
const STORAGE_MAX: u64 = 64 << 20;
const EXT_WAIT: Duration = Duration::from_secs(60);

struct Def {
    id: &'static str,
    name: &'static str,
    root: &'static str,
    exe: &'static [&'static str],
    proc: (&'static str, &'static str),
    gecko: bool,
}

const DEFS: [Def; 20] = [
    Def { id: "chrome", name: "Google Chrome", root: r"%LOCALAPPDATA%\Google\Chrome\User Data", exe: &[r"%ProgramFiles%\Google\Chrome\Application\chrome.exe", r"%ProgramFiles(x86)%\Google\Chrome\Application\chrome.exe", r"%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe"], proc: ("chrome.exe", r"\google\chrome\"), gecko: false },
    Def { id: "chrome-beta", name: "Chrome Beta", root: r"%LOCALAPPDATA%\Google\Chrome Beta\User Data", exe: &[r"%ProgramFiles%\Google\Chrome Beta\Application\chrome.exe", r"%ProgramFiles(x86)%\Google\Chrome Beta\Application\chrome.exe"], proc: ("chrome.exe", r"\google\chrome beta\"), gecko: false },
    Def { id: "chrome-dev", name: "Chrome Dev", root: r"%LOCALAPPDATA%\Google\Chrome Dev\User Data", exe: &[r"%ProgramFiles%\Google\Chrome Dev\Application\chrome.exe", r"%ProgramFiles(x86)%\Google\Chrome Dev\Application\chrome.exe"], proc: ("chrome.exe", r"\google\chrome dev\"), gecko: false },
    Def { id: "chrome-canary", name: "Chrome Canary", root: r"%LOCALAPPDATA%\Google\Chrome SxS\User Data", exe: &[r"%LOCALAPPDATA%\Google\Chrome SxS\Application\chrome.exe"], proc: ("chrome.exe", r"\google\chrome sxs\"), gecko: false },
    Def { id: "edge", name: "Microsoft Edge", root: r"%LOCALAPPDATA%\Microsoft\Edge\User Data", exe: &[r"%ProgramFiles(x86)%\Microsoft\EdgeCore\1*\msedge.exe", r"%ProgramFiles(x86)%\Microsoft\Edge\Application\msedge.exe", r"%ProgramFiles%\Microsoft\Edge\Application\msedge.exe"], proc: ("msedge.exe", r"\microsoft\edge"), gecko: false },
    Def { id: "brave", name: "Brave", root: r"%LOCALAPPDATA%\BraveSoftware\Brave-Browser\User Data", exe: &[r"%ProgramFiles%\BraveSoftware\Brave-Browser\Application\brave.exe", r"%ProgramFiles(x86)%\BraveSoftware\Brave-Browser\Application\brave.exe", r"%LOCALAPPDATA%\BraveSoftware\Brave-Browser\Application\brave.exe"], proc: ("brave.exe", r"\brave-browser\"), gecko: false },
    Def { id: "vivaldi", name: "Vivaldi", root: r"%LOCALAPPDATA%\Vivaldi\User Data", exe: &[r"%LOCALAPPDATA%\Vivaldi\Application\vivaldi.exe", r"%ProgramFiles%\Vivaldi\Application\vivaldi.exe"], proc: ("vivaldi.exe", r"\vivaldi\"), gecko: false },
    Def { id: "opera", name: "Opera", root: r"%APPDATA%\Opera Software\Opera Stable", exe: &[r"%LOCALAPPDATA%\Programs\Opera\opera.exe", r"%ProgramFiles%\Opera\opera.exe"], proc: ("opera.exe", r"\opera\"), gecko: false },
    Def { id: "opera-gx", name: "Opera GX", root: r"%APPDATA%\Opera Software\Opera GX Stable", exe: &[r"%LOCALAPPDATA%\Programs\Opera GX\opera.exe", r"%ProgramFiles%\Opera GX\opera.exe"], proc: ("opera.exe", r"opera gx"), gecko: false },
    Def { id: "chromium", name: "Chromium", root: r"%LOCALAPPDATA%\Chromium\User Data", exe: &[r"%LOCALAPPDATA%\Chromium\Application\chrome.exe"], proc: ("chrome.exe", r"\chromium\"), gecko: false },
    Def { id: "arc", name: "Arc", root: r"%LOCALAPPDATA%\Packages\TheBrowserCompany.Arc_*\LocalCache\Local\Arc\User Data", exe: &[], proc: ("arc.exe", r"thebrowsercompany"), gecko: false },
    Def { id: "helium", name: "Helium", root: r"%LOCALAPPDATA%\imput\Helium\User Data", exe: &[r"%LOCALAPPDATA%\imput\Helium\Application\chrome.exe"], proc: ("chrome.exe", r"\helium\"), gecko: false },
    Def { id: "thorium", name: "Thorium", root: r"%LOCALAPPDATA%\Thorium\User Data", exe: &[r"%LOCALAPPDATA%\Thorium\Application\thorium.exe"], proc: ("thorium.exe", r"\thorium\"), gecko: false },
    Def { id: "yandex", name: "Yandex Browser", root: r"%LOCALAPPDATA%\Yandex\YandexBrowser\User Data", exe: &[r"%LOCALAPPDATA%\Yandex\YandexBrowser\Application\browser.exe"], proc: ("browser.exe", r"\yandex\"), gecko: false },
    Def { id: "librewolf", name: "LibreWolf", root: r"%APPDATA%\librewolf", exe: &[r"%ProgramFiles%\LibreWolf\librewolf.exe"], proc: ("librewolf.exe", r"\librewolf\"), gecko: true },
    Def { id: "mullvad", name: "Mullvad Browser", root: r"%APPDATA%\Mullvad\MullvadBrowser", exe: &[r"%LOCALAPPDATA%\Programs\MullvadBrowser\mullvadbrowser.exe", r"%ProgramFiles%\Mullvad Browser\mullvadbrowser.exe"], proc: ("mullvadbrowser.exe", r"mullvad"), gecko: true },
    Def { id: "zen", name: "Zen", root: r"%APPDATA%\zen", exe: &[r"%ProgramFiles%\Zen Browser\zen.exe"], proc: ("zen.exe", r"zen"), gecko: true },
    Def { id: "waterfox", name: "Waterfox", root: r"%APPDATA%\Waterfox", exe: &[r"%ProgramFiles%\Waterfox\waterfox.exe"], proc: ("waterfox.exe", r"waterfox"), gecko: true },
    Def { id: "floorp", name: "Floorp", root: r"%APPDATA%\Floorp", exe: &[r"%ProgramFiles%\Ablaze Floorp\floorp.exe"], proc: ("floorp.exe", r"floorp"), gecko: true },
    Def { id: "firefox", name: "Mozilla Firefox", root: r"%APPDATA%\Mozilla\Firefox", exe: &[r"%ProgramFiles%\Mozilla Firefox\firefox.exe", r"%ProgramFiles(x86)%\Mozilla Firefox\firefox.exe"], proc: ("firefox.exe", r"\mozilla firefox\"), gecko: true },
];

pub struct Profile {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
}

struct Found {
    def: &'static Def,
    root: PathBuf,
    profiles: Vec<Profile>,
}

pub struct What {
    pub bookmarks: bool,
    pub history: bool,
    pub cookies: bool,
    pub extensions: bool,
}

pub struct Cand {
    pub id: String,
    pub name: String,
    pub chrome: bool,
}

#[derive(Default)]
pub struct ExtStats {
    pub ok: usize,
    pub failed: Vec<String>,
    pub skipped: usize,
    pub settings: usize,
}

pub struct Cookie {
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: i32,
}

#[derive(Default)]
pub struct Imported {
    pub id: String,
    pub bookmarks: Vec<Entry>,
    pub history: Vec<Visit>,
    pub error: Option<String>,
}

pub enum Out {
    Step(f64, String),
    Data(Imported),
    Cookies { id: String, list: Vec<Cookie>, skipped: usize },
    Ext { done: extinstall::Done, store: Option<PathBuf>, tx: mpsc::Sender<Result<(), String>> },
    ExtStats { id: String, stats: ExtStats },
}

fn err(e: impl ToString) -> String {
    e.to_string()
}

fn resolve(p: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in p.split('\\') {
        if let Some(v) = part.strip_prefix('%').and_then(|v| v.strip_suffix('%')) {
            out.push(std::env::var_os(v)?);
        } else if let Some((a, b)) = part.split_once('*') {
            out = fs::read_dir(&out)
                .ok()?
                .flatten()
                .find(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with(a) && n.ends_with(b)
                })?
                .path();
        } else {
            out.push(part);
        }
    }
    Some(out)
}

fn local_state_profiles(text: &str) -> Vec<(String, String)> {
    let v: Value = serde_json::from_str(text).unwrap_or_default();
    let mut out: Vec<(String, String)> = v
        .pointer("/profile/info_cache")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, p)| (k.clone(), p.get("name").and_then(Value::as_str).unwrap_or(k).to_owned())).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn parse_profiles_ini(text: &str) -> Vec<(String, String, bool)> {
    let mut out = vec![];
    let mut cur: Option<(String, String, bool)> = None;
    for l in text.lines().map(str::trim) {
        if let Some(sec) = l.strip_prefix('[') {
            out.extend(cur.take());
            if sec.starts_with("Profile") {
                cur = Some((String::new(), String::new(), true));
            }
        } else if let (Some(c), Some((k, v))) = (cur.as_mut(), l.split_once('=')) {
            match k {
                "Name" => c.0 = v.to_owned(),
                "Path" => c.1 = v.to_owned(),
                "IsRelative" => c.2 = v != "0",
                _ => {}
            }
        }
    }
    out.extend(cur);
    out.retain(|p| !p.1.is_empty());
    out
}

fn chromium_profiles(root: &Path) -> Vec<Profile> {
    let text = fs::read_to_string(root.join("Local State")).unwrap_or_default();
    let mut v: Vec<Profile> = local_state_profiles(&text).into_iter().map(|(id, name)| Profile { dir: root.join(&id), id, name }).filter(|p| p.dir.is_dir()).collect();
    if v.is_empty() && (root.join("Bookmarks").exists() || root.join("History").exists()) {
        v.push(Profile { id: String::new(), name: "Default".into(), dir: root.to_owned() });
    }
    v
}

fn gecko_profiles(root: &Path) -> Vec<Profile> {
    let text = fs::read_to_string(root.join("profiles.ini")).unwrap_or_default();
    parse_profiles_ini(&text)
        .into_iter()
        .map(|(name, path, rel)| Profile { dir: if rel { root.join(&path) } else { PathBuf::from(&path) }, name: if name.is_empty() { path.clone() } else { name }, id: path })
        .filter(|p| p.dir.join("places.sqlite").exists() || p.dir.join("cookies.sqlite").exists())
        .collect()
}

fn detect() -> Vec<Found> {
    DEFS.iter()
        .filter_map(|def| {
            let root = resolve(def.root).filter(|r| r.is_dir())?;
            let profiles = if def.gecko { gecko_profiles(&root) } else { chromium_profiles(&root) };
            (!profiles.is_empty()).then_some(Found { def, root, profiles })
        })
        .collect()
}

fn icon(def: &Def) -> String {
    def.exe
        .iter()
        .filter_map(|e| resolve(e).filter(|p| p.exists()))
        .find_map(|p| {
            let png = fileicon::exe(&p.to_string_lossy());
            (!png.is_empty()).then(|| format!("data:image/png;base64,{}", STANDARD.encode(png)))
        })
        .unwrap_or_default()
}

fn image(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

fn pids(def: &Def) -> Vec<u32> {
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case(def.proc.0) && image(e.th32ProcessID).is_some_and(|p| p.to_lowercase().contains(def.proc.1)) {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

unsafe extern "system" fn post_close(h: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let ids = &*(l.0 as *const Vec<u32>);
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if ids.contains(&pid) {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    true.into()
}

fn close(def: &Def) {
    let ids = pids(def);
    if ids.is_empty() {
        return;
    }
    unsafe {
        let _ = EnumWindows(Some(post_close), LPARAM(&ids as *const Vec<u32> as isize));
    }
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        if pids(def).is_empty() {
            break;
        }
    }
    for pid in pids(def) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
            }
        }
    }
    std::thread::sleep(Duration::from_millis(400));
}

pub fn list() -> Value {
    detect()
        .iter()
        .map(|f| {
            let profiles: Vec<Value> = f.profiles.iter().map(|p| json!({"id": p.id, "name": p.name})).collect();
            json!({"id": f.def.id, "name": f.def.name, "gecko": f.def.gecko, "icon": icon(f.def), "running": !pids(f.def).is_empty(), "profiles": profiles})
        })
        .collect()
}

fn open_shared(p: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).share_mode(SHARE_ALL).open(p)
}

fn read_shared(p: &Path) -> std::io::Result<Vec<u8>> {
    let mut b = Vec::new();
    open_shared(p)?.read_to_end(&mut b)?;
    Ok(b)
}

fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

struct Snap(PathBuf);

impl Snap {
    fn new(src: &Path) -> Result<Self, String> {
        static N: AtomicU32 = AtomicU32::new(0);
        let snap = Snap(std::env::temp_dir().join(format!("rb-import-{}-{}.db", std::process::id(), N.fetch_add(1, Ordering::Relaxed))));
        let copy = |from: &Path, to: &Path| open_shared(from).and_then(|mut f| std::io::copy(&mut f, &mut File::create(to)?));
        copy(src, &snap.0).map_err(|_| "couldn't read the database, close the browser and try again".to_owned())?;
        let wal = sibling(src, "-wal");
        if wal.exists() {
            let _ = copy(&wal, &sibling(&snap.0, "-wal"));
        }
        Ok(snap)
    }

    fn open(&self) -> Result<Connection, String> {
        Connection::open(&self.0).map_err(err)
    }
}

impl Drop for Snap {
    fn drop(&mut self) {
        for s in ["", "-wal", "-shm", "-journal"] {
            let _ = fs::remove_file(sibling(&self.0, s));
        }
    }
}

fn webkit_to_unix(us: i64) -> u64 {
    (us / 1_000_000 - WEBKIT_EPOCH).max(0) as u64
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

type Row = (i64, i64, i64, Option<String>, Option<String>);

fn gecko_entries(rows: &[Row]) -> Vec<Entry> {
    let folders: HashMap<i64, (i64, &str)> = rows.iter().filter(|r| r.1 == 2).map(|r| (r.0, (r.2, r.3.as_deref().unwrap_or("")))).collect();
    rows.iter()
        .filter(|r| r.1 == 1)
        .filter_map(|r| {
            let url = r.4.as_deref().filter(|u| u.starts_with("http"))?;
            let mut parts = vec![];
            let mut cur = r.2;
            while cur != 1 {
                let &(parent, title) = folders.get(&cur)?;
                parts.push((cur, title));
                cur = parent;
            }
            parts.reverse();
            if parts.first().is_some_and(|p| p.0 == 4) {
                return None;
            }
            let path: Vec<String> = parts
                .iter()
                .filter_map(|&(id, t)| match id {
                    3 => None,
                    2 => Some("Bookmarks Menu".into()),
                    5 => Some("Other Bookmarks".into()),
                    6 => Some("Mobile Bookmarks".into()),
                    _ => Some(t.replace('/', "-").trim().to_owned()),
                })
                .collect();
            let title = r.3.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(url);
            Some(Entry { title: title.to_owned(), url: url.to_owned(), folder: (!path.is_empty()).then(|| path.join("/")) })
        })
        .collect()
}

fn bookmarks(f: &Found, p: &Profile) -> Result<Vec<Entry>, String> {
    if !f.def.gecko {
        return match read_shared(&p.dir.join("Bookmarks")) {
            Ok(d) => bookmarks_io::parse("json", &d),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(err(e)),
        };
    }
    let snap = Snap::new(&p.dir.join("places.sqlite"))?;
    let c = snap.open()?;
    let mut st = c.prepare("SELECT b.id, b.type, b.parent, b.title, p.url FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk").map_err(err)?;
    let rows: Vec<Row> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(err)?.flatten().collect();
    Ok(gecko_entries(&rows))
}

fn history(f: &Found, p: &Profile) -> Result<Vec<Visit>, String> {
    let (file, sql) = if f.def.gecko {
        ("places.sqlite", "SELECT url, title, visit_count, last_visit_date / 1000000 FROM moz_places WHERE last_visit_date IS NOT NULL AND url LIKE 'http%'")
    } else {
        ("History", "SELECT url, title, visit_count, last_visit_time FROM urls WHERE hidden = 0 AND url LIKE 'http%'")
    };
    let snap = Snap::new(&p.dir.join(file))?;
    let c = snap.open()?;
    let mut st = c.prepare(sql).map_err(err)?;
    let gecko = f.def.gecko;
    let rows = st
        .query_map([], |r| {
            let t: i64 = r.get(3)?;
            Ok(Visit { url: r.get(0)?, title: r.get::<_, Option<String>>(1)?.unwrap_or_default(), visits: r.get::<_, i64>(2)?.clamp(1, i64::from(u32::MAX)) as u32, last: if gecko { t.max(0) as u64 } else { webkit_to_unix(t) } })
        })
        .map_err(err)?;
    Ok(rows.flatten().collect())
}

fn dpapi(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let inp = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&inp, None, None, None, None, 0, &mut out).ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Some(v)
    }
}

fn chromium_key(root: &Path) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_slice(&read_shared(&root.join("Local State")).ok()?).ok()?;
    let key = STANDARD.decode(v.pointer("/os_crypt/encrypted_key")?.as_str()?).ok()?;
    dpapi(key.strip_prefix(b"DPAPI")?)
}

fn decrypt(key: &[u8], blob: &[u8], strip_hash: bool) -> Option<String> {
    let rest = blob.strip_prefix(b"v10").or_else(|| blob.strip_prefix(b"v11"))?;
    if rest.len() < 12 + 16 {
        return None;
    }
    let (nonce, data) = rest.split_at(12);
    let mut out = Aes256Gcm::new_from_slice(key).ok()?.decrypt(Nonce::from_slice(nonce), data).ok()?;
    if strip_hash {
        out.drain(..32.min(out.len()));
    }
    String::from_utf8(out).ok()
}

fn same_site(v: i64, secure: bool) -> i32 {
    match v {
        0 if secure => 0,
        2 => 2,
        _ => 1,
    }
}

fn cookies(f: &Found, p: &Profile) -> Result<(Vec<Cookie>, usize), String> {
    let t = now();
    let mut out = Vec::new();
    let mut skipped = 0;
    if f.def.gecko {
        let snap = Snap::new(&p.dir.join("cookies.sqlite"))?;
        let c = snap.open()?;
        let mut st = c.prepare("SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies").map_err(err)?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)? != 0, r.get::<_, i64>(6)? != 0, r.get::<_, i64>(7)?)))
            .map_err(err)?;
        for (host, name, value, path, exp, secure, http_only, ss) in rows.flatten() {
            let exp = if exp > 100_000_000_000 { exp as f64 / 1000.0 } else { exp as f64 };
            if exp > 0.0 && exp < t {
                continue;
            }
            out.push(Cookie { host, name, value, path, expires: (exp > 0.0).then_some(exp), secure, http_only, same_site: same_site(ss, secure) });
        }
        return Ok((out, skipped));
    }
    let db = [p.dir.join("Network").join("Cookies"), p.dir.join("Cookies")].into_iter().find(|d| d.exists()).ok_or("no cookie database")?;
    let snap = Snap::new(&db)?;
    let c = snap.open()?;
    let strip = c.query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'version'", [], |r| r.get::<_, i64>(0)).unwrap_or(0) >= 24;
    let key = chromium_key(&f.root);
    let mut st = c.prepare("SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies").map_err(err)?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)? != 0, r.get::<_, i64>(7)? != 0, r.get::<_, i64>(8)?)))
        .map_err(err)?;
    for (host, name, plain, enc, path, exp, secure, http_only, ss) in rows.flatten() {
        let exp = (exp > 0).then(|| exp as f64 / 1e6 - WEBKIT_EPOCH as f64);
        if exp.is_some_and(|e| e < t) {
            continue;
        }
        let value = if enc.is_empty() { Some(plain) } else { key.as_deref().and_then(|k| decrypt(k, &enc, strip)) };
        match value {
            Some(value) => out.push(Cookie { host, name, value, path, expires: exp, secure, http_only, same_site: same_site(ss, secure) }),
            None => skipped += 1,
        }
    }
    Ok((out, skipped))
}

fn chromium_wanted(st: &Value) -> bool {
    let n = |k: &str| st[k].as_i64().unwrap_or(0);
    let no_reasons = match &st["disable_reasons"] {
        Value::Array(a) => a.is_empty(),
        v => v.as_i64().unwrap_or(0) == 0,
    };
    (n("location") == 1 || st["from_webstore"] == true) && st["was_installed_by_default"] != true && st["was_installed_by_oem"] != true && st["state"].as_i64().unwrap_or(1) == 1 && no_reasons
}

fn chromium_root(dir: &Path, id: &str) -> Option<PathBuf> {
    let mut vers: Vec<PathBuf> = fs::read_dir(dir.join("Extensions").join(id)).ok()?.flatten().map(|v| v.path()).filter(|p| p.join("manifest.json").is_file()).collect();
    vers.sort();
    vers.pop()
}

fn local_copy(dir: &Path, c: &Cand) -> Option<extinstall::Source> {
    let (path, source) = if c.chrome { (chromium_root(dir, &c.id)?, "chrome") } else { (dir.join("extensions").join(format!("{}.xpi", c.id)), "firefox") };
    path.exists().then(|| extinstall::Source::Copy(path, c.id.clone(), source))
}

fn chromium_extensions(dir: &Path) -> Vec<Cand> {
    let mut settings = serde_json::Map::new();
    for f in ["Preferences", "Secure Preferences"] {
        let v = read_shared(&dir.join(f)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if let Some(m) = v.as_ref().and_then(|v| v.pointer("/extensions/settings")).and_then(Value::as_object) {
            settings.extend(m.iter().map(|(k, x)| (k.clone(), x.clone())));
        }
    }
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(dir.join("Extensions")) else { return out };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        if !extinstall::is_chrome_id(&id) || (!settings.is_empty() && !settings.get(&id).is_some_and(chromium_wanted)) {
            continue;
        }
        let Some(root) = chromium_root(dir, &id) else { continue };
        let Some(m) = read_shared(&root.join("manifest.json")).ok().and_then(|b| serde_json::from_slice::<Value>(b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&b)).ok()) else { continue };
        let name = m["name"].as_str().map(|n| extinstall::localize(&root, &m, n)).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
        if m.get("theme").is_some() || m.get("app").is_some() || name.starts_with("Microsoft") {
            continue;
        }
        out.push(Cand { id, name, chrome: true });
    }
    out
}

fn gecko_extensions(dir: &Path) -> Vec<Cand> {
    let v: Value = read_shared(&dir.join("extensions.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    v["addons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["type"] == "extension" && a["location"] == "app-profile" && a["userDisabled"] != true && a["appDisabled"] != true && a["temporarilyInstalled"] != true)
        .filter_map(|a| {
            let id = a["id"].as_str().filter(|i| !i.is_empty())?;
            let name = a.pointer("/defaultLocale/name").and_then(Value::as_str).unwrap_or(id);
            Some(Cand { id: id.to_owned(), name: name.to_owned(), chrome: false })
        })
        .collect()
}

fn copy_storage(profile: &Path, id: &str) -> Option<PathBuf> {
    let (src, dst) = (profile.join("Local Extension Settings").join(id), extensions::storage_dir(id));
    if !src.is_dir() || dst.exists() {
        return None;
    }
    let mut left = STORAGE_MAX;
    let mut n = 0;
    let res = (|| -> std::io::Result<()> {
        fs::create_dir_all(&dst)?;
        for e in fs::read_dir(&src)?.flatten() {
            if !e.file_type()?.is_file() || e.file_name() == "LOCK" {
                continue;
            }
            let c = std::io::copy(&mut open_shared(&e.path())?, &mut File::create(dst.join(e.file_name()))?)?;
            left = left.checked_sub(c).ok_or(std::io::ErrorKind::OutOfMemory)?;
            n += 1;
        }
        Ok(())
    })();
    if res.is_err() || n == 0 {
        let _ = fs::remove_dir_all(&dst);
        return None;
    }
    Some(dst)
}

pub fn ext_added(d: &extinstall::Done, store: Option<PathBuf>, r: Result<String, String>) -> Result<(), String> {
    match r {
        Ok(id) => {
            if let Some(s) = store.filter(|s| s.file_name().is_none_or(|n| *n != *id)) {
                let _ = fs::remove_dir_all(s);
            }
            extensions::index_set(&id, &d.folder.file_name().unwrap_or_default().to_string_lossy(), d.source);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&d.folder);
            if let Some(s) = &store {
                let _ = fs::remove_dir_all(s);
            }
            Err(e)
        }
    }
}

fn import_extensions(f: &Found, p: &Profile, send: &mut dyn FnMut(Out), progress: &dyn Fn(f64) -> f64) -> ExtStats {
    let cands = if f.def.gecko { gecko_extensions(&p.dir) } else { chromium_extensions(&p.dir) };
    let installed = extensions::installed();
    let mut st = ExtStats::default();
    for (i, c) in cands.iter().enumerate() {
        send(Out::Step(progress(i as f64 / cands.len() as f64), format!("{}: extension {}", f.def.name, c.name)));
        if installed.contains(&extinstall::slug(&c.id)) {
            st.skipped += 1;
            continue;
        }
        let src = if c.chrome { extinstall::Source::Chrome(c.id.clone()) } else { extinstall::Source::Amo(c.id.clone()) };
        let done = match extinstall::install(src, &extensions::dir(), &|_| {}) {
            Ok(d) => d,
            Err(e) => match local_copy(&p.dir, c).map(|s| extinstall::install(s, &extensions::dir(), &|_| {})) {
                Some(Ok(d)) => d,
                _ => {
                    st.failed.push(format!("{}: {e}", c.name));
                    continue;
                }
            },
        };
        let store = if c.chrome && done.id.as_deref() == Some(c.id.as_str()) { copy_storage(&p.dir, &c.id) } else { None };
        let (tx, rx) = mpsc::channel();
        send(Out::Ext { done, store: store.clone(), tx });
        match rx.recv_timeout(EXT_WAIT) {
            Ok(Ok(())) => {
                st.ok += 1;
                st.settings += store.is_some() as usize;
            }
            Ok(Err(e)) => st.failed.push(format!("{}: {e}", c.name)),
            Err(_) => st.failed.push(format!("{}: the browser didn't respond", c.name)),
        }
    }
    st
}

pub fn run(sel: &[(String, String)], what: &What, send: &mut dyn FnMut(Out)) {
    let found = detect();
    let total = (sel.len() * (what.bookmarks as usize + what.history as usize + what.cookies as usize + what.extensions as usize)).max(1) as f64;
    let mut done = 0.0;
    for (bid, pid) in sel {
        let Some((f, p)) = found.iter().find(|f| f.def.id == bid).and_then(|f| f.profiles.iter().find(|p| &p.id == pid).map(|p| (f, p))) else { continue };
        if !pids(f.def).is_empty() {
            send(Out::Step(done / total, format!("Closing {}", f.def.name)));
            close(f.def);
        }
        let mut tick = |send: &mut dyn FnMut(Out), what: &str| {
            send(Out::Step(done / total, format!("{}: {what}", f.def.name)));
            done += 1.0;
        };
        let mut imp = Imported { id: bid.clone(), ..Default::default() };
        let mut errs = vec![];
        if what.bookmarks {
            tick(send, "bookmarks");
            match bookmarks(f, p) {
                Ok(v) => imp.bookmarks = v,
                Err(e) => errs.push(format!("Bookmarks: {e}")),
            }
        }
        if what.history {
            tick(send, "history");
            match history(f, p) {
                Ok(v) => imp.history = v,
                Err(e) => errs.push(format!("History: {e}")),
            }
        }
        let mut ck = (Vec::new(), 0);
        if what.cookies {
            tick(send, "cookies");
            match cookies(f, p) {
                Ok(v) => ck = v,
                Err(e) => errs.push(format!("Cookies: {e}")),
            }
        }
        imp.error = (!errs.is_empty()).then(|| errs.join("; "));
        send(Out::Data(imp));
        if what.extensions {
            let base = done;
            let stats = import_extensions(f, p, send, &|frac| (base + frac) / total);
            done += 1.0;
            send(Out::ExtStats { id: bid.clone(), stats });
        }
        let (mut rest, mut skipped) = ck;
        loop {
            let tail = rest.split_off(rest.len().min(COOKIE_CHUNK));
            send(Out::Cookies { id: bid.clone(), list: rest, skipped: std::mem::take(&mut skipped) });
            if tail.is_empty() {
                break;
            }
            rest = tail;
        }
    }
}

pub fn add_cookies(wv: &WebView, list: &[Cookie]) -> usize {
    unsafe {
        let Ok(mgr) = wv.webview().cast::<ICoreWebView2_2>().and_then(|w| w.CookieManager()) else { return 0 };
        list.iter()
            .filter(|c| {
                let Ok(ck) = mgr.CreateCookie(&HSTRING::from(&c.name), &HSTRING::from(&c.value), &HSTRING::from(&c.host), &HSTRING::from(&c.path)) else { return false };
                if let Some(e) = c.expires {
                    let _ = ck.SetExpires(e);
                }
                let _ = ck.SetIsSecure(c.secure);
                let _ = ck.SetIsHttpOnly(c.http_only);
                let _ = ck.SetSameSite(COREWEBVIEW2_COOKIE_SAME_SITE_KIND(c.same_site));
                mgr.AddOrUpdateCookie(&ck).is_ok()
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(key: &[u8], nonce: &[u8; 12], plain: &[u8]) -> Vec<u8> {
        let mut b = b"v10".to_vec();
        b.extend_from_slice(nonce);
        b.extend(Aes256Gcm::new_from_slice(key).unwrap().encrypt(Nonce::from_slice(nonce), plain).unwrap());
        b
    }

    #[test]
    fn webkit_time() {
        assert_eq!(webkit_to_unix((1_700_000_000 + WEBKIT_EPOCH) * 1_000_000), 1_700_000_000);
        assert_eq!(webkit_to_unix(0), 0);
    }

    #[test]
    fn decrypt_v10() {
        let key = [7u8; 32];
        let b = blob(&key, b"abcdefghijkl", b"secret-value");
        assert_eq!(decrypt(&key, &b, false).as_deref(), Some("secret-value"));
        assert_eq!(decrypt(&[8u8; 32], &b, false), None);
        let mut hashed = vec![9u8; 32];
        hashed.extend_from_slice(b"cookie");
        assert_eq!(decrypt(&key, &blob(&key, b"abcdefghijkl", &hashed), true).as_deref(), Some("cookie"));
    }

    #[test]
    fn decrypt_rejects_v20_and_short() {
        assert_eq!(decrypt(&[7u8; 32], b"v20\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c0123456789abcdef", false), None);
        assert_eq!(decrypt(&[7u8; 32], b"v10short", false), None);
    }

    #[test]
    fn firefox_profiles_ini() {
        let ini = "[Install4F96D1932A9F858E]\nDefault=Profiles/x.default-release\nLocked=1\n\n[Profile1]\nName=default\nIsRelative=1\nPath=Profiles/y.default\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/x.default-release\nDefault=1\n\n[Profile2]\nName=Abs\nIsRelative=0\nPath=D:/ff/abs\n\n[General]\nStartWithLastProfile=1\n";
        assert_eq!(
            parse_profiles_ini(ini),
            vec![
                ("default".to_owned(), "Profiles/y.default".to_owned(), true),
                ("default-release".to_owned(), "Profiles/x.default-release".to_owned(), true),
                ("Abs".to_owned(), "D:/ff/abs".to_owned(), false),
            ]
        );
    }

    #[test]
    fn chromium_local_state() {
        let ls = r#"{"os_crypt":{"encrypted_key":"x"},"profile":{"info_cache":{"Profile 1":{"name":"Work"},"Default":{"name":"Person 1"},"System Profile":{}}}}"#;
        assert_eq!(
            local_state_profiles(ls),
            vec![("Default".to_owned(), "Person 1".to_owned()), ("Profile 1".to_owned(), "Work".to_owned()), ("System Profile".to_owned(), "System Profile".to_owned())]
        );
        assert!(local_state_profiles("not json").is_empty());
    }

    #[test]
    fn firefox_bookmark_tree() {
        let s = |v: &str| Some(v.to_owned());
        let rows: Vec<Row> = vec![
            (1, 2, 0, s("root"), None),
            (2, 2, 1, s("menu"), None),
            (3, 2, 1, s("toolbar"), None),
            (4, 2, 1, s("tags"), None),
            (5, 2, 1, s("unfiled"), None),
            (10, 2, 3, s("Dev/Tools"), None),
            (11, 1, 3, s("Bar"), s("https://bar.com")),
            (12, 1, 10, s("Nested"), s("https://nested.com")),
            (13, 1, 2, None, s("https://menu.com")),
            (14, 1, 4, s("Tagged"), s("https://tag.com")),
            (15, 1, 5, s("Smart"), s("place:sort=8")),
        ];
        let e: Vec<(String, Option<String>)> = gecko_entries(&rows).into_iter().map(|e| (e.title, e.folder)).collect();
        assert_eq!(
            e,
            vec![
                ("Bar".to_owned(), None),
                ("Nested".to_owned(), Some("Dev-Tools".to_owned())),
                ("https://menu.com".to_owned(), Some("Bookmarks Menu".to_owned())),
            ]
        );
    }
}
