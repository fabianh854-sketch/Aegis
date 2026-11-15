//! `aegis-updater.exe`: einziges Programm von Aegis mit Netzwerkzugriff (nur über curl.exe).
//!
//! `--check` prüft auf Updates und schreibt `update.json`;
//! `--install --wait-pid <pid>` installiert ein geprüftes Update und startet Aegis neu.
#![windows_subsystem = "windows"]

mod fetch;
mod github;
mod install;
#[cfg(test)]
mod testutil;

use std::path::{Path, PathBuf};

use aegis_update::{
    installed, now, staging_dir, status_path, write_status, Decision, UpdateError, UpdateStatus,
    TRUSTED_KEYS,
};

use crate::{
    fetch::{Curl, Fetch},
    github::{check, check_releases},
    install::{cleanup_old, download_verified, replace_files, restore_missing},
};

/// Befehl von der Kommandozeile.
#[derive(Debug, PartialEq)]
enum Command {
    Check,
    Install { wait_pid: u32 },
}

fn parse_args(args: &[String]) -> Option<Command> {
    match args {
        [c] if c == "--check" => Some(Command::Check),
        [i, w, pid] if i == "--install" && w == "--wait-pid" => pid
            .parse()
            .ok()
            .map(|wait_pid| Command::Install { wait_pid }),
        _ => None,
    }
}

/// Bildet die Entscheidung auf die Statusdatei ab.
fn status_from(decision: &Decision, checked_at: u64) -> UpdateStatus {
    match decision {
        Decision::UpToDate => UpdateStatus::None { checked_at },
        Decision::Available { manifest, security } => UpdateStatus::Available {
            checked_at,
            version: manifest.version,
            security: *security,
        },
        Decision::Stale {
            version,
            expired_at,
        } => UpdateStatus::Stale {
            checked_at,
            version: *version,
            expired_at: *expired_at,
        },
        Decision::Rejected { version, reason } => UpdateStatus::Rejected {
            checked_at,
            version: *version,
            reason: reason.clone(),
        },
    }
}

/// `during_install`: Fehler bei `--install`; die App zeigt ihn nach dem Neustart an.
fn error_status(e: &UpdateError, during_install: bool) -> UpdateStatus {
    UpdateStatus::Error {
        checked_at: now(),
        message: e.to_string(),
        during_install,
    }
}

/// Schreibt den Status; ohne Konsole gibt es niemanden, dem ein Fehler hier zu melden wäre.
fn save_status(status: &UpdateStatus) {
    if let Some(path) = status_path() {
        let _ = write_status(&path, status);
    }
}

/// Ordner von `aegis-updater.exe`: einziges Ziel eines Updates.
fn target_dir() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe().map_err(|e| UpdateError::Io(e.to_string()))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| UpdateError::Io("Programmordner unbekannt".into()))
}

fn run_check(fetch: &dyn Fetch) -> UpdateStatus {
    if let Ok(target) = target_dir() {
        cleanup_old(&target);
    }
    match check(fetch, installed(), TRUSTED_KEYS, now()) {
        Ok(decision) => status_from(&decision, now()),
        Err(e) => error_status(&e, false),
    }
}

/// Ergebnis der Installation für den Neustart.
enum Installed {
    /// Dateien ersetzt.
    Updated,
    /// Kein (gültiges) Update mehr verfügbar; Status der erneuten Prüfung.
    NotAvailable(UpdateStatus),
    /// Aegis läuft nach der Wartezeit noch: nichts ersetzt, kein zweites Fenster starten.
    StillRunning,
}

/// Prüft erneut (update.json ist nicht vertrauenswürdig), lädt, prüft, wartet und ersetzt.
fn install(fetch: &dyn Fetch, target: &Path, wait_pid: u32) -> Result<Installed, UpdateError> {
    cleanup_old(target);
    let (decision, releases) = check_releases(fetch, installed(), TRUSTED_KEYS, now())?;
    let Decision::Available { manifest, .. } = decision else {
        return Ok(Installed::NotAvailable(status_from(&decision, now())));
    };
    let release = releases
        .iter()
        .find(|r| r.tag == manifest.version)
        .ok_or(UpdateError::WrongVersion)?;
    let staging = staging_dir().ok_or_else(|| UpdateError::Io("LOCALAPPDATA fehlt".into()))?;
    download_verified(fetch, release, &manifest, &staging)?;
    if !wait::for_exit(wait_pid) {
        return Ok(Installed::StillRunning);
    }
    replace_files(&staging, target, &manifest)?;
    let _ = std::fs::remove_dir_all(&staging); // Aufräumen; die Installation ist abgeschlossen
    Ok(Installed::Updated)
}

