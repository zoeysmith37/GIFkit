//! Rulers along the views, guide lines dragged out of them, and smart
//! guides that moved layers snap to.

use super::*;
use crate::stack::Guide;

/// A round ruler step for this zoom, at least `min_px` apart on screen.
pub(super) fn ruler_step(scale: f32, min_px: f32) -> f32 {
    let mut base = 1.0f32;
    while base < 1e7 {
        for k in [1.0, 2.0, 5.0] {
            if base * k * scale >= min_px {
                return base * k;
            }
        }
        base *= 10.0;
    }
    base
}

/// The closest of `targets` to any of `edges`, within `thr`: how far to
/// move, and the line it lines up with.
pub(super) fn nearest_snap(edges: &[f32], targets: &[f32], thr: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &e in edges {
        for &t in targets {
            let d = t - e;
            if d.abs() <= thr && best.is_none_or(|b| d.abs() < b.0.abs()) {
                best = Some((d, t));
            }
        }
    }
    best
}

/// Smart guide colour, like the magenta other editors use.
const SMART: Color32 = Color32::from_rgb(236, 64, 160);

impl GifKitApp {
    /// What a layer being moved can line up with, as seen in the output:
    /// the frame's edges and middle, the guides, and every other layer's
    /// edges and middle. Also the other layers' boxes, for the gaps.
    pub(super) fn snap_targets(&self, skip: usize, bounds: (f32, f32)) -> (Vec<f32>, Vec<f32>, Vec<[f32; 4]>) {
        let (mut xs, mut ys) = (vec![0.0, bounds.0 / 2.0, bounds.0], vec![0.0, bounds.1 / 2.0, bounds.1]);
        if self.show_guides {
            for g in &self.stack.guides {
                if g.vertical { xs.push(g.at) } else { ys.push(g.at) }
            }
        }
        let mut boxes = Vec::new();
        let (bw, bh) = self.dims();
        let maps = self.stack.layer_maps(bw, bh);
        let src_idx = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        for (li, l) in self.stack.layers.iter().enumerate() {
            if li == skip || !l.visible || !l.timing.shows(self.cur, self.plan.len()) {
                continue;
            }
            let Some(b) = l.source_box(src_idx, &self.fonts) else { continue };
            let b = if l.timing.keys.is_empty() { b } else { l.timing.motion(self.cur).apply(b) };
            let o = map_box(&maps[li].m, b, l.readable());
            let (x0, x1, y0, y1) = obb_extent(&o);
            xs.extend([x0, (x0 + x1) / 2.0, x1]);
            ys.extend([y0, (y0 + y1) / 2.0, y1]);
            boxes.push([x0, x1, y0, y1]);
        }
        (xs, ys, boxes)
    }

    /// Whether moving things snap to anything right now.
    pub(super) fn snapping(&self, ui: &egui::Ui) -> bool {
        (self.smart_guides || (self.show_grid && self.grid_snap)) && !ui.input(|i| i.modifiers.command)
    }

    /// Lines up moving edges (`ex` across, `ey` down; None leaves that way
    /// alone) with the smart guide targets and the grid. Gives the shift to
    /// make, and lights the lines it lined up with.
    pub(super) fn snap_edges(&mut self, skip: usize, bounds: (f32, f32), ex: &[f32], ey: &[f32], scale: f32) -> (f32, f32, Vec<[f32; 4]>) {
        let thr = 6.0 / scale;
        let (mut xs, mut ys, boxes) = if self.smart_guides { self.snap_targets(skip, bounds) } else { (vec![], vec![], vec![]) };
        let n_smart = (xs.len(), ys.len());
        if self.show_grid && self.grid_snap {
            let g = self.grid_size.max(1) as f32;
            xs.extend(ex.iter().map(|e| (e / g).round() * g));
            ys.extend(ey.iter().map(|e| (e / g).round() * g));
        }
        self.snap_lines.clear();
        let mut out = (0.0, 0.0);
        if let Some((d, t)) = nearest_snap(ex, &xs, thr) {
            out.0 = d;
            if xs[..n_smart.0].contains(&t) {
                self.snap_lines.push((true, t));
            }
        }
        if let Some((d, t)) = nearest_snap(ey, &ys, thr) {
            out.1 = d;
            if ys[..n_smart.1].contains(&t) {
                self.snap_lines.push((false, t));
            }
        }
        (out.0, out.1, boxes)
    }

