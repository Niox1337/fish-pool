use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;

/// Generates a random 32 byte key encoded as standard base64.
pub fn random_key_b64() -> String {
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    B64.encode(key)
}

/// Decrypts base64(nonce12 || ciphertext || tag16) with a base64 encoded AES-256 key.
pub fn decrypt_b64(key_b64: &str, payload_b64: &str) -> Result<String> {
    let key = B64.decode(key_b64.trim()).map_err(|_| anyhow!("密钥解码失败"))?;
    let raw = B64.decode(payload_b64.trim()).map_err(|_| anyhow!("密文解码失败"))?;
    if key.len() != 32 {
        return Err(anyhow!("密钥长度错误"));
    }
    if raw.len() <= 28 {
        return Err(anyhow!("密文格式错误"));
    }
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("密钥长度错误"))?;
    let (nonce, rest) = raw.split_at(12);
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), rest)
        .map_err(|_| anyhow!("解密失败"))?;
    String::from_utf8(plain).map_err(|_| anyhow!("明文不是 UTF-8"))
}

/// Encrypts to base64(nonce12 || ciphertext || tag16) with a base64 encoded AES-256 key.
pub fn encrypt_b64(key_b64: &str, plain: &str) -> Result<String> {
    let key = B64.decode(key_b64.trim())?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("密钥长度错误"))?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
        .map_err(|_| anyhow!("加密失败"))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = random_key_b64();
        let enc = encrypt_b64(&key, "app-secret-值").unwrap();
        assert_eq!(decrypt_b64(&key, &enc).unwrap(), "app-secret-值");
    }

    #[test]
    fn known_vector() {
        // key = 32 zero bytes, nonce = 12 zero bytes, plaintext "hi"
        let key = B64.encode([0u8; 32]);
        let cipher = Aes256Gcm::new_from_slice(&[0u8; 32]).unwrap();
        let ct = cipher.encrypt(Nonce::from_slice(&[0u8; 12]), b"hi".as_ref()).unwrap();
        let mut raw = vec![0u8; 12];
        raw.extend_from_slice(&ct);
        assert_eq!(raw.len(), 12 + 2 + 16);
        assert_eq!(decrypt_b64(&key, &B64.encode(&raw)).unwrap(), "hi");
    }

    #[test]
    fn rejects_tampering_and_bad_input() {
        let key = random_key_b64();
        let enc = encrypt_b64(&key, "secret").unwrap();
        let mut raw = B64.decode(&enc).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(decrypt_b64(&key, &B64.encode(&raw)).is_err());
        assert!(decrypt_b64(&random_key_b64(), &enc).is_err());
        assert!(decrypt_b64(&key, "AAAA").is_err());
        assert!(decrypt_b64("c2hvcnQ=", &enc).is_err());
    }
}
