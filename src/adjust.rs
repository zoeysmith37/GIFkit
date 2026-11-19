//! Photoshop-style adjustments: Levels, Curves, Exposure, Vibrance, Color
//! Balance, Black & White, Photo Filter, Channel Mixer, Selective Color,
//! Gradient Map, Threshold, built-in looks and .cube colour lookups.
//!
//! An adjustment layer holds a list of them, applied in order. Every one is
//! a colour-to-colour map, so the whole list is baked into one 3D lookup
//! table (65³) that the CPU and the graphics card both read the same way.
//!
//! Several formulas follow PhotoCraft (github.com/storytold/photocraft,
//! MIT or Apache-2.0), which measured them against Photoshop: the Levels
//! gamma toe, the Curves spline, Color Balance, Vibrance, Selective Color,
//! Photo Filter and the Black & White mix. Its MIT license is in
//! assets/PHOTOCRAFT_LICENSE.txt.

use image::RgbaImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Points along each side of the lookup table.
pub const N: usize = 65;
/// The table's 65 slices sit in a 9 × 9 grid of tiles.
pub const TILES: usize = 9;
pub const LUT_SIDE: u32 = (N * TILES) as u32;

/// One adjustment in a layer's list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustItem {
    pub on: bool,
    pub adj: Adjust,
}

impl Default for AdjustItem {
    fn default() -> Self {
        AdjustItem { on: true, adj: Adjust::Levels(Levels::default()) }
    }
}

impl AdjustItem {
    pub fn new(adj: Adjust) -> Self {
        AdjustItem { on: true, adj }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Adjust {
    Levels(Levels),
    Curves(Curves),
    Exposure(Exposure),
    BrightContrast(BrightContrast),
    Vibrance(Vibrance),
    HueSat(HueSat),
    ColorBalance(ColorBalance),
    BlackWhite(BlackWhite),
    PhotoFilter(PhotoFilter),
    ChannelMixer(ChannelMixer),
    SelectiveColor(SelectiveColor),
    GradientMap(GradientMap),
    Threshold(Threshold),
    Invert(Invert),
    Posterize(Posterize),
    Look(LookAdj),
    Cube(Cube),
}

/// What the Add menu offers, in order.
pub const KINDS: [&str; 17] = [
    "Levels", "Curves", "Exposure", "Brightness/Contrast", "Vibrance", "Hue/Saturation", "Color Balance", "Black & White", "Photo Filter", "Channel Mixer", "Selective Color", "Invert", "Posterize", "Threshold", "Gradient Map", "Looks", "Color Lookup",
];

/// Where the Add menu puts a line: before these.
pub const MENU_BREAKS: [usize; 3] = [4, 11, 15];

impl Adjust {
    /// A fresh one of `KINDS[k]`.
    pub fn make(k: usize) -> Adjust {
        match k {
            0 => Adjust::Levels(Levels::default()),
            1 => Adjust::Curves(Curves::default()),
            2 => Adjust::Exposure(Exposure::default()),
            3 => Adjust::BrightContrast(BrightContrast::default()),
            4 => Adjust::Vibrance(Vibrance::default()),
            5 => Adjust::HueSat(HueSat::default()),
            6 => Adjust::ColorBalance(ColorBalance::default()),
            7 => Adjust::BlackWhite(BlackWhite::default()),
            8 => Adjust::PhotoFilter(PhotoFilter::default()),
            9 => Adjust::ChannelMixer(ChannelMixer::default()),
            10 => Adjust::SelectiveColor(SelectiveColor::default()),
            11 => Adjust::Invert(Invert {}),
            12 => Adjust::Posterize(Posterize::default()),
            13 => Adjust::Threshold(Threshold::default()),
            14 => Adjust::GradientMap(GradientMap::default()),
            15 => Adjust::Look(LookAdj::default()),
            _ => Adjust::Cube(Cube::default()),
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Adjust::Levels(_) => 0,
            Adjust::Curves(_) => 1,
            Adjust::Exposure(_) => 2,
            Adjust::BrightContrast(_) => 3,
            Adjust::Vibrance(_) => 4,
            Adjust::HueSat(_) => 5,
            Adjust::ColorBalance(_) => 6,
            Adjust::BlackWhite(_) => 7,
            Adjust::PhotoFilter(_) => 8,
            Adjust::ChannelMixer(_) => 9,
            Adjust::SelectiveColor(_) => 10,
            Adjust::Invert(_) => 11,
            Adjust::Posterize(_) => 12,
            Adjust::Threshold(_) => 13,
            Adjust::GradientMap(_) => 14,
            Adjust::Look(_) => 15,
            Adjust::Cube(_) => 16,
        }
    }

    pub fn name(&self) -> &'static str {
        KINDS[self.index()]
    }

    /// Breeze icon for it.
    pub fn icon(k: usize) -> &'static str {
        [
            "view-object-histogram-linear", "draw-bezier-curves", "photo", "colors-luma", "format-fill-color", "color-mode-hue-shift-positive", "color-management", "color-mode-black-white", "view-filter", "colors-chromared", "color-picker-grey", "color-mode-invert-image", "pixelate", "color-fill", "color-gradient", "tool_imageeffects", "kstars_colors",
        ][k]
    }
}

/// How an adjustment layer's result meets what's under it (Photoshop's
/// blend modes, worked into the lookup table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum Blend {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    Lighten,
    Screen,
    ColorDodge,
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    pub fn is_normal(&self) -> bool {
        *self == Blend::Normal
    }

    /// For the shader: its place in `ALL`.
    pub fn code(self) -> u32 {
        Blend::ALL.iter().position(|b| *b == self).unwrap_or(0) as u32
    }

    pub const ALL: [Blend; 16] = [
        Blend::Normal, Blend::Darken, Blend::Multiply, Blend::ColorBurn, Blend::Lighten, Blend::Screen, Blend::ColorDodge, Blend::Overlay, Blend::SoftLight, Blend::HardLight, Blend::Difference, Blend::Exclusion, Blend::Hue, Blend::Saturation, Blend::Color, Blend::Luminosity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Darken => "Darken",
            Blend::Multiply => "Multiply",
            Blend::ColorBurn => "Color Burn",
            Blend::Lighten => "Lighten",
            Blend::Screen => "Screen",
            Blend::ColorDodge => "Color Dodge",
            Blend::Overlay => "Overlay",
            Blend::SoftLight => "Soft Light",
            Blend::HardLight => "Hard Light",
            Blend::Difference => "Difference",
            Blend::Exclusion => "Exclusion",
            Blend::Hue => "Hue",
            Blend::Saturation => "Saturation",
            Blend::Color => "Color",
            Blend::Luminosity => "Luminosity",
        }
    }

    /// The adjusted colour `s` laid over the original `b` (W3C compositing).
    pub fn apply(self, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
        let sep = |f: &dyn Fn(f32, f32) -> f32| -> [f32; 3] { std::array::from_fn(|i| f(b[i], s[i]).clamp(0.0, 1.0)) };
        let hard = |cb: f32, cs: f32| if cs <= 0.5 { cb * 2.0 * cs } else { let t = 2.0 * cs - 1.0; cb + t - cb * t };
        match self {
            Blend::Normal => s,
            Blend::Darken => sep(&|cb, cs| cb.min(cs)),
            Blend::Multiply => sep(&|cb, cs| cb * cs),
            Blend::ColorBurn => sep(&|cb, cs| if cb >= 1.0 { 1.0 } else if cs <= 0.0 { 0.0 } else { 1.0 - ((1.0 - cb) / cs).min(1.0) }),
            Blend::Lighten => sep(&|cb, cs| cb.max(cs)),
            Blend::Screen => sep(&|cb, cs| cb + cs - cb * cs),
            Blend::ColorDodge => sep(&|cb, cs| if cb <= 0.0 { 0.0 } else if cs >= 1.0 { 1.0 } else { (cb / (1.0 - cs)).min(1.0) }),
            Blend::Overlay => sep(&|cb, cs| hard(cs, cb)),
            Blend::SoftLight => sep(&|cb, cs| {
                if cs <= 0.5 {
                    cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
                } else {
                    let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                    cb + (2.0 * cs - 1.0) * (d - cb)
                }
            }),
            Blend::HardLight => sep(&hard),
            Blend::Difference => sep(&|cb, cs| (cb - cs).abs()),
            Blend::Exclusion => sep(&|cb, cs| cb + cs - 2.0 * cb * cs),
            Blend::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
            Blend::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
            Blend::Color => set_lum(s, lum(b)),
            Blend::Luminosity => set_lum(b, lum(s)),
        }
    }
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// `c` with its saturation (max − min) set to `s`, its hue kept.
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx - mn <= 1e-9 {
        return [0.0; 3];
    }
    c.map(|v| (v - mn) * s / (mx - mn))
}

