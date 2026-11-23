//! Resource usage aggregated from the nodes that host the user's bots.

use super::Reply;
use crate::app::App;
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::models::UserRow;
use crate::node::NodeError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use worker::{Method, Request};

/// Merges node usage reports. Sources are kept only for bots in `bot_ids`. Plugin sources take their
/// label from `plugin_names`, keyed by plugin id, when it has an entry.
pub fn merge_usage(
    user: &UserRow,
    reports: &[Value],
    bot_ids: &HashSet<String>,
    plugin_names: &HashMap<String, String>,
) -> Value {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut sources = Vec::new();
    let mut history: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    for r in reports {
        for s in r.get("sources").and_then(Value::as_array).into_iter().flatten() {
            let bot = s.get("bot_id").and_then(Value::as_str).unwrap_or("");
            if bot_ids.contains(bot) {
                let mut s = s.clone();
                let name = s
                    .get("plugin_id")
                    .and_then(Value::as_str)
                    .and_then(|id| plugin_names.get(id));
                if let Some(name) = name {
                    s["label"] = json!(name);
                }
                sources.push(s);
            }
        }
        for h in r.get("history").and_then(Value::as_array).into_iter().flatten() {
            let e = history.entry(num(h, "ts")).or_insert((0, 0));
            e.0 += num(h, "memory_bytes");
            e.1 += num(h, "storage_bytes");
        }
    }
    let memory: i64 = sources.iter().map(|s| num(s, "memory_bytes")).sum();
    let storage: i64 = sources.iter().map(|s| num(s, "storage_bytes")).sum();
    json!({
        "quota": {"memory_mb": user.quota_memory_mb, "storage_mb": user.quota_storage_mb},
        "memory_bytes": memory,
        "storage_bytes": storage,
        "sources": sources,
        "history": history.into_iter().map(|(ts, (m, s))| json!({"ts": ts, "memory_bytes": m, "storage_bytes": s})).collect::<Vec<_>>(),
    })
}

pub async fn get(req: &Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    Ok(Reply::json(usage_for(app, &user).await?))
}

/// Usage object of one user, collected from every node hosting one of their bots.
pub async fn usage_for(app: &App, user: &UserRow) -> ApiResult<Value> {
    #[derive(Deserialize)]
    struct B {
        id: String,
        node_id: String,
    }
    let bots: Vec<B> = app
        .db
        .all("SELECT id, node_id FROM bots WHERE user_id = ?1", args![user.id])
        .await?;
    let bot_ids: HashSet<String> = bots.iter().map(|b| b.id.clone()).collect();
    let nodes: HashSet<String> = bots.iter().map(|b| b.node_id.clone()).collect();
    let q: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("user_id", &user.id)
        .finish();
    let mut reports = Vec::new();
    for node in nodes {
        let client = app.node_client(&node).await?;
        match client.call(Method::Get, &format!("/v1/usage?{q}"), None, 8_000).await {
            Ok(v) => reports.push(v),
            Err(NodeError::Unreachable(_)) => return Err(ApiError::node_unreachable()),
            Err(e) => return Err(ApiError::new(502, "node_error", e.detail())),
        }
    }
    #[derive(Deserialize)]
    struct P {
        id: String,
        name: String,
        display_name: String,
    }
    let plugins: Vec<P> = app
        .db
        .all(
            "SELECT p.id, p.name, p.display_name FROM plugins p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = ?1",
            args![user.id],
        )
        .await?;
    let names: HashMap<String, String> = plugins
        .into_iter()
        .filter_map(|p| {
            let label = if p.display_name.trim().is_empty() {
                p.name
            } else {
                p.display_name
            };
            (!label.trim().is_empty()).then_some((p.id, label))
        })
        .collect();
    Ok(merge_usage(user, &reports, &bot_ids, &names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::fixtures;

    #[test]
    fn merges_reports() {
        let ids: HashSet<String> = ["b1".to_string()].into();
        let r1 = json!({
            "sources": [
                {"bot_id": "b1", "kind": "core", "plugin_id": null, "label": "核心连接", "memory_bytes": 100, "storage_bytes": 10, "cpu_percent": 0.5},
                {"bot_id": "zz", "kind": "core", "plugin_id": null, "label": "x", "memory_bytes": 999, "storage_bytes": 999, "cpu_percent": 0.0}
            ],
            "history": [{"ts": 1, "memory_bytes": 5, "storage_bytes": 6}, {"ts": 2, "memory_bytes": 1, "storage_bytes": 1}]
        });
        let r2 = json!({
            "sources": [{"bot_id": "b1", "kind": "plugin", "plugin_id": "p1", "label": "插件", "memory_bytes": 50, "storage_bytes": 20, "cpu_percent": 0.0}],
            "history": [{"ts": 1, "memory_bytes": 5, "storage_bytes": 4}]
        });
        let names: HashMap<String, String> = [("p1".to_string(), "表情包管理".to_string())].into();
        let v = merge_usage(&fixtures::user(), &[r1, r2], &ids, &names);
        assert_eq!(v["sources"][1]["label"], "表情包管理");
        assert_eq!(v["sources"][0]["label"], "核心连接");
        assert_eq!(v["memory_bytes"], 150);
        assert_eq!(v["storage_bytes"], 30);
        assert_eq!(v["sources"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["history"][0],
            json!({"ts": 1, "memory_bytes": 10, "storage_bytes": 10})
        );
        assert_eq!(v["quota"]["memory_mb"], 256);
        let empty = merge_usage(&fixtures::user(), &[], &HashSet::new(), &HashMap::new());
        assert_eq!(empty["memory_bytes"], 0);
        assert_eq!(empty["history"], json!([]));
    }
}
