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
}

impl Ease {
    pub const ALL: [Ease; 5] = [Ease::Smooth, Ease::Linear, Ease::In, Ease::Out, Ease::Hold];

    pub fn label(self) -> &'static str {
        match self {
            Ease::Smooth => "Smooth",
            Ease::Linear => "Linear",
            Ease::In => "Ease in",
            Ease::Out => "Ease out",
            Ease::Hold => "Hold",
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
        Key { frame: 0, x: 0.0, y: 0.0, scale_x: 100.0, scale_y: 100.0, angle: 0.0, opacity: 100.0, ease: Ease::Smooth }
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
}

impl Default for KeyFile {
    fn default() -> Self {
        let k = Key::default();
        KeyFile { frame: 0, x: 0.0, y: 0.0, scale: None, scale_x: None, scale_y: None, angle: k.angle, opacity: k.opacity, smooth: None, ease: None }
    }
}

impl From<KeyFile> for Key {
    fn from(f: KeyFile) -> Key {
        let s = f.scale.unwrap_or(100.0);
        let ease = f.ease.unwrap_or(if f.smooth.unwrap_or(true) { Ease::Smooth } else { Ease::Linear });
        Key { frame: f.frame, x: f.x, y: f.y, scale_x: f.scale_x.unwrap_or(s), scale_y: f.scale_y.unwrap_or(s), angle: f.angle, opacity: f.opacity, ease }
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
    pub fn clip_at(&self, i: usize, n: usize) -> Option<(usize, usize)> {
        self.spans(n).into_iter().find(|(a, b)| i >= *a && i <= *b)
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
        t = match (ea, eb) {
            (true, true) => t * t * (3.0 - 2.0 * t),
            (true, false) => 1.0 - (1.0 - t) * (1.0 - t),
            (false, true) => t * t,
            (false, false) => t,
        };
        let (ma, mb) = (m(&a), m(&b));
        let l = |x: f32, y: f32| x + (y - x) * t;
        Motion { dx: l(ma.dx, mb.dx), dy: l(ma.dy, mb.dy), sx: l(ma.sx, mb.sx), sy: l(ma.sy, mb.sy), angle: l(ma.angle, mb.angle), opacity: l(ma.opacity, mb.opacity) }
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
        let k = Key { frame, x: m.dx, y: m.dy, scale_x: m.sx * 100.0, scale_y: m.sy * 100.0, angle: m.angle, opacity: m.opacity * 100.0, ease: Ease::Smooth };
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
        assert_eq!(t.clip_at(8, 10), Some((8, 9)));
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