/// Startet `aegis.exe` aus dem Zielordner.
fn start_aegis(target: &Path) {
    let _ = std::process::Command::new(target.join("aegis.exe"))
        .current_dir(target)
        .spawn();
}

/// Status nach `--install`; jeder Fehler ist als Installationsfehler markiert.
fn install_status(result: &Result<Installed, UpdateError>, at: u64) -> UpdateStatus {
    match result {
        Ok(Installed::Updated) => UpdateStatus::None { checked_at: at },
        Ok(Installed::NotAvailable(status)) => status.clone(),
        Ok(Installed::StillRunning) => UpdateStatus::Error {
            checked_at: at,
            message: "Aegis wurde nicht rechtzeitig beendet.".into(),
            during_install: true,
        },
        Err(e) => UpdateStatus::Error {
            checked_at: at,
            message: e.to_string(),
            during_install: true,
        },
    }
}

fn run_install(fetch: &dyn Fetch, wait_pid: u32) {
    let target = match target_dir() {
        Ok(t) => t,
        Err(e) => return save_status(&error_status(&e, true)),
    };
    let result = install(fetch, &target, wait_pid);
    save_status(&install_status(&result, now()));
    if matches!(result, Ok(Installed::StillRunning)) {
        return; // Aegis läuft noch: kein zweites Fenster starten
    }
    // Nach einem Abbruch mitten im Ersetzen fehlt aegis.exe womöglich: alte Datei zurückholen.
    restore_missing(&target);
    start_aegis(&target);
}

/// Warten auf das Ende des Aegis-Prozesses.
mod wait {
    /// Höchstens 60 s; existiert der Prozess nicht (mehr), geht es sofort weiter.
    /// `false`: Der Prozess läuft nach der Wartezeit noch.
    #[cfg(windows)]
    pub fn for_exit(pid: u32) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        const TIMEOUT_MS: u32 = 60_000;
        // SAFETY: Reine Win32-Aufrufe mit geprüftem Handle, das genau einmal geschlossen wird.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return true; // Prozess gibt es nicht mehr
            }
            let result = WaitForSingleObject(handle, TIMEOUT_MS);
            CloseHandle(handle);
            result == WAIT_OBJECT_0
        }
    }

    #[cfg(not(windows))]
    pub fn for_exit(_pid: u32) -> bool {
        true
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Some(Command::Check) => save_status(&run_check(&Curl)),
        Some(Command::Install { wait_pid }) => run_install(&Curl, wait_pid),
        None => std::process::exit(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_accepts_only_known_commands() {
        assert_eq!(parse_args(&args(&["--check"])), Some(Command::Check));
        assert_eq!(
            parse_args(&args(&["--install", "--wait-pid", "1234"])),
            Some(Command::Install { wait_pid: 1234 })
        );
        for bad in [
            &[][..],
            &["--install"],
            &["--install", "--wait-pid"],
            &["--install", "--wait-pid", "abc"],
            &["--install", "--wait-pid", "-1"],
            &["--check", "--extra"],
            &["--wait-pid", "1", "--install"],
            &["https://evil.example"],
        ] {
            assert_eq!(parse_args(&args(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn wait_returns_at_once_for_missing_or_ended_process() {
        // Windows-PIDs sind Vielfache von 4: diese gibt es nie.
        let start = std::time::Instant::now();
        assert!(wait::for_exit(u32::MAX - 2));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--unbekannt")
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(wait::for_exit(pid));
        let _ = child.wait();
        assert!(start.elapsed() < std::time::Duration::from_secs(30));
    }

    #[test]
    fn status_maps_decisions() {
        assert_eq!(
            status_from(&Decision::UpToDate, 7),
            UpdateStatus::None { checked_at: 7 }
        );
        assert_eq!(
            status_from(
                &Decision::Available {
                    manifest: test_manifest("0.2.0"),
                    security: true
                },
                7
            ),
            UpdateStatus::Available {
                checked_at: 7,
                version: v("0.2.0"),
                security: true
            }
        );
        assert_eq!(
            status_from(
                &Decision::Stale {
                    version: v("0.2.0"),
                    expired_at: 5
                },
                7
            ),
            UpdateStatus::Stale {
                checked_at: 7,
                version: v("0.2.0"),
                expired_at: 5
            }
        );
        assert_eq!(
            status_from(
                &Decision::Rejected {
                    version: v("0.2.0"),
                    reason: "Signatur".into()
                },
                7
            ),
            UpdateStatus::Rejected {
                checked_at: 7,
                version: v("0.2.0"),
                reason: "Signatur".into()
            }
        );
    }
}
