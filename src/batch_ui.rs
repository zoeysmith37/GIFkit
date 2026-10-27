//! Platform targets (size limit plus frame size) and Batch: one set of
//! edits and output settings applied to many files.

use super::*;
use crate::presets::{Preset, PresetLayer};
use std::path::Path;

/// Where a file is headed: its upload limit (with a little headroom) and,
/// for emoji, the largest side it may have.
pub(super) struct Platform {
    pub name: &'static str,
    pub limit: f32,
    pub mb: bool,
    pub side: Option<u32>,
    /// the site's own limit, as the menu shows it
    pub note: &'static str,
}

pub(super) const PLATFORMS: [Platform; 6] = [
    Platform { name: "Discord", limit: 9.5, mb: true, side: None, note: "10 MB" },
    Platform { name: "Discord Emoji", limit: 250.0, mb: false, side: Some(128), note: "256 KB · 128 px" },
    Platform { name: "Slack Emoji", limit: 125.0, mb: false, side: Some(128), note: "128 KB · 128 px" },
    Platform { name: "Twitch Emote", limit: 950.0, mb: false, side: Some(112), note: "1 MB · 112 px" },
    Platform { name: "X", limit: 14.0, mb: true, side: None, note: "15 MB" },
    Platform { name: "Email", limit: 4.5, mb: true, side: None, note: "5 MB" },
];

/// Shrink `t` so a `w`×`h` frame (before any resize) fits in a square of
/// `side`. Never makes it bigger.
pub(super) fn fit_side(t: &mut Transform, (w, h): (u32, u32), side: u32) {
    let (w, h) = if t.crop_on && t.crop[2] > 0 && t.crop[3] > 0 { (t.crop[2], t.crop[3]) } else { (w, h) };
    if w.max(h) <= side {
        return;
    }
    t.resize_on = true;
    t.rs_pct = 0.0;
    (t.rs_w, t.rs_h) = if w >= h { (side, 0) } else { (0, side) };
}

/// A preset with its fonts and images looked up, ready for any file.
#[derive(Clone)]
pub(super) struct ReadyPreset {
    pub pr: Preset,
    pub images: HashMap<PathBuf, Arc<OverlaySource>>,
    pub missing: Vec<String>,
}

/// The preset's frame edits and layers for a `w`×`h` animation of `n`
/// frames: positions scale from the size it was made on.
pub(super) fn preset_parts(rp: &ReadyPreset, (w, h): (u32, u32), n: usize) -> (Option<Transform>, Vec<(LayerKind, bool)>) {
    let pr = &rp.pr;
    let (sw, sh) = (pr.src_size.0.max(1) as f32, pr.src_size.1.max(1) as f32);
    let (fx, fy) = if pr.src_size.0 == 0 { (1.0, 1.0) } else { (w as f32 / sw, h as f32 / sh) };
    let fs = fx.min(fy);
    let sc = |v: i32, f: f32| (v as f32 * f).round() as i32;
    let t = pr.transform.clone().map(|mut t| {
        t.crop = [(t.crop[0] as f32 * fx) as u32, (t.crop[1] as f32 * fy) as u32, (t.crop[2] as f32 * fx) as u32, (t.crop[3] as f32 * fy) as u32];
        t
    });
    let mut out = Vec::new();
    for l in pr.layers.iter().flatten() {
        let kind = match l.clone() {
            PresetLayer::Effects { visible, fx: e } => (LayerKind::Effects(Box::new(e)), visible),
            PresetLayer::Text { visible, mut text, .. } => {
                text.x = sc(text.x, fx);
                text.y = sc(text.y, fy);
                text.size *= fs;
                (LayerKind::Text(text), visible)
            }
            PresetLayer::Censor { visible, mut censor } => {
                censor.x = sc(censor.x, fx);
                censor.y = sc(censor.y, fy);
                censor.w = (censor.w as f32 * fx) as u32;
                censor.h = (censor.h as f32 * fy) as u32;
                (LayerKind::Censor(censor), visible)
            }
            PresetLayer::Image { visible, path, mut placement, opacity } => {
                let Some(src) = rp.images.get(&path) else { continue };
                placement.x = sc(placement.x, fx);
                placement.y = sc(placement.y, fy);
                placement.w = (placement.w as f32 * fs).max(1.0) as u32;
                placement.h = (placement.h as f32 * fs).max(1.0) as u32;
                (LayerKind::Overlay(OverlayLayer { src: src.clone(), placements: vec![placement; n.max(1)], opacity }), visible)
            }
            PresetLayer::Watermark { visible, watermark } => (LayerKind::Watermark(Box::new(watermark)), visible),
            PresetLayer::Shield { visible, shield } => (LayerKind::Shield(shield), visible),
            PresetLayer::Shape { visible, mut shape } => {
                shape.x = sc(shape.x, fx);
                shape.y = sc(shape.y, fy);
                shape.w = ((shape.w as f32 * fx) as u32).max(1);
                shape.h = ((shape.h as f32 * fy) as u32).max(1);
                shape.stroke_w *= fs;
                (LayerKind::Shape(Box::new(shape)), visible)
            }
            PresetLayer::Markup { visible, mut markup } => {
                for st in &mut markup.strokes {
                    for pt in &mut st.points {
                        pt[0] *= fx;
                        pt[1] *= fy;
                    }
                    st.width *= fs;
                }
                (LayerKind::Markup(markup), visible)
            }
            PresetLayer::Emoji { visible, mut emoji } => {
                emoji.x = sc(emoji.x, fx);
                emoji.y = sc(emoji.y, fy);
                emoji.w = ((emoji.w as f32 * fs) as u32).max(1);
                emoji.h = ((emoji.h as f32 * fs) as u32).max(1);
                (LayerKind::Emoji(Box::new(emoji)), visible)
            }
        };
        out.push(kind);
    }
    (t, out)
}

