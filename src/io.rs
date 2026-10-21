//! Loading and exporting animations.

use crate::anim::{Animation, Frame};
use crate::encode::{encode_gif, GifOptions};
use anyhow::{bail, Context, Result};
use image::{AnimationDecoder, RgbaImage};
use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn delay_to_cs(d: image::Delay) -> u16 {
    let (n, dnm) = d.numer_denom_ms();
    let ms = if dnm == 0 { 100.0 } else { n as f64 / dnm as f64 };
    ((ms / 10.0).round() as i64).clamp(0, u16::MAX as i64) as u16
}

fn frames_from_decoder<'a, D: AnimationDecoder<'a>>(dec: D) -> Result<Vec<Frame>> {
    let mut out = Vec::new();
    for f in dec.into_frames() {
        // a damaged or cut-off file still opens with the frames before the damage,
        // as browsers show it
        let f = match f {
            Ok(f) => f,
            Err(e) if out.is_empty() => return Err(e.into()),
            Err(_) => break,
        };
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
        let info = probe_video(path)?;
        return load_video_range(path, &VideoImport::defaults(&info), None);
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    load_animation_bytes(&bytes)
}

/// Video file extensions opened through ffmpeg.
pub const VIDEO_EXTS: &[&str] = &["avif", "mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "ogv", "3gp"];

pub fn is_video(path: &Path) -> bool {
    path.extension().map(|e| VIDEO_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
}

/// Default longest side when importing video (GIFs of full-HD video need
/// gigabytes of memory and make huge files). The import window can change it.
pub const VIDEO_MAX_SIDE: u32 = 640;

/// What ffprobe says about a video.
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

/// How to bring a video in: the time range, frame rate and size.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoImport {
    pub start: f64,
    pub end: f64,
    /// GIF delay per frame in 1/100 s (2 = 50 fps … 10 = 10 fps)
    pub delay_cs: u16,
    /// longest side, 0 = keep the video's size
    pub max_side: u32,
    /// how much a frame may differ from the last one kept and still be
    /// merged into it (its time added on): see [`MERGE_LEVELS`]
    pub merge: usize,
}

/// Merging frames that barely change: (name, share of pixels that may change).
/// Screen recordings sit still most of the time, so this saves a lot.
pub const MERGE_LEVELS: [(&str, f64); 4] = [("Keep every frame", -1.0), ("Merge repeated frames", 0.0002), ("Merge small changes", 0.005), ("Merge most changes", 0.03)];

/// Share of pixels that differ clearly between two RGBA buffers (video
/// noise under a small step doesn't count). Stops counting past `limit`.
pub fn changed_share(a: &[u8], b: &[u8], limit: f64) -> f64 {
    const TOL: i32 = 18;
    let n = a.len() / 4;
    let stop = (limit * n as f64) as usize + 1;
    let mut changed = 0usize;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if (p[0] as i32 - q[0] as i32).abs() > TOL || (p[1] as i32 - q[1] as i32).abs() > TOL || (p[2] as i32 - q[2] as i32).abs() > TOL || (p[3] as i32 - q[3] as i32).abs() > TOL {
            changed += 1;
            if changed > stop {
                break;
            }
        }
    }
    changed as f64 / n.max(1) as f64
}

/// Frame delays GIFs can show exactly, fastest first.
pub const VIDEO_DELAYS: [u16; 7] = [2, 3, 4, 5, 6, 8, 10];

impl VideoImport {
    pub fn defaults(info: &VideoInfo) -> Self {
        // the fastest exact GIF rate that isn't above the source
        let delay_cs = VIDEO_DELAYS.into_iter().find(|d| 100.0 / *d as f64 <= info.fps + 0.01).unwrap_or(10);
        Self { start: 0.0, end: info.duration, delay_cs, max_side: VIDEO_MAX_SIDE, merge: 1 }
    }

    /// Output frame size (even numbers, as video scalers need).
    pub fn size(&self, info: &VideoInfo) -> (u32, u32) {
        let (w, h) = (info.width.max(2), info.height.max(2));
        let long = w.max(h);
        let s = if self.max_side == 0 || self.max_side >= long { 1.0 } else { self.max_side as f64 / long as f64 };
        let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
        (even(w as f64 * s), even(h as f64 * s))
    }

    pub fn frames(&self) -> usize {
        ((self.end - self.start).max(0.0) * 100.0 / self.delay_cs as f64).round().max(1.0) as usize
    }

    /// Memory the frames take once loaded.
    pub fn bytes(&self, info: &VideoInfo) -> u64 {
        let (w, h) = self.size(info);
        self.frames() as u64 * w as u64 * h as u64 * 4
    }
}

/// Size, frame rate and length of a video via ffprobe.
pub fn probe_video(path: &Path) -> Result<VideoInfo> {
    anyhow::ensure!(ffmpeg_available(), "Opening videos needs ffmpeg installed and on PATH");
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate,r_frame_rate,duration:stream_tags=rotate:stream_side_data=rotation:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .context("Failed to run ffprobe")?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).context("ffprobe gave no answer")?;
    let st = v["streams"].get(0).context("No video stream in the file")?;
    let num = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| x.as_f64());
    let rate = |x: &serde_json::Value| {
        let (a, b) = x.as_str()?.split_once('/')?;
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (a > 0.0 && b > 0.0).then(|| a / b).filter(|f| *f > 0.5 && *f < 1000.0)
    };
    let mut width = st["width"].as_u64().unwrap_or(0) as u32;
    let mut height = st["height"].as_u64().unwrap_or(0) as u32;
    anyhow::ensure!(width > 0 && height > 0, "Couldn't read the video size");
    // phone videos are often stored sideways with a rotation flag; ffmpeg turns them upright
    let rot = st["tags"]["rotate"].as_str().and_then(|r| r.parse::<i64>().ok()).or_else(|| st["side_data_list"].get(0).and_then(|d| d["rotation"].as_i64())).unwrap_or(0);
    if rot.rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    let fps = rate(&st["avg_frame_rate"]).or_else(|| rate(&st["r_frame_rate"])).unwrap_or(25.0);
    let duration = num(&v["format"]["duration"]).or_else(|| num(&st["duration"])).unwrap_or(0.0);
    anyhow::ensure!(duration > 0.0, "Couldn't read the video length");
    Ok(VideoInfo { width, height, fps, duration })
}

