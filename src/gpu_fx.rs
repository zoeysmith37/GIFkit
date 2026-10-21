//! Effects layers, censors and the AI shield as passes for the graphics
//! card (see gpu_fx.wgsl). Each function here follows its CPU twin step by
//! step (`effects::apply_frame`, `booth::apply`, `motion::looped` and
//! `motion::transition`, `effects::censor_obb`, `effects::shield`), working
//! out sizes and numbers on the CPU so the shader only does per-pixel work.
//! Error-diffusion dither is the one thing left out: each pixel depends on
//! every pixel before it, so it stays on the CPU.

use crate::booth::BoothFx;
use crate::effects::{CensorMethod, CensorSettings, EffectSettings, ShieldSettings};
use crate::gpu_render::{GpuIn, GpuPass, PassKind};
use crate::motion::{LoopFx, Transition};
use crate::text::FontEntry;
use image::RgbaImage;
use std::f32::consts::TAU;

/// Scratch pictures the passes share.
const T_FLAGS: u8 = 0;
const T_BLUR: u8 = 1;
const T_BLOCKS: u8 = 2;
const T_SMALL: u8 = 3;
const T_BIG: u8 = 4;
const T_REGION: u8 = 5;
const T_MID: u8 = 6;
/// the picture before a filter, to mix back toward
const T_PRE: u8 = 7;

/// Builds the passes for one layer, keeping track of the frame's size.
pub struct Plan<'a, 'f> {
    pub passes: Vec<GpuPass<'a>>,
    pub size: (u32, u32),
    pub owned: &'f mut Vec<RgbaImage>,
}

impl<'a, 'f> Plan<'a, 'f> {
    pub fn new(size: (u32, u32), owned: &'f mut Vec<RgbaImage>) -> Self {
        Plan { passes: Vec::new(), size, owned }
    }

    fn wf(&self) -> (f32, f32) {
        (self.size.0 as f32, self.size.1 as f32)
    }

