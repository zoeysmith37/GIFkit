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
