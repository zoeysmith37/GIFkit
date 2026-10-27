//! Random stress runs over the whole edit pipeline: frame edits, every layer
//! kind with odd settings, timing, previews and encoders. Run with
//! `cargo test --release fuzz -- --ignored --nocapture` (FUZZ_N, FUZZ_SEED).

use crate::anim::{Animation, Frame};
use crate::stack::{LayerKind, Stack};
use crate::timeline::{Clip, Key};
use image::{Rgba, RgbaImage};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
    fn chance(&mut self, p: f64) -> bool {
        self.f() < p
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<T: Clone>(&mut self, v: &[T]) -> T {
        v[self.below(v.len())].clone()
    }
}

const VARIANTS: &[&str] = &["ActualSize","Apng","Arrow","Avif","Bicubic","Bilinear","Blinds","Blur","Bottom","BottomLeft","BottomRight","Bubble","Bulge","Burst","Censor","Center","CenterCrop","Comic","Dark","DelayAll","DeleteObject","Dent","Drop","Effects","Ellipse","Emoji","Export","Fade","Float","Heartbeat","Blink","Squash","Hold","Linear","Smooth","FishEye","Fit","FitPad","Fps","FullScreen","Gif","Glitch","Glow","Handbook","Heart","Heavy","Highlighter","History","Hop","Iris","Kaleidoscope","Lanczos3","Left","Light","Line","Markup","Mirror","Mp4","Nearest","New","NextFrame","None","Open","Overlay","Pen","Pencil","Percent","Pixelate","PlayPause","Png","PopArt","PrevFrame","Pulse","Quit","Rainbow","Rect","Redo","Right","Rounded","Save","SaveAs","Shake","Shape","Shield","SlideDown","SlideLeft","SlideRight","SlideUp","Solid","Speech","Spin","SplitView","Squeeze","Star","Stretch","System","Text","Thermal","Thought","TlDelete","TlKey","TlRazor","TlSplit","Top","TopLeft","TopRight","Triangle","Tunnel","Twirl","Undo","Watermark","WebmAlpha","WipeDown","WipeLeft","WipeRight","WipeUp","Wobble","XRay","Zoom","ZoomIn","ZoomOut",];
const TEXTS: &[&str] = &["", " ", "Hello", "a\nb\n\nc", "🔥🔥 emoji 👍🏽", "مرحبا بالعالم", "日本語のテキスト", "x", "averyveryveryverylongwordwithoutanyspacesatallthatkeepsgoing and then some more words to wrap around the canvas edge"];

fn perturb(v: &mut Value, r: &mut Rng, p: f64, key: &str) {
    match v {
        Value::Bool(b) => {
            if r.chance(p) {
                *b = !*b
            }
        }
        Value::Number(n) => {
            if r.chance(p) {
                let x = n.as_f64().unwrap_or(0.0);
                let c = [0.0, 1.0, -1.0, 2.0, 3.0, 7.0, x * 3.0, -x, x * 0.5, 100.0, 255.0, 999.0, 0.001, x + 1.0];
                let y: f64 = r.pick(&c);
                *v = if n.is_u64() { Value::from(y.abs().round() as u64) } else if n.is_i64() { Value::from(y.round() as i64) } else { serde_json::Number::from_f64(y).map(Value::Number).unwrap_or(Value::from(0)) };
            }
        }
        Value::String(s) => {
            if r.chance(p) {
                if key == "text" {
                    *s = r.pick(TEXTS).to_string();
                } else if VARIANTS.contains(&s.as_str()) {
                    *s = r.pick(VARIANTS).to_string();
                }
            }
        }
        Value::Array(a) => {
            for e in a.iter_mut() {
                perturb(e, r, p, key);
            }
        }
        Value::Object(o) => {
            for (k, e) in o.iter_mut() {
                perturb(e, r, p, k);
            }
        }
        Value::Null => {}
    }
}

/// A copy of `base` with some fields set to odd values. Each change is kept
/// only if the whole still parses, so one bad value doesn't undo the rest.
fn fuzz<T: Serialize + DeserializeOwned + Clone>(base: &T, r: &mut Rng, p: f64) -> T {
    fn leaves(v: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match v {
            Value::Object(o) => {
                for (k, e) in o {
                    path.push(k.clone());
                    leaves(e, path, out);
                    path.pop();
                }
            }
            Value::Array(a) => {
                for (i, e) in a.iter().enumerate() {
                    path.push(format!("#{i}"));
                    leaves(e, path, out);
                    path.pop();
                }
            }
            _ => out.push(path.clone()),
        }
    }
    fn at<'a>(v: &'a mut Value, path: &[String]) -> &'a mut Value {
        path.iter().fold(v, |v, k| match k.strip_prefix('#') {
            Some(i) => &mut v[i.parse::<usize>().unwrap()],
            None => &mut v[k.as_str()],
        })
    }
    let mut v = serde_json::to_value(base).unwrap();
    let mut ps = Vec::new();
    leaves(&v, &mut Vec::new(), &mut ps);
    for path in ps {
        if !r.chance(p) {
            continue;
        }
        let key = path.iter().rev().find(|k| !k.starts_with('#')).cloned().unwrap_or_default();
        let old = at(&mut v, &path).clone();
        perturb(at(&mut v, &path), r, 1.0, &key);
        if serde_json::from_value::<T>(v.clone()).is_err() {
            *at(&mut v, &path) = old;
        }
    }
    serde_json::from_value(v).unwrap_or_else(|_| base.clone())
}

fn anim(r: &mut Rng) -> Animation {
    let (w, h) = r.pick(&[(1u32, 1u32), (2, 3), (17, 9), (64, 48), (1, 40), (40, 1), (96, 96), (33, 64)]);
    let n = r.pick(&[1usize, 2, 3, 5, 9]);
    let delays = [0u16, 1, 2, 5, 10, 10, 10, 37, 500, 65535];
    let frames = (0..n)
        .map(|i| {
            let img = RgbaImage::from_fn(w, h, |x, y| {
                let a = if (x + y + i as u32) % 7 == 0 { 0 } else { 255 };
                Rgba([(x * 37 + i as u32 * 50) as u8, (y * 23) as u8, ((x ^ y) * 11) as u8, a])
            });
            let mut f = Frame::new(img, r.pick(&delays));
            f.enabled = !r.chance(0.15);
            // gaps and clip starts from splitting the GIF row
            f.blank = r.chance(0.2);
            f.cut = r.chance(0.2);
            f
        })
        .collect();
    Animation { frames, loop_count: r.pick(&[None, Some(0), Some(1), Some(3)]) }
}

fn layer(r: &mut Rng, w: u32, h: u32, n: usize, p: f64) -> LayerKind {
    use crate::{effects, shapes, text};
    match r.below(10) {
        0 | 1 => LayerKind::Effects(Box::new(fuzz(&effects_base(r), r, p))),
        2 => LayerKind::Text(fuzz(&text::Annotation::default(), r, p)),
        3 => LayerKind::Censor(fuzz(&effects::CensorSettings::default(), r, p)),
        4 => {
            let k = r.pick(&[1usize, 2, 3]);
            let (ow, oh) = r.pick(&[(1u32, 1u32), (10, 4), (50, 70)]);
            let src = crate::overlay::OverlaySource { name: "o".into(), path: None, frames: (0..k).map(|i| RgbaImage::from_pixel(ow, oh, Rgba([i as u8 * 80, 200, 50, 180]))).collect(), delays: (0..k).map(|_| r.pick(&[0u16, 3, 10])).collect() };
            let pl = crate::overlay::Placement { enabled: true, x: 2, y: 3, w: w.max(2) / 2, h: h.max(2) / 2, angle: 15.0 };
            let m = r.pick(&[0usize, 1, n, n + 3]);
            let placements = (0..m).map(|_| fuzz(&pl, r, p)).collect();
            LayerKind::Overlay(crate::stack::OverlayLayer { src: Arc::new(src), placements, opacity: r.pick(&[0.0, 50.0, 100.0, 150.0, -10.0]) })
        }
        5 => {
            let mut wm = fuzz(&effects::WatermarkSettings::default(), r, p);
            if r.chance(0.5) {
                wm.image = Some(Arc::new(RgbaImage::from_pixel(r.pick(&[1u32, 9, 120]), r.pick(&[1u32, 9, 70]), Rgba([255, 0, 0, 128]))));
            }
            LayerKind::Watermark(Box::new(wm))
        }
        6 => LayerKind::Shield(fuzz(&effects::ShieldSettings::default(), r, p)),
        7 => LayerKind::Shape(Box::new(fuzz(&shapes::ShapeSettings::default(), r, p))),
        8 => {
            let k = r.below(4);
            let strokes = (0..k)
                .map(|_| {
                    let np = r.pick(&[0usize, 1, 2, 5, 40]);
                    let points = (0..np).map(|_| [(r.f() * 1.4 - 0.2) as f32 * w as f32, (r.f() * 1.4 - 0.2) as f32 * h as f32]).collect();
                    fuzz(&shapes::PenStroke { points, ..Default::default() }, r, p)
                })
                .collect();
            LayerKind::Markup(shapes::MarkupSettings { strokes })
        }
        _ => LayerKind::Emoji(Box::new(fuzz(&shapes::EmojiSettings::default(), r, p))),
    }
}

