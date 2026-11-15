//! Update-Hinweis der App: liest nur die lokale Statusdatei und startet `aegis-updater.exe`.
//! Die App selbst greift nie auf das Netzwerk zu.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use aegis_update::UpdateStatus;

/// Was die Hinweisleiste unter der Titelleiste zeigt.
#[derive(Debug, PartialEq)]
pub enum Banner {
    Hidden,
    Update { version: String },
    Security { version: String },
    Stale { since: u64 },
    /// `clock`: abgelehnt, weil das Manifest „noch nicht gültig“ ist (Systemuhr geht nach).
    Rejected { version: String, clock: bool },
    /// Fehler bei der letzten Installation (aus `update.json` beim Start).
    InstallFailed { message: String },
}

/// So lange nach dem Fehler zeigt die App einen Installationsfehler beim Start noch an.
pub const INSTALL_FAILURE_SHOWN_FOR_SECS: u64 = 10 * 60;

/// Meldung eines Installationsfehlers, wenn er höchstens 10 Minuten alt ist.
/// Liegt `checked_at` in der Zukunft (Uhr verstellt), wird er angezeigt: lieber einmal zu viel.
pub fn install_failure(status: Option<&UpdateStatus>, now: u64) -> Option<String> {
    match status {
        Some(UpdateStatus::Error { checked_at, message, during_install: true })
            if now.saturating_sub(*checked_at) <= INSTALL_FAILURE_SHOWN_FOR_SECS =>
        {
            Some(message.clone())
        }
        _ => None,
    }
}

/// Ein noch nicht geschlossener Installationsfehler geht jedem anderen Hinweis vor.
pub fn select_banner(install_failure: Option<&str>, status: Option<&UpdateStatus>, dismissed: bool) -> Banner {
    match install_failure {
        Some(message) => Banner::InstallFailed { message: message.to_string() },
        None => banner_for(status, dismissed),
    }
}

/// Rückmeldung für „Jetzt nach Updates suchen“: (Text, ist Fehler).
/// `None`, wenn die Hinweisleiste das Ergebnis zeigt.
pub fn check_notice(status: &UpdateStatus) -> Option<(String, bool)> {
    match status {
        UpdateStatus::None { .. } => Some(("Kein Update verfügbar".to_string(), false)),
        UpdateStatus::Error { message, .. } => Some((format!("Update-Prüfung fehlgeschlagen: {message}"), true)),
        UpdateStatus::Available { .. } | UpdateStatus::Stale { .. } | UpdateStatus::Rejected { .. } => None,
    }
}

/// Rückmeldung, wenn eine manuelle Prüfung in der Wartezeit kein Ergebnis liefert.
pub const CHECK_TIMEOUT_NOTICE: &str = "Update-Prüfung fehlgeschlagen: keine Antwort vom Updater";

pub fn banner_for(status: Option<&UpdateStatus>, dismissed: bool) -> Banner {
    match status {
        Some(UpdateStatus::Available { version, security: true, .. }) => {
            Banner::Security { version: version.to_string() }
        }
        Some(UpdateStatus::Available { .. }) if dismissed => Banner::Hidden,
        Some(UpdateStatus::Available { version, .. }) => Banner::Update { version: version.to_string() },
        Some(UpdateStatus::Stale { expired_at, .. }) => Banner::Stale { since: *expired_at },
        Some(UpdateStatus::Rejected { version, reason, .. }) => Banner::Rejected {
            version: version.to_string(),
            clock: reason == aegis_update::REASON_CLOCK,
        },
        Some(UpdateStatus::None { .. } | UpdateStatus::Error { .. }) | None => Banner::Hidden,
    }
}

/// Text der Hinweisleiste; `None`, wenn sie verborgen ist.
pub fn banner_text(banner: &Banner) -> Option<String> {
    match banner {
        Banner::Hidden => None,
        Banner::Security { version } => Some(format!("Sicherheitsupdate v{version} verfügbar")),
        Banner::Update { version } => Some(format!("Neue Version v{version} verfügbar")),
        Banner::Stale { since } => Some(format!("Update-Status nicht bestätigt – seit {}", format_date(*since))),
        Banner::Rejected { version, clock: false } => {
            Some(format!("Update v{version} konnte nicht verifiziert werden – nicht installiert"))
        }
        Banner::Rejected { version, clock: true } => {
            Some(format!("Update v{version} konnte nicht verifiziert werden – Systemuhr prüfen"))
        }
        Banner::InstallFailed { message } => Some(format!("Update fehlgeschlagen: {message}")),
    }
}

