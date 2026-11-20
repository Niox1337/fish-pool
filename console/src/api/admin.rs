//! Admin API: overview, users, quotas, service periods and invite codes.

use super::bots::{bots_for_user, proxy_logs};
use super::internal::stop_expired;
use super::usage::usage_for;
use super::{json_body, Reply};
use crate::app::{ok_json, App, BOT_COLS};
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::invite::{self, InviteRow, DAY_MS, INVITE_COLS};
use crate::models::{admin_user_view, AccountQuota, BotRow, UserCounts, UserRow, UNLIMITED_COUNT, USER_COUNTS_SQL};
use crate::util::{now_ms, log_error};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use wasm_bindgen::JsValue;
use worker::{Method, Request};

const PAGE: i64 = 50;
const PROBE_TIMEOUT_MS: u64 = 5_000;

fn query_param(req: &Request, key: &str) -> ApiResult<Option<String>> {
    let url = req.url()?;
    Ok(url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty()))
}

/// Escapes `%`, `_` and `\` for `LIKE ... ESCAPE '\'`.
pub fn like_pattern(q: &str) -> String {
    let mut out = String::from("%");
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

// ---------- node probes ----------

#[derive(Deserialize)]
struct NodeInfo {
    id: String,
    base_url: String,
    last_seen_at: Option<i64>,
}

struct Probe {
    node: NodeInfo,
    health: Result<Value, String>,
    summary: Option<Value>,
}

async fn probe(app: &App, node: NodeInfo, with_health: bool) -> Probe {
    let client = match app.node_client(&node.id).await {
        Ok(c) => c,
        Err(e) => return Probe { node, health: Err(e.message), summary: None },
    };
    let health = if with_health {
        client.call(Method::Get, "/v1/health", None, PROBE_TIMEOUT_MS).await.map_err(|e| e.detail())
    } else {
        Ok(Value::Null)
    };
    let summary = match &health {
        Ok(_) => match client.call(Method::Get, "/v1/usage/summary", None, PROBE_TIMEOUT_MS).await {
            Ok(v) => Some(v),
            Err(e) => {
                log_error(&format!("usage summary of {}: {e:?}", node.id));
                None
            }
        },
        Err(_) => None,
    };
    Probe { node, health, summary }
}

/// Probes every node in parallel and records the contact time of the ones that answered.
async fn probe_nodes(app: &App, with_health: bool) -> ApiResult<Vec<Probe>> {
    app.ensure_default_node().await?;
    let nodes: Vec<NodeInfo> = app.db.all("SELECT id, base_url, last_seen_at FROM nodes ORDER BY id", &[]).await?;
    let mut probes = join_all(nodes.into_iter().map(|n| probe(app, n, with_health))).await;
    let now = now_ms();
    for p in probes.iter_mut() {
        if p.summary.is_some() || (with_health && p.health.is_ok()) {
            app.mark_node_seen(&p.node.id).await?;
            p.node.last_seen_at = Some(now);
        }
    }
    Ok(probes)
}

fn num(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

#[derive(Deserialize, Serialize)]
struct PluginCpu {
    user_id: String,
    bot_id: String,
    bot_name: String,
    plugin_id: String,
    label: String,
    cpu_percent: f64,
}

#[derive(Deserialize)]
struct PluginCpuSample {
    percent: f64,
    sampled_at: i64,
    plugins: Vec<PluginCpu>,
}

#[derive(Serialize)]
struct AdminPluginCpu {
    node_id: String,
    sampled_at: i64,
    #[serde(flatten)]
    plugin: PluginCpu,
}

fn plugin_cpu_overview(probes: &[Probe]) -> Value {
    let mut percent = 0.0;
    let mut reporting_nodes = 0;
    let mut items = Vec::new();
    for p in probes {
        let Some(sample) = p.summary.as_ref().and_then(|s| s.get("plugin_cpu"))
            .and_then(|v| PluginCpuSample::deserialize(v).ok()) else { continue; };
        reporting_nodes += 1;
        percent += sample.percent;
        items.extend(sample.plugins.into_iter().map(|plugin| AdminPluginCpu {
            node_id: p.node.id.clone(),
            sampled_at: sample.sampled_at,
            plugin,
        }));
    }
    items.sort_unstable_by(|a, b| b.plugin.cpu_percent.total_cmp(&a.plugin.cpu_percent)
        .then_with(|| a.plugin.plugin_id.cmp(&b.plugin.plugin_id)));
    items.truncate(20);
    json!({ "percent": (reporting_nodes > 0).then_some(percent), "reporting_nodes": reporting_nodes, "items": items })
}

/// Per user (memory, storage) from node summaries. A user is known only when every node
/// hosting one of their bots answered.
fn user_usage(probes: &[Probe], user_nodes: &HashMap<String, HashSet<String>>) -> HashMap<String, (i64, i64)> {
    let mut per_node: HashMap<&str, HashMap<String, (i64, i64)>> = HashMap::new();
    for p in probes {
        if let Some(s) = &p.summary {
            let map = per_node.entry(p.node.id.as_str()).or_default();
            for u in s.get("users").and_then(Value::as_array).into_iter().flatten() {
                let id = u.get("user_id").and_then(Value::as_str).unwrap_or("").to_string();
                let e = map.entry(id).or_insert((0, 0));
                e.0 += num(u, "memory_bytes");
                e.1 += num(u, "storage_bytes");
            }
        }
    }
    let mut out = HashMap::new();
    for (user, nodes) in user_nodes {
        let mut total = (0, 0);
        let mut known = true;
        for n in nodes {
            match per_node.get(n.as_str()) {
                Some(m) => {
                    let (a, b) = m.get(user).copied().unwrap_or((0, 0));
                    total.0 += a;
                    total.1 += b;
                }
                None => known = false,
            }
        }
        if known {
            out.insert(user.clone(), total);
        }
    }
    out
}

async fn user_nodes(app: &App, user_ids: &[String]) -> ApiResult<HashMap<String, HashSet<String>>> {
    let mut out: HashMap<String, HashSet<String>> = user_ids.iter().map(|u| (u.clone(), HashSet::new())).collect();
    if user_ids.is_empty() {
        return Ok(out);
    }
    #[derive(Deserialize)]
    struct Row {
        user_id: String,
        node_id: String,
    }
    let marks = vec!["?"; user_ids.len()].join(", ");
    let binds: Vec<JsValue> = user_ids.iter().map(|u| JsValue::from_str(u)).collect();
    let rows: Vec<Row> = app.db.all(&format!("SELECT DISTINCT user_id, node_id FROM bots WHERE user_id IN ({marks})"), &binds).await?;
    for r in rows {
        out.entry(r.user_id).or_default().insert(r.node_id);
    }
    Ok(out)
}

// ---------- overview ----------

#[derive(Deserialize)]
struct Totals {
    users_total: i64,
    users_new_7d: i64,
    users_active: i64,
    users_expiring_7d: i64,
    users_expired: i64,
    quota_memory_mb: i64,
    quota_storage_mb: i64,
    bots_total: i64,
    bots_running: i64,
    bots_starting: i64,
    bots_stopped: i64,
    bots_error: i64,
    plugins_total: i64,
    plugins_running: i64,
    plugins_sleeping: i64,
    plugins_installing: i64,
    plugins_error: i64,
    plugins_over_quota: i64,
    platforms_qq: i64,
    platforms_weixin: i64,
    platforms_error: i64,
    invites_unused: i64,
    invites_used_7d: i64,
}

const TOTALS_SQL: &str = "SELECT \
(SELECT COUNT(*) FROM users) AS users_total, \
(SELECT COUNT(*) FROM users WHERE created_at > ?1 - 7 * ?2) AS users_new_7d, \
(SELECT COUNT(*) FROM users WHERE role = 'admin' OR service_until > ?1) AS users_active, \
(SELECT COUNT(*) FROM users WHERE role != 'admin' AND service_until > ?1 AND service_until <= ?1 + 7 * ?2) AS users_expiring_7d, \
(SELECT COUNT(*) FROM users WHERE role != 'admin' AND (service_until IS NULL OR service_until <= ?1)) AS users_expired, \
(SELECT COALESCE(SUM(quota_memory_mb), 0) FROM users) AS quota_memory_mb, \
(SELECT COALESCE(SUM(quota_storage_mb), 0) FROM users) AS quota_storage_mb, \
(SELECT COUNT(*) FROM bots) AS bots_total, \
(SELECT COUNT(*) FROM bots WHERE status = 'running') AS bots_running, \
(SELECT COUNT(*) FROM bots WHERE status = 'starting') AS bots_starting, \
(SELECT COUNT(*) FROM bots WHERE status = 'stopped') AS bots_stopped, \
(SELECT COUNT(*) FROM bots WHERE status = 'error') AS bots_error, \
(SELECT COUNT(*) FROM plugins) AS plugins_total, \
(SELECT COUNT(*) FROM plugins WHERE status = 'running') AS plugins_running, \
(SELECT COUNT(*) FROM plugins WHERE status = 'sleeping') AS plugins_sleeping, \
(SELECT COUNT(*) FROM plugins WHERE status = 'installing') AS plugins_installing, \
(SELECT COUNT(*) FROM plugins WHERE status = 'error') AS plugins_error, \
(SELECT COUNT(*) FROM plugins WHERE status = 'over_quota') AS plugins_over_quota, \
(SELECT COUNT(*) FROM platforms WHERE kind = 'qq_official') AS platforms_qq, \
(SELECT COUNT(*) FROM platforms WHERE kind = 'weixin_ilink') AS platforms_weixin, \
(SELECT COUNT(*) FROM platforms WHERE enabled = 1 AND status = 'error') AS platforms_error, \
(SELECT COUNT(*) FROM invite_codes WHERE revoked_at IS NULL AND used_count < max_uses AND (expires_at IS NULL OR expires_at > ?1)) AS invites_unused, \
(SELECT COUNT(*) FROM invite_redemptions WHERE redeemed_at > ?1 - 7 * ?2) AS invites_used_7d";

const PROBLEMS_SQL: &str = "SELECT * FROM (\
SELECT b.user_id, u.email AS user_email, b.id AS bot_id, b.name AS bot_name, 'bot' AS kind, b.id AS id, b.name AS label, \
b.status, b.status_detail, b.updated_at FROM bots b JOIN users u ON u.id = b.user_id WHERE b.status IN ('error', 'over_quota') \
UNION ALL \
SELECT b.user_id, u.email, b.id, b.name, 'platform', p.id, p.display_name, p.status, p.status_detail, b.updated_at \
FROM platforms p JOIN bots b ON b.id = p.bot_id JOIN users u ON u.id = b.user_id WHERE p.enabled = 1 AND p.status IN ('error', 'over_quota') \
UNION ALL \
SELECT b.user_id, u.email, b.id, b.name, 'plugin', p.id, p.display_name, p.status, p.status_detail, p.updated_at \
FROM plugins p JOIN bots b ON b.id = p.bot_id JOIN users u ON u.id = b.user_id WHERE p.status IN ('error', 'over_quota')\
) ORDER BY updated_at DESC LIMIT 50";

#[derive(Deserialize, serde::Serialize)]
struct Problem {
    user_id: String,
    user_email: String,
    bot_id: String,
    bot_name: String,
    kind: String,
    id: String,
    label: String,
    status: String,
    status_detail: String,
    updated_at: i64,
}

pub async fn overview(req: &Request, app: &App) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let now = now_ms();
    let t: Totals = app.db.first(TOTALS_SQL, args![now, DAY_MS]).await?.ok_or_else(|| ApiError::internal("totals query returned no row"))?;
    let problems: Vec<Problem> = app.db.all(PROBLEMS_SQL, &[]).await?;
    let probes = probe_nodes(app, true).await?;
    let (mut memory, mut storage) = (0i64, 0i64);
    let mut nodes = Vec::new();
    for p in &probes {
        if let Some(s) = &p.summary {
            memory += num(s, "memory_bytes");
            storage += num(s, "storage_bytes");
        }
        let mut n = Map::new();
        n.insert("id".into(), json!(p.node.id));
        n.insert("base_url".into(), json!(p.node.base_url));
        n.insert("last_seen".into(), json!(p.node.last_seen_at));
        match &p.health {
            Ok(h) => {
                n.insert("online".into(), json!(true));
                n.insert("version".into(), h.get("version").cloned().unwrap_or(Value::Null));
                n.insert("memory_total_bytes".into(), h.get("memory_total_bytes").cloned().unwrap_or(Value::Null));
                n.insert("memory_used_bytes".into(), h.get("memory_used_bytes").cloned().unwrap_or(Value::Null));
                n.insert("bots".into(), h.get("bots").cloned().unwrap_or(json!(0)));
            }
            Err(e) => {
                n.insert("online".into(), json!(false));
                n.insert("version".into(), Value::Null);
                n.insert("memory_total_bytes".into(), Value::Null);
                n.insert("memory_used_bytes".into(), Value::Null);
                n.insert("bots".into(), Value::Null);
                n.insert("error".into(), json!(e));
            }
        }
        nodes.push(Value::Object(n));
    }
    Ok(Reply::json(json!({
        "users": {
            "total": t.users_total, "new_7d": t.users_new_7d, "active": t.users_active,
            "expiring_7d": t.users_expiring_7d, "expired": t.users_expired,
        },
        "bots": {"total": t.bots_total, "running": t.bots_running, "starting": t.bots_starting, "stopped": t.bots_stopped, "error": t.bots_error},
        "plugins": {
            "total": t.plugins_total, "running": t.plugins_running, "sleeping": t.plugins_sleeping, "installing": t.plugins_installing,
            "error": t.plugins_error, "over_quota": t.plugins_over_quota,
        },
        "platforms": {"qq_official": t.platforms_qq, "weixin_ilink": t.platforms_weixin, "error": t.platforms_error},
        "usage": {"memory_bytes": memory, "storage_bytes": storage, "quota_memory_mb": t.quota_memory_mb, "quota_storage_mb": t.quota_storage_mb},
        "plugin_cpu": plugin_cpu_overview(&probes),
        "invites": {"unused": t.invites_unused, "used_7d": t.invites_used_7d},
        "nodes": nodes,
        "problems": problems,
    })))
}

// ---------- system ----------

const SYSTEM_TIMEOUT_MS: u64 = 8_000;

/// Row counts of every D1 table plus a few derived counters, read in one query.
const D1_SQL: &str = "SELECT \
(SELECT COUNT(*) FROM users) AS users, \
(SELECT COUNT(*) FROM users WHERE role = 'admin') AS users_admin, \
(SELECT COUNT(*) FROM users WHERE last_login_at > ?1 - ?2) AS users_login_24h, \
(SELECT COUNT(*) FROM sessions) AS sessions, \
(SELECT COUNT(*) FROM sessions WHERE expires_at > ?1) AS sessions_active, \
(SELECT COUNT(*) FROM bots) AS bots, \
(SELECT COUNT(*) FROM bots WHERE desired_running = 1) AS bots_desired_running, \
(SELECT COUNT(*) FROM bots WHERE needs_push = 1) AS bots_needs_push, \
(SELECT COUNT(*) FROM platforms) AS platforms, \
(SELECT COUNT(*) FROM plugins) AS plugins, \
(SELECT COUNT(*) FROM llm_keys) AS llm_keys, \
(SELECT COUNT(*) FROM invite_codes) AS invite_codes, \
(SELECT COUNT(*) FROM invite_redemptions) AS invite_redemptions, \
(SELECT COUNT(*) FROM email_codes) AS email_codes, \
(SELECT COUNT(*) FROM captchas) AS captchas, \
(SELECT COUNT(*) FROM rate_events) AS rate_events, \
(SELECT COUNT(*) FROM qr_tasks) AS qr_tasks, \
(SELECT COUNT(*) FROM qr_tasks WHERE expires_at > ?1) AS qr_tasks_active, \
(SELECT COUNT(*) FROM node_tombstones) AS node_tombstones, \
(SELECT COUNT(*) FROM nodes) AS nodes";

/// Tables reported by [`D1_SQL`], in display order.
pub const D1_TABLES: [&str; 14] = [
    "users", "sessions", "bots", "platforms", "plugins", "llm_keys", "invite_codes", "invite_redemptions",
    "email_codes", "captchas", "rate_events", "qr_tasks", "node_tombstones", "nodes",
];

/// Shapes the D1 counter row into table rows and named counters.
pub fn d1_view(row: &Map<String, Value>, duration_ms: Option<f64>, size_bytes: Option<usize>, round_trip_ms: i64) -> Value {
    let n = |k: &str| row.get(k).and_then(Value::as_i64).unwrap_or(0);
    let tables: Vec<Value> = D1_TABLES.iter().map(|t| json!({"name": t, "rows": n(t)})).collect();
    json!({
        "size_bytes": size_bytes,
        "query_ms": duration_ms,
        "round_trip_ms": round_trip_ms,
        "tables": tables,
        "users_admin": n("users_admin"),
        "users_login_24h": n("users_login_24h"),
        "sessions_active": n("sessions_active"),
        "bots_desired_running": n("bots_desired_running"),
        "bots_needs_push": n("bots_needs_push"),
        "qr_tasks_active": n("qr_tasks_active"),
    })
}

/// User ids a node system report refers to, sorted and unique.
pub fn system_user_ids(system: &Value) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut take = |v: Option<&Value>| {
        if let Some(s) = v.and_then(Value::as_str).filter(|s| !s.is_empty()) {
            ids.push(s.to_string());
        }
    };
    for t in system.get("tenants").and_then(Value::as_array).into_iter().flatten() {
        take(t.get("user_id"));
    }
    for p in system.get("processes").and_then(Value::as_array).into_iter().flatten() {
        take(p.get("plugin").and_then(|x| x.get("user_id")));
    }
    for b in system.pointer("/logs/bots").and_then(Value::as_array).into_iter().flatten() {
        take(b.get("user_id"));
    }
    ids.sort();
    ids.dedup();
    ids
}

