//! Project files (`.gifkit`): a zip with `project.json` (frame edits, layers
//! and output settings) plus every image the project needs as PNG, so it
//! opens the same on any machine, even without the original files.
//!
//!   project.json
//!   pics/00000.png …          every picture once (see below)
//!   watermarks/<n>.png        watermark images
//!
//! Pictures are pooled: the frames being edited, the opened file's frames
//! and image layers' frames all point into one list, and a picture that
//! shows up twice is stored once. A picture can be a patch: the part that
//! changed since another picture, drawn over it (where nothing changed in
//! a patch the pixels are left clear, which packs down to almost nothing).
//! PNGs with 256 colours or fewer (every GIF frame) are stored with a
//! palette.
//!
//! Older projects (version 1) kept every frame whole, under frames/,
//! original/ and overlays/<n>/; those still open.

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
const VERSION: u32 = 2;

/// One picture in the pool: whole, or a patch over an earlier one.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PicMeta {
    /// drawn over this picture (None: the PNG is the whole picture)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<usize>,
    #[serde(skip_serializing_if = "is_zero32")]
    pub x: u32,
    #[serde(skip_serializing_if = "is_zero32")]
    pub y: u32,
    /// clear pixels in the patch keep what's under them
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub keep: bool,
}

fn is_zero32(v: &u32) -> bool {
    *v == 0
}

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
    /// a trimmed GIF frame kept behind a clip edge
    #[serde(default, skip_serializing_if = "is_zero")]
    pub trim: u8,
    /// its picture in the pool (version 2)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pic: Option<usize>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
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
    /// the speed envelope
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub speed_env: Vec<crate::timeline::SpeedPoint>,
    /// the picture pool (version 2)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pics: Vec<PicMeta>,
    /// each image layer's frames, as pool pictures
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub overlay_pics: std::collections::BTreeMap<usize, Vec<usize>>,
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
    // a palette when it has 256 colours or fewer
    let mut index: HashMap<[u8; 4], u8> = HashMap::new();
    let mut indexed = Vec::with_capacity((img.width() * img.height()) as usize);
    let mut fits = true;
    for p in img.pixels() {
        let k = p.0;
        let n = index.len();
        let i = match index.get(&k) {
            Some(&i) => i,
            None if n < 256 => {
                index.insert(k, n as u8);
                n as u8
            }
            None => {
                fits = false;
                break;
            }
        };
        indexed.push(i);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        if fits {
            let mut cols = vec![[0u8; 4]; index.len()];
            for (c, &i) in &index {
                cols[i as usize] = *c;
            }
            let palette: Vec<u8> = cols.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
            let mut trns: Vec<u8> = cols.iter().map(|c| c[3]).collect();
            while trns.last() == Some(&255) {
                trns.pop();
            }
            enc.set_color(png::ColorType::Indexed);
            enc.set_palette(palette);
            if !trns.is_empty() {
                enc.set_trns(trns);
            }
            enc.write_header()?.write_image_data(&indexed)?;
        } else {
            enc.set_color(png::ColorType::Rgba);
            enc.write_header()?.write_image_data(img.as_raw())?;
        }
    }
    Ok(out)
}

/// The pool being filled: each picture's meta and what's stored for it,
/// and each one whole (to compare new ones with).
#[derive(Default)]
struct Pool {
    meta: Vec<PicMeta>,
    stored: Vec<Arc<RgbaImage>>,
    whole: Vec<Arc<RgbaImage>>,
    seen: HashMap<u64, Vec<usize>>,
}

fn pic_hash(img: &RgbaImage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    img.dimensions().hash(&mut h);
    img.as_raw().hash(&mut h);
    h.finish()
}

impl Pool {
    /// A sequence of pictures into the pool, each a patch over the one
    /// before when that saves room (worked out side by side first).
    fn add_all(&mut self, imgs: Vec<Arc<RgbaImage>>) -> Vec<usize> {
        let prep: Vec<(u64, Option<(PicMeta, RgbaImage)>)> = (0..imgs.len()).into_par_iter().map(|i| (pic_hash(&imgs[i]), if i > 0 { patch(&imgs[i - 1], &imgs[i]) } else { None })).collect();
        let mut prev = None;
        let mut out = Vec::with_capacity(imgs.len());
        for (img, (h, p)) in imgs.into_iter().zip(prep) {
            let id = self.add(img, h, prev.zip(p));
            out.push(id);
            prev = Some(id);
        }
        out
    }

