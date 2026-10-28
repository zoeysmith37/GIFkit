#![allow(dead_code)] // whole-animation helpers kept as a library API
//! Whole-animation operations: geometry and timing.

use crate::anim::{Animation, Frame};
use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

// ---------------------------------------------------------------- geometry

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ResizeFilter {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos3,
}

impl ResizeFilter {
    pub const ALL: [ResizeFilter; 4] = [ResizeFilter::Nearest, ResizeFilter::Bilinear, ResizeFilter::Bicubic, ResizeFilter::Lanczos3];
    pub fn label(self) -> &'static str {
        match self {
            ResizeFilter::Nearest => "Nearest (pixel art)",
            ResizeFilter::Bilinear => "Bilinear",
            ResizeFilter::Bicubic => "Bicubic",
            ResizeFilter::Lanczos3 => "Lanczos3 (sharp)",
        }
    }
    fn to_filter(self) -> FilterType {
        match self {
            ResizeFilter::Nearest => FilterType::Nearest,
            ResizeFilter::Bilinear => FilterType::Triangle,
            ResizeFilter::Bicubic => FilterType::CatmullRom,
            ResizeFilter::Lanczos3 => FilterType::Lanczos3,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum AspectMode {
    Stretch,
    CenterCrop,
    FitPad,
}

impl AspectMode {
    pub const ALL: [AspectMode; 3] = [AspectMode::CenterCrop, AspectMode::FitPad, AspectMode::Stretch];
    pub fn label(self) -> &'static str {
        match self {
            AspectMode::Stretch => "Stretch to fill",
            AspectMode::CenterCrop => "Center and crop to fit",
            AspectMode::FitPad => "Fit inside (transparent padding)",
        }
    }
}

pub fn resize_image(img: &RgbaImage, w: u32, h: u32, mode: AspectMode, filter: ResizeFilter) -> RgbaImage {
    let f = filter.to_filter();
    let (iw, ih) = img.dimensions();
    match mode {
        AspectMode::Stretch => imageops::resize(img, w, h, f),
        AspectMode::CenterCrop => {
            let s = (w as f32 / iw as f32).max(h as f32 / ih as f32);
            let nw = ((iw as f32 * s).round() as u32).max(w);
            let nh = ((ih as f32 * s).round() as u32).max(h);
            let r = imageops::resize(img, nw, nh, f);
            imageops::crop_imm(&r, (nw - w) / 2, (nh - h) / 2, w, h).to_image()
        }
        AspectMode::FitPad => {
            let s = (w as f32 / iw as f32).min(h as f32 / ih as f32);
            let nw = ((iw as f32 * s).round() as u32).clamp(1, w);
            let nh = ((ih as f32 * s).round() as u32).clamp(1, h);
            let r = imageops::resize(img, nw, nh, f);
            let mut out = RgbaImage::new(w, h);
            imageops::replace(&mut out, &r, ((w - nw) / 2) as i64, ((h - nh) / 2) as i64);
            out
        }
    }
}

pub fn resize(anim: &Animation, w: u32, h: u32, mode: AspectMode, filter: ResizeFilter) -> Animation {
    anim.map_images(|_, img| resize_image(img, w.max(1), h.max(1), mode, filter))
}

pub fn crop(anim: &Animation, x: u32, y: u32, w: u32, h: u32) -> Animation {
    let (aw, ah) = (anim.width(), anim.height());
    let x = x.min(aw.saturating_sub(1));
    let y = y.min(ah.saturating_sub(1));
    let w = w.clamp(1, aw - x);
    let h = h.clamp(1, ah - y);
    anim.map_images(|_, img| imageops::crop_imm(img, x, y, w, h).to_image())
}

/// Bounding box of pixels that differ from the "background" in any frame.
/// Background = transparent pixels if the animation has alpha, otherwise the
/// colour of the top-left pixel of the first frame.
pub fn auto_trim_rect(anim: &Animation, tolerance: u8) -> Option<(u32, u32, u32, u32)> {
    let first = anim.frames.first()?;
    let bg = *first.image.get().get_pixel(0, 0);
    let use_alpha = bg[3] < 255;
    let (w, h) = (anim.width(), anim.height());
    let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0, 0);
    let tol = tolerance as i32;
    for f in anim.frames.iter().filter(|f| f.enabled) {
        let img = f.image.get();
        for y in 0..h {
            for x in 0..w {
                let p = img.get_pixel(x, y);
                let is_bg = if use_alpha {
                    p[3] <= tolerance
                } else {
                    (0..3).all(|k| (p[k] as i32 - bg[k] as i32).abs() <= tol)
                };
                if !is_bg {
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
    }
    if minx == u32::MAX {
        return None;
    }
    Some((minx, miny, maxx - minx + 1, maxy - miny + 1))
}

pub fn flip_h(anim: &Animation) -> Animation {
    anim.map_images(|_, i| imageops::flip_horizontal(i))
}
pub fn flip_v(anim: &Animation) -> Animation {
    anim.map_images(|_, i| imageops::flip_vertical(i))
}
pub fn rotate90(anim: &Animation) -> Animation {
    anim.map_images(|_, i| imageops::rotate90(i))
}
pub fn rotate180(anim: &Animation) -> Animation {
    anim.map_images(|_, i| imageops::rotate180(i))
}
pub fn rotate270(anim: &Animation) -> Animation {
    anim.map_images(|_, i| imageops::rotate270(i))
}

/// Bilinear sample with transparent outside.
#[inline]
pub fn sample_bilinear(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return Rgba([0, 0, 0, 0]);
    }
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let get = |xx: i64, yy: i64| -> [f32; 4] {
        if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
            [0.0; 4]
        } else {
            let p = img.get_pixel(xx as u32, yy as u32);
            // premultiply
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        }
    };
    let a = get(x0, y0);
    let b = get(x0 + 1, y0);
    let c = get(x0, y0 + 1);
    let d = get(x0 + 1, y0 + 1);
    let mut o = [0f32; 4];
    for k in 0..4 {
        o[k] = a[k] * (1.0 - fx) * (1.0 - fy) + b[k] * fx * (1.0 - fy) + c[k] * (1.0 - fx) * fy + d[k] * fx * fy;
    }
    if o[3] <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let ia = 255.0 / o[3];
    Rgba([
        (o[0] * ia).round().clamp(0.0, 255.0) as u8,
        (o[1] * ia).round().clamp(0.0, 255.0) as u8,
        (o[2] * ia).round().clamp(0.0, 255.0) as u8,
        o[3].round().clamp(0.0, 255.0) as u8,
    ])
}

/// Sample with edge clamping (no transparency introduced).
#[inline]
pub fn sample_clamped(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    sample_bilinear(img, x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32))
}

