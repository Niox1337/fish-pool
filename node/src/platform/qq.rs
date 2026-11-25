use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

use super::{backoff_secs, Inbound, StatusSink};
use crate::logs::LogBuffer;
use crate::model::{chain_text, Component, MessageType, Sender, PLATFORM_QQ};

pub const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";
pub const API_BASE: &str = "https://api.sgroup.qq.com";
/// Intent bit for group @ messages and C2C messages.
pub const INTENT_GROUP_AND_C2C: u64 = 1 << 25;

const SRC: &str = "qq_official";
/// Age up to which an inbound message id is used as the passive reply target, in groups and private chats.
const REPLY_WINDOW: Duration = Duration::from_secs(60 * 60);

pub mod op {
    pub const DISPATCH: u8 = 0;
    pub const HEARTBEAT: u8 = 1;
    pub const IDENTIFY: u8 = 2;
    pub const RESUME: u8 = 6;
    pub const RECONNECT: u8 = 7;
    pub const INVALID_SESSION: u8 = 9;
    pub const HELLO: u8 = 10;
    pub const HEARTBEAT_ACK: u8 = 11;
}

#[derive(Debug, Deserialize)]
pub struct GatewayPayload {
    #[serde(default)]
    pub id: Option<String>,
    pub op: u8,
    #[serde(default)]
    pub d: Value,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

/// Parses `{access_token, expires_in}` where `expires_in` may be a string or a number.
pub fn parse_token_response(v: &Value) -> Result<(String, u64)> {
    let token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("缺少 access_token");
            anyhow!("获取 QQ access token 失败: {msg}")
        })?;
    let expires = match v.get("expires_in") {
        Some(Value::String(s)) => s.parse().unwrap_or(7200),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(7200),
        _ => 7200,
    };
    Ok((token.to_string(), expires))
}

pub fn identify_payload(token: &str, intents: u64) -> Value {
    json!({"op": op::IDENTIFY, "d": {
        "token": format!("QQBot {token}"),
        "intents": intents,
        "shard": [0, 1],
        "properties": {"$os": "linux", "$browser": "fish-node", "$device": "fish-node"}
    }})
}

pub fn resume_payload(token: &str, session_id: &str, seq: u64) -> Value {
    json!({"op": op::RESUME, "d": {"token": format!("QQBot {token}"), "session_id": session_id, "seq": seq}})
}

pub fn heartbeat_payload(seq: Option<u64>) -> Value {
    json!({"op": op::HEARTBEAT, "d": seq})
}

fn normalize_url(u: &str) -> String {
    if u.starts_with("http://") || u.starts_with("https://") {
        u.to_string()
    } else {
        format!("https://{}", u.trim_start_matches('/'))
    }
}