    /// The pool's number for `img` (hash `h`), adding it if it's new: as
    /// `patch` over the picture given with it when there is one.
    fn add(&mut self, img: Arc<RgbaImage>, h: u64, patch: Option<(usize, (PicMeta, RgbaImage))>) -> usize {
        if let Some(ids) = self.seen.get(&h) {
            if let Some(&i) = ids.iter().find(|&&i| Arc::ptr_eq(&self.whole[i], &img) || self.whole[i].as_raw() == img.as_raw() && self.whole[i].dimensions() == img.dimensions()) {
                return i;
            }
        }
        let id = self.meta.len();
        let (meta, stored) = patch.map(|(p, (m, s))| (PicMeta { over: Some(p), ..m }, Arc::new(s))).unwrap_or_else(|| (PicMeta::default(), img.clone()));
        self.meta.push(meta);
        self.stored.push(stored);
        self.whole.push(img);
        self.seen.entry(h).or_default().push(id);
        id
    }
}

/// `img` as a patch over `under`, when that's worth it: the box round what
/// changed, with unchanged pixels left clear where that can't be mistaken
/// for real see-through.
fn patch(under: &RgbaImage, img: &RgbaImage) -> Option<(PicMeta, RgbaImage)> {
    if under.dimensions() != img.dimensions() {
        return None;
    }
    let (w, h) = img.dimensions();
    let (a, b) = (under.as_raw(), img.as_raw());
    let px = |buf: &[u8], x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        let row = (y * w * 4) as usize..((y + 1) * w * 4) as usize;
        let (ra, rb) = (&a[row.clone()], &b[row]);
        if ra == rb {
            continue;
        }
        // the first and last pixel that differ on this row
        let first = ra.chunks_exact(4).zip(rb.chunks_exact(4)).position(|(p, q)| p != q).unwrap_or(0) as u32;
        let last = (w as usize - 1 - ra.chunks_exact(4).rev().zip(rb.chunks_exact(4).rev()).position(|(p, q)| p != q).unwrap_or(0)) as u32;
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if x0 > x1 {
        return None;
    }
    let (rw, rh) = (x1 - x0 + 1, y1 - y0 + 1);
    let share = (rw * rh) as f32 / (w * h) as f32;
    // clear can mean "unchanged" only where both are solid
    let mut solid = true;
    let mut same = 0usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (p, q) = (px(a, x, y), px(b, x, y));
            if p[3] != 255 || q[3] != 255 {
                solid = false;
            }
            if p == q {
                same += 1;
            }
        }
    }
    let same = same as f32 / (rw * rh) as f32;
    let keep = solid && same > 0.15;
    if !keep && share > 0.7 {
        return None;
    }
    let out = RgbaImage::from_fn(rw, rh, |x, y| {
        let (sx, sy) = (x + x0, y + y0);
        let q = px(b, sx, sy);
        if keep && px(a, sx, sy) == q { image::Rgba([0, 0, 0, 0]) } else { image::Rgba(q) }
    });
    Some((PicMeta { over: None, x: x0, y: y0, keep }, out))
}

/// The pool's pictures whole again, from their decoded PNGs (in order: a
/// patch's picture always comes before it).
fn rebuild(meta: &[PicMeta], mut stored: Vec<Option<RgbaImage>>) -> Result<Vec<Arc<RgbaImage>>> {
    let mut whole: Vec<Arc<RgbaImage>> = Vec::with_capacity(meta.len());
    for (i, m) in meta.iter().enumerate() {
        let img = stored[i].take().with_context(|| format!("pics/{i:05}.png is missing"))?;
        let pic = match m.over {
            None => img,
            Some(o) => {
                let mut base = (*whole.get(o).with_context(|| format!("picture {i} is damaged"))?.clone()).clone();
                for (x, y, p) in img.enumerate_pixels() {
                    let (tx, ty) = (x + m.x, y + m.y);
                    if tx < base.width() && ty < base.height() && !(m.keep && p.0 == [0, 0, 0, 0]) {
                        base.put_pixel(tx, ty, *p);
                    }
                }
                base
            }
        };
        whole.push(Arc::new(pic));
    }
    Ok(whole)
}

