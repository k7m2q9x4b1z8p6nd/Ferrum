use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "https://api.github.com/repos/k7m2q9x4b1z8p6nd/Ferrum/releases/latest";
const UA: &str = concat!("Ferrum/", env!("CARGO_PKG_VERSION"));
const HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
const MAX_BYTES: u64 = 150 * 1024 * 1024;
const HOPS: usize = 6;
const BODY_CAP: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub date: String,
    pub body: String,
    pub url: String,
    pub exe: Option<String>,
    pub sha: Option<String>,
}

impl Release {
    pub fn view(&self) -> Value {
        json!({"v": self.version, "cur": VERSION, "name": self.name, "date": self.date, "html": markdown(&self.body), "url": self.url, "can": self.exe.is_some() && self.sha.is_some()})
    }
}

fn parse(v: &str) -> Option<Vec<u64>> {
    v.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect()
}

pub fn newer(latest: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse(latest), parse(current)) else { return false };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn dev() -> bool {
    std::env::var_os("FERRUM_UPDATE_URL").is_some()
}

pub fn parse_release(v: &Value) -> Result<Option<Release>, String> {
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = v["tag_name"].as_str().ok_or("Unexpected response")?;
    let asset = |name: &str| v["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))?["browser_download_url"].as_str().map(str::to_owned);
    Ok(Some(Release {
        version: tag.trim().trim_start_matches(['v', 'V']).to_owned(),
        name: v["name"].as_str().unwrap_or("").to_owned(),
        date: v["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
        body: v["body"].as_str().unwrap_or("").to_owned(),
        url: v["html_url"].as_str().unwrap_or("").to_owned(),
        exe: asset("ferrum.exe"),
        sha: asset("ferrum.exe.sha256"),
    }))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into()
}

pub fn host_ok(url: &str, dev: bool) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.contains('@') {
        return false;
    }
    let host = auth.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(auth, |(h, _)| h).to_ascii_lowercase();
    match scheme {
        "https" => HOSTS.contains(&host.as_str()) || dev && host == "localhost",
        "http" => dev && (host == "127.0.0.1" || host == "localhost"),
        _ => false,
    }
}

fn open(agent: &ureq::Agent, url: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut url = url.to_owned();
    for _ in 0..HOPS {
        if !host_ok(&url, dev()) {
            return Err("Download host is not allowed".into());
        }
        let r = agent.get(&url).header("User-Agent", UA).header("Accept", accept).call().map_err(|e| e.to_string())?;
        if r.status().is_redirection() {
            url = r.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("Bad redirect")?.to_owned();
            continue;
        }
        if !r.status().is_success() {
            return Err(format!("Server returned {}", r.status().as_u16()));
        }
        return Ok(r);
    }
    Err("Too many redirects".into())
}

pub fn check() -> Result<Option<Release>, String> {
    let url = std::env::var("FERRUM_UPDATE_URL").unwrap_or_else(|_| LATEST.to_owned());
    let mut r = match open(&agent(), &url, "application/vnd.github+json") {
        Ok(r) => r,
        Err(e) if e == "Server returned 404" => return Ok(None),
        Err(e) => return Err(e),
    };
    let v = r.body_mut().read_json::<Value>().map_err(|e| e.to_string())?;
    Ok(parse_release(&v)?.filter(|r| newer(&r.version, VERSION)))
}

pub fn notify_once(version: &str) -> bool {
    let f = crate::settings::dir().join("update_notified");
    let seen = std::fs::read_to_string(&f).is_ok_and(|s| s.trim() == version);
    if !seen {
        let _ = std::fs::write(&f, version);
    }
    !seen
}

pub fn parse_sha(text: &str, file: &str) -> Option<String> {
    let hash = |t: &str| (t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())).then(|| t.to_ascii_lowercase());
    text.lines().filter_map(|l| {
        let mut it = l.split_whitespace();
        let h = hash(it.next()?)?;
        match it.next().map(|n| n.trim_start_matches('*').rsplit(['/', '\\']).next().unwrap_or("")) {
            Some(n) if !n.eq_ignore_ascii_case(file) => None,
            _ => Some(h),
        }
    }).next()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn verify(got: &[u8], want: &str) -> Result<(), String> {
    if hex(got) == want.to_ascii_lowercase() { Ok(()) } else { Err("Checksum mismatch, update refused".into()) }
}

pub fn staging() -> PathBuf {
    std::env::temp_dir().join("ferrum-update")
}

pub fn cleanup() {
    let _ = std::fs::remove_dir_all(staging());
}

pub fn download(rel: &Release, dest: &Path, cancel: &AtomicBool, on: &dyn Fn(u64, u64)) -> Result<(), String> {
    let (Some(exe), Some(sha)) = (&rel.exe, &rel.sha) else { return Err("This release has no ferrum.exe and ferrum.exe.sha256 files".into()) };
    let agent = agent();
    let mut text = String::new();
    open(&agent, sha, "*/*")?.body_mut().as_reader().take(4096).read_to_string(&mut text).map_err(|e| e.to_string())?;
    let want = parse_sha(&text, "ferrum.exe").ok_or("The checksum file is not valid")?;
    let mut resp = open(&agent, exe, "application/octet-stream")?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0u64);
    if total > MAX_BYTES {
        return Err("The update file is too large".into());
    }
    let dir = dest.parent().ok_or("Bad destination")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dest.with_extension("part");
    let r = (|| {
        let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let (mut h, mut got, mut buf, mut head) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024], [0u8; 2]);
        let mut rd = resp.body_mut().as_reader();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_owned());
            }
            let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if got == 0 {
                head.copy_from_slice(&buf[..2.min(n)]);
            }
            got += n as u64;
            if got > MAX_BYTES {
                return Err("The update file is too large".into());
            }
            h.update(&buf[..n]);
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            on(got, total.max(got));
        }
        f.sync_all().map_err(|e| e.to_string())?;
        if &head != b"MZ" {
            return Err("The downloaded file is not an executable".into());
        }
        verify(&h.finalize(), &want)
    })();
    match r {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn esc(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(c),
    }
}

