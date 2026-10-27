//! Text rendering onto frames (ab_glyph).

use crate::ops::blend_over;
use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, VariableFont};
use std::sync::Arc;
use image::{Rgba, RgbaImage};

pub static SANS_BOLD: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");
pub static MONO_BOLD: &[u8] = include_bytes!("../assets/DejaVuSansMono-Bold.ttf");

/// A loaded font: the rasteriser's copy, plus the raw file for shaping.
#[derive(Clone)]
pub struct FontEntry {
    pub name: String,
    pub font: FontArc,
    /// the font file, for the shaper (right-to-left text, joined and combining letters)
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    /// variable font axis settings, e.g. wght = 700
    pub coords: Vec<([u8; 4], f32)>,
}

impl FontEntry {
    pub fn from_data(name: impl Into<String>, data: Vec<u8>, index: u32, coords: Vec<([u8; 4], f32)>) -> anyhow::Result<Self> {
        let mut fv = FontVec::try_from_vec_and_index(data.clone(), index).map_err(|e| anyhow::anyhow!("Invalid font: {e}"))?;
        for (tag, v) in &coords {
            fv.set_variation(tag, *v);
        }
        Ok(Self { name: name.into(), font: FontArc::new(fv), data: Arc::new(data), index, coords })
    }
}

pub fn builtin_fonts() -> Vec<FontEntry> {
    [("Sans Bold (built-in)", SANS_BOLD), ("Mono Bold (built-in)", MONO_BOLD), ("Noto Sans (built-in)", crate::theme::NOTO_MEDIUM)]
        .into_iter()
        .map(|(n, d)| FontEntry::from_data(n, d.to_vec(), 0, Vec::new()).expect("font"))
        .collect()
}

pub fn load_font_file(path: &std::path::Path) -> anyhow::Result<FontEntry> {
    let data = std::fs::read(path)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Custom".into());
    FontEntry::from_data(name, data, 0, Vec::new())
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Border {
    None,
    Light,
    Heavy,
}

impl Border {
    pub fn radius(self, size: f32) -> i32 {
        match self {
            Border::None => 0,
            Border::Light => (size / 16.0).ceil().max(1.0) as i32,
            Border::Heavy => (size / 8.0).ceil().max(2.0) as i32,
        }
    }
}

/// A strip across the top or bottom of the canvas that the text sits in
/// (the meme caption look). The canvas grows to make room for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Caption {
    #[default]
    None,
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Annotation {
    pub text: String,
    pub font_idx: usize,
    pub size: f32,
    pub color: [u8; 4],
    pub border: Border,
    /// outline width in pixels; None = from `border` (older files)
    pub border_width: Option<f32>,
    pub border_color: [u8; 4],
    /// top-left of the text block (pixels)
    pub x: i32,
    pub y: i32,
    pub align: Align,
    /// 0..=100
    pub opacity: u8,
    /// 1-based inclusive frame range; to = 0 means "end"
    pub from: usize,
    pub to: usize,
    pub visible: bool,
    /// clockwise rotation in degrees about the text centre
    pub angle: f32,
    /// horizontal squash (<1) or stretch (>1) of the rendered text
    pub stretch: f32,
    /// the same up and down
    pub stretch_y: f32,
    /// start new lines where the text reaches the canvas edge
    pub wrap: bool,
    /// keep the text centred across the canvas
    pub centre: bool,
    pub caption: Caption,
    pub bar_color: [u8; 3],
    /// room above and below the text in the bar, % of the text size
    pub bar_pad: f32,
    // worked out from the canvas each time it changes (see Stack::layout_text)
    /// line length for `wrap`, in the same units as `size` (0 = no wrapping)
    pub wrap_px: f32,
    /// the bar in output pixels: top edge and height
    pub bar_y: i32,
    pub bar_h: u32,
    /// typing: the text shows letter by letter or word by word
    pub reveal: Reveal,
    /// over this long (seconds), from where the layer starts
    pub reveal_secs: f32,
}

/// How text comes in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Reveal {
    #[default]
    None,
    Letters,
    Words,
}

impl Annotation {
    /// How many characters (line breaks not counted) show after each step of
    /// typing, from none to all. Empty when it doesn't type.
    pub fn reveal_steps(&self, f: &FontEntry) -> Vec<usize> {
        let shown: String = self.shown_text(f).chars().filter(|c| *c != '\n').collect();
        let n = shown.chars().count();
        let mut v = vec![0];
        match self.reveal {
            Reveal::None => return Vec::new(),
            Reveal::Letters => {
                // a space adds no step of its own
                for (i, c) in shown.chars().enumerate() {
                    if !c.is_whitespace() {
                        v.push(i + 1);
                    }
                }
            }
            Reveal::Words => {
                let chars: Vec<char> = shown.chars().collect();
                for i in 0..n {
                    if !chars[i].is_whitespace() && (i + 1 == n || chars[i + 1].is_whitespace()) {
                        v.push(i + 1);
                    }
                }
            }
        }
        // very long text: in at most 200 steps
        if v.len() > 201 {
            let k = v.len() as f32 / 200.0;
            v = (0..=200).map(|i| v[((i as f32 * k) as usize).min(v.len() - 1)]).collect();
            v.dedup();
        }
        v
    }