/// Removes `<@!id>` and `<@id>` mention markers from message content.
fn strip_mentions(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("<@") {
        match rest[start..].find('>') {
            Some(end) => {
                out.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Converts a QQ dispatch event into an `Inbound` when it is a group or C2C message.
/// `GROUP_MESSAGE_CREATE` carries every group message when the group lets the bot receive all
/// messages, and counts as an @ only when `mentions` marks the bot with `is_you`.
pub fn parse_dispatch(t: &str, d: &Value, platform_id: &str, self_id: &str) -> Option<Inbound> {
    let (mt, group_id, sender_id) = match t {
        "GROUP_AT_MESSAGE_CREATE" | "GROUP_MESSAGE_CREATE" => {
            let gid = d
                .get("group_openid")
                .or_else(|| d.get("group_id"))?
                .as_str()?
                .to_string();
            let author = d.get("author")?;
            let uid = author
                .get("member_openid")
                .or_else(|| author.get("id"))?
                .as_str()?
                .to_string();
            (MessageType::Group, Some(gid), uid)
        }
        "C2C_MESSAGE_CREATE" => {
            let author = d.get("author")?;
            let uid = author
                .get("user_openid")
                .or_else(|| author.get("id"))?
                .as_str()?
                .to_string();
            (MessageType::Private, None, uid)
        }
        _ => return None,
    };
    let msg_id = d.get("id")?.as_str()?.to_string();
    let is_at = match t {
        "GROUP_AT_MESSAGE_CREATE" => true,
        "GROUP_MESSAGE_CREATE" => d
            .get("mentions")
            .and_then(Value::as_array)
            .is_some_and(|m| m.iter().any(|u| u.get("is_you").and_then(Value::as_bool) == Some(true))),
        _ => false,
    };
    let text = strip_mentions(d.get("content").and_then(|c| c.as_str()).unwrap_or(""));
    let mut chain = vec![];
    if is_at {
        chain.push(Component::At {
            user_id: self_id.to_string(),
            name: None,
        });
    }
    if !text.is_empty() {
        chain.push(Component::plain(text.clone()));
    }
    if let Some(atts) = d.get("attachments").and_then(|a| a.as_array()) {
        for a in atts {
            let ct = a.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
            let Some(url) = a.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            if ct.starts_with("image") {
                chain.push(Component::image_url(normalize_url(url)));
            } else if ct.starts_with("voice") || ct.starts_with("audio") {
                chain.push(Component::Record {
                    url: Some(normalize_url(url)),
                    path: None,
                });
            }
        }
    }
    let session_id = group_id.clone().unwrap_or_else(|| sender_id.clone());
    Some(Inbound {
        platform_id: platform_id.to_string(),
        platform_kind: PLATFORM_QQ,
        self_id: self_id.to_string(),
        message_type: mt,
        group_id,
        session_id,
        sender: Sender {
            user_id: sender_id.clone(),
            nickname: sender_id,
        },
        text,
        chain,
        is_at,
        msg_id,
        raw: d.clone(),
    })
}

/// Log line for a QQ event that changes whether the bot is in a group or a friend list, or whether
/// it may send active messages there. None for other events.
pub fn notice_text(t: &str, d: &Value) -> Option<String> {
    let field = |key: &str| d.get(key).and_then(Value::as_str).unwrap_or("未知");
    let group = || format!("群 {}", field("group_openid"));
    let by = || format!("（操作人 {}）", field("op_member_openid"));
    let user = || format!("用户 {}", field("openid"));
    Some(match t {
        "GROUP_ADD_ROBOT" => format!("机器人被拉进{}{}", group(), by()),
        "GROUP_DEL_ROBOT" => format!("机器人被移出{}{}，不会再收到这个群的消息", group(), by()),
        "GROUP_MSG_REJECT" => format!("{} 关闭了机器人的主动消息{}", group(), by()),
        "GROUP_MSG_RECEIVE" => format!("{} 开启了机器人的主动消息{}", group(), by()),
        "FRIEND_ADD" => format!("{} 添加了机器人", user()),
        "FRIEND_DEL" => format!("{} 删除了机器人，不会再收到这个用户的私聊", user()),
        "C2C_MSG_REJECT" => format!("{} 关闭了机器人的主动消息", user()),
        "C2C_MSG_RECEIVE" => format!("{} 开启了机器人的主动消息", user()),
        _ => return None,
    })
}

pub fn text_message(content: &str, msg_id: Option<&str>, msg_seq: u32) -> Value {
    let mut v = json!({"content": content, "msg_type": 0, "msg_seq": msg_seq});
    if let Some(id) = msg_id {
        v["msg_id"] = json!(id);
    }
    v
}

pub fn media_message(file_info: &str, content: &str, msg_id: Option<&str>, msg_seq: u32) -> Value {
    let mut v = json!({"content": content, "msg_type": 7, "media": {"file_info": file_info}, "msg_seq": msg_seq});
    if let Some(id) = msg_id {
        v["msg_id"] = json!(id);
    }
    v
}

/// Error answered by the QQ open API, with the platform error code when the body carries one.
#[derive(Debug)]
pub struct QqApiError {
    pub path: String,
    pub status: reqwest::StatusCode,
    pub code: Option<i64>,
    pub message: String,
}

impl std::fmt::Display for QqApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QQ API {} 返回 {}: {}", self.path, self.status, self.message)
    }
}

impl std::error::Error for QqApiError {}

/// QQ error code for a user who refuses messages from the bot.
const USER_REFUSED: i64 = 40054013;

/// Whether a failed active send may still go out as an interaction recall message.
fn recall_may_help(err: &anyhow::Error) -> bool {
    err.downcast_ref::<QqApiError>().is_some_and(|e| {
        e.status.is_client_error() && e.status != reqwest::StatusCode::UNAUTHORIZED && e.code != Some(USER_REFUSED)
    })
}

/// How a message body went out.
#[derive(Debug, PartialEq)]
enum Sent {
    Normal,
    Recall,
}

fn target_path(mt: MessageType, session_id: &str, what: &str) -> String {
    match mt {
        MessageType::Group => format!("/v2/groups/{session_id}/{what}"),
        MessageType::Private => format!("/v2/users/{session_id}/{what}"),
    }
}

struct TokenState {
    token: String,
    valid_until: Instant,
}

/// Credentials, token cache and passive reply bookkeeping of one QQ bot.
pub struct QqConn {
    pub platform_id: String,
    pub app_id: String,
    secret: String,
    http: reqwest::Client,
    logs: Arc<LogBuffer>,
    pub token_url: String,
    pub api_base: String,
    token: tokio::sync::Mutex<Option<TokenState>>,
    /// Next `msg_seq` per replied message id.
    seqs: Mutex<HashMap<String, (u32, Instant)>>,
    /// Last inbound message id per session for passive replies without an explicit target.
    last_msg: Mutex<HashMap<String, (String, Instant)>>,
    self_id: Mutex<String>,
    /// Events arrive through HTTP callbacks instead of the gateway.
    pub webhook: bool,
    /// Recent callback event ids, used to drop retried deliveries.
    seen_events: Mutex<VecDeque<String>>,
    /// Recent inbound message ids, used to drop a message pushed more than once.
    seen_msgs: Mutex<VecDeque<String>>,
    /// Event types that were ignored and already logged once.
    ignored_types: Mutex<HashSet<String>>,
}

impl QqConn {
    pub fn new(platform_id: &str, app_id: &str, secret: &str, http: reqwest::Client, logs: Arc<LogBuffer>) -> Self {
        QqConn {
            platform_id: platform_id.to_string(),
            app_id: app_id.to_string(),
            secret: secret.to_string(),
            http,
            logs,
            token_url: TOKEN_URL.to_string(),
            api_base: API_BASE.to_string(),
            token: tokio::sync::Mutex::new(None),
            seqs: Mutex::new(HashMap::new()),
            last_msg: Mutex::new(HashMap::new()),
            self_id: Mutex::new(app_id.to_string()),
            webhook: false,
            seen_events: Mutex::new(VecDeque::new()),
            seen_msgs: Mutex::new(VecDeque::new()),
            ignored_types: Mutex::new(HashSet::new()),
        }
    }

    pub fn approx_bytes(&self) -> usize {
        let a = self.seqs.lock().unwrap().len();
        let b = self.last_msg.lock().unwrap().len();
        16 * 1024 + (a + b) * 160
    }

    pub async fn access_token(&self) -> Result<String> {
        let mut g = self.token.lock().await;
        if let Some(t) = g.as_ref() {
            if Instant::now() < t.valid_until {
                return Ok(t.token.clone());
            }
        }
        let resp = self
            .http
            .post(&self.token_url)
            .timeout(Duration::from_secs(15))
            .json(&json!({"appId": self.app_id, "clientSecret": self.secret}))
            .send()
            .await
            .context("请求 QQ access token 失败")?;
        let v: Value = resp.json().await.context("QQ access token 响应解析失败")?;
        let (token, expires) = parse_token_response(&v)?;
        let margin = expires.saturating_sub(60).max(30);
        *g = Some(TokenState {
            token: token.clone(),
            valid_until: Instant::now() + Duration::from_secs(margin),
        });
        Ok(token)
    }

    async fn invalidate_token(&self) {
        *self.token.lock().await = None;
    }

    pub(crate) fn has_credentials(&self, app_id: &str, secret: &str) -> bool {
        self.app_id == app_id && self.secret == secret
    }

    async fn api(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        self.api_at(&self.api_base, method, path, body).await
    }

    pub(crate) async fn api_at(
        &self,
        base: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let token = self.access_token().await?;
        let mut req = self
            .http
            .request(method, format!("{base}{path}"))
            .header("Authorization", format!("QQBot {token}"))
            .header("X-Union-Appid", &self.app_id)
            .timeout(Duration::from_secs(20));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            if status.as_u16() == 401 {
                self.invalidate_token().await;
            }
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or(&text);
            return Err(QqApiError {
                path: path.to_string(),
                status,
                code: v.get("code").or_else(|| v.get("err_code")).and_then(Value::as_i64),
                message: msg.to_string(),
            }
            .into());
        }
        Ok(v)
    }

    /// Posts one message body. A rejected active private message is sent again as an interaction recall
    /// message (`is_wakeup`), which QQ allows once per period for 30 days after the user last chatted.
    async fn post_message(&self, mt: MessageType, path: &str, mut body: Value, active: bool) -> Result<Sent> {
        let err = match self.api(reqwest::Method::POST, path, Some(&body)).await {
            Ok(_) => return Ok(Sent::Normal),
            Err(e) => e,
        };
        if !active || mt != MessageType::Private || !recall_may_help(&err) {
            return Err(err);
        }
        body["is_wakeup"] = json!(true);
        self.api(reqwest::Method::POST, path, Some(&body))
            .await
            .with_context(|| format!("主动消息被拒（{err}），改用互动召回也没有发出"))?;
        self.logs
            .info(SRC, format!("主动消息被拒（{err}），已改用互动召回发出"));
        Ok(Sent::Recall)
    }

    pub async fn gateway_url(&self) -> Result<String> {
        let v = self.api(reqwest::Method::GET, "/gateway", None).await?;
        v.get("url")
            .and_then(|u| u.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("gateway 响应缺少 url"))
    }

    pub fn remember_inbound(&self, mt: MessageType, session_id: &str, msg_id: &str) {
        let mut g = self.last_msg.lock().unwrap();
        if g.len() > 2048 {
            let now = Instant::now();
            g.retain(|_, (_, t)| now.duration_since(*t) < REPLY_WINDOW);
        }
        g.insert(
            format!("{}:{session_id}", mt.origin_tag()),
            (msg_id.to_string(), Instant::now()),
        );
    }

    fn next_seq(&self, msg_id: Option<&str>) -> u32 {
        let Some(id) = msg_id else {
            return rand::random::<u16>() as u32 + 1;
        };
        let mut g = self.seqs.lock().unwrap();
        if g.len() > 2048 {
            let now = Instant::now();
            g.retain(|_, (_, t)| now.duration_since(*t) < REPLY_WINDOW);
        }
        let e = g.entry(id.to_string()).or_insert((0, Instant::now()));
        e.0 += 1;
        e.0
    }

    fn passive_target(&self, mt: MessageType, session_id: &str) -> Option<String> {
        let g = self.last_msg.lock().unwrap();
        g.get(&format!("{}:{session_id}", mt.origin_tag()))
            .filter(|(_, t)| t.elapsed() < REPLY_WINDOW)
            .map(|(id, _)| id.clone())
    }

    async fn upload_image(&self, mt: MessageType, session_id: &str, img: &Component) -> Result<String> {
        let Component::Image { url, path, base64 } = img else {
            return Err(anyhow!("not an image"));
        };
        let mut body = json!({"file_type": 1, "srv_send_msg": false});
        if let Some(u) = url.as_ref().filter(|u| u.starts_with("http")) {
            body["url"] = json!(u);
        } else if let Some(b) = base64.as_ref() {
            let data = b.split_once("base64,").map(|(_, d)| d).unwrap_or(b);
            body["file_data"] = json!(data);
        } else if let Some(p) = path.as_ref().or(url.as_ref()) {
            let p = p.trim_start_matches("file://");
            let bytes = tokio::fs::read(p).await.with_context(|| format!("读取图片 {p} 失败"))?;
            body["file_data"] = json!(B64.encode(bytes));
        } else {
            return Err(anyhow!("图片缺少来源"));
        }
        let v = self
            .api(
                reqwest::Method::POST,
                &target_path(mt, session_id, "files"),
                Some(&body),
            )
            .await?;
        v.get("file_info")
            .and_then(|f| f.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("上传响应缺少 file_info"))
    }

    /// Sends a chain as passive replies when a reply target is known, otherwise as active messages.
    /// A chain sent through an interaction recall stops after its first message, since QQ accepts one recall per period.
    pub async fn send(
        &self,
        mt: MessageType,
        session_id: &str,
        chain: &[Component],
        reply_to: Option<&str>,
    ) -> Result<()> {
        let reply = reply_to
            .map(str::to_string)
            .or_else(|| self.passive_target(mt, session_id));
        let reply = reply.as_deref();
        let text = chain_text(chain).trim().to_string();
        let images: Vec<&Component> = chain.iter().filter(|c| matches!(c, Component::Image { .. })).collect();
        let path = target_path(mt, session_id, "messages");
        let active = reply.is_none();
        let total = usize::from(!text.is_empty()) + images.len();
        let mut sent = 0;
        let mut recalled = false;
        if !text.is_empty() {
            let body = text_message(&text, reply, self.next_seq(reply));
            recalled = self.post_message(mt, &path, body, active).await? == Sent::Recall;
            sent += 1;
        }
        for img in images {
            if recalled {
                break;
            }
            let body = match self.upload_image(mt, session_id, img).await {
                Ok(info) => media_message(&info, " ", reply, self.next_seq(reply)),
                Err(e) => {
                    self.logs.warn(SRC, format!("图片发送失败，改为文本链接: {e}"));
                    match img {
                        Component::Image { url: Some(u), .. } if u.starts_with("http") => {
                            text_message(&format!("[图片] {u}"), reply, self.next_seq(reply))
                        }
                        _ => {
                            sent += 1;
                            continue;
                        }
                    }
                }
            };
            recalled = self.post_message(mt, &path, body, active).await? == Sent::Recall;
            sent += 1;
        }
        if recalled && sent < total {
            self.logs.warn(
                SRC,
                format!("互动召回每个周期只能发一条，这次还有 {} 段内容没有发出", total - sent),
            );
        }
        Ok(())
    }
}

enum SessionEnd {
    Reconnect,
    Reidentify,
    Fatal(String),
    Cancelled,
}

struct Session {
    id: Option<String>,
    seq: Option<u64>,
}

/// Runs the gateway connection until cancelled, reconnecting with backoff.
pub async fn run(conn: Arc<QqConn>, tx: mpsc::Sender<Inbound>, status: StatusSink, cancel: CancellationToken) {
    let mut session = Session { id: None, seq: None };
    let mut attempt = 0u32;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        status("connecting", "正在连接 QQ 网关");
        let started = Instant::now();
        let end = run_once(&conn, &tx, &status, &cancel, &mut session).await;
        if started.elapsed() > Duration::from_secs(120) {
            attempt = 0;
        }
        let wait = match end {
            Ok(SessionEnd::Cancelled) => return,
            Ok(SessionEnd::Reconnect) => backoff_secs(attempt, 60),
            Ok(SessionEnd::Reidentify) => {
                session.id = None;
                session.seq = None;
                backoff_secs(attempt + 1, 60)
            }
            Ok(SessionEnd::Fatal(msg)) => {
                conn.logs.error(SRC, msg.clone());
                status("error", &msg);
                session.id = None;
                600
            }
            Err(e) => {
                let msg = format!("{e:#}");
                conn.logs.warn(SRC, format!("连接中断: {msg}"));
                status("error", &msg);
                backoff_secs(attempt + 1, 120)
            }
        };
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

async fn run_once(
    conn: &Arc<QqConn>,
    tx: &mpsc::Sender<Inbound>,
    status: &StatusSink,
    cancel: &CancellationToken,
    session: &mut Session,
) -> Result<SessionEnd> {
    let token = conn.access_token().await?;
    let url = conn.gateway_url().await?;
    let (ws, _) = tokio::time::timeout(Duration::from_secs(20), tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| anyhow!("连接网关超时"))??;
    let (mut sink, mut stream) = ws.split();

    let hello = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .map_err(|_| anyhow!("等待 Hello 超时"))?
        .ok_or_else(|| anyhow!("网关在 Hello 前关闭"))??;
    let hello: GatewayPayload = match hello {
        WsMessage::Text(t) => serde_json::from_str(&t)?,
        other => return Err(anyhow!("意外的首帧: {other:?}")),
    };
    if hello.op != op::HELLO {
        return Err(anyhow!("首帧 op={} 不是 Hello", hello.op));
    }
    let interval = hello
        .d
        .get("heartbeat_interval")
        .and_then(|v| v.as_u64())
        .unwrap_or(41_250);
    let first = match (&session.id, session.seq) {
        (Some(id), Some(seq)) => resume_payload(&token, id, seq),
        _ => identify_payload(&token, INTENT_GROUP_AND_C2C),
    };
    sink.send(WsMessage::Text(first.to_string())).await?;

    let mut hb = tokio::time::interval(Duration::from_millis(interval.max(5_000)));
    hb.tick().await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = sink.send(WsMessage::Close(None)).await;
                return Ok(SessionEnd::Cancelled);
            }
            _ = hb.tick() => {
                sink.send(WsMessage::Text(heartbeat_payload(session.seq).to_string())).await?;
            }
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(SessionEnd::Reconnect) };
                match msg? {
                    WsMessage::Text(t) => {
                        let p: GatewayPayload = match serde_json::from_str(&t) {
                            Ok(p) => p,
                            Err(e) => { conn.logs.warn(SRC, format!("无法解析网关消息: {e}")); continue; }
                        };
                        if let Some(s) = p.s { session.seq = Some(s); }
                        match p.op {
                            op::DISPATCH => handle_dispatch(conn, tx, status, session, &p).await,
                            op::HEARTBEAT => {
                                sink.send(WsMessage::Text(heartbeat_payload(session.seq).to_string())).await?;
                            }
                            op::RECONNECT => return Ok(SessionEnd::Reconnect),
                            op::INVALID_SESSION => {
                                conn.logs.warn(SRC, "会话失效，重新鉴权");
                                return Ok(SessionEnd::Reidentify);
                            }
                            op::HEARTBEAT_ACK => {}
                            other => tracing::debug!("qq gateway op {other}"),
                        }
                    }
                    WsMessage::Ping(p) => { sink.send(WsMessage::Pong(p)).await?; }
                    WsMessage::Close(frame) => {
                        let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
                        let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                        return Ok(close_action(conn, code, &reason).await);
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn close_action(conn: &QqConn, code: u16, reason: &str) -> SessionEnd {
    conn.logs.warn(SRC, format!("网关关闭 code={code} {reason}"));
    match code {
        4004 => {
            conn.invalidate_token().await;
            SessionEnd::Reidentify
        }
        4006 | 4007 | 4009 => SessionEnd::Reidentify,
        4013 | 4014 => SessionEnd::Fatal(format!("机器人没有群聊或单聊消息权限 ({code})")),
        4914 => SessionEnd::Fatal("机器人已下架，只能连接沙箱环境".into()),
        4915 => SessionEnd::Fatal("机器人已被封禁".into()),
        _ => SessionEnd::Reconnect,
    }
}

async fn handle_dispatch(
    conn: &Arc<QqConn>,
    tx: &mpsc::Sender<Inbound>,
    status: &StatusSink,
    session: &mut Session,
    p: &GatewayPayload,
) {
    let t = p.t.as_deref().unwrap_or("");
    match t {
        "READY" => {
            session.id = p.d.get("session_id").and_then(|s| s.as_str()).map(str::to_string);
            if let Some(uid) = p.d.pointer("/user/id").and_then(|s| s.as_str()) {
                *conn.self_id.lock().unwrap() = uid.to_string();
            }
            let name = p.d.pointer("/user/username").and_then(|s| s.as_str()).unwrap_or("");
            conn.logs.info(SRC, format!("已连接 QQ 网关 {name}"));
            status("connected", "");
        }
        "RESUMED" => {
            conn.logs.info(SRC, "会话已恢复");
            status("connected", "");
        }
        _ => conn.dispatch_event(t, &p.d, tx),
    }
}

/// Records `id` in `recent` and returns false when it was already there. Keeps the latest 256 ids.
fn first_seen(recent: &Mutex<VecDeque<String>>, id: &str) -> bool {
    if id.is_empty() {
        return true;
    }
    let mut g = recent.lock().unwrap();
    if g.iter().any(|e| e == id) {
        return false;
    }
    if g.len() >= 256 {
        g.pop_front();
    }
    g.push_back(id.to_string());
    true
}

/// HTTP callback (Webhook) delivery: Ed25519 keys derived from the bot secret,
/// callback URL validation (op 13) and signed event pushes (op 0).
pub mod webhook {
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
    use serde_json::{json, Value};

    pub const OP_DISPATCH: u8 = 0;
    pub const OP_ACK: u8 = 12;
    pub const OP_VALIDATE: u8 = 13;

    /// Signing key whose seed is the secret repeated until it has at least 32 bytes, cut to 32.
    pub fn signing_key(secret: &str) -> Option<SigningKey> {
        if secret.is_empty() {
            return None;
        }
        let mut seed = secret.as_bytes().to_vec();
        while seed.len() < 32 {
            seed = seed.repeat(2);
        }
        let bytes: [u8; 32] = seed[..32].try_into().ok()?;
        Some(SigningKey::from_bytes(&bytes))
    }

    /// Verifies `X-Signature-Ed25519` over `timestamp || body`.
    pub fn verify(secret: &str, timestamp: &str, sig_hex: &str, body: &[u8]) -> bool {
        let Some(key) = signing_key(secret) else { return false };
        let Ok(raw) = hex::decode(sig_hex.trim()) else {
            return false;
        };
        let Ok(arr) = <[u8; 64]>::try_from(raw.as_slice()) else {
            return false;
        };
        if arr[63] & 224 != 0 {
            return false;
        }
        let sig = Signature::from_bytes(&arr);
        let mut msg = Vec::with_capacity(timestamp.len() + body.len());
        msg.extend_from_slice(timestamp.as_bytes());
        msg.extend_from_slice(body);
        key.verifying_key().verify(&msg, &sig).is_ok()
    }

    /// Answer to a validation request: `{plain_token, signature}` where the signature covers
    /// `event_ts || plain_token`. Only short alphanumeric tokens and numeric timestamps are signed,
    /// so the answer never covers bytes that could form a signed JSON event.
    pub fn validation_answer(secret: &str, d: &Value) -> Result<Value, String> {
        let token = d.get("plain_token").and_then(Value::as_str).unwrap_or("");
        let ts = match d.get("event_ts") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        if token.is_empty() || token.len() > 128 || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("plain_token 格式不正确".into());
        }
        if ts.is_empty() || ts.len() > 20 || !ts.bytes().all(|b| b.is_ascii_digit()) {
            return Err("event_ts 格式不正确".into());
        }
        let key = signing_key(secret).ok_or("缺少 AppSecret")?;
        let sig = key.sign(format!("{ts}{token}").as_bytes());
        Ok(json!({"plain_token": token, "signature": hex::encode(sig.to_bytes())}))
    }
}

/// Outcome of one callback delivery.
#[derive(Debug, PartialEq)]
pub enum WebhookReply {
    Ok(Value),
    Unauthorized(String),
    BadRequest(String),
}

/// Callback request fields forwarded by the console.
#[derive(Debug, Deserialize)]
pub struct WebhookDelivery {
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub app_id: String,
    pub body: String,
}

impl QqConn {
    /// Records a callback event id and returns false when it was already seen.
    fn first_delivery(&self, id: &str) -> bool {
        first_seen(&self.seen_events, id)
    }

    /// Logs notices and queues messages, each message id once. Logs the first ignored event of each type.
    fn dispatch_event(&self, t: &str, d: &Value, tx: &mpsc::Sender<Inbound>) {
        if let Some(line) = notice_text(t, d) {
            self.logs.info(SRC, line);
            return;
        }
        let self_id = self.self_id.lock().unwrap().clone();
        let Some(ev) = parse_dispatch(t, d, &self.platform_id, &self_id) else {
            if self.ignored_types.lock().unwrap().insert(t.to_string()) {
                self.logs.info(SRC, format!("收到未处理的 QQ 事件 {t}，已忽略"));
            }
            return;
        };
        if !first_seen(&self.seen_msgs, &ev.msg_id) {
            return;
        }
        self.remember_inbound(ev.message_type, &ev.session_id, &ev.msg_id);
        if tx.try_send(ev).is_err() {
            self.logs.warn(SRC, "消息队列已满，丢弃一条消息");
        }
    }

    /// Handles one callback: validates the signature, answers op 13 and queues op 0 messages.
    pub fn handle_webhook(&self, w: &WebhookDelivery, tx: &mpsc::Sender<Inbound>, status: &StatusSink) -> WebhookReply {
        if !self.webhook {
            return WebhookReply::BadRequest("该 QQ 平台使用 WebSocket 连接，未开启 Webhook".into());
        }
        if !w.app_id.is_empty() && w.app_id.trim() != self.app_id {
            return WebhookReply::Unauthorized("X-Bot-Appid 与平台不匹配".into());
        }
        let signed = !w.signature.is_empty();
        if signed && !webhook::verify(&self.secret, &w.timestamp, &w.signature, w.body.as_bytes()) {
            self.logs.warn(SRC, "收到签名无效的 Webhook 请求，已拒绝");
            return WebhookReply::Unauthorized("签名校验失败".into());
        }
        let p: GatewayPayload = match serde_json::from_str(&w.body) {
            Ok(p) => p,
            Err(e) => return WebhookReply::BadRequest(format!("无法解析回调内容: {e}")),
        };
        match p.op {
            webhook::OP_VALIDATE => match webhook::validation_answer(&self.secret, &p.d) {
                Ok(v) => {
                    self.logs.info(SRC, "已通过 QQ 开放平台的回调地址验证");
                    status("connected", "");
                    WebhookReply::Ok(v)
                }
                Err(e) => WebhookReply::BadRequest(e),
            },
            webhook::OP_DISPATCH => {
                if !signed {
                    return WebhookReply::Unauthorized("事件推送缺少签名".into());
                }
                status("connected", "");
                if !self.first_delivery(p.id.as_deref().unwrap_or("")) {
                    return WebhookReply::Ok(json!({"op": webhook::OP_ACK}));
                }
                self.dispatch_event(p.t.as_deref().unwrap_or(""), &p.d, tx);
                WebhookReply::Ok(json!({"op": webhook::OP_ACK}))
            }
            other => {
                tracing::debug!("qq webhook op {other}");
                WebhookReply::Ok(json!({"op": webhook::OP_ACK}))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_parsing() {
        let (t, e) = parse_token_response(&json!({"access_token": "abc", "expires_in": "7200"})).unwrap();
        assert_eq!((t.as_str(), e), ("abc", 7200));
        let (_, e) = parse_token_response(&json!({"access_token": "abc", "expires_in": 60})).unwrap();
        assert_eq!(e, 60);
        assert!(
            parse_token_response(&json!({"code": 100016, "message": "invalid appid"}))
                .unwrap_err()
                .to_string()
                .contains("invalid appid")
        );
    }

    #[test]
    fn gateway_payloads() {
        let p: GatewayPayload = serde_json::from_str(r#"{"op":10,"d":{"heartbeat_interval":45000}}"#).unwrap();
        assert_eq!(p.op, op::HELLO);
        assert_eq!(p.d["heartbeat_interval"], 45000);
        let id = identify_payload("tok", INTENT_GROUP_AND_C2C);
        assert_eq!(id["op"], 2);
        assert_eq!(id["d"]["token"], "QQBot tok");
        assert_eq!(id["d"]["intents"], 33554432u64);
        assert_eq!(resume_payload("tok", "sid", 7)["d"]["seq"], 7);
        assert_eq!(heartbeat_payload(None), json!({"op": 1, "d": null}));
    }

    #[test]
    fn parse_group_at_message() {
        let d = json!({
            "id": "ROBOT1.0_abc",
            "content": " <@!123>你好 ",
            "timestamp": "2024-01-01T00:00:00+08:00",
            "group_openid": "G_OPENID",
            "author": {"member_openid": "M_OPENID", "id": "M_OPENID"},
            "attachments": [{"content_type": "image/png", "url": "gchat.qpic.cn/x.png", "filename": "x.png"}]
        });
        let ev = parse_dispatch("GROUP_AT_MESSAGE_CREATE", &d, "p1", "bot").unwrap();
        assert_eq!(ev.message_type, MessageType::Group);
        assert_eq!(ev.session_id, "G_OPENID");
        assert_eq!(ev.group_id.as_deref(), Some("G_OPENID"));
        assert_eq!(ev.sender.user_id, "M_OPENID");
        assert_eq!(ev.text, "你好");
        assert_eq!(ev.msg_id, "ROBOT1.0_abc");
        assert!(ev.is_at);
        assert_eq!(
            ev.chain.last().unwrap(),
            &Component::image_url("https://gchat.qpic.cn/x.png")
        );
    }

    #[test]
    fn parse_c2c_message() {
        let d = json!({"id": "m2", "content": "hello", "author": {"user_openid": "U1"}});
        let ev = parse_dispatch("C2C_MESSAGE_CREATE", &d, "p1", "bot").unwrap();
        assert_eq!(ev.message_type, MessageType::Private);
        assert_eq!(ev.session_id, "U1");
        assert!(!ev.is_at);
        assert_eq!(ev.chain, vec![Component::plain("hello")]);
        assert!(parse_dispatch("GUILD_CREATE", &d, "p1", "bot").is_none());
        assert!(parse_dispatch("C2C_MESSAGE_CREATE", &json!({"content": "x"}), "p1", "bot").is_none());
    }

    #[test]
    fn parse_group_full_message() {
        let d = json!({
            "id": "m3",
            "content": "<@M2> 你好",
            "group_openid": "G1",
            "author": {"member_openid": "M1"},
            "mentions": [{"member_openid": "M2", "is_you": false}]
        });
        let ev = parse_dispatch("GROUP_MESSAGE_CREATE", &d, "p1", "bot").unwrap();
        assert_eq!(ev.session_id, "G1");
        assert_eq!(ev.sender.user_id, "M1");
        assert_eq!(ev.text, "你好");
        assert!(!ev.is_at);
        assert_eq!(ev.chain, vec![Component::plain("你好")]);

        let mut at = d.clone();
        at["mentions"] = json!([{"member_openid": "BOT", "bot": true, "is_you": true}]);
        let ev = parse_dispatch("GROUP_MESSAGE_CREATE", &at, "p1", "bot").unwrap();
        assert!(ev.is_at);
        assert!(matches!(&ev.chain[0], Component::At { user_id, .. } if user_id == "bot"));
    }

    #[test]
    fn dispatch_drops_repeated_messages_and_logs_ignored_types_once() {
        let c = webhook_conn();
        let (tx, mut rx) = mpsc::channel(4);
        let d = json!({"id": "m1", "content": "hi", "group_openid": "G1", "author": {"member_openid": "M1"}});
        c.dispatch_event("GROUP_AT_MESSAGE_CREATE", &d, &tx);
        c.dispatch_event("GROUP_MESSAGE_CREATE", &d, &tx);
        assert_eq!(rx.try_recv().unwrap().msg_id, "m1");
        assert!(rx.try_recv().is_err(), "same message id is queued once");

        c.dispatch_event("SOMETHING_NEW", &json!({}), &tx);
        c.dispatch_event("SOMETHING_NEW", &json!({}), &tx);
        assert!(rx.try_recv().is_err());
        let (logged, _) = c.logs.read_after(None, 10);
        let ignored: Vec<_> = logged.iter().filter(|e| e.message.contains("SOMETHING_NEW")).collect();
        assert_eq!(ignored.len(), 1);
    }

    #[test]
    fn notice_events() {
        let d = json!({"group_openid": "G1", "op_member_openid": "M1", "timestamp": 1});
        assert_eq!(
            notice_text("GROUP_MSG_REJECT", &d).as_deref(),
            Some("群 G1 关闭了机器人的主动消息（操作人 M1）")
        );
        assert_eq!(
            notice_text("GROUP_DEL_ROBOT", &d).as_deref(),
            Some("机器人被移出群 G1（操作人 M1），不会再收到这个群的消息")
        );
        assert_eq!(
            notice_text("C2C_MSG_RECEIVE", &json!({"openid": "U1"})).as_deref(),
            Some("用户 U1 开启了机器人的主动消息")
        );
        assert_eq!(
            notice_text("FRIEND_ADD", &json!({})).as_deref(),
            Some("用户 未知 添加了机器人")
        );
        assert!(notice_text("GROUP_AT_MESSAGE_CREATE", &d).is_none());
    }

    #[test]
    fn reply_bodies_and_seq() {
        let b = text_message("hi", Some("m1"), 1);
        assert_eq!(b, json!({"content": "hi", "msg_type": 0, "msg_id": "m1", "msg_seq": 1}));
        let m = media_message("INFO", " ", Some("m1"), 2);
        assert_eq!(m["msg_type"], 7);
        assert_eq!(m["media"]["file_info"], "INFO");
        let logs = Arc::new(LogBuffer::new("b", 10));
        let c = QqConn::new("p", "a", "s", reqwest::Client::new(), logs);
        assert_eq!(c.next_seq(Some("m1")), 1);
        assert_eq!(c.next_seq(Some("m1")), 2);
        assert_eq!(c.next_seq(Some("m2")), 1);
        c.remember_inbound(MessageType::Group, "g", "m9");
        assert_eq!(c.passive_target(MessageType::Group, "g").as_deref(), Some("m9"));
        assert_eq!(c.passive_target(MessageType::Private, "g"), None);
        let ago = |mins: u64| Instant::now().checked_sub(Duration::from_secs(mins * 60)).unwrap();
        c.last_msg
            .lock()
            .unwrap()
            .insert("GroupMessage:g".into(), ("m9".into(), ago(59)));
        assert_eq!(c.passive_target(MessageType::Group, "g").as_deref(), Some("m9"));
        c.last_msg
            .lock()
            .unwrap()
            .insert("GroupMessage:g".into(), ("m9".into(), ago(61)));
        assert_eq!(c.passive_target(MessageType::Group, "g"), None);
    }

    /// Serves a QQ API stand-in where active messages lack permission, and records every message body.
    async fn mock_qq() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
        use axum::extract::{Path, State};
        use axum::http::StatusCode;
        use axum::routing::post;
        use axum::Json;
        type Log = Arc<Mutex<Vec<(String, Value)>>>;
        async fn message(
            State(log): State<Log>,
            Path((kind, id)): Path<(String, String)>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            log.lock().unwrap().push((format!("{kind}/{id}"), body.clone()));
            if id == "refuser" {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"code": USER_REFUSED, "message": "用户拒收消息"})),
                );
            }
            if body.get("msg_id").is_some() || (kind == "users" && body["is_wakeup"] == true) {
                return (StatusCode::OK, Json(json!({"id": "sent"})));
            }
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"code": 40034105, "message": "主动消息发送失败，无权限"})),
            )
        }
        let log: Log = Arc::default();
        let app = axum::Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token": "t", "expires_in": 7200})) }),
            )
            .route("/v2/{kind}/{id}/messages", post(message))
            .route(
                "/v2/{kind}/{id}/files",
                post(|| async { Json(json!({"file_info": "F"})) }),
            )
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, log)
    }

    #[tokio::test]
    async fn rejected_active_private_messages_go_out_as_recalls() {
        let (base, log) = mock_qq().await;
        let logs = Arc::new(LogBuffer::new("b", 10));
        let mut c = QqConn::new("p", "a", "s", reqwest::Client::new(), logs);
        c.token_url = format!("{base}/token");
        c.api_base = base;
        let take = || std::mem::take(&mut *log.lock().unwrap());
        let image = Component::Image {
            url: None,
            path: None,
            base64: Some("aGk=".into()),
        };

        c.send(
            MessageType::Private,
            "u1",
            &[Component::plain("hi"), image.clone()],
            None,
        )
        .await
        .unwrap();
        let bodies = take();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].1.get("is_wakeup").is_none());
        assert_eq!(bodies[1].1["is_wakeup"], true);
        assert_eq!(bodies[1].1["content"], "hi");

        c.remember_inbound(MessageType::Private, "u1", "m1");
        c.send(MessageType::Private, "u1", &[Component::plain("hi"), image], None)
            .await
            .unwrap();
        let bodies = take();
        assert_eq!(bodies.len(), 2);
        assert!(bodies
            .iter()
            .all(|(_, b)| b["msg_id"] == "m1" && b.get("is_wakeup").is_none()));

        let err = c
            .send(MessageType::Group, "g1", &[Component::plain("hi")], None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("无权限"));
        assert_eq!(take().len(), 1);

        c.send(MessageType::Private, "refuser", &[Component::plain("hi")], None)
            .await
            .unwrap_err();
        assert_eq!(take().len(), 1);
    }

    fn webhook_conn() -> QqConn {
        let logs = Arc::new(LogBuffer::new("b", 10));
        let mut c = QqConn::new(
            "p1",
            "1024",
            "naOC0ocQE3shWLAfffVLB1rhYPG7",
            reqwest::Client::new(),
            logs,
        );
        c.webhook = true;
        c
    }

    fn signed(secret: &str, ts: &str, body: &str) -> WebhookDelivery {
        use ed25519_dalek::Signer;
        let key = webhook::signing_key(secret).unwrap();
        let sig = key.sign(format!("{ts}{body}").as_bytes());
        WebhookDelivery {
            timestamp: ts.into(),
            signature: hex::encode(sig.to_bytes()),
            app_id: "1024".into(),
            body: body.into(),
        }
    }

    #[test]
    fn webhook_seed_repeats_secret() {
        let a = webhook::signing_key("naOC0ocQE3shWLAfffVLB1rhYPG7").unwrap();
        let b = webhook::signing_key("naOC0ocQE3shWLAfffVLB1rhYPG7naOC").unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        assert!(webhook::signing_key("").is_none());
        let long = "x".repeat(40);
        assert_eq!(webhook::signing_key(&long).unwrap().to_bytes(), [b'x'; 32]);
    }

    #[test]
    fn webhook_signature_round_trip() {
        let secret = "naOC0ocQE3shWLAfffVLB1rhYPG7";
        let w = signed(secret, "1725442341", r#"{"op":0}"#);
        assert!(webhook::verify(secret, "1725442341", &w.signature, b"{\"op\":0}"));
        assert!(!webhook::verify(secret, "1725442342", &w.signature, b"{\"op\":0}"));
        assert!(!webhook::verify(secret, "1725442341", &w.signature, b"{\"op\":1}"));
        assert!(!webhook::verify(
            "other-secret",
            "1725442341",
            &w.signature,
            b"{\"op\":0}"
        ));
        assert!(!webhook::verify(secret, "1725442341", "zz", b"{\"op\":0}"));
    }

    #[test]
    fn webhook_validation_answer() {
        let secret = "naOC0ocQE3shWLAfffVLB1rhYPG7";
        let v = webhook::validation_answer(
            secret,
            &json!({"plain_token": "Arq0D5A61EgUu4OxUvOp", "event_ts": "1725442341"}),
        )
        .unwrap();
        assert_eq!(v["plain_token"], "Arq0D5A61EgUu4OxUvOp");
        assert_eq!(
            v["signature"],
            "3be3ed93973d9d382e3c4f0a65b57cabc9e48e3f7fcbca6c238e349f51bb5263cb24e3723053f44af5540143a04450bdbdd4824453d483c58ad82e4fb039350a"
        );
        let sig = v["signature"].as_str().unwrap();
        assert!(webhook::verify(secret, "1725442341", sig, b"Arq0D5A61EgUu4OxUvOp"));
        assert!(webhook::validation_answer(secret, &json!({"plain_token": "{\"op\":0}", "event_ts": "1"})).is_err());
        assert!(webhook::validation_answer(secret, &json!({"plain_token": "abc", "event_ts": "1a"})).is_err());
        assert!(webhook::validation_answer(secret, &json!({"plain_token": "abc", "event_ts": 1725442341})).is_ok());
    }

    #[test]
    fn webhook_delivery_flow() {
        let c = webhook_conn();
        let (tx, mut rx) = mpsc::channel(4);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let status: StatusSink = Arc::new(move |st: &str, _: &str| s2.lock().unwrap().push(st.to_string()));

        let validate = WebhookDelivery {
            timestamp: String::new(),
            signature: String::new(),
            app_id: "1024".into(),
            body: r#"{"op":13,"d":{"plain_token":"abc123","event_ts":"1725442341"}}"#.into(),
        };
        match c.handle_webhook(&validate, &tx, &status) {
            WebhookReply::Ok(v) => assert_eq!(v["plain_token"], "abc123"),
            other => panic!("{other:?}"),
        }
        assert_eq!(seen.lock().unwrap().as_slice(), ["connected"]);

        let body = r#"{"id":"ev1","op":0,"t":"C2C_MESSAGE_CREATE","d":{"id":"m1","content":"hi","author":{"user_openid":"u1"}}}"#;
        let unsigned = WebhookDelivery {
            timestamp: String::new(),
            signature: String::new(),
            app_id: "1024".into(),
            body: body.into(),
        };
        assert!(matches!(
            c.handle_webhook(&unsigned, &tx, &status),
            WebhookReply::Unauthorized(_)
        ));

        let mut forged = signed("naOC0ocQE3shWLAfffVLB1rhYPG7", "1725442341", body);
        forged.body = body.replace("hi", "bye");
        assert!(matches!(
            c.handle_webhook(&forged, &tx, &status),
            WebhookReply::Unauthorized(_)
        ));

        let ok = signed("naOC0ocQE3shWLAfffVLB1rhYPG7", "1725442341", body);
        assert_eq!(c.handle_webhook(&ok, &tx, &status), WebhookReply::Ok(json!({"op": 12})));
        let ev = rx.try_recv().expect("queued message");
        assert_eq!(ev.text, "hi");
        assert_eq!(c.handle_webhook(&ok, &tx, &status), WebhookReply::Ok(json!({"op": 12})));
        assert!(rx.try_recv().is_err(), "retried delivery is dropped");

        let mut wrong_app = signed("naOC0ocQE3shWLAfffVLB1rhYPG7", "1725442341", body);
        wrong_app.app_id = "999".into();
        assert!(matches!(
            c.handle_webhook(&wrong_app, &tx, &status),
            WebhookReply::Unauthorized(_)
        ));

        let notice = r#"{"id":"ev2","op":0,"t":"GROUP_MSG_REJECT","d":{"group_openid":"G1","op_member_openid":"M1"}}"#;
        let ok = signed("naOC0ocQE3shWLAfffVLB1rhYPG7", "1725442341", notice);
        assert_eq!(c.handle_webhook(&ok, &tx, &status), WebhookReply::Ok(json!({"op": 12})));
        assert!(rx.try_recv().is_err(), "notice is not a message");
        let (logged, _) = c.logs.read_after(None, 10);
        assert!(logged
            .iter()
            .any(|e| e.message == "群 G1 关闭了机器人的主动消息（操作人 M1）"));

        let mut ws = webhook_conn();
        ws.webhook = false;
        assert!(matches!(
            ws.handle_webhook(&ok, &tx, &status),
            WebhookReply::BadRequest(_)
        ));
    }
}
