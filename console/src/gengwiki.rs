//! Client for the gengwiki AI gateway: an OpenAI compatible DeepSeek endpoint with prepaid keys.
//! The service token from `GENGWIKI_GATEWAY_TOKEN` creates keys, a key reads its own balance.

use crate::api::keys::upstream_message;
use crate::http;
use serde::Deserialize;
use serde_json::{json, Value};
use worker::Method;

pub const DEFAULT_BASE: &str = "https://gateway.gengwiki.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
pub const KEY_NAME: &str = "gengwiki 网关";
/// Largest gift per invite code, in fen.
pub const MAX_GIFT_FEN: i64 = 1_000 * 100;
const NAME_CHARS: usize = 60;
const TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Gengwiki {
    /// Origin without a trailing slash, the OpenAI base URL is this plus `/v1`.
    pub base: String,
    pub token: Option<String>,
    pub model: String,
}

/// A key the service created or reissued.
#[derive(Debug, Clone)]
pub struct Granted {
    pub key: String,
    pub id: String,
}

impl Gengwiki {
    pub fn new(base: Option<&str>, token: Option<&str>, model: Option<&str>) -> Self {
        let base = base.map(str::trim).filter(|b| !b.is_empty()).unwrap_or(DEFAULT_BASE).trim_end_matches('/').to_string();
        let token = token.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let model = model.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_MODEL).to_string();
        Self { base, token, model }
    }

    pub fn api_base(&self) -> String {
        format!("{}/v1", self.base)
    }

    /// True when an LLM key's base URL points at this gateway.
    pub fn serves(&self, base_url: &str) -> bool {
        base_url.trim_end_matches('/') == self.api_base()
    }

    /// Creates a key with `budget_fen`. A repeated `external_ref` reissues the same key with a new secret.
    pub async fn grant(&self, name: &str, budget_fen: i64, external_ref: &str) -> Result<Granted, String> {
        let token = self.token.as_deref().ok_or("GENGWIKI_GATEWAY_TOKEN 未配置")?;
        let body = json!({"name": key_label(name), "budget": yuan(budget_fen), "external_ref": external_ref}).to_string();
        let auth = format!("Bearer {token}");
        let headers = [("authorization", auth.as_str()), ("content-type", "application/json"), ("accept", "application/json")];
        let resp = http::send(Method::Post, &format!("{}/manage/keys", self.base), &headers, Some(body.as_bytes()), TIMEOUT_MS).await?;
        if !resp.ok() {
            return Err(upstream_message(resp.status, &resp.text()));
        }
        parse_granted(&resp.json().unwrap_or(Value::Null)).ok_or_else(|| "gengwiki 网关返回的内容无法识别".into())
    }

    /// The balance a key reports for itself.
    pub async fn balance(&self, api_key: &str) -> Result<Balance, String> {
        let auth = format!("Bearer {api_key}");
        let headers = [("authorization", auth.as_str()), ("accept", "application/json")];
        let resp = http::send(Method::Get, &format!("{}/balance", self.api_base()), &headers, None, TIMEOUT_MS).await?;
        if !resp.ok() {
            return Err(upstream_message(resp.status, &resp.text()));
        }
        resp.json().and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| "gengwiki 网关返回的内容无法识别".into())
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
pub struct Balance {
    pub currency: String,
    pub available: String,
    pub budget: String,
    pub spent: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

fn parse_granted(v: &Value) -> Option<Granted> {
    let key = v.get("key")?.as_str()?.to_string();
    let id = v.pointer("/row/id")?.as_str()?.to_string();
    (!key.is_empty() && !id.is_empty()).then_some(Granted { key, id })
}

/// Key name on the gateway side, cut to its 60 character limit.
pub fn key_label(name: &str) -> String {
    name.trim().chars().take(NAME_CHARS).collect()
}

/// Fen as yuan with two decimals.
pub fn yuan(fen: i64) -> String {
    format!("{}.{:02}", fen / 100, fen % 100)
}

/// Yuan with at most two decimals as fen, None for anything else or a negative amount.
pub fn parse_fen(s: &str) -> Option<i64> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() || whole.len() > 9 || frac.len() > 2 || (s.contains('.') && frac.is_empty()) || !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac = format!("{frac:0<2}");
    Some(whole.parse::<i64>().ok()? * 100 + frac.parse::<i64>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_fen("5"), Some(500));
        assert_eq!(parse_fen("5.5"), Some(550));
        assert_eq!(parse_fen(" 0.01 "), Some(1));
        assert_eq!(parse_fen("12.34"), Some(1234));
        for bad in ["", ".5", "5.", "-1", "1.234", "1e3", "abc", "1,5"] {
            assert_eq!(parse_fen(bad), None, "{bad}");
        }
        assert_eq!(yuan(500), "5.00");
        assert_eq!(yuan(1234), "12.34");
        assert_eq!(yuan(7), "0.07");
    }

    #[test]
    fn base_and_matching() {
        let g = Gengwiki::new(None, Some(" t "), None);
        assert_eq!(g.api_base(), "https://gateway.gengwiki.com/v1");
        assert_eq!(g.token.as_deref(), Some("t"));
        assert_eq!(g.model, DEFAULT_MODEL);
        assert!(g.serves("https://gateway.gengwiki.com/v1/"));
        assert!(!g.serves("https://api.deepseek.com/v1"));
        let local = Gengwiki::new(Some("http://127.0.0.1:9/"), Some(""), Some("m"));
        assert_eq!(local.api_base(), "http://127.0.0.1:9/v1");
        assert!(local.token.is_none());
    }

    #[test]
    fn granted_replies() {
        let g = parse_granted(&json!({"key": "gwk_ai_x", "row": {"id": "u1"}})).unwrap();
        assert_eq!((g.key.as_str(), g.id.as_str()), ("gwk_ai_x", "u1"));
        assert!(parse_granted(&json!({"key": "", "row": {"id": "u1"}})).is_none());
        assert!(parse_granted(&json!({"row": {"id": "u1"}})).is_none());
        assert_eq!(key_label(&"鱼".repeat(80)).chars().count(), 60);
    }
}