/// Photoshop's Hue/Saturation: hue −180..180°, saturation and lightness
/// −100..100; colorize paints one hue over the lightness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HueSat {
    pub hue: f32,
    pub saturation: f32,
    pub lightness: f32,
    pub colorize: bool,
}

/// Brightness −150..150, contrast −50..100. Legacy: the old straight-line
/// version (and GIFkit's older Effects sliders), −100..100 each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BrightContrast {
    pub brightness: f32,
    pub contrast: f32,
    pub legacy: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Invert {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Posterize {
    pub levels: u32,
}

impl Default for Posterize {
    fn default() -> Self {
        Posterize { levels: 4 }
    }
}

// ---------------------------------------------------------------- settings

/// One channel's Levels, in 0..255 levels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelsCh {
    pub in_black: f32,
    pub in_white: f32,
    pub gamma: f32,
    pub out_black: f32,
    pub out_white: f32,
}

impl Default for LevelsCh {
    fn default() -> Self {
        LevelsCh { in_black: 0.0, in_white: 255.0, gamma: 1.0, out_black: 0.0, out_white: 255.0 }
    }
}

/// `ch[0]` all three channels, then red, green and blue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Levels {
    pub ch: [LevelsCh; 4],
}

/// Curve points in 0..255 (input, output), per channel as Levels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    pub ch: [Vec<[f32; 2]>; 4],
}

impl Default for Curves {
    fn default() -> Self {
        Curves { ch: std::array::from_fn(|_| identity_curve()) }
    }
}

pub fn identity_curve() -> Vec<[f32; 2]> {
    vec![[0.0, 0.0], [255.0, 255.0]]
}

/// Stops of light, a lift, and a gamma, worked in linear light.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exposure {
    pub exposure: f32,
    pub offset: f32,
    pub gamma: f32,
}

impl Default for Exposure {
    fn default() -> Self {
        Exposure { exposure: 0.0, offset: 0.0, gamma: 1.0 }
    }
}

/// −100..100 each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Vibrance {
    pub vibrance: f32,
    pub saturation: f32,
}

/// Shadows, midtones and highlights, each cyan–red, magenta–green and
/// yellow–blue in −100..100.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorBalance {
    pub tones: [[f32; 3]; 3],
    pub preserve: bool,
}

impl Default for ColorBalance {
    fn default() -> Self {
        ColorBalance { tones: [[0.0; 3]; 3], preserve: true }
    }
}

/// How much each colour counts toward the grey (%): reds, yellows, greens,
/// cyans, blues, magentas. And a tint over it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlackWhite {
    pub weights: [f32; 6],
    pub tint: bool,
    pub tint_color: [u8; 3],
}

pub const BW_DEFAULT: [f32; 6] = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];

impl Default for BlackWhite {
    fn default() -> Self {
        BlackWhite { weights: BW_DEFAULT, tint: false, tint_color: [225, 211, 179] }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhotoFilter {
    pub color: [u8; 3],
    /// %
    pub density: f32,
    pub preserve: bool,
}

impl Default for PhotoFilter {
    fn default() -> Self {
        PhotoFilter { color: [236, 138, 0], density: 25.0, preserve: true }
    }
}

/// Each output channel from red, green, blue (%) plus a constant (%).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMixer {
    pub rows: [[f32; 4]; 3],
    pub mono: bool,
}

impl Default for ChannelMixer {
    fn default() -> Self {
        ChannelMixer { rows: [[100.0, 0.0, 0.0, 0.0], [0.0, 100.0, 0.0, 0.0], [0.0, 0.0, 100.0, 0.0]], mono: false }
    }
}

/// Reds, yellows, greens, cyans, blues, magentas, whites, neutrals, blacks:
/// cyan, magenta, yellow and black ink, −100..100 each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectiveColor {
    pub ranges: [[f32; 4]; 9],
    pub relative: bool,
}

impl Default for SelectiveColor {
    fn default() -> Self {
        SelectiveColor { ranges: [[0.0; 4]; 9], relative: true }
    }
}

pub const SC_RANGES: [&str; 9] = ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals", "Blacks"];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    /// 0..1
    pub at: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GradientMap {
    pub stops: Vec<Stop>,
    pub reverse: bool,
}

impl Default for GradientMap {
    fn default() -> Self {
        GradientMap { stops: GRADIENTS[0].1.to_vec(), reverse: false }
    }
}

const fn st(at: f32, c: [u8; 3]) -> Stop {
    Stop { at, color: c }
}

/// Gradient Map presets.
pub const GRADIENTS: [(&str, &[Stop]); 8] = [
    ("Black, white", &[st(0.0, [0, 0, 0]), st(1.0, [255, 255, 255])]),
    ("Sepia", &[st(0.0, [26, 14, 6]), st(0.5, [150, 100, 60]), st(1.0, [255, 244, 222])]),
    ("Duotone blue", &[st(0.0, [12, 20, 60]), st(1.0, [180, 230, 255])]),
    ("Sunset", &[st(0.0, [20, 4, 40]), st(0.35, [160, 30, 90]), st(0.7, [250, 130, 40]), st(1.0, [255, 240, 170])]),
    ("Infrared", &[st(0.0, [0, 0, 0]), st(0.3, [90, 0, 140]), st(0.6, [240, 40, 60]), st(0.85, [255, 210, 40]), st(1.0, [255, 255, 255])]),
    ("Game Boy", &[st(0.0, [15, 56, 15]), st(0.33, [48, 98, 48]), st(0.66, [139, 172, 15]), st(1.0, [155, 188, 15])]),
    ("Neon", &[st(0.0, [10, 0, 30]), st(0.5, [255, 0, 170]), st(1.0, [0, 255, 240])]),
    ("Gold", &[st(0.0, [30, 18, 0]), st(0.6, [200, 150, 40]), st(1.0, [255, 250, 210])]),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Threshold {
    /// 1..255: lighter than this goes white
    pub level: f32,
}

impl Default for Threshold {
    fn default() -> Self {
        Threshold { level: 128.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Look {
    #[default]
    TealOrange,
    FadedFilm,
    BleachBypass,
    CrossProcess,
    DayForNight,
    GoldenHour,
    Matte,
    Noir,
    Vaporwave,
    Gotham,
    Nashville,
}

impl Look {
    pub const ALL: [Look; 11] = [Look::TealOrange, Look::FadedFilm, Look::BleachBypass, Look::CrossProcess, Look::DayForNight, Look::GoldenHour, Look::Matte, Look::Noir, Look::Vaporwave, Look::Gotham, Look::Nashville];

    pub fn label(self) -> &'static str {
        match self {
            Look::TealOrange => "Teal and orange",
            Look::FadedFilm => "Faded film",
            Look::BleachBypass => "Bleach bypass",
            Look::CrossProcess => "Cross process",
            Look::DayForNight => "Day for night",
            Look::GoldenHour => "Golden hour",
            Look::Matte => "Matte",
            Look::Noir => "Noir",
            Look::Vaporwave => "Vaporwave",
            Look::Gotham => "Gotham",
            Look::Nashville => "Nashville",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookAdj {
    pub look: Look,
    /// %
    pub amount: f32,
}

impl Default for LookAdj {
    fn default() -> Self {
        LookAdj { look: Look::TealOrange, amount: 100.0 }
    }
}

/// A loaded .cube file, kept in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cube {
    pub name: String,
    pub lut: CubeData,
    /// %
    pub amount: f32,
}

impl Default for Cube {
    fn default() -> Self {
        Cube { name: String::new(), lut: CubeData::default(), amount: 100.0 }
    }
}

/// A cube lookup, `size`³ RGB bytes, red fastest. Saved as base64; shown
/// in Debug (and so in layer signatures) by a hash, not its bytes.
#[derive(Clone, Default)]
pub struct CubeData {
    pub size: usize,
    pub data: Arc<Vec<u8>>,
    hash: u64,
}

impl PartialEq for CubeData {
    fn eq(&self, o: &Self) -> bool {
        self.size == o.size && self.hash == o.hash && (Arc::ptr_eq(&self.data, &o.data) || self.data == o.data)
    }
}

impl std::fmt::Debug for CubeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cube({} {:x})", self.size, self.hash)
    }
}

impl CubeData {
    pub fn new(size: usize, data: Vec<u8>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (size, &data).hash(&mut h);
        CubeData { size, data: Arc::new(data), hash: h.finish() }
    }

    pub fn is_empty(&self) -> bool {
        self.size < 2 || self.data.len() < self.size.pow(3) * 3
    }

    fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let at = |r: usize, g: usize, b: usize| {
            let k = ((b * n + g) * n + r) * 3;
            [self.data[k] as f32 / 255.0, self.data[k + 1] as f32 / 255.0, self.data[k + 2] as f32 / 255.0]
        };
        trilinear(c, n, at)
    }
}

#[derive(Serialize, Deserialize)]
struct CubeSer {
    size: usize,
    data: String,
}

impl Serialize for CubeData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        CubeSer { size: self.size, data: b64_encode(&self.data) }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for CubeData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let c = CubeSer::deserialize(d)?;
        let data = b64_decode(&c.data).unwrap_or_default();
        Ok(CubeData::new(c.size, data))
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for ch in b.chunks(3) {
        let v = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= ch.len() {
                s.push(B64[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut v, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let d = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        v = v << 6 | d as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((v >> bits) as u8);
        }
    }
    Some(out)
}

/// Read a .cube file (Adobe/Resolve 3D LUT), kept at up to 33 a side.
pub fn parse_cube(text: &str) -> Result<CubeData, String> {
    let mut size = 0usize;
    let (mut lo, mut hi) = ([0.0f32; 3], [1.0f32; 3]);
    let mut vals: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let head = it.next().unwrap_or("");
        let nums = |it: std::str::SplitWhitespace| -> Vec<f32> { it.filter_map(|t| t.parse().ok()).collect() };
        match head {
            "LUT_3D_SIZE" => size = it.next().and_then(|t| t.parse().ok()).unwrap_or(0),
            "LUT_1D_SIZE" => return Err("1D lookups aren't supported, only 3D ones".into()),
            "DOMAIN_MIN" => {
                let v = nums(it);
                if v.len() == 3 {
                    lo = [v[0], v[1], v[2]];
                }
            }
            "DOMAIN_MAX" => {
                let v = nums(it);
                if v.len() == 3 {
                    hi = [v[0], v[1], v[2]];
                }
            }
            "TITLE" | "LUT_3D_INPUT_RANGE" => {}
            _ => {
                if let Ok(r) = head.parse::<f32>() {
                    let v = nums(it);
                    if v.len() >= 2 {
                        vals.push([r, v[0], v[1]]);
                    }
                }
            }
        }
    }
    if !(2..=256).contains(&size) {
        return Err("No LUT_3D_SIZE in the file".into());
    }
    if vals.len() < size.pow(3) {
        return Err(format!("Expected {} colours, found {}", size.pow(3), vals.len()));
    }
    let norm = |v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| ((v[i] - lo[i]) / (hi[i] - lo[i]).max(1e-6)).clamp(0.0, 1.0)) };
    // kept at most 33 a side: resampled when bigger
    let out_n = size.min(33);
    let src_at = |r: usize, g: usize, b: usize| norm(vals[(b * size + g) * size + r]);
    let mut data = Vec::with_capacity(out_n.pow(3) * 3);
    for b in 0..out_n {
        for g in 0..out_n {
            for r in 0..out_n {
                let c = if out_n == size {
                    src_at(r, g, b)
                } else {
                    let f = |i: usize| i as f32 / (out_n - 1) as f32;
                    trilinear([f(r), f(g), f(b)], size, src_at)
                };
                data.extend(c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8));
            }
        }
    }
    Ok(CubeData::new(out_n, data))
}

