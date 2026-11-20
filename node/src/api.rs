use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::model::DesiredBot;
use crate::node::Node;
use crate::qr::Kind;
use crate::sign::{self, now_ms, VerifyError, VerifyInput};

mod qq_panels;

pub const MAX_BODY: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
        }
    }

    fn not_found() -> Self {
        ApiError::new(StatusCode::NOT_FOUND, "bot_not_found", "机器人不存在")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"code": self.code, "message": self.message}})),
        )
            .into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

pub fn router(node: Arc<Node>) -> Router {
    let mut v1 = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/bots/{bot_id}", put(put_bot).delete(delete_bot))
        .route("/v1/bots/{bot_id}/restart", post(restart_bot))
        .route("/v1/bots/{bot_id}/status", get(bot_status))
        .route("/v1/bots/{bot_id}/logs", get(bot_logs))
        .route("/v1/bots/{bot_id}/plugins/{plugin_id}/reload", post(reload_plugin))
        .route("/v1/bots/{bot_id}/platforms/{platform_id}/qq-webhook", post(qq_webhook))
        .route("/v1/bots/{bot_id}/platforms/{platform_id}/qq-panels", get(qq_panels::list).post(qq_panels::create))
        .route("/v1/bots/{bot_id}/platforms/{platform_id}/qq-panels/{panel_id}", put(qq_panels::update).delete(qq_panels::delete))
        .route("/v1/usage", get(usage))
        .route("/v1/usage/summary", get(usage_summary))
        .route("/v1/qr/qq", post(qr_qq_create))
        .route("/v1/qr/qq/{task_id}", get(qr_qq_get))
        .route("/v1/qr/weixin", post(qr_wx_create))
        .route("/v1/qr/weixin/{task_id}", get(qr_wx_get));
    if node.shared.cfg.dev_services {
        v1 = v1
            .route("/v1/dev/bots/{bot_id}/inject", post(loopback_inject))
            .route("/v1/dev/bots/{bot_id}/outbox", get(loopback_outbox));
    }
    let v1 = v1.layer(middleware::from_fn_with_state(node.clone(), verify_signature));
    v1.fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not_found", "接口不存在") })
        .with_state(node)
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

async fn verify_signature(State(node): State<Arc<Node>>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大").into_response(),
    };
    let path_q = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let cfg = &node.shared.cfg;
    let keys: [&[u8]; 2] = [&cfg.secret, cfg.secret_text.as_bytes()];
    let input = VerifyInput {
        node: header(&parts.headers, sign::HEADER_NODE),
        ts: header(&parts.headers, sign::HEADER_TS),
        sig: header(&parts.headers, sign::HEADER_SIG),
        method: parts.method.as_str(),
        path_with_query: path_q,
        body: &bytes,
    };
    if let Err(e) = sign::verify(&keys, &cfg.node_id, &input, now_ms()) {
        let (code, msg) = match e {
            VerifyError::MissingHeaders => ("signature_missing", "缺少签名头"),
            VerifyError::WrongNode => ("node_mismatch", "节点 ID 不匹配"),
            VerifyError::Expired => ("signature_expired", "签名已过期"),
            VerifyError::BadSignature => ("signature_invalid", "签名无效"),
        };
        return ApiError::new(StatusCode::UNAUTHORIZED, code, msg).into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

async fn health(State(node): State<Arc<Node>>) -> ApiResult {
    Ok(Json(node.health().await))
}

async fn put_bot(State(node): State<Arc<Node>>, Path(bot_id): Path<String>, body: axum::body::Bytes) -> ApiResult {
    let mut d: DesiredBot = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_body", format!("请求体格式错误: {e}")))?;
    if d.bot_id.is_empty() {
        d.bot_id = bot_id.clone();
    }
    if d.bot_id != bot_id {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "bot_id 与路径不一致",
        ));
    }
    if d.user_id.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_body", "缺少 user_id"));
    }
    if let Some(existing) = node.get(&bot_id) {
        if existing.user_id() != d.user_id {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "bot_owner_mismatch",
                "机器人归属用户不一致",
            ));
        }
    }
    node.put(d)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("保存失败: {e}")))?;
    Ok(Json(json!({"ok": true})))
}

