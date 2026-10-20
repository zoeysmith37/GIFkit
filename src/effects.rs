//! Per-frame image effects (colour, filters, distortions, frames) plus
//! animation-flow effects. Mirrors the "Effects" page of the reference tool.

use crate::anim::{effective_delay, Animation};
use crate::ops::{self, blend_over, sample_bilinear, sample_clamped};
use crate::text::FontEntry;
use image::{imageops, Rgba, RgbaImage};
use rayon::prelude::*;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EffectSettings {
    // Colorize (100 = unchanged)
    pub hue: f32,
    pub saturation: f32,
    pub lightness: f32,
    // -100..100
    pub brightness: f32,
    pub contrast: f32,

    // Colour presets
    pub grayscale: bool,
    pub sepia: bool,
    pub monochrome: bool,
    pub negative: bool,
    pub tint: bool,
    pub tint_color: [u8; 3],
    pub tint_amount: f32,
    pub change_bg: bool,
    pub bg_color: [u8; 3],

    // Transformations
    pub flip_v: bool,
    pub flip_h: bool,
    pub rot90: bool,
    pub rot270: bool,
    pub rot_any: bool,
    pub rot_deg: f32,

    // Replace colour with transparency
    pub rc_white: bool,
    pub rc_black: bool,
    pub rc_custom: bool,
    pub rc_color: [u8; 3],
    pub rc_fuzz: f32,
    pub rc_clean_edges: bool,
    pub rc_edge_px: u32,

    // Animation flow
    pub reverse: bool,
    pub set_loop: bool,
    pub loop_count: u16, // 0 = forever
    pub boomerang: bool,
    pub counter: bool,
    pub fade_in: bool,
    pub fade_out: bool,
    pub fade_frames: usize,
    pub fade_to_transparent: bool,
    /// how the animation starts and ends (Fade is the old fade in / out)
    pub intro: crate::motion::Transition,
    pub outro: crate::motion::Transition,
    // looping motion over the whole animation
    pub loop_shake: bool,
    pub loop_pulse: bool,
    pub loop_hop: bool,
    pub loop_wobble: bool,
    pub loop_rainbow: bool,
    pub loop_glitch: bool,
    pub loop_amount: f32,
    pub loop_cycles: u32,

    // Blur / sharpen
    pub blur: bool,
    pub blur_sigma: f32,
    pub sharpen: bool,
    pub sharpen_sigma: f32,

    // Dithering
    pub ordered_dither: bool,
    pub error_dither: bool,
    pub dither_levels: u8,

    // Artistic
    pub oil: bool,
    pub oil_radius: u32,
    pub emboss: bool,
    pub posterize: bool,
    pub posterize_levels: u8,
    pub solarize: bool,
    pub noise: bool,
    pub noise_amount: f32,

    // Distortions
    pub swirl: bool,
    pub swirl_deg: f32,
    pub implode: bool,
    pub implode_amount: f32,
    pub wave: bool,
    pub wave_amp: f32,
    pub wave_len: f32,

    // Filters
    pub gotham: bool,
    pub lomo: bool,
    pub nashville: bool,
    pub toaster: bool,
    pub vignette: bool,
    pub polaroid: bool,

    // Frames
    pub frame_camera: bool,
    pub frame_fuzzy: bool,
    pub frame_rounded: bool,
    pub rounded_radius: f32,
    pub frame_border: bool,
    pub border_px: u32,
    pub border_color: [u8; 3],

    // Photo Booth style effect (one at a time)
    pub booth: crate::booth::BoothFx,
    pub booth_amount: f32,
    /// centre of the distortion, % of the frame
    pub booth_cx: f32,
    pub booth_cy: f32,
    /// a distortion, separate from the photo booth look so both can be on
    pub warp: crate::booth::BoothFx,
    pub warp_amount: f32,
    pub warp_cx: f32,
    pub warp_cy: f32,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            hue: 100.0,
            saturation: 100.0,
            lightness: 100.0,
            brightness: 0.0,
            contrast: 0.0,
            grayscale: false,
            sepia: false,
            monochrome: false,
            negative: false,
            tint: false,
            tint_color: [255, 120, 180],
            tint_amount: 50.0,
            change_bg: false,
            bg_color: [255, 255, 255],
            flip_v: false,
            flip_h: false,
            rot90: false,
            rot270: false,
            rot_any: false,
            rot_deg: 45.0,
            rc_white: false,
            rc_black: false,
            rc_custom: false,
            rc_color: [0xff, 0xff, 0xfd],
            rc_fuzz: 2.0,
            rc_clean_edges: true,
            rc_edge_px: 2,
            reverse: false,
            set_loop: false,
            loop_count: 0,
            boomerang: false,
            counter: false,
            fade_in: false,
            fade_out: false,
            fade_frames: 8,
            fade_to_transparent: false,
            intro: crate::motion::Transition::None,
            outro: crate::motion::Transition::None,
            loop_shake: false,
            loop_pulse: false,
            loop_hop: false,
            loop_wobble: false,
            loop_rainbow: false,
            loop_glitch: false,
            loop_amount: 50.0,
            loop_cycles: 1,
            blur: false,
            blur_sigma: 2.0,
            sharpen: false,
            sharpen_sigma: 1.0,
            ordered_dither: false,
            error_dither: false,
            dither_levels: 4,
            oil: false,
            oil_radius: 3,
            emboss: false,
            posterize: false,
            posterize_levels: 4,
            solarize: false,
            noise: false,
            noise_amount: 20.0,
            swirl: false,
            swirl_deg: 90.0,
            implode: false,
            implode_amount: 0.5,
            wave: false,
            wave_amp: 6.0,
            wave_len: 60.0,
            gotham: false,
            lomo: false,
            nashville: false,
            toaster: false,
            vignette: false,
            polaroid: false,
            frame_camera: false,
            frame_fuzzy: false,
            frame_rounded: false,
            rounded_radius: 24.0,
            frame_border: false,
            border_px: 8,
            border_color: [0, 0, 0],
            booth: crate::booth::BoothFx::None,
            booth_amount: 50.0,
            booth_cx: 50.0,
            booth_cy: 50.0,
            warp: crate::booth::BoothFx::None,
            warp_amount: 50.0,
            warp_cx: 50.0,
            warp_cy: 50.0,
        }
    }
}

