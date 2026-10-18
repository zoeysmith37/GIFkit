//! "No AI training" markers added to saved files.
//!
//! Text comment for people and simple scrapers, plus XMP with the PLUS
//! `DataMining` field set to "prohibited" where the format has a place for it.

pub const COMMENT: &str = crate::encode::NOAI_COMMENT;

fn xmp() -> String {
    format!(
        concat!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>",
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">",
            "<rdf:Description rdf:about=\"\" xmlns:plus=\"http://ns.useplus.org/ldf/xmp/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
            "<plus:DataMining>http://ns.useplus.org/ldf/vocab/DMI-PROHIBITED</plus:DataMining>",
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>",
            "</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"r\"?>"
        ),
        COMMENT
    )
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(data.len() + 12);
    c.extend_from_slice(&(data.len() as u32).to_be_bytes());
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    c.extend_from_slice(&h.finalize().to_be_bytes());
    c
}

/// Add a tEXt comment and an XMP iTXt chunk right after IHDR.
/// Returns the input unchanged if it isn't a PNG.
pub fn tag_png(b: Vec<u8>) -> Vec<u8> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if b.len() < 33 || &b[..8] != SIG || &b[12..16] != b"IHDR" {
        return b;
    }
    let ihdr_end = 8 + 12 + u32::from_be_bytes([b[8], b[9], b[10], b[11]]) as usize;
    if ihdr_end > b.len() {
        return b;
    }
    let mut text = b"Comment\0".to_vec();
    text.extend_from_slice(COMMENT.as_bytes());
    // iTXt: keyword, 0, compression flag, method, language\0, translated keyword\0, text
    let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    itxt.extend_from_slice(xmp().as_bytes());
    let mut out = Vec::with_capacity(b.len() + 1024);
    out.extend_from_slice(&b[..ihdr_end]);
    out.extend(png_chunk(b"tEXt", &text));
    out.extend(png_chunk(b"iTXt", &itxt));
    out.extend_from_slice(&b[ihdr_end..]);
    out
}

/// Append an XMP chunk to an extended (VP8X) WebP. Simple WebPs are left as they are.
pub fn tag_webp(mut b: Vec<u8>) -> Vec<u8> {
    if b.len() < 30 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" || &b[12..16] != b"VP8X" {
        return b;
    }
    b[20] |= 0x04; // XMP present
    let x = xmp();
    b.extend_from_slice(b"XMP ");
    b.extend_from_slice(&(x.len() as u32).to_le_bytes());
    b.extend_from_slice(x.as_bytes());
    if x.len() % 2 == 1 {
        b.push(0);
    }
    let riff = (b.len() - 8) as u32;
    b[4..8].copy_from_slice(&riff.to_le_bytes());
    b
}

// ---------------------------------------------------------------- AVIF (HEIF)

