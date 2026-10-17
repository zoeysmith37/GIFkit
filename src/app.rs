//! The egui application.

use crate::anim::{effective_delay, Animation};
use crate::effects::{CensorMethod, CensorSettings, EffectSettings, ShieldSettings};
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
}

impl Tool {
    const ALL: [Tool; 14] = [
        Tool::Effects,
        Tool::Text,
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
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Tool::Resize => "⇲",
            Tool::Crop => "✂",
            Tool::Rotate => "⟳",
            Tool::Reverse => "⇄",
            Tool::Speed => "◷",
            Tool::Optimize => "⚙",
            Tool::Effects => "✦",
            Tool::Text => "T",
            Tool::Censor => "▦",
            Tool::AddImage => "◩",
            Tool::Watermark => "©",
            Tool::Shield => "◈",
            Tool::Cut => "✄",
            Tool::Frames => "▤",
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
        }
    }
    fn title(self) -> &'static str {
        match self {
            Tool::Rotate => "Rotate / flip",
            Tool::Reverse => "Reverse / loop",
            Tool::Optimize => "Compression",
            Tool::AddImage => "Image",
            _ => self.label(),
        }
    }
    fn layer_kind(self) -> Option<Kind> {
        match self {
            Tool::Effects => Some(Kind::Effects),
            Tool::Text => Some(Kind::Text),
            Tool::Censor => Some(Kind::Censor),
            Tool::AddImage => Some(Kind::Overlay),
            Tool::Watermark => Some(Kind::Watermark),
            Tool::Shield => Some(Kind::Shield),
            _ => None,
        }
    }
    /// Tools that use left-drag on the canvas (otherwise left-drag pans).
    fn uses_canvas_drag(self) -> bool {
        matches!(self, Tool::Crop | Tool::Text | Tool::Censor | Tool::AddImage)
    }
}

fn kind_tool(k: Kind) -> Tool {
    match k {
        Kind::Effects => Tool::Effects,
        Kind::Text => Tool::Text,
        Kind::Censor => Tool::Censor,
        Kind::Overlay => Tool::AddImage,
        Kind::Watermark => Tool::Watermark,
        Kind::Shield => Tool::Shield,
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
    fn can_target(self) -> bool {
        matches!(self, ExportFormat::Gif | ExportFormat::Webp | ExportFormat::Avif)
    }
    fn tip(self) -> &'static str {
        match self {
            ExportFormat::Gif => "Plays everywhere",
            ExportFormat::Webp => "Often 30 to 70% smaller than GIF. Works in browsers and most apps.",
            ExportFormat::Avif => "Smallest files. Needs a modern browser.",
            ExportFormat::Apng => "Lossless with full colour and transparency",
            ExportFormat::Mp4 => "Video without transparency",
            ExportFormat::Webm => "Video with transparency",
            ExportFormat::Frames => "One image per frame",
            ExportFormat::Sheet => "Sprite sheet for game engines",
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
}

struct Job {
    label: String,
    rx: Receiver<JobOutput>,
    progress: Arc<AtomicUsize>,
    total: usize,
}

struct CmpResult {
    key: u64,
    anim: Animation,
    bytes: Arc<Vec<u8>>,
    gen: u64,
}

struct CmpJob {
    key: u64,
    rx: Receiver<anyhow::Result<(Animation, Vec<u8>)>>,
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

/// Texture cache invalidated by a key.
#[derive(Default)]
struct TexCache {
    key: u64,
    map: HashMap<usize, TextureHandle>,
}

impl TexCache {
    fn get(&mut self, key: u64, i: usize) -> Option<TextureHandle> {
        if self.key != key {
            self.map.clear();
            self.key = key;
        }
        self.map.get(&i).cloned()
    }
    fn put(&mut self, ctx: &egui::Context, i: usize, img: &RgbaImage, opts: TextureOptions, name: &str) -> TextureHandle {
        if self.map.len() > 600 {
            self.map.clear();
        }
        let t = ctx.load_texture(format!("{name}{i}"), to_color_image(img), opts);
        self.map.insert(i, t.clone());
        t
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
    Resize { id: u64, li: usize, fixed: (f32, f32), out0: Obb, src: Obb, size0: f32, d0: f32 },
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

pub struct GifForgeApp {
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
    checker: bool,
    pixelated: bool,

    base_tex: TexCache,
    thumb_tex: TexCache,
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
    font_picker_open: bool,
    ffmpeg: bool,
    theme: ThemeChoice,

    compare: bool,
    view_left: ViewSource,
    view_right: ViewSource,
    view_single: ViewSource,
    cmp: Option<CmpResult>,
    cmp_job: Option<CmpJob>,
    cmp_key_seen: u64,
    cmp_key_since: f64,
    cmp_gen: u64,

    // tool UI state
    crop_lock: AspectLock,
    trim_tol: u8,
    drag: Option<DragMode>,
    gif_opts: GifOptions,
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
    fr_thumb: f32,
    video_bg: [u8; 3],

    // layout
    sidebar_compact: bool,
    layers_collapsed: bool,
    layer_drag: Option<u64>,
    export_open: bool,
    export_fmt: ExportFormat,
    frames_zip: bool,
    presets_open: bool,
    presets: Vec<crate::presets::Preset>,
    preset_problems: Vec<String>,
    preset_name: String,
    preset_inc: (bool, bool, bool),
    font_preview_queue: Vec<String>,
    preview_fonts: HashMap<String, bool>,
    prep_cache: Option<(u64, Arc<stack::Prepared>)>,
    webp_opts: io::WebpOptions,
    avif_quality: u8,
    target_on: bool,
    target_val: f32,
    target_mb: bool,
    history_open: bool,
    /// last committed live state for automatic undo entries: (signature, stack, gif options)
    committed: Option<(u64, Stack, GifOptions)>,
    pending_sig: u64,
    commit_since: f64,
    last_comp: Option<(usize, TextureHandle)>,
    last_tick: Option<f64>,
    resize_by_pct: bool,
    about_open: bool,
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
    about_icon: Option<TextureHandle>,
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

/// Quick targets, with a little headroom under each service's limit.
const TARGET_PRESETS: [(&str, f32, bool); 3] = [("Discord 10 MB", 9.5, true), ("X 15 MB", 14.0, true), ("Emote 256 KB", 250.0, false)];

/// Draw a tool's icon. A few are vector drawings because no bundled font has a
/// clear glyph for them; the rest use the tool's text glyph.
fn paint_tool_icon(painter: &egui::Painter, t: Tool, c: Pos2, size: f32, col: Color32) {
    let s = size / 15.0; // designed at 15 px
    let stroke = Stroke::new((1.6 * s).max(1.2), col);
    match t {
        Tool::Shield => {
            // "no AI" sign: circle with a slash over the letters AI
            let r = 7.0 * s;
            painter.text(c + vec2(0.0, 0.3 * s), egui::Align2::CENTER_CENTER, "AI", egui::FontId::new(8.6 * s, egui::FontFamily::Name("gf-sans-bold".into())), col);
            painter.circle_stroke(c, r, stroke);
            let d = r * std::f32::consts::FRAC_1_SQRT_2;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
        }
        Tool::Crop => {
            // two interlocking corner brackets
            let a = 6.5 * s;
            let b = 3.5 * s;
            painter.line_segment([c + vec2(-b, -a - 1.5 * s), c + vec2(-b, b)], stroke);
            painter.line_segment([c + vec2(-b, b), c + vec2(a + 1.5 * s, b)], stroke);
            painter.line_segment([c + vec2(-a - 1.5 * s, -b), c + vec2(b, -b)], stroke);
            painter.line_segment([c + vec2(b, -b), c + vec2(b, a + 1.5 * s)], stroke);
        }
        _ => {
            painter.text(c, egui::Align2::CENTER_CENTER, t.icon(), egui::FontId::proportional(size), col);
        }
    }
}

/// Clock with a counter-clockwise arrow: the usual "history" symbol.
fn paint_history_icon(painter: &egui::Painter, c: Pos2, size: f32, col: Color32) {
    let s = size / 15.0;
    let stroke = Stroke::new((1.6 * s).max(1.2), col);
    let r = 6.5 * s;
    // open circle (gap at the upper left where the arrow head sits)
    let n = 28;
    let (a0, a1) = (-2.4f32, 2.9f32);
    let pts: Vec<Pos2> = (0..=n).map(|i| {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        c + vec2(a.cos(), a.sin()) * r
    }).collect();
    painter.add(egui::Shape::line(pts, stroke));
    // arrow head at the start of the arc
    let tip = c + vec2(a0.cos(), a0.sin()) * r;
    painter.line_segment([tip, tip + vec2(-0.6, 3.4) * s], stroke);
    painter.line_segment([tip, tip + vec2(3.4, 0.6) * s], stroke);
    // hands
    painter.line_segment([c, c + vec2(0.0, -3.8 * s)], stroke);
    painter.line_segment([c, c + vec2(2.8 * s, 1.6 * s)], stroke);
}

/// Open a folder in the system file manager.
fn open_in_file_manager(dir: &std::path::Path) {
    open_external(&dir.to_string_lossy());
}

const REPO_URL: &str = "https://github.com/zoeysmith37/gifforge";

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
        Kind::Text => "text",
        Kind::Censor => "censor",
        Kind::Overlay => "overlay",
        Kind::Watermark => "watermark",
        Kind::Shield => "shield",
    }
}

// ---------------------------------------------------------------- Breeze-ish widgets

/// A collapsible Breeze group box: clickable title row above a framed area.
fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
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
fn group_ex<R>(ui: &mut egui::Ui, title: &str, default_open: bool, badge: Option<String>, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let pal = *p();
    if title.is_empty() {
        let r = egui::Frame::new()
            .fill(mix(pal.window, pal.view, 0.35))
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(3.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui)
            })
            .inner;
        ui.add_space(8.0);
        return Some(r);
    }
    let id = ui.make_persistent_id(("grp", title));
    let cmd: Option<bool> = ui.ctx().data(|d| d.get_temp::<Option<bool>>(egui::Id::unique(GROUP_CMD))).flatten();
    let mut open: bool = ui.ctx().data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    if let Some(c) = cmd {
        open = c;
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let hovered = resp.hovered();
    if resp.clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, 3.0, mix(pal.window, pal.highlight, 0.12));
    }
    let arrow = if open { "▾" } else { "▸" };
    painter.text(pos2(rect.left() + 4.0, rect.center().y), egui::Align2::LEFT_CENTER, arrow, egui::FontId::proportional(13.0), pal.fg_muted);
    painter.text(pos2(rect.left() + 20.0, rect.center().y), egui::Align2::LEFT_CENTER, title, theme::medium(13.5), pal.fg);
    if let Some(b) = badge {
        painter.text(pos2(rect.right() - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, b, egui::FontId::proportional(11.0), pal.highlight);
    }
    if !open {
        ui.add_space(2.0);
        return None;
    }
    let r = egui::Frame::new()
        .fill(mix(pal.window, pal.view, 0.35))
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(3.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(8.0);
    Some(r)
}

fn toolbar_sep(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    ui.add(egui::Separator::default().vertical().spacing(8.0));
    ui.add_space(2.0);
}

/// Height of toolbar / transport controls.
const TOOLBAR_H: f32 = 28.0;
/// Height of quick-pick chips (preset rows).
const CHIP_H: f32 = 22.0;

/// Small quick-pick button (preset rows): same height and padding everywhere.
fn chip(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Button::new(text.into()).wrap_mode(egui::TextWrapMode::Extend).min_size(vec2(28.0, CHIP_H)))
}

/// Square flat icon button.
fn icon_button(ui: &mut egui::Ui, icon: &str, enabled: bool) -> egui::Response {
    let pal = p();
    ui.add_enabled(enabled, egui::Button::new(RichText::new(icon).size(15.0).color(if enabled { pal.fg } else { pal.fg_muted })).frame_when_inactive(false).min_size(vec2(28.0, 28.0)))
}

/// Flat icon toggle; returns true when clicked.
fn toggle_icon(ui: &mut egui::Ui, icon: &str, on: bool, tip: &str) -> bool {
    let pal = p();
    ui.add(egui::Button::new(RichText::new(icon).size(15.0).color(if on { pal.fg } else { pal.fg_muted })).selected(on).frame_when_inactive(on).min_size(vec2(28.0, 28.0))).on_hover_text(tip).clicked()
}

/// One segment of a segmented control.
fn segment(ui: &mut egui::Ui, text: &str, selected: bool, tip: &str) -> bool {
    let pal = p();
    ui.add(egui::Button::new(RichText::new(text).size(15.0).color(if selected { pal.fg } else { pal.fg_muted })).selected(selected).min_size(vec2(30.0, TOOLBAR_H))).on_hover_text(tip).clicked()
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

/// Breeze "default" button.
fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    let pal = p();
    ui.add(egui::Button::new(RichText::new(text).font(theme::medium(13.5)).color(pal.fg)).fill(mix(pal.button, pal.highlight, 0.28)).stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(120.0, 28.0))).clicked()
}

fn tinted(ui: &mut egui::Ui, text: &str, tint: Color32) -> bool {
    let pal = p();
    ui.add(egui::Button::new(RichText::new(text).size(12.0).color(pal.fg)).fill(mix(pal.button, tint, 0.3)).stroke(Stroke::new(1.0, mix(pal.border_strong, tint, 0.6)))).clicked()
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(p().fg_muted));
}

/// Slider whose handle stops at the range, but typed values can go beyond it.
fn soft_slider<'a>(v: &'a mut f32, r: std::ops::RangeInclusive<f32>, text: &str) -> egui::Slider<'a> {
    egui::Slider::new(v, r).text(text).clamping(egui::SliderClamping::Never)
}

/// Flat toolbar button (frame only on hover), as in KDE toolbars.
fn tool_button(ui: &mut egui::Ui, icon: &str, text: &str, enabled: bool) -> egui::Response {
    let pal = p();
    let txt = RichText::new(format!("{icon}  {text}")).color(if enabled { pal.fg } else { pal.fg_muted });
    ui.add_enabled(enabled, egui::Button::new(txt).frame_when_inactive(false).min_size(vec2(0.0, TOOLBAR_H)))
}

fn small_flat(ui: &mut egui::Ui, text: &str, tip: &str) -> bool {
    let pal = p();
    ui.add(egui::Button::new(RichText::new(text).size(11.5).color(pal.fg_muted)).frame_when_inactive(false).min_size(vec2(18.0, 20.0))).on_hover_text(tip).clicked()
}

