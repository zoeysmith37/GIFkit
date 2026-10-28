//! In-between frames that follow the motion.
//!
//! The tracker first looks for a grid of patches of one frame in the other,
//! over a wide area and allowing for turning and growing. Patches that
//! agree are sorted into whole things moving as one: the camera, then a
//! character, each a turn, growth and shift. The fine motion of every pixel
//! is worked out too, coarse to fine, each patch also trying what the
//! tracker found around it.
//!
//! Every pixel then picks the thing it belongs to (judged over a window
//! around it, so things move as pieces rather than as scattered bits). The
//! thing's motion is taken out, what's left (small now) is found by the
//! fine search and smoothed within the thing, and the two together are
//! that pixel's motion. Part way along, a turning thing has turned part
//! way, so spins sweep round rather than shrink through the middle.
//!
//! Where it still can't be followed (the two frames disagree, or the
//! motion tears) the frames are faded instead, each moved as the camera
//! moved: soft there, but never in pieces, and the background still glides.
//! Only a cut (hardly any patch found anywhere) gets no in-between.

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

/// The tracker's rough answer: a grid of patches of one frame and where
/// each went in the other (None where it couldn't be found), at the working
/// size.
struct Hints {
    gw: usize,
    gh: usize,
    step: f32,
    cells: Vec<Option<crate::track::Moved>>,
}

impl Hints {
    /// The patches around (x, y) (working pixels).
    fn around(&self, x: f32, y: f32) -> impl Iterator<Item = &crate::track::Moved> {
        let (cx, cy) = ((x / self.step - 0.5).round() as i64, (y / self.step - 0.5).round() as i64);
        (-1..=1i64).flat_map(move |dy| (-1..=1i64).map(move |dx| (cx + dx, cy + dy))).filter_map(move |(gx, gy)| {
            (gx >= 0 && gy >= 0 && (gx as usize) < self.gw && (gy as usize) < self.gh).then(|| self.cells[gy as usize * self.gw + gx as usize].as_ref()).flatten()
        })
    }

}

/// Patch size and step for the patch search, at every level.
const P: i32 = 8;
const S: usize = 4;

