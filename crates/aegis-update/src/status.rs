//! Statusdatei `update.json`: Der Updater schreibt sie, die App liest sie.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{manifest::UpdateError, version::Version};

/// Ergebnis der letzten Update-Prüfung.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum UpdateStatus {
    None {
        checked_at: u64,
    },
    Available {
        checked_at: u64,
        version: Version,
        security: bool,
    },
    Stale {
        checked_at: u64,
        version: Version,
        expired_at: u64,
    },
    Rejected {
        checked_at: u64,
        version: Version,
        reason: String,
    },
    Error {
        checked_at: u64,
        message: String,
        /// `true`: Fehler bei `--install` (nicht bei einer Prüfung). Fehlt in älteren Dateien.
        #[serde(default)]
        during_install: bool,
    },
}

impl UpdateStatus {
    /// Zeitpunkt der Prüfung (Unix-Sekunden).
    pub fn checked_at(&self) -> u64 {
        match self {
            Self::None { checked_at }
            | Self::Available { checked_at, .. }
            | Self::Stale { checked_at, .. }
            | Self::Rejected { checked_at, .. }
            | Self::Error { checked_at, .. } => *checked_at,
        }
    }
}

fn io_err(e: std::io::Error) -> UpdateError {
    UpdateError::Io(e.to_string())
}

/// Schreibt atomar: Temp-Datei im selben Ordner, dann umbenennen.
pub fn write_status(path: &Path, status: &UpdateStatus) -> Result<(), UpdateError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let data = serde_json::to_vec(status).map_err(|e| UpdateError::BadFormat(e.to_string()))?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = PathBuf::from(name);

    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path) // unter Windows wird die Zieldatei ersetzt
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp); // Aufräumen; der eigentliche Fehler geht an den Aufrufer
        return Err(io_err(e));
    }
    Ok(())
}

/// Liest die Statusdatei; jedes Problem (fehlt, kaputt, unbekannt) ergibt `None`.
pub fn read_status(path: &Path) -> Option<UpdateStatus> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `%APPDATA%\Aegis\update.json`
pub fn status_path() -> Option<PathBuf> {
    env_dir("APPDATA").map(|d| d.join("Aegis").join("update.json"))
}

/// `%LOCALAPPDATA%\Aegis\update`
pub fn staging_dir() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA").map(|d| d.join("Aegis").join("update"))
}

/// Aktuelle Zeit in Unix-Sekunden.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aegis-update-status-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn status_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("update.json");
        let all = [
            UpdateStatus::None { checked_at: 1 },
            UpdateStatus::Available {
                checked_at: 2,
                version: v("0.2.0"),
                security: true,
            },
            UpdateStatus::Stale {
                checked_at: 3,
                version: v("0.2.0"),
                expired_at: 99,
            },
            UpdateStatus::Rejected {
                checked_at: 4,
                version: v("0.2.0"),
                reason: "Signatur".into(),
            },
            UpdateStatus::Error {
                checked_at: 5,
                message: "offline".into(),
                during_install: false,
            },
            UpdateStatus::Error {
                checked_at: 6,
                message: "Hash".into(),
                during_install: true,
            },
        ];
        for (i, s) in all.iter().enumerate() {
            write_status(&path, s).unwrap();
            assert_eq!(read_status(&path).as_ref(), Some(s));
            assert_eq!(s.checked_at(), i as u64 + 1);
        }
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"status\":\"error\""), "{json}");
        assert!(json.contains("\"during_install\":true"), "{json}");
    }

    #[test]
    fn old_error_without_during_install_still_parses() {
        // So schrieb der Updater vor dem Feld `during_install`.
        let dir = temp_dir("old-error");
        let path = dir.join("update.json");
        fs::write(
            &path,
            r#"{"status":"error","checked_at":9,"message":"offline"}"#,
        )
        .unwrap();
        assert_eq!(
            read_status(&path),
            Some(UpdateStatus::Error {
                checked_at: 9,
                message: "offline".into(),
                during_install: false
            })
        );
    }

    #[test]
    fn corrupt_or_missing_status_is_none() {
        let dir = temp_dir("corrupt");
        let path = dir.join("update.json");
        assert_eq!(read_status(&path), None);
        fs::write(&path, "{").unwrap();
        assert_eq!(read_status(&path), None);
        fs::write(&path, r#"{"status":"unbekannt","checked_at":1}"#).unwrap();
        assert_eq!(read_status(&path), None);
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_status(&path), None);
    }

    #[test]
    fn write_is_atomic_no_tmp_left() {
        let dir = temp_dir("atomic");
        let path = dir.join("update.json");
        write_status(&path, &UpdateStatus::None { checked_at: 1 }).unwrap();
        write_status(&path, &UpdateStatus::None { checked_at: 2 }).unwrap();
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("update.json")]);
        assert_eq!(
            read_status(&path),
            Some(UpdateStatus::None { checked_at: 2 })
        );
    }

    #[test]
    fn write_creates_missing_parent_dir() {
        let dir = temp_dir("parent");
        let path = dir.join("Aegis").join("update.json");
        write_status(&path, &UpdateStatus::None { checked_at: 7 }).unwrap();
        assert_eq!(
            read_status(&path),
            Some(UpdateStatus::None { checked_at: 7 })
        );
    }

    #[test]
    fn now_is_plausible() {
        assert!(now() > 1_700_000_000);
    }
}
