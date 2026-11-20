//! Platform binding: QR flows proxied to the node, manual QQ credentials, enable and delete.

use super::{json_body, read_body, Reply};
use crate::app::{ok_json, App, PLATFORM_COLS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::models::{BotRow, PlatformRow};
use crate::node::NodeError;
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Method, Request};

const MAX_PLATFORMS_PER_BOT: i64 = 6;

fn node_err(e: NodeError) -> ApiError {
    match e {
        NodeError::Unreachable(_) => ApiError::node_unreachable(),
        other => ApiError::new(502, "node_error", other.detail()),
    }
}

/// Keeps the stored `mode` when a refresh of the credentials does not set one.
fn merge_extra(stored: &str, mut fresh: Value) -> Value {
    let old: Value = serde_json::from_str(stored).unwrap_or(Value::Null);
    if let (Some(mode), Some(obj)) = (old.get("mode"), fresh.as_object_mut()) {
        obj.entry("mode").or_insert_with(|| mode.clone());
    }
    fresh
}

fn parse_mode(mode: Option<&str>) -> ApiResult<Option<&'static str>> {
    match mode {
        None => Ok(None),
        Some("websocket") => Ok(Some("websocket")),
        Some("webhook") => Ok(Some("webhook")),
        Some(_) => Err(ApiError::bad_request("连接方式只能是 websocket 或 webhook")),
    }
}

fn platform_kind(kind: &str) -> &'static str {
    if kind == "qq" {
        "qq_official"
    } else {
        "weixin_ilink"
    }
}

/// Maps a node QR status onto the statuses the API exposes for that flow.
pub fn map_qr_status(kind: &str, status: &str) -> &'static str {
    match status {
        "pending" | "waiting" | "wait" => "pending",
        "scanned" | "scaned" if kind == "weixin" => "scanned",
        "scanned" | "scaned" => "pending",
        "expired" => "expired",
        "created" | "confirmed" | "done" => "created",
        _ => "error",
    }
}

async fn platform_by_id(app: &App, bot_id: &str, pid: &str) -> ApiResult<PlatformRow> {
    app.db
        .first(&format!("SELECT {PLATFORM_COLS} FROM platforms WHERE id = ?1 AND bot_id = ?2"), args![pid, bot_id])
        .await?
        .ok_or_else(|| ApiError::not_found("平台不存在"))
}

pub async fn qq_panels(
    req: &mut Request, app: &App, bot_id: &str, pid: &str, method: Method, panel_id: Option<&str>,
) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let platform = platform_by_id(app, &bot.id, pid).await?;
    if platform.kind != "qq_official" {
        return Err(ApiError::not_found("QQ 接入不存在"));
    }
    if method != Method::Get {
        App::require_service(&user)?;
    }

    let mut path = format!("/v1/bots/{}/platforms/{}/qq-panels", bot.id, platform.id);
    if let Some(id) = panel_id {
        if id.is_empty() || id.len() > 256 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
            return Err(ApiError::bad_request("指令面板 ID 格式错误"));
        }
        path.push('/');
        path.push_str(id);
    }
    if method == Method::Get {
        let scope = req.url()?.query_pairs().find(|(name, _)| name == "scope").map(|(_, value)| value.into_owned());
        match scope.as_deref() {
            Some(scope @ ("c2c" | "group")) => path.push_str(&format!("?scope={scope}")),
            _ => return Err(ApiError::bad_request("请选择单聊或群聊场景")),
        }
    }
    let body = if matches!(method, Method::Post | Method::Put) { Some(json_body::<Value>(req).await?) } else { None };
    let client = app.node_client(&bot.node_id).await?;
    match client.call(method, &path, body.as_ref(), 60_000).await {
        Ok(value) => Ok(Reply::json(value)),
        Err(NodeError::Status(400, message)) => Err(ApiError::new(400, "qq_panel_invalid", message)),
        Err(NodeError::Status(409, message)) => Err(ApiError::conflict("qq_panel_changed", message)),
        Err(NodeError::Status(502, message)) => Err(ApiError::new(502, "qq_panel_upstream", message)),
        Err(e) => Err(node_err(e)),
    }
}

