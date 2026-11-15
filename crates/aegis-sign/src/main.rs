//! Lokales Signierwerkzeug des Maintainers (wird nie ausgeliefert).
//!
//! `keygen`, `manifest` und `refresh`; siehe `usage()`.

mod build;

use aegis_update::{now, Manifest, Version, TRUSTED_KEYS};
use minisign::{KeyPair, PublicKey, SecretKey};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::ExitCode,
};

const MANIFEST_NAME: &str = "aegis-manifest.json";
const SIG_NAME: &str = "aegis-manifest.json.minisig";

fn usage() -> String {
    "Verwendung:\n  \
     aegis-sign keygen --dir <ordner>\n  \
     aegis-sign manifest --version X.Y.Z --key <aegis.key> --app <aegis.exe> \
     --updater <aegis-updater.exe> [--previous <manifest.json> --previous-sig <manifest.json.minisig>] [--security] --out <ordner>\n  \
     aegis-sign refresh --key <aegis.key> --manifest <json> --sig <minisig> --out <ordner>"
        .to_string()
}

/// Zerlegt `--name wert`-Paare und Schalter; unbekannte oder doppelte Angaben sind Fehler.
fn parse_flags(
    args: &[String],
    valued: &[&str],
    switches: &[&str],
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut values = HashMap::new();
    let mut on = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let name = arg
            .strip_prefix("--")
            .ok_or_else(|| format!("Unerwartetes Argument: {arg}"))?;
        if switches.contains(&name) {
            if on.iter().any(|s| s == name) {
                return Err(format!("--{name} ist doppelt angegeben."));
            }
            on.push(name.to_string());
        } else if valued.contains(&name) {
            let value = it
                .next()
                .ok_or_else(|| format!("--{name} braucht einen Wert."))?;
            if values.insert(name.to_string(), value.clone()).is_some() {
                return Err(format!("--{name} ist doppelt angegeben."));
            }
        } else {
            return Err(format!("Unbekannte Option: --{name}"));
        }
    }
    Ok((values, on))
}

fn required(values: &HashMap<String, String>, name: &str) -> Result<PathBuf, String> {
    values
        .get(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("--{name} fehlt."))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{} lesen: {e}", path.display()))
}

fn load_secret_key(path: &Path) -> Result<(SecretKey, PublicKey), String> {
    // Fragt das Passwort interaktiv ab.
    let sk = SecretKey::from_file(path, None)
        .map_err(|e| format!("Schlüssel {} laden: {e}", path.display()))?;
    let pk = PublicKey::from_secret_key(&sk).map_err(|e| format!("Public Key ableiten: {e}"))?;
    Ok((sk, pk))
}

/// Signiert, prüft im Speicher gegen `TRUSTED_KEYS` und schreibt erst danach beide Dateien.
fn sign_and_write(
    manifest: &Manifest,
    sk: &SecretKey,
    pk: &PublicKey,
    out: &Path,
    at: u64,
) -> Result<(), String> {
    // Genau diese Bytes werden signiert und geschrieben.
    let json = serde_json::to_vec(manifest).map_err(|e| format!("Manifest serialisieren: {e}"))?;
    let sig = build::sign_bytes(sk, pk, &json, manifest.version)?;
    build::self_check(&json, &sig, TRUSTED_KEYS, at)?;
    fs::create_dir_all(out).map_err(|e| format!("{} anlegen: {e}", out.display()))?;
    fs::write(out.join(MANIFEST_NAME), &json).map_err(|e| format!("Manifest schreiben: {e}"))?;
    fs::write(out.join(SIG_NAME), sig).map_err(|e| format!("Signatur schreiben: {e}"))?;
    println!(
        "Geschrieben: {} und {}",
        out.join(MANIFEST_NAME).display(),
        out.join(SIG_NAME).display()
    );
    Ok(())
}

/// Bricht vor der Passwortabfrage ab, wenn nichts verifiziert werden könnte.
fn require_trusted_keys() -> Result<(), String> {
    if TRUSTED_KEYS.is_empty() {
        return Err(build::NO_KEY_MSG.to_string());
    }
    Ok(())
}

