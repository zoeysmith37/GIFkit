//! Effects in the spirit of the classic Photo Booth app: colour looks
//! (glow, comic, pencil, thermal, x-ray, pop art) and funhouse distortions
//! (bulge, dent, twirl, squeeze, stretch, mirror, light tunnel, fish eye,
//! kaleidoscope). One is active at a time, with a strength and a centre.

use crate::effects::warp;
use image::{imageops, Rgba, RgbaImage};
use rayon::prelude::*;
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum BoothFx {
    #[default]
    None,
    Glow,
    Comic,
    Pencil,
    Thermal,
    XRay,
    PopArt,
    Bulge,
    Dent,
    Twirl,
    Squeeze,
    Stretch,
    Mirror,
    Tunnel,
    FishEye,
    Kaleidoscope,
}

impl BoothFx {
    pub const LOOKS: [BoothFx; 6] = [BoothFx::Glow, BoothFx::Comic, BoothFx::Pencil, BoothFx::Thermal, BoothFx::XRay, BoothFx::PopArt];
    pub const DISTORT: [BoothFx; 9] = [BoothFx::Bulge, BoothFx::Dent, BoothFx::Twirl, BoothFx::Squeeze, BoothFx::Stretch, BoothFx::Mirror, BoothFx::Tunnel, BoothFx::FishEye, BoothFx::Kaleidoscope];

    pub fn label(self) -> &'static str {
        match self {
            BoothFx::None => "None",
            BoothFx::Glow => "Glow",
            BoothFx::Comic => "Comic Book",
            BoothFx::Pencil => "Colour Pencil",
            BoothFx::Thermal => "Thermal Camera",
            BoothFx::XRay => "X-Ray",
            BoothFx::PopArt => "Pop Art",
            BoothFx::Bulge => "Bulge",
            BoothFx::Dent => "Dent",
            BoothFx::Twirl => "Twirl",
            BoothFx::Squeeze => "Squeeze",
            BoothFx::Stretch => "Stretch",
            BoothFx::Mirror => "Mirror",
            BoothFx::Tunnel => "Light Tunnel",
            BoothFx::FishEye => "Fish Eye",
            BoothFx::Kaleidoscope => "Kaleidoscope",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            BoothFx::None => "",
            BoothFx::Glow => "Soft dreamy bloom",
            BoothFx::Comic => "Ink outlines and flat colours",
            BoothFx::Pencil => "Hatched pencil sketch",
            BoothFx::Thermal => "Heat-camera colours by brightness",
            BoothFx::XRay => "Inverted and tinted blue",
            BoothFx::PopArt => "Four bold colour panels",
            BoothFx::Bulge => "Puffs out the middle",
            BoothFx::Dent => "Pinches in the middle",
            BoothFx::Twirl => "Spins around the middle",
            BoothFx::Squeeze => "Narrows the middle",
            BoothFx::Stretch => "Widens the middle",
            BoothFx::Mirror => "Left half reflected onto the right",
            BoothFx::Tunnel => "Edges streak out from a circle",
            BoothFx::FishEye => "Round wide-angle lens",
            BoothFx::Kaleidoscope => "Mirrored slices round the centre",
        }
    }

    /// Uses the centre point.
    pub fn has_centre(self) -> bool {
        matches!(self, BoothFx::Bulge | BoothFx::Dent | BoothFx::Twirl | BoothFx::Squeeze | BoothFx::Stretch | BoothFx::Mirror | BoothFx::Tunnel | BoothFx::FishEye | BoothFx::Kaleidoscope)
    }
}

/// One stacked look or distortion with its own settings. Several can be on
/// at once (in the order of `LOOKS` and `DISTORT`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BoothItem {
    pub fx: BoothFx,
    /// 0 and up; distortions keep getting stronger past 100
    pub amount: f32,
    /// centre, % of the frame
    pub cx: f32,
    pub cy: f32,
    /// the other way round (pinch instead of bulge, the other spin…)
    pub reverse: bool,
}