/// `at(r, g, b)` on an `n`-point grid, read between points.
fn trilinear(c: [f32; 3], n: usize, at: impl Fn(usize, usize, usize) -> [f32; 3]) -> [f32; 3] {
    let f = c.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
    let i = f.map(|v| (v.floor() as usize).min(n - 2));
    let t: [f32; 3] = std::array::from_fn(|k| f[k] - i[k] as f32);
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
    let c00 = lerp(at(i[0], i[1], i[2]), at(i[0] + 1, i[1], i[2]), t[0]);
    let c10 = lerp(at(i[0], i[1] + 1, i[2]), at(i[0] + 1, i[1] + 1, i[2]), t[0]);
    let c01 = lerp(at(i[0], i[1], i[2] + 1), at(i[0] + 1, i[1], i[2] + 1), t[0]);
    let c11 = lerp(at(i[0], i[1] + 1, i[2] + 1), at(i[0] + 1, i[1] + 1, i[2] + 1), t[0]);
    lerp(lerp(c00, c10, t[1]), lerp(c01, c11, t[1]), t[2])
}

// ---------------------------------------------------------------- maths

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

const D50: [f32; 3] = [0.964_22, 1.0, 0.825_21];
const SRGB_TO_XYZ_D50: [[f32; 3]; 3] = [[0.436_074, 0.385_064, 0.143_080], [0.222_504, 0.716_878, 0.060_618], [0.013_932, 0.097_104, 0.714_173]];
const XYZ_D50_TO_SRGB: [[f32; 3]; 3] = [[3.133_856, -1.616_867, -0.490_615], [-0.978_768, 1.916_142, 0.033_454], [0.071_945, -0.228_991, 1.405_243]];

fn mat3(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2])
}

/// The blend-mode luminosity (W3C).
pub fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// `c` moved to luminosity `l`, its hue kept (the Luminosity blend).
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    let c = c.map(|v| v + d);
    let l = lum(c);
    let (mn, mx) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    let mut c = c;
    if mn < 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - mn).max(1e-6));
    }
    if mx > 1.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (mx - l).max(1e-6));
    }
    c.map(|v| v.clamp(0.0, 1.0))
}

/// Levels' midtone gamma: `u^(1/g)`, but above 1 a toe of slope 2^g that
/// joins the power curve, as Photoshop does (PhotoCraft's fit).
fn levels_gamma(u: f32, g: f32) -> f32 {
    let g = g.clamp(0.01, 9.99);
    let c = 1.0 / g;
    if g <= 1.0 {
        return u.powf(c);
    }
    let (a, q) = (2f32.powf(g), 2.2f32);
    let u1 = ((1.0 - c / q) / (a * (1.0 - 1.0 / q))).powf(1.0 / (1.0 - c));
    if u >= u1 || u1 <= 0.0 {
        return u.powf(c);
    }
    let b = (a * u1 - u1.powf(c)) / u1.powf(q);
    if !b.is_finite() {
        return u.powf(c);
    }
    a * u - b * u.powf(q)
}

/// One Levels channel on `v` in 0..1.
pub fn levels(ch: &LevelsCh, v: f32) -> f32 {
    let range = (ch.in_white - ch.in_black).max(1.0);
    let u = ((v * 255.0 - ch.in_black) / range).clamp(0.0, 1.0);
    let u = if (ch.gamma - 1.0).abs() < 1e-4 { u } else { levels_gamma(u, ch.gamma) };
    ((ch.out_black + u * (ch.out_white - ch.out_black)) / 255.0).clamp(0.0, 1.0)
}

