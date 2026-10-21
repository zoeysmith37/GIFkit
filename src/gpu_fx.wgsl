// Effects, censors and the AI shield on the graphics card. Each pass works
// on straight (not premultiplied) 8-bit pixels in 0..255 units, the way
// the CPU code in effects.rs, booth.rs and motion.rs does, and rounds the
// same way between steps so the two agree to a level or so.

struct U {
    p: array<vec4<f32>, 24>,
};

@group(0) @binding(0) var t0: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;
@group(0) @binding(2) var t1: texture_2d<f32>;
@group(0) @binding(3) var t2: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> cuts: array<u32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// ---- helpers

// Rust's f32::round: halves away from zero (WGSL's round() goes to even)
fn rnd(v: f32) -> f32 {
    return sign(v) * floor(abs(v) + 0.5);
}

fn rnd4(v: vec4<f32>) -> vec4<f32> {
    return sign(v) * floor(abs(v) + vec4<f32>(0.5));
}

fn c8(v: f32) -> f32 {
    return clamp(rnd(v), 0.0, 255.0);
}

fn c8v(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c8(v.x), c8(v.y), c8(v.z));
}

// Rust's fract keeps the sign
fn fract_rs(x: f32) -> f32 {
    return x - trunc(x);
}

fn dims(t: texture_2d<f32>) -> vec2<i32> {
    return vec2<i32>(textureDimensions(t));
}

// a pixel in 0..255 units, edges clamped
fn ld(t: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let d = dims(t) - vec2<i32>(1);
    return rnd4(textureLoad(t, clamp(p, vec2<i32>(0), d), 0) * 255.0);
}

// a float texture (kept unrounded between passes)
fn ldf(t: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let d = dims(t) - vec2<i32>(1);
    return textureLoad(t, clamp(p, vec2<i32>(0), d), 0);
}

fn out8(c: vec4<f32>) -> vec4<f32> {
    return clamp(rnd4(c), vec4<f32>(0.0), vec4<f32>(255.0)) / 255.0;
}

fn luma(c: vec4<f32>) -> f32 {
    return 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
}

fn smoothstep_rs(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    return a + (b - a) * t;
}

// ops::blend_over on straight pixels
fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    let sa = src.a / 255.0;
    if (sa >= 1.0) {
        return src;
    }
    if (sa <= 0.0) {
        return dst;
    }
    let da = dst.a / 255.0;
    let oa = sa + da * (1.0 - sa);
    let v = (src.rgb * sa + dst.rgb * da * (1.0 - sa)) / oa;
    return vec4<f32>(c8v(v), rnd(oa * 255.0));
}

