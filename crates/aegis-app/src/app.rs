use std::{fs, path::PathBuf, time::{Duration, Instant}};

use aegis_core::{AegisError, Entry, KdfParams, MasterChange, Session, Vault};
use eframe::egui::{
    self, Align, Align2, FontId, Layout, Margin, Pos2, Rect, RichText, Rounding, Sense, Stroke,
    UiBuilder, ViewportCommand,
};
use zeroize::{Zeroize, Zeroizing};

use aegis_update::UpdateStatus;

use crate::{
    clipboard::{ClipboardGuard, SystemClipboard},
    generator, settings,
    task::Task,
    theme,
    updates::{self, Banner},
};

const AUTO_LOCK_AFTER: Duration = Duration::from_secs(5 * 60);
const CLIPBOARD_CLEAR_AFTER: Duration = Duration::from_secs(30);
const STATUS_VISIBLE_FOR: Duration = Duration::from_secs(4);
const TITLE_BAR_HEIGHT: f32 = 40.0;
const BANNER_HEIGHT: f32 = 36.0;
/// Wie oft die Statusdatei des Updaters nach dem Start gelesen wird.
const UPDATE_POLL_EVERY: Duration = Duration::from_secs(2);
/// So lange wird höchstens auf das Ergebnis der Update-Prüfung gewartet.
const UPDATE_POLL_FOR: Duration = Duration::from_secs(2 * 60);
const SIDEBAR_OPEN_WIDTH: f32 = 220.0;
const SIDEBAR_CLOSED_WIDTH: f32 = 56.0;
/// Mindestlänge für neue oder geänderte Master-Passwörter (in Zeichen, nicht Bytes).
const MIN_MASTER_LEN: usize = 12;

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Entries,
    Generator,
    Settings,
}

/// Geöffneter Eintrag. `index` ist `None` für einen neuen Eintrag.
struct Editor {
    index: Option<usize>,
    entry: Entry,
    /// Stand beim Öffnen, um ungespeicherte Änderungen zu erkennen.
    original: Entry,
    reveal: bool,
    confirm_delete: bool,
    confirm_cancel: bool,
}

struct Settings {
    auto_lock: bool,
    clear_clipboard: bool,
}

/// Wartet auf das Ergebnis einer laufenden Update-Prüfung des Updaters.
struct UpdatePoll {
    /// Startzeit (Unix-Sekunden): nur Ergebnisse ab diesem Zeitpunkt zählen.
    started_at: u64,
    give_up_at: Instant,
    next_read: Instant,
    /// „Jetzt nach Updates suchen“: auch „kein Update“ und Fehler melden (die Startprüfung bleibt still).
    manual: bool,
}

/// Klick in der Hinweisleiste.
enum BannerAction {
    Install,
    Later,
    /// Installationsfehler gelesen und geschlossen.
    DismissInstallFailure,
}

pub struct AegisApp {
    /// Pfad zum Tresor oder Fehlertext, wenn er nicht bestimmt werden kann.
    path: Result<PathBuf, String>,
    // Entsperren
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    /// Läuft gerade Entsperren/Anlegen im Hintergrund (Argon2)?
    pending: Option<Task<Session>>,
    // Sitzung
    session: Option<Session>,
    nav: Nav,
    sidebar_open: bool,
    /// Mini-Schild für die Titelleiste, wird beim ersten Frame hochgeladen.
    mark: Option<egui::TextureHandle>,
    search: String,
    focus_search: bool,
    editor: Option<Editor>,
    status: String,
    status_error: bool,
    status_at: Instant,
    last_activity: Instant,
    clipboard: ClipboardGuard<SystemClipboard>,
    // Passwortgenerator
    gen_opts: generator::Options,
    gen_password: Zeroizing<String>,
    // Einstellungen
    settings: Settings,
    /// Dauerhafte Einstellungen (`settings.json`).
    persisted: settings::Persisted,
    show_master_form: bool,
    current_master: Zeroizing<String>,
    new_master: Zeroizing<String>,
    new_master_confirm: Zeroizing<String>,
    // Updates
    updater: Option<PathBuf>,
    update_poll: Option<UpdatePoll>,
    update_status: Option<UpdateStatus>,
    /// „Später“ gilt nur für diese Sitzung der App.
    update_dismissed: bool,
    /// Fehler der letzten Installation (beim Start einmal aus `update.json` gelesen), bis er geschlossen wird.
    install_failure: Option<String>,
}

impl Default for AegisApp {
    fn default() -> Self {
        // Kein Rückfall auf das aktuelle Verzeichnis: der Tresor soll nie an einem zufälligen Ort landen.
        let path = std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(|base| PathBuf::from(base).join("Aegis").join("vault.aegis"))
            .ok_or_else(|| "APPDATA nicht gesetzt – Speicherort unbekannt".to_string());
        let gen_opts = generator::Options::default();
        let gen_password = Zeroizing::new(generator::generate(&gen_opts).unwrap_or_default());
        Self {
            path,
            password: Zeroizing::default(),
            confirm: Zeroizing::default(),
            pending: None,
            session: None,
            nav: Nav::Entries,
            sidebar_open: false, // Standard: eingeklappt, nur Symbole
            mark: None,
            search: String::new(),
            focus_search: false,
            editor: None,
            status: String::new(),
            status_error: false,
            status_at: Instant::now(),
            last_activity: Instant::now(),
            clipboard: ClipboardGuard::new(SystemClipboard),
            gen_opts,
            gen_password,
            settings: Settings { auto_lock: true, clear_clipboard: true },
            persisted: settings::Persisted::default(),
            show_master_form: false,
            current_master: Zeroizing::default(),
            new_master: Zeroizing::default(),
            new_master_confirm: Zeroizing::default(),
            updater: None,
            update_poll: None,
            update_status: None,
            update_dismissed: false,
            install_failure: None,
        }
    }
}

impl AegisApp {
    /// Startet die App: lädt die Einstellungen und sucht bei Bedarf im Hintergrund nach Updates.
    pub fn new() -> Self {
        let mut app = Self::default();
        if let Some(path) = settings::path() {
            app.persisted = settings::load(&path);
        }
        app.updater = updates::updater_path();
        // Genau einmal und vor der Startprüfung lesen: Diese überschreibt update.json gleich.
        let last = aegis_update::status_path().and_then(|path| aegis_update::read_status(&path));
        app.install_failure = updates::install_failure(last.as_ref(), aegis_update::now());
        if app.persisted.check_updates_on_start {
            // Ein Fehler beim Start der Prüfung ist hier kein Grund zu stören; „Jetzt suchen“ zeigt ihn.
            let _ = app.start_update_check(false);
        }
        app
    }

    /// Startet `--check` und wartet danach auf ein frisches Ergebnis in `update.json`.
    fn start_update_check(&mut self, manual: bool) -> std::io::Result<()> {
        let updater = self
            .updater
            .clone()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Updater nicht gefunden"))?;
        let started_at = aegis_update::now();
        updates::spawn_check(&updater)?;
        let now = Instant::now();
        self.update_poll = Some(UpdatePoll {
            started_at,
            give_up_at: now + UPDATE_POLL_FOR,
            next_read: now + UPDATE_POLL_EVERY,
            manual,
        });
        Ok(())
    }

