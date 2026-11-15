//! Gemeinsame Test-Helfer: Fake-Netz, Testschlüssel, Temp-Ordner.

use std::{
    cell::RefCell,
    collections::HashMap,
    io::Cursor,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use aegis_update::{sha256_hex, FileEntry, Manifest, UpdateError, Version, VALIDITY_SECS};
use serde_json::{json, Value};

use crate::{
    fetch::Fetch,
    github::{DOWNLOAD_PREFIX, MANIFEST_ASSET, RELEASES_URL, SIGNATURE_ASSET},
};

pub const NOW: u64 = 1_800_000_000;
pub const APP: &[u8] = b"neue aegis.exe";
pub const UPDATER: &[u8] = b"neuer aegis-updater.exe";

/// Netz aus einer Tabelle URL → Inhalt; unbekannte URL ergibt einen Netzfehler.
#[derive(Default)]
pub struct FakeFetch {
    pub files: HashMap<String, Vec<u8>>,
}

impl Fetch for FakeFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        self.files
            .get(url)
            .cloned()
            .ok_or_else(|| UpdateError::Network(format!("404: {url}")))
    }
}

/// Zeichnet alle angefragten URLs auf.
pub struct CountingFetch<'a> {
    inner: &'a FakeFetch,
    urls: RefCell<Vec<String>>,
}

impl<'a> CountingFetch<'a> {
    pub fn new(inner: &'a FakeFetch) -> Self {
        Self {
            inner,
            urls: RefCell::new(Vec::new()),
        }
    }

    /// Angefragte URLs, deren Dateiname `name` ist.
    pub fn requested(&self, name: &str) -> Vec<String> {
        let suffix = format!("/{name}");
        self.urls
            .borrow()
            .iter()
            .filter(|u| u.ends_with(&suffix))
            .cloned()
            .collect()
    }
}

impl Fetch for CountingFetch<'_> {
    fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        self.urls.borrow_mut().push(url.to_string());
        self.inner.get(url)
    }
}

pub fn v(s: &str) -> Version {
    s.parse().unwrap()
}

pub fn asset_url(tag: &str, name: &str) -> String {
    format!("{DOWNLOAD_PREFIX}{tag}/{name}")
}

/// Minisign-Testschlüssel.
pub struct Key(minisign::KeyPair);

impl Key {
    pub fn new() -> Self {
        Self(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
    }

    pub fn public(&self) -> String {
        self.0.pk.to_base64()
    }

    pub fn sign(&self, data: &[u8]) -> String {
        minisign::sign(Some(&self.0.pk), &self.0.sk, Cursor::new(data), None, None)
            .unwrap()
            .to_string()
    }
}

/// Gültiges Manifest für `APP` und `UPDATER`.
pub fn test_manifest(version: &str) -> Manifest {
    Manifest {
        schema: 1,
        version: v(version),
        security: false,
        min_secure: v("0.0.0"),
        issued_at: NOW,
        expires_at: NOW + VALIDITY_SECS,
        files: vec![
            FileEntry {
                name: "aegis.exe".into(),
                sha256: sha256_hex(APP),
            },
            FileEntry {
                name: "aegis-updater.exe".into(),
                sha256: sha256_hex(UPDATER),
            },
        ],
    }
}

/// Ein Eintrag der GitHub-API-Antwort.
pub fn release_entry(tag: &str, draft: bool, prerelease: bool, assets: &[(&str, String)]) -> Value {
    let assets: Vec<Value> = assets
        .iter()
        .map(|(name, url)| json!({ "name": name, "browser_download_url": url, "size": 1 }))
        .collect();
    json!({
        "tag_name": tag,
        "name": format!("Aegis {tag}"),
        "body": "Release-Notes sind nicht vertrauenswürdig",
        "draft": draft,
        "prerelease": prerelease,
        "assets": assets,
    })
}

/// Veröffentlicht ein vollständiges, mit `key` signiertes Release im Fake-Netz.
pub fn publish(fetch: &mut FakeFetch, key: &Key, tag: &str) -> Value {
    let manifest = serde_json::to_vec(&test_manifest(tag.trim_start_matches('v'))).unwrap();
    let sig = key.sign(&manifest);
    let files = [
        (MANIFEST_ASSET, manifest),
        (SIGNATURE_ASSET, sig.into_bytes()),
        ("aegis.exe", APP.to_vec()),
        ("aegis-updater.exe", UPDATER.to_vec()),
    ];
    let mut assets = Vec::new();
    for (name, data) in files {
        let url = asset_url(tag, name);
        fetch.files.insert(url.clone(), data);
        assets.push((name, url));
    }
    release_entry(tag, false, false, &assets)
}

pub fn set_releases(fetch: &mut FakeFetch, entries: Vec<Value>) {
    fetch.files.insert(
        RELEASES_URL.to_string(),
        serde_json::to_vec(&entries).unwrap(),
    );
}

/// Eindeutiger, leerer Temp-Ordner.
pub fn temp_dir(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "aegis-updater-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Verweigert per ACL allen („Jeder“, S-1-1-0) das Anlegen von Dateien im Ordner; Drop hebt es auf.
#[cfg(windows)]
pub struct DenyWrite(PathBuf);

#[cfg(windows)]
impl DenyWrite {
    const ICACLS: &'static str = r"C:\Windows\System32\icacls.exe";

    pub fn new(dir: &std::path::Path) -> Self {
        let status = std::process::Command::new(Self::ICACLS)
            .arg(dir)
            .args(["/deny", "*S-1-1-0:(WD)"])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "icacls /deny fehlgeschlagen");
        // Gegenprobe: Der Ordner ist jetzt wirklich nicht beschreibbar.
        assert!(std::fs::write(dir.join("probe"), b"x").is_err());
        Self(dir.to_path_buf())
    }
}

#[cfg(windows)]
impl Drop for DenyWrite {
    fn drop(&mut self) {
        let _ = std::process::Command::new(Self::ICACLS)
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .stdout(std::process::Stdio::null())
            .status();
    }
}
