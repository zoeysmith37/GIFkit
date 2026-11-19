//! The Adjust panel: adjustment layers and their editors (Curves and Levels
//! drawn over the frame's histogram, the gradient bar, and the sliders).

use super::*;
use crate::adjust::{self, Adjust, AdjustItem, Histogram, Stop};

/// What a card's header asked for.
enum CardAct {
    Up(usize),
    Down(usize),
    Dup(usize),
    Del(usize),
}

const CH_NAMES: [&str; 4] = ["RGB", "Red", "Green", "Blue"];

fn ch_colour(k: usize) -> Color32 {
    match k {
        1 => Color32::from_rgb(232, 72, 72),
        2 => Color32::from_rgb(70, 200, 90),
        3 => Color32::from_rgb(70, 140, 240),
        _ => p().fg,
    }
}

/// A "Presets" drop-down: the one picked, if any.
fn preset_pick(ui: &mut egui::Ui, id: &str, names: &[&str]) -> Option<usize> {
    let mut out = None;
    let w = ui.available_width();
    egui::ComboBox::from_id_salt(id).width(w.max(40.0)).icon(chevron).truncate().selected_text("Presets").show_ui(ui, |ui| {
        for (i, n) in names.iter().enumerate() {
            if ui.selectable_label(false, *n).clicked() {
                out = Some(i);
            }
        }
    });
    out
}

/// Which of RGB, Red, Green, Blue an editor shows, kept per card.
fn channel_pick(ui: &mut egui::Ui, id: egui::Id) -> usize {
    let mut k: usize = ui.data(|d| d.get_temp(id)).unwrap_or(0);
    seg_row(ui, &mut k, &[(0, "RGB"), (1, "Red"), (2, "Green"), (3, "Blue")]);
    ui.data_mut(|d| d.insert_temp(id, k));
    k
}

/// Histogram columns in `r`, scaled so one spike doesn't flatten the rest.
fn paint_hist(painter: &egui::Painter, r: Rect, bins: &[u32; 256], col: Color32) {
    let mut sorted: Vec<u32> = bins[1..255].to_vec();
    sorted.sort_unstable();
    let top = (sorted[sorted.len() * 995 / 1000] as f32).max(1.0);
    let w = r.width() / 256.0;
    for (i, b) in bins.iter().enumerate() {
        if *b == 0 {
            continue;
        }
        let h = (*b as f32 / top).min(1.0) * r.height();
        let x = r.left() + i as f32 * w;
        painter.rect_filled(Rect::from_min_max(pos2(x, r.bottom() - h), pos2(x + w.max(1.0), r.bottom())), 0.0, col);
    }
}

