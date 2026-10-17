//! System font discovery (fontconfig dirs on Linux, the OS font folders on
//! Windows / macOS) via `fontdb`, grouped into families and styles.

use ab_glyph::{FontArc, FontVec};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SysFace {
    pub style: String,
    pub path: PathBuf,
    pub index: u32,
    pub weight: u16,
    pub italic: bool,
}

#[derive(Clone, Debug)]
pub struct SysFamily {
    pub name: String,
    pub faces: Vec<SysFace>,
}

impl SysFamily {
    /// The "Regular" face, or the one closest to it.
    pub fn default_face(&self) -> &SysFace {
        self.faces.iter().min_by_key(|f| (f.italic as u16, (f.weight as i32 - 400).unsigned_abs())).unwrap_or(&self.faces[0])
    }
}

fn weight_name(w: u16) -> &'static str {
    match w {
        0..=149 => "Thin",
        150..=249 => "ExtraLight",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "SemiBold",
        650..=749 => "Bold",
        750..=849 => "ExtraBold",
        _ => "Black",
    }
}

/// Scan the system for fonts. Takes a moment; call from a background thread.
pub fn scan() -> Vec<SysFamily> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut fams: std::collections::BTreeMap<String, SysFamily> = std::collections::BTreeMap::new();
    for f in db.faces() {
        let path = match &f.source {
            fontdb::Source::File(p) => p.clone(),
            fontdb::Source::SharedFile(p, _) => p.clone(),
            _ => continue,
        };
        let Some((name, _)) = f.families.first() else { continue };
        let italic = !matches!(f.style, fontdb::Style::Normal);
        let mut style = weight_name(f.weight.0).to_string();
        if italic {
            style = if style == "Regular" { "Italic".into() } else { format!("{style} Italic") };
        }
        let fam = fams.entry(name.to_lowercase()).or_insert_with(|| SysFamily { name: name.clone(), faces: Vec::new() });
        if !fam.faces.iter().any(|x| x.style == style) {
            fam.faces.push(SysFace { style, path, index: f.index, weight: f.weight.0, italic });
        }
    }
    let mut out: Vec<SysFamily> = fams.into_values().collect();
    for f in &mut out {
        f.faces.sort_by_key(|x| (x.italic, x.weight));
    }
    out
}

pub fn load(face: &SysFace) -> anyhow::Result<FontArc> {
    let data = std::fs::read(&face.path)?;
    let fv = FontVec::try_from_vec_and_index(data, face.index).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(FontArc::new(fv))
}
