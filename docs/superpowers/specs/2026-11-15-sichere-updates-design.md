# Sichere Updates – Design

Stand: 2026-11-15 · Basis: v0.1.0

## Absicht
Nutzer sollen keine neue Version verpassen, besonders keine Sicherheitsupdates. Updates werden mit einem Klick
installiert. **Sicherheit hat absolute Priorität** (Vorgabe des Nutzers): Kein Update darf installiert werden,
das nicht vom Besitzer des Signaturschlüssels stammt – auch nicht, wenn GitHub-Konto, GitHub Actions oder die
Verbindung kompromittiert sind. Zusätzliche Skripte/Schritte pro Release sind ausdrücklich in Ordnung.

## Ziele
1. Beim Start (abschaltbar, Standard an) erfährt der Nutzer von einer neueren Version; Sicherheitsupdates
   werden deutlich (rot, nicht dauerhaft wegklickbar) angezeigt.
2. „Aktualisieren“ lädt, prüft die Signatur, ersetzt die Programmdateien und startet Aegis neu.
3. `aegis.exe` bleibt netzwerkfrei; `deny.toml` bleibt unverändert (keine Netzwerk-Crates im Workspace).
4. Release-Dateien werden lokal gebaut und lokal signiert; der private Schlüssel verlässt den Rechner nie.

## Nicht-Ziele
- Installation ohne Bestätigung, Hintergrunddienst, geplante Aufgaben.
- Authenticode-Signatur (Windows SmartScreen) – kostenpflichtiges Zertifikat, separates Thema.
- Vorabversionen (Tags mit `-`) werden vom Updater ignoriert.
- Delta-Updates, Rollback-Funktion.

## Anti-Ziele (gescheitert, auch wenn es „funktioniert“)
- Eine Datei wird ausgeführt oder ersetzt, bevor ihre Signatur geprüft ist.
- Eine Angabe aus ungeprüfter Quelle (GitHub-API, Release Notes, Dateiname) entscheidet über Version,
  Sicherheitsstatus oder welche Datei wohin kommt.
- Ein Angreifer kann eine ältere, korrekt signierte Version unterschieben (Downgrade).
- Ein Fehler beim Update lässt Aegis unbenutzbar zurück.

## Bedrohungsmodell
| Angreifer kontrolliert | Schutz |
|---|---|
| Netzwerk / gefälschtes TLS | HTTPS über `curl.exe` (Schannel), zusätzlich Signatur – Netzwerk ist nicht vertrauenswürdig |
| GitHub-Konto / Releases | Signatur mit Schlüssel, der nur lokal liegt; öffentlicher Schlüssel fest in `aegis-updater` |
| GitHub Actions / Workflow | Release-Dateien werden lokal gebaut; CI veröffentlicht nichts mehr |
| Altes signiertes Release | Signierte Version muss größer als die installierte sein |
| Vertauschte Dateien (Updater ↔ App) | Dateiname steht im signierten Kommentar |
| Lokale Schadsoftware mit Nutzerrechten | Außerhalb des Modells (könnte Aegis direkt verändern) |

## Ansätze (besprochen)
| Ansatz | Abwägung |
|---|---|
| **Getrennter Updater, lokal signiert (gewählt)** | Aegis bleibt offline, fälschungssicher, mehr Release-Schritte |
| Hinweis-Link auf Releases-Seite | Kein Risiko, aber nichts automatisch |
| Updater in Aegis (`self_update`) | Bricht Offline-Versprechen und `deny.toml` |
| Signieren in GitHub Actions | Bequem, aber Kontoübernahme = gültig signierter Schadcode |

## Design

### Signatur und signiertes Manifest
- Verfahren **minisign** (Ed25519). Prüfen im Updater mit Crate `minisign-verify` (MIT, ohne Abhängigkeiten).
- Jedes Release enthält **`aegis-manifest.json` + `aegis-manifest.json.minisig`**. Nur das Manifest wird signiert;
  es bindet alle Dateien per SHA-256. Inhalt:
  `{ "schema": 1, "version": "0.1.1", "security": false, "min_secure": "0.1.1", "issued_at": "...", "expires_at": "...",`
  `  "files": [ { "name": "aegis.exe", "sha256": "..." }, { "name": "aegis-updater.exe", "sha256": "..." } ] }`
  - `min_secure`: höchste Version, die eine Sicherheitslücke schließt (kumulativ). Ist die installierte Version kleiner,
    ist das Update rot – auch wenn das neueste Release selbst kein Sicherheitsupdate ist.
  - `expires_at` = `issued_at` + **7 Tage** (Entscheidung des Nutzers). Schutz gegen ein verstecktes neues Release
    (Freeze-Angriff): Ist das Manifest der höchsten gültigen Version abgelaufen, meldet `--check` `stale` und Aegis
    zeigt „Update-Status nicht bestätigt – seit <Datum>“. Ein abgelaufenes Manifest wird **nie** installiert.
  - `scripts/refresh-manifest.ps1` (wöchentlich auszuführen): lädt das Manifest des neuesten Releases, prüft es,
    setzt `issued_at`/`expires_at` neu, signiert (Passwort-Abfrage) und ersetzt Manifest + Signatur im Release.
    Version, Prüfsummen und `min_secure` bleiben unverändert (das Skript bricht ab, wenn sie sich ändern würden).
