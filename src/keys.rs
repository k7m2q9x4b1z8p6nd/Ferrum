use webview2_com::{AcceleratorKeyPressedEventHandler, ContainsFullScreenElementChangedEventHandler, Microsoft::Web::WebView2::Win32::*};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MBUTTON, VK_MENU, VK_SHIFT};
use wry::{WebView, WebViewExtWindows};

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

pub fn new_tab_click() -> bool {
    down(VK_CONTROL.0) || down(VK_MBUTTON.0)
}

fn name(vk: u32, ctrl: bool, shift: bool) -> Option<String> {
    Some(
        match (ctrl, shift, vk) {
            (true, false, 0x54) => "newtab",
            (true, false, 0x57) => "close",
            (true, true, 0x54) => "reopen",
            (true, true, 0x4E) => "private",
            (true, false, 0x09 | 0x22) => "next",
            (true, true, 0x09) | (true, false, 0x21) => "prev",
            (true, false, 0x31..=0x39) => return Some(format!("tab{}", vk - 0x30)),
            (true, _, 0x4C) | (false, false, 0x75) => "addr",
            (true, _, 0x52) | (false, false, 0x74) => "reload",
            (true, false, 0x44) => "star",
            (true, false, 0x48) => "history",
            (true, false, 0x4A) => "downloads",
            (true, false, 0x42) => "bookmarks",
            (true, false, 0xBC) => "settings",
            (false, false, 0x7A) => "fullscreen",
            (true, true, 0x46) => "focus",
            _ => return None,
        }
        .to_owned(),
    )
}

pub fn watch_fs(wv: &WebView, on: impl Fn(bool) + 'static) {
    unsafe {
        let h = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |w, _| {
            let Some(w) = w else { return Ok(()) };
            let mut v = Default::default();
            w.ContainsFullScreenElement(&mut v)?;
            on(v.as_bool());
            Ok(())
        }));
        let mut t = 0;
        let _ = wv.webview().add_ContainsFullScreenElementChanged(&h, &mut t);
    }
}

pub fn watch(wv: &WebView, on: impl Fn(String) + 'static) {
    unsafe {
        let c = wv.controller();
        let h = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, a| {
            let Some(a) = a else { return Ok(()) };
            let (mut kind, mut vk, mut lp) = (COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, 0, 0);
            a.KeyEventKind(&mut kind)?;
            a.VirtualKey(&mut vk)?;
            a.KeyEventLParam(&mut lp)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                return Ok(());
            }
            if down(VK_MENU.0) {
                return Ok(());
            }
            if let Some(n) = name(vk, down(VK_CONTROL.0), down(VK_SHIFT.0)) {
                a.SetHandled(true)?;
                if lp & (1 << 30) == 0 {
                    on(n);
                }
            }
            Ok(())
        }));
        let mut t = 0;
        let _ = c.add_AcceleratorKeyPressed(&h, &mut t);
    }
}
