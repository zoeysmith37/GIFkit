// Pop Art splits a picture at its own brightness thirds. The CPU sorts every
// pixel's brightness; here each pixel's exact brightness key
// (299 r + 587 g + 114 b, 0..255000) is counted, and the counts are walked to
// find the two cut points. buf[0..2] gets the cuts, the counts start at 4.

struct H {
    p: array<vec4<f32>, 24>,
};

@group(0) @binding(0) var img: texture_2d<f32>;
@group(0) @binding(1) var<uniform> h: H;
@group(0) @binding(4) var<storage, read_write> buf: array<atomic<u32>>;

const BINS: u32 = 255001u;

@compute @workgroup_size(16, 16)
fn cs_count(@builtin(global_invocation_id) id: vec3<u32>) {
    let d = textureDimensions(img);
    if (id.x >= d.x || id.y >= d.y) {
        return;
    }
    let c = vec3<u32>(floor(textureLoad(img, vec2<i32>(id.xy), 0).rgb * 255.0 + vec3<f32>(0.5)));
    let key = c.r * 299u + c.g * 587u + c.b * 114u;
    atomicAdd(&buf[4u + key], 1u);
}

var<workgroup> part: array<u32, 256>;

// one workgroup: each thread sums a run of bins, then thread 0 finds the
// runs holding the two sorted positions and walks them
@compute @workgroup_size(256)
fn cs_cuts(@builtin(local_invocation_index) t: u32) {
    let run = (BINS + 255u) / 256u;
    var s = 0u;
    for (var i = t * run; i < min((t + 1u) * run, BINS); i = i + 1u) {
        s = s + atomicLoad(&buf[4u + i]);
    }
    part[t] = s;
    workgroupBarrier();
    if (t != 0u) {
        return;
    }
    for (var q = 0u; q < 2u; q = q + 1u) {
        // the sorted position wanted: values below it plus one
        let want = u32(h.p[0][q]) + 1u;
        var seen = 0u;
        var r = 0u;
        loop {
            if (r >= 255u || seen + part[r] >= want) {
                break;
            }
            seen = seen + part[r];
            r = r + 1u;
        }
        var key = r * run;
        loop {
            if (key >= BINS - 1u) {
                break;
            }
            let n = atomicLoad(&buf[4u + key]);
            if (seen + n >= want) {
                break;
            }
            seen = seen + n;
            key = key + 1u;
        }
        atomicStore(&buf[q], key);
    }
}