/// A natural cubic spline through `points` (0..255), flat past the ends,
/// sampled at `n` inputs over 0..1 (Photoshop's Curves).
pub fn curve_samples(points: &[[f32; 2]], n: usize) -> Vec<f32> {
    let last = (n.max(2) - 1) as f32;
    let mut pts: Vec<(f64, f64)> = points.iter().map(|p| (p[0] as f64 / 255.0, p[1] as f64 / 255.0)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
    if pts.len() < 2 {
        return (0..n).map(|i| i as f32 / last).collect();
    }
    let m = pts.len();
    let mut m2 = vec![0.0f64; m];
    if m > 2 {
        let (mut c, mut d) = (vec![0.0f64; m], vec![0.0f64; m]);
        for i in 1..m - 1 {
            let (h0, h1) = (pts[i].0 - pts[i - 1].0, pts[i + 1].0 - pts[i].0);
            let (a, b, cc) = (h0 / 6.0, (h0 + h1) / 3.0, h1 / 6.0);
            let r = (pts[i + 1].1 - pts[i].1) / h1 - (pts[i].1 - pts[i - 1].1) / h0;
            let den = b - a * c[i - 1];
            c[i] = cc / den;
            d[i] = (r - a * d[i - 1]) / den;
        }
        for i in (1..m - 1).rev() {
            m2[i] = d[i] - c[i] * m2[i + 1];
        }
    }
    (0..n)
        .map(|k| {
            let x = k as f64 / last as f64;
            if x <= pts[0].0 {
                return pts[0].1.clamp(0.0, 1.0) as f32;
            }
            if x >= pts[m - 1].0 {
                return pts[m - 1].1.clamp(0.0, 1.0) as f32;
            }
            let i = pts.windows(2).position(|w| x <= w[1].0).unwrap_or(m - 2);
            let ((x0, y0), (x1, y1)) = (pts[i], pts[i + 1]);
            let h = x1 - x0;
            let (a, b) = ((x1 - x) / h, (x - x0) / h);
            let y = a * y0 + b * y1 + ((a * a * a - a) * m2[i] + (b * b * b - b) * m2[i + 1]) * h * h / 6.0;
            y.clamp(0.0, 1.0) as f32
        })
        .collect()
}

/// A table read between its entries.
fn table(t: &[f32], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * (t.len() - 1) as f32;
    let i = (f.floor() as usize).min(t.len() - 2);
    t[i] + (t[i + 1] - t[i]) * (f - i as f32)
}

/// One channel of Color Balance (PhotoCraft's measured curve).
struct Balance {
    b: f32,
    w: f32,
    e: f32,
    knee: Option<(f32, f32, f32, f32)>,
}

impl Balance {
    fn new(tones: [[f32; 3]; 3], preserve: bool, ch: usize) -> Self {
        let [sh, mid, hi] = tones.map(|t| t.map(|s| s.clamp(-100.0, 100.0)));
        let max = |t: [f32; 3]| t[0].max(t[1]).max(t[2]);
        let min = |t: [f32; 3]| t[0].min(t[1]).min(t[2]);
        let (black, white, m) = if preserve {
            (max(sh) - sh[ch], hi[ch] - min(hi), mid[ch] - (max(mid) + min(mid)) / 2.0)
        } else {
            ((-sh[ch]).max(0.0), hi[ch].max(0.0), mid[ch] + (sh[ch] + hi[ch]) / 2.0)
        };
        let gamma = 2f32.powf(m / 100.0);
        let e = 1.0 / gamma.max(0.01);
        let knee = if gamma > 1.0 {
            let s0 = 2f32.powf(gamma);
            let u0 = (2.0 * s0.powf(1.0 / (e - 1.0))).min(1.0);
            (!u0.is_nan() && u0 > 0.0).then(|| (u0, s0, u0.powf(e), e * u0.powf(e - 1.0)))
        } else {
            None
        };
        Balance { b: black / 255.0, w: 1.0 - white / 255.0, e, knee }
    }

    fn eval(&self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        if v >= self.w {
            return 1.0;
        }
        if v <= self.b {
            return 0.0;
        }
        let u = (v - self.b) / (self.w - self.b);
        match self.knee {
            Some((u0, s0, p, d)) if u < u0 => {
                let t = u / u0;
                let (t2, t3) = (t * t, t * t * t);
                ((t3 - 2.0 * t2 + t) * u0 * s0 + (3.0 * t2 - 2.0 * t3) * p + (t3 - t2) * u0 * d).clamp(0.0, 1.0)
            }
            _ => u.powf(self.e),
        }
    }
}

fn hsv_hue(c: [f32; 3], mx: f32, mn: f32) -> f32 {
    let d = mx - mn;
    let h = if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if mx == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    h * 60.0
}

/// Positive vibrance holds back on reds and skin.
fn vibrance_skin(h: f32) -> f32 {
    if h <= 30.0 {
        1.0
    } else if h < 45.0 {
        (45.0 - h) / 15.0
    } else if h < 300.0 {
        0.0
    } else {
        (h - 300.0) / 60.0
    }
}

/// Vibrance (`v` −1..1) on a linear-light colour (PhotoCraft's fit).
fn vibrance_lin(c: [f32; 3], v: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx <= 0.0 || mx - mn <= 1e-9 {
        return c;
    }
    let t = 1.0 - mn / mx;
    let shape = t * (1.0 - t) * (2.0 - t + t * t) * mx * (1.0 - mx).max(0.0);
    let (t2, v2) = if v < 0.0 {
        let a = -v;
        (t * (1.0 - a / 4.0) * (1.0 - a + a * t * (1.0 + t) / 2.0), mx - a * shape)
    } else {
        let [c0, c1, p, q, r0, ra, rt] = [0.5001f32, 0.0887, 0.4374, 0.7573, -0.8113, 0.5154, 2.8026];
        let w = vibrance_skin(hsv_hue(c, mx, mn));
        let a = v * (1.0 - w) + 0.64 * v.powf(1.6) * w;
        let boost = (c0 * a + c1 * a * a) * t.powf(p) * (1.0 - t).powf(q) * ((r0 + ra * a + rt * (1.0 - t)) * (1.0 - mx).max(0.0)).exp();
        ((t * (1.0 + boost)).min(1.0), mx + v / 4.0 * shape)
    };
    let m2 = v2 * (1.0 - t2);
    let k = (v2 - m2) / (mx - mn);
    c.map(|x| m2 + (x - mn) * k)
}

fn black_white_gray(c: [f32; 3], w: &[f32; 6]) -> f32 {
    let (r, g, b) = (c[0], c[1], c[2]);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let mid = r + g + b - max - min;
    let primary = if r >= g && r >= b {
        0
    } else if g >= b {
        2
    } else {
        4
    };
    let secondary = if b <= r && b <= g {
        1
    } else if r <= g {
        3
    } else {
        5
    };
    (min + (mid - min) * w[secondary] / 100.0 + (max - mid) * w[primary] / 100.0).clamp(0.0, 1.0)
}

fn selective_weights(c: [f32; 3]) -> [f32; 9] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let mid = c[0] + c[1] + c[2] - max - min;
    let top = |i: usize| if c[i] >= max { max - mid } else { 0.0 };
    let bottom = |i: usize| if c[i] <= min { mid - min } else { 0.0 };
    [
        top(0),
        bottom(2),
        top(1),
        bottom(0),
        top(2),
        bottom(1),
        ((min - 0.5) * 2.0).max(0.0),
        (1.0 - (max - 0.5).abs() - (min - 0.5).abs()).clamp(0.0, 1.0),
        ((0.5 - max) * 2.0).max(0.0),
    ]
}

fn selective_color(c: [f32; 3], relative: bool, adj: &[[f32; 4]; 9]) -> [f32; 3] {
    let w = selective_weights(c);
    let mut delta = [0.0f32; 3];
    for (r, wr) in w.iter().enumerate() {
        if *wr <= 0.0 {
            continue;
        }
        let a = adj[r].map(|v| v / 100.0);
        for i in 0..3 {
            let ink = 1.0 - c[i];
            let d = a[i] + a[3];
            let d = if relative {
                d * ink
            } else if d > 0.0 {
                d * (2.0 * (1.0 - ink)).min(1.0)
            } else {
                d * (2.0 * ink).min(1.0)
            };
            delta[i] += d * wr;
        }
    }
    std::array::from_fn(|i| (c[i] - delta[i]).clamp(0.0, 1.0))
}

fn photo_filter_matrix(color: [f32; 3], density: f32) -> [[f32; 3]; 3] {
    let d = density.clamp(0.0, 1.0);
    let xyz = mat3(&SRGB_TO_XYZ_D50, color.map(srgb_to_linear));
    let s: [f32; 3] = std::array::from_fn(|i| 1.0 - d + d * xyz[i] / D50[i]);
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| XYZ_D50_TO_SRGB[r][k] * s[k] * SRGB_TO_XYZ_D50[k][c]).sum()))
}

fn gradient(stops: &[Stop], t: f32) -> [f32; 3] {
    let col = |s: &Stop| s.color.map(|v| v as f32 / 255.0);
    match stops {
        [] => [t; 3],
        [s] => col(s),
        _ => {
            if t <= stops[0].at {
                return col(&stops[0]);
            }
            for w in stops.windows(2) {
                if t <= w[1].at {
                    let k = if w[1].at > w[0].at { (t - w[0].at) / (w[1].at - w[0].at) } else { 0.0 };
                    let (a, b) = (col(&w[0]), col(&w[1]));
                    return std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k);
                }
            }
            col(&stops[stops.len() - 1])
        }
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    let l = lum(c);
    c.map(|v| (l + (v - l) * s).clamp(0.0, 1.0))
}

/// An S-shaped contrast around the middle, `k` 0..1.
fn s_curve(v: f32, k: f32) -> f32 {
    let s = v * v * (3.0 - 2.0 * v);
    v + (s - v) * k
}

