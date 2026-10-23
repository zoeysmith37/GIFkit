//! Project files (`.gifkit`): a zip with `project.json` (frame edits, layers
//! and output settings) plus every image the project needs as PNG, so it
//! opens the same on any machine, even without the original files.
//!
//!   project.json
//!   frames/00000.png …        the frames being edited
//!   original/00000.png …      the opened file, when it differs from the frames
//!   overlays/<n>/00000.png …  image layer frames
//!   watermarks/<n>.png        watermark images

use crate::anim::{Animation, Frame};
use crate::effects::{CensorSettings, EffectSettings, ShieldSettings, WatermarkSettings};
use crate::overlay::Placement;
use crate::shapes::{EmojiSettings, MarkupSettings, ShapeSettings};
use crate::stack::Transform;
use crate::text::Annotation;
use anyhow::{Context, Result};
use image::RgbaImage;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const EXT: &str = "gifkit";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FrameMeta {
    pub delay: u16,
    pub enabled: bool,
    /// a GIF track clip starts here
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    /// a gap on the GIF track (shows nothing)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blank: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum ProjLayer {
    Effects { fx: EffectSettings },
    Text { text: Annotation, font: Option<(String, String)> },
    Censor { censor: CensorSettings },
    /// frames are stored under overlays/<id>/
    Image { id: usize, name: String, path: Option<PathBuf>, delays: Vec<u16>, placements: Vec<Placement>, opacity: f32 },
    /// the image, if any, is stored as watermarks/<id>.png
    Watermark { id: usize, watermark: WatermarkSettings, has_image: bool },
    Shield { shield: ShieldSettings },
    Shape { shape: ShapeSettings },
    Markup { markup: MarkupSettings },
    Emoji { emoji: EmojiSettings },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjLayerEntry {
    pub visible: bool,
    #[serde(default)]
    pub label: Option<String>,
    pub layer: ProjLayer,
    /// clips and keyframes (none in older projects)
    #[serde(default, skip_serializing_if = "crate::timeline::Timing::is_default")]
    pub timing: crate::timeline::Timing,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// the bar's colour on the timeline
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// the layer's opacity, %
    #[serde(default = "full", skip_serializing_if = "is_full")]
    pub opacity: f32,
    /// clipped to the layer below
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clip: bool,
}

fn full() -> f32 {
    100.0
}

fn is_full(v: &f32) -> bool {
    *v >= 100.0
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProjectFile {
    pub version: u32,
    /// GIFkit version that wrote it
    pub app: String,
    /// the file the project was started from
    pub source_name: String,
    pub source_path: Option<PathBuf>,
    pub source_size: Option<u64>,
    pub loop_count: Option<u16>,
    pub frames: Vec<FrameMeta>,
    /// the original file's frames are stored separately (they differ from `frames`)
    pub original: Option<(Vec<FrameMeta>, Option<u16>)>,
    pub transform: Transform,
    pub layers: Vec<ProjLayerEntry>,
    /// timeline markers
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<crate::timeline::Marker>,
    /// the GIF track's colour on the timeline
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_color: Option<[u8; 3]>,
    /// output and tool settings, kept loose so new fields never break old files
    pub settings: serde_json::Value,
}

/// Everything in a project, decoded.
pub struct Project {
    pub file: ProjectFile,
    pub base: Animation,
    pub original: Option<Animation>,
    pub overlays: HashMap<usize, Vec<RgbaImage>>,
    pub watermarks: HashMap<usize, RgbaImage>,
}

fn png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // fast: projects are saved often and the zip is never shared as an image
        enc.set_compression(png::Compression::Fast);
        enc.write_header()?.write_image_data(img.as_raw())?;
    }
    Ok(out)
}

fn meta(a: &Animation) -> Vec<FrameMeta> {
    a.frames.iter().map(|f| FrameMeta { delay: f.delay_cs, enabled: f.enabled, cut: f.cut, blank: f.blank }).collect()
}

