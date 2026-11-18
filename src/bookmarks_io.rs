use crate::bookmarks::Bookmarks;
use serde_json::Value;
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub title: String,
    pub url: String,
    pub folder: Option<String>,
}

pub const FORMATS: [&str; 3] = ["json", "html", "csv"];

fn folder_path(path: &str) -> Option<String> {
    let mut out = String::with_capacity(path.len());
    for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

fn clean(name: &str) -> String {
    name.replace('/', "-").trim().to_owned()
}

fn push(out: &mut Vec<Entry>, title: &str, url: &str, path: &str) {
    let url = url.trim();
    if url.is_empty() {
        return;
    }
    let title = if title.trim().is_empty() { url } else { title.trim() };
    out.push(Entry { title: title.to_owned(), url: url.to_owned(), folder: folder_path(path) });
}

pub fn parse(ext: &str, data: &[u8]) -> Result<Vec<Entry>, String> {
    let text = String::from_utf8_lossy(data);
    let text = text.trim_start_matches('\u{feff}');
    match ext.to_ascii_lowercase().as_str() {
        "json" => parse_json(text),
        "html" | "htm" => Ok(parse_html(text)),
        "csv" => Ok(parse_csv(text)),
        _ => Err("Unsupported file type".into()),
    }
}

pub fn export(bm: &Bookmarks, fmt: &str) -> String {
    match fmt {
        "html" => to_html(bm),
        "csv" => to_csv(bm),
        _ => serde_json::to_string_pretty(bm).unwrap_or_default(),
    }
}

fn parse_json(text: &str) -> Result<Vec<Entry>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(items) = v.get("items").and_then(Value::as_array) {
        for b in items {
            let f = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("");
            push(&mut out, f("title"), f("url"), f("folder"));
        }
    } else if let Some(roots) = v.get("roots").and_then(Value::as_object) {
        for (key, node) in roots {
            let path = match (key.as_str(), node.get("name").and_then(Value::as_str)) {
                ("bookmark_bar", _) => String::new(),
                (_, Some(n)) => clean(n),
                _ => String::new(),
            };
            walk(node, &path, &mut out, true);
        }
    } else {
        walk(&v, "", &mut out, true);
    }
    Ok(out)
}

fn walk(node: &Value, path: &str, out: &mut Vec<Entry>, top: bool) {
    if let Some(arr) = node.as_array() {
        for n in arr {
            walk(n, path, out, false);
        }
        return;
    }
    let s = |k: &str| node.get(k).and_then(Value::as_str);
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        let mut p = path.to_owned();
        if !top && !matches!(s("root"), Some("toolbarFolder" | "placesRoot")) {
            p.push('/');
            p.push_str(&clean(s("title").or(s("name")).unwrap_or("")));
        }
        for c in children {
            walk(c, &p, out, false);
        }
    } else if let Some(url) = s("url").or(s("uri")) {
        if s("type") != Some("folder") && s("type") != Some("text/x-moz-place-container") {
            push(out, s("title").or(s("name")).unwrap_or(""), url, path);
        }
    }
}

fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let dec = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let ent = &rest[1..e];
            let c = match ent {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let n = ent.strip_prefix('#')?;
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, e + 1))
        });
        match dec {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn attr<'a>(tag: &'a str, lower: &str, name: &str) -> Option<Cow<'a, str>> {
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=').filter(|_| before_ok) else { continue };
        let rest = rest.trim_start();
        let v = match rest.chars().next()? {
            q @ ('"' | '\'') => rest[1..].split(q).next()?,
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?,
        };
        return Some(unescape(v));
    }
    None
}

