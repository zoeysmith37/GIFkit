//! Loading and exporting animations.

use crate::anim::{Animation, Frame};
use crate::encode::{encode_gif, GifOptions};
use anyhow::{bail, Context, Result};
use image::{AnimationDecoder, RgbaImage};
use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

fn delay_to_cs(d: image::Delay) -> u16 {
    let (n, dnm) = d.numer_denom_ms();
    let ms = if dnm == 0 { 100.0 } else { n as f64 / dnm as f64 };
    ((ms / 10.0).round() as i64).clamp(0, u16::MAX as i64) as u16
}

fn frames_from_decoder<'a, D: AnimationDecoder<'a>>(dec: D) -> Result<Vec<Frame>> {
    let mut out = Vec::new();
    for f in dec.into_frames() {
        let f = f?;
        let d = delay_to_cs(f.delay());
        out.push(Frame::new(f.into_buffer(), d));
    }
    Ok(out)
}

fn read_gif_loop(bytes: &[u8]) -> Option<u16> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::Indexed);
    let mut dec = opts.read_info(Cursor::new(bytes)).ok()?;
    // The NETSCAPE extension appears before the first frame; reading one frame header suffices.
    let _ = dec.next_frame_info();
    match dec.repeat() {
        gif::Repeat::Infinite => None,
        gif::Repeat::Finite(0) => Some(1),
        gif::Repeat::Finite(n) => Some(n.saturating_add(1)),
    }
}

/// Load any supported file: animated GIF / APNG / WebP, or a still image (1 frame).
pub fn load_animation(path: &Path) -> Result<Animation> {
    if is_video(path) {
        return load_video(path);
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    load_animation_bytes(&bytes)
}

/// Video file extensions opened through ffmpeg.
pub const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "ogv", "3gp"];

pub fn is_video(path: &Path) -> bool {
    path.extension().map(|e| VIDEO_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
}

/// Longest side and frame cap used when importing video (GIFs of full-HD
/// video need gigabytes of memory and make huge files).
pub const VIDEO_MAX_SIDE: u32 = 640;
pub const VIDEO_MAX_FRAMES: usize = 1500;

/// Source frame rate via ffprobe, if available.
fn probe_fps(path: &Path) -> Option<f64> {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=avg_frame_rate,r_frame_rate", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .ok()?;
    let txt = String::from_utf8_lossy(&out.stdout);
    txt.lines()
        .filter_map(|l| {
            let (a, b) = l.trim().split_once('/')?;
            let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
            (b > 0.0 && a > 0.0).then(|| a / b)
        })
        .find(|f| *f > 0.5 && *f < 1000.0)
}

/// Import a video as frames (needs ffmpeg). Frame rate is snapped to a rate
/// GIF delays can represent exactly (50, 33.3, 25, 20 ... fps); large videos are
/// scaled down to [`VIDEO_MAX_SIDE`].
pub fn load_video(path: &Path) -> Result<Animation> {
    anyhow::ensure!(ffmpeg_available(), "Opening videos needs ffmpeg installed and on PATH");
    let src_fps = probe_fps(path).unwrap_or(25.0);
    // GIF delays are whole 1/100 s: pick the fastest exact rate not above the source
    let delay_cs = [2u16, 3, 4, 5, 6, 8, 10].into_iter().find(|d| 100.0 / *d as f64 <= src_fps + 0.01).unwrap_or(10);
    let fps = 100.0 / delay_cs as f64;
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp = std::env::temp_dir().join(format!("gifforge_vid_{}_{}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let m = VIDEO_MAX_SIDE;
    let vf = format!("fps={fps},scale='if(gt(iw,ih),min({m},iw),-2)':'if(gt(iw,ih),-2,min({m},ih))':flags=lanczos,format=rgba");
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(path)
        .args(["-an", "-vf", &vf, "-frames:v", &VIDEO_MAX_FRAMES.to_string()])
        .arg(tmp.join("f%06d.png"))
        .output()
        .context("Failed to run ffmpeg")?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("ffmpeg could not read the video: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
    }
    let mut names: Vec<PathBuf> = std::fs::read_dir(&tmp)?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "png").unwrap_or(false)).collect();
    names.sort();
    use rayon::prelude::*;
    let frames: Vec<Frame> = names.par_iter().map(|p| image::open(p).map(|i| Frame::new(i.to_rgba8(), delay_cs))).collect::<std::result::Result<_, _>>()?;
    let _ = std::fs::remove_dir_all(&tmp);
    anyhow::ensure!(!frames.is_empty(), "The video has no frames");
    Ok(Animation { frames, loop_count: None })
}

pub fn load_animation_bytes(bytes: &[u8]) -> Result<Animation> {
    let fmt = image::guess_format(bytes).context("Unrecognised image format")?;
    let mut loop_count = None;
    let frames = match fmt {
        image::ImageFormat::Gif => {
            loop_count = read_gif_loop(bytes);
            let dec = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
            frames_from_decoder(dec)?
        }
        image::ImageFormat::Png => {
            let dec = image::codecs::png::PngDecoder::new(Cursor::new(bytes))?;
            if dec.is_apng()? {
                frames_from_decoder(dec.apng()?)?
            } else {
                vec![Frame::new(image::load_from_memory(bytes)?.to_rgba8(), 10)]
            }
        }
        image::ImageFormat::WebP => {
            let dec = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))?;
            if dec.has_animation() {
                // libwebp composites frames exactly (image's decoder can be off by one)
                loop_count = webp_loop(bytes);
                let d = webp_animation::Decoder::new(bytes).map_err(|e| anyhow::anyhow!("WebP: {e:?}"))?;
                let (w, h) = d.dimensions();
                let mut prev = 0i32;
                let mut out = Vec::new();
                for f in d {
                    let img = RgbaImage::from_raw(w, h, f.data().to_vec()).context("WebP frame size")?;
                    let d_ms = (f.timestamp() - prev).max(0);
                    prev = f.timestamp();
                    out.push(Frame::new(img, ((d_ms + 5) / 10).clamp(1, 65535) as u16));
                }
                out
            } else {
                vec![Frame::new(image::load_from_memory(bytes)?.to_rgba8(), 10)]
            }
        }
        _ => vec![Frame::new(image::load_from_memory(bytes)?.to_rgba8(), 10)],
    };
    if frames.is_empty() {
        bail!("File contains no frames");
    }
    Ok(Animation { frames, loop_count })
}

