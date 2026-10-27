//! Point tracking: follow a patch of the picture from frame to frame, so a
//! layer can stick to something that moves.
//!
//! Each frame it guesses where the patch went from how it was moving, looks
//! there first on a half-size picture over a wide area, then closes in at
//! full size. Brightness changes are ignored (each patch is compared less
//! its own average), the template slowly takes on how the thing looks now
//! while still being pulled back to how it looked at the start, and a match
//! that is much worse than usual is treated as lost: the point carries on
//! along its last heading until it's found again.

use image::RgbaImage;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The patch size used for a click: this far out from the point each way.
pub const CLICK_PATCH: i32 = 12;
/// How far (full-size pixels) it looks around where it expects the patch.
const REACH: i32 = 64;

/// A frame as brightness, at full and half size.
pub struct Gray {
    full: Plane,
    half: Plane,
}

struct Plane {
    w: i32,
    h: i32,
    v: Vec<f32>,
}

impl Plane {
    fn at(&self, x: i32, y: i32) -> f32 {
        self.v[(y.clamp(0, self.h - 1) * self.w + x.clamp(0, self.w - 1)) as usize]
    }

    /// The patch around (cx, cy) (`rx`, `ry` out each way, every `step`
    /// pixels), less its average.
    fn patch(&self, cx: f32, cy: f32, rx: i32, ry: i32) -> Vec<f32> {
        let (x0, y0) = (cx.round() as i32, cy.round() as i32);
        let mut v = Vec::with_capacity(((2 * rx + 1) * (2 * ry + 1)) as usize);
        for y in -ry..=ry {
            for x in -rx..=rx {
                v.push(self.at(x0 + x, y0 + y));
            }
        }
        let m = v.iter().sum::<f32>() / v.len() as f32;
        v.iter_mut().for_each(|p| *p -= m);
        v
    }

    /// How unlike `t` (made by `patch`) this plane is around (cx, cy):
    /// mean absolute difference, brightness evened out. Stops early past `best`.
    fn cost(&self, t: &[f32], cx: i32, cy: i32, rx: i32, ry: i32, best: f32) -> f32 {
        let n = t.len() as f32;
        let mut m = 0.0;
        for y in -ry..=ry {
            for x in -rx..=rx {
                m += self.at(cx + x, cy + y);
            }
        }
        m /= n;
        let mut sum = 0.0;
        let mut k = 0;
        let stop = best * n;
        for y in -ry..=ry {
            for x in -rx..=rx {
                sum += (self.at(cx + x, cy + y) - m - t[k]).abs();
                k += 1;
            }
            if sum > stop {
                return sum / n;
            }
        }
        sum / n
    }
}

impl Gray {
    pub fn new(img: &RgbaImage) -> Gray {
        let (w, h) = img.dimensions();
        let v: Vec<f32> = img
            .pixels()
            .map(|p| {
                // see-through counts as black, so a sprite on nothing tracks
                let a = p[3] as f32 / 255.0;
                (p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114) * a
            })
            .collect();
        let full = Plane { w: w as i32, h: h as i32, v };
        let (hw, hh) = ((w / 2).max(1) as i32, (h / 2).max(1) as i32);
        let mut hv = vec![0.0; (hw * hh) as usize];
        for y in 0..hh {
            for x in 0..hw {
                let s = full.at(2 * x, 2 * y) + full.at(2 * x + 1, 2 * y) + full.at(2 * x, 2 * y + 1) + full.at(2 * x + 1, 2 * y + 1);
                hv[(y * hw + x) as usize] = s / 4.0;
            }
        }
        Gray { full, half: Plane { w: hw, h: hh, v: hv } }
    }
}