/// Effects with stacked looks and distortions and some animation on, so the
/// fuzzing has lists and switches to work on.
fn effects_base(r: &mut Rng) -> crate::effects::EffectSettings {
    use crate::booth::{BoothFx, BoothItem};
    let mut e = crate::effects::EffectSettings::default();
    let item = |r: &mut Rng, list: &[BoothFx]| BoothItem {
        fx: r.pick(list),
        amount: r.pick(&[0.0, 1.0, 50.0, 100.0, 250.0, 5000.0, 1e7, -40.0]),
        cx: r.pick(&[0.0, 50.0, 100.0, -20.0, 300.0]),
        cy: r.pick(&[0.0, 50.0, 100.0, -20.0]),
        reverse: r.chance(0.5),
    };
    for _ in 0..r.below(3) {
        let it = item(r, &BoothFx::LOOKS);
        e.looks.push(it);
    }
    for _ in 0..r.below(4) {
        let it = item(r, &BoothFx::DISTORT);
        e.warps.push(it);
    }
    e.anim = r.chance(0.4);
    e.counter = r.chance(0.4);
    e.counter_pos = r.pick(&[0u8, 1, 2, 3, 9]);
    e.counter_size = r.pick(&[0.0, 7.0, 40.0, 500.0]);
    e.intro = crate::motion::Transition::ALL[r.below(crate::motion::Transition::ALL.len())];
    e.outro = crate::motion::Transition::ALL[r.below(crate::motion::Transition::ALL.len())];
    e.intro_frames = r.pick(&[0usize, 1, 3, 1000]);
    e.outro_frames = r.pick(&[0usize, 2, 1000]);
    e.loop_spin = r.chance(0.3);
    e.loop_float = r.chance(0.3);
    e.loop_squash = r.chance(0.3);
    e.loop_heartbeat = r.chance(0.3);
    e.loop_blink = r.chance(0.3);
    e.loop_reverse = r.chance(0.5);
    e.loop_amount = r.pick(&[0.0, 50.0, 200.0, 1000.0]);
    e.wave = r.chance(0.2);
    e.wave_reverse = r.chance(0.5);
    e.pixelate = r.chance(0.2);
    e.pixelate_size = r.pick(&[0u32, 1, 2, 9, 100000]);
    e.oil = r.chance(0.1);
    e.oil_detail = r.pick(&[0u32, 2, 64, 1000]);
    e.emboss = r.chance(0.2);
    e.emboss_dir = r.pick(&[0u8, 3, 7, 200]);
    e.emboss_colour = r.chance(0.5);
    e.solarize = r.chance(0.2);
    e.solarize_threshold = r.pick(&[-5.0, 0.0, 127.0, 255.0, 900.0]);
    e.noise = r.chance(0.2);
    e.noise_mono = r.chance(0.5);
    e.noise_still = r.chance(0.5);
    for (on, v) in [(&mut e.gotham, &mut e.gotham_amount), (&mut e.lomo, &mut e.lomo_amount), (&mut e.toaster, &mut e.toaster_amount), (&mut e.vignette, &mut e.vignette_amount)] {
        *on = r.chance(0.2);
        *v = r.pick(&[0.0, 40.0, 100.0, 300.0, -50.0]);
    }
    e
}

fn timing(r: &mut Rng, n: usize) -> crate::timeline::Timing {
    let mut t = crate::timeline::Timing::default();
    if r.chance(0.5) {
        let k = r.below(4);
        t.clips = Some((0..k).map(|_| { let e = r.below(n + 3); Clip { start: r.below(n + 3), end: r.pick(&[None, Some(e), Some(0)]) } }).collect());
    }
    if r.chance(0.4) {
        for _ in 0..r.below(4) {
            t.keys.push(Key { frame: r.below(n + 4), x: r.pick(&[0.0, 30.0, -500.0]), y: r.pick(&[0.0, 12.0]), scale_x: r.pick(&[0.0, 50.0, 100.0, 400.0, -20.0]), scale_y: r.pick(&[0.0, 100.0, 250.0]), angle: r.pick(&[0.0, 90.0, 720.0]), opacity: r.pick(&[0.0, 50.0, 100.0, 300.0]), ease: crate::timeline::Ease::ALL[r.below(5)] });
        }
        t.sort_keys();
    }
    t
}

static HITS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// One random case. Returns a short description of it.
fn case(seed: u64, stage: &Mutex<&'static str>) -> String {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let fonts = crate::text::builtin_fonts();
    let p = r.pick(&[0.05, 0.15, 0.4]);
    let base = anim(&mut r);
    let (w, h, n) = (base.width(), base.height(), base.len());
    let mut st = Stack::default();
    if r.chance(0.7) {
        st.transform = fuzz(&crate::stack::Transform::default(), &mut r, p);
        // keep sizes from growing past what a test can hold
        let t = &mut st.transform;
        t.rs_w = t.rs_w.min(600);
        t.rs_h = t.rs_h.min(600);
        t.rs_pct = t.rs_pct.clamp(-50.0, 400.0);
        for v in &mut t.pad {
            *v = (*v).min(300);
        }
    }
    // outputs bigger than this test machine can hold aren't interesting here
    let g = st.transform.geo_map(w, h);
    if g.w * g.h > 9_000_000.0 {
        st.transform.resize_on = false;
    }
    let k = r.below(5);
    for _ in 0..k {
        let kind = layer(&mut r, w, h, n, p);
        let id = st.push(kind);
        let tm = timing(&mut r, n);
        let vis = !r.chance(0.1);
        let op = r.pick(&[100.0f32, 100.0, 50.0, 0.0, -10.0, 300.0]);
        let clip = r.chance(0.3);
        if let Some(l) = st.layers.iter_mut().find(|l| l.id == id) {
            l.clip = clip;
            l.timing = tm;
            l.visible = vis;
            l.opacity = op;
        }
    }
    let desc = format!("seed {seed}: {w}x{h} n={n} layers={:?}", st.layers.iter().map(|l| format!("{:?}", l.kind())).collect::<Vec<_>>());
    let verbose = std::env::var_os("FUZZ_VERBOSE").is_some();
    if verbose {
        println!("{desc}\ntransform {}", serde_json::to_string(&st.transform).unwrap());
        for l in &st.layers {
            let j = match &l.kind {
                LayerKind::Effects(x) => serde_json::to_string(x),
                LayerKind::Text(x) => serde_json::to_string(x),
                LayerKind::Censor(x) => serde_json::to_string(x),
                LayerKind::Watermark(x) => serde_json::to_string(x),
                LayerKind::Shield(x) => serde_json::to_string(x),
                LayerKind::Shape(x) => serde_json::to_string(x),
                LayerKind::Markup(x) => serde_json::to_string(x),
                LayerKind::Emoji(x) => serde_json::to_string(x),
                LayerKind::Overlay(x) => Ok(format!("overlay {:?}", x.placements)),
            };
            println!("{:?} {:?} {}", l.kind(), l.timing, j.unwrap_or_default());
        }
    }
    let t0 = std::time::Instant::now();
    let set = |s: &'static str| {
        if verbose {
            println!("  {:>7.2}s -> {s}", t0.elapsed().as_secs_f32());
        }
        *stage.lock().unwrap() = s
    };
    set("layout_text");
    st.layout_text(w, h, &fonts);
    set("plan");
    let plan = st.plan(&base);
    let times = crate::stack::plan_times(&plan);
    let _ = (st.signature(), st.plan_signature(), st.needs_render());
    set("prepare");
    let prep = st.prepare(&base, &fonts);
    set("render_frame");
    for i in 0..plan.len() {
        let img = st.render_frame(&base, &plan, &times, i, &fonts, &prep);
        assert!(img.width() > 0 && img.height() > 0, "empty frame");
    }
    set("gpu");
    gpu_check(&st, &base, &plan, &times, &fonts, &prep, &desc);
    set("proxy");
    if w > 4 && h > 4 {
        let s = 0.5;
        let (pw, ph) = crate::stack::proxy_size(w, h, s);
        let small = Animation { frames: base.frames.iter().map(|f| Frame { image: crate::pixels::Pixels::new(image::imageops::resize(&*f.image.get(), pw, ph, image::imageops::FilterType::Triangle)), ..f.clone() }).collect(), loop_count: base.loop_count };
        let ps = st.proxy(s);
        let pp = ps.prepare(&small, &fonts);
        for i in 0..plan.len().min(3) {
            ps.render_frame(&small, &plan, &times, i, &fonts, &pp);
        }
    }
    set("flatten");
    let flat = st.flatten(&base, &fonts, None);
    let shown = flat.frames.iter().filter(|f| f.enabled).count();
    if verbose {
        println!("  flat {}x{} x{} shown {shown}", flat.width(), flat.height(), flat.len());
    }
    use crate::io::PreviewFormat as P;
    let gif = fuzz(&crate::encode::GifOptions::default(), &mut r, p);
    let fmts = vec![
        ("gif", P::Gif(gif.clone())),
        ("webp", P::Webp(fuzz(&crate::io::WebpOptions::default(), &mut r, p))),
        ("apng", P::Apng),
        ("frames", P::Frames(crate::io::SplitFormat::Gif, crate::io::PngOptions::default(), gif)),
        ("sheet", P::Sheet(fuzz(&crate::io::SheetOptions::default(), &mut r, p))),
    ];
    for (name, f) in fmts {
        set(name);
        // effects that turn the frame can still make it enormous; encoding
        // those only finds out how much memory this machine has
        if shown == 0 || flat.width() as u64 * flat.height() as u64 > 16_000_000 {
            continue;
        }
        let _ = crate::io::encode_preview(&flat, &f, None);
    }
    desc
}

thread_local! {
    static FUZZ_GPU: std::cell::RefCell<Option<Option<crate::gpu_render::GpuRenderer>>> = const { std::cell::RefCell::new(None) };
}

/// The same frames on the graphics card (when there is one, FUZZ_GPU=1):
/// they must come out the size the CPU makes them.
#[allow(clippy::too_many_arguments)]
fn gpu_check(st: &Stack, base: &Animation, plan: &[crate::stack::PlanFrame], times: &[f32], fonts: &[crate::text::FontEntry], prep: &crate::stack::Prepared, desc: &str) {
    if std::env::var_os("FUZZ_GPU").is_none() {
        return;
    }
    FUZZ_GPU.with(|g| {
        let mut g = g.borrow_mut();
        let gpu = g.get_or_insert_with(crate::gpu_render::GpuRenderer::headless);
        let Some(gpu) = gpu.as_mut() else { return };
        for i in 0..plan.len().min(4) {
            let Some(job) = st.gpu_frame(base, plan, times, i, fonts, prep) else { continue };
            let src = plan[i].src;
            let frame = &base.frames[src].image.get();
            if !job.fits(frame, gpu.max_side) {
                continue;
            }
            // keys unique to this run: the renderer keeps pictures by key
            static KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let key = KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tex = gpu.render(frame, key, &job, key);
            let out = gpu.read(&tex);
            let cpu = st.render_frame(base, plan, times, i, fonts, prep);
            assert_eq!(out.dimensions(), cpu.dimensions(), "gpu size differs, frame {i}: {desc}");
        }
    });
}

