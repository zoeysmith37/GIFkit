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
/// Each with how many patches agree, and how many of those are at the
/// edge of the picture (`w`×`h`): the camera's reach the edges.
fn groups(cells: &[&crate::track::Moved], step: f32, most: usize, w: f32, h: f32) -> Vec<(Motion, usize, usize)> {
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
        // (a few stray patches can agree by chance: past the first thing,
        // a few more are needed)
        if n < if out.is_empty() { 3 } else { 4 } {
            break;
        }
        let ins: Vec<&crate::track::Moved> = left.iter().copied().filter(|c| fits(&m, c)).collect();
        // the turn and growth each patch measured itself (steadier than
        // fitting them to a few middles), about the middle of them all
        let m = {
            let mut angs: Vec<f32> = ins.iter().map(|c| c.ang).collect();
            let mut scs: Vec<f32> = ins.iter().map(|c| c.sc).collect();
            angs.sort_by(|a, b| a.total_cmp(b));
            scs.sort_by(|a, b| a.total_cmp(b));
            let k = ins.len() as f32;
            let (px, py) = (ins.iter().map(|c| c.x).sum::<f32>() / k, ins.iter().map(|c| c.y).sum::<f32>() / k);
            let (qx, qy) = (ins.iter().map(|c| c.to_x).sum::<f32>() / k, ins.iter().map(|c| c.to_y).sum::<f32>() / k);
            Motion { cx: px, cy: py, tx: qx - px, ty: qy - py, ang: angs[angs.len() / 2], sc: scs[scs.len() / 2], rx: 0.0, ry: 0.0 }
        };
        let edge = ins.iter().filter(|c| c.x < step * 1.5 || c.y < step * 1.5 || c.x > w - step * 1.5 || c.y > h - step * 1.5).count();
        left.retain(|c| !fits(&m, c));
        out.push((m, ins.len(), edge));
    }
    out
}

/// Blur `v` over just the pixels where `inside`, so nothing from outside
/// leaks in. Past the edge of `inside` it carries on with what's nearest
/// (wider and wider), so a mesh corner just outside a thing still moves
/// with it.
fn blur_within(v: &[(f32, f32)], inside: &[bool], w: usize, h: usize, r: i32) -> Vec<(f32, f32)> {
    let m: Vec<f32> = inside.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let xs: Vec<f32> = v.iter().zip(&m).map(|(p, k)| p.0 * k).collect();
    let ys: Vec<f32> = v.iter().zip(&m).map(|(p, k)| p.1 * k).collect();
    let mut out: Vec<Option<(f32, f32)>> = vec![None; w * h];
    for rr in [r, r * 3, r * 8] {
        let (den, nx, ny) = (box_blur(&m, w, h, rr), box_blur(&xs, w, h, rr), box_blur(&ys, w, h, rr));
        for i in 0..w * h {
            if out[i].is_none() && den[i] > 1e-3 {
                out[i] = Some((nx[i] / den[i], ny[i] / den[i]));
            }
        }
    }
    out.into_iter().map(|o| o.unwrap_or((0.0, 0.0))).collect()
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
                        count[k[ny as usize * w + nx as usize] as usize & 7] += 1;
                    }
                }
            }
            let mut best = k[i];
            for c in 0..n {
                if count[c] > count[best as usize & 7] {
                    best = c as u8;
                }
            }
            best
        })
        .collect()
}

/// The layer kept for what stays still on screen (burned-in text, a logo).
const STILL: u8 = 7;

