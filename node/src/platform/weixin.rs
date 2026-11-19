use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{backoff_secs, Inbound, StatusSink};
use crate::logs::LogBuffer;
use crate::model::{chain_text, Component, MessageType, Sender, PLATFORM_WEIXIN};
use crate::sign::now_ms;
use crate::store::NodeDb;

pub const DEFAULT_BASE: &str = "https://ilinkai.weixin.qq.com";
pub const BOT_TYPE: &str = "3";
pub const CHANNEL_VERSION: &str = "fish-node/0.1";
pub const SESSION_TIMEOUT_ERRCODE: i64 = -14;
pub const ITEM_TEXT: i64 = 1;
pub const ITEM_IMAGE: i64 = 2;
pub const ITEM_VOICE: i64 = 3;
pub const ITEM_FILE: i64 = 4;
pub const ITEM_VIDEO: i64 = 5;
/// `message_type` value of messages authored by the bot itself.
pub const MSG_TYPE_BOT: i64 = 2;
const CONTEXT_TTL_MS: i64 = 24 * 3600 * 1000;
const LONG_POLL: Duration = Duration::from_secs(40);
const SRC: &str = "weixin_ilink";

pub fn normalize_base(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.is_empty() {
        DEFAULT_BASE.to_string()
    } else {
        b.to_string()
    }
}

/// Request headers of the iLink bot API. `X-WECHAT-UIN` is base64 of a random decimal u32.
pub fn headers(token: Option<&str>) -> Vec<(&'static str, String)> {
    let uin = B64.encode(rand::random::<u32>().to_string());
    let mut h = vec![
        ("Content-Type", "application/json".to_string()),
        ("AuthorizationType", "ilink_bot_token".to_string()),
        ("X-WECHAT-UIN", uin),
    ];
    if let Some(t) = token {
        h.push(("Authorization", format!("Bearer {t}")));
    }
    h
}

