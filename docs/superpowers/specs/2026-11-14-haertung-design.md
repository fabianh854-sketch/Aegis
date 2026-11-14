# Aegis: Härtung und Bugfixes – Design

Stand: 2026-11-14 · Basis: v0.0.8 (`8852960`)

## Absicht

Aegis ist ein Offline-Passwortmanager für einen Benutzer unter Windows. Ziel dieser Runde ist,
bestehende Sicherheitslücken und Bugs zu schließen, **ohne neue Funktionen**. Die sicherheitskritische
Logik soll aus der UI herausgelöst und mit Tests abgesichert werden.

## Ziele

1. Kein Klartext-Passwort bleibt länger als nötig in Zwischenablage oder Speicher.
2. Eine manipulierte oder kaputte Tresordatei kann die App nicht zum Absturz bringen und keinen
   bestehenden Tresor überschreiben.
3. Speichern blockiert die Oberfläche nicht mehr (keine Argon2-Ableitung pro Speichern).
4. Das Master-Passwort lässt sich nur mit dem aktuellen Passwort ändern und hat eine Mindestlänge.
5. Sicherheitslogik (Session, Zwischenablage) ist durch Tests abgedeckt.

## Nicht-Ziele

- Neue Funktionen (Export, Import, URL-Feld, TOTP, Tastenkürzel, Sortierung).
- Änderung des Dateiformats: **v1 bleibt**, bestehende Tresore öffnen sich unverändert.
- Argon2 beim Entsperren in einen Hintergrund-Thread verlagern (Variante C, später).
- Aufteilen von `app.rs` in Ansichten, optische Änderungen an der UI.
- CI-Workflow für Pull Requests.

## Anti-Ziele (gilt als gescheitert, auch wenn es „funktioniert“)

- Ein bestehender Tresor wird unlesbar oder lässt sich mit dem bisherigen Passwort nicht mehr öffnen.
- Ein neues Crate mit Netzwerkzugriff oder eine neue, nicht in `deny.toml` erlaubte Lizenz kommt dazu.
- Ein Fehler wird still geschluckt (z. B. Backup schlägt fehl, App tut so, als sei gespeichert).

## Rahmenbedingungen

- Rust stable (getestet mit 1.96), egui/eframe 0.29, nur Windows.
- `cargo deny check` muss weiter bestehen. `arboard` 3.6 ist bereits über eframe im Baum und darf
  direkt als Abhängigkeit von `aegis-app` eingetragen werden.
- UI-Texte bleiben Deutsch.

## Ansätze

| Variante | Inhalt | Abwägung |
|---|---|---|
| A: Minimal | Jede Lücke direkt in `app.rs` flicken | Kleinster Diff, aber Sicherheitslogik bleibt in 1100 Zeilen UI verstreut und ungetestet |
| **B: Session-Modul (gewählt)** | `Session` in `aegis-core`, `clipboard`-Modul in `aegis-app`, `app.rs` ruft nur noch auf | Riskante Teile testbar, UI-Optik unverändert, mittlerer Umfang |
| C: B + Hintergrund-Thread + Ansichten-Split | Zusätzlich kein Einfrieren beim Entsperren | Fasst jede UI-Zeile an, viel Umbau für kleinen Gewinn |

Gewählt vom Nutzer: **B**.

## Design

### aegis-core

**`Session`** (neu, `session.rs`) ersetzt die öffentlichen Funktionen `load`/`save`/`save_with_params`:

```rust
pub struct Session { /* path, header [u8; HEADER_LEN], key Zeroizing<[u8; 32]>, vault Vault */ }

impl Session {
    pub fn create(path: &Path, master: &[u8], params: &KdfParams) -> Result<Session, AegisError>;
    pub fn open(path: &Path, master: &[u8]) -> Result<Session, AegisError>;
    pub fn vault(&self) -> &Vault;
    pub fn commit(&mut self, candidate: Vault) -> Result<(), AegisError>;
    pub fn change_master(&mut self, current: &[u8], new: &[u8], params: &KdfParams) -> Result<(), AegisError>;
    pub fn path(&self) -> &Path;
}
```

- `open` liest Header, prüft Parameter-Grenzen, leitet den Schlüssel einmal ab und behält Schlüssel
  und Header im Speicher.
- `commit` verschlüsselt `candidate` mit dem gehaltenen Schlüssel, **gleichem Salt und Header-Feldern**
  und **neuer zufälliger Nonce**, schreibt die Datei und übernimmt `candidate` erst bei Erfolg.
  (XChaCha20 mit 24-Byte-Zufallsnonce: Nonce-Wiederholung praktisch ausgeschlossen.)
- `create` schlägt mit neuem Fehler `AegisError::AlreadyExists` fehl, wenn die Datei existiert
  (atomar über `OpenOptions::create_new`, kein `exists()`-Check vorher).
- `change_master` prüft `current` durch Ableiten mit gespeichertem Salt/Parametern und
  konstantzeitigem Vergleich mit dem gehaltenen Schlüssel (neuer Fehler `AegisError::WrongPassword`),
  erzeugt dann neues Salt, neuen Schlüssel, schreibt neu. Mindestlänge prüft die App, nicht der Kern.
- `Drop`: Schlüssel und Vault werden genullt.