    /// Which step of typing shows `t` seconds after the layer starts (None:
    /// all of it).
    pub fn reveal_step(&self, steps: usize, t: f32) -> Option<usize> {
        if self.reveal == Reveal::None || steps < 2 {
            return None;
        }
        let k = (t.max(0.0) / self.reveal_secs.max(0.05) * (steps - 1) as f32).floor() as usize;
        (k < steps - 1).then_some(k)
    }
}

impl Default for Annotation {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_idx: 0,
            size: 32.0,
            color: [0, 0, 0, 255],
            border: Border::None,
            border_width: None,
            border_color: [0, 0, 0, 255],
            x: 10,
            y: 10,
            align: Align::Center,
            opacity: 100,
            from: 1,
            to: 0,
            visible: true,
            angle: 0.0,
            stretch: 1.0,
            stretch_y: 1.0,
            wrap: false,
            centre: false,
            caption: Caption::None,
            bar_color: [255, 255, 255],
            bar_pad: 35.0,
            wrap_px: 0.0,
            bar_y: 0,
            bar_h: 0,
            reveal: Reveal::None,
            reveal_secs: 1.5,
        }
    }
}

impl Annotation {
    pub fn applies_to(&self, frame_idx0: usize) -> bool {
        let i = frame_idx0 + 1;
        i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

/// One shaped glyph: id and pen offset (pixels) from the start of the line.
struct Placed {
    id: GlyphId,
    x: f32,
    y: f32,
    /// which character of the line it's drawn for (typing reveals by these)
    ch: usize,
}

/// Shape a line with HarfBuzz rules (rustybuzz): right-to-left scripts,
/// joined Arabic letters, Indic and Thai marks, ligatures and kerning. Mixed
/// direction text is split into runs and put in visual order first (Unicode
/// bidi). Falls back to plain glyph-by-glyph layout if the font can't be shaped.
fn shape_line(f: &FontEntry, line: &str, scale: PxScale) -> (Vec<Placed>, f32) {
    let sf = f.font.as_scaled(scale);
    if let Some(mut face) = rustybuzz::Face::from_slice(&f.data, f.index) {
        if !f.coords.is_empty() {
            let vars: Vec<rustybuzz::Variation> = f.coords.iter().map(|(t, v)| rustybuzz::Variation { tag: rustybuzz::ttf_parser::Tag::from_bytes(t), value: *v }).collect();
            face.set_variations(&vars);
        }
        let (hs, vs) = (sf.h_scale_factor(), sf.v_scale_factor());
        let mut x = 0.0;
        let mut v = Vec::new();
        // byte offset to character number
        let char_at = |b: usize| line[..b.min(line.len())].chars().count();
        let bidi = unicode_bidi::BidiInfo::new(line, None);
        let runs: Vec<(std::ops::Range<usize>, bool)> = match bidi.paragraphs.first() {
            Some(para) => {
                let (levels, runs) = bidi.visual_runs(para, para.range.clone());
                runs.into_iter().map(|r| {
                    let rtl = levels[r.start].is_rtl();
                    (r, rtl)
                }).collect()
            }
            None => Vec::new(),
        };
        for (range, rtl) in runs {
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(&line[range.clone()]);
            buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
            buf.guess_segment_properties();
            let out = rustybuzz::shape(&face, &[], buf);
            let start = range.start;
            for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
                v.push(Placed { id: GlyphId(info.glyph_id as u16), x: x + pos.x_offset as f32 * hs, y: -(pos.y_offset as f32) * vs, ch: char_at(start + info.cluster as usize) });
                x += pos.x_advance as f32 * hs;
            }
        }
        return (v, x);
    }
    let mut x = 0.0;
    let mut prev = None;
    let mut v = Vec::new();
    for (ci, c) in line.chars().enumerate() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        v.push(Placed { id, x, y: 0.0, ch: ci });
        x += sf.h_advance(id);
        prev = Some(id);
    }
    (v, x)
}

