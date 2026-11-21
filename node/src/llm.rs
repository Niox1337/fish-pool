use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::model::LlmSpec;

/// An HTTP error reported by an LLM API, or an error object in a successful response when `status` is None.
#[derive(Debug)]
pub struct ApiError {
    pub status: Option<reqwest::StatusCode>,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "LLM 返回 {status}: {}", self.message),
            None => write!(f, "LLM 接口错误: {}", self.message),
        }
    }
}

impl std::error::Error for ApiError {}

/// The LLM request did not get an HTTP response.
#[derive(Debug)]
pub struct RequestFailed(pub String);

impl std::fmt::Display for RequestFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LLM 请求失败: {}", self.0)
    }
}

impl std::error::Error for RequestFailed {}

/// LLM failures with a known cause that the chat user is told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trouble {
    NoKey,
    Balance,
    BadKey,
    ModelMissing,
    Busy,
    Unreachable,
    Refused,
}

impl Trouble {
    /// Reply sent to the chat in place of the model's answer.
    pub fn notice(self) -> &'static str {
        match self {
            Trouble::NoKey => "我还没有接上 AI 模型，暂时没法聊天哦。请小鱼的主人登录 fish.niox.lol，先在「模型 Key」里添加一个 Key，再到「机器人设置」里选上它。",
            Trouble::Balance => "我用的 AI 模型账户没余额啦，暂时回不了消息。请小鱼的主人给模型 Key 充值，或者登录 fish.niox.lol，在「机器人设置」里换一个有余额的 Key。",
            Trouble::BadKey => "我用的模型 Key 失效了，可能是填错了、被停用了或者过期了，暂时回不了消息。请小鱼的主人登录 fish.niox.lol，在「模型 Key」里检查或重新添加。",
            Trouble::ModelMissing => "我选的 AI 模型现在用不了，可能是名字填错了或者已经下线，暂时回不了消息。请小鱼的主人登录 fish.niox.lol，在「机器人设置」里换一个模型。",
            Trouble::Busy => "AI 模型那边现在太忙了，我排不上队。等一两分钟再来找我吧。",
            Trouble::Unreachable => "我暂时连不上 AI 模型，可能是对方服务出了点问题。过几分钟再试试吧。",
            Trouble::Refused => "这条消息被 AI 模型的内容审核拦下了，我没法回答。换个说法再问问吧。",
        }
    }

    /// Minimum time between two notices of this kind in one chat session.
    pub fn cooldown(self) -> Duration {
        match self {
            Trouble::Busy | Trouble::Unreachable => Duration::from_secs(60),
            Trouble::Refused => Duration::from_secs(10),
            _ => Duration::from_secs(600),
        }
    }

    /// Whether the same request with other options cannot succeed.
    pub fn is_account_problem(self) -> bool {
        matches!(self, Trouble::NoKey | Trouble::Balance | Trouble::BadKey | Trouble::ModelMissing)
    }
}

/// Classifies an error from `chat_reply`. None for errors without a known cause.
pub fn trouble(e: &anyhow::Error) -> Option<Trouble> {
    if e.downcast_ref::<RequestFailed>().is_some() {
        return Some(Trouble::Unreachable);
    }
    let api = e.downcast_ref::<ApiError>()?;
    let text = format!("{} {}", api.code, api.message).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    let status = api.status.map(|s| s.as_u16());
    if status == Some(402)
        || has(&["insufficient_quota", "insufficient_balance", "insufficient balance", "insufficient_user_quota",
                 "exceeded your current quota", "credit balance is too low", "余额不足", "欠费", "arrearage"])
    {
        return Some(Trouble::Balance);
    }
    if status == Some(401)
        || has(&["invalid_api_key", "invalid api key", "incorrect api key", "key_disabled", "key_expired", "authentication_error"])
    {
        return Some(Trouble::BadKey);
    }
    if status == Some(404) || has(&["model_not_found", "does not exist", "model not found", "no such model"]) {
        return Some(Trouble::ModelMissing);
    }
    if matches!(status, Some(429) | Some(529)) || has(&["rate_limit", "overloaded"]) {
        return Some(Trouble::Busy);
    }
    if status.is_some_and(|s| s >= 500) {
        return Some(Trouble::Unreachable);
    }
    if has(&["content_filter", "content exists risk", "data_inspection_failed", "content_policy"]) {
        return Some(Trouble::Refused);
    }
    None
}

