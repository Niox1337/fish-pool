use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

/// Command used to start the fork server. `--socket <path>` is appended.
#[derive(Clone, Debug)]
pub struct SidecarCommand {
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<String, oneshot::Sender<std::result::Result<u32, String>>>>,
    exits: Mutex<HashMap<String, oneshot::Sender<i32>>>,
}

impl Shared {
    fn fail_all(&self) {
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err("插件宿主进程已退出".into()));
        }
        for (_, tx) in self.exits.lock().unwrap().drain() {
            let _ = tx.send(-1);
        }
    }

    fn handle(&self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            tracing::warn!("sidecar sent invalid json: {line}");
            return;
        };
        let proc_id = v.get("proc_id").and_then(|p| p.as_str()).unwrap_or("").to_string();
        match v.get("op").and_then(|o| o.as_str()).unwrap_or("") {
            "spawned" => {
                let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
                if let Some(tx) = self.pending.lock().unwrap().remove(&proc_id) {
                    let _ = tx.send(Ok(pid));
                }
            }
            "error" => {
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("spawn failed")
                    .to_string();
                if let Some(tx) = self.pending.lock().unwrap().remove(&proc_id) {
                    let _ = tx.send(Err(msg));
                } else {
                    tracing::warn!("sidecar error for {proc_id}: {msg}");
                }
            }
            "exited" => {
                let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) as i32;
                if let Some(tx) = self.exits.lock().unwrap().remove(&proc_id) {
                    let _ = tx.send(code);
                }
            }
            other => tracing::debug!("sidecar op {other}"),
        }
    }
}

struct Conn {
    tx: mpsc::UnboundedSender<String>,
    alive: Arc<AtomicBool>,
    _child: tokio::process::Child,
}

/// Client of the plugin fork server. The server is started lazily on the first spawn.
pub struct Sidecar {
    cmd: SidecarCommand,
    pub run_dir: PathBuf,
    conn: tokio::sync::Mutex<Option<Conn>>,
    shared: Arc<Shared>,
    root_pid: AtomicU32,
}

impl Sidecar {
    pub fn new(cmd: SidecarCommand, run_dir: PathBuf) -> Self {
        Sidecar {
            cmd,
            run_dir,
            conn: tokio::sync::Mutex::new(None),
            shared: Arc::default(),
            root_pid: AtomicU32::new(0),
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join("sidecar.sock")
    }

    /// Pid of the fork server process, zero when not running.
    pub fn pid(&self) -> u32 {
        self.root_pid.load(Ordering::Relaxed)
    }

    async fn ensure(&self) -> Result<mpsc::UnboundedSender<String>> {
        let mut g = self.conn.lock().await;
        if let Some(c) = g.as_ref() {
            if c.alive.load(Ordering::SeqCst) {
                return Ok(c.tx.clone());
            }
        }
        *g = None;
        std::fs::create_dir_all(&self.run_dir)?;
        let sock = self.socket_path();
        let _ = std::fs::remove_file(&sock);
        let mut cmd = tokio::process::Command::new(&self.cmd.program);
        cmd.args(&self.cmd.args)
            .arg("--socket")
            .arg(&sock)
            .envs(self.cmd.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.cmd.cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            if let Some(gid) = self.cmd.gid {
                cmd.gid(gid);
            }
            if let Some(uid) = self.cmd.uid {
                cmd.uid(uid);
            }
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("启动插件宿主 {} 失败", self.cmd.program))?;
        self.root_pid.store(child.id().unwrap_or(0), Ordering::Relaxed);
        for (name, pipe) in [
            (
                "stdout",
                child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            ),
            (
                "stderr",
                child
                    .stderr
                    .take()
                    .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            ),
        ] {
            if let Some(pipe) = pipe {
                tokio::spawn(async move {
                    let mut lines = BufReader::new(pipe).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        tracing::info!(target: "sidecar", stream = name, "{l}");
                    }
                });
            }
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let stream = loop {
            match UnixStream::connect(&sock).await {
                Ok(s) => break s,
                Err(e) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(anyhow!("插件宿主启动后退出: {status}"));
                    }
                    if Instant::now() > deadline {
                        return Err(anyhow!("连接插件宿主超时: {e}"));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        let (rd, mut wr) = stream.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let alive = Arc::new(AtomicBool::new(true));
        let alive_w = alive.clone();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if wr.write_all(line.as_bytes()).await.is_err() || wr.write_all(b"\n").await.is_err() {
                    alive_w.store(false, Ordering::SeqCst);
                    break;
                }
            }
        });
        let shared = self.shared.clone();
        let alive_r = alive.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(rd).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    shared.handle(&line);
                }
            }
            alive_r.store(false, Ordering::SeqCst);
            tracing::warn!("sidecar control connection closed");
            shared.fail_all();
        });
        tracing::info!("plugin fork server started");
        *g = Some(Conn {
            tx: tx.clone(),
            alive,
            _child: child,
        });
        Ok(tx)
    }

    /// Registers interest in the exit notification of a process before it is spawned.
    pub fn watch_exit(&self, proc_id: &str) -> oneshot::Receiver<i32> {
        let (tx, rx) = oneshot::channel();
        self.shared.exits.lock().unwrap().insert(proc_id.to_string(), tx);
        rx
    }

    pub fn forget(&self, proc_id: &str) {
        self.shared.exits.lock().unwrap().remove(proc_id);
        self.shared.pending.lock().unwrap().remove(proc_id);
    }

    /// Sends a spawn request and returns the child pid.
    pub async fn spawn(&self, req: Value) -> Result<u32> {
        let proc_id = req.get("proc_id").and_then(|p| p.as_str()).unwrap_or("").to_string();
        let tx = self.ensure().await?;
        let (rtx, rrx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(proc_id.clone(), rtx);
        tx.send(req.to_string()).map_err(|_| anyhow!("插件宿主连接已断开"))?;
        match tokio::time::timeout(Duration::from_secs(30), rrx).await {
            Ok(Ok(Ok(pid))) => Ok(pid),
            Ok(Ok(Err(msg))) => Err(anyhow!("插件启动失败: {msg}")),
            Ok(Err(_)) => Err(anyhow!("插件宿主连接已断开")),
            Err(_) => {
                self.shared.pending.lock().unwrap().remove(&proc_id);
                Err(anyhow!("等待插件启动超时"))
            }
        }
    }

    pub async fn kill(&self, proc_id: &str) {
        let g = self.conn.lock().await;
        if let Some(c) = g.as_ref() {
            let _ = c.tx.send(json!({"op": "kill", "proc_id": proc_id}).to_string());
        }
    }

    pub async fn shutdown(&self) {
        let mut g = self.conn.lock().await;
        if let Some(mut c) = g.take() {
            let _ = c._child.start_kill();
        }
        self.root_pid.store(0, Ordering::Relaxed);
    }
}
