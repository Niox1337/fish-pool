//! Request context: env access, sessions, node selection and desired state pushes.

use crate::crypto::{self, decode_key32};
use crate::db::Db;
use crate::error::{ApiError, ApiResult};
use crate::models::*;
use crate::node::{NodeClient, NodeError};
use crate::util::{log_error, now_ms};
use crate::args;
use serde_json::{json, Value};
use std::cell::Cell;
use worker::{Env, Method, Request};

pub const SESSION_COOKIE: &str = "fish_session";
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
pub const PUSH_TIMEOUT_MS: u64 = 8_000;

thread_local! {
    static NODE_SEEDED: Cell<bool> = const { Cell::new(false) };
}

pub struct App {
    pub env: Env,
    pub db: Db,
    pub dev: bool,
}

pub const BOT_COLS: &str = "id, user_id, name, avatar, node_id, status, status_detail, desired_running, needs_push, created_at, updated_at, settings_json";
/// status_detail of bots stopped because the service period ended.
pub const SERVICE_EXPIRED_DETAIL: &str = "服务已到期，兑换新的邀请码后可以重新启动";

pub const USER_COLS: &str = "id, email, password_hash, role, created_at, quota_memory_mb, quota_storage_mb, quota_bots, quota_plugins, last_login_at, service_until";
pub const PLATFORM_COLS: &str = "id, bot_id, kind, display_name, app_id, secret_enc, extra_json, enabled, status, status_detail, created_at";
pub const PLUGIN_COLS: &str = "id, bot_id, repo_url, name, display_name, version, author, description, enabled, config_json, schema_json, status, status_detail, install_rev, created_at, updated_at";
pub const KEY_COLS: &str = "id, user_id, name, base_url, model, api_key_enc, created_at";

/// Reads a cookie value from a `Cookie` header.
pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

pub fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}", SESSION_TTL_MS / 1000)
}

pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

impl App {
    pub fn new(env: Env) -> worker::Result<Self> {
        let db = Db(env.d1("DB")?);
        let mut app = Self { env, db, dev: false };
        app.dev = app.var("FISH_DEV").map(|v| v == "1" || v == "true").unwrap_or(false);
        Ok(app)
    }

    /// Reads a plain var or secret as a string.
    pub fn var(&self, name: &str) -> Option<String> {
        js_sys::Reflect::get(&self.env, &name.into()).ok().and_then(|v| v.as_string())
    }

    pub fn data_key(&self) -> ApiResult<[u8; 32]> {
        self.var("DATA_KEY")
            .and_then(|k| decode_key32(&k))
            .ok_or_else(|| ApiError::internal("DATA_KEY missing or not base64 32 bytes"))
    }

    pub fn encrypt(&self, plaintext: &str) -> ApiResult<String> {
        Ok(crypto::encrypt(&self.data_key()?, plaintext))
    }

    pub fn decryptor(&self) -> ApiResult<impl Fn(&str) -> Option<String>> {
        let key = self.data_key()?;
        Ok(move |s: &str| crypto::decrypt(&key, s))
    }

    /// Secret of a node: `NODE_SECRET_<ID>` with non alphanumerics as `_`, else `NODE_SECRET`.
    pub fn node_secret(&self, node_id: &str) -> Option<[u8; 32]> {
        let suffix: String = node_id.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
        self.var(&format!("NODE_SECRET_{suffix}"))
            .or_else(|| self.var("NODE_SECRET"))
            .and_then(|s| decode_key32(&s))
    }

    /// AI Gateway from `AI_GATEWAY` and the `AI_GATEWAY_TOKEN` secret, when both are valid.
    pub fn ai_gateway(&self) -> Option<crate::gateway::AiGateway> {
        crate::gateway::AiGateway::new(&self.var("AI_GATEWAY")?, &self.var("AI_GATEWAY_TOKEN")?)
    }

    pub fn default_node_id(&self) -> String {
        self.var("DEFAULT_NODE_ID").filter(|s| !s.is_empty()).unwrap_or_else(|| "node-1".into())
    }

