use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_DOWNLOAD: u64 = 64 << 20;
const MAX_UNPACKED: u64 = 256 << 20;
const MAX_FILES: usize = 20_000;
const MAX_SCAN: u64 = 4 << 20;
pub const PRODVERSION: &str = "146.0.0.0";
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
const POLYFILL: &str = include_str!("../assets/browser-polyfill.min.js");
const SW_SHIM: &str = include_str!("../assets/rb-sw-shim.js");
const SHIM: &str = "\n;(function(){var b=globalThis.browser,c=globalThis.chrome;if(b&&c){if(!b.menus&&c.contextMenus)b.menus=c.contextMenus;if(!b.browserAction&&c.action)b.browserAction=c.action}})();\n";
const HARD_PERMS: &[&str] = &["webRequestBlocking", "contextualIdentities", "dns", "pkcs11", "captivePortal", "geckoProfiler"];
const DROP_PERMS: &[&str] = &["tabHide", "find", "browserSettings", "menus.overrideContext", "activityLog", "theme", "urlbar", "telemetry", "mozillaAddons", "normandyAddonStudy", "trialML", "networkStatus"];
const DROP_KEYS: &[&str] = &["sidebar_action", "chrome_settings_overrides", "protocol_handlers", "experiment_apis", "theme_experiment", "langpacks", "dictionaries", "search_provider", "l10n_resources", "hidden"];
const HARD_JS: &[(&str, &str)] = &[
    ("filterResponseData", "it rewrites responses with Firefox-only webRequest.filterResponseData"),
    ("contextualIdentities", "it uses Firefox container tabs (contextualIdentities)"),
    ("proxy.onRequest", "it uses Firefox-only proxy.onRequest"),
];

#[derive(Debug, PartialEq)]
pub enum Source {
    Chrome(String),
    Amo(String),
    Url(String),
    Github(String),
    Path(PathBuf),
    Copy(PathBuf, String, &'static str),
}

#[derive(Clone)]
pub struct Done {
    pub folder: PathBuf,
    pub name: String,
    pub notes: Vec<String>,
    pub source: &'static str,
    pub id: Option<String>,
}

#[derive(Debug)]
pub struct Prep {
    pub firefox: bool,
    pub bg: Option<(Vec<String>, bool)>,
    pub bg_page: Option<String>,
    pub notes: Vec<String>,
    pub mv2: bool,
}

pub fn is_chrome_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

fn is_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn parse(input: &str) -> Result<Source, String> {
    let t = input.trim().trim_matches('"');
    if t.is_empty() {
        return Err("Enter a link, an extension ID or choose a file".into());
    }
    if is_chrome_id(t) {
        return Ok(Source::Chrome(t.to_owned()));
    }
    if t.len() > 4 && t[..5].eq_ignore_ascii_case("http:") {
        return Err("Only https links are allowed".into());
    }
    if t.len() > 7 && t[..8].eq_ignore_ascii_case("https://") {
        let rest = &t[8..];
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.to_ascii_lowercase();
        let path = path.split(['?', '#']).next().unwrap_or("");
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if host == "chromewebstore.google.com" || host == "chrome.google.com" {
            return segs.iter().find(|s| is_chrome_id(s)).map(|s| Source::Chrome((*s).to_owned())).ok_or_else(|| "No extension ID in that Chrome Web Store link".into());
        }
        if host == "addons.mozilla.org" {
            return segs
                .iter()
                .position(|s| *s == "addon")
                .and_then(|i| segs.get(i + 1))
                .filter(|s| is_slug(s))
                .map(|s| Source::Amo((*s).to_owned()))
                .ok_or_else(|| "No add-on name in that addons.mozilla.org link".into());
        }
        let low = path.to_ascii_lowercase();
        return if [".crx", ".xpi", ".zip"].iter().any(|e| low.ends_with(e)) { Ok(Source::Url(t.to_owned())) } else { Err("Unrecognized link. Use a Chrome Web Store or addons.mozilla.org page, or a direct .crx/.xpi/.zip link".into()) };
    }
    let p = Path::new(t);
    if p.is_absolute() && p.exists() {
        return Ok(Source::Path(p.to_owned()));
    }
    if is_slug(t) {
        return Ok(Source::Amo(t.to_owned()));
    }
    Err("Not a link, extension ID, add-on name or existing path".into())
}

pub fn is_package_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else { return false };
    let path = rest.split_once('/').map_or("", |(_, p)| p).split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    path.ends_with(".crx") || path.ends_with(".xpi")
}

pub fn store_request(host: &str, body: &str) -> Option<Source> {
    let m: Value = serde_json::from_str(body).ok()?;
    let id = m["id"].as_str()?;
    match (m["c"].as_str()?, m["k"].as_str()?, host) {
        ("store_install", "chrome", "chromewebstore.google.com" | "chrome.google.com") if is_chrome_id(id) => Some(Source::Chrome(id.to_owned())),
        ("store_install", "amo", "addons.mozilla.org") if is_slug(id) => Some(Source::Amo(id.to_owned())),
        _ => None,
    }
}

pub fn folder_of(s: &Source) -> Option<String> {
    match s {
        Source::Chrome(l) | Source::Amo(l) => Some(slug(l)),
        _ => None,
    }
}

pub fn strip_crx(d: &[u8]) -> Result<&[u8], String> {
    if d.starts_with(b"PK") {
        return Ok(d);
    }
    let word = |i: usize| d.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if d.get(..4) != Some(b"Cr24") {
        return Err("Not a CRX or zip file".into());
    }
    let start = match word(4) {
        Some(3) => word(8).and_then(|h| h.checked_add(12)),
        Some(2) => word(8).zip(word(12)).and_then(|(a, b)| a.checked_add(b)?.checked_add(16)),
        _ => return Err("Unsupported CRX version".into()),
    };
    start.and_then(|s| d.get(s..)).filter(|z| z.starts_with(b"PK")).ok_or_else(|| "Corrupt CRX header".into())
}

pub fn extract(zip: &[u8], dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("Bad archive: {e}"))?;
    if z.len() > MAX_FILES {
        return Err("Archive has too many files".into());
    }
    let mut left = MAX_UNPACKED;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| format!("Bad archive: {e}"))?;
        let rel = f.enclosed_name().ok_or_else(|| format!("Unsafe path in archive: {}", f.name()))?;
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut w = fs::File::create(&out).map_err(|e| e.to_string())?;
        let n = io::copy(&mut (&mut f).take(left + 1), &mut w).map_err(|e| e.to_string())?;
        if n > left {
            return Err("Archive is too large when unpacked".into());
        }
        left -= n;
    }
    Ok(())
}

fn chrome_version(v: &str) -> String {
    let n: Vec<String> = v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).take(4).map(|s| s.parse::<u32>().map_or(65535, |n| n.min(65535)).to_string()).collect();
    if n.is_empty() { "0.0.1".into() } else { n.join(".") }
}

fn strip_svg(v: &mut Value) -> bool {
    match v {
        Value::String(s) => s.to_ascii_lowercase().ends_with(".svg"),
        Value::Object(m) => {
            m.retain(|_, x| !strip_svg(x));
            false
        }
        _ => false,
    }
}

