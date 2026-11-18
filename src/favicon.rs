use base64::{engine::general_purpose::STANDARD, Engine};
use webview2_com::{FaviconChangedEventHandler, GetFaviconCompletedHandler, Microsoft::Web::WebView2::Win32::*};
use windows::{core::Interface, Win32::System::Com::IStream};
use wry::{WebView, WebViewExtWindows};

fn data_url(s: &IStream) -> String {
    let (mut out, mut buf) = (Vec::new(), [0u8; 16384]);
    loop {
        let mut n = 0;
        if unsafe { s.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut n)) }.is_err() || n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    if out.is_empty() { String::new() } else { format!("data:image/png;base64,{}", STANDARD.encode(out)) }
}

pub fn watch(wv: &WebView, on: impl Fn(String) + Clone + 'static) {
    unsafe {
        let Ok(w) = wv.webview().cast::<ICoreWebView2_15>() else { return };
        let w2 = w.clone();
        let h = FaviconChangedEventHandler::create(Box::new(move |_, _| {
            let on = on.clone();
            let done = GetFaviconCompletedHandler::create(Box::new(move |_, s| {
                on(s.map(|s| data_url(&s)).unwrap_or_default());
                Ok(())
            }));
            let _ = w2.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &done);
            Ok(())
        }));
        let mut t = 0;
        let _ = w.add_FaviconChanged(&h, &mut t);
    }
}
