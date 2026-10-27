//! Point tracking: follow a small patch of the picture from frame to frame,
//! so a layer can stick to something that moves.

use image::RgbaImage;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The patch matched: this many pixels out from the point each way.
const PATCH: i32 = 10;
/// How far it looks for the patch in the next frame.
const REACH: i32 = 24;

/// A frame as brightness, for matching.
pub struct Gray {
    w: i32,
    h: i32,
    v: Vec<u8>,
}

impl Gray {
    pub fn new(img: &RgbaImage) -> Gray {
        let (w, h) = img.dimensions();
        let v = img.pixels().map(|p| {
            // see-through counts as black, so a moving sprite on nothing tracks
            let a = p[3] as u32;
            (((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) * a / 255) as u8
        }).collect();
        Gray { w: w as i32, h: h as i32, v }
    }

    fn at(&self, x: i32, y: i32) -> u8 {
        self.v[(y.clamp(0, self.h - 1) * self.w + x.clamp(0, self.w - 1)) as usize]
    }

    fn patch(&self, cx: i32, cy: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(((2 * PATCH + 1) * (2 * PATCH + 1)) as usize);
        for y in -PATCH..=PATCH {
            for x in -PATCH..=PATCH {
                out.push(self.at(cx + x, cy + y));
            }
        }
        out
    }

    /// How unlike `patch` this frame is around (cx, cy), stopping early
    /// once it's worse than `best`.
    fn cost(&self, patch: &[u8], cx: i32, cy: i32, best: u32) -> u32 {
        let mut sum = 0u32;
        let mut k = 0;
        for y in -PATCH..=PATCH {
            for x in -PATCH..=PATCH {
                sum += self.at(cx + x, cy + y).abs_diff(patch[k]) as u32;
                k += 1;
            }
            if sum >= best {
                return sum;
            }
        }
        sum
    }

    /// Where `patch` sits nearest (cx, cy).
    fn find(&self, patch: &[u8], first: &[u8], cx: i32, cy: i32) -> (i32, i32) {
        let mut best = (u32::MAX, cx, cy);
        for dy in -REACH..=REACH {
            for dx in -REACH..=REACH {
                let (x, y) = (cx + dx, cy + dy);
                // like the last frame's patch, and a little like the first
                // one, so it doesn't drift off
                let c = self.cost(patch, x, y, best.0);
                if c >= best.0 {
                    continue;
                }
                let c = c + self.cost(first, x, y, u32::MAX) / 3;
                // a small pull towards not moving, for flat areas
                let c = c + ((dx * dx + dy * dy) as u32) / 4;
                if c < best.0 {
                    best = (c, x, y);
                }
            }
        }
        (best.1, best.2)
    }
}

/// Follow the point `at` on frame `start` through `frames` (forwards and
/// backwards from there): where it is on each frame. `progress` counts
/// frames done (and carries Stop).
pub fn track(frames: &[&Gray], start: usize, at: (f32, f32), progress: &AtomicUsize) -> Vec<(f32, f32)> {
    let n = frames.len();
    let mut out = vec![at; n];
    if n == 0 || start >= n {
        return out;
    }
    let p0 = (at.0.round() as i32, at.1.round() as i32);
    let first = frames[start].patch(p0.0, p0.1);
    for dir in [1i64, -1] {
        let (mut p, mut patch) = (p0, first.clone());
        let mut i = start as i64 + dir;
        while i >= 0 && (i as usize) < n {
            if crate::stack::stopped(Some(progress)) {
                return out;
            }
            let f = frames[i as usize];
            p = f.find(&patch, &first, p.0, p.1);
            patch = f.patch(p.0, p.1);
            out[i as usize] = (p.0 as f32 + (at.0 - p0.0 as f32), p.1 as f32 + (at.1 - p0.1 as f32));
            progress.fetch_add(1, Ordering::Relaxed);
            i += dir;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn follows_a_moving_square() {
        // a textured square drifting across noise
        let mut seed = 7u32;
        let bg: Vec<u8> = (0..120 * 90).map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 26) as u8 * 3
        }).collect();
        let frame = |x0: u32, y0: u32| {
            RgbaImage::from_fn(120, 90, |x, y| {
                let inside = x >= x0 && x < x0 + 16 && y >= y0 && y < y0 + 16;
                let v = if inside { if (x - x0) / 4 % 2 == (y - y0) / 4 % 2 { 250 } else { 120 } } else { bg[(y * 120 + x) as usize] };
                Rgba([v, v, v, 255])
            })
        };
        let path: Vec<(u32, u32)> = (0..12).map(|i| (10 + i * 7, 20 + i * 4)).collect();
        let grays: Vec<Gray> = path.iter().map(|&(x, y)| Gray::new(&frame(x, y))).collect();
        let refs: Vec<&Gray> = grays.iter().collect();
        // start in the middle, on the square's centre
        let s = 5;
        let at = (path[s].0 as f32 + 8.0, path[s].1 as f32 + 8.0);
        let got = track(&refs, s, at, &AtomicUsize::new(0));
        for (i, (&(x, y), g)) in path.iter().zip(&got).enumerate() {
            let want = (x as f32 + 8.0, y as f32 + 8.0);
            assert!((g.0 - want.0).abs() <= 1.0 && (g.1 - want.1).abs() <= 1.0, "frame {i}: {g:?} not {want:?}");
        }
    }
}
