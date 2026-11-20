use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::sidecar::Sidecar;
use crate::model::{Component, MessageEvent};
use crate::sandbox::Cgroups;

#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Handler {
    pub id: String,
    pub kind: String,
    pub pattern: Option<String>,
    pub priority: i64,
    /// `{"name", "description", "parameters"}` for `llm_tool` handlers.
    pub tool: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Hello {
    pub name: String,
    pub version: String,
    pub handlers: Vec<Handler>,
    pub wake_commands: Option<Vec<String>>,
}

/// Line oriented writer to one plugin process.
#[derive(Clone)]
pub struct ProcWriter(mpsc::UnboundedSender<String>);

impl ProcWriter {
    pub fn send(&self, v: &Value) -> bool {
        self.0.send(v.to_string()).is_ok()
    }
}

/// Requests a plugin process makes to the node.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginOut {
    Send {
        event_id: Option<String>,
        session: Option<String>,
        chain: Vec<Component>,
    },
    SendTo {
        session: String,
        chain: Vec<Component>,
    },
    Llm {
        req_id: String,
        prompt: Option<String>,
        system_prompt: Option<String>,
        contexts: Vec<Value>,
        image_urls: Vec<String>,
        tools: Vec<Value>,
    },
    Log {
        level: String,
        message: String,
    },
}

pub struct OutMsg {
    pub plugin_id: String,
    pub plugin_name: String,
    pub writer: ProcWriter,
    pub msg: PluginOut,
}

#[derive(Default)]
struct Waiter {
    matched: Option<oneshot::Sender<bool>>,
    done: Option<oneshot::Sender<(bool, bool)>>,
    hook: Option<oneshot::Sender<Value>>,
}

/// A connected plugin process.
pub struct PluginProc {
    pub proc_id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub pid: u32,
    pub hello: Hello,
    pub(crate) activity: watch::Sender<Instant>,
    writer: ProcWriter,
    waiters: Arc<Mutex<HashMap<String, Waiter>>>,
    /// Cancelled when the plugin socket closes.
    pub closed: CancellationToken,
    /// Last `error` message reported by the plugin.
    pub last_error: Arc<Mutex<Option<String>>>,
}

pub struct LaunchSpec {
    pub proc_id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub bot_id: String,
    pub plugin_dir: PathBuf,
    pub venv_site_packages: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub config: Value,
    /// Shared install name. Plugin processes with the same name may be forked from one template.
    pub share_key: Option<String>,
}

fn parse_chain(v: Option<&Value>) -> Vec<Component> {
    v.and_then(|c| serde_json::from_value::<Vec<Component>>(c.clone()).ok())
        .unwrap_or_default()
}

fn str_opt(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(str::to_string)
}

/// Parses a plugin to node message into a `PluginOut` request when it is one.
pub fn parse_out(v: &Value) -> Option<PluginOut> {
    match v.get("op")?.as_str()? {
        "send" => Some(PluginOut::Send {
            event_id: str_opt(v, "event_id"),
            session: str_opt(v, "session"),
            chain: parse_chain(v.get("chain")),
        }),
        "send_to" => Some(PluginOut::SendTo {
            session: str_opt(v, "session")?,
            chain: parse_chain(v.get("chain")),
        }),
        "llm" => Some(PluginOut::Llm {
            req_id: str_opt(v, "req_id")?,
            prompt: str_opt(v, "prompt"),
            system_prompt: str_opt(v, "system_prompt"),
            contexts: v
                .get("contexts")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default(),
            image_urls: v
                .get("image_urls")
                .and_then(|c| c.as_array())
                .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            tools: v.get("tools").and_then(Value::as_array).cloned().unwrap_or_default(),
        }),
        "log" => Some(PluginOut::Log {
            level: str_opt(v, "level").unwrap_or_else(|| "info".into()),
            message: str_opt(v, "message").unwrap_or_default(),
        }),
        _ => None,
    }
}

