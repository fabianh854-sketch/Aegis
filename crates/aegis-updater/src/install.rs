//! Herunterladen, Prüfen und Ersetzen von `aegis.exe` und `aegis-updater.exe`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use aegis_update::{sha256_hex, FileEntry, Manifest, UpdateError};

use crate::{
    fetch::Fetch,
    github::{is_allowed_url, ReleaseRef},
};

/// Die einzigen Dateien, die der Updater jemals schreibt oder ersetzt.
const FILES: [&str; 2] = ["aegis.exe", "aegis-updater.exe"];

fn io_err(context: &str, e: io::Error) -> UpdateError {
    UpdateError::Io(format!("{context}: {e}"))
}

/// Zweite Sicherung neben `verify`: nur die festen Dateinamen, nie Pfade.
fn checked_name(entry: &FileEntry) -> Result<&str, UpdateError> {
    FILES
        .iter()
        .find(|n| **n == entry.name)
        .copied()
        .ok_or_else(|| UpdateError::BadFormat(format!("unerlaubter Dateiname {:?}", entry.name)))
}

fn with_suffix(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{name}{suffix}"))
}

/// Liest eine Datei und vergleicht ihre SHA-256 mit dem Manifest.
fn verify_file(path: &Path, entry: &FileEntry) -> Result<(), UpdateError> {
    let data = fs::read(path).map_err(|e| io_err(&format!("{} lesen", path.display()), e))?;
    if sha256_hex(&data) != entry.sha256 {
        return Err(UpdateError::Hash(entry.name.clone()));
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), UpdateError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(io_err(&format!("{} löschen", path.display()), e))
        }
        _ => Ok(()),
    }
}

/// Lädt alle Dateien des Manifests nach `staging`; jede Datei wird vor dem Schreiben per SHA-256 geprüft.
pub fn download_verified(
    fetch: &dyn Fetch,
    release: &ReleaseRef,
    manifest: &Manifest,
    staging: &Path,
) -> Result<(), UpdateError> {
    if release.tag != manifest.version {
        return Err(UpdateError::WrongVersion);
    }
    // Frischer Staging-Ordner: keine Reste früherer Versuche.
    match fs::remove_dir_all(staging) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(io_err("Staging-Ordner leeren", e))
        }
        _ => {}
    }
    fs::create_dir_all(staging).map_err(|e| io_err("Staging-Ordner anlegen", e))?;

    let result = manifest.files.iter().try_for_each(|entry| {
        let name = checked_name(entry)?;
        let url = release
            .assets
            .get(name)
            .filter(|url| is_allowed_url(url))
            .ok_or_else(|| UpdateError::BadFormat(format!("{name} fehlt im Release")))?;
        let data = fetch.get(url)?;
        if sha256_hex(&data) != entry.sha256 {
            return Err(UpdateError::Hash(name.to_string()));
        }
        fs::write(staging.join(name), &data).map_err(|e| io_err(&format!("{name} speichern"), e))
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(staging); // Aufräumen; der eigentliche Fehler geht an den Aufrufer
    }
    result
}

/// Eine in Schritt 3 von [`replace_files`] bearbeitete Datei, für das Zurückrollen.
struct Swapped {
    current: PathBuf,
    /// Die alte Datei liegt jetzt unter `<name>.old`.
    old: Option<PathBuf>,
    /// Die neue Datei liegt schon unter `<name>`.
    placed: bool,
}

/// Ersetzt die Dateien in `target` durch die geprüften aus `staging`; bei Fehler bleibt der alte Stand.
///
/// 1. SHA-256 der Dateien im Staging erneut prüfen.
/// 2. Nach `target\<name>.new` kopieren (auch über Laufwerksgrenzen) und dort erneut prüfen.
/// 3. `target\<name>` → `<name>.old`, `<name>.new` → `<name>`; jede Störung rollt alles zurück.
pub fn replace_files(
    staging: &Path,
    target: &Path,
    manifest: &Manifest,
) -> Result<(), UpdateError> {
    for entry in &manifest.files {
        verify_file(&staging.join(checked_name(entry)?), entry)?;
    }

    let mut new_files = Vec::new();
    let copied = manifest.files.iter().try_for_each(|entry| {
        let name = checked_name(entry)?;
        let new = with_suffix(target, name, ".new");
        remove_if_exists(&new)?;
        new_files.push(new.clone());
        fs::copy(staging.join(name), &new)
            .map_err(|e| io_err(&format!("{name} in den Zielordner kopieren"), e))?;
        verify_file(&new, entry)
    });
    if let Err(e) = copied {
        remove_all(&new_files);
        return Err(e);
    }

    let mut swapped: Vec<Swapped> = Vec::new();
    let result = manifest.files.iter().try_for_each(|entry| {
        let name = checked_name(entry)?;
        let current = target.join(name);
        let old = with_suffix(target, name, ".old");
        let new = with_suffix(target, name, ".new");
        remove_if_exists(&old)?;
        let old = match fs::rename(&current, &old) {
            Ok(()) => Some(old),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_err(&format!("{name} beiseitelegen"), e)),
        };
        swapped.push(Swapped {
            current: current.clone(),
            old,
            placed: false,
        });
        fs::rename(&new, &current).map_err(|e| io_err(&format!("{name} einsetzen"), e))?;
        if let Some(last) = swapped.last_mut() {
            last.placed = true;
        }
        Ok(())
    });

    if let Err(e) = result {
        let rollback = roll_back(&swapped);
        remove_all(&new_files);
        return Err(match rollback {
            Ok(()) => e,
            Err(r) => UpdateError::Io(format!("{e}; Zurückrollen fehlgeschlagen: {r}")),
        });
    }
    Ok(())
}

