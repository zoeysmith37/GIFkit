//! "Add image": composite a (possibly animated) image over the animation with
//! per-frame placement.

use crate::anim::Animation;
use image::{imageops, RgbaImage};

#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Placement {
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// clockwise rotation in degrees about the image centre
    pub angle: f32,
}

#[derive(Clone)]
pub struct OverlaySource {
    pub name: String,
    /// where it was loaded from (for presets)
    pub path: Option<std::path::PathBuf>,
    pub frames: Vec<RgbaImage>,
    pub delays: Vec<u16>,
}

impl OverlaySource {
    pub fn native_size(&self) -> (u32, u32) {
        self.frames.first().map(|f| f.dimensions()).unwrap_or((1, 1))
    }

    /// Pick the overlay frame to show at time `t` seconds (animated overlays loop).
    pub fn frame_at(&self, t: f32) -> &RgbaImage {
        if self.frames.len() <= 1 {
            return &self.frames[0];
        }
        let total: f32 = self.delays.iter().map(|d| crate::anim::effective_delay(*d) as f32 / 100.0).sum();
        let mut tt = if total > 0.0 { t % total } else { 0.0 };
        for (i, d) in self.delays.iter().enumerate() {
            let d = crate::anim::effective_delay(*d) as f32 / 100.0;
            if tt < d {
                return &self.frames[i];
            }
            tt -= d;
        }
        &self.frames[0]
    }
}

/// Draw `img` (already the right frame) into `dst` filling the oriented box `b`.
pub fn draw_in_obb(dst: &mut RgbaImage, img: &RgbaImage, b: crate::geo::Obb, opacity: f32) {
    let w = b.w.round().max(1.0) as u32;
    let h = b.h.round().max(1.0) as u32;
    if w > 16384 || h > 16384 {
        return;
    }
    let scaled;
    let mut s: &RgbaImage = if img.dimensions() == (w, h) {
        img
    } else {
        scaled = imageops::resize(img, w, h, imageops::FilterType::Triangle);
        &scaled
    };
    let rotated;
    if (b.angle % 360.0).abs() > 0.01 {
        rotated = crate::ops::rotate_any_image(s, b.angle, image::Rgba([0, 0, 0, 0]), true);
        s = &rotated;
    }
    let x = (b.cx - s.width() as f32 / 2.0).round() as i64;
    let y = (b.cy - s.height() as f32 / 2.0).round() as i64;
    crate::ops::overlay_with_opacity(dst, s, x, y, opacity);
}

pub fn draw_overlay(base: &RgbaImage, src: &OverlaySource, p: &Placement, t: f32, opacity: f32) -> RgbaImage {
    let mut out = base.clone();
    if !p.enabled || p.w == 0 || p.h == 0 {
        return out;
    }
    let f = src.frame_at(t);
    let scaled;
    let img = if f.dimensions() == (p.w, p.h) {
        f
    } else {
        scaled = imageops::resize(f, p.w, p.h, imageops::FilterType::Triangle);
        &scaled
    };
    crate::ops::overlay_with_opacity(&mut out, img, p.x as i64, p.y as i64, opacity);
    out
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply(anim: &Animation, src: &OverlaySource, placements: &[Placement], opacity: f32) -> Animation {
    let times = crate::effects::frame_start_times(anim);
    anim.map_images(|i, img| {
        let p = placements.get(i).copied().unwrap_or(Placement { enabled: false, x: 0, y: 0, w: 0, h: 0, angle: 0.0 });
        draw_overlay(img, src, &p, times[i], opacity)
    })
}