/// One level of the patch search: start from `init`, for patches every `S`
/// pixels try the motion there, the motions found next to it, and a little
/// around the best; then spread each patch's motion over its pixels.
fn refine(a: &Lum, b: &Lum, init: &Field, radius: i32, hints: &Hints, k: f32) -> Field {
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
                // what the tracker found for the patches around here
                let (mx, my) = ((px + P / 2) as f32 / k, (py + P / 2) as f32 / k);
                for c in hints.around(mx, my) {
                    let m = c.motion_at(mx, my);
                    cands.push((m.0 * k, m.1 * k));
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
fn flow(a: &Lum, b: &Lum, hints: &Hints) -> Field {
    flow_levels(a, b, hints, 6, 60)
}

/// The same for motion known to be small: fewer levels, less smoothing.
fn flow_small(a: &Lum, b: &Lum) -> Field {
    flow_levels(a, b, &Hints { gw: 0, gh: 0, step: 1.0, cells: Vec::new() }, 3, 24)
}

fn flow_levels(a: &Lum, b: &Lum, hints: &Hints, most: usize, settle_full: usize) -> Field {
    let mut pa = vec![a.clone()];
    let mut pb = vec![b.clone()];
    // not so small that small things vanish
    while pa.last().unwrap().w.min(pa.last().unwrap().h) >= 64 && pa.len() < most {
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
        let k = pa[l].w as f32 / a.w as f32;
        f = refine(&pa[l], &pb[l], &f, if l == top { 6 } else { 2 }, hints, k);
        f = settle(&pa[l], &pb[l], &f, if l == 0 { settle_full } else { 16 });
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

/// The tracker's patches worth going by: with detail, a fair match, and
/// agreeing with the patch found going back the other way. Returns hints
/// each way, how many patches had detail, and how many of those were found.
fn keep_found(fa: (usize, usize, Vec<crate::track::Moved>), fb: (usize, usize, Vec<crate::track::Moved>), step: f32) -> (Hints, Hints, usize, usize) {
    let check = |from: &(usize, usize, Vec<crate::track::Moved>), back: &(usize, usize, Vec<crate::track::Moved>)| -> Vec<Option<crate::track::Moved>> {
        from.2
            .iter()
            .map(|c| {
                if c.detail < 3.0 || c.cost > (c.detail * 0.8).max(10.0) {
                    return None;
                }
                // going back from where it landed should come home
                let (bx, by) = ((c.to_x / step - 0.5).round() as i64, (c.to_y / step - 0.5).round() as i64);
                let mut home = false;
                for gy in by - 1..=by + 1 {
                    for gx in bx - 1..=bx + 1 {
                        if gx < 0 || gy < 0 || gx as usize >= back.0 || gy as usize >= back.1 {
                            continue;
                        }
                        let o = &back.2[gy as usize * back.0 + gx as usize];
                        if (o.x - c.to_x).abs() > step || (o.y - c.to_y).abs() > step {
                            continue;
                        }
                        let m = o.motion_at(c.to_x, c.to_y);
                        if (c.to_x + m.0 - c.x).hypot(c.to_y + m.1 - c.y) < step * 0.5 {
                            home = true;
                        }
                    }
                }
                home.then_some(*c)
            })
            .collect()
    };
    let (ca, cb) = (check(&fa, &fb), check(&fb, &fa));
    let detailed = fa.2.iter().filter(|c| c.detail >= 3.0).count();
    let found = ca.iter().flatten().count();
    (Hints { gw: fa.0, gh: fa.1, step, cells: ca }, Hints { gw: fb.0, gh: fb.1, step, cells: cb }, detailed, found)
}

/// A picture as colour (alpha counted in), for choosing motions.
struct Rgb {
    w: usize,
    h: usize,
    v: Vec<[f32; 3]>,
}

impl Rgb {
    fn from(img: &RgbaImage) -> Rgb {
        let v = img.pixels().map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a]
        }).collect();
        Rgb { w: img.width() as usize, h: img.height() as usize, v }
    }

    fn at(&self, x: i32, y: i32) -> [f32; 3] {
        self.v[y.clamp(0, self.h as i32 - 1) as usize * self.w + x.clamp(0, self.w as i32 - 1) as usize]
    }

    /// Less the average around each pixel: the detail alone, so a flash or
    /// a change of light doesn't stop things matching.
    fn detail(&self) -> Rgb {
        let ch: Vec<Vec<f32>> = (0..3).map(|c| box_blur(&self.v.iter().map(|p| p[c]).collect::<Vec<_>>(), self.w, self.h, 5)).collect();
        Rgb { w: self.w, h: self.h, v: self.v.iter().enumerate().map(|(i, p)| [0, 1, 2].map(|c| p[c] - ch[c][i])).collect() }
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (p, q, r, s) = (self.at(x0, y0), self.at(x0 + 1, y0), self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1));
        [0, 1, 2].map(|c| (p[c] * (1.0 - fx) + q[c] * fx) * (1.0 - fy) + (r[c] * (1.0 - fx) + s[c] * fx) * fy)
    }
}

/// A box average `r` out each way (rows then columns).
fn box_blur(v: &[f32], w: usize, h: usize, r: i32) -> Vec<f32> {
    let pass = |v: &[f32], horiz: bool| -> Vec<f32> {
        (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as i32, (i / w) as i32);
                let (mut s, mut k) = (0.0f32, 0.0f32);
                for d in -r..=r {
                    let (nx, ny) = if horiz { (x + d, y) } else { (x, y + d) };
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        s += v[ny as usize * w + nx as usize];
                        k += 1.0;
                    }
                }
                s / k
            })
            .collect()
    };
    pass(&pass(v, true), false)
}

/// How a piece moves from one frame to the other: its middle (cx, cy) goes
/// by (tx, ty) while it turns by `ang` and grows by `sc` about it. Part
/// way there it has turned part way, so spinning things sweep round
/// instead of shrinking through the middle.
#[derive(Clone, Copy, Debug)]
struct Motion {
    cx: f32,
    cy: f32,
    tx: f32,
    ty: f32,
    ang: f32,
    sc: f32,
    /// what's left after that: this pixel's own small move on top
    rx: f32,
    ry: f32,
}