impl GifForgeApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let theme_choice = match std::env::var("GIFFORGE_THEME").as_deref() {
            Ok("dark") => ThemeChoice::Dark,
            Ok("light") => ThemeChoice::Light,
            _ => theme::load_theme_choice(),
        };
        theme_choice.apply(&cc.egui_ctx);
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
            checker: true,
            pixelated: false,
            base_tex: TexCache::default(),
            thumb_tex: TexCache::default(),
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
            font_picker_open: false,
            ffmpeg: io::ffmpeg_available(),
            theme: theme_choice,
            compare: true,
            view_left: ViewSource::Original,
            view_right: ViewSource::Compressed,
            cmp: None,
            cmp_job: None,
            cmp_key_seen: 0,
            cmp_key_since: 0.0,
            cmp_gen: 0,
            crop_lock: AspectLock::Free,
            trim_tol: 8,
            drag: None,
            gif_opts: GifOptions::default(),
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
            fr_thumb: 96.0,
            video_bg: [255, 255, 255],
            view_single: ViewSource::Edit,
            sidebar_compact: theme::get_setting("sidebar_compact").as_deref() == Some("1"),
            layers_collapsed: false,
            layer_drag: None,
            export_open: false,
            export_fmt: ExportFormat::Gif,
            frames_zip: false,
            presets_open: false,
            presets: Vec::new(),
            preset_problems: Vec::new(),
            preset_name: std::env::var("GIFFORGE_PRESET_NAME").unwrap_or_default(),
            preset_inc: (true, true, true),
            font_preview_queue: Vec::new(),
            preview_fonts: HashMap::new(),
            prep_cache: None,
            webp_opts: io::WebpOptions::default(),
            avif_quality: 60,
            target_on: false,
            target_val: 9.5,
            target_mb: true,
            history_open: false,
            committed: None,
            commit_since: 0.0,
            pending_sig: 0,
            last_comp: None,
            last_tick: None,
            resize_by_pct: true,
            about_open: false,
            renaming: None,
            rename_fresh: false,
            edit_rows_shown: Default::default(),
            edit_labels: HashMap::new(),
            edit_renaming: None,
            fit_record: None,
            fit_keep_size: theme::get_setting("fit_keep_size").as_deref() == Some("1"),
            about_icon: None,
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
        if let Ok(t) = std::env::var("GIFFORGE_TOOL") {
            if let Some(tool) = Tool::ALL.iter().find(|x| x.label().to_lowercase().starts_with(&t.to_lowercase())) {
                app.tool = *tool;
            }
        }
        if std::env::var("GIFFORGE_COMPARE").is_ok() {
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
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let p2 = progress.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let out = stack::bg_pool().install(|| f(&p2));
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
        let total = self.plan.len() * 2;
        self.spawn(ctx, label, total, move |p| {
            let out = stack.flatten(&base, &fonts, Some(p));
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
        self.spawn(ctx, if transform_only { "Bake frame edits" } else { "Bake everything" }, total, move |p| {
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
            JobOutput::Loaded(Ok((a, path, size))) => {
                self.undo.clear();
                self.redo.clear();
                self.committed = None;
                self.path = path;
                self.file_size = size;
                self.modified = false;
                self.cmp = None;
                self.original = Some(a.clone());
                self.orig_gen += 1;
                self.base = Some(a);
                self.stack = Stack::default();
                self.edit_labels.clear();
                self.edit_rows_shown.clear();
                self.sel.clear();
                self.cur = 0;
                self.elapsed = 0.0;
                self.views = [ViewState::default(); 3];
                self.bump();
                self.after_base_change(true);
                let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sequence".into());
                self.info(format!("Opened {name}"));
                // scripting hook (screenshots): a typical layer stack
                if std::env::var("GIFFORGE_DEMO").is_ok() {
                    self.add_layer(LayerKind::Shield(ShieldSettings::default()));
                    self.add_layer(LayerKind::Effects(Box::new(EffectSettings { saturation: 140.0, vignette: true, ..Default::default() })));
                    self.add_layer(LayerKind::Censor(CensorSettings { x: 200, y: 140, w: 70, h: 60, angle: 12.0, ..Default::default() }));
                    self.add_layer(LayerKind::Text(Annotation { text: "This caption is a really long line of text for testing".into(), size: 22.0, x: 20, y: 190, ..Default::default() }));
                    self.add_layer(LayerKind::Watermark(Box::default()));
                }
                // scripting hook: GIFFORGE_OVERLAY=path adds an image layer on open
                if let Ok(ov) = std::env::var("GIFFORGE_OVERLAY") {
                    if let Ok(a) = io::load_animation(std::path::Path::new(&ov)) {
                        self.add_overlay(OverlaySource { name: ov.rsplit('/').next().unwrap_or("overlay").into(), path: Some(PathBuf::from(&ov)), delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image).clone()).collect() });
                    }
                }
            }
            JobOutput::Info(Ok(s)) => self.info(s),
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
                self.info("Overlay added – drag it on the preview to position it");
            }
            JobOutput::Anim(Err(e)) | JobOutput::Baked(Err(e)) | JobOutput::Loaded(Err(e)) | JobOutput::Info(Err(e)) | JobOutput::Overlay(Err(e)) | JobOutput::Fit(Err(e)) => self.error(format!("{label} failed: {e:#}")),
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
        if self.undo.len() > 100 {
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

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let pal = *p();
        if self.about_icon.is_none() {
            if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon/gifforge-128.png")) {
                self.about_icon = Some(ctx.load_texture("about-icon", to_color_image(&img.to_rgba8()), TextureOptions::LINEAR));
            }
        }
        let mut open = true;
        egui::Window::new(RichText::new("About").font(theme::medium(15.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -30.0))
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.horizontal(|ui| {
                    if let Some(t) = &self.about_icon {
                        ui.add(egui::Image::new(t).fit_to_exact_size(vec2(72.0, 72.0)));
                    }
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("GifForge").font(theme::medium(22.0)).color(pal.fg));
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(pal.fg));
                        let commit = env!("GIFFORGE_COMMIT");
                        let built = if commit.is_empty() { format!("Built {}", env!("GIFFORGE_BUILD_DATE")) } else { format!("Built {} · {commit}", env!("GIFFORGE_BUILD_DATE")) };
                        hint(ui, &built);
                    });
                });
                ui.add_space(8.0);
                ui.label("A GIF editor and optimiser that runs on your own machine.");
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
                ui.add_space(4.0);
                hint(ui, "Licensed under Apache 2.0");
            });
        if !open {
            self.about_open = false;
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
        egui::Window::new(RichText::new("History").font(theme::medium(15.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(260.0)
            .default_height(360.0)
            .default_pos(ctx.content_rect().right_top() + vec2(-640.0, 70.0))
            .show(ctx, |ui| {
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
        let k = match kind {
            LayerKind::Effects(_) => Kind::Effects,
            LayerKind::Text(_) => Kind::Text,
            LayerKind::Censor(_) => Kind::Censor,
            LayerKind::Overlay(_) => Kind::Overlay,
            LayerKind::Watermark(_) => Kind::Watermark,
            LayerKind::Shield(_) => Kind::Shield,
        };
        let id = self.stack.push(kind);
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
            .add_filter("Animations, images & videos", &["gif", "png", "apng", "webp", "jpg", "jpeg", "bmp", "mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "ogv", "3gp"])
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
        let label = if io::is_video(&p) { "Importing video" } else { "Open" };
        self.spawn(ctx, label, 1, move |_| {
            // a video's size says nothing about GIF sizes, so don't compare against it
            let size = if io::is_video(&p) { None } else { std::fs::metadata(&p).ok().map(|m| m.len()) };
            JobOutput::Loaded(io::load_animation(&p).map(|a| (a, Some(p.clone()), size)))
        });
    }

    fn open_sequence(&mut self, ps: Vec<PathBuf>, ctx: &egui::Context) {
        let n = ps.len();
        self.spawn(ctx, "Open sequence", n, move |_| JobOutput::Loaded(io::load_sequence(&ps, 10).map(|a| (a, None, None))));
    }

    fn base_name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "animation".into())
    }

    fn save_gif_dialog(&mut self, ctx: &egui::Context) {
        if self.base.is_none() {
            return;
        }
        let Some(path) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name(format!("{}_edit.gif", self.base_name())).save_file() else { return };
        if let Some(c) = &self.cmp {
            if c.key == self.cmp_key() {
                let r = std::fs::write(&path, &*c.bytes);
                let n = c.bytes.len() as u64;
                match r {
                    Ok(_) => self.info(format!("Saved {} ({})", path.display(), fmt_size(n))),
                    Err(e) => self.error(e.to_string()),
                }
                return;
            }
        }
        let opts = self.gif_opts.clone();
        self.run_export(ctx, "Save GIF", move |a, p| io::save_gif(a, &opts, &path, Some(p)).map(|n| format!("Saved {} ({})", path.display(), fmt_size(n))));
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let mut dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        // Ctrl+V with copied files (file manager "Copy") or pasted paths also opens them
        if !ctx.egui_wants_keyboard_input() {
            let pasted: Vec<String> = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }).collect());
            for t in pasted {
                dropped.extend(paths_from_text(&t));
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
                frames: a.frames.iter().map(|f| (*f.image).clone()).collect(),
            }))
        });
    }

    // ------------------------------------------------------------ keyboard

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (undo, redo, open, save, space, left, right, fit, one, zin, zout, cmp) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                cmd && i.key_pressed(egui::Key::O),
                cmd && (i.key_pressed(egui::Key::S) || i.key_pressed(egui::Key::E)),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                !cmd && i.key_pressed(egui::Key::F),
                !cmd && i.key_pressed(egui::Key::Num1),
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
                !cmd && i.key_pressed(egui::Key::C),
            )
        });
        if self.busy() {
            return;
        }
        if open {
            self.open_dialog(ctx);
        }
        if save && self.base.is_some() {
            self.export_open = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.export_open = false;
            self.presets_open = false;
            self.font_picker_open = false;
            self.about_open = false;
        }
        if typing {
            return;
        }
        if undo {
            self.do_undo();
        }
        if redo {
            self.do_redo();
        }
        if space {
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
        let vi = self.active();
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
        self.elapsed += dt;
        let n = self.plan.len();
        if !self.plan[self.cur].enabled {
            self.cur = self.next_enabled(self.cur);
        }
        let _ = n;
        // Advance at most one frame per repaint. After a slow frame we drop the
        // backlog instead of racing through frames to catch up (no speed bursts).
        let d = effective_delay(self.plan[self.cur].delay) as f64 / 100.0;
        if self.elapsed >= d {
            self.elapsed = (self.elapsed - d).min(d * 0.5);
            self.cur = self.next_enabled(self.cur);
        }
        let d = effective_delay(self.plan[self.cur].delay) as f64 / 100.0;
        let remain = (d - self.elapsed).max(0.004);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(remain));
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
        let key = hash_parts(&[self.version, self.pixelated as u64]);
        if let Some(t) = self.base_tex.get(key, src) {
            return Some(t);
        }
        let img = self.base.as_ref()?.frames.get(src)?.image.clone();
        let o = self.tex_opts();
        Some(self.base_tex.put(ctx, src, &img, o, "base"))
    }

    fn thumb_texture(&mut self, ctx: &egui::Context, idx: usize) -> Option<TextureHandle> {
        if let Some(t) = self.thumb_tex.get(self.version, idx) {
            return Some(t);
        }
        let img = self.base.as_ref()?.frames.get(idx)?.image.clone();
        let s = (160.0 / img.width().max(img.height()) as f32).min(1.0);
        let th = image::imageops::thumbnail(&*img, ((img.width() as f32 * s) as u32).max(1), ((img.height() as f32 * s) as u32).max(1));
        Some(self.thumb_tex.put(ctx, idx, &th, TextureOptions::LINEAR, "thumb"))
    }

    fn composite_key(&self) -> u64 {
        hash_parts(&[self.stack.signature(), self.version, self.timing_rev, self.pixelated as u64, self.plan.len() as u64, self.stack.plan_signature()])
    }

    /// Layer geometry / sprites for the current stack, cached per composite key.
    fn prepared(&mut self) -> Option<Arc<stack::Prepared>> {
        let key = self.comp_key;
        if let Some((k, p)) = &self.prep_cache {
            if *k == key {
                return Some(p.clone());
            }
        }
        let base = self.base.as_ref()?;
        let p = Arc::new(self.stack.prepare(base, &self.fonts));
        self.prep_cache = Some((key, p.clone()));
        Some(p)
    }

    fn render_composite(&mut self, i: usize) -> Option<RgbaImage> {
        if i >= self.plan.len() {
            return None;
        }
        let prep = self.prepared()?;
        let base = self.base.as_ref()?;
        Some(self.stack.render_frame(base, &self.plan, &self.plan_times, i, &self.fonts, &prep))
    }

    /// Frame `i` with effects and layers but without crop / resize / rotate,
    /// so the crop tool can show the whole frame (outside the crop dimmed).
    fn crop_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        let base = self.base.as_ref()?;
        let mut st = self.stack.clone();
        st.transform.crop_on = false;
        st.transform.resize_on = false;
        st.transform.rot_on = false;
        if !st.needs_render() {
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
    fn composite_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        if !self.stack.needs_render() {
            let src = self.plan.get(i)?.src;
            return self.base_texture(ctx, src);
        }
        let key = self.comp_key;
        if let Some(t) = self.comp_tex.get(key, i) {
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
        let img = self.render_composite(i)?;
        let o = self.tex_opts();
        let t = self.comp_tex.put(ctx, i, &img, o, "comp");
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
        let stable = now - self.comp_key_since > 0.25;
        if self.prerender.is_none() && stable && self.stack.needs_render() && !self.plan.is_empty() {
            let cached = self.comp_tex.key == key && self.comp_tex.map.len() >= self.plan.len();
            if cached {
                return;
            }
            let Some(base) = self.base.clone() else { return };
            let stack = self.stack.clone();
            let plan = self.plan.clone();
            let times = self.plan_times.clone();
            let fonts = self.fonts.clone();
            // render in playback order, starting just after the frame on screen
            let n = plan.len();
            let start = self.cur + 1;
            let missing: Vec<usize> = (0..n).map(|k| (start + k) % n).filter(|i| !(self.comp_tex.key == key && self.comp_tex.map.contains_key(i))).collect();
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
                            ctx2.request_repaint();
                        });
                    }
                });
                drop(tx);
            });
            self.prerender = Some(Prerender { key, rx });
        } else if self.prerender.is_none() && !stable && self.stack.needs_render() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    // ------------------------------------------------------------ compare / compressed output

    fn cmp_key(&self) -> u64 {
        hash_parts(&[self.stack.signature(), self.version, self.timing_rev, hash_str(&format!("{:?}", self.preview_format())), self.stack.plan_signature()])
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
            _ => return None,
        })
    }

    fn wants_compressed(&self) -> bool {
        let shown = if self.compare { self.view_left == ViewSource::Compressed || self.view_right == ViewSource::Compressed } else { self.view_single == ViewSource::Compressed };
        self.preview_format().is_some() && (shown || self.tool == Tool::Optimize)
    }

    fn manage_compare(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(j) = &self.cmp_job {
            match j.rx.try_recv() {
                Ok(res) => {
                    let key = j.key;
                    self.cmp_job = None;
                    match res {
                        Ok((anim, bytes)) => {
                            self.cmp_gen += 1;
                            self.cmp = Some(CmpResult { key, anim, bytes: Arc::new(bytes), gen: self.cmp_gen });
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
        if now - self.cmp_key_since < 1.0 || busy_input {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
            return;
        }
        let base = self.base.clone().unwrap();
        let stack = self.stack.clone();
        let fonts = self.fonts.clone();
        let Some(pf) = self.preview_format() else { return };
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let res = stack::bg_pool().install(|| {
                let t0 = std::time::Instant::now();
                let out = stack.flatten(&base, &fonts, None);
                let t1 = t0.elapsed();
                let (anim, bytes) = io::encode_preview(&out, &pf)?;
                let t2 = t0.elapsed();
                if std::env::var_os("GIFFORGE_PERF").is_some() {
                    eprintln!("compress: flatten {:.1}s encode {:.1}s decode {:.1}s", t1.as_secs_f32(), (t2 - t1).as_secs_f32(), (t0.elapsed() - t2).as_secs_f32());
                }
                Ok((anim, bytes))
            });
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
        self.cmp_job = Some(CmpJob { key, rx });
    }

    fn orig_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        let key = hash_parts(&[self.orig_gen, self.pixelated as u64]);
        if let Some(t) = self.orig_tex.get(key, i) {
            return Some(t);
        }
        let img = self.original.as_ref()?.frames.get(i)?.image.clone();
        let o = self.tex_opts();
        Some(self.orig_tex.put(ctx, i, &img, o, "orig"))
    }

    fn cmp_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        let c = self.cmp.as_ref()?;
        let key = hash_parts(&[c.gen, self.pixelated as u64]);
        if let Some(t) = self.cmp_tex.get(key, i) {
            return Some(t);
        }
        let img = c.anim.frames.get(i)?.image.clone();
        let o = self.tex_opts();
        Some(self.cmp_tex.put(ctx, i, &img, o, "cmp"))
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

impl eframe::App for GifForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let perf_t0 = std::time::Instant::now();
        theme::set_current(&ctx);
        let now = ctx.input(|i| i.time);
        self.poll_job(&ctx);
        self.handle_dropped(&ctx);
        self.shortcuts(&ctx);
        self.refresh_plan();
        self.advance(&ctx);
        self.manage_prerender(&ctx, now);
        self.manage_compare(&ctx, now);
        self.load_preview_fonts(&ctx);
        let pal = *p();

        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(pal.header).inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 }))
            .show(ui, |ui| self.toolbar(ui, &ctx));

        egui::Panel::bottom("status").frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(10, 3))).show(ui, |ui| self.status_bar(ui));

        let side_w = if self.sidebar_compact { 46.0 } else { 168.0 };
        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(side_w)
            .frame(egui::Frame::new().fill(pal.view).inner_margin(egui::Margin { left: 5, right: 5, top: 8, bottom: 6 }))
            .show(ui, |ui| self.sidebar(ui));

        if self.base.is_some() {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(350.0)
                .size_range(290.0..=620.0)
                .frame(egui::Frame::new().fill(pal.window))
                .show(ui, |ui| {
                    egui::Panel::bottom("layers_panel")
                        .resizable(!self.layers_collapsed)
                        .default_size(230.0)
                        .size_range(if self.layers_collapsed { 34.0..=34.0 } else { 120.0..=700.0 })
                        .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 6 }))
                        .show(ui, |ui| self.layer_panel(ui, &ctx));
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 10, right: 8, top: 8, bottom: 4 })).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            paint_tool_icon(ui.painter(), self.tool, ir.center(), 18.0, pal.highlight);
                            ui.label(RichText::new(self.tool.title()).font(theme::medium(16.0)).color(pal.fg));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if small_flat(ui, "⊟", "Collapse all sections") {
                                    set_all_groups(ui.ctx(), false);
                                }
                                if small_flat(ui, "⊞", "Expand all sections") {
                                    set_all_groups(ui.ctx(), true);
                                }
                            });
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.add_enabled_ui(!self.busy(), |ui| self.tool_panel(ui, &ctx));
                        });
                    });
                });

            egui::Panel::bottom("transport")
                .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(8, 5)))
                .show(ui, |ui| self.transport(ui));
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.canvas)).show(ui, |ui| {
            if self.base.is_none() {
                self.empty_state(ui, &ctx);
            } else if self.tool == Tool::Frames {
                egui::Frame::new().inner_margin(10.0).show(ui, |ui| self.frames_grid(ui, &ctx));
            } else {
                self.canvas(ui, &ctx);
            }
        });

        self.export_window(&ctx);
        self.history_window(&ctx);
        self.about_window(&ctx);
        self.track_live_edits(&ctx, now);
        self.presets_window(&ctx);
        clear_group_command(&ctx);
        if std::env::var_os("GIFFORGE_PERF").is_some() {
            let ms = perf_t0.elapsed().as_secs_f64() * 1000.0;
            eprintln!("frame {:.1} ms cur={} prerender={}", ms, self.cur, self.prerender.is_some());
        }

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let r = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("gifforge_drop_overlay")));
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

