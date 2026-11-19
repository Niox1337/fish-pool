use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::model::LlmSpec;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.to_string(),
            content: content.into(),
        }
    }
}

pub struct ChatRequest<'a> {
    pub spec: &'a LlmSpec,
    pub model: &'a str,
    pub messages: Vec<ChatMessage>,
    pub image_urls: Vec<String>,
    pub temperature: f32,
    pub max_tokens: u32,
}

/// Builds the JSON body of an OpenAI compatible `/chat/completions` request.
pub fn build_body(req: &ChatRequest<'_>) -> Value {
    let n = req.messages.len();
    let messages: Vec<Value> = req
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if i + 1 == n && m.role == "user" && !req.image_urls.is_empty() {
                let mut parts = vec![json!({"type": "text", "text": m.content})];
                for u in &req.image_urls {
                    parts.push(json!({"type": "image_url", "image_url": {"url": u}}));
                }
                json!({"role": "user", "content": parts})
            } else {
                json!({"role": m.role, "content": m.content})
            }
        })
        .collect();
    json!({
        "model": req.model,
        "messages": messages,
        "temperature": req.temperature,
        "max_tokens": req.max_tokens,
        "stream": false,
    })
}

/// Extracts the assistant text from a chat completion response.
pub fn parse_response(v: &Value) -> Result<String> {
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
        return Err(anyhow!("LLM 接口错误: {msg}"));
    }
    let content = v
        .pointer("/choices/0/message/content")
        .ok_or_else(|| anyhow!("LLM 响应缺少 choices"))?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    Ok(strip_think(&text).trim().to_string())
}

/// Removes a leading `<think>...</think>` reasoning block that some models emit inline.
fn strip_think(s: &str) -> &str {
    let t = s.trim_start();
    if t.starts_with("<think>") {
        if let Some(end) = t.find("</think>") {
            return &t[end + "</think>".len()..];
        }
    }
    s
}

pub async fn chat(http: &reqwest::Client, req: ChatRequest<'_>) -> Result<String> {
    if req.spec.api_key.is_empty() || req.spec.base_url.is_empty() {
        return Err(anyhow!("未配置 LLM 密钥"));
    }
    let url = format!("{}/chat/completions", req.spec.base_url.trim_end_matches('/'));
    let body = build_body(&req);
    let resp = http
        .post(url)
        .bearer_auth(&req.spec.api_key)
        .timeout(Duration::from_secs(90))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("LLM 请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let msg = v
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| text.chars().take(300).collect());
        return Err(anyhow!("LLM 返回 {status}: {msg}"));
    }
    parse_response(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_with_images() {
        let spec = LlmSpec {
            base_url: "x".into(),
            model: "m".into(),
            api_key: "k".into(),
        };
        let req = ChatRequest {
            spec: &spec,
            model: "deepseek-chat",
            messages: vec![ChatMessage::new("system", "s"), ChatMessage::new("user", "u")],
            image_urls: vec!["https://i".into()],
            temperature: 0.5,
            max_tokens: 10,
        };
        let b = build_body(&req);
        assert_eq!(b["model"], "deepseek-chat");
        assert_eq!(b["messages"][0]["content"], "s");
        assert_eq!(b["messages"][1]["content"][1]["image_url"]["url"], "https://i");
    }

    #[test]
    fn parse_variants() {
        let v = json!({"choices": [{"message": {"content": "<think>hmm</think>\n你好"}}]});
        assert_eq!(parse_response(&v).unwrap(), "你好");
        let v = json!({"error": {"message": "bad key"}});
        assert!(parse_response(&v).unwrap_err().to_string().contains("bad key"));
        assert!(parse_response(&json!({})).is_err());
    }
}
