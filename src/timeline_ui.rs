//! The timeline dock under the viewport, in the spirit of Kdenlive: a ruler
//! with the playhead, the GIF's own track, and one track per layer whose
//! clips can be moved, trimmed, split and deleted, with keyframes on top.

use super::*;
use crate::timeline::{Clip, Key};

/// The GIF track's id in the timeline (layers use their own ids, which start at 1).
const GIF: u64 = 0;
const RULER_H: f32 = 22.0;
const GIF_H: f32 = 40.0;
const ROW_H: f32 = 28.0;
const EDGE: f32 = 5.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum TlGrab {
    Playhead,
    Body,
    Start,
    End,
    Key(usize),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TlDrag {
    track: u64,
    clip: usize,
    grab: TlGrab,
    /// frame under the pointer when the drag began
    at: f32,
    /// the clip (first, last) or key frame when the drag began
    orig: (usize, usize),
}

/// Track colours by layer kind, mixed into the view colour.
fn kind_colour(k: Kind) -> Color32 {
    match k {
        Kind::Effects => Color32::from_rgb(155, 89, 182),
        Kind::Text => Color32::from_rgb(61, 174, 233),
        Kind::Censor => Color32::from_rgb(127, 140, 141),
        Kind::Overlay => Color32::from_rgb(26, 188, 156),
        Kind::Watermark => Color32::from_rgb(93, 109, 126),
        Kind::Shield => Color32::from_rgb(39, 174, 96),
        Kind::Shape => Color32::from_rgb(243, 156, 18),
        Kind::Markup => Color32::from_rgb(233, 30, 99),
        Kind::Emoji => Color32::from_rgb(241, 196, 15),
    }
}

fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Color32, stroke: Stroke) {
    let pts = vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)];
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

impl GifKitApp {
    /// Runs of output frames the GIF actually shows (skipped source frames are gaps).
    fn gif_runs(&self) -> Vec<(usize, usize)> {
        let Some(base) = &self.base else { return Vec::new() };
        let shown: Vec<bool> = self.plan.iter().map(|f| base.frames[f.src].enabled).collect();
        let mut v = Vec::new();
        let mut i = 0;
        while i < shown.len() {
            if shown[i] {
                let s = i;
                while i + 1 < shown.len() && shown[i + 1] {
                    i += 1;
                }
                v.push((s, i));
            }
            i += 1;
        }
        v
    }

    /// The clips of a track as (first, last); the GIF's are its shown runs.
    fn track_spans(&self, track: u64) -> Vec<(usize, usize)> {
        let n = self.plan.len();
        if track == GIF {
            return self.gif_runs();
        }
        let Some(li) = self.stack.index_of(track) else { return Vec::new() };
        let l = &self.stack.layers[li];
        match &l.timing.clips {
            Some(_) => l.timing.spans(n),
            None => {
                // older layers: show their frame range as clips
                let mut t = crate::timeline::Timing::default();
                t.take_clips(&self.stack.legacy_shown(li, &self.plan));
                t.spans(n)
            }
        }
    }

    /// Show or skip the source frames behind output frames `a..=b`.
    fn gif_set(&mut self, a: usize, b: usize, on: bool) {
        let Some(base) = self.base.as_mut() else { return };
        for i in a..=b.min(self.plan.len().saturating_sub(1)) {
            base.frames[self.plan[i].src].enabled = on;
        }
        self.timing_rev += 1;
        self.after_base_change(false);
    }

    fn tl_layer_mut(&mut self, track: u64) -> Option<&mut stack::Layer> {
        let li = self.stack.index_of(track)?;
        self.stack.ensure_clips(li, &self.plan);
        self.stack.layers.get_mut(li)
    }

    /// Split the selected track at the playhead.
    fn tl_split(&mut self, now: f64) {
        let (n, f) = (self.plan.len(), self.cur);
        let Some(track) = self.tl_track else { return };
        self.snapshot(&format!("tl-split{now}"), now);
        if track == GIF {
            // the GIF has nothing to split into: a skipped frame makes the cut
            return;
        }
        let done = self.tl_layer_mut(track).map(|l| l.timing.split(f, n).then(|| l.timing.spans(n).iter().position(|s| s.0 == f))).flatten();
        if let Some(c) = done {
            self.modified = true;
            self.tl_clip = c;
        }
    }

    fn tl_delete(&mut self, now: f64) {
        let Some(track) = self.tl_track else { return };
        let n = self.plan.len();
        if let Some(k) = self.tl_key {
            self.snapshot(&format!("tl-delkey{now}"), now);
            if let Some(l) = self.tl_layer_mut(track) {
                if k < l.timing.keys.len() {
                    l.timing.keys.remove(k);
                }
            }
            self.tl_key = None;
            self.modified = true;
            return;
        }
        let Some(c) = self.tl_clip else { return };
        let spans = self.track_spans(track);
        let Some(&(a, b)) = spans.get(c) else { return };
        self.snapshot(&format!("tl-del{now}"), now);
        if track == GIF {
            if spans.len() > 1 || a > 0 || b + 1 < n {
                self.gif_set(a, b, false);
            }
        } else if let Some(l) = self.tl_layer_mut(track) {
            if let Some(cl) = &mut l.timing.clips {
                // clips are kept sorted, so the index matches the spans
                cl.sort_by_key(|c| c.start);
                if c < cl.len() {
                    cl.remove(c);
                }
            }
            self.modified = true;
        }
        self.tl_clip = None;
    }

