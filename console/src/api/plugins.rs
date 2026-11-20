//! Plugin inspection from GitHub and per bot plugin management.

use super::{json_body, Reply};
use crate::app::{ok_json, App, PLUGIN_COLS, PUSH_TIMEOUT_MS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::github::{self, PluginMeta, RepoRef};
use crate::http;
use crate::models::PluginRow;
use crate::node::NodeError;
use crate::schema;
use crate::util::{new_id, now_ms};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Method, Request};

pub struct Inspected {
    pub meta: PluginMeta,
    pub repo_url: String,
    pub schema: Option<Value>,
    /// Commit the metadata was read at. None when GitHub did not report the latest commit.
    pub commit: Option<String>,
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

struct Bases {
    api: String,
    raw: String,
    git: String,
}

/// GitHub endpoints. Dev mode may point them at a local fake through GITHUB_API_BASE, GITHUB_RAW_BASE
/// and GITHUB_GIT_BASE.
fn bases(app: &App) -> Bases {
    let pick = |k: &str, d: &str| if app.dev { app.var(k).filter(|s| !s.is_empty()) } else { None }.unwrap_or_else(|| d.to_string());
    Bases {
        api: pick("GITHUB_API_BASE", github::API_BASE),
        raw: pick("GITHUB_RAW_BASE", github::RAW_BASE),
        git: pick("GITHUB_GIT_BASE", github::GIT_BASE),
    }
}

/// Latest commit of the repository's branch, or of its default branch. Reads the git ref advertisement
/// first and the REST API second. None when neither answers.
pub async fn latest_commit(app: &App, repo: &RepoRef) -> Option<String> {
    let b = bases(app);
    if let Ok(resp) = gh_get(app, &github::refs_url(&b.git, repo), false).await {
        if resp.status == 200 {
            if let Some(c) = github::advertised_commit(&resp.body, repo.branch.as_deref()) {
                return Some(c);
            }
        }
    }
    let reference = repo.branch.as_deref().unwrap_or("HEAD");
    let resp = gh_get(app, &github::api_commit_url(&b.api, repo, reference), true).await.ok()?;
    if resp.status != 200 {
        return None;
    }
    resp.json()?.get("sha")?.as_str().filter(|s| github::is_commit(s)).map(|s| s.to_ascii_lowercase())
}

/// Fetches a raw file. Ok(None) means the file does not exist.
async fn raw_file(app: &App, repo: &RepoRef, branch: &str, file: &str) -> ApiResult<Option<String>> {
    let resp = gh_get(app, &github::raw_url(&bases(app).raw, repo, branch, file), false).await?;
    match resp.status {
        200 => Ok(Some(resp.text())),
        404 => Ok(None),
        _ => Err(github_down()),
    }
}

/// Explains a missing metadata file. Uses the GitHub API only on this path, because unauthenticated
/// API calls from Worker egress addresses are usually rate limited.
async fn missing_metadata(app: &App, repo: &RepoRef) -> ApiError {
    let not_plugin = |msg: &str| ApiError::new(400, "not_astrbot_plugin", msg);
    let unclear = || not_plugin("没有找到 metadata.yaml。请确认仓库存在、是公开仓库，并且是 AstrBot 插件");
    let Ok(api) = gh_get(app, &github::api_repo_url(&bases(app).api, repo), true).await else {
        return unclear();
    };
    match api.status {
        200 if api.json().and_then(|v| v.get("private").and_then(Value::as_bool)) == Some(true) => repo_invalid("仅支持公开仓库"),
        200 => not_plugin("仓库中没有 metadata.yaml，不是 AstrBot 插件"),
        404 | 451 => repo_invalid("仓库不存在或不是公开仓库"),
        _ => unclear(),
    }
}

pub async fn inspect(app: &App, repo_url: &str) -> ApiResult<Inspected> {
    let repo = github::parse_repo_url(repo_url).ok_or_else(|| repo_invalid("仓库地址无效，请粘贴 https://github.com/作者/仓库 形式的链接"))?;
    let commit = latest_commit(app, &repo).await;
    inspect_at(app, &repo, commit).await
}

/// Reads the plugin metadata at `commit`, or at the head of the branch when it is None.
async fn inspect_at(app: &App, repo: &RepoRef, commit: Option<String>) -> ApiResult<Inspected> {
    let repo = repo.clone();
    let branch = commit.clone().or_else(|| repo.branch.clone()).unwrap_or_else(|| "HEAD".to_string());

    let meta_text = match raw_file(app, &repo, &branch, "metadata.yaml").await? {
        Some(t) => t,
        None => match raw_file(app, &repo, &branch, "metadata.yml").await? {
            Some(t) => t,
            None => return Err(missing_metadata(app, &repo).await),
        },
    };
    let meta = github::parse_metadata(&meta_text, &repo)
        .ok_or_else(|| ApiError::new(400, "not_astrbot_plugin", "metadata.yaml 缺少插件名称或格式不正确"))?;
    let schema = raw_file(app, &repo, &branch, "_conf_schema.json").await?.and_then(|t| github::parse_schema(&t));
    let repo_url = match &repo.branch {
        Some(b) => format!("{}/tree/{b}", repo.canonical_url()),
        None => repo.canonical_url(),
    };
    Ok(Inspected { meta, repo_url, schema, commit })
}

#[derive(Deserialize)]
struct InstallReq {
    plugin_id: String,
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
    let body: InstallReq = json_body(req).await?;
    let used = app
        .db
        .count("SELECT COUNT(*) AS n FROM plugins p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = ?1", args![user.id])
        .await?;
    if user.quota_plugins >= 0 && used >= user.quota_plugins {
        return Err(ApiError::quota(format!("最多可以安装 {} 个插件", user.quota_plugins)));
    }
    let item = super::market::find(req, app, &body.plugin_id).await?;
    let ins = inspect(app, &item.repo_url).await?;
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
            "INSERT INTO plugins (id, bot_id, repo_url, name, display_name, version, author, description, enabled, config_json, schema_json, status, status_detail, install_rev, commit_sha, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, 'installing', '', ?11, ?12, ?11, ?11)",
            args![id, bot.id, ins.repo_url, ins.meta.name, ins.meta.display_name, ins.meta.version, ins.meta.author, ins.meta.description,
                  serde_json::to_string(&config)?, schema_json, now, ins.commit.clone().unwrap_or_default()],
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
    config_revision: Option<String>,
}

fn config_changed() -> ApiError {
    ApiError::conflict("plugin_config_changed", "插件设置有更新，请重新载入后再编辑")
}

pub async fn patch(req: &mut Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let p = plugin_by_id(app, &bot.id, pid).await?;
    let body: PatchReq = json_body(req).await?;
    let config = match body.config {
        Some(c) => {
            if body.config_revision.is_some_and(|revision| revision != p.config_revision()) {
                return Err(config_changed());
            }
            schema::validate(p.schema().as_ref(), &c).map_err(|m| ApiError::new(400, "config_invalid", m))?;
            Some(serde_json::to_string(&c)?)
        }
        None => None,
    };
    let changed = app.db
        .run(
            "UPDATE plugins SET enabled = COALESCE(?2, enabled), config_json = COALESCE(?3, config_json), \
             status = CASE WHEN ?2 = 0 THEN 'stopped' WHEN ?2 = 1 AND enabled = 0 AND status = 'stopped' THEN 'installing' ELSE status END, \
             updated_at = ?4 WHERE id = ?1 AND (?3 IS NULL OR (config_json = ?5 AND schema_json IS ?6))",
            args![p.id, body.enabled.map(i64::from), config, now_ms(), p.config_json, p.schema_json],
        )
        .await?;
    if changed == 0 {
        plugin_by_id(app, &bot.id, pid).await?;
        return Err(config_changed());
    }
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(plugin_by_id(app, &bot.id, pid).await?.view()))
}

