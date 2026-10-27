//! When each layer shows and how it moves: clips on the timeline and
//! keyframes. Times are output frames (positions in the frame plan), so a
//! layer stays where it was put on the timeline whatever frame edits do.

use crate::geo::Obb;
use image::RgbaImage;

/// A stretch of frames a layer shows on, both ends included. `end: None`
/// runs to the last frame, so a clip covering everything keeps covering it
/// when frames are added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub start: usize,
    pub end: Option<usize>,
}

impl Clip {
    pub fn all() -> Self {
        Clip { start: 0, end: None }
    }
    /// The last frame, for an animation of `n` frames.
    pub fn last(&self, n: usize) -> usize {
        self.end.unwrap_or(usize::MAX).min(n.saturating_sub(1))
    }
}

/// A keyframe: how far the layer has moved, grown, turned and faded at
/// `frame`, relative to where it was placed.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, from = "KeyFile")]
pub struct Key {
    pub frame: usize,
    /// offset in source pixels
    pub x: f32,
    pub y: f32,
    /// width and height, % of the placed size
    pub scale_x: f32,
    pub scale_y: f32,
    /// degrees clockwise, added to the layer's own angle
    pub angle: f32,
    /// 0..=100
    pub opacity: f32,
    /// how the motion arrives at and leaves this key
    pub ease: Ease,
    /// with `Ease::Custom`: the timing on the way to the next key, as a
    /// CSS cubic-bezier (x1, y1, x2, y2); y can go past 0..1 to overshoot
    pub curve: [f32; 4],
}

/// The default custom curve: slow out and in, like Smooth.
pub const CURVE_SMOOTH: [f32; 4] = [0.42, 0.0, 0.58, 1.0];

/// Where a CSS cubic-bezier timing curve is at `t` (0..=1).
pub fn bezier(c: [f32; 4], t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let [x1, y1, x2, y2] = c;
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let at = |a: f32, b: f32, s: f32| 3.0 * a * s * (1.0 - s) * (1.0 - s) + 3.0 * b * s * s * (1.0 - s) + s * s * s;
    // the curve's x only grows, so halving finds where it reaches t
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if at(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, (lo + hi) / 2.0)
}

/// How motion moves through a keyframe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ease {
    /// slows down arriving and leaving
    #[default]
    Smooth,
    /// a steady speed
    Linear,
    /// slows down arriving here
    In,
    /// starts slowly leaving here
    Out,
    /// stays put until the next key, then jumps
    Hold,
    /// a curve of your own, on the way to the next key
    Custom,
}

impl Ease {
    pub const ALL: [Ease; 6] = [Ease::Smooth, Ease::Linear, Ease::In, Ease::Out, Ease::Hold, Ease::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Ease::Smooth => "Smooth",
            Ease::Linear => "Linear",
            Ease::In => "Ease in",
            Ease::Out => "Ease out",
            Ease::Hold => "Hold",
            Ease::Custom => "Custom",
        }
    }

    fn slow_arriving(self) -> bool {
        matches!(self, Ease::Smooth | Ease::In)
    }

    fn slow_leaving(self) -> bool {
        matches!(self, Ease::Smooth | Ease::Out)
    }
}

impl Default for Key {
    fn default() -> Self {
        Key { frame: 0, x: 0.0, y: 0.0, scale_x: 100.0, scale_y: 100.0, angle: 0.0, opacity: 100.0, ease: Ease::Smooth, curve: CURVE_SMOOTH }
    }
}

/// A key as stored, reading older projects that had one `scale` for both.
#[derive(serde::Deserialize)]
#[serde(default)]
struct KeyFile {
    frame: usize,
    x: f32,
    y: f32,
    scale: Option<f32>,
    scale_x: Option<f32>,
    scale_y: Option<f32>,
    angle: f32,
    opacity: f32,
    /// older projects: smooth or linear
    smooth: Option<bool>,
    ease: Option<Ease>,
    curve: Option<[f32; 4]>,
}

