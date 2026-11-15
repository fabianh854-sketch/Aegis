//! Signiertes Update-Manifest: Format, Prüfung und Entscheidung über das Angebot.

use crate::version::Version;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Gültigkeit eines Manifests ab Ausstellung: genau 7 Tage.
pub const VALIDITY_SECS: u64 = 7 * 24 * 3600;
/// Toleranz für eine vorgehende Uhr: ein Tag.
const MAX_CLOCK_SKEW_SECS: u64 = 86400;
/// Einzige erlaubte Dateien im Manifest, je genau einmal.
const ALLOWED_FILES: [&str; 2] = ["aegis.exe", "aegis-updater.exe"];

/// Fehler bei Prüfung, Download und Installation von Updates.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum UpdateError {
    #[error("Die Signatur ist ungültig.")]
    BadSignature,
    #[error("Es ist kein vertrauenswürdiger Schlüssel hinterlegt.")]
    NoTrustedKey,
    #[error("Ungültiges Format: {0}")]
    BadFormat(String),
    #[error("Das Manifest ist abgelaufen.")]
    Expired,
    #[error("Das Manifest ist noch nicht gültig.")]
    NotYetValid,
    #[error("Die Version im Manifest passt nicht zur veröffentlichten Version.")]
    WrongVersion,
    #[error("Prüfsumme stimmt nicht: {0}")]
    Hash(String),
    #[error("Dateifehler: {0}")]
    Io(String),
    #[error("Netzwerkfehler: {0}")]
    Network(String),
}

/// Eine Datei des Releases mit erwarteter SHA-256-Prüfsumme.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub name: String,
    /// 64 Kleinbuchstaben-Hex.
    pub sha256: String,
}

/// Der signierte Inhalt von `manifest.json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub version: Version,
    pub security: bool,
    pub min_secure: Version,
    /// Unix-Sekunden.
    pub issued_at: u64,
    pub expires_at: u64,
    pub files: Vec<FileEntry>,
}

/// SHA-256 als 64 Kleinbuchstaben-Hex.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn bad_format(msg: impl Into<String>) -> UpdateError {
    UpdateError::BadFormat(msg.into())
}

/// Prüft Signatur (gegen jeden Schlüssel in `keys`), Schema, Dateien und Zeiten.
/// Den Ablauf (`now > expires_at`) prüft sie nicht; das entscheidet [`decide`].
pub fn verify(json: &[u8], sig: &str, keys: &[&str], now: u64) -> Result<Manifest, UpdateError> {
    if keys.is_empty() {
        return Err(UpdateError::NoTrustedKey);
    }
    let signature = Signature::decode(sig).map_err(|_| UpdateError::BadSignature)?;
    let signed = keys.iter().any(|key| {
        PublicKey::from_base64(key)
            .and_then(|pk| pk.verify(json, &signature, false))
            .is_ok()
    });
    if !signed {
        return Err(UpdateError::BadSignature);
    }

    let manifest: Manifest = serde_json::from_slice(json).map_err(|e| bad_format(e.to_string()))?;
    if manifest.schema != 1 {
        return Err(bad_format(format!(
            "unbekanntes Schema {}",
            manifest.schema
        )));
    }

    let names_ok = manifest.files.len() == ALLOWED_FILES.len()
        && ALLOWED_FILES
            .iter()
            .all(|n| manifest.files.iter().filter(|f| f.name == *n).count() == 1);
    if !names_ok {
        return Err(bad_format(
            "Dateiliste muss genau aegis.exe und aegis-updater.exe enthalten",
        ));
    }
    if manifest.files.iter().any(|f| !is_sha256_hex(&f.sha256)) {
        return Err(bad_format(
            "sha256 muss aus 64 Kleinbuchstaben-Hexzeichen bestehen",
        ));
    }

    if manifest.issued_at.checked_add(VALIDITY_SECS) != Some(manifest.expires_at) {
        return Err(bad_format("Gültigkeit muss genau 7 Tage betragen"));
    }
    if manifest.issued_at > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
        return Err(UpdateError::NotYetValid);
    }
    Ok(manifest)
}

/// Ergebnis der Auswahl unter allen veröffentlichten Releases.
#[derive(Debug, PartialEq)]
pub enum Decision {
    UpToDate,
    Available {
        manifest: Manifest,
        security: bool,
    },
    /// Neueres Release, dessen Manifest abgelaufen ist.
    Stale {
        version: Version,
        expired_at: u64,
    },
    /// Neueres Release mit ungültigem Manifest: mögliche Manipulation.
    Rejected {
        version: Version,
        reason: String,
    },
}

