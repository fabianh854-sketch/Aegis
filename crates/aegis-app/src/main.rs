#![windows_subsystem = "windows"]

mod app;
mod clipboard;
mod generator;
mod settings;
mod task;
mod theme;
mod updates;

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
            .with_transparent(true)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Aegis",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(AegisApp::new()))
        }),
    )
}

/// Fenster- und Taskleisten-Icon, eingebettet aus `assets/icon/aegis-256.png` (erzeugt aus `aegis.svg`).
fn app_icon() -> egui::IconData {
    // Die Datei ist fest eingebettet; scheitert das Dekodieren, ist der Build kaputt, nicht die Laufzeit.
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/aegis-256.png"))
        .expect("eingebettetes App-Icon ist ein gültiges PNG")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_256_square_rgba() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