    /// Upserts the default node from `DEFAULT_NODE_URL` once per isolate.
    pub async fn ensure_default_node(&self) -> ApiResult<()> {
        if NODE_SEEDED.with(|s| s.get()) {
            return Ok(());
        }
        if let Some(url) = self.var("DEFAULT_NODE_URL").filter(|s| !s.is_empty()) {
            self.db
                .run(
                    "INSERT INTO nodes (id, base_url, region, created_at) VALUES (?1, ?2, '', ?3) \
                     ON CONFLICT(id) DO UPDATE SET base_url = excluded.base_url",
                    args![self.default_node_id(), url, now_ms()],
                )
                .await?;
        }
        NODE_SEEDED.with(|s| s.set(true));
        Ok(())
    }

    /// Picks the node with the fewest bots.
    /// shortcut: ignores node capacity columns and live load, upgrade when a second node is added.
    pub async fn assign_node(&self) -> ApiResult<String> {
        self.ensure_default_node().await?;
        #[derive(serde::Deserialize)]
        struct Pick {
            id: String,
        }
        let pick: Option<Pick> = self
            .db
            .first(
                "SELECT n.id AS id FROM nodes n LEFT JOIN bots b ON b.node_id = n.id \
                 GROUP BY n.id ORDER BY COUNT(b.id) ASC, n.created_at ASC LIMIT 1",
                &[],
            )
            .await?;
        pick.map(|p| p.id).ok_or_else(|| ApiError::new(503, "no_node", "暂无可用的运行节点，请联系管理员"))
    }

    pub async fn node_client(&self, node_id: &str) -> ApiResult<NodeClient> {
        self.ensure_default_node().await?;
        let node: NodeRow = self
            .db
            .first("SELECT id, base_url FROM nodes WHERE id = ?1", args![node_id])
            .await?
            .ok_or_else(|| ApiError::internal(format!("node {node_id} not found")))?;
        let secret = self.node_secret(node_id).ok_or_else(|| ApiError::internal(format!("secret for node {node_id} missing")))?;
        Ok(NodeClient { id: node.id, base_url: node.base_url, secret })
    }

    // ---------- sessions ----------

    pub async fn current_user(&self, req: &Request) -> ApiResult<UserRow> {
        self.optional_user(req).await?.ok_or_else(ApiError::unauthorized)
    }

    /// Current user, required to have role `admin`.
    pub async fn require_admin(&self, req: &Request) -> ApiResult<UserRow> {
        let user = self.current_user(req).await?;
        if !user.is_admin() {
            return Err(ApiError::forbidden("需要管理员权限"));
        }
        Ok(user)
    }

    /// Fails with `service_expired` unless the user has an active service period.
    pub fn require_service(user: &UserRow) -> ApiResult<()> {
        if user.service_active_at(now_ms()) {
            Ok(())
        } else {
            Err(ApiError::new(403, "service_expired", "服务已到期，请兑换新的邀请码"))
        }
    }

    pub async fn mark_node_seen(&self, node_id: &str) -> ApiResult<()> {
        self.db.run("UPDATE nodes SET last_seen_at = ?2 WHERE id = ?1", args![node_id, now_ms()]).await?;
        Ok(())
    }

    pub async fn optional_user(&self, req: &Request) -> ApiResult<Option<UserRow>> {
        let header = req.headers().get("cookie").ok().flatten().unwrap_or_default();
        let Some(token) = cookie_value(&header, SESSION_COOKIE).filter(|t| !t.is_empty()) else { return Ok(None) };
        let hash = crypto::sha256_hex(token.as_bytes());
        self.db
            .first(
                &format!(
                    "SELECT {} FROM users u JOIN sessions s ON s.user_id = u.id WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                    USER_COLS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                ),
                args![hash, now_ms()],
            )
            .await
    }

    /// Creates a session and returns the Set-Cookie value.
    pub async fn create_session(&self, user_id: &str, user_agent: &str) -> ApiResult<String> {
        let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, crate::util::random_bytes::<32>());
        let now = now_ms();
        let ua: String = user_agent.chars().take(300).collect();
        self.db
            .batch(vec![
                self.db.stmt("DELETE FROM sessions WHERE user_id = ?1 AND expires_at <= ?2", args![user_id, now])?,
                self.db.stmt(
                    "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, user_agent) VALUES (?1, ?2, ?3, ?4, ?5)",
                    args![crypto::sha256_hex(token.as_bytes()), user_id, now, now + SESSION_TTL_MS, ua],
                )?,
            ])
            .await?;
        Ok(session_cookie(&token))
    }