    /// A pass from `ins` into `out` (None: the frame) at `size`.
    fn raw(&mut self, op: u32, sub: f32, ins: [Option<GpuIn<'a>>; 3], out: Option<u8>, size: (u32, u32), float: bool, more: &[[f32; 4]]) {
        let mut u = vec![[op as f32, sub, size.0 as f32, size.1 as f32]];
        u.extend_from_slice(more);
        self.passes.push(GpuPass { kind: PassKind::Fx, size, out, float, ins, u });
        if out.is_none() {
            self.size = size;
        }
    }

    /// A pass over the frame that keeps its size.
    fn op(&mut self, op: u32, more: &[[f32; 4]]) {
        let s = self.size;
        self.raw(op, 0.0, [Some(GpuIn::Cur), None, None], None, s, false, more);
    }

    fn op_sub(&mut self, op: u32, sub: f32, more: &[[f32; 4]]) {
        let s = self.size;
        self.raw(op, sub, [Some(GpuIn::Cur), None, None], None, s, false, more);
    }

    fn modulate(&mut self, hue: f32, sat: f32, light: f32) {
        self.op(3, &[[(hue - 100.0) / 200.0, sat / 100.0, light / 100.0, 0.0]]);
    }

    fn bright_contrast(&mut self, bright: f32, contrast: f32) {
        let c = contrast.clamp(-100.0, 100.0) * 2.55;
        let cf = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
        self.op(4, &[[cf, bright * 2.55, 0.0, 0.0]]);
    }

    fn colorize(&mut self, c: [u8; 3], amount: f32) {
        self.op(5, &[[c[0] as f32, c[1] as f32, c[2] as f32, amount.clamp(0.0, 1.0)]]);
    }

    fn gamma(&mut self, g: f32) {
        self.op(6, &[[1.0 / g, 0.0, 0.0, 0.0]]);
    }

    fn vignette(&mut self, strength: f32, c: [u8; 3]) {
        self.op(7, &[[strength, c[0] as f32, c[1] as f32, c[2] as f32]]);
    }

    /// (v * m + add) / div on each colour channel
    fn affine(&mut self, m: [f32; 3], add: [f32; 3], div: [f32; 3]) {
        self.op(11, &[[m[0], m[1], m[2], 0.0], [add[0], add[1], add[2], 0.0], [div[0], div[1], div[2], 1.0]]);
    }

    /// imageops::blur of `src` into `out`, edges clamped to `region` (x0 y0 x1 y1).
    fn blur(&mut self, src: GpuIn<'a>, out: Option<u8>, size: (u32, u32), sigma: f32, region: [f32; 4]) {
        let sigma = if sigma == 0.0 { 0.8 } else { sigma };
        let n = kernel_size(sigma) as f32;
        self.raw(16, 0.0, [Some(src), None, None], Some(T_MID), size, true, &[[n, sigma, 0.0, 0.0], region]);
        self.raw(17, 0.0, [Some(GpuIn::Tmp(T_MID)), None, None], out, size, false, &[[n, sigma, 0.0, 0.0], region]);
    }

    fn blur_all(&mut self, src: GpuIn<'a>, out: Option<u8>, size: (u32, u32), sigma: f32) {
        self.blur(src, out, size, sigma, [0.0, 0.0, size.0 as f32, size.1 as f32]);
    }

    /// imageops::resize with the triangle filter, `src` of `from` into `out`.
    fn resize_tri(&mut self, src: GpuIn<'a>, from: (u32, u32), to: (u32, u32), out: u8) {
        if from == to {
            // the CPU copies
            self.raw(0, 0.0, [Some(src), None, None], Some(out), to, false, &[]);
            return;
        }
        let axis = |a: u32, b: u32| {
            let ratio = a as f32 / b as f32;
            let sratio = ratio.max(1.0);
            [ratio, sratio, sratio, 1.0]
        };
        self.passes.push(GpuPass { kind: PassKind::ResizeV, size: (from.0, to.1), out: Some(T_MID), float: true, ins: [Some(src), None, None], u: vec![axis(from.1, to.1), [0.0, from.1 as f32, 0.0, 1.0]] });
        self.passes.push(GpuPass { kind: PassKind::ResizeH, size: to, out: Some(out), float: false, ins: [Some(GpuIn::Tmp(T_MID)), None, None], u: vec![axis(from.0, to.0), [0.0, from.0 as f32, 0.0, 0.0]] });
    }

    /// Exact pixel moves: in = (a X + b Y + c, d X + e Y + f).
    fn remap(&mut self, size: (u32, u32), m: [f32; 6]) {
        self.raw(28, 0.0, [Some(GpuIn::Cur), None, None], None, size, false, &[[m[0], m[1], m[2], m[3]], [m[4], m[5], 0.0, 0.0]]);
    }

    /// A picture made on the CPU, over the frame at (x, y).
    fn over(&mut self, img: RgbaImage, x: i32, y: i32, opacity: f32) {
        let k = self.owned.len();
        self.owned.push(img);
        let s = self.size;
        self.raw(31, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Owned(k)), None], None, s, false, &[[x as f32, y as f32, opacity, 0.0]]);
    }

    /// text::draw_simple
    fn text(&mut self, fonts: &[FontEntry], text: &str, x: i32, y: i32, size: f32) {
        if fonts.is_empty() {
            return;
        }
        let a = crate::text::Annotation { text: text.into(), size, x, y, align: crate::text::Align::Left, ..Default::default() };
        let s = crate::text::render_sprite(fonts, &a);
        self.over(s, a.x, a.y, a.opacity as f32 / 100.0);
    }

    /// motion::place
    fn place(&mut self, scale: f32, angle_deg: f32, dx: f32, dy: f32, transparent: bool) {
        self.place_xy(scale, scale, angle_deg, dx, dy, transparent);
    }

    /// motion::place_xy
    fn place_xy(&mut self, sx: f32, sy: f32, angle_deg: f32, dx: f32, dy: f32, transparent: bool) {
        let (s, c) = (-angle_deg.to_radians()).sin_cos();
        let bg = if transparent { [0.0; 4] } else { [0.0, 0.0, 0.0, 255.0] };
        self.op(36, &[[sx, s, c, sy], [dx, dy, 0.0, 0.0], bg]);
    }

    /// Run `f` at `amount` %: below 100 the result is mixed back toward the
    /// picture before it (timeline::blend).
    fn at_strength(&mut self, amount: f32, f: impl FnOnce(&mut Self)) {
        if (amount - 100.0).abs() < 0.05 {
            f(self);
            return;
        }
        let size = self.size;
        self.raw(0, 0.0, [Some(GpuIn::Cur), None, None], Some(T_PRE), size, false, &[]);
        f(self);
        let size = self.size;
        self.raw(50, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_PRE)), None], None, size, false, &[[(amount / 100.0).clamp(0.0, crate::effects::MAX_STRENGTH), size.0 as f32, size.1 as f32, 0.0]]);
    }

    /// motion::pixelate
    fn pixelate(&mut self, block: u32) {
        let (wu, hu) = self.size;
        self.blocks([0, 0, wu, hu], block, T_BLOCKS);
        self.raw(39, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_BLOCKS)), None], None, (wu, hu), false, &[[block as f32, 0.0, 0.0, 0.0]]);
    }

    /// motion's mask: kind, numbers, see-through
    fn mask(&mut self, kind: u32, a: [f32; 4], b: [f32; 4], transparent: bool) {
        self.op_sub(37, kind as f32, &[a, b, [if transparent { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]]);
    }

    /// Average `block`-sized squares of `region` (x y w h) into scratch `out`.
    fn blocks(&mut self, region: [u32; 4], block: u32, out: u8) {
        let size = (region[2].div_ceil(block), region[3].div_ceil(block));
        self.raw(38, 0.0, [Some(GpuIn::Cur), None, None], Some(out), size, false, &[[region[0] as f32, region[1] as f32, region[2] as f32, region[3] as f32], [block as f32, 0.0, 0.0, 0.0]]);
    }
}

