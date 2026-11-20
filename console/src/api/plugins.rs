//! Plugin inspection from GitHub and per bot plugin management.

use super::{json_body, Reply};
use crate::app::{ok_json, App, PLUGIN_COLS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::github::{self, PluginMeta, RepoRef};
use crate::http;
use crate::models::PluginRow;
use crate::schema;
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Method, Request};

pub struct Inspected {
    pub meta: PluginMeta,
    pub repo_url: String,
    pub schema: Option<Value>,
    pub has_requirements: bool,
}

impl Inspected {
    pub fn view(&self) -> Value {
        json!({
            "name": self.meta.name,
            "display_name": self.meta.display_name,
            "version": self.meta.version,
            "author": self.meta.author,
            "description": self.meta.description,
            "repo_url": self.repo_url,
            "schema": self.schema,
            "has_requirements": self.has_requirements,
        })
    }
}

fn repo_invalid(msg: &str) -> ApiError {
    ApiError::new(400, "repo_invalid", msg)
}

fn github_down() -> ApiError {
    ApiError::new(502, "github_unavailable", "暂时无法访问 GitHub，请稍后再试")
}

async fn gh_get(app: &App, url: &str, api: bool) -> ApiResult<http::HttpResponse> {
    let token = app.var("GITHUB_TOKEN").filter(|t| !t.is_empty()).map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = vec![("user-agent", "fish-console")];
    if api {
        headers.push(("accept", "application/vnd.github+json"));
        if let Some(t) = &token {
            headers.push(("authorization", t.as_str()));
        }
    }
    http::send(Method::Get, url, &headers, None, 8_000).await.map_err(|e| {
        crate::util::log_error(&format!("github {url}: {e}"));
        github_down()
    })
}

/// GitHub endpoints. Dev mode may point them at a local fake through GITHUB_API_BASE and GITHUB_RAW_BASE.
fn bases(app: &App) -> (String, String) {
    let pick = |k: &str, d: &str| if app.dev { app.var(k).filter(|s| !s.is_empty()) } else { None }.unwrap_or_else(|| d.to_string());
    (pick("GITHUB_API_BASE", github::API_BASE), pick("GITHUB_RAW_BASE", github::RAW_BASE))
}

/// Fetches a raw file. Ok(None) means the file does not exist.
async fn raw_file(app: &App, repo: &RepoRef, branch: &str, file: &str) -> ApiResult<Option<String>> {
    let resp = gh_get(app, &github::raw_url(&bases(app).1, repo, branch, file), false).await?;
    match resp.status {
        200 => Ok(Some(resp.text())),
        404 => Ok(None),
        _ => Err(github_down()),
    }
}

pub async fn inspect(app: &App, repo_url: &str) -> ApiResult<Inspected> {
    let repo = github::parse_repo_url(repo_url).ok_or_else(|| repo_invalid("仓库地址无效，请粘贴 https://github.com/作者/仓库 形式的链接"))?;
    let api = gh_get(app, &github::api_repo_url(&bases(app).0, &repo), true).await?;
    match api.status {
        200 => {}
        404 | 451 => return Err(repo_invalid("仓库不存在或不是公开仓库")),
        403 | 429 => return Err(ApiError::new(502, "github_unavailable", "GitHub 请求过于频繁，请稍后再试")),
        _ => return Err(github_down()),
    }
    let info = api.json().unwrap_or(Value::Null);
    if info.get("private").and_then(Value::as_bool) == Some(true) {
        return Err(repo_invalid("仅支持公开仓库"));
    }
    let default_branch = info.get("default_branch").and_then(Value::as_str).unwrap_or("main").to_string();
    let branch = repo.branch.clone().unwrap_or(default_branch);

    let meta_text = match raw_file(app, &repo, &branch, "metadata.yaml").await? {
        Some(t) => t,
        None => raw_file(app, &repo, &branch, "metadata.yml")
            .await?
            .ok_or_else(|| ApiError::new(400, "not_astrbot_plugin", "仓库中没有 metadata.yaml，不是 AstrBot 插件"))?,
    };
    let meta = github::parse_metadata(&meta_text, &repo)
        .ok_or_else(|| ApiError::new(400, "not_astrbot_plugin", "metadata.yaml 缺少插件名称或格式不正确"))?;
    let schema = raw_file(app, &repo, &branch, "_conf_schema.json").await?.and_then(|t| github::parse_schema(&t));
    let has_requirements = raw_file(app, &repo, &branch, "requirements.txt").await?.is_some();
    let repo_url = match &repo.branch {
        Some(b) => format!("{}/tree/{b}", repo.canonical_url()),
        None => repo.canonical_url(),
    };
    Ok(Inspected { meta, repo_url, schema, has_requirements })
}

#[derive(Deserialize)]
struct RepoReq {
    repo_url: String,
}