- `--check` lädt nur Manifest + Signatur (wenige KB). Beim Installieren werden die `.exe`-Dateien gegen die
  SHA-256-Werte aus dem geprüften Manifest verglichen.
- Nur das geprüfte Manifest bestimmt Version, Sicherheitsstatus, Dateinamen und Prüfsummen. Die GitHub-API dient
  nur zum Finden der Download-Adressen.
- Beide Dateien haben immer dieselbe Version und werden **ganz oder gar nicht** installiert.
- Öffentlicher Schlüssel als Konstante in `aegis-updater`; Fingerabdruck zusätzlich im README (siehe Erstvertrauen).
  Privater Schlüssel: passwortgeschützte minisign-Datei außerhalb des Repos (`%USERPROFILE%\.aegis-signing\`).
  Sicherung/Ersatzschlüssel: **offen, der Nutzer erarbeitet ein eigenes Konzept.** Bis dahin genau ein Schlüssel.
  Damit ein späteres Konzept ohne Umbau passt, ist die Konstante eine Liste vertrauenswürdiger Schlüssel (anfangs
  mit einem Eintrag); jeder Eintrag in der Liste darf signieren. Ein Wechsel geht nur über ein Release, das mit
  einem bereits vertrauenswürdigen Schlüssel signiert ist.

### Versionswahl und Downgrade-Schutz
- Der Updater holt die Liste der Releases (`/releases`), ignoriert Entwürfe und Vorabversionen, prüft für die
  Kandidaten das Manifest und nimmt die **höchste gültig signierte Version** (nicht das neueste nach Datum).
- Angenommen wird nur: Version > installiert, Manifest-Version = Tag-Version, Signatur gültig, Manifest nicht abgelaufen.

### Erstvertrauen und Installationsort
- Ab dem ersten Release mit Updater enthält die Zip `aegis.exe` und `aegis-updater.exe`. Nutzer von v0.1.0 müssen
  dieses Release einmal manuell installieren. Das README nennt den Fingerabdruck des öffentlichen Schlüssels und wie
  man das Manifest mit minisign von Hand prüft.
- Ziel des Updates ist der Ordner von `aegis-updater.exe`, feste Namen `aegis.exe`/`aegis-updater.exe`; nur diese
  beiden Dateien werden ersetzt. Ist der Ordner nicht beschreibbar: Fehlermeldung, alte Dateien bleiben.

### Schutz der Release-Maschine
- `release.ps1` zeigt alle Commits seit dem letzten Release-Tag (Autor, Titel, `--stat`) und verlangt eine
  ausdrückliche Bestätigung „geprüft“ vor dem Signieren.
- Pflicht-Prüfungen im Skript: `cargo deny check` (wird bei Bedarf einmalig installiert), `cargo test --locked`,
  `Cargo.lock` unverändert nach dem Build.
- `release.yml` verliert `contents: write` (nur noch Lesen, Bauen, Testen).
- Empfehlung an den Nutzer: Zwei-Faktor-Anmeldung bei GitHub aktivieren.

### Neuer Crate `crates/aegis-updater` (Binary `aegis-updater.exe`)
- Netzwerk ausschließlich über `C:\Windows\System32\curl.exe` (absoluter Pfad, `--proto =https --proto-redir =https
  --tlsv1.2 --fail -L`), keine Netzwerk-Crates. JSON mit `serde_json`.
- Quelle fest: `https://api.github.com/repos/fabianh854-sketch/Aegis/releases` (siehe Versionswahl).
- Befehle:
  - `--check`: Kandidaten holen, Manifeste prüfen (siehe Versionswahl); Ergebnis atomar nach
    `%APPDATA%\Aegis\update.json` schreiben: `{ "status": "available"|"none"|"error"|"stale", "version", "security", "checked_at", "message" }`
    (`security` = installiert < `min_secure` des geprüften Manifests). Nur geprüfte Angaben landen dort.
  - `--install --wait-pid <pid>`: Manifest erneut laden und prüfen, Dateien nach `%LOCALAPPDATA%\Aegis\update\`
    laden, SHA-256 gegen das Manifest prüfen (vor dem Verschieben erneut), auf Ende von `<pid>` warten, laufende Dateien per Umbenennen
    (`*.old`) beiseite legen, neue an ihren Platz verschieben, Aegis starten. Bei jedem Fehler: alte Dateien
    zurück, Fehler in `update.json`, Aegis trotzdem starten.
- Installierte Version: aus eigener `CARGO_PKG_VERSION` (Updater und App haben dieselbe Workspace-Version).
- Aufräumen: `*.old` beim nächsten Lauf löschen.

### Änderungen in `aegis-app`
- Beim Start (Einstellung „Beim Start nach Updates suchen“, Standard an) `aegis-updater.exe --check` aus dem
  Ordner der eigenen `.exe` starten (`std::process::Command`, kein Netzwerk in Aegis), danach `update.json` lesen
  (alle paar Sekunden, bis ein neues `checked_at` da ist).
- Hinweis unter der Titelleiste: rot „Sicherheitsupdate vX verfügbar“ (nur „Jetzt aktualisieren“), neutral
  „Neue Version vX verfügbar“ („Aktualisieren“, „Später“). Fehlt `aegis-updater.exe`, wird nichts angezeigt.
- „Aktualisieren“: Tresor sperren (Zwischenablage-Regel wie beim Sperren), Updater mit `--install --wait-pid`
  starten, Fenster schließen.
- Einstellungen: Schalter für die Prüfung beim Start und Knopf „Jetzt nach Updates suchen“. Der Schalter wird in
  `%APPDATA%\Aegis\settings.json` gespeichert (bisher gibt es keine gespeicherten Einstellungen).
- README: Das Offline-Versprechen wird präzisiert – Aegis selbst hat kein Netzwerk; nur der getrennte Updater
  fragt bei GitHub nach (abschaltbar).

### Release-Ablauf (neu, lokal)
- `scripts/new-signing-key.ps1` (einmalig): erzeugt Schlüsselpaar, gibt den öffentlichen Schlüssel aus, der in
  `aegis-updater` eingetragen wird.
- `scripts/release.ps1 -Version 0.1.1 [-Security]`: prüft sauberen Arbeitsbaum, Tag `v<Version>` = HEAD und
  Cargo-Version; Commit-Prüfung mit Bestätigung (siehe Schutz der Release-Maschine); `cargo deny check`;
  `cargo test --locked`; `cargo build --release --locked`; erzeugt das Manifest (`min_secure` aus dem vorherigen
  Manifest übernommen bzw. bei `-Security` auf die neue Version gesetzt); signiert es (Passwort-Abfrage); prüft die
  Signatur gegen den eingebauten öffentlichen Schlüssel; baut Zip + `.sha256`; `gh release create` mit Zip, `.sha256`,
  beiden `.exe`, Manifest und `.minisig`.
- Signieren mit einem kleinen Werkzeug-Crate `crates/aegis-sign` (nicht ausgeliefert), Crate `minisign` (MIT),
  damit keine externe Installation nötig ist.
- `.github/workflows/release.yml`: baut und testet weiter bei Tags, **veröffentlicht aber keine Dateien mehr**
  (sonst könnte ein manipulierter Workflow unsignierte Dateien neben die echten legen – harmlos für den Updater,
  aber irreführend für manuelle Downloads).

## Tests
- Updater: gültige Signatur → angenommen; manipulierte Datei, falscher Schlüssel, falscher Dateiname im Kommentar,
  kleinere/gleiche Version, kaputtes Kommentarformat → abgelehnt. Versionsvergleich. `update.json` schreiben/lesen.
  Ersetzen der Dateien inkl. Zurückrollen bei Fehler (mit Temp-Ordnern, ohne Netz).
- App: Lesen von `update.json` → richtiger Hinweis (keiner / neutral / rot / Fehler still).
- Netzwerkteil (`curl`) wird nicht in Unit-Tests aufgerufen; manueller Ende-zu-Ende-Test mit einem Test-Release.

## Entscheidungen des Nutzers
- Lokales Signieren, Sicherheit vor Komfort, zusätzliche Skripte erlaubt.
- Ablaufdatum des Manifests: 7 Tage.
- Schlüsselverlust/-diebstahl: Konzept folgt später.

## Entscheidungen des Planers (zum Gegenlesen)
- `curl.exe` aus System32 statt HTTP-Crate, damit `deny.toml` unverändert bleibt.
- Lokales Bauen statt Signieren des CI-Builds (Workflow-Kompromittierung).
- Raw-`.exe` als Release-Dateien zusätzlich zur Zip (kein Entpacken im Updater nötig).
- Signiertes Manifest statt Signatur pro Datei (billiges `--check`, alles an einer Stelle gebunden).
- Höchste gültige Version statt `/releases/latest`; `min_secure` kumulativ.
- Sicherheitsstatus nur aus dem signierten Manifest, nicht aus Release Notes.
- Ziel = Ordner des Updaters, nur die zwei `.exe`; Commit-Bestätigung und `cargo deny` im Release-Skript.
- Ein Fehler bei der Prüfung beim Start wird nicht angezeigt (kein Netz = normal); ein Fehler beim Installieren schon.

## Dem Planer überlassen
Genaue Texte, Dateinamen im Staging-Ordner, Polling-Intervall für `update.json`, Aufteilung in Module.
