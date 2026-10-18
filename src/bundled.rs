//! Fonts that ship with GIFkit, so there's a good choice of styles and
//! scripts on any system. All are openly licensed (OFL, Apache 2.0 or the
//! Ubuntu Font Licence, see assets/FONTS_LICENSE.txt). Built by
//! tools/build_fonts.py from github.com/google/fonts.

use crate::text::FontEntry;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

static ZIP: &[u8] = include_bytes!("../assets/fonts/fonts.zip");
static INDEX: &str = include_str!("../assets/fonts/fonts.json");

#[derive(serde::Deserialize)]
pub struct Face {
    pub style: String,
    pub file: String,
    /// variable font axis values for this style
    pub coords: Vec<(String, f32)>,
}

#[derive(serde::Deserialize)]
pub struct Family {
    pub family: String,
    /// the script the font is mainly for ("Latin & Cyrillic", "Arabic", …)
    pub group: String,
    pub faces: Vec<Face>,
}

impl Family {
    pub fn default_face(&self) -> &Face {
        self.faces.iter().find(|f| f.style == "Regular").unwrap_or(&self.faces[0])
    }
}

pub fn families() -> &'static [Family] {
    static ALL: OnceLock<Vec<Family>> = OnceLock::new();
    ALL.get_or_init(|| serde_json::from_str(INDEX).unwrap_or_default())
}

pub fn family(name: &str) -> Option<&'static Family> {
    families().iter().find(|f| f.family == name)
}

/// The font file (unpacked once, then shared).
pub fn bytes(file: &str) -> Option<Arc<Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(b) = cache.lock().ok()?.get(file) {
        return Some(b.clone());
    }
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(ZIP)).ok()?;
    let mut f = z.by_name(file).ok()?;
    let mut data = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut data).ok()?;
    let data = Arc::new(data);
    cache.lock().ok()?.insert(file.to_string(), data.clone());
    Some(data)
}

/// Load one style of a bundled family (the regular style if `style` is unknown).
pub fn load(name: &str, style: &str) -> Option<(FontEntry, String)> {
    let fam = family(name)?;
    let face = fam.faces.iter().find(|f| f.style == style).unwrap_or_else(|| fam.default_face());
    let data = bytes(&face.file)?;
    let coords = face
        .coords
        .iter()
        .filter_map(|(t, v)| {
            let b = t.as_bytes();
            (b.len() == 4).then(|| ([b[0], b[1], b[2], b[3]], *v))
        })
        .collect();
    let e = FontEntry::from_data(format!("{name} {}", face.style), (*data).clone(), 0, coords).ok()?;
    Some((e, face.style.clone()))
}
