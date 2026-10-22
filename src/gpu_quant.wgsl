// GIF colour mapping: each pixel to its palette index, exactly as the
// encoder's CPU path picks it (encode::Palette::index_of). Four pixels a
// thread, packed into one u32.

struct Q {
    w: u32,
    h: u32,
    x0: u32,
    y0: u32,
    // 0: NeuQuant's own search (its netindex), 1: nearest by RGB distance
    mode: u32,
    n: u32,
    ordered: u32,
    stride: u32,
    spread: f32,
};

@group(0) @binding(0) var img: texture_2d<f32>;
@group(0) @binding(1) var<uniform> q: Q;
@group(0) @binding(2) var<storage, read> pal: array<vec4<i32>, 256>;
@group(0) @binding(3) var<storage, read> netindex: array<u32, 256>;
@group(0) @binding(4) var<storage, read_write> out: array<u32>;

const BAYER = array<u32, 64>(
    0u, 32u, 8u, 40u, 2u, 34u, 10u, 42u,
    48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u,
    12u, 44u, 4u, 36u, 14u, 46u, 6u, 38u,
    60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
    3u, 35u, 11u, 43u, 1u, 33u, 9u, 41u,
    51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u,
    15u, 47u, 7u, 39u, 13u, 45u, 5u, 37u,
    63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u
);

// color_quant's search_netindex, step for step (ties and all)
fn nq(r: i32, g: i32, b: i32, a: i32) -> u32 {
    var bestd: i32 = 1 << 30;
    var best = 0u;
    var i = netindex[u32(g)];
    var j = select(0u, i - 1u, i > 0u);
    loop {
        if (!(i < q.n || j > 0u)) {
            break;
        }
        if (i < q.n) {
            let p = pal[i];
            var e = p.g - g;
            var dist = e * e;
            if (dist >= bestd) {
                break;
            }
            e = p.b - b;
            dist = dist + e * e;
            if (dist < bestd) {
                e = p.r - r;
                dist = dist + e * e;
                if (dist < bestd) {
                    e = p.a - a;
                    dist = dist + e * e;
                    if (dist < bestd) {
                        bestd = dist;
                        best = i;
                    }
                }
            }
            i = i + 1u;
        }
        if (j > 0u) {
            let p = pal[j];
            var e = p.g - g;
            var dist = e * e;
            if (dist >= bestd) {
                break;
            }
            e = p.b - b;
            dist = dist + e * e;
            if (dist < bestd) {
                e = p.r - r;
                dist = dist + e * e;
                if (dist < bestd) {
                    e = p.a - a;
                    dist = dist + e * e;
                    if (dist < bestd) {
                        bestd = dist;
                        best = j;
                    }
                }
            }
            j = j - 1u;
        }
    }
    return best;
}

// nearest by RGB distance, the lowest index on a tie
fn nearest(r: i32, g: i32, b: i32) -> u32 {
    var bestd: i32 = 0x7fffffff;
    var best = 0u;
    for (var k = 0u; k < q.n; k = k + 1u) {
        let p = pal[k];
        let d = (p.r - r) * (p.r - r) + (p.g - g) * (p.g - g) + (p.b - b) * (p.b - b);
        if (d < bestd) {
            bestd = d;
            best = k;
            if (d == 0) {
                break;
            }
        }
    }
    return best;
}

@compute @workgroup_size(64)
fn cs_map(@builtin(global_invocation_id) id: vec3<u32>) {
    let k = id.y * q.stride + id.x;
    let total = q.w * q.h;
    if (k * 4u >= total) {
        return;
    }
    var packed = 0u;
    for (var t = 0u; t < 4u; t = t + 1u) {
        let p = k * 4u + t;
        if (p >= total) {
            break;
        }
        let x = q.x0 + p % q.w;
        let y = q.y0 + p / q.w;
        var c = vec3<f32>(round(textureLoad(img, vec2<u32>(x, y), 0).rgb * 255.0));
        if (q.ordered != 0u) {
            let d = (f32(BAYER[(y % 8u) * 8u + x % 8u]) + 0.5) / 64.0 - 0.5;
            c = clamp(round(c + vec3<f32>(d * q.spread)), vec3<f32>(0.0), vec3<f32>(255.0));
        }
        let ci = vec3<i32>(c);
        var idx = 0u;
        if (q.mode == 0u) {
            idx = nq(ci.r, ci.g, ci.b, 255);
        } else {
            idx = nearest(ci.r, ci.g, ci.b);
        }
        packed = packed | ((idx & 255u) << (8u * t));
    }
    out[k] = packed;
}
