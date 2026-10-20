//! Non-destructive live edit stack.
//!
//! The base animation holds the pixels. Everything else is applied live when
//! a frame is shown or exported:
//!
//!   base frames
//!     → transform: cut range, speed, reverse/boomerang (the frame *plan*),
//!                  crop → resize → flip/rotate (geometry)
//!     → ordered layers, bottom to top: effects / censor / image / text
//!
//! Layer coordinates and frame ranges refer to the output (what you see).

use crate::anim::{effective_delay, Animation, Frame};
use crate::effects::{self, CensorSettings, EffectSettings, ShieldSettings, WatermarkSettings, WatermarkSprite};
use crate::geo::{map_box, GeoMap, Obb};
use crate::ops::{self, AspectMode, ResizeFilter};
use crate::overlay::{OverlaySource, Placement};
use crate::shapes::{self, EmojiSettings, MarkupSettings, ShapeKind, ShapeSettings, StrokeSprite};
use crate::text::{self, Annotation, FontEntry};
use image::{imageops, Rgba, RgbaImage};
use rayon::prelude::*;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------- transform

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SpeedMode {
    Percent,
    DelayAll,
    Fps,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Transform {
    // timing
    pub cut_on: bool,
    pub cut_by_time: bool,
    /// 1-based inclusive base frame range; end 0 = last
    pub cut_start: usize,
    pub cut_end: usize,
    pub cut_t0: f32,
    pub cut_t1: f32,
    pub speed_on: bool,
    pub speed_mode: SpeedMode,
    pub speed_val: f32,
    pub reverse: bool,
    pub boomerang: bool,
    /// master switch for reverse / boomerang / loop
    pub play_on: bool,
    pub set_loop: bool,
    pub loop_count: u16,
    // geometry
    pub crop_on: bool,
    pub crop: [u32; 4],
    pub resize_on: bool,
    pub rs_w: u32,
    pub rs_h: u32,
    pub rs_pct: f32,
    pub rs_mode: AspectMode,
    pub rs_filter: ResizeFilter,
    /// master switch for flips / rotation
    pub rot_on: bool,
    pub flip_v: bool,
    pub flip_h: bool,
    /// 0, 90, 180 or 270 (clockwise)
    pub rot: u16,
    pub rot_any: bool,
    pub rot_deg: f32,
    pub rot_expand: bool,
    pub rot_bg_transparent: bool,
    pub rot_bg: [u8; 3],
    // extra canvas around the frames, after everything else
    pub pad_on: bool,
    /// pixels added at the top, right, bottom and left
    pub pad: [u32; 4],
    pub pad_transparent: bool,
    pub pad_color: [u8; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            cut_on: false,
            cut_by_time: false,
            cut_start: 1,
            cut_end: 0,
            cut_t0: 0.0,
            cut_t1: 1.0,
            speed_on: false,
            speed_mode: SpeedMode::Percent,
            speed_val: 100.0,
            reverse: false,
            boomerang: false,
            set_loop: false,
            play_on: true,
            loop_count: 0,
            crop_on: false,
            crop: [0, 0, 0, 0],
            resize_on: false,
            rs_w: 0,
            rs_h: 0,
            rs_pct: 0.0,
            rs_mode: AspectMode::CenterCrop,
            rs_filter: ResizeFilter::Lanczos3,
            rot_on: false,
            flip_v: false,
            flip_h: false,
            rot: 0,
            rot_any: false,
            rot_deg: 45.0,
            rot_expand: true,
            rot_bg_transparent: true,
            rot_bg: [255, 255, 255],
            pad_on: false,
            pad: [0; 4],
            pad_transparent: false,
            pad_color: [255, 255, 255],
        }
    }
}

pub fn resize_target(w: u32, h: u32, tw: u32, th: u32, pct: f32) -> Option<(u32, u32)> {
    if pct > 0.0 {
        return Some((((w as f32 * pct / 100.0).round() as u32).max(1), ((h as f32 * pct / 100.0).round() as u32).max(1)));
    }
    match (tw, th) {
        (0, 0) => None,
        (tw, 0) => Some((tw, ((h as f32 * tw as f32 / w as f32).round() as u32).max(1))),
        (0, th) => Some((((w as f32 * th as f32 / h as f32).round() as u32).max(1), th)),
        (tw, th) => Some((tw, th)),
    }
}

impl Transform {
    pub fn geometry_identity(&self) -> bool {
        !(self.crop_on && self.crop[2] > 0 && self.crop[3] > 0) && !self.resize_on && !self.rotates() && !self.pads()
    }

    /// Adds canvas around the frames.
    pub fn pads(&self) -> bool {
        self.pad_on && self.pad.iter().any(|p| *p > 0)
    }

    /// Put the frame on a bigger canvas (transparent or a colour).
    fn apply_pad(&self, img: RgbaImage) -> RgbaImage {
        if !self.pads() {
            return img;
        }
        let [t, r, b, l] = self.pad;
        let bg = if self.pad_transparent { Rgba([0, 0, 0, 0]) } else { Rgba([self.pad_color[0], self.pad_color[1], self.pad_color[2], 255]) };
        let (w, h) = img.dimensions();
        let mut out = RgbaImage::from_pixel(w + l + r, h + t + b, bg);
        imageops::replace(&mut out, &img, l as i64, t as i64);
        out
    }