// ops::sample_bilinear: premultiplied bilinear, see-through outside
fn sample_bl(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let d = dims(t);
    let w = f32(d.x);
    let h = f32(d.y);
    if (x < -1.0 || y < -1.0 || x > w || y > h) {
        return vec4<f32>(0.0);
    }
    let x0 = i32(floor(x));
    let y0 = i32(floor(y));
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    var q: array<vec4<f32>, 4>;
    for (var k = 0; k < 4; k = k + 1) {
        let xx = x0 + (k & 1);
        let yy = y0 + (k >> 1u);
        if (xx < 0 || yy < 0 || xx >= d.x || yy >= d.y) {
            q[k] = vec4<f32>(0.0);
        } else {
            let p = rnd4(textureLoad(t, vec2<i32>(xx, yy), 0) * 255.0);
            let a = p.a / 255.0;
            q[k] = vec4<f32>(p.rgb * a, p.a);
        }
    }
    let o = q[0] * (1.0 - fx) * (1.0 - fy) + q[1] * fx * (1.0 - fy) + q[2] * (1.0 - fx) * fy + q[3] * fx * fy;
    if (o.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let ia = 255.0 / o.a;
    return vec4<f32>(c8v(o.rgb * ia), c8(o.a));
}

fn sample_clamped(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let d = vec2<f32>(dims(t) - vec2<i32>(1));
    return sample_bl(t, clamp(x, 0.0, d.x), clamp(y, 0.0, d.y));
}

// motion.rs's bilinear: straight channels, nothing outside half a pixel
fn bilinear_m(t: texture_2d<f32>, x: f32, y: f32, ok: ptr<function, bool>) -> vec4<f32> {
    let d = dims(t);
    let w = f32(d.x);
    let h = f32(d.y);
    if (x < -0.5 || y < -0.5 || x > w - 0.5 || y > h - 0.5) {
        *ok = false;
        return vec4<f32>(0.0);
    }
    *ok = true;
    let x0 = i32(floor(x));
    let y0 = i32(floor(y));
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    let a = ld(t, vec2<i32>(x0, y0));
    let b = ld(t, vec2<i32>(x0 + 1, y0));
    let c = ld(t, vec2<i32>(x0, y0 + 1));
    let e = ld(t, vec2<i32>(x0 + 1, y0 + 1));
    let top = a * (1.0 - fx) + b * fx;
    let bot = c * (1.0 - fx) + e * fx;
    return rnd4(top * (1.0 - fy) + bot * fy);
}

fn hue2rgb(p: f32, q: f32, t0_: f32) -> f32 {
    var t = t0_;
    if (t < 0.0) {
        t = t + 1.0;
    }
    if (t > 1.0) {
        t = t - 1.0;
    }
    if (t < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * t;
    }
    if (t < 0.5) {
        return q;
    }
    if (t < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

fn modulate(c: vec3<f32>, dh: f32, sm: f32, lm: f32) -> vec3<f32> {
    let r = c.r / 255.0;
    let g = c.g / 255.0;
    let b = c.b / 255.0;
    let mx = max(max(r, g), b);
    let mn = min(min(r, g), b);
    let l = (mx + mn) / 2.0;
    var h = 0.0;
    var s = 0.0;
    if (abs(mx - mn) >= 1e-6) {
        let d = mx - mn;
        if (l > 0.5) {
            s = d / (2.0 - mx - mn);
        } else {
            s = d / (mx + mn);
        }
        if (mx == r) {
            h = (g - b) / d + select(0.0, 6.0, g < b);
        } else if (mx == g) {
            h = (b - r) / d + 2.0;
        } else {
            h = (r - g) / d + 4.0;
        }
        h = h / 6.0;
    }
    let hh0 = h + dh;
    let hh = hh0 - floor(hh0);
    let ss = clamp(s * sm, 0.0, 1.0);
    let ll = clamp(l * lm, 0.0, 1.0);
    if (ss <= 0.0) {
        return vec3<f32>(ll) * 255.0;
    }
    var q = 0.0;
    if (ll < 0.5) {
        q = ll * (1.0 + ss);
    } else {
        q = ll + ss - ll * ss;
    }
    let p = 2.0 * ll - q;
    return vec3<f32>(hue2rgb(p, q, hh + 1.0 / 3.0), hue2rgb(p, q, hh), hue2rgb(p, q, hh - 1.0 / 3.0)) * 255.0;
}

fn colour_dist(c: vec3<f32>, t: vec3<f32>) -> f32 {
    let d = c - t;
    return sqrt(d.x * d.x + d.y * d.y + d.z * d.z);
}

// ---- 64-bit maths for the noise hash (lo, hi)

fn mul_wide(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    return vec2<u32>((p00 & 0xffffu) | (mid << 16u), p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u));
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let w = mul_wide(a.x, b.x);
    return vec2<u32>(w.x, w.y + a.x * b.y + a.y * b.x);
}

fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2<u32>((a.x >> n) | (a.y << (32u - n)), a.y >> n);
}

fn hash3(x: u32, y: u32, z: u32) -> u32 {
    var h = (x * 0x8da6b343u) ^ (y * 0xd8163841u) ^ (z * 0xcb1ab31fu);
    h = h ^ (h >> 13u);
    h = h * 0x5bd1e995u;
    return h ^ (h >> 15u);
}

const BAYER8 = array<u32, 64>(
    0u, 32u, 8u, 40u, 2u, 34u, 10u, 42u,
    48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u,
    12u, 44u, 4u, 36u, 14u, 46u, 6u, 38u,
    60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
    3u, 35u, 11u, 43u, 1u, 33u, 9u, 41u,
    51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u,
    15u, 47u, 7u, 39u, 13u, 45u, 5u, 37u,
    63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u,
);

// booth.rs's Sobel edge strength on brightness (0..1 units)
fn lum01(t: texture_2d<f32>, p: vec2<i32>) -> f32 {
    return luma(ld(t, p)) / 255.0;
}

fn edge(t: texture_2d<f32>, p: vec2<i32>) -> f32 {
    let gx = lum01(t, p + vec2<i32>(1, -1)) + 2.0 * lum01(t, p + vec2<i32>(1, 0)) + lum01(t, p + vec2<i32>(1, 1)) - lum01(t, p + vec2<i32>(-1, -1)) - 2.0 * lum01(t, p + vec2<i32>(-1, 0)) - lum01(t, p + vec2<i32>(-1, 1));
    let gy = lum01(t, p + vec2<i32>(-1, 1)) + 2.0 * lum01(t, p + vec2<i32>(0, 1)) + lum01(t, p + vec2<i32>(1, 1)) - lum01(t, p + vec2<i32>(-1, -1)) - 2.0 * lum01(t, p + vec2<i32>(0, -1)) - lum01(t, p + vec2<i32>(1, -1));
    return sqrt(gx * gx + gy * gy);
}

// where a distortion reads from (effects::warp's callers)
fn warp_from(kind: i32, x: f32, y: f32) -> vec2<f32> {
    let a = u.p[1];
    let b = u.p[2];
    if (kind == 0) {
        // swirl: cx cy rad ang
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d >= a.z) {
            return vec2<f32>(x, y);
        }
        let f = 1.0 - d / a.z;
        let t = a.w * f * f;
        let s = sin(t);
        let c = cos(t);
        return vec2<f32>(a.x + dx * c - dy * s, a.y + dx * s + dy * c);
    }
    if (kind == 1) {
        // implode: cx cy rad amount
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d >= a.z || d < 1e-3) {
            return vec2<f32>(x, y);
        }
        let f = pow(sin(3.14159265 * d / a.z / 2.0), -a.w);
        return vec2<f32>(a.x + dx * f, a.y + dy * f);
    }
    if (kind == 2) {
        // wave: amp len phase
        return vec2<f32>(x, y + a.x * sin((2.0 * 3.14159265 * x / a.y) + a.z));
    }
    if (kind == 3) {
        // bulge, dent, fish eye: cx cy r0 k
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d >= a.z || d < 1e-3) {
            return vec2<f32>(x, y);
        }
        let t = d / a.z;
        let s = pow(t, 1.0 + a.w) / t;
        return vec2<f32>(a.x + dx * s, a.y + dy * s);
    }
    if (kind == 4) {
        // twirl: cx cy r0 ang
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d >= a.z) {
            return vec2<f32>(x, y);
        }
        let f = 1.0 - d / a.z;
        let t = a.w * f * f;
        let s = sin(t);
        let c = cos(t);
        return vec2<f32>(a.x + dx * c - dy * s, a.y + dx * s + dy * c);
    }
    if (kind == 5) {
        // squeeze, stretch: cx power width
        let dx = x - a.x;
        var span = 0.0;
        if (dx < 0.0) {
            span = max(a.x, 1.0);
        } else {
            span = max(a.z - a.x, 1.0);
        }
        let t = min(abs(dx) / span, 1.0);
        var sg = 1.0;
        if (dx < 0.0) {
            sg = -1.0;
        }
        return vec2<f32>(a.x + sg * pow(t, a.y) * span, y);
    }
    if (kind == 6) {
        // light tunnel: cx cy r0 squash
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d <= a.z) {
            return vec2<f32>(x, y);
        }
        let s = (a.z + (d - a.z) * a.w) / d;
        return vec2<f32>(a.x + dx * s, a.y + dy * s);
    }
    if (kind == 9) {
        // a tunnel the other way: cx cy r0 squash
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        if (d >= a.z || d < 1e-3) {
            return vec2<f32>(x, y);
        }
        let s = (a.z - (a.z - d) * a.w) / d;
        return vec2<f32>(a.x + dx * s, a.y + dy * s);
    }
    if (kind == 7) {
        // kaleidoscope: cx cy seg reach, the slices turned by b.x
        let dx = x - a.x;
        let dy = y - a.y;
        let d = sqrt(dx * dx + dy * dy);
        let seg = a.z;
        let ang = atan2(dy, dx) + 3.14159265 + b.x;
        var t = ang - seg * floor(ang / seg);
        if (t > seg / 2.0) {
            t = seg - t;
        }
        t = t - seg / 4.0;
        let q = d / a.w;
        let m = q - 2.0 * floor(q / 2.0);
        var dd = m;
        if (m > 1.0) {
            dd = 2.0 - m;
        }
        dd = dd * a.w;
        return vec2<f32>(a.x + dd * cos(t), a.y + dd * sin(t));
    }
    // the AI shield's jitter: z % 97, z % 89
    let ja = (sin(x * 0.37 + y * 0.11 + a.x) + cos(y * 0.29 - x * 0.07)) * 0.25;
    let jb = (sin(y * 0.41 - x * 0.13 + a.y) + cos(x * 0.23 + y * 0.05)) * 0.25;
    return vec2<f32>(x + ja, y + jb);
}

