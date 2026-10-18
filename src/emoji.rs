//! Emoji: the full Twemoji set (CC-BY 4.0) as SVGs, with names, keywords and
//! groups from emojibase (MIT). Built by tools/build_emoji.py.
//! Rendered with resvg at whatever size they are placed, so they stay crisp.

use image::RgbaImage;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

static ZIP: &[u8] = include_bytes!("../assets/emoji/twemoji.zip");
static INDEX: &str = include_str!("../assets/emoji/emoji.json");

pub const GROUPS: [&str; 9] = ["Smileys & emotion", "People & body", "Animals & nature", "Food & drink", "Travel & places", "Activities", "Objects", "Symbols", "Flags"];

/// Short labels for the category tabs.
pub const GROUP_SHORT: [&str; 9] = ["Smileys", "People", "Nature", "Food", "Travel", "Activities", "Objects", "Symbols", "Flags"];

pub const TONES: [&str; 6] = ["Default", "Light", "Medium-light", "Medium", "Medium-dark", "Dark"];

#[derive(serde::Deserialize)]
pub struct Emoji {
    /// the character(s)
    #[allow(dead_code)]
    pub e: String,
    /// Twemoji file name (hex code points)
    pub f: String,
    /// group index into GROUPS
    pub g: u8,
    /// name
    pub n: String,
    /// search keywords, space separated
    pub k: String,
    /// skin tone versions 1..=5: (character, file)
    pub s: Vec<(String, String)>,
}

impl Emoji {
    /// File for a skin tone (0 = default), falling back to the default.
    pub fn file(&self, tone: usize) -> &str {
        if tone > 0 {
            if let Some((_, f)) = self.s.get(tone - 1) {
                return f;
            }
        }
        &self.f
    }
}

pub fn all() -> &'static [Emoji] {
    static ALL: OnceLock<Vec<Emoji>> = OnceLock::new();
    ALL.get_or_init(|| serde_json::from_str(INDEX).unwrap_or_default())
}

/// Emoji (and tone) for a file name, including skin-tone files.
pub fn by_file(f: &str) -> Option<(&'static Emoji, usize)> {
    static MAP: OnceLock<HashMap<&'static str, (usize, usize)>> = OnceLock::new();
    let m = MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, e) in all().iter().enumerate() {
            m.insert(e.f.as_str(), (i, 0));
            for (t, (_, sf)) in e.s.iter().enumerate() {
                m.insert(sf.as_str(), (i, t + 1));
            }
        }
        m
    });
    m.get(f).map(|&(i, t)| (&all()[i], t))
}

/// Name for display ("fire", "waving hand: dark skin tone").
pub fn name_of(f: &str) -> String {
    match by_file(f) {
        Some((e, 0)) => e.n.clone(),
        Some((e, t)) => format!("{}: {} skin tone", e.n, TONES[t].to_lowercase()),
        None => "emoji".into(),
    }
}

/// Indices of emoji matching every word of `query` (name or keywords),
/// optionally within one group. Best matches first: the exact name, then
/// names starting with the query, other name matches, then keyword matches.
pub fn search(query: &str, group: Option<u8>) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(u8, usize)> = Vec::new();
    for (i, e) in all().iter().enumerate() {
        if group.is_some_and(|g| e.g != g) {
            continue;
        }
        if words.is_empty() {
            hits.push((0, i));
            continue;
        }
        let name = e.n.to_lowercase();
        let in_name = |w: &str| name.split(|c: char| !c.is_alphanumeric()).any(|nw| nw.starts_with(w));
        if !words.iter().all(|w| in_name(w) || name.contains(w) || e.k.split(' ').any(|k| k.starts_with(w))) {
            continue;
        }
        let score = if name == q {
            0
        } else if name.starts_with(&q) {
            1
        } else if words.iter().all(|w| in_name(w)) {
            2
        } else if words.iter().all(|w| name.contains(w)) {
            3
        } else {
            4
        };
        hits.push((score, i));
    }
    hits.sort_by_key(|h| h.0);
    hits.into_iter().map(|h| h.1).collect()
}

fn archive() -> &'static Mutex<zip::ZipArchive<std::io::Cursor<&'static [u8]>>> {
    static A: OnceLock<Mutex<zip::ZipArchive<std::io::Cursor<&'static [u8]>>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(zip::ZipArchive::new(std::io::Cursor::new(ZIP)).expect("bundled emoji archive")))
}

/// Parsed SVG of an emoji file (cached).
pub fn tree(f: &str) -> Option<Arc<resvg::usvg::Tree>> {
    static TREES: OnceLock<Mutex<HashMap<String, Arc<resvg::usvg::Tree>>>> = OnceLock::new();
    let cache = TREES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(t) = cache.lock().ok()?.get(f) {
        return Some(t.clone());
    }
    let mut data = Vec::new();
    {
        let mut a = archive().lock().ok()?;
        let mut file = a.by_name(&format!("{f}.svg")).ok()?;
        file.read_to_end(&mut data).ok()?;
    }
    let t = Arc::new(resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?);
    cache.lock().ok()?.insert(f.to_string(), t.clone());
    Some(t)
}

/// Draw an emoji into `pm` filling a `w`×`h` box that `tf` places.
pub fn draw(f: &str, pm: &mut resvg::tiny_skia::PixmapMut, w: f32, h: f32, tf: resvg::tiny_skia::Transform) -> Option<()> {
    let t = tree(f)?;
    let s = t.size();
    let tf = tf.pre_scale(w / s.width(), h / s.height());
    resvg::render(&t, tf, pm);
    Some(())
}

/// The emoji as an image of `size`×`size` pixels.
pub fn render(f: &str, size: u32) -> Option<RgbaImage> {
    let mut pm = resvg::tiny_skia::Pixmap::new(size.max(1), size.max(1))?;
    draw(f, &mut pm.as_mut(), size as f32, size as f32, resvg::tiny_skia::Transform::identity())?;
    let mut out = RgbaImage::new(pm.width(), pm.height());
    for (d, p) in out.pixels_mut().zip(pm.pixels()) {
        let c = p.demultiply();
        *d = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(out)
}