// ------------------------------------------------------------- colour helpers

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hue2rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    (hue2rgb(p, q, h + 1.0 / 3.0), hue2rgb(p, q, h), hue2rgb(p, q, h - 1.0 / 3.0))
}

#[inline]
fn luma(p: &Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

#[inline]
fn c8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn map_rgb(img: &mut RgbaImage, f: impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync) {
    img.par_chunks_mut(4).for_each(|p| {
        let (r, g, b) = f(p[0] as f32, p[1] as f32, p[2] as f32);
        p[0] = c8(r);
        p[1] = c8(g);
        p[2] = c8(b);
    });
}

pub fn modulate(img: &mut RgbaImage, hue: f32, sat: f32, light: f32) {
    let dh = (hue - 100.0) / 200.0; // ±0.5 turn
    let sm = sat / 100.0;
    let lm = light / 100.0;
    map_rgb(img, |r, g, b| {
        let (h, s, l) = rgb_to_hsl(r / 255.0, g / 255.0, b / 255.0);
        let (r, g, b) = hsl_to_rgb((h + dh).rem_euclid(1.0), (s * sm).clamp(0.0, 1.0), (l * lm).clamp(0.0, 1.0));
        (r * 255.0, g * 255.0, b * 255.0)
    });
}

pub fn brightness_contrast(img: &mut RgbaImage, bright: f32, contrast: f32) {
    // values may be typed beyond ±100; the curve is already a hard threshold at 100
    let c = contrast.clamp(-100.0, 100.0) * 2.55;
    let cf = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
    let b = bright * 2.55;
    map_rgb(img, |r, g, bb| ((r - 128.0) * cf + 128.0 + b, (g - 128.0) * cf + 128.0 + b, (bb - 128.0) * cf + 128.0 + b));
}

fn colorize(img: &mut RgbaImage, color: [u8; 3], amount: f32) {
    let a = amount.clamp(0.0, 1.0);
    map_rgb(img, |r, g, b| {
        (r * (1.0 - a) + color[0] as f32 * a, g * (1.0 - a) + color[1] as f32 * a, b * (1.0 - a) + color[2] as f32 * a)
    });
}

fn gamma(img: &mut RgbaImage, gamma: f32) {
    let inv = 1.0 / gamma;
    let lut: Vec<u8> = (0..256).map(|v| c8(255.0 * (v as f32 / 255.0).powf(inv))).collect();
    img.par_chunks_mut(4).for_each(|p| {
        for k in 0..3 {
            p[k] = lut[p[k] as usize];
        }
    });
}

fn vignette(img: &mut RgbaImage, strength: f32, color: [u8; 3]) {
    let (w, h) = img.dimensions();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let maxd = (cx * cx + cy * cy).sqrt();
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let x = (i as u32 % w) as f32;
        let y = (i as u32 / w) as f32;
        let d = (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / maxd).clamp(0.0, 1.0);
        let t = (smoothstep(0.45, 1.0, d) * strength).clamp(0.0, 1.0);
        for k in 0..3 {
            p[k] = c8(p[k] as f32 * (1.0 - t) + color[k] as f32 * t);
        }
    });
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ------------------------------------------------------------- individual effects

pub fn grayscale(img: &mut RgbaImage) {
    map_rgb(img, |r, g, b| {
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        (l, l, l)
    });
}

pub fn sepia(img: &mut RgbaImage) {
    map_rgb(img, |r, g, b| {
        (r * 0.393 + g * 0.769 + b * 0.189, r * 0.349 + g * 0.686 + b * 0.168, r * 0.272 + g * 0.534 + b * 0.131)
    });
}

fn monochrome(img: &mut RgbaImage) {
    map_rgb(img, |r, g, b| {
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        let v = if l >= 128.0 { 255.0 } else { 0.0 };
        (v, v, v)
    });
}

fn tint(img: &mut RgbaImage, color: [u8; 3], amount: f32) {
    let a = (amount / 100.0).clamp(0.0, 1.0);
    map_rgb(img, |r, g, b| {
        let l = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        let (tr, tg, tb) = (color[0] as f32 * l, color[1] as f32 * l, color[2] as f32 * l);
        (r * (1.0 - a) + tr * a, g * (1.0 - a) + tg * a, b * (1.0 - a) + tb * a)
    });
}

fn flatten_on(img: &mut RgbaImage, c: [u8; 3]) {
    img.par_chunks_mut(4).for_each(|p| {
        let o = blend_over(Rgba([c[0], c[1], c[2], 255]), Rgba([p[0], p[1], p[2], p[3]]));
        p.copy_from_slice(&o.0);
    });
}

fn color_dist(p: &[u8], c: [u8; 3]) -> f32 {
    (((p[0] as f32 - c[0] as f32).powi(2) + (p[1] as f32 - c[1] as f32).powi(2) + (p[2] as f32 - c[2] as f32).powi(2)) as f32).sqrt()
}

/// Make pixels close to `target` transparent. `fuzz` in percent.
pub fn replace_color(img: &mut RgbaImage, target: [u8; 3], fuzz: f32, clean_edges: Option<u32>) {
    let thr = fuzz.max(0.0) / 100.0 * 441.67;
    img.par_chunks_mut(4).for_each(|p| {
        if color_dist(p, target) <= thr {
            p[3] = 0;
        }
    });
    if let Some(px) = clean_edges {
        if px == 0 {
            return;
        }
        let (w, h) = img.dimensions();
        let edge_thr = (thr * 4.0).max(0.30 * 441.67);
        let alpha: Vec<u8> = img.pixels().map(|p| p[3]).collect();
        let r = px as i32;
        let src = img.clone();
        img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
            if alpha[i] == 0 {
                return;
            }
            let x = (i as u32 % w) as i32;
            let y = (i as u32 / w) as i32;
            let mut near = false;
            'o: for dy in -r..=r {
                for dx in -r..=r {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if alpha[(ny as u32 * w + nx as u32) as usize] == 0 {
                        near = true;
                        break 'o;
                    }
                }
            }
            if near {
                let d = color_dist(&src.get_pixel(x as u32, y as u32).0, target);
                if d <= edge_thr {
                    // fade by closeness to the removed colour
                    let t = (d / edge_thr).clamp(0.0, 1.0);
                    p[3] = c8(p[3] as f32 * t * t);
                    if p[3] < 40 {
                        p[3] = 0;
                    }
                }
            }
        });
    }
}

