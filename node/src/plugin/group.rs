//! Group processes: one process of the fork server runs several light plugins of a bot.
//!
//! A group process is started with `{"op": "spawn", "mode": "group"}` and answers
//! `{"op": "ready"}` on its control socket. Each plugin in it is a slot: the node
//! sends `load` with the slot's own socket, and the slot speaks the plugin process
//! protocol on that socket, so `proc::connect` and `PluginProc` work unchanged. The
//! group process reports `slot_exited` when a slot ends. It is told to exit when its
//! last slot ended, and it takes no new slots once it is draining.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::proc::{bind_plugin_socket, connect, LaunchControl, LaunchSpec, OutMsg, PluginProc};
use super::sidecar::Sidecar;
use crate::sandbox::Cgroups;

/// Slots a group process holds at once.
const MAX_SLOTS: usize = 16;
/// Slots a group process ever starts. Each unloaded plugin may leave objects behind in the process.
const MAX_LOADS: u32 = 32;
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Time an unloaded slot has to end before the whole group process is killed.
const UNLOAD_GRACE: Duration = Duration::from_secs(10);

/// True when the plugin venv has packages of its own, beyond the ones the sidecar provides.
pub fn has_own_packages(site_packages: Option<&Path>) -> bool {
    let Some(dir) = site_packages else { return false };
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".dist-info")),
        Err(_) => dir.exists(),
    }
}

struct Host {
    proc_id: String,
    bot_id: String,
    pid: u32,
    tx: mpsc::UnboundedSender<String>,
    /// Exit code senders of the slots the node still waits for.
    slots: Mutex<HashMap<String, oneshot::Sender<i32>>>,
    loads: AtomicU32,
    draining: AtomicBool,
    closed: CancellationToken,
}

impl Host {
    fn send(&self, v: &Value) {
        let _ = self.tx.send(v.to_string());
    }

    fn usable(&self) -> bool {
        !self.draining.load(Ordering::Relaxed)
            && !self.closed.is_cancelled()
            && self.slots.lock().unwrap().len() < MAX_SLOTS
            && self.loads.load(Ordering::Relaxed) < MAX_LOADS
    }
}

#[derive(Default)]
struct State {
    hosts: HashMap<String, Vec<Arc<Host>>>,
    /// Group process of each slot until the slot ends or the process exits.
    slots: HashMap<String, Arc<Host>>,
}

pub struct Groups {
    sidecar: Arc<Sidecar>,
    state: Mutex<State>,
    /// Held while choosing or starting a group process, so plugins of a starting bot share one process.
    // shortcut: one lock for all bots, so group processes start one at a time on the node, use a lock per bot if node start-up gets slow.
    start: tokio::sync::Mutex<()>,
}

impl Groups {
    pub fn new(sidecar: Arc<Sidecar>) -> Self {
        Groups {
            sidecar,
            state: Mutex::default(),
            start: tokio::sync::Mutex::new(()),
        }
    }

