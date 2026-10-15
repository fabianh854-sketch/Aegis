#![windows_subsystem = "windows"]

mod app;
mod generator;
mod theme;

use app::AegisApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        // Größe und Rahmen wie im Entwurf: 880 × 560, eigene Titelleiste, runde Ecken.
        viewport: egui::ViewportBuilder::default()
            .with_title("Aegis")
            .with_inner_size([880.0, 560.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native(
        "Aegis",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(AegisApp::default()))
        }),
    )
}
