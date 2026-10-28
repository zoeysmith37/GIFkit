//! Point tracking: follow a patch of the picture from frame to frame, so a
//! layer can stick to something that moves.
//!
//! Frames are kept as colour at full, half and quarter size. Each frame the
//! patch is looked for on the small picture over a wide area (up to 40% of
//! the frame away from where it's expected), turned and sized a little
//! either way since it can spin and grow; then the best place is narrowed
//! down on the bigger pictures. Brightness changes count for less than
//! changes in colour and shape. When nothing matches well it's lost: it
//! looks over the whole frame for how it looked at the start and just now,
//! and if it still isn't there it stays put rather than wandering off.

use image::RgbaImage;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The patch size used for a click: this far out from the point each way.
pub const CLICK_PATCH: i32 = 12;

/// One level of a frame: colour, row by row.
struct Plane {
    w: i32,
    h: i32,
    v: Vec<[f32; 3]>,
}

impl Plane {
    fn at(&self, x: i32, y: i32) -> [f32; 3] {
        self.v[(y.clamp(0, self.h - 1) * self.w + x.clamp(0, self.w - 1)) as usize]
    }

    fn half(&self) -> Plane {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut v = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let (a, b, c, d) = (self.at(2 * x, 2 * y), self.at(2 * x + 1, 2 * y), self.at(2 * x, 2 * y + 1), self.at(2 * x + 1, 2 * y + 1));
                v.push([(a[0] + b[0] + c[0] + d[0]) / 4.0, (a[1] + b[1] + c[1] + d[1]) / 4.0, (a[2] + b[2] + c[2] + d[2]) / 4.0]);
            }
        }
        Plane { w, h, v }
    }
}

/// A frame, ready for matching.
pub struct Gray {
    levels: [Plane; 3],
}

impl Gray {
    pub fn new(img: &RgbaImage) -> Gray {
        let (w, h) = img.dimensions();
        let v = img
            .pixels()
            .map(|p| {
                // see-through counts as black, so a sprite on nothing tracks
                let a = p[3] as f32 / 255.0;
                [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a]
            })
            .collect();
        let full = Plane { w: w as i32, h: h as i32, v };
        let half = full.half();
        let quarter = half.half();
        Gray { levels: [full, half, quarter] }
    }

    fn size(&self) -> (i32, i32) {
        (self.levels[0].w, self.levels[0].h)
    }
}

/// How a patch is laid out: points around its middle, at one level.
#[derive(Clone)]
struct Shape {
    pts: Vec<(f32, f32)>,
}

impl Shape {
    /// `rx`, `ry` out each way at full size, seen at `level`, every `step`
    /// pixels there.
    fn new(rx: i32, ry: i32, level: usize, step: i32) -> Shape {
        let d = (1 << level) as f32;
        let (lx, ly) = ((rx as f32 / d).round().max(2.0) as i32, (ry as f32 / d).round().max(2.0) as i32);
        let mut pts = Vec::new();
        let mut y = -ly;
        while y <= ly {
            let mut x = -lx;
            while x <= lx {
                pts.push((x as f32, y as f32));
                x += step;
            }
            y += step;
        }
        Shape { pts }
    }
}

/// A patch as it looks: colours at each point of a shape (around its own
/// middle, upright), less most of its average.
#[derive(Clone)]
struct Look {
    c: Vec<[f32; 3]>,
    mean: [f32; 3],
}

/// The colours at `shape` around (cx, cy) on `p`, turned by `ang` and
/// sized by `sc`.
fn sample(p: &Plane, shape: &Shape, cx: f32, cy: f32, ang: f32, sc: f32) -> Look {
    let (s, c) = ang.sin_cos();
    let mut v = Vec::with_capacity(shape.pts.len());
    let mut m = [0.0f32; 3];
    for &(x, y) in &shape.pts {
        let (x, y) = (x * sc, y * sc);
        let q = p.at((cx + x * c - y * s).round() as i32, (cy + x * s + y * c).round() as i32);
        m[0] += q[0];
        m[1] += q[1];
        m[2] += q[2];
        v.push(q);
    }
    let n = v.len().max(1) as f32;
    let mean = [m[0] / n, m[1] / n, m[2] / n];
    Look { c: v, mean }
}