pub async fn update(req: &Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    let p = plugin_by_id(app, &bot.id, pid).await?;
    let ins = inspect(app, &p.repo_url).await?;
    if !apply_update(app, &p, &ins, now_ms()).await? {
        plugin_by_id(app, &bot.id, pid).await?;
        return Err(config_changed());
    }
    touch_bot(app, &bot.id).await?;
    app.push_bot(&bot.id).await?;
    Ok(Reply::json(plugin_by_id(app, &bot.id, pid).await?.view()))
}

/// Stores the metadata of `ins` on the plugin, keeps its settings, pins it to `ins.commit` and bumps
/// `install_rev` so the node reinstalls it. Returns false when the plugin changed since `p` was read.
async fn apply_update(app: &App, p: &PluginRow, ins: &Inspected, now: i64) -> ApiResult<bool> {
    let mut config = ins.schema.as_ref().map(schema::defaults).unwrap_or_else(|| json!({}));
    schema::merge_missing(&mut config, &p.config(), true);
    let schema_json = ins.schema.as_ref().map(serde_json::to_string).transpose()?;
    let changed = app
        .db
        .run(
            "UPDATE plugins SET display_name = ?2, version = ?3, author = ?4, description = ?5, config_json = ?6, schema_json = ?7, \
             status = CASE WHEN enabled = 1 THEN 'installing' ELSE 'stopped' END, status_detail = '', install_rev = ?8, updated_at = ?8, \
             commit_sha = ?11 \
             WHERE id = ?1 AND config_json = ?9 AND install_rev = ?10",
            args![p.id, ins.meta.display_name, ins.meta.version, ins.meta.author, ins.meta.description, serde_json::to_string(&config)?, schema_json, now,
                  p.config_json, p.install_rev, ins.commit.clone().unwrap_or_default()],
        )
        .await?;
    Ok(changed > 0)
}

