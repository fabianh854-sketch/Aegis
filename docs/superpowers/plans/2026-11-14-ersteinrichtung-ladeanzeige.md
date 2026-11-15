# Ersteinrichtung und Ladeanzeige Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ohne Tresor direkt „Master-Passwort festlegen“ zeigen; Entsperren/Anlegen im Hintergrund mit Spinner im Button.

**Architecture:** Neues Modul `task.rs` (generischer Hintergrund-Job: Thread + `mpsc`-Kanal, pro Frame abgefragt). `app.rs` startet `Session::open`/`create` darüber und leitet Modus und Texte aus „Datei existiert?“ und „Job läuft?“ ab.

**Tech Stack:** Rust stable, std::thread/std::sync::mpsc, egui 0.29 (`egui::Spinner`).

**Spec:** `docs/superpowers/specs/2026-11-14-ersteinrichtung-ladeanzeige-design.md`

## Global Constraints

- **Nichts committen** (ausdrücklicher Wunsch des Nutzers). Keine Commit-Schritte.
- Keine neuen Crates, keine Änderung an `aegis-core`.
- UI-Texte exakt: „Master-Passwort festlegen“, „Willkommen bei Aegis. Dieses Passwort öffnet deinen Tresor und lässt sich nicht wiederherstellen.“, „Master-Passwort (mind. 12 Zeichen)“, „Wird entsperrt…“, „Wird angelegt…“. Bestehend: „Tresor entsperren“, „Entsperren“, „Tresor anlegen“, „Master-Passwort“, „Passwort bestätigen“.
- `cargo clippy --workspace --all-targets -- -D warnings` bleibt sauber.

## Review Focus

- Thread bricht ab (Panik) → Fehlermeldung, kein ewiger Spinner (Test in Task 1).
- Doppelklick/Enter während der Job läuft → kein zweiter Job (Absicherung in Task 2: `unlock()` kehrt sofort zurück, wenn `pending.is_some()`).
- Falsches Passwort → Spinner verschwindet, Fehlermeldung, Passwortfeld bleibt gefüllt zum Korrigieren (Task 2).
- Datei wird während der Ersteinrichtung angelegt (Job fertig) → App geht in die Hauptansicht, nicht zurück in den Entsperren-Bildschirm (Task 2: Erfolg setzt `session`).
- Leeres Passwort / zu kurz → Meldung sofort, ohne Spinner (Task 2: Prüfungen vor dem Start).

---

### Task 1: `task.rs` – Hintergrund-Job

**Files:**
- Create: `crates/aegis-app/src/task.rs`
- Modify: `crates/aegis-app/src/main.rs` (`mod task;`)

**Interfaces:**
- Produces:
```rust
pub struct Task<T> { rx: std::sync::mpsc::Receiver<Result<T, String>> }
impl<T: Send + 'static> Task<T> {
    /// Startet `job` in einem eigenen Thread. `on_done` läuft im Thread nach dem Senden (für ctx.request_repaint()).
    pub fn spawn(job: impl FnOnce() -> Result<T, String> + Send + 'static, on_done: impl FnOnce() + Send + 'static) -> Self;
    /// None = läuft noch. Some(Err("Vorgang unerwartet abgebrochen")) wenn der Thread ohne Ergebnis endet.
    pub fn poll(&self) -> Option<Result<T, String>>;
}
```

- [ ] **Step 1: Failing tests**
```rust
#[test] fn pending_until_job_finishes() { /* Job wartet auf einen zweiten Kanal; poll() == None; freigeben; bis 2 s pollen → Some(Ok(42)) */ }
#[test] fn job_error_is_reported() { /* Job gibt Err("x") → irgendwann Some(Err("x")) */ }
#[test] fn panic_is_reported_not_pending_forever() { /* Job panict → irgendwann Some(Err("Vorgang unerwartet abgebrochen")) */ }
#[test] fn on_done_runs_after_result() { /* on_done setzt AtomicBool; nach Ok ist es true */ }
```
- [ ] **Step 2:** `cargo test -p aegis-app task` → FAIL (nicht kompilierbar).
- [ ] **Step 3:** Implementieren (`try_recv`: `Empty` → None, `Disconnected` → Abbruch-Fehler).
- [ ] **Step 4:** `cargo test -p aegis-app` → PASS.

### Task 2: Ersteinrichtung und Spinner in `app.rs`

**Files:**
- Modify: `crates/aegis-app/src/app.rs`

**Interfaces:**
- Consumes: `Task<Session>` (Task 1).
- Produces (privat, getestet): `fn unlock_texts(first_start: bool, busy: bool) -> UnlockTexts` mit
  `struct UnlockTexts { title: &'static str, button: &'static str }`:
  - (true, false) → ("Master-Passwort festlegen", "Tresor anlegen"); (true, true) → (…, "Wird angelegt…")
  - (false, false) → ("Tresor entsperren", "Entsperren"); (false, true) → (…, "Wird entsperrt…")

Änderungen:
- Feld `creating` entfernen; neues Feld `pending: Option<Task<Session>>`.
- `unlock_view`: `first_start = path ok && !exists`. Hinweistext und Feldbeschriftung bei Ersteinrichtung laut Global Constraints; sonst bisheriger Text. Bestätigungsfeld nur bei Ersteinrichtung. Links „Noch kein Tresor? Neu anlegen“ / „Zurück zum Entsperren“ entfernen. Während `pending`: Eingaben per `ui.add_enabled(false, …)`, Button deaktiviert mit `egui::Spinner` links vom Text, Enter ignoriert. Höhe der Spalte ggf. anpassen.
- `unlock(first_start)`: zurück, wenn `pending.is_some()`. Prüfungen wie bisher sofort; danach Passwort als `Zeroizing<String>` kopieren und `Task::spawn(move || Session::create/open(...).map_err(|e| e.to_string()), move || ctx.request_repaint())` – dafür `ctx: egui::Context` als Parameter.
- `update`: wenn `pending` → `poll()`: `Some(Ok(s))` → wie bisher Erfolg (Session setzen, Felder nullen, Nav, Aktivität); `Some(Err(e))` → Status rot, Passwortfeld bleibt; `None` → `ctx.request_repaint_after(Duration::from_millis(100))`.

- [ ] **Step 1: Failing test** `unlock_texts_cover_all_states` mit den vier Fällen oben.
- [ ] **Step 2:** `cargo test -p aegis-app unlock_texts` → FAIL.
- [ ] **Step 3:** Änderungen umsetzen.
- [ ] **Step 4:** `cargo test --workspace` → PASS; `cargo clippy --workspace --all-targets -- -D warnings` → sauber; `cargo build -p aegis-app` (bzw. `cargo check`, falls die .exe gesperrt ist) → ok.
- [ ] **Step 5:** Manuell mit `APPDATA` auf leeren Temp-Ordner: Start zeigt „Master-Passwort festlegen“; anlegen zeigt Spinner, danach Hauptansicht; sperren, falsches Passwort → Spinner, dann Fehlermeldung.
