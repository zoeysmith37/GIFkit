#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod anim;
mod app;
mod booth;
mod bundled;
mod effects;
mod encode;
mod ffmpeg;
mod geo;
mod gpu;
mod gpu_fx;
mod paste;
mod pixels;
mod gpu_render;
mod icons;
mod io;
mod keys;
mod lzw;
mod motion;
mod noai;
mod ops;
mod overlay;
mod presets;
mod project;
mod stack;
mod target;
mod shapes;
mod emoji;
mod sysfonts;
mod text;
mod timeline;
mod track;
mod interp;
mod rife;
mod update;
#[cfg(windows)]
mod winreg;
mod theme;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod fuzz;

fn app_icon() -> eframe::egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icon/gifkit-256.png")).map(|i| i.to_rgba8()).unwrap_or_else(|_| image::RgbaImage::new(1, 1));
    eframe::egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
}

fn main() -> eframe::Result {
    // `gifkit --thumbnail <project> <out.png> [size]`: for file managers
    // (a freedesktop thumbnailer); no window
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--version") {
        println!("GIFkit {}", update::VERSION);
        std::process::exit(0);
    }
    if args.get(1).is_some_and(|a| a == "--install-update") {
        match args.get(2).map(|a| update::install_file(std::path::Path::new(a))) {
            Some(Ok(())) => std::process::exit(0),
            Some(Err(e)) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
            None => std::process::exit(2),
        }
    }
    // files a Windows update left behind, now free to go
    update::clean_up();
    if args.get(1).is_some_and(|a| a == "--thumbnail") {
        let size = args.get(4).and_then(|s| s.to_str()).and_then(|s| s.parse().ok()).unwrap_or(256u32);
        let ok = match (args.get(2), args.get(3)) {
            (Some(i), Some(o)) => project::thumbnail(std::path::Path::new(i), size).and_then(|t| t.save_with_format(o, image::ImageFormat::Png).map_err(Into::into)),
            _ => Err(anyhow::anyhow!("usage: gifkit --thumbnail <project> <out.png> [size]")),
        };
        if let Err(e) = ok {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    // Windows: projects open with GIFkit and get Explorer thumbnails
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        if args.get(1).is_some_and(|a| a == "--register") {
            match winreg::register(&exe) {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        std::thread::spawn(move || {
            let _ = winreg::register(&exe);
            winreg::set_installed_version(update::VERSION);
        });
    }
    // glibc keeps big blocks freed by our many worker threads (frames,
    // mostly) instead of handing them back, hundreds of MB over a session.
    // A fixed threshold has blocks of a megabyte and up come straight from
    // the system and go straight back when freed. The rest is trimmed when
    // the app goes quiet (app.rs)
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if std::env::var_os("GIFKIT_NO_MALLOC_TUNE").is_none() {
        unsafe {
            libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
        }
    }
    // leave a core free for the compositor and the rest of the desktop
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let _ = rayon::ThreadPoolBuilder::new().num_threads(cores.saturating_sub(1).max(1)).build_global();
    // winit 0.30 (used by eframe) has no file drag-and-drop on native Wayland.
    // On Wayland sessions with XWayland available we therefore run through
    // XWayland, where drag-and-drop works. Opt out with GIFKIT_WAYLAND=1 or
    // the "native Wayland" setting.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let native = std::env::var_os("GIFKIT_WAYLAND").is_some() || theme::get_setting("wayland").as_deref() == Some("native");
        if !native && std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some() {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = |renderer: eframe::Renderer| eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("GIFkit")
            .with_app_id("gifkit")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        // Ctrl+C / X / V come in as keys: the app reads the clipboard itself
        // (files, browser pictures) and passes text on to text fields
        clipboard_shortcuts: false,
        renderer,
        ..Default::default()
    };
    // the window draws through wgpu (Vulkan, DirectX 12, Metal) so previews can
    // be made on the graphics card; OpenGL is the fallback for machines where
    // wgpu can't start. GIFKIT_RENDERER=glow forces OpenGL.
    let glow_only = std::env::var("GIFKIT_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("glow"));
    if !glow_only {
        let first = initial.clone();
        match eframe::run_native("GIFkit", options(eframe::Renderer::Wgpu), Box::new(move |cc| Ok(Box::new(app::GifKitApp::new(cc, first))))) {
            Err(e) => eprintln!("GIFkit: wgpu couldn't start ({e}), using OpenGL"),
            ok => return ok,
        }
    }
    eframe::run_native("GIFkit", options(eframe::Renderer::Glow), Box::new(move |cc| Ok(Box::new(app::GifKitApp::new(cc, initial)))))
}
