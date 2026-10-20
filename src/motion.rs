//! Animated effects: intro and outro transitions (fades, wipes, slides,
//! bounces …) and looping motion (shake, pulse, hop …). Each works on one
//! frame, given where that frame sits in the animation.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Transition {
    #[default]
    None,
    Fade,
    WipeLeft,
    WipeRight,
    WipeUp,
    WipeDown,
    Iris,
    Blinds,
    Zoom,
    Spin,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    Drop,
    Pixelate,
}

impl Transition {
    pub const ALL: [Transition; 16] = [
        Transition::None,
        Transition::Fade,
        Transition::WipeLeft,
        Transition::WipeRight,
        Transition::WipeUp,
        Transition::WipeDown,
        Transition::Iris,
        Transition::Blinds,
        Transition::Zoom,
        Transition::Spin,
        Transition::SlideLeft,
        Transition::SlideRight,
        Transition::SlideUp,
        Transition::SlideDown,
        Transition::Drop,
        Transition::Pixelate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Transition::None => "None",
            Transition::Fade => "Fade",
            Transition::WipeLeft => "Wipe from left",
            Transition::WipeRight => "Wipe from right",
            Transition::WipeUp => "Wipe from bottom",
            Transition::WipeDown => "Wipe from top",
            Transition::Iris => "Iris (circle)",
            Transition::Blinds => "Blinds",
            Transition::Zoom => "Zoom",
            Transition::Spin => "Spin",
            Transition::SlideLeft => "Slide from left",
            Transition::SlideRight => "Slide from right",
            Transition::SlideUp => "Slide from bottom",
            Transition::SlideDown => "Slide from top",
            Transition::Drop => "Drop and bounce",
            Transition::Pixelate => "Pixelate",
        }
    }
}

/// Looping motion over the whole animation. Several can be on at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopFx {
    Shake,
    Pulse,
    Hop,
    Wobble,
    Rainbow,
    Glitch,
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// The classic bouncing ball ease: lands, bounces twice, settles.
fn ease_out_bounce(t: f32) -> f32 {
    let (n, d) = (7.5625, 2.75);
    if t < 1.0 / d {
        n * t * t
    } else if t < 2.0 / d {
        let t = t - 1.5 / d;
        n * t * t + 0.75
    } else if t < 2.5 / d {
        let t = t - 2.25 / d;
        n * t * t + 0.9375
    } else {
        let t = t - 2.625 / d;
        n * t * t + 0.984375
    }
}

fn hidden_px(transparent: bool) -> Rgba<u8> {
    if transparent { Rgba([0, 0, 0, 0]) } else { Rgba([0, 0, 0, 255]) }
}

/// Blend each pixel toward the hidden colour by `1 - cover(x, y)`.
fn mask(img: &mut RgbaImage, transparent: bool, cover: impl Fn(f32, f32) -> f32 + Sync) {
    let w = img.width() as usize;
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        let k = cover(x, y).clamp(0.0, 1.0);
        if k >= 1.0 {
            return;
        }
        if transparent {
            p[3] = (p[3] as f32 * k).round() as u8;
        } else {
            for c in &mut p[..3] {
                *c = (*c as f32 * k).round() as u8;
            }
            p[3] = (p[3] as f32 + (255.0 - p[3] as f32) * (1.0 - k)).round() as u8;
        }
    });
}

/// Draw `src` scaled, turned and moved (about its centre) onto a blank frame.
fn place(src: &RgbaImage, scale: f32, angle_deg: f32, dx: f32, dy: f32, bg: Rgba<u8>) -> RgbaImage {
    let (w, h) = src.dimensions();
    let mut out = RgbaImage::from_pixel(w, h, bg);
    if scale <= 0.001 {
        return out;
    }
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (s, c) = (-angle_deg.to_radians()).sin_cos();
    let wu = w as usize;
    out.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        // inverse map: output pixel to source position
        let x = (i % wu) as f32 + 0.5 - cx - dx;
        let y = (i / wu) as f32 + 0.5 - cy - dy;
        let sx = (x * c - y * s) / scale + cx - 0.5;
        let sy = (x * s + y * c) / scale + cy - 0.5;
        if let Some(v) = bilinear(src, sx, sy) {
            // source over the background
            let a = v[3] as f32 / 255.0;
            for k in 0..3 {
                p[k] = (v[k] as f32 * a + p[k] as f32 * (1.0 - a)).round() as u8;
            }
            p[3] = (v[3] as f32 + p[3] as f32 * (1.0 - a)).round() as u8;
        }
    });
    out
}