/// How unlike `look` the patch at (cx, cy) on `p` is, turned by `ang` and
/// sized by `sc` (as `diff` measures it), without making a look of it.
fn cost_at(p: &Plane, shape: &Shape, look: &Look, cx: f32, cy: f32, ang: f32, sc: f32) -> f32 {
    let (s, c) = ang.sin_cos();
    let at = |x: f32, y: f32| {
        let (x, y) = (x * sc, y * sc);
        p.at((cx + x * c - y * s).round() as i32, (cy + x * s + y * c).round() as i32)
    };
    let mut m = [0.0f32; 3];
    for &(x, y) in &shape.pts {
        let q = at(x, y);
        m[0] += q[0];
        m[1] += q[1];
        m[2] += q[2];
    }
    let n = shape.pts.len().max(1) as f32;
    let m = [m[0] / n, m[1] / n, m[2] / n];
    let mut sum = 0.0;
    for (&(x, y), l) in shape.pts.iter().zip(&look.c) {
        let q = at(x, y);
        for k in 0..3 {
            sum += ((l[k] - look.mean[k]) - (q[k] - m[k])).abs();
        }
    }
    let shift: f32 = (0..3).map(|k| (look.mean[k] - m[k]).abs()).sum::<f32>() / 3.0;
    sum / (n * 3.0) + shift / 3.0
}

/// Where it is and how it sits.
#[derive(Clone, Copy, Debug)]
struct Pose {
    x: f32,
    y: f32,
    /// radians, relative to how it was picked
    ang: f32,
    sc: f32,
}

/// How unlike `looks` (each at its level) a place is. With one look a
/// level, that one. With two (how it looks lately, how it looked when
/// picked) mostly the first but partly the second, so it can change how
/// it looks yet never slide onto something else.
fn best_of(f: &Gray, looks: &[(&Look, usize)], shapes: &[Shape; 3], lvl: usize, cx: f32, cy: f32, ang: f32, sc: f32) -> f32 {
    let mut cs = looks.iter().filter(|(_, l)| *l == lvl).map(|(l, _)| cost_at(&f.levels[lvl], &shapes[lvl], l, cx, cy, ang, sc));
    match (cs.next(), cs.next()) {
        (Some(a), Some(b)) => a * 0.6 + b * 0.4,
        (Some(a), None) => a,
        _ => f32::MAX,
    }
}