impl Default for KeyFile {
    fn default() -> Self {
        let k = Key::default();
        KeyFile { frame: 0, x: 0.0, y: 0.0, scale: None, scale_x: None, scale_y: None, angle: k.angle, opacity: k.opacity, smooth: None, ease: None, curve: None }
    }
}

impl From<KeyFile> for Key {
    fn from(f: KeyFile) -> Key {
        let s = f.scale.unwrap_or(100.0);
        let ease = f.ease.unwrap_or(if f.smooth.unwrap_or(true) { Ease::Smooth } else { Ease::Linear });
        Key { frame: f.frame, x: f.x, y: f.y, scale_x: f.scale_x.unwrap_or(s), scale_y: f.scale_y.unwrap_or(s), angle: f.angle, opacity: f.opacity, ease, curve: f.curve.unwrap_or(CURVE_SMOOTH) }
    }
}

/// The motion at one frame, from the keyframes around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    /// factors (1 = placed size)
    pub sx: f32,
    pub sy: f32,
    pub angle: f32,
    /// factor (1 = fully shown)
    pub opacity: f32,
}

impl Motion {
    pub const NONE: Motion = Motion { dx: 0.0, dy: 0.0, sx: 1.0, sy: 1.0, angle: 0.0, opacity: 1.0 };

    /// Moves, grows or turns the layer (rather than only fading it).
    pub fn moves(&self) -> bool {
        self.dx.abs() > 0.01 || self.dy.abs() > 0.01 || (self.sx - 1.0).abs() > 0.001 || (self.sy - 1.0).abs() > 0.001 || self.angle.abs() > 0.01
    }

