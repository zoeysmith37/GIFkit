//! A frame's pixels, kept packed when they're stored for a while and
//! unpacked on demand into one shared cache with a size limit.
//!
//! Decoded RGBA is 4 bytes a pixel: a 300 frame 800x450 GIF is 400 MB that
//! way, from a file under 1 MB. Frames with 256 colours or fewer (every GIF
//! frame) pack to 1 byte a pixel plus a palette, then LZ4; others pack as
//! QOI. Unpacking is fast enough to do as frames are needed, so memory is
//! set by the cache size rather than by the length of the animation.

use image::RgbaImage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

enum Store {
    /// as it is (new, temporary or small)
    Raw(Arc<RgbaImage>),
    /// palette indices, LZ4 compressed (size prepended)
    Indexed { palette: Box<[[u8; 4]]>, data: Box<[u8]> },
    /// lossless QOI
    Qoi(Box<[u8]>),
}

struct Inner {
    id: u64,
    w: u32,
    h: u32,
    store: Store,
    /// the unpacked picture while something still holds it (the cache, or
    /// whoever asked for it last)
    open: Mutex<Weak<RgbaImage>>,
    /// being unpacked ahead of time
    fetching: std::sync::atomic::AtomicBool,
}

/// Bytes of unpacked (raw) frames alive, for the memory report.
pub static RAW_BYTES: AtomicUsize = AtomicUsize::new(0);

impl Drop for Inner {
    fn drop(&mut self) {
        if let Store::Raw(i) = &self.store {
            RAW_BYTES.fetch_sub(i.as_raw().len(), Ordering::Relaxed);
        }
        if !matches!(self.store, Store::Raw(_)) {
            cache_forget(self.id);
        }
    }
}

/// A frame's pixels. Cloning shares them.
#[derive(Clone)]
pub struct Pixels(Arc<Inner>);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Pixels {
    pub fn new(img: RgbaImage) -> Self {
        Self::from_arc(Arc::new(img))
    }