/// Unix-Sekunden als `TT.MM.JJJJ` (UTC).
pub fn format_date(unix_secs: u64) -> String {
    // Tage seit 1970-01-01 in Datum umrechnen (proleptischer gregorianischer Kalender, Algorithmus nach H. Hinnant).
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // Tag in der 400-Jahre-Epoche
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; // Monat ab März
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}.{month:02}.{year}")
}

/// Stammt der Status aus einer Prüfung, die nach ihrem Start lief?
/// Ein Installationsfehler ist nie das Ergebnis einer Prüfung.
pub fn is_fresh(status: &UpdateStatus, started_at: u64) -> bool {
    let install_error = matches!(status, UpdateStatus::Error { during_install: true, .. });
    !install_error && status.checked_at() >= started_at
}

/// `aegis-updater.exe` neben der laufenden `aegis.exe`, nur wenn die Datei existiert.
pub fn updater_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("aegis-updater.exe");
    path.is_file().then_some(path)
}

pub fn spawn_check(updater: &Path) -> std::io::Result<()> {
    spawn(updater, &["--check".to_string()])
}

/// `--install --wait-pid <eigene PID>`: der Updater wartet, bis diese App beendet ist.
pub fn spawn_install(updater: &Path) -> std::io::Result<()> {
    spawn(updater, &["--install".to_string(), "--wait-pid".to_string(), std::process::id().to_string()])
}