    pub fn is_identity(&self) -> bool {
        self.geometry_identity() && !self.cut_on && !(self.speed_on && !(self.speed_mode == SpeedMode::Percent && self.speed_val == 100.0)) && !(self.play_on && (self.reverse || self.boomerang || self.set_loop))
    }

    pub fn rotates(&self) -> bool {
        self.rot_on && (self.flip_v || self.flip_h || self.rot % 360 != 0 || (self.rot_any && self.rot_deg % 360.0 != 0.0))
    }

    /// Crop → resize → flip / rotate.
    pub fn apply_geometry(&self, src: &RgbaImage) -> RgbaImage {
        let mut img: Option<RgbaImage> = None;
        let cur = |i: &Option<RgbaImage>| -> (u32, u32) { i.as_ref().map(|x| x.dimensions()).unwrap_or(src.dimensions()) };
        if self.crop_on && self.crop[2] > 0 && self.crop[3] > 0 {
            let (w, h) = src.dimensions();
            let x = self.crop[0].min(w.saturating_sub(1));
            let y = self.crop[1].min(h.saturating_sub(1));
            let cw = self.crop[2].clamp(1, w - x);
            let ch = self.crop[3].clamp(1, h - y);
            img = Some(imageops::crop_imm(src, x, y, cw, ch).to_image());
        }
        if self.resize_on {
            let (w, h) = cur(&img);
            if let Some((tw, th)) = resize_target(w, h, self.rs_w, self.rs_h, self.rs_pct) {
                if (tw, th) != (w, h) || self.rs_mode == AspectMode::FitPad {
                    let s = img.as_ref().unwrap_or(src);
                    img = Some(ops::resize_image(s, tw, th, self.rs_mode, self.rs_filter));
                }
            }
        }
        let mut out = img.unwrap_or_else(|| src.clone());
        if !self.rot_on {
            return self.apply_pad(out);
        }
        if self.flip_v {
            out = imageops::flip_vertical(&out);
        }
        if self.flip_h {
            out = imageops::flip_horizontal(&out);
        }
        match self.rot % 360 {
            90 => out = imageops::rotate90(&out),
            180 => out = imageops::rotate180(&out),
            270 => out = imageops::rotate270(&out),
            _ => {}
        }
        if self.rot_any && self.rot_deg % 360.0 != 0.0 {
            let bg = if self.rot_bg_transparent { Rgba([0, 0, 0, 0]) } else { Rgba([self.rot_bg[0], self.rot_bg[1], self.rot_bg[2], 255]) };
            out = ops::rotate_any_image(&out, self.rot_deg, bg, self.rot_expand);
        }
        self.apply_pad(out)
    }

    /// Where `apply_geometry` moves source pixels (must mirror it exactly).
    pub fn geo_map(&self, w: u32, h: u32) -> GeoMap {
        let mut g = GeoMap::new(w as f32, h as f32);
        let (mut cw, mut ch) = (w, h);
        if self.crop_on && self.crop[2] > 0 && self.crop[3] > 0 {
            let x = self.crop[0].min(w.saturating_sub(1));
            let y = self.crop[1].min(h.saturating_sub(1));
            cw = self.crop[2].clamp(1, w - x);
            ch = self.crop[3].clamp(1, h - y);
            g.crop(x as f32, y as f32, cw as f32, ch as f32);
        }
        if self.resize_on {
            if let Some((tw, th)) = resize_target(cw, ch, self.rs_w, self.rs_h, self.rs_pct) {
                if (tw, th) != (cw, ch) || self.rs_mode == AspectMode::FitPad {
                    let (fw, fh) = (cw as f32, ch as f32);
                    match self.rs_mode {
                        AspectMode::Stretch => g.scale_to(tw as f32 / fw, th as f32 / fh, tw as f32, th as f32),
                        AspectMode::CenterCrop => {
                            let s = (tw as f32 / fw).max(th as f32 / fh);
                            let nw = ((fw * s).round() as u32).max(tw);
                            let nh = ((fh * s).round() as u32).max(th);
                            g.scale_to(nw as f32 / fw, nh as f32 / fh, nw as f32, nh as f32);
                            g.crop(((nw - tw) / 2) as f32, ((nh - th) / 2) as f32, tw as f32, th as f32);
                        }
                        AspectMode::FitPad => {
                            let s = (tw as f32 / fw).min(th as f32 / fh);
                            let nw = ((fw * s).round() as u32).clamp(1, tw);
                            let nh = ((fh * s).round() as u32).clamp(1, th);
                            g.scale_to(nw as f32 / fw, nh as f32 / fh, nw as f32, nh as f32);
                            g.offset(((tw - nw) / 2) as f32, ((th - nh) / 2) as f32, tw as f32, th as f32);
                        }
                    }
                }
            }
        }
        if self.rot_on {
            if self.flip_v {
                g.flip_v();
            }
            if self.flip_h {
                g.flip_h();
            }
            match self.rot % 360 {
                90 => g.rot90(),
                180 => g.rot180(),
                270 => g.rot270(),
                _ => {}
            }
            if self.rot_any && self.rot_deg % 360.0 != 0.0 {
                g.rot_any(self.rot_deg, self.rot_expand);
            }
        }
        if self.pads() {
            let [t, r, b, l] = self.pad;
            let (w, h) = (g.w, g.h);
            g.offset(l as f32, t as f32, w + (l + r) as f32, h + (t + b) as f32);
        }
        g
    }

