//! Engine tests (no GUI). Run with `cargo test --release`.

use crate::anim::{Animation, Frame};
use crate::effects::{self, EffectSettings};
use crate::encode::{encode_gif, GifOptions};
use crate::{io, ops, overlay, text};
use image::{Rgba, RgbaImage};
use std::sync::atomic::AtomicUsize;

fn synth(transparent: bool) -> Animation {
    let (w, h) = (96u32, 64u32);
    let frames = (0..12)
        .map(|i| {
            let mut img = if transparent { RgbaImage::new(w, h) } else { RgbaImage::from_fn(w, h, |x, y| Rgba([(x * 2) as u8, (y * 3) as u8, 120, 255])) };
            // moving square
            for y in 20..40 {
                for x in (i * 5)..(i * 5 + 20) {
                    if x < w {
                        img.put_pixel(x, y, Rgba([255, 40, 40, 255]));
                    }
                }
            }
            Frame::new(img, 5 + (i % 3) as u16)
        })
        .collect();
    Animation { frames, loop_count: None }
}

fn max_diff(a: &Animation, b: &Animation) -> u32 {
    let mut m = 0;
    for (fa, fb) in a.frames.iter().zip(&b.frames) {
        for (pa, pb) in fa.image.pixels().zip(fb.image.pixels()) {
            if pa[3] < 128 && pb[3] < 128 {
                continue;
            }
            for k in 0..4 {
                m = m.max((pa[k] as i32 - pb[k] as i32).unsigned_abs());
            }
        }
    }
    m
}

#[test]
fn roundtrip_gif_lossless_options() {
    for transparent in [false, true] {
        let a = synth(transparent);
        for opt in [
            GifOptions::default(),
            GifOptions { frame_diff: false, ..Default::default() },
            GifOptions { global_palette: true, ..Default::default() },
            GifOptions { dither: true, ..Default::default() },
            GifOptions { force_custom_lzw: true, ..Default::default() },
            GifOptions { force_custom_lzw: true, frame_diff: false, ..Default::default() },
        ] {
            let bytes = encode_gif(&a, &opt, None).unwrap();
            let b = io::load_animation_bytes(&bytes).unwrap();
            assert_eq!(b.len(), a.len(), "frame count {opt:?}");
            assert_eq!((b.width(), b.height()), (a.width(), a.height()));
            let d = max_diff(&a, &b);
            eprintln!("transparent={transparent} diff={d} size={} {opt:?}", bytes.len());
            // palette quantisation may shift colours slightly but structure must hold
            assert!(d < 200, "transparent={transparent} opt={opt:?} diff={d}");
            for (fa, fb) in a.frames.iter().zip(&b.frames) {
                assert_eq!(fa.delay_cs, fb.delay_cs);
            }
        }
    }
}

#[test]
fn lossy_and_merge_shrink() {
    let a = synth(false);
    let base = encode_gif(&a, &GifOptions { frame_diff: false, merge_duplicates: false, ..Default::default() }, None).unwrap();
    let diffed = encode_gif(&a, &GifOptions::default(), None).unwrap();
    assert!(diffed.len() < base.len(), "diff {} vs full {}", diffed.len(), base.len());
    // duplicates merge
    let mut d = a.clone();
    d.frames.insert(3, d.frames[2].clone());
    let bytes = encode_gif(&d, &GifOptions::default(), None).unwrap();
    let back = io::load_animation_bytes(&bytes).unwrap();
    assert_eq!(back.len(), a.len());
    assert_eq!(back.frames[2].delay_cs, d.frames[2].delay_cs * 2);
}

#[test]
fn loop_count_roundtrip() {
    let mut a = synth(false);
    a.loop_count = Some(3);
    let b = io::load_animation_bytes(&encode_gif(&a, &GifOptions::default(), None).unwrap()).unwrap();
    assert_eq!(b.loop_count, Some(3));
    a.loop_count = None;
    let b = io::load_animation_bytes(&encode_gif(&a, &GifOptions::default(), None).unwrap()).unwrap();
    assert_eq!(b.loop_count, None);
}

#[test]
fn load_real_gif_if_present() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test.gif");
    if !p.exists() {
        return;
    }
    let a = io::load_animation(&p).unwrap();
    assert!(a.len() > 1);
    let opt = GifOptions { lossy: 60, ..Default::default() };
    let t = std::time::Instant::now();
    let bytes = encode_gif(&a, &opt, None).unwrap();
    eprintln!("real gif: {} frames {}x{} -> {} bytes in {:?}", a.len(), a.width(), a.height(), bytes.len(), t.elapsed());
}

#[test]
fn apng_roundtrip() {
    let a = synth(true);
    let b = io::load_animation_bytes(&io::encode_apng(&a, true).unwrap()).unwrap();
    assert_eq!(b.len(), a.len());
    assert_eq!(max_diff(&a, &b), 0);
}

#[test]
fn geometry_ops() {
    let a = synth(false);
    assert_eq!(ops::rotate90(&a).width(), a.height());
    let r = ops::resize(&a, 48, 48, ops::AspectMode::CenterCrop, ops::ResizeFilter::Lanczos3);
    assert_eq!((r.width(), r.height()), (48, 48));
    let r = ops::resize(&a, 48, 48, ops::AspectMode::FitPad, ops::ResizeFilter::Nearest);
    assert_eq!((r.width(), r.height()), (48, 48));
    let c = ops::crop(&a, 10, 10, 30, 20);
    assert_eq!((c.width(), c.height()), (30, 20));
    let rr = ops::rotate_any(&a, 45.0, Rgba([0, 0, 0, 0]), true);
    assert!(rr.width() > a.width());
    assert_eq!(ops::boomerang(&a).len(), a.len() * 2 - 2);
    assert_eq!(ops::cut_frames(&a, 2, 5).len(), 4);
    assert_eq!(ops::crossfade(&a, 2, true).len(), a.len() * 3);
    let t = ops::auto_trim_rect(&synth(true), 0).unwrap();
    assert_eq!((t.1, t.3), (20, 20));
    let s = ops::change_speed(&a, 200.0);
    assert!(s.duration_secs() < a.duration_secs());
    let (s0, e0) = ops::time_to_frame_range(&a, 0.0, 0.2);
    assert!(s0 == 0 && e0 >= 2);
    assert_eq!(ops::drop_every_nth(&a, 2).len(), a.len() / 2);
}

#[test]
fn every_effect_runs() {
    let a = synth(false);
    let fonts = text::builtin_fonts();
    let p = AtomicUsize::new(0);
    let mut all = EffectSettings::default();
    // flip every bool on one at a time
    macro_rules! each {
        ($($f:ident),*) => {$(
            let s = EffectSettings { $f: true, ..Default::default() };
            let out = effects::apply(&a, &s, &fonts, &p);
            assert!(!out.is_empty(), stringify!($f));
            let w = out.width(); let h = out.height();
            assert!(out.frames.iter().all(|f| f.image.dimensions() == (w, h)), stringify!($f));
            all.$f = true;
        )*};
    }
    each!(grayscale, sepia, monochrome, negative, tint, change_bg, flip_v, flip_h, rot90, rot270, rot_any, rc_white, rc_black, rc_custom,
        reverse, set_loop, boomerang, counter, fade_in, fade_out, blur, sharpen, ordered_dither, error_dither, oil, emboss, posterize,
        solarize, noise, swirl, implode, wave, gotham, lomo, nashville, toaster, vignette, polaroid, frame_camera, frame_fuzzy,
        frame_rounded, frame_border);
    all.rot90 = false;
    let out = effects::apply(&a, &all, &fonts, &p);
    assert!(!out.is_empty());
    let s = EffectSettings { hue: 150.0, saturation: 50.0, lightness: 120.0, brightness: 20.0, contrast: 30.0, ..Default::default() };
    effects::apply(&a, &s, &fonts, &p);
}

#[test]
fn replace_white_makes_transparent() {
    let mut img = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 254, 255]));
    img.put_pixel(10, 10, Rgba([0, 0, 0, 255]));
    effects::replace_color(&mut img, [255, 255, 255], 2.0, Some(2));
    assert_eq!(img.get_pixel(0, 0)[3], 0);
    assert_eq!(img.get_pixel(10, 10)[3], 255);
}

#[test]
fn text_and_censor_and_overlay() {
    let a = synth(false);
    let fonts = text::builtin_fonts();
    let ann = text::Annotation { text: "Hello\nworld".into(), color: [255, 255, 255, 255], ..Default::default() };
    let s = text::render_sprite(&fonts, &ann);
    assert!(s.width() > 20 && s.height() > 40);
    assert!(s.pixels().any(|p| p[3] == 255 && p[0] == 255));
    let mut img = (*a.frames[0].image).clone();
    text::draw_sprite(&mut img, &s, &ann);
    assert_ne!(img.as_raw(), a.frames[0].image.as_raw());

    let c = effects::CensorSettings { x: 0, y: 0, w: 40, h: 40, ..Default::default() };
    let out = effects::censor_frame(&a.frames[0].image, 0, &c);
    assert_ne!(out.as_raw(), a.frames[0].image.as_raw());

    let src = overlay::OverlaySource { name: "x".into(), path: None, frames: vec![RgbaImage::from_pixel(10, 10, Rgba([0, 255, 0, 255]))], delays: vec![10] };
    let pl = vec![overlay::Placement { enabled: true, x: 5, y: 5, w: 20, h: 20, angle: 0.0 }; a.len()];
    let o = overlay::apply(&a, &src, &pl, 1.0);
    assert_eq!(*o.frames[0].image.get_pixel(10, 10), Rgba([0, 255, 0, 255]));
}

