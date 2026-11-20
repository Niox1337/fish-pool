//! gengwiki AI gateway: an OpenAI compatible DeepSeek endpoint with prepaid keys.
//! Each user gets one gateway key. Invites and admins add credits to it, each with its own expiry,
//! and the gateway bills the credit that expires first. The service token from `GENGWIKI_GATEWAY_TOKEN`
//! manages keys and credits, a key reads its own balance.

use crate::api::keys::upstream_message;
use crate::app::App;
use crate::args;
use crate::error::ApiResult;
use crate::http;
use crate::settings::BotSettings;
use crate::util::{log_error, new_id, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::Method;

pub const DEFAULT_BASE: &str = "https://gateway.gengwiki.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
pub const KEY_NAME: &str = "活动获取";
/// Largest single credit, in fen.
pub const MAX_CREDIT_FEN: i64 = 1_000 * 100;
const NAME_CHARS: usize = 60;
const NOTE_CHARS: usize = 200;
const TIMEOUT_MS: u64 = 10_000;
/// Pending credits older than this are retried for at most this long.
const RETRY_WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Gengwiki {
    /// Origin without a trailing slash, the OpenAI base URL is this plus `/v1`.
    pub base: String,
    pub token: Option<String>,
    pub model: String,
}

/// A key the service created, reissued or rotated, with its plaintext.
#[derive(Debug, Clone)]
pub struct IssuedKey {
    pub key: String,
    pub id: String,
}

/// A credit as the gateway reports it, money in nano-yuan.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RemoteCredit {
    pub id: String,
    #[serde(default)]
    pub external_ref: Option<String>,
    pub amount_nano: i64,
    pub spent_nano: i64,
    pub remaining_nano: i64,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub expired: bool,
}

/// One usable credit in a key holder's balance, yuan strings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BalanceCredit {
    pub available: String,
    pub amount: String,
    pub spent: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Balance {
    pub currency: String,
    pub available: String,
    pub budget: String,
    pub spent: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub credits: Vec<BalanceCredit>,
}

const UNREADABLE: &str = "活动额度服务返回的内容无法识别";

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

    /// Calls a `/manage` route with the service token and returns the JSON reply.
    async fn manage(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let token = self.token.as_deref().ok_or("GENGWIKI_GATEWAY_TOKEN 未配置")?;
        let auth = format!("Bearer {token}");
        let text = body.map(|b| b.to_string());
        let mut headers = vec![("authorization", auth.as_str()), ("accept", "application/json")];
        if text.is_some() {
            headers.push(("content-type", "application/json"));
        }
        let resp = http::send(method, &format!("{}/manage{path}", self.base), &headers, text.as_deref().map(str::as_bytes), TIMEOUT_MS).await?;
        if !resp.ok() {
            return Err(upstream_message(resp.status, &resp.text()));
        }
        resp.json().ok_or_else(|| UNREADABLE.into())
    }

    /// Creates the key of `external_ref` with an empty base budget. A repeated `external_ref` reissues that key with a new secret.
    pub async fn issue_key(&self, name: &str, external_ref: &str) -> Result<IssuedKey, String> {
        let v = self.manage(Method::Post, "/keys", Some(json!({"name": key_label(name), "budget": "0", "external_ref": external_ref}))).await?;
        parse_issued(&v).ok_or_else(|| UNREADABLE.into())
    }

    /// Replaces the secret of a key.
    pub async fn rotate(&self, key_id: &str) -> Result<IssuedKey, String> {
        let v = self.manage(Method::Post, &format!("/keys/{key_id}/rotate"), None).await?;
        parse_issued(&v).ok_or_else(|| UNREADABLE.into())
    }

    /// Adds a credit. A repeated `external_ref` returns the credit already there.
    pub async fn add_credit(&self, key_id: &str, fen: i64, expires_at: Option<i64>, external_ref: &str, note: &str) -> Result<RemoteCredit, String> {
        let body = json!({"amount": yuan(fen), "expires_at": expires_at.map(rfc3339), "external_ref": external_ref, "note": note_label(note)});
        let v = self.manage(Method::Post, &format!("/keys/{key_id}/credits"), Some(body)).await?;
        serde_json::from_value(v["credit"].clone()).map_err(|_| UNREADABLE.into())
    }

    /// Every credit of a key, expired ones included.
    pub async fn credits(&self, key_id: &str) -> Result<Vec<RemoteCredit>, String> {
        let v = self.manage(Method::Get, &format!("/keys/{key_id}/credits"), None).await?;
        serde_json::from_value(v["items"].clone()).map_err(|_| UNREADABLE.into())
    }

    /// Changes the total amount or the expiry of a credit. `expires_at` of `Some(None)` removes the expiry.
    pub async fn patch_credit(&self, key_id: &str, credit_id: &str, fen: Option<i64>, expires_at: Option<Option<i64>>) -> Result<RemoteCredit, String> {
        let mut body = serde_json::Map::new();
        if let Some(f) = fen {
            body.insert("amount".into(), json!(yuan(f)));
        }
        if let Some(e) = expires_at {
            body.insert("expires_at".into(), json!(e.map(rfc3339)));
        }
        let v = self.manage(Method::Patch, &format!("/keys/{key_id}/credits/{credit_id}"), Some(Value::Object(body))).await?;
        serde_json::from_value(v["credit"].clone()).map_err(|_| UNREADABLE.into())
    }

    /// The balance a key reports for itself.
    pub async fn balance(&self, api_key: &str) -> Result<Balance, String> {
        let auth = format!("Bearer {api_key}");
        let headers = [("authorization", auth.as_str()), ("accept", "application/json")];
        let resp = http::send(Method::Get, &format!("{}/balance", self.api_base()), &headers, None, TIMEOUT_MS).await?;
        if !resp.ok() {
            return Err(upstream_message(resp.status, &resp.text()));
        }
        resp.json().and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| UNREADABLE.into())
    }
}