#[test]
#[ignore]
fn fuzz_pipeline() {
    let n: u64 = std::env::var("FUZZ_N").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let seed0: u64 = std::env::var("FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        if std::env::var_os("FUZZ_VERBOSE").is_some() {
            println!("{}", std::backtrace::Backtrace::force_capture());
        }
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<String>().cloned().or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        HITS.lock().unwrap().push((loc, msg));
    }));
    let mut fails: Vec<(String, String, String, String)> = Vec::new();
    let mut slow = Vec::new();
    for s in seed0..seed0 + n {
        let stage: Mutex<&'static str> = Mutex::new("start");
        if let Ok(f) = std::env::var("FUZZ_LAST") {
            let _ = std::fs::write(f, s.to_string());
        }
        let t0 = std::time::Instant::now();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(s, &stage)));
        let dt = t0.elapsed().as_secs_f32();
        let st = *stage.lock().unwrap();
        match res {
            Ok(d) => {
                if dt > 4.0 {
                    slow.push(format!("{dt:.1}s {d}"));
                    println!("slow {dt:.1}s {d}");
                }
            }
            Err(_) => {
                let (loc, msg) = HITS.lock().unwrap().last().cloned().unwrap_or_default();
                fails.push((loc, msg, st.to_string(), format!("seed {s}")));
            }
        }
    }
    std::panic::set_hook(prev);
    let mut seen = std::collections::BTreeMap::new();
    for (loc, msg, stage, seed) in &fails {
        seen.entry(loc.clone()).or_insert_with(Vec::new).push(format!("[{stage}] {seed}: {}", msg.chars().take(160).collect::<String>()));
    }
    println!("\n==== {} cases, {} panics at {} places, {} slow", n, fails.len(), seen.len(), slow.len());
    for (loc, v) in &seen {
        println!("-- {loc} ({}x)\n   {}", v.len(), v.iter().take(3).cloned().collect::<Vec<_>>().join("\n   "));
    }
    for s in slow.iter().take(15) {
        println!("slow: {s}");
    }
}

/// Values a person can type past the end of a slider, one at a time, timed.
#[test]
#[ignore]
fn extremes() {
    use crate::effects::EffectSettings;
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_fn(320, 240, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8, 255]));
    let base = Animation { frames: vec![Frame::new(img.clone(), 10), Frame::new(img, 10)], loop_count: None };
    let fx = |f: &dyn Fn(&mut EffectSettings)| {
        let mut s = EffectSettings::default();
        f(&mut s);
        LayerKind::Effects(Box::new(s))
    };
    let mut cases: Vec<(String, Box<dyn Fn() -> (Stack, Option<crate::io::PreviewFormat>)>)> = Vec::new();
    macro_rules! case {
        ($name:expr, $mk:expr) => {
            cases.push(($name.to_string(), Box::new(move || $mk)));
        };
    }
    let one = |k: LayerKind| {
        let mut s = Stack::default();
        s.push(k);
        (s, None)
    };
    for v in [0.0f32, -5.0, 500.0] {
        case!(format!("blur {v}"), one(fx(&|s| (s.blur, s.blur_sigma) = (true, v))));
        case!(format!("sharpen {v}"), one(fx(&|s| (s.sharpen, s.sharpen_sigma) = (true, v))));
        case!(format!("rounded {v}"), one(fx(&|s| (s.frame_rounded, s.rounded_radius) = (true, v * 10.0))));
        case!(format!("wave len {v}"), one(fx(&|s| (s.wave, s.wave_len, s.wave_amp) = (true, v, 500.0))));
        case!(format!("noise {v}"), one(fx(&|s| (s.noise, s.noise_amount) = (true, v * 2.0))));
        case!(format!("implode {v}"), one(fx(&|s| (s.implode, s.implode_amount) = (true, v))));
        case!(format!("swirl {v}"), one(fx(&|s| (s.swirl, s.swirl_deg) = (true, v * 100.0))));
        case!(format!("rc fuzz {v}"), one(fx(&|s| (s.rc_white, s.rc_fuzz) = (true, v))));
        case!(format!("hue/sat/light {v}"), one(fx(&|s| (s.hue, s.saturation, s.lightness, s.brightness, s.contrast) = (v, v, v, v, v))));
        case!(format!("loop amount {v}"), one(fx(&|s| (s.loop_shake, s.loop_pulse, s.loop_wobble, s.loop_amount) = (true, true, true, v))));
        case!(format!("booth {v}"), one(fx(&|s| (s.booth, s.booth_amount) = (crate::booth::BoothFx::Kaleidoscope, v))));
        case!(format!("warp {v}"), one(fx(&|s| (s.warp, s.warp_amount) = (crate::booth::BoothFx::Twirl, v))));
    }
    for v in [0u32, 64, 255] {
        case!(format!("oil {v}"), one(fx(&|s| (s.oil, s.oil_radius) = (true, v))));
        case!(format!("dither {v}"), one(fx(&|s| (s.ordered_dither, s.error_dither, s.dither_levels) = (true, true, v as _))));
        case!(format!("posterize {v}"), one(fx(&|s| (s.posterize, s.posterize_levels) = (true, v as _))));
        case!(format!("rc edge {v}"), one(fx(&|s| (s.rc_white, s.rc_clean_edges, s.rc_edge_px) = (true, true, v as _))));
        case!(format!("fade frames {v}"), one(fx(&|s| (s.intro, s.fade_frames) = (crate::motion::Transition::Iris, v as _))));
    }
    for v in [1u32, 2000] {
        case!(format!("border {v}"), one(fx(&|s| (s.frame_border, s.border_px) = (true, v as _))));
    }
    for (sz, txt) in [(4000.0f32, "Hello world"), (400.0, "a long caption that wraps onto many many lines at this size"), (4.0, "tiny")] {
        for wrap in [false, true] {
            case!(format!("text {sz} wrap {wrap}"), {
                let mut a = crate::text::Annotation::default();
                (a.size, a.text, a.wrap) = (sz, txt.to_string(), wrap);
                one(LayerKind::Text(a))
            });
        }
    }
    case!("text outline 200", {
        let mut a = crate::text::Annotation::default();
        (a.size, a.border, a.border_width) = (300.0, crate::text::Border::Heavy, Some(200.0));
        one(LayerKind::Text(a))
    });
    case!("text stretch 2000%", {
        let mut a = crate::text::Annotation::default();
        (a.size, a.stretch) = (200.0, 20.0);
        one(LayerKind::Text(a))
    });
    for v in [0.0f32, 500.0] {
        case!(format!("censor blur {v}"), {
            let c = crate::effects::CensorSettings { method: crate::effects::CensorMethod::Blur, sigma: v, w: 300, h: 200, ..Default::default() };
            one(LayerKind::Censor(c))
        });
        case!(format!("censor pixel {v}"), {
            let c = crate::effects::CensorSettings { method: crate::effects::CensorMethod::Pixelate, square: v as _, w: 300, h: 200, ..Default::default() };
            one(LayerKind::Censor(c))
        });
        case!(format!("shape stroke {v}"), {
            let s = crate::shapes::ShapeSettings { stroke_on: true, stroke_w: v, w: 200, h: 150, ..Default::default() };
            one(LayerKind::Shape(Box::new(s)))
        });
        case!(format!("shield {v}"), one(LayerKind::Shield(crate::effects::ShieldSettings { strength: v, ..Default::default() })));
        case!(format!("watermark size {v}"), {
            let mut w = crate::effects::WatermarkSettings::default();
            (w.size, w.tiled, w.spacing) = (v, true, 0.0);
            one(LayerKind::Watermark(Box::new(w)))
        });
    }
    for v in [0.0f32, 0.001, 100000.0] {
        case!(format!("speed {v}%"), {
            let mut s = Stack::default();
            (s.transform.speed_on, s.transform.speed_val) = (true, v);
            (s, None)
        });
        case!(format!("fps {v}"), {
            let mut s = Stack::default();
            (s.transform.speed_on, s.transform.speed_mode, s.transform.speed_val) = (true, crate::stack::SpeedMode::Fps, v);
            (s, None)
        });
    }
    for v in [0.0f32, -50.0, 1000.0] {
        case!(format!("resize {v}%"), {
            let mut s = Stack::default();
            (s.transform.resize_on, s.transform.rs_pct) = (true, v);
            (s, None)
        });
        case!(format!("rotate {v}°"), {
            let mut s = Stack::default();
            (s.transform.rot_on, s.transform.rot_any, s.transform.rot_deg) = (true, true, v);
            (s, None)
        });
    }
    for l in [0u16, 200, 999] {
        case!(format!("gif lossy {l}"), (Stack::default(), Some(crate::io::PreviewFormat::Gif(crate::encode::GifOptions { lossy: l, ..Default::default() }))));
    }
    for c in [0u16, 1, 2] {
        case!(format!("gif colours {c}"), (Stack::default(), Some(crate::io::PreviewFormat::Gif(crate::encode::GifOptions { colors: c, ..Default::default() }))));
    }
    case!("webp q 0 m 0", (Stack::default(), Some(crate::io::PreviewFormat::Webp(crate::io::WebpOptions { quality: 0.0, method: 0, ..Default::default() }))));
    case!("webp q 1000 m 99", (Stack::default(), Some(crate::io::PreviewFormat::Webp(crate::io::WebpOptions { quality: 1000.0, method: 99, ..Default::default() }))));
    case!("sheet 0 cols", (Stack::default(), Some(crate::io::PreviewFormat::Sheet(crate::io::SheetOptions { columns: 0, padding: 5000, ..Default::default() }))));
    case!("sheet 9999 cols", (Stack::default(), Some(crate::io::PreviewFormat::Sheet(crate::io::SheetOptions { columns: 9999, ..Default::default() }))));

    std::panic::set_hook(Box::new(|_| {}));
    for (name, mk) in &cases {
        let t0 = std::time::Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (mut st, fmt) = mk();
            st.layout_text(320, 240, &fonts);
            let flat = st.flatten(&base, &fonts, None);
            let f = fmt.unwrap_or(crate::io::PreviewFormat::Gif(Default::default()));
            let (w, h) = (flat.width(), flat.height());
            let enc = if flat.frames.iter().any(|f| f.enabled) { crate::io::encode_preview(&flat, &f, None).map(|(_, b)| b.len()).map_err(|e| e.to_string()) } else { Err("no frames".into()) };
            format!("{w}x{h} x{} -> {:?}", flat.len(), enc)
        }));
        let dt = t0.elapsed().as_secs_f32();
        let flag = if r.is_err() { "PANIC" } else if dt > 3.0 { "SLOW" } else { "" };
        println!("{flag:5} {dt:6.2}s  {name:28} {}", r.unwrap_or_else(|e| e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()).chars().take(140).collect::<String>());
    }
}

