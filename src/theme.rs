//! KDE Breeze-inspired look (Plasma 6 colour schemes), dark and light.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme, Vec2};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use std::sync::Arc;

pub static NOTO_REGULAR: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");
pub static NOTO_MEDIUM: &[u8] = include_bytes!("../assets/NotoSans-Medium.ttf");

#[derive(Clone, Copy)]
pub struct Palette {
    /// Window background (panels)
    pub window: Color32,
    /// Toolbar / header area
    pub header: Color32,
    /// View background (lists, viewports, text fields)
    pub view: Color32,
    pub view_alt: Color32,
    pub button: Color32,
    pub button_hover: Color32,
    pub fg: Color32,
    pub fg_muted: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub highlight: Color32,
    pub highlight_soft: Color32,
    pub negative: Color32,
    pub positive: Color32,
    pub neutral: Color32,
    pub canvas: Color32,
}

pub const DARK: Palette = Palette {
    window: Color32::from_rgb(0x20, 0x23, 0x26),
    header: Color32::from_rgb(0x29, 0x2c, 0x30),
    view: Color32::from_rgb(0x14, 0x16, 0x18),
    view_alt: Color32::from_rgb(0x1d, 0x1f, 0x22),
    button: Color32::from_rgb(0x29, 0x2c, 0x30),
    button_hover: Color32::from_rgb(0x33, 0x37, 0x3c),
    fg: Color32::from_rgb(0xfc, 0xfc, 0xfc),
    fg_muted: Color32::from_rgb(0xa1, 0xa9, 0xb1),
    border: Color32::from_rgb(0x3e, 0x42, 0x46),
    border_strong: Color32::from_rgb(0x55, 0x5a, 0x5f),
    highlight: Color32::from_rgb(0x3d, 0xae, 0xe9),
    highlight_soft: Color32::from_rgb(0x1e, 0x3f, 0x53),
    negative: Color32::from_rgb(0xda, 0x44, 0x53),
    positive: Color32::from_rgb(0x27, 0xae, 0x60),
    neutral: Color32::from_rgb(0xf6, 0x74, 0x00),
    canvas: Color32::from_rgb(0x10, 0x12, 0x14),
};

pub const LIGHT: Palette = Palette {
    window: Color32::from_rgb(0xef, 0xf0, 0xf1),
    header: Color32::from_rgb(0xde, 0xe0, 0xe2),
    view: Color32::from_rgb(0xff, 0xff, 0xff),
    view_alt: Color32::from_rgb(0xf7, 0xf7, 0xf7),
    button: Color32::from_rgb(0xfc, 0xfc, 0xfc),
    button_hover: Color32::from_rgb(0xe8, 0xf4, 0xfb),
    fg: Color32::from_rgb(0x23, 0x26, 0x29),
    fg_muted: Color32::from_rgb(0x70, 0x7d, 0x8a),
    border: Color32::from_rgb(0xc4, 0xc7, 0xca),
    border_strong: Color32::from_rgb(0xa8, 0xac, 0xb0),
    highlight: Color32::from_rgb(0x3d, 0xae, 0xe9),
    highlight_soft: Color32::from_rgb(0xd4, 0xec, 0xf9),
    negative: Color32::from_rgb(0xda, 0x44, 0x53),
    positive: Color32::from_rgb(0x27, 0xae, 0x60),
    neutral: Color32::from_rgb(0xf6, 0x74, 0x00),
    canvas: Color32::from_rgb(0xd8, 0xdb, 0xde),
};

static IS_DARK: AtomicBool = AtomicBool::new(true);
/// Current (dark, light) palettes. Replaced (and the old one leaked) only when
/// the desktop colours change, which is rare.
static PALS: AtomicPtr<(Palette, Palette)> = AtomicPtr::new(std::ptr::null_mut());
/// KDE colour scheme darkness: 0 = no scheme found, 1 = dark, 2 = light.
static SCHEME: AtomicU8 = AtomicU8::new(0);
/// Set by the watcher thread when kdeglobals changed.
static DIRTY: AtomicBool = AtomicBool::new(false);

/// (dark, light) palettes with the system colours applied.
fn palettes() -> &'static (Palette, Palette) {
    let ptr = PALS.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: pointers stored in PALS come from Box::leak and are never freed
        return unsafe { &*ptr };
    }
    let new = Box::into_raw(Box::new(build_palettes()));
    match PALS.compare_exchange(std::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => unsafe { &*new },
        Err(cur) => {
            // another thread won the race
            drop(unsafe { Box::from_raw(new) });
            unsafe { &*cur }
        }
    }
}

