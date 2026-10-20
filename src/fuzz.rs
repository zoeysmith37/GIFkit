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

const VARIANTS: &[&str] = &["ActualSize","Apng","Arrow","Avif","Bicubic","Bilinear","Blinds","Blur","Bottom","BottomLeft","BottomRight","Bubble","Bulge","Burst","Censor","Center","CenterCrop","Comic","Dark","DelayAll","DeleteObject","Dent","Drop","Effects","Ellipse","Emoji","Export","Fade","FishEye","Fit","FitPad","Fps","FullScreen","Gif","Glitch","Glow","Handbook","Heart","Heavy","Highlighter","History","Hop","Iris","Kaleidoscope","Lanczos3","Left","Light","Line","Markup","Mirror","Mp4","Nearest","New","NextFrame","None","Open","Overlay","Pen","Pencil","Percent","Pixelate","PlayPause","Png","PopArt","PrevFrame","Pulse","Quit","Rainbow","Rect","Redo","Right","Rounded","Save","SaveAs","Shake","Shape","Shield","SlideDown","SlideLeft","SlideRight","SlideUp","Solid","Speech","Spin","SplitView","Squeeze","Star","Stretch","System","Text","Thermal","Thought","TlDelete","TlKey","TlRazor","TlSplit","Top","TopLeft","TopRight","Triangle","Tunnel","Twirl","Undo","Watermark","WebmAlpha","WipeDown","WipeLeft","WipeRight","WipeUp","Wobble","XRay","Zoom","ZoomIn","ZoomOut",];
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
            f
        })
        .collect();
    Animation { frames, loop_count: r.pick(&[None, Some(0), Some(1), Some(3)]) }
}

fn layer(r: &mut Rng, w: u32, h: u32, n: usize, p: f64) -> LayerKind {
    use crate::{effects, shapes, text};
    match r.below(10) {
        0 | 1 => LayerKind::Effects(Box::new(fuzz(&effects::EffectSettings::default(), r, p))),
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

fn timing(r: &mut Rng, n: usize) -> crate::timeline::Timing {
    let mut t = crate::timeline::Timing::default();
    if r.chance(0.5) {
        let k = r.below(4);
        t.clips = Some((0..k).map(|_| { let e = r.below(n + 3); Clip { start: r.below(n + 3), end: r.pick(&[None, Some(e), Some(0)]) } }).collect());
    }
    if r.chance(0.4) {
        for _ in 0..r.below(4) {
            t.keys.push(Key { frame: r.below(n + 4), x: r.pick(&[0.0, 30.0, -500.0]), y: r.pick(&[0.0, 12.0]), scale: r.pick(&[0.0, 50.0, 100.0, 400.0, -20.0]), angle: r.pick(&[0.0, 90.0, 720.0]), opacity: r.pick(&[0.0, 50.0, 100.0, 300.0]), smooth: r.chance(0.5) });
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
        if let Some(l) = st.layers.iter_mut().find(|l| l.id == id) {
            l.timing = tm;
            l.visible = vis;
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
    set("proxy");
    if w > 4 && h > 4 {
        let s = 0.5;
        let (pw, ph) = crate::stack::proxy_size(w, h, s);
        let small = Animation { frames: base.frames.iter().map(|f| Frame { image: Arc::new(image::imageops::resize(&*f.image, pw, ph, image::imageops::FilterType::Triangle)), ..f.clone() }).collect(), loop_count: base.loop_count };
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
        if shown == 0 {
            continue;
        }
        let _ = crate::io::encode_preview(&flat, &f);
    }
    desc
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
            let enc = if flat.frames.iter().any(|f| f.enabled) { crate::io::encode_preview(&flat, &f).map(|(_, b)| b.len()).map_err(|e| e.to_string()) } else { Err("no frames".into()) };
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