/// Startet den Updater direkt (keine Shell, kein PATH) und wartet nicht auf ihn.
fn spawn(updater: &Path, args: &[String]) -> std::io::Result<()> {
    let mut command = Command::new(updater);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: kein Konsolenfenster
    }
    command.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_update::{read_status, Version};

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    fn available(security: bool) -> UpdateStatus {
        UpdateStatus::Available { checked_at: 10, version: v("0.1.1"), security }
    }

    #[test]
    fn banner_hidden_for_none_and_error() {
        assert_eq!(banner_for(None, false), Banner::Hidden);
        let none = UpdateStatus::None { checked_at: 1 };
        assert_eq!(banner_for(Some(&none), false), Banner::Hidden);
        let err = UpdateStatus::Error { checked_at: 1, message: "offline".into(), during_install: false };
        assert_eq!(banner_for(Some(&err), false), Banner::Hidden);
        assert_eq!(banner_for(Some(&err), true), Banner::Hidden);
    }

    #[test]
    fn banner_security_ignores_dismiss() {
        let expected = Banner::Security { version: "0.1.1".into() };
        assert_eq!(banner_for(Some(&available(true)), false), expected);
        assert_eq!(banner_for(Some(&available(true)), true), expected);
    }

    #[test]
    fn banner_update_hidden_when_dismissed() {
        assert_eq!(banner_for(Some(&available(false)), false), Banner::Update { version: "0.1.1".into() });
        assert_eq!(banner_for(Some(&available(false)), true), Banner::Hidden);
    }

    #[test]
    fn banner_stale() {
        let s = UpdateStatus::Stale { checked_at: 5, version: v("0.1.1"), expired_at: 1_700_000_000 };
        assert_eq!(banner_for(Some(&s), false), Banner::Stale { since: 1_700_000_000 });
        assert_eq!(banner_for(Some(&s), true), Banner::Stale { since: 1_700_000_000 });
    }

    #[test]
    fn banner_rejected_ignores_dismiss() {
        let s = UpdateStatus::Rejected { checked_at: 5, version: v("0.1.1"), reason: "Signatur".into() };
        let expected = Banner::Rejected { version: "0.1.1".into(), clock: false };
        assert_eq!(banner_for(Some(&s), false), expected);
        assert_eq!(banner_for(Some(&s), true), expected);
    }

    #[test]
    fn corrupt_status_shows_nothing() {
        let dir = std::env::temp_dir().join(format!("aegis-app-updates-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("update.json");
        std::fs::write(&path, "{ kaputt").unwrap();
        assert_eq!(banner_for(read_status(&path).as_ref(), false), Banner::Hidden);
    }

    #[test]
    fn banner_texts_are_german_and_exact() {
        assert_eq!(banner_text(&Banner::Hidden), None);
        assert_eq!(
            banner_text(&Banner::Security { version: "0.1.1".into() }).as_deref(),
            Some("Sicherheitsupdate v0.1.1 verfügbar")
        );
        assert_eq!(
            banner_text(&Banner::Update { version: "0.1.1".into() }).as_deref(),
            Some("Neue Version v0.1.1 verfügbar")
        );
        assert_eq!(
            banner_text(&Banner::Stale { since: 1_700_000_000 }).as_deref(),
            Some("Update-Status nicht bestätigt – seit 14.11.2023")
        );
        assert_eq!(
            banner_text(&Banner::Rejected { version: "0.1.1".into(), clock: false }).as_deref(),
            Some("Update v0.1.1 konnte nicht verifiziert werden – nicht installiert")
        );
        assert_eq!(
            banner_text(&Banner::Rejected { version: "0.1.1".into(), clock: true }).as_deref(),
            Some("Update v0.1.1 konnte nicht verifiziert werden – Systemuhr prüfen")
        );
        assert_eq!(
            banner_text(&Banner::InstallFailed { message: "Prüfsumme stimmt nicht: aegis.exe".into() }).as_deref(),
            Some("Update fehlgeschlagen: Prüfsumme stimmt nicht: aegis.exe")
        );
    }

    #[test]
    fn banner_rejected_names_clock() {
        let s = UpdateStatus::Rejected {
            checked_at: 5,
            version: v("0.1.1"),
            reason: aegis_update::REASON_CLOCK.into(),
        };
        let expected = Banner::Rejected { version: "0.1.1".into(), clock: true };
        assert_eq!(banner_for(Some(&s), false), expected);
        assert_eq!(banner_for(Some(&s), true), expected);
    }

    fn install_error(checked_at: u64) -> UpdateStatus {
        UpdateStatus::Error { checked_at, message: "Hash".into(), during_install: true }
    }

    #[test]
    fn install_failure_only_for_recent_install_errors() {
        let now = 10_000;
        assert_eq!(install_failure(Some(&install_error(now)), now).as_deref(), Some("Hash"));
        assert_eq!(install_failure(Some(&install_error(now - 600)), now).as_deref(), Some("Hash"));
        assert_eq!(install_failure(Some(&install_error(now - 601)), now), None, "älter als 10 Minuten");
        let check_error = UpdateStatus::Error { checked_at: now, message: "offline".into(), during_install: false };
        assert_eq!(install_failure(Some(&check_error), now), None, "Fehler einer Prüfung bleibt still");
        assert_eq!(install_failure(Some(&UpdateStatus::None { checked_at: now }), now), None);
        assert_eq!(install_failure(Some(&available(true)), now), None);
        assert_eq!(install_failure(None, now), None);
    }

    #[test]
    fn install_failure_banner_comes_first() {
        let security = available(true);
        assert_eq!(
            select_banner(Some("Hash"), Some(&security), false),
            Banner::InstallFailed { message: "Hash".into() }
        );
        // Nach „Schließen“ (kein Installationsfehler mehr) erscheint der normale Hinweis.
        assert_eq!(select_banner(None, Some(&security), false), Banner::Security { version: "0.1.1".into() });
        assert_eq!(select_banner(None, None, false), Banner::Hidden);
    }

    #[test]
    fn manual_check_notice() {
        assert_eq!(
            check_notice(&UpdateStatus::None { checked_at: 1 }),
            Some(("Kein Update verfügbar".to_string(), false))
        );
        let err = UpdateStatus::Error { checked_at: 1, message: "Netzwerkfehler: offline".into(), during_install: false };
        assert_eq!(
            check_notice(&err),
            Some(("Update-Prüfung fehlgeschlagen: Netzwerkfehler: offline".to_string(), true))
        );
        // Diese Ergebnisse zeigt die Hinweisleiste.
        assert_eq!(check_notice(&available(false)), None);
        let rejected = UpdateStatus::Rejected { checked_at: 1, version: v("0.1.1"), reason: "Signatur".into() };
        assert_eq!(check_notice(&rejected), None);
    }

    #[test]
    fn date_formatting() {
        assert_eq!(format_date(0), "01.01.1970");
        assert_eq!(format_date(86_399), "01.01.1970");
        assert_eq!(format_date(86_400), "02.01.1970");
        assert_eq!(format_date(951_782_400), "29.02.2000"); // Schaltjahr (400er-Regel)
        assert_eq!(format_date(1_700_000_000), "14.11.2023");
        assert_eq!(format_date(1_709_164_800), "29.02.2024");
        assert_eq!(format_date(1_709_251_200), "01.03.2024");
        assert_eq!(format_date(4_102_444_799), "31.12.2099");
    }

    #[test]
    fn freshness_is_checked_against_start_time() {
        let s = UpdateStatus::None { checked_at: 100 };
        assert!(!is_fresh(&s, 101));
        assert!(is_fresh(&s, 100));
        assert!(is_fresh(&s, 50));
        // Ein Installationsfehler ist nie das Ergebnis einer Prüfung.
        assert!(!is_fresh(&install_error(100), 50));
    }
}
