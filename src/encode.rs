//! GIF encoder with optimisation options.
//!
//! - palette reduction (NeuQuant, 2..=256 colours, per-frame or global palette)
//! - frame differencing: only the changed rectangle of each frame is stored and
//!   unchanged pixels become transparent (like gifsicle -O2)
//! - "lossy": pixels whose colour changed by less than a tolerance are treated
//!   as unchanged, which massively improves LZW compression on noisy footage
//! - identical consecutive frames are merged (their delays are summed)
//! - optional Floyd–Steinberg dithering against the palette

use crate::anim::{Animation, Frame};
use color_quant::NeuQuant;
use image::RgbaImage;
use rayon::prelude::*;
use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GifOptions {
    /// Max colours per palette (2..=256).
    pub colors: u16,
    /// Use one palette for every frame (smaller files, may band on varied footage).
    pub global_palette: bool,
    /// Store only changed regions, unchanged pixels transparent.
    pub frame_diff: bool,
    /// 0 = lossless differencing, up to 200 = very aggressive.
    pub lossy: u16,
    /// NeuQuant sampling factor: 1 = best quality / slowest, 30 = fastest.
    pub quant_speed: i32,
    /// Floyd–Steinberg dither when mapping to the palette.
    pub dither: bool,
    /// Ordered (Bayer) dither instead, when `dither` is off: a regular
    /// pattern, made on the graphics card.
    pub ordered_dither: bool,
    /// Merge consecutive identical frames into one (delays summed).
    pub merge_duplicates: bool,
    /// Alpha below this is treated as fully transparent.
    pub alpha_threshold: u8,
    /// Always use the built-in LZW encoder (testing aid).
    pub force_custom_lzw: bool,
    /// Write a "noai, noimageai" comment into the file.
    pub noai_tag: bool,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            colors: 256,
            global_palette: false,
            frame_diff: true,
            lossy: 0,
            quant_speed: 10,
            dither: false,
            ordered_dither: false,
            merge_duplicates: true,
            alpha_threshold: 128,
            force_custom_lzw: false,
            // on by default: files are marked as not for AI training
            noai_tag: true,
        }
    }
}

/// Marker text for "do not use for AI training" (DeviantArt / common opt-out convention).
pub const NOAI_COMMENT: &str = "noai, noimageai - not licensed for AI/ML training";

/// A frame ready for encoding, before quantisation.
struct Prepared {
    // shared with the source frame (packed): unpacked only while it's encoded
    rgba: crate::pixels::Pixels,
    delay: u16,
}

fn has_transparency(frames: &[Frame], thr: u8) -> bool {
    frames.par_iter().any(|f| f.image.any_alpha_below(thr))
}

fn collect_samples(images: &[&RgbaImage], thr: u8, max_samples: usize) -> Vec<u8> {
    let total: usize = images.iter().map(|i| (i.width() * i.height()) as usize).sum();
    let step = (total / max_samples.max(1)).max(1);
    let mut out = Vec::with_capacity((total / step + 1) * 4);
    let mut k = 0usize;
    for img in images {
        for p in img.pixels() {
            if k % step == 0 && p[3] >= thr {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            k += 1;
        }
    }
    out
}

/// A palette plus lookup structure.
struct Palette {
    nq: Option<NeuQuant>,
    rgb: Vec<u8>, // 3 * n
    transparent: Option<u8>,
    /// exact colour -> index (exact palettes)
    lut: std::collections::HashMap<u32, u8>,
}

fn key(r: u8, g: u8, b: u8) -> u32 {
    u32::from_le_bytes([r, g, b, 0])
}

fn make_lut(rgb: &[u8], opaque: usize) -> std::collections::HashMap<u32, u8> {
    let mut m = std::collections::HashMap::with_capacity(opaque);
    for i in (0..opaque).rev() {
        m.insert(key(rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]), i as u8);
    }
    m
}