    /// Liest alle 2 s die Statusdatei, bis ein Ergebnis dieser Prüfung da ist (höchstens 2 Minuten).
    fn poll_update_status(&mut self, ctx: &egui::Context, now: Instant) {
        let Some(poll) = &mut self.update_poll else { return };
        if now >= poll.next_read {
            poll.next_read = now + UPDATE_POLL_EVERY;
            let fresh = aegis_update::status_path()
                .and_then(|path| aegis_update::read_status(&path))
                .filter(|status| updates::is_fresh(status, poll.started_at));
            if let Some(status) = fresh {
                let manual = poll.manual;
                self.update_poll = None;
                if manual {
                    match updates::check_notice(&status) {
                        Some((text, error)) => self.set_status(text, error),
                        // Ausdrücklich gesucht: ein früheres „Später“ gilt dann nicht mehr.
                        None => self.update_dismissed = false,
                    }
                }
                self.update_status = Some(status);
                return;
            }
        }
        if now >= poll.give_up_at {
            let manual = poll.manual;
            self.update_poll = None;
            if manual {
                self.set_status(updates::CHECK_TIMEOUT_NOTICE, true);
            }
        } else {
            ctx.request_repaint_after(poll.next_read.saturating_duration_since(now));
        }
    }

    /// „Aktualisieren“: sperrt, startet den Updater und schließt die App, damit er sie ersetzen kann.
    fn install_update(&mut self, ctx: &egui::Context) {
        let Some(updater) = self.updater.clone() else {
            self.set_status("Updater konnte nicht gestartet werden: nicht gefunden", true);
            return;
        };
        if self.session.is_some() {
            self.lock(); // leert auch die Zwischenablage, falls so eingestellt
        }
        match updates::spawn_install(&updater) {
            Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
            Err(e) => self.set_status(format!("Updater konnte nicht gestartet werden: {e}"), true),
        }
    }
}

impl eframe::App for AegisApp {
    /// Transparent, damit die runden Fensterecken des Entwurfs sichtbar sind.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        if ctx.input(|i| !i.events.is_empty()) {
            self.last_activity = now;
        }

        if self.session.is_some() {
            if self.settings.auto_lock && now.duration_since(self.last_activity) >= AUTO_LOCK_AFTER {
                self.lock();
                self.set_status("Automatisch gesperrt", false);
            } else {
                ctx.request_repaint_after(Duration::from_secs(10));
            }
        }

        // Hinweise wie „Gespeichert“ verschwinden nach ein paar Sekunden, Fehler bleiben stehen.
        if !self.status.is_empty() && !self.status_error && self.session.is_some() {
            let shown = now.duration_since(self.status_at);
            if shown >= STATUS_VISIBLE_FOR {
                self.status.clear();
            } else {
                ctx.request_repaint_after(STATUS_VISIBLE_FOR - shown);
            }
        }

        if let Some(next) = self.clipboard.tick(now) {
            ctx.request_repaint_after(next);
        }

        self.poll_update_status(ctx, now);

        if let Some(task) = &self.pending {
            match task.poll() {
                Some(result) => {
                    self.pending = None;
                    self.finish_unlock(result);
                }
                None => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }

        // Ein Fenster ohne Windows-Rahmen: eigener Rand mit Radius 12 und eigene Titelleiste.
        let window = egui::Frame::none()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .rounding(Rounding::same(12.0));
        let mark = self
            .mark
            .get_or_insert_with(|| ctx.load_texture("aegis-mark", title_mark_image(), egui::TextureOptions::LINEAR))
            .id();
        egui::CentralPanel::default().frame(window).show(ctx, |ui| {
            let app_rect = ui.max_rect();
            let title_rect = Rect::from_min_size(app_rect.min, egui::vec2(app_rect.width(), TITLE_BAR_HEIGHT));
            let unlocked = self.session.is_some();
            let status = if unlocked { self.status.as_str() } else { "" };
            // Der Seitenleisten-Knopf erscheint nur dort, wo es eine Seitenleiste gibt.
            let toggle = (unlocked && self.editor.is_none()).then_some(self.sidebar_open);
            if title_bar(ui, title_rect, mark, status, self.status_error, toggle) {
                self.sidebar_open = !self.sidebar_open;
            }

            // Hinweisleiste direkt unter der Titelleiste; der Inhalt rückt entsprechend nach unten.
            let banner = updates::select_banner(
                self.install_failure.as_deref(),
                self.update_status.as_ref(),
                self.update_dismissed,
            );
            let mut content_top = title_rect.bottom();
            let mut banner_action = None;
            if banner != Banner::Hidden {
                let banner_rect = Rect::from_min_size(
                    egui::pos2(app_rect.left(), content_top),
                    egui::vec2(app_rect.width(), BANNER_HEIGHT),
                );
                banner_action = banner_bar(ui, banner_rect, &banner);
                content_top += BANNER_HEIGHT;
            }

            let content = Rect::from_min_max(egui::pos2(app_rect.left(), content_top), app_rect.max);
            let mut content_ui = ui.new_child(UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
            if unlocked {
                self.main_view(&mut content_ui);
            } else {
                self.unlock_view(&mut content_ui);
            }

            match banner_action {
                Some(BannerAction::Install) => self.install_update(ctx),
                Some(BannerAction::Later) => self.update_dismissed = true,
                Some(BannerAction::DismissInstallFailure) => self.install_failure = None,
                None => {}
            }
        });
    }

    /// Beim Beenden kein Passwort in der Zwischenablage zurücklassen.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.settings.clear_clipboard {
            self.clipboard.clear_now();
        }
    }
}

