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
        !(self.crop_on && self.crop[2] > 0 && self.crop[3] > 0) && !self.resize_on && !self.rotates()
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
            return out;
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
        out
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
        g
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Effects,
    Text,
    Censor,
    Overlay,
    Watermark,
    Shield,
}

#[derive(Clone)]
pub struct Layer {
    pub id: u64,
    pub visible: bool,
    pub kind: LayerKind,
    /// user-chosen name (None = automatic)
    pub label: Option<String>,
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
            }
    }
    /// The layer's oriented box in source-frame space (for source frame `src`).
    pub fn source_box(&self, src: usize, fonts: &[FontEntry]) -> Option<Obb> {
        match &self.kind {
            LayerKind::Effects(_) | LayerKind::Watermark(_) | LayerKind::Shield(_) => None,
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
        !matches!(self.kind, LayerKind::Censor(_))
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
}

/// Thread pool for background work (pre-rendering, compression, exports) so
/// it never competes with preview rendering on the UI thread's pool.
pub fn bg_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n.saturating_sub(1).max(1))
            .thread_name(|i| format!("gifforge-bg-{i}"))
            .build()
            .expect("thread pool")
    })
}

impl Stack {
    pub fn push(&mut self, kind: LayerKind) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.layers.push(Layer { id, visible: true, kind, label: None });
        id
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn needs_render(&self) -> bool {
        !self.transform.geometry_identity() || self.layers.iter().any(|l| l.active())
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
        idx.into_iter()
            .map(|src| {
                let f = &base.frames[src];
                PlanFrame { src, delay: t.scale_delay(f.delay_cs), enabled: f.enabled }
            })
            .collect()
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
        let mut g = self.transform.geo_map(w, h);
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
        Prepared { maps, sprites, watermarks }
    }

    /// The transformed frame before any layer (geometry only).
    pub fn geometry_frame(&self, base: &Animation, plan: &[PlanFrame], i: usize) -> RgbaImage {
        let src = &base.frames[plan[i].src].image;
        if self.transform.geometry_identity() { (**src).clone() } else { self.transform.apply_geometry(src) }
    }

    /// Render output frame `i` of `plan`.
    pub fn render_frame(&self, base: &Animation, plan: &[PlanFrame], times: &[f32], i: usize, fonts: &[FontEntry], prep: &Prepared) -> RgbaImage {
        let n = plan.len();
        let src = plan[i].src;
        let mut img = self.geometry_frame(base, plan, i);
        for (li, l) in self.layers.iter().enumerate() {
            if !l.active() {
                continue;
            }
            let m = &prep.maps[li].m;
            match &l.kind {
                LayerKind::Effects(fx) => img = effects::apply_frame(&img, i, n, times[i], fx, fonts),
                LayerKind::Censor(c) => {
                    if c.applies_to(src) {
                        if let Some(b) = l.source_box(src, fonts) {
                            img = effects::censor_obb(&img, map_box(m, b, false), c);
                        }
                    }
                }
                LayerKind::Overlay(o) => {
                    if let Some(b) = l.source_box(src, fonts) {
                        let frame = o.src.frame_at(times[i]);
                        crate::overlay::draw_in_obb(&mut img, frame, map_box(m, b, true), o.opacity / 100.0);
                    }
                }
                LayerKind::Watermark(w) => {
                    if let Some(Some(sp)) = prep.watermarks.get(li) {
                        effects::draw_watermark(&mut img, sp, w.opacity);
                    }
                }
                LayerKind::Shield(sh) => img = effects::shield(&img, sh, src),
                LayerKind::Text(t) => {
                    if let Some(Some((s, x, y))) = prep.sprites.get(li) {
                        if t.applies_to(src) {
                            crate::ops::overlay_with_opacity(&mut img, s, *x, *y, t.opacity as f32 / 100.0);
                        }
                    }
                }
            }
        }
        img
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
                let img = if render { self.render_frame(base, &plan, &times, i, fonts, &prep) } else { (*base.frames[plan[i].src].image).clone() };
                if let Some(p) = progress {
                    p.fetch_add(1, Ordering::Relaxed);
                }
                Frame { image: Arc::new(img), delay_cs: plan[i].delay, enabled: plan[i].enabled }
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
