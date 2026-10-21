//! Video on the graphics card through ffmpeg: hardware decoding when videos
//! come in, and hardware H.264 and AV1 encoders for MP4 and AVIF. Each
//! encoder is tried once on a tiny clip; whatever works is used, and the
//! software codecs are the fallback for everything (and for any file the
//! hardware gives up on).
//!
//! GIF, WebP and APNG have no hardware encoders anywhere: they stay on the
//! CPU, spread over all cores.

use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Settings ▸ Use Hardware Acceleration (on unless turned off): video on
/// the card here, and frames on the card in `gpu_render`.
pub fn enabled() -> bool {
    crate::theme::get_setting("hw_accel").map(|v| v != "0").unwrap_or(true)
}

pub fn set_enabled(on: bool) {
    crate::theme::set_setting("hw_accel", if on { "1" } else { "0" });
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Nvenc,
    Vaapi,
    Qsv,
    Amf,
    Mf,
    VideoToolbox,
}

/// A hardware encoder that worked on this machine.
#[derive(Clone, Debug)]
pub struct HwEnc {
    pub name: &'static str,
    pub kind: Kind,
    /// the render node, for VAAPI
    device: Option<String>,
}

impl HwEnc {
    /// Readable name for messages.
    pub fn label(&self) -> &'static str {
        match self.kind {
            Kind::Nvenc => "NVIDIA NVENC",
            Kind::Vaapi => "VA-API",
            Kind::Qsv => "Intel Quick Sync",
            Kind::Amf => "AMD AMF",
            Kind::Mf => "Media Foundation",
            Kind::VideoToolbox => "VideoToolbox",
        }
    }

    /// Arguments that go before the input.
    pub fn pre_input(&self) -> Vec<String> {
        match (&self.kind, &self.device) {
            (Kind::Vaapi, Some(d)) => vec!["-vaapi_device".into(), d.clone()],
            _ => Vec::new(),
        }
    }

    /// What ends the filter chain: the pixel layout the encoder takes, and
    /// for VAAPI the upload to the card.
    pub fn filter_tail(&self) -> &'static str {
        match self.kind {
            Kind::Vaapi => ",format=nv12,hwupload",
            Kind::Qsv | Kind::Amf | Kind::Mf => ",format=nv12",
            Kind::Nvenc | Kind::VideoToolbox => ",format=yuv420p",
        }
    }

    /// The codec and its quality, from a CRF-like value (lower is better).
    pub fn codec_args(&self, crf: i32) -> Vec<String> {
        let q = crf.clamp(1, 51).to_string();
        let pct = (100 - crf * 2).clamp(1, 100).to_string();
        let mut a: Vec<String> = vec!["-c:v".into(), self.name.into()];
        let more: Vec<&str> = match self.kind {
            Kind::Nvenc => vec!["-preset", "p5", "-rc", "vbr", "-cq", &q, "-b:v", "0"],
            Kind::Vaapi => vec!["-rc_mode", "CQP", "-qp", &q],
            Kind::Qsv => vec!["-global_quality", &q],
            Kind::Amf => vec!["-rc", "cqp", "-qp_i", &q, "-qp_p", &q],
            Kind::Mf => vec!["-rate_control", "quality", "-quality", &pct],
            Kind::VideoToolbox => vec!["-q:v", &pct],
        };
        a.extend(more.into_iter().map(String::from));
        a
    }
}

