//! Core data model: an animation is a list of fully-composited RGBA frames.
//!
//! Frames are always stored "unoptimized" (every frame is a full canvas image),
//! so every tool can treat frames independently. Optimisation (frame
//! differencing, palette reduction) only happens at export time.

use image::RgbaImage;
use std::sync::Arc;

#[derive(Clone)]
pub struct Frame {
    pub image: Arc<RgbaImage>,
    /// Delay in 1/100 s (GIF native unit).
    pub delay_cs: u16,
    /// Disabled ("skipped") frames are hidden from playback and export.
    pub enabled: bool,
    /// A clip on the timeline's GIF track starts here (a split).
    pub cut: bool,
}

impl Frame {
    pub fn new(image: RgbaImage, delay_cs: u16) -> Self {
        Self { image: Arc::new(image), delay_cs, enabled: true, cut: false }
    }
    pub fn with_image(&self, image: RgbaImage) -> Self {
        Self { image: Arc::new(image), delay_cs: self.delay_cs, enabled: self.enabled, cut: self.cut }
    }
}

#[derive(Clone)]
pub struct Animation {
    pub frames: Vec<Frame>,
    /// None = loop forever, Some(n) = play n times in total.
    pub loop_count: Option<u16>,
}

impl Animation {
    pub fn width(&self) -> u32 {
        self.frames.first().map(|f| f.image.width()).unwrap_or(0)
    }
    pub fn height(&self) -> u32 {
        self.frames.first().map(|f| f.image.height()).unwrap_or(0)
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn enabled_count(&self) -> usize {
        self.frames.iter().filter(|f| f.enabled).count()
    }
    /// Total duration in seconds of enabled frames.
    pub fn duration_secs(&self) -> f32 {
        self.frames
            .iter()
            .filter(|f| f.enabled)
            .map(|f| effective_delay(f.delay_cs) as f32 / 100.0)
            .sum()
    }
    /// Only the frames that will be exported.
    pub fn enabled_frames(&self) -> Vec<Frame> {
        self.frames.iter().filter(|f| f.enabled).cloned().collect()
    }
    /// Map every frame image through `f` (in parallel), keeping delays/flags.
    pub fn map_images<F>(&self, f: F) -> Animation
    where
        F: Fn(usize, &RgbaImage) -> RgbaImage + Sync,
    {
        use rayon::prelude::*;
        let frames = self
            .frames
            .par_iter()
            .enumerate()
            .map(|(i, fr)| fr.with_image(f(i, &fr.image)))
            .collect();
        Animation { frames, loop_count: self.loop_count }
    }
}

/// Browsers treat delays below 2cs as 10cs. Mirror that for playback/timing.
pub fn effective_delay(delay_cs: u16) -> u16 {
    if delay_cs < 2 { 10 } else { delay_cs }
}