const CLAUDE_BASE_URL: &str = "https://api.anthropic.com/v1";
const CLAUDE_API_VERSION: &str = "2026-10-21";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Provider-native message, or an array of content blocks for Claude.
    pub raw: Option<Value>,
    /// Set on `tool` messages.
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.to_string(),
            content: content.into(),
            ..Default::default()
        }
    }

    /// The assistant turn that asked for tool calls, in the provider's own format.
    pub fn assistant_raw(raw: Value) -> Self {
        ChatMessage {
            role: "assistant".into(),
            raw: Some(raw),
            ..Default::default()
        }
    }

    pub fn tool_result(call_id: &str, content: impl Into<String>) -> Self {
        ChatMessage {
            role: "tool".into(),
            content: content.into(),
            tool_call_id: Some(call_id.to_string()),
            ..Default::default()
        }
    }
}

/// A function the model may call. `parameters` is a JSON schema object.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// Assistant message to append before the tool results when `tool_calls` is not empty.
    pub raw: Option<Value>,
}

impl ChatReply {
    pub fn plugin_result(self, req_id: &str) -> Value {
        let mut result = json!({"op": "llm_result", "req_id": req_id, "text": self.text, "tool_calls": self.tool_calls});
        if let Some(raw) = self.raw {
            if let Some(thinking) = raw.as_array().and_then(|blocks| blocks.iter().find(|b| b["type"] == "thinking")) {
                result["reasoning_content"] = thinking["thinking"].clone();
                result["reasoning_signature"] = thinking["signature"].clone();
            } else {
                if let Some(reasoning) = raw.get("reasoning_content") {
                    result["reasoning_content"] = reasoning.clone();
                }
                let extra = raw["tool_calls"].as_array().into_iter().flatten().filter_map(|call| {
                    Some((call["id"].as_str()?.to_string(), call.get("extra_content")?.clone()))
                }).collect::<serde_json::Map<String, Value>>();
                if !extra.is_empty() {
                    result["tools_call_extra_content"] = Value::Object(extra);
                }
            }
        }
        result
    }
}

pub struct ChatRequest<'a> {
    pub spec: &'a LlmSpec,
    pub model: &'a str,
    pub messages: Vec<ChatMessage>,
    pub image_urls: Vec<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub tools: Vec<ToolDef>,
    /// Keeps the tool definitions but tells the model not to call any.
    pub tool_choice_none: bool,
}

fn openai_message(m: &ChatMessage) -> Value {
    if let Some(raw) = &m.raw {
        return raw.clone();
    }
    match &m.tool_call_id {
        Some(id) => json!({"role": "tool", "tool_call_id": id, "content": m.content}),
        None => json!({"role": m.role, "content": m.content}),
    }
}

fn claude_image(url: &str) -> Value {
    let source = match url.strip_prefix("data:").and_then(|uri| uri.split_once(";base64,")) {
        Some((media_type, data)) => json!({"type": "base64", "media_type": media_type, "data": data}),
        None => json!({"type": "url", "url": url}),
    };
    json!({"type": "image", "source": source})
}

