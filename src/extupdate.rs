use crate::{
    extensions,
    extinstall::{self, Done, Source, PRODVERSION, UA},
    settings,
};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const DAY: u64 = 86_400;
const UBLOCK: &str = "gorhill/uBlock";
const UBLOCK_DIR: &str = "ublock";

pub struct Target {
    pub id: String,
    pub src: Source,
    pub current: String,
}

pub fn endpoint(source: &str, folder: &str) -> Option<Source> {
    match source {
        "chrome" if extinstall::is_chrome_id(folder) => Some(Source::Chrome(folder.to_owned())),
        "chrome" if folder == UBLOCK_DIR => Some(Source::Github(UBLOCK.to_owned())),
        "firefox" if !folder.is_empty() => Some(Source::Amo(folder.to_owned())),
        _ => None,
    }
}

pub fn targets() -> Vec<Target> {
    extensions::targets().into_iter().filter_map(|(id, source, folder, current)| Some(Target { src: endpoint(source, &folder)?, id, current })).collect()
}

fn parts(v: &str) -> Option<Vec<u64>> {
    let mut out = vec![];
    for p in v.trim().trim_start_matches(['v', 'V']).split('.') {
        let digits = p.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(n) = p[..digits].parse() else { break };
        out.push(n);
        if digits < p.len() {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}

pub fn newer(latest: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parts(latest), parts(current)) else { return false };
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    ['"', '\''].iter().find_map(|q| {
        let pat = format!(" {name}={q}");
        let s = tag.find(&pat)? + pat.len();
        tag[s..].split_once(*q).map(|x| x.0)
    })
}

pub fn parse_update_xml(xml: &str) -> Result<Option<String>, String> {
    let s = xml.find("<updatecheck").ok_or("Unexpected response")?;
    let tag = &xml[s..s + xml[s..].find('>').ok_or("Unexpected response")?];
    match attr(tag, "status") {
        Some("ok") => attr(tag, "version").map(|v| Some(v.to_owned())).ok_or_else(|| "Unexpected response".into()),
        Some("noupdate") => Ok(None),
        Some(e) => Err(format!("Not available in the Chrome Web Store ({e})")),
        None => Err("Unexpected response".into()),
    }
}

fn amo_version(v: &Value) -> Option<String> {
    v["current_version"]["version"].as_str().map(str::to_owned)
}

fn tag_version(v: &Value) -> Option<String> {
    v["tag_name"].as_str().map(|t| t.trim_start_matches(['v', 'V']).to_owned())
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    extinstall::agent().get(url).header("User-Agent", UA).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "Not found".to_owned(),
        e => format!("Check failed: {e}"),
    })
}

fn json(url: &str) -> Result<Value, String> {
    get(url)?.body_mut().with_config().limit(4 << 20).read_json().map_err(|e| format!("Check failed: {e}"))
}

pub fn check(src: &Source, current: &str) -> Result<Option<String>, String> {
    let latest = match src {
        Source::Chrome(id) => {
            let url = format!("https://clients2.google.com/service/update2/crx?response=updatecheck&prodversion={PRODVERSION}&acceptformat=crx2,crx3&x=id%3D{id}%26v%3D{current}%26uc");
            let xml = get(&url)?.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("Check failed: {e}"))?;
            parse_update_xml(&xml)?
        }
        Source::Amo(s) => amo_version(&json(&format!("https://addons.mozilla.org/api/v5/addons/addon/{}/", s.replace('{', "%7B").replace('}', "%7D").replace('@', "%40")))?),
        Source::Github(r) => tag_version(&json(&format!("https://api.github.com/repos/{r}/releases/latest"))?),
        _ => None,
    };
    Ok(latest.filter(|v| newer(v, current)))
}

pub fn stage(src: Source) -> Result<Done, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    extinstall::install(src, &settings::dir().join(format!("ext_upd_{nanos}")), &|_| {})
}

pub fn unstage(d: &Done) {
    if let Some(p) = d.folder.parent() {
        let _ = std::fs::remove_dir_all(p);
    }
}