/// Where the patch sits in `f`, looking around `guess`: a wide look on the
/// half-size picture, then a close one at full size, then a sub-pixel nudge.
/// Returns the place and how bad the match was.
fn find(f: &Gray, tmpl: &[f32], half_t: &[f32], first: &[f32], (rx, ry): (i32, i32), guess: (f32, f32)) -> ((f32, f32), f32) {
    // half size: steps of 2 full-size pixels over the whole reach
    let (hrx, hry) = ((rx / 2).max(2), (ry / 2).max(2));
    let (gx, gy) = ((guess.0 / 2.0).round() as i32, (guess.1 / 2.0).round() as i32);
    let r = REACH / 2;
    let mut best = (f32::MAX, gx, gy);
    for dy in -r..=r {
        for dx in -r..=r {
            // a small pull towards the guess, so look-alikes further off lose
            let pull = ((dx * dx + dy * dy) as f32).sqrt() * 0.15;
            let c = f.half.cost(half_t, gx + dx, gy + dy, hrx, hry, best.0 - pull) + pull;
            if c < best.0 {
                best = (c, gx + dx, gy + dy);
            }
        }
    }
    // full size, close in (wide enough to make up for what the half-size
    // look can't see: odd pixels, and fine patterns that blur when halved)
    const C: i32 = 4;
    const W: usize = (2 * C + 1) as usize;
    let (fx, fy) = (best.1 * 2, best.2 * 2);
    let mut costs = [[0.0f32; W]; W];
    let mut fb = (f32::MAX, fx, fy);
    for dy in -C..=C {
        for dx in -C..=C {
            let (x, y) = (fx + dx, fy + dy);
            // like now, and a little like the start, so it doesn't drift off
            let c = f.full.cost(tmpl, x, y, rx, ry, f32::MAX) + 0.35 * f.full.cost(first, x, y, rx, ry, f32::MAX);
            costs[(dy + C) as usize][(dx + C) as usize] = c;
            if c < fb.0 {
                fb = (c, x, y);
            }
        }
    }
    // sub-pixel: the bottom of a parabola through the best and its neighbours
    let (bx, by) = ((fb.1 - fx + C) as usize, (fb.2 - fy + C) as usize);
    let off = |l: f32, c: f32, r: f32| {
        let d = l - 2.0 * c + r;
        if d > 1e-6 { (0.5 * (l - r) / d).clamp(-0.5, 0.5) } else { 0.0 }
    };
    let sx = if (1..W - 1).contains(&bx) { off(costs[by][bx - 1], costs[by][bx], costs[by][bx + 1]) } else { 0.0 };
    let sy = if (1..W - 1).contains(&by) { off(costs[by - 1][bx], costs[by][bx], costs[by + 1][bx]) } else { 0.0 };
    ((fb.1 as f32 + sx, fb.2 as f32 + sy), fb.0 / 1.35)
}

/// Follow the patch `size` (half-width, half-height) around `at` on frame
/// `start` through `frames`, forwards and backwards: where it is on each
/// frame. `progress` counts frames done (and carries Stop).
#[cfg_attr(not(test), allow(dead_code))]
pub fn track(frames: &[&Gray], start: usize, at: (f32, f32), size: (i32, i32), progress: &AtomicUsize) -> Vec<(f32, f32)> {
    let n = frames.len();
    let mut out = vec![at; n];
    if n == 0 || start >= n {
        return out;
    }
    let mut trust = vec![true; n];
    follow(frames, start, at, size, 1, n - 1, &mut out, &mut trust, progress);
    follow(frames, start, at, size, -1, 0, &mut out, &mut trust, progress);
    out
}