fn convolve3(img: &RgbaImage, k: [f32; 9], bias: f32, gray: bool) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let x = (i as u32 % w) as i32;
        let y = (i as u32 / w) as i32;
        let mut acc = [0f32; 3];
        for ky in -1..=1i32 {
            for kx in -1..=1i32 {
                let sx = (x + kx).clamp(0, w as i32 - 1) as u32;
                let sy = (y + ky).clamp(0, h as i32 - 1) as u32;
                let p = img.get_pixel(sx, sy);
                let kv = k[((ky + 1) * 3 + kx + 1) as usize];
                if gray {
                    let l = luma(p);
                    acc[0] += l * kv;
                } else {
                    for c in 0..3 {
                        acc[c] += p[c] as f32 * kv;
                    }
                }
            }
        }
        if gray {
            let v = c8(acc[0] + bias);
            o[0] = v;
            o[1] = v;
            o[2] = v;
        } else {
            for c in 0..3 {
                o[c] = c8(acc[c] + bias);
            }
        }
    });
    out
}

fn oil_paint(img: &RgbaImage, radius: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let r = radius.max(1) as i32;
    const LEVELS: usize = 20;
    let mut out = img.clone();
    out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let x = (i as u32 % w) as i32;
        let y = (i as u32 / w) as i32;
        let mut count = [0u32; LEVELS];
        let mut sum = [[0u32; 3]; LEVELS];
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let sx = (x + dx).clamp(0, w as i32 - 1) as u32;
                let sy = (y + dy).clamp(0, h as i32 - 1) as u32;
                let p = img.get_pixel(sx, sy);
                let b = ((luma(p) / 256.0) * LEVELS as f32) as usize;
                let b = b.min(LEVELS - 1);
                count[b] += 1;
                for c in 0..3 {
                    sum[b][c] += p[c] as u32;
                }
            }
        }
        let (bi, bc) = count.iter().enumerate().max_by_key(|(_, c)| **c).map(|(i, c)| (i, *c)).unwrap();
        if bc > 0 {
            for c in 0..3 {
                o[c] = (sum[bi][c] / bc) as u8;
            }
        }
    });
    out
}

fn posterize(img: &mut RgbaImage, levels: u8) {
    let l = levels.max(2) as f32 - 1.0;
    map_rgb(img, |r, g, b| ((r / 255.0 * l).round() / l * 255.0, (g / 255.0 * l).round() / l * 255.0, (b / 255.0 * l).round() / l * 255.0));
}

fn solarize(img: &mut RgbaImage) {
    map_rgb(img, |r, g, b| {
        let f = |v: f32| if v > 127.0 { 255.0 - v } else { v };
        (f(r), f(g), f(b))
    });
}

fn add_noise(img: &mut RgbaImage, amount: f32, seed: u64) {
    let w = img.width() as u64;
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        // hash-based per-pixel noise so it is deterministic and parallel
        let mut z = seed.wrapping_mul(0x9E3779B97F4A7C15) ^ (i as u64).wrapping_mul(0xBF58476D1CE4E5B9) ^ w;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        let n1 = ((z & 0xffff) as f32 / 65535.0) - 0.5;
        let n2 = (((z >> 16) & 0xffff) as f32 / 65535.0) - 0.5;
        let n3 = (((z >> 32) & 0xffff) as f32 / 65535.0) - 0.5;
        let a = amount * 2.0;
        p[0] = c8(p[0] as f32 + n1 * a);
        p[1] = c8(p[1] as f32 + n2 * a);
        p[2] = c8(p[2] as f32 + n3 * a);
    });
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

pub fn ordered_dither(img: &mut RgbaImage, levels: u8) {
    let l = levels.max(2) as f32 - 1.0;
    let w = img.width();
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let x = (i as u32 % w) as usize;
        let y = (i as u32 / w) as usize;
        let t = (BAYER8[y % 8][x % 8] as f32 + 0.5) / 64.0 - 0.5;
        for k in 0..3 {
            let v = p[k] as f32 / 255.0 * l + t;
            p[k] = c8(v.round().clamp(0.0, l) / l * 255.0);
        }
    });
}

pub fn error_diffusion(img: &mut RgbaImage, levels: u8) {
    let l = levels.max(2) as f32 - 1.0;
    let (w, h) = img.dimensions();
    let mut buf: Vec<[f32; 3]> = img.pixels().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = y * w as usize + x;
            let old = buf[i];
            let mut new = [0f32; 3];
            for k in 0..3 {
                new[k] = ((old[k] / 255.0 * l).round().clamp(0.0, l)) / l * 255.0;
            }
            let err = [old[0] - new[0], old[1] - new[1], old[2] - new[2]];
            buf[i] = new;
            let mut push = |xx: i64, yy: i64, f: f32| {
                if xx >= 0 && xx < w as i64 && yy < h as i64 {
                    let j = yy as usize * w as usize + xx as usize;
                    for k in 0..3 {
                        buf[j][k] += err[k] * f;
                    }
                }
            };
            push(x as i64 + 1, y as i64, 7.0 / 16.0);
            push(x as i64 - 1, y as i64 + 1, 3.0 / 16.0);
            push(x as i64, y as i64 + 1, 5.0 / 16.0);
            push(x as i64 + 1, y as i64 + 1, 1.0 / 16.0);
        }
    }
    for (p, v) in img.pixels_mut().zip(buf) {
        p[0] = c8(v[0]);
        p[1] = c8(v[1]);
        p[2] = c8(v[2]);
    }
}