impl Motion {
    /// Just a shift, for the point (x, y).
    fn shift(x: f32, y: f32, m: (f32, f32)) -> Motion {
        Motion { cx: x, cy: y, tx: 0.0, ty: 0.0, ang: 0.0, sc: 1.0, rx: m.0, ry: m.1 }
    }

    /// Going back the other way (without a pixel's own move).
    fn inverse(&self) -> Motion {
        Motion { cx: self.cx + self.tx, cy: self.cy + self.ty, tx: -self.tx, ty: -self.ty, ang: -self.ang, sc: 1.0 / self.sc, rx: 0.0, ry: 0.0 }
    }

    /// The same with a pixel's own move on top.
    fn plus(&self, r: (f32, f32)) -> Motion {
        Motion { rx: r.0, ry: r.1, ..*self }
    }

    /// Where (x, y) is a share `t` of the way along.
    fn at(&self, x: f32, y: f32, t: f32) -> (f32, f32) {
        let (x, y) = (x + self.rx * t, y + self.ry * t);
        let (sn, cs) = (self.ang * t).sin_cos();
        let k = self.sc.powf(t);
        let (qx, qy) = ((x - self.cx) * k, (y - self.cy) * k);
        (self.cx + self.tx * t + qx * cs - qy * sn, self.cy + self.ty * t + qx * sn + qy * cs)
    }

    /// Where the point that is at (x, y) a share `t` of the way along started.
    fn back(&self, x: f32, y: f32, t: f32) -> (f32, f32) {
        let (sn, cs) = (-self.ang * t).sin_cos();
        let k = self.sc.powf(-t);
        let (qx, qy) = ((x - self.cx - self.tx * t) * k, (y - self.cy - self.ty * t) * k);
        (self.cx + qx * cs - qy * sn - self.rx * t, self.cy + qx * sn + qy * cs - self.ry * t)
    }

    /// How far (x, y) goes all the way.
    fn vec(&self, x: f32, y: f32) -> (f32, f32) {
        let p = self.at(x, y, 1.0);
        (p.0 - x, p.1 - y)
    }
}

/// The turn, growth and shift that best carries the patches' middles to
/// where they went, or None when it can't be told (too few, too close, or
/// growing beyond belief).
fn fit(cs: &[&crate::track::Moved]) -> Option<Motion> {
    let n = cs.len() as f32;
    if cs.len() < 2 {
        return None;
    }
    let (px, py) = (cs.iter().map(|c| c.x).sum::<f32>() / n, cs.iter().map(|c| c.y).sum::<f32>() / n);
    let (qx, qy) = (cs.iter().map(|c| c.to_x).sum::<f32>() / n, cs.iter().map(|c| c.to_y).sum::<f32>() / n);
    let (mut a, mut b, mut dd) = (0.0f32, 0.0f32, 0.0f32);
    for c in cs {
        let (dx, dy, ex, ey) = (c.x - px, c.y - py, c.to_x - qx, c.to_y - qy);
        a += dx * ex + dy * ey;
        b += dx * ey - dy * ex;
        dd += dx * dx + dy * dy;
    }
    if dd < 1.0 {
        return None;
    }
    let sc = a.hypot(b) / dd;
    (0.6..1.7).contains(&sc).then(|| Motion { cx: px, cy: py, tx: qx - px, ty: qy - py, ang: b.atan2(a), sc, rx: 0.0, ry: 0.0 })
}

/// Whole things moving as one (the camera, a character), from the patches
/// found: the turn, growth and shift most of them agree with, then the
/// same among the ones left over. Up to `most`.
fn groups(cells: &[&crate::track::Moved], step: f32, most: usize) -> Vec<Motion> {
    let fits = |m: &Motion, c: &crate::track::Moved| {
        let p = m.at(c.x, c.y, 1.0);
        (p.0 - c.to_x).hypot(p.1 - c.to_y) < step * 0.35
    };
    let mut left: Vec<&crate::track::Moved> = cells.to_vec();
    let mut out = Vec::new();
    while out.len() < most && left.len() >= 3 {
        let tries: Vec<(usize, Motion)> = (0..left.len())
            .into_par_iter()
            .flat_map_iter(|i| {
                let left = &left;
                (i + 1..left.len()).filter_map(move |j| fit(&[left[i], left[j]]).map(|m| (left.iter().filter(|c| fits(&m, c)).count(), m)))
            })
            .collect();
        let Some(&(n, m)) = tries.iter().max_by_key(|t| t.0) else { break };
        if n < 3 {
            break;
        }
        let ins: Vec<&crate::track::Moved> = left.iter().copied().filter(|c| fits(&m, c)).collect();
        let m = fit(&ins).unwrap_or(m);
        left.retain(|c| !fits(&m, c));
        out.push(m);
    }
    out
}

