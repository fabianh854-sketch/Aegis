//! Dauerhafte Einstellungen in `%APPDATA%\Aegis\settings.json`.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

/// Einstellungen, die den Neustart überdauern.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Persisted {
    /// Beim Start im Hintergrund nach Updates suchen.
    pub check_updates_on_start: bool,
}

impl Default for Persisted {
    fn default() -> Self {
        Self { check_updates_on_start: true }
    }
}

/// `%APPDATA%\Aegis\settings.json`
pub fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(|base| PathBuf::from(base).join("Aegis").join("settings.json"))
}

/// Liest die Einstellungen; jedes Problem (fehlt, kaputt) ergibt die Standardwerte.
pub fn load(path: &Path) -> Persisted {
    fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

/// Schreibt atomar: Temp-Datei im selben Ordner, dann umbenennen.
pub fn save(path: &Path, p: &Persisted) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let data = serde_json::to_vec_pretty(p).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}-{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let tmp = PathBuf::from(name);

    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path) // unter Windows wird die Zieldatei ersetzt
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp); // Aufräumen; der eigentliche Fehler geht an den Aufrufer
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aegis-app-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_default_checks_on_start() {
        assert!(Persisted::default().check_updates_on_start);
        // Fehlende Datei und fehlendes Feld ergeben ebenfalls „an“.
        let dir = temp_dir("default");
        assert!(load(&dir.join("settings.json")).check_updates_on_start);
        fs::write(dir.join("leer.json"), "{}").unwrap();
        assert!(load(&dir.join("leer.json")).check_updates_on_start);
    }

    #[test]
    fn settings_roundtrip_and_corrupt_falls_back() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sub").join("settings.json"); // Ordner wird angelegt
        let off = Persisted { check_updates_on_start: false };
        save(&path, &off).unwrap();
        assert_eq!(load(&path), off);
        save(&path, &Persisted::default()).unwrap(); // überschreibt
        assert_eq!(load(&path), Persisted::default());
        // Keine Temp-Reste neben der Datei.
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");

        fs::write(&path, "{ kaputt").unwrap();
        assert_eq!(load(&path), Persisted::default());
        fs::write(&path, r#"{"check_updates_on_start":"nein"}"#).unwrap();
        assert_eq!(load(&path), Persisted::default());
    }
}
