//! Netzwerkfreie Bausteine für sichere Updates: Version, signiertes Manifest, Prüfung.

mod keys;
mod manifest;
mod version;

pub use keys::TRUSTED_KEYS;
pub use manifest::{
    decide, sha256_hex, verify, Decision, FileEntry, Manifest, UpdateError, VALIDITY_SECS,
};
pub use version::{installed, Version};
