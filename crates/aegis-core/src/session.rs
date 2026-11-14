//! Geöffneter Tresor: hält den abgeleiteten Schlüssel, damit Speichern ohne erneute
//! Argon2-Ableitung auskommt. Das Master-Passwort selbst wird nicht gespeichert.

use std::{
    fs,
    path::{Path, PathBuf},
};

use zeroize::Zeroizing;

use crate::{
    crypto::{self, KdfParams, KEY_LEN, NONCE_LEN, SALT_LEN},
    error::AegisError,
    storage,
    vault::{Header, Vault, HEADER_LEN},
};

/// Kein `Debug`, damit der Schlüssel nicht versehentlich in Logs landet.
pub struct Session {
    path: PathBuf,
    header: Header,
    key: Zeroizing<[u8; KEY_LEN]>,
    vault: Vault,
}

impl Session {
    /// Legt einen neuen, leeren Tresor an. Eine vorhandene Datei wird nie überschrieben.
    pub fn create(path: &Path, master: &[u8], params: &KdfParams) -> Result<Session, AegisError> {
        let header = Header {
            params: params.clone(),
            salt: crypto::random_bytes::<SALT_LEN>()?,
            nonce: crypto::random_bytes::<NONCE_LEN>()?,
        };
        let key = crypto::derive_key(master, &header.salt, &header.params)?;
        let vault = Vault::default();
        storage::create_new(path, &encrypt(&key, &header, &vault)?)?;
        Ok(Session { path: path.to_path_buf(), header, key, vault })
    }

