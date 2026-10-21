//! Keyboard shortcuts the user can change: each action has a default
//! binding, and choices are saved in the settings as "keys".

use eframe::egui::{self, Key, Modifiers};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Quit,
    Undo,
    Redo,
    History,
    PlayPause,
    PrevFrame,
    NextFrame,
    ZoomIn,
    ZoomOut,
    Fit,
    ActualSize,
    SplitView,
    FullScreen,
    Handbook,
    DeleteObject,
    TlSplit,
    TlKey,
    TlRazor,
    TlDelete,
    TlIn,
    TlOut,
    TlMarker,
    TlPrevEdit,
    TlNextEdit,
    TlDuplicate,
    TlNudgeLeft,
    TlNudgeRight,
    Duplicate,
}

impl Action {
    pub const ALL: [Action; 33] = [
        Action::New,
        Action::Open,
        Action::Save,
        Action::SaveAs,
        Action::Export,
        Action::Quit,
        Action::Undo,
        Action::Redo,
        Action::History,
        Action::PlayPause,
        Action::PrevFrame,
        Action::NextFrame,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::Fit,
        Action::ActualSize,
        Action::SplitView,
        Action::FullScreen,
        Action::Handbook,
        Action::DeleteObject,
        Action::TlSplit,
        Action::TlKey,
        Action::TlRazor,
        Action::TlDelete,
        Action::TlIn,
        Action::TlOut,
        Action::TlMarker,
        Action::TlPrevEdit,
        Action::TlNextEdit,
        Action::TlDuplicate,
        Action::TlNudgeLeft,
        Action::TlNudgeRight,
        Action::Duplicate,
    ];

    /// The groups the shortcuts dialog shows, in order.
    pub const GROUPS: [&'static str; 6] = ["File", "Edit", "Playback", "View", "Timeline", "Help"];

    /// Which of `GROUPS` this belongs to.
    pub fn group(self) -> &'static str {
        use Action::*;
        match self {
            New | Open | Save | SaveAs | Export | Quit => "File",
            Undo | Redo | History | Duplicate | DeleteObject => "Edit",
            PlayPause | PrevFrame | NextFrame => "Playback",
            ZoomIn | ZoomOut | Fit | ActualSize | SplitView | FullScreen => "View",
            TlSplit | TlKey | TlRazor | TlDelete | TlIn | TlOut | TlMarker | TlPrevEdit | TlNextEdit | TlDuplicate | TlNudgeLeft | TlNudgeRight => "Timeline",
            Handbook => "Help",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::New => "New project",
            Action::Open => "Open",
            Action::Save => "Save the project",
            Action::SaveAs => "Save the project as",
            Action::Export => "Export",
            Action::Quit => "Quit",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::History => "History",
            Action::PlayPause => "Play or pause",
            Action::PrevFrame => "Previous frame",
            Action::NextFrame => "Next frame",
            Action::ZoomIn => "Zoom in",
            Action::ZoomOut => "Zoom out",
            Action::Fit => "Fit to view",
            Action::ActualSize => "Actual size",
            Action::SplitView => "Single or split view",
            Action::FullScreen => "Full screen",
            Action::Handbook => "Handbook",
            Action::DeleteObject => "Delete the selected object",
            Action::TlSplit => "Timeline: split at the playhead",
            Action::TlKey => "Timeline: add a keyframe",
            Action::TlRazor => "Timeline: selection or razor tool",
            Action::TlDelete => "Timeline: delete clip or keyframe",
            Action::TlIn => "Timeline: set the in point",
            Action::TlOut => "Timeline: set the out point",
            Action::TlMarker => "Timeline: add a marker",
            Action::TlPrevEdit => "Timeline: go to the previous edit",
            Action::TlNextEdit => "Timeline: go to the next edit",
            Action::TlDuplicate => "Timeline: duplicate the selected clips",
            Action::TlNudgeLeft => "Timeline: nudge the selection a frame earlier",
            Action::TlNudgeRight => "Timeline: nudge the selection a frame later",
            Action::Duplicate => "Duplicate the selected layer",
        }
    }