/// image's gaussian kernel size for a sigma.
fn kernel_size(sigma: f32) -> u32 {
    let possible = (((((sigma - 0.8) / 0.3) + 1.) * 2.) + 1.).max(3.) as u32;
    if possible % 2 == 0 { possible + 1 } else { possible }
}

/// Can the card do every effect this layer has on?
pub fn effects_ok(s: &EffectSettings) -> bool {
    !s.error_dither
}

/// effects::apply_frame as passes.
pub fn effects<'a>(p: &mut Plan<'a, '_>, s: &EffectSettings, idx: usize, n: usize, elapsed_s: f32, clip_s: f32, fonts: &[FontEntry]) {
    let d = EffectSettings::default();
    if s.hue != d.hue || s.saturation != d.saturation || s.lightness != d.lightness {
        p.modulate(s.hue, s.saturation, s.lightness);
    }
    if s.brightness != 0.0 || s.contrast != 0.0 {
        p.bright_contrast(s.brightness, s.contrast);
    }
    if s.rc_white || s.rc_black || s.rc_custom {
        let edges = if s.rc_clean_edges { Some(s.rc_edge_px) } else { None };
        for (on, c) in [(s.rc_white, [255, 255, 255]), (s.rc_black, [0, 0, 0]), (s.rc_custom, s.rc_color)] {
            if !on {
                continue;
            }
            let thr = s.rc_fuzz.max(0.0) / 100.0 * 441.67;
            let col = [c[0] as f32, c[1] as f32, c[2] as f32];
            p.op(13, &[[col[0], col[1], col[2], thr]]);
            if let Some(px) = edges.filter(|px| *px > 0) {
                let edge_thr = (thr * 4.0).max(0.30 * 441.67);
                let size = p.size;
                p.raw(14, 0.0, [Some(GpuIn::Cur), None, None], Some(T_FLAGS), size, false, &[[px as f32, 0.0, 0.0, 0.0]]);
                p.raw(15, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_FLAGS)), None], None, size, false, &[[col[0], col[1], col[2], edge_thr], [px as f32, 0.0, 0.0, 0.0]]);
            }
        }
    }
    if s.grayscale {
        p.op(8, &[]);
    }
    if s.sepia {
        p.op(9, &[]);
    }
    if s.monochrome {
        p.op(10, &[]);
    }
    if s.negative {
        p.affine([-1.0; 3], [255.0; 3], [1.0; 3]);
    }
    if s.tint {
        let c = s.tint_color;
        p.op(12, &[[c[0] as f32, c[1] as f32, c[2] as f32, (s.tint_amount / 100.0).clamp(0.0, 1.0)]]);
    }
    if s.blur {
        let size = p.size;
        p.blur_all(GpuIn::Cur, None, size, s.blur_sigma.clamp(0.1, crate::effects::MAX_SIGMA));
    }
    if s.sharpen {
        let size = p.size;
        p.blur_all(GpuIn::Cur, Some(T_BLUR), size, s.sharpen_sigma.clamp(0.1, crate::effects::MAX_SIGMA));
        p.raw(18, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_BLUR)), None], None, size, false, &[[2.0, 0.0, 0.0, 0.0]]);
    }
    if s.oil {
        p.op(19, &[[s.oil_radius.clamp(1, 16) as f32, s.oil_detail.clamp(2, 64) as f32, 0.0, 0.0]]);
    }
    if s.emboss {
        let k = crate::effects::emboss_kernel(s.emboss_dir);
        p.at_strength(s.emboss_amount, |p| p.op(20, &[[k[0], k[1], k[2], k[3]], [k[4], k[5], k[6], k[7]], [k[8], if s.emboss_colour { 1.0 } else { 0.0 }, 0.0, 0.0]]));
    }
    if s.posterize {
        p.op(21, &[[s.posterize_levels.max(2) as f32 - 1.0, 0.0, 0.0, 0.0]]);
    }
    if s.solarize {
        p.op(22, &[[s.solarize_threshold, if s.solarize_reverse { 1.0 } else { 0.0 }, 0.0, 0.0]]);
    }
    if s.noise {
        let seed = if s.noise_still { 1.0 } else { idx as f32 + 1.0 };
        p.op(23, &[[s.noise_amount, seed, p.size.0 as f32, if s.noise_mono { 1.0 } else { 0.0 }]]);
    }
    if s.pixelate && s.pixelate_size > 1 {
        p.pixelate(s.pixelate_size);
    }
    if s.swirl {
        let (w, h) = p.wf();
        let (cx, cy) = (w / 2.0, h / 2.0);
        p.op_sub(24, 0.0, &[[cx, cy, cx.min(cy), s.swirl_deg.to_radians()]]);
    }
    if s.implode {
        let (w, h) = p.wf();
        let (cx, cy) = (w / 2.0, h / 2.0);
        p.op_sub(24, 1.0, &[[cx, cy, cx.min(cy), s.implode_amount]]);
    }
    if s.wave {
        let phase = idx as f32 / n.max(1) as f32 * TAU * if s.wave_reverse { -1.0 } else { 1.0 };
        p.op_sub(24, 2.0, &[[s.wave_amp, s.wave_len.max(1.0), phase, 0.0]]);
    }
    let (looks, warps) = s.booth_items();
    for it in looks.iter().chain(warps.iter()) {
        booth_item(p, it);
    }
    if s.gotham {
        p.at_strength(s.gotham_amount, |p| {
            p.modulate(100.0, 10.0, 120.0);
            p.colorize([0x22, 0x2b, 0x6d], 0.2);
            p.gamma(0.5);
            p.bright_contrast(0.0, 25.0);
        });
    }
    if s.lomo {
        p.at_strength(s.lomo_amount, |p| {
            let lo = -(0.22 * 255.0);
            p.affine([1.0; 3], [lo, lo, 0.0], [0.56, 0.56, 1.0]);
            p.vignette(0.85, [0, 0, 0]);
        });
    }
    if s.nashville {
        p.at_strength(s.nashville_amount, |p| {
            p.colorize([0xf7, 0xda, 0xae], 0.25);
            p.affine([0.87, 0.87, 0.87], [0.13 * 255.0 * 0.6, 0.13 * 255.0 * 0.2, 0.13 * 255.0 * 0.3], [1.0; 3]);
            p.bright_contrast(5.0, 15.0);
            p.gamma(1.2);
        });
    }
    if s.toaster {
        p.at_strength(s.toaster_amount, |p| {
            p.colorize([0xf6, 0x8b, 0x1f], 0.18);
            p.modulate(100.0, 85.0, 110.0);
            p.gamma(1.15);
            p.vignette(0.7, [0x50, 0x10, 0x00]);
        });
    }
    if s.vignette {
        p.vignette(0.9 * s.vignette_amount / 100.0, [0, 0, 0]);
    }
    if s.ordered_dither {
        p.op(26, &[[s.dither_levels.max(2) as f32 - 1.0, 0.0, 0.0, 0.0]]);
    }
    if s.change_bg {
        let c = s.bg_color;
        p.op(27, &[[c[0] as f32, c[1] as f32, c[2] as f32, 0.0]]);
    }
    let (w, h) = p.size;
    let (wf, hf) = (w as f32, h as f32);
    if s.flip_v {
        p.remap((w, h), [1.0, 0.0, 0.0, 0.0, -1.0, hf - 1.0]);
    }
    if s.flip_h {
        p.remap((w, h), [-1.0, 0.0, wf - 1.0, 0.0, 1.0, 0.0]);
    }
    if s.rot90 {
        let (w, h) = p.size;
        p.remap((h, w), [0.0, 1.0, 0.0, -1.0, 0.0, h as f32 - 1.0]);
    }
    if s.rot270 {
        let (w, h) = p.size;
        p.remap((h, w), [0.0, -1.0, w as f32 - 1.0, 1.0, 0.0, 0.0]);
    }
    if s.rot_any {
        let (w, h) = p.wf();
        let (sn, cs) = s.rot_deg.to_radians().sin_cos();
        let nw = (w * cs.abs() + h * sn.abs()).ceil() as u32;
        let nh = (w * sn.abs() + h * cs.abs()).ceil() as u32;
        p.raw(29, 0.0, [Some(GpuIn::Cur), None, None], None, (nw, nh), false, &[[cs, sn, w / 2.0, h / 2.0], [nw as f32 / 2.0, nh as f32 / 2.0, 0.0, 0.0]]);
    }
    if s.frame_camera {
        camera(p, fonts, idx);
    }
    if s.frame_fuzzy {
        let (w, h) = p.size;
        let f = (w.min(h) as f32 * 0.08).max(2.0);
        p.raw(32, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Noise(0)), None], None, (w, h), false, &[[f, 0.0, 0.0, 0.0]]);
    }
    if s.frame_rounded {
        let (w, h) = p.wf();
        p.op(33, &[[s.rounded_radius.min(w / 2.0).min(h / 2.0), 0.0, 0.0, 0.0]]);
    }
    if s.polaroid {
        let (w, h) = p.size;
        let side = ((w.min(h) as f32) * 0.06).round().max(4.0) as u32;
        p.raw(34, 0.0, [Some(GpuIn::Cur), None, None], None, (w + side * 2, h + side * 5), false, &[[side as f32, 0.0, 0.0, 0.0]]);
    }
    if s.frame_border {
        let (w, h) = p.size;
        let px = s.border_px;
        let c = s.border_color;
        p.raw(35, 0.0, [Some(GpuIn::Cur), None, None], None, (w + px * 2, h + px * 2), false, &[[px as f32, c[0] as f32, c[1] as f32, c[2] as f32]]);
    }
    let loops = s.loop_fx();
    if !loops.is_empty() {
        const STEPS: usize = 3600;
        let (li, ln) = if clip_s > 0.0 { (((elapsed_s / clip_s).clamp(0.0, 1.0) * STEPS as f32).round() as usize, STEPS) } else { (idx, n) };
        looped(p, &loops, li, ln, s.loop_amount, s.loop_cycles, s.fade_to_transparent, s.loop_reverse);
    }
    let (fi, fo) = s.transition_frames();
    let (intro, outro) = s.transitions();
    if intro != Transition::None && idx < fi {
        transition(p, intro, (idx as f32 + 1.0) / (fi as f32 + 1.0), s.fade_to_transparent);
    }
    if outro != Transition::None && n > 0 && idx + fo >= n {
        transition(p, outro, (n - idx) as f32 / (fo as f32 + 1.0), s.fade_to_transparent);
    }
    if s.counter {
        let (w, h) = p.size;
        if let Some((sp, x, y, o)) = crate::effects::counter_sprite(fonts, elapsed_s, w, h, s.counter_pos, s.counter_size) {
            p.over(sp, x, y, o);
        }
    }
}

