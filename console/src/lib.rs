//! fish-console: the Cloudflare Worker behind fish.niox.lol.
//! `/api/*` is handled here, every other path is served from the static SPA assets.

pub mod api;
pub mod app;
pub mod captcha;
pub mod crypto;
pub mod db;
pub mod email;
pub mod error;
pub mod gateway;
pub mod gengwiki;
pub mod github;
pub mod http;
pub mod invite;
pub mod models;
pub mod node;
pub mod schema;
pub mod settings;
pub mod util;

use worker::{event, Context, Env, Headers, Request, Response, Result, ScheduleContext, ScheduledEvent};

fn json_response(status: u16, body: &serde_json::Value, cookies: &[String]) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    headers.set("x-content-type-options", "nosniff")?;
    for c in cookies {
        headers.append("set-cookie", c)?;
    }
    let bytes = serde_json::to_vec(body).map_err(|e| worker::Error::RustError(e.to_string()))?;
    Ok(Response::from_bytes(bytes)?.with_status(status).with_headers(headers))
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.path();
    if path != "/api" && !path.starts_with("/api/") {
        return env.assets("ASSETS")?.fetch_request(req).await;
    }
    let app = match app::App::new(env) {
        Ok(a) => a,
        Err(e) => {
            let err = error::ApiError::internal(format!("init: {e}"));
            return json_response(err.status, &err.body(), &[]);
        }
    };
    if path.starts_with(api::archive::PREFIX) {
        return api::archive::proxy(req, &app).await;
    }
    match api::handle(req, &app).await {
        Ok(reply) => json_response(reply.status, &reply.body, &reply.cookies),
        Err(e) => json_response(e.status, &e.body(), &[]),
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    match app::App::new(env) {
        Ok(app) => {
            if let Err(e) = api::run_maintenance(&app).await {
                util::log_error(&format!("maintenance failed: {e}"));
            }
        }
        Err(e) => util::log_error(&format!("maintenance init failed: {e}")),
    }
}
