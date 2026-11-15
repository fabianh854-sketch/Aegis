//! Manifest bauen, erneuern, signieren und prüfen (ohne Dateizugriff, daher testbar).

use aegis_update::{sha256_hex, verify, FileEntry, Manifest, Version, VALIDITY_SECS};
use minisign::{PublicKey, SecretKey};
use std::io::Cursor;

/// Baut das Manifest für eine neue Version.
/// `min_secure` wird bei Sicherheitsupdates die neue Version, sonst vom Vorgänger übernommen.
pub fn build_manifest(
    version: Version,
    security: bool,
    previous: Option<&Manifest>,
    files: &[(&str, &[u8])],
    now: u64,
) -> Result<Manifest, String> {
    if let Some(p) = previous {
        if version <= p.version {
            return Err(format!(
                "Die Version {version} muss größer sein als die vorherige {}.",
                p.version
            ));
        }
    }
    let min_secure = if security {
        version
    } else {
        previous.map_or(
            Version {
                major: 0,
                minor: 0,
                patch: 0,
            },
            |p| p.min_secure,
        )
    };
    if min_secure > version {
        return Err(format!(
            "min_secure {min_secure} wäre größer als die Version {version}."
        ));
    }
    Ok(Manifest {
        schema: 1,
        version,
        security,
        min_secure,
        issued_at: now,
        expires_at: now
            .checked_add(VALIDITY_SECS)
            .ok_or("Zeitstempel zu groß.")?,
        files: files
            .iter()
            .map(|(name, bytes)| FileEntry {
                name: (*name).to_string(),
                sha256: sha256_hex(bytes),
            })
            .collect(),
    })
}

/// Neues Manifest mit unverändertem Inhalt; nur `issued_at`/`expires_at` ändern sich.
pub fn refresh_manifest(old: &Manifest, now: u64) -> Manifest {
    Manifest {
        issued_at: now,
        expires_at: now.saturating_add(VALIDITY_SECS),
        ..old.clone()
    }
}

/// Signiert genau `data` (prehashed) mit vertrauenswürdigem Kommentar `aegis-manifest v=<version>`.
pub fn sign_bytes(
    sk: &SecretKey,
    pk: &PublicKey,
    data: &[u8],
    version: Version,
) -> Result<String, String> {
    let comment = format!("aegis-manifest v={version}");
    minisign::sign(Some(pk), sk, Cursor::new(data), Some(&comment), None)
        .map(|sig| sig.into_string())
        .map_err(|e| format!("Signieren fehlgeschlagen: {e}"))
}

/// Fehlermeldung bei leerer Schlüsselliste.
pub const NO_KEY_MSG: &str =
    "Kein vertrauenswürdiger Schlüssel in aegis-update/src/keys.rs eingetragen.";

/// Prüft Manifest und Signatur wie der Updater. Mit leerer Schlüsselliste schlägt sie fehl.
pub fn self_check(json: &[u8], sig: &str, keys: &[&str], now: u64) -> Result<Manifest, String> {
    if keys.is_empty() {
        return Err(NO_KEY_MSG.to_string());
    }
    verify(json, sig, keys, now).map_err(|e| format!("Selbstprüfung fehlgeschlagen: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::KeyPair;

    const NOW: u64 = 1_800_000_000;

    fn v(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    fn files() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("aegis.exe", b"app-bytes"),
            ("aegis-updater.exe", b"updater-bytes"),
        ]
    }

    fn previous() -> Manifest {
        build_manifest(v(0, 2, 0), true, None, &files(), NOW - 1000).unwrap()
    }

    #[test]
    fn security_sets_min_secure_to_version() {
        let m = build_manifest(v(0, 3, 0), true, Some(&previous()), &files(), NOW).unwrap();
        assert_eq!(m.version, v(0, 3, 0));
        assert!(m.security);
        assert_eq!(m.min_secure, v(0, 3, 0));
        assert_eq!(m.schema, 1);
        assert_eq!(m.issued_at, NOW);
        assert_eq!(m.expires_at, NOW + VALIDITY_SECS);
        assert_eq!(m.files[0].name, "aegis.exe");
        assert_eq!(m.files[0].sha256, sha256_hex(b"app-bytes"));
        assert_eq!(m.files[1].sha256, sha256_hex(b"updater-bytes"));
    }

    #[test]
    fn non_security_carries_previous_min_secure() {
        let prev = previous();
        assert_eq!(prev.min_secure, v(0, 2, 0));
        let m = build_manifest(v(0, 2, 1), false, Some(&prev), &files(), NOW).unwrap();
        assert!(!m.security);
        assert_eq!(m.min_secure, v(0, 2, 0));
        let first = build_manifest(v(0, 1, 0), false, None, &files(), NOW).unwrap();
        assert_eq!(first.min_secure, v(0, 0, 0));
    }

    #[test]
    fn version_must_increase() {
        let prev = previous();
        assert!(build_manifest(v(0, 2, 0), false, Some(&prev), &files(), NOW).is_err());
        assert!(build_manifest(v(0, 1, 9), true, Some(&prev), &files(), NOW).is_err());
    }

    #[test]
    fn min_secure_above_version_is_rejected() {
        let mut prev = previous();
        prev.min_secure = v(9, 0, 0);
        assert!(build_manifest(v(0, 3, 0), false, Some(&prev), &files(), NOW).is_err());
    }

    #[test]
    fn refresh_changes_only_times() {
        let old = previous();
        let later = NOW + 3 * 24 * 3600;
        let new = refresh_manifest(&old, later);
        assert_eq!(new.issued_at, later);
        assert_eq!(new.expires_at, later + VALIDITY_SECS);
        let mut expected = old.clone();
        expected.issued_at = later;
        expected.expires_at = later + VALIDITY_SECS;
        assert_eq!(new, expected);
    }

    #[test]
    fn signed_by_tool_verifies() {
        let kp = KeyPair::generate_unencrypted_keypair().unwrap();
        let m = build_manifest(v(0, 3, 0), true, None, &files(), NOW).unwrap();
        let json = serde_json::to_vec(&m).unwrap();
        let sig = sign_bytes(&kp.sk, &kp.pk, &json, m.version).unwrap();
        let pk = kp.pk.to_base64();
        assert_eq!(verify(&json, &sig, &[&pk], NOW).unwrap(), m);
        assert_eq!(self_check(&json, &sig, &[&pk], NOW).unwrap(), m);
        assert!(sig.contains("aegis-manifest v=0.3.0"));
        assert!(minisign::SignatureBox::from_string(&sig)
            .unwrap()
            .is_prehashed());
        let mut tampered = json.clone();
        tampered.push(b' ');
        assert!(self_check(&tampered, &sig, &[&pk], NOW).is_err());
    }

    #[test]
    fn self_check_fails_closed_without_keys() {
        let err = self_check(b"{}", "x", &[], NOW).unwrap_err();
        assert!(err
            .contains("Kein vertrauenswürdiger Schlüssel in aegis-update/src/keys.rs eingetragen"));
    }
}