impl Palette {
    fn build(samples: &[u8], colors: u16, need_transparent: bool, speed: i32) -> Palette {
        let mut n = colors.clamp(2, 256) as usize;
        if need_transparent {
            n = n.saturating_sub(1).max(1);
        }
        let mut samples = samples.to_vec();
        if samples.is_empty() {
            samples.extend_from_slice(&[0, 0, 0, 255]);
        }
        // Count distinct colours; if few, build an exact palette (no quantisation loss).
        let mut distinct = std::collections::HashSet::new();
        for c in samples.chunks_exact(4) {
            distinct.insert([c[0], c[1], c[2]]);
            if distinct.len() > n {
                break;
            }
        }
        let (nq, mut rgb) = if distinct.len() <= n {
            let mut v: Vec<[u8; 3]> = distinct.into_iter().collect();
            v.sort();
            (None, v.into_iter().flatten().collect::<Vec<u8>>())
        } else {
            // NeuQuant learns poorly from few samples: repeat small sets and lower the
            // sampling factor so it always sees enough pixels to converge.
            while samples.len() < 120_000 {
                let s = samples.clone();
                samples.extend_from_slice(&s);
            }
            let npix = (samples.len() / 4) as i32;
            let sf = speed.clamp(1, 30).min((npix / 60_000).max(1));
            let nq = NeuQuant::new(sf, n.max(4), &samples);
            let rgb = nq.color_map_rgb();
            (Some(nq), rgb)
        };
        let mut transparent = None;
        if need_transparent {
            transparent = Some((rgb.len() / 3) as u8);
            rgb.extend_from_slice(&[0, 0, 0]);
        }
        let opaque = rgb.len() / 3 - transparent.map(|_| 1).unwrap_or(0);
        let lut = if nq.is_none() { make_lut(&rgb, opaque) } else { Default::default() };
        Palette { nq, rgb, transparent, lut }
    }

    fn len(&self) -> usize {
        self.rgb.len() / 3
    }

    fn opaque_len(&self) -> usize {
        self.len() - self.transparent.map(|_| 1).unwrap_or(0)
    }

    #[inline]
    fn index_of(&self, r: u8, g: u8, b: u8) -> u8 {
        if let Some(nq) = &self.nq {
            nq.index_of(&[r, g, b, 255]) as u8
        } else if let Some(i) = self.lut.get(&key(r, g, b)) {
            *i
        } else {
            // exact / nearest search over small palette
            let mut best = 0usize;
            let mut bd = i32::MAX;
            for i in 0..self.opaque_len() {
                let pr = self.rgb[i * 3] as i32;
                let pg = self.rgb[i * 3 + 1] as i32;
                let pb = self.rgb[i * 3 + 2] as i32;
                let d = (pr - r as i32).pow(2) + (pg - g as i32).pow(2) + (pb - b as i32).pow(2);
                if d < bd {
                    bd = d;
                    best = i;
                    if d == 0 {
                        break;
                    }
                }
            }
            best as u8
        }
    }

    /// The palette as the graphics card searches it.
    fn gpu_table(&self) -> crate::gpu_render::QuantTable {
        match &self.nq {
            Some(nq) => {
                let entries: Vec<[i32; 4]> = nq.color_map_rgba().chunks_exact(4).map(|c| [c[0] as i32, c[1] as i32, c[2] as i32, c[3] as i32]).collect();
                // NeuQuant's index by green (its colour map is already sorted by green)
                let mut netindex = [0u32; 256];
                let (mut prev, mut start) = (0usize, 0usize);
                for (i, e) in entries.iter().enumerate() {
                    let g = e[1].clamp(0, 255) as usize;
                    if g != prev {
                        netindex[prev] = ((start + i) >> 1) as u32;
                        for v in netindex.iter_mut().take(g).skip(prev + 1) {
                            *v = i as u32;
                        }
                        prev = g;
                        start = i;
                    }
                }
                let max = entries.len().saturating_sub(1);
                netindex[prev] = ((start + max) >> 1) as u32;
                for v in netindex.iter_mut().skip(prev + 1) {
                    *v = max as u32;
                }
                crate::gpu_render::QuantTable { mode: 0, entries, netindex }
            }
            None => {
                let entries = (0..self.opaque_len()).map(|i| [self.rgb[i * 3] as i32, self.rgb[i * 3 + 1] as i32, self.rgb[i * 3 + 2] as i32, 255]).collect();
                crate::gpu_render::QuantTable { mode: 1, entries, netindex: [0; 256] }
            }
        }
    }

