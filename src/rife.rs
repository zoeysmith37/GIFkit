//! AI in-betweens: RIFE (Real-Time Intermediate Flow Estimation, Huang et
//! al.; model v4.6 from Practical-RIFE, MIT licence; converted for ncnn by
//! nihui, MIT licence), run by a small network runtime of our own so
//! nothing outside GIFkit is needed.
//!
//! The network takes two frames and a moment between them and gives the
//! motion from that moment back to each frame, plus a mask saying how much
//! of each frame to use. It works on a smaller copy (it's the slow part);
//! the motion is then scaled up and the full-size frames are moved along it
//! and mixed by the mask here, so detail is kept.
//!
//! The model is stored as ncnn has it: a text file listing layers and a
//! binary file of weights (16-bit). Only the layer kinds this model uses
//! are understood.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// The longer side the network works at: big moves are found better
/// smaller, and it's quicker; detail comes back from the full-size frames.
pub const RUN_SIZE: u32 = 448;

const PARAM: &str = include_str!("../assets/rife/flownet.param");
const WEIGHTS: &[u8] = include_bytes!("../assets/rife/flownet.bin");

/// Channels of a picture, row by row.
#[derive(Clone)]
pub(crate) struct Mat {
    c: usize,
    h: usize,
    w: usize,
    d: Vec<f32>,
}

impl Mat {
    fn new(c: usize, h: usize, w: usize) -> Mat {
        Mat { c, h, w, d: vec![0.0; c * h * w] }
    }