fn inline(s: &str) -> String {
    let (mut out, mut rest) = (String::new(), s);
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix('`').filter(|r| r.contains('`')) {
            let i = r.find('`').unwrap_or(0);
            out.push_str("<code>");
            r[..i].chars().for_each(|c| esc(c, &mut out));
            out.push_str("</code>");
            rest = &r[i + 1..];
        } else if let Some(i) = rest.strip_prefix("**").and_then(|r| r.find("**").filter(|i| *i > 0)) {
            out.push_str(&format!("<b>{}</b>", inline(&rest[2..2 + i])));
            rest = &rest[i + 4..];
        } else if let Some((text, after)) = link(rest.strip_prefix('!').filter(|r| r.starts_with('[')).unwrap_or(rest)) {
            out.push_str(&inline(text));
            rest = after;
        } else {
            esc(c, &mut out);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn link(s: &str) -> Option<(&str, &str)> {
    let r = s.strip_prefix('[')?;
    let i = r.find("](")?;
    let mut depth = 1;
    let j = r[i + 2..].find(|c| {
        depth += (c == '(') as i32 - (c == ')') as i32;
        depth == 0
    })?;
    Some((&r[..i], &r[i + 3 + j..]))
}

pub fn markdown(src: &str) -> String {
    let (mut out, mut list, mut para) = (String::new(), false, false);
    for line in src.chars().take(BODY_CAP).collect::<String>().lines() {
        let t = line.trim();
        let bullet = ["- ", "* ", "+ "].iter().find_map(|p| t.strip_prefix(p)).or_else(|| t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).map(|(_, r)| r));
        if t.is_empty() || t.starts_with('#') || bullet.is_none() {
            if list {
                out.push_str("</ul>");
                list = false;
            }
        }
        if t.is_empty() {
            para = false;
        } else if t.starts_with('#') {
            out.push_str(&format!("<h4>{}</h4>", inline(t.trim_start_matches('#').trim())));
            para = false;
        } else if let Some(b) = bullet {
            if !list {
                out.push_str("<ul>");
                list = true;
            }
            out.push_str(&format!("<li>{}</li>", inline(b.trim())));
            para = false;
        } else {
            out.push_str(&format!("{}{}", if para { "<br>" } else { "<p>" }, inline(t)));
            para = true;
        }
    }
    if list {
        out.push_str("</ul>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_and_minor() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(!newer("1.0.0", "1.0.1"));
        assert!(newer("1.10.0", "1.9.0"));
        assert!(!newer("1.9.0", "1.10.0"));
        assert!(newer("2.0.0", "1.99.99"));
    }

    #[test]
    fn prefix_and_equal() {
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(newer("V1.1", "v1.0.9"));
        assert!(!newer("v1.0.0", "1.0.0"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(newer("1.0.0.1", "1.0.0"));
    }

    #[test]
    fn malformed() {
        assert!(!newer("", "1.0.0"));
        assert!(!newer("abc", "1.0.0"));
        assert!(!newer("1.x.0", "1.0.0"));
        assert!(!newer("2.0.0-beta", "1.0.0"));
        assert!(!newer("2.0.0", "garbage"));
        assert!(!newer("1..1", "1.0.0"));
    }

    #[test]
    fn release_full() {
        let v = json!({"tag_name":"v1.2.0","name":"Ferrum 1.2","published_at":"2026-11-18T10:00:00Z","body":"- a","html_url":"https://github.com/x/y","assets":[{"name":"ferrum.exe.sha256","browser_download_url":"https://github.com/s"},{"name":"Ferrum.exe","browser_download_url":"https://github.com/e"}]});
        let r = parse_release(&v).unwrap().unwrap();
        assert_eq!((r.version.as_str(), r.date.as_str(), r.name.as_str()), ("1.2.0", "2026-11-18", "Ferrum 1.2"));
        assert_eq!((r.exe.as_deref(), r.sha.as_deref()), (Some("https://github.com/e"), Some("https://github.com/s")));
        assert_eq!(r.view()["can"], true);
    }

    #[test]
    fn release_minimal_and_ignored() {
        let r = parse_release(&json!({"tag_name":"1.0.1"})).unwrap().unwrap();
        assert!(r.body.is_empty() && r.exe.is_none() && r.name.is_empty());
        assert_eq!(r.view()["can"], false);
        assert!(parse_release(&json!({"tag_name":"2.0.0","draft":true})).unwrap().is_none());
        assert!(parse_release(&json!({"tag_name":"2.0.0","prerelease":true})).unwrap().is_none());
        assert!(parse_release(&json!({"name":"x"})).is_err());
    }

    #[test]
    fn markdown_is_safe_and_basic() {
        let h = markdown("## New <script>alert(1)</script>\n- **fast** `a<b`\n- [site](javascript:alert(1)) end\n\nplain \"q\"\nnext");
        assert!(!h.contains("<script"));
        assert!(h.contains("<h4>New &lt;script&gt;"));
        assert!(h.contains("<ul><li><b>fast</b> <code>a&lt;b</code></li><li>site end</li></ul>"));
        assert!(!h.contains("javascript"));
        assert!(h.contains("<p>plain &quot;q&quot;<br>next"));
        assert_eq!(markdown(""), "");
        assert_eq!(markdown("<img src=x onerror=1>"), "<p>&lt;img src=x onerror=1&gt;");
        assert_eq!(markdown("**unclosed `tick"), "<p>**unclosed `tick");
    }

    #[test]
    fn host_allowlist() {
        assert!(host_ok("https://github.com/a/b", false));
        assert!(host_ok("https://objects.githubusercontent.com/x?y=1", false));
        assert!(host_ok("https://release-assets.githubusercontent.com/x", false));
        assert!(!host_ok("http://github.com/a", false));
        assert!(!host_ok("https://evil.com/github.com", false));
        assert!(!host_ok("https://github.com.evil.com/a", false));
        assert!(!host_ok("https://github.com@evil.com/a", false));
        assert!(!host_ok("https://evilgithub.com/a", false));
        assert!(!host_ok("ftp://github.com/a", false));
        assert!(!host_ok("http://127.0.0.1:9/a", false));
        assert!(host_ok("http://127.0.0.1:9/a", true));
        assert!(!host_ok("http://evil.com/a", true));
    }

    #[test]
    fn sha_file_parsing() {
        let h = "A".repeat(64);
        let l = "a".repeat(64);
        assert_eq!(parse_sha(&format!("{h}  ferrum.exe\n"), "ferrum.exe"), Some(l.clone()));
        assert_eq!(parse_sha(&format!("{h} *dist\\Ferrum.exe"), "ferrum.exe"), Some(l.clone()));
        assert_eq!(parse_sha(&h, "ferrum.exe"), Some(l.clone()));
        assert_eq!(parse_sha(&format!("{h}  other.zip\n{}  ferrum.exe", "b".repeat(64)), "ferrum.exe"), Some("b".repeat(64)));
        assert_eq!(parse_sha(&format!("{h}  other.zip"), "ferrum.exe"), None);
        assert_eq!(parse_sha("deadbeef  ferrum.exe", "ferrum.exe"), None);
        assert_eq!(parse_sha("", "ferrum.exe"), None);
        assert_eq!(parse_sha(&"g".repeat(64), "ferrum.exe"), None);
    }

    #[test]
    fn hash_verification() {
        let d = Sha256::digest(b"abc");
        let ok = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&d, ok).is_ok());
        assert!(verify(&d, &ok.to_uppercase()).is_ok());
        assert!(verify(&d, &"0".repeat(64)).is_err());
        assert!(verify(&d, "").is_err());
    }
}
