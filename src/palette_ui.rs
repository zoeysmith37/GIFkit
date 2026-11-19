//! The command palette (Ctrl+K): type to find any tool, adjustment, command
//! or recent file, with its shortcut beside it so it can be learnt.

use super::*;
use crate::keys::Action as A;

/// What a palette entry does.
#[derive(Clone)]
pub(super) enum PalCmd {
    Action(A),
    Tool(Tool),
    Quick(Quick),
    Recent(PathBuf),
    ReduceSize,
    Shortcuts,
    AutoCompress,
    NewAdjustLayer,
}

pub(super) struct PalEntry {
    pub icon: &'static str,
    pub title: String,
    pub kind: &'static str,
    pub keys: String,
    /// more words it answers to
    pub also: String,
    pub cmd: PalCmd,
}

#[derive(Default)]
pub(super) struct Palette {
    pub open: bool,
    pub query: String,
    pub sel: usize,
    /// just opened: the field takes the keys
    pub fresh: bool,
}

/// How well `q` matches `text`: its letters in order, earlier and closer
/// together scoring higher. None when they aren't all there.
fn score(q: &str, text: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut s = 0i32;
    let mut at = 0usize;
    let mut last: Option<usize> = None;
    for qc in q.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let i = (at..t.len()).find(|&i| t[i] == qc)?;
        // a run of letters, or the start of a word, counts more
        s += match last {
            Some(l) if l + 1 == i => 8,
            _ if i == 0 || !t[i - 1].is_alphanumeric() => 6,
            _ => 1,
        };
        s -= (i - at).min(10) as i32 / 3;
        last = Some(i);
        at = i + 1;
    }
    Some(s)
}

/// Icons for the commands that don't come with one.
fn action_icon(a: A) -> &'static str {
    match a {
        A::New => "document-new",
        A::Open => "document-open",
        A::Save => "document-save",
        A::SaveAs => "document-save-as",
        A::Export => "document-export",
        A::Quit => "application-exit",
        A::Undo => "edit-undo",
        A::Redo => "edit-redo",
        A::History => "view-history",
        A::PlayPause => "media-playback-start",
        A::PrevFrame => "go-previous",
        A::NextFrame => "go-next",
        A::ZoomIn => "zoom-in",
        A::ZoomOut => "zoom-out",
        A::Fit => "zoom-fit-best",
        A::ActualSize => "zoom-original",
        A::SplitView => "view-split-left-right",
        A::FullScreen => "view-fullscreen",
        A::Handbook => "help-contents",
        A::Duplicate => "edit-duplicate",
        A::DeleteObject => "edit-delete",
        A::OnionSkin => "view-visible",
        A::Compress => "view-refresh",
        A::LayerUp | A::LayerTop => "go-up",
        A::LayerDown | A::LayerBottom => "go-down",
        A::AdjLevels => "view-object-histogram-linear",
        A::AdjCurves => "draw-bezier-curves",
        A::AdjHueSat => "color-mode-hue-shift-positive",
        A::AdjBalance => "color-management",
        A::AdjBlackWhite => "color-mode-black-white",
        A::AdjInvert => "color-mode-invert-image",
        _ => "tools-wizard",
    }
}