/// Look for `looks` in `f` around `guess`: within `reach` full-size
/// pixels, turned up to `turn` radians either way from `base` (a whole turn
/// when it's 3 or more). `pull` favours places near the guess. Returns the
/// best pose and how unlike it is.
///
/// First the place alone, cheaply, on the smallest picture: the few best
/// places are kept. Around each of those it then tries turning and sizing,
/// and the best is narrowed down on the bigger pictures.
#[allow(clippy::too_many_arguments)]
fn search(f: &Gray, looks: &[(&Look, usize)], shapes: &[Shape; 3], base: Pose, guess: (f32, f32), reach: f32, turn: f32, pull: f32) -> (Pose, f32) {
    use rayon::prelude::*;
    // the smallest level the patch is still a few pixels across at
    let top = shapes.iter().rposition(|s| s.pts.len() >= 81).unwrap_or(0);
    let d = (1 << top) as f32;
    let pl = &f.levels[top];
    // the middle stays on the picture
    let inside = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && x <= (pl.w - 1) as f32 && y <= (pl.h - 1) as f32;
    let r = (reach / d).ceil() as i32;
    let (gx, gy) = (guess.0 / d, guess.1 / d);
    // wide looks skip more places, each found place is then looked round
    let step = (r / 24).clamp(1, 8);
    // 1: where, as it sat last time
    let rows: Vec<i32> = (-r..=r).step_by(step as usize).collect();
    let mut spots: Vec<(f32, f32, f32)> = rows
        .par_iter()
        .flat_map_iter(|&dy| {
            let mut v = Vec::new();
            let mut dx = -r;
            while dx <= r {
                let (cx, cy) = (gx + dx as f32, gy + dy as f32);
                if inside(cx, cy) && dx * dx + dy * dy <= r * r {
                    let away = ((dx * dx + dy * dy) as f32).sqrt() * d / reach.max(1.0) * pull;
                    v.push((best_of(f, looks, shapes, top, cx, cy, base.ang, base.sc) + away, cx, cy));
                }
                dx += step;
            }
            v
        })
        .collect();
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    // the best few, not all next to each other, and always where it was
    // expected (small pictures can make everything look alike)
    let mut keep: Vec<(f32, f32, f32)> = Vec::new();
    if inside(gx, gy) {
        keep.push((0.0, gx.round(), gy.round()));
    }
    for s in spots {
        if keep.len() >= 6 {
            break;
        }
        if keep.iter().all(|k| (k.1 - s.1).abs() + (k.2 - s.2).abs() > 3.0) {
            keep.push(s);
        }
    }
    if keep.is_empty() {
        return (base, f32::MAX);
    }
    // the places skipped next to each kept one, still as it sat last time
    if step > 1 {
        keep = keep
            .par_iter()
            .map(|&(c0, kx, ky)| {
                let mut b = (c0, kx, ky);
                for dy in -step..=step {
                    for dx in -step..=step {
                        let (cx, cy) = (kx + dx as f32, ky + dy as f32);
                        if inside(cx, cy) {
                            let away = (((cx - gx).powi(2) + (cy - gy).powi(2)).sqrt() * d / reach.max(1.0)) * pull;
                            let c = best_of(f, looks, shapes, top, cx, cy, base.ang, base.sc) + away;
                            if c < b.0 {
                                b = (c, cx, cy);
                            }
                        }
                    }
                }
                b
            })
            .collect();
    }
    // 2: around each, turned and sized
    let turns: Vec<f32> = if turn >= 3.0 { (0..8).map(|k| k as f32 * std::f32::consts::FRAC_PI_4).collect() } else if turn > 0.0 { vec![-turn / 2.0, 0.0, turn / 2.0] } else { vec![0.0] };
    let sizes = [0.9f32, 1.0, 1.1];
    // the few best after looking round them
    keep.sort_by(|a, b| a.0.total_cmp(&b.0));
    keep.truncate(4);
    let mut best = keep
        .par_iter()
        .map(|&(_, kx, ky)| {
            let mut b = (f32::MAX, base);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (cx, cy) = (kx + dx as f32, ky + dy as f32);
                    if !inside(cx, cy) {
                        continue;
                    }
                    let away = (((cx - gx).powi(2) + (cy - gy).powi(2)).sqrt() * d / reach.max(1.0)) * pull;
                    for &t in &turns {
                        for &s in &sizes {
                            let (ang, sc) = (base.ang + t, (base.sc * s).clamp(0.25, 4.0));
                            let c = best_of(f, looks, shapes, top, cx, cy, ang, sc) + away + if turn < 3.0 { t.abs() * 3.0 } else { 0.0 } + (s - 1.0).abs() * 10.0;
                            if c < b.0 {
                                b = (c, Pose { x: cx * d, y: cy * d, ang, sc });
                            }
                        }
                    }
                }
            }
            b
        })
        .reduce(|| (f32::MAX, base), |a, b| if b.0 < a.0 { b } else { a });
    // 3: closer at each bigger level: a pixel or two, a touch of turn and size
    for lvl in (0..top).rev() {
        let dl = (1 << lvl) as f32;
        let p = &f.levels[lvl];
        let (bx, by) = (best.1.x / dl, best.1.y / dl);
        let fine = 0.06 / (top - lvl) as f32;
        let mut nb = (f32::MAX, best.1);
        let w = if lvl == 0 { 2 } else { 3 };
        for dy in -w..=w {
            for dx in -w..=w {
                let (cx, cy) = (bx + dx as f32, by + dy as f32);
                if cx < 0.0 || cy < 0.0 || cx > (p.w - 1) as f32 || cy > (p.h - 1) as f32 {
                    continue;
                }
                for k in -1..=1 {
                    for s in [0.97f32, 1.0, 1.03] {
                        let (ang, sc) = (best.1.ang + k as f32 * fine, best.1.sc * s);
                        // turning or resizing has to be worth it
                        let c = best_of(f, looks, shapes, lvl, cx, cy, ang, sc) + (k as f32).abs() * 0.4 + (s - 1.0).abs() * 15.0;
                        if c < nb.0 {
                            nb = (c, Pose { x: cx * dl, y: cy * dl, ang, sc });
                        }
                    }
                }
            }
        }
        if nb.0 < f32::MAX {
            best = nb;
        }
    }
    // how unlike, at full size and without the nudges
    let c = best_of(f, looks, shapes, 0, best.1.x, best.1.y, best.1.ang, best.1.sc);
    (best.1, c)
}