#[test]
#[ignore]
fn outline_look() {
    let fonts = crate::text::builtin_fonts();
    let mut row = Vec::new();
    for (sz, b) in [(24.0f32, crate::text::Border::Light), (64.0, crate::text::Border::Heavy), (160.0, crate::text::Border::Heavy)] {
        let a = crate::text::Annotation { text: "Outline Ag!".into(), size: sz, border: b, color: [255, 255, 255, 255], border_color: [0, 0, 0, 255], ..Default::default() };
        let t0 = std::time::Instant::now();
        row.push(crate::text::render_sprite(&fonts, &a));
        println!("{sz}: {:?}", t0.elapsed());
    }
    let w = row.iter().map(|i| i.width()).max().unwrap();
    let h: u32 = row.iter().map(|i| i.height()).sum();
    let mut out = RgbaImage::from_pixel(w, h, Rgba([200, 60, 60, 255]));
    let mut y = 0;
    for i in &row {
        image::imageops::overlay(&mut out, i, 0, y as i64);
        y += i.height();
    }
    out.save(std::env::var("OUT").unwrap()).unwrap();
    let a = crate::text::Annotation { text: "Huge".into(), size: 4000.0, border: crate::text::Border::Heavy, ..Default::default() };
    let t0 = std::time::Instant::now();
    let s = crate::text::render_sprite(&fonts, &a);
    println!("4000 heavy: {}x{} {:?}", s.width(), s.height(), t0.elapsed());
}

/// Every exporter writes a file that opens again with the right frames.
#[test]
#[ignore]
fn exporters_round_trip() {
    use crate::io;
    let dir = std::env::temp_dir().join("gifkit_export_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (w, h, n) in [(1u32, 1u32, 1usize), (3, 2, 2), (97, 61, 7), (64, 64, 1)] {
        let frames = (0..n).map(|i| {
            let img = RgbaImage::from_fn(w, h, |x, y| Rgba([(x * 40 + i as u32 * 30) as u8, (y * 50) as u8, 128, if (x + y) % 5 == 0 { 0 } else { 255 }]));
            Frame::new(img, [2u16, 10, 37][i % 3])
        }).collect();
        let a = Animation { frames, loop_count: Some(2) };
        let tag = format!("{w}x{h}x{n}");
        let check = |what: &str, r: anyhow::Result<Animation>| match r {
            Ok(b) => {
                let ok = b.len() == n && b.width() == w && b.height() == h;
                println!("{} {tag:10} {what:8} {}x{} x{}", if ok { "ok  " } else { "DIFF" }, b.width(), b.height(), b.len());
            }
            Err(e) => println!("FAIL {tag:10} {what:8} {e:#}"),
        };
        let p = dir.join(format!("{tag}.gif"));
        check("gif", io::save_gif(&a, &Default::default(), &p, None).and_then(|_| io::load_animation(&p)));
        let p = dir.join(format!("{tag}.png"));
        check("apng", io::save_apng(&a, &p, true).and_then(|_| io::load_animation(&p)));
        let p = dir.join(format!("{tag}.webp"));
        check("webp", io::encode_webp(&a, &Default::default()).and_then(|b| { std::fs::write(&p, b)?; io::load_animation(&p) }));
        let p = dir.join(format!("{tag}.avif"));
        check("avif", io::encode_avif(&a, 60).and_then(|b| { std::fs::write(&p, b)?; io::load_animation(&p) }));
        for (f, ext) in [(io::VideoFormat::Mp4, "mp4"), (io::VideoFormat::WebmAlpha, "webm")] {
            let p = dir.join(format!("{tag}.{ext}"));
            match io::export_video(&a, &p, f, [0, 0, 0], true).and_then(|_| io::load_video(&p)) {
                Ok(b) => println!("{} {tag:10} {ext:8} {}x{} x{} (video sizes round to even)", if b.len() >= 1 { "ok  " } else { "DIFF" }, b.width(), b.height(), b.len()),
                Err(e) => println!("FAIL {tag:10} {ext:8} {e:#}"),
            }
        }
        let p = dir.join(format!("{tag}.zip"));
        match io::export_frames_zip(&a, &p, "f") {
            Ok(k) => println!("{} {tag:10} zip      {k} files", if k == n { "ok  " } else { "DIFF" }),
            Err(e) => println!("FAIL {tag:10} zip      {e:#}"),
        }
        let d2 = dir.join(format!("{tag}_frames"));
        std::fs::create_dir_all(&d2).unwrap();
        match io::export_frames_dir(&a, &d2, "f", io::SplitFormat::Gif) {
            Ok(k) => println!("{} {tag:10} frames   {k} files", if k == n { "ok  " } else { "DIFF" }),
            Err(e) => println!("FAIL {tag:10} frames   {e:#}"),
        }
        let p = dir.join(format!("{tag}_sheet.png"));
        match io::export_spritesheet(&a, &p, &Default::default()) {
            Ok(s) => println!("ok   {tag:10} sheet    {s}"),
            Err(e) => println!("FAIL {tag:10} sheet    {e:#}"),
        }
    }
}