    fn color(&self, i: u8) -> [u8; 3] {
        let i = i as usize * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// Map a rectangle of an RGBA image to palette indices.
/// `mask[i] == false` → pixel is forced transparent.
fn map_region(
    img: &RgbaImage,
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    pal: &Palette,
    mask: Option<&[bool]>,
    thr: u8,
    dither: bool,
    ordered: bool,
) -> Vec<u8> {
    let mut out = vec![0u8; (w * h) as usize];
    let tidx = pal.transparent;
    if !dither {
        // the card maps big regions; small ones aren't worth the trip
        let gpu = if (w as u64 * h as u64) >= GPU_MAP_MIN && crate::gpu_render::enabled() {
            let t = pal.gpu_table();
            crate::gpu_render::with_shared_keep(|g| g.map_palette(img, x0, y0, w, h, &t, ordered)).flatten()
        } else {
            None
        };
        match gpu {
            Some(v) if v.len() == out.len() => out = v,
            _ => {
                out.par_chunks_mut(w as usize).enumerate().for_each(|(yy, row)| {
                    for (xx, o) in row.iter_mut().enumerate() {
                        let (x, y) = (x0 + xx as u32, y0 + yy as u32);
                        let p = img.get_pixel(x, y);
                        let [r, g, b] = if ordered { bayer(x, y, [p[0], p[1], p[2]]) } else { [p[0], p[1], p[2]] };
                        *o = pal.index_of(r, g, b);
                    }
                });
            }
        }
        // see-through pixels, and ones the mask leaves out
        if let Some(t) = tidx {
            out.par_chunks_mut(w as usize).enumerate().for_each(|(yy, row)| {
                for (xx, o) in row.iter_mut().enumerate() {
                    let i = yy * w as usize + xx;
                    let keep = mask.map(|m| m[i]).unwrap_or(true);
                    if !keep || img.get_pixel(x0 + xx as u32, y0 + yy as u32)[3] < thr {
                        *o = t;
                    }
                }
            });
        }
        return out;
    }
    // Floyd–Steinberg (sequential).
    let ww = w as usize;
    let mut err_cur = vec![[0i32; 3]; ww + 2];
    let mut err_next = vec![[0i32; 3]; ww + 2];
    for yy in 0..h as usize {
        for e in err_next.iter_mut() {
            *e = [0; 3];
        }
        for xx in 0..ww {
            let p = img.get_pixel(x0 + xx as u32, y0 + yy as u32);
            let i = yy * ww + xx;
            let keep = mask.map(|m| m[i]).unwrap_or(true);
            if (!keep || p[3] < thr) && tidx.is_some() {
                out[i] = tidx.unwrap();
                continue;
            }
            let e = err_cur[xx + 1];
            let r = (p[0] as i32 + e[0] / 16).clamp(0, 255) as u8;
            let g = (p[1] as i32 + e[1] / 16).clamp(0, 255) as u8;
            let b = (p[2] as i32 + e[2] / 16).clamp(0, 255) as u8;
            let idx = pal.index_of(r, g, b);
            out[i] = idx;
            let c = pal.color(idx);
            let d = [r as i32 - c[0] as i32, g as i32 - c[1] as i32, b as i32 - c[2] as i32];
            for k in 0..3 {
                err_cur[xx + 2][k] += d[k] * 7;
                err_next[xx][k] += d[k] * 3;
                err_next[xx + 1][k] += d[k] * 5;
                err_next[xx + 2][k] += d[k];
            }
        }
        std::mem::swap(&mut err_cur, &mut err_next);
    }
    out
}

/// Regions this many pixels or more are mapped on the graphics card.
const GPU_MAP_MIN: u64 = 1 << 16;

/// The ordered dither's 8×8 Bayer matrix.
const BAYER8: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

/// A pixel nudged by the ordered dither at (x, y), as gpu_quant.wgsl does
/// it: offsets are quarter steps, so rounding never lands on a half.
fn bayer(x: u32, y: u32, c: [u8; 3]) -> [u8; 3] {
    let d = (BAYER8[((y % 8) * 8 + x % 8) as usize] as f32 + 0.5) / 64.0 - 0.5;
    c.map(|v| (v as f32 + d * 32.0).round().clamp(0.0, 255.0) as u8)
}

fn same_image(a: &RgbaImage, b: &RgbaImage) -> bool {
    a.as_raw() == b.as_raw()
}

/// Every distinct opaque colour in the animation with its pixel count, most
/// frequent first. `None` when there are more than `cap` distinct colours.
fn colour_census(frames: &[Prepared], thr: u8, cap: usize) -> Option<Vec<([u8; 3], u64)>> {
    let mut map: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for f in frames {
        f.rgba.colour_counts(|c, n| {
            if c[3] >= thr {
                *map.entry(u32::from_le_bytes([c[0], c[1], c[2], 0])).or_insert(0) += n;
            }
        });
        if map.len() > cap {
            return None;
        }
    }
    let mut v: Vec<([u8; 3], u64)> = map.into_iter().map(|(k, n)| {
        let b = k.to_le_bytes();
        ([b[0], b[1], b[2]], n)
    }).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Some(v)
}

impl Palette {
    /// Exact palette from a colour list (no quantiser).
    fn exact(colours: &[[u8; 3]], need_transparent: bool) -> Palette {
        let mut rgb: Vec<u8> = colours.iter().flatten().copied().collect();
        if rgb.is_empty() {
            rgb.extend_from_slice(&[0, 0, 0]);
        }
        let mut transparent = None;
        if need_transparent {
            transparent = Some((rgb.len() / 3) as u8);
            rgb.extend_from_slice(&[0, 0, 0]);
        }
        let opaque = rgb.len() / 3 - transparent.map(|_| 1).unwrap_or(0);
        let lut = make_lut(&rgb, opaque);
        Palette { nq: None, rgb, transparent, lut }
    }

    /// True if every colour in `set` is an exact entry of this palette.
    fn contains_all(&self, set: &std::collections::HashSet<[u8; 3]>) -> bool {
        if self.nq.is_some() {
            return false;
        }
        set.iter().all(|c| self.lut.contains_key(&key(c[0], c[1], c[2])))
    }
}

/// Bytes a local colour table costs (padded to a power of two).
fn local_table_bytes(pal: &Palette) -> usize {
    3 * pal.len().max(2).next_power_of_two()
}


/// One way of writing a frame, with its encoded size.
struct Candidate<'a> {
    pal: &'a Palette,
    local: bool,
    masked: bool,
    wildcard: bool,
    data: Vec<u8>,
    indices: Vec<u8>,
}

/// Encode an animation to GIF bytes.
///
/// Lossless size optimisations (identical pixels, smaller file):
/// - an exact global colour table built from the animation's most common
///   colours; frames whose colours all fit it skip their local table
/// - per frame, the cheapest of: changed pixels with transparency vs. the whole
///   changed rectangle, and global vs. local table (each trial-encoded)
/// - local tables only as large as needed (smaller LZW code size)
/// - transparent animations: each frame trimmed to its visible area
pub fn encode_gif(anim: &Animation, opt: &GifOptions, progress: Option<&AtomicUsize>) -> anyhow::Result<Vec<u8>> {
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames to encode");
    let w = frames[0].image.width();
    let h = frames[0].image.height();
    anyhow::ensure!(w <= 65535 && h <= 65535, "Image too large for GIF");
    let thr = opt.alpha_threshold.max(1);

    // Merge duplicates.
    let mut prepared: Vec<Prepared> = Vec::with_capacity(frames.len());
    for f in &frames {
        if opt.merge_duplicates {
            if let Some(last) = prepared.last_mut() {
                if crate::pixels::Pixels::ptr_eq(&last.rgba, &f.image) || same_image(&last.rgba.get(), &f.image.get()) {
                    last.delay = last.delay.saturating_add(f.delay_cs);
                    continue;
                }
            }
        }
        prepared.push(Prepared { rgba: f.image.clone(), delay: f.delay_cs });
    }

    let transparent_anim = has_transparency(&frames, thr);
    // With real transparency we cannot use "keep previous frame" differencing:
    // each frame must clear the previous one ("don't stack frames").
    let use_diff = opt.frame_diff && !transparent_anim;
    let need_transparent_idx = transparent_anim || use_diff;
    let max_n = {
        // colours available when a transparent slot is reserved
        let n = opt.colors.clamp(2, 256) as usize;
        if need_transparent_idx { (n - 1).max(1) } else { n }
    };

    // Global colour table: exact when the animation's colours allow it.
    let census = colour_census(&prepared, thr, 1 << 16);
    let forced_global = opt.global_palette;
    let global: Option<Palette> = match (&census, forced_global) {
        (Some(c), _) if c.len() <= max_n => Some(Palette::exact(&c.iter().map(|x| x.0).collect::<Vec<_>>(), need_transparent_idx)),
        // exactly one colour too many for a transparent slot: keep it exact; only
        // frames written without transparency can use it
        (Some(c), _) if !transparent_anim && c.len() <= opt.colors.clamp(2, 256) as usize => Some(Palette::exact(&c.iter().map(|x| x.0).collect::<Vec<_>>(), false)),
        (_, true) => {
            // a frame unpacked at a time
            let total: usize = prepared.iter().map(|p| (p.rgba.width() * p.rgba.height()) as usize).sum::<usize>().max(1);
            let mut samples = Vec::new();
            for p in &prepared {
                let img = p.rgba.get();
                let share = 600_000 * (img.width() * img.height()) as usize / total;
                samples.extend(collect_samples(&[&*img], thr, share.max(1)));
            }
            Some(Palette::build(&samples, opt.colors, need_transparent_idx, opt.quant_speed))
        }
        // too many colours overall: the most common ones still serve frames that only use them
        (Some(c), false) => Some(Palette::exact(&c.iter().take(max_n).map(|x| x.0).collect::<Vec<_>>(), need_transparent_idx)),
        (None, false) => None,
    };

    let mut buf = Vec::new();
    {
        let gpal: &[u8] = global.as_ref().map(|p| p.rgb.as_slice()).unwrap_or(&[]);
        let mut enc = gif::Encoder::new(&mut buf, w as u16, h as u16, gpal)?;
        enc.set_repeat(match anim.loop_count {
            None => gif::Repeat::Infinite,
            Some(n) => gif::Repeat::Finite(n.saturating_sub(1)),
        })?;
        if opt.noai_tag {
            enc.write_raw_extension(gif::AnyExtension(0xFE), &[NOAI_COMMENT.as_bytes()])?;
        }

        // Source colour last painted at each pixel (RGB + "painted" flag), for differencing.
        // Comparing against the *source* (not the quantised output) avoids re-sending
        // pixels whose only difference is palette error, while bounding lossy drift.
        let tol = (opt.lossy as i32 * opt.lossy as i32) / 4; // squared distance tolerance
        let max_dist = opt.lossy as f32 / 2.0;

        // One frame, differenced against `canvas` (what earlier frames of its run painted).
        let encode_one = |fi: usize, pf: &Prepared, canvas: &mut Vec<[u8; 4]>| -> anyhow::Result<gif::Frame<'static>> {
            let img_arc = pf.rgba.get();
            let img = &*img_arc;
            let (mut x0, mut y0, mut x1, mut y1) = (0u32, 0u32, w - 1, h - 1);
            let mut mask: Option<Vec<bool>> = None;

            if use_diff && fi > 0 {
                let mut changed = vec![false; (w * h) as usize];
                let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
                for y in 0..h {
                    for x in 0..w {
                        let i = (y * w + x) as usize;
                        let p = img.get_pixel(x, y);
                        let c = canvas[i];
                        let d = (p[0] as i32 - c[0] as i32).pow(2) + (p[1] as i32 - c[1] as i32).pow(2) + (p[2] as i32 - c[2] as i32).pow(2);
                        if c[3] == 0 || d > tol {
                            changed[i] = true;
                            minx = minx.min(x);
                            miny = miny.min(y);
                            maxx = maxx.max(x);
                            maxy = maxy.max(y);
                        }
                    }
                }
                if minx == u32::MAX {
                    // Nothing changed: a 1x1 transparent frame holds the delay.
                    (x0, y0, x1, y1) = (0, 0, 0, 0);
                    changed[0] = false;
                } else {
                    (x0, y0, x1, y1) = (minx, miny, maxx, maxy);
                }
                let rw = x1 - x0 + 1;
                let rh = y1 - y0 + 1;
                let mut m = vec![false; (rw * rh) as usize];
                for yy in 0..rh {
                    for xx in 0..rw {
                        m[(yy * rw + xx) as usize] = changed[((y0 + yy) * w + x0 + xx) as usize];
                    }
                }
                mask = Some(m);
            } else if transparent_anim {
                // Each frame clears its own area afterwards, so only the visible
                // part needs storing.
                let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
                for (x, y, p) in img.enumerate_pixels() {
                    if p[3] >= thr {
                        minx = minx.min(x);
                        miny = miny.min(y);
                        maxx = maxx.max(x);
                        maxy = maxy.max(y);
                    }
                }
                if minx == u32::MAX {
                    (x0, y0, x1, y1) = (0, 0, 0, 0);
                } else {
                    (x0, y0, x1, y1) = (minx, miny, maxx, maxy);
                }
            }

            let rw = x1 - x0 + 1;
            let rh = y1 - y0 + 1;
            let nothing_changed = mask.as_ref().map(|m| !m.iter().any(|v| *v)).unwrap_or(false);

            // Colours each mode needs.
            let colours_of = |masked: bool| -> (std::collections::HashSet<[u8; 3]>, Vec<u8>) {
                let mut set = std::collections::HashSet::new();
                let mut samples = Vec::new();
                for yy in 0..rh {
                    for xx in 0..rw {
                        let keep = !masked || mask.as_ref().map(|m| m[(yy * rw + xx) as usize]).unwrap_or(true);
                        let p = img.get_pixel(x0 + xx, y0 + yy);
                        if keep && p[3] >= thr {
                            if set.len() <= 256 {
                                set.insert([p[0], p[1], p[2]]);
                            }
                            samples.extend_from_slice(&[p[0], p[1], p[2], 255]);
                        }
                    }
                }
                (set, samples)
            };

            // Build the candidate (palette, mode) list.
            let mut locals: Vec<(bool, Palette)> = Vec::new(); // (masked, palette)
            let mut plans: Vec<(bool, bool, Option<usize>, bool)> = Vec::new(); // (masked, local, local idx, wildcard)
            let modes: &[bool] = if mask.is_some() && !nothing_changed { &[true, false] } else { &[true] };
            for &masked in modes {
                let (set, samples) = colours_of(masked);
                let n_all = opt.colors.clamp(2, 256) as usize;
                let uses_t = transparent_anim || (masked && mask.is_some());
                let cap = if uses_t { n_all - 1 } else { n_all };
                let exact_fits = set.len() <= cap;
                let fits_global = global.as_ref().map(|g| (g.transparent.is_some() || !uses_t) && (forced_global || g.contains_all(&set))).unwrap_or(false);
                let global_wild = global.as_ref().map(|g| g.transparent.is_some()).unwrap_or(false);
                // whole-rectangle and wildcard variants only ever win with the
                // global table (measured), so they skip local-table trials
                if !masked && !fits_global {
                    continue;
                }
                if fits_global {
                    plans.push((masked, false, None, false));
                    if !masked && global_wild {
                        // transparent-or-own-colour for unchanged pixels
                        plans.push((true, false, None, true));
                    }
                }
                if !forced_global && masked {
                    let pal = if exact_fits {
                        let mut v: Vec<[u8; 3]> = set.into_iter().collect();
                        v.sort();
                        // keep a transparent slot when it fits, so the wildcard mode can use it
                        let slot = uses_t || (mask.is_some() && v.len() < n_all);
                        Palette::exact(&v, slot)
                    } else {
                        Palette::build(&samples, opt.colors, true, opt.quant_speed)
                    };
                    locals.push((masked, pal));
                    plans.push((masked, true, Some(locals.len() - 1), false));
                }
            }
            if plans.is_empty() {
                // forced global but nothing else fits: use it anyway (quantised)
                plans.push((true, false, None, false));
            }

            let mask_ref = mask.as_deref();
            let cands: Vec<Candidate> = plans
                .par_iter()
                .map(|&(masked, local, li, wildcard)| {
                    let pal: &Palette = match li {
                        Some(i) => &locals[i].1,
                        None => global.as_ref().unwrap(),
                    };
                    let m = if masked { mask_ref } else { None };
                    let indices = map_region(img, x0, y0, rw, rh, pal, m, thr, opt.dither, opt.ordered_dither);
                    let (data, indices) = if wildcard {
                        let full = map_region(img, x0, y0, rw, rh, pal, None, thr, opt.dither, opt.ordered_dither);
                        crate::lzw::encode_ex(&indices, Some(&full), &pal.rgb, pal.transparent, max_dist)
                    } else {
                        (crate::lzw::encode(&indices, &pal.rgb, pal.transparent, max_dist), indices)
                    };
                    Candidate { pal, local, masked, wildcard, data, indices }
                })
                .collect();
            let best = cands
                .into_iter()
                .min_by_key(|c| c.data.len() + if c.local { local_table_bytes(c.pal) } else { 0 })
                .unwrap();

            // Remember the source colour of every pixel we (re)painted.
            if use_diff {
                for yy in 0..rh {
                    for xx in 0..rw {
                        let idx = best.indices[(yy * rw + xx) as usize];
                        if Some(idx) != best.pal.transparent || !best.masked {
                            let _ = best.wildcard;
                            let p = img.get_pixel(x0 + xx, y0 + yy);
                            canvas[((y0 + yy) * w + x0 + xx) as usize] = [p[0], p[1], p[2], 255];
                        }
                    }
                }
            }

            let frame: gif::Frame<'static> = gif::Frame {
                delay: pf.delay,
                dispose: if use_diff { gif::DisposalMethod::Keep } else if transparent_anim { gif::DisposalMethod::Background } else { gif::DisposalMethod::Any },
                transparent: best.pal.transparent,
                needs_user_input: false,
                top: y0 as u16,
                left: x0 as u16,
                width: rw as u16,
                height: rh as u16,
                interlaced: false,
                palette: if best.local { Some(best.pal.rgb.clone()) } else { None },
                buffer: Cow::Owned(best.data),
            };
            if let Some(p) = progress {
                p.fetch_add(1, Ordering::Relaxed);
            }
            crate::stack::yield_to_foreground(progress);
            if crate::stack::stopped(progress) {
                anyhow::bail!("Stopped");
            }
            Ok(frame)
        };