fn split_perms(o: &mut Map<String, Value>, key: &str, host_key: &str, notes: &mut Vec<String>) -> Result<(), String> {
    let Some(Value::Array(list)) = o.remove(key) else { return Ok(()) };
    let (mut api, mut host): (Vec<Value>, Vec<Value>) = (vec![], vec![]);
    for p in list.iter().filter_map(Value::as_str) {
        if HARD_PERMS.contains(&p) {
            return Err(format!("it needs the Firefox-only '{p}' permission"));
        }
        if DROP_PERMS.contains(&p) {
            notes.push(format!("Dropped unsupported permission '{p}'"));
            continue;
        }
        let p = if p == "menus" { "contextMenus" } else { p };
        let dst = if p == "<all_urls>" || p.contains("://") { &mut host } else { &mut api };
        if !dst.iter().any(|x| x == p) {
            dst.push(json!(p));
        }
    }
    if !api.is_empty() {
        o.insert(key.into(), Value::Array(api));
    }
    if !host.is_empty() {
        let mut all = o.get(host_key).and_then(Value::as_array).cloned().unwrap_or_default();
        for h in host {
            if !all.contains(&h) {
                all.push(h);
            }
        }
        o.insert(host_key.into(), Value::Array(all));
    }
    Ok(())
}

fn csp_ok(p: &str) -> bool {
    p.split(';').map(str::trim).filter(|d| !d.is_empty()).all(|d| {
        let mut t = d.split_whitespace();
        let allowed: &[&str] = match t.next() {
            Some("script-src" | "script-src-elem") => &["'self'", "'none'", "'wasm-unsafe-eval'"],
            Some("object-src") => &["'self'", "'none'"],
            _ => return true,
        };
        t.all(|x| allowed.contains(&x) || x.starts_with("http://localhost") || x.starts_with("http://127.0.0.1"))
    })
}

fn fix_csp(o: &mut Map<String, Value>, notes: &mut Vec<String>) {
    let Some(v) = o.remove("content_security_policy") else { return };
    let mut out = Map::new();
    match v {
        Value::String(p) => {
            if csp_ok(&p) {
                out.insert("extension_pages".into(), json!(p));
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                if x.as_str().is_some_and(|p| k == "sandbox" || csp_ok(p)) {
                    out.insert(k, x);
                }
            }
        }
        _ => {}
    }
    if out.is_empty() {
        notes.push("Using the default content security policy".into());
    } else {
        o.insert("content_security_policy".into(), Value::Object(out));
    }
}

pub fn convert(m: &mut Value) -> Result<Option<Prep>, String> {
    let o = m.as_object_mut().ok_or("manifest.json is not an object")?;
    if o.contains_key("theme") {
        return Err("browser themes can't be applied in this browser".into());
    }
    let firefox = o.contains_key("browser_specific_settings") || o.contains_key("applications");
    let scripts_only = o.get("background").is_some_and(|b| (b["scripts"].is_array() || b["page"].is_string()) && b["service_worker"].is_null());
    if o.get("manifest_version") == Some(&json!(3)) && !firefox && !scripts_only {
        return Ok(None);
    }
    let keep = o.get("manifest_version") == Some(&json!(2)) && o.get("permissions").and_then(Value::as_array).is_some_and(|a| a.iter().any(|p| p == "webRequestBlocking"));
    let mut notes = vec![];
    if !keep {
        o.insert("manifest_version".into(), json!(3));
    }
    for k in ["browser_specific_settings", "applications", "user_scripts", "developer"] {
        o.remove(k);
    }
    for k in DROP_KEYS {
        if o.remove(*k).is_some() {
            notes.push(format!("Dropped unsupported '{k}'"));
        }
    }
    if let Some(v) = o.get("version").and_then(Value::as_str).map(str::to_owned) {
        let c = chrome_version(&v);
        if c != v {
            o.insert("version_name".into(), json!(v));
            o.insert("version".into(), json!(c));
        }
    }
    for k in ["browser_action", "page_action"].iter().filter(|_| !keep) {
        if let Some(v) = o.remove(*k) {
            o.entry("action").or_insert(v);
        }
    }
    for k in ["action", "options_ui", "browser_action"] {
        if let Some(a) = o.get_mut(k).and_then(Value::as_object_mut) {
            for x in ["browser_style", "chrome_style", "default_area", "theme_icons", "show_matches", "hide_matches", "pinned"] {
                a.remove(x);
            }
        }
    }
    if keep {
        return keep_v2(o, notes, firefox);
    }
    split_perms(o, "permissions", "host_permissions", &mut notes)?;
    split_perms(o, "optional_permissions", "optional_host_permissions", &mut notes)?;
    if let Some(Value::Array(a)) = o.get("web_accessible_resources") {
        if a.iter().all(Value::is_string) {
            if a.is_empty() {
                o.remove("web_accessible_resources");
            } else {
                let r = Value::Array(a.clone());
                o.insert("web_accessible_resources".into(), json!([{"resources": r, "matches": ["<all_urls>"]}]));
            }
        }
    }
    fix_csp(o, &mut notes);
    if o.get_mut("icons").is_some_and(strip_svg) {
        o.remove("icons");
    }
    if let Some(a) = o.get_mut("action").and_then(Value::as_object_mut) {
        if a.get_mut("default_icon").is_some_and(strip_svg) {
            a.remove("default_icon");
        }
    }
    if let Some(c) = o.get_mut("commands").and_then(Value::as_object_mut) {
        for (from, to) in [("_execute_browser_action", "_execute_action"), ("_execute_page_action", "_execute_action")] {
            if let Some(x) = c.remove(from) {
                c.entry(to).or_insert(x);
            }
        }
    }
    let mut bg = None;
    let mut bg_page = None;
    if let Some(b) = o.get_mut("background").and_then(Value::as_object_mut) {
        let module = b.get("type") == Some(&json!("module"));
        let strs = |v: Option<&Value>| v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>());
        let scripts = match (b.get("service_worker").and_then(Value::as_str), b.get("page").and_then(Value::as_str)) {
            (Some(s), _) => vec![s.to_owned()],
            (None, page) => {
                if firefox || scripts_only {
                    notes.push("Background code runs as a service worker with a small window/document/localStorage/XMLHttpRequest shim: DOM-heavy code (canvas, Audio, DOMParser) won't work".into());
                }
                bg_page = page.map(str::to_owned);
                if bg_page.is_some() { vec![] } else { strs(b.get("scripts")).unwrap_or_default() }
            }
        };
        b.clear();
        if !scripts.is_empty() || bg_page.is_some() {
            b.insert("service_worker".into(), json!("rb-background.js"));
            if module {
                b.insert("type".into(), json!("module"));
            }
            bg = Some((scripts, module));
        }
    }
    if bg.is_none() {
        o.remove("background");
    } else if firefox {
        let perms = o.entry("permissions").or_insert_with(|| json!([]));
        if let Some(a) = perms.as_array_mut().filter(|a| !a.contains(&json!("storage"))) {
            a.push(json!("storage"));
        }
    }
    if firefox {
        polyfill_content(o);
    }
    Ok(Some(Prep { firefox, bg, bg_page, notes, mv2: false }))
}