/// The Curves graph: drag points, click to add one, drag it off the graph
/// or double-click it to take it away. True when the curve changed.
fn curve_editor(ui: &mut egui::Ui, pts: &mut Vec<[f32; 2]>, hist: Option<&[u32; 256]>, ch: usize, id: egui::Id) -> bool {
    let pal = *p();
    let side = ui.available_width().min(300.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    let to_screen = |v: [f32; 2]| pos2(rect.left() + v[0] / 255.0 * side, rect.bottom() - v[1] / 255.0 * side);
    let to_val = |p: Pos2| [((p.x - rect.left()) / side * 255.0).clamp(0.0, 255.0), ((rect.bottom() - p.y) / side * 255.0).clamp(0.0, 255.0)];
    let drag_id = id.with("drag");
    let mut changed = false;
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let near = |p: Pos2, pts: &[[f32; 2]]| pts.iter().position(|v| to_screen(*v).distance(p) <= 9.0);
    if resp.double_clicked() {
        if let Some(k) = resp.interact_pointer_pos().and_then(|p| near(p, pts)) {
            if pts.len() > 2 {
                pts.remove(k);
                changed = true;
            }
        }
    }
    if resp.drag_started() {
        if let Some(pp) = ui.input(|i| i.pointer.press_origin()) {
            let k = match near(pp, pts) {
                Some(k) => k,
                None => {
                    // a new point on the curve where it was pressed
                    let v = to_val(pp);
                    let y = adjust::curve_samples(pts, 256)[v[0].round() as usize] * 255.0;
                    pts.push([v[0], y]);
                    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                    changed = true;
                    pts.iter().position(|q| q[0] == v[0]).unwrap_or(0)
                }
            };
            ui.data_mut(|d| d.insert_temp(drag_id, k));
        }
    }
    if resp.dragged() {
        if let (Some(k), Some(pp)) = (ui.data(|d| d.get_temp::<usize>(drag_id)), resp.interact_pointer_pos()) {
            if k < pts.len() {
                let far = !rect.expand(24.0).contains(pp);
                if far && pts.len() > 2 && k > 0 && k + 1 < pts.len() {
                    pts.remove(k);
                    ui.data_mut(|d| d.remove::<usize>(drag_id));
                } else {
                    let v = to_val(pp);
                    let lo = if k > 0 { pts[k - 1][0] + 1.0 } else { 0.0 };
                    let hi = if k + 1 < pts.len() { pts[k + 1][0] - 1.0 } else { 255.0 };
                    pts[k] = [v[0].clamp(lo, hi.max(lo)).round(), v[1].round()];
                }
                changed = true;
            }
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    // the graph
    let painter = ui.painter_at(rect.expand(6.0));
    painter.rect_filled(rect, 2.0, pal.view);
    if let Some(h) = hist {
        paint_hist(&painter, rect, h, mix(pal.view, pal.fg, 0.16));
    }
    let grid = Stroke::new(1.0, mix(pal.view, pal.fg, 0.12));
    for q in 1..4 {
        let f = q as f32 / 4.0;
        painter.line_segment([pos2(rect.left() + f * side, rect.top()), pos2(rect.left() + f * side, rect.bottom())], grid);
        painter.line_segment([pos2(rect.left(), rect.top() + f * side), pos2(rect.right(), rect.top() + f * side)], grid);
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, mix(pal.view, pal.fg, 0.22)));
    let ys = adjust::curve_samples(pts, 128);
    let line: Vec<Pos2> = ys.iter().enumerate().map(|(i, y)| to_screen([i as f32 / 127.0 * 255.0, y * 255.0])).collect();
    painter.add(egui::Shape::line(line, Stroke::new(1.8, ch_colour(ch))));
    let dragging = ui.data(|d| d.get_temp::<usize>(drag_id));
    for (k, v) in pts.iter().enumerate() {
        let c = to_screen(*v);
        let on = dragging == Some(k);
        let r = Rect::from_center_size(c, vec2(9.0, 9.0));
        painter.rect_filled(r, 1.0, if on { pal.highlight } else { pal.view });
        painter.rect_stroke(r, 1.0, Stroke::new(1.4, if on { Color32::WHITE } else { ch_colour(ch) }), egui::StrokeKind::Middle);
    }
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
    // what's under the pointer
    let read = dragging.and_then(|k| pts.get(k).copied()).or_else(|| resp.hover_pos().map(|hp| {
        let v = to_val(hp);
        [v[0].round(), (adjust::curve_samples(pts, 256)[v[0].round() as usize] * 255.0).round()]
    }));
    if let Some(v) = read {
        painter.text(rect.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, format!("In {:.0}  Out {:.0}", v[0], v[1]), egui::FontId::proportional(11.0), pal.fg_muted);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let resp = resp.on_hover_text("Drag a point. Click the line to add one. Drag a point off the graph or double-click it to take it away");
    let _ = resp;
    changed
}

/// A level bar with triangle handles at `vals` (0..255): which one moved.
fn level_bar(ui: &mut egui::Ui, id: egui::Id, vals: &mut [f32], fills: &[Color32], gradient: bool) -> bool {
    let pal = *p();
    let w = ui.available_width().min(300.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
    let bar = Rect::from_min_size(rect.left_top(), vec2(w, 7.0));
    let painter = ui.painter_at(rect.expand(6.0));
    if gradient {
        let n = 64;
        for i in 0..n {
            let v = (i as f32 / (n - 1) as f32 * 255.0) as u8;
            let x0 = bar.left() + bar.width() * i as f32 / n as f32;
            painter.rect_filled(Rect::from_min_max(pos2(x0, bar.top()), pos2(x0 + bar.width() / n as f32 + 0.5, bar.bottom())), 0.0, Color32::from_gray(v));
        }
        painter.rect_stroke(bar, 0.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
    }
    let x_of = |v: f32| bar.left() + v / 255.0 * w;
    let drag_id = id.with("drag");
    let mut changed = false;
    if resp.drag_started() {
        if let Some(pp) = ui.input(|i| i.pointer.press_origin()) {
            let k = (0..vals.len()).min_by(|a, b| (x_of(vals[*a]) - pp.x).abs().total_cmp(&(x_of(vals[*b]) - pp.x).abs()));
            if let Some(k) = k {
                ui.data_mut(|d| d.insert_temp(drag_id, k));
            }
        }
    }
    if resp.dragged() {
        if let (Some(k), Some(pp)) = (ui.data(|d| d.get_temp::<usize>(drag_id)), resp.interact_pointer_pos()) {
            vals[k] = ((pp.x - bar.left()) / w * 255.0).clamp(0.0, 255.0).round();
            changed = true;
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    for (k, v) in vals.iter().enumerate() {
        let x = x_of(*v);
        let tip = pos2(x, bar.bottom() + 1.0);
        let pts = vec![tip, pos2(x - 6.0, rect.bottom() - 1.0), pos2(x + 6.0, rect.bottom() - 1.0)];
        painter.add(egui::Shape::convex_polygon(pts, fills[k], Stroke::new(1.0, mix(pal.fg, pal.window, 0.3))));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    changed
}

fn paint_gradient(painter: &egui::Painter, r: Rect, stops: &[Stop], reverse: bool) {
    let item = AdjustItem::new(Adjust::GradientMap(adjust::GradientMap { stops: stops.to_vec(), reverse }));
    let n = (r.width() as usize).clamp(2, 400);
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        // a grey of this lightness through the map
        let c = adjust::map_one(&item, [t, t, t]);
        let x0 = r.left() + r.width() * i as f32 / n as f32;
        painter.rect_filled(Rect::from_min_max(pos2(x0, r.top()), pos2(x0 + r.width() / n as f32 + 0.6, r.bottom())), 0.0, Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
    }
}

/// The Gradient Map's bar: pick a stop, drag it, click under the bar to add one.
fn gradient_editor(ui: &mut egui::Ui, g: &mut adjust::GradientMap, id: egui::Id) {
    let pal = *p();
    let sel_id = id.with("sel");
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    let w = ui.available_width().min(300.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click_and_drag());
    let bar = Rect::from_min_size(rect.left_top(), vec2(w, 22.0));
    let painter = ui.painter_at(rect.expand(6.0));
    paint_gradient(&painter, bar, &g.stops, false);
    painter.rect_stroke(bar, 0.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
    let x_of = |t: f32| bar.left() + t * w;
    let hit = |pp: Pos2, stops: &[Stop]| stops.iter().position(|s| (x_of(s.at) - pp.x).abs() <= 7.0 && pp.y >= bar.bottom() - 2.0);
    let drag_id = id.with("drag");
    if resp.drag_started() || resp.clicked() {
        if let Some(pp) = ui.input(|i| i.pointer.press_origin()) {
            match hit(pp, &g.stops) {
                Some(k) => sel = k,
                None if pp.y >= bar.bottom() - 2.0 || resp.clicked() => {
                    let t = ((pp.x - bar.left()) / w).clamp(0.0, 1.0);
                    let c = adjust::map_one(&AdjustItem::new(Adjust::GradientMap(adjust::GradientMap { stops: g.stops.clone(), reverse: false })), [t, t, t]);
                    g.stops.push(Stop { at: t, color: c.map(|v| (v * 255.0).round() as u8) });
                    sel = g.stops.len() - 1;
                }
                None => {}
            }
            if resp.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, sel));
            }
        }
    }
    if resp.dragged() {
        if let (Some(k), Some(pp)) = (ui.data(|d| d.get_temp::<usize>(drag_id)), resp.interact_pointer_pos()) {
            if let Some(s) = g.stops.get_mut(k) {
                s.at = ((pp.x - bar.left()) / w).clamp(0.0, 1.0);
            }
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    sel = sel.min(g.stops.len().saturating_sub(1));
    for (k, s) in g.stops.iter().enumerate() {
        let x = x_of(s.at);
        let top = bar.bottom() + 2.0;
        let pts = vec![pos2(x, top), pos2(x + 6.0, top + 7.0), pos2(x + 6.0, rect.bottom() - 1.0), pos2(x - 6.0, rect.bottom() - 1.0), pos2(x - 6.0, top + 7.0)];
        let edge = if k == sel { pal.highlight } else { mix(pal.fg, pal.window, 0.3) };
        painter.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(s.color[0], s.color[1], s.color[2]), Stroke::new(if k == sel { 2.0 } else { 1.0 }, edge)));
    }
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    if let Some(s) = g.stops.get_mut(sel) {
        form_row(ui, "Stop", |ui| {
            ui.horizontal(|ui| {
                colour_btn(ui, &mut s.color);
                let mut pct = s.at * 100.0;
                if ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%").max_decimals(0)).changed() {
                    s.at = pct / 100.0;
                }
            });
        });
    }
    if g.stops.len() > 2 {
        form_row(ui, "", |ui| {
            if ui.button("Remove stop").clicked() {
                g.stops.remove(sel);
            }
        });
    }
}

/// Slider row helper: label, value, range.
fn slide(ui: &mut egui::Ui, label: &str, v: &mut f32, r: std::ops::RangeInclusive<f32>, suffix: &str, decimals: usize) -> egui::Response {
    form_row(ui, label, |ui| fill_slider(ui, egui::Slider::new(v, r.clone()).suffix(suffix).max_decimals(decimals)))
}

impl GifKitApp {
    /// The current source frame's histogram, worked out once per frame shown.
    fn adjust_histogram(&mut self) -> Option<Arc<Histogram>> {
        let base = self.base.as_ref()?;
        let src = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let key = (src, self.version);
        if let Some((k, h)) = &self.adj_hist {
            if *k == key {
                return Some(h.clone());
            }
        }
        let img = base.frames.get(src)?.image.get();
        let h = Arc::new(adjust::histogram(&img));
        self.adj_hist = Some((key, h.clone()));
        Some(h)
    }

    pub(super) fn ui_adjust(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let pal = *p();
        let blank = EffectSettings { adjust_layer: true, ..Default::default() };
        let sel = self.sel_index(Kind::Adjust);
        // with one there, another empty layer; with none, the menu below makes it
        if sel.is_some() && add_layer_button(ui, "Add New Adjustment Layer") {
            self.add_layer(LayerKind::Effects(Box::new(blank.clone())));
            return;
        }
        if sel.is_some() {
            ui.add_space(GROUP_GAP);
        }
        hint(ui, if sel.is_some() { "Editing the selected adjustment layer. Adjustments run top to bottom" } else { "Pick an adjustment to make a layer for it" });
        ui.add_space(6.0);
        let mut pick: Option<usize> = None;
        // Photoshop's adjustment menu
        let w = ui.available_width();
        let menu = egui::Button::image_and_text(crate::icons::image(ui.ctx(), "list-add", 16.0, pal.fg), RichText::new(if sel.is_some() { "Add Adjustment" } else { "New Adjustment Layer" }).color(pal.fg)).min_size(vec2(w, CHIP_H));
        egui::containers::menu::MenuButton::from_button(menu).ui(ui, |ui| {
            ui.set_min_width(220.0);
            for (k, name) in adjust::KINDS.iter().enumerate() {
                if k == 2 || k == 5 || k == 9 || k == 11 {
                    ui.separator();
                }
                let b = egui::Button::image_and_text(crate::icons::image(ui.ctx(), Adjust::icon(k), 16.0, pal.fg), *name);
                if ui.add(b).on_hover_text(adjust_tip(k)).clicked() {
                    pick = Some(k);
                    ui.close();
                }
            }
        });
        ui.add_space(8.0);
        if pick.is_some() {
            ui.ctx().request_repaint();
        }
        let hist = self.adjust_histogram();
        let Some(li) = sel else {
            if let Some(k) = pick {
                let mut fx = blank.clone();
                fx.adjust.push(AdjustItem::new(Adjust::make(k)));
                self.add_layer(LayerKind::Effects(Box::new(fx)));
            }
            return;
        };
        let lid = self.stack.layers[li].id;
        let LayerKind::Effects(fx) = &self.stack.layers[li].kind else { return };
        let mut items = fx.adjust.clone();
        let before = items.clone();
        if let Some(k) = pick {
            items.push(AdjustItem::new(Adjust::make(k)));
            ui.data_mut(|d| d.insert_temp(egui::Id::unique(("adj-open", lid, items.len() - 1)), true));
        }
        if items.is_empty() {
            hint(ui, "No adjustments on this layer yet");
        }
        let mut act = None;
        let n = items.len();
        for (k, item) in items.iter_mut().enumerate() {
            if let Some(a) = adjust_card(ui, item, k, n, lid, hist.as_deref()) {
                act = Some(a);
            }
        }
        match act {
            Some(CardAct::Up(k)) if k > 0 => items.swap(k, k - 1),
            Some(CardAct::Down(k)) if k + 1 < items.len() => items.swap(k, k + 1),
            Some(CardAct::Dup(k)) => {
                let c = items[k].clone();
                items.insert(k + 1, c);
            }
            Some(CardAct::Del(k)) => {
                items.remove(k);
            }
            _ => {}
        }
        if items != before {
            if let LayerKind::Effects(fx) = &mut self.stack.layers[li].kind {
                fx.adjust = items;
            }
            self.modified = true;
        }
    }
}

fn adjust_tip(k: usize) -> &'static str {
    [
        "Set the black, white and middle points, per channel",
        "Reshape the tones with a curve, per channel",
        "Stops of light, worked in linear light",
        "Boost dull colours more than strong ones; skin is held back",
        "Push shadows, midtones and highlights toward a colour",
        "Grey from colour, with how bright each colour turns out",
        "A coloured lens filter",
        "Each channel made from a mix of the others",
        "Change the ink in each colour range",
        "Map dark to light onto a gradient",
        "Pure black and white at a level",
        "Ready-made colour grades",
        "Load a .cube lookup table from Resolve, Premiere and others",
    ][k]
}

/// One adjustment's card: its header, and its editor when open.
fn adjust_card(ui: &mut egui::Ui, item: &mut AdjustItem, k: usize, n: usize, lid: u64, hist: Option<&Histogram>) -> Option<CardAct> {
    let pal = *p();
    let open_id = egui::Id::unique(("adj-open", lid, k));
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
    let mut act = None;
    egui::Frame::new().fill(pal.view).stroke(Stroke::new(1.0, pal.border)).corner_radius(3.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (fr, fresp) = ui.allocate_exact_size(vec2(14.0, 22.0), Sense::click());
            paint_fold(ui.painter(), fr, pal.fg_muted, open);
            if fresp.clicked() {
                open = !open;
            }
            ui.add(egui::Checkbox::without_text(&mut item.on)).on_hover_text("Turn this adjustment on or off");
            crate::icons::paint(ui.painter(), Adjust::icon(item.adj.index()), ui.cursor().min + vec2(8.0, 11.0), 16.0, pal.fg);
            ui.add_space(20.0);
            let name = ui.add(egui::Label::new(RichText::new(item.adj.name()).color(if item.on { pal.fg } else { pal.fg_muted })).sense(Sense::click()));
            if name.clicked() {
                open = !open;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if small_icon(ui, "edit-delete", "Delete this adjustment") {
                    act = Some(CardAct::Del(k));
                }
                if small_icon(ui, "edit-duplicate", "Duplicate") {
                    act = Some(CardAct::Dup(k));
                }
                if k + 1 < n && small_icon(ui, "go-down", "Move down (runs later)") {
                    act = Some(CardAct::Down(k));
                }
                if k > 0 && small_icon(ui, "go-up", "Move up (runs earlier)") {
                    act = Some(CardAct::Up(k));
                }
            });
        });
        if open {
            ui.add_space(4.0);
            ui.add_enabled_ui(item.on, |ui| adjust_editor(ui, &mut item.adj, egui::Id::unique(("adj", lid, k)), hist));
        }
    });
    ui.add_space(6.0);
    ui.data_mut(|d| d.insert_temp(open_id, open));
    act
}