/// Rotate by arbitrary degrees (clockwise), expanding the canvas. Fills with `bg`.
pub fn rotate_any_image(img: &RgbaImage, degrees: f32, bg: Rgba<u8>, expand: bool) -> RgbaImage {
    let r = degrees.to_radians();
    let (s, c) = r.sin_cos();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (nw, nh) = if expand {
        ((w * c.abs() + h * s.abs()).ceil() as u32, (w * s.abs() + h * c.abs()).ceil() as u32)
    } else {
        (img.width(), img.height())
    };
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ncx, ncy) = (nw as f32 / 2.0, nh as f32 / 2.0);
    let mut out = RgbaImage::from_pixel(nw, nh, bg);
    for y in 0..nh {
        for x in 0..nw {
            let dx = x as f32 + 0.5 - ncx;
            let dy = y as f32 + 0.5 - ncy;
            // inverse rotation
            let sx = dx * c + dy * s + cx - 0.5;
            let sy = -dx * s + dy * c + cy - 0.5;
            let p = sample_bilinear(img, sx, sy);
            if p[3] > 0 {
                let o = out.get_pixel_mut(x, y);
                *o = blend_over(*o, p);
            }
        }
    }
    out
}

pub fn rotate_any(anim: &Animation, degrees: f32, bg: Rgba<u8>, expand: bool) -> Animation {
    anim.map_images(|_, i| rotate_any_image(i, degrees, bg, expand))
}