fn look(l: Look, c: [f32; 3]) -> [f32; 3] {
    let y = lum(c);
    match l {
        Look::TealOrange => {
            // shadows toward teal, highlights toward orange, skin kept warm
            let sh = 1.0 - smooth(0.0, 0.55, y);
            let hi = smooth(0.45, 1.0, y);
            let c = [c[0] - 0.10 * sh + 0.08 * hi, c[1] + 0.03 * sh + 0.02 * hi, c[2] + 0.10 * sh - 0.10 * hi];
            saturate(c.map(|v| s_curve(v.clamp(0.0, 1.0), 0.35)), 1.1)
        }
        Look::FadedFilm => {
            let c = saturate(c, 0.78).map(|v| 0.08 + v * 0.84);
            [c[0] * 1.03 + 0.01, c[1], c[2] * 0.94 + 0.03].map(|v| v.clamp(0.0, 1.0))
        }
        Look::BleachBypass => {
            // the grey laid over the colour in Overlay, then half the colour out
            let ov = c.map(|v| if y < 0.5 { 2.0 * v * y } else { 1.0 - 2.0 * (1.0 - v) * (1.0 - y) });
            saturate(mix3(c, ov, 0.7), 0.45).map(|v| s_curve(v, 0.3))
        }
        Look::CrossProcess => [s_curve(c[0], 0.7), s_curve(c[1], 0.45) * 0.95 + 0.04, 0.18 + c[2] * 0.64].map(|v| v.clamp(0.0, 1.0)),
        Look::DayForNight => {
            let c = saturate(c, 0.45).map(|v| v * 0.55);
            [c[0] * 0.8, c[1] * 0.92, (c[2] * 1.25 + 0.06).min(1.0)]
        }
        Look::GoldenHour => {
            let c = [c[0] * 1.08 + 0.04, c[1] * 1.0 + 0.015, c[2] * 0.82];
            saturate(c.map(|v| s_curve(v.clamp(0.0, 1.0), 0.2)), 1.12)
        }
        Look::Matte => c.map(|v| 0.06 + s_curve(v, 0.25) * 0.88),
        Look::Noir => {
            let g = black_white_gray(c, &[30.0, 50.0, 40.0, 60.0, 20.0, 50.0]);
            [s_curve(s_curve(g, 1.0), 0.6); 3]
        }
        Look::Vaporwave => {
            let a = [1.0, 0.25, 0.75];
            let b = [0.15, 0.9, 1.0];
            let t = smooth(0.15, 0.9, y);
            let tint = mix3(a, b, t);
            mix3(c, set_lum(tint, (y * 1.05 + 0.04).min(1.0)), 0.55)
        }
        // run from the older filters' own code instead (legacy_cube)
        Look::Gotham | Look::Nashville => c,
    }
}

// ---------------------------------------------------------------- compiled

/// An adjustment ready to run on colours, with its tables worked out.
enum Stage {
    Levels([LevelsCh; 4]),
    Tables([Option<Vec<f32>>; 4]),
    Exposure(f32, f32, f32),
    Vibrance(f32, f32),
    Balance([Balance; 3], bool),
    BlackWhite([f32; 6], Option<[f32; 3]>),
    Matrix([[f32; 3]; 3], bool),
    Mixer([[f32; 4]; 3], bool),
    Selective([[f32; 4]; 9], bool),
    Gradient(Vec<Stop>, bool),
    Threshold(f32),
    Look(Look, f32),
    Cube(CubeData, f32),
    HueSat(f32, f32, f32, bool),
    Invert,
    Posterize(f32),
}

fn compile(items: &[AdjustItem]) -> Vec<Stage> {
    let mut out = Vec::new();
    for it in items.iter().filter(|i| i.on) {
        out.push(match &it.adj {
            Adjust::Levels(l) => {
                if l.ch.iter().all(|c| *c == LevelsCh::default()) {
                    continue;
                }
                Stage::Levels(l.ch)
            }
            Adjust::Curves(c) => {
                let t: [Option<Vec<f32>>; 4] = std::array::from_fn(|k| (c.ch[k] != identity_curve()).then(|| curve_samples(&c.ch[k], 1024)));
                if t.iter().all(|t| t.is_none()) {
                    continue;
                }
                Stage::Tables(t)
            }
            Adjust::Exposure(e) if *e == Exposure::default() => continue,
            Adjust::Vibrance(v) if v.vibrance == 0.0 && v.saturation == 0.0 => continue,
            Adjust::ColorBalance(b) if b.tones == [[0.0; 3]; 3] => continue,
            Adjust::ChannelMixer(m) if *m == ChannelMixer::default() => continue,
            Adjust::SelectiveColor(sc) if sc.ranges == [[0.0; 4]; 9] => continue,
            Adjust::PhotoFilter(pf) if pf.density <= 0.0 => continue,
            Adjust::Look(l) if l.amount <= 0.0 => continue,
            Adjust::HueSat(h) if *h == HueSat::default() => continue,
            Adjust::BrightContrast(b) if b.brightness == 0.0 && b.contrast == 0.0 => continue,
            Adjust::HueSat(h) => Stage::HueSat(h.hue, h.saturation.clamp(-100.0, 100.0) / 100.0, h.lightness.clamp(-100.0, 100.0) / 100.0, h.colorize),
            Adjust::BrightContrast(b) => Stage::Tables([Some(bc_table(b)), None, None, None]),
            Adjust::Invert(_) => Stage::Invert,
            Adjust::Posterize(p) => Stage::Posterize(p.levels.clamp(2, 255) as f32),
            Adjust::Look(l) if matches!(l.look, Look::Gotham | Look::Nashville) => Stage::Cube(legacy_cube(l.look), (l.amount / 100.0).clamp(0.0, 1.0)),
            Adjust::Exposure(e) => Stage::Exposure(2f32.powf(e.exposure.clamp(-20.0, 20.0)), e.offset, e.gamma.clamp(0.01, 9.99)),
            Adjust::Vibrance(v) => Stage::Vibrance(v.vibrance.clamp(-100.0, 100.0) / 100.0, v.saturation.clamp(-100.0, 100.0) / 100.0),
            Adjust::ColorBalance(b) => Stage::Balance(std::array::from_fn(|ch| Balance::new(b.tones, b.preserve, ch)), b.preserve),
            Adjust::BlackWhite(b) => Stage::BlackWhite(b.weights, b.tint.then(|| b.tint_color.map(|v| v as f32 / 255.0))),
            Adjust::PhotoFilter(p) => Stage::Matrix(photo_filter_matrix(p.color.map(|v| v as f32 / 255.0), p.density / 100.0), p.preserve),
            Adjust::ChannelMixer(m) => Stage::Mixer(m.rows.map(|r| r.map(|v| v / 100.0)), m.mono),
            Adjust::SelectiveColor(s) => Stage::Selective(s.ranges, s.relative),
            Adjust::GradientMap(g) => {
                let mut stops = g.stops.clone();
                stops.sort_by(|a, b| a.at.total_cmp(&b.at));
                Stage::Gradient(stops, g.reverse)
            }
            Adjust::Threshold(t) => Stage::Threshold(t.level.clamp(1.0, 255.0)),
            Adjust::Look(l) => Stage::Look(l.look, (l.amount / 100.0).clamp(0.0, 1.0)),
            Adjust::Cube(c) => {
                if c.lut.is_empty() {
                    continue;
                }
                Stage::Cube(c.lut.clone(), (c.amount / 100.0).clamp(0.0, 1.0))
            }
        });
    }
    out
}

fn run(stage: &Stage, c: [f32; 3]) -> [f32; 3] {
    match stage {
        Stage::Levels(ch) => {
            let c: [f32; 3] = std::array::from_fn(|i| levels(&ch[i + 1], c[i]));
            c.map(|v| levels(&ch[0], v))
        }
        Stage::Tables(t) => {
            let c: [f32; 3] = std::array::from_fn(|i| t[i + 1].as_ref().map_or(c[i], |t| table(t, c[i])));
            match &t[0] {
                Some(t) => c.map(|v| table(t, v)),
                None => c,
            }
        }
        Stage::Exposure(m, off, gamma) => c.map(|v| {
            let l = v.powf(2.2) * m + off;
            let l = l.max(0.0).powf(1.0 / gamma);
            l.powf(1.0 / 2.2).clamp(0.0, 1.0)
        }),
        Stage::Vibrance(v, s) => {
            let mut l = c.map(srgb_to_linear);
            if *v != 0.0 {
                l = vibrance_lin(l, *v);
            }
            if *s != 0.0 {
                let g = 0.288 * l[0] + 0.712 * l[1];
                l = l.map(|x| (g + (1.0 + s) * (x - g)).clamp(0.0, 1.0));
            }
            l.map(|x| linear_to_srgb(x.clamp(0.0, 1.0)))
        }
        Stage::Balance(b, preserve) => {
            let out: [f32; 3] = std::array::from_fn(|i| b[i].eval(c[i]));
            if *preserve {
                set_lum(out, lum(c))
            } else {
                out
            }
        }
        Stage::BlackWhite(w, tint) => {
            let g = black_white_gray(c, w);
            match tint {
                Some(t) => set_lum(*t, g),
                None => [g; 3],
            }
        }
        Stage::Matrix(m, preserve) => {
            let f = mat3(m, c.map(srgb_to_linear)).map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
            if *preserve {
                set_lum(f, lum(c))
            } else {
                f
            }
        }
        Stage::Mixer(rows, mono) => {
            let mix = |r: &[f32; 4]| (r[0] * c[0] + r[1] * c[1] + r[2] * c[2] + r[3]).clamp(0.0, 1.0);
            if *mono {
                [mix(&rows[0]); 3]
            } else {
                [mix(&rows[0]), mix(&rows[1]), mix(&rows[2])]
            }
        }
        Stage::Selective(r, rel) => selective_color(c, *rel, r),
        Stage::Gradient(stops, rev) => {
            let t = lum(c);
            gradient(stops, if *rev { 1.0 - t } else { t })
        }
        Stage::Threshold(level) => [if lum(c) * 255.0 >= *level - 0.5 { 1.0 } else { 0.0 }; 3],
        Stage::Look(l, k) => mix3(c, look(*l, c), *k),
        Stage::Cube(lut, k) => mix3(c, lut.sample(c), *k),
        Stage::HueSat(hue, s, l, colorize) => hue_saturation(c, *hue, *s, *l, *colorize),
        Stage::Invert => c.map(|v| 1.0 - v),
        Stage::Posterize(n) => c.map(|v| {
            let x = (v * 255.0).round();
            let bin = (x * n / 256.0).floor().min(n - 1.0);
            (bin * 255.0 / (n - 1.0)).floor() / 255.0
        }),
    }
}