fn parse_html(text: &str) -> Vec<Entry> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::with_capacity(text.len() / 100);
    let mut path = String::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut pending: Option<Option<String>> = None;
    let mut pos = 0;
    while let Some(i) = lower[pos..].find('<') {
        let start = pos + i;
        let Some(len) = lower[start..].find('>') else { break };
        let end = start + len;
        let tag = &text[start + 1..end];
        let ltag = &lower[start + 1..end];
        let name = ltag.split(|c: char| c.is_whitespace() || c == '/').find(|s| !s.is_empty()).unwrap_or("");
        let closing = lower[start + 1..].starts_with('/');
        pos = end + 1;
        let inner = |pos: &mut usize, close: &str| {
            let stop = lower[*pos..].find(close).map_or(text.len(), |k| *pos + k);
            let t = &text[*pos..stop];
            *pos = stop;
            unescape(t.trim())
        };
        match (name, closing) {
            ("h3", false) => {
                let toolbar = attr(tag, ltag, "personal_toolbar_folder").is_some_and(|v| v.eq_ignore_ascii_case("true"));
                let t = inner(&mut pos, "</h3");
                pending = Some((!toolbar).then(|| clean(&t)));
            }
            ("dl", false) => {
                stack.push(path.len());
                if let Some(n) = pending.take().flatten().filter(|n| !n.is_empty()) {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(&n);
                }
            }
            ("dl", true) => {
                if let Some(len) = stack.pop() {
                    path.truncate(len);
                }
            }
            ("a", false) => {
                let url = attr(tag, ltag, "href").unwrap_or_default();
                let t = inner(&mut pos, "</a");
                push(&mut out, &t, &url, &path);
            }
            _ => {}
        }
    }
    out
}

fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            ('"', true) => quoted = false,
            ('"', false) if field.is_empty() => quoted = true,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r' | '\n', false) => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                if row.iter().any(|f| !f.trim().is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            _ => field.push(c),
        }
    }
    row.push(field);
    if row.iter().any(|f| !f.trim().is_empty()) {
        rows.push(row);
    }
    rows
}

fn parse_csv(text: &str) -> Vec<Entry> {
    let rows = csv_rows(text);
    let Some(first) = rows.first() else { return vec![] };
    let heads: Vec<String> = first.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    let find = |names: &[&str]| heads.iter().position(|h| names.contains(&h.as_str()));
    let url_col = find(&["url", "uri", "href", "link", "address"]);
    let (title, url, folder, skip) = match url_col {
        Some(u) => (find(&["title", "name"]), u, find(&["folder", "path", "folders", "folder path", "parent"]), 1),
        None => (Some(0), 1, Some(2), 0),
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows[skip..] {
        let get = |c: Option<usize>| c.and_then(|c| r.get(c)).map_or("", String::as_str);
        push(&mut out, get(title), get(Some(url)), get(folder));
    }
    out
}

fn to_csv(bm: &Bookmarks) -> String {
    let q = |s: &str| {
        if s.contains(['"', ',', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
    };
    let mut s = String::from("title,url,folder\r\n");
    for b in &bm.items {
        s += &format!("{},{},{}\r\n", q(&b.title), q(&b.url), q(b.folder.as_deref().unwrap_or("")));
    }
    s
}

#[derive(Default)]
struct Node {
    name: String,
    items: Vec<(String, String)>,
    subs: Vec<Node>,
}

fn node_at<'a>(root: &'a mut Node, path: &str) -> &'a mut Node {
    let mut cur = root;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        let i = match cur.subs.iter().position(|n| n.name == part) {
            Some(i) => i,
            None => {
                cur.subs.push(Node { name: part.to_owned(), ..Default::default() });
                cur.subs.len() - 1
            }
        };
        cur = &mut cur.subs[i];
    }
    cur
}

fn write_node(n: &Node, depth: usize, s: &mut String) {
    let pad = "    ".repeat(depth);
    for (t, u) in &n.items {
        s.push_str(&format!("{pad}<DT><A HREF=\"{}\">{}</A>\n", escape(u), escape(t)));
    }
    for sub in &n.subs {
        s.push_str(&format!("{pad}<DT><H3>{}</H3>\n{pad}<DL><p>\n", escape(&sub.name)));
        write_node(sub, depth + 1, s);
        s.push_str(&format!("{pad}</DL><p>\n"));
    }
}

