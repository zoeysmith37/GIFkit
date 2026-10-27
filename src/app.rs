//! The egui application.

use crate::anim::{effective_delay, Animation};
use crate::effects::{CensorMethod, CensorSettings, EffectSettings, ShieldSettings};
use crate::shapes::{EmojiSettings, MarkupSettings, PenMode, PenStroke, ShapeKind, ShapeSettings};
use crate::geo::{map_box, source_angle, unmap_box, Affine, Obb};
use crate::encode::GifOptions;
use crate::io::{self, SheetOptions, SplitFormat, VideoFormat};
use crate::ops::{self, AspectMode, ResizeFilter};
use crate::overlay::{OverlaySource, Placement};
use crate::stack::{self, Kind, LayerKind, OverlayLayer, PlanFrame, SpeedMode, Stack, Transform};
use crate::text::{self, Align, Annotation, FontEntry};
use crate::theme::{self, mix, p, ThemeChoice};
use eframe::egui::{self, pos2, vec2, Color32, PointerButton, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[path = "timeline_ui.rs"]
mod timeline_ui;
#[path = "batch_ui.rs"]
mod batch_ui;
#[path = "smoke.rs"]
mod smoke;
#[path = "handbook.rs"]
mod handbook;

// ================================================================ tools

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Tool {
    Resize,
    Crop,
    Rotate,
    Reverse,
    Speed,
    Optimize,
    Effects,
    Text,
    Censor,
    AddImage,
    Watermark,
    Shield,
    Cut,
    Frames,
    Shapes,
    Markup,
    Emoji,
    Presets,
    Animation,
}

impl Tool {
    const ALL: [Tool; 19] = [
        Tool::Effects,
        Tool::Animation,
        Tool::Text,
        Tool::Shapes,
        Tool::Emoji,
        Tool::Markup,
        Tool::Censor,
        Tool::AddImage,
        Tool::Watermark,
        Tool::Shield,
        Tool::Resize,
        Tool::Crop,
        Tool::Rotate,
        Tool::Reverse,
        Tool::Speed,
        Tool::Cut,
        Tool::Frames,
        Tool::Optimize,
        Tool::Presets,
    ];
    fn label(self) -> &'static str {
        match self {
            Tool::Resize => "Resize",
            Tool::Crop => "Crop",
            Tool::Rotate => "Rotate",
            Tool::Reverse => "Reverse",
            Tool::Speed => "Speed",
            Tool::Optimize => "Optimize",
            Tool::Effects => "Effects",
            Tool::Text => "Text",
            Tool::Censor => "Censor",
            Tool::AddImage => "Image",
            Tool::Watermark => "Watermark",
            Tool::Shield => "AI shield",
            Tool::Cut => "Cut",
            Tool::Frames => "Frames",
            Tool::Shapes => "Shapes",
            Tool::Markup => "Markup",
            Tool::Emoji => "Emoji",
            Tool::Presets => "Presets",
            Tool::Animation => "Animation",
        }
    }
    /// Breeze icon name.
    fn breeze(self) -> &'static str {
        match self {
            Tool::Resize => "transform-scale",
            Tool::Crop => "transform-crop",
            Tool::Rotate => "object-rotate-right",
            Tool::Reverse => "media-playlist-repeat",
            Tool::Speed => "speedometer",
            Tool::Optimize => "archive-insert",
            Tool::Effects => "tools-wizard",
            Tool::Text => "draw-text",
            Tool::Censor => "pixelate",
            Tool::AddImage => "insert-image",
            Tool::Watermark => "document-sign",
            Tool::Shield => "security-high",
            Tool::Cut => "edit-cut",
            Tool::Frames => "kdenlive-show-video",
            Tool::Shapes => "shapes",
            Tool::Markup => "draw-freehand",
            Tool::Emoji => "smiley",
            Tool::Presets => "bookmarks",
            Tool::Animation => "animation-stage",
        }
    }
    fn tip(self) -> &'static str {
        match self {
            Tool::Resize => "Resize",
            Tool::Crop => "Crop",
            Tool::Rotate => "Rotate or flip",
            Tool::Reverse => "Reverse, boomerang and loop",
            Tool::Speed => "Playback speed",
            Tool::Optimize => "Compression",
            Tool::Effects => "Colour, filters and distortions",
            Tool::Text => "Text layers",
            Tool::Censor => "Pixelate or blur part of the frame",
            Tool::AddImage => "Image overlays",
            Tool::Watermark => "Text or logo watermark",
            Tool::Shield => "Noise that discourages AI training",
            Tool::Cut => "Keep a range of frames",
            Tool::Frames => "Edit, reorder and time single frames",
            Tool::Shapes => "Shapes, arrows and speech bubbles",
            Tool::Emoji => "Emoji stickers",
            Tool::Markup => "Draw and highlight with a pen",
            Tool::Presets => "Save edits to reuse on other files",
            Tool::Animation => "Intros, outros, looping motion and a timer",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Tool::Rotate => "Rotate / Flip",
            Tool::Optimize => "Compression",
            Tool::AddImage => "Image",
            Tool::Shield => "AI Shield",
            Tool::Reverse => "Reverse / Loop",
            _ => self.label(),
        }
    }
    fn layer_kind(self) -> Option<Kind> {
        match self {
            Tool::Effects => Some(Kind::Effects),
            Tool::Animation => Some(Kind::Animation),
            Tool::Text => Some(Kind::Text),
            Tool::Censor => Some(Kind::Censor),
            Tool::AddImage => Some(Kind::Overlay),
            Tool::Watermark => Some(Kind::Watermark),
            Tool::Shield => Some(Kind::Shield),
            Tool::Shapes => Some(Kind::Shape),
            Tool::Markup => Some(Kind::Markup),
            Tool::Emoji => Some(Kind::Emoji),
            _ => None,
        }
    }
    /// Tools that use left-drag on the canvas (otherwise left-drag pans).
    fn uses_canvas_drag(self) -> bool {
        matches!(self, Tool::Crop | Tool::Text | Tool::Censor | Tool::AddImage | Tool::Shapes | Tool::Markup | Tool::Emoji)
    }
}

fn kind_tool(k: Kind) -> Tool {
    match k {
        Kind::Effects => Tool::Effects,
        Kind::Animation => Tool::Animation,
        Kind::Text => Tool::Text,
        Kind::Censor => Tool::Censor,
        Kind::Overlay => Tool::AddImage,
        Kind::Watermark => Tool::Watermark,
        Kind::Shield => Tool::Shield,
        Kind::Shape => Tool::Shapes,
        Kind::Markup => Tool::Markup,
        Kind::Emoji => Tool::Emoji,
    }
}

/// Compression settings + the resize part of the frame edits: what "Find
/// settings" changes, so it can be undone before searching again.
#[derive(Clone, PartialEq, Debug)]
struct CompState {
    gif: GifOptions,
    webp: io::WebpOptions,
    avif: u8,
    resize: (bool, u32, u32, u32), // on, w, h, pct bits
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExportFormat {
    Gif,
    Webp,
    Avif,
    Apng,
    Mp4,
    Webm,
    Frames,
    Sheet,
}

impl ExportFormat {
    const ALL: [ExportFormat; 8] = [ExportFormat::Gif, ExportFormat::Webp, ExportFormat::Avif, ExportFormat::Apng, ExportFormat::Mp4, ExportFormat::Webm, ExportFormat::Frames, ExportFormat::Sheet];
    fn label(self) -> &'static str {
        match self {
            ExportFormat::Gif => "GIF",
            ExportFormat::Webp => "WebP",
            ExportFormat::Avif => "AVIF",
            ExportFormat::Apng => "APNG",
            ExportFormat::Mp4 => "MP4",
            ExportFormat::Webm => "WebM",
            ExportFormat::Frames => "Frames",
            ExportFormat::Sheet => "Sprite sheet",
        }
    }
    fn needs_ffmpeg(self) -> bool {
        matches!(self, ExportFormat::Mp4 | ExportFormat::Webm | ExportFormat::Avif)
    }
    /// Formats that can carry the "noai" markers (all of them).
    fn can_tag(self) -> bool {
        true
    }
    fn can_target(self) -> bool {
        matches!(self, ExportFormat::Gif | ExportFormat::Webp | ExportFormat::Avif)
    }
    fn tip(self) -> &'static str {
        match self {
            ExportFormat::Gif => "Plays everywhere",
            ExportFormat::Webp => "Often 30 to 70% smaller than GIF",
            ExportFormat::Avif => "Smallest files. Needs a modern browser.",
            ExportFormat::Apng => "Lossless with full colour and transparency",
            ExportFormat::Mp4 => "Video without transparency",
            ExportFormat::Webm => "Video with transparency",
            ExportFormat::Frames => "One image per frame",
            ExportFormat::Sheet => "Every frame on one image for game engines",
        }
    }
}

// ================================================================ jobs

enum JobOutput {
    Anim(anyhow::Result<Animation>),
    Baked(anyhow::Result<(Animation, Stack)>),
    Loaded(anyhow::Result<(Animation, Option<PathBuf>, Option<u64>)>),
    Info(anyhow::Result<String>),
    Overlay(anyhow::Result<OverlaySource>),
    /// target-size search result to apply
    Fit(anyhow::Result<crate::target::Fit>),
    ProjectLoaded(anyhow::Result<(crate::project::Project, PathBuf)>),
    ProjectSaved(anyhow::Result<PathBuf>),
    /// the best loop: first output frame, the one matching it, how different
    LoopPoint(Option<(usize, usize, f32)>),
    /// a layer (id) to put on frames `first..`: its centres there, and
    /// where it was placed
    Track(u64, usize, Vec<(f32, f32)>, (f32, f32)),
}

/// Something that would throw away unsaved changes, waiting for an answer.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    New,
    Quit,
    Open(PathBuf),
    Sequence(Vec<PathBuf>),
}

struct Job {
    label: String,
    rx: Receiver<JobOutput>,
    progress: Arc<AtomicUsize>,
    total: usize,
}

struct CmpResult {
    key: u64,
    /// the preview is a sprite sheet (one big image, not frames)
    sheet: bool,
    anim: Animation,
    bytes: Arc<Vec<u8>>,
    gen: u64,
    /// for long animations: the plan frames that were encoded (the rest is estimated)
    sample: Option<Vec<usize>>,
    /// file size: exact, or estimated from the sample
    size: u64,
    /// the frames' real size (big results are shrunk for the screen)
    full: (u32, u32),
    /// when (seconds) the exported part starts and ends: the in and out
    /// points, less see-through frames cut off the end
    span: (f32, f32),
}

impl CmpResult {
    /// The encoded frame to show for plan frame `cur` at time `t`.
    fn frame_for(&self, cur: usize, t: f32, plan: &[stack::PlanFrame]) -> usize {
        match &self.sample {
            // the sample's frames play back to back, and identical neighbours
            // are merged into one longer frame, so find the nearest sampled
            // frame and look it up by its time within the sample
            Some(s) => {
                let Some(j) = s.iter().enumerate().min_by_key(|(_, i)| (**i as i64 - cur as i64).unsigned_abs()).map(|(j, _)| j) else { return 0 };
                let at: u32 = s[..j].iter().filter_map(|&i| plan.get(i)).map(|f| effective_delay(f.delay) as u32).sum();
                stack::frame_at_time(&self.anim, at as f32 / 100.0 + 0.001)
            }
            None => stack::frame_at_time(&self.anim, (t - self.span.0).max(0.0)),
        }
    }

    /// Is time `t` (seconds) in what's exported?
    fn covers(&self, t: f32) -> bool {
        self.sample.is_some() || (t + 1e-4 >= self.span.0 && t < self.span.1 - 1e-4)
    }

    fn size_text(&self) -> String {
        if self.sample.is_some() { format!("≈ {}", fmt_size(self.size)) } else { fmt_size(self.size) }
    }
}

/// Above this many pixels (frames × width × height) the compressed preview
/// encodes a sample of the animation and estimates the size, so editing long
/// clips stays quick. "Compress Fully" does the real thing on request.
const SAMPLE_PIXELS: u64 = 60_000_000;

struct CmpJob {
    key: u64,
    sheet: bool,
    /// set to stop it (an edit made it out of date)
    stop: Arc<AtomicUsize>,
    rx: Receiver<anyhow::Result<(Animation, Vec<u8>, Option<Vec<usize>>, u64, (u32, u32), (f32, f32))>>,
}

struct Prerender {
    key: u64,
    rx: Receiver<(u64, usize, RgbaImage)>,
}

#[derive(Clone)]
struct Snapshot {
    anim: Animation,
    stack: Stack,
    gif: GifOptions,
    /// the edit made after this state (shown in History)
    label: String,
}

/// A picture to show: one egui made from pixels, or one the graphics card
/// rendered (kept alive by the cache that owns it).
#[derive(Clone)]
struct Tex {
    id: egui::TextureId,
    size: Vec2,
    /// an egui texture stays alive while any copy of this is held (the last
    /// picture kept on screen outlives the cache that made it)
    _keep: Option<TextureHandle>,
}

impl Tex {
    fn id(&self) -> egui::TextureId {
        self.id
    }
    fn size_vec2(&self) -> Vec2 {
        self.size
    }
}

impl From<TextureHandle> for Tex {
    fn from(t: TextureHandle) -> Tex {
        // the handle's cache keeps the texture; egui frees it when the cache drops it
        Tex { id: t.id(), size: t.size_vec2(), _keep: Some(t) }
    }
}

/// Graphics memory the views' rendered frames may hold.
const GPU_FRAMES_BUDGET: usize = 160 << 20;

/// Frames the graphics card rendered for the views, registered with egui.
struct GpuFrames {
    key: u64,
    map: HashMap<usize, (eframe::wgpu::Texture, egui::TextureId, u64)>,
    /// the frame last put on screen, kept after its key went stale so the
    /// view never shows a freed texture while the new one is made
    shown: Option<(eframe::wgpu::Texture, egui::TextureId)>,
    bytes: usize,
    tick: u64,
    /// frames in a row the last picture stayed up while the card was busy
    since_busy: u32,
}

/// Frame thumbnails, made in the background and only for frames someone
/// is looking at (the frame grid and the timeline), kept while there's room.
#[derive(Default)]
struct Thumbs {
    key: u64,
    map: HashMap<usize, (TextureHandle, u64)>,
    pending: std::collections::HashSet<usize>,
    queue: Vec<(usize, Arc<RgbaImage>)>,
    rx: Option<Receiver<(u64, usize, RgbaImage)>>,
    tx: Option<std::sync::mpsc::Sender<(u64, usize, RgbaImage)>>,
    tick: u64,
}

/// Longest side of a thumbnail, and how many are kept on the graphics card.
const THUMB_SIDE: u32 = 192;
const THUMB_KEEP: usize = 800;

impl Thumbs {
    fn get(&mut self, key: u64, i: usize) -> Option<TextureHandle> {
        if self.key != key {
            self.map.clear();
            self.pending.clear();
            self.queue.clear();
            self.key = key;
        }
        let tick = self.tick;
        self.map.get_mut(&i).map(|e| {
            e.1 = tick;
            e.0.clone()
        })
    }

    fn want(&mut self, i: usize, img: Arc<RgbaImage>) {
        if self.pending.insert(i) {
            self.queue.push((i, img));
        }
    }

    /// Once a frame: take in finished thumbnails, start the ones asked for.
    fn pump(&mut self, ctx: &egui::Context) {
        self.tick += 1;
        if self.tx.is_none() {
            let (tx, rx) = channel();
            (self.tx, self.rx) = (Some(tx), Some(rx));
        }
        let mut got = 0;
        // a few dozen uploads a frame keeps scrolling smooth
        while got < 48 {
            let Some(Ok((key, i, img))) = self.rx.as_ref().map(|r| r.try_recv()) else { break };
            got += 1;
            if key != self.key {
                continue;
            }
            self.pending.remove(&i);
            let t = ctx.load_texture(format!("thumb{i}"), to_color_image(&img), TextureOptions::LINEAR);
            self.map.insert(i, (t, self.tick));
        }
        if self.map.len() > THUMB_KEEP {
            // forget the ones not seen for longest
            let mut v: Vec<(u64, usize)> = self.map.iter().map(|(i, e)| (e.1, *i)).collect();
            v.sort_unstable();
            for (_, i) in v.into_iter().take(self.map.len() - THUMB_KEEP * 4 / 5) {
                self.map.remove(&i);
            }
        }
        if !self.queue.is_empty() {
            let batch = std::mem::take(&mut self.queue);
            let (tx, key, ctx2) = (self.tx.clone().unwrap(), self.key, ctx.clone());
            stack::bg_pool().spawn(move || {
                use rayon::prelude::*;
                batch.into_par_iter().for_each_with(tx, |tx, (i, img)| {
                    let s = (THUMB_SIDE as f32 / img.width().max(img.height()) as f32).min(1.0);
                    let (w, h) = (((img.width() as f32 * s).round() as u32).max(1), ((img.height() as f32 * s).round() as u32).max(1));
                    let th = if s < 1.0 { image::imageops::resize(&*img, w, h, image::imageops::FilterType::Triangle) } else { (*img).clone() };
                    let _ = tx.send((key, i, th));
                });
                ctx2.request_repaint();
            });
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
}

/// Texture cache invalidated by a key. The least recently shown go first
/// once it's over its budget.
#[derive(Default)]
struct TexCache {
    key: u64,
    map: HashMap<usize, (TextureHandle, u64)>,
    /// bytes of texture held, to stay inside a budget
    bytes: usize,
    tick: u64,
    /// the on-screen size it was filled for
    side: u32,
}

/// Graphics memory one texture cache may hold. On built-in graphics this is
/// ordinary memory, so it's kept to what smooth playback needs.
const TEX_BUDGET: usize = 64 << 20;

/// Pictures wider or taller than this are shrunk before they go to the
/// graphics card: the screen can't show more, and uploads stay quick.
const TEX_MAX_SIDE: u32 = 2048;

/// How big (longest side, in screen pixels) pictures are shown at the
/// moment, rounded up to a power of two. Pictures shown at under half their
/// size go to the card at this size instead.
static TEX_SIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(TEX_MAX_SIDE);
/// The biggest any view showed a picture this frame (screen pixels).
static TEX_NEED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl TexCache {
    fn get(&mut self, key: u64, i: usize) -> Option<TextureHandle> {
        let side = TEX_SIDE.load(Ordering::Relaxed);
        if self.key != key || self.side != side {
            self.map.clear();
            self.bytes = 0;
            self.key = key;
            self.side = side;
        }
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&i).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }
    fn put(&mut self, ctx: &egui::Context, i: usize, img: &RgbaImage, opts: TextureOptions, name: &str) -> TextureHandle {
        let side = img.width().max(img.height());
        // no bigger than the screen shows it (when that saves most of it)
        let screen = TEX_SIDE.load(Ordering::Relaxed);
        let limit = if screen * 2 <= side { screen } else { TEX_MAX_SIDE };
        let small;
        let img = if side > limit {
            let k = limit as f32 / side as f32;
            let (w, h) = stack::proxy_size(img.width(), img.height(), k);
            small = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
            &small
        } else {
            img
        };
        let size = img.width() as usize * img.height() as usize * 4;
        // the least recently shown go, down to seven eighths
        if self.bytes + size > crate::pixels::scaled(TEX_BUDGET) {
            let mut v: Vec<(u64, usize)> = self.map.iter().map(|(k, e)| (e.1, *k)).collect();
            v.sort_unstable();
            for (_, k) in v {
                if self.bytes + size <= crate::pixels::scaled(TEX_BUDGET) / 8 * 7 {
                    break;
                }
                if let Some((t, _)) = self.map.remove(&k) {
                    self.bytes -= t.size()[0] * t.size()[1] * 4;
                }
            }
        }
        let t = ctx.load_texture(format!("{name}{i}"), to_color_image(img), opts);
        self.tick += 1;
        self.map.insert(i, (t.clone(), self.tick));
        self.bytes += size;
        t
    }
    /// How many pictures `w` by `h` fit in the budget.
    fn room_for(w: u32, h: u32) -> usize {
        crate::pixels::scaled(TEX_BUDGET) / (w as usize * h as usize * 4).max(1)
    }
}

// ================================================================ misc state

#[derive(Clone, Copy, PartialEq, Debug)]
enum AspectLock {
    Free,
    Square,
    R4x3,
    R3x4,
    R16x9,
    R9x16,
    R3x2,
    R2x1,
    Original,
}

impl AspectLock {
    const ALL: [AspectLock; 9] = [
        AspectLock::Free,
        AspectLock::Square,
        AspectLock::R4x3,
        AspectLock::R3x4,
        AspectLock::R16x9,
        AspectLock::R9x16,
        AspectLock::R3x2,
        AspectLock::R2x1,
        AspectLock::Original,
    ];
    fn label(self) -> &'static str {
        match self {
            AspectLock::Free => "No",
            AspectLock::Square => "1:1 (square)",
            AspectLock::R4x3 => "4:3",
            AspectLock::R3x4 => "3:4",
            AspectLock::R16x9 => "16:9",
            AspectLock::R9x16 => "9:16",
            AspectLock::R3x2 => "3:2",
            AspectLock::R2x1 => "2:1",
            AspectLock::Original => "Same as image",
        }
    }
    fn ratio(self, w: u32, h: u32) -> Option<f32> {
        Some(match self {
            AspectLock::Free => return None,
            AspectLock::Square => 1.0,
            AspectLock::R4x3 => 4.0 / 3.0,
            AspectLock::R3x4 => 3.0 / 4.0,
            AspectLock::R16x9 => 16.0 / 9.0,
            AspectLock::R9x16 => 9.0 / 16.0,
            AspectLock::R3x2 => 1.5,
            AspectLock::R2x1 => 2.0,
            AspectLock::Original => w as f32 / h.max(1) as f32,
        })
    }
}

/// Which frames an overlay placement edit affects.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PlaceScope {
    All,
    Following,
    This,
}

impl PlaceScope {
    fn label(self) -> &'static str {
        match self {
            PlaceScope::All => "All frames",
            PlaceScope::Following => "This and following frames",
            PlaceScope::This => "This frame only",
        }
    }
    fn range(self, cur: usize, n: usize) -> std::ops::Range<usize> {
        match self {
            PlaceScope::All => 0..n,
            PlaceScope::Following => cur..n,
            PlaceScope::This => cur..(cur + 1).min(n),
        }
    }
}

/// What a viewport shows.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ViewSource {
    Edit,
    Original,
    Compressed,
}

impl ViewSource {
    const ALL: [ViewSource; 3] = [ViewSource::Original, ViewSource::Edit, ViewSource::Compressed];
    fn tip(self) -> &'static str {
        match self {
            ViewSource::Edit => "All edits before compression",
            ViewSource::Original => "The file as opened",
            ViewSource::Compressed => "All edits as the saved file will look",
        }
    }
    fn label(self) -> &'static str {
        match self {
            ViewSource::Edit => "Live edit",
            ViewSource::Original => "Original",
            ViewSource::Compressed => "Compressed",
        }
    }
}

/// Coordinate space a viewport shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Space {
    /// source frames (original file, crop view)
    Source,
    /// the edited output (live edit, compressed)
    Output,
}

/// In-progress canvas edit of a layer box.
#[derive(Clone, Copy)]
enum BoxDrag {
    Move { id: u64, li: usize, start: Pos2, src: Obb },
    /// `axis`: 0 = both (corner), 1 = width only, 2 = height only (edge handles).
    /// `size0` / `stretch0`: text size and stretch at the start.
    Resize { id: u64, li: usize, fixed: (f32, f32), out0: Obb, src: Obb, size0: f32, stretch0: f32, axis: u8 },
    Rotate { id: u64, li: usize, a0: f32, out0: Obb, src: Obb },
    New { id: u64, li: usize, anchor: Pos2 },
}

impl BoxDrag {
    fn id(&self) -> u64 {
        match *self {
            BoxDrag::Move { id, .. } | BoxDrag::Resize { id, .. } | BoxDrag::Rotate { id, .. } | BoxDrag::New { id, .. } => id,
        }
    }
    fn li(&self) -> usize {
        match *self {
            BoxDrag::Move { li, .. } | BoxDrag::Resize { li, .. } | BoxDrag::Rotate { li, .. } | BoxDrag::New { li, .. } => li,
        }
    }
}

/// Zoom / pan state of one viewport.
#[derive(Clone, Copy)]
struct ViewState {
    zoom: Option<f32>,
    pan: Vec2,
    fit: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self { zoom: None, pan: Vec2::ZERO, fit: 1.0 }
    }
}

#[derive(Clone, Copy)]
enum DragMode {
    New(Pos2),
    Move(Vec2),
    Corner(Pos2),
}

/// Screen <-> image coordinate mapping of a viewport.
#[derive(Clone, Copy)]
struct Mapper {
    rect: Rect,
    scale: f32,
}

impl Mapper {
    fn to_img(&self, p: Pos2) -> Pos2 {
        ((p - self.rect.min) / self.scale).to_pos2()
    }
    fn to_screen(&self, p: Pos2) -> Pos2 {
        self.rect.min + p.to_vec2() * self.scale
    }
    fn rect_to_screen(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_max(self.to_screen(pos2(x, y)), self.to_screen(pos2(x + w, y + h)))
    }
}

struct Status {
    text: String,
    error: bool,
}

// ================================================================ app

pub struct GifKitApp {
    base: Option<Animation>,
    original: Option<Animation>,
    orig_gen: u64,
    path: Option<PathBuf>,
    file_size: Option<u64>,
    modified: bool,
    version: u64,
    timing_rev: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_coalesce: Option<(String, f64)>,

    stack: Stack,
    fx_draft: EffectSettings,
    /// the Animation panel's settings before its layer exists
    anim_draft: EffectSettings,
    sel: HashMap<&'static str, u64>,

    plan: Vec<PlanFrame>,
    plan_times: Vec<f32>,
    plan_key: (u64, u64, u64),
    out_size: (f32, f32),

    tool: Tool,
    playing: bool,
    cur: usize,
    elapsed: f64,

    /// 0 = single view, 1 = left, 2 = right
    views: [ViewState; 3],
    active_view: usize,
    link_views: bool,
    box_drag: Option<BoxDrag>,
    /// what transparent pixels show on in the viewports
    backdrop: Backdrop,
    backdrop_col: [u8; 3],
    pixelated: bool,
    /// ghosts of the frames around the playhead on the preview
    onion: bool,
    /// those ghosts for the viewport being drawn: picture and tint
    onion_paint: Vec<(Tex, Color32)>,

    base_tex: TexCache,
    thumbs: Thumbs,
    comp_tex: TexCache,
    orig_tex: TexCache,
    cmp_tex: TexCache,
    crop_tex: TexCache,
    crop_prep: Option<(u64, Arc<stack::Prepared>)>,
    crop_live: bool,
    checker_tex: Option<TextureHandle>,
    comp_key: u64,
    comp_key_since: f64,
    prerender: Option<Prerender>,
    prerender_cancel: Arc<AtomicU64>,

    job: Option<Job>,
    status: Option<Status>,
    fonts: Vec<FontEntry>,
    builtin_font_count: usize,
    sys_fonts: Arc<std::sync::Mutex<Option<Vec<crate::sysfonts::SysFamily>>>>,
    font_keys: HashMap<String, usize>,
    font_meta: HashMap<usize, (String, String)>,
    font_search: String,
    ffmpeg: bool,
    theme: ThemeChoice,

    compare: bool,
    view_left: ViewSource,
    view_right: ViewSource,
    view_single: ViewSource,
    cmp: Option<CmpResult>,
    cmp_job: Option<CmpJob>,
    /// frames being packed in the background: (old id, packed)
    pack_job: Option<Receiver<Vec<(u64, crate::pixels::Pixels)>>>,
    /// frames that didn't pack smaller (not tried again)
    pack_tried: std::collections::HashSet<u64>,
    pack_last: f64,
    cmp_key_seen: u64,
    cmp_key_since: f64,
    cmp_gen: u64,

    // tool UI state
    crop_lock: AspectLock,
    trim_tol: u8,
    drag: Option<DragMode>,
    gif_opts: GifOptions,
    /// the settings picked to match the opened GIF
    gif_opts_open: GifOptions,
    opt_drop_nth: usize,
    place_scope: PlaceScope,
    split_fmt: SplitFormat,
    split_prefix: String,
    sheet: SheetOptions,
    fr_from: usize,
    fr_to: usize,
    fr_every: usize,
    fr_delay: u16,
    fr_crossfade: usize,
    fr_crossfade_wrap: bool,
    /// in-betweens move with the picture, not only fade
    fr_follow: bool,
    /// Make Seamless: how many frames the end fades into the start over
    loop_blend: usize,
    fr_thumb: f32,
    video_bg: [u8; 3],

    // layout
    sidebar_compact: bool,
    layers_collapsed: bool,
    /// where the divider between the two views sits, 0 to 1 across
    split_frac: f32,
    /// the layer panel's height when open, to slide back to
    layers_open_h: f32,
    /// the timeline's height when shown, to slide back to
    timeline_open_h: f32,
    layer_drag: Option<u64>,
    export_open: bool,
    export_fmt: ExportFormat,
    frames_zip: bool,
    presets: Vec<crate::presets::Preset>,
    batch: batch_ui::BatchState,
    smoke: Option<smoke::Smoke>,
    handbook_open: bool,
    handbook_topic: usize,
    preset_problems: Vec<String>,
    preset_name: String,
    preset_inc: (bool, bool, bool),
    font_preview_queue: Vec<String>,
    preview_fonts: HashMap<String, bool>,
    prep_cache: Option<(u64, Arc<stack::Prepared>)>,
    webp_opts: io::WebpOptions,
    avif_quality: u8,
    target_val: f32,
    target_mb: bool,
    history_open: bool,
    /// last committed live state for automatic undo entries: (signature, stack, gif options)
    committed: Option<(u64, Stack, GifOptions)>,
    pending_sig: u64,
    commit_since: f64,
    last_comp: Option<(usize, Tex)>,
    /// rendering on the graphics card, when the window draws with wgpu
    gpu: Option<(crate::gpu_render::GpuRenderer, std::sync::Arc<egui::mutex::RwLock<eframe::egui_wgpu::Renderer>>)>,
    gpu_frames: GpuFrames,

    last_tick: Option<f64>,
    /// while playing, the original view keeps its own time (seconds into
    /// the original), so cut or skipped frames in the edit don't make it jump
    orig_clock: f64,
    resize_by_pct: bool,
    about_open: bool,
    /// the ffmpeg set-up window
    ff_open: bool,
    /// a download of ffmpeg under way
    ff_job: Option<(Arc<crate::ffmpeg::Progress>, std::sync::mpsc::Receiver<Result<PathBuf, String>>)>,
    /// what the last try said (an error, or where it was found)
    ff_msg: Option<(bool, String)>,
    /// a video to open once ffmpeg is set up
    ff_then: Option<PathBuf>,
    /// layer being renamed (id, text) and whether its field still needs focus
    renaming: Option<(u64, String)>,
    rename_fresh: bool,
    /// frame edits that keep a row in the layer panel while switched off
    edit_rows_shown: std::collections::HashSet<Tool>,
    /// custom names for frame-edit rows
    edit_labels: HashMap<Tool, String>,
    edit_renaming: Option<(Tool, String)>,
    /// (settings before, settings after) the last "Find settings"
    fit_record: Option<(CompState, CompState)>,
    fit_keep_size: bool,
    frames_png: io::PngOptions,
    export_name: String,
    export_name_edited: bool,
    open_after_save: bool,
    last_dir: Option<PathBuf>,
    /// folder to open once the running export finishes
    pending_open: Option<PathBuf>,
    /// measured width of the transport's view controls (last frame)
    play_speed: f32,
    /// a video waiting for its import settings
    video_import: Option<(PathBuf, io::VideoInfo, io::VideoImport)>,
    /// estimate long animations' compressed size from a sample (setting)
    sample_preview: bool,
    /// the settings the user asked to compress fully once
    cmp_full_key: u64,
    shortcuts_open: bool,
    /// the window title last sent, so it's only set when it changes
    win_title: String,
    // the timeline dock
    show_timeline: bool,
    /// pixels per frame (0 fits the whole animation)
    tl_zoom: f32,
    /// pixels per frame that fit the whole animation, and the zoom now (last drawn)
    tl_fit: f32,
    tl_zoom_now: f32,
    /// where the playhead was last frame, to follow it when it moves
    tl_cur_seen: usize,
    /// a zoom the toolbar asked for, applied about the playhead
    tl_zoom_to: Option<f32>,
    /// middle-button panning, and dragging the scroll bar (grab offset in frames)
    tl_pan: bool,
    tl_bar_drag: Option<f32>,
    /// a clip dragged over others: the boundary it goes in at
    tl_drop: Option<usize>,
    /// the track whose name was right-clicked (its menu is the layer's)
    tl_menu_head: Option<u64>,
    /// more selected clips besides the main one, as (track, first frame)
    tl_sel: Vec<(u64, usize)>,
    /// Copy Result: the file export writes to instead of asking, and the file
    /// to put on the clipboard once it's written
    copy_to: Option<PathBuf>,
    copy_pending: Option<PathBuf>,
    /// autosave: when it last looked, what it last wrote, a write under way
    autosave_at: f64,
    autosave_sig: u64,
    autosave_job: Option<Receiver<bool>>,
    /// work kept from a session that didn't end normally: (file, when, project path)
    restore_offer: Option<(PathBuf, String, Option<PathBuf>)>,
    /// export: leave see-through frames at the end off, and only the part
    /// between the in and out points
    export_cut_empty: bool,
    export_zone_only: bool,
    /// keyframes selected besides the main one: (track, frame)
    tl_keys: Vec<(u64, usize)>,
    /// a keyframe Alt+clicked out of the selection while the playhead is on
    /// it: (track, frame), until the playhead moves
    tl_key_dropped: Option<(u64, usize)>,
    /// keyframes moving together: (track, frame when the drag began), and
    /// how far they've moved
    tl_key_group: Vec<(u64, usize)>,
    tl_key_moved: i64,
    /// clips moving together: (track, clip index, first, last) when the drag began
    tl_group: Vec<(u64, usize, usize, usize)>,
    /// a rubber band selection started here
    tl_box: Option<Pos2>,
    /// copied clips or a keyframe
    tl_board: Option<timeline_ui::TlBoard>,
    /// copied clips that didn't fit: (track, frame, clips as offset and length)
    tl_paste_ask: Option<(u64, usize, Vec<(usize, usize)>)>,
    /// snap edges and keys to things nearby while dragging; the frame snapped to
    tl_snap: bool,
    tl_snap_line: Option<usize>,
    /// in and out points (output frames), and whether playback loops between them
    tl_in: Option<usize>,
    tl_out: Option<usize>,
    tl_zone_loop: bool,
    /// show only this layer in the live view
    tl_solo: Option<u64>,
    /// track heights, and the track whose bottom edge is being dragged
    tl_gif_h: f32,
    tl_row_h: f32,
    tl_row_drag: Option<usize>,
    /// a marker being moved, renamed, or right-clicked
    tl_marker_drag: Option<usize>,
    tl_marker_edit: Option<(usize, String)>,
    tl_menu_marker: Option<usize>,
    /// where the timeline is, and whether it was clicked last (Copy, Paste
    /// and Duplicate then work on its clips rather than on layers)
    tl_area: Rect,
    tl_focus: bool,
    /// layers Copy put aside
    layer_board: Vec<stack::Layer>,
    /// files or a browser picture from the last Ctrl+V
    paste_in: Option<crate::paste::Pasted>,
    /// first frame shown, and how far the tracks are scrolled
    tl_x0: f32,
    tl_y0: f32,
    /// selected track (a layer id, or 0 for the GIF) and clip / key on it
    tl_track: Option<u64>,
    tl_clip: Option<usize>,
    tl_key: Option<usize>,
    tl_razor: bool,
    tl_drag: Option<timeline_ui::TlDrag>,
    /// width of the track name column, and drags on it
    tl_head_w: f32,
    tl_head_drag: bool,
    /// a layer track being dragged to a new place in the stack
    tl_reorder: Option<usize>,
    /// frames the GIF will grow by when the drag past its end lets go
    /// the menu bar, where clicks don't change what Copy and Paste work on
    menubar_rect: Rect,
    tl_extend: usize,
    /// what a drag can snap to, fixed when it starts
    tl_snap_pts: Option<Vec<usize>>,
    /// a clip dragged into free room on its row: where it starts there
    tl_free_drop: Option<usize>,
    /// the GIF's frames (as shown) when an edge drag began
    tl_gif_d0: Option<Vec<crate::anim::Frame>>,
    /// what making the GIF longer adds
    tl_extend_mode: timeline_ui::Extend,
    /// asking whether to save first, and what to do after
    confirm: Option<Pending>,
    /// the same, once a save started from the question finishes
    after_save: Option<Pending>,
    /// the question was answered: let the window close
    quitting: bool,
    /// changing a motion value at a frame without a keyframe adds one
    tl_autokey: bool,
    /// Draw Path: a drag on the preview becomes the selected layer's motion
    path_draw: bool,
    /// the path being drawn, in source pixels
    path_pts: Vec<(f32, f32)>,
    /// Track: a click on the preview picks a spot for the layer to follow
    path_track: bool,
    /// while trimming the GIF's outer ends: the output frames that stay
    tl_trim: Option<(usize, usize)>,
    keymap: crate::keys::Keymap,
    /// the action whose new key is being waited for (shortcuts window)
    key_capture: Option<crate::keys::Action>,
    /// the shortcuts dialog's search text
    keys_filter: String,
    /// the tool panel's search text, and the tool it was typed for
    panel_find: String,
    panel_find_tool: Option<Tool>,
    /// the layer list's search text, and whether its box is showing
    layer_find: String,
    layer_find_open: bool,
    layer_find_fresh: bool,
    /// previews of big pictures are made from frames shrunk to this long
    /// side (0 = always full size)
    preview_max: u32,
    /// memory kept for speed, percent of normal (Low 50, High 250)
    mem_pct: usize,
    /// the process's memory for the status bar, and when it was read
    mem_seen: (usize, f64),
    /// the shrunk frames: (key, scale, frames), and the job making them
    proxy: Option<(u64, f32, Animation, Option<Animation>)>,
    proxy_job: Option<(u64, Receiver<(Animation, Option<Animation>)>)>,
    /// the frame on screen rendering in the background: (key, frame, result)
    live_job: Option<(u64, usize, Receiver<RgbaImage>)>,
    /// how long the last frame took to render here, to decide when to go
    /// to the background instead
    last_render_ms: f64,
    /// an emoji being dragged from the picker onto the picture
    emoji_drag: Option<String>,
    /// views that take drops this frame: where they are and how they map
    drop_views: Vec<(Rect, Mapper)>,
    /// where the edited views were last frame (for keys over them)
    views_last: Vec<Rect>,
    show_toolbar: bool,
    show_statusbar: bool,
    /// recently opened files, newest first
    recent: Vec<PathBuf>,
    /// where the project was last saved (None = never)
    project_path: Option<PathBuf>,
    /// style for the next shape drawn
    shape_tpl: ShapeSettings,
    /// style for the next layer of each kind, edited while none exists
    text_tpl: Annotation,
    censor_tpl: CensorSettings,
    wm_tpl: crate::effects::WatermarkSettings,
    shield_tpl: ShieldSettings,
    emoji_tpl: EmojiSettings,
    emoji_tex: HashMap<String, TextureHandle>,
    /// emoji picker: current pick (file), skin tone, search, tab, recent picks, click mode
    emoji_pick: String,
    emoji_tone: usize,
    emoji_search: String,
    emoji_tab: EmojiTab,
    emoji_recent: Vec<String>,
    emoji_replace: bool,
    pen: PenStroke,
    highlighter: PenStroke,
    pen_tool: PenTool,
    pen_scope: PenScope,
    /// stroke being drawn: (layer id, points in the viewport's image space, space)
    pen_live: Option<(u64, Vec<Pos2>, Space)>,
}

/// Output and tool settings stored in a project file. Every field is
/// optional so projects from other versions still open.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ProjSettings {
    gif: Option<GifOptions>,
    webp: Option<io::WebpOptions>,
    avif: Option<u8>,
    format: String,
    video_bg: Option<[u8; 3]>,
    frames_png: Option<io::PngOptions>,
    split_fmt: Option<SplitFormat>,
    frames_zip: Option<bool>,
    split_prefix: Option<String>,
    sheet: Option<SheetOptions>,
    target_val: Option<f32>,
    target_mb: Option<bool>,
    tool: String,
    edit_labels: Vec<(String, String)>,
}

/// Behind transparent pixels in the viewports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backdrop {
    Checker,
    Solid,
    /// the viewport background itself
    Canvas,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EmojiTab {
    Recent,
    Group(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PenTool {
    Pen,
    Highlighter,
    Eraser,
}

/// Which frames a new markup stroke shows on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PenScope {
    All,
    FromHere,
    ThisFrame,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn hash_parts(parts: &[u64]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

fn fmt_size(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{:.2} MiB", b as f64 / 1024.0 / 1024.0)
    } else if b >= 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

/// "1:05.3" or "12.4 s".
fn fmt_secs(t: f64) -> String {
    if t >= 60.0 {
        format!("{}:{:04.1}", (t / 60.0).floor() as u64, t % 60.0)
    } else {
        format!("{t:.1} s")
    }
}

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Shrink the output by `scale` through the Resize frame edit (as "Find
/// settings" does), keeping any resize already set.
fn apply_fit_scale(t: &mut Transform, scale: f32) {
    if scale >= 0.999 {
        return;
    }
    if t.resize_on && t.rs_pct > 0.0 {
        t.rs_pct *= scale;
    } else if t.resize_on && (t.rs_w > 0 || t.rs_h > 0) {
        t.rs_w = (t.rs_w as f32 * scale).round() as u32;
        t.rs_h = (t.rs_h as f32 * scale).round() as u32;
    } else {
        t.rs_w = 0;
        t.rs_h = 0;
        t.rs_pct = (scale * 1000.0).round() / 10.0;
    }
    t.resize_on = true;
}


/// Draw a tool's icon. A few are vector drawings because no bundled font has a
/// clear glyph for them; the rest use the tool's text glyph.
fn paint_tool_icon(painter: &egui::Painter, t: Tool, c: Pos2, size: f32, col: Color32) {
    crate::icons::paint(painter, t.breeze(), c, if size > 19.0 { 22.0 } else { 16.0 }, col);
}

/// The app icon drawn live. On KDE it follows the colour scheme like the
/// installed SVG does (tile = window, frames = text, front = accent).
/// Elsewhere it is the mono icon the other platforms get.
fn paint_app_icon(painter: &egui::Painter, rect: Rect, pal: &theme::Palette) {
    let (bg, text, hl, hl_text) = if theme::scheme_is_dark().is_some() {
        (pal.window, pal.fg, pal.highlight, Color32::WHITE)
    } else {
        let (d, l) = (Color32::from_rgb(0x22, 0x26, 0x2c), Color32::from_rgb(0xe6, 0xeb, 0xef));
        (d, l, l, d)
    };
    let k = rect.width() / 512.0;
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * k;
    let r = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), vec2(w, h) * k);
    let rad = |v: f32| egui::CornerRadius::same((v * k).round().clamp(0.0, 255.0) as u8);
    let alpha = |c: Color32, a: f32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8);
    painter.rect_filled(r(16.0, 16.0, 480.0, 480.0), rad(112.0), bg);
    painter.rect_stroke(r(17.5, 17.5, 477.0, 477.0), rad(110.5), Stroke::new(3.0 * k, alpha(text, 0.14)), egui::StrokeKind::Middle);
    painter.rect_stroke(r(200.0, 88.0, 224.0, 192.0), rad(30.0), Stroke::new(18.0 * k, alpha(text, 0.45)), egui::StrokeKind::Middle);
    painter.rect_filled(r(152.0, 136.0, 248.0, 200.0), rad(30.0), bg);
    painter.rect_stroke(r(152.0, 136.0, 248.0, 200.0), rad(30.0), Stroke::new(18.0 * k, alpha(text, 0.7)), egui::StrokeKind::Middle);
    painter.rect_filled(r(88.0, 196.0, 272.0, 216.0), rad(32.0), hl);
    // the letters: round-capped strokes
    let w = 26.0 * k;
    let st = Stroke::new(w, hl_text);
    let line = |pts: Vec<Pos2>| {
        for p in [pts[0], *pts.last().unwrap()] {
            painter.circle_filled(p, w / 2.0, hl_text);
        }
        for p in &pts[1..pts.len() - 1] {
            painter.circle_filled(*p, w / 2.0, hl_text);
        }
        painter.add(egui::Shape::line(pts, st));
    };
    // G: arc from -58° anticlockwise round to 0°, then the bar inwards
    let mut g: Vec<Pos2> = (0..=40).map(|i| {
        let a = (-58.0f32 - 302.0 * i as f32 / 40.0).to_radians();
        at(158.0 + 36.0 * a.cos(), 304.0 + 36.0 * a.sin())
    }).collect();
    g.push(at(174.0, 304.0));
    line(g);
    line(vec![at(238.0, 268.0), at(238.0, 340.0)]);
    line(vec![at(282.0, 340.0), at(282.0, 268.0), at(326.0, 268.0)]);
    line(vec![at(282.0, 302.0), at(316.0, 302.0)]);
}

/// Open a folder in the system file manager.
fn open_in_file_manager(dir: &std::path::Path) {
    open_external(&dir.to_string_lossy());
}

const REPO_URL: &str = "https://github.com/zoeysmith37/GIFkit";

/// Open a URL or path with the system's default handler.
fn open_external(target: &str) {
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(target).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(target).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(target).spawn();
    let _ = r;
}

/// History label for a structural edit key like "delay3" or "dup12 4.5".
fn key_label(key: &str) -> String {
    let word: String = key.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    match word.as_str() {
        "reorder" => "Reorder layers".into(),
        "del" => "Delete".into(),
        "preset" => "Apply preset".into(),
        "addcensor" => "Add censor".into(),
        "delay" => "Frame delay".into(),
        "range" => "Skip / enable frames".into(),
        "loop" => "Loop count".into(),
        "dup" | "duplicate" => "Duplicate frame".into(),
        "skip" | "toggle" => "Skip / enable frame".into(),
        "left" | "right" | "move" => "Move frame".into(),
        "tl" => match key.trim_start_matches("tl-").chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().as_str() {
            "split" | "razor" => "Split clip".into(),
            "del" => "Delete clip".into(),
            "delkey" => "Delete keyframe".into(),
            "add" => "Add clip".into(),
            "key" | "keyedit" => "Keyframe".into(),
            "drag" => "Move clip".into(),
            "gifmove" => "Move frames".into(),
            "join" => "Join clips".into(),
            "reorder" => "Reorder layers".into(),
            "lengthen" => "Make longer".into(),
            "clearkeys" => "Clear keyframes".into(),
            "full" => "Show on every frame".into(),
            "marker" => "Marker".into(),
            "lock" => "Lock layer".into(),
            _ => "Timeline".into(),
        },
        "" => "Edit".into(),
        w => {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Names of the settings that differ between two versions of a layer.
fn changed_fields(a: &LayerKind, b: &LayerKind) -> String {
    let val = |k: &LayerKind| -> Option<serde_json::Value> {
        match k {
            LayerKind::Effects(x) => serde_json::to_value(&**x).ok(),
            LayerKind::Text(x) => serde_json::to_value(x).ok(),
            LayerKind::Censor(x) => serde_json::to_value(x).ok(),
            LayerKind::Watermark(x) => serde_json::to_value(&**x).ok(),
            LayerKind::Shield(x) => serde_json::to_value(x).ok(),
            LayerKind::Shape(x) => serde_json::to_value(&**x).ok(),
            LayerKind::Markup(x) => Some(serde_json::json!({ "strokes": x.strokes.len() })),
            LayerKind::Emoji(x) => serde_json::to_value(&**x).ok(),
            LayerKind::Overlay(_) => None,
        }
    };
    if let (LayerKind::Overlay(x), LayerKind::Overlay(y)) = (a, b) {
        return if x.opacity != y.opacity { "opacity".into() } else { "move / size".into() };
    }
    let (Some(serde_json::Value::Object(x)), Some(serde_json::Value::Object(y))) = (val(a), val(b)) else { return String::new() };
    let mut keys: Vec<String> = x.iter().filter(|(k, v)| y.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
    // group position fields
    if keys.iter().any(|k| k == "x" || k == "y") {
        keys.retain(|k| k != "x" && k != "y");
        keys.insert(0, "move".into());
    }
    if keys.iter().any(|k| k == "w" || k == "h") {
        keys.retain(|k| k != "w" && k != "h");
        keys.insert(0, "size".into());
    }
    let n = keys.len();
    let mut out: Vec<String> = keys.into_iter().take(2).map(|k| k.replace('_', " ")).collect();
    if n > 2 {
        out.push(format!("+{}", n - 2));
    }
    out.join(", ")
}

/// Short description of what changed between two live states.
fn describe_edit(os: &Stack, og: &GifOptions, ns: &Stack, ng: &GifOptions) -> String {
    use std::collections::HashMap;
    let old: HashMap<u64, &stack::Layer> = os.layers.iter().map(|l| (l.id, l)).collect();
    let new: HashMap<u64, &stack::Layer> = ns.layers.iter().map(|l| (l.id, l)).collect();
    let short = |l: &stack::Layer| elide(&l.name(), 24);
    let added: Vec<&&stack::Layer> = ns.layers.iter().filter(|l| !old.contains_key(&l.id)).map(|l| new.get(&l.id).unwrap()).collect();
    let removed: Vec<&stack::Layer> = os.layers.iter().filter(|l| !new.contains_key(&l.id)).collect();
    let mut parts: Vec<String> = Vec::new();
    for l in &added {
        parts.push(format!("Add {}", short(l)));
    }
    for l in &removed {
        parts.push(format!("Delete {}", short(l)));
    }
    let common_old: Vec<u64> = os.layers.iter().map(|l| l.id).filter(|id| new.contains_key(id)).collect();
    let common_new: Vec<u64> = ns.layers.iter().map(|l| l.id).filter(|id| old.contains_key(id)).collect();
    if common_old != common_new {
        parts.push("Reorder layers".into());
    }
    if added.is_empty() {
        for l in &ns.layers {
            if let Some(o) = old.get(&l.id) {
                if o.label != l.label {
                    parts.push(format!("Rename to {}", short(l)));
                } else if o.visible != l.visible {
                    parts.push(format!("{} {}", if l.visible { "Show" } else { "Hide" }, short(l)));
                } else if stack::layer_signature(o) != stack::layer_signature(l) {
                    let what = changed_fields(&o.kind, &l.kind);
                    let name = match l.kind() {
                        Kind::Effects => "Effects".to_string(),
                        Kind::Animation => "Animation".to_string(),
                        _ => short(l),
                    };
                    parts.push(if what.is_empty() { format!("Edit {name}") } else { format!("{name}: {what}") });
                }
            }
        }
    }
    let (a, b) = (&os.transform, &ns.transform);
    if a != b {
        let mut t: Vec<&str> = Vec::new();
        if (a.cut_on, a.cut_by_time, a.cut_start, a.cut_end, a.cut_t0.to_bits(), a.cut_t1.to_bits()) != (b.cut_on, b.cut_by_time, b.cut_start, b.cut_end, b.cut_t0.to_bits(), b.cut_t1.to_bits()) {
            t.push("Cut");
        }
        if (a.speed_on, a.speed_mode, a.speed_val.to_bits()) != (b.speed_on, b.speed_mode, b.speed_val.to_bits()) {
            t.push("Speed");
        }
        if (a.reverse, a.boomerang, a.set_loop, a.loop_count) != (b.reverse, b.boomerang, b.set_loop, b.loop_count) {
            t.push("Reverse / loop");
        }
        if (a.crop_on, a.crop) != (b.crop_on, b.crop) {
            t.push("Crop");
        }
        if (a.resize_on, a.rs_w, a.rs_h, a.rs_pct.to_bits(), a.rs_mode, a.rs_filter) != (b.resize_on, b.rs_w, b.rs_h, b.rs_pct.to_bits(), b.rs_mode, b.rs_filter) {
            t.push("Resize");
        }
        if t.is_empty() {
            t.push("Rotate / flip");
        }
        parts.push(t.join(", "));
    }
    if og != ng {
        parts.push("Compression".into());
    }
    match parts.len() {
        0 => "Edit".into(),
        1 | 2 => parts.join(" · "),
        n => format!("{} + {} more", parts[0], n - 1),
    }
}

fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::Effects => "fx",
        Kind::Animation => "anim",
        Kind::Text => "text",
        Kind::Censor => "censor",
        Kind::Overlay => "overlay",
        Kind::Watermark => "watermark",
        Kind::Shield => "shield",
        Kind::Shape => "shape",
        Kind::Markup => "markup",
        Kind::Emoji => "emoji",
    }
}

// ---------------------------------------------------------------- Breeze-ish widgets

/// A collapsible Breeze group box: clickable title row above a framed area.
fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnMut(&mut egui::Ui) -> R) -> Option<R> {
    group_ex(ui, title, true, None, add)
}

const GROUP_CMD: &str = "gf_group_cmd";

/// Collapse (false) or expand (true) every group on the next frame.
fn set_all_groups(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(GROUP_CMD), Some(open)));
}

fn clear_group_command(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Option<bool>>(egui::Id::unique(GROUP_CMD)));
}

/// Collapsible group with a default state and an optional badge (e.g. number of active options).
fn group_ex<R>(ui: &mut egui::Ui, title: &str, default_open: bool, badge: Option<String>, mut add: impl FnMut(&mut egui::Ui) -> R) -> Option<R> {
    // KDE settings pages are flat: a heading with a thin line under it, then the form
    let pal = *p();
    if title.is_empty() {
        let r = add(ui);
        ui.add_space(GROUP_GAP);
        return Some(r);
    }
    // searching: the group shows, open, if its title or anything in it matches
    let mut forced = false;
    let mut all = false;
    if find_on() {
        let probing = FIND.with(|f| f.borrow().probing);
        if probing {
            let all = find_matches(&group_search_text(title));
            if all {
                find_note(title);
            }
            return Some(find_scoped(Some(""), all, false, || add(ui)));
        }
        all = find_matches(&group_search_text(title));
        if !all {
            let (_, hit) = find_scoped(Some(""), false, false, || find_probe(ui, &mut add));
            if !hit {
                return None;
            }
        }
        forced = true;
    }
    let id = ui.make_persistent_id(("grp", title));
    let cmd: Option<bool> = ui.ctx().data(|d| d.get_temp::<Option<bool>>(egui::Id::unique(GROUP_CMD))).flatten();
    let mut open: bool = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    if let Some(c) = cmd {
        open = c;
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if resp.clicked() && !forced {
        open = !open;
    }
    ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    // open while it's a search result, without changing how it's left
    let open = open || forced;
    let painter = ui.painter();
    let col = if resp.hovered() { pal.fg } else { pal.fg_muted };
    crate::icons::paint(painter, if open { "go-down" } else { "go-next" }, pos2(rect.left() + 8.0, rect.center().y), 16.0, col);
    let galley = painter.layout_no_wrap(title.to_string(), theme::medium(14.0), pal.fg);
    let tw = galley.size().x;
    painter.galley(pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, pal.fg);
    if badge.is_some() {
        painter.circle_filled(pos2(rect.left() + 20.0 + tw + 8.0, rect.center().y), 3.0, pal.highlight);
    }
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, pal.border));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let _ = resp;
    let t = theme::openness(ui.ctx(), id.with("anim"), open);
    if t <= 0.0 {
        ui.add_space(6.0);
        return None;
    }
    let h_id = id.with("height");
    let mut body = |ui: &mut egui::Ui| if forced { find_scoped(Some(""), all, false, || add(ui)) } else { add(ui) };
    if t >= 1.0 {
        ui.add_space(8.0);
        let top = ui.cursor().top();
        let r = body(ui);
        let h = ui.cursor().top() - top;
        ui.ctx().data_mut(|d| d.insert_temp(h_id, h));
        ui.add_space(GROUP_GAP + 4.0);
        return Some(r);
    }
    // sliding open or shut: the body drawn whole but clipped to the part showing
    let full: f32 = ui.ctx().data(|d| d.get_temp(h_id)).unwrap_or(160.0);
    let total = 8.0 + full + GROUP_GAP + 4.0;
    let shown = (6.0 + (total - 6.0) * t).max(0.0);
    let w = ui.available_width();
    let at = ui.cursor().min;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(at, vec2(w, total.max(1.0) + 2000.0))).layout(*ui.layout()));
    child.set_clip_rect(Rect::from_min_size(at, vec2(w, shown)).intersect(ui.clip_rect()));
    child.add_space(8.0);
    let top = child.cursor().top();
    let r = body(&mut child);
    let h = child.cursor().top() - top;
    ui.ctx().data_mut(|d| d.insert_temp(h_id, h));
    ui.allocate_exact_size(vec2(w, shown), Sense::hover());
    ui.ctx().request_repaint();
    Some(r)
}

fn toolbar_sep(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.add(egui::Separator::default().vertical().spacing(8.0));
    ui.add_space(2.0);
}

/// Height of toolbar / transport controls.
const TOOLBAR_H: f32 = 30.0;
/// Height of quick-pick chips and choice buttons.
const CHIP_H: f32 = 30.0;
/// Space between the boxes of the inspector (Kirigami largeSpacing + a bit).
const GROUP_GAP: f32 = 12.0;

/// Right-click actions in the layer list.
#[derive(Clone, Copy)]
enum LayerAct {
    Rename(usize),
    RenameEdit(Tool),
    Duplicate(usize),
    Toggle(usize),
    Clip(usize),
    Move(usize, usize),
    Delete(usize),
    CollapseTo(usize),
    CollapseAll,
    CollapseEdits,
}

/// Breeze checked look: accent tint with an accent outline, not a solid pill.
fn sel_btn<'a>(b: egui::Button<'a>, on: bool) -> egui::Button<'a> {
    let pal = p();
    if on {
        b.fill(theme::checked_fill(pal)).stroke(Stroke::new(1.0, pal.highlight))
    } else {
        b
    }
}


/// A row of choices that share the width evenly (a segmented control).
fn seg_row<T: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut T, items: &[(T, &str)]) -> egui::Response {
    let mut out: Option<egui::Response> = None;
    let labels: Vec<&str> = items.iter().map(|i| i.1).collect();
    for l in &labels {
        find_note(l);
    }
    // rows wrap evenly rather than cutting labels short
    let min_w = label_w(ui, &labels);
    even_grid(ui, items.len(), min_w, |ui, i, size| {
        let (v, label) = items[i];
        // KDE uses radio buttons for one-of-several choices
        let mut r = cell(ui, size, |ui| radio(ui, *current == v, label));
        if r.clicked() && *current != v {
            *current = v;
            r.mark_changed();
        }
        out = Some(match out.take() {
            Some(o) => o | r,
            None => r,
        });
    });
    out.unwrap_or_else(|| ui.label(""))
}

/// Lay `n` controls out in rows that fill the width. Every cell is the same
/// size and rows are balanced, so no row ends up ragged or odd-sized.
/// `min_w` is the narrowest a cell may get (0 = all in one row).
fn even_grid(ui: &mut egui::Ui, n: usize, min_w: f32, mut add: impl FnMut(&mut egui::Ui, usize, Vec2)) {
    if n == 0 {
        return;
    }
    let gap = 4.0;
    let avail = ui.available_width();
    let max_cols = if min_w <= 0.0 { n } else { (((avail + gap) / (min_w + gap)).floor() as usize).clamp(1, n) };
    let rows = n.div_ceil(max_cols);
    let cols = n.div_ceil(rows);
    let w = ((avail - gap * (cols - 1) as f32) / cols as f32).floor().max(8.0);
    let size = vec2(w, CHIP_H);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        // a label too long for its cell is cut short instead of pushing the panel wider
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        for r in 0..rows {
            ui.horizontal(|ui| {
                for i in r * cols..((r + 1) * cols).min(n) {
                    add(ui, i, size);
                }
            });
        }
    });
}

/// Width a button needs for the longest of `labels`.
fn label_w(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let w = labels.iter().map(|l| ui.ctx().fonts_mut(|f| f.layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE).size().x)).fold(0.0, f32::max);
    // room for a button's padding, or a radio / checkbox indicator
    w + (2.0 * ui.spacing().button_padding.x).max(16.0 + 7.0) + 6.0
}

/// Toggle buttons in even rows.
fn toggle_grid(ui: &mut egui::Ui, min_w: f32, items: &mut [(&mut bool, &str, &str)]) {
    FIND.with(|f| f.borrow_mut().choice = true);
    // a search that matched some of these shows just those
    if FIND.with(|f| f.borrow().items) {
        let total = items.len();
        let mut some: Vec<(&mut bool, &str, &str)> = items.iter_mut().filter(|i| find_matches(i.1)).map(|(b, l, t)| (&mut **b, *l, *t)).collect();
        if !some.is_empty() && some.len() < total {
            return find_scoped(None, false, false, || toggle_grid(ui, min_w, &mut some));
        }
    }
    let n = items.len();
    let labels: Vec<&str> = items.iter().map(|i| i.1).collect();
    let min_w = min_w.max(label_w(ui, &labels));
    even_grid(ui, n, min_w, |ui, i, size| {
        let (v, label, tip) = &mut items[i];
        // on/off settings are checkboxes in KDE
        let r = cell(ui, size, |ui| check(ui, v, *label));
        if !tip.is_empty() {
            r.on_hover_text(*tip);
        }
    });
}

/// Colour button (KDE KColorButton style: a framed button with a swatch).
/// Its popup has the colour picker, a hex field and an eyedropper.
fn colour_btn(ui: &mut egui::Ui, c: &mut [u8; 3]) -> egui::Response {
    let mut rgba = [c[0], c[1], c[2], 255];
    let r = colour_edit(ui, &mut rgba, false);
    *c = [rgba[0], rgba[1], rgba[2]];
    r
}

fn colour_btn_a(ui: &mut egui::Ui, c: &mut [u8; 4]) -> egui::Response {
    colour_edit(ui, c, true)
}

/// An eyedropper waiting for a click: which colour button it fills in, and
/// the window's pixels to pick from once they arrive.
struct Eyedrop {
    target: egui::Id,
    since: u64,
    asked: bool,
    shot: Option<Arc<egui::ColorImage>>,
}

static EYEDROP: std::sync::Mutex<Option<Eyedrop>> = std::sync::Mutex::new(None);

fn eyedrop_picked_id(id: egui::Id) -> egui::Id {
    id.with("eyedrop-picked")
}

fn colour_edit(ui: &mut egui::Ui, c: &mut [u8; 4], alpha: bool) -> egui::Response {
    let pal = *p();
    let mut resp = ui.add(egui::Button::new("").min_size(vec2(40.0, CHIP_H)));
    // a colour picked from the screen for this button
    if let Some(px) = ui.ctx().data_mut(|d| d.remove_temp::<[u8; 3]>(eyedrop_picked_id(resp.id))) {
        c[0] = px[0];
        c[1] = px[1];
        c[2] = px[2];
        resp.mark_changed();
    }
    let sw = resp.rect.shrink2(vec2(6.0, 5.0));
    if c[3] < 255 {
        // checks under see-through colours
        let half = Rect::from_min_max(sw.min, pos2(sw.center().x, sw.max.y));
        ui.painter().rect_filled(sw, 2.0, Color32::WHITE);
        ui.painter().rect_filled(half, 2.0, Color32::from_gray(190));
    }
    ui.painter().rect_filled(sw, 2.0, Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]));
    ui.painter().rect_stroke(sw, 2.0, Stroke::new(1.0, mix(pal.fg, pal.button, 0.6)), egui::StrokeKind::Inside);
    let mut pick = false;
    egui::Popup::from_toggle_button_response(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.spacing_mut().interact_size.x = 44.0;
        ui.spacing_mut().slider_width = 240.0;
        ui.set_width(240.0);
        let mut col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        if egui::color_picker::color_picker_color32(ui, &mut col, if alpha { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque }) {
            *c = col.to_srgba_unmultiplied();
            resp.mark_changed();
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let mut hex = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
            let te = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(84.0).font(egui::TextStyle::Monospace));
            if te.changed() {
                if let Some(v) = parse_hex(&hex) {
                    c[0] = v[0];
                    c[1] = v[1];
                    c[2] = v[2];
                    resp.mark_changed();
                }
            }
            let b = egui::Button::image_and_text(crate::icons::image(ui.ctx(), "color-picker", 16.0, pal.fg), "Pick from Screen");
            if ui.add_sized(vec2(ui.available_width(), CHIP_H), b).on_hover_text("Click anywhere in the window to take its colour. Esc cancels.").clicked() {
                pick = true;
                ui.close();
            }
        });
    });
    if pick {
        let frame = ui.ctx().cumulative_frame_nr();
        if let Ok(mut e) = EYEDROP.lock() {
            *e = Some(Eyedrop { target: resp.id, since: frame, asked: false, shot: None });
        }
        ui.ctx().request_repaint();
    }
    resp
}

/// Runs the eyedropper while one is active: takes a picture of the window,
/// shows a magnifier at the pointer and hands the clicked colour back.
fn eyedropper(ctx: &egui::Context) {
    let Ok(mut guard) = EYEDROP.lock() else { return };
    let Some(e) = guard.as_mut() else { return };
    let frame = ctx.cumulative_frame_nr();
    if !e.asked && frame > e.since + 1 {
        // a frame later, so the colour popup is gone from the picture
        e.asked = true;
        let c2 = ctx.clone();
        ctx.request_screenshot(move |img| {
            if let Ok(mut g) = EYEDROP.lock() {
                if let Some(e) = g.as_mut() {
                    e.shot = Some(img);
                }
            }
            c2.request_repaint();
        });
    }
    ctx.request_repaint();
    let pal = *p();
    let screen = ctx.content_rect();
    let mut done: Option<Option<[u8; 3]>> = None;
    egui::Area::new(egui::Id::unique("gifkit-eyedropper")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::Crosshair);
        let ppp = ctx.pixels_per_point();
        let Some(shot) = e.shot.clone() else {
            ui.painter().text(r.center_top() + vec2(0.0, 40.0), egui::Align2::CENTER_CENTER, "Getting ready…", theme::medium(14.0), pal.fg);
            return;
        };
        let at = |pos: Pos2| -> Option<[u8; 3]> {
            let x = ((pos.x - screen.min.x) * ppp) as i64;
            let y = ((pos.y - screen.min.y) * ppp) as i64;
            let [w, h] = shot.size;
            (x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h).then(|| {
                let c = shot.pixels[y as usize * w + x as usize];
                [c.r(), c.g(), c.b()]
            })
        };
        if let Some(pos) = ctx.pointer_hover_pos() {
            // magnifier: 11×11 screen pixels, the middle one outlined
            const N: i64 = 11;
            const CELL: f32 = 9.0;
            let size = N as f32 * CELL;
            let mut origin = pos + vec2(18.0, 18.0);
            if origin.x + size + 4.0 > screen.max.x {
                origin.x = pos.x - 18.0 - size;
            }
            if origin.y + size + 30.0 > screen.max.y {
                origin.y = pos.y - 18.0 - size - 26.0;
            }
            let painter = ui.painter();
            let frame = Rect::from_min_size(origin, vec2(size, size));
            painter.rect_filled(frame.expand(3.0), theme::RADIUS, pal.window);
            for j in 0..N {
                for i in 0..N {
                    let p = pos + vec2((i - N / 2) as f32, (j - N / 2) as f32) / ppp;
                    let c = at(p).unwrap_or([0, 0, 0]);
                    let cr = Rect::from_min_size(origin + vec2(i as f32 * CELL, j as f32 * CELL), vec2(CELL, CELL));
                    painter.rect_filled(cr, 0.0, Color32::from_rgb(c[0], c[1], c[2]));
                }
            }
            let mid = Rect::from_min_size(origin + vec2((N / 2) as f32 * CELL, (N / 2) as f32 * CELL), vec2(CELL, CELL));
            painter.rect_stroke(mid, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
            painter.rect_stroke(mid.expand(1.5), 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
            painter.rect_stroke(frame.expand(3.0), theme::RADIUS, Stroke::new(1.0, pal.border_strong), egui::StrokeKind::Inside);
            if let Some(c) = at(pos) {
                let label = Rect::from_min_size(pos2(frame.min.x - 3.0, frame.max.y + 5.0), vec2(size + 6.0, 22.0));
                painter.rect_filled(label, theme::RADIUS, pal.window);
                painter.rect_filled(Rect::from_min_size(label.min + vec2(5.0, 5.0), vec2(12.0, 12.0)), 2.0, Color32::from_rgb(c[0], c[1], c[2]));
                painter.text(label.left_center() + vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]), egui::FontId::monospace(12.0), pal.fg);
            }
            if resp.clicked() {
                done = Some(at(pos));
            }
        }
        if resp.secondary_clicked() {
            done = Some(None);
        }
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        done = Some(None);
    }
    if let Some(res) = done {
        let target = e.target;
        *guard = None;
        if let Some(c) = res {
            ctx.data_mut(|d| d.insert_temp(eyedrop_picked_id(target), c));
        }
    }
}

/// Width of the label column in inspector forms.
const FORM_LABEL_W: f32 = 86.0;

/// One row of a KDE-style form: a right-aligned label in a fixed column on the
/// left and the controls in a column that fills the rest. The controls get a
/// vertical ui of a fixed width, so nothing in them can widen the panel.
// ------------------------------------------------------------ panel search
//
// The search box over the tool panel filters its rows as they're drawn. A
// row shows when its label matches, when the heading it sits under matches
// (a group title or an effect's own sub-heading, so "squeeze" brings its
// strength, reverse and centre along), or when anything inside it does (a
// checkbox in a grid, say). For that last case the row is drawn once out of
// sight to see what it holds, then for real if something matched.

#[derive(Default)]
struct Find {
    /// lower case; empty: not searching
    q: String,
    /// drawing out of sight to see what matches
    probing: bool,
    hit: bool,
    /// the sub-heading rows sit under
    ctx: String,
    /// the group's title matched: everything in it shows
    group_all: bool,
    /// grids show only their matching items
    items: bool,
    /// rows shown for the search so far
    shown: usize,
    /// a row in this group matched by something in it (a checkbox, say):
    /// the settings after it go with it
    follow: bool,
    /// the row being probed is a set of on/off choices
    choice: bool,
}

thread_local! {
    static FIND: std::cell::RefCell<Find> = std::cell::RefCell::new(Find::default());
}

fn find_on() -> bool {
    FIND.with(|f| !f.borrow().q.is_empty())
}

/// A group's title, and words people search for that it covers.
fn group_search_text(title: &str) -> String {
    let more = match title {
        "Transparency" => " background remove green screen chroma key spill cut out",
        _ => "",
    };
    format!("{title}{more}")
}

fn find_matches(text: &str) -> bool {
    FIND.with(|f| {
        let f = f.borrow();
        !f.q.is_empty() && text.to_lowercase().contains(&f.q)
    })
}

/// A piece of text in what's being drawn: while probing, a match counts.
fn find_note(text: &str) {
    if find_matches(text) {
        FIND.with(|f| f.borrow_mut().hit = true);
    }
}

/// Is a row with this label shown as it is (by its label or what it sits under)?
fn find_row_shown(label: &str) -> bool {
    let (on, probing, ctx, all) = FIND.with(|f| {
        let f = f.borrow();
        (!f.q.is_empty(), f.probing, f.ctx.clone(), f.group_all)
    });
    if !on {
        return true;
    }
    let by = find_matches(label) || (!ctx.is_empty() && find_matches(&ctx)) || all;
    if probing {
        if by {
            FIND.with(|f| f.borrow_mut().hit = true);
        }
        return true;
    }
    by
}

/// A row shown for real while searching.
fn find_count() {
    FIND.with(|f| {
        let mut f = f.borrow_mut();
        if !f.q.is_empty() && !f.probing {
            f.shown += 1;
        }
    });
}

/// Draw without taking any room or showing anything.
fn find_hidden<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width().max(1.0), 2000.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()).invisible());
    add(&mut child)
}

/// Draw out of sight and report whether anything in it matched.
fn find_probe<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    let (r, hit, _) = find_probe_choice(ui, add);
    (r, hit)
}

/// `find_probe`, also saying whether what was drawn is a grid of on/off choices.
fn find_probe_choice<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool, bool) {
    let (was_p, was_h, was_c) = FIND.with(|f| {
        let mut f = f.borrow_mut();
        let w = (f.probing, f.hit, f.choice);
        f.probing = true;
        f.hit = false;
        f.choice = false;
        w
    });
    let r = find_hidden(ui, add);
    let (hit, choice) = FIND.with(|f| {
        let mut f = f.borrow_mut();
        let h = (f.hit, f.choice);
        f.probing = was_p;
        f.hit = was_h || h.0;
        f.choice = was_c;
        h
    });
    (r, hit, choice)
}

fn find_follow() -> bool {
    FIND.with(|f| {
        let f = f.borrow();
        !f.q.is_empty() && !f.probing && f.follow
    })
}

/// Run `add` with the search's per-group state set, then put it back.
fn find_scoped<R>(ctx: Option<&str>, all: bool, items: bool, add: impl FnOnce() -> R) -> R {
    let old = FIND.with(|f| {
        let mut f = f.borrow_mut();
        let old = (f.ctx.clone(), f.group_all, f.items, f.follow);
        if let Some(c) = ctx {
            // a new group or heading: nothing to follow yet
            f.ctx = c.to_string();
            f.follow = false;
        }
        f.group_all = all;
        f.items = items;
        old
    });
    let r = add();
    FIND.with(|f| {
        let mut f = f.borrow_mut();
        (f.ctx, f.group_all, f.items, f.follow) = old;
    });
    r
}

/// A search field with Breeze's find icon and a clear button.
fn search_box(ui: &mut egui::Ui, text: &mut String, hint_text: &str, id: egui::Id) -> egui::Response {
    let pal = *p();
    let r = ui.add(egui::TextEdit::singleline(text).id(id).hint_text(hint_text).margin(egui::Margin { left: 26, right: 26, top: 4, bottom: 4 }).desired_width(ui.available_width()));
    crate::icons::paint(ui.painter(), "edit-find", pos2(r.rect.left() + 13.0, r.rect.center().y), 16.0, pal.fg_muted);
    if !text.is_empty() {
        let cr = Rect::from_center_size(pos2(r.rect.right() - 13.0, r.rect.center().y), vec2(20.0, 20.0));
        let c = ui.interact(cr, r.id.with("clear"), Sense::click()).on_hover_text("Clear");
        crate::icons::paint(ui.painter(), "edit-clear", cr.center(), 16.0, if c.hovered() { pal.fg } else { pal.fg_muted });
        if c.clicked() {
            text.clear();
        }
    }
    r
}

/// A form row: the label in its column, the controls beside it.
fn form_row<R>(ui: &mut egui::Ui, label: &str, mut add: impl FnMut(&mut egui::Ui) -> R) -> R {
    if find_row_shown(label) {
        find_count();
        return form_row_raw(ui, label, &mut add);
    }
    // not by its label: see whether anything in it matches
    let (r, hit, choice) = find_probe_choice(ui, |ui| form_row_raw(ui, label, &mut add));
    if !hit {
        // after a match in this group its settings come along, but not
        // other sets of choices
        if find_follow() && !choice {
            find_count();
            return form_row_raw(ui, label, &mut add);
        }
        return r;
    }
    let all = FIND.with(|f| f.borrow().group_all);
    find_count();
    let r = find_scoped(None, all, true, || form_row_raw(ui, label, &mut add));
    FIND.with(|f| f.borrow_mut().follow = true);
    r
}

/// A form row shown only by its label (its controls can only be drawn once).
fn form_row_once<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if find_row_shown(label) || find_follow() {
        find_count();
        form_row_raw(ui, label, add)
    } else {
        find_hidden(ui, |ui| form_row_raw(ui, label, add))
    }
}

fn form_row_raw<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(FORM_LABEL_W, CHIP_H), Sense::hover());
        if !label.is_empty() {
            // KDE form labels end with a colon
            let text = if label.ends_with(':') { label.to_string() } else { format!("{label}:") };
            let col = if ui.is_enabled() { p().fg } else { p().fg_muted };
            ui.painter().text(r.right_center() - vec2(2.0, 0.0), egui::Align2::RIGHT_CENTER, text, egui::TextStyle::Body.resolve(ui.style()), col);
        }
        let w = (ui.available_width() - 1.0).max(40.0);
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.set_max_width(w);
            add(ui)
        })
        .inner
    })
    .inner
}

/// Run `add` in a cell of exactly `size`, controls centred vertically.
fn cell<R>(ui: &mut egui::Ui, size: Vec2, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.set_max_width(size.x);
        add(ui)
    })
    .inner
}

/// A number box that fills the rest of the row.
fn num_fill<'a, N: egui::emath::Numeric>(ui: &mut egui::Ui, v: &'a mut N, step: f64, f: impl FnOnce(egui::DragValue<'a>) -> egui::DragValue<'a>) -> egui::Response {
    let size = vec2(ui.available_width(), CHIP_H);
    spin(ui, size, v, step, f)
}

/// A KDE spin box: a field with up and down arrows on the right. The value can
/// also be dragged or typed as egui allows.
fn spin<'a, N: egui::emath::Numeric>(ui: &mut egui::Ui, size: Vec2, v: &'a mut N, step: f64, f: impl FnOnce(egui::DragValue<'a>) -> egui::DragValue<'a>) -> egui::Response {
    let pal = *p();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let enabled = ui.is_enabled();
    // the frame goes under everything, filled in once we know the state
    let bg = ui.painter().add(egui::Shape::Noop);
    let arrows_w = if rect.width() >= 64.0 { 18.0 } else { 0.0 };
    let field = Rect::from_min_max(rect.min, pos2(rect.max.x - arrows_w, rect.max.y));
    let arr = Rect::from_min_max(pos2(field.max.x, rect.min.y), rect.max);
    // arrows first, so the box shows the new value this frame
    let mut stepped = false;
    if arrows_w > 0.0 {
        let id = ui.next_auto_id().with("spin");
        let halves = [(1.0, Rect::from_min_max(arr.min, pos2(arr.max.x, arr.center().y))), (-1.0, Rect::from_min_max(pos2(arr.min.x, arr.center().y), arr.max))];
        for (k, half) in halves {
            let r = ui.interact(half, id.with(k as i32), Sense::click());
            if enabled && r.clicked() {
                *v = N::from_f64(v.to_f64() + k * step);
                stepped = true;
            }
            if enabled && r.hovered() {
                ui.painter().rect_filled(half.shrink(2.0), 2.0, mix(pal.view, pal.highlight, 0.2));
            }
        }
    }
    // a child that takes no room of its own: the space is already allocated above
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Min)));
    let w = &mut child.visuals_mut().widgets;
    for st in [&mut w.inactive, &mut w.hovered, &mut w.active] {
        st.bg_fill = Color32::TRANSPARENT;
        st.weak_bg_fill = Color32::TRANSPARENT;
        st.bg_stroke = Stroke::NONE;
        st.expansion = 0.0;
    }
    // the value sits on the left, as in a Qt spin box
    child.spacing_mut().interact_size = field.size();
    let mut resp = child.add(f(egui::DragValue::new(v)));
    let hovered = enabled && ui.rect_contains_pointer(rect);
    let focus = resp.has_focus() || resp.dragged();
    let stroke = Stroke::new(1.0, if focus || hovered { pal.highlight } else { pal.border });
    ui.painter().set(bg, egui::Shape::rect_filled(rect, 3.0, if enabled { pal.view } else { pal.window }));
    ui.painter().rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    if arrows_w > 0.0 {
        let col = if enabled { pal.fg } else { pal.fg_muted };
        let c = arr.center() - vec2(1.0, 0.0);
        let s = 3.0;
        for (dy, up) in [(-4.0, true), (4.0, false)] {
            let y = c.y + dy;
            let t = if up { s * 0.5 } else { -s * 0.5 };
            ui.painter().add(egui::Shape::line(vec![pos2(c.x - s, y + t), pos2(c.x, y - t), pos2(c.x + s, y + t)], Stroke::new(1.2, col)));
        }
    }
    if stepped {
        resp.mark_changed();
    }
    resp
}

/// A button that fills the width of the field column.
/// X and Y rows for an object `obj_h` tall, Y shown up from the bottom of a
/// `frame_h` frame to the object's bottom edge (stored as a top edge, down).
fn pos_rows(ui: &mut egui::Ui, x: &mut i32, y: &mut i32, obj_h: i32, frame_h: u32) {
    form_row(ui, "Position", |ui| {
        even_grid(ui, 2, 0.0, |ui, i, size| {
            if i == 0 {
                spin(ui, size, x, 1.0, |d| d.prefix("X  ")).on_hover_text("Left edge, from the left of the frame");
            } else {
                let mut up = frame_h as i32 - (*y + obj_h);
                if spin(ui, size, &mut up, 1.0, |d| d.prefix("Y  ")).on_hover_text("Bottom edge, up from the bottom of the frame").changed() {
                    *y = frame_h as i32 - obj_h - up;
                }
            }
        })
    });
}

fn wide_button(ui: &mut egui::Ui, b: egui::Button<'_>) -> egui::Response {
    ui.add_sized(vec2(ui.available_width(), CHIP_H), b)
}

/// Slider that fills the row, with its value box at a fixed width.
fn fill_slider(ui: &mut egui::Ui, s: egui::Slider<'_>) -> egui::Response {
    ui.spacing_mut().interact_size.x = 64.0;
    let w = ui.available_width();
    ui.spacing_mut().slider_width = (w - 64.0 - ui.spacing().item_spacing.x - 2.0).max(40.0);
    ui.horizontal(|ui| slider(ui, s)).inner
}

/// A labelled slider row.
fn slider_row(ui: &mut egui::Ui, label: &str, s: egui::Slider<'_>) -> egui::Response {
    form_row_once(ui, label, |ui| fill_slider(ui, s))
}

/// Drop shadow switch and its look, greyed out while it's off.
fn shadow_rows(ui: &mut egui::Ui, on: &mut bool, sh: &mut crate::shapes::Shadow) {
    form_row(ui, "Shadow", |ui| check(ui, on, "Drop shadow"));
    ui.add_enabled_ui(*on, |ui| {
        slider_row(ui, "Angle", soft_slider(&mut sh.angle, 0.0..=360.0, "").suffix("°").max_decimals(0)).on_hover_text("Direction the shadow falls. 45° is down and to the right.");
        slider_row(ui, "Distance", soft_slider(&mut sh.distance, 0.0..=30.0, "").suffix("%").max_decimals(1)).on_hover_text("How far the shadow sits from the shape, as a share of its size");
        slider_row(ui, "Blur", egui::Slider::new(&mut sh.blur, 0.0..=20.0).suffix("%").max_decimals(1)).on_hover_text("Softness, as a share of the shape's size");
        slider_row(ui, "Strength", egui::Slider::new(&mut sh.opacity, 0.0..=100.0).suffix("%").max_decimals(0));
    });
}

/// A labelled row with an on/off checkbox; the rest stays in place and greys out when off.
fn opt_row<R>(ui: &mut egui::Ui, label: &str, on: &mut bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // a switch of its own: shown by its own name, not as a setting of what's above
    if !find_row_shown(label) {
        return find_hidden(ui, |ui| form_row_raw(ui, label, |ui| ui.horizontal(|ui| {
            check(ui, on, "");
            ui.add_enabled_ui(false, add).inner
        }).inner));
    }
    find_count();
    form_row_raw(ui, label, |ui| {
        ui.horizontal(|ui| {
            check(ui, on, "");
            let en = *on;
            ui.add_enabled_ui(en, add).inner
        })
        .inner
    })
}

/// A button that opens a menu: its text, then Breeze's down chevron on the right.
fn drop_button<'a>(ui: &egui::Ui, text: impl Into<egui::WidgetText>, col: Color32) -> egui::Button<'a> {
    egui::Button::new(text.into()).right_text(crate::icons::image(ui.ctx(), "go-down", 16.0, col))
}

/// Breeze's thin down chevron for combo boxes, in place of egui's triangle.
fn chevron(ui: &egui::Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let c = rect.center();
    let col = visuals.fg_stroke.color;
    let s = 3.5;
    ui.painter().add(egui::Shape::line(vec![c + vec2(-s, -s * 0.5), c + vec2(0.0, s * 0.5), c + vec2(s, -s * 0.5)], Stroke::new(1.3, col)));
}

/// A full-width combo box in the field column.
fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, current: &mut T, items: &[(T, &str)]) -> egui::Response {
    let text = items.iter().find(|i| i.0 == *current).map(|i| i.1).unwrap_or("");
    let w = ui.available_width();
    egui::ComboBox::from_id_salt(id)
        .width(w.max(40.0))
        .icon(chevron)
        .truncate()
        .selected_text(text)
        .show_ui(ui, |ui| {
            for (v, l) in items {
                ui.selectable_value(current, *v, *l);
            }
        })
        .response
}


/// Breeze radio button: an accent ring with a dot when chosen.
fn radio(ui: &mut egui::Ui, on: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let pal = *p();
    let text: egui::WidgetText = text.into();
    find_note(text.text());
    let d = 16.0;
    let gap = 7.0;
    let wrap_w = (ui.available_width() - d - gap).max(10.0);
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Truncate), wrap_w, egui::TextStyle::Body);
    let size = vec2(d + gap + galley.size().x, galley.size().y.max(d).max(22.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let en = ui.is_enabled();
        let c = pos2(rect.left() + d / 2.0, rect.center().y);
        let hov = resp.hovered() && en;
        let painter = ui.painter();
        if on {
            let fill = if en { pal.highlight } else { mix(pal.button, pal.highlight, 0.4) };
            painter.circle_filled(c, d / 2.0, fill);
            painter.circle_filled(c, 3.2, Color32::WHITE);
        } else {
            painter.circle_filled(c, d / 2.0, pal.view);
            painter.circle_stroke(c, d / 2.0 - 0.5, Stroke::new(1.0, if hov { pal.highlight } else { theme::outline(&pal) }));
        }
        let col = if en { pal.fg } else { pal.fg_muted };
        painter.galley(pos2(c.x + d / 2.0 + gap, rect.center().y - galley.size().y / 2.0), galley, col);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), on, ""));
    resp
}

/// Breeze checkbox: accent fill with a white tick when on.
fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let pal = *p();
    let text: egui::WidgetText = text.into();
    find_note(text.text());
    let box_s = 16.0;
    let gap = 7.0;
    // long labels are cut short rather than pushing the panel wider
    let wrap_w = (ui.available_width() - box_s - gap).max(10.0);
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Truncate), wrap_w, egui::TextStyle::Body);
    // on its own in a form row it's as tall as the row, so it lines up with
    // the row's label; in a row of other controls those centre it
    let min_h = if ui.layout().is_horizontal() { 22.0 } else { CHIP_H };
    let size = vec2(box_s + gap + galley.size().x, galley.size().y.max(box_s).max(min_h));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let en = ui.is_enabled();
        let b = Rect::from_center_size(pos2(rect.left() + box_s / 2.0, rect.center().y), vec2(box_s, box_s));
        let hov = resp.hovered() && en;
        let painter = ui.painter();
        if *on {
            let fill = if en { pal.highlight } else { mix(pal.button, pal.highlight, 0.4) };
            painter.rect_filled(b, 3.0, fill);
            let st = Stroke::new(1.8, Color32::WHITE);
            let pts = vec![b.left_top() + vec2(3.8, 8.4), b.left_top() + vec2(6.8, 11.3), b.left_top() + vec2(12.3, 5.0)];
            painter.add(egui::Shape::line(pts, st));
        } else {
            painter.rect_filled(b, 3.0, pal.view);
            painter.rect_stroke(b, 3.0, Stroke::new(1.0, if hov { pal.highlight } else { theme::outline(&pal) }), egui::StrokeKind::Inside);
        }
        if hov && *on {
            painter.rect_stroke(b.expand(1.0), 4.0, Stroke::new(1.0, mix(pal.highlight, pal.fg, 0.3)), egui::StrokeKind::Inside);
        }
        let col = if en { pal.fg } else { pal.fg_muted };
        painter.galley(pos2(b.right() + gap, rect.center().y - galley.size().y / 2.0), galley, col);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    resp
}

/// Slider with the Breeze look: thin groove, solid accent fill, round handle.
fn slider(ui: &mut egui::Ui, s: egui::Slider<'_>) -> egui::Response {
    let pal = *p();
    ui.scope(|ui| {
        let enabled = ui.is_enabled();
        let v = ui.visuals_mut();
        let groove = mix(pal.button, pal.fg, 0.16);
        if enabled {
            v.selection.bg_fill = pal.highlight;
            v.widgets.inactive.bg_fill = groove;
            v.widgets.hovered.bg_fill = mix(groove, pal.fg, 0.1);
            v.widgets.active.bg_fill = mix(groove, pal.highlight, 0.4);
        } else {
            // greyed out: solid, darker colours rather than see-through ones,
            // so the handle never shows the groove through it
            let groove = mix(pal.button, pal.fg, 0.07);
            let handle = mix(pal.button, pal.window, 0.5);
            v.selection.bg_fill = mix(groove, pal.highlight, 0.35);
            for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
                w.bg_fill = handle;
                w.weak_bg_fill = mix(pal.button, pal.window, 0.4);
                w.fg_stroke = Stroke::new(1.0, pal.fg_muted);
                w.bg_stroke = Stroke::new(1.0, mix(theme::outline(&pal), pal.window, 0.4));
            }
            v.widgets.noninteractive.bg_fill = groove;
            v.widgets.inactive.bg_fill = groove;
        }
        if !enabled {
            ui.set_opacity(1.0);
        }
        // egui sizes the handle from the row height; Breeze's is smaller
        ui.spacing_mut().interact_size.y = 22.0;
        ui.add(s)
    })
    .inner
}

/// Flat toolbar button with a Breeze icon (frame only on hover, as in KDE toolbars).
fn icon_tool(ui: &mut egui::Ui, icon: &str, tip: &str, enabled: bool) -> egui::Response {
    let pal = *p();
    let r = ui.add_enabled(enabled, egui::Button::new("").frame_when_inactive(false).min_size(vec2(TOOLBAR_H, TOOLBAR_H)));
    crate::icons::paint(ui.painter(), icon, r.rect.center(), 16.0, if enabled { pal.fg } else { mix(pal.window, pal.fg, 0.35) });
    if tip.is_empty() { r } else { r.on_hover_text(tip) }
}

/// Main toolbar button: KDE's toolbar icon size, and text beside the icon
/// for the main actions when the desktop asks for it.
fn tb_tool(ui: &mut egui::Ui, icon: &str, text: &str, tip: &str, enabled: bool) -> egui::Response {
    let pal = *p();
    let (with_text, size) = theme::toolbar_style();
    let col = if enabled { pal.fg } else { mix(pal.window, pal.fg, 0.35) };
    let img = crate::icons::image(ui.ctx(), icon, size, col);
    let b = if with_text && !text.is_empty() { egui::Button::image_and_text(img, RichText::new(text).color(col)) } else { egui::Button::image(img) };
    let r = ui.add_enabled(enabled, b.frame_when_inactive(false).min_size(vec2(size + 10.0, size + 10.0)));
    if tip.is_empty() { r } else { r.on_hover_text(tip) }
}

/// Main toolbar toggle (checked look when on).
fn tb_toggle(ui: &mut egui::Ui, icon: &str, on: bool, tip: &str) -> bool {
    let pal = *p();
    let (_, size) = theme::toolbar_style();
    let col = if ui.is_enabled() { pal.fg } else { mix(pal.window, pal.fg, 0.35) };
    let b = egui::Button::image(crate::icons::image(ui.ctx(), icon, size, col)).frame_when_inactive(on).min_size(vec2(size + 10.0, size + 10.0));
    ui.add(sel_btn(b, on)).on_hover_text(tip).clicked()
}

/// A menu entry with a Breeze icon and an optional shortcut.
fn menu_item(ui: &mut egui::Ui, icon: &str, text: &str, shortcut: &str, enabled: bool) -> bool {
    menu_item_tip(ui, icon, text, shortcut, enabled, "")
}

/// A menu entry with a tooltip (Open Recent shows the full path).
fn menu_item_tip(ui: &mut egui::Ui, icon: &str, text: &str, shortcut: &str, enabled: bool, tip: &str) -> bool {
    let pal = *p();
    let col = if enabled { pal.fg } else { pal.fg_muted };
    let img = crate::icons::image(ui.ctx(), if icon.is_empty() { "list-add" } else { icon }, 16.0, col);
    let img = if icon.is_empty() { img.tint(Color32::TRANSPARENT) } else { img };
    let (job, key) = mnemonic(ui, text, col);
    // Breeze menu rows are taller, with more room after the icon
    let mut b = egui::Button::image_and_text(img, job).min_size(vec2(0.0, 28.0)).gap(10.0);
    if !shortcut.is_empty() {
        b = b.shortcut_text(RichText::new(shortcut).color(pal.fg_muted));
    }
    let mut r = ui.add_enabled(enabled, b);
    if !tip.is_empty() {
        r = r.on_hover_text(tip);
    }
    if r.clicked() || (enabled && mnemonic_pressed(ui, key)) {
        ui.close();
        true
    } else {
        false
    }
}

/// A checkable menu entry: a tick in the icon column when on.
fn menu_check(ui: &mut egui::Ui, on: bool, text: &str, shortcut: &str) -> bool {
    let pal = *p();
    let img = crate::icons::image(ui.ctx(), "dialog-ok-apply", 16.0, pal.fg);
    let img = if on { img } else { img.tint(Color32::TRANSPARENT) };
    let (job, key) = mnemonic(ui, text, pal.fg);
    let mut b = egui::Button::image_and_text(img, job).min_size(vec2(0.0, 28.0)).gap(10.0);
    if !shortcut.is_empty() {
        b = b.shortcut_text(RichText::new(shortcut).color(pal.fg_muted));
    }
    if ui.add(b).clicked() || mnemonic_pressed(ui, key) {
        ui.close();
        true
    } else {
        false
    }
}

/// A submenu whose title lines up with the icon column, with Breeze's thin
/// chevron on the right.
fn sub_menu(ui: &mut egui::Ui, icon: &str, text: &str, add: impl FnOnce(&mut egui::Ui)) {
    let pal = *p();
    let img = crate::icons::image(ui.ctx(), if icon.is_empty() { "list-add" } else { icon }, 16.0, pal.fg);
    let img = if icon.is_empty() { img.tint(Color32::TRANSPARENT) } else { img };
    let (job, _) = mnemonic(ui, text, pal.fg);
    let b = egui::Button::image_and_text(img, job).min_size(vec2(0.0, 28.0)).gap(10.0).right_text(crate::icons::image(ui.ctx(), "go-next", 16.0, pal.fg));
    egui::containers::menu::SubMenuButton::from_button(b).ui(ui, add);
}

/// A top-level menu in the menu bar. Alt plus its underlined letter opens it.
fn top_menu(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let (job, key) = mnemonic(ui, title, p().fg);
    // the popup id egui gives the button's menu, known before the button is added
    let popup = ui.next_auto_id().with("popup");
    if let Some(k) = key {
        if ui.input(|i| i.modifiers.alt && !i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(k)) {
            egui::Popup::open_id(ui.ctx(), popup);
            ui.ctx().data_mut(|d| d.insert_temp(mnemonics_id(), true));
        }
    }
    egui::containers::menu::MenuButton::from_button(egui::Button::new(job)).ui(ui, add);
}

fn mnemonics_id() -> egui::Id {
    egui::Id::unique("mnemonics-kbd")
}

/// Whether menus show their underlined letters: while Alt is held, or after a
/// menu was opened from the keyboard (KDE's default "hide until Alt" style).
fn mnemonics_shown(ctx: &egui::Context) -> bool {
    // always shown, so the letters can be found without knowing to hold Alt
    let _ = ctx;
    true
}

/// Lay out a label written with Qt's "&" mnemonic marker ("&&" for a plain &).
/// Returns the text and the key that picks it.
fn mnemonic(ui: &egui::Ui, text: &str, col: Color32) -> (egui::text::LayoutJob, Option<egui::Key>) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let show = mnemonics_shown(ui.ctx());
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat { font_id: font, color: col, ..Default::default() };
    let under = egui::TextFormat { underline: if show { Stroke::new(1.0, col) } else { Stroke::NONE }, ..plain.clone() };
    let mut key = None;
    let mut chars = text.chars();
    let mut buf = String::new();
    while let Some(c) = chars.next() {
        if c != '&' {
            buf.push(c);
            continue;
        }
        match chars.next() {
            Some('&') => buf.push('&'),
            Some(m) if key.is_none() => {
                if !buf.is_empty() {
                    job.append(&std::mem::take(&mut buf), 0.0, plain.clone());
                }
                job.append(&m.to_string(), 0.0, under.clone());
                key = egui::Key::from_name(&m.to_uppercase().to_string());
            }
            Some(m) => buf.push(m),
            None => {}
        }
    }
    if !buf.is_empty() {
        job.append(&buf, 0.0, plain);
    }
    (job, key)
}

/// The mnemonic letter pressed while this menu is open (no modifiers needed).
fn mnemonic_pressed(ui: &egui::Ui, key: Option<egui::Key>) -> bool {
    let Some(k) = key else { return false };
    ui.input(|i| !i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt && i.key_pressed(k))
}

/// A dialog in a window of its own, as KDE dialogs are, centred over the main
/// window the first time it opens. Sets `open` to false when the user closes
/// it from the title bar or with Esc.
fn dialog(ctx: &egui::Context, id: &str, title: &str, size: Vec2, resizable: bool, open: &mut bool, add: impl FnMut(&mut egui::Ui)) {
    dialog_ex(ctx, id, title, size, resizable, false, open, add)
}

/// `dialog`, optionally kept above other windows.
#[allow(clippy::too_many_arguments)]
fn dialog_ex(ctx: &egui::Context, id: &str, title: &str, size: Vec2, resizable: bool, on_top: bool, open: &mut bool, mut add: impl FnMut(&mut egui::Ui)) {
    let pal = *p();
    let vid = egui::ViewportId::from_hash_of(("gifkit-dialog", id));
    let pos_id = egui::Id::unique(("dialog-pos", id));
    let pos = ctx.data_mut(|d| d.get_temp::<Pos2>(pos_id)).or_else(|| {
        let main = ctx.input(|i| i.viewport().outer_rect)?;
        let p = main.center() - size / 2.0 - vec2(0.0, 30.0);
        ctx.data_mut(|d| d.insert_temp(pos_id, p));
        Some(p)
    });
    let mut b = egui::ViewportBuilder::default().with_title(title).with_app_id("gifkit").with_inner_size(size).with_resizable(resizable).with_minimize_button(false).with_active(true);
    if !resizable {
        b = b.with_maximize_button(false);
    }
    if on_top {
        b = b.with_window_level(egui::WindowLevel::AlwaysOnTop);
    }
    if let Some(pos) = pos {
        b = b.with_position(pos);
    }
    let close = ctx.show_viewport_immediate(vid, b, |ui, class| {
        if class == egui::ViewportClass::EmbeddedWindow {
            add(ui);
        } else {
            let used = egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::same(14)))
                .show(ui, |ui| {
                    add(ui);
                    ui.cursor().top() + 14.0
                })
                .inner;
            // fixed-size dialogs grow or shrink to fit what's in them, so
            // bigger fonts or interface scales never push buttons out of sight
            if !resizable {
                let now = ui.ctx().input(|i| i.viewport().inner_rect).map(|r| r.height()).unwrap_or(size.y);
                let want = used.clamp(size.y * 0.6, size.y * 1.8).ceil();
                if std::env::var_os("GIFKIT_PERF").is_some() {
                    eprintln!("dialog {id}: used {used:.0} now {now:.0} want {want:.0}");
                }
                if (want - now).abs() > 1.5 {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(size.x, want)));
                }
            }
        }
        ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
    });
    if close {
        *open = false;
        ctx.data_mut(|d| d.remove::<Pos2>(pos_id));
    }
}

/// A Breeze progress bar: a thin rounded groove filled with the highlight,
/// its text above it. `None` for an unknown amount (a sliding piece).
fn breeze_progress(ui: &mut egui::Ui, frac: Option<f32>, text: &str) {
    let pal = *p();
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).color(pal.fg));
        if let Some(f) = frac {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{:.0}%", f.clamp(0.0, 1.0) * 100.0)).color(pal.fg_muted));
            });
        }
    });
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(r, 3.0, mix(pal.view, pal.fg, 0.12), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
    match frac {
        Some(f) => {
            let w = r.width() * f.clamp(0.0, 1.0);
            if w > 0.5 {
                painter.rect_filled(Rect::from_min_size(r.min, vec2(w.max(6.0), r.height())), 3.0, pal.highlight);
            }
        }
        None => {
            // a piece a quarter long, going back and forth
            let t = ui.input(|i| i.time) as f32;
            let k = (t * 0.8).fract();
            let k = if k < 0.5 { k * 2.0 } else { 2.0 - k * 2.0 };
            let w = r.width() * 0.25;
            let x = r.min.x + (r.width() - w) * k;
            painter.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), 3.0, pal.highlight);
            ui.ctx().request_repaint();
        }
    }
}

/// Recently opened files, newest first. Saved as a JSON list on one line
/// (older versions wrote one path a line, which kept only the first).
fn load_recent() -> Vec<PathBuf> {
    let v = theme::get_setting("recent").unwrap_or_default();
    match serde_json::from_str::<Vec<String>>(&v) {
        Ok(l) => l.into_iter().filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
        Err(_) => v.split('\n').filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
    }
}

/// Put a file on the system clipboard (as a file, the way a file manager
/// copies one), so it pastes into chats, posts and folders. The clipboard
/// is kept for the life of the app: on Linux it goes when its owner does.
fn copy_file_to_clipboard(p: &std::path::Path) -> anyhow::Result<()> {
    static CB: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);
    let mut g = CB.lock().map_err(|_| anyhow::anyhow!("clipboard busy"))?;
    if g.is_none() {
        *g = Some(arboard::Clipboard::new()?);
    }
    g.as_mut().unwrap().set().file_list(&[p])?;
    Ok(())
}

/// "3 minutes", "2 hours", "5 days".
fn fmt_ago(s: u64) -> String {
    let (n, unit) = if s < 90 { (s, "second") } else if s < 5400 { (s / 60, "minute") } else if s < 129_600 { (s / 3600, "hour") } else { (s / 86_400, "day") };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Shorten to `max` characters with an ellipsis.
fn elide(s: &str, max: usize) -> String {
    let s = s.lines().next().unwrap_or("");
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}


/// The full-width "Add New … Layer" button at the top of a layer tool.
fn add_layer_button(ui: &mut egui::Ui, text: &str) -> bool {
    let pal = *p();
    // a plain Breeze button; the icon carries the emphasis
    let b = egui::Button::image_and_text(crate::icons::image(ui.ctx(), "list-add", 16.0, pal.fg), RichText::new(text).color(pal.fg));
    ui.add_sized(vec2(ui.available_width(), CHIP_H), b).clicked()
}

fn hint(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).size(12.0).color(p().fg_muted))
}

/// Slider whose handle stops at the range, but typed values can go beyond it.
fn soft_slider<'a>(v: &'a mut f32, r: std::ops::RangeInclusive<f32>, text: &str) -> egui::Slider<'a> {
    let s = egui::Slider::new(v, r).clamping(egui::SliderClamping::Never);
    if text.is_empty() { s } else { s.text(text) }
}

fn small_icon(ui: &mut egui::Ui, icon: &str, tip: &str) -> bool {
    let pal = *p();
    let r = ui.add(egui::Button::new("").frame_when_inactive(false).min_size(vec2(22.0, 22.0)));
    crate::icons::paint(ui.painter(), icon, r.rect.center(), 16.0, if r.hovered() { pal.fg } else { pal.fg_muted });
    r.on_hover_text(tip).clicked()
}

impl GifKitApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        if std::env::var_os("GIFKIT_PERF").is_some() {
            match &cc.wgpu_render_state {
                Some(rs) => eprintln!("renderer: wgpu on {:?} ({:?})", rs.adapter.get_info().name, rs.adapter.get_info().backend),
                None => eprintln!("renderer: OpenGL"),
            }
        }
        if let Some(z) = theme::get_setting("ui_scale").and_then(|v| v.parse::<f32>().ok()).filter(|z| (0.5..=3.0).contains(z)) {
            cc.egui_ctx.set_zoom_factor(z);
        }
        // a renderer for background work (export, the compressed preview),
        // on a device of its own: long jobs then never hold up the window's
        // device (resizing waits for it to go idle). Made off the UI thread
        // since the shaders compile first.
        if std::env::var_os("GIFKIT_CPU_PREVIEW").is_none() {
            std::thread::spawn(|| {
                if let Some((r, name)) = crate::gpu_render::GpuRenderer::headless_named() {
                    crate::gpu_render::set_shared(r, name);
                }
            });
        }
        theme::load_toolbar_choice();
        let theme_choice = match std::env::var("GIFKIT_THEME").as_deref() {
            Ok("dark") => ThemeChoice::Dark,
            Ok("light") => ThemeChoice::Light,
            _ => theme::load_theme_choice(),
        };
        theme_choice.apply(&cc.egui_ctx);
        theme::watch(&cc.egui_ctx);
        let mut app = Self {
            base: None,
            original: None,
            orig_gen: 1,
            path: None,
            file_size: None,
            modified: false,
            version: 1,
            timing_rev: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            last_coalesce: None,
            stack: Stack::default(),
            fx_draft: EffectSettings::default(),
            anim_draft: EffectSettings { anim: true, ..Default::default() },
            sel: HashMap::new(),
            plan: Vec::new(),
            plan_times: Vec::new(),
            plan_key: (0, 0, 0),
            out_size: (1.0, 1.0),
            tool: Tool::Effects,
            playing: true,
            cur: 0,
            elapsed: 0.0,
            views: [ViewState::default(); 3],
            active_view: 2,
            link_views: false,
            box_drag: None,
            backdrop: match theme::get_setting("backdrop").as_deref() {
                Some("solid") => Backdrop::Solid,
                Some("canvas") => Backdrop::Canvas,
                _ => Backdrop::Checker,
            },
            backdrop_col: theme::get_setting("backdrop_col").and_then(|v| parse_hex(&v)).unwrap_or([255, 255, 255]),
            pixelated: theme::get_setting("pixelated").as_deref() == Some("1"),
            onion: theme::get_setting("onion").as_deref() == Some("1") || std::env::var_os("GIFKIT_ONION").is_some(),
            onion_paint: Vec::new(),
            base_tex: TexCache::default(),
            thumbs: Thumbs::default(),
            comp_tex: TexCache::default(),
            orig_tex: TexCache::default(),
            cmp_tex: TexCache::default(),
            crop_tex: TexCache::default(),
            crop_prep: None,
            crop_live: true,
            checker_tex: None,
            comp_key: 0,
            comp_key_since: 0.0,
            prerender: None,
            prerender_cancel: Arc::new(AtomicU64::new(0)),
            job: None,
            status: None,
            fonts: text::builtin_fonts(),
            builtin_font_count: text::builtin_fonts().len(),
            sys_fonts: Arc::new(std::sync::Mutex::new(None)),
            font_keys: HashMap::new(),
            font_meta: HashMap::new(),
            font_search: String::new(),
            ffmpeg: { let f = io::ffmpeg_available(); if f { crate::gpu::warm_up(); } f },
            theme: theme_choice,
            compare: true,
            view_left: ViewSource::Original,
            view_right: ViewSource::Compressed,
            cmp: None,
            cmp_job: None,
            pack_job: None,
            pack_tried: std::collections::HashSet::new(),
            pack_last: 0.0,
            cmp_key_seen: 0,
            cmp_key_since: 0.0,
            cmp_gen: 0,
            crop_lock: AspectLock::Free,
            trim_tol: 8,
            drag: None,
            gif_opts: GifOptions::default(),
            gif_opts_open: GifOptions::default(),
            opt_drop_nth: 0,
            place_scope: PlaceScope::All,
            split_fmt: SplitFormat::Png,
            split_prefix: "frame".into(),
            sheet: SheetOptions::default(),
            fr_from: 1,
            fr_to: 5,
            fr_every: 2,
            fr_delay: 10,
            fr_crossfade: 2,
            fr_crossfade_wrap: true,
            fr_follow: true,
            loop_blend: 8,
            fr_thumb: 96.0,
            video_bg: [255, 255, 255],
            view_single: ViewSource::Edit,
            sidebar_compact: theme::get_setting("sidebar_compact").as_deref() != Some("0"),
            layers_collapsed: false,
            layers_open_h: 380.0,
            split_frac: theme::get_setting("split").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.5).clamp(0.05, 0.95),
            timeline_open_h: 270.0,
            layer_drag: None,
            export_open: false,
            export_fmt: ExportFormat::Gif,
            frames_zip: false,
            presets: Vec::new(),
            batch: Default::default(),
            smoke: smoke::Smoke::from_env(),
            handbook_open: std::env::var_os("GIFKIT_HANDBOOK").is_some(),
            handbook_topic: std::env::var("GIFKIT_HANDBOOK").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            preset_problems: Vec::new(),
            preset_name: std::env::var("GIFKIT_PRESET_NAME").unwrap_or_default(),
            preset_inc: (true, true, true),
            font_preview_queue: Vec::new(),
            preview_fonts: HashMap::new(),
            prep_cache: None,
            webp_opts: io::WebpOptions::default(),
            avif_quality: 60,
            target_val: 9.5,
            target_mb: true,
            history_open: false,
            committed: None,
            commit_since: 0.0,
            pending_sig: 0,
            last_comp: None,
            gpu: cc.wgpu_render_state.as_ref().filter(|rs| std::env::var_os("GIFKIT_CPU_PREVIEW").is_none() && crate::gpu_render::worth_using(&rs.adapter.get_info())).map(|rs| (crate::gpu_render::GpuRenderer::new(rs.device.clone(), rs.queue.clone()), rs.renderer.clone())),
            gpu_frames: GpuFrames { key: 0, map: HashMap::new(), shown: None, bytes: 0, tick: 0, since_busy: 0 },
            last_tick: None,
            orig_clock: 0.0,
            resize_by_pct: true,
            about_open: false,
            ff_open: false,
            ff_job: None,
            ff_msg: None,
            ff_then: None,
            renaming: None,
            rename_fresh: false,
            edit_rows_shown: Default::default(),
            edit_labels: HashMap::new(),
            edit_renaming: None,
            fit_record: None,
            frames_png: io::PngOptions::default(),
            export_name: String::new(),
            export_name_edited: false,
            open_after_save: theme::get_setting("open_after_save").as_deref() == Some("1"),
            last_dir: theme::get_setting("last_dir").map(PathBuf::from).filter(|d| d.is_dir()),
            pending_open: None,
            play_speed: 1.0,
            video_import: None,
            sample_preview: theme::get_setting("sample_preview").as_deref() != Some("0"),
            cmp_full_key: 0,
            shortcuts_open: false,
            win_title: String::new(),
            show_timeline: theme::get_setting("show_timeline").as_deref() != Some("0"),
            tl_zoom: 0.0,
            tl_fit: 1.0,
            tl_zoom_now: 1.0,
            tl_cur_seen: 0,
            tl_zoom_to: None,
            tl_pan: false,
            tl_bar_drag: None,
            tl_drop: None,
            tl_menu_head: None,
            tl_sel: Vec::new(),
            copy_to: None,
            copy_pending: None,
            autosave_at: 0.0,
            autosave_sig: 0,
            autosave_job: None,
            restore_offer: None,
            export_cut_empty: theme::get_setting("export_cut_empty").as_deref() != Some("0"),
            export_zone_only: false,
            tl_keys: Vec::new(),
            tl_key_dropped: None,
            tl_key_group: Vec::new(),
            tl_key_moved: 0,
            tl_group: Vec::new(),
            tl_box: None,
            tl_board: None,
            tl_paste_ask: None,
            tl_snap: theme::get_setting("tl_snap").map(|v| v != "0").unwrap_or(true),
            tl_snap_line: None,
            tl_in: None,
            tl_out: None,
            tl_zone_loop: true,
            tl_solo: None,
            tl_gif_h: theme::get_setting("tl_gif_h").and_then(|v| v.parse().ok()).unwrap_or(40.0),
            tl_row_h: theme::get_setting("tl_row_h").and_then(|v| v.parse().ok()).unwrap_or(28.0),
            tl_row_drag: None,
            tl_marker_drag: None,
            tl_marker_edit: None,
            tl_menu_marker: None,
            tl_area: Rect::NOTHING,
            tl_focus: false,
            layer_board: Vec::new(),
            paste_in: None,
            tl_x0: 0.0,
            tl_y0: 0.0,
            tl_track: None,
            tl_clip: None,
            tl_key: None,
            tl_razor: false,
            tl_drag: None,
            tl_head_w: theme::get_setting("timeline_names").and_then(|v| v.parse().ok()).unwrap_or(170.0),
            tl_head_drag: false,
            tl_reorder: None,
            tl_extend: 0,
            tl_snap_pts: None,
            tl_free_drop: None,
            tl_gif_d0: None,
            menubar_rect: Rect::NOTHING,
            tl_extend_mode: timeline_ui::Extend::load(),
            confirm: None,
            after_save: None,
            quitting: false,
            tl_autokey: theme::get_setting("auto_key").map(|v| v != "0").unwrap_or(true),
            path_draw: false,
            path_pts: Vec::new(),
            path_track: false,
            tl_trim: None,
            keymap: crate::keys::Keymap::load(),
            key_capture: None,
            keys_filter: String::new(),
            panel_find: String::new(),
            panel_find_tool: None,
            layer_find: String::new(),
            layer_find_open: false,
            layer_find_fresh: false,
            preview_max: theme::get_setting("preview_max").and_then(|v| v.parse().ok()).unwrap_or(1280),
            mem_pct: {
                let pct = theme::get_setting("mem_pct").and_then(|v| v.parse().ok()).unwrap_or(100);
                crate::pixels::set_mem_pct(pct);
                pct
            },
            mem_seen: (0, -10.0),
            proxy: None,
            proxy_job: None,
            live_job: None,
            last_render_ms: 0.0,
            emoji_drag: None,
            drop_views: Vec::new(),
            views_last: Vec::new(),
            show_toolbar: theme::get_setting("show_toolbar").as_deref() != Some("0"),
            show_statusbar: theme::get_setting("show_statusbar").as_deref() != Some("0"),
            recent: load_recent(),
            project_path: None,
            shape_tpl: ShapeSettings::default(),
            text_tpl: Annotation { text: "Your text".into(), ..Default::default() },
            censor_tpl: CensorSettings::default(),
            wm_tpl: Default::default(),
            shield_tpl: ShieldSettings::default(),
            emoji_tpl: EmojiSettings::default(),
            emoji_tex: HashMap::new(),
            emoji_pick: "1f600".into(),
            emoji_tone: theme::get_setting("emoji_tone").and_then(|v| v.parse().ok()).filter(|t: &usize| *t <= 5).unwrap_or(0),
            emoji_search: String::new(),
            emoji_tab: EmojiTab::Group(0),
            emoji_recent: theme::get_setting("recent_emoji").map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default(),
            emoji_replace: false,
            pen: PenStroke::default(),
            highlighter: PenStroke { color: [255, 230, 0], width: 22.0, opacity: 40.0, mode: PenMode::Highlighter, ..Default::default() },
            pen_tool: PenTool::Pen,
            pen_scope: PenScope::All,
            pen_live: None,
            fit_keep_size: theme::get_setting("fit_keep_size").as_deref() == Some("1"),
        };
        // scan system fonts in the background
        {
            let slot = app.sys_fonts.clone();
            std::thread::spawn(move || {
                let fams = crate::sysfonts::scan();
                if let Ok(mut g) = slot.lock() {
                    *g = Some(fams);
                }
            });
        }
        // Optional start-up state (handy for scripting & screenshots)
        if let Ok(t) = std::env::var("GIFKIT_TOOL") {
            if let Some(tool) = Tool::ALL.iter().find(|x| x.label().to_lowercase().starts_with(&t.to_lowercase())) {
                app.tool = *tool;
                if app.tool == Tool::Presets {
                    let (l, pr) = crate::presets::list_with_problems();
                    app.presets = l;
                    app.preset_problems = pr;
                }
            }
        }
        if let Ok(f) = std::env::var("GIFKIT_FORMAT") {
            if let Some(f) = ExportFormat::ALL.iter().find(|x| x.label().to_lowercase().starts_with(&f.to_lowercase())) {
                app.export_fmt = *f;
            }
        }
        // scripting hook: where Ctrl+S saves the project without asking
        if let Ok(p) = std::env::var("GIFKIT_PROJECT_PATH") {
            app.project_path = Some(PathBuf::from(p));
        }
        if let Ok(q) = std::env::var("GIFKIT_EMOJI_SEARCH") {
            app.emoji_search = q;
        }
        if std::env::var("GIFKIT_ABOUT").is_ok() {
            app.about_open = true;
        }
        // work kept from a session that didn't close normally
        if let Some((kept, note)) = Self::autosave_paths().filter(|(k, _)| k.is_file()) {
            let info: serde_json::Value = std::fs::read_to_string(&note).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let project = info["project"].as_str().map(PathBuf::from);
            let name = project.as_ref().or(info["source"].as_str().map(PathBuf::from).as_ref()).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            let ago = info["saved"].as_u64().and_then(|t| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|n| n.as_secs().saturating_sub(t)));
            let when = match (name, ago) {
                (Some(n), Some(s)) => format!("{n} · saved {} ago", fmt_ago(s)),
                (Some(n), None) => n,
                (None, Some(s)) => format!("Saved {} ago", fmt_ago(s)),
                _ => String::new(),
            };
            app.restore_offer = Some((kept, when, project));
        }
        // Windows doesn't come with ffmpeg: offer to set it up (once, unless asked to again)
        if (cfg!(windows) || std::env::var_os("GIFKIT_FFMPEG_ASK").is_some()) && !app.ffmpeg && theme::get_setting("ffmpeg_ask").as_deref() != Some("no") {
            app.ff_open = true;
        }
        if std::env::var("GIFKIT_HISTORY").is_ok() {
            app.history_open = true;
        }
        if let Ok(f) = std::env::var("GIFKIT_BATCH") {
            app.batch.open = true;
            app.batch.files = std::env::split_paths(&f).collect();
            app.batch.edits = batch_ui::BatchEdits::Current;
        }
        if std::env::var("GIFKIT_SHORTCUTS").is_ok() {
            app.shortcuts_open = true;
        }
        if std::env::var("GIFKIT_EXPORT").is_ok() {
            app.export_open = true;
        }
        if std::env::var("GIFKIT_COMPARE").is_ok() {
            app.compare = true;
        }
        if let Some(p) = initial {
            app.open_path(p, &cc.egui_ctx);
        }
        app
    }

    // ------------------------------------------------------------ status / jobs

    fn info(&mut self, s: impl Into<String>) {
        self.status = Some(Status { text: s.into(), error: false });
    }
    fn error(&mut self, s: impl Into<String>) {
        self.status = Some(Status { text: s.into(), error: true });
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn spawn(&mut self, ctx: &egui::Context, label: &str, total: usize, f: impl FnOnce(&AtomicUsize) -> JobOutput + Send + 'static) {
        self.spawn_in(ctx, label, total, true, f);
    }

    /// `bg`: run in the background pool (shared with previews). Saving and
    /// opening projects don't, so they never wait behind a preview render.
    fn spawn_in(&mut self, ctx: &egui::Context, label: &str, total: usize, bg: bool, f: impl FnOnce(&AtomicUsize) -> JobOutput + Send + 'static) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let p2 = progress.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let out = if bg { stack::bg_pool().install(|| f(&p2)) } else { f(&p2) };
            let _ = tx.send(out);
            ctx2.request_repaint();
        });
        self.job = Some(Job { label: label.to_string(), rx, progress, total: total.max(1) });
    }

    /// Run a structural operation on the base frames in the background, with undo.
    fn run_anim(&mut self, ctx: &egui::Context, label: &str, f: impl FnOnce(&Animation, &AtomicUsize) -> anyhow::Result<Animation> + Send + 'static) {
        let Some(anim) = self.base.clone() else { return };
        let total = anim.len();
        self.spawn(ctx, label, total, move |p| JobOutput::Anim(f(&anim, p)));
    }

    /// Flatten base + live stack in the background, then hand the result to `f`.
    fn run_export(&mut self, ctx: &egui::Context, label: &str, f: impl FnOnce(&Animation, &AtomicUsize) -> anyhow::Result<String> + Send + 'static) {
        let Some(base) = self.base.clone() else { return };
        let stack = self.stack.clone();
        let fonts = self.fonts.clone();
        let (range, cut_empty) = self.export_trim();
        let total = range.map(|(a, b)| b + 1 - a).unwrap_or(self.plan.len()) * 2;
        self.spawn(ctx, label, total, move |p| {
            let (out, _) = stack.flatten_for_export(&base, &fonts, Some(p), range, cut_empty);
            JobOutput::Info(f(&out, p))
        });
    }

    /// Bake live edits into the frames. `transform_only` keeps the layers live.
    fn bake(&mut self, ctx: &egui::Context, transform_only: bool) {
        let Some(base) = self.base.clone() else { return };
        if self.stack.is_empty() || (transform_only && self.stack.transform.is_identity()) {
            self.info("Nothing to bake");
            return;
        }
        let mut stack = self.stack.clone();
        let fonts = self.fonts.clone();
        let total = self.plan.len();
        // the user is waiting on this, so it doesn't queue behind preview renders
        self.spawn_in(ctx, if transform_only { "Collapse frame edits" } else { "Collapse all" }, total, false, move |p| {
            let mut to_flatten = stack.clone();
            if transform_only {
                to_flatten.layers.clear();
            }
            let out = to_flatten.flatten(&base, &fonts, Some(p));
            stack.transform = Transform::default();
            if !transform_only {
                stack.layers.clear();
            }
            JobOutput::Baked(Ok((out, stack)))
        });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            Err(_) => {
                self.job = None;
                self.error("Background task crashed");
                return;
            }
        };
        let label = job.label.clone();
        self.job = None;
        match res {
            JobOutput::Anim(Ok(a)) => {
                self.set_base_with_undo(a, None, &label);
                self.info(format!("✔ {label}"));
            }
            JobOutput::Baked(Ok((a, s))) => {
                self.set_base_with_undo(a, Some(s), &label);
                self.info(format!("✔ {label}"));
            }
            JobOutput::Loaded(Ok((mut a, path, size))) => {
                // held pictures (and a video's merged still parts) become
                // several frames sharing one picture, so moving layers keep
                // moving over them
                a.split_holds();
                self.undo.clear();
                self.redo.clear();
                self.committed = None;
                self.path = path;
                self.file_size = size;
                self.modified = false;
                // save it again the way it was saved, so the size stays close
                let like = self.path.as_ref().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"))).and_then(|p| std::fs::read(p).ok()).and_then(|b| crate::encode::gif_info(&b));
                let opts = like.map(|i| GifOptions::like(&i)).unwrap_or_default();
                self.gif_opts = GifOptions { noai_tag: self.gif_opts.noai_tag, ..opts.clone() };
                self.gif_opts_open = self.gif_opts.clone();
                self.cmp = None;
                self.original = Some(a.clone());
                self.orig_gen += 1;
                self.base = Some(a);
                self.stack = Stack::default();
                self.edit_labels.clear();
                self.edit_rows_shown.clear();
                self.export_name_edited = false;
                self.project_path = std::env::var("GIFKIT_PROJECT_PATH").ok().map(PathBuf::from);
                self.sel.clear();
                self.cur = 0;
                self.elapsed = 0.0;
                self.views = [ViewState::default(); 3];
                self.bump();
                self.after_base_change(true);
                let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sequence".into());
                self.info(format!("Opened {name}"));
                // scripting hook (screenshots): a typical layer stack
                if std::env::var("GIFKIT_DEMO").is_ok() {
                    self.add_layer(LayerKind::Shield(ShieldSettings::default()));
                    self.add_layer(LayerKind::Effects(Box::new(EffectSettings { saturation: 140.0, vignette: true, ..Default::default() })));
                    self.add_layer(LayerKind::Censor(CensorSettings { x: 200, y: 140, w: 70, h: 60, angle: 12.0, ..Default::default() }));
                    self.add_layer(LayerKind::Text(Annotation { text: "This caption is a really long line of text for testing".into(), size: 22.0, x: 20, y: 190, ..Default::default() }));
                    self.add_layer(LayerKind::Watermark(Box::default()));
                }
                // scripting hook (screenshots): shapes and markup
                if std::env::var("GIFKIT_DEMO_SHAPES").is_ok() {
                    let mk = |kind, x, y, w, h, angle, fill: [u8; 3]| ShapeSettings { kind, x, y, w, h, angle, fill, ..Default::default() };
                    self.add_layer(LayerKind::Shape(Box::new(ShapeSettings { stroke: [34, 38, 44], fill: [255, 255, 255], corner: 30.0, ..mk(ShapeKind::Speech, 150, 18, 150, 82, 0.0, [255, 255, 255]) })));
                    self.add_layer(LayerKind::Shape(Box::new(ShapeSettings { stroke_on: false, shadow: true, ..mk(ShapeKind::Arrow, 30, 150, 120, 34, -25.0, [255, 59, 48]) })));
                    self.add_layer(LayerKind::Shape(Box::new(ShapeSettings { points: 5, ..mk(ShapeKind::Star, 20, 20, 70, 70, 10.0, [255, 204, 0]) })));
                    self.add_layer(LayerKind::Emoji(Box::new(EmojiSettings { emoji: "1f525".into(), shadow: true, x: 240, y: 150, w: 64, h: 64, angle: -12.0, ..Default::default() })));
                    let pts: Vec<[f32; 2]> = (0..40).map(|i| { let t = i as f32 / 39.0; [170.0 + 120.0 * t, 205.0 + 10.0 * (t * 9.0).sin()] }).collect();
                    let hl: Vec<[f32; 2]> = vec![[30.0, 112.0], [140.0, 112.0]];
                    self.add_layer(LayerKind::Markup(MarkupSettings { strokes: vec![
                        PenStroke { points: hl, color: [255, 230, 0], width: 20.0, opacity: 45.0, mode: PenMode::Highlighter, ..Default::default() },
                        PenStroke { points: pts, color: [61, 174, 233], width: 5.0, ..Default::default() },
                    ] }));
                }
                // scripting hook for performance tests: an effects layer on top
                if std::env::var("GIFKIT_DEMO_FX").is_ok() {
                    self.add_layer(LayerKind::Effects(Box::new(EffectSettings { saturation: 140.0, vignette: true, blur: true, blur_sigma: 2.0, ..Default::default() })));
                }
                // scripting hook: GIFKIT_OVERLAY=path adds an image layer on open
                if let Ok(ov) = std::env::var("GIFKIT_OVERLAY") {
                    if let Ok(a) = io::load_animation(std::path::Path::new(&ov)) {
                        self.add_overlay(OverlaySource { name: ov.rsplit('/').next().unwrap_or("overlay").into(), path: Some(PathBuf::from(&ov)), delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image.get()).clone()).collect() });
                    }
                }
            }
            JobOutput::Info(r) => self.saved(r.map_err(|e| anyhow::anyhow!("{label} failed: {e:#}"))),
            JobOutput::Fit(Ok(fit)) => {
                let before = self.comp_state();
                let (scale, n, reached, desc) = (fit.scale, fit.bytes.len() as u64, fit.reached, fit.settings.clone());
                if let Some(o) = fit.gif {
                    self.gif_opts = o;
                }
                if let Some(o) = fit.webp {
                    self.webp_opts = o;
                }
                if let Some(q) = fit.avif {
                    self.avif_quality = q;
                }
                apply_fit_scale(&mut self.stack.transform, scale);
                self.fit_record = Some((before, self.comp_state()));
                if reached {
                    self.info(format!("Fits: {} with {desc}", fmt_size(n)));
                } else {
                    self.error(format!("Can't reach the target – smallest is {} ({desc})", fmt_size(n)));
                }
            }
            JobOutput::Overlay(Ok(src)) => {
                self.add_overlay(src);
                self.tool = Tool::AddImage;
                self.info("Overlay added – drag it on the preview to position it");
            }
            JobOutput::Anim(Err(e)) | JobOutput::Baked(Err(e)) | JobOutput::Loaded(Err(e)) | JobOutput::Overlay(Err(e)) | JobOutput::Fit(Err(e)) | JobOutput::ProjectLoaded(Err(e)) | JobOutput::ProjectSaved(Err(e)) => self.error(format!("{label} failed: {e:#}")),
            JobOutput::ProjectLoaded(Ok((pr, path))) => self.apply_project(pr, path),
            JobOutput::LoopPoint(None) => self.info("Too few frames to look for a loop"),
            JobOutput::Track(id, a, pos, centre) => {
                if let Some(li) = self.stack.index_of(id) {
                    let now = ctx.input(|i| i.time);
                    self.snapshot(&format!("track{now}"), now);
                    let k = self.stack.layers[li].timing.set_positions(a, &pos, centre);
                    self.modified = true;
                    self.info(format!("Tracked over {} frames: {k} keyframes", pos.len()));
                }
            }
            JobOutput::LoopPoint(Some((a, b, d))) => {
                // in and out around it, looping, to see it and save it
                self.tl_set_in(Some(a));
                self.tl_set_out(Some(b.saturating_sub(1).max(a)));
                self.tl_zone_loop = true;
                self.export_zone_only = true;
                self.cur = a;
                self.info(format!("Best loop: frames {}–{} ({:.0}% different at the seam). In and out are set, and export saves just that.", a + 1, b, d * 100.0));
            }
            JobOutput::ProjectSaved(Ok(path)) => {
                self.modified = false;
                self.clear_autosave();
                self.info(format!("Saved project {}", path.display()));
                self.project_path = Some(path);
            }
        }
    }

    fn bump(&mut self) {
        self.version += 1;
        self.timing_rev += 1;
    }

    fn set_base_with_undo(&mut self, a: Animation, new_stack: Option<Stack>, label: &str) {
        let size_changed = self.base.as_ref().map(|o| (o.width(), o.height(), o.len())) != Some((a.width(), a.height(), a.len()));
        self.flush_live_edit();
        if let Some(old) = self.base.take() {
            self.push_undo(Snapshot { anim: old, stack: self.stack.clone(), gif: self.gif_opts.clone(), label: label.to_string() });
        }
        self.committed = None;
        self.redo.clear();
        self.base = Some(a);
        if let Some(s) = new_stack {
            self.stack = s;
        }
        self.modified = true;
        self.last_coalesce = None;
        self.bump();
        self.after_base_change(size_changed);
    }

    fn push_undo(&mut self, s: Snapshot) {
        self.undo.push(s);
        // fewer steps back on Low memory, more on High
        let keep = (crate::pixels::scaled(100 << 10) >> 10).clamp(30, 250);
        while self.undo.len() > keep {
            self.undo.remove(0);
        }
    }

    /// Push an undo snapshot (base + stack), coalescing repeats of the same edit.
    fn snapshot(&mut self, key: &str, now: f64) {
        if let Some((k, t)) = &self.last_coalesce {
            if k == key && now - *t < 1.5 {
                self.last_coalesce = Some((key.to_string(), now));
                return;
            }
        }
        self.flush_live_edit();
        if let Some(a) = &self.base {
            let s = Snapshot { anim: a.clone(), stack: self.stack.clone(), gif: self.gif_opts.clone(), label: key_label(key) };
            self.push_undo(s);
            self.redo.clear();
        }
        self.committed = None;
        self.last_coalesce = Some((key.to_string(), now));
        self.modified = true;
    }

    fn do_undo(&mut self) {
        self.flush_live_edit();
        if let Some(prev) = self.undo.pop() {
            let label = prev.label.clone();
            if let Some(cur) = self.base.replace(prev.anim) {
                let stack = std::mem::replace(&mut self.stack, prev.stack);
                let gif = std::mem::replace(&mut self.gif_opts, prev.gif);
                self.redo.push(Snapshot { anim: cur, stack, gif, label: label.clone() });
            }
            self.last_coalesce = None;
            self.committed = None;
            self.bump();
            self.after_base_change(false);
            self.info(format!("Undo: {label}"));
        }
    }

    fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let label = next.label.clone();
            if let Some(cur) = self.base.replace(next.anim) {
                let stack = std::mem::replace(&mut self.stack, next.stack);
                let gif = std::mem::replace(&mut self.gif_opts, next.gif);
                self.undo.push(Snapshot { anim: cur, stack, gif, label: label.clone() });
            }
            self.committed = None;
            self.bump();
            self.after_base_change(false);
            self.info(format!("Redo: {label}"));
        }
    }

    /// Target size in bytes (binary units, matching how sizes are shown).
    fn comp_state(&self) -> CompState {
        let t = &self.stack.transform;
        CompState { gif: self.gif_opts.clone(), webp: self.webp_opts, avif: self.avif_quality, resize: (t.resize_on, t.rs_w, t.rs_h, t.rs_pct.to_bits()) }
    }

    /// If the current settings are exactly what the last "Find settings"
    /// produced, go back to what they were before it (so a new search can
    /// go up as well as down).
    fn undo_last_fit(&mut self) -> bool {
        let Some((before, after)) = self.fit_record.take() else { return false };
        let cur = self.comp_state();
        let mut undone = false;
        // the size Find chose is undone even if compression was tweaked since,
        // so a new search can make the image bigger again
        if cur.resize == after.resize {
            let t = &mut self.stack.transform;
            (t.resize_on, t.rs_w, t.rs_h) = (before.resize.0, before.resize.1, before.resize.2);
            t.rs_pct = f32::from_bits(before.resize.3);
            undone = true;
        }
        // compression settings only if they are still exactly what Find set
        if (&cur.gif, &cur.webp, cur.avif) == (&after.gif, &after.webp, after.avif) {
            self.gif_opts = before.gif;
            self.webp_opts = before.webp;
            self.avif_quality = before.avif;
            undone = true;
        }
        undone
    }

    fn target_bytes(&self) -> u64 {
        (self.target_val as f64 * if self.target_mb { 1_048_576.0 } else { 1024.0 }) as u64
    }

    fn live_sig(&self) -> u64 {
        let labels: Vec<&Option<String>> = self.stack.layers.iter().map(|l| &l.label).collect();
        hash_parts(&[self.stack.signature(), hash_str(&format!("{:?}{:?}{:?}{labels:?}", self.gif_opts, self.webp_opts, self.avif_quality))])
    }

    /// Turn a pending (not yet recorded) live edit into an undo entry now.
    fn flush_live_edit(&mut self) {
        let Some((sig, old_stack, old_gif)) = self.committed.take() else { return };
        if sig == self.live_sig() {
            self.committed = Some((sig, old_stack, old_gif));
            return;
        }
        let Some(a) = &self.base else { return };
        let label = describe_edit(&old_stack, &old_gif, &self.stack, &self.gif_opts);
        let s = Snapshot { anim: a.clone(), stack: old_stack, gif: old_gif, label };
        self.push_undo(s);
        self.redo.clear();
        self.committed = Some((self.live_sig(), self.stack.clone(), self.gif_opts.clone()));
    }

    /// Record live edits (sliders, layers, compression) as undo entries once
    /// they have settled.
    fn track_live_edits(&mut self, ctx: &egui::Context, now: f64) {
        if self.base.is_none() || self.busy() {
            return;
        }
        let sig = self.live_sig();
        let Some((csig, _, _)) = &self.committed else {
            self.committed = Some((sig, self.stack.clone(), self.gif_opts.clone()));
            self.pending_sig = sig;
            return;
        };
        if sig == *csig {
            self.pending_sig = sig;
            return;
        }
        if sig != self.pending_sig {
            self.pending_sig = sig;
            self.commit_since = now;
        }
        let busy_input = ctx.input(|i| i.pointer.any_down()) || ctx.dragged_id().is_some() || self.box_drag.is_some() || self.drag.is_some() || ctx.egui_wants_keyboard_input();
        if busy_input || now - self.commit_since < 0.7 {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        self.flush_live_edit();
    }

    /// Jump through history: positive = redo n steps, negative = undo.
    fn history_jump(&mut self, steps: i64) {
        if steps < 0 {
            for _ in 0..(-steps) {
                self.do_undo();
            }
        } else {
            for _ in 0..steps {
                self.do_redo();
            }
        }
    }

    /// PNG colour control shared by frames and sprite sheets.
    fn png_colours_ui(ui: &mut egui::Ui, o: &mut io::PngOptions) {
        form_row(ui, "Colours", |ui| {
            even_grid(ui, 3, 0.0, |ui, i, size| match i {
                0 => {
                    if ui.add_sized(size, sel_btn(egui::Button::new("Full"), o.colors == 0)).on_hover_text("Every colour kept").clicked() {
                        o.colors = 0;
                    }
                }
                1 => {
                    if ui.add_sized(size, sel_btn(egui::Button::new("Palette"), o.colors > 0)).on_hover_text("Indexed PNG. Much smaller. Exact when the image has this few colours.").clicked() && o.colors == 0 {
                        o.colors = 256;
                    }
                }
                _ => {
                    let mut c = if o.colors == 0 { 256 } else { o.colors };
                    let en = o.colors > 0;
                    if ui.add_enabled_ui(en, |ui| spin(ui, size, &mut c, 1.0, |d| d.range(2..=256))).inner.on_hover_text("Palette size").changed() {
                        o.colors = c;
                    }
                }
            })
        });
    }

    /// Frames: file type and colours (Optimize panel).
    fn frames_settings_ui(&mut self, ui: &mut egui::Ui) {
        form_row(ui, "Files", |ui| seg_row(ui, &mut self.split_fmt, &[(SplitFormat::Png, "PNG"), (SplitFormat::Gif, "GIF")])).on_hover_text("GIF writes single-frame GIFs. ZIP export needs PNG.");
        if self.split_fmt == SplitFormat::Png {
            Self::png_colours_ui(ui, &mut self.frames_png);
        } else {
            slider_row(ui, "Colours", egui::Slider::new(&mut self.gif_opts.colors, 2..=256));
            dither_row(ui, &mut self.gif_opts);
        }
    }

    /// Sprite sheet layout line: "6×13 frames · 2048×4096 px".
    fn sheet_hint(&self) -> String {
        let n = self.plan.iter().filter(|f| f.enabled).count().max(1) as u32;
        let (fw, fh) = (self.out_size.0 as u32, self.out_size.1 as u32);
        let (cols, rows, w, h, cw, ch) = io::sheet_layout(n, fw, fh, &self.sheet);
        let used = if (cw, ch) != (w, h) { format!(" ({w}×{h} used)") } else { String::new() };
        format!("{cols}×{rows} frames · {cw}×{ch} px{used}")
    }

    /// Sprite sheet layout (Optimize panel).
    fn sheet_layout_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.plan.iter().filter(|f| f.enabled).count().max(1) as u32;
        let h = self.sheet_hint();
        let s = &mut self.sheet;
        form_row(ui, "Columns", |ui| num_fill(ui, &mut s.columns, 1.0, |d| d.range(0..=n).custom_formatter(|v, _| if v == 0.0 { "Automatic".into() } else { format!("{v}") }))).on_hover_text("0 picks the layout automatically");
        form_row(ui, "Padding", |ui| num_fill(ui, &mut s.padding, 1.0, |d| d.range(0..=256).suffix(" px")));
        form_row(ui, "", |ui| check(ui, &mut s.pow2, "Power-of-two size").on_hover_text("Pads the sheet to sizes like 512, 1024 or 2048 as most game engines expect. Automatic columns then pick the smallest such sheet."));
        form_row(ui, "Result", |ui| hint(ui, &h));
    }

    /// Options that only matter when writing the file (Export window).
    fn save_options_ui(&mut self, ui: &mut egui::Ui) {
        let noai_tip = "Marks the file as not for AI training. Adds a “noai” note and the standard XMP “data mining prohibited” field where the format allows it. Some sites and scrapers respect it.";
        egui::Grid::new("saveopts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Frames:");
            ui.vertical(|ui| {
                if check(ui, &mut self.export_cut_empty, "Cut empty end frames").on_hover_text("Frames at the end with nothing to see (fully see-through) are left out, so a shortened clip doesn't save a long empty stretch").changed() {
                    theme::set_setting("export_cut_empty", if self.export_cut_empty { "1" } else { "0" });
                }
                match self.tl_zone() {
                    Some((a, b)) => {
                        check(ui, &mut self.export_zone_only, &format!("Only In to Out (frames {}–{})", a + 1, b + 1)).on_hover_text("Save only the part between the timeline's in and out points");
                    }
                    None => {
                        ui.add_enabled_ui(false, |ui| {
                            let mut off = false;
                            check(ui, &mut off, "Only In to Out").on_disabled_hover_text("Set in and out points on the timeline first (I and O)");
                        });
                    }
                }
            });
            ui.end_row();
            if self.export_fmt.can_tag() {
                ui.label("Tag:");
                check(ui, &mut self.gif_opts.noai_tag, "Add “noai” tag").on_hover_text(noai_tip);
                ui.end_row();
            }
            match self.export_fmt {
                ExportFormat::Frames => {
                    if self.split_fmt == SplitFormat::Gif {
                        self.frames_zip = false;
                    }
                    ui.label("Save to:");
                    ui.horizontal(|ui| {
                        if radio(ui, !self.frames_zip, "Folder").clicked() {
                            self.frames_zip = false;
                        }
                        ui.add_space(12.0);
                        ui.add_enabled_ui(self.split_fmt == SplitFormat::Png, |ui| {
                            if radio(ui, self.frames_zip, "ZIP file").on_disabled_hover_text("ZIP holds PNG frames. Pick PNG in Optimize.").clicked() {
                                self.frames_zip = true;
                            }
                        });
                    });
                    ui.end_row();
                    ui.label("Prefix:");
                    ui.add(egui::TextEdit::singleline(&mut self.split_prefix).desired_width(160.0)).on_hover_text("Files are named prefix_000, prefix_001 and so on");
                    ui.end_row();
                }
                ExportFormat::Sheet => {
                    let s = &mut self.sheet;
                    ui.label("Also write:");
                    ui.vertical(|ui| {
                        check(ui, &mut s.write_json, "JSON metadata");
                        check(ui, &mut s.write_godot, "Godot SpriteFrames (.tres)").on_hover_text("Assign the .tres to an AnimatedSprite2D/3D. Frame delays are kept.");
                    });
                    ui.end_row();
                    if s.write_godot {
                        ui.label("Godot path:");
                        ui.add(egui::TextEdit::singleline(&mut s.godot_res_dir).desired_width(170.0)).on_hover_text("Where the PNG will sit in your Godot project");
                        ui.end_row();
                        ui.label("Animation:");
                        ui.add(egui::TextEdit::singleline(&mut s.anim_name).desired_width(130.0));
                        ui.end_row();
                    }
                }
                _ => {}
            }
            let folder = self.export_fmt == ExportFormat::Frames && !self.frames_zip;
            if !folder {
                ui.label("Name:");
                ui.horizontal(|ui| {
                    if ui.add(egui::TextEdit::singleline(&mut self.export_name).desired_width(170.0)).changed() {
                        self.export_name_edited = !self.export_name.trim().is_empty();
                    }
                    ui.label(RichText::new(format!(".{}", self.export_ext())).color(p().fg_muted));
                });
                ui.end_row();
            }
            ui.label("After saving:");
            if check(ui, &mut self.open_after_save, "Open the containing folder").changed() {
                theme::set_setting("open_after_save", if self.open_after_save { "1" } else { "0" });
            }
            ui.end_row();
        });
    }

    /// One line describing the compression settings in use.
    fn comp_summary(&self) -> String {
        let png = |o: &io::PngOptions| if o.colors == 0 { "PNG full colour".to_string() } else { format!("PNG {} colours", o.colors) };
        match self.export_fmt {
            ExportFormat::Gif => {
                let o = &self.gif_opts;
                format!("{} colours{} · lossy {}", o.colors, if o.global_palette { " (one palette)" } else { "" }, o.lossy)
            }
            ExportFormat::Webp => if self.webp_opts.lossless { "Lossless".into() } else { format!("Quality {:.0}", self.webp_opts.quality) },
            ExportFormat::Avif => format!("Quality {}", self.avif_quality),
            ExportFormat::Apng => "Lossless".into(),
            ExportFormat::Mp4 => "H.264 video".into(),
            ExportFormat::Webm => "VP9 video with alpha".into(),
            ExportFormat::Frames if self.split_fmt == SplitFormat::Gif && !self.frames_zip => format!("GIF frames · {} colours", self.gif_opts.colors),
            ExportFormat::Frames => png(&self.frames_png),
            ExportFormat::Sheet => png(&self.sheet.png),
        }
    }

    /// Format picker as a compact drop-down button.
    fn format_menu(&mut self, ui: &mut egui::Ui, h: f32) -> egui::Response {
        let r = ui.add(drop_button(ui, self.export_fmt.label(), p().fg).min_size(vec2(0.0, h))).on_hover_text("Output format");
        let mut pick = None;
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(200.0);
            for f in ExportFormat::ALL {
                let en = !f.needs_ffmpeg() || self.ffmpeg;
                let label = if en { RichText::new(f.label()) } else { RichText::new(format!("{}  (needs ffmpeg)", f.label())).color(p().fg_muted) };
                let b = ui.add(egui::Button::selectable(self.export_fmt == f, label));
                let b = if en { b.on_hover_text(f.tip()) } else { b.on_hover_text("Needs ffmpeg. Click to set it up.") };
                if b.clicked() {
                    if en {
                        pick = Some(f);
                    } else {
                        self.ff_open = true;
                        ui.close();
                    }
                }
            }
        });
        if let Some(f) = pick {
            self.export_fmt = f;
        }
        r
    }

    /// File name stem for the save dialog.
    fn default_stem(&self) -> String {
        let b = self.base_name();
        match self.export_fmt {
            ExportFormat::Gif => format!("{b}_edit"),
            ExportFormat::Frames => format!("{b}_frames"),
            ExportFormat::Sheet => format!("{b}_sheet"),
            _ => b,
        }
    }

    fn export_stem(&self) -> String {
        let n = self.export_name.trim();
        if self.export_name_edited && !n.is_empty() { n.to_string() } else { self.default_stem() }
    }

    fn export_ext(&self) -> &'static str {
        match self.export_fmt {
            ExportFormat::Gif => "gif",
            ExportFormat::Webp => "webp",
            ExportFormat::Avif => "avif",
            ExportFormat::Apng | ExportFormat::Sheet => "png",
            ExportFormat::Mp4 => "mp4",
            ExportFormat::Webm => "webm",
            ExportFormat::Frames => "zip",
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let pal = *p();
        let mut open = true;
        dialog(ctx, "about", "About GIFkit", vec2(400.0, 470.0), false, &mut open, |ui| {
                ui.set_width(340.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
                    paint_app_icon(ui.painter(), r, &pal);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("GIFkit").font(theme::medium(22.0)).color(pal.fg));
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(pal.fg));
                        let commit = env!("GIFKIT_COMMIT");
                        let built = if commit.is_empty() { format!("Built {}", env!("GIFKIT_BUILD_DATE")) } else { format!("Built {} · {commit}", env!("GIFKIT_BUILD_DATE")) };
                        hint(ui, &built);
                    });
                });
                ui.add_space(8.0);
                ui.label("A GIF editor and optimiser");
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.link("GitHub").clicked() {
                        open_external(REPO_URL);
                    }
                    ui.label("·");
                    if ui.link("Releases").clicked() {
                        open_external(&format!("{REPO_URL}/releases"));
                    }
                    ui.label("·");
                    if ui.link("Report a problem").clicked() {
                        open_external(&format!("{REPO_URL}/issues"));
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Credits").font(theme::medium(13.5)));
                hint(ui, "Made by zoeysmith37");
                hint(ui, "Built with Rust, egui, image, gif, libwebp and rayon");
                hint(ui, "Video and AVIF via ffmpeg (optional)");
                hint(ui, "Fonts: Noto Sans (OFL) and DejaVu");
                hint(ui, "Emoji: Twemoji (CC-BY 4.0) with names from emojibase (MIT)");
                hint(ui, "Icons: KDE Breeze (LGPL 3)");
                hint(ui, "Text fonts: open licence fonts from Google Fonts (OFL, Apache 2.0, UFL)");
                ui.add_space(4.0);
                hint(ui, "Licensed under Apache 2.0");
            });
        if !open {
            self.about_open = false;
        }
    }

    /// Every animation kept around (the opened one, the edit, undo steps,
    /// the screen-sized copies and the compressed preview).
    fn kept_anims_mut(&mut self) -> Vec<&mut Animation> {
        let mut v: Vec<&mut Animation> = Vec::new();
        v.extend(self.original.as_mut());
        v.extend(self.base.as_mut());
        for s in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            v.push(&mut s.anim);
        }
        if let Some(p) = self.proxy.as_mut() {
            v.push(&mut p.2);
            v.extend(p.3.as_mut());
        }
        if let Some(c) = self.cmp.as_mut() {
            v.push(&mut c.anim);
        }
        v
    }

    /// Pack kept frames that aren't yet, in the background, then put the
    /// packed ones in everywhere the same frame is kept.
    fn manage_packing(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(rx) = &self.pack_job {
            match rx.try_recv() {
                Ok(done) => {
                    self.pack_job = None;
                    let mut swap: HashMap<u64, crate::pixels::Pixels> = HashMap::new();
                    for (old, p) in done {
                        if p.is_packed() {
                            swap.insert(old, p);
                        } else {
                            self.pack_tried.insert(old);
                        }
                    }
                    for a in self.kept_anims_mut() {
                        for f in &mut a.frames {
                            if let Some(p) = swap.get(&f.image.id()) {
                                f.image = p.clone();
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    return;
                }
                Err(_) => self.pack_job = None,
            }
        }
        if now - self.pack_last < 1.0 {
            return;
        }
        self.pack_last = now;
        let tried = std::mem::take(&mut self.pack_tried);
        let mut seen = std::collections::HashSet::new();
        let mut todo: Vec<crate::pixels::Pixels> = Vec::new();
        for a in self.kept_anims_mut() {
            for f in &a.frames {
                let p = &f.image;
                if !p.is_packed() && p.width() * p.height() >= 64 * 64 && !tried.contains(&p.id()) && seen.insert(p.id()) {
                    todo.push(p.clone());
                }
            }
        }
        self.pack_tried = tried;
        if todo.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            stack::lower_thread_priority();
            let out: Vec<(u64, crate::pixels::Pixels)> = stack::bg_pool().install(|| {
                use rayon::prelude::*;
                todo.par_iter()
                    .map(|p| {
                        stack::yield_to_foreground(None);
                        (p.id(), p.packed())
                    })
                    .collect()
            });
            let _ = tx.send(out);
            ctx2.request_repaint();
        });
        self.pack_job = Some(rx);
    }

    /// GIFKIT_MEM: every few seconds, what each copy and cache holds (MB).
    fn mem_report(&mut self, ctx: &egui::Context) {
        if std::env::var_os("GIFKIT_MEM").is_none() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let last = ctx.data(|d| d.get_temp::<f64>(egui::Id::unique("mem-last"))).unwrap_or(-10.0);
        if now - last < 3.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            return;
        }
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("mem-last"), now));
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let mb = |b: usize| b as f64 / 1_048_576.0;
        let mut seen = std::collections::HashSet::new();
        let mut uniq = |a: &Animation| -> usize { a.frames.iter().filter(|f| seen.insert(f.image.id() as usize)).map(|f| f.image.stored_bytes()).sum() };
        let orig = self.original.as_ref().map(&mut uniq).unwrap_or(0);
        let base = self.base.as_ref().map(&mut uniq).unwrap_or(0);
        let proxy = self.proxy.as_ref().map(|p| uniq(&p.2) + p.3.as_ref().map(&mut uniq).unwrap_or(0)).unwrap_or(0);
        let undo: usize = self.undo.iter().chain(self.redo.iter()).map(|s| uniq(&s.anim)).sum();
        let cmp = self.cmp.as_ref().map(|c| uniq(&c.anim) + c.bytes.len()).unwrap_or(0);
        let tex = |t: &TexCache| t.bytes;
        let rss = std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).map(|l| l.split_whitespace().nth(1).unwrap_or("0").parse::<usize>().unwrap_or(0) * 1024)).unwrap_or(0);
        eprintln!(
            "mem: rss {:.0} | raw frames alive {:.0} | gpu frames {:.0} | unpacked cache {:.0} | original {:.0} base {:.0} proxy {:.0} undo/redo {:.0} ({} steps) compressed {:.0} | textures base {:.0} comp {:.0} orig {:.0} cmp {:.0} crop {:.0} thumbs {}",
            mb(rss), mb(crate::pixels::RAW_BYTES.load(Ordering::Relaxed)), mb(self.gpu_frames.bytes), mb(crate::pixels::cache_bytes()), mb(orig), mb(base), mb(proxy), mb(undo), self.undo.len() + self.redo.len(), mb(cmp),
            mb(tex(&self.base_tex)), mb(tex(&self.comp_tex)), mb(tex(&self.orig_tex)), mb(tex(&self.cmp_tex)), mb(tex(&self.crop_tex)), self.thumbs.map.len()
        );
    }

    /// Setting ffmpeg up: point at it, or (on Windows) download a copy.
    fn ffmpeg_window(&mut self, ctx: &egui::Context) {
        // a download finishing
        if let Some(r) = self.ff_job.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
            self.ff_job = None;
            match r {
                Ok(_) => self.ffmpeg_ready(ctx),
                Err(e) => self.ff_msg = Some((false, e)),
            }
        }
        if !self.ff_open {
            return;
        }
        let pal = *p();
        let mut open = true;
        let (mut download, mut locate, mut recheck, mut cancel, mut close) = (false, false, false, false, false);
        let mut ask = theme::get_setting("ffmpeg_ask").as_deref() != Some("no");
        let ask_before = ask;
        let found = crate::ffmpeg::path().filter(|_| self.ffmpeg);
        let job = self.ff_job.as_ref().map(|(pr, _)| pr.clone());
        let msg = self.ff_msg.clone();
        // after the main window is up, so it isn't lost behind it
        if ctx.cumulative_frame_nr() < 3 {
            ctx.request_repaint();
            return;
        }
        dialog_ex(ctx, "ffmpeg", "ffmpeg", vec2(470.0, 280.0), false, true, &mut open, |ui| {
            ui.set_width(440.0);
            let big = |ui: &mut egui::Ui, t: &str| ui.label(RichText::new(t).font(theme::medium(16.0)).color(pal.fg));
            if let Some(pr) = &job {
                big(ui, "Downloading ffmpeg…");
                ui.add_space(8.0);
                let (got, total) = (pr.got.load(std::sync::atomic::Ordering::Relaxed), pr.total.load(std::sync::atomic::Ordering::Relaxed));
                let mb = |b: u64| b as f32 / 1_048_576.0;
                if pr.unpacking.load(std::sync::atomic::Ordering::Relaxed) {
                    breeze_progress(ui, None, "Unpacking…");
                } else if total > 0 {
                    breeze_progress(ui, Some(got as f32 / total as f32), &format!("{:.0} of {:.0} MB", mb(got), mb(total)));
                } else {
                    breeze_progress(ui, None, &format!("{:.0} MB", mb(got)));
                }
                ui.add_space(4.0);
                if let Some(d) = crate::ffmpeg::own_dir() {
                    hint(ui, &format!("Going into {}", d.display()));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("Cancel").clicked();
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            if let Some(f) = &found {
                big(ui, "ffmpeg is set up");
                ui.add_space(6.0);
                ui.label("Videos open, and MP4, WebM and AVIF can be saved.");
                ui.add_space(4.0);
                hint(ui, &format!("Using {}", f.display()));
            } else {
                big(ui, "ffmpeg wasn't found");
                ui.add_space(6.0);
                ui.label("GIFkit uses ffmpeg to open videos and to save MP4, WebM and AVIF.");
                ui.add_space(10.0);
                if crate::ffmpeg::can_download() {
                    download = ui.add(egui::Button::new("Download ffmpeg").stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(0.0, 28.0))).clicked();
                    ui.add_space(10.0);
                }
                // the tool the command uses, past any sudo
                let tool = crate::ffmpeg::install_hint().split_whitespace().find(|w| *w != "sudo").unwrap_or("");
                ui.label(format!("Install via terminal ({tool}):"));
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let cmd = crate::ffmpeg::install_hint();
                    ui.label(RichText::new(cmd).monospace().color(pal.fg));
                    if ui.small_button("Copy").on_hover_text("Copy the install command").clicked() {
                        ui.ctx().copy_text(cmd.into());
                    }
                });
            }
            if let Some((ok, m)) = &msg {
                ui.add_space(6.0);
                ui.label(RichText::new(m).color(if *ok { pal.fg_muted } else { Color32::from_rgb(218, 68, 83) }));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let what = if cfg!(windows) { "Locate ffmpeg.exe…" } else { "Locate ffmpeg…" };
                locate = ui.button(if found.is_some() { "Use Another…" } else { what }).clicked();
                if found.is_none() {
                    recheck = ui.button("Check Again").on_hover_text("Look again, after installing it").clicked();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button(if found.is_some() { "Close" } else { "Not Now" }).clicked();
                });
            });
            if found.is_none() && cfg!(windows) {
                ui.add_space(6.0);
                ui.checkbox(&mut ask, "Ask when GIFkit starts");
            }
        });
        if ask != ask_before {
            theme::set_setting("ffmpeg_ask", if ask { "yes" } else { "no" });
        }
        if cancel {
            if let Some((pr, _)) = &self.ff_job {
                pr.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if download && self.ff_job.is_none() {
            self.ff_msg = None;
            let pr = Arc::new(crate::ffmpeg::Progress::default());
            let (tx, rx) = std::sync::mpsc::channel();
            let (pr2, ctx2) = (pr.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(crate::ffmpeg::download(pr2).map_err(|e| e.to_string()));
                ctx2.request_repaint();
            });
            self.ff_job = Some((pr, rx));
        }
        if locate {
            let mut d = rfd::FileDialog::new().set_title("Where is ffmpeg?");
            if cfg!(windows) {
                d = d.add_filter("ffmpeg", &["exe"]);
            }
            if let Some(f) = d.pick_file() {
                match crate::ffmpeg::use_file(&f) {
                    Ok(()) => self.ffmpeg_ready(ctx),
                    Err(e) => self.ff_msg = Some((false, e.to_string())),
                }
            }
        }
        if recheck {
            if crate::ffmpeg::recheck() {
                self.ffmpeg_ready(ctx);
            } else {
                self.ff_msg = Some((false, "Still not found. If you just installed it, try Locate, or restart GIFkit.".into()));
            }
        }
        if close || !open {
            if let Some((pr, _)) = &self.ff_job {
                pr.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.ff_open = false;
            self.ff_then = None;
        }
    }

    /// ffmpeg works now: turn on what needs it, and open the video that was waiting.
    fn ffmpeg_ready(&mut self, ctx: &egui::Context) {
        self.ffmpeg = true;
        crate::gpu::warm_up();
        self.ff_msg = None;
        self.info("ffmpeg is set up");
        if let Some(p) = self.ff_then.take() {
            self.ff_open = false;
            self.open_path_now(p, ctx);
        }
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        if !self.history_open || self.base.is_none() {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut jump: Option<i64> = None;
        let n_undo = self.undo.len();
        dialog(ctx, "history", "History", vec2(300.0, 440.0), true, &mut open, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let row = |ui: &mut egui::Ui, text: String, current: bool, future: bool| -> bool {
                            let col = if future { pal.fg_muted } else { pal.fg };
                            let t = RichText::new(elide(&text, 36)).color(col);
                            let t = if future { t.italics() } else { t };
                            ui.add(egui::Button::selectable(current, t).min_size(vec2(0.0, 24.0))).on_hover_text(text).clicked()
                        };
                        if row(ui, "Opened".into(), n_undo == 0, false) {
                            jump = Some(-(n_undo as i64));
                        }
                        for i in 0..n_undo {
                            let current = i + 1 == n_undo;
                            if row(ui, self.undo[i].label.clone(), current, false) {
                                jump = Some(i as i64 + 1 - n_undo as i64);
                            }
                        }
                        for (j, s) in self.redo.iter().rev().enumerate() {
                            if row(ui, s.label.clone(), false, true) {
                                jump = Some(j as i64 + 1);
                            }
                        }
                    });
                });
            });
        if !open {
            self.history_open = false;
        }
        if let Some(j) = jump {
            if j != 0 {
                self.history_jump(j);
            }
        }
    }

    fn after_base_change(&mut self, reset_tools: bool) {
        self.refresh_plan();
        let Some(a) = self.base.as_ref() else { return };
        let (w, h, n) = (a.width(), a.height(), a.len());
        let t = &mut self.stack.transform;
        if reset_tools || t.crop[2] == 0 || !t.crop_on {
            t.crop = [(w as f32 * 0.1) as u32, (h as f32 * 0.1) as u32, (w as f32 * 0.8) as u32, (h as f32 * 0.8) as u32];
        }
        if reset_tools && !t.cut_on {
            t.cut_start = 1;
            t.cut_end = 0;
            t.cut_t0 = 0.0;
            t.cut_t1 = a.duration_secs();
        }
        self.fr_to = self.fr_to.min(n).max(1);
        self.out_size = (w as f32, h as f32);
    }

    fn refresh_plan(&mut self) {
        let key = (self.version, self.timing_rev, self.stack.plan_signature());
        if key == self.plan_key {
            return;
        }
        self.plan_key = key;
        if let Some(b) = &self.base {
            self.plan = self.stack.plan(b);
            self.plan_times = stack::plan_times(&self.plan);
        } else {
            self.plan.clear();
            self.plan_times.clear();
        }
        if self.cur >= self.plan.len() {
            self.cur = self.plan.len().saturating_sub(1);
        }
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(0);
        self.stack.sync_len(n);
    }

    // ------------------------------------------------------------ layer selection

    /// Index of the selected layer of kind `k` (falls back to the topmost of that kind).
    fn sel_index(&self, k: Kind) -> Option<usize> {
        if let Some(id) = self.sel.get(kind_key(k)) {
            if let Some(i) = self.stack.index_of(*id) {
                if self.stack.layers[i].kind() == k {
                    return Some(i);
                }
            }
        }
        self.stack.layers.iter().rposition(|l| l.kind() == k)
    }

    fn select(&mut self, k: Kind, id: u64) {
        self.sel.insert(kind_key(k), id);
    }

    fn add_layer(&mut self, kind: LayerKind) -> u64 {
        let id = self.stack.push(kind);
        let k = self.stack.layers.last().map(|l| l.kind()).unwrap_or(Kind::Effects);
        self.select(k, id);
        self.modified = true;
        id
    }

    fn add_overlay(&mut self, src: OverlaySource) {
        let (aw, ah) = { let (w, h) = self.dims(); (w as f32, h as f32) };
        let (ow, oh) = src.native_size();
        let s = (aw * 0.4 / ow as f32).min(ah * 0.4 / oh as f32).min(1.0);
        let w = (ow as f32 * s).round().max(1.0) as u32;
        let h = (oh as f32 * s).round().max(1.0) as u32;
        let pl = Placement { enabled: true, x: ((aw as i32 - w as i32) / 2), y: ((ah as i32 - h as i32) / 2), w, h, angle: 0.0 };
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        self.add_layer(LayerKind::Overlay(OverlayLayer { src: Arc::new(src), placements: vec![pl; n], opacity: 100.0 }));
    }

    // ------------------------------------------------------------ file ops

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Animations, images, videos & projects", &["gif", "png", "apng", "webp", "jpg", "jpeg", "bmp", "avif", "mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "ogv", "3gp", crate::project::EXT])
            .add_filter("GIFkit projects", &[crate::project::EXT])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.open_path(p, ctx);
        }
    }

    fn open_sequence_dialog(&mut self, ctx: &egui::Context) {
        if let Some(mut ps) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).pick_files() {
            ps.sort();
            self.open_sequence(ps, ctx);
        }
    }

    fn open_path(&mut self, p: PathBuf, ctx: &egui::Context) {
        // opening another file replaces this one: ask first if it has changes
        if self.base.is_some() && self.modified {
            self.confirm = Some(Pending::Open(p));
            return;
        }
        self.open_path_now(p, ctx);
    }

    fn open_path_now(&mut self, p: PathBuf, ctx: &egui::Context) {
        self.add_recent(&p);
        if crate::project::is_project(&p) {
            self.spawn_in(ctx, "Open project", 1, false, move |_| JobOutput::ProjectLoaded(crate::project::load(&p).map(|pr| (pr, p.clone()))));
            return;
        }
        if io::is_video(&p) && !self.ffmpeg {
            // set ffmpeg up, then open it
            self.ff_then = Some(p);
            self.ff_open = true;
            self.info("Opening videos needs ffmpeg");
            return;
        }
        if io::is_video(&p) {
            // ask about range, frame rate and size first
            match io::probe_video(&p) {
                Ok(info) => {
                    let mut opt = io::VideoImport::defaults(&info);
                    // scripting hook (tests): GIFKIT_VIDEO_IMPORT=side,delay,merge imports right away
                    if let Some(v) = std::env::var("GIFKIT_VIDEO_IMPORT").ok() {
                        let n: Vec<u32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                        if let [side, delay, merge] = n[..] {
                            (opt.max_side, opt.delay_cs, opt.merge) = (side, delay as u16, merge as usize);
                            self.import_video(ctx, p, opt);
                            return;
                        }
                    }
                    self.video_import = Some((p, info, opt));
                }
                Err(e) => self.error(format!("{e:#}")),
            }
            return;
        }
        self.spawn(ctx, "Open", 1, move |_| {
            let size = std::fs::metadata(&p).ok().map(|m| m.len());
            JobOutput::Loaded(io::load_animation(&p).map(|a| (a, Some(p.clone()), size)))
        });
    }

    /// Import a video with the chosen range, frame rate and size.
    fn import_video(&mut self, ctx: &egui::Context, p: PathBuf, opt: io::VideoImport) {
        let total = opt.frames();
        self.spawn_in(ctx, "Importing video", total, false, move |pr| {
            // a video's size says nothing about GIF sizes, so don't compare against it
            JobOutput::Loaded(io::load_video_range(&p, &opt, Some(pr)).map(|a| (a, Some(p.clone()), None)))
        });
    }

    fn video_import_window(&mut self, ctx: &egui::Context) {
        let Some((path, info, mut opt)) = self.video_import.clone() else { return };
        let pal = *p();
        let mut open = true;
        let mut go = false;
        let mut cancel = false;
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        dialog(ctx, "video", "Import Video", vec2(460.0, 400.0), false, &mut open, |ui| {
            ui.label(RichText::new(elide(&name, 48)).font(theme::medium(15.0)).color(pal.fg));
            hint(ui, &format!("{}×{} · {:.2} fps · {}", info.width, info.height, info.fps, fmt_secs(info.duration)));
            ui.add_space(10.0);
            form_row(ui, "Start:", |ui| num_fill(ui, &mut opt.start, 0.1, |d| d.range(0.0..=info.duration).speed(0.05).max_decimals(2).suffix(" s")));
            form_row(ui, "End:", |ui| num_fill(ui, &mut opt.end, 0.1, |d| d.range(0.0..=info.duration).speed(0.05).max_decimals(2).suffix(" s")));
            if opt.end <= opt.start {
                opt.end = (opt.start + 0.1).min(info.duration);
            }
            let rates: Vec<(u16, String)> = io::VIDEO_DELAYS.iter().map(|d| (*d, format!("{:.1} fps", 100.0 / *d as f64).replace(".0 ", " "))).collect();
            let items: Vec<(u16, &str)> = rates.iter().map(|(d, l)| (*d, l.as_str())).collect();
            form_row(ui, "Frame rate:", |ui| combo(ui, "vid-fps", &mut opt.delay_cs, &items)).on_hover_text("GIFs can only show these rates exactly");
            let sizes: Vec<(u32, String)> = [240u32, 320, 480, 640, 800, 1080, 0]
                .iter()
                .filter(|m| **m == 0 || **m < info.width.max(info.height))
                .map(|m| {
                    let o = io::VideoImport { max_side: *m, ..opt.clone() };
                    let (w, h) = o.size(&info);
                    (*m, if *m == 0 { format!("Original ({w}×{h})") } else { format!("{w}×{h}") })
                })
                .collect();
            let items: Vec<(u32, &str)> = sizes.iter().map(|(m, l)| (*m, l.as_str())).collect();
            form_row(ui, "Size:", |ui| combo(ui, "vid-size", &mut opt.max_side, &items));
            let levels: Vec<(usize, &str)> = io::MERGE_LEVELS.iter().enumerate().map(|(i, l)| (i, l.0)).collect();
            form_row(ui, "Still parts:", |ui| combo(ui, "vid-merge", &mut opt.merge, &levels)).on_hover_text(
                "Frames that barely change from the one before reuse its picture, so they take less memory \
                 and the saved file is smaller. Effects and animations still move over them. \
                 Screen recordings are still most of the time, so this saves a lot.\n\n\
                 Repeated: only frames that look the same.\nSmall changes: also a moving cursor or a few typed letters.\nMost changes: anything but big movement.",
            );
            ui.add_space(8.0);
            let gb = opt.bytes(&info) as f64 / 1e9;
            let frames = opt.frames();
            let mem = if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{:.0} MB", gb * 1000.0) };
            let merging = opt.merge > 0;
            form_row(ui, "Result:", |ui| hint(ui, &if merging { format!("Up to {frames} frames · up to {mem} in memory, less where the video is still") } else { format!("{frames} frames · {mem} in memory") }));
            if gb > 4.0 {
                let tip = if merging { "⚠ That could be a lot of memory if the video moves a lot. A shorter range, a lower frame rate or a smaller size will be quicker to edit." } else { "⚠ That's a lot of memory. A shorter range, a lower frame rate, a smaller size or merging still parts will be much quicker to edit." };
                form_row(ui, "", |ui| ui.label(RichText::new(tip).color(pal.neutral)));
            }
            ui.add_space(12.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::image_and_text(crate::icons::image(ui.ctx(), "dialog-cancel", 16.0, pal.fg), "Cancel").min_size(vec2(96.0, 30.0))).clicked() {
                        cancel = true;
                    }
                    if ui.add(egui::Button::image_and_text(crate::icons::image(ui.ctx(), "document-import", 16.0, pal.fg), "Import").stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(96.0, 30.0))).clicked() {
                        go = true;
                    }
                });
            });
        });
        if go {
            self.video_import = None;
            self.import_video(ctx, path, opt);
        } else if !open || cancel {
            self.video_import = None;
        } else if let Some(v) = self.video_import.as_mut() {
            v.2 = opt;
        }
    }

    fn open_sequence(&mut self, ps: Vec<PathBuf>, ctx: &egui::Context) {
        if self.base.is_some() && self.modified {
            self.confirm = Some(Pending::Sequence(ps));
            return;
        }
        let n = ps.len();
        self.spawn(ctx, "Open sequence", n, move |_| JobOutput::Loaded(io::load_sequence(&ps, 10).map(|a| (a, None, None))));
    }

    fn base_name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "animation".into())
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let mut dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        // Ctrl+V with files or a browser picture on the clipboard
        match self.paste_in.take() {
            Some(crate::paste::Pasted::Files(ps)) if !self.busy() => {
                if self.base.is_some() {
                    if let Some(p) = ps.into_iter().next() {
                        self.load_overlay(p, ctx);
                    }
                    return;
                }
                dropped.extend(ps);
            }
            Some(crate::paste::Pasted::Url(url, still)) if !self.busy() => {
                let has_base = self.base.is_some();
                self.spawn(ctx, "Fetch the pasted picture", 1, move |_| {
                    let fetched = crate::paste::download(&url).and_then(|p| {
                        let a = io::load_animation(&p);
                        let _ = std::fs::remove_file(&p);
                        a
                    });
                    // the still copy from the clipboard when it can't be fetched
                    let a = match (fetched, still) {
                        (Ok(a), _) => Ok(a),
                        (Err(_), Some(img)) => Ok(Animation { frames: vec![crate::anim::Frame::new(img, 10)], loop_count: None }),
                        (Err(e), None) => Err(e),
                    };
                    if has_base {
                        JobOutput::Overlay(a.map(|a| OverlaySource { name: "Pasted image".into(), path: None, delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image.get()).clone()).collect() }))
                    } else {
                        JobOutput::Loaded(a.map(|a| (a, None, None)))
                    }
                });
                return;
            }
            p => self.paste_in = p.filter(|_| self.busy()),
        }
        // Ctrl+V: copied files (a file manager's "Copy"), pasted paths, or a
        // copied picture. With nothing open they open; otherwise each one
        // becomes an image layer.
        if !ctx.egui_wants_keyboard_input() && !self.busy() {
            let (texts, image) = ctx.input(|i| {
                let t: Vec<String> = i.events.iter().filter_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }).collect();
                let img = i.events.iter().find_map(|e| if let egui::Event::PasteImage(c) = e { Some(c.clone()) } else { None });
                (t, img)
            });
            let pasted: Vec<PathBuf> = texts.iter().flat_map(|t| paths_from_text(t)).collect();
            if let Some(c) = image.filter(|_| pasted.is_empty()) {
                let [w, h] = c.size;
                let raw: Vec<u8> = c.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
                if let Some(img) = RgbaImage::from_raw(w as u32, h as u32, raw) {
                    if self.base.is_some() {
                        self.add_overlay(OverlaySource { name: "Pasted image".into(), path: None, delays: vec![10], frames: vec![img] });
                        self.tool = Tool::AddImage;
                        self.info("Pasted as an image layer – drag it on the preview to position it");
                    } else {
                        let a = Animation { frames: vec![crate::anim::Frame::new(img, 10)], loop_count: None };
                        self.spawn(ctx, "Paste", 1, move |_| JobOutput::Loaded(Ok((a, None, None))));
                    }
                }
                return;
            }
            if !pasted.is_empty() {
                if self.base.is_some() {
                    // one job at a time: the first file goes in
                    if let Some(p) = pasted.into_iter().next() {
                        self.load_overlay(p, ctx);
                    }
                    return;
                }
                dropped.extend(pasted);
            }
        }
        if dropped.is_empty() || self.busy() {
            return;
        }
        if self.base.is_some() && self.tool == Tool::AddImage {
            self.load_overlay(dropped[0].clone(), ctx);
        } else if self.base.is_some() && self.tool == Tool::Frames {
            let mut ps = dropped;
            ps.sort();
            let d = self.fr_delay;
            self.run_anim(ctx, "Add frames", move |a, _| {
                let imgs = ps.iter().map(|p| io::load_still(p)).collect::<anyhow::Result<Vec<_>>>()?;
                Ok(ops::append_images(a, imgs, d))
            });
        } else if dropped.len() > 1 {
            let mut ps = dropped;
            ps.sort();
            self.open_sequence(ps, ctx);
        } else {
            self.open_path(dropped[0].clone(), ctx);
        }
    }

    fn load_overlay(&mut self, p: PathBuf, ctx: &egui::Context) {
        self.spawn(ctx, "Load overlay", 1, move |_| {
            JobOutput::Overlay(io::load_animation(&p).map(|a| OverlaySource {
                name: p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                path: Some(p.clone()),
                delays: a.frames.iter().map(|f| f.delay_cs).collect(),
                frames: a.frames.iter().map(|f| (*f.image.get()).clone()).collect(),
            }))
        });
    }

    // ------------------------------------------------------------ keyboard

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use crate::keys::Action as A;
        // while a new key is being picked in the shortcuts window, keys go there
        if self.key_capture.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let km = self.keymap.clone();
        let hit = |a: A| ctx.input(|i| km.pressed(i, a));
        let (undo, redo, play, left, right, fit, one, zin, zout, cmp) = (hit(A::Undo), hit(A::Redo), hit(A::PlayPause), hit(A::PrevFrame), hit(A::NextFrame), hit(A::Fit), hit(A::ActualSize), hit(A::ZoomIn), hit(A::ZoomOut), hit(A::SplitView));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.export_open = false;
            self.shortcuts_open = false;
            self.about_open = false;
        }
        if self.busy() {
            return;
        }
        if hit(A::New) {
            self.new_project();
        }
        if hit(A::Quit) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if hit(A::History) {
            self.history_open = !self.history_open;
        }
        if hit(A::Open) {
            self.open_dialog(ctx);
        }
        if hit(A::Export) && self.base.is_some() {
            self.export_open = true;
        }
        if self.base.is_some() {
            if hit(A::SaveAs) {
                self.save_project(ctx, true);
            } else if hit(A::Save) {
                self.save_project(ctx, false);
            }
        }
        if hit(A::FullScreen) {
            let on = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
        }
        if hit(A::OnionSkin) && !typing {
            self.toggle_onion();
        }
        if hit(A::Handbook) {
            self.handbook_open = true;
        }
        // letters belong to an open menu, and Alt+letter to the menu bar
        let alt = ctx.input(|i| i.modifiers.alt);
        if !egui::Popup::is_any_open(ctx) {
            ctx.data_mut(|d| d.remove::<bool>(mnemonics_id()));
        }
        if typing || alt || egui::Popup::is_any_open(ctx) {
            return;
        }
        if undo {
            self.do_undo();
        }
        // Delete over the picture removes the selected text, shape, emoji…
        let over_view = ctx.pointer_hover_pos().is_some_and(|p| self.views_last.iter().any(|r| r.contains(p)));
        if over_view && hit(A::DeleteObject) {
            if let Some(li) = self.tool.layer_kind().and_then(|k| self.sel_index(k)).filter(|li| !self.stack.layers[*li].locked) {
                let now = ctx.input(|i| i.time);
                self.snapshot(&format!("del{now}"), now);
                let name = self.stack.layers[li].name();
                self.stack.layers.remove(li);
                self.modified = true;
                self.info(format!("Deleted {name}"));
            }
        }
        if redo {
            self.do_redo();
        }
        if play {
            self.playing = !self.playing;
        }
        if left {
            self.step(-1);
        }
        if right {
            self.step(1);
        }
        if fit {
            self.zoom_fit();
        }
        if one {
            self.zoom_to(1.0);
        }
        if zin {
            self.zoom_to(self.cur_scale() * 1.25);
        }
        if zout {
            self.zoom_to(self.cur_scale() / 1.25);
        }
        if cmp && self.base.is_some() {
            self.compare = !self.compare;
        }
        // copy, paste and duplicate away from the timeline (it does its own
        // while the pointer is on it)
        let over_tl = ctx.pointer_hover_pos().is_some_and(|p| self.tl_area.contains(p));
        if self.base.is_some() && !over_tl {
            let now = ctx.input(|i| i.time);
            let cut = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Cut))) && !ctx.egui_wants_keyboard_input();
            if cut {
                self.edit_cut(ctx, now);
            }
            let (copy, paste_layers, paste_clips) = ctx.input(|i| {
                (
                    i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(t) if t == LAYERS_TAG)),
                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(t) if t == timeline_ui::BOARD_TAG)),
                )
            });
            if copy {
                self.edit_copy(ctx);
            }
            if paste_layers {
                self.paste_layers(now);
            }
            if paste_clips && self.tl_focus {
                self.tl_paste(now);
            }
            if hit(A::Duplicate) {
                self.edit_duplicate(ctx, now);
            }
        }
        // in and out points, markers and jumps between edits work anywhere
        if self.base.is_some() {
            let now = ctx.input(|i| i.time);
            if hit(A::TlIn) {
                self.tl_set_in(Some(self.cur));
            }
            if hit(A::TlOut) {
                self.tl_set_out(Some(self.cur));
            }
            if hit(A::TlMarker) {
                self.tl_add_marker(now);
            }
            if hit(A::TlPrevEdit) {
                self.tl_jump(false);
            }
            if hit(A::TlNextEdit) {
                self.tl_jump(true);
            }
        }
    }

    fn step(&mut self, d: i32) {
        if self.plan.is_empty() {
            return;
        }
        self.playing = false;
        let n = self.plan.len() as i32;
        self.cur = (self.cur as i32 + d).rem_euclid(n) as usize;
        self.elapsed = 0.0;
    }

    // ------------------------------------------------------------ zoom / pan

    /// Viewport that zoom buttons / keys act on.
    fn active(&self) -> usize {
        if self.compare { self.active_view.clamp(1, 2) } else { 0 }
    }
    fn targets(&self, vi: usize) -> Vec<usize> {
        if self.link_views && vi > 0 { vec![1, 2] } else { vec![vi] }
    }
    fn scale_of(&self, vi: usize) -> f32 {
        self.views[vi].zoom.unwrap_or(self.views[vi].fit)
    }
    fn cur_scale(&self) -> f32 {
        self.scale_of(self.active())
    }
    fn zoom_fit_v(&mut self, vi: usize) {
        for t in self.targets(vi) {
            self.views[t].zoom = None;
            self.views[t].pan = Vec2::ZERO;
        }
    }
    fn zoom_fit(&mut self) {
        self.zoom_fit_v(self.active());
    }
    fn zoom_to(&mut self, s: f32) {
        self.zoom_to_v(self.active(), s);
    }
    fn zoom_to_v(&mut self, vi: usize, s: f32) {
        let old = self.scale_of(vi);
        let s = s.clamp(0.02, 64.0);
        for t in self.targets(vi) {
            self.views[t].pan = self.views[vi].pan * (s / old);
            self.views[t].zoom = Some(s);
        }
    }
    fn pan_by(&mut self, vi: usize, d: Vec2) {
        for t in self.targets(vi) {
            if t == vi || self.link_views {
                self.views[t].pan += d;
            }
        }
    }
    /// Zoom by `factor` keeping the image point under `pointer` fixed.
    fn zoom_at(&mut self, vi: usize, pointer: Pos2, viewport_center: Pos2, factor: f32) {
        let s = self.scale_of(vi);
        let ns = (s * factor).clamp(0.02, 64.0);
        let c = viewport_center + self.views[vi].pan;
        let u = (pointer - c) / s;
        let nc = pointer - u * ns;
        let pan = nc - viewport_center;
        for t in self.targets(vi) {
            self.views[t].pan = pan;
            self.views[t].zoom = Some(ns);
        }
    }

    // ------------------------------------------------------------ playback

    fn advance(&mut self, ctx: &egui::Context) {
        if !self.playing || self.plan.is_empty() || !self.plan.iter().any(|f| f.enabled) {
            self.last_tick = None;
            // paused: the original waits where the edit is, to start from there
            self.orig_clock = self.paused_original().and_then(|i| self.original.as_ref().map(|o| stack::frame_start_time(o, i))).unwrap_or(0.0) as f64;
            return;
        }
        // Real elapsed time. egui's stable_dt falls back to a fixed 1/60 s
        // guess after delayed (non-immediate) repaints, which made playback
        // crawl whenever the mouse was idle.
        let now = ctx.input(|i| i.time);
        let dt = match self.last_tick {
            Some(t) => (now - t).clamp(0.0, 0.25),
            None => 0.0,
        };
        self.last_tick = Some(now);
        self.elapsed += dt * self.play_speed as f64;
        if let Some(o) = &self.original {
            let total = o.duration_secs() as f64;
            self.orig_clock = if total > 0.0 { (self.orig_clock + dt * self.play_speed as f64).rem_euclid(total) } else { 0.0 };
        }
        let n = self.plan.len();
        if !self.plan[self.cur].enabled {
            self.cur = self.next_enabled(self.cur);
        }
        // Keep to real time, as a video player does: when the screen can't
        // show every frame (big frames, a slow graphics driver), frames are
        // skipped instead of the animation playing in slow motion. dt is
        // capped above, so a stall never races ahead.
        let mut steps = 0;
        loop {
            let d = effective_delay(self.plan[self.cur].delay) as f64 / 100.0;
            if self.elapsed < d || steps > n {
                break;
            }
            self.elapsed -= d;
            self.cur = self.next_play(self.cur);
            steps += 1;
        }
        let d = effective_delay(self.plan[self.cur].delay) as f64 / 100.0;
        let remain = ((d - self.elapsed) / self.play_speed.max(0.05) as f64).max(0.004);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(remain));
    }

    /// The frame after `from` in playback: inside the in and out points
    /// when looping between them.
    fn next_play(&self, from: usize) -> usize {
        let next = self.next_enabled(from);
        if let Some((a, b)) = self.tl_zone().filter(|_| self.tl_zone_loop) {
            if next > b || next < a || next <= from {
                // back to the first shown frame of the zone
                return (a..=b).find(|&i| self.plan.get(i).is_some_and(|f| f.enabled)).unwrap_or(next);
            }
        }
        next
    }

    /// The in and out points, when both are set and in order.
    fn tl_zone(&self) -> Option<(usize, usize)> {
        let n = self.plan.len();
        match (self.tl_in, self.tl_out) {
            (None, None) => None,
            (a, b) => {
                let (a, b) = (a.unwrap_or(0).min(n.saturating_sub(1)), b.unwrap_or(n.saturating_sub(1)).min(n.saturating_sub(1)));
                (a <= b).then_some((a, b))
            }
        }
    }

    fn next_enabled(&self, from: usize) -> usize {
        let n = self.plan.len();
        for k in 1..=n {
            let i = (from + k) % n;
            if self.plan[i].enabled {
                return i;
            }
        }
        from
    }

    fn play_time(&self) -> f32 {
        self.plan_times.get(self.cur).copied().unwrap_or(0.0) + self.elapsed as f32
    }

    // ------------------------------------------------------------ textures

    fn tex_opts(&self) -> TextureOptions {
        if self.pixelated {
            TextureOptions::NEAREST
        } else {
            TextureOptions::LINEAR
        }
    }

    fn base_texture(&mut self, ctx: &egui::Context, src: usize) -> Option<TextureHandle> {
        if self.waiting_proxy() {
            return None;
        }
        let key = hash_parts(&[self.version, self.pixelated as u64, self.ready_proxy().is_some() as u64]);
        if let Some(t) = self.base_tex.get(key, src) {
            return Some(t);
        }
        let img = match self.ready_proxy() {
            Some((_, a)) => a.frames.get(src)?.image.get(),
            None => self.base.as_ref()?.frames.get(src)?.image.get(),
        };
        let o = self.tex_opts();
        Some(self.base_tex.put(ctx, src, &img, o, "base"))
    }

    /// A frame's thumbnail if it's ready; otherwise it's asked for and made
    /// in the background (from the shrunk copy of big pictures when there is one).
    fn thumb_texture(&mut self, _ctx: &egui::Context, idx: usize) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(self.version, idx) {
            return Some(t);
        }
        let img = match self.ready_proxy() {
            Some((_, a)) => a.frames.get(idx)?.image.get(),
            None => self.base.as_ref()?.frames.get(idx)?.image.get(),
        };
        self.thumbs.want(idx, img);
        None
    }

    fn composite_key(&self) -> u64 {
        let proxy = self.ready_proxy().map(|(s, _)| s.to_bits() as u64).unwrap_or(0);
        hash_parts(&[self.stack.signature(), self.version, self.timing_rev, self.pixelated as u64, self.plan.len() as u64, self.stack.plan_signature(), proxy, self.tl_solo.unwrap_or(u64::MAX)])
    }

    /// How much smaller preview frames are than the source (1 = full size).
    fn proxy_scale(&self) -> f32 {
        let (w, h) = self.dims();
        let side = w.max(h) as f32;
        if self.preview_max == 0 || side <= self.preview_max as f32 * 1.15 {
            1.0
        } else {
            self.preview_max as f32 / side
        }
    }

    fn proxy_key(&self) -> u64 {
        hash_parts(&[self.version, self.orig_gen, self.proxy_scale().to_bits() as u64])
    }

    /// The shrunk frames, when they're ready for the current source.
    fn ready_proxy(&self) -> Option<(f32, &Animation)> {
        let s = self.proxy_scale();
        if s >= 1.0 {
            return None;
        }
        let key = self.proxy_key();
        self.proxy.as_ref().filter(|p| p.0 == key).map(|p| (p.1, &p.2))
    }

    /// A big source whose shrunk copy is still being made: views wait for it
    /// (a moment) instead of shrinking full-size frames on the UI thread.
    fn waiting_proxy(&self) -> bool {
        self.proxy_scale() < 1.0 && self.ready_proxy().is_none() && self.proxy_job.is_some()
    }

    /// The original's frames for the screen: shrunk when the source is big.
    fn orig_small(&self) -> Option<&Animation> {
        let key = self.proxy_key();
        self.proxy.as_ref().filter(|p| p.0 == key && self.proxy_scale() < 1.0).and_then(|p| p.3.as_ref())
    }

    /// Make the shrunk frames in the background whenever the source changes.
    fn manage_proxy(&mut self, ctx: &egui::Context) {
        let s = self.proxy_scale();
        if let Some((k, rx)) = &self.proxy_job {
            match rx.try_recv() {
                Ok((a, o)) => {
                    let k = *k;
                    self.proxy_job = None;
                    if k == self.proxy_key() {
                        self.proxy = Some((k, s, a, o));
                        ctx.request_repaint();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
                Err(_) => self.proxy_job = None,
            }
        }
        if s >= 1.0 {
            self.proxy = None;
            return;
        }
        let key = self.proxy_key();
        if self.proxy.as_ref().is_some_and(|p| p.0 == key) || self.proxy_job.as_ref().is_some_and(|j| j.0 == key) {
            return;
        }
        let Some(base) = self.base.clone() else { return };
        let original = self.original.clone();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            stack::lower_thread_priority();
            let out = stack::bg_pool().install(|| {
                let shrink = |a: &Animation| {
                    let (w, h) = stack::proxy_size(a.width(), a.height(), s);
                    let px: Vec<&crate::pixels::Pixels> = a.frames.iter().map(|f| &f.image).collect();
                    let small = stack::shrink_pixels(&px, w, h);
                    let frames = a.frames.iter().zip(small).map(|(f, i)| crate::anim::Frame { image: i, ..f.clone() }).collect();
                    Animation { frames, loop_count: a.loop_count }
                };
                let small = shrink(&base);
                // the original mostly shares the source's frames: reuse those
                let orig = original.map(|o| {
                    let (w, h) = stack::proxy_size(o.width(), o.height(), s);
                    let shared: Vec<Option<usize>> = o.frames.iter().map(|f| base.frames.iter().position(|b| crate::pixels::Pixels::ptr_eq(&b.image, &f.image))).collect();
                    let own: Vec<&crate::pixels::Pixels> = o.frames.iter().zip(&shared).filter(|(_, k)| k.is_none()).map(|(f, _)| &f.image).collect();
                    let mut made = stack::shrink_pixels(&own, w, h).into_iter();
                    let frames = o
                        .frames
                        .iter()
                        .zip(&shared)
                        .map(|(f, k)| match k {
                            Some(k) => crate::anim::Frame { image: small.frames[*k].image.clone(), ..f.clone() },
                            None => crate::anim::Frame { image: made.next().unwrap_or_else(|| crate::pixels::Pixels::new(image::imageops::resize(&*f.image.get(), w, h, image::imageops::FilterType::Triangle))), ..f.clone() },
                        })
                        .collect();
                    Animation { frames, loop_count: o.loop_count }
                });
                (small, orig)
            });
            let _ = tx.send(out);
            ctx2.request_repaint();
        });
        self.proxy_job = Some((key, rx));
    }

    /// What previews render from: the shrunk frames with a matching stack
    /// when the source is big, else the source itself.
    fn render_inputs(&self) -> Option<(Stack, Animation)> {
        let (mut st, a) = match self.ready_proxy() {
            Some((s, a)) => (self.stack.proxy(s), a.clone()),
            None => (self.stack.clone(), self.base.clone()?),
        };
        // a soloed layer shows on its own in the live view (not in exports)
        if let Some(id) = self.tl_solo {
            for l in &mut st.layers {
                l.visible = l.id == id;
            }
        }
        Some((st, a))
    }

    /// The output size in full-size pixels (previews may be smaller).
    fn true_out_size(&self) -> Vec2 {
        let (w, h) = self.dims();
        let g = self.stack.eff_transform(true).geo_map(w, h);
        vec2(g.w.round(), g.h.round())
    }

    /// Layer geometry / sprites for the current stack, cached per composite key.
    fn prepared(&mut self) -> Option<Arc<stack::Prepared>> {
        let key = self.comp_key;
        let (st, base) = self.render_inputs()?;
        if let Some((k, p)) = &self.prep_cache {
            // the key is from the start of this frame: a panel may have
            // added or removed a layer since
            if *k == key && p.fits(&st) {
                return Some(p.clone());
            }
        }
        let p = Arc::new(st.prepare(&base, &self.fonts));
        self.prep_cache = Some((key, p.clone()));
        Some(p)
    }

    fn render_composite(&mut self, i: usize) -> Option<RgbaImage> {
        if i >= self.plan.len() {
            return None;
        }
        let prep = self.prepared()?;
        let (st, base) = self.render_inputs()?;
        let t0 = std::time::Instant::now();
        let img = st.render_frame(&base, &self.plan, &self.plan_times, i, &self.fonts, &prep);
        self.last_render_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Some(img)
    }

    /// Frame `i` with effects and layers but without crop / resize / rotate,
    /// so the crop tool can show the whole frame (outside the crop dimmed).
    fn crop_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        let base = self.base.as_ref()?;
        let mut st = self.stack.clone();
        st.transform.crop_on = false;
        st.transform.resize_on = false;
        st.transform.rot_on = false;
        if !st.needs_render() && !self.has_blank() {
            return None;
        }
        let key = hash_parts(&[st.signature(), self.version, self.timing_rev, self.pixelated as u64, 0xC20F]);
        if let Some(t) = self.crop_tex.get(key, i) {
            return Some(t);
        }
        let prep = match &self.crop_prep {
            Some((k, p)) if *k == key => p.clone(),
            _ => {
                let p = Arc::new(st.prepare(base, &self.fonts));
                self.crop_prep = Some((key, p.clone()));
                p
            }
        };
        if i >= self.plan.len() {
            return None;
        }
        let img = st.render_frame(base, &self.plan, &self.plan_times, i, &self.fonts, &prep);
        // effects that change the frame size (borders, polaroid) can't line up with crop coordinates
        if img.dimensions() != (base.width(), base.height()) {
            return None;
        }
        let o = self.tex_opts();
        Some(self.crop_tex.put(ctx, i, &img, o, "crop"))
    }

    /// Texture of output frame `i` with all live edits applied.
    /// The frame rendered on the graphics card, when it can draw everything
    /// in the stack (else None and the CPU path takes over).
    fn gpu_composite(&mut self, ctx: &egui::Context, i: usize) -> Option<Tex> {
        if self.gpu.is_none() || !self.stack.gpu_capable() || !crate::gpu_render::enabled() {
            return None;
        }
        let key = self.comp_key;
        // the card is still busy with the last one: keep it up a moment longer
        // (while a slider is dragged; playing, the picture keeps moving)
        if !self.playing && (self.gpu_frames.key != key || !self.gpu_frames.map.contains_key(&i)) {
            if let (Some((gpu, _)), Some((_, t))) = (&self.gpu, &self.last_comp) {
                if gpu.busy() && self.gpu_frames.since_busy < 30 {
                    self.gpu_frames.since_busy += 1;
                    let t = t.clone();
                    ctx.request_repaint_after(std::time::Duration::from_millis(4));
                    return Some(t);
                }
            }
            self.gpu_frames.since_busy = 0;
        }
        if self.gpu_frames.key != key {
            self.free_gpu_frames();
            self.gpu_frames.key = key;
        }
        self.gpu_frames.tick += 1;
        let tick = self.gpu_frames.tick;
        if let Some(e) = self.gpu_frames.map.get_mut(&i) {
            e.2 = tick;
            return Some(Tex { id: e.1, size: vec2(e.0.width() as f32, e.0.height() as f32), _keep: None });
        }
        let prep = self.prepared()?;
        let (st, base) = self.render_inputs()?;
        let src = self.plan.get(i)?.src;
        let job = st.gpu_frame(&base, &self.plan, &self.plan_times, i, &self.fonts, &prep)?;
        let frame = &*base.frames.get(src)?.image.get();
        let frame_key = hash_parts(&[self.version, src as u64, base.width() as u64, base.height() as u64]);
        let filter = if self.pixelated { eframe::wgpu::FilterMode::Nearest } else { eframe::wgpu::FilterMode::Linear };
        let (gpu, renderer) = self.gpu.as_mut()?;
        if !job.fits(frame, gpu.max_side) {
            return None;
        }
        let tex = gpu.render(frame, frame_key, &job, key);
        let view = tex.create_view(&Default::default());
        let id = renderer.write().register_native_texture(&gpu.device, &view, filter);
        let size = vec2(tex.width() as f32, tex.height() as f32);
        let bytes = tex.width() as usize * tex.height() as usize * 4;
        // keep to a budget, dropping what was looked at longest ago
        while self.gpu_frames.bytes + bytes > crate::pixels::scaled(GPU_FRAMES_BUDGET) && !self.gpu_frames.map.is_empty() {
            let oldest = *self.gpu_frames.map.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| k).unwrap();
            if let Some((t, id, _)) = self.gpu_frames.map.remove(&oldest) {
                self.gpu_frames.bytes -= t.width() as usize * t.height() as usize * 4;
                if self.last_comp.as_ref().is_some_and(|c| c.1.id == id) {
                    if let Some((old, oid)) = self.gpu_frames.shown.replace((t, id)) {
                        renderer.write().free_texture(&oid);
                        old.destroy();
                    }
                } else {
                    renderer.write().free_texture(&id);
                    t.destroy();
                }
            }
        }
        self.gpu_frames.bytes += bytes;
        self.gpu_frames.map.insert(i, (tex, id, tick));
        Some(Tex { id, size, _keep: None })
    }

    /// Playing: unpack the source frames coming up in the background, and
    /// have the card draw the next frame before it's due, so frames that
    /// aren't kept (long animations) never hold the picture up.
    fn read_ahead(&mut self, ctx: &egui::Context) {
        if !self.playing || self.plan.is_empty() {
            return;
        }
        let src_anim = match self.ready_proxy() {
            Some((_, a)) => Some(a),
            None => self.base.as_ref(),
        };
        if let Some(a) = src_anim {
            let mut j = self.cur;
            for _ in 0..8 {
                j = self.next_enabled(j);
                if let Some(f) = self.plan.get(j).and_then(|p| a.frames.get(p.src)) {
                    f.image.prefetch();
                }
            }
        }
        // the next frame on the card, one a screen refresh
        if self.needs_render() && self.gpu.is_some() && self.gpu_frames.key == self.comp_key {
            let mut j = self.cur;
            for _ in 0..2 {
                j = self.next_enabled(j);
                if !self.gpu_frames.map.contains_key(&j) {
                    let ready = self.render_inputs().and_then(|(_, b)| self.plan.get(j).and_then(|p| b.frames.get(p.src)).map(|f| f.image.is_ready())).unwrap_or(false);
                    // not while its picture is still being unpacked
                    if ready {
                        let _ = self.gpu_composite(ctx, j);
                    }
                    break;
                }
            }
        }
    }

    fn free_gpu_frames(&mut self) {
        let Some((_, renderer)) = &self.gpu else { return };
        let mut r = renderer.write();
        let shown = self.last_comp.as_ref().map(|c| c.1.id);
        for (_, (t, id, _)) in self.gpu_frames.map.drain() {
            if Some(id) == shown {
                // swap it in as the one kept on screen
                if let Some((old, oid)) = self.gpu_frames.shown.replace((t, id)) {
                    r.free_texture(&oid);
                    old.destroy();
                }
            } else {
                r.free_texture(&id);
                t.destroy();
            }
        }
        self.gpu_frames.bytes = 0;
    }

    fn toggle_onion(&mut self) {
        self.onion = !self.onion;
        theme::set_setting("onion", if self.onion { "1" } else { "0" });
        self.info(if self.onion { "Onion skin on: frames before show red, the next one green" } else { "Onion skin off" });
    }

    /// Onion skin: two frames back in red and one ahead in green, fainter
    /// further off. Not while playing (it would only smear).
    fn onion_frames(&mut self, ctx: &egui::Context) -> Vec<(Tex, Color32)> {
        let n = self.plan.len();
        let keep = self.last_comp.clone();
        let v = self.onion_of(self.cur, n, |me, i| me.composite_texture(ctx, i));
        // what the main view shows stays the frame at the playhead
        self.last_comp = keep;
        v
    }

    /// The onion skin around frame `i` of `n`, with `get` making a frame's picture.
    fn onion_of(&mut self, i: usize, n: usize, mut get: impl FnMut(&mut Self, usize) -> Option<Tex>) -> Vec<(Tex, Color32)> {
        if !self.onion || self.playing || n < 2 {
            return Vec::new();
        }
        let mut v = Vec::new();
        for (d, tint) in [(-2i64, Color32::from_rgba_unmultiplied(255, 90, 90, 28)), (-1, Color32::from_rgba_unmultiplied(255, 90, 90, 58)), (1, Color32::from_rgba_unmultiplied(90, 230, 140, 58))] {
            let j = i as i64 + d;
            if j < 0 || j >= n as i64 {
                continue;
            }
            if let Some(t) = get(self, j as usize) {
                v.push((t, tint));
            }
        }
        v
    }

    fn composite_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<Tex> {
        if self.waiting_proxy() {
            return None;
        }
        if !self.needs_render() {
            let src = self.plan.get(i)?.src;
            return self.base_texture(ctx, src).map(Tex::from);
        }
        if let Some(t) = self.gpu_composite(ctx, i) {
            self.last_comp = Some((i, t.clone()));
            return Some(t);
        }
        let key = self.comp_key;
        if let Some(t) = self.comp_tex.get(key, i).map(Tex::from) {
            self.last_comp = Some((i, t.clone()));
            return Some(t);
        }
        // While playing and the background pre-render is filling the cache, keep
        // showing the last frame instead of stalling the UI thread on a render.
        if self.playing && self.prerender.as_ref().map(|p| p.key == key).unwrap_or(false) {
            if let Some((li, t)) = &self.last_comp {
                if *li != i || self.comp_tex.map.is_empty() {
                    return Some(t.clone());
                }
            }
        }
        // a render that keeps the screen waiting goes to the background, and
        // the last picture stays up until it's done
        if let Some((k, ji, rx)) = &self.live_job {
            if let Ok(img) = rx.try_recv() {
                let (k, ji) = (*k, *ji);
                self.live_job = None;
                let o = self.tex_opts();
                if std::env::var_os("GIFKIT_PERF").is_some() {
                    eprintln!("live render: frame {ji} {}", if k == key { "current" } else { "stale" });
                }
                if k == key {
                    let t = Tex::from(self.comp_tex.put(ctx, ji, &img, o, "comp"));
                    if ji == i {
                        self.last_comp = Some((i, t.clone()));
                        return Some(t);
                    }
                } else if ji == i {
                    // made for settings that have moved on since (a slider
                    // still being dragged): show it anyway, as the newest there
                    // is, and start on the current settings straight away
                    let t = Tex::from(ctx.load_texture("comp-live", to_color_image(&img), o));
                    self.last_comp = Some((i, t));
                }
            } else if *k == key && *ji == i {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
                return self.last_comp.as_ref().map(|c| c.1.clone());
            }
        }
        // big pictures always render in the background
        if self.last_render_ms > 40.0 || self.proxy_scale() < 1.0 {
            if self.live_job.is_none() {
                let prep = self.prepared()?;
                let (st, base) = self.render_inputs()?;
                let (plan, times, fonts) = (self.plan.clone(), self.plan_times.clone(), self.fonts.clone());
                let (tx, rx) = channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let img = st.render_frame(&base, &plan, &times, i, &fonts, &prep);
                    let _ = tx.send(img);
                    ctx2.request_repaint();
                });
                self.live_job = Some((key, i, rx));
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return self.last_comp.as_ref().map(|c| c.1.clone());
        }
        let img = self.render_composite(i)?;
        let o = self.tex_opts();
        let t = Tex::from(self.comp_tex.put(ctx, i, &img, o, "comp"));
        self.last_comp = Some((i, t.clone()));
        Some(t)
    }

    /// Track composite key changes; once stable, pre-render all frames in the background.
    fn manage_prerender(&mut self, ctx: &egui::Context, now: f64) {
        let key = self.composite_key();
        if key != self.comp_key {
            self.comp_key = key;
            self.comp_key_since = now;
            ctx.request_repaint();
            self.prerender = None;
            self.prerender_cancel.store(key, Ordering::Relaxed);
        }
        // frames the graphics card renders are made as they're shown
        if self.gpu.is_some() && self.stack.gpu_capable() && crate::gpu_render::enabled() {
            self.prerender = None;
            return;
        }
        let mut done = false;
        if let Some(pr) = &self.prerender {
            let mut got = Vec::new();
            loop {
                match pr.rx.try_recv() {
                    Ok(m) => {
                        got.push(m);
                        if got.len() >= 24 {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        done = true;
                        break;
                    }
                }
            }
            let o = self.tex_opts();
            let pkey = pr.key;
            for (k, i, img) in got {
                if k == self.comp_key && pkey == k && self.comp_tex.get(k, i).is_none() {
                    let _ = self.comp_tex.put(ctx, i, &img, o, "comp");
                }
            }
            if !done {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if done {
            self.prerender = None;
            self.comp_key_since = f64::INFINITY;
        }
        // playing past what the cache holds: render the next stretch
        if self.playing && self.prerender.is_none() && self.comp_key_since.is_infinite() && self.needs_render() && !self.plan.is_empty() {
            let next = self.next_enabled(self.cur);
            if !(self.comp_tex.key == key && self.comp_tex.map.contains_key(&next)) {
                self.comp_key_since = now - 1.0;
            }
        }
        let stable = now - self.comp_key_since > 0.5;
        if self.prerender.is_none() && stable && self.needs_render() && !self.plan.is_empty() {
            // as many as the texture budget holds
            let room = TexCache::room_for(self.true_out_size().x as u32, self.true_out_size().y as u32);
            let cached = self.comp_tex.key == key && self.comp_tex.map.len() >= self.plan.len().min(room);
            if cached {
                return;
            }
            let Some((stack, base)) = self.render_inputs() else { return };
            let plan = self.plan.clone();
            let times = self.plan_times.clone();
            let fonts = self.fonts.clone();
            // render in playback order, starting just after the frame on screen
            let n = plan.len();
            let start = self.cur + 1;
            let missing: Vec<usize> = (0..n).map(|k| (start + k) % n).take(room).filter(|i| !(self.comp_tex.key == key && self.comp_tex.map.contains_key(i))).collect();
            let cancel = self.prerender_cancel.clone();
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            let prep = self.prepared();
            std::thread::spawn(move || {
                let pool = stack::bg_pool();
                let prep = prep.unwrap_or_else(|| Arc::new(stack.prepare(&base, &fonts)));
                let next = AtomicUsize::new(0);
                pool.scope(|sc| {
                    for _ in 0..pool.current_num_threads() {
                        let tx = tx.clone();
                        let (next, missing, stack, base, plan, times, fonts, prep, cancel, ctx2) = (&next, &missing, &stack, &base, &plan, &times, &fonts, &prep, &cancel, &ctx2);
                        sc.spawn(move |_| loop {
                            let j = next.fetch_add(1, Ordering::Relaxed);
                            if j >= missing.len() || cancel.load(Ordering::Relaxed) != key {
                                break;
                            }
                            let i = missing[j];
                            let img = stack.render_frame(base, plan, times, i, fonts, prep);
                            if tx.send((key, i, img)).is_err() {
                                break;
                            }
                            // batch repaints instead of one per finished frame
                            ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                        });
                    }
                });
                drop(tx);
            });
            self.prerender = Some(Prerender { key, rx });
        } else if self.prerender.is_none() && !stable && self.needs_render() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    // ------------------------------------------------------------ compare / compressed output

    /// The opened GIF, when saving it as GIF would change nothing: no layers,
    /// edits, timeline changes or new settings. It's then kept as it is.
    /// The selected layer of the open tool.
    fn sel_layer_index(&self) -> Option<usize> {
        self.tool.layer_kind().and_then(|k| self.sel_index(k))
    }

    /// Copy the selected layer (kept here; the clipboard gets a tag so
    /// Ctrl+V comes back to it).
    fn copy_layer(&mut self, ctx: &egui::Context) {
        let Some(li) = self.sel_layer_index() else { return };
        let l = self.stack.layers[li].clone();
        let name = l.name();
        self.layer_board = vec![l];
        ctx.copy_text(LAYERS_TAG.into());
        self.info(format!("Copied {name}"));
    }

    /// Paste copied layers above the others, a little down and to the right.
    fn paste_layers(&mut self, now: f64) {
        if self.layer_board.is_empty() || self.base.is_none() {
            return;
        }
        self.snapshot(&format!("paste{now}"), now);
        let board = self.layer_board.clone();
        let mut last = None;
        for src in board {
            let mut kind = src.kind.clone();
            offset_layer(&mut kind, 12);
            let id = self.stack.push(kind);
            if let Some(l) = self.stack.layers.iter_mut().find(|l| l.id == id) {
                l.label = src.label.clone();
                l.visible = src.visible;
                l.timing = src.timing.clone();
                l.color = src.color;
                l.opacity = src.opacity;
                l.clip = src.clip;
                last = Some((l.kind(), id));
            }
        }
        if let Some((k, id)) = last {
            self.select(k, id);
            self.tool = kind_tool(k);
        }
        // the next paste lands a little further on
        for l in &mut self.layer_board {
            offset_layer(&mut l.kind, 12);
        }
        self.modified = true;
    }

    /// Edit ▸ Copy, Paste, Duplicate and Delete: the timeline's clips when it
    /// was clicked last, else the selected layer.
    fn edit_copy(&mut self, ctx: &egui::Context) {
        if self.tl_focus {
            self.tl_copy(ctx)
        } else {
            self.copy_layer(ctx)
        }
    }

    fn edit_cut(&mut self, ctx: &egui::Context, now: f64) {
        if self.tl_focus {
            self.tl_cut_clips(ctx, now);
        } else if let Some(li) = self.sel_layer_index().filter(|li| !self.stack.layers[*li].locked) {
            self.copy_layer(ctx);
            self.apply_layer_act(ctx, LayerAct::Delete(li), now);
        }
    }

    fn edit_paste(&mut self, now: f64) {
        if self.tl_focus {
            self.tl_paste(now)
        } else {
            self.paste_layers(now)
        }
    }

    fn edit_duplicate(&mut self, ctx: &egui::Context, now: f64) {
        if self.tl_focus {
            self.tl_duplicate(now);
        } else if let Some(li) = self.sel_layer_index() {
            self.apply_layer_act(ctx, LayerAct::Duplicate(li), now);
        }
    }

    fn edit_delete(&mut self, ctx: &egui::Context, now: f64) {
        if self.tl_focus {
            self.tl_delete_selected(now);
        } else if let Some(li) = self.sel_layer_index().filter(|li| !self.stack.layers[*li].locked) {
            self.apply_layer_act(ctx, LayerAct::Delete(li), now);
        }
    }

    /// Gaps on the timeline's GIF track (frames that show nothing).
    fn has_blank(&self) -> bool {
        self.base.as_ref().is_some_and(|b| b.frames.iter().any(|f| f.blank))
    }

    /// Frames need drawing: layers, frame edits, or gaps on the GIF track.
    fn needs_render(&self) -> bool {
        self.stack.needs_render() || self.has_blank()
    }

    fn untouched_gif(&self) -> Option<PathBuf> {
        if self.export_fmt != ExportFormat::Gif || self.gif_opts.clone() != (GifOptions { noai_tag: self.gif_opts.noai_tag, ..self.gif_opts_open.clone() }) {
            return None;
        }
        let path = self.path.as_ref().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif")))?;
        let (base, orig) = (self.base.as_ref()?, self.original.as_ref()?);
        let same = base.len() == orig.len() && base.frames.iter().zip(&orig.frames).all(|(a, b)| a.enabled && !a.blank && crate::pixels::Pixels::ptr_eq(&a.image, &b.image) && a.delay_cs == b.delay_cs);
        let plain = self.stack.layers.iter().all(|l| !l.visible) && self.stack.transform.is_identity() && self.stack.loop_count(base) == orig.loop_count;
        // the in and out points, or see-through frames at the end, change it
        let (range, cut) = self.export_trim();
        let trimmed = range.is_some() || (cut && base.frames.iter().rev().find(|f| f.enabled).is_some_and(|f| f.image.is_fully_transparent()));
        let plain = plain && !trimmed;
        (same && plain).then(|| path.clone())
    }

    /// What export leaves out: everything outside the in and out points
    /// (when asked), and see-through frames at the end (when asked).
    fn export_trim(&self) -> (Option<(usize, usize)>, bool) {
        (self.tl_zone().filter(|_| self.export_zone_only), self.export_cut_empty)
    }

    fn cmp_key(&self) -> u64 {
        let (range, cut) = self.export_trim();
        hash_parts(&[self.stack.signature(), self.version, self.timing_rev, hash_str(&format!("{:?}{range:?}{cut}", self.preview_format())), self.stack.plan_signature()])
    }

    /// The output format the compressed preview shows (shared with Export).
    fn preview_format(&self) -> Option<io::PreviewFormat> {
        use io::PreviewFormat as P;
        Some(match self.export_fmt {
            ExportFormat::Gif => P::Gif(self.gif_opts.clone()),
            ExportFormat::Webp => P::Webp(self.webp_opts),
            ExportFormat::Avif if self.ffmpeg => P::Avif(self.avif_quality),
            ExportFormat::Apng => P::Apng,
            ExportFormat::Mp4 if self.ffmpeg => P::Video(VideoFormat::Mp4, self.video_bg),
            ExportFormat::Webm if self.ffmpeg => P::Video(VideoFormat::WebmAlpha, [0, 0, 0]),
            ExportFormat::Frames => {
                let fmt = if self.frames_zip { SplitFormat::Png } else { self.split_fmt };
                P::Frames(fmt, self.frames_png, self.gif_opts.clone())
            }
            ExportFormat::Sheet => P::Sheet(self.sheet.clone()),
            _ => return None,
        })
    }

    fn wants_compressed(&self) -> bool {
        let single = !self.compare || self.tool == Tool::Frames;
        let shown = if single { self.view_single == ViewSource::Compressed } else { self.view_left == ViewSource::Compressed || self.view_right == ViewSource::Compressed };
        self.preview_format().is_some() && (shown || self.tool == Tool::Optimize || self.export_open)
    }

    fn manage_compare(&mut self, ctx: &egui::Context, now: f64) {
        // an edit made the running one out of date: stop it rather than let
        // it finish for nothing while the user works
        if let Some(j) = &self.cmp_job {
            if j.key != self.cmp_key() {
                j.stop.fetch_or(stack::STOP_BIT, Ordering::Relaxed);
                self.cmp_job = None;
                if std::env::var_os("GIFKIT_PERF").is_some() {
                    eprintln!("compress: stopped, out of date");
                }
            }
        }
        if let Some(j) = &self.cmp_job {
            match j.rx.try_recv() {
                Ok(res) => {
                    let (key, sheet, j_stop) = (j.key, j.sheet, j.stop.clone());
                    self.cmp_job = None;
                    match res {
                        Ok((anim, bytes, sample, size, full, span)) => {
                            self.cmp_gen += 1;
                            self.cmp = Some(CmpResult { key, sheet, anim, bytes: Arc::new(bytes), gen: self.cmp_gen, sample, size, full, span });
                        }
                        Err(e) if stack::stopped(Some(&j_stop)) => {
                            let _ = e;
                        }
                        Err(e) => self.error(format!("Compression preview failed: {e:#}")),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(60)),
                Err(_) => self.cmp_job = None,
            }
        }
        if !self.wants_compressed() || self.base.is_none() {
            return;
        }
        let key = self.cmp_key();
        if key != self.cmp_key_seen {
            self.cmp_key_seen = key;
            self.cmp_key_since = now;
        }
        let up_to_date = self.cmp.as_ref().map(|c| c.key == key).unwrap_or(false);
        if up_to_date || self.cmp_job.is_some() {
            return;
        }
        // wait until edits settle (and the user isn't mid-drag)
        // wait until edits have settled and nothing is being dragged / held
        let busy_input = ctx.input(|i| i.pointer.any_down()) || ctx.dragged_id().is_some() || self.box_drag.is_some() || self.drag.is_some();
        if busy_input {
            self.cmp_key_since = now;
        }
        // half a second after the last change, so quick tweaks don't each start an encode
        if now - self.cmp_key_since < 0.5 || busy_input {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
            return;
        }
        // nothing changed: the preview is the file itself
        if let Some(path) = self.untouched_gif() {
            let (orig, noai) = (self.original.clone().unwrap(), self.gif_opts.noai_tag);
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let res = std::fs::read(&path).map_err(anyhow::Error::from).map(|b| {
                    let b = if noai { crate::encode::tag_gif(b) } else { b };
                    let full = (orig.width(), orig.height());
                    let side = full.0.max(full.1);
                    let anim = if side > TEX_MAX_SIDE {
                        let (w, h) = stack::proxy_size(full.0, full.1, TEX_MAX_SIDE as f32 / side as f32);
                        {
                            let px: Vec<&crate::pixels::Pixels> = orig.frames.iter().map(|f| &f.image).collect();
                            let small = stack::shrink_pixels(&px, w, h);
                            Animation { frames: orig.frames.iter().zip(small).map(|(f, i)| crate::anim::Frame { image: i, ..f.clone() }).collect(), loop_count: orig.loop_count }
                        }
                    } else {
                        orig
                    };
                    let n = b.len() as u64;
                    let d = anim.duration_secs();
                    (anim, b, None, n, full, (0.0, d))
                });
                let _ = tx.send(res);
                ctx2.request_repaint();
            });
            self.cmp_job = Some(CmpJob { key, sheet: false, stop: Arc::new(AtomicUsize::new(0)), rx });
            return;
        }
        let base = self.base.clone().unwrap();
        let stack = self.stack.clone();
        let fonts = self.fonts.clone();
        let Some(pf) = self.preview_format() else { return };
        let is_sheet = matches!(pf, io::PreviewFormat::Sheet(_));
        // long or large animations: encode a few stretches and estimate the rest
        let n = self.plan.len();
        // the output size (not out_size, which only follows the Edit view)
        let os = self.true_out_size();
        let pixels = n as u64 * os.x as u64 * os.y as u64;
        // only frames that are shown: skipped ones are left out of the file, so
        // sampling them would leave the preview with gaps
        let (range, cut_empty) = self.export_trim();
        let (ra, rb) = range.unwrap_or((0, n.saturating_sub(1)));
        let shown: Vec<usize> = (ra..=rb.min(n.saturating_sub(1))).filter(|&i| self.plan[i].enabled).collect();
        let sample: Option<Vec<usize>> = (self.sample_preview && !is_sheet && self.cmp_full_key != key && pixels > SAMPLE_PIXELS && shown.len() > 60).then(|| {
            const RUNS: usize = 6;
            const LEN: usize = 8;
            let m = shown.len();
            let mut v: Vec<usize> = (0..RUNS).flat_map(|k| {
                let start = k * (m - LEN) / (RUNS - 1);
                shown[start..start + LEN].to_vec()
            }).collect();
            v.dedup();
            v
        });
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        let stop = Arc::new(AtomicUsize::new(0));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            stack::lower_thread_priority();
            let stop = Some(&*stop2);
            let res = stack::bg_pool().install(|| {
                let t0 = std::time::Instant::now();
                let mut span = (0.0f32, 0.0f32);
                let out = match &sample {
                    None => {
                        let (a, s) = stack.flatten_for_export(&base, &fonts, stop, range, cut_empty);
                        span = s;
                        a
                    }
                    Some(idx) => {
                        let plan = stack.plan(&base);
                        let times = stack::plan_times(&plan);
                        let prep = stack.prepare(&base, &fonts);
                        let frames = stack.render_frames(&base, &plan, &times, idx, &fonts, &prep, stop);
                        let mut a = Animation { frames, loop_count: stack.loop_count(&base) };
                        crate::effects::normalize_sizes(&mut a);
                        a
                    }
                };
                let t1 = t0.elapsed();
                anyhow::ensure!(!stack::stopped(stop), "Stopped");
                let (anim, bytes) = io::encode_preview(&out, &pf, stop)?;
                // shrink big results here, off the UI thread, for the screen
                let full = (anim.width(), anim.height());
                let side = full.0.max(full.1);
                let anim = if side > TEX_MAX_SIDE {
                    let (w, h) = stack::proxy_size(full.0, full.1, TEX_MAX_SIDE as f32 / side as f32);
                    {
                            let px: Vec<&crate::pixels::Pixels> = anim.frames.iter().map(|f| &f.image).collect();
                            let small = stack::shrink_pixels(&px, w, h);
                            Animation { frames: anim.frames.iter().zip(small).map(|(f, i)| crate::anim::Frame { image: i, ..f.clone() }).collect(), loop_count: anim.loop_count }
                        }
                } else {
                    anim
                };
                let t2 = t0.elapsed();
                if std::env::var_os("GIFKIT_PERF").is_some() {
                    eprintln!("compress: flatten {:.1}s encode {:.1}s decode {:.1}s sampled {}", t1.as_secs_f32(), (t2 - t1).as_secs_f32(), (t0.elapsed() - t2).as_secs_f32(), sample.is_some());
                }
                // scale the sample's size up by the share of shown time it covers
                let size = match &sample {
                    None => bytes.len() as u64,
                    Some(_) => {
                        let plan = stack.plan(&base);
                        let total: u64 = plan.iter().enumerate().filter(|(i, f)| f.enabled && range.is_none_or(|(a, b)| *i >= a && *i <= b)).map(|(_, f)| f.delay.max(1) as u64).sum();
                        let part: u64 = out.frames.iter().filter(|f| f.enabled).map(|f| f.delay_cs.max(1) as u64).sum();
                        (bytes.len() as f64 * total as f64 / part.max(1) as f64) as u64
                    }
                };
                Ok((anim, bytes, sample, size, full, span))
            });
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
        self.cmp_job = Some(CmpJob { key, sheet: is_sheet, stop, rx });
    }

    fn orig_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        if self.waiting_proxy() {
            return None;
        }
        let key = hash_parts(&[self.orig_gen, self.pixelated as u64, self.orig_small().is_some() as u64]);
        if let Some(t) = self.orig_tex.get(key, i) {
            return Some(t);
        }
        let img = match self.orig_small() {
            Some(o) => o.frames.get(i)?.image.get(),
            None => self.original.as_ref()?.frames.get(i)?.image.get(),
        };
        let o = self.tex_opts();
        Some(self.orig_tex.put(ctx, i, &img, o, "orig"))
    }

    fn cmp_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        let c = self.cmp.as_ref()?;
        let key = hash_parts(&[c.gen, self.pixelated as u64]);
        if let Some(t) = self.cmp_tex.get(key, i) {
            return Some(t);
        }
        let img = c.anim.frames.get(i)?.image.get();
        let o = self.tex_opts();
        Some(self.cmp_tex.put(ctx, i, &img, o, "cmp"))
    }

    /// What sits behind transparent pixels: `vis` is the visible part of `img_rect`.
    fn paint_backdrop(&mut self, painter: &egui::Painter, ctx: &egui::Context, vis: Rect, img_rect: Rect) {
        match self.backdrop {
            Backdrop::Checker => {
                let ct = self.checker(ctx);
                let uv = Rect::from_min_max(((vis.min - img_rect.min) / 16.0).to_pos2(), ((vis.max - img_rect.min) / 16.0).to_pos2());
                painter.image(ct.id(), vis, uv, Color32::WHITE);
            }
            Backdrop::Solid => {
                let c = self.backdrop_col;
                painter.rect_filled(vis, 0.0, Color32::from_rgb(c[0], c[1], c[2]));
            }
            Backdrop::Canvas => {}
        }
    }

    fn checker(&mut self, ctx: &egui::Context) -> TextureHandle {
        if self.checker_tex.is_none() {
            let a = Color32::from_gray(0xcc);
            let b = Color32::from_gray(0x99);
            let img = egui::ColorImage::new([2, 2], vec![a, b, b, a]);
            self.checker_tex = Some(ctx.load_texture("checker", img, TextureOptions::NEAREST_REPEAT));
        }
        self.checker_tex.clone().unwrap()
    }

    fn dims(&self) -> (u32, u32) {
        self.base.as_ref().map(|a| (a.width(), a.height())).unwrap_or((1, 1))
    }
}

// ================================================================ UI

impl eframe::App for GifKitApp {
    /// A normal exit: nothing to restore next time.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.clear_autosave();
    }

    /// Ctrl+C / X / V arrive as keys (clipboard shortcuts are off in the
    /// window options): copy and cut become egui's events, and paste reads
    /// the clipboard here, so files and browser pictures come in whole.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !raw.events.iter().any(|e| matches!(e, egui::Event::Key { modifiers, .. } if modifiers.command)) {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let mut out = Vec::with_capacity(raw.events.len());
        for e in std::mem::take(&mut raw.events) {
            match e {
                // Ctrl+Shift+C is Copy Result, not Copy
                egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt && !modifiers.shift => out.push(egui::Event::Copy),
                egui::Event::Key { key: egui::Key::X, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt => out.push(egui::Event::Cut),
                egui::Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt => {
                    let (events, extra) = crate::paste::read(typing);
                    out.extend(events);
                    if extra.is_some() {
                        self.paste_in = extra;
                    }
                }
                // their releases go too, as with egui's own handling
                egui::Event::Key { key: k @ (egui::Key::C | egui::Key::X | egui::Key::V), modifiers, .. } if modifiers.command && !modifiers.alt && !(k == egui::Key::C && modifiers.shift) => {}
                e => out.push(e),
            }
        }
        raw.events = out;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // what was clicked last decides what Copy and Paste work on
        if let Some(p) = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.press_origin()).flatten()) {
            // not the menu bar or an open menu: those act on what was clicked before
            let in_menu = self.menubar_rect.contains(p) || ctx.layer_id_at(p).is_some_and(|l| l.order != egui::Order::Background);
            if !in_menu {
                self.tl_focus = self.tl_area.contains(p);
            }
        }
        let perf_t0 = std::time::Instant::now();
        self.mem_report(&ctx);
        self.smoke_step(&ctx);
        stack::mark_ui_thread();
        // textures follow the size pictures were shown at last frame
        let need = TEX_NEED.swap(0, Ordering::Relaxed);
        if need > 0 {
            let side = need.next_power_of_two().clamp(512, TEX_MAX_SIDE);
            // only grow at once; shrink when it's well under, so a zoom
            // back and forth doesn't redo every texture
            let cur = TEX_SIDE.load(Ordering::Relaxed);
            if side > cur || side * 2 < cur {
                TEX_SIDE.store(side, Ordering::Relaxed);
            }
        }
        // background work gives way while the user is clicking, dragging,
        // scrolling or typing (and for a moment after)
        if ctx.input(|i| i.pointer.any_down() || i.events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::MouseWheel { .. } | egui::Event::Text(_)))) {
            stack::foreground_busy(300);
        }
        theme::refresh(&ctx, self.theme);
        self.guard_close(&ctx);
        self.thumbs.pump(&ctx);
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true) || std::env::var_os("GIFKIT_ASSUME_FOCUS").is_some();
        // a dialog of ours having focus still counts as the app being active
        let dialog_open = self.tl_paste_ask.is_some() || self.handbook_open || self.batch.open || self.export_open || self.history_open || self.about_open || self.ff_open || self.restore_offer.is_some() || self.shortcuts_open || self.video_import.is_some();
        theme::set_focused(&ctx, focused || dialog_open, self.theme);
        self.views_last = self.drop_views.drain(..).map(|v| v.0).collect();
        // text that follows the canvas (wrapping, centring, caption bars)
        if self.base.is_some() {
            let (w, h) = self.dims();
            self.stack.layout_text(w, h, &self.fonts);
        }
        // KDE caption: "document — app", with a star while there are unsaved changes
        let title = match self.project_path.as_ref().or(self.path.as_ref()).and_then(|p| p.file_name()) {
            Some(n) if self.base.is_some() => format!("{}{} — GIFkit", n.to_string_lossy(), if self.modified { " *" } else { "" }),
            None if self.base.is_some() => format!("Untitled{} — GIFkit", if self.modified { " *" } else { "" }),
            _ => "GIFkit".to_string(),
        };
        if title != self.win_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.win_title = title;
        }
        theme::set_current(&ctx);
        let now = ctx.input(|i| i.time);
        // GIFKIT_PERF: name the slow steps of a slow frame
        let perf = std::env::var_os("GIFKIT_PERF").is_some();
        let mut lap = std::time::Instant::now();
        let mut mark = |what: &str| {
            if perf && lap.elapsed().as_millis() > 25 {
                eprintln!("  slow: {what} {:.0} ms", lap.elapsed().as_secs_f64() * 1000.0);
            }
            lap = std::time::Instant::now();
        };
        self.poll_job(&ctx);
        mark("jobs");
        self.handle_dropped(&ctx);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::C)) {
            self.copy_result(&ctx);
        }
        self.shortcuts(&ctx);
        self.refresh_plan();
        mark("plan");
        self.advance(&ctx);
        self.manage_proxy(&ctx);
        mark("proxy");
        self.manage_prerender(&ctx, now);
        self.read_ahead(&ctx);
        mark("prerender");
        self.manage_compare(&ctx, now);
        mark("compare");
        self.manage_packing(&ctx, now);
        self.manage_autosave(now);
        mark("packing");
        self.load_preview_fonts(&ctx);
        mark("fonts");
        let pal = *p();

        // KDE layout: menu bar, then the main toolbar, both on the header colour
        let mb = egui::Panel::top("menubar")
            .frame(egui::Frame::new().fill(pal.header).inner_margin(egui::Margin { left: 4, right: 4, top: 2, bottom: 0 }))
            .show(ui, |ui| self.menubar(ui, &ctx));
        self.menubar_rect = mb.response.rect;
        if self.show_toolbar {
            egui::Panel::top("toolbar")
                .frame(egui::Frame::new().fill(pal.header).inner_margin(egui::Margin { left: 6, right: 8, top: 3, bottom: 4 }))
                .show(ui, |ui| self.toolbar(ui, &ctx));
        }

        if self.show_statusbar {
            egui::Panel::bottom("status").frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(10, 3))).show(ui, |ui| self.status_bar(ui));
        }

        // the names slide in and out
        let names = theme::openness(&ctx, egui::Id::unique("rail-names"), !self.sidebar_compact);
        let rail_w = (40.0 + 100.0 * names).round();
        if names > 0.0 && names < 1.0 {
            ctx.request_repaint();
        }
        if self.base.is_some() {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(360.0)
                .size_range(300.0..=(ctx.content_rect().width() - rail_w - 440.0).clamp(300.0, 640.0))
                .frame(egui::Frame::new().fill(pal.window))
                .show(ui, |ui| {
                    // the tool's settings always keep at least half the height
                    let lmax = (ui.available_height() * 0.5).clamp(140.0, 800.0);
                    // it slides open and shut between its header and the height it had
                    let t = theme::openness(&ctx, egui::Id::unique("layers-open"), !self.layers_collapsed);
                    let open_h = self.layers_open_h.clamp(140.0, lmax);
                    let mut panel = egui::Panel::bottom("layers_panel")
                        .resizable(!self.layers_collapsed)
                        .default_size(380.0f32.min(lmax))
                        .size_range(if self.layers_collapsed { 34.0..=34.0 } else { 140.0..=lmax });
                    let sliding = t > 0.0 && t < 1.0;
                    if sliding {
                        panel = panel.resizable(false).exact_size(34.0 + (open_h - 34.0) * t);
                        ctx.request_repaint();
                    }
                    let shown = panel
                        .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 2 }))
                        .show(ui, |ui| {
                            if !sliding {
                                self.layer_panel(ui, &ctx);
                                return;
                            }
                            // drawn at its open height and clipped, so it can't push the panel taller
                            let r = ui.max_rect();
                            let full = Rect::from_min_size(r.min, vec2(r.width(), (open_h - 8.0).max(r.height())));
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                            child.set_clip_rect(ui.clip_rect().intersect(r));
                            self.layer_panel(&mut child, &ctx);
                        });
                    if !sliding && !self.layers_collapsed {
                        self.layers_open_h = shown.response.rect.height();
                    }
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 10, right: 8, top: 8, bottom: 4 })).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(vec2(22.0, 24.0), Sense::hover());
                            paint_tool_icon(ui.painter(), self.tool, ir.center(), 22.0, pal.fg);
                            ui.label(RichText::new(self.tool.title()).font(theme::medium(16.0)).color(pal.fg));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                if small_icon(ui, "collapse-all", "Collapse all sections") {
                                    set_all_groups(ui.ctx(), false);
                                }
                                if small_icon(ui, "expand-all", "Expand all sections") {
                                    set_all_groups(ui.ctx(), true);
                                }
                            });
                        });
                        ui.add_space(6.0);
                        // a new tool starts with an empty search
                        if self.panel_find_tool != Some(self.tool) {
                            // GIFKIT_FIND: a search to start with (for testing)
                            self.panel_find = std::env::var("GIFKIT_FIND").unwrap_or_default();
                            self.panel_find_tool = Some(self.tool);
                        }
                        search_box(ui, &mut self.panel_find, "Search settings", egui::Id::unique("panel-find"));
                        ui.add_space(8.0);
                        let q = self.panel_find.trim().to_lowercase();
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            FIND.with(|f| *f.borrow_mut() = Find { q: q.clone(), ..Default::default() });
                            ui.add_enabled_ui(!self.busy(), |ui| self.tool_panel(ui, &ctx));
                            let shown = FIND.with(|f| std::mem::take(&mut *f.borrow_mut()).shown);
                            if !q.is_empty() && shown == 0 {
                                ui.add_space(8.0);
                                hint(ui, "No settings match the search.");
                            }
                            ui.add_space(8.0);
                        });
                    });
                });
        }

        // the tool rail sits right beside the inspector so the pointer travels less
        egui::Panel::right("rail")
            .resizable(false)
            .exact_size(rail_w)
            .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 4, right: 4, top: 6, bottom: 6 }))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().vline(r.right() + 4.0, r.y_range().expand(6.0), Stroke::new(1.0, pal.border));
                ui.painter().vline(r.left() - 4.0, r.y_range().expand(6.0), Stroke::new(1.0, pal.border));
                self.rail(ui);
            });

        if self.base.is_some() {
            // the timeline at the bottom, the play controls under the view as in Kdenlive
            // it slides up and down from the height it had
            let t = theme::openness(&ctx, egui::Id::unique("timeline-open"), self.show_timeline);
            if t > 0.0 {
                // never more than about half the height, so the view keeps room
                let tmax = ((ui.available_height() - 60.0) * 0.5).clamp(110.0, 640.0);
                let mut panel = egui::Panel::bottom("timeline")
                    .resizable(true)
                    .default_size(270.0f32.min(tmax))
                    .size_range(110.0f32.min(tmax)..=tmax);
                let sliding = t < 1.0;
                let open_h = self.timeline_open_h.clamp(110.0f32.min(tmax), tmax);
                if sliding {
                    let h = open_h * t;
                    panel = panel.resizable(false).size_range(0.0..=tmax).exact_size(h);
                    ctx.request_repaint();
                }
                // the margins shrink with it, so a nearly shut timeline never
                // takes less than no room (that dropped the play bar too low)
                let mg = |v: f32| (v * if sliding { t } else { 1.0 }).round() as i8;
                let shown = panel
                    .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 6, right: 6, top: mg(6.0), bottom: mg(4.0) }))
                    .show(ui, |ui| {
                        if !sliding {
                            self.timeline_dock(ui, &ctx);
                            return;
                        }
                        // while it slides it's drawn at its open height off to
                        // the side and clipped, so it can't push the panel taller
                        let r = ui.max_rect();
                        let full = Rect::from_min_size(r.min, vec2(r.width(), (open_h - 10.0).max(r.height())));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
                        child.set_clip_rect(ui.clip_rect().intersect(r.expand2(vec2(6.0, 0.0))));
                        self.timeline_dock(&mut child, &ctx);
                    });
                if !sliding {
                    self.timeline_open_h = shown.response.rect.height();
                }
            }
            egui::Panel::bottom("transport")
                .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(8, 5)))
                .show(ui, |ui| self.transport(ui));
        }

        mark("panels");
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.canvas)).show(ui, |ui| {
            if self.base.is_none() {
                self.empty_state(ui, &ctx);
            } else if self.tool == Tool::Frames {
                // live preview beside the frame grid, so frame edits can be watched as they play
                egui::Panel::right("frames_preview")
                    .resizable(true)
                    .default_size(380.0)
                    .size_range(220.0..=900.0)
                    .frame(egui::Frame::new().fill(pal.canvas))
                    .show(ui, |ui| {
                        let r = ui.available_rect_before_wrap();
                        ui.allocate_rect(r, Sense::hover());
                        let src = self.view_single;
                        self.source_view(ui, &ctx, r, src, 0, false);
                    });
                egui::Frame::new().inner_margin(10.0).show(ui, |ui| self.frames_grid(ui, &ctx));
            } else {
                self.canvas(ui, &ctx);
            }
        });

        self.export_window(&ctx);
        self.history_window(&ctx);
        self.confirm_window(&ctx);
        self.about_window(&ctx);
        self.restore_window(&ctx);
        self.batch_window(&ctx);
        self.handbook_window(&ctx);
        self.tl_paste_window(&ctx);
        self.ffmpeg_window(&ctx);
        mark("canvas");
        self.emoji_drag_drop(&ctx);
        self.track_live_edits(&ctx, now);
        mark("edits");
        self.shortcuts_window(&ctx);
        self.video_import_window(&ctx);
        eyedropper(&ctx);
        clear_group_command(&ctx);
        if std::env::var_os("GIFKIT_PERF").is_some() {
            let ms = perf_t0.elapsed().as_secs_f64() * 1000.0;
            eprintln!("frame {:.1} ms cur={} prerender={} t={:.3} orig={:?} clock={:.3} playing={}", ms, self.cur, self.prerender.is_some(), ctx.input(|i| i.time), self.original_index(), self.orig_clock, self.playing);
        }

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let r = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("gifkit_drop_overlay")));
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(150));
            painter.rect_stroke(r.shrink(12.0), 6.0, Stroke::new(2.0, pal.highlight), egui::StrokeKind::Inside);
            let msg = match (self.base.is_some(), self.tool) {
                (true, Tool::AddImage) => "Drop to add image layer",
                (true, Tool::Frames) => "Drop to append frames",
                _ => "Drop to open",
            };
            painter.text(r.center(), egui::Align2::CENTER_CENTER, msg, theme::medium(22.0), Color32::WHITE);
        }
    }
}

impl GifKitApp {
    /// KWrite-style menu bar.
    fn menubar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let idle = !self.busy();
        let has = self.base.is_some();
        let theme_before = self.theme;
        let backdrop_before = (self.backdrop, self.backdrop_col);
        egui::MenuBar::new().ui(ui, |ui| {
            top_menu(ui, "&File", |ui| {
                ui.set_min_width(250.0);
                if menu_item(ui, "document-new", "&New Project", &self.keymap.text(crate::keys::Action::New), idle) {
                    self.new_project();
                }
                if menu_item(ui, "document-open", "&Open…", &self.keymap.text(crate::keys::Action::Open), idle) {
                    self.open_dialog(ctx);
                }
                let mut reopen = None;
                let mut clear = false;
                sub_menu(ui, "document-open-recent", "Open &Recent", |ui| {
                    ui.set_min_width(260.0);
                    if self.recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new(RichText::new("No Entries").color(p().fg_muted)).min_size(vec2(0.0, 28.0)));
                    }
                    for (i, path) in self.recent.iter().enumerate() {
                        let name = path.file_name().map(|n| n.to_string_lossy().replace('&', "&&")).unwrap_or_default();
                        // numbered as KDE numbers recent files, the number being the mnemonic
                        let text = if i < 9 { format!("&{} {name}", i + 1) } else { name };
                        if menu_item_tip(ui, "", &text, "", idle, &path.display().to_string()) {
                            reopen = Some(path.clone());
                        }
                    }
                    if !self.recent.is_empty() {
                        ui.separator();
                        if menu_item(ui, "edit-clear-all", "&Clear List", "", true) {
                            clear = true;
                        }
                    }
                });
                if let Some(p) = reopen {
                    self.open_path(p, ctx);
                }
                if clear {
                    self.recent.clear();
                    theme::set_setting("recent", "");
                }
                if menu_item(ui, "document-import", "Open Image Se&quence…", "", idle) {
                    self.open_sequence_dialog(ctx);
                }
                ui.separator();
                if menu_item(ui, "document-save", "&Save", &self.keymap.text(crate::keys::Action::Save), idle && has) {
                    self.save_project(ctx, false);
                }
                if menu_item(ui, "document-save-as", "Save &As…", &self.keymap.text(crate::keys::Action::SaveAs), idle && has) {
                    self.save_project(ctx, true);
                }
                ui.separator();
                if menu_item_tip(ui, "edit-copy", "Copy &Result", "Ctrl+Shift+C", idle && has, "Save the result as Export would and copy the file, to paste into a chat, a post or a folder") {
                    self.copy_result(ctx);
                }
                if menu_item(ui, "document-export", "&Export…", &self.keymap.text(crate::keys::Action::Export), idle && has) {
                    self.export_open = true;
                }
                if menu_item_tip(ui, "archive-insert", "&Batch…", "", idle, "Put the same edits and output settings on many files") {
                    self.batch.open = true;
                    if has {
                        self.batch.edits = batch_ui::BatchEdits::Current;
                    }
                    let (l, pr) = crate::presets::list_with_problems();
                    self.presets = l;
                    self.preset_problems = pr;
                }
                ui.separator();
                if menu_item(ui, "application-exit", "&Quit", &self.keymap.text(crate::keys::Action::Quit), true) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            top_menu(ui, "&Edit", |ui| {
                ui.set_min_width(250.0);
                let pending = self.committed.as_ref().map(|c| c.0 != self.live_sig()).unwrap_or(false);
                let undo = self.undo.last().map(|u| format!("&Undo {}", elide(&u.label, 28).replace('&', "&&"))).unwrap_or_else(|| "&Undo".into());
                if menu_item(ui, "edit-undo", &undo, &self.keymap.text(crate::keys::Action::Undo), idle && (pending || !self.undo.is_empty())) {
                    self.do_undo();
                }
                let redo = self.redo.last().map(|u| format!("Re&do {}", elide(&u.label, 28).replace('&', "&&"))).unwrap_or_else(|| "Re&do".into());
                if menu_item(ui, "edit-redo", &redo, &self.keymap.text(crate::keys::Action::Redo), idle && !self.redo.is_empty()) {
                    self.do_redo();
                }
                ui.separator();
                {
                    // the timeline's clips when it was clicked last, else the selected layer
                    let now = ctx.input(|i| i.time);
                    let what = if self.tl_focus { "Clips" } else { "Layer" };
                    let can_copy = if self.tl_focus { self.tl_clip.is_some() || self.tl_key.is_some() } else { self.sel_layer_index().is_some() };
                    let can_paste = if self.tl_focus { self.tl_board.is_some() } else { !self.layer_board.is_empty() };
                    if menu_item(ui, "edit-cut", &format!("Cu&t {what}"), "Ctrl+X", has && idle && can_copy) {
                        self.edit_cut(ctx, now);
                    }
                    if menu_item(ui, "edit-copy", &format!("&Copy {what}"), "Ctrl+C", has && can_copy) {
                        self.edit_copy(ctx);
                    }
                    if menu_item(ui, "edit-paste", &format!("&Paste {what}"), "Ctrl+V", has && idle && can_paste) {
                        self.edit_paste(now);
                    }
                    if menu_item(ui, "edit-duplicate", &format!("Dupli&cate {what}"), &self.keymap.text(crate::keys::Action::Duplicate), has && idle && can_copy) {
                        self.edit_duplicate(ctx, now);
                    }
                    if menu_item(ui, "edit-delete", &format!("&Delete {what}"), "Del", has && idle && can_copy) {
                        self.edit_delete(ctx, now);
                    }
                }
                ui.separator();
                if menu_item(ui, "view-history", "&History", &self.keymap.text(crate::keys::Action::History), true) {
                    self.history_open = !self.history_open;
                }
                if menu_item(ui, "bookmarks", "&Presets", "", has) {
                    self.open_tool(Tool::Presets);
                }
                ui.separator();
                if menu_item(ui, "merge", "&Collapse All", "", idle && has && !self.stack.is_empty()) {
                    self.bake(ctx, false);
                }
                if menu_item(ui, "dialog-ok-apply", "Collapse &Frame Edits", "", idle && has && !self.stack.transform.is_identity()) {
                    self.bake(ctx, true);
                }
            });
            top_menu(ui, "&View", |ui| {
                ui.set_min_width(250.0);
                if menu_check(ui, !self.compare, "&Single View", &self.keymap.text(crate::keys::Action::SplitView)) {
                    self.compare = false;
                }
                if menu_check(ui, self.compare, "S&plit View", &self.keymap.text(crate::keys::Action::SplitView)) {
                    self.compare = true;
                }
                if menu_check(ui, self.link_views, "&Link Both Views", "") {
                    self.link_views = !self.link_views;
                }
                ui.separator();
                if menu_item(ui, "zoom-in", "Zoom &In", &self.keymap.text(crate::keys::Action::ZoomIn), has) {
                    self.zoom_to(self.cur_scale() * 1.25);
                }
                if menu_item(ui, "zoom-out", "Zoom &Out", &self.keymap.text(crate::keys::Action::ZoomOut), has) {
                    self.zoom_to(self.cur_scale() / 1.25);
                }
                if menu_item(ui, "zoom-fit-best", "&Fit to View", &self.keymap.text(crate::keys::Action::Fit), has) {
                    self.zoom_fit();
                }
                if menu_item(ui, "zoom-original", "&Actual Size", &self.keymap.text(crate::keys::Action::ActualSize), has) {
                    self.zoom_to(1.0);
                }
                if menu_check(ui, self.onion, "&Onion Skin", &self.keymap.text(crate::keys::Action::OnionSkin)) {
                    self.toggle_onion();
                }
                ui.separator();
                if menu_check(ui, self.show_timeline, "Show &Timeline", "") {
                    self.show_timeline = !self.show_timeline;
                    theme::set_setting("show_timeline", if self.show_timeline { "1" } else { "0" });
                }
                let full = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
                if menu_check(ui, full, "F&ull Screen Mode", &self.keymap.text(crate::keys::Action::FullScreen)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
                }
            });
            top_menu(ui, "&Settings", |ui| {
                ui.set_min_width(290.0);
                if menu_item_tip(ui, "", "Configure &ffmpeg…", "", true, "Where GIFkit finds ffmpeg, for video and AVIF") {
                    self.ff_msg = None;
                    self.ff_open = true;
                }
                ui.separator();
                if menu_check(ui, self.show_toolbar, "Show &Toolbar", "") {
                    self.show_toolbar = !self.show_toolbar;
                    theme::set_setting("show_toolbar", if self.show_toolbar { "1" } else { "0" });
                }
                if menu_check(ui, self.show_statusbar, "Show St&atusbar", "") {
                    self.show_statusbar = !self.show_statusbar;
                    theme::set_setting("show_statusbar", if self.show_statusbar { "1" } else { "0" });
                }
                if menu_check(ui, !self.sidebar_compact, "Show Tool &Names", "") {
                    self.set_rail_compact(!self.sidebar_compact);
                }
                ui.separator();
                sub_menu(ui, "", "Show T&ransparency As", |ui| {
                    ui.set_min_width(220.0);
                    if menu_check(ui, self.backdrop == Backdrop::Checker, "C&heckerboard", "") {
                        self.backdrop = Backdrop::Checker;
                    }
                    if menu_check(ui, self.backdrop == Backdrop::Canvas, "&Viewport Background", "") {
                        self.backdrop = Backdrop::Canvas;
                    }
                    if menu_check(ui, self.backdrop == Backdrop::Solid, "&Solid Colour", "") {
                        self.backdrop = Backdrop::Solid;
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(34.0);
                        ui.label("Colour:");
                        colour_btn(ui, &mut self.backdrop_col);
                    });
                });
                if menu_check(ui, theme::animate(), "&Animate Panels", "") {
                    theme::set_animate(ui.ctx(), !theme::animate());
                }
                if menu_check(ui, self.pixelated, "&Pixelated Preview", "") {
                    self.pixelated = !self.pixelated;
                    theme::set_setting("pixelated", if self.pixelated { "1" } else { "0" });
                }
                sub_menu(ui, "", "Pre&view Resolution", |ui| {
                    ui.set_min_width(240.0);
                    for (v, l) in [(0u32, "&Full Size"), (1920, "Up to &1920 px"), (1280, "Up to 1&280 px (Faster)"), (960, "Up to &960 px (Fastest)")] {
                        if menu_check(ui, self.preview_max == v, l, "") {
                            self.preview_max = v;
                            theme::set_setting("preview_max", &v.to_string());
                        }
                    }
                });
                sub_menu(ui, "", "&Interface Scale", |ui| {
                    ui.set_min_width(160.0);
                    let cur = ctx.zoom_factor();
                    for (v, l) in [(0.8f32, "&80%"), (0.9, "&90%"), (1.0, "&100%"), (1.1, "11&0%"), (1.25, "12&5%"), (1.5, "15&0%"), (1.75, "17&5%"), (2.0, "&200%")] {
                        if menu_check(ui, (cur - v).abs() < 0.01, l, "") {
                            ctx.set_zoom_factor(v);
                            theme::set_setting("ui_scale", &format!("{v}"));
                        }
                    }
                });
                ui.separator();
                let hw = crate::gpu::enabled();
                if menu_check(ui, hw, "Use &Hardware Acceleration", "") {
                    crate::gpu::set_enabled(!hw);
                    self.free_gpu_frames();
                    self.comp_key = 0;
                }
                sub_menu(ui, "", "&Memory Use", |ui| {
                    ui.set_min_width(250.0);
                    for (v, l) in [(50usize, "&Low (Saves Memory)"), (100, "&Normal"), (250, "&High (Smoother)")] {
                        if menu_check(ui, self.mem_pct == v, l, "") {
                            self.mem_pct = v;
                            crate::pixels::set_mem_pct(v);
                            theme::set_setting("mem_pct", &v.to_string());
                        }
                    }
                });
                if menu_check(ui, self.sample_preview, "&Estimate Size of Long Animations", "") {
                    self.sample_preview = !self.sample_preview;
                    theme::set_setting("sample_preview", if self.sample_preview { "1" } else { "0" });
                }
                if menu_item(ui, "document-open", "Open Presets F&older", "", true) {
                    if let Some(dir) = crate::presets::folder() {
                        let _ = std::fs::create_dir_all(&dir);
                        open_in_file_manager(&dir);
                    }
                }
                ui.separator();
                sub_menu(ui, "", "&Colour Scheme", |ui| {
                    ui.set_min_width(160.0);
                    for (t, l) in [(ThemeChoice::System, "&Follow System"), (ThemeChoice::Dark, "&Dark"), (ThemeChoice::Light, "&Light")] {
                        if menu_check(ui, self.theme == t, l, "") {
                            self.theme = t;
                        }
                    }
                });
                if cfg!(all(unix, not(target_os = "macos"))) {
                    let native = theme::get_setting("wayland").as_deref() == Some("native");
                    if menu_check(ui, native, "Native &Wayland", "") {
                        theme::set_setting("wayland", if native { "xwayland" } else { "native" });
                        self.info("Applies after a restart");
                    }
                }
                ui.separator();
                if menu_item(ui, "configure-shortcuts", "&Keyboard Shortcuts", "", true) {
                    self.shortcuts_open = true;
                }
            });
            top_menu(ui, "&Help", |ui| {
                ui.set_min_width(250.0);
                if menu_item(ui, "help-contents", "GIFkit &Handbook", &self.keymap.text(crate::keys::Action::Handbook), true) {
                    self.handbook_open = true;
                }
                if menu_item(ui, "", "GIFkit &Readme", "", true) {
                    open_external(&format!("{REPO_URL}#readme"));
                }
                ui.separator();
                if menu_item(ui, "tools-report-bug", "&Report Bug…", "", true) {
                    open_external(&format!("{REPO_URL}/issues"));
                }
                if menu_item(ui, "view-refresh", "Check for &Updates", "", true) {
                    open_external(&format!("{REPO_URL}/releases"));
                }
                if menu_item(ui, "", "GIFkit &Website", "", true) {
                    open_external(REPO_URL);
                }
                ui.separator();
                if menu_item(ui, "help-about", "&About GIFkit", "", true) {
                    self.about_open = true;
                }
            });
        });
        if theme_before != self.theme {
            self.theme.apply(ctx);
            theme::save_theme_choice(self.theme);
        }
        if backdrop_before != (self.backdrop, self.backdrop_col) {
            self.save_backdrop();
        }
    }

    /// Remember a file for File ▸ Open Recent (ten at most, newest first).
    fn add_recent(&mut self, p: &std::path::Path) {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        self.recent.retain(|r| *r != p);
        self.recent.insert(0, p);
        self.recent.truncate(10);
        let list: Vec<String> = self.recent.iter().map(|r| r.display().to_string()).collect();
        // one line: the settings file has a setting a line
        theme::set_setting("recent", &serde_json::to_string(&list).unwrap_or_default());
    }

    fn save_backdrop(&self) {
        theme::set_setting("backdrop", match self.backdrop {
            Backdrop::Checker => "checker",
            Backdrop::Solid => "solid",
            Backdrop::Canvas => "canvas",
        });
        let c = self.backdrop_col;
        theme::set_setting("backdrop_col", &format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
    }

    fn set_rail_compact(&mut self, compact: bool) {
        self.sidebar_compact = compact;
        theme::set_setting("sidebar_compact", if compact { "1" } else { "0" });
    }

    /// Switch tools (the rail, the menus and the layer list all use this).
    fn open_tool(&mut self, t: Tool) {
        if t == Tool::Presets && self.tool != Tool::Presets {
            let (l, pr) = crate::presets::list_with_problems();
            self.presets = l;
            self.preset_problems = pr;
        }
        self.tool = t;
        self.drag = None;
    }

    /// Do what was waiting on the unsaved-changes question.
    fn run_pending(&mut self, ctx: &egui::Context, what: Pending) {
        self.modified = false;
        match what {
            Pending::New => self.close_document(),
            Pending::Quit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::Open(p) => self.open_path_now(p, ctx),
            Pending::Sequence(ps) => self.open_sequence(ps, ctx),
        }
    }

    /// The window closing, and the save-first question KDE apps ask.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.modified && self.base.is_some() && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Pending::Quit);
        }
        // a save started from the question: carry on once it's done, or
        // drop it if the save was cancelled or failed
        if let Some(what) = self.after_save.clone() {
            if self.job.is_none() {
                self.after_save = None;
                if !self.modified {
                    self.run_pending(ctx, what);
                }
            }
        }
    }

    /// "The document has been modified": Save, Discard or Cancel, laid out
    /// as KDE's message boxes are.
    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(what) = self.confirm.clone() else { return };
        let pal = *p();
        let mut open = true;
        let mut answer: Option<u8> = None;
        let name = self.project_path.as_ref().or(self.path.as_ref()).and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        let title = match what {
            Pending::Quit => "Close Document",
            Pending::New => "New Project",
            _ => "Open",
        };
        dialog(ctx, "confirm", title, vec2(460.0, 160.0), false, &mut open, |ui| {
            ui.horizontal_top(|ui| {
                ui.add(crate::icons::image(ui.ctx(), "dialog-warning", 48.0, pal.fg));
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("The document \"{}\" has been modified.", elide(&name, 40))).color(pal.fg));
                    ui.label(RichText::new("Do you want to save your changes or discard them?").color(pal.fg));
                });
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = |ui: &mut egui::Ui, icon: &str, text: &str, default: bool| {
                        let b = egui::Button::image_and_text(crate::icons::image(ui.ctx(), icon, 16.0, pal.fg), text).min_size(vec2(96.0, 30.0));
                        ui.add(if default { b.stroke(Stroke::new(1.0, pal.highlight)) } else { b }).clicked()
                    };
                    if btn(ui, "dialog-cancel", "Cancel", false) {
                        answer = Some(0);
                    }
                    if btn(ui, "edit-delete", "Discard", false) {
                        answer = Some(2);
                    }
                    if btn(ui, "document-save", "Save", true) {
                        answer = Some(1);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                answer = Some(1);
            }
        });
        if !open {
            answer = Some(0);
        }
        match answer {
            Some(0) => self.confirm = None,
            Some(1) => {
                self.confirm = None;
                self.save_project(ctx, false);
                // the save runs in the background; what was asked for follows it
                self.after_save = Some(what);
            }
            Some(_) => {
                self.confirm = None;
                self.run_pending(ctx, what);
            }
            None => {}
        }
    }

    /// Close the current file and start over (asks first when there are unsaved changes).
    fn new_project(&mut self) {
        if self.base.is_none() {
            return;
        }
        if self.modified {
            self.confirm = Some(Pending::New);
            return;
        }
        self.close_document();
    }

    /// Forget the open animation and everything done to it.
    fn close_document(&mut self) {
        self.clear_autosave();
        self.base = None;
        self.original = None;
        self.orig_gen += 1;
        self.path = None;
        self.project_path = None;
        self.file_size = None;
        self.stack = Stack::default();
        self.undo.clear();
        self.redo.clear();
        self.committed = None;
        self.cmp = None;
        self.plan.clear();
        self.plan_times.clear();
        self.sel.clear();
        self.edit_labels.clear();
        self.edit_rows_shown.clear();
        self.modified = false;
        self.export_open = false;
        self.cur = 0;
        self.bump();
        self.info("New project");
    }

    /// Main toolbar: file actions, undo, and the view controls, each as its own button.
    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (_, tb_icon) = theme::toolbar_style();
        let tb_h = tb_icon + 10.0;
        // right-clicking the bar gives KDE's toolbar menu
        let bg = ui.interact(ui.max_rect(), ui.scope_id().with("toolbar-bg"), Sense::click());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let idle = !self.busy();
            let has = self.base.is_some();
            if tb_tool(ui, "document-new", "New", "New project (Ctrl+N)", idle && has).clicked() {
                self.new_project();
            }
            if tb_tool(ui, "document-open", "Open…", "Open a GIF, image, video or project (Ctrl+O)", idle).clicked() {
                self.open_dialog(ctx);
            }
            if tb_tool(ui, "document-save", "Save", "Save the project (Ctrl+S)", idle && has).clicked() {
                self.save_project(ctx, false);
            }
            if tb_tool(ui, "document-save-as", "", "Save the project as (Ctrl+Shift+S)", idle && has).clicked() {
                self.save_project(ctx, true);
            }
            toolbar_sep(ui);
            let pending = self.committed.as_ref().map(|c| c.0 != self.live_sig()).unwrap_or(false);
            let undo_tip = self.undo.last().map(|u| format!("Undo {} (Ctrl+Z)", u.label)).unwrap_or_else(|| "Undo (Ctrl+Z)".into());
            if tb_tool(ui, "edit-undo", "", &undo_tip, idle && (pending || !self.undo.is_empty())).clicked() {
                self.do_undo();
            }
            let redo_tip = self.redo.last().map(|u| format!("Redo {} (Ctrl+Y)", u.label)).unwrap_or_else(|| "Redo (Ctrl+Y)".into());
            if tb_tool(ui, "edit-redo", "", &redo_tip, idle && !self.redo.is_empty()).clicked() {
                self.do_redo();
            }
            if has {
                toolbar_sep(ui);
                self.view_controls(ui);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // exporting lives in File ▸ Export… (Ctrl+E)
                if has {
                    ui.add_enabled_ui(idle, |ui| self.format_menu(ui, tb_h));
                    if ui.available_width() > 80.0 {
                        ui.label(RichText::new("Export as").color(p().fg_muted)).on_hover_text("The format the compressed preview shows and File ▸ Export saves");
                    }
                }
            });
        });
        bg.context_menu(|ui| {
            ui.set_min_width(220.0);
            ui.add_space(2.0);
            ui.label(RichText::new("Main Toolbar").font(theme::medium(13.0)).color(p().fg_muted));
            ui.separator();
            let (text, size) = theme::toolbar_choice();
            sub_menu(ui, "", "&Text Position", |ui| {
                ui.set_min_width(220.0);
                for (v, l) in [(0u8, "&Follow System"), (1, "&Icons Only"), (2, "Text &Alongside Icons")] {
                    if menu_check(ui, text == v, l, "") {
                        theme::set_toolbar_choice(v, size);
                    }
                }
            });
            sub_menu(ui, "", "Icon &Size", |ui| {
                ui.set_min_width(200.0);
                for (v, l) in [(0u32, "&Follow System"), (16, "&Small (16×16)"), (22, "&Medium (22×22)"), (32, "&Large (32×32)")] {
                    if menu_check(ui, size == v, l, "") {
                        theme::set_toolbar_choice(text, v);
                    }
                }
            });
            ui.separator();
            if menu_item(ui, "", "&Hide Toolbar", "", true) {
                self.show_toolbar = false;
                theme::set_setting("show_toolbar", "0");
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 3.0, Stroke::new(1.0, pal.border));
        let mut stop = false;
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                ui.spinner();
                ui.label(RichText::new(format!("{}…", job.label)).size(12.5));
            } else if let Some(s) = &self.status {
                ui.label(RichText::new(&s.text).size(12.5).color(if s.error { pal.negative } else { pal.fg_muted }));
            } else {
                ui.label(RichText::new("Ready").size(12.5).color(pal.fg_muted));
            }
            // the right end: Stop, always there and only active while work runs,
            // with the progress bar beside it as in KDE's job tracker
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let running = self.job.is_some();
                let tip = match &self.job {
                    Some(j) => format!("Stop {}", j.label.to_lowercase()),
                    None => "Stop the running operation".to_string(),
                };
                let b = ui.add_enabled(running, egui::Button::image(crate::icons::image(ui.ctx(), "process-stop", 16.0, if running { pal.fg } else { pal.fg_muted })).frame_when_inactive(false).min_size(vec2(22.0, 20.0)));
                if b.on_hover_text(tip).on_disabled_hover_text("Nothing to stop").clicked() {
                    stop = true;
                }
                if let Some(job) = &self.job {
                    let done = (job.progress.load(Ordering::Relaxed) & !stack::STOP_BIT).min(job.total);
                    if job.total > 1 {
                        ui.add(egui::ProgressBar::new(done as f32 / job.total as f32).desired_width(200.0).desired_height(12.0).fill(pal.highlight));
                    }
                }
                // memory in use, read every couple of seconds
                let now = ui.input(|i| i.time);
                if now - self.mem_seen.1 > 2.0 {
                    self.mem_seen = (crate::pixels::process_bytes().unwrap_or(0), now);
                }
                if self.mem_seen.0 > 0 {
                    ui.add_space(4.0);
                    let lvl = match self.mem_pct {
                        v if v < 100 => "Low",
                        v if v > 100 => "High",
                        _ => "Normal",
                    };
                    ui.label(RichText::new(format!("{:.0} MiB", self.mem_seen.0 as f64 / 1_048_576.0)).size(12.5).color(pal.fg_muted)).on_hover_text(format!("Memory in use. Memory Use is {lvl} (Settings)."));
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
                }
                if self.base.is_some() {
                    let name = self.project_path.as_ref().or(self.path.as_ref()).and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
                    let room = ((ui.available_width() - 200.0) / 7.0).max(6.0) as usize;
                    ui.add_space(8.0);
                    ui.label(RichText::new(if self.modified { format!("{} •", elide(&name, room.min(40))) } else { elide(&name, room.min(40)) }).size(12.5).color(pal.fg_muted)).on_hover_text(if self.modified { "Unsaved changes" } else { "" });
                }
            });
        });
        if stop {
            self.stop_job();
        }
    }

    /// Give up on the running operation: its work notices and ends, and
    /// whatever it was making is dropped.
    fn stop_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.progress.fetch_or(stack::STOP_BIT, Ordering::Relaxed);
            self.info(format!("Stopped {}", job.label.to_lowercase()));
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        use crate::keys::{Action, Binding};
        if !self.shortcuts_open {
            self.key_capture = None;
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut set: Option<(Action, Option<Binding>)> = None;
        let mut reset: Option<Action> = None;
        let mut reset_all = false;
        let mut capture = self.key_capture;
        let km = self.keymap.clone();
        let mut filter = std::mem::take(&mut self.keys_filter);
        dialog(ctx, "shortcuts", "Keyboard Shortcuts", vec2(900.0, 680.0), true, &mut open, |ui| {
            // the next key press goes to the action waiting for one
            if let Some(a) = capture {
                let press = ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        // a modifier on its own waits for the key that goes with it
                        egui::Event::Key { key, pressed: true, modifiers, .. } if !["Control", "Shift", "Alt", "Meta", "Super", "Command"].iter().any(|m| key.name().contains(m)) => Some((*key, *modifiers)),
                        _ => None,
                    })
                });
                if let Some((key, m)) = press {
                    match key {
                        egui::Key::Escape => {}
                        egui::Key::Backspace if m.is_none() => set = Some((a, None)),
                        _ => set = Some((a, Some(Binding::from_press(key, m)))),
                    }
                    capture = None;
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search shortcuts").desired_width(220.0));
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset All").clicked() {
                        reset_all = true;
                    }
                });
            });
            ui.add_space(8.0);
            let q = filter.trim().to_lowercase();
            let hit = |a: Action| q.is_empty() || a.label().to_lowercase().contains(&q) || a.group().to_lowercase().contains(&q) || km.get(a).map(|b| b.text().to_lowercase().contains(&q)).unwrap_or(false);
            const FIXED: [(&str, &str); 10] = [
                ("Ctrl+V", "Paste files or pictures"),
                ("Alt+letter", "Open a menu"),
                ("Mouse wheel", "Zoom"),
                ("Middle or right drag", "Pan"),
                ("Alt+drag", "Pan in drawing tools"),
                ("Shift+drag", "Move in straight lines"),
                ("Shift while resizing", "Keep or free proportions"),
                ("Ctrl while resizing", "Scale in one direction"),
                ("Ctrl+wheel", "Zoom the timeline"),
                ("Shift+wheel", "Scroll the timeline sideways"),
            ];
            let fixed: Vec<(&str, &str)> = FIXED.iter().copied().filter(|(k, w)| q.is_empty() || k.to_lowercase().contains(&q) || w.to_lowercase().contains(&q)).collect();
            // each group a card; cards go into columns, each to the shortest
            let groups: Vec<(&str, Vec<Action>)> = Action::GROUPS.iter().map(|g| (*g, Action::ALL.iter().copied().filter(|a| a.group() == *g && hit(*a)).collect::<Vec<_>>())).filter(|(_, v)| !v.is_empty()).collect();
            let cols = ((ui.available_width() / 380.0).floor() as usize).clamp(1, 3);
            let mut col_of = vec![Vec::new(); cols];
            let mut heights = vec![0usize; cols];
            for (k, (g, v)) in groups.iter().enumerate() {
                // two columns: the short groups on the left, Edit and the
                // long Timeline group on the right; otherwise the shortest
                let c = if cols == 2 { usize::from(matches!(*g, "Edit" | "Timeline")) } else { (0..cols).min_by_key(|c| heights[*c]).unwrap_or(0) };
                col_of[c].push(k);
                heights[c] += v.len() + 2;
            }
            let card = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
                egui::Frame::new()
                    .fill(mix(pal.window, pal.view, 0.55))
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(title).font(theme::medium(13.5)).color(pal.fg));
                        ui.add_space(4.0);
                        body(ui);
                    });
                ui.add_space(10.0);
            };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.columns(cols, |cs| {
                    for (c, ui) in cs.iter_mut().enumerate() {
                        for &k in &col_of[c] {
                            let (title, acts) = &groups[k];
                            card(ui, title, &mut |ui: &mut egui::Ui| {
                                for &a in acts {
                                    ui.horizontal(|ui| {
                                        ui.set_min_height(CHIP_H);
                                        // the card already says Timeline
                                        let full = a.label();
                                        let name = match full.strip_prefix("Timeline: ") {
                                            Some(r) => r[..1].to_uppercase() + &r[1..],
                                            None => full.to_string(),
                                        };
                                        let room = (ui.available_width() - 166.0).max(40.0);
                                        ui.allocate_ui_with_layout(vec2(room, CHIP_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                            ui.add(egui::Label::new(RichText::new(name).color(pal.fg)).truncate()).on_hover_text(full);
                                        });
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let changed = km.get(a) != a.default_binding();
                                            let back = format!("Back to {}", a.default_binding().map(|b| b.text()).unwrap_or_else(|| "none".into()));
                                            if ui.add_enabled(changed, egui::Button::image(crate::icons::image(ui.ctx(), "edit-undo", 16.0, pal.fg)).frame_when_inactive(false)).on_hover_text(back).clicked() {
                                                reset = Some(a);
                                            }
                                            let waiting = capture == Some(a);
                                            let text = if waiting { "Press keys…".to_string() } else { km.get(a).map(|b| b.text()).unwrap_or_else(|| "None".into()) };
                                            let col = if km.get(a).is_some() || waiting { pal.fg } else { pal.fg_muted };
                                            let b = ui.add(sel_btn(egui::Button::new(RichText::new(text).font(theme::medium(13.0)).color(col)).min_size(vec2(124.0, 26.0)), waiting));
                                            if b.clicked() {
                                                capture = if waiting { None } else { Some(a) };
                                            }
                                        });
                                    });
                                }
                            });
                        }
                    }
                });
                if !fixed.is_empty() {
                    card(ui, "Mouse and Fixed Keys", &mut |ui: &mut egui::Ui| {
                        let fcols = if ui.available_width() > 700.0 { 2 } else { 1 };
                        let per = fixed.len().div_ceil(fcols);
                        ui.columns(fcols, |cs| {
                            for (c, ui) in cs.iter_mut().enumerate() {
                                egui::Grid::new(("fixed", c)).num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                                    for (k, what) in fixed.iter().skip(c * per).take(per) {
                                        ui.label(RichText::new(*k).font(theme::medium(13.0)).color(pal.fg));
                                        ui.add(egui::Label::new(RichText::new(*what).color(pal.fg_muted)).wrap());
                                        ui.end_row();
                                    }
                                });
                            }
                        });
                    });
                }
                if groups.is_empty() && fixed.is_empty() {
                    hint(ui, "No shortcuts match.");
                }
            });
            if capture.is_some() {
                ui.ctx().request_repaint();
            }
        });
        self.keys_filter = filter;
        self.key_capture = capture;
        if let Some((a, b)) = set {
            if let Some(lost) = self.keymap.set(a, b) {
                self.info(format!("{} is now {}; {} has no shortcut", b.map(|b| b.text()).unwrap_or_default(), a.label(), lost.label()));
            }
        }
        if let Some(a) = reset {
            self.keymap.reset(a);
        }
        if reset_all {
            self.keymap.reset_all();
        }
        if !open {
            self.shortcuts_open = false;
            self.key_capture = None;
        }
    }

    /// The tool rail: layer tools, frame edits, output, then presets and history.
    fn rail(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let compact = self.sidebar_compact;
        // how far the names are shown (0 icons only .. 1 names), while it slides
        let names = theme::openness(ui.ctx(), egui::Id::unique("rail-names"), !compact);
        egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let divider = |ui: &mut egui::Ui| {
                ui.add_space(4.0);
                let r = ui.available_rect_before_wrap();
                ui.painter().hline(r.x_range().shrink(4.0), r.top() + 1.0, Stroke::new(1.0, pal.border));
                ui.add_space(6.0);
            };
            for t in Tool::ALL {
                if matches!(t, Tool::Resize | Tool::Frames | Tool::Presets) {
                    divider(ui);
                }
                self.rail_row(ui, Some(t), names);
            }
            self.rail_row(ui, None, names);
            divider(ui);
            let (icon, tip) = if compact { ("sidebar-collapse-right", "Show tool names") } else { ("sidebar-collapse-left", "Hide tool names") };
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, mix(pal.window, pal.highlight, 0.12));
            }
            let x = r.center().x + (r.left() + 16.0 - r.center().x) * names;
            crate::icons::paint(ui.painter(), icon, pos2(x, r.center().y), 16.0, pal.fg_muted);
            if resp.on_hover_text(tip).clicked() {
                self.set_rail_compact(!compact);
            }
        });
    }

    /// One rail entry. `None` is the History window toggle.
    fn rail_row(&mut self, ui: &mut egui::Ui, t: Option<Tool>, names: f32) {
        // tool tips name the tool once the names are mostly hidden
        let compact = names < 0.5;
        let pal = *p();
        let has = self.base.is_some();
        let sel = match t {
            Some(t) => self.tool == t && has,
            None => self.history_open,
        };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let painter = ui.painter();
        // the colour scheme's selection colours, like a Qt item view
        if sel {
            painter.rect_filled(r, 4.0, pal.highlight);
        } else if resp.hovered() {
            painter.rect_filled(r, 4.0, mix(pal.window, pal.highlight, 0.18));
            painter.rect_stroke(r, 4.0, Stroke::new(1.0, mix(pal.window, pal.highlight, 0.6)), egui::StrokeKind::Inside);
        }
        let (icon, label, tip) = match t {
            Some(t) => (t.breeze(), t.label(), t.tip()),
            None => ("view-history", "History", "Every change, with jumps back and forth (Ctrl+H)"),
        };
        let icon_col = if sel { Color32::WHITE } else if has || t.is_none() { pal.fg } else { pal.fg_muted };
        // icons go from big and centred to small at the left as the names come in
        let icon_size = 22.0 - 6.0 * names;
        let icon_x = r.center().x + (r.left() + 15.0 - r.center().x) * names;
        crate::icons::paint(painter, icon, pos2(icon_x, r.center().y), icon_size, icon_col);
        if names > 0.0 {
            // names fade in, cut off at the rail's edge while it's narrow
            let tp = painter.with_clip_rect(r.intersect(painter.clip_rect()));
            // just right of the icon, wherever it is on its way
            tp.text(pos2(icon_x + icon_size / 2.0 + 7.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), icon_col.gamma_multiply(names));
            // how many layers of this kind there are, in muted text
            if let Some(k) = t.and_then(|t| t.layer_kind()) {
                let n = self.stack.count(k);
                if n > 0 && names > 0.9 {
                    let c = if sel { Color32::WHITE } else { pal.fg_muted };
                    tp.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, n.to_string(), egui::FontId::proportional(12.0), c.gamma_multiply((names - 0.9) * 10.0));
                }
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let resp = if compact { resp.on_hover_text(format!("{label}\n{tip}")) } else { resp.on_hover_text(tip) };
        if resp.clicked() {
            match t {
                Some(t) => self.open_tool(t),
                None => self.history_open = !self.history_open,
            }
        }
    }

    /// Photoshop-style layer list: top of the stack first, drag to reorder.
    fn layer_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        // the panel takes its height from what it holds: claim all of it up
        // front so it never creeps shorter frame by frame
        let full = ui.available_height();
        ui.set_min_height(full);
        let now = ctx.input(|i| i.time);
        // header
        ui.horizontal(|ui| {
            let arrow = crate::icons::image(ui.ctx(), if self.layers_collapsed { "go-next" } else { "go-down" }, 16.0, pal.fg_muted);
            let h = ui.add(egui::Button::image_and_text(arrow, RichText::new("Layers").font(theme::medium(14.0)).color(pal.fg)).frame(false));
            if h.on_hover_text("Right-click a layer for more").clicked() {
                self.layers_collapsed = !self.layers_collapsed;
            }
            if !self.layers_collapsed {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let on = self.layer_find_open;
                    let b = ui.add(sel_btn(egui::Button::image(crate::icons::image(ui.ctx(), "edit-find", 16.0, pal.fg)).frame_when_inactive(on), on)).on_hover_text(if on { "Hide the search" } else { "Search layers" });
                    if b.clicked() {
                        self.layer_find_open = !on;
                        self.layer_find_fresh = !on;
                        if on {
                            self.layer_find.clear();
                        }
                    }
                });
            }
        });
        if self.layers_collapsed {
            return;
        }
        ui.add_space(4.0);
        if self.layer_find_open {
            let r = search_box(ui, &mut self.layer_find, "Search layers", egui::Id::unique("layer-find"));
            if std::mem::take(&mut self.layer_find_fresh) {
                r.request_focus();
            }
            ui.add_space(4.0);
        }
        // the selected layer's lock and opacity, as in Photoshop's layer panel
        let sel_li = self.sel_layer_index();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(sel_li.is_some(), |ui| {
                let (locked, mut op) = sel_li.map(|i| (self.stack.layers[i].locked, self.stack.layers[i].opacity)).unwrap_or((false, 100.0));
                let icon = crate::icons::image(ui.ctx(), if locked { "object-locked" } else { "object-unlocked" }, 16.0, pal.fg);
                let b = ui.add(sel_btn(egui::Button::image(icon).min_size(vec2(28.0, 26.0)), locked)).on_hover_text(if locked { "Unlock the layer" } else { "Lock the layer so it can't be changed" });
                if b.clicked() {
                    if let Some(i) = sel_li {
                        self.stack.layers[i].locked = !locked;
                        self.modified = true;
                    }
                }
                let clipped = sel_li.is_some_and(|i| self.stack.layers[i].clip);
                let can_clip = sel_li.is_some_and(|i| i > 0) && !locked;
                let icon = crate::icons::image(ui.ctx(), "mask-alpha", 16.0, pal.fg);
                let c = ui.add_enabled(can_clip, sel_btn(egui::Button::image(icon).min_size(vec2(28.0, 26.0)), clipped)).on_hover_text(if clipped { "Stop clipping to the layer below" } else { "Clip to the layer below: show only where it has drawn" });
                if c.clicked() {
                    if let Some(i) = sel_li {
                        self.stack.layers[i].clip = !clipped;
                        self.modified = true;
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Opacity:").color(if sel_li.is_some() { pal.fg } else { pal.fg_muted }));
                ui.add_enabled_ui(!locked, |ui| {
                    if fill_slider(ui, egui::Slider::new(&mut op, 0.0..=100.0).suffix("%").max_decimals(0)).on_hover_text("The whole layer's opacity").changed() {
                        if let Some(i) = sel_li {
                            self.stack.layers[i].opacity = op;
                            self.modified = true;
                        }
                    }
                });
            });
        });
        ui.add_space(4.0);
        let find = self.layer_find.trim().to_lowercase();
        // the list, with a row of buttons under it
        let mut rect = ui.available_rect_before_wrap();
        // room for the buttons: gap, a toolbar row and item spacing, so the panel never creeps taller
        let list_h = (rect.height() - 4.0 - TOOLBAR_H - 2.0 * ui.spacing().item_spacing.y).max(40.0);
        rect.set_height(list_h);
        ui.painter().rect_filled(rect, 3.0, pal.view);
        ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        let n = self.stack.layers.len();
        let row_h = 30.0;
        let mut select: Option<(Kind, u64)> = None;
        let mut toggle: Option<usize> = None;
        let mut lock: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut drop_to: Option<(usize, usize)> = None;
        let mut to_frames = false;
        let mut rename_done: Option<Option<(usize, String)>> = None;
        let mut edit_rename_done: Option<Option<(Tool, String)>> = None;
        let mut edit_toggle: Option<Tool> = None;
        let mut edit_remove: Option<Tool> = None;
        let mut menu_act: Option<LayerAct> = None;
        let idle = !self.busy();
        let frame_edits = !self.stack.transform.is_identity();
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut rows: Vec<(usize, Rect)> = Vec::new();
            for li in (0..n).rev() {
                let (id, name, kind, visible, locked, clipped) = {
                    let l = &self.stack.layers[li];
                    (l.id, l.name(), l.kind(), l.visible, l.locked, l.clip && li > 0)
                };
                if !find.is_empty() && !name.to_lowercase().contains(&find) && !kind_tool(kind).label().to_lowercase().contains(&find) {
                    continue;
                }
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
                rows.push((li, r));
                let selected = self.sel.get(kind_key(kind)) == Some(&id) && self.tool == kind_tool(kind);
                let dragging_me = self.layer_drag == Some(id);
                let painter = ui.painter().clone();
                // selection in the scheme's selection colours, as Qt item views draw it
                if selected {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, pal.highlight);
                } else if resp.hovered() {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, mix(pal.view, pal.highlight, 0.18));
                }
                let pal = if selected { theme::Palette { fg: Color32::WHITE, fg_muted: mix(Color32::WHITE, pal.highlight, 0.35), ..pal } } else { pal };
                painter.hline(r.x_range().shrink(4.0), r.bottom(), Stroke::new(1.0, mix(pal.view, pal.border, 0.5)));
                // eye
                let eye_r = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(22.0, 22.0));
                let eye = ui.interact(eye_r, egui::Id::unique(("layer-eye", id)), Sense::click()).on_hover_text("Show / hide");
                crate::icons::paint(&painter, if visible { "view-visible" } else { "view-hidden" }, eye_r.center(), 16.0, if visible { pal.fg } else { pal.fg_muted });
                if eye.clicked() {
                    toggle = Some(li);
                }
                // clipped layers sit indented under their base, with Photoshop's bent arrow
                let ind = if clipped { 16.0 } else { 0.0 };
                if clipped {
                    let (ax, ay) = (r.left() + 34.0, r.center().y);
                    let st = Stroke::new(1.3, if visible { pal.fg } else { pal.fg_muted });
                    painter.add(egui::Shape::line(vec![pos2(ax, ay - 6.0), pos2(ax, ay + 4.0)], st));
                    painter.add(egui::Shape::line(vec![pos2(ax - 3.0, ay + 1.0), pos2(ax, ay + 4.5), pos2(ax + 3.0, ay + 1.0)], st));
                }
                // icon + name (elided)
                paint_tool_icon(&painter, kind_tool(kind), pos2(r.left() + 38.0 + ind, r.center().y), 14.0, if visible { pal.fg } else { pal.fg_muted });
                let del_w = 44.0;
                let name_w = (r.width() - 52.0 - ind - del_w - 6.0).max(10.0);
                let mut job = egui::text::LayoutJob::simple_singleline(elide(&name, 60), egui::FontId::proportional(13.0), if visible { pal.fg } else { pal.fg_muted });
                job.wrap = egui::text::TextWrapping { max_width: name_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let galley = ui.painter().layout_job(job);
                let gy = r.center().y - galley.size().y / 2.0;
                let renaming = self.renaming.as_ref().map(|(rid, _)| *rid == id).unwrap_or(false);
                if !renaming {
                    painter.galley(pos2(r.left() + 52.0 + ind, gy), galley, pal.fg);
                } else if let Some((_, buf)) = &mut self.renaming {
                    let name_r = Rect::from_min_size(pos2(r.left() + 48.0, r.top() + 4.0), vec2(name_w + 4.0, r.height() - 8.0));
                    let te = ui.put(name_r, egui::TextEdit::singleline(buf).font(egui::FontId::proportional(13.0)).margin(vec2(4.0, 2.0)));
                    if self.rename_fresh {
                        te.request_focus();
                        self.rename_fresh = false;
                    }
                    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                    if esc {
                        rename_done = Some(None);
                    } else if te.lost_focus() || enter {
                        rename_done = Some(Some((li, buf.clone())));
                    }
                }
                let resp = resp.on_hover_text("Click to edit. Double-click to rename. Drag to reorder. Right-click for more.");
                if resp.double_clicked() && !eye.hovered() {
                    self.renaming = Some((id, name.clone()));
                    self.rename_fresh = true;
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(230.0);
                    let top = li + 1 == n;
                    let bottom = li == 0;
                    let mut act = |a: LayerAct| menu_act = Some(a);
                    if menu_item(ui, "edit-rename", "&Rename", "", true) {
                        act(LayerAct::Rename(li));
                    }
                    if menu_item(ui, "edit-duplicate", "&Duplicate Layer", "", true) {
                        act(LayerAct::Duplicate(li));
                    }
                    if menu_item(ui, if visible { "view-hidden" } else { "view-visible" }, if visible { "&Hide Layer" } else { "&Show Layer" }, "", true) {
                        act(LayerAct::Toggle(li));
                    }
                    if menu_check(ui, clipped, "&Clip to Layer Below", "") && li > 0 && !locked {
                        act(LayerAct::Clip(li));
                    }
                    ui.separator();
                    if menu_item(ui, "go-top", "Bring to &Front", "", !top) {
                        act(LayerAct::Move(li, n - 1));
                    }
                    if menu_item(ui, "go-up", "Move &Up", "", !top) {
                        act(LayerAct::Move(li, li + 1));
                    }
                    if menu_item(ui, "go-down", "Move Do&wn", "", !bottom) {
                        act(LayerAct::Move(li, li.saturating_sub(1)));
                    }
                    if menu_item(ui, "go-bottom", "Send to &Back", "", !bottom) {
                        act(LayerAct::Move(li, 0));
                    }
                    ui.separator();
                    if menu_item(ui, "merge", "&Collapse Down", "", idle) {
                        act(LayerAct::CollapseTo(li));
                    }
                    if menu_item(ui, "merge", "Collapse &All", "", idle) {
                        act(LayerAct::CollapseAll);
                    }
                    ui.separator();
                    if menu_item(ui, "edit-delete", "De&lete Layer", "", true) {
                        act(LayerAct::Delete(li));
                    }
                });
                // the lock: always shown when on, on hover otherwise
                let over = ui.rect_contains_pointer(r);
                if locked || over || selected {
                    let lock_r = Rect::from_center_size(pos2(r.right() - 36.0, r.center().y), vec2(20.0, 20.0));
                    let lk = ui.interact(lock_r, egui::Id::unique(("layer-lock", id)), Sense::click()).on_hover_text(if locked { "Unlock" } else { "Lock" });
                    let col = if locked || lk.hovered() { pal.fg } else { pal.fg_muted };
                    crate::icons::paint(&painter, if locked { "object-locked" } else { "object-unlocked" }, lock_r.center(), 16.0, col);
                    if lk.clicked() {
                        lock = Some(li);
                    }
                }
                // delete (pointer anywhere over the row – the ✕ itself takes the hover)
                if over || selected {
                    let del_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(20.0, 20.0));
                    let del = ui.interact(del_r, egui::Id::unique(("layer-del", id)), Sense::click()).on_hover_text("Delete layer");
                    crate::icons::paint(&painter, "window-close", del_r.center(), 16.0, if del.hovered() { pal.negative } else { pal.fg_muted });
                    if del.clicked() {
                        remove = Some(li);
                    }
                }
                if resp.clicked() && !eye.clicked() {
                    select = Some((kind, id));
                }
                if resp.drag_started() {
                    self.layer_drag = Some(id);
                }
                if dragging_me {
                    painter.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, pal.highlight), egui::StrokeKind::Inside);
                }
            }
            // base frames row
            // active frame edits, each opening its own tool, then the frames themselves
            let mut edit_rows: Vec<(Tool, String, Option<bool>)> = self.frame_edit_rows().into_iter().map(|(t, l, on)| (t, l, Some(on))).collect();
            edit_rows.push((Tool::Frames, "Frames".to_string(), None));
            for (t, label, enabled) in edit_rows {
                let shown = self.edit_labels.get(&t).cloned().unwrap_or_else(|| label.clone());
                if !find.is_empty() && !shown.to_lowercase().contains(&find) && !t.label().to_lowercase().contains(&find) {
                    continue;
                }
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h - 4.0), Sense::click());
                let painter = ui.painter();
                let sel = self.tool == t;
                if sel {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, pal.highlight);
                } else if resp.hovered() {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 3.0, mix(pal.view, pal.highlight, 0.18));
                }
                let pal = if sel { theme::Palette { fg: Color32::WHITE, fg_muted: mix(Color32::WHITE, pal.highlight, 0.35), ..pal } } else { pal };
                let on = enabled.unwrap_or(true);
                let col = if t == Tool::Frames || !on { pal.fg_muted } else { pal.fg };
                let mut eye_hit = false;
                if let Some(on) = enabled {
                    let eye_r = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(22.0, 22.0));
                    let eye = ui.interact(eye_r, egui::Id::unique(("edit-eye", t)), Sense::click()).on_hover_text("Turn this edit on or off");
                    crate::icons::paint(painter, if on { "view-visible" } else { "view-hidden" }, eye_r.center(), 16.0, if on { pal.fg } else { pal.fg_muted });
                    if eye.clicked() {
                        edit_toggle = Some(t);
                    }
                    eye_hit = eye.hovered();
                    if ui.rect_contains_pointer(r) || sel {
                        let del_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(20.0, 20.0));
                        let del = ui.interact(del_r, egui::Id::unique(("edit-del", t)), Sense::click()).on_hover_text("Remove this edit");
                        crate::icons::paint(painter, "window-close", del_r.center(), 16.0, if del.hovered() { pal.negative } else { pal.fg_muted });
                        if del.clicked() {
                            edit_remove = Some(t);
                        }
                        eye_hit |= del.hovered();
                    }
                }
                paint_tool_icon(&painter, t, pos2(r.left() + 38.0, r.center().y), 13.0, pal.fg_muted);
                let custom = self.edit_labels.get(&t).cloned();
                let shown = custom.clone().unwrap_or_else(|| label.clone());
                let editing = self.edit_renaming.as_ref().map(|(et, _)| *et == t).unwrap_or(false);
                if !editing {
                    let mut job = egui::text::LayoutJob::simple_singleline(shown, egui::FontId::proportional(12.5), col);
                    job.wrap = egui::text::TextWrapping { max_width: (r.width() - 80.0).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let galley = ui.painter().layout_job(job);
                    painter.galley(pos2(r.left() + 52.0, r.center().y - galley.size().y / 2.0), galley, col);
                } else if let Some((_, buf)) = &mut self.edit_renaming {
                    let name_r = Rect::from_min_size(pos2(r.left() + 48.0, r.top() + 3.0), vec2((r.width() - 80.0).max(40.0), r.height() - 6.0));
                    let te = ui.put(name_r, egui::TextEdit::singleline(buf).font(egui::FontId::proportional(12.5)).margin(vec2(4.0, 1.0)));
                    if self.rename_fresh {
                        te.request_focus();
                        self.rename_fresh = false;
                    }
                    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                    if esc {
                        edit_rename_done = Some(None);
                    } else if te.lost_focus() || enter {
                        edit_rename_done = Some(Some((t, buf.clone())));
                    }
                }
                if t != Tool::Frames && resp.double_clicked() && !eye_hit {
                    self.edit_renaming = Some((t, custom.clone().unwrap_or_else(|| t.label().to_string())));
                    self.rename_fresh = true;
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(230.0);
                    if let Some(on) = enabled {
                        if menu_item(ui, "edit-rename", "&Rename", "", true) {
                            menu_act = Some(LayerAct::RenameEdit(t));
                        }
                        if menu_item(ui, if on { "view-hidden" } else { "view-visible" }, if on { "Turn O&ff" } else { "Turn &On" }, "", true) {
                            edit_toggle = Some(t);
                        }
                        ui.separator();
                    }
                    if menu_item(ui, "dialog-ok-apply", "Collapse Frame Edits", "", idle && frame_edits) {
                        menu_act = Some(LayerAct::CollapseEdits);
                    }
                    if menu_item(ui, "merge", "Collapse All", "", idle && n > 0) {
                        menu_act = Some(LayerAct::CollapseAll);
                    }
                    if enabled.is_some() {
                        ui.separator();
                        if menu_item(ui, "edit-delete", "Re&move Edit", "", true) {
                            edit_remove = Some(t);
                        }
                    }
                });
                let tip = if t == Tool::Frames {
                    "Source frames".to_string()
                } else if custom.is_some() {
                    format!("{label}\nDouble-click to rename")
                } else {
                    "Click to adjust. Double-click to rename.".to_string()
                };
                if resp.on_hover_text(tip).clicked() && !eye_hit {
                    if t == Tool::Frames {
                        to_frames = true;
                    } else {
                        self.tool = t;
                        self.drag = None;
                    }
                }
            }
            // drag-to-reorder: insertion line + drop
            if let Some(did) = self.layer_drag {
                if let Some(pp) = ui.ctx().pointer_hover_pos().or_else(|| ui.ctx().pointer_interact_pos()) {
                    if let Some(from) = self.stack.index_of(did) {
                        // rows are top-first; compute stack insertion position
                        let mut pos = 0usize; // stack index to insert at (0 = bottom)
                        let mut line_y = rows.last().map(|r| r.1.bottom()).unwrap_or(pp.y);
                        for (li, rr) in &rows {
                            if pp.y < rr.center().y {
                                pos = li + 1;
                                line_y = rr.top();
                                break;
                            }
                        }
                        ui.painter().hline(rect.x_range().shrink(4.0), line_y, Stroke::new(2.0, pal.highlight));
                        if ui.ctx().input(|i| i.pointer.any_released()) {
                            drop_to = Some((from, pos));
                        }
                    }
                }
                if !ui.ctx().input(|i| i.pointer.any_down()) {
                    self.layer_drag = None;
                }
            }
            if n == 0 {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    hint(ui, "No layers yet. Pick a tool to add one.");
                });
            }
        });
        // buttons for the selected row, as another way in than right-click
        let sel_layer = self.tool.layer_kind().and_then(|k| self.sel_index(k));
        let sel_edit = (self.tool.layer_kind().is_none() && self.edit_rows_shown.contains(&self.tool)).then_some(self.tool);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let what = if sel_layer.is_some() { "layer" } else { "edit" };
            if icon_tool(ui, "edit-duplicate", "Duplicate the selected layer", sel_layer.is_some()).clicked() {
                menu_act = sel_layer.map(LayerAct::Duplicate);
            }
            if icon_tool(ui, "edit-rename", &format!("Rename the selected {what}"), sel_layer.is_some() || sel_edit.is_some()).clicked() {
                menu_act = sel_layer.map(LayerAct::Rename).or(sel_edit.map(LayerAct::RenameEdit));
            }
            if icon_tool(ui, "edit-delete", &format!("Delete the selected {what}"), sel_layer.is_some() || sel_edit.is_some()).clicked() {
                match (sel_layer, sel_edit) {
                    (Some(li), _) => menu_act = Some(LayerAct::Delete(li)),
                    (None, Some(t)) => edit_remove = Some(t),
                    _ => {}
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_tool(ui, "merge", "Collapse all layers and frame edits into the frames", idle && (n > 0 || frame_edits)).clicked() {
                    menu_act = Some(LayerAct::CollapseAll);
                }
            });
        });
        // fill what's left: the panel takes its size from its contents, so
        // without this it would shrink a little every frame
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 0.0 {
            ui.allocate_rect(rest, Sense::hover());
        }
        if let Some((k, id)) = select {
            self.tool = kind_tool(k);
            self.select(k, id);
        }
        if let Some(li) = toggle {
            self.stack.layers[li].visible = !self.stack.layers[li].visible;
            self.modified = true;
        }
        if let Some(li) = lock {
            self.stack.layers[li].locked = !self.stack.layers[li].locked;
            self.modified = true;
        }
        if let Some((from, pos)) = drop_to {
            self.layer_drag = None;
            if from < n && pos <= n && pos != from && pos != from + 1 {
                self.snapshot(&format!("reorder{now}"), now);
                let l = self.stack.layers.remove(from);
                let pos = if pos > from { pos - 1 } else { pos };
                self.stack.layers.insert(pos.min(self.stack.layers.len()), l);
            }
        }
        if let Some(done) = rename_done {
            if let Some((li, text)) = done {
                if let Some(l) = self.stack.layers.get_mut(li) {
                    let t = text.trim().to_string();
                    l.label = if t.is_empty() { None } else { Some(t) };
                }
            }
            self.renaming = None;
        }
        if let Some(done) = edit_rename_done {
            if let Some((t, text)) = done {
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.edit_labels.remove(&t);
                } else {
                    self.edit_labels.insert(t, text);
                }
            }
            self.edit_renaming = None;
        }
        if let Some(t) = edit_toggle {
            let tr = &mut self.stack.transform;
            match t {
                Tool::Cut => tr.cut_on = !tr.cut_on,
                Tool::Speed => tr.speed_on = !tr.speed_on,
                Tool::Reverse => tr.play_on = !tr.play_on,
                Tool::Crop => tr.crop_on = !tr.crop_on,
                Tool::Resize => tr.resize_on = !tr.resize_on,
                Tool::Rotate => tr.rot_on = !tr.rot_on,
                _ => {}
            }
        }
        if let Some(t) = edit_remove {
            self.reset_frame_edit(t);
        }
        if let Some(i) = remove {
            self.snapshot(&format!("del{now}"), now);
            self.stack.layers.remove(i);
        }
        if to_frames {
            self.tool = Tool::Frames;
        }
        if let Some(act) = menu_act {
            self.apply_layer_act(ctx, act, now);
        }
    }

    /// Make one layer and every visible layer below it part of the frames.
    /// Layers above stay editable and frame edits stay live.
    fn collapse_down(&mut self, ctx: &egui::Context, li: usize) {
        let Some(base) = self.base.clone() else { return };
        if li >= self.stack.layers.len() {
            return;
        }
        let lower = Stack { transform: Transform::default(), layers: self.stack.layers[..=li].iter().filter(|l| l.visible).cloned().collect(), next_id: self.stack.next_id, ..Default::default() };
        let mut keep = self.stack.clone();
        // hidden layers in the merged part stay as they are
        keep.layers = self.stack.layers[..=li].iter().filter(|l| !l.visible).cloned().chain(self.stack.layers[li + 1..].iter().cloned()).collect();
        let fonts = self.fonts.clone();
        let total = base.len();
        self.spawn_in(ctx, "Collapse down", total, false, move |p| {
            let out = lower.flatten(&base, &fonts, Some(p));
            JobOutput::Baked(Ok((out, keep)))
        });
    }

    /// A layer action from the layer list or the timeline.
    fn apply_layer_act(&mut self, ctx: &egui::Context, act: LayerAct, now: f64) {
        let n = self.stack.layers.len();
        match act {
            LayerAct::Rename(li) => {
                let l = &self.stack.layers[li];
                self.renaming = Some((l.id, l.name()));
                self.rename_fresh = true;
            }
            LayerAct::RenameEdit(t) => {
                self.edit_renaming = Some((t, self.edit_labels.get(&t).cloned().unwrap_or_else(|| t.label().to_string())));
                self.rename_fresh = true;
            }
            LayerAct::Duplicate(li) => {
                self.snapshot(&format!("duplicate{now}"), now);
                let src = self.stack.layers[li].clone();
                let id = self.stack.push(src.kind.clone());
                if let Some(mut l) = self.stack.layers.pop() {
                    l.label = src.label.as_ref().map(|s| format!("{s} copy"));
                    l.visible = src.visible;
                    // the copy keeps its clips, keyframes and colour
                    l.timing = src.timing.clone();
                    l.color = src.color;
                    l.opacity = src.opacity;
                    l.clip = src.clip;
                    self.stack.layers.insert(li + 1, l);
                }
                self.select(src.kind(), id);
                self.modified = true;
            }
            LayerAct::Toggle(li) => {
                self.stack.layers[li].visible = !self.stack.layers[li].visible;
                self.modified = true;
            }
            LayerAct::Clip(li) => {
                self.stack.layers[li].clip = !self.stack.layers[li].clip;
                self.modified = true;
            }
            LayerAct::Move(from, to) => {
                if from != to && to < n {
                    self.snapshot(&format!("reorder{now}"), now);
                    let l = self.stack.layers.remove(from);
                    self.stack.layers.insert(to, l);
                }
            }
            LayerAct::Delete(li) => {
                self.snapshot(&format!("del{now}"), now);
                self.stack.layers.remove(li);
                self.modified = true;
            }
            LayerAct::CollapseTo(li) => self.collapse_down(ctx, li),
            LayerAct::CollapseAll => self.bake(ctx, false),
            LayerAct::CollapseEdits => self.bake(ctx, true),
        }
    }

    /// Is a frame edit switched on?
    fn edit_enabled(&self, t: Tool) -> bool {
        let tr = &self.stack.transform;
        match t {
            Tool::Cut => tr.cut_on,
            Tool::Speed => tr.speed_on,
            Tool::Reverse => tr.play_on && (tr.reverse || tr.boomerang || tr.set_loop),
            Tool::Crop => tr.crop_on,
            Tool::Resize => tr.resize_on,
            Tool::Rotate => tr.rotates(),
            _ => false,
        }
    }

    /// Set a frame edit back to its defaults and drop its row.
    fn reset_frame_edit(&mut self, t: Tool) {
        let dur = self.base.as_ref().map(|a| a.duration_secs()).unwrap_or(1.0);
        let tr = &mut self.stack.transform;
        match t {
            Tool::Cut => {
                tr.cut_on = false;
                tr.cut_start = 1;
                tr.cut_end = 0;
                tr.cut_t0 = 0.0;
                tr.cut_t1 = dur;
            }
            Tool::Speed => {
                tr.speed_on = false;
                tr.speed_mode = SpeedMode::Percent;
                tr.speed_val = 100.0;
            }
            Tool::Reverse => {
                tr.reverse = false;
                tr.boomerang = false;
                tr.set_loop = false;
                tr.play_on = true;
            }
            Tool::Crop => tr.crop_on = false,
            Tool::Resize => {
                tr.resize_on = false;
                tr.rs_w = 0;
                tr.rs_h = 0;
                tr.rs_pct = 0.0;
            }
            Tool::Rotate => {
                tr.rot_on = false;
                tr.flip_h = false;
                tr.flip_v = false;
                tr.rot = 0;
                tr.rot_any = false;
            }
            _ => {}
        }
        self.edit_rows_shown.remove(&t);
        self.edit_labels.remove(&t);
        self.modified = true;
    }

    /// One (tool, label, enabled) per frame edit that is on – or was on and
    /// is now switched off – in pipeline order.
    fn frame_edit_rows(&mut self) -> Vec<(Tool, String, bool)> {
        for t in [Tool::Cut, Tool::Speed, Tool::Reverse, Tool::Crop, Tool::Resize, Tool::Rotate] {
            if self.edit_enabled(t) {
                self.edit_rows_shown.insert(t);
            }
        }
        let t = &self.stack.transform;
        let shown = |x: Tool| self.edit_rows_shown.contains(&x);
        let mut v = Vec::new();
        if shown(Tool::Cut) {
            let what = match &self.base {
                Some(b) => {
                    let (s, e) = t.cut_range(b);
                    format!("frames {}–{}", s + 1, e + 1)
                }
                None => String::new(),
            };
            v.push((Tool::Cut, format!("Cut · {what}"), t.cut_on));
        }
        if shown(Tool::Speed) {
            let what = match t.speed_mode {
                SpeedMode::Percent => format!("{:.0}%", t.speed_val),
                SpeedMode::DelayAll => format!("{:.0} cs", t.speed_val),
                SpeedMode::Fps => format!("{:.1} fps", t.speed_val),
            };
            v.push((Tool::Speed, format!("Speed · {what}"), t.speed_on));
        }
        if shown(Tool::Reverse) && (t.reverse || t.boomerang || t.set_loop) {
            let mut p = Vec::new();
            if t.reverse {
                p.push("reverse");
            }
            if t.boomerang {
                p.push("boomerang");
            }
            if t.set_loop {
                p.push("loop");
            }
            v.push((Tool::Reverse, format!("Playback · {}", p.join(", ")), t.play_on));
        }
        if shown(Tool::Crop) {
            v.push((Tool::Crop, format!("Crop · {}×{}", t.crop[2], t.crop[3]), t.crop_on));
        }
        if shown(Tool::Resize) {
            let what = if t.rs_pct > 0.0 {
                format!("{}%", (t.rs_pct * 10.0).round() / 10.0)
            } else {
                match (t.rs_w, t.rs_h) {
                    (0, 0) => "unchanged".into(),
                    (w, 0) => format!("{w} px wide"),
                    (0, h) => format!("{h} px tall"),
                    (w, h) => format!("{w}×{h}"),
                }
            };
            v.push((Tool::Resize, format!("Resize · {what}"), t.resize_on));
        }
        if shown(Tool::Rotate) {
            let mut p = Vec::new();
            if t.flip_h || t.flip_v {
                p.push("flip".to_string());
            }
            if t.rot_any && t.rot_deg != 0.0 {
                p.push(format!("{:.0}°", t.rot_deg));
            } else if t.rot != 0 {
                p.push(format!("{}°", t.rot));
            }
            v.push((Tool::Rotate, format!("Rotate · {}", p.join(", ")), t.rot_on));
        }
        v
    }

    /// KDE placeholder message: a big grey icon, a heading, a line of help and
    /// a button. The whole area also takes dropped files.
    fn empty_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let r = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(r, Sense::click());
        let c = r.center();
        crate::icons::paint(ui.painter(), "document-open", c - vec2(0.0, 70.0), 64.0, mix(pal.canvas, pal.fg, 0.4));
        ui.painter().text(c + vec2(0.0, -6.0), egui::Align2::CENTER_CENTER, "No Animation Open", theme::medium(20.0), mix(pal.canvas, pal.fg, 0.75));
        let help = if self.ffmpeg { "Open or drop a GIF, APNG, WebP, video or a set of images" } else { "Open or drop a GIF, APNG, WebP or a set of images" };
        ui.painter().text(c + vec2(0.0, 22.0), egui::Align2::CENTER_CENTER, help, egui::TextStyle::Body.resolve(ui.style()), mix(pal.canvas, pal.fg, 0.55));
        let br = Rect::from_center_size(c + vec2(0.0, 66.0), vec2(120.0, CHIP_H));
        let b = egui::Button::image_and_text(crate::icons::image(ctx, "document-open", 16.0, pal.fg), "Open…");
        let clicked = ui.put(br, b).clicked();
        if (clicked || resp.double_clicked()) && !self.busy() {
            self.open_dialog(ctx);
        }
    }

    /// The timeline bar: playback, position, time and playback speed. Nothing else.
    fn transport(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 5.0, Stroke::new(1.0, pal.border));
        let n = self.plan.len();
        let (delay, enabled) = self.plan.get(self.cur).map(|f| (f.delay, f.enabled)).unwrap_or((0, true));
        let total: f32 = self.plan.iter().filter(|f| f.enabled).map(|f| effective_delay(f.delay) as f32 / 100.0).sum();
        let now_t = self.play_time().min(total);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if icon_tool(ui, "media-skip-backward", "Previous frame (←)", true).clicked() {
                self.step(-1);
            }
            let (icon, tip) = if self.playing { ("media-playback-pause", "Pause (Space)") } else { ("media-playback-start", "Play (Space)") };
            // flat media buttons, as KDE players have them
            if icon_tool(ui, icon, tip, true).clicked() {
                self.playing = !self.playing;
            }
            if icon_tool(ui, "media-skip-forward", "Next frame (→)", true).clicked() {
                self.step(1);
            }
            ui.add_space(6.0);
            // fixed-width readouts so the slider never jumps as the numbers change
            let digits = n.max(1).to_string().len();
            let time_w = 112.0;
            let frame_w = 30.0 + 16.0 * digits as f32;
            let speed_w = 74.0;
            // the readouts and buttons sit on the right; the slider takes what's left
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // show or hide the timeline dock
                let on = self.show_timeline;
                let b = ui.add(sel_btn(egui::Button::new("").frame_when_inactive(on).min_size(vec2(TOOLBAR_H, TOOLBAR_H)), on)).on_hover_text(if on { "Hide the timeline" } else { "Show the timeline" });
                crate::icons::paint(ui.painter(), "player-time", b.rect.center(), 16.0, pal.fg);
                if b.clicked() {
                    self.show_timeline = !on;
                    theme::set_setting("show_timeline", if self.show_timeline { "1" } else { "0" });
                }
                // in a narrow window the time goes first, then the speed menu
                let room = ui.available_width() - 90.0;
                let (show_speed, show_frame, show_time) = (room > speed_w + frame_w + 8.0, room > frame_w, room > speed_w + frame_w + time_w + 12.0);
                let speeds = [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 4.0];
                let label = |v: f32| if v == 1.0 { "1×".to_string() } else { format!("{v}×") };
                if show_speed {
                egui::ComboBox::from_id_salt("play_speed")
                    .icon(chevron)
                    .width(speed_w)
                    .selected_text(label(self.play_speed))
                    .show_ui(ui, |ui| {
                        for v in speeds {
                            if ui.selectable_label(self.play_speed == v, label(v)).clicked() {
                                self.play_speed = v;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Playback speed of the preview. Exports keep their own timing.");
                }
                let mono = |s: String| RichText::new(s).font(egui::FontId::monospace(12.5)).color(pal.fg);
                if show_frame {
                    ui.add_sized(vec2(frame_w, TOOLBAR_H), egui::Label::new(mono(format!("{:>w$} / {}", self.cur + 1, n, w = digits))))
                        .on_hover_text(format!("Frame. Delay {delay} cs{}", if enabled { "" } else { " · skipped" }));
                }
                if show_time {
                    ui.add_sized(vec2(time_w, TOOLBAR_H), egui::Label::new(mono(format!("{now_t:.2} / {total:.2} s")))).on_hover_text("Time and length in seconds");
                }
                ui.add_space(6.0);
                ui.spacing_mut().slider_width = (ui.available_width() - 4.0).max(60.0);
                let mut f = self.cur + 1;
                if slider(ui, egui::Slider::new(&mut f, 1..=n.max(1)).show_value(false)).changed() {
                    self.cur = f - 1;
                    self.playing = false;
                    self.elapsed = 0.0;
                }
            });
        });
    }

    /// View mode, zoom and display toggles (in the main toolbar).
    fn view_controls(&mut self, ui: &mut egui::Ui) {
        if tb_toggle(ui, "view-fullscreen", !self.compare, "Single view (C)") {
            self.compare = false;
        }
        if tb_toggle(ui, "view-split-left-right", self.compare, "Split view (C)") {
            self.compare = true;
        }
        // always there, greyed out in single view, so the toolbar never shifts
        let split = self.compare;
        if ui.add_enabled_ui(split, |ui| tb_toggle(ui, "link", self.link_views && split, "Link zoom and pan of both views")).inner {
            self.link_views = !self.link_views;
        }
        // zoom lives in each view's own corner
    }

    // ------------------------------------------------------------ export

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_open || self.base.is_none() {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut go = false;
        let mut cancel = false;
        if !self.export_name_edited {
            self.export_name = self.default_stem();
        }
        dialog(ctx, "export", "Export", vec2(660.0, 400.0), false, &mut open, |ui| {
                ui.set_width(600.0);
                ui.horizontal_top(|ui| {
                    // ---- preview of the file as it will be saved
                    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 260.0), Sense::hover());
                    self.export_preview(ui, ctx, rect);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(self.export_fmt.label()).font(theme::medium(17.0)).color(pal.fg));
                        let fresh = self.cmp.as_ref().filter(|c| c.key == self.cmp_key());
                        match fresh {
                            Some(c) => {
                                let n = c.size;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(c.size_text()).font(theme::medium(22.0)).color(pal.positive));
                                    if let Some(o) = self.file_size {
                                        hint(ui, &format!("{:+.0}% vs original", (n as f64 / o as f64 - 1.0) * 100.0));
                                    }
                                });
                            }
                            None if self.preview_format().is_none() => {
                                ui.label(RichText::new("Size shown after saving").font(theme::medium(15.0)).color(pal.fg_muted));
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new("Encoding…").font(theme::medium(15.0)).color(pal.fg_muted));
                                });
                            }
                        }
                        let (w, h) = (self.out_size.0 as u32, self.out_size.1 as u32);
                        // what's saved: the in to out part when that's asked for
                        let (ra, rb) = self.export_trim().0.unwrap_or((0, self.plan.len().saturating_sub(1)));
                        let part = || self.plan.iter().enumerate().filter(move |(i, f)| f.enabled && *i >= ra && *i <= rb).map(|(_, f)| f);
                        let n = part().count();
                        let dur: f32 = part().map(|f| effective_delay(f.delay) as f32 / 100.0).sum();
                        let dims = match self.export_fmt {
                            ExportFormat::Sheet => format!("{n} frames of {w}×{h} on one sheet"),
                            ExportFormat::Frames => format!("{n} files of {w}×{h}"),
                            _ => format!("{w}×{h} · {n} frames · {dur:.2}s"),
                        };
                        hint(ui, &dims);
                        ui.horizontal(|ui| {
                            hint(ui, &self.comp_summary());
                            if ui.link("Change").on_hover_text("Compression settings in Optimize").clicked() {
                                self.tool = Tool::Optimize;
                                cancel = true;
                            }
                        });
                        ui.add_space(10.0);
                        // a flat section: heading, thin line, then the rows
                        ui.label(RichText::new("Save Options").font(theme::medium(13.5)).color(pal.fg));
                        let r = ui.available_rect_before_wrap();
                        ui.painter().hline(r.x_range(), r.top() + 3.0, Stroke::new(1.0, pal.border));
                        ui.add_space(10.0);
                        ui.scope(|ui| {
                            ui.set_width(ui.available_width());
                            self.save_options_ui(ui);
                        });
                    });
                });
                ui.add_space(8.0);
                let folder = self.export_fmt == ExportFormat::Frames && !self.frames_zip;
                ui.horizontal(|ui| {
                    if folder {
                        hint(ui, "You'll pick a folder for the frames");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // KDE order: the main action first, then Cancel
                        if ui.add(egui::Button::image_and_text(crate::icons::image(ui.ctx(), "dialog-cancel", 16.0, pal.fg), "Cancel").min_size(vec2(96.0, CHIP_H))).clicked() {
                            cancel = true;
                        }
                        let icon = crate::icons::image(ui.ctx(), if folder { "document-open" } else { "document-save" }, 16.0, pal.fg);
                        go = ui.add(egui::Button::image_and_text(icon, if folder { "Choose Folder…" } else { "Save…" }).stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(96.0, CHIP_H))).clicked();
                    });
                });
            });
        if !open || cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.export_open = false;
        }
        if go {
            self.export_open = false;
            self.do_export(ctx);
        }
    }

    /// Small animated preview of the output inside the Export window.
    fn export_preview(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect) {
        let pal = *p();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, pal.canvas);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        let fresh = self.cmp.as_ref().map(|c| c.key == self.cmp_key()).unwrap_or(false);
        let (tex, size) = if fresh {
            let (i, size) = {
                let c = self.cmp.as_ref().unwrap();
                (c.frame_for(self.cur, self.play_time(), &self.plan), vec2(c.full.0 as f32, c.full.1 as f32))
            };
            (self.cmp_texture(ctx, i).map(Tex::from), size)
        } else {
            let t = self.composite_texture(ctx, self.cur);
            let size = t.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(self.out_size.0, self.out_size.1));
            (t, size)
        };
        let inner = rect.shrink(10.0);
        let sc = (inner.width() / size.x).min(inner.height() / size.y);
        let ir = Rect::from_center_size(inner.center(), size * sc);
        self.paint_backdrop(&painter, ctx, ir, ir);
        if let Some(t) = tex {
            painter.image(t.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if fresh { Color32::WHITE } else { Color32::from_white_alpha(140) });
        }
        if !fresh && self.cmp_job.is_some() {
            let c = rect.center();
            let t = ui.input(|i| i.time) as f32;
            painter.circle_filled(c, 16.0, Color32::from_black_alpha(165));
            painter.circle_stroke(c, 10.0, Stroke::new(2.5, Color32::from_white_alpha(60)));
            let a = t * 6.0;
            painter.line_segment([c, c + vec2(a.cos(), a.sin()) * 10.0], Stroke::new(2.5, pal.highlight));
            ctx.request_repaint();
        }
    }

    /// Save dialog starting in the last used folder.
    fn save_path(&mut self, filter: &str, exts: &[&str]) -> Option<PathBuf> {
        // Copy Result: to a file of its own, no dialog
        if self.copy_to.is_some() {
            let p = self.copy_to.take()?.with_extension(exts[0]);
            self.copy_pending = Some(p.clone());
            return Some(p);
        }
        let mut d = rfd::FileDialog::new().add_filter(filter, exts).set_file_name(format!("{}.{}", self.export_stem(), exts[0]));
        if let Some(dir) = &self.last_dir {
            d = d.set_directory(dir);
        }
        let p = d.save_file()?;
        self.remember_dir(p.parent().map(|d| d.to_path_buf()));
        Some(p)
    }

    fn pick_dir(&mut self) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = &self.last_dir {
            d = d.set_directory(dir);
        }
        let p = d.pick_folder()?;
        self.remember_dir(Some(p.clone()));
        Some(p)
    }

    fn remember_dir(&mut self, d: Option<PathBuf>) {
        let Some(d) = d else { return };
        theme::set_setting("last_dir", &d.to_string_lossy());
        self.pending_open = self.open_after_save.then(|| d.clone());
        self.last_dir = Some(d);
    }

    /// Report a finished save and open its folder when asked to.
    fn saved(&mut self, r: anyhow::Result<String>) {
        if let Some(p) = self.copy_pending.take() {
            // made for the clipboard: put it there
            match r {
                Ok(_) => match copy_file_to_clipboard(&p) {
                    Ok(()) => self.info(format!("Copied {} ({}): paste it into a chat, a post or a folder", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), fmt_size(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)))),
                    Err(e) => self.error(format!("Couldn't copy: {e:#}")),
                },
                Err(e) => self.error(format!("{e:#}")),
            }
            return;
        }
        match r {
            Ok(s) => {
                self.info(s);
                if let Some(d) = self.pending_open.take() {
                    open_in_file_manager(&d);
                }
            }
            Err(e) => {
                self.pending_open = None;
                self.error(format!("{e:#}"));
            }
        }
    }

    /// Copy Result: save the output (as Export would) to a file of its own
    /// and put that file on the clipboard.
    pub(super) fn copy_result(&mut self, ctx: &egui::Context) {
        if self.base.is_none() || self.busy() {
            return;
        }
        if self.export_fmt == ExportFormat::Frames {
            self.info("Frames save as a folder: pick another format to copy");
            return;
        }
        let dir = std::env::temp_dir().join("gifkit-copied");
        let _ = std::fs::create_dir_all(&dir);
        // last time's copies go (anything already pasted keeps its own)
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(e.path());
        }
        self.copy_to = Some(dir.join(self.export_stem()));
        self.do_export(ctx);
        self.copy_to = None;
    }

    fn do_export(&mut self, ctx: &egui::Context) {
        if self.base.is_none() {
            return;
        }
        self.pending_open = None;
        // the preview already holds the encoded file: save it directly
        let noai = self.gif_opts.noai_tag && self.export_fmt.can_tag();
        // video tags are written by ffmpeg, so a tagged video is encoded again
        let reuse = !matches!(self.export_fmt, ExportFormat::Frames | ExportFormat::Sheet) && !(noai && matches!(self.export_fmt, ExportFormat::Mp4 | ExportFormat::Webm));
        if reuse && self.preview_format().is_some() {
            if let Some(bytes) = self.cmp.as_ref().filter(|c| c.key == self.cmp_key() && c.sample.is_none()).map(|c| c.bytes.clone()) {
                let bytes: Arc<Vec<u8>> = match self.export_fmt {
                    ExportFormat::Webp if noai => Arc::new(crate::noai::tag_webp((*bytes).clone())),
                    ExportFormat::Apng if noai => Arc::new(crate::noai::tag_png((*bytes).clone())),
                    ExportFormat::Avif if noai => Arc::new(crate::noai::tag_avif((*bytes).clone())),
                    _ => bytes,
                };
                let ext = self.export_ext();
                if let Some(path) = self.save_path(&self.export_fmt.label().to_string(), &[ext]) {
                    let r = std::fs::write(&path, &*bytes).map(|_| format!("Saved {} ({})", path.display(), fmt_size(bytes.len() as u64))).map_err(anyhow::Error::from);
                    self.saved(r);
                }
                return;
            }
        }
        match self.export_fmt {
            ExportFormat::Gif if self.untouched_gif().is_some() => {
                let src = self.untouched_gif().unwrap();
                if let Some(path) = self.save_path("GIF", &["gif"]) {
                    let r = std::fs::read(&src).map(|b| if noai { crate::encode::tag_gif(b) } else { b }).and_then(|b| std::fs::write(&path, &b).map(|_| b.len())).map(|n| format!("Saved {} ({}, unchanged)", path.display(), fmt_size(n as u64))).map_err(anyhow::Error::from);
                    self.saved(r);
                }
            }
            ExportFormat::Gif => {
                if let Some(path) = self.save_path("GIF", &["gif"]) {
                    let opts = self.gif_opts.clone();
                    self.run_export(ctx, "Save GIF", move |a, p| io::save_gif(a, &opts, &path, Some(p)).map(|n| format!("Saved {} ({})", path.display(), fmt_size(n))));
                }
            }
            ExportFormat::Webp => {
                if let Some(path) = self.save_path("WebP", &["webp"]) {
                    let o = self.webp_opts;
                    self.run_export(ctx, "Save WebP", move |a, _| {
                        let b = io::encode_webp(a, &o)?;
                        let b = if noai { crate::noai::tag_webp(b) } else { b };
                        std::fs::write(&path, &b)?;
                        Ok(format!("Saved {} ({})", path.display(), fmt_size(b.len() as u64)))
                    });
                }
            }
            ExportFormat::Avif => {
                if let Some(path) = self.save_path("AVIF", &["avif"]) {
                    let q = self.avif_quality;
                    self.run_export(ctx, "Save AVIF", move |a, _| {
                        let b = io::encode_avif(a, q)?;
                        let b = if noai { crate::noai::tag_avif(b) } else { b };
                        std::fs::write(&path, &b)?;
                        Ok(format!("Saved {} ({}){}", path.display(), fmt_size(b.len() as u64), crate::gpu::last_used_note()))
                    });
                }
            }
            ExportFormat::Apng => {
                if let Some(path) = self.save_path("PNG", &["png", "apng"]) {
                    let tag = self.gif_opts.noai_tag;
                    self.run_export(ctx, "Save APNG", move |a, _| io::save_apng(a, &path, tag).map(|n| format!("Saved {} ({})", path.display(), fmt_size(n))));
                }
            }
            ExportFormat::Mp4 | ExportFormat::Webm => {
                let (fmt, ext) = if self.export_fmt == ExportFormat::Mp4 { (VideoFormat::Mp4, "mp4") } else { (VideoFormat::WebmAlpha, "webm") };
                if let Some(path) = self.save_path(ext, &[ext]) {
                    let bg = self.video_bg;
                    self.run_export(ctx, "Export video", move |a, _| io::export_video(a, &path, fmt, bg, noai).map(|_| format!("Saved {}{}", path.display(), crate::gpu::last_used_note())));
                }
            }
            ExportFormat::Frames => {
                let pre = self.split_prefix.clone();
                if self.frames_zip {
                    if let Some(path) = self.save_path("ZIP", &["zip"]) {
                        let png = io::PngOptions { noai, ..self.frames_png };
                        self.run_export(ctx, "ZIP frames", move |a, _| io::export_frames_zip_opts(a, &path, &pre, &png).map(|n| format!("Wrote {n} frames to {}", path.display())));
                    }
                } else if let Some(dir) = self.pick_dir() {
                    let (fmt, png, gif) = (self.split_fmt, io::PngOptions { noai, ..self.frames_png }, self.gif_opts.clone());
                    self.run_export(ctx, "Split frames", move |a, _| io::export_frames_dir_opts(a, &dir, &pre, fmt, &png, &gif).map(|n| format!("Wrote {n} frames to {}", dir.display())));
                }
            }
            ExportFormat::Sheet => {
                if let Some(path) = self.save_path("PNG", &["png"]) {
                    let mut opts = self.sheet.clone();
                    opts.png.noai = noai;
                    self.run_export(ctx, "Sprite sheet", move |a, _| io::export_spritesheet(a, &path, &opts).map(|s| format!("Wrote {s}")));
                }
            }
        }
    }

    // ------------------------------------------------------------ project files

    fn proj_settings(&self) -> ProjSettings {
        ProjSettings {
            gif: Some(self.gif_opts.clone()),
            webp: Some(self.webp_opts),
            avif: Some(self.avif_quality),
            format: self.export_fmt.label().into(),
            video_bg: Some(self.video_bg),
            frames_png: Some(self.frames_png),
            split_fmt: Some(self.split_fmt),
            frames_zip: Some(self.frames_zip),
            split_prefix: Some(self.split_prefix.clone()),
            sheet: Some(self.sheet.clone()),
            target_val: Some(self.target_val),
            target_mb: Some(self.target_mb),
            tool: self.tool.label().into(),
            edit_labels: self.edit_labels.iter().map(|(t, l)| (t.label().to_string(), l.clone())).collect(),
        }
    }

    /// Save to the project's file, or ask where (always with `save_as`).
    fn save_project(&mut self, ctx: &egui::Context, save_as: bool) {
        use crate::project::EXT;
        let Some(base) = self.base.clone() else { return };
        if self.busy() {
            return;
        }
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let mut d = rfd::FileDialog::new().add_filter("GIFkit project", &[EXT]).set_file_name(format!("{}.{EXT}", self.base_name()));
                if let Some(dir) = self.project_path.as_ref().and_then(|p| p.parent()).or(self.last_dir.as_deref()) {
                    d = d.set_directory(dir);
                }
                let Some(mut p) = d.save_file() else { return };
                if !crate::project::is_project(&p) {
                    p.set_extension(EXT);
                }
                p
            }
        };
        let (file, overlays, watermarks) = self.project_parts();
        let original = self.original.clone();
        self.spawn_in(ctx, "Save project", 1, false, move |_| JobOutput::ProjectSaved(crate::project::save(&path, file, &base, original.as_ref(), &overlays, &watermarks).map(|_| path.clone())));
    }

    /// The project as it would be saved: its description, and the pictures
    /// image and watermark layers use.
    #[allow(clippy::type_complexity)]
    fn project_parts(&self) -> (crate::project::ProjectFile, HashMap<usize, Vec<RgbaImage>>, HashMap<usize, Arc<RgbaImage>>) {
        use crate::project::{ProjLayer, ProjLayerEntry, ProjectFile};
        let mut overlays = HashMap::new();
        let mut watermarks = HashMap::new();
        let layers = self
            .stack
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let layer = match &l.kind {
                    LayerKind::Effects(fx) => ProjLayer::Effects { fx: (**fx).clone() },
                    LayerKind::Text(t) => ProjLayer::Text { text: t.clone(), font: self.font_key_of(t.font_idx) },
                    LayerKind::Censor(c) => ProjLayer::Censor { censor: c.clone() },
                    LayerKind::Overlay(o) => {
                        overlays.insert(i, o.src.frames.clone());
                        ProjLayer::Image { id: i, name: o.src.name.clone(), path: o.src.path.clone(), delays: o.src.delays.clone(), placements: o.placements.clone(), opacity: o.opacity }
                    }
                    LayerKind::Watermark(w) => {
                        let mut w2 = (**w).clone();
                        w2.font_key = self.font_key_of(w.font_idx);
                        if let Some(img) = &w.image {
                            watermarks.insert(i, img.clone());
                        }
                        ProjLayer::Watermark { id: i, watermark: w2, has_image: w.image.is_some() }
                    }
                    LayerKind::Shield(sh) => ProjLayer::Shield { shield: sh.clone() },
                    LayerKind::Shape(sh) => ProjLayer::Shape { shape: (**sh).clone() },
                    LayerKind::Markup(m) => ProjLayer::Markup { markup: m.clone() },
                    LayerKind::Emoji(e) => ProjLayer::Emoji { emoji: (**e).clone() },
                };
                ProjLayerEntry { visible: l.visible, label: l.label.clone(), layer, timing: l.timing.clone(), locked: l.locked, color: l.color, opacity: l.opacity, clip: l.clip }
            })
            .collect();
        let file = ProjectFile {
            source_name: self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            source_path: self.path.clone(),
            source_size: self.file_size,
            transform: self.stack.transform.clone(),
            layers,
            markers: self.stack.markers.clone(),
            gif_color: self.stack.gif_color,
            settings: serde_json::to_value(self.proj_settings()).unwrap_or_default(),
            ..Default::default()
        };
        (file, overlays, watermarks)
    }

    /// Where unsaved work is kept, and a note of what it was.
    fn autosave_paths() -> Option<(PathBuf, PathBuf)> {
        let dir = theme::config_path()?.parent()?.to_path_buf();
        Some((dir.join("autosave.gifkit"), dir.join("autosave.json")))
    }

    /// Every minute or so, unsaved work is written aside in the background,
    /// so a crash or a power cut loses little. A normal exit removes it.
    fn manage_autosave(&mut self, now: f64) {
        if let Some(rx) = &self.autosave_job {
            if rx.try_recv().is_ok() {
                self.autosave_job = None;
            }
            return;
        }
        // GIFKIT_AUTOSAVE_SECS: a shorter wait, for testing
        let every = std::env::var("GIFKIT_AUTOSAVE_SECS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(60.0);
        if !self.modified || self.base.is_none() || self.busy() || self.restore_offer.is_some() || now - self.autosave_at < every {
            return;
        }
        let sig = hash_parts(&[self.version, self.stack.signature(), self.timing_rev, self.stack.plan_signature(), self.stack.layers.len() as u64]);
        self.autosave_at = now;
        if sig == self.autosave_sig {
            return;
        }
        self.autosave_sig = sig;
        let Some((path, note)) = Self::autosave_paths() else { return };
        let (file, overlays, watermarks) = self.project_parts();
        let (base, original) = (self.base.clone().unwrap(), self.original.clone());
        let info = serde_json::json!({ "project": self.project_path, "source": self.path, "saved": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) });
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            stack::lower_thread_priority();
            let tmp = path.with_extension("gifkit.part");
            let ok = stack::bg_pool().install(|| crate::project::save(&tmp, file, &base, original.as_ref(), &overlays, &watermarks)).is_ok() && std::fs::rename(&tmp, &path).is_ok();
            if ok {
                let _ = std::fs::write(&note, info.to_string());
            }
            let _ = tx.send(ok);
        });
        self.autosave_job = Some(rx);
    }

    /// The work is saved, closed or discarded: nothing to restore.
    fn clear_autosave(&mut self) {
        if let Some((a, b)) = Self::autosave_paths() {
            let _ = std::fs::remove_file(a);
            let _ = std::fs::remove_file(b);
        }
        self.autosave_sig = 0;
    }

    /// GIFkit didn't close normally last time: offer the work it kept.
    fn restore_window(&mut self, ctx: &egui::Context) {
        let Some((path, when, project)) = self.restore_offer.clone() else { return };
        if ctx.cumulative_frame_nr() < 3 {
            ctx.request_repaint();
            return;
        }
        let pal = *p();
        let mut open = true;
        let (mut restore, mut discard) = (false, false);
        dialog_ex(ctx, "restore", "Restore Work", vec2(440.0, 200.0), false, true, &mut open, |ui| {
            ui.set_width(410.0);
            ui.label(RichText::new("Restore unsaved work?").font(theme::medium(16.0)).color(pal.fg));
            ui.add_space(6.0);
            ui.label("GIFkit didn't close normally last time. Your work was kept.");
            if !when.is_empty() {
                hint(ui, &when);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    discard = ui.button("Discard").clicked();
                    restore = ui.add(egui::Button::new("Restore").stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(90.0, 0.0))).clicked();
                });
            });
        });
        if restore {
            self.restore_offer = None;
            match crate::project::load(&path) {
                Ok(pr) => {
                    self.apply_project(pr, path);
                    // saving goes where the work came from, not over the kept copy
                    self.project_path = project;
                    self.modified = true;
                    self.info("Restored your unsaved work");
                }
                Err(e) => self.error(format!("Couldn't restore: {e:#}")),
            }
        } else if discard || !open {
            self.restore_offer = None;
            self.clear_autosave();
        }
    }

    fn apply_project(&mut self, pr: crate::project::Project, path: PathBuf) {
        use crate::project::ProjLayer;
        let crate::project::Project { file, base, original, mut overlays, mut watermarks } = pr;
        self.undo.clear();
        self.redo.clear();
        self.committed = None;
        self.path = file.source_path.clone();
        self.file_size = file.source_size;
        self.cmp = None;
        self.original = Some(original.unwrap_or_else(|| base.clone()));
        self.orig_gen += 1;
        let n = base.len();
        self.base = Some(base);
        self.stack = Stack::default();
        self.edit_labels.clear();
        self.edit_rows_shown.clear();
        self.export_name_edited = false;
        self.sel.clear();
        self.cur = 0;
        self.elapsed = 0.0;
        self.views = [ViewState::default(); 3];
        self.stack.transform = file.transform.clone();
        for e in file.layers {
            let kind = match e.layer {
                ProjLayer::Effects { fx } => LayerKind::Effects(Box::new(fx)),
                ProjLayer::Text { mut text, font } => {
                    text.font_idx = self.resolve_font(&font);
                    LayerKind::Text(text)
                }
                ProjLayer::Censor { censor } => LayerKind::Censor(censor),
                ProjLayer::Image { id, name, path, delays, placements, opacity } => {
                    let frames = overlays.remove(&id).unwrap_or_else(|| vec![RgbaImage::new(1, 1)]);
                    let delays = if delays.len() == frames.len() { delays } else { vec![10; frames.len()] };
                    LayerKind::Overlay(OverlayLayer { src: Arc::new(OverlaySource { name, path, frames, delays }), placements, opacity })
                }
                ProjLayer::Watermark { id, mut watermark, .. } => {
                    watermark.font_idx = self.resolve_font(&watermark.font_key);
                    watermark.image = watermarks.remove(&id).map(Arc::new);
                    LayerKind::Watermark(Box::new(watermark))
                }
                ProjLayer::Shield { shield } => LayerKind::Shield(shield),
                ProjLayer::Shape { shape } => LayerKind::Shape(Box::new(shape)),
                ProjLayer::Markup { markup } => LayerKind::Markup(markup),
                ProjLayer::Emoji { emoji } => LayerKind::Emoji(Box::new(emoji)),
            };
            let id = self.stack.push(kind);
            if let Some(l) = self.stack.layers.iter_mut().find(|l| l.id == id) {
                l.visible = e.visible;
                l.label = e.label;
                l.timing = e.timing;
                l.locked = e.locked;
                l.color = e.color;
                l.opacity = e.opacity;
                l.clip = e.clip;
            }
        }
        self.stack.markers = file.markers;
        self.stack.gif_color = file.gif_color;
        self.stack.sync_len(n);
        if let Ok(st) = serde_json::from_value::<ProjSettings>(file.settings) {
            if let Some(v) = st.gif {
                self.gif_opts = v;
            }
            if let Some(v) = st.webp {
                self.webp_opts = v;
            }
            if let Some(v) = st.avif {
                self.avif_quality = v;
            }
            if let Some(f) = ExportFormat::ALL.iter().find(|f| f.label() == st.format) {
                self.export_fmt = *f;
            }
            if let Some(v) = st.video_bg {
                self.video_bg = v;
            }
            if let Some(v) = st.frames_png {
                self.frames_png = v;
            }
            if let Some(v) = st.split_fmt {
                self.split_fmt = v;
            }
            if let Some(v) = st.frames_zip {
                self.frames_zip = v;
            }
            if let Some(v) = st.split_prefix {
                self.split_prefix = v;
            }
            if let Some(v) = st.sheet {
                self.sheet = v;
            }
            if let Some(v) = st.target_val {
                self.target_val = v;
            }
            if let Some(v) = st.target_mb {
                self.target_mb = v;
            }
            if let Some(t) = Tool::ALL.iter().find(|t| t.label() == st.tool) {
                self.tool = *t;
            }
            for (t, l) in st.edit_labels {
                if let Some(t) = Tool::ALL.iter().find(|x| x.label() == t) {
                    self.edit_labels.insert(*t, l);
                }
            }
        }
        // show the frame edit rows that are in use
        self.bump();
        self.after_base_change(false);
        self.modified = false;
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.project_path = Some(path);
        self.info(format!("Opened project {name}"));
    }

    // ------------------------------------------------------------ presets

    fn ui_presets(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let mut apply: Option<crate::presets::Preset> = None;
        let mut delete: Option<String> = None;
        let mut save = false;
        group(ui, "Save Current Edits", |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 70.0;
                ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(w));
                save = ui.add_enabled(!self.preset_name.trim().is_empty(), egui::Button::new("Save").min_size(vec2(64.0, CHIP_H))).clicked();
            });
            ui.add_space(2.0);
            form_row(ui, "Include", |ui| {
                check(ui, &mut self.preset_inc.0, "Frame edits");
                check(ui, &mut self.preset_inc.1, "Layers");
                check(ui, &mut self.preset_inc.2, "Compression");
            });
        });
        group(ui, "Saved Presets", |ui| {
            if !self.preset_problems.is_empty() {
                ui.label(RichText::new(format!("⚠ {} problem(s) reading presets", self.preset_problems.len())).color(pal.neutral).size(12.0)).on_hover_text(self.preset_problems.join("\n"));
            }
            if self.presets.is_empty() {
                hint(ui, "No presets yet");
            }
            for (i, pr) in self.presets.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 110.0).max(60.0));
                        ui.label(RichText::new(elide(&pr.name, 30)).font(theme::medium(13.0)).color(pal.fg));
                        hint(ui, &pr.summary());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if small_icon(ui, "edit-delete", "Delete preset") {
                            delete = Some(pr.name.clone());
                        }
                        if ui.add(egui::Button::new("Apply").min_size(vec2(64.0, CHIP_H))).clicked() {
                            apply = Some(pr.clone());
                        }
                    });
                });
            }
        });
        if save {
            let pr = self.make_preset(self.preset_name.trim().to_string());
            match crate::presets::save(&pr) {
                Ok(_) => {
                    self.info(format!("Saved preset “{}”", pr.name));
                    self.preset_name.clear();
                }
                Err(e) => self.error(format!("Could not save preset: {e:#}")),
            }
            let (l, pr) = crate::presets::list_with_problems();
            self.presets = l;
            self.preset_problems = pr;
        }
        if let Some(name) = delete {
            crate::presets::delete(&name);
            let (l, pr) = crate::presets::list_with_problems();
            self.presets = l;
            self.preset_problems = pr;
        }
        if let Some(pr) = apply {
            self.apply_preset(&pr, ctx);
        }
    }

    fn font_key_of(&self, idx: usize) -> Option<(String, String)> {
        if let Some(m) = self.font_meta.get(&idx) {
            return Some(m.clone());
        }
        self.fonts.get(idx).map(|f| ("builtin".to_string(), f.name.clone()))
    }

    fn resolve_font(&mut self, key: &Option<(String, String)>) -> usize {
        match key {
            Some((fam, style)) if fam == "builtin" => self.fonts.iter().position(|f| &f.name == style).unwrap_or(0),
            Some((fam, style)) => self.font_index_for(fam, style).unwrap_or(0),
            None => 0,
        }
    }

    fn make_preset(&self, name: String) -> crate::presets::Preset {
        use crate::presets::{Preset, PresetLayer};
        let (inc_t, inc_l, inc_g) = self.preset_inc;
        let src = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let layers = inc_l.then(|| {
            self.stack
                .layers
                .iter()
                .filter_map(|l| {
                    let visible = l.visible;
                    Some(match &l.kind {
                        LayerKind::Effects(fx) => PresetLayer::Effects { visible, fx: (**fx).clone() },
                        LayerKind::Text(t) => PresetLayer::Text { visible, text: t.clone(), font: self.font_key_of(t.font_idx) },
                        LayerKind::Censor(c) => PresetLayer::Censor { visible, censor: c.clone() },
                        LayerKind::Overlay(o) => PresetLayer::Image { visible, path: o.src.path.clone()?, placement: o.placement(src), opacity: o.opacity },
                        LayerKind::Watermark(w) => {
                            let mut w = (**w).clone();
                            w.font_key = self.font_key_of(w.font_idx);
                            PresetLayer::Watermark { visible, watermark: w }
                        }
                        LayerKind::Shield(s) => PresetLayer::Shield { visible, shield: s.clone() },
                        LayerKind::Shape(s) => PresetLayer::Shape { visible, shape: (**s).clone() },
                        LayerKind::Markup(m) => PresetLayer::Markup { visible, markup: m.clone() },
                        LayerKind::Emoji(e) => PresetLayer::Emoji { visible, emoji: (**e).clone() },
                    })
                })
                .collect()
        });
        Preset { name, src_size: self.dims(), transform: inc_t.then(|| self.stack.transform.clone()), layers, gif: inc_g.then(|| self.gif_opts.clone()) }
    }

    fn apply_preset(&mut self, pr: &crate::presets::Preset, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.snapshot(&format!("preset{now}"), now);
        let rp = self.ready_preset(pr);
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let (t, layers) = batch_ui::preset_parts(&rp, self.dims(), n);
        if let Some(t) = t {
            self.stack.transform = t;
        }
        if let Some(g) = &pr.gif {
            self.gif_opts = g.clone();
        }
        for (k, vis) in layers {
            let id = self.stack.push(k);
            if let Some(i) = self.stack.index_of(id) {
                self.stack.layers[i].visible = vis;
            }
        }
        self.modified = true;
        self.refresh_plan();
        if rp.missing.is_empty() {
            self.info(format!("Applied preset “{}”", pr.name));
        } else {
            self.error(format!("Applied “{}”; missing images: {}", pr.name, rp.missing.join(", ")));
        }
    }

    // ------------------------------------------------------------ font previews

    /// Register system fonts with egui so the picker can show each name in its own face.
    fn load_preview_fonts(&mut self, ctx: &egui::Context) {
        if self.font_preview_queue.is_empty() {
            return;
        }
        let batch: Vec<String> = self.font_preview_queue.drain(..).take(6).collect();
        self.font_preview_queue.clear();
        // (name, file data, face index): bundled fonts, then the system's
        let mut faces: Vec<(String, Option<Vec<u8>>, u32)> = Vec::new();
        {
            let g = self.sys_fonts.lock().ok();
            let fams = g.as_ref().and_then(|g| g.as_ref());
            for n in &batch {
                if let Some(b) = crate::bundled::family(n) {
                    faces.push((n.clone(), crate::bundled::bytes(&b.default_face().file).map(|d| (*d).clone()), 0));
                } else if let Some(f) = fams.and_then(|v| v.iter().find(|f| &f.name == n)) {
                    let face = f.default_face();
                    let ok = std::fs::metadata(&face.path).map(|m| m.len() < 12_000_000).unwrap_or(false);
                    faces.push((n.clone(), if ok { std::fs::read(&face.path).ok() } else { None }, face.index));
                } else if fams.is_some() {
                    faces.push((n.clone(), None, 0));
                }
            }
        }
        for (name, data, index) in faces {
            self.preview_fonts.insert(name.clone(), false);
            if let Some(data) = data {
                let mut fd = egui::FontData::from_owned(data);
                fd.index = index;
                ctx.add_font(egui::epaint::text::FontInsert::new(
                    &format!("sys:{name}"),
                    fd,
                    vec![
                        egui::epaint::text::InsertFontFamily { family: egui::FontFamily::Name(format!("sys:{name}").into()), priority: egui::epaint::text::FontPriority::Highest },
                    ],
                ));
                // fallback for glyphs the face lacks (symbol fonts, arrows)
                ctx.add_font(egui::epaint::text::FontInsert::new(
                    &format!("sys-fb:{name}"),
                    egui::FontData::from_static(theme::NOTO_REGULAR),
                    vec![egui::epaint::text::InsertFontFamily { family: egui::FontFamily::Name(format!("sys:{name}").into()), priority: egui::epaint::text::FontPriority::Lowest }],
                ));
                self.preview_fonts.insert(name, true);
            }
        }
        ctx.request_repaint();
    }

    /// FontId showing `family` in its own face (once loaded), else the UI font.
    fn preview_font(&mut self, family: &str, size: f32) -> egui::FontId {
        match self.preview_fonts.get(family) {
            Some(true) => egui::FontId::new(size, egui::FontFamily::Name(format!("sys:{family}").into())),
            Some(false) => egui::FontId::proportional(size),
            None => {
                if !self.font_preview_queue.iter().any(|f| f == family) {
                    self.font_preview_queue.push(family.to_string());
                }
                egui::FontId::proportional(size)
            }
        }
    }

    // ------------------------------------------------------------ canvas

    /// Draw one viewport (view state `vi`): background, image and labels.
    /// Handles wheel zoom and panning for that viewport (or both, when linked).
    #[allow(clippy::too_many_arguments)]
    fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, vi: usize, rect: Rect, tex: Option<Tex>, img_size: Vec2, label: Option<(String, Color32)>, badge: Option<String>, busy: bool) -> (egui::Response, Mapper) {
        let pal = *p();
        let fit = ((rect.width() - 24.0) / img_size.x).min((rect.height() - 24.0) / img_size.y).clamp(0.02, 64.0);
        self.views[vi].fit = fit;
        let scale = self.views[vi].zoom.unwrap_or(fit);
        let center = rect.center() + self.views[vi].pan;
        let img_rect = Rect::from_center_size(center, img_size * scale);
        // how big it's shown, for the size textures are made at
        let shown = (img_size.x.max(img_size.y) * scale * ctx.pixels_per_point()).ceil() as u32;
        TEX_NEED.fetch_max(shown, Ordering::Relaxed);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, pal.canvas);
        let vis = img_rect.intersect(rect);
        if vis.is_positive() {
            if self.backdrop != Backdrop::Canvas {
                painter.rect_filled(img_rect.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(60));
            }
            self.paint_backdrop(&painter, ctx, vis, img_rect);
            if let Some(t) = &tex {
                let ts = t.size_vec2();
                // a texture made smaller than the picture (big sources) is
                // stretched over it; otherwise it's drawn at its own size
                let same_shape = ((ts.x / ts.y) - (img_size.x / img_size.y)).abs() < 0.02 * (img_size.x / img_size.y) && ts.x < img_size.x - 0.5;
                let tr = if same_shape { img_rect } else { Rect::from_min_size(img_rect.min, ts * scale) };
                painter.image(t.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                // onion skin: the frames around it, faint and tinted
                for (o, tint) in std::mem::take(&mut self.onion_paint) {
                    let os = o.size_vec2();
                    let or = if same_shape { img_rect } else { Rect::from_min_size(img_rect.min, os * scale) };
                    painter.image(o.id(), or, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
                }
            }
        }
        self.onion_paint.clear();
        let resp = ui.interact(rect, egui::Id::unique(("gifkit-viewport", vi)), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            self.active_view = vi;
        }
        if resp.hovered() {
            let (scroll, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta(), i.pointer.hover_pos()));
            let factor = zoom * (scroll.y * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-4 {
                if let Some(pp) = pointer {
                    self.zoom_at(vi, pp, rect.center(), factor);
                }
            }
            if scroll.x.abs() > 0.0 {
                self.pan_by(vi, vec2(scroll.x, 0.0));
            }
        }
        let left_pans = !self.tool.uses_canvas_drag() || ui.input(|i| i.modifiers.alt);
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) || (left_pans && resp.dragged_by(PointerButton::Primary)) {
            self.pan_by(vi, resp.drag_delta());
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if resp.hovered() && left_pans {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.double_clicked_by(PointerButton::Middle) || (left_pans && resp.double_clicked()) {
            self.zoom_fit_v(vi);
        }

        let mut y = rect.top() + 46.0;
        for (text, col, warn) in [label.map(|(t, c)| (t, c, false)), badge.map(|t| (t, Color32::WHITE, true))].into_iter().flatten() {
            let galley = painter.layout_no_wrap(text, theme::medium(12.5), col);
            let pos = pos2(rect.left() + 10.0, y);
            let bg = Rect::from_min_size(pos, galley.size() + vec2(16.0, 8.0));
            painter.rect_filled(bg, 3.0, if warn { Color32::from_rgba_unmultiplied(150, 70, 0, 225) } else { Color32::from_black_alpha(165) });
            painter.galley(pos + vec2(8.0, 4.0), galley, col);
            if warn && busy {
                let c = bg.right_center() + vec2(14.0, 0.0);
                let t = ui.input(|i| i.time) as f32;
                painter.circle_filled(c, 9.0, Color32::from_black_alpha(165));
                painter.circle_stroke(c, 6.0, Stroke::new(2.0, Color32::from_white_alpha(60)));
                let a = t * 6.0;
                painter.line_segment([c, c + vec2(a.cos(), a.sin()) * 6.0], Stroke::new(2.0, pal.highlight));
                ui.ctx().request_repaint();
            }
            y = bg.bottom() + 6.0;
        }
        if self.compare && self.active_view == vi && !self.link_views {
            painter.rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(1.0, pal.highlight.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        }
        if tex.is_some() {
            self.view_zoom_overlay(ui, vi, rect);
        }
        (resp, Mapper { rect: img_rect, scale })
    }

    /// Width the zoom controls take at the top right of a view (0 when the
    /// view is too small for them).
    fn zoom_bar_w(rect: Rect) -> f32 {
        let w = 4.0 * 26.0 + 58.0 + 14.0;
        if rect.width() < w + 200.0 || rect.height() < 120.0 { 0.0 } else { w }
    }

    /// Zoom controls at the top right of a view, in line with its source
    /// picker, for that view.
    fn view_zoom_overlay(&mut self, ui: &mut egui::Ui, vi: usize, rect: Rect) {
        let (b, pct_w) = (26.0, 58.0);
        let w = Self::zoom_bar_w(rect);
        if w == 0.0 {
            return;
        }
        let bar = Rect::from_min_size(pos2(rect.right() - w - 10.0, rect.top() + 7.0), vec2(w, b + 6.0));
        let hovered = ui.rect_contains_pointer(bar);
        ui.painter().rect_filled(bar, 4.0, Color32::from_black_alpha(if hovered { 200 } else { 150 }));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(5.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 1.0;
        let white = Color32::from_gray(235);
        let btn = |ui: &mut egui::Ui, icon: &str, tip: &str, on: bool| -> bool {
            let (r, resp) = ui.allocate_exact_size(vec2(b, b), Sense::click());
            if on || resp.hovered() {
                ui.painter().rect_filled(r, 3.0, Color32::from_white_alpha(if on { 45 } else { 30 }));
            }
            crate::icons::paint(ui.painter(), icon, r.center(), 16.0, white);
            resp.on_hover_text(tip).clicked()
        };
        let scale = self.scale_of(vi);
        if btn(&mut child, "zoom-out", "Zoom out", false) {
            self.zoom_to_v(vi, scale / 1.25);
        }
        let mut pct = scale * 100.0;
        let mut pc = child.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(child.cursor().min, vec2(pct_w, b))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        pc.spacing_mut().interact_size = vec2(pct_w, b);
        let wv = &mut pc.visuals_mut().widgets;
        for st in [&mut wv.inactive, &mut wv.hovered, &mut wv.active] {
            st.bg_stroke = Stroke::NONE;
            st.weak_bg_fill = Color32::TRANSPARENT;
            st.bg_fill = Color32::TRANSPARENT;
            st.fg_stroke.color = white;
        }
        if pc.add(egui::DragValue::new(&mut pct).range(2.0..=6400.0).speed(1.0).suffix("%").max_decimals(0)).on_hover_text("Zoom. Drag or click to type.").changed() {
            self.zoom_to_v(vi, pct / 100.0);
        }
        child.add_space(pct_w);
        if btn(&mut child, "zoom-in", "Zoom in", false) {
            self.zoom_to_v(vi, scale * 1.25);
        }
        if btn(&mut child, "zoom-fit-best", "Fit to view", self.views[vi].zoom.is_none()) {
            self.zoom_fit_v(vi);
        }
        if btn(&mut child, "zoom-original", "Actual size", (scale - 1.0).abs() < 1e-3) {
            self.zoom_to_v(vi, 1.0);
        }
    }

    /// Index of the original-file frame to show: the same source frame when the
    /// frame structure is unchanged, otherwise by time.
    fn original_index(&self) -> Option<usize> {
        let o = self.original.as_ref()?;
        if self.playing {
            // its own time, so it plays as the original did whatever the edit does
            return Some(stack::frame_at_time(o, self.orig_clock as f32));
        }
        self.paused_original()
    }

    /// The original frame that matches the edit's frame on screen.
    fn paused_original(&self) -> Option<usize> {
        let o = self.original.as_ref()?;
        let b = self.base.as_ref()?;
        if o.len() == b.len() {
            self.plan.get(self.cur).map(|f| f.src.min(o.len() - 1))
        } else {
            Some(stack::frame_at_time(o, self.play_time()))
        }
    }

    /// Draw a viewport for a given source. Editing tools work wherever the
    /// coordinates are known: live edit & compressed (output space) and the
    /// original file (source space, when its size matches the frames).
    fn source_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect, src: ViewSource, vi: usize, show_label: bool) {
        self.source_view_inner(ui, ctx, rect, src, vi, show_label);
        self.view_picker(ui, rect, src, vi);
    }

    /// Size note shown next to a view's source name.
    fn view_info(&self, src: ViewSource) -> String {
        match src {
            ViewSource::Original => self.file_size.map(fmt_size).unwrap_or_default(),
            ViewSource::Edit => String::new(),
            ViewSource::Compressed => {
                let f = self.export_fmt.label();
                if self.preview_format().is_none() {
                    return format!("{f} · no preview");
                }
                match &self.cmp {
                    Some(c) if c.key == self.cmp_key() && c.sheet => {
                        let (w, h) = c.full;
                        format!("{f} · {w}×{h} · {}", c.size_text())
                    }
                    Some(c) if c.key == self.cmp_key() => {
                        let n = c.size;
                        let sampled = if c.sample.is_some() { " · sampled" } else { "" };
                        match self.file_size {
                            Some(o) => format!("{f} · {} ({:+.0}%){sampled}", c.size_text(), (n as f64 / o as f64 - 1.0) * 100.0),
                            None => format!("{f} · {}{sampled}", c.size_text()),
                        }
                    }
                    _ => format!("{f} · …"),
                }
            }
        }
    }

    /// Source chooser floating in a viewport's top-left corner.
    fn view_picker(&mut self, ui: &mut egui::Ui, rect: Rect, src: ViewSource, vi: usize) {
        let pal = *p();
        let info = self.view_info(src);
        let zoom_w = Self::zoom_bar_w(rect);
        let area = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2((rect.width() - 20.0 - if zoom_w > 0.0 { zoom_w + 10.0 } else { 0.0 }).max(40.0), 28.0));
        // drop detail from the label until it fits the view
        let font = theme::medium(12.5);
        let fits = |t: &String| ui.painter().layout_no_wrap(t.clone(), font.clone(), Color32::WHITE).size().x + 24.0 <= area.width();
        let short = info.split(" (").next().unwrap_or("").to_string();
        let mut text = src.label().to_string();
        for cand in [info.clone(), short] {
            let t = format!("{}  ·  {}", src.label(), cand);
            if !cand.is_empty() && fits(&t) {
                text = t;
                break;
            }
        }
        let mut pick = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            let r = ui.add(
                egui::Button::new(RichText::new(text).font(theme::medium(12.5)).color(Color32::WHITE))
                    .right_text(crate::icons::image(ui.ctx(), "go-down", 16.0, Color32::WHITE))
                    .fill(Color32::from_black_alpha(170))
                    .stroke(Stroke::new(1.0, Color32::from_white_alpha(30)))
                    .corner_radius(4.0)
                    .min_size(vec2(0.0, 26.0)),
            );
            let r = r.on_hover_text(src.tip());
            egui::Popup::menu(&r).show(|ui| {
                ui.set_min_width(170.0);
                for s in ViewSource::ALL {
                    let resp = ui.add(egui::Button::selectable(s == src, s.label())).on_hover_text(s.tip());
                    if resp.clicked() {
                        pick = Some(s);
                    }
                }
            });
        });
        let _ = pal;
        if let Some(s) = pick {
            match vi {
                1 => self.view_left = s,
                2 => self.view_right = s,
                _ => self.view_single = s,
            }
        }
    }

    fn source_view_inner(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, rect: Rect, src: ViewSource, vi: usize, show_label: bool) {
        let t = self.play_time();
        let crop_mode = self.tool == Tool::Crop;
        // there is no compressed result of the uncropped frame: crop on the live view
        let src = if crop_mode && src == ViewSource::Compressed { ViewSource::Edit } else { src };
        match src {
            ViewSource::Edit => {
                let (tex, size) = if crop_mode {
                    if self.composite_texture(ctx, self.cur).is_some() {
                        let s = self.true_out_size();
                        self.out_size = (s.x, s.y);
                    }
                    let s = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
                    let (w, h) = self.dims();
                    let live = if self.crop_live { self.crop_texture(ctx, self.cur) } else { None };
                    (live.or_else(|| self.base_texture(ctx, s)).map(Tex::from), vec2(w as f32, h as f32))
                } else {
                    // laid out at full size whatever size the preview was made at
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = self.true_out_size();
                    self.out_size = (size.x, size.y);
                    self.onion_paint = self.onion_frames(ctx);
                    (tex, size)
                };
                let label = crop_mode.then(|| (if self.crop_live { "Full frame · with edits".to_string() } else { "Full frame · original".to_string() }, Color32::WHITE));
                let _ = show_label;
                let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, label, None, false);
                let space = if crop_mode { Space::Source } else { Space::Output };
                self.canvas_interaction(ui, &resp, m, (size.x, size.y), space, vi);
            }
            ViewSource::Original => {
                let Some(i) = self.original_index() else { return };
                let (size, same) = {
                    let o = self.original.as_ref().unwrap();
                    (vec2(o.width() as f32, o.height() as f32), (o.width(), o.height()) == self.dims())
                };
                let tex = self.orig_texture(ctx, i).map(Tex::from);
                let on = self.original.as_ref().map(|o| o.len()).unwrap_or(0);
                self.onion_paint = self.onion_of(i, on, |me, j| me.orig_texture(ctx, j).map(Tex::from));
                let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, None, None, false);
                // the original doesn't show what layers do, so only the crop is edited here
                if same && self.tool == Tool::Crop {
                    self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Source, vi);
                }
            }
            ViewSource::Compressed => {
                let encoding = self.cmp_job.is_some();
                let fresh = self.cmp.as_ref().map(|c| c.key == self.cmp_key()).unwrap_or(false);
                // a sampled estimate only has a few stretches of frames: elsewhere
                // the edit shows, rather than the nearest sampled frame jumping about
                let outside = fresh && self.cmp.as_ref().and_then(|c| c.sample.as_ref()).is_some_and(|s| !s.contains(&self.cur));
                let t_now = self.play_time();
                let not_saved = fresh && self.cmp.as_ref().is_some_and(|c| !c.covers(t_now));
                if outside || not_saved {
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = self.true_out_size();
                    let note = if not_saved { "Not exported · showing the edit" } else { "Not in the size sample · showing the edit" };
                    self.onion_paint = self.onion_frames(ctx);
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, Some((note.into(), Color32::from_gray(225))), None, false);
                    if !crop_mode {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output, vi);
                    }
                } else if fresh {
                    let (i, size, n) = {
                        let c = self.cmp.as_ref().unwrap();
                        (c.frame_for(self.cur, t, &self.plan), vec2(c.full.0 as f32, c.full.1 as f32), c.size)
                    };
                    let _ = n;
                    let sheet = self.cmp.as_ref().map(|c| c.sheet).unwrap_or(false);
                    let tex = self.cmp_texture(ctx, i).map(Tex::from);
                    // its own frames around it (identical ones are merged in the file)
                    let cn = self.cmp.as_ref().map(|c| if c.sheet { 0 } else { c.anim.len() }).unwrap_or(0);
                    self.onion_paint = self.onion_of(i, cn, |me, j| me.cmp_texture(ctx, j).map(Tex::from));
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, None, None, false);
                    // a sprite sheet doesn't share the frame's coordinates: no editing handles
                    if !crop_mode && !sheet {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output, vi);
                    }
                } else {
                    // compression pending / running: show the live preview with a warning
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = self.true_out_size();
                    let badge = if self.preview_format().is_none() {
                        if self.export_fmt.needs_ffmpeg() && !self.ffmpeg { "⚠ Needs ffmpeg to preview" } else { "Uncompressed output" }
                    } else if encoding {
                        "⚠ Compressing"
                    } else {
                        "⚠ Not compressed yet"
                    };
                    self.onion_paint = self.onion_frames(ctx);
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, None, Some(badge.into()), encoding);
                    if !crop_mode {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output, vi);
                    }
                }
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        if !self.compare {
            let s = self.view_single;
            self.source_view(ui, ctx, full, s, 0, false);
            return;
        }
        let gap = 2.0;
        // the divider sits where it was dragged, each side keeping some room
        let min_side = (full.width() * 0.5 - 4.0).min(180.0);
        let mid = (full.min.x + full.width() * self.split_frac).clamp(full.min.x + min_side, full.max.x - min_side);
        let left = Rect::from_min_max(full.min, pos2(mid - gap / 2.0, full.max.y));
        let right = Rect::from_min_max(pos2(mid + gap / 2.0, full.min.y), full.max);
        let (l, r) = (self.view_left, self.view_right);
        self.source_view(ui, ctx, left, l, 1, true);
        self.source_view(ui, ctx, right, r, 2, true);
        // drawn and grabbed after the views, so it's on top of them
        let grab = Rect::from_center_size(pos2(mid, full.center().y), vec2(10.0, full.height()));
        let d = ui.interact(grab, egui::Id::unique("split-divider"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        let hot = d.hovered() || d.dragged();
        let line = Rect::from_min_max(pos2(left.max.x, full.min.y), pos2(right.min.x, full.max.y));
        ui.painter().rect_filled(if hot { line.expand2(vec2(1.0, 0.0)) } else { line }, 0.0, if hot { pal.highlight } else { pal.border });
        if d.dragged() {
            if let Some(pp) = d.interact_pointer_pos() {
                self.split_frac = ((pp.x - full.min.x) / full.width().max(1.0)).clamp(0.05, 0.95);
            }
        }
        if d.drag_stopped() {
            theme::set_setting("split", &format!("{:.4}", self.split_frac));
        }
        if d.double_clicked() {
            self.split_frac = 0.5;
            theme::set_setting("split", "0.5");
        }
        d.on_hover_text("Drag to resize the views. Double-click to make them even.");
    }

    /// Canvas editing. `space` says which coordinates the viewport shows:
    /// source frames (original / crop view) or the edited output.
    fn canvas_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response, m: Mapper, bounds: (f32, f32), space: Space, vi: usize) {
        if space == Space::Output {
            self.drop_views.push((resp.rect, m));
        }
        let painter = ui.painter_at(resp.rect);
        // in split view, gizmos only appear in the viewport you're working in
        if resp.hovered() || resp.dragged() {
            self.active_view = vi;
        }
        let show = !self.compare || self.active_view == vi;
        let pal = *p();
        let alt = ui.input(|i| i.modifiers.alt);
        let started = resp.drag_started_by(PointerButton::Primary) && !alt;
        let dragged = resp.dragged_by(PointerButton::Primary) && !alt;
        let stopped = resp.drag_stopped();
        if self.tool == Tool::Crop {
            if space != Space::Source {
                return;
            }
            let ratio = self.base.as_ref().and_then(|a| self.crop_lock.ratio(a.width(), a.height()));
            let mut r = self.stack.transform.crop.map(|v| v as f32);
            rect_tool(&mut self.drag, started, dragged, stopped, resp, &m, &mut r, bounds, ratio, true);
            let nr = r.map(|v| v.round().max(0.0) as u32);
            if nr != self.stack.transform.crop {
                self.stack.transform.crop = nr;
                self.stack.transform.crop_on = true;
            }
            let sr = m.rect_to_screen(r[0], r[1], r[2], r[3]);
            if !show {
                return;
            }
            if self.stack.transform.crop_on {
                let full = m.rect;
                let dim = Color32::from_black_alpha(150);
                painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, sr.min.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, sr.max.y), full.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, sr.min.y), pos2(sr.min.x, sr.max.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(sr.max.x, sr.min.y), pos2(full.max.x, sr.max.y)), 0.0, dim);
            }
            draw_sel(&painter, sr, pal.highlight);
            return;
        }
        let Some(kind) = self.tool.layer_kind() else { return };
        if kind == Kind::Effects || kind == Kind::Animation {
            return;
        }
        if kind == Kind::Markup {
            self.markup_interaction(ui, resp, m, space, show, (started, dragged, stopped));
            return;
        }
        let (bw, bh) = self.dims();
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let maps = match space {
            Space::Output => Some(self.stack.layer_maps(bw, bh)),
            Space::Source => None,
        };
        let map_of = |li: usize| maps.as_ref().map(|m| m[li].m).unwrap_or(Affine::IDENTITY);

        // candidate layers of this kind on this frame: (index, id, source box, displayed box)
        let mut cands: Vec<(usize, u64, Obb, Obb)> = Vec::new();
        for (li, l) in self.stack.layers.iter().enumerate() {
            if l.kind() != kind || !l.visible {
                continue;
            }
            let applies = match &l.kind {
                LayerKind::Text(t) => t.applies_to(src_idx),
                LayerKind::Censor(c) => c.w == 0 || c.applies_to(src_idx),
                LayerKind::Shape(sh) => sh.w == 0 || sh.applies_to(src_idx),
                LayerKind::Emoji(e) => e.w == 0 || e.applies_to(src_idx),
                _ => true,
            };
            // hidden by its clips on the timeline, or locked: not there to pick
            if !applies || !l.timing.shows(self.cur, self.plan.len()) || l.locked {
                continue;
            }
            if let Some(b) = l.source_box(src_idx, &self.fonts) {
                cands.push((li, l.id, b, map_box(&map_of(li), b, l.readable())));
            }
        }
        let sel_li = self.sel_index(kind);
        let sel = sel_li.and_then(|li| cands.iter().find(|c| c.0 == li).copied());
        let to_screen = |p: (f32, f32)| m.to_screen(pos2(p.0, p.1));
        let rot_handle = |o: &Obb| to_screen(o.from_local(0.0, -o.h / 2.0 - 30.0 / m.scale));
        let ptr_img = resp.interact_pointer_pos().map(|p| m.to_img(p));
        let shift = ui.input(|i| i.modifiers.shift);

        // ---- Track: a click picks the spot the layer follows
        if self.path_track && space == Space::Output {
            if let Some((li, id, b, _)) = sel {
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                if resp.clicked() {
                    if let (Some(pi), Some(base)) = (ptr_img, self.base.clone()) {
                        let (px, py) = map_of(li).inverse().apply(pi.x, pi.y);
                        let n = self.plan.len();
                        let l = &self.stack.layers[li];
                        let (a, z) = l.timing.extent(n);
                        let mo = l.timing.motion(self.cur);
                        // the layer keeps where it sits from the spot
                        let off = (b.cx + mo.dx - px, b.cy + mo.dy - py);
                        let srcs: Vec<usize> = self.plan[a..=z].iter().map(|f| f.src).collect();
                        let start = self.cur.clamp(a, z) - a;
                        let centre = (b.cx, b.cy);
                        self.path_track = false;
                        let ctx = ui.ctx().clone();
                        self.spawn(&ctx, "Tracking", srcs.len(), move |p| {
                            use rayon::prelude::*;
                            let grays: Vec<crate::track::Gray> = srcs.par_iter().map(|&s| crate::track::Gray::new(&base.frames[s].image.get())).collect();
                            let refs: Vec<&crate::track::Gray> = grays.iter().collect();
                            let pts = crate::track::track(&refs, start, (px, py), p);
                            if std::env::var_os("GIFKIT_TRACK_DEBUG").is_some() {
                                eprintln!("track from {:?} on frame {start}: {:?}", (px, py), pts.iter().step_by(5).map(|q| (q.0 as i32, q.1 as i32)).collect::<Vec<_>>());
                            }
                            let pos = pts.into_iter().map(|q| (q.0 + off.0, q.1 + off.1)).collect();
                            JobOutput::Track(id, a, pos, centre)
                        });
                    }
                }
                return;
            }
        }

        // ---- Draw Path: the drag is the path, not a move
        if self.path_draw && space == Space::Output {
            if let Some((li, _, b, _)) = sel {
                let inv = map_of(li).inverse();
                let to_src = |p: Pos2| inv.apply(p.x, p.y);
                if started {
                    let origin = ui.input(|i| i.pointer.press_origin()).map(|p| m.to_img(p));
                    self.path_pts = origin.map(to_src).into_iter().collect();
                }
                if dragged {
                    if let Some(pi) = ptr_img {
                        let q = to_src(pi);
                        let far = self.path_pts.last().is_none_or(|l| ((l.0 - q.0).powi(2) + (l.1 - q.1).powi(2)).sqrt() * m.scale > 2.0);
                        if far {
                            self.path_pts.push(q);
                        }
                    }
                }
                if show && self.path_pts.len() > 1 {
                    let mp = map_of(li);
                    let pts: Vec<Pos2> = self.path_pts.iter().map(|&(x, y)| to_screen(mp.apply(x, y))).collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(2.0, pal.highlight)));
                }
                if stopped && self.path_pts.len() > 1 {
                    let pts = std::mem::take(&mut self.path_pts);
                    let now = ui.input(|i| i.time);
                    self.snapshot(&format!("path{now}"), now);
                    let n = self.plan.len();
                    let k = self.stack.layers[li].timing.follow_path(&pts, (b.cx, b.cy), n);
                    self.modified = true;
                    // done: back to moving things, with the path shown
                    self.path_draw = false;
                    self.info(format!("The layer follows the path: {k} keyframes"));
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                return;
            }
        }

        // ---- start of a drag: decide what is being edited
        if started {
            // where the button went down, not where the pointer is once the
            // drag counts as started: a handle grabbed is the one pressed on
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let (Some(sp), Some(pi)) = (origin, origin.map(|p| m.to_img(p))) {
                let mut new_drag = None;
                if let Some((li, id, b, o)) = sel {
                    if rot_handle(&o).distance(sp) <= HANDLE_HIT {
                        let a0 = (pi.y - o.cy).atan2(pi.x - o.cx).to_degrees();
                        new_drag = Some(BoxDrag::Rotate { id, li, a0, out0: o, src: b });
                    } else if let Some((fixed, axis)) = o
                        .corners()
                        .iter()
                        .position(|c| to_screen(*c).distance(sp) <= HANDLE_HIT)
                        .map(|k| (o.to_local(o.corners()[(k + 2) % 4].0, o.corners()[(k + 2) % 4].1), 0u8))
                        .or_else(|| edge_mids(&o).iter().position(|c| to_screen(*c).distance(sp) <= HANDLE_HIT).map(|k| {
                            // the opposite edge stays put
                            let f = [(0.0, o.h / 2.0), (-o.w / 2.0, 0.0), (0.0, -o.h / 2.0), (o.w / 2.0, 0.0)][k];
                            (f, if k % 2 == 0 { 2 } else { 1 })
                        }))
                    {
                        let (size0, stretch0) = match &self.stack.layers[li].kind {
                            LayerKind::Text(t) => (t.size, t.stretch),
                            _ => (0.0, 1.0),
                        };
                        new_drag = Some(BoxDrag::Resize { id, li, fixed, out0: o, src: b, size0, stretch0, axis });
                    } else if o.contains(pi.x, pi.y) {
                        new_drag = Some(BoxDrag::Move { id, li, start: pi, src: b });
                    }
                }
                if new_drag.is_none() {
                    if let Some(&(li, id, b, _)) = cands.iter().rev().find(|c| c.3.contains(pi.x, pi.y)) {
                        self.select(kind, id);
                        new_drag = Some(BoxDrag::Move { id, li, start: pi, src: b });
                    } else if kind == Kind::Censor {
                        let now = ui.input(|i| i.time);
                        self.snapshot(&format!("addcensor{now}"), now);
                        let id = self.add_layer(LayerKind::Censor(CensorSettings { x: 0, y: 0, w: 0, h: 0, angle: 0.0, from: 1, to: 0, visible: true, ..self.censor_tpl.clone() }));
                        let li = self.stack.layers.len() - 1;
                        new_drag = Some(BoxDrag::New { id, li, anchor: pi });
                    } else if kind == Kind::Shape {
                        let now = ui.input(|i| i.time);
                        self.snapshot(&format!("addshape{now}"), now);
                        let sh = ShapeSettings { w: 0, h: 0, from: 1, to: 0, angle: 0.0, ..self.shape_tpl.clone() };
                        let id = self.add_layer(LayerKind::Shape(Box::new(sh)));
                        let li = self.stack.layers.len() - 1;
                        new_drag = Some(BoxDrag::New { id, li, anchor: pi });
                    } else if kind == Kind::Emoji {
                        let now = ui.input(|i| i.time);
                        self.snapshot(&format!("addemoji{now}"), now);
                        let e = EmojiSettings { emoji: self.emoji_pick.clone(), opacity: self.emoji_tpl.opacity, shadow: self.emoji_tpl.shadow, shadow_style: self.emoji_tpl.shadow_style, ..Default::default() };
                        self.note_recent_emoji();
                        let id = self.add_layer(LayerKind::Emoji(Box::new(e)));
                        let li = self.stack.layers.len() - 1;
                        new_drag = Some(BoxDrag::New { id, li, anchor: pi });
                    }
                }
                self.box_drag = new_drag;
            }
        }

        // ---- drag update
        if dragged {
            if let (Some(d), Some(pi)) = (self.box_drag, ptr_img) {
                let li = d.li();
                if li < self.stack.layers.len() && self.stack.layers[li].id == d.id() {
                    let mp = maps.as_ref().map(|_| self.stack.layer_maps(bw, bh)[li].m).unwrap_or(Affine::IDENTITY);
                    let readable = self.stack.layers[li].readable();
                    match d {
                        BoxDrag::Move { start, src, .. } => {
                            let (mut ox, mut oy) = (pi.x - start.x, pi.y - start.y);
                            // Shift: move along the nearest horizontal, vertical or 45° line (as seen on screen)
                            if ui.input(|i| i.modifiers.shift) && (ox != 0.0 || oy != 0.0) {
                                let step = std::f32::consts::FRAC_PI_4;
                                let ang = (oy.atan2(ox) / step).round() * step;
                                let (dxn, dyn_) = (ang.cos(), ang.sin());
                                let len = ox * dxn + oy * dyn_;
                                (ox, oy) = (dxn * len, dyn_ * len);
                            }
                            let (dx, dy) = mp.inverse().apply_vec(ox, oy);
                            let nb = Obb { cx: src.cx + dx, cy: src.cy + dy, ..src };
                            self.write_box(li, nb, None, src_idx);
                        }
                        BoxDrag::Resize { fixed, out0, src, size0, stretch0, axis, .. } => {
                            // Shift flips the proportion lock (images and emoji start locked),
                            // Ctrl scales along one axis only, edge handles always do
                            let ctrl = ui.input(|i| i.modifiers.command);
                            let emoji = matches!(&self.stack.layers[li].kind, LayerKind::Shape(sh) if sh.kind == ShapeKind::Emoji) || kind == Kind::Emoji;
                            let lock = (kind == Kind::Overlay || emoji) != shift;
                            let lp = out0.to_local(pi.x, pi.y);
                            let (mut w, mut h) = ((lp.0 - fixed.0).abs().max(1.0), (lp.1 - fixed.1).abs().max(1.0));
                            let mut axis = axis;
                            if axis == 0 && ctrl {
                                axis = if (w / out0.w - 1.0).abs() >= (h / out0.h.max(1e-3) - 1.0).abs() { 1 } else { 2 };
                            }
                            match axis {
                                1 => h = out0.h,
                                2 => w = out0.w,
                                _ if lock => {
                                    let r = out0.w / out0.h.max(1e-3);
                                    w = w.max(h * r);
                                    h = w / r;
                                }
                                _ => {}
                            }
                            let sx = if lp.0 >= fixed.0 { 1.0 } else { -1.0 };
                            let sy = if lp.1 >= fixed.1 { 1.0 } else { -1.0 };
                            let lcx = if axis == 2 { 0.0 } else { fixed.0 + sx * w / 2.0 };
                            let lcy = if axis == 1 { 0.0 } else { fixed.1 + sy * h / 2.0 };
                            let c = out0.from_local(lcx, lcy);
                            let out = Obb { cx: c.0, cy: c.1, w, h, angle: out0.angle };
                            let nb = unmap_box(&mp, out, src.angle);
                            if size0 > 0.0 {
                                // text: height sets the font size, width the stretch
                                let (ky, kx) = (nb.h / src.h.max(1e-3), nb.w / src.w.max(1e-3));
                                if let LayerKind::Text(t) = &mut self.stack.layers[li].kind {
                                    t.size = (size0 * ky).clamp(2.0, 4000.0);
                                    t.stretch = (stretch0 * kx / ky.max(1e-3)).clamp(0.05, 20.0);
                                }
                            }
                            self.write_box(li, nb, None, src_idx);
                        }
                        BoxDrag::Rotate { a0, out0, src, .. } => {
                            let a = (pi.y - out0.cy).atan2(pi.x - out0.cx).to_degrees();
                            let mut phi = out0.angle + (a - a0);
                            if shift {
                                phi = (phi / 15.0).round() * 15.0;
                            }
                            let th = source_angle(&mp, phi, readable);
                            self.write_box(li, Obb { angle: th, ..src }, None, src_idx);
                        }
                        BoxDrag::New { anchor, .. } if matches!(&self.stack.layers[li].kind, LayerKind::Shape(sh) if sh.kind.is_linear()) => {
                            // lines and arrows run from where the drag started to the pointer
                            let (mut dx, mut dy) = (pi.x - anchor.x, pi.y - anchor.y);
                            let len = (dx * dx + dy * dy).sqrt().max(1.0);
                            let mut ang = dy.atan2(dx).to_degrees();
                            if shift {
                                ang = (ang / 15.0).round() * 15.0;
                                (dx, dy) = (len * ang.to_radians().cos(), len * ang.to_radians().sin());
                            }
                            let thick = match &self.stack.layers[li].kind {
                                LayerKind::Shape(sh) if sh.kind == ShapeKind::Arrow => (len * 0.22).clamp(12.0, 120.0),
                                LayerKind::Shape(sh) => (sh.stroke_w * mp.det().abs().sqrt() * 3.0).max(8.0),
                                _ => 8.0,
                            };
                            let out = Obb { cx: anchor.x + dx / 2.0, cy: anchor.y + dy / 2.0, w: len, h: thick, angle: ang };
                            let th = source_angle(&mp, ang, false);
                            self.write_box(li, unmap_box(&mp, out, th), None, src_idx);
                        }
                        BoxDrag::New { anchor, .. } if kind == Kind::Emoji => {
                            // emoji stay square: the larger side of the drag wins
                            let side = (pi.x - anchor.x).abs().max((pi.y - anchor.y).abs()).max(1.0);
                            let (sx, sy) = (if pi.x >= anchor.x { 1.0 } else { -1.0 }, if pi.y >= anchor.y { 1.0 } else { -1.0 });
                            let out = Obb { cx: anchor.x + sx * side / 2.0, cy: anchor.y + sy * side / 2.0, w: side, h: side, angle: 0.0 };
                            let th = source_angle(&mp, 0.0, true);
                            self.write_box(li, unmap_box(&mp, out, th), None, src_idx);
                        }
                        BoxDrag::New { anchor, .. } => {
                            let (x0, y0) = (anchor.x.min(pi.x), anchor.y.min(pi.y));
                            let (x1, y1) = (anchor.x.max(pi.x), anchor.y.max(pi.y));
                            let out = Obb::from_rect(x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0), 0.0);
                            let th = source_angle(&mp, 0.0, false);
                            self.write_box(li, unmap_box(&mp, out, th), None, src_idx);
                        }
                    }
                }
            }
        }
        if stopped {
            // a click without a drag drops a shape of the default size there
            if let Some(BoxDrag::New { id, li, anchor }) = self.box_drag {
                if li < self.stack.layers.len() && self.stack.layers[li].id == id {
                    let tiny = match &self.stack.layers[li].kind {
                        LayerKind::Emoji(e) => e.w < 4 || e.h < 4,
                        _ => false,
                    };
                    if tiny {
                        let mp = maps.as_ref().map(|_| self.stack.layer_maps(bw, bh)[li].m).unwrap_or(Affine::IDENTITY);
                        let (sx, sy) = mp.inverse().apply(anchor.x, anchor.y);
                        let d = self.default_emoji(bw, bh, Some((sx, sy)));
                        if let LayerKind::Emoji(e) = &mut self.stack.layers[li].kind {
                            **e = d;
                        }
                    }
                    if let LayerKind::Shape(sh) = &self.stack.layers[li].kind {
                        if sh.w < 4 || sh.h < 4 {
                            let mut d = self.default_shape(bw, bh);
                            let mp = maps.as_ref().map(|_| self.stack.layer_maps(bw, bh)[li].m).unwrap_or(Affine::IDENTITY);
                            let (sx, sy) = mp.inverse().apply(anchor.x, anchor.y);
                            d.x = (sx - d.w as f32 / 2.0).round() as i32;
                            d.y = (sy - d.h as f32 / 2.0).round() as i32;
                            if let LayerKind::Shape(sh) = &mut self.stack.layers[li].kind {
                                **sh = d;
                            }
                        }
                    }
                }
            }
            self.box_drag = None;
        }
        // plain click selects the topmost layer under the pointer
        if resp.clicked() {
            if let Some(pi) = ptr_img {
                if let Some(&(_, id, _, _)) = cands.iter().rev().find(|c| c.3.contains(pi.x, pi.y)) {
                    self.select(kind, id);
                }
            }
        }

        // ---- draw outlines and handles (recomputed after edits)
        let sel_id = self.sel_index(kind).map(|i| self.stack.layers[i].id);
        let maps2 = match space {
            Space::Output => Some(self.stack.layer_maps(bw, bh)),
            Space::Source => None,
        };
        if !show {
            return;
        }
        for (li, l) in self.stack.layers.iter().enumerate() {
            if l.kind() != kind || !l.visible {
                continue;
            }
            let Some(b) = l.source_box(src_idx, &self.fonts) else { continue };
            if !l.timing.shows(self.cur, self.plan.len()) {
                continue;
            }
            let mp = maps2.as_ref().map(|m| m[li].m).unwrap_or(Affine::IDENTITY);
            let o = map_box(&mp, b, l.readable());
            // keyframed motion: a dashed outline where it is on this frame,
            // the handles stay on where it was placed
            let mo = l.timing.motion(self.cur);
            // the selected layer's path over its clip: a dot a frame
            if Some(l.id) == sel_id && l.timing.keys.len() > 1 {
                let n = self.plan.len();
                let (a, z) = l.timing.extent(n);
                let pts: Vec<Pos2> = (a..=z).map(|i| {
                    let mi = l.timing.motion(i);
                    to_screen(mp.apply(b.cx + mi.dx, b.cy + mi.dy))
                }).collect();
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, pal.highlight.gamma_multiply(0.6))));
                for (k, p) in pts.iter().enumerate() {
                    let here = a + k == self.cur;
                    painter.circle_filled(*p, if here { 4.0 } else { 1.8 }, if here { pal.highlight } else { Color32::WHITE.gamma_multiply(0.8) });
                }
            }
            if mo.moves() {
                let o2 = map_box(&mp, mo.apply(b), l.readable());
                let mut pts2: Vec<Pos2> = o2.corners().iter().map(|c| to_screen(*c)).collect();
                pts2.push(pts2[0]);
                painter.extend(egui::Shape::dashed_line(&pts2, Stroke::new(1.2, pal.highlight), 6.0, 4.0));
            }
            let pts: Vec<Pos2> = o.corners().iter().map(|c| to_screen(*c)).collect();
            if Some(l.id) == sel_id {
                painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(2.5, Color32::from_black_alpha(150))));
                painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(1.2, pal.highlight)));
                let top_mid = to_screen(o.from_local(0.0, -o.h / 2.0));
                let rh = rot_handle(&o);
                painter.line_segment([top_mid, rh], Stroke::new(1.2, pal.highlight));
                painter.circle_filled(rh, 7.0, Color32::WHITE);
                painter.circle_stroke(rh, 7.0, Stroke::new(1.5, pal.highlight));
                for c in &pts {
                    let h = Rect::from_center_size(*c, vec2(10.0, 10.0));
                    painter.rect_filled(h, 1.0, Color32::WHITE);
                    painter.rect_stroke(h, 1.0, Stroke::new(1.0, pal.highlight), egui::StrokeKind::Middle);
                }
                for c in edge_mids(&o) {
                    let h = Rect::from_center_size(to_screen(c), vec2(7.0, 7.0));
                    painter.rect_filled(h, 1.0, Color32::WHITE);
                    painter.rect_stroke(h, 1.0, Stroke::new(1.0, pal.highlight), egui::StrokeKind::Middle);
                }
            } else {
                painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0, Color32::from_white_alpha(110))));
            }
        }
        if let Some(pp) = resp.hover_pos() {
            if !alt && self.box_drag.is_none() {
                let pi = m.to_img(pp);
                if let Some(sid) = sel_id {
                    if let Some(c) = cands.iter().find(|c| c.1 == sid) {
                        if rot_handle(&c.3).distance(pp) <= HANDLE_HIT {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Alias);
                        } else if c.3.corners().iter().any(|q| to_screen(*q).distance(pp) <= HANDLE_HIT) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                        } else if let Some(k) = edge_mids(&c.3).iter().position(|q| to_screen(*q).distance(pp) <= HANDLE_HIT) {
                            // pick the arrow closest to how the edge faces on screen
                            let a = (c.3.angle + if k % 2 == 0 { 90.0 } else { 0.0 }).rem_euclid(180.0);
                            ui.ctx().set_cursor_icon(if (45.0..135.0).contains(&a) { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
                        } else if c.3.contains(pi.x, pi.y) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                        }
                    }
                }
            }
        }
    }

    /// Pen, highlighter and eraser on the canvas.
    fn markup_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response, m: Mapper, space: Space, show: bool, (started, dragged, stopped): (bool, bool, bool)) {
        let (bw, bh) = self.dims();
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let shift = ui.input(|i| i.modifiers.shift);
        let layer_map = |s: &Self, li: usize| -> Affine {
            match space {
                Space::Output => s.stack.layer_maps(bw, bh)[li].m,
                Space::Source => Affine::IDENTITY,
            }
        };
        let painter = ui.painter_at(resp.rect);
        if self.pen_tool == PenTool::Eraser {
            let r_img = self.pen.width / 2.0;
            if started || dragged {
                if let (Some(pp), Some(li)) = (resp.interact_pointer_pos(), self.sel_index(Kind::Markup)) {
                    let pi = m.to_img(pp);
                    let mp = layer_map(self, li);
                    let sc = crate::shapes::map_scale(&mp);
                    if started {
                        let now = ui.input(|i| i.time);
                        self.snapshot(&format!("erase{now}"), now);
                    }
                    if let LayerKind::Markup(mk) = &mut self.stack.layers[li].kind {
                        mk.strokes.retain(|st| {
                            !st.applies_to(src_idx)
                                || !st.points.iter().any(|q| {
                                    let (x, y) = mp.apply(q[0], q[1]);
                                    ((x - pi.x).powi(2) + (y - pi.y).powi(2)).sqrt() <= r_img + st.width * sc / 2.0
                                })
                        });
                    }
                }
            }
            if show {
                if let Some(hp) = resp.hover_pos() {
                    painter.circle_stroke(hp, (r_img * m.scale).max(3.0), Stroke::new(1.5, Color32::WHITE));
                    painter.circle_stroke(hp, (r_img * m.scale).max(3.0) + 1.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
                }
            }
            return;
        }
        let brush = if self.pen_tool == PenTool::Highlighter { self.highlighter.clone() } else { self.pen.clone() };
        if started {
            if let Some(pp) = resp.interact_pointer_pos() {
                let now = ui.input(|i| i.time);
                self.snapshot(&format!("pen{now}"), now);
                let id = match self.sel_index(Kind::Markup) {
                    Some(li) => self.stack.layers[li].id,
                    None => self.add_layer(LayerKind::Markup(MarkupSettings::default())),
                };
                self.pen_live = Some((id, vec![m.to_img(pp)], space));
            }
        }
        if dragged {
            if let (Some(pp), Some((_, pts, _))) = (resp.interact_pointer_pos(), self.pen_live.as_mut()) {
                let pi = m.to_img(pp);
                if shift {
                    pts.truncate(1);
                    pts.push(pi);
                } else if pts.last().map(|l| l.distance(pi) * m.scale >= 1.5).unwrap_or(true) {
                    pts.push(pi);
                }
            }
        }
        if stopped {
            if let Some((id, pts, sp)) = self.pen_live.take() {
                if let Some(li) = self.stack.index_of(id) {
                    let mp = if sp == space { layer_map(self, li) } else { Affine::IDENTITY };
                    let inv = mp.inverse();
                    let sc = crate::shapes::map_scale(&mp);
                    let (from, to) = match self.pen_scope {
                        PenScope::All => (1, 0),
                        PenScope::FromHere => (src_idx + 1, 0),
                        PenScope::ThisFrame => (src_idx + 1, src_idx + 1),
                    };
                    let points = pts.iter().map(|q| {
                        let (x, y) = inv.apply(q.x, q.y);
                        [x, y]
                    }).collect();
                    let st = PenStroke { points, width: brush.width / sc, from, to, ..brush.clone() };
                    if let LayerKind::Markup(mk) = &mut self.stack.layers[li].kind {
                        mk.strokes.push(st);
                    }
                    self.modified = true;
                }
            }
        }
        if !show {
            return;
        }
        // the stroke being drawn, shown straight away
        let col = Color32::from_rgba_unmultiplied(brush.color[0], brush.color[1], brush.color[2], (brush.opacity / 100.0 * 255.0) as u8);
        let w = (brush.width * m.scale).max(1.0);
        if let Some((_, pts, _)) = &self.pen_live {
            let sp: Vec<Pos2> = pts.iter().map(|q| m.to_screen(*q)).collect();
            if sp.len() == 1 || brush.mode == PenMode::Pen {
                for q in [sp[0], *sp.last().unwrap()] {
                    painter.circle_filled(q, w / 2.0, col);
                }
            }
            if sp.len() > 1 {
                painter.add(egui::Shape::line(sp, Stroke::new(w, col)));
            }
            ui.ctx().request_repaint();
        }
        if let Some(hp) = resp.hover_pos() {
            if !ui.input(|i| i.modifiers.alt) {
                painter.circle_stroke(hp, w / 2.0, Stroke::new(1.0, Color32::from_white_alpha(200)));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
    }

    /// Write an edited source-space box back into layer `li`.
    fn write_box(&mut self, li: usize, b: Obb, text_size: Option<f32>, src_idx: usize) {
        let scope = self.place_scope;
        let fonts = &self.fonts;
        match &mut self.stack.layers[li].kind {
            LayerKind::Text(t) => {
                if let Some(s) = text_size {
                    t.size = s;
                }
                let (w, h) = text::measure(fonts, t);
                t.x = (b.cx - w as f32 / 2.0).round() as i32;
                t.y = (b.cy - h as f32 / 2.0).round() as i32;
                t.angle = b.angle;
            }
            LayerKind::Censor(c) => {
                c.w = b.w.round().max(1.0) as u32;
                c.h = b.h.round().max(1.0) as u32;
                c.x = (b.cx - c.w as f32 / 2.0).round() as i32;
                c.y = (b.cy - c.h as f32 / 2.0).round() as i32;
                c.angle = b.angle;
            }
            LayerKind::Overlay(o) => {
                let n = o.placements.len();
                if n == 0 {
                    return;
                }
                let w = b.w.round().max(1.0) as u32;
                let h = b.h.round().max(1.0) as u32;
                let x = (b.cx - w as f32 / 2.0).round() as i32;
                let y = (b.cy - h as f32 / 2.0).round() as i32;
                let cur = src_idx.min(n - 1);
                for q in &mut o.placements[scope.range(cur, n)] {
                    q.x = x;
                    q.y = y;
                    q.w = w;
                    q.h = h;
                    q.angle = b.angle;
                }
            }
            LayerKind::Shape(sh) => {
                sh.w = b.w.round().max(1.0) as u32;
                sh.h = b.h.round().max(1.0) as u32;
                sh.x = (b.cx - sh.w as f32 / 2.0).round() as i32;
                sh.y = (b.cy - sh.h as f32 / 2.0).round() as i32;
                sh.angle = b.angle;
            }
            LayerKind::Emoji(e) => {
                e.w = b.w.round().max(1.0) as u32;
                e.h = b.h.round().max(1.0) as u32;
                e.x = (b.cx - e.w as f32 / 2.0).round() as i32;
                e.y = (b.cy - e.h as f32 / 2.0).round() as i32;
                e.angle = b.angle;
            }
            LayerKind::Effects(_) | LayerKind::Watermark(_) | LayerKind::Shield(_) | LayerKind::Markup(_) => {}
        }
        self.modified = true;
    }

    // ------------------------------------------------------------ frames grid

    fn frames_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let now = ctx.input(|i| i.time);
        let n = self.base.as_ref().map(|a| a.len()).unwrap_or(0);
        let sel_src = self.plan.get(self.cur).map(|f| f.src).unwrap_or(usize::MAX);
        let (cs, ce) = self.base.as_ref().map(|b| self.stack.transform.cut_range(b)).unwrap_or((0, 0));
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{n} source frames")).font(theme::medium(14.0)).color(pal.fg));
            ui.label(RichText::new("ℹ").color(pal.fg_muted)).on_hover_text("Changes here apply under all live edits. Skipped frames are left out of playback and export. Right-click a frame for more.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().slider_width = 140.0;
                slider(ui, egui::Slider::new(&mut self.fr_thumb, 32.0..=220.0).show_value(false));
                ui.label("Size:");
            });
        });
        ui.add_space(6.0);
        // every part of a cell scales with the thumbnail, so cells never
        // outgrow their column and spill into the preview
        let s = self.fr_thumb.round();
        let pad = if s < 60.0 { 3.0 } else { 5.0 };
        let small = s < 72.0;
        let foot = if small { 16.0 } else { 22.0 };
        let cell = vec2(s + pad * 2.0, s + pad * 2.0 + foot);
        let gap = if small { 4.0 } else { 8.0 };
        let mut action: Option<(usize, &'static str)> = None;
        let mut delay_change: Option<(usize, u16)> = None;
        let cols = (((ui.available_width() - 14.0 + gap) / (cell.x + gap)).floor() as usize).max(1);
        // only the rows in view are laid out (and only their thumbnails made),
        // so a few thousand frames scroll as easily as a few
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, cell.y, n.div_ceil(cols), |ui, rows| {
            for row in rows {
                ui.horizontal(|ui| {
                    for i in row * cols..((row + 1) * cols).min(n) {
                        let (enabled, mut delay) = {
                            let f = &self.base.as_ref().unwrap().frames[i];
                            (f.enabled, f.delay_cs)
                        };
                        let in_cut = i >= cs && i <= ce;
                        let tex = self.thumb_texture(ctx, i);
                        let sel = i == sel_src;
                        let (r, resp) = ui.allocate_exact_size(cell, Sense::click());
                        let hovered = ui.rect_contains_pointer(r);
                        let painter = ui.painter_at(r);
                        painter.rect_filled(r, 3.0, if sel { pal.highlight_soft } else if hovered { mix(pal.view, pal.highlight, 0.08) } else { pal.view });
                        painter.rect_stroke(r, 3.0, Stroke::new(1.0, if sel { pal.highlight } else { pal.border }), egui::StrokeKind::Inside);
                        let tr = Rect::from_min_size(r.min + vec2(pad, pad), vec2(s, s));
                        painter.rect_filled(tr, 2.0, Color32::from_gray(200));
                        if let Some(t) = &tex {
                            let ts = t.size_vec2();
                            let k = (s / ts.x).min(s / ts.y);
                            let ir = Rect::from_center_size(tr.center(), ts * k);
                            painter.image(t.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if enabled && in_cut { Color32::WHITE } else { Color32::from_white_alpha(60) });
                        }
                        if !enabled {
                            painter.line_segment([tr.left_top(), tr.right_bottom()], Stroke::new(2.0, pal.negative));
                            painter.line_segment([tr.right_top(), tr.left_bottom()], Stroke::new(2.0, pal.negative));
                        }
                        // footer: number on the left, delay on the right
                        let fr = Rect::from_min_max(pos2(r.min.x + pad, tr.max.y), pos2(r.max.x - pad, r.max.y - 1.0));
                        let num = format!("{}{}", i + 1, if in_cut { "" } else { " ✂" });
                        painter.text(pos2(fr.min.x + 1.0, fr.center().y), egui::Align2::LEFT_CENTER, num, egui::FontId::proportional(if small { 10.5 } else { 12.0 }), pal.fg_muted);
                        if !small {
                            let dr = Rect::from_min_max(pos2(fr.max.x - 54.0, fr.min.y + 2.0), pos2(fr.max.x, fr.max.y - 1.0));
                            // a small borderless value, sized to the footer
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(dr).layout(egui::Layout::right_to_left(egui::Align::Center)));
                            child.spacing_mut().interact_size = dr.size();
                            child.spacing_mut().button_padding = vec2(3.0, 0.0);
                            for st in [&mut child.visuals_mut().widgets.inactive] {
                                st.bg_stroke = Stroke::NONE;
                                st.weak_bg_fill = Color32::TRANSPARENT;
                                st.bg_fill = Color32::TRANSPARENT;
                            }
                            child.style_mut().override_font_id = Some(egui::FontId::proportional(12.0));
                            let d = child.add(egui::DragValue::new(&mut delay).range(1..=6000).speed(0.2).suffix(" cs")).on_hover_text("Delay in hundredths of a second. Drag or click to type.");
                            if d.changed() {
                                delay_change = Some((i, delay));
                            }
                        }
                        // quick actions over the thumbnail on hover, when there's room
                        if hovered && s >= 64.0 {
                            let b = 22.0;
                            let strip = Rect::from_min_size(pos2(tr.min.x, tr.max.y - b - 2.0), vec2(tr.width(), b + 2.0));
                            painter.rect_filled(strip, 0.0, Color32::from_black_alpha(140));
                            let acts: [(&str, &str, &'static str); 5] = [
                                (if enabled { "view-hidden" } else { "view-visible" }, if enabled { "Skip" } else { "Use again" }, "toggle"),
                                ("edit-duplicate", "Duplicate", "copy"),
                                ("go-previous", "Move left", "left"),
                                ("go-next", "Move right", "right"),
                                ("edit-delete", "Delete", "delete"),
                            ];
                            let room = ((tr.width() / (b + 1.0)).floor() as usize).min(acts.len());
                            let x0 = tr.center().x - room as f32 * (b + 1.0) / 2.0;
                            for (k, (icon, tip, act)) in acts.iter().take(room).enumerate() {
                                let br = Rect::from_min_size(pos2(x0 + k as f32 * (b + 1.0), strip.min.y + 1.0), vec2(b, b));
                                let rr = ui.interact(br, ui.scope_id().with(("fr-act", i, k)), Sense::click());
                                if rr.hovered() {
                                    painter.rect_filled(br, 2.0, Color32::from_white_alpha(40));
                                }
                                crate::icons::paint(&painter, icon, br.center(), 16.0, Color32::WHITE);
                                if rr.on_hover_text(*tip).clicked() {
                                    action = Some((i, act));
                                }
                            }
                        }
                        let resp = resp.on_hover_text(format!("Frame {} · {} cs{}", i + 1, delay, if enabled { "" } else { " · skipped" }));
                        if resp.clicked() {
                            action = Some((i, "select"));
                        }
                        resp.context_menu(|ui| {
                            ui.set_min_width(200.0);
                            if menu_item(ui, if enabled { "view-hidden" } else { "view-visible" }, if enabled { "&Skip Frame" } else { "&Use Frame Again" }, "", true) {
                                action = Some((i, "toggle"));
                            }
                            if menu_item(ui, "edit-duplicate", "&Duplicate", "", true) {
                                action = Some((i, "copy"));
                            }
                            ui.separator();
                            if menu_item(ui, "go-previous", "Move &Left", "", i > 0) {
                                action = Some((i, "left"));
                            }
                            if menu_item(ui, "go-next", "Move &Right", "", i + 1 < n) {
                                action = Some((i, "right"));
                            }
                            ui.separator();
                            if menu_item(ui, "edit-delete", "De&lete Frame", "", n > 1) {
                                action = Some((i, "delete"));
                            }
                        });
                    }
                });
            }
        });
        if self.busy() {
            return;
        }
        if let Some((i, d)) = delay_change {
            self.snapshot(&format!("delay{i}"), now);
            self.base.as_mut().unwrap().frames[i].delay_cs = d;
            self.timing_rev += 1;
        }
        if let Some((i, act)) = action {
            if act == "select" {
                if let Some(pi) = self.plan.iter().position(|f| f.src == i) {
                    self.cur = pi;
                }
                self.playing = false;
                return;
            }
            self.snapshot(&format!("{act}{i}{now}"), now);
            let a = self.base.as_mut().unwrap();
            match act {
                "toggle" => a.frames[i].enabled = !a.frames[i].enabled,
                "copy" => {
                    let f = a.frames[i].clone();
                    a.frames.insert(i + 1, f);
                }
                "left" if i > 0 => a.frames.swap(i, i - 1),
                "right" if i + 1 < a.len() => a.frames.swap(i, i + 1),
                "delete" if a.len() > 1 => {
                    a.frames.remove(i);
                }
                _ => {}
            }
            if act == "toggle" {
                self.timing_rev += 1;
            } else {
                self.bump();
            }
            self.after_base_change(false);
        }
    }

    // ------------------------------------------------------------ tool panels

    fn tool_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let timed = self.tool.layer_kind().and_then(|k| self.sel_index(k)).and_then(|i| self.stack.layers.get(i)).is_some_and(|l| l.timing.clips.is_some());
        SEL_TIMED.with(|c| c.set(timed));
        // a locked layer's settings can be seen but not changed
        if self.sel_layer_index().is_some_and(|i| self.stack.layers[i].locked) {
            ui.add_enabled_ui(false, |ui| self.tool_panel_inner(ui, ctx));
            return;
        }
        self.tool_panel_inner(ui, ctx);
    }

    fn tool_panel_inner(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.tool {
            Tool::Resize => self.ui_resize(ui, ctx),
            Tool::Crop => self.ui_crop(ui, ctx),
            Tool::Rotate => self.ui_rotate(ui, ctx),
            Tool::Reverse => self.ui_reverse(ui, ctx),
            Tool::Speed => self.ui_speed(ui, ctx),
            Tool::Optimize => self.ui_optimize(ui, ctx),
            Tool::Effects => self.ui_effects(ui, ctx),
            Tool::Animation => self.ui_animation(ui),
            Tool::Text => self.ui_text(ui, ctx),
            Tool::Censor => self.ui_censor(ui, ctx),
            Tool::AddImage => self.ui_add_image(ui, ctx),
            Tool::Watermark => self.ui_watermark(ui, ctx),
            Tool::Shield => self.ui_shield(ui, ctx),
            Tool::Shapes => self.ui_shapes(ui, ctx),
            Tool::Markup => self.ui_markup(ui, ctx),
            Tool::Emoji => self.ui_emoji(ui, ctx),
            Tool::Cut => self.ui_cut(ui, ctx),
            Tool::Frames => self.ui_frames(ui, ctx),
            Tool::Presets => self.ui_presets(ui, ctx),
        }
    }

    /// Enable switch, reset and bake for live frame-edit tools.
    fn transform_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, on: impl Fn(&mut Transform) -> &mut bool, reset: impl Fn(&mut Transform)) {
        let mut enabled = *on(&mut self.stack.transform);
        let mut do_reset = false;
        let mut do_bake = false;
        ui.horizontal(|ui| {
            check(ui, &mut enabled, "Enabled");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                do_bake = ui.add(egui::Button::new("Collapse").min_size(vec2(84.0, CHIP_H))).on_hover_text("Make frame edits permanent. Layers stay editable.").clicked();
                do_reset = ui.add(egui::Button::new("Reset").min_size(vec2(84.0, CHIP_H))).clicked();
            });
        });
        *on(&mut self.stack.transform) = enabled;
        if do_reset {
            reset(&mut self.stack.transform);
            *on(&mut self.stack.transform) = false;
        }
        if do_bake {
            self.bake(ctx, true);
        }
        ui.add_space(8.0);
    }

    fn ui_resize(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.resize_on, |t| {
            t.rs_w = 0;
            t.rs_h = 0;
            t.rs_pct = 0.0;
        });
        let (bw, bh) = self.dims();
        let tr = &mut self.stack.transform;
        let (w, h) = if tr.crop_on && tr.crop[2] > 0 { (tr.crop[2], tr.crop[3]) } else { (bw, bh) };
        let before = (tr.rs_w, tr.rs_h, tr.rs_pct.to_bits(), tr.rs_mode, tr.rs_filter);
        // follow the stored settings (undo, presets)
        if tr.rs_pct == 0.0 && (tr.rs_w > 0 || tr.rs_h > 0) {
            self.resize_by_pct = false;
        } else if tr.rs_pct > 0.0 {
            self.resize_by_pct = true;
        }
        let by_pct = &mut self.resize_by_pct;
        group(ui, "Size", |ui| {
            let mut mode = *by_pct;
            form_row(ui, "Resize by", |ui| seg_row(ui, &mut mode, &[(false, "Pixels"), (true, "Percent")]));
            if mode != *by_pct {
                *by_pct = mode;
                if !mode {
                    // keep the current size when switching
                    if tr.rs_pct > 0.0 {
                        if let Some((tw, th)) = stack::resize_target(w, h, 0, 0, tr.rs_pct) {
                            tr.rs_w = tw;
                            tr.rs_h = th;
                        }
                        tr.rs_pct = 0.0;
                    }
                } else {
                    let cur = match stack::resize_target(w, h, tr.rs_w, tr.rs_h, 0.0) {
                        Some((tw, _)) => tw as f32 / w.max(1) as f32 * 100.0,
                        None => 100.0,
                    };
                    tr.rs_pct = (cur * 10.0).round() / 10.0;
                }
            }
            if *by_pct {
                let mut pct = if tr.rs_pct > 0.0 { tr.rs_pct } else { 100.0 };
                let r1 = slider_row(ui, "Scale", egui::Slider::new(&mut pct, 1.0..=400.0).suffix(" %").max_decimals(1).clamping(egui::SliderClamping::Never).logarithmic(true))
                    .on_hover_text("Drag or type any value such as 37.5 or 1200");
                if r1.changed() {
                    tr.rs_pct = pct.clamp(0.1, 6400.0);
                }
                const PCTS: [f32; 9] = [10.0, 25.0, 33.3, 50.0, 75.0, 100.0, 150.0, 200.0, 400.0];
                form_row(ui, "", |ui| {
                    even_grid(ui, PCTS.len(), 60.0, |ui, i, size| {
                        if ui.add_sized(size, sel_btn(egui::Button::new(format!("{}%", PCTS[i])), tr.rs_pct == PCTS[i])).clicked() {
                            tr.rs_pct = PCTS[i];
                        }
                    })
                });
            } else {
                tr.rs_pct = 0.0;
                form_row(ui, "Size", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let v = if i == 0 { &mut tr.rs_w } else { &mut tr.rs_h };
                        spin(ui, size, v, 1.0, |d| d.range(0..=16384).prefix(if i == 0 { "W  " } else { "H  " }).suffix(" px")).on_hover_text("Leave one at 0 to keep the aspect ratio");
                    })
                });
                form_row(ui, "", |ui| {
                    if wide_button(ui, egui::Button::new("Clear")).clicked() {
                        tr.rs_w = 0;
                        tr.rs_h = 0;
                    }
                });
            }
            let res = match stack::resize_target(w, h, tr.rs_w, tr.rs_h, tr.rs_pct) {
                Some((tw, th)) => format!("{w}×{h} → {tw}×{th}"),
                None => format!("{w}×{h}"),
            };
            form_row(ui, "Result", |ui| cell(ui, vec2(ui.available_width(), CHIP_H), |ui| hint(ui, &res)));
        });
        group(ui, "Fit", |ui| {
            let modes: Vec<(AspectMode, &str)> = AspectMode::ALL.iter().map(|m| (*m, m.label())).collect();
            form_row(ui, "Aspect", |ui| combo(ui, "rsmode", &mut tr.rs_mode, &modes));
            let filters: Vec<(ResizeFilter, &str)> = ResizeFilter::ALL.iter().map(|m| (*m, m.label())).collect();
            form_row(ui, "Filter", |ui| combo(ui, "rsfilt", &mut tr.rs_filter, &filters));
        });
        if before != (tr.rs_w, tr.rs_h, tr.rs_pct.to_bits(), tr.rs_mode, tr.rs_filter) {
            tr.resize_on = true;
        }
        // more room around the picture, for captions or a bigger frame
        group(ui, "Canvas", |ui| {
            let names = ["Top", "Bottom", "Left", "Right"];
            let idx = [0usize, 2, 3, 1];
            form_row(ui, "Add space", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    let v = &mut tr.pad[idx[i]];
                    spin(ui, size, v, 1.0, |d| d.range(0..=8192).prefix(format!("{}  ", names[i])).suffix(" px"));
                });
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    let v = &mut tr.pad[idx[i + 2]];
                    spin(ui, size, v, 1.0, |d| d.range(0..=8192).prefix(format!("{}  ", names[i + 2])).suffix(" px"));
                });
            });
            form_row(ui, "Fill", |ui| {
                ui.horizontal(|ui| {
                    if radio(ui, !tr.pad_transparent, "Colour").clicked() {
                        tr.pad_transparent = false;
                    }
                    ui.add_enabled_ui(!tr.pad_transparent, |ui| colour_btn(ui, &mut tr.pad_color));
                    ui.add_space(10.0);
                    if radio(ui, tr.pad_transparent, "Transparent").clicked() {
                        tr.pad_transparent = true;
                    }
                })
            });
            tr.pad_on = tr.pad.iter().any(|p| *p > 0);
            let [t, r, b, l] = tr.pad;
            let (ow, oh) = match stack::resize_target(w, h, tr.rs_w, tr.rs_h, tr.rs_pct) {
                Some((tw, th)) if tr.resize_on => (tw, th),
                _ => (w, h),
            };
            form_row(ui, "Result", |ui| hint(ui, &if tr.pad_on { format!("{}×{} with the space", ow + l + r, oh + t + b) } else { "No space added".to_string() }));
            form_row(ui, "", |ui| {
                ui.add_enabled_ui(tr.pad_on, |ui| {
                    if wide_button(ui, egui::Button::new("Remove Space")).clicked() {
                        tr.pad = [0; 4];
                        tr.pad_on = false;
                    }
                });
            });
        });
    }

    fn ui_crop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.crop_on, |_| {});
        let (w, h) = self.dims();
        group(ui, "Show", |ui| {
            form_row(ui, "Preview", |ui| seg_row(ui, &mut self.crop_live, &[(true, "With Edits"), (false, "Original")]));
            form_row(ui, "", |ui| hint(ui, "Drag on the image to set the crop"));
        });
        let tr = &mut self.stack.transform;
        let before = tr.crop;
        group(ui, "Area", |ui| {
            let r = &mut tr.crop;
            form_row(ui, "Position", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut r[0], 1.0, |d| d.range(0..=w.saturating_sub(1)).prefix("X  "));
                    } else {
                        // shown up from the bottom of the frame
                        let mut up = h.saturating_sub(r[1] + r[3]);
                        if spin(ui, size, &mut up, 1.0, |d| d.range(0..=h.saturating_sub(1)).prefix("Y  ")).on_hover_text("Bottom edge, up from the bottom of the frame").changed() {
                            r[1] = h.saturating_sub(up + r[3]);
                        }
                    }
                })
            });
            let h0 = r[3];
            form_row(ui, "Size", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    let max = if i == 0 { w } else { h };
                    spin(ui, size, &mut r[2 + i], 1.0, |d| d.range(1..=max).prefix(if i == 0 { "W  " } else { "H  " }));
                })
            });
            // a new height keeps the bottom edge, as Y counts from there
            if r[3] != h0 {
                r[1] = (r[1] + h0).saturating_sub(r[3]);
            }
            r[2] = r[2].min(w.saturating_sub(r[0])).max(1);
            r[3] = r[3].min(h.saturating_sub(r[1])).max(1);
            let locks: Vec<(AspectLock, &str)> = AspectLock::ALL.iter().map(|l| (*l, l.label())).collect();
            form_row(ui, "Aspect", |ui| combo(ui, "croplock", &mut self.crop_lock, &locks));
            form_row(ui, "", |ui| {
                if wide_button(ui, egui::Button::new("Select Whole Frame")).clicked() {
                    *r = [0, 0, w, h];
                }
            });
        });
        let mut detect = false;
        group(ui, "Auto-Trim", |ui| {
            form_row(ui, "Tolerance", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut self.trim_tol, 1.0, |d| d.range(0..=255));
                    } else {
                        detect = ui.add_sized(size, egui::Button::new("Detect")).on_hover_text("Shrink the area to the content of every frame").clicked();
                    }
                })
            });
        });
        if detect {
            if let Some(a) = &self.base {
                match ops::auto_trim_rect(a, self.trim_tol) {
                    Some((x, y, w, h)) => self.stack.transform.crop = [x, y, w, h],
                    None => self.error("Nothing to trim"),
                }
            }
        }
        if self.stack.transform.crop != before {
            self.stack.transform.crop_on = true;
        }
    }

    fn ui_rotate(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.rot_on, |t| {
            t.flip_h = false;
            t.flip_v = false;
            t.rot = 0;
            t.rot_any = false;
        });
        let tr = &mut self.stack.transform;
        let before = format!("{} {} {} {} {} {} {} {:?}", tr.flip_v, tr.flip_h, tr.rot, tr.rot_any, tr.rot_deg, tr.rot_expand, tr.rot_bg_transparent, tr.rot_bg);
        group(ui, "Flip", |ui| {
            form_row(ui, "Flip", |ui| {
                check(ui, &mut tr.flip_h, "Horizontally");
                check(ui, &mut tr.flip_v, "Vertically");
            });
        });
        group(ui, "Rotate", |ui| {
            form_row(ui, "Turn", |ui| seg_row(ui, &mut tr.rot, &[(0u16, "None"), (90, "90° clockwise"), (180, "180°"), (270, "90° anticlockwise")]));
            let changed = opt_row(ui, "Free angle", &mut tr.rot_any, |ui| fill_slider(ui, egui::Slider::new(&mut tr.rot_deg, -180.0..=180.0).suffix("°").max_decimals(1).clamping(egui::SliderClamping::Never)).changed());
            if changed {
                tr.rot_any = true;
            }
            form_row(ui, "", |ui| check(ui, &mut tr.rot_expand, "Expand canvas"));
            form_row(ui, "Fill", |ui| {
                ui.horizontal(|ui| {
                    check(ui, &mut tr.rot_bg_transparent, "Transparent");
                    ui.add_enabled_ui(!tr.rot_bg_transparent, |ui| colour_btn(ui, &mut tr.rot_bg).on_hover_text("Colour of the corners the turn uncovers"));
                })
            });
        });
        if before != format!("{} {} {} {} {} {} {} {:?}", tr.flip_v, tr.flip_h, tr.rot, tr.rot_any, tr.rot_deg, tr.rot_expand, tr.rot_bg_transparent, tr.rot_bg) {
            tr.rot_on = true;
        }
    }

    fn ui_reverse(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let tr = &mut self.stack.transform;
        group(ui, "Playback", |ui| {
            form_row(ui, "Direction", |ui| {
                check(ui, &mut tr.reverse, "Reverse");
                check(ui, &mut tr.boomerang, "Boomerang").on_hover_text("Play forward and then backward");
            });
            opt_row(ui, "Loop count", &mut tr.set_loop, |ui| num_fill(ui, &mut tr.loop_count, 1.0, |d| d.range(0..=65535)).on_hover_text("0 loops forever"));
        });
        form_row(ui, "", |ui| {
            if wide_button(ui, egui::Button::new("Collapse Frame Edits")).on_hover_text("Make frame edits permanent. Layers stay editable.").clicked() {
                self.bake(ctx, true);
            }
        });
        group(ui, "Seamless Loop", |ui| {
            let idle = !self.busy() && self.base.is_some();
            form_row(ui, "", |ui| {
                let b = ui.add_enabled_ui(idle, |ui| wide_button(ui, egui::Button::new("Find Loop Point"))).inner.on_hover_text("Finds the two frames that look most alike and sets in and out around them, so it loops with the least jump");
                if b.clicked() {
                    self.find_loop_point(ctx);
                }
            });
            form_row(ui, "Blend", |ui| {
                ui.horizontal(|ui| {
                    num_fill(ui, &mut self.loop_blend, 1.0, |d| d.range(1..=60).suffix(" frames")).on_hover_text("How many frames the end fades into the start over");
                });
            });
            form_row(ui, "", |ui| {
                let go = ui.add_enabled_ui(idle, |ui| wide_button(ui, egui::Button::new("Blend the Seam"))).inner.on_hover_text("The end fades into the start, and the animation gets that many frames shorter, so the jump back to frame 1 disappears");
                if go.clicked() {
                    let n = self.loop_blend;
                    self.run_anim(ctx, "Seamless loop", move |a, _| Ok(ops::loop_crossfade(a, n)));
                }
            });
        });
    }

    /// Look for the best loop in the background (on the frames as edited).
    fn find_loop_point(&mut self, ctx: &egui::Context) {
        let Some(base) = self.base.clone() else { return };
        let plan = self.plan.clone();
        let n = plan.len();
        self.spawn(ctx, "Find loop point", n, move |_| {
            let frames: Vec<Arc<RgbaImage>> = plan.iter().filter_map(|f| base.frames.get(f.src)).map(|f| f.image.get()).collect();
            JobOutput::LoopPoint(ops::find_loop(&frames, (n / 4).max(4)))
        });
    }

    fn ui_speed(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.speed_on, |t| {
            t.speed_mode = SpeedMode::Percent;
            t.speed_val = 100.0;
        });
        let avg_fps = self.base.as_ref().map(|a| a.enabled_count() as f32 / a.duration_secs().max(0.001)).unwrap_or(0.0);
        let tr = &mut self.stack.transform;
        let before = (tr.speed_mode, tr.speed_val.to_bits());
        group(ui, "Speed", |ui| {
            form_row(ui, "Set as", |ui| combo(ui, "spmode", &mut tr.speed_mode, &[(SpeedMode::Percent, "% of original"), (SpeedMode::DelayAll, "Delay per frame (cs)"), (SpeedMode::Fps, "Frame rate (fps)")]));
            match tr.speed_mode {
                SpeedMode::Percent => {
                    slider_row(ui, "Speed", soft_slider(&mut tr.speed_val, 10.0..=400.0, "").suffix(" %").max_decimals(0));
                    const PCTS: [f32; 6] = [25.0, 50.0, 100.0, 150.0, 200.0, 300.0];
                    form_row(ui, "", |ui| {
                        even_grid(ui, PCTS.len(), 60.0, |ui, i, size| {
                            if ui.add_sized(size, sel_btn(egui::Button::new(format!("{:.0}%", PCTS[i])), tr.speed_val == PCTS[i])).clicked() {
                                tr.speed_val = PCTS[i];
                            }
                        })
                    });
                }
                SpeedMode::DelayAll => {
                    form_row(ui, "Delay", |ui| num_fill(ui, &mut tr.speed_val, 1.0, |d| d.range(1.0..=100000.0).speed(0.2).max_decimals(0).suffix(" cs"))).on_hover_text("One delay for every frame in hundredths of a second");
                }
                SpeedMode::Fps => {
                    form_row(ui, "Frame rate", |ui| num_fill(ui, &mut tr.speed_val, 1.0, |d| d.range(0.1..=100.0).speed(0.1).max_decimals(2).suffix(" fps"))).on_hover_text("One frame rate for every frame");
                }
            }
            if tr.speed_mode != SpeedMode::Percent {
                let d = if tr.speed_mode == SpeedMode::Fps { (100.0 / tr.speed_val).round() } else { tr.speed_val };
                let text = if d < 2.0 { "⚠ Too fast for browsers".to_string() } else { format!("{} cs per frame ({:.2} fps)", d.max(1.0), 100.0 / d.max(1.0)) };
                form_row(ui, "", |ui| {
                    if d < 2.0 {
                        ui.label(RichText::new(&text).color(p().neutral)).on_hover_text("Most browsers play delays under 2 cs at 10 cs.");
                    } else {
                        hint(ui, &text);
                    }
                });
            }
            form_row(ui, "Source", |ui| cell(ui, vec2(ui.available_width(), CHIP_H), |ui| hint(ui, &format!("{avg_fps:.1} fps"))));
        });
        if before != (tr.speed_mode, tr.speed_val.to_bits()) {
            tr.speed_on = true;
        }
    }

    fn ui_cut(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.cut_on, |t| {
            t.cut_start = 1;
            t.cut_end = 0;
        });
        let (n, dur) = self.base.as_ref().map(|a| (a.len(), a.duration_secs())).unwrap_or((1, 0.0));
        let src = self.plan.get(self.cur).map(|f| f.src + 1).unwrap_or(1);
        let tr = &mut self.stack.transform;
        let before = format!("{} {} {} {} {}", tr.cut_by_time, tr.cut_start, tr.cut_end, tr.cut_t0, tr.cut_t1);
        group(ui, "Range", |ui| {
            form_row(ui, "Measure in", |ui| seg_row(ui, &mut tr.cut_by_time, &[(false, "Frames"), (true, "Seconds")]));
            if tr.cut_by_time {
                form_row(ui, "Keep", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let v = if i == 0 { &mut tr.cut_t0 } else { &mut tr.cut_t1 };
                        spin(ui, size, v, 0.1, |d| d.range(0.0..=dur).speed(0.01).max_decimals(2).prefix(if i == 0 { "From " } else { "To " }).suffix(" s"));
                    })
                });
            } else {
                let mut end = if tr.cut_end == 0 { n } else { tr.cut_end };
                form_row(ui, "Keep", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let v = if i == 0 { &mut tr.cut_start } else { &mut end };
                        spin(ui, size, v, 1.0, |d| d.range(1..=n).prefix(if i == 0 { "From " } else { "To " }));
                    })
                });
                tr.cut_end = if end >= n { 0 } else { end };
                form_row(ui, "", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let r = ui.add_sized(size, egui::Button::new(if i == 0 { "Start Here" } else { "End Here" })).on_hover_text("Use the current frame");
                        if r.clicked() {
                            if i == 0 {
                                tr.cut_start = src;
                            } else {
                                tr.cut_end = src;
                            }
                        }
                    })
                });
            }
        });
        if before != format!("{} {} {} {} {}", tr.cut_by_time, tr.cut_start, tr.cut_end, tr.cut_t0, tr.cut_t1) {
            tr.cut_on = true;
        }
        if let Some(b) = &self.base {
            let (s, e) = self.stack.transform.cut_range(b);
            form_row(ui, "Result", |ui| hint(ui, &format!("Frames {}–{} ({} frames)", s + 1, e + 1, e - s + 1)));
        }
    }

    fn ui_optimize(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        form_row(ui, "Format", |ui| self.format_menu(ui, CHIP_H));
        ui.add_space(8.0);
        let cur_key = self.cmp_key();
        let busy = self.cmp_job.is_some();
        let mut full = false;
        group(ui, "Result", |ui| match &self.cmp {
            Some(c) => {
                let n = c.size;
                let stale = c.key != cur_key;
                // in the field column, lined up with the options below
                form_row(ui, if c.sample.is_some() { "Estimate" } else { "Size" }, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(c.size_text()).font(theme::medium(16.0)).color(if stale { pal.fg_muted } else { pal.positive }));
                        if let Some(orig) = self.file_size {
                            hint(ui, &format!("{:+.0}% vs original", (n as f64 / orig as f64 - 1.0) * 100.0));
                        }
                        if stale || busy {
                            ui.spinner();
                        }
                    });
                });
                if let Some(smp) = &c.sample {
                    form_row(ui, "", |ui| {
                        hint(ui, &format!("Estimated from {} of {} frames to keep editing quick", smp.len(), self.plan.len()));
                        full = ui.add_enabled(!busy, egui::Button::new("Compress Fully").min_size(vec2(0.0, CHIP_H))).on_hover_text("Encode every frame for the exact size and preview. Takes a while on long clips.").clicked();
                    });
                }
            }
            None => {
                form_row(ui, "Size", |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        hint(ui, "Encoding…");
                    });
                });
            }
        });
        if full {
            self.cmp_full_key = cur_key;
            self.cmp = None;
        }
        let mut find = false;
        let mut reset = false;
        let idle = !self.busy();
        if self.export_fmt.can_target() {
            group(ui, "Target Size", |ui| {
                form_row(ui, "Limit", |ui| {
                    // a spin box with its unit beside it, as Qt forms do
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        if i == 0 {
                            spin(ui, size, &mut self.target_val, 1.0, |d| d.range(0.01..=10000.0).speed(0.1).max_decimals(2));
                        } else {
                            ui.scope(|ui| {
                                ui.set_width(size.x);
                                combo(ui, "target_unit", &mut self.target_mb, &[(true, "MiB"), (false, "KiB")]);
                            });
                        }
                    })
                });
                form_row(ui, "", |ui| {
                    let r = wide_button(ui, drop_button(ui, "Platforms", p().fg)).on_hover_text("Upload limits of popular sites. Emoji sizes shrink the frames too.");
                    let mut pick = None;
                    egui::Popup::menu(&r).show(|ui| {
                        ui.set_min_width(190.0);
                        for (i, pf) in batch_ui::PLATFORMS.iter().enumerate() {
                            if menu_item(ui, "", pf.name, pf.note, true) {
                                pick = Some(i);
                            }
                        }
                    });
                    if let Some(i) = pick {
                        self.use_platform(i);
                    }
                });
                form_row(ui, "", |ui| {
                    if check(ui, &mut self.fit_keep_size, "Keep frame size").on_hover_text("Only change compression and never shrink the frames").changed() {
                        theme::set_setting("fit_keep_size", if self.fit_keep_size { "1" } else { "0" });
                    }
                });
                form_row(ui, "", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        if i == 0 {
                            find = ui.add_enabled_ui(idle, |ui| ui.add_sized(size, egui::Button::new("Find Settings").stroke(Stroke::new(1.0, p().highlight)))).inner.on_hover_text("Finds the best quality under the limit and applies it. Quality drops first and size only if needed.").clicked();
                        } else {
                            reset = ui.add_enabled_ui(idle, |ui| ui.add_sized(size, egui::Button::new("Reset"))).inner.on_hover_text("Back to lossless settings. Also undoes any downscale from Find Settings.").clicked();
                        }
                    })
                });
            });
        }
        if reset {
            self.undo_last_fit();
            self.gif_opts = GifOptions { noai_tag: self.gif_opts.noai_tag, ..self.gif_opts_open.clone() };
            self.webp_opts = io::WebpOptions::default();
            self.avif_quality = 60;
            self.info("Compression reset");
        }
        if find {
            // start from the settings before the previous search, not its result
            self.undo_last_fit();
            let max = self.target_bytes();
            let tf = match self.export_fmt {
                ExportFormat::Webp => crate::target::TargetFormat::Webp(self.webp_opts),
                ExportFormat::Avif => crate::target::TargetFormat::Avif(self.avif_quality),
                _ => crate::target::TargetFormat::Gif(self.gif_opts.clone()),
            };
            let allow_scale = !self.fit_keep_size;
            // the preview already made the file the current settings give
            let have = self.cmp.as_ref().filter(|c| c.key == self.cmp_key() && c.sample.is_none() && !c.sheet).map(|c| (*c.bytes).clone());
            let (range, cut_empty) = self.export_trim();
            if let Some(base) = self.base.clone() {
                let stack = self.stack.clone();
                let fonts = self.fonts.clone();
                let total = self.plan.len();
                self.spawn(ctx, &format!("Finding settings under {}", fmt_size(max)), total, move |p| {
                    // render each scale through the real Resize step, trimmed
                    // as Export trims, so sizes match the preview and the file
                    JobOutput::Fit(crate::target::fit_from(&tf, max, allow_scale, p, have, |sc| {
                        let mut st = stack.clone();
                        apply_fit_scale(&mut st.transform, sc);
                        st.flatten_for_export(&base, &fonts, None, range, cut_empty).0
                    }))
                });
            }
        }
        match self.export_fmt {
            ExportFormat::Gif => {}
            ExportFormat::Webp => {
                let o = &mut self.webp_opts;
                group(ui, "WebP", |ui| {
                    form_row(ui, "Mode", |ui| seg_row(ui, &mut o.lossless, &[(false, "Lossy"), (true, "Lossless")]));
                    let lossy = !o.lossless;
                    ui.add_enabled_ui(lossy, |ui| slider_row(ui, "Quality", egui::Slider::new(&mut o.quality, 0.0..=100.0).max_decimals(0)).on_hover_text("75–90 looks like the original for most GIFs"));
                    slider_row(ui, "Effort", egui::Slider::new(&mut o.method, 0..=6)).on_hover_text("Higher makes smaller files but takes longer");
                });
                return;
            }
            ExportFormat::Avif => {
                group(ui, "AVIF", |ui| {
                    slider_row(ui, "Quality", egui::Slider::new(&mut self.avif_quality, 0..=100)).on_hover_text("50–70 is a good balance");
                    form_row(ui, "", |ui| hint(ui, "Slow to encode so the preview takes a while"));
                });
                return;
            }
            ExportFormat::Mp4 => {
                group(ui, "MP4", |ui| {
                    form_row(ui, "Background", |ui| colour_btn(ui, &mut self.video_bg).on_hover_text("MP4 has no transparency. This colour fills it."));
                });
                return;
            }
            ExportFormat::Apng => {
                hint(ui, "APNG is lossless and has no settings");
                return;
            }
            ExportFormat::Frames => {
                group(ui, "Frames", |ui| self.frames_settings_ui(ui));
                return;
            }
            ExportFormat::Sheet => {
                group(ui, "Layout", |ui| self.sheet_layout_ui(ui));
                group(ui, "PNG", |ui| Self::png_colours_ui(ui, &mut self.sheet.png));
                hint(ui, "The compressed view shows the whole sheet");
                return;
            }
            _ => {
                hint(ui, "No compression settings for this format");
                return;
            }
        }
        group(ui, "Presets", |ui| {
            let tag = self.gif_opts.noai_tag;
            let opts = [
                ("Best", GifOptions { quant_speed: 1, noai_tag: tag, ..GifOptions::default() }),
                ("Balanced", GifOptions { lossy: 30, noai_tag: tag, ..GifOptions::default() }),
                ("Small", GifOptions { lossy: 60, colors: 128, global_palette: true, noai_tag: tag, ..GifOptions::default() }),
                ("Tiny", GifOptions { lossy: 120, colors: 48, global_palette: true, noai_tag: tag, ..GifOptions::default() }),
            ];
            form_row(ui, "Start from", |ui| {
                even_grid(ui, opts.len(), 70.0, |ui, i, size| {
                    if ui.add_sized(size, sel_btn(egui::Button::new(opts[i].0), self.gif_opts == opts[i].1)).clicked() {
                        self.gif_opts = opts[i].1.clone();
                    }
                })
            });
        });
        let o = &mut self.gif_opts;
        group(ui, "Lossy", |ui| {
            slider_row(ui, "Level", egui::Slider::new(&mut o.lossy, 0..=200).clamping(egui::SliderClamping::Never)).on_hover_text("Lets similar colours merge. 30–80 is usually invisible.");
        });
        group(ui, "Colours", |ui| {
            slider_row(ui, "Maximum", egui::Slider::new(&mut o.colors, 2..=256));
            slider_row(ui, "Quantizer", egui::Slider::new(&mut o.quant_speed, 1..=30)).on_hover_text("Speed of the palette search. 1 gives the best palette and 30 is fastest.");
            form_row(ui, "", |ui| {
                check(ui, &mut o.global_palette, "One palette for all frames").on_hover_text("Smaller files but may show banding on varied footage");
            });
            dither_row(ui, o);
        });
        let drop_nth = &mut self.opt_drop_nth;
        let mut drop_go = false;
        group(ui, "Frames", |ui| {
            slider_row(ui, "Alpha cut-off", egui::Slider::new(&mut o.alpha_threshold, 1..=255)).on_hover_text("Pixels below this opacity become transparent");
            form_row(ui, "", |ui| {
                check(ui, &mut o.frame_diff, "Store only changed pixels");
                check(ui, &mut o.merge_duplicates, "Merge identical frames");
            });
            ui.add_enabled_ui(o.merge_duplicates, |ui| {
                slider_row(ui, "Near matches", egui::Slider::new(&mut o.merge_tolerance, 0..=100).custom_formatter(|v, _| if v == 0.0 { "Off".into() } else { format!("{v:.0}") })).on_hover_text("Also merges frames that barely change. Good for screen recordings and noisy video.");
            });
            form_row(ui, "Drop frames", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, drop_nth, 1.0, |d| d.range(0..=100).prefix("1 in ")).on_hover_text("Removes one frame out of every this many");
                    } else {
                        drop_go = ui.add_enabled_ui(*drop_nth >= 2, |ui| ui.add_sized(size, egui::Button::new("Apply"))).inner.on_hover_text("Removes source frames and keeps the timing").clicked();
                    }
                })
            });
        });
        if drop_go {
            let n = self.opt_drop_nth;
            self.run_anim(ctx, "Drop frames", move |a, _| Ok(ops::drop_every_nth(a, n)));
        }
    }

    fn ui_effects(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if add_layer_button(ui, "Add New Effects Layer") {
            self.add_layer(LayerKind::Effects(Box::default()));
        }
        ui.add_space(GROUP_GAP);
        let sel = self.sel_index(Kind::Effects);
        ui.horizontal(|ui| {
            hint(ui, if sel.is_some() { "Editing the selected effects layer" } else { "Changing anything adds an effects layer" });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Reset").min_size(vec2(84.0, CHIP_H))).on_hover_text("Clear every effect on this layer").clicked() {
                    match sel {
                        Some(i) => self.stack.layers[i].kind = LayerKind::Effects(Box::default()),
                        None => self.fx_draft = EffectSettings::default(),
                    }
                }
            });
        });
        ui.add_space(8.0);
        let _ = ctx;
        // the frame on screen's top-left colour, for keying out a background
        let corner = self.base.as_ref().and_then(|b| self.plan.get(self.cur).and_then(|f| b.frames.get(f.src))).map(|f| {
            let p = f.image.get().get_pixel(0, 0).0;
            [p[0], p[1], p[2]]
        });
        let Stack { transform, layers, .. } = &mut self.stack;
        let fx: &mut EffectSettings = match sel {
            Some(i) => match &mut layers[i].kind {
                LayerKind::Effects(f) => f,
                _ => &mut self.fx_draft,
            },
            None => &mut self.fx_draft,
        };
        effects_ui(ui, fx, transform, corner);
        if sel.is_none() && self.fx_draft != EffectSettings::default() {
            let fx = std::mem::take(&mut self.fx_draft);
            self.add_layer(LayerKind::Effects(Box::new(fx)));
        }
    }

    fn ui_animation(&mut self, ui: &mut egui::Ui) {
        let blank = EffectSettings { anim: true, ..Default::default() };
        if add_layer_button(ui, "Add New Animation Layer") {
            self.add_layer(LayerKind::Effects(Box::new(blank.clone())));
        }
        ui.add_space(GROUP_GAP);
        let sel = self.sel_index(Kind::Animation);
        ui.horizontal(|ui| {
            hint(ui, if sel.is_some() { "Editing the selected animation layer" } else { "Changing anything adds an animation layer" });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Reset").min_size(vec2(84.0, CHIP_H))).on_hover_text("Clear the animation on this layer").clicked() {
                    match sel {
                        Some(i) => self.stack.layers[i].kind = LayerKind::Effects(Box::new(blank.clone())),
                        None => self.anim_draft = blank.clone(),
                    }
                }
            });
        });
        ui.add_space(8.0);
        let fx: &mut EffectSettings = match sel.map(|i| &mut self.stack.layers[i].kind) {
            Some(LayerKind::Effects(f)) => f,
            _ => &mut self.anim_draft,
        };
        anim_ui(ui, fx);
        if sel.is_none() && self.anim_draft != blank {
            let fx = std::mem::replace(&mut self.anim_draft, blank);
            self.add_layer(LayerKind::Effects(Box::new(fx)));
        }
    }

    /// Angle editor shared by the placed layers: the value, then quick turns.
    fn angle_row(ui: &mut egui::Ui, angle: &mut f32) {
        form_row(ui, "Angle", |ui| {
            num_fill(ui, angle, 1.0, |d| d.speed(0.5).suffix("°").max_decimals(1)).on_hover_text("Or drag the round handle. Hold Shift to snap.");
        });
        form_row(ui, "", |ui| {
            const TURNS: [(&str, f32); 4] = [("−90°", -90.0), ("0°", 0.0), ("90°", 90.0), ("180°", 180.0)];
            even_grid(ui, 4, 0.0, |ui, i, size| {
                if ui.add_sized(size, sel_btn(egui::Button::new(TURNS[i].0), *angle == TURNS[i].1)).clicked() {
                    *angle = TURNS[i].1;
                }
            })
        });
    }

    /// Load (or reuse) a bundled or system font face, returning its index in `self.fonts`.
    fn font_index_for(&mut self, family: &str, style: &str) -> Option<usize> {
        let key = format!("{family}|{style}");
        if let Some(i) = self.font_keys.get(&key) {
            return Some(*i);
        }
        // fonts that ship with GIFkit come first, so projects look the same everywhere
        if crate::bundled::family(family).is_some() {
            let Some((entry, real_style)) = crate::bundled::load(family, style) else {
                self.error(format!("Could not load {family}"));
                return None;
            };
            self.fonts.push(entry);
            let i = self.fonts.len() - 1;
            self.font_keys.insert(key, i);
            self.font_meta.insert(i, (family.to_string(), real_style));
            return Some(i);
        }
        let face = {
            let guard = self.sys_fonts.lock().ok()?;
            let fams = guard.as_ref()?;
            let f = fams.iter().find(|f| f.name == family)?;
            f.faces.iter().find(|x| x.style == style).or_else(|| Some(f.default_face())).cloned()?
        };
        match crate::sysfonts::load(family, &face) {
            Ok(entry) => {
                self.fonts.push(entry);
                let i = self.fonts.len() - 1;
                self.font_keys.insert(key, i);
                self.font_meta.insert(i, (family.to_string(), face.style.clone()));
                Some(i)
            }
            Err(e) => {
                self.error(format!("Could not load {family}: {e:#}"));
                None
            }
        }
    }

    fn builtin_preview_font(i: usize, size: f32) -> egui::FontId {
        match i {
            0 => egui::FontId::new(size, egui::FontFamily::Name("gf-sans-bold".into())),
            1 => egui::FontId::new(size, egui::FontFamily::Name("gf-mono-bold".into())),
            _ => theme::medium(size),
        }
    }

    /// Font chooser with every name shown in its own typeface.
    fn font_picker(&mut self, ui: &mut egui::Ui, cur_idx: usize) -> Option<usize> {
        let pal = *p();
        let mut pick: Option<usize> = None;
        let mut pick_sys: Option<(String, String)> = None;
        let cur_name = self.fonts.get(cur_idx).map(|f| f.name.clone()).unwrap_or_default();
        let cur_font = match self.font_meta.get(&cur_idx).cloned() {
            Some((fam, _)) => self.preview_font(&fam, 15.0),
            None if cur_idx < self.builtin_font_count => Self::builtin_preview_font(cur_idx, 15.0),
            None => egui::FontId::proportional(15.0),
        };
        let r = ui.add(drop_button(ui, RichText::new(elide(&cur_name, 30)).font(cur_font).color(pal.fg), pal.fg).min_size(vec2(ui.available_width(), 32.0)));
        if r.clicked() {
            self.font_search.clear();
        }
        if let Some((fam, style)) = self.font_meta.get(&cur_idx).cloned() {
            let styles: Vec<String> = match crate::bundled::family(&fam) {
                Some(b) => b.faces.iter().map(|x| x.style.clone()).collect(),
                None => self.sys_fonts.lock().ok().and_then(|g| g.as_ref().and_then(|v| v.iter().find(|f| f.name == fam).map(|f| f.faces.iter().map(|x| x.style.clone()).collect()))).unwrap_or_default(),
            };
            if styles.len() > 1 {
                ui.add_space(2.0);
                form_row(ui, "Style", |ui| {
                    egui::ComboBox::from_id_salt("fontstyle").icon(chevron).width(ui.available_width()).truncate().selected_text(&style).show_ui(ui, |ui| {
                        for st in &styles {
                            if ui.selectable_label(*st == style, st).clicked() {
                                pick_sys = Some((fam.clone(), st.clone()));
                            }
                        }
                    });
                });
            }
        }
        // a popup, so it draws over the layers panel and everything else
        let list_w = r.rect.width();
        egui::Popup::from_toggle_button_response(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(list_w).show(|ui| {
            ui.set_width(list_w - 12.0);
            let q = self.font_search.to_lowercase();
            let fams: Option<Vec<String>> = self.sys_fonts.lock().ok().and_then(|g| g.as_ref().map(|v| v.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).map(|f| f.name.clone()).collect()));
            let cur_fam = self.font_meta.get(&cur_idx).map(|m| m.0.clone());
            {
                let r = ui.add(egui::TextEdit::singleline(&mut self.font_search).hint_text("Search fonts…").desired_width(f32::INFINITY));
                if self.font_search.is_empty() && !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                // fonts loaded from a file this session
                let extra: Vec<(usize, String)> = (self.builtin_font_count..self.fonts.len()).filter(|i| !self.font_meta.contains_key(i)).map(|i| (i, self.fonts[i].name.clone())).filter(|(_, n)| q.is_empty() || n.to_lowercase().contains(&q)).collect();
                for (i, n) in extra {
                    if ui.add(egui::Button::selectable(i == cur_idx, RichText::new(n).font(egui::FontId::proportional(15.0)))).clicked() {
                        pick = Some(i);
                    }
                }
                ui.separator();
                // bundled fonts by script, then the system's
                enum Row {
                    Head(String),
                    Font(String),
                    Builtin(usize),
                }
                let mut rows: Vec<Row> = Vec::new();
                let mut last_group = "";
                for f in crate::bundled::families() {
                    if !(q.is_empty() || f.family.to_lowercase().contains(&q) || f.group.to_lowercase().contains(&q)) {
                        continue;
                    }
                    if f.group != last_group {
                        rows.push(Row::Head(f.group.clone()));
                        last_group = &f.group;
                    }
                    rows.push(Row::Font(f.family.clone()));
                }
                let scanning = fams.is_none();
                if let Some(fams) = &fams {
                    let sys: Vec<&String> = fams.iter().filter(|f| crate::bundled::family(f).is_none()).collect();
                    if !sys.is_empty() {
                        rows.push(Row::Head("On This Computer".into()));
                        rows.extend(sys.into_iter().map(|f| Row::Font(f.clone())));
                    }
                }
                // the original built-in faces, last
                let builtins: Vec<usize> = (0..self.builtin_font_count).filter(|i| q.is_empty() || self.fonts[*i].name.to_lowercase().contains(&q)).collect();
                if !builtins.is_empty() {
                    rows.push(Row::Head("Built In".into()));
                    rows.extend(builtins.into_iter().map(Row::Builtin));
                }
                let row_h = 26.0;
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show_rows(ui, row_h, rows.len(), |ui, range| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for row in &rows[range] {
                            match row {
                                Row::Head(g) => {
                                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                                    ui.painter().text(r.left_center() + vec2(4.0, 2.0), egui::Align2::LEFT_CENTER, g, theme::medium(13.0), pal.fg_muted);
                                }
                                Row::Font(fam) => {
                                    let font = self.preview_font(fam, 15.0);
                                    let sel = cur_fam.as_deref() == Some(fam.as_str());
                                    let r = ui.add(egui::Button::selectable(sel, RichText::new(format!("   {}", elide(fam, 34))).font(font)).min_size(vec2(0.0, row_h)));
                                    if r.on_hover_text(fam).clicked() {
                                        pick_sys = Some((fam.clone(), String::new()));
                                    }
                                }
                                Row::Builtin(i) => {
                                    let name = self.fonts[*i].name.trim_end_matches(" (built-in)").to_string();
                                    let r = ui.add(egui::Button::selectable(*i == cur_idx, RichText::new(format!("   {name}")).font(Self::builtin_preview_font(*i, 15.0))).min_size(vec2(0.0, row_h)));
                                    if r.clicked() {
                                        pick = Some(*i);
                                    }
                                }
                            }
                        }
                    });
                });
                if scanning {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        hint(ui, "Looking for fonts on this computer…");
                    });
                }
            }
            if pick.is_some() || pick_sys.is_some() {
                ui.close();
            }
        });
        if let Some((fam, st)) = pick_sys {
            let style = if st.is_empty() && crate::bundled::family(&fam).is_some() {
                crate::bundled::family(&fam).map(|b| b.default_face().style.clone()).unwrap_or_default()
            } else if st.is_empty() {
                self.sys_fonts.lock().ok().and_then(|g| g.as_ref().and_then(|v| v.iter().find(|f| f.name == fam).map(|f| f.default_face().style.clone()))).unwrap_or_default()
            } else {
                st
            };
            pick = self.font_index_for(&fam, &style);
        }
        pick
    }

    /// The big "Add New … Layer" button, then the selected layer of this kind
    /// (picked in the layer list, so no chooser is needed here).
    fn layer_chooser(&mut self, ui: &mut egui::Ui, kind: Kind, add_label: &str, mut add: impl FnMut(&mut Self)) -> Option<usize> {
        if add_layer_button(ui, add_label) {
            add(self);
        }
        ui.add_space(GROUP_GAP);
        self.sel_index(kind)
    }

    /// Store edited settings in the selected layer, or start a layer with them
    /// when there is none yet.
    fn commit_layer(&mut self, sel: Option<usize>, changed: bool, kind: LayerKind) {
        if !changed {
            return;
        }
        match sel {
            Some(i) => {
                self.stack.layers[i].kind = kind;
                self.modified = true;
            }
            None => {
                self.add_layer(kind);
            }
        }
    }

    /// Default placement of a new text layer.
    fn place_text(&self, a: &mut Annotation) {
        let (w, h) = self.dims();
        a.x = (w as i32 / 2 - 80).max(0);
        a.y = (h as i32 - 60).max(0);
        a.angle = 0.0;
        a.from = 1;
        a.to = 0;
    }

    /// Default placement of a new censor region.
    fn place_censor(&self, c: &mut CensorSettings) {
        let (w, h) = self.dims();
        c.x = (w / 4) as i32;
        c.y = (h / 4) as i32;
        c.w = (w / 2).max(1);
        c.h = (h / 2).max(1);
        c.angle = 0.0;
        c.from = 1;
        c.to = 0;
    }

    /// Position and size rows for a box in source pixels. Y is shown the way
    /// people read a graph: 0 at the bottom, up to the box's bottom edge.
    fn box_rows(ui: &mut egui::Ui, x: &mut i32, y: &mut i32, w: &mut u32, h: &mut u32, max: u32, frame_h: u32) {
        let bh = *h as i32;
        pos_rows(ui, x, y, bh, frame_h);
        form_row(ui, "Size", |ui| {
            even_grid(ui, 2, 0.0, |ui, i, size| {
                let v = if i == 0 { &mut *w } else { &mut *h };
                spin(ui, size, v, 1.0, |d| d.range(1..=max).prefix(if i == 0 { "W  " } else { "H  " }));
            })
        });
        // a new height keeps the bottom edge where it was, as Y counts from there
        *y -= *h as i32 - bh;
    }

    fn ui_text(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let sel = self.layer_chooser(ui, Kind::Text, "Add New Text Layer", |s| {
            let mut a = s.text_tpl.clone();
            s.place_text(&mut a);
            s.add_layer(LayerKind::Text(a));
        });
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let fh = self.dims().1;
        let mut a = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Text(a)) => a.clone(),
            _ => self.text_tpl.clone(),
        };
        let before = a.clone();
        if sel.is_none() {
            hint(ui, "Changing anything adds a text layer. Drag it into place on the image.");
            ui.add_space(6.0);
        }
        group(ui, "Text", |ui| {
            // a fixed id keeps the focus when the first letter adds the layer
            ui.add(egui::TextEdit::multiline(&mut a.text).id_salt("text_body").desired_rows(2).desired_width(f32::INFINITY));
        });
        group(ui, "Typing", |ui| {
            form_row(ui, "Reveal", |ui| {
                ui.horizontal(|ui| {
                    for (r, label) in [(text::Reveal::None, "All at once"), (text::Reveal::Letters, "Letters"), (text::Reveal::Words, "Words")] {
                        if radio(ui, a.reveal == r, label).clicked() {
                            a.reveal = r;
                        }
                    }
                })
                .response
                .on_hover_text("Type the text in letter by letter or word by word, from where the layer starts")
            });
            ui.add_enabled_ui(a.reveal != text::Reveal::None, |ui| {
                slider_row(ui, "Over", soft_slider(&mut a.reveal_secs, 0.2..=10.0, "").suffix(" s").max_decimals(1)).on_hover_text("How long typing it all takes");
            });
        });
        group(ui, "Layout", |ui| {
            use text::Caption;
            form_row(ui, "Caption bar", |ui| seg_row(ui, &mut a.caption, &[(Caption::None, "None"), (Caption::Top, "Top"), (Caption::Bottom, "Bottom")]))
                .on_hover_text("A strip above or below the picture with the text in it, sized to fit the text");
            let bar = a.caption != Caption::None;
            if bar {
                form_row(ui, "Bar colour", |ui| colour_btn(ui, &mut a.bar_color));
                slider_row(ui, "Bar padding", soft_slider(&mut a.bar_pad, 0.0..=150.0, "").suffix("%").max_decimals(0)).on_hover_text("Room above and below the text, as a share of the text size");
            }
            ui.add_enabled_ui(!bar, |ui| {
                let (mut wrap, mut centre) = (a.wrap || bar, a.centre || bar);
                form_row(ui, "Fit", |ui| {
                    check(ui, &mut wrap, "Wrap at the canvas edge");
                    check(ui, &mut centre, "Centre across the canvas");
                });
                if !bar {
                    (a.wrap, a.centre) = (wrap, centre);
                }
            });
        });
        let mut picked = None;
        let mut load_file = false;
        group(ui, "Font", |ui| {
            picked = self.font_picker(ui, a.font_idx);
            ui.add_space(2.0);
            load_file = wide_button(ui, egui::Button::new("Load Font File…")).clicked();
        });
        if load_file {
            if let Some(path) = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf", "ttc"]).pick_file() {
                match text::load_font_file(&path) {
                    Ok(f) => {
                        self.fonts.push(f);
                        picked = Some(self.fonts.len() - 1);
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
        }
        if let Some(fi) = picked {
            // keep the text centred on the same spot in the new font
            let (w0, h0) = text::measure(&self.fonts, &a);
            let (cx, cy) = (a.x as f32 + w0 as f32 / 2.0, a.y as f32 + h0 as f32 / 2.0);
            a.font_idx = fi;
            if sel.is_some() {
                let (w1, h1) = text::measure(&self.fonts, &a);
                a.x = (cx - w1 as f32 / 2.0).round() as i32;
                a.y = (cy - h1 as f32 / 2.0).round() as i32;
            }
        }
        group(ui, "Style", |ui| {
            form_row(ui, "Size", |ui| {
                ui.horizontal(|ui| {
                    colour_btn_a(ui, &mut a.color).on_hover_text("Text colour");
                    num_fill(ui, &mut a.size, 1.0, |d| d.range(4.0..=4000.0).suffix(" px"));
                })
            });
            form_row(ui, "Outline", |ui| {
                ui.horizontal(|ui| {
                    colour_btn_a(ui, &mut a.border_color).on_hover_text("Outline colour");
                    let mut w = a.outline_px() as f32;
                    if num_fill(ui, &mut w, 1.0, |d| d.range(0.0..=200.0).speed(0.2).suffix(" px")).on_hover_text("0 turns the outline off").changed() {
                        a.border_width = Some(w.round());
                    }
                })
            });
            form_row(ui, "Scale", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    let v = if i == 0 { &mut a.stretch } else { &mut a.stretch_y };
                    let mut pct = *v * 100.0;
                    let tip = if i == 0 { "Squash or stretch the text sideways. Drag a side handle on the image to do it by hand." } else { "Squash or stretch the text up and down" };
                    if spin(ui, size, &mut pct, 1.0, |d| d.range(5.0..=2000.0).speed(0.5).max_decimals(0).prefix(if i == 0 { "X  " } else { "Y  " }).suffix("%")).on_hover_text(tip).changed() {
                        *v = pct / 100.0;
                    }
                })
            });
            let scaled = (a.stretch - 1.0).abs() > 1e-3 || (a.stretch_y - 1.0).abs() > 1e-3;
            form_row(ui, "", |ui| {
                if ui.add_enabled_ui(scaled, |ui| wide_button(ui, egui::Button::new("Reset Scale"))).inner.clicked() {
                    (a.stretch, a.stretch_y) = (1.0, 1.0);
                }
            });
            form_row(ui, "Align", |ui| seg_row(ui, &mut a.align, &[(Align::Left, "Left"), (Align::Center, "Centre"), (Align::Right, "Right")]));
            slider_row(ui, "Opacity", egui::Slider::new(&mut a.opacity, 0..=100).suffix("%"));
            form_row(ui, "Colours", |ui| {
                let r = wide_button(ui, drop_button(ui, "Quick Colours", p().fg));
                egui::Popup::menu(&r).show(|ui| {
                    ui.set_min_width(170.0);
                    let presets: [(&str, [u8; 4], [u8; 4]); 5] = [
                        ("White / black", [255, 255, 255, 255], [0, 0, 0, 255]),
                        ("Black / white", [0, 0, 0, 255], [255, 255, 255, 255]),
                        ("Yellow / black", [255, 230, 0, 255], [0, 0, 0, 255]),
                        ("Red / white", [230, 30, 40, 255], [255, 255, 255, 255]),
                        ("Cyan / navy", [80, 230, 255, 255], [10, 20, 70, 255]),
                    ];
                    for (name, c, b) in presets {
                        if ui.button(name).clicked() {
                            a.color = c;
                            a.border_color = b;
                        }
                    }
                });
            });
        });
        let placed = sel.is_some();
        ui.add_enabled_ui(placed, |ui| {
            group(ui, "Placement", |ui| {
                let th = text::measure(&self.fonts, &a).1 as i32;
                pos_rows(ui, &mut a.x, &mut a.y, th, fh);
                Self::angle_row(ui, &mut a.angle);
                frames_row(ui, &mut a.from, &mut a.to, n);
            });
        })
        .response
        .on_disabled_hover_text("Add a text layer first");
        if sel.is_none() {
            (a.x, a.y, a.angle, a.from, a.to) = (before.x, before.y, before.angle, before.from, before.to);
        }
        let changed = a != before;
        if sel.is_none() && changed {
            self.place_text(&mut a);
        }
        // the next text takes this style, but not its caption bar
        self.text_tpl = Annotation { text: self.text_tpl.text.clone(), x: 0, y: 0, angle: 0.0, from: 1, to: 0, caption: text::Caption::None, bar_h: 0, ..a.clone() };
        self.commit_layer(sel, changed, LayerKind::Text(a));
    }

    fn ui_censor(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let (w, h) = self.dims();
        let sel = self.layer_chooser(ui, Kind::Censor, "Add New Censor Layer", |s| {
            let mut c = s.censor_tpl.clone();
            s.place_censor(&mut c);
            s.add_layer(LayerKind::Censor(c));
        });
        let mut c = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Censor(c)) => c.clone(),
            _ => self.censor_tpl.clone(),
        };
        let before = c.clone();
        if sel.is_none() {
            ui.add_space(6.0);
        }
        group(ui, "Method", |ui| {
            form_row(ui, "Method", |ui| seg_row(ui, &mut c.method, &[(CensorMethod::Pixelate, "Pixelate"), (CensorMethod::Blur, "Blur"), (CensorMethod::Solid, "Solid Colour")]));
            ui.add_space(4.0);
            match c.method {
                CensorMethod::Pixelate => {
                    form_row(ui, "Block size", |ui| num_fill(ui, &mut c.square, 1.0, |d| d.range(2..=1000).suffix(" px")));
                }
                CensorMethod::Blur => {
                    slider_row(ui, "Radius", soft_slider(&mut c.sigma, 1.0..=40.0, "").max_decimals(1));
                }
                CensorMethod::Solid => {
                    form_row(ui, "Colour", |ui| colour_btn(ui, &mut c.color));
                }
            }
        });
        ui.add_enabled_ui(sel.is_some(), |ui| {
            group(ui, "Placement", |ui| {
                Self::box_rows(ui, &mut c.x, &mut c.y, &mut c.w, &mut c.h, (w.max(h) * 4).max(1), h);
                Self::angle_row(ui, &mut c.angle);
                frames_row(ui, &mut c.from, &mut c.to, n);
            });
        })
        .response
        .on_disabled_hover_text("Draw a region on the image first");
        if sel.is_none() {
            // the greyed-out placement boxes may clamp the stand-in values; that's no edit
            (c.x, c.y, c.w, c.h, c.angle, c.from, c.to) = (before.x, before.y, before.w, before.h, before.angle, before.from, before.to);
        }
        let changed = c != before;
        if sel.is_none() && changed {
            self.place_censor(&mut c);
        }
        self.censor_tpl = CensorSettings { x: 0, y: 0, w: 0, h: 0, angle: 0.0, from: 1, to: 0, ..c.clone() };
        self.commit_layer(sel, changed, LayerKind::Censor(c));
    }

    fn ui_add_image(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let sel = self.layer_chooser(ui, Kind::Overlay, "Add New Image Layer…", |s| {
            if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "gif", "jpg", "jpeg", "webp", "bmp"]).pick_file() {
                s.load_overlay(path, ctx);
            }
        });
        let (aw, ah) = self.dims();
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let mut scope = self.place_scope;
        // with no image layer the settings show greyed out with stand-in values
        let mut dummy: Option<OverlayLayer> = None;
        let o: &mut OverlayLayer = match sel.map(|i| &mut self.stack.layers[i].kind) {
            Some(LayerKind::Overlay(o)) => o,
            _ => {
                ui.add_space(6.0);
                let src = OverlaySource { name: String::new(), path: None, delays: vec![10], frames: vec![RgbaImage::new(1, 1)] };
                dummy = Some(OverlayLayer { src: Arc::new(src), placements: vec![Placement { enabled: true, x: 0, y: 0, w: 1, h: 1, angle: 0.0 }], opacity: 100.0 });
                dummy.as_mut().unwrap()
            }
        };
        let real = sel.is_some();
        let native = o.src.native_size();
        let nframes = o.src.frames.len();
        let cur = src_idx.min(o.placements.len().saturating_sub(1));
        ui.add_enabled_ui(real, |ui| {
            group(ui, "Image", |ui| {
                form_row(ui, "Source", |ui| cell(ui, vec2(ui.available_width(), CHIP_H), |ui| hint(ui, &if real { format!("{}×{} · {} frame{}", native.0, native.1, nframes, if nframes == 1 { "" } else { "s" }) } else { "None".into() })));
                slider_row(ui, "Opacity", egui::Slider::new(&mut o.opacity, 0.0..=100.0).suffix("%").max_decimals(0));
            });
            if o.placements.is_empty() {
                return;
            }
            group(ui, "Placement", |ui| {
                form_row(ui, "Apply to", |ui| combo(ui, "pscope", &mut scope, &[(PlaceScope::All, PlaceScope::All.label()), (PlaceScope::Following, PlaceScope::Following.label()), (PlaceScope::This, PlaceScope::This.label())]));
                let mut pl = o.placements[cur];
                Self::box_rows(ui, &mut pl.x, &mut pl.y, &mut pl.w, &mut pl.h, 16384, ah);
                Self::angle_row(ui, &mut pl.angle);
                form_row(ui, "Fit", |ui| {
                    even_grid(ui, 3, 0.0, |ui, i, size| {
                        let (label, tip) = [("Native", "Original image size"), ("Centre", "Centre on the frame"), ("Fill", "Cover the whole frame")][i];
                        if ui.add_sized(size, egui::Button::new(label)).on_hover_text(tip).clicked() {
                            match i {
                                0 => {
                                    pl.w = native.0;
                                    pl.h = native.1;
                                }
                                1 => {
                                    pl.x = (aw as i32 - pl.w as i32) / 2;
                                    pl.y = (ah as i32 - pl.h as i32) / 2;
                                }
                                _ => {
                                    pl.x = 0;
                                    pl.y = 0;
                                    pl.w = aw;
                                    pl.h = ah;
                                }
                            }
                        }
                    })
                });
                let mut hidden = !pl.enabled;
                form_row(ui, "Visibility", |ui| {
                    check(ui, &mut hidden, format!("Hide on frame {}", cur + 1));
                });
                pl.enabled = !hidden;
                if pl != o.placements[cur] {
                    let n = o.placements.len();
                    let enabled_changed = pl.enabled != o.placements[cur].enabled;
                    for q in &mut o.placements[scope.range(cur, n)] {
                        q.x = pl.x;
                        q.y = pl.y;
                        q.w = pl.w;
                        q.h = pl.h;
                        q.angle = pl.angle;
                    }
                    if enabled_changed {
                        o.placements[cur].enabled = pl.enabled;
                    }
                }
                form_row(ui, "", |ui| {
                    if wide_button(ui, egui::Button::new("Show on All Frames")).clicked() {
                        for q in &mut o.placements {
                            q.enabled = true;
                        }
                    }
                });
            });
        })
        .response
        .on_disabled_hover_text("Add an image layer first");
        drop(dummy);
        self.place_scope = scope;
    }

    fn ui_watermark(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let sel = self.layer_chooser(ui, Kind::Watermark, "Add New Watermark Layer", |s| {
            let w = s.wm_tpl.clone();
            s.add_layer(LayerKind::Watermark(Box::new(w)));
        });
        let mut w = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Watermark(w)) => (**w).clone(),
            _ => self.wm_tpl.clone(),
        };
        let before = w.clone();
        let mut browse = false;
        group(ui, "Content", |ui| {
            form_row(ui, "Show", |ui| seg_row(ui, &mut w.use_image, &[(false, "Text"), (true, "Image")]));
            if w.use_image {
                form_row(ui, "Image", |ui| {
                    browse = wide_button(ui, egui::Button::new("Choose Image…")).clicked();
                });
                let name = w.image_path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "None chosen".into());
                form_row(ui, "", |ui| hint(ui, &elide(&name, 30)));
            } else {
                form_row(ui, "Text", |ui| ui.add(egui::TextEdit::singleline(&mut w.text).desired_width(ui.available_width())));
                form_row(ui, "Colour", |ui| colour_btn_a(ui, &mut w.color));
                opt_row(ui, "Outline", &mut w.outline, |ui| {
                    let mut px = w.outline_px.unwrap_or(0.0);
                    if num_fill(ui, &mut px, 1.0, |d| d.range(0.0..=100.0).speed(0.2).suffix(" px")).on_hover_text("0 picks a width from the text size").changed() {
                        w.outline_px = if px <= 0.0 { None } else { Some(px.round()) };
                    }
                });
            }
        });
        let mut picked = None;
        if !w.use_image {
            group(ui, "Font", |ui| {
                picked = self.font_picker(ui, w.font_idx);
            });
        }
        if browse {
            if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_file() {
                match io::load_still(&path) {
                    Ok(img) => {
                        w.image = Some(Arc::new(img));
                        w.image_path = Some(path);
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
        }
        if let Some(fi) = picked {
            w.font_idx = fi;
        }
        group(ui, "Placement", |ui| {
            slider_row(ui, "Size", soft_slider(&mut w.size, 1.0..=40.0, "").suffix(" %").max_decimals(1)).on_hover_text("Size as a share of the frame's shorter side");
            slider_row(ui, "Opacity", egui::Slider::new(&mut w.opacity, 0.0..=100.0).suffix("%").max_decimals(0));
            form_row(ui, "Layout", |ui| seg_row(ui, &mut w.tiled, &[(false, "Single"), (true, "Tiled")]));
            if w.tiled {
                slider_row(ui, "Angle", soft_slider(&mut w.angle, -90.0..=90.0, "").suffix("°").max_decimals(0));
                slider_row(ui, "Spacing", soft_slider(&mut w.spacing, 0.8..=4.0, "").max_decimals(2));
            } else {
                use crate::effects::Anchor;
                form_row(ui, "Position", |ui| {
                    combo(ui, "wmanchor", &mut w.anchor, &[
                        (Anchor::TopLeft, "Top left"),
                        (Anchor::Top, "Top"),
                        (Anchor::TopRight, "Top right"),
                        (Anchor::Left, "Left"),
                        (Anchor::Center, "Centre"),
                        (Anchor::Right, "Right"),
                        (Anchor::BottomLeft, "Bottom left"),
                        (Anchor::Bottom, "Bottom"),
                        (Anchor::BottomRight, "Bottom right"),
                    ])
                });
                form_row(ui, "Margin", |ui| num_fill(ui, &mut w.margin, 1.0, |d| d.range(0.0..=40.0).speed(0.1).max_decimals(1).suffix(" %")).on_hover_text("Gap to the edge as a share of the frame's shorter side"));
            }
        });
        let changed = w != before;
        self.wm_tpl = w.clone();
        self.commit_layer(sel, changed, LayerKind::Watermark(Box::new(w)));
    }

    /// A new shape from the current style, centred on the frame.
    fn default_shape(&self, w: u32, h: u32) -> ShapeSettings {
        let mut s = self.shape_tpl.clone();
        let side = (w.min(h) as f32 * 0.4).max(8.0) as u32;
        let (bw, bh) = match s.kind {
            ShapeKind::Line => ((w as f32 * 0.5) as u32, ((s.stroke_w * 3.0) as u32).max(12)),
            ShapeKind::Arrow => ((w as f32 * 0.5) as u32, (side / 3).max(16)),
            ShapeKind::Speech | ShapeKind::Bubble | ShapeKind::Thought => (side * 3 / 2, side),
            _ => (side, side),
        };
        s.w = bw.max(4);
        s.h = bh.max(4);
        s.x = (w as i32 - s.w as i32) / 2;
        s.y = (h as i32 - s.h as i32) / 2;
        s.angle = 0.0;
        s.from = 1;
        s.to = 0;
        s
    }

    fn emoji_texture(&mut self, ctx: &egui::Context, code: &str) -> Option<TextureHandle> {
        if let Some(t) = self.emoji_tex.get(code) {
            return Some(t.clone());
        }
        let img = crate::emoji::render(code, 64)?;
        let t = ctx.load_texture(format!("emoji-{code}"), to_color_image(&img), TextureOptions::LINEAR);
        self.emoji_tex.insert(code.to_string(), t.clone());
        Some(t)
    }

    /// Colour button followed by as many quick swatches as fit the row.
    fn swatches(ui: &mut egui::Ui, c: &mut [u8; 3]) {
        const SW: [[u8; 3]; 9] = [[255, 255, 255], [34, 38, 44], [255, 59, 48], [255, 122, 26], [255, 204, 0], [52, 199, 89], [61, 174, 233], [175, 82, 222], [255, 105, 180]];
        ui.horizontal(|ui| {
            colour_btn(ui, c);
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing.x = 3.0;
            let fit = (((ui.available_width() + 3.0) / 19.0).floor() as usize).min(SW.len());
            for s in &SW[..fit] {
                let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                let col = Color32::from_rgb(s[0], s[1], s[2]);
                ui.painter().rect_filled(r, 3.0, col);
                let edge = if *c == *s { p().highlight } else { theme::outline(p()) };
                ui.painter().rect_stroke(r, 3.0, Stroke::new(if *c == *s { 2.0 } else { 1.0 }, edge), egui::StrokeKind::Inside);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *c = *s;
                }
            }
        });
    }

    /// Labelled colour row with quick swatches.
    fn colour_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
        form_row(ui, label, |ui| Self::swatches(ui, c));
    }

    fn ui_shapes(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let (w, h) = self.dims();
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let sel = self.layer_chooser(ui, Kind::Shape, "Add New Shape Layer", |s| {
            let sh = s.default_shape(w, h);
            s.add_layer(LayerKind::Shape(Box::new(sh)));
        });
        if sel.is_none() {
            ui.add_space(6.0);
        }
        // edit the selected shape, or the style of the next one
        let mut st = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Shape(s)) => (**s).clone(),
            _ => self.shape_tpl.clone(),
        };
        let before = st.clone();
        group(ui, "Shape", |ui| {
            let kinds: Vec<ShapeKind> = ShapeKind::ALL.iter().copied().filter(|k| *k != ShapeKind::Emoji || st.kind == ShapeKind::Emoji).collect();
            let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
            even_grid(ui, kinds.len(), label_w(ui, &labels), |ui, i, size| {
                let k = kinds[i];
                if ui.add_sized(size, sel_btn(egui::Button::new(k.label()).truncate(), st.kind == k)).clicked() {
                    st.kind = k;
                }
            });
            if st.kind.is_linear() {
                ui.add_space(2.0);
                hint(ui, "Drag from start to end. Shift keeps it straight.");
            }
        });
        if st.kind == ShapeKind::Emoji {
            hint(ui, "Emoji have their own tool now");
        } else {
            group(ui, "Style", |ui| {
                if st.kind == ShapeKind::Line {
                    Self::colour_row(ui, "Line", &mut st.stroke);
                } else {
                    opt_row(ui, "Fill", &mut st.fill_on, |ui| Self::swatches(ui, &mut st.fill));
                    opt_row(ui, "Outline", &mut st.stroke_on, |ui| Self::swatches(ui, &mut st.stroke));
                }
                let lined = st.stroke_on || st.kind == ShapeKind::Line;
                ui.add_enabled_ui(lined, |ui| slider_row(ui, "Width", egui::Slider::new(&mut st.stroke_w, 1.0..=60.0).suffix(" px").max_decimals(1).clamping(egui::SliderClamping::Never)));
                match st.kind {
                    ShapeKind::Rounded | ShapeKind::Speech => {
                        slider_row(ui, "Corners", egui::Slider::new(&mut st.corner, 0.0..=50.0).suffix("%").max_decimals(0));
                    }
                    ShapeKind::Star | ShapeKind::Burst => {
                        slider_row(ui, "Points", egui::Slider::new(&mut st.points, 3..=24));
                    }
                    _ => {}
                }
            });
        }
        group(ui, "Look", |ui| {
            slider_row(ui, "Opacity", egui::Slider::new(&mut st.opacity, 0.0..=100.0).suffix("%").max_decimals(0));
            shadow_rows(ui, &mut st.shadow, &mut st.shadow_style);
        });
        ui.add_enabled_ui(sel.is_some(), |ui| {
            group(ui, "Placement", |ui| {
                Self::box_rows(ui, &mut st.x, &mut st.y, &mut st.w, &mut st.h, (w.max(h) * 4).max(1), h);
                Self::angle_row(ui, &mut st.angle);
                frames_row(ui, &mut st.from, &mut st.to, n);
            });
        })
        .response
        .on_disabled_hover_text("Draw a shape on the image first");
        if sel.is_none() {
            (st.x, st.y, st.w, st.h, st.angle, st.from, st.to) = (before.x, before.y, before.w, before.h, before.angle, before.from, before.to);
        }
        if let Some(i) = sel {
            if st != before {
                if let LayerKind::Shape(s) = &mut self.stack.layers[i].kind {
                    **s = st.clone();
                }
                self.modified = true;
            }
        }
        // the next shape uses the latest style
        let geo = (self.shape_tpl.x, self.shape_tpl.y, self.shape_tpl.w, self.shape_tpl.h, self.shape_tpl.angle, self.shape_tpl.from, self.shape_tpl.to);
        self.shape_tpl = ShapeSettings { x: geo.0, y: geo.1, w: geo.2, h: geo.3, angle: geo.4, from: geo.5, to: geo.6, ..st };
    }

    /// Follow an emoji dragged from the picker, and drop it where it lands on a view.
    fn emoji_drag_drop(&mut self, ctx: &egui::Context) {
        let Some(f) = self.emoji_drag.clone() else { return };
        let pos = ctx.input(|i| i.pointer.hover_pos());
        if let (Some(p), Some(t)) = (pos, self.emoji_texture(ctx, &f)) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("emoji-drag")));
            let r = Rect::from_center_size(p, vec2(48.0, 48.0));
            painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::from_white_alpha(220));
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if ctx.input(|i| i.pointer.primary_down()) {
            ctx.request_repaint();
            return;
        }
        self.emoji_drag = None;
        let Some(p) = pos else { return };
        let Some(&(_, m)) = self.drop_views.iter().find(|(r, _)| r.contains(p)) else { return };
        if self.busy() || self.base.is_none() {
            return;
        }
        let now = ctx.input(|i| i.time);
        self.snapshot(&format!("emoji-drop{now}"), now);
        self.emoji_pick = f;
        self.note_recent_emoji();
        let (w, h) = self.dims();
        let id = self.add_layer(LayerKind::Emoji(Box::new(self.default_emoji(w, h, None))));
        // the drop point is in output pixels: take it back to the source frame
        let Some(li) = self.stack.index_of(id) else { return };
        let maps = self.stack.layer_maps(w, h);
        let o = m.to_img(p);
        let (cx, cy) = maps[li].m.inverse().apply(o.x, o.y);
        if let LayerKind::Emoji(e) = &mut self.stack.layers[li].kind {
            e.x = (cx - e.w as f32 / 2.0).round() as i32;
            e.y = (cy - e.h as f32 / 2.0).round() as i32;
        }
        self.tool = Tool::Emoji;
    }

    /// A new emoji layer with the current pick, square, centred on `at` (source px) or the frame.
    fn default_emoji(&self, w: u32, h: u32, at: Option<(f32, f32)>) -> EmojiSettings {
        let side = ((w.min(h) as f32) * 0.3).max(16.0) as u32;
        let (cx, cy) = at.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        EmojiSettings { emoji: self.emoji_pick.clone(), x: (cx - side as f32 / 2.0).round() as i32, y: (cy - side as f32 / 2.0).round() as i32, w: side, h: side, opacity: self.emoji_tpl.opacity, shadow: self.emoji_tpl.shadow, shadow_style: self.emoji_tpl.shadow_style, ..Default::default() }
    }

    /// Put the current pick at the front of the recent list (saved).
    fn note_recent_emoji(&mut self) {
        let f = self.emoji_pick.clone();
        self.emoji_recent.retain(|x| *x != f);
        self.emoji_recent.insert(0, f);
        self.emoji_recent.truncate(32);
        theme::set_setting("recent_emoji", &self.emoji_recent.join(","));
    }

    fn ui_emoji(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let (w, h) = self.dims();
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let sel = self.layer_chooser(ui, Kind::Emoji, "Add New Emoji Layer", |s| {
            let e = s.default_emoji(w, h, None);
            s.add_layer(LayerKind::Emoji(Box::new(e)));
        });
        let full = ui.available_width();
        let mut picked: Option<String> = None;
        group(ui, "Pick", |ui| {
            let inner = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut self.emoji_search).hint_text("Search emoji").desired_width(inner));
            let searching = !self.emoji_search.trim().is_empty();
            if !searching {
                let mut tabs: Vec<(EmojiTab, &str, &str)> = Vec::new();
                if !self.emoji_recent.is_empty() {
                    tabs.push((EmojiTab::Recent, "Recent", "Recently used"));
                }
                for (g, name) in crate::emoji::GROUP_SHORT.iter().enumerate() {
                    tabs.push((EmojiTab::Group(g as u8), name, crate::emoji::GROUPS[g]));
                }
                ui.add_space(2.0);
                let labels: Vec<&str> = tabs.iter().map(|t| t.1).collect();
                even_grid(ui, tabs.len(), label_w(ui, &labels), |ui, i, size| {
                    let (t, label, tip) = tabs[i];
                    if ui.add_sized(size, sel_btn(egui::Button::new(label).truncate(), self.emoji_tab == t)).on_hover_text(tip).clicked() {
                        self.emoji_tab = t;
                    }
                });
            }
            ui.add_space(2.0);
            // skin tone
            form_row(ui, "Skin tone", |ui| ui.horizontal(|ui| {
                const TONE_COL: [[u8; 3]; 6] = [[255, 204, 77], [247, 222, 206], [243, 210, 162], [213, 171, 136], [175, 126, 87], [124, 83, 62]];
                for (t, c) in TONE_COL.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 8.0, Color32::from_rgb(c[0], c[1], c[2]));
                    if self.emoji_tone == t {
                        ui.painter().circle_stroke(r.center(), 9.5, Stroke::new(2.0, pal.highlight));
                    }
                    if resp.on_hover_text(crate::emoji::TONES[t]).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.emoji_tone = t;
                        theme::set_setting("emoji_tone", &t.to_string());
                    }
                }
            }));
            // the list to show: files with the skin tone applied
            let files: Vec<String> = if searching {
                crate::emoji::search(&self.emoji_search, None).into_iter().map(|i| crate::emoji::all()[i].file(self.emoji_tone).to_string()).collect()
            } else {
                match self.emoji_tab {
                    EmojiTab::Recent => self.emoji_recent.clone(),
                    EmojiTab::Group(g) => crate::emoji::search("", Some(g)).into_iter().map(|i| crate::emoji::all()[i].file(self.emoji_tone).to_string()).collect(),
                }
            };
            if searching {
                hint(ui, &format!("{} found", files.len()));
            }
            // leave room for the frame, its margin and the scroll bar so the panel never grows,
            // then stretch the cells to fill the row
            let room = inner - 22.0;
            let cols = ((room + 3.0) / (40.0 + 3.0)).floor().max(1.0) as usize;
            let cell_s = ((room + 3.0) / cols as f32 - 3.0).floor();
            let icon_s = (cell_s - 6.0).min(40.0);
            let rows = files.len().div_ceil(cols);
            let mut budget = 48; // new textures per frame, so the picker never stalls
            egui::Frame::new().fill(pal.view).stroke(Stroke::new(1.0, pal.border)).corner_radius(theme::RADIUS).inner_margin(3.0).show(ui, |ui| {
                ui.set_width(inner - 8.0);
                egui::ScrollArea::vertical().id_salt(("emoji-grid", self.emoji_tab == EmojiTab::Recent, searching)).max_height(300.0).auto_shrink([false, true]).show_rows(ui, cell_s + 3.0, rows, |ui, range| {
                    ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                    ui.spacing_mut().button_padding = vec2(2.0, 2.0);
                    for r in range {
                        ui.horizontal(|ui| {
                            for f in files.iter().skip(r * cols).take(cols) {
                                let tex = if self.emoji_tex.contains_key(f) || budget > 0 {
                                    if !self.emoji_tex.contains_key(f) {
                                        budget -= 1;
                                    }
                                    self.emoji_texture(ctx, f)
                                } else {
                                    None
                                };
                                let on = *f == self.emoji_pick;
                                let b = match &tex {
                                    Some(t) => egui::Button::image(egui::Image::new(t).fit_to_exact_size(vec2(icon_s, icon_s))),
                                    None => egui::Button::new(""),
                                };
                                let resp = ui.add(sel_btn(b.frame_when_inactive(on), on).min_size(vec2(cell_s, cell_s))).interact(Sense::drag()).on_hover_text(format!("{}. Click to add, or drag onto the picture.", crate::emoji::name_of(f)));
                                if resp.clicked() {
                                    picked = Some(f.clone());
                                }
                                if resp.drag_started() {
                                    self.emoji_drag = Some(f.clone());
                                }
                            }
                        });
                    }
                });
            });
            if budget == 0 {
                ctx.request_repaint();
            }
            if files.is_empty() {
                hint(ui, if searching { "Nothing matches. Try another word." } else { "Nothing here yet" });
            }
            ui.add_space(4.0);
            ui.add_enabled_ui(sel.is_some(), |ui| {
                form_row(ui, "Clicking", |ui| seg_row(ui, &mut self.emoji_replace, &[(false, "Adds a layer"), (true, "Swaps selected")]));
            })
            .response
            .on_disabled_hover_text("Clicking an emoji adds a new layer");
        });
        let _ = full;
        if let Some(f) = picked {
            self.emoji_pick = f.clone();
            self.note_recent_emoji();
            let now = ui.input(|i| i.time);
            self.snapshot(&format!("emoji{now}"), now);
            match sel {
                Some(i) if self.emoji_replace => {
                    if let LayerKind::Emoji(e) = &mut self.stack.layers[i].kind {
                        e.emoji = f;
                    }
                    self.modified = true;
                }
                _ => {
                    let e = self.default_emoji(w, h, None);
                    self.add_layer(LayerKind::Emoji(Box::new(e)));
                }
            }
        }
        let sel = self.sel_index(Kind::Emoji);
        let mut e = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Emoji(e)) => (**e).clone(),
            _ => self.emoji_tpl.clone(),
        };
        let before = e.clone();
        group(ui, "Look", |ui| {
            slider_row(ui, "Opacity", egui::Slider::new(&mut e.opacity, 0.0..=100.0).suffix("%").max_decimals(0));
            shadow_rows(ui, &mut e.shadow, &mut e.shadow_style);
        });
        ui.add_enabled_ui(sel.is_some(), |ui| {
            group(ui, "Placement", |ui| {
                let eh = e.h as i32;
                pos_rows(ui, &mut e.x, &mut e.y, eh, h);
                form_row(ui, "Size", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let v = if i == 0 { &mut e.w } else { &mut e.h };
                        spin(ui, size, v, 1.0, |d| d.range(4..=(w.max(h) * 4).max(4)).prefix(if i == 0 { "W  " } else { "H  " }).suffix(" px"));
                    })
                });
                e.y -= e.h as i32 - eh;
                Self::angle_row(ui, &mut e.angle);
                frames_row(ui, &mut e.from, &mut e.to, n);
            });
        })
        .response
        .on_disabled_hover_text("Click an emoji or drag on the image first");
        if sel.is_none() {
            (e.x, e.y, e.w, e.h, e.angle, e.from, e.to) = (before.x, before.y, before.w, before.h, before.angle, before.from, before.to);
        }
        self.emoji_tpl.opacity = e.opacity;
        self.emoji_tpl.shadow = e.shadow;
        self.emoji_tpl.shadow_style = e.shadow_style;
        if let Some(i) = sel {
            if e != before {
                self.stack.layers[i].kind = LayerKind::Emoji(Box::new(e));
                self.modified = true;
            }
        }
    }

    fn ui_markup(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let sel = self.layer_chooser(ui, Kind::Markup, "Add New Markup Layer", |s| {
            s.add_layer(LayerKind::Markup(MarkupSettings::default()));
        });
        if sel.is_none() {
            hint(ui, "Drag on the image to draw. A markup layer starts with the first stroke.");
            ui.add_space(6.0);
        }
        group(ui, "Brush", |ui| {
            form_row(ui, "Tool", |ui| seg_row(ui, &mut self.pen_tool, &[(PenTool::Pen, "Pen"), (PenTool::Highlighter, "Highlighter"), (PenTool::Eraser, "Eraser")])).on_hover_text("The eraser removes whole strokes you drag over");
            ui.add_space(4.0);
            let eraser = self.pen_tool == PenTool::Eraser;
            let b = match self.pen_tool {
                PenTool::Highlighter => &mut self.highlighter,
                _ => &mut self.pen,
            };
            ui.add_enabled_ui(!eraser, |ui| Self::colour_row(ui, "Colour", &mut b.color));
            slider_row(ui, "Size", egui::Slider::new(&mut b.width, 1.0..=80.0).suffix(" px").max_decimals(1).clamping(egui::SliderClamping::Never));
            ui.add_enabled_ui(!eraser, |ui| slider_row(ui, "Opacity", egui::Slider::new(&mut b.opacity, 5.0..=100.0).suffix("%").max_decimals(0)));
            form_row(ui, "", |ui| hint(ui, "Hold Shift for a straight line"));
        });
        group(ui, "Draw On", |ui| {
            form_row(ui, "Draw on", |ui| seg_row(ui, &mut self.pen_scope, &[(PenScope::All, "All frames"), (PenScope::FromHere, "From here"), (PenScope::ThisFrame, "This frame")])).on_hover_text("This frame lets you step through frames to draw a little animation");
        });
        let count = sel
            .map(|i| match &self.stack.layers[i].kind {
                LayerKind::Markup(m) => m.strokes.len(),
                _ => 0,
            })
            .unwrap_or(0);
        let mut undo = false;
        let mut clear = false;
        ui.add_enabled_ui(sel.is_some(), |ui| {
            group(ui, "Strokes", |ui| {
                form_row(ui, "On layer", |ui| cell(ui, vec2(ui.available_width(), CHIP_H), |ui| hint(ui, &format!("{count} stroke{}", if count == 1 { "" } else { "s" }))));
                form_row(ui, "", |ui| {
                    even_grid(ui, 2, 0.0, |ui, i, size| {
                        let b = egui::Button::new(if i == 0 { "Remove Last" } else { "Clear All" }).truncate();
                        let r = ui.add_enabled_ui(count > 0, |ui| ui.add_sized(size, b)).inner;
                        if r.clicked() {
                            if i == 0 {
                                undo = true;
                            } else {
                                clear = true;
                            }
                        }
                    })
                });
            });
        })
        .response
        .on_disabled_hover_text("Draw on the image first");
        if let Some(i) = sel {
            if let LayerKind::Markup(m) = &mut self.stack.layers[i].kind {
                if undo {
                    m.strokes.pop();
                }
                if clear {
                    m.strokes.clear();
                }
            }
        }
    }

    fn ui_shield(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let sel = self.layer_chooser(ui, Kind::Shield, "Add New AI Shield Layer", |s| {
            let sh = s.shield_tpl.clone();
            s.add_layer(LayerKind::Shield(sh));
        });
        ui.add_space(6.0);
        let mut s = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Shield(s)) => s.clone(),
            _ => self.shield_tpl.clone(),
        };
        let before = s.clone();
        group(ui, "Perturbation", |ui| {
            slider_row(ui, "Strength", soft_slider(&mut s.strength, 0.0..=20.0, "").max_decimals(1)).on_hover_text("How strong the noise is. 4 to 8 is hard to see.");
            form_row(ui, "Adds", |ui| {
                check(ui, &mut s.pattern, "High-frequency pattern");
                check(ui, &mut s.noise, "Colour noise");
                check(ui, &mut s.jitter, "Sub-pixel jitter").on_hover_text("Warps the image by tiny amounts");
                check(ui, &mut s.per_frame, "Vary every frame").on_hover_text("Stronger protection but much larger GIFs");
            });
            form_row(ui, "Seed", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut s.seed, 1.0, |d| d);
                    } else if ui.add_sized(size, egui::Button::new("Randomise")).clicked() {
                        s.seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1) % 1_000_000) as u32;
                    }
                })
            });
        });
        let changed = s != before;
        self.shield_tpl = s.clone();
        self.commit_layer(sel, changed, LayerKind::Shield(s));
    }

    fn ui_frames(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let n = self.base.as_ref().map(|a| a.len()).unwrap_or(1);
        let now = ctx.input(|i| i.time);
        if add_layer_button(ui, "Add Frames…") {
            if let Some(mut ps) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).pick_files() {
                ps.sort();
                let d = self.fr_delay;
                self.run_anim(ctx, "Add frames", move |a, _| {
                    let imgs = ps.iter().map(|p| io::load_still(p)).collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(ops::append_images(a, imgs, d))
                });
            }
        }
        ui.add_space(GROUP_GAP);
        let mut range_act: Option<(bool, bool)> = None;
        let two = |ui: &mut egui::Ui, act: &mut Option<(bool, bool)>, nth: bool| {
            form_row(ui, "", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if ui.add_sized(size, egui::Button::new(if i == 0 { "Skip" } else { "Enable" })).on_hover_text(if i == 0 { "Leave these frames out of the result" } else { "Bring these frames back" }).clicked() {
                        *act = Some((nth, i == 1));
                    }
                })
            });
        };
        group(ui, "Skip / Enable", |ui| {
            form_row(ui, "Frames", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    let v = if i == 0 { &mut self.fr_from } else { &mut self.fr_to };
                    spin(ui, size, v, 1.0, |d| d.range(1..=n).prefix(if i == 0 { "From " } else { "To " }));
                })
            });
            two(ui, &mut range_act, false);
            ui.add_space(4.0);
            form_row(ui, "Pattern", |ui| {
                num_fill(ui, &mut self.fr_every, 1.0, |d| {
                    d.range(2..=1000)
                        .custom_formatter(|v, _| format!("Every {}", ordinal(v as usize)))
                        .custom_parser(|s| s.trim().trim_start_matches("Every").trim().trim_end_matches(|c: char| c.is_alphabetic()).trim().parse().ok())
                })
            });
            two(ui, &mut range_act, true);
        });
        if let Some((nth, en)) = range_act {
            self.snapshot(&format!("range{now}"), now);
            let (from, to, every) = (self.fr_from.max(1) - 1, self.fr_to.max(1) - 1, self.fr_every.max(2));
            let a = self.base.as_mut().unwrap();
            for (i, f) in a.frames.iter_mut().enumerate() {
                let hit = if nth { (i + 1) % every == 0 } else { i >= from.min(to) && i <= to.max(from) };
                if hit {
                    f.enabled = en;
                }
            }
            self.timing_rev += 1;
        }
        group(ui, "Timing", |ui| {
            let mut set_all = false;
            form_row(ui, "Delay", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut self.fr_delay, 1.0, |d| d.range(1..=65535).suffix(" cs")).on_hover_text(format!("{:.2} fps", 100.0 / self.fr_delay.max(1) as f32));
                    } else {
                        set_all = ui.add_sized(size, egui::Button::new("Set All")).on_hover_text("Give every frame this delay").clicked();
                    }
                })
            });
            if set_all {
                let d = self.fr_delay;
                self.run_anim(ctx, "Set delays", move |a, _| Ok(ops::set_all_delays(a, d)));
            }
            let mut forever = self.base.as_ref().map(|a| a.loop_count.is_none()).unwrap_or(true);
            let mut count = self.base.as_ref().and_then(|a| a.loop_count).unwrap_or(1);
            let mut changed = false;
            form_row(ui, "Loop", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        changed |= cell(ui, size, |ui| check(ui, &mut forever, "Forever")).changed();
                    } else {
                        changed |= ui.add_enabled_ui(!forever, |ui| spin(ui, size, &mut count, 1.0, |d| d.range(1..=65535).suffix(" times"))).inner.changed();
                    }
                })
            });
            if changed {
                self.snapshot("loop", now);
                self.base.as_mut().unwrap().loop_count = if forever { None } else { Some(count.max(1)) };
            }
        });
        group(ui, "In-Betweens", |ui| {
            let mut go = false;
            form_row(ui, "Add", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut self.fr_crossfade, 1.0, |d| d.range(1..=30).suffix(" each")).on_hover_text("New frames between each pair");
                    } else {
                        cell(ui, size, |ui| check(ui, &mut self.fr_crossfade_wrap, "Loop")).on_hover_text("Also blend the last frame into the first");
                    }
                })
            });
            form_row(ui, "", |ui| {
                check(ui, &mut self.fr_follow, "Follow motion").on_hover_text("Moves the picture along with what moves in it, so things slide instead of fading through each other");
            });
            // too short a delay and browsers slow it down
            let shortest = self.base.as_ref().and_then(|a| a.frames.iter().filter(|f| f.enabled).map(|f| f.delay_cs).min()).unwrap_or(10);
            let each = shortest as f32 / (self.fr_crossfade + 1) as f32;
            if each < 2.0 {
                form_row(ui, "", |ui| {
                    ui.label(RichText::new("⚠ Not enough time to share: it gets longer").color(p().neutral).size(12.0)).on_hover_text("Each frame's time is shared with its new ones, but no frame goes under 2 cs (browsers slow those down). Use fewer to keep the length.");
                });
            }
            form_row(ui, "", |ui| {
                go = wide_button(ui, egui::Button::new("Make In-Betweens")).on_hover_text("Smoother motion. The length stays the same.").clicked();
            });
            if go {
                let (steps, wrap, follow) = (self.fr_crossfade, self.fr_crossfade_wrap, self.fr_follow);
                if let Some(base) = self.base.clone() {
                    let mut stack = self.stack.clone();
                    let plan_src: Vec<usize> = self.plan.iter().map(|f| f.src).collect();
                    self.spawn(ctx, "In-betweens", base.len(), move |p| {
                        let (out, counts) = ops::inbetween_counted(&base, steps, wrap, follow, Some(p));
                        // layers stretch with the frames: where each old frame
                        // now starts, counting shown frames as the timeline does,
                        // and how many it became
                        let mut pos = Vec::with_capacity(base.len());
                        let mut at = 0usize;
                        for (f, &cnt) in base.frames.iter().zip(&counts) {
                            pos.push((at, cnt));
                            if f.enabled {
                                at += cnt;
                            }
                        }
                        let map = |k: usize| plan_src.get(k).and_then(|&s| pos.get(s)).copied();
                        for l in &mut stack.layers {
                            if let Some(cl) = &mut l.timing.clips {
                                for c in cl.iter_mut() {
                                    if let Some((a, _)) = map(c.start) {
                                        let end = c.end.and_then(|e| map(e)).map(|(b, n)| b + n - 1);
                                        *c = crate::timeline::Clip { start: a, end: c.end.and(end) };
                                    }
                                }
                            }
                            for key in &mut l.timing.keys {
                                if let Some((a, _)) = map(key.frame) {
                                    key.frame = a;
                                }
                            }
                        }
                        JobOutput::Baked(Ok((out, stack)))
                    });
                }
            }
            let mut bg = false;
            form_row(ui, "Background", |ui| {
                bg = wide_button(ui, egui::Button::new("Use First Frame")).on_hover_text("Draw every frame over frame 1").clicked();
            });
            if bg {
                self.run_anim(ctx, "First frame as background", move |a, _| Ok(ops::first_frame_as_background(a)));
            }
        });
        let skipped = self.base.as_ref().map(|a| a.len() - a.enabled_count()).unwrap_or(0);
        let label = match skipped {
            0 => "Delete Skipped Frames".to_string(),
            1 => "Delete 1 Skipped Frame".to_string(),
            n => format!("Delete {n} Skipped Frames"),
        };
        let b = egui::Button::image_and_text(crate::icons::image(ctx, "edit-delete", 16.0, p().fg), label);
        if ui.add_enabled_ui(skipped > 0, |ui| ui.add_sized(vec2(ui.available_width(), CHIP_H), b)).inner.clicked() {
            self.run_anim(ctx, "Remove skipped", move |a, _| {
                let mut a = a.clone();
                a.frames.retain(|f| f.enabled);
                Ok(a)
            });
        }
    }
}

/// "1st", "2nd", "3rd", "4th" …
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, x) if x != 11 => "st",
        (2, x) if x != 12 => "nd",
        (3, x) if x != 13 => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

thread_local! {
    /// The selected layer's timing lives on the timeline (its clips), so the
    /// panel's frame range is replaced by a pointer there.
    static SEL_TIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Source-frame range editor (1-based, `to` 0 = end).
fn frames_row(ui: &mut egui::Ui, from: &mut usize, to: &mut usize, n: usize) {
    if SEL_TIMED.with(|c| c.get()) {
        form_row(ui, "Frames", |ui| hint(ui, "Set by its clips on the timeline"));
        return;
    }
    form_row(ui, "Frames", |ui| {
        even_grid(ui, 2, 0.0, |ui, i, size| {
            if i == 0 {
                spin(ui, size, from, 1.0, |d| d.range(1..=n).prefix("From "));
            } else {
                let mut t = if *to == 0 { n } else { *to };
                if spin(ui, size, &mut t, 1.0, |d| d.range(1..=n).prefix("To ")).changed() {
                    *to = if t >= n { 0 } else { t };
                }
            }
        })
    });
}

// ================================================================ effects panel

fn on(b: &[bool]) -> Option<String> {
    let n = b.iter().filter(|x| **x).count();
    (n > 0).then(|| n.to_string())
}

fn effects_ui(ui: &mut egui::Ui, fx: &mut EffectSettings, _tr: &mut Transform, corner: Option<[u8; 3]>) {
    let d = EffectSettings::default();
    let colour_changed = fx.hue != d.hue || fx.saturation != d.saturation || fx.lightness != d.lightness || fx.brightness != d.brightness || fx.contrast != d.contrast;
    group_ex(ui, "Colour", true, colour_changed.then(|| "●".to_string()), |ui| {
        slider_row(ui, "Hue", soft_slider(&mut fx.hue, 0.0..=200.0, "").max_decimals(0));
        slider_row(ui, "Saturation", soft_slider(&mut fx.saturation, 0.0..=200.0, "").max_decimals(0));
        slider_row(ui, "Lightness", soft_slider(&mut fx.lightness, 0.0..=200.0, "").max_decimals(0));
        slider_row(ui, "Brightness", soft_slider(&mut fx.brightness, -100.0..=100.0, "").max_decimals(0));
        slider_row(ui, "Contrast", soft_slider(&mut fx.contrast, -100.0..=100.0, "").max_decimals(0));
        form_row(ui, "", |ui| {
            ui.add_enabled_ui(colour_changed, |ui| {
                if wide_button(ui, egui::Button::new("Reset Colour")).clicked() {
                    fx.hue = 100.0;
                    fx.saturation = 100.0;
                    fx.lightness = 100.0;
                    fx.brightness = 0.0;
                    fx.contrast = 0.0;
                }
            });
        });
    });
    group_ex(ui, "Looks", true, on(&[fx.grayscale, fx.sepia, fx.monochrome, fx.negative, fx.tint, fx.change_bg]), |ui| {
        form_row(ui, "Style", |ui| toggle_grid(ui, 72.0, &mut [(&mut fx.grayscale, "Grayscale", ""), (&mut fx.sepia, "Sepia", ""), (&mut fx.monochrome, "Mono", "Black and white only"), (&mut fx.negative, "Invert", "Negative colours")]));
        opt_row(ui, "Tint", &mut fx.tint, |ui| {
            colour_btn(ui, &mut fx.tint_color);
            num_fill(ui, &mut fx.tint_amount, 1.0, |d| d.range(0.0..=100.0).max_decimals(0).suffix("%"));
        });
        opt_row(ui, "Background", &mut fx.change_bg, |ui| {
            colour_btn(ui, &mut fx.bg_color).on_hover_text("Fills transparent areas");
        });
    });
    use crate::booth::{BoothFx, BoothItem};
    // older layers kept one look and one distortion; they stack now
    if fx.booth != BoothFx::None {
        let it = BoothItem { fx: fx.booth, amount: fx.booth_amount, cx: fx.booth_cx, cy: fx.booth_cy, reverse: false };
        if BoothFx::LOOKS.contains(&fx.booth) {
            fx.looks.insert(0, it);
        } else {
            fx.warps.insert(0, it);
        }
        fx.booth = BoothFx::None;
    }
    if fx.warp != BoothFx::None {
        fx.warps.insert(0, BoothItem { fx: fx.warp, amount: fx.warp_amount, cx: fx.warp_cx, cy: fx.warp_cy, reverse: false });
        fx.warp = BoothFx::None;
    }
    // photo booth looks, artistic styles and filters: one place for how it looks
    let styled = !fx.looks.is_empty() || fx.oil || fx.emboss || fx.posterize || fx.solarize || fx.noise || fx.pixelate || fx.gotham || fx.lomo || fx.nashville || fx.toaster || fx.vignette || fx.polaroid;
    group_ex(ui, "Styles", true, styled.then(|| "●".to_string()), |ui| {
        form_row(ui, "Photo booth", |ui| stack_grid(ui, &BoothFx::LOOKS, &mut fx.looks, None));
        form_row(ui, "Artistic", |ui| {
            toggle_grid(ui, 92.0, &mut [
                (&mut fx.oil, "Oil Paint", "Smooths detail into brush-like patches"),
                (&mut fx.emboss, "Emboss", "Raised relief look"),
                (&mut fx.posterize, "Posterize", "Fewer colour steps"),
                (&mut fx.solarize, "Solarize", "Inverts the brightest tones"),
                (&mut fx.noise, "Noise", "Adds random grain"),
                (&mut fx.pixelate, "Pixelate", "Big square pixels"),
            ])
        });
        form_row(ui, "Filter", |ui| {
            toggle_grid(ui, 92.0, &mut [
                (&mut fx.gotham, "Gotham", "Cool and gritty high contrast"),
                (&mut fx.lomo, "Lomo", "Saturated with dark corners"),
                (&mut fx.nashville, "Nashville", "Warm pink vintage tint"),
                (&mut fx.toaster, "Toaster", "Warm faded film look"),
                (&mut fx.vignette, "Vignette", "Darkens the corners"),
                (&mut fx.polaroid, "Polaroid", "White instant-photo frame"),
            ])
        });
        for it in fx.looks.iter_mut() {
            sub_head(ui, it.fx.label());
            slider_row(ui, "Strength", soft_slider(&mut it.amount, 0.0..=100.0, "").suffix("%").max_decimals(0)).on_hover_text("Type a bigger number for a stronger effect");
        }
        if fx.oil {
            sub_head(ui, "Oil Paint");
            slider_row(ui, "Brush", egui::Slider::new(&mut fx.oil_radius, 1..=16).suffix(" px")).on_hover_text("How far each patch reaches");
            slider_row(ui, "Detail", egui::Slider::new(&mut fx.oil_detail, 2..=64)).on_hover_text("How many shades it keeps apart. Fewer gives flatter patches.");
        }
        if fx.emboss {
            sub_head(ui, "Emboss");
            slider_row(ui, "Strength", soft_slider(&mut fx.emboss_amount, 0.0..=100.0, "").suffix("%").max_decimals(0)).on_hover_text("Type a bigger number for a stronger effect");
            const DIRS: [(u8, &str); 8] = [(0, "Top left"), (1, "Top"), (2, "Top right"), (3, "Right"), (4, "Bottom right"), (5, "Bottom"), (6, "Bottom left"), (7, "Left")];
            form_row(ui, "Light from", |ui| combo(ui, "fx_emboss_dir", &mut fx.emboss_dir, &DIRS));
            form_row(ui, "", |ui| check(ui, &mut fx.emboss_colour, "Keep colours").on_hover_text("Raised relief over the picture instead of grey"));
        }
        if fx.posterize {
            sub_head(ui, "Posterize");
            slider_row(ui, "Levels", egui::Slider::new(&mut fx.posterize_levels, 2..=16).clamping(egui::SliderClamping::Never));
        }
        if fx.solarize {
            sub_head(ui, "Solarize");
            slider_row(ui, "Threshold", egui::Slider::new(&mut fx.solarize_threshold, 0.0..=255.0).max_decimals(0)).on_hover_text("Tones past this flip");
            form_row(ui, "", |ui| check(ui, &mut fx.solarize_reverse, "Reverse").on_hover_text("Flip the dark tones instead"));
        }
        if fx.noise {
            sub_head(ui, "Noise");
            slider_row(ui, "Amount", soft_slider(&mut fx.noise_amount, 1.0..=128.0, "").max_decimals(0));
            form_row(ui, "", |ui| {
                ui.horizontal(|ui| {
                    check(ui, &mut fx.noise_mono, "Grey").on_hover_text("The same grain on every colour");
                    check(ui, &mut fx.noise_still, "Still").on_hover_text("The same grain on every frame");
                })
            });
        }
        if fx.pixelate {
            sub_head(ui, "Pixelate");
            slider_row(ui, "Block size", egui::Slider::new(&mut fx.pixelate_size, 2..=64).suffix(" px").clamping(egui::SliderClamping::Never));
        }
        for (on, name, v) in [
            (fx.gotham, "Gotham", &mut fx.gotham_amount),
            (fx.lomo, "Lomo", &mut fx.lomo_amount),
            (fx.nashville, "Nashville", &mut fx.nashville_amount),
            (fx.toaster, "Toaster", &mut fx.toaster_amount),
            (fx.vignette, "Vignette", &mut fx.vignette_amount),
        ] {
            if on {
                sub_head(ui, name);
                slider_row(ui, "Strength", soft_slider(v, 0.0..=100.0, "").suffix("%").max_decimals(0)).on_hover_text("Type a bigger number for a stronger effect");
            }
        }
    });
    group_ex(ui, "Transparency", false, on(&[fx.rc_white, fx.rc_black, fx.rc_custom]), |ui| {
        form_row(ui, "Remove", |ui| toggle_grid(ui, 0.0, &mut [(&mut fx.rc_white, "White", ""), (&mut fx.rc_black, "Black", ""), (&mut fx.rc_custom, "Colour", "")]));
        ui.add_enabled_ui(fx.rc_custom, |ui| {
            form_row(ui, "Colour", |ui| {
                ui.horizontal(|ui| {
                    colour_btn(ui, &mut fx.rc_color);
                    if let Some(c) = corner {
                        if ui.button("From Corner").on_hover_text("The colour of the frame's top-left corner, usually the background").clicked() {
                            fx.rc_color = c;
                        }
                    }
                    let mut hex = format!("#{:02x}{:02x}{:02x}", fx.rc_color[0], fx.rc_color[1], fx.rc_color[2]);
                    if ui.add(egui::TextEdit::singleline(&mut hex).desired_width(ui.available_width())).changed() {
                        if let Some(c) = parse_hex(&hex) {
                            fx.rc_color = c;
                        }
                    }
                })
            });
        });
        let any = fx.rc_white || fx.rc_black || fx.rc_custom;
        ui.add_enabled_ui(any, |ui| {
            slider_row(ui, "Fuzz", soft_slider(&mut fx.rc_fuzz, 0.0..=60.0, "").suffix(" %").max_decimals(0)).on_hover_text("How close a colour must be to count");
            ui.add_enabled_ui(fx.rc_custom, |ui| {
                slider_row(ui, "Soft edge", soft_slider(&mut fx.rc_soft, 0.0..=40.0, "").suffix(" %").max_decimals(0)).on_hover_text("Colours a little past the fuzz fade out instead of staying, for smooth edges (green screen)");
                form_row(ui, "Spill", |ui| check(ui, &mut fx.rc_despill, "Remove colour spill")).on_hover_text("Takes the removed colour's tint out of what's left, such as a green glow from a green screen");
            });
            opt_row(ui, "Clean edges", &mut fx.rc_clean_edges, |ui| {
                num_fill(ui, &mut fx.rc_edge_px, 1.0, |d| d.range(1..=50).suffix(" px")).on_hover_text("Removes leftover fringe around cut-out areas");
            });
        });
    });
    // made before the Animation panel: still edited here
    let moving = fx.intro != crate::motion::Transition::None || fx.outro != crate::motion::Transition::None || !fx.loop_fx().is_empty() || fx.fade_in || fx.fade_out;
    if !fx.anim && (moving || fx.counter) {
        hint(ui, "This layer animates. New animation goes in the Animation panel.");
        anim_ui(ui, fx);
    }
    group_ex(ui, "Blur & Sharpen", false, on(&[fx.blur, fx.sharpen]), |ui| {
        opt_row(ui, "Blur", &mut fx.blur, |ui| fill_slider(ui, soft_slider(&mut fx.blur_sigma, 0.3..=20.0, "").max_decimals(1)));
        opt_row(ui, "Sharpen", &mut fx.sharpen, |ui| fill_slider(ui, soft_slider(&mut fx.sharpen_sigma, 0.3..=10.0, "").max_decimals(1)));
    });
    group_ex(ui, "Dither", false, on(&[fx.ordered_dither, fx.error_dither]), |ui| {
        form_row(ui, "Method", |ui| toggle_grid(ui, 0.0, &mut [(&mut fx.ordered_dither, "Ordered", "Regular pattern dithering"), (&mut fx.error_dither, "Diffusion", "Error diffusion dithering")]));
        ui.add_enabled_ui(fx.ordered_dither || fx.error_dither, |ui| slider_row(ui, "Levels", egui::Slider::new(&mut fx.dither_levels, 2..=16).clamping(egui::SliderClamping::Never)));
    });
    group_ex(ui, "Distort", false, on(&[!fx.warps.is_empty(), fx.wave, fx.swirl, fx.implode]), |ui| {
        form_row(ui, "Effect", |ui| stack_grid(ui, &BoothFx::DISTORT, &mut fx.warps, Some((&mut fx.wave, "Wave", "Ripples the picture up and down"))));
        for (k, it) in fx.warps.iter_mut().enumerate() {
            ui.push_id(("warp", k), |ui| {
                sub_head(ui, it.fx.label());
                slider_row(ui, "Strength", soft_slider(&mut it.amount, 0.0..=200.0, "").suffix("%").max_decimals(0)).on_hover_text("Type a bigger number for an even stronger effect");
                form_row(ui, "", |ui| check(ui, &mut it.reverse, "Reverse").on_hover_text(it.fx.reverse_tip()));
                if it.fx.has_centre() {
                    slider_row(ui, "Centre X", egui::Slider::new(&mut it.cx, 0.0..=100.0).suffix("%").max_decimals(0));
                    // up from the bottom, like the other Y values
                    let mut up = 100.0 - it.cy;
                    if slider_row(ui, "Centre Y", egui::Slider::new(&mut up, 0.0..=100.0).suffix("%").max_decimals(0)).changed() {
                        it.cy = 100.0 - up;
                    }
                }
            });
        }
        if fx.wave {
            sub_head(ui, "Wave");
            slider_row(ui, "Height", soft_slider(&mut fx.wave_amp, 1.0..=100.0, "").suffix(" px").max_decimals(0));
            slider_row(ui, "Wavelength", soft_slider(&mut fx.wave_len, 4.0..=500.0, "").suffix(" px").max_decimals(0));
            form_row(ui, "", |ui| check(ui, &mut fx.wave_reverse, "Reverse").on_hover_text("The waves travel the other way"));
        }
        // Twirl, Dent and Bulge do these now; older layers keep theirs
        if fx.swirl {
            opt_row(ui, "Swirl (old)", &mut fx.swirl, |ui| fill_slider(ui, soft_slider(&mut fx.swirl_deg, -720.0..=720.0, "").suffix("°").max_decimals(0)));
        }
        if fx.implode {
            opt_row(ui, "Implode (old)", &mut fx.implode, |ui| fill_slider(ui, soft_slider(&mut fx.implode_amount, -2.0..=2.0, "").max_decimals(2)));
        }
    });
    group_ex(ui, "Frame", false, on(&[fx.frame_camera, fx.frame_fuzzy, fx.frame_rounded, fx.frame_border]), |ui| {
        form_row(ui, "Style", |ui| {
            toggle_grid(ui, 72.0, &mut [
                (&mut fx.frame_camera, "Camera", "Viewfinder frame"),
                (&mut fx.frame_fuzzy, "Fuzzy", "Soft blurred edges"),
                (&mut fx.frame_rounded, "Rounded", "Rounded corners"),
                (&mut fx.frame_border, "Border", "Solid border"),
            ])
        });
        ui.add_enabled_ui(fx.frame_rounded, |ui| slider_row(ui, "Radius", soft_slider(&mut fx.rounded_radius, 1.0..=200.0, "").suffix(" px").max_decimals(0)));
        ui.add_enabled_ui(fx.frame_border, |ui| {
            form_row(ui, "Border", |ui| {
                ui.horizontal(|ui| {
                    colour_btn(ui, &mut fx.border_color);
                    num_fill(ui, &mut fx.border_px, 1.0, |d| d.range(1..=2000).suffix(" px"));
                })
            });
        });
    });
    // flips and rotation live in the Rotate frame edit; only shown for old layers that use them
    if !(fx.flip_v || fx.flip_h || fx.rot90 || fx.rot270 || fx.rot_any) {
        return;
    }
    group_ex(ui, "Transform (Old)", true, on(&[fx.flip_v, fx.flip_h, fx.rot90, fx.rot270, fx.rot_any]), |ui| {
        hint(ui, "Use the Rotate frame edit instead. These stay for older projects.");
        form_row(ui, "Transform", |ui| {
            toggle_grid(ui, 64.0, &mut [(&mut fx.flip_h, "Flip horizontally", ""), (&mut fx.flip_v, "Flip vertically", ""), (&mut fx.rot90, "Turn 90° clockwise", ""), (&mut fx.rot270, "Turn 90° anticlockwise", "")])
        });
        opt_row(ui, "Rotate", &mut fx.rot_any, |ui| num_fill(ui, &mut fx.rot_deg, 1.0, |d| d.speed(0.5).suffix("°")));
    });
}

/// GIF dithering: off, error diffusion or an ordered pattern.
fn dither_row(ui: &mut egui::Ui, o: &mut crate::encode::GifOptions) {
    let mut mode = if o.dither { 1u8 } else if o.ordered_dither { 2 } else { 0 };
    let r = form_row(ui, "Dither", |ui| seg_row(ui, &mut mode, &[(0, "None"), (1, "Diffusion"), (2, "Ordered")]));
    r.on_hover_text("Diffusion scatters the error between colours and looks smoothest. Ordered uses a fixed pattern: faster on the graphics card, often smaller, and steady between frames.");
    o.dither = mode == 1;
    o.ordered_dither = mode == 2;
}

/// A small heading inside a group, over one effect's own settings.
fn sub_head(ui: &mut egui::Ui, text: &str) {
    // the rows under it go with it in a search
    FIND.with(|f| {
        let mut f = f.borrow_mut();
        f.ctx = text.to_string();
        f.follow = false;
    });
    if find_row_shown("") {
        ui.add_space(4.0);
        form_row_raw(ui, "", |ui| ui.label(egui::RichText::new(text).strong()));
    }
}

/// Checkboxes for stacked looks or distortions: ticking one adds it to
/// `items` (kept in the order of `list`), unticking takes it out. `extra`
/// is one more plain switch in the same grid.
fn stack_grid(ui: &mut egui::Ui, list: &[crate::booth::BoothFx], items: &mut Vec<crate::booth::BoothItem>, extra: Option<(&mut bool, &str, &str)>) {
    let mut on: Vec<bool> = list.iter().map(|b| items.iter().any(|i| i.fx == *b)).collect();
    {
        let mut rows: Vec<(&mut bool, &str, &str)> = on.iter_mut().zip(list).map(|(o, b)| (o, b.label(), b.tip())).collect();
        if let Some(e) = extra {
            rows.push(e);
        }
        toggle_grid(ui, 92.0, &mut rows);
    }
    for (b, o) in list.iter().zip(&on) {
        let has = items.iter().any(|i| i.fx == *b);
        if *o && !has {
            items.push(crate::booth::BoothItem { fx: *b, ..Default::default() });
        } else if !*o && has {
            items.retain(|i| i.fx != *b);
        }
    }
    items.sort_by_key(|i| list.iter().position(|b| *b == i.fx).unwrap_or(usize::MAX));
}

/// The Animation panel's settings (also shown for older effects layers that animate).
fn anim_ui(ui: &mut egui::Ui, fx: &mut EffectSettings) {
    use crate::motion::Transition as T;
    // the old fade switches become the Fade transition
    if fx.fade_in && fx.intro == T::None {
        fx.intro = T::Fade;
    }
    if fx.fade_out && fx.outro == T::None {
        fx.outro = T::Fade;
    }
    fx.fade_in = false;
    fx.fade_out = false;
    let items: Vec<(T, &str)> = T::ALL.iter().map(|t| (*t, t.label())).collect();
    let ends = fx.intro != T::None || fx.outro != T::None;
    group_ex(ui, "Intro & Outro", true, ends.then(|| "●".to_string()), |ui| {
        let (mut fi, mut fo) = fx.transition_frames();
        form_row(ui, "Intro", |ui| combo(ui, "fx_intro", &mut fx.intro, &items)).on_hover_text("How the layer's picture appears");
        ui.add_enabled_ui(fx.intro != T::None, |ui| {
            if form_row(ui, "Length", |ui| num_fill(ui, &mut fi, 1.0, |d| d.range(1..=10000).suffix(" frames"))).changed() {
                fx.intro_frames = fi;
            }
        });
        form_row(ui, "Outro", |ui| combo(ui, "fx_outro", &mut fx.outro, &items)).on_hover_text("How it goes away");
        ui.add_enabled_ui(fx.outro != T::None, |ui| {
            if form_row(ui, "Length", |ui| num_fill(ui, &mut fo, 1.0, |d| d.range(1..=10000).suffix(" frames"))).changed() {
                fx.outro_frames = fo;
            }
        });
    });
    let looping = !fx.loop_fx().is_empty();
    group_ex(ui, "Loop", true, looping.then(|| "●".to_string()), |ui| {
        form_row(ui, "Motion", |ui| {
            toggle_grid(ui, 84.0, &mut [
                (&mut fx.loop_shake, "Shake", "Jitters about"),
                (&mut fx.loop_pulse, "Pulse", "Grows and shrinks"),
                (&mut fx.loop_heartbeat, "Heartbeat", "Two quick beats each time round"),
                (&mut fx.loop_hop, "Hop", "Bounces up and down"),
                (&mut fx.loop_float, "Float", "Bobs gently up and down"),
                (&mut fx.loop_wobble, "Wobble", "Rocks side to side"),
                (&mut fx.loop_spin, "Spin", "Turns all the way round"),
                (&mut fx.loop_squash, "Squash", "Squashes and stretches"),
            ])
        });
        form_row(ui, "Look", |ui| {
            toggle_grid(ui, 84.0, &mut [
                (&mut fx.loop_rainbow, "Rainbow", "Cycles through the colours"),
                (&mut fx.loop_glitch, "Glitch", "Digital glitches now and then"),
                (&mut fx.loop_blink, "Blink", "Gone for half of each time round"),
            ])
        });
        ui.add_enabled_ui(looping, |ui| {
            slider_row(ui, "Strength", soft_slider(&mut fx.loop_amount, 0.0..=200.0, "").suffix("%").max_decimals(0));
            form_row(ui, "Repeats", |ui| num_fill(ui, &mut fx.loop_cycles, 1.0, |d| d.range(1..=20).suffix("× per loop"))).on_hover_text("Whole numbers keep the loop seamless");
            form_row(ui, "", |ui| check(ui, &mut fx.loop_reverse, "Reverse").on_hover_text("Turn, bob and cycle colours the other way"));
        });
    });
    let moving = ends || looping;
    group_ex(ui, "Timer", false, fx.counter.then(|| "●".to_string()), |ui| {
        form_row(ui, "", |ui| check(ui, &mut fx.counter, "Show running time").on_hover_text("A seconds counter over the picture"));
        ui.add_enabled_ui(fx.counter, |ui| {
            const CORNERS: [(u8, &str); 4] = [(2, "Top left"), (3, "Top right"), (0, "Bottom left"), (1, "Bottom right")];
            form_row(ui, "Corner", |ui| combo(ui, "fx_counter_pos", &mut fx.counter_pos, &CORNERS));
            slider_row(ui, "Size", soft_slider(&mut fx.counter_size, 2.0..=40.0, "").suffix("%").max_decimals(0)).on_hover_text("Of the picture's height");
        });
    });
    group_ex(ui, "Hidden Parts", false, None, |ui| {
        ui.add_enabled_ui(moving, |ui| {
            form_row(ui, "Fill", |ui| check(ui, &mut fx.fade_to_transparent, "Transparent").on_hover_text("What shows where the picture moved away. Black otherwise."));
        });
    });
}

// ================================================================ helpers

/// What Copy puts on the system clipboard for layers, so Ctrl+V comes back here.
const LAYERS_TAG: &str = "GIFkit layers";

/// Move a placed layer by `d` pixels right and down (pasted copies).
fn offset_layer(k: &mut LayerKind, d: i32) {
    match k {
        LayerKind::Text(t) => (t.x, t.y) = (t.x + d, t.y + d),
        LayerKind::Shape(s) => (s.x, s.y) = (s.x + d, s.y + d),
        LayerKind::Emoji(e) => (e.x, e.y) = (e.x + d, e.y + d),
        LayerKind::Censor(c) => (c.x, c.y) = (c.x + d, c.y + d),
        LayerKind::Overlay(o) => {
            for p in &mut o.placements {
                (p.x, p.y) = (p.x + d, p.y + d);
            }
        }
        LayerKind::Markup(m) => {
            for st in &mut m.strokes {
                for pt in &mut st.points {
                    pt[0] += d as f32;
                    pt[1] += d as f32;
                }
            }
        }
        _ => {}
    }
}

/// File paths from pasted text: `file://` URIs (percent-encoded) or plain paths.
fn paths_from_text(t: &str) -> Vec<PathBuf> {
    fn pct_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    t.lines()
        .map(|l| l.trim().trim_matches('"'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.strip_prefix("file://") {
            Some(rest) => PathBuf::from(pct_decode(rest.trim_start_matches("localhost"))),
            None => PathBuf::from(l),
        })
        .filter(|p| p.is_file())
        .collect()
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Screen-space pick radius for resize corners and the rotate handle.
const HANDLE_HIT: f32 = 16.0;

/// Midpoints of a box's top, right, bottom and left edges.
fn edge_mids(o: &Obb) -> [(f32, f32); 4] {
    [o.from_local(0.0, -o.h / 2.0), o.from_local(o.w / 2.0, 0.0), o.from_local(0.0, o.h / 2.0), o.from_local(-o.w / 2.0, 0.0)]
}

fn draw_sel(painter: &egui::Painter, r: Rect, accent: Color32) {
    painter.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_black_alpha(160)), egui::StrokeKind::Middle);
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
        let h = Rect::from_center_size(c, vec2(10.0, 10.0));
        painter.rect_filled(h, 1.0, Color32::WHITE);
        painter.rect_stroke(h, 1.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
    }
}

fn hit_corner(m: &Mapper, r: &[f32; 4], screen: Pos2) -> Option<Pos2> {
    let corners = [
        (pos2(r[0], r[1]), pos2(r[0] + r[2], r[1] + r[3])),
        (pos2(r[0] + r[2], r[1]), pos2(r[0], r[1] + r[3])),
        (pos2(r[0], r[1] + r[3]), pos2(r[0] + r[2], r[1])),
        (pos2(r[0] + r[2], r[1] + r[3]), pos2(r[0], r[1])),
    ];
    for (c, opp) in corners {
        if m.to_screen(c).distance(screen) <= HANDLE_HIT {
            return Some(opp);
        }
    }
    None
}

fn apply_aspect(fixed: Pos2, mut pt: Pos2, aspect: Option<f32>) -> Pos2 {
    if let Some(ar) = aspect {
        let dx = pt.x - fixed.x;
        let dy = pt.y - fixed.y;
        let w = dx.abs().max(dy.abs() * ar);
        let h = w / ar;
        pt.x = fixed.x + w * dx.signum();
        pt.y = fixed.y + h * if dy == 0.0 { 1.0 } else { dy.signum() };
    }
    pt
}

/// Scale the box from `fixed` to `pt` (already at the right aspect) down
/// uniformly until it fits inside the bounds.
fn fit_aspect_in_bounds(fixed: Pos2, pt: Pos2, b: (f32, f32)) -> Pos2 {
    let fixed = clamp_pt(fixed, b);
    let (dx, dy) = (pt.x - fixed.x, pt.y - fixed.y);
    let room_x = if dx >= 0.0 { b.0 - fixed.x } else { fixed.x };
    let room_y = if dy >= 0.0 { b.1 - fixed.y } else { fixed.y };
    let mut k: f32 = 1.0;
    if dx.abs() > room_x {
        k = k.min(room_x / dx.abs());
    }
    if dy.abs() > room_y {
        k = k.min(room_y / dy.abs());
    }
    pos2(fixed.x + dx * k.max(0.0), fixed.y + dy * k.max(0.0))
}

fn clamp_pt(pt: Pos2, b: (f32, f32)) -> Pos2 {
    pos2(pt.x.clamp(0.0, b.0), pt.y.clamp(0.0, b.1))
}

/// Rectangle tool: draw new (if `allow_new`), move, or resize from corners.
/// When `allow_new` is false the rect may extend outside the bounds (overlays).
#[allow(clippy::too_many_arguments)]
fn rect_tool(drag: &mut Option<DragMode>, started: bool, dragged: bool, stopped: bool, resp: &egui::Response, m: &Mapper, r: &mut [f32; 4], bounds: (f32, f32), aspect: Option<f32>, allow_new: bool) {
    let clampb = |pt: Pos2| if allow_new { clamp_pt(pt, bounds) } else { pt };
    if started {
        // where the button went down, not where the pointer is once egui decides it's a drag
        if let Some(sp) = resp.ctx.input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos()) {
            let ip = m.to_img(sp);
            let has = r[2] > 0.0 && r[3] > 0.0;
            *drag = if let Some(opp) = has.then(|| hit_corner(m, r, sp)).flatten() {
                Some(DragMode::Corner(opp))
            } else if has && Rect::from_min_size(pos2(r[0], r[1]), vec2(r[2], r[3])).contains(ip) {
                Some(DragMode::Move(ip - pos2(r[0], r[1])))
            } else if allow_new {
                Some(DragMode::New(clampb(ip)))
            } else {
                None
            };
        }
    }
    if dragged {
        if let (Some(d), Some(sp)) = (*drag, resp.interact_pointer_pos()) {
            let ip = m.to_img(sp);
            match d {
                DragMode::New(a) | DragMode::Corner(a) => {
                    let b = match aspect {
                        // keep the ratio exact: shrink along the diagonal instead of clamping each axis
                        Some(_) if allow_new => fit_aspect_in_bounds(a, apply_aspect(a, ip, aspect), bounds),
                        _ => clampb(apply_aspect(a, clampb(ip), aspect)),
                    };
                    let mn = pos2(a.x.min(b.x).round(), a.y.min(b.y).round());
                    let mx = pos2(a.x.max(b.x).round(), a.y.max(b.y).round());
                    let (mut w, mut h) = ((mx.x - mn.x).max(1.0), (mx.y - mn.y).max(1.0));
                    if let Some(ar) = aspect {
                        // whole pixels at the exact ratio, never past the edge
                        h = (w / ar).round().max(1.0);
                        while allow_new && w > 1.0 && (mn.y + h > bounds.1 || mn.x + w > bounds.0) {
                            w -= 1.0;
                            h = (w / ar).round().max(1.0);
                        }
                    }
                    // keep the fixed corner where it is when the size was adjusted
                    let x = if b.x < a.x { (a.x.round() - w).max(0.0) } else { mn.x };
                    let y = if b.y < a.y { (a.y.round() - h).max(0.0) } else { mn.y };
                    *r = [x, y, w, h];
                }
                DragMode::Move(off) => {
                    let (mut nx, mut ny) = ((ip.x - off.x).round(), (ip.y - off.y).round());
                    if allow_new {
                        nx = nx.clamp(0.0, (bounds.0 - r[2]).max(0.0));
                        ny = ny.clamp(0.0, (bounds.1 - r[3]).max(0.0));
                    }
                    r[0] = nx;
                    r[1] = ny;
                }
            }
        }
    }
    if stopped {
        *drag = None;
    }
}