pub async fn system(req: &Request, app: &App) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let now = now_ms();
    let (rows, meta): (Vec<Map<String, Value>>, _) = app.db.all_meta(D1_SQL, args![now, DAY_MS]).await?;
    let d1_rtt = now_ms() - now;
    let row = rows.into_iter().next().unwrap_or_default();
    let d1 = d1_view(&row, meta.as_ref().and_then(|m| m.duration), meta.as_ref().and_then(|m| m.size_after), d1_rtt);

    app.ensure_default_node().await?;
    let nodes: Vec<NodeInfo> = app.db.all("SELECT id, base_url, last_seen_at FROM nodes ORDER BY id", &[]).await?;
    let results = join_all(nodes.into_iter().map(|node| async move {
        let started = now_ms();
        let res = match app.node_client(&node.id).await {
            Ok(c) => c.call(Method::Get, "/v1/system", None, SYSTEM_TIMEOUT_MS).await.map_err(|e| e.detail()),
            Err(e) => Err(e.message),
        };
        (node, res, now_ms() - started)
    }))
    .await;
    let mut user_ids = Vec::new();
    let mut out = Vec::new();
    for (mut node, res, latency) in results {
        if res.is_ok() {
            app.mark_node_seen(&node.id).await?;
            node.last_seen_at = Some(now_ms());
        }
        let mut n = Map::new();
        n.insert("id".into(), json!(node.id));
        n.insert("base_url".into(), json!(node.base_url));
        n.insert("last_seen".into(), json!(node.last_seen_at));
        n.insert("latency_ms".into(), json!(latency));
        match res {
            Ok(v) => {
                user_ids.extend(system_user_ids(&v));
                n.insert("online".into(), json!(true));
                n.insert("system".into(), v);
            }
            Err(e) => {
                n.insert("online".into(), json!(false));
                n.insert("system".into(), Value::Null);
                n.insert("error".into(), json!(e));
            }
        }
        out.push(Value::Object(n));
    }
    user_ids.sort();
    user_ids.dedup();
    let mut users = Map::new();
    if !user_ids.is_empty() {
        #[derive(Deserialize)]
        struct U {
            id: String,
            email: String,
        }
        let marks = vec!["?"; user_ids.len()].join(", ");
        let binds: Vec<JsValue> = user_ids.iter().map(|u| JsValue::from_str(u)).collect();
        let found: Vec<U> = app.db.all(&format!("SELECT id, email FROM users WHERE id IN ({marks})"), &binds).await?;
        for u in found {
            users.insert(u.id, json!(u.email));
        }
    }
    let colo = req.cf().map(|cf| cf.colo());
    Ok(Reply::json(json!({
        "generated_at": now_ms(),
        "console": {"colo": colo, "d1": d1},
        "nodes": out,
        "users": users,
    })))
}