impl AegisApp {
    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_error = error;
        self.status_at = Instant::now();
    }

    fn lock(&mut self) {
        if self.settings.clear_clipboard {
            self.clipboard.clear_now();
        }
        self.session = None; // Schlüssel und Einträge werden beim Drop genullt
        self.editor = None;
        self.password.zeroize();
        self.confirm.zeroize();
        self.search.clear();
        self.current_master.zeroize();
        self.new_master.zeroize();
        self.new_master_confirm.zeroize();
        self.show_master_form = false;
        self.status.clear();
        self.nav = Nav::Entries;
    }

    // ---- Entsperren (Entwurf 01) -------------------------------------------

    fn unlock_view(&mut self, ui: &mut egui::Ui) {
        // Ersteinrichtung genau dann, wenn es noch keine Tresordatei gibt.
        let first_start = self.path.as_ref().is_ok_and(|p| !p.exists());
        let busy = self.pending.is_some();
        let texts = unlock_texts(first_start, busy);

        // Spalte 320 px breit, senkrecht und waagerecht mittig.
        let height = if first_start { 410.0 } else { 310.0 };
        let column = Rect::from_center_size(ui.max_rect().center(), egui::vec2(320.0, height));
        let mut ui = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
        ui.spacing_mut().item_spacing.y = 8.0;

        ui.vertical_centered(|ui| {
            // Der längere Titel der Ersteinrichtung passt sonst nicht in die 320 px.
            ui.label(theme::heading(texts.title, if first_start { 27.0 } else { 34.0 }));
        });
        ui.add_space(12.0);
        let intro = if first_start {
            "Willkommen bei Aegis. Dieses Passwort öffnet deinen Tresor und lässt sich nicht wiederherstellen."
        } else {
            "Dein Master-Passwort öffnet den Tresor. Er bleibt lokal auf diesem Gerät."
        };
        ui.label(RichText::new(intro).size(14.0).color(theme::MUTED));
        ui.add_space(16.0);

        let label = if first_start { "Master-Passwort (mind. 12 Zeichen)" } else { "Master-Passwort" };
        ui.label(theme::muted(label, 13.0));
        let first = ui.add_enabled(!busy, password_input(&mut self.password));
        let mut enter = first.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if first_start {
            ui.add_space(4.0);
            ui.label(theme::muted("Passwort bestätigen", 13.0));
            let second = ui.add_enabled(!busy, password_input(&mut self.confirm));
            enter |= second.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        }
        ui.add_space(12.0);

        let button = egui::Button::new(theme::medium(texts.button, 15.0).color(theme::ACCENT_TEXT))
            .fill(theme::ACCENT)
            .stroke(Stroke::NONE)
            .min_size(egui::vec2(320.0, 44.0));
        // Text mittig: egui richtet Button-Text nach dem Layout aus.
        let response = ui.vertical_centered_justified(|ui| ui.add_enabled(!busy, button)).inner;
        if busy {
            // Drehender Kreis links im Button, solange Argon2 im Hintergrund rechnet.
            let spot = Rect::from_center_size(
                egui::pos2(response.rect.left() + 26.0, response.rect.center().y),
                egui::vec2(18.0, 18.0),
            );
            egui::Spinner::new().size(18.0).color(theme::ACCENT_TEXT).paint_at(&ui, spot);
        }
        if (response.clicked() || enter) && !busy {
            let ctx = ui.ctx().clone();
            self.unlock(first_start, ctx);
        }
        ui.add_space(12.0);

        if let Err(problem) = &self.path {
            ui.add_space(4.0);
            ui.label(RichText::new(problem).size(13.0).color(theme::DANGER));
        } else if !self.status.is_empty() {
            ui.add_space(4.0);
            let color = if self.status_error { theme::DANGER } else { theme::MUTED };
            ui.label(RichText::new(&self.status).size(13.0).color(color));
        }
    }

    /// Prüft die Eingaben sofort und startet die Schlüsselableitung im Hintergrund.
    fn unlock(&mut self, first_start: bool, ctx: egui::Context) {
        if self.pending.is_some() {
            return; // kein zweiter Vorgang, solange einer läuft
        }
        let path = match &self.path {
            Ok(p) => p.clone(),
            Err(problem) => {
                let problem = problem.clone();
                self.set_status(problem, true);
                return;
            }
        };
        if self.password.is_empty() {
            self.set_status("Bitte Master-Passwort eingeben", true);
            return;
        }
        if first_start {
            if let Err(msg) = validate_new_master(&self.password, &self.confirm) {
                self.set_status(msg, true);
                return;
            }
            if let Some(dir) = path.parent() {
                if let Err(e) = fs::create_dir_all(dir) {
                    self.set_status(format!("Ordner konnte nicht angelegt werden: {e}"), true);
                    return;
                }
            }
        }

        let password = self.password.clone();
        self.status.clear();
        self.pending = Some(Task::spawn(
            move || {
                let result = if first_start {
                    Session::create(&path, password.as_bytes(), &KdfParams::default())
                } else {
                    Session::open(&path, password.as_bytes())
                };
                result.map_err(|e| e.to_string())
            },
            move || ctx.request_repaint(),
        ));
    }

    fn finish_unlock(&mut self, result: Result<Session, String>) {
        match result {
            Ok(session) => {
                self.session = Some(session);
                self.password.zeroize();
                self.confirm.zeroize();
                self.nav = Nav::Entries;
                self.last_activity = Instant::now();
                self.status.clear();
            }
            Err(e) => self.set_status(e, true),
        }
    }

    // ---- Hauptansicht ------------------------------------------------------

    fn main_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        // Der Eintrag (Entwurf 03) hat keine Seitenleiste, die Übersichten schon.
        let sidebar_width = match (self.editor.is_some(), self.sidebar_open) {
            (true, _) => 0.0,
            (false, true) => SIDEBAR_OPEN_WIDTH,
            (false, false) => SIDEBAR_CLOSED_WIDTH,
        };
        if sidebar_width > 0.0 {
            let side = Rect::from_min_size(rect.min, egui::vec2(sidebar_width, rect.height()));
            let mut side_ui = ui.new_child(UiBuilder::new().max_rect(side).layout(Layout::top_down(Align::Min)));
            self.sidebar(&mut side_ui);
        }

        let main = Rect::from_min_max(egui::pos2(rect.left() + sidebar_width, rect.top()), rect.max);
        let padding = if self.editor.is_none() && self.nav == Nav::Generator {
            egui::vec2(32.0, 22.0)
        } else {
            egui::vec2(32.0, 28.0)
        };
        let mut main_ui = ui.new_child(
            UiBuilder::new().max_rect(main.shrink2(padding)).layout(Layout::top_down(Align::Min)),
        );
        if self.session.is_none() {
            return;
        }
        if self.editor.is_some() {
            self.editor_view(&mut main_ui);
        } else {
            match self.nav {
                Nav::Entries => self.entries_view(&mut main_ui),
                Nav::Generator => self.generator_view(&mut main_ui),
                Nav::Settings => self.settings_view(&mut main_ui),
            }
        }
    }

    /// Seitenleiste (Entwurf 02 aufgeklappt, 02b eingeklappt).
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let open = self.sidebar_open;
        let outer = ui.max_rect();
        let inner = if open { outer.shrink2(egui::vec2(14.0, 20.0)) } else { outer.shrink2(egui::vec2(10.0, 20.0)) };
        let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));

        // Auf-/Zuklappen sitzt in der Titelleiste; die Navigation beginnt direkt oben.
        ui.spacing_mut().item_spacing.y = if open { 2.0 } else { 10.0 };
        if !open && nav_button(&mut ui, false, Icon::Search, "Suchen", open) {
            self.nav = Nav::Entries;
            self.focus_search = true;
        }
        let items = [
            (Nav::Entries, Icon::List, "Alle Einträge"),
            (Nav::Generator, Icon::Key, "Passwortgenerator"),
            (Nav::Settings, Icon::Gear, "Einstellungen"),
        ];
        for (nav, icon, label) in items {
            if nav_button(&mut ui, self.nav == nav, icon, label, open) {
                self.nav = nav;
            }
        }

        let lock_width = if open { inner.width() } else { 36.0 };
        let lock_rect = Rect::from_min_size(egui::pos2(inner.left(), inner.bottom() - 36.0), egui::vec2(lock_width, 36.0));
        let mut lock_ui = ui.new_child(UiBuilder::new().max_rect(lock_rect));
        if nav_button(&mut lock_ui, false, Icon::Door, "Sperren", open) {
            self.lock();
        }
    }

    // ---- Einträge (Entwurf 02) ---------------------------------------------

    fn entries_view(&mut self, ui: &mut egui::Ui) {
        let query = self.search.to_lowercase();
        let rows: Vec<(usize, String, String)> = match self.session.as_ref().map(Session::vault) {
            Some(vault) => vault
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    query.is_empty()
                        || e.title.to_lowercase().contains(&query)
                        || e.username.to_lowercase().contains(&query)
                })
                .map(|(i, e)| (i, e.title.clone(), e.username.clone()))
                .collect(),
            None => return,
        };

        let mut open_index = None;
        let mut new_entry = false;
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.label(theme::heading("Einträge", 28.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::accent_button("+ Neuer Eintrag")).clicked() {
                    new_entry = true;
                }
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(theme::muted("Suchen", 13.0))
                        .font(FontId::proportional(13.0))
                        .margin(Margin::symmetric(10.0, 9.0))
                        .desired_width(200.0),
                );
                if self.focus_search {
                    search.request_focus();
                    self.focus_search = false;
                }
            });
        });
        ui.add_space(12.0);

        if rows.is_empty() {
            let text = if self.search.is_empty() { "Noch keine Einträge" } else { "Keine Treffer" };
            ui.label(theme::muted(text, 14.0));
        } else {
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .rounding(Rounding::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let last = rows.len() - 1;
                        for (n, (index, title, username)) in rows.iter().enumerate() {
                            // Platzhalter für den Hover-Hintergrund, damit er hinter dem Inhalt liegt.
                            let background = ui.painter().add(egui::Shape::Noop);
                            let row = egui::Frame::none()
                                .inner_margin(Margin::symmetric(16.0, 14.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 14.0;
                                        avatar(ui, title);
                                        ui.vertical(|ui| {
                                            ui.spacing_mut().item_spacing.y = 2.0;
                                            ui.label(theme::medium(title.as_str(), 15.0));
                                            ui.label(theme::muted(username.as_str(), 13.0));
                                        });
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            let details = egui::Button::new(RichText::new("Details").size(13.0))
                                                .fill(egui::Color32::TRANSPARENT)
                                                .stroke(Stroke::new(1.0, theme::BORDER))
                                                .min_size(egui::vec2(0.0, 30.0));
                                            if ui.add(details).clicked() {
                                                open_index = Some(*index);
                                            }
                                        });
                                    });
                                })
                                .response;

                            let rect = row.rect;
                            if ui.rect_contains_pointer(rect) {
                                let top = if n == 0 { 11.0 } else { 0.0 };
                                let bottom = if n == last { 11.0 } else { 0.0 };
                                let rounding = Rounding { nw: top, ne: top, sw: bottom, se: bottom };
                                ui.painter().set(
                                    background,
                                    egui::epaint::RectShape::filled(rect, rounding, theme::SURFACE_HI),
                                );
                            }
                            if n < last {
                                ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::BORDER));
                            }
                        }
                    });
            });
        }

        if let Some(index) = open_index {
            self.open_editor(Some(index));
        }
        if new_entry {
            self.open_editor(None);
        }
    }

    fn open_editor(&mut self, index: Option<usize>) {
        let entry = match (index, &self.session) {
            (Some(i), Some(session)) => session.vault().entries[i].clone(),
            _ => Entry::default(),
        };
        self.editor = Some(Editor {
            index,
            original: entry.clone(),
            entry,
            reveal: false,
            confirm_delete: false,
            confirm_cancel: false,
        });
    }

    // ---- Eintrag (Entwurf 03) ----------------------------------------------

    fn editor_view(&mut self, ui: &mut egui::Ui) {
        let is_new = self.editor.as_ref().is_none_or(|e| e.index.is_none());
        // Überschrift ist der Eintragstitel, wie im Entwurf.
        let heading = match self.editor.as_ref() {
            Some(e) if !e.entry.title.trim().is_empty() => e.entry.title.clone(),
            _ => "Neuer Eintrag".to_string(),
        };
        let confirm_cancel = self.editor.as_ref().is_some_and(|e| e.confirm_cancel);
        let mut save = false;
        let mut cancel = false;
        let mut delete = false;
        let mut copy = false;

        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.label(theme::heading(heading, 28.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(theme::accent_button("Speichern")).clicked() {
                    save = true;
                }
                let label = if confirm_cancel { "Änderungen verwerfen?" } else { "Abbrechen" };
                if ui.add(theme::secondary_button(label)).clicked() {
                    cancel = true;
                }
            });
        });
        ui.add_space(14.0);

        let Some(editor) = self.editor.as_mut() else { return };

        // Titel und Benutzername nebeneinander, 20 px Abstand.
        let gap = 20.0;
        let column = (ui.available_width() - gap) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.vertical(|ui| {
                ui.set_width(column);
                ui.label(theme::muted("Titel", 13.0));
                ui.add(theme::input(&mut editor.entry.title));
            });
            ui.vertical(|ui| {
                ui.set_width(column);
                ui.label(theme::muted("Benutzername", 13.0));
                ui.add(theme::input(&mut editor.entry.username));
            });
        });
        ui.add_space(14.0);

        // Passwort: „Kopieren“ rechts über dem Feld, „Anzeigen“ im Feld.
        ui.horizontal(|ui| {
            ui.label(theme::muted("Passwort", 13.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                if ui.add(theme::link_button("Kopieren", theme::ACCENT, 13.0)).clicked() {
                    copy = true;
                }
            });
        });
        let password_id = egui::Id::new("entry_password");
        let focused = ui.memory(|m| m.has_focus(password_id));
        egui::Frame::none()
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, if focused { theme::ACCENT } else { theme::BORDER }))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let width = ui.available_width() - 70.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.entry.password)
                            .id(password_id)
                            .frame(false)
                            .margin(Margin::ZERO)
                            .password(!editor.reveal)
                            .font(FontId::monospace(15.0))
                            .desired_width(width),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if editor.reveal { "Verbergen" } else { "Anzeigen" };
                        if ui.add(theme::link_button(label, theme::TEXT, 13.0)).clicked() {
                            editor.reveal = !editor.reveal;
                        }
                    });
                });
            });
        ui.add_space(4.0);
        strength_bars(ui, &editor.entry.password, 44.0);
        ui.add_space(14.0);

        ui.label(theme::muted("Notizen", 13.0));
        let notes_height = (ui.available_height() - if is_new { 8.0 } else { 40.0 }).max(80.0);
        ui.add(
            egui::TextEdit::multiline(&mut editor.entry.notes)
                .font(FontId::proportional(14.0))
                .margin(Margin::symmetric(14.0, 12.0))
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, notes_height)),
        );

        if !is_new {
            ui.add_space(12.0);
            let label = if editor.confirm_delete { "Wirklich löschen?" } else { "Löschen" };
            if ui.add(theme::link_button(label, theme::DANGER, 14.0)).clicked() {
                if editor.confirm_delete {
                    delete = true;
                } else {
                    editor.confirm_delete = true;
                }
            }
        }

        // Abbrechen mit ungespeicherten Änderungen braucht einen zweiten Klick.
        if cancel {
            if editor.entry != editor.original && !editor.confirm_cancel {
                editor.confirm_cancel = true;
            } else {
                self.editor = None;
                self.status.clear();
            }
        }
        if copy {
            if let Some(editor) = self.editor.as_ref() {
                let secret = Zeroizing::new(editor.entry.password.clone());
                self.copy_secret(ui.ctx(), &secret);
            }
        }
        if save {
            self.save_editor();
        }
        if delete {
            self.delete_editor_entry();
        }
    }

    /// Schreibt den Eintrag auf einer Kopie des Vaults und übernimmt ihn erst nach erfolgreichem Speichern.
    fn save_editor(&mut self) {
        let Some(editor) = self.editor.as_ref() else { return };
        if editor.entry.title.trim().is_empty() {
            self.set_status("Bitte einen Titel eingeben", true);
            return;
        }
        let Some(session) = self.session.as_ref() else { return };
        let mut candidate = session.vault().clone();
        let entry = editor.entry.clone();
        match editor.index {
            Some(i) => candidate.entries[i] = entry,
            None => candidate.entries.push(entry),
        }
        if self.commit(candidate) {
            self.editor = None;
            self.set_status("Gespeichert", false);
        }
    }

    fn delete_editor_entry(&mut self) {
        let Some(index) = self.editor.as_ref().and_then(|e| e.index) else { return };
        let Some(session) = self.session.as_ref() else { return };
        let mut candidate = session.vault().clone();
        candidate.entries.remove(index);
        if self.commit(candidate) {
            self.editor = None;
            self.set_status("Eintrag gelöscht", false);
        }
    }

    /// Speichert `candidate` auf die Platte und macht es nur bei Erfolg zum aktuellen Vault.
    fn commit(&mut self, candidate: Vault) -> bool {
        let Some(session) = self.session.as_mut() else { return false };
        match session.commit(candidate) {
            Ok(()) => true,
            Err(e) => {
                self.set_status(format!("Speichern fehlgeschlagen: {e}"), true);
                false
            }
        }
    }

    // ---- Passwortgenerator (Entwurf 04) ------------------------------------

    /// Kopiert ein Geheimnis geschützt. Klappt das nicht, normal kopieren und darauf hinweisen.
    fn copy_secret(&mut self, ctx: &egui::Context, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let clear_after = self.settings.clear_clipboard.then_some(CLIPBOARD_CLEAR_AFTER);
        if self.clipboard.copy(secret, clear_after, Instant::now()).is_err() {
            ctx.copy_text(secret.to_string());
            self.set_status("Kopiert – Verlauf konnte nicht ausgeschlossen werden", false);
        }
        ctx.request_repaint();
    }

    fn regenerate(&mut self) {
        self.gen_password = Zeroizing::new(generator::generate(&self.gen_opts).unwrap_or_default());
    }

    fn generator_view(&mut self, ui: &mut egui::Ui) {
        let mut regenerate = false;
        let mut copy = false;
        ui.label(theme::heading("Passwortgenerator", 28.0));
        ui.add_space(4.0);

        theme::plain_card(theme::SURFACE_HI, Margin::same(18.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 14.0;
            let shown = if self.gen_password.is_empty() {
                "Mindestens eine Zeichengruppe wählen".to_string()
            } else {
                self.gen_password.to_string()
            };
            ui.label(RichText::new(shown).size(22.0).family(theme::mono_medium()));
            ui.horizontal(|ui| {
                // Balken auf Höhe der 36 px hohen Buttons.
                ui.vertical(|ui| {
                    ui.add_space(16.0);
                    strength_bars(ui, &self.gen_password, 40.0);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if ui.add(theme::accent_button("Kopieren")).clicked() {
                        copy = true;
                    }
                    if ui.add(theme::secondary_button("Neu erzeugen")).clicked() {
                        regenerate = true;
                    }
                });
            });
        });
        ui.add_space(4.0);

        ui.spacing_mut().item_spacing.y = 0.0;
        regenerate |= length_slider(ui, &mut self.gen_opts.length, 12, 32);
        regenerate |= toggle_row(ui, "Großbuchstaben", None, &mut self.gen_opts.upper, 40.0);
        regenerate |= toggle_row(ui, "Kleinbuchstaben", None, &mut self.gen_opts.lower, 40.0);
        regenerate |= toggle_row(ui, "Zahlen", None, &mut self.gen_opts.digits, 40.0);
        regenerate |= toggle_row(ui, "Symbole", None, &mut self.gen_opts.symbols, 40.0);
        regenerate |= toggle_row(
            ui,
            "Mehrdeutige Zeichen ausschließen",
            None,
            &mut self.gen_opts.exclude_ambiguous,
            40.0,
        );

        if copy {
            let secret = self.gen_password.clone();
            self.copy_secret(ui.ctx(), &secret);
        }
        if regenerate {
            self.regenerate();
        }
    }

    // ---- Einstellungen (Entwurf 05) ----------------------------------------

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        let mut change_master = false;
        ui.label(theme::heading("Einstellungen", 28.0));
        ui.add_space(6.0);

        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            toggle_row(
                ui,
                "Automatisch sperren",
                Some("Nach 5 Minuten Inaktivität"),
                &mut self.settings.auto_lock,
                72.0,
            );
            toggle_row(
                ui,
                "Zwischenablage nach 30 Sekunden leeren",
                None,
                &mut self.settings.clear_clipboard,
                52.0,
            );
            if toggle_row(
                ui,
                "Beim Start nach Updates suchen",
                None,
                &mut self.persisted.check_updates_on_start,
                52.0,
            ) {
                self.save_persisted();
            }

            ui.horizontal(|ui| {
                ui.set_min_height(52.0);
                ui.label(RichText::new("Updates").size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.updater.is_some() {
                        let busy = self.update_poll.is_some();
                        let button = theme::secondary_button("Jetzt nach Updates suchen").min_size(egui::vec2(0.0, 34.0));
                        if ui.add_enabled(!busy, button).clicked() {
                            match self.start_update_check(true) {
                                Ok(()) => self.set_status("Suche nach Updates…", false),
                                Err(e) => self.set_status(format!("Updater konnte nicht gestartet werden: {e}"), true),
                            }
                        }
                    } else {
                        ui.label(theme::muted("Updater nicht gefunden", 13.0));
                    }
                });
            });

            ui.horizontal(|ui| {
                ui.set_min_height(66.0);
                ui.label(RichText::new("Master-Passwort ändern").size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let button = theme::secondary_button("Master-Passwort ändern").min_size(egui::vec2(0.0, 34.0));
                    if ui.add(button).clicked() {
                        self.show_master_form = !self.show_master_form;
                    }
                });
            });
            if self.show_master_form {
                theme::card().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.add(
                        password_input(&mut self.current_master)
                            .hint_text(theme::muted("Aktuelles Master-Passwort", 15.0)),
                    );
                    ui.add(
                        password_input(&mut self.new_master)
                            .hint_text(theme::muted("Neues Master-Passwort (mind. 12 Zeichen)", 15.0)),
                    );
                    ui.add(password_input(&mut self.new_master_confirm).hint_text(theme::muted("Wiederholen", 15.0)));
                    if ui.add(theme::accent_button("Speichern")).clicked() {
                        change_master = true;
                    }
                });
            }

            ui.horizontal(|ui| {
                ui.set_min_height(52.0);
                ui.label(RichText::new("Speicherort").size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let shown = match &self.path {
                        Ok(p) => p.display().to_string(),
                        Err(problem) => problem.clone(),
                    };
                    ui.add(egui::Label::new(theme::muted(shown.clone(), 13.0).monospace()).truncate())
                        .on_hover_text(shown);
                });
            });
            ui.add_space(16.0);

            theme::plain_card(theme::SURFACE, Margin::symmetric(20.0, 18.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(theme::muted("Verschlüsselung", 13.0));
                ui.label(RichText::new("Argon2id zur Schlüsselableitung, XChaCha20-Poly1305 zur Verschlüsselung").size(14.0));
                ui.label(theme::muted("vault.aegis · AEGS · v1", 13.0).monospace());
            });
        });

        if change_master {
            self.change_master();
        }
    }

    /// Speichert die dauerhaften Einstellungen sofort; Fehler erscheinen rot in der Statusleiste.
    fn save_persisted(&mut self) {
        let result = match settings::path() {
            Some(path) => settings::save(&path, &self.persisted).map_err(|e| e.to_string()),
            None => Err("APPDATA nicht gesetzt".to_string()),
        };
        if let Err(e) = result {
            self.set_status(format!("Einstellung konnte nicht gespeichert werden: {e}"), true);
        }
    }

    fn change_master(&mut self) {
        if self.current_master.is_empty() {
            self.set_status("Bitte das aktuelle Passwort eingeben", true);
            return;
        }
        if let Err(msg) = validate_new_master(&self.new_master, &self.new_master_confirm) {
            self.set_status(msg, true);
            return;
        }
        let Some(session) = self.session.as_mut() else { return };
        let result = session.change_master(
            self.current_master.as_bytes(),
            self.new_master.as_bytes(),
            &KdfParams::default(),
        );
        self.current_master.zeroize();
        match result {
            Ok(outcome) => {
                self.new_master.zeroize();
                self.new_master_confirm.zeroize();
                self.show_master_form = false;
                match outcome {
                    MasterChange::Complete => self.set_status("Master-Passwort geändert", false),
                    // Rot, damit der Hinweis stehen bleibt: das neue Passwort gilt trotzdem.
                    MasterChange::BackupNotRemoved(path) => self.set_status(
                        format!(
                            "Master-Passwort geändert, aber altes Backup {} konnte nicht entfernt werden – bitte manuell löschen",
                            path.display()
                        ),
                        true,
                    ),
                }
            }
            Err(AegisError::WrongPassword) => self.set_status(AegisError::WrongPassword.to_string(), true),
            Err(e) => self.set_status(format!("Speichern fehlgeschlagen: {e}"), true),
        }
    }
}

