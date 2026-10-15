#![windows_subsystem = "windows"]

mod app;

use app::AegisApp;
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Aegis",
        options,
        Box::new(|_cc| Ok(Box::new(AegisApp::default()))),
    )
}
