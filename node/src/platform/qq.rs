use std::collections::HashMap;
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
const GROUP_REPLY_WINDOW: Duration = Duration::from_secs(5 * 60);
const C2C_REPLY_WINDOW: Duration = Duration::from_secs(60 * 60);

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

/// Converts a QQ dispatch event into an `Inbound` when it is a group @ or C2C message.
pub fn parse_dispatch(t: &str, d: &Value, platform_id: &str, self_id: &str) -> Option<Inbound> {
    let (mt, group_id, sender_id) = match t {
        "GROUP_AT_MESSAGE_CREATE" => {
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
    let text = strip_mentions(d.get("content").and_then(|c| c.as_str()).unwrap_or(""));
    let mut chain = vec![];
    if mt == MessageType::Group {
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
        is_at: mt == MessageType::Group,
        msg_id,
        raw: d.clone(),
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

    async fn api(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let token = self.access_token().await?;
        let mut req = self
            .http
            .request(method, format!("{}{}", self.api_base, path))
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
            return Err(anyhow!("QQ API {path} 返回 {status}: {msg}"));
        }
        Ok(v)
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
            g.retain(|_, (_, t)| now.duration_since(*t) < C2C_REPLY_WINDOW);
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
            g.retain(|_, (_, t)| now.duration_since(*t) < C2C_REPLY_WINDOW);
        }
        let e = g.entry(id.to_string()).or_insert((0, Instant::now()));
        e.0 += 1;
        e.0
    }

    fn passive_target(&self, mt: MessageType, session_id: &str) -> Option<String> {
        let window = if mt == MessageType::Group {
            GROUP_REPLY_WINDOW
        } else {
            C2C_REPLY_WINDOW
        };
        let g = self.last_msg.lock().unwrap();
        g.get(&format!("{}:{session_id}", mt.origin_tag()))
            .filter(|(_, t)| t.elapsed() < window)
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
        if !text.is_empty() {
            let body = text_message(&text, reply, self.next_seq(reply));
            self.api(reqwest::Method::POST, &path, Some(&body)).await?;
        }
        for img in images {
            match self.upload_image(mt, session_id, img).await {
                Ok(info) => {
                    let body = media_message(&info, " ", reply, self.next_seq(reply));
                    self.api(reqwest::Method::POST, &path, Some(&body)).await?;
                }
                Err(e) => {
                    self.logs.warn(SRC, format!("图片发送失败，改为文本链接: {e}"));
                    if let Component::Image { url: Some(u), .. } = img {
                        if u.starts_with("http") {
                            let body = text_message(&format!("[图片] {u}"), reply, self.next_seq(reply));
                            self.api(reqwest::Method::POST, &path, Some(&body)).await?;
                        }
                    }
                }
            }
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
        _ => {
            let self_id = conn.self_id.lock().unwrap().clone();
            if let Some(ev) = parse_dispatch(t, &p.d, &conn.platform_id, &self_id) {
                conn.remember_inbound(ev.message_type, &ev.session_id, &ev.msg_id);
                if tx.try_send(ev).is_err() {
                    conn.logs.warn(SRC, "消息队列已满，丢弃一条消息");
                }
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
    }
}
