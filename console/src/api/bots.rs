//! Bots: CRUD, lifecycle and logs.

use super::{json_body, Reply};
use crate::app::{ok_json, App, BOT_COLS, PLATFORM_COLS, PLUGIN_COLS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::models::{bot_view, BotRow, PlatformRow, PluginRow};
use crate::node::NodeError;
use crate::settings::BotSettings;
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Method, Request};

pub const MAX_AVATAR_BYTES: usize = 256 * 1024;

pub fn check_bot_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 32 {
        return Err(ApiError::bad_request("名称长度应为 1 到 32 个字符"));
    }
    Ok(n.to_string())
}

pub fn check_avatar(avatar: &str) -> ApiResult<()> {
    if avatar.len() > MAX_AVATAR_BYTES {
        return Err(ApiError::bad_request("头像图片过大，请小于 256KB"));
    }
    if !(avatar.starts_with("data:image/")
        || avatar.starts_with("https://")
        || (avatar.starts_with('/') && !avatar.starts_with("//")))
    {
        return Err(ApiError::bad_request("头像必须是图片 data URL 或 https 链接"));
    }
    Ok(())
}

pub async fn list(req: &Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    Ok(Reply::json(json!({"items": bots_for_user(app, &user.id).await?})))
}

/// Bot views of every bot of a user, with masked platform secrets.
pub async fn bots_for_user(app: &App, user_id: &str) -> ApiResult<Vec<Value>> {
    let bots: Vec<BotRow> = app
        .db
        .all(
            &format!("SELECT {BOT_COLS} FROM bots WHERE user_id = ?1 ORDER BY created_at"),
            args![user_id],
        )
        .await?;
    let cols = |c: &str| c.split(", ").map(|x| format!("t.{x}")).collect::<Vec<_>>().join(", ");
    let platforms: Vec<PlatformRow> = app
        .db
        .all(
            &format!(
                "SELECT {} FROM platforms t JOIN bots b ON b.id = t.bot_id WHERE b.user_id = ?1 ORDER BY t.created_at",
                cols(PLATFORM_COLS)
            ),
            args![user_id],
        )
        .await?;
    let plugins: Vec<PluginRow> = app
        .db
        .all(
            &format!(
                "SELECT {} FROM plugins t JOIN bots b ON b.id = t.bot_id WHERE b.user_id = ?1 ORDER BY t.created_at",
                cols(PLUGIN_COLS)
            ),
            args![user_id],
        )
        .await?;
    let mut items = Vec::new();
    for bot in &bots {
        let views = platforms
            .iter()
            .filter(|p| p.bot_id == bot.id)
            .map(|p| app.platform_view(p))
            .collect::<ApiResult<Vec<_>>>()?;
        let plugs: Vec<PluginRow> = plugins.iter().filter(|p| p.bot_id == bot.id).cloned().collect();
        items.push(bot_view(bot, views, &plugs));
    }
    Ok(items)
}

#[derive(Deserialize)]
struct CreateReq {
    name: String,
    key_id: Option<String>,
    model: Option<String>,
}

pub async fn create(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    App::require_service(&user)?;
    let body: CreateReq = json_body(req).await?;
    let name = check_bot_name(&body.name)?;
    let count = app
        .db
        .count("SELECT COUNT(*) AS n FROM bots WHERE user_id = ?1", args![user.id])
        .await?;
    if user.quota_bots >= 0 && count >= user.quota_bots {
        return Err(ApiError::quota(format!("最多可以创建 {} 只小鱼", user.quota_bots)));
    }
    let key_id = body
        .key_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::bad_request("请选择模型 Key"))?;
    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| ApiError::bad_request("请选择模型"))?;
    let settings = BotSettings::for_new_bot(&name, Some((key_id, model)));
    settings.validate().map_err(ApiError::bad_request)?;
    let node_id = app.assign_node().await?;
    let id = new_id();
    let now = now_ms();
    let inserted = app.db
        .run(
            "INSERT INTO bots (id, user_id, name, avatar, node_id, status, status_detail, desired_running, needs_push, created_at, updated_at, settings_json) \
             SELECT ?1, ?2, ?3, NULL, ?4, 'starting', '', 1, 1, ?5, ?5, ?6 \
             WHERE EXISTS (SELECT 1 FROM llm_keys WHERE id = ?7 AND user_id = ?2)",
            args![id, user.id, name, node_id, now, serde_json::to_string(&settings)?, key_id],
        )
        .await?;
    if inserted == 0 {
        return Err(ApiError::not_found("所选的模型 Key 不存在"));
    }
    app.push_bot(&id).await?;
    Ok(Reply::json(app.bot_json(&id).await?))
}

pub async fn get(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    if let Err(e) = app.refresh_status(&bot).await {
        crate::util::log_error(&format!("refresh status of {id}: {e}"));
    }
    Ok(Reply::json(app.bot_json(id).await?))
}

#[derive(Deserialize)]
struct PatchReq {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    avatar: Option<Option<String>>,
    settings: Option<Value>,
}

/// Distinguishes an absent field (None) from an explicit null (Some(None)).
fn double_option<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::deserialize(d)?))
}