/// What Batch puts on each file.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum BatchEdits {
    /// the open file's edits (frame edits, layers, compression)
    Current,
    /// nothing: only the output settings
    None,
    /// a saved preset, by its place in the list
    Saved(usize),
}

pub(super) struct BatchState {
    pub open: bool,
    pub files: Vec<PathBuf>,
    pub edits: BatchEdits,
    pub fmt: ExportFormat,
    /// a platform from PLATFORMS: its limit and frame size
    pub platform: Option<usize>,
    /// None = next to each file
    pub out: Option<PathBuf>,
}

impl Default for BatchState {
    fn default() -> Self {
        Self { open: false, files: Vec::new(), edits: BatchEdits::None, fmt: ExportFormat::Gif, platform: None, out: None }
    }
}

/// Where a batch file is written: `name-gifkit.ext`, numbered when taken.
fn batch_out_path(src: &Path, out: Option<&Path>, ext: &str) -> PathBuf {
    let dir = out.map(|p| p.to_path_buf()).or_else(|| src.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let mut p = dir.join(format!("{stem}-gifkit.{ext}"));
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-gifkit-{k}.{ext}"));
        k += 1;
    }
    p
}

/// Batch settings, made once and used on every file.
pub(super) struct BatchJob {
    pub rp: Option<ReadyPreset>,
    pub out: Option<PathBuf>,
    pub fmt: ExportFormat,
    pub platform: Option<usize>,
    pub gif: GifOptions,
    pub noai: bool,
    pub webp: io::WebpOptions,
    pub avif: u8,
    pub keep_size: bool,
    pub fonts: Vec<FontEntry>,
}