impl GifForgeApp {
    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let idle = !self.busy();
            let has = self.base.is_some();
            if tool_button(ui, "⏏", "Open", idle).on_hover_text("Open a GIF, image or video (Ctrl+O)").clicked() {
                self.open_dialog(ctx);
            }
            if tool_button(ui, "⊞", "Sequence", idle).on_hover_text("Make an animation from a set of images").clicked() {
                self.open_sequence_dialog(ctx);
            }
            toolbar_sep(ui);
            let pending = self.committed.as_ref().map(|c| c.0 != self.live_sig()).unwrap_or(false);
            let undo_tip = self.undo.last().map(|u| format!("Undo {} (Ctrl+Z)", u.label)).unwrap_or_else(|| "Undo (Ctrl+Z)".into());
            if icon_button(ui, "↶", idle && (pending || !self.undo.is_empty())).on_hover_text(undo_tip).clicked() {
                self.do_undo();
            }
            let redo_tip = self.redo.last().map(|u| format!("Redo {} (Ctrl+Y)", u.label)).unwrap_or_else(|| "Redo (Ctrl+Y)".into());
            if icon_button(ui, "↷", idle && !self.redo.is_empty()).on_hover_text(redo_tip).clicked() {
                self.do_redo();
            }
            let hr = ui.add(egui::Button::new("").selected(self.history_open).frame_when_inactive(self.history_open).min_size(vec2(28.0, 28.0))).on_hover_text("History");
            paint_history_icon(ui.painter(), hr.rect.center(), 15.0, if self.history_open { pal.fg } else if hr.hovered() { pal.fg } else { pal.fg_muted });
            if hr.clicked() {
                self.history_open = !self.history_open;
            }
            toolbar_sep(ui);
            let pr = ui.add_enabled(has, egui::Button::new(RichText::new("❖  Presets").color(if self.presets_open { Color32::WHITE } else { pal.fg })).selected(self.presets_open).frame_when_inactive(self.presets_open).min_size(vec2(0.0, TOOLBAR_H)));
            if pr.on_hover_text("Save edits to reuse on other files").clicked() {
                self.presets_open = !self.presets_open;
                if self.presets_open {
                    { let (l, pr) = crate::presets::list_with_problems(); self.presets = l; self.preset_problems = pr; }
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let export = ui
                    .add_enabled(
                        has && idle,
                        egui::Button::new(RichText::new("⬇  Export").font(theme::medium(14.0)).color(Color32::WHITE)).fill(pal.highlight).stroke(Stroke::new(1.0, mix(pal.highlight, Color32::BLACK, 0.25))).min_size(vec2(96.0, TOOLBAR_H)),
                    )
                    .on_hover_text("Save the result (Ctrl+S)");
                if export.clicked() {
                    self.export_open = true;
                }
                let before = self.theme;
                let mut about = false;
                let mr = icon_button(ui, "⋮", true).on_hover_text("Theme, links and About");
                egui::Popup::menu(&mr).show(|ui| {
                    ui.set_min_width(190.0);
                    ui.label(RichText::new("Theme").size(11.5).color(pal.fg_muted));
                    for t in [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light] {
                        ui.selectable_value(&mut self.theme, t, t.label());
                    }
                    if cfg!(all(unix, not(target_os = "macos"))) {
                        ui.separator();
                        let mut native = theme::get_setting("wayland").as_deref() == Some("native");
                        if ui.checkbox(&mut native, "Native Wayland").on_hover_text("Applies after a restart. When off the app runs through XWayland so drag and drop works.").changed() {
                            theme::set_setting("wayland", if native { "native" } else { "xwayland" });
                        }
                    }
                    ui.separator();
                    if ui.button("GitHub page").clicked() {
                        open_external(REPO_URL);
                        ui.close();
                    }
                    if ui.button("Check for updates").on_hover_text("Opens the Releases page").clicked() {
                        open_external(&format!("{REPO_URL}/releases"));
                        ui.close();
                    }
                    if ui.button("Report a problem").clicked() {
                        open_external(&format!("{REPO_URL}/issues"));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("About GifForge").clicked() {
                        about = true;
                        ui.close();
                    }
                });
                if about {
                    self.about_open = true;
                }
                if before != self.theme {
                    self.theme.apply(ctx);
                    theme::save_theme_choice(self.theme);
                }
                // file info
                if let Some(a) = &self.base {
                    let m = |s: String| RichText::new(s).size(12.0).color(pal.fg_muted);
                    let dur: f32 = self.plan.iter().filter(|f| f.enabled).map(|f| effective_delay(f.delay) as f32 / 100.0).sum();
                    let (ow, oh) = self.out_size;
                    ui.add_space(8.0);
                    ui.label(m(format!("{}×{} · {} frames · {:.2}s", ow as u32, oh as u32, self.plan.len(), dur)));
                    if let Some(s) = self.file_size {
                        ui.label(m(fmt_size(s)));
                    }
                    let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
                    let name = elide(&name, 28);
                    ui.label(RichText::new(if self.modified { format!("{name} •") } else { name }).font(theme::medium(13.0)).color(pal.fg)).on_hover_text(if self.modified { "Unsaved changes" } else { "" });
                    let _ = a;
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 3.0, Stroke::new(1.0, pal.border));
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                ui.spinner();
                let done = job.progress.load(Ordering::Relaxed).min(job.total);
                ui.label(RichText::new(format!("{}…", job.label)).size(12.5));
                if job.total > 1 {
                    ui.add(egui::ProgressBar::new(done as f32 / job.total as f32).desired_width(200.0).desired_height(12.0).fill(pal.highlight));
                }
            } else if let Some(s) = &self.status {
                ui.label(RichText::new(&s.text).size(12.5).color(if s.error { pal.negative } else { pal.fg_muted }));
            } else {
                ui.label(RichText::new("Ready").size(12.5).color(pal.fg_muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("?").size(12.5).color(pal.fg_muted)).on_hover_text(
                    "Mouse wheel zooms. Middle or right drag pans.\nAlt+drag pans in drawing tools.\nShift+drag moves in straight lines.\nF fits the view. 1 shows actual size.\nSpace plays or pauses. ← and → step.\nC switches single and split view.\nCtrl+S exports. Ctrl+Z and Ctrl+Y undo and redo.\nCtrl+V opens copied files.",
                );
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let compact = self.sidebar_compact;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let section = |ui: &mut egui::Ui, s: &str| {
                if compact {
                    ui.add_space(4.0);
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.x_range().shrink(6.0), r.top() + 2.0, Stroke::new(1.0, pal.border));
                    ui.add_space(6.0);
                } else {
                    ui.add_space(6.0);
                    ui.label(RichText::new(s.to_uppercase()).size(10.5).color(pal.fg_muted));
                    ui.add_space(1.0);
                }
            };
            for (i, t) in Tool::ALL.iter().copied().enumerate() {
                match i {
                    0 => section(ui, "Layers"),
                    6 => section(ui, "Frame edits"),
                    12 => section(ui, "Frames & output"),
                    _ => {}
                }
                self.sidebar_row(ui, t, compact);
            }
            ui.add_space(8.0);
            let tip = if compact { "Expand sidebar" } else { "Collapse sidebar" };
            if small_flat(ui, if compact { "»" } else { "«" }, tip) {
                self.sidebar_compact = !compact;
                theme::set_setting("sidebar_compact", if compact { "0" } else { "1" });
            }
        });
    }