/// The patch's look at every level, as it sits in `f` at `pose`.
fn looks_at(f: &Gray, shapes: &[Shape; 3], pose: Pose) -> Vec<Look> {
    (0..3).map(|l| {
        let d = (1 << l) as f32;
        sample(&f.levels[l], &shapes[l], pose.x / d, pose.y / d, pose.ang, pose.sc)
    }).collect()
}

/// One run of the tracker from `start` in direction `dir` up to frame
/// `until` (inclusive), writing into `out`, and into `trust` whether the
/// match there was good. Returns false if stopped.
#[allow(clippy::too_many_arguments)]
fn follow(frames: &[&Gray], start: usize, at: (f32, f32), size: (i32, i32), dir: i64, until: usize, out: &mut [(f32, f32)], trust: &mut [f32], progress: &AtomicUsize) -> bool {
    let n = frames.len();
    let (rx, ry) = (size.0.clamp(4, 200), size.1.clamp(4, 200));
    let shapes = [Shape::new(rx, ry, 0, if rx.max(ry) > 24 { 2 } else { 1 }), Shape::new(rx, ry, 1, 1), Shape::new(rx, ry, 2, 1)];
    let (w, h) = frames[start].size();
    // how far it may go in a frame: a good part of the picture
    let reach = (w.max(h) as f32 * 0.4).max(64.0);
    let mut pose = Pose { x: at.0, y: at.1, ang: 0.0, sc: 1.0 };
    let first = looks_at(frames[start], &shapes, pose);
    let mut last = first.clone();
    out[start] = at;
    trust[start] = 1.0;
    let mut vel = (0.0f32, 0.0f32);
    // how good a match usually is here, to tell when it's lost
    let mut usual: Option<f32> = None;
    let mut lost = 0usize;
    let mut i = start as i64 + dir;
    let past = |i: i64| if dir > 0 { i > until as i64 } else { i < until as i64 };
    while i >= 0 && (i as usize) < n && !past(i) {
        if crate::stack::stopped(Some(progress)) {
            return false;
        }
        let f = frames[i as usize];
        let both: Vec<(&Look, usize)> = (0..3).flat_map(|k| [(&last[k], k), (&first[k], k)]).collect();
        let guess = if lost == 0 { (pose.x + vel.0, pose.y + vel.1) } else { (pose.x, pose.y) };
        let refs = &both;
        // the longer it's been lost, the less like it has to look to count as
        // found again (it may have turned or bent since)
        let limit = usual.map(|u| (u * 2.2).max(u + 12.0) * (1.0 + 0.25 * lost as f32).min(3.0));
        // close by first (quick), as far as it might have gone if not there
        let near = ((vel.0 * vel.0 + vel.1 * vel.1).sqrt() * 3.0 + rx.max(ry) as f32).max(48.0).min(reach);
        let (mut found, mut cost) = search(f, &both, &shapes, pose, guess, near, 0.6, 6.0);
        if near < reach && limit.is_some_and(|l| cost > l) {
            let wide = search(f, &both, &shapes, pose, guess, reach, 0.6, 6.0);
            if wide.1 < cost {
                (found, cost) = wide;
            }
        }
        if std::env::var_os("TRACK_DEBUG").is_some() {
            eprintln!("frame {i}: guess {guess:?} found ({:.1},{:.1}) ang {:.2} sc {:.2} cost {cost:.1} usual {usual:?}", found.x, found.y, found.ang, found.sc);
        }
        let mut ok = limit.is_none_or(|l| cost <= l);
        let mut got = (found, cost);
        if !ok && lost % 2 == 0 {
            // lost: look over the whole picture, any way round (every other
            // frame: it's a big look)
            let whole = (w.max(h) as f32) * 1.2;
            let again = search(f, refs, &shapes, pose, (w as f32 / 2.0, h as f32 / 2.0), whole, std::f32::consts::PI, 0.0);
            if limit.is_some_and(|l| again.1 <= l) {
                ok = true;
                got = again;
            }
        }
        if ok {
            let q = got.0;
            let nv = (q.x - pose.x, q.y - pose.y);
            vel = if lost == 0 { (vel.0 * 0.4 + nv.0 * 0.6, vel.1 * 0.4 + nv.1 * 0.6) } else { (0.0, 0.0) };
            pose = q;
            lost = 0;
            usual = Some(match usual {
                Some(u) => u * 0.75 + got.1 * 0.25,
                None => got.1.max(4.0),
            });
            // how it looks now becomes what's looked for next
            last = looks_at(f, &shapes, pose);
        } else {
            // stays where it was last seen
            lost += 1;
            vel = (0.0, 0.0);
        }
        out[i as usize] = (pose.x, pose.y);
        // how sure: 0 when lost, else higher the better it matched
        trust[i as usize] = if ok { usual.map(|u| (u / got.1.max(0.5)).min(2.0)).unwrap_or(1.0) } else { 0.0 };
        progress.fetch_add(1, Ordering::Relaxed);
        i += dir;
    }
    true
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
    let mut trust = vec![1.0; n];
    follow(frames, start, at, size, 1, n - 1, &mut out, &mut trust, progress);
    follow(frames, start, at, size, -1, 0, &mut out, &mut trust, progress);
    out
}

