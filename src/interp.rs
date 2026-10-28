//! In-between frames from optical flow.
//!
//! For each pair of frames the motion of every pixel is worked out both
//! ways (A to B and B to A), from a small copy of the pictures up to a
//! working size, so large moves are found as well as small ones. Each
//! frame's pixels are then pushed part of the way along their motion to the
//! in-between moment and laid down there ("splatting"). Pixels whose two
//! motions don't agree (they were covered or uncovered) count for much
//! less, which is what keeps something moving in front of a background
//! from leaving a ghost. Where neither frame lands anything, the two are
//! mixed as they are.
//!
//! When a pair can't be trusted (a cut, or motion too wild to follow) no
//! frame is made up: the caller holds the nearer one instead, which looks
//! better than a smeared guess.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;

/// Longest side flow is worked out at; bigger frames are matched smaller
/// and the motion scaled up.
const WORK: u32 = 384;

/// A picture as brightness (alpha counted in), for matching.
#[derive(Clone)]
struct Lum {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Lum {
    fn from(img: &RgbaImage, w: usize, h: usize) -> Lum {
        let small = if (img.width() as usize, img.height() as usize) == (w, h) { None } else { Some(image::imageops::resize(img, w as u32, h as u32, image::imageops::FilterType::Triangle)) };
        let src = small.as_ref().unwrap_or(img);
        let v = src
            .pixels()
            .map(|p| {
                let a = p[3] as f32 / 255.0;
                (p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114) * a
            })
            .collect();
        Lum { w, h, v }
    }

    fn at(&self, x: i32, y: i32) -> f32 {
        self.v[y.clamp(0, self.h as i32 - 1) as usize * self.w + x.clamp(0, self.w as i32 - 1) as usize]
    }

    /// Bilinear look-up.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let a = self.at(x0, y0) * (1.0 - fx) + self.at(x0 + 1, y0) * fx;
        let b = self.at(x0, y0 + 1) * (1.0 - fx) + self.at(x0 + 1, y0 + 1) * fx;
        a * (1.0 - fy) + b * fy
    }

    fn half(&self) -> Lum {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut v = Vec::with_capacity(w * h);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                v.push((self.at(2 * x, 2 * y) + self.at(2 * x + 1, 2 * y) + self.at(2 * x, 2 * y + 1) + self.at(2 * x + 1, 2 * y + 1)) / 4.0);
            }
        }
        Lum { w, h, v }
    }
}

/// Motion per pixel at some size: (dx, dy) in that size's pixels.
#[derive(Clone)]
struct Field {
    w: usize,
    h: usize,
    v: Vec<(f32, f32)>,
}

impl Field {
    fn zero(w: usize, h: usize) -> Field {
        Field { w, h, v: vec![(0.0, 0.0); w * h] }
    }

    fn at(&self, x: i32, y: i32) -> (f32, f32) {
        self.v[y.clamp(0, self.h as i32 - 1) as usize * self.w + x.clamp(0, self.w as i32 - 1) as usize]
    }

    fn sample(&self, x: f32, y: f32) -> (f32, f32) {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let m = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        m(m(self.at(x0, y0), self.at(x0 + 1, y0), fx), m(self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1), fx), fy)
    }

    /// Twice the size (motion doubled with it).
    fn up(&self, w: usize, h: usize) -> Field {
        let sx = self.w as f32 / w as f32;
        let sy = self.h as f32 / h as f32;
        let (kx, ky) = (w as f32 / self.w as f32, h as f32 / self.h as f32);
        let v = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let f = self.sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
                (f.0 * kx, f.1 * ky)
            })
            .collect();
        Field { w, h, v }
    }
}

/// Patch size and step for the patch search, at every level.
const P: i32 = 8;
const S: usize = 4;

