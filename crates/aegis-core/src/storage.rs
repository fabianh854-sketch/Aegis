use std::{fs::File, io::Write, path::Path};

use crate::error::AegisError;

/// Schreibt in eine temporäre Datei, sichert sie auf Platte und ersetzt dann
/// die Zieldatei. Bei einem Absturz bleibt entweder die alte oder die neue Datei.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), AegisError> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?; // unter Windows wird die Zieldatei ersetzt
    Ok(())
}