// ---------- users ----------

/// UserRow columns plus [`UserCounts`], read in one query.
#[derive(Deserialize)]
struct AdminUserRow {
    id: String,
    email: String,
    password_hash: String,
    role: String,
    created_at: i64,
    quota_memory_mb: i64,
    quota_storage_mb: i64,
    quota_bots: i64,
    quota_plugins: i64,
    last_login_at: Option<i64>,
    service_until: Option<i64>,
    bot_count: i64,
    plugin_count: i64,
    running_count: i64,
    error_count: i64,
}

impl AdminUserRow {
    fn split(self) -> (UserRow, UserCounts) {
        let counts = UserCounts {
            bot_count: self.bot_count,
            plugin_count: self.plugin_count,
            running_count: self.running_count,
            error_count: self.error_count,
        };
        let user = UserRow {
            id: self.id,
            email: self.email,
            password_hash: self.password_hash,
            role: self.role,
            created_at: self.created_at,
            quota_memory_mb: self.quota_memory_mb,
            quota_storage_mb: self.quota_storage_mb,
            quota_bots: self.quota_bots,
            quota_plugins: self.quota_plugins,
            last_login_at: self.last_login_at,
            service_until: self.service_until,
        };
        (user, counts)
    }
}

fn admin_user_select() -> String {
    let cols = crate::app::USER_COLS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ");
    format!("SELECT {cols}, {USER_COUNTS_SQL} FROM users u")
}

