use crate::{extinstall, extupdate, settings};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::{Path, PathBuf}};
use webview2_com::{
    take_pwstr,
    BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler, Microsoft::Web::WebView2::Win32::*, ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
};
use windows::core::{Interface, BOOL, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

pub fn dir() -> PathBuf {
    let d = settings::dir().join("extensions");
    let _ = std::fs::create_dir_all(&d);
    d
}

const ICON_MAX: u64 = 256 << 10;
const ICON_MIME: &[(&str, &str)] = &[("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("webp", "image/webp"), ("gif", "image/gif"), ("svg", "image/svg+xml")];

#[derive(Deserialize)]
#[serde(untagged)]
enum Raw {
    Folder(String),
    Full { folder: String, #[serde(default)] source: String },
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(from = "Raw")]
struct Rec {
    folder: String,
    source: String,
}

impl From<Raw> for Rec {
    fn from(r: Raw) -> Self {
        match r {
            Raw::Folder(folder) => Rec { folder, source: String::new() },
            Raw::Full { folder, source } => Rec { folder, source },
        }
    }
}

fn index() -> BTreeMap<String, Rec> {
    std::fs::read(settings::dir().join("extensions.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_index(i: &BTreeMap<String, Rec>) {
    if let Ok(b) = serde_json::to_vec_pretty(i) {
        let _ = std::fs::write(settings::dir().join("extensions.json"), b);
    }
}

const PRESET: &[(&str, &str)] = &[("ublock-origin", "gorhill/uBlock"), ("sponsorblock", "mnjggcdmjocbbbhaepdhchncahnbgone"), ("spotify-adblock", "spotify-adblock")];

fn preset_file() -> PathBuf {
    settings::dir().join("preinstalled.json")
}

fn preset_done() -> Option<Vec<String>> {
    serde_json::from_slice(&std::fs::read(preset_file()).ok()?).ok()
}

pub fn preset_mark(key: &str) {
    let mut d = preset_done().unwrap_or_default();
    if !d.iter().any(|k| k == key) {
        d.push(key.to_owned());
        if let Ok(b) = serde_json::to_vec(&d) {
            let _ = std::fs::write(preset_file(), b);
        }
    }
}

pub fn preset_pending() -> Vec<(&'static str, extinstall::Source)> {
    let done = preset_done();
    let existing = done.is_none() && std::fs::read_dir(dir()).is_ok_and(|mut r| r.any(|e| e.is_ok_and(|e| e.path().is_dir())));
    if existing {
        PRESET.iter().for_each(|(k, _)| preset_mark(k));
        return vec![];
    }
    let done = done.unwrap_or_default();
    PRESET
        .iter()
        .filter(|(k, _)| !done.iter().any(|d| d == k))
        .map(|(k, s)| (*k, if extinstall::is_chrome_id(s) { extinstall::Source::Chrome((*s).to_owned()) } else if s.contains('/') { extinstall::Source::Github((*s).to_owned()) } else { extinstall::Source::Amo((*s).to_owned()) }))
        .collect()
}

const FENCE: &str = "*://rb.localhost/*";

fn fence_manifest(m: &mut Value) -> bool {
    let mut changed = false;
    for c in m.get_mut("content_scripts").and_then(Value::as_array_mut).into_iter().flatten().filter_map(Value::as_object_mut) {
        if let Some(list) = c.entry("exclude_matches").or_insert_with(|| Value::Array(vec![])).as_array_mut().filter(|l| !l.iter().any(|v| v == FENCE)) {
            list.push(FENCE.into());
            changed = true;
        }
    }
    changed
}

fn fence(root: &Path) {
    let mp = root.join("manifest.json");
    let Some(mut m) = std::fs::read_to_string(&mp).ok().and_then(|r| serde_json::from_str::<Value>(r.trim_start_matches('\u{feff}')).ok()) else { return };
    if fence_manifest(&mut m) {
        if let Ok(b) = serde_json::to_vec_pretty(&m) {
            let _ = std::fs::write(&mp, b);
        }
    }
}

pub fn drop_local() {
    let Some(i) = std::fs::read(settings::dir().join("extensions.json")).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, Rec>>(&b).ok()) else { return };
    let mut i = i;
    let stale = [extinstall::slug("ddkjiahejlhfcafbddmgiahcphecmpfh"), "ublock-origin".to_owned()];
    if i.values().any(|r| stale.contains(&r.folder)) {
        i.retain(|_, r| !stale.contains(&r.folder));
        save_index(&i);
    }
    for f in ["easylist.txt", "ublock.txt", "privacy.txt", "unbreak.txt"] {
        let _ = std::fs::remove_file(settings::dir().join(f));
    }
    let keep: Vec<String> = i.into_values().map(|r| r.folder).collect();
    let mut any = false;
    for e in std::fs::read_dir(dir()).into_iter().flatten().flatten() {
        if !e.path().is_dir() {
            continue;
        }
        if keep.iter().any(|k| e.file_name() == k.as_str()) {
            fence(&e.path());
        } else {
            any |= std::fs::remove_dir_all(e.path()).is_ok();
        }
    }
    if any {
        let _ = std::fs::write(preset_file(), "[]");
    }
}

pub fn forget_local(wv: &WebView) {
    each(wv, |v| {
        for (e, _) in v.into_iter().filter(|(_, x)| !x.rm && !x.name.starts_with("Microsoft")) {
            let h = BrowserExtensionRemoveCompletedHandler::create(Box::new(|_| Ok(())));
            unsafe {
                let _ = e.Remove(&h);
            }
        }
    });
}

pub fn index_set(id: &str, folder: &str, source: &str) {
    let mut i = index();
    i.insert(id.to_owned(), Rec { folder: folder.to_owned(), source: source.to_owned() });
    save_index(&i);
}

pub fn index_take(id: &str) -> Option<String> {
    let mut i = index();
    let f = i.remove(id);
    save_index(&i);
    f.map(|r| r.folder)
}

pub fn installed() -> Vec<String> {
    index().into_values().map(|r| r.folder).collect()
}

pub fn storage_dir(id: &str) -> PathBuf {
    settings::dir().join("webview").join("EBWebView").join("Default").join("Local Extension Settings").join(id)
}

fn source_of(rec: &Rec, root: &Path) -> &'static str {
    match rec.source.as_str() {
        "chrome" => "chrome",
        "firefox" => "firefox",
        "local" => "local",
        _ if extinstall::is_chrome_id(&rec.folder) => "chrome",
        _ if root.join("rb-polyfill.js").is_file() => "firefox",
        _ => "local",
    }
}

#[derive(Default)]
struct Meta {
    name: String,
    version: String,
    icon: String,
    options: bool,
    popup: bool,
}

fn data_url(p: &Path) -> Option<String> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    let mime = ICON_MIME.iter().find(|(e, _)| *e == ext)?.1;
    (std::fs::metadata(p).ok()?.len() <= ICON_MAX).then_some(())?;
    Some(format!("data:{mime};base64,{}", STANDARD.encode(std::fs::read(p).ok()?)))
}

fn best_icon(root: &Path, v: &Value) -> Option<PathBuf> {
    let mut c: Vec<(u32, &str)> = match v {
        Value::String(s) => vec![(0, s.as_str())],
        Value::Object(m) => m.iter().filter_map(|(k, p)| Some((k.parse().unwrap_or(0), p.as_str()?))).collect(),
        _ => vec![],
    };
    c.sort_by_key(|(n, _)| (*n < 32, if *n < 32 { u32::MAX - n } else { *n }));
    c.iter().find_map(|(_, p)| extinstall::safe_rel(p).map(|r| root.join(r)).filter(|f| f.is_file()))
}

fn scan_icon(root: &Path) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = ["images", "icons", "img", "assets", "icon"]
        .iter()
        .filter_map(|d| std::fs::read_dir(root.join(d)).ok())
        .flat_map(|r| r.flatten().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("png")) && std::fs::metadata(p).is_ok_and(|m| m.len() <= ICON_MAX))
        .collect();
    c.sort_by_key(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        (!(n.contains("icon") || n.contains("logo")), !n.contains("128"), n)
    });
    c.into_iter().next()
}