fn polyfill_content(o: &mut Map<String, Value>) {
    if let Some(Value::Array(cs)) = o.get_mut("content_scripts") {
        for c in cs.iter_mut().filter(|c| c["world"] != "MAIN") {
            if let Some(Value::Array(js)) = c.get_mut("js") {
                js.insert(0, json!("rb-polyfill.js"));
            }
        }
    }
}

fn keep_v2(o: &mut Map<String, Value>, mut notes: Vec<String>, firefox: bool) -> Result<Option<Prep>, String> {
    notes.push("Kept as a Manifest V2 extension so request blocking works".into());
    for key in ["permissions", "optional_permissions"] {
        let Some(Value::Array(list)) = o.get_mut(key) else { continue };
        let mut out = vec![];
        for p in list.iter().filter_map(Value::as_str) {
            if p != "webRequestBlocking" && p != "dns" && HARD_PERMS.contains(&p) {
                return Err(format!("it needs the Firefox-only '{p}' permission"));
            }
            if p == "dns" || DROP_PERMS.contains(&p) {
                notes.push(format!("Dropped unsupported permission '{p}'"));
                continue;
            }
            out.push(json!(if p == "menus" { "contextMenus" } else { p }));
        }
        *list = out;
    }
    if o.get_mut("icons").is_some_and(strip_svg) || o.get("icons").and_then(Value::as_object).is_some_and(Map::is_empty) {
        o.remove("icons");
    }
    if let Some(a) = o.get_mut("browser_action").and_then(Value::as_object_mut) {
        if a.get_mut("default_icon").is_some_and(strip_svg) || a.get("default_icon").and_then(Value::as_object).is_some_and(Map::is_empty) {
            a.remove("default_icon");
        }
    }
    if firefox {
        polyfill_content(o);
        if let Some(Value::Array(s)) = o.get_mut("background").and_then(|b| b.get_mut("scripts")) {
            s.insert(0, json!("rb-polyfill.js"));
        }
    }
    Ok(Some(Prep { firefox, bg: None, bg_page: None, notes, mv2: true }))
}

pub fn inject_html(h: &str) -> String {
    const TAG: &str = "<script src=\"/rb-polyfill.js\"></script>";
    let low = h.to_ascii_lowercase();
    match low.find("<head").and_then(|i| low[i..].find('>').map(|j| i + j + 1)) {
        Some(p) => format!("{}{TAG}{}", &h[..p], &h[p..]),
        None => format!("{TAG}{h}"),
    }
}

pub fn bg_source(scripts: &[String], firefox: bool, module: bool) -> String {
    let pre: &[&str] = if firefox { &["rb-polyfill.js", "rb-sw-shim.js"] } else { &[] };
    let files = pre.iter().map(|s| (*s).to_owned()).chain(scripts.iter().map(|s| s.trim_start_matches('/').to_owned()));
    let q = |s: String| serde_json::to_string(&s).unwrap_or_default();
    if module {
        files.map(|s| format!("import {};\n", q(format!("./{s}")))).collect()
    } else {
        format!("importScripts({});\n", files.map(q).collect::<Vec<_>>().join(","))
    }
}

fn tag_attr(tag: &str, name: &str) -> Option<String> {
    let low = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = low[from..].find(name).map(|i| i + from) {
        from = i + name.len();
        let rest = tag[from..].trim_start();
        if !low[..i].ends_with(|c: char| c.is_ascii_whitespace() || c == '"' || c == '\'') || !rest.starts_with('=') {
            continue;
        }
        let v = rest[1..].trim_start();
        return Some(match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_owned(),
            _ => v.split(|c: char| c.is_ascii_whitespace() || c == '>').next().unwrap_or("").to_owned(),
        });
    }
    None
}

pub fn page_scripts(h: &str) -> Vec<(bool, String, bool)> {
    let low = h.to_ascii_lowercase();
    let mut out = vec![];
    let mut at = 0;
    while let Some(i) = low[at..].find("<script").map(|i| i + at) {
        let Some(gt) = low[i..].find('>').map(|j| i + j) else { break };
        let tag = &h[i + 7..gt];
        let end = low[gt..].find("</script").map_or(h.len(), |j| gt + j);
        let module = tag_attr(tag, "type").is_some_and(|t| t.eq_ignore_ascii_case("module"));
        match tag_attr(tag, "src") {
            Some(src) => out.push((true, src, module)),
            None if !h[gt + 1..end].trim().is_empty() => out.push((false, h[gt + 1..end].to_owned(), module)),
            None => {}
        }
        at = end.max(gt + 1);
    }
    out
}

fn norm(base: &str, src: &str) -> Option<String> {
    let src = src.split(['?', '#']).next()?;
    if src.contains(['\\', ':']) || src.starts_with("//") {
        return None;
    }
    let mut seg: Vec<&str> = if src.starts_with('/') { vec![] } else { base.rsplit_once('/').map_or(vec![], |(d, _)| d.split('/').collect()) };
    for s in src.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                seg.pop()?;
            }
            s => seg.push(s),
        }
    }
    (!seg.is_empty()).then(|| seg.join("/"))
}

fn page_bg(root: &Path, page: &str, notes: &mut Vec<String>) -> Result<(Vec<String>, bool), String> {
    let rel = norm("", page).ok_or("its background page path is invalid")?;
    let html = fs::read_to_string(root.join(&rel)).map_err(|_| "its background page is missing".to_owned())?;
    let (mut files, mut module) = (vec![], false);
    for (n, (is_src, v, m)) in page_scripts(&html).into_iter().filter(|(s, v, _)| !(*s && v.ends_with("rb-polyfill.js"))).enumerate() {
        module |= m;
        if is_src {
            match norm(&rel, &v) {
                Some(f) if root.join(&f).is_file() => files.push(f),
                _ => notes.push(format!("Skipped background script '{v}' (remote or missing)")),
            }
        } else {
            let f = format!("rb-inline-{n}.js");
            fs::write(root.join(&f), v).map_err(|e| e.to_string())?;
            files.push(f);
        }
    }
    Ok((files, module))
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path) -> Result<(), String>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk(&p, f)?;
        } else if t.is_file() && e.metadata().is_ok_and(|m| m.len() <= MAX_SCAN) {
            f(&p)?;
        }
    }
    Ok(())
}

fn ext_of(p: &Path, exts: &[&str]) -> bool {
    p.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
}

fn scan_js(dir: &Path, mv2: bool) -> Result<(), String> {
    walk(dir, &mut |p| {
        if ext_of(p, &["js"]) {
            let s = String::from_utf8_lossy(&fs::read(p).map_err(|e| e.to_string())?).into_owned();
            if let Some((_, why)) = HARD_JS.iter().filter(|(k, _)| !(mv2 && *k == "filterResponseData")).find(|(k, _)| s.contains(k)) {
                return Err((*why).to_owned());
            }
        }
        Ok(())
    })
}

