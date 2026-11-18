use webview2_com::{take_pwstr, SourceChangedEventHandler};
use windows::core::PWSTR;
use wry::{WebView, WebViewExtWindows};

pub fn watch(wv: &WebView, on: impl Fn(String) + 'static) {
    unsafe {
        let h = SourceChangedEventHandler::create(Box::new(move |w, _| {
            let Some(w) = w else { return Ok(()) };
            let mut u = PWSTR::null();
            w.Source(&mut u)?;
            on(take_pwstr(u));
            Ok(())
        }));
        let mut t = 0;
        let _ = wv.webview().add_SourceChanged(&h, &mut t);
    }
}