/// Import part of a video as frames, streaming raw pixels from ffmpeg (no
/// temporary files, no length cap). `progress` counts frames as they arrive.
pub fn load_video_range(path: &Path, opt: &VideoImport, progress: Option<&std::sync::atomic::AtomicUsize>) -> Result<Animation> {
    // decode on the graphics card when there is one; if that fails, again in software
    let hw = !crate::gpu::decode_args().is_empty();
    match load_video_range_with(path, opt, progress, hw) {
        Err(_) if hw && !crate::stack::stopped(progress) => {
            if let Some(p) = progress {
                p.fetch_and(crate::stack::STOP_BIT, std::sync::atomic::Ordering::Relaxed);
            }
            load_video_range_with(path, opt, progress, false)
        }
        r => r,
    }
}

fn load_video_range_with(path: &Path, opt: &VideoImport, progress: Option<&std::sync::atomic::AtomicUsize>, hw: bool) -> Result<Animation> {
    use std::io::Read;
    let info = probe_video(path)?;
    let (w, h) = opt.size(&info);
    let fps = 100.0 / opt.delay_cs as f64;
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if hw {
        cmd.args(crate::gpu::decode_args());
    }
    if opt.start > 0.0 {
        cmd.args(["-ss", &format!("{:.3}", opt.start)]);
    }
    cmd.arg("-i").arg(path);
    if opt.end < info.duration {
        cmd.args(["-t", &format!("{:.3}", (opt.end - opt.start).max(0.01))]);
    }
    cmd.args(["-an", "-vf", &format!("fps={fps},scale={w}:{h}:flags=lanczos,format=rgba"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let mut child = cmd.spawn().context("Failed to run ffmpeg")?;
    let mut stdout = child.stdout.take().context("ffmpeg output")?;
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let frame_len = (w * h * 4) as usize;
    let mut frames: Vec<Frame> = Vec::new();
    let limit = MERGE_LEVELS.get(opt.merge).map(|l| l.1).unwrap_or(-1.0);
    let mut buf = vec![0u8; frame_len];
    loop {
        match stdout.read_exact(&mut buf) {
            Ok(()) => {
                // a frame close enough to the last one kept only adds its time to it
                let last = frames.last_mut().filter(|f| limit >= 0.0 && f.delay_cs as u32 + (opt.delay_cs as u32) < 65535);
                match last {
                    Some(f) if changed_share(f.image.as_raw(), &buf, limit) <= limit => f.delay_cs += opt.delay_cs,
                    _ => frames.push(Frame::new(RgbaImage::from_raw(w, h, std::mem::replace(&mut buf, vec![0u8; frame_len])).context("frame size")?, opt.delay_cs)),
                }
                if let Some(p) = progress {
                    p.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if crate::stack::stopped(progress) {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("Stopped");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() && frames.is_empty() {
        bail!("ffmpeg could not read the video: {}", err.lines().last().unwrap_or(""));
    }
    anyhow::ensure!(!frames.is_empty(), "The video has no frames in that range");
    Ok(Animation { frames, loop_count: None })
}

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
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_video(path: &Path) -> Result<Animation> {
    load_video_opts(path, Some(VIDEO_MAX_SIDE), None)
}

/// [`load_video`] with an optional size cap and an exact output size to crop
/// to (used to decode our own previews, which ffmpeg pads to even sizes).
pub fn load_video_opts(path: &Path, max_side: Option<u32>, crop_to: Option<(u32, u32)>) -> Result<Animation> {
    load_video_dec(path, max_side, crop_to, None)
}

/// `decoder` forces an ffmpeg decoder (e.g. libvpx-vp9, which keeps WebM alpha).
fn load_video_dec(path: &Path, max_side: Option<u32>, crop_to: Option<(u32, u32)>, decoder: Option<&str>) -> Result<Animation> {
    // a forced decoder (WebM alpha) stays in software; otherwise the card first
    let hw = decoder.is_none() && !crate::gpu::decode_args().is_empty();
    match load_video_dec_with(path, max_side, crop_to, decoder, hw) {
        Err(_) if hw => load_video_dec_with(path, max_side, crop_to, decoder, false),
        r => r,
    }
}

fn load_video_dec_with(path: &Path, max_side: Option<u32>, crop_to: Option<(u32, u32)>, decoder: Option<&str>, hw: bool) -> Result<Animation> {
    anyhow::ensure!(ffmpeg_available(), "Opening videos needs ffmpeg installed and on PATH");
    let src_fps = probe_fps(path).unwrap_or(25.0);
    // GIF delays are whole 1/100 s: pick the fastest exact rate not above the source
    let delay_cs = [2u16, 3, 4, 5, 6, 8, 10].into_iter().find(|d| 100.0 / *d as f64 <= src_fps + 0.01).unwrap_or(10);
    let fps = 100.0 / delay_cs as f64;
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp = std::env::temp_dir().join(format!("gifkit_vid_{}_{}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut vf = format!("fps={fps}");
    if let Some(m) = max_side {
        vf.push_str(&format!(",scale='if(gt(iw,ih),min({m},iw),-2)':'if(gt(iw,ih),-2,min({m},ih))':flags=lanczos"));
    }
    if let Some((w, h)) = crop_to {
        vf.push_str(&format!(",crop={w}:{h}:0:0"));
    }
    vf.push_str(",format=rgba");
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error", "-y"]);
    if hw {
        cmd.args(crate::gpu::decode_args());
    }
    if let Some(d) = decoder {
        cmd.args(["-c:v", d]);
    }
    let out = cmd
        .arg("-i")
        .arg(path)
        .args(["-an", "-vf", &vf])
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
        enc.set_animated(frames.len() as u32, anim.loop_count.map(|n| n as u32).unwrap_or(0))?;
        let mut wr = enc.write_header()?;
        for f in &frames {
            wr.set_frame_delay(f.delay_cs, 100)?;
            wr.write_image_data(f.image.as_raw())?;
        }
        wr.finish()?;
    }
    Ok(if noai { crate::noai::tag_png(buf) } else { buf })
}

pub fn save_apng(anim: &Animation, path: &Path, noai: bool) -> Result<u64> {
    let b = encode_apng(anim, noai)?;
    std::fs::write(path, &b)?;
    Ok(b.len() as u64)
}

/// PNG output settings (frames and sprite sheets).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PngOptions {
    /// 0 = full colour; 2..=256 = indexed palette PNG (exact when the image
    /// has that few colours, quantised otherwise)
    pub colors: u16,
    /// add the "noai" markers (set at export time)
    #[serde(skip)]
    pub noai: bool,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self { colors: 0, noai: false }
    }
}

/// Encode a PNG: full colour (max compression) or indexed with a palette.
pub fn encode_png(img: &RgbaImage, o: &PngOptions) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_compression(png::Compression::High);
        if o.colors == 0 {
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(img.as_raw())?;
        } else {
            let n = o.colors.clamp(2, 256) as usize;
            // exact palette when the image fits, otherwise NeuQuant (RGBA aware)
            let mut exact: std::collections::HashMap<[u8; 4], u8> = std::collections::HashMap::new();
            let mut fits = true;
            for p in img.pixels() {
                let k = if p[3] == 0 { [0, 0, 0, 0] } else { p.0 };
                if !exact.contains_key(&k) {
                    if exact.len() >= n {
                        fits = false;
                        break;
                    }
                    let i = exact.len() as u8;
                    exact.insert(k, i);
                }
            }
            let (palette, indices): (Vec<[u8; 4]>, Vec<u8>) = if fits {
                let mut pal = vec![[0u8; 4]; exact.len()];
                for (c, i) in &exact {
                    pal[*i as usize] = *c;
                }
                let idx = img.pixels().map(|p| exact[&if p[3] == 0 { [0, 0, 0, 0] } else { p.0 }]).collect();
                (pal, idx)
            } else {
                let nq = color_quant::NeuQuant::new(10, n, img.as_raw());
                let pal: Vec<[u8; 4]> = nq.color_map_rgba().chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
                let idx = img.pixels().map(|p| nq.index_of(&p.0) as u8).collect();
                (pal, idx)
            };
            let depth = match palette.len() {
                0..=2 => png::BitDepth::One,
                3..=4 => png::BitDepth::Two,
                5..=16 => png::BitDepth::Four,
                _ => png::BitDepth::Eight,
            };
            let bits = match depth {
                png::BitDepth::One => 1,
                png::BitDepth::Two => 2,
                png::BitDepth::Four => 4,
                _ => 8,
            };
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(depth);
            enc.set_palette(palette.iter().flat_map(|c| [c[0], c[1], c[2]]).collect::<Vec<u8>>());
            if palette.iter().any(|c| c[3] < 255) {
                enc.set_trns(palette.iter().map(|c| c[3]).collect::<Vec<u8>>());
            }
            // pack rows at the chosen bit depth
            let per_byte = 8 / bits;
            let row_bytes = (w as usize).div_ceil(per_byte);
            let mut data = vec![0u8; row_bytes * h as usize];
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let v = indices[y * w as usize + x];
                    let bi = y * row_bytes + x / per_byte;
                    let shift = 8 - bits * (x % per_byte + 1);
                    data[bi] |= v << shift;
                }
            }
            enc.write_header()?.write_image_data(&data)?;
        }
    }
    Ok(if o.noai { crate::noai::tag_png(out) } else { out })
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SplitFormat {
    Png,
    Gif,
}

/// One frame encoded as it would be written by the frames export.
pub fn encode_frame_file(f: &Frame, fmt: SplitFormat, png: &PngOptions, gif: &GifOptions) -> Result<Vec<u8>> {
    match fmt {
        SplitFormat::Png => encode_png(&f.image, png),
        SplitFormat::Gif => {
            let single = Animation { frames: vec![Frame { enabled: true, ..f.clone() }], loop_count: None };
            encode_gif(&single, &GifOptions { frame_diff: false, merge_duplicates: false, ..gif.clone() }, None)
        }
    }
}

/// Write every enabled frame as an individual file into `dir`.
#[allow(dead_code)]
pub fn export_frames_dir(anim: &Animation, dir: &Path, prefix: &str, fmt: SplitFormat) -> Result<usize> {
    export_frames_dir_opts(anim, dir, prefix, fmt, &PngOptions::default(), &GifOptions::default())
}

pub fn export_frames_dir_opts(anim: &Animation, dir: &Path, prefix: &str, fmt: SplitFormat, png: &PngOptions, gif: &GifOptions) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let frames = anim.enabled_frames();
    let digits = frames.len().to_string().len().max(3);
    for (i, f) in frames.iter().enumerate() {
        let name = format!("{prefix}_{:0width$}", i + 1, width = digits);
        let ext = if fmt == SplitFormat::Png { "png" } else { "gif" };
        std::fs::write(dir.join(format!("{name}.{ext}")), encode_frame_file(f, fmt, png, gif)?)?;
    }
    Ok(frames.len())
}

/// Write every enabled frame as PNG into a ZIP archive.
#[allow(dead_code)]
pub fn export_frames_zip(anim: &Animation, path: &Path, prefix: &str) -> Result<usize> {
    export_frames_zip_opts(anim, path, prefix, &PngOptions::default())
}

pub fn export_frames_zip_opts(anim: &Animation, path: &Path, prefix: &str, png: &PngOptions) -> Result<usize> {
    let frames = anim.enabled_frames();
    let file = File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let digits = frames.len().to_string().len().max(3);
    for (i, f) in frames.iter().enumerate() {
        zip.start_file(format!("{prefix}_{:0width$}.png", i + 1, width = digits), opts)?;
        zip.write_all(&encode_png(&f.image, png)?)?;
    }
    zip.finish()?;
    Ok(frames.len())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SheetOptions {
    /// 0 = automatic (roughly square sheet).
    pub columns: u32,
    pub padding: u32,
    pub write_json: bool,
    pub write_godot: bool,
    /// res:// directory that the sheet will live in inside the Godot project.
    pub godot_res_dir: String,
    pub anim_name: String,
    /// pad the sheet to power-of-two width and height (and pick the column
    /// count that needs the smallest such canvas when columns is automatic)
    pub pow2: bool,
    pub png: PngOptions,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self { columns: 0, padding: 0, write_json: true, write_godot: true, godot_res_dir: "res://".into(), anim_name: "default".into(), pow2: true, png: PngOptions::default() }
    }
}

/// Sheet layout: (columns, rows, used width, used height, canvas width, canvas height).
pub fn sheet_layout(n: u32, fw: u32, fh: u32, opt: &SheetOptions) -> (u32, u32, u32, u32, u32, u32) {
    let n = n.max(1);
    let p = opt.padding;
    let size = |cols: u32| {
        let rows = n.div_ceil(cols);
        let w = cols * fw + (cols - 1) * p;
        let h = rows * fh + (rows - 1) * p;
        let (cw, ch) = if opt.pow2 { (w.next_power_of_two(), h.next_power_of_two()) } else { (w, h) };
        (cols, rows, w, h, cw, ch)
    };
    if opt.columns > 0 {
        return size(opt.columns.min(n));
    }
    if !opt.pow2 {
        return size(((n as f32).sqrt().ceil() as u32).max(1));
    }
    // smallest power-of-two area, then the squarest
    (1..=n)
        .map(size)
        .min_by_key(|&(_, _, w, h, cw, ch)| ((cw as u64) * (ch as u64), cw.max(ch), (w as i64 - h as i64).unsigned_abs()))
        .unwrap()
}

/// Build the sheet image and the top-left of every frame.
pub fn render_spritesheet(anim: &Animation, opt: &SheetOptions) -> Result<(RgbaImage, Vec<(u32, u32)>, u32, u32)> {
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let (fw, fh) = frames[0].image.dimensions();
    let (cols, rows, _, _, cw, ch) = sheet_layout(frames.len() as u32, fw, fh, opt);
    let p = opt.padding;
    let mut sheet = RgbaImage::new(cw, ch);
    let mut rects = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let x = (i as u32 % cols) * (fw + p);
        let y = (i as u32 / cols) * (fh + p);
        image::imageops::replace(&mut sheet, &*f.image, x as i64, y as i64);
        rects.push((x, y));
    }
    Ok((sheet, rects, cols, rows))
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Export a sprite sheet PNG (+ optional JSON metadata and Godot 4 SpriteFrames .tres).
pub fn export_spritesheet(anim: &Animation, png_path: &Path, opt: &SheetOptions) -> Result<String> {
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let (fw, fh) = frames[0].image.dimensions();
    let p = opt.padding;
    let (sheet, rects, cols, rows) = render_spritesheet(anim, opt)?;
    std::fs::write(png_path, encode_png(&sheet, &opt.png)?)?;
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

/// What the "compressed" preview should encode to.
#[derive(Clone, Debug)]
pub enum PreviewFormat {
    Gif(GifOptions),
    Webp(WebpOptions),
    Avif(u8),
    /// lossless: the preview is the input itself, only the size is measured
    Apng,
    Video(VideoFormat, [u8; 3]),
    /// one file per frame
    Frames(SplitFormat, PngOptions, GifOptions),
    /// the sprite sheet image itself
    Sheet(SheetOptions),
}

/// Encode `anim` like the real export would, then decode the result so the
/// preview shows exactly what the file will look like. Returns (decoded, bytes).
pub fn encode_preview(anim: &Animation, f: &PreviewFormat) -> Result<(Animation, Vec<u8>)> {
    let (w, h) = (anim.width(), anim.height());
    match f {
        PreviewFormat::Gif(o) => {
            let b = encode_gif(anim, o, None)?;
            Ok((decode_gif_bytes(&b)?, b))
        }
        PreviewFormat::Webp(o) => {
            let b = encode_webp(anim, o)?;
            Ok((load_animation_bytes(&b)?, b))
        }
        PreviewFormat::Apng => {
            let b = encode_apng(anim, false)?;
            let mut a = anim.clone();
            a.frames.retain(|f| f.enabled);
            Ok((a, b))
        }
        PreviewFormat::Frames(fmt, png, gif) => {
            use rayon::prelude::*;
            let frames = anim.enabled_frames();
            let files: Vec<Vec<u8>> = frames.par_iter().map(|f| encode_frame_file(f, *fmt, png, gif)).collect::<Result<_>>()?;
            let decoded: Vec<Frame> = files
                .par_iter()
                .zip(frames.par_iter())
                .map(|(b, f)| Ok(Frame { image: Arc::new(image::load_from_memory(b)?.to_rgba8()), delay_cs: f.delay_cs, enabled: true, cut: false }))
                .collect::<Result<_>>()?;
            // the size shown is the total of all files
            let total: Vec<u8> = files.concat();
            Ok((Animation { frames: decoded, loop_count: anim.loop_count }, total))
        }
        PreviewFormat::Sheet(opt) => {
            let (sheet, _, _, _) = render_spritesheet(anim, opt)?;
            let b = encode_png(&sheet, &opt.png)?;
            let img = image::load_from_memory(&b)?.to_rgba8();
            Ok((Animation { frames: vec![Frame::new(img, 100)], loop_count: None }, b))
        }
        PreviewFormat::Avif(q) => {
            let b = encode_avif(anim, *q)?;
            let mut dec = decode_video_bytes(&b, "avif", (w, h))?;
            // ffmpeg can't decode AVIF's alpha layer; it's stored near-losslessly,
            // so show the source's transparency
            if anim.frames.iter().any(|f| f.image.pixels().any(|p| p[3] < 255)) {
                let mut src = anim.clone();
                src.frames.retain(|f| f.enabled);
                let mut t = 0.0f32;
                for f in &mut dec.frames {
                    let s = &src.frames[crate::stack::frame_at_time(&src, t + 0.001)].image;
                    let img = Arc::make_mut(&mut f.image);
                    for (p, q) in img.pixels_mut().zip(s.pixels()) {
                        p[3] = q[3];
                    }
                    t += crate::anim::effective_delay(f.delay_cs) as f32 / 100.0;
                }
            }
            Ok((dec, b))
        }
        PreviewFormat::Video(vf, bg) => {
            static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let ext = if *vf == VideoFormat::Mp4 { "mp4" } else { "webm" };
            let tmp = std::env::temp_dir().join(format!("gifkit_prev_{}_{}.{ext}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            export_video(anim, &tmp, *vf, *bg, false)?;
            let b = std::fs::read(&tmp)?;
            let dec = if *vf == VideoFormat::WebmAlpha { Some("libvpx-vp9") } else { None };
            let a = load_video_dec(&tmp, None, Some((w, h)), dec);
            let _ = std::fs::remove_file(&tmp);
            Ok((a?, b))
        }
    }
}

fn decode_video_bytes(b: &[u8], ext: &str, size: (u32, u32)) -> Result<Animation> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp = std::env::temp_dir().join(format!("gifkit_dec_{}_{}.{ext}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::write(&tmp, b)?;
    let a = load_video_opts(&tmp, None, Some(size));
    let _ = std::fs::remove_file(&tmp);
    a
}

/// Encode an animated AVIF with ffmpeg; `quality` 0..=100.
pub fn encode_avif(anim: &Animation, quality: u8) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("gifkit_avif_{}_{}.avif", std::process::id(), quality));
    export_video_q(anim, &tmp, VideoFormat::Avif, [0, 0, 0], Some(quality), false)?;
    let b = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok(b)
}

/// Export to video via ffmpeg (must be on PATH). Uses the concat demuxer so
/// per-frame delays are preserved exactly.
pub fn export_video(anim: &Animation, path: &Path, fmt: VideoFormat, bg: [u8; 3], noai: bool) -> Result<()> {
    export_video_q(anim, path, fmt, bg, None, noai)
}

/// [`export_video`] with an optional quality (0..=100) mapped to the codec's CRF.
pub fn export_video_q(anim: &Animation, path: &Path, fmt: VideoFormat, bg: [u8; 3], quality: Option<u8>, noai: bool) -> Result<()> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let frames = anim.enabled_frames();
    anyhow::ensure!(!frames.is_empty(), "No enabled frames");
    let tmp = std::env::temp_dir().join(format!("gifkit_{}_{}", std::process::id(), RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
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

    // quality 0..100 -> CRF (lower = better)
    let crf = |best: f32, worst: f32, default: u8| -> i32 {
        match quality {
            Some(q) => (worst + (best - worst) * q.min(100) as f32 / 100.0).round() as i32,
            None => default as i32,
        }
    };
    // the graphics card first when it has an encoder for this; software after
    let hw = match fmt {
        VideoFormat::Mp4 => crate::gpu::h264(),
        // hardware AV1 can't carry AVIF's second (alpha) stream
        VideoFormat::Avif if !has_alpha => crate::gpu::av1(),
        _ => None,
    };
    let attempt = |hw: Option<&crate::gpu::HwEnc>| -> Result<()> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y").arg("-loglevel").arg("error");
        if let Some(h) = hw {
            cmd.args(h.pre_input());
        }
        cmd.arg("-f").arg("concat").arg("-safe").arg("0").arg("-i").arg(&list_path);
        let tail = hw.map(|h| h.filter_tail()).unwrap_or("");
        match fmt {
            VideoFormat::Mp4 => {
                let q = crf(12.0, 40.0, 18);
                match hw {
                    Some(h) => {
                        cmd.arg("-vf").arg(format!("pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=50{tail}"));
                        cmd.args(h.codec_args(q));
                    }
                    None => {
                        cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=50", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", &q.to_string()]);
                    }
                }
                cmd.args(["-movflags", "+faststart"]);
            }
            VideoFormat::WebmAlpha => {
                cmd.args(["-vf", "fps=50", "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-b:v", "0", "-crf", &crf(15.0, 55.0, 30).to_string(), "-auto-alt-ref", "0"]);
            }
            VideoFormat::Avif => {
                let q = crf(10.0, 55.0, 28);
                match hw {
                    Some(h) => {
                        cmd.arg("-vf").arg(format!("fps=50,pad=ceil(iw/2)*2:ceil(ih/2)*2{tail}"));
                        cmd.args(h.codec_args(q));
                    }
                    None => {
                        if has_alpha {
                            // AVIF stores alpha as a second AV1 stream
                            cmd.args(["-filter_complex", "[0:v]fps=50,pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=rgba,split[c][a];[c]format=yuv420p[cc];[a]alphaextract,format=gray[al]", "-map", "[cc]", "-map", "[al]"]);
                        } else {
                            cmd.args(["-vf", "fps=50,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p"]);
                        }
                        cmd.args(["-c:v", "libaom-av1", "-crf", &q.to_string(), "-b:v", "0", "-cpu-used", "6", "-row-mt", "1", "-still-picture", "0"]);
                    }
                }
                cmd.args(["-f", "avif"]);
                if anim.loop_count.is_none() {
                    cmd.args(["-loop", "0"]);
                }
            }
        }
        if noai {
            cmd.arg("-metadata").arg(format!("comment={}", crate::noai::COMMENT));
        }
        cmd.arg(path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().context("Failed to run ffmpeg (is it installed and on PATH?)")?;
        if !out.status.success() {
            bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(())
    };
    let used = match hw.as_ref().map(|h| (h, attempt(Some(h)))) {
        Some((h, Ok(()))) => Some(format!("{}, {}", h.label(), h.name)),
        // the card couldn't do this one (size, profile, driver): software
        _ => {
            let r = attempt(None);
            if r.is_err() {
                let _ = std::fs::remove_dir_all(&tmp);
            }
            r?;
            None
        }
    };
    crate::gpu::note_used(used);
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Decode GIF bytes back into an animation (used to preview optimisation results).
pub fn decode_gif_bytes(bytes: &[u8]) -> Result<Animation> {
    load_animation_bytes(bytes)
}