    fn sidebar_row(&mut self, ui: &mut egui::Ui, t: Tool, compact: bool) {
        let pal = *p();
        let sel = self.tool == t;
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        let painter = ui.painter();
        if sel {
            painter.rect_filled(r, 3.0, pal.highlight_soft);
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, pal.highlight), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_filled(r, 3.0, mix(pal.view, pal.highlight_soft, 0.5));
        }
        let icon_col = if sel { pal.highlight } else { pal.fg };
        let icon_x = if compact { r.center().x } else { r.left() + 16.0 };
        paint_tool_icon(painter, t, pos2(icon_x, r.center().y), 15.0, icon_col);
        if !compact {
            painter.text(r.left_center() + vec2(32.0, 0.0), egui::Align2::LEFT_CENTER, t.label(), egui::FontId::proportional(13.5), pal.fg);
        }
        let tr = &self.stack.transform;
        let badge: Option<String> = match t {
            _ if t.layer_kind().is_some() => {
                let n = self.stack.count(t.layer_kind().unwrap());
                (n > 0).then(|| n.to_string())
            }
            Tool::Resize => tr.resize_on.then(|| "●".into()),
            Tool::Crop => tr.crop_on.then(|| "●".into()),
            Tool::Rotate => tr.rotates().then(|| "●".into()),
            Tool::Reverse => (tr.reverse || tr.boomerang || tr.set_loop).then(|| "●".into()),
            Tool::Speed => tr.speed_on.then(|| "●".into()),
            Tool::Cut => tr.cut_on.then(|| "●".into()),
            _ => None,
        };
        if let Some(b) = badge {
            let c = if compact { r.right_top() + vec2(-7.0, 7.0) } else { r.right_center() - vec2(12.0, 0.0) };
            if b == "●" {
                painter.circle_filled(c, 3.5, pal.positive);
            } else if compact {
                painter.circle_filled(c, 3.5, pal.highlight);
            } else {
                painter.circle_filled(c, 7.5, pal.highlight);
                painter.text(c, egui::Align2::CENTER_CENTER, b, egui::FontId::proportional(10.0), Color32::WHITE);
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let resp = if compact { resp.on_hover_text(format!("{} – {}", t.label(), t.tip())) } else { resp.on_hover_text(t.tip()) };
        if resp.clicked() {
            self.tool = t;
            self.drag = None;
        }
    }

    /// Photoshop-style layer list: top of the stack first, drag to reorder.
    fn layer_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let now = ctx.input(|i| i.time);
        // header
        ui.horizontal(|ui| {
            let arrow = if self.layers_collapsed { "▸" } else { "▾" };
            let h = ui.add(egui::Button::new(RichText::new(format!("{arrow}  Layers")).font(theme::medium(14.0)).color(pal.fg)).frame(false));
            if h.clicked() {
                self.layers_collapsed = !self.layers_collapsed;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_enabled_ui(!self.busy() && !self.stack.is_empty(), |ui| {
                    if icon_button(ui, "⬇", true).on_hover_text("Make all layers and frame edits permanent").clicked() {
                        self.bake(ctx, false);
                    }
                });
                ui.menu_button(RichText::new("+").size(16.0).color(pal.fg), |ui| {
                    for (t, label) in [(Tool::Effects, "Effects"), (Tool::Text, "Text"), (Tool::Censor, "Censor"), (Tool::AddImage, "Image…"), (Tool::Watermark, "Watermark"), (Tool::Shield, "AI shield")] {
                        let b = ui.add(egui::Button::new(format!("      {label}")).min_size(vec2(150.0, 0.0)));
                        paint_tool_icon(ui.painter(), t, pos2(b.rect.left() + 16.0, b.rect.center().y), 13.0, pal.fg);
                        if b.clicked() {
                            self.new_layer_of(t, ctx);
                        }
                    }
                })
                .response
                .on_hover_text("Add layer");
            });
        });
        if self.layers_collapsed {
            return;
        }
        ui.add_space(2.0);
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 3.0, pal.view);
        ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
        let n = self.stack.layers.len();
        let row_h = 30.0;
        let mut select: Option<(Kind, u64)> = None;
        let mut toggle: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut drop_to: Option<(usize, usize)> = None;
        let mut to_frames = false;
        let mut rename_done: Option<Option<(usize, String)>> = None;
        let mut edit_rename_done: Option<Option<(Tool, String)>> = None;
        let mut edit_toggle: Option<Tool> = None;
        let mut edit_remove: Option<Tool> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut rows: Vec<(usize, Rect)> = Vec::new();
            for li in (0..n).rev() {
                let (id, name, kind, visible) = {
                    let l = &self.stack.layers[li];
                    (l.id, l.name(), l.kind(), l.visible)
                };
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
                rows.push((li, r));
                let selected = self.sel.get(kind_key(kind)) == Some(&id) && self.tool == kind_tool(kind);
                let dragging_me = self.layer_drag == Some(id);
                let painter = ui.painter().clone();
                if selected {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 2.0, pal.highlight_soft);
                } else if resp.hovered() {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 2.0, mix(pal.view, pal.highlight_soft, 0.45));
                }
                painter.hline(r.x_range().shrink(4.0), r.bottom(), Stroke::new(1.0, mix(pal.view, pal.border, 0.5)));
                // eye
                let eye_r = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(22.0, 22.0));
                let eye = ui.interact(eye_r, egui::Id::unique(("layer-eye", id)), Sense::click()).on_hover_text("Show / hide");
                painter.text(eye_r.center(), egui::Align2::CENTER_CENTER, if visible { "◉" } else { "○" }, egui::FontId::proportional(14.0), if visible { pal.highlight } else { pal.fg_muted });
                if eye.clicked() {
                    toggle = Some(li);
                }
                // icon + name (elided)
                paint_tool_icon(&painter, kind_tool(kind), pos2(r.left() + 38.0, r.center().y), 14.0, if visible { pal.fg } else { pal.fg_muted });
                let del_w = 22.0;
                let name_w = (r.width() - 52.0 - del_w - 6.0).max(10.0);
                let mut job = egui::text::LayoutJob::simple_singleline(elide(&name, 60), egui::FontId::proportional(13.0), if visible { pal.fg } else { pal.fg_muted });
                job.wrap = egui::text::TextWrapping { max_width: name_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let galley = ui.painter().layout_job(job);
                let gy = r.center().y - galley.size().y / 2.0;
                let renaming = self.renaming.as_ref().map(|(rid, _)| *rid == id).unwrap_or(false);
                if !renaming {
                    painter.galley(pos2(r.left() + 52.0, gy), galley, pal.fg);
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
                let resp = resp.on_hover_text("Click to edit. Double-click to rename. Drag to reorder.");
                if resp.double_clicked() && !eye.hovered() {
                    self.renaming = Some((id, name.clone()));
                    self.rename_fresh = true;
                }
                // delete (pointer anywhere over the row – the ✕ itself takes the hover)
                if ui.rect_contains_pointer(r) || selected {
                    let del_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(20.0, 20.0));
                    let del = ui.interact(del_r, egui::Id::unique(("layer-del", id)), Sense::click()).on_hover_text("Delete layer");
                    painter.text(del_r.center(), egui::Align2::CENTER_CENTER, "✕", egui::FontId::proportional(11.5), if del.hovered() { pal.negative } else { pal.fg_muted });
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
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h - 4.0), Sense::click());
                let painter = ui.painter();
                let sel = self.tool == t;
                if sel {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 2.0, pal.highlight_soft);
                } else if resp.hovered() {
                    painter.rect_filled(r.shrink2(vec2(2.0, 1.0)), 2.0, mix(pal.view, pal.highlight_soft, 0.45));
                }
                let on = enabled.unwrap_or(true);
                let col = if t == Tool::Frames || !on { pal.fg_muted } else { pal.fg };
                let mut eye_hit = false;
                if let Some(on) = enabled {
                    let eye_r = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(22.0, 22.0));
                    let eye = ui.interact(eye_r, egui::Id::unique(("edit-eye", t)), Sense::click()).on_hover_text("Turn this edit on or off");
                    painter.text(eye_r.center(), egui::Align2::CENTER_CENTER, if on { "◉" } else { "○" }, egui::FontId::proportional(13.0), if on { pal.highlight } else { pal.fg_muted });
                    if eye.clicked() {
                        edit_toggle = Some(t);
                    }
                    eye_hit = eye.hovered();
                    if ui.rect_contains_pointer(r) || sel {
                        let del_r = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(20.0, 20.0));
                        let del = ui.interact(del_r, egui::Id::unique(("edit-del", t)), Sense::click()).on_hover_text("Remove this edit");
                        painter.text(del_r.center(), egui::Align2::CENTER_CENTER, "✕", egui::FontId::proportional(11.5), if del.hovered() { pal.negative } else { pal.fg_muted });
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
                    hint(ui, "No layers yet. Add one with +");
                });
            }
        });
        if let Some((k, id)) = select {
            self.tool = kind_tool(k);
            self.select(k, id);
        }
        if let Some(li) = toggle {
            self.stack.layers[li].visible = !self.stack.layers[li].visible;
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

    /// "+" menu / sidebar shortcut for creating a layer of a kind.
    fn new_layer_of(&mut self, t: Tool, ctx: &egui::Context) {
        let (w, h) = self.dims();
        match t {
            Tool::Effects => {
                self.add_layer(LayerKind::Effects(Box::default()));
            }
            Tool::Text => {
                self.add_layer(LayerKind::Text(Annotation { text: "Your text".into(), x: (w as i32 / 2 - 80).max(0), y: (h as i32 - 60).max(0), ..Default::default() }));
            }
            Tool::Censor => {
                self.add_layer(LayerKind::Censor(CensorSettings { x: (w / 4) as i32, y: (h / 4) as i32, w: w / 2, h: h / 2, ..Default::default() }));
            }
            Tool::AddImage => {
                if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "gif", "jpg", "jpeg", "webp", "bmp"]).pick_file() {
                    self.load_overlay(path, ctx);
                }
            }
            Tool::Watermark => {
                self.add_layer(LayerKind::Watermark(Box::default()));
            }
            Tool::Shield => {
                self.add_layer(LayerKind::Shield(ShieldSettings::default()));
            }
            _ => {}
        }
        self.tool = t;
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let r = ui.available_rect_before_wrap();
        let inner = r.shrink(48.0);
        let resp = ui.allocate_rect(inner, Sense::click());
        let stroke_col = if resp.hovered() { pal.highlight } else { pal.border_strong };
        ui.painter().rect_filled(inner, 6.0, mix(pal.canvas, pal.view, 0.4));
        let dash = egui::Shape::dashed_line(&[inner.left_top(), inner.right_top(), inner.right_bottom(), inner.left_bottom(), inner.left_top()], Stroke::new(1.5, stroke_col), 8.0, 6.0);
        ui.painter().extend(dash);
        let c = inner.center();
        ui.painter().text(c - vec2(0.0, 40.0), egui::Align2::CENTER_CENTER, "▤", egui::FontId::proportional(56.0), pal.fg_muted);
        ui.painter().text(c + vec2(0.0, 18.0), egui::Align2::CENTER_CENTER, "Drop a GIF here or click to open", theme::medium(19.0), pal.fg);
        ui.painter().text(c + vec2(0.0, 46.0), egui::Align2::CENTER_CENTER, if self.ffmpeg { "GIF · APNG · WebP · videos · image sequences" } else { "GIF · APNG · WebP · image sequences" }, egui::FontId::proportional(13.5), pal.fg_muted);
        if resp.clicked() && !self.busy() {
            self.open_dialog(ctx);
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 5.0, Stroke::new(1.0, pal.border));
        let n = self.plan.len();
        let (delay, enabled) = self.plan.get(self.cur).map(|f| (f.delay, f.enabled)).unwrap_or((0, true));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if icon_button(ui, "|◀", true).on_hover_text("Previous frame (←)").clicked() {
                self.step(-1);
            }
            if ui.add(egui::Button::new(if self.playing { "❚❚" } else { "▶" }).min_size(vec2(34.0, 26.0))).on_hover_text("Play / pause (Space)").clicked() {
                self.playing = !self.playing;
            }
            if icon_button(ui, "▶|", true).on_hover_text("Next frame (→)").clicked() {
                self.step(1);
            }
            ui.add_space(4.0);
            let right_w = if self.compare { 430.0 } else { 400.0 };
            ui.spacing_mut().slider_width = (ui.available_width() - right_w - 90.0).max(80.0);
            let mut f = self.cur + 1;
            if ui.add(egui::Slider::new(&mut f, 1..=n.max(1)).show_value(false)).changed() {
                self.cur = f - 1;
                self.playing = false;
                self.elapsed = 0.0;
            }
            ui.label(RichText::new(format!("{}/{}", self.cur + 1, n)).size(12.5)).on_hover_text(format!("Delay {delay} cs{}", if enabled { "" } else { " · skipped" }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if toggle_icon(ui, "▩", self.pixelated, "Pixelated preview (nearest neighbour)") {
                    self.pixelated = !self.pixelated;
                }
                if toggle_icon(ui, "▦", self.checker, "Checkerboard behind transparency") {
                    self.checker = !self.checker;
                }
                toolbar_sep(ui);
                if self.compare && toggle_icon(ui, "⚭", self.link_views, "Link zoom and pan of both views") {
                    self.link_views = !self.link_views;
                }
                if icon_button(ui, "1:1", true).on_hover_text("Actual size (1)").clicked() {
                    self.zoom_to(1.0);
                }
                if ui.add(egui::Button::new("Fit").selected(self.views[self.active()].zoom.is_none()).min_size(vec2(0.0, 24.0))).on_hover_text("Fit (F)").clicked() {
                    self.zoom_fit();
                }
                if icon_button(ui, "+", true).on_hover_text("Zoom in").clicked() {
                    self.zoom_to(self.cur_scale() * 1.25);
                }
                let mut pct = self.cur_scale() * 100.0;
                if ui.add(egui::DragValue::new(&mut pct).range(2.0..=6400.0).speed(1.0).suffix("%").max_decimals(0)).on_hover_text("Zoom").changed() {
                    self.zoom_to(pct / 100.0);
                }
                if icon_button(ui, "−", true).on_hover_text("Zoom out").clicked() {
                    self.zoom_to(self.cur_scale() / 1.25);
                }
                toolbar_sep(ui);
                // single / split segmented control
                if segment(ui, "◫", self.compare, "Split view (C)") {
                    self.compare = true;
                }
                if segment(ui, "▭", !self.compare, "Single view (C)") {
                    self.compare = false;
                }
            });
        });
    }

    // ------------------------------------------------------------ export

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_open || self.base.is_none() {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut go = false;
        egui::Window::new(RichText::new("Export").font(theme::medium(15.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0))
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for f in ExportFormat::ALL {
                        let en = !f.needs_ffmpeg() || self.ffmpeg;
                        let r = ui.add_enabled(en, egui::Button::new(f.label()).selected(self.export_fmt == f).min_size(vec2(0.0, 26.0)));
                        let r = if en { r.on_hover_text(f.tip()) } else { r.on_disabled_hover_text("Needs ffmpeg on PATH") };
                        if r.clicked() {
                            self.export_fmt = f;
                        }
                    }
                });
                ui.add_space(8.0);
                egui::Frame::new().fill(mix(pal.window, pal.view, 0.35)).stroke(Stroke::new(1.0, pal.border)).corner_radius(3.0).inner_margin(10.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.export_fmt {
                        ExportFormat::Gif => {
                            let o = &self.gif_opts;
                            ui.label(format!("{} colours{} · lossy {}", o.colors, if o.global_palette { " (global)" } else { "" }, o.lossy));
                            if let Some(c) = &self.cmp {
                                if c.key == self.cmp_key() {
                                    let n = c.bytes.len() as u64;
                                    let mut t = format!("Size: {}", fmt_size(n));
                                    if let Some(orig) = self.file_size {
                                        t.push_str(&format!(" ({:+.0}%)", (n as f64 / orig as f64 - 1.0) * 100.0));
                                    }
                                    ui.label(RichText::new(t).color(pal.positive));
                                }
                            }
                            if ui.link("Compression settings…").clicked() {
                                self.tool = Tool::Optimize;
                                self.export_open = false;
                            }
                            ui.checkbox(&mut self.gif_opts.noai_tag, "Add “noai” tag").on_hover_text("Adds a “noai, noimageai” note to the file. Some sites and scrapers respect it.");
                        }
                        ExportFormat::Webp => {
                            let o = &mut self.webp_opts;
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut o.lossless, false, "Lossy");
                                ui.selectable_value(&mut o.lossless, true, "Lossless");
                            });
                            if !o.lossless {
                                ui.add(egui::Slider::new(&mut o.quality, 0.0..=100.0).text("quality")).on_hover_text("75–90 looks like the original for most GIFs");
                            }
                            ui.add(egui::Slider::new(&mut o.method, 0..=6).text("effort")).on_hover_text("Higher makes smaller files but takes longer");
                        }
                        ExportFormat::Avif => {
                            ui.add(egui::Slider::new(&mut self.avif_quality, 0..=100).text("quality")).on_hover_text("50–70 is a good balance");
                            hint(ui, "Encoding can take a while");
                        }
                        ExportFormat::Apng => {
                            ui.label("24-bit colour, full alpha");
                            ui.checkbox(&mut self.gif_opts.noai_tag, "Add “noai” tag");
                        }
                        ExportFormat::Mp4 => {
                            ui.horizontal(|ui| {
                                ui.label("Background");
                                ui.color_edit_button_srgb(&mut self.video_bg).on_hover_text("Fills transparent areas");
                            });
                        }
                        ExportFormat::Webm => {
                            ui.label("VP9 with alpha");
                        }
                        ExportFormat::Frames => {
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut self.split_fmt, SplitFormat::Png, "PNG");
                                ui.selectable_value(&mut self.split_fmt, SplitFormat::Gif, "GIF");
                                ui.separator();
                                ui.selectable_value(&mut self.frames_zip, false, "Folder");
                                ui.selectable_value(&mut self.frames_zip, true, "ZIP").on_hover_text("ZIP files always contain PNGs");
                            });
                            ui.horizontal(|ui| {
                                ui.label("Prefix");
                                ui.add(egui::TextEdit::singleline(&mut self.split_prefix).desired_width(140.0));
                            });
                        }
                        ExportFormat::Sheet => {
                            let n = self.plan.iter().filter(|f| f.enabled).count().max(1) as u32;
                            let (fw, fh) = (self.out_size.0 as u32, self.out_size.1 as u32);
                            let s = &mut self.sheet;
                            ui.horizontal(|ui| {
                                ui.label("Columns");
                                ui.add(egui::DragValue::new(&mut s.columns).range(0..=n)).on_hover_text("0 picks a layout automatically");
                                ui.label("Padding");
                                ui.add(egui::DragValue::new(&mut s.padding).range(0..=256).suffix(" px"));
                            });
                            let cols = if s.columns == 0 { (n as f32).sqrt().ceil() as u32 } else { s.columns.min(n) }.max(1);
                            let rows = n.div_ceil(cols);
                            hint(ui, &format!("{cols}×{rows} · {}×{} px", cols * fw + (cols - 1) * s.padding, rows * fh + (rows - 1) * s.padding));
                            ui.checkbox(&mut s.write_json, "JSON metadata");
                            ui.checkbox(&mut s.write_godot, "Godot SpriteFrames (.tres)").on_hover_text("Assign the .tres to an AnimatedSprite2D/3D. Frame delays are kept.");
                            if s.write_godot {
                                ui.horizontal(|ui| {
                                    ui.label("res://");
                                    ui.add(egui::TextEdit::singleline(&mut s.godot_res_dir).desired_width(140.0)).on_hover_text("Where the PNG will sit in your Godot project");
                                    ui.label("Name");
                                    ui.add(egui::TextEdit::singleline(&mut s.anim_name).desired_width(70.0));
                                });
                            }
                        }
                    }
                });
                if self.export_fmt.can_target() {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.target_on, "Fit under").on_hover_text("Finds the best quality that fits. Quality drops first and size only if needed.");
                        ui.add_enabled_ui(self.target_on, |ui| {
                            ui.add(egui::DragValue::new(&mut self.target_val).range(0.01..=10000.0).speed(0.1).max_decimals(2));
                            ui.selectable_value(&mut self.target_mb, true, "MiB");
                            ui.selectable_value(&mut self.target_mb, false, "KiB");
                        });
                    });
                    if self.target_on {
                        ui.horizontal_wrapped(|ui| {
                            for (lbl, v, mb) in TARGET_PRESETS {
                                if chip(ui, lbl).clicked() {
                                    self.target_val = v;
                                    self.target_mb = mb;
                                }
                            }
                        });
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    go = primary(ui, &format!("Export {}…", self.export_fmt.label()));
                });
            });
        if !open {
            self.export_open = false;
        }
        if go {
            self.export_open = false;
            self.do_export(ctx);
        }
    }

    fn do_export(&mut self, ctx: &egui::Context) {
        let name = self.base_name();
        if self.target_on && self.export_fmt.can_target() {
            let (ext, fmt) = match self.export_fmt {
                ExportFormat::Gif => ("gif", crate::target::TargetFormat::Gif(self.gif_opts.clone())),
                ExportFormat::Webp => ("webp", crate::target::TargetFormat::Webp(self.webp_opts)),
                _ => ("avif", crate::target::TargetFormat::Avif(self.avif_quality)),
            };
            let max = self.target_bytes();
            if let Some(path) = rfd::FileDialog::new().add_filter(ext, &[ext]).set_file_name(format!("{name}.{ext}")).save_file() {
                self.run_export(ctx, &format!("Fitting under {}", fmt_size(max)), move |a, p| {
                    let fit = crate::target::fit(a, &fmt, max, p)?;
                    std::fs::write(&path, &fit.bytes)?;
                    let n = fit.bytes.len() as u64;
                    if fit.reached {
                        Ok(format!("Saved {} ({}, {})", path.display(), fmt_size(n), fit.settings))
                    } else {
                        Ok(format!("Couldn't reach target – smallest is {} ({}). Saved anyway.", fmt_size(n), fit.settings))
                    }
                });
            }
            return;
        }
        // the preview already holds the encoded file: save it directly
        if self.export_fmt != ExportFormat::Gif && self.preview_format().is_some() && !(self.export_fmt == ExportFormat::Apng && self.gif_opts.noai_tag) {
            if let Some(c) = self.cmp.as_ref().filter(|c| c.key == self.cmp_key()) {
                let ext = match self.export_fmt {
                    ExportFormat::Webp => "webp",
                    ExportFormat::Avif => "avif",
                    ExportFormat::Apng => "png",
                    ExportFormat::Mp4 => "mp4",
                    _ => "webm",
                };
                let bytes = c.bytes.clone();
                if let Some(path) = rfd::FileDialog::new().add_filter(ext, &[ext]).set_file_name(format!("{name}.{ext}")).save_file() {
                    match std::fs::write(&path, &*bytes) {
                        Ok(_) => self.info(format!("Saved {} ({})", path.display(), fmt_size(bytes.len() as u64))),
                        Err(e) => self.error(e.to_string()),
                    }
                }
                return;
            }
        }
        match self.export_fmt {
            ExportFormat::Webp => {
                if let Some(path) = rfd::FileDialog::new().add_filter("WebP", &["webp"]).set_file_name(format!("{name}.webp")).save_file() {
                    let o = self.webp_opts;
                    self.run_export(ctx, "Save WebP", move |a, _| {
                        let b = io::encode_webp(a, &o)?;
                        std::fs::write(&path, &b)?;
                        Ok(format!("Saved {} ({})", path.display(), fmt_size(b.len() as u64)))
                    });
                }
            }
            ExportFormat::Avif => {
                if let Some(path) = rfd::FileDialog::new().add_filter("AVIF", &["avif"]).set_file_name(format!("{name}.avif")).save_file() {
                    let q = self.avif_quality;
                    self.run_export(ctx, "Save AVIF", move |a, _| {
                        let b = io::encode_avif(a, q)?;
                        std::fs::write(&path, &b)?;
                        Ok(format!("Saved {} ({})", path.display(), fmt_size(b.len() as u64)))
                    });
                }
            }
            ExportFormat::Gif => self.save_gif_dialog(ctx),
            ExportFormat::Apng => {
                if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png", "apng"]).set_file_name(format!("{name}.png")).save_file() {
                    let tag = self.gif_opts.noai_tag;
                    self.run_export(ctx, "Save APNG", move |a, _| io::save_apng(a, &path, tag).map(|n| format!("Saved {} ({})", path.display(), fmt_size(n))));
                }
            }
            ExportFormat::Mp4 | ExportFormat::Webm => {
                let (fmt, ext) = if self.export_fmt == ExportFormat::Mp4 { (VideoFormat::Mp4, "mp4") } else { (VideoFormat::WebmAlpha, "webm") };
                if let Some(path) = rfd::FileDialog::new().add_filter(ext, &[ext]).set_file_name(format!("{name}.{ext}")).save_file() {
                    let bg = self.video_bg;
                    self.run_export(ctx, "Export video", move |a, _| io::export_video(a, &path, fmt, bg).map(|_| format!("Saved {}", path.display())));
                }
            }
            ExportFormat::Frames => {
                let pre = self.split_prefix.clone();
                if self.frames_zip {
                    if let Some(path) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).set_file_name(format!("{name}_frames.zip")).save_file() {
                        self.run_export(ctx, "ZIP frames", move |a, _| io::export_frames_zip(a, &path, &pre).map(|n| format!("Wrote {n} frames to {}", path.display())));
                    }
                } else if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    let fmt = self.split_fmt;
                    self.run_export(ctx, "Split frames", move |a, _| io::export_frames_dir(a, &dir, &pre, fmt).map(|n| format!("Wrote {n} frames to {}", dir.display())));
                }
            }
            ExportFormat::Sheet => {
                if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("{name}_sheet.png")).save_file() {
                    let opts = self.sheet.clone();
                    self.run_export(ctx, "Sprite sheet", move |a, _| io::export_spritesheet(a, &path, &opts).map(|s| format!("Wrote {s}")));
                }
            }
        }
    }

    // ------------------------------------------------------------ presets

    fn presets_window(&mut self, ctx: &egui::Context) {
        if !self.presets_open || self.base.is_none() {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut apply: Option<crate::presets::Preset> = None;
        let mut delete: Option<String> = None;
        let mut save = false;
        egui::Window::new(RichText::new("Presets").font(theme::medium(15.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .default_pos(ctx.content_rect().left_top() + vec2(260.0, 70.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Save current").font(theme::medium(13.0)));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Name").desired_width(220.0));
                    save = ui.add_enabled(!self.preset_name.trim().is_empty(), egui::Button::new("Save")).clicked();
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.preset_inc.0, "Frame edits");
                    ui.checkbox(&mut self.preset_inc.1, "Layers");
                    ui.checkbox(&mut self.preset_inc.2, "Compression");
                });
                ui.horizontal(|ui| {
                    if let Some(dir) = crate::presets::folder() {
                        if chip(ui, "Open folder").on_hover_text(dir.display().to_string()).clicked() {
                            let _ = std::fs::create_dir_all(&dir);
                            open_in_file_manager(&dir);
                        }
                    }
                });
                ui.add_space(6.0);
                ui.separator();
                if self.presets.is_empty() {
                    hint(ui, "No presets yet");
                }
                if !self.preset_problems.is_empty() {
                    ui.label(RichText::new(format!("⚠ {} problem(s) reading presets", self.preset_problems.len())).color(pal.neutral).size(12.0)).on_hover_text(self.preset_problems.join("\n"));
                }
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    for pr in &self.presets {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(elide(&pr.name, 30)).color(pal.fg));
                                hint(ui, &pr.summary());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if small_flat(ui, "✕", "Delete preset") {
                                    delete = Some(pr.name.clone());
                                }
                                if ui.button("Apply").clicked() {
                                    apply = Some(pr.clone());
                                }
                            });
                        });
                        ui.add_space(2.0);
                    }
                });
            });
        if !open {
            self.presets_open = false;
        }
        if save {
            let pr = self.make_preset(self.preset_name.trim().to_string());
            match crate::presets::save(&pr) {
                Ok(_) => {
                    self.info(format!("Saved preset “{}”", pr.name));
                    self.preset_name.clear();
                }
                Err(e) => self.error(format!("Could not save preset: {e:#}")),
            }
            { let (l, pr) = crate::presets::list_with_problems(); self.presets = l; self.preset_problems = pr; }
        }
        if let Some(name) = delete {
            crate::presets::delete(&name);
            { let (l, pr) = crate::presets::list_with_problems(); self.presets = l; self.preset_problems = pr; }
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
                    })
                })
                .collect()
        });
        Preset { name, src_size: self.dims(), transform: inc_t.then(|| self.stack.transform.clone()), layers, gif: inc_g.then(|| self.gif_opts.clone()) }
    }

    fn apply_preset(&mut self, pr: &crate::presets::Preset, ctx: &egui::Context) {
        use crate::presets::PresetLayer;
        let now = ctx.input(|i| i.time);
        self.snapshot(&format!("preset{now}"), now);
        let (w, h) = self.dims();
        let (sw, sh) = (pr.src_size.0.max(1) as f32, pr.src_size.1.max(1) as f32);
        let (fx, fy) = if pr.src_size.0 == 0 { (1.0, 1.0) } else { (w as f32 / sw, h as f32 / sh) };
        let fs = fx.min(fy);
        let sc = |v: i32, f: f32| (v as f32 * f).round() as i32;
        if let Some(t) = &pr.transform {
            let mut t = t.clone();
            t.crop = [(t.crop[0] as f32 * fx) as u32, (t.crop[1] as f32 * fy) as u32, (t.crop[2] as f32 * fx) as u32, (t.crop[3] as f32 * fy) as u32];
            self.stack.transform = t;
        }
        if let Some(g) = &pr.gif {
            self.gif_opts = g.clone();
        }
        let mut missing = Vec::new();
        if let Some(layers) = &pr.layers {
            let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
            for l in layers {
                let id = match l.clone() {
                    PresetLayer::Effects { visible, fx: e } => (self.stack.push(LayerKind::Effects(Box::new(e))), visible),
                    PresetLayer::Text { visible, mut text, font } => {
                        text.x = sc(text.x, fx);
                        text.y = sc(text.y, fy);
                        text.size *= fs;
                        text.font_idx = self.resolve_font(&font);
                        (self.stack.push(LayerKind::Text(text)), visible)
                    }
                    PresetLayer::Censor { visible, mut censor } => {
                        censor.x = sc(censor.x, fx);
                        censor.y = sc(censor.y, fy);
                        censor.w = (censor.w as f32 * fx) as u32;
                        censor.h = (censor.h as f32 * fy) as u32;
                        (self.stack.push(LayerKind::Censor(censor)), visible)
                    }
                    PresetLayer::Image { visible, path, mut placement, opacity } => match io::load_animation(&path) {
                        Ok(a) => {
                            placement.x = sc(placement.x, fx);
                            placement.y = sc(placement.y, fy);
                            placement.w = (placement.w as f32 * fs).max(1.0) as u32;
                            placement.h = (placement.h as f32 * fs).max(1.0) as u32;
                            let src = OverlaySource { name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), path: Some(path.clone()), delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image).clone()).collect() };
                            (self.stack.push(LayerKind::Overlay(OverlayLayer { src: Arc::new(src), placements: vec![placement; n], opacity })), visible)
                        }
                        Err(_) => {
                            missing.push(path.display().to_string());
                            continue;
                        }
                    },
                    PresetLayer::Watermark { visible, mut watermark } => {
                        watermark.font_idx = self.resolve_font(&watermark.font_key);
                        if let Some(p) = &watermark.image_path {
                            watermark.image = io::load_still(p).ok().map(Arc::new);
                        }
                        (self.stack.push(LayerKind::Watermark(Box::new(watermark))), visible)
                    }
                    PresetLayer::Shield { visible, shield } => (self.stack.push(LayerKind::Shield(shield)), visible),
                };
                if let Some(i) = self.stack.index_of(id.0) {
                    self.stack.layers[i].visible = id.1;
                }
            }
        }
        self.modified = true;
        self.refresh_plan();
        if missing.is_empty() {
            self.info(format!("Applied preset “{}”", pr.name));
        } else {
            self.error(format!("Applied “{}”; missing images: {}", pr.name, missing.join(", ")));
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
        let faces: Vec<(String, crate::sysfonts::SysFace)> = {
            let Ok(g) = self.sys_fonts.lock() else { return };
            let Some(fams) = g.as_ref() else { return };
            batch.iter().filter_map(|n| fams.iter().find(|f| &f.name == n).map(|f| (n.clone(), f.default_face().clone()))).collect()
        };
        for (name, face) in faces {
            self.preview_fonts.insert(name.clone(), false);
            let ok = std::fs::metadata(&face.path).map(|m| m.len() < 12_000_000).unwrap_or(false);
            if !ok {
                continue;
            }
            if let Ok(data) = std::fs::read(&face.path) {
                let mut fd = egui::FontData::from_owned(data);
                fd.index = face.index;
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
    fn viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, vi: usize, rect: Rect, tex: Option<TextureHandle>, img_size: Vec2, label: Option<(String, Color32)>, badge: Option<String>, busy: bool) -> (egui::Response, Mapper) {
        let pal = *p();
        let fit = ((rect.width() - 24.0) / img_size.x).min((rect.height() - 24.0) / img_size.y).clamp(0.02, 64.0);
        self.views[vi].fit = fit;
        let scale = self.views[vi].zoom.unwrap_or(fit);
        let center = rect.center() + self.views[vi].pan;
        let img_rect = Rect::from_center_size(center, img_size * scale);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, pal.canvas);
        let vis = img_rect.intersect(rect);
        if vis.is_positive() {
            painter.rect_filled(img_rect.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(60));
            if self.checker {
                let ct = self.checker(ctx);
                let uv = Rect::from_min_max(((vis.min - img_rect.min) / 16.0).to_pos2(), ((vis.max - img_rect.min) / 16.0).to_pos2());
                painter.image(ct.id(), vis, uv, Color32::WHITE);
            } else {
                painter.rect_filled(vis, 0.0, Color32::WHITE);
            }
            if let Some(t) = &tex {
                let ts = t.size_vec2();
                let tr = Rect::from_min_size(img_rect.min, ts * scale);
                painter.image(t.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        let resp = ui.interact(rect, egui::Id::unique(("gifforge-viewport", vi)), Sense::click_and_drag());
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
        (resp, Mapper { rect: img_rect, scale })
    }

    /// Index of the original-file frame to show: the same source frame when the
    /// frame structure is unchanged, otherwise by time.
    fn original_index(&self) -> Option<usize> {
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
                    Some(c) if c.key == self.cmp_key() => {
                        let n = c.bytes.len() as u64;
                        match self.file_size {
                            Some(o) => format!("{f} · {} ({:+.0}%)", fmt_size(n), (n as f64 / o as f64 - 1.0) * 100.0),
                            None => format!("{f} · {}", fmt_size(n)),
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
        let text = if info.is_empty() { format!("{}  ▾", src.label()) } else { format!("{}  ·  {}  ▾", src.label(), info) };
        let area = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2((rect.width() - 20.0).max(40.0), 28.0));
        let mut pick = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            let r = ui.add(
                egui::Button::new(RichText::new(text).font(theme::medium(12.5)).color(Color32::WHITE))
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
                    if let Some(t) = self.composite_texture(ctx, self.cur) {
                        let s = t.size_vec2();
                        self.out_size = (s.x, s.y);
                    }
                    let s = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
                    let (w, h) = self.dims();
                    let live = if self.crop_live { self.crop_texture(ctx, self.cur) } else { None };
                    (live.or_else(|| self.base_texture(ctx, s)), vec2(w as f32, h as f32))
                } else {
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = tex.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(self.out_size.0, self.out_size.1));
                    self.out_size = (size.x, size.y);
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
                let tex = self.orig_texture(ctx, i);
                let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, None, None, false);
                if same {
                    self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Source, vi);
                }
            }
            ViewSource::Compressed => {
                let encoding = self.cmp_job.is_some();
                let fresh = self.cmp.as_ref().map(|c| c.key == self.cmp_key()).unwrap_or(false);
                if fresh {
                    let (i, size, n) = {
                        let c = self.cmp.as_ref().unwrap();
                        (stack::frame_at_time(&c.anim, t), vec2(c.anim.width() as f32, c.anim.height() as f32), c.bytes.len() as u64)
                    };
                    let _ = n;
                    let tex = self.cmp_texture(ctx, i);
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, None, None, false);
                    if !crop_mode {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output, vi);
                    }
                } else {
                    // compression pending / running: show the live preview with a warning
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = tex.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(self.out_size.0, self.out_size.1));
                    let badge = if self.preview_format().is_none() {
                        if self.export_fmt.needs_ffmpeg() && !self.ffmpeg { "⚠ Needs ffmpeg to preview" } else { "Uncompressed output" }
                    } else if encoding {
                        "⚠ Compressing"
                    } else {
                        "⚠ Not compressed yet"
                    };
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
        let left = Rect::from_min_max(full.min, pos2(full.center().x - gap / 2.0, full.max.y));
        let right = Rect::from_min_max(pos2(full.center().x + gap / 2.0, full.min.y), full.max);
        ui.painter().rect_filled(Rect::from_min_max(pos2(left.max.x, full.min.y), pos2(right.min.x, full.max.y)), 0.0, pal.border);
        let (l, r) = (self.view_left, self.view_right);
        self.source_view(ui, ctx, left, l, 1, true);
        self.source_view(ui, ctx, right, r, 2, true);
    }

    /// Canvas editing. `space` says which coordinates the viewport shows:
    /// source frames (original / crop view) or the edited output.
    fn canvas_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response, m: Mapper, bounds: (f32, f32), space: Space, vi: usize) {
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
        if kind == Kind::Effects {
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
                _ => true,
            };
            if !applies {
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

        // ---- start of a drag: decide what is being edited
        if started {
            if let (Some(sp), Some(pi)) = (resp.interact_pointer_pos(), ptr_img) {
                let mut new_drag = None;
                if let Some((li, id, b, o)) = sel {
                    if rot_handle(&o).distance(sp) <= HANDLE_HIT {
                        let a0 = (pi.y - o.cy).atan2(pi.x - o.cx).to_degrees();
                        new_drag = Some(BoxDrag::Rotate { id, li, a0, out0: o, src: b });
                    } else if let Some(k) = o.corners().iter().position(|c| to_screen(*c).distance(sp) <= HANDLE_HIT) {
                        let f = o.to_local(o.corners()[(k + 2) % 4].0, o.corners()[(k + 2) % 4].1);
                        let size0 = match &self.stack.layers[li].kind {
                            LayerKind::Text(t) => t.size,
                            _ => 0.0,
                        };
                        let d0 = ((pi.x - o.cx).powi(2) + (pi.y - o.cy).powi(2)).sqrt().max(1.0);
                        new_drag = Some(BoxDrag::Resize { id, li, fixed: f, out0: o, src: b, size0, d0 });
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
                        let id = self.add_layer(LayerKind::Censor(CensorSettings::default()));
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
                        BoxDrag::Resize { fixed, out0, src, size0, d0, .. } => {
                            if size0 > 0.0 {
                                let d = ((pi.x - out0.cx).powi(2) + (pi.y - out0.cy).powi(2)).sqrt();
                                let ns = (size0 * d / d0).clamp(4.0, 4000.0);
                                self.write_box(li, src, Some(ns), src_idx);
                            } else {
                                let lp = out0.to_local(pi.x, pi.y);
                                let (mut w, mut h) = ((lp.0 - fixed.0).abs().max(1.0), (lp.1 - fixed.1).abs().max(1.0));
                                if kind == Kind::Overlay && !shift {
                                    let r = out0.w / out0.h.max(1e-3);
                                    w = w.max(h * r);
                                    h = w / r;
                                }
                                let sx = if lp.0 >= fixed.0 { 1.0 } else { -1.0 };
                                let sy = if lp.1 >= fixed.1 { 1.0 } else { -1.0 };
                                let lc = (fixed.0 + sx * w / 2.0, fixed.1 + sy * h / 2.0);
                                let c = out0.from_local(lc.0, lc.1);
                                let out = Obb { cx: c.0, cy: c.1, w, h, angle: out0.angle };
                                self.write_box(li, unmap_box(&mp, out, src.angle), None, src_idx);
                            }
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
            let mp = maps2.as_ref().map(|m| m[li].m).unwrap_or(Affine::IDENTITY);
            let o = map_box(&mp, b, l.readable());
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
                        } else if c.3.contains(pi.x, pi.y) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                        }
                    }
                }
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
            LayerKind::Effects(_) | LayerKind::Watermark(_) | LayerKind::Shield(_) => {}
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
            ui.label(RichText::new("ℹ").color(pal.fg_muted)).on_hover_text("Changes here apply under all live edits. Skipped frames are left out of playback and export.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.fr_thumb, 56.0..=200.0).show_value(false));
                ui.label("Size");
            });
        });
        ui.add_space(6.0);
        let cell_w = self.fr_thumb + 24.0;
        let mut action: Option<(usize, &'static str)> = None;
        let mut delay_change: Option<(usize, u16)> = None;
        let cols = (((ui.available_width() - 16.0) / (cell_w + 8.0)).floor() as usize).max(1);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for row in 0..n.div_ceil(cols) {
                ui.horizontal(|ui| {
                    for i in row * cols..((row + 1) * cols).min(n) {
                        let (enabled, mut delay) = {
                            let f = &self.base.as_ref().unwrap().frames[i];
                            (f.enabled, f.delay_cs)
                        };
                        let in_cut = i >= cs && i <= ce;
                        let tex = self.thumb_texture(ctx, i);
                        let sel = i == sel_src;
                        egui::Frame::new()
                            .fill(if sel { pal.highlight_soft } else { pal.view })
                            .stroke(Stroke::new(1.0, if sel { pal.highlight } else { pal.border }))
                            .corner_radius(3.0)
                            .inner_margin(6.0)
                            .show(ui, |ui| {
                                ui.set_width(cell_w - 12.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(format!("#{}{}", i + 1, if in_cut { "" } else { " · cut" })).size(11.5).color(pal.fg_muted));
                                    let box_size = vec2(self.fr_thumb, self.fr_thumb);
                                    let (r, resp) = ui.allocate_exact_size(box_size, Sense::click());
                                    ui.painter().rect_filled(r, 2.0, Color32::from_gray(200));
                                    if let Some(t) = &tex {
                                        let ts = t.size_vec2();
                                        let s = (box_size.x / ts.x).min(box_size.y / ts.y);
                                        let ir = Rect::from_center_size(r.center(), ts * s);
                                        ui.painter().image(t.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if enabled && in_cut { Color32::WHITE } else { Color32::from_white_alpha(60) });
                                    }
                                    if !enabled {
                                        ui.painter().line_segment([r.left_top(), r.right_bottom()], Stroke::new(2.0, pal.negative));
                                        ui.painter().line_segment([r.right_top(), r.left_bottom()], Stroke::new(2.0, pal.negative));
                                    }
                                    if resp.clicked() {
                                        action = Some((i, "select"));
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Delay").size(11.5));
                                        if ui.add(egui::DragValue::new(&mut delay).range(1..=6000).speed(0.2)).changed() {
                                            delay_change = Some((i, delay));
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if tinted(ui, if enabled { "skip" } else { "enable" }, if enabled { pal.negative } else { pal.highlight }) {
                                            action = Some((i, "toggle"));
                                        }
                                        if tinted(ui, "copy", pal.highlight) {
                                            action = Some((i, "copy"));
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if chip(ui, "◀").on_hover_text("Move left").clicked() {
                                            action = Some((i, "left"));
                                        }
                                        if chip(ui, "▶").on_hover_text("Move right").clicked() {
                                            action = Some((i, "right"));
                                        }
                                        if chip(ui, "✕").on_hover_text("Delete frame").clicked() {
                                            action = Some((i, "delete"));
                                        }
                                    });
                                });
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
        match self.tool {
            Tool::Resize => self.ui_resize(ui, ctx),
            Tool::Crop => self.ui_crop(ui, ctx),
            Tool::Rotate => self.ui_rotate(ui, ctx),
            Tool::Reverse => self.ui_reverse(ui, ctx),
            Tool::Speed => self.ui_speed(ui, ctx),
            Tool::Optimize => self.ui_optimize(ui, ctx),
            Tool::Effects => self.ui_effects(ui, ctx),
            Tool::Text => self.ui_text(ui, ctx),
            Tool::Censor => self.ui_censor(ui, ctx),
            Tool::AddImage => self.ui_add_image(ui, ctx),
            Tool::Watermark => self.ui_watermark(ui, ctx),
            Tool::Shield => self.ui_shield(ui, ctx),
            Tool::Cut => self.ui_cut(ui, ctx),
            Tool::Frames => self.ui_frames(ui, ctx),
        }
    }

    /// Enable switch, reset and bake for live frame-edit tools.
    fn transform_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, on: impl Fn(&mut Transform) -> &mut bool, reset: impl Fn(&mut Transform)) {
        let mut enabled = *on(&mut self.stack.transform);
        let mut do_reset = false;
        let mut do_bake = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut enabled, "Enabled");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                do_bake = ui.button("⬇ Bake").on_hover_text("Make frame edits permanent. Layers stay editable.").clicked();
                do_reset = ui.button("Reset").clicked();
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
        ui.add_space(4.0);
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
            ui.horizontal(|ui| {
                if ui.selectable_label(!*by_pct, "Pixels").clicked() && *by_pct {
                    *by_pct = false;
                    if tr.rs_pct > 0.0 {
                        // keep the current size when switching
                        if let Some((tw, th)) = stack::resize_target(w, h, 0, 0, tr.rs_pct) {
                            tr.rs_w = tw;
                            tr.rs_h = th;
                        }
                        tr.rs_pct = 0.0;
                    }
                }
                if ui.selectable_label(*by_pct, "Percent").clicked() && !*by_pct {
                    *by_pct = true;
                    let cur = match stack::resize_target(w, h, tr.rs_w, tr.rs_h, 0.0) {
                        Some((tw, _)) => tw as f32 / w.max(1) as f32 * 100.0,
                        None => 100.0,
                    };
                    tr.rs_pct = (cur * 10.0).round() / 10.0;
                }
            });
            ui.add_space(2.0);
            if *by_pct {
                let mut pct = if tr.rs_pct > 0.0 { tr.rs_pct } else { 100.0 };
                let r1 = ui.add(egui::Slider::new(&mut pct, 1.0..=400.0).suffix(" %").max_decimals(1).clamping(egui::SliderClamping::Never).logarithmic(true))
                    .on_hover_text("Drag or type any value such as 37.5 or 1200");
                if r1.changed() {
                    tr.rs_pct = pct.clamp(0.1, 6400.0);
                }
                ui.horizontal_wrapped(|ui| {
                    for pc in [10.0, 25.0, 33.3, 50.0, 75.0, 150.0, 200.0, 400.0] {
                        if chip(ui, format!("{pc}%")).clicked() {
                            tr.rs_pct = pc;
                        }
                    }
                });
            } else {
                tr.rs_pct = 0.0;
                egui::Grid::new("rs").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Width");
                    ui.add(egui::DragValue::new(&mut tr.rs_w).range(0..=16384).suffix(" px")).on_hover_text("Leave at 0 to keep the aspect ratio");
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::DragValue::new(&mut tr.rs_h).range(0..=16384).suffix(" px")).on_hover_text("Leave at 0 to keep the aspect ratio");
                    ui.end_row();
                });
                if chip(ui, "Clear").clicked() {
                    tr.rs_w = 0;
                    tr.rs_h = 0;
                }
            }
            match stack::resize_target(w, h, tr.rs_w, tr.rs_h, tr.rs_pct) {
                Some((tw, th)) => hint(ui, &format!("{w}×{h} → {tw}×{th}")),
                None => hint(ui, &format!("{w}×{h}")),
            }
        });
        group(ui, "Fit", |ui| {
            egui::Grid::new("rsfit").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Aspect");
                egui::ComboBox::from_id_salt("rsmode").selected_text(tr.rs_mode.label()).width(200.0).show_ui(ui, |ui| {
                    for m in AspectMode::ALL {
                        ui.selectable_value(&mut tr.rs_mode, m, m.label());
                    }
                });
                ui.end_row();
                ui.label("Filter");
                egui::ComboBox::from_id_salt("rsfilt").selected_text(tr.rs_filter.label()).width(200.0).show_ui(ui, |ui| {
                    for f in ResizeFilter::ALL {
                        ui.selectable_value(&mut tr.rs_filter, f, f.label());
                    }
                });
                ui.end_row();
            });
        });
        if before != (tr.rs_w, tr.rs_h, tr.rs_pct.to_bits(), tr.rs_mode, tr.rs_filter) {
            tr.resize_on = true;
        }
    }

    fn ui_crop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.crop_on, |_| {});
        let (w, h) = self.dims();
        ui.horizontal(|ui| {
            ui.label("Show");
            ui.selectable_value(&mut self.crop_live, true, "With edits").on_hover_text("Show effects and layers on the whole frame");
            ui.selectable_value(&mut self.crop_live, false, "Original").on_hover_text("Show the source frames only");
        });
        hint(ui, "Drag on the image to set the crop");
        let tr = &mut self.stack.transform;
        let before = tr.crop;
        group(ui, "Area", |ui| {
            let r = &mut tr.crop;
            egui::Grid::new("crop").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut r[0]).range(0..=w.saturating_sub(1)));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut r[1]).range(0..=h.saturating_sub(1)));
                ui.end_row();
                ui.label("W");
                ui.add(egui::DragValue::new(&mut r[2]).range(1..=w));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut r[3]).range(1..=h));
                ui.end_row();
            });
            r[2] = r[2].min(w.saturating_sub(r[0])).max(1);
            r[3] = r[3].min(h.saturating_sub(r[1])).max(1);
            ui.horizontal(|ui| {
                ui.label("Aspect");
                egui::ComboBox::from_id_salt("croplock").selected_text(self.crop_lock.label()).width(130.0).show_ui(ui, |ui| {
                    for l in AspectLock::ALL {
                        ui.selectable_value(&mut self.crop_lock, l, l.label());
                    }
                });
                if ui.button("All").on_hover_text("Select the whole frame").clicked() {
                    *r = [0, 0, w, h];
                }
            });
        });
        let mut detect = false;
        group(ui, "Auto-trim", |ui| {
            ui.horizontal(|ui| {
                ui.label("Tolerance");
                ui.add(egui::DragValue::new(&mut self.trim_tol).range(0..=255));
                detect = ui.button("Detect").on_hover_text("Shrink the area to the content of every frame").clicked();
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
            ui.horizontal(|ui| {
                ui.toggle_value(&mut tr.flip_h, "↔ Horizontal");
                ui.toggle_value(&mut tr.flip_v, "↕ Vertical");
            });
        });
        group(ui, "Rotate", |ui| {
            ui.horizontal(|ui| {
                for (deg, label) in [(0u16, "0°"), (90, "↻ 90°"), (180, "180°"), (270, "↺ 90°")] {
                    if ui.selectable_label(tr.rot == deg, label).clicked() {
                        tr.rot = deg;
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr.rot_any, "Free");
                if ui.add(egui::DragValue::new(&mut tr.rot_deg).speed(0.5).suffix("°")).changed() {
                    tr.rot_any = true;
                }
            });
            if ui.add(egui::Slider::new(&mut tr.rot_deg, -180.0..=180.0).show_value(false).clamping(egui::SliderClamping::Never)).changed() {
                tr.rot_any = true;
            }
            ui.checkbox(&mut tr.rot_expand, "Expand canvas");
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr.rot_bg_transparent, "Transparent fill");
                if !tr.rot_bg_transparent {
                    ui.color_edit_button_srgb(&mut tr.rot_bg);
                }
            });
        });
        if before != format!("{} {} {} {} {} {} {} {:?}", tr.flip_v, tr.flip_h, tr.rot, tr.rot_any, tr.rot_deg, tr.rot_expand, tr.rot_bg_transparent, tr.rot_bg) {
            tr.rot_on = true;
        }
    }

    fn ui_reverse(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let tr = &mut self.stack.transform;
        group(ui, "Playback", |ui| {
            ui.checkbox(&mut tr.reverse, "⟲ Reverse");
            ui.checkbox(&mut tr.boomerang, "⇄ Boomerang").on_hover_text("Play forward and then backward");
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr.set_loop, "Loop count");
                if tr.set_loop {
                    ui.add(egui::DragValue::new(&mut tr.loop_count).range(0..=65535)).on_hover_text("0 loops forever");
                }
            });
        });
        if ui.button("⬇ Bake").on_hover_text("Make frame edits permanent. Layers stay editable.").clicked() {
            self.bake(ctx, true);
        }
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
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut tr.speed_val).range(0.1..=100000.0).speed(1.0));
                egui::ComboBox::from_id_salt("spmode")
                    .width(150.0)
                    .selected_text(match tr.speed_mode {
                        SpeedMode::Percent => "% of original",
                        SpeedMode::DelayAll => "cs per frame",
                        SpeedMode::Fps => "fps",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::Percent, "% of original");
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::DelayAll, "cs per frame").on_hover_text("One delay for every frame in hundredths of a second");
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::Fps, "fps").on_hover_text("One frame rate for every frame");
                    });
            });
            if tr.speed_mode == SpeedMode::Percent {
                ui.add(soft_slider(&mut tr.speed_val, 10.0..=400.0, "%"));
                ui.horizontal(|ui| {
                    for pc in [25.0, 50.0, 100.0, 150.0, 200.0, 300.0] {
                        if chip(ui, format!("{pc:.0}%")).clicked() {
                            tr.speed_val = pc;
                        }
                    }
                });
            } else {
                let d = if tr.speed_mode == SpeedMode::Fps { (100.0 / tr.speed_val).round() } else { tr.speed_val };
                if d < 2.0 {
                    ui.label(RichText::new("⚠ Too fast for browsers").color(p().neutral)).on_hover_text("Most browsers play delays under 2 cs at 10 cs.");
                }
                if tr.speed_mode == SpeedMode::Fps {
                    hint(ui, &format!("= {} cs ({:.2} fps)", d.max(1.0), 100.0 / d.max(1.0)));
                }
            }
            hint(ui, &format!("Source: {avg_fps:.1} fps"));
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
            ui.horizontal(|ui| {
                ui.selectable_value(&mut tr.cut_by_time, false, "Frames");
                ui.selectable_value(&mut tr.cut_by_time, true, "Seconds");
            });
            if tr.cut_by_time {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut tr.cut_t0).range(0.0..=dur).speed(0.01).suffix(" s"));
                    ui.label("–");
                    ui.add(egui::DragValue::new(&mut tr.cut_t1).range(0.0..=dur).speed(0.01).suffix(" s"));
                });
            } else {
                let mut end = if tr.cut_end == 0 { n } else { tr.cut_end };
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut tr.cut_start).range(1..=n));
                    ui.label("–");
                    ui.add(egui::DragValue::new(&mut end).range(1..=n));
                });
                tr.cut_end = if end >= n { 0 } else { end };
                ui.horizontal(|ui| {
                    if chip(ui, "Start here").on_hover_text("Use the current frame").clicked() {
                        tr.cut_start = src;
                    }
                    if chip(ui, "End here").on_hover_text("Use the current frame").clicked() {
                        tr.cut_end = src;
                    }
                });
            }
        });
        if before != format!("{} {} {} {} {}", tr.cut_by_time, tr.cut_start, tr.cut_end, tr.cut_t0, tr.cut_t1) {
            tr.cut_on = true;
        }
        if let Some(b) = &self.base {
            let (s, e) = self.stack.transform.cut_range(b);
            hint(ui, &format!("Keeps {}–{} ({} frames)", s + 1, e + 1, e - s + 1));
        }
    }

    fn ui_optimize(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        group(ui, "Format", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for f in [ExportFormat::Gif, ExportFormat::Webp, ExportFormat::Avif, ExportFormat::Apng, ExportFormat::Mp4, ExportFormat::Webm] {
                    let en = !f.needs_ffmpeg() || self.ffmpeg;
                    let r = ui.add_enabled(en, egui::Button::new(f.label()).selected(self.export_fmt == f));
                    let r = if en { r.on_hover_text(f.tip()) } else { r.on_disabled_hover_text("Needs ffmpeg on PATH") };
                    if r.clicked() {
                        self.export_fmt = f;
                    }
                }
            });
            hint(ui, "Preview and Export use this format");
        });
        let cur_key = self.cmp_key();
        let busy = self.cmp_job.is_some();
        group(ui, "Result", |ui| match &self.cmp {
            Some(c) => {
                let n = c.bytes.len() as u64;
                let stale = c.key != cur_key;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(fmt_size(n)).font(theme::medium(16.0)).color(if stale { pal.fg_muted } else { pal.positive }));
                    if let Some(orig) = self.file_size {
                        hint(ui, &format!("{:+.0}% vs original", (n as f64 / orig as f64 - 1.0) * 100.0));
                    }
                    if stale || busy {
                        ui.spinner();
                    }
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    hint(ui, "Encoding…");
                });
            }
        });
        let mut find = false;
        let mut reset = false;
        let idle = !self.busy();
        group(ui, "Target size", |ui| {
            ui.horizontal(|ui| {
                ui.label("Under");
                ui.add(egui::DragValue::new(&mut self.target_val).range(0.01..=10000.0).speed(0.1).max_decimals(2));
                ui.selectable_value(&mut self.target_mb, true, "MiB");
                ui.selectable_value(&mut self.target_mb, false, "KiB");
                find = ui.add_enabled(idle && self.export_fmt.can_target(), egui::Button::new("Find settings")).on_hover_text("Finds the best quality under this size and applies it. Quality drops first and size only if needed.").clicked();
            });
            ui.horizontal_wrapped(|ui| {
                for (lbl, v, mb) in TARGET_PRESETS {
                    if chip(ui, lbl).clicked() {
                        self.target_val = v;
                        self.target_mb = mb;
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.fit_keep_size, "Keep size").on_hover_text("Find settings changes compression only and never shrinks the frames").changed() {
                theme::set_setting("fit_keep_size", if self.fit_keep_size { "1" } else { "0" });
            }
            reset = ui.add_enabled(idle, egui::Button::new("Reset compression")).on_hover_text("Go back to lossless settings and undo any downscale from Find settings").clicked();
        });
        if reset {
            self.undo_last_fit();
            self.gif_opts = GifOptions { noai_tag: self.gif_opts.noai_tag, ..GifOptions::default() };
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
            if let Some(base) = self.base.clone() {
                let stack = self.stack.clone();
                let fonts = self.fonts.clone();
                let total = self.plan.len();
                self.spawn(ctx, &format!("Finding settings under {}", fmt_size(max)), total, move |p| {
                    // render each scale through the real Resize step so sizes match the preview
                    JobOutput::Fit(crate::target::fit_with(&tf, max, allow_scale, p, |sc| {
                        let mut st = stack.clone();
                        apply_fit_scale(&mut st.transform, sc);
                        st.flatten(&base, &fonts, None)
                    }))
                });
            }
        }
        match self.export_fmt {
            ExportFormat::Gif => {}
            ExportFormat::Webp => {
                let o = &mut self.webp_opts;
                group(ui, "WebP", |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut o.lossless, false, "Lossy");
                        ui.selectable_value(&mut o.lossless, true, "Lossless");
                    });
                    if !o.lossless {
                        ui.add(egui::Slider::new(&mut o.quality, 0.0..=100.0).text("quality")).on_hover_text("75–90 looks like the original for most GIFs");
                    }
                    ui.add(egui::Slider::new(&mut o.method, 0..=6).text("effort")).on_hover_text("Higher makes smaller files but takes longer");
                });
                return;
            }
            ExportFormat::Avif => {
                group(ui, "AVIF", |ui| {
                    ui.add(egui::Slider::new(&mut self.avif_quality, 0..=100).text("quality")).on_hover_text("50–70 is a good balance");
                    hint(ui, "Slow to encode so the preview takes a while");
                });
                return;
            }
            ExportFormat::Mp4 => {
                group(ui, "MP4", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Background");
                        ui.color_edit_button_srgb(&mut self.video_bg).on_hover_text("MP4 has no transparency. This colour fills it.");
                    });
                });
                return;
            }
            ExportFormat::Apng => {
                hint(ui, "APNG is lossless and has no settings");
                return;
            }
            _ => {
                hint(ui, "No compression settings for this format");
                return;
            }
        }
        group(ui, "Presets", |ui| {
            ui.horizontal_wrapped(|ui| {
                if chip(ui, "Best").clicked() {
                    self.gif_opts = GifOptions { quant_speed: 1, ..GifOptions::default() };
                }
                if chip(ui, "Balanced").clicked() {
                    self.gif_opts = GifOptions { lossy: 30, ..GifOptions::default() };
                }
                if chip(ui, "Small").clicked() {
                    self.gif_opts = GifOptions { lossy: 60, colors: 128, global_palette: true, ..GifOptions::default() };
                }
                if chip(ui, "Tiny").clicked() {
                    self.gif_opts = GifOptions { lossy: 120, colors: 48, global_palette: true, ..GifOptions::default() };
                }
            });
        });
        let o = &mut self.gif_opts;
        group(ui, "Lossy", |ui| {
            ui.add(egui::Slider::new(&mut o.lossy, 0..=200).text("level").clamping(egui::SliderClamping::Never)).on_hover_text("Lets similar colours merge. 30–80 is usually invisible.");
        });
        group(ui, "Colours", |ui| {
            ui.add(egui::Slider::new(&mut o.colors, 2..=256).text("max"));
            ui.checkbox(&mut o.global_palette, "One palette for all frames").on_hover_text("Smaller files but may show banding on varied footage");
            ui.checkbox(&mut o.dither, "Dither");
            ui.add(egui::Slider::new(&mut o.quant_speed, 1..=30).text("quantizer speed")).on_hover_text("1 gives the best palette and 30 is fastest");
        });
        let drop_nth = &mut self.opt_drop_nth;
        let mut drop_go = false;
        group(ui, "Frames", |ui| {
            ui.checkbox(&mut o.frame_diff, "Store only changed pixels");
            ui.checkbox(&mut o.merge_duplicates, "Merge identical frames");
            ui.add(egui::Slider::new(&mut o.alpha_threshold, 1..=255).text("alpha cut-off")).on_hover_text("Pixels below this opacity become transparent");
            ui.horizontal(|ui| {
                ui.label("Drop every");
                ui.add(egui::DragValue::new(drop_nth).range(0..=100));
                ui.label("th frame");
                drop_go = ui.add_enabled(*drop_nth >= 2, egui::Button::new("Apply")).on_hover_text("Removes source frames and keeps the timing").clicked();
            });
        });
        if drop_go {
            let n = self.opt_drop_nth;
            self.run_anim(ctx, "Drop frames", move |a, _| Ok(ops::drop_every_nth(a, n)));
        }
    }

    fn ui_effects(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let fx_layers: Vec<(usize, u64)> = self.stack.layers.iter().enumerate().filter(|(_, l)| l.kind() == Kind::Effects).map(|(i, l)| (i, l.id)).collect();
        let sel = self.sel_index(Kind::Effects);
        if fx_layers.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for (n, (i, id)) in fx_layers.iter().enumerate() {
                    if ui.selectable_label(Some(*i) == sel, format!("Effects {}", n + 1)).clicked() {
                        self.select(Kind::Effects, *id);
                    }
                }
            });
        }
        let sel = self.sel_index(Kind::Effects);
        ui.horizontal(|ui| {
            if let Some(i) = sel {
                ui.checkbox(&mut self.stack.layers[i].visible, "Visible");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !fx_layers.is_empty() && ui.button("+ Layer").on_hover_text("Add another effects layer to place effects above or below other layers").clicked() {
                    self.add_layer(LayerKind::Effects(Box::default()));
                }
                if ui.button("Reset").clicked() {
                    match sel {
                        Some(i) => self.stack.layers[i].kind = LayerKind::Effects(Box::default()),
                        None => self.fx_draft = EffectSettings::default(),
                    }
                }
            });
        });
        let _ = ctx;
        let Stack { transform, layers, .. } = &mut self.stack;
        let fx: &mut EffectSettings = match sel {
            Some(i) => match &mut layers[i].kind {
                LayerKind::Effects(f) => f,
                _ => &mut self.fx_draft,
            },
            None => &mut self.fx_draft,
        };
        effects_ui(ui, fx, transform);
        if sel.is_none() && self.fx_draft != EffectSettings::default() {
            let fx = std::mem::take(&mut self.fx_draft);
            self.add_layer(LayerKind::Effects(Box::new(fx)));
        }
    }

    /// Angle editor shared by text / censor / image layers.
    fn angle_row(ui: &mut egui::Ui, angle: &mut f32) {
        ui.horizontal(|ui| {
            ui.label("Angle");
            ui.add(egui::DragValue::new(angle).speed(0.5).suffix("°").max_decimals(1)).on_hover_text("Or drag the round handle. Hold Shift to snap.");
            for (lbl, v) in [("0", 0.0), ("−90", -90.0), ("90", 90.0), ("180", 180.0)] {
                if chip(ui, lbl).clicked() {
                    *angle = v;
                }
            }
        });
    }

    /// Load (or reuse) a system font face, returning its index in `self.fonts`.
    fn font_index_for(&mut self, family: &str, style: &str) -> Option<usize> {
        let key = format!("{family}|{style}");
        if let Some(i) = self.font_keys.get(&key) {
            return Some(*i);
        }
        let face = {
            let guard = self.sys_fonts.lock().ok()?;
            let fams = guard.as_ref()?;
            let f = fams.iter().find(|f| f.name == family)?;
            f.faces.iter().find(|x| x.style == style).or_else(|| Some(f.default_face())).cloned()?
        };
        match crate::sysfonts::load(&face) {
            Ok(font) => {
                self.fonts.push(FontEntry { name: format!("{family} {}", face.style), font });
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
        let r = ui.add(egui::Button::new(RichText::new(format!("{}  ▾", elide(&cur_name, 30))).font(cur_font).color(pal.fg)).min_size(vec2(ui.available_width(), 30.0)));
        if r.clicked() {
            self.font_picker_open = !self.font_picker_open;
            self.font_search.clear();
        }
        if let Some((fam, style)) = self.font_meta.get(&cur_idx).cloned() {
            let styles: Vec<String> = self.sys_fonts.lock().ok().and_then(|g| g.as_ref().and_then(|v| v.iter().find(|f| f.name == fam).map(|f| f.faces.iter().map(|x| x.style.clone()).collect()))).unwrap_or_default();
            if styles.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label("Style");
                    egui::ComboBox::from_id_salt("fontstyle").width(150.0).selected_text(&style).show_ui(ui, |ui| {
                        for st in &styles {
                            if ui.selectable_label(*st == style, st).clicked() {
                                pick_sys = Some((fam.clone(), st.clone()));
                            }
                        }
                    });
                });
            }
        }
        if self.font_picker_open {
            let q = self.font_search.to_lowercase();
            let fams: Option<Vec<String>> = self.sys_fonts.lock().ok().and_then(|g| g.as_ref().map(|v| v.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).map(|f| f.name.clone()).collect()));
            let cur_fam = self.font_meta.get(&cur_idx).map(|m| m.0.clone());
            egui::Frame::new().fill(pal.view).stroke(Stroke::new(1.0, pal.border)).corner_radius(3.0).inner_margin(6.0).show(ui, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.font_search).hint_text("Search fonts…").desired_width(f32::INFINITY));
                if self.font_search.is_empty() && !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                // built-in and file fonts
                let extra: Vec<(usize, String)> = (0..self.fonts.len()).filter(|i| *i < self.builtin_font_count || !self.font_meta.contains_key(i)).map(|i| (i, self.fonts[i].name.clone())).filter(|(_, n)| q.is_empty() || n.to_lowercase().contains(&q)).collect();
                for (i, n) in extra {
                    let f = if i < self.builtin_font_count { Self::builtin_preview_font(i, 15.0) } else { egui::FontId::proportional(15.0) };
                    if ui.add(egui::Button::selectable(i == cur_idx, RichText::new(n).font(f))).clicked() {
                        pick = Some(i);
                    }
                }
                ui.separator();
                match fams {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            hint(ui, "Scanning fonts…");
                        });
                    }
                    Some(fams) => {
                        let row_h = 26.0;
                        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show_rows(ui, row_h, fams.len(), |ui, range| {
                            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for fam in &fams[range] {
                                let font = self.preview_font(fam, 15.0);
                                let sel = cur_fam.as_deref() == Some(fam.as_str());
                                let r = ui.add(egui::Button::selectable(sel, RichText::new(elide(fam, 34)).font(font)).min_size(vec2(0.0, row_h - 4.0)));
                                if r.on_hover_text(fam).clicked() {
                                    pick_sys = Some((fam.clone(), String::new()));
                                }
                            }
                            });
                        });
                    }
                }
            });
        }
        if let Some((fam, st)) = pick_sys {
            let style = if st.is_empty() {
                self.sys_fonts.lock().ok().and_then(|g| g.as_ref().and_then(|v| v.iter().find(|f| f.name == fam).map(|f| f.default_face().style.clone()))).unwrap_or_default()
            } else {
                st
            };
            pick = self.font_index_for(&fam, &style);
        }
        if pick.is_some() {
            self.font_picker_open = false;
        }
        pick
    }

    /// Layer chips for a kind (when there is more than one) + add/remove row.
    fn layer_chooser(&mut self, ui: &mut egui::Ui, kind: Kind, add_label: &str, mut add: impl FnMut(&mut Self)) -> Option<usize> {
        let items: Vec<(usize, u64, String)> = self.stack.layers.iter().enumerate().filter(|(_, l)| l.kind() == kind).map(|(i, l)| (i, l.id, elide(&l.name(), 16))).collect();
        let sel = self.sel_index(kind);
        ui.horizontal_wrapped(|ui| {
            if items.len() > 1 {
                for (i, id, name) in &items {
                    if ui.selectable_label(Some(*i) == sel, name).clicked() {
                        self.select(kind, *id);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button(add_label).clicked() {
                add(self);
            }
            if let Some(i) = self.sel_index(kind) {
                ui.checkbox(&mut self.stack.layers[i].visible, "Visible");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if small_flat(ui, "✕", "Delete layer") {
                        self.stack.layers.remove(i);
                    }
                });
            }
        });
        if items.is_empty() {
            return None;
        }
        self.sel_index(kind)
    }

    fn ui_text(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let Some(sel) = self.layer_chooser(ui, Kind::Text, "+ Text", |s| {
            let (w, h) = s.dims();
            s.add_layer(LayerKind::Text(Annotation { text: "Your text".into(), x: (w as i32 / 2 - 80).max(0), y: (h as i32 - 60).max(0), ..Default::default() }));
        }) else {
            hint(ui, "Add text and drag it into place");
            return;
        };
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        if let LayerKind::Text(a) = &mut self.stack.layers[sel].kind {
            ui.add(egui::TextEdit::multiline(&mut a.text).desired_rows(2).desired_width(f32::INFINITY));
        }
        let cur_font = match &self.stack.layers[sel].kind {
            LayerKind::Text(a) => a.font_idx,
            _ => 0,
        };
        let mut picked = None;
        let mut load_file = false;
        group(ui, "Font", |ui| {
            picked = self.font_picker(ui, cur_font);
            load_file = chip(ui, "Load font file…").clicked();
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
        if let (Some(fi), LayerKind::Text(a)) = (picked, &mut self.stack.layers[sel].kind) {
            let (w0, h0) = text::measure(&self.fonts, a);
            let (cx, cy) = (a.x as f32 + w0 as f32 / 2.0, a.y as f32 + h0 as f32 / 2.0);
            a.font_idx = fi;
            let (w1, h1) = text::measure(&self.fonts, a);
            a.x = (cx - w1 as f32 / 2.0).round() as i32;
            a.y = (cy - h1 as f32 / 2.0).round() as i32;
        }
        let LayerKind::Text(a) = &mut self.stack.layers[sel].kind else { return };
        group(ui, "Style", |ui| {
            egui::Grid::new("txtgrid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Size");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut a.size).range(4.0..=4000.0).suffix(" px"));
                    ui.color_edit_button_srgba_unmultiplied(&mut a.color);
                });
                ui.end_row();
                ui.label("Outline");
                ui.horizontal(|ui| {
                    let mut w = a.outline_px() as f32;
                    if ui.add(egui::DragValue::new(&mut w).range(0.0..=200.0).speed(0.2).suffix(" px")).on_hover_text("0 turns the outline off").changed() {
                        a.border_width = Some(w.round());
                    }
                    ui.color_edit_button_srgba_unmultiplied(&mut a.border_color);
                });
                ui.end_row();
                ui.label("Align");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut a.align, Align::Left, "L");
                    ui.selectable_value(&mut a.align, Align::Center, "C");
                    ui.selectable_value(&mut a.align, Align::Right, "R");
                });
                ui.end_row();
                ui.label("Opacity");
                ui.add(egui::DragValue::new(&mut a.opacity).range(0..=100).suffix("%"));
                ui.end_row();
                ui.label("Colours");
                egui::ComboBox::from_id_salt("cpreset").width(140.0).selected_text("Preset…").show_ui(ui, |ui| {
                    let presets: [(&str, [u8; 4], [u8; 4]); 5] = [
                        ("White / black", [255, 255, 255, 255], [0, 0, 0, 255]),
                        ("Black / white", [0, 0, 0, 255], [255, 255, 255, 255]),
                        ("Yellow / black", [255, 230, 0, 255], [0, 0, 0, 255]),
                        ("Red / white", [230, 30, 40, 255], [255, 255, 255, 255]),
                        ("Cyan / navy", [80, 230, 255, 255], [10, 20, 70, 255]),
                    ];
                    for (name, c, b) in presets {
                        if ui.selectable_label(false, name).clicked() {
                            a.color = c;
                            a.border_color = b;
                        }
                    }
                });
                ui.end_row();
            });
        });
        group(ui, "Placement", |ui| {
            ui.horizontal(|ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut a.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut a.y));
            });
            Self::angle_row(ui, &mut a.angle);
            frames_row(ui, &mut a.from, &mut a.to, n);
        });
    }

    fn ui_censor(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let (w, h) = self.dims();
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        let Some(sel) = self.layer_chooser(ui, Kind::Censor, "+ Region", |s| {
            s.add_layer(LayerKind::Censor(CensorSettings { x: (w / 4) as i32, y: (h / 4) as i32, w: w / 2, h: h / 2, ..Default::default() }));
        }) else {
            hint(ui, "Drag on the image to draw a region");
            return;
        };
        let LayerKind::Censor(c) = &mut self.stack.layers[sel].kind else { return };
        group(ui, "Method", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut c.method, CensorMethod::Pixelate, "Pixelate");
                ui.selectable_value(&mut c.method, CensorMethod::Blur, "Blur");
                ui.selectable_value(&mut c.method, CensorMethod::Solid, "Solid");
            });
            match c.method {
                CensorMethod::Pixelate => {
                    ui.horizontal(|ui| {
                        ui.label("Block");
                        ui.add(egui::DragValue::new(&mut c.square).range(2..=1000).suffix(" px"));
                    });
                }
                CensorMethod::Blur => {
                    ui.add(soft_slider(&mut c.sigma, 1.0..=40.0, "radius"));
                }
                CensorMethod::Solid => {
                    ui.horizontal(|ui| {
                        ui.label("Colour");
                        ui.color_edit_button_srgb(&mut c.color);
                    });
                }
            }
        });
        group(ui, "Placement", |ui| {
            egui::Grid::new("cens").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut c.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut c.y));
                ui.end_row();
                ui.label("W");
                ui.add(egui::DragValue::new(&mut c.w).range(0..=w * 4));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut c.h).range(0..=h * 4));
                ui.end_row();
            });
            Self::angle_row(ui, &mut c.angle);
            frames_row(ui, &mut c.from, &mut c.to, n);
        });
    }

    fn ui_add_image(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(sel) = self.layer_chooser(ui, Kind::Overlay, "+ Image…", |s| {
            if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "gif", "jpg", "jpeg", "webp", "bmp"]).pick_file() {
                s.load_overlay(path, ctx);
            }
        }) else {
            hint(ui, "Add a PNG, GIF, JPG or WebP. You can also drop one here.");
            return;
        };
        let (aw, ah) = self.dims();
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let scope_before = self.place_scope;
        let mut scope = scope_before;
        let LayerKind::Overlay(o) = &mut self.stack.layers[sel].kind else { return };
        let native = o.src.native_size();
        let nframes = o.src.frames.len();
        group(ui, "Image", |ui| {
            hint(ui, &format!("{}×{} · {} frame{}", native.0, native.1, nframes, if nframes == 1 { "" } else { "s" }));
            ui.add(egui::Slider::new(&mut o.opacity, 0.0..=100.0).text("opacity %"));
        });
        if o.placements.is_empty() {
            return;
        }
        let cur = src_idx.min(o.placements.len() - 1);
        group(ui, "Placement", |ui| {
            ui.horizontal(|ui| {
                ui.label("Edits apply to");
                egui::ComboBox::from_id_salt("pscope").width(150.0).selected_text(scope.label()).show_ui(ui, |ui| {
                    for s in [PlaceScope::All, PlaceScope::Following, PlaceScope::This] {
                        ui.selectable_value(&mut scope, s, s.label());
                    }
                });
            });
            let mut pl = o.placements[cur];
            egui::Grid::new("ovl").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("X");
                ui.add(egui::DragValue::new(&mut pl.x));
                ui.label("Y");
                ui.add(egui::DragValue::new(&mut pl.y));
                ui.end_row();
                ui.label("W");
                ui.add(egui::DragValue::new(&mut pl.w).range(1..=16384));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut pl.h).range(1..=16384));
                ui.end_row();
            });
            Self::angle_row(ui, &mut pl.angle);
            ui.horizontal_wrapped(|ui| {
                if chip(ui, "Native").on_hover_text("Original image size").clicked() {
                    pl.w = native.0;
                    pl.h = native.1;
                }
                if chip(ui, "Center").clicked() {
                    pl.x = (aw as i32 - pl.w as i32) / 2;
                    pl.y = (ah as i32 - pl.h as i32) / 2;
                }
                if chip(ui, "Fill").on_hover_text("Cover the whole frame").clicked() {
                    pl.x = 0;
                    pl.y = 0;
                    pl.w = aw;
                    pl.h = ah;
                }
            });
            let mut hidden = !pl.enabled;
            ui.checkbox(&mut hidden, format!("Hide on frame {}", cur + 1));
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
            if chip(ui, "Show on all frames").clicked() {
                for q in &mut o.placements {
                    q.enabled = true;
                }
            }
        });
        self.place_scope = scope;
    }

    fn ui_watermark(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let Some(sel) = self.layer_chooser(ui, Kind::Watermark, "+ Watermark", |s| {
            s.add_layer(LayerKind::Watermark(Box::default()));
        }) else {
            hint(ui, "Add text or a logo that stays in place");
            return;
        };
        let (cur_font, use_image) = match &self.stack.layers[sel].kind {
            LayerKind::Watermark(w) => (w.font_idx, w.use_image),
            _ => return,
        };
        let mut picked = None;
        let mut browse = false;
        if let LayerKind::Watermark(w) = &mut self.stack.layers[sel].kind {
            group(ui, "Content", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut w.use_image, false, "Text");
                    ui.selectable_value(&mut w.use_image, true, "Image");
                });
                if w.use_image {
                    ui.horizontal(|ui| {
                        browse = ui.button("Choose image…").clicked();
                        if let Some(p) = &w.image_path {
                            hint(ui, &elide(&p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), 24));
                        }
                    });
                } else {
                    ui.add(egui::TextEdit::singleline(&mut w.text).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        ui.color_edit_button_srgba_unmultiplied(&mut w.color);
                        ui.checkbox(&mut w.outline, "Outline");
                        if w.outline {
                            let mut px = w.outline_px.unwrap_or(0.0);
                            let r = ui.add(egui::DragValue::new(&mut px).range(0.0..=100.0).speed(0.2).suffix(" px")).on_hover_text("0 picks a width from the text size");
                            if r.changed() {
                                w.outline_px = if px <= 0.0 { None } else { Some(px.round()) };
                            }
                        }
                    });
                }
            });
        }
        if !use_image {
            group(ui, "Font", |ui| {
                picked = self.font_picker(ui, cur_font);
            });
        }
        if browse {
            if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_file() {
                match io::load_still(&path) {
                    Ok(img) => {
                        if let LayerKind::Watermark(w) = &mut self.stack.layers[sel].kind {
                            w.image = Some(Arc::new(img));
                            w.image_path = Some(path);
                        }
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
        }
        let LayerKind::Watermark(w) = &mut self.stack.layers[sel].kind else { return };
        if let Some(fi) = picked {
            w.font_idx = fi;
        }
        group(ui, "Placement", |ui| {
            ui.add(soft_slider(&mut w.size, 1.0..=40.0, "size %")).on_hover_text("Size as a share of the frame's shorter side");
            ui.add(egui::Slider::new(&mut w.opacity, 0.0..=100.0).text("opacity %"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut w.tiled, false, "Single");
                ui.selectable_value(&mut w.tiled, true, "Tiled");
            });
            if w.tiled {
                ui.add(egui::Slider::new(&mut w.angle, -90.0..=90.0).text("angle °"));
                ui.add(soft_slider(&mut w.spacing, 0.8..=4.0, "spacing"));
            } else {
                ui.horizontal(|ui| {
                    egui::Grid::new("anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                        for row in crate::effects::Anchor::GRID {
                            for a in row {
                                let sel = w.anchor == a;
                                if ui.add(egui::Button::new(if sel { "●" } else { "·" }).selected(sel).min_size(vec2(24.0, 22.0))).clicked() {
                                    w.anchor = a;
                                }
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label("Margin");
                        ui.add(egui::DragValue::new(&mut w.margin).range(0.0..=40.0).speed(0.1).suffix(" %"));
                    });
                });
            }
        });
    }

    fn ui_shield(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let Some(sel) = self.layer_chooser(ui, Kind::Shield, "+ AI shield", |s| {
            s.add_layer(LayerKind::Shield(ShieldSettings::default()));
        }) else {
            hint(ui, "Adds faint noise that makes frames worse as AI training data");
            return;
        };
        hint(ui, "Adds faint noise that makes frames worse as AI training data.");
        ui.label(RichText::new("Not a guarantee. More…").size(12.0).color(p().fg_muted).underline()).on_hover_text(
            "It is much weaker than tools like Glaze or Nightshade. Strong GIF compression also removes some of it.\n\nPair it with the “noai” tag in the Export window.",
        );
        let LayerKind::Shield(s) = &mut self.stack.layers[sel].kind else { return };
        group(ui, "Perturbation", |ui| {
            ui.add(soft_slider(&mut s.strength, 0.0..=20.0, "strength")).on_hover_text("How strong the noise is. 4 to 8 is hard to see.");
            ui.checkbox(&mut s.pattern, "High-frequency pattern");
            ui.checkbox(&mut s.noise, "Colour noise");
            ui.checkbox(&mut s.jitter, "Sub-pixel jitter").on_hover_text("Warps the image by tiny amounts");
            ui.checkbox(&mut s.per_frame, "Vary every frame").on_hover_text("Stronger protection but much larger GIFs");
            ui.horizontal(|ui| {
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut s.seed));
                if chip(ui, "Randomise").clicked() {
                    s.seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1) % 1_000_000) as u32;
                }
            });
        });
    }

    fn ui_frames(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let n = self.base.as_ref().map(|a| a.len()).unwrap_or(1);
        let now = ctx.input(|i| i.time);
        if ui.button("+ Add frames…").on_hover_text("Add images to the end as new frames").clicked() {
            if let Some(mut ps) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).pick_files() {
                ps.sort();
                let d = self.fr_delay;
                self.run_anim(ctx, "Add frames", move |a, _| {
                    let imgs = ps.iter().map(|p| io::load_still(p)).collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(ops::append_images(a, imgs, d))
                });
            }
        }
        ui.add_space(4.0);
        let mut range_act: Option<(bool, bool)> = None;
        group(ui, "Skip / enable", |ui| {
            ui.horizontal(|ui| {
                ui.label("Frames");
                ui.add(egui::DragValue::new(&mut self.fr_from).range(1..=n));
                ui.label("–");
                ui.add(egui::DragValue::new(&mut self.fr_to).range(1..=n));
                if tinted(ui, "Skip", pal.negative) {
                    range_act = Some((false, false));
                }
                if tinted(ui, "Enable", pal.highlight) {
                    range_act = Some((false, true));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Every");
                ui.add(egui::DragValue::new(&mut self.fr_every).range(2..=1000));
                ui.label("th");
                if tinted(ui, "Skip", pal.negative) {
                    range_act = Some((true, false));
                }
                if tinted(ui, "Enable", pal.highlight) {
                    range_act = Some((true, true));
                }
            });
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
            ui.horizontal(|ui| {
                ui.label("Delay");
                ui.add(egui::DragValue::new(&mut self.fr_delay).range(1..=65535).suffix(" cs")).on_hover_text(format!("{:.2} fps", 100.0 / self.fr_delay.max(1) as f32));
                if ui.button("Set all").clicked() {
                    let d = self.fr_delay;
                    self.run_anim(ctx, "Set delays", move |a, _| Ok(ops::set_all_delays(a, d)));
                }
            });
            let mut forever = self.base.as_ref().map(|a| a.loop_count.is_none()).unwrap_or(true);
            let mut count = self.base.as_ref().and_then(|a| a.loop_count).unwrap_or(1);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Loop");
                changed |= ui.checkbox(&mut forever, "forever").changed();
                if !forever {
                    changed |= ui.add(egui::DragValue::new(&mut count).range(1..=65535).suffix("×")).changed();
                }
            });
            if changed {
                self.snapshot("loop", now);
                self.base.as_mut().unwrap().loop_count = if forever { None } else { Some(count.max(1)) };
            }
        });
        group(ui, "Blend", |ui| {
            ui.horizontal(|ui| {
                ui.label("Crossfade");
                ui.add(egui::DragValue::new(&mut self.fr_crossfade).range(1..=30)).on_hover_text("Number of in-between frames");
                ui.checkbox(&mut self.fr_crossfade_wrap, "loop").on_hover_text("Also blend the last frame into the first");
                if ui.button("Apply").clicked() {
                    let (s, w) = (self.fr_crossfade, self.fr_crossfade_wrap);
                    self.run_anim(ctx, "Crossfade", move |a, _| Ok(ops::crossfade(a, s, w)));
                }
            });
            if ui.button("First frame as background").on_hover_text("Draw every frame over frame 1").clicked() {
                self.run_anim(ctx, "First frame as background", move |a, _| Ok(ops::first_frame_as_background(a)));
            }
        });
        let skipped = self.base.as_ref().map(|a| a.len() - a.enabled_count()).unwrap_or(0);
        if skipped > 0 && tinted(ui, &format!("Delete {skipped} skipped"), pal.negative) {
            self.run_anim(ctx, "Remove skipped", move |a, _| {
                let mut a = a.clone();
                a.frames.retain(|f| f.enabled);
                Ok(a)
            });
        }
    }
}