pub(crate) fn warp(img: &RgbaImage, clamp: bool, f: impl Fn(f32, f32) -> (f32, f32) + Sync) -> RgbaImage {
    let w = img.width();
    let mut out = img.clone();
    out.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let x = (i as u32 % w) as f32;
        let y = (i as u32 / w) as f32;
        let (sx, sy) = f(x, y);
        let p = if clamp { sample_clamped(img, sx, sy) } else { sample_bilinear(img, sx, sy) };
        o.copy_from_slice(&p.0);
    });
    out
}

fn swirl(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let rad = cx.min(cy);
    let ang = degrees.to_radians();
    warp(img, true, |x, y| {
        let dx = x - cx;
        let dy = y - cy;
        let d = (dx * dx + dy * dy).sqrt();
        if d >= rad {
            return (x, y);
        }
        let f = 1.0 - d / rad;
        let a = ang * f * f;
        let (s, c) = a.sin_cos();
        (cx + dx * c - dy * s, cy + dx * s + dy * c)
    })
}

fn implode(img: &RgbaImage, amount: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let rad = cx.min(cy);
    warp(img, true, |x, y| {
        let dx = x - cx;
        let dy = y - cy;
        let d = (dx * dx + dy * dy).sqrt();
        if d >= rad || d < 1e-3 {
            return (x, y);
        }
        let f = (std::f32::consts::PI * d / rad / 2.0).sin().powf(-amount);
        (cx + dx * f, cy + dy * f)
    })
}

fn wave(img: &RgbaImage, amp: f32, len: f32, phase: f32) -> RgbaImage {
    let l = len.max(1.0);
    warp(img, true, |x, y| (x, y + amp * ((2.0 * std::f32::consts::PI * x / l) + phase).sin()))
}

fn gotham(img: &mut RgbaImage) {
    modulate(img, 100.0, 10.0, 120.0);
    colorize(img, [0x22, 0x2b, 0x6d], 0.2);
    gamma(img, 0.5);
    brightness_contrast(img, 0.0, 25.0);
}

fn lomo(img: &mut RgbaImage) {
    let lvl = |v: f32| ((v - 0.22 * 255.0) / (0.56)).clamp(0.0, 255.0);
    map_rgb(img, |r, g, b| (lvl(r), lvl(g), b));
    vignette(img, 0.85, [0, 0, 0]);
}

fn nashville(img: &mut RgbaImage) {
    colorize(img, [0xf7, 0xda, 0xae], 0.25);
    map_rgb(img, |r, g, b| (r * 0.87 + 0.13 * 255.0 * 0.6, g * 0.87 + 0.13 * 255.0 * 0.2, b * 0.87 + 0.13 * 255.0 * 0.3));
    brightness_contrast(img, 5.0, 15.0);
    gamma(img, 1.2);
}

fn toaster(img: &mut RgbaImage) {
    colorize(img, [0xf6, 0x8b, 0x1f], 0.18);
    modulate(img, 100.0, 85.0, 110.0);
    gamma(img, 1.15);
    vignette(img, 0.7, [0x50, 0x10, 0x00]);
}

fn polaroid(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let side = ((w.min(h) as f32) * 0.06).round().max(4.0) as u32;
    let bottom = side * 4;
    let mut out = RgbaImage::from_pixel(w + side * 2, h + side + bottom, Rgba([250, 250, 246, 255]));
    let mut inner = img.clone();
    colorize(&mut inner, [255, 235, 200], 0.08);
    imageops::overlay(&mut out, &inner, side as i64, side as i64);
    out
}

fn rounded(img: &mut RgbaImage, radius: f32) {
    let (w, h) = img.dimensions();
    let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let x = (i as u32 % w) as f32 + 0.5;
        let y = (i as u32 / w) as f32 + 0.5;
        let cx = if x < r { r } else if x > w as f32 - r { w as f32 - r } else { x };
        let cy = if y < r { r } else if y > h as f32 - r { h as f32 - r } else { y };
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let a = (r - d + 0.5).clamp(0.0, 1.0);
        p[3] = c8(p[3] as f32 * a);
    });
}

fn fuzzy(img: &mut RgbaImage) {
    let (w, h) = img.dimensions();
    let f = (w.min(h) as f32 * 0.08).max(2.0);
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let x = (i as u32 % w) as f32 + 0.5;
        let y = (i as u32 / w) as f32 + 0.5;
        let d = x.min(y).min(w as f32 - x).min(h as f32 - y);
        // jitter the edge a little to look "fuzzy"
        let j = (((x * 12.9898 + y * 78.233).sin() * 43758.547).fract() - 0.5) * f * 0.6;
        let a = smoothstep(0.0, f, d + j);
        p[3] = c8(p[3] as f32 * a);
    });
}

fn border(img: &RgbaImage, px: u32, color: [u8; 3]) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::from_pixel(w + px * 2, h + px * 2, Rgba([color[0], color[1], color[2], 255]));
    imageops::replace(&mut out, img, px as i64, px as i64);
    out
}

fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: Rgba<u8>) {
    let (iw, ih) = img.dimensions();
    for yy in y.max(0)..(y + h).min(ih as i32) {
        for xx in x.max(0)..(x + w).min(iw as i32) {
            let d = img.get_pixel_mut(xx as u32, yy as u32);
            *d = blend_over(*d, c);
        }
    }
}

fn camera(img: &mut RgbaImage, fonts: &[FontEntry], frame_idx: usize) {
    let (w, h) = img.dimensions();
    let m = (w.min(h) as f32 * 0.06) as i32;
    let len = (w.min(h) as f32 * 0.12) as i32;
    let t = ((w.min(h) as f32) * 0.012).max(2.0) as i32;
    let white = Rgba([255, 255, 255, 230]);
    let (w, h) = (w as i32, h as i32);
    // corner brackets
    for (cx, cy, sx, sy) in [(m, m, 1, 1), (w - m, m, -1, 1), (m, h - m, 1, -1), (w - m, h - m, -1, -1)] {
        let x0 = if sx > 0 { cx } else { cx - len };
        let y0 = if sy > 0 { cy } else { cy - t };
        fill_rect(img, x0, y0, len, t, white);
        let x1 = if sx > 0 { cx } else { cx - t };
        let y1 = if sy > 0 { cy } else { cy - len };
        fill_rect(img, x1, y1, t, len, white);
    }
    // centre cross
    let cl = len / 3;
    fill_rect(img, w / 2 - cl, h / 2 - t / 2, cl * 2, t, Rgba([255, 255, 255, 160]));
    fill_rect(img, w / 2 - t / 2, h / 2 - cl, t, cl * 2, Rgba([255, 255, 255, 160]));
    // blinking REC dot
    let size = (h as f32 * 0.05).max(10.0);
    let dot_r = (size * 0.35) as i32;
    let dx = m + len / 2 + dot_r;
    let dy = m + len / 2 + dot_r;
    if (frame_idx / 8) % 2 == 0 {
        for yy in -dot_r..=dot_r {
            for xx in -dot_r..=dot_r {
                if xx * xx + yy * yy <= dot_r * dot_r {
                    fill_rect(img, dx + xx, dy + yy, 1, 1, Rgba([230, 30, 30, 255]));
                }
            }
        }
    }
    crate::text::draw_simple(img, fonts, "REC", dx + dot_r * 2, dy - (size * 0.6) as i32, size);
}

