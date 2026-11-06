//! 앱 아이콘 추출 (EXE/DLL/ICO 리소스 → RGBA, Store 앱 PNG 로고 → RGBA).

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW,
};

use crate::cmdline::parse_icon_location;
use crate::pathutil;
use crate::win::expand_env;

/// 직선(비-premultiplied) 알파 RGBA 이미지.
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// 아이콘 출처.
#[derive(Debug, Clone)]
pub enum IconSource {
    /// `DisplayIcon` 형식(`path,index`).
    Resource(String),
    /// PNG 파일(Store 앱 로고).
    Png(String),
}

pub fn load(source: &IconSource, size: u32) -> Option<RgbaImage> {
    match source {
        IconSource::Resource(spec) => extract_resource_icon(spec, size),
        IconSource::Png(path) => load_png(path, size),
    }
}

fn extract_resource_icon(spec: &str, size: u32) -> Option<RgbaImage> {
    let (path, index) = parse_icon_location(spec)?;
    let path = expand_env(&path);
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        return load_png(&path, size);
    }
    if !(lower.ends_with(".exe") || lower.ends_with(".ico") || lower.ends_with(".dll")) {
        return None;
    }
    let wide: Vec<u16> = path.encode_utf16().collect();
    if wide.len() >= 260 {
        return None;
    }
    let mut name = [0u16; 260];
    name[..wide.len()].copy_from_slice(&wide);

    let mut icons = [HICON::default()];
    let mut id = 0u32;
    // SAFETY: name은 NUL 종료 MAX_PATH 버퍼, 출력 배열 길이는 1이다.
    let n = unsafe {
        PrivateExtractIconsW(
            &name,
            index,
            size as i32,
            size as i32,
            Some(&mut icons),
            Some(&mut id),
            0,
        )
    };
    if n == 0 || n == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    // SAFETY: 유효한 HICON을 변환한 뒤 정확히 한 번 파괴한다.
    unsafe {
        let img = hicon_to_rgba(icons[0]);
        let _ = DestroyIcon(icons[0]);
        img
    }
}

unsafe fn bitmap_pixels(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    // SAFETY: buf는 w*h*4 바이트, bmi는 32bpp top-down 헤더.
    let lines = unsafe {
        GetDIBits(
            dc,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    (lines > 0).then_some(buf)
}

unsafe fn hicon_to_rgba(icon: HICON) -> Option<RgbaImage> {
    let mut info = ICONINFO::default();
    // SAFETY: 유효한 HICON.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let color = info.hbmColor;
    let mask = info.hbmMask;

    let result = (|| {
        if color.is_invalid() {
            return None; // 흑백 아이콘은 건너뛴다
        }
        let mut bm = BITMAP::default();
        // SAFETY: bm 크기를 정확히 전달한다.
        let got = unsafe {
            GetObjectW(
                color.into(),
                size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut BITMAP as *mut c_void),
            )
        };
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 || bm.bmWidth > 512 || bm.bmHeight > 512
        {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        // SAFETY: 메모리 DC는 아래에서 해제한다.
        let dc = unsafe { CreateCompatibleDC(None) };
        // SAFETY: 유효한 DC와 비트맵.
        let mut px = unsafe { bitmap_pixels(dc, color, w, h) };
        if let Some(buf) = px.as_mut() {
            let has_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
            if !has_alpha {
                // 알파 없는 구형 아이콘: AND 마스크(흰색=투명)로 알파를 만든다.
                // SAFETY: 유효한 DC와 마스크 비트맵.
                match unsafe { bitmap_pixels(dc, mask, w, h) } {
                    Some(m) => {
                        for (p, mk) in buf.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                            p[3] = if mk[0] == 0 { 255 } else { 0 };
                        }
                    }
                    None => buf.chunks_exact_mut(4).for_each(|p| p[3] = 255),
                }
            }
            for p in buf.chunks_exact_mut(4) {
                p.swap(0, 2); // BGRA → RGBA
            }
        }
        // SAFETY: CreateCompatibleDC로 만든 DC.
        unsafe {
            let _ = DeleteDC(dc);
        }
        px.map(|pixels| RgbaImage {
            width: w as u32,
            height: h as u32,
            pixels,
        })
    })();

    // SAFETY: GetIconInfo가 만든 비트맵은 호출자가 해제해야 한다.
    unsafe {
        if !color.is_invalid() {
            let _ = DeleteObject(color.into());
        }
        if !mask.is_invalid() {
            let _ = DeleteObject(mask.into());
        }
    }
    result
}

fn load_png(path: &str, size: u32) -> Option<RgbaImage> {
    let img = image::open(path).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let img = if w > size * 2 || h > size * 2 {
        image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (w, h) = img.dimensions();
    Some(RgbaImage {
        width: w,
        height: h,
        pixels: img.into_raw(),
    })
}

/// Store 앱 매니페스트의 로고 경로(스케일 한정자 변형 포함)를 실제 파일로 해석한다.
pub fn resolve_store_logo(install_path: &str) -> Option<String> {
    let manifest =
        std::fs::read_to_string(pathutil::join(install_path, "AppxManifest.xml")).ok()?;
    let rel = attr_value(&manifest, "Square44x44Logo=\"")
        .or_else(|| element_text(&manifest, "<Logo>"))?;
    let full = pathutil::join(install_path, &rel.replace('/', "\\"));
    if std::path::Path::new(&full).is_file() {
        return Some(full);
    }
    // Assets\Square44x44Logo.png → Square44x44Logo.targetsize-48_altform-unplated.png 등
    let dir = pathutil::parent(&full)?.to_string();
    let stem = pathutil::file_stem(&full).to_lowercase();
    let mut best: Option<(i32, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !(name.starts_with(&format!("{stem}.")) && name.ends_with(".png")) {
            continue;
        }
        let score = [
            ("targetsize-48_altform-unplated", 100),
            ("targetsize-64_altform-unplated", 95),
            ("targetsize-32_altform-unplated", 90),
            ("targetsize-48_altform-lightunplated", 85),
            ("targetsize-48", 80),
            ("scale-200", 70),
            ("targetsize-64", 65),
            ("scale-150", 60),
            ("scale-100", 50),
        ]
        .iter()
        .find(|(k, _)| name.contains(k))
        .map_or(10, |(_, s)| *s);
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, entry.path().to_string_lossy().into_owned()));
        }
    }
    best.map(|(_, p)| p)
}

fn attr_value(xml: &str, needle: &str) -> Option<String> {
    let start = xml.find(needle)? + needle.len();
    let end = xml[start..].find('"')? + start;
    Some(xml[start..end].to_string()).filter(|s| !s.is_empty())
}

fn element_text(xml: &str, open: &str) -> Option<String> {
    let start = xml.find(open)? + open.len();
    let end = xml[start..].find('<')? + start;
    Some(xml[start..end].trim().to_string()).filter(|s| !s.is_empty())
}
