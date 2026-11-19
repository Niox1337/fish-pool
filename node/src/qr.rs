use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::crypto;
use crate::platform::weixin::{self, QrStatus};
use crate::sign::now_ms;

pub const QQ_BIND_HOST: &str = "https://q.qq.com";
const TASK_TTL: Duration = Duration::from_secs(10 * 60);
const IDLE_STOP: Duration = Duration::from_secs(120);
const RESULT_KEEP: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Qq,
    Weixin,
}

struct Task {
    kind: Kind,
    created: Instant,
    last_query: Instant,
    finished_at: Option<Instant>,
    status: &'static str,
    message: Option<String>,
    result: Value,
}

impl Task {
    fn to_json(&self) -> Value {
        let mut v = json!({"status": self.status});
        if let Some(m) = &self.message {
            v["message"] = json!(m);
        }
        if let Value::Object(map) = &self.result {
            for (k, val) in map {
                v[k] = val.clone();
            }
        }
        v
    }
}

/// In memory QR login tasks with background pollers.
pub struct QrTasks {
    http: reqwest::Client,
    tasks: Mutex<HashMap<String, Arc<Mutex<Task>>>>,
    pub qq_host: String,
    pub wx_base: String,
}

/// Maps a QQ bind poll response to `(status, app_id, secret)` or an error message.
/// Bind status plus `(app_id, secret)` once created.
pub type BindOutcome = (&'static str, Option<(String, String)>);

pub fn qq_poll_result(v: &Value, key_b64: &str) -> std::result::Result<BindOutcome, String> {
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let status = match data.get("status") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    };
    match status {
        2 => {
            let appid = match data.get("bot_appid") {
                Some(Value::String(s)) => s.trim().to_string(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            let enc = data
                .get("bot_encrypt_secret")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if appid.is_empty() || enc.is_empty() {
                return Err("扫码成功但未返回完整凭证".into());
            }
            let secret = crypto::decrypt_b64(key_b64, &enc).map_err(|e| format!("QQ 凭证解密失败: {e}"))?;
            Ok(("created", Some((appid, secret))))
        }
        3 => Ok(("expired", None)),
        _ => Ok(("pending", None)),
    }
}

fn check_retcode(v: &Value) -> Result<()> {
    if let Some(rc) = v.get("retcode") {
        let ok = rc.as_i64() == Some(0) || rc.as_str() == Some("0");
        if !ok {
            let msg = v
                .get("msg")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("QQ 绑定接口返回失败");
            return Err(anyhow!("{msg}"));
        }
    }
    Ok(())
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl QrTasks {
    pub fn new(http: reqwest::Client) -> Self {
        QrTasks {
            http,
            tasks: Mutex::new(HashMap::new()),
            qq_host: QQ_BIND_HOST.to_string(),
            wx_base: weixin::DEFAULT_BASE.to_string(),
        }
    }

    fn prune(&self) {
        let mut g = self.tasks.lock().unwrap();
        g.retain(|_, t| {
            let t = t.lock().unwrap();
            t.created.elapsed() < TASK_TTL + RESULT_KEEP && t.finished_at.is_none_or(|f| f.elapsed() < RESULT_KEEP)
        });
    }

    fn insert(&self, kind: Kind) -> (String, Arc<Mutex<Task>>) {
        self.prune();
        let id = ulid::Ulid::new().to_string().to_lowercase();
        let now = Instant::now();
        let t = Arc::new(Mutex::new(Task {
            kind,
            created: now,
            last_query: now,
            finished_at: None,
            status: "pending",
            message: None,
            result: Value::Null,
        }));
        self.tasks.lock().unwrap().insert(id.clone(), t.clone());
        (id, t)
    }

    pub fn get(&self, kind: Kind, id: &str) -> Option<Value> {
        let t = self.tasks.lock().unwrap().get(id).cloned()?;
        let mut g = t.lock().unwrap();
        if g.kind != kind {
            return None;
        }
        g.last_query = Instant::now();
        if g.finished_at.is_none() && g.created.elapsed() > TASK_TTL {
            g.status = "expired";
            g.finished_at = Some(Instant::now());
        }
        Some(g.to_json())
    }

    pub async fn create_qq(self: &Arc<Self>) -> Result<Value> {
        let key = crypto::random_key_b64();
        let resp = self
            .http
            .post(format!("{}/lite/create_bind_task", self.qq_host))
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(10))
            .json(&json!({"key": key}))
            .send()
            .await
            .context("请求 QQ 绑定任务失败")?;
        let v: Value = resp.json().await.context("QQ 绑定任务响应解析失败")?;
        check_retcode(&v)?;
        let remote = v
            .pointer("/data/task_id")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if remote.is_empty() {
            return Err(anyhow!("QQ 绑定任务响应缺少 task_id"));
        }
        let host = self
            .qq_host
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let qr_url = format!(
            "https://{host}/qqbot/openclaw/connect.html?task_id={}&_wv=2",
            url_encode(&remote)
        );
        let (id, task) = self.insert(Kind::Qq);
        let this = self.clone();
        tokio::spawn(async move { this.poll_qq(task, remote, key).await });
        Ok(json!({
            "task_id": id, "qr_url": qr_url, "interval_s": 2,
            "expires_at": now_ms() + TASK_TTL.as_millis() as i64,
        }))
    }

    async fn poll_qq(&self, task: Arc<Mutex<Task>>, remote: String, key: String) {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            {
                let g = task.lock().unwrap();
                if g.finished_at.is_some() || g.last_query.elapsed() > IDLE_STOP || g.created.elapsed() > TASK_TTL {
                    return;
                }
            }
            let res = async {
                let resp = self
                    .http
                    .post(format!("{}/lite/poll_bind_result", self.qq_host))
                    .header("Accept", "application/json")
                    .timeout(Duration::from_secs(10))
                    .json(&json!({"task_id": remote}))
                    .send()
                    .await?;
                let v: Value = resp.json().await?;
                check_retcode(&v)?;
                Ok::<Value, anyhow::Error>(v)
            }
            .await;
            let mut g = task.lock().unwrap();
            match res {
                Ok(v) => match qq_poll_result(&v, &key) {
                    Ok(("pending", _)) => {}
                    Ok((status, creds)) => {
                        g.status = status;
                        if let Some((app_id, secret)) = creds {
                            g.result = json!({"app_id": app_id, "secret": secret});
                        }
                        g.finished_at = Some(Instant::now());
                        return;
                    }
                    Err(msg) => {
                        g.status = "error";
                        g.message = Some(msg);
                        g.finished_at = Some(Instant::now());
                        return;
                    }
                },
                Err(e) => tracing::debug!("qq bind poll: {e:#}"),
            }
        }
    }

    pub async fn create_weixin(self: &Arc<Self>) -> Result<Value> {
        let base = weixin::normalize_base(&self.wx_base);
        let url = format!("{base}/ilink/bot/get_bot_qrcode");
        let call = weixin::Call {
            method: reqwest::Method::GET,
            url: &url,
            token: None,
            query: &[("bot_type", weixin::BOT_TYPE)],
            body: None,
            timeout: Duration::from_secs(15),
            headers: &[],
        };
        let v = weixin::request_json(&self.http, call).await?;
        let (qrcode, img) = weixin::parse_qr(&v)?;
        let (id, task) = self.insert(Kind::Weixin);
        let this = self.clone();
        tokio::spawn(async move { this.poll_weixin(task, base, qrcode).await });
        Ok(json!({
            "task_id": id, "qr_url": img, "interval_s": 2,
            "expires_at": now_ms() + 5 * 60 * 1000,
        }))
    }

    async fn poll_weixin(&self, task: Arc<Mutex<Task>>, base: String, qrcode: String) {
        let url = format!("{base}/ilink/bot/get_qrcode_status");
        loop {
            {
                let g = task.lock().unwrap();
                if g.finished_at.is_some() || g.last_query.elapsed() > IDLE_STOP || g.created.elapsed() > TASK_TTL {
                    return;
                }
            }
            let call = weixin::Call {
                method: reqwest::Method::GET,
                url: &url,
                token: None,
                query: &[("qrcode", qrcode.as_str())],
                body: None,
                timeout: Duration::from_secs(40),
                headers: &[("iLink-App-ClientVersion", "1")],
            };
            let res = weixin::request_json(&self.http, call).await;
            let status = match res {
                Ok(v) => weixin::parse_qr_status(&v, &base),
                Err(e) => {
                    let timeout = e.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout());
                    if !timeout {
                        tracing::debug!("weixin qr poll: {e:#}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    continue;
                }
            };
            let finished = {
                let mut g = task.lock().unwrap();
                match status {
                    QrStatus::Pending => false,
                    QrStatus::Scanned => {
                        g.status = "scanned";
                        false
                    }
                    QrStatus::Expired => {
                        g.status = "expired";
                        g.finished_at = Some(Instant::now());
                        true
                    }
                    QrStatus::Denied => {
                        g.status = "error";
                        g.message = Some("用户取消登录".into());
                        g.finished_at = Some(Instant::now());
                        true
                    }
                    QrStatus::Error(m) => {
                        g.status = "error";
                        g.message = Some(m);
                        g.finished_at = Some(Instant::now());
                        true
                    }
                    QrStatus::Confirmed {
                        token,
                        bot_id,
                        base_url,
                        user_id,
                    } => {
                        g.status = "created";
                        g.result = json!({"bot_id": bot_id, "token": token, "base_url": base_url, "user_id": user_id});
                        g.finished_at = Some(Instant::now());
                        true
                    }
                }
            };
            if finished {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qq_bind_result_mapping() {
        let key = crypto::random_key_b64();
        let enc = crypto::encrypt_b64(&key, "SECRET123").unwrap();
        let v = json!({"retcode": 0, "data": {"status": 2, "bot_appid": "102000", "bot_encrypt_secret": enc}});
        assert_eq!(
            qq_poll_result(&v, &key).unwrap(),
            ("created", Some(("102000".into(), "SECRET123".into())))
        );
        assert_eq!(
            qq_poll_result(&json!({"data": {"status": 1}}), &key).unwrap(),
            ("pending", None)
        );
        assert_eq!(
            qq_poll_result(&json!({"data": {"status": 3}}), &key).unwrap(),
            ("expired", None)
        );
        assert!(qq_poll_result(&json!({"data": {"status": 2, "bot_appid": "1"}}), &key).is_err());
        let wrong = crypto::random_key_b64();
        assert!(qq_poll_result(&v, &wrong).is_err());
        assert!(check_retcode(&json!({"retcode": 1, "msg": "bad"})).is_err());
        assert!(check_retcode(&json!({"retcode": 0})).is_ok());
        assert_eq!(url_encode("a b/c"), "a%20b%2Fc");
    }
}
