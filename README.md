# Aegis

Offline-Passwortmanager in Rust, nur für Windows, mit nativer GUI (egui) und einem Benutzer.

## Aufbau

- `crates/aegis-core`: Krypto, Dateiformat, atomares Speichern. Ohne GUI, mit Tests.
- `crates/aegis-app`: Oberfläche, nutzt `aegis-core`.
- `deny.toml`: Prüft Lizenzen, Sicherheitslücken und verbotene Netzwerk-Crates.

## Starten

```powershell
cargo test -p aegis-core
cargo run -p aegis-app
```

## Prüfen

```powershell
cargo deny check
cargo build --release --offline
```

## Datenspeicherung

Der Vault liegt unter `%APPDATA%\Aegis\vault.aegis`.