    fn plane(&self, c: usize) -> &[f32] {
        &self.d[c * self.h * self.w..(c + 1) * self.h * self.w]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    RSub,
}

enum Op {
    Input,
    Split,
    Concat,
    /// bilinear, as PyTorch without align_corners
    Interp { sh: f32, sw: f32 },
    Conv { out: usize, k: usize, stride: usize, pad: usize, taps: Vec<Vec<f32>>, bias: Vec<f32>, leaky: Option<f32> },
    Deconv { out: usize, k: usize, stride: usize, pad: usize, taps: Vec<Vec<f32>>, bias: Vec<f32> },
    Binary(Bin),
    Scalar(Bin, f32),
    Leaky(f32),
    Shuffle(usize),
    Crop { from: usize, to: usize },
    Sum(Vec<f32>),
    Sigmoid,
    Warp,
}

struct Layer {
    op: Op,
    ins: Vec<usize>,
    outs: Vec<usize>,
}

pub(crate) struct Net {
    layers: Vec<Layer>,
    blobs: HashMap<String, usize>,
}

fn f16_to_f32(b: u16) -> f32 {
    half::f16::from_bits(b).to_f32()
}

impl Net {
    /// Read the model. Errors say what didn't fit (it's checked by tests).
    fn load(param: &str, mut bin: &[u8]) -> Result<Net, String> {
        let mut lines = param.lines();
        if lines.next().map(str::trim) != Some("7767517") {
            return Err("not an ncnn param file".into());
        }
        lines.next();
        let mut blobs: HashMap<String, usize> = HashMap::new();
        let id = |n: &str, blobs: &mut HashMap<String, usize>| -> usize {
            let k = blobs.len();
            *blobs.entry(n.to_string()).or_insert(k)
        };
        let mut layers = Vec::new();
        let take = |n: usize, bin: &mut &[u8]| -> Result<Vec<u8>, String> {
            if bin.len() < n {
                return Err("weights end early".into());
            }
            let (a, b) = bin.split_at(n);
            *bin = b;
            Ok(a.to_vec())
        };
        for line in lines {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() < 4 {
                continue;
            }
            let (kind, nin, nout) = (t[0], t[2].parse::<usize>().map_err(|e| e.to_string())?, t[3].parse::<usize>().map_err(|e| e.to_string())?);
            let ins: Vec<usize> = t[4..4 + nin].iter().map(|n| id(n, &mut blobs)).collect();
            let outs: Vec<usize> = t[4 + nin..4 + nin + nout].iter().map(|n| id(n, &mut blobs)).collect();
            // key=value, and -233xx=count,values for lists
            let mut p: HashMap<i32, Vec<f32>> = HashMap::new();
            for kv in &t[4 + nin + nout..] {
                let (k, v) = kv.split_once('=').ok_or("bad parameter")?;
                let k: i32 = k.parse().map_err(|_| "bad key")?;
                let vals: Vec<f32> = if k <= -23300 { v.split(',').skip(1).map(|x| x.parse().unwrap_or(0.0)).collect() } else { vec![v.parse().map_err(|_| format!("bad value {v}"))?] };
                p.insert(if k <= -23300 { -(k + 23300) } else { k }, vals);
            }
            let g = |k: i32, d: f32| p.get(&k).and_then(|v| v.first()).copied().unwrap_or(d);
            let op = match kind {
                "Input" => Op::Input,
                "Split" => Op::Split,
                "Concat" => Op::Concat,
                "Interp" => {
                    if g(0, 0.0) as i32 != 2 {
                        return Err("only bilinear Interp".into());
                    }
                    Op::Interp { sh: g(1, 1.0), sw: g(2, 1.0) }
                }
                "Convolution" | "Deconvolution" => {
                    let out = g(0, 0.0) as usize;
                    let k = g(1, 1.0) as usize;
                    let stride = g(3, 1.0) as usize;
                    let pad = g(4, 0.0) as usize;
                    let n = g(6, 0.0) as usize;
                    let has_bias = g(5, 0.0) as i32 == 1;
                    let flag = u32::from_le_bytes(take(4, &mut bin)?.try_into().unwrap());
                    let w: Vec<f32> = match flag {
                        0 => take(n * 4, &mut bin)?.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
                        0x0130_6B47 => {
                            let raw = take(n * 2, &mut bin)?;
                            // 16-bit weights are padded to whole 4 bytes
                            if n % 2 == 1 {
                                take(2, &mut bin)?;
                            }
                            raw.chunks(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect()
                        }
                        f => return Err(format!("weights stored as {f:#x}")),
                    };
                    let bias: Vec<f32> = if has_bias { take(out * 4, &mut bin)?.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect() } else { vec![0.0; out] };
                    let inp = n / (out * k * k);
                    // one matrix (out × in) per kernel tap
                    let taps: Vec<Vec<f32>> = (0..k * k).map(|tap| (0..out * inp).map(|oi| w[oi * k * k + tap]).collect()).collect();
                    if kind == "Convolution" {
                        let act = g(9, 0.0) as i32;
                        let leaky = match act {
                            0 => None,
                            2 => Some(p.get(&10).and_then(|v| v.first()).copied().unwrap_or(0.0)),
                            a => return Err(format!("activation {a}")),
                        };
                        Op::Conv { out, k, stride, pad, taps, bias, leaky }
                    } else {
                        Op::Deconv { out, k, stride, pad, taps, bias }
                    }
                }
                "BinaryOp" => {
                    let b = match g(0, 0.0) as i32 {
                        0 => Bin::Add,
                        1 => Bin::Sub,
                        2 => Bin::Mul,
                        3 => Bin::Div,
                        7 => Bin::RSub,
                        o => return Err(format!("BinaryOp {o}")),
                    };
                    if g(1, 0.0) as i32 == 1 {
                        Op::Scalar(b, g(2, 0.0))
                    } else {
                        Op::Binary(b)
                    }
                }
                "ReLU" => Op::Leaky(g(0, 0.0)),
                "PixelShuffle" => Op::Shuffle(g(0, 1.0) as usize),
                "Crop" => {
                    let (s, e, a) = (p.get(&9).cloned().unwrap_or_default(), p.get(&10).cloned().unwrap_or_default(), p.get(&11).cloned().unwrap_or_default());
                    if a != [0.0] || s.len() != 1 || e.len() != 1 {
                        return Err("only channel Crop".into());
                    }
                    Op::Crop { from: s[0] as usize, to: e[0] as usize }
                }
                "Eltwise" => {
                    if g(0, 0.0) as i32 != 1 {
                        return Err("only Eltwise sum".into());
                    }
                    Op::Sum(p.get(&1).cloned().unwrap_or_default())
                }
                "Sigmoid" => Op::Sigmoid,
                "rife.Warp" => Op::Warp,
                k => return Err(format!("layer {k}")),
            };
            layers.push(Layer { op, ins, outs });
        }
        if !bin.is_empty() {
            return Err(format!("{} bytes of weights left over", bin.len()));
        }
        Ok(Net { layers, blobs })
    }

    /// Run it with the named inputs; gives the named outputs.
    fn run(&self, inputs: &[(&str, Mat)], want: &[&str]) -> Result<Vec<Arc<Mat>>, String> {
        let mut vals: Vec<Option<Arc<Mat>>> = vec![None; self.blobs.len()];
        for (n, m) in inputs {
            let i = *self.blobs.get(*n).ok_or(format!("no input {n}"))?;
            vals[i] = Some(Arc::new(m.clone()));
        }
        for l in &self.layers {
            let get = |k: usize| vals[l.ins[k]].clone().ok_or("missing value");
            let out: Arc<Mat> = match &l.op {
                Op::Input => continue,
                Op::Split => {
                    let v = get(0)?;
                    for &o in &l.outs {
                        vals[o] = Some(v.clone());
                    }
                    continue;
                }
                Op::Concat => {
                    let parts: Vec<Arc<Mat>> = (0..l.ins.len()).map(get).collect::<Result<_, _>>()?;
                    let (h, w) = (parts[0].h, parts[0].w);
                    let mut d = Vec::with_capacity(parts.iter().map(|p| p.d.len()).sum());
                    for p in &parts {
                        if (p.h, p.w) != (h, w) {
                            return Err("Concat sizes differ".into());
                        }
                        d.extend_from_slice(&p.d);
                    }
                    Arc::new(Mat { c: parts.iter().map(|p| p.c).sum(), h, w, d })
                }
                Op::Interp { sh, sw } => {
                    let v = get(0)?;
                    Arc::new(resize(&v, (v.h as f32 * sh) as usize, (v.w as f32 * sw) as usize))
                }
                Op::Conv { out, k, stride, pad, taps, bias, leaky } => Arc::new(conv(&*get(0)?, *out, *k, *stride, *pad, taps, bias, *leaky)),
                Op::Deconv { out, k, stride, pad, taps, bias } => Arc::new(deconv(&*get(0)?, *out, *k, *stride, *pad, taps, bias)),
                Op::Binary(b) => {
                    let (x, y) = (get(0)?, get(1)?);
                    Arc::new(binary(&x, &y, *b)?)
                }
                Op::Scalar(b, s) => {
                    let x = get(0)?;
                    let f = |v: f32| match b {
                        Bin::Add => v + s,
                        Bin::Sub => v - s,
                        Bin::Mul => v * s,
                        Bin::Div => v / s,
                        Bin::RSub => s - v,
                    };
                    Arc::new(Mat { d: x.d.par_iter().map(|&v| f(v)).collect(), ..(*x).clone_shape() })
                }
                Op::Leaky(s) => {
                    let x = get(0)?;
                    Arc::new(Mat { d: x.d.par_iter().map(|&v| if v < 0.0 { v * s } else { v }).collect(), ..(*x).clone_shape() })
                }
                Op::Sigmoid => {
                    let x = get(0)?;
                    Arc::new(Mat { d: x.d.par_iter().map(|&v| 1.0 / (1.0 + (-v).exp())).collect(), ..(*x).clone_shape() })
                }
                Op::Shuffle(r) => Arc::new(shuffle(&*get(0)?, *r)),
                Op::Crop { from, to } => {
                    let x = get(0)?;
                    let hw = x.h * x.w;
                    Arc::new(Mat { c: to - from, h: x.h, w: x.w, d: x.d[from * hw..to * hw].to_vec() })
                }
                Op::Sum(k) => {
                    let (x, y) = (get(0)?, get(1)?);
                    let (a, b) = (k.first().copied().unwrap_or(1.0), k.get(1).copied().unwrap_or(1.0));
                    Arc::new(Mat { d: x.d.par_iter().zip(&y.d).map(|(p, q)| p * a + q * b).collect(), ..(*x).clone_shape() })
                }
                Op::Warp => Arc::new(warp(&*get(0)?, &*get(1)?)),
            };
            vals[l.outs[0]] = Some(out);
        }
        want.iter().map(|n| self.blobs.get(*n).and_then(|&i| vals[i].clone()).ok_or(format!("no output {n}"))).collect()
    }
}

impl Mat {
    fn clone_shape(&self) -> Mat {
        Mat { c: self.c, h: self.h, w: self.w, d: Vec::new() }
    }
}

/// `dst (m×n) += a (m×k) · b (k×n)`, all row by row.
fn matmul(dst: &mut [f32], a: &[f32], b: &[f32], m: usize, k: usize, n: usize) {
    debug_assert!(dst.len() >= m * n && a.len() >= m * k && b.len() >= k * n);
    let par = if m * n * k > 1 << 16 { gemm::Parallelism::Rayon(0) } else { gemm::Parallelism::None };
    // SAFETY: the sizes and strides describe the slices exactly (checked above)
    unsafe {
        gemm::gemm(m, n, k, dst.as_mut_ptr(), 1, n as isize, true, a.as_ptr(), 1, k as isize, b.as_ptr(), 1, n as isize, 1.0f32, 1.0f32, false, false, false, par);
    }
}

/// A convolution, as one matrix product per kernel tap over a shifted copy.
#[allow(clippy::too_many_arguments)]
fn conv(x: &Mat, out: usize, k: usize, stride: usize, pad: usize, taps: &[Vec<f32>], bias: &[f32], leaky: Option<f32>) -> Mat {
    let ho = (x.h + 2 * pad - k) / stride + 1;
    let wo = (x.w + 2 * pad - k) / stride + 1;
    let n = ho * wo;
    let mut o = Mat::new(out, ho, wo);
    for (c, b) in bias.iter().enumerate() {
        o.d[c * n..(c + 1) * n].fill(*b);
    }
    let mut shifted = vec![0.0f32; x.c * n];
    for ky in 0..k {
        for kx in 0..k {
            shifted.par_chunks_mut(n).enumerate().for_each(|(ci, dst)| {
                let src = x.plane(ci);
                for oy in 0..ho {
                    let sy = (oy * stride + ky) as isize - pad as isize;
                    let row = &mut dst[oy * wo..(oy + 1) * wo];
                    if sy < 0 || sy >= x.h as isize {
                        row.fill(0.0);
                        continue;
                    }
                    let srow = &src[sy as usize * x.w..(sy as usize + 1) * x.w];
                    for (ox, v) in row.iter_mut().enumerate() {
                        let sx = (ox * stride + kx) as isize - pad as isize;
                        *v = if sx < 0 || sx >= x.w as isize { 0.0 } else { srow[sx as usize] };
                    }
                }
            });
            matmul(&mut o.d, &taps[ky * k + kx], &shifted, out, x.c, n);
        }
    }
    if let Some(s) = leaky {
        o.d.par_iter_mut().for_each(|v| {
            if *v < 0.0 {
                *v *= s
            }
        });
    }
    o
}

/// A transposed convolution: each tap's product spread back out.
fn deconv(x: &Mat, out: usize, k: usize, stride: usize, pad: usize, taps: &[Vec<f32>], bias: &[f32]) -> Mat {
    let ho = (x.h - 1) * stride + k - 2 * pad;
    let wo = (x.w - 1) * stride + k - 2 * pad;
    let n = x.h * x.w;
    let mut o = Mat::new(out, ho, wo);
    for (c, b) in bias.iter().enumerate() {
        o.d[c * ho * wo..(c + 1) * ho * wo].fill(*b);
    }
    let mut part = vec![0.0f32; out * n];
    for ky in 0..k {
        for kx in 0..k {
            part.fill(0.0);
            matmul(&mut part, &taps[ky * k + kx], &x.d, out, x.c, n);
            o.d.par_chunks_mut(ho * wo).enumerate().for_each(|(c, dst)| {
                let src = &part[c * n..(c + 1) * n];
                for y in 0..x.h {
                    let oy = (y * stride + ky) as isize - pad as isize;
                    if oy < 0 || oy >= ho as isize {
                        continue;
                    }
                    for xx in 0..x.w {
                        let ox = (xx * stride + kx) as isize - pad as isize;
                        if ox >= 0 && ox < wo as isize {
                            dst[oy as usize * wo + ox as usize] += src[y * x.w + xx];
                        }
                    }
                }
            });
        }
    }
    o
}

/// Bilinear resize, as PyTorch's (align_corners off, no smoothing first).
fn resize(x: &Mat, h: usize, w: usize) -> Mat {
    let coeffs = |n_in: usize, n_out: usize| -> Vec<(usize, usize, f32)> {
        let scale = n_in as f32 / n_out as f32;
        (0..n_out)
            .map(|d| {
                let f = ((d as f32 + 0.5) * scale - 0.5).max(0.0);
                let s = (f.floor() as usize).min(n_in - 1);
                let a = f - s as f32;
                if s + 1 >= n_in { (n_in - 1, n_in - 1, 0.0) } else { (s, s + 1, a) }
            })
            .collect()
    };
    let (cy, cx) = (coeffs(x.h, h), coeffs(x.w, w));
    let mut o = Mat::new(x.c, h, w);
    o.d.par_chunks_mut(h * w).enumerate().for_each(|(c, dst)| {
        let src = x.plane(c);
        for (y, &(y0, y1, b)) in cy.iter().enumerate() {
            for (xx, &(x0, x1, a)) in cx.iter().enumerate() {
                let top = src[y0 * x.w + x0] * (1.0 - a) + src[y0 * x.w + x1] * a;
                let bot = src[y1 * x.w + x0] * (1.0 - a) + src[y1 * x.w + x1] * a;
                dst[y * w + xx] = top * (1.0 - b) + bot * b;
            }
        }
    });
    o
}

fn shuffle(x: &Mat, r: usize) -> Mat {
    let c = x.c / (r * r);
    let (h, w) = (x.h * r, x.w * r);
    let mut o = Mat::new(c, h, w);
    o.d.par_chunks_mut(h * w).enumerate().for_each(|(oc, dst)| {
        for dy in 0..r {
            for dx in 0..r {
                let src = x.plane(oc * r * r + dy * r + dx);
                for y in 0..x.h {
                    for xx in 0..x.w {
                        dst[(y * r + dy) * w + xx * r + dx] = src[y * x.w + xx];
                    }
                }
            }
        }
    });
    o
}

fn binary(x: &Mat, y: &Mat, b: Bin) -> Result<Mat, String> {
    let hw = x.h * x.w;
    if (x.h, x.w) != (y.h, y.w) || !(x.c == y.c || y.c == 1 || x.c == 1) {
        return Err("BinaryOp shapes".into());
    }
    let c = x.c.max(y.c);
    let f = |p: f32, q: f32| match b {
        Bin::Add => p + q,
        Bin::Sub => p - q,
        Bin::Mul => p * q,
        Bin::Div => p / q,
        Bin::RSub => q - p,
    };
    let d = (0..c * hw).into_par_iter().map(|i| {
        let (ci, j) = (i / hw, i % hw);
        let p = x.d[if x.c == 1 { j } else { ci * hw + j }];
        let q = y.d[if y.c == 1 { j } else { ci * hw + j }];
        f(p, q)
    });
    Ok(Mat { c, h: x.h, w: x.w, d: d.collect() })
}

/// Each pixel looked up where its motion (2 channels: x, y) points, its
/// neighbours mixed (edges held).
fn warp(x: &Mat, f: &Mat) -> Mat {
    let (h, w) = (x.h, x.w);
    let (fx, fy) = (f.plane(0), f.plane(1));
    let mut o = Mat::new(x.c, h, w);
    o.d.par_chunks_mut(h * w).enumerate().for_each(|(c, dst)| {
        let src = x.plane(c);
        for y in 0..h {
            for xx in 0..w {
                let i = y * w + xx;
                dst[i] = bilinear(src, w, h, xx as f32 + fx[i], y as f32 + fy[i]);
            }
        }
    });
    o
}

fn bilinear(src: &[f32], w: usize, h: usize, sx: f32, sy: f32) -> f32 {
    let (x0f, y0f) = (sx.floor(), sy.floor());
    let (a, b) = (sx - x0f, sy - y0f);
    let cl = |v: f32, n: usize| (v as isize).clamp(0, n as isize - 1) as usize;
    let (x0, x1, y0, y1) = (cl(x0f, w), cl(x0f + 1.0, w), cl(y0f, h), cl(y0f + 1.0, h));
    let top = src[y0 * w + x0] * (1.0 - a) + src[y0 * w + x1] * a;
    let bot = src[y1 * w + x0] * (1.0 - a) + src[y1 * w + x1] * a;
    top * (1.0 - b) + bot * b
}

fn net() -> Result<&'static Net, String> {
    static NET: OnceLock<Result<Net, String>> = OnceLock::new();
    NET.get_or_init(|| Net::load(PARAM, WEIGHTS)).as_ref().map_err(|e| e.clone())
}

/// `img` with the pixels in `mask` filled from around them (a pyramid:
/// each level the average of what's known below it, then filled back up),
/// so burned-in text can be taken out before things are moved.
pub fn fill_in(img: &RgbaImage, mask: &[bool]) -> RgbaImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    // colour × weight, and weight, per level
    let mut levels: Vec<(usize, usize, Vec<[f32; 5]>)> = vec![(w, h, img.pixels().zip(mask).map(|(p, &m)| if m { [0.0; 5] } else { [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32, 1.0] }).collect())];
    while levels.last().unwrap().0 > 1 || levels.last().unwrap().1 > 1 {
        let (lw, lh, v) = levels.last().unwrap();
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut n = vec![[0.0f32; 5]; nw * nh];
        for y in 0..*lh {
            for x in 0..*lw {
                let s = v[y * lw + x];
                let d = &mut n[(y / 2) * nw + x / 2];
                for c in 0..5 {
                    d[c] += s[c];
                }
            }
        }
        levels.push((nw, nh, n));
    }
    // fill each level's gaps from the level above it
    for l in (0..levels.len() - 1).rev() {
        let (lower, upper) = levels.split_at_mut(l + 1);
        let (lw, lh, v) = &mut lower[l];
        let (uw, uh, u) = &upper[0];
        // the level above, smoothly between its pixels (it's filled and
        // evened out already, apart from the top which is still a sum)
        let up = |x: usize, y: usize| -> [f32; 5] {
            let (fx, fy) = (((x as f32 + 0.5) / 2.0 - 0.5).max(0.0), ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0));
            let (x0, y0) = ((fx as usize).min(uw - 1), (fy as usize).min(uh - 1));
            let (x1, y1) = ((x0 + 1).min(uw - 1), (y0 + 1).min(uh - 1));
            let (a, b) = (fx - x0 as f32, fy - y0 as f32);
            let mut o = [0.0f32; 5];
            for (px, py, k) in [(x0, y0, (1.0 - a) * (1.0 - b)), (x1, y0, a * (1.0 - b)), (x0, y1, (1.0 - a) * b), (x1, y1, a * b)] {
                let s = u[py * uw + px];
                if s[4] > 1e-6 {
                    for c in 0..4 {
                        o[c] += s[c] / s[4] * k;
                    }
                    o[4] += k;
                }
            }
            o
        };
        for y in 0..*lh {
            for x in 0..*lw {
                let d = &mut v[y * *lw + x];
                if d[4] < 1e-3 {
                    let s = up(x, y);
                    if s[4] > 1e-6 {
                        *d = [s[0] / s[4], s[1] / s[4], s[2] / s[4], s[3] / s[4], 1.0];
                    }
                } else if d[4] != 1.0 {
                    let k = d[4];
                    *d = [d[0] / k, d[1] / k, d[2] / k, d[3] / k, 1.0];
                }
            }
        }
    }
    let base = &levels[0].2;
    let mut out = img.clone();
    for (i, &m) in mask.iter().enumerate() {
        if m {
            let s = base[i];
            out.put_pixel((i % w) as u32, (i / w) as u32, Rgba([0, 1, 2, 3].map(|c| s[c].round().clamp(0.0, 255.0) as u8)));
        }
    }
    out
}

