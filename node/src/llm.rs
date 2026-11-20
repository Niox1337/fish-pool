use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::model::LlmSpec;

const CLAUDE_BASE_URL: &str = "https://api.anthropic.com/v1";
const CLAUDE_API_VERSION: &str = "2026-10-21";

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

/// Builds the provider's chat request body.
pub fn build_body(req: &ChatRequest<'_>) -> Value {
    let base_url = req.spec.base_url.trim_end_matches('/');
    let claude = base_url == CLAUDE_BASE_URL;
    let n = req.messages.len();
    let messages: Vec<Value> = req
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !claude || message.role != "system")
        .map(|(i, m)| {
            if i + 1 == n && m.role == "user" && !req.image_urls.is_empty() {
                let mut parts = vec![json!({"type": "text", "text": m.content})];
                for u in &req.image_urls {
                    let part = if claude {
                        let source = match u.strip_prefix("data:").and_then(|uri| uri.split_once(";base64,")) {
                            Some((media_type, data)) => json!({"type": "base64", "media_type": media_type, "data": data}),
                            None => json!({"type": "url", "url": u}),
                        };
                        json!({"type": "image", "source": source})
                    } else {
                        json!({"type": "image_url", "image_url": {"url": u}})
                    };
                    parts.push(part);
                }
                json!({"role": "user", "content": parts})
            } else {
                json!({"role": m.role, "content": m.content})
            }
        })
        .collect();
    if claude {
        let mut body = json!({"model": req.model, "messages": messages, "max_tokens": req.max_tokens, "stream": false});
        let system: Vec<_> = req.messages.iter()
            .filter(|message| message.role == "system" && !message.content.is_empty())
            .map(|message| json!({"type": "text", "text": message.content}))
            .collect();
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        match req.model {
            "claude-haiku-5-5" => body["thinking"] = json!({"type": "disabled"}),
            "claude-sonnet-5-5" => body["thinking"] = json!({"type": "between_tools"}),
            "claude-opus-5-5" => body["output_config"] = json!({"effort": "low"}),
            _ => {}
        }
        return body;
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "temperature": req.temperature,
        "stream": false,
    });
    let limit_field = match (base_url, req.model) {
        ("https://api.openai.com/v1", "gpt-6-luna" | "gpt-6.1-terra" | "gpt-6-sol") => {
            body["reasoning_effort"] = json!("none");
            "max_completion_tokens"
        }
        ("https://api.moonshot.cn/v1", "kimi-k3" | "kimi-k3.1-pro" | "kimi-k3.1-pro-highspeed") => {
            body.as_object_mut().unwrap().remove("temperature");
            body["reasoning_effort"] = json!("low");
            "max_completion_tokens"
        }
        ("https://api.deepseek.com" | "https://api.deepseek.com/v1", "deepseek-flash" | "deepseek-v4-pro") => {
            body["thinking"] = json!({"type": "disabled"});
            "max_tokens"
        }
        ("https://open.bigmodel.cn/api/paas/v4", "glm-5.3-flash" | "glm-5.3-flashx" | "glm-5.3") => {
            let temperature = (req.temperature.clamp(0.0, 1.0) as f64 * 100.0).round() / 100.0;
            body["temperature"] = json!(temperature);
            body["reasoning_effort"] = json!("low");
            "max_tokens"
        }
        (base, "qwen3.8-flash" | "qwen3.7-plus" | "qwen3.8-max") if is_dashscope_beijing(base) => {
            body["enable_thinking"] = json!(false);
            body["temperature"] = json!(req.temperature.min(1.9));
            "max_completion_tokens"
        }
        ("https://ark.cn-beijing.volces.com/api/v3", "doubao-seed-2-1-lite-260915" | "doubao-seed-2-0-mini-260428" | "doubao-seed-2-1-pro-260915") => {
            body["thinking"] = json!({"type": "disabled"});
            "max_completion_tokens"
        }
        ("https://api.xiaomimimo.com/v1", "mimo-v2.6-flash" | "mimo-v2.6-pro" | "mimo-v2.6-pro-ultraspeed") => {
            body["reasoning_effort"] = json!("none");
            body["temperature"] = json!(req.temperature.min(1.5));
            "max_completion_tokens"
        }
        ("https://generativelanguage.googleapis.com/v1beta/openai", "gemini-4-hydrogen" | "gemini-4-neon" | "gemini-4-carbon") => {
            body["reasoning_effort"] = json!("low");
            "max_tokens"
        }
        ("https://api.x.ai/v1", model @ ("grok-4.3" | "grok-4.6" | "grok-4.7")) => {
            body["temperature"] = json!(req.temperature.min(1.0));
            if model != "grok-4.3" {
                body["reasoning_effort"] = json!("low");
            }
            "max_tokens"
        }
        (
            "https://api.minimax.cn/v1" | "https://api.minimaxi.com/v1" | "https://api.minimax.io/v1",
            model @ ("MiniMax-M3.1-Flash" | "MiniMax-M3"),
        ) => {
            body.as_object_mut().unwrap().remove("temperature");
            if model == "MiniMax-M3.1-Flash" {
                body["reasoning_effort"] = json!("low");
                body["reasoning_split"] = json!(true);
            } else {
                body["thinking"] = json!({"type": "disabled"});
            }
            "max_completion_tokens"
        }
        ("https://api.longcat.chat/openai/v1", "LongCat-2.5") => {
            body.as_object_mut().unwrap().remove("temperature");
            body["thinking"] = json!({"type": "disabled"});
            "max_tokens"
        }
        _ => "max_tokens",
    };
    body[limit_field] = json!(req.max_tokens);
    body
}

