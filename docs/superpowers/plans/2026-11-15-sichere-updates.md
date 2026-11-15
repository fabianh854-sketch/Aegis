# Sichere Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Aegis meldet neue (Sicherheits-)Versionen und installiert sie per Klick – nur wenn ein lokal signiertes Manifest sie bestätigt.

**Architecture:** Neuer Bibliotheks-Crate `aegis-update` (Manifest, Signaturprüfung, Versionsentscheidung, Statusdatei – ohne Netzwerk). Neuer Binary-Crate `aegis-updater` (Netz nur über `curl.exe`, `--check`/`--install`). Neues Werkzeug `aegis-sign` (Schlüssel, Manifest erzeugen/signieren/erneuern; nicht ausgeliefert). `aegis-app` startet den Updater, liest `update.json`, zeigt den Hinweis. Release läuft lokal per PowerShell-Skripten.

**Tech Stack:** Rust stable, `minisign-verify = "0.3"` (prüfen), `minisign = "0.10"` (nur `aegis-sign` und Tests), `serde`/`serde_json`, `C:\Windows\System32\curl.exe`, PowerShell 7, `gh`.

**Spec:** `docs/superpowers/specs/2026-11-15-sichere-updates-design.md`

## Global Constraints

- `aegis.exe` (Crate `aegis-app`) hat keinen Netzwerkzugriff; keine Netzwerk-Crates im Workspace; `deny.toml` bleibt unverändert und `cargo deny check` muss bestehen.
- Netzwerk nur im Updater, nur über absoluten Pfad `C:\Windows\System32\curl.exe` mit `--proto =https --proto-redir =https --tlsv1.2 --fail --location --silent --show-error --max-time 60`.
- Repository fest: `fabianh854-sketch/Aegis`; API `https://api.github.com/repos/fabianh854-sketch/Aegis/releases`.
- Manifest-Datei `aegis-manifest.json` + `aegis-manifest.json.minisig`; Schema-Version `1`; Ablauf **7 Tage** nach `issued_at`.
- Erlaubte Dateinamen im Manifest genau: `aegis.exe`, `aegis-updater.exe`; beide müssen vorkommen.
- Vertrauenswürdige Schlüssel: `pub const TRUSTED_KEYS: &[&str]` (minisign-Public-Key-Strings) in `aegis-update`; bis der Nutzer einen Schlüssel erzeugt hat, leer → alles wird abgelehnt.
- Statusdatei `%APPDATA%\Aegis\update.json`, Einstellungen `%APPDATA%\Aegis\settings.json`, Staging `%LOCALAPPDATA%\Aegis\update\`.
- Prüfungen in dieser Reihenfolge, jede Ablehnung ist endgültig: Signatur → Schema → Ablauf → Version > installiert → Tag = Manifest-Version → SHA-256 jeder Datei.
- UI-Texte Deutsch. `cargo clippy --workspace --all-targets -- -D warnings` sauber.
- Commits nur nach Rücksprache mit dem Nutzer (er entscheidet, wann committet wird).

## Review Focus

- Manifest mit gültiger Signatur, aber unbekannten zusätzlichen Feldern oder falschem `schema` → abgelehnt (Test in Task 1: `unknown_schema_rejected`, serde `deny_unknown_fields`).
- `--install` während eine andere Instanz von Aegis noch läuft (andere PID) → Ersetzen scheitert sauber mit Rollback (Test in Task 4: `replace_rolls_back_when_target_locked` simuliert gesperrte Datei per offenem Handle).
- `update.json` beschädigt oder von älterem Schema → App zeigt nichts und stürzt nicht ab (Test in Task 5: `corrupt_status_shows_nothing`).
- Systemuhr falsch (weit in der Zukunft/Vergangenheit) → bei Zukunft gilt das Manifest als abgelaufen (stale), bei `issued_at` > jetzt + 1 Tag ebenfalls abgelehnt (Test in Task 1: `issued_in_future_rejected`).
- Updater-Ordner nicht beschreibbar → Fehlermeldung in `update.json`, alte Dateien bleiben (Test in Task 4: `readonly_target_keeps_old_files`).

---

### Task 1: Crate `aegis-update` – Version, Manifest, Prüfung

**Files:**
- Create: `crates/aegis-update/Cargo.toml`, `crates/aegis-update/src/lib.rs`, `src/version.rs`, `src/manifest.rs`, `src/keys.rs`
- Modify: `Cargo.toml` (Workspace-Mitglied)

**Interfaces – Produces:**
```rust
// version.rs
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Version { pub major: u32, pub minor: u32, pub patch: u32 }
impl std::str::FromStr for Version { type Err = UpdateError; }   // nur "MAJOR.MINOR.PATCH", optional führendes "v"
impl std::fmt::Display for Version {}                             // "0.1.1"
pub fn installed() -> Version;                                     // aus env!("CARGO_PKG_VERSION")

