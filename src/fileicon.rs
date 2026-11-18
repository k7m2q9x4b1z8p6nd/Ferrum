use std::{
    collections::HashMap,
    mem::size_of,
    sync::{LazyLock, Mutex},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Graphics::Gdi::{DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, HBITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS},
        Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
        UI::{
            Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES},
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO},
        },
    },
};

static CACHE: LazyLock<Mutex<HashMap<String, Vec<u8>>>> = LazyLock::new(Default::default);

pub fn png(ext: &str) -> Vec<u8> {
    if ext.is_empty() || ext.len() > 16 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Vec::new();
    }
    CACHE.lock().unwrap().entry(ext.to_ascii_lowercase()).or_insert_with_key(|e| render(&format!(".{e}"), true).unwrap_or_default()).clone()
}

pub fn exe(path: &str) -> Vec<u8> {
    CACHE.lock().unwrap().entry(path.to_owned()).or_insert_with_key(|p| render(p, false).unwrap_or_default()).clone()
}

fn render(path: &str, by_ext: bool) -> Option<Vec<u8>> {
    unsafe {
        let name: Vec<u16> = format!("{path}\0").encode_utf16().collect();
        let mut fi = SHFILEINFOW::default();
        SHGetFileInfoW(PCWSTR(name.as_ptr()), FILE_ATTRIBUTE_NORMAL, Some(&mut fi), size_of::<SHFILEINFOW>() as u32, if by_ext { SHGFI_USEFILEATTRIBUTES | SHGFI_ICON | SHGFI_LARGEICON } else { SHGFI_ICON | SHGFI_LARGEICON });
        if fi.hIcon.is_invalid() {
            return None;
        }
        let mut ii = ICONINFO::default();
        let out = GetIconInfo(fi.hIcon, &mut ii).ok().and_then(|_| {
            let mut bm = BITMAP::default();
            GetObjectW(ii.hbmColor.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _));
            let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
            let bits = |hbm: HBITMAP| {
                let mut bi = BITMAPINFO::default();
                bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w as i32, biHeight: -(h as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
                let mut px = vec![0u8; (w * h * 4) as usize];
                let dc = GetDC(None);
                let n = GetDIBits(dc, hbm, 0, h, Some(px.as_mut_ptr() as _), &mut bi, DIB_RGB_COLORS);
                ReleaseDC(None, dc);
                (n != 0).then_some(px)
            };
            let mut px = bits(ii.hbmColor)?;
            let no_alpha = px.chunks_exact(4).all(|p| p[3] == 0);
            let mask = if no_alpha { bits(ii.hbmMask) } else { None };
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                p.swap(0, 2);
                if no_alpha {
                    p[3] = if mask.as_ref().is_none_or(|m| m[i * 4] == 0) { 255 } else { 0 };
                }
            }
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&px).ok()?;
            Some(out)
        });
        let _ = DeleteObject(ii.hbmColor.into());
        let _ = DeleteObject(ii.hbmMask.into());
        let _ = DestroyIcon(fi.hIcon);
        out
    }
}
