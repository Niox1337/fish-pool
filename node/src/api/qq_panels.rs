use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ApiError, ApiResult};
use crate::node::Node;
use crate::platform::qq::QqConn;

const API_BASE: &str = "https://api.bot.qq.com";
const PANELS: &str = "/v2/panels";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Scope {
    C2c,
    Group,
}

impl Scope {
    fn as_str(&self) -> &str {
        match self {
            Self::C2c => "c2c",
            Self::Group => "group",
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ListQuery {
    scope: Scope,
}

#[derive(Deserialize, Serialize)]
struct Panel {
    items: Vec<PanelItem>,
    #[serde(default)]
    remark: String,
}

#[derive(Deserialize, Serialize)]
struct PanelItem {
    name: String,
    #[serde(default)]
    desc: String,
    #[serde(rename = "type")]
    kind: ItemType,
    #[serde(default)]
    only_admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<String>,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ItemType {
    Command,
    Link,
}

#[derive(Deserialize)]
struct Create {
    scope: Scope,
    panel: Panel,
}

#[derive(Deserialize)]
struct Update {
    panel: Panel,
    expected_version: Option<u64>,
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "qq_panel_invalid", message)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| invalid("指令面板格式错误"))
}

fn item_path(panel_id: &str) -> Result<String, ApiError> {
    if panel_id.is_empty()
        || panel_id.len() > 256
        || !panel_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(invalid("指令面板 ID 格式错误"));
    }
    Ok(format!("{PANELS}/{panel_id}"))
}

impl Panel {
    fn validated(mut self) -> Result<Self, ApiError> {
        if self.items.len() > 20 || self.remark.chars().count() > 255 {
            return Err(invalid("每个面板最多 20 项，备注最多 255 个字符"));
        }
        for item in &mut self.items {
            item.name = item.name.trim().to_owned();
            item.desc = item.desc.trim().to_owned();
            if item.name.is_empty()
                || item.name.chars().count() > 14
                || item.name.contains(['\r', '\n'])
                || item.desc.chars().count() > 30
            {
                return Err(invalid("名称或指令须为 1 到 14 个字符，说明最多 30 个字符"));
            }
            if item.kind == ItemType::Link {
                let url = item.link.as_deref().and_then(|link| Url::parse(link).ok());
                if !url.is_some_and(|url| {
                    url.scheme() == "https" && url.has_host() && url.username().is_empty() && url.password().is_none()
                }) {
                    return Err(invalid("跳转链接须为有效的 HTTPS 地址"));
                }
            } else {
                item.link = None;
            }
        }
        Ok(self)
    }
}

fn connect(node: &Node, bot_id: &str, platform_id: &str) -> Result<Arc<QqConn>, ApiError> {
    let bot = node.get(bot_id).ok_or_else(ApiError::not_found)?;
    bot.qq_panel_connection(platform_id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "qq_platform_missing",
            "节点上没有这组 QQ 接入，请检查接入设置",
        )
    })
}

async fn call(node: &Node, qq: &QqConn, method: Method, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
    let fixture = node
        .shared
        .cfg
        .dev_services
        .then(|| std::env::var("FISH_QQ_PANEL_API_BASE").ok())
        .flatten();
    let base = fixture.as_deref().unwrap_or(API_BASE).trim_end_matches('/');
    qq.api_at(base, method, path, body).await.map_err(|err| {
        let message = err.to_string().chars().take(360).collect::<String>();
        ApiError::new(StatusCode::BAD_GATEWAY, "qq_panel_upstream", message)
    })
}

pub(super) async fn list(
    State(node): State<Arc<Node>>,
    Path((bot_id, platform_id)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
) -> ApiResult {
    let qq = connect(&node, &bot_id, &platform_id)?;
    let path = format!("{PANELS}?scope={}&limit=50", query.scope.as_str());
    let mut value = call(&node, &qq, Method::GET, &path, None).await?;
    if value == json!({"is_end": true}) {
        value["records"] = json!([]);
    }
    if !value.get("records").is_some_and(Value::is_array) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "qq_panel_upstream",
            "QQ 返回的指令面板列表格式错误",
        ));
    }
    Ok(Json(value))
}

pub(super) async fn create(
    State(node): State<Arc<Node>>,
    Path((bot_id, platform_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let input: Create = parse(&body)?;
    let panel = input.panel.validated()?;
    let qq = connect(&node, &bot_id, &platform_id)?;
    let data = json!({"scope": input.scope, "target_type": "all", "panel": panel});
    let result = call(&node, &qq, Method::POST, PANELS, Some(&data)).await?;
    if result.get("panel_id").and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "qq_panel_upstream",
            "QQ 未返回面板 ID，请刷新列表确认创建结果",
        ));
    }
    Ok(Json(result))
}

pub(super) async fn update(
    State(node): State<Arc<Node>>,
    Path((bot_id, platform_id, panel_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let path = item_path(&panel_id)?;
    let input: Update = parse(&body)?;
    let panel = input.panel.validated()?;
    let qq = connect(&node, &bot_id, &platform_id)?;
    if let Some(expected) = input.expected_version {
        let current = call(&node, &qq, Method::GET, &path, None).await?;
        let version = current
            .get("version")
            .or_else(|| current.pointer("/panel/version"))
            .and_then(Value::as_u64);
        if version != Some(expected) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "qq_panel_changed",
                "QQ 上的面板已有更新，请重新载入后再编辑",
            ));
        }
    }
    Ok(Json(
        call(&node, &qq, Method::PUT, &path, Some(&json!({"panel": panel}))).await?,
    ))
}

pub(super) async fn delete(
    State(node): State<Arc<Node>>,
    Path((bot_id, platform_id, panel_id)): Path<(String, String, String)>,
) -> ApiResult {
    let path = item_path(&panel_id)?;
    let qq = connect(&node, &bot_id, &platform_id)?;
    call(&node, &qq, Method::DELETE, &path, None).await?;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_panel_items_and_safe_paths() {
        let panel: Panel = serde_json::from_value(json!({"items": [
            {"name": " /帮助 ", "type": "command", "link": "https://example.com"},
            {"name": "文档", "type": "link", "link": "https://example.com"}
        ]}))
        .unwrap();
        let body = serde_json::to_value(panel.validated().unwrap()).unwrap();
        assert_eq!(body["items"][0]["name"], "/帮助");
        assert!(body["items"][0].get("link").is_none());
        let bad: Panel = serde_json::from_value(json!({"items": [
            {"name": "文档", "type": "link", "link": "http://example.com"}
        ]}))
        .unwrap();
        assert!(bad.validated().is_err());
        assert!(item_path("../menu").is_err());
        assert_eq!(item_path("p_102030405_x8k2").unwrap(), "/v2/panels/p_102030405_x8k2");
    }
}
