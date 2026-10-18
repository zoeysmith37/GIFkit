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
    rgba: RgbaImage,
    delay: u16,
}

fn has_transparency(frames: &[Frame], thr: u8) -> bool {
    frames.par_iter().any(|f| f.image.pixels().any(|p| p[3] < thr))
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
) -> Vec<u8> {
    let mut out = vec![0u8; (w * h) as usize];
    let tidx = pal.transparent;
    if !dither {
        out.par_chunks_mut(w as usize).enumerate().for_each(|(yy, row)| {
            for xx in 0..w as usize {
                let p = img.get_pixel(x0 + xx as u32, y0 + yy as u32);
                let i = yy * w as usize + xx;
                let keep = mask.map(|m| m[i]).unwrap_or(true);
                row[xx] = if !keep || p[3] < thr {
                    tidx.unwrap_or_else(|| pal.index_of(p[0], p[1], p[2]))
                } else {
                    pal.index_of(p[0], p[1], p[2])
                };
            }
        });
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

fn same_image(a: &RgbaImage, b: &RgbaImage) -> bool {
    a.as_raw() == b.as_raw()
}

/// Every distinct opaque colour in the animation with its pixel count, most
/// frequent first. `None` when there are more than `cap` distinct colours.
fn colour_census(frames: &[Prepared], thr: u8, cap: usize) -> Option<Vec<([u8; 3], u64)>> {
    let mut map: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for f in frames {
        for p in f.rgba.pixels() {
            if p[3] >= thr {
                *map.entry(u32::from_le_bytes([p[0], p[1], p[2], 0])).or_insert(0) += 1;
            }
        }
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
                if same_image(&last.rgba, &f.image) {
                    last.delay = last.delay.saturating_add(f.delay_cs);
                    continue;
                }
            }
        }
        prepared.push(Prepared { rgba: (*f.image).clone(), delay: f.delay_cs });
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
            let imgs: Vec<&RgbaImage> = prepared.iter().map(|p| &p.rgba).collect();
            let samples = collect_samples(&imgs, thr, 600_000);
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
        let mut canvas: Vec<[u8; 4]> = vec![[0, 0, 0, 0]; (w * h) as usize];
        let tol = (opt.lossy as i32 * opt.lossy as i32) / 4; // squared distance tolerance
        let max_dist = opt.lossy as f32 / 2.0;

        for (fi, pf) in prepared.iter().enumerate() {
            let img = &pf.rgba;
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
                    let indices = map_region(img, x0, y0, rw, rh, pal, m, thr, opt.dither);
                    let (data, indices) = if wildcard {
                        let full = map_region(img, x0, y0, rw, rh, pal, None, thr, opt.dither);
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

            let frame = gif::Frame {
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
            enc.write_lzw_pre_encoded_frame(&frame)?;
            if let Some(p) = progress {
                p.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(buf)
}
