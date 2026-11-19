//! LLM API keys.

use super::{json_body, Reply};
use crate::app::{ok_json, App, KEY_COLS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::http;
use crate::models::LlmKeyRow;
use crate::settings::BotSettings;
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Method, Request};

const MAX_KEYS: i64 = 20;

/// Normalizes a base URL: https (http only in dev), no query or fragment, no trailing slash.
pub fn normalize_base_url(input: &str, allow_http: bool) -> Option<String> {
    let u = url::Url::parse(input.trim()).ok()?;
    let scheme_ok = u.scheme() == "https" || (allow_http && u.scheme() == "http");
    if !scheme_ok || u.host_str().is_none() || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() {
        return None;
    }
    Some(u.as_str().trim_end_matches('/').to_string())
}

/// Picks the most useful error text out of an upstream error response.
pub fn upstream_message(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let msg = parsed.as_ref().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let text = msg.unwrap_or_else(|| body.chars().take(200).collect());
    if text.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}：{}", text.trim())
    }
}

pub async fn list(req: &Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let rows: Vec<LlmKeyRow> = app
        .db
        .all(&format!("SELECT {KEY_COLS} FROM llm_keys WHERE user_id = ?1 ORDER BY created_at"), args![user.id])
        .await?;
    let dec = app.decryptor()?;
    let items: Vec<Value> = rows.iter().map(|r| r.view(dec(&r.api_key_enc).as_deref())).collect();
    Ok(Reply::json(json!({"items": items})))
}

#[derive(Deserialize)]
struct CreateReq {
    name: String,
    base_url: String,
    model: String,
    api_key: String,
}

async fn validate_upstream(base_url: &str, api_key: &str) -> ApiResult<()> {
    let auth = format!("Bearer {api_key}");
    let headers = [("authorization", auth.as_str()), ("accept", "application/json"), ("user-agent", "fish-console")];
    let invalid = |m: String| ApiError::new(400, "llm_key_invalid", format!("API Key 校验失败：{m}"));
    let resp = http::send(Method::Get, &format!("{base_url}/models"), &headers, None, 8_000)
        .await
        .map_err(|e| invalid(if e == "timeout" { "请求超时（8 秒）".into() } else { format!("无法连接到服务：{e}") }))?;
    if !resp.ok() {
        return Err(invalid(upstream_message(resp.status, &resp.text())));
    }
    Ok(())
}

pub async fn create(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let body: CreateReq = json_body(req).await?;
    let name = body.name.trim();
    let model = body.model.trim();
    let api_key = body.api_key.trim();
    if name.is_empty() || name.chars().count() > 50 {
        return Err(ApiError::bad_request("名称长度应为 1 到 50 个字符"));
    }
    if model.is_empty() || model.chars().count() > 100 {
        return Err(ApiError::bad_request("模型名称长度应为 1 到 100 个字符"));
    }
    if api_key.is_empty() || api_key.len() > 500 {
        return Err(ApiError::bad_request("API Key 长度无效"));
    }
    let base_url = normalize_base_url(&body.base_url, app.dev).ok_or_else(|| ApiError::bad_request("接口地址必须是有效的 https 地址"))?;
    if app.db.count("SELECT COUNT(*) AS n FROM llm_keys WHERE user_id = ?1", args![user.id]).await? >= MAX_KEYS {
        return Err(ApiError::quota(format!("最多保存 {MAX_KEYS} 个 API Key")));
    }
    validate_upstream(&base_url, api_key).await?;
    let id = new_id();
    app.db
        .run(
            "INSERT INTO llm_keys (id, user_id, name, base_url, model, api_key_enc, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            args![id, user.id, name, base_url, model, app.encrypt(api_key)?, now_ms()],
        )
        .await?;
    let row: LlmKeyRow = app
        .db
        .first(&format!("SELECT {KEY_COLS} FROM llm_keys WHERE id = ?1"), args![id])
        .await?
        .ok_or_else(|| ApiError::internal("inserted key missing"))?;
    Ok(Reply::json(row.view(Some(api_key))))
}

pub async fn delete(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let n = app.db.run("DELETE FROM llm_keys WHERE id = ?1 AND user_id = ?2", args![id, user.id]).await?;
    if n == 0 {
        return Err(ApiError::not_found("API Key 不存在"));
    }
    #[derive(Deserialize)]
    struct B {
        id: String,
        settings_json: String,
    }
    let bots: Vec<B> = app.db.all("SELECT id, settings_json FROM bots WHERE user_id = ?1", args![user.id]).await?;
    for b in bots {
        if BotSettings::from_stored(&b.settings_json).llm.key_id.as_deref() == Some(id) {
            app.push_bot(&b.id).await?;
        }
    }
    Ok(Reply::json(ok_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        assert_eq!(normalize_base_url("https://api.deepseek.com/v1/", false).as_deref(), Some("https://api.deepseek.com/v1"));
        assert_eq!(normalize_base_url(" https://open.bigmodel.cn/api/paas/v4 ", false).as_deref(), Some("https://open.bigmodel.cn/api/paas/v4"));
        assert_eq!(normalize_base_url("https://api.openai.com", false).as_deref(), Some("https://api.openai.com"));
        assert!(normalize_base_url("http://x.com/v1", false).is_none());
        assert_eq!(normalize_base_url("http://127.0.0.1:8701/v1", true).as_deref(), Some("http://127.0.0.1:8701/v1"));
        assert!(normalize_base_url("https://x.com/v1?a=1", false).is_none());
        assert!(normalize_base_url("https://u:p@x.com/v1", false).is_none());
        assert!(normalize_base_url("ftp://x.com", true).is_none());
        assert!(normalize_base_url("nonsense", true).is_none());
    }

    #[test]
    fn upstream_messages() {
        assert_eq!(upstream_message(401, r#"{"error":{"message":"Invalid key"}}"#), "HTTP 401：Invalid key");
        assert_eq!(upstream_message(403, r#"{"message":"denied"}"#), "HTTP 403：denied");
        assert_eq!(upstream_message(400, r#"{"error":"bad"}"#), "HTTP 400：bad");
        assert_eq!(upstream_message(500, ""), "HTTP 500");
        assert_eq!(upstream_message(502, "gateway"), "HTTP 502：gateway");
    }
}