/// Converts AstrBot's OpenAI-style message history, including tool calls and their results.
pub fn plugin_contexts(spec: &LlmSpec, contexts: &[Value]) -> Vec<ChatMessage> {
    let claude = spec.base_url.trim_end_matches('/') == CLAUDE_BASE_URL;
    contexts.iter().filter_map(|message| {
        let role = message["role"].as_str()?;
        let content = &message["content"];
        let parts: Vec<Value> = match content {
            Value::Array(parts) => parts.iter().filter_map(|part| match part["type"].as_str()? {
                "text" => Some(json!({"type": "text", "text": part["text"].as_str()?})),
                "image_url" if role == "user" => {
                    let url = part["image_url"]["url"].as_str()?;
                    Some(if claude { claude_image(url) } else { json!({"type": "image_url", "image_url": {"url": url}}) })
                }
                _ => None,
            }).collect(),
            Value::String(text) if !text.is_empty() => vec![json!({"type": "text", "text": text})],
            _ => vec![],
        };
        let text = parts.iter().filter_map(|p| p["text"].as_str()).collect::<String>();
        if role == "tool" {
            return Some(ChatMessage::tool_result(message["tool_call_id"].as_str()?, text));
        }
        if !matches!(role, "system" | "user" | "assistant") {
            return None;
        }
        let calls = message["tool_calls"].as_array().filter(|calls| role == "assistant" && !calls.is_empty());
        let think = content.as_array().and_then(|parts| parts.iter().find(|part| part["type"] == "think"));
        if claude {
            if let Some(calls) = calls {
                let mut blocks = parts;
                if let Some((reasoning, signature)) = think.and_then(|part| Some((part["think"].as_str()?, part["encrypted"].as_str()?))) {
                    blocks.insert(0, json!({"type": "thinking", "thinking": reasoning, "signature": signature}));
                }
                for call in calls {
                    let args = call["function"]["arguments"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok())?;
                    blocks.push(json!({
                        "type": "tool_use", "id": call["id"].as_str()?,
                        "name": call["function"]["name"].as_str()?, "input": args,
                    }));
                }
                return Some(ChatMessage::assistant_raw(Value::Array(blocks)));
            }
        } else if let Some(calls) = calls {
            let content = if text.is_empty() { Value::Null } else { Value::String(text) };
            let mut raw = json!({"role": "assistant", "content": content, "tool_calls": calls});
            if let Some(reasoning) = think.and_then(|part| part["think"].as_str()).or_else(|| message["reasoning_content"].as_str()) {
                raw["reasoning_content"] = json!(reasoning);
            }
            if let Some(details) = message.get("reasoning_details") {
                raw["reasoning_details"] = details.clone();
            }
            return Some(ChatMessage::assistant_raw(raw));
        }
        let mut result = ChatMessage::new(role, text);
        if role == "user" && parts.iter().any(|part| part["type"] != "text") {
            result.raw = Some(if claude { Value::Array(parts) } else { json!({"role": role, "content": parts}) });
        }
        Some(result)
    }).collect()
}

/// Claude messages. Consecutive tool results become one user turn of `tool_result` blocks.
fn claude_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for m in messages.iter().filter(|m| m.role != "system") {
        if let Some(raw) = &m.raw {
            out.push(json!({"role": m.role, "content": raw}));
        } else if let Some(id) = &m.tool_call_id {
            let block = json!({"type": "tool_result", "tool_use_id": id, "content": m.content});
            let merged = out.last_mut().and_then(|last| {
                let is_results = last["role"] == "user"
                    && last["content"].as_array().is_some_and(|c| c.iter().all(|b| b["type"] == "tool_result"));
                if is_results { last["content"].as_array_mut() } else { None }
            });
            match merged {
                Some(blocks) => blocks.push(block),
                None => out.push(json!({"role": "user", "content": [block]})),
            }
        } else {
            out.push(json!({"role": m.role, "content": m.content}));
        }
    }
    out
}

