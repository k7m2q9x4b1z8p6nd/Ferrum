use crate::{icons, settings};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    pub last: u64,
}

const LOCAL: usize = 5;
const MAX: usize = 8;

#[derive(Serialize, Deserialize)]
struct Entry {
    url: String,
    title: String,
    visits: u32,
    last: u64,
    search: bool,
}

#[derive(Default)]
pub struct History {
    items: Vec<Entry>,
    hosts: HashMap<String, u32>,
    pub dirty: bool,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn bare(url: &str) -> &str {
    let u = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    u.strip_prefix("www.").unwrap_or(u)
}

fn host(url: &str) -> String {
    bare(url).split(['/', '?', '#']).next().unwrap_or("").to_lowercase()
}

fn best_host<'a>(hosts: &'a HashMap<String, u32>, prefix: &str) -> Option<&'a str> {
    hosts
        .iter()
        .filter(|(h, _)| h.len() > prefix.len() && h.starts_with(prefix))
        .min_by(|a, b| b.1.cmp(a.1).then(a.0.len().cmp(&b.0.len())).then(a.0.cmp(b.0)))
        .map(|(h, _)| h.as_str())
}

fn tracked(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else { return false };
    !rest.split(['/', '?', '#']).next().unwrap_or("").eq_ignore_ascii_case("rb.localhost")
}