const MIN_PASSWORD_CHARS: usize = 12;

/// Regel für das Schlüsselpasswort: nicht leer, mindestens 12 Zeichen (nicht Bytes), Wiederholung gleich.
fn check_new_password(pw: &str, confirm: &str) -> Result<(), String> {
    if pw.is_empty() {
        return Err("Das Passwort darf nicht leer sein.".to_string());
    }
    if pw.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!(
            "Das Passwort muss mindestens {MIN_PASSWORD_CHARS} Zeichen lang sein."
        ));
    }
    if pw != confirm {
        return Err("Die Passwörter stimmen nicht überein.".to_string());
    }
    Ok(())
}

fn keygen(args: &[String]) -> Result<(), String> {
    let (values, _) = parse_flags(args, &["dir"], &[])?;
    let dir = required(&values, "dir")?;
    let key_path = dir.join("aegis.key");
    let pub_path = dir.join("aegis.pub");
    if key_path.exists() || pub_path.exists() {
        return Err(format!(
            "{} oder {} existiert bereits; es wird nichts überschrieben.",
            key_path.display(),
            pub_path.display()
        ));
    }
    // Passwort vor dem Anlegen der Dateien abfragen und prüfen.
    let password = rpassword::prompt_password("Passwort für den Signierschlüssel: ")
        .map_err(|e| format!("Passwort lesen: {e}"))?;
    let confirm = rpassword::prompt_password("Passwort wiederholen: ")
        .map_err(|e| format!("Passwort lesen: {e}"))?;
    check_new_password(&password, &confirm)?;
    fs::create_dir_all(&dir).map_err(|e| format!("{} anlegen: {e}", dir.display()))?;
    // create_new: schlägt fehl, falls die Datei inzwischen existiert.
    let open = |p: &Path| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(p)
            .map_err(|e| format!("{} anlegen: {e}", p.display()))
    };
    let pub_file = open(&pub_path)?;
    let key_file = match open(&key_path) {
        Ok(f) => f,
        Err(e) => {
            let _ = fs::remove_file(&pub_path);
            return Err(e);
        }
    };
    // Das geprüfte Passwort wird ausdrücklich übergeben (keine minisign-Abfrage).
    let KeyPair { pk, .. } =
        KeyPair::generate_and_write_encrypted_keypair(pub_file, key_file, None, Some(password))
            .map_err(|e| {
                // Unvollständige Dateien nicht stehen lassen.
                let _ = fs::remove_file(&key_path);
                let _ = fs::remove_file(&pub_path);
                format!("Schlüsselerzeugung fehlgeschlagen: {e}")
            })?;
    eprintln!(
        "Schlüssel gespeichert: {} (verschlüsselt, sicher aufbewahren) und {}.",
        key_path.display(),
        pub_path.display()
    );
    eprintln!("Diese Zeile in crates/aegis-update/src/keys.rs (TRUSTED_KEYS) eintragen:");
    println!("{}", pk.to_base64());
    Ok(())
}

fn manifest(args: &[String]) -> Result<(), String> {
    require_trusted_keys()?;
    let (values, on) = parse_flags(
        args,
        &[
            "version",
            "key",
            "app",
            "updater",
            "previous",
            "previous-sig",
            "out",
        ],
        &["security"],
    )?;
    let version: Version = values
        .get("version")
        .ok_or("--version fehlt.")?
        .parse()
        .map_err(|e| format!("--version: {e}"))?;
    let security = on.iter().any(|s| s == "security");
    let key = required(&values, "key")?;
    let app = read(&required(&values, "app")?)?;
    let updater = read(&required(&values, "updater")?)?;
    let out = required(&values, "out")?;
    let previous: Option<Manifest> = match build::previous_paths(
        values.get("previous").map(String::as_str),
        values.get("previous-sig").map(String::as_str),
    )? {
        Some((json_path, sig_path)) => {
            let json = read(Path::new(json_path))?;
            let sig = String::from_utf8(read(Path::new(sig_path))?)
                .map_err(|_| "Die Signaturdatei des vorherigen Manifests ist kein Text.")?;
            // Nur ein verifiziertes Manifest darf min_secure liefern.
            Some(build::verify_previous(&json, &sig, TRUSTED_KEYS, now())?)
        }
        None => None,
    };
    let at = now();
    let manifest = build::build_manifest(
        version,
        security,
        previous.as_ref(),
        &[("aegis.exe", &app), ("aegis-updater.exe", &updater)],
        at,
    )?;
    let (sk, pk) = load_secret_key(&key)?;
    sign_and_write(&manifest, &sk, &pk, &out, at)
}

