//! Layer styles, as in Photoshop: a stroke round the outside, a drop
//! shadow and an outer glow, drawn under a layer's own pixels. They work on
//! the layer's prepared picture, so the graphics card and the processor
//! draw the same thing.

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub on: bool,
    pub color: [u8; 3],
    /// px
    pub size: f32,
    /// %
    pub opacity: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { on: false, color: [0, 0, 0], size: 3.0, opacity: 100.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub on: bool,
    pub color: [u8; 3],
    /// %
    pub opacity: f32,
    /// where the light comes from, degrees (120: upper left)
    pub angle: f32,
    /// px
    pub distance: f32,
    /// softness, px
    pub size: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { on: false, color: [0, 0, 0], opacity: 75.0, angle: 120.0, distance: 5.0, size: 5.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glow {
    pub on: bool,
    pub color: [u8; 3],
    /// %
    pub opacity: f32,
    /// px
    pub size: f32,
    /// how much of the glow is solid before it fades, %
    pub spread: f32,
}

impl Default for Glow {
    fn default() -> Self {
        Glow { on: false, color: [255, 236, 120], opacity: 90.0, size: 10.0, spread: 20.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerStyle {
    pub stroke: Stroke,
    pub shadow: Shadow,
    pub glow: Glow,
}

impl LayerStyle {
    pub fn is_none(&self) -> bool {
        !self.stroke.on && !self.shadow.on && !self.glow.on
    }

    /// How much room the style takes round the picture, px at `scale`.
    pub fn pad(&self, scale: f32) -> u32 {
        let mut p: f32 = 0.0;
        if self.stroke.on {
            p = p.max(self.stroke.size * scale + 1.0);
        }
        if self.shadow.on {
            p = p.max((self.shadow.distance + self.shadow.size * 2.0) * scale + 2.0);
        }
        if self.glow.on {
            p = p.max(self.glow.size * scale + 2.0);
        }
        p.ceil().min(1024.0) as u32
    }

    /// The picture with its style under it, grown by `pad(scale)` on every
    /// side. Straight alpha in and out.
    pub fn apply(&self, sp: &RgbaImage, scale: f32) -> RgbaImage {
        let pad = self.pad(scale);
        let (w, h) = (sp.width() + 2 * pad, sp.height() + 2 * pad);
        let n = (w * h) as usize;
        // the layer's coverage on the bigger canvas
        let mut a = vec![0f32; n];
        for (x, y, p) in sp.enumerate_pixels() {
            a[((y + pad) * w + x + pad) as usize] = p[3] as f32 / 255.0;
        }
        // premultiplied colour, built bottom to top
        let mut out = vec![[0f32; 4]; n];
        let mut over = |cov: &[f32], col: [u8; 3], op: f32| {
            let c = col.map(|v| v as f32 / 255.0);
            for (o, &k) in out.iter_mut().zip(cov) {
                let s = (k * op).clamp(0.0, 1.0);
                if s <= 0.0 {
                    continue;
                }
                for ch in 0..3 {
                    o[ch] = c[ch] * s + o[ch] * (1.0 - s);
                }
                o[3] = s + o[3] * (1.0 - s);
            }
        };
        if self.shadow.on {
            let s = &self.shadow;
            let r = s.size * scale;
            let mut cov = blur(&a, w as usize, h as usize, r);
            // the light at `angle` throws the shadow the other way
            let t = s.angle.to_radians();
            let (dx, dy) = ((-t.cos() * s.distance * scale).round() as i64, (t.sin() * s.distance * scale).round() as i64);
            cov = shift(&cov, w as usize, h as usize, dx, dy);
            over(&cov, s.color, s.opacity / 100.0);
        }
        if self.glow.on {
            let g = &self.glow;
            let r = (g.size * scale).max(1.0);
            // solid out to the spread, then fading to nothing at the size
            let solid = r * (g.spread / 100.0).clamp(0.0, 1.0);
            let d = distance(&a, w as usize, h as usize);
            let cov: Vec<f32> = d.iter().zip(&a).map(|(&dd, &av)| (1.0 - ((dd - solid) / (r - solid).max(1.0)).clamp(0.0, 1.0)).powf(1.2).max(av)).collect();
            over(&cov, g.color, g.opacity / 100.0);
        }
        if self.stroke.on {
            let st = &self.stroke;
            let cov = dilate(&a, w as usize, h as usize, st.size * scale);
            over(&cov, st.color, st.opacity / 100.0);
        }
        // the layer itself on top
        let mut img = RgbaImage::new(w, h);
        for (x, y, p) in sp.enumerate_pixels() {
            let i = ((y + pad) * w + x + pad) as usize;
            let s = p[3] as f32 / 255.0;
            let o = &mut out[i];
            for ch in 0..3 {
                o[ch] = p[ch] as f32 / 255.0 * s + o[ch] * (1.0 - s);
            }
            o[3] = s + o[3] * (1.0 - s);
        }
        for (i, px) in img.pixels_mut().enumerate() {
            let o = out[i];
            if o[3] <= 1e-4 {
                continue;
            }
            let un = |v: f32| ((v / o[3]).clamp(0.0, 1.0) * 255.0).round() as u8;
            *px = Rgba([un(o[0]), un(o[1]), un(o[2]), (o[3].clamp(0.0, 1.0) * 255.0).round() as u8]);
        }
        img
    }
}

/// Moved by (dx, dy), empty where nothing comes in.
fn shift(v: &[f32], w: usize, h: usize, dx: i64, dy: i64) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    for y in 0..h as i64 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i64 {
            continue;
        }
        for x in 0..w as i64 {
            let sx = x - dx;
            if sx >= 0 && sx < w as i64 {
                out[(y as usize) * w + x as usize] = v[(sy as usize) * w + sx as usize];
            }
        }
    }
    out
}

/// A soft blur close to a gaussian of radius `r`: three box blurs.
pub fn blur(v: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    if r < 0.5 {
        return v.to_vec();
    }
    // three boxes whose spread adds up to a gaussian of sigma r/2
    let sigma = r / 2.0;
    let wi = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).floor().max(1.0) as usize;
    let rad = (wi.saturating_sub(1)) / 2;
    let mut a = v.to_vec();
    let mut b = vec![0f32; v.len()];
    for _ in 0..3 {
        box_h(&a, &mut b, w, h, rad.max(1));
        box_v(&b, &mut a, w, h, rad.max(1));
    }
    a
}

fn box_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let k = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc: f32 = (0..=r.min(w - 1)).map(|i| row[i]).sum();
        for x in 0..w {
            dst[y * w + x] = acc * k;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
}

fn box_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let k = 1.0 / (2 * r + 1) as f32;
    for x in 0..w {
        let mut acc: f32 = (0..=r.min(h - 1)).map(|i| src[i * w + x]).sum();
        for y in 0..h {
            dst[y * w + x] = acc * k;
            if y + r + 1 < h {
                acc += src[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= src[(y - r) * w + x];
            }
        }
    }
}

/// The coverage grown outward by `r` px, with a smooth edge: how far each
/// pixel is from the shape (exact distances), turned into coverage.
pub fn dilate(a: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    distance(a, w, h).iter().zip(a).map(|(&dd, &av)| (r + 0.5 - dd).clamp(0.0, 1.0).max(av)).collect()
}

/// How far each pixel is from the shape (the pixels at least half
/// covered), exactly, in px.
pub fn distance(a: &[f32], w: usize, h: usize) -> Vec<f32> {
    let inf = 1e20f32;
    let mut d: Vec<f32> = a.iter().map(|&v| if v >= 0.5 { 0.0 } else { inf }).collect();
    let mut f = vec![0f32; w.max(h)];
    let mut out = vec![0f32; w.max(h)];
    let mut v = vec![0usize; w.max(h)];
    let mut z = vec![0f32; w.max(h) + 1];
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        edt_1d(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&d[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut out[..w], &mut v, &mut z);
        d[y * w..(y + 1) * w].copy_from_slice(&out[..w]);
    }
    for x in d.iter_mut() {
        *x = x.sqrt();
    }
    d
}

/// Felzenszwalb and Huttenlocher's distance transform along one line.
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        *dq = (q as f32 - p as f32).powi(2) + f[p];
    }
}
