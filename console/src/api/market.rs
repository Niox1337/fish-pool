//! Public AstrBot catalogue, served through the console for authenticated users.

use super::Reply;
use crate::app::App;
use crate::error::{ApiError, ApiResult};
use crate::{github, http};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::{Cache, Method, Request, Response};

const REGISTRY: &str = "https://cloud.astrbot.app/api/v1/market/plugins.json";

#[derive(Serialize, Deserialize)]
pub struct MarketPlugin {
    pub id: String,
    name: String,
    author: String,
    display_name: String,
    description: String,
    pub repo_url: String,
    category: String,
    tags: Vec<String>,
}

fn field<'a>(record: &'a Value, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn entry(id: &str, record: &Value) -> Option<MarketPlugin> {
    let name = field(record, "name").or_else(|| (!id.contains('/')).then_some(id))?;
    let author = field(record, "author")?;
    let plugin_id = format!("{author}/{name}");
    if id != plugin_id && id != name {
        return None;
    }
    let url = field(record, "repo")?;
    if !url.starts_with("https://") {
        return None;
    }
    let repo = github::parse_repo_url(url)?;
    let repo_url = repo.branch.as_ref().map_or_else(
        || repo.canonical_url(),
        |branch| format!("{}/tree/{branch}", repo.canonical_url()),
    );
    let description = field(record, "short_desc")
        .or_else(|| field(record, "desc"))
        .unwrap_or("");
    let tags = record
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(12)
        .map(|tag| tag.chars().take(40).collect())
        .collect();
    Some(MarketPlugin {
        id: plugin_id,
        name: name.to_string(),
        author: author.to_string(),
        display_name: field(record, "display_name")
            .unwrap_or(name)
            .chars()
            .take(100)
            .collect(),
        description: description.chars().take(260).collect(),
        repo_url,
        category: field(record, "category").unwrap_or("").chars().take(40).collect(),
        tags,
    })
}

fn catalogue(raw: &Value) -> Option<Vec<MarketPlugin>> {
    if raw.get("$meta")?.get("schema_version")?.as_u64() != Some(1) {
        return None;
    }
    let mut entries: Vec<_> = raw
        .as_object()?
        .iter()
        .filter(|(id, _)| id.as_str() != "$meta")
        .filter_map(|(id, record)| {
            let downloads = record.get("download_count").and_then(Value::as_u64).unwrap_or(0);
            Some((downloads, entry(id, record)?))
        })
        .collect();
    entries.sort_unstable_by(|(a, p), (b, q)| b.cmp(a).then_with(|| p.id.cmp(&q.id)));
    if entries.is_empty() {
        return None;
    }
    Some(entries.into_iter().map(|(_, item)| item).collect())
}

fn unavailable() -> ApiError {
    ApiError::new(502, "plugin_market_unavailable", "暂时读不到 AstrBot 插件市场")
}

/// Reads the catalogue body `{"items": [MarketPlugin]}` from the edge cache, or from the registry when it is not cached.
async fn body(req: &Request, app: &App) -> ApiResult<Value> {
    let mut cache_url = req.url().map_err(|_| unavailable())?;
    cache_url.set_path("/api/.cache/astrbot-market-v1");
    cache_url.set_query(None);
    let cache = Cache::default();
    if !app.dev {
        if let Ok(Some(mut cached)) = cache.get(cache_url.as_str(), false).await {
            if let Ok(body) = cached.json::<Value>().await {
                return Ok(body);
            }
        }
    }

    let source = if app.dev { app.var("ASTRBOT_MARKET_URL") } else { None };
    let response = http::send(
        Method::Get,
        source.as_deref().unwrap_or(REGISTRY),
        &[("accept", "application/json")],
        None,
        8_000,
    )
    .await
    .map_err(|_| unavailable())?;
    let items = response
        .ok()
        .then(|| response.json())
        .flatten()
        .as_ref()
        .and_then(catalogue)
        .ok_or_else(unavailable)?;
    let body = json!({ "items": items });
    if !app.dev {
        if let Ok(mut cached) = Response::from_json(&body) {
            let _ = cached.headers_mut().set("cache-control", "public, max-age=300");
            let _ = cache.put(cache_url.as_str(), cached).await;
        }
    }
    Ok(body)
}

pub async fn list(req: &Request, app: &App) -> ApiResult<Reply> {
    app.current_user(req).await?;
    Ok(Reply::json(body(req, app).await?))
}

/// Finds a catalogue entry by its market ID. Plugins install only from entries returned here.
pub async fn find(req: &Request, app: &App, id: &str) -> ApiResult<MarketPlugin> {
    let body = body(req, app).await?;
    let items: Vec<MarketPlugin> = serde_json::from_value(body["items"].clone()).map_err(|_| unavailable())?;
    items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::new(404, "market_plugin_not_found", "插件市场里没有这个插件"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_catalogue_identity_and_only_offers_github_sources() {
        let data = json!({
            "$meta": {"schema_version": 1, "repo": "https://github.com/owner/metadata"},
            "owner/normal": {"name": "normal", "author": "owner", "repo": "https://github.com/owner/normal.git", "desc": "Full", "short_desc": "Short"},
            "legacy": {"author": "owner", "repo": "https://github.com/owner/legacy/tree/dev", "download_count": 5},
            "other/bad": {"name": "bad", "author": "other", "repo": "https://example.org/other/bad"},
            "owner/mismatch": {"name": "different", "author": "owner", "repo": "https://github.com/owner/mismatch"},
        });
        let items = catalogue(&data).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "owner/legacy");
        assert_eq!(items[0].repo_url, "https://github.com/owner/legacy/tree/dev");
        assert_eq!(items[1].repo_url, "https://github.com/owner/normal");
        assert_eq!(items[1].description, "Short");
        assert!(catalogue(&json!({"$meta":{"schema_version":2}})).is_none());
    }
}