/// Each pixel's motion from those around it: the one that sits in the
/// middle of the rest (as `ends` places it), so a lone speck of another
/// motion goes but an edge between two stays sharp.
fn vmedian<T: Copy + Send + Sync>(labs: &[T], w: usize, h: usize, ends: impl Fn(&T, f32, f32) -> [f32; 4] + Sync) -> Vec<T> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            let mut near: Vec<[f32; 4]> = Vec::with_capacity(9);
            let mut who: Vec<usize> = Vec::with_capacity(9);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        let j = ny as usize * w + nx as usize;
                        near.push(ends(&labs[j], x as f32, y as f32));
                        who.push(j);
                    }
                }
            }
            let mut best = (f32::MAX, i);
            for (k, e) in near.iter().enumerate() {
                let d: f32 = near.iter().map(|o| (e[0] - o[0]).abs() + (e[1] - o[1]).abs() + (e[2] - o[2]).abs() + (e[3] - o[3]).abs()).sum();
                if d < best.0 {
                    best = (d, who[k]);
                }
            }
            labs[best.1]
        })
        .collect()
}

/// Blur `v` over just the pixels where `inside`, so nothing from outside
/// leaks in (where nothing is inside nearby, it stays as it was).
fn blur_within(v: &[(f32, f32)], inside: &[bool], w: usize, h: usize, r: i32) -> Vec<(f32, f32)> {
    let m: Vec<f32> = inside.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let xs: Vec<f32> = v.iter().zip(&m).map(|(p, k)| p.0 * k).collect();
    let ys: Vec<f32> = v.iter().zip(&m).map(|(p, k)| p.1 * k).collect();
    let (den, nx, ny) = (box_blur(&m, w, h, r), box_blur(&xs, w, h, r), box_blur(&ys, w, h, r));
    (0..w * h).map(|i| if den[i] > 1e-3 { (nx[i] / den[i], ny[i] / den[i]) } else { v[i] }).collect()
}

/// The commonest of the 5×5 around each pixel (of `n` kinds): ragged edges
/// and lone specks go.
fn majority(k: &[u8], w: usize, h: usize, n: usize) -> Vec<u8> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            let mut count = [0u16; 8];
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        count[k[ny as usize * w + nx as usize] as usize] += 1;
                    }
                }
            }
            let mut best = k[i];
            for c in 0..n {
                if count[c] > count[best as usize] {
                    best = c as u8;
                }
            }
            best
        })
        .collect()
}