fn inject_pages(dir: &Path) -> Result<(), String> {
    walk(dir, &mut |p| {
        if ext_of(p, &["html", "htm"]) {
            if let Ok(h) = fs::read_to_string(p) {
                if !h.contains("rb-polyfill.js") {
                    fs::write(p, inject_html(&h)).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    })
}

fn fix_locales(root: &Path, m: &mut Value, notes: &mut Vec<String>) {
    let Some(o) = m.as_object_mut() else { return };
    let loc = root.join("_locales");
    let mut names: Vec<String> = fs::read_dir(&loc).map(|r| r.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    for n in names.iter_mut().filter(|n| n.contains('-')) {
        let u = n.replace('-', "_");
        if !loc.join(&u).exists() && fs::rename(loc.join(&*n), loc.join(&u)).is_ok() {
            *n = u;
        }
    }
    names.sort();
    let cur = o.get("default_locale").and_then(Value::as_str).map(|d| d.replace('-', "_"));
    match (cur, names.is_empty()) {
        (Some(_), true) => {
            o.remove("default_locale");
            notes.push("Dropped default_locale: the extension has no _locales folder".into());
        }
        (Some(d), false) if names.contains(&d) => {
            o.insert("default_locale".into(), json!(d));
        }
        (_, false) => {
            let pick = ["en", "en_US", "en_GB"].iter().find(|c| names.iter().any(|n| n == *c)).map_or_else(|| names[0].clone(), |c| (*c).to_owned());
            o.insert("default_locale".into(), json!(pick));
        }
        (None, true) => {}
    }
}

fn svg_icon(m: &Value) -> Option<String> {
    let svg = |v: &Value| v.as_str().filter(|s| s.to_ascii_lowercase().ends_with(".svg")).map(str::to_owned);
    let mut c: Vec<(u32, String)> = m["icons"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.parse().ok()?, svg(v)?))).collect();
    c.sort();
    c.pop().map(|x| x.1).or_else(|| svg(&m["browser_action"]["default_icon"])).or_else(|| svg(&m["action"]["default_icon"]))
}

fn prepare(root: &Path, key: Option<&str>) -> Result<Vec<String>, String> {
    let mp = root.join("manifest.json");
    let raw = fs::read_to_string(&mp).map_err(|_| "manifest.json is unreadable".to_owned())?;
    let mut m: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| format!("manifest.json is invalid: {e}"))?;
    if let (Some(k), Some(o)) = (key, m.as_object_mut()) {
        o.entry("key").or_insert(json!(k));
    }
    let write = |m: &Value| fs::write(&mp, serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?).map_err(|e| e.to_string());
    let svg = svg_icon(&m);
    let Some(mut p) = convert(&mut m)? else {
        if key.is_some() {
            write(&m)?;
        }
        return Ok(vec![]);
    };
    if p.firefox {
        scan_js(root, p.mv2)?;
        fs::write(root.join("rb-polyfill.js"), format!("{POLYFILL}{SHIM}")).map_err(|e| e.to_string())?;
        inject_pages(root)?;
        fix_locales(root, &mut m, &mut p.notes);
    }
    if let Some(page) = &p.bg_page {
        let (scripts, module) = page_bg(root, page, &mut p.notes)?;
        if scripts.is_empty() {
            if let Some(o) = m.as_object_mut() {
                o.remove("background");
            }
            p.bg = None;
            p.notes.push("Dropped the background page: it has no usable scripts".into());
        } else {
            if module {
                m["background"]["type"] = json!("module");
            }
            p.bg = Some((scripts, module));
        }
    }
    if let Some((scripts, module)) = &p.bg {
        if p.firefox {
            fs::write(root.join("rb-sw-shim.js"), SW_SHIM).map_err(|e| e.to_string())?;
        }
        fs::write(root.join("rb-background.js"), bg_source(scripts, p.firefox, *module)).map_err(|e| e.to_string())?;
    }
    if let Some(i) = svg.filter(|_| m.get("icons").is_none()) {
        fs::write(root.join("rb-icon.txt"), i).map_err(|e| e.to_string())?;
    }
    write(&m)?;
    Ok(p.notes)
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().https_only(true).timeout_global(Some(Duration::from_secs(120))).build().into()
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut r = agent().get(url).header("User-Agent", UA).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "Not found".to_owned(),
        e => format!("Download failed: {e}"),
    })?;
    let d = r.body_mut().with_config().limit(MAX_DOWNLOAD).read_to_vec().map_err(|e| format!("Download failed: {e}"))?;
    if d.is_empty() { Err("Nothing to download (extension removed or unavailable)".into()) } else { Ok(d) }
}

fn amo_xpi(slug: &str) -> Result<String, String> {
    let url = format!("https://addons.mozilla.org/api/v5/addons/addon/{}/", slug.replace('{', "%7B").replace('}', "%7D").replace('@', "%40"));
    let v: Value = agent().get(&url).header("User-Agent", UA).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "Add-on not found on addons.mozilla.org".to_owned(),
        e => format!("Lookup failed: {e}"),
    })?.body_mut().read_json().map_err(|e| format!("Lookup failed: {e}"))?;
    if v["type"] != "extension" {
        return Err("That add-on is not an extension (theme, dictionary or language pack)".into());
    }
    v["current_version"]["file"]["url"].as_str().map(str::to_owned).ok_or_else(|| "Add-on has no downloadable file".into())
}

fn github_zip(repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let v: Value = agent().get(&url).header("User-Agent", UA).call().map_err(|e| format!("Lookup failed: {e}"))?.body_mut().read_json().map_err(|e| format!("Lookup failed: {e}"))?;
    v["assets"].as_array().into_iter().flatten().filter_map(|a| a["browser_download_url"].as_str()).find(|u| u.ends_with(".chromium.zip")).map(str::to_owned).ok_or_else(|| "No Chromium build in the latest release".into())
}

fn copy_dir(from: &Path, to: &Path, left: &mut u64) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let t = e.file_type().map_err(|e| e.to_string())?;
        let dst = to.join(e.file_name());
        if t.is_dir() {
            copy_dir(&e.path(), &dst, left)?;
        } else if t.is_file() {
            let n = fs::copy(e.path(), &dst).map_err(|e| e.to_string())?;
            *left = left.checked_sub(n).ok_or("Folder is too large")?;
        }
    }
    Ok(())
}

fn find_root(dir: &Path) -> Result<PathBuf, String> {
    if dir.join("manifest.json").is_file() {
        return Ok(dir.to_owned());
    }
    let subs: Vec<PathBuf> = fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()).collect();
    match subs.as_slice() {
        [one] if one.join("manifest.json").is_file() => Ok(one.clone()),
        _ => Err("No manifest.json found: not a browser extension".into()),
    }
}

pub fn safe_rel(p: &str) -> Option<PathBuf> {
    let p = Path::new(p.split(['?', '#']).next()?.trim_start_matches('/'));
    (!p.as_os_str().is_empty() && p.components().all(|c| matches!(c, std::path::Component::Normal(_)))).then(|| p.to_owned())
}

pub fn localize(root: &Path, m: &Value, s: &str) -> String {
    let Some(key) = s.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) else { return s.to_owned() };
    let loc = m["default_locale"].as_str().unwrap_or("en");
    [loc.to_owned(), loc.replace('-', "_"), "en".to_owned(), "en_US".to_owned()]
        .iter()
        .find_map(|l| {
            let v: Value = serde_json::from_slice(&fs::read(root.join("_locales").join(l).join("messages.json")).ok()?).ok()?;
            v.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).and_then(|(_, e)| e["message"].as_str().map(str::to_owned))
        })
        .unwrap_or_default()
}