/// Broken files give an error, never a crash or a hang.
#[test]
#[ignore]
fn broken_files() {
    let s = std::env::var("FILES").unwrap();
    let mut r = Rng(7);
    std::panic::set_hook(Box::new(|_| {}));
    for path in s.split(':') {
        let Ok(good) = std::fs::read(path) else { continue };
        let mut variants: Vec<(String, Vec<u8>)> = vec![("empty".into(), vec![]), ("header only".into(), good[..good.len().min(16)].to_vec())];
        for k in [2usize, 3, 10, 100] {
            variants.push((format!("first 1/{k}"), good[..good.len() / k].to_vec()));
        }
        for _ in 0..30 {
            let mut b = good.clone();
            for _ in 0..1 + r.below(20) {
                let i = r.below(b.len());
                b[i] = r.next() as u8;
            }
            variants.push(("flipped bytes".into(), b));
        }
        let mut worst = 0.0f32;
        let (mut errs, mut oks, mut panics) = (0, 0, Vec::new());
        for (what, b) in &variants {
            let t0 = std::time::Instant::now();
            let tmp = std::env::temp_dir().join(format!("gifkit_broken.{}", path.rsplit('.').next().unwrap()));
            std::fs::write(&tmp, b).unwrap();
            let res = std::panic::catch_unwind(|| if path.ends_with(".gifkit") { crate::project::load(&tmp).map(|_| ()) } else { crate::io::load_animation(&tmp).map(|_| ()) });
            worst = worst.max(t0.elapsed().as_secs_f32());
            match res {
                Ok(Ok(())) => oks += 1,
                Ok(Err(_)) => errs += 1,
                Err(e) => panics.push(format!("{what}: {}", e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
            }
        }
        println!("{path}: {oks} opened, {errs} errors, {} panics, slowest {worst:.2}s {:?}", panics.len(), panics.iter().take(3).collect::<Vec<_>>());
    }
}

#[test]
#[ignore]
fn video_merge_levels() {
    let p = std::path::PathBuf::from(std::env::var("VIDEO").unwrap());
    let info = crate::io::probe_video(&p).unwrap();
    for merge in 0..crate::io::MERGE_LEVELS.len() {
        let mut o = crate::io::VideoImport::defaults(&info);
        o.end = o.end.min(30.0);
        o.merge = merge;
        let t0 = std::time::Instant::now();
        let a = crate::io::load_video_range(&p, &o, None).unwrap();
        let total: u32 = a.frames.iter().map(|f| f.delay_cs as u32).sum();
        println!("{:24} {:5} frames of {:5}, {:.2} s total, {:.0} MB, {:.1}s", crate::io::MERGE_LEVELS[merge].0, a.len(), o.frames(), total as f32 / 100.0, (a.len() * a.width() as usize * a.height() as usize * 4) as f64 / 1e6, t0.elapsed().as_secs_f32());
    }
}

/// Contact sheet of icons: ICON_LIST has lines "label|svg path".
#[test]
#[ignore]
fn icon_sheet() {
    let list = std::fs::read_to_string(std::env::var("ICON_LIST").unwrap()).unwrap();
    let fonts = crate::text::builtin_fonts();
    let items: Vec<(&str, &str)> = list.lines().filter_map(|l| l.split_once('|')).collect();
    let (cw, ch, cols) = (230u32, 44u32, 6u32);
    let rows = (items.len() as u32).div_ceil(cols);
    let mut sheet = RgbaImage::from_pixel(cw * cols, ch * rows, Rgba([35, 38, 41, 255]));
    for (k, (label, path)) in items.iter().enumerate() {
        let (x, y) = ((k as u32 % cols) * cw, (k as u32 / cols) * ch);
        if let Ok(svg) = std::fs::read_to_string(path) {
            if let Some(img) = crate::icons::render(&crate::icons::recolour(&svg, eframe::egui::Color32::from_rgb(252, 252, 252)), 32) {
                let rgba = RgbaImage::from_raw(32, 32, img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect()).unwrap();
                image::imageops::overlay(&mut sheet, &rgba, x as i64 + 6, y as i64 + 6);
            }
        }
        let a = crate::text::Annotation { text: label.to_string(), size: 13.0, color: [230, 230, 230, 255], ..Default::default() };
        let t = crate::text::render_sprite(&fonts, &a);
        image::imageops::overlay(&mut sheet, &t, x as i64 + 44, y as i64 + 12);
    }
    sheet.save(std::env::var("OUT").unwrap()).unwrap();
}

#[test]
#[ignore]
fn gpu_probe() {
    let t0 = std::time::Instant::now();
    println!("h264 {:?}", crate::gpu::h264().map(|h| h.name));
    println!("av1 {:?}", crate::gpu::av1().map(|h| h.name));
    println!("summary {:?} in {:?}", crate::gpu::summary(), t0.elapsed());
}

/// The graphics card's frames against the CPU's, for every kind of layer and
/// frame edit the card draws. Needs a GPU (or lavapipe).
#[test]
#[ignore]
fn gpu_matches_cpu() {
    use crate::stack::Transform;
    let Some(mut gpu) = crate::gpu_render::GpuRenderer::headless() else {
        println!("no GPU adapter");
        return;
    };
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_fn(160, 120, |x, y| Rgba([(x * 255 / 160) as u8, (y * 2) as u8, ((x ^ y) * 3) as u8, if (x / 20 + y / 20) % 5 == 0 { 90 } else { 255 }]));
    let base = Animation { frames: vec![Frame::new(img.clone(), 10), Frame::new(img, 10)], loop_count: None };
    let t = |f: &dyn Fn(&mut Transform)| {
        let mut t = Transform::default();
        f(&mut t);
        t
    };
    let mut cases: Vec<(&str, Transform, Vec<LayerKind>, Vec<crate::timeline::Key>)> = vec![
        ("plain", Transform::default(), vec![], vec![]),
        ("crop", t(&|t| (t.crop_on, t.crop) = (true, [20, 10, 100, 70])), vec![], vec![]),
        ("resize down", t(&|t| (t.resize_on, t.rs_pct) = (true, 50.0)), vec![], vec![]),
        ("resize up", t(&|t| (t.resize_on, t.rs_pct) = (true, 175.0)), vec![], vec![]),
        ("fit pad", t(&|t| (t.resize_on, t.rs_w, t.rs_h, t.rs_mode) = (true, 120, 120, crate::ops::AspectMode::FitPad)), vec![], vec![]),
        ("centre crop", t(&|t| (t.resize_on, t.rs_w, t.rs_h, t.rs_mode) = (true, 90, 90, crate::ops::AspectMode::CenterCrop)), vec![], vec![]),
        ("flip + 90", t(&|t| (t.rot_on, t.flip_h, t.rot) = (true, true, 90)), vec![], vec![]),
        ("turn 30 bg", t(&|t| (t.rot_on, t.rot_any, t.rot_deg, t.rot_expand, t.rot_bg_transparent) = (true, true, 30.0, true, false)), vec![], vec![]),
        ("pad", t(&|t| (t.pad_on, t.pad, t.pad_transparent) = (true, [10, 0, 20, 5], false)), vec![], vec![]),
    ];
    let text = crate::text::Annotation { text: "Hello GPU".into(), size: 28.0, x: 10, y: 20, color: [255, 255, 0, 255], border: crate::text::Border::Light, ..Default::default() };
    let shape = crate::shapes::ShapeSettings { x: 60, y: 30, w: 60, h: 40, angle: 20.0, ..Default::default() };
    let emoji = crate::shapes::EmojiSettings { x: 100, y: 60, w: 40, h: 40, ..Default::default() };
    let ov = crate::overlay::OverlaySource { name: "o".into(), path: None, frames: vec![RgbaImage::from_fn(30, 20, |x, _| Rgba([255, (x * 8) as u8, 0, 200]))], delays: vec![10] };
    let overlay = LayerKind::Overlay(crate::stack::OverlayLayer { src: Arc::new(ov), placements: vec![crate::overlay::Placement { enabled: true, x: 20, y: 50, w: 60, h: 40, angle: 25.0 }; 2], opacity: 80.0 });
    let layers = || vec![LayerKind::Text(text.clone()), LayerKind::Shape(Box::new(shape.clone())), LayerKind::Emoji(Box::new(emoji.clone())), overlay.clone()];
    cases.push(("layers", Transform::default(), layers(), vec![]));
    cases.push(("layers + turn", t(&|t| (t.rot_on, t.rot_any, t.rot_deg) = (true, true, -15.0)), layers(), vec![]));
    cases.push(("layers + resize", t(&|t| (t.resize_on, t.rs_pct) = (true, 60.0)), layers(), vec![]));
    let mk = crate::timeline::Key { frame: 0, x: 15.0, y: -5.0, scale_x: 150.0, scale_y: 80.0, angle: 30.0, opacity: 60.0, ease: crate::timeline::Ease::Linear };
    cases.push(("motion", Transform::default(), layers(), vec![mk]));
    let one = |k: usize| vec![layers().remove(k)];
    for (k, nm) in ["text", "shape", "emoji", "overlay"].iter().enumerate() {
        cases.push((Box::leak(format!("turn+{nm}").into_boxed_str()), t(&|t| (t.rot_on, t.rot_any, t.rot_deg) = (true, true, -15.0)), one(k), vec![]));
        cases.push((Box::leak(format!("motion {nm}").into_boxed_str()), Transform::default(), one(k), vec![mk]));
    }
    cases.push(("motion text", Transform::default(), vec![LayerKind::Text(text.clone())], vec![mk]));
    cases.push(("move text", Transform::default(), vec![LayerKind::Text(text.clone())], vec![crate::timeline::Key { scale_x: 100.0, scale_y: 100.0, angle: 0.0, opacity: 100.0, ..mk }]));
    cases.push(("scale text", Transform::default(), vec![LayerKind::Text(text.clone())], vec![crate::timeline::Key { x: 0.0, y: 0.0, angle: 0.0, opacity: 100.0, ..mk }]));
    cases.push(("turn text", Transform::default(), vec![LayerKind::Text(text.clone())], vec![crate::timeline::Key { x: 0.0, y: 0.0, scale_x: 100.0, scale_y: 100.0, opacity: 100.0, ..mk }]));
    cases.push(("fade shape", Transform::default(), vec![LayerKind::Shape(Box::new(shape.clone()))], vec![crate::timeline::Key { x: 0.0, y: 0.0, scale_x: 100.0, scale_y: 100.0, angle: 0.0, ..mk }]));
    let mut cap = text.clone();
    cap.caption = crate::text::Caption::Bottom;
    cases.push(("caption", Transform::default(), vec![LayerKind::Text(cap)], vec![]));
    let mut typing = text.clone();
    (typing.reveal, typing.reveal_secs) = (crate::text::Reveal::Letters, 0.3);
    cases.push(("typing text", t(&|t| (t.rot_on, t.rot_any, t.rot_deg) = (true, true, -10.0)), vec![LayerKind::Text(typing)], vec![]));
    let mut wm = crate::effects::WatermarkSettings::default();
    wm.tiled = true;
    cases.push(("watermark tiled", Transform::default(), vec![LayerKind::Watermark(Box::new(wm))], vec![]));
    // every effect, alone, then a few together
    use crate::booth::BoothFx;
    use crate::effects::EffectSettings as E;
    use crate::motion::Transition as Tr;
    let fx = |f: &dyn Fn(&mut E)| {
        let mut e = E::default();
        f(&mut e);
        LayerKind::Effects(Box::new(e))
    };
    let mut fxc: Vec<(String, LayerKind)> = vec![
        ("fx modulate".into(), fx(&|e| (e.hue, e.saturation, e.lightness) = (140.0, 150.0, 90.0))),
        ("fx bright contrast".into(), fx(&|e| (e.brightness, e.contrast) = (20.0, 30.0))),
        ("fx rc white".into(), fx(&|e| (e.rc_white, e.rc_fuzz) = (true, 30.0))),
        ("fx rc custom edges".into(), fx(&|e| (e.rc_custom, e.rc_color, e.rc_fuzz, e.rc_edge_px) = (true, [128, 60, 90], 25.0, 3))),
        ("fx rc black no edge".into(), fx(&|e| (e.rc_black, e.rc_fuzz, e.rc_clean_edges) = (true, 20.0, false))),
        ("fx key soft spill".into(), fx(&|e| (e.rc_custom, e.rc_color, e.rc_fuzz, e.rc_soft, e.rc_despill, e.rc_clean_edges) = (true, [40, 200, 60], 12.0, 25.0, true, false))),
        ("fx key soft only".into(), fx(&|e| (e.rc_custom, e.rc_color, e.rc_fuzz, e.rc_soft) = (true, [128, 60, 90], 10.0, 30.0))),
        ("fx grayscale".into(), fx(&|e| e.grayscale = true)),
        ("fx sepia".into(), fx(&|e| e.sepia = true)),
        ("fx monochrome".into(), fx(&|e| e.monochrome = true)),
        ("fx negative".into(), fx(&|e| e.negative = true)),
        ("fx tint".into(), fx(&|e| e.tint = true)),
        ("fx blur".into(), fx(&|e| e.blur = true)),
        ("fx blur 7".into(), fx(&|e| (e.blur, e.blur_sigma) = (true, 7.0))),
        ("fx sharpen".into(), fx(&|e| (e.sharpen, e.sharpen_sigma) = (true, 1.5))),
        ("fx oil".into(), fx(&|e| e.oil = true)),
        ("fx oil 9".into(), fx(&|e| (e.oil, e.oil_radius) = (true, 9))),
        ("fx emboss".into(), fx(&|e| e.emboss = true)),
        ("fx posterize".into(), fx(&|e| e.posterize = true)),
        ("fx solarize".into(), fx(&|e| e.solarize = true)),
        ("fx noise".into(), fx(&|e| e.noise = true)),
        ("fx swirl".into(), fx(&|e| e.swirl = true)),
        ("fx implode".into(), fx(&|e| e.implode = true)),
        ("fx wave".into(), fx(&|e| e.wave = true)),
        ("fx gotham".into(), fx(&|e| e.gotham = true)),
        ("fx lomo".into(), fx(&|e| e.lomo = true)),
        ("fx nashville".into(), fx(&|e| e.nashville = true)),
        ("fx toaster".into(), fx(&|e| e.toaster = true)),
        ("fx vignette".into(), fx(&|e| e.vignette = true)),
        ("fx ordered dither".into(), fx(&|e| e.ordered_dither = true)),
        ("fx change bg".into(), fx(&|e| (e.change_bg, e.bg_color) = (true, [20, 200, 90]))),
        ("fx flip v".into(), fx(&|e| e.flip_v = true)),
        ("fx flip h".into(), fx(&|e| e.flip_h = true)),
        ("fx rot90".into(), fx(&|e| e.rot90 = true)),
        ("fx rot270".into(), fx(&|e| e.rot270 = true)),
        ("fx rot any".into(), fx(&|e| (e.rot_any, e.rot_deg) = (true, 30.0))),
        ("fx camera".into(), fx(&|e| e.frame_camera = true)),
        ("fx fuzzy".into(), fx(&|e| e.frame_fuzzy = true)),
        ("fx rounded".into(), fx(&|e| e.frame_rounded = true)),
        ("fx polaroid".into(), fx(&|e| e.polaroid = true)),
        ("fx border".into(), fx(&|e| e.frame_border = true)),
        ("fx counter".into(), fx(&|e| e.counter = true)),
        ("fx shake".into(), fx(&|e| e.loop_shake = true)),
        ("fx pulse".into(), fx(&|e| (e.loop_pulse, e.loop_cycles) = (true, 3))),
        ("fx hop".into(), fx(&|e| (e.loop_hop, e.loop_cycles) = (true, 3))),
        ("fx wobble".into(), fx(&|e| (e.loop_wobble, e.loop_cycles) = (true, 3))),
        ("fx rainbow".into(), fx(&|e| (e.loop_rainbow, e.loop_cycles) = (true, 3))),
        ("fx glitch".into(), fx(&|e| (e.loop_glitch, e.loop_amount) = (true, 200.0))),
    ];
    for b in BoothFx::LOOKS.iter().chain(BoothFx::DISTORT.iter()) {
        let b = *b;
        fxc.push((format!("booth {}", b.label()), fx(&move |e| (e.booth, e.booth_amount, e.booth_cx, e.booth_cy) = (b, 70.0, 40.0, 60.0))));
    }
    fxc.push(("warp + booth".into(), fx(&|e| (e.booth, e.warp) = (BoothFx::Comic, BoothFx::Twirl))));
    use crate::booth::BoothItem;
    let item = |fx: BoothFx, amount: f32, reverse: bool| BoothItem { fx, amount, cx: 40.0, cy: 60.0, reverse };
    for b in BoothFx::DISTORT.iter() {
        let b = *b;
        fxc.push((format!("warp {} 250", b.label()), fx(&move |e| e.warps = vec![item(b, 250.0, false)])));
        fxc.push((format!("warp {} reversed", b.label()), fx(&move |e| e.warps = vec![item(b, 80.0, true)])));
    }
    fxc.push(("looks stacked".into(), fx(&move |e| e.looks = vec![item(BoothFx::Comic, 60.0, false), item(BoothFx::Thermal, 40.0, false)])));
    fxc.push(("warps stacked".into(), fx(&move |e| e.warps = vec![item(BoothFx::Twirl, 120.0, false), item(BoothFx::Bulge, 70.0, false), item(BoothFx::Mirror, 50.0, true)])));
    fxc.push(("wave reversed".into(), fx(&|e| (e.wave, e.wave_reverse) = (true, true))));
    for (nm, f) in [("gotham", 0usize), ("lomo", 1), ("nashville", 2), ("toaster", 3), ("vignette", 4)] {
        fxc.push((format!("fx {nm} 40%"), fx(&move |e| match f {
            0 => (e.gotham, e.gotham_amount) = (true, 40.0),
            1 => (e.lomo, e.lomo_amount) = (true, 40.0),
            2 => (e.nashville, e.nashville_amount) = (true, 40.0),
            3 => (e.toaster, e.toaster_amount) = (true, 40.0),
            _ => (e.vignette, e.vignette_amount) = (true, 40.0),
        })));
    }
    fxc.push(("look glow 300".into(), fx(&move |e| e.looks = vec![item(BoothFx::Glow, 300.0, false)])));
    fxc.push(("look comic 180".into(), fx(&move |e| e.looks = vec![item(BoothFx::Comic, 180.0, false)])));
    fxc.push(("fx gotham 250%".into(), fx(&|e| (e.gotham, e.gotham_amount) = (true, 250.0))));
    fxc.push(("fx emboss 300%".into(), fx(&|e| (e.emboss, e.emboss_amount) = (true, 300.0))));
    fxc.push(("fx loop 900%".into(), fx(&|e| (e.loop_wobble, e.loop_amount) = (true, 900.0))));
    fxc.push(("fx oil detail".into(), fx(&|e| (e.oil, e.oil_detail) = (true, 50))));
    fxc.push(("fx emboss dir".into(), fx(&|e| (e.emboss, e.emboss_dir, e.emboss_amount) = (true, 3, 60.0))));
    fxc.push(("fx emboss colour".into(), fx(&|e| (e.emboss, e.emboss_dir, e.emboss_colour) = (true, 6, true))));
    fxc.push(("fx solarize rev".into(), fx(&|e| (e.solarize, e.solarize_threshold, e.solarize_reverse) = (true, 80.0, true))));
    fxc.push(("fx noise mono".into(), fx(&|e| (e.noise, e.noise_mono, e.noise_still) = (true, true, true))));
    fxc.push(("fx pixelate".into(), fx(&|e| (e.pixelate, e.pixelate_size) = (true, 7))));
    fxc.push(("fx heartbeat".into(), fx(&|e| (e.loop_heartbeat, e.loop_amount) = (true, 150.0))));
    fxc.push(("fx squash float".into(), fx(&|e| (e.loop_squash, e.loop_float, e.loop_reverse) = (true, true, true))));
    fxc.push(("fx spin".into(), fx(&|e| e.loop_spin = true)));
    fxc.push(("fx blink rev".into(), fx(&|e| (e.loop_blink, e.loop_reverse) = (true, true))));
    fxc.push(("intro long".into(), fx(&|e| (e.intro, e.intro_frames) = (Tr::Iris, 6))));
    for pos in 0..4u8 {
        fxc.push((format!("counter {pos}"), fx(&move |e| (e.counter, e.counter_pos, e.counter_size) = (true, pos, 12.0))));
    }
    for t in Tr::ALL.iter().skip(1) {
        let t = *t;
        fxc.push((format!("intro {}", t.label()), fx(&move |e| (e.intro, e.fade_frames) = (t, 3))));
        fxc.push((format!("outro {} clear", t.label()), fx(&move |e| (e.outro, e.fade_frames, e.fade_to_transparent) = (t, 2, true))));
    }
    fxc.push(("fx combo".into(), fx(&|e| (e.sepia, e.blur, e.vignette, e.frame_rounded, e.polaroid, e.loop_wobble) = (true, true, true, true, true, true))));
    fxc.push(("fx combo 2".into(), fx(&|e| (e.contrast, e.posterize, e.rot90, e.frame_border, e.noise, e.counter) = (40.0, true, true, true, true, true))));
    for (nm, l) in fxc {
        cases.push((Box::leak(nm.into_boxed_str()), Transform::default(), vec![l], vec![]));
    }
    let cen = |m: crate::effects::CensorMethod, angle: f32| LayerKind::Censor(crate::effects::CensorSettings { x: 30, y: 20, w: 70, h: 50, method: m, angle, ..Default::default() });
    for (nm, m) in [("solid", crate::effects::CensorMethod::Solid), ("blur", crate::effects::CensorMethod::Blur), ("pixelate", crate::effects::CensorMethod::Pixelate)] {
        cases.push((Box::leak(format!("censor {nm}").into_boxed_str()), Transform::default(), vec![cen(m, 0.0)], vec![]));
        cases.push((Box::leak(format!("censor {nm} turned").into_boxed_str()), Transform::default(), vec![cen(m, 25.0)], vec![]));
    }
    let sh = crate::effects::ShieldSettings::default();
    cases.push(("shield", Transform::default(), vec![LayerKind::Shield(sh.clone())], vec![]));
    cases.push(("shield jitter", Transform::default(), vec![LayerKind::Shield(crate::effects::ShieldSettings { jitter: true, per_frame: true, strength: 12.0, ..sh.clone() })], vec![]));
    // layers on both sides of an effect that changes the size
    cases.push(("polaroid + text", t(&|t| (t.resize_on, t.rs_pct) = (true, 80.0)), vec![LayerKind::Shape(Box::new(shape.clone())), fx(&|e| (e.polaroid, e.swirl) = (true, true)), LayerKind::Text(text.clone())], vec![]));
    cases.push(("fade effect", Transform::default(), vec![fx(&|e| (e.negative, e.rot90) = (true, false))], vec![crate::timeline::Key { x: 0.0, y: 0.0, scale_x: 100.0, scale_y: 100.0, angle: 0.0, ..mk }]));
    cases.push(("censor moved", Transform::default(), vec![cen(crate::effects::CensorMethod::Pixelate, 10.0)], vec![crate::timeline::Key { opacity: 100.0, ..mk }]));
    cases.push(("fx + turn frame", t(&|t| (t.rot_on, t.rot_any, t.rot_deg) = (true, true, 20.0)), vec![fx(&|e| (e.oil, e.frame_fuzzy) = (true, true))], vec![]));
    // clipping: layers whose name ends in '^' are clipped to the one below
    let clip_cases: Vec<(&str, Vec<(LayerKind, bool)>)> = vec![
        ("clip fx to text", vec![(LayerKind::Text(text.clone()), false), (fx(&|e| e.negative = true), true)]),
        ("clip glow to shape", vec![(LayerKind::Shape(Box::new(shape.clone())), false), (fx(&|e| e.looks = vec![crate::booth::BoothItem { fx: BoothFx::Glow, amount: 80.0, ..Default::default() }]), true)]),
        ("clip shape to emoji", vec![(LayerKind::Emoji(Box::new(emoji.clone())), false), (LayerKind::Shape(Box::new(shape.clone())), true)]),
        ("clip two to text", vec![(LayerKind::Text(text.clone()), false), (fx(&|e| e.grayscale = true), true), (LayerKind::Shape(Box::new(shape.clone())), true)]),
        ("clip to effect", vec![(fx(&|e| e.sepia = true), false), (fx(&|e| e.negative = true), true)]),
        ("clip fx to overlay", vec![(overlay.clone(), false), (fx(&|e| e.blur = true), true)]),
        ("anim shake on emoji", vec![(LayerKind::Emoji(Box::new(emoji.clone())), false), (fx(&|e| (e.anim, e.loop_shake, e.loop_amount) = (true, true, 200.0)), true)]),
        ("anim spin on shape", vec![(LayerKind::Shape(Box::new(shape.clone())), false), (fx(&|e| (e.anim, e.loop_spin, e.loop_cycles) = (true, true, 3)), true)]),
        ("anim intro on text", vec![(LayerKind::Text(text.clone()), false), (fx(&|e| (e.anim, e.intro, e.fade_frames) = (true, Tr::SlideLeft, 4)), true)]),
        ("grey then anim", vec![(LayerKind::Shape(Box::new(shape.clone())), false), (fx(&|e| e.grayscale = true), true), (fx(&|e| (e.anim, e.loop_wobble, e.loop_amount) = (true, true, 300.0)), true), (fx(&|e| e.negative = true), true)]),
        ("anim on image", vec![(overlay.clone(), false), (fx(&|e| (e.anim, e.loop_float, e.loop_amount) = (true, true, 400.0)), true)]),
        ("clip then unclipped", vec![(LayerKind::Text(text.clone()), false), (fx(&|e| e.negative = true), true), (LayerKind::Emoji(Box::new(emoji.clone())), false)]),
    ];
    for (nm, ls) in clip_cases {
        let clips: Vec<bool> = ls.iter().map(|l| l.1).collect();
        let kinds: Vec<LayerKind> = ls.into_iter().map(|l| l.0).collect();
        cases.push((Box::leak(format!("{nm}^{}", clips.iter().map(|c| if *c { '1' } else { '0' }).collect::<String>()).into_boxed_str()), Transform::default(), kinds, vec![]));
    }
    let only = std::env::var("GPU_ONLY").ok();
    for (ci, (name, tr, ls, keys)) in cases.into_iter().enumerate() {
        if let Some(o) = &only {
            if !name.contains(o.as_str()) {
                continue;
            }
        }
        let mut st = Stack::default();
        st.transform = tr;
        let clips: Vec<bool> = name.rsplit_once('^').map(|(_, c)| c.chars().map(|c| c == '1').collect()).unwrap_or_default();
        for (k, kind) in ls.into_iter().enumerate() {
            let id = st.push(kind);
            let l = st.layers.iter_mut().find(|l| l.id == id).unwrap();
            if !keys.is_empty() {
                l.timing.keys = keys.clone();
            }
            l.clip = clips.get(k).copied().unwrap_or(false);
        }
        st.layout_text(160, 120, &fonts);
        let plan = st.plan(&base);
        let times = crate::stack::plan_times(&plan);
        let prep = st.prepare(&base, &fonts);
        let cpu = st.render_frame(&base, &plan, &times, 0, &fonts, &prep);
        let Some(job) = st.gpu_frame(&base, &plan, &times, 0, &fonts, &prep) else {
            println!("{name:18} not on the GPU");
            continue;
        };
        let tex = gpu.render(&base.frames[0].image.get(), 1, &job, ci as u64 + 1);
        let g = gpu.read(&tex);
        if g.dimensions() != cpu.dimensions() {
            println!("{name:18} SIZE gpu {:?} cpu {:?}", g.dimensions(), cpu.dimensions());
            continue;
        }
        // premultiplied difference, so colour under full transparency doesn't count
        let (mut sum, mut max, mut bad) = (0f64, 0i32, 0usize);
        for (a, b) in g.pixels().zip(cpu.pixels()) {
            let mut worst = 0;
            for c in 0..4 {
                let pa = if c == 3 { a[3] as i32 } else { a[c] as i32 * a[3] as i32 / 255 };
                let pb = if c == 3 { b[3] as i32 } else { b[c] as i32 * b[3] as i32 / 255 };
                worst = worst.max((pa - pb).abs());
            }
            sum += worst as f64;
            max = max.max(worst);
            if worst > 24 {
                bad += 1;
            }
        }
        let npx = (g.width() * g.height()) as f64;
        println!("{name:18} mean {:5.2}  max {max:3}  off {:5.2}%", sum / npx, bad as f64 * 100.0 / npx);
        if let Ok(dir) = std::env::var("GPU_OUT") {
            let mut both = RgbaImage::new(g.width() * 2, g.height());
            image::imageops::overlay(&mut both, &cpu, 0, 0);
            image::imageops::overlay(&mut both, &g, g.width() as i64, 0);
            both.save(format!("{dir}/gpu_{}.png", name.replace(' ', "_"))).unwrap();
        }
    }
}

/// Whole-animation flatten (what export and the compressed preview use) on
/// the graphics card against the CPU, with timings. Needs a GPU or lavapipe.
#[test]
#[ignore]
fn gpu_flatten_matches_cpu() {
    use crate::effects::EffectSettings as E;
    let fonts = crate::text::builtin_fonts();
    let (w, h, n) = (320u32, 240u32, 24usize);
    let frames = (0..n)
        .map(|k| Frame::new(RgbaImage::from_fn(w, h, |x, y| Rgba([((x + k as u32 * 7) % 256) as u8, (y * 255 / h) as u8, ((x ^ y) % 256) as u8, if (x / 40 + y / 40) % 7 == 0 { 120 } else { 255 }])), 6))
        .collect();
    let base = Animation { frames, loop_count: None };
    let mut st = Stack::default();
    st.transform.resize_on = true;
    st.transform.rs_pct = 90.0;
    let fx = E { sepia: true, blur: true, swirl: true, vignette: true, polaroid: true, loop_wobble: true, intro: crate::motion::Transition::Iris, ..E::default() };
    st.push(LayerKind::Effects(Box::new(fx)));
    st.push(LayerKind::Text(crate::text::Annotation { text: "Export".into(), size: 30.0, x: 20, y: 30, color: [255, 255, 255, 255], ..Default::default() }));
    st.push(LayerKind::Censor(crate::effects::CensorSettings { x: 150, y: 80, w: 80, h: 60, angle: 15.0, ..Default::default() }));
    st.push(LayerKind::Shield(crate::effects::ShieldSettings::default()));
    let t0 = std::time::Instant::now();
    let cpu = st.flatten(&base, &fonts, None);
    let tc = t0.elapsed();
    let Some((r, name)) = crate::gpu_render::GpuRenderer::headless_named() else {
        println!("no GPU adapter");
        return;
    };
    crate::gpu_render::set_shared(r, name.clone());
    let _ = st.flatten(&base, &fonts, None); // shaders warm up
    let t0 = std::time::Instant::now();
    let gpu = st.flatten(&base, &fonts, None);
    let tg = t0.elapsed();
    assert_eq!(cpu.len(), gpu.len());
    let (mut worst, mut sum, mut px) = (0i32, 0f64, 0f64);
    for (a, b) in cpu.frames.iter().zip(&gpu.frames) {
        assert_eq!(a.image.dimensions(), b.image.dimensions());
        assert_eq!(a.delay_cs, b.delay_cs);
        for (p, q) in a.image.get().pixels().zip(b.image.get().pixels()) {
            let mut d = 0;
            for c in 0..4 {
                let (pa, qa) = if c == 3 { (p[3] as i32, q[3] as i32) } else { (p[c] as i32 * p[3] as i32 / 255, q[c] as i32 * q[3] as i32 / 255) };
                d = d.max((pa - qa).abs());
            }
            worst = worst.max(d);
            sum += d as f64;
            px += 1.0;
        }
    }
    println!("{name}: cpu {tc:?}, gpu {tg:?}; mean {:.3} max {worst}", sum / px);
    assert!(sum / px < 1.0, "frames drift from the CPU");
}


/// Ctrl+wheel reaches egui as a zoom factor, not a scroll (the timeline
/// zooms from `zoom_delta`).
#[test]
fn ctrl_wheel_is_zoom() {
    use eframe::egui;
    let ctx = egui::Context::default();
    let mut seen = (1.0f32, egui::Vec2::ZERO);
    for k in 0..3 {
        let mut raw = egui::RawInput::default();
        raw.time = Some(k as f64 * 0.05);
        if k == 1 {
            raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, 1.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: egui::Modifiers::CTRL });
        }
        let _ = ctx.run_ui(raw, |ui| {
            let z = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            if z.0 != 1.0 {
                seen = z;
            }
        });
    }
    assert!(seen.0 > 1.0, "Ctrl+wheel should zoom, got {seen:?}");
    assert_eq!(seen.1, egui::Vec2::ZERO, "and not scroll");
    println!("Ctrl+wheel: zoom factor {}", seen.0);
}