impl PluginProc {
    /// Sends an event and returns receivers for the `matched` and `done` replies.
    pub fn dispatch(
        &self,
        event_id: &str,
        event: &MessageEvent,
    ) -> (oneshot::Receiver<bool>, oneshot::Receiver<(bool, bool)>) {
        let (mtx, mrx) = oneshot::channel();
        let (dtx, drx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(
            event_id.to_string(),
            Waiter {
                matched: Some(mtx),
                done: Some(dtx),
                hook: None,
            },
        );
        if !self
            .writer
            .send(&json!({"op": "event", "event_id": event_id, "event": event}))
        {
            self.waiters.lock().unwrap().remove(event_id);
        }
        (mrx, drx)
    }

    pub fn forget_event(&self, event_id: &str) {
        self.waiters.lock().unwrap().remove(event_id);
    }

    /// Runs the plugin's `llm_request` hook and returns the possibly modified request.
    pub async fn llm_hook(&self, event_id: &str, request: Value, timeout: Duration) -> Option<Value> {
        let key = format!("hook:{event_id}");
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(
            key.clone(),
            Waiter {
                hook: Some(tx),
                ..Default::default()
            },
        );
        self.writer
            .send(&json!({"op": "llm_hook", "event_id": event_id, "request": request}));
        let r = tokio::time::timeout(timeout, rx).await.ok().and_then(|r| r.ok());
        self.waiters.lock().unwrap().remove(&key);
        r
    }

    /// Runs an `llm_tool` handler. Returns the reply `{"text", "sent"}` or `{"error"}`, or None on timeout.
    pub async fn tool_call(
        &self,
        call_id: &str,
        event_id: &str,
        event: &MessageEvent,
        name: &str,
        arguments: &Value,
        timeout: Duration,
    ) -> Option<Value> {
        let key = format!("tool:{call_id}");
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(
            key.clone(),
            Waiter {
                hook: Some(tx),
                ..Default::default()
            },
        );
        self.writer.send(&json!({
            "op": "tool_call",
            "call_id": call_id,
            "event_id": event_id,
            "event": event,
            "name": name,
            "arguments": arguments,
        }));
        let r = tokio::time::timeout(timeout, rx).await.ok().and_then(|r| r.ok());
        self.waiters.lock().unwrap().remove(&key);
        r
    }

    /// Asks the plugin whether it has background work. Returns the reasons it is busy,
    /// empty when idle, or None when it does not answer in time.
    pub async fn idle_check(&self, timeout: Duration) -> Option<Vec<String>> {
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().unwrap().insert(
            IDLE_KEY.to_string(),
            Waiter {
                hook: Some(tx),
                ..Default::default()
            },
        );
        self.writer.send(&json!({"op": "idle_check"}));
        let r = tokio::time::timeout(timeout, rx).await.ok().and_then(|r| r.ok());
        self.waiters.lock().unwrap().remove(IDLE_KEY);
        let busy = r?.get("busy")?.as_array()?.clone();
        Some(busy.iter().filter_map(|b| b.as_str().map(str::to_string)).collect())
    }

    /// Function tools this plugin registered.
    pub fn tools(&self) -> Vec<&Value> {
        self.hello
            .handlers
            .iter()
            .filter(|h| h.kind == "llm_tool")
            .filter_map(|h| h.tool.as_ref())
            .collect()
    }

    pub fn has_handler(&self, kind: &str) -> bool {
        self.hello.handlers.iter().any(|h| h.kind == kind)
    }

    pub fn handler_priority(&self, kind: &str) -> i64 {
        self.hello
            .handlers
            .iter()
            .filter(|h| h.kind == kind)
            .map(|h| h.priority)
            .max()
            .unwrap_or(0)
    }

    pub fn shutdown(&self) {
        self.writer.send(&json!({"op": "shutdown"}));
    }

    pub fn writer(&self) -> ProcWriter {
        self.writer.clone()
    }
}

const IDLE_KEY: &str = "idle";

/// Exit code of a plugin process whose module failed to import or initialize.
pub const EXIT_LOAD_FAILED: i32 = 2;

/// Plugin load failure: an `error` op before `hello`, or exit code `EXIT_LOAD_FAILED`.
/// Holds the reported message or the last error level log line.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

enum PreHello {
    Hello(Hello),
    Error(String),
    Closed,
    Timeout,
}

fn is_error_level(level: &str) -> bool {
    matches!(level.to_ascii_lowercase().as_str(), "error" | "critical" | "fatal")
}

/// Starts one plugin process through the fork server and waits for its `hello`.
/// A load failure is returned as an error that downcasts to `LoadError`.
/// Returns the process and a receiver that resolves with the exit code.
pub async fn launch(
    sidecar: &Sidecar,
    spec: LaunchSpec,
    out: mpsc::Sender<OutMsg>,
    tenant: Option<(&Cgroups, &str)>,
) -> Result<(Arc<PluginProc>, oneshot::Receiver<i32>)> {
    std::fs::create_dir_all(&sidecar.run_dir)?;
    let sock = sidecar.run_dir.join(format!("{}.sock", spec.proc_id));
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666));
    }
    let mut exit_rx = sidecar.watch_exit(&spec.proc_id);
    let req = json!({
        "op": "spawn",
        "proc_id": spec.proc_id,
        "plugin_dir": spec.plugin_dir,
        "venv_site_packages": spec.venv_site_packages,
        "data_dir": spec.data_dir,
        "plugin_name": spec.plugin_name,
        "bot_id": spec.bot_id,
        "socket": sock,
        "wait_for_memory_limit": tenant.is_some(),
        "share_key": spec.share_key,
        "config_on_socket": true,
    });
    let cleanup = |e: anyhow::Error| {
        let _ = std::fs::remove_file(&sock);
        sidecar.forget(&spec.proc_id);
        e
    };
    let pid = sidecar.spawn(req).await.map_err(cleanup)?;
    if let Some((cgroups, user_id)) = tenant {
        if let Err(e) = cgroups.add_pid(user_id, pid) {
            sidecar.kill(&spec.proc_id).await;
            return Err(cleanup(anyhow!("无法将插件进程加入内存配额组: {e}")));
        }
    }
    let mut stream = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(60), listener.accept()) => match r {
            Ok(Ok((s, _))) => s,
            Ok(Err(e)) => { sidecar.kill(&spec.proc_id).await; return Err(cleanup(e.into())) }
            Err(_) => { sidecar.kill(&spec.proc_id).await; return Err(cleanup(anyhow!("插件进程未连接"))) }
        },
        code = &mut exit_rx => {
            let code = code.unwrap_or(-1);
            if code == EXIT_LOAD_FAILED {
                return Err(cleanup(anyhow::Error::new(LoadError(format!("插件加载失败 (code {code})")))));
            }
            return Err(cleanup(anyhow!("插件进程启动后立即退出 (code {code})")));
        }
    };
    drop(listener);
    let _ = std::fs::remove_file(&sock);
    // The config goes over this socket, so it never passes through the fork server or a template
    // process that later forks plugin processes of other tenants.
    let mut first = Vec::new();
    if tenant.is_some() {
        first.push(0x01);
    }
    first.extend_from_slice(json!({"op": "config", "config": spec.config}).to_string().as_bytes());
    first.push(b'\n');
    if let Err(e) = stream.write_all(&first).await {
        sidecar.kill(&spec.proc_id).await;
        return Err(cleanup(e.into()));
    }

    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    let (wtx, mut wrx) = mpsc::unbounded_channel::<String>();
    let writer = ProcWriter(wtx);
    let hello_deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut drain_deadline = hello_deadline;
    let mut exited: Option<i32> = None;
    let mut last_err_log: Option<String> = None;
    let phase = loop {
        let deadline = if exited.is_some() {
            drain_deadline
        } else {
            hello_deadline
        };
        let line = tokio::select! {
            r = tokio::time::timeout_at(deadline, lines.next_line()) => match r {
                Ok(Ok(Some(l))) => l,
                Ok(Ok(None)) | Ok(Err(_)) => break PreHello::Closed,
                Err(_) if exited.is_some() => break PreHello::Closed,
                Err(_) => break PreHello::Timeout,
            },
            code = &mut exit_rx, if exited.is_none() => {
                exited = Some(code.unwrap_or(-1));
                drain_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
                continue;
            }
        };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("op").and_then(|o| o.as_str()) {
            Some("hello") => {
                break PreHello::Hello(
                    serde_json::from_value::<Hello>(v.get("plugin").cloned().unwrap_or(Value::Null))
                        .unwrap_or_default(),
                )
            }
            Some("error") => break PreHello::Error(str_opt(&v, "message").unwrap_or_else(|| "插件加载失败".into())),
            _ => {
                if let Some(msg) = parse_out(&v) {
                    if let PluginOut::Log { level, message } = &msg {
                        if is_error_level(level) {
                            last_err_log = Some(message.clone());
                        }
                    }
                    let _ = out
                        .send(OutMsg {
                            plugin_id: spec.plugin_id.clone(),
                            plugin_name: spec.plugin_name.clone(),
                            writer: writer.clone(),
                            msg,
                        })
                        .await;
                }
            }
        }
    };
    let hello = match phase {
        PreHello::Hello(h) if exited.is_none() => h,
        PreHello::Hello(_) | PreHello::Closed => {
            let code = match exited {
                Some(c) => c,
                None => match tokio::time::timeout(Duration::from_secs(3), &mut exit_rx).await {
                    Ok(c) => c.unwrap_or(-1),
                    Err(_) => {
                        sidecar.kill(&spec.proc_id).await;
                        -1
                    }
                },
            };
            if code == EXIT_LOAD_FAILED {
                let msg = last_err_log.unwrap_or_else(|| format!("插件加载失败 (code {code})"));
                return Err(cleanup(anyhow::Error::new(LoadError(msg))));
            }
            let msg = match last_err_log {
                Some(l) => format!("插件在 hello 之前退出 (code {code}): {l}"),
                None => format!("插件在 hello 之前退出 (code {code})"),
            };
            return Err(cleanup(anyhow!(msg)));
        }
        PreHello::Error(msg) => {
            sidecar.kill(&spec.proc_id).await;
            return Err(cleanup(anyhow::Error::new(LoadError(msg))));
        }
        PreHello::Timeout => {
            sidecar.kill(&spec.proc_id).await;
            return Err(cleanup(anyhow!("等待插件 hello 超时")));
        }
    };

    tokio::spawn(async move {
        while let Some(line) = wrx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() || wr.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let proc = Arc::new(PluginProc {
        proc_id: spec.proc_id.clone(),
        plugin_id: spec.plugin_id.clone(),
        plugin_name: spec.plugin_name.clone(),
        pid,
        hello,
        activity: watch::channel(Instant::now()).0,
        writer: writer.clone(),
        waiters: Arc::default(),
        closed: CancellationToken::new(),
        last_error: Arc::default(),
    });

    let waiters = proc.waiters.clone();
    let closed = proc.closed.clone();
    let last_error = proc.last_error.clone();
    let (plugin_id, plugin_name) = (spec.plugin_id.clone(), spec.plugin_name.clone());
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let event_id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("").to_string();
            match v.get("op").and_then(|o| o.as_str()).unwrap_or("") {
                "matched" => {
                    let m = v.get("matched").and_then(|m| m.as_bool()).unwrap_or(false);
                    let mut g = waiters.lock().unwrap();
                    if let Some(w) = g.get_mut(&event_id) {
                        if let Some(tx) = w.matched.take() {
                            let _ = tx.send(m);
                        }
                    }
                }
                "done" => {
                    let stop = v.get("stop").and_then(|m| m.as_bool()).unwrap_or(false);
                    let call_llm = v.get("call_llm").and_then(|m| m.as_bool()).unwrap_or(false);
                    if let Some(mut w) = waiters.lock().unwrap().remove(&event_id) {
                        if let Some(tx) = w.matched.take() {
                            let _ = tx.send(true);
                        }
                        if let Some(tx) = w.done.take() {
                            let _ = tx.send((stop, call_llm));
                        }
                    }
                }
                "llm_hook_result" => {
                    if let Some(mut w) = waiters.lock().unwrap().remove(&format!("hook:{event_id}")) {
                        if let Some(tx) = w.hook.take() {
                            let _ = tx.send(v.get("request").cloned().unwrap_or(Value::Null));
                        }
                    }
                }
                "tool_result" => {
                    let call_id = str_opt(&v, "call_id").unwrap_or_default();
                    if let Some(mut w) = waiters.lock().unwrap().remove(&format!("tool:{call_id}")) {
                        if let Some(tx) = w.hook.take() {
                            let _ = tx.send(v.clone());
                        }
                    }
                }
                "idle_result" => {
                    if let Some(mut w) = waiters.lock().unwrap().remove(IDLE_KEY) {
                        if let Some(tx) = w.hook.take() {
                            let _ = tx.send(v.clone());
                        }
                    }
                }
                "error" => {
                    let msg = str_opt(&v, "message").unwrap_or_default();
                    *last_error.lock().unwrap() = Some(msg.clone());
                    let _ = out
                        .send(OutMsg {
                            plugin_id: plugin_id.clone(),
                            plugin_name: plugin_name.clone(),
                            writer: writer.clone(),
                            msg: PluginOut::Log {
                                level: "error".into(),
                                message: msg,
                            },
                        })
                        .await;
                }
                _ => {
                    if let Some(msg) = parse_out(&v) {
                        let _ = out
                            .send(OutMsg {
                                plugin_id: plugin_id.clone(),
                                plugin_name: plugin_name.clone(),
                                writer: writer.clone(),
                                msg,
                            })
                            .await;
                    }
                }
            }
        }
        waiters.lock().unwrap().clear();
        closed.cancel();
    });

    Ok((proc, exit_rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_out_messages() {
        let v = json!({"op": "send", "event_id": "e1", "session": "p:FriendMessage:u", "chain": [{"type": "plain", "text": "x"}]});
        assert_eq!(
            parse_out(&v),
            Some(PluginOut::Send {
                event_id: Some("e1".into()),
                session: Some("p:FriendMessage:u".into()),
                chain: vec![Component::plain("x")]
            })
        );
        let v = json!({"op": "llm", "req_id": "r", "prompt": "p", "image_urls": ["u"]});
        assert!(
            matches!(parse_out(&v), Some(PluginOut::Llm { image_urls, .. }) if image_urls == vec!["u".to_string()])
        );
        assert_eq!(parse_out(&json!({"op": "send_to", "chain": []})), None);
        assert_eq!(parse_out(&json!({"op": "matched"})), None);
        let h: Hello = serde_json::from_value(json!({"name": "n", "version": "1", "handlers": [
            {"id": "h", "kind": "command", "pattern": "ping", "priority": 1}]}))
        .unwrap();
        assert_eq!(h.handlers[0].pattern.as_deref(), Some("ping"));
    }
}