/// Porter-Duff "over".
#[inline]
pub fn blend_over(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let sa = src[3] as f32 / 255.0;
    if sa >= 1.0 {
        return src;
    }
    if sa <= 0.0 {
        return dst;
    }
    let da = dst[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    let mut o = [0u8; 4];
    for k in 0..3 {
        let v = (src[k] as f32 * sa + dst[k] as f32 * da * (1.0 - sa)) / oa;
        o[k] = v.round().clamp(0.0, 255.0) as u8;
    }
    o[3] = (oa * 255.0).round() as u8;
    Rgba(o)
}

/// Blend `src` onto `dst` at (x, y) with global opacity.
pub fn overlay_with_opacity(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64, opacity: f32) {
    let (dw, dh) = dst.dimensions();
    for sy in 0..src.height() {
        let ty = y + sy as i64;
        if ty < 0 || ty >= dh as i64 {
            continue;
        }
        for sx in 0..src.width() {
            let tx = x + sx as i64;
            if tx < 0 || tx >= dw as i64 {
                continue;
            }
            let mut p = *src.get_pixel(sx, sy);
            p[3] = (p[3] as f32 * opacity).round() as u8;
            let d = dst.get_pixel_mut(tx as u32, ty as u32);
            *d = blend_over(*d, p);
        }
    }
}

// ---------------------------------------------------------------- timing

pub fn reverse(anim: &Animation) -> Animation {
    let mut a = anim.clone();
    a.frames.reverse();
    a
}

/// Play forward then backward (without duplicating the end frames).
pub fn boomerang(anim: &Animation) -> Animation {
    let mut a = anim.clone();
    let n = anim.frames.len();
    if n > 2 {
        for i in (1..n - 1).rev() {
            a.frames.push(anim.frames[i].clone());
        }
    }
    a
}

/// Scale speed. `percent` = 200 → twice as fast.
pub fn change_speed(anim: &Animation, percent: f32) -> Animation {
    let mut a = anim.clone();
    let p = percent.max(1.0) / 100.0;
    for f in &mut a.frames {
        f.delay_cs = ((f.delay_cs as f32 / p).round() as u16).max(1);
    }
    a
}

pub fn set_all_delays(anim: &Animation, delay: u16) -> Animation {
    let mut a = anim.clone();
    for f in &mut a.frames {
        f.delay_cs = delay;
    }
    a
}

/// Keep frames `start..=end` (0-based, inclusive).
pub fn cut_frames(anim: &Animation, start: usize, end: usize) -> Animation {
    let mut a = anim.clone();
    let end = end.min(anim.len().saturating_sub(1));
    let start = start.min(end);
    a.frames = anim.frames[start..=end].to_vec();
    a
}

/// Convert a time range in seconds to an inclusive frame range.
pub fn time_to_frame_range(anim: &Animation, t0: f32, t1: f32) -> (usize, usize) {
    let mut t = 0.0;
    let (mut s, mut e) = (None, 0);
    for (i, f) in anim.frames.iter().enumerate() {
        let d = crate::anim::effective_delay(f.delay_cs) as f32 / 100.0;
        let mid = t + d * 0.5;
        if mid >= t0 && s.is_none() {
            s = Some(i);
        }
        if mid <= t1 {
            e = i;
        }
        t += d;
    }
    let s = s.unwrap_or(0);
    (s, e.max(s))
}

/// Remove every N-th frame (decimate) and give its time to the previous frame.
pub fn drop_every_nth(anim: &Animation, n: usize) -> Animation {
    if n < 2 {
        return anim.clone();
    }
    let mut out: Vec<Frame> = Vec::new();
    for (i, f) in anim.frames.iter().enumerate() {
        if (i + 1) % n == 0 {
            if let Some(last) = out.last_mut() {
                last.delay_cs = last.delay_cs.saturating_add(f.delay_cs);
            }
        } else {
            out.push(f.clone());
        }
    }
    Animation { frames: out, loop_count: anim.loop_count }
}

pub fn lerp_image(a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
    let mut out = a.clone();
    for (o, (pa, pb)) in out.pixels_mut().zip(a.pixels().zip(b.pixels())) {
        for k in 0..4 {
            o[k] = (pa[k] as f32 * (1.0 - t) + pb[k] as f32 * t).round() as u8;
        }
    }
    out
}

/// Insert `steps` blended frames between each pair of frames. The total
/// duration of each original frame is split between it and its in-betweens.
pub fn crossfade(anim: &Animation, steps: usize, wrap: bool) -> Animation {
    inbetween(anim, steps, wrap, false, None)
}

/// `steps` new frames between each pair. With `follow`, the picture is
/// moved along with what moves in it (block motion) before the two are
/// mixed, so things slide instead of fading through each other. Each
/// frame's time is shared with its in-betweens, so the length stays the
/// same. Skipped frames and gaps are left as they are.
pub fn inbetween(anim: &Animation, steps: usize, wrap: bool, follow: bool, progress: Option<&std::sync::atomic::AtomicUsize>) -> Animation {
    inbetween_counted(anim, steps, wrap, follow, crate::interp::Unsure::Blur, progress).0
}

/// Where frame `i` shows something that stays put on screen over the
/// frames around it (`reach` either way): burned-in text, a logo, a border.
/// Either its colour stays the same (with detail near, as flat areas can't
/// be seen to move anyway), or its outline does (see-through text over a
/// moving picture changes colour but not shape). Full size. None when
/// there's nothing, or too few frames to tell.
pub fn still_mask(anim: &Animation, i: usize, reach: usize) -> Option<Vec<bool>> {
    use rayon::prelude::*;
    let n = anim.len();
    let idx: Vec<usize> = (i.saturating_sub(reach)..(i + reach + 1).min(n)).filter(|&k| anim.frames[k].enabled && !anim.frames[k].blank).collect();
    if idx.len() < 5 {
        return None;
    }
    let imgs: Vec<_> = idx.iter().map(|&k| anim.frames[k].image.get()).collect();
    let (w, h) = imgs[0].dimensions();
    if imgs.iter().any(|m| m.dimensions() != (w, h)) {
        return None;
    }
    let (wu, hu) = (w as usize, h as usize);
    // brightness, and its detail (less the average around it)
    let lums: Vec<Vec<f32>> = imgs.par_iter().map(|m| m.pixels().map(|p| (p[0] as f32 * 0.3 + p[1] as f32 * 0.59 + p[2] as f32 * 0.11) * p[3] as f32 / 255.0).collect()).collect();
    let details: Vec<Vec<f32>> = lums
        .par_iter()
        .map(|l| {
            let avg = box_mean(l, wu, hu, 3);
            l.iter().zip(avg).map(|(a, b)| a - b).collect()
        })
        .collect();
    let keep: Vec<bool> = (0..wu * hu)
        .into_par_iter()
        .map(|j| {
            // the same colour throughout (allowing for a video's noise)
            let first = imgs[0].as_raw();
            let same = imgs[1..].iter().all(|m| {
                let r = m.as_raw();
                (0..4).all(|c| (r[j * 4 + c] as i32 - first[j * 4 + c] as i32).abs() <= 24)
            });
            let edgy = details[0][j].abs() > 12.0;
            // or the same outline: strong detail, the same way, throughout
            let d0 = details[0][j];
            let shape = d0.abs() > 20.0 && details[1..].iter().all(|d| d[j] * d0.signum() > 14.0 && (d[j] - d0).abs() < 35.0);
            (same && edgy) || shape
        })
        .collect();
    if !keep.iter().any(|&b| b) {
        return None;
    }
    // text and logos are dense: lone specks (a leaf that happened not to
    // move) go
    let ones: Vec<f32> = keep.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let dense = box_mean(&ones, wu, hu, 4);
    let keep: Vec<f32> = ones.iter().zip(&dense).map(|(&k, &d)| if k > 0.0 && d > 0.15 { 1.0 } else { 0.0 }).collect();
    // a little wider, for soft edges
    let spread = box_mean(&keep, wu, hu, 2);
    Some(spread.into_iter().map(|v| v > 0.01).collect())
}

/// Each value the average of those `r` out each way (a box, rows then columns).
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |v: &[f32], horiz: bool| -> Vec<f32> {
        let (len, lines) = if horiz { (w, h) } else { (h, w) };
        let mut out = vec![0.0f32; w * h];
        for line in 0..lines {
            let at = |k: usize| if horiz { line * w + k } else { k * w + line };
            let mut sum = 0.0f32;
            let mut cnt = 0usize;
            for k in 0..r.min(len) {
                sum += v[at(k)];
                cnt += 1;
            }
            for k in 0..len {
                if k + r < len {
                    sum += v[at(k + r)];
                    cnt += 1;
                }
                if k > r {
                    sum -= v[at(k - r - 1)];
                    cnt -= 1;
                }
                out[at(k)] = sum / cnt as f32;
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// [`inbetween`], also saying how many frames each frame became.
///
/// Following motion, every pair gets a first look before any is finished,
/// so each can borrow from the pairs either side (things they saw moving,
/// the path they took) and from what stays still around it. `progress`
/// then counts each frame twice: once looked at, once finished.
pub fn inbetween_counted(anim: &Animation, steps: usize, wrap: bool, follow: bool, unsure: crate::interp::Unsure, progress: Option<&std::sync::atomic::AtomicUsize>) -> (Animation, Vec<usize>) {
    use rayon::prelude::*;
    if steps == 0 || anim.len() < 2 {
        return (anim.clone(), vec![1; anim.len()]);
    }
    let n = anim.len();
    // each frame and the next shown one it leads into
    let next_of = |i: usize| -> Option<usize> {
        let f = &anim.frames[i];
        if !f.enabled || f.blank {
            return None;
        }
        let mut j = i + 1;
        loop {
            if j >= n {
                if !wrap {
                    return None;
                }
                j = 0;
            }
            if j == i {
                return None;
            }
            let g = &anim.frames[j];
            if g.enabled && !g.blank {
                return (j > i || wrap).then_some(j);
            }
            if g.enabled && g.blank {
                // a gap comes next: nothing to blend into
                return None;
            }
            j += 1;
        }
    };
    let tick = || {
        if let Some(p) = progress {
            p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    };
    // first looks (None at a cut, or not following)
    let looks: Vec<Option<crate::interp::Glance>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let g = if follow { next_of(i).and_then(|j| crate::interp::Glance::new(&anim.frames[i].image.get(), &anim.frames[j].image.get())) } else { None };
            if follow {
                tick();
            }
            g
        })
        .collect();
    // the pair before each (the one leading into it)
    let mut before: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        if let Some(j) = next_of(i) {
            before[j] = Some(i);
        }
    }
    let made: Vec<Vec<Frame>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let f = &anim.frames[i];
            let Some(j) = next_of(i) else {
                // stays as it is (its time and place kept)
                tick();
                return vec![f.clone()];
            };
            let parts = steps + 1;
            // split the delay evenly, all of it
            let d = f.delay_cs as usize;
            // (2 cs at least: shorter plays slow in browsers, so it runs a little long instead)
            let share = |k: usize| ((d * (k + 1) + parts / 2) / parts - (d * k + parts / 2) / parts).max(2) as u16;
            let (a, b) = (f.image.get(), anim.frames[j].image.get());
            // following motion: None at a cut, and then the nearer frame is
            // held, not smeared
            let pair = follow.then(|| {
                looks[i].as_ref().map(|g| {
                    let prev = before[i].and_then(|p| looks[p].as_ref());
                    let next = looks[j].as_ref();
                    crate::interp::Pair::finish(&a, &b, g, prev, next, still_mask(anim, i, 4))
                })
            });
            let mut v = vec![Frame { delay_cs: share(0), ..f.clone() }];
            for s in 1..=steps {
                let t = s as f32 / parts as f32;
                let img = match &pair {
                    Some(Some(p)) => p.render(&a, &b, t, 1.0 / parts as f32, unsure),
                    Some(None) => (if t < 0.5 { &a } else { &b }).as_ref().clone(),
                    None => lerp_image(&a, &b, t),
                };
                v.push(Frame { image: crate::pixels::Pixels::new(img), delay_cs: share(s), enabled: true, cut: false, blank: false, trim: 0 });
            }
            tick();
            v
        })
        .collect();
    let counts = made.iter().map(|v| v.len()).collect();
    (Animation { frames: made.into_iter().flatten().collect(), loop_count: anim.loop_count }, counts)
}