/// Loop count from a WebP ANIM chunk (`None` = forever).
fn webp_loop(bytes: &[u8]) -> Option<u16> {
    let i = bytes.windows(4).position(|w| w == b"ANIM")?;
    let n = u16::from_le_bytes([*bytes.get(i + 12)?, *bytes.get(i + 13)?]);
    (n != 0).then_some(n)
}

/// Load a still image (first frame only) for overlays / extra frames.
pub fn load_still(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path)?.to_rgba8())
}

/// Build an animation from a list of image files (sorted by the caller).
/// Images of differing sizes are fitted (centred, transparent padding) to the first.
pub fn load_sequence(paths: &[PathBuf], delay_cs: u16) -> Result<Animation> {
    let mut frames = Vec::new();
    let mut size: Option<(u32, u32)> = None;
    for p in paths {
        let img = load_still(p)?;
        let img = match size {
            None => {
                size = Some(img.dimensions());
                img
            }
            Some((w, h)) => fit_onto_canvas(&img, w, h),
        };
        frames.push(Frame::new(img, delay_cs));
    }
    if frames.is_empty() {
        bail!("No images selected");
    }
    Ok(Animation { frames, loop_count: None })
}

/// Scale `img` to fit inside w×h (keeping aspect) and centre it on a transparent canvas.
pub fn fit_onto_canvas(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img.clone();
    }
    let s = (w as f32 / img.width() as f32).min(h as f32 / img.height() as f32);
    let nw = ((img.width() as f32 * s).round() as u32).max(1);
    let nh = ((img.height() as f32 * s).round() as u32).max(1);
    let scaled = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut out = RgbaImage::new(w, h);
    image::imageops::overlay(&mut out, &scaled, ((w - nw) / 2) as i64, ((h - nh) / 2) as i64);
    out
}

pub fn save_gif(anim: &Animation, opt: &GifOptions, path: &Path, progress: Option<&std::sync::atomic::AtomicUsize>) -> Result<u64> {
    let bytes = encode_gif(anim, opt, progress)?;
    std::fs::write(path, &bytes)?;
    Ok(bytes.len() as u64)
}