fn parse_issued(v: &Value) -> Option<IssuedKey> {
    let key = v.get("key")?.as_str()?.to_string();
    let id = v.pointer("/row/id")?.as_str()?.to_string();
    (!key.is_empty() && !id.is_empty()).then_some(IssuedKey { key, id })
}

/// Key name on the gateway side, cut to its 60 character limit.
pub fn key_label(name: &str) -> String {
    name.trim().chars().take(NAME_CHARS).collect()
}

fn note_label(note: &str) -> String {
    note.trim().chars().take(NOTE_CHARS).collect()
}

/// Gateway reference of a user's key.
pub fn key_ref(user_id: &str) -> String {
    format!("user:{user_id}")
}

/// Gateway side name of the key of `email`.
pub fn key_name(email: &str) -> String {
    format!("大肥鱼 {email}")
}

/// Fen as yuan with two decimals.
pub fn yuan(fen: i64) -> String {
    format!("{}.{:02}", fen / 100, fen % 100)
}

/// Nano-yuan as yuan with up to four decimals, rounded down.
pub fn yuan_nano(nano: i64) -> String {
    let tenth_li = nano / 100_000;
    format!("{}{}.{:04}", if nano < 0 { "-" } else { "" }, (tenth_li / 10_000).abs(), (tenth_li % 10_000).abs())
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

/// Milliseconds since the epoch as an RFC 3339 UTC time.
pub fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01, proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

// ---------- accounts and credits ----------

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub user_id: String,
    pub remote_key_id: String,
    pub llm_key_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditRow {
    pub id: String,
    pub user_id: String,
    pub amount_fen: i64,
    pub expires_at: Option<i64>,
    pub source: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub note: String,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub remote_id: Option<String>,
    pub error: Option<String>,
}

pub const CREDIT_COLS: &str = "id, user_id, amount_fen, expires_at, source, ref, note, created_by, created_at, remote_id, error";

pub async fn account(app: &App, user_id: &str) -> ApiResult<Option<Account>> {
    app.db.first("SELECT user_id, remote_key_id, llm_key_id FROM gateway_accounts WHERE user_id = ?1", args![user_id]).await
}

/// Saves `issued` as the user's gateway key: a new `llm_keys` row, or the existing one when it is still there.
async fn store_key(app: &App, user_id: &str, issued: &IssuedKey, gw: &Gengwiki) -> ApiResult<String> {
    let existing: Option<Account> = account(app, user_id).await?;
    let enc = app.encrypt(&issued.key)?;
    if let Some(a) = existing {
        let n = app.db.run("UPDATE llm_keys SET api_key_enc = ?3 WHERE id = ?1 AND user_id = ?2", args![a.llm_key_id, user_id, enc]).await?;
        if n == 1 {
            app.db.run("UPDATE gateway_accounts SET remote_key_id = ?2 WHERE user_id = ?1", args![user_id, issued.id]).await?;
            return Ok(a.llm_key_id);
        }
    }
    let llm_key_id = new_id();
    let now = now_ms();
    app.db
        .batch(vec![
            app.db.stmt(
                "INSERT INTO llm_keys (id, user_id, name, base_url, model, api_key_enc, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                args![llm_key_id, user_id, KEY_NAME, gw.api_base(), gw.model, enc, now],
            )?,
            app.db.stmt(
                "INSERT INTO gateway_accounts (user_id, remote_key_id, llm_key_id, created_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (user_id) DO UPDATE SET remote_key_id = ?2, llm_key_id = ?3",
                args![user_id, issued.id, llm_key_id, now],
            )?,
        ])
        .await?;
    adopt_key(app, user_id, &llm_key_id, &gw.model).await?;
    Ok(llm_key_id)
}

/// The user's gateway account, creating the key on first use.
async fn ensure_account(app: &App, user_id: &str, email: &str, gw: &Gengwiki) -> Result<Account, String> {
    if let Some(a) = account(app, user_id).await.map_err(|e| e.message)? {
        return Ok(a);
    }
    let issued = gw.issue_key(&key_name(email), &key_ref(user_id)).await?;
    let llm_key_id = store_key(app, user_id, &issued, gw).await.map_err(|e| e.message)?;
    Ok(Account { user_id: user_id.into(), remote_key_id: issued.id, llm_key_id })
}

/// Gives the user's key a new secret. A deleted model key is added again.
pub async fn rotate_key(app: &App, user_id: &str) -> ApiResult<Option<String>> {
    let Some(a) = account(app, user_id).await? else { return Ok(None) };
    let gw = app.gengwiki();
    let issued = gw.rotate(&a.remote_key_id).await.map_err(|e| crate::error::ApiError::new(502, "upstream_error", format!("刷新失败：{e}")))?;
    let llm_key_id = store_key(app, user_id, &issued, &gw).await?;
    push_bots_using(app, user_id, &llm_key_id).await?;
    Ok(Some(llm_key_id))
}

async fn push_bots_using(app: &App, user_id: &str, llm_key_id: &str) -> ApiResult<()> {
    #[derive(Deserialize)]
    struct B {
        id: String,
        settings_json: String,
    }
    let bots: Vec<B> = app.db.all("SELECT id, settings_json FROM bots WHERE user_id = ?1", args![user_id]).await?;
    for b in bots {
        if BotSettings::from_stored(&b.settings_json).llm.key_id.as_deref() == Some(llm_key_id) {
            app.db.run("UPDATE bots SET needs_push = 1 WHERE id = ?1", args![b.id]).await?;
            if let Err(e) = app.push_bot(&b.id).await {
                log_error(&format!("push bot {} after key change: {}", b.id, e.message));
            }
        }
    }
    Ok(())
}

/// Points the user's bots whose LLM key is unset or deleted at `key_id`, then pushes them.
async fn adopt_key(app: &App, user_id: &str, key_id: &str, model: &str) -> ApiResult<()> {
    #[derive(Deserialize)]
    struct Bot {
        id: String,
        settings_json: String,
        has_key: i64,
    }
    let bots: Vec<Bot> = app
        .db
        .all(
            "SELECT b.id, b.settings_json, EXISTS (SELECT 1 FROM llm_keys k WHERE k.user_id = b.user_id \
             AND k.id = json_extract(b.settings_json, '$.llm.key_id')) AS has_key FROM bots b WHERE b.user_id = ?1",
            args![user_id],
        )
        .await?;
    for b in bots.into_iter().filter(|b| b.has_key == 0) {
        let mut settings = BotSettings::from_stored(&b.settings_json);
        settings.llm.key_id = Some(key_id.to_string());
        settings.llm.model = model.to_string();
        app.db
            .run(
                "UPDATE bots SET settings_json = ?2, needs_push = 1, updated_at = ?3 WHERE id = ?1",
                args![b.id, serde_json::to_string(&settings)?, now_ms()],
            )
            .await?;
        if let Err(e) = app.push_bot(&b.id).await {
            log_error(&format!("push bot {} after gateway key: {}", b.id, e.message));
        }
    }
    Ok(())
}

/// Sends the given pending credits to the gateway, creating the key of each user on first use.
/// Failures are stored on the credit. Returns the credits the gateway accepted.
async fn send_credits(app: &App, pending: Vec<CreditRow>) -> ApiResult<usize> {
    let gw = app.gengwiki();
    let mut sent = 0;
    for c in pending {
        let result = async {
            let email: Option<String> = app
                .db
                .first::<Value>("SELECT email FROM users WHERE id = ?1", args![c.user_id])
                .await
                .map_err(|e| e.message)?
                .and_then(|v| v["email"].as_str().map(str::to_string));
            let email = email.ok_or("user missing")?;
            let a = ensure_account(app, &c.user_id, &email, &gw).await?;
            gw.add_credit(&a.remote_key_id, c.amount_fen, c.expires_at, &c.reference, &c.note).await
        }
        .await;
        match result {
            Ok(remote) => {
                app.db.run("UPDATE gateway_credits SET remote_id = ?2, error = NULL WHERE id = ?1", args![c.id, remote.id]).await?;
                sent += 1;
            }
            Err(e) => {
                log_error(&format!("gateway credit {}: {e}", c.reference));
                app.db.run("UPDATE gateway_credits SET error = ?2 WHERE id = ?1", args![c.id, e.chars().take(300).collect::<String>()]).await?;
            }
        }
    }
    Ok(sent)
}

/// Sends the pending credits of one user.
pub async fn sync_user(app: &App, user_id: &str) -> ApiResult<usize> {
    let pending: Vec<CreditRow> = app
        .db
        .all(&format!("SELECT {CREDIT_COLS} FROM gateway_credits WHERE user_id = ?1 AND remote_id IS NULL ORDER BY created_at"), args![user_id])
        .await?;
    send_credits(app, pending).await
}

/// Retries pending credits created before `settled_before` and within the retry window.
pub async fn sync_pending(app: &App, settled_before: i64, limit: i64) -> ApiResult<usize> {
    let pending: Vec<CreditRow> = app
        .db
        .all(
            &format!(
                "SELECT {CREDIT_COLS} FROM gateway_credits WHERE remote_id IS NULL AND created_at <= ?1 AND created_at > ?2 \
                 ORDER BY created_at LIMIT ?3"
            ),
            args![settled_before, now_ms() - RETRY_WINDOW_MS, limit],
        )
        .await?;
    send_credits(app, pending).await
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
        assert_eq!(yuan_nano(4_983_200_000), "4.9832");
        assert_eq!(yuan_nano(16_899_999), "0.0168");
        assert_eq!(yuan_nano(0), "0.0000");
        assert_eq!(yuan_nano(-5_000_000_000), "-5.0000");
    }

    #[test]
    fn times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_795_192_517_575), "2026-11-20T16:35:17Z");
        assert_eq!(rfc3339(4_102_444_799_000), "2099-12-31T23:59:59Z");
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
    fn replies() {
        let k = parse_issued(&json!({"key": "gwk_ai_x", "row": {"id": "u1"}})).unwrap();
        assert_eq!((k.key.as_str(), k.id.as_str()), ("gwk_ai_x", "u1"));
        assert!(parse_issued(&json!({"key": "", "row": {"id": "u1"}})).is_none());
        assert!(parse_issued(&json!({"row": {"id": "u1"}})).is_none());
        assert_eq!(key_label(&"鱼".repeat(80)).chars().count(), 60);
        let c: RemoteCredit = serde_json::from_value(json!({
            "id": "c1", "external_ref": "admin:x", "note": "", "amount_nano": 5, "spent_nano": 1, "remaining_nano": 4,
            "expires_at": null, "expired": false, "created_at": "2026-11-20T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(c.remaining_nano, 4);
        let b: Balance = serde_json::from_value(json!({"currency": "CNY", "available": "1", "budget": "1", "spent": "0"})).unwrap();
        assert!(b.credits.is_empty());
    }
}