impl Default for BoothItem {
    fn default() -> Self {
        BoothItem { fx: BoothFx::None, amount: 50.0, cx: 50.0, cy: 50.0, reverse: false }
    }
}

impl BoothFx {
    pub fn reverse_tip(self) -> &'static str {
        match self {
            BoothFx::Bulge => "Pinch in instead",
            BoothFx::Dent => "Puff out instead",
            BoothFx::Twirl => "Spin the other way",
            BoothFx::Squeeze => "Widen instead",
            BoothFx::Stretch => "Narrow instead",
            BoothFx::Mirror => "Right half onto the left",
            BoothFx::Tunnel => "Streak in toward the middle",
            BoothFx::FishEye => "Shrink the middle",
            BoothFx::Kaleidoscope => "Turn the slices",
            _ => "",
        }
    }
}

/// Where a distortion reads each output pixel from: the shader's
/// `warp_from` (kinds 3–7, 9), so CPU and graphics card agree.
pub fn warp_point(kind: i32, a: [f32; 4], b: [f32; 4], x: f32, y: f32) -> (f32, f32) {
    let (dx, dy) = (x - a[0], y - a[1]);
    let d = (dx * dx + dy * dy).sqrt();
    match kind {
        3 => {
            if d >= a[2] || d < 1e-3 {
                return (x, y);
            }
            let t = d / a[2];
            let s = t.powf(1.0 + a[3]) / t;
            (a[0] + dx * s, a[1] + dy * s)
        }
        4 => {
            if d >= a[2] {
                return (x, y);
            }
            let f = 1.0 - d / a[2];
            let t = a[3] * f * f;
            let (s, c) = t.sin_cos();
            (a[0] + dx * c - dy * s, a[1] + dx * s + dy * c)
        }
        5 => {
            let span = if dx < 0.0 { a[0].max(1.0) } else { (a[2] - a[0]).max(1.0) };
            let t = (dx.abs() / span).min(1.0);
            (a[0] + dx.signum() * t.powf(a[1]) * span, y)
        }
        6 => {
            if d <= a[2] {
                return (x, y);
            }
            let s = (a[2] + (d - a[2]) * a[3]) / d;
            (a[0] + dx * s, a[1] + dy * s)
        }
        9 => {
            // a tunnel the other way: the inside streaks toward the middle
            if d >= a[2] || d < 1e-3 {
                return (x, y);
            }
            let s = (a[2] - (a[2] - d) * a[3]) / d;
            (a[0] + dx * s, a[1] + dy * s)
        }
        _ => {
            // kaleidoscope: cx cy seg reach; b[0] turns the slices
            let seg = a[2];
            let ang = dy.atan2(dx) + PI + b[0];
            let mut t = ang.rem_euclid(seg);
            if t > seg / 2.0 {
                t = seg - t;
            }
            let t = t - seg / 4.0;
            let m = (d / a[3]).rem_euclid(2.0);
            let dd = if m > 1.0 { 2.0 - m } else { m } * a[3];
            (a[0] + dd * t.cos(), a[1] + dd * t.sin())
        }
    }
}