/// Inserts a platform or refreshes the credentials of the existing one with the same kind and app_id.
async fn upsert_platform(
    app: &App,
    bot: &BotRow,
    kind: &str,
    app_id: &str,
    secret: &str,
    extra: Value,
    display_name: &str,
) -> ApiResult<PlatformRow> {
    let enc = app.encrypt(secret)?;
    let extra_s = serde_json::to_string(&extra)?;
    #[derive(Deserialize)]
    struct Id {
        id: String,
        extra_json: String,
    }
    let existing: Option<Id> = app
        .db
        .first("SELECT id, extra_json FROM platforms WHERE bot_id = ?1 AND kind = ?2 AND app_id = ?3", args![bot.id, kind, app_id])
        .await?;
    let id = match existing {
        Some(e) => {
            let extra_s = serde_json::to_string(&merge_extra(&e.extra_json, extra.clone()))?;
            app.db
                .run(
                    "UPDATE platforms SET secret_enc = ?2, extra_json = ?3, enabled = 1, status = 'connecting', status_detail = '' WHERE id = ?1",
                    args![e.id, enc, extra_s],
                )
                .await?;
            e.id
        }
        None => {
            let n = app.db.count("SELECT COUNT(*) AS n FROM platforms WHERE bot_id = ?1", args![bot.id]).await?;
            if n >= MAX_PLATFORMS_PER_BOT {
                return Err(ApiError::quota(format!("每只小鱼最多绑定 {MAX_PLATFORMS_PER_BOT} 个平台")));
            }
            let id = new_id();
            app.db
                .run(
                    "INSERT INTO platforms (id, bot_id, kind, display_name, app_id, secret_enc, extra_json, enabled, status, status_detail, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 'connecting', '', ?8)",
                    args![id, bot.id, kind, display_name, app_id, enc, extra_s, now_ms()],
                )
                .await?;
            id
        }
    };
    app.db.run("UPDATE bots SET updated_at = ?2 WHERE id = ?1", args![bot.id, now_ms()]).await?;
    platform_by_id(app, &bot.id, &id).await
}

pub async fn qr_start(req: &Request, app: &App, bot_id: &str, kind: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let client = app.node_client(&bot.node_id).await?;
    let v = client.call(Method::Post, &format!("/v1/qr/{kind}"), Some(&json!({})), 10_000).await.map_err(node_err)?;
    let task_id = v.get("task_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| ApiError::new(502, "node_error", "节点没有返回二维码任务"))?;
    let expires_at = v.get("expires_at").and_then(Value::as_i64).unwrap_or(now_ms() + 5 * 60 * 1000);
    app.db
        .run(
            "INSERT INTO qr_tasks (task_id, kind, bot_id, node_id, platform_id, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6) \
             ON CONFLICT(kind, task_id) DO UPDATE SET bot_id = excluded.bot_id, expires_at = excluded.expires_at",
            args![task_id, kind, bot.id, bot.node_id, expires_at, now_ms()],
        )
        .await?;
    Ok(Reply::json(json!({
        "task_id": task_id,
        "qr_url": v.get("qr_url").cloned().unwrap_or(Value::Null),
        "interval_s": v.get("interval_s").cloned().unwrap_or(json!(2)),
        "expires_at": expires_at,
    })))
}

pub async fn qr_poll(req: &Request, app: &App, bot_id: &str, kind: &str, task_id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    #[derive(Deserialize)]
    struct Task {
        platform_id: Option<String>,
    }
    let task: Task = app
        .db
        .first("SELECT platform_id FROM qr_tasks WHERE kind = ?1 AND task_id = ?2 AND bot_id = ?3", args![kind, task_id, bot.id])
        .await?
        .ok_or_else(|| ApiError::not_found("二维码任务不存在"))?;
    if let Some(pid) = task.platform_id {
        if let Ok(p) = platform_by_id(app, &bot.id, &pid).await {
            return Ok(Reply::json(json!({"status": "created", "platform": app.platform_view(&p)?})));
        }
    }
    let client = app.node_client(&bot.node_id).await?;
    let v = client.call(Method::Get, &format!("/v1/qr/{kind}/{task_id}"), None, 10_000).await.map_err(node_err)?;
    let raw = v.get("status").and_then(Value::as_str).unwrap_or("error");
    let status = map_qr_status(kind, raw);
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    if status != "created" {
        let mut out = json!({"status": status});
        if let Some(m) = v.get("message").and_then(Value::as_str) {
            out["message"] = json!(m);
        }
        return Ok(Reply::json(out));
    }
    let platform = if kind == "qq" {
        let (app_id, secret) = (s("app_id"), s("secret"));
        if app_id.is_empty() || secret.is_empty() {
            return Ok(Reply::json(json!({"status": "error", "message": "扫码结果缺少机器人凭据"})));
        }
        upsert_platform(app, &bot, platform_kind(kind), &app_id, &secret, json!({}), &format!("QQ 机器人 {app_id}")).await?
    } else {
        let (wx_bot, token) = (s("bot_id"), s("token"));
        if wx_bot.is_empty() || token.is_empty() {
            return Ok(Reply::json(json!({"status": "error", "message": "扫码结果缺少机器人凭据"})));
        }
        let extra = json!({"base_url": s("base_url"), "user_id": s("user_id")});
        upsert_platform(app, &bot, platform_kind(kind), &wx_bot, &token, extra, "微信机器人").await?
    };
    app.db
        .run("UPDATE qr_tasks SET platform_id = ?3 WHERE kind = ?1 AND task_id = ?2", args![kind, task_id, platform.id])
        .await?;
    app.push_bot(&bot.id).await?;
    let fresh = platform_by_id(app, &bot.id, &platform.id).await?;
    Ok(Reply::json(json!({"status": "created", "platform": app.platform_view(&fresh)?})))
}

#[derive(Deserialize)]
struct ManualReq {
    app_id: String,
    secret: String,
    mode: Option<String>,
}

pub async fn qq_manual(req: &mut Request, app: &App, bot_id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let body: ManualReq = json_body(req).await?;
    let app_id = body.app_id.trim();
    let secret = body.secret.trim();
    if app_id.is_empty() || app_id.len() > 32 || !app_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError::bad_request("AppID 格式不正确"));
    }
    if secret.is_empty() || secret.len() > 256 || secret.chars().any(char::is_whitespace) {
        return Err(ApiError::bad_request("AppSecret 格式不正确"));
    }
    let extra = match parse_mode(body.mode.as_deref())? {
        Some(m) => json!({"mode": m}),
        None => json!({}),
    };
    let p = upsert_platform(app, &bot, "qq_official", app_id, secret, extra, &format!("QQ 机器人 {app_id}")).await?;
    app.push_bot(&bot.id).await?;
    let fresh = platform_by_id(app, &bot.id, &p.id).await?;
    Ok(Reply::json(app.platform_view(&fresh)?))
}