    pub fn from_arc(img: Arc<RgbaImage>) -> Self {
        let (w, h) = img.dimensions();
        RAW_BYTES.fetch_add(img.as_raw().len(), Ordering::Relaxed);
        Pixels(Arc::new(Inner { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), w, h, store: Store::Raw(img), open: Mutex::new(Weak::new()), fetching: std::sync::atomic::AtomicBool::new(false) }))
    }

    pub fn width(&self) -> u32 {
        self.0.w
    }
    pub fn height(&self) -> u32 {
        self.0.h
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.0.w, self.0.h)
    }

    /// Stays the same for these pixels (and their clones) while they live.
    pub fn id(&self) -> u64 {
        self.0.id
    }

    /// The same pixels (a clone of the other, not just equal ones).
    pub fn ptr_eq(a: &Pixels, b: &Pixels) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    pub fn is_packed(&self) -> bool {
        !matches!(self.0.store, Store::Raw(_))
    }

    /// Bytes these pixels take while stored (not counting the cache).
    pub fn stored_bytes(&self) -> usize {
        match &self.0.store {
            Store::Raw(i) => i.as_raw().len(),
            Store::Indexed { palette, data } => palette.len() * 4 + data.len(),
            Store::Qoi(d) => d.len(),
        }
    }

    /// Is any pixel's alpha below `thr`? Packed frames answer from their
    /// palette, without unpacking.
    pub fn any_alpha_below(&self, thr: u8) -> bool {
        match &self.0.store {
            Store::Indexed { palette, .. } => palette.iter().any(|c| c[3] < thr),
            _ => self.get().pixels().any(|p| p[3] < thr),
        }
    }

    /// Is every pixel fully see-through? Packed frames answer from their palette.
    pub fn is_fully_transparent(&self) -> bool {
        match &self.0.store {
            Store::Indexed { palette, .. } => palette.iter().all(|c| c[3] == 0),
            _ => self.get().pixels().all(|p| p[3] == 0),
        }
    }

    /// Each colour (RGBA) and how many pixels have it. Packed frames count
    /// their palette indices instead of unpacking.
    pub fn colour_counts(&self, mut f: impl FnMut([u8; 4], u64)) {
        match &self.0.store {
            Store::Indexed { palette, data } => {
                let idx = lz4_flex::decompress_size_prepended(data).unwrap_or_default();
                let mut n = [0u64; 256];
                for &k in &idx {
                    n[k as usize] += 1;
                }
                for (k, c) in palette.iter().enumerate() {
                    if n[k] > 0 {
                        f(*c, n[k]);
                    }
                }
            }
            _ => {
                let img = self.get();
                let mut map: HashMap<u32, u64> = HashMap::new();
                for p in img.pixels() {
                    *map.entry(u32::from_le_bytes(p.0)).or_insert(0) += 1;
                }
                for (c, n) in map {
                    f(c.to_le_bytes(), n);
                }
            }
        }
    }

    /// Unpacked already (or never packed)?
    pub fn is_ready(&self) -> bool {
        matches!(self.0.store, Store::Raw(_)) || self.0.open.lock().ok().and_then(|w| w.upgrade()).is_some()
    }

    /// Unpack in the background now, so it's ready when it's needed (the
    /// frames just ahead in playback).
    pub fn prefetch(&self) {
        if self.is_ready() || self.0.fetching.swap(true, Ordering::Relaxed) {
            return;
        }
        let p = self.clone();
        crate::stack::bg_pool().spawn(move || {
            let _ = p.get();
            p.0.fetching.store(false, Ordering::Relaxed);
        });
    }

    /// The pixels, unpacked (from the cache when they're there).
    pub fn get(&self) -> Arc<RgbaImage> {
        if let Store::Raw(i) = &self.0.store {
            return i.clone();
        }
        if let Some(i) = self.0.open.lock().ok().and_then(|w| w.upgrade()) {
            cache_touch(self.0.id);
            return i;
        }
        let img = Arc::new(self.unpack());
        if let Ok(mut w) = self.0.open.lock() {
            *w = Arc::downgrade(&img);
        }
        cache_put(self.0.id, img.clone());
        img
    }

    fn unpack(&self) -> RgbaImage {
        let (w, h) = (self.0.w, self.0.h);
        match &self.0.store {
            Store::Raw(i) => (**i).clone(),
            Store::Indexed { palette, data } => {
                let idx = lz4_flex::decompress_size_prepended(data).unwrap_or_default();
                let mut out = vec![0u8; (w * h * 4) as usize];
                for (o, &k) in out.chunks_exact_mut(4).zip(idx.iter()) {
                    o.copy_from_slice(&palette[k as usize]);
                }
                RgbaImage::from_raw(w, h, out).unwrap_or_else(|| RgbaImage::new(w, h))
            }
            Store::Qoi(d) => match qoi::decode_to_vec(d) {
                Ok((_, px)) => RgbaImage::from_raw(w, h, px).unwrap_or_else(|| RgbaImage::new(w, h)),
                Err(_) => RgbaImage::new(w, h),
            },
        }
    }

    /// A packed copy, for keeping. Small pictures, and ones that wouldn't
    /// get much smaller, stay as they are.
    pub fn packed(&self) -> Pixels {
        let Store::Raw(img) = &self.0.store else { return self.clone() };
        let (w, h) = (self.0.w, self.0.h);
        let n = (w as usize) * (h as usize);
        if n < 64 * 64 {
            return self.clone();
        }
        let store = match palette_of(img) {
            Some((palette, idx)) => Store::Indexed { palette: palette.into_boxed_slice(), data: lz4_flex::compress_prepend_size(&idx).into_boxed_slice() },
            None => match qoi::encode_to_vec(img.as_raw(), w, h) {
                // not worth it below a third off
                Ok(q) if q.len() * 3 < n * 4 * 2 => Store::Qoi(q.into_boxed_slice()),
                _ => return self.clone(),
            },
        };
        let p = Pixels(Arc::new(Inner { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), w, h, store, open: Mutex::new(Weak::new()), fetching: std::sync::atomic::AtomicBool::new(false) }));
        // it was just in use: keep the unpacked picture to hand
        if let Ok(mut o) = p.0.open.lock() {
            *o = Arc::downgrade(img);
        }
        cache_put(p.0.id, img.clone());
        p
    }
}