/// Builds the provider's chat request body.
pub fn build_body(req: &ChatRequest<'_>) -> Value {
    let base_url = req.spec.base_url.trim_end_matches('/');
    let claude = base_url == CLAUDE_BASE_URL;
    let n = req.messages.len();
    let plain_tail = req.messages.last().is_some_and(|m| m.role == "user" && m.raw.is_none() && m.tool_call_id.is_none());
    let mut messages: Vec<Value> = if claude {
        claude_messages(&req.messages)
    } else {
        req.messages.iter().map(openai_message).collect()
    };
    if plain_tail && !req.image_urls.is_empty() {
        let m = &req.messages[n - 1];
        let mut parts = vec![json!({"type": "text", "text": m.content})];
        for u in &req.image_urls {
            let part = if claude {
                claude_image(u)
            } else {
                json!({"type": "image_url", "image_url": {"url": u}})
            };
            parts.push(part);
        }
        if let Some(last) = messages.last_mut() {
            *last = json!({"role": "user", "content": parts});
        }
    }
    if claude {
        let mut body = json!({"model": req.model, "messages": messages, "max_tokens": req.max_tokens, "stream": false});
        if !req.tools.is_empty() {
            body["tools"] = req
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
                .collect();
            if req.tool_choice_none {
                body["tool_choice"] = json!({"type": "none"});
            }
        }
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
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
        if req.tool_choice_none {
            body["tool_choice"] = json!("none");
        }
    }
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

fn parse_claude_response(v: &Value) -> Result<ChatReply> {
    let blocks = v.get("content").and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Claude 响应缺少 content"))?;
    let text: String = blocks.iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    let tool_calls: Vec<ToolCall> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|b| ToolCall {
            id: b.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
            name: b.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            arguments: b.get("input").cloned().unwrap_or_else(|| json!({})),
        })
        .collect();
    let raw = (!tool_calls.is_empty()).then(|| Value::Array(blocks.clone()));
    Ok(ChatReply { text: text.trim().to_owned(), tool_calls, raw })
}

/// Fields of an OpenAI compatible assistant message that are sent back with tool results.
const ASSISTANT_ECHO_FIELDS: [&str; 4] = ["content", "tool_calls", "reasoning_content", "reasoning_details"];

