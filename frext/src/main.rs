//! frext binary entry point.

use eframe::egui;
use frext::{app::FrextApp, persistence::Store};

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
        Box::new(|cc| {
            frext::theme::apply(&cc.egui_ctx);
            Ok(Box::new(FrextApp::new(store)))
        }),
    )
}