/// One run of the tracker from `start` in direction `dir` up to frame
/// `until` (inclusive), writing into `out`, and into `trust` whether the
/// match there was good. Returns false if stopped.
#[allow(clippy::too_many_arguments)]
fn follow(frames: &[&Gray], start: usize, at: (f32, f32), size: (i32, i32), dir: i64, until: usize, out: &mut [(f32, f32)], trust: &mut [bool], progress: &AtomicUsize) -> bool {
    let n = frames.len();
    let (rx, ry) = (size.0.clamp(4, 80), size.1.clamp(4, 80));
    let f0 = frames[start];
    let first = f0.full.patch(at.0, at.1, rx, ry);
    let first_half = f0.half.patch(at.0 / 2.0, at.1 / 2.0, (rx / 2).max(2), (ry / 2).max(2));
    out[start] = at;
    trust[start] = true;
    // once lost for good, nothing after is trusted
    let mut gone = false;
    let mut p = at;
    let mut vel = (0.0f32, 0.0f32);
    let mut tmpl = first.clone();
    let mut half_t = first_half;
    // how good a match usually is here, to tell when it's lost
    let mut usual: Option<f32> = None;
    let mut lost = 0;
    let mut i = start as i64 + dir;
    let past = |i: i64| if dir > 0 { i > until as i64 } else { i < until as i64 };
    while i >= 0 && (i as usize) < n && !past(i) {
        if crate::stack::stopped(Some(progress)) {
            return false;
        }
        let f = frames[i as usize];
        let guess = (p.0 + vel.0, p.1 + vel.1);
        let (q, cost) = find(f, &tmpl, &half_t, &first, (rx, ry), guess);
        let bad = usual.is_some_and(|u| cost > (u * 3.0).max(18.0));
        if bad {
            gone |= lost >= 3;
        }
        if bad && lost < 8 {
            // lost for now: keep going the way it was going
            lost += 1;
            p = guess;
            vel = (vel.0 * 0.9, vel.1 * 0.9);
        } else {
            lost = 0;
            let nv = (q.0 - p.0, q.1 - p.1);
            // heading, smoothed so one odd frame doesn't throw it
            vel = (vel.0 * 0.4 + nv.0 * 0.6, vel.1 * 0.4 + nv.1 * 0.6);
            p = q;
            usual = Some(match usual {
                Some(u) => u * 0.8 + cost * 0.2,
                None => cost.max(1.0),
            });
            // the template follows how it looks now, a little at a time
            let now = f.full.patch(p.0, p.1, rx, ry);
            tmpl.iter_mut().zip(&now).for_each(|(t, c)| *t = *t * 0.7 + c * 0.3);
            let nh = f.half.patch(p.0 / 2.0, p.1 / 2.0, (rx / 2).max(2), (ry / 2).max(2));
            half_t.iter_mut().zip(&nh).for_each(|(t, c)| *t = *t * 0.7 + c * 0.3);
        }
        out[i as usize] = p;
        trust[i as usize] = !bad && !gone;
        progress.fetch_add(1, Ordering::Relaxed);
        i += dir;
    }
    true
}

/// A spot picked by hand: on frame `frame` the thing is at `at`, in a patch
/// `size` out each way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub frame: usize,
    pub at: (f32, f32),
    pub size: (i32, i32),
}