pub async fn patch(req: &mut Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    let body: PatchReq = json_body(req).await?;
    let name = match &body.name {
        Some(n) => check_bot_name(n)?,
        None => bot.name.clone(),
    };
    let avatar = match body.avatar {
        Some(Some(a)) => {
            check_avatar(&a)?;
            Some(a)
        }
        Some(None) => None,
        None => bot.avatar.clone(),
    };
    let mut settings = BotSettings::from_stored(&bot.settings_json);
    if let Some(patch) = &body.settings {
        settings = settings.apply_patch(patch).map_err(ApiError::bad_request)?;
        if let Some(kid) = &settings.llm.key_id {
            let owned = app
                .db
                .count(
                    "SELECT COUNT(*) AS n FROM llm_keys WHERE id = ?1 AND user_id = ?2",
                    args![kid, user.id],
                )
                .await?;
            if owned == 0 {
                return Err(ApiError::bad_request("所选的 API Key 不存在"));
            }
        }
    }
    app.db
        .run(
            "UPDATE bots SET name = ?2, avatar = ?3, settings_json = ?4, updated_at = ?5 WHERE id = ?1",
            args![bot.id, name, avatar, serde_json::to_string(&settings)?, now_ms()],
        )
        .await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(app.bot_json(&bot.id).await?))
}

pub async fn lifecycle(req: &Request, app: &App, id: &str, action: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    if action != "stop" {
        App::require_service(&user)?;
    }
    let (running, status) = if action == "stop" {
        (0i64, "stopped")
    } else {
        (1i64, "starting")
    };
    app.db
        .run(
            "UPDATE bots SET desired_running = ?2, status = ?3, status_detail = '', updated_at = ?4 WHERE id = ?1",
            args![bot.id, running, status, now_ms()],
        )
        .await?;
    let pushed = app.push_bot(&bot.id).await?;
    if action == "restart" && pushed {
        let client = app.node_client(&bot.node_id).await?;
        if let Err(e) = client
            .call(Method::Post, &format!("/v1/bots/{}/restart", bot.id), None, 8_000)
            .await
        {
            app.db
                .run(
                    "UPDATE bots SET status = 'error', status_detail = ?2 WHERE id = ?1",
                    args![bot.id, e.detail()],
                )
                .await?;
        }
    }
    Ok(Reply::json(app.bot_json(&bot.id).await?))
}

pub async fn delete(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    app.db.run("DELETE FROM bots WHERE id = ?1", args![bot.id]).await?;
    app.delete_on_node(&bot.id, &bot.node_id).await?;
    Ok(Reply::json(ok_json()))
}

pub async fn logs(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    proxy_logs(req, app, &bot).await
}

/// Proxies `GET /v1/bots/:id/logs` for `bot`, passing the `after` cursor of `req`.
pub async fn proxy_logs(req: &Request, app: &App, bot: &BotRow) -> ApiResult<Reply> {
    let url = req.url()?;
    let after = url
        .query_pairs()
        .find(|(k, _)| k == "after")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let q: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("after", &after)
        .finish();
    let client = app.node_client(&bot.node_id).await?;
    match client
        .call(Method::Get, &format!("/v1/bots/{}/logs?{q}", bot.id), None, 8_000)
        .await
    {
        Ok(v) => Ok(Reply::json(json!({
            "items": v.get("items").cloned().unwrap_or_else(|| json!([])),
            "cursor": v.get("cursor").cloned().unwrap_or_else(|| json!(after)),
        }))),
        Err(e) if e.is_not_found() => Ok(Reply::json(json!({"items": [], "cursor": after}))),
        Err(NodeError::Unreachable(_)) => Err(ApiError::node_unreachable()),
        Err(e) => Err(ApiError::new(502, "node_error", e.detail())),
    }
}

/// Lists the bot's conversation sessions stored on its node.
pub async fn conversations(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    let client = app.node_client(&bot.node_id).await?;
    match client
        .call(Method::Get, &format!("/v1/bots/{}/conversations", bot.id), None, 8_000)
        .await
    {
        Ok(v) => Ok(Reply::json(json!({
            "items": v.get("items").cloned().unwrap_or_else(|| json!([])),
        }))),
        Err(e) if e.is_not_found() => Ok(Reply::json(json!({"items": []}))),
        Err(NodeError::Unreachable(_)) => Err(ApiError::node_unreachable()),
        Err(e) => Err(ApiError::new(502, "node_error", e.detail())),
    }
}

/// Deletes the conversation session named by the `session` query parameter, or every session without it.
pub async fn clear_conversations(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, id).await?;
    let session = req
        .url()?
        .query_pairs()
        .find(|(k, _)| k == "session")
        .map(|(_, v)| v.into_owned())
        .filter(|s| !s.is_empty());
    let mut path = format!("/v1/bots/{}/conversations", bot.id);
    if let Some(s) = session {
        let q: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("session", &s)
            .finish();
        path = format!("{path}?{q}");
    }
    let client = app.node_client(&bot.node_id).await?;
    match client.call(Method::Delete, &path, None, 8_000).await {
        Ok(_) => Ok(Reply::json(ok_json())),
        Err(e) if e.is_not_found() => Ok(Reply::json(ok_json())),
        Err(NodeError::Unreachable(_)) => Err(ApiError::node_unreachable()),
        Err(e) => Err(ApiError::new(502, "node_error", e.detail())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_avatars() {
        assert_eq!(check_bot_name("  小鱼 ").unwrap(), "小鱼");
        assert!(check_bot_name(" ").is_err());
        assert!(check_bot_name(&"鱼".repeat(33)).is_err());
        assert!(check_avatar("data:image/png;base64,AAAA").is_ok());
        assert!(check_avatar("https://x/y.png").is_ok());
        assert!(check_avatar("javascript:alert(1)").is_err());
        assert!(check_avatar("/avatars/fish.png").is_ok());
        assert!(check_avatar("//evil.example/x.png").is_err());
        assert!(check_avatar(&format!("data:image/png;base64,{}", "A".repeat(MAX_AVATAR_BYTES))).is_err());
    }
}
