//! Layer styles, as in Photoshop: drop shadow, outer glow, stroke, inner
//! shadow, inner glow, satin, colour and gradient overlays and bevel and
//! emboss, plus the layer's fill opacity. They work on the layer's
//! prepared picture, so the graphics card and the processor draw the same
//! thing.

use crate::adjust::Blend;
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokePos {
    #[default]
    Outside,
    Inside,
    Center,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub on: bool,
    pub color: [u8; 3],
    /// px
    pub size: f32,
    /// %
    pub opacity: f32,
    pub position: StrokePos,
    pub blend: Blend,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { on: false, color: [0, 0, 0], size: 3.0, opacity: 100.0, position: StrokePos::Outside, blend: Blend::Normal }
    }
}

/// A drop shadow.
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
    /// how hard its edge is before it softens, %
    pub spread: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { on: false, color: [0, 0, 0], opacity: 75.0, angle: 120.0, distance: 5.0, size: 5.0, spread: 0.0 }
    }
}

/// A shadow inside the edge, as if the layer were cut out of the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InnerShadow {
    pub on: bool,
    pub color: [u8; 3],
    pub opacity: f32,
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
    /// %
    pub choke: f32,
    pub blend: Blend,
}

impl Default for InnerShadow {
    fn default() -> Self {
        InnerShadow { on: false, color: [0, 0, 0], opacity: 75.0, angle: 120.0, distance: 5.0, size: 5.0, choke: 0.0, blend: Blend::Multiply }
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InnerGlow {
    pub on: bool,
    pub color: [u8; 3],
    pub opacity: f32,
    pub size: f32,
    /// %
    pub choke: f32,
    /// glows out from the middle instead of in from the edge
    pub center: bool,
    pub blend: Blend,
}

impl Default for InnerGlow {
    fn default() -> Self {
        InnerGlow { on: false, color: [255, 255, 190], opacity: 75.0, size: 5.0, choke: 0.0, center: false, blend: Blend::Screen }
    }
}

/// Silky shading from two offset, blurred copies of the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Satin {
    pub on: bool,
    pub color: [u8; 3],
    pub opacity: f32,
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
    pub invert: bool,
    pub blend: Blend,
}

impl Default for Satin {
    fn default() -> Self {
        Satin { on: false, color: [0, 0, 0], opacity: 50.0, angle: 19.0, distance: 11.0, size: 14.0, invert: true, blend: Blend::Multiply }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverlay {
    pub on: bool,
    pub color: [u8; 3],
    pub opacity: f32,
    pub blend: Blend,
}

impl Default for ColorOverlay {
    fn default() -> Self {
        ColorOverlay { on: false, color: [255, 0, 0], opacity: 100.0, blend: Blend::Normal }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradStyle {
    #[default]
    Linear,
    Radial,
    Reflected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GradientOverlay {
    pub on: bool,
    /// colour at the start and the end
    pub from: [u8; 3],
    pub to: [u8; 3],
    pub opacity: f32,
    /// degrees: 90 runs from the bottom up
    pub angle: f32,
    pub style: GradStyle,
    /// %
    pub scale: f32,
    pub reverse: bool,
    pub blend: Blend,
}

impl Default for GradientOverlay {
    fn default() -> Self {
        GradientOverlay { on: false, from: [0, 0, 0], to: [255, 255, 255], opacity: 100.0, angle: 90.0, style: GradStyle::Linear, scale: 100.0, reverse: false, blend: Blend::Normal }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BevelStyle {
    #[default]
    Inner,
    Outer,
    Emboss,
    Pillow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bevel {
    pub on: bool,
    pub style: BevelStyle,
    /// %
    pub depth: f32,
    /// raised (false: sunk in)
    pub up: bool,
    /// px
    pub size: f32,
    /// px
    pub soften: f32,
    /// where the light comes from, degrees
    pub angle: f32,
    /// how high the light is, degrees
    pub altitude: f32,
    pub hi_color: [u8; 3],
    pub hi_opacity: f32,
    pub hi_blend: Blend,
    pub sh_color: [u8; 3],
    pub sh_opacity: f32,
    pub sh_blend: Blend,
}

impl Default for Bevel {
    fn default() -> Self {
        Bevel {
            on: false,
            style: BevelStyle::Inner,
            depth: 100.0,
            up: true,
            size: 5.0,
            soften: 0.0,
            angle: 120.0,
            altitude: 30.0,
            hi_color: [255, 255, 255],
            hi_opacity: 75.0,
            hi_blend: Blend::Screen,
            sh_color: [0, 0, 0],
            sh_opacity: 75.0,
            sh_blend: Blend::Multiply,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerStyle {
    pub stroke: Stroke,
    pub shadow: Shadow,
    pub glow: Glow,
    pub inner_shadow: InnerShadow,
    pub inner_glow: InnerGlow,
    pub satin: Satin,
    pub overlay: ColorOverlay,
    pub gradient: GradientOverlay,
    pub bevel: Bevel,
    /// the layer's own pixels, %, under its effects (Photoshop's Fill)
    pub fill: f32,
}

impl Default for LayerStyle {
    fn default() -> Self {
        LayerStyle {
            stroke: Stroke::default(),
            shadow: Shadow::default(),
            glow: Glow::default(),
            inner_shadow: InnerShadow::default(),
            inner_glow: InnerGlow::default(),
            satin: Satin::default(),
            overlay: ColorOverlay::default(),
            gradient: GradientOverlay::default(),
            bevel: Bevel::default(),
            fill: 100.0,
        }
    }
}

/// Lay colour `c` over premultiplied `o` at strength `s` in `mode`, the
/// way `stack::blend_onto` does.
fn comp(o: &mut [f32; 4], c: [f32; 3], s: f32, mode: Blend) {
    let s = s.clamp(0.0, 1.0);
    if s <= 0.0 {
        return;
    }
    let ba = o[3];
    if ba <= 1e-6 || mode.is_normal() {
        for k in 0..3 {
            o[k] = c[k] * s + o[k] * (1.0 - s);
        }
    } else {
        let bc = [o[0] / ba, o[1] / ba, o[2] / ba];
        let m = mode.apply(bc, c);
        for k in 0..3 {
            o[k] = s * (1.0 - ba) * c[k] + s * ba * m[k] + (1.0 - s) * o[k];
        }
    }
    o[3] = s + ba * (1.0 - s);
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| v as f32 / 255.0)
}

/// The offset a light at `angle` throws a shadow `dist` px away.
fn offset(angle: f32, dist: f32) -> (i64, i64) {
    let t = angle.to_radians();
    ((-t.cos() * dist).round() as i64, (t.sin() * dist).round() as i64)
}

/// Harder edged: everything over `1 - k` is solid.
fn harden(v: &mut [f32], k: f32) {
    let k = k.clamp(0.0, 0.99);
    if k > 0.0 {
        v.iter_mut().for_each(|x| *x = (*x / (1.0 - k)).min(1.0));
    }
}

impl LayerStyle {
    pub fn is_none(&self) -> bool {
        !self.stroke.on && !self.shadow.on && !self.glow.on && !self.inner_shadow.on && !self.inner_glow.on && !self.satin.on && !self.overlay.on && !self.gradient.on && !self.bevel.on && self.fill >= 100.0
    }

    /// How many effects are on.
    pub fn count(&self) -> usize {
        [self.stroke.on, self.shadow.on, self.glow.on, self.inner_shadow.on, self.inner_glow.on, self.satin.on, self.overlay.on, self.gradient.on, self.bevel.on].iter().filter(|b| **b).count()
    }

    /// How much room the style takes round the picture, px at `scale`.
    pub fn pad(&self, scale: f32) -> u32 {
        let mut p: f32 = 0.0;
        if self.stroke.on && self.stroke.position != StrokePos::Inside {
            let k = if self.stroke.position == StrokePos::Center { 0.5 } else { 1.0 };
            p = p.max(self.stroke.size * k * scale + 1.0);
        }
        if self.shadow.on {
            p = p.max((self.shadow.distance + self.shadow.size * 2.0) * scale + 2.0);
        }
        if self.glow.on {
            p = p.max(self.glow.size * scale + 2.0);
        }
        if self.bevel.on && self.bevel.style != BevelStyle::Inner {
            p = p.max((self.bevel.size + self.bevel.soften) * scale + 2.0);
        }
        p.ceil().min(1024.0) as u32
    }

    /// The picture with its style, grown by `pad(scale)` on every side.
    /// Straight alpha in and out.
    pub fn apply(&self, sp: &RgbaImage, scale: f32) -> RgbaImage {
        let pad = self.pad(scale);
        let (w, h) = (sp.width() + 2 * pad, sp.height() + 2 * pad);
        let (wu, hu) = (w as usize, h as usize);
        let n = wu * hu;
        // the layer's coverage and colour on the bigger canvas
        let mut a = vec![0f32; n];
        let mut col = vec![[0f32; 3]; n];
        for (x, y, p) in sp.enumerate_pixels() {
            let i = ((y + pad) * w + x + pad) as usize;
            a[i] = p[3] as f32 / 255.0;
            col[i] = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0];
        }
        // distances out from the shape and in from its edge, when wanted
        let mut d_out_c: Option<Vec<f32>> = None;
        let mut d_in_c: Option<Vec<f32>> = None;
        let mut d_out = |a: &[f32]| d_out_c.get_or_insert_with(|| distance(a, wu, hu)).clone();
        let mut d_in = |a: &[f32]| d_in_c.get_or_insert_with(|| distance(&a.iter().map(|v| 1.0 - v).collect::<Vec<_>>(), wu, hu)).clone();
        // under the layer, premultiplied, bottom to top
        let mut out = vec![[0f32; 4]; n];
        let fill = (self.fill / 100.0).clamp(0.0, 1.0);
        if self.shadow.on {
            let s = &self.shadow;
            let mut cov = blur(&a, wu, hu, s.size * scale);
            harden(&mut cov, s.spread / 100.0);
            let (dx, dy) = offset(s.angle, s.distance * scale);
            cov = shift(&cov, wu, hu, dx, dy);
            let c = rgb(s.color);
            // the layer knocks it out where it's see-through (Fill)
            for ((o, k), &av) in out.iter_mut().zip(cov).zip(&a) {
                comp(o, c, k * (1.0 - av * (1.0 - fill)) * s.opacity / 100.0, Blend::Normal);
            }
        }
        if self.glow.on {
            let g = &self.glow;
            let r = (g.size * scale).max(1.0);
            // solid out to the spread, then fading to nothing at the size
            let solid = r * (g.spread / 100.0).clamp(0.0, 1.0);
            let d = d_out(&a);
            let c = rgb(g.color);
            for ((o, &dd), &av) in out.iter_mut().zip(&d).zip(&a) {
                let k = (1.0 - ((dd - solid) / (r - solid).max(1.0)).clamp(0.0, 1.0)).powf(1.2).max(av) * (1.0 - av * (1.0 - fill));
                comp(o, c, k * g.opacity / 100.0, Blend::Normal);
            }
        }
        let st = &self.stroke;
        if st.on && st.position != StrokePos::Inside {
            let r = st.size * scale * if st.position == StrokePos::Center { 0.5 } else { 1.0 };
            let d = d_out(&a);
            let c = rgb(st.color);
            for ((o, &dd), &av) in out.iter_mut().zip(&d).zip(&a) {
                // under the layer; where the layer is see-through only the ring shows
                let k = (r + 0.5 - dd).clamp(0.0, 1.0).max(av) * (1.0 - av * (1.0 - fill));
                comp(o, c, k * st.opacity / 100.0, Blend::Normal);
            }
        }
        // the layer itself and what's laid inside it
        let mut inn: Vec<[f32; 4]> = a.iter().zip(&col).map(|(&k, c)| {
            let s = k * fill;
            [c[0] * s, c[1] * s, c[2] * s, s]
        }).collect();
        if self.gradient.on {
            let g = &self.gradient;
            // across the layer's own box
            let (mut x0, mut y0, mut x1, mut y1) = (wu, hu, 0usize, 0usize);
            for (i, &k) in a.iter().enumerate() {
                if k > 0.01 {
                    let (x, y) = (i % wu, i / wu);
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
            if x1 >= x0 && y1 >= y0 {
                let (cx, cy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
                let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
                let t = g.angle.to_radians();
                let (ux, uy) = (t.cos(), -t.sin());
                let k = (g.scale / 100.0).max(0.01);
                let half = ((ux.abs() * bw + uy.abs() * bh) / 2.0 * k).max(0.5);
                let radius = (bw.max(bh) / 2.0 * k).max(0.5);
                let (c0, c1) = (rgb(g.from), rgb(g.to));
                for (i, o) in inn.iter_mut().enumerate() {
                    if a[i] <= 0.0 {
                        continue;
                    }
                    let (px, py) = ((i % wu) as f32 - cx, (i / wu) as f32 - cy);
                    let along = px * ux + py * uy;
                    let mut f = match g.style {
                        GradStyle::Linear => 0.5 + along / (2.0 * half),
                        GradStyle::Radial => (px * px + py * py).sqrt() / radius,
                        GradStyle::Reflected => along.abs() / half,
                    }
                    .clamp(0.0, 1.0);
                    if g.reverse {
                        f = 1.0 - f;
                    }
                    let c = [c0[0] + (c1[0] - c0[0]) * f, c0[1] + (c1[1] - c0[1]) * f, c0[2] + (c1[2] - c0[2]) * f];
                    comp(o, c, a[i] * g.opacity / 100.0, g.blend);
                }
            }
        }
        if self.overlay.on {
            let c = rgb(self.overlay.color);
            for (o, &k) in inn.iter_mut().zip(&a) {
                comp(o, c, k * self.overlay.opacity / 100.0, self.overlay.blend);
            }
        }
        if self.satin.on {
            let s = &self.satin;
            let (dx, dy) = offset(s.angle, s.distance * scale);
            let p = blur(&shift(&a, wu, hu, dx, dy), wu, hu, s.size * scale);
            let q = blur(&shift(&a, wu, hu, -dx, -dy), wu, hu, s.size * scale);
            let c = rgb(s.color);
            for (i, o) in inn.iter_mut().enumerate() {
                let mut v = (p[i] - q[i]).abs();
                if s.invert {
                    v = 1.0 - v;
                }
                comp(o, c, a[i] * v * s.opacity / 100.0, s.blend);
            }
        }
        if self.inner_glow.on {
            let g = &self.inner_glow;
            let r = (g.size * scale).max(1.0);
            let solid = r * (g.choke / 100.0).clamp(0.0, 1.0);
            let d = d_in(&a);
            let c = rgb(g.color);
            for ((o, &dd), &av) in inn.iter_mut().zip(&d).zip(&a) {
                let e = (1.0 - ((dd - 0.5 - solid) / (r - solid).max(1.0)).clamp(0.0, 1.0)).powf(1.2);
                let k = if g.center { 1.0 - e } else { e };
                comp(o, c, av * k * g.opacity / 100.0, g.blend);
            }
        }
        if self.inner_shadow.on {
            let s = &self.inner_shadow;
            // the outside, cast in over the edge
            let inv: Vec<f32> = a.iter().map(|v| 1.0 - v).collect();
            let (dx, dy) = offset(s.angle, s.distance * scale);
            let mut cov = blur(&shift_fill(&inv, wu, hu, dx, dy, 1.0), wu, hu, s.size * scale);
            harden(&mut cov, s.choke / 100.0);
            let c = rgb(s.color);
            for ((o, k), &av) in inn.iter_mut().zip(cov).zip(&a) {
                comp(o, c, av * k * s.opacity / 100.0, s.blend);
            }
        }
        if st.on && st.position != StrokePos::Outside {
            let r = st.size * scale * if st.position == StrokePos::Center { 0.5 } else { 1.0 };
            let d = d_in(&a);
            let c = rgb(st.color);
            for ((o, &dd), &av) in inn.iter_mut().zip(&d).zip(&a) {
                comp(o, c, av * (r + 0.5 - dd).clamp(0.0, 1.0) * st.opacity / 100.0, st.blend);
            }
        }
        // the inside over what's under it
        for (o, i) in out.iter_mut().zip(&inn) {
            let s = i[3];
            for k in 0..3 {
                o[k] = i[k] + o[k] * (1.0 - s);
            }
            o[3] = s + o[3] * (1.0 - s);
        }
        if self.bevel.on {
            self.bevel_onto(&mut out, &a, wu, hu, scale, &mut d_in, &mut d_out);
        }
        let mut img = RgbaImage::new(w, h);
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

    /// Bevel and emboss: a height from the distances to the edge, lit from
    /// the bevel's light, highlights and shadows laid over.
    #[allow(clippy::too_many_arguments)]
    fn bevel_onto(&self, out: &mut [[f32; 4]], a: &[f32], w: usize, h: usize, scale: f32, d_in: &mut impl FnMut(&[f32]) -> Vec<f32>, d_out: &mut impl FnMut(&[f32]) -> Vec<f32>) {
        let b = &self.bevel;
        let s = (b.size * scale).max(1.0);
        let din = d_in(a);
        let dout = if b.style == BevelStyle::Inner { vec![0.0; a.len()] } else { d_out(a) };
        let ramp = |d: f32, len: f32| {
            let t = (d / len.max(0.5)).clamp(0.0, 1.0);
            1.0 - (1.0 - t) * (1.0 - t)
        };
        // where it shows: the shape, and outside it for the outer kinds
        let mut mask = vec![0f32; a.len()];
        let mut ht = vec![0f32; a.len()];
        for i in 0..a.len() {
            let av = a[i];
            // edge pixels part covered sit part way across the edge
            let (din_i, dout_i) = (din[i] + (av - 0.5).min(0.0), dout[i] - (av - 0.5).max(0.0));
            let (din, dout) = (|_: usize| din_i, |_: usize| dout_i);
            let (hi, ho, mo) = match b.style {
                BevelStyle::Inner => (ramp(din(i) - 0.5, s), 0.0, 0.0),
                BevelStyle::Outer => (1.0, 1.0 - ramp(dout(i) - 0.5, s), (s + 1.0 - dout(i)).clamp(0.0, 1.0)),
                BevelStyle::Emboss => (0.5 + 0.5 * ramp(din(i) - 0.5, s / 2.0), 0.5 - 0.5 * ramp(dout(i) - 0.5, s / 2.0), (s / 2.0 + 1.0 - dout(i)).clamp(0.0, 1.0)),
                BevelStyle::Pillow => (ramp(din(i) - 0.5, s / 2.0), ramp(dout(i) - 0.5, s / 2.0), (s / 2.0 + 1.0 - dout(i)).clamp(0.0, 1.0)),
            };
            ht[i] = hi * av + ho * (1.0 - av);
            mask[i] = if b.style == BevelStyle::Inner { av } else { av + (1.0 - av) * mo };
        }
        // always a little smoothing, or the edge's pixel steps show as streaks
        ht = blur(&ht, w, h, (b.soften * scale).max(1.5 * scale.max(1.0)));
        let depth = b.depth / 100.0 * if b.up { 1.0 } else { -1.0 };
        let (t, al) = (b.angle.to_radians(), b.altitude.clamp(0.0, 90.0).to_radians());
        let l = [al.cos() * t.cos(), -al.cos() * t.sin(), al.sin()];
        let flat = al.sin();
        let (hc, sc) = (rgb(b.hi_color), rgb(b.sh_color));
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if mask[i] <= 0.0 {
                    continue;
                }
                let at = |xx: usize, yy: usize| ht[yy * w + xx] * s * depth;
                let gx = (at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y)) / 2.0;
                let gy = (at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1))) / 2.0;
                let nl = (gx * gx + gy * gy + 1.0).sqrt();
                let shade = (-gx * l[0] - gy * l[1] + l[2]) / nl;
                let hi = ((shade - flat) / (1.0 - flat).max(1e-3)).clamp(0.0, 1.0);
                let sh = ((flat - shade) / flat.max(1e-3)).clamp(0.0, 1.0);
                let o = &mut out[i];
                comp(o, sc, mask[i] * sh * b.sh_opacity / 100.0, b.sh_blend);
                comp(o, hc, mask[i] * hi * b.hi_opacity / 100.0, b.hi_blend);
            }
        }
    }
}

/// Moved by (dx, dy), `fill` where nothing comes in.
fn shift_fill(v: &[f32], w: usize, h: usize, dx: i64, dy: i64, fill: f32) -> Vec<f32> {
    let mut out = vec![fill; v.len()];
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