async fn admin_users(app: &App, rows: Vec<AdminUserRow>) -> ApiResult<Vec<Value>> {
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let nodes = user_nodes(app, &ids).await?;
    let needs_probe = nodes.values().any(|n| !n.is_empty());
    let usage = if needs_probe { user_usage(&probe_nodes(app, false).await?, &nodes) } else { HashMap::new() };
    Ok(rows
        .into_iter()
        .map(|r| {
            let (user, counts) = r.split();
            let u = if nodes.get(&user.id).is_some_and(|n| n.is_empty()) { Some((0, 0)) } else { usage.get(&user.id).copied() };
            admin_user_view(&user, &counts, u)
        })
        .collect())
}

async fn admin_user(app: &App, id: &str) -> ApiResult<Value> {
    let row: AdminUserRow = app
        .db
        .first(&format!("{} WHERE u.id = ?1", admin_user_select()), args![id])
        .await?
        .ok_or_else(|| ApiError::not_found("用户不存在"))?;
    Ok(admin_users(app, vec![row]).await?.remove(0))
}

pub async fn users(req: &Request, app: &App) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let q = query_param(req, "q")?.map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
    let cursor = query_param(req, "cursor")?;
    let sql = format!(
        "{} WHERE (?1 IS NULL OR u.email LIKE ?1 ESCAPE '\\') AND (?2 IS NULL OR u.id < ?2) ORDER BY u.id DESC LIMIT ?3",
        admin_user_select()
    );
    let mut rows: Vec<AdminUserRow> = app.db.all(&sql, args![q.as_deref().map(like_pattern), cursor, PAGE + 1]).await?;
    let next = if rows.len() as i64 > PAGE {
        rows.truncate(PAGE as usize);
        rows.last().map(|r| r.id.clone())
    } else {
        None
    };
    Ok(Reply::json(json!({"items": admin_users(app, rows).await?, "cursor": next})))
}