/// A distortion's warp kind and numbers for a `w`×`h` frame (None for the
/// looks and for Mirror, which copies pixels instead).
pub fn distort_params(it: &BoothItem, w: u32, h: u32) -> Option<(i32, [f32; 4], [f32; 4])> {
    let a = (it.amount / 100.0).max(0.0);
    let (wf, hf) = (w as f32, h as f32);
    let (cx, cy) = (it.cx / 100.0 * wf, it.cy / 100.0 * hf);
    let rad = (w.min(h) as f32 / 2.0).max(1.0);
    let z = [0.0; 4];
    // pulling in (a dent) stays short of folding over, however strong
    let pinch = |a: f32| -(1.0 - 1.0 / (1.0 + a * 0.8)) * 0.95;
    let fx = match (it.fx, it.reverse) {
        (BoothFx::Bulge, true) => BoothFx::Dent,
        (BoothFx::Dent, true) => BoothFx::Bulge,
        (BoothFx::Squeeze, true) => BoothFx::Stretch,
        (BoothFx::Stretch, true) => BoothFx::Squeeze,
        (f, _) => f,
    };
    Some(match fx {
        BoothFx::Bulge => (3, [cx, cy, rad * 1.1, a * 1.65], z),
        BoothFx::Dent => (3, [cx, cy, rad * 1.1, pinch(a)], z),
        BoothFx::FishEye => (3, [cx, cy, w.max(h) as f32 * 0.5, if it.reverse { pinch(a) } else { a * 1.6 }], z),
        BoothFx::Twirl => (4, [cx, cy, rad * 1.2, (a * 540.0).to_radians() * if it.reverse { -1.0 } else { 1.0 }], z),
        BoothFx::Squeeze => (5, [cx, 1.0 / (1.0 + a * 2.0), wf, 0.0], z),
        BoothFx::Stretch => (5, [cx, 1.0 + a * 2.0, wf, 0.0], z),
        BoothFx::Tunnel => {
            let r0 = rad * (0.75 - 0.4 * a).max(0.05);
            let squash = (1.0 - 0.96 * a).max(0.01);
            (if it.reverse { 9 } else { 6 }, [cx, cy, r0, squash], z)
        }
        BoothFx::Kaleidoscope => {
            let n = 4 + (a * 8.0).round() as i32;
            let seg = TAU / n as f32;
            let reach = ((wf - cx).max(cx) * 0.95).max(1.0);
            (7, [cx, cy, seg, reach], [if it.reverse { seg / 2.0 } else { 0.0 }, 0.0, 0.0, 0.0])
        }
        _ => return None,
    })
}

/// Apply one stacked item.
pub fn apply_item(src: &RgbaImage, it: &BoothItem) -> RgbaImage {
    let (w, h) = src.dimensions();
    if w == 0 || h == 0 || it.fx == BoothFx::None {
        return src.clone();
    }
    if BoothFx::LOOKS.contains(&it.fx) {
        return apply(src, it.fx, it.amount.min(100.0), it.cx, it.cy);
    }
    if it.fx == BoothFx::Mirror {
        let cx = it.cx / 100.0 * w as f32;
        return mirror(src, cx, it.reverse);
    }
    let Some((kind, a, b)) = distort_params(it, w, h) else { return src.clone() };
    let mut out = warp(src, true, |x, y| warp_point(kind, a, b, x, y));
    if it.fx == BoothFx::FishEye {
        fish_mask(&mut out, a[0], a[1], a[2], (it.amount / 100.0).clamp(0.0, 1.0));
    }
    out
}

/// The fish eye's round lens: dark outside the circle, soft at the edge.
fn fish_mask(out: &mut RgbaImage, cx: f32, cy: f32, r0: f32, a: f32) {
    let w = out.width();
    out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let m = lerp(1.0, ((r0 - d) / 1.5).clamp(0.0, 1.0), a);
        for c in 0..3 {
            o[c] = c8(o[c] as f32 * m);
        }
    });
}

/// One half reflected onto the other (`reverse`: the right onto the left).
fn mirror(src: &RgbaImage, cx: f32, reverse: bool) -> RgbaImage {
    let w = src.width();
    let mut out = src.clone();
    let mx = cx.clamp(1.0, (w as f32 - 1.0).max(1.0));
    out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let x = (i as u32 % w) as f32;
        let y = i as u32 / w;
        if (!reverse && x >= mx) || (reverse && x < mx) {
            let sx = (2.0 * mx - x - 1.0).clamp(0.0, w as f32 - 1.0) as u32;
            o.copy_from_slice(&src.get_pixel(sx, y).0);
        }
    });
    out
}