fn refresh(args: &[String]) -> Result<(), String> {
    require_trusted_keys()?;
    let (values, _) = parse_flags(args, &["key", "manifest", "sig", "out"], &[])?;
    let key = required(&values, "key")?;
    let old_json = read(&required(&values, "manifest")?)?;
    let old_sig = String::from_utf8(read(&required(&values, "sig")?)?)
        .map_err(|_| "Die Signaturdatei ist kein Text.".to_string())?;
    let out = required(&values, "out")?;
    let at = now();
    // Signatur des alten Manifests muss stimmen; der Ablauf ist hier egal.
    let old = build::self_check(&old_json, &old_sig, TRUSTED_KEYS, at)?;
    let manifest = build::refresh_manifest(&old, at);
    let (sk, pk) = load_secret_key(&key)?;
    sign_and_write(&manifest, &sk, &pk, &out, at)
}

fn run(args: &[String]) -> Result<(), String> {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "keygen" => keygen(rest),
        Some((cmd, rest)) if cmd == "manifest" => manifest(rest),
        Some((cmd, rest)) if cmd == "refresh" => refresh(rest),
        _ => Err(usage()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("Fehler: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_values_and_switches() {
        let (v, on) = parse_flags(
            &s(&["--version", "1.0.0", "--security", "--out", "o"]),
            &["version", "out"],
            &["security"],
        )
        .unwrap();
        assert_eq!(v["version"], "1.0.0");
        assert_eq!(v["out"], "o");
        assert_eq!(on, vec!["security"]);
    }

    #[test]
    fn rejects_unknown_duplicate_and_missing_values() {
        assert!(parse_flags(&s(&["--x", "1"]), &["a"], &[]).is_err());
        assert!(parse_flags(&s(&["--a", "1", "--a", "2"]), &["a"], &[]).is_err());
        assert!(parse_flags(&s(&["--a"]), &["a"], &[]).is_err());
        assert!(parse_flags(&s(&["stray"]), &["a"], &[]).is_err());
    }

    #[test]
    fn keygen_never_overwrites_existing_key() {
        let dir = std::env::temp_dir().join(format!("aegis-sign-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("aegis.key"), b"vorhanden").unwrap();
        let err = keygen(&s(&["--dir", dir.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("nichts überschrieben"));
        assert_eq!(fs::read(dir.join("aegis.key")).unwrap(), b"vorhanden");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn password_rules() {
        assert!(check_new_password("", "").unwrap_err().contains("leer"));
        assert!(check_new_password("abcdefghijk", "abcdefghijk")
            .unwrap_err()
            .contains("12 Zeichen"));
        assert!(check_new_password("abcdefghijkl", "abcdefghijkl").is_ok());
        // 12 Umlaute = 24 Bytes, aber nur 12 Zeichen; 11 Umlaute = 22 Bytes, zu kurz.
        assert!(check_new_password("äöüäöüäöüäöü", "äöüäöüäöüäöü").is_ok());
        assert!(check_new_password("äöüäöüäöüäö", "äöüäöüäöüäö").is_err());
        assert!(check_new_password("abcdefghijkl", "abcdefghijkm")
            .unwrap_err()
            .contains("stimmen nicht überein"));
    }
}