pub async fn user(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let user = admin_user(app, id).await?;
    Ok(Reply::json(json!({"user": user, "bots": bots_for_user(app, id).await?})))
}

pub async fn user_usage_route(req: &Request, app: &App, id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let user = app.user_by_id(id).await?;
    Ok(Reply::json(usage_for(app, &user).await?))
}

pub async fn user_bot_logs(req: &Request, app: &App, id: &str, bot_id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let bot: BotRow = app
        .db
        .first(&format!("SELECT {BOT_COLS} FROM bots WHERE id = ?1 AND user_id = ?2"), args![bot_id, id])
        .await?
        .ok_or_else(|| ApiError::not_found("机器人不存在"))?;
    proxy_logs(req, app, &bot).await
}

/// Allowed quota ranges: (field, column, min, max).
const QUOTA_FIELDS: [(&str, &str, i64, i64); 4] = [
    ("memory_mb", "quota_memory_mb", 64, 65_536),
    ("storage_mb", "quota_storage_mb", 64, 1_048_576),
    ("bots", "quota_bots", 0, 100),
    ("plugins", "quota_plugins", 0, 500),
];

/// Validated column updates from a PATCH body quota object.
pub fn quota_updates(quota: &Value) -> ApiResult<Vec<(&'static str, i64)>> {
    let obj = quota.as_object().ok_or_else(|| ApiError::bad_request("quota 必须是对象"))?;
    if let Some(k) = obj.keys().find(|k| !QUOTA_FIELDS.iter().any(|f| f.0 == k.as_str())) {
        return Err(ApiError::bad_request(format!("未知的配额字段 {k}")));
    }
    let mut out = Vec::new();
    for (field, col, min, max) in QUOTA_FIELDS {
        if let Some(v) = obj.get(field) {
            if v.is_null() && matches!(field, "bots" | "plugins") {
                out.push((col, UNLIMITED_COUNT));
                continue;
            }
            let n = v.as_i64().filter(|n| (min..=max).contains(n)).ok_or_else(|| ApiError::bad_request(format!("{field} 应为 {min} 到 {max} 的整数")))?;
            out.push((col, n));
        }
    }
    Ok(out)
}

/// New service_until after optionally setting it and then extending it by whole days.
pub fn next_service_until(current: Option<i64>, set: Option<Option<i64>>, extend_days: Option<i64>, now: i64) -> Option<i64> {
    let base = match set {
        Some(v) => v,
        None => current,
    };
    match extend_days {
        Some(d) => Some(base.unwrap_or(0).max(now) + d * DAY_MS),
        None => base,
    }
}