fn meta(a: &Animation) -> Vec<FrameMeta> {
    a.frames.iter().map(|f| FrameMeta { delay: f.delay_cs, enabled: f.enabled, cut: f.cut, blank: f.blank, trim: f.trim, pic: None }).collect()
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

    // every picture into the pool once; each frame a patch over the one
    // before it where that's smaller
    let mut pool = Pool::default();
    for (m, id) in file.frames.iter_mut().zip(pool.add_all(base.frames.iter().map(|f| f.image.get()).collect())) {
        m.pic = Some(id);
    }
    if let (Some(o), Some((om, _))) = (orig, file.original.as_mut()) {
        for (m, id) in om.iter_mut().zip(pool.add_all(o.frames.iter().map(|f| f.image.get()).collect())) {
            m.pic = Some(id);
        }
    }
    let mut ids: Vec<&usize> = overlays.keys().collect();
    ids.sort();
    for id in ids {
        let v = pool.add_all(overlays[id].iter().map(|f| Arc::new(f.clone())).collect());
        file.overlay_pics.insert(*id, v);
    }
    file.pics = pool.meta;
    let mut jobs: Vec<(String, Arc<RgbaImage>)> = pool.stored.into_iter().enumerate().map(|(i, img)| (format!("pics/{i:05}.png"), img)).collect();
    for (id, img) in watermarks {
        jobs.push((format!("watermarks/{id}.png"), img.clone()));
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
    // the pool (version 2): each picture whole, shared by every frame using it
    let stored: Vec<Option<RgbaImage>> = (0..file.pics.len()).map(|i| decoded.remove(&format!("pics/{i:05}.png"))).collect();
    let pics: Vec<crate::pixels::Pixels> = rebuild(&file.pics, stored)?.into_iter().map(crate::pixels::Pixels::from_arc).collect();
    let mut frames_of = |dir: &str, meta: &[FrameMeta], loop_count: Option<u16>| -> Result<Animation> {
        let frames = meta
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let image = match m.pic {
                    Some(p) => pics.get(p).cloned().with_context(|| format!("picture {p} is missing"))?,
                    None => crate::pixels::Pixels::new(decoded.remove(&format!("{dir}/{i:05}.png")).with_context(|| format!("{dir}/{i:05}.png is missing"))?),
                };
                Ok(Frame { image, delay_cs: m.delay, enabled: m.enabled, cut: m.cut, blank: m.blank, trim: m.trim })
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
                let fr: Vec<RgbaImage> = match file.overlay_pics.get(id) {
                    Some(v) => v.iter().filter_map(|&p| pics.get(p).map(|x| (*x.get()).clone())).collect(),
                    None => (0..delays.len().max(1)).filter_map(|i| decoded.remove(&format!("overlays/{id}/{i:05}.png"))).collect(),
                };
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

#[cfg(test)]
mod size_tests {
    use super::*;

    fn anim(frames: Vec<RgbaImage>) -> Animation {
        Animation { frames: frames.into_iter().map(|f| Frame::new(f, 7)).collect(), loop_count: None }
    }

    /// Every kind of picture comes back exactly: see-through ones, repeats,
    /// small changes (patches), whole changes, many colours, and image
    /// layer frames.
    #[test]
    fn pictures_come_back_exactly() {
        let bg = |seed: u32| RgbaImage::from_fn(64, 48, move |x, y| image::Rgba([(x * 3 + seed) as u8, (y * 5) as u8, ((x ^ y) * 7) as u8, 255]));
        let mut a = bg(0);
        let mut moved = a.clone();
        for x in 10..20 {
            moved.put_pixel(x, 12, image::Rgba([0, 0, 0, 0]));
            moved.put_pixel(x, 13, image::Rgba([9, 9, 9, 9]));
        }
        a.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        // many colours (no palette), a repeat, a see-through frame
        let rich = RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 4) as u8, (y * 5) as u8, ((x * y) % 251) as u8, 255]));
        let clear = RgbaImage::from_fn(64, 48, |x, _| image::Rgba([255, 0, 0, if x < 30 { 0 } else { 128 }]));
        let frames = vec![a.clone(), moved.clone(), a.clone(), bg(40), rich.clone(), rich, clear.clone(), a];
        let base = anim(frames.clone());
        let orig = anim(vec![frames[0].clone(), bg(3)]);
        let mut overlays = HashMap::new();
        overlays.insert(5usize, vec![clear.clone(), moved.clone(), clear]);
        let out = std::env::temp_dir().join(format!("pics-{}.gifkit", std::process::id()));
        let mut file = ProjectFile::default();
        file.layers = vec![ProjLayerEntry { visible: true, label: None, layer: ProjLayer::Image { id: 5, name: "x".into(), path: None, delays: vec![5; 3], placements: Vec::new(), opacity: 1.0 }, timing: Default::default(), locked: false, color: None, opacity: 100.0, clip: false }];
        save(&out, file, &base, Some(&orig), &overlays, &HashMap::new()).unwrap();
        let back = load(&out).unwrap();
        let _ = std::fs::remove_file(&out);
        for (i, (f, want)) in back.base.frames.iter().zip(&frames).enumerate() {
            assert_eq!(f.image.get().as_raw(), want.as_raw(), "frame {i}");
        }
        let o = back.original.unwrap();
        assert_eq!(o.frames[1].image.get().as_raw(), bg(3).as_raw());
        let ov = &back.overlays[&5];
        assert_eq!(ov.len(), 3);
        assert_eq!(ov[1].as_raw(), moved.as_raw());
        // repeats are stored once
        assert!(back.file.pics.len() <= 8, "{} pictures", back.file.pics.len());
    }

    /// Projects saved before the pool (every frame whole) still open.
    #[test]
    fn version_1_projects_open() {
        let img = RgbaImage::from_fn(8, 8, |x, y| image::Rgba([x as u8 * 30, y as u8 * 30, 0, 255]));
        let out = std::env::temp_dir().join(format!("v1-{}.gifkit", std::process::id()));
        {
            let f = std::fs::File::create(&out).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("project.json", o).unwrap();
            z.write_all(br#"{"version":1,"app":"1.4.0","frames":[{"delay":5,"enabled":true},{"delay":6,"enabled":true}],"transform":{},"layers":[]}"#).unwrap();
            for i in 0..2 {
                z.start_file(format!("frames/{i:05}.png"), o).unwrap();
                let mut b = Vec::new();
                image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png).unwrap();
                z.write_all(&b).unwrap();
            }
            z.finish().unwrap();
        }
        let back = load(&out);
        let _ = std::fs::remove_file(&out);
        let back = back.unwrap();
        assert_eq!(back.base.len(), 2);
        assert_eq!(back.base.frames[1].delay_cs, 6);
        assert_eq!(back.base.frames[0].image.get().as_raw(), img.as_raw());
    }

    /// How big a project of a file comes out, and what's in it.
    /// FILE=… cargo test --release project_size -- --ignored --nocapture
    #[test]
    #[ignore]
    fn project_size() {
        let src = std::path::PathBuf::from(std::env::var("FILE").unwrap());
        let a = crate::io::load_animation(&src).unwrap();
        let out = std::env::temp_dir().join("size-test.gifkit");
        let t = std::time::Instant::now();
        save(&out, ProjectFile::default(), &a, None, &HashMap::new(), &HashMap::new()).unwrap();
        let ms = t.elapsed().as_millis();
        let size = std::fs::metadata(&out).unwrap().len();
        eprintln!("{}: {} frames {}x{}, file {} KB, project {} KB, saved in {ms} ms", src.display(), a.len(), a.width(), a.height(), std::fs::metadata(&src).unwrap().len() / 1024, size / 1024);
        let t = std::time::Instant::now();
        let back = load(&out).unwrap();
        eprintln!("loaded in {} ms", t.elapsed().as_millis());
        assert_eq!(back.base.len(), a.len());
        for (x, y) in back.base.frames.iter().zip(&a.frames) {
            assert_eq!(x.image.get().as_raw(), y.image.get().as_raw());
            assert_eq!(x.delay_cs, y.delay_cs);
        }
    }
}