/// One level of the patch search: start from `init`, for patches every `S`
/// pixels try the motion there, the motions found next to it, and a little
/// around the best; then spread each patch's motion over its pixels.
fn refine(a: &Lum, b: &Lum, init: &Field, radius: i32) -> Field {
    let (w, h) = (a.w, a.h);
    let gw = w.div_ceil(S);
    let gh = h.div_ceil(S);
    let cost = |px: i32, py: i32, f: (f32, f32), best: f32| -> f32 {
        let mut sum = 0.0;
        for y in 0..P {
            for x in 0..P {
                let (sx, sy) = (px + x, py + y);
                sum += (a.at(sx, sy) - b.sample(sx as f32 + f.0, sy as f32 + f.1)).abs();
            }
            if sum > best {
                return sum;
            }
        }
        sum
    };
    // rows in parallel; along a row each patch also tries its left
    // neighbour's motion (it's probably the same thing)
    let rows: Vec<Vec<((f32, f32), f32)>> = (0..gh)
        .into_par_iter()
        .map(|gy| {
            let mut row: Vec<((f32, f32), f32)> = Vec::with_capacity(gw);
            for gx in 0..gw {
                let (px, py) = ((gx * S) as i32 - P / 2 + S as i32 / 2, (gy * S) as i32 - P / 2 + S as i32 / 2);
                let c = init.at(px + P / 2, py + P / 2);
                let mut cands = vec![c, (0.0, 0.0), init.at(px + P / 2, py - S as i32 + P / 2)];
                if let Some(l) = row.last() {
                    cands.push(l.0);
                }
                let mut best = (c, f32::MAX);
                for f in cands {
                    let e = cost(px, py, f, best.1);
                    if e < best.1 {
                        best = (f, e);
                    }
                }
                // look around it, a pixel at a time
                let base = best.0;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let f = (base.0 + dx as f32, base.1 + dy as f32);
                        let e = cost(px, py, f, best.1);
                        if e < best.1 {
                            best = (f, e);
                        }
                    }
                }
                // still a poor match: something small or fast the levels
                // above didn't see; look much further for it
                if best.1 / (P * P) as f32 > 6.0 {
                    let base = best.0;
                    let far = radius * 5;
                    for dy in (-far..=far).step_by(2) {
                        for dx in (-far..=far).step_by(2) {
                            let f = (base.0 + dx as f32, base.1 + dy as f32);
                            let e = cost(px, py, f, best.1);
                            if e < best.1 {
                                best = (f, e);
                            }
                        }
                    }
                    let base = best.0;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let f = (base.0 + dx as f32, base.1 + dy as f32);
                            let e = cost(px, py, f, best.1);
                            if e < best.1 {
                                best = (f, e);
                            }
                        }
                    }
                }
                // and a half pixel either way
                let base = best.0;
                for (dx, dy) in [(0.5, 0.0), (-0.5, 0.0), (0.0, 0.5), (0.0, -0.5)] {
                    let f = (base.0 + dx, base.1 + dy);
                    let e = cost(px, py, f, best.1);
                    if e < best.1 {
                        best = (f, e);
                    }
                }
                row.push(best);
            }
            row
        })
        .collect();
    // every pixel: the patches over it, the better-matching ones counting more
    let v: Vec<(f32, f32)> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let rows = &rows;
            (0..w).map(move |x| {
                let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
                let gx0 = (x as i32 - P / 2) / S as i32;
                let gy0 = (y as i32 - P / 2) / S as i32;
                for gy in gy0 - 1..=gy0 + 2 {
                    for gx in gx0 - 1..=gx0 + 2 {
                        if gx < 0 || gy < 0 || gx as usize >= gw || gy as usize >= gh {
                            continue;
                        }
                        let (px, py) = (gx * S as i32 - P / 2 + S as i32 / 2, gy * S as i32 - P / 2 + S as i32 / 2);
                        if (x as i32) < px || (y as i32) < py || x as i32 >= px + P || y as i32 >= py + P {
                            continue;
                        }
                        let (f, e) = rows[gy as usize][gx as usize];
                        // how well this patch's motion fits this very pixel
                        let d = (a.at(x as i32, y as i32) - b.sample(x as f32 + f.0, y as f32 + f.1)).abs();
                        let wgt = 1.0 / (1.0 + d + e / (P * P) as f32);
                        sx += f.0 * wgt;
                        sy += f.1 * wgt;
                        sw += wgt;
                    }
                }
                if sw > 0.0 { (sx / sw, sy / sw) } else { init.at(x as i32, y as i32) }
            })
        })
        .collect();
    median(&Field { w, h, v })
}