/// booth::apply_item as passes.
fn booth_item(p: &mut Plan<'_, '_>, it: &crate::booth::BoothItem) {
    let (w, h) = p.size;
    if w == 0 || h == 0 || it.fx == BoothFx::None {
        return;
    }
    if BoothFx::LOOKS.contains(&it.fx) {
        if it.amount > 100.0 {
            // past full strength: pushed further from the picture
            p.at_strength(it.amount, |p| booth(p, it.fx, 100.0, it.cx, it.cy));
        } else {
            booth(p, it.fx, it.amount, it.cx, it.cy);
        }
        return;
    }
    if it.fx == BoothFx::Mirror {
        let wf = w as f32;
        let cx = it.cx / 100.0 * wf;
        p.op(48, &[[cx.clamp(1.0, (wf - 1.0).max(1.0)), if it.reverse { 1.0 } else { 0.0 }, 0.0, 0.0]]);
        return;
    }
    let Some((kind, a, b)) = crate::booth::distort_params(it, w, h) else { return };
    p.op_sub(24, kind as f32, &[a, b]);
    if it.fx == BoothFx::FishEye {
        p.op(25, &[[a[0], a[1], a[2], (it.amount / 100.0).clamp(0.0, 1.0)]]);
    }
}

/// effects::camera: brackets, a cross, the blinking dot and REC.
fn camera(p: &mut Plan<'_, '_>, fonts: &[FontEntry], frame_idx: usize) {
    let (w, h) = p.size;
    let m = (w.min(h) as f32 * 0.06) as i32;
    let len = (w.min(h) as f32 * 0.12) as i32;
    let t = ((w.min(h) as f32) * 0.012).max(2.0) as i32;
    let (w, h) = (w as i32, h as i32);
    let mut rects: Vec<[f32; 4]> = Vec::new();
    for (cx, cy, sx, sy) in [(m, m, 1, 1), (w - m, m, -1, 1), (m, h - m, 1, -1), (w - m, h - m, -1, -1)] {
        let x0 = if sx > 0 { cx } else { cx - len };
        let y0 = if sy > 0 { cy } else { cy - t };
        rects.push([x0 as f32, y0 as f32, len as f32, t as f32]);
        let x1 = if sx > 0 { cx } else { cx - t };
        let y1 = if sy > 0 { cy } else { cy - len };
        rects.push([x1 as f32, y1 as f32, t as f32, len as f32]);
    }
    let cl = len / 3;
    rects.push([(w / 2 - cl) as f32, (h / 2 - t / 2) as f32, (cl * 2) as f32, t as f32]);
    rects.push([(w / 2 - t / 2) as f32, (h / 2 - cl) as f32, t as f32, (cl * 2) as f32]);
    let size = (h as f32 * 0.05).max(10.0);
    let dot_r = (size * 0.35) as i32;
    let dx = m + len / 2 + dot_r;
    let dy = m + len / 2 + dot_r;
    let on = (frame_idx / 8) % 2 == 0;
    let mut u = vec![[dx as f32, dy as f32, dot_r as f32, if on { 1.0 } else { 0.0 }]];
    u.extend(rects);
    p.op(30, &u);
    p.text(fonts, "REC", dx + dot_r * 2, dy - (size * 0.6) as i32, size);
}

