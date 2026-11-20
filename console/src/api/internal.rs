//! Node callbacks and scheduled maintenance.

use super::{read_body, Reply};
use crate::app::{keep_expiry_detail, ok_json, valid_bot_status, valid_platform_status, valid_plugin_status, App, SERVICE_EXPIRED_DETAIL};
use crate::args;
use crate::crypto::{verify_request, SigError};
use crate::error::{ApiError, ApiResult};
use crate::node::signed_path;
use crate::util::{log, log_error, now_ms};
use serde::Deserialize;
use worker::Request;

#[derive(Deserialize)]
struct Event {
    bot_id: String,
    kind: String,
    #[serde(default)]
    id: String,
    status: String,
    #[serde(default)]
    status_detail: String,
}

#[derive(Deserialize)]
struct Events {
    node_id: String,
    #[serde(default)]
    events: Vec<Event>,
}

/// Verifies a node signed request and returns the node id.
pub async fn node_auth(req: &Request, app: &App, method: &str, body: &[u8]) -> ApiResult<String> {
    let header = |k: &str| req.headers().get(k).ok().flatten();
    let bad_sig = || ApiError::new(401, "bad_signature", "签名校验失败");
    let node_id = header("x-fish-node").ok_or_else(bad_sig)?;
    let (ts, sig) = (header("x-fish-ts"), header("x-fish-sig"));
    let path = signed_path(req.url()?.as_ref()).ok_or_else(bad_sig)?;
    app.ensure_default_node().await?;
    let known = app.db.count("SELECT COUNT(*) AS n FROM nodes WHERE id = ?1", args![node_id]).await? > 0;
    let secret = app.node_secret(&node_id).filter(|_| known).ok_or_else(bad_sig)?;
    match verify_request(&secret, ts.as_deref(), sig.as_deref(), now_ms(), method, &path, body) {
        Ok(()) => Ok(node_id),
        Err(SigError::Skew) => Err(ApiError::new(401, "bad_signature", "请求时间戳超出允许范围")),
        Err(_) => Err(bad_sig()),
    }
}

pub async fn node_events(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let bad_sig = || ApiError::new(401, "bad_signature", "签名校验失败");
    let body = read_body(req).await?;
    let node_id = node_auth(req, app, "POST", &body).await?;
    let payload: Events = serde_json::from_slice(&body).map_err(|e| ApiError::bad_request(format!("事件格式错误：{e}")))?;
    if payload.node_id != node_id {
        return Err(bad_sig());
    }

    let mut stmts = Vec::new();
    for ev in payload.events.iter().take(500) {
        let detail: String = ev.status_detail.chars().take(500).collect();
        match ev.kind.as_str() {
            "bot_status" if valid_bot_status(&ev.status) => stmts.push(app.db.stmt(
                &format!("UPDATE bots SET status = ?3, status_detail = {} WHERE id = ?1 AND node_id = ?2 AND needs_push = 0", keep_expiry_detail("?3", "?4")),
                args![ev.bot_id, node_id, ev.status, detail],
            )?),
            "platform_status" if valid_platform_status(&ev.status) => stmts.push(app.db.stmt(
                "UPDATE platforms SET status = ?3, status_detail = ?4 WHERE id = ?1 AND bot_id IN (SELECT id FROM bots WHERE id = ?2 AND node_id = ?5)",
                args![ev.id, ev.bot_id, ev.status, detail, node_id],
            )?),
            "plugin_status" if valid_plugin_status(&ev.status) => stmts.push(app.db.stmt(
                "UPDATE plugins SET status = ?3, status_detail = ?4, updated_at = ?6 WHERE id = ?1 AND bot_id IN (SELECT id FROM bots WHERE id = ?2 AND node_id = ?5)",
                args![ev.id, ev.bot_id, ev.status, detail, node_id, now_ms()],
            )?),
            _ => {}
        }
    }
    app.db.batch(stmts).await?;
    app.mark_node_seen(&node_id).await?;
    // The node is reachable again, so pending desired states can be delivered.
    app.repush_pending(Some(&node_id), 20).await?;
    Ok(Reply::json(ok_json()))
}