/// Frame a share `t` of the way from `a` to `b`, with what stays still on
/// screen (`still`, full size) taken out first, so the network doesn't
/// move it with the picture, and laid back on top after.
pub fn between_pinned(a: &RgbaImage, b: &RgbaImage, t: f32, most: u32, still: Option<&[bool]>) -> Result<RgbaImage, String> {
    let Some(m) = still else { return between(a, b, t, most) };
    let (ca, cb) = rayon::join(|| fill_in(a, m), || fill_in(b, m));
    let mut out = between(&ca, &cb, t, most)?;
    let w = out.width() as usize;
    for (k, &st) in m.iter().enumerate() {
        if st {
            let (x, y) = ((k % w) as u32, (k / w) as u32);
            let (pa, pb) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            // how much this is the text itself (unlike what was filled in
            // under it) rather than picture caught in the mask's margin
            let off = |p: [u8; 4], q: [u8; 4]| (0..3).map(|c| (p[c] as f32 - q[c] as f32).abs()).sum::<f32>() / 3.0;
            let text = ((off(pa, ca.get_pixel(x, y).0) + off(pb, cb.get_pixel(x, y).0)) / 2.0 / 30.0).clamp(0.0, 1.0);
            let o = out.get_pixel(x, y).0;
            out.put_pixel(x, y, Rgba([0, 1, 2, 3].map(|c| {
                let pin = pa[c] as f32 * (1.0 - t) + pb[c] as f32 * t;
                (o[c] as f32 * (1.0 - text) + pin * text).round() as u8
            })));
        }
    }
    Ok(out)
}

