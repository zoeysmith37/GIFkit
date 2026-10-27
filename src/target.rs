//! "Fit to file size": search encoder settings (and, if needed, a smaller
//! scale) for the best-looking result under a byte budget.

use crate::anim::{Animation, Frame};
use crate::encode::{encode_gif, GifOptions};
use crate::io::{encode_avif, encode_webp, WebpOptions};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum TargetFormat {
    Gif(GifOptions),
    Webp(WebpOptions),
    Avif(u8),
}

/// One step on the quality ladder (index 0 = best quality).
#[derive(Clone, Debug)]
enum Rung {
    Gif(GifOptions, f32),
    Webp(WebpOptions, f32),
    Avif(u8, f32),
}

impl Rung {
    fn scale(&self) -> f32 {
        match self {
            Rung::Gif(_, s) | Rung::Webp(_, s) | Rung::Avif(_, s) => *s,
        }
    }
    fn describe(&self) -> String {
        let sc = self.scale();
        let scale = if sc < 0.999 { format!(", {:.0}% size", sc * 100.0) } else { String::new() };
        match self {
            Rung::Gif(o, _) => format!("lossy {}, {} colours{scale}", o.lossy, o.colors),
            Rung::Webp(o, _) => if o.lossless { format!("lossless{scale}") } else { format!("quality {:.0}{scale}", o.quality) },
            Rung::Avif(q, _) => format!("quality {q}{scale}"),
        }
    }
}

const SCALES: [f32; 8] = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.33, 0.25];

fn ladder(fmt: &TargetFormat) -> Vec<Rung> {
    let mut v = Vec::new();
    match fmt {
        TargetFormat::Gif(base) => {
            // Order is "how bad it looks": moderate lossy first, then smaller
            // frames (looks far better than heavy lossy, especially on video),
            // and only at the smallest sizes heavier lossy / fewer colours.
            const SOFT: u16 = 80;
            v.push(Rung::Gif(base.clone(), 1.0));
            for l in [20u16, 40, 60, SOFT] {
                if l > base.lossy {
                    v.push(Rung::Gif(GifOptions { lossy: l, ..base.clone() }, 1.0));
                }
            }
            let soft = base.lossy.max(SOFT);
            for s in SCALES {
                v.push(Rung::Gif(GifOptions { lossy: soft, ..base.clone() }, s));
            }
            let last = *SCALES.last().unwrap();
            for l in [120u16, 160, 200] {
                if l > soft {
                    v.push(Rung::Gif(GifOptions { lossy: l, ..base.clone() }, last));
                }
            }
            for c in [128u16, 64, 32] {
                if c < base.colors {
                    v.push(Rung::Gif(GifOptions { lossy: soft.max(200), colors: c, ..base.clone() }, last));
                }
            }
        }
        TargetFormat::Webp(base) => {
            if base.lossless {
                v.push(Rung::Webp(*base, 1.0));
            }
            let start = if base.lossless { 95.0 } else { base.quality };
            let mut q = start;
            while q >= 5.0 {
                v.push(Rung::Webp(WebpOptions { lossless: false, quality: q, ..*base }, 1.0));
                q -= 7.5;
            }
            for s in SCALES {
                v.push(Rung::Webp(WebpOptions { lossless: false, quality: 60.0, ..*base }, s));
            }
        }
        TargetFormat::Avif(base) => {
            let mut q = *base as i32;
            while q >= 0 {
                v.push(Rung::Avif(q as u8, 1.0));
                q -= 8;
            }
            for s in SCALES {
                v.push(Rung::Avif(40, s));
            }
        }
    }
    v
}

/// Quality-only ladder (frame size fixed): every lossy / colour step at 100%.
fn locked_ladder(fmt: &TargetFormat) -> Vec<Rung> {
    let mut v = Vec::new();
    match fmt {
        TargetFormat::Gif(base) => {
            v.push(Rung::Gif(base.clone(), 1.0));
            for l in [20u16, 40, 60, 80, 100, 130, 160, 200] {
                if l > base.lossy {
                    v.push(Rung::Gif(GifOptions { lossy: l, ..base.clone() }, 1.0));
                }
            }
            let top = base.lossy.max(200);
            for c in [192u16, 128, 96, 64, 48, 32, 16] {
                if c < base.colors {
                    v.push(Rung::Gif(GifOptions { lossy: top, colors: c, ..base.clone() }, 1.0));
                }
            }
        }
        _ => {
            v = ladder(fmt).into_iter().filter(|r| r.scale() >= 0.999).collect();
        }
    }
    v
}