/// Every stage on one colour (0..1).
fn eval(stages: &[Stage], mut c: [f32; 3]) -> [f32; 3] {
    for s in stages {
        c = run(s, c).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    }
    c
}

/// One adjustment on its own, for drawing its curve in the panel.
pub fn map_one(item: &AdjustItem, c: [f32; 3]) -> [f32; 3] {
    eval(&compile(std::slice::from_ref(item)), c)
}

/// The lookup table for `items`, as a picture: slice `b` (blue) is tile
/// `(b % TILES, b / TILES)`, red across it and green down. Kept for the
/// last few lists asked for, as building one takes a few milliseconds.
pub fn lut(items: &[AdjustItem], mode: Blend) -> Option<Arc<RgbaImage>> {
    static CACHE: Mutex<Vec<(u64, Arc<RgbaImage>)>> = Mutex::new(Vec::new());
    let stages = compile(items);
    if stages.is_empty() {
        return None;
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", items.iter().filter(|i| i.on).collect::<Vec<_>>()).hash(&mut h);
        mode.hash(&mut h);
        h.finish()
    };
    if let Ok(c) = CACHE.lock() {
        if let Some((_, img)) = c.iter().find(|(k, _)| *k == key) {
            return Some(img.clone());
        }
    }
    let mut img = RgbaImage::new(LUT_SIDE, LUT_SIDE);
    let side = LUT_SIDE as usize;
    img.as_mut().par_chunks_mut(side * 4).enumerate().for_each(|(y, row)| {
        let (g, tile_y) = (y % N, y / N);
        let step = 1.0 / (N - 1) as f32;
        for x in 0..side {
            let (r, tile_x) = (x % N, x / N);
            let b = tile_y * TILES + tile_x;
            let px = &mut row[x * 4..x * 4 + 4];
            if b >= N {
                continue;
            }
            let src = [r as f32 * step, g as f32 * step, b as f32 * step];
            let c = mode.apply(src, eval(&stages, src));
            px[0] = (c[0] * 255.0).round() as u8;
            px[1] = (c[1] * 255.0).round() as u8;
            px[2] = (c[2] * 255.0).round() as u8;
            px[3] = 255;
        }
    });
    let img = Arc::new(img);
    if let Ok(mut c) = CACHE.lock() {
        c.insert(0, (key, img.clone()));
        c.truncate(6);
    }
    Some(img)
}

/// A colour (0..255 each) through the table, as the shader reads it.
#[inline]
pub fn lookup(lut: &RgbaImage, c: [f32; 3]) -> [f32; 3] {
    let raw = lut.as_raw();
    let side = LUT_SIDE as usize;
    let k = (N - 1) as f32 / 255.0;
    let f = c.map(|v| v * k);
    let i = f.map(|v| (v.floor() as usize).min(N - 2));
    let t: [f32; 3] = std::array::from_fn(|j| f[j] - i[j] as f32);
    let at = |r: usize, g: usize, b: usize| -> [f32; 3] {
        let x = r + (b % TILES) * N;
        let y = g + (b / TILES) * N;
        let o = (y * side + x) * 4;
        [raw[o] as f32, raw[o + 1] as f32, raw[o + 2] as f32]
    };
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t) };
    let c00 = lerp(at(i[0], i[1], i[2]), at(i[0] + 1, i[1], i[2]), t[0]);
    let c10 = lerp(at(i[0], i[1] + 1, i[2]), at(i[0] + 1, i[1] + 1, i[2]), t[0]);
    let c01 = lerp(at(i[0], i[1], i[2] + 1), at(i[0] + 1, i[1], i[2] + 1), t[0]);
    let c11 = lerp(at(i[0], i[1] + 1, i[2] + 1), at(i[0] + 1, i[1] + 1, i[2] + 1), t[0]);
    let lo = lerp(c00, c10, t[1]);
    let hi = lerp(c01, c11, t[1]);
    lerp(lo, hi, t[2])
}

/// Rust's round: halves away from zero, as the shader's `rnd`.
#[inline]
fn c8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// `items` on every pixel of `img`.
pub fn apply(img: &mut RgbaImage, items: &[AdjustItem], mode: Blend) {
    let Some(lut) = lut(items, mode) else { return };
    let w = img.width() as usize;
    img.as_mut().par_chunks_mut(w.max(1) * 4).for_each(|row| {
        for px in row.chunks_exact_mut(4) {
            if px[3] == 0 {
                continue;
            }
            let c = lookup(&lut, [px[0] as f32, px[1] as f32, px[2] as f32]);
            px[0] = c8(c[0]);
            px[1] = c8(c[1]);
            px[2] = c8(c[2]);
        }
    });
}

// ---------------------------------------------------------------- analysis

/// Pixel counts by level: red, green, blue and luminosity.
#[derive(Clone)]
pub struct Histogram {
    pub bins: [[u32; 256]; 4],
}

pub fn histogram(img: &RgbaImage) -> Histogram {
    let rows: Vec<[[u32; 256]; 4]> = img
        .as_raw()
        .par_chunks(img.width().max(1) as usize * 4)
        .map(|row| {
            let mut b = [[0u32; 256]; 4];
            for px in row.chunks_exact(4) {
                if px[3] == 0 {
                    continue;
                }
                b[0][px[0] as usize] += 1;
                b[1][px[1] as usize] += 1;
                b[2][px[2] as usize] += 1;
                let l = lum([px[0] as f32, px[1] as f32, px[2] as f32]).round().clamp(0.0, 255.0) as usize;
                b[3][l] += 1;
            }
            b
        })
        .collect();
    let mut bins = [[0u32; 256]; 4];
    for r in rows {
        for c in 0..4 {
            for i in 0..256 {
                bins[c][i] += r[c][i];
            }
        }
    }
    Histogram { bins }
}

impl Histogram {
    /// The levels below and above which `clip` (0..1) of the pixels lie.
    fn ends(&self, c: usize, clip: f64) -> (f32, f32) {
        let b = &self.bins[c];
        let total: u64 = b.iter().map(|v| *v as u64).sum();
        if total == 0 {
            return (0.0, 255.0);
        }
        let cut = (total as f64 * clip) as u64;
        let mut acc = 0u64;
        let lo = (0..256).find(|&i| {
            acc += b[i] as u64;
            acc > cut
        });
        acc = 0;
        let hi = (0..256).rev().find(|&i| {
            acc += b[i] as u64;
            acc > cut
        });
        let (lo, hi) = (lo.unwrap_or(0) as f32, hi.unwrap_or(255) as f32);
        if hi - lo < 2.0 {
            (0.0, 255.0)
        } else {
            (lo, hi)
        }
    }

    /// Auto Tone: each channel stretched to fill 0..255 (0.1% clipped).
    pub fn auto_levels(&self) -> Levels {
        let mut l = Levels::default();
        for c in 0..3 {
            let (lo, hi) = self.ends(c, 0.001);
            l.ch[c + 1].in_black = lo;
            l.ch[c + 1].in_white = hi;
        }
        l
    }

    /// Auto Contrast: all channels stretched together, keeping colours.
    pub fn auto_contrast(&self) -> Levels {
        let (lo, hi) = self.ends(3, 0.001);
        let mut l = Levels::default();
        l.ch[0].in_black = lo;
        l.ch[0].in_white = hi;
        l
    }

    /// The same as a curve: Auto in Curves.
    pub fn auto_curves(&self) -> Curves {
        let mut c = Curves::default();
        for ch in 0..3 {
            let (lo, hi) = self.ends(ch, 0.001);
            c.ch[ch + 1] = vec![[lo, 0.0], [hi, 255.0]];
        }
        c
    }
}

