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
        .map(|f| Frame { image: Arc::new(image::imageops::resize(&*f.image, nw, nh, image::imageops::FilterType::CatmullRom)), delay_cs: f.delay_cs, enabled: f.enabled })
        .collect();
    Animation { frames, loop_count: anim.loop_count }
}

/// Result of a fit: bytes, a short description of the settings used, and
/// whether the target was actually reached.
pub struct Fit {
    pub bytes: Vec<u8>,
    pub settings: String,
    pub reached: bool,
    /// GIF settings and frame scale that produced it (GIF targets only)
    pub gif: Option<GifOptions>,
    pub scale: f32,
}

fn make_fit(rungs: &[Rung], i: usize, bytes: Vec<u8>, reached: bool) -> Fit {
    let r = &rungs[i];
    let gif = match r {
        Rung::Gif(o, _) => Some(o.clone()),
        _ => None,
    };
    Fit { bytes, settings: r.describe(), reached, gif, scale: r.scale() }
}

/// Find the highest-quality settings whose output is at most `max_bytes`.
/// Binary search over a quality ladder (sizes are close to monotonic).
pub fn fit(anim: &Animation, fmt: &TargetFormat, max_bytes: u64, trials: &AtomicUsize) -> Result<Fit> {
    let rungs = ladder(fmt);
    let mut scaled_cache: HashMap<u32, Arc<Animation>> = HashMap::new();
    let mut results: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut eval = |i: usize| -> Result<usize> {
        if let Some(b) = results.get(&i) {
            return Ok(b.len());
        }
        let r = &rungs[i];
        let key = (r.scale() * 1000.0) as u32;
        let a = scaled_cache.entry(key).or_insert_with(|| Arc::new(scaled(anim, r.scale()))).clone();
        let b = match r {
            Rung::Gif(o, _) => encode_gif(&a, o, None)?,
            Rung::Webp(o, _) => encode_webp(&a, o)?,
            Rung::Avif(q, _) => encode_avif(&a, *q)?,
        };
        trials.fetch_add(1, Ordering::Relaxed);
        let n = b.len();
        results.insert(i, b);
        Ok(n)
    };
    let max = max_bytes as usize;
    if eval(0)? <= max {
        let b = results.remove(&0).unwrap();
        return Ok(make_fit(&rungs, 0, b, true));
    }
    let last = rungs.len() - 1;
    if eval(last)? > max {
        let b = results.remove(&last).unwrap();
        return Ok(make_fit(&rungs, last, b, false));
    }
    // invariant: lo too big, hi fits
    let (mut lo, mut hi) = (0usize, last);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if eval(mid)? <= max {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let b = results.remove(&hi).unwrap();
    Ok(make_fit(&rungs, hi, b, true))
}