fn varint(d: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for s in 0..10 {
        let b = *d.get(*i)?;
        *i += 1;
        v |= u64::from(b & 0x7f) << (7 * s);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn pb_fields(d: &[u8]) -> Vec<(u64, &[u8])> {
    let (mut i, mut out) = (0, vec![]);
    while i < d.len() {
        let Some(t) = varint(d, &mut i) else { break };
        match t & 7 {
            0 => {
                if varint(d, &mut i).is_none() {
                    break;
                }
            }
            1 => i += 8,
            5 => i += 4,
            2 => {
                let Some(n) = varint(d, &mut i) else { break };
                let end = i.saturating_add(n as usize);
                let Some(f) = d.get(i..end) else { break };
                out.push((t >> 3, f));
                i = end;
            }
            _ => break,
        }
    }
    out
}

pub fn crx_key(d: &[u8]) -> Option<(String, String)> {
    let word = |i: usize| d.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if d.get(..4) != Some(b"Cr24") || word(4) != Some(3) {
        return None;
    }
    let head = d.get(12..word(8)?.checked_add(12)?)?;
    let f = pb_fields(head);
    let signed = f.iter().find(|(t, _)| *t == 10000)?.1;
    let id = pb_fields(signed).into_iter().find(|(t, _)| *t == 1)?.1;
    let key = f.iter().filter(|(t, _)| *t == 2 || *t == 3).filter_map(|(_, p)| pb_fields(p).into_iter().find(|(t, _)| *t == 1).map(|x| x.1)).find(|k| Sha256::digest(k)[..16] == *id)?;
    Some((STANDARD.encode(key), id.iter().flat_map(|b| [b >> 4, b & 15]).map(|n| char::from(b'a' + n)).collect()))
}

pub fn slug(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let t = t.trim_matches('-');
    if t.is_empty() { "extension".into() } else { t.chars().take(40).collect() }
}

fn stem(s: &str) -> String {
    let s = s.split(['?', '#']).next().unwrap_or(s).trim_end_matches(['/', '\\']);
    slug(Path::new(s).file_stem().and_then(|x| x.to_str()).unwrap_or(s.rsplit('/').next().unwrap_or(s)))
}

fn archive_bytes(d: &[u8], stage: &Path) -> Result<(), String> {
    extract(strip_crx(d)?, stage)
}

fn build(src: &Source, stage: &Path, say: &dyn Fn(&str)) -> Result<(String, Option<(String, String)>), String> {
    let mut key = None;
    let label = match src {
        Source::Chrome(id) => {
            say("Downloading from the Chrome Web Store");
            let url = format!("https://clients2.google.com/service/update2/crx?response=redirect&prodversion={PRODVERSION}&acceptformat=crx2,crx3&x=id%3D{id}%26uc");
            let d = fetch(&url).map_err(|e| if e == "Not found" { "Extension not found on the Chrome Web Store".into() } else { e })?;
            say("Unpacking");
            archive_bytes(&d, stage)?;
            key = crx_key(&d);
            id.clone()
        }
        Source::Amo(s) => {
            say("Looking up on addons.mozilla.org");
            let xpi = amo_xpi(s)?;
            say("Downloading");
            archive_bytes(&fetch(&xpi)?, stage)?;
            s.clone()
        }
        Source::Url(u) => {
            say("Downloading");
            archive_bytes(&fetch(u)?, stage)?;
            stem(u)
        }
        Source::Github(r) => {
            say("Looking up on GitHub");
            let u = github_zip(r)?;
            say("Downloading");
            archive_bytes(&fetch(&u)?, stage)?;
            stem(r)
        }
        Source::Copy(p, label, _) => {
            key = build(&Source::Path(p.clone()), stage, say)?.1;
            label.clone()
        }
        Source::Path(p) if p.is_dir() => {
            say("Copying");
            copy_dir(p, stage, &mut { MAX_UNPACKED })?;
            stem(&p.to_string_lossy())
        }
        Source::Path(p) => {
            say("Unpacking");
            if fs::metadata(p).map_err(|e| e.to_string())?.len() > MAX_DOWNLOAD {
                return Err("File is too large".into());
            }
            archive_bytes(&fs::read(p).map_err(|e| e.to_string())?, stage)?;
            stem(&p.to_string_lossy())
        }
    };
    Ok((label, key))
}

pub fn install(src: Source, dest: &Path, say: &dyn Fn(&str)) -> Result<Done, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let stage = dest.with_file_name(format!("ext_stage_{nanos}"));
    let res = (|| {
        fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let (label, key) = build(&src, &stage, say)?;
        let root = find_root(&stage)?;
        say("Checking compatibility");
        let notes = prepare(&root, key.as_ref().map(|k| k.0.as_str())).map_err(|e| format!("Not compatible: {e}"))?;
        let name = fs::read(root.join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|m| Some(localize(&root, &m, m["name"].as_str()?)).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| label.clone());
        let source = match &src {
            Source::Chrome(_) => "chrome",
            Source::Amo(_) => "firefox",
            Source::Github(_) => "chrome",
            Source::Copy(_, _, s) => s,
            _ => "local",
        };
        let folder = dest.join(slug(&label));
        if folder.exists() {
            fs::remove_dir_all(&folder).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        fs::rename(&root, &folder).map_err(|e| e.to_string())?;
        Ok(Done { folder, name, notes, source, id: key.map(|k| k.1) })
    })();
    let _ = fs::remove_dir_all(&stage);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ID: &str = "ddkjiahejlhfcafbddmgiahcphecmpfh";

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(vec![]));
        for (n, c) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(c.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn parse_chrome() {
        assert_eq!(parse(ID).unwrap(), Source::Chrome(ID.into()));
        let u = format!("https://chromewebstore.google.com/detail/ublock-origin-lite/{ID}?hl=en");
        assert_eq!(parse(&u).unwrap(), Source::Chrome(ID.into()));
        let u = format!("https://chrome.google.com/webstore/detail/x/{ID}");
        assert_eq!(parse(&u).unwrap(), Source::Chrome(ID.into()));
        assert!(parse("https://chromewebstore.google.com/category/extensions").is_err());
    }

    #[test]
    fn parse_amo_and_urls() {
        assert_eq!(parse("https://addons.mozilla.org/en-US/firefox/addon/ublock-origin/").unwrap(), Source::Amo("ublock-origin".into()));
        assert_eq!(parse("ublock-origin").unwrap(), Source::Amo("ublock-origin".into()));
        assert_eq!(parse("https://example.com/a/b.XPI?x=1").unwrap(), Source::Url("https://example.com/a/b.XPI?x=1".into()));
        assert!(parse("http://example.com/a.crx").is_err());
        assert!(parse("https://example.com/page").is_err());
        assert!(parse("  ").is_err());
        assert!(parse("not a thing!").is_err());
    }

    #[test]
    fn crx_header() {
        let z = zip_of(&[("manifest.json", "{}")]);
        let mut v3 = b"Cr24".to_vec();
        v3.extend(3u32.to_le_bytes());
        v3.extend(5u32.to_le_bytes());
        v3.extend([9; 5]);
        v3.extend(&z);
        assert_eq!(strip_crx(&v3).unwrap(), &z[..]);
        let mut v2 = b"Cr24".to_vec();
        v2.extend(2u32.to_le_bytes());
        v2.extend(2u32.to_le_bytes());
        v2.extend(3u32.to_le_bytes());
        v2.extend([9; 5]);
        v2.extend(&z);
        assert_eq!(strip_crx(&v2).unwrap(), &z[..]);
        assert_eq!(strip_crx(&z).unwrap(), &z[..]);
        assert!(strip_crx(b"Cr24\x03\0\0\0\xff\xff\xff\xff").is_err());
        assert!(strip_crx(b"nope").is_err());
        assert!(strip_crx(b"").is_err());
    }

    #[test]
    fn extract_rejects_zip_slip() {
        let base = std::env::temp_dir().join(format!("rb_zipslip_{}", std::process::id()));
        let dest = base.join("out");
        fs::create_dir_all(&dest).unwrap();
        for bad in ["../evil.txt", "a/../../evil.txt", "/abs.txt"] {
            assert!(extract(&zip_of(&[(bad, "x")]), &dest).is_err(), "{bad}");
        }
        assert!(!base.join("evil.txt").exists());
        extract(&zip_of(&[("manifest.json", "{}"), ("js/a.js", "1")]), &dest).unwrap();
        assert!(dest.join("js/a.js").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn firefox_mv2_converts() {
        let mut m = json!({
            "manifest_version": 2, "name": "T", "version": "1.2.0b3",
            "browser_specific_settings": {"gecko": {"id": "t@x"}},
            "permissions": ["tabs", "menus", "https://*/*", "<all_urls>", "tabHide"],
            "browser_action": {"default_popup": "p.html", "default_icon": "i.svg"},
            "background": {"scripts": ["a.js", "b.js"]},
            "web_accessible_resources": ["x.png"],
            "content_security_policy": "script-src 'self'",
            "icons": {"48": "i.svg", "96": "i.png"},
            "content_scripts": [{"matches": ["<all_urls>"], "js": ["c.js"]}]
        });
        let p = convert(&mut m).unwrap().unwrap();
        assert!(p.firefox);
        assert_eq!(m["manifest_version"], 3);
        assert_eq!(m["version"], "1.2.0.3");
        assert_eq!(m["version_name"], "1.2.0b3");
        assert!(m.get("browser_specific_settings").is_none() && m.get("browser_action").is_none() && m["content_security_policy"].is_object());
        assert_eq!(m["permissions"], json!(["tabs", "contextMenus", "storage"]));
        assert_eq!(m["host_permissions"], json!(["https://*/*", "<all_urls>"]));
        assert_eq!(m["action"], json!({"default_popup": "p.html"}));
        assert_eq!(m["background"], json!({"service_worker": "rb-background.js"}));
        assert_eq!(m["web_accessible_resources"][0]["resources"], json!(["x.png"]));
        assert_eq!(m["icons"], json!({"96": "i.png"}));
        assert_eq!(m["content_scripts"][0]["js"], json!(["rb-polyfill.js", "c.js"]));
        assert_eq!(p.bg, Some((vec!["a.js".into(), "b.js".into()], false)));
        assert!(p.bg_page.is_none());
    }

    #[test]
    fn convert_leaves_chrome_mv3_and_rejects_incompatible() {
        let mut ok = json!({"manifest_version": 3, "name": "T", "version": "1", "background": {"service_worker": "sw.js"}});
        assert!(convert(&mut ok).unwrap().is_none());
        let mut blocking = json!({"manifest_version": 2, "permissions": ["webRequest", "webRequestBlocking"]});
        let p = convert(&mut blocking).unwrap().unwrap();
        assert!(p.mv2 && blocking["manifest_version"] == 2);
        let mut hard = json!({"manifest_version": 2, "permissions": ["webRequestBlocking", "contextualIdentities"]});
        assert!(convert(&mut hard).unwrap_err().contains("contextualIdentities"));
        let mut page = json!({"manifest_version": 2, "background": {"page": "bg.html"}});
        let p = convert(&mut page).unwrap().unwrap();
        assert_eq!(p.bg_page.as_deref(), Some("bg.html"));
        assert_eq!(page["background"], json!({"service_worker": "rb-background.js"}));
        assert!(convert(&mut json!({"manifest_version": 2, "theme": {}})).is_err());
        assert!(convert(&mut json!([])).is_err());
    }

    #[test]
    #[ignore]
    fn preset_installs_from_stores() {
        let dest = std::env::temp_dir().join(format!("rb_preset_{}", std::process::id()));
        for (k, s) in [("ublock", "ddkjiahejlhfcafbddmgiahcphecmpfh"), ("sponsorblock", "mnjggcdmjocbbbhaepdhchncahnbgone"), ("spotify-adblock", "spotify-adblock")] {
            let r = install(parse(s).unwrap(), &dest, &|_| {});
            eprintln!("{k}: {:?}", r.as_ref().map(|d| (&d.name, &d.notes, d.id.clone())));
        }
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    #[ignore]
    fn live_github_ublock() {
        let dest = tmp("live_gh");
        let d = install(Source::Github("gorhill/uBlock".into()), &dest.join("out"), &|_| {}).unwrap();
        let m: Value = serde_json::from_slice(&fs::read(d.folder.join("manifest.json")).unwrap()).unwrap();
        println!("LIVE gh: {} v2={} src={} notes={:?} icons={}", d.name, m["manifest_version"], d.source, d.notes, m["icons"]);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn helpers() {
        assert_eq!(chrome_version("2.0"), "2.0");
        assert_eq!(chrome_version("beta"), "0.0.1");
        assert_eq!(chrome_version("1.2.3.4.5"), "1.2.3.4");
        assert_eq!(chrome_version("99999.1"), "65535.1");
        assert_eq!(inject_html("<html><HEAD lang=x><title>a</title></head>"), "<html><HEAD lang=x><script src=\"/rb-polyfill.js\"></script><title>a</title></head>");
        assert_eq!(inject_html("<p>x"), "<script src=\"/rb-polyfill.js\"></script><p>x");
        assert_eq!(bg_source(&["/a.js".into()], true, false), "importScripts(\"rb-polyfill.js\",\"rb-sw-shim.js\",\"a.js\");\n");
        assert_eq!(bg_source(&["a.js".into()], false, true), "import \"./a.js\";\n");
        assert!(safe_rel("../x.html").is_none() && safe_rel("a/b.html?x").is_some());
        assert_eq!(slug("Ublock Origin!"), "ublock-origin");
        assert_eq!(stem("https://e.com/dl/My Ext.xpi?x=1"), "my-ext");
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rb_ext_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &Path, rel: &str, c: &str) {
        let f = root.join(rel);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, c).unwrap();
    }

    #[test]
    fn html_scripts_and_paths() {
        let h = "<html><head><script src=\"../lib/a.js\"></script><SCRIPT type='module' src=b.js></SCRIPT><script>var x = 1;</script><script src=\"https://cdn/x.js\" defer></script><script></script>";
        assert_eq!(
            page_scripts(h),
            vec![(true, "../lib/a.js".into(), false), (true, "b.js".into(), true), (false, "var x = 1;".into(), false), (true, "https://cdn/x.js".into(), false)]
        );
        assert_eq!(norm("bg/index.html", "../lib/a.js").as_deref(), Some("lib/a.js"));
        assert_eq!(norm("bg/index.html", "/x/y.js?v=1").as_deref(), Some("x/y.js"));
        assert_eq!(norm("index.html", "a/./b.js").as_deref(), Some("a/b.js"));
        assert!(norm("index.html", "../../x.js").is_none() && norm("a.html", "https://x/y.js").is_none() && norm("a.html", "c:\\x.js").is_none());
    }

    #[test]
    fn csp_conversion() {
        let mut notes = vec![];
        let mut o = json!({"content_security_policy": "script-src 'self'; object-src 'self'"}).as_object().unwrap().clone();
        fix_csp(&mut o, &mut notes);
        assert_eq!(o["content_security_policy"], json!({"extension_pages": "script-src 'self'; object-src 'self'"}));
        let mut o = json!({"content_security_policy": "script-src 'self' 'unsafe-eval'; object-src 'self'"}).as_object().unwrap().clone();
        fix_csp(&mut o, &mut notes);
        assert!(!o.contains_key("content_security_policy") && notes.len() == 1);
        let mut o = json!({"content_security_policy": {"extension_pages": "script-src 'self' https://x.com", "sandbox": "sandbox allow-scripts; script-src 'self' 'unsafe-eval'"}}).as_object().unwrap().clone();
        fix_csp(&mut o, &mut notes);
        assert_eq!(o["content_security_policy"], json!({"sandbox": "sandbox allow-scripts; script-src 'self' 'unsafe-eval'"}));
    }

    #[test]
    fn html_background_page_becomes_worker() {
        let r = tmp("bgpage");
        put(&r, "manifest.json", r#"{"manifest_version":2,"name":"__MSG_n__","default_locale":"en-US","version":"1.0","applications":{"gecko":{"id":"a@b","strict_min_version":"60.0"}},
            "background":{"page":"bg/index.html","persistent":true},"browser_action":{"default_popup":"pop/p.html","browser_style":true},
            "options_ui":{"page":"opt.html","browser_style":true,"open_in_tab":true},"sidebar_action":{"default_panel":"s.html"},
            "permissions":["storage","<all_urls>","*://*.x.com/*"],"web_accessible_resources":[],"content_security_policy":"script-src 'self'; object-src 'self'"}"#);
        put(&r, "_locales/en-US/messages.json", r#"{"n":{"message":"Nice Ext"}}"#);
        put(&r, "bg/index.html", "<html><body><script src=\"../lib.js\"></script><script src=\"main.js\"></script><script>window.boot=1</script></body></html>");
        put(&r, "lib.js", "var a=1;");
        put(&r, "bg/main.js", "localStorage.setItem('a','b');");
        put(&r, "pop/p.html", "<!doctype html><html><head><title>p</title></head><body><script src=p.js></script></body></html>");
        put(&r, "opt.html", "<body>opts</body>");
        let notes = prepare(&r, None).unwrap();
        let m: Value = serde_json::from_slice(&fs::read(r.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["manifest_version"], 3);
        assert_eq!(m["background"], json!({"service_worker": "rb-background.js"}));
        assert_eq!(m["action"], json!({"default_popup": "pop/p.html"}));
        assert_eq!(m["options_ui"], json!({"page": "opt.html", "open_in_tab": true}));
        assert_eq!(m["default_locale"], "en_US");
        assert!(r.join("_locales/en_US/messages.json").is_file() && !r.join("_locales/en-US").exists());
        assert_eq!(m["host_permissions"], json!(["<all_urls>", "*://*.x.com/*"]));
        assert_eq!(m["permissions"], json!(["storage"]));
        assert!(m.get("web_accessible_resources").is_none() && m.get("sidebar_action").is_none() && m.get("applications").is_none());
        assert_eq!(m["content_security_policy"], json!({"extension_pages": "script-src 'self'; object-src 'self'"}));
        assert_eq!(fs::read_to_string(r.join("rb-background.js")).unwrap(), "importScripts(\"rb-polyfill.js\",\"rb-sw-shim.js\",\"lib.js\",\"bg/main.js\",\"rb-inline-2.js\");\n");
        assert_eq!(fs::read_to_string(r.join("rb-inline-2.js")).unwrap(), "window.boot=1");
        assert!(fs::read_to_string(r.join("pop/p.html")).unwrap().contains("<head><script src=\"/rb-polyfill.js\"></script>"));
        assert!(fs::read_to_string(r.join("opt.html")).unwrap().starts_with("<script src=\"/rb-polyfill.js\">"));
        assert!(r.join("rb-sw-shim.js").is_file() && r.join("rb-polyfill.js").is_file());
        assert_eq!(localize(&r, &m, "__MSG_N__"), "Nice Ext");
        assert!(notes.iter().any(|n| n.contains("service worker")));
        let _ = fs::remove_dir_all(&r);
    }

    #[test]
    fn locales_without_default_and_missing_dir() {
        let r = tmp("loc");
        put(&r, "_locales/pt-BR/messages.json", "{}");
        put(&r, "_locales/en/messages.json", "{}");
        let mut m = json!({"name": "x"});
        fix_locales(&r, &mut m, &mut vec![]);
        assert_eq!(m["default_locale"], "en");
        assert!(r.join("_locales/pt_BR").is_dir());
        let e = tmp("loc2");
        let mut m = json!({"default_locale": "en"});
        let mut notes = vec![];
        fix_locales(&e, &mut m, &mut notes);
        assert!(m.get("default_locale").is_none() && notes.len() == 1);
        let _ = fs::remove_dir_all(&r);
        let _ = fs::remove_dir_all(&e);
    }

    #[test]
    fn mv3_firefox_with_scripts_and_missing_page_scripts() {
        let r = tmp("mv3ff");
        put(&r, "manifest.json", r#"{"manifest_version":3,"name":"x","version":"2.0","browser_specific_settings":{"gecko":{"id":"x@y"}},"background":{"scripts":["a.js"]},"host_permissions":["https://*/*"],"web_accessible_resources":[{"resources":["a.png"],"matches":["<all_urls>"]}]}"#);
        put(&r, "a.js", "1");
        prepare(&r, None).unwrap();
        let m: Value = serde_json::from_slice(&fs::read(r.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["background"], json!({"service_worker": "rb-background.js"}));
        assert_eq!(m["web_accessible_resources"][0]["resources"], json!(["a.png"]));
        assert_eq!(m["host_permissions"], json!(["https://*/*"]));
        let _ = fs::remove_dir_all(&r);
        let r = tmp("badpage");
        put(&r, "manifest.json", r#"{"manifest_version":2,"name":"x","version":"1","background":{"page":"bg.html"}}"#);
        put(&r, "bg.html", "<script src=\"https://cdn.x/y.js\"></script>");
        let notes = prepare(&r, None).unwrap();
        let m: Value = serde_json::from_slice(&fs::read(r.join("manifest.json")).unwrap()).unwrap();
        assert!(m.get("background").is_none() && notes.iter().any(|n| n.contains("no usable scripts")));
        let _ = fs::remove_dir_all(&r);
    }

    fn ld(tag: u64, b: &[u8]) -> Vec<u8> {
        let mut out = vec![];
        let mut t = tag << 3 | 2;
        loop {
            out.push((t & 0x7f) as u8 | if t > 0x7f { 0x80 } else { 0 });
            t >>= 7;
            if t == 0 {
                break;
            }
        }
        let mut n = b.len();
        loop {
            out.push((n & 0x7f) as u8 | if n > 0x7f { 0x80 } else { 0 });
            n >>= 7;
            if n == 0 {
                break;
            }
        }
        out.extend(b);
        out
    }

    #[test]
    fn crx3_key_and_id() {
        let publisher = [0x30u8, 0x82, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let google = [0x30u8, 0x82, 9, 9, 9, 9, 9, 9, 9, 9, 9];
        let id = &Sha256::digest(publisher)[..16];
        let mut head = ld(2, &[ld(1, &google), ld(2, b"sig")].concat());
        head.extend(ld(2, &[ld(1, &publisher), ld(2, b"sig")].concat()));
        head.extend(ld(10000, &ld(1, id)));
        let mut crx = b"Cr24".to_vec();
        crx.extend(3u32.to_le_bytes());
        crx.extend((head.len() as u32).to_le_bytes());
        crx.extend(&head);
        crx.extend(zip_of(&[("manifest.json", "{}")]));
        let (k, eid) = crx_key(&crx).unwrap();
        assert_eq!(STANDARD.decode(k).unwrap(), publisher);
        assert_eq!(eid.len(), 32);
        assert!(is_chrome_id(&eid));
        assert!(crx_key(b"PK\x03\x04").is_none());
    }

    #[test]
    fn key_is_injected_for_mv3() {
        let r = tmp("key");
        put(&r, "manifest.json", r#"{"manifest_version":3,"name":"x","version":"1"}"#);
        prepare(&r, Some("QUJD")).unwrap();
        let m: Value = serde_json::from_slice(&fs::read(r.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["key"], "QUJD");
        let _ = fs::remove_dir_all(&r);
    }

    fn check_installed(root: &Path) -> Vec<String> {
        let mut bad = vec![];
        let m: Value = serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        if m["manifest_version"] != 3 {
            bad.push("manifest_version".into());
        }
        let mut need: Vec<String> = vec![];
        let mut add = |v: &Value| {
            if let Some(s) = v.as_str() {
                need.push(s.to_owned());
            }
        };
        add(&m["background"]["service_worker"]);
        add(&m["action"]["default_popup"]);
        add(&m["options_ui"]["page"]);
        add(&m["options_page"]);
        for c in m["content_scripts"].as_array().into_iter().flatten() {
            for k in ["js", "css"] {
                c[k].as_array().into_iter().flatten().for_each(&mut add);
            }
        }
        for v in m["icons"].as_object().into_iter().flat_map(|o| o.values()) {
            add(v);
        }
        for n in need {
            if !n.contains('*') && !root.join(n.trim_start_matches('/')).is_file() {
                bad.push(format!("missing {n}"));
            }
        }
        for k in ["browser_action", "page_action", "applications", "browser_specific_settings"] {
            if m.get(k).is_some() {
                bad.push(format!("leftover {k}"));
            }
        }
        if m["permissions"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|p| p.contains("://") || p == "<all_urls>") {
            bad.push("host in permissions".into());
        }
        if m["web_accessible_resources"].as_array().into_iter().flatten().any(|w| !w.is_object()) {
            bad.push("war format".into());
        }
        if m["content_security_policy"].is_string() {
            bad.push("csp string".into());
        }
        bad
    }

    #[test]
    fn package_urls() {
        assert!(is_package_url("https://addons.mozilla.org/firefox/downloads/file/123/ublock-1.0.xpi?src=x"));
        assert!(is_package_url("https://example.com/a/B.CRX#f"));
        assert!(!is_package_url("http://example.com/a.xpi"));
        assert!(!is_package_url("https://example.com/a.zip"));
        assert!(!is_package_url("https://addons.mozilla.org/en-US/firefox/addon/ublock-origin/"));
        assert!(!is_package_url("https://example.com/?f=a.xpi"));
        assert!(!is_package_url("https://example.xpi"));
    }

    #[test]
    fn store_requests() {
        let m = |c: &str, k: &str, id: &str| format!(r#"{{"c":"{c}","k":"{k}","id":"{id}"}}"#);
        assert_eq!(store_request("chromewebstore.google.com", &m("store_install", "chrome", ID)), Some(Source::Chrome(ID.into())));
        assert_eq!(store_request("chrome.google.com", &m("store_install", "chrome", ID)), Some(Source::Chrome(ID.into())));
        assert_eq!(store_request("addons.mozilla.org", &m("store_install", "amo", "ublock-origin")), Some(Source::Amo("ublock-origin".into())));
        assert_eq!(store_request("evil.com", &m("store_install", "chrome", ID)), None);
        assert_eq!(store_request("addons.mozilla.org", &m("store_install", "chrome", ID)), None);
        assert_eq!(store_request("chromewebstore.google.com", &m("store_install", "amo", "ublock-origin")), None);
        assert_eq!(store_request("chromewebstore.google.com", &m("store_install", "chrome", "short")), None);
        assert_eq!(store_request("chromewebstore.google.com", &m("store_install", "chrome", &ID.to_uppercase())), None);
        assert_eq!(store_request("addons.mozilla.org", &m("store_install", "amo", "../x")), None);
        assert_eq!(store_request("addons.mozilla.org", &m("settings", "amo", "x")), None);
        assert_eq!(store_request("addons.mozilla.org", "not json"), None);
        assert_eq!(store_request("addons.mozilla.org", r#"{"c":"store_install","k":"amo"}"#), None);
    }

    #[test]
    fn store_folders() {
        assert_eq!(folder_of(&Source::Chrome(ID.into())).as_deref(), Some(ID));
        assert_eq!(folder_of(&Source::Amo("Spotify-Adblock".into())).as_deref(), Some("spotify-adblock"));
        assert_eq!(folder_of(&Source::Url("https://a/b.xpi".into())), None);
    }

    #[test]
    fn parse_store_pages() {
        assert_eq!(parse("https://addons.mozilla.org/en-US/firefox/addon/spotify-adblock/").unwrap(), Source::Amo("spotify-adblock".into()));
        assert_eq!(parse("https://addons.mozilla.org/firefox/addon/ublock-origin/?utm=1").unwrap(), Source::Amo("ublock-origin".into()));
        assert!(parse("https://addons.mozilla.org/en-US/firefox/").is_err());
    }

    #[test]
    #[ignore]
    fn live_amo_extensions() {
        let slugs = std::env::var("RB_AMO").unwrap_or_else(|_| "darkreader,ublock-origin,stylus".into());
        for s in slugs.split(',') {
            let dest = tmp(&format!("live_{s}"));
            match install(Source::Amo(s.to_owned()), &dest.join("out"), &|_| {}) {
                Ok(d) => println!("LIVE {s}: ok name={:?} notes={:?} problems={:?}", d.name, d.notes, check_installed(&d.folder)),
                Err(e) => println!("LIVE {s}: refused: {e}"),
            }
            let _ = fs::remove_dir_all(&dest);
        }
    }
}
