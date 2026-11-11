//! Changing an animation's frame rate while keeping its length: new frames
//! picked (or blended) from the old ones at the new rate, and everything
//! that counts in frames moved to the same moments.

use crate::anim::{effective_delay, Animation, Frame};
use crate::pixels::Pixels;
use crate::stack::{plan_times, Stack};
use image::RgbaImage;
use rayon::prelude::*;
use std::sync::Arc;

/// How new frames get their picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    /// the frame showing at that moment: frames repeat or are dropped
    Nearest,
    /// slower: the frames it spans mixed (a motion blur); faster: a fade
    /// between the two around it
    Blend,
}

/// Where the old frames sit in time, in hundredths of a second: (start,
/// length). Skipped frames take no time.
fn spans(base: &Animation) -> Vec<(f64, f64)> {
    let mut t = 0.0;
    base.frames
        .iter()
        .map(|f| {
            let d = if f.enabled { effective_delay(f.delay_cs) as f64 } else { 0.0 };
            let s = (t, d);
            t += d;
            s
        })
        .collect()
}

/// How many frames the animation has at `fps`.
pub fn frames_at(base: &Animation, fps: f64) -> usize {
    let total: f64 = spans(base).iter().map(|s| s.1).sum();
    ((total * fps / 100.0).round() as usize).max(1)
}

/// `pics` mixed by weight, colour weighted by how solid it is.
fn mix(pics: &[(Arc<RgbaImage>, f64)]) -> RgbaImage {
    let (w, h) = pics[0].0.dimensions();
    let sum: f64 = pics.iter().map(|p| p.1).sum::<f64>().max(1e-9);
    let ws: Vec<f32> = pics.iter().map(|p| (p.1 / sum) as f32).collect();
    let mut out = RgbaImage::new(w, h);
    out.as_mut()
        .par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let off = y * w as usize * 4;
            for x in 0..w as usize {
                let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
                for (p, wt) in pics.iter().zip(&ws) {
                    let px = &p.0.as_raw()[off + x * 4..off + x * 4 + 4];
                    let pa = px[3] as f32 * wt;
                    r += px[0] as f32 * pa;
                    g += px[1] as f32 * pa;
                    b += px[2] as f32 * pa;
                    a += pa;
                }
                let q = &mut row[x * 4..x * 4 + 4];
                if a > 0.0 {
                    q[0] = (r / a).round().min(255.0) as u8;
                    q[1] = (g / a).round().min(255.0) as u8;
                    q[2] = (b / a).round().min(255.0) as u8;
                }
                q[3] = a.round().min(255.0) as u8;
            }
        });
    out
}