pub async fn patch_user(req: &mut Request, app: &App, id: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let body: Value = json_body(req).await?;
    let obj = body.as_object().ok_or_else(|| ApiError::bad_request("请求体必须是对象"))?;
    let user = app.user_by_id(id).await?;
    let quota = match obj.get("quota") {
        Some(q) => quota_updates(q)?,
        None => Vec::new(),
    };
    let set = match obj.get("service_until") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(v.as_i64().filter(|n| *n >= 0).ok_or_else(|| ApiError::bad_request("service_until 应为毫秒时间戳或 null"))?)),
    };
    let extend = match obj.get("extend_days") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_i64().filter(|n| (1..=3650).contains(n)).ok_or_else(|| ApiError::bad_request("extend_days 应为 1 到 3650 的整数"))?),
    };
    let now = now_ms();
    let service_changed = set.is_some() || extend.is_some();
    if !quota.is_empty() || service_changed {
        let mut sets: Vec<String> = quota.iter().map(|(c, _)| format!("{c} = ?")).collect();
        let mut binds: Vec<JsValue> = quota.iter().map(|(_, n)| JsValue::from_f64(*n as f64)).collect();
        if service_changed {
            sets.push("service_until = ?".into());
            binds.push(match next_service_until(user.service_until, set, extend, now) {
                Some(t) => JsValue::from_f64(t as f64),
                None => JsValue::NULL,
            });
        }
        binds.push(JsValue::from_str(&user.id));
        app.db.run(&format!("UPDATE users SET {} WHERE id = ?", sets.join(", ")), &binds).await?;
    }
    if !quota.is_empty() {
        #[derive(Deserialize)]
        struct Id {
            id: String,
        }
        let bots: Vec<Id> = app.db.all("SELECT id FROM bots WHERE user_id = ?1", args![user.id]).await?;
        for b in bots {
            app.push_bot(&b.id).await?;
        }
    }
    if service_changed {
        stop_expired(app, now, Some(&user.id)).await?;
    }
    Ok(Reply::json(admin_user(app, &user.id).await?))
}

// ---------- invites ----------

#[derive(Deserialize)]
struct InviteListRow {
    code: String,
    days: i64,
    max_uses: i64,
    used_count: i64,
    note: String,
    created_by_email: Option<String>,
    created_at: i64,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
    quota_json: Option<String>,
    #[serde(default)]
    gateway_fen: i64,
}

#[derive(Deserialize)]
struct RedemptionRow {
    code: String,
    user_id: String,
    user_email: Option<String>,
    days: i64,
    redeemed_at: i64,
    gateway_fen: i64,
    gateway_key_id: Option<String>,
    gateway_error: Option<String>,
}

/// Gateway key state of a redemption: none, pending, failed or granted.
fn gateway_state(r: &RedemptionRow) -> Option<&'static str> {
    match (r.gateway_fen > 0, &r.gateway_key_id, &r.gateway_error) {
        (false, _, _) => None,
        (true, Some(_), _) => Some("granted"),
        (true, None, Some(_)) => Some("failed"),
        (true, None, None) => Some("pending"),
    }
}

async fn invite_views(app: &App, rows: Vec<InviteListRow>) -> ApiResult<Vec<Value>> {
    let mut redemptions: HashMap<String, Vec<Value>> = HashMap::new();
    if !rows.is_empty() {
        let marks = vec!["?"; rows.len()].join(", ");
        let binds: Vec<JsValue> = rows.iter().map(|r| JsValue::from_str(&r.code)).collect();
        let found: Vec<RedemptionRow> = app
            .db
            .all(
                &format!(
                    "SELECT r.code, r.user_id, u.email AS user_email, r.days, r.redeemed_at, r.gateway_fen, r.gateway_key_id, r.gateway_error \
                     FROM invite_redemptions r \
                     LEFT JOIN users u ON u.id = r.user_id WHERE r.code IN ({marks}) ORDER BY r.redeemed_at"
                ),
                &binds,
            )
            .await?;
        for r in found {
            let gateway = gateway_state(&r);
            redemptions.entry(r.code.clone()).or_default().push(json!({
                "user_id": r.user_id, "user_email": r.user_email, "days": r.days, "redeemed_at": r.redeemed_at,
                "gateway": gateway, "gateway_error": r.gateway_error,
            }));
        }
    }
    Ok(rows
        .into_iter()
        .map(|r| {
            let red = redemptions.remove(&r.code).unwrap_or_default();
            let quota = r.quota_json.as_deref().and_then(|s| serde_json::from_str::<AccountQuota>(s).ok());
            json!({
                "code": r.code, "days": r.days, "max_uses": r.max_uses, "used_count": r.used_count, "note": r.note,
                "created_by_email": r.created_by_email, "created_at": r.created_at, "expires_at": r.expires_at,
                "revoked_at": r.revoked_at, "redemptions": red, "quota": quota,
                "gateway_budget": (r.gateway_fen > 0).then(|| crate::gengwiki::yuan(r.gateway_fen)),
            })
        })
        .collect())
}

/// SQL condition for an invite list status filter.
pub fn status_filter(status: &str) -> ApiResult<&'static str> {
    Ok(match status {
        "all" => "1 = 1",
        "unused" => "i.revoked_at IS NULL AND i.used_count < i.max_uses",
        "used" => "i.used_count > 0",
        "revoked" => "i.revoked_at IS NOT NULL",
        _ => return Err(ApiError::bad_request("status 只能是 unused、used、revoked 或 all")),
    })
}

/// Splits an invite list cursor `{created_at}.{code}`.
pub fn parse_invite_cursor(c: &str) -> ApiResult<(i64, String)> {
    let bad = || ApiError::bad_request("cursor 无效");
    let (ts, code) = c.split_once('.').ok_or_else(bad)?;
    Ok((ts.parse().map_err(|_| bad())?, code.to_string()))
}