/// booth::apply as passes.
fn booth(p: &mut Plan<'_, '_>, fx: BoothFx, amount: f32, cx: f32, cy: f32) {
    let (w, h) = p.size;
    if w == 0 || h == 0 || fx == BoothFx::None {
        return;
    }
    let a = (amount / 100.0).clamp(0.0, 1.0);
    let (wf, hf) = (w as f32, h as f32);
    let (cx, cy) = (cx / 100.0 * wf, cy / 100.0 * hf);
    let rad = (w.min(h) as f32 / 2.0).max(1.0);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    match fx {
        BoothFx::None => {}
        BoothFx::Glow => {
            let (sw, sh) = ((w / 2).max(1), (h / 2).max(1));
            p.resize_tri(GpuIn::Cur, (w, h), (sw, sh), T_SMALL);
            p.blur_all(GpuIn::Tmp(T_SMALL), Some(T_SMALL), (sw, sh), (sw.min(sh) as f32 * 0.035).max(0.5));
            p.resize_tri(GpuIn::Tmp(T_SMALL), (sw, sh), (w, h), T_BIG);
            p.raw(42, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_BIG)), None], None, (w, h), false, &[[a, 0.0, 0.0, 0.0]]);
        }
        BoothFx::Comic => {
            p.blur_all(GpuIn::Cur, None, (w, h), 1.2);
            p.op(43, &[[lerp(0.9, 0.25, a), 0.0, 0.0, 0.0]]);
        }
        BoothFx::Pencil => {
            p.blur_all(GpuIn::Cur, Some(T_BLUR), (w, h), 0.8);
            let period = (w.min(h) as f32 / 60.0).clamp(3.0, 9.0);
            p.raw(44, 0.0, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_BLUR)), Some(GpuIn::Noise(1))], None, (w, h), false, &[[period, a, 0.0, 0.0]]);
        }
        BoothFx::Thermal => p.op(45, &[[a, 0.0, 0.0, 0.0]]),
        BoothFx::XRay => p.op(46, &[[a, 0.0, 0.0, 0.0]]),
        BoothFx::PopArt => {
            let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
            p.resize_tri(GpuIn::Cur, (w, h), (hw, hh), T_SMALL);
            let len = (hw * hh) as usize;
            let spread = 0.08 * (a - 0.5);
            let k = |f: f32| ((len - 1) as f32 * f) as usize;
            p.passes.push(GpuPass { kind: PassKind::Hist, size: (hw, hh), out: None, float: false, ins: [Some(GpuIn::Tmp(T_SMALL)), None, None], u: vec![[k(0.36 - spread) as f32, k(0.68 + spread) as f32, 0.0, 0.0]] });
            p.raw(47, 0.0, [Some(GpuIn::Tmp(T_SMALL)), None, None], None, (w, h), false, &[[hw as f32, hh as f32, 0.0, 0.0]]);
        }
        BoothFx::Bulge | BoothFx::Dent | BoothFx::FishEye => {
            let (r0, k) = match fx {
                BoothFx::Bulge => (rad * 1.1, a * 1.65),
                BoothFx::Dent => (rad * 1.1, -(a * 0.8)),
                _ => (w.max(h) as f32 * 0.5, a * 1.6),
            };
            p.op_sub(24, 3.0, &[[cx, cy, r0, k]]);
            if fx == BoothFx::FishEye {
                p.op(25, &[[cx, cy, r0, a]]);
            }
        }
        BoothFx::Twirl => p.op_sub(24, 4.0, &[[cx, cy, rad * 1.2, (a * 540.0).to_radians()]]),
        BoothFx::Squeeze | BoothFx::Stretch => {
            let pw = if fx == BoothFx::Squeeze { 1.0 / (1.0 + a * 2.0) } else { 1.0 + a * 2.0 };
            p.op_sub(24, 5.0, &[[cx, pw, wf, 0.0]]);
        }
        BoothFx::Mirror => p.op(48, &[[cx.clamp(1.0, (wf - 1.0).max(1.0)), 0.0, 0.0, 0.0]]),
        BoothFx::Tunnel => p.op_sub(24, 6.0, &[[cx, cy, rad * lerp(0.75, 0.35, a), lerp(1.0, 0.04, a)]]),
        BoothFx::Kaleidoscope => {
            let n = 4 + (a * 8.0).round() as i32;
            let seg = TAU / n as f32;
            let reach = ((wf - cx).max(cx) * 0.95).max(1.0);
            p.op_sub(24, 7.0, &[[cx, cy, seg, reach]]);
        }
    }
}