// ------------------------------------------------------------- pipeline

/// Apply all enabled per-frame effects to one image.
/// `idx` and `n` count the shown frames of the clip (intros and outros go by
/// frames); `elapsed_s` and `clip_s` are the time into the clip and its length
/// (looping motion goes by time, so frames of different lengths keep it even).
pub fn apply_frame(src: &RgbaImage, idx: usize, n: usize, elapsed_s: f32, clip_s: f32, s: &EffectSettings, fonts: &[FontEntry]) -> RgbaImage {
    let mut img = src.clone();
    let d = EffectSettings::default();

    if s.hue != d.hue || s.saturation != d.saturation || s.lightness != d.lightness {
        modulate(&mut img, s.hue, s.saturation, s.lightness);
    }
    if s.brightness != 0.0 || s.contrast != 0.0 {
        brightness_contrast(&mut img, s.brightness, s.contrast);
    }
    // replace colour before colour presets so the target colour matches the source
    if s.rc_white || s.rc_black || s.rc_custom {
        let edges = if s.rc_clean_edges { Some(s.rc_edge_px) } else { None };
        if s.rc_white {
            replace_color(&mut img, [255, 255, 255], s.rc_fuzz, edges);
        }
        if s.rc_black {
            replace_color(&mut img, [0, 0, 0], s.rc_fuzz, edges);
        }
        if s.rc_custom {
            replace_color(&mut img, s.rc_color, s.rc_fuzz, edges);
        }
    }
    if s.grayscale {
        grayscale(&mut img);
    }
    if s.sepia {
        sepia(&mut img);
    }
    if s.monochrome {
        monochrome(&mut img);
    }
    if s.negative {
        map_rgb(&mut img, |r, g, b| (255.0 - r, 255.0 - g, 255.0 - b));
    }
    if s.tint {
        tint(&mut img, s.tint_color, s.tint_amount);
    }
    if s.blur {
        img = imageops::blur(&img, s.blur_sigma.max(0.1));
    }
    if s.sharpen {
        img = imageops::unsharpen(&img, s.sharpen_sigma.max(0.1), 2);
    }
    if s.oil {
        img = oil_paint(&img, s.oil_radius);
    }
    if s.emboss {
        img = convolve3(&img, [-2.0, -1.0, 0.0, -1.0, 1.0, 1.0, 0.0, 1.0, 2.0], 0.0, true);
    }
    if s.posterize {
        posterize(&mut img, s.posterize_levels);
    }
    if s.solarize {
        solarize(&mut img);
    }
    if s.noise {
        add_noise(&mut img, s.noise_amount, idx as u64 + 1);
    }
    if s.swirl {
        img = swirl(&img, s.swirl_deg);
    }
    if s.implode {
        img = implode(&img, s.implode_amount);
    }
    if s.wave {
        let phase = idx as f32 / n.max(1) as f32 * std::f32::consts::TAU;
        img = wave(&img, s.wave_amp, s.wave_len, phase);
    }
    if s.booth != crate::booth::BoothFx::None {
        img = crate::booth::apply(&img, s.booth, s.booth_amount, s.booth_cx, s.booth_cy);
    }
    if s.warp != crate::booth::BoothFx::None {
        img = crate::booth::apply(&img, s.warp, s.warp_amount, s.warp_cx, s.warp_cy);
    }
    if s.gotham {
        gotham(&mut img);
    }
    if s.lomo {
        lomo(&mut img);
    }
    if s.nashville {
        nashville(&mut img);
    }
    if s.toaster {
        toaster(&mut img);
    }
    if s.vignette {
        vignette(&mut img, 0.9, [0, 0, 0]);
    }
    if s.ordered_dither {
        ordered_dither(&mut img, s.dither_levels);
    }
    if s.error_dither {
        error_diffusion(&mut img, s.dither_levels);
    }
    if s.change_bg {
        flatten_on(&mut img, s.bg_color);
    }
    // transformations
    if s.flip_v {
        img = imageops::flip_vertical(&img);
    }
    if s.flip_h {
        img = imageops::flip_horizontal(&img);
    }
    if s.rot90 {
        img = imageops::rotate90(&img);
    }
    if s.rot270 {
        img = imageops::rotate270(&img);
    }
    if s.rot_any {
        img = ops::rotate_any_image(&img, s.rot_deg, Rgba([0, 0, 0, 0]), true);
    }
    // frames / decorations
    if s.frame_camera {
        camera(&mut img, fonts, idx);
    }
    if s.frame_fuzzy {
        fuzzy(&mut img);
    }
    if s.frame_rounded {
        rounded(&mut img, s.rounded_radius);
    }
    if s.polaroid {
        img = polaroid(&img);
    }
    if s.frame_border {
        img = border(&img, s.border_px, s.border_color);
    }
    // looping motion, then the intro and outro on top
    let loops = s.loop_fx();
    if !loops.is_empty() {
        // position through the clip by time, in steps fine enough to look smooth
        const STEPS: usize = 3600;
        let (li, ln) = if clip_s > 0.0 { (((elapsed_s / clip_s).clamp(0.0, 1.0) * STEPS as f32).round() as usize, STEPS) } else { (idx, n) };
        img = crate::motion::looped(&img, &loops, li, ln, s.loop_amount, s.loop_cycles, s.fade_to_transparent);
    }
    let ff = s.fade_frames.max(1);
    let (intro, outro) = s.transitions();
    if intro != crate::motion::Transition::None && idx < ff {
        img = crate::motion::transition(&img, intro, (idx as f32 + 1.0) / (ff as f32 + 1.0), s.fade_to_transparent);
    }
    if outro != crate::motion::Transition::None && n > 0 && idx + ff >= n {
        img = crate::motion::transition(&img, outro, (n - idx) as f32 / (ff as f32 + 1.0), s.fade_to_transparent);
    }
    if s.counter {
        let size = (img.height() as f32 * 0.07).max(12.0);
        let txt = format!("{:.2}s", elapsed_s);
        let x = 6;
        let y = img.height() as i32 - size as i32 - 10;
        crate::text::draw_simple(&mut img, fonts, &txt, x, y, size);
    }
    img
}

