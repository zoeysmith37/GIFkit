//! GIF LZW encoder with optional "lossy" matching (in the spirit of gifsicle
//! --lossy): while extending a dictionary string, a pixel may be replaced by a
//! visually similar palette colour if that lets the current string continue.
//! Longer strings → fewer codes → smaller files.

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn put(&mut self, code: u16, size: u32) {
        self.acc |= (code as u32) << self.nbits;
        self.nbits += size;
        while self.nbits >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }
    fn flush(&mut self) {
        if self.nbits > 0 {
            self.out.push(self.acc as u8);
        }
        self.acc = 0;
        self.nbits = 0;
    }
}

/// Returns `[min_code_size, lzw bytes...]` as expected by
/// `gif::Encoder::write_lzw_pre_encoded_frame`.
///
/// `palette` is RGB triplets; `max_dist` is the largest Euclidean RGB distance
/// a substituted pixel may have (0 = lossless).
pub fn encode(indices: &[u8], palette: &[u8], transparent: Option<u8>, max_dist: f32) -> Vec<u8> {
    encode_ex(indices, None, palette, transparent, max_dist).0
}

/// Like [`encode`], but pixel `i` may also be written as `alt[i]` (when it
/// differs from `indices[i]`) if that continues the current dictionary string.
/// Used for unchanged pixels, which can be either transparent or their own
/// colour with identical results. Returns the LZW data and the indices
/// actually written.
pub fn encode_ex(indices: &[u8], alt: Option<&[u8]>, palette: &[u8], transparent: Option<u8>, max_dist: f32) -> (Vec<u8>, Vec<u8>) {
    let mut chosen = indices.to_vec();
    let pal_len = (palette.len() / 3).max(2);
    let max_idx = indices.iter().chain(alt.unwrap_or(&[]).iter()).copied().max().unwrap_or(0) as usize + 1;
    let needed = pal_len.max(max_idx).max(4);
    let min_code_size = (needed.next_power_of_two().trailing_zeros()).clamp(2, 8);

    let clear: u16 = 1 << min_code_size;
    let eoi: u16 = clear + 1;
    let mut bw = BitWriter { out: Vec::with_capacity(indices.len() / 2 + 16), acc: 0, nbits: 0 };
    bw.out.push(min_code_size as u8);

    if indices.is_empty() {
        bw.put(clear, min_code_size + 1);
        bw.put(eoi, min_code_size + 1);
        bw.flush();
        return (bw.out, chosen);
    }

    // allowed substitutions table: allowed[a*256+b] when colour b may stand in for a
    let lossy = max_dist > 0.0;
    let mut allowed = Vec::new();
    if lossy {
        let thr = max_dist * max_dist;
        let n = palette.len() / 3;
        allowed = vec![false; 256 * 256];
        for a in 0..n {
            for b in 0..n {
                if Some(a as u8) == transparent || Some(b as u8) == transparent {
                    continue;
                }
                let d = (0..3).map(|k| (palette[a * 3 + k] as f32 - palette[b * 3 + k] as f32).powi(2)).sum::<f32>();
                allowed[a * 256 + b] = d <= thr;
            }
        }
    }

    // trie: child[node*256 + byte] = code (0 = none); kids[node] = list of child bytes
    let mut child = vec![0u16; 4096 * 256];
    let mut kids: Vec<Vec<u8>> = vec![Vec::new(); 4096];
    let mut used_nodes: Vec<u16> = Vec::new();

    let mut code_size = min_code_size + 1;
    let mut next: u16 = eoi + 1;

    bw.put(clear, code_size);

    let mut cur: u16 = indices[0] as u16;
    let mut i = 1;
    while i < indices.len() {
        let px = indices[i];
        let c = child[cur as usize * 256 + px as usize];
        if c != 0 {
            cur = c;
            i += 1;
            continue;
        }
        if let Some(a) = alt {
            let ap = a[i];
            if ap != px {
                let c2 = child[cur as usize * 256 + ap as usize];
                if c2 != 0 {
                    cur = c2;
                    chosen[i] = ap;
                    i += 1;
                    continue;
                }
            }
        }
        if lossy {
            // find the closest existing continuation that is "close enough"
            let mut best: Option<u16> = None;
            let mut best_d = u32::MAX;
            for &k in &kids[cur as usize] {
                if allowed[px as usize * 256 + k as usize] {
                    let d = (0..3).map(|q| (palette[px as usize * 3 + q] as i32 - palette[k as usize * 3 + q] as i32).pow(2) as u32).sum::<u32>();
                    if d < best_d {
                        best_d = d;
                        best = Some(child[cur as usize * 256 + k as usize]);
                    }
                }
            }
            if let Some(b) = best {
                cur = b;
                i += 1;
                continue;
            }
        }
        // emit and add new entry
        bw.put(cur, code_size);
        if next < 4096 {
            child[cur as usize * 256 + px as usize] = next;
            if kids[cur as usize].is_empty() {
                used_nodes.push(cur);
            }
            kids[cur as usize].push(px);
            next += 1;
            if next > (1u16 << code_size) && code_size < 12 {
                code_size += 1;
            }
        } else {
            // table full: reset
            bw.put(clear, code_size);
            for &n in &used_nodes {
                for &k in &kids[n as usize] {
                    child[n as usize * 256 + k as usize] = 0;
                }
                kids[n as usize].clear();
            }
            used_nodes.clear();
            code_size = min_code_size + 1;
            next = eoi + 1;
        }
        cur = px as u16;
        i += 1;
    }
    bw.put(cur, code_size);
    bw.put(eoi, code_size);
    bw.flush();
    (bw.out, chosen)
}