    /// The gaps from a moved box to the nearest layer on each side, shown
    /// while it moves.
    pub(super) fn note_gaps(&mut self, b: [f32; 4], boxes: &[[f32; 4]]) {
        self.snap_gaps.clear();
        let [x0, x1, y0, y1] = b;
        let mut best: [Option<(f32, [f32; 4])>; 4] = [None; 4];
        let mut keep = |k: usize, d: f32, seg: [f32; 4]| {
            if d > 0.5 && best[k].is_none_or(|(bd, _)| d < bd) {
                best[k] = Some((d, seg));
            }
        };
        for o in boxes {
            let (oy0, oy1) = (o[2].max(y0), o[3].min(y1));
            if oy1 > oy0 {
                let ym = (oy0 + oy1) / 2.0;
                keep(0, x0 - o[1], [o[1], ym, x0, ym]);
                keep(1, o[0] - x1, [x1, ym, o[0], ym]);
            }
            let (ox0, ox1) = (o[0].max(x0), o[1].min(x1));
            if ox1 > ox0 {
                let xm = (ox0 + ox1) / 2.0;
                keep(2, y0 - o[3], [xm, o[3], xm, y0]);
                keep(3, o[2] - y1, [xm, y1, xm, o[2]]);
            }
        }
        self.snap_gaps = best.into_iter().flatten().map(|(_, s)| s).collect();
    }

