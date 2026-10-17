//! The egui application.

use crate::anim::{effective_delay, Animation};
use crate::effects::{CensorMethod, CensorSettings, EffectSettings};
use crate::geo::{map_box, source_angle, unmap_box, Affine, Obb};
use crate::encode::GifOptions;
use crate::io::{self, SheetOptions, SplitFormat, VideoFormat};
use crate::ops::{self, AspectMode, ResizeFilter};
use crate::overlay::{OverlaySource, Placement};
use crate::stack::{self, Kind, LayerKind, OverlayLayer, PlanFrame, SpeedMode, Stack, Transform};
use crate::text::{self, Align, Annotation, Border, FontEntry};
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
    Cut,
    Split,
    Frames,
    Export,
}

impl Tool {
    const ALL: [Tool; 14] = [
        Tool::Effects,
        Tool::Text,
        Tool::Censor,
        Tool::AddImage,
        Tool::Resize,
        Tool::Crop,
        Tool::Rotate,
        Tool::Reverse,
        Tool::Speed,
        Tool::Cut,
        Tool::Frames,
        Tool::Optimize,
        Tool::Split,
        Tool::Export,
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
            Tool::Text => "Add text",
            Tool::Censor => "Censor",
            Tool::AddImage => "Add image",
            Tool::Cut => "Cut",
            Tool::Split => "Split",
            Tool::Frames => "Frames",
            Tool::Export => "Save / Export",
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
            Tool::Cut => "✄",
            Tool::Split => "⊞",
            Tool::Frames => "▤",
            Tool::Export => "⬇",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Tool::Resize => "Resize",
            Tool::Crop => "Crop",
            Tool::Rotate => "Rotate / flip",
            Tool::Reverse => "Reverse / loop",
            Tool::Speed => "Speed",
            Tool::Optimize => "Optimize & compress",
            Tool::Effects => "Effects",
            Tool::Text => "Text layers",
            Tool::Censor => "Censor regions",
            Tool::AddImage => "Image overlays",
            Tool::Cut => "Cut",
            Tool::Split => "Split into frames",
            Tool::Frames => "Frame editor",
            Tool::Export => "Save / convert",
        }
    }
    fn layer_kind(self) -> Option<Kind> {
        match self {
            Tool::Effects => Some(Kind::Effects),
            Tool::Text => Some(Kind::Text),
            Tool::Censor => Some(Kind::Censor),
            Tool::AddImage => Some(Kind::Overlay),
            _ => None,
        }
    }
    fn is_transform_tool(self) -> bool {
        matches!(self, Tool::Resize | Tool::Crop | Tool::Rotate | Tool::Reverse | Tool::Speed | Tool::Cut)
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
    }
}

// ================================================================ jobs

enum JobOutput {
    Anim(anyhow::Result<Animation>),
    Baked(anyhow::Result<(Animation, Stack)>),
    Loaded(anyhow::Result<(Animation, Option<PathBuf>, Option<u64>)>),
    Info(anyhow::Result<String>),
    Overlay(anyhow::Result<OverlaySource>),
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
    fn label(self) -> &'static str {
        match self {
            ViewSource::Edit => "Live edit",
            ViewSource::Original => "Original file",
            ViewSource::Compressed => "Live edit (compressed)",
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

fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::Effects => "fx",
        Kind::Text => "text",
        Kind::Censor => "censor",
        Kind::Overlay => "overlay",
    }
}

// ---------------------------------------------------------------- Breeze-ish widgets