/// Scale every frame (Lanczos-ish triangle filter, alpha kept).
pub fn scaled(anim: &Animation, s: f32) -> Animation {
    if s >= 0.999 {
        return anim.clone();
    }
    use rayon::prelude::*;
    let (w, h) = (anim.width(), anim.height());
    let nw = ((w as f32 * s).round() as u32).max(1);
    let nh = ((h as f32 * s).round() as u32).max(1);
    let frames = anim
        .frames
        .par_iter()
        .map(|f| Frame { image: crate::pixels::Pixels::new(image::imageops::resize(&*f.image.get(), nw, nh, image::imageops::FilterType::CatmullRom)), delay_cs: f.delay_cs, enabled: f.enabled, cut: f.cut, blank: f.blank, trim: f.trim })
        .collect();
    Animation { frames, loop_count: anim.loop_count }
}

/// Result of a fit: bytes, a short description of the settings used, and
/// whether the target was actually reached.
pub struct Fit {
    pub bytes: Vec<u8>,
    pub settings: String,
    pub reached: bool,
    /// settings and frame scale that produced it
    pub gif: Option<GifOptions>,
    pub webp: Option<WebpOptions>,
    pub avif: Option<u8>,
    pub scale: f32,
}

fn make_fit(rungs: &[Rung], i: usize, bytes: Vec<u8>, reached: bool) -> Fit {
    let r = &rungs[i];
    let (mut gif, mut webp, mut avif) = (None, None, None);
    match r {
        Rung::Gif(o, _) => gif = Some(o.clone()),
        Rung::Webp(o, _) => webp = Some(*o),
        Rung::Avif(q, _) => avif = Some(*q),
    }
    Fit { bytes, settings: r.describe(), reached, gif, webp, avif, scale: r.scale() }
}

/// Find the highest-quality settings whose output is at most `max_bytes`.
/// Binary search over a quality ladder (sizes are close to monotonic).
#[cfg_attr(not(test), allow(dead_code))]
pub fn fit(anim: &Animation, fmt: &TargetFormat, max_bytes: u64, trials: &AtomicUsize) -> Result<Fit> {
    fit_with(fmt, max_bytes, true, trials, |s| scaled(anim, s))
}

/// [`fit`] where `render(scale)` produces the frames at a given scale – lets the
/// app render through its real resize step so results match the preview.
/// With `allow_scale` false, only compression settings change, never the frame size.
pub fn fit_with(fmt: &TargetFormat, max_bytes: u64, allow_scale: bool, trials: &AtomicUsize, render: impl Fn(f32) -> Animation) -> Result<Fit> {
    fit_from(fmt, max_bytes, allow_scale, trials, None, render)
}