fn lum(p: &Rgba<u8>) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn c8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Colour from a gradient of (position, colour) stops.
fn ramp(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let k = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
            return [lerp(a.1[0], b.1[0], k), lerp(a.1[1], b.1[1], k), lerp(a.1[2], b.1[2], k)];
        }
    }
    stops.last().map(|s| s.1).unwrap_or([0.0; 3])
}

/// Edge strength 0..1 from a Sobel filter on brightness.
fn edges(img: &RgbaImage) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let l: Vec<f32> = img.pixels().map(lum).collect();
    let at = |x: i64, y: i64| l[(y.clamp(0, h as i64 - 1) as u32 * w + x.clamp(0, w as i64 - 1) as u32) as usize];
    (0..(w * h) as usize)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i as u32 % w) as i64, (i as u32 / w) as i64);
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x - 1, y) - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x, y - 1) - at(x + 1, y - 1);
            (gx * gx + gy * gy).sqrt()
        })
        .collect()
}

/// Apply `fx`. `amount` is 0..=100, (cx, cy) the centre as 0..=100 % of the frame.
pub fn apply(src: &RgbaImage, fx: BoothFx, amount: f32, cx: f32, cy: f32) -> RgbaImage {
    let (w, h) = src.dimensions();
    if w == 0 || h == 0 || fx == BoothFx::None {
        return src.clone();
    }
    let a = (amount / 100.0).clamp(0.0, 1.0);
    let (cx, cy) = (cx / 100.0 * w as f32, cy / 100.0 * h as f32);
    let rad = (w.min(h) as f32 / 2.0).max(1.0);
    match fx {
        BoothFx::None => src.clone(),
        BoothFx::Glow => {
            // soft focus: a wide blur screened over a slightly brighter image
            // blur a quarter-size copy: same look, a fraction of the work
            let (sw, sh) = ((w / 2).max(1), (h / 2).max(1));
            let small = imageops::resize(src, sw, sh, imageops::FilterType::Triangle);
            let small = imageops::blur(&small, (sw.min(sh) as f32 * 0.035).max(0.5));
            let blur = imageops::resize(&small, w, h, imageops::FilterType::Triangle);
            let mut out = src.clone();
            out.par_chunks_mut(4).zip(blur.par_chunks(4)).for_each(|(o, b)| {
                for c in 0..3 {
                    let s = o[c] as f32 / 255.0 * (1.0 + 0.06 * a);
                    let g = b[c] as f32 / 255.0;
                    let screen = 1.0 - (1.0 - s) * (1.0 - g);
                    o[c] = c8(lerp(s, screen, a) * 255.0);
                }
            });
            out
        }
        BoothFx::Comic => {
            // smoothed flat colours with bold ink lines
            let soft = imageops::blur(src, 1.2);
            let e = edges(&soft);
            let levels = 4.0;
            let thresh = lerp(0.9, 0.25, a);
            let mut out = soft.clone();
            out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                if e[i] > thresh {
                    o[0] = 20;
                    o[1] = 20;
                    o[2] = 20;
                    return;
                }
                // flatten brightness into a few steps but keep each pixel's hue
                let (r, g, b) = (o[0] as f32, o[1] as f32, o[2] as f32);
                let v = r.max(g).max(b).max(1.0);
                let q = ((v / 255.0 * (levels - 1.0)).round() / (levels - 1.0)).max(0.12);
                let k = q * 255.0 / v;
                let mean = (r + g + b) / 3.0;
                for (c, ch) in [r, g, b].into_iter().enumerate() {
                    // a little extra saturation for the printed look
                    let sat = mean + (ch - mean) * 1.35;
                    o[c] = c8(sat * k);
                }
            });
            out
        }
        BoothFx::Pencil => {
            // pale paper, dark outlines and diagonal hatching where it's dark
            let e = edges(&imageops::blur(src, 0.8));
            let period = (w.min(h) as f32 / 60.0).clamp(3.0, 9.0);
            let mut out = src.clone();
            out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
                let l = (0.299 * o[0] as f32 + 0.587 * o[1] as f32 + 0.114 * o[2] as f32) / 255.0;
                let dark = (1.0 - l).powf(1.3);
                // two hatch directions, the second only in the darkest areas
                let line = |u: f32, jitter: f32| {
                    let t = ((u + jitter) / period).fract();
                    (1.0 - (t - 0.5).abs() * 4.0).clamp(0.0, 1.0)
                };
                let j = ((x * 12.9898 + y * 78.233).sin() * 43758.545).fract() * 0.8;
                let mut ink = line(x + y, j) * dark;
                if dark > 0.55 {
                    ink = ink.max(line(x - y, j) * (dark - 0.45));
                }
                let outline = (e[i] * 1.6).clamp(0.0, 1.0);
                let shade = 1.0 - (ink * (0.45 + 0.4 * a)).max(outline * 0.9);
                for c in 0..3 {
                    let tint = lerp(250.0, o[c] as f32, 0.25 + 0.35 * a);
                    o[c] = c8(tint * shade);
                }
            });
            out
        }
        BoothFx::Thermal => {
            let stops = [(0.0, [10.0, 0.0, 40.0]), (0.2, [40.0, 0.0, 160.0]), (0.4, [170.0, 0.0, 170.0]), (0.6, [240.0, 30.0, 30.0]), (0.78, [255.0, 150.0, 0.0]), (0.92, [255.0, 240.0, 60.0]), (1.0, [255.0, 255.0, 255.0])];
            let mut out = src.clone();
            out.par_chunks_mut(4).for_each(|o| {
                let l = (0.299 * o[0] as f32 + 0.587 * o[1] as f32 + 0.114 * o[2] as f32) / 255.0;
                let t = ((l - 0.5) * (0.8 + a * 0.8) + 0.5).clamp(0.0, 1.0);
                let c = ramp(&stops, t);
                o[0] = c8(c[0]);
                o[1] = c8(c[1]);
                o[2] = c8(c[2]);
            });
            out
        }
        BoothFx::XRay => {
            let mut out = src.clone();
            out.par_chunks_mut(4).for_each(|o| {
                let l = (0.299 * o[0] as f32 + 0.587 * o[1] as f32 + 0.114 * o[2] as f32) / 255.0;
                let v = (1.0 - l).powf(lerp(1.0, 0.6, a));
                o[0] = c8(v * lerp(235.0, 190.0, a));
                o[1] = c8(v * 245.0);
                o[2] = c8(v * 250.0 + (1.0 - v) * 18.0);
            });
            out
        }
        BoothFx::PopArt => {
            // four small copies, each mapped to a three-colour palette
            let pals: [[[f32; 3]; 3]; 4] = [
                [[230.0, 60.0, 30.0], [120.0, 150.0, 140.0], [250.0, 230.0, 40.0]],
                [[70.0, 170.0, 60.0], [230.0, 140.0, 210.0], [245.0, 245.0, 190.0]],
                [[30.0, 35.0, 80.0], [230.0, 120.0, 100.0], [150.0, 200.0, 240.0]],
                [[30.0, 70.0, 220.0], [240.0, 120.0, 70.0], [210.0, 240.0, 120.0]],
            ];
            let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
            let small = imageops::resize(src, hw, hh, imageops::FilterType::Triangle);
            let mut out = RgbaImage::new(w, h);
            // split at this image's own brightness thirds so every panel shows all three colours
            let mut ls: Vec<f32> = small.pixels().map(lum).collect();
            ls.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f32| ls[((ls.len() - 1) as f32 * f) as usize];
            let spread = 0.08 * (a - 0.5);
            let cut = (q(0.36 - spread), q(0.68 + spread));
            out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                let (x, y) = (i as u32 % w, i as u32 / w);
                let q = (x >= hw) as usize + 2 * (y >= hh) as usize;
                let p = small.get_pixel((x % hw).min(hw - 1), (y % hh).min(hh - 1));
                let l = lum(p);
                let c = if l < cut.0 { pals[q][0] } else if l < cut.1 { pals[q][1] } else { pals[q][2] };
                o.copy_from_slice(&[c8(c[0]), c8(c[1]), c8(c[2]), p[3]]);
            });
            out
        }
        BoothFx::Bulge | BoothFx::Dent | BoothFx::FishEye => {
            let (r0, k) = match fx {
                BoothFx::Bulge => (rad * 1.1, a * 1.65),
                BoothFx::Dent => (rad * 1.1, -(a * 0.8)),
                _ => (w.max(h) as f32 * 0.5, a * 1.6),
            };
            let fish = fx == BoothFx::FishEye;
            let mut out = warp(src, true, |x, y| {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= r0 || d < 1e-3 {
                    return (x, y);
                }
                let t = d / r0;
                // inside the circle sample nearer the centre (bulge) or further out (dent)
                let s = t.powf(1.0 + k) / t;
                (cx + dx * s, cy + dy * s)
            });
            if fish {
                // round lens: black outside the circle with a soft edge
                out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                    let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
                    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                    let m = lerp(1.0, ((r0 - d) / 1.5).clamp(0.0, 1.0), a);
                    for c in 0..3 {
                        o[c] = c8(o[c] as f32 * m);
                    }
                });
            }
            out
        }
        BoothFx::Twirl => {
            let ang = (a * 540.0).to_radians();
            let r0 = rad * 1.2;
            warp(src, true, |x, y| {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= r0 {
                    return (x, y);
                }
                let f = 1.0 - d / r0;
                let t = ang * f * f;
                let (s, c) = t.sin_cos();
                (cx + dx * c - dy * s, cy + dx * s + dy * c)
            })
        }
        BoothFx::Squeeze | BoothFx::Stretch => {
            // a horizontal funhouse mirror: the middle narrows or widens, the sides stay put
            let p = if fx == BoothFx::Squeeze { 1.0 / (1.0 + a * 2.0) } else { 1.0 + a * 2.0 };
            warp(src, true, |x, y| {
                let dx = x - cx;
                let span = if dx < 0.0 { cx.max(1.0) } else { (w as f32 - cx).max(1.0) };
                let t = (dx.abs() / span).min(1.0);
                (cx + dx.signum() * t.powf(p) * span, y)
            })
        }
        BoothFx::Mirror => {
            let mut out = src.clone();
            let mx = cx.clamp(1.0, (w as f32 - 1.0).max(1.0));
            out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                let x = (i as u32 % w) as f32;
                let y = i as u32 / w;
                if x >= mx {
                    let sx = (2.0 * mx - x - 1.0).clamp(0.0, w as f32 - 1.0) as u32;
                    o.copy_from_slice(&src.get_pixel(sx, y).0);
                }
            });
            out
        }
        BoothFx::Tunnel => {
            // the centre circle stays, everything outside streaks out from its edge
            let r0 = rad * lerp(0.75, 0.35, a);
            warp(src, true, |x, y| {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d <= r0 {
                    return (x, y);
                }
                let s = (r0 + (d - r0) * lerp(1.0, 0.04, a)) / d;
                (cx + dx * s, cy + dy * s)
            })
        }
        BoothFx::Kaleidoscope => {
            let n = 4 + (a * 8.0).round() as i32;
            let seg = TAU / n as f32;
            // the slice points right, where a landscape frame has the most picture
            let reach = ((w as f32 - cx).max(cx) * 0.95).max(1.0);
            warp(src, true, |x, y| {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                let mut t = (dy.atan2(dx) + PI).rem_euclid(seg);
                if t > seg / 2.0 {
                    t = seg - t;
                }
                let t = t - seg / 4.0;
                // fold distances past the picture back in, so the pattern keeps going
                let m = (d / reach).rem_euclid(2.0);
                let dd = if m > 1.0 { 2.0 - m } else { m } * reach;
                (cx + dd * t.cos(), cy + dd * t.sin())
            })
        }
    }
}
