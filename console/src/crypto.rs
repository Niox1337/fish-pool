//! Hashing, HMAC request signing, AES-256-GCM secret storage and PBKDF2 password hashing.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const PBKDF2_ITERATIONS: u32 = 100_000;
/// Allowed clock difference for signed node requests.
pub const SIG_MAX_SKEW_MS: i64 = 60_000;

pub fn b64(data: &[u8]) -> String {
    B64.encode(data)
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    B64.decode(s.trim()).ok()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Constant time byte comparison.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Decodes a base64 32 byte key such as `DATA_KEY` or `NODE_SECRET`.
pub fn decode_key32(s: &str) -> Option<[u8; 32]> {
    let v = b64_decode(s)?;
    v.try_into().ok()
}

/// The string that is signed for node requests: `{ts}\n{METHOD}\n{path_with_query}\n{hex(sha256(body))}`.
pub fn sig_payload(ts: i64, method: &str, path_with_query: &str, body: &[u8]) -> String {
    format!(
        "{ts}\n{}\n{path_with_query}\n{}",
        method.to_uppercase(),
        sha256_hex(body)
    )
}

pub fn sign_request(secret: &[u8], ts: i64, method: &str, path_with_query: &str, body: &[u8]) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(sig_payload(ts, method, path_with_query, body).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Debug, PartialEq)]
pub enum SigError {
    Missing,
    Skew,
    Mismatch,
}

/// Checks a signature header pair against the request parts and the current time.
pub fn verify_request(
    secret: &[u8],
    ts_header: Option<&str>,
    sig_header: Option<&str>,
    now_ms: i64,
    method: &str,
    path_with_query: &str,
    body: &[u8],
) -> Result<(), SigError> {
    let (Some(ts), Some(sig)) = (ts_header, sig_header) else {
        return Err(SigError::Missing);
    };
    let ts: i64 = ts.trim().parse().map_err(|_| SigError::Missing)?;
    if (now_ms - ts).abs() > SIG_MAX_SKEW_MS {
        return Err(SigError::Skew);
    }
    let expected = sign_request(secret, ts, method, path_with_query, body);
    if ct_eq(expected.as_bytes(), sig.trim().to_lowercase().as_bytes()) {
        Ok(())
    } else {
        Err(SigError::Mismatch)
    }
}

/// Encrypts to base64(nonce12 || ciphertext || tag16).
pub fn encrypt_with_nonce(key: &[u8; 32], nonce: [u8; 12], plaintext: &[u8]) -> String {
    let cipher = Aes256Gcm::new(key.into());
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .expect("aes-gcm encrypt");
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    b64(&out)
}

pub fn encrypt(key: &[u8; 32], plaintext: &str) -> String {
    encrypt_with_nonce(key, crate::util::random_bytes::<12>(), plaintext.as_bytes())
}

pub fn decrypt(key: &[u8; 32], stored: &str) -> Option<String> {
    let raw = b64_decode(stored)?;
    if raw.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new(key.into());
    let pt = cipher.decrypt(Nonce::from_slice(&raw[..12]), &raw[12..]).ok()?;
    String::from_utf8(pt).ok()
}

/// Parsed `pbkdf2$iterations$salt_b64$hash_b64`.
#[derive(Debug, PartialEq)]
pub struct PasswordHash {
    pub iterations: u32,
    pub salt: Vec<u8>,
    pub hash: Vec<u8>,
}

pub fn format_password_hash(iterations: u32, salt: &[u8], hash: &[u8]) -> String {
    format!("pbkdf2${iterations}${}${}", b64(salt), b64(hash))
}

pub fn parse_password_hash(s: &str) -> Option<PasswordHash> {
    let mut it = s.split('$');
    if it.next()? != "pbkdf2" {
        return None;
    }
    let iterations = it.next()?.parse().ok()?;
    let salt = b64_decode(it.next()?)?;
    let hash = b64_decode(it.next()?)?;
    if it.next().is_some() || hash.is_empty() {
        return None;
    }
    Some(PasswordHash { iterations, salt, hash })
}

#[cfg(target_arch = "wasm32")]
mod subtle {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = ["crypto", "subtle"], js_name = importKey, catch)]
        pub fn import_key(
            format: &str,
            key: &js_sys::Uint8Array,
            alg: &str,
            extractable: bool,
            usages: &js_sys::Array,
        ) -> Result<js_sys::Promise, JsValue>;
        #[wasm_bindgen(js_namespace = ["crypto", "subtle"], js_name = deriveBits, catch)]
        pub fn derive_bits(alg: &JsValue, key: &JsValue, length: u32) -> Result<js_sys::Promise, JsValue>;
    }
}

/// PBKDF2-HMAC-SHA256 producing 32 bytes, through WebCrypto in the Worker.
#[cfg(target_arch = "wasm32")]
pub async fn pbkdf2_sha256(password: &str, salt: &[u8], iterations: u32) -> Result<Vec<u8>, String> {
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;
    let err = |e: JsValue| format!("webcrypto: {e:?}");
    let usages = js_sys::Array::of1(&JsValue::from_str("deriveBits"));
    let pw = js_sys::Uint8Array::from(password.as_bytes());
    let key = JsFuture::from(subtle::import_key("raw", &pw, "PBKDF2", false, &usages).map_err(err)?)
        .await
        .map_err(err)?;
    let alg = js_sys::Object::new();
    let set = |k: &str, v: &JsValue| js_sys::Reflect::set(&alg, &JsValue::from_str(k), v).map(|_| ());
    set("name", &"PBKDF2".into()).map_err(err)?;
    set("hash", &"SHA-256".into()).map_err(err)?;
    set("salt", &js_sys::Uint8Array::from(salt).into()).map_err(err)?;
    set("iterations", &JsValue::from_f64(iterations as f64)).map_err(err)?;
    let bits = JsFuture::from(subtle::derive_bits(&alg, &key, 256).map_err(err)?)
        .await
        .map_err(err)?;
    Ok(js_sys::Uint8Array::new(&bits).to_vec())
}