impl EffectSettings {
    /// The intro and outro, with the old fade in / out switches as Fade.
    pub fn transitions(&self) -> (crate::motion::Transition, crate::motion::Transition) {
        use crate::motion::Transition as T;
        let intro = if self.intro == T::None && self.fade_in { T::Fade } else { self.intro };
        let outro = if self.outro == T::None && self.fade_out { T::Fade } else { self.outro };
        (intro, outro)
    }

    pub fn loop_fx(&self) -> Vec<crate::motion::LoopFx> {
        use crate::motion::LoopFx as L;
        [(self.loop_shake, L::Shake), (self.loop_pulse, L::Pulse), (self.loop_hop, L::Hop), (self.loop_wobble, L::Wobble), (self.loop_rainbow, L::Rainbow), (self.loop_glitch, L::Glitch)]
            .into_iter()
            .filter_map(|(on, f)| on.then_some(f))
            .collect()
    }

    /// Where `apply_frame` moves pixels (flips, rotations, polaroid / border padding),
    /// so layers above this effect can follow the content.
    pub fn geo_map(&self, g: &mut crate::geo::GeoMap) {
        if self.flip_v {
            g.flip_v();
        }
        if self.flip_h {
            g.flip_h();
        }
        if self.rot90 {
            g.rot90();
        }
        if self.rot270 {
            g.rot270();
        }
        if self.rot_any {
            g.rot_any(self.rot_deg, true);
        }
        if self.polaroid {
            let side = ((g.w.min(g.h)) * 0.06).round().max(4.0);
            let (w, h) = (g.w, g.h);
            g.offset(side, side, w + side * 2.0, h + side * 5.0);
        }
        if self.frame_border {
            let px = self.border_px as f32;
            let (w, h) = (g.w, g.h);
            g.offset(px, px, w + px * 2.0, h + px * 2.0);
        }
    }
}

