//! 2-D affine maps that describe where source pixels end up after geometric
//! edits (crop, resize, flips, rotations, border/polaroid padding). Layers are
//! stored in source-frame coordinates and carried through these maps, so they
//! stay attached to the content they cover.

/// x' = a·x + b·y + tx,  y' = c·x + d·y + ty   (image coordinates, y down;
/// positive angles are clockwise on screen).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn translate(x: f32, y: f32) -> Self {
        Affine { tx: x, ty: y, ..Self::IDENTITY }
    }
    pub fn scale(sx: f32, sy: f32) -> Self {
        Affine { a: sx, d: sy, ..Self::IDENTITY }
    }
    /// Clockwise rotation (degrees) about the origin.
    pub fn rotate(deg: f32) -> Self {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: -s, c: s, d: c, tx: 0.0, ty: 0.0 }
    }
    /// `self` followed by `o`.
    pub fn then(self, o: Affine) -> Affine {
        Affine {
            a: o.a * self.a + o.b * self.c,
            b: o.a * self.b + o.b * self.d,
            c: o.c * self.a + o.d * self.c,
            d: o.c * self.b + o.d * self.d,
            tx: o.a * self.tx + o.b * self.ty + o.tx,
            ty: o.c * self.tx + o.d * self.ty + o.ty,
        }
    }
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.b * y + self.tx, self.c * x + self.d * y + self.ty)
    }
    pub fn apply_vec(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.b * y, self.c * x + self.d * y)
    }
    pub fn det(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }
    pub fn inverse(&self) -> Affine {
        let det = self.det();
        if det.abs() < 1e-9 {
            return Self::IDENTITY;
        }
        let ia = self.d / det;
        let ib = -self.b / det;
        let ic = -self.c / det;
        let id = self.a / det;
        Affine { a: ia, b: ib, c: ic, d: id, tx: -(ia * self.tx + ib * self.ty), ty: -(ic * self.tx + id * self.ty) }
    }
}

/// An affine map plus the size of the image it produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoMap {
    pub m: Affine,
    pub w: f32,
    pub h: f32,
}

impl GeoMap {
    pub fn new(w: f32, h: f32) -> Self {
        GeoMap { m: Affine::IDENTITY, w, h }
    }
    fn push(&mut self, a: Affine, w: f32, h: f32) {
        self.m = self.m.then(a);
        self.w = w;
        self.h = h;
    }
    pub fn crop(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.push(Affine::translate(-x, -y), w, h);
    }
    pub fn scale_to(&mut self, sx: f32, sy: f32, nw: f32, nh: f32) {
        self.push(Affine::scale(sx, sy), nw, nh);
    }
    pub fn offset(&mut self, dx: f32, dy: f32, nw: f32, nh: f32) {
        self.push(Affine::translate(dx, dy), nw, nh);
    }
    pub fn flip_h(&mut self) {
        let (w, h) = (self.w, self.h);
        self.push(Affine { a: -1.0, tx: w, ..Affine::IDENTITY }, w, h);
    }
    pub fn flip_v(&mut self) {
        let (w, h) = (self.w, self.h);
        self.push(Affine { d: -1.0, ty: h, ..Affine::IDENTITY }, w, h);
    }
    pub fn rot90(&mut self) {
        let (w, h) = (self.w, self.h);
        // x' = h - y, y' = x
        self.push(Affine { a: 0.0, b: -1.0, c: 1.0, d: 0.0, tx: h, ty: 0.0 }, h, w);
    }
    pub fn rot180(&mut self) {
        let (w, h) = (self.w, self.h);
        self.push(Affine { a: -1.0, d: -1.0, tx: w, ty: h, ..Affine::IDENTITY }, w, h);
    }
    pub fn rot270(&mut self) {
        let (w, h) = (self.w, self.h);
        // x' = y, y' = w - x
        self.push(Affine { a: 0.0, b: 1.0, c: -1.0, d: 0.0, tx: 0.0, ty: w }, h, w);
    }
    /// Matches `ops::rotate_any_image`.
    pub fn rot_any(&mut self, deg: f32, expand: bool) {
        let (w, h) = (self.w, self.h);
        let (s, c) = deg.to_radians().sin_cos();
        let (nw, nh) = if expand { ((w * c.abs() + h * s.abs()).ceil(), (w * s.abs() + h * c.abs()).ceil()) } else { (w, h) };
        let a = Affine::translate(-w / 2.0, -h / 2.0).then(Affine::rotate(deg)).then(Affine::translate(nw / 2.0, nh / 2.0));
        self.push(a, nw, nh);
    }
}

/// An oriented box in some image space (centre, size, clockwise angle in degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obb {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub angle: f32,
}