/// [`fit_with`], given the file the current settings already make (the
/// preview's), so it isn't made again.
pub fn fit_from(fmt: &TargetFormat, max_bytes: u64, allow_scale: bool, trials: &AtomicUsize, have: Option<Vec<u8>>, render: impl Fn(f32) -> Animation) -> Result<Fit> {
    let mut rungs = ladder(fmt);
    if !allow_scale {
        rungs = locked_ladder(fmt);
    }
    let mut scaled_cache: HashMap<u32, Arc<Animation>> = HashMap::new();
    let mut get = |sc: f32| -> Arc<Animation> { scaled_cache.entry((sc * 1000.0) as u32).or_insert_with(|| Arc::new(render(sc))).clone() };
    let debug = std::env::var_os("GIFKIT_FIT_DEBUG").is_some();
    let encode = |r: &Rung, a: &Animation| -> Result<Vec<u8>> {
        trials.fetch_add(1, Ordering::Relaxed);
        let t = std::time::Instant::now();
        let b = match r {
            Rung::Gif(o, _) => encode_gif(a, o, None)?,
            Rung::Webp(o, _) => encode_webp(a, o)?,
            Rung::Avif(q, _) => encode_avif(a, *q)?,
        };
        if debug {
            eprintln!("  encode {} ({} frames): {} bytes in {:.2}s", r.describe(), a.frames.len(), b.len(), t.elapsed().as_secs_f32());
        }
        Ok(b)
    };
    let max = max_bytes as usize;
    let last = rungs.len() - 1;
    let mut full: HashMap<usize, Vec<u8>> = HashMap::new();
    if let Some(b) = have {
        full.insert(0, b);
    }
    let first = get(rungs[0].scale());
    let picks = sample_frames(first.frames.len());
    let found = match &picks {
        // short: every try is the real thing
        None => search(last, max, |i| {
            if let Some(b) = full.get(&i) {
                return Ok(b.len());
            }
            let b = encode(&rungs[i], &get(rungs[i].scale()))?;
            let n = b.len();
            full.insert(i, b);
            Ok(n)
        })?,
        // long: search on a sample, scaled up, then check with the real thing
        Some(picks) => {
            let part = |a: &Animation| Animation { frames: picks.iter().filter_map(|&k| a.frames.get(k).cloned()).collect(), loop_count: a.loop_count };
            // the real size at the best settings, and the sample's: how far
            // the sample is out, to correct every other guess by
            let real0 = match full.get(&0) {
                Some(b) => b.len(),
                None => {
                    let b = encode(&rungs[0], &first)?;
                    let n = b.len();
                    full.insert(0, b);
                    n
                }
            };
            let sample = part(&first);
            let sample0 = encode(&rungs[0], &sample)?.len().max(1);
            let ratio = real0 as f64 / sample0 as f64;
            let mut est: HashMap<usize, usize> = HashMap::new();
            let guess = search(last, max, |i| {
                if let Some(&n) = est.get(&i) {
                    return Ok(n);
                }
                // the real size where it's known
                if let Some(b) = full.get(&i) {
                    return Ok(b.len());
                }
                // only the sampled frames are scaled, not the whole thing
                let sc = rungs[i].scale() / rungs[0].scale();
                let a = scaled(&sample, sc);
                let n = (encode(&rungs[i], &a)?.len() as f64 * ratio) as usize;
                est.insert(i, n);
                Ok(n)
            })?;
            let mut i = match guess {
                Found::Fits(i) => i,
                Found::Never => last,
            };
            let mut size = |i: usize, full: &mut HashMap<usize, Vec<u8>>| -> Result<usize> {
                if let Some(b) = full.get(&i) {
                    return Ok(b.len());
                }
                let b = encode(&rungs[i], &get(rungs[i].scale()))?;
                let n = b.len();
                full.insert(i, b);
                Ok(n)
            };
            if size(i, &mut full)? <= max {
                // the sample can guess big: one better rung may fit too
                // (unless the sample put it well over)
                while i > 0 && est.get(&(i - 1)).is_none_or(|&e| e as f64 <= max as f64 * 1.03) && size(i - 1, &mut full)? <= max {
                    i -= 1;
                }
                Found::Fits(i)
            } else {
                // it guessed small: step on until it fits
                loop {
                    if i == last {
                        break Found::Never;
                    }
                    i += 1;
                    if size(i, &mut full)? <= max {
                        break Found::Fits(i);
                    }
                }
            }
        }
    };
    let (i, reached) = match found {
        Found::Fits(i) => (i, true),
        Found::Never => (last, false),
    };
    let b = match full.remove(&i) {
        Some(b) => b,
        None => encode(&rungs[i], &get(rungs[i].scale()))?,
    };
    Ok(make_fit(&rungs, i, b, reached))
}

/// For a long animation, the frames a search tries: three runs of eight in
/// a row (so frame-to-frame savings show), from the start, middle and end.
fn sample_frames(n: usize) -> Option<Vec<usize>> {
    const RUN: usize = 8;
    if n < 60 || std::env::var_os("GIFKIT_FIT_FULL").is_some() {
        return None;
    }
    let mut v = Vec::new();
    for start in [0, n / 2 - RUN / 2, n - RUN] {
        v.extend(start..start + RUN);
    }
    Some(v)
}

enum Found {
    /// the first rung (best quality) that fits, as far as the search can tell
    Fits(usize),
    /// not even the last rung fits
    Never,
}

/// Find where rungs `0..=last` go from too big to fitting under `max`.
/// The best rung is tried first, then the smallest (cheap: tiny frames),
/// then each try is aimed by drawing a line through the log sizes of the
/// two nearest known rungs. When that stops closing in fast, it halves.
fn search(last: usize, max: usize, mut size: impl FnMut(usize) -> Result<usize>) -> Result<Found> {
    let s0 = size(0)?;
    if s0 <= max {
        return Ok(Found::Fits(0));
    }
    if last == 0 {
        return Ok(Found::Never);
    }
    let sl = size(last)?;
    if sl > max {
        return Ok(Found::Never);
    }
    // invariant: lo too big, hi fits
    let (mut lo, mut hi, mut slo, mut shi) = (0usize, last, s0 as f64, sl as f64);
    let target = (max as f64).ln();
    let mut halve = false;
    while hi - lo > 1 {
        let t = if halve || slo <= shi {
            (lo + hi) / 2
        } else {
            let f = (slo.ln() - target) / (slo.ln() - shi.ln());
            // aim just past the crossing: the first rung that fits
            let t = lo as f64 + f * (hi - lo) as f64;
            (t.ceil() as usize).clamp(lo + 1, hi - 1)
        };
        let width = hi - lo;
        let st = size(t)? as f64;
        if st <= max as f64 {
            hi = t;
            shi = st;
        } else {
            lo = t;
            slo = st;
        }
        // a try that took off less than half the gap: halve next time
        halve = (hi - lo) * 2 > width;
    }
    Ok(Found::Fits(hi))
}