/// Markers, locks, keyframe easing and GIF splits survive a save.
#[test]
fn timeline_bits_round_trip() {
    use crate::project::{FrameMeta, ProjectFile};
    use crate::timeline::{Ease, Key, Marker};
    let mut f = ProjectFile::default();
    f.markers = vec![Marker { frame: 4, label: "Intro".into() }];
    f.frames = vec![FrameMeta { delay: 5, enabled: true, cut: false, blank: false }, FrameMeta { delay: 5, enabled: true, cut: true, blank: true }];
    let json = serde_json::to_string(&f).unwrap();
    let back: ProjectFile = serde_json::from_str(&json).unwrap();
    assert_eq!(back.markers, f.markers);
    assert!(back.frames[1].cut && !back.frames[0].cut);
    let k = Key { frame: 3, ease: Ease::Hold, ..Default::default() };
    let k2: Key = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
    assert_eq!(k2.ease, Ease::Hold);
    // older files: smooth true / false
    let old: Key = serde_json::from_str(r#"{"frame":1,"smooth":false}"#).unwrap();
    assert_eq!(old.ease, Ease::Linear);
    // a hold key keeps its value until the next one
    let mut t = crate::timeline::Timing::default();
    t.keys = vec![Key { frame: 0, x: 0.0, ease: Ease::Hold, ..Default::default() }, Key { frame: 10, x: 100.0, ..Default::default() }];
    assert_eq!(t.motion(9).dx, 0.0);
    assert_eq!(t.motion(10).dx, 100.0);
}

/// A layer added after the layer data was prepared (a panel adding one
/// mid-frame) renders instead of reading past the end.
#[test]
fn stale_prepared_layers() {
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_pixel(40, 30, Rgba([10, 200, 30, 255]));
    let mut base = Animation { frames: vec![Frame::new(img.clone(), 10), Frame::new(img.clone(), 10), Frame::new(img, 10)], loop_count: None };
    base.frames[1].blank = true;
    let mut st = Stack::default();
    st.push(LayerKind::Text(crate::text::Annotation { text: "hi".into(), ..Default::default() }));
    let prep = st.prepare(&base, &fonts);
    st.push(LayerKind::Effects(Box::new(crate::effects::EffectSettings { anim: true, counter: true, ..Default::default() })));
    let plan = st.plan(&base);
    let times = crate::stack::plan_times(&plan);
    for i in 0..plan.len() {
        let f = st.render_frame(&base, &plan, &times, i, &fonts, &prep);
        assert_eq!(f.dimensions(), (40, 30));
        assert!(st.gpu_frame(&base, &plan, &times, i, &fonts, &prep).is_none());
    }
}

/// An effect clipped to a shape changes the shape's pixels and nothing else.
#[test]
fn clipping_mask() {
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_pixel(60, 40, Rgba([200, 40, 40, 255]));
    let base = Animation { frames: vec![Frame::new(img, 10)], loop_count: None };
    let mut st = Stack::default();
    let shape = crate::shapes::ShapeSettings { x: 20, y: 10, w: 20, h: 20, fill: [0, 0, 255], stroke_on: false, ..Default::default() };
    st.push(LayerKind::Shape(Box::new(shape)));
    let id = st.push(LayerKind::Effects(Box::new(crate::effects::EffectSettings { negative: true, ..Default::default() })));
    st.layout_text(60, 40, &fonts);
    let plan = st.plan(&base);
    let times = crate::stack::plan_times(&plan);
    let render = |st: &Stack| {
        let prep = st.prepare(&base, &fonts);
        st.render_frame(&base, &plan, &times, 0, &fonts, &prep)
    };
    let free = render(&st);
    st.layers.iter_mut().find(|l| l.id == id).unwrap().clip = true;
    let clipped = render(&st);
    // outside the shape: the background as it was, not inverted
    assert_eq!(clipped.get_pixel(2, 2).0, [200, 40, 40, 255]);
    assert_ne!(free.get_pixel(2, 2).0, [200, 40, 40, 255]);
    // inside: inverted like the unclipped effect
    assert_eq!(clipped.get_pixel(30, 20).0, free.get_pixel(30, 20).0);
    assert!(st.gpu_frame(&base, &plan, &times, 0, &fonts, &st.prepare(&base, &fonts)).is_some());
}

/// Every look, style and filter across its strength range, on both paths:
/// neither should ever come out black or empty.
#[test]
#[ignore]
fn strength_sweep() {
    use crate::booth::{BoothFx, BoothItem};
    use crate::effects::EffectSettings as E;
    let gpu = crate::gpu_render::GpuRenderer::headless();
    let mut gpu = gpu;
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_fn(97, 61, |x, y| Rgba([(x * 2) as u8 + 40, (y * 3) as u8 + 30, ((x ^ y) * 3) as u8, 255]));
    let base = Animation { frames: vec![Frame::new(img, 10)], loop_count: None };
    let mut cases: Vec<(String, E)> = Vec::new();
    for a in [0.0f32, 1.0, 13.0, 50.0, 99.0, 100.0, 101.0, 150.0, 400.0, 2000.0] {
        for b in BoothFx::LOOKS {
            cases.push((format!("{} {a}", b.label()), E { looks: vec![BoothItem { fx: b, amount: a, ..Default::default() }], ..Default::default() }));
        }
        cases.push((format!("gotham {a}"), E { gotham: true, gotham_amount: a, ..Default::default() }));
        cases.push((format!("lomo {a}"), E { lomo: true, lomo_amount: a, ..Default::default() }));
        cases.push((format!("nashville {a}"), E { nashville: true, nashville_amount: a, ..Default::default() }));
        cases.push((format!("toaster {a}"), E { toaster: true, toaster_amount: a, ..Default::default() }));
        cases.push((format!("vignette {a}"), E { vignette: true, vignette_amount: a, ..Default::default() }));
        cases.push((format!("emboss {a}"), E { emboss: true, emboss_amount: a, ..Default::default() }));
    }
    let mut bad = 0;
    for (name, fx) in cases {
        let mut st = Stack::default();
        st.push(LayerKind::Effects(Box::new(fx)));
        let plan = st.plan(&base);
        let times = crate::stack::plan_times(&plan);
        let prep = st.prepare(&base, &fonts);
        let cpu = st.render_frame(&base, &plan, &times, 0, &fonts, &prep);
        let mean = |i: &RgbaImage| i.pixels().map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64 + p[3] as u64).sum::<u64>() / (i.width() * i.height()) as u64;
        let mut line = format!("{name:22} cpu {:4}", mean(&cpu));
        if let Some(g) = gpu.as_mut() {
            let job = st.gpu_frame(&base, &plan, &times, 0, &fonts, &prep).expect("gpu job");
            let tex = g.render(&base.frames[0].image.get(), 1, &job, 3);
            let out = g.read(&tex);
            let d: u64 = out.pixels().zip(cpu.pixels()).map(|(a, b)| (0..4).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs() as u64).sum::<u64>()).max().unwrap_or(0);
            line += &format!(" gpu {:4} maxdiff {d}", mean(&out));
            if d > 12 || mean(&out) < 20 {
                bad += 1;
                line += "  <<<";
            }
        }
        if mean(&cpu) < 20 {
            bad += 1;
            line += "  <<< cpu";
        }
        println!("{line}");
    }
    assert_eq!(bad, 0);
}