#[test]
fn exports() {
    let dir = std::env::temp_dir().join(format!("gifkit_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = synth(true);
    assert_eq!(io::export_frames_dir(&a, &dir.join("png"), "f", io::SplitFormat::Png).unwrap(), 12);
    assert_eq!(io::export_frames_dir(&a, &dir.join("gif"), "f", io::SplitFormat::Gif).unwrap(), 12);
    assert_eq!(io::export_frames_zip(&a, &dir.join("f.zip"), "f").unwrap(), 12);
    let written = io::export_spritesheet(&a, &dir.join("sheet.png"), &io::SheetOptions::default()).unwrap();
    assert!(written.contains("sheet_frames.tres"));
    let tres = std::fs::read_to_string(dir.join("sheet_frames.tres")).unwrap();
    assert!(tres.contains("SpriteFrames") && tres.contains("AtlasTexture_11"));
    let sheet = image::open(dir.join("sheet.png")).unwrap();
    assert!(sheet.width().is_power_of_two() && sheet.height().is_power_of_two());
    io::export_spritesheet(&a, &dir.join("sheet2.png"), &io::SheetOptions { pow2: false, ..Default::default() }).unwrap();
    assert_eq!(image::open(dir.join("sheet2.png")).unwrap().width(), 96 * 4);
    if io::ffmpeg_available() {
        io::export_video(&synth(false), &dir.join("v.mp4"), io::VideoFormat::Mp4, [255, 255, 255], false).unwrap();
        assert!(std::fs::metadata(dir.join("v.mp4")).unwrap().len() > 0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn bench_real_gif() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test.gif");
    let a = io::load_animation(&p).unwrap();
    for (name, o) in [
        ("default", GifOptions::default()),
        ("nomerge", GifOptions { merge_duplicates: false, ..Default::default() }),
        ("global", GifOptions { global_palette: true, ..Default::default() }),
        ("lossy40", GifOptions { lossy: 40, ..Default::default() }),
        ("lossy40 global", GifOptions { lossy: 40, global_palette: true, ..Default::default() }),
        ("lossy80 global 128", GifOptions { lossy: 80, global_palette: true, colors: 128, ..Default::default() }),
        ("nodiff", GifOptions { frame_diff: false, ..Default::default() }),
        ("custom lzw", GifOptions { force_custom_lzw: true, ..Default::default() }),
        ("lzwlossy120 global", GifOptions { lossy: 120, global_palette: true, ..Default::default() }),
    ] {
        let t = std::time::Instant::now();
        let b = encode_gif(&a, &o, None).unwrap();
        let back = io::load_animation_bytes(&b).unwrap();
        eprintln!("{name:20} {:>8} bytes  {:?}  maxdiff {}", b.len(), t.elapsed(), max_diff(&a, &back));
    }
}

#[test]
fn custom_lzw_exact_large() {
    // big noisy frame forces several dictionary resets
    let mut img = RgbaImage::new(400, 300);
    let mut z: u32 = 12345;
    for p in img.pixels_mut() {
        z = z.wrapping_mul(1103515245).wrapping_add(12345);
        *p = Rgba([(z >> 16) as u8 & 0xf0, (z >> 8) as u8 & 0xe0, (z >> 24) as u8, 255]);
    }
    let a = Animation { frames: vec![Frame::new(img.clone(), 10), Frame::new(img, 10)], loop_count: None };
    for colors in [2u16, 4, 16, 200, 256] {
        let o = GifOptions { force_custom_lzw: true, colors, frame_diff: false, merge_duplicates: false, ..Default::default() };
        let b = encode_gif(&a, &o, None).unwrap();
        let o2 = GifOptions { colors, frame_diff: false, merge_duplicates: false, ..Default::default() };
        let b2 = encode_gif(&a, &o2, None).unwrap();
        let d1 = io::load_animation_bytes(&b).unwrap();
        let d2 = io::load_animation_bytes(&b2).unwrap();
        assert_eq!(max_diff(&d1, &d2), 0, "colors {colors}");
    }
}

#[test]
fn live_stack_matches_baked_and_plans() {
    use crate::stack::{LayerKind, SpeedMode, Stack};
    let a = synth(false);
    let fonts = text::builtin_fonts();
    let mut s = Stack::default();
    assert!(!s.needs_render());
    assert_eq!(max_diff(&a, &s.flatten(&a, &fonts, None)), 0);

    let fx = EffectSettings { grayscale: true, ..Default::default() };
    s.push(LayerKind::Effects(Box::new(fx)));
    s.push(LayerKind::Text(text::Annotation { text: "hi".into(), ..Default::default() }));
    s.push(LayerKind::Censor(effects::CensorSettings { x: 0, y: 0, w: 10, h: 10, ..Default::default() }));
    s.transform.boomerang = true;
    let plan = s.plan(&a);
    assert_eq!(plan.len(), a.len() * 2 - 2);
    let flat = s.flatten(&a, &fonts, None);
    assert_eq!(flat.len(), plan.len());
    let times = crate::stack::plan_times(&plan);
    let prep = s.prepare(&a, &fonts);
    let f5 = s.render_frame(&a, &plan, &times, 5, &fonts, &prep);
    assert_eq!(f5.as_raw(), flat.frames[5].image.as_raw());

    // layer order matters: text above grayscale keeps colour, below it doesn't
    let red = |img: &image::RgbaImage| img.pixels().any(|p| p[0] > 200 && p[1] < 60);
    let mut s2 = Stack::default();
    s2.push(LayerKind::Effects(Box::new(EffectSettings { grayscale: true, ..Default::default() })));
    s2.push(LayerKind::Text(text::Annotation { text: "XX".into(), color: [255, 0, 0, 255], border: text::Border::None, x: 0, y: 0, ..Default::default() }));
    assert!(red(&s2.flatten(&a, &fonts, None).frames[0].image));
    s2.layers.swap(0, 1);
    assert!(!red(&s2.flatten(&a, &fonts, None).frames[0].image));

    // transform: cut + speed + crop + resize + rotate
    let mut s3 = Stack::default();
    s3.transform.cut_on = true;
    s3.transform.cut_start = 3;
    s3.transform.cut_end = 6;
    s3.transform.speed_on = true;
    s3.transform.speed_mode = SpeedMode::Percent;
    s3.transform.speed_val = 200.0;
    s3.transform.crop_on = true;
    s3.transform.crop = [10, 10, 40, 30];
    s3.transform.resize_on = true;
    s3.transform.rs_pct = 200.0;
    s3.transform.rot_on = true;
    s3.transform.rot = 90;
    let out = s3.flatten(&a, &fonts, None);
    assert_eq!(out.len(), 4);
    assert_eq!((out.width(), out.height()), (60, 80));
    assert_eq!(out.frames[0].delay_cs, ((a.frames[2].delay_cs as f32) / 2.0).round() as u16);

    // hidden layers have no effect
    for l in &mut s.layers {
        l.visible = false;
    }
    s.transform.boomerang = false;
    assert!(!s.needs_render());
    assert_eq!(crate::stack::frame_at_time(&a, 0.0), 0);
}

#[test]
fn layers_follow_frame_edits() {
    use crate::stack::{LayerKind, Stack};
    let a = synth(false); // 96x64
    let fonts = text::builtin_fonts();
    let red = |p: &image::Rgba<u8>| p[0] > 240 && p[1] < 15 && p[2] < 15;
    let mut s = Stack::default();
    s.push(LayerKind::Censor(effects::CensorSettings { x: 40, y: 40, w: 20, h: 10, method: effects::CensorMethod::Solid, color: [255, 0, 0], ..Default::default() }));
    // centre of the region in source space: (50, 45)
    s.transform.rot_on = true;
    s.transform.rot = 90; // → x' = 64 - y, y' = x
    let out = s.flatten(&a, &fonts, None);
    assert_eq!((out.width(), out.height()), (64, 96));
    assert!(red(out.frames[0].image.get_pixel(64 - 45, 50)), "censor did not follow 90° rotation");
    // rotated region is 10 wide, 20 tall now
    assert!(red(out.frames[0].image.get_pixel(64 - 45, 50 + 8)));
    assert!(!red(out.frames[0].image.get_pixel(64 - 45 + 8, 50)));

    // crop + resize + free rotation
    s.transform.rot = 0;
    s.transform.crop_on = true;
    s.transform.crop = [20, 20, 60, 40];
    s.transform.resize_on = true;
    s.transform.rs_pct = 200.0;
    s.transform.rot_any = true;
    s.transform.rot_deg = 30.0;
    let g = s.transform.geo_map(96, 64);
    let (cx, cy) = g.m.apply(50.0, 45.0);
    let out = s.flatten(&a, &fonts, None);
    assert!(red(out.frames[0].image.get_pixel(cx.round() as u32, cy.round() as u32)), "censor did not follow crop/resize/rotate");

    // an effects layer that rotates (below the censor) is followed too
    let mut s2 = Stack::default();
    s2.push(LayerKind::Effects(Box::new(EffectSettings { rot90: true, ..Default::default() })));
    s2.push(LayerKind::Censor(effects::CensorSettings { x: 40, y: 40, w: 20, h: 10, method: effects::CensorMethod::Solid, color: [255, 0, 0], ..Default::default() }));
    let out = s2.flatten(&a, &fonts, None);
    assert!(red(out.frames[0].image.get_pixel(64 - 45, 50)), "censor did not follow an effects-layer rotation");

    // rotated layer itself: 90° censor is taller than wide
    let mut s3 = Stack::default();
    s3.push(LayerKind::Censor(effects::CensorSettings { x: 40, y: 40, w: 20, h: 6, angle: 90.0, method: effects::CensorMethod::Solid, color: [255, 0, 0], ..Default::default() }));
    let out = s3.flatten(&a, &fonts, None);
    assert!(red(out.frames[0].image.get_pixel(50, 52)));
    assert!(!red(out.frames[0].image.get_pixel(58, 43)));
}

#[test]
fn pixelate_censor_on_real_gif() {
    use crate::stack::{LayerKind, Stack};
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test.gif");
    let a = io::load_animation(&p).unwrap();
    let fonts = text::builtin_fonts();
    let mut s = Stack::default();
    s.push(LayerKind::Censor(effects::CensorSettings { x: 67, y: 99, w: 74, h: 47, ..Default::default() }));
    let plan = s.plan(&a);
    let times = crate::stack::plan_times(&plan);
    let prep = s.prepare(&a, &fonts);
    let img = s.render_frame(&a, &plan, &times, 37, &fonts, &prep);
    let diff = img.pixels().zip(a.frames[37].image.pixels()).filter(|(x, y)| x != y).count();
    assert!(diff > 100, "pixelate changed only {diff} pixels");
}

#[test]
#[ignore]
fn bench_effects_per_frame() {
    use crate::stack::{LayerKind, Stack};
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test.gif");
    let a = io::load_animation(&p).unwrap();
    let fonts = text::builtin_fonts();
    macro_rules! each {
        ($($f:ident),*) => {$(
            let mut s = Stack::default();
            s.push(LayerKind::Effects(Box::new(EffectSettings { $f: true, ..Default::default() })));
            let plan = s.plan(&a);
            let times = crate::stack::plan_times(&plan);
            let prep = s.prepare(&a, &fonts);
            let t = std::time::Instant::now();
            for i in 0..10 { let _ = s.render_frame(&a, &plan, &times, i, &fonts, &prep); }
            eprintln!("{:16} {:7.2} ms/frame", stringify!($f), t.elapsed().as_secs_f64() * 100.0);
        )*};
    }
    each!(grayscale, tint, blur, sharpen, oil, emboss, noise, swirl, implode, wave, gotham, lomo, nashville, toaster, vignette, polaroid, frame_camera, frame_fuzzy, frame_rounded, rot_any, error_dither, ordered_dither, rc_white, counter);
    let mut s = Stack::default();
    s.push(LayerKind::Text(text::Annotation { text: "Hello".into(), ..Default::default() }));
    let t = std::time::Instant::now();
    for _ in 0..10 { let _ = s.prepare(&a, &fonts); }
    eprintln!("prepare(text)    {:7.2} ms", t.elapsed().as_secs_f64() * 100.0);
}


#[test]
fn watermark_single_and_tiled() {
    let fonts = text::builtin_fonts();
    let a = synth(false);
    let (w, h) = (a.width() as f32, a.height() as f32);
    let mut wm = effects::WatermarkSettings { text: "WM".into(), size: 20.0, opacity: 100.0, ..Default::default() };
    let sp = wm.sprite(&fonts, w, h).expect("sprite");
    match &sp {
        effects::WatermarkSprite::Single(s, x, y) => {
            // bottom-right anchor: sprite ends near the right / bottom edges
            assert!(*x + s.width() as i64 <= w as i64 && *x + (s.width() as i64) > (w as i64) * 3 / 4);
            assert!(*y + s.height() as i64 <= h as i64);
        }
        _ => panic!("expected single"),
    }
    let mut img = (*a.frames[0].image).clone();
    effects::draw_watermark(&mut img, &sp, wm.opacity);
    assert_ne!(img.as_raw(), a.frames[0].image.as_raw());
    // top-left corner untouched by a bottom-right watermark
    assert_eq!(img.get_pixel(1, 1), a.frames[0].image.get_pixel(1, 1));

    wm.tiled = true;
    let sp = wm.sprite(&fonts, w, h).unwrap();
    let mut img = (*a.frames[0].image).clone();
    effects::draw_watermark(&mut img, &sp, 100.0);
    let changed = img.pixels().zip(a.frames[0].image.pixels()).filter(|(p, q)| p != q).count();
    assert!(changed > (w * h) as usize / 20, "tiled watermark should cover the frame, changed {changed}");

    // empty text = inactive
    let none = effects::WatermarkSettings { text: "  ".into(), ..Default::default() };
    assert!(none.sprite(&fonts, w, h).is_none());
}

#[test]
fn shield_is_subtle_and_deterministic() {
    let a = synth(false);
    let s = effects::ShieldSettings { strength: 6.0, ..Default::default() };
    let src = &a.frames[0].image;
    let o1 = effects::shield(src, &s, 0);
    let o2 = effects::shield(src, &s, 5);
    assert_eq!(o1.as_raw(), o2.as_raw(), "same seed, static pattern");
    let mut maxd = 0i32;
    let mut changed = 0;
    for (p, q) in o1.pixels().zip(src.pixels()) {
        for c in 0..3 {
            let d = (p[c] as i32 - q[c] as i32).abs();
            maxd = maxd.max(d);
            if d > 0 {
                changed += 1;
            }
        }
    }
    assert!(changed > (src.width() * src.height()) as usize, "perturbs most pixels");
    assert!(maxd <= 12, "stays subtle, max diff {maxd}");
    let pf = effects::ShieldSettings { per_frame: true, ..s.clone() };
    assert_ne!(effects::shield(src, &pf, 0).as_raw(), effects::shield(src, &pf, 1).as_raw());
}

#[test]
fn watermark_and_shield_layers_render_in_stack() {
    use crate::stack::{LayerKind, Stack};
    let fonts = text::builtin_fonts();
    let a = synth(false);
    let mut st = Stack::default();
    st.push(LayerKind::Shield(effects::ShieldSettings::default()));
    st.push(LayerKind::Watermark(Box::new(effects::WatermarkSettings { opacity: 100.0, ..Default::default() })));
    assert!(st.needs_render());
    let out = st.flatten(&a, &fonts, None);
    assert_eq!(out.len(), a.len());
    assert_ne!(out.frames[0].image.as_raw(), a.frames[0].image.as_raw());
}

#[test]
fn preset_roundtrip_json() {
    use crate::presets::{Preset, PresetLayer};
    let mut tr = crate::stack::Transform::default();
    tr.speed_on = true;
    tr.speed_val = 150.0;
    let p = Preset {
        name: "Test".into(),
        src_size: (96, 64),
        transform: Some(tr),
        layers: Some(vec![
            PresetLayer::Effects { visible: true, fx: EffectSettings { grayscale: true, ..Default::default() } },
            PresetLayer::Watermark { visible: true, watermark: Default::default() },
            PresetLayer::Shield { visible: false, shield: Default::default() },
        ]),
        gif: Some(GifOptions { lossy: 40, ..Default::default() }),
    };
    let j = serde_json::to_string(&p).unwrap();
    let q: Preset = serde_json::from_str(&j).unwrap();
    assert_eq!(q.name, "Test");
    assert_eq!(q.transform.as_ref().unwrap().speed_val, 150.0);
    assert_eq!(q.layers.as_ref().unwrap().len(), 3);
    assert_eq!(q.gif.as_ref().unwrap().lossy, 40);
    assert!(q.summary().contains("3 layers"));
}

#[test]
fn noai_tag_written() {
    let a = synth(false);
    let opts = GifOptions { noai_tag: true, ..Default::default() };
    let b = encode_gif(&a, &opts, None).unwrap();
    let needle = b"noai";
    assert!(b.windows(needle.len()).any(|w| w == needle));
    let b = io::encode_apng(&a, true).unwrap();
    assert!(b.windows(needle.len()).any(|w| w == needle));
    assert_eq!(io::load_animation_bytes(&b).unwrap().len(), a.len());
}

/// Writes a sample preset into the config folder (used for UI screenshots).
#[test]
#[ignore]
fn write_demo_preset() {
    use crate::presets::{Preset, PresetLayer};
    let mut tr = crate::stack::Transform::default();
    tr.rot_on = true;
    tr.flip_h = true;
    let p = Preset {
        name: "Sepia + watermark".into(),
        src_size: (320, 240),
        transform: Some(tr),
        layers: Some(vec![
            PresetLayer::Effects { visible: true, fx: EffectSettings { sepia: true, vignette: true, ..Default::default() } },
            PresetLayer::Text { visible: true, text: crate::text::Annotation { text: "PRESET".into(), size: 30.0, x: 20, y: 20, ..Default::default() }, font: None },
            PresetLayer::Watermark { visible: true, watermark: effects::WatermarkSettings { tiled: true, opacity: 30.0, ..Default::default() } },
        ]),
        gif: Some(GifOptions { lossy: 40, ..Default::default() }),
    };
    crate::presets::save(&p).unwrap();
    let mut q = p.clone();
    q.name = "Small & shielded".into();
    q.transform = None;
    q.layers = Some(vec![PresetLayer::Shield { visible: true, shield: Default::default() }]);
    crate::presets::save(&q).unwrap();
}

#[test]
#[ignore]
fn size_probe() {
    let path = std::env::var("PROBE").unwrap_or_else(|_| "testdata/test.gif".into());
    let a = io::load_animation(std::path::Path::new(&path)).unwrap();
    eprintln!("{path}: original {} bytes, {} frames", std::fs::metadata(&path).unwrap().len(), a.len());
    for (name, o) in [
        ("default", GifOptions::default()),
        ("nomerge", GifOptions { merge_duplicates: false, ..Default::default() }),
        ("global", GifOptions { global_palette: true, ..Default::default() }),
        ("nodiff", GifOptions { frame_diff: false, merge_duplicates: false, ..Default::default() }),
    ] {
        let b = encode_gif(&a, &o, None).unwrap();
        let back = io::decode_gif_bytes(&b).unwrap();
        eprintln!("{name}: {} bytes, maxdiff {}", b.len(), max_diff(&a, &back));
    }
}

#[test]
fn lossless_256_colours_and_sprites() {
    // 256 distinct colours in one frame: must stay exact even though differencing
    // wants a transparent slot
    let img = RgbaImage::from_fn(64, 64, |x, y| {
        let i = (y / 4 * 16 + x / 4) as u8;
        Rgba([i, 255 - i, i.wrapping_mul(7), 255])
    });
    let mut img2 = img.clone();
    img2.put_pixel(3, 3, Rgba([1, 2, 3, 255]));
    let a = Animation { frames: vec![Frame::new(img, 10), Frame::new(img2, 10)], loop_count: None };
    let b = encode_gif(&a, &GifOptions::default(), None).unwrap();
    assert_eq!(max_diff(&a, &io::decode_gif_bytes(&b).unwrap()), 0);

    // transparent sprite: frames are trimmed to the visible area, still exact
    let s = synth(true);
    let b = encode_gif(&s, &GifOptions::default(), None).unwrap();
    let back = io::decode_gif_bytes(&b).unwrap();
    assert_eq!(max_diff(&s, &back), 0);
    let full: usize = s.len() * (96 * 64) / 2; // generous bound for untrimmed frames
    assert!(b.len() < full, "{} bytes", b.len());
}

#[test]
fn webp_roundtrip_and_smaller() {
    let a = synth(false);
    let lossless = io::encode_webp(&a, &io::WebpOptions { lossless: true, ..Default::default() }).unwrap();
    let back = io::load_animation_bytes(&lossless).unwrap();
    assert_eq!(back.len(), a.len());
    for (fa, fb) in a.frames.iter().zip(&back.frames) {
        for (x, y, pa) in fa.image.enumerate_pixels() {
            let pb = fb.image.get_pixel(x, y);
            if pa != pb {
                panic!("lossless WebP differs at {x},{y}: {pa:?} vs {pb:?}");
            }
        }
    }
    let lossy = io::encode_webp(&a, &io::WebpOptions { lossless: false, quality: 60.0, method: 4 }).unwrap();
    assert!(io::load_animation_bytes(&lossy).unwrap().len() == a.len());
    // transparency survives
    let t = synth(true);
    let b = io::encode_webp(&t, &io::WebpOptions { lossless: true, ..Default::default() }).unwrap();
    let back = io::load_animation_bytes(&b).unwrap();
    assert_eq!(back.frames[0].image.get_pixel(0, 0)[3], 0);
}

#[test]
fn fit_to_target_size() {
    use crate::target::{fit, TargetFormat};
    let a = io::load_animation(std::path::Path::new("testdata/test.gif")).unwrap();
    let trials = AtomicUsize::new(0);
    let base = encode_gif(&a, &GifOptions::default(), None).unwrap().len() as u64;
    // generous target: first rung, untouched settings
    let f = fit(&a, &TargetFormat::Gif(GifOptions::default()), base + 10, &trials).unwrap();
    assert!(f.reached && f.bytes.len() as u64 <= base + 10);
    // tight target: must shrink and still fit
    let goal = base / 3;
    let f = fit(&a, &TargetFormat::Gif(GifOptions::default()), goal, &trials).unwrap();
    assert!(f.reached, "{}", f.settings);
    assert!(f.bytes.len() as u64 <= goal);
    assert!(io::decode_gif_bytes(&f.bytes).is_ok());
    // WebP
    let f = fit(&a, &TargetFormat::Webp(io::WebpOptions::default()), 150_000, &trials).unwrap();
    assert!(f.reached && f.bytes.len() <= 150_000, "{} {}", f.bytes.len(), f.settings);
}

#[test]
fn avif_export_if_ffmpeg() {
    if !io::ffmpeg_available() {
        return;
    }
    let a = synth(true);
    let b = io::encode_avif(&a, 50).unwrap();
    assert!(b.len() > 100 && &b[4..8] == b"ftyp", "not an AVIF");
}

#[test]
#[ignore]
fn dump_webp() {
    let a = synth(false);
    let b = io::encode_webp(&a, &io::WebpOptions { lossless: true, ..Default::default() }).unwrap();
    let d = std::env::var("DUMP").unwrap();
    std::fs::write(format!("{d}/l.webp"), &b).unwrap();
    for (i, f) in a.frames.iter().enumerate() {
        f.image.save(format!("{d}/f{i}.png")).unwrap();
    }
}

#[test]
#[ignore]
fn webp_probe() {
    let path = std::env::var("PROBE").unwrap_or_else(|_| "testdata/test.gif".into());
    let a = io::load_animation(std::path::Path::new(&path)).unwrap();
    for (l, q, m) in [(true, 75.0, 4u8), (true, 100.0, 6), (false, 90.0, 4), (false, 75.0, 4), (false, 50.0, 4), (false, 20.0, 6)] {
        let t = std::time::Instant::now();
        let b = io::encode_webp(&a, &io::WebpOptions { lossless: l, quality: q, method: m }).unwrap();
        eprintln!("webp lossless={l} q={q} m={m}: {} bytes ({:.1}s)", b.len(), t.elapsed().as_secs_f32());
    }
    for q in [80u8, 60, 40] {
        let t = std::time::Instant::now();
        if let Ok(b) = io::encode_avif(&a, q) {
            eprintln!("avif q={q}: {} bytes ({:.1}s)", b.len(), t.elapsed().as_secs_f32());
        }
    }
}

#[test]
#[ignore]
fn encode_timing() {
    let path = std::env::var("PROBE").unwrap_or_else(|_| "testdata/test.gif".into());
    let a = io::load_animation(std::path::Path::new(&path)).unwrap();
    for o in [GifOptions::default(), GifOptions { lossy: 60, ..Default::default() }, GifOptions { lossy: 200, ..Default::default() }, GifOptions { lossy: 60, colors: 128, ..Default::default() }] {
        let t = std::time::Instant::now();
        let b = encode_gif(&a, &o, None).unwrap();
        eprintln!("lossy {}: {} bytes in {:.2}s", o.lossy, b.len(), t.elapsed().as_secs_f32());
    }
}

/// Writes README comparison encodes of $PROBE into $DUMP.
#[test]
#[ignore]
fn readme_encodes() {
    let path = std::env::var("PROBE").unwrap();
    let d = std::env::var("DUMP").unwrap();
    let a = io::load_animation(std::path::Path::new(&path)).unwrap();
    let run = |name: &str, f: &dyn Fn() -> Vec<u8>| {
        let t = std::time::Instant::now();
        let b = f();
        std::fs::write(format!("{d}/{name}"), &b).unwrap();
        eprintln!("{name}\t{}\t{:.1}s", b.len(), t.elapsed().as_secs_f32());
    };
    run("gif_lossless.gif", &|| encode_gif(&a, &GifOptions::default(), None).unwrap());
    run("gif_lossy60.gif", &|| encode_gif(&a, &GifOptions { lossy: 60, ..Default::default() }, None).unwrap());
    run("gif_lossy120.gif", &|| encode_gif(&a, &GifOptions { lossy: 120, ..Default::default() }, None).unwrap());
    run("webp_lossless.webp", &|| io::encode_webp(&a, &io::WebpOptions { lossless: true, quality: 75.0, method: 4 }).unwrap());
    run("webp_q80.webp", &|| io::encode_webp(&a, &io::WebpOptions { lossless: false, quality: 80.0, method: 4 }).unwrap());
    run("avif_q60.avif", &|| io::encode_avif(&a, 60).unwrap());
}

#[test]
#[ignore]
fn parse_preset_file() {
    let p = std::env::var("PRESET").unwrap();
    let s = std::fs::read_to_string(p).unwrap();
    let r: Result<crate::presets::Preset, _> = serde_json::from_str(&s);
    if let Err(e) = &r {
        panic!("parse error: {e}");
    }
}

#[test]
fn video_import_if_ffmpeg() {
    if !io::ffmpeg_available() {
        return;
    }
    // export a short clip, then import it again
    let a = synth(false);
    let dir = std::env::temp_dir().join(format!("gf_vid_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("clip.mp4");
    io::export_video(&a, &p, io::VideoFormat::Mp4, [0, 0, 0], true).unwrap();
    let b = io::load_animation(&p).unwrap();
    assert!(b.len() >= 8, "{} frames", b.len());
    assert_eq!((b.width(), b.height()), (96, 64));
    let dur = b.duration_secs();
    assert!((dur - a.duration_secs()).abs() < 0.25, "{dur} vs {}", a.duration_secs());
    // the noai comment ends up in the container
    let raw = std::fs::read(&p).unwrap();
    assert!(raw.windows(4).any(|w| w == b"noai"), "mp4 has no noai comment");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_every_format() {
    use io::PreviewFormat as P;
    let a = synth(true);
    let mut fmts = vec![P::Gif(GifOptions::default()), P::Webp(io::WebpOptions::default()), P::Apng];
    if io::ffmpeg_available() {
        fmts.push(P::Avif(60));
        fmts.push(P::Video(io::VideoFormat::Mp4, [255, 255, 255]));
        fmts.push(P::Video(io::VideoFormat::WebmAlpha, [0, 0, 0]));
    }
    for f in fmts {
        let (dec, bytes) = io::encode_preview(&a, &f).unwrap_or_else(|e| panic!("{f:?}: {e:#}"));
        assert!(!bytes.is_empty());
        assert_eq!((dec.width(), dec.height()), (a.width(), a.height()), "{f:?}");
        assert!((dec.duration_secs() - a.duration_secs()).abs() < 0.2, "{f:?}: {} vs {}", dec.duration_secs(), a.duration_secs());
    }
}

#[test]
#[ignore]
fn preview_alpha_probe() {
    use io::PreviewFormat as P;
    let a = synth(true);
    for f in [P::Avif(60), P::Video(io::VideoFormat::WebmAlpha, [0, 0, 0]), P::Webp(io::WebpOptions::default())] {
        let (dec, _) = io::encode_preview(&a, &f).unwrap();
        eprintln!("{f:?}: corner alpha {} square {:?}", dec.frames[0].image.get_pixel(90, 2)[3], dec.frames[0].image.get_pixel(5, 30));
    }
}

#[test]
fn sheet_pow2_and_indexed_png() {
    let a = synth(true);
    // 12 frames of 96x64
    let mut o = io::SheetOptions { pow2: true, ..Default::default() };
    let (cols, rows, w, h, cw, ch) = io::sheet_layout(12, 96, 64, &o);
    assert!(cw.is_power_of_two() && ch.is_power_of_two(), "{cw}x{ch}");
    assert!(cols * rows >= 12 && w <= cw && h <= ch);
    // smallest pow2 area among all column counts
    for c in 1..=12u32 {
        let r = 12u32.div_ceil(c);
        let area = (c * 96).next_power_of_two() as u64 * (r * 64).next_power_of_two() as u64;
        assert!(area >= cw as u64 * ch as u64);
    }
    let (sheet, rects, _, _) = io::render_spritesheet(&a, &o).unwrap();
    assert_eq!(sheet.dimensions(), (cw, ch));
    assert_eq!(rects.len(), 12);
    // indexed PNG: exact when colours fit, transparency kept, smaller than full colour
    o.png.colors = 256;
    let full = io::encode_png(&sheet, &io::PngOptions { colors: 0, noai: false }).unwrap();
    let pal = io::encode_png(&sheet, &o.png).unwrap();
    let back = image::load_from_memory(&pal).unwrap().to_rgba8();
    assert_eq!(back.dimensions(), sheet.dimensions());
    for (p, q) in back.pixels().zip(sheet.pixels()) {
        if q[3] == 0 {
            assert_eq!(p[3], 0);
        } else {
            assert_eq!(p, q);
        }
    }
    assert!(pal.len() < full.len(), "{} vs {}", pal.len(), full.len());
    // quantised path (too many colours) still decodes
    let grad = RgbaImage::from_fn(64, 64, |x, y| Rgba([(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]));
    let q = io::encode_png(&grad, &io::PngOptions { colors: 16, noai: false }).unwrap();
    assert_eq!(image::load_from_memory(&q).unwrap().to_rgba8().dimensions(), (64, 64));
    // previews
    let (dec, _) = io::encode_preview(&a, &io::PreviewFormat::Sheet(o.clone())).unwrap();
    assert_eq!((dec.width(), dec.height()), (cw, ch));
    let (dec, bytes) = io::encode_preview(&a, &io::PreviewFormat::Frames(io::SplitFormat::Png, io::PngOptions { colors: 64, noai: false }, GifOptions::default())).unwrap();
    assert_eq!(dec.len(), a.len());
    assert!(!bytes.is_empty());
}

#[test]
fn kde_scheme_parsing() {
    let txt = "[General]\nAccentColor=233,64,155\n\n[Colors:Window]\nBackgroundNormal=239,240,241\nForegroundNormal=35,38,41\nForegroundInactive=112,125,138\n\n[Colors:View]\nBackgroundNormal=255,255,255\n\n[Colors:Header][Inactive]\nBackgroundNormal=1,2,3\n";
    let k = crate::theme::parse_kdeglobals_for_test(txt);
    assert_eq!(k.0, Some(eframe::egui::Color32::from_rgb(233, 64, 155)));
    let (pal, dark) = k.1.unwrap();
    assert!(!dark);
    assert_eq!(pal.window, eframe::egui::Color32::from_rgb(239, 240, 241));
    // inactive header section must not be picked up
    assert_eq!(pal.header, pal.window);
    let dark_txt = "[Colors:Window]\nBackgroundNormal=#202326\nForegroundNormal=252,252,252\n";
    assert!(crate::theme::parse_kdeglobals_for_test(dark_txt).1.unwrap().1);
    assert!(crate::theme::parse_kdeglobals_for_test("[General]\nfoo=bar\n").1.is_none());
}

#[test]
fn noai_markers_in_png_apng_webp() {
    let has = |b: &[u8], n: &[u8]| b.windows(n.len()).any(|w| w == n);
    let a = synth(true);
    // still PNG, full colour and palette
    for colors in [0u16, 64] {
        let img = (*a.frames[0].image).clone();
        let plain = io::encode_png(&img, &io::PngOptions { colors, noai: false }).unwrap();
        let tagged = io::encode_png(&img, &io::PngOptions { colors, noai: true }).unwrap();
        assert!(!has(&plain, b"noai") && has(&tagged, b"noai") && has(&tagged, b"DMI-PROHIBITED"));
        let dec = png::Decoder::new(std::io::Cursor::new(&tagged)).read_info().unwrap();
        let info = dec.info();
        assert!(info.uncompressed_latin1_text.iter().any(|t| t.keyword == "Comment" && t.text.contains("noai")));
        assert!(info.utf8_text.iter().any(|t| t.keyword == "XML:com.adobe.xmp"));
        assert_eq!(image::load_from_memory(&tagged).unwrap().to_rgba8().dimensions(), img.dimensions());
    }
    // APNG keeps its frames
    let ap = io::encode_apng(&a, true).unwrap();
    assert!(has(&ap, b"DMI-PROHIBITED"));
    assert_eq!(io::load_animation_bytes(&ap).unwrap().len(), a.len());
    // WebP: XMP chunk, flag set, RIFF size right, still decodes
    let w = io::encode_webp(&a, &io::WebpOptions::default()).unwrap();
    let t = crate::noai::tag_webp(w.clone());
    assert!(has(&t, b"XMP ") && t[20] & 0x04 != 0);
    assert_eq!(u32::from_le_bytes([t[4], t[5], t[6], t[7]]) as usize, t.len() - 8);
    assert_eq!(io::load_animation_bytes(&t).unwrap().len(), io::load_animation_bytes(&w).unwrap().len());
    // non-images pass through untouched
    assert_eq!(crate::noai::tag_png(b"hello".to_vec()), b"hello");
    assert_eq!(crate::noai::tag_webp(b"hello".to_vec()), b"hello");
}

#[test]
fn noai_avif_still_decodes() {
    if !io::ffmpeg_available() {
        return;
    }
    let has = |b: &[u8], n: &[u8]| b.windows(n.len()).any(|w| w == n);
    let dir = std::env::temp_dir().join(format!("gf_avif_tag_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for alpha in [false, true] {
        let a = synth(alpha);
        let plain = io::encode_avif(&a, 60).unwrap();
        let tagged = crate::noai::tag_avif(plain.clone());
        assert_ne!(plain, tagged, "alpha={alpha}: not tagged");
        assert!(has(&tagged, b"DMI-PROHIBITED") && has(&tagged, b"application/rdf+xml"));
        let (pp, tp) = (dir.join(format!("p{alpha}.avif")), dir.join(format!("t{alpha}.avif")));
        std::fs::write(&pp, &plain).unwrap();
        std::fs::write(&tp, &tagged).unwrap();
        let (x, y) = (io::load_video(&pp).unwrap(), io::load_video(&tp).unwrap());
        assert_eq!(x.len(), y.len());
        assert_eq!((x.width(), x.height()), (y.width(), y.height()));
        for (f, g) in x.frames.iter().zip(&y.frames) {
            assert_eq!(f.image.as_raw(), g.image.as_raw(), "alpha={alpha}: pixels changed");
        }
        if let Ok(keep) = std::env::var("GIFKIT_KEEP_AVIF") {
            std::fs::copy(&tp, std::path::Path::new(&keep).join(format!("tagged_{alpha}.avif"))).unwrap();
            std::fs::copy(&pp, std::path::Path::new(&keep).join(format!("plain_{alpha}.avif"))).unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(crate::noai::tag_avif(b"hello".to_vec()), b"hello");
}

#[test]
fn shapes_render_every_kind() {
    use crate::geo::Obb;
    use crate::shapes::{shape_sprite, ShapeKind, ShapeSettings};
    for k in ShapeKind::ALL.into_iter().chain([ShapeKind::Emoji]) {
        for shadow in [false, true] {
            let s = ShapeSettings { kind: k, shadow, ..Default::default() };
            let (img, x, y) = shape_sprite(&s, Obb { cx: 100.0, cy: 80.0, w: 120.0, h: 70.0, angle: 20.0 }, 1.0).unwrap_or_else(|| panic!("{k:?} gave no sprite"));
            let opaque = img.pixels().filter(|p| p[3] > 200).count();
            assert!(opaque > 200, "{k:?} shadow={shadow}: only {opaque} opaque pixels");
            // the sprite covers the rotated box
            assert!(x < 40 && y < 45 && x + img.width() as i64 > 160 && y + img.height() as i64 > 115, "{k:?} sprite at {x},{y} {}x{}", img.width(), img.height());
        }
    }
    // fill off + outline on leaves the middle clear
    let s = ShapeSettings { kind: ShapeKind::Rect, fill_on: false, stroke_on: true, stroke_w: 3.0, ..Default::default() };
    let (img, x, y) = shape_sprite(&s, Obb { cx: 50.0, cy: 50.0, w: 60.0, h: 60.0, angle: 0.0 }, 1.0).unwrap();
    assert_eq!(img.get_pixel((50 - x) as u32, (50 - y) as u32)[3], 0);
    assert!(img.get_pixel((20 - x) as u32, (50 - y) as u32)[3] > 200);
}

#[test]
fn shape_and_markup_layers_in_stack() {
    use crate::shapes::{MarkupSettings, PenStroke, ShapeKind, ShapeSettings};
    use crate::stack::{LayerKind, Stack};
    let a = synth(false);
    let base = (*a.frames[0].image).clone();
    let mut st = Stack::default();
    st.push(LayerKind::Shape(Box::new(ShapeSettings { kind: ShapeKind::Rect, x: 10, y: 10, w: 30, h: 20, fill: [255, 0, 255], stroke_on: false, ..Default::default() })));
    // one stroke on every frame, one on frame 3 only
    st.push(LayerKind::Markup(MarkupSettings { strokes: vec![
        PenStroke { points: vec![[60.0, 50.0], [90.0, 50.0]], color: [0, 255, 0], width: 6.0, ..Default::default() },
        PenStroke { points: vec![[60.0, 10.0], [90.0, 10.0]], color: [0, 0, 255], width: 6.0, from: 3, to: 3, ..Default::default() },
    ] }));
    let out = st.flatten(&a, &[], None);
    let f0 = &out.frames[0].image;
    assert_eq!(f0.get_pixel(25, 20).0, [255, 0, 255, 255], "shape fill");
    assert_ne!(base.get_pixel(25, 20).0, [255, 0, 255, 255]);
    assert_eq!(f0.get_pixel(75, 50).0, [0, 255, 0, 255], "pen stroke");
    assert_ne!(f0.get_pixel(75, 10).0, [0, 0, 255, 255], "frame-3 stroke must not show on frame 1");
    assert_eq!(out.frames[2].image.get_pixel(75, 10).0, [0, 0, 255, 255], "frame-3 stroke shows on frame 3");
    // a flipped frame carries the shape along
    st.transform.rot_on = true;
    st.transform.flip_h = true;
    let out = st.flatten(&a, &[], None);
    let w = out.width();
    assert_eq!(out.frames[0].image.get_pixel(w - 1 - 25, 20).0, [255, 0, 255, 255]);
}

#[test]
fn emoji_set_search_and_render() {
    use crate::emoji;
    let all = emoji::all();
    assert!(all.len() > 1800, "only {} emoji", all.len());
    // every graphic (skin tones too) is in the archive and draws something
    let files: Vec<&str> = all.iter().flat_map(|e| std::iter::once(e.f.as_str()).chain(e.s.iter().map(|s| s.1.as_str()))).collect();
    assert!(files.len() > 3400);
    use rayon::prelude::*;
    let bad: Vec<&&str> = files.par_iter().filter(|f| emoji::render(f, 24).map(|i| i.pixels().filter(|p| p[3] > 0).count() < 10).unwrap_or(true)).collect();
    assert!(bad.is_empty(), "emoji that don't render: {bad:?}");
    // every group has emoji, in order
    for g in 0..9u8 {
        assert!(emoji::search("", Some(g)).len() > 50, "group {g}");
    }
    // search by name and by keyword, name matches first
    let fire = emoji::search("fire", None);
    assert_eq!(all[fire[0]].f, "1f525");
    let happy = emoji::search("happy", None);
    assert!(happy.iter().any(|&i| all[i].f == "1f600"), "keyword search");
    assert!(emoji::search("cat face", None).iter().any(|&i| all[i].f == "1f431"));
    assert!(emoji::search("zzqqxx", None).is_empty());
    // skin tones resolve to their base emoji
    let wave = &all[emoji::search("waving hand", None)[0]];
    assert_eq!(wave.s.len(), 5);
    assert_eq!(emoji::by_file(wave.file(5)).map(|x| x.1), Some(5));
    assert!(emoji::name_of(wave.file(1)).contains("light skin tone"));
}

#[test]
fn emoji_layer_renders() {
    use crate::shapes::EmojiSettings;
    use crate::stack::{LayerKind, Stack};
    let a = synth(false);
    let mut st = Stack::default();
    st.push(LayerKind::Emoji(Box::new(EmojiSettings { emoji: "1f525".into(), x: 20, y: 10, w: 40, h: 40, ..Default::default() })));
    let out = st.flatten(&a, &[], None);
    let before = &a.frames[0].image;
    let after = &out.frames[0].image;
    let changed = (20..60).flat_map(|x| (10..50).map(move |y| (x, y))).filter(|&(x, y)| before.get_pixel(x, y) != after.get_pixel(x, y)).count();
    assert!(changed > 600, "emoji changed only {changed} pixels");
    assert_eq!(before.get_pixel(90, 60), after.get_pixel(90, 60), "outside the box stays as it was");
}

#[test]
#[ignore]
fn booth_contact_sheet() {
    // GIFKIT_BOOTH_IN=photo.png GIFKIT_BOOTH_OUT=sheet.png cargo test booth_contact_sheet -- --ignored
    use crate::booth::{apply, BoothFx};
    let Ok(inp) = std::env::var("GIFKIT_BOOTH_IN") else { return };
    let src = image::open(inp).unwrap().to_rgba8();
    let all: Vec<BoothFx> = BoothFx::LOOKS.iter().chain(BoothFx::DISTORT.iter()).copied().collect();
    let (w, h) = src.dimensions();
    let cols = 4u32;
    let rows = (all.len() as u32 + 1).div_ceil(cols);
    let mut sheet = image::RgbaImage::from_pixel(cols * w, rows * h, image::Rgba([0, 0, 0, 255]));
    image::imageops::overlay(&mut sheet, &src, 0, 0);
    for (i, fx) in all.iter().enumerate() {
        let out = apply(&src, *fx, 50.0, 50.0, 50.0);
        let k = i as u32 + 1;
        image::imageops::overlay(&mut sheet, &out, ((k % cols) * w) as i64, ((k / cols) * h) as i64);
    }
    sheet.save(std::env::var("GIFKIT_BOOTH_OUT").unwrap()).unwrap();
}

#[test]
fn booth_effects_change_pixels() {
    use crate::booth::{apply, BoothFx};
    let a = synth(false);
    let src = &a.frames[3].image;
    for fx in BoothFx::LOOKS.iter().chain(BoothFx::DISTORT.iter()) {
        let out = apply(src, *fx, 60.0, 50.0, 50.0);
        assert_eq!(out.dimensions(), src.dimensions(), "{fx:?}");
        let diff = out.pixels().zip(src.pixels()).filter(|(x, y)| x != y).count();
        assert!(diff > 100, "{fx:?} changed only {diff} pixels");
    }
    // the effects layer runs it
    let fx = crate::effects::EffectSettings { booth: BoothFx::Thermal, ..Default::default() };
    let out = crate::effects::apply_frame(src, 0, 1, 0.0, 0.0, &fx, &[]);
    assert_ne!(out.as_raw(), src.as_raw());
}

#[test]
fn text_stretch_changes_width_only() {
    let fonts = crate::text::builtin_fonts();
    let t = crate::text::Annotation { text: "Hello".into(), size: 30.0, ..Default::default() };
    let (w, h) = crate::text::measure(&fonts, &t);
    let t2 = crate::text::Annotation { stretch: 0.5, ..t.clone() };
    let (w2, h2) = crate::text::measure(&fonts, &t2);
    assert_eq!(h, h2);
    assert!((w2 as i64 - (w as i64 / 2)).abs() <= 1, "{w} -> {w2}");
    let sp = crate::text::render_sprite(&fonts, &t2);
    assert_eq!(sp.dimensions(), (w2, h2));
}


#[test]
fn project_round_trip() {
    use crate::project::{load, save, ProjLayer, ProjLayerEntry, ProjectFile};
    use std::collections::HashMap;
    use std::sync::Arc;
    let base = synth(true);
    // an "original" that differs (frames were edited since opening)
    let mut original = base.clone();
    original.frames.truncate(5);
    let overlay: Vec<image::RgbaImage> = (0..3).map(|i| image::RgbaImage::from_pixel(8, 6, image::Rgba([i * 80, 10, 200, 255]))).collect();
    let wm = Arc::new(image::RgbaImage::from_pixel(5, 5, image::Rgba([1, 2, 3, 128])));
    let layers = vec![
        ProjLayerEntry { visible: true, label: Some("Caption".into()), timing: Default::default(), layer: ProjLayer::Text { text: crate::text::Annotation { text: "hi".into(), stretch: 0.7, ..Default::default() }, font: Some(("builtin".into(), "Sans Bold".into())) } },
        ProjLayerEntry { visible: false, label: None, timing: Default::default(), layer: ProjLayer::Image { id: 1, name: "ov.png".into(), path: None, delays: vec![5, 5, 5], placements: vec![Default::default(); base.len()], opacity: 80.0 } },
        ProjLayerEntry { visible: true, label: None, timing: Default::default(), layer: ProjLayer::Watermark { id: 2, watermark: Default::default(), has_image: true } },
        ProjLayerEntry { visible: true, label: None, timing: crate::timeline::Timing { clips: Some(vec![crate::timeline::Clip { start: 2, end: Some(5) }]), keys: vec![crate::timeline::Key { frame: 3, x: 10.0, ..Default::default() }] }, layer: ProjLayer::Emoji { emoji: crate::shapes::EmojiSettings { emoji: "1f525".into(), w: 20, h: 20, ..Default::default() } } },
        ProjLayerEntry { visible: true, label: None, timing: Default::default(), layer: ProjLayer::Markup { markup: crate::shapes::MarkupSettings { strokes: vec![crate::shapes::PenStroke { points: vec![[1.0, 2.0], [3.0, 4.0]], ..Default::default() }] } } },
    ];
    let mut transform = crate::stack::Transform::default();
    transform.reverse = true;
    let file = ProjectFile { source_name: "test.gif".into(), transform, layers, settings: serde_json::json!({"format": "WebP"}), ..Default::default() };
    let dir = std::env::temp_dir().join(format!("gf_proj_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.gifkit");
    let ov: HashMap<usize, Vec<image::RgbaImage>> = [(1, overlay.clone())].into();
    let wms: HashMap<usize, Arc<image::RgbaImage>> = [(2, wm.clone())].into();
    save(&path, file, &base, Some(&original), &ov, &wms).unwrap();
    let pr = load(&path).unwrap();
    assert_eq!(pr.base.len(), base.len());
    assert_eq!(pr.base.loop_count, base.loop_count);
    for (a, b) in pr.base.frames.iter().zip(&base.frames) {
        assert_eq!(a.image.as_raw(), b.image.as_raw(), "frames are lossless");
        assert_eq!((a.delay_cs, a.enabled), (b.delay_cs, b.enabled));
    }
    assert_eq!(pr.original.as_ref().map(|o| o.len()), Some(5));
    assert_eq!(pr.overlays[&1].len(), 3);
    assert_eq!(pr.overlays[&1][2].as_raw(), overlay[2].as_raw());
    assert_eq!(pr.watermarks[&2].as_raw(), wm.as_raw());
    assert!(pr.file.transform.reverse);
    assert_eq!(pr.file.layers.len(), 5);
    assert_eq!(pr.file.layers[0].label.as_deref(), Some("Caption"));
    assert!(!pr.file.layers[1].visible);
    assert!(matches!(&pr.file.layers[0].layer, ProjLayer::Text { text, .. } if (text.stretch - 0.7).abs() < 1e-6));
    assert_eq!(pr.file.settings["format"], "WebP");
    // clips and keyframes come back
    assert_eq!(pr.file.layers[3].timing.spans(10), vec![(2, 5)]);
    assert_eq!(pr.file.layers[3].timing.keys[0].x, 10.0);
    assert!(pr.file.layers[0].timing.is_default());
    // an unchanged original isn't stored twice
    let f2 = ProjectFile::default();
    save(&path, f2, &base, Some(&base.clone()), &HashMap::new(), &HashMap::new()).unwrap();
    assert!(load(&path).unwrap().original.is_none());
    // not a project
    std::fs::write(dir.join("bad.gifkit"), b"nope").unwrap();
    assert!(load(&dir.join("bad.gifkit")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn booth_zero_strength_is_identity() {
    use crate::booth::{apply, BoothFx};
    let a = synth(false);
    let src = &a.frames[3].image;
    for fx in [BoothFx::Glow, BoothFx::Bulge, BoothFx::Dent, BoothFx::Twirl, BoothFx::Squeeze, BoothFx::Stretch, BoothFx::Tunnel, BoothFx::FishEye] {
        let out = apply(src, fx, 0.0, 50.0, 50.0);
        let worst = out.pixels().zip(src.pixels()).map(|(p, q)| (0..4).map(|c| (p[c] as i32 - q[c] as i32).abs()).max().unwrap_or(0)).max().unwrap_or(0);
        assert!(worst <= 1, "{fx:?} at 0% still changes pixels by {worst}");
    }
}

#[test]
fn bundled_fonts_load_and_draw() {
    let fams = crate::bundled::families();
    assert!(fams.len() >= 50, "font index missing");
    for f in fams {
        for face in &f.faces {
            let (e, _) = crate::bundled::load(&f.family, &face.style).unwrap_or_else(|| panic!("{} {}", f.family, face.style));
            let a = crate::text::Annotation { text: "Ab".into(), size: 32.0, ..Default::default() };
            let img = crate::text::render_sprite(&[e], &a);
            assert!(img.pixels().any(|p| p[3] > 0), "{} {} drew nothing", f.family, face.style);
        }
    }
}

#[test]
fn arabic_is_shaped_right_to_left() {
    // joined Arabic is narrower than its isolated letters laid side by side
    let (e, _) = crate::bundled::load("Amiri", "Regular").unwrap();
    let joined = crate::text::rasterize(&e, "سلام", 48.0, crate::text::Align::Left).0;
    let apart = crate::text::rasterize(&e, "س ل ا م", 48.0, crate::text::Align::Left).0;
    assert!(joined < apart);
}

#[test]
#[ignore]
fn bundled_font_sheet() {
    let samples: &[(&str, &str)] = &[
        ("Oswald", "GIFkit Привет"), ("Anton", "SO MUCH WOW"), ("Lobster", "Have a nice day"), ("Press Start 2P", "GAME OVER"),
        ("Amiri", "مرحبا بالعالم"), ("Cairo", "أهلاً وسهلاً"), ("Frank Ruhl Libre", "שלום עולם"), ("Kanit", "สวัสดีครับ"),
        ("Catamaran", "வணக்கம்"), ("Mallanna", "నమస్కారం"), ("Hind", "नमस्ते दुनिया"), ("Noto Sans Georgian", "გამარჯობა"),
        ("Dela Gothic One", "こんにちは世界"), ("Do Hyeon", "안녕하세요"), ("ZCOOL KuaiLe", "你好世界"), ("Rubik Glitch", "GLITCH"),
    ];
    let mut rows = Vec::new();
    for (fam, text) in samples {
        let (e, _) = crate::bundled::load(fam, "Regular").unwrap();
        let a = crate::text::Annotation { text: format!("{fam}: {text}"), size: 30.0, color: [20, 20, 20, 255], border_width: Some(0.0), ..Default::default() };
        rows.push(crate::text::render_sprite(&[e], &a));
    }
    let w = rows.iter().map(|r| r.width()).max().unwrap() + 20;
    let h: u32 = rows.iter().map(|r| r.height() + 6).sum::<u32>() + 20;
    let mut out = image::RgbaImage::from_pixel(w, h, image::Rgba([250, 250, 250, 255]));
    let mut y = 10;
    for r in rows {
        image::imageops::overlay(&mut out, &r, 10, y as i64);
        y += r.height() + 6;
    }
    out.save("target/font_sheet.png").unwrap();
}

#[test]
fn long_gif_encodes_in_runs() {
    // more frames than one run, so the encoder splits and joins them
    let one = synth(false);
    let mut a = Animation { frames: Vec::new(), loop_count: None };
    for k in 0..25 {
        a.frames.extend(one.frames.iter().map(|f| Frame::new((*f.image).clone(), f.delay_cs + (k % 2) as u16)));
    }
    assert!(a.len() > 200);
    let bytes = encode_gif(&a, &GifOptions::default(), None).unwrap();
    let b = io::load_animation_bytes(&bytes).unwrap();
    assert_eq!(b.len(), a.len());
    assert!(max_diff(&a, &b) < 200);
}

#[test]
fn fast_speed_skips_frames_instead_of_crawling() {
    use crate::stack::{SpeedMode, Stack};
    // 2 cs frames at 200% would be 1 cs, which players show as 10 cs
    let a = Animation { frames: (0..40).map(|_| Frame::new(RgbaImage::new(4, 4), 2)).collect(), loop_count: None };
    let mut s = Stack::default();
    s.transform.speed_on = true;
    s.transform.speed_mode = SpeedMode::Percent;
    s.transform.speed_val = 200.0;
    let plan = s.plan(&a);
    let shown: Vec<_> = plan.iter().filter(|f| f.enabled).collect();
    assert!(shown.iter().all(|f| f.delay >= 2), "no delay under 2 cs");
    let total: u32 = shown.iter().map(|f| f.delay as u32).sum();
    assert_eq!(total, 40, "twice as fast: 80 cs becomes 40 cs");
    assert_eq!(shown.len(), 20);
    // 3 cs at 150% stays every frame, with the 2 cs remainder spread evenly
    let b = Animation { frames: (0..10).map(|_| Frame::new(RgbaImage::new(4, 4), 3)).collect(), loop_count: None };
    s.transform.speed_val = 150.0;
    let plan = s.plan(&b);
    assert!(plan.iter().all(|f| f.enabled && f.delay == 2));
}

#[test]
fn layer_clips_and_keyframes_render() {
    use crate::shapes::{ShapeKind, ShapeSettings};
    use crate::stack::{LayerKind, Stack};
    use crate::timeline::{Clip, Key};
    let a = Animation { frames: (0..10).map(|_| Frame::new(RgbaImage::from_pixel(60, 40, Rgba([0, 0, 0, 255])), 5)).collect(), loop_count: None };
    let mut s = Stack::default();
    let id = s.push(LayerKind::Shape(Box::new(ShapeSettings { kind: ShapeKind::Rect, x: 5, y: 5, w: 10, h: 10, fill: [255, 0, 0], ..Default::default() })));
    let li = s.index_of(id).unwrap();
    s.layers[li].timing.clips = Some(vec![Clip { start: 2, end: Some(7) }]);
    // moves 30 px right from frame 2 to frame 7, steady speed
    s.layers[li].timing.keys = vec![Key { frame: 2, smooth: false, ..Default::default() }, Key { frame: 7, x: 30.0, smooth: false, ..Default::default() }];
    let out = s.flatten(&a, &[], None);
    let red = |f: usize, x: u32, y: u32| out.frames[f].image.get_pixel(x, y)[0] > 200;
    assert!(!red(0, 10, 10) && !red(1, 10, 10), "hidden before its clip");
    assert!(red(2, 10, 10), "shows where placed at the first key");
    assert!(red(7, 40, 10) && !red(7, 10, 10), "moved 30 px by the last key");
    assert!(!red(8, 40, 10) && !red(8, 10, 10), "hidden after its clip");
    // a keyed fade mixes with what's under it
    s.layers[li].timing.keys = vec![Key { frame: 0, opacity: 50.0, ..Default::default() }];
    let out = s.flatten(&a, &[], None);
    let p = out.frames[3].image.get_pixel(10, 10);
    assert!((110..=145).contains(&p[0]), "half faded: {p:?}");
}

#[test]
fn effect_intro_starts_with_its_clip() {
    use crate::stack::{LayerKind, Stack};
    use crate::timeline::Clip;
    let a = Animation { frames: (0..10).map(|_| Frame::new(RgbaImage::from_pixel(8, 8, Rgba([200, 200, 200, 255])), 5)).collect(), loop_count: None };
    let mut s = Stack::default();
    let fx = EffectSettings { intro: crate::motion::Transition::Fade, fade_frames: 3, ..Default::default() };
    let id = s.push(LayerKind::Effects(Box::new(fx)));
    let li = s.index_of(id).unwrap();
    s.layers[li].timing.clips = Some(vec![Clip { start: 4, end: Some(8) }]);
    let out = s.flatten(&a, &[], None);
    let v = |f: usize| out.frames[f].image.get_pixel(4, 4)[0];
    assert_eq!(v(2), 200, "outside the clip nothing happens");
    assert!(v(4) < 80, "the fade starts at the clip: {}", v(4));
    assert_eq!(v(7), 200, "and is done three frames in");
}

#[test]
fn canvas_space_pads_and_layers_follow() {
    use crate::shapes::{ShapeKind, ShapeSettings};
    use crate::stack::{LayerKind, Stack};
    let a = Animation { frames: (0..2).map(|_| Frame::new(RgbaImage::from_pixel(20, 10, Rgba([0, 0, 255, 255])), 5)).collect(), loop_count: None };
    let mut s = Stack::default();
    s.transform.pad_on = true;
    s.transform.pad = [6, 0, 14, 2]; // top, right, bottom, left
    s.push(LayerKind::Shape(Box::new(ShapeSettings { kind: ShapeKind::Rect, x: 0, y: 0, w: 4, h: 4, fill: [255, 0, 0], ..Default::default() })));
    let out = s.flatten(&a, &[], None);
    let f = &out.frames[0].image;
    assert_eq!(f.dimensions(), (22, 30));
    assert_eq!(f.get_pixel(0, 0).0, [255, 255, 255, 255], "new space is white by default");
    assert_eq!(f.get_pixel(10, 25).0, [255, 255, 255, 255]);
    assert_eq!(f.get_pixel(15, 10).0, [0, 0, 255, 255], "the picture sits after the left and top space");
    assert_eq!(f.get_pixel(3, 7)[0], 255, "a layer at the source corner moves with the picture");
    s.transform.pad_transparent = true;
    let out = s.flatten(&a, &[], None);
    assert_eq!(out.frames[0].image.get_pixel(0, 0)[3], 0);
}

#[test]
fn caption_bar_fits_its_text() {
    use crate::stack::{LayerKind, Stack};
    use crate::text::{Annotation, Caption};
    let fonts = text::builtin_fonts();
    let a = Animation { frames: vec![Frame::new(RgbaImage::from_pixel(200, 100, Rgba([0, 0, 255, 255])), 5)], loop_count: None };
    let mut s = Stack::default();
    let long = "a caption long enough that it has to wrap onto more than one line";
    let id = s.push(LayerKind::Text(Annotation { text: long.into(), size: 16.0, caption: Caption::Bottom, ..Default::default() }));
    while s.layout_text(200, 100, &fonts) {}
    let out = s.flatten(&a, &fonts, None);
    let f = &out.frames[0].image;
    assert_eq!(f.width(), 200, "a caption bar only adds height");
    assert!(f.height() > 100 + 40, "two or more lines make the bar taller: {}", f.height());
    assert_eq!(f.get_pixel(100, 50).0, [0, 0, 255, 255], "the picture is untouched");
    assert_eq!(f.get_pixel(1, 101).0, [255, 255, 255, 255], "the bar is white");
    let dark = (100..f.height()).flat_map(|y| (0..200).map(move |x| (x, y))).filter(|&(x, y)| f.get_pixel(x, y)[0] < 80).count();
    assert!(dark > 50, "black text in the bar");
    // the text fits the width
    let li = s.index_of(id).unwrap();
    if let LayerKind::Text(t) = &s.layers[li].kind {
        let (tw, _) = text::measure(&fonts, t);
        assert!(tw as f32 <= 200.0, "wrapped to the canvas: {tw}");
        assert!(t.x >= 0, "centred: {}", t.x);
    }
    // more padding, taller bar
    if let LayerKind::Text(t) = &mut s.layers[li].kind {
        t.bar_pad = 100.0;
    }
    while s.layout_text(200, 100, &fonts) {}
    assert!(s.flatten(&a, &fonts, None).frames[0].image.height() > f.height());
}

#[test]
fn preview_proxy_matches_a_shrunk_full_render() {
    use crate::shapes::{ShapeKind, ShapeSettings};
    use crate::stack::{proxy_size, LayerKind, Stack};
    let a = Animation { frames: (0..2).map(|_| Frame::new(RgbaImage::from_fn(400, 300, |x, y| Rgba([(x / 2) as u8, (y / 2) as u8, 90, 255])), 5)).collect(), loop_count: None };
    let mut s = Stack::default();
    s.transform.crop_on = true;
    s.transform.crop = [40, 20, 300, 240];
    s.transform.resize_on = true;
    s.transform.rs_w = 240;
    s.transform.rs_h = 0;
    s.transform.pad_on = true;
    s.transform.pad = [10, 0, 10, 0];
    s.push(LayerKind::Shape(Box::new(ShapeSettings { kind: ShapeKind::Rect, x: 100, y: 100, w: 80, h: 60, fill: [255, 0, 0], ..Default::default() })));
    let full = s.flatten(&a, &[], None);
    let k = 0.5;
    let (pw, ph) = proxy_size(400, 300, k);
    let small = Animation { frames: a.frames.iter().map(|f| Frame::new(image::imageops::resize(&*f.image, pw, ph, image::imageops::FilterType::Triangle), 5)).collect(), loop_count: None };
    let prev = s.proxy(k).flatten(&small, &[], None);
    let (fw, fh) = full.frames[0].image.dimensions();
    let (qw, qh) = prev.frames[0].image.dimensions();
    assert!((qw as f32 - fw as f32 * k).abs() <= 2.0 && (qh as f32 - fh as f32 * k).abs() <= 2.0, "{fw}x{fh} vs {qw}x{qh}");
    // the red box lands in the same place, at half the size
    let red = |img: &RgbaImage, x: u32, y: u32| img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1))[0] > 200 && img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1))[1] < 60;
    let f = &full.frames[0].image;
    let q = &prev.frames[0].image;
    let mut same = 0;
    let mut total = 0;
    for y in (0..fh).step_by(4) {
        for x in (0..fw).step_by(4) {
            total += 1;
            if red(f, x, y) == red(q, (x as f32 * k) as u32, (y as f32 * k) as u32) {
                same += 1;
            }
        }
    }
    assert!(same as f32 / total as f32 > 0.97, "{same} of {total}");
}

#[test]
fn gif_info_reads_palettes_and_tags_once() {
    let mut buf = Vec::new();
    {
        let mut e = gif::Encoder::new(&mut buf, 2, 2, &[0, 0, 0, 255, 255, 255]).unwrap();
        for _ in 0..3 {
            let mut f = gif::Frame::default();
            (f.width, f.height, f.buffer) = (2, 2, std::borrow::Cow::Owned(vec![0, 1, 1, 0]));
            e.write_frame(&f).unwrap();
        }
    }
    let i = crate::encode::gif_info(&buf).unwrap();
    assert_eq!((i.frames, i.colors, i.global_only, i.has_comment), (3, 2, true, false));
    let t = crate::encode::tag_gif(buf.clone());
    assert!(t.len() > buf.len());
    assert!(crate::encode::gif_info(&t).unwrap().has_comment);
    assert_eq!(crate::encode::tag_gif(t.clone()).len(), t.len());
    // still a GIF that decodes to the same frames
    let mut d = gif::DecodeOptions::new().read_info(std::io::Cursor::new(&t)).unwrap();
    let mut n = 0;
    while d.read_next_frame().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 3);
}


/// A still picture made longer, with a fade in and out: whatever the timeline
/// does (skips, cuts, speed, reverse), the first and last frames that show
/// are the faintest and match each other.
#[test]
fn fades_line_up_with_timeline_changes() {
    use crate::stack::{LayerKind, Stack};
    let still = image::RgbaImage::from_pixel(16, 16, image::Rgba([200, 200, 200, 255]));
    let base = Animation { frames: (0..30).map(|_| crate::anim::Frame::new(still.clone(), 10)).collect(), loop_count: None };
    let lum = |img: &image::RgbaImage| img.get_pixel(8, 8)[0] as i32;
    type Tweak = fn(&mut Stack, &mut Animation);
    let cases: Vec<(&str, Tweak)> = vec![
        ("plain", |_, _| {}),
        ("skip end", |_, b| (25..30).for_each(|i| b.frames[i].enabled = false)),
        ("skip start", |_, b| (0..4).for_each(|i| b.frames[i].enabled = false)),
        ("cut", |s, _| {
            let t = &mut s.transform;
            (t.cut_on, t.cut_start, t.cut_end) = (true, 5, 22);
        }),
        ("speed 400%", |s, _| (s.transform.speed_on, s.transform.speed_val) = (true, 400.0)),
        ("speed 800%", |s, _| (s.transform.speed_on, s.transform.speed_val) = (true, 800.0)),
        ("speed 50%", |s, _| (s.transform.speed_on, s.transform.speed_val) = (true, 50.0)),
        ("reverse", |s, _| (s.transform.play_on, s.transform.reverse) = (true, true)),
        ("boomerang", |s, _| (s.transform.play_on, s.transform.boomerang) = (true, true)),
    ];
    for (name, tweak) in cases {
        let mut b = base.clone();
        let mut st = Stack::default();
        let mut fx = crate::effects::EffectSettings::default();
        fx.intro = crate::motion::Transition::Fade;
        fx.outro = crate::motion::Transition::Fade;
        fx.fade_to_transparent = false;
        st.push(LayerKind::Effects(Box::new(fx)));
        tweak(&mut st, &mut b);
        let flat = st.flatten(&b, &[], None);
        let shown: Vec<i32> = flat.frames.iter().filter(|f| f.enabled).map(|f| lum(&f.image)).collect();
        let (first, last) = (shown[0], *shown.last().unwrap());
        let min = *shown.iter().min().unwrap();
        assert!(first == min && last == min, "{name}: first {first} last {last} min {min} {shown:?}");
        assert!(first < 40, "{name}: starts too bright {shown:?}");
    }
}

#[test]
fn distance_map_matches_brute_force() {
    let (w, h) = (23usize, 17usize);
    let set: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 31 == 0).collect();
    let d = crate::ops::distance_sq(&set, w, h);
    for y in 0..h {
        for x in 0..w {
            let best = (0..w * h).filter(|&j| set[j]).map(|j| ((j % w) as f32 - x as f32).powi(2) + ((j / w) as f32 - y as f32).powi(2)).fold(f32::MAX, f32::min);
            assert!((d[y * w + x] - best).abs() < 1e-3, "{x},{y}: {} vs {best}", d[y * w + x]);
        }
    }
    let near = crate::ops::near_any(&set, w, h, 2);
    for y in 0..h {
        for x in 0..w {
            let any = (0..w * h).any(|j| set[j] && (j % w).abs_diff(x) <= 2 && (j / w).abs_diff(y) <= 2);
            assert_eq!(near[y * w + x], any);
        }
    }
}