/// Each motion the middle of the 3×3 around it: one wrong patch doesn't spread.
fn median(f: &Field) -> Field {
    let v = (0..f.h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..f.w).map(move |x| {
                let mut xs = [0.0f32; 9];
                let mut ys = [0.0f32; 9];
                let mut k = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let q = f.at(x as i32 + dx, y as i32 + dy);
                        xs[k] = q.0;
                        ys[k] = q.1;
                        k += 1;
                    }
                }
                xs.sort_by(|a, b| a.total_cmp(b));
                ys.sort_by(|a, b| a.total_cmp(b));
                (xs[4], ys[4])
            })
        })
        .collect();
    Field { w: f.w, h: f.h, v }
}

/// Smooth the motion where it's guesswork, keeping it where it's sure and
/// never across an edge: each pixel's own estimate counts by how well it
/// fits and how much detail there is to fit; the rest comes from the
/// neighbours of the same colour. (Flat areas, like skin or sky in a
/// drawing, have nothing to match on their own and take the motion of
/// their outlines.)
fn settle(a: &Lum, b: &Lum, f: &Field, iters: usize) -> Field {
    let (w, h) = (f.w, f.h);
    // how much each pixel's own estimate is worth
    let own: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let (xi, yi) = (x as i32, y as i32);
                let gx = (a.at(xi + 1, yi) - a.at(xi - 1, yi)).abs();
                let gy = (a.at(xi, yi + 1) - a.at(xi, yi - 1)).abs();
                let detail = ((gx + gy) / 20.0).min(1.0);
                let m = f.at(xi, yi);
                let fit = 1.0 / (1.0 + (a.at(xi, yi) - b.sample(x as f32 + m.0, y as f32 + m.1)).abs() / 8.0);
                detail * fit * 2.0 + 0.02
            })
        })
        .collect();
    let mut cur = f.v.clone();
    for _ in 0..iters {
        let prev = cur.clone();
        cur = (0..h)
            .into_par_iter()
            .flat_map_iter(|y| {
                let (prev, own) = (&prev, &own);
                (0..w).map(move |x| {
                    let i = y * w + x;
                    let c0 = a.at(x as i32, y as i32);
                    let (mut sx, mut sy, mut sw) = (f.v[i].0 * own[i], f.v[i].1 * own[i], own[i]);
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        // neighbours across an edge hardly count
                        let k = (-(a.at(nx, ny) - c0).abs() / 6.0).exp();
                        let q = prev[ny as usize * w + nx as usize];
                        sx += q.0 * k;
                        sy += q.1 * k;
                        sw += k;
                    }
                    (sx / sw, sy / sw)
                })
            })
            .collect();
    }
    Field { w, h, v: cur }
}

/// Motion from `a` to `b`, coarse to fine.
fn flow(a: &Lum, b: &Lum) -> Field {
    let mut pa = vec![a.clone()];
    let mut pb = vec![b.clone()];
    // not so small that small things vanish
    while pa.last().unwrap().w.min(pa.last().unwrap().h) >= 64 && pa.len() < 6 {
        let (na, nb) = (pa.last().unwrap().half(), pb.last().unwrap().half());
        pa.push(na);
        pb.push(nb);
    }
    let top = pa.len() - 1;
    let mut f = Field::zero(pa[top].w, pa[top].h);
    for l in (0..=top).rev() {
        if (f.w, f.h) != (pa[l].w, pa[l].h) {
            f = f.up(pa[l].w, pa[l].h);
        }
        // the smallest level looks furthest (it's cheap there)
        f = refine(&pa[l], &pb[l], &f, if l == top { 6 } else { 2 });
        f = settle(&pa[l], &pb[l], &f, if l == 0 { 60 } else { 16 });
    }
    f
}

/// How far, per pixel, going `ab` then `ba` from where it lands misses
/// coming back: small where the thing is seen in both frames.
fn misses(ab: &Field, ba: &Field) -> Vec<f32> {
    (0..ab.h)
        .flat_map(|y| (0..ab.w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let f = ab.at(x as i32, y as i32);
            let g = ba.sample(x as f32 + f.0, y as f32 + f.1);
            ((f.0 + g.0).powi(2) + (f.1 + g.1).powi(2)).sqrt()
        })
        .collect()
}

/// What can be made between two frames.
pub struct Pair {
    ab: Field,
    ba: Field,
    miss_a: Vec<f32>,
    miss_b: Vec<f32>,
    /// working size to full size
    scale: (f32, f32),
}

