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
    let b = io::load_animation_bytes(&io::encode_apng(&a).unwrap()).unwrap();
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
    let ann = text::Annotation { text: "Hello\nworld".into(), ..Default::default() };
    let s = text::render_sprite(&fonts, &ann);
    assert!(s.width() > 20 && s.height() > 40);
    assert!(s.pixels().any(|p| p[3] == 255 && p[0] == 255));
    let mut img = (*a.frames[0].image).clone();
    text::draw_sprite(&mut img, &s, &ann);
    assert_ne!(img.as_raw(), a.frames[0].image.as_raw());

    let c = effects::CensorSettings { x: 0, y: 0, w: 40, h: 40, ..Default::default() };
    let out = effects::censor_frame(&a.frames[0].image, 0, &c);
    assert_ne!(out.as_raw(), a.frames[0].image.as_raw());

    let src = overlay::OverlaySource { name: "x".into(), frames: vec![RgbaImage::from_pixel(10, 10, Rgba([0, 255, 0, 255]))], delays: vec![10] };
    let pl = vec![overlay::Placement { enabled: true, x: 5, y: 5, w: 20, h: 20, angle: 0.0 }; a.len()];
    let o = overlay::apply(&a, &src, &pl, 1.0);
    assert_eq!(*o.frames[0].image.get_pixel(10, 10), Rgba([0, 255, 0, 255]));
}

#[test]
fn exports() {
    let dir = std::env::temp_dir().join(format!("gifforge_test_{}", std::process::id()));
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
    assert_eq!(sheet.width(), 96 * 4);
    if io::ffmpeg_available() {
        io::export_video(&synth(false), &dir.join("v.mp4"), io::VideoFormat::Mp4, [255, 255, 255]).unwrap();
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