pub fn sweep() {
    for e in std::fs::read_dir(settings::dir()).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("ext_upd_") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn stamp() -> PathBuf {
    settings::dir().join("ext_update.json")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn due_at(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|l| now.saturating_sub(l) >= DAY)
}

pub fn due() -> bool {
    due_at(std::fs::read(stamp()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["last"].as_u64()), now())
}

pub fn mark() {
    let _ = std::fs::write(stamp(), serde_json::json!({"last": now()}).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("1.10.0", "1.9.0"));
        assert!(!newer("1.9.0", "1.10.0"));
        assert!(newer("v2.0", "1.99.99"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(newer("1.0.0.1", "1.0.0"));
        assert!(!newer("2.0.0", "2.0.0"));
    }

    #[test]
    fn versions_tolerate_suffixes_and_junk() {
        assert!(!newer("1.2.0b3", "1.2.0"));
        assert!(newer("1.2.1b3", "1.2.0"));
        assert!(newer("1.3", "1.2.0b3"));
        assert!(newer("1.2.0-beta", "1.1.9"));
        assert!(!newer("", "1.0"));
        assert!(!newer("abc", "1.0"));
        assert!(!newer("2.0", "garbage"));
    }

    #[test]
    fn xml_ok_and_noupdate() {
        let ok = r#"<?xml version='1.0' encoding='UTF-8'?><gupdate xmlns="http://www.google.com/update2/response" protocol="2.0"><app appid="abc" status="ok"><updatecheck codebase="https://x/y.crx" hash_sha256="aa" size="12" status="ok" version="1.4.2"/></app></gupdate>"#;
        assert_eq!(parse_update_xml(ok), Ok(Some("1.4.2".into())));
        let none = r#"<gupdate><app appid="abc" status="ok"><updatecheck status="noupdate"/></app></gupdate>"#;
        assert_eq!(parse_update_xml(none), Ok(None));
        let single = "<gupdate><app><updatecheck status='ok' version='3.0'/></app></gupdate>";
        assert_eq!(parse_update_xml(single), Ok(Some("3.0".into())));
    }

    #[test]
    fn xml_errors() {
        assert!(parse_update_xml(r#"<gupdate><app status="error-unknownApplication"/></gupdate>"#).is_err());
        assert!(parse_update_xml(r#"<gupdate><app><updatecheck status="error-unknownApplication"/></app></gupdate>"#).unwrap_err().contains("error-unknownApplication"));
        assert!(parse_update_xml(r#"<updatecheck status="ok"/>"#).is_err());
        assert!(parse_update_xml("").is_err());
        assert!(parse_update_xml("<updatecheck status=\"ok\"").is_err());
    }

    #[test]
    fn json_versions() {
        assert_eq!(amo_version(&serde_json::json!({"current_version": {"version": "1.5"}})), Some("1.5".into()));
        assert_eq!(amo_version(&serde_json::json!({})), None);
        assert_eq!(tag_version(&serde_json::json!({"tag_name": "v1.62.0"})), Some("1.62.0".into()));
        assert_eq!(tag_version(&serde_json::json!({"tag_name": "1.62.0"})), Some("1.62.0".into()));
    }

    #[test]
    fn endpoint_selection() {
        let id = "ddkjiahejlhfcafbddmgiahcphecmpfh";
        assert_eq!(endpoint("chrome", id), Some(Source::Chrome(id.into())));
        assert_eq!(endpoint("chrome", "ublock"), Some(Source::Github(UBLOCK.into())));
        assert_eq!(endpoint("firefox", "ublock-origin"), Some(Source::Amo("ublock-origin".into())));
        assert_eq!(endpoint("chrome", "some-local-thing"), None);
        assert_eq!(endpoint("local", id), None);
        assert_eq!(endpoint("firefox", ""), None);
    }

    #[test]
    fn due_after_a_day() {
        assert!(due_at(None, 5));
        assert!(!due_at(Some(1000), 1000 + DAY - 1));
        assert!(due_at(Some(1000), 1000 + DAY));
        assert!(!due_at(Some(5000), 100));
    }
}
