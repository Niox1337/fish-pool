//! Platform binding: QR flows proxied to the node, manual QQ credentials, enable and delete.

use super::{json_body, Reply};
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
    }
    let existing: Option<Id> = app
        .db
        .first("SELECT id FROM platforms WHERE bot_id = ?1 AND kind = ?2 AND app_id = ?3", args![bot.id, kind, app_id])
        .await?;
    let id = match existing {
        Some(e) => {
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
    let p = upsert_platform(app, &bot, "qq_official", app_id, secret, json!({}), &format!("QQ 机器人 {app_id}")).await?;
    app.push_bot(&bot.id).await?;
    let fresh = platform_by_id(app, &bot.id, &p.id).await?;
    Ok(Reply::json(app.platform_view(&fresh)?))
}

#[derive(Deserialize)]
struct PatchReq {
    enabled: Option<bool>,
    display_name: Option<String>,
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
    let status = if enabled != 0 && p.enabled == 0 { "connecting" } else { p.status.as_str() };
    app.db
        .run(
            "UPDATE platforms SET display_name = ?2, enabled = ?3, status = ?4 WHERE id = ?1",
            args![p.id, display_name, enabled, status],
        )
        .await?;
    app.db.run("UPDATE bots SET updated_at = ?2 WHERE id = ?1", args![bot.id, now_ms()]).await?;
    app.push_bot(&bot.id).await?;
    let fresh = platform_by_id(app, &bot.id, pid).await?;
    Ok(Reply::json(app.platform_view(&fresh)?))
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
    fn qr_status_mapping() {
        assert_eq!(map_qr_status("qq", "pending"), "pending");
        assert_eq!(map_qr_status("qq", "scanned"), "pending");
        assert_eq!(map_qr_status("weixin", "scanned"), "scanned");
        assert_eq!(map_qr_status("weixin", "expired"), "expired");
        assert_eq!(map_qr_status("qq", "created"), "created");
        assert_eq!(map_qr_status("qq", "weird"), "error");
    }
}