struct UnlockTexts {
    title: &'static str,
    button: &'static str,
}

/// Überschrift und Button-Text des Entsperren-Bildschirms.
fn unlock_texts(first_start: bool, busy: bool) -> UnlockTexts {
    match (first_start, busy) {
        (true, false) => UnlockTexts { title: "Master-Passwort festlegen", button: "Tresor anlegen" },
        (true, true) => UnlockTexts { title: "Master-Passwort festlegen", button: "Wird angelegt…" },
        (false, false) => UnlockTexts { title: "Tresor entsperren", button: "Entsperren" },
        (false, true) => UnlockTexts { title: "Tresor entsperren", button: "Wird entsperrt…" },
    }
}

/// Prüft ein neues Master-Passwort und seine Wiederholung.
fn validate_new_master(new: &str, confirm: &str) -> Result<(), &'static str> {
    if new.is_empty() {
        return Err("Bitte ein neues Passwort eingeben");
    }
    if new.chars().count() < MIN_MASTER_LEN {
        return Err("Mindestens 12 Zeichen");
    }
    if new != confirm {
        return Err("Die Passwörter stimmen nicht überein");
    }
    Ok(())
}

// ---- Fenster-Titelleiste ---------------------------------------------------

/// Titelleiste wie im Entwurf: Logo und „Aegis“ links, Minimieren und Schließen rechts.
/// Ein Statushinweis (z. B. „Gespeichert“) steht links neben den Fensterknöpfen.
/// `sidebar_toggle: Some(offen)` zeigt rechts neben „Aegis“ den Seitenleisten-Knopf.
/// Gibt `true` zurück, wenn er geklickt wurde.
fn title_bar(
    ui: &mut egui::Ui,
    rect: Rect,
    mark: egui::TextureId,
    status: &str,
    status_error: bool,
    sidebar_toggle: Option<bool>,
) -> bool {
    let drag = ui.interact(rect, egui::Id::new("title_bar"), Sense::click_and_drag());
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let close_rect = Rect::from_min_size(egui::pos2(rect.right() - 46.0, rect.top()), egui::vec2(46.0, rect.height()));
    let min_rect = close_rect.translate(egui::vec2(-46.0, 0.0));
    let close = ui.interact(close_rect, egui::Id::new("title_close"), Sense::click());
    let minimize = ui.interact(min_rect, egui::Id::new("title_minimize"), Sense::click());

    let painter = ui.painter();
    // Mini-Schild (32 px Bild, auf 16 px gezeichnet, damit es auch bei hoher Auflösung scharf bleibt).
    let logo = Rect::from_min_size(egui::pos2(rect.left() + 13.0, rect.center().y - 8.0), egui::vec2(16.0, 16.0));
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(mark, logo, full, egui::Color32::WHITE);
    let name = painter.text(
        egui::pos2(logo.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Aegis",
        FontId::proportional(12.0),
        theme::MUTED,
    );

    let mut toggled = false;
    if let Some(open) = sidebar_toggle {
        // 28 × 28, rahmenlos wie die Navigation; Hover-Fläche wie bei Minimieren.
        let spot = Rect::from_center_size(egui::pos2(name.right() + 12.0 + 14.0, rect.center().y), egui::vec2(28.0, 28.0));
        let label = if open { "Seitenleiste einklappen" } else { "Seitenleiste ausklappen" };
        let response = ui.interact(spot, egui::Id::new("sidebar_toggle"), Sense::click()).on_hover_text(label);
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(spot, 6.0, theme::SURFACE_HI);
        }
        let color = if response.hovered() { theme::TEXT } else { theme::MUTED };
        draw_sidebar_icon(painter, spot.center(), open, color);
        toggled = response.clicked();
    }

    let painter = ui.painter();

    if !status.is_empty() {
        let color = if status_error { theme::DANGER } else { theme::MUTED };
        painter.text(
            egui::pos2(min_rect.left() - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status,
            FontId::proportional(12.0),
            color,
        );
    }

    if minimize.hovered() {
        painter.rect_filled(min_rect, 0.0, theme::SURFACE_HI);
    }
    if close.hovered() {
        let rounding = Rounding { ne: 12.0, ..Rounding::ZERO };
        painter.rect_filled(close_rect, rounding, theme::DANGER.gamma_multiply(0.35));
    }
    let line = Stroke::new(1.0, theme::TEXT);
    let c = min_rect.center();
    painter.line_segment([c - egui::vec2(5.0, 0.0), c + egui::vec2(5.0, 0.0)], line);
    let c = close_rect.center();
    painter.line_segment([c + egui::vec2(-6.0, -6.0), c + egui::vec2(6.0, 6.0)], line);
    painter.line_segment([c + egui::vec2(-6.0, 6.0), c + egui::vec2(6.0, -6.0)], line);

    if minimize.clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    if close.clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    toggled
}

/// Hinweisleiste (36 px) unter der Titelleiste. Gibt zurück, welcher Knopf gedrückt wurde.
fn banner_bar(ui: &mut egui::Ui, rect: Rect, banner: &Banner) -> Option<BannerAction> {
    let text = updates::banner_text(banner)?;
    let danger = matches!(banner, Banner::Security { .. } | Banner::Rejected { .. } | Banner::InstallFailed { .. });
    let (fill, color) = match banner {
        _ if danger => (theme::DANGER.gamma_multiply(0.14), theme::DANGER),
        Banner::Stale { .. } => (theme::SURFACE, theme::MUTED),
        _ => (theme::SURFACE_HI, theme::TEXT),
    };
    ui.painter().rect_filled(rect, Rounding::ZERO, fill);
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::BORDER));

    let mut action = None;
    let inner = rect.shrink2(egui::vec2(16.0, 0.0));
    let mut bar = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    bar.add(egui::Label::new(RichText::new(text).size(14.0).color(color)).truncate());
    bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // Von rechts nach links: der Hauptknopf steht ganz rechts.
        let small = egui::vec2(0.0, 26.0);
        match banner {
            Banner::Security { .. } => {
                if ui.add(theme::accent_button("Jetzt aktualisieren").min_size(small)).clicked() {
                    action = Some(BannerAction::Install);
                }
            }
            Banner::Update { .. } => {
                if ui.add(theme::accent_button("Aktualisieren").min_size(small)).clicked() {
                    action = Some(BannerAction::Install);
                }
                if ui.add(theme::secondary_button("Später").min_size(small)).clicked() {
                    action = Some(BannerAction::Later);
                }
            }
            Banner::InstallFailed { .. } => {
                if ui.add(theme::secondary_button("Schließen").min_size(small)).clicked() {
                    action = Some(BannerAction::DismissInstallFailure);
                }
            }
            Banner::Hidden | Banner::Stale { .. } | Banner::Rejected { .. } => {}
        }
    });
    action
}