async fn delete_bot(State(node): State<Arc<Node>>, Path(bot_id): Path<String>) -> ApiResult {
    node.delete(&bot_id).await;
    Ok(Json(json!({"ok": true})))
}

async fn restart_bot(State(node): State<Arc<Node>>, Path(bot_id): Path<String>) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    bot.restart().await;
    Ok(Json(json!({"ok": true})))
}

async fn reload_plugin(State(node): State<Arc<Node>>, Path((bot_id, plugin_id)): Path<(String, String)>) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    bot.reload_plugin(&plugin_id).await
        .map_err(|error| ApiError::new(StatusCode::CONFLICT, "plugin_reload_unavailable", error))?;
    Ok(Json(json!({"ok": true})))
}

async fn bot_status(State(node): State<Arc<Node>>, Path(bot_id): Path<String>) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    Ok(Json(bot.status_json()))
}

#[derive(Deserialize)]
struct LogsQuery {
    after: Option<String>,
}

async fn bot_logs(State(node): State<Arc<Node>>, Path(bot_id): Path<String>, Query(q): Query<LogsQuery>) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    let after = q.after.as_deref().and_then(|a| a.parse::<u64>().ok());
    let (items, cursor) = bot.logs.read_after(after, 200);
    Ok(Json(json!({"items": items, "cursor": cursor})))
}

#[derive(Deserialize)]
struct UsageQuery {
    user_id: Option<String>,
}

async fn usage(State(node): State<Arc<Node>>, Query(q): Query<UsageQuery>) -> ApiResult {
    let user = q
        .user_id
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_query", "缺少 user_id"))?;
    Ok(Json(node.usage.user_usage(&node, &user).await))
}

async fn usage_summary(State(node): State<Arc<Node>>) -> ApiResult {
    Ok(Json(node.usage.summary(&node).await))
}

fn upstream(e: anyhow::Error) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "qr_upstream_error", format!("{e:#}"))
}

fn task_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "qr_task_not_found", "二维码任务不存在或已过期")
}

async fn qr_qq_create(State(node): State<Arc<Node>>) -> ApiResult {
    node.qr.create_qq().await.map(Json).map_err(upstream)
}

async fn qr_qq_get(State(node): State<Arc<Node>>, Path(task_id): Path<String>) -> ApiResult {
    node.qr.get(Kind::Qq, &task_id).map(Json).ok_or_else(task_not_found)
}

async fn qr_wx_create(State(node): State<Arc<Node>>) -> ApiResult {
    node.qr.create_weixin().await.map(Json).map_err(upstream)
}

async fn qr_wx_get(State(node): State<Arc<Node>>, Path(task_id): Path<String>) -> ApiResult {
    node.qr.get(Kind::Weixin, &task_id).map(Json).ok_or_else(task_not_found)
}

async fn qq_webhook(
    State(node): State<Arc<Node>>,
    Path((bot_id, platform_id)): Path<(String, String)>,
    Json(input): Json<crate::platform::qq::WebhookDelivery>,
) -> ApiResult {
    use crate::platform::qq::WebhookReply;
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    match bot.qq_webhook(&platform_id, &input).await {
        WebhookReply::Ok(v) => Ok(Json(v)),
        WebhookReply::Unauthorized(m) => Err(ApiError::new(StatusCode::UNAUTHORIZED, "webhook_unauthorized", m)),
        WebhookReply::BadRequest(m) => Err(ApiError::new(StatusCode::BAD_REQUEST, "webhook_rejected", m)),
    }
}

async fn loopback_inject(
    State(node): State<Arc<Node>>,
    Path(bot_id): Path<String>,
    Json(input): Json<crate::platform::loopback::Inject>,
) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    bot.inject_loopback(input)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "loopback_rejected", e))?;
    Ok(Json(json!({"ok": true})))
}

async fn loopback_outbox(State(node): State<Arc<Node>>, Path(bot_id): Path<String>) -> ApiResult {
    let bot = node.get(&bot_id).ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({"items": bot.loopback_outbox()})))
}