impl GifKitApp {
    fn palette_entries(&self) -> Vec<PalEntry> {
        let mut v = Vec::new();
        for t in Tool::ALL {
            let key = match t {
                Tool::Text => Some(A::ToolText),
                Tool::Shapes => Some(A::ToolShapes),
                Tool::Markup => Some(A::ToolMarkup),
                Tool::Emoji => Some(A::ToolEmoji),
                Tool::Mask => Some(A::ToolMask),
                Tool::Adjust => Some(A::ToolAdjust),
                Tool::Effects => Some(A::ToolEffects),
                _ => None,
            };
            v.push(PalEntry { icon: t.breeze(), title: t.label().to_string(), kind: "Tool", keys: key.map(|a| self.keymap.text(a)).unwrap_or_default(), also: t.tip().to_string(), cmd: PalCmd::Tool(t) });
        }
        for (k, name) in crate::adjust::KINDS.iter().enumerate() {
            let key = match k {
                0 => Some(A::AdjLevels),
                1 => Some(A::AdjCurves),
                5 => Some(A::AdjHueSat),
                6 => Some(A::AdjBalance),
                7 => Some(A::AdjBlackWhite),
                11 => Some(A::AdjInvert),
                _ => None,
            };
            v.push(PalEntry { icon: crate::adjust::Adjust::icon(k), title: name.to_string(), kind: "Adjustment", keys: key.map(|a| self.keymap.text(a)).unwrap_or_default(), also: "adjust colour color".into(), cmd: PalCmd::Quick(Quick::Kind(k)) });
        }
        for (q, name, a, also) in [
            (Quick::Desaturate, "Desaturate", A::AdjDesaturate, "grey gray black white"),
            (Quick::AutoTone, "Auto Tone", A::AutoTone, "fix levels"),
            (Quick::AutoContrast, "Auto Contrast", A::AutoContrast, "fix levels"),
            (Quick::AutoColor, "Auto Color", A::AutoColor, "fix cast white balance"),
        ] {
            v.push(PalEntry { icon: "tools-wizard", title: name.into(), kind: "Adjustment", keys: self.keymap.text(a), also: also.into(), cmd: PalCmd::Quick(q) });
        }
        for a in A::ALL {
            // the timeline's own keys and the tool and adjustment keys are above
            if a.timeline() || matches!(a.group(), "Tools" | "Adjustments") || a == A::Palette {
                continue;
            }
            v.push(PalEntry { icon: action_icon(a), title: a.label().to_string(), kind: "Command", keys: self.keymap.text(a), also: a.group().to_string(), cmd: PalCmd::Action(a) });
        }
        v.push(PalEntry { icon: "archive-insert", title: "Reduce Project Size…".into(), kind: "Command", keys: String::new(), also: "shrink smaller file trim compact".into(), cmd: PalCmd::ReduceSize });
        v.push(PalEntry { icon: "configure-shortcuts", title: "Keyboard Shortcuts…".into(), kind: "Command", keys: String::new(), also: "hotkeys keys".into(), cmd: PalCmd::Shortcuts });
        v.push(PalEntry {
            icon: "view-refresh",
            title: if self.auto_compress { "Turn Off Automatic Compression" } else { "Turn On Automatic Compression" }.into(),
            kind: "Setting",
            keys: String::new(),
            also: "preview performance".into(),
            cmd: PalCmd::AutoCompress,
        });
        v.push(PalEntry { icon: "list-add", title: "New Adjustment Layer".into(), kind: "Layer", keys: String::new(), also: "adjust".into(), cmd: PalCmd::NewAdjustLayer });
        for p in self.recent.iter().take(8) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            v.push(PalEntry { icon: "document-open-recent", title: name, kind: "Recent", keys: String::new(), also: p.to_string_lossy().into_owned(), cmd: PalCmd::Recent(p.clone()) });
        }
        v
    }

    fn run_palette(&mut self, ctx: &egui::Context, cmd: PalCmd) {
        let has = self.base.is_some();
        match cmd {
            PalCmd::Action(a) => self.palette_fire = Some(a),
            PalCmd::Tool(t) if has => self.open_tool(t),
            PalCmd::Quick(q) if has => self.quick_adjust(q),
            PalCmd::Recent(p) => self.open_path(p, ctx),
            PalCmd::ReduceSize if has => self.shrink_open = true,
            PalCmd::Shortcuts => self.shortcuts_open = true,
            PalCmd::AutoCompress => {
                self.auto_compress = !self.auto_compress;
                theme::set_setting("auto_compress", if self.auto_compress { "1" } else { "0" });
            }
            PalCmd::NewAdjustLayer if has => {
                self.add_layer(LayerKind::Effects(Box::new(EffectSettings { adjust_layer: true, ..Default::default() })));
                self.tool = Tool::Adjust;
            }
            _ => self.info("Open an animation first"),
        }
        ctx.request_repaint();
    }

    pub(super) fn palette_window(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let pal = *p();
        let entries = self.palette_entries();
        let q = self.palette.query.trim().to_string();
        let mut found: Vec<(i32, usize)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let title = score(&q, &e.title).map(|s| s + 20);
                let other = score(&q, &format!("{} {}", e.kind, e.also));
                title.or(other).map(|s| (s, i))
            })
            .collect();
        // best first; with nothing typed, the order they're listed in
        if !q.is_empty() {
            found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        let shown: Vec<usize> = found.iter().map(|f| f.1).take(12).collect();
        let mut run: Option<usize> = None;
        let mut close = false;
        // keys first, so the field doesn't see them
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                self.palette.sel = (self.palette.sel + 1).min(shown.len().saturating_sub(1));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                self.palette.sel = self.palette.sel.saturating_sub(1);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) && !shown.is_empty() {
                run = Some(shown[self.palette.sel.min(shown.len() - 1)]);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                close = true;
            }
        });
        let screen = ctx.content_rect();
        let w = 560f32.min(screen.width() - 32.0);
        // the rest of the window dims behind it
        egui::Area::new(egui::Id::unique("palette-dim")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(90));
            if r.clicked() {
                close = true;
            }
        });
        egui::Area::new(egui::Id::unique("palette")).order(egui::Order::Tooltip).fixed_pos(pos2(screen.center().x - w / 2.0, screen.top() + 70.0)).show(ctx, |ui| {
            egui::Frame::new()
                .fill(pal.window)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(6.0)
                .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(110) })
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(w - 16.0);
                    ui.horizontal(|ui| {
                        crate::icons::paint(ui.painter(), "edit-find", ui.cursor().min + vec2(10.0, 15.0), 16.0, pal.fg_muted);
                        ui.add_space(24.0);
                        let te = egui::TextEdit::singleline(&mut self.palette.query).hint_text("Type a tool, adjustment or command…").desired_width(f32::INFINITY).font(egui::FontId::proportional(15.0)).frame(egui::Frame::NONE);
                        let r = ui.add_sized(vec2(ui.available_width(), 30.0), te);
                        // the field keeps the keys while the palette is up
                        if self.palette.fresh || !r.has_focus() {
                            r.request_focus();
                            self.palette.fresh = false;
                        }
                        if r.changed() {
                            self.palette.sel = 0;
                        }
                    });
                    ui.add_space(4.0);
                    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().min.y, Stroke::new(1.0, pal.border));
                    ui.add_space(6.0);
                    if shown.is_empty() {
                        ui.add_space(6.0);
                        hint(ui, "  Nothing matches");
                        ui.add_space(6.0);
                    }
                    let sel = self.palette.sel.min(shown.len().saturating_sub(1));
                    for (row, &i) in shown.iter().enumerate() {
                        let e = &entries[i];
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        let on = row == sel;
                        if resp.hovered() && ui.input(|i| i.pointer.delta().length() > 0.0) {
                            self.palette.sel = row;
                        }
                        if on {
                            ui.painter().rect_filled(r, 4.0, pal.highlight);
                        }
                        let fg = if on { Color32::WHITE } else { pal.fg };
                        let muted = if on { Color32::from_white_alpha(200) } else { pal.fg_muted };
                        crate::icons::paint(ui.painter(), e.icon, r.left_center() + vec2(16.0, 0.0), 16.0, fg);
                        ui.painter().text(r.left_center() + vec2(34.0, 0.0), egui::Align2::LEFT_CENTER, &e.title, egui::FontId::proportional(14.0), fg);
                        let mut x = r.right() - 10.0;
                        if !e.keys.is_empty() {
                            let g = ui.painter().layout_no_wrap(e.keys.clone(), egui::FontId::monospace(11.5), muted);
                            let kr = Rect::from_min_size(pos2(x - g.size().x - 10.0, r.center().y - 10.0), vec2(g.size().x + 10.0, 20.0));
                            ui.painter().rect_stroke(kr, 3.0, Stroke::new(1.0, muted.gamma_multiply(0.6)), egui::StrokeKind::Inside);
                            ui.painter().galley(kr.min + vec2(5.0, (20.0 - g.size().y) / 2.0), g, muted);
                            x = kr.left() - 10.0;
                        }
                        ui.painter().text(pos2(x, r.center().y), egui::Align2::RIGHT_CENTER, e.kind, egui::FontId::proportional(12.0), muted);
                        if resp.clicked() {
                            run = Some(i);
                        }
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        hint(ui, "↑↓ to pick · Enter to run · Esc to close");
                    });
                });
        });
        if let Some(i) = run {
            let cmd = entries[i].cmd.clone();
            self.palette = Palette::default();
            self.run_palette(ctx, cmd);
        } else if close {
            self.palette = Palette::default();
        }
    }
}