/// Each pixel's layer, and each layer's own small motion on top of its
/// thing's. Layer 0 is the fine motion alone (for what no thing explains);
/// layer k is `things[k - 1]`. Each thing's motion is taken out of `b` and
/// what's left (small now) found by the fine search. Every pixel then picks
/// the thing it belongs to, by which fits best around it; and within each
/// layer what's left is smoothed, so it moves as one piece that bends
/// rather than as scattered bits. Still pixels get a layer of their own.
fn pick(a: &Rgb, b: &Rgb, la: &Lum, lb: &Lum, things: &[Motion], fine: &Field, still: Option<&[bool]>) -> (Vec<u8>, Vec<Field>) {
    let (w, h) = (a.w, a.h);
    // the window a fit is judged over
    let agg_r = 8;
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
    let rests: Vec<Field> = things
        .par_iter()
        .map(|&g| {
            // hardly moving: what's left is the fine motion already found
            let corner = g.at(0.0, 0.0, 1.0);
            let far = g.at((w - 1) as f32, (h - 1) as f32, 1.0);
            if corner.0.hypot(corner.1) < 0.5 && (far.0 - (w - 1) as f32).hypot(far.1 - (h - 1) as f32) < 0.5 {
                return fine.clone();
            }
            let warped = Lum {
                w,
                h,
                v: (0..w * h)
                    .map(|i| {
                        let p = g.at((i % w) as f32, (i / w) as f32, 1.0);
                        lb.sample(p.0, p.1)
                    })
                    .collect(),
            };
            // what's left is small and gets smoothed anyway: half size will do
            let rest = if w > 160 { flow_small(&la.half(), &warped.half()).up(w, h) } else { flow_small(la, &warped) };
            // and it's only what's left: much more than a part of the
            // thing's own motion is a wrong match
            let most = 0.3 * g.tx.hypot(g.ty) + 6.0;
            Field {
                w,
                h,
                v: rest
                    .v
                    .into_iter()
                    .map(|r| {
                        let l = r.0.hypot(r.1);
                        if l > most { (r.0 * most / l, r.1 * most / l) } else { r }
                    })
                    .collect(),
            }
        })
        .collect();
    for (&g, r) in things.iter().zip(rests) {
        layers.push((g, r, 0.0));
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
    if let Some(s) = still {
        for (k, &st) in kind.iter_mut().zip(s) {
            if st {
                *k = STILL;
            }
        }
    }
    // what's left, smoothed within each layer (the fine layer less, it's
    // there for small details)
    let rests = layers
        .iter()
        .enumerate()
        .map(|(k, (_, rest, _))| {
            let inside: Vec<bool> = kind.iter().map(|&c| c as usize == k).collect();
            Field { w, h, v: blur_within(&rest.v, &inside, w, h, if k == 0 { 5 } else { 8 }) }
        })
        .collect();
    (kind, rests)
}

/// The size motion is worked out at for frames this big.
fn working_size(fw: u32, fh: u32) -> (usize, usize) {
    let k = (WORK as f32 / fw.max(fh) as f32).min(1.0);
    (((fw as f32 * k).round() as usize).max(8), ((fh as f32 * k).round() as usize).max(8))
}

fn small(img: &RgbaImage, w: usize, h: usize) -> RgbaImage {
    if (img.width() as usize, img.height() as usize) == (w, h) {
        img.clone()
    } else {
        image::imageops::resize(img, w as u32, h as u32, image::imageops::FilterType::Triangle)
    }
}

/// The first look at a pair: the tracker's patches each way and the things
/// moving as one. The in-between job takes one of every pair first, so
/// each pair can then borrow from the pairs either side.
pub struct Glance {
    w: usize,
    h: usize,
    step: f32,
    ha: Hints,
    hb: Hints,
    /// things moving as one, the camera first
    things: Vec<Motion>,
    /// the share of patches (with detail) found in the other frame
    found: f32,
}

impl Glance {
    /// None at a cut (hardly anything in one frame is in the other).
    pub fn new(a: &RgbaImage, b: &RgbaImage) -> Option<Glance> {
        if a.dimensions() != b.dimensions() {
            return None;
        }
        let (w, h) = working_size(a.width(), a.height());
        let (sa, sb) = (small(a, w, h), small(b, w, h));
        let (ga, gb) = (crate::track::Gray::new(&sa), crate::track::Gray::new(&sb));
        let step = (w.max(h) as i32 / 10).clamp(10, 48);
        let r = (step * 7 / 10).max(8);
        let (fa, fb) = rayon::join(|| crate::track::match_grid(&ga, &gb, step, r), || crate::track::match_grid(&gb, &ga, step, r));
        let (ha, hb, detailed, found) = keep_found(fa, fb, step as f32);
        let debug = std::env::var_os("INTERP_DEBUG").is_some();
        if debug {
            eprintln!("patches found: {found} of {detailed} with detail");
        }
        // a cut: hardly anything with detail is in the other frame
        if detailed >= 4 && (found as f32) < detailed as f32 * 0.2 && std::env::var_os("INTERP_FORCE").is_none() {
            return None;
        }
        // whole things moving as one, the same both ways (with the patches
        // found going back, turned round)
        let mut cells: Vec<&crate::track::Moved> = ha.cells.iter().flatten().collect();
        let back: Vec<crate::track::Moved> = hb.cells.iter().flatten().map(|c| crate::track::Moved { x: c.to_x, y: c.to_y, to_x: c.x, to_y: c.y, ang: -c.ang, sc: 1.0 / c.sc, ..*c }).collect();
        cells.extend(back.iter());
        let mut moving = groups(&cells, step as f32, 3, w as f32, h as f32);
        // the camera: the thing reaching the edges of the picture most
        if let Some(i) = (0..moving.len()).max_by_key(|&i| (moving[i].2, moving[i].1)) {
            if moving[i].2 > 0 {
                let c = moving.remove(i);
                moving.insert(0, c);
            }
        }
        let things: Vec<Motion> = moving.into_iter().map(|f| f.0).collect();
        if debug {
            for g in &things {
                eprintln!("thing at ({:.0},{:.0}) moves ({:.1},{:.1}) turns {:.2} grows {:.2}", g.cx, g.cy, g.tx, g.ty, g.ang, g.sc);
            }
        }
        let found = if detailed > 0 { found as f32 / detailed as f32 } else { 1.0 };
        Some(Glance { w, h, step: step as f32, ha, hb, things, found })
    }
}

/// What to do where the motion can't be followed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Unsure {
    /// smear it along its way, as a camera would
    #[default]
    Blur,
    /// the nearer frame, moved as well as it can be
    Nearer,
    /// the two frames faded
    Fade,
}