impl BatchJob {
    /// Load `src`, edit it, fit it and save it: where it went, its size and
    /// whether it got under the limit.
    pub fn one(&self, src: &Path) -> anyhow::Result<(PathBuf, u64, bool)> {
        let a = io::load_animation(src)?;
        let dims = (a.width(), a.height());
        let mut stack = Stack::default();
        if let Some(rp) = &self.rp {
            let (t, layers) = preset_parts(rp, dims, a.len());
            if let Some(t) = t {
                stack.transform = t;
            }
            for (k, vis) in layers {
                let id = stack.push(k);
                if let Some(li) = stack.index_of(id) {
                    stack.layers[li].visible = vis;
                }
            }
        }
        let pf = self.platform.and_then(|i| PLATFORMS.get(i));
        if let Some(side) = pf.and_then(|p| p.side) {
            fit_side(&mut stack.transform, dims, side);
        }
        let ext = match self.fmt {
            ExportFormat::Webp => "webp",
            ExportFormat::Avif => "avif",
            _ => "gif",
        };
        let path = batch_out_path(src, self.out.as_deref(), ext);
        let gif = GifOptions { noai_tag: self.noai, ..self.gif.clone() };
        let tf = match self.fmt {
            ExportFormat::Webp => crate::target::TargetFormat::Webp(self.webp),
            ExportFormat::Avif => crate::target::TargetFormat::Avif(self.avif),
            _ => crate::target::TargetFormat::Gif(gif.clone()),
        };
        let (data, reached) = match pf {
            Some(pf) => {
                let max = (pf.limit as f64 * if pf.mb { 1_048_576.0 } else { 1024.0 }) as u64;
                let trials = AtomicUsize::new(0);
                let fit = crate::target::fit_with(&tf, max, !self.keep_size, &trials, |sc| {
                    let mut st = stack.clone();
                    apply_fit_scale(&mut st.transform, sc);
                    st.flatten(&a, &self.fonts, None)
                })?;
                (fit.bytes, fit.reached)
            }
            None => {
                let flat = stack.flatten(&a, &self.fonts, None);
                let b = match self.fmt {
                    ExportFormat::Webp => io::encode_webp(&flat, &self.webp)?,
                    ExportFormat::Avif => io::encode_avif(&flat, self.avif)?,
                    _ => crate::encode::encode_gif(&flat, &gif, None)?,
                };
                (b, true)
            }
        };
        let data = match self.fmt {
            ExportFormat::Webp if self.noai => crate::noai::tag_webp(data),
            ExportFormat::Avif if self.noai => crate::noai::tag_avif(data),
            _ => data,
        };
        std::fs::write(&path, &data)?;
        Ok((path, data.len() as u64, reached))
    }
}

