//! `/api/*` routing.

mod admin;
mod auth;
mod bots;
mod internal;
pub mod archive;
pub(crate) mod keys;
mod platforms;
mod plugins;
mod usage;

use crate::app::App;
use crate::error::{ApiError, ApiResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
use worker::{Method, Request};

pub use internal::run_maintenance;

pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub cookies: Vec<String>,
}

impl Reply {
    pub fn json(body: Value) -> Self {
        Self { status: 200, body, cookies: vec![] }
    }
    pub fn with_cookie(mut self, cookie: String) -> Self {
        self.cookies.push(cookie);
        self
    }
}

pub const MAX_BODY_BYTES: usize = 512 * 1024;

pub async fn read_body(req: &mut Request) -> ApiResult<Vec<u8>> {
    let bytes = req.bytes().await.map_err(|_| ApiError::bad_request("请求体读取失败"))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(ApiError::new(413, "payload_too_large", "请求内容过大"));
    }
    Ok(bytes)
}

pub async fn json_body<T: DeserializeOwned>(req: &mut Request) -> ApiResult<T> {
    let bytes = read_body(req).await?;
    let slice: &[u8] = if bytes.iter().all(|b| b.is_ascii_whitespace()) { b"{}" } else { &bytes };
    serde_json::from_slice(slice).map_err(|e| ApiError::bad_request(format!("请求格式错误：{e}")))
}

pub fn client_ip(req: &Request) -> String {
    req.headers().get("cf-connecting-ip").ok().flatten().unwrap_or_else(|| "unknown".into())
}

pub fn user_agent(req: &Request) -> String {
    req.headers().get("user-agent").ok().flatten().unwrap_or_default()
}

fn is_mutating(m: &Method) -> bool {
    matches!(m, Method::Post | Method::Put | Method::Patch | Method::Delete)
}

pub async fn handle(mut req: Request, app: &App) -> ApiResult<Reply> {
    let method = req.method();
    let path = req.path();
    let segs: Vec<String> = path.trim_start_matches("/api/").split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();

    let internal = s.first() == Some(&"internal");
    let qq_callback = s.len() == 3 && s[0] == "qq" && s[1] == "webhook";
    if is_mutating(&method) && !internal && !qq_callback {
        let csrf = req.headers().get("x-fish-csrf").ok().flatten();
        if csrf.as_deref() != Some("1") {
            return Err(ApiError::new(403, "csrf_failed", "请求缺少安全校验头，请刷新页面后重试"));
        }
    }

    use Method::*;
    match (method, s.as_slice()) {
        (Get, ["captcha"]) => auth::captcha(app).await,
        (Post, ["auth", "email-code"]) => auth::email_code(&mut req, app).await,
        (Post, ["auth", "register"]) => auth::register(&mut req, app).await,
        (Post, ["auth", "login"]) => auth::login(&mut req, app).await,
        (Post, ["auth", "logout"]) => auth::logout(&req, app).await,
        (Post, ["auth", "reset"]) => auth::reset(&mut req, app).await,
        (Get, ["me"]) => auth::me(&req, app).await,
        (Post, ["me", "redeem"]) => auth::redeem(&mut req, app).await,

        (Get, ["llm-keys"]) => keys::list(&req, app).await,
        (Post, ["llm-keys"]) => keys::create(&mut req, app).await,
        (Delete, ["llm-keys", id]) => keys::delete(&req, app, id).await,
        (Get, ["llm-keys", id, "balance"]) => keys::balance(&req, app, id).await,

        (Get, ["bots"]) => bots::list(&req, app).await,
        (Post, ["bots"]) => bots::create(&mut req, app).await,
        (Get, ["bots", id]) => bots::get(&req, app, id).await,
        (Patch, ["bots", id]) => bots::patch(&mut req, app, id).await,
        (Delete, ["bots", id]) => bots::delete(&req, app, id).await,
        (Post, ["bots", id, action @ ("start" | "stop" | "restart")]) => bots::lifecycle(&req, app, id, action).await,
        (Get, ["bots", id, "logs"]) => bots::logs(&req, app, id).await,

        (Post, ["bots", id, "platforms", kind @ ("qq" | "weixin"), "qr"]) => platforms::qr_start(&req, app, id, kind).await,
        (Get, ["bots", id, "platforms", kind @ ("qq" | "weixin"), "qr", task]) => platforms::qr_poll(&req, app, id, kind, task).await,
        (Post, ["qq", "webhook", pid]) => platforms::qq_webhook(&mut req, app, pid).await,
        (Post, ["bots", id, "platforms", "qq", "manual"]) => platforms::qq_manual(&mut req, app, id).await,
        (method @ (Get | Post), ["bots", id, "platforms", pid, "qq-panels"]) => platforms::qq_panels(&mut req, app, id, pid, method, None).await,
        (method @ (Put | Delete), ["bots", id, "platforms", pid, "qq-panels", panel]) => platforms::qq_panels(&mut req, app, id, pid, method, Some(panel)).await,
        (Patch, ["bots", id, "platforms", pid]) => platforms::patch(&mut req, app, id, pid).await,
        (Delete, ["bots", id, "platforms", pid]) => platforms::delete(&req, app, id, pid).await,

        (Post, ["plugins", "inspect"]) => plugins::inspect_route(&mut req, app).await,
        (Post, ["bots", id, "plugins"]) => plugins::install(&mut req, app, id).await,
        (Patch, ["bots", id, "plugins", pid]) => plugins::patch(&mut req, app, id, pid).await,
        (Post, ["bots", id, "plugins", pid, "update"]) => plugins::update(&req, app, id, pid).await,
        (Post, ["bots", id, "plugins", pid, "reload"]) => plugins::reload(&req, app, id, pid).await,
        (Delete, ["bots", id, "plugins", pid]) => plugins::delete(&req, app, id, pid).await,

        (Get, ["usage"]) => usage::get(&req, app).await,

        (Get, ["admin", "overview"]) => admin::overview(&req, app).await,
        (Get, ["admin", "system"]) => admin::system(&req, app).await,
        (Get, ["admin", "users"]) => admin::users(&req, app).await,
        (Get, ["admin", "users", id]) => admin::user(&req, app, id).await,
        (Patch, ["admin", "users", id]) => admin::patch_user(&mut req, app, id).await,
        (Get, ["admin", "users", id, "usage"]) => admin::user_usage_route(&req, app, id).await,
        (Get, ["admin", "users", id, "bots", bot_id, "logs"]) => admin::user_bot_logs(&req, app, id, bot_id).await,
        (Get, ["admin", "invites"]) => admin::invites(&req, app).await,
        (Post, ["admin", "invites"]) => admin::create_invites(&mut req, app).await,
        (Delete, ["admin", "invites", code]) => admin::revoke_invite(&req, app, code).await,

        (Post, ["internal", "node", "events"]) => internal::node_events(&mut req, app).await,

        _ => Err(ApiError::not_found("接口不存在")),
    }
}