        // Long animations are split into runs that each start with a whole frame,
        // so the runs encode in parallel. The run length is fixed so the file is
        // the same on every machine.
        const RUN: usize = 120;
        let runs: Vec<&[Prepared]> = if prepared.len() > RUN * 3 / 2 { prepared.chunks(RUN).collect() } else { vec![&prepared[..]] };
        let encoded: Vec<anyhow::Result<Vec<gif::Frame<'static>>>> = runs
            .par_iter()
            .map(|run| {
                let mut canvas: Vec<[u8; 4]> = vec![[0, 0, 0, 0]; (w * h) as usize];
                run.iter().enumerate().map(|(fi, pf)| encode_one(fi, pf, &mut canvas)).collect()
            })
            .collect();
        for run in encoded {
            for frame in run? {
                enc.write_lzw_pre_encoded_frame(&frame)?;
            }
        }
    }
    Ok(buf)
}

/// What a GIF file was saved with, read from its blocks without decoding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GifInfo {
    /// largest colour table used
    pub colors: u16,
    /// every frame uses the file's one palette
    pub global_only: bool,
    pub has_comment: bool,
    pub frames: usize,
}

/// Walk a GIF's blocks. None when it isn't a readable GIF.
pub fn gif_info(b: &[u8]) -> Option<GifInfo> {
    if b.len() < 13 || &b[..3] != b"GIF" {
        return None;
    }
    let mut info = GifInfo { global_only: true, ..Default::default() };
    let table = |packed: u8| if packed & 0x80 != 0 { Some(2usize << (packed & 7)) } else { None };
    let mut p = 13;
    let global = table(b[10]);
    if let Some(n) = global {
        p += n * 3;
        info.colors = n as u16;
    }
    // skip a chain of sub-blocks
    let skip = |mut p: usize| -> Option<usize> {
        loop {
            let n = *b.get(p)? as usize;
            p += 1 + n;
            if n == 0 {
                return Some(p);
            }
        }
    };
    loop {
        match *b.get(p)? {
            0x21 => {
                if *b.get(p + 1)? == 0xFE {
                    info.has_comment = true;
                }
                p = skip(p + 2)?;
            }
            0x2C => {
                let packed = *b.get(p + 9)?;
                p += 10;
                match table(packed) {
                    Some(n) => {
                        p += n * 3;
                        info.global_only = false;
                        info.colors = info.colors.max(n as u16);
                    }
                    None if global.is_none() => info.global_only = false,
                    None => {}
                }
                p = skip(p + 1)?;
                info.frames += 1;
            }
            0x3B => break,
            _ => return None,
        }
    }
    (info.frames > 0).then_some(info)
}