/// A Breeze group box: title above a framed area.
fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let pal = p();
    if !title.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(title).font(theme::medium(13.5)).color(pal.fg));
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
    r
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
    ui.add_enabled(enabled, egui::Button::new(txt).frame_when_inactive(false).min_size(vec2(0.0, 30.0)))
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
            let out = f(&p2);
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
                self.set_base_with_undo(a, None);
                self.info(format!("✔ {label}"));
            }
            JobOutput::Baked(Ok((a, s))) => {
                self.set_base_with_undo(a, Some(s));
                self.info(format!("✔ {label}"));
            }
            JobOutput::Loaded(Ok((a, path, size))) => {
                self.undo.clear();
                self.redo.clear();
                self.path = path;
                self.file_size = size;
                self.modified = false;
                self.cmp = None;
                self.original = Some(a.clone());
                self.orig_gen += 1;
                self.base = Some(a);
                self.stack = Stack::default();
                self.sel.clear();
                self.cur = 0;
                self.elapsed = 0.0;
                self.views = [ViewState::default(); 3];
                self.bump();
                self.after_base_change(true);
                let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sequence".into());
                self.info(format!("Opened {name}"));
                // scripting hook: GIFFORGE_OVERLAY=path adds an image layer on open
                if let Ok(ov) = std::env::var("GIFFORGE_OVERLAY") {
                    if let Ok(a) = io::load_animation(std::path::Path::new(&ov)) {
                        self.add_overlay(OverlaySource { name: ov.rsplit('/').next().unwrap_or("overlay").into(), delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image).clone()).collect() });
                    }
                }
            }
            JobOutput::Info(Ok(s)) => self.info(s),
            JobOutput::Overlay(Ok(src)) => {
                self.add_overlay(src);
                self.info("Overlay added – drag it on the preview to position it");
            }
            JobOutput::Anim(Err(e)) | JobOutput::Baked(Err(e)) | JobOutput::Loaded(Err(e)) | JobOutput::Info(Err(e)) | JobOutput::Overlay(Err(e)) => self.error(format!("{label} failed: {e:#}")),
        }
    }

    fn bump(&mut self) {
        self.version += 1;
        self.timing_rev += 1;
    }

    fn set_base_with_undo(&mut self, a: Animation, new_stack: Option<Stack>) {
        let size_changed = self.base.as_ref().map(|o| (o.width(), o.height(), o.len())) != Some((a.width(), a.height(), a.len()));
        if let Some(old) = self.base.take() {
            self.push_undo(Snapshot { anim: old, stack: self.stack.clone() });
        }
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
        if self.undo.len() > 60 {
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
        if let Some(a) = &self.base {
            let s = Snapshot { anim: a.clone(), stack: self.stack.clone() };
            self.push_undo(s);
            self.redo.clear();
        }
        self.last_coalesce = Some((key.to_string(), now));
        self.modified = true;
    }

    fn do_undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            if let Some(cur) = self.base.replace(prev.anim) {
                self.redo.push(Snapshot { anim: cur, stack: std::mem::replace(&mut self.stack, prev.stack) });
            }
            self.last_coalesce = None;
            self.bump();
            self.after_base_change(false);
            self.info("Undo");
        }
    }

    fn do_redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            if let Some(cur) = self.base.replace(next.anim) {
                self.undo.push(Snapshot { anim: cur, stack: std::mem::replace(&mut self.stack, next.stack) });
            }
            self.bump();
            self.after_base_change(false);
            self.info("Redo");
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
            .add_filter("Animations & images", &["gif", "png", "apng", "webp", "jpg", "jpeg", "bmp"])
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
        self.spawn(ctx, "Open", 1, move |_| {
            let size = std::fs::metadata(&p).ok().map(|m| m.len());
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
                cmd && i.key_pressed(egui::Key::S),
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
        if save {
            self.save_gif_dialog(ctx);
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
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).min(0.25) as f64;
        self.elapsed += dt;
        let n = self.plan.len();
        if !self.plan[self.cur].enabled {
            self.cur = self.next_enabled(self.cur);
        }
        let mut guard = 0;
        loop {
            let d = effective_delay(self.plan[self.cur].delay) as f64 / 100.0;
            if self.elapsed < d || guard > n {
                let remain = (d - self.elapsed).max(0.005);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(remain));
                break;
            }
            self.elapsed -= d;
            self.cur = self.next_enabled(self.cur);
            guard += 1;
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

    fn render_composite(&self, i: usize) -> Option<RgbaImage> {
        let base = self.base.as_ref()?;
        if i >= self.plan.len() {
            return None;
        }
        let prep = self.stack.prepare(base, &self.fonts);
        Some(self.stack.render_frame(base, &self.plan, &self.plan_times, i, &self.fonts, &prep))
    }

    /// Texture of output frame `i` with all live edits applied.
    fn composite_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        if !self.stack.needs_render() {
            let src = self.plan.get(i)?.src;
            return self.base_texture(ctx, src);
        }
        let key = self.comp_key;
        if let Some(t) = self.comp_tex.get(key, i) {
            return Some(t);
        }
        let img = self.render_composite(i)?;
        let o = self.tex_opts();
        Some(self.comp_tex.put(ctx, i, &img, o, "comp"))
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
            let missing: Vec<usize> = (0..plan.len()).filter(|i| !(self.comp_tex.key == key && self.comp_tex.map.contains_key(i))).collect();
            let cancel = self.prerender_cancel.clone();
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                use rayon::prelude::*;
                let prep = stack.prepare(&base, &fonts);
                missing.par_iter().for_each_with(tx, |tx, &i| {
                    if cancel.load(Ordering::Relaxed) != key {
                        return;
                    }
                    let img = stack.render_frame(&base, &plan, &times, i, &fonts, &prep);
                    let _ = tx.send((key, i, img));
                    ctx2.request_repaint();
                });
            });
            self.prerender = Some(Prerender { key, rx });
        } else if self.prerender.is_none() && !stable && self.stack.needs_render() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    // ------------------------------------------------------------ compare / compressed output

    fn cmp_key(&self) -> u64 {
        hash_parts(&[self.stack.signature(), self.version, self.timing_rev, hash_str(&format!("{:?}", self.gif_opts)), self.stack.plan_signature()])
    }

    fn wants_compressed(&self) -> bool {
        (self.compare && (self.view_left == ViewSource::Compressed || self.view_right == ViewSource::Compressed)) || self.tool == Tool::Optimize
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
        let opts = self.gif_opts.clone();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let res = (|| {
                let out = stack.flatten(&base, &fonts, None);
                let bytes = crate::encode::encode_gif(&out, &opts, None)?;
                let anim = io::decode_gif_bytes(&bytes)?;
                Ok((anim, bytes))
            })();
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
        theme::set_current(&ctx);
        let now = ctx.input(|i| i.time);
        self.poll_job(&ctx);
        self.handle_dropped(&ctx);
        self.shortcuts(&ctx);
        self.refresh_plan();
        self.advance(&ctx);
        self.manage_prerender(&ctx, now);
        self.manage_compare(&ctx, now);
        let pal = *p();

        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(pal.header).inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 6 }))
            .show(ui, |ui| self.toolbar(ui, &ctx));

        egui::Panel::bottom("status").frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(10, 4))).show(ui, |ui| self.status_bar(ui));

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(196.0)
            .size_range(160.0..=300.0)
            .frame(egui::Frame::new().fill(pal.view).inner_margin(egui::Margin { left: 6, right: 6, top: 8, bottom: 6 }))
            .show(ui, |ui| self.sidebar(ui, &ctx));

        if self.base.is_some() {
            egui::Panel::right("options")
                .resizable(true)
                .default_size(380.0)
                .size_range(300.0..=640.0)
                .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin { left: 12, right: 10, top: 10, bottom: 8 }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(self.tool.icon()).size(20.0).color(pal.highlight));
                        ui.heading(self.tool.title());
                        if self.tool.is_transform_tool() || self.tool.layer_kind().is_some() {
                            ui.label(RichText::new("LIVE").size(10.5).strong().color(pal.positive));
                        }
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.add_enabled_ui(!self.busy(), |ui| self.tool_panel(ui, &ctx));
                    });
                });

            egui::Panel::bottom("transport")
                .frame(egui::Frame::new().fill(pal.window).inner_margin(egui::Margin::symmetric(10, 6)))
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

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let r = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("gifforge_drop_overlay")));
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(150));
            painter.rect_stroke(r.shrink(12.0), 6.0, Stroke::new(2.0, pal.highlight), egui::StrokeKind::Inside);
            let msg = match (self.base.is_some(), self.tool) {
                (true, Tool::AddImage) => "Drop to add as an image overlay",
                (true, Tool::Frames) => "Drop to append frames",
                _ => "Drop to open (multiple files = image sequence)",
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
            if tool_button(ui, "⏏", "Open…", idle).on_hover_text("Ctrl+O").clicked() {
                self.open_dialog(ctx);
            }
            if tool_button(ui, "⊞", "Open sequence…", idle).clicked() {
                self.open_sequence_dialog(ctx);
            }
            let has = self.base.is_some();
            if tool_button(ui, "⬇", "Save GIF…", idle && has).on_hover_text("Ctrl+S – applies all live edits and uses the Optimize settings").clicked() {
                self.save_gif_dialog(ctx);
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            if tool_button(ui, "↶", "Undo", idle && !self.undo.is_empty()).on_hover_text("Ctrl+Z").clicked() {
                self.do_undo();
            }
            if tool_button(ui, "↷", "Redo", idle && !self.redo.is_empty()).on_hover_text("Ctrl+Y / Ctrl+Shift+Z").clicked() {
                self.do_redo();
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let cmp_on = self.compare;
            let r = ui.add_enabled(has, egui::Button::new(RichText::new("◫  Compare").color(if cmp_on { Color32::WHITE } else { pal.fg })).selected(cmp_on).frame_when_inactive(cmp_on).min_size(vec2(0.0, 30.0)));
            if r.on_hover_text("Two synchronised viewports (C)").clicked() {
                self.compare = !self.compare;
            }
            if self.compare {
                ui.add_space(6.0);
                for (id, side, val) in [("vl", "Left:", &mut self.view_left), ("vr", "Right:", &mut self.view_right)] {
                    ui.label(RichText::new(side).size(12.0).color(pal.fg_muted));
                    egui::ComboBox::from_id_salt(id).width(150.0).selected_text(val.label()).show_ui(ui, |ui| {
                        for s in [ViewSource::Edit, ViewSource::Original, ViewSource::Compressed] {
                            ui.selectable_value(val, s, s.label());
                        }
                    });
                    ui.add_space(4.0);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let before = self.theme;
                egui::ComboBox::from_id_salt("theme").width(130.0).selected_text(format!("◐  {}", self.theme.label())).show_ui(ui, |ui| {
                    for t in [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light] {
                        ui.selectable_value(&mut self.theme, t, t.label());
                    }
                    if cfg!(all(unix, not(target_os = "macos"))) {
                        ui.separator();
                        let mut native = theme::get_setting("wayland").as_deref() == Some("native");
                        if ui.checkbox(&mut native, "Native Wayland (restart)").on_hover_text("Off (default): run through XWayland so drag-and-drop works.\nOn: native Wayland – sharper on fractional scaling, but no drag-and-drop (paste with Ctrl+V instead).").changed() {
                            theme::set_setting("wayland", if native { "native" } else { "xwayland" });
                        }
                    }
                });
                if before != self.theme {
                    self.theme.apply(ctx);
                    theme::save_theme_choice(self.theme);
                }
            });
        });
        if let Some(a) = &self.base {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.add_space(6.0);
                let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
                ui.label(RichText::new(name).font(theme::medium(13.0)).color(pal.fg));
                if self.modified {
                    ui.label(RichText::new("● modified").size(12.0).color(pal.neutral));
                }
                let m = |s: String| RichText::new(s).size(12.0).color(pal.fg_muted);
                if let Some(s) = self.file_size {
                    ui.label(m(format!("File size: {}", fmt_size(s))));
                }
                let (ow, oh) = self.out_size;
                if (ow as u32, oh as u32) != (a.width(), a.height()) {
                    ui.label(m(format!("{}×{} → {}×{} px", a.width(), a.height(), ow as u32, oh as u32)));
                } else {
                    ui.label(m(format!("{}×{} px", a.width(), a.height())));
                }
                let dur: f32 = self.plan.iter().filter(|f| f.enabled).map(|f| effective_delay(f.delay) as f32 / 100.0).sum();
                ui.label(m(format!("{} frames", self.plan.len())));
                ui.label(m(format!("{dur:.2} s")));
                ui.label(m(format!("loop {}", self.stack.loop_count(a).map(|n| format!("{n}×")).unwrap_or_else(|| "forever".into()))));
                let edits = self.stack.layers.len() + (!self.stack.transform.is_identity()) as usize;
                if edits > 0 {
                    ui.label(RichText::new(format!("{edits} live edit{}", if edits == 1 { "" } else { "s" })).size(12.0).color(pal.highlight));
                }
            });
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 4.0, Stroke::new(1.0, pal.border));
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                ui.spinner();
                let done = job.progress.load(Ordering::Relaxed).min(job.total);
                ui.label(format!("{}…", job.label));
                if job.total > 1 {
                    ui.add(egui::ProgressBar::new(done as f32 / job.total as f32).desired_width(220.0).desired_height(14.0).fill(pal.highlight));
                }
            } else if let Some(s) = &self.status {
                ui.label(RichText::new(&s.text).size(12.5).color(if s.error { pal.negative } else { pal.fg_muted }));
            } else {
                ui.label(RichText::new("Ready – drop a GIF, APNG, WebP or images onto the window").size(12.5).color(pal.fg_muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Wheel zoom · middle/right-drag pan · F fit · 1 = 100% · Space play · ←/→ step · C compare").size(11.5).color(pal.fg_muted));
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let section = |ui: &mut egui::Ui, s: &str| {
                ui.add_space(2.0);
                ui.label(RichText::new(s).size(11.5).color(pal.fg_muted));
            };
            section(ui, "Layers");
            for (i, t) in Tool::ALL.iter().copied().enumerate() {
                if i == 4 {
                    ui.add_space(4.0);
                    section(ui, "Frame edits");
                }
                if i == 10 {
                    ui.add_space(4.0);
                    section(ui, "Frames & output");
                }
                self.sidebar_row(ui, t);
            }
            if self.base.is_some() {
                ui.add_space(10.0);
                section(ui, "Layer stack (top → bottom)");
                self.layer_list(ui, ctx);
            }
        });
    }

    fn sidebar_row(&mut self, ui: &mut egui::Ui, t: Tool) {
        let pal = *p();
        let sel = self.tool == t;
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let painter = ui.painter();
        if sel {
            painter.rect_filled(r, 3.0, pal.highlight_soft);
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, pal.highlight), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_filled(r, 3.0, mix(pal.view, pal.highlight_soft, 0.5));
        }
        let icon_col = if sel { pal.highlight } else { pal.fg };
        painter.text(r.left_center() + vec2(18.0, 0.0), egui::Align2::CENTER_CENTER, t.icon(), egui::FontId::proportional(16.0), icon_col);
        painter.text(r.left_center() + vec2(36.0, 0.0), egui::Align2::LEFT_CENTER, t.label(), egui::FontId::proportional(13.5), pal.fg);
        let tr = &self.stack.transform;
        let badge: Option<String> = match t {
            Tool::Effects | Tool::Text | Tool::Censor | Tool::AddImage => {
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
            let c = r.right_center() - vec2(14.0, 0.0);
            if b == "●" {
                painter.circle_filled(c, 4.0, pal.positive);
            } else {
                painter.circle_filled(c, 8.0, pal.highlight);
                painter.text(c, egui::Align2::CENTER_CENTER, b, egui::FontId::proportional(10.5), Color32::WHITE);
            }
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.tool = t;
            self.drag = None;
        }
    }

    fn layer_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let now = ctx.input(|i| i.time);
        if self.stack.layers.is_empty() {
            hint(ui, "No layers yet. Effects, text, censor regions and images appear here; drag ≡ to reorder.");
        }
        let n = self.stack.layers.len();
        let mut goto: Option<(Kind, u64)> = None;
        let mut remove: Option<usize> = None;
        let mut mv: Option<(usize, usize)> = None; // (from, insert position)
        let mut toggled = false;
        for li in (0..n).rev() {
            let (id, name, kind, mut visible) = {
                let l = &self.stack.layers[li];
                (l.id, l.name(), l.kind(), l.visible)
            };
            let selected = self.sel.get(kind_key(kind)) == Some(&id) && self.tool == kind_tool(kind);
            let row = egui::Frame::new()
                .fill(if selected { pal.highlight_soft } else { Color32::TRANSPARENT })
                .stroke(Stroke::new(1.0, if selected { pal.highlight } else { Color32::TRANSPARENT }))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::symmetric(2, 1))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        ui.dnd_drag_source(egui::Id::unique(("layer-drag", id)), li, |ui| {
                            ui.label(RichText::new("≡").color(pal.fg_muted)).on_hover_cursor(egui::CursorIcon::Grab);
                        });
                        let eye = ui.add(egui::Button::new(RichText::new(if visible { "◉" } else { "○" }).color(if visible { pal.highlight } else { pal.fg_muted })).frame_when_inactive(false)).on_hover_text("Show / hide");
                        if eye.clicked() {
                            visible = !visible;
                            toggled = true;
                        }
                        let w = (ui.available_width() - 60.0).max(30.0);
                        ui.allocate_ui(vec2(w, 22.0), |ui| {
                            let txt = RichText::new(&name).size(12.5).color(if visible { pal.fg } else { pal.fg_muted });
                            if ui.add(egui::Label::new(txt).truncate().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                goto = Some((kind, id));
                            }
                        });
                        if small_flat(ui, "▲", "Move up") && li + 1 < n {
                            mv = Some((li, li + 2));
                        }
                        if small_flat(ui, "▼", "Move down") && li > 0 {
                            mv = Some((li, li - 1));
                        }
                        if small_flat(ui, "✕", "Delete layer") {
                            remove = Some(li);
                        }
                    });
                })
                .response;
            // drop target
            let resp = ui.interact(row.rect, egui::Id::unique(("layer-row", id)), Sense::hover());
            if let Some(from) = resp.dnd_hover_payload::<usize>() {
                if *from != li {
                    let above = ui.input(|i| i.pointer.hover_pos()).map(|p| p.y < row.rect.center().y).unwrap_or(true);
                    let y = if above { row.rect.top() } else { row.rect.bottom() };
                    ui.painter().hline(row.rect.x_range(), y, Stroke::new(2.0, pal.highlight));
                }
            }
            if let Some(from) = resp.dnd_release_payload::<usize>() {
                let above = ui.input(|i| i.pointer.interact_pos()).map(|p| p.y < row.rect.center().y).unwrap_or(true);
                // list is top-first: "above" in the list means higher in the stack
                let pos = if above { li + 1 } else { li };
                mv = Some((*from, pos));
            }
            if toggled {
                self.stack.layers[li].visible = visible;
                toggled = false;
                self.modified = true;
            }
        }
        if let Some((k, id)) = goto {
            self.tool = kind_tool(k);
            self.select(k, id);
        }
        if let Some((from, pos)) = mv {
            if from < n && pos <= n && pos != from && pos != from + 1 {
                self.snapshot(&format!("reorder{now}"), now);
                let l = self.stack.layers.remove(from);
                let pos = if pos > from { pos - 1 } else { pos };
                self.stack.layers.insert(pos.min(self.stack.layers.len()), l);
            }
        }
        if let Some(i) = remove {
            self.snapshot(&format!("del{now}"), now);
            self.stack.layers.remove(i);
        }
        ui.add_space(6.0);
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("⬇  Bake everything").on_hover_text("Permanently apply all layers and frame edits to the frames (undoable)").clicked() {
                    self.bake(ctx, false);
                }
            });
        });
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
        ui.painter().text(c - vec2(0.0, 48.0), egui::Align2::CENTER_CENTER, "▤", egui::FontId::proportional(56.0), pal.fg_muted);
        ui.painter().text(c + vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, "Drop a GIF, APNG or WebP here, or click to open", theme::medium(19.0), pal.fg);
        ui.painter().text(c + vec2(0.0, 44.0), egui::Align2::CENTER_CENTER, "Drop several PNG/JPG files at once to build an animation from a sequence.", egui::FontId::proportional(13.5), pal.fg_muted);
        ui.painter().text(c + vec2(0.0, 66.0), egui::Align2::CENTER_CENTER, "Everything runs locally.", egui::FontId::proportional(13.5), pal.fg_muted);
        if resp.clicked() && !self.busy() {
            self.open_dialog(ctx);
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let pal = *p();
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.top() - 6.0, Stroke::new(1.0, pal.border));
        let n = self.plan.len();
        let (delay, enabled) = self.plan.get(self.cur).map(|f| (f.delay, f.enabled)).unwrap_or((0, true));
        ui.horizontal(|ui| {
            if ui.button("|◀").on_hover_text("Previous frame (←)").clicked() {
                self.step(-1);
            }
            if ui.add(egui::Button::new(if self.playing { "❚❚  Pause" } else { "▶  Play" }).min_size(vec2(84.0, 0.0))).on_hover_text("Space").clicked() {
                self.playing = !self.playing;
            }
            if ui.button("▶|").on_hover_text("Next frame (→)").clicked() {
                self.step(1);
            }
            let mut f = self.cur + 1;
            ui.spacing_mut().slider_width = (ui.available_width() - 560.0).max(100.0);
            if ui.add(egui::Slider::new(&mut f, 1..=n.max(1)).show_value(false)).changed() {
                self.cur = f - 1;
                self.playing = false;
                self.elapsed = 0.0;
            }
            ui.label(RichText::new(format!("{}/{} · {} cs{}", self.cur + 1, n, delay, if enabled { "" } else { " · skipped" })).size(12.5));
            ui.separator();
            if ui.button("−").on_hover_text("Zoom out (−)").clicked() {
                self.zoom_to(self.cur_scale() / 1.25);
            }
            let mut pct = self.cur_scale() * 100.0;
            if ui.add(egui::DragValue::new(&mut pct).range(2.0..=6400.0).speed(1.0).suffix("%").max_decimals(0)).on_hover_text("Zoom – or use the mouse wheel over the image").changed() {
                self.zoom_to(pct / 100.0);
            }
            if ui.button("+").on_hover_text("Zoom in (+)").clicked() {
                self.zoom_to(self.cur_scale() * 1.25);
            }
            if ui.add(egui::Button::new("Fit").selected(self.views[self.active()].zoom.is_none())).on_hover_text("F").clicked() {
                self.zoom_fit();
            }
            if ui.button("1:1").on_hover_text("1").clicked() {
                self.zoom_to(1.0);
            }
            if self.compare {
                ui.checkbox(&mut self.link_views, "Link views").on_hover_text("Zoom and pan both viewports together. Off: each viewport has its own zoom/pan; the buttons act on the last one you used.");
            }
            ui.separator();
            ui.checkbox(&mut self.checker, "Checker");
            ui.checkbox(&mut self.pixelated, "Pixelated").on_hover_text("Nearest-neighbour preview scaling (good for pixel art)");
        });
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

        let mut y = rect.top() + 10.0;
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
        let t = self.play_time();
        let crop_mode = self.tool == Tool::Crop;
        match src {
            ViewSource::Edit => {
                let (tex, size) = if crop_mode {
                    if let Some(t) = self.composite_texture(ctx, self.cur) {
                        let s = t.size_vec2();
                        self.out_size = (s.x, s.y);
                    }
                    let s = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
                    let (w, h) = self.dims();
                    (self.base_texture(ctx, s), vec2(w as f32, h as f32))
                } else {
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = tex.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(self.out_size.0, self.out_size.1));
                    self.out_size = (size.x, size.y);
                    (tex, size)
                };
                let label = if crop_mode {
                    Some(("Crop: full frame before edits".to_string(), Color32::WHITE))
                } else if show_label {
                    Some(("Live edit · uncompressed".to_string(), Color32::WHITE))
                } else {
                    None
                };
                let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, label, None, false);
                let space = if crop_mode { Space::Source } else { Space::Output };
                self.canvas_interaction(ui, &resp, m, (size.x, size.y), space);
            }
            ViewSource::Original => {
                let Some(i) = self.original_index() else { return };
                let (size, same) = {
                    let o = self.original.as_ref().unwrap();
                    (vec2(o.width() as f32, o.height() as f32), (o.width(), o.height()) == self.dims())
                };
                let label = format!("Original{}", self.file_size.map(|s| format!(" · {}", fmt_size(s))).unwrap_or_default());
                let tex = self.orig_texture(ctx, i);
                let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, Some((label, Color32::WHITE)), None, false);
                if same {
                    self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Source);
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
                    let mut label = format!("Live edit · compressed · {}", fmt_size(n));
                    if let Some(o) = self.file_size {
                        let pct = (n as f64 / o as f64 - 1.0) * 100.0;
                        label.push_str(&format!(" ({}{:.0}%)", if pct >= 0.0 { "+" } else { "" }, pct));
                    }
                    let tex = self.cmp_texture(ctx, i);
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, Some((label, Color32::WHITE)), None, false);
                    if !crop_mode {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output);
                    }
                } else {
                    // compression pending / running: show the live preview with a warning
                    let tex = self.composite_texture(ctx, self.cur);
                    let size = tex.as_ref().map(|t| t.size_vec2()).unwrap_or(vec2(self.out_size.0, self.out_size.1));
                    let label = self.cmp.as_ref().map(|c| format!("Live edit · compressed · last {}", fmt_size(c.bytes.len() as u64))).unwrap_or_else(|| "Live edit · compressed".into());
                    let badge = if encoding { "⚠ Compressing… showing uncompressed preview" } else { "⚠ Waiting for edits to settle – uncompressed preview" };
                    let (resp, m) = self.viewport(ui, ctx, vi, rect, tex, size, Some((label, Color32::from_gray(200))), Some(badge.into()), encoding);
                    if !crop_mode {
                        self.canvas_interaction(ui, &resp, m, (size.x, size.y), Space::Output);
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
            self.source_view(ui, ctx, full, ViewSource::Edit, 0, false);
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
    fn canvas_interaction(&mut self, ui: &mut egui::Ui, resp: &egui::Response, m: Mapper, bounds: (f32, f32), space: Space) {
        let painter = ui.painter_at(resp.rect);
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
        let rot_handle = |o: &Obb| to_screen(o.from_local(0.0, -o.h / 2.0 - 24.0 / m.scale));
        let ptr_img = resp.interact_pointer_pos().map(|p| m.to_img(p));
        let shift = ui.input(|i| i.modifiers.shift);

        // ---- start of a drag: decide what is being edited
        if started {
            if let (Some(sp), Some(pi)) = (resp.interact_pointer_pos(), ptr_img) {
                let mut new_drag = None;
                if let Some((li, id, b, o)) = sel {
                    if rot_handle(&o).distance(sp) <= 10.0 {
                        let a0 = (pi.y - o.cy).atan2(pi.x - o.cx).to_degrees();
                        new_drag = Some(BoxDrag::Rotate { id, li, a0, out0: o, src: b });
                    } else if let Some(k) = o.corners().iter().position(|c| to_screen(*c).distance(sp) <= 9.0) {
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
                            let (dx, dy) = mp.inverse().apply_vec(pi.x - start.x, pi.y - start.y);
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
                painter.circle_filled(rh, 5.5, Color32::WHITE);
                painter.circle_stroke(rh, 5.5, Stroke::new(1.2, pal.highlight));
                for c in &pts {
                    let h = Rect::from_center_size(*c, vec2(8.0, 8.0));
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
                        if rot_handle(&c.3).distance(pp) <= 10.0 {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Alias);
                        } else if c.3.corners().iter().any(|q| to_screen(*q).distance(pp) <= 9.0) {
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
            LayerKind::Effects(_) => {}
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
            hint(ui, "Edits here happen underneath all live edits. Skipped frames are excluded from playback and export.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.fr_thumb, 56.0..=200.0).show_value(false));
                ui.label("Thumbnail size");
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
                                        if ui.small_button("◀").on_hover_text("Move left").clicked() {
                                            action = Some((i, "left"));
                                        }
                                        if ui.small_button("▶").on_hover_text("Move right").clicked() {
                                            action = Some((i, "right"));
                                        }
                                        if ui.small_button("✕").on_hover_text("Delete frame").clicked() {
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
            Tool::Cut => self.ui_cut(ui, ctx),
            Tool::Split => self.ui_split(ui, ctx),
            Tool::Frames => self.ui_frames(ui, ctx),
            Tool::Export => self.ui_export(ui, ctx),
        }
    }

    /// Header for live frame-edit tools: enable switch, reset, bake.
    fn transform_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, on: impl Fn(&mut Transform) -> &mut bool, reset: impl Fn(&mut Transform)) {
        let mut enabled = *on(&mut self.stack.transform);
        let mut do_reset = false;
        let mut do_bake = false;
        ui.horizontal(|ui| {
            ui.checkbox(&mut enabled, "Enabled");
            do_reset = ui.button("Reset").clicked();
            do_bake = ui.button("⬇ Bake frame edits").on_hover_text("Permanently apply crop/resize/rotate/speed/cut/reverse to the frames; layers stay live").clicked();
        });
        *on(&mut self.stack.transform) = enabled;
        if do_reset {
            reset(&mut self.stack.transform);
            *on(&mut self.stack.transform) = false;
        }
        if do_bake {
            self.bake(ctx, true);
        }
        hint(ui, "Live: applied underneath your effects and layers as you change it.");
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
        group(ui, "Size", |ui| {
            egui::Grid::new("rs").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("↔ Width:");
                ui.add(egui::DragValue::new(&mut tr.rs_w).range(0..=16384).suffix(" px"));
                hint(ui, "0 = auto");
                ui.end_row();
                ui.label("↕ Height:");
                ui.add(egui::DragValue::new(&mut tr.rs_h).range(0..=16384).suffix(" px"));
                hint(ui, "0 = auto");
                ui.end_row();
                ui.label("Percentage:");
                ui.add(egui::DragValue::new(&mut tr.rs_pct).range(0.0..=6400.0).suffix(" %"));
                hint(ui, "overrides w/h");
                ui.end_row();
            });
            ui.horizontal(|ui| {
                for pc in [25.0, 50.0, 200.0, 400.0] {
                    if ui.small_button(format!("{pc:.0}%")).clicked() {
                        tr.rs_pct = pc;
                    }
                }
                if ui.small_button("Clear").clicked() {
                    tr.rs_pct = 0.0;
                    tr.rs_w = 0;
                    tr.rs_h = 0;
                }
            });
        });
        group(ui, "If the aspect ratio does not match", |ui| {
            egui::ComboBox::from_id_salt("rsmode").selected_text(tr.rs_mode.label()).width(260.0).show_ui(ui, |ui| {
                for m in AspectMode::ALL {
                    ui.selectable_value(&mut tr.rs_mode, m, m.label());
                }
            });
            ui.add_space(4.0);
            ui.label("Resampling:");
            egui::ComboBox::from_id_salt("rsfilt").selected_text(tr.rs_filter.label()).width(260.0).show_ui(ui, |ui| {
                for f in ResizeFilter::ALL {
                    ui.selectable_value(&mut tr.rs_filter, f, f.label());
                }
            });
        });
        if before != (tr.rs_w, tr.rs_h, tr.rs_pct.to_bits(), tr.rs_mode, tr.rs_filter) {
            tr.resize_on = true;
        }
        match stack::resize_target(w, h, tr.rs_w, tr.rs_h, tr.rs_pct) {
            Some((tw, th)) => ui.label(format!("Result: {w}×{h} → {tw}×{th}")),
            None => ui.label(RichText::new("Enter a width, height or percentage").color(p().fg_muted)),
        };
        hint(ui, "Layer positions are in output pixels and are not rescaled.");
    }

    fn ui_crop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.crop_on, |_| {});
        let (w, h) = self.dims();
        hint(ui, "Drag on the image to select the area. Drag inside to move it, corners to resize. Alt-drag or middle/right-drag pans.");
        let tr = &mut self.stack.transform;
        let before = tr.crop;
        group(ui, "Selection", |ui| {
            let r = &mut tr.crop;
            ui.horizontal(|ui| {
                ui.label("Left:");
                ui.add(egui::DragValue::new(&mut r[0]).range(0..=w.saturating_sub(1)));
                ui.label("Top:");
                ui.add(egui::DragValue::new(&mut r[1]).range(0..=h.saturating_sub(1)));
            });
            ui.horizontal(|ui| {
                ui.label("Width:");
                ui.add(egui::DragValue::new(&mut r[2]).range(1..=w));
                ui.label("Height:");
                ui.add(egui::DragValue::new(&mut r[3]).range(1..=h));
            });
            r[2] = r[2].min(w.saturating_sub(r[0])).max(1);
            r[3] = r[3].min(h.saturating_sub(r[1])).max(1);
            if ui.button("Select all").clicked() {
                *r = [0, 0, w, h];
            }
        });
        group(ui, "Lock aspect ratio", |ui| {
            egui::ComboBox::from_id_salt("croplock").selected_text(self.crop_lock.label()).width(200.0).show_ui(ui, |ui| {
                for l in AspectLock::ALL {
                    ui.selectable_value(&mut self.crop_lock, l, l.label());
                }
            });
        });
        let mut detect = false;
        group(ui, "Auto-trim", |ui| {
            hint(ui, "Find the smallest box containing all non-background pixels across every frame (great for sprite exports).");
            ui.horizontal(|ui| {
                ui.label("Tolerance");
                ui.add(egui::DragValue::new(&mut self.trim_tol).range(0..=255));
                detect = ui.button("Detect").clicked();
            });
        });
        if detect {
            if let Some(a) = &self.base {
                match ops::auto_trim_rect(a, self.trim_tol) {
                    Some((x, y, w, h)) => self.stack.transform.crop = [x, y, w, h],
                    None => self.error("Nothing to trim – every pixel is background"),
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
            ui.checkbox(&mut tr.flip_v, "↕ Flip vertically");
            ui.checkbox(&mut tr.flip_h, "↔ Flip horizontally");
        });
        group(ui, "Rotate", |ui| {
            ui.horizontal(|ui| {
                for (deg, label) in [(0u16, "0°"), (90, "↻ 90°"), (180, "⟲ 180°"), (270, "↺ 270°")] {
                    if ui.selectable_label(tr.rot == deg, label).clicked() {
                        tr.rot = deg;
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr.rot_any, "Free rotation");
                if ui.add(egui::DragValue::new(&mut tr.rot_deg).speed(0.5).suffix("°")).changed() {
                    tr.rot_any = true;
                }
            });
            if ui.add(egui::Slider::new(&mut tr.rot_deg, -180.0..=180.0).show_value(false).clamping(egui::SliderClamping::Never)).changed() {
                tr.rot_any = true;
            }
            ui.checkbox(&mut tr.rot_expand, "Expand canvas to fit");
            ui.horizontal(|ui| {
                ui.checkbox(&mut tr.rot_bg_transparent, "Transparent background");
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
            ui.checkbox(&mut tr.boomerang, "⇄ Run to the end and back to the start (boomerang)");
            ui.checkbox(&mut tr.set_loop, "Change the number of times to play (loop)");
            if tr.set_loop {
                ui.indent("loopn", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut tr.loop_count).range(0..=65535));
                        hint(ui, "times (0 = forever)");
                    });
                });
            }
        });
        group(ui, "Flip", |ui| {
            if ui.checkbox(&mut tr.flip_v, "↕ Flip vertically").changed() {
                tr.rot_on = true;
            }
            if ui.checkbox(&mut tr.flip_h, "↔ Flip horizontally").changed() {
                tr.rot_on = true;
            }
        });
        hint(ui, "Live. The seconds counter and fades are in Effects → Animation flow.");
        if ui.button("⬇ Bake frame edits").clicked() {
            self.bake(ctx, true);
        }
    }

    fn ui_speed(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.transform_header(ui, ctx, |t| &mut t.speed_on, |t| {
            t.speed_mode = SpeedMode::Percent;
            t.speed_val = 100.0;
        });
        let (avg_fps, n) = self.base.as_ref().map(|a| (a.enabled_count() as f32 / a.duration_secs().max(0.001), a.len())).unwrap_or((0.0, 0));
        let tr = &mut self.stack.transform;
        let before = (tr.speed_mode, tr.speed_val.to_bits());
        group(ui, "Speed", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut tr.speed_val).range(0.1..=100000.0).speed(1.0));
                egui::ComboBox::from_id_salt("spmode")
                    .width(210.0)
                    .selected_text(match tr.speed_mode {
                        SpeedMode::Percent => "% of original speed",
                        SpeedMode::DelayAll => "delay for all frames (1/100 s)",
                        SpeedMode::Fps => "frames per second (all frames)",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::Percent, "% of original speed");
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::DelayAll, "delay for all frames (1/100 s)");
                        ui.selectable_value(&mut tr.speed_mode, SpeedMode::Fps, "frames per second (all frames)");
                    });
            });
            if tr.speed_mode == SpeedMode::Percent {
                ui.add(soft_slider(&mut tr.speed_val, 10.0..=400.0, "%"));
                ui.horizontal(|ui| {
                    for pc in [25.0, 50.0, 100.0, 150.0, 200.0, 300.0] {
                        if ui.small_button(format!("{pc:.0}%")).clicked() {
                            tr.speed_val = pc;
                        }
                    }
                });
            } else {
                let d = if tr.speed_mode == SpeedMode::Fps { (100.0 / tr.speed_val).round() } else { tr.speed_val };
                if d < 2.0 {
                    ui.label(RichText::new("⚠ Delays below 2 cs are played at 10 cs by most browsers.").color(p().neutral));
                }
                if tr.speed_mode == SpeedMode::Fps {
                    hint(ui, &format!("GIF delays are in 1/100 s, so {:.1} fps becomes {} cs ({:.2} fps).", tr.speed_val, d.max(1.0), 100.0 / d.max(1.0)));
                }
            }
            hint(ui, &format!("Source: {n} frames, average {avg_fps:.1} fps."));
        });
        if before != (tr.speed_mode, tr.speed_val.to_bits()) {
            tr.speed_on = true;
        }
        hint(ui, "Per-frame delays can be edited in the Frames tool.");
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
        ui.label(format!("Source: {n} frames, {dur:.2} seconds"));
        group(ui, "Cutting options", |ui| {
            egui::ComboBox::from_id_salt("cutmode").selected_text(if tr.cut_by_time { "Cut by time (seconds)" } else { "Cut by frame number" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut tr.cut_by_time, false, "Cut by frame number");
                ui.selectable_value(&mut tr.cut_by_time, true, "Cut by time (seconds)");
            });
            if tr.cut_by_time {
                ui.horizontal(|ui| {
                    ui.label("Start:");
                    ui.add(egui::DragValue::new(&mut tr.cut_t0).range(0.0..=dur).speed(0.01).suffix(" s"));
                    ui.label("End:");
                    ui.add(egui::DragValue::new(&mut tr.cut_t1).range(0.0..=dur).speed(0.01).suffix(" s"));
                });
            } else {
                let mut end = if tr.cut_end == 0 { n } else { tr.cut_end };
                ui.horizontal(|ui| {
                    ui.label("Start:");
                    ui.add(egui::DragValue::new(&mut tr.cut_start).range(1..=n));
                    ui.label("End:");
                    ui.add(egui::DragValue::new(&mut end).range(1..=n));
                });
                tr.cut_end = if end >= n { 0 } else { end };
                ui.horizontal(|ui| {
                    if ui.small_button("Start = current frame").clicked() {
                        tr.cut_start = src;
                    }
                    if ui.small_button("End = current frame").clicked() {
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
            hint(ui, &format!("Keeps source frames {}–{} ({} frames).", s + 1, e + 1, e - s + 1));
        }
    }

    fn ui_optimize(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        if ui.add(egui::Button::new(RichText::new("◫  Compare live edit vs compressed").color(if self.compare { Color32::WHITE } else { pal.fg })).selected(self.compare)).clicked() {
            self.compare = !self.compare;
        }
        hint(ui, "The compressed result re-encodes automatically when anything changes. These settings are used whenever a GIF is saved.");
        let cur_key = self.cmp_key();
        let busy = self.cmp_job.is_some();
        group(ui, "Result", |ui| match &self.cmp {
            Some(c) => {
                let n = c.bytes.len() as u64;
                let stale = c.key != cur_key;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Compressed: {}", fmt_size(n))).font(theme::medium(15.0)).color(if stale { pal.fg_muted } else { pal.positive }));
                    if stale || busy {
                        ui.spinner();
                    }
                });
                if let Some(orig) = self.file_size {
                    hint(ui, &format!("{:.0}% of the original {}", n as f64 / orig as f64 * 100.0, fmt_size(orig)));
                }
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Encoding…");
                });
            }
        });
        group(ui, "Presets", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Max quality").clicked() {
                    self.gif_opts = GifOptions { quant_speed: 1, ..GifOptions::default() };
                }
                if ui.button("Balanced").clicked() {
                    self.gif_opts = GifOptions { lossy: 30, ..GifOptions::default() };
                }
                if ui.button("Small").clicked() {
                    self.gif_opts = GifOptions { lossy: 60, colors: 128, global_palette: true, ..GifOptions::default() };
                }
                if ui.button("Tiny").clicked() {
                    self.gif_opts = GifOptions { lossy: 120, colors: 48, global_palette: true, ..GifOptions::default() };
                }
            });
        });
        let o = &mut self.gif_opts;
        group(ui, "Lossy compression", |ui| {
            ui.add(egui::Slider::new(&mut o.lossy, 0..=200).text("level").clamping(egui::SliderClamping::Never));
            hint(ui, "Similar colours may merge inside the LZW stream and between frames. 30–80 is usually invisible on footage.");
        });
        group(ui, "Colours", |ui| {
            ui.add(egui::Slider::new(&mut o.colors, 2..=256).text("max colours"));
            ui.checkbox(&mut o.global_palette, "Use one global palette for all frames");
            ui.checkbox(&mut o.dither, "Dither (Floyd–Steinberg)");
            ui.add(egui::Slider::new(&mut o.quant_speed, 1..=30).text("quantizer speed")).on_hover_text("1 = best palette, 30 = fastest");
        });
        group(ui, "Frames", |ui| {
            ui.checkbox(&mut o.frame_diff, "Store only changed pixels (frame differencing)");
            ui.checkbox(&mut o.merge_duplicates, "Merge identical consecutive frames");
            ui.add(egui::Slider::new(&mut o.alpha_threshold, 1..=255).text("alpha cut-off"));
            hint(ui, "Animations with transparency automatically use \"don't stack frames\" disposal.");
        });
        group(ui, "Drop frames", |ui| {
            ui.horizontal(|ui| {
                ui.label("Drop every");
                ui.add(egui::DragValue::new(&mut self.opt_drop_nth).range(0..=100));
                ui.label("th frame");
                if ui.add_enabled(self.opt_drop_nth >= 2, egui::Button::new("Apply")).clicked() {
                    let n = self.opt_drop_nth;
                    self.run_anim(ctx, "Drop frames", move |a, _| Ok(ops::drop_every_nth(a, n)));
                }
            });
            hint(ui, "Removes source frames (undoable); their time is given to the previous frame so speed is kept.");
        });
        if primary(ui, "Save GIF…") {
            self.save_gif_dialog(ctx);
        }
    }

    fn ui_effects(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let fx_layers: Vec<(usize, u64)> = self.stack.layers.iter().enumerate().filter(|(_, l)| l.kind() == Kind::Effects).map(|(i, l)| (i, l.id)).collect();
        let sel = self.sel_index(Kind::Effects);
        ui.horizontal_wrapped(|ui| {
            if fx_layers.len() > 1 {
                for (n, (i, id)) in fx_layers.iter().enumerate() {
                    if ui.selectable_label(Some(*i) == sel, format!("Effects #{}", n + 1)).clicked() {
                        self.select(Kind::Effects, *id);
                    }
                }
            }
            if !fx_layers.is_empty() && ui.button("+ New effects layer").on_hover_text("Effects layers can sit above or below text/images – reorder in the layer stack").clicked() {
                self.add_layer(LayerKind::Effects(Box::default()));
            }
        });
        let sel = self.sel_index(Kind::Effects);
        ui.horizontal(|ui| {
            if let Some(i) = sel {
                ui.checkbox(&mut self.stack.layers[i].visible, "Layer visible");
            }
            if ui.button("Reset").clicked() {
                match sel {
                    Some(i) => self.stack.layers[i].kind = LayerKind::Effects(Box::default()),
                    None => self.fx_draft = EffectSettings::default(),
                }
            }
            if ui.button("⬇ Bake everything").on_hover_text("Permanently apply all live edits to the frames").clicked() {
                self.bake(ctx, false);
            }
        });
        hint(ui, "Every effect you tick is live and they all stack. Sliders stop at sensible limits – type a value to go beyond.");
        let _ = pal;
        let Stack { transform, layers, .. } = &mut self.stack;
        let fx: &mut EffectSettings = match sel {
            Some(i) => match &mut layers[i].kind {
                LayerKind::Effects(f) => f,
                _ => &mut self.fx_draft,
            },
            None => &mut self.fx_draft,
        };
        effects_ui(ui, fx, transform);
        // first change with no effects layer creates one
        if sel.is_none() && self.fx_draft != EffectSettings::default() {
            let fx = std::mem::take(&mut self.fx_draft);
            self.add_layer(LayerKind::Effects(Box::new(fx)));
        }
    }

    /// Angle editor shared by text / censor / image layers.
    fn angle_row(ui: &mut egui::Ui, angle: &mut f32) {
        ui.horizontal(|ui| {
            ui.label("Rotation:");
            ui.add(egui::DragValue::new(angle).speed(0.5).suffix("°").max_decimals(1));
            for (lbl, v) in [("0°", 0.0), ("-90°", -90.0), ("90°", 90.0), ("180°", 180.0)] {
                if ui.small_button(lbl).clicked() {
                    *angle = v;
                }
            }
        });
        ui.add(egui::Slider::new(angle, -180.0..=180.0).show_value(false).clamping(egui::SliderClamping::Never));
        hint(ui, "Or drag the round handle above the box on the preview (Shift snaps to 15°).");
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
            f.faces.iter().find(|x| x.style == style).cloned()?
        };
        match crate::sysfonts::load(&face) {
            Ok(font) => {
                self.fonts.push(FontEntry { name: format!("{family} {style}"), font });
                let i = self.fonts.len() - 1;
                self.font_keys.insert(key, i);
                self.font_meta.insert(i, (family.to_string(), style.to_string()));
                Some(i)
            }
            Err(e) => {
                self.error(format!("Could not load font {family} {style}: {e:#}"));
                None
            }
        }
    }

    /// Font chooser: searchable list of built-in + system font families, plus style.
    fn font_picker(&mut self, ui: &mut egui::Ui, cur_idx: usize) -> Option<usize> {
        let pal = *p();
        let mut pick: Option<usize> = None;
        let mut pick_sys: Option<(String, String)> = None;
        let cur_name = self.fonts.get(cur_idx).map(|f| f.name.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Font:");
            let r = ui.add(egui::Button::new(RichText::new(format!("{cur_name}  ▾")).color(pal.fg)).min_size(vec2(200.0, 0.0)));
            if r.clicked() {
                self.font_picker_open = !self.font_picker_open;
            }
        });
        // style of the current family
        if let Some((fam, style)) = self.font_meta.get(&cur_idx).cloned() {
            let styles: Vec<String> = self.sys_fonts.lock().ok().and_then(|g| g.as_ref().and_then(|v| v.iter().find(|f| f.name == fam).map(|f| f.faces.iter().map(|x| x.style.clone()).collect()))).unwrap_or_default();
            if styles.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label("Style:");
                    egui::ComboBox::from_id_salt("fontstyle").width(160.0).selected_text(&style).show_ui(ui, |ui| {
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
            egui::Frame::new().fill(pal.view).stroke(Stroke::new(1.0, pal.border)).corner_radius(3.0).inner_margin(6.0).show(ui, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.font_search).hint_text("Search fonts…").desired_width(f32::INFINITY));
                if self.font_search.is_empty() && !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                let q = self.font_search.to_lowercase();
                egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.label(RichText::new("Built-in").size(11.5).color(pal.fg_muted));
                    for i in 0..self.builtin_font_count.min(self.fonts.len()) {
                        let n = self.fonts[i].name.clone();
                        if (q.is_empty() || n.to_lowercase().contains(&q)) && ui.selectable_label(i == cur_idx, &n).clicked() {
                            pick = Some(i);
                        }
                    }
                    let extra: Vec<(usize, String)> = (self.builtin_font_count..self.fonts.len()).filter(|i| !self.font_meta.contains_key(i)).map(|i| (i, self.fonts[i].name.clone())).collect();
                    if !extra.is_empty() {
                        ui.label(RichText::new("Loaded from file").size(11.5).color(pal.fg_muted));
                        for (i, n) in extra {
                            if (q.is_empty() || n.to_lowercase().contains(&q)) && ui.selectable_label(i == cur_idx, &n).clicked() {
                                pick = Some(i);
                            }
                        }
                    }
                    let guard = self.sys_fonts.lock().ok();
                    match guard.as_ref().and_then(|g| g.as_ref()) {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Scanning system fonts…");
                            });
                        }
                        Some(fams) => {
                            ui.label(RichText::new(format!("System ({} families)", fams.len())).size(11.5).color(pal.fg_muted));
                            let cur_fam = self.font_meta.get(&cur_idx).map(|m| m.0.clone());
                            for f in fams.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)) {
                                if ui.selectable_label(cur_fam.as_deref() == Some(f.name.as_str()), &f.name).clicked() {
                                    pick_sys = Some((f.name.clone(), f.default_face().style.clone()));
                                }
                            }
                        }
                    }
                });
            });
        }
        if let Some((fam, st)) = pick_sys {
            pick = self.font_index_for(&fam, &st);
        }
        if pick.is_some() {
            self.font_picker_open = false;
        }
        pick
    }

    fn ui_text(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        hint(ui, "Text layers are live and stick to the content: crops, resizes and rotations carry them along. Drag to move, corners to scale, the round handle to rotate.");
        let sel = self.sel_index(Kind::Text);
        ui.horizontal(|ui| {
            if ui.button("+ Add text").clicked() {
                let (w, h) = self.dims();
                self.add_layer(LayerKind::Text(Annotation { text: "Your text".into(), x: (w as i32 / 2 - 80).max(0), y: (h as i32 - 60).max(0), ..Default::default() }));
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("✕ Remove")).clicked() {
                self.stack.layers.remove(sel.unwrap());
            }
            if ui.button("Load font file…").on_hover_text("Load a .ttf / .otf font file").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf", "ttc"]).pick_file() {
                    match text::load_font_file(&path) {
                        Ok(f) => {
                            self.fonts.push(f);
                            let fi = self.fonts.len() - 1;
                            if let Some(i) = self.sel_index(Kind::Text) {
                                if let LayerKind::Text(a) = &mut self.stack.layers[i].kind {
                                    a.font_idx = fi;
                                }
                            }
                        }
                        Err(e) => self.error(format!("{e:#}")),
                    }
                }
            }
        });
        let texts: Vec<(usize, u64, String)> = self
            .stack
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match &l.kind {
                LayerKind::Text(a) => Some((i, l.id, a.text.lines().next().unwrap_or("").chars().take(14).collect())),
                _ => None,
            })
            .collect();
        if texts.is_empty() {
            hint(ui, "No text yet – click \"+ Add text\".");
            return;
        }
        let sel = self.sel_index(Kind::Text);
        ui.horizontal_wrapped(|ui| {
            for (i, id, name) in &texts {
                if ui.selectable_label(Some(*i) == sel, format!("T {name}")).clicked() {
                    self.select(Kind::Text, *id);
                }
            }
        });
        let Some(sel) = self.sel_index(Kind::Text) else { return };
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        // font picker first (needs &mut self)
        let cur_font = match &self.stack.layers[sel].kind {
            LayerKind::Text(a) => a.font_idx,
            _ => 0,
        };
        let mut picked = None;
        group(ui, "Font", |ui| {
            picked = self.font_picker(ui, cur_font);
        });
        if let (Some(fi), LayerKind::Text(a)) = (picked, &mut self.stack.layers[sel].kind) {
            // keep the text centred when its box size changes
            let (w0, h0) = text::measure(&self.fonts, a);
            let (cx, cy) = (a.x as f32 + w0 as f32 / 2.0, a.y as f32 + h0 as f32 / 2.0);
            a.font_idx = fi;
            let (w1, h1) = text::measure(&self.fonts, a);
            a.x = (cx - w1 as f32 / 2.0).round() as i32;
            a.y = (cy - h1 as f32 / 2.0).round() as i32;
        }
        let layer = &mut self.stack.layers[sel];
        let visible = &mut layer.visible;
        let LayerKind::Text(a) = &mut layer.kind else { return };
        group(ui, "Text", |ui| {
            ui.add(egui::TextEdit::multiline(&mut a.text).desired_rows(2).desired_width(f32::INFINITY));
            ui.checkbox(visible, "Visible");
            egui::Grid::new("txtgrid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Size / colour:");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut a.size).range(4.0..=4000.0).suffix(" px"));
                    ui.color_edit_button_srgba_unmultiplied(&mut a.color);
                });
                ui.end_row();
                ui.label("Source frames:");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut a.from).range(1..=n));
                    ui.label("–");
                    let mut to = if a.to == 0 { n } else { a.to };
                    if ui.add(egui::DragValue::new(&mut to).range(1..=n)).changed() {
                        a.to = if to >= n { 0 } else { to };
                    }
                    if a.to == 0 {
                        hint(ui, "(end)");
                    }
                });
                ui.end_row();
                ui.label("Align:");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("align").width(80.0).selected_text(format!("{:?}", a.align)).show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.align, Align::Left, "Left");
                        ui.selectable_value(&mut a.align, Align::Center, "Center");
                        ui.selectable_value(&mut a.align, Align::Right, "Right");
                    });
                    ui.label("Opacity:");
                    ui.add(egui::DragValue::new(&mut a.opacity).range(0..=100).suffix("%"));
                });
                ui.end_row();
                ui.label("Position:");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut a.x).prefix("x "));
                    ui.add(egui::DragValue::new(&mut a.y).prefix("y "));
                });
                ui.end_row();
                ui.label("Outline:");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("border").width(80.0).selected_text(format!("{:?}", a.border)).show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.border, Border::None, "None");
                        ui.selectable_value(&mut a.border, Border::Light, "Light");
                        ui.selectable_value(&mut a.border, Border::Heavy, "Heavy");
                    });
                    ui.color_edit_button_srgba_unmultiplied(&mut a.border_color);
                });
                ui.end_row();
                ui.label("Preset:");
                egui::ComboBox::from_id_salt("cpreset").width(150.0).selected_text("Choose…").show_ui(ui, |ui| {
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
            ui.add_space(4.0);
            Self::angle_row(ui, &mut a.angle);
        });
        hint(ui, "Position, size and frame numbers refer to the source frames, so the text follows crops, resizes, rotations, cuts and reversal.");
    }

    fn ui_censor(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let (w, h) = self.dims();
        let n = self.base.as_ref().map(|b| b.len()).unwrap_or(1).max(1);
        hint(ui, "Censor regions are live and stick to the content. Drag on the image to draw a new region; drag inside to move, corners to resize, the round handle to rotate.");
        let sel = self.sel_index(Kind::Censor);
        ui.horizontal(|ui| {
            if ui.button("+ Add region").clicked() {
                self.add_layer(LayerKind::Censor(CensorSettings { x: (w / 4) as i32, y: (h / 4) as i32, w: w / 2, h: h / 2, ..Default::default() }));
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("✕ Remove")).clicked() {
                self.stack.layers.remove(sel.unwrap());
            }
        });
        let regions: Vec<(usize, u64)> = self.stack.layers.iter().enumerate().filter(|(_, l)| l.kind() == Kind::Censor).map(|(i, l)| (i, l.id)).collect();
        if regions.is_empty() {
            hint(ui, "No regions yet – drag on the image or click \"+ Add region\".");
            return;
        }
        let sel = self.sel_index(Kind::Censor);
        ui.horizontal_wrapped(|ui| {
            for (k, (i, id)) in regions.iter().enumerate() {
                if ui.selectable_label(Some(*i) == sel, format!("Region #{}", k + 1)).clicked() {
                    self.select(Kind::Censor, *id);
                }
            }
        });
        let Some(sel) = self.sel_index(Kind::Censor) else { return };
        let layer = &mut self.stack.layers[sel];
        let visible = &mut layer.visible;
        let LayerKind::Censor(c) = &mut layer.kind else { return };
        group(ui, "Area (source-frame pixels)", |ui| {
            ui.checkbox(visible, "Visible");
            ui.horizontal(|ui| {
                ui.label("Left:");
                ui.add(egui::DragValue::new(&mut c.x));
                ui.label("Top:");
                ui.add(egui::DragValue::new(&mut c.y));
            });
            ui.horizontal(|ui| {
                ui.label("Width:");
                ui.add(egui::DragValue::new(&mut c.w).range(0..=w * 4));
                ui.label("Height:");
                ui.add(egui::DragValue::new(&mut c.h).range(0..=h * 4));
            });
            Self::angle_row(ui, &mut c.angle);
        });
        group(ui, "Method", |ui| {
            egui::ComboBox::from_id_salt("cm").selected_text(format!("{:?}", c.method)).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.method, CensorMethod::Pixelate, "Pixelate");
                ui.selectable_value(&mut c.method, CensorMethod::Blur, "Blur");
                ui.selectable_value(&mut c.method, CensorMethod::Solid, "Solid colour");
            });
            match c.method {
                CensorMethod::Pixelate => {
                    ui.horizontal(|ui| {
                        ui.label("Square size:");
                        ui.add(egui::DragValue::new(&mut c.square).range(2..=1000).suffix(" px"));
                    });
                }
                CensorMethod::Blur => {
                    ui.add(soft_slider(&mut c.sigma, 1.0..=40.0, "blur radius"));
                }
                CensorMethod::Solid => {
                    ui.horizontal(|ui| {
                        ui.label("Colour:");
                        ui.color_edit_button_srgb(&mut c.color);
                    });
                }
            }
        });
        group(ui, "Source frames", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.from).range(1..=n));
                ui.label("–");
                let mut to = if c.to == 0 { n } else { c.to };
                if ui.add(egui::DragValue::new(&mut to).range(1..=n)).changed() {
                    c.to = if to >= n { 0 } else { to };
                }
            });
        });
    }

    fn ui_add_image(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        hint(ui, "Image overlays are live and stick to the content. PNG/GIF/JPG/WebP – animated overlays loop. Drag to move, corners to resize (Shift = free aspect), round handle to rotate.");
        let sel = self.sel_index(Kind::Overlay);
        ui.horizontal(|ui| {
            if ui.button("+ Add image…").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "gif", "jpg", "jpeg", "webp", "bmp"]).pick_file() {
                    self.load_overlay(path, ctx);
                }
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("✕ Remove")).clicked() {
                self.stack.layers.remove(sel.unwrap());
            }
        });
        let items: Vec<(usize, u64, String)> = self
            .stack
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, l)| match &l.kind {
                LayerKind::Overlay(o) => Some((i, l.id, o.src.name.clone())),
                _ => None,
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let sel = self.sel_index(Kind::Overlay);
        ui.horizontal_wrapped(|ui| {
            for (i, id, name) in &items {
                if ui.selectable_label(Some(*i) == sel, name).clicked() {
                    self.select(Kind::Overlay, *id);
                }
            }
        });
        let Some(sel) = self.sel_index(Kind::Overlay) else { return };
        let (aw, ah) = self.dims();
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label("Edits affect:");
            egui::ComboBox::from_id_salt("pscope").width(190.0).selected_text(self.place_scope.label()).show_ui(ui, |ui| {
                for s in [PlaceScope::All, PlaceScope::Following, PlaceScope::This] {
                    ui.selectable_value(&mut self.place_scope, s, s.label());
                }
            });
        });
        let scope = self.place_scope;
        let layer = &mut self.stack.layers[sel];
        let visible = &mut layer.visible;
        let LayerKind::Overlay(o) = &mut layer.kind else { return };
        let native = o.src.native_size();
        let nframes = o.src.frames.len();
        group(ui, "Layer", |ui| {
            hint(ui, &format!("{}×{} px, {} frame{}", native.0, native.1, nframes, if nframes == 1 { "" } else { "s" }));
            ui.checkbox(visible, "Visible");
            ui.add(egui::Slider::new(&mut o.opacity, 0.0..=100.0).text("opacity %"));
        });
        if o.placements.is_empty() {
            return;
        }
        let cur = src_idx.min(o.placements.len() - 1);
        group(ui, &format!("Placement on source frame #{}", cur + 1), |ui| {
            let mut pl = o.placements[cur];
            let mut dont = !pl.enabled;
            ui.checkbox(&mut dont, "Don't add image to this frame");
            pl.enabled = !dont;
            ui.horizontal(|ui| {
                ui.label("Left:");
                ui.add(egui::DragValue::new(&mut pl.x));
                ui.label("Top:");
                ui.add(egui::DragValue::new(&mut pl.y));
            });
            ui.horizontal(|ui| {
                ui.label("Width:");
                ui.add(egui::DragValue::new(&mut pl.w).range(1..=16384));
                ui.label("Height:");
                ui.add(egui::DragValue::new(&mut pl.h).range(1..=16384));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Native size").clicked() {
                    pl.w = native.0;
                    pl.h = native.1;
                }
                if ui.small_button("Center").clicked() {
                    pl.x = (aw as i32 - pl.w as i32) / 2;
                    pl.y = (ah as i32 - pl.h as i32) / 2;
                }
                if ui.small_button("Fill frame").clicked() {
                    pl.x = 0;
                    pl.y = 0;
                    pl.w = aw;
                    pl.h = ah;
                }
            });
            Self::angle_row(ui, &mut pl.angle);
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
            ui.horizontal(|ui| {
                if ui.button("Copy to all frames").clicked() {
                    let pl = o.placements[cur];
                    for q in &mut o.placements {
                        *q = pl;
                    }
                }
                if ui.button("Enable on all").clicked() {
                    for q in &mut o.placements {
                        q.enabled = true;
                    }
                }
            });
        });
        hint(ui, "Positions are in source-frame pixels, so the image follows crops, resizes and rotations.");
    }

    fn ui_split(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        hint(ui, "Exports include all live edits.");
        group(ui, "Individual frames", |ui| {
            ui.horizontal(|ui| {
                ui.label("Output format:");
                egui::ComboBox::from_id_salt("splitfmt")
                    .selected_text(match self.split_fmt {
                        SplitFormat::Png => "PNG (full detail, alpha)",
                        SplitFormat::Gif => "GIF, full details in every frame",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.split_fmt, SplitFormat::Png, "PNG (full detail, alpha)");
                        ui.selectable_value(&mut self.split_fmt, SplitFormat::Gif, "GIF, full details in every frame");
                    });
            });
            ui.horizontal(|ui| {
                ui.label("File prefix:");
                ui.text_edit_singleline(&mut self.split_prefix);
            });
            ui.horizontal(|ui| {
                if primary(ui, "Split to folder…") {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let (pre, fmt) = (self.split_prefix.clone(), self.split_fmt);
                        self.run_export(ctx, "Split frames", move |a, _| io::export_frames_dir(a, &dir, &pre, fmt).map(|n| format!("Wrote {n} frames to {}", dir.display())));
                    }
                }
                if ui.button("ZIP of PNGs…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).set_file_name(format!("{}_frames.zip", self.base_name())).save_file() {
                        let pre = self.split_prefix.clone();
                        self.run_export(ctx, "ZIP frames", move |a, _| io::export_frames_zip(a, &path, &pre).map(|n| format!("Wrote {n} frames to {}", path.display())));
                    }
                }
            });
        });
        self.sheet_group(ui, ctx);
    }

    fn sheet_group(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let n = self.plan.iter().filter(|f| f.enabled).count().max(1) as u32;
        let (fw, fh) = (self.out_size.0 as u32, self.out_size.1 as u32);
        let base_name = self.base_name();
        let mut export = false;
        group(ui, "Sprite sheet / game engine", |ui| {
            let s = &mut self.sheet;
            ui.horizontal(|ui| {
                ui.label("Columns:");
                ui.add(egui::DragValue::new(&mut s.columns).range(0..=n));
                hint(ui, "(0 = auto)");
                ui.label("Padding:");
                ui.add(egui::DragValue::new(&mut s.padding).range(0..=256).suffix(" px"));
            });
            let cols = if s.columns == 0 { (n as f32).sqrt().ceil() as u32 } else { s.columns.min(n) }.max(1);
            let rows = n.div_ceil(cols);
            hint(ui, &format!("Sheet: {cols}×{rows} cells, {}×{} px", cols * fw + (cols - 1) * s.padding, rows * fh + (rows - 1) * s.padding));
            ui.checkbox(&mut s.write_json, "Write JSON metadata (frame rects + durations)");
            ui.checkbox(&mut s.write_godot, "Write Godot 4 SpriteFrames (.tres)");
            if s.write_godot {
                ui.indent("godot", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("res:// folder:");
                        ui.text_edit_singleline(&mut s.godot_res_dir);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Animation name:");
                        ui.text_edit_singleline(&mut s.anim_name);
                    });
                    hint(ui, "Put the PNG and .tres in that folder and assign the .tres to an AnimatedSprite2D/3D. Per-frame delays are preserved.");
                });
            }
            export = primary(ui, "Export sprite sheet…");
        });
        if export {
            if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("{base_name}_sheet.png")).save_file() {
                let opts = self.sheet.clone();
                self.run_export(ctx, "Sprite sheet", move |a, _| io::export_spritesheet(a, &path, &opts).map(|s| format!("Wrote {s}")));
            }
        }
    }

    fn ui_frames(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let n = self.base.as_ref().map(|a| a.len()).unwrap_or(1);
        let now = ctx.input(|i| i.time);
        hint(ui, "Frame edits change the source frames directly (undoable); all live edits are re-applied on top instantly.");
        if ui.button("+ Add frames from files…").clicked() {
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
        group(ui, "Toggle a range of frames", |ui| {
            ui.horizontal(|ui| {
                ui.label("From:");
                ui.add(egui::DragValue::new(&mut self.fr_from).range(1..=n));
                ui.label("To:");
                ui.add(egui::DragValue::new(&mut self.fr_to).range(1..=n));
            });
            ui.horizontal(|ui| {
                if tinted(ui, "Skip range", pal.negative) {
                    range_act = Some((false, false));
                }
                if tinted(ui, "Enable range", pal.highlight) {
                    range_act = Some((false, true));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Every:");
                ui.add(egui::DragValue::new(&mut self.fr_every).range(2..=1000));
                if tinted(ui, "Skip every N-th", pal.negative) {
                    range_act = Some((true, false));
                }
                if tinted(ui, "Enable every N-th", pal.highlight) {
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
        group(ui, "GIF options", |ui| {
            ui.horizontal(|ui| {
                ui.label("Delay time:");
                ui.add(egui::DragValue::new(&mut self.fr_delay).range(1..=65535).suffix(" cs"));
                ui.label(RichText::new(format!("({:.2} FPS)", 100.0 / self.fr_delay.max(1) as f32)).strong());
                if ui.button("Set for all").clicked() {
                    let d = self.fr_delay;
                    self.run_anim(ctx, "Set delays", move |a, _| Ok(ops::set_all_delays(a, d)));
                }
            });
            let mut forever = self.base.as_ref().map(|a| a.loop_count.is_none()).unwrap_or(true);
            let mut count = self.base.as_ref().and_then(|a| a.loop_count).unwrap_or(1);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Loop count:");
                changed |= ui.checkbox(&mut forever, "forever").changed();
                if !forever {
                    changed |= ui.add(egui::DragValue::new(&mut count).range(1..=65535).suffix(" plays")).changed();
                }
            });
            if changed {
                self.snapshot("loop", now);
                self.base.as_mut().unwrap().loop_count = if forever { None } else { Some(count.max(1)) };
            }
            ui.checkbox(&mut self.gif_opts.global_palette, "Use global colormap");
            hint(ui, "Use the same set of colours for all frames to reduce file size.");
        });
        group(ui, "Effects", |ui| {
            ui.horizontal(|ui| {
                ui.label("Crossfade:");
                ui.add(egui::DragValue::new(&mut self.fr_crossfade).range(1..=30));
                ui.label("in-between frames");
            });
            ui.checkbox(&mut self.fr_crossfade_wrap, "Also blend last → first (seamless loop)");
            if ui.button("Crossfade frames").clicked() {
                let (s, w) = (self.fr_crossfade, self.fr_crossfade_wrap);
                self.run_anim(ctx, "Crossfade", move |a, _| Ok(ops::crossfade(a, s, w)));
            }
            ui.separator();
            if ui.button("Use the first frame as background").on_hover_text("Composite every frame over frame 1").clicked() {
                self.run_anim(ctx, "First frame as background", move |a, _| Ok(ops::first_frame_as_background(a)));
            }
            hint(ui, "Transparent animations are exported with \"don't stack frames\" disposal automatically.");
        });
        let skipped = self.base.as_ref().map(|a| a.len() - a.enabled_count()).unwrap_or(0);
        if skipped > 0 && tinted(ui, &format!("Remove {skipped} skipped frame(s) permanently"), pal.negative) {
            self.run_anim(ctx, "Remove skipped", move |a, _| {
                let mut a = a.clone();
                a.frames.retain(|f| f.enabled);
                Ok(a)
            });
        }
    }

    fn ui_export(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        hint(ui, "Everything is exported with all live edits applied.");
        let summary = {
            let o = &self.gif_opts;
            format!("{} colours{}, lossy {}, {}", o.colors, if o.global_palette { " (global)" } else { "" }, o.lossy, if o.frame_diff { "frame differencing on" } else { "full frames" })
        };
        let mut save_gif = false;
        let mut goto_opt = false;
        group(ui, "GIF", |ui| {
            hint(ui, &summary);
            ui.horizontal(|ui| {
                save_gif = primary(ui, "Save GIF…");
                goto_opt = ui.button("Optimize settings…").clicked();
            });
        });
        if save_gif {
            self.save_gif_dialog(ctx);
        }
        if goto_opt {
            self.tool = Tool::Optimize;
        }
        let mut save_apng = false;
        group(ui, "Animated PNG (APNG)", |ui| {
            hint(ui, "Full 24-bit colour with 8-bit alpha. Supported by all modern browsers.");
            save_apng = ui.button("Save APNG…").clicked();
        });
        if save_apng {
            if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png", "apng"]).set_file_name(format!("{}.png", self.base_name())).save_file() {
                self.run_export(ctx, "Save APNG", move |a, _| io::save_apng(a, &path).map(|n| format!("Saved {} ({})", path.display(), fmt_size(n))));
            }
        }
        let mut video: Option<(VideoFormat, &'static str)> = None;
        let ffmpeg = self.ffmpeg;
        let mut recheck = false;
        group(ui, "Video", |ui| {
            if !ffmpeg {
                ui.label(RichText::new("ffmpeg was not found on PATH – install it to enable MP4/WebM export.").color(pal.neutral));
                recheck = ui.small_button("Check again").clicked();
                return;
            }
            ui.horizontal(|ui| {
                ui.label("MP4 background (for transparency):");
                ui.color_edit_button_srgb(&mut self.video_bg);
            });
            ui.horizontal(|ui| {
                if ui.button("To MP4 (H.264)…").clicked() {
                    video = Some((VideoFormat::Mp4, "mp4"));
                }
                if ui.button("To WebM (VP9 + alpha)…").clicked() {
                    video = Some((VideoFormat::WebmAlpha, "webm"));
                }
            });
        });
        if recheck {
            self.ffmpeg = io::ffmpeg_available();
        }
        if let Some((fmt, ext)) = video {
            if let Some(path) = rfd::FileDialog::new().add_filter(ext, &[ext]).set_file_name(format!("{}.{ext}", self.base_name())).save_file() {
                let bg = self.video_bg;
                self.run_export(ctx, "Export video", move |a, _| io::export_video(a, &path, fmt, bg).map(|_| format!("Saved {}", path.display())));
            }
        }
        let mut goto_split = false;
        group(ui, "Frames", |ui| {
            goto_split = ui.button("PNG frames / ZIP…").clicked();
        });
        if goto_split {
            self.tool = Tool::Split;
        }
        self.sheet_group(ui, ctx);
    }
}

// ================================================================ effects panel

fn effects_ui(ui: &mut egui::Ui, fx: &mut EffectSettings, tr: &mut Transform) {
    group(ui, "Colorize", |ui| {
        ui.add(soft_slider(&mut fx.hue, 0.0..=200.0, "Hue"));
        ui.add(soft_slider(&mut fx.saturation, 0.0..=200.0, "Saturation"));
        ui.add(soft_slider(&mut fx.lightness, 0.0..=200.0, "Lightness"));
        ui.add(soft_slider(&mut fx.brightness, -100.0..=100.0, "Brightness"));
        ui.add(soft_slider(&mut fx.contrast, -100.0..=100.0, "Contrast"));
        if ui.small_button("Reset colour").clicked() {
            fx.hue = 100.0;
            fx.saturation = 100.0;
            fx.lightness = 100.0;
            fx.brightness = 0.0;
            fx.contrast = 0.0;
        }
    });
    group(ui, "Color presets", |ui| {
        ui.checkbox(&mut fx.grayscale, "Grayscale");
        ui.checkbox(&mut fx.sepia, "Sepia");
        ui.checkbox(&mut fx.monochrome, "Monochrome");
        ui.checkbox(&mut fx.negative, "Negative (invert colors)");
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.tint, "Tint");
            if fx.tint {
                ui.color_edit_button_srgb(&mut fx.tint_color);
                ui.add(egui::DragValue::new(&mut fx.tint_amount).range(0.0..=100.0).suffix("%"));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.change_bg, "Change background color");
            if fx.change_bg {
                ui.color_edit_button_srgb(&mut fx.bg_color);
            }
        });
    });
    group(ui, "Transformations", |ui| {
        ui.checkbox(&mut fx.flip_v, "↕ Flip vertically");
        ui.checkbox(&mut fx.flip_h, "↔ Flip horizontally");
        ui.checkbox(&mut fx.rot90, "↻ Rotate 90°");
        ui.checkbox(&mut fx.rot270, "↺ Rotate 270°");
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.rot_any, "Rotate by");
            ui.add(egui::DragValue::new(&mut fx.rot_deg).speed(0.5).suffix("°"));
        });
    });
    group(ui, "Animation flow", |ui| {
        ui.checkbox(&mut tr.reverse, "⟲ Reverse");
        ui.checkbox(&mut tr.set_loop, "Change the number of times to play (loop)");
        if tr.set_loop {
            ui.indent("fxloop", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut tr.loop_count).range(0..=65535));
                    hint(ui, "(0 = forever)");
                });
            });
        }
        ui.checkbox(&mut tr.boomerang, "⇄ Run to the end and reverse back to the start");
        ui.checkbox(&mut fx.counter, "◷ Add seconds counter");
        ui.checkbox(&mut fx.fade_in, "Fade in");
        ui.checkbox(&mut fx.fade_out, "Fade out");
        if fx.fade_in || fx.fade_out {
            ui.indent("fade", |ui| {
                ui.horizontal(|ui| {
                    ui.label("over");
                    ui.add(egui::DragValue::new(&mut fx.fade_frames).range(1..=10000));
                    ui.label("frames");
                });
                ui.checkbox(&mut fx.fade_to_transparent, "Fade to transparency (instead of black)");
            });
        }
        hint(ui, "Reverse, boomerang and loop apply to the whole animation (shared with the Reverse tool).");
    });
    group(ui, "Replace color with transparency", |ui| {
        ui.checkbox(&mut fx.rc_white, "□ White");
        ui.checkbox(&mut fx.rc_black, "■ Black");
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.rc_custom, "Custom");
            let mut hex = format!("#{:02x}{:02x}{:02x}", fx.rc_color[0], fx.rc_color[1], fx.rc_color[2]);
            if ui.add(egui::TextEdit::singleline(&mut hex).desired_width(70.0)).changed() {
                if let Some(c) = parse_hex(&hex) {
                    fx.rc_color = c;
                }
            }
            ui.color_edit_button_srgb(&mut fx.rc_color);
        });
        if fx.rc_white || fx.rc_black || fx.rc_custom {
            ui.add(soft_slider(&mut fx.rc_fuzz, 0.0..=60.0, "Fuzz %"));
            hint(ui, "How similar colours can be to count as equal – useful when the background has subtle variations.");
            ui.horizontal(|ui| {
                ui.checkbox(&mut fx.rc_clean_edges, "Clean up edges");
                if fx.rc_clean_edges {
                    ui.add(egui::DragValue::new(&mut fx.rc_edge_px).range(1..=50).suffix(" px"));
                }
            });
        }
    });
    group(ui, "Blurring and sharpening", |ui| {
        ui.checkbox(&mut fx.blur, "Gaussian blur");
        if fx.blur {
            ui.add(soft_slider(&mut fx.blur_sigma, 0.3..=20.0, "σ"));
        }
        ui.checkbox(&mut fx.sharpen, "Sharpen");
        if fx.sharpen {
            ui.add(soft_slider(&mut fx.sharpen_sigma, 0.3..=10.0, "σ"));
        }
    });
    group(ui, "Dithering and patterns", |ui| {
        ui.checkbox(&mut fx.ordered_dither, "Ordered dithering (Bayer)");
        ui.checkbox(&mut fx.error_dither, "Error diffusion (Floyd–Steinberg)");
        if fx.ordered_dither || fx.error_dither {
            ui.add(egui::Slider::new(&mut fx.dither_levels, 2..=16).text("levels / channel").clamping(egui::SliderClamping::Never));
        }
    });
    group(ui, "Artistic", |ui| {
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.oil, "Oil paint");
            if fx.oil {
                ui.add(egui::DragValue::new(&mut fx.oil_radius).range(1..=32).prefix("r "));
            }
        });
        ui.checkbox(&mut fx.emboss, "Emboss");
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.posterize, "Posterize");
            if fx.posterize {
                ui.add(egui::DragValue::new(&mut fx.posterize_levels).range(2..=255).suffix(" levels"));
            }
        });
        ui.checkbox(&mut fx.solarize, "Solarize");
        ui.checkbox(&mut fx.noise, "Add noise");
        if fx.noise {
            ui.add(soft_slider(&mut fx.noise_amount, 1.0..=128.0, "amount"));
        }
    });
    group(ui, "Distortions", |ui| {
        ui.checkbox(&mut fx.swirl, "Swirl");
        if fx.swirl {
            ui.add(soft_slider(&mut fx.swirl_deg, -720.0..=720.0, "°"));
        }
        ui.checkbox(&mut fx.implode, "Implode");
        if fx.implode {
            ui.add(soft_slider(&mut fx.implode_amount, -2.0..=2.0, "amount"));
        }
        ui.checkbox(&mut fx.wave, "Wave");
        if fx.wave {
            ui.add(soft_slider(&mut fx.wave_amp, 1.0..=100.0, "amplitude"));
            ui.add(soft_slider(&mut fx.wave_len, 4.0..=500.0, "wavelength"));
        }
    });
    group(ui, "Filters", |ui| {
        ui.checkbox(&mut fx.gotham, "Gotham");
        ui.checkbox(&mut fx.lomo, "Lomo");
        ui.checkbox(&mut fx.nashville, "Nashville");
        ui.checkbox(&mut fx.toaster, "Toaster");
        ui.checkbox(&mut fx.vignette, "Vignette");
        ui.checkbox(&mut fx.polaroid, "Polaroid");
    });
    group(ui, "Frames", |ui| {
        ui.checkbox(&mut fx.frame_camera, "Camera");
        ui.checkbox(&mut fx.frame_fuzzy, "Fuzzy");
        ui.checkbox(&mut fx.frame_rounded, "Rounded corners");
        if fx.frame_rounded {
            ui.add(soft_slider(&mut fx.rounded_radius, 1.0..=200.0, "radius px"));
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut fx.frame_border, "Solid border");
            if fx.frame_border {
                ui.add(egui::DragValue::new(&mut fx.border_px).range(1..=2000).suffix(" px"));
                ui.color_edit_button_srgb(&mut fx.border_color);
            }
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

fn draw_sel(painter: &egui::Painter, r: Rect, accent: Color32) {
    painter.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_black_alpha(160)), egui::StrokeKind::Middle);
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
        let h = Rect::from_center_size(c, vec2(8.0, 8.0));
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
        if m.to_screen(c).distance(screen) <= 9.0 {
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

fn clamp_pt(pt: Pos2, b: (f32, f32)) -> Pos2 {
    pos2(pt.x.clamp(0.0, b.0), pt.y.clamp(0.0, b.1))
}

/// Rectangle tool: draw new (if `allow_new`), move, or resize from corners.
/// When `allow_new` is false the rect may extend outside the bounds (overlays).
#[allow(clippy::too_many_arguments)]
fn rect_tool(drag: &mut Option<DragMode>, started: bool, dragged: bool, stopped: bool, resp: &egui::Response, m: &Mapper, r: &mut [f32; 4], bounds: (f32, f32), aspect: Option<f32>, allow_new: bool) {
    let clampb = |pt: Pos2| if allow_new { clamp_pt(pt, bounds) } else { pt };
    if started {
        if let Some(sp) = resp.interact_pointer_pos() {
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
                    let b = clampb(apply_aspect(a, clampb(ip), aspect));
                    let mn = pos2(a.x.min(b.x).round(), a.y.min(b.y).round());
                    let mx = pos2(a.x.max(b.x).round(), a.y.max(b.y).round());
                    *r = [mn.x, mn.y, (mx.x - mn.x).max(1.0), (mx.y - mn.y).max(1.0)];
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
