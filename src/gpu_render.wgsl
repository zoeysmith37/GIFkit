// GIFkit's frame pipeline on the graphics card. Everything here works in
// premultiplied alpha in gamma space (as the CPU path and egui do), and
// samples by hand with textureLoad so edges and filtering match the CPU.

// ---- frame geometry: crop, resize, flips and turns, padding, in one pass

struct Geo {
    inv0: vec4<f32>,   // out px -> source px: a b c d
    inv1: vec4<f32>,   // tx ty, nearest (1/0), unused
    crop: vec4<f32>,   // the picture in source px: x0 y0 x1 y1
    can0: vec4<f32>,   // out px -> the resized canvas before a free turn: a b c d
    can1: vec4<f32>,   // tx ty canvas_w canvas_h
    inner: vec4<f32>,  // inside the padding, out px: x0 y0 x1 y1
    pad_bg: vec4<f32>, // straight rgba 0..1
    rot_bg: vec4<f32>,
    foot: vec4<f32>,   // source px under one output px (x, y)
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> g: Geo;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn tap(t: texture_2d<f32>, p: vec2<i32>, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
    return premul(textureLoad(t, clamp(p, lo, hi), 0));
}

// bilinear between pixel centres, `uv` in continuous pixel units
fn bilinear(t: texture_2d<f32>, uv: vec2<f32>, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
    let s = uv - vec2<f32>(0.5);
    let f = floor(s);
    let fr = s - f;
    let i = vec2<i32>(f);
    let a = tap(t, i, lo, hi);
    let b = tap(t, i + vec2<i32>(1, 0), lo, hi);
    let c = tap(t, i + vec2<i32>(0, 1), lo, hi);
    let d = tap(t, i + vec2<i32>(1, 1), lo, hi);
    return mix(mix(a, b, fr.x), mix(c, d, fr.x), fr.y);
}

// shrinking takes several bilinear taps across the footprint, so detail
// averages out instead of flickering
fn area(t: texture_2d<f32>, uv: vec2<f32>, foot: vec2<f32>, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
    let n = clamp(ceil(foot), vec2<f32>(1.0), vec2<f32>(6.0));
    var acc = vec4<f32>(0.0);
    for (var y = 0; y < i32(n.y); y = y + 1) {
        for (var x = 0; x < i32(n.x); x = x + 1) {
            let o = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) / n - vec2<f32>(0.5);
            acc = acc + bilinear(t, uv + o * foot, lo, hi);
        }
    }
    return acc / (n.x * n.y);
}