fn to_html(bm: &Bookmarks) -> String {
    let mut root = Node::default();
    for f in &bm.folders {
        node_at(&mut root, f);
    }
    for b in &bm.items {
        node_at(&mut root, b.folder.as_deref().unwrap_or("")).items.push((b.title.clone(), b.url.clone()));
    }
    let mut s = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n");
    write_node(&root, 1, &mut s);
    s.push_str("</DL><p>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::Bookmark;

    fn sample() -> Bookmarks {
        let b = |id, t: &str, u: &str, f: Option<&str>| Bookmark { id, title: t.into(), url: u.into(), folder: f.map(Into::into) };
        let mut bm = Bookmarks::default();
        bm.items = vec![
            b(1, "Tom & \"Jerry\" <x>", "https://a.com/?q=1&r=2", None),
            b(2, "Comma, title", "https://b.com", Some("Work")),
            b(3, "Deep", "https://c.com", Some("Work/Sub")),
        ];
        bm.folders = vec!["Work".into(), "Work/Sub".into()];
        bm
    }

    fn entries(bm: &Bookmarks) -> Vec<Entry> {
        bm.items.iter().map(|b| Entry { title: b.title.clone(), url: b.url.clone(), folder: b.folder.clone() }).collect()
    }

    #[test]
    fn json_roundtrip() {
        let bm = sample();
        assert_eq!(parse("json", export(&bm, "json").as_bytes()).unwrap(), entries(&bm));
    }

    #[test]
    fn html_roundtrip() {
        let bm = sample();
        let mut got = parse("html", export(&bm, "html").as_bytes()).unwrap();
        let mut want = entries(&bm);
        got.sort_by(|a, b| a.url.cmp(&b.url));
        want.sort_by(|a, b| a.url.cmp(&b.url));
        assert_eq!(got, want);
    }

    #[test]
    fn csv_roundtrip() {
        let bm = sample();
        assert_eq!(parse("csv", export(&bm, "csv").as_bytes()).unwrap(), entries(&bm));
    }

    #[test]
    fn csv_headers_case_and_quotes() {
        let r = parse("csv", b"Name,Folder,URL\r\n\"A, \"\"B\"\"\",F/G,http://x.com\r\n").unwrap();
        assert_eq!(r, vec![Entry { title: "A, \"B\"".into(), url: "http://x.com".into(), folder: Some("F/G".into()) }]);
    }

    #[test]
    fn html_chrome_style() {
        let h = "<DL><p><DT><H3 PERSONAL_TOOLBAR_FOLDER=\"true\">Bar</H3><DL><p><DT><A HREF=\"http://a.com\" ADD_DATE=\"1\">A &amp; B</A><DT><H3>Sub</H3><DL><p><DT><A HREF=\"http://s.com\">S&#39;s</A></DL><p></DL><p><DT><A HREF=\"http://t.com\">T</A></DL>";
        let r = parse("html", h.as_bytes()).unwrap();
        assert_eq!(r[0], Entry { title: "A & B".into(), url: "http://a.com".into(), folder: None });
        assert_eq!(r[1], Entry { title: "S's".into(), url: "http://s.com".into(), folder: Some("Sub".into()) });
        assert_eq!(r[2].folder, None);
    }

    #[test]
    fn chrome_json() {
        let j = r#"{"roots":{"bookmark_bar":{"name":"Bookmarks bar","type":"folder","children":[{"type":"url","name":"A","url":"http://a.com"},{"type":"folder","name":"F","children":[{"type":"url","name":"B","url":"http://b.com"}]}]},"other":{"name":"Other bookmarks","type":"folder","children":[{"type":"url","name":"C","url":"http://c.com"}]}}}"#;
        let r = parse("json", j.as_bytes()).unwrap();
        let f: Vec<_> = r.iter().map(|e| e.folder.as_deref()).collect();
        assert_eq!(f, [None, Some("F"), Some("Other bookmarks")]);
    }

    #[test]
    fn firefox_json() {
        let j = r#"{"root":"placesRoot","children":[{"root":"toolbarFolder","title":"toolbar","typeCode":2,"children":[{"typeCode":1,"title":"A","uri":"http://a.com"},{"typeCode":2,"title":"F","children":[{"typeCode":1,"title":"B","uri":"http://b.com"}]}]},{"root":"unfiledBookmarksFolder","title":"Other","typeCode":2,"children":[{"typeCode":1,"title":"C","uri":"http://c.com"}]}]}"#;
        let r = parse("json", j.as_bytes()).unwrap();
        let f: Vec<_> = r.iter().map(|e| e.folder.as_deref()).collect();
        assert_eq!(f, [None, Some("F"), Some("Other")]);
    }
}