fn build_palettes() -> (Palette, Palette) {
    let kde = read_kde();
    let accent = accent_override().unwrap_or(kde.accent);
    let scheme = kde.scheme.map(|(pal, dark)| (accent.map(|a| with_accent(pal, a, dark)).unwrap_or(pal), dark));
    SCHEME.store(match scheme { Some((_, true)) => 1, Some((_, false)) => 2, None => 0 }, Ordering::Relaxed);
    let (mut d, mut l) = match accent {
        Some(a) => (with_accent(DARK, a, true), with_accent(LIGHT, a, false)),
        None => (DARK, LIGHT),
    };
    match scheme {
        Some((pal, true)) => d = pal,
        Some((pal, false)) => l = pal,
        None => {}
    }
    (d, l)
}

/// Dark or light according to the KDE colour scheme, if there is one.
pub fn scheme_is_dark() -> Option<bool> {
    match SCHEME.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Re-derive the accent-dependent colours (same mixes as the Breeze defaults).
fn with_accent(mut p: Palette, a: Color32, dark: bool) -> Palette {
    p.highlight = a;
    if dark {
        p.highlight_soft = lerp(p.view, a, 0.25);
    } else {
        p.highlight_soft = lerp(Color32::WHITE, a, 0.22);
        p.button_hover = lerp(Color32::WHITE, a, 0.12);
    }
    p
}

fn parse_kde_color(v: &str) -> Option<Color32> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
        return Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    let c: Vec<u8> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (c.len() >= 3).then(|| Color32::from_rgb(c[0], c[1], c[2]))
}

/// `GIFKIT_ACCENT=r,g,b` overrides the accent; `GIFKIT_ACCENT=default` keeps Breeze blue.
fn accent_override() -> Option<Option<Color32>> {
    std::env::var("GIFKIT_ACCENT").ok().map(|v| parse_kde_color(&v))
}

fn kdeglobals_path() -> Option<std::path::PathBuf> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("kdeglobals"))
}

#[derive(Default)]
struct Kde {
    /// custom accent, else the colour scheme's selection colour
    accent: Option<Color32>,
    /// palette from the colour scheme and whether it is dark
    scheme: Option<(Palette, bool)>,
}

/// KDE Plasma's colours from kdeglobals. Empty elsewhere.
fn read_kde() -> Kde {
    let Some(text) = kdeglobals_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Kde::default() };
    parse_kdeglobals(&text)
}

fn parse_kdeglobals(text: &str) -> Kde {
    let mut section = String::new();
    let mut vals: std::collections::HashMap<(String, String), Color32> = Default::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if let Some(c) = parse_kde_color(v) {
            vals.insert((section.clone(), k.trim().to_string()), c);
        }
    }
    let get = |sec: &str, key: &str| vals.get(&(format!("[{sec}]"), key.to_string())).copied();
    let accent = get("General", "AccentColor").or(get("Colors:Selection", "BackgroundNormal"));
    let scheme = (|| {
        let window = get("Colors:Window", "BackgroundNormal")?;
        let fg = get("Colors:Window", "ForegroundNormal")?;
        let view = get("Colors:View", "BackgroundNormal").unwrap_or(window);
        let button = get("Colors:Button", "BackgroundNormal").unwrap_or(window);
        let luma = |c: Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        let dark = luma(window) < 128.0;
        let base = if dark { DARK } else { LIGHT };
        Some((
            Palette {
                window,
                header: get("Colors:Header", "BackgroundNormal").unwrap_or(window),
                view,
                view_alt: get("Colors:View", "BackgroundAlternate").unwrap_or(lerp(view, window, 0.5)),
                button,
                button_hover: if dark { lerp(button, fg, 0.045) } else { base.button_hover },
                fg,
                fg_muted: get("Colors:Window", "ForegroundInactive").unwrap_or(lerp(window, fg, 0.6)),
                border: lerp(window, fg, if dark { 0.136 } else { 0.21 }),
                border_strong: lerp(window, fg, if dark { 0.24 } else { 0.35 }),
                highlight: get("Colors:Selection", "BackgroundNormal").unwrap_or(base.highlight),
                highlight_soft: base.highlight_soft,
                negative: get("Colors:Window", "ForegroundNegative").unwrap_or(base.negative),
                positive: get("Colors:Window", "ForegroundPositive").unwrap_or(base.positive),
                neutral: get("Colors:Window", "ForegroundNeutral").unwrap_or(base.neutral),
                canvas: if dark { lerp(view, Color32::BLACK, 0.2) } else { lerp(window, fg, 0.1) },
            },
            dark,
        ))
    })();
    Kde { accent, scheme }
}

