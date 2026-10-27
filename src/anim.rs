//! Core data model: an animation is a list of fully-composited RGBA frames.
//!
//! Frames are always stored "unoptimized" (every frame is a full canvas image),
//! so every tool can treat frames independently. Optimisation (frame
//! differencing, palette reduction) only happens at export time.

use crate::pixels::Pixels;
use image::RgbaImage;

#[derive(Clone)]
pub struct Frame {
    pub image: Pixels,
    /// Delay in 1/100 s (GIF native unit).
    pub delay_cs: u16,
    /// Disabled ("skipped") frames are hidden from playback and export.
    pub enabled: bool,
    /// A clip on the timeline's GIF track starts here (a split).
    pub cut: bool,
    /// A gap on the GIF track: plays for its delay but shows nothing.
    pub blank: bool,
    /// A gap frame trimmed off a GIF clip, kept so pulling the edge back
    /// brings it back: 1 = off the end of the clip before, 2 = off the start
    /// of the clip after (in the order the timeline shows).
    pub trim: u8,
}

impl Frame {
    pub fn new(image: RgbaImage, delay_cs: u16) -> Self {
        Self { image: Pixels::new(image), delay_cs, enabled: true, cut: false, blank: false, trim: 0 }
    }
    pub fn with_image(&self, image: RgbaImage) -> Self {
        Self { image: Pixels::new(image), delay_cs: self.delay_cs, enabled: self.enabled, cut: self.cut, blank: self.blank, trim: self.trim }
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
    /// Frames held for several times the usual frame length become that
    /// many frames sharing one picture, so layers that move (shakes, fades,
    /// keyframes) keep moving while the picture holds. Saving merges them
    /// back (identical frames in a row become one). Returns how many were added.
    pub fn split_holds(&mut self) -> usize {
        // the usual frame length: the commonest one (the shorter on a tie)
        let mut count: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for f in self.frames.iter().filter(|f| f.enabled) {
            *count.entry(effective_delay(f.delay_cs)).or_insert(0) += 1;
        }
        let Some(base) = count.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(d, _)| *d) else { return 0 };
        let base = base.max(2);
        let parts = |f: &Frame| -> usize {
            let d = effective_delay(f.delay_cs);
            if f.enabled && d >= base * 2 { ((d as f32 / base as f32).round() as usize).max(1) } else { 1 }
        };
        let total: usize = self.frames.iter().map(parts).sum();
        // very long holds in a long animation: leave it be
        if total == self.frames.len() || total > 20000.max(self.frames.len()) {
            return 0;
        }
        let mut out = Vec::with_capacity(total);
        for f in &self.frames {
            let n = parts(f);
            if n == 1 {
                out.push(f.clone());
                continue;
            }
            let d = effective_delay(f.delay_cs);
            for k in 0..n {
                // the last part takes what's left, so the total time is the same
                let delay = if k + 1 == n { d - base * (n as u16 - 1) } else { base };
                out.push(Frame { delay_cs: delay, cut: f.cut && k == 0, ..f.clone() });
            }
        }
        let added = out.len() - self.frames.len();
        self.frames = out;
        added
    }

    /// The frames to export, with identical frames in a row merged into one
    /// (their times added): held pictures go back to being one frame.
    pub fn enabled_frames_merged(&self) -> Vec<Frame> {
        let mut out: Vec<Frame> = Vec::new();
        for f in self.frames.iter().filter(|f| f.enabled) {
            if let Some(last) = out.last_mut() {
                let same = crate::pixels::Pixels::ptr_eq(&last.image, &f.image) || (last.image.dimensions() == f.image.dimensions() && last.image.get().as_raw() == f.image.get().as_raw());
                if same && (last.delay_cs as u32 + f.delay_cs as u32) < 65535 {
                    last.delay_cs += f.delay_cs;
                    continue;
                }
            }
            out.push(f.clone());
        }
        out
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
            .map(|(i, fr)| fr.with_image(f(i, &fr.image.get())))
            .collect();
        Animation { frames, loop_count: self.loop_count }
    }
}

/// Browsers treat delays below 2cs as 10cs. Mirror that for playback/timing.
pub fn effective_delay(delay_cs: u16) -> u16 {
    if delay_cs < 2 { 10 } else { delay_cs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_split_into_shared_frames() {
        let img = image::RgbaImage::new(4, 4);
        let mut a = Animation { frames: vec![Frame::new(img.clone(), 4), Frame::new(img.clone(), 4), Frame::new(img.clone(), 100), Frame::new(img, 4)], loop_count: None };
        let before = a.duration_secs();
        assert_eq!(a.split_holds(), 24);
        assert_eq!(a.len(), 28);
        assert!((a.duration_secs() - before).abs() < 1e-4);
        // the parts share one picture
        assert!(crate::pixels::Pixels::ptr_eq(&a.frames[2].image, &a.frames[26].image));
        // even frames: nothing to do
        let mut b = Animation { frames: vec![Frame::new(image::RgbaImage::new(2, 2), 5); 3], loop_count: None };
        assert_eq!(b.split_holds(), 0);
    }
}