fn page(m: &Value, keys: &[&[&str]]) -> Option<String> {
    keys.iter().find_map(|path| path.iter().try_fold(m, |v, k| v.get(*k))?.as_str().filter(|p| extinstall::safe_rel(p).is_some()).map(str::to_owned))
}

fn meta(root: &Path) -> Meta {
    let Some(m) = std::fs::read(root.join("manifest.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return Meta::default() };
    let s = |k: &str| m[k].as_str().map(|v| extinstall::localize(root, &m, v)).unwrap_or_default();
    let icon = best_icon(root, &m["icons"])
        .or_else(|| best_icon(root, &m["action"]["default_icon"]))
        .or_else(|| best_icon(root, &m["browser_action"]["default_icon"]))
        .or_else(|| std::fs::read(root.join("rb-icon.txt")).ok().and_then(|b| extinstall::safe_rel(String::from_utf8_lossy(&b).trim())).map(|r| root.join(r)).filter(|f| f.is_file()))
        .or_else(|| scan_icon(root))
        .and_then(|p| data_url(&p))
        .unwrap_or_default();
    Meta {
        name: s("name"),
        version: m["version_name"].as_str().or(m["version"].as_str()).unwrap_or_default().to_owned(),
        icon,
        options: page(&m, &[&["options_ui", "page"], &["options_page"]]).is_some(),
        popup: page(&m, &[&["action", "default_popup"], &["browser_action", "default_popup"], &["page_action", "default_popup"]]).is_some(),
    }
}

pub fn page_url(id: &str, which: &str) -> Option<String> {
    let rec = index().remove(id).filter(|_| extinstall::is_chrome_id(id))?;
    let root = dir().join(extinstall::safe_rel(&rec.folder)?);
    let m: Value = serde_json::from_slice(&std::fs::read(root.join("manifest.json")).ok()?).ok()?;
    let rel = match which {
        "options" => page(&m, &[&["options_ui", "page"], &["options_page"]])?,
        _ => page(&m, &[&["action", "default_popup"], &["browser_action", "default_popup"], &["page_action", "default_popup"]])?,
    };
    Some(format!("chrome-extension://{id}/{}", rel.trim_start_matches('/')))
}

#[derive(Serialize)]
pub struct Ext {
    pub id: String,
    pub name: String,
    pub on: bool,
    pub rm: bool,
    pub version: String,
    pub source: &'static str,
    pub icon: String,
    pub opt: bool,
    pub pop: bool,
    pub up: bool,
}

fn each(wv: &WebView, f: impl FnOnce(Vec<(ICoreWebView2BrowserExtension, Ext)>) + 'static) {
    unsafe {
        let Ok(profile) = wv.webview().cast::<ICoreWebView2_13>().and_then(|w| w.Profile()).and_then(|p| p.cast::<ICoreWebView2Profile7>()) else {
            return f(vec![]);
        };
        let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_, list| {
            let mut out = vec![];
            let known = index();
            if let Some(list) = list {
                let mut n = 0;
                list.Count(&mut n)?;
                for i in 0..n {
                    let e = list.GetValueAtIndex(i)?;
                    let (mut id, mut name, mut on) = (PWSTR::null(), PWSTR::null(), BOOL(0));
                    e.Id(&mut id)?;
                    e.Name(&mut name)?;
                    e.IsEnabled(&mut on)?;
                    let (id, name) = (take_pwstr(id), take_pwstr(name));
                    let rec = known.get(&id);
                    let root = rec.and_then(|r| extinstall::safe_rel(&r.folder)).map(|f| dir().join(f));
                    let mt = root.as_deref().map(meta).unwrap_or_default();
                    let name = if mt.name.is_empty() { name } else { mt.name };
                    let source = rec.zip(root.as_deref()).map_or("local", |(r, p)| source_of(r, p));
                    out.push((e, Ext { rm: rec.is_some(), id, name, on: on.as_bool(), version: mt.version, source, icon: mt.icon, opt: mt.options, pop: mt.popup, up: rec.is_some_and(|r| extupdate::endpoint(source, &r.folder).is_some()) }));
                }
            }
            f(out);
            Ok(())
        }));
        let _ = profile.GetBrowserExtensions(&handler);
    }
}