impl Stack {
    /// Remake `base` at `fps` frames per second, the same length. Layers,
    /// clips, keyframes, markers and ranges move to the same moments.
    /// Skipped frames are left out (they don't play). False when there's
    /// nothing to do it to.
    pub fn change_frame_rate(&mut self, base: &mut Animation, fps: f64, how: Resample) -> bool {
        let fps = fps.clamp(crate::io::MIN_FPS, crate::io::MAX_FPS);
        let old_spans = spans(base);
        let total: f64 = old_spans.iter().map(|s| s.1).sum();
        if total <= 0.0 {
            return false;
        }
        let n = frames_at(base, fps);
        let step = 100.0 / fps;
        // the playing frame at time t (cs)
        let playing: Vec<usize> = (0..base.len()).filter(|&i| old_spans[i].1 > 0.0).collect();
        let at = |t: f64| -> usize {
            let k = playing.partition_point(|&i| old_spans[i].0 <= t + 1e-9);
            playing[k.saturating_sub(1)]
        };
        // the new frames' times and delays: each ends at its exact time
        // rounded, the last one at the old end
        let edge = |k: usize| ((k as f64 * step).min(total)).round() as i64;
        let delays: Vec<u16> = (0..n)
            .map(|k| {
                let end = if k + 1 == n { total.round() as i64 } else { edge(k + 1) };
                (end - edge(k)).clamp(2, u16::MAX as i64) as u16
            })
            .collect();
        let frames: Vec<Frame> = (0..n)
            .into_par_iter()
            .map(|k| {
                let (t0, t1) = (k as f64 * step, ((k + 1) as f64 * step).min(total));
                let i = at(t0);
                let src = &base.frames[i];
                let mut f = Frame { image: src.image.clone(), delay_cs: delays[k], enabled: true, cut: false, blank: src.blank, trim: src.trim };
                if how == Resample::Blend && !src.blank {
                    // every playing frame this one spans, by how long
                    let over: Vec<(usize, f64)> = playing
                        .iter()
                        .filter_map(|&j| {
                            let (s, d) = old_spans[j];
                            let o = (t1.min(s + d) - t0.max(s)).max(0.0);
                            (o > 1e-6).then_some((j, o))
                        })
                        .collect();
                    let pics: Vec<(usize, f64)> = if over.len() > 1 {
                        over
                    } else {
                        // inside one frame: fade toward the next or the last
                        // one by how far the middle is from its middle
                        let c = (t0 + t1) / 2.0;
                        let (s, d) = old_spans[i];
                        let ci = s + d / 2.0;
                        let pos = playing.iter().position(|&j| j == i).unwrap_or(0);
                        let other = if c < ci { pos.checked_sub(1) } else { Some(pos + 1).filter(|p| *p < playing.len()) };
                        match other.map(|p| playing[p]) {
                            Some(j) => {
                                let (sj, dj) = old_spans[j];
                                let cj = sj + dj / 2.0;
                                let wj = ((c - ci).abs() / (cj - ci).abs().max(1e-9)).clamp(0.0, 0.5);
                                vec![(i, 1.0 - wj), (j, wj)]
                            }
                            None => vec![(i, 1.0)],
                        }
                    };
                    let pics: Vec<(usize, f64)> = pics.into_iter().filter(|(j, w)| *w > 0.004 && !base.frames[*j].blank).collect();
                    if pics.len() > 1 {
                        let imgs: Vec<(Arc<RgbaImage>, f64)> = pics.iter().map(|(j, w)| (base.frames[*j].image.get(), *w)).collect();
                        if imgs.iter().all(|p| p.0.dimensions() == imgs[0].0.dimensions()) {
                            f.image = Pixels::new(mix(&imgs));
                        }
                    }
                }
                f
            })
            .collect();
        // old source frame → the new frames covering the same time
        let first = |i: usize| -> usize { ((old_spans[i].0 / step + 1e-6).floor() as usize).min(n - 1) };
        let last = |i: usize| -> usize {
            let (s, d) = old_spans[i];
            (((s + d) / step - 1e-6).ceil() as usize).saturating_sub(1).clamp(first(i), n - 1)
        };
        let old_n = base.len();
        // a 1-based range (`to` 0: to the end); past the end: shows nowhere
        let range = |from: usize, to: usize| -> (usize, usize) {
            if from > old_n {
                return (n + 1, n + 1);
            }
            let f = first(from.max(1) - 1) + 1;
            let t = if to == 0 { 0 } else if to > old_n { n + 1 } else { (last(to - 1) + 1).max(f) };
            (f, t)
        };
        // the timeline: output frames to new output frames by time, with
        // the speed envelope set aside (it counts in output frames too)
        let env = std::mem::take(&mut self.speed_env);
        let old_times = plan_times(&self.plan(base));
        let old_plan = self.plan(base);
        let new_base = Animation { frames, loop_count: base.loop_count };
        let mut cut_frames = vec![false; n];
        for (i, f) in base.frames.iter().enumerate() {
            if f.cut && old_spans[i].1 > 0.0 {
                cut_frames[first(i)] = true;
            }
        }
        let mut new_base = new_base;
        for (f, c) in new_base.frames.iter_mut().zip(cut_frames) {
            f.cut = c;
        }
        // frame ranges in source frames
        for l in &mut self.layers {
            use crate::stack::LayerKind;
            match &mut l.kind {
                LayerKind::Text(t) => (t.from, t.to) = range(t.from, t.to),
                LayerKind::Censor(c) => (c.from, c.to) = range(c.from, c.to),
                LayerKind::Shape(sh) => (sh.from, sh.to) = range(sh.from, sh.to),
                LayerKind::Emoji(em) => (em.from, em.to) = range(em.from, em.to),
                LayerKind::Markup(mk) => {
                    for st in &mut mk.strokes {
                        (st.from, st.to) = range(st.from, st.to);
                    }
                }
                _ => {}
            }
        }
        let tr = &mut self.transform;
        if !tr.cut_by_time {
            let end = if tr.cut_end == 0 { 0 } else { tr.cut_end.min(old_n) };
            (tr.cut_start, tr.cut_end) = range(tr.cut_start.clamp(1, old_n), end);
            if tr.cut_end > n {
                tr.cut_end = 0;
            }
        }
        let new_times = plan_times(&self.plan(&new_base));
        let new_n = new_times.len();
        let out_first = |i: usize| -> usize {
            let t = old_times.get(i).copied().unwrap_or(f32::MAX);
            new_times.partition_point(|x| *x <= t + 1e-4).saturating_sub(1)
        };
        let out_last = |i: usize| -> usize {
            let i = i.min(old_times.len().saturating_sub(1));
            let d = old_plan.get(i).filter(|f| f.enabled).map(|f| effective_delay(f.delay) as f32 / 100.0).unwrap_or(0.0);
            let end = old_times.get(i).copied().unwrap_or(0.0) + d;
            new_times.partition_point(|x| *x < end - 1e-4).saturating_sub(1).max(out_first(i))
        };
        for l in &mut self.layers {
            if let Some(clips) = &mut l.timing.clips {
                for c in clips.iter_mut() {
                    let s = out_first(c.start);
                    c.end = c.end.map(|e| out_last(e).max(s).min(new_n.saturating_sub(1)));
                    c.start = s;
                }
            }
            for k in &mut l.timing.keys {
                k.frame = out_first(k.frame);
            }
            // two keys landing on one frame: the later one wins
            l.timing.keys.reverse();
            let mut seen = std::collections::HashSet::new();
            l.timing.keys.retain(|k| seen.insert(k.frame));
            l.timing.keys.reverse();
        }
        for m in &mut self.markers {
            m.frame = out_first(m.frame);
        }
        self.speed_env = env;
        for p in &mut self.speed_env {
            p.frame = out_first(p.frame);
        }
        let mut seen = std::collections::HashSet::new();
        self.speed_env.reverse();
        self.speed_env.retain(|p| seen.insert(p.frame));
        self.speed_env.reverse();
        *base = new_base;
        true
    }
}