/// Stellt die alten Dateien wieder her, in umgekehrter Reihenfolge.
fn roll_back(swapped: &[Swapped]) -> Result<(), UpdateError> {
    let mut first_error = None;
    for s in swapped.iter().rev() {
        let step = match &s.old {
            // `rename` ersetzt unter Windows eine vorhandene neue Datei.
            Some(old) => fs::rename(old, &s.current)
                .map_err(|e| io_err(&format!("{} wiederherstellen", s.current.display()), e)),
            None if s.placed => remove_if_exists(&s.current),
            None => Ok(()),
        };
        if let Err(e) = step {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn remove_all(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path); // bestes Bemühen; Reste räumt cleanup_old beim nächsten Lauf
    }
}

/// Löscht Reste früherer Updates (`*.old`, `*.new` der festen Dateinamen).
pub fn cleanup_old(target: &Path) {
    for name in FILES {
        for suffix in [".old", ".new"] {
            // Kann scheitern, solange die alte Datei noch läuft; dann beim nächsten Mal.
            let _ = fs::remove_file(with_suffix(target, name, suffix));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use std::{collections::HashMap, fs, path::PathBuf};

    const OLD_APP: &[u8] = b"alte aegis.exe";
    const OLD_UPDATER: &[u8] = b"alter aegis-updater.exe";

    fn release(tag: &str) -> ReleaseRef {
        ReleaseRef {
            tag: v(tag.trim_start_matches('v')),
            assets: HashMap::from([
                ("aegis.exe".to_string(), asset_url(tag, "aegis.exe")),
                (
                    "aegis-updater.exe".to_string(),
                    asset_url(tag, "aegis-updater.exe"),
                ),
            ]),
        }
    }

    /// Ziel mit alter Installation, Staging mit neuen Dateien.
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let root = temp_dir(name);
        let target = root.join("ziel");
        let staging = root.join("staging");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(target.join("aegis.exe"), OLD_APP).unwrap();
        fs::write(target.join("aegis-updater.exe"), OLD_UPDATER).unwrap();
        fs::write(staging.join("aegis.exe"), APP).unwrap();
        fs::write(staging.join("aegis-updater.exe"), UPDATER).unwrap();
        (staging, target)
    }

    fn assert_old_state(target: &Path) {
        assert_eq!(fs::read(target.join("aegis.exe")).unwrap(), OLD_APP);
        assert_eq!(
            fs::read(target.join("aegis-updater.exe")).unwrap(),
            OLD_UPDATER
        );
        let mut names: Vec<_> = fs::read_dir(target)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["aegis-updater.exe", "aegis.exe"], "keine Reste");
    }

    #[test]
    fn download_writes_verified_files() {
        let mut fetch = FakeFetch::default();
        publish(&mut fetch, &Key::new(), "v0.2.0");
        let staging = temp_dir("download-ok").join("staging");
        download_verified(
            &fetch,
            &release("v0.2.0"),
            &test_manifest("0.2.0"),
            &staging,
        )
        .unwrap();
        assert_eq!(fs::read(staging.join("aegis.exe")).unwrap(), APP);
        assert_eq!(
            fs::read(staging.join("aegis-updater.exe")).unwrap(),
            UPDATER
        );
    }

    #[test]
    fn download_rejects_hash_mismatch() {
        let mut fetch = FakeFetch::default();
        publish(&mut fetch, &Key::new(), "v0.2.0");
        fetch.files.insert(
            asset_url("v0.2.0", "aegis-updater.exe"),
            b"manipuliert".to_vec(),
        );
        let staging = temp_dir("download-hash").join("staging");
        let result = download_verified(
            &fetch,
            &release("v0.2.0"),
            &test_manifest("0.2.0"),
            &staging,
        );
        assert!(matches!(result, Err(UpdateError::Hash(_))), "{result:?}");
        assert!(
            !staging.join("aegis-updater.exe").exists(),
            "ungeprüfte Datei darf nicht liegen bleiben"
        );
    }

    #[test]
    fn download_rejects_foreign_url_and_wrong_tag() {
        let mut fetch = FakeFetch::default();
        publish(&mut fetch, &Key::new(), "v0.2.0");
        let staging = temp_dir("download-url").join("staging");

        let mut foreign = release("v0.2.0");
        let evil = "https://evil.example/aegis.exe".to_string();
        fetch.files.insert(evil.clone(), APP.to_vec());
        foreign.assets.insert("aegis.exe".into(), evil);
        assert!(download_verified(&fetch, &foreign, &test_manifest("0.2.0"), &staging).is_err());

        assert_eq!(
            download_verified(
                &fetch,
                &release("v0.2.0"),
                &test_manifest("0.3.0"),
                &staging
            ),
            Err(UpdateError::WrongVersion)
        );
    }

    #[test]
    fn replace_files_swaps_both() {
        let (staging, target) = setup("swap");
        replace_files(&staging, &target, &test_manifest("0.2.0")).unwrap();
        assert_eq!(fs::read(target.join("aegis.exe")).unwrap(), APP);
        assert_eq!(fs::read(target.join("aegis-updater.exe")).unwrap(), UPDATER);
        assert_eq!(fs::read(target.join("aegis.exe.old")).unwrap(), OLD_APP);
        assert_eq!(
            fs::read(target.join("aegis-updater.exe.old")).unwrap(),
            OLD_UPDATER
        );
        assert!(!target.join("aegis.exe.new").exists());
        assert!(!target.join("aegis-updater.exe.new").exists());
    }

    #[test]
    fn replace_rejects_tampered_staging() {
        let (staging, target) = setup("tampered");
        fs::write(staging.join("aegis.exe"), b"nach dem Download vertauscht").unwrap();
        let result = replace_files(&staging, &target, &test_manifest("0.2.0"));
        assert!(matches!(result, Err(UpdateError::Hash(_))), "{result:?}");
        assert_old_state(&target);
    }

    #[cfg(windows)]
    #[test]
    fn replace_rolls_back_when_target_locked() {
        use std::os::windows::fs::OpenOptionsExt;
        let (staging, target) = setup("locked");
        // Offener Schreib-Handle ohne Freigaben: Umbenennen von aegis-updater.exe scheitert.
        let lock = fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(target.join("aegis-updater.exe"))
            .unwrap();
        let result = replace_files(&staging, &target, &test_manifest("0.2.0"));
        drop(lock);
        assert!(matches!(result, Err(UpdateError::Io(_))), "{result:?}");
        assert_old_state(&target);
    }

    #[cfg(windows)]
    #[test]
    fn readonly_target_keeps_old_files() {
        let (staging, target) = setup("readonly");
        // Zielordner nicht beschreibbar: ACL verweigert allen das Anlegen von Dateien.
        let _guard = DenyWrite::new(&target);
        let result = replace_files(&staging, &target, &test_manifest("0.2.0"));
        drop(_guard);
        assert!(matches!(result, Err(UpdateError::Io(_))), "{result:?}");
        assert_old_state(&target);
    }

    #[test]
    fn cleanup_removes_old_files() {
        let (_, target) = setup("cleanup");
        for name in [
            "aegis.exe.old",
            "aegis-updater.exe.old",
            "aegis.exe.new",
            "aegis-updater.exe.new",
            "fremd.old",
        ] {
            fs::write(target.join(name), b"x").unwrap();
        }
        cleanup_old(&target);
        for name in [
            "aegis.exe.old",
            "aegis-updater.exe.old",
            "aegis.exe.new",
            "aegis-updater.exe.new",
        ] {
            assert!(!target.join(name).exists(), "{name} muss weg sein");
        }
        assert!(target.join("fremd.old").exists(), "fremde Dateien bleiben");
        assert_eq!(fs::read(target.join("aegis.exe")).unwrap(), OLD_APP);
        // Leerer oder fehlender Ordner: kein Fehler, keine Panik.
        cleanup_old(&target.join("fehlt"));
    }
}
