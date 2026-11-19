//! A number field you can scrub, as Krita's slider spin boxes: a Breeze
//! frame with the value (and its unit) inside and the accent filling it up
//! to where the value sits in its range. Drag across it to change it
//! (Shift for fine steps, Ctrl to land on round numbers), click it to type.
//! Built like `egui::Slider`, so it takes the same calls.

use super::*;
use egui::emath::Numeric;

/// Settings whose defaults are known while a panel draws them: where the
/// settings sit in memory, how big they are, and a copy of their defaults'
/// bytes. A number field inside them finds its own default by where it
/// sits, so every field of every panel can be reset without each saying so.
struct Scope {
    at: usize,
    len: usize,
    bytes: Vec<u8>,
}

thread_local! {
    static SCOPES: std::cell::RefCell<Vec<Scope>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// While it lives, fields inside `value` reset to their place in `default`.
pub(super) struct Defaults(());

impl Defaults {
    pub fn new<T>(value: &T, default: &T) -> Defaults {
        let len = std::mem::size_of::<T>();
        // SAFETY: `default` is a live `T`, so its `len` bytes can be read.
        // Only the bytes are kept; they are never used as a `T` again, only
        // numbers are read back out of them (see `default_for`)
        let bytes = unsafe { std::slice::from_raw_parts(default as *const T as *const u8, len) }.to_vec();
        SCOPES.with(|s| s.borrow_mut().push(Scope { at: value as *const T as usize, len, bytes }));
        Defaults(())
    }
}

impl Drop for Defaults {
    fn drop(&mut self) {
        SCOPES.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// The default of the number at `v`, when it sits inside settings whose
/// defaults are in scope.
pub(super) fn default_for<N: Numeric>(v: &N) -> Option<f64> {
    let at = v as *const N as usize;
    let n = std::mem::size_of::<N>();
    SCOPES.with(|s| {
        let s = s.borrow();
        let sc = s.iter().rev().find(|sc| at >= sc.at && at + n <= sc.at + sc.len)?;
        let off = at - sc.at;
        // SAFETY: the number lies inside the settings value, so the same
        // place in its default (the same type, the same layout) holds a
        // number of the same type; it's read unaligned from the copy
        let d: N = unsafe { std::ptr::read_unaligned(sc.bytes[off..off + n].as_ptr() as *const N) };
        let d = d.to_f64();
        d.is_finite().then_some(d)
    })
}

type Formatter<'a> = Box<dyn 'a + Fn(f64, std::ops::RangeInclusive<usize>) -> String>;

pub(super) struct Spin<'a> {
    get: Box<dyn 'a + FnMut(Option<f64>) -> f64>,
    range: std::ops::RangeInclusive<f64>,
    integral: bool,
    suffix: String,
    text: String,
    max_decimals: Option<usize>,
    /// None: decided by the range (see `hard`)
    clamping: Option<egui::SliderClamping>,
    log: bool,
    formatter: Option<Formatter<'a>>,
    default: Option<f64>,
}

impl<'a> Spin<'a> {
    pub fn new<N: Numeric>(value: &'a mut N, range: std::ops::RangeInclusive<N>) -> Self {
        let r = range.start().to_f64()..=range.end().to_f64();
        let default = default_for(&*value);
        Spin {
            get: Box::new(move |set| {
                if let Some(v) = set {
                    *value = N::from_f64(v);
                }
                value.to_f64()
            }),
            range: r,
            integral: N::INTEGRAL,
            suffix: String::new(),
            text: String::new(),
            max_decimals: None,
            clamping: None,
            log: false,
            formatter: None,
            default,
        }
    }
    pub fn suffix(mut self, s: impl ToString) -> Self {
        self.suffix = s.to_string();
        self
    }
    pub fn text(mut self, s: impl ToString) -> Self {
        self.text = s.to_string();
        self
    }
    pub fn max_decimals(mut self, n: usize) -> Self {
        self.max_decimals = Some(n);
        self
    }
    pub fn clamping(mut self, c: egui::SliderClamping) -> Self {
        self.clamping = Some(c);
        self
    }
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }
    pub fn custom_formatter(mut self, f: impl 'a + Fn(f64, std::ops::RangeInclusive<usize>) -> String) -> Self {
        self.formatter = Some(Box::new(f));
        self
    }
    /// What it starts as: a dot shows when it's been changed from this,
    /// and right-click sets it back.
    pub fn default_value(mut self, v: f64) -> Self {
        self.default = Some(v);
        self
    }

    fn decimals(&self) -> usize {
        if self.integral {
            return 0;
        }
        self.max_decimals.unwrap_or_else(|| {
            let span = (self.range.end() - self.range.start()).abs();
            if span >= 100.0 {
                0
            } else if span >= 2.0 {
                1
            } else {
                2
            }
        })
    }

    fn show(&self, v: f64) -> String {
        if let Some(f) = &self.formatter {
            return f(v, 0..=self.decimals());
        }
        let d = self.decimals();
        let mut s = format!("{v:.d$}");
        if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        if s == "-0" {
            s = "0".into();
        }
        if self.suffix.is_empty() { s } else { format!("{s}{}", self.suffix) }
    }

    /// Where `v` sits in the range, 0..1 (on a log scale when asked).
    fn frac(&self, v: f64) -> f64 {
        let (a, b) = (*self.range.start(), *self.range.end());
        if b == a {
            return 0.0;
        }
        if self.log && a > 0.0 && b > 0.0 {
            ((v.max(a).ln() - a.ln()) / (b.ln() - a.ln())).clamp(0.0, 1.0)
        } else {
            ((v - a) / (b - a)).clamp(0.0, 1.0)
        }
    }

    fn value_at(&self, t: f64) -> f64 {
        let (a, b) = (*self.range.start(), *self.range.end());
        let t = t.clamp(0.0, 1.0);
        if self.log && a > 0.0 && b > 0.0 {
            (a.ln() + t * (b.ln() - a.ln())).exp()
        } else {
            a + t * (b - a)
        }
    }

    /// Can a typed value go past the range? As Blender's fields: yes, the
    /// range is only how far dragging goes. A true percentage (0 to 100 %)
    /// stays inside it, and so does a field asked to clamp.
    fn hard(&self) -> bool {
        match self.clamping {
            Some(egui::SliderClamping::Always) => true,
            Some(_) => false,
            None => *self.range.start() == 0.0 && *self.range.end() == 100.0 && self.suffix.trim() == "%",
        }
    }

    fn tidy(&self, v: f64, typed: bool) -> f64 {
        let (lo, hi) = (self.range.start().min(*self.range.end()), self.range.start().max(*self.range.end()));
        let v = if typed && !self.hard() { v } else { v.clamp(lo, hi) };
        if self.integral { v.round() } else { v }
    }
}

impl egui::Widget for Spin<'_> {
    fn ui(mut self, ui: &mut egui::Ui) -> egui::Response {
        let pal = *p();
        let h = ui.spacing().interact_size.y.max(22.0);
        let w = ui.available_width().max(60.0);
        let (rect, mut resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
        let id = resp.id;
        let typing_id = id.with("typing");
        let grab_id = id.with("grab");
        let mut v = (self.get)(None);
        let before = v;
        let enabled = ui.is_enabled();
        // a value typed past the range stretches it, so dragging and the
        // fill work around it; back inside (a reset), the range is as it was
        if !self.hard() && v.is_finite() {
            let (a, b) = (*self.range.start(), *self.range.end());
            if a <= b && (v < a || v > b) {
                self.range = a.min(v)..=b.max(v);
            }
        }

        // typing a value in
        let typing: Option<String> = ui.data(|d| d.get_temp(typing_id));
        if let Some(mut buf) = typing {
            let te = egui::TextEdit::singleline(&mut buf).align(egui::Align2::CENTER_CENTER).margin(vec2(6.0, 3.0));
            let r = ui.put(rect, te);
            if ui.data(|d| d.get_temp::<bool>(id.with("fresh"))).unwrap_or(false) {
                r.request_focus();
                if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(buf.chars().count()))));
                    st.store(ui.ctx(), r.id);
                }
                ui.data_mut(|d| d.remove::<bool>(id.with("fresh")));
            }
            let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if esc {
                ui.data_mut(|d| d.remove::<String>(typing_id));
            } else if enter || r.lost_focus() {
                let t: String = buf.trim().trim_end_matches(self.suffix.trim()).replace(['°', '%', ','], ".").replace("..", ".");
                let t = t.trim().trim_end_matches(char::is_alphabetic).trim().to_string();
                if let Ok(x) = t.parse::<f64>() {
                    v = self.tidy(x, true);
                    (self.get)(Some(v));
                }
                ui.data_mut(|d| d.remove::<String>(typing_id));
            } else {
                ui.data_mut(|d| d.insert_temp(typing_id, buf));
            }
            if v != before {
                resp.mark_changed();
            }
            return resp;
        }

        // scrubbing: the value follows the pointer's movement, not where it
        // was pressed, so grabbing it never makes the value jump
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(grab_id, (self.frac(v), v)));
        }
        if resp.dragged() {
            let (mut t, start) = ui.data(|d| d.get_temp::<(f64, f64)>(grab_id)).unwrap_or((self.frac(v), v));
            let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            let dx = resp.drag_delta().x as f64 / rect.width().max(1.0) as f64;
            t += dx * if shift { 0.1 } else { 1.0 };
            ui.data_mut(|d| d.insert_temp(grab_id, (t, start)));
            let mut nv = self.value_at(t);
            if ctrl {
                // round numbers: steps of a tenth of a power of ten of the span
                let span = (self.range.end() - self.range.start()).abs().max(1e-9);
                let step = 10f64.powf((span / 10.0).log10().floor());
                nv = (nv / step).round() * step;
            } else if !self.integral {
                let k = 10f64.powi(self.decimals() as i32);
                nv = (nv * k).round() / k;
            }
            v = self.tidy(nv, false);
            (self.get)(Some(v));
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<(f64, f64)>(grab_id));
        }
        if resp.clicked() && enabled {
            ui.data_mut(|d| {
                d.insert_temp(typing_id, self.show(v).trim_end_matches(self.suffix.as_str()).to_string());
                d.insert_temp(id.with("fresh"), true);
            });
            ui.ctx().request_repaint();
        }
        // right-click: back to where it started
        if let Some(def) = self.default {
            let shown = self.show(def);
            let mut reset = false;
            resp.context_menu(|ui| {
                if ui.button(format!("Reset to {shown}")).clicked() {
                    reset = true;
                    ui.close();
                }
            });
            if reset {
                v = def;
                (self.get)(Some(v));
            }
        }
        if resp.hovered() && enabled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if v != before {
            resp.mark_changed();
        }

        // drawing it, as a Breeze line edit with the accent as its fill
        let hot = resp.hovered() || resp.dragged();
        let painter = ui.painter_at(rect.expand(1.0));
        let frame = if !enabled { mix(pal.view, pal.window, 0.5) } else { pal.view };
        let edge = if resp.dragged() || resp.has_focus() { pal.highlight } else if hot && enabled { mix(theme::outline(&pal), pal.highlight, 0.5) } else { theme::outline(&pal) };
        painter.rect(rect, 3.0, frame, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        let t = self.frac(v) as f32;
        if t > 0.0 {
            let fill = Rect::from_min_max(rect.min + vec2(1.0, 1.0), pos2(rect.left() + 1.0 + (rect.width() - 2.0) * t, rect.bottom() - 1.0));
            let col = if enabled { mix(pal.view, pal.highlight, if hot { 0.5 } else { 0.38 }) } else { mix(pal.view, pal.fg_muted, 0.18) };
            painter.rect_filled(fill, egui::CornerRadius { nw: 2, sw: 2, ne: if t > 0.99 { 2 } else { 0 }, se: if t > 0.99 { 2 } else { 0 } }, col);
        }
        let changed_from_default = self.default.is_some_and(|d| (d - v).abs() > 1e-6);
        let fg = if !enabled { pal.fg_muted } else { pal.fg };
        let font = egui::TextStyle::Button.resolve(ui.style());
        if !self.text.is_empty() {
            painter.text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &self.text, font.clone(), pal.fg_muted);
        }
        let label = self.show(v);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, fg);
        if changed_from_default {
            painter.circle_filled(pos2(rect.right() - 7.0, rect.center().y), 2.5, pal.highlight);
        }
        resp
    }
}
