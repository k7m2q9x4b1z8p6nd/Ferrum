use crate::settings;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::HashSet,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

const MAX_BYTES: usize = 64 * 1024;
const BATCH: usize = 60;
const RETRY: Duration = Duration::from_secs(7 * 86400);
static BUSY: AtomicBool = AtomicBool::new(false);

fn file(host: &str, ext: &str) -> PathBuf {
    let d = settings::dir().join("icons");
    let _ = std::fs::create_dir_all(&d);
    d.join(format!("{host}.{ext}"))
}

pub fn valid(host: &str) -> bool {
    !host.is_empty() && host.len() <= 253 && !host.starts_with('.') && !host.contains("..") && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

pub fn host_of(url: &str) -> String {
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else { return String::new() };
    let h = rest.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("");
    let h = h.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(h, |(h, _)| h).to_lowercase();
    if valid(&h) { h } else { String::new() }
}

pub fn mime(b: &[u8]) -> Option<&'static str> {
    match b {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0, 0, 1, 0, ..] => Some("image/x-icon"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ if b.starts_with(b"<svg") || b.starts_with(b"<?xml") => Some("image/svg+xml"),
        _ => None,
    }
}

pub fn read(host: &str) -> Option<(&'static str, Vec<u8>)> {
    if !valid(host) {
        return None;
    }
    let b = std::fs::read(file(host, "img")).ok()?;
    Some((mime(&b)?, b))
}

pub fn save(host: &str, data_url: &str) -> bool {
    let Some(b64) = data_url.strip_prefix("data:image/png;base64,") else { return false };
    let Ok(bytes) = STANDARD.decode(b64) else { return false };
    if !valid(host) || bytes.is_empty() || bytes.len() > MAX_BYTES {
        return false;
    }
    let p = file(host, "img");
    if std::fs::read(&p).is_ok_and(|old| old == bytes) {
        return false;
    }
    std::fs::write(p, bytes).is_ok()
}

fn have(host: &str) -> bool {
    file(host, "img").exists() || std::fs::metadata(file(host, "none")).and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|a| a < RETRY))
}

fn fetch(agent: &ureq::Agent, host: &str) -> Option<Vec<u8>> {
    let r = agent.get(format!("https://{host}/favicon.ico")).call().ok()?;
    let mut b = Vec::new();
    r.into_body().into_reader().take(MAX_BYTES as u64 + 1).read_to_end(&mut b).ok()?;
    (b.len() <= MAX_BYTES && mime(&b).is_some()).then_some(b)
}

pub fn prefetch(hosts: Vec<String>, done: impl FnOnce() + Send + 'static) {
    if hosts.is_empty() || BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(4))).build().into();
        let mut seen = HashSet::new();
        let todo: Vec<String> = hosts.into_iter().filter(|h| valid(h) && seen.insert(h.clone()) && !have(h)).take(BATCH).collect();
        let mut got = false;
        for h in &todo {
            match fetch(&agent, h) {
                Some(b) => got |= std::fs::write(file(h, "img"), b).is_ok(),
                None => {
                    let _ = std::fs::write(file(h, "none"), b"");
                }
            }
        }
        BUSY.store(false, Ordering::SeqCst);
        if got {
            done();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_extracted_and_validated() {
        assert_eq!(host_of("https://Www.Example.com:8443/a?b#c"), "www.example.com");
        assert_eq!(host_of("http://u:p@a.b/x"), "a.b");
        assert_eq!(host_of("http://rb.localhost/x"), "rb.localhost");
        assert_eq!(host_of("ftp://a.b"), "");
        assert_eq!(host_of("https://"), "");
        assert_eq!(host_of("https://a b.com/"), "");
        assert!(!valid("../evil") && !valid("a/b") && !valid("a..b") && !valid(".a") && valid("a-b.c1"));
    }

    #[test]
    fn mime_sniffing_rejects_non_images() {
        assert_eq!(mime(b"\x89PNG\r\n"), Some("image/png"));
        assert_eq!(mime(&[0, 0, 1, 0, 1, 0]), Some("image/x-icon"));
        assert_eq!(mime(b"<svg xmlns"), Some("image/svg+xml"));
        assert_eq!(mime(b"<!doctype html>"), None);
        assert_eq!(mime(b""), None);
    }
}