/// Break `text` into lines no wider than `max_w`, at spaces where it can and
/// inside words that are too long on their own.
pub fn wrap_text(f: &FontEntry, text: &str, size: f32, max_w: f32) -> String {
    let scale = PxScale::from(size);
    let width = |s: &str| shape_line(f, s, scale).1;
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if line.is_empty() || width(&cand) <= max_w {
                line = cand;
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            // a word longer than the line: break it between letters
            while width(&line) > max_w && line.chars().count() > 1 {
                let chars: Vec<char> = line.chars().collect();
                let mut k = chars.len() - 1;
                while k > 1 && width(&chars[..k].iter().collect::<String>()) > max_w {
                    k -= 1;
                }
                out.push(chars[..k].iter().collect());
                line = chars[k..].iter().collect();
            }
        }
        out.push(line);
    }
    out.join("\n")
}

impl Annotation {
    /// The text as it's laid out: wrapped to `wrap_px` when that's set.
    pub fn shown_text(&self, f: &FontEntry) -> std::borrow::Cow<'_, str> {
        if self.wrap_px > 0.0 && (self.wrap || self.caption != Caption::None) {
            let room = (self.wrap_px - 2.0 * self.outline_px() as f32) / self.stretch.clamp(0.05, 20.0);
            std::borrow::Cow::Owned(wrap_text(f, &self.text, self.size, room.max(self.size)))
        } else {
            std::borrow::Cow::Borrowed(&self.text)
        }
    }
}

/// Rasterise text to a coverage mask (w, h, alpha) – no colour.
/// Width and height text takes at `size`, without drawing it.
pub fn extent(f: &FontEntry, text: &str, size: f32) -> (u32, u32) {
    let scale = PxScale::from(size);
    let sf = f.font.as_scaled(scale);
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    let n = text.split('\n').count();
    let maxw = text.split('\n').map(|l| shape_line(f, l, scale).1).fold(0.0, f32::max).ceil() as u32 + 2;
    (maxw, (line_h * n as f32).ceil() as u32 + 2)
}

/// The most pixels one piece of text may cover (about 200 MB of picture).
/// Bigger text is drawn smaller to fit, instead of running out of memory.
const MAX_TEXT_PIXELS: f32 = 48_000_000.0;

impl Annotation {
    /// This text, made smaller when drawing it at its size would go past
    /// MAX_TEXT_PIXELS (huge sizes, or long text wrapped into a tall column).
    pub fn within_budget(&self, f: &FontEntry) -> std::borrow::Cow<'_, Annotation> {
        let (w, h) = extent(f, &self.shown_text(f), self.size);
        let r = self.outline_px() as f32;
        let area = (stretched(w, self.stretch) as f32 + 2.0 * r) * (stretched(h, self.stretch_y) as f32 + 2.0 * r);
        if area <= MAX_TEXT_PIXELS || self.size <= 1.0 {
            return std::borrow::Cow::Borrowed(self);
        }
        let k = (MAX_TEXT_PIXELS / area).sqrt() * 0.98;
        let mut a = self.clone();
        a.size = (self.size * k).max(1.0);
        a.border_width = Some(r * k);
        a.wrap_px = self.wrap_px * k;
        std::borrow::Cow::Owned(a)
    }
}

/// `rasterize`, drawing only the first `limit` characters (not counting
/// line breaks) while laying out all of them, so nothing moves as more show.
pub fn rasterize_upto(f: &FontEntry, text: &str, size: f32, align: Align, limit: Option<usize>) -> (u32, u32, Vec<f32>) {
    let font = &f.font;
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    let lines: Vec<(Vec<Placed>, f32)> = text.split('\n').map(|l| shape_line(f, l, scale)).collect();
    let maxw = lines.iter().map(|l| l.1).fold(0.0, f32::max).ceil() as u32 + 2;
    let total_h = (line_h * lines.len() as f32).ceil() as u32 + 2;
    let mut mask = vec![0f32; (maxw * total_h) as usize];
    // characters on the lines before (line breaks don't count)
    let line_start: Vec<usize> = text.split('\n').scan(0usize, |acc, l| {
        let s = *acc;
        *acc += l.chars().count();
        Some(s)
    }).collect();
    for (li, (glyphs, width)) in lines.iter().enumerate() {
        let off_x = match align {
            Align::Left => 0.0,
            Align::Center => (maxw as f32 - width) / 2.0,
            Align::Right => maxw as f32 - width,
        };
        let base_y = sf.ascent() + li as f32 * line_h;
        for g in glyphs {
            if limit.is_some_and(|n| line_start[li] + g.ch >= n) {
                continue;
            }
            let gl = g.id.with_scale_and_position(scale, ab_glyph::point(off_x + g.x, base_y + g.y));
            if let Some(og) = font.outline_glyph(gl) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as u32) < maxw && (py as u32) < total_h {
                        let i = (py as u32 * maxw + px as u32) as usize;
                        mask[i] = (mask[i] + cov).min(1.0);
                    }
                });
            }
        }
    }
    (maxw, total_h, mask)
}

