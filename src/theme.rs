//! KDE Breeze-inspired look (Plasma 6 colour schemes), dark and light.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// (dark, light) palettes with the system accent colour applied.
fn palettes() -> &'static (Palette, Palette) {
    static P: std::sync::OnceLock<(Palette, Palette)> = std::sync::OnceLock::new();
    P.get_or_init(|| match system_accent() {
        Some(a) => (with_accent(DARK, a, true), with_accent(LIGHT, a, false)),
        None => (DARK, LIGHT),
    })
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

/// KDE Plasma's accent colour from kdeglobals (custom accent first, then the
/// colour scheme's selection colour). `GIFFORGE_ACCENT=r,g,b` overrides;
/// `GIFFORGE_ACCENT=default` keeps Breeze blue. None elsewhere.
fn system_accent() -> Option<Color32> {
    if let Ok(v) = std::env::var("GIFFORGE_ACCENT") {
        return parse_kde_color(&v);
    }
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    let text = std::fs::read_to_string(base.join("kdeglobals")).ok()?;
    let mut section = String::new();
    let (mut accent, mut selection) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match (section.as_str(), k.trim()) {
            ("[General]", "AccentColor") => accent = parse_kde_color(v),
            ("[Colors:Selection]", "BackgroundNormal") => selection = parse_kde_color(v),
            _ => {}
        }
    }
    accent.or(selection)
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

fn visuals(pal: &Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(3);
    v.panel_fill = pal.window;
    v.window_fill = pal.window;
    v.window_stroke = Stroke::new(1.0, pal.border);
    v.window_corner_radius = CornerRadius::same(5);
    v.menu_corner_radius = CornerRadius::same(4);
    v.extreme_bg_color = pal.view;
    v.text_edit_bg_color = Some(pal.view);
    v.faint_bg_color = pal.view_alt;
    v.code_bg_color = pal.view_alt;
    v.hyperlink_color = pal.highlight;
    v.warn_fg_color = pal.neutral;
    v.error_fg_color = pal.negative;
    v.selection.bg_fill = pal.highlight;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.window_shadow = Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 35 }) };
    v.slider_trailing_fill = true;
    v.striped = false;
    v.indent_has_left_vline = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = pal.window;
    w.noninteractive.weak_bg_fill = pal.window;
    w.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, pal.fg);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = pal.view; // checkbox / radio interior, slider rail
    w.inactive.weak_bg_fill = pal.button;
    w.inactive.bg_stroke = Stroke::new(1.0, pal.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, pal.fg);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = pal.view;
    w.hovered.weak_bg_fill = pal.button_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, pal.highlight);
    w.hovered.fg_stroke = Stroke::new(1.5, pal.fg);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = pal.highlight_soft;
    w.active.weak_bg_fill = pal.highlight_soft;
    w.active.bg_stroke = Stroke::new(1.0, pal.highlight);
    w.active.fg_stroke = Stroke::new(1.5, pal.fg);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = pal.view;
    w.open.weak_bg_fill = pal.button_hover;
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
        s.spacing.item_spacing = Vec2::new(6.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.spacing.interact_size = Vec2::new(40.0, 24.0);
        s.spacing.slider_width = 150.0;
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
            ThemeChoice::Dark => "Breeze Dark",
            ThemeChoice::Light => "Breeze Light",
        }
    }
    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            ThemeChoice::System => egui::ThemePreference::System,
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
    Some(base.join("gifforge").join("settings.conf"))
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
