//! Netzwerkfreie Bausteine für sichere Updates: Version, signiertes Manifest, Prüfung.

mod keys;
mod manifest;
mod status;
mod version;

pub use keys::TRUSTED_KEYS;
pub use manifest::{
    decide, sha256_hex, verify, Decision, FileEntry, Manifest, UpdateError, VALIDITY_SECS,
};
pub use status::{now, read_status, staging_dir, status_path, write_status, UpdateStatus};
pub use version::{installed, Version};