@fragment
fn fs_geo(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let o = pos.xy;
    if (o.x < g.inner.x || o.x >= g.inner.z || o.y < g.inner.y || o.y >= g.inner.w) {
        return premul(g.pad_bg);
    }
    // the corners a free turn opens up, or a fitted picture's bars
    // a free turn fills the whole canvas with its background first, as
    // the CPU does (the picture's own see-through parts show it too)
    let bg = premul(g.rot_bg);
    let uv = vec2<f32>(g.inv0.x * o.x + g.inv0.y * o.y + g.inv1.x, g.inv0.z * o.x + g.inv0.w * o.y + g.inv1.y);
    // how much of the pixel is on the picture: soft edges when it's turned
    let cov2 = clamp(min(uv - g.crop.xy, g.crop.zw - uv) / max(g.foot.xy, vec2<f32>(1.0)) + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    let cov = cov2.x * cov2.y;
    if (cov <= 0.0) {
        return bg;
    }
    let lo = vec2<i32>(floor(g.crop.xy));
    let hi = vec2<i32>(ceil(g.crop.zw)) - vec2<i32>(1);
    var c: vec4<f32>;
    if (g.inv1.z > 0.5) {
        c = tap(src, vec2<i32>(floor(uv)), lo, hi);
    } else {
        c = area(src, uv, g.foot.xy, lo, hi);
    }
    c = c * cov;
    return c + bg * (1.0 - c.a);
}

// ---- layers: a picture (or a solid colour) drawn through an affine map

struct Quad {
    m0: vec4<f32>,   // sprite px -> out px: a b c d
    m1: vec4<f32>,   // tx ty sprite_w sprite_h
    i0: vec4<f32>,   // out px -> sprite px: a b c d
    i1: vec4<f32>,   // tx ty out_w out_h
    col: vec4<f32>,  // solid colour, straight rgba
    opts: vec4<f32>, // opacity, solid (1/0), foot x, foot y
};

@group(0) @binding(0) var img: texture_2d<f32>;
@group(0) @binding(1) var<uniform> q: Quad;

@vertex
fn vs_quad(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // the sprite's corners, one pixel out so soft edges aren't cut
    let k = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let s = k[i] * (q.m1.zw + vec2<f32>(2.0)) - vec2<f32>(1.0);
    let p = vec2<f32>(q.m0.x * s.x + q.m0.y * s.y + q.m1.x, q.m0.z * s.x + q.m0.w * s.y + q.m1.y);
    return vec4<f32>(p.x / q.i1.z * 2.0 - 1.0, 1.0 - p.y / q.i1.w * 2.0, 0.0, 1.0);
}

@fragment
fn fs_quad(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let o = pos.xy;
    let uv = vec2<f32>(q.i0.x * o.x + q.i0.y * o.y + q.i1.x, q.i0.z * o.x + q.i0.w * o.y + q.i1.y);
    let size = q.m1.zw;
    let foot = max(q.opts.zw, vec2<f32>(1.0));
    let cov2 = clamp(min(uv, size - uv) / foot + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    let cov = cov2.x * cov2.y;
    if (cov <= 0.0) {
        discard;
    }
    var c: vec4<f32>;
    if (q.opts.y > 0.5) {
        c = premul(q.col);
    } else {
        let hi = vec2<i32>(size) - vec2<i32>(1);
        c = area(img, uv, q.opts.zw, vec2<i32>(0), hi);
    }
    return c * (cov * q.opts.x);
}

// ---- resizing, as the CPU's image crate does it: one direction at a time,
// straight (not premultiplied) channels, the same kernels and edges

struct Rs {
    a: vec4<f32>, // ratio, filter scale, support, kernel (0 nearest 1 triangle 2 catmull-rom 3 lanczos3)
    b: vec4<f32>, // source offset along the axis, source length, other-axis offset, vertical (1/0)
};

@group(0) @binding(0) var rs_src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> rs: Rs;

fn sinc(t: f32) -> f32 {
    if (t == 0.0) {
        return 1.0;
    }
    let a = t * 3.14159265;
    return sin(a) / a;
}

fn kernel(x: f32, k: i32) -> f32 {
    let a = abs(x);
    if (k == 0) {
        return 1.0;
    }
    if (k == 1) {
        return max(1.0 - a, 0.0);
    }
    if (k == 2) {
        // Mitchell–Netravali with b = 0, c = 0.5
        if (a < 1.0) {
            return (9.0 * a * a * a - 15.0 * a * a + 6.0) / 6.0;
        }
        if (a < 2.0) {
            return (-3.0 * a * a * a + 15.0 * a * a - 24.0 * a + 12.0) / 6.0;
        }
        return 0.0;
    }
    if (a < 3.0) {
        return sinc(x) * sinc(x / 3.0);
    }
    return 0.0;
}

fn resize_axis(pos: vec2<f32>) -> vec4<f32> {
    let vertical = rs.b.w > 0.5;
    let out_i = select(pos.x, pos.y, vertical) - 0.5;
    let ratio = rs.a.x;
    let sratio = rs.a.y;
    let support = rs.a.z;
    let k = i32(rs.a.w);
    let len = i32(rs.b.y);
    let input = (out_i + 0.5) * ratio;
    let left = clamp(i32(floor(input - support)), 0, len - 1);
    let right = clamp(i32(ceil(input + support)), left + 1, len);
    let centre = input - 0.5;
    var acc = vec4<f32>(0.0);
    var sum = 0.0;
    for (var i = left; i < right; i = i + 1) {
        let w = kernel((f32(i) - centre) / sratio, k);
        var p: vec2<i32>;
        if (vertical) {
            p = vec2<i32>(i32(pos.x - 0.5) + i32(rs.b.z), i + i32(rs.b.x));
        } else {
            p = vec2<i32>(i + i32(rs.b.x), i32(pos.y - 0.5));
        }
        acc = acc + textureLoad(rs_src, p, 0) * w;
        sum = sum + w;
    }
    return acc / sum;
}

@fragment
fn fs_resize_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // into a float texture, unclamped, as the CPU keeps it between the passes
    return resize_axis(pos.xy) * 255.0;
}

@fragment
fn fs_resize_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return clamp(resize_axis(pos.xy) / 255.0, vec4<f32>(0.0), vec4<f32>(1.0));
}
