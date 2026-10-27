//! The timeline dock under the viewport, in the spirit of Kdenlive: a ruler
//! with the playhead, the GIF's own track, and one track per layer whose
//! clips can be moved, trimmed, split and deleted, with keyframes on top.

use super::*;
use crate::timeline::{Clip, Ease, Key, Marker};

/// Markers on the ruler (Breeze's orange).
const MARK: Color32 = Color32::from_rgb(246, 116, 0);

/// The GIF track's id in the timeline (layers use their own ids, which start at 1).
const GIF: u64 = 0;
const RULER_H: f32 = 22.0;
const EDGE: f32 = 5.0;
/// room before frame 1, so a clip's start edge is easy to grab
const PAD: f32 = 14.0;
/// most pixels per frame when zoomed in
const ZOOM_MAX: f32 = 400.0;
/// How far out past fitting everything the timeline zooms (a share of the fit).
const ZOOM_OUT: f32 = 0.25;
/// the scroll bar along the bottom
const BAR_H: f32 = 9.0;

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

/// The timeline toolbar's actions.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TbAct {
    Select,
    Razor,
    Split,
    Join,
    AddClip,
    Delete,
    Copy,
    Paste,
    Duplicate,
    KeyAdd,
    KeyRemove,
    PrevEdit,
    NextEdit,
    In,
    Out,
    Marker,
    Snap,
    Loop,
    TrimEnd,
}

/// What Copy put aside: clips as (offset from the first, length), or a keyframe.
#[derive(Clone)]
pub(super) enum TlBoard {
    Clips(Vec<(usize, usize)>),
    Key(Key),
    /// GIF clips: their frames, and each image layer's placements on them
    Frames(Vec<crate::anim::Frame>, Vec<(u64, Vec<crate::overlay::Placement>)>),
}

/// What making the GIF longer adds (the GIF row's right-click menu).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Extend {
    /// the last frame stays up
    Hold,
    /// it plays again from the start
    Loop,
    /// the same frames, spread over the longer time
    Stretch,
}

impl Extend {
    pub(super) fn load() -> Extend {
        match theme::get_setting("tl_extend").as_deref() {
            Some("loop") => Extend::Loop,
            Some("stretch") => Extend::Stretch,
            _ => Extend::Hold,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Extend::Hold => "hold",
            Extend::Loop => "loop",
            Extend::Stretch => "stretch",
        }
    }
}

/// Where a clip `len` frames long fits among `spans` (others on its row,
/// itself left out) nearest to starting at `want`: anywhere when `after`
/// is false, else the first room at or after it.
fn free_slot(spans: &[(usize, usize)], len: usize, want: usize, n: usize, after: bool) -> Option<usize> {
    if len == 0 || len > n {
        return None;
    }
    let fits = |a: usize| a + len <= n && !spans.iter().any(|&(x, y)| a <= y && a + len - 1 >= x);
    let last = n - len;
    let want = want.min(last);
    if fits(want) {
        return Some(want);
    }
    // the room around each clip's ends is where a fit starts
    let mut cands: Vec<usize> = vec![0, last];
    for &(x, y) in spans {
        cands.push(y + 1);
        if x >= len {
            cands.push(x - len);
        }
    }
    cands.into_iter().filter(|&a| a <= last && fits(a) && (!after || a >= want)).min_by_key(|&a| a.abs_diff(want))
}

/// The GIF's frames (as shown) with clip `a..=b` of `d0` given new edges
/// `na..=nb`: what it gives up becomes a gap, and the gap it's pulled into
/// fills by holding its end, playing it again or stretching it.
fn gif_edge_fill(d0: &[crate::anim::Frame], (a, b): (usize, usize), (na, nb): (usize, usize), how: Extend) -> Vec<crate::anim::Frame> {
    let mut d = d0.to_vec();
    if b >= d.len() || nb >= d.len() || na > nb {
        return d;
    }
    for i in a..=b {
        if i < na || i > nb {
            d[i].blank = true;
        }
    }
    let clip = &d0[a..=b];
    let len = clip.len();
    if na < a || nb > b {
        let total = nb + 1 - na;
        // the part of the clip that stays, for stretching
        let (k0, k1) = (na.max(a) - a, nb.min(b) - a);
        for j in 0..total {
            let pos = na as i64 + j as i64 - a as i64;
            let src = match how {
                Extend::Hold => &clip[pos.clamp(0, len as i64 - 1) as usize],
                Extend::Loop => &clip[pos.rem_euclid(len as i64) as usize],
                Extend::Stretch => &clip[k0 + j * (k1 + 1 - k0) / total],
            };
            d[na + j] = crate::anim::Frame { cut: false, blank: false, enabled: true, ..src.clone() };
        }
    }
    d[na].cut = true;
    if let Some(f) = d.get_mut(nb + 1) {
        f.cut = true;
    }
    d
}

/// What the timeline puts on the system clipboard, so Ctrl+V comes back here.
pub(super) const BOARD_TAG: &str = "GIFkit timeline clips";