    /// A box in source space, moved by this motion about its centre.
    pub fn apply(&self, b: Obb) -> Obb {
        Obb { cx: b.cx + self.dx, cy: b.cy + self.dy, w: b.w * self.sx, h: b.h * self.sy, angle: b.angle + self.angle }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Timing {
    /// None: the layer shows wherever its own frame range says (older
    /// projects); Some: only inside these clips
    pub clips: Option<Vec<Clip>>,
    /// sorted by frame
    pub keys: Vec<Key>,
}

impl Timing {
    pub fn is_default(&self) -> bool {
        self.clips.is_none() && self.keys.is_empty()
    }

    /// Clips sorted, clamped to `n` frames, as (first, last).
    pub fn spans(&self, n: usize) -> Vec<(usize, usize)> {
        let Some(c) = &self.clips else { return vec![(0, n.saturating_sub(1))] };
        let mut v: Vec<(usize, usize)> = c.iter().filter(|c| c.start < n).map(|c| (c.start, c.last(n))).filter(|(a, b)| a <= b).collect();
        v.sort();
        v
    }

    pub fn shows(&self, i: usize, n: usize) -> bool {
        match &self.clips {
            None => true,
            Some(c) => c.iter().any(|c| i >= c.start && i <= c.last(n)),
        }
    }

    /// The clip frame `i` falls in, as (first, last).
    /// From the first frame it shows to the last, across all its clips:
    /// what intros, outros and loops run over, so moving clips about never
    /// carries a fade into the middle.
    pub fn extent(&self, n: usize) -> (usize, usize) {
        let sp = self.spans(n);
        match (sp.first(), sp.iter().map(|s| s.1).max()) {
            (Some(f), Some(l)) => (f.0, l),
            _ => (0, n.saturating_sub(1)),
        }
    }

    pub fn motion(&self, i: usize) -> Motion {
        let k = &self.keys;
        if k.is_empty() {
            return Motion::NONE;
        }
        let m = |k: &Key| Motion { dx: k.x, dy: k.y, sx: k.scale_x / 100.0, sy: k.scale_y / 100.0, angle: k.angle, opacity: (k.opacity / 100.0).clamp(0.0, 1.0) };
        if i <= k[0].frame {
            return m(&k[0]);
        }
        let last = k[k.len() - 1];
        if i >= last.frame {
            return m(&last);
        }
        let j = k.iter().position(|q| q.frame > i).unwrap_or(k.len() - 1);
        let (a, b) = (k[j - 1], k[j]);
        if a.ease == Ease::Hold {
            return m(&a);
        }
        let mut t = (i - a.frame) as f32 / (b.frame - a.frame).max(1) as f32;
        // eased keys slow down as the layer leaves or arrives
        let (ea, eb) = (a.ease.slow_leaving(), b.ease.slow_arriving());
        t = if a.ease == Ease::Custom {
            // its own curve decides the whole way to the next key
            bezier(a.curve, t)
        } else {
            match (ea, eb) {
            (true, true) => t * t * (3.0 - 2.0 * t),
            (true, false) => 1.0 - (1.0 - t) * (1.0 - t),
            (false, true) => t * t,
            (false, false) => t,
            }
        };
        let (ma, mb) = (m(&a), m(&b));
        let l = |x: f32, y: f32| x + (y - x) * t;
        Motion { dx: l(ma.dx, mb.dx), dy: l(ma.dy, mb.dy), sx: l(ma.sx, mb.sx), sy: l(ma.sy, mb.sy), angle: l(ma.angle, mb.angle), opacity: l(ma.opacity, mb.opacity) }
    }

    /// Move along `path` (centres, in source pixels) over the layer's
    /// clips, at an even speed: keyframes where the path bends, linear in
    /// between. Size, turn and fade stay as they were. `centre` is where the
    /// layer was placed. Returns how many keys it made.
    pub fn follow_path(&mut self, path: &[(f32, f32)], centre: (f32, f32), n: usize) -> usize {
        if path.len() < 2 || n == 0 {
            return 0;
        }
        let (a, z) = self.extent(n);
        let frames = z - a + 1;
        // where it is each frame: the same distance along the path each time
        let mut len = vec![0.0f32];
        for w in path.windows(2) {
            let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
            len.push(len.last().unwrap() + d);
        }
        let total = *len.last().unwrap();
        let at = |s: f32| -> (f32, f32) {
            let j = len.iter().position(|&l| l >= s).unwrap_or(len.len() - 1).max(1);
            let (l0, l1) = (len[j - 1], len[j]);
            let t = if l1 > l0 { (s - l0) / (l1 - l0) } else { 0.0 };
            (path[j - 1].0 + (path[j].0 - path[j - 1].0) * t, path[j - 1].1 + (path[j].1 - path[j - 1].1) * t)
        };
        let pos: Vec<(f32, f32)> = (0..frames).map(|i| at(if frames > 1 { total * i as f32 / (frames - 1) as f32 } else { 0.0 })).collect();
        self.set_positions(a, &pos, centre)
    }

    /// Put the layer's centre at `pos[i]` on frame `a + i` (source pixels):
    /// keyframes where the line bends, linear in between. Size, turn and
    /// fade stay as they were. Returns how many keys it made.
    pub fn set_positions(&mut self, a: usize, pos: &[(f32, f32)], centre: (f32, f32)) -> usize {
        let frames = pos.len();
        if frames == 0 {
            return 0;
        }
        let z = a + frames - 1;
        // keep the frames where the line bends (the rest are in between)
        let mut keep = vec![false; frames];
        keep[0] = true;
        keep[frames - 1] = true;
        fn simplify(pos: &[(f32, f32)], keep: &mut [bool], i: usize, j: usize) {
            if j <= i + 1 {
                return;
            }
            let (p, q) = (pos[i], pos[j]);
            let (mut worst, mut wi) = (0.0f32, i);
            for k in i + 1..j {
                // where a straight line between the two would put it
                let t = (k - i) as f32 / (j - i) as f32;
                let e = (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
                let d = ((pos[k].0 - e.0).powi(2) + (pos[k].1 - e.1).powi(2)).sqrt();
                if d > worst {
                    worst = d;
                    wi = k;
                }
            }
            if worst > 1.0 {
                keep[wi] = true;
                simplify(pos, keep, i, wi);
                simplify(pos, keep, wi, j);
            }
        }
        simplify(pos, &mut keep, 0, frames - 1);
        let old = self.clone();
        self.keys.retain(|k| k.frame < a || k.frame > z);
        let mut made = 0;
        for (i, p) in pos.iter().enumerate().filter(|(i, _)| keep[*i]) {
            let f = a + i;
            let m = old.motion(f);
            self.keys.push(Key { frame: f, x: p.0 - centre.0, y: p.1 - centre.1, scale_x: m.sx * 100.0, scale_y: m.sy * 100.0, angle: m.angle, opacity: m.opacity * 100.0, ease: Ease::Linear, curve: CURVE_SMOOTH });
            made += 1;
        }
        self.sort_keys();
        made
    }

    pub fn key_at(&self, frame: usize) -> Option<usize> {
        self.keys.iter().position(|k| k.frame == frame)
    }

    /// Add a key at `frame` holding the motion there now (so nothing jumps).
    pub fn add_key(&mut self, frame: usize) -> usize {
        if let Some(i) = self.key_at(frame) {
            return i;
        }
        let m = self.motion(frame);
        let k = Key { frame, x: m.dx, y: m.dy, scale_x: m.sx * 100.0, scale_y: m.sy * 100.0, angle: m.angle, opacity: m.opacity * 100.0, ease: Ease::Smooth, curve: CURVE_SMOOTH };
        self.keys.push(k);
        self.sort_keys();
        self.key_at(frame).unwrap_or(0)
    }

    pub fn sort_keys(&mut self) {
        self.keys.sort_by_key(|k| k.frame);
        self.keys.dedup_by_key(|k| k.frame);
    }

    /// Make the clips explicit from what showed before (`shown` per output frame).
    pub fn take_clips(&mut self, shown: &[bool]) {
        if self.clips.is_some() {
            return;
        }
        let n = shown.len();
        let mut v = Vec::new();
        let mut i = 0;
        while i < n {
            if !shown[i] {
                i += 1;
                continue;
            }
            let s = i;
            while i + 1 < n && shown[i + 1] {
                i += 1;
            }
            v.push(Clip { start: s, end: if i + 1 >= n { None } else { Some(i) } });
            i += 1;
        }
        self.clips = Some(v);
    }

    /// Cut the clip under `frame` in two, the second starting at `frame`.
    pub fn split(&mut self, frame: usize, n: usize) -> bool {
        let Some(c) = &mut self.clips else { return false };
        let Some(k) = c.iter().position(|c| frame > c.start && frame <= c.last(n)) else { return false };
        let old = c[k];
        c[k].end = Some(frame - 1);
        c.insert(k + 1, Clip { start: frame, end: old.end });
        true
    }
}

/// The linear part of a layer's motion on a sprite already drawn turned by
/// `base_deg`: stretch along the layer's own sides, then turn by `extra_deg`.
/// Rows of a 2×2 matrix: (a, b, c, d) maps (x, y) to (a x + b y, c x + d y).
pub fn motion_matrix(sx: f32, sy: f32, base_deg: f32, extra_deg: f32) -> [f32; 4] {
    let (s0, c0) = base_deg.to_radians().sin_cos();
    let (s1, c1) = (base_deg + extra_deg).to_radians().sin_cos();
    // R(base + extra) · S(sx, sy) · R(-base)
    let (a, b, c, d) = (c0, s0, -s0, c0);
    let (p, q, r, t) = (c1 * sx, -s1 * sy, s1 * sx, c1 * sy);
    [p * a + q * c, p * b + q * d, r * a + t * c, r * b + t * d]
}

/// Draw `img` (placed at `x`,`y`) moved by `d` and transformed by `lin`
/// (see [`motion_matrix`]) about `centre`, all in output pixels. Returns the
/// new sprite and its corner.
pub fn transform_sprite(img: &RgbaImage, x: i64, y: i64, centre: (f32, f32), d: (f32, f32), lin: [f32; 4]) -> Option<(RgbaImage, i64, i64)> {
    let [la, lb, lc, ld] = lin;
    let det = la * ld - lb * lc;
    if det.abs() < 1e-6 {
        return None;
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = centre;
    // where the sprite's corners land, for the size of the result
    let fwd = |px: f32, py: f32| {
        let (ux, uy) = (px - cx, py - cy);
        (cx + la * ux + lb * uy + d.0, cy + lc * ux + ld * uy + d.1)
    };
    let corners = [fwd(x as f32, y as f32), fwd(x as f32 + w, y as f32), fwd(x as f32, y as f32 + h), fwd(x as f32 + w, y as f32 + h)];
    let x0 = corners.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor();
    let y0 = corners.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor();
    let x1 = corners.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil();
    let y1 = corners.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil();
    let (ow, oh) = ((x1 - x0).max(1.0) as u32, (y1 - y0).max(1.0) as u32);
    if ow as u64 * oh as u64 > 64_000_000 {
        return None;
    }
    let mut out = RgbaImage::new(ow, oh);
    use rayon::prelude::*;
    let owu = ow as usize;
    out.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        // back from the output pixel to the sprite
        let (ox, oy) = (x0 + (i % owu) as f32 + 0.5 - d.0 - cx, y0 + (i / owu) as f32 + 0.5 - d.1 - cy);
        let (ux, uy) = ((ld * ox - lb * oy) / det, (-lc * ox + la * oy) / det);
        let (sx, sy) = (cx + ux - x as f32 - 0.5, cy + uy - y as f32 - 0.5);
        if let Some(v) = sample(img, sx, sy) {
            p.copy_from_slice(&v);
        }
    });
    Some((out, x0 as i64, y0 as i64))
}

/// Bilinear sample with transparent edges (premultiplied while mixing, so
/// edges don't pick up dark fringes).
fn sample(img: &RgbaImage, x: f32, y: f32) -> Option<[u8; 4]> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if x < -1.0 || y < -1.0 || x > w as f32 || y > h as f32 {
        return None;
    }
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xx: i32, yy: i32| -> [f32; 4] {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            return [0.0; 4];
        }
        let p = img.get_pixel(xx as u32, yy as u32).0;
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut o = [0f32; 4];
    for k in 0..4 {
        o[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    if o[3] < 0.5 {
        return None;
    }
    let al = o[3] / 255.0;
    Some([(o[0] / al).round().clamp(0.0, 255.0) as u8, (o[1] / al).round().clamp(0.0, 255.0) as u8, (o[2] / al).round().clamp(0.0, 255.0) as u8, o[3].round() as u8])
}

/// Mix `after` back toward `before` so the layer shows at `amount` (0..1).
pub fn blend(after: &mut RgbaImage, before: &RgbaImage, amount: f32) {
    use rayon::prelude::*;
    // past 1 it pushes further away from `before` (stronger effects)
    let a = amount.clamp(0.0, crate::effects::MAX_STRENGTH);
    after.par_chunks_mut(4).zip(before.par_chunks(4)).for_each(|(p, q)| {
        for k in 0..4 {
            p[k] = (q[k] as f32 + (p[k] as f32 - q[k] as f32) * a).round().clamp(0.0, 255.0) as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_a_drawn_path() {
        let mut t = Timing::default();
        // an L: right 100, then down 100
        let path = vec![(0.0, 0.0), (50.0, 0.0), (100.0, 0.0), (100.0, 50.0), (100.0, 100.0)];
        let k = t.follow_path(&path, (0.0, 0.0), 21);
        assert!(k >= 3 && k <= 5, "{k} keys: the ends and the corner");
        let m = |i| t.motion(i);
        assert!((m(0).dx).abs() < 0.5 && (m(20).dy - 100.0).abs() < 0.5);
        // half way along is the corner, at an even speed
        assert!((m(10).dx - 100.0).abs() < 1.0 && m(10).dy.abs() < 1.0, "{:?}", m(10));
        assert!((m(5).dx - 50.0).abs() < 1.0);
    }

    #[test]
    fn custom_curve() {
        // ends fixed, linear stays linear, overshoot goes past
        for c in [CURVE_SMOOTH, [0.0, 0.0, 1.0, 1.0], [0.3, 1.6, 0.6, 1.0]] {
            assert!(bezier(c, 0.0).abs() < 1e-3 && (bezier(c, 1.0) - 1.0).abs() < 1e-3);
        }
        assert!((bezier([0.0, 0.0, 1.0, 1.0], 0.3) - 0.3).abs() < 0.01);
        assert!(bezier([0.3, 1.6, 0.6, 1.0], 0.6) > 1.0, "overshoot");
        let mut t = Timing::default();
        t.keys = vec![Key { frame: 0, x: 0.0, ease: Ease::Custom, curve: [0.3, 1.6, 0.6, 1.0], ..Default::default() }, Key { frame: 10, x: 100.0, ..Default::default() }];
        assert!((1..10).any(|i| t.motion(i).dx > 100.0), "goes past and comes back");
        // and it saves
        let back: Key = serde_json::from_str(&serde_json::to_string(&t.keys[0]).unwrap()).unwrap();
        assert_eq!(back, t.keys[0]);
    }

    #[test]
    fn keys_tween_and_hold() {
        let mut t = Timing::default();
        t.keys = vec![Key { frame: 10, x: 0.0, ease: Ease::Linear, ..Default::default() }, Key { frame: 20, x: 100.0, opacity: 0.0, ease: Ease::Linear, ..Default::default() }];
        assert_eq!(t.motion(0).dx, 0.0, "holds before the first key");
        assert!((t.motion(15).dx - 50.0).abs() < 0.01);
        assert!((t.motion(15).opacity - 0.5).abs() < 0.01);
        assert_eq!(t.motion(30).dx, 100.0, "holds after the last key");
        // a smooth key starts slower
        t.keys[0].ease = Ease::Smooth;
        t.keys[1].ease = Ease::Smooth;
        assert!(t.motion(12).dx < 20.0);
        // adding a key between keeps the motion where it was
        let before = t.motion(15);
        t.add_key(15);
        assert_eq!(t.keys.len(), 3);
        assert!((t.motion(15).dx - before.dx).abs() < 0.01);
    }

    #[test]
    fn clips_from_old_ranges_split_and_show() {
        let mut t = Timing::default();
        // shown on frames 2..=4 and 7..=9 of 10
        let shown: Vec<bool> = (0..10).map(|i| (2..=4).contains(&i) || i >= 7).collect();
        t.take_clips(&shown);
        assert_eq!(t.spans(10), vec![(2, 4), (7, 9)]);
        assert_eq!(t.clips.as_ref().unwrap()[1].end, None, "a clip to the end stays open");
        assert!(t.shows(3, 10) && !t.shows(5, 10) && t.shows(9, 10));
        assert!(t.split(8, 10));
        assert_eq!(t.spans(10), vec![(2, 4), (7, 7), (8, 9)]);
        assert_eq!(t.spans(10).into_iter().find(|(a, b)| 8 >= *a && 8 <= *b), Some((8, 9)));
    }

    #[test]
    fn sprite_moves_and_turns() {
        let img = RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        let (o, x, y) = transform_sprite(&img, 0, 0, (5.0, 5.0), (20.0, 0.0), motion_matrix(1.0, 1.0, 0.0, 0.0)).unwrap();
        assert_eq!((x, y, o.width()), (20, 0, 10));
        let (o, _, _) = transform_sprite(&img, 0, 0, (5.0, 5.0), (0.0, 0.0), motion_matrix(2.0, 2.0, 0.0, 45.0)).unwrap();
        assert!(o.width() > 25, "doubled and turned it needs more room");
    }
}

/// A named point on the timeline (output frame).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub frame: usize,
    pub label: String,
}