/// Watch kdeglobals so colour scheme and accent changes show up live.
pub fn watch(ctx: &egui::Context) {
    let Some(path) = kdeglobals_path() else { return };
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("kde-colours".into()).spawn(move || {
        let stamp = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut last = stamp(&path);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let now = stamp(&path);
            if now != last {
                last = now;
                DIRTY.store(true, Ordering::Release);
                ctx.request_repaint();
            }
        }
    });
}

/// Rebuild the palettes when the desktop colours changed. Call once per frame.
pub fn refresh(ctx: &egui::Context, choice: ThemeChoice) {
    if !DIRTY.swap(false, Ordering::AcqRel) {
        return;
    }
    let new = Box::leak(Box::new(build_palettes()));
    PALS.store(new, Ordering::Release);
    ctx.set_visuals_of(Theme::Dark, visuals(&new.0, true));
    ctx.set_visuals_of(Theme::Light, visuals(&new.1, false));
    choice.apply(ctx);
    ctx.request_repaint();
}

/// Palette of the theme currently in use (set every frame by the app).
pub fn p() -> &'static Palette {
    let (d, l) = palettes();
    if IS_DARK.load(Ordering::Relaxed) { d } else { l }
}

pub fn set_current(ctx: &egui::Context) {
    IS_DARK.store(ctx.theme() == Theme::Dark, Ordering::Relaxed);
}

/// Mix two colours, t = 0 → a, 1 → b.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Breeze frame outline: the base colour moved a little toward the text colour.
pub fn outline(pal: &Palette) -> Color32 {
    lerp(pal.button, pal.fg, if IS_DARK.load(Ordering::Relaxed) { 0.22 } else { 0.28 })
}

/// Breeze "checked" fill: the button colour tinted with the accent.
pub fn checked_fill(pal: &Palette) -> Color32 {
    lerp(pal.button, pal.highlight, 0.32)
}

/// Corner radius of buttons, fields and frames (Breeze uses 5 px in Plasma 6).
pub const RADIUS: u8 = 5;

fn visuals(pal: &Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(RADIUS);
    let edge = lerp(pal.button, pal.fg, if dark { 0.22 } else { 0.28 });
    let tint = lerp(pal.view, pal.highlight, if dark { 0.35 } else { 0.25 });
    v.panel_fill = pal.window;
    v.window_fill = pal.window;
    v.window_stroke = Stroke::new(1.0, pal.border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.extreme_bg_color = pal.view;
    v.text_edit_bg_color = Some(pal.view);
    v.faint_bg_color = pal.view_alt;
    v.code_bg_color = pal.view_alt;
    v.hyperlink_color = pal.highlight;
    v.warn_fg_color = pal.neutral;
    v.error_fg_color = pal.negative;
    // Breeze 6 selections are a soft accent tint. The stroke doubles as the
    // focus ring of text fields and the text of selected list items.
    v.selection.bg_fill = tint;
    v.selection.stroke = Stroke::new(1.0, lerp(pal.highlight, pal.fg, if dark { 0.35 } else { 0.0 }));
    v.window_shadow = Shadow { offset: [0, 3], blur: 14, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 35 }) };
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;
    v.indent_has_left_vline = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = pal.window;
    w.noninteractive.weak_bg_fill = pal.window;
    w.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, pal.fg);
    w.noninteractive.corner_radius = r;

    // bg_fill: checkbox and radio interiors, slider groove and handle
    w.inactive.bg_fill = pal.button;
    w.inactive.weak_bg_fill = pal.button;
    w.inactive.bg_stroke = Stroke::new(1.0, edge);
    w.inactive.fg_stroke = Stroke::new(1.0, pal.fg);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = pal.button;
    w.hovered.weak_bg_fill = lerp(pal.button, pal.highlight, 0.10);
    w.hovered.bg_stroke = Stroke::new(1.0, pal.highlight);
    w.hovered.fg_stroke = Stroke::new(1.0, pal.fg);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = lerp(pal.button, pal.highlight, 0.25);
    w.active.weak_bg_fill = lerp(pal.button, pal.highlight, 0.32);
    w.active.bg_stroke = Stroke::new(1.0, pal.highlight);
    w.active.fg_stroke = Stroke::new(1.0, pal.fg);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = pal.button;
    w.open.weak_bg_fill = lerp(pal.button, pal.highlight, 0.18);
    w.open.bg_stroke = Stroke::new(1.0, pal.highlight);
    w.open.fg_stroke = Stroke::new(1.0, pal.fg);
    w.open.corner_radius = r;
    v
}