/// Native PBKDF2 used by unit tests.
#[cfg(not(target_arch = "wasm32"))]
pub async fn pbkdf2_sha256(password: &str, salt: &[u8], iterations: u32) -> Result<Vec<u8>, String> {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2::<HmacSha256>(password.as_bytes(), salt, iterations, &mut out).map_err(|e| e.to_string())?;
    Ok(out.to_vec())
}

pub async fn hash_password(password: &str) -> Result<String, String> {
    let salt = crate::util::random_bytes::<16>();
    let hash = pbkdf2_sha256(password, &salt, PBKDF2_ITERATIONS).await?;
    Ok(format_password_hash(PBKDF2_ITERATIONS, &salt, &hash))
}

pub async fn verify_password(password: &str, stored: &str) -> Result<bool, String> {
    let Some(parsed) = parse_password_hash(stored) else {
        return Ok(false);
    };
    let hash = pbkdf2_sha256(password, &parsed.salt, parsed.iterations).await?;
    Ok(ct_eq(&hash, &parsed.hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures_util::FutureExt::now_or_never(f).expect("ready future")
    }

    #[test]
    fn sha256_known() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_matches_rfc4231_case2() {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac.finalize().into_bytes()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn signature_payload_and_verify() {
        let secret = [7u8; 32];
        assert_eq!(
            sig_payload(1000, "get", "/v1/health", b""),
            "1000\nGET\n/v1/health\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let sig = sign_request(&secret, 1000, "PUT", "/v1/bots/x", b"{}");
        assert_eq!(sig.len(), 64);
        assert_eq!(
            verify_request(&secret, Some("1000"), Some(&sig), 1000, "PUT", "/v1/bots/x", b"{}"),
            Ok(())
        );
        assert_eq!(
            verify_request(
                &secret,
                Some("1000"),
                Some(&sig.to_uppercase()),
                30_000,
                "PUT",
                "/v1/bots/x",
                b"{}"
            ),
            Ok(())
        );
        assert_eq!(
            verify_request(&secret, Some("1000"), Some(&sig), 1000, "PUT", "/v1/bots/x", b"{ }"),
            Err(SigError::Mismatch)
        );
        assert_eq!(
            verify_request(&secret, Some("1000"), Some(&sig), 1000, "PUT", "/v1/bots/y", b"{}"),
            Err(SigError::Mismatch)
        );
        assert_eq!(
            verify_request(&secret, Some("1000"), Some(&sig), 70_000, "PUT", "/v1/bots/x", b"{}"),
            Err(SigError::Skew)
        );
        assert_eq!(
            verify_request(&secret, None, Some(&sig), 1000, "PUT", "/", b""),
            Err(SigError::Missing)
        );
        assert_eq!(
            verify_request(&[8u8; 32], Some("1000"), Some(&sig), 1000, "PUT", "/v1/bots/x", b"{}"),
            Err(SigError::Mismatch)
        );
    }

    #[test]
    fn aes_gcm_roundtrip_and_layout() {
        let key = [1u8; 32];
        let stored = encrypt_with_nonce(&key, [2u8; 12], b"secret-value");
        let raw = b64_decode(&stored).unwrap();
        assert_eq!(raw.len(), 12 + "secret-value".len() + 16);
        assert_eq!(&raw[..12], &[2u8; 12]);
        assert_eq!(decrypt(&key, &stored).as_deref(), Some("secret-value"));
        assert_eq!(decrypt(&[9u8; 32], &stored), None);
        let mut tampered = raw.clone();
        tampered[14] ^= 1;
        assert_eq!(decrypt(&key, &b64(&tampered)), None);
        let a = encrypt(&key, "x");
        let b = encrypt(&key, "x");
        assert_ne!(a, b);
        assert_eq!(decrypt(&key, &a).as_deref(), Some("x"));
    }

    #[test]
    fn key_decoding() {
        assert!(decode_key32(&b64(&[0u8; 32])).is_some());
        assert!(decode_key32(&b64(&[0u8; 16])).is_none());
        assert!(decode_key32("not base64!").is_none());
    }

    #[test]
    fn password_hash_format_and_verify() {
        let stored = block_on(hash_password("hunter22")).unwrap();
        assert!(stored.starts_with("pbkdf2$100000$"));
        let parsed = parse_password_hash(&stored).unwrap();
        assert_eq!(parsed.salt.len(), 16);
        assert_eq!(parsed.hash.len(), 32);
        assert!(block_on(verify_password("hunter22", &stored)).unwrap());
        assert!(!block_on(verify_password("hunter23", &stored)).unwrap());
        assert!(!block_on(verify_password("x", "garbage")).unwrap());
    }

    #[test]
    fn pbkdf2_known_vector() {
        let out = block_on(pbkdf2_sha256("password", b"salt", 1)).unwrap();
        assert_eq!(
            hex::encode(out),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
    }
}