pub fn list(wv: &WebView, done: impl FnOnce(Vec<Ext>) + 'static) {
    each(wv, move |v| done(v.into_iter().map(|(_, e)| e).filter(|e| !e.name.starts_with("Microsoft")).collect()));
}

pub fn enable(wv: &WebView, id: String, on: bool) {
    each(wv, move |v| {
        if let Some((e, _)) = v.into_iter().find(|(_, x)| x.id == id) {
            let h = BrowserExtensionEnableCompletedHandler::create(Box::new(|_| Ok(())));
            unsafe {
                let _ = e.Enable(on, &h);
            }
        }
    });
}

fn profile(wv: &WebView) -> Option<ICoreWebView2Profile7> {
    unsafe { wv.webview().cast::<ICoreWebView2_13>().and_then(|w| w.Profile()).and_then(|p| p.cast::<ICoreWebView2Profile7>()).ok() }
}

fn ext_id(e: &ICoreWebView2BrowserExtension) -> Result<String, String> {
    let mut id = PWSTR::null();
    unsafe { e.Id(&mut id).map_err(|e| e.message())? };
    Ok(take_pwstr(id))
}

fn add_raw(profile: &ICoreWebView2Profile7, path: &Path, done: impl FnOnce(Result<ICoreWebView2BrowserExtension, String>) + 'static) {
    fence(path);
    let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |r, e| {
        done(r.map_err(|e| format!("The browser rejected the extension: {}", e.message())).and_then(|_| e.ok_or_else(|| "The browser rejected the extension".to_owned())));
        Ok(())
    }));
    unsafe {
        let _ = profile.AddBrowserExtension(&HSTRING::from(path), &handler);
    }
}