    /// Starts a plugin as a slot of a group process of its bot and waits for its `hello`.
    /// `workdir` is the work dir of a new group process. Errors are those of `proc::connect`.
    pub async fn launch(
        self: &Arc<Self>,
        spec: LaunchSpec,
        out: mpsc::Sender<OutMsg>,
        tenant: Option<(&Cgroups, &str)>,
        workdir: &Path,
    ) -> Result<(Arc<PluginProc>, oneshot::Receiver<i32>)> {
        let (listener, sock) = bind_plugin_socket(&self.sidecar.run_dir, &spec.proc_id)?;
        let (tx, rx) = oneshot::channel();
        let host = {
            let _start = self.start.lock().await;
            let found = {
                let st = self.state.lock().unwrap();
                st.hosts
                    .get(&spec.bot_id)
                    .and_then(|hosts| hosts.iter().find(|h| h.usable()).cloned())
            };
            let host = match found {
                Some(h) => h,
                None => match self.start_host(&spec.bot_id, tenant, workdir).await {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = std::fs::remove_file(&sock);
                        return Err(e);
                    }
                },
            };
            let mut st = self.state.lock().unwrap();
            host.slots.lock().unwrap().insert(spec.proc_id.clone(), tx);
            host.loads.fetch_add(1, Ordering::Relaxed);
            st.slots.insert(spec.proc_id.clone(), host.clone());
            host.send(&json!({
                "op": "load",
                "proc_id": spec.proc_id,
                "plugin_dir": spec.plugin_dir,
                "plugin_name": spec.plugin_name,
                "data_dir": spec.data_dir,
                "socket": sock,
            }));
            host
        };
        let control = SlotControl {
            groups: self.clone(),
            proc_id: spec.proc_id.clone(),
        };
        connect(listener, sock, host.pid, &spec, out, rx, false, &control).await
    }

    /// Asks the group process to stop the slot and kills the process when the slot does not end in
    /// time. Returns false when `proc_id` is not a slot.
    pub fn kill(self: &Arc<Self>, proc_id: &str) -> bool {
        let Some(host) = self.state.lock().unwrap().slots.get(proc_id).cloned() else {
            return false;
        };
        host.send(&json!({"op": "unload", "proc_id": proc_id}));
        let groups = self.clone();
        let id = proc_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(UNLOAD_GRACE).await;
            let stuck = groups
                .state
                .lock()
                .unwrap()
                .slots
                .get(&id)
                .is_some_and(|h| Arc::ptr_eq(h, &host));
            if stuck {
                tracing::warn!("plugin slot {id} did not stop, killing group process {}", host.proc_id);
                groups.sidecar.kill(&host.proc_id).await;
            }
        });
        true
    }

    /// Stops waiting for the slot's exit code. The group process exits once no slot is waited for.
    pub fn forget(&self, proc_id: &str) {
        let mut st = self.state.lock().unwrap();
        let Some(host) = st.slots.get(proc_id).cloned() else { return };
        host.slots.lock().unwrap().remove(proc_id);
        Self::retire_if_empty(&mut st, &host);
    }

    /// Called with the state lock held. Tells an empty group process to exit and stops using it.
    fn retire_if_empty(st: &mut State, host: &Arc<Host>) {
        if !host.slots.lock().unwrap().is_empty() {
            return;
        }
        host.draining.store(true, Ordering::Relaxed);
        host.send(&json!({"op": "exit"}));
        Self::drop_host(st, host);
    }

    /// Removes the group process from the processes new slots may use.
    fn drop_host(st: &mut State, host: &Arc<Host>) {
        if let Some(hosts) = st.hosts.get_mut(&host.bot_id) {
            hosts.retain(|h| !Arc::ptr_eq(h, host));
            if hosts.is_empty() {
                st.hosts.remove(&host.bot_id);
            }
        }
    }

    fn slot_exited(&self, host: &Arc<Host>, v: &Value) {
        let Some(proc_id) = v.get("proc_id").and_then(Value::as_str) else { return };
        let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1) as i32;
        if v.get("drain").and_then(Value::as_bool).unwrap_or(false) {
            host.draining.store(true, Ordering::Relaxed);
        }
        let sender = {
            let mut st = self.state.lock().unwrap();
            if st.slots.get(proc_id).is_some_and(|h| Arc::ptr_eq(h, host)) {
                st.slots.remove(proc_id);
            }
            let sender = host.slots.lock().unwrap().remove(proc_id);
            Self::retire_if_empty(&mut st, host);
            sender
        };
        if let Some(tx) = sender {
            let _ = tx.send(code);
        }
    }

    /// Ends every slot of a group process that exited or closed its control socket.
    fn close(&self, host: &Arc<Host>) {
        host.closed.cancel();
        host.draining.store(true, Ordering::Relaxed);
        let senders: Vec<_> = {
            let mut st = self.state.lock().unwrap();
            st.slots.retain(|_, h| !Arc::ptr_eq(h, host));
            Self::drop_host(&mut st, host);
            host.slots.lock().unwrap().drain().map(|(_, tx)| tx).collect()
        };
        for tx in senders {
            let _ = tx.send(-1);
        }
        self.sidecar.forget(&host.proc_id);
    }

    async fn start_host(
        self: &Arc<Self>,
        bot_id: &str,
        tenant: Option<(&Cgroups, &str)>,
        workdir: &Path,
    ) -> Result<Arc<Host>> {
        let host_id = format!("group-{}", ulid::Ulid::new().to_string().to_lowercase());
        let (listener, sock) = bind_plugin_socket(&self.sidecar.run_dir, &host_id)?;
        let mut exit_rx = self.sidecar.watch_exit(&host_id);
        let fail = |e: anyhow::Error| {
            let _ = std::fs::remove_file(&sock);
            self.sidecar.forget(&host_id);
            e
        };
        let req = json!({
            "op": "spawn",
            "mode": "group",
            "proc_id": host_id,
            "plugin_dir": workdir,
            "data_dir": workdir,
            "plugin_name": "group",
            "bot_id": bot_id,
            "socket": sock,
            "wait_for_memory_limit": tenant.is_some(),
        });
        let pid = self.sidecar.spawn(req).await.map_err(fail)?;
        let handshake = async {
            if let Some((cgroups, user_id)) = tenant {
                cgroups
                    .add_pid(user_id, pid)
                    .map_err(|e| anyhow!("无法将插件进程加入内存配额组: {e}"))?;
            }
            let accepted = tokio::select! {
                r = tokio::time::timeout(READY_TIMEOUT, listener.accept()) => r,
                code = &mut exit_rx => return Err(anyhow!("插件共用进程启动后立即退出 (code {})", code.unwrap_or(-1))),
            };
            let (mut stream, _) = accepted.map_err(|_| anyhow!("插件共用进程未连接"))??;
            let _ = std::fs::remove_file(&sock);
            if tenant.is_some() {
                stream.write_all(&[0x01]).await?;
            }
            let (rd, wr) = stream.into_split();
            let mut lines = BufReader::new(rd).lines();
            let ready = tokio::time::timeout(READY_TIMEOUT, async {
                while let Ok(Some(line)) = lines.next_line().await {
                    let op = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("op").and_then(Value::as_str).map(str::to_string));
                    if op.as_deref() == Some("ready") {
                        return true;
                    }
                }
                false
            });
            let ready = tokio::select! {
                r = ready => r.unwrap_or(false),
                _ = &mut exit_rx => false,
            };
            if !ready {
                return Err(anyhow!("插件共用进程没有就绪"));
            }
            Ok((lines, wr))
        };
        let (mut lines, mut wr) = match handshake.await {
            Ok(v) => v,
            Err(e) => {
                self.sidecar.kill(&host_id).await;
                return Err(fail(e));
            }
        };
        drop(listener);

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if wr.write_all(line.as_bytes()).await.is_err() || wr.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });
        let host = Arc::new(Host {
            proc_id: host_id.clone(),
            bot_id: bot_id.to_string(),
            pid,
            tx,
            slots: Mutex::default(),
            loads: AtomicU32::new(0),
            draining: AtomicBool::new(false),
            closed: CancellationToken::new(),
        });
        self.state
            .lock()
            .unwrap()
            .hosts
            .entry(bot_id.to_string())
            .or_default()
            .push(host.clone());
        let groups = self.clone();
        let h = host.clone();
        tokio::spawn(async move {
            let read = async {
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v.get("op").and_then(Value::as_str) == Some("slot_exited") {
                        groups.slot_exited(&h, &v);
                    }
                }
            };
            tokio::select! {
                _ = read => {}
                _ = exit_rx => {}
            }
            groups.close(&h);
        });
        tracing::info!("started plugin group process {host_id} pid={pid} for bot {bot_id}");
        Ok(host)
    }

    /// Work dir for the group processes of a bot.
    pub fn workdir(bot_dir: &Path) -> PathBuf {
        bot_dir.join("group")
    }
}

struct SlotControl {
    groups: Arc<Groups>,
    proc_id: String,
}

impl LaunchControl for SlotControl {
    fn kill(&self) -> BoxFuture<'_, ()> {
        self.groups.kill(&self.proc_id);
        Box::pin(async {})
    }

    fn forget(&self) {
        self.groups.forget(&self.proc_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_packages_are_dist_info_dirs() {
        let dir = std::env::temp_dir().join(format!("fish-group-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("_virtualenv")).unwrap();
        std::fs::write(dir.join("_virtualenv.pth"), "").unwrap();
        assert!(!has_own_packages(None));
        assert!(!has_own_packages(Some(&dir)));
        std::fs::create_dir_all(dir.join("httpx-0.27.0.dist-info")).unwrap();
        assert!(has_own_packages(Some(&dir)));
        assert!(!has_own_packages(Some(&dir.join("missing"))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