/// The colours of `img` and each pixel's index, if there are 256 or fewer.
fn palette_of(img: &RgbaImage) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let mut map: HashMap<u32, u8> = HashMap::with_capacity(256);
    let mut pal: Vec<[u8; 4]> = Vec::with_capacity(256);
    let mut idx = Vec::with_capacity((img.width() * img.height()) as usize);
    let (mut last, mut last_i) = (u32::MAX, 0u8);
    for p in img.as_raw().chunks_exact(4) {
        let c = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        if c != last {
            last_i = match map.get(&c) {
                Some(&i) => i,
                None => {
                    if pal.len() == 256 {
                        return None;
                    }
                    let i = pal.len() as u8;
                    pal.push([p[0], p[1], p[2], p[3]]);
                    map.insert(c, i);
                    i
                }
            };
            last = c;
        }
        idx.push(last_i);
    }
    Some((pal, idx))
}

/// Packs frames in the background while more are being decoded, and swaps
/// each packed one in as soon as it's ready, so a long file never sits
/// unpacked in memory all at once.
pub struct Packer {
    tx: std::sync::mpsc::Sender<(usize, Pixels)>,
    rx: std::sync::mpsc::Receiver<(usize, Pixels)>,
    out: usize,
}

impl Default for Packer {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Packer { tx, rx, out: 0 }
    }
}

impl Packer {
    /// Pack `frames[i]` (just added) in the background; take in what's done.
    pub fn add(&mut self, frames: &mut [crate::anim::Frame], i: usize) {
        let p = frames[i].image.clone();
        let tx = self.tx.clone();
        self.out += 1;
        crate::stack::bg_pool().spawn(move || {
            let _ = tx.send((i, p.packed()));
        });
        // keep up: don't let more than a few dozen wait
        self.wait_until(frames, 48);
    }

    fn take(&mut self, frames: &mut [crate::anim::Frame], (i, p): (usize, Pixels)) {
        self.out -= 1;
        if let Some(f) = frames.get_mut(i) {
            f.image = p;
        }
    }