pub fn encode_apng(anim: &Animation, noai: bool) -> Result<Vec<u8>> {
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let (w, h) = frames[0].image.dimensions();
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        if noai {
            enc.add_text_chunk("Comment".into(), crate::encode::NOAI_COMMENT.into())?;
        }
        enc.set_animated(frames.len() as u32, anim.loop_count.map(|n| n as u32).unwrap_or(0))?;
        let mut wr = enc.write_header()?;
        for f in &frames {
            wr.set_frame_delay(f.delay_cs, 100)?;
            wr.write_image_data(f.image.as_raw())?;
        }
        wr.finish()?;
    }
    Ok(buf)
}

pub fn save_apng(anim: &Animation, path: &Path, noai: bool) -> Result<u64> {
    let b = encode_apng(anim, noai)?;
    std::fs::write(path, &b)?;
    Ok(b.len() as u64)
}

fn png_bytes(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    img.write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png)?;
    Ok(v)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SplitFormat {
    Png,
    Gif,
}

/// Write every enabled frame as an individual file into `dir`.
pub fn export_frames_dir(anim: &Animation, dir: &Path, prefix: &str, fmt: SplitFormat) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let frames = anim.enabled_frames();
    let digits = frames.len().to_string().len().max(3);
    for (i, f) in frames.iter().enumerate() {
        let name = format!("{prefix}_{:0width$}", i + 1, width = digits);
        match fmt {
            SplitFormat::Png => f.image.save(dir.join(format!("{name}.png")))?,
            SplitFormat::Gif => {
                let single = Animation { frames: vec![f.clone()], loop_count: None };
                let b = encode_gif(&single, &GifOptions { frame_diff: false, ..Default::default() }, None)?;
                std::fs::write(dir.join(format!("{name}.gif")), b)?;
            }
        }
    }
    Ok(frames.len())
}