/// Write a project. `images` are the frames of image layers and watermark
/// images, keyed by the ids used in `file.layers`.
pub fn save(path: &Path, mut file: ProjectFile, base: &Animation, original: Option<&Animation>, overlays: &HashMap<usize, Vec<RgbaImage>>, watermarks: &HashMap<usize, Arc<RgbaImage>>) -> Result<()> {
    file.version = VERSION;
    file.app = env!("CARGO_PKG_VERSION").into();
    file.frames = meta(base);
    file.loop_count = base.loop_count;
    // only keep the original when it isn't simply the same frames
    let orig = original.filter(|o| o.len() != base.len() || o.frames.iter().zip(&base.frames).any(|(a, b)| !crate::pixels::Pixels::ptr_eq(&a.image, &b.image) && a.image.get().as_raw() != b.image.get().as_raw()));
    file.original = orig.map(|o| (meta(o), o.loop_count));

    // encode every image in parallel, then write them in order
    // frames are shared pictures, layers' are borrowed
    enum Pic<'a> {
        Own(std::sync::Arc<RgbaImage>),
        Ref(&'a RgbaImage),
    }
    impl std::ops::Deref for Pic<'_> {
        type Target = RgbaImage;
        fn deref(&self) -> &RgbaImage {
            match self {
                Pic::Own(a) => a,
                Pic::Ref(r) => r,
            }
        }
    }
    let mut jobs: Vec<(String, Pic)> = Vec::new();
    for (i, f) in base.frames.iter().enumerate() {
        jobs.push((format!("frames/{i:05}.png"), Pic::Own(f.image.get())));
    }
    if let Some(o) = orig {
        for (i, f) in o.frames.iter().enumerate() {
            jobs.push((format!("original/{i:05}.png"), Pic::Own(f.image.get())));
        }
    }
    for (id, frames) in overlays {
        for (i, f) in frames.iter().enumerate() {
            jobs.push((format!("overlays/{id}/{i:05}.png"), Pic::Ref(f)));
        }
    }
    for (id, img) in watermarks {
        jobs.push((format!("watermarks/{id}.png"), Pic::Ref(img)));
    }
    let encoded: Vec<(String, Vec<u8>)> = jobs.par_iter().map(|(n, img)| png(img).map(|b| (n.clone(), b))).collect::<Result<_>>()?;

    // write next to the target, then move into place, so a failed save never eats the old file
    let tmp = path.with_extension(format!("{EXT}.part"));
    {
        let f = std::fs::File::create(&tmp).with_context(|| format!("can't write {}", tmp.display()))?;
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
        let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        z.start_file("project.json", deflate)?;
        z.write_all(serde_json::to_string_pretty(&file)?.as_bytes())?;
        for (name, bytes) in encoded {
            z.start_file(name, stored)?;
            z.write_all(&bytes)?;
        }
        z.finish()?.flush()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("can't replace {}", path.display()))?;
    Ok(())
}

/// Read a project.
pub fn load(path: &Path) -> Result<Project> {
    let f = std::fs::File::open(path).with_context(|| format!("can't open {}", path.display()))?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).context("not a GIFkit project")?;
    let mut json = String::new();
    z.by_name("project.json").context("project.json is missing")?.read_to_string(&mut json)?;
    let file: ProjectFile = serde_json::from_str(&json).context("project.json is damaged")?;
    anyhow::ensure!(file.version <= VERSION, "this project was made by a newer GIFkit ({})", file.app);

    // pull every PNG out, then decode them in parallel
    let mut raw: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let name = e.name().to_string();
        if name.ends_with(".png") {
            let mut b = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut b)?;
            raw.insert(name, b);
        }
    }
    let decoded: HashMap<String, RgbaImage> = raw.into_par_iter().map(|(n, b)| image::load_from_memory(&b).map(|i| (n.clone(), i.to_rgba8())).with_context(|| format!("{n} is damaged"))).collect::<Result<_>>()?;
    let mut decoded = decoded;
    let mut frames_of = |dir: &str, meta: &[FrameMeta], loop_count: Option<u16>| -> Result<Animation> {
        let frames = meta
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let img = decoded.remove(&format!("{dir}/{i:05}.png")).with_context(|| format!("{dir}/{i:05}.png is missing"))?;
                Ok(Frame { image: crate::pixels::Pixels::new(img), delay_cs: m.delay, enabled: m.enabled, cut: m.cut, blank: m.blank })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Animation { frames, loop_count })
    };
    let base = frames_of("frames", &file.frames, file.loop_count)?;
    anyhow::ensure!(!base.is_empty(), "the project has no frames");
    let original = match &file.original {
        Some((m, lc)) => Some(frames_of("original", m, *lc)?),
        None => None,
    };
    let mut overlays = HashMap::new();
    let mut watermarks = HashMap::new();
    for l in &file.layers {
        match &l.layer {
            ProjLayer::Image { id, delays, .. } => {
                let fr: Vec<RgbaImage> = (0..delays.len().max(1)).filter_map(|i| decoded.remove(&format!("overlays/{id}/{i:05}.png"))).collect();
                if !fr.is_empty() {
                    overlays.insert(*id, fr);
                }
            }
            ProjLayer::Watermark { id, has_image: true, .. } => {
                if let Some(img) = decoded.remove(&format!("watermarks/{id}.png")) {
                    watermarks.insert(*id, img);
                }
            }
            _ => {}
        }
    }
    Ok(Project { file, base, original, overlays, watermarks })
}

pub fn is_project(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}
