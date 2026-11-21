//! Admin API for the gateway credits of a user.

use super::{json_body, Reply};
use crate::app::App;
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::gengwiki::{self, CreditRow, RemoteCredit, CREDIT_COLS, MAX_CREDIT_FEN};
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use worker::Request;

const NOTE_CHARS: usize = 200;

fn amount_fen(v: &Value) -> ApiResult<i64> {
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.as_f64().map(|n| n.to_string()))
        .and_then(|s| gengwiki::parse_fen(s.trim()))
        .filter(|f| (1..=MAX_CREDIT_FEN).contains(f))
        .ok_or_else(|| ApiError::bad_request(format!("额度应为 0.01 到 {} 元，最多两位小数", MAX_CREDIT_FEN / 100)))
}

fn expires_ms(v: &Value) -> ApiResult<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        v => v
            .as_i64()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| ApiError::bad_request("expires_at 应为毫秒时间戳或 null")),
    }
}

/// Credits of a user, newest first, each with what the gateway reports for it.
async fn credit_views(app: &App, user_id: &str) -> ApiResult<Value> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(flatten)]
        credit: CreditRow,
        created_by_email: Option<String>,
    }
    let cols = CREDIT_COLS
        .split(", ")
        .map(|c| format!("c.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let rows: Vec<Row> = app
        .db
        .all(
            &format!(
                "SELECT {cols}, u.email AS created_by_email FROM gateway_credits c LEFT JOIN users u ON u.id = c.created_by \
                 WHERE c.user_id = ?1 ORDER BY c.created_at DESC"
            ),
            args![user_id],
        )
        .await?;
    let gw = app.gengwiki();
    let account = gengwiki::account(app, user_id).await?;
    let (remote, remote_error): (HashMap<String, RemoteCredit>, Option<String>) = match &account {
        Some(a) if rows.iter().any(|r| r.credit.remote_id.is_some()) => match gw.credits(&a.remote_key_id).await {
            Ok(items) => (items.into_iter().map(|c| (c.id.clone(), c)).collect(), None),
            Err(e) => (HashMap::new(), Some(e)),
        },
        _ => (HashMap::new(), None),
    };
    let now = now_ms();
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let c = r.credit;
            let state = match (&c.remote_id, &c.error) {
                (Some(_), _) => "granted",
                (None, Some(_)) => "failed",
                (None, None) => "pending",
            };
            let live = c.remote_id.as_ref().and_then(|id| remote.get(id));
            json!({
                "id": c.id, "source": c.source, "amount": gengwiki::yuan(c.amount_fen), "expires_at": c.expires_at,
                "expired": c.expires_at.is_some_and(|t| t <= now), "note": c.note, "created_at": c.created_at,
                "created_by_email": r.created_by_email, "state": state, "error": c.error,
                "spent": live.map(|l| gengwiki::yuan_nano(l.spent_nano)),
                "remaining": live.map(|l| gengwiki::yuan_nano(l.remaining_nano)),
            })
        })
        .collect();
    Ok(
        json!({"configured": gw.token.is_some(), "account": account.is_some(), "items": items, "remote_error": remote_error}),
    )
}

pub async fn list(req: &Request, app: &App, user_id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let user = app.user_by_id(user_id).await?;
    Ok(Reply::json(credit_views(app, &user.id).await?))
}

/// Adds a credit and sends it to the gateway. A failed send stays pending for the maintenance run.
pub async fn add(req: &mut Request, app: &App, user_id: &str) -> ApiResult<Reply> {
    let admin = app.require_admin(req).await?;
    let body: Value = json_body(req).await?;
    let user = app.user_by_id(user_id).await?;
    if app.gengwiki().token.is_none() {
        return Err(ApiError::bad_request("活动额度服务未配置"));
    }
    let fen = amount_fen(&body["amount"])?;
    let expires_at = expires_ms(&body["expires_at"])?;
    if expires_at.is_some_and(|t| t <= now_ms()) {
        return Err(ApiError::bad_request("过期时间应晚于现在"));
    }
    let note: String = body["note"]
        .as_str()
        .unwrap_or("")
        .trim()
        .chars()
        .take(NOTE_CHARS)
        .collect();
    let id = new_id();
    app.db
        .run(
            "INSERT INTO gateway_credits (id, user_id, amount_fen, expires_at, source, ref, note, created_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'admin', ?5, ?6, ?7, ?8)",
            args![id, user.id, fen, expires_at, format!("admin:{id}"), note, admin.id, now_ms()],
        )
        .await?;
    gengwiki::sync_user(app, &user.id).await?;
    let mut reply = Reply::json(credit_views(app, &user.id).await?);
    reply.status = 201;
    Ok(reply)
}

/// Changes the total amount or the expiry of a credit.
pub async fn patch(req: &mut Request, app: &App, user_id: &str, credit_id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let body: Value = json_body(req).await?;
    let obj = body
        .as_object()
        .ok_or_else(|| ApiError::bad_request("请求体必须是对象"))?;
    let user = app.user_by_id(user_id).await?;
    let credit: CreditRow = app
        .db
        .first(
            &format!("SELECT {CREDIT_COLS} FROM gateway_credits WHERE id = ?1 AND user_id = ?2"),
            args![credit_id, user.id],
        )
        .await?
        .ok_or_else(|| ApiError::not_found("额度不存在"))?;
    let fen = obj.get("amount").map(amount_fen).transpose()?;
    let expires_at = obj.get("expires_at").map(expires_ms).transpose()?;
    if fen.is_none() && expires_at.is_none() {
        return Err(ApiError::bad_request("没有要修改的内容"));
    }
    if let Some(remote_id) = &credit.remote_id {
        let account = gengwiki::account(app, &user.id)
            .await?
            .ok_or_else(|| ApiError::internal("gateway account missing"))?;
        app.gengwiki()
            .patch_credit(&account.remote_key_id, remote_id, fen, expires_at)
            .await
            .map_err(|e| ApiError::new(502, "upstream_error", format!("修改失败：{e}")))?;
    }
    app.db
        .run(
            "UPDATE gateway_credits SET amount_fen = ?3, expires_at = ?4 WHERE id = ?1 AND user_id = ?2",
            args![
                credit.id,
                user.id,
                fen.unwrap_or(credit.amount_fen),
                expires_at.unwrap_or(credit.expires_at)
            ],
        )
        .await?;
    if credit.remote_id.is_none() {
        gengwiki::sync_user(app, &user.id).await?;
    }
    Ok(Reply::json(credit_views(app, &user.id).await?))
}