pub fn add(wv: &WebView, path: PathBuf, done: impl FnOnce(Result<String, String>) + 'static) {
    let Some(profile) = profile(wv) else {
        return done(Err("Extensions are unavailable".into()));
    };
    add_raw(&profile, &path, move |r| done(r.and_then(|e| ext_id(&e))));
}

fn disable(e: &ICoreWebView2BrowserExtension) {
    let h = BrowserExtensionEnableCompletedHandler::create(Box::new(|_| Ok(())));
    unsafe {
        let _ = e.Enable(false, &h);
    }
}

fn rollback(profile: ICoreWebView2Profile7, old: PathBuf, bak: PathBuf, on: bool, err: String, done: impl FnOnce(Result<String, String>) + 'static) {
    if bak.exists() {
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::rename(&bak, &old);
    }
    add_raw(&profile, &old, move |r| {
        if let (Ok(e), false) = (&r, on) {
            disable(e);
        }
        done(Err(err));
    });
}

pub fn replace(wv: &WebView, id: String, staged: PathBuf, source: &'static str, done: impl FnOnce(Result<String, String>) + 'static) {
    let Some(profile) = profile(wv) else {
        return done(Err("Extensions are unavailable".into()));
    };
    let Some(folder) = index().get(&id).map(|r| r.folder.clone()).filter(|f| extinstall::safe_rel(f).is_some()) else {
        return done(Err("Extension cannot be updated".into()));
    };
    each(wv, move |v| {
        let Some((ext, x)) = v.into_iter().find(|(_, x)| x.id == id) else {
            return done(Err("Extension not found".into()));
        };
        let (on, old, bak) = (x.on, dir().join(&folder), dir().join(format!("{folder}.old")));
        let h = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |r| {
            if r.is_err() {
                done(Err("Could not replace the old version".into()));
                return Ok(());
            }
            let _ = std::fs::remove_dir_all(&bak);
            if let Err(e) = std::fs::rename(&old, &bak).and_then(|_| std::fs::rename(&staged, &old)) {
                rollback(profile, old, bak, on, format!("Could not replace files: {e}"), done);
                return Ok(());
            }
            let p = profile.clone();
            add_raw(&profile, &old.clone(), move |r| match r {
                Ok(e) => {
                    if !on {
                        disable(&e);
                    }
                    let _ = std::fs::remove_dir_all(&bak);
                    match ext_id(&e) {
                        Ok(new) => {
                            if new != id {
                                index_take(&id);
                            }
                            index_set(&new, &folder, source);
                            done(Ok(new));
                        }
                        Err(e) => done(Err(e)),
                    }
                }
                Err(e) => rollback(p, old, bak, on, e, done),
            });
            Ok(())
        }));
        unsafe {
            let _ = ext.Remove(&h);
        }
    });
}

pub fn targets() -> Vec<(String, &'static str, String, String)> {
    index()
        .into_iter()
        .filter_map(|(id, r)| {
            let root = dir().join(extinstall::safe_rel(&r.folder)?);
            Some((id, source_of(&r, &root), r.folder, meta(&root).version))
        })
        .collect()
}

pub fn remove(wv: &WebView, id: String, done: impl FnOnce(bool) + 'static) {
    each(wv, move |v| match v.into_iter().find(|(_, x)| x.id == id) {
        Some((e, _)) => {
            let done = std::cell::RefCell::new(Some(done));
            let h = BrowserExtensionRemoveCompletedHandler::create(Box::new(move |r| {
                if let Some(f) = done.borrow_mut().take() {
                    f(r.is_ok());
                }
                Ok(())
            }));
            unsafe {
                let _ = e.Remove(&h);
            }
        }
        None => done(false),
    });
}

#[cfg(test)]
mod tests {
    use super::fence_manifest;
    use serde_json::json;

    #[test]
    fn fences_every_content_script_once() {
        let mut m = json!({"content_scripts": [{"matches": ["<all_urls>"]}, {"matches": ["https://a/*"], "exclude_matches": ["https://b/*"]}]});
        assert!(fence_manifest(&mut m));
        assert_eq!(m["content_scripts"][0]["exclude_matches"], json!(["*://rb.localhost/*"]));
        assert_eq!(m["content_scripts"][1]["exclude_matches"], json!(["https://b/*", "*://rb.localhost/*"]));
        assert!(!fence_manifest(&mut m));
    }

    #[test]
    fn manifest_without_content_scripts_is_untouched() {
        let mut m = json!({"name": "x"});
        assert!(!fence_manifest(&mut m));
        assert_eq!(m, json!({"name": "x"}));
    }
}