impl Pair {
    /// The motion between `a` and `b`, or None when it can't be trusted (a
    /// cut, or too wild to follow).
    pub fn new(a: &RgbaImage, b: &RgbaImage) -> Option<Pair> {
        if a.dimensions() != b.dimensions() {
            return None;
        }
        let (fw, fh) = a.dimensions();
        let k = (WORK as f32 / fw.max(fh) as f32).min(1.0);
        let (w, h) = (((fw as f32 * k).round() as usize).max(8), ((fh as f32 * k).round() as usize).max(8));
        let (la, lb) = (Lum::from(a, w, h), Lum::from(b, w, h));
        let (ab, ba) = rayon::join(|| flow(&la, &lb), || flow(&lb, &la));
        let (miss_a, miss_b) = (misses(&ab, &ba), misses(&ba, &ab));
        // how much of each frame the other can't account for
        let bad = |m: &[f32], f: &Field, l: &Lum, o: &Lum| -> f32 {
            let mut n = 0;
            for (i, &e) in m.iter().enumerate() {
                let (x, y) = ((i % f.w) as f32, (i / f.w) as f32);
                let mv = f.v[i];
                let off = (l.sample(x, y) - o.sample(x + mv.0, y + mv.1)).abs();
                // disagreeing motions only matter where there's detail to
                // go by (in flat colour any motion fits)
                let (xi, yi) = (x as i32, y as i32);
                let detail = (l.at(xi + 1, yi) - l.at(xi - 1, yi)).abs() + (l.at(xi, yi + 1) - l.at(xi, yi - 1)).abs();
                if (detail > 12.0 && e > 1.5 + 0.1 * (mv.0.hypot(mv.1))) || off > 40.0 {
                    n += 1;
                }
            }
            n as f32 / m.len().max(1) as f32
        };
        let worst = bad(&miss_a, &ab, &la, &lb).max(bad(&miss_b, &ba, &lb, &la));
        // how broken up the motion is: one thing moving over a background
        // only jumps along its outline; a muddle jumps all over
        let rough = |f: &Field| -> f32 {
            let mut n = 0;
            for y in 0..f.h as i32 {
                for x in 0..f.w as i32 {
                    let m = f.at(x, y);
                    let lim = 2.0f32.max(0.15 * m.0.hypot(m.1));
                    let r = f.at(x + 1, y);
                    let d = f.at(x, y + 1);
                    if (m.0 - r.0).hypot(m.1 - r.1) > lim || (m.0 - d.0).hypot(m.1 - d.1) > lim {
                        n += 1;
                    }
                }
            }
            n as f32 / (f.w * f.h).max(1) as f32
        };
        let muddle = rough(&ab).max(rough(&ba));
        if std::env::var_os("INTERP_DEBUG").is_some() {
            eprintln!("unaccounted: {:.0}%  broken up: {:.0}%", worst * 100.0, muddle * 100.0);
        }
        if (worst > 0.3 || muddle > 0.08) && std::env::var_os("INTERP_FORCE").is_none() {
            return None;
        }
        Some(Pair { ab, ba, miss_a, miss_b, scale: (fw as f32 / w as f32, fh as f32 / h as f32) })
    }