/// Mini-Schild aus `assets/icon/aegis-mark-32.png` (erzeugt aus `aegis-mark.svg`).
fn title_mark_image() -> egui::ColorImage {
    // Fest eingebettet: Ein Dekodierfehler wäre ein kaputter Build, kein Laufzeitfall.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/aegis-mark-32.png"))
        .expect("eingebettetes Mini-Schild ist ein gültiges PNG");
    egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba)
}

/// Seitenleisten-Symbol: Fensterrahmen mit linker Spalte, die gefüllt ist, wenn die Leiste offen ist.
fn draw_sidebar_icon(painter: &egui::Painter, c: Pos2, open: bool, color: egui::Color32) {
    let frame = Rect::from_center_size(c, egui::vec2(14.0, 11.0));
    let divider = frame.left() + 4.5;
    if open {
        let column = Rect::from_min_max(frame.left_top(), egui::pos2(divider, frame.bottom()));
        painter.rect_filled(column, Rounding { nw: 2.5, sw: 2.5, ..Rounding::ZERO }, color.gamma_multiply(0.6));
    }
    let stroke = Stroke::new(ICON_STROKE, color);
    painter.rect_stroke(frame, 2.5, stroke);
    painter.line_segment([egui::pos2(divider, frame.top()), egui::pos2(divider, frame.bottom())], stroke);
}