/// Repositories checked per run of `check_updates`.
const UPDATE_REPOS_PER_RUN: i64 = 5;
/// Time between two checks of the same repository.
const UPDATE_INTERVAL_MS: i64 = 3600 * 1000;

/// Looks for new commits of installed plugin repositories that were not checked within
/// `UPDATE_INTERVAL_MS` and moves every plugin of a repository with a new commit to it, the same way
/// a manual update does. Returns the number of updated plugins.
pub async fn check_updates(app: &App, now: i64) -> ApiResult<usize> {
    app.db
        .batch(vec![
            app.db.stmt("INSERT OR IGNORE INTO plugin_repos (repo_url) SELECT DISTINCT repo_url FROM plugins", args![])?,
            app.db.stmt("DELETE FROM plugin_repos WHERE repo_url NOT IN (SELECT repo_url FROM plugins)", args![])?,
        ])
        .await?;
    #[derive(Deserialize)]
    struct Due {
        repo_url: String,
    }
    let due: Vec<Due> = app
        .db
        .all(
            "SELECT repo_url FROM plugin_repos WHERE checked_at <= ?1 ORDER BY checked_at LIMIT ?2",
            args![now - UPDATE_INTERVAL_MS, UPDATE_REPOS_PER_RUN],
        )
        .await?;
    let mut updated = 0;
    let mut bots: Vec<String> = Vec::new();
    for d in due {
        app.db.run("UPDATE plugin_repos SET checked_at = ?2 WHERE repo_url = ?1", args![d.repo_url, now]).await?;
        let Some(repo) = github::parse_repo_url(&d.repo_url) else { continue };
        let Some(commit) = latest_commit(app, &repo).await else { continue };
        let stale: Vec<PluginRow> = app
            .db
            .all(&format!("SELECT {PLUGIN_COLS} FROM plugins WHERE repo_url = ?1 AND commit_sha != ?2"), args![d.repo_url, commit])
            .await?;
        if !stale.is_empty() {
            let ins = match inspect_at(app, &repo, Some(commit.clone())).await {
                Ok(ins) => ins,
                Err(e) => {
                    crate::util::log_error(&format!("plugin update {} at {commit}: {}", d.repo_url, e.message));
                    continue;
                }
            };
            for p in &stale {
                if ins.meta.name != p.name {
                    crate::util::log_error(&format!("plugin update {} at {commit}: name changed to {}", d.repo_url, ins.meta.name));
                    continue;
                }
                if apply_update(app, p, &ins, now).await? {
                    updated += 1;
                    if !bots.contains(&p.bot_id) {
                        bots.push(p.bot_id.clone());
                    }
                }
            }
        }
        app.db.run("UPDATE plugin_repos SET commit_sha = ?2 WHERE repo_url = ?1", args![d.repo_url, commit]).await?;
    }
    for bot_id in bots {
        touch_bot(app, &bot_id).await?;
        app.push_bot(&bot_id).await?;
    }
    Ok(updated)
}

pub async fn reload(req: &Request, app: &App, bot_id: &str, pid: &str) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let bot = app.owned_bot(&user, bot_id).await?;
    App::require_service(&user)?;
    let plugin = plugin_by_id(app, &bot.id, pid).await?;
    if bot.desired_running == 0 || plugin.enabled == 0 {
        return Err(ApiError::conflict("plugin_reload_unavailable", "启动小鱼并启用插件后才能重载"));
    }
    let client = app.node_client(&bot.node_id).await?;
    let path = format!("/v1/bots/{}/plugins/{}/reload", bot.id, plugin.id);
    match client.call(Method::Post, &path, None, PUSH_TIMEOUT_MS).await {
        Ok(_) => Ok(Reply::json(ok_json())),
        Err(NodeError::Status(409, message)) => Err(ApiError::conflict("plugin_reload_unavailable", message)),
        Err(NodeError::Unreachable(_)) => Err(ApiError::node_unreachable()),
        Err(error) => Err(ApiError::new(502, "node_error", error.detail())),
    }
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