/// Seconds elapsed at the start of each frame.
pub fn frame_start_times(anim: &Animation) -> Vec<f32> {
    let mut t = 0.0;
    anim.frames
        .iter()
        .map(|f| {
            let s = t;
            t += effective_delay(f.delay_cs) as f32 / 100.0;
            s
        })
        .collect()
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply(anim: &Animation, s: &EffectSettings, fonts: &[FontEntry], progress: &std::sync::atomic::AtomicUsize) -> Animation {
    use std::sync::atomic::Ordering;
    // animation flow that changes order happens first so counters/fades follow the new order
    let mut a = anim.clone();
    if s.reverse {
        a = ops::reverse(&a);
    }
    if s.boomerang {
        a = ops::boomerang(&a);
    }
    if s.set_loop {
        a.loop_count = if s.loop_count == 0 { None } else { Some(s.loop_count) };
    }
    let n = a.len();
    let times = frame_start_times(&a);
    let total = a.duration_secs();
    // Frames are processed in parallel; individual effects are also parallel inside.
    let frames = a
        .frames
        .par_iter()
        .enumerate()
        .map(|(i, f)| {
            let img = apply_frame(&f.image, i, n, times[i], total, s, fonts);
            progress.fetch_add(1, Ordering::Relaxed);
            f.with_image(img)
        })
        .collect::<Vec<_>>();
    // Effects like rotate/polaroid change size; ensure all frames share the largest size.
    let mut out = Animation { frames, loop_count: a.loop_count };
    normalize_sizes(&mut out);
    out
}

/// Make all frames the same size (pads smaller frames, top-left aligned centring).
pub fn normalize_sizes(a: &mut Animation) {
    let mw = a.frames.iter().map(|f| f.image.width()).max().unwrap_or(0);
    let mh = a.frames.iter().map(|f| f.image.height()).max().unwrap_or(0);
    for f in &mut a.frames {
        if f.image.dimensions() != (mw, mh) {
            let mut c = RgbaImage::new(mw, mh);
            let x = (mw - f.image.width()) / 2;
            let y = (mh - f.image.height()) / 2;
            imageops::replace(&mut c, &*f.image, x as i64, y as i64);
            *f = f.with_image(c);
        }
    }
}

// ------------------------------------------------------------- censor

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum CensorMethod {
    Pixelate,
    Blur,
    Solid,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CensorSettings {
    /// top-left of the unrotated box (source-frame pixels)
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub method: CensorMethod,
    pub square: u32,
    pub sigma: f32,
    pub color: [u8; 3],
    pub from: usize,
    pub to: usize, // 0 = end
    pub visible: bool,
    /// clockwise rotation in degrees about the box centre
    pub angle: f32,
}

impl Default for CensorSettings {
    fn default() -> Self {
        Self { x: 0, y: 0, w: 0, h: 0, method: CensorMethod::Pixelate, square: 10, sigma: 8.0, color: [0, 0, 0], from: 1, to: 0, visible: true, angle: 0.0 }
    }
}

impl CensorSettings {
    /// `frame` is the 0-based source frame index.
    pub fn applies_to(&self, frame: usize) -> bool {
        let i = frame + 1;
        self.w > 0 && self.h > 0 && i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

/// Axis-aligned censor (legacy helper, also used by tests).
#[cfg_attr(not(test), allow(dead_code))]
pub fn censor_frame(img: &RgbaImage, idx: usize, c: &CensorSettings) -> RgbaImage {
    if !c.applies_to(idx) {
        return img.clone();
    }
    censor_rect(img, c.x, c.y, c.w, c.h, c)
}

/// Censor an oriented box (output-space) with the method/params of `c`.
pub fn censor_obb(img: &RgbaImage, b: crate::geo::Obb, c: &CensorSettings) -> RgbaImage {
    let (x0, y0, x1, y1) = b.bounds();
    let (bx, by) = (x0.floor() as i32, y0.floor() as i32);
    let (bw, bh) = ((x1.ceil() as i32 - bx).max(0) as u32, (y1.ceil() as i32 - by).max(0) as u32);
    if bw == 0 || bh == 0 {
        return img.clone();
    }
    let axis_aligned = (b.angle % 90.0).abs() < 0.01;
    let censored = censor_rect(img, bx, by, bw, bh, c);
    if axis_aligned {
        return censored;
    }
    let (iw, ih) = img.dimensions();
    let mut out = img.clone();
    for yy in by.max(0)..(by + bh as i32).min(ih as i32) {
        for xx in bx.max(0)..(bx + bw as i32).min(iw as i32) {
            if b.contains(xx as f32 + 0.5, yy as f32 + 0.5) {
                out.put_pixel(xx as u32, yy as u32, *censored.get_pixel(xx as u32, yy as u32));
            }
        }
    }
    out
}

fn censor_rect(img: &RgbaImage, x: i32, y: i32, w: u32, h: u32, c: &CensorSettings) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let x0 = x.clamp(0, iw as i32) as u32;
    let y0 = y.clamp(0, ih as i32) as u32;
    let x1 = (x + w as i32).clamp(0, iw as i32) as u32;
    let y1 = (y + h as i32).clamp(0, ih as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return img.clone();
    }
    let (x, y, w, h) = (x0, y0, x1 - x0, y1 - y0);
    let mut out = img.clone();
    match c.method {
        CensorMethod::Solid => {
            for yy in y..y + h {
                for xx in x..x + w {
                    out.put_pixel(xx, yy, Rgba([c.color[0], c.color[1], c.color[2], 255]));
                }
            }
        }
        CensorMethod::Blur => {
            let region = imageops::crop_imm(img, x, y, w, h).to_image();
            let b = imageops::blur(&region, c.sigma.max(0.5));
            imageops::replace(&mut out, &b, x as i64, y as i64);
        }
        CensorMethod::Pixelate => {
            let sq = c.square.max(2);
            let mut by = y;
            while by < y + h {
                let bh = sq.min(y + h - by);
                let mut bx = x;
                while bx < x + w {
                    let bw = sq.min(x + w - bx);
                    let mut acc = [0u32; 4];
                    for yy in by..by + bh {
                        for xx in bx..bx + bw {
                            let p = img.get_pixel(xx, yy);
                            for k in 0..4 {
                                acc[k] += p[k] as u32;
                            }
                        }
                    }
                    let n = bw * bh;
                    let col = Rgba([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, (acc[3] / n) as u8]);
                    for yy in by..by + bh {
                        for xx in bx..bx + bw {
                            out.put_pixel(xx, yy, col);
                        }
                    }
                    bx += sq;
                }
                by += sq;
            }
        }
    }
    out
}

// ------------------------------------------------------------- watermark

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// (fx, fy) in 0..=1
    fn frac(self) -> (f32, f32) {
        match self {
            Anchor::TopLeft => (0.0, 0.0),
            Anchor::Top => (0.5, 0.0),
            Anchor::TopRight => (1.0, 0.0),
            Anchor::Left => (0.0, 0.5),
            Anchor::Center => (0.5, 0.5),
            Anchor::Right => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 1.0),
            Anchor::Bottom => (0.5, 1.0),
            Anchor::BottomRight => (1.0, 1.0),
        }
    }
}

/// A watermark anchored to the output frame (text or image, single or tiled).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WatermarkSettings {
    pub text: String,
    pub font_idx: usize,
    /// (family, style) for presets
    pub font_key: Option<(String, String)>,
    pub use_image: bool,
    pub image_path: Option<std::path::PathBuf>,
    #[serde(skip)]
    pub image: Option<std::sync::Arc<RgbaImage>>,
    /// text height / image width as % of the frame's shorter side
    pub size: f32,
    pub color: [u8; 4],
    pub outline: bool,
    /// outline width in pixels (None = automatic)
    pub outline_px: Option<f32>,
    /// 0..=100
    pub opacity: f32,
    pub anchor: Anchor,
    /// % of the shorter side
    pub margin: f32,
    pub tiled: bool,
    pub angle: f32,
    /// gap between tiles, as a multiple of the tile size
    pub spacing: f32,
}

impl PartialEq for WatermarkSettings {
    fn eq(&self, o: &Self) -> bool {
        let same_image = match (&self.image, &o.image) {
            (None, None) => true,
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        };
        same_image
            && self.text == o.text
            && self.font_idx == o.font_idx
            && self.font_key == o.font_key
            && self.use_image == o.use_image
            && self.image_path == o.image_path
            && self.size == o.size
            && self.color == o.color
            && self.outline == o.outline
            && self.outline_px == o.outline_px
            && self.opacity == o.opacity
            && self.anchor == o.anchor
            && self.margin == o.margin
            && self.tiled == o.tiled
            && self.angle == o.angle
            && self.spacing == o.spacing
    }
}

impl Default for WatermarkSettings {
    fn default() -> Self {
        Self {
            text: "© your name".into(),
            font_idx: 0,
            font_key: None,
            use_image: false,
            image_path: None,
            image: None,
            size: 6.0,
            color: [255, 255, 255, 255],
            outline: true,
            outline_px: None,
            opacity: 60.0,
            anchor: Anchor::BottomRight,
            margin: 3.0,
            tiled: false,
            angle: -30.0,
            spacing: 1.2,
        }
    }
}

/// Pre-rendered watermark for a frame size.
pub enum WatermarkSprite {
    Single(RgbaImage, i64, i64),
    Tiled(RgbaImage, f32, f32),
}