// ---- Kleine Bausteine ------------------------------------------------------

#[derive(Clone, Copy)]
enum Icon {
    Search,
    List,
    Key,
    Gear,
    Door,
}

/// Eintrag der Seitenleiste: 36 px hoch, Symbol links, Text 14 px. Eingeklappt nur das Symbol.
fn nav_button(ui: &mut egui::Ui, selected: bool, icon: Icon, label: &str, open: bool) -> bool {
    let width = if open { ui.available_width() } else { 36.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 36.0), Sense::click());
    // Sperren ist im Entwurf rot markiert.
    let color = match icon {
        Icon::Door => theme::DANGER,
        _ if selected => theme::TEXT,
        _ => theme::MUTED,
    };
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 8.0, theme::SURFACE_HI);
    } else if response.hovered() {
        painter.rect_filled(rect, 8.0, theme::SURFACE);
    }

    let center = if open { egui::pos2(rect.left() + 18.0, rect.center().y) } else { rect.center() };
    draw_icon(painter, icon, center, color);
    if open {
        painter.text(
            egui::pos2(rect.left() + 36.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.0),
            color,
        );
    }
    response.on_hover_text(label).clicked()
}

/// Stärke der Linien in allen gezeichneten Symbolen.
const ICON_STROKE: f32 = 1.25;