/// Frame a share `t` of the way from `a` to `b`. The network works on a
/// copy at most `most` pixels on its longer side.
pub fn between(a: &RgbaImage, b: &RgbaImage, t: f32, most: u32) -> Result<RgbaImage, String> {
    let (fw, fh) = a.dimensions();
    let k = (most as f32 / fw.max(fh) as f32).min(1.0);
    let (w, h) = (((fw as f32 * k).round() as usize).max(8), ((fh as f32 * k).round() as usize).max(8));
    // the network wants sides in 32s: the rest is left black, as it was trained
    let (pw, ph) = (w.div_ceil(32) * 32, h.div_ceil(32) * 32);
    let load = |img: &RgbaImage| -> Mat {
        let small = if (w, h) == (fw as usize, fh as usize) { img.clone() } else { image::imageops::resize(img, w as u32, h as u32, image::imageops::FilterType::Triangle) };
        let mut m = Mat::new(3, ph, pw);
        for (x, y, p) in small.enumerate_pixels() {
            for c in 0..3 {
                m.d[c * ph * pw + y as usize * pw + x as usize] = p[c] as f32 / 255.0;
            }
        }
        m
    };
    let (ma, mb) = rayon::join(|| load(a), || load(b));
    let time = Mat { c: 1, h: ph, w: pw, d: vec![t; ph * pw] };
    let got = net()?.run(&[("in0", ma), ("in1", mb), ("in2", time)], &["327", "332"])?;
    let (flow, mask) = (&got[0], &got[1]);
    // the motion and mask at full size; full-size frames moved and mixed
    let (sx, sy) = (w as f32 / fw as f32, h as f32 / fh as f32);
    let at = |m: &Mat, c: usize, x: f32, y: f32| bilinear(m.plane(c), m.w, m.h, (x + 0.5) * sx - 0.5, (y + 0.5) * sy - 0.5);
    let get = |img: &RgbaImage, x: f32, y: f32| -> [f32; 4] {
        let (x0f, y0f) = (x.floor(), y.floor());
        let (ax, ay) = (x - x0f, y - y0f);
        let cl = |v: f32, n: u32| (v as i64).clamp(0, n as i64 - 1) as u32;
        let (x0, x1, y0, y1) = (cl(x0f, fw), cl(x0f + 1.0, fw), cl(y0f, fh), cl(y0f + 1.0, fh));
        let p = |x: u32, y: u32| img.get_pixel(x, y).0;
        [0, 1, 2, 3].map(|c| (p(x0, y0)[c] as f32 * (1.0 - ax) + p(x1, y0)[c] as f32 * ax) * (1.0 - ay) + (p(x0, y1)[c] as f32 * (1.0 - ax) + p(x1, y1)[c] as f32 * ax) * ay)
    };
    let rows: Vec<Vec<Rgba<u8>>> = (0..fh)
        .into_par_iter()
        .map(|y| {
            (0..fw)
                .map(|x| {
                    let (xf, yf) = (x as f32, y as f32);
                    let m = at(mask, 0, xf, yf);
                    let (ax, ay) = (at(flow, 0, xf, yf) / sx, at(flow, 1, xf, yf) / sy);
                    let (bx, by) = (at(flow, 2, xf, yf) / sx, at(flow, 3, xf, yf) / sy);
                    let ca = get(a, xf + ax, yf + ay);
                    let cb = get(b, xf + bx, yf + by);
                    Rgba([0, 1, 2, 3].map(|c| (ca[c] * m + cb[c] * (1.0 - m)).round().clamp(0.0, 255.0) as u8))
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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_in_takes_from_around() {
        let img = RgbaImage::from_fn(8, 8, |x, _| if x < 4 { Rgba([200, 0, 0, 255]) } else { Rgba([0, 0, 200, 255]) });
        let mask: Vec<bool> = (0..64).map(|i| i % 8 == 1 && i / 8 == 3).collect();
        let mut marked = img.clone();
        marked.put_pixel(1, 3, Rgba([0, 255, 0, 255]));
        let f = fill_in(&marked, &mask);
        assert_eq!(f.get_pixel(1, 3)[1], 0, "the mark is gone");
        assert!(f.get_pixel(1, 3)[0] > 150, "red from around it: {:?}", f.get_pixel(1, 3));
        assert_eq!(f.get_pixel(5, 5), img.get_pixel(5, 5));
    }

    #[test]
    fn model_loads_whole() {
        let n = net().expect("loads");
        assert_eq!(n.layers.len(), 215);
    }

    /// A known small case for each layer kind against working by hand.
    #[test]
    fn layers_do_what_they_say() {
        // 3×3 conv, one in, one out, pad 1: a centre-only kernel copies
        let x = Mat { c: 1, h: 3, w: 3, d: (1..=9).map(|v| v as f32).collect() };
        let mut taps = vec![vec![0.0]; 9];
        taps[4] = vec![1.0];
        let y = conv(&x, 1, 3, 1, 1, &taps, &[0.5], None);
        assert_eq!(y.d, x.d.iter().map(|v| v + 0.5).collect::<Vec<_>>());
        // the top-left tap takes the pixel up and left
        let mut taps = vec![vec![0.0]; 9];
        taps[0] = vec![1.0];
        let y = conv(&x, 1, 3, 1, 1, &taps, &[0.0], None);
        assert_eq!(y.d[4], 1.0);
        // stride 2
        let y = conv(&x, 1, 3, 2, 1, &{
            let mut t = vec![vec![0.0]; 9];
            t[4] = vec![1.0];
            t
        }, &[0.0], None);
        assert_eq!((y.h, y.w, y.d.clone()), (2, 2, vec![1.0, 3.0, 7.0, 9.0]));
        // shuffle: 4 channels of 1 pixel become one 2×2
        let s = shuffle(&Mat { c: 4, h: 1, w: 1, d: vec![1.0, 2.0, 3.0, 4.0] }, 2);
        assert_eq!(s.d, vec![1.0, 2.0, 3.0, 4.0]);
        // resize up 2× (PyTorch gives 1, 1.25, 1.75, 2 for 1,2)
        let r = resize(&Mat { c: 1, h: 1, w: 2, d: vec![1.0, 2.0] }, 1, 4);
        assert_eq!(r.d, vec![1.0, 1.25, 1.75, 2.0]);
        // a transposed conv, 2× up, one tap: spreads to its spot
        let mut taps = vec![vec![0.0]; 16];
        taps[5] = vec![1.0];
        let d = deconv(&Mat { c: 1, h: 2, w: 2, d: vec![1.0, 2.0, 3.0, 4.0] }, 1, 4, 2, 1, &taps, &[0.0]);
        assert_eq!((d.h, d.w), (4, 4));
        assert_eq!(d.d, vec![1.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // warp by a whole pixel right
        let wv = warp(&Mat { c: 1, h: 1, w: 3, d: vec![1.0, 2.0, 3.0] }, &Mat { c: 2, h: 1, w: 3, d: vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0] });
        assert_eq!(wv.d, vec![2.0, 3.0, 3.0]);
    }

    /// Frames i and i+2 of a file; the made middle against the real one.
    /// FILE=… FRAMES="0;4;20" cargo test --release rife_triplets -- --ignored --nocapture
    #[test]
    #[ignore]
    fn rife_triplets() {
        let a = crate::io::load_animation(std::path::Path::new(&std::env::var("FILE").unwrap())).unwrap();
        let most: u32 = std::env::var("MOST").ok().and_then(|v| v.parse().ok()).unwrap_or(RUN_SIZE);
        let psnr = |p: &RgbaImage, q: &RgbaImage| {
            let mse: f64 = p.pixels().zip(q.pixels()).map(|(x, y)| (0..3).map(|c| (x[c] as f64 - y[c] as f64).powi(2)).sum::<f64>()).sum::<f64>() / (p.width() as f64 * p.height() as f64 * 3.0);
            10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
        };
        let mut sheet = Vec::new();
        for i in std::env::var("FRAMES").unwrap().split(';').map(|v| v.trim().parse::<usize>().unwrap()) {
            let (f0, f1, f2) = if let Ok(d) = std::env::var("SLIDE") {
                // one frame, cropped sliding along: the middle is known exactly
                let d: u32 = d.parse().unwrap();
                let f = a.frames[i].image.get();
                let (w, h) = (f.width() - 2 * d - 2, f.height() - d - 2);
                let crop = |o: u32| std::sync::Arc::new(image::imageops::crop_imm(&*f, o, o / 2, w, h).to_image());
                (crop(0), crop(d), crop(2 * d))
            } else {
                (a.frames[i].image.get(), a.frames[i + 1].image.get(), a.frames[i + 2].image.get())
            };
            let t0 = std::time::Instant::now();
            let mid = between(&f0, &f2, 0.5, most).unwrap();
            let ms = t0.elapsed().as_millis();
            let fade = crate::ops::lerp_image(&f0, &f2, 0.5);
            eprintln!("{i}: rife {:.2} dB, fade {:.2} dB, {ms} ms", psnr(&mid, &f1), psnr(&fade, &f1));
            sheet.push(((*f0).clone(), fade, mid, (*f1).clone(), (*f2).clone()));
        }
        if let Ok(out) = std::env::var("OUT") {
            let (w, h) = sheet[0].0.dimensions();
            let mut img = RgbaImage::new(w * 5, h * sheet.len() as u32);
            for (r, row) in sheet.iter().enumerate() {
                for (c, im) in [&row.0, &row.1, &row.2, &row.3, &row.4].into_iter().enumerate() {
                    image::imageops::overlay(&mut img, im, (c as u32 * w) as i64, (r as u32 * h) as i64);
                }
            }
            image::imageops::resize(&img, img.width() / 3, img.height() / 3, image::imageops::FilterType::Triangle).save(out).unwrap();
        }
    }
}
