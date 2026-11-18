use std::time::Duration;
use webview2_com::{Microsoft::Web::WebView2::Win32::*, TrySuspendCompletedHandler};
use windows::core::{Interface, BOOL};
use wry::{WebView, WebViewExtWindows};

const IDLE: Duration = Duration::from_secs(300);

pub fn idle() -> Duration {
    std::env::var("FERRUM_SUSPEND_SECS").ok().and_then(|s| s.parse().ok()).map_or(IDLE, Duration::from_secs)
}

pub fn warm() -> Duration {
    idle() / 5
}

pub struct Tab {
    pub hidden_for: Option<Duration>,
    pub suspended: bool,
    pub pinned: bool,
    pub loading: bool,
    pub restoring: bool,
    pub audio: bool,
}

pub fn eligible(t: &Tab, limit: Duration, downloading: bool) -> bool {
    !downloading && !t.suspended && !t.pinned && !t.loading && !t.restoring && !t.audio && t.hidden_for.is_some_and(|d| d >= limit)
}

pub fn playing_audio(wv: &WebView) -> bool {
    let mut on = BOOL::default();
    wv.webview().cast::<ICoreWebView2_8>().is_ok_and(|c| unsafe { c.IsDocumentPlayingAudio(&mut on).is_ok() && on.as_bool() })
}

pub fn suspend(wv: &WebView, done: impl FnOnce(bool) + 'static) {
    let Ok(c) = wv.webview().cast::<ICoreWebView2_3>() else { return done(false) };
    let h = TrySuspendCompletedHandler::create(Box::new(move |r, ok| {
        done(r.is_ok() && ok);
        Ok(())
    }));
    unsafe {
        let _ = c.TrySuspend(&h);
    }
}

pub fn resume(wv: &WebView) {
    if let Ok(c) = wv.webview().cast::<ICoreWebView2_3>() {
        unsafe {
            let _ = c.Resume();
        }
    }
}

pub fn level(wv: &WebView, low: bool) {
    if let Ok(c) = wv.webview().cast::<ICoreWebView2_19>() {
        let l = if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
        unsafe {
            let _ = c.SetMemoryUsageTargetLevel(l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab() -> Tab {
        Tab { hidden_for: Some(Duration::from_secs(600)), suspended: false, pinned: false, loading: false, restoring: false, audio: false }
    }

    const LIMIT: Duration = Duration::from_secs(300);

    #[test]
    fn hidden_long_enough_is_eligible() {
        assert!(eligible(&tab(), LIMIT, false));
        assert!(eligible(&Tab { hidden_for: Some(LIMIT), ..tab() }, LIMIT, false));
    }

    #[test]
    fn visible_or_recent_is_not() {
        assert!(!eligible(&Tab { hidden_for: None, ..tab() }, LIMIT, false));
        assert!(!eligible(&Tab { hidden_for: Some(Duration::from_secs(299)), ..tab() }, LIMIT, false));
    }

    #[test]
    fn busy_tabs_are_kept_awake() {
        assert!(!eligible(&Tab { audio: true, ..tab() }, LIMIT, false));
        assert!(!eligible(&Tab { pinned: true, ..tab() }, LIMIT, false));
        assert!(!eligible(&Tab { loading: true, ..tab() }, LIMIT, false));
        assert!(!eligible(&Tab { restoring: true, ..tab() }, LIMIT, false));
        assert!(!eligible(&Tab { suspended: true, ..tab() }, LIMIT, false));
        assert!(!eligible(&tab(), LIMIT, true));
    }
}
