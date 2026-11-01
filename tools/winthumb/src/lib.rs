//! Thumbnails of GIFkit projects in Windows Explorer.
//!
//! Explorer loads this DLL (registered by gifkit.exe next to it) and hands
//! it each `.gifkit` file as a stream. A project is a zip: its
//! `thumbnail.jpg` / `thumbnail.png` is used, or the first frame for
//! projects saved before there was one.

use image::RgbaImage;
use std::io::Read;

/// The picture to show for a project's bytes, at most `size` across.
pub fn picture(bytes: &[u8], size: u32) -> Option<RgbaImage> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let name = ["thumbnail.jpg", "thumbnail.png", "pics/00000.png", "frames/00000.png"].into_iter().find(|n| z.by_name(n).is_ok())?;
    let mut b = Vec::new();
    z.by_name(name).ok()?.read_to_end(&mut b).ok()?;
    let mut img = fit(image::load_from_memory(&b).ok()?.to_rgba8(), size.max(256));
    // older projects: their first frame, marked as a project here (newer
    // ones have the mark in their thumbnail already)
    if !name.starts_with("thumbnail") {
        badge(&mut img);
    }
    Some(fit(img, size))
}

fn fit(img: RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= size || size == 0 {
        return img;
    }
    let k = size as f32 / w.max(h) as f32;
    image::imageops::resize(&img, ((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1), image::imageops::FilterType::Triangle)
}

/// The GIFkit icon in the bottom-right corner of `img` (with a soft shadow),
/// so a project's thumbnail doesn't pass for a plain picture.
fn badge(img: &mut RgbaImage) {
    static ICON: std::sync::OnceLock<Option<RgbaImage>> = std::sync::OnceLock::new();
    let Some(icon) = ICON.get_or_init(|| image::load_from_memory(include_bytes!("../../../assets/icon/gifkit-128.png")).ok().map(|i| i.to_rgba8())) else { return };
    let (w, h) = img.dimensions();
    let b = ((w.min(h) as f32 * 0.32).round() as u32).clamp(12, 128);
    if b + 2 > w.min(h) {
        return;
    }
    let ic = image::imageops::resize(icon, b, b, image::imageops::FilterType::Lanczos3);
    let m = ((b as f32 * 0.08).round() as i64).max(1);
    let (x0, y0) = (w as i64 - b as i64 - m, h as i64 - b as i64 - m);
    // the shadow: the icon's shape, blurred, a little down and right
    let pad = (b / 6).max(2);
    let mut sh = image::GrayImage::new(b + pad * 2, b + pad * 2);
    for (x, y, p) in ic.enumerate_pixels() {
        sh.put_pixel(x + pad, y + pad, image::Luma([p[3]]));
    }
    let sh = image::imageops::blur(&sh, (b as f32 / 24.0).max(1.0));
    let off = (b as i64 / 24).max(1);
    let mix = |img: &mut RgbaImage, x: i64, y: i64, c: [f32; 3], a: f32| {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || a <= 0.0 {
            return;
        }
        let p = img.get_pixel_mut(x as u32, y as u32);
        let da = p[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for k in 0..3 {
            p[k] = ((c[k] * a + p[k] as f32 * da * (1.0 - a)) / oa.max(1e-6)).round().clamp(0.0, 255.0) as u8;
        }
        p[3] = (oa * 255.0).round() as u8;
    };
    for (x, y, p) in sh.enumerate_pixels() {
        mix(img, x0 + x as i64 - pad as i64 + off, y0 + y as i64 - pad as i64 + off, [0.0; 3], p[0] as f32 / 255.0 * 0.45);
    }
    for (x, y, p) in ic.enumerate_pixels() {
        mix(img, x0 + x as i64, y0 + y as i64, [p[0] as f32, p[1] as f32, p[2] as f32], p[3] as f32 / 255.0);
    }
}


/// The class Explorer asks for (also written to the registry by gifkit.exe).
pub const CLSID_STR: &str = "{6F3B1C2E-9A4D-4E7B-8C51-2D7A0B9E4F13}";

#[cfg(windows)]
mod com {
    use super::picture;
    use std::ffi::c_void;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, S_FALSE};
    use windows::Win32::Graphics::Gdi::{CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP};
    use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl, IStream};
    use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
    use windows::Win32::UI::Shell::{IThumbnailProvider, IThumbnailProvider_Impl, WTSAT_ARGB, WTS_ALPHATYPE};
    use windows_core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HRESULT};

    const CLSID: GUID = GUID::from_u128(0x6F3B1C2E_9A4D_4E7B_8C51_2D7A0B9E4F13);

    #[implement(IThumbnailProvider, IInitializeWithStream)]
    struct Provider {
        data: Mutex<Option<Vec<u8>>>,
    }

    impl IInitializeWithStream_Impl for Provider_Impl {
        fn Initialize(&self, pstream: Ref<'_, IStream>, _grfmode: u32) -> windows_core::Result<()> {
            let s = pstream.ok()?;
            let mut buf = Vec::new();
            let mut chunk = vec![0u8; 1 << 16];
            loop {
                let mut n = 0u32;
                unsafe { s.Read(chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, Some(&mut n)) }.ok()?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n as usize]);
                // a project bigger than this isn't worth reading for a thumbnail
                if buf.len() > 1 << 30 {
                    return Err(E_FAIL.into());
                }
            }
            *self.data.lock().map_err(|_| windows_core::Error::from(E_FAIL))? = Some(buf);
            Ok(())
        }
    }

    impl IThumbnailProvider_Impl for Provider_Impl {
        fn GetThumbnail(&self, cx: u32, phbmp: *mut HBITMAP, pdwalpha: *mut WTS_ALPHATYPE) -> windows_core::Result<()> {
            if phbmp.is_null() || pdwalpha.is_null() {
                return Err(E_POINTER.into());
            }
            let data = self.data.lock().map_err(|_| windows_core::Error::from(E_FAIL))?;
            let img = data.as_deref().and_then(|d| picture(d, cx)).ok_or_else(|| windows_core::Error::from(E_FAIL))?;
            let (w, h) = img.dimensions();
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    // top-down rows
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let hbmp = unsafe { CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }?;
            if bits.is_null() {
                return Err(E_FAIL.into());
            }
            // blue, green, red, alpha, with the colour already times alpha
            let out = unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize) };
            for (o, p) in out.chunks_exact_mut(4).zip(img.pixels()) {
                let a = p[3] as u32;
                let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                o[0] = m(p[2]);
                o[1] = m(p[1]);
                o[2] = m(p[0]);
                o[3] = p[3];
            }
            unsafe {
                *phbmp = hbmp;
                *pdwalpha = WTSAT_ARGB;
            }
            Ok(())
        }
    }

    #[implement(IClassFactory)]
    struct Factory;

    impl IClassFactory_Impl for Factory_Impl {
        fn CreateInstance(&self, punkouter: Ref<'_, IUnknown>, riid: *const GUID, ppvobject: *mut *mut c_void) -> windows_core::Result<()> {
            if ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *ppvobject = std::ptr::null_mut() };
            if !punkouter.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let p: IUnknown = Provider { data: Mutex::new(None) }.into();
            unsafe { p.query(riid, ppvobject) }.ok()
        }

        fn LockServer(&self, _flock: BOOL) -> windows_core::Result<()> {
            Ok(())
        }
    }

    /// Explorer asks for our class here.
    ///
    /// # Safety
    /// Called by COM with valid pointers.
    #[no_mangle]
    pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
        if ppv.is_null() || rclsid.is_null() || riid.is_null() {
            return E_POINTER;
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if unsafe { *rclsid } != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let f: IClassFactory = Factory.into();
        unsafe { f.query(riid, ppv) }
    }

    /// Kept loaded once used (thumbnails come in bursts).
    #[no_mangle]
    pub extern "system" fn DllCanUnloadNow() -> HRESULT {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn project(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut out);
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (n, b) in entries {
                z.start_file(*n, o).unwrap();
                z.write_all(b).unwrap();
            }
            z.finish().unwrap();
        }
        out.into_inner()
    }

    fn png(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba(c))).write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png).unwrap();
        b
    }

    #[test]
    fn thumbnail_first_then_first_frame() {
        let p = project(&[("project.json", b"{}".to_vec()), ("pics/00000.png", png(40, 20, [0, 0, 255, 255])), ("thumbnail.png", png(300, 150, [255, 0, 0, 255]))]);
        let t = picture(&p, 96).unwrap();
        assert_eq!(t.dimensions(), (96, 48));
        assert_eq!(t.get_pixel(3, 3)[0], 255);
        let old = project(&[("project.json", b"{}".to_vec()), ("frames/00000.png", png(40, 20, [0, 0, 255, 255]))]);
        assert_eq!(picture(&old, 96).unwrap().dimensions(), (40, 20));
        assert!(picture(b"not a zip", 96).is_none());
    }
}