/// motion::looped as passes.
#[allow(clippy::too_many_arguments)]
fn looped(p: &mut Plan<'_, '_>, fx: &[LoopFx], idx: usize, n: usize, amount: f32, cycles: u32, transparent: bool, reverse: bool) {
    if fx.is_empty() || n == 0 {
        return;
    }
    use crate::motion::hash;
    let k = (amount / 100.0).clamp(0.0, crate::motion::MAX_LOOP);
    let (w, h) = p.wf();
    let l = crate::motion::loop_place(fx, idx, n, amount, cycles, reverse, w, h);
    if l.moves() {
        p.place_xy(l.sx, l.sy, l.angle, l.dx, l.dy, transparent);
    }
    if fx.contains(&LoopFx::Rainbow) {
        let (s, c) = l.hue.to_radians().sin_cos();
        p.op(
            40,
            &[
                [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928, 0.0],
                [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283, 0.0],
                [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072, 0.0],
            ],
        );
    }
    if fx.contains(&LoopFx::Glitch) && hash(idx as u64 * 7 + 3) < 0.35 * k.min(1.0) + 0.05 {
        let (wu, hu) = p.size;
        let seed = idx as u64;
        let split = ((wu.min(hu) as f32) * 0.02 * k).round().max(1.0);
        let mut u = vec![[split, 0.0, 0.0, 0.0]];
        for b in 0..4u64 {
            let y0 = (hash(seed * 31 + b) * hu as f32) as u32;
            let bh = ((hash(seed * 37 + b) * 0.08 + 0.01) * hu as f32) as u32;
            let shift = ((hash(seed * 41 + b) - 0.5) * wu as f32 * 0.15 * k) as i32;
            u.push([y0 as f32, (y0 + bh).min(hu) as f32, shift as f32, 0.0]);
        }
        p.op(41, &u);
    }
    if l.hide > 0.0 {
        transition(p, Transition::Fade, 1.0 - l.hide, transparent);
    }
}