/// What can be made between two frames.
pub struct Pair {
    /// working size, and from it to full size
    w: usize,
    h: usize,
    scale: (f32, f32),
    /// things moving as one (camera first) and the same going back
    things: Vec<Motion>,
    undo: Vec<Motion>,
    /// each side's layer per pixel (0: the fine motion alone, k: thing
    /// k - 1, STILL) and each layer's own small motion on top
    kind_a: Vec<u8>,
    rest_a: Vec<Field>,
    kind_b: Vec<u8>,
    rest_b: Vec<Field>,
    /// how each thing moved the step before and after, where it's found
    /// there: its path curves through them
    paths: Vec<(Option<(f32, f32)>, Option<(f32, f32)>)>,
    /// full size: what stays still on screen
    still: Option<Vec<bool>>,
    /// each side's motion smoothed right out (one piece, can't break), and
    /// how chaotic the layered motion is around each pixel: where it is,
    /// the smooth one is used instead
    smooth_a: Field,
    smooth_b: Field,
    chaos_a: Vec<f32>,
    chaos_b: Vec<f32>,
}

/// Each pixel's whole motion (to the other frame) from its layer.
fn whole_motion(kind: &[u8], rest: &[Field], things: &[Motion], w: usize, h: usize) -> Field {
    Field {
        w,
        h,
        v: (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                match kind[i] {
                    STILL => (0.0, 0.0),
                    0 => rest[0].v[i],
                    k => things[k as usize - 1].plus(rest[k as usize].v[i]).vec(x, y),
                }
            })
            .collect(),
    }
}

/// How chaotic motion is around each pixel: the share of pixels near it
/// whose motion jumps from a neighbour's. One outline between two things
/// is a thin line of jumps; something breaking up has them everywhere.
fn chaos(f: &Field) -> Vec<f32> {
    let (w, h) = (f.w, f.h);
    let jump: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            let m = f.at(x, y);
            let lim = 3.0 + 0.1 * m.0.hypot(m.1);
            let far = [(1, 0), (0, 1)].iter().any(|&(dx, dy)| {
                let q = f.at(x + dx, y + dy);
                (m.0 - q.0).hypot(m.1 - q.1) > lim
            });
            if far { 1.0 } else { 0.0 }
        })
        .collect();
    let dens = box_blur(&jump, w, h, 5);
    box_blur(&dens.into_iter().map(|d| ((d - 0.08) / 0.12).clamp(0.0, 1.0)).collect::<Vec<_>>(), w, h, 3)
}

/// `f` smoothed right out: one piece that bends, whatever it was before.
fn smoothed(f: &Field) -> Field {
    let (w, h) = (f.w, f.h);
    let r = (w.max(h) as i32 / 24).max(4);
    let xs = box_blur(&box_blur(&f.v.iter().map(|m| m.0).collect::<Vec<_>>(), w, h, r), w, h, r);
    let ys = box_blur(&box_blur(&f.v.iter().map(|m| m.1).collect::<Vec<_>>(), w, h, r), w, h, r);
    Field { w, h, v: xs.into_iter().zip(ys).collect() }
}