    /// Öffnet einen Tresor. Die Parameter im Kopf werden vor der Ableitung geprüft.
    pub fn open(path: &Path, master: &[u8]) -> Result<Session, AegisError> {
        let data = fs::read(path)?;
        let header = Header::parse(&data)?;
        let key = crypto::derive_key(master, &header.salt, &header.params)?;
        let plaintext =
            Zeroizing::new(crypto::open(&key, &header.nonce, &data[..HEADER_LEN], &data[HEADER_LEN..])?);
        let vault = serde_json::from_slice(&plaintext)?;
        Ok(Session { path: path.to_path_buf(), header, key, vault })
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Speichert `candidate` mit neuer Nonce und übernimmt es erst nach erfolgreichem Schreiben.
    pub fn commit(&mut self, candidate: Vault) -> Result<(), AegisError> {
        let header = Header { nonce: crypto::random_bytes::<NONCE_LEN>()?, ..self.header.clone() };
        storage::write_atomic(&self.path, &encrypt(&self.key, &header, &candidate)?)?;
        self.header = header;
        self.vault = candidate;
        Ok(())
    }

    /// Wechselt das Master-Passwort. `current` muss zum gehaltenen Schlüssel passen.
    /// Danach wird auch das Backup ersetzt, damit keine Kopie mit dem alten Passwort bleibt.
    pub fn change_master(&mut self, current: &[u8], new: &[u8], params: &KdfParams) -> Result<(), AegisError> {
        let check = crypto::derive_key(current, &self.header.salt, &self.header.params)?;
        if !constant_time_eq(&check[..], &self.key[..]) {
            return Err(AegisError::WrongPassword);
        }
        let header = Header {
            params: params.clone(),
            salt: crypto::random_bytes::<SALT_LEN>()?,
            nonce: crypto::random_bytes::<NONCE_LEN>()?,
        };
        let key = crypto::derive_key(new, &header.salt, &header.params)?;
        let data = encrypt(&key, &header, &self.vault)?;
        storage::write_atomic(&self.path, &data)?;
        self.header = header;
        self.key = key;
        // Altes Backup durch das neue ersetzen; klappt das nicht, lieber gar kein Backup als eins
        // mit dem alten Passwort. Erst wenn auch Löschen scheitert, geht ein Fehler an die UI.
        let backup = storage::backup_path(&self.path);
        if storage::replace(&backup, &data).is_err() {
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

fn encrypt(key: &[u8; KEY_LEN], header: &Header, vault: &Vault) -> Result<Vec<u8>, AegisError> {
    let encoded = header.encode();
    let plaintext = Zeroizing::new(serde_json::to_vec(vault)?);
    let mut file = encoded.to_vec();
    file.extend_from_slice(&crypto::seal(key, &header.nonce, &encoded, &plaintext)?);
    Ok(file)
}

/// Vergleich ohne frühen Abbruch, damit die Laufzeit nichts über den Schlüssel verrät.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{derive_key, seal},
        storage::{backup_path, tests::temp_dir},
        vault::{Entry, Header},
    };
    use std::fs;

    fn fast() -> KdfParams {
        KdfParams { m_cost: 8, t_cost: 1, p_cost: 1 }
    }

    fn entry(title: &str) -> Entry {
        Entry { title: title.into(), username: "fabian".into(), password: "geheim".into(), notes: String::new() }
    }

    fn with_entry(title: &str) -> Vault {
        Vault { entries: vec![entry(title)] }
    }

    #[test]
    fn create_then_open_roundtrip() {
        let path = temp_dir("s_roundtrip").join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        s.commit(with_entry("GitHub")).unwrap();
        drop(s);
        let s = Session::open(&path, b"master").unwrap();
        assert_eq!(s.vault().entries[0].title, "GitHub");
        assert_eq!(s.path(), path.as_path());
    }

    #[test]
    fn empty_vault_roundtrip() {
        let path = temp_dir("s_empty").join("vault.aegis");
        drop(Session::create(&path, b"master", &fast()).unwrap());
        assert!(Session::open(&path, b"master").unwrap().vault().entries.is_empty());
    }

    #[test]
    fn create_refuses_existing_file() {
        let path = temp_dir("s_exists").join("vault.aegis");
        fs::write(&path, b"x").unwrap();
        assert!(matches!(Session::create(&path, b"master", &fast()), Err(AegisError::AlreadyExists)));
        assert_eq!(fs::read(&path).unwrap(), b"x");
    }

    #[test]
    fn wrong_password_fails() {
        let path = temp_dir("s_wrong").join("vault.aegis");
        drop(Session::create(&path, b"master", &fast()).unwrap());
        assert!(matches!(Session::open(&path, b"falsch"), Err(AegisError::Crypto)));
    }

    #[test]
    fn tampered_file_fails() {
        let path = temp_dir("s_tamper").join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        s.commit(with_entry("A")).unwrap();
        drop(s);
        let mut data = fs::read(&path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0x01;
        fs::write(&path, &data).unwrap();
        assert!(matches!(Session::open(&path, b"master"), Err(AegisError::Crypto)));
    }

    #[test]
    fn commit_keeps_salt_changes_nonce() {
        let path = temp_dir("s_nonce").join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        let before = fs::read(&path).unwrap();
        s.commit(with_entry("A")).unwrap();
        let after = fs::read(&path).unwrap();
        assert_eq!(before[17..33], after[17..33]);
        assert_ne!(before[33..57], after[33..57]);
    }

    #[test]
    fn failed_commit_keeps_old_vault() {
        let dir = temp_dir("s_fail");
        let path = dir.join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(s.commit(with_entry("A")).is_err());
        assert!(s.vault().entries.is_empty());
    }

    #[test]
    fn change_master_with_wrong_current_fails() {
        let path = temp_dir("s_cm_wrong").join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            s.change_master(b"falsch", b"neues-passwort", &fast()),
            Err(AegisError::WrongPassword)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn change_master_switches_password_for_file_and_backup() {
        let path = temp_dir("s_cm").join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        s.commit(with_entry("A")).unwrap();
        s.change_master(b"master", b"neues-passwort", &fast()).unwrap();
        // Weiter speichern funktioniert mit dem neuen Schlüssel.
        s.commit(with_entry("B")).unwrap();
        drop(s);
        for p in [path.clone(), backup_path(&path)] {
            assert!(Session::open(&p, b"master").is_err(), "{}", p.display());
            assert!(Session::open(&p, b"neues-passwort").is_ok(), "{}", p.display());
        }
        assert_eq!(Session::open(&path, b"neues-passwort").unwrap().vault().entries[0].title, "B");
    }

    #[test]
    fn change_master_leaves_no_copy_with_old_password() {
        let dir = temp_dir("s_cm_copies");
        let path = dir.join("vault.aegis");
        let mut s = Session::create(&path, b"master", &fast()).unwrap();
        s.commit(with_entry("A")).unwrap();
        s.commit(with_entry("B")).unwrap();
        s.change_master(b"master", b"neues-passwort", &fast()).unwrap();
        drop(s);
        for e in fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            assert!(Session::open(&p, b"master").is_err(), "alte Kopie: {}", p.display());
        }
    }

    #[test]
    fn opens_v1_file_written_like_v0_0_8() {
        let path = temp_dir("s_v1").join("vault.aegis");
        let header = Header { params: fast(), salt: [7; 16], nonce: [9; 24] };
        let encoded = header.encode();
        let key = derive_key(b"master", &header.salt, &header.params).unwrap();
        let json = serde_json::to_vec(&with_entry("Alt")).unwrap();
        let mut file = encoded.to_vec();
        file.extend_from_slice(&seal(&key, &header.nonce, &encoded, &json).unwrap());
        fs::write(&path, file).unwrap();
        assert_eq!(Session::open(&path, b"master").unwrap().vault().entries[0].title, "Alt");
    }
}