    /// Take in finished ones until no more than `max` are waiting. A thread
    /// of the pool itself helps with the work rather than block it (with
    /// one thread, blocking would wait forever).
    fn wait_until(&mut self, frames: &mut [crate::anim::Frame], max: usize) {
        loop {
            while let Ok(r) = self.rx.try_recv() {
                self.take(frames, r);
            }
            if self.out <= max {
                return;
            }
            match rayon::yield_now() {
                Some(rayon::Yield::Executed) => {}
                _ => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
    }

    /// Wait for the rest.
    pub fn finish(mut self, frames: &mut [crate::anim::Frame]) {
        self.wait_until(frames, 0);
    }
}

// ---------------------------------------------------------------- the cache

struct Cache {
    map: HashMap<u64, (Arc<RgbaImage>, u64)>,
    bytes: usize,
    tick: u64,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
static BUDGET: AtomicUsize = AtomicUsize::new(128 << 20);

/// How much unpacked picture the cache keeps (bytes).
pub fn set_budget(b: usize) {
    BUDGET.store(b.max(16 << 20), Ordering::Relaxed);
}

static MEM_PCT: AtomicUsize = AtomicUsize::new(100);

/// How much memory GIFkit keeps for speed, as a percent of normal: every
/// cache budget scales with it.
pub fn set_mem_pct(pct: usize) {
    let pct = pct.clamp(25, 400);
    MEM_PCT.store(pct, Ordering::Relaxed);
    if std::env::var_os("GIFKIT_PIXCACHE_MB").is_none() {
        set_budget((128usize << 20) * pct / 100);
    }
}

/// `bytes` scaled by the memory setting.
pub fn scaled(bytes: usize) -> usize {
    scale_by(bytes, MEM_PCT.load(Ordering::Relaxed))
}

fn scale_by(bytes: usize, pct: usize) -> usize {
    (bytes as u128 * pct as u128 / 100) as usize
}

/// Memory the whole process uses now (bytes), where the system says.
pub fn process_bytes() -> Option<usize> {
    memory_stats::memory_stats().map(|m| m.physical_mem)
}

/// Bytes of unpacked pictures held by the cache.
pub fn cache_bytes() -> usize {
    CACHE.lock().ok().and_then(|c| c.as_ref().map(|c| c.bytes)).unwrap_or(0)
}

fn cache_put(id: u64, img: Arc<RgbaImage>) {
    static ENV: std::sync::Once = std::sync::Once::new();
    ENV.call_once(|| {
        if let Some(mb) = std::env::var("GIFKIT_PIXCACHE_MB").ok().and_then(|v| v.parse::<usize>().ok()) {
            set_budget(mb << 20);
        }
    });
    let Ok(mut g) = CACHE.lock() else { return };
    let c = g.get_or_insert_with(|| Cache { map: HashMap::new(), bytes: 0, tick: 0 });
    c.tick += 1;
    let b = img.as_raw().len();
    if let Some(old) = c.map.insert(id, (img, c.tick)) {
        c.bytes -= old.0.as_raw().len();
    }
    c.bytes += b;
    let budget = BUDGET.load(Ordering::Relaxed);
    if c.bytes > budget {
        // the least recently used go, down to seven eighths of the budget
        let mut v: Vec<(u64, u64)> = c.map.iter().map(|(k, e)| (e.1, *k)).collect();
        v.sort_unstable();
        let target = budget / 8 * 7;
        for (_, k) in v {
            if c.bytes <= target {
                break;
            }
            if k == id {
                continue;
            }
            if let Some(e) = c.map.remove(&k) {
                c.bytes -= e.0.as_raw().len();
            }
        }
    }
}

fn cache_touch(id: u64) {
    if let Ok(mut g) = CACHE.lock() {
        if let Some(c) = g.as_mut() {
            c.tick += 1;
            let t = c.tick;
            if let Some(e) = c.map.get_mut(&id) {
                e.1 = t;
            }
        }
    }
}

fn cache_forget(id: u64) {
    if let Ok(mut g) = CACHE.lock() {
        if let Some(c) = g.as_mut() {
            if let Some(e) = c.map.remove(&id) {
                c.bytes -= e.0.as_raw().len();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32, colours: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let k = (x / 3 + y / 5) % colours;
            image::Rgba([(k * 37) as u8, (k * 91 % 251) as u8, (k * 13) as u8, if k % 7 == 0 { 0 } else { 255 }])
        })
    }

    #[test]
    fn packs_and_unpacks_exactly() {
        for colours in [1, 2, 200, 256] {
            let img = checker(200, 150, colours);
            let p = Pixels::new(img.clone()).packed();
            assert!(p.is_packed(), "{colours} colours");
            assert!(p.stored_bytes() < 200 * 150 * 4 / 4 + 1100, "{colours}: {}", p.stored_bytes());
            assert_eq!(*p.get(), img);
            // and from the stored form, with nothing cached
            cache_forget(p.id());
            *p.0.open.lock().unwrap() = Weak::new();
            assert_eq!(*p.get(), img);
        }
        // many colours: QOI
        let img = RgbaImage::from_fn(300, 200, |x, y| image::Rgba([x as u8, y as u8, (x ^ y) as u8, 255]));
        let p = Pixels::new(img.clone()).packed();
        cache_forget(p.id());
        *p.0.open.lock().unwrap() = Weak::new();
        assert_eq!(*p.get(), img);
        // tiny ones stay raw
        assert!(!Pixels::new(RgbaImage::new(10, 10)).packed().is_packed());
    }

    #[test]
    fn packer_swaps_in_packed_frames() {
        let mut frames: Vec<crate::anim::Frame> = Vec::new();
        let mut pk = Packer::default();
        for k in 0..100u32 {
            frames.push(crate::anim::Frame::new(checker(128, 96, 3 + k % 50), 4));
            let i = frames.len() - 1;
            pk.add(&mut frames, i);
        }
        pk.finish(&mut frames);
        for (k, f) in frames.iter().enumerate() {
            assert!(f.image.is_packed());
            assert_eq!(*f.image.get(), checker(128, 96, 3 + k as u32 % 50));
        }
    }

    #[test]
    fn memory_level_scales_budgets() {
        // pure: other tests share the live setting
        assert_eq!(scale_by(64 << 20, 50), 32 << 20);
        assert_eq!(scale_by(64 << 20, 250), 160 << 20);
        assert_eq!(scale_by(64 << 20, 100), 64 << 20);
    }

    #[test]
    fn cache_stays_in_budget() {
        set_budget(16 << 20);
        let frames: Vec<Pixels> = (0..40).map(|k| Pixels::new(checker(512, 512, 2 + k)).packed()).collect();
        for f in &frames {
            let _ = f.get();
        }
        assert!(cache_bytes() <= 16 << 20, "{}", cache_bytes());
        // still right after being pushed out
        assert_eq!(*frames[0].get(), checker(512, 512, 2));
        drop(frames);
        set_budget(256 << 20);
    }
}