#[cfg(test)]
mod tests {
    use super::*;

    /// FILE=some.gif: how long fitting takes at a few targets.
    #[test]
    #[ignore]
    fn fit_timing() {
        let a = crate::io::load_animation(std::path::Path::new(&std::env::var("FILE").unwrap())).unwrap();
        let fmt = TargetFormat::Gif(GifOptions::default());
        let t = std::time::Instant::now();
        let have = encode_gif(&a, &GifOptions::default(), None).unwrap();
        eprintln!("one full encode: {:.1}s", t.elapsed().as_secs_f32());
        let base = have.len() as f64;
        for k in [0.8, 0.5, 0.3, 0.15] {
            let trials = AtomicUsize::new(0);
            let t = std::time::Instant::now();
            // as the app does: the preview's file is already there
            let f = fit_from(&fmt, (base * k) as u64, true, &trials, Some(have.clone()), |s| scaled(&a, s)).unwrap();
            eprintln!("{k}: {:.1}s {} tries -> {} ({}, fits {})", t.elapsed().as_secs_f32(), trials.load(Ordering::Relaxed), f.settings, f.bytes.len(), f.reached);
        }
    }

    /// The old way, for comparison: best rung, last rung, then halving.
    fn halving(last: usize, max: usize, mut size: impl FnMut(usize) -> usize) -> Option<usize> {
        if size(0) <= max {
            return Some(0);
        }
        if size(last) > max {
            return None;
        }
        let (mut lo, mut hi) = (0, last);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if size(mid) <= max {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some(hi)
    }

    /// The search lands where too big turns into fits, doing less work
    /// (pixels encoded) than halving.
    #[test]
    fn search_finds_the_line_quickly() {
        let a = crate::io::load_animation(std::path::Path::new("testdata/test.gif")).unwrap();
        let fmt = TargetFormat::Gif(GifOptions::default());
        let rungs = ladder(&fmt);
        let sizes: Vec<usize> = rungs
            .iter()
            .map(|r| match r {
                Rung::Gif(o, s) => encode_gif(&scaled(&a, *s), o, None).unwrap().len(),
                _ => unreachable!(),
            })
            .collect();
        // and made-up size lines: smooth, steep, stepped
        let n = rungs.len();
        let lines: Vec<(Vec<usize>, Vec<f32>)> = vec![
            (sizes.clone(), rungs.iter().map(|r| r.scale()).collect()),
            ((0..n).map(|i| (1_000_000.0 * 0.85f64.powi(i as i32)) as usize).collect(), vec![1.0; n]),
            ((0..n).map(|i| 1_000_000 - i * 40_000).collect(), vec![1.0; n]),
            ((0..n).map(|i| 1_000_000 >> (i / 4)).collect(), vec![1.0; n]),
        ];
        let (mut new_work, mut old_work) = (0.0, 0.0);
        for (sz, sc) in &lines {
            for k in [0.97, 0.8, 0.6, 0.4, 0.25, 0.12] {
                let max = (sz[0] as f64 * k) as usize;
                let mut w = 0.0;
                let got = search(n - 1, max, |i| {
                    w += (sc[i] * sc[i]) as f64;
                    Ok(sz[i])
                })
                .unwrap();
                let mut w2 = 0.0;
                let want = halving(n - 1, max, |i| {
                    w2 += (sc[i] * sc[i]) as f64;
                    sz[i]
                });
                match got {
                    Found::Fits(i) => {
                        assert!(sz[i] <= max, "{k}: rung {i} doesn't fit");
                        assert!(i == 0 || sz[i - 1] > max, "{k}: rung {} fits too", i - 1);
                    }
                    Found::Never => assert!(want.is_none()),
                }
                new_work += w;
                old_work += w2;
            }
        }
        eprintln!("work: new {new_work:.1} vs halving {old_work:.1}");
        assert!(new_work < old_work * 0.85, "new {new_work:.1} vs halving {old_work:.1}");
    }
}
