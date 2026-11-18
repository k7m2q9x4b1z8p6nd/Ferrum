use crate::{settings, ONBOARDING};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_TABS: usize = 100;
const MAX_URL: usize = 4096;
const MAX_TITLE: usize = 300;
const MAX_SCROLL: f64 = 1e7;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Entry {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub scroll: f64,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct Session {
    pub tabs: Vec<Entry>,
    #[serde(default)]
    pub active: usize,
}

fn keep(url: &str) -> bool {
    url.len() <= MAX_URL && !url.starts_with(ONBOARDING) && ["http://", "https://", "file://"].iter().any(|p| url.starts_with(p))
}

fn clean(raw: Vec<Entry>, active: usize) -> Session {
    let mut tabs = vec![];
    let mut act = 0;
    for (i, mut e) in raw.into_iter().enumerate() {
        if i == active {
            act = tabs.len();
        }
        if !keep(&e.url) {
            continue;
        }
        e.title = e.title.chars().take(MAX_TITLE).collect();
        e.scroll = if e.scroll.is_finite() { e.scroll.clamp(0.0, MAX_SCROLL) } else { 0.0 };
        tabs.push(e);
    }
    tabs.truncate(MAX_TABS);
    Session { active: act.min(tabs.len().saturating_sub(1)), tabs }
}

pub fn capture(tabs: impl IntoIterator<Item = (Entry, bool)>, active: usize) -> Session {
    let mut raw = vec![];
    let mut act = 0;
    for (i, (e, private)) in tabs.into_iter().enumerate() {
        if i == active {
            act = raw.len();
        }
        if !private {
            raw.push(e);
        }
    }
    clean(raw, act)
}

pub fn encode(s: &Session) -> Vec<u8> {
    serde_json::to_vec(s).unwrap_or_default()
}

pub fn decode(b: &[u8]) -> Option<Session> {
    let s: Session = serde_json::from_slice(b).ok()?;
    let s = clean(s.tabs, s.active);
    (!s.tabs.is_empty()).then_some(s)
}

fn path() -> PathBuf {
    settings::dir().join("session.json")
}

pub fn load() -> Option<Session> {
    decode(&std::fs::read(path()).ok()?)
}

pub fn clear() {
    let _ = std::fs::remove_file(path());
}

pub fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dest.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub fn write(bytes: &[u8]) {
    let _ = write_atomic(&path(), bytes);
}

pub fn write_bg(bytes: Vec<u8>) {
    std::thread::spawn(move || write(&bytes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(url: &str) -> (Entry, bool) {
        (Entry { url: url.into(), title: format!("t {url}"), pinned: false, scroll: 0.0 }, false)
    }

    #[test]
    fn drops_private_empty_invalid_and_onboarding() {
        let mut p = e("https://p.example/");
        p.1 = true;
        let s = capture([e("https://a.example/"), p, e(""), e("javascript:1"), e("chrome-extension://x/a.html"), e(ONBOARDING), e("http://rb.localhost/settings"), e("file:///c:/a.html")], 0);
        let urls: Vec<_> = s.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://a.example/", "http://rb.localhost/settings", "file:///c:/a.html"]);
    }

    #[test]
    fn active_follows_the_filtered_list() {
        let mut p = e("https://p.example/");
        p.1 = true;
        let s = capture([p, e("https://a.example/"), e(""), e("https://b.example/")], 3);
        assert_eq!(s.active, 1);
        assert_eq!(s.tabs[s.active].url, "https://b.example/");
    }

    #[test]
    fn active_on_dropped_tab_falls_to_next_and_clamps() {
        assert_eq!(capture([e("https://a/"), e(""), e("https://c/")], 1).active, 1);
        assert_eq!(capture([e("https://a/"), e("")], 1).active, 0);
        assert_eq!(capture([e("https://a/")], 9).active, 0);
    }

    #[test]
    fn caps_tab_count() {
        let s = capture((0..250).map(|i| e(&format!("https://a/{i}"))), 200);
        assert_eq!(s.tabs.len(), MAX_TABS);
        assert_eq!(s.active, MAX_TABS - 1);
    }

    #[test]
    fn roundtrip_keeps_state() {
        let mut a = e("https://a.example/").0;
        a.pinned = true;
        a.scroll = 123.5;
        let s = capture([(a, false), e("https://b.example/")], 1);
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    #[test]
    fn decode_sanitises_and_rejects_garbage() {
        assert_eq!(decode(b"not json"), None);
        assert_eq!(decode(br#"{"tabs":[]}"#), None);
        assert_eq!(decode(br#"{"tabs":[{"url":"ftp://x/"},{"url":""}]}"#), None);
        let s = decode(br#"{"tabs":[{"url":"https://a/","scroll":-5},{"url":"https://b/","scroll":1e30}],"active":7}"#).unwrap();
        assert_eq!((s.tabs[0].scroll, s.tabs[1].scroll, s.active), (0.0, MAX_SCROLL, 0));
        assert_eq!(s.tabs[0].title, "");
    }

    #[test]
    fn title_is_capped() {
        let mut a = e("https://a/");
        a.0.title = "x".repeat(MAX_TITLE * 3);
        assert_eq!(capture([a], 0).tabs[0].title.chars().count(), MAX_TITLE);
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = std::env::temp_dir().join(format!("rb_sess_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("session.json");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