fn ramp_thermal(t: f32) -> vec3<f32> {
    var pos = array<f32, 7>(0.0, 0.2, 0.4, 0.6, 0.78, 0.92, 1.0);
    var col = array<vec3<f32>, 7>(vec3<f32>(10.0, 0.0, 40.0), vec3<f32>(40.0, 0.0, 160.0), vec3<f32>(170.0, 0.0, 170.0), vec3<f32>(240.0, 30.0, 30.0), vec3<f32>(255.0, 150.0, 0.0), vec3<f32>(255.0, 240.0, 60.0), vec3<f32>(255.0, 255.0, 255.0));
    let tt = clamp(t, 0.0, 1.0);
    for (var i = 0; i < 6; i = i + 1) {
        if (tt <= pos[i + 1]) {
            let k = clamp((tt - pos[i]) / max(pos[i + 1] - pos[i], 1e-6), 0.0, 1.0);
            return col[i] + (col[i + 1] - col[i]) * k;
        }
    }
    return col[6];
}

fn pop_colour(q: i32, k: i32) -> vec3<f32> {
    var pals = array<vec3<f32>, 12>(
        vec3<f32>(230.0, 60.0, 30.0), vec3<f32>(120.0, 150.0, 140.0), vec3<f32>(250.0, 230.0, 40.0),
        vec3<f32>(70.0, 170.0, 60.0), vec3<f32>(230.0, 140.0, 210.0), vec3<f32>(245.0, 245.0, 190.0),
        vec3<f32>(30.0, 35.0, 80.0), vec3<f32>(230.0, 120.0, 100.0), vec3<f32>(150.0, 200.0, 240.0),
        vec3<f32>(30.0, 70.0, 220.0), vec3<f32>(240.0, 120.0, 70.0), vec3<f32>(210.0, 240.0, 120.0),
    );
    return pals[q * 3 + k];
}

fn pencil_line(v: f32, j: f32, period: f32) -> f32 {
    let t = fract_rs((v + j) / period);
    return clamp(1.0 - abs(t - 0.5) * 4.0, 0.0, 1.0);
}

// how much of an intro / outro mask shows at (x, y), motion::transition
fn cover(kind: i32, x: f32, y: f32) -> f32 {
    let a = u.p[1];
    let b = u.p[2];
    if (kind == 0) {
        return a.x;
    }
    if (kind == 1) {
        return (a.x - x) / a.y;
    }
    if (kind == 2) {
        return (a.x - (a.z - x)) / a.y;
    }
    if (kind == 3) {
        return (a.x - y) / a.y;
    }
    if (kind == 4) {
        return (a.x - (a.w - y)) / a.y;
    }
    if (kind == 5) {
        let dx = x - b.x;
        let dy = y - b.y;
        return (a.x - sqrt(dx * dx + dy * dy)) / a.y;
    }
    // blinds: open slat
    return (a.x - (y % a.y)) / 1.5;
}