pub fn install(ctx: &egui::Context) {
    // Fonts: Noto Sans (Plasma's default UI font), DejaVu fallback for symbols.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("noto".into(), Arc::new(egui::FontData::from_static(NOTO_REGULAR)));
    fonts.font_data.insert("noto-medium".into(), Arc::new(egui::FontData::from_static(NOTO_MEDIUM)));
    fonts.font_data.insert("dejavu".into(), Arc::new(egui::FontData::from_static(crate::text::SANS_BOLD)));
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "noto".into());
    prop.push("dejavu".into());
    fonts.families.entry(FontFamily::Monospace).or_default().push("dejavu".into());
    let mut medium = vec!["noto-medium".to_string(), "noto".to_string()];
    medium.extend(fonts.families[&FontFamily::Proportional].iter().skip(1).cloned());
    fonts.families.insert(FontFamily::Name("medium".into()), medium);
    // built-in text fonts, so the font picker can show each in its own face
    fonts.font_data.insert("dejavu-mono".into(), Arc::new(egui::FontData::from_static(crate::text::MONO_BOLD)));
    fonts.families.insert(FontFamily::Name("gf-sans-bold".into()), vec!["dejavu".into(), "noto".into()]);
    fonts.families.insert(FontFamily::Name("gf-mono-bold".into()), vec!["dejavu-mono".into(), "dejavu".into(), "noto".into()]);
    ctx.set_fonts(fonts);

    let (d, l) = palettes();
    ctx.set_visuals_of(Theme::Dark, visuals(d, true));
    ctx.set_visuals_of(Theme::Light, visuals(l, false));
    ctx.all_styles_mut(|s| {
        // Kirigami units: 4 px inside a group, 6 px between controls, 8 px between groups
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 3.0);
        // value boxes keep one width as their numbers grow
        s.spacing.interact_size = Vec2::new(72.0, 26.0);
        s.spacing.slider_width = 150.0;
        s.spacing.slider_rail_height = 4.0;
        s.spacing.menu_margin = egui::Margin::same(4);
        s.spacing.combo_width = 120.0;
        s.spacing.icon_width = 16.0;
        s.spacing.icon_width_inner = 10.0;
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Name("medium".into()))),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

// ---------------------------------------------------------------- settings file

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
        }
    }
    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            // follow the KDE colour scheme when there is one
            ThemeChoice::System => match scheme_is_dark() {
                Some(true) => egui::ThemePreference::Dark,
                Some(false) => egui::ThemePreference::Light,
                None => egui::ThemePreference::System,
            },
            ThemeChoice::Dark => egui::ThemePreference::Dark,
            ThemeChoice::Light => egui::ThemePreference::Light,
        });
    }
}

pub fn config_path() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    }?;
    let dir = base.join("gifkit");
    migrate_old_config(&base, &dir);
    Some(dir.join("settings.conf"))
}

/// The app used to be called GifForge: carry its settings and presets over once.
fn migrate_old_config(base: &std::path::Path, dir: &std::path::Path) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let old = base.join("gifforge");
        if dir.exists() || !old.is_dir() {
            return;
        }
        if std::fs::rename(&old, dir).is_err() {
            // different file system or locked: copy instead
            let _ = copy_dir(&old, dir);
        }
    });
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

fn read_settings() -> Vec<(String, String)> {
    let s = config_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    s.lines().filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect()
}

pub fn get_setting(key: &str) -> Option<String> {
    read_settings().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

pub fn set_setting(key: &str, val: &str) {
    let mut all = read_settings();
    all.retain(|(k, _)| k != key);
    all.push((key.to_string(), val.to_string()));
    if let Some(p) = config_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let body: String = all.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        let _ = std::fs::write(p, body);
    }
}

pub fn load_theme_choice() -> ThemeChoice {
    match get_setting("theme").as_deref() {
        Some("dark") => ThemeChoice::Dark,
        Some("light") => ThemeChoice::Light,
        _ => ThemeChoice::System,
    }
}

pub fn save_theme_choice(c: ThemeChoice) {
    let v = match c {
        ThemeChoice::System => "system",
        ThemeChoice::Dark => "dark",
        ThemeChoice::Light => "light",
    };
    set_setting("theme", v);
}

#[cfg(test)]
pub fn parse_kdeglobals_for_test(text: &str) -> (Option<Color32>, Option<(Palette, bool)>) {
    let k = parse_kdeglobals(text);
    (k.accent, k.scheme)
}
