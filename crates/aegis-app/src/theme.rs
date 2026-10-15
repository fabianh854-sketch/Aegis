//! Farben, Schriften und Stil aus dem Claude-Design-Entwurf (`design/Aegis Screens.dc.html`).
//! Die Farbwerte sind aus den oklch-Angaben des Entwurfs nach sRGB umgerechnet.

use eframe::egui::{
    self, Color32, FontFamily, FontId, Margin, RichText, Rounding, Stroke, TextStyle,
};

pub const PANEL: Color32 = Color32::from_rgb(0x0c, 0x0d, 0x0f); // oklch(0.16 …) Fenster, Seitenleiste
pub const SURFACE: Color32 = Color32::from_rgb(0x14, 0x16, 0x19); // oklch(0.20 …) Felder, Listen
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x1e, 0x21, 0x24); // oklch(0.245 …) Auswahl, Karten
pub const BORDER: Color32 = Color32::from_rgb(0x30, 0x33, 0x37);
pub const TEXT: Color32 = Color32::from_rgb(0xf0, 0xf2, 0xf4);
pub const MUTED: Color32 = Color32::from_rgb(0xa7, 0xab, 0xb1);
pub const ACCENT: Color32 = Color32::from_rgb(0x61, 0xda, 0x92);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x0a, 0x14, 0x0e);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x94, 0x8c);

/// Schriftfamilie für Überschriften (Source Serif 4).
pub const SERIF: &str = "serif";
/// IBM Plex Sans Medium (Gewicht 500), für Buttons und Eintragstitel.
pub const SANS_MEDIUM: &str = "sans_medium";
/// IBM Plex Mono Medium, für das erzeugte Passwort.
pub const MONO_MEDIUM: &str = "mono_medium";

pub fn serif() -> FontFamily {
    FontFamily::Name(SERIF.into())
}

pub fn sans_medium() -> FontFamily {
    FontFamily::Name(SANS_MEDIUM.into())
}

pub fn mono_medium() -> FontFamily {
    FontFamily::Name(MONO_MEDIUM.into())
}

/// Bindet die Schriften aus `assets/fonts` ein. Die Dateien sind per `include_bytes!` im Programm.
/// Lizenz: SIL Open Font License 1.1 (siehe `assets/fonts/OFL-*.txt`).
pub fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions};

    let mut fonts = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 6] = [
        ("plex_sans", include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
        ("plex_sans_medium", include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
        ("plex_mono", include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        ("plex_mono_medium", include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
        ("source_serif", include_bytes!("../assets/fonts/SourceSerif4-Regular.ttf")),
        ("source_serif_semibold", include_bytes!("../assets/fonts/SourceSerif4-Semibold.ttf")),
    ];
    for (name, bytes) in files {
        fonts.font_data.insert(name.to_owned(), FontData::from_static(bytes));
    }

    // Standardschrift und Monospace zuerst auf IBM Plex, die egui-Schriften bleiben als Ausweichliste.
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.insert(0, "plex_sans".to_owned());
    }
    if let Some(list) = fonts.families.get_mut(&FontFamily::Monospace) {
        list.insert(0, "plex_mono".to_owned());
    }
    fonts.families.insert(serif(), vec!["source_serif".to_owned(), "plex_sans".to_owned()]);
    fonts.families.insert(sans_medium(), vec!["plex_sans_medium".to_owned(), "plex_sans".to_owned()]);
    fonts.families.insert(mono_medium(), vec!["plex_mono_medium".to_owned(), "plex_mono".to_owned()]);
    ctx.set_fonts(fonts);
}

/// Setzt das dunkle Farbschema, die Schriftgrößen und Abstände passend zum Entwurf.
pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
    v.selection.stroke = Stroke::new(1.0, ACCENT); // Rahmen eines fokussierten Eingabefelds
    widget(&mut v.widgets.noninteractive, PANEL);
    widget(&mut v.widgets.inactive, SURFACE);
    widget(&mut v.widgets.hovered, SURFACE_HI);
    widget(&mut v.widgets.active, SURFACE_HI);
    widget(&mut v.widgets.open, SURFACE);
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(13.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Heading, FontId::new(28.0, serif())),
        ]
        .into();
    });
}

fn widget(w: &mut egui::style::WidgetVisuals, fill: Color32) {
    w.bg_fill = fill;
    w.weak_bg_fill = fill;
    w.bg_stroke = Stroke::new(1.0, BORDER);
    w.rounding = Rounding::same(8.0);
    w.fg_stroke = Stroke::new(1.0, TEXT);
}

// ---- Text ----------------------------------------------------------------

/// Überschrift in Source Serif 4.
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).size(size).family(serif())
}

/// Text in IBM Plex Sans Medium.
pub fn medium(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).size(size).family(sans_medium())
}

/// Gedämpfter Text, z. B. Feldbeschriftungen.
pub fn muted(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).size(size).color(MUTED)
}

// ---- Buttons -------------------------------------------------------------

/// Primärer Button in Akzentgrün (z. B. „Speichern“), 36 px hoch.
pub fn accent_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(medium(text, 14.0).color(ACCENT_TEXT))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(egui::vec2(0.0, 36.0))
}

/// Sekundärer Button mit Rahmen (z. B. „Abbrechen“), 36 px hoch.
pub fn secondary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(14.0).color(TEXT))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, BORDER))
        .min_size(egui::vec2(0.0, 36.0))
}

/// Textlink ohne Rahmen (z. B. „Kopieren“, „Löschen“).
pub fn link_button(text: impl Into<String>, color: Color32, size: f32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(size).color(color))
        .frame(false)
        .small()
}

// ---- Flächen und Felder --------------------------------------------------

/// Fläche ohne Rahmen mit Radius 12 (Generator-Karte, Verschlüsselungs-Hinweis).
pub fn plain_card(fill: Color32, margin: Margin) -> egui::Frame {
    egui::Frame::none().fill(fill).rounding(Rounding::same(12.0)).inner_margin(margin)
}

/// Kleine Karte mit Rahmen, z. B. für das Formular „Master-Passwort ändern“.
pub fn card() -> egui::Frame {
    egui::Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::same(16.0))
}

/// Einzeiliges Eingabefeld wie im Entwurf: 44 px hoch, 15 px Schrift, 14 px Innenabstand.
pub fn input(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .font(FontId::proportional(15.0))
        .margin(Margin::symmetric(14.0, 12.0))
        .desired_width(f32::INFINITY)
}