impl WatermarkSettings {
    pub fn active(&self) -> bool {
        if self.use_image { self.image.is_some() } else { !self.text.trim().is_empty() }
    }

    pub fn sprite(&self, fonts: &[crate::text::FontEntry], w: f32, h: f32) -> Option<WatermarkSprite> {
        if !self.active() {
            return None;
        }
        let short = w.min(h).max(1.0);
        let px = (self.size / 100.0 * short).clamp(4.0, 4000.0);
        let mut sp = if self.use_image {
            let img = self.image.as_ref()?;
            let s = px / img.width().max(1) as f32;
            let nw = (img.width() as f32 * s).round().max(1.0) as u32;
            let nh = (img.height() as f32 * s).round().max(1.0) as u32;
            imageops::resize(&**img, nw, nh, imageops::FilterType::Triangle)
        } else {
            let a = crate::text::Annotation {
                text: self.text.clone(),
                font_idx: self.font_idx.min(fonts.len().saturating_sub(1)),
                size: px,
                color: self.color,
                border: if self.outline { crate::text::Border::Light } else { crate::text::Border::None },
                border_width: if self.outline { self.outline_px } else { Some(0.0) },
                border_color: [0, 0, 0, (self.color[3] as f32 * 0.8) as u8],
                ..Default::default()
            };
            crate::text::render_sprite(fonts, &a)
        };
        if self.tiled {
            if self.angle % 360.0 != 0.0 {
                sp = crate::ops::rotate_any_image(&sp, self.angle, Rgba([0, 0, 0, 0]), true);
            }
            let sx = sp.width() as f32 * self.spacing.max(0.5);
            let sy = sp.height() as f32 * self.spacing.max(0.5) * 1.6;
            return Some(WatermarkSprite::Tiled(sp, sx, sy));
        }
        let m = self.margin / 100.0 * short;
        let (fx, fy) = self.anchor.frac();
        let x = m + (w - 2.0 * m - sp.width() as f32) * fx;
        let y = m + (h - 2.0 * m - sp.height() as f32) * fy;
        Some(WatermarkSprite::Single(sp, x.round() as i64, y.round() as i64))
    }
}

pub fn draw_watermark(img: &mut RgbaImage, sp: &WatermarkSprite, opacity: f32) {
    let o = (opacity / 100.0).clamp(0.0, 1.0);
    match sp {
        WatermarkSprite::Single(s, x, y) => crate::ops::overlay_with_opacity(img, s, *x, *y, o),
        WatermarkSprite::Tiled(s, sx, sy) => {
            let (w, h) = (img.width() as f32, img.height() as f32);
            let mut row = 0;
            let mut y = -(s.height() as f32);
            while y < h {
                let off = if row % 2 == 1 { sx / 2.0 } else { 0.0 };
                let mut x = -(s.width() as f32) + off;
                while x < w {
                    crate::ops::overlay_with_opacity(img, s, x.round() as i64, y.round() as i64, o);
                    x += sx;
                }
                y += sy;
                row += 1;
            }
        }
    }
}

// ------------------------------------------------------------- AI shield

/// Perturbation that makes frames less useful as clean AI training data.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ShieldSettings {
    /// amplitude in colour levels (0..255)
    pub strength: f32,
    /// structured high-frequency pattern
    pub pattern: bool,
    /// per-pixel chroma noise
    pub noise: bool,
    /// sub-pixel geometric jitter
    pub jitter: bool,
    /// change the perturbation every frame (hurts GIF compression)
    pub per_frame: bool,
    pub seed: u32,
}

impl Default for ShieldSettings {
    fn default() -> Self {
        Self { strength: 6.0, pattern: true, noise: true, jitter: false, per_frame: false, seed: 1337 }
    }
}

#[inline]
fn hash3(x: u32, y: u32, z: u32) -> u32 {
    let mut h = x.wrapping_mul(0x8da6b343) ^ y.wrapping_mul(0xd8163841) ^ z.wrapping_mul(0xcb1ab31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^ (h >> 15)
}

pub fn shield(src: &RgbaImage, s: &ShieldSettings, frame: usize) -> RgbaImage {
    let z = if s.per_frame { s.seed.wrapping_add(frame as u32 * 7919) } else { s.seed };
    let mut img = if s.jitter {
        let zz = z;
        warp(src, true, move |x, y| {
            let a = ((x * 0.37 + y * 0.11 + (zz % 97) as f32).sin() + (y * 0.29 - x * 0.07).cos()) * 0.25;
            let b = ((y * 0.41 - x * 0.13 + (zz % 89) as f32).sin() + (x * 0.23 + y * 0.05).cos()) * 0.25;
            (x + a, y + b)
        })
    } else {
        src.clone()
    };
    // a few high-frequency gratings with seeded orientations / phases per channel
    let mut waves = [[0f32; 4]; 9];
    for (k, w) in waves.iter_mut().enumerate() {
        let r = hash3(z, k as u32, 99);
        let ang = (r % 3600) as f32 / 3600.0 * std::f32::consts::TAU;
        let period = 2.2 + ((r >> 12) % 100) as f32 / 40.0; // 2.2–4.7 px
        let f = std::f32::consts::TAU / period;
        *w = [ang.cos() * f, ang.sin() * f, ((r >> 20) % 628) as f32 / 100.0, 0.0];
    }
    let amp = s.strength.max(0.0);
    let w = img.width();
    img.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        if p[3] < 8 {
            return;
        }
        let x = (i as u32 % w) as f32;
        let y = (i as u32 / w) as f32;
        for c in 0..3 {
            let mut d = 0.0;
            if s.pattern {
                let mut v = 0.0;
                for k in 0..3 {
                    let wv = waves[c * 3 + k];
                    v += (wv[0] * x + wv[1] * y + wv[2]).sin();
                }
                d += v / 3.0 * 0.8;
            }
            if s.noise {
                let r = hash3(i as u32, c as u32, z);
                d += ((r & 0xffff) as f32 / 32767.5 - 1.0) * 0.6;
            }
            p[c] = c8(p[c] as f32 + d * amp);
        }
    });
    img
}