    /// Add a clip on the selected layer at the playhead, filling the gap it's in.
    fn tl_add_clip(&mut self, now: f64) {
        let Some(track) = self.tl_track.filter(|t| *t != GIF) else { return };
        let (n, f) = (self.plan.len(), self.cur);
        let spans = self.track_spans(track);
        if spans.iter().any(|(a, b)| f >= *a && f <= *b) {
            return;
        }
        let lo = spans.iter().filter(|s| s.1 < f).map(|s| s.1 + 1).max().unwrap_or(0);
        let hi = spans.iter().filter(|s| s.0 > f).map(|s| s.0 - 1).min().unwrap_or(n.saturating_sub(1));
        // about a second, inside the gap
        let len = (n / 4).clamp(1, 25);
        let start = f.max(lo);
        let last = (start + len - 1).min(hi);
        self.snapshot(&format!("tl-add{now}"), now);
        if let Some(l) = self.tl_layer_mut(track) {
            let cl = l.timing.clips.get_or_insert_with(Vec::new);
            cl.push(Clip { start, end: if last + 1 >= n { None } else { Some(last) } });
            cl.sort_by_key(|c| c.start);
            self.tl_clip = l.timing.spans(n).iter().position(|s| s.0 == start);
        }
        self.modified = true;
    }

    fn tl_add_key(&mut self, now: f64) {
        let Some(track) = self.tl_track.filter(|t| *t != GIF) else { return };
        let f = self.cur;
        self.snapshot(&format!("tl-key{now}"), now);
        if let Some(l) = self.stack.index_of(track).and_then(|li| self.stack.layers.get_mut(li)) {
            let k = l.timing.add_key(f);
            self.tl_key = Some(k);
            self.modified = true;
        }
    }