    // ---------- rate events ----------

    pub async fn rate_count(&self, bucket: &str, window_ms: i64) -> ApiResult<i64> {
        self.db
            .count("SELECT COUNT(*) AS n FROM rate_events WHERE bucket = ?1 AND created_at > ?2", args![bucket, now_ms() - window_ms])
            .await
    }

    pub async fn rate_hit(&self, bucket: &str) -> ApiResult<()> {
        self.db.run("INSERT INTO rate_events (bucket, created_at) VALUES (?1, ?2)", args![bucket, now_ms()]).await?;
        Ok(())
    }

    // ---------- bots ----------

    pub async fn user_by_id(&self, id: &str) -> ApiResult<UserRow> {
        self.db
            .first(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), args![id])
            .await?
            .ok_or_else(|| ApiError::not_found("用户不存在"))
    }

    pub async fn owned_bot(&self, user: &UserRow, bot_id: &str) -> ApiResult<BotRow> {
        self.db
            .first(&format!("SELECT {BOT_COLS} FROM bots WHERE id = ?1 AND user_id = ?2"), args![bot_id, user.id])
            .await?
            .ok_or_else(|| ApiError::not_found("小鱼不存在"))
    }

    pub async fn bot_by_id(&self, bot_id: &str) -> ApiResult<Option<BotRow>> {
        self.db.first(&format!("SELECT {BOT_COLS} FROM bots WHERE id = ?1"), args![bot_id]).await
    }

    pub async fn bot_platforms(&self, bot_id: &str) -> ApiResult<Vec<PlatformRow>> {
        self.db
            .all(&format!("SELECT {PLATFORM_COLS} FROM platforms WHERE bot_id = ?1 ORDER BY created_at"), args![bot_id])
            .await
    }

    pub async fn bot_plugins(&self, bot_id: &str) -> ApiResult<Vec<PluginRow>> {
        self.db
            .all(&format!("SELECT {PLUGIN_COLS} FROM plugins WHERE bot_id = ?1 ORDER BY created_at"), args![bot_id])
            .await
    }

    pub fn platform_view(&self, p: &PlatformRow) -> ApiResult<Value> {
        let dec = self.decryptor()?;
        Ok(p.view(dec(&p.secret_enc).as_deref()))
    }

    pub async fn bot_json(&self, bot_id: &str) -> ApiResult<Value> {
        let bot = self.bot_by_id(bot_id).await?.ok_or_else(|| ApiError::not_found("小鱼不存在"))?;
        let platforms = self.bot_platforms(bot_id).await?;
        let plugins = self.bot_plugins(bot_id).await?;
        let views = platforms.iter().map(|p| self.platform_view(p)).collect::<ApiResult<Vec<_>>>()?;
        Ok(bot_view(&bot, views, &plugins))
    }

    pub async fn desired_for(&self, bot: &BotRow) -> ApiResult<DesiredBot> {
        let user = self.user_by_id(&bot.user_id).await?;
        let platforms = self.bot_platforms(&bot.id).await?;
        let plugins = self.bot_plugins(&bot.id).await?;
        let settings = crate::settings::BotSettings::from_stored(&bot.settings_json);
        let key: Option<LlmKeyRow> = match settings.llm.key_id.as_deref() {
            Some(kid) => self.db.first(&format!("SELECT {KEY_COLS} FROM llm_keys WHERE id = ?1 AND user_id = ?2"), args![kid, user.id]).await?,
            None => None,
        };
        let dec = self.decryptor()?;
        let mut desired = build_desired(&user, bot, &platforms, &plugins, key.as_ref(), &dec);
        if let (Some(llm), Some(gateway)) = (desired.llm.as_mut(), self.ai_gateway()) {
            gateway.apply(llm);
        }
        Ok(desired)
    }

    /// Pushes the desired state of a bot. D1 records the outcome: on failure the bot is marked
    /// `error` with `needs_push = 1`, on success `needs_push` is cleared. Returns whether it succeeded.
    pub async fn push_bot(&self, bot_id: &str) -> ApiResult<bool> {
        let Some(bot) = self.bot_by_id(bot_id).await? else { return Ok(false) };
        let desired = self.desired_for(&bot).await?;
        let client = self.node_client(&bot.node_id).await?;
        let body = serde_json::to_value(&desired)?;
        let now = now_ms();
        match client.call(Method::Put, &format!("/v1/bots/{}", bot.id), Some(&body), PUSH_TIMEOUT_MS).await {
            Ok(_) => {
                // A bot that was in error because of a failed push returns to its intended state.
                let fallback = if desired.running { "starting" } else { "stopped" };
                self.db
                    .run(
                        "UPDATE bots SET needs_push = 0, \
                         status = CASE WHEN needs_push = 1 OR status = 'error' THEN ?2 ELSE status END, \
                         status_detail = CASE WHEN needs_push = 1 OR status = 'error' THEN '' ELSE status_detail END \
                         WHERE id = ?1",
                        args![bot.id, fallback],
                    )
                    .await?;
                Ok(true)
            }
            Err(e) => {
                log_error(&format!("push bot {} to {} failed: {e:?}", bot.id, client.id));
                self.db
                    .run(
                        "UPDATE bots SET needs_push = 1, status = 'error', status_detail = ?2, updated_at = ?3 WHERE id = ?1",
                        args![bot.id, e.detail(), now],
                    )
                    .await?;
                Ok(false)
            }
        }
    }

    /// Pushes every bot with a pending desired state, optionally limited to one node.
    pub async fn repush_pending(&self, node_id: Option<&str>, limit: i64) -> ApiResult<usize> {
        #[derive(serde::Deserialize)]
        struct Id {
            id: String,
        }
        let ids: Vec<Id> = match node_id {
            Some(n) => self.db.all("SELECT id FROM bots WHERE needs_push = 1 AND node_id = ?1 LIMIT ?2", args![n, limit]).await?,
            None => self.db.all("SELECT id FROM bots WHERE needs_push = 1 LIMIT ?1", args![limit]).await?,
        };
        let mut ok = 0;
        for id in ids {
            if self.push_bot(&id.id).await.unwrap_or(false) {
                ok += 1;
            }
        }
        Ok(ok)
    }

    /// Removes a deleted bot from its node, leaving a tombstone for later retries on failure.
    pub async fn delete_on_node(&self, bot_id: &str, node_id: &str) -> ApiResult<bool> {
        let client = self.node_client(node_id).await?;
        match client.call(Method::Delete, &format!("/v1/bots/{bot_id}"), None, PUSH_TIMEOUT_MS).await {
            Ok(_) => {
                self.db.run("DELETE FROM node_tombstones WHERE bot_id = ?1", args![bot_id]).await?;
                Ok(true)
            }
            Err(e) if e.is_not_found() => {
                self.db.run("DELETE FROM node_tombstones WHERE bot_id = ?1", args![bot_id]).await?;
                Ok(true)
            }
            Err(e) => {
                log_error(&format!("delete bot {bot_id} on {node_id} failed: {e:?}"));
                self.db
                    .run(
                        "INSERT INTO node_tombstones (bot_id, node_id, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(bot_id) DO NOTHING",
                        args![bot_id, node_id, now_ms()],
                    )
                    .await?;
                Ok(false)
            }
        }
    }

    /// Refreshes live status from the node into D1. Pushes first when the node lacks the bot
    /// or a push is pending.
    pub async fn refresh_status(&self, bot: &BotRow) -> ApiResult<()> {
        let client = self.node_client(&bot.node_id).await?;
        let path = format!("/v1/bots/{}/status", bot.id);
        let mut res = client.call(Method::Get, &path, None, 4_000).await;
        let missing = matches!(&res, Err(e) if e.is_not_found());
        if missing || (bot.needs_push == 1 && res.is_ok()) {
            if self.push_bot(&bot.id).await? {
                res = client.call(Method::Get, &path, None, 4_000).await;
            } else {
                return Ok(());
            }
        }
        match res {
            Ok(status) => self.apply_node_status(&bot.id, &status).await,
            Err(NodeError::Unreachable(_)) => {
                self.db
                    .run(
                        "UPDATE bots SET status = 'error', status_detail = ?2 WHERE id = ?1",
                        args![bot.id, NodeError::Unreachable(String::new()).detail()],
                    )
                    .await?;
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    /// Writes a node status report (`GET /v1/bots/:id/status` shape) into D1.
    pub async fn apply_node_status(&self, bot_id: &str, status: &Value) -> ApiResult<()> {
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut stmts = Vec::new();
        let bot_status = s(status, "status");
        if valid_bot_status(&bot_status) {
            stmts.push(self.db.stmt(
                &format!("UPDATE bots SET status = ?2, status_detail = {} WHERE id = ?1 AND needs_push = 0", keep_expiry_detail("?2", "?3")),
                args![bot_id, bot_status, s(status, "status_detail")],
            )?);
        }
        for p in status.get("platforms").and_then(Value::as_array).into_iter().flatten() {
            let st = s(p, "status");
            if valid_platform_status(&st) {
                stmts.push(self.db.stmt(
                    "UPDATE platforms SET status = ?3, status_detail = ?4 WHERE id = ?1 AND bot_id = ?2",
                    args![s(p, "id"), bot_id, st, s(p, "status_detail")],
                )?);
            }
        }
        for p in status.get("plugins").and_then(Value::as_array).into_iter().flatten() {
            let st = s(p, "status");
            if valid_plugin_status(&st) {
                let version = p.get("version").and_then(Value::as_str).map(str::to_string);
                stmts.push(self.db.stmt(
                    "UPDATE plugins SET status = ?3, status_detail = ?4, version = COALESCE(?5, version) WHERE id = ?1 AND bot_id = ?2",
                    args![s(p, "id"), bot_id, st, s(p, "status_detail"), version],
                )?);
            }
        }
        self.db.batch(stmts).await.map(|_| ())
    }
}

pub fn valid_bot_status(s: &str) -> bool {
    matches!(s, "starting" | "running" | "stopped" | "error")
}
pub fn valid_platform_status(s: &str) -> bool {
    matches!(s, "connected" | "connecting" | "error" | "disabled")
}
pub fn valid_plugin_status(s: &str) -> bool {
    matches!(s, "installing" | "running" | "stopped" | "error" | "over_quota")
}

/// SQL expression for a new bot status_detail that keeps the expiry note while the bot
/// stays stopped. `status` and `detail` are the bind placeholders of the reported values.
pub fn keep_expiry_detail(status: &str, detail: &str) -> String {
    format!("CASE WHEN {status} = 'stopped' AND desired_running = 0 AND status_detail = '{SERVICE_EXPIRED_DETAIL}' THEN status_detail ELSE {detail} END")
}

pub fn ok_json() -> Value {
    json!({"ok": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies() {
        assert_eq!(cookie_value("a=1; fish_session=tok; b=2", "fish_session").as_deref(), Some("tok"));
        assert_eq!(cookie_value("fish_session_x=1", "fish_session"), None);
        assert_eq!(cookie_value("", "fish_session"), None);
        let c = session_cookie("t");
        for part in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/", "Max-Age=2592000"] {
            assert!(c.contains(part));
        }
    }
}