#[derive(Deserialize)]
struct PatchReq {
    enabled: Option<bool>,
    display_name: Option<String>,
    mode: Option<String>,
}

pub async fn patch(req: &mut Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let p = platform_by_id(app, &bot.id, pid).await?;
    let body: PatchReq = json_body(req).await?;
    let display_name = match body.display_name {
        Some(n) => {
            let n = n.trim().to_string();
            if n.is_empty() || n.chars().count() > 32 {
                return Err(ApiError::bad_request("显示名称长度应为 1 到 32 个字符"));
            }
            n
        }
        None => p.display_name.clone(),
    };
    let enabled = body.enabled.map(i64::from).unwrap_or(p.enabled);
    let mode = parse_mode(body.mode.as_deref())?;
    if mode.is_some() && p.kind != "qq_official" {
        return Err(ApiError::bad_request("只有 QQ 机器人可以切换连接方式"));
    }
    let mode_changed = mode.is_some_and(|m| Some(m) != p.qq_mode());
    let mut extra: Value = serde_json::from_str(&p.extra_json).unwrap_or_else(|_| json!({}));
    if let (Some(m), Some(obj)) = (mode, extra.as_object_mut()) {
        obj.insert("mode".into(), json!(m));
    }
    let (status, detail) = if (enabled != 0 && p.enabled == 0) || (enabled != 0 && mode_changed) {
        ("connecting", "")
    } else {
        (p.status.as_str(), p.status_detail.as_str())
    };
    app.db
        .run(
            "UPDATE platforms SET display_name = ?2, enabled = ?3, status = ?4, status_detail = ?5, extra_json = ?6 WHERE id = ?1",
            args![p.id, display_name, enabled, status, detail, serde_json::to_string(&extra)?],
        )
        .await?;
    app.db.run("UPDATE bots SET updated_at = ?2 WHERE id = ?1", args![bot.id, now_ms()]).await?;
    app.push_bot(&bot.id).await?;
    let fresh = platform_by_id(app, &bot.id, pid).await?;
    Ok(Reply::json(app.platform_view(&fresh)?))
}

const MAX_CALLBACK_BYTES: usize = 64 * 1024;

