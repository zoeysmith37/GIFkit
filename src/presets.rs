//! Saved presets: frame edits, layers and compression settings that can be
//! re-applied to other animations. Stored as JSON in the config directory.

use crate::effects::{CensorSettings, EffectSettings, ShieldSettings, WatermarkSettings};
use crate::encode::GifOptions;
use crate::overlay::Placement;
use crate::stack::Transform;
use crate::text::Annotation;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PresetLayer {
    Effects { visible: bool, fx: EffectSettings },
    Text { visible: bool, text: Annotation, font: Option<(String, String)> },
    Censor { visible: bool, censor: CensorSettings },
    Image { visible: bool, path: PathBuf, placement: Placement, opacity: f32 },
    Watermark { visible: bool, watermark: WatermarkSettings },
    Shield { visible: bool, shield: ShieldSettings },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    /// frame size the preset was made on (positions are scaled to fit)
    pub src_size: (u32, u32),
    pub transform: Option<Transform>,
    pub layers: Option<Vec<PresetLayer>>,
    pub gif: Option<GifOptions>,
}

impl Preset {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.transform.is_some() {
            parts.push("frame edits".to_string());
        }
        if let Some(l) = &self.layers {
            parts.push(format!("{} layer{}", l.len(), if l.len() == 1 { "" } else { "s" }));
        }
        if self.gif.is_some() {
            parts.push("compression".to_string());
        }
        parts.join(" · ")
    }
}

fn dir() -> Option<PathBuf> {
    Some(crate::theme::config_path()?.parent()?.join("presets"))
}

fn file_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    format!("{}.json", safe.trim())
}

pub fn list() -> Vec<Preset> {
    let Some(d) = dir() else { return Vec::new() };
    let mut out: Vec<Preset> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .filter_map(|e| serde_json::from_str::<Preset>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn save(p: &Preset) -> anyhow::Result<()> {
    let d = dir().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
    std::fs::create_dir_all(&d)?;
    std::fs::write(d.join(file_name(&p.name)), serde_json::to_string_pretty(p)?)?;
    Ok(())
}

pub fn delete(name: &str) {
    if let Some(d) = dir() {
        let _ = std::fs::remove_file(d.join(file_name(name)));
    }
}