const INVITE_SELECT: &str = "SELECT i.code, i.days, i.max_uses, i.used_count, i.note, u.email AS created_by_email, \
i.created_at, i.expires_at, i.revoked_at, i.quota_json, i.gateway_fen FROM invite_codes i LEFT JOIN users u ON u.id = i.created_by";

pub async fn invites(req: &Request, app: &App) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let status = query_param(req, "status")?.unwrap_or_else(|| "all".into());
    let filter = status_filter(&status)?;
    let cursor = query_param(req, "cursor")?.map(|c| parse_invite_cursor(&c)).transpose()?;
    let (ts, code) = match &cursor {
        Some((t, c)) => (Some(*t), Some(c.as_str())),
        None => (None, None),
    };
    let sql = format!(
        "{INVITE_SELECT} WHERE {filter} AND (?1 IS NULL OR i.created_at < ?1 OR (i.created_at = ?1 AND i.code < ?2)) \
         ORDER BY i.created_at DESC, i.code DESC LIMIT ?3"
    );
    let mut rows: Vec<InviteListRow> = app.db.all(&sql, args![ts, code, PAGE + 1]).await?;
    let next = if rows.len() as i64 > PAGE {
        rows.truncate(PAGE as usize);
        rows.last().map(|r| format!("{}.{}", r.created_at, r.code))
    } else {
        None
    };
    Ok(Reply::json(json!({"items": invite_views(app, rows).await?, "cursor": next, "default_quota": AccountQuota::default()})))
}

#[derive(Deserialize)]
struct CreateInvites {
    days: i64,
    #[serde(default = "one")]
    count: i64,
    #[serde(default = "one")]
    max_uses: i64,
    #[serde(default)]
    note: String,
    expires_at: Option<i64>,
    #[serde(default)]
    quota: AccountQuota,
    /// gengwiki gateway credit in yuan for each redemption.
    #[serde(default)]
    gateway_budget: Option<String>,
}

fn one() -> i64 {
    1
}

pub async fn create_invites(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let admin = app.require_admin(req).await?;
    let body: CreateInvites = json_body(req).await?;
    quota_updates(&serde_json::to_value(body.quota)?)?;
    let quota_json = serde_json::to_string(&body.quota)?;
    if !(1..=3650).contains(&body.days) {
        return Err(ApiError::bad_request("days 应为 1 到 3650"));
    }
    if !(1..=100).contains(&body.count) {
        return Err(ApiError::bad_request("count 应为 1 到 100"));
    }
    if !(1..=1000).contains(&body.max_uses) {
        return Err(ApiError::bad_request("max_uses 应为 1 到 1000"));
    }
    let note = body.note.trim().to_string();
    if note.chars().count() > 200 {
        return Err(ApiError::bad_request("备注最多 200 个字符"));
    }
    let now = now_ms();
    if body.expires_at.is_some_and(|t| t <= now) {
        return Err(ApiError::bad_request("expires_at 必须晚于当前时间"));
    }
    let gateway_fen = gateway_fen(body.gateway_budget.as_deref())?;
    if gateway_fen > 0 && app.gengwiki().token.is_none() {
        return Err(ApiError::bad_request("gengwiki 网关未配置，不能附带额度"));
    }
    let mut codes = Vec::new();
    for attempt in 0..3 {
        codes = (0..body.count).map(|_| invite::generate()).collect::<Vec<_>>();
        let stmts = codes
            .iter()
            .map(|c| {
                app.db.stmt(
                    "INSERT INTO invite_codes (code, days, max_uses, used_count, note, created_by, created_at, expires_at, quota_json, gateway_fen) \
                     VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9)",
                    args![c, body.days, body.max_uses, note, admin.id, now, body.expires_at, quota_json, gateway_fen],
                )
            })
            .collect::<ApiResult<Vec<_>>>()?;
        match app.db.batch(stmts).await {
            Ok(_) => break,
            Err(e) if e.code == "conflict" && attempt < 2 => continue,
            Err(e) => return Err(e),
        }
    }
    let rows: Vec<InviteListRow> = codes
        .into_iter()
        .map(|code| InviteListRow {
            code,
            days: body.days,
            max_uses: body.max_uses,
            used_count: 0,
            note: note.clone(),
            created_by_email: Some(admin.email.clone()),
            created_at: now,
            expires_at: body.expires_at,
            revoked_at: None,
            quota_json: Some(quota_json.clone()),
            gateway_fen,
        })
        .collect();
    Ok(Reply::json(json!({"items": invite_views(app, rows).await?})))
}

/// Optional gateway credit of an invite as fen, empty meaning none.
pub fn gateway_fen(budget: Option<&str>) -> ApiResult<i64> {
    let Some(b) = budget.map(str::trim).filter(|b| !b.is_empty()) else { return Ok(0) };
    crate::gengwiki::parse_fen(b)
        .filter(|f| *f <= crate::gengwiki::MAX_GIFT_FEN)
        .ok_or_else(|| ApiError::bad_request(format!("网关额度应为 0 到 {} 元，最多两位小数", crate::gengwiki::MAX_GIFT_FEN / 100)))
}