impl GifKitApp {
    /// Look up a preset's fonts and load its images, once, before using it.
    pub(super) fn ready_preset(&mut self, pr: &Preset) -> ReadyPreset {
        let mut pr = pr.clone();
        let mut images = HashMap::new();
        let mut missing = Vec::new();
        for l in pr.layers.iter_mut().flatten() {
            match l {
                PresetLayer::Text { text, font, .. } => text.font_idx = self.resolve_font(font),
                PresetLayer::Watermark { watermark, .. } => {
                    watermark.font_idx = self.resolve_font(&watermark.font_key);
                    if let Some(p) = &watermark.image_path {
                        watermark.image = io::load_still(p).ok().map(Arc::new);
                    }
                }
                PresetLayer::Image { path, .. } if !images.contains_key(path) => match io::load_animation(path) {
                    Ok(a) => {
                        let src = OverlaySource { name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), path: Some(path.clone()), delays: a.frames.iter().map(|f| f.delay_cs).collect(), frames: a.frames.iter().map(|f| (*f.image.get()).clone()).collect() };
                        images.insert(path.clone(), Arc::new(src));
                    }
                    Err(_) => missing.push(path.display().to_string()),
                },
                _ => {}
            }
        }
        ReadyPreset { pr, images, missing }
    }

    /// Use a platform's limit and frame size on the open file.
    pub(super) fn use_platform(&mut self, i: usize) {
        let Some(pf) = PLATFORMS.get(i) else { return };
        self.target_val = pf.limit;
        self.target_mb = pf.mb;
        let lim = if pf.mb { format!("{} MiB", pf.limit) } else { format!("{} KiB", pf.limit) };
        match pf.side {
            Some(side) => {
                let dims = self.dims();
                fit_side(&mut self.stack.transform, dims, side);
                self.info(format!("{}: {lim} and {side} px. Press Find Settings.", pf.name));
            }
            None => self.info(format!("{}: {lim}. Press Find Settings.", pf.name)),
        }
    }

    pub(super) fn batch_window(&mut self, ctx: &egui::Context) {
        if !self.batch.open {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut go = false;
        let idle = !self.busy();
        let has = self.base.is_some();
        let names: Vec<String> = self.presets.iter().map(|p| p.name.clone()).collect();
        dialog(ctx, "batch", "Batch", vec2(500.0, 470.0), false, &mut open, |ui| {
            ui.set_width(470.0);
            let b = &mut self.batch;
            group(ui, "Files", |ui| {
                egui::Frame::new().fill(pal.view).stroke(Stroke::new(1.0, mix(pal.view, pal.fg, 0.2))).corner_radius(3.0).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(120.0).min_scrolled_height(120.0).auto_shrink([false, false]).show(ui, |ui| {
                        if b.files.is_empty() {
                            hint(ui, "Add files to work on");
                        }
                        let mut drop = None;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, f) in b.files.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.set_min_height(22.0);
                                ui.label(f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()).on_hover_text(f.display().to_string());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new(RichText::new("✕").size(11.0).color(pal.fg_muted)).frame(false)).on_hover_text("Leave out").clicked() {
                                        drop = Some(i);
                                    }
                                });
                            });
                        }
                        if let Some(i) = drop {
                            b.files.remove(i);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Add Files…").clicked() {
                        let mut d = rfd::FileDialog::new().add_filter("Animations", &["gif", "webp", "png", "apng", "avif", "mp4", "webm", "mov", "mkv"]);
                        if let Some(dir) = &self.last_dir {
                            d = d.set_directory(dir);
                        }
                        for f in d.pick_files().unwrap_or_default() {
                            if !b.files.contains(&f) {
                                b.files.push(f);
                            }
                        }
                    }
                    if ui.add_enabled(!b.files.is_empty(), egui::Button::new("Clear")).clicked() {
                        b.files.clear();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let n = b.files.len();
                        hint(ui, &if n == 1 { "1 file".to_string() } else { format!("{n} files") });
                    });
                });
            });
            group(ui, "Output", |ui| {
                form_row(ui, "Edits", |ui| {
                    let label = |e: BatchEdits| match e {
                        BatchEdits::Current => "This file's edits".to_string(),
                        BatchEdits::None => "None".to_string(),
                        BatchEdits::Saved(i) => names.get(i).cloned().unwrap_or_default(),
                    };
                    egui::ComboBox::from_id_salt("batch_edits").width(ui.available_width()).selected_text(label(b.edits)).show_ui(ui, |ui| {
                        ui.add_enabled_ui(has, |ui| ui.selectable_value(&mut b.edits, BatchEdits::Current, label(BatchEdits::Current)));
                        ui.selectable_value(&mut b.edits, BatchEdits::None, "None");
                        for i in 0..names.len() {
                            ui.selectable_value(&mut b.edits, BatchEdits::Saved(i), label(BatchEdits::Saved(i)));
                        }
                    });
                });
                form_row(ui, "Format", |ui| seg_row(ui, &mut b.fmt, &[(ExportFormat::Gif, "GIF"), (ExportFormat::Webp, "WebP"), (ExportFormat::Avif, "AVIF")]));
                form_row(ui, "For", |ui| {
                    let sel = b.platform.and_then(|i| PLATFORMS.get(i)).map(|p| p.name).unwrap_or("No limit");
                    egui::ComboBox::from_id_salt("batch_platform").width(ui.available_width()).selected_text(sel).show_ui(ui, |ui| {
                        ui.selectable_value(&mut b.platform, None, "No limit");
                        for (i, pf) in PLATFORMS.iter().enumerate() {
                            ui.selectable_value(&mut b.platform, Some(i), pf.name);
                        }
                    });
                });
                form_row(ui, "Save to", |ui| {
                    ui.horizontal(|ui| {
                        let t = b.out.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Next to each file".into());
                        ui.label(elide(&t, 34)).on_hover_text(t);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if b.out.is_some() && ui.button("Reset").clicked() {
                                b.out = None;
                            }
                            if ui.button("Choose…").clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    b.out = Some(d);
                                }
                            }
                        });
                    });
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = b.files.len();
                    go = ui.add_enabled(idle && n > 0, egui::Button::new(if n == 1 { "Run on 1 File".to_string() } else { format!("Run on {n} Files") }).stroke(Stroke::new(1.0, pal.highlight)).min_size(vec2(120.0, 0.0))).clicked();
                });
            });
        });
        if !open {
            self.batch.open = false;
        }
        if go {
            self.run_batch(ctx);
        }
    }

    fn run_batch(&mut self, ctx: &egui::Context) {
        let pr = match self.batch.edits {
            BatchEdits::Current if self.base.is_some() => {
                let keep = self.preset_inc;
                self.preset_inc = (true, true, true);
                let pr = self.make_preset("This file".into());
                self.preset_inc = keep;
                Some(pr)
            }
            BatchEdits::Saved(i) => self.presets.get(i).cloned(),
            _ => None,
        };
        let job = BatchJob {
            rp: pr.map(|pr| self.ready_preset(&pr)),
            out: self.batch.out.clone(),
            fmt: self.batch.fmt,
            platform: self.batch.platform,
            gif: GifOptions::default(),
            noai: self.gif_opts.noai_tag,
            webp: self.webp_opts,
            avif: self.avif_quality,
            keep_size: self.fit_keep_size,
            fonts: self.fonts.clone(),
        };
        let job = BatchJob { gif: job.rp.as_ref().and_then(|r| r.pr.gif.clone()).unwrap_or_else(|| self.gif_opts.clone()), ..job };
        let files = self.batch.files.clone();
        let out = job.out.clone();
        let total = files.len();
        if let Some(r) = &job.rp {
            if !r.missing.is_empty() {
                self.error(format!("Missing images, left out: {}", r.missing.join(", ")));
            }
        }
        self.batch.open = false;
        self.spawn(ctx, &format!("Batch: {total} file{}", if total == 1 { "" } else { "s" }), total, move |p| {
            let mut done = 0;
            let mut bytes = 0u64;
            let mut fails: Vec<String> = Vec::new();
            let mut missed = 0;
            let mut last_dir = None;
            for (i, src) in files.iter().enumerate() {
                p.store(i, std::sync::atomic::Ordering::Relaxed);
                let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let r = job.one(src);
                match r {
                    Ok((path, n, reached)) => {
                        done += 1;
                        bytes += n;
                        if !reached {
                            missed += 1;
                        }
                        last_dir = path.parent().map(|d| d.display().to_string());
                    }
                    Err(e) => fails.push(format!("{name}: {e:#}")),
                }
            }
            p.store(total, std::sync::atomic::Ordering::Relaxed);
            let mut msg = format!("Batch saved {done} of {total} file{} ({})", if total == 1 { "" } else { "s" }, fmt_size(bytes));
            if let Some(d) = last_dir.filter(|_| out.is_some()) {
                msg += &format!(" to {d}");
            }
            if missed > 0 {
                msg += &format!(". {missed} couldn't get under the limit");
            }
            if fails.is_empty() {
                JobOutput::Info(Ok(msg))
            } else {
                JobOutput::Info(Err(anyhow::anyhow!("{msg}. Failed: {}", fails.join("; "))))
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_side_only_shrinks() {
        let mut t = Transform::default();
        fit_side(&mut t, (100, 80), 128);
        assert!(!t.resize_on);
        fit_side(&mut t, (400, 200), 128);
        assert!(t.resize_on && t.rs_w == 128 && t.rs_h == 0);
        let mut t = Transform::default();
        fit_side(&mut t, (200, 400), 112);
        assert_eq!((t.rs_w, t.rs_h), (0, 112));
    }

    #[test]
    fn batch_runs_a_preset_to_an_emoji() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test.gif");
        let d = std::env::temp_dir().join(format!("gifkit-batch-run-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = io::load_animation(&src).unwrap();
        // a preset made on a frame twice the size: positions scale down
        let mut text = crate::text::Annotation::default();
        text.text = "hi".into();
        text.x = 40;
        let pr = Preset { name: "t".into(), src_size: (a.width() * 2, a.height() * 2), transform: None, layers: Some(vec![PresetLayer::Text { visible: true, text, font: None }]), gif: None };
        let rp = ReadyPreset { pr, images: HashMap::new(), missing: Vec::new() };
        let (_, layers) = preset_parts(&rp, (a.width(), a.height()), a.len());
        match &layers[0].0 {
            LayerKind::Text(t) => assert_eq!(t.x, 20),
            _ => panic!("not text"),
        }
        let slack = PLATFORMS.iter().position(|p| p.name == "Slack Emoji").unwrap();
        let job = BatchJob { rp: Some(rp), out: Some(d.clone()), fmt: ExportFormat::Gif, platform: Some(slack), gif: GifOptions::default(), noai: true, webp: Default::default(), avif: 60, keep_size: false, fonts: crate::text::builtin_fonts() };
        let (path, n, reached) = job.one(&src).unwrap();
        assert!(reached, "{n} bytes");
        assert!(n <= 125 * 1024);
        let out = io::load_animation(&path).unwrap();
        assert!(out.width().max(out.height()) <= 128, "{}x{}", out.width(), out.height());
        // a second run doesn't overwrite the first
        let (p2, _, _) = BatchJob { platform: None, rp: None, ..job }.one(&src).unwrap();
        assert_ne!(p2, path);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn batch_names_dont_overwrite() {
        let d = std::env::temp_dir().join(format!("gifkit-batch-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("cat.gif");
        let a = batch_out_path(&src, None, "gif");
        assert_eq!(a, d.join("cat-gifkit.gif"));
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(batch_out_path(&src, Some(&d), "gif"), d.join("cat-gifkit-2.gif"));
        std::fs::remove_dir_all(&d).ok();
    }
}