/// Where a patch of one frame went in the next: its middle at (x, y) moved
/// to (to_x, to_y), turned by `ang` and grown by `sc`. `cost` is how unlike
/// it looks there, `detail` how much it has to go by (flat patches match
/// anywhere).
#[derive(Clone, Copy, Debug)]
pub struct Moved {
    pub x: f32,
    pub y: f32,
    pub to_x: f32,
    pub to_y: f32,
    pub ang: f32,
    pub sc: f32,
    pub cost: f32,
    pub detail: f32,
}

impl Moved {
    /// The motion this patch gives the point (px, py): its turn and size
    /// change taken about its middle.
    pub fn motion_at(&self, px: f32, py: f32) -> (f32, f32) {
        let (s, c) = self.ang.sin_cos();
        let (qx, qy) = ((px - self.x) * self.sc, (py - self.y) * self.sc);
        (self.to_x + qx * c - qy * s - px, self.to_y + qx * s + qy * c - py)
    }
}

/// Patches every `step` pixels (each `r` out each way) of `a`, looked for in
/// `b` over a wide area, turned and sized: a rough picture of how everything
/// moved, for in-betweens to start from. Row by row.
pub fn match_grid(a: &Gray, b: &Gray, step: i32, r: i32) -> (usize, usize, Vec<Moved>) {
    use rayon::prelude::*;
    let (w, h) = a.size();
    let (gw, gh) = (((w + step - 1) / step).max(1) as usize, ((h + step - 1) / step).max(1) as usize);
    // big patches: every few pixels is plenty to tell where they went
    let shapes = [Shape::new(r, r, 0, if r > 16 { 3 } else { 1 }), Shape::new(r, r, 1, if r > 16 { 2 } else { 1 }), Shape::new(r, r, 2, 1)];
    let reach = (w.max(h) as f32 * 0.5).max(32.0);
    let centre = |i: usize| (((i % gw) as i32 * step + step / 2).min(w - 1) as f32, ((i / gw) as i32 * step + step / 2).min(h - 1) as f32);
    let prepare = |i: usize| {
        let (x, y) = centre(i);
        let looks = looks_at(a, &shapes, Pose { x, y, ang: 0.0, sc: 1.0 });
        let l0 = &looks[0];
        let detail = l0.c.iter().map(|q| (0..3).map(|k| (q[k] - l0.mean[k]).abs()).sum::<f32>() / 3.0).sum::<f32>() / l0.c.len().max(1) as f32;
        (x, y, looks, detail)
    };
    // staying put is favoured a little, so flat or repeating patches don't
    // jump about
    let wide = |x: f32, y: f32, looks: &[Look]| {
        let refs: Vec<(&Look, usize)> = looks.iter().enumerate().map(|(k, l)| (l, k)).collect();
        search(b, &refs, &shapes, Pose { x, y, ang: 0.0, sc: 1.0 }, (x, y), reach, 1.6, 2.0)
    };
    // a few patches looked for far and wide show how most things moved
    let few: Vec<(f32, f32)> = (0..gw * gh)
        .into_par_iter()
        .filter(|&i| (i % gw) % 3 == 1 && (i / gw) % 3 == 1)
        .filter_map(|i| {
            let (x, y, looks, detail) = prepare(i);
            (detail >= 3.0).then(|| {
                let (q, _) = wide(x, y, &looks);
                (q.x - x, q.y - y)
            })
        })
        .collect();
    let typical = if few.is_empty() {
        (0.0, 0.0)
    } else {
        let mut xs: Vec<f32> = few.iter().map(|m| m.0).collect();
        let mut ys: Vec<f32> = few.iter().map(|m| m.1).collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        ys.sort_by(|a, b| a.total_cmp(b));
        (xs[xs.len() / 2], ys[ys.len() / 2])
    };
    let v = (0..gw * gh)
        .into_par_iter()
        .map(|i| {
            let (x, y, looks, detail) = prepare(i);
            let refs: Vec<(&Look, usize)> = looks.iter().enumerate().map(|(k, l)| (l, k)).collect();
            let pose = Pose { x, y, ang: 0.0, sc: 1.0 };
            // near where most things went first (quick), far and wide only
            // when it isn't there
            let mut got = search(b, &refs, &shapes, pose, (x + typical.0, y + typical.1), step as f32 * 2.0, 1.6, 2.0);
            if got.1 > (detail * 0.45).max(6.0) {
                let far = wide(x, y, &looks);
                if far.1 < got.1 {
                    got = far;
                }
            }
            let (q, cost) = got;
            Moved { x, y, to_x: q.x, to_y: q.y, ang: q.ang, sc: q.sc, cost, detail }
        })
        .collect();
    (gw, gh, v)
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
/// the two by how near each one is, trusting only the parts that held on,
/// so drift never builds up. Before the first and after the last, it
/// tracks out from that one.
pub fn track_anchors(frames: &[&Gray], anchors: &[Anchor], progress: &AtomicUsize) -> Vec<(f32, f32)> {
    let n = frames.len();
    let mut a: Vec<Anchor> = anchors.iter().copied().filter(|a| a.frame < n).collect();
    a.sort_by_key(|a| a.frame);
    a.dedup_by_key(|a| a.frame);
    if n == 0 || a.is_empty() {
        return vec![(0.0, 0.0); n];
    }
    let mut out = vec![a[0].at; n];
    let mut scratch = vec![1.0; n];
    // out before the first, out after the last
    follow(frames, a[0].frame, a[0].at, a[0].size, -1, 0, &mut out, &mut scratch, progress);
    let l = a[a.len() - 1];
    follow(frames, l.frame, l.at, l.size, 1, n - 1, &mut out, &mut scratch, progress);
    for w in a.windows(2) {
        let (p, q) = (w[0], w[1]);
        let (mut fwd, mut back) = (vec![(0.0, 0.0); n], vec![(0.0, 0.0); n]);
        let (mut tf, mut tb) = (vec![0.0f32; n], vec![0.0f32; n]);
        follow(frames, p.frame, p.at, p.size, 1, q.frame, &mut fwd, &mut tf, progress);
        follow(frames, q.frame, q.at, q.size, -1, p.frame, &mut back, &mut tb, progress);
        let span = (q.frame - p.frame) as f32;
        for i in p.frame..=q.frame {
            let t = (i - p.frame) as f32 / span.max(1.0);
            let blend = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 * (1.0 - t) + b.0 * t, a.1 * (1.0 - t) + b.1 * t);
            let apart = ((fwd[i].0 - back[i].0).powi(2) + (fwd[i].1 - back[i].1).powi(2)).sqrt();
            let near = p.size.0.max(p.size.1).max(q.size.0).max(q.size.1) as f32;
            out[i] = match (tf[i] > 0.0, tb[i] > 0.0) {
                // both found the same thing: lean on the nearer one
                (true, true) if apart <= near => blend(fwd[i], back[i], t),
                // both found something, but not the same: the surer one
                // (nearness counts a little)
                (true, true) => {
                    if tf[i] * (1.5 - t) >= tb[i] * (0.5 + t) { fwd[i] } else { back[i] }
                }
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
                // a texture that isn't the same shifted (a checkerboard is)
                let (cx, cy) = (fx as u32 / 3, fy as u32 / 3);
                let hsh = (cx.wrapping_mul(73856093) ^ cy.wrapping_mul(19349663)).wrapping_mul(2654435761) >> 28;
                let c = 70 + (hsh as i32) * 11;
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
        // the square jumps across the frame half way: with a second picked
        // spot after the jump, both halves are right
        let bg = noise_bg(200, 150, 9);
        let path: Vec<(f32, f32)> = (0..10).map(|i| if i < 5 { (20.0 + i as f32 * 3.0, 30.0) } else { (150.0 + i as f32 * 3.0 - 15.0, 110.0) }).collect();
        let grays: Vec<Gray> = path.iter().map(|&(x, y)| Gray::new(&frame(&bg, x, y, 0))).collect();
        let refs: Vec<&Gray> = grays.iter().collect();
        let c = |i: usize| (path[i].0 + 10.0, path[i].1 + 10.0);
        // (alone it may find it again too: it looks over the whole frame when lost)
        // a box drawn round the square: the patch fits it
        let anchors = [Anchor { frame: 0, at: c(0), size: (9, 9) }, Anchor { frame: 6, at: c(6), size: (9, 9) }];
        let got = track_anchors(&refs, &anchors, &AtomicUsize::new(0));
        for i in [1, 2, 3, 7, 8, 9] {
            assert!((got[i].0 - c(i).0).abs() <= 1.0 && (got[i].1 - c(i).1).abs() <= 1.0, "frame {i}: {:?} not {:?}", got[i], c(i));
        }
    }
}

/// FILE=clip SPOTS="frame,x,y,rx,ry;..." OUT=sheet.png: track and draw the
/// path on every frame, for judging the tracker on real footage.
#[cfg(test)]
mod harness {
    use super::*;

    #[test]
    #[ignore]
    fn track_file() {
        let a = crate::io::load_animation(std::path::Path::new(&std::env::var("FILE").unwrap())).unwrap();
        let spots: Vec<Anchor> = std::env::var("SPOTS").unwrap().split(';').filter(|s| !s.is_empty()).map(|s| {
            let v: Vec<f32> = s.split(',').map(|x| x.trim().parse().unwrap()).collect();
            Anchor { frame: v[0] as usize, at: (v[1], v[2]), size: (v[3] as i32, v[4] as i32) }
        }).collect();
        let imgs: Vec<std::sync::Arc<RgbaImage>> = a.frames.iter().map(|f| f.image.get()).collect();
        eprintln!("loaded {} frames at {:?}", imgs.len(), imgs[0].dimensions());
        let grays: Vec<Gray> = imgs.iter().map(|i| Gray::new(i)).collect();
        let refs: Vec<&Gray> = grays.iter().collect();
        let t = std::time::Instant::now();
        let pts = track_anchors(&refs, &spots, &AtomicUsize::new(0));
        eprintln!("{} frames in {:.2}s", pts.len(), t.elapsed().as_secs_f32());
        for (i, p) in pts.iter().enumerate() {
            eprintln!("{i}: {:.0},{:.0}", p.0, p.1);
        }
        // a sheet: each frame small, with a cross where it thinks the thing is
        let (w, h) = imgs[0].dimensions();
        let sc = 200.0 / w as f32;
        let (tw, th) = (200u32, (h as f32 * sc) as u32);
        let cols = 8u32;
        let rows = (imgs.len() as u32).div_ceil(cols);
        let mut sheet = RgbaImage::new(tw * cols, th * rows);
        for (i, img) in imgs.iter().enumerate() {
            let small = image::imageops::resize(&**img, tw, th, image::imageops::FilterType::Triangle);
            let (ox, oy) = ((i as u32 % cols) * tw, (i as u32 / cols) * th);
            image::imageops::overlay(&mut sheet, &small, ox as i64, oy as i64);
            let (cx, cy) = ((pts[i].0 * sc) as i32 + ox as i32, (pts[i].1 * sc) as i32 + oy as i32);
            let red = image::Rgba([255, 0, 40, 255]);
            for d in -9..=9i32 {
                for t in -1..=1i32 {
                    for (x, y) in [(cx + d, cy + t), (cx + t, cy + d)] {
                        if x >= ox as i32 && y >= oy as i32 && x < (ox + tw) as i32 && y < (oy + th) as i32 {
                            sheet.put_pixel(x as u32, y as u32, red);
                        }
                    }
                }
            }
        }
        sheet.save(std::env::var("OUT").unwrap()).unwrap();
    }
}