/// Make the loop seamless: the last `n` frames fade into the first `n`, and
/// the animation ends where the fade began, so playing it round shows no jump.
pub fn loop_crossfade(anim: &Animation, n: usize) -> Animation {
    let l = anim.len();
    let n = n.min(l / 2);
    if n == 0 {
        return anim.clone();
    }
    let mut out: Vec<Frame> = Vec::with_capacity(l - n);
    for i in 0..l - n {
        let f = &anim.frames[i];
        if i < n {
            // from the end's frame towards the start's
            let end = &anim.frames[l - n + i];
            let t = (i as f32 + 0.5) / n as f32;
            out.push(Frame { image: crate::pixels::Pixels::new(lerp_image(&end.image.get(), &f.image.get(), t)), delay_cs: f.delay_cs, enabled: f.enabled, cut: f.cut, blank: false, trim: 0 });
        } else {
            out.push(f.clone());
        }
    }
    Animation { frames: out, loop_count: anim.loop_count }
}

/// The best place to loop: two frames at least `min_len` apart that look
/// most alike, as (first, the one that matches it, how different 0..1).
/// Playing first..match-1 round then shows the least jump.
pub fn find_loop(frames: &[std::sync::Arc<RgbaImage>], min_len: usize) -> Option<(usize, usize, f32)> {
    use rayon::prelude::*;
    let n = frames.len();
    if n < 3 {
        return None;
    }
    // small copies to compare
    const S: u32 = 24;
    let small: Vec<Vec<f32>> = frames
        .par_iter()
        .map(|f| image::imageops::resize(&**f, S, S, image::imageops::FilterType::Triangle).pixels().flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        }).collect())
        .collect();
    let min_len = min_len.clamp(2, n - 1);
    // long animations: look at every so many frames
    let step = (n / 400).max(1);
    let best = (0..n)
        .step_by(step)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|&i| {
            (i + min_len..n)
                .step_by(step)
                .map(|j| {
                    let d: f32 = small[i].iter().zip(&small[j]).map(|(a, b)| (a - b).abs()).sum::<f32>() / (small[i].len() as f32 * 255.0);
                    // a little in favour of longer loops
                    (d - (j - i) as f32 * 1e-6, i, j, d)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    Some((best.1, best.2, best.3))
}

/// Composite each frame over the first frame (useful for transparent sprite
/// animations that should keep a static background).
pub fn first_frame_as_background(anim: &Animation) -> Animation {
    let Some(bg) = anim.frames.first().map(|f| f.image.get()) else { return anim.clone() };
    anim.map_images(|i, img| {
        if i == 0 {
            return img.clone();
        }
        let mut b = (*bg).clone();
        imageops::overlay(&mut b, img, 0, 0);
        b
    })
}

/// Add frames from still images, fitted to the animation canvas.
pub fn append_images(anim: &Animation, imgs: Vec<RgbaImage>, delay: u16) -> Animation {
    let mut a = anim.clone();
    let (w, h) = (anim.width(), anim.height());
    for img in imgs {
        let img = if w == 0 { img } else { crate::io::fit_onto_canvas(&img, w, h) };
        a.frames.push(Frame::new(img, delay));
    }
    a
}


/// Squared distance from each cell to the nearest `true` cell (exact, after
/// Felzenszwalb and Huttenlocher). Cells far from any are a large number.
pub fn distance_sq(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    use rayon::prelude::*;
    const INF: f32 = 1e20;
    // one dimension: lower envelope of parabolas
    fn pass(f: &[f32], out: &mut [f32], v: &mut Vec<usize>, z: &mut Vec<f64>) {
        let n = f.len();
        if n == 0 {
            return;
        }
        v.clear();
        z.clear();
        v.push(0);
        z.push(f64::NEG_INFINITY);
        z.push(f64::INFINITY);
        let fx = |q: usize| f[q] as f64 + (q * q) as f64;
        for q in 1..n {
            let mut k = v.len() - 1;
            let mut s = (fx(q) - fx(v[k])) / (2.0 * (q as f64 - v[k] as f64));
            while s <= z[k] {
                v.pop();
                z.pop();
                k -= 1;
                s = (fx(q) - fx(v[k])) / (2.0 * (q as f64 - v[k] as f64));
            }
            v.push(q);
            z[k + 1] = s;
            z.push(f64::INFINITY);
        }
        let mut k = 0;
        for (q, o) in out.iter_mut().enumerate() {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let d = q as f64 - v[k] as f64;
            *o = (d * d + f[v[k]] as f64).min(INF as f64) as f32;
        }
    }
    let mut g: Vec<f32> = inside.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    // columns
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let f: Vec<f32> = (0..h).map(|y| g[y * w + x]).collect();
            let mut o = vec![0.0; h];
            pass(&f, &mut o, &mut Vec::new(), &mut Vec::new());
            o
        })
        .collect();
    for (x, c) in cols.iter().enumerate() {
        for y in 0..h {
            g[y * w + x] = c[y];
        }
    }
    // rows
    g.par_chunks_mut(w.max(1)).for_each(|row| {
        let f = row.to_vec();
        pass(&f, row, &mut Vec::new(), &mut Vec::new());
    });
    g
}

/// For each cell: is any `true` cell within the square of radius `r` around it.
pub fn near_any(set: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut horiz = vec![false; w * h];
    for y in 0..h {
        let row = &set[y * w..(y + 1) * w];
        let mut pre = vec![0u32; w + 1];
        for x in 0..w {
            pre[x + 1] = pre[x] + row[x] as u32;
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            horiz[y * w + x] = pre[b] > pre[a];
        }
    }
    let mut out = vec![false; w * h];
    let mut pre = vec![0u32; h + 1];
    for x in 0..w {
        for y in 0..h {
            pre[y + 1] = pre[y] + horiz[y * w + x] as u32;
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = pre[b] > pre[a];
        }
    }
    out
}