/// VAAPI render nodes, first card first.
fn render_nodes() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|d| d.flatten().map(|e| e.path().to_string_lossy().into_owned()).filter(|p| p.contains("renderD")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Encoders worth trying here, most capable first.
fn candidates(codec: &str) -> Vec<HwEnc> {
    let mut v = Vec::new();
    let mut add = |name: &'static str, kind: Kind, device: Option<String>| v.push(HwEnc { name, kind, device });
    let (nv, va, qsv, amf, mf, vt) = match codec {
        "h264" => ("h264_nvenc", "h264_vaapi", "h264_qsv", "h264_amf", "h264_mf", Some("h264_videotoolbox")),
        _ => ("av1_nvenc", "av1_vaapi", "av1_qsv", "av1_amf", "av1_mf", None),
    };
    if cfg!(target_os = "macos") {
        if let Some(vt) = vt {
            add(vt, Kind::VideoToolbox, None);
        }
        return v;
    }
    add(nv, Kind::Nvenc, None);
    if cfg!(target_os = "linux") {
        for d in render_nodes() {
            add(va, Kind::Vaapi, Some(d));
        }
    }
    add(qsv, Kind::Qsv, None);
    if cfg!(windows) {
        add(amf, Kind::Amf, None);
        add(mf, Kind::Mf, None);
    }
    v
}

/// Run ffmpeg, giving up after `limit` (a driver can hang on a bad card).
fn run_quiet(args: &[String], limit: Duration) -> bool {
    let mut cmd = crate::io::hidden_cmd("ffmpeg");
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = cmd.spawn() else { return false };
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.success(),
            Ok(None) if t0.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Does this encoder really work (a driver, a card and the right build)?
fn works(e: &HwEnc) -> bool {
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];
    a.extend(e.pre_input());
    a.extend(["-f", "lavfi", "-i", "testsrc2=s=256x256:r=10:d=0.5"].map(String::from));
    a.push("-vf".into());
    a.push(format!("null{}", e.filter_tail()));
    a.extend(e.codec_args(28));
    a.extend(["-frames:v", "4", "-f", "null", "-"].map(String::from));
    run_quiet(&a, Duration::from_secs(8))
}

fn find(codec: &'static str) -> Option<HwEnc> {
    if !crate::io::ffmpeg_available() {
        return None;
    }
    candidates(codec).into_iter().find(works)
}

static H264: OnceLock<Option<HwEnc>> = OnceLock::new();
static AV1: OnceLock<Option<HwEnc>> = OnceLock::new();

/// The hardware H.264 encoder to use, if there is one and it's turned on.
pub fn h264() -> Option<HwEnc> {
    enabled().then(|| H264.get_or_init(|| find("h264")).clone()).flatten()
}

/// The hardware AV1 encoder to use, if there is one and it's turned on.
pub fn av1() -> Option<HwEnc> {
    enabled().then(|| AV1.get_or_init(|| find("av1")).clone()).flatten()
}

/// Look for hardware encoders in the background, so the first export
/// doesn't wait for it.
pub fn warm_up() {
    std::thread::spawn(|| {
        crate::stack::lower_thread_priority();
        let _ = H264.get_or_init(|| find("h264"));
        let _ = AV1.get_or_init(|| find("av1"));
    });
}

/// What was found, for the Settings menu (None while still looking).
#[cfg_attr(not(test), allow(dead_code))]
pub fn summary() -> Option<String> {
    let (h, a) = (H264.get()?, AV1.get()?);
    Some(match (h, a) {
        (None, None) => "No hardware encoder found · decoding still tries the card".into(),
        (Some(h), None) => format!("H.264 on {}", h.label()),
        (None, Some(a)) => format!("AV1 on {}", a.label()),
        (Some(h), Some(a)) if h.kind == a.kind => format!("H.264 and AV1 on {}", h.label()),
        (Some(h), Some(a)) => format!("H.264 on {}, AV1 on {}", h.label(), a.label()),
    })
}

/// Arguments that decode on the card when it can (ffmpeg picks the API and
/// falls back to software for codecs the card doesn't do).
pub fn decode_args() -> Vec<&'static str> {
    if enabled() {
        vec!["-hwaccel", "auto"]
    } else {
        Vec::new()
    }
}

static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Remember what the last video export ran on, for its message.
pub fn note_used(what: Option<String>) {
    if let Ok(mut l) = LAST.lock() {
        *l = what;
    }
}

/// " · on the graphics card (…)" after a hardware export, else "".
pub fn last_used_note() -> String {
    LAST.lock().ok().and_then(|l| l.clone()).map(|w| format!(" · encoded on the graphics card ({w})")).unwrap_or_default()
}
