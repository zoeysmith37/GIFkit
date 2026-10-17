#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod anim;
mod app;
mod effects;
mod encode;
mod geo;
mod io;
mod lzw;
mod ops;
mod overlay;
mod presets;
mod stack;
mod target;
mod sysfonts;
mod text;
mod theme;
#[cfg(test)]
mod tests;

fn app_icon() -> eframe::egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icon/gifforge-256.png")).map(|i| i.to_rgba8()).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
    eframe::egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

fn main() -> eframe::Result {
    // winit 0.30 (used by eframe) has no file drag-and-drop on native Wayland.
    // On Wayland sessions with XWayland available we therefore run through
    // XWayland, where drag-and-drop works. Opt out with GIFFORGE_WAYLAND=1 or
    // the "native Wayland" setting.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let native = std::env::var_os("GIFFORGE_WAYLAND").is_some() || theme::get_setting("wayland").as_deref() == Some("native");
        if !native && std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some() {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("GifForge – local GIF editor")
            .with_app_id("gifforge")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("GifForge", options, Box::new(move |cc| Ok(Box::new(app::GifForgeApp::new(cc, initial)))))
}