/// An animation clipped to a layer moves that layer: where it was, the
/// picture underneath shows again (no ghost of it left behind).
#[test]
fn clipped_animation_moves_its_base() {
    let fonts = crate::text::builtin_fonts();
    let img = RgbaImage::from_pixel(80, 60, Rgba([200, 40, 40, 255]));
    let base = Animation { frames: vec![Frame::new(img, 10)], loop_count: None };
    let mut st = Stack::default();
    let shape = crate::shapes::ShapeSettings { x: 30, y: 20, w: 20, h: 20, fill: [0, 0, 255], stroke_on: false, ..Default::default() };
    st.push(LayerKind::Shape(Box::new(shape)));
    // slides in from the left: on its first frame the shape is far to the left
    let id = st.push(LayerKind::Effects(Box::new(crate::effects::EffectSettings { anim: true, intro: crate::motion::Transition::SlideRight, fade_frames: 4, ..Default::default() })));
    st.layers.iter_mut().find(|l| l.id == id).unwrap().clip = true;
    st.layout_text(80, 60, &fonts);
    let plan = st.plan(&base);
    let times = crate::stack::plan_times(&plan);
    let prep = st.prepare(&base, &fonts);
    let out = st.render_frame(&base, &plan, &times, 0, &fonts, &prep);
    // where the shape was: the background, not the shape and not black
    assert_eq!(out.get_pixel(40, 30).0, [200, 40, 40, 255], "ghost left behind");
    // the rest of the frame untouched
    assert_eq!(out.get_pixel(2, 2).0, [200, 40, 40, 255]);
}