fn adjust_editor(ui: &mut egui::Ui, adj: &mut Adjust, id: egui::Id, hist: Option<&Histogram>) {
    match adj {
        Adjust::Levels(l) => {
            let ch = channel_pick(ui, id.with("ch"));
            ui.add_space(4.0);
            let w = ui.available_width().min(300.0);
            let (r, _) = ui.allocate_exact_size(vec2(w, 80.0), Sense::hover());
            let painter = ui.painter_at(r);
            painter.rect_filled(r, 2.0, mix(p().view, p().window, 0.5));
            if let Some(h) = hist {
                paint_hist(&painter, r.shrink(1.0), &h.bins[if ch == 0 { 3 } else { ch - 1 }], mix(ch_colour(ch), p().view, 0.35));
            }
            let c = &mut l.ch[ch];
            // the grey handle sits where the middle grey lands: t = 0.5^gamma
            let mid = c.in_black + (c.in_white - c.in_black) * 0.5f32.powf(c.gamma);
            let mut ins = [c.in_black, mid, c.in_white];
            if level_bar(ui, id.with(("in", ch)), &mut ins, &[Color32::BLACK, Color32::from_gray(128), Color32::WHITE], false) {
                c.in_black = ins[0].min(c.in_white - 2.0).max(0.0);
                c.in_white = ins[2].max(c.in_black + 2.0).min(255.0);
                let t = ((ins[1] - c.in_black) / (c.in_white - c.in_black).max(1.0)).clamp(0.01, 0.99);
                if (ins[1] - mid).abs() > 0.01 {
                    c.gamma = (t.ln() / 0.5f32.ln()).clamp(0.1, 9.99);
                }
            }
            form_row(ui, "Input", |ui| {
                even_grid(ui, 3, 0.0, |ui, i, size| match i {
                    0 => {
                        spin(ui, size, &mut c.in_black, 1.0, |d| d.range(0.0..=253.0).max_decimals(0));
                    }
                    1 => {
                        spin(ui, size, &mut c.gamma, 0.01, |d| d.range(0.1..=9.99).speed(0.01).max_decimals(2));
                    }
                    _ => {
                        spin(ui, size, &mut c.in_white, 1.0, |d| d.range(2.0..=255.0).max_decimals(0));
                    }
                })
            });
            ui.add_space(4.0);
            let mut outs = [c.out_black, c.out_white];
            if level_bar(ui, id.with(("out", ch)), &mut outs, &[Color32::BLACK, Color32::WHITE], true) {
                c.out_black = outs[0];
                c.out_white = outs[1];
            }
            form_row(ui, "Output", |ui| {
                even_grid(ui, 2, 0.0, |ui, i, size| {
                    if i == 0 {
                        spin(ui, size, &mut c.out_black, 1.0, |d| d.range(0.0..=255.0).max_decimals(0));
                    } else {
                        spin(ui, size, &mut c.out_white, 1.0, |d| d.range(0.0..=255.0).max_decimals(0));
                    }
                })
            });
            ui.add_space(4.0);
            {
                even_grid(ui, 3, 0.0, |ui, i, size| match i {
                    0 => {
                        if ui.add_sized(size, egui::Button::new("Auto")).on_hover_text("Each channel stretched to fill black to white: fixes a colour cast too").clicked() {
                            if let Some(h) = hist {
                                *l = h.auto_levels();
                            }
                        }
                    }
                    1 => {
                        if ui.add_sized(size, egui::Button::new("Contrast")).on_hover_text("Auto contrast: stretched together, colours kept").clicked() {
                            if let Some(h) = hist {
                                *l = h.auto_contrast();
                            }
                        }
                    }
                    _ => {
                        if ui.add_sized(size, egui::Button::new("Reset")).clicked() {
                            *l = adjust::Levels::default();
                        }
                    }
                });
            }
        }
        Adjust::Curves(c) => {
            let ch = channel_pick(ui, id.with("ch"));
            ui.add_space(4.0);
            let bins = hist.map(|h| &h.bins[if ch == 0 { 3 } else { ch - 1 }]);
            curve_editor(ui, &mut c.ch[ch], bins, ch, id.with(("curve", ch)));
            ui.add_space(4.0);
            {
                even_grid(ui, 3, 0.0, |ui, i, size| match i {
                    0 => {
                        ui.allocate_ui(size, |ui| {
                            let names: Vec<&str> = adjust::CURVE_PRESETS.iter().map(|p| p.0).collect();
                            if let Some(k) = preset_pick(ui, &format!("{id:?}-cp"), &names) {
                                c.ch[ch] = adjust::CURVE_PRESETS[k].1.to_vec();
                            }
                        });
                    }
                    1 => {
                        if ui.add_sized(size, egui::Button::new("Auto")).on_hover_text("Each channel stretched to fill black to white").clicked() {
                            if let Some(h) = hist {
                                *c = h.auto_curves();
                            }
                        }
                    }
                    _ => {
                        if ui.add_sized(size, egui::Button::new("Reset")).on_hover_text(format!("Straighten the {} curve", CH_NAMES[ch])).clicked() {
                            c.ch[ch] = adjust::identity_curve();
                        }
                    }
                });
            }
        }
        Adjust::Exposure(e) => {
            slide(ui, "Exposure", &mut e.exposure, -5.0..=5.0, " EV", 2).on_hover_text("Stops of light: +1 doubles it");
            slide(ui, "Offset", &mut e.offset, -0.5..=0.5, "", 3).on_hover_text("Lifts or sinks the shadows most");
            slide(ui, "Gamma", &mut e.gamma, 0.1..=9.99, "", 2).on_hover_text("Bends the midtones: above 1 lighter");
        }
        Adjust::Vibrance(v) => {
            slide(ui, "Vibrance", &mut v.vibrance, -100.0..=100.0, "", 0).on_hover_text("Dull colours gain most; skin tones are held back");
            slide(ui, "Saturation", &mut v.saturation, -100.0..=100.0, "", 0).on_hover_text("Every colour by the same amount");
        }
        Adjust::ColorBalance(b) => {
            let tid = id.with("tone");
            let mut tone: usize = ui.data(|d| d.get_temp(tid)).unwrap_or(1);
            form_row(ui, "Tones", |ui| combo(ui, &format!("{id:?}-tone"), &mut tone, &[(0, "Shadows"), (1, "Midtones"), (2, "Highlights")]));
            ui.data_mut(|d| d.insert_temp(tid, tone));
            let t = &mut b.tones[tone];
            for (i, (lo, hi)) in [("Cyan", "Red"), ("Magenta", "Green"), ("Yellow", "Blue")].iter().enumerate() {
                form_row(ui, hi, |ui| fill_slider(ui, egui::Slider::new(&mut t[i], -100.0..=100.0).max_decimals(0))).on_hover_text(format!("Left toward {lo}, right toward {hi}"));
            }
            form_row(ui, "", |ui| check(ui, &mut b.preserve, "Preserve luminosity"));
        }
        Adjust::BlackWhite(b) => {
            const NAMES: [&str; 6] = ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];
            for (i, nm) in NAMES.iter().enumerate() {
                form_row(ui, nm, |ui| fill_slider(ui, egui::Slider::new(&mut b.weights[i], -200.0..=300.0).suffix("%").max_decimals(0)));
            }
            form_row(ui, "", |ui| {
                ui.horizontal(|ui| {
                    check(ui, &mut b.tint, "Tint");
                    ui.add_enabled_ui(b.tint, |ui| colour_btn(ui, &mut b.tint_color));
                })
            });
            form_row(ui, "", |ui| {
                let names: Vec<&str> = adjust::BW_PRESETS.iter().map(|p| p.0).collect();
                if let Some(k) = preset_pick(ui, &format!("{id:?}-bw"), &names) {
                    b.weights = adjust::BW_PRESETS[k].1;
                }
            });
        }
        Adjust::PhotoFilter(f) => {
            form_row(ui, "Filter", |ui| {
                let names: Vec<&str> = adjust::FILTERS.iter().map(|p| p.0).collect();
                let cur = adjust::FILTERS.iter().position(|p| p.1 == f.color);
                let mut v = cur.unwrap_or(usize::MAX);
                let mut items: Vec<(usize, &str)> = names.iter().enumerate().map(|(i, n)| (i, *n)).collect();
                if cur.is_none() {
                    items.push((usize::MAX, "Custom colour"));
                }
                if combo(ui, &format!("{id:?}-pf"), &mut v, &items).changed() && v != usize::MAX {
                    f.color = adjust::FILTERS[v].1;
                }
            });
            form_row(ui, "Colour", |ui| colour_btn(ui, &mut f.color));
            slide(ui, "Density", &mut f.density, 0.0..=100.0, "%", 0);
            form_row(ui, "", |ui| check(ui, &mut f.preserve, "Preserve luminosity"));
        }
        Adjust::ChannelMixer(m) => {
            let oid = id.with("out");
            let mut o: usize = ui.data(|d| d.get_temp(oid)).unwrap_or(0);
            if m.mono {
                o = 0;
            }
            form_row(ui, "Output", |ui| {
                ui.add_enabled_ui(!m.mono, |ui| seg_row(ui, &mut o, &[(0, "Red"), (1, "Green"), (2, "Blue")]));
            });
            ui.data_mut(|d| d.insert_temp(oid, o));
            let row = &mut m.rows[o];
            for (i, nm) in ["Red", "Green", "Blue", "Constant"].iter().enumerate() {
                form_row(ui, nm, |ui| fill_slider(ui, egui::Slider::new(&mut row[i], -200.0..=200.0).suffix("%").max_decimals(0)));
            }
            let total = row[0] + row[1] + row[2];
            form_row(ui, "", |ui| hint(ui, &format!("Total {total:.0}%{}", if (total - 100.0).abs() > 0.5 { " (100% keeps brightness)" } else { "" })));
            let was = m.mono;
            form_row(ui, "", |ui| check(ui, &mut m.mono, "Monochrome"));
            if m.mono && !was && m.rows[0] == [100.0, 0.0, 0.0, 0.0] {
                // Photoshop's starting mix for black and white
                m.rows[0] = [40.0, 40.0, 20.0, 0.0];
            }
        }
        Adjust::SelectiveColor(s) => {
            let rid = id.with("range");
            let mut r: usize = ui.data(|d| d.get_temp(rid)).unwrap_or(0);
            form_row(ui, "Colours", |ui| {
                let items: Vec<(usize, &str)> = adjust::SC_RANGES.iter().enumerate().map(|(i, n)| (i, *n)).collect();
                combo(ui, &format!("{id:?}-sc"), &mut r, &items)
            });
            ui.data_mut(|d| d.insert_temp(rid, r));
            for (i, nm) in ["Cyan", "Magenta", "Yellow", "Black"].iter().enumerate() {
                form_row(ui, nm, |ui| fill_slider(ui, egui::Slider::new(&mut s.ranges[r][i], -100.0..=100.0).suffix("%").max_decimals(0)));
            }
            form_row(ui, "Method", |ui| seg_row(ui, &mut s.relative, &[(true, "Relative"), (false, "Absolute")]))
                .on_hover_text("Relative: by how much ink is already there. Absolute: by a set amount");
        }
        Adjust::GradientMap(g) => {
            gradient_editor(ui, g, id.with("grad"));
            form_row(ui, "", |ui| {
                ui.horizontal(|ui| {
                    check(ui, &mut g.reverse, "Reverse");
                });
            });
            form_row(ui, "", |ui| {
                let names: Vec<&str> = adjust::GRADIENTS.iter().map(|p| p.0).collect();
                if let Some(k) = preset_pick(ui, &format!("{id:?}-gm"), &names) {
                    g.stops = adjust::GRADIENTS[k].1.to_vec();
                }
            });
        }
        Adjust::Threshold(t) => {
            let w = ui.available_width().min(300.0);
            let (r, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
            let painter = ui.painter_at(r);
            painter.rect_filled(r, 2.0, mix(p().view, p().window, 0.5));
            if let Some(h) = hist {
                paint_hist(&painter, r.shrink(1.0), &h.bins[3], mix(p().fg, p().view, 0.45));
            }
            let x = r.left() + t.level / 255.0 * r.width();
            painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.5, p().highlight));
            let mut v = [t.level];
            if level_bar(ui, id.with("thr"), &mut v, &[Color32::from_gray(128)], true) {
                t.level = v[0].clamp(1.0, 255.0);
            }
            slide(ui, "Level", &mut t.level, 1.0..=255.0, "", 0).on_hover_text("Lighter than this turns white, the rest black");
        }
        Adjust::Look(l) => {
            form_row(ui, "Look", |ui| {
                let items: Vec<(adjust::Look, &str)> = adjust::Look::ALL.iter().map(|k| (*k, k.label())).collect();
                combo(ui, &format!("{id:?}-look"), &mut l.look, &items)
            });
            slide(ui, "Amount", &mut l.amount, 0.0..=100.0, "%", 0);
        }
        Adjust::Cube(c) => {
            form_row(ui, "File", |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Load .cube…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("3D LUT", &["cube", "CUBE"]).pick_file() {
                            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| adjust::parse_cube(&t)) {
                                Ok(lut) => {
                                    c.lut = lut;
                                    c.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                }
                                Err(e) => {
                                    ui.data_mut(|d| d.insert_temp(id.with("err"), e));
                                }
                            }
                        }
                    }
                    if c.lut.is_empty() {
                        hint(ui, "None loaded");
                    } else {
                        hint(ui, &c.name);
                    }
                });
            });
            if let Some(e) = ui.data(|d| d.get_temp::<String>(id.with("err"))) {
                form_row(ui, "", |ui| ui.label(RichText::new(&e).size(12.0).color(p().negative)));
            }
            slide(ui, "Amount", &mut c.amount, 0.0..=100.0, "%", 0);
            form_row(ui, "", |ui| hint(ui, "Saved inside the project"));
        }
    }
}