    /// The frame a share `t` of the way from `a` to `b`.
    ///
    /// The motion is carried to the in-between moment first (each pixel's
    /// motion laid down where it will be then; where two land on one spot
    /// the one moving unlike the rest of the picture is taken to be in
    /// front; gaps filled from around them). Then both frames are looked up
    /// smoothly along it and mixed, each counting for less where it can't
    /// see that spot. A wrong guess stretches a little rather than breaking
    /// the picture into pieces.
    pub fn at(&self, a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
        let (fw, fh) = a.dimensions();
        let (w, h) = (self.ab.w, self.ab.h);
        // the motion at time t, at the working size, from each side: (motion
        // a to b, how sure, front score)
        let project = |f: &Field, miss: &[f32], s: f32, sign: f32| -> Vec<Option<((f32, f32), f32)>> {
            let typical = {
                let mut xs: Vec<f32> = f.v.iter().map(|m| m.0).collect();
                let mut ys: Vec<f32> = f.v.iter().map(|m| m.1).collect();
                xs.sort_by(|a, b| a.total_cmp(b));
                ys.sort_by(|a, b| a.total_cmp(b));
                (xs[xs.len() / 2], ys[ys.len() / 2])
            };
            let mut best: Vec<Option<((f32, f32), f32, f32)>> = vec![None; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let m = f.v[i];
                    let trust = (-(miss[i] * miss[i]) / 2.0).exp();
                    let z = (m.0 - typical.0).hypot(m.1 - typical.1) + trust * 0.5;
                    let (tx, ty) = ((x as f32 + m.0 * s).round(), (y as f32 + m.1 * s).round());
                    // a 2×2 footprint, so a stretch doesn't leave pinholes
                    for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                        let (px, py) = (tx + dx - 0.5, ty + dy - 0.5);
                        let (px, py) = (px.round() as i64, py.round() as i64);
                        if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                            continue;
                        }
                        let j = py as usize * w + px as usize;
                        if best[j].is_none_or(|b| z > b.2) {
                            best[j] = Some(((m.0 * sign, m.1 * sign), trust, z));
                        }
                    }
                }
            }
            best.into_iter().map(|b| b.map(|(m, tr, _)| (m, tr))).collect()
        };
        let (pa, pb) = rayon::join(|| project(&self.ab, &self.miss_a, t, 1.0), || project(&self.ba, &self.miss_b, 1.0 - t, -1.0));
        // one motion per spot: the surer side, or both when they agree
        let mut mt: Vec<Option<(f32, f32)>> = pa
            .iter()
            .zip(&pb)
            .map(|(a, b)| match (a, b) {
                (Some((ma, ta)), Some((mb, tb))) => {
                    let apart = (ma.0 - mb.0).hypot(ma.1 - mb.1);
                    if apart < 2.0 {
                        let k = ta / (ta + tb).max(1e-6);
                        Some((ma.0 * k + mb.0 * (1.0 - k), ma.1 * k + mb.1 * (1.0 - k)))
                    } else if ta >= tb {
                        Some(*ma)
                    } else {
                        Some(*mb)
                    }
                }
                (Some((m, _)), None) | (None, Some((m, _))) => Some(*m),
                (None, None) => None,
            })
            .collect();
        // gaps: from around them
        for _ in 0..32 {
            let holes: Vec<usize> = (0..w * h).filter(|&i| mt[i].is_none()).collect();
            if holes.is_empty() {
                break;
            }
            let fill: Vec<(usize, (f32, f32))> = holes
                .iter()
                .filter_map(|&i| {
                    let (x, y) = ((i % w) as i64, (i / w) as i64);
                    let (mut sx, mut sy, mut k) = (0.0, 0.0, 0.0);
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 {
                            if let Some(q) = mt[ny as usize * w + nx as usize] {
                                sx += q.0;
                                sy += q.1;
                                k += 1.0;
                            }
                        }
                    }
                    (k > 0.0).then(|| (i, (sx / k, sy / k)))
                })
                .collect();
            for (i, v) in fill {
                mt[i] = Some(v);
            }
        }
        let field = Field { w, h, v: mt.into_iter().map(|m| m.unwrap_or((0.0, 0.0))).collect() };
        // how sure each frame is of a spot it's sampled at (seen in both)
        let sure = |miss: &[f32], x: f32, y: f32| -> f32 {
            let (xi, yi) = (x.round().clamp(0.0, (w - 1) as f32) as usize, y.round().clamp(0.0, (h - 1) as f32) as usize);
            let e = miss[yi * w + xi];
            (-(e * e) / 2.0).exp().max(0.05)
        };
        let samp = |img: &RgbaImage, x: f32, y: f32| -> [f32; 4] {
            let x = x.clamp(0.0, (fw - 1) as f32);
            let y = y.clamp(0.0, (fh - 1) as f32);
            let (x0, y0) = (x.floor() as u32, y.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(fw - 1), (y0 + 1).min(fh - 1));
            let (ax, ay) = (x - x0 as f32, y - y0 as f32);
            let p = |x: u32, y: u32| img.get_pixel(x, y).0;
            let mut o = [0.0f32; 4];
            for c in 0..4 {
                let top = p(x0, y0)[c] as f32 * (1.0 - ax) + p(x1, y0)[c] as f32 * ax;
                let bot = p(x0, y1)[c] as f32 * (1.0 - ax) + p(x1, y1)[c] as f32 * ax;
                o[c] = top * (1.0 - ay) + bot * ay;
            }
            o
        };
        let (sx, sy) = self.scale;
        // per pixel: the made colour, and how much the two frames disagree there
        let rows: Vec<Vec<([f32; 4], f32)>> = (0..fh)
            .into_par_iter()
            .map(|y| {
                (0..fw)
                    .map(|x| {
                        let (wx, wy) = ((x as f32 + 0.5) / sx - 0.5, (y as f32 + 0.5) / sy - 0.5);
                        let m = field.sample(wx, wy);
                        // where it was in a, and where it will be in b
                        let (ax, ay) = (wx - m.0 * t, wy - m.1 * t);
                        let (bx, by) = (wx + m.0 * (1.0 - t), wy + m.1 * (1.0 - t));
                        let ca = samp(a, (ax + 0.5) * sx - 0.5, (ay + 0.5) * sy - 0.5);
                        let cb = samp(b, (bx + 0.5) * sx - 0.5, (by + 0.5) * sy - 0.5);
                        let wa = (1.0 - t) * sure(&self.miss_a, ax, ay);
                        let wb = t * sure(&self.miss_b, bx, by);
                        let o = [0, 1, 2, 3].map(|c| (ca[c] * wa + cb[c] * wb) / (wa + wb));
                        // both sure of this spot yet showing different things:
                        // the motion is wrong here
                        let differ = (0..3).map(|c| (ca[c] - cb[c]).abs()).sum::<f32>() / 3.0;
                        let both = sure(&self.miss_a, ax, ay).min(sure(&self.miss_b, bx, by));
                        (o, ((differ - 35.0) / 45.0).clamp(0.0, 1.0) * both)
                    })
                    .collect()
            })
            .collect();
        // spread the doubt a little so the fall-back has soft edges
        let (iw, ih) = (fw as usize, fh as usize);
        let raw: Vec<f32> = rows.iter().flat_map(|r| r.iter().map(|p| p.1)).collect();
        let rad = ((fw.max(fh) as f32 / 150.0).ceil() as i64).max(2);
        let blur = |v: &[f32], horiz: bool| -> Vec<f32> {
            (0..iw * ih)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % iw) as i64, (i / iw) as i64);
                    let mut s = 0.0;
                    let mut k = 0.0;
                    for d in -rad..=rad {
                        let (nx, ny) = if horiz { (x + d, y) } else { (x, y + d) };
                        if nx >= 0 && ny >= 0 && nx < iw as i64 && ny < ih as i64 {
                            s += v[ny as usize * iw + nx as usize];
                            k += 1.0;
                        }
                    }
                    s / k
                })
                .collect()
        };
        let doubt = blur(&blur(&raw, true), false);
        // where in doubt: the nearer frame as it is
        let near = if t < 0.5 { a } else { b };
        let mut out = RgbaImage::new(fw, fh);
        for (y, row) in rows.into_iter().enumerate() {
            for (x, (o, _)) in row.into_iter().enumerate() {
                let k = (doubt[y * iw + x] * 2.0).min(1.0);
                let n = near.get_pixel(x as u32, y as u32).0;
                let v = [0, 1, 2, 3].map(|c| o[c] * (1.0 - k) + n[c] as f32 * k);
                out.put_pixel(x as u32, y as u32, Rgba(v.map(|v| v.round().clamp(0.0, 255.0) as u8)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured square at `x0` over a textured background.
    fn scene(x0: i32, y0: i32) -> RgbaImage {
        RgbaImage::from_fn(160, 120, |x, y| {
            let (x, y) = (x as i32, y as i32);
            let inside = x >= x0 && x < x0 + 24 && y >= y0 && y < y0 + 24;
            let hsh = |a: i32, b: i32, s: u32| ((a as u32).wrapping_mul(73856093) ^ (b as u32).wrapping_mul(19349663) ^ s).wrapping_mul(2654435761) >> 26;
            if inside {
                let v = 120 + (hsh((x - x0) / 3, (y - y0) / 3, 1) as i32) * 2;
                Rgba([255, v as u8, 40, 255])
            } else {
                let v = 40 + hsh(x / 4, y / 4, 7) as i32;
                Rgba([v as u8, v as u8, (v + 30) as u8, 255])
            }
        })
    }

    #[test]
    fn large_motion_lands_half_way_without_a_ghost() {
        // 40 px apart: far past the old block search
        let (a, b) = (scene(20, 40), scene(60, 48));
        let p = Pair::new(&a, &b).expect("trusted");
        let mid = p.at(&a, &b, 0.5);
        let want = scene(40, 44);
        // inside the square where it should be: square colours
        let red = |img: &RgbaImage, x: u32, y: u32| img.get_pixel(x, y)[0] > 200 && img.get_pixel(x, y)[2] < 90;
        let mut hit = 0;
        let mut total = 0;
        for y in 48..64 {
            for x in 44..60 {
                total += 1;
                if red(&mid, x, y) {
                    hit += 1;
                }
            }
        }
        assert!(hit * 10 >= total * 9, "square at the half way spot: {hit}/{total}");
        // where it started and where it ends: background, not a faded copy
        for (x, y) in [(26, 46), (76, 62)] {
            assert!(!red(&mid, x, y) && mid.get_pixel(x, y)[0] < 140, "ghost at {x},{y}: {:?}", mid.get_pixel(x, y));
        }
        // and mostly like the real thing overall
        let err: f64 = mid.pixels().zip(want.pixels()).map(|(p, q)| (0..3).map(|c| (p[c] as f64 - q[c] as f64).abs()).sum::<f64>()).sum::<f64>() / (160.0 * 120.0 * 3.0);
        assert!(err < 12.0, "mean error {err:.1}");
    }

    #[test]
    fn a_cut_is_not_blended() {
        let a = scene(20, 40);
        let b = RgbaImage::from_fn(160, 120, |x, y| Rgba([(x * 7 % 255) as u8, (y * 13 % 255) as u8, 200, 255]));
        assert!(Pair::new(&a, &b).is_none());
    }

    /// FILE=clip PAIRS="0,1;13,14" OUT=sheet.png: each pair as A, a plain
    /// blend, the in-between, B.
    #[test]
    #[ignore]
    fn compare_file() {
        let a = crate::io::load_animation(std::path::Path::new(&std::env::var("FILE").unwrap())).unwrap();
        let pairs: Vec<(usize, usize)> = std::env::var("PAIRS").unwrap().split(';').map(|p| {
            let v: Vec<usize> = p.split(',').map(|x| x.trim().parse().unwrap()).collect();
            (v[0], v[1])
        }).collect();
        let (w, h) = (a.width(), a.height());
        let mut sheet = RgbaImage::new(w * 4, h * pairs.len() as u32);
        for (r, &(i, j)) in pairs.iter().enumerate() {
            let (fa, fb) = (a.frames[i].image.get(), a.frames[j].image.get());
            let t0 = std::time::Instant::now();
            let pair = Pair::new(&fa, &fb);
            let mid = match &pair {
                Some(p) => p.at(&fa, &fb, 0.5),
                None => (*fa).clone(),
            };
            eprintln!("{i}-{j}: {} in {:.0} ms", if pair.is_some() { "made" } else { "held (can't trust)" }, t0.elapsed().as_secs_f32() * 1000.0);
            let blend = crate::ops::lerp_image(&fa, &fb, 0.5);
            for (c, img) in [&*fa, &blend, &mid, &*fb].into_iter().enumerate() {
                image::imageops::overlay(&mut sheet, img, (c as u32 * w) as i64, (r as u32 * h) as i64);
            }
        }
        let small = image::imageops::resize(&sheet, sheet.width() / 2, sheet.height() / 2, image::imageops::FilterType::Triangle);
        small.save(std::env::var("OUT").unwrap()).unwrap();
    }

    /// A flat-coloured shape (as in a drawing): nothing to match inside it,
    /// only its outline. It still moves as one piece.
    #[test]
    fn flat_shapes_move_whole() {
        let blob = |cx: f32, cy: f32| {
            RgbaImage::from_fn(200, 140, |x, y| {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let d = (dx * dx / 900.0 + dy * dy / 400.0).sqrt();
                if d < 0.94 {
                    Rgba([250, 200, 60, 255])
                } else if d < 1.0 {
                    Rgba([30, 20, 10, 255])
                } else {
                    Rgba([120, 190, 240, 255])
                }
            })
        };
        let (a, b) = (blob(50.0, 60.0), blob(110.0, 80.0));
        let p = Pair::new(&a, &b).expect("trusted");
        let mid = p.at(&a, &b, 0.5);
        let want = blob(80.0, 70.0);
        let wrong = mid.pixels().zip(want.pixels()).filter(|(p, q)| (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 60)).count();
        assert!(wrong < 200 * 140 / 50, "{wrong} pixels wrong");
    }
}