impl Obb {
    pub fn from_rect(x: f32, y: f32, w: f32, h: f32, angle: f32) -> Self {
        Obb { cx: x + w / 2.0, cy: y + h / 2.0, w, h, angle }
    }
    /// Point in local (unrotated, centred) coordinates.
    pub fn to_local(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, c) = (-self.angle).to_radians().sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (c * dx - s * dy, s * dx + c * dy)
    }
    pub fn from_local(&self, lx: f32, ly: f32) -> (f32, f32) {
        let (s, c) = self.angle.to_radians().sin_cos();
        (self.cx + c * lx - s * ly, self.cy + s * lx + c * ly)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (lx, ly) = self.to_local(x, y);
        lx.abs() <= self.w / 2.0 && ly.abs() <= self.h / 2.0
    }
    pub fn corners(&self) -> [(f32, f32); 4] {
        let (hw, hh) = (self.w / 2.0, self.h / 2.0);
        [self.from_local(-hw, -hh), self.from_local(hw, -hh), self.from_local(hw, hh), self.from_local(-hw, hh)]
    }
    /// Axis-aligned bounds (x0, y0, x1, y1).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let cs = self.corners();
        let x0 = cs.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let y0 = cs.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let x1 = cs.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let y1 = cs.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        (x0, y0, x1, y1)
    }
}

fn norm_deg(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    }
    if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// Extra rotation applied to readable content under a mirroring map, chosen
/// per map (not per box) so it stays a bijection: upright content stays upright.
fn readable_correction(m: &Affine, readable: bool) -> f32 {
    if readable && m.det() < 0.0 {
        let (x, _y) = m.apply_vec(1.0, 0.0);
        if x < -1e-4 {
            return 180.0;
        }
    }
    0.0
}

/// Map a source-space box through `m`. With `readable`, content is not shown
/// upside-down after a mirroring map (text / images stay legible after flips).
pub fn map_box(m: &Affine, b: Obb, readable: bool) -> Obb {
    let (cx, cy) = m.apply(b.cx, b.cy);
    let (s, c) = b.angle.to_radians().sin_cos();
    let u = m.apply_vec(c, s);
    let v = m.apply_vec(-s, c);
    let su = (u.0 * u.0 + u.1 * u.1).sqrt();
    let sv = (v.0 * v.0 + v.1 * v.1).sqrt();
    let ang = u.1.atan2(u.0).to_degrees() + readable_correction(m, readable);
    Obb { cx, cy, w: b.w * su, h: b.h * sv, angle: norm_deg(ang) }
}

/// Source-space angle whose mapped (displayed) angle is `out_angle`.
pub fn source_angle(m: &Affine, out_angle: f32, readable: bool) -> f32 {
    let phi = out_angle - readable_correction(m, readable);
    let (s, c) = phi.to_radians().sin_cos();
    let (x, y) = m.inverse().apply_vec(c, s);
    norm_deg(y.atan2(x).to_degrees())
}

/// Convert an edited output-space box back to source space, keeping the
/// source angle `src_angle` (only centre and size change).
pub fn unmap_box(m: &Affine, out: Obb, src_angle: f32) -> Obb {
    let (cx, cy) = m.inverse().apply(out.cx, out.cy);
    let (s, c) = src_angle.to_radians().sin_cos();
    let u = m.apply_vec(c, s);
    let v = m.apply_vec(-s, c);
    let su = (u.0 * u.0 + u.1 * u.1).sqrt().max(1e-6);
    let sv = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-6);
    Obb { cx, cy, w: out.w / su, h: out.h / sv, angle: src_angle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot90_matches_imageops() {
        let mut g = GeoMap::new(100.0, 50.0);
        g.rot90();
        assert_eq!((g.w, g.h), (50.0, 100.0));
        // top-left corner goes to top-right
        let (x, y) = g.m.apply(0.0, 0.0);
        assert!((x - 50.0).abs() < 1e-4 && y.abs() < 1e-4);
    }

    #[test]
    fn roundtrip_box() {
        let mut g = GeoMap::new(200.0, 100.0);
        g.crop(10.0, 5.0, 150.0, 90.0);
        g.scale_to(2.0, 2.0, 300.0, 180.0);
        g.rot_any(30.0, true);
        g.flip_h();
        let b = Obb::from_rect(20.0, 30.0, 40.0, 10.0, 15.0);
        let o = map_box(&g.m, b, true);
        let back = unmap_box(&g.m, o, b.angle);
        assert!((back.cx - b.cx).abs() < 1e-3 && (back.cy - b.cy).abs() < 1e-3);
        assert!((back.w - b.w).abs() < 1e-3 && (back.h - b.h).abs() < 1e-3);
        let th = source_angle(&g.m, o.angle, true);
        assert!(norm_deg(th - b.angle).abs() < 0.01, "{th} vs {}", b.angle);
        // horizontal flip keeps text upright
        let mut f = GeoMap::new(100.0, 100.0);
        f.flip_h();
        assert!(map_box(&f.m, Obb::from_rect(0.0, 0.0, 10.0, 10.0, 0.0), true).angle.abs() < 1e-3);
    }
}