/// Request URL for `path`, using the gateway endpoint when one is set.
fn request_url(spec: &LlmSpec, path: &str) -> String {
    let base = if spec.endpoint.is_empty() { &spec.base_url } else { &spec.endpoint };
    format!("{}/{path}", base.trim_end_matches('/'))
}

fn is_dashscope_beijing(base: &str) -> bool {
    base == "https://dashscope.aliyuncs.com/compatible-mode/v1"
        || base.strip_prefix("https://")
            .and_then(|url| url.strip_suffix(".cn-beijing.maas.aliyuncs.com/compatible-mode/v1"))
            .is_some_and(|workspace| !workspace.is_empty() && workspace.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn parse_claude_response(v: &Value) -> Result<String> {
    let blocks = v.get("content").and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Claude 响应缺少 content"))?;
    let text: String = blocks.iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    Ok(text.trim().to_owned())
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
    let claude = req.spec.base_url.trim_end_matches('/') == CLAUDE_BASE_URL;
    let path = if claude { "messages" } else { "chat/completions" };
    let body = build_body(&req);
    let mut request = http
        .post(request_url(req.spec, path))
        .timeout(Duration::from_secs(90))
        .json(&body);
    if claude {
        request = request
            .header("x-api-key", &req.spec.api_key)
            .header("anthropic-version", CLAUDE_API_VERSION);
    } else {
        request = request.bearer_auth(&req.spec.api_key);
    }
    if !req.spec.endpoint.is_empty() {
        request = request
            .header("cf-aig-authorization", format!("Bearer {}", req.spec.gateway_token))
            .header("cf-aig-collect-log", "false");
    }
    let resp = request
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
    if claude { parse_claude_response(&v) } else { parse_response(&v) }
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
            ..Default::default()
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
    fn documented_chat_parameters_are_scoped_to_the_official_endpoints() {
        let body = |base_url: &str, model: &str| {
            let spec = LlmSpec { base_url: base_url.into(), model: model.into(), api_key: "k".into(), ..Default::default() };
            build_body(&ChatRequest {
                spec: &spec,
                model,
                messages: vec![ChatMessage::new("user", "hello")],
                image_urls: vec![],
                temperature: 1.25,
                max_tokens: 1024,
            })
        };
        let openai = body("https://api.openai.com/v1", "gpt-6-luna");
        assert_eq!(openai["max_completion_tokens"], 1024);
        assert_eq!(openai["reasoning_effort"], "none");
        assert_eq!(openai["temperature"], 1.25);
        assert!(openai.get("max_tokens").is_none());

        let kimi = body("https://api.moonshot.cn/v1", "kimi-k3.1-pro");
        assert_eq!(kimi["max_completion_tokens"], 1024);
        assert_eq!(kimi["reasoning_effort"], "low");
        assert!(kimi.get("temperature").is_none());
        assert!(kimi.get("max_tokens").is_none());

        let deepseek = body("https://api.deepseek.com/v1/", "deepseek-flash");
        assert_eq!(deepseek["max_tokens"], 1024);
        assert_eq!(deepseek["thinking"]["type"], "disabled");

        let glm = body("https://open.bigmodel.cn/api/paas/v4", "glm-5.3-flash");
        assert_eq!(glm["temperature"], 1.0);
        assert_eq!(glm["reasoning_effort"], "low");

        let gateway = body("https://gateway.example.com/v1", "gpt-6-luna");
        assert_eq!(gateway["max_tokens"], 1024);
        assert_eq!(gateway["temperature"], 1.25);
        assert!(gateway.get("reasoning_effort").is_none());

        let qwen = body("https://llm-demo.cn-beijing.maas.aliyuncs.com/compatible-mode/v1", "qwen3.8-flash");
        assert_eq!(qwen["max_completion_tokens"], 1024);
        assert_eq!(qwen["enable_thinking"], false);

        let doubao = body("https://ark.cn-beijing.volces.com/api/v3", "doubao-seed-2-1-lite-260915");
        assert_eq!(doubao["max_completion_tokens"], 1024);
        assert_eq!(doubao["thinking"]["type"], "disabled");

        let mimo = body("https://api.xiaomimimo.com/v1", "mimo-v2.6-flash");
        assert_eq!(mimo["max_completion_tokens"], 1024);
        assert_eq!(mimo["reasoning_effort"], "none");

        let gemini = body("https://generativelanguage.googleapis.com/v1beta/openai", "gemini-4-hydrogen");
        assert_eq!(gemini["max_tokens"], 1024);
        assert_eq!(gemini["reasoning_effort"], "low");

        for model in ["grok-4.6", "grok-4.7"] {
            let grok = body("https://api.x.ai/v1", model);
            assert_eq!(grok["max_tokens"], 1024);
            assert_eq!(grok["temperature"], 1.0);
            assert_eq!(grok["reasoning_effort"], "low");
        }
        let grok = body("https://api.x.ai/v1", "grok-4.3");
        assert_eq!(grok["temperature"], 1.0);
        assert!(grok.get("reasoning_effort").is_none());

        let flash = body("https://api.minimax.cn/v1", "MiniMax-M3.1-Flash");
        assert_eq!(flash["max_completion_tokens"], 1024);
        assert_eq!(flash["reasoning_effort"], "low");
        assert_eq!(flash["reasoning_split"], true);
        assert!(flash.get("temperature").is_none());

        let m3 = body("https://api.minimax.io/v1", "MiniMax-M3");
        assert_eq!(m3["max_completion_tokens"], 1024);
        assert_eq!(m3["thinking"]["type"], "disabled");
        assert!(m3.get("temperature").is_none());

        let longcat = body("https://api.longcat.chat/openai/v1", "LongCat-2.5");
        assert_eq!(longcat["max_tokens"], 1024);
        assert_eq!(longcat["thinking"]["type"], "disabled");
        assert!(longcat.get("temperature").is_none());

        let custom = body("https://example.com/v1", "grok-4.7");
        assert_eq!(custom["temperature"], 1.25);
        assert!(custom.get("reasoning_effort").is_none());
    }

    #[test]
    fn claude_uses_native_message_and_image_blocks() {
        let spec = LlmSpec {
            base_url: CLAUDE_BASE_URL.into(),
            model: "claude-sonnet-5-5".into(),
            api_key: "test".into(),
            ..Default::default()
        };
        let body = build_body(&ChatRequest {
            spec: &spec,
            model: &spec.model,
            messages: vec![ChatMessage::new("system", "guide"), ChatMessage::new("user", "hello")],
            image_urls: vec!["https://example.com/a.png".into(), "data:image/png;base64,AAAA".into()],
            temperature: 0.7,
            max_tokens: 1024,
        });
        assert_eq!(body["system"], json!([{"type": "text", "text": "guide"}]));
        assert_eq!(body["messages"], json!([{"role": "user", "content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
        ]}]));
        assert_eq!(body["thinking"]["type"], "between_tools");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("temperature").is_none());

        let reply = json!({"content": [
            {"type": "thinking", "thinking": "don't display"},
            {"type": "text", "text": " hello"},
            {"type": "text", "text": " world "},
        ]});
        assert_eq!(parse_claude_response(&reply).unwrap(), "hello world");
    }

    #[test]
    fn gateway_endpoint_replaces_base_url_but_keeps_provider_tuning() {
        let mut spec = LlmSpec {
            base_url: "https://api.openai.com/v1/".into(),
            model: "gpt-6-luna".into(),
            api_key: "k".into(),
            ..Default::default()
        };
        assert_eq!(request_url(&spec, "chat/completions"), "https://api.openai.com/v1/chat/completions");
        spec.endpoint = "https://gateway.ai.cloudflare.com/v1/acc/fish/openai".into();
        assert_eq!(request_url(&spec, "chat/completions"), "https://gateway.ai.cloudflare.com/v1/acc/fish/openai/chat/completions");
        let body = build_body(&ChatRequest {
            spec: &spec,
            model: "gpt-6-luna",
            messages: vec![ChatMessage::new("user", "hi")],
            image_urls: vec![],
            temperature: 1.0,
            max_tokens: 64,
        });
        assert_eq!(body["max_completion_tokens"], 64);
        assert_eq!(body["reasoning_effort"], "none");
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