/// Each pixel's motion, in layers. The tracker's patches are sorted into
/// whole things moving as one (the camera, a character): each a turn,
/// growth and shift. Each thing's motion is taken out of `b` and what's
/// left (small now) found by the fine search. Every pixel then picks the
/// thing it belongs to, by which fits best around it; and within each
/// thing what's left is smoothed, so it moves as one piece that bends
/// rather than as scattered bits. (With nothing found, the fine motion
/// alone.)
fn pick(a: &Rgb, b: &Rgb, la: &Lum, lb: &Lum, things: &[Motion], fine: &Field) -> (Field, Vec<Motion>) {
    let (w, h) = (a.w, a.h);
    // the window a fit is judged over, and how far what's left is smoothed
    let (agg_r, rs) = (8, 8);
    let diff_at = |x: usize, y: usize, m: (f32, f32)| -> f32 {
        let (tx, ty) = (x as f32 + m.0, y as f32 + m.1);
        let p = a.v[y * w + x];
        let q = b.sample(tx, ty);
        let off = (-tx).max(tx - (w - 1) as f32).max(-ty).max(ty - (h - 1) as f32).max(0.0);
        (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>() / 3.0 + if off > 2.0 { 25.0 } else { 0.0 }
    };
    // the layers: the fine motion on its own (a handicap: it's the
    // fall-back), then each thing with what's left over
    let mut layers: Vec<(Motion, Field, f32)> = vec![(Motion::shift(0.0, 0.0, (0.0, 0.0)), fine.clone(), 4.0)];
    for &g in things {
        // hardly moving: what's left is the fine motion already found
        let corner = g.at(0.0, 0.0, 1.0);
        let far = g.at((w - 1) as f32, (h - 1) as f32, 1.0);
        if corner.0.hypot(corner.1) < 0.5 && (far.0 - (w - 1) as f32).hypot(far.1 - (h - 1) as f32) < 0.5 {
            layers.push((g, fine.clone(), 0.0));
            continue;
        }
        let warped = Lum {
            w,
            h,
            v: (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let p = g.at((i % w) as f32, (i / w) as f32, 1.0);
                    lb.sample(p.0, p.1)
                })
                .collect(),
        };
        layers.push((g, flow_small(la, &warped), 0.0));
    }
    // which layer each pixel is in
    let costs: Vec<Vec<f32>> = layers
        .iter()
        .map(|(g, rest, pen)| {
            // a thing is judged by its own motion (what's left over could
            // make anything fit anywhere); a pixel or two of give
            let d: Vec<f32> = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    if *pen > 0.0 {
                        return diff_at(x, y, rest.v[i]);
                    }
                    let r = rest.v[i];
                    let k = (2.0 / r.0.hypot(r.1).max(1e-3)).min(1.0);
                    diff_at(x, y, g.plus((r.0 * k, r.1 * k)).vec(x as f32, y as f32))
                })
                .collect();
            box_blur(&d, w, h, agg_r).into_iter().map(|c| c + pen).collect()
        })
        .collect();
    let mut kind: Vec<u8> = (0..w * h).map(|i| (0..layers.len()).min_by(|&p, &q| costs[p][i].total_cmp(&costs[q][i])).unwrap_or(0) as u8).collect();
    for _ in 0..2 {
        kind = majority(&kind, w, h, layers.len());
    }
    // what's left, smoothed within each layer
    let mut labs: Vec<Motion> = (0..w * h).map(|i| Motion::shift((i % w) as f32, (i / w) as f32, fine.v[i])).collect();
    for (k, (g, rest, _)) in layers.iter().enumerate() {
        let inside: Vec<bool> = kind.iter().map(|&c| c as usize == k).collect();
        let smooth = if k == 0 { rest.v.clone() } else { blur_within(&rest.v, &inside, w, h, rs) };
        for i in 0..w * h {
            if inside[i] {
                labs[i] = if k == 0 { Motion::shift((i % w) as f32, (i / w) as f32, smooth[i]) } else { g.plus(smooth[i]) };
            }
        }
    }
    let v = (0..w * h).map(|i| labs[i].vec((i % w) as f32, (i / w) as f32)).collect();
    (Field { w, h, v }, labs)
}

/// What can be made between two frames.
pub struct Pair {
    ab: Field,
    ba: Field,
    /// each pixel's motion as a piece (turning as well as moving)
    lab_ab: Vec<Motion>,
    lab_ba: Vec<Motion>,
    miss_a: Vec<f32>,
    miss_b: Vec<f32>,
    /// how the camera moved (the biggest thing moving as one), for where
    /// the rest can't be followed
    camera: Option<Motion>,
    /// working size to full size
    scale: (f32, f32),
}