/// Track with several picked spots: each is exact on its frame. Between
/// two, it tracks on from the earlier and back from the later and blends
/// the two by how near each one is, so drift never builds up. Before the
/// first and after the last, it tracks out from that one.
pub fn track_anchors(frames: &[&Gray], anchors: &[Anchor], progress: &AtomicUsize) -> Vec<(f32, f32)> {
    let n = frames.len();
    let mut a: Vec<Anchor> = anchors.iter().copied().filter(|a| a.frame < n).collect();
    a.sort_by_key(|a| a.frame);
    a.dedup_by_key(|a| a.frame);
    if n == 0 || a.is_empty() {
        return vec![(0.0, 0.0); n];
    }
    let mut out = vec![a[0].at; n];
    let mut scratch = vec![true; n];
    // out before the first, out after the last
    follow(frames, a[0].frame, a[0].at, a[0].size, -1, 0, &mut out, &mut scratch, progress);
    let l = a[a.len() - 1];
    follow(frames, l.frame, l.at, l.size, 1, n - 1, &mut out, &mut scratch, progress);
    for w in a.windows(2) {
        let (p, q) = (w[0], w[1]);
        let (mut fwd, mut back) = (vec![(0.0, 0.0); n], vec![(0.0, 0.0); n]);
        let (mut tf, mut tb) = (vec![false; n], vec![false; n]);
        follow(frames, p.frame, p.at, p.size, 1, q.frame, &mut fwd, &mut tf, progress);
        follow(frames, q.frame, q.at, q.size, -1, p.frame, &mut back, &mut tb, progress);
        let span = (q.frame - p.frame) as f32;
        for i in p.frame..=q.frame {
            let t = (i - p.frame) as f32 / span.max(1.0);
            let blend = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 * (1.0 - t) + b.0 * t, a.1 * (1.0 - t) + b.1 * t);
            out[i] = match (tf[i], tb[i]) {
                // both found it: lean on the nearer one
                (true, true) => blend(fwd[i], back[i], t),
                // only one did: that one
                (true, false) => fwd[i],
                (false, true) => back[i],
                // neither: a straight line between the two spots
                (false, false) => blend(p.at, q.at, t),
            };
        }
    }
    for k in &a {
        out[k.frame] = k.at;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn noise_bg(w: u32, h: u32, seed: u32) -> Vec<u8> {
        let mut s = seed;
        (0..w * h)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 26) as u8 * 3
            })
            .collect()
    }

    /// A textured square at (x0, y0), `light` added to its brightness.
    fn frame(bg: &[u8], x0: f32, y0: f32, light: i32) -> RgbaImage {
        RgbaImage::from_fn(200, 150, |x, y| {
            let (fx, fy) = (x as f32 - x0, y as f32 - y0);
            let inside = (0.0..20.0).contains(&fx) && (0.0..20.0).contains(&fy);
            let v = if inside {
                let c = if (fx as i32 / 5 + fy as i32 / 5) % 2 == 0 { 230 } else { 90 };
                (c + light).clamp(0, 255) as u8
            } else {
                bg[(y * 200 + x) as usize]
            };
            Rgba([v, v, v, 255])
        })
    }

    fn check(path: &[(f32, f32)], light: impl Fn(usize) -> i32, start: usize, tol: f32) {
        let bg = noise_bg(200, 150, 7);
        let grays: Vec<Gray> = path.iter().enumerate().map(|(i, &(x, y))| Gray::new(&frame(&bg, x, y, light(i)))).collect();
        let refs: Vec<&Gray> = grays.iter().collect();
        let at = (path[start].0 + 10.0, path[start].1 + 10.0);
        let got = track(&refs, start, at, (CLICK_PATCH, CLICK_PATCH), &AtomicUsize::new(0));
        for (i, (&(x, y), g)) in path.iter().zip(&got).enumerate() {
            let want = (x + 10.0, y + 10.0);
            assert!((g.0 - want.0).abs() <= tol && (g.1 - want.1).abs() <= tol, "frame {i}: {g:?} not {want:?}");
        }
    }

    #[test]
    fn follows_a_moving_square() {
        let path: Vec<(f32, f32)> = (0..12).map(|i| (10.0 + i as f32 * 7.0, 20.0 + i as f32 * 4.0)).collect();
        check(&path, |_| 0, 5, 1.0);
    }

    #[test]
    fn follows_fast_motion() {
        // 30 px a frame: past the old reach
        let path: Vec<(f32, f32)> = (0..6).map(|i| (5.0 + i as f32 * 30.0, 60.0 + (i % 2) as f32 * 8.0)).collect();
        check(&path, |_| 0, 0, 1.0);
    }

    #[test]
    fn keeps_hold_through_light_changes() {
        let path: Vec<(f32, f32)> = (0..10).map(|i| (20.0 + i as f32 * 12.0, 40.0 + i as f32 * 6.0)).collect();
        check(&path, |i| (i as i32 - 5) * 12, 0, 1.0);
    }

    #[test]
    fn follows_a_curve() {
        let path: Vec<(f32, f32)> = (0..24).map(|i| {
            let a = i as f32 * 0.26;
            (90.0 + 55.0 * a.cos(), 65.0 + 45.0 * a.sin())
        }).collect();
        // (drawn on whole pixels, so the truth itself is only good to half a pixel)
        check(&path, |_| 0, 0, 1.5);
    }

    #[test]
    fn anchors_fix_what_tracking_misses() {
        // the square jumps across the frame half way: tracking from the start
        // alone loses it, a second picked spot after the jump gets it back
        let bg = noise_bg(200, 150, 9);
        let path: Vec<(f32, f32)> = (0..10).map(|i| if i < 5 { (20.0 + i as f32 * 3.0, 30.0) } else { (150.0 + i as f32 * 3.0 - 15.0, 110.0) }).collect();
        let grays: Vec<Gray> = path.iter().map(|&(x, y)| Gray::new(&frame(&bg, x, y, 0))).collect();
        let refs: Vec<&Gray> = grays.iter().collect();
        let c = |i: usize| (path[i].0 + 10.0, path[i].1 + 10.0);
        let one = track(&refs, 0, c(0), (9, 9), &AtomicUsize::new(0));
        assert!((one[8].0 - c(8).0).abs() > 20.0, "alone it can't jump that far");
        // a box drawn round the square: the patch fits it
        let anchors = [Anchor { frame: 0, at: c(0), size: (9, 9) }, Anchor { frame: 6, at: c(6), size: (9, 9) }];
        let got = track_anchors(&refs, &anchors, &AtomicUsize::new(0));
        for i in [1, 2, 3, 7, 8, 9] {
            assert!((got[i].0 - c(i).0).abs() <= 1.0 && (got[i].1 - c(i).1).abs() <= 1.0, "frame {i}: {:?} not {:?}", got[i], c(i));
        }
    }
}
