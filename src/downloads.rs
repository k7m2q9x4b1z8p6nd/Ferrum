use crate::{fetch::{self, Rt}, settings};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use webview2_com::{take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler, Microsoft::Web::WebView2::Win32::*, StateChangedEventHandler};
use windows::core::{Interface, PWSTR};
use wry::{WebView, WebViewExtWindows};

const CAP: usize = 500;
static NEXT: AtomicU32 = AtomicU32::new(1);

#[derive(Serialize, Deserialize)]
pub struct Dl {
    #[serde(default)]
    pub id: u32,
    pub url: String,
    pub path: PathBuf,
    pub size: u64,
    pub state: String,
    #[serde(skip)]
    pub received: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub ranged: bool,
    #[serde(default)]
    pub chunks: Vec<(u64, u64)>,
    #[serde(default)]
    pub referer: String,
    #[serde(skip)]
    pub rt: Option<Arc<Rt>>,
    #[serde(skip)]
    pub speed: f64,
    #[serde(skip)]
    mark: u64,
    #[serde(skip, default = "Instant::now")]
    at: Instant,
}

impl Dl {
    pub fn new(url: String, path: PathBuf) -> Self {
        Self { id: NEXT.fetch_add(1, Ordering::Relaxed), url, path, size: 0, state: "active".into(), received: 0, total: 0, ranged: false, chunks: vec![], referer: String::new(), rt: None, speed: 0.0, mark: 0, at: Instant::now() }
    }

    pub fn tick(&mut self) -> bool {
        let dt = self.at.elapsed();
        if dt < Duration::from_millis(500) {
            return false;
        }
        self.speed = self.received.saturating_sub(self.mark) as f64 / dt.as_secs_f64();
        self.mark = self.received;
        self.at = Instant::now();
        true
    }

    pub fn left(&self) -> u64 {
        self.chunks.iter().map(|c| c.1 - c.0).sum()
    }

    pub fn discard(&self) {
        let _ = std::fs::remove_file(fetch::part_path(&self.path));
    }

    pub fn gone(&self) -> bool {
        self.state == "done" && !self.path.exists()
    }

    pub fn view(&self) -> Value {
        let gone = self.gone();
        json!({
            "id": self.id, "gone": gone,
            "name": self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.url.clone()),
            "state": self.state, "size": self.size, "recv": self.received, "total": self.total, "speed": self.speed,
            "can": self.state == "done" && !gone,
            "pause": self.ranged && self.state == "active", "paused": self.state == "paused", "cancel": matches!(self.state.as_str(), "active" | "paused"),
        })
    }
}

pub fn find(dls: &[Dl], id: u32) -> Option<&Dl> {
    dls.iter().find(|d| d.id == id && d.state == "done" && !d.gone())
}

pub fn load() -> Vec<Dl> {
    let mut v: Vec<Dl> = std::fs::read(settings::dir().join("downloads.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    NEXT.store(v.iter().map(|d| d.id).max().unwrap_or(0) + 1, Ordering::Relaxed);
    for d in v.iter_mut().filter(|d| d.id == 0) {
        d.id = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    for d in v.iter_mut().filter(|d| d.state == "active") {
        d.state = if d.ranged && fetch::part_path(&d.path).exists() { "paused" } else { "fail" }.into();
    }
    for d in v.iter_mut().filter(|d| d.state == "paused") {
        d.received = d.total.saturating_sub(d.left());
    }
    v
}

pub fn save(dls: &mut Vec<Dl>) {
    if dls.len() > CAP {
        dls.drain(..dls.len() - CAP);
    }
    let done: Vec<&Dl> = dls.iter().filter(|d| d.state != "active" || (d.ranged && !d.chunks.is_empty())).collect();
    if let Ok(b) = serde_json::to_vec(&done) {
        let _ = std::fs::write(settings::dir().join("downloads.json"), b);
    }
}

fn report(op: &ICoreWebView2DownloadOperation, url: &str, on: &impl Fn(String, u64, u64)) {
    let (mut r, mut t) = (0i64, 0i64);
    unsafe {
        let _ = op.BytesReceived(&mut r);
        let _ = op.TotalBytesToReceive(&mut t);
    }
    on(url.to_owned(), r.max(0) as u64, t.max(0) as u64);
}

pub fn watch(wv: &WebView, on: impl Fn(String, u64, u64) + Clone + 'static) {
    unsafe {
        let Ok(w) = wv.webview().cast::<ICoreWebView2_4>() else { return };
        let h = DownloadStartingEventHandler::create(Box::new(move |_, args| {
            let Some(op) = args.and_then(|a| a.DownloadOperation().ok()) else { return Ok(()) };
            let mut u = PWSTR::null();
            op.Uri(&mut u)?;
            let url = take_pwstr(u);
            let (on_b, url_b) = (on.clone(), url.clone());
            let bytes = BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
                if let Some(op) = op {
                    report(&op, &url_b, &on_b);
                }
                Ok(())
            }));
            let state = StateChangedEventHandler::create(Box::new({
                let on = on.clone();
                move |op, _| {
                    if let Some(op) = op {
                        report(&op, &url, &on);
                    }
                    Ok(())
                }
            }));
            let mut t = 0;
            let _ = op.add_BytesReceivedChanged(&bytes, &mut t);
            let _ = op.add_StateChanged(&state, &mut t);
            Ok(())
        }));
        let mut t = 0;
        let _ = w.add_DownloadStarting(&h, &mut t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(path: PathBuf) -> Dl {
        let mut d = Dl::new("http://x/f".into(), path);
        d.state = "done".into();
        d
    }

    #[test]
    fn find_requires_listed_done_existing_file() {
        let p = std::env::temp_dir().join(format!("rb_dl_find_{}.txt", std::process::id()));
        std::fs::write(&p, b"x").unwrap();
        let ok = done(p.clone());
        let mut active = done(p.clone());
        active.state = "active".into();
        let missing = done(p.with_extension("gone"));
        let (a, b, c) = (ok.id, active.id, missing.id);
        let list = vec![ok, active, missing];
        assert_eq!(find(&list, a).map(|d| d.path.clone()), Some(p.clone()));
        assert!(find(&list, b).is_none());
        assert!(find(&list, c).is_none());
        assert!(find(&list, u32::MAX).is_none());
        std::fs::remove_file(&p).unwrap();
        assert!(find(&list, a).is_none());
    }
}