/// Navigationssymbole als feine Linienzeichnungen, ca. 12 × 12 px, ohne gefüllte Flächen.
fn draw_icon(painter: &egui::Painter, icon: Icon, c: Pos2, color: egui::Color32) {
    let stroke = Stroke::new(ICON_STROKE, color);
    let p = |dx: f32, dy: f32| egui::pos2(c.x + dx, c.y + dy);
    match icon {
        Icon::Search => {
            painter.circle_stroke(p(-1.0, -1.0), 4.25, stroke);
            painter.line_segment([p(2.1, 2.1), p(5.5, 5.5)], stroke);
        }
        Icon::List => {
            // Aufzählungspunkte und kürzere Linien wirken leichter als drei volle Balken.
            for dy in [-4.0, 0.0, 4.0] {
                painter.circle_filled(p(-5.25, dy), 0.95, color);
                painter.line_segment([p(-2.75, dy), p(6.0, dy)], stroke);
            }
        }
        Icon::Key => {
            // Ring, Schaft und zwei Zähne.
            painter.circle_stroke(p(-3.25, 0.0), 2.75, stroke);
            painter.line_segment([p(-0.5, 0.0), p(6.0, 0.0)], stroke);
            painter.line_segment([p(4.25, 0.0), p(4.25, 2.5)], stroke);
            painter.line_segment([p(2.25, 0.0), p(2.25, 1.75)], stroke);
        }
        Icon::Gear => {
            // Zahnrad: 8 Zähne als geschlossener Umriss, innen ein Ring als Nabe.
            const TEETH: usize = 8;
            let (outer, inner) = (5.75, 4.25);
            let step = std::f32::consts::TAU / TEETH as f32;
            let at = |r: f32, a: f32| p(r * a.cos(), r * a.sin());
            let mut outline = Vec::with_capacity(TEETH * 4);
            for k in 0..TEETH {
                let mid = k as f32 * step;
                outline.extend([
                    at(inner, mid - 0.34),
                    at(outer, mid - 0.2),
                    at(outer, mid + 0.2),
                    at(inner, mid + 0.34),
                ]);
            }
            painter.add(egui::Shape::closed_line(outline, stroke));
            painter.circle_stroke(c, 1.9, stroke);
        }
        Icon::Door => {
            // Tür mit Knauf auf einer Bodenlinie, die links und rechts übersteht.
            let door = Rect::from_min_max(p(-3.75, -6.0), p(3.75, 5.5));
            painter.rect_stroke(door, Rounding { nw: 1.5, ne: 1.5, sw: 0.0, se: 0.0 }, stroke);
            painter.line_segment([p(-6.0, 5.5), p(6.0, 5.5)], stroke);
            painter.circle_filled(p(1.6, 0.0), 0.95, color);
        }
    }
}