/// `candidates`: (Tag-Version, Ergebnis von [`verify`] ohne Ablaufprüfung oder Fehler).
/// Wählt die höchste gültige Version, nicht die neueste.
pub fn decide(
    installed: Version,
    candidates: Vec<(Version, Result<Manifest, UpdateError>)>,
    now: u64,
) -> Decision {
    let mut best: Option<Manifest> = None;
    let mut rejected: Option<(Version, String)> = None;

    for (tag, result) in candidates {
        let invalid = match result {
            Ok(m) if m.version == tag => {
                if best.as_ref().is_none_or(|b| m.version > b.version) {
                    best = Some(m);
                }
                continue;
            }
            Ok(_) => UpdateError::WrongVersion,
            Err(e) => e,
        };
        if tag > installed && rejected.as_ref().is_none_or(|(v, _)| tag > *v) {
            rejected = Some((tag, invalid.to_string()));
        }
    }

    match best {
        Some(m) if m.version > installed => {
            if now > m.expires_at {
                Decision::Stale {
                    version: m.version,
                    expired_at: m.expires_at,
                }
            } else {
                let security = installed < m.min_secure;
                Decision::Available {
                    manifest: m,
                    security,
                }
            }
        }
        _ => match rejected {
            Some((version, reason)) => Decision::Rejected { version, reason },
            None => Decision::UpToDate,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Version;
    use std::io::Cursor;

    const NOW: u64 = 1_800_000_000;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    fn hex(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn manifest(version: &str) -> Manifest {
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
                    sha256: hex('a'),
                },
                FileEntry {
                    name: "aegis-updater.exe".into(),
                    sha256: hex('b'),
                },
            ],
        }
    }

    /// Signiert beliebige Bytes; liefert (Bytes, Signatur, Public Key als Base64).
    fn sign_raw(json: Vec<u8>) -> (Vec<u8>, String, String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig = minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(&json), None, None).unwrap();
        (json, sig.to_string(), kp.pk.to_base64())
    }

    fn signed(m: &Manifest) -> (Vec<u8>, String, String) {
        sign_raw(serde_json::to_vec(m).unwrap())
    }

    fn signed_json(value: serde_json::Value) -> (Vec<u8>, String, String) {
        sign_raw(serde_json::to_vec(&value).unwrap())
    }

    fn is_bad_format(r: Result<Manifest, UpdateError>) -> bool {
        matches!(r, Err(UpdateError::BadFormat(_)))
    }

    #[test]
    fn valid_manifest_verifies() {
        let m = manifest("0.2.0");
        let (json, sig, pk) = signed(&m);
        assert_eq!(verify(&json, &sig, &[&pk], NOW), Ok(m));
    }

    #[test]
    fn valid_with_second_trusted_key() {
        let m = manifest("0.2.0");
        let (json, sig, pk) = signed(&m);
        let (_, _, other) = signed(&m);
        assert_eq!(verify(&json, &sig, &[&other, &pk], NOW), Ok(m));
    }

    #[test]
    fn expired_manifest_still_verifies() {
        let m = manifest("0.2.0");
        let (json, sig, pk) = signed(&m);
        assert_eq!(verify(&json, &sig, &[&pk], m.expires_at + 1), Ok(m));
    }

    #[test]
    fn tampered_manifest_rejected() {
        let (mut json, sig, pk) = signed(&manifest("0.2.0"));
        json[10] ^= 1;
        assert_eq!(
            verify(&json, &sig, &[&pk], NOW),
            Err(UpdateError::BadSignature)
        );
    }

    #[test]
    fn wrong_key_rejected() {
        let (json, sig, _) = signed(&manifest("0.2.0"));
        let (_, _, other) = signed(&manifest("0.2.0"));
        assert_eq!(
            verify(&json, &sig, &[&other], NOW),
            Err(UpdateError::BadSignature)
        );
    }

    #[test]
    fn garbage_signature_or_key_rejected() {
        let (json, sig, pk) = signed(&manifest("0.2.0"));
        assert_eq!(
            verify(&json, "kein minisign", &[&pk], NOW),
            Err(UpdateError::BadSignature)
        );
        assert_eq!(
            verify(&json, &sig, &["kein key"], NOW),
            Err(UpdateError::BadSignature)
        );
    }

    #[test]
    fn empty_key_list_rejects_everything() {
        let (json, sig, _) = signed(&manifest("0.2.0"));
        assert_eq!(
            verify(&json, &sig, &[], NOW),
            Err(UpdateError::NoTrustedKey)
        );
        assert!(crate::TRUSTED_KEYS.is_empty());
    }

    #[test]
    fn unknown_schema_rejected() {
        let mut m = manifest("0.2.0");
        m.schema = 2;
        let (json, sig, pk) = signed(&m);
        assert!(is_bad_format(verify(&json, &sig, &[&pk], NOW)));
    }

    #[test]
    fn unknown_field_rejected() {
        let mut value = serde_json::to_value(manifest("0.2.0")).unwrap();
        value["x"] = 1.into();
        let (json, sig, pk) = signed_json(value);
        assert!(is_bad_format(verify(&json, &sig, &[&pk], NOW)));

        let mut value = serde_json::to_value(manifest("0.2.0")).unwrap();
        value["files"][0]["extra"] = true.into();
        let (json, sig, pk) = signed_json(value);
        assert!(is_bad_format(verify(&json, &sig, &[&pk], NOW)));
    }

    #[test]
    fn non_json_payload_rejected() {
        let (json, sig, pk) = sign_raw(b"das ist kein json".to_vec());
        assert!(is_bad_format(verify(&json, &sig, &[&pk], NOW)));
    }

    #[test]
    fn missing_or_extra_file_rejected() {
        let mut only_app = manifest("0.2.0");
        only_app.files.truncate(1);
        let mut only_updater = manifest("0.2.0");
        only_updater.files.remove(0);
        let mut extra = manifest("0.2.0");
        extra.files.push(FileEntry {
            name: "evil.dll".into(),
            sha256: hex('c'),
        });
        let mut duplicate = manifest("0.2.0");
        duplicate.files.push(duplicate.files[0].clone());
        let mut renamed = manifest("0.2.0");
        renamed.files[1].name = "evil.dll".into();
        let mut traversal = manifest("0.2.0");
        traversal.files[0].name = "..\\aegis.exe".into();
        let mut empty = manifest("0.2.0");
        empty.files.clear();
        for m in [
            only_app,
            only_updater,
            extra,
            duplicate,
            renamed,
            traversal,
            empty,
        ] {
            let (json, sig, pk) = signed(&m);
            assert!(
                is_bad_format(verify(&json, &sig, &[&pk], NOW)),
                "Dateiliste {:?} muss abgelehnt werden",
                m.files
            );
        }
    }

    #[test]
    fn file_order_does_not_matter() {
        let mut m = manifest("0.2.0");
        m.files.reverse();
        let (json, sig, pk) = signed(&m);
        assert_eq!(verify(&json, &sig, &[&pk], NOW), Ok(m));
    }

    #[test]
    fn bad_sha256_rejected() {
        for bad in [
            "",
            "abc",
            &"A".repeat(64),
            &"g".repeat(64),
            &"a".repeat(63),
            &"a".repeat(65),
        ] {
            let mut m = manifest("0.2.0");
            m.files[0].sha256 = bad.to_string();
            let (json, sig, pk) = signed(&m);
            assert!(
                is_bad_format(verify(&json, &sig, &[&pk], NOW)),
                "sha256 {bad:?} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn issued_in_future_rejected() {
        let mut m = manifest("0.2.0");
        m.issued_at = NOW + 2 * 86400;
        m.expires_at = m.issued_at + VALIDITY_SECS;
        let (json, sig, pk) = signed(&m);
        assert_eq!(
            verify(&json, &sig, &[&pk], NOW),
            Err(UpdateError::NotYetValid)
        );
    }

    #[test]
    fn issued_within_clock_skew_accepted() {
        let mut m = manifest("0.2.0");
        m.issued_at = NOW + 86400;
        m.expires_at = m.issued_at + VALIDITY_SECS;
        let (json, sig, pk) = signed(&m);
        assert_eq!(verify(&json, &sig, &[&pk], NOW), Ok(m));
    }

    #[test]
    fn validity_must_be_seven_days() {
        for delta in [
            VALIDITY_SECS + 86400,
            VALIDITY_SECS - 1,
            VALIDITY_SECS + 1,
            0,
        ] {
            let mut m = manifest("0.2.0");
            m.expires_at = m.issued_at + delta;
            let (json, sig, pk) = signed(&m);
            assert!(
                is_bad_format(verify(&json, &sig, &[&pk], NOW)),
                "Gültigkeit {delta} s muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn overflowing_validity_rejected_without_panic() {
        let mut m = manifest("0.2.0");
        m.issued_at = u64::MAX - 10;
        m.expires_at = 5;
        let (json, sig, pk) = signed(&m);
        assert!(verify(&json, &sig, &[&pk], NOW).is_err());
    }

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn ok(version: &str) -> (Version, Result<Manifest, UpdateError>) {
        (v(version), Ok(manifest(version)))
    }

    #[test]
    fn decide_picks_highest_valid_not_newest() {
        let cands = vec![
            (v("0.3.0"), Err(UpdateError::BadSignature)),
            ok("0.2.0"),
            ok("0.2.5"),
        ];
        match decide(v("0.1.0"), cands, NOW) {
            Decision::Available { manifest, security } => {
                assert_eq!(manifest.version, v("0.2.5"));
                assert!(!security);
            }
            other => panic!("unerwartet: {other:?}"),
        }
    }

    #[test]
    fn decide_rejects_tag_mismatch() {
        // Manifest 0.2.0 unter Tag 0.3.0 ist ungültig und wird als Manipulation gemeldet.
        let cands = vec![(v("0.3.0"), Ok(manifest("0.2.0")))];
        assert!(matches!(
            decide(v("0.1.0"), cands, NOW),
            Decision::Rejected { version, .. } if version == v("0.3.0")
        ));
        // Ein gültiger Kandidat daneben wird trotzdem angeboten, das falsche nie.
        let cands = vec![(v("0.3.0"), Ok(manifest("0.2.0"))), ok("0.2.0")];
        assert!(matches!(
            decide(v("0.1.0"), cands, NOW),
            Decision::Available { manifest, .. } if manifest.version == v("0.2.0")
        ));
    }

    #[test]
    fn decide_reports_rejected_newer() {
        let bad = (v("0.2.0"), Err(UpdateError::BadSignature));
        assert!(matches!(
            decide(v("0.1.0"), vec![bad], NOW),
            Decision::Rejected { version, .. } if version == v("0.2.0")
        ));

        let bad = (v("0.2.0"), Err(UpdateError::BadSignature));
        assert!(matches!(
            decide(v("0.1.0"), vec![bad, ok("0.1.5")], NOW),
            Decision::Available { manifest, .. } if manifest.version == v("0.1.5")
        ));
    }

    #[test]
    fn decide_rejected_names_highest_invalid_tag() {
        let cands = vec![
            (v("0.2.0"), Err(UpdateError::BadSignature)),
            (v("0.4.0"), Err(UpdateError::BadFormat("x".into()))),
            (v("0.3.0"), Err(UpdateError::BadSignature)),
        ];
        assert!(matches!(
            decide(v("0.1.0"), cands, NOW),
            Decision::Rejected { version, .. } if version == v("0.4.0")
        ));
    }

    #[test]
    fn decide_ignores_invalid_not_newer_than_installed() {
        let cands = vec![
            (v("0.1.0"), Err(UpdateError::BadSignature)),
            (v("0.0.9"), Err(UpdateError::BadSignature)),
        ];
        assert_eq!(decide(v("0.1.0"), cands, NOW), Decision::UpToDate);
    }

    #[test]
    fn decide_up_to_date() {
        assert_eq!(
            decide(v("0.2.0"), vec![ok("0.2.0"), ok("0.1.0")], NOW),
            Decision::UpToDate
        );
        assert_eq!(decide(v("0.2.0"), vec![], NOW), Decision::UpToDate);
    }

    #[test]
    fn decide_stale_when_expired() {
        let m = manifest("0.2.0");
        let expired_at = m.expires_at;
        assert_eq!(
            decide(
                v("0.1.0"),
                vec![(v("0.2.0"), Ok(m.clone()))],
                expired_at + 1
            ),
            Decision::Stale {
                version: v("0.2.0"),
                expired_at
            }
        );
        // Genau zum Ablaufzeitpunkt ist es noch gültig.
        assert!(matches!(
            decide(v("0.1.0"), vec![(v("0.2.0"), Ok(m))], expired_at),
            Decision::Available { .. }
        ));
    }

    #[test]
    fn decide_security_is_cumulative() {
        let cands = || {
            let mut newest = manifest("0.3.0");
            newest.min_secure = v("0.2.0");
            vec![(v("0.3.0"), Ok(newest)), ok("0.2.0")]
        };
        assert!(matches!(
            decide(v("0.1.0"), cands(), NOW),
            Decision::Available { security: true, .. }
        ));
        // Wer schon min_secure hat, bekommt kein Sicherheits-Update.
        assert!(matches!(
            decide(v("0.2.0"), cands(), NOW),
            Decision::Available {
                security: false,
                ..
            }
        ));
    }
}