pub async fn inspect_route(req: &mut Request, app: &App) -> ApiResult<Reply> {
    app.current_user(req).await?;
    let body: RepoReq = json_body(req).await?;
    Ok(Reply::json(inspect(app, &body.repo_url).await?.view()))
}

async fn plugin_by_id(app: &App, bot_id: &str, pid: &str) -> ApiResult<PluginRow> {
    app.db
        .first(&format!("SELECT {PLUGIN_COLS} FROM plugins WHERE id = ?1 AND bot_id = ?2"), args![pid, bot_id])
        .await?
        .ok_or_else(|| ApiError::not_found("插件不存在"))
}

async fn touch_bot(app: &App, bot_id: &str) -> ApiResult<()> {
    app.db.run("UPDATE bots SET updated_at = ?2 WHERE id = ?1", args![bot_id, now_ms()]).await?;
    Ok(())
}

pub async fn install(req: &mut Request, app: &App, bot_id: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    App::require_service(&user)?;
    let body: RepoReq = json_body(req).await?;
    let used = app
        .db
        .count("SELECT COUNT(*) AS n FROM plugins p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = ?1", args![user.id])
        .await?;
    if user.quota_plugins >= 0 && used >= user.quota_plugins {
        return Err(ApiError::quota(format!("最多可以安装 {} 个插件", user.quota_plugins)));
    }
    let ins = inspect(app, &body.repo_url).await?;
    let exists = app.db.count("SELECT COUNT(*) AS n FROM plugins WHERE bot_id = ?1 AND name = ?2", args![bot.id, ins.meta.name]).await?;
    if exists > 0 {
        return Err(ApiError::conflict("plugin_exists", "这只小鱼已经安装了同名插件"));
    }
    let config = ins.schema.as_ref().map(schema::defaults).unwrap_or_else(|| json!({}));
    let schema_json = ins.schema.as_ref().map(serde_json::to_string).transpose()?;
    let id = new_id();
    let now = now_ms();
    let inserted = app
        .db
        .run(
            "INSERT INTO plugins (id, bot_id, repo_url, name, display_name, version, author, description, enabled, config_json, schema_json, status, status_detail, install_rev, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, 'installing', '', ?11, ?11, ?11)",
            args![id, bot.id, ins.repo_url, ins.meta.name, ins.meta.display_name, ins.meta.version, ins.meta.author, ins.meta.description,
                  serde_json::to_string(&config)?, schema_json, now],
        )
        .await;
    if let Err(e) = inserted {
        if e.code == "conflict" {
            return Err(ApiError::conflict("plugin_exists", "这只小鱼已经安装了同名插件"));
        }
        return Err(e);
    }
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(plugin_by_id(app, &bot.id, &id).await?.view()))
}

#[derive(Deserialize)]
struct PatchReq {
    enabled: Option<bool>,
    config: Option<Value>,
}

pub async fn patch(req: &mut Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let p = plugin_by_id(app, &bot.id, pid).await?;
    let body: PatchReq = json_body(req).await?;
    let config = match &body.config {
        Some(c) => {
            schema::validate(p.schema().as_ref(), c).map_err(|m| ApiError::new(400, "config_invalid", m))?;
            c.clone()
        }
        None => p.config(),
    };
    let enabled = body.enabled.map(i64::from).unwrap_or(p.enabled);
    let status = if enabled == 0 { "stopped" } else if p.enabled == 0 && p.status == "stopped" { "installing" } else { p.status.as_str() };
    app.db
        .run(
            "UPDATE plugins SET enabled = ?2, config_json = ?3, status = ?4, updated_at = ?5 WHERE id = ?1",
            args![p.id, enabled, serde_json::to_string(&config)?, status, now_ms()],
        )
        .await?;
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(plugin_by_id(app, &bot.id, pid).await?.view()))
}

pub async fn update(req: &Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let p = plugin_by_id(app, &bot.id, pid).await?;
    let ins = inspect(app, &p.repo_url).await?;
    let mut config = ins.schema.as_ref().map(schema::defaults).unwrap_or_else(|| json!({}));
    schema::merge_missing(&mut config, &p.config(), true);
    let schema_json = ins.schema.as_ref().map(serde_json::to_string).transpose()?;
    let now = now_ms();
    app.db
        .run(
            "UPDATE plugins SET display_name = ?2, version = ?3, author = ?4, description = ?5, config_json = ?6, schema_json = ?7, \
             status = 'installing', status_detail = '', install_rev = ?8, updated_at = ?8 WHERE id = ?1",
            args![p.id, ins.meta.display_name, ins.meta.version, ins.meta.author, ins.meta.description, serde_json::to_string(&config)?, schema_json, now],
        )
        .await?;
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(plugin_by_id(app, &bot.id, pid).await?.view()))
}

pub async fn delete(req: &Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let n = app.db.run("DELETE FROM plugins WHERE id = ?1 AND bot_id = ?2", args![pid, bot.id]).await?;
    if n == 0 {
        return Err(ApiError::not_found("插件不存在"));
    }
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(ok_json()))
}