// keys.rs
pub const TRUSTED_KEYS: &[&str] = &[];

// manifest.rs
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)] #[serde(deny_unknown_fields)]
pub struct Manifest { pub schema: u32, pub version: Version, pub security: bool, pub min_secure: Version,
                      pub issued_at: u64 /* Unix-Sekunden */, pub expires_at: u64, pub files: Vec<FileEntry> }
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)] #[serde(deny_unknown_fields)]
pub struct FileEntry { pub name: String, pub sha256: String /* 64 Kleinbuchstaben-Hex */ }
pub const VALIDITY_SECS: u64 = 7 * 24 * 3600;
pub fn verify(json: &[u8], sig: &str, keys: &[&str], now: u64) -> Result<Manifest, UpdateError>;   // Signatur, Schema, Dateien, Zeiten
pub fn sha256_hex(data: &[u8]) -> String;
#[derive(Debug, PartialEq)] pub enum Decision { UpToDate, Available { manifest: Manifest, security: bool }, Stale { version: Version, expired_at: u64 },
                                         Rejected { version: Version, reason: String } }
/// `candidates`: (Tag-Version, Ergebnis von `verify` ohne Ablaufprüfung oder Fehler). Wählt die höchste gültige Version.
pub fn decide(installed: Version, candidates: Vec<(Version, Result<Manifest, UpdateError>)>, now: u64) -> Decision;
#[derive(Debug, thiserror::Error, PartialEq)] pub enum UpdateError { /* BadSignature, NoTrustedKey, BadFormat(String), Expired, NotYetValid, WrongVersion, Hash(String), Io(String), Network(String) */ }
```
- `verify` prüft: Signatur gegen jeden Schlüssel in `keys` (Leere Liste → `NoTrustedKey`); `schema == 1`; genau die zwei erlaubten Dateinamen, je einmal; `sha256` Format; `expires_at == issued_at + VALIDITY_SECS`; `issued_at <= now + 86400`. **Ablauf (`now > expires_at`) prüft `verify` nicht** – das entscheidet `decide`, damit „stale“ meldbar ist.
- `decide`: Kandidaten mit Fehler oder `manifest.version != Tag-Version` sind ungültig. Höchste **gültige** Version > installiert: abgelaufen → `Stale`, sonst `Available { security: installed < manifest.min_secure }`. Gibt es keine gültige neuere, aber einen **ungültigen Kandidaten mit Tag > installiert** → `Rejected { version: <höchster solcher Tag>, reason }` (mögliche Manipulation, wird rot angezeigt). Sonst `UpToDate`.
- Abhängigkeiten: `minisign-verify = "0.3"`, `serde`, `serde_json`, `thiserror`, `sha2 = "0.10"` (RustCrypto, MIT/Apache; bereits transitiv über argon2? sonst neu). Dev: `minisign = "0.10"` zum Erzeugen von Testschlüsseln.

- [ ] **Step 1: Failing tests** (Testhelfer `fn signed(m: &Manifest) -> (Vec<u8>, String, String /*pk*/)` erzeugt mit `minisign::KeyPair::generate_unencrypted_keypair`)
```rust
#[test] fn version_parse_and_order() { assert!("v0.1.10".parse::<Version>().unwrap() > "0.1.9".parse().unwrap()); assert!("0.1".parse::<Version>().is_err()); assert!("0.1.1-beta".parse::<Version>().is_err()); }
#[test] fn valid_manifest_verifies() { /* verify(...) == Ok(m) */ }
#[test] fn tampered_manifest_rejected() { /* ein Byte in json ändern → BadSignature */ }
#[test] fn wrong_key_rejected() { /* anderer pk → BadSignature */ }
#[test] fn empty_key_list_rejects_everything() { /* keys = &[] → NoTrustedKey */ }
#[test] fn unknown_schema_rejected() { /* schema 2 → BadFormat; Zusatzfeld "x":1 → BadFormat */ }
#[test] fn missing_or_extra_file_rejected() { /* nur aegis.exe; oder "evil.dll" zusätzlich → BadFormat */ }
#[test] fn issued_in_future_rejected() { /* issued_at = now + 2 Tage → NotYetValid */ }
#[test] fn validity_must_be_seven_days() { /* expires_at = issued_at + 8 Tage → BadFormat */ }
#[test] fn decide_picks_highest_valid_not_newest() { /* Kandidaten 0.3.0 (Fehler), 0.2.0 ok, 0.2.5 ok → Available 0.2.5 */ }
#[test] fn decide_rejects_tag_mismatch() { /* Tag 0.3.0, Manifest 0.2.0 → ignoriert */ }
#[test] fn decide_reports_rejected_newer() { /* installiert 0.1.0, Kandidat 0.2.0 mit BadSignature → Rejected{0.2.0}; zusätzlich gültiges 0.1.5 → Available 0.1.5 */ }
#[test] fn decide_up_to_date() { /* höchste = installiert → UpToDate */ }
#[test] fn decide_stale_when_expired() { /* now > expires_at → Stale */ }
#[test] fn decide_security_is_cumulative() { /* installiert 0.1.0, neueste 0.3.0 security=false, min_secure 0.2.0 → Available{security:true} */ }
```
- [ ] **Step 2:** `cargo test -p aegis-update` → FAIL (nicht kompilierbar).
- [ ] **Step 3:** Implementieren wie in Interfaces.
- [ ] **Step 4:** `cargo test -p aegis-update` → PASS.

### Task 2: Statusdatei und Einstellungen (in `aegis-update`)

**Files:** Create `crates/aegis-update/src/status.rs`

**Interfaces – Produces:**
```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)] #[serde(tag = "status", rename_all = "lowercase")]
pub enum UpdateStatus { None { checked_at: u64 }, Available { checked_at: u64, version: Version, security: bool },
                        Stale { checked_at: u64, version: Version, expired_at: u64 }, Rejected { checked_at: u64, version: Version, reason: String },
                        Error { checked_at: u64, message: String } }
