use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{crypto, error::AegisError};

/// `vault.aegis` → `vault.aegis.bak`
pub(crate) fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

/// Schreibt `data` in eine neue Datei mit zufälligem Namen neben `path` und sichert sie auf Platte.
fn write_temp(path: &Path, data: &[u8]) -> Result<PathBuf, AegisError> {
    let tag: [u8; 8] = crypto::random_bytes()?;
    let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{hex}.tmp"));
    let tmp = PathBuf::from(name);

    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()
    })();
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = fs::remove_file(&tmp); // Aufräumen; der eigentliche Fehler geht an den Aufrufer
            Err(e.into())
        }
    }
}

/// Schreibt atomar: neue Temp-Datei, alte Datei als `.bak` sichern, dann Temp-Datei umbenennen.
/// Bei einem Absturz bleibt entweder die alte oder die neue Datei. Jeder Fehler bricht ab.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<(), AegisError> {
    let tmp = write_temp(path, data)?;
    let result = (|| -> io::Result<()> {
        if path.exists() {
            fs::copy(path, backup_path(path))?;
        }
        fs::rename(&tmp, path) // unter Windows wird die Zieldatei ersetzt
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Legt `path` neu an. Existiert die Datei schon, wird nichts geschrieben (`AlreadyExists`).
pub(crate) fn create_new(path: &Path, data: &[u8]) -> Result<(), AegisError> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(AegisError::AlreadyExists),
        Err(e) => return Err(e.into()),
    };
    let result = file.write_all(data).and_then(|()| file.sync_all());
    if let Err(e) = result {
        drop(file);
        let _ = fs::remove_file(path); // halbe Datei nicht liegen lassen
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Eigenes, leeres Verzeichnis je Test, damit parallele Läufe sich nicht stören.
    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let tag: [u8; 8] = crate::crypto::random_bytes().unwrap();
        let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("aegis_{name}_{hex}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_write_creates_no_backup() {
        let path = temp_dir("first").join("vault.aegis");
        write_atomic(&path, b"a").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a");
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn second_write_keeps_previous_as_backup() {
        let path = temp_dir("second").join("vault.aegis");
        write_atomic(&path, b"a").unwrap();
        write_atomic(&path, b"b").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"b");
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), b"a");
        assert_eq!(backup_path(&path).file_name().unwrap(), "vault.aegis.bak");
    }

    #[test]
    fn no_tmp_files_left_behind() {
        let dir = temp_dir("notmp");
        let path = dir.join("vault.aegis");
        write_atomic(&path, b"a").unwrap();
        write_atomic(&path, b"b").unwrap();
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn create_new_refuses_existing() {
        let path = temp_dir("createnew").join("vault.aegis");
        std::fs::write(&path, b"alt").unwrap();
        assert!(matches!(create_new(&path, b"neu"), Err(AegisError::AlreadyExists)));
        assert_eq!(std::fs::read(&path).unwrap(), b"alt");
    }

    #[test]
    fn create_new_writes_fresh_file() {
        let path = temp_dir("createfresh").join("vault.aegis");
        create_new(&path, b"neu").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"neu");
    }
}