/// Curves presets: name and RGB points.
pub const CURVE_PRESETS: [(&str, &[[f32; 2]]); 7] = [
    ("Default", &[[0.0, 0.0], [255.0, 255.0]]),
    ("More contrast", &[[0.0, 0.0], [64.0, 50.0], [191.0, 205.0], [255.0, 255.0]]),
    ("Strong contrast", &[[0.0, 0.0], [64.0, 38.0], [191.0, 217.0], [255.0, 255.0]]),
    ("Lighter", &[[0.0, 0.0], [128.0, 155.0], [255.0, 255.0]]),
    ("Darker", &[[0.0, 0.0], [128.0, 100.0], [255.0, 255.0]]),
    ("Faded", &[[0.0, 32.0], [128.0, 128.0], [255.0, 236.0]]),
    ("Negative", &[[0.0, 255.0], [255.0, 0.0]]),
];

/// Photo Filter presets.
pub const FILTERS: [(&str, [u8; 3]); 8] = [
    ("Warming (85)", [236, 138, 0]),
    ("Warming (81)", [235, 177, 19]),
    ("Cooling (80)", [0, 109, 255]),
    ("Cooling (82)", [0, 181, 255]),
    ("Red", [234, 26, 26]),
    ("Green", [25, 201, 25]),
    ("Sepia", [172, 122, 51]),
    ("Underwater", [0, 194, 177]),
];

/// Black & White presets.
pub const BW_PRESETS: [(&str, [f32; 6]); 6] = [
    ("Default", BW_DEFAULT),
    ("Red filter", [120.0, 110.0, -10.0, -50.0, -50.0, 120.0]),
    ("Green filter", [-40.0, 120.0, 120.0, -50.0, -50.0, -40.0]),
    ("Blue filter", [-50.0, -50.0, -10.0, 120.0, 110.0, 120.0]),
    ("High contrast", [150.0, 130.0, -20.0, -60.0, -60.0, 150.0]),
    ("Infrared", [-40.0, 235.0, 144.0, -68.0, -3.0, -107.0]),
];

/// What an Adjust layer is called: its adjustment, or how many.
pub fn layer_name(items: &[AdjustItem]) -> String {
    match items {
        [] => "Adjustments".into(),
        [one] => one.adj.name().into(),
        [a, b] => format!("{} + {}", a.adj.name(), b.adj.name()),
        [a, rest @ ..] => format!("{} + {} more", a.adj.name(), rest.len()),
    }
}

/// One of every adjustment, each set to change something: for tests.
#[cfg(test)]
pub fn sample_list() -> Vec<AdjustItem> {
    let mut l = Levels::default();
    (l.ch[0].in_black, l.ch[0].in_white, l.ch[0].gamma) = (20.0, 230.0, 1.4);
    l.ch[3].out_black = 30.0;
    let mut c = Curves::default();
    c.ch[0] = vec![[0.0, 0.0], [64.0, 40.0], [190.0, 215.0], [255.0, 255.0]];
    c.ch[1] = vec![[0.0, 20.0], [255.0, 240.0]];
    let mut sc = SelectiveColor::default();
    sc.ranges[0] = [30.0, -20.0, 10.0, 5.0];
    sc.ranges[7] = [0.0, 10.0, -15.0, 0.0];
    let cube = {
        let n = 5;
        let mut d = Vec::new();
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    d.extend([(g * 255 / (n - 1)) as u8, (b * 255 / (n - 1)) as u8, (r * 255 / (n - 1)) as u8]);
                }
            }
        }
        CubeData::new(n, d)
    };
    [
        Adjust::Levels(l),
        Adjust::Curves(c),
        Adjust::Exposure(Exposure { exposure: 0.6, offset: 0.02, gamma: 1.1 }),
        Adjust::Vibrance(Vibrance { vibrance: 40.0, saturation: -10.0 }),
        Adjust::ColorBalance(ColorBalance { tones: [[10.0, 0.0, -20.0], [-15.0, 5.0, 25.0], [0.0, -10.0, 10.0]], preserve: true }),
        Adjust::BlackWhite(BlackWhite { tint: true, ..Default::default() }),
        Adjust::PhotoFilter(PhotoFilter::default()),
        Adjust::ChannelMixer(ChannelMixer { rows: [[80.0, 30.0, -10.0, 0.0], [0.0, 100.0, 0.0, 5.0], [20.0, 0.0, 90.0, 0.0]], mono: false }),
        Adjust::SelectiveColor(sc),
        Adjust::GradientMap(GradientMap { stops: GRADIENTS[3].1.to_vec(), reverse: false }),
        Adjust::Threshold(Threshold { level: 110.0 }),
        Adjust::Look(LookAdj { look: Look::BleachBypass, amount: 70.0 }),
        Adjust::Cube(Cube { name: "swap".into(), lut: cube, amount: 100.0 }),
        Adjust::HueSat(HueSat { hue: 40.0, saturation: 25.0, lightness: -10.0, colorize: false }),
        Adjust::BrightContrast(BrightContrast { brightness: 30.0, contrast: 20.0, legacy: false }),
        Adjust::Invert(Invert {}),
        Adjust::Posterize(Posterize { levels: 6 }),
        Adjust::Look(LookAdj { look: Look::Gotham, amount: 80.0 }),
    ]
    .into_iter()
    .map(AdjustItem::new)
    .collect()
}

fn rgb_to_hsl(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-7 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    (hsv_hue(c, max, min) / 360.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// Photoshop's Hue/Saturation (PhotoCraft's measurements): lightness first,
/// then the hue turned and the saturation scaled; colorize paints one hue.
fn hue_saturation(c: [f32; 3], hue: f32, s: f32, l: f32, colorize: bool) -> [f32; 3] {
    let lighten = |v: f32| if l > 0.0 { v + (1.0 - v) * l } else { v * (1.0 + l) };
    if colorize {
        let (_, _, ll) = rgb_to_hsl(c);
        let sat = if s == 0.0 { 0.25 } else { (s.abs()).min(1.0) };
        return hsl_to_rgb(hue.rem_euclid(360.0) / 360.0, sat, lighten(ll));
    }
    let lc = c.map(lighten);
    let (hh, ss, ll) = rgb_to_hsl(lc);
    let hh = (hh + hue / 360.0).rem_euclid(1.0);
    let ss = if s > 0.0 { ss / (1.0 - s).max(1.0 / 512.0) } else { ss * (1.0 + s) }.clamp(0.0, 1.0);
    hsl_to_rgb(hh, ss, ll)
}

/// Brightness/Contrast as one channel's table.
fn bc_table(b: &BrightContrast) -> Vec<f32> {
    let n = 1024;
    if b.legacy {
        // GIFkit's older Effects sliders, the classic straight lines
        let c = b.contrast.clamp(-100.0, 100.0) * 2.55;
        let cf = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
        let br = b.brightness * 2.55;
        return (0..n).map(|k| (((k as f32 / (n - 1) as f32 * 255.0 - 128.0) * cf + 128.0 + br) / 255.0).clamp(0.0, 1.0)).collect();
    }
    let step = |k: usize, n: usize| k as f32 / (n - 1) as f32;
    let br = b.brightness.clamp(-150.0, 150.0);
    let bright: Vec<f32> = if br == 0.0 {
        (0..n).map(|k| step(k, n)).collect()
    } else if br > 0.0 {
        let c = BrightCurve::new(br);
        (0..n).map(|k| c.raised(step(k, n))).collect()
    } else {
        // lowered: the raised curve turned around
        let c = BrightCurve::new(-br);
        let fine = 4 * n;
        let ys: Vec<f32> = (0..fine).map(|j| c.raised(step(j, fine))).collect();
        let mut j = 0;
        (0..n)
            .map(|k| {
                let y = step(k, n);
                while j + 2 < fine && ys[j + 1] < y {
                    j += 1;
                }
                let (y0, y1) = (ys[j], ys[j + 1]);
                let f = if y1 > y0 { ((y - y0) / (y1 - y0)).clamp(0.0, 1.0) } else { 0.0 };
                (step(j, fine) + f / (fine - 1) as f32).clamp(0.0, 1.0)
            })
            .collect()
    };
    let ct = b.contrast.clamp(-50.0, 100.0);
    bright.into_iter().map(|v| modern_contrast(v, ct).clamp(0.0, 1.0)).collect()
}

/// Photoshop's modern Brightness curve (PhotoCraft's fit): a line to a
/// knee, then a roll-off that pins white.
struct BrightCurve {
    s: f32,
    e: f32,
    x0: f32,
    alpha: f32,
    hermite: bool,
}

impl BrightCurve {
    fn new(b: f32) -> Self {
        let b = b.abs();
        let s = 1.375f32.powf(b / 50.0);
        let e = 1.0 / (1.0 + 0.084 * b);
        let knee = 126.0 / 255.0;
        let x0 = (knee / s).min(1.0);
        let secant = (1.0 - knee) / (1.0 - x0).max(1e-6);
        let (alpha, beta) = (s / secant, e / secant);
        BrightCurve { s, e, x0, alpha, hermite: alpha * alpha + beta * beta <= 9.0 }
    }

    fn raised(&self, x: f32) -> f32 {
        let y0: f32 = 126.0 / 255.0;
        if x <= self.x0 {
            return y0.min(self.s * x);
        }
        let h = (1.0 - self.x0).max(1e-6);
        let t = ((x - self.x0) / h).clamp(0.0, 1.0);
        let y = if self.hermite {
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * self.s + (-2.0 * t3 + 3.0 * t2) + (t3 - t2) * h * self.e
        } else {
            1.0 - (1.0 - y0) * (1.0 - t).powf(self.alpha)
        };
        y.clamp(0.0, 1.0)
    }
}

/// Photoshop's modern Contrast: an S around the middle that pins black and white.
fn modern_contrast(v: f32, contrast: f32) -> f32 {
    if contrast == 0.0 {
        return v;
    }
    let k = contrast / 128.0;
    let (end, mid) = (1.0 - k, 1.0 + k);
    let half = |t: f32| end * 0.5 * (t * t * t - 2.0 * t * t + t) + (-2.0 * t * t * t + 3.0 * t * t) * 0.5 + mid * 0.5 * (t * t * t - t * t);
    let v = v.clamp(0.0, 1.0);
    if v <= 0.5 {
        half(v / 0.5)
    } else {
        1.0 - half((1.0 - v) / 0.5)
    }
}

/// Gotham and Nashville from the older Effects filters, run once over every
/// colour of the table's grid so they come out the same.
fn legacy_cube(l: Look) -> CubeData {
    static CACHE: Mutex<Vec<(Look, CubeData)>> = Mutex::new(Vec::new());
    if let Ok(c) = CACHE.lock() {
        if let Some((_, d)) = c.iter().find(|(k, _)| *k == l) {
            return d.clone();
        }
    }
    // as fine as the layer's own table, so nothing is read between points twice
    let n = N;
    let mut img = RgbaImage::new((n * n) as u32, n as u32);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let v = |i: usize| ((i * 255) as f32 / (n - 1) as f32).round() as u8;
                img.put_pixel((r + b * n) as u32, g as u32, image::Rgba([v(r), v(g), v(b), 255]));
            }
        }
    }
    match l {
        Look::Gotham => crate::effects::gotham(&mut img),
        _ => crate::effects::nashville(&mut img),
    }
    let mut data = Vec::with_capacity(n * n * n * 3);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                data.extend_from_slice(&img.get_pixel((r + b * n) as u32, g as u32).0[..3]);
            }
        }
    }
    let d = CubeData::new(n, data);
    if let Ok(mut c) = CACHE.lock() {
        c.push((l, d.clone()));
    }
    d
}

