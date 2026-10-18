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
    "application-exit", "archive-insert", "arrow-down", "arrow-right", "collapse-all", "color-picker", "expand-all", "go-next", "bookmarks", "configure-shortcuts", "dialog-ok-apply", "document-export",
    "document-new", "document-open", "document-open-recent", "document-save", "document-save-as", "document-sign",
    "draw-freehand", "draw-text", "edit-clear-all", "edit-cut", "edit-delete", "edit-duplicate", "edit-redo",
    "edit-rename", "edit-undo", "format-fill-color", "go-bottom", "go-down", "go-top", "go-up", "grid-rectangular",
    "help-about", "insert-image", "kdenlive-show-video", "link", "list-add",
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
    let key = (name.to_string(), px, col.to_array());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().ok()?.get(&key) {
        return Some(t.clone());
    }
    let (_, s16, s22) = ICONS.iter().find(|i| i.0 == name)?;
    // the 16 px design is sharper at small sizes
    let src = if size <= 19.0 { s16 } else { s22 };
    let img = render(&recolour(src, col), px)?;
    let t = ctx.load_texture(format!("icon-{name}-{px}"), img, TextureOptions::LINEAR);
    cache.lock().ok()?.insert(key, t.clone());
    Some(t)
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