impl Pair {
    /// The motion between `a` and `b`, or None at a cut (nothing in one
    /// is in the other).
    pub fn new(a: &RgbaImage, b: &RgbaImage) -> Option<Pair> {
        if a.dimensions() != b.dimensions() {
            return None;
        }
        let (fw, fh) = a.dimensions();
        let k = (WORK as f32 / fw.max(fh) as f32).min(1.0);
        let (w, h) = (((fw as f32 * k).round() as usize).max(8), ((fh as f32 * k).round() as usize).max(8));
        let small = |img: &RgbaImage| if (fw as usize, fh as usize) == (w, h) { img.clone() } else { image::imageops::resize(img, w as u32, h as u32, image::imageops::FilterType::Triangle) };
        let (sa, sb) = (small(a), small(b));
        let (la, lb) = (Lum::from(&sa, w, h), Lum::from(&sb, w, h));
        // the tracker's rough answer, both ways
        let clock = std::time::Instant::now();
        let lap = |what: &str| {
            if std::env::var_os("INTERP_DEBUG").is_some() {
                eprintln!("  {what}: {} ms", clock.elapsed().as_millis());
            }
        };
        let (ga, gb) = (crate::track::Gray::new(&sa), crate::track::Gray::new(&sb));
        let step = (w.max(h) as i32 / 10).clamp(10, 48);
        let r = (step * 7 / 10).max(8);
        let (fa, fb) = rayon::join(|| crate::track::match_grid(&ga, &gb, step, r), || crate::track::match_grid(&gb, &ga, step, r));
        lap("patches");
        let (ha, hb, detailed, found) = keep_found(fa, fb, step as f32);
        if std::env::var_os("INTERP_DEBUG").is_some() {
            eprintln!("patches found: {found} of {detailed} with detail");
        }
        // a cut: hardly anything with detail is in the other frame
        if detailed >= 4 && (found as f32) < detailed as f32 * 0.2 && std::env::var_os("INTERP_FORCE").is_none() {
            return None;
        }
        let (ab, ba) = rayon::join(|| flow(&la, &lb, &ha), || flow(&lb, &la, &hb));
        lap("fine motion");
        let (ca, cb) = (Rgb::from(&sa).detail(), Rgb::from(&sb).detail());
        let camera;
        let ((ab, lab_ab), (ba, lab_ba)) = {
            // whole things moving as one, the same both ways
            let mut cells: Vec<&crate::track::Moved> = ha.cells.iter().flatten().collect();
            // (and the patches found going back, turned round)
            let back: Vec<crate::track::Moved> = hb.cells.iter().flatten().map(|c| crate::track::Moved { x: c.to_x, y: c.to_y, to_x: c.x, to_y: c.y, ang: -c.ang, sc: 1.0 / c.sc, ..*c }).collect();
            cells.extend(back.iter());
            let things = groups(&cells, step as f32, 3);
            if std::env::var_os("INTERP_DEBUG").is_some() {
                for g in &things {
                    eprintln!("thing at ({:.0},{:.0}) moves ({:.1},{:.1}) turns {:.2} grows {:.2}", g.cx, g.cy, g.tx, g.ty, g.ang, g.sc);
                }
            }
            camera = things.first().copied();
            let undo: Vec<Motion> = things.iter().map(|g| g.inverse()).collect();
            rayon::join(|| pick(&ca, &cb, &la, &lb, &things, &ab), || pick(&cb, &ca, &lb, &la, &undo, &ba))
        };
        lap("layers");
        let (miss_a, miss_b) = (misses(&ab, &ba), misses(&ba, &ab));
        // per pixel these are noisy; in patches they say what's covered
        let (miss_a, miss_b) = (box_blur(&miss_a, w, h, 2), box_blur(&miss_b, w, h, 2));
        Some(Pair { ab, ba, lab_ab, lab_ba, miss_a, miss_b, camera, scale: (fw as f32 / w as f32, fh as f32 / h as f32) })
    }