**Parameter-Grenzen beim Laden** (neuer Fehler `AegisError::UnsafeParams`): `1 ≤ m_cost ≤ 1 GiB`
(1 048 576 KiB), `1 ≤ t_cost ≤ 16`, `1 ≤ p_cost ≤ 16`. Die Prüfung passiert **vor** der Ableitung.
Die Untergrenze bleibt niedrig, damit Tests mit schnellen Parametern laufen.

**Nullen**: `Entry` und `Vault` bekommen `Zeroize` + `ZeroizeOnDrop` (`zeroize` mit `derive` ist schon da).

**Atomares Schreiben** (`storage.rs`):
1. Temp-Datei mit zufälligem Suffix (`vault.aegis.<hex>.tmp`) per `create_new` anlegen, schreiben, `sync_all`.
2. Existiert die Zieldatei, wird sie nach `vault.aegis.bak` kopiert (Fehler → Abbruch, Temp-Datei wird
   gelöscht, Fehler geht an die UI).
3. Temp-Datei über die Zieldatei umbenennen.
Bei `change_master` wird `.bak` danach mit der neuen Datei überschrieben, damit kein Backup mit dem
alten Passwort liegen bleibt.

### aegis-app

**`clipboard.rs`** (neu): `ClipboardGuard`
- `copy(secret)`: schreibt über `arboard` mit `exclude_from_history()` und `exclude_from_cloud()`
  (Windows-Erweiterung), merkt sich den Wert als `Zeroizing<String>` und ggf. den Zeitpunkt zum Leeren.
- `tick(now)`: Nach Ablauf leeren, **aber nur, wenn die Zwischenablage noch genau unseren Wert enthält**.
- `clear_now()`: dieselbe Regel, sofort. Aufgerufen bei Sperren (manuell und automatisch) und in
  `eframe::App::on_exit`.
- Die Regel „nur unseren Wert löschen“ steckt in einer reinen Funktion, die ohne echte Zwischenablage
  getestet wird.
- **Nicht prüfbarer Zustand** (Entscheidung des Planers, s. u.):
  - Schlägt das Kopieren über `arboard` fehl, wird auf `ctx.copy_text` zurückgefallen und ein
    Hinweis angezeigt („Verlauf konnte nicht ausgeschlossen werden“). Der Inhalt gilt als „nicht prüfbar“.
  - Schlägt beim Leeren nach Ablauf das Lesen fehl (anderer Prozess hält die Zwischenablage, kein Text),
    wird beim nächsten Tick (≈ 1 s) erneut versucht.
  - Bei Sperren und Beenden wird ein nicht prüfbarer Inhalt **trotzdem geleert**: Ein Passwort, das
    liegen bleibt, wiegt schwerer als ein versehentlich gelöschter fremder Text.

**`app.rs`**
- `vault` + `master` werden zu `session: Option<Session>`. Das Master-Passwort wird nach dem Entsperren
  nicht mehr gehalten.
- Eingabefelder `password`, `confirm`, `new_master`, `new_master_confirm`, `gen_password` werden
  `Zeroizing<String>`.
- Master-Passwort ändern: drittes Feld „Aktuelles Passwort“. Mindestlänge 12 Zeichen für neue und
  geänderte Master-Passwörter (bestehende kürzere bleiben öffnungsfähig).
- Abbrechen im Editor mit Änderungen: zweistufig wie Löschen („Änderungen verwerfen?“).
- „Unlock Vault“ → „Tresor entsperren“. Speicherort in den Einstellungen zeigt den echten Pfad.
- Fehlt `APPDATA`: kein Rückfall auf `.`, stattdessen Fehlermeldung auf dem Entsperren-Bildschirm.
- Sperren leert die Zwischenablage (nach obiger Regel), wenn „Zwischenablage leeren“ aktiv ist.

## Tests (neu)

- Core: Roundtrip über `Session`; `create` auf existierende Datei → `AlreadyExists`, Datei unverändert;
  Header mit `m_cost = u32::MAX` → `UnsafeParams` ohne Ableitung; `commit` ändert Nonce, behält Salt;
  `change_master` mit falschem aktuellem Passwort → `WrongPassword`, Datei unverändert;
  nach `change_master` öffnet nur das neue Passwort Datei **und** `.bak`; Datei aus v0.0.8 (mit
  `save_with_params` erzeugt) öffnet weiterhin. Tests nutzen eindeutige Temp-Verzeichnisse.
- App: Clipboard-Regel (leeren nur bei eigenem Wert, Zeitpunkt), Mindestlängen-Prüfung.

## Dem Planer überlassen

- Genaue Fehlertexte, solange Deutsch und verständlich.
- Ob die konstantzeitige Prüfung über `subtle` (schon transitiv vorhanden) oder eine eigene Schleife läuft.
- Reihenfolge der Umsetzung.

## Entscheidungen des Planers (zum Gegenlesen)

- Grenzen der Argon2-Parameter (oben).
- Mindestlänge 12 nur für neue/geänderte Master-Passwörter.
- Leeren bei Sperren/Beenden nur, wenn die Einstellung aktiv ist.
- Zwischenablage nicht prüfbar: beim Timer erneut versuchen, bei Sperren/Beenden trotzdem leeren.
- Kompatibilitätstest: eine Datei im v1-Format wird im Test direkt aus Header-Bytes erzeugt,
  da `save_with_params` entfällt.
- `.bak`-Backup, nach Passwortwechsel überschrieben.
- Bekannte Grenze: egui hält intern Kopien von Text (Undo, Layout). Die kann Aegis nicht nullen.