    /// Name in the settings file (stays the same when labels change).
    fn id(self) -> &'static str {
        match self {
            Action::New => "new",
            Action::Open => "open",
            Action::Save => "save",
            Action::SaveAs => "save_as",
            Action::Export => "export",
            Action::Quit => "quit",
            Action::Undo => "undo",
            Action::Redo => "redo",
            Action::History => "history",
            Action::PlayPause => "play",
            Action::PrevFrame => "prev",
            Action::NextFrame => "next",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::Fit => "fit",
            Action::ActualSize => "actual",
            Action::SplitView => "split",
            Action::FullScreen => "full",
            Action::Handbook => "help",
            Action::DeleteObject => "delete",
            Action::TlSplit => "tl_split",
            Action::TlKey => "tl_key",
            Action::TlRazor => "tl_razor",
            Action::TlDelete => "tl_delete",
            Action::TlIn => "tl_in",
            Action::TlOut => "tl_out",
            Action::TlMarker => "tl_marker",
            Action::TlPrevEdit => "tl_prev_edit",
            Action::TlNextEdit => "tl_next_edit",
            Action::TlDuplicate => "tl_duplicate",
            Action::TlNudgeLeft => "tl_nudge_left",
            Action::TlNudgeRight => "tl_nudge_right",
            Action::Duplicate => "duplicate",
        }
    }

    pub fn default_binding(self) -> Option<Binding> {
        let c = |key| Some(Binding { ctrl: true, shift: false, alt: false, key });
        let cs = |key| Some(Binding { ctrl: true, shift: true, alt: false, key });
        let k = |key| Some(Binding { ctrl: false, shift: false, alt: false, key });
        let a = |key| Some(Binding { ctrl: false, shift: false, alt: true, key });
        match self {
            Action::New => c(Key::N),
            Action::Open => c(Key::O),
            Action::Save => c(Key::S),
            Action::SaveAs => cs(Key::S),
            Action::Export => c(Key::E),
            Action::Quit => c(Key::Q),
            Action::Undo => c(Key::Z),
            Action::Redo => c(Key::Y),
            Action::History => c(Key::H),
            Action::PlayPause => k(Key::Space),
            Action::PrevFrame => k(Key::ArrowLeft),
            Action::NextFrame => k(Key::ArrowRight),
            Action::ZoomIn => k(Key::Plus),
            Action::ZoomOut => k(Key::Minus),
            Action::Fit => k(Key::F),
            Action::ActualSize => k(Key::Num1),
            Action::SplitView => k(Key::C),
            Action::FullScreen => cs(Key::F),
            Action::Handbook => k(Key::F1),
            Action::DeleteObject => k(Key::Delete),
            Action::TlSplit => k(Key::S),
            Action::TlKey => k(Key::K),
            Action::TlRazor => k(Key::X),
            Action::TlDelete => k(Key::Delete),
            Action::TlIn => k(Key::I),
            Action::TlOut => k(Key::O),
            Action::TlMarker => k(Key::M),
            Action::TlPrevEdit => k(Key::ArrowUp),
            Action::TlNextEdit => k(Key::ArrowDown),
            Action::TlDuplicate => c(Key::D),
            Action::TlNudgeLeft => a(Key::ArrowLeft),
            Action::TlNudgeRight => a(Key::ArrowRight),
            Action::Duplicate => c(Key::D),
        }
    }

    /// Timeline keys only work with the pointer over the timeline, so they can
    /// share letters with the rest.
    pub fn timeline(self) -> bool {
        matches!(self, Action::TlSplit | Action::TlKey | Action::TlRazor | Action::TlDelete | Action::TlDuplicate | Action::TlNudgeLeft | Action::TlNudgeRight)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Binding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Binding {
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str(if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" });
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(match self.key {
            Key::Plus => "+",
            Key::Minus => "−",
            Key::Equals => "=",
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            k => k.name(),
        });
        s
    }

    fn save(&self) -> String {
        format!("{}{}{}{}", if self.ctrl { "c" } else { "" }, if self.shift { "s" } else { "" }, if self.alt { "a" } else { "" }, format_args!("-{}", self.key.name()))
    }

    fn load(t: &str) -> Option<Binding> {
        let (m, k) = t.split_once('-')?;
        Some(Binding { ctrl: m.contains('c'), shift: m.contains('s'), alt: m.contains('a'), key: Key::from_name(k)? })
    }

    /// This binding was pressed in this frame. Shift is ignored for keys
    /// like + and − that need it on some keyboards, unless it's part of the binding.
    pub fn pressed(&self, i: &egui::InputState) -> bool {
        let m = i.modifiers;
        let letterish = self.key.name().chars().count() == 1 && self.key.name().chars().all(|c| c.is_ascii_alphanumeric());
        let shift_ok = m.shift == self.shift || (!self.shift && !letterish && !matches!(self.key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown));
        m.command == self.ctrl && m.alt == self.alt && shift_ok && i.key_pressed(self.key)
    }

    /// The binding a key press makes (None for modifier keys on their own).
    pub fn from_press(key: Key, m: Modifiers) -> Binding {
        Binding { ctrl: m.command, shift: m.shift, alt: m.alt, key }
    }
}

#[derive(Clone, Debug)]
pub struct Keymap {
    map: HashMap<Action, Option<Binding>>,
}

impl Keymap {
    pub fn load() -> Keymap {
        let mut map: HashMap<Action, Option<Binding>> = Action::ALL.iter().map(|a| (*a, a.default_binding())).collect();
        if let Some(saved) = crate::theme::get_setting("keys") {
            for part in saved.split(';') {
                let Some((id, b)) = part.split_once('=') else { continue };
                if let Some(a) = Action::ALL.iter().find(|a| a.id() == id) {
                    map.insert(*a, if b.is_empty() { None } else { Binding::load(b) });
                }
            }
        }
        Keymap { map }
    }

    fn save(&self) {
        // only what differs from the defaults
        let parts: Vec<String> = Action::ALL
            .iter()
            .filter(|a| self.get(**a) != a.default_binding())
            .map(|a| format!("{}={}", a.id(), self.get(*a).map(|b| b.save()).unwrap_or_default()))
            .collect();
        crate::theme::set_setting("keys", &parts.join(";"));
    }

    pub fn get(&self, a: Action) -> Option<Binding> {
        self.map.get(&a).copied().flatten()
    }

    /// Text for menus and tooltips ("" when unbound).
    pub fn text(&self, a: Action) -> String {
        self.get(a).map(|b| b.text()).unwrap_or_default()
    }

    pub fn pressed(&self, i: &egui::InputState, a: Action) -> bool {
        // Redo also answers to Ctrl+Shift+Z, as most apps do
        self.get(a).is_some_and(|b| b.pressed(i)) || (a == Action::Redo && self.get(a) == a.default_binding() && i.modifiers.command && i.modifiers.shift && i.key_pressed(Key::Z)) || (a == Action::ZoomIn && self.get(a).is_some_and(|b| b.key == Key::Plus && !b.ctrl) && !i.modifiers.command && i.key_pressed(Key::Equals))
    }

    /// Set a binding, taking it from any other action that had it (in the
    /// same group: timeline keys and the rest don't clash). Returns what lost it.
    pub fn set(&mut self, a: Action, b: Option<Binding>) -> Option<Action> {
        let mut lost = None;
        if let Some(b) = b {
            for o in Action::ALL {
                if o != a && o.timeline() == a.timeline() && self.get(o) == Some(b) {
                    self.map.insert(o, None);
                    lost = Some(o);
                }
            }
        }
        self.map.insert(a, b);
        self.save();
        lost
    }

    pub fn reset(&mut self, a: Action) {
        self.set(a, a.default_binding());
    }

    pub fn reset_all(&mut self) {
        self.map = Action::ALL.iter().map(|a| (*a, a.default_binding())).collect();
        self.save();
    }
}