    /// The new delay in hundredths of a second, before rounding. Delays under
    /// 2 cs are measured as players show them (10 cs).
    fn scale_delay_exact(&self, d: u16) -> f64 {
        let v = self.speed_val.max(0.01) as f64;
        match self.speed_mode {
            SpeedMode::Percent => crate::anim::effective_delay(d) as f64 * 100.0 / v,
            SpeedMode::DelayAll => v,
            SpeedMode::Fps => 100.0 / v,
        }
    }

    fn scale_delay(&self, d: u16) -> u16 {
        if !self.speed_on {
            return d;
        }
        let v = self.speed_val.max(0.01);
        match self.speed_mode {
            SpeedMode::Percent => ((d as f32 * 100.0 / v).round() as u16).max(1),
            SpeedMode::DelayAll => (v.round() as u16).max(1),
            SpeedMode::Fps => ((100.0 / v).round() as u16).max(1),
        }
    }

    /// Base frame range kept by the cut (0-based inclusive).
    pub fn cut_range(&self, base: &Animation) -> (usize, usize) {
        let n = base.len();
        if n == 0 {
            return (0, 0);
        }
        if !self.cut_on {
            return (0, n - 1);
        }
        if self.cut_by_time {
            return ops::time_to_frame_range(base, self.cut_t0, self.cut_t1);
        }
        let s = self.cut_start.clamp(1, n) - 1;
        let e = if self.cut_end == 0 { n - 1 } else { self.cut_end.clamp(1, n) - 1 };
        (s.min(e), e.max(s))
    }
}

// ---------------------------------------------------------------- layers

#[derive(Clone)]
pub struct OverlayLayer {
    pub src: Arc<OverlaySource>,
    /// Placement per output frame.
    pub placements: Vec<Placement>,
    pub opacity: f32,
}

impl OverlayLayer {
    pub fn placement(&self, i: usize) -> Placement {
        self.placements.get(i).or(self.placements.last()).copied().unwrap_or(Placement { enabled: false, x: 0, y: 0, w: 0, h: 0, angle: 0.0 })
    }
}

#[derive(Clone)]
pub enum LayerKind {
    Effects(Box<EffectSettings>),
    Text(Annotation),
    Censor(CensorSettings),
    Overlay(OverlayLayer),
    Watermark(Box<WatermarkSettings>),
    Shield(ShieldSettings),
    Shape(Box<ShapeSettings>),
    Markup(MarkupSettings),
    Emoji(Box<EmojiSettings>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Effects,
    Text,
    Censor,
    Overlay,
    Watermark,
    Shield,
    Shape,
    Markup,
    Emoji,
}

#[derive(Clone)]
pub struct Layer {
    pub id: u64,
    pub visible: bool,
    pub kind: LayerKind,
    /// user-chosen name (None = automatic)
    pub label: Option<String>,
    /// clips and keyframes from the timeline
    pub timing: crate::timeline::Timing,
}

impl Layer {
    pub fn kind(&self) -> Kind {
        match self.kind {
            LayerKind::Effects(_) => Kind::Effects,
            LayerKind::Text(_) => Kind::Text,
            LayerKind::Censor(_) => Kind::Censor,
            LayerKind::Overlay(_) => Kind::Overlay,
            LayerKind::Watermark(_) => Kind::Watermark,
            LayerKind::Shield(_) => Kind::Shield,
            LayerKind::Shape(_) => Kind::Shape,
            LayerKind::Markup(_) => Kind::Markup,
            LayerKind::Emoji(_) => Kind::Emoji,
        }
    }
    pub fn active(&self) -> bool {
        self.visible
            && match &self.kind {
                LayerKind::Effects(fx) => **fx != EffectSettings::default(),
                LayerKind::Text(t) => !t.text.is_empty(),
                LayerKind::Censor(c) => c.w > 0 && c.h > 0,
                LayerKind::Overlay(_) => true,
                LayerKind::Watermark(w) => w.active(),
                LayerKind::Shield(s) => s.strength > 0.0 && (s.pattern || s.noise || s.jitter),
                LayerKind::Shape(s) => s.w > 0 && s.h > 0,
                LayerKind::Markup(m) => !m.strokes.is_empty(),
                LayerKind::Emoji(e) => e.w > 0 && e.h > 0,
            }
    }
    /// The layer's oriented box in source-frame space (for source frame `src`).
    pub fn source_box(&self, src: usize, fonts: &[FontEntry]) -> Option<Obb> {
        match &self.kind {
            LayerKind::Effects(_) | LayerKind::Watermark(_) | LayerKind::Shield(_) | LayerKind::Markup(_) => None,
            LayerKind::Shape(s) => Some(Obb::from_rect(s.x as f32, s.y as f32, s.w as f32, s.h as f32, s.angle)),
            LayerKind::Emoji(e) => Some(Obb::from_rect(e.x as f32, e.y as f32, e.w as f32, e.h as f32, e.angle)),
            LayerKind::Text(t) => {
                if t.text.is_empty() {
                    return None;
                }
                let (w, h) = text::measure(fonts, t);
                Some(Obb::from_rect(t.x as f32, t.y as f32, w as f32, h as f32, t.angle))
            }
            LayerKind::Censor(c) => Some(Obb::from_rect(c.x as f32, c.y as f32, c.w as f32, c.h as f32, c.angle)),
            LayerKind::Overlay(o) => {
                let p = o.placement(src);
                p.enabled.then(|| Obb::from_rect(p.x as f32, p.y as f32, p.w as f32, p.h as f32, p.angle))
            }
        }
    }