/// Periodic cleanup and retries, run from the cron trigger.
/// Credits younger than this are left to the request that made them.
const GRANT_SETTLE_MS: i64 = 2 * 60 * 1000;

pub async fn run_maintenance(app: &App) -> ApiResult<()> {
    let now = now_ms();
    let day = 24 * 3600 * 1000;
    app.db
        .batch(vec![
            app.db.stmt("DELETE FROM captchas WHERE expires_at < ?1", args![now])?,
            app.db.stmt("DELETE FROM email_codes WHERE expires_at < ?1", args![now])?,
            app.db.stmt("DELETE FROM sessions WHERE expires_at < ?1", args![now])?,
            app.db.stmt("DELETE FROM rate_events WHERE created_at < ?1", args![now - day])?,
            app.db.stmt("DELETE FROM qr_tasks WHERE expires_at < ?1", args![now - day])?,
        ])
        .await?;

    let stopped = stop_expired(app, now, None).await?;
    if stopped > 0 {
        log(&format!("maintenance stopped {stopped} bots of expired users"));
    }

    #[derive(Deserialize)]
    struct Tomb {
        bot_id: String,
        node_id: String,
    }
    let tombs: Vec<Tomb> = app.db.all("SELECT bot_id, node_id FROM node_tombstones LIMIT 50", &[]).await?;
    for t in tombs {
        if let Err(e) = app.delete_on_node(&t.bot_id, &t.node_id).await {
            log_error(&format!("tombstone {}: {e}", t.bot_id));
        }
    }
    match crate::gengwiki::sync_pending(app, now - GRANT_SETTLE_MS, 5).await {
        Ok(0) => {}
        Ok(n) => log(&format!("maintenance sent {n} gateway credits")),
        Err(e) => log_error(&format!("gateway credit retry failed: {}", e.message)),
    }
    match super::plugins::check_updates(app, now).await {
        Ok(0) => {}
        Ok(n) => log(&format!("maintenance updated {n} plugins to new upstream commits")),
        Err(e) => log_error(&format!("plugin update check failed: {}", e.message)),
    }
    let pushed = app.repush_pending(None, 50).await?;
    if pushed > 0 {
        log(&format!("maintenance re-pushed {pushed} bots"));
    }
    // A successful re-push clears status_detail, so the expiry note is restored afterwards.
    app.db
        .run(
            &format!("UPDATE bots SET status_detail = ?1 WHERE status = 'stopped' AND desired_running = 0 AND status_detail = '' AND user_id IN ({EXPIRED_USERS})"),
            args![SERVICE_EXPIRED_DETAIL, now],
        )
        .await?;
    Ok(())
}

/// Users without an active service period at `?2`.
const EXPIRED_USERS: &str = "SELECT id FROM users WHERE role != 'admin' AND (service_until IS NULL OR service_until <= ?2)";

/// Stops the running bots of users whose service period ended, optionally of one user only.
/// Data is kept.
pub async fn stop_expired(app: &App, now: i64, user_id: Option<&str>) -> ApiResult<usize> {
    #[derive(Deserialize)]
    struct Id {
        id: String,
    }
    let ids: Vec<Id> = app
        .db
        .all(&format!("SELECT id FROM bots WHERE desired_running = 1 AND user_id IN ({EXPIRED_USERS}) AND (?3 IS NULL OR user_id = ?3) LIMIT ?1"), args![100i64, now, user_id])
        .await?;
    for id in &ids {
        app.db
            .run(
                "UPDATE bots SET desired_running = 0, status = 'stopped', status_detail = ?2, updated_at = ?3 WHERE id = ?1",
                args![id.id, SERVICE_EXPIRED_DETAIL, now],
            )
            .await?;
        app.push_bot(&id.id).await?;
    }
    Ok(ids.len())
}