fn rd(b: &[u8], o: usize, n: usize) -> Option<u64> {
    let s = b.get(o..o + n)?;
    Some(s.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
}

fn wr(out: &mut Vec<u8>, v: u64, n: usize) {
    for i in (0..n).rev() {
        out.push((v >> (8 * i)) as u8);
    }
}

/// (type, start, header length, total size) of the boxes in `b[from..to]`.
fn boxes(b: &[u8], from: usize, to: usize) -> Option<Vec<([u8; 4], usize, usize, usize)>> {
    let mut v = Vec::new();
    let mut o = from;
    while o + 8 <= to {
        let mut size = rd(b, o, 4)? as usize;
        let t: [u8; 4] = b[o + 4..o + 8].try_into().ok()?;
        let mut hl = 8;
        if size == 1 {
            size = rd(b, o + 8, 8)? as usize;
            hl = 16;
        } else if size == 0 {
            size = to - o;
        }
        if size < hl || o + size > to {
            return None;
        }
        v.push((t, o, hl, size));
        o += size;
    }
    (o == to).then_some(v)
}

fn put_box(out: &mut Vec<u8>, t: &[u8; 4], body: &[u8]) {
    wr(out, (body.len() + 8) as u64, 4);
    out.extend_from_slice(t);
    out.extend_from_slice(body);
}

struct Iloc {
    version: u8,
    flags: [u8; 3],
    off: usize,
    len: usize,
    base: usize,
    idx: usize,
    /// raw items, offsets resolved: (id, method, dref, base, extents[(index, offset, length)])
    items: Vec<(u32, u16, u16, u64, Vec<(u64, u64, u64)>)>,
}

fn parse_iloc(b: &[u8]) -> Option<Iloc> {
    let version = *b.first()?;
    let flags = [b[1], b[2], b[3]];
    let off = (b[4] >> 4) as usize;
    let len = (b[4] & 15) as usize;
    let base = (b[5] >> 4) as usize;
    let idx = if version >= 1 { (b[5] & 15) as usize } else { 0 };
    let mut o = 6;
    let (count, n) = if version < 2 { (rd(b, o, 2)?, 2) } else { (rd(b, o, 4)?, 4) };
    o += n;
    let mut items = Vec::new();
    for _ in 0..count {
        let idn = if version < 2 { 2 } else { 4 };
        let id = rd(b, o, idn)? as u32;
        o += idn;
        let method = if version >= 1 {
            o += 2;
            (rd(b, o - 2, 2)? & 15) as u16
        } else {
            0
        };
        let dref = rd(b, o, 2)? as u16;
        o += 2;
        let bo = rd(b, o, base)?;
        o += base;
        let ec = rd(b, o, 2)?;
        o += 2;
        let mut ex = Vec::new();
        for _ in 0..ec {
            let i = rd(b, o, idx)?;
            o += idx;
            let eo = rd(b, o, off)?;
            o += off;
            let el = rd(b, o, len)?;
            o += len;
            ex.push((i, eo, el));
        }
        items.push((id, method, dref, bo, ex));
    }
    Some(Iloc { version, flags, off, len, base, idx, items })
}

fn write_iloc(l: &Iloc) -> Vec<u8> {
    let mut o = vec![l.version, l.flags[0], l.flags[1], l.flags[2], ((l.off << 4) | l.len) as u8, ((l.base << 4) | l.idx) as u8];
    wr(&mut o, l.items.len() as u64, if l.version < 2 { 2 } else { 4 });
    for (id, method, dref, bo, ex) in &l.items {
        wr(&mut o, *id as u64, if l.version < 2 { 2 } else { 4 });
        if l.version >= 1 {
            wr(&mut o, *method as u64, 2);
        }
        wr(&mut o, *dref as u64, 2);
        wr(&mut o, *bo, l.base);
        wr(&mut o, ex.len() as u64, 2);
        for (i, eo, el) in ex {
            wr(&mut o, *i, l.idx);
            wr(&mut o, *eo, l.off);
            wr(&mut o, *el, l.len);
        }
    }
    o
}

/// Add an XMP metadata item (linked to the primary image) to an AVIF.
/// Returns the input unchanged if anything about the file is unexpected.
pub fn tag_avif(b: Vec<u8>) -> Vec<u8> {
    try_tag_avif(&b).unwrap_or(b)
}

fn try_tag_avif(b: &[u8]) -> Option<Vec<u8>> {
    let top = boxes(b, 0, b.len())?;
    if top.first()?.0 != *b"ftyp" {
        return None;
    }
    let mi = top.iter().position(|x| &x.0 == b"meta")?;
    let (_, ms, mhl, msz) = top[mi];
    let meta_end = ms + msz;
    let kids = boxes(b, ms + mhl + 4, meta_end)?;
    let find = |t: &[u8; 4]| kids.iter().find(|k| &k.0 == t).copied();
    let (_, ps, phl, _) = find(b"pitm")?;
    let pv = b[ps + phl];
    let primary = rd(b, ps + phl + 4, if pv == 0 { 2 } else { 4 })? as u32;
    let (_, is, ihl, isz) = find(b"iloc")?;
    let mut iloc = parse_iloc(&b[is + ihl..is + isz])?;
    let (_, fs, fhl, fsz) = find(b"iinf")?;
    let iinf = &b[fs + fhl..fs + fsz];
    let iref = find(b"iref");
    if iloc.items.iter().any(|it| it.1 > 2) || (iloc.off == 0 && iloc.base == 0) {
        return None;
    }
    let new_id = iloc.items.iter().map(|it| it.0).max()? + 1;
    // infe version 2 has 16-bit item ids
    if new_id > 0xffff {
        return None;
    }
    let x = xmp();
    let xmp_bytes = x.as_bytes();

    // new iinf: bumped count + infe v2 'mime'
    let mut infe = vec![2u8, 0, 0, 0];
    wr(&mut infe, new_id as u64, 2);
    wr(&mut infe, 0, 2);
    infe.extend_from_slice(b"mime");
    infe.extend_from_slice(b"XMP\0application/rdf+xml\0");
    let cn = if iinf[0] == 0 { 2 } else { 4 };
    let count = rd(iinf, 4, cn)?;
    let mut new_iinf = iinf[..4].to_vec();
    wr(&mut new_iinf, count + 1, cn);
    new_iinf.extend_from_slice(&iinf[4 + cn..]);
    put_box(&mut new_iinf, b"infe", &infe);

    // new iref: cdsc from the XMP item to the primary image
    let mut cdsc_body = Vec::new();
    let mut new_iref = Vec::new();
    match iref {
        Some((_, rs, rhl, rsz)) => {
            let v = b[rs + rhl];
            let w = if v == 0 { 2 } else { 4 };
            wr(&mut cdsc_body, new_id as u64, w);
            wr(&mut cdsc_body, 1, 2);
            wr(&mut cdsc_body, primary as u64, w);
            new_iref.extend_from_slice(&b[rs + rhl..rs + rsz]);
            put_box(&mut new_iref, b"cdsc", &cdsc_body);
        }
        None => {
            if new_id > 0xffff || primary > 0xffff {
                return None;
            }
            wr(&mut cdsc_body, new_id as u64, 2);
            wr(&mut cdsc_body, 1, 2);
            wr(&mut cdsc_body, primary as u64, 2);
            new_iref.extend_from_slice(&[0, 0, 0, 0]);
            put_box(&mut new_iref, b"cdsc", &cdsc_body);
        }
    }

    // iloc gets one more item; its size is known before the offsets are
    iloc.items.push((new_id, 0, 0, 0, vec![(0, 0, xmp_bytes.len() as u64)]));
    let iloc_len = write_iloc(&iloc).len();
    let mut meta_body_len = 4;
    for (t, _, _, sz) in &kids {
        meta_body_len += match t {
            b"iinf" => new_iinf.len() + 8,
            b"iloc" => iloc_len + 8,
            b"iref" => new_iref.len() + 8,
            _ => *sz,
        };
    }
    if iref.is_none() {
        meta_body_len += new_iref.len() + 8;
    }
    let new_meta_len = meta_body_len + 8;
    if mhl != 8 {
        return None;
    }
    let delta = new_meta_len as i64 - msz as i64;
    let shift = |v: u64| -> u64 { if v >= meta_end as u64 { (v as i64 + delta) as u64 } else { v } };

    // file offsets in iloc (construction method 0)
    let fits = |v: u64, n: usize| n >= 8 || v < (1u64 << (8 * n));
    let last = iloc.items.len() - 1;
    for it in iloc.items[..last].iter_mut() {
        if it.1 != 0 {
            continue;
        }
        if iloc.base > 0 && it.3 > 0 {
            it.3 = shift(it.3);
            if !fits(it.3, iloc.base) {
                return None;
            }
        } else {
            for e in it.4.iter_mut() {
                e.1 = shift(e.1);
                if !fits(e.1, iloc.off) {
                    return None;
                }
            }
        }
    }
    // the XMP goes in its own mdat at the very end
    let total_before = (b.len() as i64 + delta) as u64;
    let xmp_at = total_before + 8;
    {
        let it = &mut iloc.items[last];
        if iloc.off > 0 {
            if !fits(xmp_at, iloc.off) {
                return None;
            }
            it.4[0].1 = xmp_at;
        } else {
            if !fits(xmp_at, iloc.base) {
                return None;
            }
            it.3 = xmp_at;
        }
        if iloc.len == 0 {
            // length 0 means "to the end of the file", which is where the XMP is
            it.4[0].2 = 0;
        }
    }
    let new_iloc = write_iloc(&iloc);
    debug_assert_eq!(new_iloc.len(), iloc_len);

    // rebuild meta
    let mut meta = b[ms + mhl..ms + mhl + 4].to_vec();
    for (t, s, _, sz) in &kids {
        match t {
            b"iinf" => put_box(&mut meta, b"iinf", &new_iinf),
            b"iloc" => put_box(&mut meta, b"iloc", &new_iloc),
            b"iref" => put_box(&mut meta, b"iref", &new_iref),
            _ => meta.extend_from_slice(&b[*s..s + sz]),
        }
    }
    if iref.is_none() {
        put_box(&mut meta, b"iref", &new_iref);
    }
    if meta.len() + 8 != new_meta_len {
        return None;
    }

    // chunk offsets of the image sequence tracks
    let mut tail = b[meta_end..].to_vec();
    let mut stack = vec![(0usize, tail.len())];
    let mut patches: Vec<(usize, usize)> = Vec::new(); // (pos in tail, width)
    while let Some((from, to)) = stack.pop() {
        for (t, s, hl, sz) in boxes(&tail, from, to)? {
            match &t {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => stack.push((s + hl, s + sz)),
                b"stco" | b"co64" => {
                    let w = if &t == b"stco" { 4 } else { 8 };
                    let n = rd(&tail, s + hl + 4, 4)? as usize;
                    for i in 0..n {
                        patches.push((s + hl + 8 + i * w, w));
                    }
                }
                b"saio" => return None,
                _ => {}
            }
        }
    }
    for (pos, w) in patches {
        let v = shift(rd(&tail, pos, w)?);
        if !fits(v, w) {
            return None;
        }
        let mut tmp = Vec::new();
        wr(&mut tmp, v, w);
        tail[pos..pos + w].copy_from_slice(&tmp);
    }

    let mut out = Vec::with_capacity(b.len() + 2048);
    out.extend_from_slice(&b[..ms]);
    put_box(&mut out, b"meta", &meta);
    out.extend(tail);
    if out.len() as u64 != total_before {
        return None;
    }
    put_box(&mut out, b"mdat", xmp_bytes);
    Some(out)
}