impl History {
    pub fn load() -> Self {
        let items: Vec<Entry> = std::fs::read(settings::dir().join("history.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut hosts = HashMap::new();
        for e in items.iter().filter(|e| !e.search) {
            *hosts.entry(host(&e.url)).or_insert(0u32) += e.visits;
        }
        Self { items, hosts, dirty: false }
    }

    pub fn save(&mut self) {
        self.dirty = false;
        if let Ok(b) = serde_json::to_vec(&self.items) {
            let _ = std::fs::write(settings::dir().join("history.json"), b);
        }
    }

    pub fn save_bg(&mut self) {
        self.dirty = false;
        if let Ok(b) = serde_json::to_vec(&self.items) {
            std::thread::spawn(move || {
                let _ = std::fs::write(settings::dir().join("history.json"), b);
            });
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.hosts.clear();
        self.dirty = true;
    }

    fn record(&mut self, url: &str, title: &str, search: bool) {
        if !search {
            *self.hosts.entry(host(url)).or_insert(0) += 1;
        }
        match self.items.iter_mut().find(|e| e.search == search && e.url == url) {
            Some(e) => {
                e.visits += 1;
                e.last = now();
                if !title.is_empty() {
                    e.title = title.to_owned();
                }
            }
            None => {
                self.items.push(Entry { url: url.to_owned(), title: title.to_owned(), visits: 1, last: now(), search });
            }
        }
        self.dirty = true;
    }

    pub fn import(&mut self, list: Vec<Visit>) -> usize {
        let mut index: HashMap<String, usize> = self.items.iter().enumerate().filter(|(_, e)| !e.search).map(|(i, e)| (e.url.clone(), i)).collect();
        let mut n = 0;
        for v in list.into_iter().filter(|v| tracked(&v.url)) {
            *self.hosts.entry(host(&v.url)).or_insert(0) += v.visits.max(1);
            match index.get(&v.url) {
                Some(&i) => {
                    let e = &mut self.items[i];
                    e.visits = e.visits.saturating_add(v.visits);
                    e.last = e.last.max(v.last);
                    if e.title.is_empty() {
                        e.title = v.title;
                    }
                }
                None => {
                    index.insert(v.url.clone(), self.items.len());
                    self.items.push(Entry { url: v.url, title: v.title, visits: v.visits.max(1), last: v.last, search: false });
                    n += 1;
                }
            }
        }
        self.dirty |= n > 0;
        n
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        if tracked(url) {
            self.record(url, title, false);
        }
    }

    pub fn search(&mut self, q: &str) {
        let q = q.trim();
        if !q.is_empty() {
            self.record(q, "", true);
        }
    }

    pub fn complete(&self, q: &str) -> Option<String> {
        let q = q.to_lowercase();
        if q.is_empty() || q.contains(char::is_whitespace) || q.contains('/') {
            return None;
        }
        best_host(&self.hosts, &q).map(str::to_owned)
    }

    pub fn retitle(&mut self, url: &str, title: &str) {
        if let Some(e) = self.items.iter_mut().find(|e| !e.search && e.url == url).filter(|e| e.title != title) {
            e.title = title.to_owned();
            self.dirty = true;
        }
    }

    pub fn recent(&self) -> Vec<Value> {
        let mut v: Vec<&Entry> = self.items.iter().collect();
        v.sort_by_key(|e| Reverse(e.last));
        v.iter().take(300).map(|e| json!({"url": e.url, "title": e.title, "visits": e.visits, "last": e.last, "search": e.search})).collect()
    }

    pub fn suggest(&self, q: &str, direct: Option<String>, engine: &[String]) -> Vec<Value> {
        let ql = q.trim().to_lowercase();
        if ql.is_empty() {
            return vec![];
        }
        let t = now();
        let mut hits: Vec<(u64, &Entry)> = self
            .items
            .iter()
            .filter_map(|e| {
                let (a, b) = if e.search { (e.url.to_lowercase(), String::new()) } else { (e.title.to_lowercase(), bare(&e.url).to_lowercase()) };
                let prefix = a.starts_with(&ql) || b.starts_with(&ql);
                if !prefix && !a.contains(&ql) && !b.contains(&ql) {
                    return None;
                }
                let age = t.saturating_sub(e.last) / 86400;
                Some((if prefix { 2000 } else { 0 } + u64::from(e.visits.min(100)) * 20 + 500u64.saturating_sub(age * 5), e))
            })
            .collect();
        hits.sort_by_key(|h| Reverse(h.0));
        let mut seen = HashSet::new();
        let mut out = vec![];
        if let Some(u) = direct {
            seen.insert(u.trim_end_matches('/').to_lowercase());
            out.push(json!({"q": u, "l": u, "d": "", "k": "h", "h": icons::host_of(&u)}));
        }
        for (_, e) in hits {
            if out.len() >= LOCAL {
                break;
            }
            if !seen.insert(e.url.trim_end_matches('/').to_lowercase()) {
                continue;
            }
            out.push(if e.search {
                json!({"q": e.url, "l": e.url, "d": "", "k": "s"})
            } else {
                json!({"q": e.url, "l": if e.title.is_empty() { bare(&e.url) } else { &e.title }, "d": bare(&e.url), "k": "h", "h": icons::host_of(&e.url)})
            });
        }
        for s in engine {
            if out.len() >= MAX {
                break;
            }
            if seen.insert(s.to_lowercase()) {
                out.push(json!({"q": s, "l": s, "d": "", "k": "s"}));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(l: &[(&str, u32)]) -> HashMap<String, u32> {
        l.iter().map(|(h, n)| ((*h).to_owned(), *n)).collect()
    }

    #[test]
    fn most_visited_prefix_wins() {
        let m = map(&[("youtube.com", 50), ("yoursite.org", 3), ("example.com", 99)]);
        assert_eq!(best_host(&m, "yo"), Some("youtube.com"));
        assert_eq!(best_host(&m, "your"), Some("yoursite.org"));
    }

    #[test]
    fn ties_prefer_shorter_then_alphabetical() {
        let m = map(&[("abc.com", 5), ("ab.io", 5), ("abd.io", 5)]);
        assert_eq!(best_host(&m, "a"), Some("ab.io"));
        assert_eq!(best_host(&m, "abc"), Some("abc.com"));
    }

    #[test]
    fn exact_or_missing_gives_none() {
        let m = map(&[("youtube.com", 5)]);
        assert_eq!(best_host(&m, "youtube.com"), None);
        assert_eq!(best_host(&m, "z"), None);
    }

    #[test]
    fn counts_track_visits_import_and_clear() {
        let mut h = History::default();
        h.visit("https://www.youtube.com/watch?v=1", "");
        h.visit("https://www.yoursite.org/", "");
        h.visit("https://www.yoursite.org/", "");
        assert_eq!(h.complete("yo").as_deref(), Some("yoursite.org"));
        h.import(vec![Visit { url: "https://youtube.com/a".into(), title: String::new(), visits: 10, last: 1 }]);
        assert_eq!(h.complete("YO").as_deref(), Some("youtube.com"));
        assert_eq!(h.complete("yo u"), None);
        h.clear();
        assert_eq!(h.complete("yo"), None);
    }
}