/// Write every enabled frame as PNG into a ZIP archive.
pub fn export_frames_zip(anim: &Animation, path: &Path, prefix: &str) -> Result<usize> {
    let frames = anim.enabled_frames();
    let file = File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let digits = frames.len().to_string().len().max(3);
    for (i, f) in frames.iter().enumerate() {
        zip.start_file(format!("{prefix}_{:0width$}.png", i + 1, width = digits), opts)?;
        zip.write_all(&png_bytes(&f.image)?)?;
    }
    zip.finish()?;
    Ok(frames.len())
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetOptions {
    /// 0 = automatic (roughly square sheet).
    pub columns: u32,
    pub padding: u32,
    pub write_json: bool,
    pub write_godot: bool,
    /// res:// directory that the sheet will live in inside the Godot project.
    pub godot_res_dir: String,
    pub anim_name: String,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self { columns: 0, padding: 0, write_json: true, write_godot: true, godot_res_dir: "res://".into(), anim_name: "default".into() }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Export a sprite sheet PNG (+ optional JSON metadata and Godot 4 SpriteFrames .tres).
pub fn export_spritesheet(anim: &Animation, png_path: &Path, opt: &SheetOptions) -> Result<String> {
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let (fw, fh) = frames[0].image.dimensions();
    let n = frames.len() as u32;
    let cols = if opt.columns == 0 { (n as f32).sqrt().ceil() as u32 } else { opt.columns.min(n) }.max(1);
    let rows = n.div_ceil(cols);
    let p = opt.padding;
    let sw = cols * fw + (cols - 1) * p;
    let sh = rows * fh + (rows - 1) * p;
    let mut sheet = RgbaImage::new(sw, sh);
    let mut rects = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let c = i as u32 % cols;
        let r = i as u32 / cols;
        let x = c * (fw + p);
        let y = r * (fh + p);
        image::imageops::replace(&mut sheet, &*f.image, x as i64, y as i64);
        rects.push((x, y));
    }
    sheet.save(png_path)?;
    let stem = png_path.file_stem().and_then(|s| s.to_str()).unwrap_or("sheet").to_string();
    let file_name = png_path.file_name().and_then(|s| s.to_str()).unwrap_or("sheet.png").to_string();
    let mut written = vec![file_name.clone()];

    if opt.write_json {
        let mut j = String::from("{\n  \"image\": \"");
        j.push_str(&file_name);
        j.push_str(&format!("\",\n  \"frameWidth\": {fw},\n  \"frameHeight\": {fh},\n  \"columns\": {cols},\n  \"rows\": {rows},\n  \"padding\": {p},\n  \"loop\": {},\n  \"frames\": [\n", anim.loop_count.is_none()));
        for (i, ((x, y), f)) in rects.iter().zip(frames.iter()).enumerate() {
            j.push_str(&format!(
                "    {{ \"index\": {i}, \"x\": {x}, \"y\": {y}, \"w\": {fw}, \"h\": {fh}, \"durationMs\": {} }}{}\n",
                f.delay_cs as u32 * 10,
                if i + 1 < rects.len() { "," } else { "" }
            ));
        }
        j.push_str("  ]\n}\n");
        let jp = png_path.with_extension("json");
        std::fs::write(&jp, j)?;
        written.push(jp.file_name().unwrap().to_string_lossy().into_owned());
    }

    if opt.write_godot {
        // Godot SpriteFrames: speed (fps) * relative duration per frame.
        let base = frames.iter().map(|f| f.delay_cs.max(1) as u32).fold(0, gcd).max(1);
        let speed = 100.0 / base as f64;
        let mut dir = opt.godot_res_dir.trim().to_string();
        if dir.is_empty() {
            dir = "res://".into();
        }
        if !dir.ends_with('/') {
            dir.push('/');
        }
        let mut t = format!("[gd_resource type=\"SpriteFrames\" load_steps={} format=3]\n\n", frames.len() + 2);
        t.push_str(&format!("[ext_resource type=\"Texture2D\" path=\"{dir}{file_name}\" id=\"1_sheet\"]\n\n"));
        for (i, (x, y)) in rects.iter().enumerate() {
            t.push_str(&format!(
                "[sub_resource type=\"AtlasTexture\" id=\"AtlasTexture_{i}\"]\natlas = ExtResource(\"1_sheet\")\nregion = Rect2({x}, {y}, {fw}, {fh})\n\n"
            ));
        }
        t.push_str("[resource]\nanimations = [{\n\"frames\": [");
        for (i, f) in frames.iter().enumerate() {
            let dur = f.delay_cs.max(1) as f64 / base as f64;
            t.push_str(&format!(
                "{{\n\"duration\": {dur:.3},\n\"texture\": SubResource(\"AtlasTexture_{i}\")\n}}{}",
                if i + 1 < frames.len() { ", " } else { "" }
            ));
        }
        let name = if opt.anim_name.trim().is_empty() { "default" } else { opt.anim_name.trim() };
        t.push_str(&format!(
            "],\n\"loop\": {},\n\"name\": &\"{name}\",\n\"speed\": {speed:.3}\n}}]\n",
            anim.loop_count.is_none()
        ));
        let tp = png_path.with_file_name(format!("{stem}_frames.tres"));
        std::fs::write(&tp, t)?;
        written.push(tp.file_name().unwrap().to_string_lossy().into_owned());
    }
    Ok(written.join(", "))
}

pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VideoFormat {
    Mp4,
    WebmAlpha,
    /// Animated AVIF (AV1), keeps transparency
    Avif,
}

/// Animated WebP settings.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WebpOptions {
    pub lossless: bool,
    /// 0..=100 (lossy quality, or compression effort when lossless)
    pub quality: f32,
    /// 0 = fastest .. 6 = smallest
    pub method: u8,
}

impl Default for WebpOptions {
    fn default() -> Self {
        Self { lossless: false, quality: 85.0, method: 4 }
    }
}

/// Encode an animated WebP (libwebp). Delays are kept exactly.
pub fn encode_webp(anim: &Animation, o: &WebpOptions) -> Result<Vec<u8>> {
    use webp_animation::{AnimParams, Encoder, EncoderOptions, EncodingConfig, EncodingType};
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let (w, h) = frames[0].image.dimensions();
    let cfg = EncodingConfig {
        encoding_type: if o.lossless { EncodingType::Lossless } else { EncodingType::new_lossy() },
        quality: o.quality.clamp(0.0, 100.0),
        method: o.method.min(6) as usize,
    };
    let opts = EncoderOptions {
        anim_params: AnimParams { loop_count: anim.loop_count.map(|n| n as i32).unwrap_or(0) },
        minimize_size: o.method >= 6,
        // 0 would make every frame a key frame (no inter-frame compression)
        kmin: 60,
        kmax: 120,
        allow_mixed: !o.lossless,
        encoding_config: Some(cfg),
        ..Default::default()
    };
    let mut enc = Encoder::new_with_options((w, h), opts).map_err(|e| anyhow::anyhow!("WebP: {e:?}"))?;
    let mut t = 0i32;
    for f in &frames {
        enc.add_frame(f.image.as_raw(), t).map_err(|e| anyhow::anyhow!("WebP frame: {e:?}"))?;
        t += crate::anim::effective_delay(f.delay_cs) as i32 * 10;
    }
    let data = enc.finalize(t).map_err(|e| anyhow::anyhow!("WebP: {e:?}"))?;
    Ok(data.to_vec())
}

