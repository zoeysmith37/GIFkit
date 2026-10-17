//! Text rendering onto frames (ab_glyph).

use crate::ops::blend_over;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

pub static SANS_BOLD: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");
pub static MONO_BOLD: &[u8] = include_bytes!("../assets/DejaVuSansMono-Bold.ttf");

#[derive(Clone)]
pub struct FontEntry {
    pub name: String,
    pub font: FontArc,
}

pub fn builtin_fonts() -> Vec<FontEntry> {
    vec![
        FontEntry { name: "Sans Bold (built-in)".into(), font: FontArc::try_from_slice(SANS_BOLD).expect("font") },
        FontEntry { name: "Mono Bold (built-in)".into(), font: FontArc::try_from_slice(MONO_BOLD).expect("font") },
        FontEntry { name: "Noto Sans (built-in)".into(), font: FontArc::try_from_slice(crate::theme::NOTO_MEDIUM).expect("font") },
    ]
}

pub fn load_font_file(path: &std::path::Path) -> anyhow::Result<FontEntry> {
    let data = std::fs::read(path)?;
    let font = FontArc::try_from_vec(data).map_err(|e| anyhow::anyhow!("Invalid font: {e}"))?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Custom".into());
    Ok(FontEntry { name, font })
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
}

impl Default for Annotation {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_idx: 0,
            size: 32.0,
            color: [255, 255, 255, 255],
            border: Border::Light,
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
        }
    }
}

impl Annotation {
    pub fn applies_to(&self, frame_idx0: usize) -> bool {
        let i = frame_idx0 + 1;
        i >= self.from.max(1) && (self.to == 0 || i <= self.to)
    }
}

/// Rasterise text to a coverage mask (w, h, alpha) – no colour.
pub fn rasterize(font: &FontArc, text: &str, size: f32, align: Align) -> (u32, u32, Vec<f32>) {
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    let lines: Vec<&str> = text.split('\n').collect();
    let widths: Vec<f32> = lines
        .iter()
        .map(|l| {
            let mut w = 0.0;
            let mut prev = None;
            for c in l.chars() {
                let id = sf.glyph_id(c);
                if let Some(p) = prev {
                    w += sf.kern(p, id);
                }
                w += sf.h_advance(id);
                prev = Some(id);
            }
            w
        })
        .collect();
    let maxw = widths.iter().cloned().fold(0.0, f32::max).ceil() as u32 + 2;
    let total_h = (line_h * lines.len() as f32).ceil() as u32 + 2;
    let mut mask = vec![0f32; (maxw * total_h) as usize];
    for (li, line) in lines.iter().enumerate() {
        let off_x = match align {
            Align::Left => 0.0,
            Align::Center => (maxw as f32 - widths[li]) / 2.0,
            Align::Right => maxw as f32 - widths[li],
        };
        let base_y = sf.ascent() + li as f32 * line_h;
        let mut x = off_x;
        let mut prev = None;
        for c in line.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, ab_glyph::point(x, base_y));
            x += sf.h_advance(id);
            prev = Some(id);
            if let Some(og) = font.outline_glyph(g) {
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
    let font = &fonts[a.font_idx.min(fonts.len() - 1)].font;
    let (w, h, _) = rasterize(font, &a.text, a.size, a.align);
    let r = a.outline_px() as u32;
    (w + 2 * r, h + 2 * r)
}

/// Pre-rendered RGBA sprite of an annotation (so it is rasterised once per apply).
pub fn render_sprite(fonts: &[FontEntry], a: &Annotation) -> RgbaImage {
    let font = &fonts[a.font_idx.min(fonts.len() - 1)].font;
    let (w, h, mask) = rasterize(font, &a.text, a.size, a.align);
    let r = a.outline_px();
    let sw = w + 2 * r as u32;
    let sh = h + 2 * r as u32;
    let mut out = RgbaImage::new(sw, sh);
    if r > 0 {
        // dilated mask for outline
        let mut dil = vec![0f32; (sw * sh) as usize];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let v = mask[(y as u32 * w + x as u32) as usize];
                if v <= 0.0 {
                    continue;
                }
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy > r * r + r {
                            continue;
                        }
                        let tx = (x + r + dx) as u32;
                        let ty = (y + r + dy) as u32;
                        let i = (ty * sw + tx) as usize;
                        if dil[i] < v {
                            dil[i] = v;
                        }
                    }
                }
            }
        }
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