pub async fn revoke_invite(req: &Request, app: &App, code: &str) -> ApiResult<Reply> {
    app.require_admin(req).await?;
    let code = invite::normalize(code).ok_or_else(|| ApiError::not_found("邀请码不存在"))?;
    let row: Option<InviteRow> = app.db.first(&format!("SELECT {INVITE_COLS} FROM invite_codes WHERE code = ?1"), args![code]).await?;
    if row.is_none() {
        return Err(ApiError::not_found("邀请码不存在"));
    }
    app.db.run("UPDATE invite_codes SET revoked_at = COALESCE(revoked_at, ?2) WHERE code = ?1", args![code, now_ms()]).await?;
    Ok(Reply::json(ok_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_budgets() {
        assert_eq!(gateway_fen(None).unwrap(), 0);
        assert_eq!(gateway_fen(Some(" ")).unwrap(), 0);
        assert_eq!(gateway_fen(Some("5")).unwrap(), 500);
        assert_eq!(gateway_fen(Some("1000")).unwrap(), 100_000);
        assert!(gateway_fen(Some("1000.01")).is_err());
        assert!(gateway_fen(Some("-1")).is_err());
        assert!(gateway_fen(Some("0.001")).is_err());
    }

    #[test]
    fn like_escaping() {
        assert_eq!(like_pattern("a_b%c\\"), "%a\\_b\\%c\\\\%");
        assert_eq!(like_pattern("x"), "%x%");
    }

    #[test]
    fn quota_validation() {
        let u = quota_updates(&json!({"memory_mb": 512, "bots": 3})).unwrap();
        assert_eq!(u, vec![("quota_memory_mb", 512), ("quota_bots", 3)]);
        assert!(quota_updates(&json!({"memory_mb": 1})).is_err());
        assert!(quota_updates(&json!({"bots": 1.5})).is_err());
        assert!(quota_updates(&json!({"cpu": 1})).is_err());
        assert!(quota_updates(&json!(3)).is_err());
        assert!(quota_updates(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn service_until_rules() {
        let now = 1_000 * DAY_MS;
        assert_eq!(next_service_until(Some(5), None, None, now), Some(5));
        assert_eq!(next_service_until(Some(5), Some(None), None, now), None);
        assert_eq!(next_service_until(None, None, Some(2), now), Some(now + 2 * DAY_MS));
        assert_eq!(next_service_until(Some(now + DAY_MS), None, Some(2), now), Some(now + 3 * DAY_MS));
        assert_eq!(next_service_until(Some(1), None, Some(1), now), Some(now + DAY_MS));
        assert_eq!(next_service_until(Some(1), Some(Some(now + 5)), Some(1), now), Some(now + 5 + DAY_MS));
    }

    #[test]
    fn invite_filters_and_cursor() {
        assert!(status_filter("unused").is_ok());
        assert!(status_filter("bogus").is_err());
        assert_eq!(parse_invite_cursor("123.FISH-AAAA-BBBB").unwrap(), (123, "FISH-AAAA-BBBB".to_string()));
        assert!(parse_invite_cursor("x.FISH").is_err());
        assert!(parse_invite_cursor("nodot").is_err());
    }

    #[test]
    fn d1_view_lists_every_table() {
        let mut row = Map::new();
        for (i, t) in D1_TABLES.iter().enumerate() {
            row.insert(t.to_string(), json!(i));
        }
        row.insert("bots_needs_push".into(), json!(2));
        let v = d1_view(&row, Some(0.4), Some(53_248), 12);
        assert_eq!(v["tables"].as_array().unwrap().len(), D1_TABLES.len());
        assert_eq!(v["tables"][2], json!({"name": "bots", "rows": 2}));
        assert_eq!(v["bots_needs_push"], 2);
        assert_eq!(v["sessions_active"], 0);
        assert_eq!(v["size_bytes"], 53_248);
        assert_eq!(v["query_ms"], 0.4);
        assert_eq!(v["round_trip_ms"], 12);
    }

    #[test]
    fn system_user_ids_are_collected_once() {
        let v = json!({
            "tenants": [{"user_id": "u2"}, {"user_id": "u1"}],
            "processes": [{"role": "node"}, {"role": "plugin", "plugin": {"user_id": "u1"}}, {"role": "plugin", "plugin": {"user_id": "u3"}}],
            "logs": {"bots": [{"user_id": "u4"}, {"user_id": ""}]}
        });
        assert_eq!(system_user_ids(&v), vec!["u1", "u2", "u3", "u4"]);
        assert!(system_user_ids(&json!({})).is_empty());
    }

    #[test]
    fn usage_known_only_when_all_nodes_answer() {
        let probes = vec![
            Probe {
                node: NodeInfo { id: "n1".into(), base_url: String::new(), last_seen_at: None },
                health: Ok(Value::Null),
                summary: Some(json!({"users": [{"user_id": "u1", "memory_bytes": 5, "storage_bytes": 7}]})),
            },
            Probe { node: NodeInfo { id: "n2".into(), base_url: String::new(), last_seen_at: None }, health: Err("down".into()), summary: None },
        ];
        let mut nodes = HashMap::new();
        nodes.insert("u1".to_string(), HashSet::from(["n1".to_string()]));
        nodes.insert("u2".to_string(), HashSet::from(["n1".to_string(), "n2".to_string()]));
        nodes.insert("u3".to_string(), HashSet::from(["n1".to_string()]));
        let u = user_usage(&probes, &nodes);
        assert_eq!(u.get("u1"), Some(&(5, 7)));
        assert_eq!(u.get("u2"), None);
        assert_eq!(u.get("u3"), Some(&(0, 0)));
    }
}
