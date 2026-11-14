use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    crypto::{self, KdfParams, NONCE_LEN, SALT_LEN},
    error::AegisError,
    storage,
};

/// Dateiformat: MAGIC(4) | VERSION(1) | m(4) | t(4) | p(4) | SALT(16) | NONCE(24) | CIPHERTEXT
/// Der Header dient als Associated Data, Manipulationen am Header schlagen also fehl.
const MAGIC: &[u8; 4] = b"AEGS";
const FORMAT_VERSION: u8 = 1;
pub(crate) const HEADER_LEN: usize = 4 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN; // 57

/// Obergrenzen für die Argon2-Parameter aus der Datei. Sie werden vor der Ableitung geprüft,
/// damit eine manipulierte Datei nicht beliebig viel Speicher oder Zeit anfordern kann.
const MAX_M_COST: u32 = 1024 * 1024; // 1 GiB in KiB
const MAX_T_COST: u32 = 16;
const MAX_P_COST: u32 = 16;

#[derive(Debug, Default, Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Vault {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Entry {
    pub title: String,
    pub username: String,
    pub password: String,
    pub notes: String,
}

/// Unverschlüsselter Dateikopf. Sein serialisierter Inhalt ist die Associated Data.
#[derive(Debug, Clone)]
pub(crate) struct Header {
    pub params: KdfParams,
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..4].copy_from_slice(MAGIC);
        out[4] = FORMAT_VERSION;
        out[5..9].copy_from_slice(&self.params.m_cost.to_le_bytes());
        out[9..13].copy_from_slice(&self.params.t_cost.to_le_bytes());
        out[13..17].copy_from_slice(&self.params.p_cost.to_le_bytes());
        out[17..33].copy_from_slice(&self.salt);
        out[33..57].copy_from_slice(&self.nonce);
        out
    }

    /// Liest den Kopf vom Anfang von `data` und prüft Magic, Version und Parametergrenzen.
    pub fn parse(data: &[u8]) -> Result<Header, AegisError> {
        if data.len() < HEADER_LEN || data[..4] != MAGIC[..] {
            return Err(AegisError::Format);
        }
        if data[4] != FORMAT_VERSION {
            return Err(AegisError::UnsupportedVersion(data[4]));
        }
        let word = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let params = KdfParams { m_cost: word(5), t_cost: word(9), p_cost: word(13) };
        let within = |v: u32, max: u32| (1..=max).contains(&v);
        if !within(params.m_cost, MAX_M_COST)
            || !within(params.t_cost, MAX_T_COST)
            || !within(params.p_cost, MAX_P_COST)
        {
            return Err(AegisError::UnsafeParams);
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&data[17..33]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&data[33..57]);
        Ok(Header { params, salt, nonce })
    }
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
    let header = Header {
        params: params.clone(),
        salt: crypto::random_bytes::<SALT_LEN>()?,
        nonce: crypto::random_bytes::<NONCE_LEN>()?,
    };
    let encoded = header.encode();
    let key = crypto::derive_key(master, &header.salt, params)?;
    let plaintext = Zeroizing::new(serde_json::to_vec(vault)?);
    let ciphertext = crypto::seal(&key, &header.nonce, &encoded, &plaintext)?;

    let mut file = encoded.to_vec();
    file.extend_from_slice(&ciphertext);
    storage::write_atomic(path, &file)
}

pub fn load(path: &Path, master: &[u8]) -> Result<Vault, AegisError> {
    let data = fs::read(path)?;
    let header = Header::parse(&data)?;
    let key = crypto::derive_key(master, &header.salt, &header.params)?;
    let plaintext = Zeroizing::new(crypto::open(&key, &header.nonce, &data[..HEADER_LEN], &data[HEADER_LEN..])?);
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

    fn header_with(m: u32, t: u32, p: u32) -> [u8; HEADER_LEN] {
        Header {
            params: KdfParams { m_cost: m, t_cost: t, p_cost: p },
            salt: [0; SALT_LEN],
            nonce: [0; NONCE_LEN],
        }
        .encode()
    }

    #[test]
    fn header_roundtrip() {
        let h = Header { params: fast(), salt: [1; SALT_LEN], nonce: [2; NONCE_LEN] };
        let p = Header::parse(&h.encode()).unwrap();
        assert_eq!(p.salt, [1; SALT_LEN]);
        assert_eq!(p.nonce, [2; NONCE_LEN]);
        assert_eq!(p.params.m_cost, 8);
    }

    #[test]
    fn huge_memory_cost_is_rejected() {
        let mut b = header_with(8, 1, 1);
        b[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(Header::parse(&b), Err(AegisError::UnsafeParams)));
    }

    #[test]
    fn zero_and_large_time_or_parallelism_rejected() {
        for (m, t, p) in [(0, 1, 1), (8, 0, 1), (8, 17, 1), (8, 1, 0), (8, 1, 17), (1_048_577, 1, 1)] {
            assert!(
                matches!(Header::parse(&header_with(m, t, p)), Err(AegisError::UnsafeParams)),
                "m={m} t={t} p={p}"
            );
        }
        assert!(Header::parse(&header_with(1_048_576, 16, 16)).is_ok());
    }

    #[test]
    fn short_or_foreign_file_is_format_error() {
        assert!(matches!(Header::parse(b"AEG"), Err(AegisError::Format)));
        assert!(matches!(Header::parse(&[0u8; 80]), Err(AegisError::Format)));
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
