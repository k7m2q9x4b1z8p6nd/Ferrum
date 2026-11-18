use crate::bookmarks_io::Entry;
use crate::icons;
use crate::settings::dir;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub folder: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
    pub folders: Vec<String>,
    #[serde(default)]
    pub seeded: bool,
    #[serde(skip)]
    urls: HashSet<String>,
    #[serde(skip)]
    folder_set: HashSet<String>,
    #[serde(skip)]
    last_id: u32,
    #[serde(skip)]
    pub dirty: bool,
}

const BAR_MAX: usize = 64;
const LIST_MAX: usize = 200;

impl Bookmarks {
    pub fn load() -> Self {
        let mut bm: Self = std::fs::read(dir().join("bookmarks.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        bm.reindex();
        if !bm.seeded {
            bm.seeded = true;
            bm.dirty = true;
        }
        if bm.dirty {
            bm.save();
        }
        bm
    }

    fn reindex(&mut self) {
        self.last_id = self.items.iter().map(|b| b.id).max().unwrap_or(0);
        self.urls = self.items.iter().map(|b| b.url.clone()).collect();
        self.folder_set = HashSet::with_capacity(self.folders.len());
        let old = std::mem::take(&mut self.folders);
        let items = std::mem::take(&mut self.items);
        for f in old.iter().chain(items.iter().filter_map(|b| b.folder.as_ref())) {
            self.add_folder(f);
        }
        self.dirty = self.folders.len() != old.len();
        self.items = items;
    }

    fn add_folder(&mut self, path: &str) {
        if self.folder_set.contains(path) {
            return;
        }
        for p in path.match_indices('/').map(|(i, _)| &path[..i]).chain([path]) {
            if !self.folder_set.contains(p) {
                self.folder_set.insert(p.to_owned());
                self.folders.push(p.to_owned());
            }
        }
    }

    pub fn import(&mut self, entries: Vec<Entry>) -> usize {
        let fresh: Vec<bool> = {
            let mut seen: HashSet<&str> = self.items.iter().map(|b| b.url.as_str()).collect();
            entries.iter().map(|e| seen.insert(e.url.as_str())).collect()
        };
        self.items.reserve(entries.len());
        self.urls.reserve(entries.len());
        let mut n = 0;
        for (e, _) in entries.into_iter().zip(fresh).filter(|(_, f)| *f) {
            if let Some(f) = &e.folder {
                self.add_folder(f);
            }
            self.last_id += 1;
            self.urls.insert(e.url.clone());
            self.items.push(Bookmark { id: self.last_id, title: e.title, url: e.url, folder: e.folder });
            n += 1;
        }
        self.dirty |= n > 0;
        n
    }

    pub fn save(&mut self) {
        self.dirty = false;
        let Ok(b) = serde_json::to_vec(self) else { return };
        let path = dir().join("bookmarks.json");
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn add(&mut self, title: &str, url: &str) -> u32 {
        self.last_id += 1;
        let title = if title.is_empty() { url } else { title };
        self.items.push(Bookmark { id: self.last_id, title: title.to_owned(), url: url.to_owned(), folder: None });
        self.urls.insert(url.to_owned());
        self.dirty = true;
        self.last_id
    }

    pub fn update(&mut self, id: u32, title: &str, folder: &str) {
        let (title, folder) = (title.trim(), folder.trim());
        let Some(b) = self.items.iter_mut().find(|b| b.id == id) else { return };
        if !title.is_empty() {
            b.title = title.to_owned();
        }
        b.folder = (!folder.is_empty()).then(|| folder.to_owned());
        if !folder.is_empty() {
            self.add_folder(folder);
        }
        self.dirty = true;
    }

    pub fn remove(&mut self, id: u32) {
        let Some(i) = self.items.iter().position(|b| b.id == id) else { return };
        let url = self.items.remove(i).url;
        if !self.items.iter().any(|b| b.url == url) {
            self.urls.remove(&url);
        }
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.folders.clear();
        self.urls.clear();
        self.folder_set.clear();
        self.dirty = true;
    }

    pub fn remove_folder(&mut self, path: &str) {
        let prefix = format!("{path}/");
        let inside = |f: &str| f == path || f.starts_with(&prefix);
        self.items.retain(|b| !b.folder.as_deref().is_some_and(inside));
        self.folders.retain(|f| !inside(f));
        self.reindex();
        self.dirty = true;
    }

    pub fn rename_folder(&mut self, path: &str, name: &str) {
        let name = name.replace('/', "-");
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let new = path.rsplit_once('/').map_or_else(|| name.to_owned(), |(p, _)| format!("{p}/{name}"));
        if new == path || self.folder_set.contains(&new) || !self.folder_set.contains(path) {
            return;
        }
        let prefix = format!("{path}/");
        let swap = |f: &str| if f == path { Some(new.clone()) } else { f.strip_prefix(&prefix).map(|r| format!("{new}/{r}")) };
        for f in &mut self.folders {
            if let Some(n) = swap(f) {
                *f = n;
            }
        }
        for b in &mut self.items {
            if let Some(n) = b.folder.as_deref().and_then(swap) {
                b.folder = Some(n);
            }
        }
        self.reindex();
        self.dirty = true;
    }

    pub fn new_folder(&mut self, parent: &str, name: &str) {
        let name = name.replace('/', "-");
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.add_folder(&if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") });
        self.dirty = true;
    }

    pub fn starred(&self, url: &str) -> bool {
        self.urls.contains(url)
    }

    pub fn bar(&self) -> Value {
        json!({
            "items": self.items.iter().filter(|b| b.folder.is_none()).take(BAR_MAX).map(|b| json!({"id": b.id, "title": b.title, "h": icons::host_of(&b.url)})).collect::<Vec<_>>(),
            "folders": self.folders.iter().filter(|f| !f.contains('/')).take(BAR_MAX).collect::<Vec<_>>(),
        })
    }

    pub fn children(&self, path: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let prefix = format!("{path}/");
        out.extend(self.folders.iter().filter_map(|f| {
            let name = f.strip_prefix(&prefix).filter(|n| !n.contains('/'))?;
            Some(json!({"title": name, "path": f, "sub": true}))
        }));
        let mut mine = self.items.iter().filter(|b| b.folder.as_deref() == Some(path));
        out.extend(mine.by_ref().take(LIST_MAX).map(|b| json!({"id": b.id, "title": b.title, "h": icons::host_of(&b.url)})));
        let more = mine.count();
        if more > 0 {
            out.push(json!({"id": "bookmarks", "title": format!("{more} more in Bookmarks..."), "more": true}));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks_io::{export, parse, FORMATS};
    use std::time::Instant;

    fn synth(n: usize) -> Bookmarks {
        let items = (0..n)
            .map(|i| Bookmark {
                id: i as u32 + 1,
                title: format!("Site {i} & co"),
                url: format!("https://site{i}.example.com/path?q={i}&r=1"),
                folder: (i % 10 != 0).then(|| format!("Top{}/Mid{}/Sub{}/Leaf{}", i % 20, i / 20 % 5, i / 100 % 4, i / 400 % 3)),
            })
            .collect();
        Bookmarks { items, ..Default::default() }
    }

    fn entry(url: &str, folder: Option<&str>) -> Entry {
        Entry { title: url.into(), url: url.into(), folder: folder.map(Into::into) }
    }

    #[test]
    fn deep_import_creates_parents() {
        let mut bm = Bookmarks::default();
        bm.import(vec![entry("http://a", Some("A/B/C")), entry("http://b", Some("A/X")), entry("http://a", Some("A/B/C"))]);
        assert_eq!(bm.items.len(), 2);
        assert_eq!(bm.folders, ["A", "A/B", "A/B/C", "A/X"]);
    }

    #[test]
    fn reindex_repairs_missing_parents() {
        let mut bm = Bookmarks { folders: vec!["A/B/C".into()], items: vec![Bookmark { id: 7, title: "t".into(), url: "u".into(), folder: Some("Z/Y".into()) }], ..Default::default() };
        bm.reindex();
        assert_eq!(bm.folders, ["A", "A/B", "A/B/C", "Z", "Z/Y"]);
        assert!(bm.dirty && bm.starred("u") && bm.add("n", "v") == 8);
    }

    #[test]
    fn children_lists_direct_subfolders_and_items() {
        let mut bm = Bookmarks::default();
        bm.import(vec![entry("http://1", Some("A")), entry("http://2", Some("A/B")), entry("http://3", Some("A/B/C")), entry("http://4", None)]);
        let titles = |p| bm.children(p).iter().map(|v| v["title"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(titles("A"), ["B", "http://1"]);
        assert_eq!(titles("A/B"), ["C", "http://2"]);
        let bar = bm.bar();
        assert_eq!(bar["folders"], json!(["A"]));
        assert_eq!(bar["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn children_resolves_deep_paths_with_odd_names() {
        let mut bm = Bookmarks::default();
        let deep = "The Rabbit Hole/video/Streaming Sites/Free w- Ads";
        bm.import(vec![entry("http://1", Some(deep)), entry("http://2", Some("The Rabbit Hole/video/Streaming Sites"))]);
        let kids = bm.children("The Rabbit Hole/video/Streaming Sites");
        assert_eq!(kids[0]["title"], "Free w- Ads");
        assert_eq!(kids[0]["path"], deep);
        assert!(bm.folders.contains(&deep.to_owned()));
        assert_eq!(bm.children(deep).len(), 1);
        assert!(bm.children("The Rabbit Hole/vid").is_empty());
    }

    #[test]
    fn import_skips_duplicate_urls() {
        let mut bm = Bookmarks::default();
        assert_eq!(bm.import(vec![entry("http://a", None), entry("http://a", Some("F")), entry("http://b", None)]), 2);
        assert_eq!(bm.import(vec![entry("http://a", Some("G"))]), 0);
        assert_eq!(bm.items.len(), 2);
    }

    #[test]
    fn remove_folder_deletes_subtree() {
        let mut bm = Bookmarks::default();
        bm.import(vec![entry("http://1", Some("A")), entry("http://2", Some("A/B")), entry("http://3", Some("AB")), entry("http://4", None)]);
        bm.remove_folder("A");
        assert_eq!(bm.folders, ["AB"]);
        assert_eq!(bm.items.len(), 2);
        assert!(!bm.starred("http://2") && bm.starred("http://3"));
        bm.clear();
        assert!(bm.items.is_empty() && bm.folders.is_empty() && !bm.starred("http://4"));
    }

    #[test]
    fn new_folder_and_move() {
        let mut bm = Bookmarks::default();
        let id = bm.add("t", "http://a");
        bm.new_folder("", "X");
        bm.new_folder("X", " Y/Z ");
        bm.new_folder("X", "  ");
        assert_eq!(bm.folders, ["X", "X/Y-Z"]);
        bm.update(id, "", "X/Y-Z");
        assert_eq!(bm.items[0].folder.as_deref(), Some("X/Y-Z"));
        assert_eq!(bm.items[0].title, "t");
    }

    #[test]
    fn starred_tracks_duplicate_urls() {
        let mut bm = Bookmarks::default();
        bm.add("a", "http://a");
        bm.add("a", "http://a");
        bm.remove(1);
        assert!(bm.starred("http://a"));
        bm.remove(2);
        assert!(!bm.starred("http://a"));
    }

    #[test]
    fn large_import_perf() {
        let src = synth(20_000);
        for fmt in FORMATS {
            let data = export(&src, fmt);
            let t = Instant::now();
            let list = parse(fmt, data.as_bytes()).unwrap();
            let parse_ms = t.elapsed().as_millis();
            let again = list.clone();
            let mut bm = Bookmarks::default();
            let t = Instant::now();
            let n = bm.import(list);
            let import_ms = t.elapsed().as_millis();
            eprintln!("{fmt}: parse {parse_ms} ms, import {import_ms} ms, {} KB", data.len() / 1024);
            assert_eq!(n, 20_000);
            assert_eq!(bm.import(again), 0);
            assert!(parse_ms + import_ms < 5000);
        }
    }

    #[test]
    fn rename_folder_moves_subfolders_and_items() {
        let mut bm = Bookmarks::default();
        bm.new_folder("", "A");
        bm.new_folder("A", "B");
        bm.new_folder("", "Z");
        let id = bm.add("t", "http://1");
        bm.update(id, "", "A/B");
        bm.rename_folder("A", "X/Y");
        assert_eq!(bm.folders, ["X-Y", "X-Y/B", "Z"]);
        assert_eq!(bm.items[0].folder.as_deref(), Some("X-Y/B"));
        bm.rename_folder("X-Y", "Z");
        assert_eq!(bm.folders, ["X-Y", "X-Y/B", "Z"]);
        bm.rename_folder("X-Y/B", "  ");
        assert_eq!(bm.folders, ["X-Y", "X-Y/B", "Z"]);
        bm.rename_folder("X-Y/B", "C");
        assert_eq!(bm.items[0].folder.as_deref(), Some("X-Y/C"));
    }
}