impl UpdateStatus { pub fn checked_at(&self) -> u64; }
pub fn write_status(path: &Path, status: &UpdateStatus) -> Result<(), UpdateError>;   // atomar: Temp-Datei + rename
pub fn read_status(path: &Path) -> Option<UpdateStatus>;                               // jeder Fehler → None
pub fn status_path() -> Option<PathBuf>;   // %APPDATA%\Aegis\update.json
pub fn staging_dir() -> Option<PathBuf>;   // %LOCALAPPDATA%\Aegis\update
pub fn now() -> u64;                       // Unix-Sekunden
```
- [ ] **Step 1: Failing tests:** `status_roundtrip` (alle fünf Varianten), `corrupt_or_missing_status_is_none` (Datei mit `{` und fehlende Datei → `None`), `write_is_atomic_no_tmp_left`.
- [ ] **Step 2:** FAIL. **Step 3:** Implementieren. **Step 4:** `cargo test -p aegis-update` → PASS.

### Task 3: Werkzeug `aegis-sign`

**Files:** Create `crates/aegis-sign/Cargo.toml`, `crates/aegis-sign/src/main.rs`, `src/build.rs`

**Interfaces:**
- Consumes: `Manifest`, `FileEntry`, `Version`, `VALIDITY_SECS`, `sha256_hex`, `verify`, `TRUSTED_KEYS` (Task 1).
- Produces (testbar, `build.rs`):
```rust
pub fn build_manifest(version: Version, security: bool, previous: Option<&Manifest>, files: &[(&str, &[u8])], now: u64) -> Result<Manifest, String>;
// min_secure = if security { version } else { previous.map(|p| p.min_secure).unwrap_or(Version{0,0,0}) }; Fehler, wenn version <= previous.version
pub fn refresh_manifest(old: &Manifest, now: u64) -> Manifest;   // nur issued_at/expires_at neu
```
- CLI (`main.rs`, Passwort per `minisign`-Abfrage):
  - `aegis-sign keygen --dir <ordner>` → `aegis.key` (verschlüsselt) + `aegis.pub`; bricht ab, wenn `aegis.key` existiert; gibt Public-Key-String aus.
  - `aegis-sign manifest --version X --key <aegis.key> --app <aegis.exe> --updater <aegis-updater.exe> [--previous <manifest.json>] [--security] --out <ordner>` → schreibt Manifest + `.minisig`, prüft danach selbst mit `verify(.., TRUSTED_KEYS, now)` und bricht bei Fehler ab.
  - `aegis-sign refresh --key <aegis.key> --manifest <json> --sig <minisig> --out <ordner>` → prüft das alte (Signatur ok, Ablauf egal), erzeugt `refresh_manifest`, signiert, prüft.
  - Vertrauenswürdiger Kommentar der Signatur: `aegis-manifest v=<version>`.
- [ ] **Step 1: Failing tests:** `security_sets_min_secure_to_version`, `non_security_carries_previous_min_secure`, `version_must_increase`, `refresh_changes_only_times` (alle anderen Felder gleich, `expires_at = now + 7 Tage`), `signed_by_tool_verifies` (Schlüssel im Test erzeugt, über `minisign::sign` signiert, `verify` mit diesem pk ok).
- [ ] **Step 2:** FAIL. **Step 3:** Implementieren. **Step 4:** `cargo test -p aegis-sign` → PASS.

### Task 4: `aegis-updater` – Netz, Check, Installation

**Files:** Create `crates/aegis-updater/Cargo.toml`, `src/main.rs`, `src/fetch.rs`, `src/github.rs`, `src/install.rs`; `build.rs` + `.rc` mit demselben Icon (Datei aus `crates/aegis-app/assets/icon/aegis.ico` referenzieren).

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces:
```rust
// fetch.rs
pub trait Fetch { fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError>; }
pub struct Curl;   // C:\Windows\System32\curl.exe mit den Global-Constraints-Argumenten; Header "Accept: application/vnd.github+json" und "User-Agent: aegis-updater" nur für api.github.com
// github.rs
pub struct ReleaseRef { pub tag: Version, pub assets: std::collections::HashMap<String, String> /* name → browser_download_url */ }
pub fn parse_releases(json: &[u8]) -> Result<Vec<ReleaseRef>, UpdateError>;  // überspringt draft, prerelease, Tags die nicht als Version parsen; nur URLs mit Präfix https://github.com/fabianh854-sketch/Aegis/releases/download/
pub fn check(fetch: &dyn Fetch, installed: Version, keys: &[&str], now: u64) -> Decision;  // lädt je Release (absteigend, max. 5) Manifest + Signatur, ruft verify/decide
// install.rs
pub fn download_verified(fetch: &dyn Fetch, release: &ReleaseRef, manifest: &Manifest, staging: &Path) -> Result<(), UpdateError>; // SHA-256 je Datei
pub fn replace_files(staging: &Path, target: &Path, manifest: &Manifest) -> Result<(), UpdateError>;
// prüft SHA-256 im Staging erneut; benennt target\<name> → <name>.old, verschiebt neue Datei; bei Fehler alle .old zurück
pub fn cleanup_old(target: &Path);
```
- `main.rs`: `--check` → `check(...)` → `write_status`; Netzfehler → `UpdateStatus::Error`. `--install --wait-pid <pid>` → erneut `check`; nur bei `Available` herunterladen; dann auf Prozessende warten (`OpenProcess`+`WaitForSingleObject` über `windows-sys` oder Polling von `tasklist`? → **Polling: alle 250 ms `OpenProcess(SYNCHRONIZE)` per `windows-sys = { version = "0.59", features = ["Win32_System_Threading", "Win32_Foundation"] }`, max. 60 s**); `replace_files`; Aegis aus dem Zielordner starten; jeder Fehler → `Error`-Status + Aegis trotzdem starten. Ziel = Ordner von `std::env::current_exe()`.
- [ ] **Step 1: Failing tests** (Fake-`Fetch` mit `HashMap<url, bytes>`, Testschlüssel wie Task 1):
  `parse_releases_skips_drafts_prereleases_and_foreign_urls`, `check_available_end_to_end_with_fake_fetch`, `check_none_when_no_release_newer`, `check_rejected_when_newer_signature_bad` (→ `Decision::Rejected`), `download_rejects_hash_mismatch`, `replace_files_swaps_both`, `replace_rolls_back_when_target_locked` (offener Schreib-Handle auf `aegis-updater.exe` im Ziel → Fehler, `aegis.exe` wieder alt), `readonly_target_keeps_old_files`, `cleanup_removes_old_files`.
- [ ] **Step 2:** FAIL. **Step 3:** Implementieren. **Step 4:** `cargo test -p aegis-updater` → PASS.

### Task 5: `aegis-app` – Einstellungen, Start-Check, Hinweis, Installieren

**Files:** Modify `crates/aegis-app/Cargo.toml` (+ `aegis-update`), `src/app.rs`; Create `src/settings.rs`, `src/updates.rs`

**Interfaces:**
- Consumes: `UpdateStatus`, `read_status`, `status_path` (Task 2).
- Produces:
```rust
// settings.rs
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(default)]
pub struct Persisted { pub check_updates_on_start: bool }   // Default: true
pub fn load(path: &Path) -> Persisted;                       // Fehler → Default
pub fn save(path: &Path, p: &Persisted) -> std::io::Result<()>;   // atomar
// updates.rs
#[derive(Debug, PartialEq)] pub enum Banner { Hidden, Update { version: String }, Security { version: String }, Stale { since: u64 }, Rejected { version: String } }
pub fn banner_for(status: Option<&UpdateStatus>, dismissed: bool) -> Banner;   // Error/None → Hidden; Available+security → Security (dismissed ignoriert); Available → Update oder Hidden wenn dismissed; Rejected → Rejected (dismissed ignoriert)
pub fn updater_path() -> Option<PathBuf>;   // neben current_exe, nur wenn Datei existiert
pub fn spawn_check(updater: &Path) -> std::io::Result<()>;
pub fn spawn_install(updater: &Path) -> std::io::Result<()>;   // --install --wait-pid <std::process::id()>
```
- App: beim Start `settings.json` laden; wenn Einstellung an und `updater_path()` existiert → `spawn_check`, Startzeit merken; alle 2 s `read_status` bis `checked_at >= Startzeit` (max. 2 min). Hinweisleiste (36 px) unter der Titelleiste in Haupt- und Entsperren-Ansicht: Security rot (`theme::DANGER`) „Sicherheitsupdate v{X} verfügbar“ + „Jetzt aktualisieren“; Update neutral „Neue Version v{X} verfügbar“ + „Aktualisieren“ + „Später“; Stale gelb/gedämpft „Update-Status nicht bestätigt – seit {Datum}“; Rejected rot „Update v{X} konnte nicht verifiziert werden – nicht installiert“ (kein Knopf). „Aktualisieren“: `lock()` (inkl. Zwischenablage-Regel), `spawn_install`, `ViewportCommand::Close`. Fehler beim Starten des Updaters → Status rot. Einstellungen: Schalter „Beim Start nach Updates suchen“ (speichert sofort) und Knopf „Jetzt nach Updates suchen“.
- [ ] **Step 1: Failing tests:** `settings_default_checks_on_start`, `settings_roundtrip_and_corrupt_falls_back`, `banner_hidden_for_none_and_error`, `banner_security_ignores_dismiss`, `banner_update_hidden_when_dismissed`, `banner_stale`, `banner_rejected_ignores_dismiss`, `corrupt_status_shows_nothing` (`read_status` auf kaputte Datei → `banner_for(None, false) == Hidden`).
- [ ] **Step 2:** FAIL. **Step 3:** Implementieren. **Step 4:** `cargo test --workspace` → PASS; Clippy sauber.

### Task 6: Skripte, Workflow, README

**Files:** Create `scripts/new-signing-key.ps1`, `scripts/release.ps1`, `scripts/refresh-manifest.ps1`; Modify `.github/workflows/release.yml`, `README.md`

- `new-signing-key.ps1`: `cargo run -p aegis-sign -- keygen --dir $env:USERPROFILE\.aegis-signing`; danach Anleitung: Public Key in `crates/aegis-update/src/keys.rs` eintragen, Fingerabdruck ins README, `aegis.key` zusätzlich offline sichern.
- `release.ps1 -Version X [-Security]`: Abbruch bei: schmutzigem Baum, Tag `vX` ≠ HEAD, Cargo-Version ≠ X, leerem `TRUSTED_KEYS`. Zeigt `git log --format='%h %an %s' <letzter Tag>..HEAD` und `git diff --stat`, verlangt Eingabe `geprüft`. Dann `cargo deny check` (installiert `cargo-deny` nach Rückfrage, falls fehlt), `cargo test --workspace --locked`, `cargo build --release --locked -p aegis-app -p aegis-updater`, `git diff --exit-code Cargo.lock`, vorheriges Manifest per `gh release download <letzter Tag> -p aegis-manifest.json` (falls vorhanden), `aegis-sign manifest ...`, Zip (`aegis.exe`, `aegis-updater.exe`, README, LICENSE, licenses\) + `.sha256`, `gh release create vX --verify-tag --title "Aegis vX" --generate-notes` mit Zip, `.sha256`, beiden `.exe`, Manifest, `.minisig`.
- `refresh-manifest.ps1`: neuestes Release per `gh release view --json tagName`, Manifest + Signatur laden, `aegis-sign refresh`, `gh release upload <tag> --clobber` beider Dateien.
- `release.yml`: Trigger bleibt; `permissions: contents: read`; Schritte „Paket schnüren“ und „GitHub-Release anlegen“ entfallen; Tests + Release-Build bleiben.
- README: Abschnitte „Updates“ (Ablauf, Offline-Präzisierung, Abschalten), „Release erstellen“ (Skripte, wöchentlich `refresh-manifest.ps1`), „Download prüfen“ (Fingerabdruck – Platzhalter bis Schlüssel existiert, `minisign -Vm aegis-manifest.json -P <key>`, SHA-256 vergleichen).
- [ ] **Step 1:** Skripte mit `pwsh -NoProfile -Command "& { . ./scripts/release.ps1 -WhatIf }"`? → Stattdessen `Invoke-ScriptAnalyzer` falls vorhanden, sonst `pwsh -NoProfile -Command "[System.Management.Automation.Language.Parser]::ParseFile(...)"` ohne Fehler für alle drei Skripte.
- [ ] **Step 2:** `release.ps1` ohne Schlüssel ausführen → bricht mit „Kein vertrauenswürdiger Schlüssel“ ab (erwarteter Schutz).

### Task 7: Schlüssel erzeugen und Ende-zu-Ende-Test (mit dem Nutzer)

- [ ] **Step 1:** Nutzer führt `scripts/new-signing-key.ps1` aus (Passwort wählt nur er). Ich trage den ausgegebenen Public Key in `keys.rs` und den Fingerabdruck ins README ein.
- [ ] **Step 2:** `cargo test --workspace --locked`, Clippy, `cargo deny check`.
- [ ] **Step 3:** Ende-zu-Ende ohne öffentliches Release: lokaler Testlauf von `aegis-updater --check` gegen das echte Repo → `None` bzw. `UpToDate` (es gibt noch kein Manifest) und `update.json` korrekt.
- [ ] **Step 4:** Commit nach Rücksprache; erstes Release mit Updater per `release.ps1` macht der Nutzer.