impl Annotation {
    /// Outline width in whole pixels.
    pub fn outline_px(&self) -> i32 {
        match self.border_width {
            Some(w) => w.round().clamp(0.0, 200.0) as i32,
            None => self.border.radius(self.size),
        }
    }
}

/// Size of the rendered text block including border.
pub fn measure(fonts: &[FontEntry], a: &Annotation) -> (u32, u32) {
    let font = &fonts[a.font_idx.min(fonts.len() - 1)];
    let a = &*a.within_budget(font);
    let (w, h) = extent(font, &a.shown_text(font), a.size);
    let r = a.outline_px() as u32;
    (stretched(w + 2 * r, a.stretch), stretched(h + 2 * r, a.stretch_y))
}

fn stretched(w: u32, s: f32) -> u32 {
    if (s - 1.0).abs() < 1e-3 || !s.is_finite() {
        w
    } else {
        ((w as f32 * s.clamp(0.05, 20.0)).round() as u32).max(1)
    }
}

/// Pre-rendered RGBA sprite of an annotation (so it is rasterised once per apply).
pub fn render_sprite(fonts: &[FontEntry], a: &Annotation) -> RgbaImage {
    render_sprite_upto(fonts, a, None)
}

/// `render_sprite` with only the first `limit` characters drawn (typing).
pub fn render_sprite_upto(fonts: &[FontEntry], a: &Annotation, limit: Option<usize>) -> RgbaImage {
    let font = &fonts[a.font_idx.min(fonts.len() - 1)];
    let a = &*a.within_budget(font);
    let (w, h, mask) = rasterize_upto(font, &a.shown_text(font), a.size, a.align, limit);
    let r = a.outline_px();
    let sw = w + 2 * r as u32;
    let sh = h + 2 * r as u32;
    let mut out = RgbaImage::new(sw, sh);
    if r > 0 {
        // the outline: everything within r of the letters, from a distance
        // map (one pass per row and column, so thick outlines stay quick)
        let mut inside = vec![false; (sw * sh) as usize];
        for y in 0..h {
            for x in 0..w {
                if mask[(y * w + x) as usize] >= 0.5 {
                    inside[((y + r as u32) * sw + x + r as u32) as usize] = true;
                }
            }
        }
        let d2 = crate::ops::distance_sq(&inside, sw as usize, sh as usize);
        let rf = r as f32 + 0.5;
        let dil: Vec<f32> = d2
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let (x, y) = (i as u32 % sw, i as u32 / sw);
                // soft letter edges count too, so thin strokes keep their outline
                let own = if x >= r as u32 && y >= r as u32 && x - (r as u32) < w && y - (r as u32) < h { mask[((y - r as u32) * w + x - r as u32) as usize] } else { 0.0 };
                (rf + 0.5 - d.sqrt()).clamp(0.0, 1.0).max(own)
            })
            .collect();
        let bc = a.border_color;
        for (i, v) in dil.iter().enumerate() {
            if *v > 0.0 {
                let x = i as u32 % sw;
                let y = i as u32 / sw;
                out.put_pixel(x, y, Rgba([bc[0], bc[1], bc[2], (bc[3] as f32 * v) as u8]));
            }
        }
    }
    let c = a.color;
    for y in 0..h {
        for x in 0..w {
            let v = mask[(y * w + x) as usize];
            if v > 0.0 {
                let px = out.get_pixel_mut(x + r as u32, y + r as u32);
                *px = blend_over(*px, Rgba([c[0], c[1], c[2], (c[3] as f32 * v) as u8]));
            }
        }
    }
    let (nw, nh) = (stretched(out.width(), a.stretch), stretched(out.height(), a.stretch_y));
    if (nw, nh) != out.dimensions() {
        out = image::imageops::resize(&out, nw, nh, image::imageops::FilterType::CatmullRom);
    }
    out
}

/// Draw a pre-rendered sprite with the annotation's opacity.
pub fn draw_sprite(img: &mut RgbaImage, sprite: &RgbaImage, a: &Annotation) {
    crate::ops::overlay_with_opacity(img, sprite, a.x as i64, a.y as i64, a.opacity as f32 / 100.0);
}

/// Convenience: draw arbitrary text with defaults (used for the seconds counter).
pub fn draw_simple(img: &mut RgbaImage, fonts: &[FontEntry], text: &str, x: i32, y: i32, size: f32) {
    let a = Annotation { text: text.into(), size, x, y, align: Align::Left, ..Default::default() };
    let s = render_sprite(fonts, &a);
    draw_sprite(img, &s, &a);
}