/// motion::transition as passes.
fn transition(p: &mut Plan<'_, '_>, kind: Transition, pr: f32, transparent: bool) {
    use crate::motion::{ease_in_out, ease_out_bounce, ease_out_cubic};
    let pr = pr.clamp(0.0, 1.0);
    if pr >= 1.0 || kind == Transition::None {
        return;
    }
    let (w, h) = p.wf();
    let soft = (w.min(h) * 0.04).max(2.0);
    let z = [0.0; 4];
    match kind {
        Transition::None => {}
        Transition::Fade => p.mask(0, [pr, 0.0, 0.0, 0.0], z, transparent),
        Transition::WipeLeft => p.mask(1, [pr * (w + soft), soft, w, h], z, transparent),
        Transition::WipeRight => p.mask(2, [pr * (w + soft), soft, w, h], z, transparent),
        Transition::WipeDown => p.mask(3, [pr * (h + soft), soft, w, h], z, transparent),
        Transition::WipeUp => p.mask(4, [pr * (h + soft), soft, w, h], z, transparent),
        Transition::Iris => {
            let r = ease_in_out(pr) * ((w * w + h * h).sqrt() / 2.0 + soft);
            p.mask(5, [r, soft, 0.0, 0.0], [w / 2.0, h / 2.0, 0.0, 0.0], transparent);
        }
        Transition::Blinds => {
            let slat = (h / 8.0).max(4.0);
            p.mask(6, [ease_in_out(pr) * slat, slat, 0.0, 0.0], z, transparent);
        }
        Transition::Zoom => p.place(ease_out_cubic(pr), 0.0, 0.0, 0.0, transparent),
        Transition::Spin => {
            let e = ease_out_cubic(pr);
            p.place(e, (1.0 - e) * 360.0, 0.0, 0.0, transparent);
        }
        Transition::SlideLeft => p.place(1.0, 0.0, -(1.0 - ease_out_cubic(pr)) * w, 0.0, transparent),
        Transition::SlideRight => p.place(1.0, 0.0, (1.0 - ease_out_cubic(pr)) * w, 0.0, transparent),
        Transition::SlideUp => p.place(1.0, 0.0, 0.0, (1.0 - ease_out_cubic(pr)) * h, transparent),
        Transition::SlideDown => p.place(1.0, 0.0, 0.0, -(1.0 - ease_out_cubic(pr)) * h, transparent),
        Transition::Drop => p.place(1.0, 0.0, 0.0, -(1.0 - ease_out_bounce(pr)) * h, transparent),
        Transition::Pixelate => {
            let block = ((1.0 - pr) * w.min(h) / 6.0).round().max(1.0) as u32;
            if block > 1 {
                p.pixelate(block);
            }
            p.mask(0, [(pr * 3.0).min(1.0), 0.0, 0.0, 0.0], z, transparent);
        }
    }
}