fn str_field(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn int_field(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

pub fn api_ok(v: &Value) -> bool {
    int_field(v, "ret") == 0 && int_field(v, "errcode") == 0
}

pub fn api_error(v: &Value) -> String {
    format!(
        "ret={} errcode={} errmsg={}",
        int_field(v, "ret"),
        int_field(v, "errcode"),
        str_field(v, "errmsg")
    )
}

/// Parses the QR response into `(qrcode, qrcode_img_content)`.
pub fn parse_qr(v: &Value) -> Result<(String, String)> {
    let q = str_field(v, "qrcode");
    let img = str_field(v, "qrcode_img_content");
    if q.is_empty() || img.is_empty() {
        return Err(anyhow!("微信二维码响应格式异常"));
    }
    Ok((q, img))
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum QrStatus {
    Pending,
    Scanned,
    Expired,
    Denied,
    Confirmed {
        token: String,
        bot_id: String,
        base_url: String,
        user_id: String,
    },
    Error(String),
}

pub fn parse_qr_status(v: &Value, default_base: &str) -> QrStatus {
    let s = str_field(v, "status");
    match s.as_str() {
        "confirmed" => {
            let token = str_field(v, "bot_token");
            if token.is_empty() {
                return QrStatus::Error("登录成功但未返回 token".into());
            }
            let base = str_field(v, "baseurl");
            QrStatus::Confirmed {
                token,
                bot_id: str_field(v, "ilink_bot_id"),
                base_url: normalize_base(if base.is_empty() { default_base } else { &base }),
                user_id: str_field(v, "ilink_user_id"),
            }
        }
        "scaned" | "scanned" => QrStatus::Scanned,
        "expired" => QrStatus::Expired,
        "cancel" | "canceled" | "denied" => QrStatus::Denied,
        _ => QrStatus::Pending,
    }
}

#[derive(Debug, Default)]
pub struct Updates {
    pub buf: Option<String>,
    pub msgs: Vec<Value>,
}

pub fn parse_updates(v: &Value) -> std::result::Result<Updates, (i64, String)> {
    if !api_ok(v) {
        return Err((int_field(v, "errcode"), api_error(v)));
    }
    let buf = Some(str_field(v, "get_updates_buf")).filter(|b| !b.is_empty());
    let msgs = v.get("msgs").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    Ok(Updates { buf, msgs })
}

fn item_text(item: &Value) -> Option<String> {
    match int_field(item, "type") {
        ITEM_TEXT => item
            .pointer("/text_item/text")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        ITEM_VOICE => item
            .pointer("/voice_item/text")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        _ => None,
    }
}

/// Converts one inbound iLink message into `(Inbound, context_token)`.
pub fn parse_message(msg: &Value, platform_id: &str, self_id: &str) -> Option<(Inbound, String)> {
    if int_field(msg, "message_type") == MSG_TYPE_BOT {
        return None;
    }
    let from = str_field(msg, "from_user_id");
    if from.is_empty() {
        return None;
    }
    let ctx = str_field(msg, "context_token");
    let mut text_parts = vec![];
    let mut chain = vec![];
    for item in msg.get("item_list").and_then(|l| l.as_array()).into_iter().flatten() {
        if let Some(t) = item_text(item) {
            let t = t.trim().to_string();
            if !t.is_empty() {
                chain.push(Component::plain(t.clone()));
                text_parts.push(t);
            }
            continue;
        }
        // shortcut: inbound media becomes a text placeholder, upgrade by downloading and decrypting CDN media
        let placeholder = match int_field(item, "type") {
            ITEM_IMAGE => "[图片]",
            ITEM_VOICE => "[语音]",
            ITEM_FILE => "[文件]",
            ITEM_VIDEO => "[视频]",
            _ => continue,
        };
        chain.push(Component::plain(placeholder));
    }
    let text = text_parts.join("\n");
    let mut msg_id = str_field(msg, "message_id");
    if msg_id.is_empty() {
        msg_id = str_field(msg, "msg_id");
    }
    if msg_id.is_empty() {
        msg_id = ulid::Ulid::new().to_string().to_lowercase();
    }
    Some((
        Inbound {
            platform_id: platform_id.to_string(),
            platform_kind: PLATFORM_WEIXIN,
            self_id: self_id.to_string(),
            message_type: MessageType::Private,
            group_id: None,
            session_id: from.clone(),
            sender: Sender {
                user_id: from.clone(),
                nickname: from,
            },
            text,
            chain,
            is_at: false,
            msg_id,
            raw: msg.clone(),
        },
        ctx,
    ))
}

pub fn send_body(to_user: &str, context_token: &str, text: &str) -> Value {
    json!({
        "base_info": {"channel_version": CHANNEL_VERSION},
        "msg": {
            "from_user_id": "",
            "to_user_id": to_user,
            "client_id": ulid::Ulid::new().to_string().to_lowercase(),
            "message_type": MSG_TYPE_BOT,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{"type": ITEM_TEXT, "text_item": {"text": text}}]
        }
    })
}

/// One iLink HTTP call.
pub struct Call<'a> {
    pub method: reqwest::Method,
    pub url: &'a str,
    pub token: Option<&'a str>,
    pub query: &'a [(&'a str, &'a str)],
    pub body: Option<&'a Value>,
    pub timeout: Duration,
    pub headers: &'a [(&'a str, &'a str)],
}

pub async fn request_json(http: &reqwest::Client, c: Call<'_>) -> Result<Value> {
    let url = c.url;
    let mut req = http.request(c.method, url).timeout(c.timeout).query(c.query);
    for (k, v) in headers(c.token) {
        req = req.header(k, v);
    }
    for (k, v) in c.headers {
        req = req.header(*k, *v);
    }
    if let Some(b) = c.body {
        req = req.json(b);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "iLink {url} 返回 {status}: {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(&text)?)
}

/// Credentials of a logged in iLink bot account.
pub struct WxAccount {
    pub account_id: String,
    pub token: String,
    pub base_url: String,
}

#[cfg(test)]
fn test_account() -> WxAccount {
    WxAccount {
        account_id: "acc".into(),
        token: "t".into(),
        base_url: String::new(),
    }
}

/// Running iLink account: token, sync cursor and per user context tokens.
pub struct WxConn {
    pub platform_id: String,
    pub bot_id: String,
    pub account_id: String,
    token: String,
    pub base_url: String,
    http: reqwest::Client,
    db: Arc<NodeDb>,
    logs: Arc<LogBuffer>,
    contexts: Mutex<HashMap<String, (String, i64)>>,
}

impl WxConn {
    pub fn new(
        bot_id: &str,
        platform_id: &str,
        account: WxAccount,
        http: reqwest::Client,
        db: Arc<NodeDb>,
        logs: Arc<LogBuffer>,
    ) -> Self {
        let c = WxConn {
            platform_id: platform_id.to_string(),
            bot_id: bot_id.to_string(),
            account_id: account.account_id,
            token: account.token,
            base_url: normalize_base(&account.base_url),
            http,
            db,
            logs,
            contexts: Mutex::new(HashMap::new()),
        };
        if let Some(saved) = c.db.kv_get(&c.key("ctx")) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, (String, i64)>>(&saved) {
                *c.contexts.lock().unwrap() = map;
            }
        }
        c
    }

    fn key(&self, what: &str) -> String {
        format!("bot:{}:wx:{}:{what}", self.bot_id, self.platform_id)
    }

    pub fn approx_bytes(&self) -> usize {
        8 * 1024
            + self
                .contexts
                .lock()
                .unwrap()
                .values()
                .map(|(t, _)| t.len() + 96)
                .sum::<usize>()
    }

    fn url(&self, endpoint: &str) -> String {
        format!("{}/{endpoint}", self.base_url)
    }

    pub fn set_context(&self, user: &str, token: &str) {
        if token.is_empty() {
            return;
        }
        let snapshot = {
            let mut g = self.contexts.lock().unwrap();
            if g.get(user).map(|(t, _)| t.as_str()) == Some(token) {
                if let Some(e) = g.get_mut(user) {
                    e.1 = now_ms();
                }
                return;
            }
            let cutoff = now_ms() - CONTEXT_TTL_MS;
            g.retain(|_, (_, ts)| *ts > cutoff);
            g.insert(user.to_string(), (token.to_string(), now_ms()));
            serde_json::to_string(&*g).unwrap_or_default()
        };
        self.db.kv_set(&self.key("ctx"), &snapshot);
    }

    fn context_for(&self, user: &str) -> Option<String> {
        let g = self.contexts.lock().unwrap();
        g.get(user)
            .filter(|(_, ts)| now_ms() - ts < CONTEXT_TTL_MS)
            .map(|(t, _)| t.clone())
    }

    pub async fn send(&self, user: &str, chain: &[Component]) -> Result<()> {
        let mut text = chain_text(chain).trim().to_string();
        for c in chain {
            if let Component::Image { url, .. } = c {
                // shortcut: outbound images become text links, upgrade by implementing the encrypted CDN upload
                self.logs.warn(SRC, "微信暂不支持发送图片，已改为文本链接");
                if let Some(u) = url.as_ref().filter(|u| u.starts_with("http")) {
                    text.push_str(&format!("\n[图片] {u}"));
                }
            }
        }
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let ctx = self
            .context_for(user)
            .ok_or_else(|| anyhow!("缺少 {user} 的 context_token，需要对方先发送消息（24 小时内有效）"))?;
        let url = self.url("ilink/bot/sendmessage");
        let body = send_body(user, &ctx, text);
        let call = Call {
            method: reqwest::Method::POST,
            url: &url,
            token: Some(&self.token),
            query: &[],
            body: Some(&body),
            timeout: Duration::from_secs(15),
            headers: &[],
        };
        let v = request_json(&self.http, call).await?;
        if !api_ok(&v) {
            return Err(anyhow!("发送失败: {}", api_error(&v)));
        }
        Ok(())
    }
}

/// Long polls updates until cancelled. A session timeout stops the loop with an error status.
pub async fn run(conn: Arc<WxConn>, tx: mpsc::Sender<Inbound>, status: StatusSink, cancel: CancellationToken) {
    let mut buf = conn.db.kv_get(&conn.key("buf")).unwrap_or_default();
    let mut failures = 0u32;
    status("connecting", "正在连接微信");
    let mut connected = false;
    loop {
        let body = json!({"base_info": {"channel_version": CHANNEL_VERSION}, "get_updates_buf": buf});
        let url = conn.url("ilink/bot/getupdates");
        let call = Call {
            method: reqwest::Method::POST,
            url: &url,
            token: Some(&conn.token),
            query: &[],
            body: Some(&body),
            timeout: LONG_POLL,
            headers: &[],
        };
        let fut = request_json(&conn.http, call);
        let res = tokio::select! {
            _ = cancel.cancelled() => return,
            r = fut => r,
        };
        let wait = match res {
            Ok(v) => match parse_updates(&v) {
                Ok(u) => {
                    failures = 0;
                    if !connected {
                        connected = true;
                        conn.logs.info(SRC, "微信消息通道已连接");
                        status("connected", "");
                    }
                    if let Some(b) = u.buf {
                        if b != buf {
                            buf = b;
                            conn.db.kv_set(&conn.key("buf"), &buf);
                        }
                    }
                    for m in &u.msgs {
                        if let Some((ev, ctx)) = parse_message(m, &conn.platform_id, &conn.account_id) {
                            conn.set_context(&ev.session_id, &ctx);
                            if tx.try_send(ev).is_err() {
                                conn.logs.warn(SRC, "消息队列已满，丢弃一条消息");
                            }
                        }
                    }
                    0
                }
                Err((code, msg)) if code == SESSION_TIMEOUT_ERRCODE => {
                    conn.logs.error(SRC, format!("微信登录已失效，请重新扫码绑定 ({msg})"));
                    status("error", "微信登录已失效，请重新扫码绑定");
                    conn.db.kv_delete(&conn.key("buf"));
                    cancel.cancelled().await;
                    return;
                }
                Err((_, msg)) => {
                    failures += 1;
                    conn.logs.warn(SRC, format!("拉取消息失败: {msg}"));
                    5
                }
            },
            Err(e) => {
                let is_timeout = e.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout());
                if is_timeout {
                    0
                } else {
                    failures += 1;
                    connected = false;
                    conn.logs.warn(SRC, format!("拉取消息失败: {e:#}"));
                    status("error", &format!("{e:#}"));
                    backoff_secs(failures, 60)
                }
            }
        };
        if wait > 0 {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_parsing() {
        let (q, img) =
            parse_qr(&json!({"qrcode": "abc", "qrcode_img_content": "https://liteapp.weixin.qq.com/q/x"})).unwrap();
        assert_eq!(q, "abc");
        assert!(img.starts_with("https://"));
        assert!(parse_qr(&json!({"qrcode": "abc"})).is_err());
        assert_eq!(
            parse_qr_status(&json!({"status": "wait"}), DEFAULT_BASE),
            QrStatus::Pending
        );
        assert_eq!(
            parse_qr_status(&json!({"status": "scaned"}), DEFAULT_BASE),
            QrStatus::Scanned
        );
        assert_eq!(
            parse_qr_status(&json!({"status": "expired"}), DEFAULT_BASE),
            QrStatus::Expired
        );
        let s = parse_qr_status(
            &json!({"status": "confirmed", "bot_token": "T", "ilink_bot_id": "B@im.bot", "ilink_user_id": "U", "baseurl": "https://x/"}),
            DEFAULT_BASE,
        );
        assert_eq!(
            s,
            QrStatus::Confirmed {
                token: "T".into(),
                bot_id: "B@im.bot".into(),
                base_url: "https://x".into(),
                user_id: "U".into()
            }
        );
        assert!(matches!(
            parse_qr_status(&json!({"status": "confirmed"}), DEFAULT_BASE),
            QrStatus::Error(_)
        ));
    }

    #[test]
    fn updates_and_messages() {
        let v = json!({"ret": 0, "get_updates_buf": "BUF2", "msgs": [
            {"from_user_id": "u1@im.wechat", "to_user_id": "bot", "message_type": 1, "context_token": "CTX",
             "message_id": 42, "item_list": [
                {"type": 1, "text_item": {"text": " 你好 "}},
                {"type": 2, "image_item": {"media": {"encrypt_query_param": "x"}}},
                {"type": 3, "voice_item": {"text": "语音转文字"}}
             ]},
            {"from_user_id": "bot", "message_type": 2, "item_list": []}
        ]});
        let u = parse_updates(&v).unwrap();
        assert_eq!(u.buf.as_deref(), Some("BUF2"));
        assert_eq!(u.msgs.len(), 2);
        let (ev, ctx) = parse_message(&u.msgs[0], "p1", "bot").unwrap();
        assert_eq!(ctx, "CTX");
        assert_eq!(ev.session_id, "u1@im.wechat");
        assert_eq!(ev.msg_id, "42");
        assert_eq!(ev.text, "你好\n语音转文字");
        assert_eq!(ev.chain[1], Component::plain("[图片]"));
        assert_eq!(ev.message_type, MessageType::Private);
        assert!(parse_message(&u.msgs[1], "p1", "bot").is_none());
        let err = parse_updates(&json!({"ret": -1, "errcode": -14, "errmsg": "session timeout"})).unwrap_err();
        assert_eq!(err.0, SESSION_TIMEOUT_ERRCODE);
    }

    #[test]
    fn send_body_shape() {
        let b = send_body("u1", "CTX", "hi");
        assert_eq!(b["msg"]["to_user_id"], "u1");
        assert_eq!(b["msg"]["context_token"], "CTX");
        assert_eq!(b["msg"]["message_type"], 2);
        assert_eq!(b["msg"]["item_list"][0]["text_item"]["text"], "hi");
        let h = headers(Some("T"));
        assert!(h.iter().any(|(k, v)| *k == "Authorization" && v == "Bearer T"));
        let uin = &h.iter().find(|(k, _)| *k == "X-WECHAT-UIN").unwrap().1;
        let decoded = String::from_utf8(B64.decode(uin).unwrap()).unwrap();
        assert!(decoded.parse::<u32>().is_ok());
    }

    #[test]
    fn context_tokens_persist() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(NodeDb::open(&dir.path().join("n.db")).unwrap());
        let logs = Arc::new(LogBuffer::new("b", 10));
        let c = WxConn::new(
            "b",
            "p",
            test_account(),
            reqwest::Client::new(),
            db.clone(),
            logs.clone(),
        );
        c.set_context("u1", "CTX1");
        assert_eq!(c.context_for("u1").as_deref(), Some("CTX1"));
        let c2 = WxConn::new("b", "p", test_account(), reqwest::Client::new(), db, logs);
        assert_eq!(c2.context_for("u1").as_deref(), Some("CTX1"));
        assert_eq!(c2.base_url, DEFAULT_BASE);
    }
}