impl GifOptions {
    /// Settings close to how a GIF was saved, so saving it again stays near
    /// its size.
    pub fn like(info: &GifInfo) -> GifOptions {
        GifOptions { colors: info.colors.clamp(2, 256), global_palette: info.global_only, ..GifOptions::default() }
    }
}

/// Add the "noai" comment to GIF bytes, right after the header.
pub fn tag_gif(b: Vec<u8>) -> Vec<u8> {
    let Some(info) = gif_info(&b) else { return b };
    if info.has_comment && b.windows(NOAI_COMMENT.len()).any(|w| w == NOAI_COMMENT.as_bytes()) {
        return b;
    }
    let mut p = 13;
    if b[10] & 0x80 != 0 {
        p += (2usize << (b[10] & 7)) * 3;
    }
    let mut out = Vec::with_capacity(b.len() + NOAI_COMMENT.len() + 8);
    out.extend_from_slice(&b[..p]);
    out.extend_from_slice(&[0x21, 0xFE]);
    for c in NOAI_COMMENT.as_bytes().chunks(255) {
        out.push(c.len() as u8);
        out.extend_from_slice(c);
    }
    out.push(0);
    out.extend_from_slice(&b[p..]);
    out
}

/// The card picks exactly the palette entries the CPU does, NeuQuant and
/// exact palettes, plain and with the ordered dither; and how long each takes.
#[cfg(test)]
#[test]
#[ignore]
fn gpu_mapping_matches_cpu() {
    let Some(mut gpu) = crate::gpu_render::GpuRenderer::headless() else {
        println!("no GPU adapter");
        return;
    };
    let (w, h) = (1280u32, 720u32);
    let img = RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, (((x ^ y) * 7) % 256) as u8, 255]));
    let samples: Vec<u8> = img.pixels().step_by(41).flat_map(|p| p.0).collect();
    let nq_pal = Palette::build(&samples, 256, true, 10);
    let few: Vec<[u8; 3]> = (0..40u8).map(|i| [i * 6, 255 - i * 6, (i * 37) % 255]).collect();
    let exact = Palette::exact(&few, false);
    for (name, pal) in [("neuquant", &nq_pal), ("exact", &exact)] {
        for ordered in [false, true] {
            let t0 = std::time::Instant::now();
            let cpu = map_region(&img, 13, 7, w - 20, h - 9, pal, None, 128, false, ordered);
            let tc = t0.elapsed();
            let t = pal.gpu_table();
            let t1 = std::time::Instant::now();
            let g = gpu.map_palette(&img, 13, 7, w - 20, h - 9, &t, ordered).expect("gpu map");
            let tg = t1.elapsed();
            let diff = cpu.iter().zip(&g).filter(|(a, b)| a != b).count();
            println!("{name} ordered={ordered}: {diff} differ of {}  cpu {:?} gpu {:?}", cpu.len(), tc, tg);
            assert_eq!(diff, 0);
        }
    }
}