    /// Content that should stay readable (never shown upside-down after flips).
    pub fn readable(&self) -> bool {
        match &self.kind {
            LayerKind::Censor(_) => false,
            // arrows and bubbles follow flips; an emoji stays the right way up
            LayerKind::Shape(s) => s.kind == ShapeKind::Emoji,
            _ => true,
        }
    }

    pub fn name(&self) -> String {
        if let Some(l) = self.label.as_ref().filter(|l| !l.trim().is_empty()) {
            return l.clone();
        }
        match &self.kind {
            LayerKind::Effects(_) => "Effects".into(),
            LayerKind::Text(t) => t.text.lines().next().unwrap_or("").to_string(),
            LayerKind::Censor(c) => format!("Censor · {:?}", c.method),
            LayerKind::Overlay(o) => o.src.name.clone(),
            LayerKind::Watermark(w) => if w.use_image { "Watermark · image".into() } else { format!("Watermark · {}", w.text) },
            LayerKind::Shield(_) => "AI shield".into(),
            LayerKind::Shape(s) => {
                if s.kind == ShapeKind::Emoji {
                    format!("Emoji · {}", crate::emoji::name_of(&s.emoji))
                } else {
                    format!("Shape · {}", s.kind.label())
                }
            }
            LayerKind::Markup(m) => format!("Markup · {} stroke{}", m.strokes.len(), if m.strokes.len() == 1 { "" } else { "s" }),
            LayerKind::Emoji(e) => format!("Emoji · {}", crate::emoji::name_of(&e.emoji)),
        }
    }
}

/// Hash of a layer's content (not its id or visibility).
pub fn layer_signature(l: &Layer) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match &l.kind {
        LayerKind::Effects(fx) => format!("{fx:?}").hash(&mut h),
        LayerKind::Text(t) => format!("{t:?}").hash(&mut h),
        LayerKind::Censor(c) => format!("{c:?}").hash(&mut h),
        LayerKind::Overlay(o) => (o.opacity.to_bits(), format!("{:?}", o.placements), Arc::as_ptr(&o.src) as usize).hash(&mut h),
        LayerKind::Watermark(w) => (format!("{w:?}"), w.image.as_ref().map(|i| Arc::as_ptr(i) as usize)).hash(&mut h),
        LayerKind::Shield(sh) => format!("{sh:?}").hash(&mut h),
        LayerKind::Shape(sh) => format!("{sh:?}").hash(&mut h),
        LayerKind::Markup(m) => format!("{m:?}").hash(&mut h),
        LayerKind::Emoji(e) => format!("{e:?}").hash(&mut h),
    }
    if !l.timing.is_default() {
        format!("{:?}", l.timing).hash(&mut h);
    }
    h.finish()
}

#[derive(Clone, Default)]
pub struct Stack {
    pub transform: Transform,
    /// Bottom-most first.
    pub layers: Vec<Layer>,
    pub next_id: u64,
}

/// Give the frames their sped up delays without going under 2 cs, which
/// browsers and players turn into 10 cs (a crawl). Frames too short to show
/// are skipped and their time goes to the frame before, so the overall speed
/// is right. Rounding error is carried along instead of piling up.
fn retime(plan: &mut [PlanFrame], exact: &[f64]) {
    let mut t = 0.0f64;
    let mut open: Option<(usize, i64)> = None; // frame being timed, its start
    for i in 0..plan.len() {
        if !plan[i].enabled {
            continue;
        }
        let now = t.round() as i64;
        match open {
            Some((_, start)) if now - start < 2 => {
                // still under 2 cs: this frame folds into the open one
                plan[i].enabled = false;
                plan[i].delay = 0;
            }
            _ => {
                if let Some((o, start)) = open {
                    plan[o].delay = (now - start).clamp(2, u16::MAX as i64) as u16;
                }
                open = Some((i, now));
            }
        }
        t += exact[i];
    }
    if let Some((o, start)) = open {
        plan[o].delay = (t.round() as i64 - start).clamp(2, u16::MAX as i64) as u16;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlanFrame {
    pub src: usize,
    pub delay: u16,
    pub enabled: bool,
}

/// Per-render cache: layer maps and text sprites (already scaled / rotated
/// into output space), computed once per render.
pub struct Prepared {
    pub maps: Vec<GeoMap>,
    sprites: Vec<Option<(RgbaImage, i64, i64)>>,
    watermarks: Vec<Option<WatermarkSprite>>,
    /// shape and markup sprites per layer, each with its source frame range
    stamps: Vec<Vec<StrokeSprite>>,
}

/// Run the calling thread at a lower priority, so long background renders
/// never starve the desktop compositor or the app's own UI.
pub fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        // on Linux the nice value is per thread
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
    #[cfg(windows)]
    unsafe {
        // Windows runs normal priority threads round-robin, so a pool of busy
        // workers would otherwise take turns with the UI thread and stall playback
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
    }
    #[cfg(target_os = "macos")]
    unsafe {
        // "utility" quality of service: runs on efficiency cores first and yields to the UI
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// Thread pool for background work (pre-rendering, compression, exports) so
/// it never competes with preview rendering on the UI thread's pool.
pub fn bg_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n.saturating_sub(1).max(1))
            .thread_name(|i| format!("gifkit-bg-{i}"))
            .start_handler(|_| lower_thread_priority())
            .build()
            .expect("thread pool")
    })
}

