//! Shape and markup layers: vector shapes, speech bubbles and emoji placed in
//! a box, and freehand pen / highlighter strokes. Drawn with tiny-skia.

use crate::geo::{Affine, Obb};
use image::RgbaImage;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

// ---------------------------------------------------------------- shapes

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ShapeKind {
    Rect,
    Rounded,
    Ellipse,
    Triangle,
    Star,
    Burst,
    Heart,
    Arrow,
    Line,
    Speech,
    Bubble,
    Thought,
    Emoji,
}

impl ShapeKind {
    /// Shapes offered in the Shapes tool. `Emoji` stays in the enum so
    /// projects and presets from 0.7.0 still load; it is its own tool now.
    pub const ALL: [ShapeKind; 12] = [
        ShapeKind::Rect,
        ShapeKind::Rounded,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Star,
        ShapeKind::Burst,
        ShapeKind::Heart,
        ShapeKind::Arrow,
        ShapeKind::Line,
        ShapeKind::Speech,
        ShapeKind::Bubble,
        ShapeKind::Thought,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Rect => "Rectangle",
            ShapeKind::Rounded => "Rounded",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Star => "Star",
            ShapeKind::Burst => "Burst",
            ShapeKind::Heart => "Heart",
            ShapeKind::Arrow => "Arrow",
            ShapeKind::Line => "Line",
            ShapeKind::Speech => "Speech",
            ShapeKind::Bubble => "Bubble",
            ShapeKind::Thought => "Thought",
            ShapeKind::Emoji => "Emoji",
        }
    }
    /// Drawn by dragging from start to end rather than as a box.
    pub fn is_linear(self) -> bool {
        matches!(self, ShapeKind::Arrow | ShapeKind::Line)
    }
}

/// How a drop shadow looks. Distance and blur are a share of the shape's size,
/// so the shadow keeps its look when the shape is resized.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Shadow {
    /// direction the shadow falls, degrees clockwise from the right (45 = down right)
    pub angle: f32,
    /// % of the shape's larger side
    pub distance: f32,
    /// % of the shape's larger side
    pub blur: f32,
    /// 0..=100
    pub opacity: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        // the look shadows had before they could be adjusted
        Self { angle: 45.0, distance: 4.0, blur: 0.0, opacity: 35.0 }
    }
}

impl Shadow {
    /// Offset in pixels for a shape whose larger side is `size` pixels.
    fn offset(&self, size: f32) -> (f32, f32) {
        let d = size * self.distance / 100.0 + 2.0 * (self.distance / 4.0).min(1.0);
        let (sn, cs) = self.angle.to_radians().sin_cos();
        // the old shadow sat d pixels right and d down, so 45° keeps that spot
        (cs * d * std::f32::consts::SQRT_2, sn * d * std::f32::consts::SQRT_2)
    }

    fn blur_px(&self, size: f32) -> f32 {
        size * self.blur / 100.0
    }

    /// Room a sprite needs around the shape for the shadow.
    fn pad(&self, size: f32) -> f32 {
        let (dx, dy) = self.offset(size);
        dx.abs().max(dy.abs()) + self.blur_px(size) * 3.0 + 2.0
    }

