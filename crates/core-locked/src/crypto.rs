use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("kunci tidak valid")]
    BadKey,
    #[error("data terenkripsi rusak atau kunci salah")]
    Decrypt,
    #[error("gagal membuat hash")]
    Hash,
}

const PREFIX: &str = "v1:";

/// Enkripsi rahasia (kata sandi router, API key AI) dengan XChaCha20-Poly1305.
#[derive(Clone)]
pub struct Cipher(XChaCha20Poly1305);

impl Cipher {
    pub fn from_base64(key: &str) -> Result<Self, CryptoError> {
        let bytes = STANDARD
            .decode(key.trim())
            .map_err(|_| CryptoError::BadKey)?;
        if bytes.len() != 32 {
            return Err(CryptoError::BadKey);
        }
        Ok(Self(
            XChaCha20Poly1305::new_from_slice(&bytes).map_err(|_| CryptoError::BadKey)?,
        ))
    }

    pub fn generate_key() -> String {
        STANDARD.encode(random_bytes::<32>())
    }

    pub fn encrypt(&self, plain: &str) -> String {
        let nonce = random_bytes::<24>();
        let sealed = self
            .0
            .encrypt(&XNonce::from(nonce), plain.as_bytes())
            .expect("enkripsi");
        let mut out = nonce.to_vec();
        out.extend(sealed);
        format!("{PREFIX}{}", STANDARD.encode(out))
    }

    pub fn decrypt(&self, sealed: &str) -> Result<String, CryptoError> {
        let data = sealed.strip_prefix(PREFIX).ok_or(CryptoError::Decrypt)?;
        let bytes = STANDARD.decode(data).map_err(|_| CryptoError::Decrypt)?;
        if bytes.len() < 24 {
            return Err(CryptoError::Decrypt);
        }
        let (nonce, body) = bytes.split_at(24);
        let plain = self
            .0
            .decrypt(
                &XNonce::try_from(nonce).map_err(|_| CryptoError::Decrypt)?,
                body,
            )
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("sumber acak OS");
    buf
}

/// Token acak aman untuk URL (sesi, CSRF).
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    Argon2::default()
        .hash_password_with_salt(password.as_bytes(), &random_bytes::<16>())
        .map(|h| h.to_string())
        .map_err(|_| CryptoError::Hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_roundtrip() {
        let c = Cipher::from_base64(&Cipher::generate_key()).unwrap();
        let sealed = c.encrypt("rahasia");
        assert_ne!(sealed, "rahasia");
        assert_eq!(c.decrypt(&sealed).unwrap(), "rahasia");
        let other = Cipher::from_base64(&Cipher::generate_key()).unwrap();
        assert!(other.decrypt(&sealed).is_err());
    }

    #[test]
    fn password_hash() {
        let h = hash_password("benar").unwrap();
        assert!(verify_password("benar", &h));
        assert!(!verify_password("salah", &h));
    }
}