    /// The frame a share `t` of the way from `a` to `b`.
    ///
    /// Each pixel's motion is carried to the in-between moment first (where
    /// two land on one spot, the one moving unlike the rest of the picture
    /// is taken to be in front; gaps take a neighbour's). Then both frames
    /// are looked up along it and mixed, each counting only where it sees
    /// that spot (its own motion there is this one). Where that fails, the
    /// camera-following fade takes over, softly.
    pub fn at(&self, a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
        let (fw, fh) = a.dimensions();
        let (w, h) = (self.ab.w, self.ab.h);
        // which piece's motion each spot at time t follows, at the working
        // size, from each side: (motion, from b's side, how sure)
        let project = |labs: &[Motion], f: &Field, miss: &[f32], s: f32, from_b: bool| -> Vec<Option<(Motion, bool, f32)>> {
            let typical = {
                let mut xs: Vec<f32> = f.v.iter().map(|m| m.0).collect();
                let mut ys: Vec<f32> = f.v.iter().map(|m| m.1).collect();
                xs.sort_by(|a, b| a.total_cmp(b));
                ys.sort_by(|a, b| a.total_cmp(b));
                (xs[xs.len() / 2], ys[ys.len() / 2])
            };
            let mut best: Vec<Option<(Motion, bool, f32, f32)>> = vec![None; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let m = f.v[i];
                    let trust = (-(miss[i] * miss[i]) / 2.0).exp();
                    // what moves unlike the rest is taken to be in front
                    let z = (m.0 - typical.0).hypot(m.1 - typical.1) + trust * 0.5;
                    let (tx, ty) = labs[i].at(x as f32, y as f32, s);
                    // a 2×2 footprint, so a stretch doesn't leave pinholes
                    for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                        let (px, py) = ((tx + dx - 0.5).round() as i64, (ty + dy - 0.5).round() as i64);
                        if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                            continue;
                        }
                        let j = py as usize * w + px as usize;
                        if best[j].is_none_or(|b| z > b.3) {
                            best[j] = Some((labs[i], from_b, trust, z));
                        }
                    }
                }
            }
            best.into_iter().map(|b| b.map(|(m, fb, tr, _)| (m, fb, tr))).collect()
        };
        let (pa, pb) = rayon::join(|| project(&self.lab_ab, &self.ab, &self.miss_a, t, false), || project(&self.lab_ba, &self.ba, &self.miss_b, 1.0 - t, true));
        // where a spot at time t was in a and will be in b, by a piece's motion
        let ends = |l: &(Motion, bool), x: f32, y: f32| -> [f32; 4] {
            if l.1 {
                let p = l.0.back(x, y, 1.0 - t);
                let q = l.0.at(p.0, p.1, 1.0);
                [q.0, q.1, p.0, p.1]
            } else {
                let p = l.0.back(x, y, t);
                let q = l.0.at(p.0, p.1, 1.0);
                [p.0, p.1, q.0, q.1]
            }
        };
        // one motion per spot: the surer side's
        let mut mt: Vec<Option<(Motion, bool)>> = pa
            .iter()
            .zip(&pb)
            .map(|(a, b)| match (a, b) {
                (Some((ma, fa, ta)), Some((mb, fb, tb))) => Some(if ta >= tb { (*ma, *fa) } else { (*mb, *fb) }),
                (Some((m, f, _)), None) | (None, Some((m, f, _))) => Some((*m, *f)),
                (None, None) => None,
            })
            .collect();
        // gaps: a neighbour's motion
        for _ in 0..64 {
            let holes: Vec<usize> = (0..w * h).filter(|&i| mt[i].is_none()).collect();
            if holes.is_empty() {
                break;
            }
            let fill: Vec<(usize, (Motion, bool))> = holes
                .iter()
                .filter_map(|&i| {
                    let (x, y) = ((i % w) as i64, (i / w) as i64);
                    [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().find_map(|&(dx, dy)| {
                        let (nx, ny) = (x + dx, y + dy);
                        (nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64).then(|| mt[ny as usize * w + nx as usize]).flatten().map(|m| (i, m))
                    })
                })
                .collect();
            for (i, v) in fill {
                mt[i] = Some(v);
            }
        }
        // lone specks of another motion (where two things crossed) go
        let mt: Vec<(Motion, bool)> = mt.into_iter().enumerate().map(|(i, m)| m.unwrap_or((Motion::shift((i % w) as f32, (i / w) as f32, (0.0, 0.0)), false))).collect();
        let mt = vmedian(&vmedian(&mt, w, h, ends), w, h, ends);
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
        // where the motion tears (neighbours going to quite different
        // places): pictures made from it come apart there
        let tear: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let e = ends(&mt[i], x, y);
                let mut worst = 0.0f32;
                for (dx, dy) in [(1i32, 0i32), (0, 1), (-1, 0), (0, -1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let o = ends(&mt[ny as usize * w + nx as usize], nx as f32, ny as f32);
                    let gap = (o[0] - e[0] - dx as f32).hypot(o[1] - e[1] - dy as f32) + (o[2] - e[2] - dx as f32).hypot(o[3] - e[3] - dy as f32);
                    worst = worst.max(gap);
                }
                if worst > 3.0 { 1.0 } else { 0.0 }
            })
            .collect();
        // an outline tears along a line; something in pieces tears all over
        let tear: Vec<f32> = box_blur(&tear, w, h, 4).into_iter().map(|d| ((d - 0.35) / 0.25).clamp(0.0, 1.0)).collect();
        let cam = self.camera;
        let (sx, sy) = self.scale;
        // per pixel: the made colour, and how much the two frames disagree there
        let rows: Vec<Vec<([f32; 4], [f32; 4], f32)>> = (0..fh)
            .into_par_iter()
            .map(|y| {
                (0..fw)
                    .map(|x| {
                        let (wx, wy) = ((x as f32 + 0.5) / sx - 0.5, (y as f32 + 0.5) / sy - 0.5);
                        // where it was in a, and where it will be in b
                        let l = &mt[(wy.round().clamp(0.0, (h - 1) as f32) as usize) * w + wx.round().clamp(0.0, (w - 1) as f32) as usize];
                        let [ax, ay, bx, by] = ends(l, wx, wy);
                        let ca = samp(a, (ax + 0.5) * sx - 0.5, (ay + 0.5) * sy - 0.5);
                        let cb = samp(b, (bx + 0.5) * sx - 0.5, (by + 0.5) * sy - 0.5);
                        // each frame sees the spot only if its own motion
                        // there is this one (else something else is in front)
                        let (vx, vy) = (bx - ax, by - ay);
                        let tol = 1.5 + 0.1 * vx.hypot(vy);
                        let ma = self.ab.sample(ax, ay);
                        let mb = self.ba.sample(bx, by);
                        let sees_a = (-((ma.0 - vx).hypot(ma.1 - vy) / tol).powi(2)).exp().max(0.02);
                        let sees_b = (-((mb.0 + vx).hypot(mb.1 + vy) / tol).powi(2)).exp().max(0.02);
                        let wa = (1.0 - t) * sees_a;
                        let wb = t * sees_b;
                        let o = [0, 1, 2, 3].map(|c| (ca[c] * wa + cb[c] * wb) / (wa + wb));
                        // both frames see this spot yet show different
                        // things: the motion is wrong here
                        let differ = (0..3).map(|c| (ca[c] - cb[c]).abs()).sum::<f32>() / 3.0;
                        // the safe way, for where the motion is in doubt: the
                        // two frames faded, each moved as the camera moved
                        let smooth = match cam {
                            Some(c) => {
                                let p = c.back(wx, wy, t);
                                let q = c.at(p.0, p.1, 1.0);
                                let ga = samp(a, (p.0 + 0.5) * sx - 0.5, (p.1 + 0.5) * sy - 0.5);
                                let gb = samp(b, (q.0 + 0.5) * sx - 0.5, (q.1 + 0.5) * sy - 0.5);
                                [0, 1, 2, 3].map(|c| ga[c] * (1.0 - t) + gb[c] * t)
                            }
                            None => {
                                let (pa, pb) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
                                [0, 1, 2, 3].map(|c| pa[c] as f32 * (1.0 - t) + pb[c] as f32 * t)
                            }
                        };
                        let torn = tear[(wy.round().clamp(0.0, (h - 1) as f32) as usize) * w + wx.round().clamp(0.0, (w - 1) as f32) as usize];
                        // (where one frame can't see the spot they're bound
                        // to differ: that's what uncovering looks like)
                        (o, smooth, (((differ - 35.0) / 45.0).clamp(0.0, 1.0) * sees_a.min(sees_b)).max(torn))
                    })
                    .collect()
            })
            .collect();
        // spread the doubt a little so the fall-back has soft edges
        let (iw, ih) = (fw as usize, fh as usize);
        let raw: Vec<f32> = rows.iter().flat_map(|r| r.iter().map(|p| p.2)).collect();
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
        // where in doubt: the smooth way
        let mut out = RgbaImage::new(fw, fh);
        for (y, row) in rows.into_iter().enumerate() {
            for (x, (o, n, _)) in row.into_iter().enumerate() {
                let k = (doubt[y * iw + x] * 2.0).min(1.0);
                let v = [0, 1, 2, 3].map(|c| o[c] * (1.0 - k) + n[c] * k);
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