/// The older Effects colour settings as adjustments that look the same (or
/// close: Hue/Saturation's lightness and Posterize's steps differ a little).
pub fn from_legacy(fx: &crate::effects::EffectSettings) -> Vec<AdjustItem> {
    let mut out = Vec::new();
    let mut push = |a: Adjust| out.push(AdjustItem::new(a));
    if fx.hue != 100.0 || fx.saturation != 100.0 || fx.lightness != 100.0 {
        let sm = fx.saturation / 100.0;
        let lm = fx.lightness / 100.0;
        push(Adjust::HueSat(HueSat {
            hue: ((fx.hue - 100.0) * 1.8 + 180.0).rem_euclid(360.0) - 180.0,
            saturation: (if sm <= 1.0 { sm - 1.0 } else { 1.0 - 1.0 / sm }) * 100.0,
            lightness: ((lm - 1.0) * 100.0).clamp(-100.0, 100.0),
            colorize: false,
        }));
    }
    if fx.brightness != 0.0 || fx.contrast != 0.0 {
        push(Adjust::BrightContrast(BrightContrast { brightness: fx.brightness, contrast: fx.contrast, legacy: true }));
    }
    let grey = [29.9, 58.7, 11.4];
    if fx.grayscale {
        push(Adjust::ChannelMixer(ChannelMixer { rows: [[grey[0], grey[1], grey[2], 0.0]; 3], mono: true }));
    }
    if fx.sepia {
        push(Adjust::ChannelMixer(ChannelMixer { rows: [[39.3, 76.9, 18.9, 0.0], [34.9, 68.6, 16.8, 0.0], [27.2, 53.4, 13.1, 0.0]], mono: false }));
    }
    if fx.monochrome {
        push(Adjust::Threshold(Threshold { level: 128.0 }));
    }
    if fx.negative {
        push(Adjust::Invert(Invert {}));
    }
    if fx.tint {
        // the colour times the grey, mixed in by the amount: a straight mix
        let a = (fx.tint_amount / 100.0).clamp(0.0, 1.0);
        let rows: [[f32; 4]; 3] = std::array::from_fn(|ch| {
            let t = fx.tint_color[ch] as f32 / 255.0 * a;
            let mut r = [grey[0] * t, grey[1] * t, grey[2] * t, 0.0];
            r[ch] += (1.0 - a) * 100.0;
            r
        });
        push(Adjust::ChannelMixer(ChannelMixer { rows, mono: false }));
    }
    if fx.posterize {
        push(Adjust::Posterize(Posterize { levels: fx.posterize_levels.max(2) as u32 }));
    }
    for (on, look, amt) in [(fx.gotham, Look::Gotham, fx.gotham_amount), (fx.nashville, Look::Nashville, fx.nashville_amount)] {
        if on {
            push(Adjust::Look(LookAdj { look, amount: amt.clamp(0.0, 100.0) }));
        }
    }
    out
}

/// Those settings cleared, what `from_legacy` took.
pub fn clear_legacy(fx: &mut crate::effects::EffectSettings) {
    let d = crate::effects::EffectSettings::default();
    (fx.hue, fx.saturation, fx.lightness, fx.brightness, fx.contrast) = (d.hue, d.saturation, d.lightness, d.brightness, d.contrast);
    (fx.grayscale, fx.sepia, fx.monochrome, fx.negative, fx.tint, fx.posterize, fx.gotham, fx.nashville) = (false, false, false, false, false, false, false, false);
    // and the numbers that went with them, so an emptied layer is empty
    (fx.tint_color, fx.tint_amount, fx.posterize_levels, fx.gotham_amount, fx.nashville_amount) = (d.tint_color, d.tint_amount, d.posterize_levels, d.gotham_amount, d.nashville_amount);
}

/// Whether a layer still has older colour settings on.
pub fn has_legacy(fx: &crate::effects::EffectSettings) -> bool {
    let mut c = fx.clone();
    clear_legacy(&mut c);
    c != *fx
}

impl Histogram {
    /// Mean, standard deviation, median and pixel count of channel `c`.
    pub fn stats(&self, c: usize) -> (f64, f64, usize, u64) {
        let b = &self.bins[c];
        let n: u64 = b.iter().map(|v| *v as u64).sum();
        if n == 0 {
            return (0.0, 0.0, 0, 0);
        }
        let mean = b.iter().enumerate().map(|(i, v)| i as f64 * *v as f64).sum::<f64>() / n as f64;
        let var = b.iter().enumerate().map(|(i, v)| (i as f64 - mean).powi(2) * *v as f64).sum::<f64>() / n as f64;
        let mut acc = 0u64;
        let median = (0..256).find(|&i| {
            acc += b[i] as u64;
            acc * 2 >= n
        });
        (mean, var.sqrt(), median.unwrap_or(0), n)
    }
}

impl Histogram {
    /// Auto Color: each channel stretched, then the middles bent so the
    /// channels' averages meet (neutral midtones stay neutral).
    pub fn auto_color(&self) -> Levels {
        let mut l = self.auto_levels();
        let means: Vec<f32> = (0..3)
            .map(|c| {
                let (m, ..) = self.stats(c);
                levels(&l.ch[c + 1], m as f32 / 255.0)
            })
            .collect();
        let target = (means[0] + means[1] + means[2]) / 3.0;
        for c in 0..3 {
            let (m, ..) = self.stats(c);
            let ch = &mut l.ch[c + 1];
            let u = ((m as f32 - ch.in_black) / (ch.in_white - ch.in_black).max(1.0)).clamp(0.02, 0.98);
            ch.gamma = (u.ln() / target.clamp(0.02, 0.98).ln()).clamp(0.4, 2.5);
        }
        l
    }
}
