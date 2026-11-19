//! The tool options bar: the settings you change while drawing, in a strip
//! over the picture (as Photoshop's options bar and Krita's tool options
//! docked on top). It edits the picked layer, or the style the next new one
//! gets, the same as the side panel does.

use super::*;

pub(super) const BAR_H: f32 = 38.0;

impl GifKitApp {
    /// Does the current tool have a bar, and is the bar turned on?
    pub(super) fn has_options_bar(&self) -> bool {
        self.options_bar && self.base.is_some() && matches!(self.tool, Tool::Shapes | Tool::Text | Tool::Markup)
    }

    pub(super) fn options_bar_ui(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let pal = *p();
        ui.painter().rect_filled(rect, 0.0, pal.window);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, pal.border));
        let inner = rect.shrink2(vec2(10.0, 5.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.set_clip_rect(rect);
        child.spacing_mut().item_spacing.x = 8.0;
        let ui = &mut child;
        crate::icons::paint(ui.painter(), self.tool.breeze(), pos2(inner.left() + 8.0, inner.center().y), 16.0, pal.fg_muted);
        ui.add_space(22.0);
        match self.tool {
            Tool::Shapes => self.shapes_bar(ui),
            Tool::Text => self.text_bar(ui),
            Tool::Markup => self.markup_bar(ui),
            _ => {}
        }
    }

    fn shapes_bar(&mut self, ui: &mut egui::Ui) {
        let sel = self.sel_index(Kind::Shape);
        let mut st = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Shape(s)) => (**s).clone(),
            _ => self.shape_tpl.clone(),
        };
        let before = st.clone();
        if st.kind != ShapeKind::Emoji {
            let kinds: Vec<(ShapeKind, &str)> = ShapeKind::ALL.iter().copied().filter(|k| *k != ShapeKind::Emoji).map(|k| (k, k.label())).collect();
            ui.add_sized(vec2(128.0, CHIP_H), |ui: &mut egui::Ui| combo(ui, "bar-shape-kind", &mut st.kind, &kinds)).on_hover_text("Shape");
            bar_sep(ui);
            if st.kind != ShapeKind::Line {
                ui.add(egui::Checkbox::without_text(&mut st.fill_on)).on_hover_text("Fill");
                ui.label("Fill");
                ui.add_enabled_ui(st.fill_on, |ui| colour_btn(ui, &mut st.fill).on_hover_text("Fill colour"));
                bar_sep(ui);
                ui.add(egui::Checkbox::without_text(&mut st.stroke_on)).on_hover_text("Outline");
                ui.label("Outline");
            } else {
                ui.label("Line");
            }
            let lined = st.stroke_on || st.kind == ShapeKind::Line;
            ui.add_enabled_ui(lined, |ui| {
                colour_btn(ui, &mut st.stroke).on_hover_text("Outline colour");
                ui.add_sized(vec2(70.0, CHIP_H), egui::DragValue::new(&mut st.stroke_w).range(1.0..=200.0).speed(0.2).max_decimals(1).suffix(" px")).on_hover_text("Outline width");
            });
            bar_sep(ui);
        }
        ui.label("Opacity");
        ui.add_sized(vec2(64.0, CHIP_H), egui::DragValue::new(&mut st.opacity).range(0.0..=100.0).max_decimals(0).suffix("%"));
        if st != before {
            match sel {
                Some(i) => {
                    if let LayerKind::Shape(s) = &mut self.stack.layers[i].kind {
                        **s = st;
                    }
                    self.modified = true;
                }
                None => {
                    let t = &self.shape_tpl;
                    self.shape_tpl = ShapeSettings { x: t.x, y: t.y, w: t.w, h: t.h, angle: t.angle, from: t.from, to: t.to, ..st };
                }
            }
        }
    }

    fn text_bar(&mut self, ui: &mut egui::Ui) {
        let sel = self.sel_index(Kind::Text);
        let mut a = match sel.map(|i| &self.stack.layers[i].kind) {
            Some(LayerKind::Text(a)) => a.clone(),
            _ => self.text_tpl.clone(),
        };
        let before = a.clone();
        let font = self.fonts.get(a.font_idx).map(|f| f.name.clone()).unwrap_or_default();
        ui.add_sized(vec2(150.0, CHIP_H), egui::Label::new(RichText::new(font).color(p().fg)).truncate()).on_hover_text("The font. Pick another in the Text panel");
        ui.add_sized(vec2(76.0, CHIP_H), egui::DragValue::new(&mut a.size).range(4.0..=4000.0).speed(0.5).max_decimals(0).suffix(" px")).on_hover_text("Size");
        colour_btn_a(ui, &mut a.color).on_hover_text("Text colour");
        bar_sep(ui);
        ui.label("Outline");
        colour_btn_a(ui, &mut a.border_color).on_hover_text("Outline colour");
        let mut w = a.outline_px() as f32;
        if ui.add_sized(vec2(64.0, CHIP_H), egui::DragValue::new(&mut w).range(0.0..=200.0).speed(0.2).suffix(" px")).on_hover_text("Outline width. 0 is none").changed() {
            a.border_width = Some(w.round());
        }
        bar_sep(ui);
        bar_choice(ui, &mut a.align, &[(Align::Left, "Left"), (Align::Center, "Centre"), (Align::Right, "Right")]);
        if a != before {
            match sel {
                Some(i) => {
                    if let LayerKind::Text(t) = &mut self.stack.layers[i].kind {
                        *t = a;
                    }
                    self.modified = true;
                }
                None => {
                    self.text_tpl = Annotation { text: self.text_tpl.text.clone(), x: 0, y: 0, angle: 0.0, from: 1, to: 0, ..a };
                }
            }
        }
    }

    fn markup_bar(&mut self, ui: &mut egui::Ui) {
        bar_choice(ui, &mut self.pen_tool, &[(PenTool::Pen, "Pen"), (PenTool::Highlighter, "Highlighter"), (PenTool::Eraser, "Eraser")]);
        bar_sep(ui);
        let eraser = self.pen_tool == PenTool::Eraser;
        let b = match self.pen_tool {
            PenTool::Highlighter => &mut self.highlighter,
            _ => &mut self.pen,
        };
        ui.add_enabled_ui(!eraser, |ui| colour_btn(ui, &mut b.color).on_hover_text("Colour"));
        ui.label("Size");
        ui.add_sized(vec2(70.0, CHIP_H), egui::DragValue::new(&mut b.width).range(1.0..=400.0).speed(0.2).max_decimals(1).suffix(" px"));
        ui.add_enabled_ui(!eraser, |ui| {
            ui.label("Opacity");
            ui.add_sized(vec2(64.0, CHIP_H), egui::DragValue::new(&mut b.opacity).range(5.0..=100.0).max_decimals(0).suffix("%"));
        });
        bar_sep(ui);
        bar_choice(ui, &mut self.pen_scope, &[(PenScope::All, "All frames"), (PenScope::FromHere, "From here"), (PenScope::ThisFrame, "This frame")]);
    }
}

/// A row of toggle buttons joined together, as KDE's segmented buttons,
/// each only as wide as its name.
fn bar_choice<T: PartialEq + Copy>(ui: &mut egui::Ui, v: &mut T, items: &[(T, &str)]) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, (k, name)) in items.iter().enumerate() {
            let on = *v == *k;
            let round = 4u8;
            let corner = egui::CornerRadius { nw: if i == 0 { round } else { 0 }, sw: if i == 0 { round } else { 0 }, ne: if i + 1 == items.len() { round } else { 0 }, se: if i + 1 == items.len() { round } else { 0 } };
            if ui.add(sel_btn(egui::Button::new(*name).corner_radius(corner).min_size(vec2(0.0, CHIP_H)), on)).clicked() {
                *v = *k;
            }
        }
    });
}

fn bar_sep(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, CHIP_H), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range().shrink(4.0), Stroke::new(1.0, p().border));
}