@fragment
fn fs_fx(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(pos.xy);
    let x = f32(ip.x);
    let y = f32(ip.y);
    let op = i32(u.p[0].x);
    let osz = u.p[0].zw; // output size
    let a = u.p[1];
    let b = u.p[2];
    let c = u.p[3];
    switch op {
        // 1 premultiplied (the frame so far) -> straight
        case 1: {
            let q = rnd4(textureLoad(t0, ip, 0) * 255.0);
            if (q.a <= 0.0) {
                return vec4<f32>(0.0);
            }
            let qa = u32(q.a);
            let un = (vec3<u32>(q.rgb) * 255u + vec3<u32>(qa / 2u)) / vec3<u32>(qa);
            return vec4<f32>(min(vec3<f32>(un), vec3<f32>(255.0)), q.a) / 255.0;
        }
        // 2 straight -> premultiplied (for the screen)
        case 2: {
            let q = ld(t0, ip);
            return vec4<f32>(rnd4(vec4<f32>(q.rgb * q.a / 255.0, q.a))) / 255.0;
        }
        // 3 modulate: dh sm lm
        case 3: {
            let q = ld(t0, ip);
            return out8(vec4<f32>(modulate(q.rgb, a.x, a.y, a.z), q.a));
        }
        // 4 brightness / contrast: cf b
        case 4: {
            let q = ld(t0, ip);
            return out8(vec4<f32>((q.rgb - vec3<f32>(128.0)) * a.x + vec3<f32>(128.0 + a.y), q.a));
        }
        // 5 colorize: rgb amount
        case 5: {
            let q = ld(t0, ip);
            return out8(vec4<f32>(q.rgb * (1.0 - a.w) + a.rgb * a.w, q.a));
        }
        // 6 gamma: 1/gamma
        case 6: {
            let q = ld(t0, ip);
            return out8(vec4<f32>(255.0 * pow(q.rgb / 255.0, vec3<f32>(a.x)), q.a));
        }
        // 7 vignette: strength rgb
        case 7: {
            let q = ld(t0, ip);
            let cx = osz.x / 2.0;
            let cy = osz.y / 2.0;
            let maxd = sqrt(cx * cx + cy * cy);
            let d = clamp(sqrt((x - cx) * (x - cx) + (y - cy) * (y - cy)) / maxd, 0.0, 1.0);
            let t = clamp(smoothstep_rs(0.45, 1.0, d) * a.x, 0.0, 1.0);
            return out8(vec4<f32>(q.rgb * (1.0 - t) + a.yzw * t, q.a));
        }
        // 8 grayscale
        case 8: {
            let q = ld(t0, ip);
            return out8(vec4<f32>(vec3<f32>(luma(q)), q.a));
        }
        // 9 sepia
        case 9: {
            let q = ld(t0, ip);
            let r = q.r * 0.393 + q.g * 0.769 + q.b * 0.189;
            let g = q.r * 0.349 + q.g * 0.686 + q.b * 0.168;
            let bb = q.r * 0.272 + q.g * 0.534 + q.b * 0.131;
            return out8(vec4<f32>(r, g, bb, q.a));
        }
        // 10 monochrome
        case 10: {
            let q = ld(t0, ip);
            return out8(vec4<f32>(vec3<f32>(select(0.0, 255.0, luma(q) >= 128.0)), q.a));
        }
        // 11 (v * m + add) / div per channel: m.rgb in a, add.rgb in b, div.rgb in c
        case 11: {
            let q = ld(t0, ip);
            return out8(vec4<f32>((q.rgb * a.rgb + b.rgb) / c.rgb, q.a));
        }
        // 12 tint: rgb amount
        case 12: {
            let q = ld(t0, ip);
            let l = luma(q) / 255.0;
            let t = a.rgb * l;
            return out8(vec4<f32>(q.rgb * (1.0 - a.w) + t * a.w, q.a));
        }
        // 13 replace a colour with see-through: rgb threshold
        case 13: {
            let q = ld(t0, ip);
            if (colour_dist(q.rgb, a.rgb) <= a.w) {
                return vec4<f32>(q.rgb, 0.0) / 255.0;
            }
            return q / 255.0;
        }
        // 14 removed pixels within r across (for the edge clean-up): r
        case 14: {
            let r = i32(a.x);
            for (var dx = -r; dx <= r; dx = dx + 1) {
                let xx = ip.x + dx;
                if (xx >= 0 && xx < i32(osz.x)) {
                    if (textureLoad(t0, vec2<i32>(xx, ip.y), 0).a == 0.0) {
                        return vec4<f32>(1.0);
                    }
                }
            }
            return vec4<f32>(0.0);
        }
        // 15 edge clean-up: t1 = 14's flags; rgb edge_threshold, r in b.x
        case 15: {
            let q = ld(t0, ip);
            if (q.a == 0.0) {
                return q / 255.0;
            }
            let r = i32(b.x);
            var near = false;
            for (var dy = -r; dy <= r; dy = dy + 1) {
                let yy = ip.y + dy;
                if (yy >= 0 && yy < i32(osz.y)) {
                    if (textureLoad(t1, vec2<i32>(ip.x, yy), 0).r > 0.5) {
                        near = true;
                    }
                }
            }
            if (near) {
                let d = colour_dist(q.rgb, a.rgb);
                if (d <= a.w) {
                    let t = clamp(d / a.w, 0.0, 1.0);
                    var al = c8(q.a * t * t);
                    if (al < 40.0) {
                        al = 0.0;
                    }
                    return vec4<f32>(q.rgb, al) / 255.0;
                }
            }
            return q / 255.0;
        }
        // 16 gaussian blur across into a float texture: size sigma, clamp x0 x1 in b
        case 16: {
            let n = i32(a.x);
            let mean = f32(n / 2);
            var sum = 0.0;
            for (var i = 0; i < n; i = i + 1) {
                let z = (f32(i) - mean) / a.y;
                sum = sum + exp(-0.5 * z * z);
            }
            var acc = vec4<f32>(0.0);
            for (var i = 0; i < n; i = i + 1) {
                let z = (f32(i) - mean) / a.y;
                let wgt = exp(-0.5 * z * z) / sum;
                let xx = clamp(ip.x + i - n / 2, i32(b.x), i32(b.z) - 1);
                acc = acc + ld(t0, vec2<i32>(xx, ip.y)) * wgt;
            }
            return acc;
        }
        // 17 gaussian blur down from the float texture, rounded: size sigma, clamp y0 y1 in b
        case 17: {
            let n = i32(a.x);
            let mean = f32(n / 2);
            var sum = 0.0;
            for (var i = 0; i < n; i = i + 1) {
                let z = (f32(i) - mean) / a.y;
                sum = sum + exp(-0.5 * z * z);
            }
            var acc = vec4<f32>(0.0);
            for (var i = 0; i < n; i = i + 1) {
                let z = (f32(i) - mean) / a.y;
                let wgt = exp(-0.5 * z * z) / sum;
                let yy = clamp(ip.y + i - n / 2, i32(b.y), i32(b.w) - 1);
                acc = acc + ldf(t0, vec2<i32>(ip.x, yy)) * wgt;
            }
            return out8(acc);
        }
        // 18 unsharp mask: t0 the picture, t1 it blurred; threshold
        case 18: {
            let q = ld(t0, ip);
            let bl = ld(t1, ip);
            let diff = q - bl;
            var o = q;
            for (var k = 0; k < 4; k = k + 1) {
                if (abs(diff[k]) > a.x) {
                    o[k] = clamp(q[k] + diff[k], 0.0, 255.0);
                }
            }
            return o / 255.0;
        }
        // 19 oil paint: radius, levels
        case 19: {
            let r = i32(a.x);
            let lv = u32(a.y);
            // per brightness bin: summed colour and count (one array keeps
            // the per-pixel memory down)
            var bins: array<vec4<u32>, 64>;
            for (var dy = -r; dy <= r; dy = dy + 1) {
                for (var dx = -r; dx <= r; dx = dx + 1) {
                    if (dx * dx + dy * dy > r * r) {
                        continue;
                    }
                    let p = ld(t0, ip + vec2<i32>(dx, dy));
                    let bi = min(u32((luma(p) / 256.0) * f32(lv)), lv - 1u);
                    bins[bi] = bins[bi] + vec4<u32>(vec3<u32>(p.rgb), 1u);
                }
            }
            var bi = 0u;
            var bc = 0u;
            for (var k = 0u; k < lv; k = k + 1u) {
                if (bins[k].w >= bc) {
                    bc = bins[k].w;
                    bi = k;
                }
            }
            let q = ld(t0, ip);
            if (bc > 0u) {
                return vec4<f32>(vec3<f32>(bins[bi].xyz / vec3<u32>(bc)), q.a) / 255.0;
            }
            return q / 255.0;
        }
        // 20 emboss: the kernel in p[1..3], in colour (c.y)
        case 20: {
            var k = array<f32, 9>(a.x, a.y, a.z, a.w, b.x, b.y, b.z, b.w, c.x);
            var acc = vec3<f32>(0.0);
            for (var ky = -1; ky <= 1; ky = ky + 1) {
                for (var kx = -1; kx <= 1; kx = kx + 1) {
                    let p = ld(t0, ip + vec2<i32>(kx, ky));
                    var v = vec3<f32>(luma(p));
                    if (c.y > 0.5) {
                        v = p.rgb;
                    }
                    acc = acc + v * k[(ky + 1) * 3 + kx + 1];
                }
            }
            let q = ld(t0, ip);
            return vec4<f32>(c8(acc.x), c8(acc.y), c8(acc.z), q.a) / 255.0;
        }
        // 21 posterize: levels - 1
        case 21: {
            let q = ld(t0, ip);
            let l = a.x;
            return out8(vec4<f32>(rnd4(vec4<f32>(q.rgb / 255.0 * l, 0.0)).rgb / l * 255.0, q.a));
        }
        // 22 solarize: threshold, the dark ones instead
        case 22: {
            let q = ld(t0, ip);
            let hi = q.rgb > vec3<f32>(a.x);
            let o = select(q.rgb, vec3<f32>(255.0) - q.rgb, hi != vec3<bool>(a.y > 0.5));
            return out8(vec4<f32>(o, q.a));
        }
        // 23 noise: amount seed width
        case 23: {
            let q = ld(t0, ip);
            let i = u32(ip.y) * u32(osz.x) + u32(ip.x);
            let seed = vec2<u32>(u32(a.y), 0u);
            let wv = vec2<u32>(u32(a.z), 0u);
            var z = mul64(seed, vec2<u32>(0x7F4A7C15u, 0x9E3779B9u));
            let zi = mul64(vec2<u32>(i, 0u), vec2<u32>(0x1CE4E5B9u, 0xBF58476Du));
            z = z ^ zi ^ wv;
            z = mul64(z ^ shr64(z, 30u), vec2<u32>(0x1CE4E5B9u, 0xBF58476Du));
            z = mul64(z ^ shr64(z, 27u), vec2<u32>(0x133111EBu, 0x94D049BBu));
            z = z ^ shr64(z, 31u);
            let n1 = f32(z.x & 0xffffu) / 65535.0 - 0.5;
            var n2 = f32((z.x >> 16u) & 0xffffu) / 65535.0 - 0.5;
            var n3 = f32(z.y & 0xffffu) / 65535.0 - 0.5;
            if (a.w > 0.5) {
                n2 = n1;
                n3 = n1;
            }
            let am = a.x * 2.0;
            return out8(vec4<f32>(q.r + n1 * am, q.g + n2 * am, q.b + n3 * am, q.a));
        }
        // 24 a distortion: kind, its numbers in p[1], p[2]
        case 24: {
            let s = warp_from(i32(u.p[0].y), x, y);
            return sample_clamped(t0, s.x, s.y) / 255.0;
        }
        // 25 the fish eye's round lens: cx cy r0 amount
        case 25: {
            let q = ld(t0, ip);
            let px = x + 0.5;
            let py = y + 0.5;
            let d = sqrt((px - a.x) * (px - a.x) + (py - a.y) * (py - a.y));
            let m = lerp(1.0, clamp((a.z - d) / 1.5, 0.0, 1.0), a.w);
            return out8(vec4<f32>(q.rgb * m, q.a));
        }
        // 26 ordered dither: levels - 1
        case 26: {
            let q = ld(t0, ip);
            let l = a.x;
            let t = (f32(BAYER8[(u32(ip.y) % 8u) * 8u + u32(ip.x) % 8u]) + 0.5) / 64.0 - 0.5;
            let v = q.rgb / 255.0 * l + vec3<f32>(t);
            return out8(vec4<f32>(clamp(rnd4(vec4<f32>(v, 0.0)).rgb, vec3<f32>(0.0), vec3<f32>(l)) / l * 255.0, q.a));
        }
        // 27 onto a solid background: rgb
        case 27: {
            return over(vec4<f32>(a.rgb, 255.0), ld(t0, ip)) / 255.0;
        }
        // 28 move pixels exactly (flips, quarter turns): in = (a.x X + a.y Y + a.z, a.w X + b.x Y + b.y)
        case 28: {
            let sx = i32(a.x) * ip.x + i32(a.y) * ip.y + i32(a.z);
            let sy = i32(a.w) * ip.x + i32(b.x) * ip.y + i32(b.y);
            return textureLoad(t0, vec2<i32>(sx, sy), 0);
        }
        // 29 free turn, expanding: cos sin cx cy, ncx ncy
        case 29: {
            let dx = x + 0.5 - b.x;
            let dy = y + 0.5 - b.y;
            let sx = dx * a.x + dy * a.y + a.z - 0.5;
            let sy = -dx * a.y + dy * a.x + a.w - 0.5;
            return sample_bl(t0, sx, sy) / 255.0;
        }
        // 30 camera frame: 10 rectangles (x y w h) in p[2..12], REC dot in a (x y r on)
        case 30: {
            var q = ld(t0, ip);
            for (var k = 0; k < 10; k = k + 1) {
                let r = u.p[2 + k];
                if (x >= r.x && x < r.x + r.z && y >= r.y && y < r.y + r.w) {
                    q = over(q, vec4<f32>(255.0, 255.0, 255.0, select(160.0, 230.0, k < 8)));
                }
            }
            if (a.w > 0.5) {
                let dx = x - a.x;
                let dy = y - a.y;
                if (abs(dx) <= a.z && abs(dy) <= a.z && dx * dx + dy * dy <= a.z * a.z) {
                    q = vec4<f32>(230.0, 30.0, 30.0, 255.0);
                }
            }
            return q / 255.0;
        }
        // 31 a picture over the frame at (x, y) with an opacity: t1 the picture
        case 31: {
            let q = ld(t0, ip);
            let sd = dims(t1);
            let sp = ip - vec2<i32>(i32(a.x), i32(a.y));
            if (sp.x < 0 || sp.y < 0 || sp.x >= sd.x || sp.y >= sd.y) {
                return q / 255.0;
            }
            var s = rnd4(textureLoad(t1, sp, 0) * 255.0);
            s.a = rnd(s.a * a.z);
            return over(q, s) / 255.0;
        }
        // 32 fuzzy edge: t1 the jitter for this size, f
        case 32: {
            let q = ld(t0, ip);
            let px = x + 0.5;
            let py = y + 0.5;
            let d = min(min(px, py), min(osz.x - px, osz.y - py));
            let j = textureLoad(t1, ip, 0).r;
            let m = smoothstep_rs(0.0, a.x, d + j);
            return vec4<f32>(q.rgb, c8(q.a * m)) / 255.0;
        }
        // 33 rounded corners: r
        case 33: {
            let q = ld(t0, ip);
            let r = a.x;
            let px = x + 0.5;
            let py = y + 0.5;
            var cx = px;
            if (px < r) {
                cx = r;
            } else if (px > osz.x - r) {
                cx = osz.x - r;
            }
            var cy = py;
            if (py < r) {
                cy = r;
            } else if (py > osz.y - r) {
                cy = osz.y - r;
            }
            let d = sqrt((px - cx) * (px - cx) + (py - cy) * (py - cy));
            let m = clamp(r - d + 0.5, 0.0, 1.0);
            return vec4<f32>(q.rgb, c8(q.a * m)) / 255.0;
        }
        // 34 polaroid: side; the picture is warmed and laid on paper
        case 34: {
            let s = i32(a.x);
            let paper = vec4<f32>(250.0, 250.0, 246.0, 255.0);
            let sp = ip - vec2<i32>(s);
            let sd = dims(t0);
            if (sp.x < 0 || sp.y < 0 || sp.x >= sd.x || sp.y >= sd.y) {
                return paper / 255.0;
            }
            let q = ld(t0, sp);
            let warm = c8v(q.rgb * (1.0 - 0.08) + vec3<f32>(255.0, 235.0, 200.0) * 0.08);
            // image's own blend (imageops::overlay), which truncates
            if (q.a == 0.0) {
                return paper / 255.0;
            }
            if (q.a == 255.0) {
                return vec4<f32>(warm, 255.0) / 255.0;
            }
            let fa = q.a / 255.0;
            let af = 1.0 + fa - fa;
            let o = (warm / 255.0 * fa + paper.rgb / 255.0 * (1.0 - fa)) / af;
            return vec4<f32>(trunc(255.0 * o), trunc(255.0 * af)) / 255.0;
        }
        // 35 border: px rgb
        case 35: {
            let s = i32(a.x);
            let sp = ip - vec2<i32>(s);
            let sd = dims(t0);
            if (sp.x < 0 || sp.y < 0 || sp.x >= sd.x || sp.y >= sd.y) {
                return vec4<f32>(a.yzw, 255.0) / 255.0;
            }
            return textureLoad(t0, sp, 0);
        }
        // 36 motion::place: scale sin cos, dx dy in b, background in c
        case 36: {
            var o = c;
            if (a.x <= 0.001 || (a.w > 0.0 && a.w <= 0.001)) {
                return o / 255.0;
            }
            let cx = osz.x / 2.0;
            let cy = osz.y / 2.0;
            let xx = x + 0.5 - cx - b.x;
            let yy = y + 0.5 - cy - b.y;
            // the height's scale (a.w), the width's when not given
            var ys = a.w;
            if (ys <= 0.001) {
                ys = a.x;
            }
            let sx = (xx * a.z - yy * a.y) / a.x + cx - 0.5;
            let sy = (xx * a.y + yy * a.z) / ys + cy - 0.5;
            var ok = false;
            let v = bilinear_m(t0, sx, sy, &ok);
            if (ok) {
                let al = v.a / 255.0;
                o = vec4<f32>(rnd4(vec4<f32>(v.rgb * al + o.rgb * (1.0 - al), v.a + o.a * (1.0 - al))));
            }
            return o / 255.0;
        }
        // 37 an intro / outro mask: kind, see-through (b.w), numbers in p[1], p[2]
        case 37: {
            let q = ld(t0, ip);
            let k = clamp(cover(i32(u.p[0].y), x + 0.5, y + 0.5), 0.0, 1.0);
            if (k >= 1.0) {
                return q / 255.0;
            }
            if (u.p[3].x > 0.5) {
                return vec4<f32>(q.rgb, rnd(q.a * k)) / 255.0;
            }
            return vec4<f32>(rnd4(vec4<f32>(q.rgb * k, q.a + (255.0 - q.a) * (1.0 - k)))) / 255.0;
        }
        // 38 block averages (pixelate): origin x y, region w h in a, block in b.x
        case 38: {
            let bs = i32(b.x);
            let o = vec2<i32>(i32(a.x), i32(a.y)) + ip * bs;
            let bw = min(bs, i32(a.x) + i32(a.z) - o.x);
            let bh = min(bs, i32(a.y) + i32(a.w) - o.y);
            var acc = vec4<u32>(0u);
            for (var yy = 0; yy < bh; yy = yy + 1) {
                for (var xx = 0; xx < bw; xx = xx + 1) {
                    acc = acc + vec4<u32>(ld(t0, o + vec2<i32>(xx, yy)));
                }
            }
            let n = u32(max(bw * bh, 1));
            return vec4<f32>(acc / vec4<u32>(n)) / 255.0;
        }
        // 39 the blocks, full size: t1 blocks, block in a.x
        case 39: {
            let bs = i32(a.x);
            return textureLoad(t1, ip / bs, 0);
        }
        // 40 rotate hue about the grey axis: rows in a b c
        case 40: {
            let q = ld(t0, ip);
            let v = vec3<f32>(dot(a.xyz, q.rgb), dot(b.xyz, q.rgb), dot(c.xyz, q.rgb));
            return out8(vec4<f32>(v, q.a));
        }
        // 41 glitch: split; bands (y0 y1 shift) in p[2..6]
        case 41: {
            let w = i32(osz.x);
            var shift = 0;
            for (var k = 0; k < 4; k = k + 1) {
                let bd = u.p[2 + k];
                if (y >= bd.x && y < bd.y) {
                    shift = i32(bd.z);
                    break;
                }
            }
            let sp = i32(a.x);
            let gx = ip.x + shift;
            let r = textureLoad(t0, vec2<i32>(((gx - sp) % w + w) % w, ip.y), 0);
            let m = textureLoad(t0, vec2<i32>((gx % w + w) % w, ip.y), 0);
            let bl = textureLoad(t0, vec2<i32>(((gx + sp) % w + w) % w, ip.y), 0);
            return vec4<f32>(r.r, m.g, bl.b, m.a);
        }
        // 42 glow: t1 the soft blur; amount
        case 42: {
            let q = ld(t0, ip);
            let g = ld(t1, ip);
            var o = q;
            for (var k = 0; k < 3; k = k + 1) {
                let s = q[k] / 255.0 * (1.0 + 0.06 * a.x);
                let gg = g[k] / 255.0;
                let screen = 1.0 - (1.0 - s) * (1.0 - gg);
                o[k] = c8(lerp(s, screen, a.x) * 255.0);
            }
            return o / 255.0;
        }
        // 43 comic book (t0 already softened): threshold
        case 43: {
            let q = ld(t0, ip);
            if (edge(t0, ip) > a.x) {
                return vec4<f32>(20.0, 20.0, 20.0, q.a) / 255.0;
            }
            let v = max(max(max(q.r, q.g), q.b), 1.0);
            let lv = 3.0;
            let qq = max(rnd(v / 255.0 * lv) / lv, 0.12);
            let k = qq * 255.0 / v;
            let mean = (q.r + q.g + q.b) / 3.0;
            let sat = vec3<f32>(mean) + (q.rgb - vec3<f32>(mean)) * 1.35;
            return vec4<f32>(c8v(sat * k), q.a) / 255.0;
        }
        // 44 colour pencil: t1 blurred for the outlines, t2 the jitter; period amount
        case 44: {
            let q = ld(t0, ip);
            let l = luma(q) / 255.0;
            let dark = pow(1.0 - l, 1.3);
            let j = textureLoad(t2, ip, 0).r;
            var ink = pencil_line(x + y, j, a.x) * dark;
            if (dark > 0.55) {
                ink = max(ink, pencil_line(x - y, j, a.x) * (dark - 0.45));
            }
            let outline = clamp(edge(t1, ip) * 1.6, 0.0, 1.0);
            let shade = 1.0 - max(ink * (0.45 + 0.4 * a.y), outline * 0.9);
            let tint = vec3<f32>(250.0) + (q.rgb - vec3<f32>(250.0)) * (0.25 + 0.35 * a.y);
            return vec4<f32>(c8v(tint * shade), q.a) / 255.0;
        }
        // 45 thermal camera: amount
        case 45: {
            let q = ld(t0, ip);
            let l = luma(q) / 255.0;
            let t = clamp((l - 0.5) * (0.8 + a.x * 0.8) + 0.5, 0.0, 1.0);
            return vec4<f32>(c8v(ramp_thermal(t)), q.a) / 255.0;
        }
        // 46 x-ray: amount
        case 46: {
            let q = ld(t0, ip);
            let l = luma(q) / 255.0;
            let v = pow(1.0 - l, lerp(1.0, 0.6, a.x));
            return vec4<f32>(c8(v * lerp(235.0, 190.0, a.x)), c8(v * 245.0), c8(v * 250.0 + (1.0 - v) * 18.0), q.a) / 255.0;
        }
        // 47 pop art: t0 the half-size copy; hw hh (cuts from the histogram pass)
        case 47: {
            let hw = i32(a.x);
            let hh = i32(a.y);
            let qd = i32(ip.x >= hw) + 2 * i32(ip.y >= hh);
            let p = ld(t0, vec2<i32>(min(ip.x % hw, hw - 1), min(ip.y % hh, hh - 1)));
            let key = u32(p.r) * 299u + u32(p.g) * 587u + u32(p.b) * 114u;
            var k = 2;
            if (key < cuts[0]) {
                k = 0;
            } else if (key < cuts[1]) {
                k = 1;
            }
            return vec4<f32>(c8v(pop_colour(qd, k)), p.a) / 255.0;
        }
        // 48 mirror: the line
        case 48: {
            if ((x >= a.x) != (a.y > 0.5)) {
                let sx = i32(clamp(2.0 * a.x - x - 1.0, 0.0, osz.x - 1.0));
                return textureLoad(t0, vec2<i32>(sx, ip.y), 0);
            }
            return textureLoad(t0, ip, 0);
        }
        // 49 AI shield: amp, pattern, noise, seed halves in a; 9 gratings in p[2..11]
        case 49: {
            let q = ld(t0, ip);
            if (q.a < 8.0) {
                return q / 255.0;
            }
            let i = u32(ip.y) * u32(osz.x) + u32(ip.x);
            let z = (u32(b.x) << 16u) | u32(b.y);
            var o = q;
            for (var ch = 0; ch < 3; ch = ch + 1) {
                var d = 0.0;
                if (a.y > 0.5) {
                    var v = 0.0;
                    for (var k = 0; k < 3; k = k + 1) {
                        let wv = u.p[3 + ch * 3 + k];
                        v = v + sin(wv.x * x + wv.y * y + wv.z);
                    }
                    d = d + v / 3.0 * 0.8;
                }
                if (a.z > 0.5) {
                    let r = hash3(i, u32(ch), z);
                    d = d + (f32(r & 0xffffu) / 32767.5 - 1.0) * 0.6;
                }
                o[ch] = c8(q[ch] + d * a.x);
            }
            return o / 255.0;
        }
        // 50 a keyed fade: t0 after, t1 before; amount, before's width and height
        case 50: {
            let q = ld(t0, ip);
            let i = ip.y * i32(osz.x) + ip.x;
            let bw = i32(a.y);
            if (i >= bw * i32(a.z)) {
                return q / 255.0;
            }
            let p = ld(t1, vec2<i32>(i % bw, i / bw));
            return vec4<f32>(rnd4(p + (q - p) * a.x)) / 255.0;
        }
        // 51 censor: mode (0 solid 1 blur 2 pixelate), region x0 y0 x1 y1 in a,
        // turned box (cx cy hw hh) in b, its sin cos and the square in c, colour in p[4]
        case 51: {
            let q = textureLoad(t0, ip, 0);
            if (x < a.x || y < a.y || x >= a.z || y >= a.w) {
                return q;
            }
            if (c.w > 0.5) {
                let dx = x + 0.5 - b.x;
                let dy = y + 0.5 - b.y;
                let lx = c.y * dx - c.x * dy;
                let ly = c.x * dx + c.y * dy;
                if (!(abs(lx) <= b.z && abs(ly) <= b.w)) {
                    return q;
                }
            }
            let mode = i32(u.p[0].y);
            if (mode == 0) {
                return vec4<f32>(u.p[4].rgb, 255.0) / 255.0;
            }
            if (mode == 1) {
                return textureLoad(t1, ip, 0);
            }
            let sq = i32(c.z);
            return textureLoad(t1, (ip - vec2<i32>(i32(a.x), i32(a.y))) / sq, 0);
        }
        default: {
            return textureLoad(t0, ip, 0);
        }
    }
}