impl Stack {
    pub fn push(&mut self, kind: LayerKind) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.layers.push(Layer { id, visible: true, kind, label: None, timing: Default::default() });
        id
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn needs_render(&self) -> bool {
        !self.eff_transform(true).geometry_identity() || self.layers.iter().any(|l| l.active())
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.transform.is_identity()
    }

    pub fn count(&self, k: Kind) -> usize {
        self.layers.iter().filter(|l| l.kind() == k).count()
    }

    /// Hash of everything that affects rendering.
    pub fn signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", self.transform).hash(&mut h);
        for l in &self.layers {
            (l.id, l.visible, layer_signature(l)).hash(&mut h);
        }
        h.finish()
    }

    /// Hash of what affects the frame plan (order / count / timing).
    pub fn plan_signature(&self) -> u64 {
        let t = &self.transform;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (t.cut_on, t.cut_by_time, t.cut_start, t.cut_end, t.cut_t0.to_bits(), t.cut_t1.to_bits(), t.speed_on, format!("{:?}", t.speed_mode), t.speed_val.to_bits(), t.play_on && t.reverse, t.play_on && t.boomerang).hash(&mut h);
        h.finish()
    }

    pub fn plan(&self, base: &Animation) -> Vec<PlanFrame> {
        if base.is_empty() {
            return Vec::new();
        }
        let t = &self.transform;
        let (s, e) = t.cut_range(base);
        let mut idx: Vec<usize> = (s..=e).collect();
        if t.play_on && t.reverse {
            idx.reverse();
        }
        if t.play_on && t.boomerang && idx.len() > 2 {
            let back: Vec<usize> = idx[1..idx.len() - 1].iter().rev().copied().collect();
            idx.extend(back);
        }
        let mut plan: Vec<PlanFrame> = idx
            .into_iter()
            .map(|src| {
                let f = &base.frames[src];
                PlanFrame { src, delay: t.scale_delay(f.delay_cs), enabled: f.enabled }
            })
            .collect();
        if t.speed_on {
            let exact: Vec<f64> = plan.iter().map(|f| t.scale_delay_exact(base.frames[f.src].delay_cs)).collect();
            retime(&mut plan, &exact);
        }
        plan
    }

    pub fn loop_count(&self, base: &Animation) -> Option<u16> {
        let t = &self.transform;
        if t.play_on && t.set_loop {
            if t.loop_count == 0 { None } else { Some(t.loop_count) }
        } else {
            base.loop_count
        }
    }

    /// Map from source-frame space to the image each layer is drawn on:
    /// frame edits followed by every active effects layer below it.
    pub fn layer_maps(&self, w: u32, h: u32) -> Vec<GeoMap> {
        self.layer_maps_for(&self.eff_transform(true), w, h)
    }

    fn layer_maps_for(&self, t: &Transform, w: u32, h: u32) -> Vec<GeoMap> {
        let mut g = t.geo_map(w, h);
        let mut out = Vec::with_capacity(self.layers.len());
        for l in &self.layers {
            out.push(g);
            if let LayerKind::Effects(fx) = &l.kind {
                if l.active() {
                    fx.geo_map(&mut g);
                }
            }
        }
        out
    }

    pub fn prepare(&self, base: &Animation, fonts: &[FontEntry]) -> Prepared {
        let maps = self.layer_maps(base.width(), base.height());
        let sprites = self
            .layers
            .iter()
            .enumerate()
            .map(|(li, l)| match &l.kind {
                LayerKind::Text(t) if l.active() => {
                    let b = l.source_box(0, fonts)?;
                    let o = map_box(&maps[li].m, b, true);
                    let scale = ((o.w / b.w.max(1e-3)) * (o.h / b.h.max(1e-3))).sqrt();
                    let mut t2 = t.clone();
                    t2.size = (t.size * scale).clamp(1.0, 4000.0);
                    t2.wrap_px = t.wrap_px * scale;
                    let mut sp = text::render_sprite(fonts, &t2);
                    if (o.angle % 360.0).abs() > 0.01 {
                        sp = crate::ops::rotate_any_image(&sp, o.angle, image::Rgba([0, 0, 0, 0]), true);
                    }
                    let x = (o.cx - sp.width() as f32 / 2.0).round() as i64;
                    let y = (o.cy - sp.height() as f32 / 2.0).round() as i64;
                    Some((sp, x, y))
                }
                _ => None,
            })
            .collect();
        let watermarks = self
            .layers
            .iter()
            .enumerate()
            .map(|(li, l)| match &l.kind {
                LayerKind::Watermark(w) if l.active() => w.sprite(fonts, maps[li].w, maps[li].h),
                _ => None,
            })
            .collect();
        let stamps = self
            .layers
            .iter()
            .enumerate()
            .map(|(li, l)| {
                if !l.active() {
                    return Vec::new();
                }
                let m = &maps[li].m;
                match &l.kind {
                    LayerKind::Shape(s) => {
                        let Some(b) = l.source_box(0, fonts) else { return Vec::new() };
                        let o = map_box(m, b, l.readable());
                        shapes::shape_sprite(s, o, shapes::map_scale(m)).map(|(img, x, y)| vec![StrokeSprite { img, x, y, from: s.from, to: s.to }]).unwrap_or_default()
                    }
                    LayerKind::Emoji(e) => {
                        let Some(b) = l.source_box(0, fonts) else { return Vec::new() };
                        let o = map_box(m, b, true);
                        shapes::emoji_sprite(&e.emoji, o, e.shadow.then_some(&e.shadow_style)).map(|(img, x, y)| vec![StrokeSprite { img, x, y, from: e.from, to: e.to }]).unwrap_or_default()
                    }
                    LayerKind::Markup(mk) => {
                        let sc = shapes::map_scale(m);
                        mk.strokes
                            .par_iter()
                            .filter_map(|st| {
                                let pts: Vec<(f32, f32)> = st.points.iter().map(|p| m.apply(p[0], p[1])).collect();
                                shapes::stroke_sprite(st, &pts, st.width * sc)
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                }
            })
            .collect();
        Prepared { maps, sprites, watermarks, stamps }
    }

    /// The transformed frame before any layer (geometry only).
    pub fn geometry_frame(&self, base: &Animation, plan: &[PlanFrame], i: usize) -> RgbaImage {
        let src = &base.frames[plan[i].src].image;
        let t = self.eff_transform(true);
        if t.geometry_identity() { (**src).clone() } else { t.apply_geometry(src) }
    }

    /// Render output frame `i` of `plan`.
    pub fn render_frame(&self, base: &Animation, plan: &[PlanFrame], times: &[f32], i: usize, fonts: &[FontEntry], prep: &Prepared) -> RgbaImage {
        let n = plan.len();
        let src = plan[i].src;
        let mut img = self.geometry_frame(base, plan, i);
        for (li, l) in self.layers.iter().enumerate() {
            if !l.active() || !l.timing.shows(i, n) {
                continue;
            }
            let mo = l.timing.motion(i);
            if mo.opacity <= 0.001 {
                continue;
            }
            // a keyed fade mixes the layer's result with what was under it
            let before = (mo.opacity < 0.999).then(|| img.clone());
            let m = &prep.maps[li].m;
            // sprites drawn ahead of time follow the motion as bitmaps
            let draw = |img: &mut RgbaImage, sp: &RgbaImage, x: i64, y: i64, alpha: f32| {
                if mo.moves() {
                    let centre = l.source_box(src, fonts).map(|b| map_box(m, b, false)).map(|o| (o.cx, o.cy)).unwrap_or((x as f32 + sp.width() as f32 / 2.0, y as f32 + sp.height() as f32 / 2.0));
                    let d = m.apply_vec(mo.dx, mo.dy);
                    if let Some((t, tx, ty)) = crate::timeline::transform_sprite(sp, x, y, centre, d, mo.scale, mo.angle * m.det().signum()) {
                        crate::ops::overlay_with_opacity(img, &t, tx, ty, alpha);
                    }
                } else {
                    crate::ops::overlay_with_opacity(img, sp, x, y, alpha);
                }
            };
            match &l.kind {
                LayerKind::Effects(fx) => {
                    // effects run over their own clip, so an intro starts where the clip does
                    let (s0, s1) = l.timing.clip_at(i, n).filter(|_| l.timing.clips.is_some()).unwrap_or((0, n.saturating_sub(1)));
                    img = effects::apply_frame(&img, i - s0, s1 - s0 + 1, times[i] - times[s0], fx, fonts);
                }
                LayerKind::Censor(c) => {
                    if c.applies_to(src) {
                        if let Some(b) = l.source_box(src, fonts) {
                            img = effects::censor_obb(&img, map_box(m, mo.apply(b), false), c);
                        }
                    }
                }
                LayerKind::Overlay(o) => {
                    if let Some(b) = l.source_box(src, fonts) {
                        let frame = o.src.frame_at(times[i]);
                        crate::overlay::draw_in_obb(&mut img, frame, map_box(m, mo.apply(b), true), o.opacity / 100.0);
                    }
                }
                LayerKind::Watermark(w) => {
                    if let Some(Some(sp)) = prep.watermarks.get(li) {
                        effects::draw_watermark(&mut img, sp, w.opacity);
                    }
                }
                LayerKind::Shield(sh) => img = effects::shield(&img, sh, src),
                LayerKind::Shape(sh) => {
                    if let Some(st) = prep.stamps.get(li) {
                        for sp in st.iter().filter(|_| sh.applies_to(src)) {
                            draw(&mut img, &sp.img, sp.x, sp.y, sh.opacity / 100.0);
                        }
                    }
                }
                LayerKind::Emoji(e) => {
                    if let Some(st) = prep.stamps.get(li) {
                        for sp in st.iter().filter(|_| e.applies_to(src)) {
                            draw(&mut img, &sp.img, sp.x, sp.y, e.opacity / 100.0);
                        }
                    }
                }
                LayerKind::Markup(_) => {
                    if let Some(st) = prep.stamps.get(li) {
                        for sp in st {
                            let i = src + 1;
                            if i >= sp.from.max(1) && (sp.to == 0 || i <= sp.to) {
                                draw(&mut img, &sp.img, sp.x, sp.y, 1.0);
                            }
                        }
                    }
                }
                LayerKind::Text(t) => {
                    if t.caption != text::Caption::None && t.bar_h > 0 && t.applies_to(src) {
                        let c = t.bar_color;
                        let (w, h) = img.dimensions();
                        let y0 = t.bar_y.clamp(0, h as i32) as u32;
                        let y1 = (t.bar_y + t.bar_h as i32).clamp(0, h as i32) as u32;
                        for y in y0..y1 {
                            for x in 0..w {
                                img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
                            }
                        }
                    }
                    if let Some(Some((s, x, y))) = prep.sprites.get(li) {
                        if t.applies_to(src) {
                            draw(&mut img, s, *x, *y, t.opacity as f32 / 100.0);
                        }
                    }
                }
            }
            if let Some(b) = before {
                crate::timeline::blend(&mut img, &b, mo.opacity);
            }
        }
        img
    }

    /// The frame edits plus the room caption bars need (when `captions`).
    pub fn eff_transform(&self, captions: bool) -> Transform {
        let mut t = self.transform.clone();
        if !captions {
            return t;
        }
        let (mut top, mut bottom) = (0u32, 0u32);
        for l in &self.layers {
            if let LayerKind::Text(a) = &l.kind {
                if l.active() {
                    match a.caption {
                        text::Caption::Top => top += a.bar_h,
                        text::Caption::Bottom => bottom += a.bar_h,
                        text::Caption::None => {}
                    }
                }
            }
        }
        if top + bottom > 0 {
            if !t.pad_on {
                t.pad = [0; 4];
                t.pad_transparent = false;
            }
            t.pad_on = true;
            t.pad[0] += top;
            t.pad[2] += bottom;
        }
        t
    }

    /// Lay out text that follows the canvas: wrapping at its edge, centring
    /// across it, and caption bars sized to their text. Returns whether
    /// anything moved (the caller re-renders then).
    pub fn layout_text(&mut self, w: u32, h: u32, fonts: &[FontEntry]) -> bool {
        use text::Caption;
        if fonts.is_empty() || !self.layers.iter().any(|l| matches!(&l.kind, LayerKind::Text(t) if t.wrap || t.centre || t.caption != Caption::None)) {
            return false;
        }
        // the canvas without caption bars sets the line length and bar sizes
        let maps0 = self.layer_maps_for(&self.eff_transform(false), w, h);
        let mut changed = false;
        let mut bars: Vec<(usize, Caption, u32)> = Vec::new();
        for li in 0..self.layers.len() {
            let active = self.layers[li].active();
            let LayerKind::Text(t) = &mut self.layers[li].kind else { continue };
            let k = maps0[li].m.det().abs().sqrt().max(1e-3);
            let wrap = t.wrap || t.caption != Caption::None;
            let wp = if wrap { (maps0[li].w * 0.92 / k).max(8.0) } else { 0.0 };
            if (t.wrap_px - wp).abs() > 0.25 {
                t.wrap_px = wp;
                changed = true;
            }
            if t.caption != Caption::None {
                let bh = if active && !t.text.is_empty() {
                    let (_, th) = text::measure(fonts, t);
                    (th as f32 * k + 2.0 * t.size * k * t.bar_pad / 100.0).round().max(1.0) as u32
                } else {
                    0
                };
                if t.bar_h != bh {
                    t.bar_h = bh;
                    changed = true;
                }
                if bh > 0 {
                    bars.push((li, t.caption, bh));
                }
            }
        }
        let top: u32 = bars.iter().filter(|b| b.1 == Caption::Top).map(|b| b.2).sum();
        // with the bars in place, put each text where it belongs
        let maps = self.layer_maps(w, h);
        let mut bar_y: std::collections::HashMap<usize, (i32, u32)> = std::collections::HashMap::new();
        let (mut ty, mut by) = (0u32, 0u32);
        for (li, side, bh) in &bars {
            let y = match side {
                Caption::Top => {
                    let y = ty;
                    ty += bh;
                    y
                }
                _ => {
                    let y = top + maps0[*li].h.round() as u32 + by;
                    by += bh;
                    y
                }
            };
            bar_y.insert(*li, (y as i32, *bh));
        }
        for li in 0..self.layers.len() {
            let LayerKind::Text(t) = &mut self.layers[li].kind else { continue };
            if !(t.centre || t.caption != Caption::None) || t.text.is_empty() {
                continue;
            }
            let m = maps[li].m;
            let inv = m.inverse();
            let (tw, th) = text::measure(fonts, t);
            let (sx, sy) = (t.x as f32 + tw as f32 / 2.0, t.y as f32 + th as f32 / 2.0);
            let (_, oy_now) = m.apply(sx, sy);
            let ox = maps[li].w / 2.0;
            let oy = match bar_y.get(&li) {
                Some(&(y, bh)) => {
                    if t.bar_y != y {
                        t.bar_y = y;
                        changed = true;
                    }
                    if t.angle != 0.0 {
                        t.angle = 0.0;
                        changed = true;
                    }
                    y as f32 + bh as f32 / 2.0
                }
                None => oy_now,
            };
            let (cx, cy) = inv.apply(ox, oy);
            let (nx, ny) = ((cx - tw as f32 / 2.0).round() as i32, (cy - th as f32 / 2.0).round() as i32);
            if (nx, ny) != (t.x, t.y) {
                (t.x, t.y) = (nx, ny);
                changed = true;
            }
        }
        changed
    }

    /// Whether layer `li` shows on each output frame by its own frame range
    /// (before it has timeline clips).
    pub fn legacy_shown(&self, li: usize, plan: &[PlanFrame]) -> Vec<bool> {
        let l = &self.layers[li];
        plan.iter()
            .map(|f| match &l.kind {
                LayerKind::Text(t) => t.applies_to(f.src),
                LayerKind::Censor(c) => c.applies_to(f.src),
                LayerKind::Shape(s) => s.applies_to(f.src),
                LayerKind::Emoji(e) => e.applies_to(f.src),
                _ => true,
            })
            .collect()
    }

    /// Give layer `li` explicit clips (from its old frame range), and hand
    /// the range over to them so the two don't both apply.
    pub fn ensure_clips(&mut self, li: usize, plan: &[PlanFrame]) {
        if self.layers[li].timing.clips.is_some() {
            return;
        }
        let shown = self.legacy_shown(li, plan);
        let l = &mut self.layers[li];
        l.timing.take_clips(&shown);
        match &mut l.kind {
            LayerKind::Text(t) => (t.from, t.to) = (1, 0),
            LayerKind::Censor(c) => (c.from, c.to) = (1, 0),
            LayerKind::Shape(s) => (s.from, s.to) = (1, 0),
            LayerKind::Emoji(e) => (e.from, e.to) = (1, 0),
            _ => {}
        }
    }

    /// Bake the stack into a new animation.
    pub fn flatten(&self, base: &Animation, fonts: &[FontEntry], progress: Option<&AtomicUsize>) -> Animation {
        let plan = self.plan(base);
        let times = plan_times(&plan);
        let prep = self.prepare(base, fonts);
        let render = self.needs_render();
        let frames: Vec<Frame> = (0..plan.len())
            .into_par_iter()
            .map(|i| {
                // untouched frames share the source image rather than copying it
                let image = if render { Arc::new(self.render_frame(base, &plan, &times, i, fonts, &prep)) } else { base.frames[plan[i].src].image.clone() };
                if let Some(p) = progress {
                    p.fetch_add(1, Ordering::Relaxed);
                }
                Frame { image, delay_cs: plan[i].delay, enabled: plan[i].enabled }
            })
            .collect();
        let mut out = Animation { frames, loop_count: self.loop_count(base) };
        effects::normalize_sizes(&mut out);
        out
    }

    /// Keep per-frame data in sync with the number of source frames.
    pub fn sync_len(&mut self, n: usize) {
        for l in &mut self.layers {
            if let LayerKind::Overlay(o) = &mut l.kind {
                if o.placements.len() != n && n > 0 {
                    let last = o.placements.last().copied().unwrap_or(Placement { enabled: true, x: 0, y: 0, w: 64, h: 64, angle: 0.0 });
                    o.placements.resize(n, last);
                }
            }
        }
    }
}

/// Start time (seconds) of each plan frame.
pub fn plan_times(plan: &[PlanFrame]) -> Vec<f32> {
    let mut t = 0.0;
    plan.iter()
        .map(|f| {
            let s = t;
            if f.enabled {
                t += effective_delay(f.delay) as f32 / 100.0;
            }
            s
        })
        .collect()
}

/// Index of the frame showing at time `t` (seconds, wrapped) in an animation.
pub fn frame_at_time(a: &Animation, t: f32) -> usize {
    let total = a.duration_secs();
    if total <= 0.0 || a.is_empty() {
        return 0;
    }
    let mut tt = t.rem_euclid(total);
    for (i, f) in a.frames.iter().enumerate() {
        if !f.enabled {
            continue;
        }
        let d = effective_delay(f.delay_cs) as f32 / 100.0;
        if tt < d {
            return i;
        }
        tt -= d;
    }
    a.len() - 1
}
