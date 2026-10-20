//! Breeze monochrome icons from KDE (LGPL, see assets/BREEZE_ICONS_LICENSE.txt).
//! Each icon comes in its 16 px and 22 px designs. They are recoloured to the
//! current palette the way KDE's icon loader does it and cached as textures.

use eframe::egui::{self, Color32, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

macro_rules! icon_set {
    ($($n:literal),* $(,)?) => {
        &[$(($n, include_str!(concat!("../assets/icons/", $n, "-16.svg")), include_str!(concat!("../assets/icons/", $n, "-22.svg")))),*]
    };
}

static ICONS: &[(&str, &str, &str)] = icon_set!(
    "application-exit", "archive-insert", "dialog-cancel", "document-import", "object-flip-horizontal", "object-flip-vertical", "object-rotate-left", "arrow-down", "arrow-right", "collapse-all", "color-picker", "expand-all", "go-next", "bookmarks", "configure-shortcuts", "dialog-ok-apply", "document-export",
    "document-new", "document-open", "document-open-recent", "document-save", "document-save-as", "document-sign",
    "draw-freehand", "draw-text", "edit-clear-all", "edit-cut", "edit-delete", "edit-duplicate", "edit-redo",
    "edit-rename", "edit-undo", "format-fill-color", "go-bottom", "go-down", "go-top", "go-up", "grid-rectangular",
    "help-about", "help-contents", "insert-image", "kdenlive-show-video", "link", "list-add",
    "markasblank", "media-playback-pause", "media-playback-start", "media-playlist-repeat", "media-skip-backward",
    "media-skip-forward", "merge", "object-rotate-right", "pixelate", "player-time", "security-high", "shapes",
    "sidebar-collapse-left", "sidebar-collapse-right", "smiley", "speedometer", "tools-report-bug", "tools-wizard",
    "transform-crop", "transform-scale", "view-fullscreen", "view-grid", "view-history", "view-refresh",
    "view-split-left-right", "view-hidden", "view-visible", "window-close", "zoom-fit-best", "zoom-in", "zoom-original", "zoom-out",
);

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// The SVG with KDE's colour scheme classes set to our colours.
fn recolour(svg: &str, fg: Color32) -> String {
    let pal = crate::theme::p();
    let style = format!(
        ".ColorScheme-Text{{color:{t};}} .ColorScheme-Background{{color:{b};}} .ColorScheme-Highlight{{color:{h};}} .ColorScheme-Accent{{color:{h};}} .ColorScheme-HighlightedText{{color:#ffffff;}} .ColorScheme-NegativeText{{color:{n};}} .ColorScheme-PositiveText{{color:{ps};}} .ColorScheme-NeutralText{{color:{nt};}}",
        t = hex(fg),
        b = hex(pal.window),
        h = hex(pal.highlight),
        n = hex(pal.negative),
        ps = hex(pal.positive),
        nt = hex(pal.neutral),
    );
    let out = match (svg.find("id=\"current-color-scheme\""), svg.find("</style>")) {
        (Some(a), Some(b)) if b > a => {
            let open = a + svg[a..].find('>').unwrap_or(0) + 1;
            format!("{}{}{}", &svg[..open], style, &svg[b..])
        }
        _ => svg.to_string(),
    };
    // a few icons carry the default text colour inline
    out.replace("#232629", &hex(fg))
}

fn render(svg: &str, px: u32) -> Option<egui::ColorImage> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(px, px)?;
    let s = tree.size();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(px as f32 / s.width(), px as f32 / s.height()), &mut pm.as_mut());
    let rgba: Vec<u8> = pm.pixels().iter().flat_map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
    // tiny-skia pixels are premultiplied, as egui wants
    Some(egui::ColorImage::from_rgba_premultiplied([px as usize, px as usize], &rgba))
}