/// effects::censor_obb as passes.
pub fn censor<'a>(p: &mut Plan<'a, '_>, b: crate::geo::Obb, c: &CensorSettings) {
    let (x0, y0, x1, y1) = b.bounds();
    let (bx, by) = (x0.floor() as i32, y0.floor() as i32);
    let (bw, bh) = ((x1.ceil() as i32 - bx).max(0), (y1.ceil() as i32 - by).max(0));
    if bw == 0 || bh == 0 {
        return;
    }
    let (iw, ih) = (p.size.0 as i32, p.size.1 as i32);
    let rx0 = bx.clamp(0, iw);
    let ry0 = by.clamp(0, ih);
    let rx1 = (bx + bw).clamp(0, iw);
    let ry1 = (by + bh).clamp(0, ih);
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }
    let turned = (b.angle % 90.0).abs() >= 0.01;
    let (s, co) = (-b.angle).to_radians().sin_cos();
    let region = [rx0 as f32, ry0 as f32, rx1 as f32, ry1 as f32];
    let obb = [b.cx, b.cy, b.w / 2.0, b.h / 2.0];
    let col = [c.color[0] as f32, c.color[1] as f32, c.color[2] as f32, 255.0];
    let size = p.size;
    let (mode, sq) = match c.method {
        CensorMethod::Solid => (0.0, 1),
        CensorMethod::Blur => {
            p.blur(GpuIn::Cur, Some(T_REGION), size, c.sigma.max(0.5), region);
            (1.0, 1)
        }
        CensorMethod::Pixelate => {
            let sq = c.square.max(2);
            p.blocks([rx0 as u32, ry0 as u32, (rx1 - rx0) as u32, (ry1 - ry0) as u32], sq, T_REGION);
            (2.0, sq)
        }
    };
    p.raw(51, mode, [Some(GpuIn::Cur), Some(GpuIn::Tmp(T_REGION)), None], None, size, false, &[region, obb, [s, co, sq as f32, if turned { 1.0 } else { 0.0 }], col]);
}

/// effects::shield as passes.
pub fn shield(p: &mut Plan<'_, '_>, s: &ShieldSettings, frame: usize) {
    let z = if s.per_frame { s.seed.wrapping_add(frame as u32 * 7919) } else { s.seed };
    if s.jitter {
        p.op_sub(24, 8.0, &[[(z % 97) as f32, (z % 89) as f32, 0.0, 0.0]]);
    }
    let mut u = vec![[s.strength.max(0.0), if s.pattern { 1.0 } else { 0.0 }, if s.noise { 1.0 } else { 0.0 }, 0.0], [(z >> 16) as f32, (z & 0xffff) as f32, 0.0, 0.0]];
    for k in 0..9u32 {
        let r = crate::effects::hash3(z, k, 99);
        let ang = (r % 3600) as f32 / 3600.0 * TAU;
        let period = 2.2 + ((r >> 12) % 100) as f32 / 40.0;
        let f = TAU / period;
        u.push([ang.cos() * f, ang.sin() * f, ((r >> 20) % 628) as f32 / 100.0, 0.0]);
    }
    p.op(49, &u);
}
