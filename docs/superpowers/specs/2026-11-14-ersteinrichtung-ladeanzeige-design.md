# Ersteinrichtung und Ladeanzeige – Design (kurz)

Stand: 2026-11-14 · Basis: v0.0.9 · vom Nutzer freigegeben

## Absicht
Beim ersten Start soll Aegis direkt zum Festlegen des Master-Passworts auffordern. Beim Entsperren
und Anlegen soll eine Ladeanzeige im Button zeigen, dass gearbeitet wird.

## Ziele / Nicht-Ziele
- Ziel: Ohne Tresordatei startet die App im Anlegen-Modus („Master-Passwort festlegen“), ohne Umweg über einen Link.
- Ziel: Argon2 läuft beim Entsperren/Anlegen in einem Hintergrund-Thread; der Button zeigt Spinner und
  „Wird entsperrt…“ / „Wird angelegt…“, Eingaben und Button sind währenddessen gesperrt.
- Nicht-Ziel: Ladeanzeige beim Ändern des Master-Passworts in den Einstellungen.
- Nicht-Ziel: Änderungen an `aegis-core` oder am Dateiformat.
- Anti-Ziel: Doppeltes Auslösen (zweiter Klick/Enter während der Arbeit), eingefrorener Spinner.

## Ansätze
| Ansatz | Abwägung |
|---|---|
| **Hintergrund-Thread + Kanal, im Frame abfragen (gewählt)** | Einfach, keine neue Abhängigkeit, `Session` ist `Send` |
| Spinner ohne Thread | Funktioniert nicht: UI-Thread blockiert, Spinner friert ein |
| Async-Runtime | Neue Abhängigkeit für einen einzigen Aufruf – unverhältnismäßig |

## Design
- `creating`-Flag entfällt: Anlegen-Modus genau dann, wenn die Tresordatei fehlt. Link „Noch kein Tresor? Neu anlegen“
  und „Zurück zum Entsperren“ entfallen.
- Texte Ersteinrichtung: Überschrift „Master-Passwort festlegen“, Hinweis „Willkommen bei Aegis. Dieses Passwort öffnet
  deinen Tresor und lässt sich nicht wiederherstellen.“, Feldbeschriftung „Master-Passwort (mind. 12 Zeichen)“.
- Neues Modul `task.rs`: generischer Hintergrund-Job (Thread + Kanal), testbar ohne GUI.
  Bricht der Thread ab (Panik), wird das als Fehler gemeldet, nie als ewiges Laden.
- Prüfungen (leeres Passwort, Mindestlänge, Übereinstimmung, Ordner anlegen) laufen weiter sofort im UI-Thread,
  nur `Session::create`/`Session::open` im Hintergrund.

## Dem Planer überlassen
Spinner-Größe/Position im Button, Repaint-Strategie.
