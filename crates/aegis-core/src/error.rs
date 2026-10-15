use thiserror::Error;

#[derive(Debug, Error)]
pub enum AegisError {
    #[error("Dateifehler: {0}")]
    Io(#[from] std::io::Error),

    #[error("Datenfehler: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Keine Zufallsquelle verfügbar")]
    Random,

    #[error("Schlüsselableitung fehlgeschlagen")]
    Kdf,

    #[error("Falsches Passwort oder beschädigte Datei")]
    Crypto,

    #[error("Ungültiges Dateiformat")]
    Format,

    #[error("Nicht unterstützte Format-Version: {0}")]
    UnsupportedVersion(u8),
}