impl Pair {
    /// The motion between `a` and `b`, or None at a cut.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(a: &RgbaImage, b: &RgbaImage) -> Option<Pair> {
        let g = Glance::new(a, b)?;
        Some(Pair::finish(a, b, &g, None, None, None))
    }

    /// The motion between `a` and `b` from a first look at them, helped by
    /// the looks at the pairs before and after (things they saw moving are
    /// tried here too, at the same pace, as a TV's motion smoothing does;
    /// and paths curve through them) and by what stays still on screen
    /// (full size).
    pub fn finish(a: &RgbaImage, b: &RgbaImage, g: &Glance, prev: Option<&Glance>, next: Option<&Glance>, still: Option<Vec<bool>>) -> Pair {
        let (fw, fh) = a.dimensions();
        let (w, h) = (g.w, g.h);
        let clock = std::time::Instant::now();
        let lap = |what: &str| {
            if std::env::var_os("INTERP_DEBUG").is_some() {
                eprintln!("  {what}: {} ms", clock.elapsed().as_millis());
            }
        };
        let (sa, sb) = (small(a, w, h), small(b, w, h));
        let (la, lb) = (Lum::from(&sa, w, h), Lum::from(&sb, w, h));
        let (ab, ba) = rayon::join(|| flow(&la, &lb, &g.ha), || flow(&lb, &la, &g.hb));
        lap("fine motion");
        let (ca, cb) = (Rgb::from(&sa).detail(), Rgb::from(&sb).detail());
        // the things, and those the pairs either side saw, carried on at
        // the same pace (where they aren't one of these already)
        let mut things = g.things.clone();
        let mut guesses: Vec<Motion> = Vec::new();
        // (from pairs that were looked at well)
        if let Some(p) = prev.filter(|p| p.found >= 0.5) {
            guesses.extend(p.things.iter().map(|q| Motion { cx: q.cx + q.tx, cy: q.cy + q.ty, ..*q }));
        }
        if let Some(n) = next.filter(|n| n.found >= 0.5) {
            guesses.extend(n.things.iter().map(|q| Motion { cx: q.cx - q.tx, cy: q.cy - q.ty, ..*q }));
        }
        for m in guesses {
            if things.len() >= 4 {
                break;
            }
            let fresh = things.iter().all(|o| {
                let (u, v) = (m.vec(m.cx, m.cy), o.vec(m.cx, m.cy));
                (u.0 - v.0).hypot(u.1 - v.1) > g.step * 0.35
            });
            if fresh {
                things.push(m);
            }
        }
        let undo: Vec<Motion> = things.iter().map(|m| m.inverse()).collect();
        let (sx, sy) = (fw as f32 / w as f32, fh as f32 / h as f32);
        // still at the working size: where the middle of the pixel is
        let still_w: Option<Vec<bool>> = still.as_ref().map(|s| {
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((((i % w) as f32 + 0.5) * sx) as usize, (((i / w) as f32 + 0.5) * sy) as usize);
                    s[y.min(fh as usize - 1) * fw as usize + x.min(fw as usize - 1)]
                })
                .collect()
        });
        let ((kind_a, rest_a), (kind_b, rest_b)) = rayon::join(|| pick(&ca, &cb, &la, &lb, &things, &ab, still_w.as_deref()), || pick(&cb, &ca, &lb, &la, &undo, &ba, still_w.as_deref()));
        lap("layers");
        // each thing's step before and after: the thing there that ends
        // where this one starts (or starts where it ends)
        let reach = g.step * 2.0;
        let paths = things
            .iter()
            .map(|m| {
                let before = prev.and_then(|p| {
                    p.things.iter().map(|q| ((q.cx + q.tx - m.cx).hypot(q.cy + q.ty - m.cy), q)).filter(|(d, _)| *d < reach).min_by(|x, y| x.0.total_cmp(&y.0)).map(|(_, q)| (q.tx, q.ty))
                });
                let after = next.and_then(|n| n.things.iter().map(|q| ((q.cx - m.cx - m.tx).hypot(q.cy - m.cy - m.ty), q)).filter(|(d, _)| *d < reach).min_by(|x, y| x.0.total_cmp(&y.0)).map(|(_, q)| (q.tx, q.ty)));
                (before, after)
            })
            .collect();
        let (wa, wb) = (whole_motion(&kind_a, &rest_a, &things, w, h), whole_motion(&kind_b, &rest_b, &undo, w, h));
        // few patches found at all: the layers can't be trusted anywhere
        // (the camera's background only when hardly anything was found:
        // it's what the patches find most surely)
        let doubt = 1.0 - ((g.found - 0.3) / 0.6).clamp(0.0, 1.0);
        let doubt_cam = 1.0 - ((g.found - 0.2) / 0.3).clamp(0.0, 1.0);
        // (what moves as the camera does counts as the camera)
        let lift = |c: Vec<f32>, kind: &[u8], whole: &Field, cam: Option<&Motion>| -> Vec<f32> {
            c.into_iter()
                .zip(kind)
                .enumerate()
                .map(|(i, (v, &k))| {
                    if k == STILL {
                        return 0.0;
                    }
                    let like_cam = cam.is_some_and(|m| {
                        let (x, y) = ((i % w) as f32, (i / w) as f32);
                        let (u, o) = (m.vec(x, y), whole.v[i]);
                        (u.0 - o.0).hypot(u.1 - o.1) < 2.0 + 0.1 * u.0.hypot(u.1)
                    });
                    v.max(if like_cam { doubt_cam } else { doubt })
                })
                .collect()
        };
        let (smooth_a, smooth_b) = (smoothed(&wa), smoothed(&wb));
        let (chaos_a, chaos_b) = (lift(chaos(&wa), &kind_a, &wa, things.first()), lift(chaos(&wb), &kind_b, &wb, undo.first()));
        Pair { w, h, scale: (sx, sy), things, undo, kind_a, rest_a, kind_b, rest_b, paths, still, smooth_a, smooth_b, chaos_a, chaos_b }
    }

    /// The frame a share `t` of the way from `a` to `b`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at(&self, a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
        self.render(a, b, t, 0.5, Unsure::Blur)
    }

    /// The frame a share `t` of the way from `a` to `b`; `gap` is the time
    /// between in-betweens (a share of the pair), for how long a blur is.
    ///
    /// Each layer is drawn as a mesh from each frame (a grid over the frame,
    /// its corners carried along their motion to the in-between moment), so
    /// a thing can bend and stretch but never come apart. The layers are
    /// laid front to back: what stays still, the things that aren't the
    /// camera, the fine motion, then the camera's background. Within a
    /// layer the two frames mix by time, each only where it sees that spot.
    /// Where the two disagree the motion is wrong, and `unsure` says what
    /// happens there.
    pub fn render(&self, a: &RgbaImage, b: &RgbaImage, t: f32, gap: f32, unsure: Unsure) -> RgbaImage {
        let (fw, fh) = a.dimensions();
        let (w, h) = (self.w, self.h);
        let (sx, sy) = self.scale;
        let n = self.things.len() + 1;
        // front to back
        let mut order: Vec<u8> = vec![STILL];
        order.extend((2..n).map(|k| k as u8));
        order.push(0);
        if n > 1 {
            order.push(1);
        }
        // a thing's path, curved through its steps before and after: the
        // offset from a straight line (full size)
        let bend = |k: u8| -> (f32, f32) {
            if k == STILL || self.things.is_empty() {
                return (0.0, 0.0);
            }
            // the fine motion goes with the camera
            let i = if k == 0 { 0 } else { k as usize - 1 };
            let m = self.things[i];
            let (p, q) = self.paths[i];
            if p.is_none() && q.is_none() {
                return (0.0, 0.0);
            }
            let cr = |p0: f32, p2: f32, p3: f32| {
                let p1 = 0.0;
                0.5 * (2.0 * p1 + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
            };
            let before = p.unwrap_or((m.tx, m.ty));
            let after = q.unwrap_or((m.tx, m.ty));
            let (dx, dy) = (cr(-before.0, m.tx, m.tx + after.0) - t * m.tx, cr(-before.1, m.ty, m.ty + after.1) - t * m.ty);
            let most = 0.35 * m.tx.hypot(m.ty) + 1.0;
            let len = dx.hypot(dy);
            let k = if len > most { most / len } else { 1.0 };
            (dx * k * sx, dy * k * sy)
        };
        // soft masks per layer and side
        let mask = |kind: &[u8], k: u8| -> Vec<f32> { box_blur(&kind.iter().map(|&c| if c == k { 1.0 } else { 0.0 }).collect::<Vec<_>>(), w, h, 1) };
        let mut masks: Vec<(Vec<f32>, Vec<f32>)> = order.par_iter().map(|&k| (mask(&self.kind_a, k), mask(&self.kind_b, k))).collect();
        // the smooth slot: everything, its "tear" how chaotic it is there
        masks.push((vec![1.0; w * h], vec![1.0; w * h]));
        let smooth_slot = order.len();
        let msample = |m: &[f32], x: f32, y: f32| -> f32 {
            let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let r0 = m[y0 * w + x0] * (1.0 - fx) + m[y0 * w + x1] * fx;
            let r1 = m[y1 * w + x0] * (1.0 - fx) + m[y1 * w + x1] * fx;
            r0 * (1.0 - fy) + r1 * fy
        };
        // the meshes: (output corners, source corners at working size,
        // output motion a to b at each corner)
        type Tri = ([(f32, f32); 3], [(f32, f32); 3], [(f32, f32); 3]);
        const G: usize = 4;
        let xs: Vec<usize> = (0..w).step_by(G).chain(std::iter::once(w - 1)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let ys: Vec<usize> = (0..h).step_by(G).chain(std::iter::once(h - 1)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        // k = SMOOTH: the whole frame, by the smoothed-out motion
        const SMOOTH: u8 = 6;
        let mesh = |k: u8, from_b: bool| -> Vec<Tri> {
            let (kind, rest, things) = if from_b { (&self.kind_b, &self.rest_b, &self.undo) } else { (&self.kind_a, &self.rest_a, &self.things) };
            let smooth = if from_b { &self.smooth_b } else { &self.smooth_a };
            let off = bend(if k == SMOOTH { 0 } else { k });
            let ts = if from_b { 1.0 - t } else { t };
            let corner = |x: usize, y: usize| -> ((f32, f32), (f32, f32), (f32, f32)) {
                let (fx, fy) = (x as f32, y as f32);
                let i = y * w + x;
                let m = if k == STILL {
                    Motion::shift(fx, fy, (0.0, 0.0))
                } else if k == SMOOTH {
                    Motion::shift(fx, fy, smooth.v[i])
                } else if k == 0 {
                    Motion::shift(fx, fy, rest[0].v[i])
                } else {
                    things[k as usize - 1].plus(rest[k as usize].v[i])
                };
                let p = m.at(fx, fy, ts);
                let e = m.at(fx, fy, 1.0);
                let v = if from_b { (fx - e.0, fy - e.1) } else { (e.0 - fx, e.1 - fy) };
                (((p.0 + 0.5) * sx - 0.5 + off.0, (p.1 + 0.5) * sy - 0.5 + off.1), (fx, fy), (v.0 * sx, v.1 * sy))
            };
            let cols = xs.len();
            let corners: Vec<_> = ys.iter().flat_map(|&y| xs.iter().map(move |&x| (x, y))).map(|(x, y)| corner(x, y)).collect();
            let mut out = Vec::new();
            for cy in 0..ys.len() - 1 {
                for cx in 0..cols - 1 {
                    // only cells with some of this layer in or next to them
                    let (x0, x1, y0, y1) = (xs[cx].saturating_sub(1), (xs[cx + 1] + 1).min(w - 1), ys[cy].saturating_sub(1), (ys[cy + 1] + 1).min(h - 1));
                    let mut any = k == SMOOTH;
                    'scan: for y in y0..=y1 {
                        if any {
                            break;
                        }
                        for x in x0..=x1 {
                            if kind[y * w + x] == k {
                                any = true;
                                break 'scan;
                            }
                        }
                    }
                    if !any {
                        continue;
                    }
                    let c = [corners[cy * cols + cx], corners[cy * cols + cx + 1], corners[(cy + 1) * cols + cx + 1], corners[(cy + 1) * cols + cx]];
                    for tri in [[c[0], c[1], c[2]], [c[0], c[2], c[3]]] {
                        out.push(([tri[0].0, tri[1].0, tri[2].0], [tri[0].1, tri[1].1, tri[2].1], [tri[0].2, tri[1].2, tri[2].2]));
                    }
                }
            }
            out
        };
        let mut layers_and_smooth = order.clone();
        layers_and_smooth.push(SMOOTH);
        let meshes: Vec<(Vec<Tri>, Vec<Tri>)> = layers_and_smooth.par_iter().map(|&k| (mesh(k, false), mesh(k, true))).collect();
        // bands of rows, each with the triangles that reach it
        const BAND: usize = 16;
        let bands = (fh as usize).div_ceil(BAND);
        let mut bins: Vec<Vec<(usize, bool, usize)>> = vec![Vec::new(); bands];
        for (slot, (ma, mb)) in meshes.iter().enumerate() {
            for (side, m) in [(false, ma), (true, mb)] {
                for (ti, tri) in m.iter().enumerate() {
                    let lo = tri.0.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0);
                    let hi = tri.0.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().min(fh as f32 - 1.0);
                    if hi < 0.0 || lo > fh as f32 - 1.0 || hi < lo {
                        continue;
                    }
                    for band in (lo as usize / BAND)..=(hi as usize / BAND).min(bands - 1) {
                        bins[band].push((slot, side, ti));
                    }
                }
            }
        }
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
        let cam = self.things.first().copied();
        let slots = order.len();
        // per output pixel: the time-mixed colour, the nearer frame's, how
        // much the two frames disagree, and the motion there
        #[derive(Clone, Copy, Default)]
        struct Smp {
            c: [f32; 4],
            a: f32,
            v: (f32, f32),
            /// how torn the mesh is here
            d: f32,
        }
        let done: Vec<Vec<([f32; 4], [f32; 4], f32, (f32, f32))>> = (0..bands)
            .into_par_iter()
            .map(|band| {
                let (y0, y1) = (band * BAND, ((band + 1) * BAND).min(fh as usize));
                let bh = y1 - y0;
                let fwu = fw as usize;
                let mut buf = vec![vec![Smp::default(); bh * fwu]; (slots + 1) * 2];
                for &(slot, side, ti) in &bins[band] {
                    let (p, s, v) = if side { meshes[slot].1[ti] } else { meshes[slot].0[ti] };
                    let m = if side { &masks[slot].1 } else { &masks[slot].0 };
                    let img = if side { b } else { a };
                    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    // stretched far past its size (or folded over): the
                    // mesh is torn here, it fades and counts as in doubt
                    let edge = |q: &[(f32, f32); 3], k: f32, l: f32| (0..3).map(|e| ((q[e].0 - q[(e + 1) % 3].0) * k).hypot((q[e].1 - q[(e + 1) % 3].1) * l)).fold(0.0f32, f32::max);
                    let grow = edge(&p, 1.0, 1.0) / edge(&s, sx, sy).max(1e-3);
                    let tear = ((grow - 2.0) / 2.0).clamp(0.0, 1.0).max(if area < 0.0 { 0.6 } else { 0.0 });
                    let keep = if slot == smooth_slot { 1.0 } else { 1.0 - tear * 0.85 };
                    let wild = if side { &self.chaos_b } else { &self.chaos_a };
                    let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor().max(0.0) as i64;
                    let maxx = p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil().min(fw as f32 - 1.0) as i64;
                    let miny = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor() as i64).max(y0 as i64);
                    let maxy = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as i64).min(y1 as i64 - 1);
                    let out = &mut buf[slot * 2 + side as usize];
                    for y in miny..=maxy {
                        for x in minx..=maxx {
                            let (px, py) = (x as f32, y as f32);
                            let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                            let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                            let w2 = 1.0 - w0 - w1;
                            if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                                continue;
                            }
                            let (qx, qy) = (s[0].0 * w0 + s[1].0 * w1 + s[2].0 * w2, s[0].1 * w0 + s[1].1 * w1 + s[2].1 * w2);
                            let al = msample(m, qx, qy) * keep;
                            let j = (y as usize - y0) * fwu + x as usize;
                            if al <= out[j].a {
                                continue;
                            }
                            let c = samp(img, (qx + 0.5) * sx - 0.5, (qy + 0.5) * sy - 0.5);
                            let d = if slot == smooth_slot { msample(wild, qx, qy) } else { tear };
                            out[j] = Smp { c, a: al, v: (v[0].0 * w0 + v[1].0 * w1 + v[2].0 * w2, v[0].1 * w0 + v[1].1 * w1 + v[2].1 * w2), d };
                        }
                    }
                }
                // front to back
                (0..bh * fwu)
                    .map(|j| {
                        let (x, y) = ((j % fwu) as u32, (y0 + j / fwu) as u32);
                        let (mut acc, mut mix, mut near, mut doubt, mut vel) = (0.0f32, [0.0f32; 4], [0.0f32; 4], 0.0f32, (0.0f32, 0.0f32));
                        for slot in 0..slots {
                            let (sa_, sb_) = (buf[slot * 2][j], buf[slot * 2 + 1][j]);
                            if sa_.a + sb_.a < 1e-3 {
                                continue;
                            }
                            let al = 1.0 - (1.0 - sa_.a) * (1.0 - sb_.a);
                            let (wa, wb) = ((1.0 - t) * sa_.a, t * sb_.a);
                            let both = [0, 1, 2, 3].map(|c| (sa_.c[c] * wa + sb_.c[c] * wb) / (wa + wb).max(1e-6));
                            let nc = if t < 0.5 { if sa_.a > 0.05 { sa_.c } else { sb_.c } } else if sb_.a > 0.05 { sb_.c } else { sa_.c };
                            let differ = (0..3).map(|c| (sa_.c[c] - sb_.c[c]).abs()).sum::<f32>() / 3.0;
                            let d = (((differ - 25.0) / 40.0).clamp(0.0, 1.0) * sa_.a.min(sb_.a)).max((sa_.d * wa + sb_.d * wb) / (wa + wb).max(1e-6));
                            let v = ((sa_.v.0 * wa + sb_.v.0 * wb) / (wa + wb).max(1e-6), (sa_.v.1 * wa + sb_.v.1 * wb) / (wa + wb).max(1e-6));
                            let k = (1.0 - acc) * al;
                            for c in 0..4 {
                                mix[c] += both[c] * k;
                                near[c] += nc[c] * k;
                            }
                            doubt += d * k;
                            vel = (vel.0 + v.0 * k, vel.1 + v.1 * k);
                            acc += k;
                            if acc > 0.999 {
                                break;
                            }
                        }
                        if acc < 0.999 {
                            // seen by neither: the two faded, moved as the camera moved
                            let (wx, wy) = ((x as f32 + 0.5) / sx - 0.5, (y as f32 + 0.5) / sy - 0.5);
                            let f = match cam {
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
                            let k = 1.0 - acc;
                            for c in 0..4 {
                                mix[c] += f[c] * k;
                                near[c] += f[c] * k;
                            }
                        }
                        // where the layers are chaos: the smooth way instead
                        let (pa_, pb_) = (buf[smooth_slot * 2][j], buf[smooth_slot * 2 + 1][j]);
                        let (wa, wb) = ((1.0 - t) * pa_.a, t * pb_.a);
                        if wa + wb > 1e-4 {
                            let k = ((pa_.d * wa + pb_.d * wb) / (wa + wb)).clamp(0.0, 1.0);
                            if k > 0.0 {
                                let sm = [0, 1, 2, 3].map(|c| (pa_.c[c] * wa + pb_.c[c] * wb) / (wa + wb));
                                let sn = if (t < 0.5 && pa_.a > 0.05) || pb_.a <= 0.05 { pa_.c } else { pb_.c };
                                let sv = ((pa_.v.0 * wa + pb_.v.0 * wb) / (wa + wb), (pa_.v.1 * wa + pb_.v.1 * wb) / (wa + wb));
                                for c in 0..4 {
                                    mix[c] = mix[c] * (1.0 - k) + sm[c] * k;
                                    near[c] = near[c] * (1.0 - k) + sn[c] * k;
                                }
                                vel = (vel.0 * (1.0 - k) + sv.0 * k, vel.1 * (1.0 - k) + sv.1 * k);
                                doubt = doubt.max(k);
                            }
                        }
                        (mix, near, doubt, vel)
                    })
                    .collect()
            })
            .collect();
        let px: Vec<([f32; 4], [f32; 4], f32, (f32, f32))> = done.into_iter().flatten().collect();
        let (iw, ih) = (fw as usize, fh as usize);
        // doubt spread a little, so the change is gradual
        let rad = ((fw.max(fh) as f32 / 200.0).ceil() as i32).max(2);
        let doubt = box_blur(&px.iter().map(|p| p.2).collect::<Vec<_>>(), iw, ih, rad);
        let base: Vec<[f32; 4]> = px
            .iter()
            .zip(&doubt)
            .map(|(p, &d)| {
                let k = match unsure {
                    Unsure::Fade => 0.0,
                    Unsure::Nearer => (d * 2.0).min(1.0),
                    Unsure::Blur => d.min(1.0) * 0.5,
                };
                [0, 1, 2, 3].map(|c| p.0[c] * (1.0 - k) + p.1[c] * k)
            })
            .collect();
        let at_base = |x: f32, y: f32| -> [f32; 4] {
            let x = x.clamp(0.0, (iw - 1) as f32);
            let y = y.clamp(0.0, (ih - 1) as f32);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
            let (ax, ay) = (x - x0 as f32, y - y0 as f32);
            [0, 1, 2, 3].map(|c| (base[y0 * iw + x0][c] * (1.0 - ax) + base[y0 * iw + x1][c] * ax) * (1.0 - ay) + (base[y1 * iw + x0][c] * (1.0 - ax) + base[y1 * iw + x1][c] * ax) * ay)
        };
        let still = self.still.as_deref();
        let rows: Vec<Vec<Rgba<u8>>> = (0..ih)
            .into_par_iter()
            .map(|y| {
                (0..iw)
                    .map(|x| {
                        let i = y * iw + x;
                        if still.is_some_and(|s| s[i]) {
                            let (pa, pb) = (a.get_pixel(x as u32, y as u32).0, b.get_pixel(x as u32, y as u32).0);
                            return Rgba([0, 1, 2, 3].map(|c| (pa[c] as f32 * (1.0 - t) + pb[c] as f32 * t).round() as u8));
                        }
                        let mut v = base[i];
                        if unsure == Unsure::Blur {
                            // a shutter open for as long as it's in doubt
                            // (up to the time between in-betweens)
                            let open = gap * (doubt[i] * 2.0).min(1.0);
                            let (vx, vy) = (px[i].3 .0 * open, px[i].3 .1 * open);
                            let len = vx.hypot(vy);
                            if len > 0.75 {
                                let steps = ((len / 1.5).ceil() as usize).clamp(2, 32);
                                let mut sum = [0.0f32; 4];
                                for s in 0..=steps {
                                    let u = s as f32 / steps as f32 - 0.5;
                                    let q = at_base(x as f32 + vx * u, y as f32 + vy * u);
                                    for c in 0..4 {
                                        sum[c] += q[c];
                                    }
                                }
                                v = sum.map(|s| s / (steps + 1) as f32);
                            }
                        }
                        Rgba(v.map(|v| v.round().clamp(0.0, 255.0) as u8))
                    })
                    .collect()
            })
            .collect();
        let mut out = RgbaImage::new(fw, fh);
        for (y, row) in rows.into_iter().enumerate() {
            for (x, p) in row.into_iter().enumerate() {
                out.put_pixel(x as u32, y as u32, p);
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
        let n = a.frames.len();
        let mut sheet = RgbaImage::new(w * 5, h * pairs.len() as u32);
        for (r, &(i, j)) in pairs.iter().enumerate() {
            let img = |k: usize| a.frames[k].image.get();
            let (fa, fb) = (img(i), img(j));
            let t0 = std::time::Instant::now();
            // as the in-between job does it: the pairs either side, and what stays still
            let look = Glance::new(&fa, &fb);
            let prev = (i > 0).then(|| Glance::new(&img(i - 1), &fa)).flatten();
            let next = (j + 1 < n).then(|| Glance::new(&fb, &img(j + 1))).flatten();
            let still = crate::ops::still_mask(&a, i, 4);
            let pair = look.as_ref().map(|g| Pair::finish(&fa, &fb, g, prev.as_ref(), next.as_ref(), still));
            let t1 = std::time::Instant::now();
            let (blur, near) = match &pair {
                Some(p) => (p.render(&fa, &fb, 0.5, 0.5, Unsure::Blur), p.render(&fa, &fb, 0.5, 0.5, Unsure::Nearer)),
                None => ((*fa).clone(), (*fa).clone()),
            };
            eprintln!("{i}-{j}: {} in {:.0} ms (two pictures {:.0} ms)", if pair.is_some() { "made" } else { "held (cut)" }, t0.elapsed().as_secs_f32() * 1000.0, t1.elapsed().as_secs_f32() * 1000.0);
            let blend = crate::ops::lerp_image(&fa, &fb, 0.5);
            for (c, im) in [&*fa, &blend, &blur, &near, &*fb].into_iter().enumerate() {
                image::imageops::overlay(&mut sheet, im, (c as u32 * w) as i64, (r as u32 * h) as i64);
            }
        }
        let small = image::imageops::resize(&sheet, sheet.width() / 2, sheet.height() / 2, image::imageops::FilterType::Triangle);
        small.save(std::env::var("OUT").unwrap()).unwrap();
    }

    /// The whole file with in-betweens, as frames to look at in motion.
    /// FILE=… STEPS=2 OUT=folder cargo test --release inbetween_file -- --ignored
    #[test]
    #[ignore]
    fn inbetween_file() {
        let a = crate::io::load_animation(std::path::Path::new(&std::env::var("FILE").unwrap())).unwrap();
        let steps: usize = std::env::var("STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
        let unsure = match std::env::var("UNSURE").as_deref() {
            Ok("near") => Unsure::Nearer,
            Ok("fade") => Unsure::Fade,
            _ => Unsure::Blur,
        };
        let t0 = std::time::Instant::now();
        let out = crate::ops::inbetween_counted(&a, steps, false, true, unsure, None).0;
        eprintln!("{} frames in {:.1} s", out.frames.len(), t0.elapsed().as_secs_f32());
        let dir = std::path::PathBuf::from(std::env::var("OUT").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        for (k, f) in out.frames.iter().enumerate() {
            f.image.get().save(dir.join(format!("{k:04}.png"))).unwrap();
        }
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