    /// Draw a shadow of whatever `draw` paints, under what is in `pm` so far.
    fn draw(&self, pm: &mut Pixmap, size: f32, tf: Transform, draw: impl Fn(&mut Pixmap, Transform) -> Option<()>) -> Option<()> {
        let (dx, dy) = self.offset(size);
        let mut sh = Pixmap::new(pm.width(), pm.height())?;
        draw(&mut sh, tf.post_translate(dx, dy))?;
        // black at the chosen strength, keeping the drawn shape's coverage
        let a = (self.opacity / 100.0).clamp(0.0, 1.0);
        let mut img = RgbaImage::new(sh.width(), sh.height());
        for (o, p) in img.pixels_mut().zip(sh.pixels()) {
            o.0 = [0, 0, 0, (p.alpha() as f32 * a).round() as u8];
        }
        let b = self.blur_px(size);
        if b >= 0.3 {
            img = image::imageops::blur(&img, b);
        }
        for (px, o) in sh.pixels_mut().iter_mut().zip(img.pixels()) {
            *px = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, o[3]).unwrap_or(*px);
        }
        pm.draw_pixmap(0, 0, sh.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        Some(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ShapeSettings {
    pub kind: ShapeKind,
    /// top-left of the unrotated box (source-frame pixels)
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// clockwise degrees about the box centre
    pub angle: f32,
    pub fill_on: bool,
    pub fill: [u8; 3],
    pub stroke_on: bool,
    pub stroke: [u8; 3],
    pub stroke_w: f32,
    /// 0..=100
    pub opacity: f32,
    /// corner rounding for Rounded and Speech, 0..=50 (% of the short side)
    pub corner: f32,
    /// number of points for Star and Burst
    pub points: u32,
    /// drop shadow below the shape
    pub shadow: bool,
    pub shadow_style: Shadow,
    /// Twemoji file name of the emoji (for Emoji)
    pub emoji: String,
    pub from: usize,
    /// 0 = end
    pub to: usize,
}

impl Default for ShapeSettings {
    fn default() -> Self {
        Self {
            kind: ShapeKind::Rounded,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            angle: 0.0,
            fill_on: true,
            fill: [255, 122, 26],
            stroke_on: false,
            stroke: [255, 255, 255],
            stroke_w: 4.0,
            opacity: 100.0,
            corner: 20.0,
            points: 5,
            shadow: false,
            shadow_style: Shadow::default(),
            emoji: "1f600".into(),
            from: 1,
            to: 0,
        }
    }
}

impl ShapeSettings {
    /// `frame` is the 0-based source frame index.
    pub fn applies_to(&self, frame: usize) -> bool {
        let i = frame + 1;
        self.w > 0 && self.h > 0 && i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

fn paint(c: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

/// Rounded rectangle centred on 0 (cubic corners).
fn rounded_rect(pb: &mut PathBuilder, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    if r <= 0.01 {
        if let Some(rc) = Rect::from_ltrb(x0, y0, x1, y1) {
            pb.push_rect(rc);
        }
        return;
    }
    let k = r * 0.5523;
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    pb.close();
}

fn oval(pb: &mut PathBuilder, x0: f32, y0: f32, x1: f32, y1: f32) {
    if let Some(rc) = Rect::from_ltrb(x0, y0, x1, y1) {
        pb.push_oval(rc);
    }
}

fn polygon(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    if let Some((first, rest)) = pts.split_first() {
        pb.move_to(first.0, first.1);
        for p in rest {
            pb.line_to(p.0, p.1);
        }
        pb.close();
    }
}

/// Path of a shape in its own box, centred on the origin (half sizes hw, hh).
fn shape_path(s: &ShapeSettings, hw: f32, hh: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let short = hw.min(hh) * 2.0;
    match s.kind {
        ShapeKind::Rect => rounded_rect(&mut pb, -hw, -hh, hw, hh, 0.0),
        ShapeKind::Rounded => rounded_rect(&mut pb, -hw, -hh, hw, hh, short * s.corner / 100.0),
        ShapeKind::Ellipse => oval(&mut pb, -hw, -hh, hw, hh),
        ShapeKind::Triangle => polygon(&mut pb, &[(0.0, -hh), (hw, hh), (-hw, hh)]),
        ShapeKind::Star | ShapeKind::Burst => {
            let n = s.points.clamp(3, 40) as usize;
            let inner = if s.kind == ShapeKind::Star { 0.45 } else { 0.72 };
            let pts: Vec<(f32, f32)> = (0..n * 2)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
                    // a burst gets slightly uneven spikes, like a comic "POW"
                    let jag = if s.kind == ShapeKind::Burst && i % 2 == 0 { 0.88 + 0.12 * ((i * 7 % 5) as f32 / 4.0) } else { 1.0 };
                    let r = if i % 2 == 0 { jag } else { inner };
                    (a.cos() * hw * r, a.sin() * hh * r)
                })
                .collect();
            polygon(&mut pb, &pts);
        }
        ShapeKind::Heart => {
            // two lobes and a point, in a unit box scaled to the shape
            let p = |x: f32, y: f32| (x * hw, y * hh);
            let m = p(0.0, -0.45);
            pb.move_to(m.0, m.1);
            let (a, b, c) = (p(0.0, -1.05), p(-1.0, -1.05), p(-1.0, -0.35));
            pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
            let (a, b, c) = (p(-1.0, 0.25), p(-0.35, 0.6), p(0.0, 1.0));
            pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
            let (a, b, c) = (p(0.35, 0.6), p(1.0, 0.25), p(1.0, -0.35));
            pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
            let (a, b, c) = (p(1.0, -1.05), p(0.0, -1.05), p(0.0, -0.45));
            pb.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
            pb.close();
        }
        ShapeKind::Arrow => {
            let hl = (hh * 2.4).min(hw * 1.2);
            let sh = hh * 0.38;
            polygon(&mut pb, &[(-hw, -sh), (hw - hl, -sh), (hw - hl, -hh), (hw, 0.0), (hw - hl, hh), (hw - hl, sh), (-hw, sh)]);
        }
        ShapeKind::Line => {
            pb.move_to(-hw, 0.0);
            pb.line_to(hw, 0.0);
        }
        ShapeKind::Speech => {
            let body = hh * 0.5;
            rounded_rect(&mut pb, -hw, -hh, hw, body, short * 0.75 * s.corner / 100.0);
            polygon(&mut pb, &[(-hw * 0.42, body - 1.0), (-hw * 0.08, body - 1.0), (-hw * 0.55, hh)]);
        }
        ShapeKind::Bubble => {
            let body = hh * 0.55;
            oval(&mut pb, -hw, -hh, hw, body);
            polygon(&mut pb, &[(-hw * 0.45, body - hh * 0.35), (-hw * 0.05, body - hh * 0.12), (-hw * 0.6, hh)]);
        }
        ShapeKind::Thought => {
            let body = hh * 0.45;
            oval(&mut pb, -hw, -hh, hw, body);
            let r1 = short * 0.09;
            oval(&mut pb, -hw * 0.5 - r1, body + hh * 0.08, -hw * 0.5 + r1, body + hh * 0.08 + r1 * 2.0);
            let r2 = short * 0.055;
            oval(&mut pb, -hw * 0.72 - r2, hh - r2 * 2.0, -hw * 0.72 + r2, hh);
        }
        ShapeKind::Emoji => return None,
    }
    pb.finish()
}

fn pixmap_to_rgba(p: &Pixmap) -> RgbaImage {
    let mut out = RgbaImage::new(p.width(), p.height());
    for (dst, px) in out.pixels_mut().zip(p.pixels()) {
        let c = px.demultiply();
        *dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// Draw a shape into an output-space box. Returns the sprite and where its
/// top-left corner goes. `scale` turns source pixels (stroke width) into output pixels.
pub fn shape_sprite(s: &ShapeSettings, o: Obb, scale: f32) -> Option<(RgbaImage, i64, i64)> {
    if o.w < 1.0 || o.h < 1.0 {
        return None;
    }
    // a line is all outline, so it always has its width
    let sw = if s.stroke_on || s.kind == ShapeKind::Line { (s.stroke_w * scale).max(0.5) } else { 0.0 };
    let (hw, hh) = (o.w / 2.0, o.h / 2.0);
    let size = o.w.max(o.h);
    let pad = sw * 2.0 + 4.0 + if s.shadow { s.shadow_style.pad(size) } else { 0.0 };
    let (bx0, by0, bx1, by1) = o.bounds();
    let x0 = (bx0 - pad).floor();
    let y0 = (by0 - pad).floor();
    let pw = ((bx1 + pad).ceil() - x0).max(1.0) as u32;
    let ph = ((by1 + pad).ceil() - y0).max(1.0) as u32;
    if pw as u64 * ph as u64 > 64_000_000 {
        return None;
    }
    let mut pm = Pixmap::new(pw, ph)?;
    let tf = Transform::from_rotate(o.angle).post_translate(o.cx - x0, o.cy - y0);

    if s.kind == ShapeKind::Emoji {
        return emoji_sprite(&s.emoji, o, s.shadow.then_some(&s.shadow_style));
    }

    let path = shape_path(s, hw, hh)?;
    let line = s.kind == ShapeKind::Line;
    if s.shadow {
        s.shadow_style.draw(&mut pm, size, tf, |sh, st| {
            let p = paint([0, 0, 0], 255);
            if line {
                sh.stroke_path(&path, &p, &Stroke { width: sw.max(1.0), line_cap: LineCap::Round, ..Default::default() }, st, None);
            } else {
                if sw > 0.0 {
                    sh.stroke_path(&path, &p, &Stroke { width: sw * 2.0, line_join: LineJoin::Round, ..Default::default() }, st, None);
                }
                if s.fill_on || sw <= 0.0 {
                    sh.fill_path(&path, &p, FillRule::Winding, st, None);
                }
            }
            Some(())
        })?;
    }
    if line {
        let w = sw.max(1.0);
        pm.stroke_path(&path, &paint(s.stroke, 255), &Stroke { width: w, line_cap: LineCap::Round, ..Default::default() }, tf, None);
    } else if s.fill_on {
        // outline drawn at twice the width under the fill, so only its outer
        // half shows and the joins inside speech bubbles stay hidden
        if sw > 0.0 {
            pm.stroke_path(&path, &paint(s.stroke, 255), &Stroke { width: sw * 2.0, line_join: LineJoin::Round, ..Default::default() }, tf, None);
        }
        pm.fill_path(&path, &paint(s.fill, 255), FillRule::Winding, tf, None);
    } else if sw > 0.0 {
        pm.stroke_path(&path, &paint(s.stroke, 255), &Stroke { width: sw, line_join: LineJoin::Round, ..Default::default() }, tf, None);
    }
    Some((pixmap_to_rgba(&pm), x0 as i64, y0 as i64))
}

// ---------------------------------------------------------------- emoji

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EmojiSettings {
    /// Twemoji file name (hex code points), skin tone included
    pub emoji: String,
    /// top-left of the unrotated box (source-frame pixels)
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// clockwise degrees about the box centre
    pub angle: f32,
    /// 0..=100
    pub opacity: f32,
    pub shadow: bool,
    pub shadow_style: Shadow,
    pub from: usize,
    /// 0 = end
    pub to: usize,
}

impl Default for EmojiSettings {
    fn default() -> Self {
        Self { emoji: "1f600".into(), x: 0, y: 0, w: 0, h: 0, angle: 0.0, opacity: 100.0, shadow: false, shadow_style: Shadow::default(), from: 1, to: 0 }
    }
}

impl EmojiSettings {
    pub fn applies_to(&self, frame: usize) -> bool {
        let i = frame + 1;
        self.w > 0 && self.h > 0 && i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

/// Draw an emoji into an output-space box (vector, so sharp at any size).
pub fn emoji_sprite(file: &str, o: Obb, shadow: Option<&Shadow>) -> Option<(RgbaImage, i64, i64)> {
    if o.w < 1.0 || o.h < 1.0 {
        return None;
    }
    let size = o.w.max(o.h);
    let pad = 2.0 + shadow.map(|s| s.pad(size)).unwrap_or(0.0);
    let (bx0, by0, bx1, by1) = o.bounds();
    let x0 = (bx0 - pad).floor();
    let y0 = (by0 - pad).floor();
    let pw = ((bx1 + pad).ceil() - x0).max(1.0) as u32;
    let ph = ((by1 + pad).ceil() - y0).max(1.0) as u32;
    if pw as u64 * ph as u64 > 64_000_000 {
        return None;
    }
    let mut pm = Pixmap::new(pw, ph)?;
    let tf = Transform::from_rotate(o.angle).post_translate(o.cx - x0, o.cy - y0).pre_translate(-o.w / 2.0, -o.h / 2.0);
    if let Some(sh) = shadow {
        sh.draw(&mut pm, size, tf, |p, t| crate::emoji::draw(file, &mut p.as_mut(), o.w, o.h, t))?;
    }
    crate::emoji::draw(file, &mut pm.as_mut(), o.w, o.h, tf)?;
    Some((pixmap_to_rgba(&pm), x0 as i64, y0 as i64))
}

// ---------------------------------------------------------------- markup

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PenMode {
    Pen,
    Highlighter,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PenStroke {
    /// points in source-frame pixels
    pub points: Vec<[f32; 2]>,
    pub color: [u8; 3],
    pub width: f32,
    /// 0..=100
    pub opacity: f32,
    pub mode: PenMode,
    /// 1-based source frame range, `to` 0 = end
    pub from: usize,
    pub to: usize,
}

impl Default for PenStroke {
    fn default() -> Self {
        Self { points: Vec::new(), color: [255, 59, 48], width: 6.0, opacity: 100.0, mode: PenMode::Pen, from: 1, to: 0 }
    }
}

impl PenStroke {
    pub fn applies_to(&self, frame: usize) -> bool {
        let i = frame + 1;
        i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MarkupSettings {
    pub strokes: Vec<PenStroke>,
}

/// A stroke drawn in output space: sprite, top-left and frame range.
pub struct StrokeSprite {
    pub img: RgbaImage,
    pub x: i64,
    pub y: i64,
    pub from: usize,
    pub to: usize,
}

fn stroke_path(pts: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (f, rest) = pts.split_first()?;
    pb.move_to(f.0, f.1);
    if rest.is_empty() {
        pb.line_to(f.0 + 0.01, f.1);
    } else if rest.len() == 1 {
        pb.line_to(rest[0].0, rest[0].1);
    } else {
        // smooth through the midpoints
        for w in pts.windows(2).skip(1) {
            let (a, b) = (w[0], w[1]);
            pb.quad_to(a.0, a.1, (a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        }
        let l = pts[pts.len() - 1];
        pb.line_to(l.0, l.1);
    }
    pb.finish()
}

/// Draw one stroke whose points are already in output space.
pub fn stroke_sprite(s: &PenStroke, pts: &[(f32, f32)], width: f32) -> Option<StrokeSprite> {
    if pts.is_empty() {
        return None;
    }
    let w = width.max(0.5);
    let pad = w + 2.0;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let (ox, oy) = ((x0 - pad).floor(), (y0 - pad).floor());
    let pw = ((x1 + pad).ceil() - ox).max(1.0) as u32;
    let ph = ((y1 + pad).ceil() - oy).max(1.0) as u32;
    if pw as u64 * ph as u64 > 64_000_000 {
        return None;
    }
    let mut pm = Pixmap::new(pw, ph)?;
    let path = stroke_path(pts)?;
    let (cap, join) = match s.mode {
        PenMode::Pen => (LineCap::Round, LineJoin::Round),
        PenMode::Highlighter => (LineCap::Square, LineJoin::Round),
    };
    // drawn opaque, then faded as a whole so overlaps don't get darker
    pm.stroke_path(&path, &paint(s.color, 255), &Stroke { width: w, line_cap: cap, line_join: join, ..Default::default() }, Transform::from_translate(-ox, -oy), None);
    let mut img = pixmap_to_rgba(&pm);
    let a = (s.opacity / 100.0).clamp(0.0, 1.0);
    if a < 1.0 {
        for p in img.pixels_mut() {
            p[3] = (p[3] as f32 * a).round() as u8;
        }
    }
    Some(StrokeSprite { img, x: ox as i64, y: oy as i64, from: s.from, to: s.to })
}

/// Average scale of an affine map (for stroke widths).
pub fn map_scale(m: &Affine) -> f32 {
    m.det().abs().sqrt().max(1e-3)
}