/// Texture of an icon drawn `size` points wide in `col`.
pub fn texture(ctx: &egui::Context, name: &str, size: f32, col: Color32) -> Option<TextureHandle> {
    static CACHE: OnceLock<Mutex<HashMap<(String, u32, [u8; 4]), TextureHandle>>> = OnceLock::new();
    let px = (size * ctx.pixels_per_point()).round().max(1.0) as u32;
    let theme = crate::theme::icon_theme().unwrap_or_default();
    let key = (format!("{theme}/{name}"), px, col.to_array());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().ok()?.get(&key) {
        return Some(t.clone());
    }
    let design = if size <= 19.0 { 16 } else { 22 };
    // the user's icon theme first (Linux), then the bundled Breeze set
    let img = match system_svg(&theme, name, design).and_then(|svg| render(&recolour(&svg, col), px)) {
        Some(img) => img,
        None => {
            let (_, s16, s22) = ICONS.iter().find(|i| i.0 == name)?;
            // the 16 px design is sharper at small sizes
            render(&recolour(if design == 16 { s16 } else { s22 }, col), px)?
        }
    };
    let t = ctx.load_texture(format!("icon-{name}-{px}"), img, TextureOptions::LINEAR);
    cache.lock().ok()?.insert(key, t.clone());
    Some(t)
}

/// An SVG icon from the user's icon theme, looked up the way the freedesktop
/// icon spec lays themes out (Breeze style and Papirus/Adwaita style folders).
fn system_svg(theme: &str, name: &str, size: u32) -> Option<String> {
    if theme.is_empty() || theme.eq_ignore_ascii_case("breeze") || theme.eq_ignore_ascii_case("breeze-dark") {
        // Breeze is what we bundle; the bundled copy matches what's installed
        return None;
    }
    static FOUND: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let key = format!("{theme}/{name}/{size}");
    let found = FOUND.get_or_init(Default::default);
    if let Some(v) = found.lock().ok()?.get(&key) {
        return v.clone();
    }
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))) {
        roots.push(h.join("icons"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(std::path::PathBuf::from(home).join(".icons"));
    }
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(dirs.split(':').filter(|d| !d.is_empty()).map(|d| std::path::PathBuf::from(d).join("icons")));
    let mut hit = None;
    'search: for root in &roots {
        let base = root.join(theme);
        if !base.is_dir() {
            continue;
        }
        for ctxdir in ["actions", "status", "devices", "places", "apps", "emblems"] {
            for sz in [size.to_string(), format!("{size}x{size}"), format!("{size}x{size}@1"), "symbolic".to_string(), "scalable".to_string()] {
                for file in [format!("{name}.svg"), format!("{name}-symbolic.svg")] {
                    for p in [base.join(ctxdir).join(&sz).join(&file), base.join(&sz).join(ctxdir).join(&file)] {
                        if let Ok(svg) = std::fs::read_to_string(&p) {
                            hit = Some(svg);
                            break 'search;
                        }
                    }
                }
            }
        }
    }
    found.lock().ok()?.insert(key, hit.clone());
    hit
}

/// An icon as an egui image, for buttons and menu entries.
pub fn image(ctx: &egui::Context, name: &str, size: f32, col: Color32) -> egui::Image<'static> {
    match texture(ctx, name, size, col) {
        Some(t) => egui::Image::from_texture((t.id(), egui::vec2(size, size))).fit_to_exact_size(egui::vec2(size, size)),
        None => egui::Image::from_texture((egui::TextureId::default(), egui::vec2(0.0, 0.0))),
    }
}

/// Paint an icon centred on `c`.
pub fn paint(painter: &egui::Painter, name: &str, c: egui::Pos2, size: f32, col: Color32) {
    if let Some(t) = texture(painter.ctx(), name, size, col) {
        // snap to whole pixels so the thin strokes stay crisp
        let ppp = painter.ctx().pixels_per_point();
        let min = ((c.to_vec2() - egui::vec2(size, size) / 2.0) * ppp).round() / ppp;
        let r = egui::Rect::from_min_size(min.to_pos2(), egui::vec2(size, size));
        painter.image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
}