fn bilinear(src: &RgbaImage, x: f32, y: f32) -> Option<[u8; 4]> {
    let (w, h) = (src.width() as i32, src.height() as i32);
    if x < -0.5 || y < -0.5 || x > w as f32 - 0.5 || y > h as f32 - 0.5 {
        return None;
    }
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xx: i32, yy: i32| src.get_pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32).0;
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut o = [0u8; 4];
    for k in 0..4 {
        let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
        let bot = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
        o[k] = (top * (1.0 - fy) + bot * fy).round() as u8;
    }
    Some(o)
}

fn pixelate(img: &mut RgbaImage, block: u32) {
    if block <= 1 {
        return;
    }
    let (w, h) = img.dimensions();
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (bw, bh) = (block.min(w - bx), block.min(h - by));
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y);
                    for k in 0..4 {
                        sum[k] += p[k] as u32;
                    }
                }
            }
            let n = bw * bh;
            let avg = Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, (sum[3] / n) as u8]);
            for y in by..by + bh {
                for x in bx..bx + bw {
                    img.put_pixel(x, y, avg);
                }
            }
        }
    }
}

/// Show `img` partly revealed: `p` runs from 0 (hidden) to 1 (fully shown).
pub fn transition(img: &RgbaImage, kind: Transition, p: f32, transparent: bool) -> RgbaImage {
    let p = p.clamp(0.0, 1.0);
    if p >= 1.0 || kind == Transition::None {
        return img.clone();
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let bg = hidden_px(transparent);
    // soft edge for wipes, in pixels
    let soft = (w.min(h) * 0.04).max(2.0);
    let mut out = img.clone();
    match kind {
        Transition::None => {}
        Transition::Fade => mask(&mut out, transparent, |_, _| p),
        Transition::WipeLeft => {
            let edge = p * (w + soft);
            mask(&mut out, transparent, |x, _| (edge - x) / soft);
        }
        Transition::WipeRight => {
            let edge = p * (w + soft);
            mask(&mut out, transparent, |x, _| (edge - (w - x)) / soft);
        }
        Transition::WipeDown => {
            let edge = p * (h + soft);
            mask(&mut out, transparent, |_, y| (edge - y) / soft);
        }
        Transition::WipeUp => {
            let edge = p * (h + soft);
            mask(&mut out, transparent, |_, y| (edge - (h - y)) / soft);
        }
        Transition::Iris => {
            let r = ease_in_out(p) * ((w * w + h * h).sqrt() / 2.0 + soft);
            let (cx, cy) = (w / 2.0, h / 2.0);
            mask(&mut out, transparent, |x, y| (r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()) / soft);
        }
        Transition::Blinds => {
            let slat = (h / 8.0).max(4.0);
            let open = ease_in_out(p) * slat;
            mask(&mut out, transparent, |_, y| (open - (y % slat)) / 1.5);
        }
        Transition::Zoom => out = place(img, ease_out_cubic(p), 0.0, 0.0, 0.0, bg),
        Transition::Spin => {
            let e = ease_out_cubic(p);
            out = place(img, e, (1.0 - e) * 360.0, 0.0, 0.0, bg);
        }
        Transition::SlideLeft => out = place(img, 1.0, 0.0, -(1.0 - ease_out_cubic(p)) * w, 0.0, bg),
        Transition::SlideRight => out = place(img, 1.0, 0.0, (1.0 - ease_out_cubic(p)) * w, 0.0, bg),
        Transition::SlideUp => out = place(img, 1.0, 0.0, 0.0, (1.0 - ease_out_cubic(p)) * h, bg),
        Transition::SlideDown => out = place(img, 1.0, 0.0, 0.0, -(1.0 - ease_out_cubic(p)) * h, bg),
        Transition::Drop => out = place(img, 1.0, 0.0, 0.0, -(1.0 - ease_out_bounce(p)) * h, bg),
        Transition::Pixelate => {
            let block = ((1.0 - p) * w.min(h) / 6.0).round().max(1.0) as u32;
            pixelate(&mut out, block);
            // fade in over the first part so it doesn't start as a solid block
            let f = (p * 3.0).min(1.0);
            mask(&mut out, transparent, |_, _| f);
        }
    }
    out
}

/// Small deterministic noise for shake and glitch (same result every render).
fn hash(i: u64) -> f32 {
    let mut x = i.wrapping_mul(0x9E3779B97F4A7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 29;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Looping motion for frame `idx` of `n`. `amount` is 0 to 100, `cycles` how
/// many times it repeats over the animation (whole numbers keep the loop seamless).
pub fn looped(img: &RgbaImage, fx: &[LoopFx], idx: usize, n: usize, amount: f32, cycles: u32, transparent: bool) -> RgbaImage {
    if fx.is_empty() || n == 0 {
        return img.clone();
    }
    let k = (amount / 100.0).clamp(0.0, 2.0);
    let phase = idx as f32 / n as f32 * cycles.max(1) as f32 * TAU;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let side = w.min(h);
    let (mut scale, mut angle, mut dx, mut dy) = (1.0f32, 0.0f32, 0.0f32, 0.0f32);
    for f in fx {
        match f {
            LoopFx::Shake => {
                dx += (hash(idx as u64 * 2) - 0.5) * side * 0.06 * k;
                dy += (hash(idx as u64 * 2 + 1) - 0.5) * side * 0.06 * k;
            }
            LoopFx::Pulse => scale *= 1.0 + 0.08 * k * (0.5 - 0.5 * phase.cos()),
            // |sin| makes a hop that lands and springs up again
            LoopFx::Hop => dy -= side * 0.12 * k * (phase / 2.0).sin().abs(),
            LoopFx::Wobble => angle += 8.0 * k * phase.sin(),
            _ => {}
        }
    }
    let mut out = if (scale, angle, dx, dy) != (1.0, 0.0, 0.0, 0.0) {
        // pulse grows past the edges; shrinking would leave borders, so only grow
        let grow = if fx.contains(&LoopFx::Shake) || fx.contains(&LoopFx::Wobble) { 1.0 + 0.08 * k } else { 1.0 };
        place(img, scale * grow, angle, dx, dy, hidden_px(transparent))
    } else {
        img.clone()
    };
    if fx.contains(&LoopFx::Rainbow) {
        hue_rotate(&mut out, idx as f32 / n as f32 * cycles.max(1) as f32 * 360.0 * k.min(1.0));
    }
    if fx.contains(&LoopFx::Glitch) && hash(idx as u64 * 7 + 3) < 0.35 * k.min(1.0) + 0.05 {
        glitch(&mut out, idx as u64, k);
    }
    out
}

fn hue_rotate(img: &mut RgbaImage, deg: f32) {
    let (s, c) = deg.to_radians().sin_cos();
    // rotation about the grey axis in RGB space
    let m = [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ];
    img.par_chunks_mut(4).for_each(|p| {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        for (k, row) in m.iter().enumerate() {
            p[k] = (row[0] * r + row[1] * g + row[2] * b).round().clamp(0.0, 255.0) as u8;
        }
    });
}

/// Colour channels pulled apart and a few rows shoved sideways.
fn glitch(img: &mut RgbaImage, seed: u64, k: f32) {
    let src = img.clone();
    let (w, h) = img.dimensions();
    let split = ((w.min(h) as f32) * 0.02 * k).round().max(1.0) as i32;
    // bands that slide
    let bands: Vec<(u32, u32, i32)> = (0..4)
        .map(|b| {
            let y0 = (hash(seed * 31 + b) * h as f32) as u32;
            let bh = ((hash(seed * 37 + b) * 0.08 + 0.01) * h as f32) as u32;
            let shift = ((hash(seed * 41 + b) - 0.5) * w as f32 * 0.15 * k) as i32;
            (y0, (y0 + bh).min(h), shift)
        })
        .collect();
    for y in 0..h {
        let shift = bands.iter().find(|(a, b, _)| y >= *a && y < *b).map(|b| b.2).unwrap_or(0);
        for x in 0..w {
            let get = |dx: i32| src.get_pixel(((x as i32 + shift + dx).rem_euclid(w as i32)) as u32, y).0;
            let (r, c, b) = (get(-split), get(0), get(split));
            img.put_pixel(x, y, Rgba([r[0], c[1], b[2], c[3]]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid() -> RgbaImage {
        RgbaImage::from_pixel(40, 30, Rgba([200, 100, 50, 255]))
    }

    #[test]
    fn transitions_hide_then_show() {
        let img = solid();
        for t in Transition::ALL {
            if t == Transition::None {
                continue;
            }
            let full = transition(&img, t, 1.0, true);
            assert_eq!(full, img, "{t:?} at the end is the frame itself");
            let start = transition(&img, t, 0.0, true);
            let shown: u32 = start.pixels().map(|p| p[3] as u32).sum();
            let all: u32 = img.pixels().map(|p| p[3] as u32).sum();
            assert!(shown < all / 2, "{t:?} starts mostly hidden ({shown} of {all})");
        }
    }

    #[test]
    fn loops_are_seamless() {
        let img = solid();
        let fx = [LoopFx::Pulse, LoopFx::Wobble, LoopFx::Hop, LoopFx::Rainbow];
        // frame n would be frame 0 again
        let a = looped(&img, &fx, 0, 10, 50.0, 2, true);
        let b = looped(&img, &fx, 10, 10, 50.0, 2, true);
        assert_eq!(a, b);
    }
}

/// A contact sheet of every transition and loop, to look at by eye:
/// GIFKIT_SHEET=/path/sheet.png cargo test --release motion_sheet -- --ignored
#[cfg(test)]
#[test]
#[ignore]
fn motion_sheet() {
    let Ok(out) = std::env::var("GIFKIT_SHEET") else { return };
    let (w, h) = (96u32, 72u32);
    let src = RgbaImage::from_fn(w, h, |x, y| {
        let c = ((x / 12 + y / 12) % 2) as u8;
        Rgba([60 + 150 * c, 120, 200 - 120 * c, 255])
    });
    let steps = [0.1f32, 0.3, 0.5, 0.7, 0.9];
    let rows = Transition::ALL.len() - 1 + 6;
    let mut sheet = RgbaImage::from_pixel((w + 4) * steps.len() as u32, (h + 4) * rows as u32, Rgba([40, 40, 40, 255]));
    let mut row = 0;
    for t in Transition::ALL.iter().skip(1) {
        for (i, p) in steps.iter().enumerate() {
            let f = transition(&src, *t, *p, false);
            image::imageops::overlay(&mut sheet, &f, (i as u32 * (w + 4)) as i64, (row * (h + 4)) as i64);
        }
        row += 1;
    }
    for l in [LoopFx::Shake, LoopFx::Pulse, LoopFx::Hop, LoopFx::Wobble, LoopFx::Rainbow, LoopFx::Glitch] {
        for i in 0..steps.len() {
            let f = looped(&src, &[l], i * 2, 10, 100.0, 1, false);
            image::imageops::overlay(&mut sheet, &f, (i as u32 * (w + 4)) as i64, (row * (h + 4)) as i64);
        }
        row += 1;
    }
    sheet.save(out).unwrap();
}