/// Public QQ callback endpoint of one platform. The node holding the bot checks the
/// Ed25519 signature and answers, this handler only routes the request there.
pub async fn qq_webhook(req: &mut Request, app: &App, pid: &str) -> ApiResult<Reply> {
    let header = |name: &str| req.headers().get(name).ok().flatten().unwrap_or_default();
    let (timestamp, signature, app_id) = (header("x-signature-timestamp"), header("x-signature-ed25519"), header("x-bot-appid"));
    let raw = read_body(req).await?;
    if raw.len() > MAX_CALLBACK_BYTES {
        return Err(ApiError::new(413, "payload_too_large", "回调内容过大"));
    }
    let body = String::from_utf8(raw).map_err(|_| ApiError::bad_request("回调内容不是 UTF-8"))?;
    let op = serde_json::from_str::<Value>(&body).ok().and_then(|v| v.get("op").and_then(Value::as_u64));
    if op.is_none() || (op != Some(13) && signature.is_empty()) {
        return Err(ApiError::new(401, "webhook_unauthorized", "回调缺少签名"));
    }
    let not_found = || ApiError::not_found("回调地址不存在");
    let p: PlatformRow = app
        .db
        .first(&format!("SELECT {PLATFORM_COLS} FROM platforms WHERE id = ?1"), args![pid])
        .await?
        .ok_or_else(not_found)?;
    if p.qq_mode() != Some("webhook") || p.enabled == 0 {
        return Err(not_found());
    }
    let bot = app.bot_by_id(&p.bot_id).await?.ok_or_else(not_found)?;
    let client = app.node_client(&bot.node_id).await?;
    let payload = json!({"timestamp": timestamp, "signature": signature, "app_id": app_id, "body": body});
    match client.call(Method::Post, &format!("/v1/bots/{}/platforms/{}/qq-webhook", bot.id, p.id), Some(&payload), 8_000).await {
        Ok(v) => Ok(Reply::json(v)),
        Err(NodeError::Status(401, d)) => Err(ApiError::new(401, "webhook_unauthorized", d)),
        Err(NodeError::Status(_, d)) => Err(ApiError::new(400, "webhook_rejected", d)),
        Err(NodeError::Unreachable(_)) => Err(ApiError::new(503, "node_unreachable", "节点暂时不可用")),
    }
}

pub async fn delete(req: &Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let n = app.db.run("DELETE FROM platforms WHERE id = ?1 AND bot_id = ?2", args![pid, bot.id]).await?;
    if n == 0 {
        return Err(ApiError::not_found("平台不存在"));
    }
    app.db.run("UPDATE bots SET updated_at = ?2 WHERE id = ?1", args![bot.id, now_ms()]).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(ok_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_survives_credential_refresh() {
        assert_eq!(merge_extra(r#"{"mode":"webhook"}"#, json!({})), json!({"mode": "webhook"}));
        assert_eq!(merge_extra(r#"{"mode":"webhook"}"#, json!({"mode": "websocket"})), json!({"mode": "websocket"}));
        assert_eq!(merge_extra("{}", json!({"bot_id": "b"})), json!({"bot_id": "b"}));
        assert_eq!(merge_extra("not json", json!({})), json!({}));
    }

    #[test]
    fn mode_values() {
        assert_eq!(parse_mode(None).unwrap(), None);
        assert_eq!(parse_mode(Some("webhook")).unwrap(), Some("webhook"));
        assert_eq!(parse_mode(Some("websocket")).unwrap(), Some("websocket"));
        assert!(parse_mode(Some("http")).is_err());
    }

    #[test]
    fn qq_mode_defaults_to_websocket() {
        let row = |kind: &str, extra: &str| PlatformRow {
            id: "p".into(),
            bot_id: "b".into(),
            kind: kind.into(),
            display_name: String::new(),
            app_id: "1".into(),
            secret_enc: String::new(),
            extra_json: extra.into(),
            enabled: 1,
            status: "connecting".into(),
            status_detail: String::new(),
            created_at: 0,
        };
        assert_eq!(row("qq_official", "{}").qq_mode(), Some("websocket"));
        assert_eq!(row("qq_official", r#"{"mode":"webhook"}"#).qq_mode(), Some("webhook"));
        assert_eq!(row("weixin_ilink", r#"{"mode":"webhook"}"#).qq_mode(), None);
        assert_eq!(row("qq_official", r#"{"mode":"webhook"}"#).view(None)["mode"], "webhook");
    }

    #[test]
    fn qr_status_mapping() {
        assert_eq!(map_qr_status("qq", "pending"), "pending");
        assert_eq!(map_qr_status("qq", "scanned"), "pending");
        assert_eq!(map_qr_status("weixin", "scanned"), "scanned");
        assert_eq!(map_qr_status("weixin", "expired"), "expired");
        assert_eq!(map_qr_status("qq", "created"), "created");
        assert_eq!(map_qr_status("qq", "weird"), "error");
    }
}