/// Track colours by layer kind, mixed into the view colour.
fn kind_colour(k: Kind) -> Color32 {
    match k {
        Kind::Effects => Color32::from_rgb(155, 89, 182),
        Kind::Animation => Color32::from_rgb(218, 68, 83),
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
        // gaps (blank frames) and skipped frames aren't part of any clip
        let edit = self.gif_edit_len();
        let shown: Vec<bool> = self.plan[..edit].iter().map(|f| base.frames[f.src].enabled && !base.frames[f.src].blank).collect();
        // a split starts a new clip
        let cut: Vec<bool> = self.plan[..edit].iter().map(|f| base.frames[f.src].cut).collect();
        let mut v = Vec::new();
        let mut i = 0;
        while i < shown.len() {
            if shown[i] {
                let s = i;
                while i + 1 < shown.len() && shown[i + 1] && !cut[i + 1] {
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

    /// Show the source frames behind output frames `a..=b`, or leave a gap
    /// there: the frames keep their time but show nothing, as a timeline does.
    fn gif_set(&mut self, a: usize, b: usize, on: bool) {
        let Some(base) = self.base.as_mut() else { return };
        for i in a..=b.min(self.plan.len().saturating_sub(1)) {
            let f = &mut base.frames[self.plan[i].src];
            f.blank = !on;
            if on {
                f.enabled = true;
            }
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
        let Some(track) = self.tl_track else { return };
        self.tl_split_at(track, self.cur, now);
    }

    /// Split `track` so a clip starts at output frame `f`.
    fn tl_split_at(&mut self, track: u64, f: usize, now: f64) {
        let n = self.plan.len();
        if track == GIF {
            // the frame where the new clip starts carries the split
            let runs = self.gif_runs();
            let Some(ci) = runs.iter().position(|&(a, b)| f > a && f <= b) else { return };
            self.snapshot(&format!("tl-split{now}"), now);
            let src = self.plan[f].src;
            if let Some(base) = self.base.as_mut() {
                base.frames[src].cut = true;
            }
            self.modified = true;
            self.tl_clip = Some(ci + 1);
            return;
        }
        self.snapshot(&format!("tl-split{now}"), now);
        let done = self.tl_layer_mut(track).map(|l| l.timing.split(f, n).then(|| l.timing.spans(n).iter().position(|s| s.0 == f))).flatten();
        if let Some(c) = done {
            self.modified = true;
            self.tl_clip = c;
        }
    }

    /// Join the selected clip with the one right after it.
    fn tl_join(&mut self, now: f64) {
        let (Some(track), Some(c)) = (self.tl_track, self.tl_clip) else { return };
        let n = self.plan.len();
        let spans = self.track_spans(track);
        let (Some(&(a, b)), Some(&(na, nb))) = (spans.get(c), spans.get(c + 1)) else { return };
        if na != b + 1 {
            return;
        }
        self.snapshot(&format!("tl-join{now}"), now);
        if track == GIF {
            let src = self.plan[na].src;
            if let Some(base) = self.base.as_mut() {
                base.frames[src].cut = false;
            }
        } else if let Some(l) = self.tl_layer_mut(track) {
            if let Some(cl) = &mut l.timing.clips {
                cl.sort_by_key(|c| c.start);
                if c + 1 < cl.len() {
                    cl.remove(c + 1);
                    cl[c] = Clip { start: a, end: if nb + 1 >= n { None } else { Some(nb) } };
                }
            }
        }
        self.modified = true;
    }

    /// Can the selected clip be joined with the next one (they touch)?
    fn tl_can_join(&self) -> bool {
        let (Some(track), Some(c)) = (self.tl_track, self.tl_clip) else { return false };
        let spans = self.track_spans(track);
        matches!((spans.get(c), spans.get(c + 1)), (Some(&(_, b)), Some(&(na, _))) if na == b + 1)
    }

    /// Move the GIF's clip `a..=b` (output frames) so it starts at output
    /// boundary `to`: its frames move in the animation, the clips after it
    /// close up, and layers stay where they are in time.
    fn tl_gif_move(&mut self, a: usize, b: usize, to: usize, now: f64) {
        let n = self.gif_edit_len();
        if b >= n || to > n || (to >= a && to <= b + 1) {
            return;
        }
        let Some((s0, e0, mut d)) = self.gif_display() else { return };
        self.snapshot(&format!("tl-gifmove{now}"), now);
        let mut block: Vec<crate::anim::Frame> = d.drain(a..=b).collect();
        let len = block.len();
        // what followed the clip starts a clip of its own now
        if let Some(f) = d.get_mut(a) {
            f.cut = true;
        }
        block[0].cut = true;
        let at = if to > b { to - len } else { to };
        d.splice(at..at, block);
        if let Some(f) = d.get_mut(at + len) {
            f.cut = true;
        }
        self.gif_write(s0, e0, d);
        self.tl_clip = self.gif_runs().iter().position(|r| r.0 == at);
        self.cur = at.min(self.plan.len().saturating_sub(1));
        self.info(format!("Moved {len} frames"));
    }

    fn tl_delete(&mut self, now: f64) {
        let Some(track) = self.tl_track else { return };
        if self.tl_locked(track) {
            self.info("The layer is locked");
            return;
        }
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
        self.tl_area = ui.max_rect();
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

        // ---- toolbar: groups that don't fit go into a menu at the end
        let locked_sel = self.tl_track.is_some_and(|t| self.tl_locked(t));
        let gap_here = sel_layer.is_some_and(|t| !self.track_spans(t).iter().any(|&(a, b)| self.cur >= a && self.cur <= b));
        let on_key = sel_layer.and_then(|t| self.stack.index_of(t)).is_some_and(|li| self.stack.layers[li].timing.key_at(self.cur).is_some());
        let any_clips = !self.tl_selected().is_empty();
        let gif_board = matches!(self.tl_board, Some(TlBoard::Frames(..)));
        let km = self.keymap.clone();
        use crate::keys::Action as A;
        let k = |a: A| {
            let t = km.text(a);
            if t.is_empty() { String::new() } else { format!(" ({t})") }
        };
        // (action, icon, menu label, tip, enabled, toggled)
        type Item = (TbAct, &'static str, &'static str, String, bool, Option<bool>);
        let mut groups: Vec<Vec<Item>> = vec![
            vec![
                (TbAct::Select, "edit-select", "Selection Tool", format!("Selection tool: move and trim clips{}", k(A::TlRazor)), true, Some(!self.tl_razor)),
                (TbAct::Razor, "edit-cut", "Razor Tool", format!("Razor tool: click a clip to cut it there{}", k(A::TlRazor)), true, Some(self.tl_razor)),
            ],
            vec![
                (TbAct::Split, "split", "Split at Playhead", format!("Split the selected track at the playhead{}", k(A::TlSplit)), idle && self.tl_track.is_some() && !locked_sel, None),
                (TbAct::Join, "merge", "Join with Next Clip", "Join the selected clip with the one right after it".into(), idle && self.tl_can_join() && !locked_sel, None),
                (TbAct::AddClip, "list-add", "Add Clip at Playhead", "Add a clip at the playhead (on an empty part of the selected track)".into(), idle && gap_here && !locked_sel, None),
                (TbAct::Delete, "edit-delete", "Delete", format!("Delete the selected clips or keyframe{}. On the GIF track it leaves a gap.", k(A::TlDelete)), idle && (self.tl_clip.is_some() || self.tl_key.is_some() || !self.tl_keys.is_empty() || !self.tl_sel.is_empty()) && !locked_sel, None),
            ],
            vec![
                (TbAct::Copy, "edit-copy", "Copy", "Copy the selected clips or keyframe (Ctrl+C). GIF clips copy their frames.".into(), any_clips || self.tl_key.is_some(), None),
                (TbAct::Paste, "edit-paste", "Paste", "Paste at the playhead (Ctrl+V). GIF frames go in after the clip under the playhead.".into(), idle && self.tl_board.is_some() && (gif_board || (sel_layer.is_some() && !locked_sel)), None),
                (TbAct::Duplicate, "edit-duplicate", "Duplicate", format!("Put a copy of the selected clips right after them{}", k(A::TlDuplicate)), idle && any_clips && !locked_sel, None),
            ],
            vec![
                (TbAct::KeyAdd, "keyframe-add", "Add Keyframe", format!("Add a keyframe on the selected layer at the playhead{}", k(A::TlKey)), idle && sel_layer.is_some() && !locked_sel, None),
                (TbAct::KeyRemove, "keyframe-remove", "Remove Keyframe", "Remove the keyframe at the playhead".into(), idle && on_key && !locked_sel, None),
                (TbAct::PrevEdit, "keyframe-previous", "Previous Edit", format!("Playhead to the previous clip edge, keyframe or marker{}", k(A::TlPrevEdit)), true, None),
                (TbAct::NextEdit, "keyframe-next", "Next Edit", format!("Playhead to the next clip edge, keyframe or marker{}", k(A::TlNextEdit)), true, None),
            ],
            vec![
                (TbAct::In, "zone-in", "Set In Point", format!("Set the in point at the playhead{}", k(A::TlIn)), true, None),
                (TbAct::Out, "zone-out", "Set Out Point", format!("Set the out point at the playhead{}", k(A::TlOut)), true, None),
                (TbAct::Marker, "bookmark-new", "Add Marker", format!("Add a marker at the playhead{}", k(A::TlMarker)), true, None),
                (TbAct::TrimEnd, "transform-crop", "Trim to Last Clip", "Shorten the animation to where its last clip ends".into(), idle && self.tl_last_clip_end().is_some_and(|e| e + 1 < self.gif_edit_len()), None),
            ],
            vec![(TbAct::Snap, "snap", "Snap", "Snap: clip edges and keyframes jump to the playhead, other clips, keyframes and markers nearby (hold Shift while dragging to place freely)".into(), true, Some(self.tl_snap))],
        ];
        if self.tl_zone().is_some() {
            groups.last_mut().unwrap().push((TbAct::Loop, "media-playlist-repeat", "Loop In to Out", "Loop playback between the in and out points".into(), true, Some(self.tl_zone_loop)));
        }
        let (_, icon_px) = theme::toolbar_style();
        let bw = icon_px + 14.0;
        let mut act: Option<TbAct> = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let avail = ui.available_width();
            // the zoom controls on the right; the slider goes first when it's narrow
            let slider_on = avail > 560.0;
            // widths as drawn last time (estimates the first time), so the
            // groups that don't fit go into the menu instead of under the zoom
            let mem = egui::Id::unique("tl-toolbar-widths");
            let (seen_groups, seen_title, seen_right): (Vec<f32>, f32, [f32; 2]) = ui.ctx().data(|d| d.get_temp(mem)).unwrap_or_default();
            let right_w = seen_right[usize::from(slider_on)].max(bw * 3.0 + if slider_on { 110.0 } else { 0.0 } + 8.0) + 8.0;
            let title_w = seen_title.max(76.0);
            let group_w = |gi: usize, g: &Vec<Item>| seen_groups.get(gi).copied().filter(|w| *w > 0.0).unwrap_or(g.len() as f32 * (bw + 2.0) + 14.0);
            let total: f32 = groups.iter().enumerate().map(|(gi, g)| group_w(gi, g)).sum();
            let room = avail - right_w - title_w;
            let fits_all = total <= room;
            let mut used = 0.0;
            let mut cut = groups.len();
            if !fits_all {
                for (gi, g) in groups.iter().enumerate() {
                    if used + group_w(gi, g) > room - bw - 16.0 {
                        cut = gi;
                        break;
                    }
                    used += group_w(gi, g);
                }
            }
            let x0 = ui.cursor().min.x;
            ui.label(RichText::new("Timeline").font(theme::medium(13.5)).color(pal.fg));
            ui.add_space(8.0);
            let new_title = ui.cursor().min.x - x0;
            let mut new_groups = seen_groups.clone();
            new_groups.resize(groups.len(), 0.0);
            for (gi, g) in groups.iter().enumerate().take(cut) {
                let gx = ui.cursor().min.x;
                if gi > 0 {
                    toolbar_sep(ui);
                }
                for (a, icon, _, tip, en, tog) in g {
                    let hit = match tog {
                        Some(on) => ui.add_enabled_ui(*en, |ui| tb_toggle(ui, icon, *on, tip)).inner,
                        None => tb_tool(ui, icon, "", tip, *en).clicked(),
                    };
                    if hit {
                        act = Some(*a);
                    }
                }
                new_groups[gi] = ui.cursor().min.x - gx;
            }
            if cut < groups.len() {
                // the rest, in a menu
                toolbar_sep(ui);
                let r = tb_tool(ui, "overflow-menu", "", "More timeline tools", true);
                egui::Popup::menu(&r).show(|ui| {
                    ui.set_min_width(220.0);
                    for (gi, g) in groups.iter().enumerate().skip(cut) {
                        if gi > cut {
                            ui.separator();
                        }
                        for (a, icon, label, _, en, tog) in g {
                            let text = if tog == &Some(true) { format!("{label} ✓") } else { label.to_string() };
                            if menu_item(ui, icon, &text, "", *en) {
                                act = Some(*a);
                            }
                        }
                    }
                });
            }
            let mut new_right = seen_right;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rx = ui.cursor().max.x;
                if tb_tool(ui, "zoom-fit-best", "", "Fit the whole animation", true).clicked() {
                    self.tl_zoom = 0.0;
                    self.tl_x0 = 0.0;
                }
                let (fit, cur_z) = (self.tl_fit.max(1e-3), self.tl_zoom_now.max(1e-3));
                let top = ZOOM_MAX.max(fit * 1.01);
                // the slider runs from all the way out to all the way in
                let fit = fit * ZOOM_OUT;
                if tb_tool(ui, "zoom-in", "", "Zoom in (Ctrl+wheel)", cur_z < top * 0.999).clicked() {
                    self.tl_zoom_to = Some(cur_z * 1.5);
                }
                if slider_on {
                    // a zoom slider, as in Kdenlive: fit at the left, closest at the right
                    let mut t = ((cur_z / fit).ln() / (top / fit).ln()).clamp(0.0, 1.0);
                    ui.spacing_mut().slider_width = 90.0;
                    if slider(ui, egui::Slider::new(&mut t, 0.0..=1.0).show_value(false)).on_hover_text("Zoom the timeline (Ctrl+wheel). Drag with the middle button to pan.").changed() {
                        self.tl_zoom_to = Some(fit * (top / fit).powf(t));
                    }
                }
                if tb_tool(ui, "zoom-out", "", "Zoom out (Ctrl+wheel)", cur_z > fit * 1.001).clicked() {
                    self.tl_zoom_to = Some(cur_z / 1.5);
                }
                new_right[usize::from(slider_on)] = rx - ui.cursor().max.x;
                if fits_all {
                    let t = self.plan_time(self.cur);
                    let text = RichText::new(format!("{:.2} s · frame {} of {n}", t.max(0.0), self.cur + 1)).monospace().size(12.0).color(pal.fg_muted);
                    // only when it fits beside the buttons
                    let w = ui.painter().layout_no_wrap(text.text().to_string(), egui::FontId::monospace(12.0), pal.fg).size().x;
                    if ui.available_width() > w + 12.0 {
                        ui.label(text);
                    }
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(mem, (new_groups, new_title, new_right)));
        });
        match act {
            Some(TbAct::Select) => self.tl_razor = false,
            Some(TbAct::Razor) => self.tl_razor = true,
            Some(TbAct::Split) => self.tl_split(now),
            Some(TbAct::Join) => self.tl_join(now),
            Some(TbAct::AddClip) => self.tl_add_clip(now),
            Some(TbAct::Delete) => self.tl_delete_selected(now),
            Some(TbAct::Copy) => self.tl_copy(ctx),
            Some(TbAct::Paste) => self.tl_paste(now),
            Some(TbAct::Duplicate) => self.tl_duplicate(now),
            Some(TbAct::KeyAdd) => self.tl_add_key(now),
            Some(TbAct::KeyRemove) => self.tl_delete(now),
            Some(TbAct::PrevEdit) => self.tl_jump(false),
            Some(TbAct::NextEdit) => self.tl_jump(true),
            Some(TbAct::In) => self.tl_set_in(Some(self.cur)),
            Some(TbAct::Out) => self.tl_set_out(Some(self.cur)),
            Some(TbAct::Marker) => self.tl_add_marker(now),
            Some(TbAct::Snap) => {
                self.tl_snap = !self.tl_snap;
                theme::set_setting("tl_snap", if self.tl_snap { "1" } else { "0" });
            }
            Some(TbAct::Loop) => self.tl_zone_loop = !self.tl_zone_loop,
            Some(TbAct::TrimEnd) => self.tl_trim_to_last(now),
            None => {}
        }
        // the selected layer's motion at the playhead. Changing a value makes
        // a keyframe there if there isn't one (auto key), so the row never
        // greys out or changes shape.
        let sel_li = sel_layer.and_then(|t| self.stack.index_of(t));
        let key_here = sel_li.and_then(|li| self.stack.layers[li].timing.key_at(self.cur).map(|k| (li, k)));
        if self.tl_drag.is_none() {
            // one Alt+clicked out stays out until the playhead moves off it
            if self.tl_key_dropped.is_some_and(|(t, f)| Some(t) != sel_layer || f != self.cur) {
                self.tl_key_dropped = None;
            }
            self.tl_key = key_here.map(|(_, k)| k).filter(|_| self.tl_key_dropped.is_none());
        }
        let mut key = match (key_here, sel_li) {
            (Some((li, k)), _) => self.stack.layers[li].timing.keys[k],
            (None, Some(li)) => {
                let m = self.stack.layers[li].timing.motion(self.cur);
                Key { frame: self.cur, x: m.dx, y: m.dy, scale_x: m.sx * 100.0, scale_y: m.sy * 100.0, angle: m.angle, opacity: m.opacity * 100.0, ease: Ease::Smooth }
            }
            _ => Key::default(),
        };
        let before = key;
        // where the layer sits, so the row can show where it is rather than
        // how far it moved: the box's left edge, and its bottom edge measured
        // up from the bottom of the frame
        let src = self.plan.get(self.cur).map(|f| f.src).unwrap_or(0);
        let frame_h = self.dims().1 as f32;
        let placed = sel_li.and_then(|li| self.stack.layers[li].source_box(src, &self.fonts));
        let (mut px, mut py) = match placed {
            Some(b) => (b.cx - b.w * key.scale_x / 200.0 + key.x, frame_h - (b.cy + key.y + b.h * key.scale_y / 200.0)),
            None => (0.0, 0.0),
        };
        let (px0, py0) = (px, py);
        // auto key: a record button, as in other animation tools
        let mut auto = self.tl_autokey;
        let editable = sel_li.is_some_and(|li| !self.stack.layers[li].locked) && (auto || key_here.is_some());
        // whole groups wrap onto more lines in a narrow window (never a
        // label without its fields)
        let field = |ui: &mut egui::Ui, prefix: &str, v: &mut f32, suffix: &str, step: f64, w: f32| {
            spin(ui, vec2(w, CHIP_H), v, step, |d| d.speed(0.5).max_decimals(1).prefix(prefix.to_string()).suffix(suffix.to_string()));
        };
        let text_w = |t: &str| ui.painter().layout_no_wrap(t.into(), egui::TextStyle::Body.resolve(ui.style()), pal.fg).size().x;
        let gap = 6.0;
        // (group, width): 0 auto key, 1 position, 2 scale, 3 angle, 4 opacity, 5 ease
        let widths = [
            CHIP_H + gap + 16.0,
            text_w("Position:") + gap * 2.0 + 108.0 * 2.0,
            text_w("Scale:") + gap * 2.0 + 88.0 * 2.0,
            text_w("Angle:") + gap + 70.0,
            text_w("Opacity:") + gap + 80.0,
            96.0,
        ];
        let avail = ui.available_width();
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        for (g, w) in widths.iter().enumerate() {
            let need = w + if lines.last().unwrap().is_empty() { 0.0 } else { 14.0 };
            if used + need > avail && !lines.last().unwrap().is_empty() {
                lines.push(Vec::new());
                used = 0.0;
            }
            used += if lines.last().unwrap().is_empty() { *w } else { need };
            lines.last_mut().unwrap().push(g);
        }
        let pos_on = editable && placed.is_some();
        for line in lines {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (k, g) in line.into_iter().enumerate() {
                    if k > 0 {
                        ui.add_space(8.0);
                    }
                    match g {
                        0 => {
                            let b = ui
                                .add(sel_btn(egui::Button::new("").frame_when_inactive(auto).min_size(vec2(CHIP_H, CHIP_H)), auto))
                                .on_hover_text(if auto { "Auto key is on: changing a value adds a keyframe at the playhead. Click to turn it off." } else { "Auto key is off: only keyframes already there can be changed. Click to turn it on." });
                            crate::icons::paint(ui.painter(), "keyframe-record", b.rect.center(), 16.0, if auto { pal.fg } else { pal.fg_muted });
                            if b.clicked() {
                                auto = !auto;
                            }
                            let (r, _) = ui.allocate_exact_size(vec2(16.0, CHIP_H), Sense::hover());
                            // filled on a keyframe, hollow between them
                            if key_here.is_some() {
                                diamond(ui.painter(), r.center(), 6.0, pal.highlight, Stroke::NONE);
                            } else {
                                diamond(ui.painter(), r.center(), 5.5, Color32::TRANSPARENT, Stroke::new(1.3, pal.fg_muted));
                            }
                        }
                        // effects, watermarks and the AI shield cover the whole frame: they only fade
                        1 => {
                            ui.add_enabled_ui(pos_on, |ui| {
                                ui.label("Position:").on_hover_text("The layer's left edge, and its bottom edge up from the bottom of the frame, in pixels");
                                field(ui, "X  ", &mut px, " px", 1.0, 108.0);
                                field(ui, "Y  ", &mut py, " px", 1.0, 108.0);
                            });
                        }
                        2 => {
                            ui.add_enabled_ui(pos_on, |ui| {
                                ui.label("Scale:").on_hover_text("Width and height, % of the size it was placed at");
                                field(ui, "X  ", &mut key.scale_x, "%", 5.0, 88.0);
                                field(ui, "Y  ", &mut key.scale_y, "%", 5.0, 88.0);
                            });
                        }
                        3 => {
                            ui.add_enabled_ui(pos_on, |ui| {
                                ui.label("Angle:");
                                field(ui, "", &mut key.angle, "°", 5.0, 70.0);
                            });
                        }
                        4 => {
                            ui.add_enabled_ui(editable, |ui| {
                                ui.label("Opacity:");
                                spin(ui, vec2(80.0, CHIP_H), &mut key.opacity, 5.0, |d| d.range(0.0..=100.0).max_decimals(0).suffix("%"));
                            });
                        }
                        _ => {
                            ui.add_enabled_ui(editable, |ui| {
                                egui::ComboBox::from_id_salt("tl-ease")
                                    .selected_text(key.ease.label())
                                    .width(86.0)
                                    .show_ui(ui, |ui| {
                                        for e in Ease::ALL {
                                            ui.selectable_value(&mut key.ease, e, e.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text("How the motion arrives at and leaves this keyframe. Hold keeps it still until the next one.");
                            });
                        }
                    }
                }
            });
        }
        // back from where it is to how far it moved
        if let Some(b) = placed {
            if px != px0 {
                key.x = px - (b.cx - b.w * key.scale_x / 200.0);
            }
            if py != py0 {
                key.y = (frame_h - py) - b.h * key.scale_y / 200.0 - b.cy;
            }
        }
        if auto != self.tl_autokey {
            self.tl_autokey = auto;
            theme::set_setting("auto_key", if auto { "1" } else { "0" });
        }
        if let Some(li) = sel_li.filter(|_| editable) {
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
        // a little room before frame 1 so clip starts are easy to grab
        let lane_w = (lanes.width() - PAD).max(10.0);
        // frames to pixels; zoom 0 fits everything, with a little room after
        // the end to drag the GIF longer into
        let span = (n as f32 * 1.08 + 2.0).max(n as f32 + 24.0);
        let fit = lane_w / span;
        self.tl_fit = fit;
        // the toolbar's zoom, about the playhead (kept where it is on screen)
        if let Some(z) = self.tl_zoom_to.take() {
            let old = if self.tl_zoom <= 0.0 { fit } else { self.tl_zoom.max(fit * ZOOM_OUT) };
            let nz = z.clamp(fit * ZOOM_OUT, ZOOM_MAX.max(fit));
            let anchor = self.cur as f32 + 0.5;
            let rel = (anchor - self.tl_x0) / (lane_w / old);
            let rel = if (0.0..=1.0).contains(&rel) { rel } else { 0.5 };
            self.tl_zoom = if (nz - fit).abs() <= fit * 0.0001 { 0.0 } else { nz };
            self.tl_x0 = anchor - rel * lane_w / nz;
        }
        let zoom = if self.tl_zoom <= 0.0 { fit } else { self.tl_zoom.max(fit * ZOOM_OUT) };
        self.tl_zoom_now = zoom;
        let visible = lane_w / zoom;
        // follow the playhead while playing, and when it's stepped (arrow
        // keys, jumps to edits): once it's out of view, the view moves on.
        // Left alone while paused, the view stays wherever it was scrolled.
        let moved = self.cur != self.tl_cur_seen;
        self.tl_cur_seen = self.cur;
        if (self.playing || moved) && self.tl_drag.is_none() {
            if self.cur as f32 + 1.0 > self.tl_x0 + visible {
                self.tl_x0 = self.cur as f32 - visible * 0.1;
            } else if (self.cur as f32) < self.tl_x0 {
                // going back: the frames before it come into view
                self.tl_x0 = self.cur as f32 + 1.0 - visible * 0.9;
            }
        }
        self.tl_x0 = self.tl_x0.clamp(0.0, (span - visible).max(0.0));
        let x0 = self.tl_x0;
        let fx = |f: f32| lanes.min.x + PAD + (f - x0) * zoom;
        let (gif_h, lay_h) = (self.tl_gif_h, self.tl_row_h);
        let rows_h = gif_h + (tracks.len() - 1) as f32 * lay_h;
        let tracks_top = area.min.y + RULER_H;
        let tracks_h = (area.max.y - tracks_top - BAR_H).max(10.0);
        // wheel: Ctrl zooms about the pointer, Shift pans; plain scrolls the
        // tracks, or pans when they all fit
        if resp.hovered() {
            let (scroll, ctrl, shift, zd) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command, i.modifiers.shift, i.zoom_delta()));
            // egui turns Ctrl+wheel (and pinches) into a zoom factor, not a scroll
            // (about 15% a notch, as Kdenlive does)
            let factor = if zd != 1.0 { zd.powf(3.0) } else if ctrl && scroll.y != 0.0 { 1.0 + scroll.y * 0.004 } else { 1.0 };
            if factor != 1.0 {
                let at = ui.input(|i| i.pointer.hover_pos()).map(|p| x0 + (p.x - lanes.min.x - PAD) / zoom).unwrap_or(x0);
                let nz = (zoom * factor).clamp(fit * ZOOM_OUT, ZOOM_MAX.max(fit));
                self.tl_zoom = if (nz - fit).abs() <= fit * 0.0001 { 0.0 } else { nz };
                self.tl_x0 = at - (at - x0) * zoom / nz;
            } else if shift || scroll.x != 0.0 || (scroll.y != 0.0 && rows_h <= tracks_h) {
                self.tl_x0 -= (scroll.x + if shift || rows_h <= tracks_h { scroll.y } else { 0.0 }) / zoom;
            } else if scroll.y != 0.0 {
                self.tl_y0 -= scroll.y;
            }
        }
        // the middle button drags the view around
        let (mid, md) = ui.input(|i| (i.pointer.middle_down(), i.pointer.delta()));
        if mid && (self.tl_pan || resp.hovered()) {
            self.tl_pan = true;
            self.tl_x0 -= md.x / zoom;
            self.tl_y0 -= md.y;
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            ctx.request_repaint();
        } else {
            self.tl_pan = false;
        }
        self.tl_y0 = self.tl_y0.clamp(0.0, (rows_h - tracks_h).max(0.0));
        let y0 = self.tl_y0;
        let row_top = |t: usize| tracks_top - y0 + if t == 0 { 0.0 } else { gif_h + (t - 1) as f32 * lay_h };
        let row_h = |t: usize| if t == 0 { gif_h } else { lay_h };

        // ruler
        painter.rect_filled(Rect::from_min_max(area.min, pos2(area.max.x, tracks_top)), 0.0, pal.window);
        painter.hline(area.x_range(), tracks_top, Stroke::new(1.0, pal.border));
        // a tick every so many frames, labels where they fit
        let step = [1usize, 2, 5, 10, 20, 25, 50, 100, 200, 500, 1000].into_iter().find(|s| *s as f32 * zoom >= 48.0).unwrap_or(2000);
        let minor = [1usize, 2, 5, 10, 25, 50, 100].into_iter().find(|s| *s as f32 * zoom >= 8.0).unwrap_or(step);
        let first = ((x0 - PAD / zoom).max(0.0) as usize / minor) * minor;
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
        // the in and out points: a band on the ruler
        if let Some((a, b)) = self.tl_zone() {
            let (xa, xb) = (fx(a as f32), fx(b as f32 + 1.0));
            let band = Rect::from_min_max(pos2(xa.max(lanes.min.x), tracks_top - 6.0), pos2(xb.min(area.max.x), tracks_top));
            if band.width() > 0.0 {
                painter.rect_filled(band, 0.0, pal.highlight.gamma_multiply(if self.tl_zone_loop { 0.75 } else { 0.35 }));
            }
            for (x, dir) in [(xa, 1.0f32), (xb, -1.0)] {
                if x >= lanes.min.x && x <= area.max.x {
                    painter.vline(x, (area.min.y + 2.0)..=tracks_top, Stroke::new(1.5, pal.highlight));
                    painter.hline(x..=x + 5.0 * dir, area.min.y + 2.0, Stroke::new(1.5, pal.highlight));
                }
            }
        }
        // markers: a flag on the ruler with its name
        let markers = self.stack.markers.clone();
        for (mi, m) in markers.iter().enumerate() {
            let x = fx(m.frame as f32);
            if x < lanes.min.x - 4.0 || x > area.max.x {
                continue;
            }
            let y = area.min.y + 2.0;
            let on = self.tl_marker_drag == Some(mi);
            let col = if on { pal.highlight } else { MARK };
            painter.add(egui::Shape::convex_polygon(vec![pos2(x, y), pos2(x + 8.0, y), pos2(x + 8.0, y + 9.0), pos2(x + 4.0, y + 12.0), pos2(x, y + 9.0)], col, Stroke::NONE));
            // the name, as far as the next marker allows
            let room = markers.get(mi + 1).map(|n| fx(n.frame as f32) - x - 14.0).unwrap_or(f32::MAX).min(area.max.x - x - 12.0);
            if room > 24.0 && self.tl_marker_edit.as_ref().map(|e| e.0) != Some(mi) {
                let g = ui.fonts_mut(|f| f.layout(m.label.clone(), egui::FontId::proportional(10.5), pal.fg, room));
                let g = if g.rows.len() > 1 { ui.fonts_mut(|f| f.layout_no_wrap("…".into(), egui::FontId::proportional(10.5), pal.fg)) } else { g };
                // on a backing so the frame numbers don't run through it
                painter.rect_filled(Rect::from_min_size(pos2(x + 9.0, y - 1.0), g.size() + vec2(4.0, 1.0)), 2.0, pal.window);
                painter.galley(pos2(x + 11.0, y - 1.0), g, MARK);
            }
        }

        // track headers and lanes
        let hovered_pos = ui.input(|i| i.pointer.hover_pos());
        let mut header_click: Option<u64> = None;
        let mut eye_click: Option<u64> = None;
        let mut lock_click: Option<u64> = None;
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
            let soloed = self.tl_solo == Some(id);
            // another layer is soloed: this one doesn't show in the view
            let muted = id != GIF && self.tl_solo.is_some_and(|s| s != id);
            if id != GIF {
                if soloed {
                    crate::icons::paint(&tp, "view-filter", eye.center(), 16.0, pal.highlight);
                } else {
                    crate::icons::paint(&tp, if visible { "view-visible" } else { "view-hidden" }, eye.center(), 16.0, pal.fg_muted);
                }
            }
            let locked = id != GIF && self.tl_locked(id);
            let lock = Rect::from_center_size(pos2(head.max.x - 14.0, head.center().y), vec2(20.0, 20.0));
            let head_hover = hovered_pos.is_some_and(|p| head.contains(p));
            if id != GIF && (locked || head_hover) {
                crate::icons::paint(&tp, if locked { "object-locked" } else { "object-unlocked" }, lock.center(), 16.0, if locked { pal.fg } else { pal.fg_muted });
            }
            let visible = visible && !muted;
            paint_tool_icon(&tp, icon_tool, pos2(head.min.x + 36.0, head.center().y), 16.0, if visible { pal.fg } else { pal.fg_muted });
            let text_w = head_w - 56.0 - if id != GIF { 22.0 } else { 0.0 };
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
                } else if resp.clicked() && hovered_pos.is_some_and(|p| lock.contains(p)) {
                    lock_click = Some(id);
                } else if resp.clicked() && hovered_pos.is_some_and(|p| head.contains(p)) {
                    header_click = Some(id);
                }
            } else if resp.clicked() && hovered_pos.is_some_and(|p| head.contains(p)) {
                header_click = Some(id);
            }
            // clips, in the track's own colour when one was picked
            let picked = if id == GIF { self.stack.gif_color } else { self.stack.index_of(id).and_then(|li| self.stack.layers[li].color) };
            let col = picked.map(|c| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or_else(|| kind.map(kind_colour).unwrap_or(pal.highlight));
            let spans = self.track_spans(id);
            let lp = tp.with_clip_rect(track_clip.intersect(lane));
            for (ci, &(a, b)) in spans.iter().enumerate() {
                let r = Rect::from_min_max(pos2(fx(a as f32), top + 3.0), pos2(fx(b as f32 + 1.0), top + h - 3.0));
                if r.max.x < lane.min.x || r.min.x > lane.max.x {
                    continue;
                }
                let sel = self.tl_is_selected(id, ci, a);
                let fill = if !visible { mix(pal.view, col, 0.2) } else if locked { mix(pal.view, col, 0.35) } else { mix(pal.view, col, 0.55) };
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
                if sel && !locked && self.tl_clip == Some(ci) && selected {
                    for x in [r.min.x, r.max.x] {
                        lp.rect_filled(Rect::from_center_size(pos2(x, r.center().y), vec2(3.0, r.height() - 6.0)), 1.0, pal.highlight);
                    }
                }
            }
            if let Some((ta, tb)) = self.tl_trim.filter(|_| id == GIF) {
                // frames that go when the drag ends
                for (a, b) in [(0usize, ta), (tb + 1, self.gif_edit_len())] {
                    if b > a {
                        let r = Rect::from_min_max(pos2(fx(a as f32), top + 1.0), pos2(fx(b as f32), top + h - 1.0));
                        lp.rect_filled(r, 3.0, Color32::from_black_alpha(170));
                        if r.width() > 50.0 {
                            lp.text(r.center(), egui::Align2::CENTER_CENTER, format!("−{}", b - a), egui::FontId::proportional(11.0), pal.fg);
                        }
                    }
                }
            }
            // a clip being moved: where it would go
            if let Some(d) = self.tl_drag.filter(|d| d.track == id && d.grab == TlGrab::Body && self.tl_group.is_empty()) {
                let len = (d.orig.1 - d.orig.0 + 1) as f32;
                let at = match (self.tl_free_drop, ui.input(|i| i.pointer.interact_pos())) {
                    // free room: it lands right there
                    (Some(w), _) => Some(w as f32),
                    (None, Some(p)) if self.tl_drop.is_some() => Some(d.orig.0 as f32 + (p.x - lanes.min.x - PAD) / zoom + x0 - d.at),
                    _ => None,
                };
                if let Some(a) = at {
                    let r = Rect::from_min_max(pos2(fx(a), top + 3.0), pos2(fx(a + len), top + h - 3.0));
                    lp.rect_filled(r, 3.0, mix(pal.view, pal.highlight, 0.35).gamma_multiply(0.85));
                    lp.rect_stroke(r, 3.0, Stroke::new(1.5, pal.highlight), egui::StrokeKind::Inside);
                }
                if let Some(t) = self.tl_drop {
                    let x = fx(t as f32);
                    lp.vline(x, (top - 1.0)..=(top + h + 1.0), Stroke::new(3.0, pal.highlight));
                }
            }
            // Boomerang: the part played back follows the first half, greyed
            let ge = self.gif_edit_len();
            if id == GIF && ge < n {
                let r = Rect::from_min_max(pos2(fx(ge as f32), top + 3.0), pos2(fx(n as f32), top + h - 3.0));
                if r.max.x > lane.min.x && r.min.x < lane.max.x {
                    lp.rect_filled(r, 3.0, mix(pal.view, col, 0.25));
                    self.paint_gif_thumbs(ctx, &lp, r, ge, n - 1, zoom, fx);
                    lp.rect_filled(r, 3.0, Color32::from_black_alpha(130));
                    lp.rect_stroke(r, 3.0, Stroke::new(1.0, mix(col, Color32::BLACK, 0.3)), egui::StrokeKind::Inside);
                    if r.width() > 80.0 {
                        lp.text(pos2(r.min.x.max(lane.min.x) + 8.0, r.center().y), egui::Align2::LEFT_CENTER, "Boomerang", egui::FontId::proportional(11.0), pal.fg_muted);
                    }
                }
            }
            if id == GIF && self.tl_extend > 0 {
                let r = Rect::from_min_max(pos2(fx(ge as f32), top + 3.0), pos2(fx((ge + self.tl_extend) as f32), top + h - 3.0));
                lp.rect_filled(r, 3.0, mix(pal.view, pal.highlight, 0.25));
                lp.rect_stroke(r, 3.0, Stroke::new(1.5, pal.highlight), egui::StrokeKind::Inside);
                lp.text(r.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, format!("+{} frames", self.tl_extend), egui::FontId::proportional(11.0), pal.fg);
            }
            // keyframes
            if let Some(li) = (id != GIF).then(|| self.stack.index_of(id)).flatten() {
                for (k, key) in self.stack.layers[li].timing.keys.iter().enumerate() {
                    let c = pos2(fx(key.frame as f32), top + h / 2.0);
                    let on = (selected && self.tl_key == Some(k)) || self.tl_keys.contains(&(id, key.frame));
                    let (r, fill, stroke) = (if on { 6.5 } else { 5.0 }, if on { pal.highlight } else { Color32::WHITE }, Stroke::new(1.0, Color32::from_black_alpha(160)));
                    // a hold key is square: nothing moves until the next one
                    if key.ease == Ease::Hold {
                        lp.rect(Rect::from_center_size(c, vec2(r * 1.5, r * 1.5)), 1.0, fill, stroke, egui::StrokeKind::Middle);
                    } else {
                        diamond(&lp, c, r, fill, stroke);
                    }
                }
            }
        }
        painter.vline(lanes.min.x, tracks_top..=area.max.y, Stroke::new(1.0, pal.border));
        // markers carry on faintly down the tracks
        for m in &self.stack.markers {
            let x = fx(m.frame as f32);
            if x >= lanes.min.x && x <= area.max.x {
                tp.vline(x, tracks_top..=area.max.y, Stroke::new(1.0, MARK.gamma_multiply(0.45)));
            }
        }
        // what a drag snapped to
        if let Some(f) = self.tl_snap_line.filter(|_| self.tl_drag.is_some()) {
            let x = fx(f as f32);
            if x >= lanes.min.x && x <= area.max.x {
                painter.vline(x, area.min.y..=area.max.y, Stroke::new(1.0, pal.highlight));
            }
        }
        // a rubber band selection
        if let (Some(o), Some(p)) = (self.tl_box, hovered_pos.or(ui.input(|i| i.pointer.interact_pos()))) {
            let r = Rect::from_two_pos(o, p).intersect(Rect::from_min_max(pos2(lanes.min.x, tracks_top), area.max));
            painter.rect(r, 0.0, pal.highlight.gamma_multiply(0.15), Stroke::new(1.0, pal.highlight), egui::StrokeKind::Inside);
        }

        // horizontal scroll bar when zoomed in: drag it, or click to jump
        let bar = Rect::from_min_max(pos2(lanes.min.x + 2.0, area.max.y - BAR_H + 1.0), pos2(area.max.x - 2.0, area.max.y - 1.0));
        let scrolls = visible < span - 0.5;
        let thumb = Rect::from_min_size(pos2(bar.min.x + bar.width() * x0 / span, bar.min.y), vec2((bar.width() * visible / span).max(20.0).min(bar.width()), bar.height()));
        if scrolls {
            let over = hovered_pos.is_some_and(|p| bar.expand(2.0).contains(p)) || self.tl_bar_drag.is_some();
            painter.rect_filled(bar, 3.0, mix(pal.view, pal.fg, 0.08));
            painter.rect_filled(thumb, 3.0, mix(pal.view, if over { pal.highlight } else { pal.fg }, if over { 0.6 } else { 0.35 }));
        }

        // playhead, at the start of its frame (on the frame's tick)
        let px = fx(self.cur as f32);
        if px >= lanes.min.x && px <= area.max.x {
            painter.vline(px, area.min.y..=area.max.y, Stroke::new(1.5, pal.negative));
            painter.add(egui::Shape::convex_polygon(vec![pos2(px - 5.0, area.min.y), pos2(px + 5.0, area.min.y), pos2(px, area.min.y + 7.0)], pal.negative, Stroke::NONE));
        }

        // ---- pointer
        let frame_at = |x: f32| x0 + (x - lanes.min.x - PAD) / zoom;
        let track_at = |y: f32| -> Option<usize> {
            (0..tracks.len()).find(|&t| y >= row_top(t) && y < row_top(t) + row_h(t)).filter(|_| y >= tracks_top)
        };
        if let Some(id) = eye_click {
            // Alt+click shows this layer on its own, as in image editors
            if ui.input(|i| i.modifiers.alt) {
                self.tl_solo = if self.tl_solo == Some(id) { None } else { Some(id) };
            } else if let Some(li) = self.stack.index_of(id) {
                self.stack.layers[li].visible = !self.stack.layers[li].visible;
                self.modified = true;
            }
        }
        if let Some(id) = lock_click {
            if let Some(li) = self.stack.index_of(id) {
                self.snapshot(&format!("tl-lock{now}"), now);
                self.stack.layers[li].locked = !self.stack.layers[li].locked;
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
        // a track's bottom edge, in the name column, sets its height
        let row_edge = |p: Pos2| -> Option<usize> {
            (p.x < lanes.min.x - 6.0 && p.y > tracks_top).then(|| (0..tracks.len()).find(|&t| (row_top(t) + row_h(t) - p.y).abs() < 3.0)).flatten()
        };
        if hovered_pos.and_then(row_edge).is_some() || self.tl_row_drag.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        // the line between names and lanes resizes the name column
        // (on the names' side only, so a clip's start at frame 1 stays easy to grab)
        let grip = Rect::from_min_max(pos2(lanes.min.x - 6.0, tracks_top), pos2(lanes.min.x - 1.0, area.max.y));
        if hovered_pos.is_some_and(|p| grip.contains(p)) || self.tl_head_drag {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                if scrolls && bar.expand(2.0).contains(p) {
                    // grab the thumb where it was pressed, or jump its middle there
                    let at = (p.x - bar.min.x) / bar.width() * span;
                    self.tl_bar_drag = Some(if thumb.contains(p) { at - x0 } else { visible / 2.0 });
                } else if grip.contains(p) {
                    self.tl_head_drag = true;
                } else if let Some(t) = row_edge(p) {
                    self.tl_row_drag = Some(t);
                } else if p.x < lanes.min.x && p.y > tracks_top {
                    // dragging a layer's name moves it up or down the stack
                    if let Some(t) = track_at(p.y).filter(|t| *t > 0) {
                        self.tl_reorder = Some(t);
                    }
                }
            }
        }
        if let Some(off) = self.tl_bar_drag {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                self.tl_x0 = (p.x - bar.min.x) / bar.width() * span - off;
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_bar_drag = None;
            }
        }
        if let Some(t) = self.tl_row_drag {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                let h = p.y - row_top(t);
                if t == 0 {
                    self.tl_gif_h = h.clamp(28.0, 160.0);
                } else {
                    self.tl_row_h = h.clamp(20.0, 80.0);
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_row_drag = None;
                theme::set_setting("tl_gif_h", &format!("{:.0}", self.tl_gif_h));
                theme::set_setting("tl_row_h", &format!("{:.0}", self.tl_row_h));
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
            let slot = (0..=layers).find(|&k| k == layers || y < row_top(k + 1) + lay_h / 2.0).unwrap_or(layers);
            let ly = if slot == layers { row_top(layers) + lay_h } else { row_top(slot + 1) };
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
        // a marker on the ruler under `x`
        let marker_at = |me: &Self, x: f32| me.stack.markers.iter().position(|m| (fx(m.frame as f32) + 4.0 - x).abs() < 7.0);
        if resp.double_clicked() {
            if let Some(p) = hovered_pos.filter(|p| p.y < tracks_top && p.x >= lanes.min.x) {
                if let Some(mi) = marker_at(self, p.x) {
                    self.tl_marker_edit = Some((mi, self.stack.markers[mi].label.clone()));
                }
            }
        }
        let press = resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked();
        if press && self.tl_drag.is_none() && !self.tl_head_drag && self.tl_bar_drag.is_none() && self.tl_row_drag.is_none() && self.tl_marker_drag.is_none() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(hovered_pos).filter(|p| lanes.contains(*p) && !(scrolls && bar.expand(2.0).contains(*p))) {
                let f = frame_at(p.x);
                // clicks pick the nearest frame start, where the playhead sits
                let fi = (f.round().max(0.0) as usize).min(n - 1);
                if p.y < tracks_top {
                    if let Some(mi) = marker_at(self, p.x) {
                        // a marker: the playhead goes there, and it can be dragged
                        self.cur = self.stack.markers[mi].frame.min(n - 1);
                        self.playing = false;
                        if resp.drag_started() {
                            self.snapshot(&format!("tl-marker{now}"), now);
                            self.tl_marker_drag = Some(mi);
                        }
                    } else {
                        self.tl_drag = Some(TlDrag { track: GIF, clip: 0, grab: TlGrab::Playhead, at: f, orig: (fi, fi) });
                        self.cur = fi;
                        self.playing = false;
                    }
                } else if let Some(t) = track_at(p.y) {
                    let id = tracks[t];
                    let spans = self.track_spans(id);
                    let multi = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                    // Alt+click takes things out of the selection
                    let remove = ui.input(|i| i.modifiers.alt);
                    let locked = self.tl_locked(id);
                    let cy = row_top(t) + row_h(t) / 2.0;
                    // keyframes first, they sit on top
                    let key = (id != GIF).then(|| self.stack.index_of(id)).flatten().and_then(|li| {
                        self.stack.layers[li].timing.keys.iter().position(|k| (fx(k.frame as f32) - p.x).abs() < 7.0 && (cy - p.y).abs() < 8.0)
                    });
                    let hit = spans.iter().position(|&(a, b)| p.x >= fx(a as f32) - EDGE && p.x <= fx(b as f32 + 1.0) + EDGE);
                    // what was selected, for Shift+click and moving several at once
                    let before = self.tl_selected();
                    let prim = self.tl_track.zip(self.tl_clip).and_then(|(t, c)| self.track_spans(t).get(c).map(|s| (t, s.0)));
                    let was_sel = hit.is_some_and(|ci| self.tl_is_selected(id, ci, spans[ci].0));
                    let key_frame = key.and_then(|k| self.stack.index_of(id).and_then(|li| self.stack.layers[li].timing.keys.get(k)).map(|k| k.frame));
                    let key_was_sel = key.zip(key_frame).is_some_and(|(k, f)| self.tl_key_is_selected(id, k, f));
                    if (multi || remove) && resp.clicked() && (key.is_some() || hit.is_some()) {
                        if let (Some(k), Some(f)) = (key, key_frame) {
                            if remove {
                                // Alt+click: out of the selection
                                if self.tl_track == Some(id) && self.tl_key == Some(k) {
                                    self.tl_key = None;
                                    self.tl_key_dropped = Some((id, f));
                                }
                                self.tl_keys.retain(|x| *x != (id, f));
                            } else if !key_was_sel {
                                // Ctrl or Shift+click: into it
                                self.tl_keys.push((id, f));
                            }
                        } else if let Some(ci) = hit {
                            let a = spans[ci].0;
                            if remove {
                                if prim == Some((id, a)) {
                                    self.tl_clip = None;
                                }
                                self.tl_sel.retain(|s| *s != (id, a));
                            } else if prim != Some((id, a)) && !self.tl_sel.contains(&(id, a)) {
                                // the new one becomes the main clip; the old one stays selected
                                self.tl_demote_key();
                                self.tl_sel.extend(prim);
                                self.tl_select_track(id);
                                self.tl_clip = Some(ci);
                                self.tl_sel.retain(|s| *s != (id, a));
                            }
                        }
                        ctx.request_repaint();
                        return;
                    }
                    self.tl_select_track(id);
                    if resp.clicked() {
                        self.tl_sel.clear();
                        self.tl_keys.clear();
                    } else if was_sel || key_was_sel {
                        // dragging one of several selected things moves them all
                        let a = hit.map(|ci| spans[ci].0).unwrap_or(0);
                        if was_sel && key.is_none() {
                            self.tl_sel = before.iter().filter(|c| (c.0, c.1) != (id, a)).map(|c| (c.0, c.1)).collect();
                        }
                    } else {
                        self.tl_sel.clear();
                        self.tl_keys.clear();
                    }
                    if let Some(k) = key {
                        let kf = self.stack.layers[self.stack.index_of(id).unwrap()].timing.keys[k].frame;
                        // a selected keyframe dragged takes the other selected ones along
                        let others: Vec<(u64, usize)> = if key_was_sel && resp.drag_started() { self.tl_selected_keys().into_iter().filter(|x| *x != (id, kf) && !self.tl_locked(x.0)).collect() } else { Vec::new() };
                        self.tl_key = Some(k);
                        self.tl_clip = None;
                        self.tl_keys.retain(|x| *x != (id, kf));
                        self.cur = kf.min(n - 1);
                        self.playing = false;
                        if !locked {
                            if resp.drag_started() {
                                self.snapshot(&format!("tl-drag{now}"), now);
                            }
                            self.tl_key_group = others.clone();
                            if !others.is_empty() {
                                self.tl_key_group.push((id, kf));
                            }
                            self.tl_key_moved = 0;
                            self.tl_drag = Some(TlDrag { track: id, clip: 0, grab: TlGrab::Key(k), at: f, orig: (kf, kf) });
                        }
                    } else if let Some(ci) = hit {
                        let (a, b) = spans[ci];
                        self.tl_clip = Some(ci);
                        self.tl_key = None;
                        if locked {
                            if resp.drag_started() {
                                self.info("The layer is locked");
                            }
                        } else if self.tl_razor {
                            if resp.clicked() && fi > a {
                                self.tl_split_at(id, fi, now);
                            }
                        } else {
                            let grab = if (fx(a as f32) - p.x).abs() < EDGE || (a as f32 <= x0 && p.x - lanes.min.x < PAD + EDGE) {
                                TlGrab::Start
                            } else if (fx(b as f32 + 1.0) - p.x).abs() < EDGE {
                                TlGrab::End
                            } else {
                                TlGrab::Body
                            };
                            let gif_move = id == GIF && grab == TlGrab::Body;
                            let _ = gif_move;
                            if resp.drag_started() {
                                if !gif_move {
                                    self.snapshot(&format!("tl-drag{now}"), now);
                                }
                                if id != GIF {
                                    if let Some(li) = self.stack.index_of(id) {
                                        self.stack.ensure_clips(li, &self.plan);
                                    }
                                }
                                // the others selected move along (layer clips, not locked ones)
                                self.tl_group.clear();
                                if grab == TlGrab::Body && id != GIF && !self.tl_sel.is_empty() {
                                    for (t, ga, gb) in self.tl_selected() {
                                        if t == GIF || self.tl_locked(t) {
                                            continue;
                                        }
                                        if let Some(li) = self.stack.index_of(t) {
                                            self.stack.ensure_clips(li, &self.plan);
                                        }
                                        if let Some(j) = self.track_spans(t).iter().position(|s| s.0 == ga) {
                                            self.tl_group.push((t, j, ga, gb));
                                        }
                                    }
                                }
                                // a GIF edge works from the frames as they were, so
                                // going back and forth over a gap doesn't pile up
                                self.tl_gif_d0 = (id == GIF && grab != TlGrab::Body).then(|| self.gif_display().map(|x| x.2)).flatten();
                                self.tl_free_drop = None;
                                self.tl_drop = None;
                                self.tl_drag = Some(TlDrag { track: id, clip: ci, grab, at: f, orig: (a, b) });
                            }
                        }
                    } else if resp.drag_started() && !self.tl_razor {
                        // dragging on an empty part draws a selection box
                        self.tl_box = Some(p);
                        if !multi && !remove {
                            self.tl_clip = None;
                            self.tl_key = None;
                            self.tl_sel.clear();
                            self.tl_keys.clear();
                        }
                    } else {
                        // empty lane: move the playhead there
                        self.tl_clip = None;
                        self.tl_key = None;
                        self.tl_sel.clear();
                        self.tl_keys.clear();
                        self.cur = fi;
                        self.playing = false;
                    }
                } else if p.y > tracks_top && resp.drag_started() && !self.tl_razor {
                    // below the rows: a selection box too (rows full of clips have no room)
                    self.tl_box = Some(p);
                    if !ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.alt) {
                        self.tl_clip = None;
                        self.tl_key = None;
                        self.tl_sel.clear();
                        self.tl_keys.clear();
                    }
                }
            }
        }
        if let (Some(d), Some(p)) = (self.tl_drag, ui.input(|i| i.pointer.interact_pos())) {
            let f = frame_at(p.x);
            let raw = f - d.at;
            let mut delta = raw.round() as i64;
            // snapping: the moving edge (or key) to the nearest thing within a few pixels
            self.tl_snap_line = None;
            let free = ui.input(|i| i.modifiers.shift);
            if self.tl_snap && !free && d.grab != TlGrab::Playhead {
                let edges: Vec<usize> = match d.grab {
                    TlGrab::Key(_) | TlGrab::Start => vec![d.orig.0],
                    TlGrab::End => vec![d.orig.1 + 1],
                    _ => vec![d.orig.0, d.orig.1 + 1],
                };
                // not to itself, where it was or where it is now
                let mut skip = vec![d.orig.0, d.orig.1 + 1];
                match d.grab {
                    TlGrab::Key(k) => skip.extend(self.stack.index_of(d.track).and_then(|li| self.stack.layers[li].timing.keys.get(k).map(|k| k.frame))),
                    _ => {
                        if let Some(&(a, b)) = self.track_spans(d.track).get(d.clip) {
                            skip.extend([a, b + 1]);
                        }
                        for &(t, j, a, b) in &self.tl_group {
                            skip.extend([a, b + 1]);
                            if let Some(&(x, y)) = self.track_spans(t).get(j) {
                                skip.extend([x, y + 1]);
                            }
                        }
                    }
                }
                let thr = (8.0 / zoom).max(0.5);
                let best = edges
                    .iter()
                    .filter_map(|&e| {
                        let pos = e as f32 + raw;
                        self.tl_snap_to(pos, thr, &skip).map(|t| ((t as f32 - pos).abs(), t as i64 - e as i64, t))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, dl, t)) = best {
                    delta = dl;
                    self.tl_snap_line = Some(t);
                }
            }
            let clamp = |v: i64, lo: usize, hi: usize| v.clamp(lo as i64, hi as i64) as usize;
            match d.grab {
                TlGrab::Playhead => {
                    self.cur = clamp(f.round() as i64, 0, n - 1);
                    self.playing = false;
                }
                TlGrab::Key(_) if !self.tl_key_group.is_empty() => {
                    // several keyframes: the same distance each, none past the ends
                    let group = self.tl_key_group.clone();
                    let lo = -(group.iter().map(|g| g.1).min().unwrap_or(0) as i64);
                    let hi = n as i64 - 1 - group.iter().map(|g| g.1).max().unwrap_or(0) as i64;
                    let dl = delta.clamp(lo, hi);
                    if dl != self.tl_key_moved {
                        let at: Vec<(u64, usize)> = group.iter().map(|&(t, f)| (t, (f as i64 + self.tl_key_moved) as usize)).collect();
                        if self.tl_move_keys(&at, dl - self.tl_key_moved) {
                            self.tl_key_moved = dl;
                        }
                    }
                    self.cur = (d.orig.0 as i64 + self.tl_key_moved).clamp(0, n as i64 - 1) as usize;
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
                TlGrab::Body if !self.tl_group.is_empty() => {
                    // several clips: the same distance each, none past its neighbours
                    let group = self.tl_group.clone();
                    let (mut lo, mut hi) = (i64::MIN, i64::MAX);
                    for &(t, _, a, b) in &group {
                        lo = lo.max(-(a as i64));
                        hi = hi.min(n as i64 - 1 - b as i64);
                        for (j, &(x, y)) in self.track_spans(t).iter().enumerate() {
                            if group.iter().any(|g| g.0 == t && g.1 == j) {
                                continue;
                            }
                            if y < a {
                                lo = lo.max(y as i64 + 1 - a as i64);
                            }
                            if x > b {
                                hi = hi.min(x as i64 - 1 - b as i64);
                            }
                        }
                    }
                    let dl = if lo <= hi { delta.clamp(lo, hi) } else { 0 };
                    for &(t, j, a, b) in &group {
                        if let Some(li) = self.stack.index_of(t) {
                            if let Some(cl) = &mut self.stack.layers[li].timing.clips {
                                cl.sort_by_key(|c| c.start);
                                if let Some(c) = cl.get_mut(j) {
                                    let (na, nb) = ((a as i64 + dl) as usize, (b as i64 + dl) as usize);
                                    *c = Clip { start: na, end: if nb + 1 >= n { None } else { Some(nb) } };
                                }
                            }
                        }
                    }
                    self.modified = true;
                }
                TlGrab::Body => {
                    // into free room on the row it goes straight there; over
                    // other clips it goes in between them and they make room
                    let spans = self.track_spans(d.track);
                    let ge = if d.track == GIF { self.gif_edit_len() } else { n };
                    let len = d.orig.1 - d.orig.0;
                    let here = spans.get(d.clip).copied().unwrap_or(d.orig);
                    let others: Vec<(usize, usize)> = spans.iter().enumerate().filter(|(j, _)| *j != d.clip).map(|(_, s)| *s).collect();
                    let want = clamp(d.orig.0 as i64 + delta, 0, ge.saturating_sub(len + 1));
                    self.tl_free_drop = None;
                    self.tl_drop = None;
                    if want == here.0 {
                        // where it is: nothing to do
                    } else if free_slot(&others, len + 1, want, ge, false) == Some(want) {
                        self.tl_free_drop = Some(want);
                    } else {
                        // the edge nearest its leading end
                        let lead = if delta < 0 { want } else { want + len + 1 } as f32;
                        let best = others.iter().flat_map(|&(x, y)| [x, y + 1]).filter(|&t| t <= here.0 || t > here.1).min_by(|x, y| (*x as f32 - lead).abs().total_cmp(&(*y as f32 - lead).abs()));
                        self.tl_drop = best.filter(|&t| t != here.0 && t != here.1 + 1);
                        self.tl_snap_line = None;
                    }
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                grab => {
                    let spans = self.track_spans(d.track);
                    // neighbours bound the move so clips never overlap
                    let lo = spans.iter().filter(|s| s.1 < d.orig.0).map(|s| s.1 + 1).max().unwrap_or(0);
                    // the GIF's editable part (with Boomerang, the first half)
                    let ge = if d.track == GIF { self.gif_edit_len() } else { n };
                    let hi = spans.iter().filter(|s| s.0 > d.orig.1).map(|s| s.0 - 1).min().unwrap_or(ge - 1);
                    let len = d.orig.1 - d.orig.0;
                    // the GIF's last clip can be pulled past the end to make it longer
                    // (with a gap after it too: the gap fills first)
                    let past = d.track == GIF && grab == TlGrab::End && !spans.iter().any(|s| s.0 > d.orig.1);
                    if past {
                        self.tl_extend = (d.orig.1 as i64 + delta - (ge as i64 - 1)).clamp(0, (ge as i64 * 4).max(250)) as usize;
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
                        // trimming the GIF's outer ends cuts the animation shorter
                        // (when it plays forwards); inner edges skip frames
                        let outer = d.track == GIF && ((grab == TlGrab::Start && d.orig.0 == 0) || (grab == TlGrab::End && d.orig.1 + 1 == ge));
                        if outer {
                            self.tl_trim = (na > 0 || nb + 1 < ge).then_some((na, nb));
                        } else if d.track == GIF {
                            // trimming leaves a gap; pulling an edge into a gap
                            // fills it the way "When Made Longer" says
                            if let (Some(d0), Some((s, e, _))) = (self.tl_gif_d0.clone(), self.gif_display()) {
                                if d0.len() == e + 1 - s && d.orig.1 < d0.len() {
                                    let nd = gif_edge_fill(&d0, d.orig, (na, nb), self.tl_extend_mode);
                                    self.gif_write(s, e, nd);
                                }
                            }
                        } else if let Some(li) = self.stack.index_of(d.track) {
                            if let Some(cl) = &mut self.stack.layers[li].timing.clips {
                                cl.sort_by_key(|c| c.start);
                                if let Some(c) = cl.get_mut(d.clip) {
                                    *c = Clip { start: na, end: if nb + 1 >= n { None } else { Some(nb) } };
                                }
                                // past another clip, its place in the row changes: follow it
                                cl.sort_by_key(|c| c.start);
                                if let Some(j) = cl.iter().position(|c| c.start == na) {
                                    self.tl_drag = Some(TlDrag { clip: j, ..d });
                                    self.tl_clip = Some(j);
                                }
                            }
                            self.modified = true;
                        }
                    }
                }
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_drag = None;
                self.tl_snap_line = None;
                self.tl_key_group.clear();
                // the selection follows the clips that moved
                if !self.tl_group.is_empty() {
                    let group = std::mem::take(&mut self.tl_group);
                    self.tl_sel = group.iter().filter(|g| !(g.0 == d.track && g.1 == d.clip)).filter_map(|&(t, j, _, _)| self.track_spans(t).get(j).map(|s| (t, s.0))).collect();
                }
                self.tl_gif_d0 = None;
                if let Some(t) = self.tl_drop.take() {
                    if d.track == GIF {
                        self.tl_gif_move(d.orig.0, d.orig.1, t, now);
                    } else {
                        self.tl_layer_reorder(d.track, d.clip, t);
                    }
                }
                if let Some(w) = self.tl_free_drop.take() {
                    if d.track == GIF {
                        self.gif_place(d.orig.0, d.orig.1, w, now);
                    } else {
                        self.tl_layer_place(d.track, d.clip, w);
                    }
                }
                if let Some((a, b)) = self.tl_trim.take() {
                    self.tl_cut(a, b);
                }
                if self.tl_extend > 0 {
                    let k = std::mem::take(&mut self.tl_extend);
                    self.tl_lengthen(k, now);
                }
            }
        }
        // a marker being dragged along the ruler
        if let Some(mi) = self.tl_marker_drag {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                let fi = frame_at(p.x).round().clamp(0.0, n as f32 - 1.0) as usize;
                if let Some(m) = self.stack.markers.get_mut(mi) {
                    if m.frame != fi {
                        m.frame = fi;
                        self.modified = true;
                    }
                }
                self.cur = fi;
            }
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_marker_drag = None;
                self.stack.markers.sort_by_key(|m| m.frame);
                self.stack.markers.dedup_by_key(|m| m.frame);
            }
        }
        // the selection box: clips it touches are selected when it's let go
        if let Some(o) = self.tl_box {
            if !ui.input(|i| i.pointer.primary_down()) {
                self.tl_box = None;
                if let Some(p) = hovered_pos.or(ui.input(|i| i.pointer.interact_pos())) {
                    let r = Rect::from_two_pos(o, p);
                    let remove = ui.input(|i| i.modifiers.alt);
                    let mut found: Vec<(u64, usize, usize)> = Vec::new();
                    let mut keys: Vec<(u64, usize)> = Vec::new();
                    for (t, &id) in tracks.iter().enumerate() {
                        let (top, h) = (row_top(t), row_h(t));
                        if top + h < r.min.y || top > r.max.y {
                            continue;
                        }
                        // keyframes whose diamond the box takes in
                        if let Some(li) = (id != GIF).then(|| self.stack.index_of(id)).flatten() {
                            let cy = top + h / 2.0;
                            if cy >= r.min.y - 6.0 && cy <= r.max.y + 6.0 {
                                for k in &self.stack.layers[li].timing.keys {
                                    let x = fx(k.frame as f32);
                                    if x >= r.min.x - 6.0 && x <= r.max.x + 6.0 {
                                        keys.push((id, k.frame));
                                    }
                                }
                            }
                        }
                        // clips only when the box is more than a key's width
                        // over them, so boxing keys doesn't take their clips
                        let keys_only = !keys.is_empty() && keys.iter().any(|k| k.0 == id);
                        for (ci, &(a, b)) in self.track_spans(id).iter().enumerate() {
                            if fx(a as f32) <= r.max.x && fx(b as f32 + 1.0) >= r.min.x && !keys_only {
                                found.push((id, ci, a));
                            }
                        }
                    }
                    if remove {
                        // Alt: the box takes things out of the selection
                        for (id, ci, a) in &found {
                            if self.tl_track == Some(*id) && self.tl_clip == Some(*ci) {
                                self.tl_clip = None;
                            }
                            self.tl_sel.retain(|s| *s != (*id, *a));
                        }
                        let main = self.tl_selected_keys().first().copied().filter(|_| self.tl_key.is_some());
                        if main.is_some_and(|m| keys.contains(&m)) {
                            self.tl_key = None;
                        }
                        self.tl_keys.retain(|k| !keys.contains(k));
                        found.clear();
                        keys.clear();
                    }
                    for k in keys {
                        if !self.tl_keys.contains(&k) {
                            self.tl_keys.push(k);
                        }
                    }
                    if let Some(&(id, ci, _)) = found.first() {
                        if self.tl_clip.is_none() {
                            self.tl_select_track(id);
                            self.tl_clip = Some(ci);
                            self.tl_key = None;
                        }
                        let prim = self.tl_track.zip(self.tl_clip).and_then(|(t, c)| self.track_spans(t).get(c).map(|s| (t, s.0)));
                        for (id, _, a) in found {
                            if Some((id, a)) != prim && !self.tl_sel.contains(&(id, a)) {
                                self.tl_sel.push((id, a));
                            }
                        }
                    }
                }
            }
        }
        if self.tl_drag.is_some() || self.tl_reorder.is_some() || self.tl_head_drag || self.tl_bar_drag.is_some() || self.tl_box.is_some() || self.tl_marker_drag.is_some() || self.tl_row_drag.is_some() {
            ctx.request_repaint();
        }

        // keys, while the pointer is over the timeline
        if resp.hovered() && !ctx.egui_wants_keyboard_input() {
            use crate::keys::Action as A;
            let km = &self.keymap;
            let (s, del, k, x, dup, nl, nr) = ui.input(|i| (km.pressed(i, A::TlSplit), km.pressed(i, A::TlDelete), km.pressed(i, A::TlKey), km.pressed(i, A::TlRazor), km.pressed(i, A::TlDuplicate), km.pressed(i, A::TlNudgeLeft), km.pressed(i, A::TlNudgeRight)));
            // Ctrl+C and Ctrl+V reach us as clipboard events
            let (copy, paste, cut) = ui.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::Copy)), i.events.iter().any(|e| matches!(e, egui::Event::Paste(t) if t == BOARD_TAG)), i.events.iter().any(|e| matches!(e, egui::Event::Cut))));
            if cut {
                self.tl_cut_clips(ctx, now);
            }
            if s {
                self.tl_split(now);
            }
            if del {
                self.tl_delete_selected(now);
            }
            if k {
                self.tl_add_key(now);
            }
            if x {
                self.tl_razor = !self.tl_razor;
            }
            if dup {
                self.tl_duplicate(now);
            }
            if nl || nr {
                self.tl_nudge(if nl { -1 } else { 1 }, now);
            }
            if copy {
                self.tl_copy(ctx);
            }
            if paste {
                self.tl_paste(now);
            }
        }
        // right-click picks what's under the pointer first, as a left click would
        if resp.secondary_clicked() {
            self.tl_menu_head = None;
            self.tl_menu_marker = None;
            if let Some(p) = hovered_pos.filter(|p| p.y < tracks_top && p.x >= lanes.min.x) {
                // the ruler: a marker's menu, or the ruler's own
                self.tl_menu_marker = Some(marker_at(self, p.x).unwrap_or_else(|| {
                    self.cur = (frame_at(p.x).floor().max(0.0) as usize).min(n - 1);
                    self.playing = false;
                    usize::MAX
                }));
            } else if let Some(p) = hovered_pos.filter(|p| p.y > tracks_top && p.x >= area.min.x) {
                if let Some(t) = track_at(p.y) {
                    let id = tracks[t];
                    self.tl_select_track(id);
                    if p.x < lanes.min.x {
                        // the name: the layer's own menu
                        self.tl_menu_head = Some(id);
                    } else {
                        let ci = self.track_spans(id).iter().position(|&(a, b)| p.x >= fx(a as f32) && p.x <= fx(b as f32 + 1.0));
                        // right-clicking outside the selection selects just that clip
                        if !ci.is_some_and(|c| self.tl_is_selected(id, c, self.track_spans(id)[c].0)) {
                            self.tl_sel.clear();
                        }
                        self.tl_clip = ci;
                    }
                }
            }
        }
        resp.context_menu(|ui| {
            ui.set_min_width(240.0);
            let km = self.keymap.clone();
            use crate::keys::Action as A;
            // the ruler
            if let Some(mi) = self.tl_menu_marker {
                if mi < self.stack.markers.len() {
                    if menu_item(ui, "edit-rename", "&Rename Marker", "", true) {
                        self.tl_marker_edit = Some((mi, self.stack.markers[mi].label.clone()));
                    }
                    if menu_item(ui, "bookmark-remove", "&Delete Marker", "", true) {
                        self.snapshot(&format!("tl-marker{now}"), now);
                        self.stack.markers.remove(mi);
                        self.modified = true;
                    }
                    ui.separator();
                }
                if menu_item(ui, "bookmark-new", "Add &Marker", &km.text(A::TlMarker), true) {
                    self.tl_add_marker(now);
                }
                if menu_item(ui, "zone-in", "Set &In Point", &km.text(A::TlIn), true) {
                    self.tl_set_in(Some(self.cur));
                }
                if menu_item(ui, "zone-out", "Set &Out Point", &km.text(A::TlOut), true) {
                    self.tl_set_out(Some(self.cur));
                }
                if menu_item(ui, "edit-clear-all", "Clea&r In and Out", "", self.tl_zone().is_some()) {
                    self.tl_in = None;
                    self.tl_out = None;
                }
                if menu_item(ui, "edit-clear-all", "Clear &All Markers", "", !self.stack.markers.is_empty()) {
                    self.snapshot(&format!("tl-marker{now}"), now);
                    self.stack.markers.clear();
                    self.modified = true;
                }
                return;
            }
            let layer = self.tl_track.filter(|t| *t != GIF).and_then(|t| self.stack.index_of(t));
            if self.tl_menu_head == Some(GIF) {
                if menu_item(ui, "split", "&Split at Playhead", &km.text(A::TlSplit), true) {
                    self.tl_split(now);
                }
                if menu_item(ui, "list-add", "Make &Longer by 1 Second", "", idle) {
                    let d = self.plan.last().map(|f| effective_delay(f.delay)).unwrap_or(10).max(2) as usize;
                    self.tl_lengthen((100 / d).max(1), now);
                }
                self.tl_extend_menu(ui);
                self.tl_colour_menu(ui, GIF, now);
                return;
            }
            if let (Some(_), Some(li)) = (self.tl_menu_head, layer) {
                let (visible, locked, id) = (self.stack.layers[li].visible, self.stack.layers[li].locked, self.stack.layers[li].id);
                let mut act = None;
                if menu_item(ui, "edit-rename", "&Rename", "", true) {
                    act = Some(LayerAct::Rename(li));
                }
                if menu_item(ui, "edit-duplicate", "&Duplicate Layer", "", true) {
                    act = Some(LayerAct::Duplicate(li));
                }
                if menu_item(ui, if visible { "view-hidden" } else { "view-visible" }, if visible { "&Hide Layer" } else { "&Show Layer" }, "", true) {
                    act = Some(LayerAct::Toggle(li));
                }
                let solo = self.tl_solo == Some(id);
                if menu_item(ui, "view-filter", if solo { "Stop &Solo" } else { "&Solo Layer" }, "Alt+click", true) {
                    self.tl_solo = if solo { None } else { Some(id) };
                }
                if menu_item(ui, if locked { "object-unlocked" } else { "object-locked" }, if locked { "Unlock La&yer" } else { "Lock La&yer" }, "", true) {
                    self.snapshot(&format!("tl-lock{now}"), now);
                    self.stack.layers[li].locked = !locked;
                    self.modified = true;
                }
                self.tl_colour_menu(ui, id, now);
                ui.separator();
                if menu_item(ui, "edit-delete", "D&elete Layer", "", idle) {
                    act = Some(LayerAct::Delete(li));
                }
                if let Some(a) = act {
                    self.apply_layer_act(ctx, a, now);
                }
                return;
            }
            let locked = self.tl_track.is_some_and(|t| self.tl_locked(t));
            let has = self.tl_track.is_some_and(|t| t != GIF) && !locked;
            let n_sel = self.tl_selected().len();
            if menu_item(ui, "split", "&Split at Playhead", &km.text(A::TlSplit), self.tl_track.is_some() && !locked) {
                self.tl_split(now);
            }
            if menu_item(ui, "merge", "&Join with Next Clip", "", self.tl_can_join() && !locked) {
                self.tl_join(now);
            }
            let gap_here = has && !self.tl_track.map(|t| self.track_spans(t)).unwrap_or_default().iter().any(|&(a, b)| self.cur >= a && self.cur <= b);
            if menu_item(ui, "list-add", "&Add Clip at Playhead", "", gap_here) {
                self.tl_add_clip(now);
            }
            if menu_item(ui, "edit-delete", if n_sel > 1 { "&Delete Clips" } else { "&Delete Clip" }, &km.text(A::TlDelete), (self.tl_clip.is_some() || self.tl_key.is_some() || !self.tl_keys.is_empty() || !self.tl_sel.is_empty()) && !locked) {
                self.tl_delete_selected(now);
            }
            ui.separator();
            let any_clips = !self.tl_selected().is_empty();
            if menu_item(ui, "edit-cut", "Cu&t", "Ctrl+X", (any_clips || self.tl_key.is_some()) && !locked) {
                self.tl_cut_clips(ctx, now);
            }
            if menu_item(ui, "edit-copy", "&Copy", "Ctrl+C", any_clips || self.tl_key.is_some()) {
                self.tl_copy(ctx);
            }
            if menu_item(ui, "edit-paste", "&Paste", "Ctrl+V", has && self.tl_board.is_some()) {
                self.tl_paste(now);
            }
            if menu_item(ui, "edit-duplicate", "D&uplicate", &km.text(A::TlDuplicate), any_clips && !locked) {
                self.tl_duplicate(now);
            }
            ui.separator();
            if menu_item(ui, "keyframe-add", "Add &Keyframe", &km.text(A::TlKey), has) {
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
            if menu_item(ui, "view-refresh", "Show on E&very Frame", "", has) {
                if let Some(li) = self.tl_track.and_then(|t| self.stack.index_of(t)) {
                    self.snapshot(&format!("tl-full{now}"), now);
                    self.stack.ensure_clips(li, &self.plan);
                    self.stack.layers[li].timing.clips = Some(vec![Clip::all()]);
                    self.modified = true;
                }
            }
            ui.separator();
            if menu_item(ui, "zone-in", "Set &In Point", &km.text(A::TlIn), true) {
                self.tl_set_in(Some(self.cur));
            }
            if menu_item(ui, "zone-out", "Set &Out Point", &km.text(A::TlOut), true) {
                self.tl_set_out(Some(self.cur));
            }
            if menu_item(ui, "bookmark-new", "Add &Marker", &km.text(A::TlMarker), true) {
                self.tl_add_marker(now);
            }
            if menu_item(ui, "list-add", "Make &Longer by 1 Second", "", idle) {
                // about a second at the last frame's speed
                let d = self.plan.last().map(|f| effective_delay(f.delay)).unwrap_or(10).max(2) as usize;
                self.tl_lengthen((100 / d).max(1), now);
            }
            if let Some(t) = self.tl_track {
                ui.separator();
                self.tl_colour_menu(ui, t, now);
            }
            if let Some(li) = layer {
                ui.separator();
                let locked = self.stack.layers[li].locked;
                if menu_item(ui, if locked { "object-unlocked" } else { "object-locked" }, if locked { "Unlock La&yer" } else { "Lock La&yer" }, "", true) {
                    self.snapshot(&format!("tl-lock{now}"), now);
                    self.stack.layers[li].locked = !locked;
                    self.modified = true;
                }
                if menu_item(ui, "edit-delete", "D&elete Layer", "", idle) {
                    self.apply_layer_act(ctx, LayerAct::Delete(li), now);
                }
            }
        });
        // renaming a marker, in place on the ruler
        if let Some((mi, mut text)) = self.tl_marker_edit.take() {
            if let Some(m) = self.stack.markers.get(mi) {
                let pos = pos2(fx(m.frame as f32) + 10.0, area.min.y);
                let mut done: Option<bool> = None;
                egui::Area::new(egui::Id::unique("tl-marker-edit")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut text).desired_width(150.0).font(egui::FontId::proportional(12.0)));
                    if !te.has_focus() && !te.lost_focus() {
                        te.request_focus();
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = Some(false);
                    } else if te.lost_focus() {
                        done = Some(true);
                    }
                });
                match done {
                    Some(true) => {
                        if self.stack.markers[mi].label != text {
                            self.snapshot(&format!("tl-marker{now}"), now);
                            self.stack.markers[mi].label = text.trim().to_string();
                            self.modified = true;
                        }
                    }
                    Some(false) => {}
                    None => self.tl_marker_edit = Some((mi, text)),
                }
            }
        }
    }

    /// A track's colour choices: Breeze's palette, the default, or any colour.
    /// How dragging the GIF's end (or Make Longer) adds frames.
    fn tl_extend_menu(&mut self, ui: &mut egui::Ui) {
        sub_menu(ui, "", "When Made L&onger", |ui| {
            ui.set_min_width(240.0);
            for (m, label) in [(Extend::Hold, "&Hold the Last Frame"), (Extend::Loop, "&Play Again from the Start"), (Extend::Stretch, "&Stretch Over the Time")] {
                if menu_check(ui, self.tl_extend_mode == m, label, "") {
                    self.tl_extend_mode = m;
                    theme::set_setting("tl_extend", m.key());
                }
            }
        });
    }

    fn tl_colour_menu(&mut self, ui: &mut egui::Ui, track: u64, now: f64) {
        const SWATCHES: [[u8; 3]; 12] = [
            [61, 174, 233],
            [29, 153, 243],
            [26, 188, 156],
            [39, 174, 96],
            [241, 196, 15],
            [253, 188, 75],
            [246, 116, 0],
            [218, 68, 83],
            [233, 30, 99],
            [155, 89, 182],
            [142, 68, 173],
            [127, 140, 141],
        ];
        let cur = if track == GIF { self.stack.gif_color } else { self.stack.index_of(track).and_then(|li| self.stack.layers[li].color) };
        let mut pick: Option<Option<[u8; 3]>> = None;
        sub_menu(ui, "format-fill-color", "C&olour", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(6.0 * 28.0);
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for c in SWATCHES {
                    let col = Color32::from_rgb(c[0], c[1], c[2]);
                    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    let on = cur == Some(c);
                    ui.painter().rect(r.shrink(2.0), 3.0, col, Stroke::new(if on || resp.hovered() { 2.0 } else { 1.0 }, if on { p().fg } else { mix(col, Color32::BLACK, 0.3) }), egui::StrokeKind::Inside);
                    if resp.clicked() {
                        pick = Some(Some(c));
                    }
                }
            });
            ui.separator();
            let mut c = cur.unwrap_or([61, 174, 233]);
            ui.horizontal(|ui| {
                if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                    pick = Some(Some(c));
                }
                ui.label("Other colour");
            });
            if menu_item(ui, "edit-clear-all", "&Default Colour", "", cur.is_some()) {
                pick = Some(None);
            }
        });
        if let Some(c) = pick {
            self.snapshot(&format!("tl-colour{track}"), now);
            if track == GIF {
                self.stack.gif_color = c;
            } else if let Some(li) = self.stack.index_of(track) {
                self.stack.layers[li].color = c;
            }
            self.modified = true;
        }
    }

    // ---- selection, clipboard and edits on several clips

    /// Every selected clip as (track, first, last): the main one and the rest.
    fn tl_selected(&self) -> Vec<(u64, usize, usize)> {
        let mut v = Vec::new();
        if let (Some(t), Some(c)) = (self.tl_track, self.tl_clip) {
            if let Some(&(a, b)) = self.track_spans(t).get(c) {
                v.push((t, a, b));
            }
        }
        for &(t, a) in &self.tl_sel {
            if let Some(&(a, b)) = self.track_spans(t).iter().find(|s| s.0 == a) {
                if !v.contains(&(t, a, b)) {
                    v.push((t, a, b));
                }
            }
        }
        v
    }

    /// Every selected keyframe as (track, frame): the main one and the rest.
    fn tl_selected_keys(&self) -> Vec<(u64, usize)> {
        let mut v: Vec<(u64, usize)> = Vec::new();
        if let (Some(k), Some(t)) = (self.tl_key, self.tl_track) {
            if let Some(f) = self.stack.index_of(t).and_then(|li| self.stack.layers[li].timing.keys.get(k)).map(|k| k.frame) {
                v.push((t, f));
            }
        }
        for &(t, f) in &self.tl_keys {
            let there = self.stack.index_of(t).is_some_and(|li| self.stack.layers[li].timing.key_at(f).is_some());
            if there && !v.contains(&(t, f)) {
                v.push((t, f));
            }
        }
        v
    }

    fn tl_key_is_selected(&self, track: u64, k: usize, frame: usize) -> bool {
        (self.tl_track == Some(track) && self.tl_key == Some(k)) || self.tl_keys.contains(&(track, frame))
    }

    /// The main keyframe joins the others (so selecting something else
    /// keeps it selected).
    fn tl_demote_key(&mut self) {
        if let (Some(k), Some(t)) = (self.tl_key, self.tl_track) {
            if let Some(f) = self.stack.index_of(t).and_then(|li| self.stack.layers[li].timing.keys.get(k)).map(|k| k.frame) {
                if !self.tl_keys.contains(&(t, f)) {
                    self.tl_keys.push((t, f));
                }
            }
        }
        self.tl_key = None;
    }

    /// Move the keyframes at `keys` (track, frame) `d` frames, all or none:
    /// none past the ends or onto a keyframe that isn't moving. The
    /// selection follows them.
    fn tl_move_keys(&mut self, keys: &[(u64, usize)], d: i64) -> bool {
        let n = self.plan.len() as i64;
        if d == 0 || keys.is_empty() {
            return d == 0;
        }
        if keys.iter().any(|&(_, f)| f as i64 + d < 0 || f as i64 + d >= n) {
            return false;
        }
        let mut tracks: Vec<u64> = keys.iter().map(|k| k.0).collect();
        tracks.dedup();
        for &t in &tracks {
            let Some(li) = self.stack.index_of(t) else { return false };
            let moving: Vec<usize> = keys.iter().filter(|k| k.0 == t).map(|k| k.1).collect();
            let tm = &self.stack.layers[li].timing;
            if moving.iter().any(|&f| tm.keys.iter().any(|k| k.frame as i64 == f as i64 + d && !moving.contains(&k.frame))) {
                return false;
            }
        }
        let main = self.tl_key.zip(self.tl_track).and_then(|(k, t)| self.stack.index_of(t).and_then(|li| self.stack.layers[li].timing.keys.get(k)).map(|k| (t, k.frame)));
        for &t in &tracks {
            let Some(li) = self.stack.index_of(t) else { continue };
            let moving: Vec<usize> = keys.iter().filter(|k| k.0 == t).map(|k| k.1).collect();
            let tm = &mut self.stack.layers[li].timing;
            // which ones first, then move them, so they never land on each other
            let idx: Vec<usize> = tm.keys.iter().enumerate().filter(|(_, k)| moving.contains(&k.frame)).map(|(i, _)| i).collect();
            for i in idx {
                tm.keys[i].frame = (tm.keys[i].frame as i64 + d) as usize;
            }
            tm.sort_keys();
        }
        let mv = |(t, f): (u64, usize)| if keys.contains(&(t, f)) { (t, (f as i64 + d) as usize) } else { (t, f) };
        self.tl_keys = self.tl_keys.iter().map(|&k| mv(k)).collect();
        if let Some((t, f)) = main.map(mv) {
            self.tl_key = self.stack.index_of(t).and_then(|li| self.stack.layers[li].timing.key_at(f));
        }
        self.modified = true;
        true
    }

    fn tl_is_selected(&self, track: u64, ci: usize, a: usize) -> bool {
        (self.tl_track == Some(track) && self.tl_clip == Some(ci)) || self.tl_sel.contains(&(track, a))
    }

    /// A locked layer's clips and keys stay as they are.
    fn tl_locked(&self, track: u64) -> bool {
        self.stack.index_of(track).is_some_and(|li| self.stack.layers[li].locked)
    }

    /// Delete every selected clip (the GIF's skip their frames).
    pub(super) fn tl_delete_selected(&mut self, now: f64) {
        let sel = self.tl_selected();
        let keys: Vec<(u64, usize)> = self.tl_selected_keys().into_iter().filter(|k| !self.tl_locked(k.0)).collect();
        if !keys.is_empty() && (keys.len() > 1 || !sel.is_empty() || self.tl_key.is_none()) {
            // keyframes (and any clips) together
            self.snapshot(&format!("tl-del{now}"), now);
            for (t, f) in keys {
                if let Some(li) = self.stack.index_of(t) {
                    self.stack.layers[li].timing.keys.retain(|k| k.frame != f);
                }
            }
            self.tl_key = None;
            self.tl_keys.clear();
            self.modified = true;
            if sel.is_empty() {
                return;
            }
        }
        if sel.len() < 2 {
            self.tl_delete(now);
            return;
        }
        self.snapshot(&format!("tl-del{now}"), now);
        for (t, a, b) in sel {
            if self.tl_locked(t) {
                continue;
            }
            if t == GIF {
                self.gif_set(a, b, false);
            } else if let Some(l) = self.tl_layer_mut(t) {
                if let Some(cl) = &mut l.timing.clips {
                    cl.retain(|c| c.start != a);
                }
            }
        }
        self.tl_clip = None;
        self.tl_sel.clear();
        self.modified = true;
    }

    /// Ctrl+C: the selected layer clips (or the keyframe) go aside.
    pub(super) fn tl_copy(&mut self, ctx: &egui::Context) {
        if let (Some(k), Some(li)) = (self.tl_key, self.tl_track.and_then(|t| self.stack.index_of(t))) {
            if let Some(key) = self.stack.layers[li].timing.keys.get(k) {
                self.tl_board = Some(TlBoard::Key(*key));
                ctx.copy_text(BOARD_TAG.into());
                self.info("Copied the keyframe");
                return;
            }
        }
        let sel = self.tl_selected();
        // GIF clips: their frames themselves
        let mut gif: Vec<(usize, usize)> = sel.iter().filter(|c| c.0 == GIF).map(|c| (c.1, c.2)).collect();
        if !gif.is_empty() {
            gif.sort();
            match self.gif_frames(&gif) {
                Some(b) => {
                    let k = match &b {
                        TlBoard::Frames(f, _) => f.len(),
                        _ => 0,
                    };
                    self.tl_board = Some(b);
                    ctx.copy_text(BOARD_TAG.into());
                    self.info(format!("Copied {k} frames of the GIF"));
                }
                None => {}
            }
            return;
        }
        let clips: Vec<(usize, usize)> = sel.into_iter().filter(|c| c.0 != GIF).map(|c| (c.1, c.2)).collect();
        let Some(first) = clips.iter().map(|c| c.0).min() else { return };
        let n = clips.len();
        self.tl_board = Some(TlBoard::Clips(clips.into_iter().map(|(a, b)| (a - first, b - a + 1)).collect()));
        ctx.copy_text(BOARD_TAG.into());
        self.info(format!("Copied {n} clip{}", if n == 1 { "" } else { "s" }));
    }

    /// Put copied clips on `track` starting at `at`, where there's room.
    /// Returns how many went in.
    fn tl_place_clips(&mut self, track: u64, at: usize, clips: &[(usize, usize)]) -> usize {
        let n = self.plan.len();
        let mut put = 0;
        for &(off, len) in clips {
            let want = at + off;
            if want >= n || len == 0 {
                continue;
            }
            let spans = self.track_spans(track);
            // where it was asked for, or the first room after that
            let Some(a) = free_slot(&spans, len, want, n, true) else { continue };
            let b = (a + len - 1).min(n - 1);
            if let Some(l) = self.tl_layer_mut(track) {
                let cl = l.timing.clips.get_or_insert_with(Vec::new);
                cl.push(Clip { start: a, end: if b + 1 >= n { None } else { Some(b) } });
                cl.sort_by_key(|c| c.start);
                put += 1;
            }
        }
        put
    }

    /// Ctrl+V: copied clips on the selected layer at the playhead, or the
    /// copied keyframe's values at the playhead.
    pub(super) fn tl_paste(&mut self, now: f64) {
        // GIF frames go into the GIF, after the clip under the playhead
        if let Some(TlBoard::Frames(frames, places)) = self.tl_board.clone() {
            self.snapshot(&format!("tl-paste{now}"), now);
            let at = self.gif_after_playhead();
            let k = frames.len();
            self.gif_insert(at, frames, &places);
            self.info(format!("Pasted {k} frames into the GIF"));
            return;
        }
        let Some(track) = self.tl_track.filter(|t| *t != GIF && !self.tl_locked(*t)) else {
            self.info("Select a layer's track to paste onto");
            return;
        };
        match self.tl_board.clone() {
            Some(TlBoard::Key(k)) => {
                self.snapshot(&format!("tl-key{now}"), now);
                if let Some(li) = self.stack.index_of(track) {
                    let t = &mut self.stack.layers[li].timing;
                    let i = t.add_key(self.cur);
                    t.keys[i] = Key { frame: self.cur, ..k };
                    self.tl_key = Some(i);
                    self.modified = true;
                }
            }
            Some(TlBoard::Clips(c)) => {
                self.snapshot(&format!("tl-add{now}"), now);
                let put = self.tl_place_clips(track, self.cur, &c);
                self.modified |= put > 0;
                self.info(if put == c.len() { format!("Pasted {put} clip{}", if put == 1 { "" } else { "s" }) } else { format!("Pasted {put} of {} clips (no room left after the playhead)", c.len()) });
            }
            Some(TlBoard::Frames(..)) | None => {}
        }
    }

    /// Ctrl+D: copies of the selected layer clips, right after them.
    pub(super) fn tl_duplicate(&mut self, now: f64) {
        // GIF clips: a copy of their frames right after each
        let mut gif: Vec<(usize, usize)> = self.tl_selected().into_iter().filter(|c| c.0 == GIF).map(|c| (c.1, c.2)).collect();
        if !gif.is_empty() {
            self.snapshot(&format!("tl-dup{now}"), now);
            // from the last, so earlier positions stay put
            gif.sort();
            for &(a, b) in gif.iter().rev() {
                if let Some(TlBoard::Frames(frames, places)) = self.gif_frames(&[(a, b)]) {
                    self.gif_insert(b + 1, frames, &places);
                }
            }
            return;
        }
        let sel: Vec<(u64, usize, usize)> = self.tl_selected().into_iter().filter(|c| c.0 != GIF && !self.tl_locked(c.0)).collect();
        if sel.is_empty() {
            return;
        }
        self.snapshot(&format!("tl-add{now}"), now);
        let mut put = 0;
        for (t, a, b) in sel {
            put += self.tl_place_clips(t, b + 1, &[(0, b - a + 1)]);
        }
        self.modified |= put > 0;
        if put == 0 {
            self.info("No room after the clip to put a copy");
        }
    }

    /// Alt+arrows: the selected clips (or keyframe) a frame earlier or later.
    fn tl_nudge(&mut self, d: i64, now: f64) {
        let n = self.plan.len();
        // several keyframes: all of them, or none
        let keys: Vec<(u64, usize)> = self.tl_selected_keys().into_iter().filter(|k| !self.tl_locked(k.0)).collect();
        if keys.len() > 1 {
            self.snapshot(&format!("tl-drag{now}"), now);
            if self.tl_move_keys(&keys, d) {
                self.cur = (self.cur as i64 + d).clamp(0, n as i64 - 1) as usize;
            }
            return;
        }
        if let (Some(k), Some(t)) = (self.tl_key, self.tl_track.filter(|t| !self.tl_locked(*t))) {
            if let Some(li) = self.stack.index_of(t) {
                let tm = &mut self.stack.layers[li].timing;
                let Some(f) = tm.keys.get(k).map(|k| k.frame) else { return };
                let nf = (f as i64 + d).clamp(0, n as i64 - 1) as usize;
                if nf != f && tm.key_at(nf).is_none() {
                    self.snapshot(&format!("tl-drag{now}"), now);
                    let tm = &mut self.stack.layers[li].timing;
                    tm.keys[k].frame = nf;
                    tm.sort_keys();
                    self.tl_key = tm.key_at(nf);
                    self.cur = nf;
                    self.modified = true;
                }
            }
            return;
        }
        let sel: Vec<(u64, usize, usize)> = self.tl_selected().into_iter().filter(|c| c.0 != GIF && !self.tl_locked(c.0)).collect();
        if sel.is_empty() {
            return;
        }
        // all of them move, or none: each stays clear of its unselected neighbours
        let ok = sel.iter().all(|&(t, a, b)| {
            let (na, nb) = (a as i64 + d, b as i64 + d);
            na >= 0 && nb < n as i64 && !self.track_spans(t).iter().any(|&(x, y)| !sel.contains(&(t, x, y)) && na <= y as i64 && nb >= x as i64)
        });
        if !ok {
            return;
        }
        self.snapshot(&format!("tl-drag{now}"), now);
        self.tl_shift_clips(&sel, d);
    }

    /// Move clips by `d` frames (already checked to fit), keeping them selected.
    fn tl_shift_clips(&mut self, sel: &[(u64, usize, usize)], d: i64) {
        let n = self.plan.len();
        let tracks: Vec<u64> = sel.iter().map(|c| c.0).collect();
        for t in tracks {
            if let Some(l) = self.tl_layer_mut(t) {
                if let Some(cl) = &mut l.timing.clips {
                    for c in cl.iter_mut() {
                        let b = c.last(n);
                        if sel.contains(&(t, c.start, b)) {
                            let (na, nb) = ((c.start as i64 + d) as usize, (b as i64 + d) as usize);
                            *c = Clip { start: na, end: if nb + 1 >= n { None } else { Some(nb) } };
                        }
                    }
                    cl.sort_by_key(|c| c.start);
                }
            }
        }
        self.tl_sel = self.tl_sel.iter().map(|&(t, a)| (t, (a as i64 + d) as usize)).collect();
        if let (Some(t), Some(_)) = (self.tl_track, self.tl_clip) {
            if let Some(&(_, a, _)) = sel.iter().find(|c| c.0 == t) {
                let na = (a as i64 + d) as usize;
                self.tl_clip = self.track_spans(t).iter().position(|s| s.0 == na);
            }
        }
        self.modified = true;
    }

    /// Frames where something starts or ends: clip edges, keyframes,
    /// markers, the in and out points, the start and the end.
    fn tl_edit_points(&self) -> Vec<usize> {
        let n = self.plan.len();
        let mut v = vec![0, n];
        let mut tracks = vec![GIF];
        tracks.extend(self.stack.layers.iter().map(|l| l.id));
        for t in tracks {
            for (a, b) in self.track_spans(t) {
                v.push(a);
                v.push(b + 1);
            }
        }
        for l in &self.stack.layers {
            v.extend(l.timing.keys.iter().map(|k| k.frame));
        }
        v.extend(self.stack.markers.iter().map(|m| m.frame));
        v.extend(self.tl_in);
        v.extend(self.tl_out.map(|o| o + 1));
        v.sort();
        v.dedup();
        v
    }

    /// Up / Down: the playhead to the previous or next edit point.
    pub(super) fn tl_jump(&mut self, forward: bool) {
        let n = self.plan.len();
        if n == 0 {
            return;
        }
        let pts = self.tl_edit_points();
        let to = if forward { pts.into_iter().find(|&p| p > self.cur && p < n) } else { pts.into_iter().rev().find(|&p| p < self.cur) };
        if let Some(f) = to {
            self.cur = f.min(n - 1);
            self.playing = false;
            self.elapsed = 0.0;
        }
    }

    pub(super) fn tl_set_in(&mut self, f: Option<usize>) {
        self.tl_in = f;
        if let (Some(a), Some(b)) = (self.tl_in, self.tl_out) {
            if a > b {
                self.tl_out = None;
            }
        }
        self.tl_zone_loop = true;
    }

    pub(super) fn tl_set_out(&mut self, f: Option<usize>) {
        self.tl_out = f;
        if let (Some(a), Some(b)) = (self.tl_in, self.tl_out) {
            if a > b {
                self.tl_in = None;
            }
        }
        self.tl_zone_loop = true;
    }

    /// M: a marker at the playhead.
    pub(super) fn tl_add_marker(&mut self, now: f64) {
        if self.stack.markers.iter().any(|m| m.frame == self.cur) {
            return;
        }
        self.snapshot(&format!("tl-marker{now}"), now);
        let label = format!("Marker {}", self.stack.markers.len() + 1);
        self.stack.markers.push(Marker { frame: self.cur, label });
        self.stack.markers.sort_by_key(|m| m.frame);
        self.modified = true;
    }

    /// The nearest snap target to `pos` (a frame boundary), within `thr` frames.
    fn tl_snap_to(&self, pos: f32, thr: f32, skip: &[usize]) -> Option<usize> {
        let mut pts = self.tl_edit_points();
        pts.push(self.cur);
        pts.into_iter().filter(|p| !skip.contains(p)).map(|p| (p, (p as f32 - pos).abs())).filter(|(_, d)| *d <= thr).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(p, _)| p)
    }


    /// The last frame any clip covers: the GIF's clips, and layer clips with
    /// an end of their own (ones that run to the end follow the length).
    pub(super) fn tl_last_clip_end(&self) -> Option<usize> {
        let mut last = self.gif_runs().iter().map(|r| r.1).max();
        let n = self.plan.len();
        for l in &self.stack.layers {
            if let Some(cl) = &l.timing.clips {
                for c in cl {
                    let e = match c.end {
                        Some(e) => e,
                        // a clip that starts later than everything still counts from its start
                        None => c.start,
                    };
                    last = Some(last.map_or(e, |x: usize| x.max(e)));
                }
            }
        }
        last.map(|l| l.min(n.saturating_sub(1)))
    }

    /// Shorten the animation to where its last clip ends.
    pub(super) fn tl_trim_to_last(&mut self, now: f64) {
        let Some(e) = self.tl_last_clip_end() else { return };
        if e + 1 >= self.gif_edit_len() {
            self.info("Nothing after the last clip to trim");
            return;
        }
        self.snapshot(&format!("tl-trimend{now}"), now);
        self.tl_cut(0, e);
    }

    /// Keep only output frames `a..=b`: the Cut edit takes the rest off, and
    /// layers move with the frames so they stay where they were.
    fn tl_cut(&mut self, a: usize, b: usize) {
        let n = self.gif_edit_len();
        if n == 0 || b >= n || a > b || (a == 0 && b + 1 == n) {
            return;
        }
        // the source frames those are, whichever way it plays
        let s = (a..=b).map(|i| self.plan[i].src).min().unwrap_or(0);
        let e = (a..=b).map(|i| self.plan[i].src).max().unwrap_or(0);
        let last = self.base.as_ref().map(|x| x.len()).unwrap_or(0);
        // layers on output frames: give them clips, then slide them back
        for li in 0..self.stack.layers.len() {
            self.stack.ensure_clips(li, &self.plan);
            let t = &mut self.stack.layers[li].timing;
            if a > 0 {
                if let Some(cl) = &mut t.clips {
                    cl.retain(|c| c.end.is_none_or(|e| e >= a));
                    for c in cl.iter_mut() {
                        c.start = c.start.saturating_sub(a);
                        c.end = c.end.map(|e| e - a);
                    }
                }
                for k in &mut t.keys {
                    k.frame = k.frame.saturating_sub(a);
                }
                // keys that land on the same frame: keep the later one
                t.keys.reverse();
                let mut seen = std::collections::HashSet::new();
                t.keys.retain(|k| seen.insert(k.frame));
                t.sort_keys();
            }
            if let Some(cl) = &mut t.clips {
                // with Boomerang the part played back shortens too
                let kept = b - a + 1;
                let total = if self.stack.transform.play_on && self.stack.transform.boomerang && kept > 2 { kept * 2 - 2 } else { kept };
                let m = total - 1;
                cl.retain(|c| c.start <= m);
                for c in cl.iter_mut() {
                    if c.end.is_some_and(|e| e >= m) {
                        c.end = None;
                    }
                }
            }
        }
        let tr = &mut self.stack.transform;
        tr.cut_on = true;
        tr.cut_by_time = false;
        tr.cut_start = s + 1;
        tr.cut_end = if e + 1 >= last { 0 } else { e + 1 };
        self.cur = self.cur.saturating_sub(a).min(b - a);
        self.tl_clip = None;
        self.modified = true;
        self.timing_rev += 1;
        self.info(format!("Cut to {} frames", b - a + 1));
    }

    /// Make the animation `k` frames longer by holding its last frame, so a
    /// single picture can become an animation for layers to move over.
    fn tl_lengthen(&mut self, k: usize, now: f64) {
        if k == 0 || self.busy() || self.base.is_none() {
            return;
        }
        self.snapshot(&format!("tl-lengthen{now}"), now);
        let mut k = k;
        // frames the Cut edit took off the end come back first
        let fwd = !(self.stack.transform.play_on && self.stack.transform.reverse);
        if let (true, Some(b)) = (fwd && self.tl_extend_mode == Extend::Hold, &self.base) {
            let tr = &mut self.stack.transform;
            if tr.cut_on && !tr.cut_by_time && tr.cut_end != 0 {
                let back = (b.len() - tr.cut_end).min(k);
                tr.cut_end += back;
                if tr.cut_end >= b.len() {
                    tr.cut_end = 0;
                }
                k -= back;
                self.modified = true;
                self.timing_rev += 1;
                if k == 0 {
                    self.info(format!("Brought back {back} frames"));
                    return;
                }
            }
        }
        self.gif_extend(k, self.tl_extend_mode);
    }

    /// Move clip `ci` of a layer's row to start at `w` (checked to be free).
    fn tl_layer_place(&mut self, track: u64, ci: usize, w: usize) {
        let n = self.plan.len();
        let Some(&(a, b)) = self.track_spans(track).get(ci) else { return };
        let nb = w + (b - a);
        let Some(l) = self.tl_layer_mut(track) else { return };
        if let Some(cl) = &mut l.timing.clips {
            cl.sort_by_key(|c| c.start);
            if let Some(c) = cl.get_mut(ci) {
                *c = Clip { start: w, end: if nb + 1 >= n { None } else { Some(nb) } };
            }
            cl.sort_by_key(|c| c.start);
        }
        self.tl_clip = self.track_spans(track).iter().position(|s| s.0 == w);
        self.tl_sel.clear();
        self.modified = true;
    }

    /// Move the GIF's clip `a..=b` into the gap starting at `w`: where it
    /// was becomes a gap, nothing else moves.
    fn gif_place(&mut self, a: usize, b: usize, w: usize, now: f64) {
        let Some((s, e, mut d)) = self.gif_display() else { return };
        if b >= d.len() || w + (b - a) >= d.len() {
            return;
        }
        self.snapshot(&format!("tl-gifmove{now}"), now);
        let block: Vec<crate::anim::Frame> = d[a..=b].to_vec();
        let len = block.len();
        for f in &mut d[a..=b] {
            f.blank = true;
        }
        for (j, f) in block.into_iter().enumerate() {
            d[w + j] = crate::anim::Frame { blank: false, enabled: true, cut: j == 0, ..f };
        }
        if let Some(f) = d.get_mut(w + len) {
            f.cut = true;
        }
        self.gif_write(s, e, d);
        self.tl_clip = self.gif_runs().iter().position(|r| r.0 == w);
        self.cur = w.min(self.plan.len().saturating_sub(1));
        self.info(format!("Moved {len} frames"));
    }

    /// Put clip `ci` of a layer's row in between its others at frame `t`:
    /// the clips it passes over move along by its length to make room.
    fn tl_layer_reorder(&mut self, track: u64, ci: usize, t: usize) {
        let n = self.plan.len();
        let spans = self.track_spans(track);
        let Some(&(a, b)) = spans.get(ci) else { return };
        let len = b - a + 1;
        let moved: Vec<(usize, usize)> = spans
            .iter()
            .enumerate()
            .map(|(j, &(x, y))| {
                if j == ci {
                    let s = if t <= a { t } else { t - len };
                    (s, s + len - 1)
                } else if t <= a && x >= t && x < a {
                    (x + len, y + len)
                } else if t > b && x > b && x < t {
                    (x - len, y - len)
                } else {
                    (x, y)
                }
            })
            .collect();
        let new_start = moved[ci].0;
        let Some(l) = self.tl_layer_mut(track) else { return };
        l.timing.clips = Some(moved.iter().map(|&(x, y)| Clip { start: x, end: if y + 1 >= n { None } else { Some(y) } }).collect());
        if let Some(cl) = &mut l.timing.clips {
            cl.sort_by_key(|c| c.start);
        }
        self.tl_clip = self.track_spans(track).iter().position(|s| s.0 == new_start);
        self.modified = true;
    }

    /// Ctrl+X: copy the selected clips (or keyframe), then take them out.
    pub(super) fn tl_cut_clips(&mut self, ctx: &egui::Context, now: f64) {
        let before = self.tl_board.is_some();
        self.tl_copy(ctx);
        if self.tl_board.is_some() || before {
            self.tl_delete_selected(now);
        }
    }

    /// GIF clips (output frames, in order) as frames to paste.
    fn gif_frames(&self, clips: &[(usize, usize)]) -> Option<TlBoard> {
        let (_, _, d) = self.gif_display()?;
        let mut frames: Vec<crate::anim::Frame> = Vec::new();
        for &(a, b) in clips {
            for i in a..=b.min(d.len().saturating_sub(1)) {
                frames.push(d[i].clone());
            }
        }
        if let Some(f) = frames.first_mut() {
            f.cut = true;
        }
        Some(TlBoard::Frames(frames, Vec::new()))
    }

    /// Where pasted GIF frames go (a place in the shown order): after the
    /// clip under the playhead, or at the playhead in a gap.
    fn gif_after_playhead(&self) -> usize {
        let f = self.gif_edit_len();
        if f == 0 {
            return 0;
        }
        let cur = self.cur.min(f - 1);
        match self.gif_runs().into_iter().find(|&(a, b)| cur >= a && cur <= b) {
            Some((_, b)) => b + 1,
            None => cur,
        }
    }

    /// Put `frames` into the GIF at `at` (in the shown order), as a clip of
    /// their own.
    fn gif_insert(&mut self, at: usize, frames: Vec<crate::anim::Frame>, _places: &[(u64, Vec<crate::overlay::Placement>)]) {
        if frames.is_empty() {
            return;
        }
        let Some((s, e, mut d)) = self.gif_display() else { return };
        let k = frames.len();
        let mut at = at.min(d.len());
        let mut frames = frames;
        frames[0].cut = true;
        for f in &mut frames {
            f.blank = false;
            f.enabled = true;
        }
        // into a gap: they fill it, and only what doesn't fit pushes on
        let gap = |f: &crate::anim::Frame| f.blank || !f.enabled;
        let mut fill = 0;
        if at < d.len() && gap(&d[at]) {
            let mut g0 = at;
            while g0 > 0 && gap(&d[g0 - 1]) {
                g0 -= 1;
            }
            let mut g1 = at;
            while g1 < d.len() && gap(&d[g1]) {
                g1 += 1;
            }
            // from the playhead if they fit, else as late in the gap as fits
            at = if g1 - at >= k { at } else { g0.max(g1.saturating_sub(k)) };
            fill = (g1 - at).min(k);
        }
        let rest = frames.split_off(fill);
        for (j, f) in frames.into_iter().enumerate() {
            d[at + j] = f;
        }
        d.splice(at + fill..at + fill, rest);
        if let Some(f) = d.get_mut(at + k) {
            f.cut = true;
        }
        self.gif_write(s, e, d);
        self.cur = at.min(self.plan.len().saturating_sub(1));
        self.tl_clip = self.gif_runs().iter().position(|r| r.0 == at);
    }

    /// Make the GIF `k` frames longer: hold its last frame, play it again
    /// from its start, or spread its frames over the longer time.
    fn gif_extend(&mut self, k: usize, how: Extend) {
        let Some((s, e, d)) = self.gif_display() else { return };
        let shown: Vec<usize> = (0..d.len()).filter(|&i| d[i].enabled && !d[i].blank).collect();
        let Some(&last) = shown.last() else { return };
        let mut d = d;
        match how {
            Extend::Hold => {
                let f = crate::anim::Frame { cut: false, enabled: true, ..d[last].clone() };
                d.extend(std::iter::repeat_n(f, k));
                self.info(format!("Added {k} frames"));
            }
            Extend::Loop => {
                let add: Vec<crate::anim::Frame> = (0..k).map(|j| crate::anim::Frame { cut: false, ..d[shown[j % shown.len()]].clone() }).collect();
                d.extend(add);
                self.info(format!("Added {k} frames, playing it again from the start"));
            }
            Extend::Stretch => {
                let len = d.len();
                let new_len = len + k;
                let pick: Vec<usize> = (0..new_len).map(|i| i * len / new_len).collect();
                let mut made: Vec<crate::anim::Frame> = pick.iter().map(|&i| d[i].clone()).collect();
                // a split stays where it was: only the first copy of a frame keeps it
                for i in 1..made.len() {
                    if pick[i] == pick[i - 1] {
                        made[i].cut = false;
                    }
                }
                d = made;
                self.info(format!("Stretched {len} frames over {new_len}"));
            }
        }
        self.gif_write(s, e, d);
    }

    /// Output frames the GIF row can edit: with Boomerang, the first half
    /// (the second plays it back and follows it).
    pub(super) fn gif_edit_len(&self) -> usize {
        let n = self.plan.len();
        let t = &self.stack.transform;
        if let Some(base) = &self.base {
            if t.play_on && t.boomerang {
                let (s, e) = t.cut_range(base);
                let f = e + 1 - s;
                if f > 2 && f <= n {
                    return f;
                }
            }
        }
        n
    }

    /// The GIF's frames as the timeline shows them (reversed when Reverse is
    /// on), with the source range they come from.
    fn gif_display(&self) -> Option<(usize, usize, Vec<crate::anim::Frame>)> {
        let base = self.base.as_ref()?;
        if base.is_empty() {
            return None;
        }
        let (s, e) = self.stack.transform.cut_range(base);
        let mut d: Vec<crate::anim::Frame> = base.frames[s..=e].to_vec();
        let t = &self.stack.transform;
        if t.play_on && t.reverse {
            d.reverse();
        }
        Some((s, e, d))
    }

    /// Put the GIF's frames back from the order the timeline shows them in.
    /// Layers stay where they are in time; ones that reached the end still do.
    fn gif_write(&mut self, s: usize, e: usize, mut d: Vec<crate::anim::Frame>) {
        self.pin_layers();
        let old_n = self.plan.len();
        let t = &self.stack.transform;
        if t.play_on && t.reverse {
            d.reverse();
        }
        let len = d.len();
        let Some(base) = self.base.as_mut() else { return };
        base.frames.splice(s..=e, d);
        let t = &mut self.stack.transform;
        if t.cut_on && !t.cut_by_time && t.cut_end != 0 {
            // the kept range now ends where the new frames do (counted from 1)
            t.cut_end = s + len;
        }
        self.frames_changed();
        let new_n = self.plan.len();
        if new_n != old_n {
            for l in &mut self.stack.layers {
                if let Some(cl) = &mut l.timing.clips {
                    for c in cl.iter_mut() {
                        if c.end.is_some_and(|x| x + 1 >= old_n) {
                            c.end = None;
                        }
                    }
                }
            }
        }
    }

    /// Before the GIF's frames change order: layers that still use an old
    /// frame range (counted in GIF frames) get clips, so they stay in time
    /// instead of following the frames about.
    fn pin_layers(&mut self) {
        for li in 0..self.stack.layers.len() {
            self.stack.ensure_clips(li, &self.plan);
        }
    }

    /// The animation's frames were added or reordered.
    fn frames_changed(&mut self) {
        self.bump();
        self.after_base_change(false);
        self.stack.sync_len(self.base.as_ref().map(|b| b.len()).unwrap_or(0));
        self.timing_rev += 1;
        self.modified = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    // frames told apart by their delay; `g` marks gaps
    fn fr(spec: &str) -> Vec<crate::anim::Frame> {
        spec.split(' ')
            .map(|t| {
                let mut f = crate::anim::Frame::new(image::RgbaImage::new(1, 1), 0);
                f.blank = t == "g";
                f.delay_cs = t.parse().unwrap_or(0);
                f
            })
            .collect()
    }
    fn show(d: &[crate::anim::Frame]) -> String {
        d.iter().map(|f| if f.blank { "g".into() } else { format!("{}{}", if f.cut { "|" } else { "" }, f.delay_cs) }).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn edge_fills_gaps() {
        let d = fr("1 2 3 g g g g 8");
        assert_eq!(show(&gif_edge_fill(&d, (0, 2), (0, 5), Extend::Hold)), "|1 2 3 3 3 3 g 8");
        assert_eq!(show(&gif_edge_fill(&d, (0, 2), (0, 6), Extend::Loop)), "|1 2 3 1 2 3 1 |8");
        assert_eq!(show(&gif_edge_fill(&d, (0, 2), (0, 5), Extend::Stretch)), "|1 1 2 2 3 3 g 8");
        // trimming leaves a gap, and pulling back out undoes it
        assert_eq!(show(&gif_edge_fill(&d, (0, 2), (0, 0), Extend::Hold)), "|1 g g g g g g 8");
        assert_eq!(show(&gif_edge_fill(&d, (0, 2), (0, 2), Extend::Hold)), "|1 2 3 g g g g 8");
        // the start edge pulled back into a gap
        let d = fr("g g g 4 5");
        assert_eq!(show(&gif_edge_fill(&d, (3, 4), (1, 4), Extend::Hold)), "g |4 4 4 5");
        assert_eq!(show(&gif_edge_fill(&d, (3, 4), (0, 4), Extend::Loop)), "|5 4 5 4 5");
    }

    #[test]
    fn free_slot_finds_room() {
        // clips at 10..=19 and 30..=39 on 50 frames
        let sp = [(10, 19), (30, 39)];
        assert_eq!(free_slot(&sp, 5, 0, 50, false), Some(0));
        // asked to overlap: the nearest room
        assert_eq!(free_slot(&sp, 5, 12, 50, false), Some(5));
        assert_eq!(free_slot(&sp, 5, 17, 50, false), Some(20));
        // only after the playhead
        assert_eq!(free_slot(&sp, 5, 12, 50, true), Some(20));
        // too long for any room after
        assert_eq!(free_slot(&sp, 11, 12, 50, true), None);
        // past the end: the last place it fits
        assert_eq!(free_slot(&sp, 5, 48, 50, false), Some(45));
        assert_eq!(free_slot(&[], 60, 0, 50, false), None);
    }

}
