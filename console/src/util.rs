//! Time, randomness, IDs and logging that work both in the Worker and in native tests.

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues)]
        pub fn get_random_values(buf: &mut [u8]);
        #[wasm_bindgen(js_namespace = Date, js_name = now)]
        pub fn date_now() -> f64;
        #[wasm_bindgen(js_namespace = console, js_name = log)]
        pub fn console_log(s: &str);
        #[wasm_bindgen(js_namespace = console, js_name = error)]
        pub fn console_error(s: &str);
    }
}

/// Current unix time in milliseconds.
pub fn now_ms() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js::date_now() as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Fills `buf` from the platform CSPRNG.
pub fn fill_random(buf: &mut [u8]) {
    #[cfg(target_arch = "wasm32")]
    {
        // getRandomValues accepts at most 65536 bytes per call.
        for chunk in buf.chunks_mut(65536) {
            js::get_random_values(chunk);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        getrandom::getrandom(buf).expect("os rng");
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    fill_random(&mut b);
    b
}

pub fn log(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    js::console_log(msg);
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{msg}");
}

pub fn log_error(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    js::console_error(msg);
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{msg}");
}

const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Lowercase ULID built from a millisecond timestamp and 80 random bits.
pub fn ulid_from(ts_ms: i64, rand: [u8; 10]) -> String {
    let mut value: u128 = (ts_ms as u128 & 0xFFFF_FFFF_FFFF) << 80;
    for (i, b) in rand.iter().enumerate() {
        value |= (*b as u128) << (8 * (9 - i));
    }
    let mut out = [0u8; 26];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (25 - i);
        *slot = CROCKFORD[((value >> shift) & 0x1F) as usize];
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

pub fn new_id() -> String {
    ulid_from(now_ms(), random_bytes::<10>())
}

/// Masks a secret as first 3 chars, an ellipsis and the last 4 chars.
pub fn mask_secret(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return "…".to_string();
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Basic email shape check: one `@`, non empty local part, dotted domain, no spaces.
pub fn valid_email(email: &str) -> bool {
    if email.len() > 254 || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else { return false };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_shape_and_order() {
        let a = ulid_from(1_700_000_000_000, [0; 10]);
        let b = ulid_from(1_700_000_000_001, [0; 10]);
        assert_eq!(a.len(), 26);
        assert!(a < b);
        assert!(a.chars().all(|c| CROCKFORD.contains(&(c as u8))));
        assert_eq!(ulid_from(0, [0; 10]), "0".repeat(26));
        assert_eq!(ulid_from(0, [0xff; 10]), format!("{}{}", "0".repeat(10), "z".repeat(16)));
    }

    #[test]
    fn masks() {
        assert_eq!(mask_secret("sk-abcdefghijka1b2"), "sk-…a1b2");
        assert!(!mask_secret("short").contains("short"));
    }

    #[test]
    fn emails() {
        assert!(valid_email("a@b.co"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("a b@c.de"));
        assert!(!valid_email("@c.de"));
        assert!(!valid_email("a@@c.de"));
        assert_eq!(normalize_email(" A@B.Co "), "a@b.co");
    }
}
