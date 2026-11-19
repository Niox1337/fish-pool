use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const HEADER_NODE: &str = "x-fish-node";
pub const HEADER_TS: &str = "x-fish-ts";
pub const HEADER_SIG: &str = "x-fish-sig";
pub const MAX_SKEW_MS: i64 = 60_000;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds the canonical string `{ts}\n{METHOD}\n{path_with_query}\n{hex(sha256(body))}`.
pub fn canonical(ts: i64, method: &str, path_with_query: &str, body: &[u8]) -> String {
    let body_hash = hex::encode(Sha256::digest(body));
    format!("{ts}\n{}\n{path_with_query}\n{body_hash}", method.to_ascii_uppercase())
}

/// Returns the lowercase hex HMAC-SHA256 signature for a request.
pub fn sign(key: &[u8], ts: i64, method: &str, path_with_query: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(canonical(ts, method, path_with_query, body).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    MissingHeaders,
    WrongNode,
    Expired,
    BadSignature,
}

pub struct VerifyInput<'a> {
    pub node: Option<&'a str>,
    pub ts: Option<&'a str>,
    pub sig: Option<&'a str>,
    pub method: &'a str,
    pub path_with_query: &'a str,
    pub body: &'a [u8],
}

/// Verifies a signed request against any of the candidate keys in constant time per key.
pub fn verify(keys: &[&[u8]], expected_node: &str, input: &VerifyInput<'_>, now: i64) -> Result<(), VerifyError> {
    let (Some(node), Some(ts), Some(sig)) = (input.node, input.ts, input.sig) else {
        return Err(VerifyError::MissingHeaders);
    };
    if node != expected_node {
        return Err(VerifyError::WrongNode);
    }
    let ts: i64 = ts.trim().parse().map_err(|_| VerifyError::MissingHeaders)?;
    if (now - ts).abs() > MAX_SKEW_MS {
        return Err(VerifyError::Expired);
    }
    let Ok(sig_bytes) = hex::decode(sig.trim()) else {
        return Err(VerifyError::BadSignature);
    };
    let msg = canonical(ts, input.method, input.path_with_query, input.body);
    for key in keys {
        let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
        mac.update(msg.as_bytes());
        if mac.verify_slice(&sig_bytes).is_ok() {
            return Ok(());
        }
    }
    Err(VerifyError::BadSignature)
}

/// Signing HTTP client for node to console callbacks.
#[derive(Clone)]
pub struct SignedClient {
    pub http: reqwest::Client,
    pub base_url: String,
    pub node_id: String,
    pub key: Vec<u8>,
}

impl SignedClient {
    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> anyhow::Result<reqwest::Response> {
        let bytes = serde_json::to_vec(body)?;
        let ts = now_ms();
        let sig = sign(&self.key, ts, "POST", path, &bytes);
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("content-type", "application/json")
            .header(HEADER_NODE, &self.node_id)
            .header(HEADER_TS, ts.to_string())
            .header(HEADER_SIG, sig)
            .timeout(std::time::Duration::from_secs(10))
            .body(bytes)
            .send()
            .await?;
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_format() {
        let c = canonical(1700000000000, "get", "/v1/health", b"");
        assert_eq!(
            c,
            "1700000000000\nGET\n/v1/health\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sign_known_vector() {
        let sig = sign(b"key", 1, "POST", "/v1/bots/x", b"{}");
        let mut mac = HmacSha256::new_from_slice(b"key").unwrap();
        mac.update(b"1\nPOST\n/v1/bots/x\n44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a");
        assert_eq!(sig, hex::encode(mac.finalize().into_bytes()));
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn verify_roundtrip_and_failures() {
        let key = b"0123456789abcdef0123456789abcdef";
        let now = 1_700_000_000_000;
        let sig = sign(key, now, "PUT", "/v1/bots/b1?x=1", b"{\"a\":1}");
        let ts = now.to_string();
        let mk = |node: &'static str, sig: &str, body: &'static [u8]| VerifyInput {
            node: Some(node),
            ts: Some(Box::leak(ts.clone().into_boxed_str())),
            sig: Some(Box::leak(sig.to_string().into_boxed_str())),
            method: "PUT",
            path_with_query: "/v1/bots/b1?x=1",
            body,
        };
        assert_eq!(verify(&[key], "node-1", &mk("node-1", &sig, b"{\"a\":1}"), now), Ok(()));
        assert_eq!(
            verify(&[key], "node-1", &mk("node-1", &sig, b"{\"a\":1}"), now + 59_000),
            Ok(())
        );
        assert_eq!(
            verify(&[key], "node-1", &mk("node-1", &sig, b"{\"a\":1}"), now + 61_000),
            Err(VerifyError::Expired)
        );
        assert_eq!(
            verify(&[key], "node-1", &mk("node-1", &sig, b"{\"a\":2}"), now),
            Err(VerifyError::BadSignature)
        );
        assert_eq!(
            verify(&[key], "node-1", &mk("node-2", &sig, b"{\"a\":1}"), now),
            Err(VerifyError::WrongNode)
        );
        assert_eq!(
            verify(&[b"other", key], "node-1", &mk("node-1", &sig, b"{\"a\":1}"), now),
            Ok(())
        );
        let missing = VerifyInput {
            node: None,
            ts: None,
            sig: None,
            method: "GET",
            path_with_query: "/",
            body: b"",
        };
        assert_eq!(
            verify(&[key], "node-1", &missing, now),
            Err(VerifyError::MissingHeaders)
        );
    }
}