/// Extracts the assistant text and tool calls from a chat completion response.
pub fn parse_response(v: &Value) -> Result<ChatReply> {
    if let Some(err) = v.get("error") {
        let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error").to_string();
        return Err(ApiError { status: None, code: error_code(err), message }.into());
    }
    let message = v.pointer("/choices/0/message").ok_or_else(|| anyhow!("LLM 响应缺少 choices"))?;
    let text = match message.get("content").unwrap_or(&Value::Null) {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let tool_calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|c| {
                    let args = c.pointer("/function/arguments").cloned().unwrap_or(Value::Null);
                    let arguments = match args {
                        Value::String(s) if s.trim().is_empty() => json!({}),
                        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
                        Value::Null => json!({}),
                        other => other,
                    };
                    ToolCall {
                        id: c.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        arguments,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let raw = (!tool_calls.is_empty()).then(|| {
        let mut out = json!({"role": "assistant"});
        for k in ASSISTANT_ECHO_FIELDS {
            if let Some(x) = message.get(k) {
                out[k] = x.clone();
            }
        }
        out
    });
    Ok(ChatReply { text: strip_think(&text).trim().to_string(), tool_calls, raw })
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

/// `code` and `type` of an OpenAI or Claude error object, joined by a space.
fn error_code(err: &Value) -> String {
    ["code", "type"]
        .iter()
        .filter_map(|k| match err.get(*k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sends one chat request and returns the text and any tool calls.
pub async fn chat_reply(http: &reqwest::Client, req: ChatRequest<'_>) -> Result<ChatReply> {
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
        .map_err(|e| RequestFailed(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let message = v
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| text.chars().take(300).collect());
        let code = v.get("error").map(error_code).unwrap_or_default();
        return Err(ApiError { status: Some(status), code, message }.into());
    }
    if claude { parse_claude_response(&v) } else { parse_response(&v) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, body: Value) -> anyhow::Error {
        let err = &body["error"];
        ApiError {
            status: Some(reqwest::StatusCode::from_u16(status).unwrap()),
            code: error_code(err),
            message: err["message"].as_str().unwrap_or_default().to_string(),
        }
        .into()
    }

    #[test]
    fn classifies_known_llm_failures() {
        let gw = |status, code: &str, message: &str| api(status, json!({"error": {"message": message, "type": code, "code": code}}));
        assert_eq!(trouble(&gw(402, "insufficient_quota", "API Key 余额不足")), Some(Trouble::Balance));
        assert_eq!(trouble(&gw(429, "insufficient_quota", "You exceeded your current quota")), Some(Trouble::Balance));
        assert_eq!(trouble(&api(400, json!({"type": "error", "error": {"type": "invalid_request_error", "message": "Your credit balance is too low to access the Anthropic API."}}))), Some(Trouble::Balance));
        assert_eq!(trouble(&gw(401, "invalid_api_key", "API Key 无效")), Some(Trouble::BadKey));
        assert_eq!(trouble(&gw(403, "key_expired", "这个 API Key 已过期")), Some(Trouble::BadKey));
        assert_eq!(trouble(&gw(404, "model_not_found", "网关没有开放模型 x")), Some(Trouble::ModelMissing));
        assert_eq!(trouble(&gw(400, "invalid_request_error", "The model `gpt-x` does not exist")), Some(Trouble::ModelMissing));
        assert_eq!(trouble(&gw(429, "rate_limit_exceeded", "请求太频繁，请稍后再试")), Some(Trouble::Busy));
        assert_eq!(trouble(&gw(503, "upstream_error", "网关暂时不可用")), Some(Trouble::Unreachable));
        assert_eq!(trouble(&gw(400, "invalid_request_error", "Content Exists Risk")), Some(Trouble::Refused));
        assert_eq!(trouble(&gw(400, "invalid_request_error", "max_tokens is too large")), None);
        assert_eq!(trouble(&RequestFailed("timeout".into()).into()), Some(Trouble::Unreachable));
        assert_eq!(trouble(&anyhow!("LLM 响应缺少 choices")), None);
        let in_body = parse_response(&json!({"error": {"message": "Insufficient Balance", "code": "invalid_request_error"}})).unwrap_err();
        assert_eq!(trouble(&in_body), Some(Trouble::Balance));
        assert_eq!(gw(402, "insufficient_quota", "API Key 余额不足").to_string(), "LLM 返回 402 Payment Required: API Key 余额不足");
    }

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
            tools: vec![],
            tool_choice_none: false,
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
                tools: vec![],
                tool_choice_none: false,
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
            tools: vec![],
            tool_choice_none: false,
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
        assert_eq!(parse_claude_response(&reply).unwrap().text, "hello world");
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
            tools: vec![],
            tool_choice_none: false,
        });
        assert_eq!(body["max_completion_tokens"], 64);
        assert_eq!(body["reasoning_effort"], "none");
    }

    #[test]
    fn parse_variants() {
        let v = json!({"choices": [{"message": {"content": "<think>hmm</think>\n你好"}}]});
        assert_eq!(parse_response(&v).unwrap().text, "你好");
        let v = json!({"error": {"message": "bad key"}});
        assert!(parse_response(&v).unwrap_err().to_string().contains("bad key"));
        assert!(parse_response(&json!({})).is_err());
    }

    fn tool_request<'a>(spec: &'a LlmSpec, messages: Vec<ChatMessage>, none: bool) -> Value {
        build_body(&ChatRequest {
            spec,
            model: &spec.model,
            messages,
            image_urls: vec![],
            temperature: 0.5,
            max_tokens: 100,
            tools: vec![ToolDef {
                name: "lookup".into(),
                description: "Looks up".into(),
                parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            }],
            tool_choice_none: none,
        })
    }

    #[test]
    fn openai_tool_round_trip() {
        let spec = LlmSpec { base_url: "https://example.com/v1".into(), model: "m".into(), api_key: "k".into(), ..Default::default() };
        let response = json!({"choices": [{"message": {
            "role": "assistant",
            "content": null,
            "reasoning_content": "think",
            "refusal": null,
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "lookup", "arguments": "{\"q\":\"梗\"}"}}],
        }}]});
        let reply = parse_response(&response).unwrap();
        assert_eq!(reply.text, "");
        assert_eq!(reply.tool_calls, vec![ToolCall { id: "c1".into(), name: "lookup".into(), arguments: json!({"q": "梗"}) }]);
        let raw = reply.raw.clone().unwrap();
        assert!(raw.get("refusal").is_none());
        assert_eq!(raw["reasoning_content"], "think");

        let messages = vec![
            ChatMessage::new("system", "s"),
            ChatMessage::new("user", "hi"),
            ChatMessage::assistant_raw(raw.clone()),
            ChatMessage::tool_result("c1", "result"),
        ];
        let body = tool_request(&spec, messages.clone(), false);
        assert_eq!(body["tools"][0], json!({"type": "function", "function": {"name": "lookup", "description": "Looks up", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}}));
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["messages"][2], raw);
        assert_eq!(body["messages"][3], json!({"role": "tool", "tool_call_id": "c1", "content": "result"}));
        assert_eq!(tool_request(&spec, messages, true)["tool_choice"], "none");
        let mut plugin_turn = raw.clone();
        plugin_turn["content"] = json!([{"type": "think", "think": "think", "encrypted": null}]);
        plugin_turn.as_object_mut().unwrap().remove("reasoning_content");
        let replay = plugin_contexts(&spec, &[
            json!({"role": "user", "content": [{"type": "text", "text": "hi"}]}),
            plugin_turn,
            json!({"role": "tool", "tool_call_id": "c1", "content": [{"type": "text", "text": "result"}]}),
        ]);
        assert_eq!(tool_request(&spec, replay, false)["messages"].as_array().unwrap().as_slice(),
                   &body["messages"].as_array().unwrap()[1..]);
        let result = reply.plugin_result("r");
        assert_eq!(result["reasoning_content"], "think");
        assert_eq!(result["tool_calls"][0], json!({"id": "c1", "name": "lookup", "arguments": {"q": "梗"}}));
        let plain = parse_response(&json!({"choices": [{"message": {"content": "done"}}]})).unwrap();
        assert!(plain.tool_calls.is_empty() && plain.raw.is_none());
    }

    #[test]
    fn claude_tool_round_trip() {
        let spec = LlmSpec { base_url: CLAUDE_BASE_URL.into(), model: "claude-sonnet-5-5".into(), api_key: "k".into(), ..Default::default() };
        let blocks = json!([
            {"type": "thinking", "thinking": "x", "signature": "sig"},
            {"type": "tool_use", "id": "t1", "name": "lookup", "input": {"q": "a"}},
            {"type": "tool_use", "id": "t2", "name": "lookup", "input": {"q": "b"}},
        ]);
        let reply = parse_claude_response(&json!({"content": blocks})).unwrap();
        assert_eq!(reply.tool_calls.len(), 2);
        assert_eq!(reply.tool_calls[1].arguments, json!({"q": "b"}));
        let messages = vec![
            ChatMessage::new("system", "s"),
            ChatMessage::new("user", "hi"),
            ChatMessage::assistant_raw(reply.raw.clone().unwrap()),
            ChatMessage::tool_result("t1", "ra"),
            ChatMessage::tool_result("t2", "rb"),
        ];
        let body = tool_request(&spec, messages, true);
        assert_eq!(body["tools"][0]["input_schema"]["properties"]["q"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({"type": "none"}));
        assert_eq!(body["messages"], json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": blocks},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ra"},
                {"type": "tool_result", "tool_use_id": "t2", "content": "rb"},
            ]},
        ]));
        let result = reply.plugin_result("r");
        let calls: Vec<_> = result["tool_calls"].as_array().unwrap().iter().map(|c| {
            json!({"id": c["id"], "type": "function", "function": {"name": c["name"], "arguments": c["arguments"].to_string()}})
        }).collect();
        let replay = plugin_contexts(&spec, &[
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "tool_calls": calls, "content": [
                {"type": "think", "think": result["reasoning_content"], "encrypted": result["reasoning_signature"]},
            ]}),
            json!({"role": "tool", "tool_call_id": "t1", "content": "ra"}),
            json!({"role": "tool", "tool_call_id": "t2", "content": "rb"}),
        ]);
        assert_eq!(tool_request(&spec, replay, true)["messages"], body["messages"]);
    }

}