/// Passwortfeld wie im Entwurf: 44 px hoch, Punkte in Monospace.
fn password_input(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .password(true)
        .font(FontId::monospace(16.0))
        .margin(Margin::symmetric(14.0, 12.0))
        .desired_width(f32::INFINITY)
}

/// Schalter im Stil des Entwurfs: 36 × 20, Akzentgrün, wenn aktiv.
fn toggle(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(36.0, 20.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let painter = ui.painter();
    let (fill, knob, knob_x) = if *value {
        (theme::ACCENT, theme::ACCENT_TEXT, rect.right() - 10.0)
    } else {
        (theme::BORDER, theme::MUTED, rect.left() + 10.0)
    };
    painter.rect_filled(rect, 10.0, fill);
    painter.circle_filled(egui::pos2(knob_x, rect.center().y), 8.0, knob);
    response
}

/// Zeile mit Text links (15 px, optional Hinweis 13 px) und Schalter rechts.
fn toggle_row(ui: &mut egui::Ui, label: &str, hint: Option<&str>, value: &mut bool, height: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(height);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let extra = if hint.is_some() { 21.0 } else { 0.0 };
            ui.add_space((height - 20.0 - extra) / 2.0);
            ui.label(RichText::new(label).size(15.0));
            if let Some(hint) = hint {
                ui.label(theme::muted(hint, 13.0));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = toggle(ui, value).changed();
        });
    });
    changed
}

/// Längen-Regler wie im Entwurf: „Länge“ darüber, grüne Spur, Zahl als Griff, 12 und 32 an den Enden.
fn length_slider(ui: &mut egui::Ui, value: &mut usize, min: usize, max: usize) -> bool {
    ui.horizontal(|ui| {
        ui.set_min_height(40.0);
        ui.label(RichText::new("Länge").size(15.0));
    });
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), Sense::click_and_drag());
    let y = rect.top() + 18.0;
    let track = Rect::from_min_max(egui::pos2(rect.left() + 28.0, y - 2.0), egui::pos2(rect.right() - 28.0, y + 2.0));

    let mut changed = false;
    if response.clicked() || response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let new = min + (t * (max - min) as f32).round() as usize;
            if new != *value {
                *value = new;
                changed = true;
            }
        }
    }

    let t = (*value - min) as f32 / (max - min) as f32;
    let x = track.left() + t * track.width();
    let painter = ui.painter();
    painter.rect_filled(track, 2.0, theme::BORDER);
    painter.rect_filled(Rect::from_min_max(track.left_top(), egui::pos2(x, track.bottom())), 2.0, theme::ACCENT);
    let mono = FontId::monospace(12.0);
    painter.text(egui::pos2(rect.left(), y), Align2::LEFT_CENTER, min.to_string(), mono.clone(), theme::MUTED);
    painter.text(egui::pos2(rect.right(), y), Align2::RIGHT_CENTER, max.to_string(), mono, theme::MUTED);
    let badge = Rect::from_center_size(egui::pos2(x, y), egui::vec2(32.0, 20.0));
    painter.rect_filled(badge, 6.0, theme::TEXT);
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        (*value).to_string(),
        FontId::new(12.0, theme::mono_medium()),
        theme::ACCENT_TEXT,
    );
    changed
}

fn avatar(ui: &mut egui::Ui, title: &str) {
    let letter = title
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    // Fest 36 × 36 gezeichnet, damit die Zeile nicht die ganze Breite einnimmt.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::PANEL);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, letter, FontId::new(17.0, theme::serif()), theme::TEXT);
}

/// Vier Balken für die Passwortstärke, ohne Beschriftung (wie im Entwurf).
fn strength_bars(ui: &mut egui::Ui, pw: &str, bar_width: f32) {
    let level = generator::strength(pw);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for i in 0..4 {
            let color = if i < level { theme::ACCENT } else { theme::BORDER };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_width, 4.0), Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_mark_is_32_square() {
        assert_eq!(title_mark_image().size, [32, 32]);
    }

    #[test]
    fn sidebar_starts_collapsed() {
        assert!(!AegisApp::default().sidebar_open);
    }

    #[test]
    fn unlock_texts_cover_all_states() {
        let t = unlock_texts(true, false);
        assert_eq!((t.title, t.button), ("Master-Passwort festlegen", "Tresor anlegen"));
        let t = unlock_texts(true, true);
        assert_eq!((t.title, t.button), ("Master-Passwort festlegen", "Wird angelegt…"));
        let t = unlock_texts(false, false);
        assert_eq!((t.title, t.button), ("Tresor entsperren", "Entsperren"));
        let t = unlock_texts(false, true);
        assert_eq!((t.title, t.button), ("Tresor entsperren", "Wird entsperrt…"));
    }

    #[test]
    fn master_policy() {
        assert!(validate_new_master("", "").is_err());
        assert!(validate_new_master("kurz", "kurz").is_err());
        assert!(validate_new_master("zwölfzeichen", "zwölfzeichen").is_ok()); // 12 Zeichen
        assert!(validate_new_master("äöüäöüäöüäö", "äöüäöüäöüäö").is_err()); // 11 Zeichen, 22 Byte
        assert!(validate_new_master("zwölfzeichen", "zwölfzeichEn").is_err());
    }
}
