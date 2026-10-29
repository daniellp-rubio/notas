//! AES-256-GCM encryption with Argon2id key derivation.
//!
//! Every note that is protected by a password stores its content as
//! `AES-256-GCM(key, nonce, plaintext)` where `key = Argon2id(password, salt)`.
//! Salt and nonce are random per note / per save and are stored next to the
//! ciphertext; neither is secret.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;

/// 32-byte derived key. Wrapped in a zeroizing holder by the state layer.
pub type Key = [u8; KEY_LEN];

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("No se pudo derivar la clave: {0}")]
    Kdf(String),
    #[error("No se pudo obtener aleatoriedad del sistema")]
    Random,
    #[error("No se pudo cifrar el contenido")]
    Encrypt,
    #[error("Contraseña incorrecta o datos dañados")]
    InvalidPassword,
    /// Wrong salt/nonce lengths, i.e. a corrupted row.
    #[error("Datos de cifrado dañados")]
    Corrupted,
}

pub type Result<T, E = CryptoError> = std::result::Result<T, E>;

/// Fresh random bytes from the OS.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| CryptoError::Random)?;
    Ok(out)
}

/// Derive a 256-bit key with Argon2id (19 MiB, 2 passes, 1 lane; 32-byte output).
pub fn derive_key(password: &str, salt: &[u8]) -> Result<Key> {
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::DEFAULT);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    Ok(key)
}

/// Result of sealing a payload: ciphertext plus the parameters needed to open it.
pub struct Sealed {
    pub cipher: Vec<u8>,
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
}

/// Encrypt `plaintext` with `key`, reusing `salt` and generating a fresh nonce.
pub fn seal(key: &Key, salt: [u8; SALT_LEN], plaintext: &[u8]) -> Result<Sealed> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Encrypt)?;
    let nonce_bytes: [u8; NONCE_LEN] = random_bytes()?;
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    Ok(Sealed {
        cipher: ciphertext,
        salt,
        nonce: nonce_bytes,
    })
}

/// Decrypt with a key that is already in memory.
pub fn open(key: &Key, nonce_bytes: [u8; NONCE_LEN], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidPassword)?;
    cipher
        .decrypt(&Nonce::from(nonce_bytes), ciphertext)
        .map_err(|_| CryptoError::InvalidPassword)
}

/// First-time lock: fresh salt, derive the key, seal the payload.
pub fn seal_with_password(password: &str, plaintext: &[u8]) -> Result<(Key, Sealed)> {
    let salt: [u8; SALT_LEN] = random_bytes()?;
    let key = derive_key(password, &salt)?;
    let sealed = seal(&key, salt, plaintext)?;
    Ok((key, sealed))
}

/// Unlock: derive the key from the stored salt and decrypt.
///
/// Returns the derived key so the caller can keep it in memory and re-encrypt
/// later saves without asking for the password again.
pub fn open_with_password(
    password: &str,
    ciphertext: &[u8],
    salt: &[u8],
    nonce_bytes: &[u8],
) -> Result<(Key, Vec<u8>)> {
    let salt: [u8; SALT_LEN] = salt.try_into().map_err(|_| CryptoError::Corrupted)?;
    let nonce: [u8; NONCE_LEN] = nonce_bytes.try_into().map_err(|_| CryptoError::Corrupted)?;
    let key = derive_key(password, &salt)?;
    let plaintext = open(&key, nonce, ciphertext)?;
    Ok((key, plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &[u8] = b"MARCADOR_SECRETO_123 un texto de prueba";

    #[test]
    fn roundtrip_with_password() {
        let (key, sealed) = seal_with_password("clave correcta", PLAINTEXT).expect("seal");
        let (derived, plain) =
            open_with_password("clave correcta", &sealed.cipher, &sealed.salt, &sealed.nonce)
                .expect("open");
        assert_eq!(plain, PLAINTEXT);
        assert_eq!(derived, key);
    }

    #[test]
    fn wrong_password_fails() {
        let (_key, sealed) = seal_with_password("clave correcta", PLAINTEXT).expect("seal");
        let err = open_with_password("otra clave", &sealed.cipher, &sealed.salt, &sealed.nonce)
            .expect_err("wrong password must fail");
        assert!(matches!(err, CryptoError::InvalidPassword));
    }

    #[test]
    fn same_plaintext_gives_different_ciphertext() {
        let (key, salt) = {
            let salt: [u8; SALT_LEN] = random_bytes().expect("salt");
            let key = derive_key("clave", &salt).expect("kdf");
            (key, salt)
        };
        let a = seal(&key, salt, PLAINTEXT).expect("seal a");
        let b = seal(&key, salt, PLAINTEXT).expect("seal b");
        assert_ne!(a.nonce, b.nonce, "nonce must be random per save");
        assert_ne!(a.cipher, b.cipher, "ciphertext must differ per save");
        assert_eq!(a.salt, b.salt);
    }

    #[test]
    fn ciphertext_does_not_leak_plaintext() {
        let (_key, sealed) = seal_with_password("clave", PLAINTEXT).expect("seal");
        assert!(
            !sealed
                .cipher
                .windows(PLAINTEXT.len())
                .any(|w| w == PLAINTEXT),
            "ciphertext must not contain the plaintext"
        );
        assert_eq!(sealed.salt.len(), SALT_LEN);
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        assert_eq!(sealed.cipher.len(), PLAINTEXT.len() + 16, "GCM tag is 16 bytes");
    }

    #[test]
    fn corrupted_nonce_is_rejected() {
        let (_key, sealed) = seal_with_password("clave", PLAINTEXT).expect("seal");
        let err = open_with_password("clave", &sealed.cipher, &sealed.salt, &[0u8; 5])
            .expect_err("short nonce must fail");
        assert!(matches!(err, CryptoError::Corrupted));
    }
}