/// Source-frame range editor (1-based, `to` 0 = end).
fn frames_row(ui: &mut egui::Ui, from: &mut usize, to: &mut usize, n: usize) {
    ui.horizontal(|ui| {
        ui.label("Frames");
        ui.add(egui::DragValue::new(from).range(1..=n));
        ui.label("–");
        let mut t = if *to == 0 { n } else { *to };
        if ui.add(egui::DragValue::new(&mut t).range(1..=n)).changed() {
            *to = if t >= n { 0 } else { t };
        }
    });
}

// ================================================================ effects panel

fn on(b: &[bool]) -> Option<String> {
    let n = b.iter().filter(|x| **x).count();
    (n > 0).then(|| n.to_string())
}

fn effects_ui(ui: &mut egui::Ui, fx: &mut EffectSettings, tr: &mut Transform) {
    let d = EffectSettings::default();
    let colour_changed = fx.hue != d.hue || fx.saturation != d.saturation || fx.lightness != d.lightness || fx.brightness != d.brightness || fx.contrast != d.contrast;
    group_ex(ui, "Colour", true, colour_changed.then(|| "●".to_string()), |ui| {
        ui.add(soft_slider(&mut fx.hue, 0.0..=200.0, "Hue"));
        ui.add(soft_slider(&mut fx.saturation, 0.0..=200.0, "Saturation"));
        ui.add(soft_slider(&mut fx.lightness, 0.0..=200.0, "Lightness"));
        ui.add(soft_slider(&mut fx.brightness, -100.0..=100.0, "Brightness"));
        ui.add(soft_slider(&mut fx.contrast, -100.0..=100.0, "Contrast"));
        if chip(ui, "Reset").clicked() {
            fx.hue = 100.0;
            fx.saturation = 100.0;
            fx.lightness = 100.0;
            fx.brightness = 0.0;
            fx.contrast = 0.0;
        }
    });
    group_ex(ui, "Looks", true, on(&[fx.grayscale, fx.sepia, fx.monochrome, fx.negative, fx.tint, fx.change_bg]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut fx.grayscale, "Grayscale");
            ui.toggle_value(&mut fx.sepia, "Sepia");
            ui.toggle_value(&mut fx.monochrome, "Mono").on_hover_text("Black and white only");
            ui.toggle_value(&mut fx.negative, "Invert").on_hover_text("Negative colours");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.tint, "Tint");
            if fx.tint {
                ui.color_edit_button_srgb(&mut fx.tint_color);
                ui.add(egui::DragValue::new(&mut fx.tint_amount).range(0.0..=100.0).suffix("%"));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.change_bg, "Background");
            if fx.change_bg {
                ui.color_edit_button_srgb(&mut fx.bg_color).on_hover_text("Fills transparent areas");
            }
        });
    });
    group_ex(ui, "Transparency", false, on(&[fx.rc_white, fx.rc_black, fx.rc_custom]), |ui| {
        ui.horizontal(|ui| {
            ui.label("Remove");
            ui.toggle_value(&mut fx.rc_white, "White");
            ui.toggle_value(&mut fx.rc_black, "Black");
            ui.toggle_value(&mut fx.rc_custom, "Colour");
            if fx.rc_custom {
                ui.color_edit_button_srgb(&mut fx.rc_color);
            }
        });
        if fx.rc_custom {
            let mut hex = format!("#{:02x}{:02x}{:02x}", fx.rc_color[0], fx.rc_color[1], fx.rc_color[2]);
            if ui.add(egui::TextEdit::singleline(&mut hex).desired_width(80.0)).changed() {
                if let Some(c) = parse_hex(&hex) {
                    fx.rc_color = c;
                }
            }
        }
        if fx.rc_white || fx.rc_black || fx.rc_custom {
            ui.add(soft_slider(&mut fx.rc_fuzz, 0.0..=60.0, "fuzz %")).on_hover_text("How close a colour must be to count");
            ui.horizontal(|ui| {
                ui.checkbox(&mut fx.rc_clean_edges, "Clean edges").on_hover_text("Removes leftover fringe around cut-out areas");
                if fx.rc_clean_edges {
                    ui.add(egui::DragValue::new(&mut fx.rc_edge_px).range(1..=50).suffix(" px"));
                }
            });
        }
    });
    group_ex(ui, "Animation", false, on(&[tr.reverse, tr.boomerang, tr.set_loop, fx.counter, fx.fade_in, fx.fade_out]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut tr.reverse, "Reverse");
            ui.toggle_value(&mut tr.boomerang, "Boomerang");
            ui.toggle_value(&mut fx.counter, "Timer").on_hover_text("Shows a running seconds counter");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut tr.set_loop, "Loop count");
            if tr.set_loop {
                ui.add(egui::DragValue::new(&mut tr.loop_count).range(0..=65535)).on_hover_text("0 loops forever");
            }
        });
        ui.horizontal(|ui| {
            ui.toggle_value(&mut fx.fade_in, "Fade in");
            ui.toggle_value(&mut fx.fade_out, "Fade out");
            if fx.fade_in || fx.fade_out {
                ui.add(egui::DragValue::new(&mut fx.fade_frames).range(1..=10000).suffix(" fr"));
                ui.checkbox(&mut fx.fade_to_transparent, "to clear").on_hover_text("Fade to transparent instead of black");
            }
        });
    });
    group_ex(ui, "Blur & sharpen", false, on(&[fx.blur, fx.sharpen]), |ui| {
        ui.checkbox(&mut fx.blur, "Blur");
        if fx.blur {
            ui.add(soft_slider(&mut fx.blur_sigma, 0.3..=20.0, "σ"));
        }
        ui.checkbox(&mut fx.sharpen, "Sharpen");
        if fx.sharpen {
            ui.add(soft_slider(&mut fx.sharpen_sigma, 0.3..=10.0, "σ"));
        }
    });
    group_ex(ui, "Dither", false, on(&[fx.ordered_dither, fx.error_dither]), |ui| {
        ui.horizontal(|ui| {
            ui.toggle_value(&mut fx.ordered_dither, "Ordered").on_hover_text("Regular pattern dithering");
            ui.toggle_value(&mut fx.error_dither, "Diffusion").on_hover_text("Error diffusion dithering");
        });
        if fx.ordered_dither || fx.error_dither {
            ui.add(egui::Slider::new(&mut fx.dither_levels, 2..=16).text("levels").clamping(egui::SliderClamping::Never));
        }
    });
    group_ex(ui, "Artistic", false, on(&[fx.oil, fx.emboss, fx.posterize, fx.solarize, fx.noise]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut fx.oil, "Oil paint").on_hover_text("Smooths detail into brush-like patches");
            ui.toggle_value(&mut fx.emboss, "Emboss").on_hover_text("Raised relief look");
            ui.toggle_value(&mut fx.posterize, "Posterize").on_hover_text("Fewer colour steps");
            ui.toggle_value(&mut fx.solarize, "Solarize").on_hover_text("Inverts the brightest tones");
            ui.toggle_value(&mut fx.noise, "Noise").on_hover_text("Adds random grain");
        });
        if fx.oil {
            ui.add(egui::Slider::new(&mut fx.oil_radius, 1..=8).text("oil radius").clamping(egui::SliderClamping::Never));
        }
        if fx.posterize {
            ui.add(egui::Slider::new(&mut fx.posterize_levels, 2..=16).text("levels").clamping(egui::SliderClamping::Never));
        }
        if fx.noise {
            ui.add(soft_slider(&mut fx.noise_amount, 1.0..=128.0, "noise"));
        }
    });
    group_ex(ui, "Distort", false, on(&[fx.swirl, fx.implode, fx.wave]), |ui| {
        ui.horizontal(|ui| {
            ui.toggle_value(&mut fx.swirl, "Swirl").on_hover_text("Twists the image around the centre");
            ui.toggle_value(&mut fx.implode, "Implode").on_hover_text("Pulls the image toward the centre");
            ui.toggle_value(&mut fx.wave, "Wave").on_hover_text("Ripples the image sideways");
        });
        if fx.swirl {
            ui.add(soft_slider(&mut fx.swirl_deg, -720.0..=720.0, "swirl °"));
        }
        if fx.implode {
            ui.add(soft_slider(&mut fx.implode_amount, -2.0..=2.0, "implode"));
        }
        if fx.wave {
            ui.add(soft_slider(&mut fx.wave_amp, 1.0..=100.0, "amplitude"));
            ui.add(soft_slider(&mut fx.wave_len, 4.0..=500.0, "wavelength"));
        }
    });
    group_ex(ui, "Filters", false, on(&[fx.gotham, fx.lomo, fx.nashville, fx.toaster, fx.vignette, fx.polaroid]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut fx.gotham, "Gotham").on_hover_text("Cool and gritty high contrast");
            ui.toggle_value(&mut fx.lomo, "Lomo").on_hover_text("Saturated with dark corners");
            ui.toggle_value(&mut fx.nashville, "Nashville").on_hover_text("Warm pink vintage tint");
            ui.toggle_value(&mut fx.toaster, "Toaster").on_hover_text("Warm faded film look");
            ui.toggle_value(&mut fx.vignette, "Vignette").on_hover_text("Darkens the corners");
            ui.toggle_value(&mut fx.polaroid, "Polaroid").on_hover_text("White instant-photo frame");
        });
    });
    group_ex(ui, "Frame", false, on(&[fx.frame_camera, fx.frame_fuzzy, fx.frame_rounded, fx.frame_border]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut fx.frame_camera, "Camera").on_hover_text("Viewfinder frame");
            ui.toggle_value(&mut fx.frame_fuzzy, "Fuzzy").on_hover_text("Soft blurred edges");
            ui.toggle_value(&mut fx.frame_rounded, "Rounded").on_hover_text("Rounded corners");
            ui.toggle_value(&mut fx.frame_border, "Border").on_hover_text("Solid border");
        });
        if fx.frame_rounded {
            ui.add(soft_slider(&mut fx.rounded_radius, 1.0..=200.0, "radius px"));
        }
        if fx.frame_border {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut fx.border_px).range(1..=2000).suffix(" px"));
                ui.color_edit_button_srgb(&mut fx.border_color);
            });
        }
    });
    group_ex(ui, "Transform", false, on(&[fx.flip_v, fx.flip_h, fx.rot90, fx.rot270, fx.rot_any]), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(&mut fx.flip_h, "↔ Flip");
            ui.toggle_value(&mut fx.flip_v, "↕ Flip");
            ui.toggle_value(&mut fx.rot90, "↻ 90°");
            ui.toggle_value(&mut fx.rot270, "↺ 90°");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.rot_any, "Rotate");
            ui.add(egui::DragValue::new(&mut fx.rot_deg).speed(0.5).suffix("°"));
        });
    });
}

// ================================================================ helpers

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
