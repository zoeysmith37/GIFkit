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

/// Folder presets are stored in.
pub fn folder() -> Option<PathBuf> {
    dir()
}

/// All saved presets, plus a note for each file (or part of one) that could
/// not be read. Parsing is lenient: an unreadable layer or section is skipped
/// rather than hiding the whole preset.
pub fn list_with_problems() -> (Vec<Preset>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let Some(d) = dir() else { return (out, vec!["No config folder (HOME / APPDATA not set)".into()]) };
    let Ok(rd) = std::fs::read_dir(&d) else { return (out, problems) };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let fname = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                problems.push(format!("{fname}: {err}"));
                continue;
            }
        };
        match parse_lenient(&text) {
            Ok((mut p, skipped)) => {
                if p.name.trim().is_empty() {
                    p.name = fname.trim_end_matches(".json").to_string();
                }
                for s in skipped {
                    problems.push(format!("{}: skipped {s}", p.name));
                }
                out.push(p);
            }
            Err(err) => problems.push(format!("{fname}: {err}")),
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    (out, problems)
}

fn parse_lenient(text: &str) -> anyhow::Result<(Preset, Vec<String>)> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(text)?;
    let obj = v.as_object().ok_or_else(|| anyhow::anyhow!("not a preset"))?;
    let mut skipped = Vec::new();
    let mut p = Preset { name: obj.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(), ..Default::default() };
    if let Some(x) = obj.get("src_size") {
        p.src_size = serde_json::from_value(x.clone()).unwrap_or_default();
    }
    if let Some(x) = obj.get("transform").filter(|x| !x.is_null()) {
        match serde_json::from_value(x.clone()) {
            Ok(t) => p.transform = Some(t),
            Err(e) => skipped.push(format!("frame edits ({e})")),
        }
    }
    if let Some(x) = obj.get("gif").filter(|x| !x.is_null()) {
        match serde_json::from_value(x.clone()) {
            Ok(g) => p.gif = Some(g),
            Err(e) => skipped.push(format!("compression ({e})")),
        }
    }
    if let Some(Value::Array(ls)) = obj.get("layers") {
        let mut layers = Vec::new();
        for (i, l) in ls.iter().enumerate() {
            match serde_json::from_value::<PresetLayer>(l.clone()) {
                Ok(l) => layers.push(l),
                Err(e) => skipped.push(format!("layer {} ({e})", i + 1)),
            }
        }
        p.layers = Some(layers);
    }
    Ok((p, skipped))
}

pub fn save(p: &Preset) -> anyhow::Result<()> {
    let d = dir().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
    std::fs::create_dir_all(&d)?;
    let path = d.join(file_name(&p.name));
    let text = serde_json::to_string_pretty(p)?;
    // write-then-rename so a crash never leaves a half-written preset
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, &path)?;
    // read it back, so a preset that can't be loaded is reported now, not next session
    let (_, skipped) = parse_lenient(&text)?;
    anyhow::ensure!(skipped.is_empty(), "saved, but parts can't be read back: {}", skipped.join("; "));
    Ok(())
}

pub fn delete(name: &str) {
    if let Some(d) = dir() {
        let _ = std::fs::remove_file(d.join(file_name(name)));
    }
}