/// Encode an animated AVIF with ffmpeg; `quality` 0..=100.
pub fn encode_avif(anim: &Animation, quality: u8) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("gifforge_avif_{}_{}.avif", std::process::id(), quality));
    export_video_q(anim, &tmp, VideoFormat::Avif, [0, 0, 0], Some(quality))?;
    let b = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok(b)
}

/// Export to video via ffmpeg (must be on PATH). Uses the concat demuxer so
/// per-frame delays are preserved exactly.
pub fn export_video(anim: &Animation, path: &Path, fmt: VideoFormat, bg: [u8; 3]) -> Result<()> {
    export_video_q(anim, path, fmt, bg, None)
}

/// [`export_video`] with an optional quality (0..=100) mapped to the codec's CRF.
pub fn export_video_q(anim: &Animation, path: &Path, fmt: VideoFormat, bg: [u8; 3], quality: Option<u8>) -> Result<()> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let tmp = std::env::temp_dir().join(format!("gifforge_{}_{}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let has_alpha = frames.iter().any(|f| f.image.pixels().any(|p| p[3] < 255));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut list = String::new();
    for (i, f) in frames.iter().enumerate() {
        let name = format!("f{:06}.png", i);
        let img = if fmt == VideoFormat::Mp4 {
            // flatten onto background, MP4 has no alpha
            let mut flat = RgbaImage::from_pixel(f.image.width(), f.image.height(), image::Rgba([bg[0], bg[1], bg[2], 255]));
            image::imageops::overlay(&mut flat, &*f.image, 0, 0);
            flat
        } else {
            (*f.image).clone()
        };
        img.save(tmp.join(&name))?;
        list.push_str(&format!("file '{}'\nduration {:.3}\n", name, crate::anim::effective_delay(f.delay_cs) as f64 / 100.0));
    }
    // concat demuxer needs the last file repeated to honour its duration
    list.push_str(&format!("file 'f{:06}.png'\n", frames.len() - 1));
    let list_path = tmp.join("list.txt");
    std::fs::write(&list_path, list)?;

    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.arg("-y").arg("-loglevel").arg("error").arg("-f").arg("concat").arg("-safe").arg("0").arg("-i").arg(&list_path);
    // quality 0..100 -> CRF (lower = better)
    let crf = |best: f32, worst: f32, default: u8| -> String {
        match quality {
            Some(q) => format!("{}", (worst + (best - worst) * q.min(100) as f32 / 100.0).round() as i32),
            None => default.to_string(),
        }
    };
    match fmt {
        VideoFormat::Mp4 => {
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=50", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", &crf(12.0, 40.0, 18), "-movflags", "+faststart"]);
        }
        VideoFormat::WebmAlpha => {
            cmd.args(["-vf", "fps=50", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-b:v", "0", "-crf", &crf(15.0, 55.0, 30), "-auto-alt-ref", "0"]);
        }
        VideoFormat::Avif => {
            let q = crf(10.0, 55.0, 28);
            if has_alpha {
                // AVIF stores alpha as a second AV1 stream
                cmd.args(["-filter_complex", "[0:v]fps=50,pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=rgba,split[c][a];[c]format=yuv420p[cc];[a]alphaextract,format=gray[al]", "-map", "[cc]", "-map", "[al]"]);
            } else {
                cmd.args(["-vf", "fps=50,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p"]);
            }
            cmd.args(["-c:v", "libaom-av1", "-crf", &q, "-b:v", "0", "-cpu-used", "6", "-row-mt", "1", "-still-picture", "0", "-f", "avif"]);
            if anim.loop_count.is_none() {
                cmd.args(["-loop", "0"]);
            }
        }
    }
    cmd.arg(path);
    let out = cmd.output().context("Failed to run ffmpeg (is it installed and on PATH?)")?;
    let _ = std::fs::remove_dir_all(&tmp);
    if !out.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Decode GIF bytes back into an animation (used to preview optimisation results).
pub fn decode_gif_bytes(bytes: &[u8]) -> Result<Animation> {
    load_animation_bytes(bytes)
}
