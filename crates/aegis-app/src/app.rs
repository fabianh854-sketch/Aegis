use std::{fs, path::PathBuf};

use aegis_core::{Entry, Vault};
use eframe::egui;
use zeroize::Zeroizing;

pub struct AegisApp {
    path: PathBuf,
    password: String,                  // Eingabefeld, nach dem Entsperren geleert
    master: Option<Zeroizing<String>>, // Master-Passwort für Speichervorgänge
    vault: Option<Vault>,
    new_title: String,
    status: String,
}

impl Default for AegisApp {
    fn default() -> Self {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        Self {
            path: base.join("Aegis").join("vault.aegis"),
            password: String::new(),
            master: None,
            vault: None,
            new_title: String::new(),
            status: String::new(),
        }
    }
}

impl eframe::App for AegisApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.vault.is_some() {
                self.list_view(ui);
            } else {
                self.unlock_view(ui);
            }
        });
    }
}

impl AegisApp {
    fn unlock_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Aegis entsperren");

        let field = ui.add(
            egui::TextEdit::singleline(&mut self.password)
                .password(true)
                .hint_text("Master-Passwort"),
        );
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if ui.button("Entsperren").clicked() || enter {
            self.unlock();
        }
        ui.label(&self.status);
    }

    fn unlock(&mut self) {
        if self.password.is_empty() {
            self.status = "Bitte Master-Passwort eingeben".into();
            return;
        }

        let result = if self.path.exists() {
            aegis_core::load(&self.path, self.password.as_bytes())
        } else {
            // Erststart: leeren Vault mit diesem Passwort anlegen
            if let Some(dir) = self.path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            aegis_core::save(&self.path, self.password.as_bytes(), &Vault::default())
                .map(|_| Vault::default())
        };

        match result {
            Ok(vault) => {
                self.vault = Some(vault);
                self.master = Some(Zeroizing::new(std::mem::take(&mut self.password)));
                self.status.clear();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn lock(&mut self) {
        self.vault = None;
        self.master = None;
        self.password.clear();
        self.status.clear();
    }

    fn add_entry(&mut self) {
        let (Some(vault), Some(master)) = (self.vault.as_mut(), self.master.as_ref()) else {
            return;
        };

        vault.entries.push(Entry {
            title: std::mem::take(&mut self.new_title),
            ..Default::default()
        });

        self.status = match aegis_core::save(&self.path, master.as_bytes(), vault) {
            Ok(()) => "Gespeichert".into(),
            Err(e) => e.to_string(),
        };
    }

    fn list_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Einträge");

        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.new_title);
            if ui.button("Hinzufügen").clicked() && !self.new_title.is_empty() {
                self.add_entry();
            }
            if ui.button("Sperren").clicked() {
                self.lock();
            }
        });

        ui.separator();

        if let Some(vault) = &self.vault {
            for entry in &vault.entries {
                ui.label(&entry.title);
            }
        }

        ui.label(&self.status);
    }
}