/// The card's preview shrink is the CPU's triangle resize.
#[test]
#[ignore]
fn gpu_shrink_matches_cpu() {
    let Some(mut gpu) = crate::gpu_render::GpuRenderer::headless() else { return };
    let img = RgbaImage::from_fn(333, 211, |x, y| Rgba([(x * 3) as u8, (y * 5) as u8, ((x * y) % 251) as u8, if (x + y) % 9 == 0 { 70 } else { 255 }]));
    for (w, h) in [(166u32, 105u32), (100, 63), (333, 211), (40, 25)] {
        let cpu = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        let g = gpu.resize_triangle(&[&img], w, h).unwrap().pop().unwrap();
        let max = cpu.pixels().zip(g.pixels()).map(|(a, b)| (0..4).map(|k| (a[k] as i32 - b[k] as i32).abs()).max().unwrap()).max().unwrap();
        println!("{w}x{h}: max diff {max}");
        assert!(max <= 1, "{w}x{h} differs by {max}");
    }
}

/// A GIF made with the card mapping colours is byte for byte the one the
/// CPU makes.
#[test]
#[ignore]
fn gif_same_with_gpu_mapping() {
    let frames: Vec<Frame> = (0..4u32).map(|k| Frame::new(RgbaImage::from_fn(400, 300, move |x, y| Rgba([((x + k * 9) % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, if x < 5 { 0 } else { 255 }])), 6)).collect();
    let anim = Animation { frames, loop_count: Some(0) };
    for ordered in [false, true] {
        let opt = crate::encode::GifOptions { ordered_dither: ordered, ..Default::default() };
        let cpu = crate::encode::encode_gif(&anim, &opt, None).unwrap();
        let Some((r, name)) = crate::gpu_render::GpuRenderer::headless_named() else { return };
        crate::gpu_render::set_shared(r, name);
        let before = crate::gpu_render::MAPPED.load(std::sync::atomic::Ordering::Relaxed);
        let gpu = crate::encode::encode_gif(&anim, &opt, None).unwrap();
        crate::gpu_render::clear_shared();
        assert!(crate::gpu_render::MAPPED.load(std::sync::atomic::Ordering::Relaxed) > before, "the card wasn't used");
        println!("ordered {ordered}: cpu {} bytes, gpu {} bytes", cpu.len(), gpu.len());
        assert!(cpu == gpu, "the files differ");
    }
}

/// An outro sits at the end of a layer however its clips are cut up and
/// moved, not at the end of each clip.
#[test]
fn outro_at_the_layers_end() {
    let fonts = crate::text::builtin_fonts();
    let base = Animation { frames: (0..20).map(|_| Frame::new(RgbaImage::from_pixel(8, 8, Rgba([200, 200, 200, 255])), 10)).collect(), loop_count: None };
    let mut st = Stack::default();
    let id = st.push(LayerKind::Effects(Box::new(crate::effects::EffectSettings { anim: true, outro: crate::motion::Transition::Fade, fade_frames: 3, ..Default::default() })));
    let l = st.layers.iter_mut().find(|l| l.id == id).unwrap();
    l.timing.clips = Some(vec![Clip { start: 0, end: Some(9) }, Clip { start: 10, end: None }]);
    let plan = st.plan(&base);
    let times = crate::stack::plan_times(&plan);
    let prep = st.prepare(&base, &fonts);
    let px = |i: usize| st.render_frame(&base, &plan, &times, i, &fonts, &prep).get_pixel(4, 4).0;
    assert_eq!(px(9), [200, 200, 200, 255], "the first clip's end isn't faded");
    assert_ne!(px(19), [200, 200, 200, 255], "the layer's end is");
}

/// A looping motion keeps moving over a stretch of identical frames (a
/// held picture), whether they're separate copies or one shared picture.
#[test]
fn motion_continues_over_held_frames() {
    use crate::anim::{Animation, Frame};
    let img = image::RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 4) as u8, (y * 5) as u8, 90, 255]));
    let one = Frame::new(img.clone(), 10);
    for shared in [false, true] {
        let frames: Vec<Frame> = (0..8).map(|_| if shared { one.clone() } else { Frame::new(img.clone(), 10) }).collect();
        let mut base = Animation { frames, loop_count: None };
        for f in &mut base.frames {
            f.image = f.image.packed();
        }
        let mut st = crate::stack::Stack::default();
        st.push(crate::stack::LayerKind::Effects(Box::new(crate::effects::EffectSettings { loop_hop: true, loop_amount: 100.0, loop_cycles: 2, ..Default::default() })));
        let out = st.flatten(&base, &[], None);
        let same = out.frames.windows(2).filter(|w| w[0].image.get().as_raw() == w[1].image.get().as_raw()).count();
        assert!(same <= 1, "shared {shared}: {same} of 7 steps didn't move");
    }
}
