use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key, XChaCha20Poly1305, XNonce,
};
use zeroize::Zeroizing;

use crate::error::AegisError;

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 24;
pub const KEY_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct KdfParams {
    pub m_cost: u32, // Speicher in KiB
    pub t_cost: u32, // Iterationen
    pub p_cost: u32, // Parallelität
}

impl Default for KdfParams {
    fn default() -> Self {
        Self { m_cost: 64 * 1024, t_cost: 3, p_cost: 1 }
    }
}

pub fn random_bytes<const N: usize>() -> Result<[u8; N], AegisError> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|_| AegisError::Random)?;
    Ok(buf)
}

pub fn derive_key(
    master: &[u8],
    salt: &[u8],
    params: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, AegisError> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
        .map_err(|_| AegisError::Kdf)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);

    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(master, salt, &mut key[..])
        .map_err(|_| AegisError::Kdf)?;
    Ok(key)
}

pub fn seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, AegisError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(XNonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| AegisError::Crypto)
}

pub fn open(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, AegisError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| AegisError::Crypto)
}
