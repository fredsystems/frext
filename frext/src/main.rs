//! frext binary entry point.

use eframe::egui;
use frext::{app::FrextApp, persistence::Store};

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Any command-line arguments after the binary name are treated as file
    // paths to open on launch.
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();

    let store = match Store::new() {
        Ok(store) => store,
        Err(err) => {
            log::error!("failed to initialise persistence store: {err}");
            return Err(eframe::Error::AppCreation(Box::new(err)));
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("frext")
            .with_title("frext")
            .with_inner_size([900.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "frext",
        native_options,
        Box::new(move |cc| {
            frext::theme::apply(&cc.egui_ctx);
            Ok(Box::new(FrextApp::with_files(store, &files)))
        }),
    )
}