    pub(super) fn timeline_dock(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = *p();
        let now = ctx.input(|i| i.time);
        let n = self.plan.len();
        if n == 0 {
            return;
        }
        // tracks: the GIF, then layers top-most first, as in the layer list
        let tracks: Vec<u64> = std::iter::once(GIF).chain(self.stack.layers.iter().rev().map(|l| l.id)).collect();
        if self.tl_track.is_some_and(|t| !tracks.contains(&t)) {
            self.tl_track = None;
        }
        let idle = !self.busy();
        let sel_layer = self.tl_track.filter(|t| *t != GIF);

        // ---- toolbar
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(RichText::new("Timeline").font(theme::medium(13.5)).color(pal.fg));
            ui.add_space(8.0);
            // the two tools, as in Kdenlive: select and move, or cut
            if tb_toggle(ui, "edit-select", !self.tl_razor, "Selection tool: move and trim clips (X switches)") {
                self.tl_razor = false;
            }
            if tb_toggle(ui, "edit-cut", self.tl_razor, "Razor tool: click a clip to cut it there (X switches)") {
                self.tl_razor = true;
            }
            toolbar_sep(ui);
            if tb_tool(ui, "node-break", "", "Split the selected track at the playhead (S)", idle && sel_layer.is_some()).clicked() {
                self.tl_split(now);
            }
            if tb_tool(ui, "list-add", "", "Add a clip at the playhead (on an empty part of the selected track)", idle && sel_layer.is_some()).clicked() {
                self.tl_add_clip(now);
            }
            if tb_tool(ui, "edit-delete", "", "Delete the selected clip or keyframe (Delete). On the GIF track this skips its frames.", idle && (self.tl_clip.is_some() || self.tl_key.is_some())).clicked() {
                self.tl_delete(now);
            }
            toolbar_sep(ui);
            if tb_tool(ui, "node-add", "", "Add a keyframe on the selected layer at the playhead (K)", idle && sel_layer.is_some()).clicked() {
                self.tl_add_key(now);
            }
            let on_key = sel_layer.and_then(|t| self.stack.index_of(t)).is_some_and(|li| self.stack.layers[li].timing.key_at(self.cur).is_some());
            if tb_tool(ui, "node-delete", "", "Remove the keyframe at the playhead", idle && on_key).clicked() {
                self.tl_delete(now);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if tb_tool(ui, "go-down", "", "Hide the timeline (the clock button by the play controls brings it back)", true).clicked() {
                    self.show_timeline = false;
                    theme::set_setting("show_timeline", "0");
                }
                toolbar_sep(ui);
                if tb_tool(ui, "zoom-fit-best", "", "Fit the whole animation", true).clicked() {
                    self.tl_zoom = 0.0;
                    self.tl_x0 = 0.0;
                }
                if tb_tool(ui, "zoom-in", "", "Zoom in (Ctrl+wheel)", true).clicked() {
                    self.tl_zoom = (self.tl_zoom.max(1.0) * 1.5).min(120.0);
                }
                if tb_tool(ui, "zoom-out", "", "Zoom out (Ctrl+wheel)", true).clicked() {
                    self.tl_zoom = (self.tl_zoom / 1.5).max(0.0);
                }
                let t = self.plan_time(self.cur);
                ui.label(RichText::new(format!("{:.2} s · frame {} of {n}", t.max(0.0), self.cur + 1)).monospace().size(12.0).color(pal.fg_muted));
            });
        });
        // the selected layer's motion at the playhead. Changing a value makes
        // a keyframe there if there isn't one (auto key), so the row never
        // greys out or changes shape.
        let sel_li = sel_layer.and_then(|t| self.stack.index_of(t));
        let key_here = sel_li.and_then(|li| self.stack.layers[li].timing.key_at(self.cur).map(|k| (li, k)));
        if self.tl_drag.is_none() {
            self.tl_key = key_here.map(|(_, k)| k);
        }
        let mut key = match (key_here, sel_li) {
            (Some((li, k)), _) => self.stack.layers[li].timing.keys[k],
            (None, Some(li)) => {
                let m = self.stack.layers[li].timing.motion(self.cur);
                Key { frame: self.cur, x: m.dx, y: m.dy, scale: m.scale * 100.0, angle: m.angle, opacity: m.opacity * 100.0, smooth: true }
            }
            _ => Key::default(),
        };
        let before = key;
        ui.add_enabled_ui(sel_li.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, CHIP_H), Sense::hover());
                // filled on a keyframe, hollow between them
                if key_here.is_some() {
                    diamond(ui.painter(), r.center(), 6.0, pal.highlight, Stroke::NONE);
                } else {
                    diamond(ui.painter(), r.center(), 5.5, Color32::TRANSPARENT, Stroke::new(1.3, pal.fg_muted));
                }
                ui.label("Motion:").on_hover_text("Changing a value adds a keyframe at the playhead");
                let field = |ui: &mut egui::Ui, label: &str, v: &mut f32, suffix: &str, step: f64| {
                    ui.label(label);
                    spin(ui, vec2(86.0, CHIP_H), v, step, |d| d.speed(0.5).max_decimals(1).suffix(suffix.to_string()));
                };
                field(ui, "X:", &mut key.x, " px", 1.0);
                field(ui, "Y:", &mut key.y, " px", 1.0);
                field(ui, "Size:", &mut key.scale, "%", 5.0);
                field(ui, "Angle:", &mut key.angle, "°", 5.0);
                ui.label("Opacity:");
                spin(ui, vec2(80.0, CHIP_H), &mut key.opacity, 5.0, |d| d.range(0.0..=100.0).max_decimals(0).suffix("%"));
                check(ui, &mut key.smooth, "Smooth").on_hover_text("Ease in and out of this keyframe");
            });
        });
        if let Some(li) = sel_li {
            if key != before {
                self.snapshot(&format!("tl-keyedit{li}"), now);
                let t = &mut self.stack.layers[li].timing;
                let k = t.add_key(self.cur);
                t.keys[k] = Key { frame: self.cur, ..key };
                self.tl_key = Some(k);
                self.modified = true;
            }
        }
        ui.add_space(2.0);

        // ---- the track area
        let area = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, pal.view);
        let head_w = self.tl_head_w.clamp(90.0, (area.width() - 120.0).max(90.0));
        let lanes = Rect::from_min_max(pos2(area.min.x + head_w, area.min.y), area.max);
        let lane_w = lanes.width().max(10.0);
        // frames to pixels; zoom 0 fits everything, with a little room after
        // the end to drag the GIF longer into
        let span = (n as f32 * 1.08 + 2.0).max(n as f32 + 24.0);
        let fit = lane_w / span;
        let zoom = if self.tl_zoom <= 0.0 { fit } else { self.tl_zoom.max(fit) };
        let visible = lane_w / zoom;
        // follow the playhead while playing
        if self.playing && self.tl_drag.is_none() && ((self.cur as f32) < self.tl_x0 || self.cur as f32 + 1.0 > self.tl_x0 + visible) {
            self.tl_x0 = self.cur as f32 - visible * 0.1;
        }
        self.tl_x0 = self.tl_x0.clamp(0.0, (span - visible).max(0.0));
        let x0 = self.tl_x0;
        let fx = |f: f32| lanes.min.x + (f - x0) * zoom;
        // wheel: scroll tracks, Shift pans, Ctrl zooms about the pointer
        if resp.hovered() {
            let (scroll, ctrl, shift) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command, i.modifiers.shift));
            if ctrl && scroll.y != 0.0 {
                let at = ui.input(|i| i.pointer.hover_pos()).map(|p| x0 + (p.x - lanes.min.x) / zoom).unwrap_or(x0);
                let nz = (zoom * (1.0 + scroll.y * 0.004)).clamp(fit, 120.0);
                self.tl_zoom = nz;
                self.tl_x0 = at - (at - x0) * zoom / nz;
            } else if shift || scroll.x != 0.0 {
                self.tl_x0 -= (scroll.x + if shift { scroll.y } else { 0.0 }) / zoom;
            } else if scroll.y != 0.0 {
                self.tl_y0 -= scroll.y;
            }
        }

        let rows_h = GIF_H + (tracks.len() - 1) as f32 * ROW_H;
        let tracks_top = area.min.y + RULER_H;
        let tracks_h = (area.max.y - tracks_top - 8.0).max(10.0);
        self.tl_y0 = self.tl_y0.clamp(0.0, (rows_h - tracks_h).max(0.0));
        let y0 = self.tl_y0;
        let row_top = |t: usize| tracks_top - y0 + if t == 0 { 0.0 } else { GIF_H + (t - 1) as f32 * ROW_H };
        let row_h = |t: usize| if t == 0 { GIF_H } else { ROW_H };

        // ruler
        painter.rect_filled(Rect::from_min_max(area.min, pos2(area.max.x, tracks_top)), 0.0, pal.window);
        painter.hline(area.x_range(), tracks_top, Stroke::new(1.0, pal.border));
        // a tick every so many frames, labels where they fit
        let step = [1usize, 2, 5, 10, 20, 25, 50, 100, 200, 500, 1000].into_iter().find(|s| *s as f32 * zoom >= 48.0).unwrap_or(2000);
        let minor = [1usize, 2, 5, 10, 25, 50, 100].into_iter().find(|s| *s as f32 * zoom >= 8.0).unwrap_or(step);
        let first = (x0 as usize / minor) * minor;
        let mut f = first;
        while (f as f32) < x0 + visible + 1.0 && f <= n {
            let x = fx(f as f32);
            if x >= lanes.min.x {
                let major = f % step == 0;
                painter.vline(x, (tracks_top - if major { 9.0 } else { 4.0 })..=tracks_top, Stroke::new(1.0, pal.fg_muted));
                if major && f < n {
                    painter.text(pos2(x + 3.0, area.min.y + 2.0), egui::Align2::LEFT_TOP, format!("{}", f + 1), egui::FontId::proportional(10.5), pal.fg_muted);
                }
            }
            f += minor;
        }

        // track headers and lanes
        let hovered_pos = ui.input(|i| i.pointer.hover_pos());
        let mut header_click: Option<u64> = None;
        let mut eye_click: Option<u64> = None;
        let track_clip = painter.clip_rect().intersect(Rect::from_min_max(pos2(area.min.x, tracks_top + 1.0), area.max));
        let tp = painter.with_clip_rect(track_clip);
        for (t, &id) in tracks.iter().enumerate() {
            let top = row_top(t);
            let h = row_h(t);
            if top > area.max.y || top + h < tracks_top {
                continue;
            }
            let head = Rect::from_min_size(pos2(area.min.x, top), vec2(head_w, h));
            let lane = Rect::from_min_max(pos2(lanes.min.x, top), pos2(area.max.x, top + h));
            let selected = self.tl_track == Some(id);
            tp.rect_filled(head, 0.0, if selected { mix(pal.window, pal.highlight, 0.25) } else { pal.window });
            tp.hline(area.x_range(), top + h, Stroke::new(1.0, mix(pal.border, pal.view, 0.4)));
            let (name, icon_tool, visible, kind) = if id == GIF {
                ("GIF".to_string(), Tool::Frames, true, None)
            } else {
                let l = &self.stack.layers[self.stack.index_of(id).unwrap()];
                (l.name(), kind_tool(l.kind()), l.visible, Some(l.kind()))
            };
            let eye = Rect::from_center_size(pos2(head.min.x + 14.0, head.center().y), vec2(20.0, 20.0));
            if id != GIF {
                crate::icons::paint(&tp, if visible { "view-visible" } else { "view-hidden" }, eye.center(), 16.0, pal.fg_muted);
            }
            paint_tool_icon(&tp, icon_tool, pos2(head.min.x + 36.0, head.center().y), 16.0, if visible { pal.fg } else { pal.fg_muted });
            let text_w = head_w - 56.0;
            let col = if visible { pal.fg } else { pal.fg_muted };
            let mut shown = name.clone();
            let mut galley = ui.fonts_mut(|f| f.layout_no_wrap(shown.clone(), egui::FontId::proportional(12.5), col));
            // shorten to fit, one character at a time
            while galley.size().x > text_w && shown.chars().count() > 2 {
                let keep: String = shown.chars().take(shown.chars().count().saturating_sub(2)).collect();
                shown = format!("{}…", keep.trim_end_matches('…').trim_end());
                galley = ui.fonts_mut(|f| f.layout_no_wrap(shown.clone(), egui::FontId::proportional(12.5), col));
            }

            tp.galley(pos2(head.min.x + 50.0, head.center().y - galley.size().y / 2.0), galley, pal.fg);
            if id != GIF {
                if resp.clicked() && hovered_pos.is_some_and(|p| eye.contains(p)) {
                    eye_click = Some(id);
                } else if resp.clicked() && hovered_pos.is_some_and(|p| head.contains(p)) {
                    header_click = Some(id);
                }
            } else if resp.clicked() && hovered_pos.is_some_and(|p| head.contains(p)) {
                header_click = Some(id);
            }
            // clips
            let col = kind.map(kind_colour).unwrap_or(pal.highlight);
            let spans = self.track_spans(id);
            let lp = tp.with_clip_rect(track_clip.intersect(lane));
            for (ci, &(a, b)) in spans.iter().enumerate() {
                let r = Rect::from_min_max(pos2(fx(a as f32), top + 3.0), pos2(fx(b as f32 + 1.0), top + h - 3.0));
                if r.max.x < lane.min.x || r.min.x > lane.max.x {
                    continue;
                }
                let sel = selected && self.tl_clip == Some(ci);
                let fill = if visible { mix(pal.view, col, 0.55) } else { mix(pal.view, col, 0.2) };
                lp.rect_filled(r, 3.0, fill);
                if id == GIF {
                    self.paint_gif_thumbs(ctx, &lp, r, a, b, zoom, fx);
                }
                lp.rect_stroke(r, 3.0, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { pal.highlight } else { mix(col, Color32::BLACK, 0.3) }), egui::StrokeKind::Inside);
                if id != GIF && r.width() > 40.0 {
                    let label = format!("{}–{}", a + 1, b + 1);
                    lp.text(pos2(r.min.x.max(lane.min.x) + 5.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(10.5), mix(pal.fg, col, 0.2));
                }
                // trim handles on the selected clip
                if sel {
                    for x in [r.min.x, r.max.x] {
                        lp.rect_filled(Rect::from_center_size(pos2(x, r.center().y), vec2(3.0, r.height() - 6.0)), 1.0, pal.highlight);
                    }
                }
            }
            if id == GIF && self.tl_extend > 0 {
                let r = Rect::from_min_max(pos2(fx(n as f32), top + 3.0), pos2(fx((n + self.tl_extend) as f32), top + h - 3.0));
                lp.rect_filled(r, 3.0, mix(pal.view, pal.highlight, 0.25));
                lp.rect_stroke(r, 3.0, Stroke::new(1.5, pal.highlight), egui::StrokeKind::Inside);
                lp.text(r.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, format!("+{} frames", self.tl_extend), egui::FontId::proportional(11.0), pal.fg);
            }
            // keyframes
            if let Some(li) = (id != GIF).then(|| self.stack.index_of(id)).flatten() {
                for (k, key) in self.stack.layers[li].timing.keys.iter().enumerate() {
                    let c = pos2(fx(key.frame as f32 + 0.5), top + h / 2.0);
                    let on = selected && self.tl_key == Some(k);
                    diamond(&lp, c, if on { 6.5 } else { 5.0 }, if on { pal.highlight } else { Color32::WHITE }, Stroke::new(1.0, Color32::from_black_alpha(160)));
                }
            }
        }
        painter.vline(lanes.min.x, tracks_top..=area.max.y, Stroke::new(1.0, pal.border));

        // horizontal scroll bar when zoomed in
        if visible < n as f32 {
            let bar = Rect::from_min_max(pos2(lanes.min.x, area.max.y - 6.0), pos2(area.max.x, area.max.y - 1.0));
            painter.rect_filled(bar, 2.0, mix(pal.view, pal.fg, 0.08));
            let a = bar.min.x + bar.width() * x0 / n as f32;
            let w = (bar.width() * visible / n as f32).max(16.0);
            painter.rect_filled(Rect::from_min_size(pos2(a, bar.min.y), vec2(w, bar.height())), 2.0, mix(pal.view, pal.fg, 0.35));
        }

        // playhead
        let px = fx(self.cur as f32 + 0.5);
        if px >= lanes.min.x && px <= area.max.x {
            painter.vline(px, area.min.y..=area.max.y, Stroke::new(1.5, pal.negative));
            painter.add(egui::Shape::convex_polygon(vec![pos2(px - 5.0, area.min.y), pos2(px + 5.0, area.min.y), pos2(px, area.min.y + 7.0)], pal.negative, Stroke::NONE));
        }

        // ---- pointer
        let frame_at = |x: f32| x0 + (x - lanes.min.x) / zoom;
        let track_at = |y: f32| -> Option<usize> {
            (0..tracks.len()).find(|&t| y >= row_top(t) && y < row_top(t) + row_h(t)).filter(|_| y >= tracks_top)
        };
        if let Some(id) = eye_click {
            if let Some(li) = self.stack.index_of(id) {
                self.stack.layers[li].visible = !self.stack.layers[li].visible;
                self.modified = true;
            }
        }
        if let Some(id) = header_click {
            self.tl_select_track(id);
        }
        // cursor hints over edges
        if let Some(p) = hovered_pos.filter(|p| lanes.contains(*p) && p.y > tracks_top) {
            if let Some(t) = track_at(p.y) {
                let f = frame_at(p.x);
                let edge = self.track_spans(tracks[t]).iter().any(|&(a, b)| (fx(a as f32) - p.x).abs() < EDGE || (fx(b as f32 + 1.0) - p.x).abs() < EDGE);
                let _ = f;
                if self.tl_razor {
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                } else if edge {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            }
        }
        // the line between names and lanes resizes the name column
        let grip = Rect::from_min_max(pos2(lanes.min.x - 4.0, tracks_top), pos2(lanes.min.x + 3.0, area.max.y));
        if hovered_pos.is_some_and(|p| grip.contains(p)) || self.tl_head_drag {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        if resp.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                if grip.contains(p) {
                    self.tl_head_drag = true;
                } else if p.x < lanes.min.x && p.y > tracks_top {
                    // dragging a layer's name moves it up or down the stack
                    if let Some(t) = track_at(p.y).filter(|t| *t > 0) {
                        self.tl_reorder = Some(t);
                    }
                }
            }
        }
        if self.tl_head_drag {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                self.tl_head_w = (p.x - area.min.x).clamp(90.0, 420.0);
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_head_drag = false;
                theme::set_setting("timeline_names", &format!("{:.0}", self.tl_head_w));
            }
        }
        if let Some(from) = self.tl_reorder {
            let layers = tracks.len() - 1;
            // slot k: before layer track k + 1 (0 = top of the list)
            let y = ui.input(|i| i.pointer.interact_pos()).map(|p| p.y).unwrap_or(0.0);
            let slot = (0..=layers).find(|&k| k == layers || y < row_top(k + 1) + ROW_H / 2.0).unwrap_or(layers);
            let ly = if slot == layers { row_top(layers) + ROW_H } else { row_top(slot + 1) };
            painter.hline(area.min.x..=area.max.x, ly, Stroke::new(2.0, pal.highlight));
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_reorder = None;
                let to = if slot > from - 1 { slot - 1 } else { slot };
                if to != from - 1 {
                    self.snapshot(&format!("tl-reorder{now}"), now);
                    let mut order: Vec<u64> = tracks[1..].to_vec();
                    let id = order.remove(from - 1);
                    order.insert(to.min(order.len()), id);
                    // the stack is bottom-first, the tracks top-first
                    let mut old = std::mem::take(&mut self.stack.layers);
                    for id in order.iter().rev() {
                        if let Some(k) = old.iter().position(|l| l.id == *id) {
                            self.stack.layers.push(old.remove(k));
                        }
                    }
                    self.stack.layers.extend(old);
                    self.modified = true;
                }
            }
        }
        let press = resp.drag_started() || resp.clicked();
        if press && self.tl_drag.is_none() && !self.tl_head_drag {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(hovered_pos).filter(|p| lanes.contains(*p)) {
                let f = frame_at(p.x);
                let fi = (f.floor().max(0.0) as usize).min(n - 1);
                if p.y < tracks_top {
                    self.tl_drag = Some(TlDrag { track: GIF, clip: 0, grab: TlGrab::Playhead, at: f, orig: (fi, fi) });
                    self.cur = fi;
                    self.playing = false;
                } else if let Some(t) = track_at(p.y) {
                    let id = tracks[t];
                    let spans = self.track_spans(id);
                    let cy = row_top(t) + row_h(t) / 2.0;
                    // keyframes first, they sit on top
                    let key = (id != GIF).then(|| self.stack.index_of(id)).flatten().and_then(|li| {
                        self.stack.layers[li].timing.keys.iter().position(|k| (fx(k.frame as f32 + 0.5) - p.x).abs() < 7.0 && (cy - p.y).abs() < 8.0)
                    });
                    let hit = spans.iter().position(|&(a, b)| p.x >= fx(a as f32) - EDGE && p.x <= fx(b as f32 + 1.0) + EDGE);
                    self.tl_select_track(id);
                    if let Some(k) = key {
                        let kf = self.stack.layers[self.stack.index_of(id).unwrap()].timing.keys[k].frame;
                        self.tl_key = Some(k);
                        self.tl_clip = None;
                        self.cur = kf.min(n - 1);
                        self.playing = false;
                        self.tl_drag = Some(TlDrag { track: id, clip: 0, grab: TlGrab::Key(k), at: f, orig: (kf, kf) });
                    } else if let Some(ci) = hit {
                        let (a, b) = spans[ci];
                        self.tl_clip = Some(ci);
                        self.tl_key = None;
                        if self.tl_razor {
                            if resp.clicked() && id != GIF && fi > a {
                                self.snapshot(&format!("tl-razor{now}"), now);
                                if let Some(l) = self.tl_layer_mut(id) {
                                    l.timing.split(fi, n);
                                }
                                self.modified = true;
                            }
                        } else {
                            let grab = if (fx(a as f32) - p.x).abs() < EDGE {
                                TlGrab::Start
                            } else if (fx(b as f32 + 1.0) - p.x).abs() < EDGE {
                                TlGrab::End
                            } else {
                                TlGrab::Body
                            };
                            if resp.drag_started() {
                                self.snapshot(&format!("tl-drag{now}"), now);
                                if id != GIF {
                                    if let Some(li) = self.stack.index_of(id) {
                                        self.stack.ensure_clips(li, &self.plan);
                                    }
                                }
                                self.tl_drag = Some(TlDrag { track: id, clip: ci, grab, at: f, orig: (a, b) });
                            }
                        }
                    } else {
                        // empty lane: move the playhead there
                        self.tl_clip = None;
                        self.tl_key = None;
                        self.cur = fi;
                        self.playing = false;
                    }
                }
            }
        }
        if let (Some(d), Some(p)) = (self.tl_drag, ui.input(|i| i.pointer.interact_pos())) {
            let f = frame_at(p.x);
            let delta = (f - d.at).round() as i64;
            let clamp = |v: i64, lo: usize, hi: usize| v.clamp(lo as i64, hi as i64) as usize;
            match d.grab {
                TlGrab::Playhead => {
                    self.cur = clamp(f.floor() as i64, 0, n - 1);
                    self.playing = false;
                }
                TlGrab::Key(k) => {
                    let nf = clamp(d.orig.0 as i64 + delta, 0, n - 1);
                    if let Some(li) = self.stack.index_of(d.track) {
                        let t = &mut self.stack.layers[li].timing;
                        if k < t.keys.len() && t.keys[k].frame != nf && t.key_at(nf).is_none() {
                            t.keys[k].frame = nf;
                            t.sort_keys();
                            let k2 = t.key_at(nf).unwrap_or(k);
                            self.tl_drag = Some(TlDrag { grab: TlGrab::Key(k2), ..d });
                            self.tl_key = Some(k2);
                            self.modified = true;
                        }
                        self.cur = nf;
                    }
                }
                grab => {
                    let spans = self.track_spans(d.track);
                    // neighbours bound the move so clips never overlap
                    let lo = spans.iter().filter(|s| s.1 < d.orig.0).map(|s| s.1 + 1).max().unwrap_or(0);
                    let hi = spans.iter().filter(|s| s.0 > d.orig.1).map(|s| s.0 - 1).min().unwrap_or(n - 1);
                    let len = d.orig.1 - d.orig.0;
                    // the GIF's last clip can be pulled past the end to make it longer
                    let past = d.track == GIF && grab == TlGrab::End && d.orig.1 + 1 == n;
                    if past {
                        self.tl_extend = (d.orig.1 as i64 + delta - (n as i64 - 1)).clamp(0, (n as i64 * 4).max(250)) as usize;
                    }
                    let (na, nb) = match grab {
                        TlGrab::Body => {
                            let a = clamp(d.orig.0 as i64 + delta, lo, hi - len);
                            (a, a + len)
                        }
                        TlGrab::Start => (clamp(d.orig.0 as i64 + delta, lo, d.orig.1), d.orig.1),
                        _ => (d.orig.0, clamp(d.orig.1 as i64 + delta, d.orig.0, hi)),
                    };
                    let cur = spans.get(d.clip).copied();
                    if cur != Some((na, nb)) {
                        if d.track == GIF {
                            // trimming the GIF skips or brings back frames at its ends
                            if let Some((a, b)) = cur {
                                if grab != TlGrab::Body {
                                    if na > a {
                                        self.gif_set(a, na - 1, false);
                                    }
                                    if na < a {
                                        self.gif_set(na, a - 1, true);
                                    }
                                    if nb < b {
                                        self.gif_set(nb + 1, b, false);
                                    }
                                    if nb > b {
                                        self.gif_set(b + 1, nb, true);
                                    }
                                }
                            }
                        } else if let Some(li) = self.stack.index_of(d.track) {
                            if let Some(cl) = &mut self.stack.layers[li].timing.clips {
                                cl.sort_by_key(|c| c.start);
                                if let Some(c) = cl.get_mut(d.clip) {
                                    *c = Clip { start: na, end: if nb + 1 >= n { None } else { Some(nb) } };
                                }
                            }
                            self.modified = true;
                        }
                    }
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_drag = None;
                if self.tl_extend > 0 {
                    let k = std::mem::take(&mut self.tl_extend);
                    self.tl_lengthen(k, now);
                }
            }
        }
        if self.tl_drag.is_some() || self.tl_reorder.is_some() || self.tl_head_drag {
            ctx.request_repaint();
        }

        // keys, while the pointer is over the timeline
        if resp.hovered() && !ctx.egui_wants_keyboard_input() {
            let (s, del, k, x) = ui.input(|i| (i.key_pressed(egui::Key::S) && i.modifiers.is_none(), i.key_pressed(egui::Key::Delete), i.key_pressed(egui::Key::K) && i.modifiers.is_none(), i.key_pressed(egui::Key::X) && i.modifiers.is_none()));
            if s {
                self.tl_split(now);
            }
            if del {
                self.tl_delete(now);
            }
            if k {
                self.tl_add_key(now);
            }
            if x {
                self.tl_razor = !self.tl_razor;
            }
        }
        // right-click picks what's under the pointer first, as a left click would
        if resp.secondary_clicked() {
            if let Some(p) = hovered_pos.filter(|p| lanes.contains(*p)) {
                if let Some(t) = track_at(p.y) {
                    let id = tracks[t];
                    self.tl_select_track(id);
                    self.tl_clip = self.track_spans(id).iter().position(|&(a, b)| p.x >= fx(a as f32) && p.x <= fx(b as f32 + 1.0));
                }
            }
        }
        resp.context_menu(|ui| {
            ui.set_min_width(220.0);
            let has = self.tl_track.is_some_and(|t| t != GIF);
            if menu_item(ui, "node-break", "&Split at Playhead", "S", has) {
                self.tl_split(now);
            }
            if menu_item(ui, "list-add", "&Add Clip at Playhead", "", has) {
                self.tl_add_clip(now);
            }
            if menu_item(ui, "edit-delete", "&Delete", "Del", self.tl_clip.is_some() || self.tl_key.is_some()) {
                self.tl_delete(now);
            }
            if menu_item(ui, "list-add", "Make &Longer by 1 Second", "", idle) {
                // about a second at the last frame's speed
                let d = self.plan.last().map(|f| effective_delay(f.delay)).unwrap_or(10).max(2) as usize;
                self.tl_lengthen((100 / d).max(1), now);
            }
            ui.separator();
            if menu_item(ui, "node-add", "Add &Keyframe", "K", has) {
                self.tl_add_key(now);
            }
            if menu_item(ui, "edit-clear-all", "Clear Keyframes", "", has) {
                if let Some(li) = self.tl_track.and_then(|t| self.stack.index_of(t)) {
                    self.snapshot(&format!("tl-clearkeys{now}"), now);
                    self.stack.layers[li].timing.keys.clear();
                    self.tl_key = None;
                    self.modified = true;
                }
            }
            if menu_item(ui, "view-refresh", "Show on &Every Frame", "", has) {
                if let Some(li) = self.tl_track.and_then(|t| self.stack.index_of(t)) {
                    self.snapshot(&format!("tl-full{now}"), now);
                    self.stack.ensure_clips(li, &self.plan);
                    self.stack.layers[li].timing.clips = Some(vec![Clip::all()]);
                    self.modified = true;
                }
            }
        });
    }

    /// Make the animation `k` frames longer by holding its last frame, so a
    /// single picture can become an animation for layers to move over.
    fn tl_lengthen(&mut self, k: usize, now: f64) {
        if k == 0 || self.busy() {
            return;
        }
        self.snapshot(&format!("tl-lengthen{now}"), now);
        let Some(base) = self.base.as_mut() else { return };
        let Some(last) = base.frames.last().cloned() else { return };
        let last = crate::anim::Frame { enabled: true, ..last };
        base.frames.extend(std::iter::repeat_n(last, k));
        self.bump();
        self.after_base_change(false);
        self.stack.sync_len(self.base.as_ref().map(|b| b.len()).unwrap_or(0));
        self.info(format!("Added {k} frames"));
    }

    /// Select a track: its layer becomes the selected one, with its tool open.
    fn tl_select_track(&mut self, id: u64) {
        if self.tl_track != Some(id) {
            self.tl_clip = None;
            self.tl_key = None;
        }
        self.tl_track = Some(id);
        if id == GIF {
            return;
        }
        if let Some(li) = self.stack.index_of(id) {
            let k = self.stack.layers[li].kind();
            self.tool = kind_tool(k);
            self.select(k, id);
        }
    }

    /// Frame pictures along the GIF clip, as many as fit.
    #[allow(clippy::too_many_arguments)]
    fn paint_gif_thumbs(&mut self, ctx: &egui::Context, painter: &egui::Painter, r: Rect, a: usize, b: usize, zoom: f32, fx: impl Fn(f32) -> f32) {
        let h = r.height() - 4.0;
        let every = ((h * 1.3) / zoom).ceil().max(1.0) as usize;
        let mut f = a;
        while f <= b {
            let x = fx(f as f32);
            if x > painter.clip_rect().max.x {
                break;
            }
            if x + h * 1.5 >= painter.clip_rect().min.x {
                let src = self.plan[f].src;
                if let Some(t) = self.thumb_texture(ctx, src) {
                    let ts = t.size_vec2();
                    let k = h / ts.y;
                    let w = (ts.x * k).min((b + 1 - f) as f32 * zoom - 2.0).max(0.0);
                    let ir = Rect::from_min_size(pos2(x + 1.0, r.min.y + 2.0), vec2(w, h));
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2((w / (ts.x * k)).min(1.0), 1.0));
                    painter.image(t.id(), ir, uv, Color32::from_white_alpha(230));
                }
            }
            f += every;
        }
    }

    /// Seconds from the start to output frame `i` (skipped frames take no time).
    fn plan_time(&self, i: usize) -> f32 {
        self.plan.iter().take(i).filter(|f| f.enabled).map(|f| effective_delay(f.delay) as f32 / 100.0).sum()
    }
}
