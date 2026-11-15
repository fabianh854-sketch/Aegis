# Aegis

Passwortmanager in Rust, nur für Windows, mit nativer GUI (egui) und einem Benutzer.
Aegis selbst arbeitet vollständig offline; nur der getrennte Updater (`aegis-updater.exe`, siehe „Updates“) fragt auf Wunsch bei GitHub nach neuen Versionen.

## Aufbau

- `crates/aegis-core`: Krypto, Dateiformat, atomares Speichern. Ohne GUI, mit Tests.
- `crates/aegis-app`: Oberfläche, nutzt `aegis-core`. Kein Netzwerkzugriff (Binary `aegis.exe` im Release).
- `crates/aegis-update`: Prüfung signierter Update-Manifeste (Signatur, Ablauf, Versionen, Prüfsummen).
- `crates/aegis-updater`: Der einzige Teil mit Netzwerkzugriff (`aegis-updater.exe`), nur über `curl.exe` von Windows.
- `crates/aegis-sign`: Signierwerkzeug des Maintainers. Wird nicht ausgeliefert.
- `scripts/`: Skripte für Schlüssel, Release und Manifest-Erneuerung (PowerShell 7).
- `deny.toml`: Prüft Lizenzen, Sicherheitslücken und verbotene Netzwerk-Crates.

## Starten

```powershell
cargo test --workspace
cargo run -p aegis-app
```

## Prüfen

```powershell
cargo deny check
cargo build --release --offline
```

## Updates

Beim Start fragt Aegis (wenn eingeschaltet) im Hintergrund bei GitHub nach einer neueren Version:

- **Aegis selbst hat kein Netzwerk.** `aegis.exe` enthält keinen Netzwerkcode. Nur das getrennte Programm `aegis-updater.exe` im selben Ordner redet mit GitHub (ausschließlich HTTPS über das in Windows enthaltene `curl.exe`) und lädt nur Release-Dateien von `fabianh854-sketch/Aegis`.
- Jedes Release enthält ein **signiertes Manifest** (`aegis-manifest.json` und `aegis-manifest.json.minisig`). Der Updater übernimmt Version, Sicherheitsstatus und Prüfsummen ausschließlich aus einem Manifest, dessen Signatur zum fest eingebauten öffentlichen Schlüssel passt. Kein Download wird ausgeführt oder installiert, bevor Signatur und Prüfsummen stimmen.
- Ältere Versionen werden nie installiert (kein Downgrade). Manifeste sind nur 7 Tage gültig; ein abgelaufenes wird nie installiert und Aegis zeigt dann „Update-Status nicht bestätigt“.
- Ein **Sicherheitsupdate** wird rot angezeigt. Installiert wird erst nach deinem Klick auf „Aktualisieren“: Der Tresor wird gesperrt, `aegis.exe` und `aegis-updater.exe` werden zusammen ersetzt, Aegis startet neu. Bei einem Fehler bleiben die alten Dateien.
- **Abschalten:** Einstellungen → „Beim Start nach Updates suchen“. Dann wird bei keinem Start nachgefragt. Fehlt `aegis-updater.exe`, tut Aegis nichts in dieser Richtung.

Wer von v0.1.0 kommt, installiert das erste Release mit Updater einmal von Hand (Zip entpacken). Davor bitte „Download prüfen“ lesen.

## Release erstellen

Releases werden **lokal gebaut und signiert**, nicht in GitHub Actions. `.github/workflows/release.yml` baut und testet nur noch und hat nur Leserechte; es veröffentlicht nichts. Voraussetzungen: PowerShell 7 (`pwsh`), Rust, `git` und `gh` (angemeldet). Für dein GitHub-Konto bitte Zwei-Faktor-Anmeldung aktivieren.

1. **Einmalig:** `pwsh scripts\new-signing-key.ps1` erzeugt das Schlüsselpaar in `%USERPROFILE%\.aegis-signing\` (Passwort mit mindestens 12 Zeichen). Den ausgegebenen öffentlichen Schlüssel in `crates/aegis-update/src/keys.rs` (`TRUSTED_KEYS`) und im README unter „Download prüfen“ eintragen, beides committen. `aegis.key` zusätzlich offline sichern und nie committen. Solange `TRUSTED_KEYS` leer ist, lehnt alles ab.
2. **Pro Release:** Version in `Cargo.toml` erhöhen, committen, Tag `vX.Y.Z` auf diesen Commit setzen und pushen, dann
   `pwsh scripts\release.ps1 -Version X.Y.Z` (bei einem Sicherheitsupdate zusätzlich `-Security`).
   Das Skript bricht ab bei schmutzigem Arbeitsbaum, falschem Tag, falscher Cargo-Version oder leerem `TRUSTED_KEYS`. Es zeigt alle Commits seit dem letzten Release und verlangt die Eingabe `geprüft`, führt `cargo deny check`, die Tests und den Release-Build aus, baut Manifest und Zip, fragt das Schlüsselpasswort (das Skript speichert es nie) und veröffentlicht das Release mit Zip, `.sha256`, `aegis.exe`, `aegis-updater.exe`, Manifest und Signatur.
3. **Wöchentlich:** `pwsh scripts\refresh-manifest.ps1`. Manifeste laufen nach 7 Tagen ab; wird das vergessen, meldet der Updater „Update-Status nicht bestätigt“. Das Skript signiert das Manifest des neuesten Releases mit neuem Gültigkeitszeitraum neu (Version, Prüfsummen und `min_secure` bleiben gleich).

## Download prüfen

Öffentlicher Schlüssel (minisign): `<wird nach dem Erzeugen des Schlüssels eingetragen>`

Diesen Schlüssel nur aus dem README im Repository nehmen, nicht von der Release-Seite, damit eine Manipulation dort nicht reicht. So prüfst du einen Download von Hand:

1. Manifest und Signatur herunterladen (`aegis-manifest.json`, `aegis-manifest.json.minisig`) und mit [minisign](https://jedisct1.github.io/minisign/) prüfen:

   ```powershell
   minisign -Vm aegis-manifest.json -P <öffentlicher Schlüssel>
   ```

   Die Ausgabe muss „Signature and comment signature verified“ enthalten.
2. Die SHA-256-Prüfsummen der Programme mit den Einträgen im Manifest vergleichen:

   ```powershell
   Get-FileHash .\aegis.exe, .\aegis-updater.exe -Algorithm SHA256
   Get-Content aegis-manifest.json
   ```

   Die Werte unter `files` müssen mit den Hashes übereinstimmen (Kleinschreibung beachten: das Manifest nutzt Kleinbuchstaben, `Get-FileHash` Großbuchstaben).
3. Für die Zip gilt zusätzlich die Datei `aegis-vX.Y.Z-windows-x64.zip.sha256`; sie schützt nur vor Übertragungsfehlern, nicht vor Manipulation. Maßgeblich ist das signierte Manifest.

## Datenspeicherung

Der Vault liegt unter `%APPDATA%\Aegis\vault.aegis`.
Vor jedem Speichern wird die bisherige Datei als `vault.aegis.bak` gesichert.
Nach einem Wechsel des Master-Passworts wird auch das Backup ersetzt, damit keine Kopie mit dem alten Passwort bleibt.
Die Update-Einstellung steht in `%APPDATA%\Aegis\settings.json`, das Ergebnis der letzten Update-Prüfung in `%APPDATA%\Aegis\update.json`.
