//! frext binary entry point.

use eframe::egui;
use frext::{app::FrextApp, persistence::Store};

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Command-line arguments after the binary name are paths to open. A
    // directory argument opens the file-tree sidebar rooted there; the
    // first directory wins. Everything else is treated as a file to open.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut dir: Option<std::path::PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        let path = std::path::PathBuf::from(arg);
        if path.is_dir() {
            if dir.is_none() {
                dir = Some(path);
            }
        } else {
            files.push(path);
        }
    }

    let store = match Store::new() {
        Ok(store) => store,
        Err(err) => {
            log::error!("failed to initialise persistence store: {err}");
            return Err(eframe::Error::AppCreation(Box::new(err)));
        }
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id("frext")
        .with_title("frext")
        .with_inner_size([900.0, 640.0]);
    if let Some(icon) = frext::icon::window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "frext",
        native_options,
        Box::new(move |cc| {
            frext::theme::apply(&cc.egui_ctx);
            Ok(Box::new(FrextApp::with_args(store, &files, dir.as_deref())))
        }),
    )
}
