use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    crypto::{self, KdfParams, NONCE_LEN, SALT_LEN},
    error::AegisError,
    storage,
};

/// Dateiformat: MAGIC(4) | VERSION(1) | m(4) | t(4) | p(4) | SALT(16) | NONCE(24) | CIPHERTEXT
/// Der Header dient als Associated Data, Manipulationen am Header schlagen also fehl.
const MAGIC: &[u8; 4] = b"AEGS";
const FORMAT_VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN; // 57

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Vault {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Entry {
    pub title: String,
    pub username: String,
    pub password: String,
    pub notes: String,
}

pub fn save(path: &Path, master: &[u8], vault: &Vault) -> Result<(), AegisError> {
    save_with_params(path, master, vault, &KdfParams::default())
}

pub fn save_with_params(
    path: &Path,
    master: &[u8],
    vault: &Vault,
    params: &KdfParams,
) -> Result<(), AegisError> {
    let salt = crypto::random_bytes::<SALT_LEN>()?;
    let nonce = crypto::random_bytes::<NONCE_LEN>()?;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(FORMAT_VERSION);
    header.extend_from_slice(&params.m_cost.to_le_bytes());
    header.extend_from_slice(&params.t_cost.to_le_bytes());
    header.extend_from_slice(&params.p_cost.to_le_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);

    let key = crypto::derive_key(master, &salt, params)?;
    let plaintext = Zeroizing::new(serde_json::to_vec(vault)?);
    let ciphertext = crypto::seal(&key, &nonce, &header, &plaintext)?;

    let mut file = header;
    file.extend_from_slice(&ciphertext);
    storage::write_atomic(path, &file)
}

pub fn load(path: &Path, master: &[u8]) -> Result<Vault, AegisError> {
    let data = fs::read(path)?;

    if data.len() < HEADER_LEN || data[..4] != MAGIC[..] {
        return Err(AegisError::Format);
    }
    if data[4] != FORMAT_VERSION {
        return Err(AegisError::UnsupportedVersion(data[4]));
    }

    // Längen sind oben geprüft, daher ist unwrap hier unkritisch.
    let params = KdfParams {
        m_cost: u32::from_le_bytes(data[5..9].try_into().unwrap()),
        t_cost: u32::from_le_bytes(data[9..13].try_into().unwrap()),
        p_cost: u32::from_le_bytes(data[13..17].try_into().unwrap()),
    };
    let salt: [u8; SALT_LEN] = data[17..33].try_into().unwrap();
    let nonce: [u8; NONCE_LEN] = data[33..57].try_into().unwrap();

    let header = &data[..HEADER_LEN];
    let key = crypto::derive_key(master, &salt, &params)?;
    let plaintext = Zeroizing::new(crypto::open(&key, &nonce, header, &data[HEADER_LEN..])?);

    Ok(serde_json::from_slice(&plaintext)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fast() -> KdfParams {
        KdfParams { m_cost: 8, t_cost: 1, p_cost: 1 }
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aegis_test_{name}.aegis"))
    }

    fn sample() -> Vault {
        Vault {
            entries: vec![Entry {
                title: "GitHub".into(),
                username: "fabian".into(),
                password: "geheim".into(),
                notes: String::new(),
            }],
        }
    }

    #[test]
    fn roundtrip() {
        let path = temp("roundtrip");
        save_with_params(&path, b"master", &sample(), &fast()).unwrap();
        let loaded = load(&path, b"master").unwrap();
        assert_eq!(loaded.entries[0].title, "GitHub");
    }

    #[test]
    fn wrong_password_fails() {
        let path = temp("wrong_pw");
        save_with_params(&path, b"master", &sample(), &fast()).unwrap();
        assert!(load(&path, b"falsch").is_err());
    }

    #[test]
    fn tampered_file_fails() {
        let path = temp("tampered");
        save_with_params(&path, b"master", &sample(), &fast()).unwrap();

        let mut data = fs::read(&path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0x01;
        fs::write(&path, &data).unwrap();

        assert!(load(&path, b"master").is_err());
    }
}
