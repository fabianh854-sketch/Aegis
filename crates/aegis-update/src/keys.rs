//! Vertrauenswürdige Minisign-Public-Keys (Base64, eine Zeile je Schlüssel).

/// Jeder Eintrag darf Release-Manifeste signieren. Ein Wechsel geht nur über ein Release,
/// das mit einem hier bereits eingetragenen Schlüssel signiert ist.
pub const TRUSTED_KEYS: &[&str] = &[
    // Hauptschlüssel, Key-ID C4FE01C4EC24E778, erzeugt am 2026-11-15.
    "RWR45yTsxAH+xHgzb35rEMu0rj9MgdWjFK9okZwflgA5TPikEYDPXK8M",
];