    /// The rulers, the guides and the smart guide lines for view `vi`.
    /// `full` is the view with its rulers, `inner` the picture's part.
    pub(super) fn guides_ui(&mut self, ui: &mut egui::Ui, full: Rect, inner: Rect, vi: usize) {
        if self.base.is_none() {
            return;
        }
        let Some((m, size)) = self.views[vi].shown else {
            if self.show_rulers {
                ui.painter().rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, inner.min.y)), 0.0, p().window);
                ui.painter().rect_filled(Rect::from_min_max(full.min, pos2(inner.min.x, full.max.y)), 0.0, p().window);
            }
            return;
        };
        let pal = *p();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        let ctrl = ui.input(|i| i.modifiers.command);
        let pic = ui.painter_at(inner);
        let guide_col = Color32::from_rgb(0, 200, 255);

        // ---- the grid, under everything else
        if self.show_grid {
            let g = self.grid_size.max(1) as f32;
            // every line, or every few when they'd crowd together
            let every = (6.0 / (g * m.scale)).ceil().max(1.0);
            let step = g * every;
            let area = m.rect.intersect(inner);
            let col = Color32::from_white_alpha(38);
            let dark = Color32::from_black_alpha(38);
            let lo = m.to_img(area.min);
            let hi = m.to_img(area.max);
            let mut x = (lo.x / step).ceil() * step;
            while x <= hi.x {
                let sx = pic.round_to_pixel_center(m.to_screen(pos2(x, 0.0)).x);
                pic.line_segment([pos2(sx, area.top()), pos2(sx, area.bottom())], Stroke::new(1.0, dark));
                pic.line_segment([pos2(sx + 1.0, area.top()), pos2(sx + 1.0, area.bottom())], Stroke::new(1.0, col));
                x += step;
            }
            let mut y = (lo.y / step).ceil() * step;
            while y <= hi.y {
                let sy = pic.round_to_pixel_center(m.to_screen(pos2(0.0, y)).y);
                pic.line_segment([pos2(area.left(), sy), pos2(area.right(), sy)], Stroke::new(1.0, dark));
                pic.line_segment([pos2(area.left(), sy + 1.0), pos2(area.right(), sy + 1.0)], Stroke::new(1.0, col));
                y += step;
            }
        }

        // ---- the guides, and grabbing them
        if self.show_guides {
            for (i, g) in self.stack.guides.iter().enumerate() {
                let (a, b) = if g.vertical {
                    let x = pic.round_to_pixel_center(m.to_screen(pos2(g.at, 0.0)).x);
                    (pos2(x, inner.top()), pos2(x, inner.bottom()))
                } else {
                    let y = pic.round_to_pixel_center(m.to_screen(pos2(0.0, g.at)).y);
                    (pos2(inner.left(), y), pos2(inner.right(), y))
                };
                let hot = self.guide_drag.is_some_and(|d| d.vi == vi && d.idx == Some(i));
                pic.line_segment([a, b], Stroke::new(if hot { 2.0 } else { 1.0 }, guide_col));
            }
            // grabbed after the picture so they win over it
            if self.guide_drag.is_none() {
                let mut grab = None;
                for (i, g) in self.stack.guides.iter().enumerate() {
                    let hit = if g.vertical {
                        let x = m.to_screen(pos2(g.at, 0.0)).x;
                        Rect::from_min_max(pos2(x - 3.0, inner.top()), pos2(x + 3.0, inner.bottom()))
                    } else {
                        let y = m.to_screen(pos2(0.0, g.at)).y;
                        Rect::from_min_max(pos2(inner.left(), y - 3.0), pos2(inner.right(), y + 3.0))
                    }
                    .intersect(inner);
                    if !hit.is_positive() {
                        continue;
                    }
                    let r = ui.interact(hit, egui::Id::unique(("gifkit-guide", vi, i)), Sense::drag());
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(if g.vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
                    }
                    if r.drag_started() {
                        grab = Some(GuideDrag { vi, vertical: g.vertical, idx: Some(i) });
                    }
                }
                if let Some(g) = grab {
                    let now = ui.input(|i| i.time);
                    self.snapshot(&format!("guide{now}"), now);
                    self.guide_drag = Some(g);
                }
            }
        }

        // ---- smart guide lines from a layer being moved
        if !down {
            self.snap_lines.clear();
            self.snap_gaps.clear();
        }
        if self.active_view == vi {
            for g in &self.snap_gaps {
                let (a, b) = (m.to_screen(pos2(g[0], g[1])), m.to_screen(pos2(g[2], g[3])));
                pic.line_segment([a, b], Stroke::new(1.0, SMART));
                let across = a.y == b.y;
                for e in [a, b] {
                    let t = if across { vec2(0.0, 4.0) } else { vec2(4.0, 0.0) };
                    pic.line_segment([e - t, e + t], Stroke::new(1.0, SMART));
                }
                let d = ((g[2] - g[0]).abs() + (g[3] - g[1]).abs()).round();
                let tag = pic.layout_no_wrap(format!("{d}"), egui::FontId::proportional(11.0), Color32::WHITE);
                let mid = a + (b - a) / 2.0 + if across { vec2(0.0, -11.0) } else { vec2(14.0, 0.0) };
                let bg = Rect::from_center_size(mid, tag.size() + vec2(8.0, 2.0));
                pic.rect_filled(bg, 3.0, SMART);
                pic.galley(bg.min + vec2(4.0, 1.0), tag, Color32::WHITE);
            }
            for &(vertical, at) in &self.snap_lines {
                let s = m.to_screen(pos2(at, at));
                let s = pos2(pic.round_to_pixel_center(s.x), pic.round_to_pixel_center(s.y));
                let seg = if vertical { [pos2(s.x, inner.top()), pos2(s.x, inner.bottom())] } else { [pos2(inner.left(), s.y), pos2(inner.right(), s.y)] };
                pic.line_segment(seg, Stroke::new(1.0, SMART));
            }
        }

        // ---- the rulers
        let top = Rect::from_min_max(pos2(inner.left(), full.top()), pos2(full.right(), inner.top()));
        let left = Rect::from_min_max(pos2(full.left(), inner.top()), pos2(inner.left(), full.bottom()));
        if self.show_rulers {
            let painter = ui.painter_at(full);
            let font = egui::FontId::proportional(10.0);
            let step = ruler_step(m.scale, 64.0);
            let minor_n = if step * m.scale >= 120.0 { 10.0 } else { 5.0 };
            let minor = step / minor_n;
            for (bar, horiz) in [(top, true), (left, false)] {
                painter.rect_filled(bar, 0.0, pal.window);
                // the picture's own stretch, a shade lighter
                let (a, b) = (m.to_screen(pos2(0.0, 0.0)), m.to_screen(pos2(size.x, size.y)));
                let span = if horiz { Rect::from_min_max(pos2(a.x.max(bar.left()), bar.top()), pos2(b.x.min(bar.right()), bar.bottom())) } else { Rect::from_min_max(pos2(bar.left(), a.y.max(bar.top())), pos2(bar.right(), b.y.min(bar.bottom()))) };
                if span.is_positive() {
                    painter.rect_filled(span, 0.0, mix(pal.window, pal.fg, 0.06));
                }
                let (lo, hi) = if horiz { (m.to_img(pos2(bar.left(), 0.0)).x, m.to_img(pos2(bar.right(), 0.0)).x) } else { (m.to_img(pos2(0.0, bar.top())).y, m.to_img(pos2(0.0, bar.bottom())).y) };
                let k0 = (lo / minor).floor() as i64;
                let k1 = (hi / minor).ceil() as i64;
                let edge = if horiz { bar.bottom() } else { bar.right() };
                let p = painter.with_clip_rect(bar);
                for k in k0..=k1 {
                    let v = k as f32 * minor;
                    let major = k.rem_euclid(minor_n as i64) == 0;
                    let half = !major && minor_n == 10.0 && k.rem_euclid(5) == 0;
                    let len = if major { RULER - 4.0 } else if half { 7.0 } else { 4.0 };
                    let col = if major { mix(pal.fg, pal.window, 0.35) } else { mix(pal.fg, pal.window, 0.6) };
                    let s = m.to_screen(pos2(v, v));
                    if horiz {
                        p.line_segment([pos2(s.x.round() + 0.5, edge), pos2(s.x.round() + 0.5, edge - len)], Stroke::new(1.0, col));
                    } else {
                        p.line_segment([pos2(edge, s.y.round() + 0.5), pos2(edge - len, s.y.round() + 0.5)], Stroke::new(1.0, col));
                    }
                    if major {
                        let g = p.layout_no_wrap(format!("{}", v.round() as i64), font.clone(), pal.fg_muted);
                        if horiz {
                            p.galley(pos2(s.x + 3.0, bar.top() + 1.0), g, pal.fg_muted);
                        } else {
                            // read bottom to top, as rulers do
                            let at = pos2(bar.left() + 1.0, s.y - 3.0);
                            p.add(egui::epaint::TextShape::new(at, g, pal.fg_muted).with_angle(-std::f32::consts::FRAC_PI_2));
                        }
                    }
                }
                // where the pointer is
                if let Some(pp) = pointer.filter(|pp| inner.contains(*pp)) {
                    let seg = if horiz { [pos2(pp.x, bar.top()), pos2(pp.x, bar.bottom())] } else { [pos2(bar.left(), pp.y), pos2(bar.right(), pp.y)] };
                    p.line_segment(seg, Stroke::new(1.0, pal.highlight));
                }
                let line = if horiz { [pos2(bar.left(), edge - 0.5), pos2(bar.right(), edge - 0.5)] } else { [pos2(edge - 0.5, bar.top()), pos2(edge - 0.5, bar.bottom())] };
                painter.line_segment(line, Stroke::new(1.0, pal.border));
            }
            let corner = Rect::from_min_max(full.min, inner.min);
            painter.rect_filled(corner, 0.0, pal.window);
            painter.line_segment([pos2(corner.right() - 0.5, corner.top()), pos2(corner.right() - 0.5, corner.bottom())], Stroke::new(1.0, pal.border));
            painter.line_segment([pos2(corner.left(), corner.bottom() - 0.5), pos2(corner.right(), corner.bottom() - 0.5)], Stroke::new(1.0, pal.border));

            // a guide pulled out of a ruler: the top one makes a level guide
            if self.guide_drag.is_none() {
                for (bar, vertical, name) in [(top, false, "top"), (left, true, "left")] {
                    let r = ui.interact(bar, egui::Id::unique(("gifkit-ruler", vi, name)), Sense::drag()).on_hover_text("Drag into the picture to add a guide");
                    if r.drag_started() {
                        let now = ui.input(|i| i.time);
                        self.snapshot(&format!("guide{now}"), now);
                        self.show_guides = true;
                        self.guide_drag = Some(GuideDrag { vi, vertical, idx: None });
                    }
                }
            }
        }

        // ---- a guide on the move
        let Some(mut d) = self.guide_drag.filter(|d| d.vi == vi) else { return };
        let Some(pp) = ui.input(|i| i.pointer.latest_pos()) else { return };
        let over_picture = inner.contains(pp);
        if down {
            let img = m.to_img(pp);
            let mut at = (if d.vertical { img.x } else { img.y }).round();
            if !ctrl {
                // the frame's edges and middle pull it in
                let len = if d.vertical { size.x } else { size.y };
                if let Some((dv, _)) = nearest_snap(&[at], &[0.0, (len / 2.0).round(), len], 6.0 / m.scale) {
                    at += dv;
                }
            }
            match d.idx {
                Some(i) if i < self.stack.guides.len() => self.stack.guides[i].at = at,
                Some(_) => {}
                None if over_picture => {
                    self.stack.guides.push(Guide { vertical: d.vertical, at });
                    d.idx = Some(self.stack.guides.len() - 1);
                }
                None => {}
            }
            ui.ctx().set_cursor_icon(if d.vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
            // its place beside the pointer
            let tag = ui.painter().layout_no_wrap(format!("{} {}", if d.vertical { "X" } else { "Y" }, at as i64), theme::medium(11.5), Color32::WHITE);
            let bg = Rect::from_min_size(pp + vec2(14.0, 14.0), tag.size() + vec2(12.0, 6.0));
            ui.painter().rect_filled(bg, 3.0, Color32::from_black_alpha(190));
            ui.painter().galley(bg.min + vec2(6.0, 3.0), tag, Color32::WHITE);
            self.guide_drag = Some(d);
            ui.ctx().request_repaint();
        } else {
            // let go: dropped back on a ruler (or outside) takes it away
            if let Some(i) = d.idx.filter(|i| *i < self.stack.guides.len()) {
                if !over_picture {
                    self.stack.guides.remove(i);
                    self.info("Guide removed");
                }
                self.modified = true;
            }
            self.guide_drag = None;
        }
    }
}

/// The left, right, top and bottom of a box as seen.
pub(super) fn obb_extent(o: &Obb) -> (f32, f32, f32, f32) {
    let c = o.corners();
    let xs = c.map(|p| p.0);
    let ys = c.map(|p| p.1);
    (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max))
}
