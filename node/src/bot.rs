use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{safe_name, Config};
use crate::events::Events;
use crate::llm::{self, ChatMessage, ChatRequest, ToolDef};
use crate::logs::LogBuffer;
use crate::model::{
    parse_origin, unified_origin, BotSettings, Component, DesiredBot, LlmSpec, MessageEvent, MessageType, PlatformSpec,
    PluginSpec, PLATFORM_QQ, PLATFORM_WEIXIN,
};
use crate::pipeline::{is_listed, listed_ids, wake, RateLimiter};
use crate::platform::{loopback, qq, weixin, Inbound, PlatformConn, StatusSink};
use crate::plugin::install::Installer;
use crate::plugin::proc::EXIT_UNGROUPABLE;
use crate::plugin::{
    launch, restart_backoff, Groups, LaunchSpec, LoadError, OutMsg, PluginOut, PluginProc, Sidecar, Ungroupable,
};
use crate::sandbox::{chown_tree, Cgroups};
use crate::store::{ConvDb, NodeDb};

/// Process wide resources shared by all bots.
pub struct Shared {
    pub cfg: Config,
    pub http: reqwest::Client,
    pub db: Arc<NodeDb>,
    pub conv: Arc<ConvDb>,
    pub sidecar: Arc<Sidecar>,
    /// Group processes that run light plugins of one bot together.
    pub groups: Arc<Groups>,
    pub events: Events,
    pub cgroups: Option<Cgroups>,
    pub installer: Installer,
    pub install_sem: Semaphore,
    /// Latest measured storage per user, refreshed by the usage sampler.
    pub storage_by_user: Mutex<HashMap<String, u64>>,
}

impl Shared {
    /// Stops a plugin process or a plugin slot of a group process.
    pub async fn kill_proc(&self, proc_id: &str) {
        if !self.groups.kill(proc_id) {
            self.sidecar.kill(proc_id).await;
        }
    }

    /// Drops the exit watch of a plugin process or slot.
    pub fn forget_proc(&self, proc_id: &str) {
        self.groups.forget(proc_id);
        self.sidecar.forget(proc_id);
    }

    /// Removes shared plugin installs that no bot uses any more.
    pub async fn gc_shared_plugins(self: &Arc<Self>) {
        let _permit = self.install_sem.acquire().await;
        let shared = self.clone();
        let removed = tokio::task::spawn_blocking(move || shared.installer.gc_shared(&shared.cfg.tenants_dir()))
            .await
            .unwrap_or(0);
        if removed > 0 {
            tracing::info!("removed {removed} unused shared plugin installs");
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ItemStatus {
    pub status: String,
    pub status_detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

struct Board {
    status: String,
    detail: String,
    platforms: BTreeMap<String, ItemStatus>,
    plugins: BTreeMap<String, ItemStatus>,
}

#[derive(Clone, Debug)]
struct ReplyTarget {
    platform_id: String,
    mt: MessageType,
    session_id: String,
    msg_id: Option<String>,
}

struct Core {
    cancel: CancellationToken,
    inbound_tx: mpsc::Sender<Inbound>,
    out_tx: mpsc::Sender<OutMsg>,
}

struct PluginSlot {
    spec: PluginSpec,
    config: watch::Sender<Value>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

enum PluginExit {
    Stop,
    Reload,
    Failed(i32),
    Sleep,
}

#[derive(Default)]
struct Runtime {
    core: Option<Core>,
    platforms: HashMap<String, (PlatformSpec, CancellationToken)>,
    plugins: HashMap<String, PluginSlot>,
}

const MATCH_WAIT: Duration = Duration::from_secs(2);
const DONE_WAIT: Duration = Duration::from_secs(120);
const HOOK_WAIT: Duration = Duration::from_secs(5);
/// Hook kinds that tell a plugin about other plugins. A plugin with one of them stays running so
/// it sees every load and unload.
const LIFECYCLE_HOOKS: [&str; 2] = ["plugin_loaded", "plugin_unloaded"];
/// Longest wait for one plugin's `decorating_result` hooks, which may render images.
const DECORATE_WAIT: Duration = Duration::from_secs(30);
const TOOL_WAIT: Duration = Duration::from_secs(60);
/// Rounds of tool calls before the model is told to answer without tools.
const MAX_TOOL_ROUNDS: usize = 5;
const MAX_TOOL_RESULT_CHARS: usize = 8000;
const INFLIGHT_TTL: Duration = Duration::from_secs(600);
const IDLE_CHECK_WAIT: Duration = Duration::from_secs(5);
/// Longest time an inbound message waits for sleeping plugins to start.
const WAKE_WAIT: Duration = Duration::from_secs(30);
/// Status detail of a plugin running in a group process.
const GROUPED_DETAIL: &str = "与同一机器人的其他轻量插件共用一个进程";

struct DormantPlugin {
    activity: watch::Sender<Instant>,
    commands: Option<Vec<String>>,
    config: watch::Receiver<Value>,
}

fn matches_wake_commands(commands: Option<&[String]>, command: Option<&str>) -> bool {
    commands.is_none_or(|names| names.is_empty() || command.is_some_and(|word| names.iter().any(|name| name == word)))
}

/// One tenant bot: desired state, live connections, plugin processes and status.
pub struct Bot {
    pub id: String,
    pub shared: Arc<Shared>,
    pub desired: RwLock<DesiredBot>,
    pub logs: Arc<LogBuffer>,
    board: Mutex<Board>,
    rt: tokio::sync::Mutex<Runtime>,
    platforms: RwLock<HashMap<String, PlatformConn>>,
    pub procs: RwLock<HashMap<String, Arc<PluginProc>>>,
    rate: Mutex<RateLimiter>,
    /// Reply target and event of recent inbound messages, by event id.
    inflight: Mutex<HashMap<String, (ReplyTarget, MessageEvent, Instant)>>,
    over_quota: Mutex<HashSet<String>>,
    /// Plugins that run in their own process even when group processes are enabled.
    // shortcut: kept in memory only, so a node restart tries grouping again, store it in node.db if plugins keep falling back after restarts.
    solo: Mutex<HashSet<String>>,
    handlers: Arc<Semaphore>,
    /// Plugins stopped while idle. Changed only while holding the `procs` write lock.
    dormant: Mutex<HashMap<String, DormantPlugin>>,
    /// Bumped whenever a plugin leaves `dormant`.
    dormant_gen: watch::Sender<u64>,
    /// Last LLM failure notice per chat session and kind.
    notices: Mutex<HashMap<(String, llm::Trouble), Instant>>,
    /// Last time a seen session or sender was written to `conversations.db`, by kind and id.
    seen_written: Mutex<HashMap<(&'static str, String), Instant>>,
}

/// Minimum time between two writes of the same seen session or sender.
const SEEN_WRITE_EVERY: Duration = Duration::from_secs(300);

/// Reply to the built in `/sid` command: the session origin and the sender's account id.
/// `raw` is the message text before wake prefixes were removed. A bare `sid` without a prefix is not a command.
fn sid_reply(ev: &MessageEvent, raw: &str) -> Option<String> {
    let text = ev.message_str.trim();
    let command = text.eq_ignore_ascii_case("/sid") || (text.eq_ignore_ascii_case("sid") && raw.trim() != text);
    if !ev.is_at_or_wake || !command {
        return None;
    }
    let mut lines = vec![
        format!("当前会话：{}", ev.unified_msg_origin),
        format!("你的账号：{}", ev.sender.user_id),
    ];
    if let Some(g) = ev.group_id.as_deref().filter(|g| !g.is_empty()) {
        lines.push(format!("群号：{g}"));
    }
    lines.push("会话可以填进插件的会话列表，账号可以填进管理员。".into());
    Some(lines.join("\n"))
}

fn log_level(l: &str) -> &'static str {
    match l.to_ascii_lowercase().as_str() {
        "error" | "critical" | "fatal" => "error",
        "warn" | "warning" => "warn",
        "debug" | "trace" => "debug",
        _ => "info",
    }
}

fn platform_source(kind: &str) -> &'static str {
    match kind {
        PLATFORM_QQ => PLATFORM_QQ,
        PLATFORM_WEIXIN => PLATFORM_WEIXIN,
        _ => "core",
    }
}

impl Bot {
    pub fn new(shared: Arc<Shared>, desired: DesiredBot) -> Arc<Bot> {
        Arc::new(Bot {
            id: desired.bot_id.clone(),
            logs: Arc::new(LogBuffer::new(&desired.bot_id, 1000)),
            shared,
            desired: RwLock::new(desired),
            board: Mutex::new(Board {
                status: "starting".into(),
                detail: String::new(),
                platforms: BTreeMap::new(),
                plugins: BTreeMap::new(),
            }),
            rt: tokio::sync::Mutex::new(Runtime::default()),
            platforms: RwLock::new(HashMap::new()),
            procs: RwLock::new(HashMap::new()),
            rate: Mutex::new(RateLimiter::default()),
            inflight: Mutex::new(HashMap::new()),
            over_quota: Mutex::new(HashSet::new()),
            solo: Mutex::new(HashSet::new()),
            handlers: Arc::new(Semaphore::new(8)),
            dormant: Mutex::new(HashMap::new()),
            dormant_gen: watch::channel(0).0,
            notices: Mutex::new(HashMap::new()),
            seen_written: Mutex::new(HashMap::new()),
        })
    }

    pub fn user_id(&self) -> String {
        self.desired.read().unwrap().user_id.clone()
    }

    // ---- status ----

    fn set_bot_status(&self, status: &str, detail: &str) {
        let changed = {
            let mut b = self.board.lock().unwrap();
            let c = b.status != status || b.detail != detail;
            b.status = status.into();
            b.detail = detail.into();
            c
        };
        if changed {
            self.shared
                .events
                .push(&self.id, "bot_status", &self.id, status, detail);
        }
    }

    pub fn set_platform_status(&self, id: &str, status: &str, detail: &str) {
        let item = ItemStatus {
            status: status.into(),
            status_detail: detail.into(),
            version: None,
        };
        let changed = self
            .board
            .lock()
            .unwrap()
            .platforms
            .insert(id.to_string(), item.clone())
            != Some(item);
        if changed {
            self.shared.events.push(&self.id, "platform_status", id, status, detail);
        }
    }

    fn set_plugin_status(&self, id: &str, status: &str, detail: &str, version: Option<String>) {
        let item = ItemStatus {
            status: status.into(),
            status_detail: detail.into(),
            version,
        };
        let changed = self.board.lock().unwrap().plugins.insert(id.to_string(), item.clone()) != Some(item);
        if changed {
            self.shared.events.push(&self.id, "plugin_status", id, status, detail);
        }
    }

    pub fn status_json(&self) -> Value {
        let b = self.board.lock().unwrap();
        json!({
            "status": b.status,
            "status_detail": b.detail,
            "platforms": b.platforms.iter().map(|(id, s)| json!({"id": id, "status": s.status, "status_detail": s.status_detail})).collect::<Vec<_>>(),
            "plugins": b.plugins.iter().map(|(id, s)| json!({
                "id": id, "status": s.status, "status_detail": s.status_detail, "version": s.version.clone().unwrap_or_default()
            })).collect::<Vec<_>>(),
        })
    }

    pub fn plugin_status(&self, id: &str) -> Option<ItemStatus> {
        self.board.lock().unwrap().plugins.get(id).cloned()
    }

    // shortcut: core memory is estimated from buffer sizes, upgrade with a counting allocator per bot.
    /// Estimated memory held by this bot inside the node process.
    pub fn core_memory_bytes(&self) -> u64 {
        let platforms: usize = self.platforms.read().unwrap().values().map(|p| p.approx_bytes()).sum();
        let inflight = self.inflight.lock().unwrap().len() * 512;
        (48 * 1024 + self.logs.approx_bytes() + platforms + inflight) as u64
    }

    // ---- reconciliation ----

    pub async fn apply(self: &Arc<Self>, d: DesiredBot) {
        let mut rt = self.rt.lock().await;
        self.apply_locked(&mut rt, d).await;
    }

    async fn apply_locked(self: &Arc<Self>, rt: &mut Runtime, d: DesiredBot) {
        *self.desired.write().unwrap() = d.clone();
        if let Some(cg) = &self.shared.cgroups {
            cg.set_limit(&d.user_id, d.quota.memory_mb);
        }
        if !d.running {
            if rt.core.is_some() {
                self.logs.info("core", "机器人已停止");
            }
            self.stop_locked(rt).await;
            self.prune_plugin_dirs(&d).await;
            self.set_bot_status("stopped", "");
            return;
        }
        if rt.core.is_none() {
            self.start_core(rt);
            self.logs.info("core", format!("机器人 {} 已启动", d.name));
        }
        self.reconcile_platforms(rt, &d);
        self.reconcile_plugins(rt, &d).await;
        self.prune_plugin_dirs(&d).await;
        let detail = if d.platforms.iter().any(|p| p.enabled) {
            ""
        } else {
            "尚未绑定聊天平台"
        };
        self.set_bot_status("running", detail);
    }

    pub async fn restart(self: &Arc<Self>) {
        let mut rt = self.rt.lock().await;
        self.logs.info("core", "正在重启");
        self.stop_locked(&mut rt).await;
        self.over_quota.lock().unwrap().clear();
        let d = self.desired.read().unwrap().clone();
        self.apply_locked(&mut rt, d).await;
    }

    pub async fn stop(self: &Arc<Self>) {
        let mut rt = self.rt.lock().await;
        self.stop_locked(&mut rt).await;
    }

    /// Stops everything and removes all data of this bot.
    pub async fn destroy(self: &Arc<Self>) {
        let mut rt = self.rt.lock().await;
        self.stop_locked(&mut rt).await;
        let user = self.user_id();
        let _ = self.shared.conv.delete_bot(&self.id);
        let _ = self.shared.conv.delete_seen(&self.id);
        let _ = self.shared.db.delete_bot(&self.id);
        let dir = self.shared.cfg.bot_dir(&user, &self.id);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        self.shared.gc_shared_plugins().await;
        self.set_bot_status("stopped", "已删除");
    }

    /// Stops a plugin because its tenant exceeded the memory quota.
    pub async fn stop_over_quota(self: &Arc<Self>, plugin_id: &str) {
        self.over_quota.lock().unwrap().insert(plugin_id.to_string());
        let mut rt = self.rt.lock().await;
        if let Some(slot) = rt.plugins.remove(plugin_id) {
            self.logs
                .warn(&format!("plugin:{}", slot.spec.name), "内存长时间超出配额，插件已停止");
            Self::stop_slot(slot).await;
        }
        self.set_plugin_status(
            plugin_id,
            "over_quota",
            "内存长时间超出配额，插件已停止，重启机器人后恢复",
            None,
        );
    }

    async fn stop_slot(slot: PluginSlot) {
        slot.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(8), slot.task).await;
    }

    async fn stop_locked(&self, rt: &mut Runtime) {
        let slots: Vec<(String, PluginSlot)> = rt.plugins.drain().collect();
        let stops = slots.into_iter().map(|(id, slot)| async move {
            Self::stop_slot(slot).await;
            id
        });
        for id in futures_util::future::join_all(stops).await {
            if self.plugin_status(&id).is_none_or(|s| s.status != "over_quota") {
                self.set_plugin_status(&id, "stopped", "", None);
            }
        }
        for (id, (_, tok)) in rt.platforms.drain() {
            tok.cancel();
            self.set_platform_status(&id, "disabled", "机器人已停止");
        }
        self.platforms.write().unwrap().clear();
        self.procs.write().unwrap().clear();
        if let Some(core) = rt.core.take() {
            core.cancel.cancel();
        }
    }

    fn start_core(self: &Arc<Self>, rt: &mut Runtime) {
        let cancel = CancellationToken::new();
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Inbound>(256);
        let (out_tx, mut out_rx) = mpsc::channel::<OutMsg>(256);
        let bot = self.clone();
        let c = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = c.cancelled() => break,
                    Some(ib) = inbound_rx.recv() => {
                        let b = bot.clone();
                        tokio::spawn(async move {
                            let Ok(_permit) = b.handlers.clone().acquire_owned().await else { return };
                            b.handle_inbound(ib).await;
                        });
                    }
                    Some(m) = out_rx.recv() => {
                        let b = bot.clone();
                        tokio::spawn(async move { b.handle_out(m).await });
                    }
                }
            }
        });
        rt.core = Some(Core {
            cancel,
            inbound_tx,
            out_tx,
        });
    }

    fn status_sink(self: &Arc<Self>, id: &str) -> StatusSink {
        let weak = Arc::downgrade(self);
        let id = id.to_string();
        Arc::new(move |s: &str, d: &str| {
            if let Some(b) = weak.upgrade() {
                b.set_platform_status(&id, s, d);
            }
        })
    }

    fn reconcile_platforms(self: &Arc<Self>, rt: &mut Runtime, d: &DesiredBot) {
        let wanted: HashMap<&str, &PlatformSpec> = d
            .platforms
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.as_str(), p))
            .collect();
        let stale: Vec<String> = rt
            .platforms
            .iter()
            .filter(|(id, (spec, _))| wanted.get(id.as_str()).is_none_or(|w| *w != spec))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some((spec, tok)) = rt.platforms.remove(&id) {
                tok.cancel();
                self.platforms.write().unwrap().remove(&id);
                self.logs.info(platform_source(&spec.kind), format!("断开平台 {id}"));
            }
        }
        {
            let ids: HashSet<&str> = d.platforms.iter().map(|p| p.id.as_str()).collect();
            self.board
                .lock()
                .unwrap()
                .platforms
                .retain(|id, _| ids.contains(id.as_str()));
        }
        for spec in &d.platforms {
            if !spec.enabled {
                self.set_platform_status(&spec.id, "disabled", "");
                continue;
            }
            if rt.platforms.contains_key(&spec.id) {
                continue;
            }
            if let Err(msg) = self.start_platform(rt, spec) {
                self.logs.error(platform_source(&spec.kind), msg.clone());
                self.set_platform_status(&spec.id, "error", &msg);
            }
        }
    }

    pub(crate) fn qq_panel_connection(&self, id: &str) -> Option<Arc<qq::QqConn>> {
        let desired = self.desired.read().unwrap();
        let spec = desired.platforms.iter().find(|p| p.id == id && p.kind == PLATFORM_QQ)?;
        if spec.app_id.is_empty() || spec.secret.is_empty() {
            return None;
        }
        let fixture_token = self
            .shared
            .cfg
            .dev_services
            .then(|| std::env::var("FISH_QQ_PANEL_TOKEN_URL").ok())
            .flatten();
        if fixture_token.is_none() {
            if let Some(PlatformConn::Qq(conn)) = self.platforms.read().unwrap().get(id) {
                if conn.has_credentials(&spec.app_id, &spec.secret) {
                    return Some(conn.clone());
                }
            }
        }
        let mut conn = qq::QqConn::new(
            id,
            &spec.app_id,
            &spec.secret,
            self.shared.http.clone(),
            self.logs.clone(),
        );
        if let Some(url) = fixture_token {
            conn.token_url = url;
        }
        Some(Arc::new(conn))
    }

    fn start_platform(self: &Arc<Self>, rt: &mut Runtime, spec: &PlatformSpec) -> Result<(), String> {
        let core = rt.core.as_ref().ok_or("机器人未运行")?;
        let tok = core.cancel.child_token();
        let sink = self.status_sink(&spec.id);
        let http = self.shared.http.clone();
        match spec.kind.as_str() {
            PLATFORM_QQ => {
                if spec.app_id.is_empty() || spec.secret.is_empty() {
                    return Err("缺少 QQ 机器人 AppID 或 AppSecret".into());
                }
                let mut conn = qq::QqConn::new(&spec.id, &spec.app_id, &spec.secret, http, self.logs.clone());
                conn.webhook = spec.extra.get("mode").and_then(|v| v.as_str()) == Some("webhook");
                let conn = Arc::new(conn);
                self.platforms
                    .write()
                    .unwrap()
                    .insert(spec.id.clone(), PlatformConn::Qq(conn.clone()));
                if conn.webhook {
                    self.logs.info(
                        platform_source(&spec.kind),
                        "QQ 平台使用 Webhook 接收消息，等待开放平台回调",
                    );
                    sink("connecting", "等待 QQ 开放平台回调验证");
                } else {
                    tokio::spawn(qq::run(conn, core.inbound_tx.clone(), sink, tok.clone()));
                }
            }
            PLATFORM_WEIXIN => {
                if spec.secret.is_empty() {
                    return Err("缺少微信 bot token，请重新扫码".into());
                }
                let extra = |k: &str| spec.extra.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let account = if spec.app_id.is_empty() {
                    extra("bot_id")
                } else {
                    spec.app_id.clone()
                };
                let account = weixin::WxAccount {
                    account_id: account,
                    token: spec.secret.clone(),
                    base_url: extra("base_url"),
                };
                let conn = Arc::new(weixin::WxConn::new(
                    &self.id,
                    &spec.id,
                    account,
                    http,
                    self.shared.db.clone(),
                    self.logs.clone(),
                ));
                self.platforms
                    .write()
                    .unwrap()
                    .insert(spec.id.clone(), PlatformConn::Weixin(conn.clone()));
                tokio::spawn(weixin::run(conn, core.inbound_tx.clone(), sink, tok.clone()));
            }
            other => return Err(format!("不支持的平台类型 {other}")),
        }
        self.set_platform_status(&spec.id, "connecting", "");
        rt.platforms.insert(spec.id.clone(), (spec.clone(), tok));
        Ok(())
    }

    async fn reconcile_plugins(self: &Arc<Self>, rt: &mut Runtime, d: &DesiredBot) {
        let over = self.over_quota.lock().unwrap().clone();
        let wanted: HashSet<&str> = d.plugins.iter().map(|p| p.id.as_str()).collect();
        let removed: Vec<String> = rt
            .plugins
            .keys()
            .filter(|id| !wanted.contains(id.as_str()))
            .cloned()
            .collect();
        let mut uninstalled = false;
        for id in removed {
            if let Some(slot) = rt.plugins.remove(&id) {
                let dir = self.shared.cfg.plugin_dir(&d.user_id, &d.bot_id, &slot.spec.name);
                self.logs.info(&format!("plugin:{}", slot.spec.name), "插件已卸载");
                Self::stop_slot(slot).await;
                let _ = tokio::fs::remove_dir_all(dir).await;
                uninstalled = true;
            }
            self.board.lock().unwrap().plugins.remove(&id);
        }
        if uninstalled {
            self.shared.gc_shared_plugins().await;
        }
        {
            let ids: HashSet<&str> = wanted.clone();
            self.board
                .lock()
                .unwrap()
                .plugins
                .retain(|id, _| ids.contains(id.as_str()));
        }
        for spec in &d.plugins {
            let blocked = over.contains(&spec.id);
            if !spec.enabled || blocked {
                if let Some(slot) = rt.plugins.remove(&spec.id) {
                    Self::stop_slot(slot).await;
                }
                if blocked {
                    self.set_plugin_status(
                        &spec.id,
                        "over_quota",
                        "内存长时间超出配额，插件已停止，重启机器人后恢复",
                        None,
                    );
                } else {
                    self.set_plugin_status(&spec.id, "stopped", "", None);
                }
                continue;
            }
            if let Some(slot) = rt.plugins.get_mut(&spec.id) {
                if !slot.spec.needs_reinstall(spec) {
                    if slot.spec.config != spec.config {
                        slot.config.send_replace(spec.config.clone());
                        slot.spec = spec.clone();
                        self.logs.info(&format!("plugin:{}", spec.name), "插件配置已更新");
                    }
                    continue;
                }
            }
            if let Some(slot) = rt.plugins.remove(&spec.id) {
                self.logs
                    .info(&format!("plugin:{}", spec.name), "插件版本变化，重新安装");
                Self::stop_slot(slot).await;
            }
            let Some(core) = rt.core.as_ref() else { return };
            let cancel = core.cancel.child_token();
            let (config, updates) = watch::channel(spec.config.clone());
            let task =
                tokio::spawn(
                    self.clone()
                        .supervise_plugin(spec.clone(), updates, cancel.clone(), core.out_tx.clone()),
                );
            rt.plugins.insert(
                spec.id.clone(),
                PluginSlot {
                    spec: spec.clone(),
                    config,
                    cancel,
                    task,
                },
            );
        }
    }

    /// Removes plugin directories of this bot that no plugin in `d` uses, then unused shared installs.
    /// Callers have stopped every plugin whose directory is removed.
    async fn prune_plugin_dirs(self: &Arc<Self>, d: &DesiredBot) {
        let root = self.shared.cfg.bot_dir(&d.user_id, &d.bot_id).join("plugins");
        let keep: HashSet<String> = d.plugins.iter().map(|p| safe_name(&p.name)).collect();
        let Ok(mut rd) = tokio::fs::read_dir(&root).await else {
            return;
        };
        let mut removed = false;
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if keep.contains(&name) {
                continue;
            }
            if tokio::fs::remove_dir_all(e.path()).await.is_ok() {
                tracing::info!(bot = %d.bot_id, "removed directory of uninstalled plugin {name}");
                removed = true;
            }
        }
        if removed {
            self.shared.gc_shared_plugins().await;
        }
    }

    /// Stops the plugin, deletes its data directory and starts it again when it is enabled.
    pub async fn clear_plugin_data(self: &Arc<Self>, id: &str) -> Result<(), &'static str> {
        let mut rt = self.rt.lock().await;
        let d = self.desired.read().unwrap().clone();
        let spec = d.plugins.iter().find(|p| p.id == id).ok_or("插件不存在")?;
        if let Some(slot) = rt.plugins.remove(id) {
            Self::stop_slot(slot).await;
        }
        let data = self
            .shared
            .cfg
            .plugin_dir(&d.user_id, &d.bot_id, &spec.name)
            .join("data");
        let result = match tokio::fs::remove_dir_all(&data).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                tracing::warn!(bot = %self.id, "clear plugin data: {e}");
                Err("删除插件数据失败，请稍后重试")
            }
        };
        if result.is_ok() {
            self.logs.info(&format!("plugin:{}", spec.name), "插件数据已清除");
        }
        if d.running {
            self.reconcile_plugins(&mut rt, &d).await;
        }
        result
    }

    pub async fn reload_plugin(&self, id: &str) -> Result<(), &'static str> {
        let rt = self.rt.lock().await;
        let slot = rt.plugins.get(id).ok_or("插件当前未启用或小鱼尚未启动")?;
        let dir = self.shared.cfg.plugin_dir(&self.user_id(), &self.id, &slot.spec.name);
        if !self.shared.installer.is_installed(&slot.spec, &dir) {
            return Err("插件尚未安装完成，请检查安装状态");
        }
        slot.config
            .send(slot.spec.config.clone())
            .map_err(|_| "插件进程不可用，请稍后重试")
    }

    fn storage_over_quota(&self) -> bool {
        let (user, quota) = {
            let d = self.desired.read().unwrap();
            (d.user_id.clone(), d.quota.storage_mb)
        };
        let used = self
            .shared
            .storage_by_user
            .lock()
            .unwrap()
            .get(&user)
            .copied()
            .unwrap_or(0);
        used > quota * 1024 * 1024
    }

    async fn sleep_or_cancel(cancel: &CancellationToken, d: Duration) -> bool {
        tokio::select! {
            _ = cancel.cancelled() => false,
            _ = tokio::time::sleep(d) => true,
        }
    }

    /// Resolves once no relevant message has arrived for `idle` and the plugin reports no
    /// background work. Never resolves when `idle` is None.
    async fn wait_idle(
        &self,
        proc: &PluginProc,
        activity: &mut watch::Receiver<Instant>,
        idle: Option<Duration>,
        src: &str,
    ) {
        let Some(idle) = idle else {
            return std::future::pending().await;
        };
        if LIFECYCLE_HOOKS.iter().any(|kind| proc.has_handler(kind)) {
            self.logs.info(src, "插件会接收其他插件加载和卸载的通知，不会休眠");
            return std::future::pending().await;
        }
        let mut reported: Option<String> = None;
        loop {
            let due = *activity.borrow_and_update() + idle;
            if Instant::now() < due {
                tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
                continue;
            }
            let reason = match proc.idle_check(IDLE_CHECK_WAIT).await {
                Some(busy) if busy.is_empty() => return,
                Some(busy) => busy.join("，"),
                None => "插件没有回应空闲检查".to_string(),
            };
            if reported.as_deref() != Some(reason.as_str()) {
                self.logs.info(src, format!("插件仍有后台活动（{reason}），暂不休眠"));
                reported = Some(reason);
            }
            tokio::time::sleep(idle).await;
        }
    }

    /// Moves a plugin from `procs` to `dormant`. Returns false, changing nothing, when a message
    /// arrived after `activity` was last read.
    fn enter_dormant(
        &self,
        proc: &PluginProc,
        activity: &watch::Receiver<Instant>,
        config: &watch::Receiver<Value>,
    ) -> bool {
        let mut procs = self.procs.write().unwrap();
        if activity.has_changed().unwrap_or(true)
            || procs.get(&proc.plugin_id).is_none_or(|p| p.proc_id != proc.proc_id)
        {
            return false;
        }
        procs.remove(&proc.plugin_id);
        self.dormant.lock().unwrap().insert(
            proc.plugin_id.clone(),
            DormantPlugin {
                activity: proc.activity.clone(),
                commands: proc.hello.wake_commands.clone(),
                config: config.clone(),
            },
        );
        true
    }

    /// Registers a running process when given one and takes the plugin out of `dormant`.
    /// Returns true when the plugin was dormant.
    fn leave_dormant(&self, plugin_id: &str, proc: Option<Arc<PluginProc>>) -> bool {
        let was_dormant = {
            let mut procs = self.procs.write().unwrap();
            if let Some(p) = proc {
                procs.insert(plugin_id.to_string(), p);
            }
            self.dormant.lock().unwrap().remove(plugin_id)
        };
        if was_dormant.is_some() {
            self.dormant_gen.send_modify(|g| *g += 1);
        }
        was_dormant.is_some()
    }

    /// Wakes plugins that may handle this message and returns the running plugins.
    async fn awake_procs(&self, ev: &MessageEvent) -> Vec<Arc<PluginProc>> {
        let command = if ev.is_at_or_wake {
            ev.message_str
                // Python's str.isspace also includes these four separators.
                .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
                .find(|word| !word.is_empty())
        } else {
            None
        };
        let mut gen = self.dormant_gen.subscribe();
        let deadline = tokio::time::Instant::now() + WAKE_WAIT;
        loop {
            {
                let procs = self.procs.read().unwrap();
                let now = Instant::now();
                for proc in procs.values() {
                    if matches_wake_commands(proc.hello.wake_commands.as_deref(), command) {
                        proc.activity.send_replace(now);
                    }
                }
                let mut waiting = false;
                for plugin in self.dormant.lock().unwrap().values() {
                    // A reload may register different commands from the sleeping process.
                    if plugin.config.has_changed().unwrap_or(true)
                        || matches_wake_commands(plugin.commands.as_deref(), command)
                    {
                        waiting = true;
                        plugin.activity.send_replace(now);
                    }
                }
                if !waiting {
                    return procs.values().cloned().collect();
                }
            }
            if tokio::time::timeout_at(deadline, gen.changed()).await.is_err() {
                self.logs
                    .warn("core", "有插件没能及时唤醒，这条消息只交给正在运行的插件");
                return self.procs.read().unwrap().values().cloned().collect();
            }
        }
    }

    async fn supervise_plugin(
        self: Arc<Self>,
        spec: PluginSpec,
        mut config: watch::Receiver<Value>,
        cancel: CancellationToken,
        out_tx: mpsc::Sender<OutMsg>,
    ) {
        let mut attempt = 0u32;
        // False only right after waking from sleep with the same config.
        let mut announce_load = true;
        let shared = self.shared.clone();
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let src = format!("plugin:{}", spec.name);
            let user = self.user_id();
            let plugin_dir = shared.cfg.plugin_dir(&user, &self.id, &spec.name);

            if !shared.installer.is_installed(&spec, &plugin_dir) {
                if self.storage_over_quota() {
                    self.set_plugin_status(&spec.id, "error", "存储空间超出配额，无法安装插件", None);
                    self.logs.error(&src, "存储空间超出配额，无法安装插件");
                    if !Self::sleep_or_cancel(&cancel, Duration::from_secs(300)).await {
                        return;
                    }
                    continue;
                }
                self.set_plugin_status(&spec.id, "installing", "", None);
                let res = tokio::select! {
                    _ = cancel.cancelled() => return,
                    r = async {
                        let r = {
                            let _permit = shared.install_sem.acquire().await;
                            shared.installer.ensure(&spec, &plugin_dir, &self.logs, &src).await
                        };
                        if r.is_ok() {
                            shared.gc_shared_plugins().await;
                        }
                        r
                    } => r,
                };
                if let Err(e) = res {
                    let msg = format!("安装失败: {e:#}");
                    self.logs.error(&src, msg.clone());
                    self.set_plugin_status(&spec.id, "error", &msg, None);
                    if !Self::sleep_or_cancel(&cancel, restart_backoff(attempt + 3)).await {
                        return;
                    }
                    attempt += 1;
                    continue;
                }
            }
            let Some(inst) = shared.installer.installed(&spec, &plugin_dir) else {
                // The shared install was removed after the check above, so the next pass installs again.
                attempt += 1;
                if !Self::sleep_or_cancel(&cancel, restart_backoff(attempt)).await {
                    return;
                }
                continue;
            };
            let _ = std::fs::create_dir_all(&inst.data);
            chown_tree(&shared.cfg, &inst.data);
            // The group process checks that the plugin's own packages fit with the other plugins in it.
            let grouped = !self.solo.lock().unwrap().contains(&spec.id);
            let workdir = Groups::workdir(&shared.cfg.bot_dir(&user, &self.id));
            if grouped {
                let _ = std::fs::create_dir_all(&workdir);
                chown_tree(&shared.cfg, &workdir);
            }
            let proc_id = ulid::Ulid::new().to_string().to_lowercase();
            let ls = LaunchSpec {
                proc_id: proc_id.clone(),
                plugin_id: spec.id.clone(),
                plugin_name: spec.name.clone(),
                bot_id: self.id.clone(),
                plugin_dir: inst.src.clone(),
                venv_site_packages: inst.site_packages.clone(),
                data_dir: inst.data.clone(),
                config: config.borrow_and_update().clone(),
                share_key: Some(inst.share_key.clone()),
            };
            let tenant = shared.cgroups.as_ref().map(|cg| (cg, user.as_str()));
            let ooms_before = shared.cgroups.as_ref().and_then(|cg| cg.oom_kills(&user));
            let launched = tokio::select! {
                _ = cancel.cancelled() => {
                    shared.kill_proc(&proc_id).await;
                    self.leave_dormant(&spec.id, None);
                    return
                }
                r = async {
                    if grouped {
                        shared.groups.launch(ls, out_tx.clone(), tenant, &workdir).await
                    } else {
                        launch(&shared.sidecar, ls, out_tx.clone(), tenant).await
                    }
                } => r,
            };
            if launched.is_err() {
                self.leave_dormant(&spec.id, None);
            }
            let (proc, mut exit_rx) = match launched {
                Ok(v) => v,
                Err(e) if e.downcast_ref::<Ungroupable>().is_some() => {
                    // The group process already sent the reason to the plugin log.
                    self.solo.lock().unwrap().insert(spec.id.clone());
                    continue;
                }
                Err(e) if e.downcast_ref::<LoadError>().is_some() => {
                    let detail = e.to_string();
                    self.logs
                        .error(&src, "插件加载失败，修改插件配置、更新插件或重启机器人后会重新加载");
                    self.set_plugin_status(&spec.id, "error", &detail, None);
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        updated = config.changed() => if updated.is_err() { return },
                    }
                    attempt = 0;
                    continue;
                }
                Err(e) => {
                    if grouped {
                        self.solo.lock().unwrap().insert(spec.id.clone());
                    }
                    let mut msg = format!("{e:#}");
                    if let Some(note) = self.oom_note(&user, ooms_before) {
                        msg = format!("{msg}。{note}");
                    }
                    self.logs.error(&src, format!("启动失败: {msg}"));
                    self.set_plugin_status(&spec.id, "error", &msg, None);
                    if !Self::sleep_or_cancel(&cancel, restart_backoff(attempt)).await {
                        return;
                    }
                    attempt += 1;
                    continue;
                }
            };
            let mut activity = proc.activity.subscribe();
            self.leave_dormant(&spec.id, Some(proc.clone()));
            let info = plugin_info(&proc, &spec.id);
            if announce_load {
                self.plugin_lifecycle("plugin_loaded", info.clone());
            }
            let version = Some(proc.hello.version.clone())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| spec.version.clone());
            let (shared_note, detail) = if grouped {
                ("，与同一机器人的其他轻量插件共用进程", GROUPED_DETAIL)
            } else {
                ("", "")
            };
            self.logs
                .info(&src, format!("插件已启动 pid={} 版本 {version}{shared_note}", proc.pid));
            self.set_plugin_status(&spec.id, "running", detail, Some(version.clone()));
            let started = Instant::now();
            let idle = shared.cfg.plugin_idle_sleep;
            let outcome = loop {
                let o = tokio::select! {
                    _ = cancel.cancelled() => PluginExit::Stop,
                    updated = config.changed() => if updated.is_ok() { PluginExit::Reload } else { PluginExit::Stop },
                    code = &mut exit_rx => PluginExit::Failed(code.unwrap_or(-1)),
                    _ = proc.closed.cancelled() => PluginExit::Failed(-1),
                    _ = self.wait_idle(&proc, &mut activity, idle, &src) => PluginExit::Sleep,
                };
                if matches!(o, PluginExit::Sleep) && !self.enter_dormant(&proc, &activity, &config) {
                    continue;
                }
                break o;
            };
            {
                let mut procs = self.procs.write().unwrap();
                if procs.get(&spec.id).is_some_and(|p| p.proc_id == proc_id) {
                    procs.remove(&spec.id);
                }
            }
            if !matches!(outcome, PluginExit::Sleep) {
                self.plugin_lifecycle("plugin_unloaded", info.clone());
            }
            match outcome {
                PluginExit::Stop | PluginExit::Reload => {
                    proc.shutdown();
                    if tokio::time::timeout(Duration::from_secs(3), &mut exit_rx)
                        .await
                        .is_err()
                    {
                        shared.kill_proc(&proc_id).await;
                    }
                    shared.forget_proc(&proc_id);
                    if matches!(outcome, PluginExit::Stop) {
                        return;
                    }
                    attempt = 0;
                }
                PluginExit::Sleep => {
                    proc.shutdown();
                    if tokio::time::timeout(Duration::from_secs(3), &mut exit_rx)
                        .await
                        .is_err()
                    {
                        shared.kill_proc(&proc_id).await;
                    }
                    shared.forget_proc(&proc_id);
                    let minutes = idle.map(|d| d.as_secs().div_ceil(60)).unwrap_or(0);
                    let command_only = proc.hello.wake_commands.as_ref().is_some_and(|names| !names.is_empty());
                    drop(proc);
                    let trigger = if command_only { "插件指令" } else { "消息" };
                    self.logs
                        .info(&src, format!("{minutes} 分钟未收到{trigger}，插件已休眠"));
                    let detail = format!("空闲休眠中，收到{trigger}时自动唤醒");
                    self.set_plugin_status(&spec.id, "sleeping", &detail, Some(version));
                    let mut reconfigured = false;
                    let wake_reason = loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break None,
                            changed = activity.changed() => break changed.ok().map(|()| "收到消息，正在唤醒插件"),
                            updated = config.changed() => {
                                if updated.is_err() {
                                    break None;
                                }
                                reconfigured = true;
                                if command_only {
                                    break Some("配置已更新，正在唤醒插件");
                                }
                            }
                        }
                    };
                    let Some(wake_reason) = wake_reason else {
                        self.leave_dormant(&spec.id, None);
                        self.plugin_lifecycle("plugin_unloaded", info);
                        return;
                    };
                    self.logs.info(&src, wake_reason);
                    // A sleeping plugin counts as loaded, so only a config change reloads it.
                    if reconfigured {
                        self.plugin_lifecycle("plugin_unloaded", info);
                    }
                    announce_load = reconfigured;
                    attempt = 0;
                }
                PluginExit::Failed(code) => {
                    shared.kill_proc(&proc_id).await;
                    shared.forget_proc(&proc_id);
                    if grouped {
                        // Any failure inside a group process moves the plugin to its own process.
                        self.solo.lock().unwrap().insert(spec.id.clone());
                        if code == EXIT_UNGROUPABLE {
                            attempt = 0;
                            continue;
                        }
                    }
                    let detail = proc
                        .last_error
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| format!("插件进程退出 (code {code})"));
                    let detail = match self.oom_note(&user, ooms_before).filter(|_| code < 0) {
                        Some(note) => format!("{detail}。{note}"),
                        None => detail,
                    };
                    self.logs.error(&src, format!("{detail}，稍后自动重启"));
                    self.set_plugin_status(&spec.id, "error", &detail, None);
                    if started.elapsed() > Duration::from_secs(600) {
                        attempt = 0;
                    }
                    if !Self::sleep_or_cancel(&cancel, restart_backoff(attempt)).await {
                        return;
                    }
                    attempt += 1;
                }
            }
        }
    }

    pub async fn inject_loopback(&self, input: loopback::Inject) -> Result<(), String> {
        if !self.shared.cfg.dev_services {
            return Err("FISH_DEV is required".into());
        }
        if input.text.len() > 16 * 1024
            || input.session_id.is_empty()
            || input.session_id.len() > 128
            || input.message_id.is_empty()
            || input.message_id.len() > 128
        {
            return Err("invalid loopback message".into());
        }
        let rt = self.rt.lock().await;
        let core = rt.core.as_ref().ok_or("bot is stopped")?;
        self.platforms
            .write()
            .unwrap()
            .entry(loopback::ID.into())
            .or_insert_with(|| PlatformConn::Loopback(Arc::default()));
        core.inbound_tx.try_send(input.inbound()).map_err(|e| e.to_string())
    }

    /// Passes one QQ callback delivery to the platform connection with the given id.
    pub async fn qq_webhook(self: &Arc<Self>, platform_id: &str, w: &qq::WebhookDelivery) -> qq::WebhookReply {
        let conn = match self.platforms.read().unwrap().get(platform_id) {
            Some(PlatformConn::Qq(c)) => c.clone(),
            _ => return qq::WebhookReply::BadRequest("平台未运行".into()),
        };
        let rt = self.rt.lock().await;
        let Some(core) = rt.core.as_ref() else {
            return qq::WebhookReply::BadRequest("机器人未运行".into());
        };
        let tx = core.inbound_tx.clone();
        drop(rt);
        conn.handle_webhook(w, &tx, &self.status_sink(platform_id))
    }

    pub fn loopback_outbox(&self) -> Vec<Value> {
        match self.platforms.read().unwrap().get(loopback::ID) {
            Some(PlatformConn::Loopback(c)) => c.outbox(),
            _ => vec![],
        }
    }

    // ---- message pipeline ----

    fn settings(&self) -> (BotSettings, Option<LlmSpec>) {
        let d = self.desired.read().unwrap();
        (d.settings.clone(), d.llm.clone())
    }

    fn remember_target(&self, event_id: &str, t: ReplyTarget, ev: &MessageEvent) {
        let mut g = self.inflight.lock().unwrap();
        if g.len() > 128 {
            g.retain(|_, (_, _, at)| at.elapsed() < INFLIGHT_TTL);
            if g.len() > 512 {
                g.clear();
            }
        }
        g.insert(event_id.to_string(), (t, ev.clone(), Instant::now()));
    }

    fn inflight_event(&self, event_id: &str) -> Option<(ReplyTarget, MessageEvent)> {
        self.inflight
            .lock()
            .unwrap()
            .get(event_id)
            .map(|(t, ev, _)| (t.clone(), ev.clone()))
    }

    pub fn build_event(&self, ib: &Inbound, settings: &BotSettings) -> Option<MessageEvent> {
        let b = &settings.behavior;
        let mut ids = vec![ib.sender.user_id.as_str(), ib.session_id.as_str()];
        if let Some(g) = &ib.group_id {
            ids.push(g.as_str());
        }
        if is_listed(&b.blocklist, ib.platform_kind, &ids) {
            return None;
        }
        let w = wake(b, ib.message_type, ib.is_at, &ib.text);
        Some(MessageEvent {
            platform: ib.platform_kind.to_string(),
            bot_id: self.id.clone(),
            self_id: ib.self_id.clone(),
            message_type: ib.message_type,
            group_id: ib.group_id.clone(),
            session_id: ib.session_id.clone(),
            message_id: ib.msg_id.clone(),
            unified_msg_origin: unified_origin(&ib.platform_id, ib.message_type, &ib.session_id),
            sender: ib.sender.clone(),
            message_str: w.text,
            chain: ib.chain.clone(),
            is_at_or_wake: w.woke,
            is_admin: is_listed(&b.admins, ib.platform_kind, &[ib.sender.user_id.as_str()]),
            admins: listed_ids(&b.admins, ib.platform_kind),
            timestamp: crate::sign::now_ms(),
            raw: Some(ib.raw.clone()),
        })
    }

    /// Records the session and sender of an inbound message, writing each at most once per `SEEN_WRITE_EVERY`.
    fn note_seen(&self, ev: &MessageEvent) {
        let origin = ev.unified_msg_origin.as_str();
        let entries = [("session", origin), ("user", ev.sender.user_id.as_str())];
        let now = Instant::now();
        for (kind, id) in entries {
            if id.is_empty() {
                continue;
            }
            {
                let mut g = self.seen_written.lock().unwrap();
                let key = (kind, id.to_string());
                if g.get(&key).is_some_and(|t| now.duration_since(*t) < SEEN_WRITE_EVERY) {
                    continue;
                }
                if g.len() > 1024 {
                    g.retain(|_, t| now.duration_since(*t) < SEEN_WRITE_EVERY);
                }
                g.insert(key, now);
            }
            let _ = self.shared.conv.touch_seen(&self.id, kind, id, origin);
        }
    }

    async fn handle_inbound(self: Arc<Self>, ib: Inbound) {
        let (settings, llm_spec) = self.settings();
        let Some(ev) = self.build_event(&ib, &settings) else {
            return;
        };
        self.note_seen(&ev);
        if ev.is_at_or_wake {
            let key = format!("{}:{}", ib.platform_id, ib.sender.user_id);
            let ok = self
                .rate
                .lock()
                .unwrap()
                .allow(&key, settings.behavior.rate_limit_per_minute, Instant::now());
            if !ok {
                self.logs
                    .warn("core", format!("{} 触发频率限制，消息已忽略", ib.sender.user_id));
                return;
            }
        }
        let event_id = ulid::Ulid::new().to_string().to_lowercase();
        let target = ReplyTarget {
            platform_id: ib.platform_id.clone(),
            mt: ib.message_type,
            session_id: ib.session_id.clone(),
            msg_id: Some(ib.msg_id.clone()),
        };
        if let Some(text) = sid_reply(&ev, &ib.text) {
            self.send_to_target(&target, &[Component::plain(text)]).await;
            return;
        }
        self.remember_target(&event_id, target.clone(), &ev);

        let procs = self.awake_procs(&ev).await;
        let results = futures_util::future::join_all(procs.iter().map(|p| {
            let (mrx, drx) = p.dispatch(&event_id, &ev);
            let event_id = event_id.clone();
            async move {
                let matched = matches!(tokio::time::timeout(MATCH_WAIT, mrx).await, Ok(Ok(true)));
                if !matched {
                    p.forget_event(&event_id);
                    return None;
                }
                match tokio::time::timeout(DONE_WAIT, drx).await {
                    Ok(Ok(r)) => Some(r),
                    _ => {
                        p.forget_event(&event_id);
                        Some((false, false))
                    }
                }
            }
        }))
        .await;
        let matched: Vec<(bool, bool)> = results.into_iter().flatten().collect();
        let run_llm = ev.is_at_or_wake && (matched.is_empty() || matched.iter().all(|(_, call_llm)| *call_llm));
        if run_llm {
            self.default_chat(&ev, &event_id, &target, &settings, llm_spec).await;
        }
    }

    /// Answers a message no plugin handled with the persona and the chat history.
    async fn default_chat(
        &self,
        ev: &MessageEvent,
        event_id: &str,
        target: &ReplyTarget,
        s: &BotSettings,
        spec: Option<LlmSpec>,
    ) {
        if ev.message_str.trim().is_empty() {
            return;
        }
        let Some(spec) = spec.filter(|l| !l.api_key.is_empty()) else {
            self.logs.warn("llm", "未配置 LLM 密钥，无法回复");
            self.notify_trouble(target, &ev.unified_msg_origin, llm::Trouble::NoKey)
                .await;
            return;
        };
        let session = &ev.unified_msg_origin;
        let turns = s.llm.context_turns.min(100) as usize;
        let history = self
            .shared
            .conv
            .recent(&self.id, session, turns * 2)
            .unwrap_or_default();
        let system_prompt = if s.persona.system_prompt.trim().is_empty() {
            format!(
                "你是{}，一个友好的聊天助手。",
                if s.persona.name.is_empty() {
                    "大肥鱼"
                } else {
                    &s.persona.name
                }
            )
        } else {
            s.persona.system_prompt.clone()
        };
        let request = json!({
            "prompt": ev.message_str,
            "system_prompt": system_prompt,
            "contexts": history.iter().map(|(r, c)| json!({"role": r, "content": c})).collect::<Vec<_>>(),
            "image_urls": [],
        });
        let Ok(reply) = self.run_agent(ev, event_id, target, s, &spec, request, false).await else {
            return;
        };
        if !reply.text.is_empty() {
            let prompt = reply.prompt.unwrap_or_else(|| ev.message_str.clone());
            let _ = self.shared.conv.append(&self.id, session, "user", &prompt);
            let _ = self.shared.conv.append(&self.id, session, "assistant", &reply.text);
            self.deliver(Some((event_id, ev)), target, vec![Component::plain(reply.text)], true)
                .await;
        } else if !reply.sent_by_tools {
            self.logs.warn("llm", "模型返回了空回复");
        }
    }

    /// Explains a plugin exit when the kernel killed a process of `user` for memory after `before` was read.
    fn oom_note(&self, user: &str, before: Option<u64>) -> Option<String> {
        let cg = self.shared.cgroups.as_ref()?;
        if cg.oom_kills(user)? <= before? {
            return None;
        }
        let limit = cg
            .memory_max(user)
            .map(|b| format!(" {} MB", b / (1024 * 1024)))
            .unwrap_or_default();
        Some(format!(
            "这个用户的插件合计超过了内存上限{limit}，系统终止了占用较多的插件进程。可以停用部分插件或提高内存配额"
        ))
    }

    /// Running plugins that have `kind` hooks, highest priority first.
    fn hook_procs(&self, kind: &str) -> Vec<Arc<PluginProc>> {
        let mut procs: Vec<Arc<PluginProc>> = self
            .procs
            .read()
            .unwrap()
            .values()
            .filter(|p| p.has_handler(kind))
            .cloned()
            .collect();
        procs.sort_by_key(|p| -p.handler_priority(kind));
        procs
    }

    /// Runs the `kind` hooks of every running plugin in priority order, skipping `skip_plugin`.
    /// Each plugin sees the payload as the plugins before it left it. When `stoppable`, a plugin
    /// that stops the event ends the run. Returns the final payload and whether a plugin stopped
    /// the event.
    async fn run_hooks(
        &self,
        kind: &str,
        event: Option<(&str, &MessageEvent)>,
        mut payload: Value,
        wait: Duration,
        stoppable: bool,
        skip_plugin: Option<&str>,
    ) -> (Value, bool) {
        let mut stopped = false;
        for p in self.hook_procs(kind) {
            if skip_plugin == Some(p.plugin_id.as_str()) {
                continue;
            }
            let Some(r) = p.hook(kind, event, &payload, wait).await else {
                if !p.closed.is_cancelled() {
                    self.logs.warn(
                        &format!("plugin:{}", p.plugin_name),
                        format!("{kind} 钩子超过 {} 秒没有返回", wait.as_secs()),
                    );
                }
                continue;
            };
            if let (Some(changed), Some(current)) =
                (r.get("payload").and_then(Value::as_object), payload.as_object_mut())
            {
                for (k, v) in changed {
                    current.insert(k.clone(), v.clone());
                }
            }
            if r.get("stopped").and_then(Value::as_bool) == Some(true) {
                stopped = true;
                if stoppable {
                    break;
                }
            }
        }
        (payload, stopped)
    }

    /// Runs one LLM turn the way AstrBot's main agent does. The `llm_request` hooks may change
    /// the request, then the model answers with every plugin's tools, and the `agent_begin`,
    /// `using_llm_tool`, `llm_tool_respond`, `llm_response` and `agent_done` hooks run around it.
    /// `request` holds `prompt`, `system_prompt`, `contexts` and `image_urls`.
    /// Fails with None when a hook stopped the request, or with the error of the model call.
    #[allow(clippy::too_many_arguments)]
    async fn run_agent(
        &self,
        ev: &MessageEvent,
        event_id: &str,
        target: &ReplyTarget,
        s: &BotSettings,
        spec: &LlmSpec,
        mut request: Value,
        plugin_request: bool,
    ) -> Result<AgentReply, Option<String>> {
        let event = Some((event_id, ev));
        let session = &ev.unified_msg_origin;
        request["plugin_request"] = json!(plugin_request);
        let (request, stopped) = self
            .run_hooks("llm_request", event, request, HOOK_WAIT, true, None)
            .await;
        if stopped {
            return Err(None);
        }
        let prompt = request.get("prompt").and_then(Value::as_str).map(str::to_string);
        let contexts = request
            .get("contexts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let image_urls: Vec<String> = request
            .get("image_urls")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut messages = vec![];
        if let Some(sp) = request
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            messages.push(ChatMessage::new("system", sp));
        }
        messages.extend(llm::plugin_contexts(spec, &contexts));
        if prompt.is_some() || !image_urls.is_empty() || contexts.is_empty() {
            messages.push(ChatMessage::new("user", prompt.clone().unwrap_or_default()));
        }
        let model = if s.llm.model.is_empty() {
            spec.model.clone()
        } else {
            s.llm.model.clone()
        };
        let (tools, owners) = self.collect_tools();
        let mut tools_on = !tools.is_empty();
        let mut images_on = true;
        let mut image_turns: Vec<usize> = vec![];
        let mut sent_by_tools = false;
        let mut text = String::new();
        self.run_hooks(
            "agent_begin",
            event,
            json!({"messages": messages.iter().map(message_json).collect::<Vec<_>>()}),
            HOOK_WAIT,
            true,
            None,
        )
        .await;
        for round in 0..=MAX_TOOL_ROUNDS {
            let req = ChatRequest {
                spec,
                model: &model,
                messages: messages.clone(),
                image_urls: image_urls.clone(),
                temperature: s.llm.temperature,
                max_tokens: s.llm.max_tokens.max(16),
                tools: if tools_on { tools.clone() } else { vec![] },
                tool_choice_none: tools_on && round == MAX_TOOL_ROUNDS,
            };
            let reply = match llm::chat_reply(&self.shared.http, req).await {
                Ok(r) => r,
                Err(e) if !image_turns.is_empty() && is_client_error(&e) => {
                    self.logs
                        .warn("llm", format!("模型不接受工具返回的图片，这次不带图片继续: {e:#}"));
                    for i in image_turns.drain(..).rev() {
                        messages.remove(i);
                    }
                    images_on = false;
                    continue;
                }
                Err(e) if tools_on && round == 0 && is_client_error(&e) => {
                    self.logs
                        .warn("llm", format!("模型不接受插件工具，这次不带工具回复: {e:#}"));
                    tools_on = false;
                    continue;
                }
                Err(e) => {
                    self.logs.error("llm", format!("{e:#}"));
                    if let Some(t) = llm::trouble(&e) {
                        self.notify_trouble(target, session, t).await;
                    }
                    return Err(Some(format!("{e:#}")));
                }
            };
            if !tools_on || reply.tool_calls.is_empty() {
                text = reply.text;
                break;
            }
            messages.push(ChatMessage::assistant_raw(
                reply.raw.unwrap_or_else(|| json!({"role": "assistant"})),
            ));
            let mut images: Vec<Value> = vec![];
            for call in reply.tool_calls {
                let tool = tools
                    .iter()
                    .find(|t| t.name == call.name)
                    .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.parameters}))
                    .unwrap_or_else(|| json!({"name": call.name}));
                self.run_hooks(
                    "using_llm_tool",
                    event,
                    json!({"tool": tool, "arguments": call.arguments}),
                    HOOK_WAIT,
                    true,
                    None,
                )
                .await;
                let out = self
                    .run_tool(&owners, ev, event_id, &call.name, &call.arguments, &mut sent_by_tools)
                    .await;
                self.run_hooks(
                    "llm_tool_respond",
                    event,
                    json!({"tool": tool, "arguments": call.arguments, "result": out.text}),
                    HOOK_WAIT,
                    true,
                    None,
                )
                .await;
                messages.push(ChatMessage::tool_result(
                    &call.id,
                    truncate_chars(&out.text, MAX_TOOL_RESULT_CHARS),
                ));
                if images_on {
                    for url in out.images.into_iter().take(MAX_TOOL_IMAGES) {
                        images.push(json!({"type": "text", "text": format!("[Image from tool '{}']", call.name)}));
                        images.push(json!({"type": "image_url", "image_url": {"url": url}}));
                    }
                }
            }
            if !images.is_empty() {
                let turn = json!({"role": "user", "content": images});
                for m in llm::plugin_contexts(spec, &[turn]) {
                    image_turns.push(messages.len());
                    messages.push(m);
                }
            }
        }
        let (response, _) = self
            .run_hooks(
                "llm_response",
                event,
                json!({"completion_text": text}),
                HOOK_WAIT,
                true,
                None,
            )
            .await;
        let text = response
            .get("completion_text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !text.is_empty() {
            messages.push(ChatMessage::new("assistant", text.clone()));
        }
        self.run_hooks(
            "agent_done",
            event,
            json!({"messages": messages.iter().map(message_json).collect::<Vec<_>>(), "completion_text": text}),
            HOOK_WAIT,
            true,
            None,
        )
        .await;
        Ok(AgentReply {
            prompt,
            text,
            sent_by_tools,
        })
    }

    /// Sends a result the way AstrBot's result decorate and respond stages do. The
    /// `decorating_result` hooks of every plugin may change the chain, then the chain is sent
    /// and the `after_message_sent` hooks run. Without an event the chain is sent as it is.
    /// Returns true when something was sent.
    async fn deliver(
        &self,
        event: Option<(&str, &MessageEvent)>,
        target: &ReplyTarget,
        mut chain: Vec<Component>,
        llm: bool,
    ) -> bool {
        if event.is_some() {
            let (payload, _) = self
                .run_hooks(
                    "decorating_result",
                    event,
                    json!({"chain": chain, "llm": llm}),
                    DECORATE_WAIT,
                    false,
                    None,
                )
                .await;
            if let Ok(decorated) = serde_json::from_value::<Vec<Component>>(payload["chain"].clone()) {
                chain = decorated;
            }
        }
        if chain.is_empty() {
            return false;
        }
        self.send_to_target(target, &chain).await;
        if event.is_some() {
            self.run_hooks(
                "after_message_sent",
                event,
                json!({"chain": chain, "llm": llm}),
                HOOK_WAIT,
                true,
                None,
            )
            .await;
        }
        true
    }

    /// Runs a `ProviderRequest` a plugin handler yielded and sends the reply. Returns the `agent_result` message.
    #[allow(clippy::too_many_arguments)]
    async fn plugin_agent(
        &self,
        req_id: &str,
        event_id: Option<String>,
        prompt: Option<String>,
        system_prompt: Option<String>,
        contexts: Vec<Value>,
        image_urls: Vec<String>,
    ) -> Value {
        let Some((event_id, (target, ev))) = event_id.and_then(|id| self.inflight_event(&id).map(|e| (id, e))) else {
            return json!({"op": "agent_result", "req_id": req_id, "error": "这条消息已过期，无法再请求模型"});
        };
        let (s, spec) = self.settings();
        let Some(spec) = spec.filter(|l| !l.api_key.is_empty()) else {
            return json!({"op": "agent_result", "req_id": req_id, "error": "未配置 LLM 密钥"});
        };
        let request = json!({
            "prompt": prompt,
            "system_prompt": system_prompt.unwrap_or_default(),
            "contexts": contexts,
            "image_urls": image_urls,
        });
        match self.run_agent(&ev, &event_id, &target, &s, &spec, request, true).await {
            Ok(reply) => {
                if !reply.text.is_empty() {
                    self.deliver(
                        Some((&event_id, &ev)),
                        &target,
                        vec![Component::plain(reply.text)],
                        true,
                    )
                    .await;
                }
                json!({"op": "agent_result", "req_id": req_id})
            }
            Err(None) => json!({"op": "agent_result", "req_id": req_id, "stopped": true}),
            Err(Some(e)) => json!({"op": "agent_result", "req_id": req_id, "error": e}),
        }
    }

    /// Tells running plugins that `proc`'s plugin was loaded or unloaded.
    /// Runs the `kind` lifecycle hooks of running plugins in the background. `plugin` is the
    /// `plugin_info` of the plugin that was loaded or unloaded.
    fn plugin_lifecycle(self: &Arc<Self>, kind: &'static str, plugin: Value) {
        let bot = self.clone();
        tokio::spawn(async move {
            bot.run_hooks(kind, None, json!({"plugin": plugin}), HOOK_WAIT, false, None)
                .await
        });
    }

    /// Tells the chat why there is no reply, at most once per `Trouble::cooldown` for each session and kind.
    async fn notify_trouble(&self, target: &ReplyTarget, session: &str, trouble: llm::Trouble) {
        let now = Instant::now();
        {
            let mut notices = self.notices.lock().unwrap();
            let key = (session.to_string(), trouble);
            if notices
                .get(&key)
                .is_some_and(|t| now.duration_since(*t) < trouble.cooldown())
            {
                return;
            }
            if notices.len() > 1024 {
                notices.retain(|(_, k), t| now.duration_since(*t) < k.cooldown());
            }
            notices.insert(key, now);
        }
        self.send_to_target(target, &[Component::plain(trouble.notice())]).await;
    }

    /// Function tools of all running plugins. The first plugin, by name, wins a duplicate tool name.
    fn collect_tools(&self) -> (Vec<ToolDef>, HashMap<String, Arc<PluginProc>>) {
        let mut procs: Vec<Arc<PluginProc>> = self.procs.read().unwrap().values().cloned().collect();
        procs.sort_by(|a, b| a.plugin_name.cmp(&b.plugin_name));
        let mut defs = vec![];
        let mut owners: HashMap<String, Arc<PluginProc>> = HashMap::new();
        for p in procs {
            for t in p.tools() {
                let Some(tool) = tool_definition(t) else { continue };
                if owners.contains_key(&tool.name) {
                    self.logs.warn(
                        &format!("plugin:{}", p.plugin_name),
                        format!("工具 {} 与其他插件重名，已忽略", tool.name),
                    );
                    continue;
                }
                owners.insert(tool.name.clone(), p.clone());
                defs.push(tool);
            }
        }
        (defs, owners)
    }

    /// Runs one tool call and returns the text and images handed back to the model.
    async fn run_tool(
        &self,
        owners: &HashMap<String, Arc<PluginProc>>,
        ev: &MessageEvent,
        event_id: &str,
        name: &str,
        arguments: &Value,
        sent: &mut bool,
    ) -> ToolOutput {
        let Some(p) = owners.get(name) else {
            return format!("没有名为 {name} 的工具").into();
        };
        let src = format!("plugin:{}", p.plugin_name);
        self.logs.info(&src, format!("模型调用工具 {name}"));
        let call_id = ulid::Ulid::new().to_string().to_lowercase();
        let Some(v) = p.tool_call(&call_id, event_id, ev, name, arguments, TOOL_WAIT).await else {
            self.logs
                .warn(&src, format!("工具 {name} 超过 {} 秒没有返回", TOOL_WAIT.as_secs()));
            return "工具执行超时".into();
        };
        if v.get("sent").and_then(Value::as_bool) == Some(true) {
            *sent = true;
        }
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            self.logs.warn(&src, format!("工具 {name} 出错: {err}"));
            return err.into();
        }
        ToolOutput {
            text: v.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
            images: v
                .get("images")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|u| u.starts_with("data:image/") || u.starts_with("https://"))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    async fn send_to_target(&self, t: &ReplyTarget, chain: &[Component]) {
        let conn = self.platforms.read().unwrap().get(&t.platform_id).cloned();
        let Some(conn) = conn else {
            self.logs
                .warn("core", format!("平台 {} 未连接，消息未发送", t.platform_id));
            return;
        };
        let source = match &conn {
            PlatformConn::Loopback(_) => loopback::KIND,
            PlatformConn::Qq(_) => PLATFORM_QQ,
            PlatformConn::Weixin(_) => PLATFORM_WEIXIN,
        };
        if let Err(e) = conn.send(t.mt, &t.session_id, chain, t.msg_id.as_deref()).await {
            self.logs.error(source, format!("发送失败: {e:#}"));
        }
    }

    fn target_for(&self, event_id: Option<&str>, session: Option<&str>) -> Option<ReplyTarget> {
        if let Some(id) = event_id {
            if let Some((t, _, _)) = self.inflight.lock().unwrap().get(id) {
                return Some(t.clone());
            }
        }
        let (platform_id, mt, session_id) = parse_origin(session?)?;
        Some(ReplyTarget {
            platform_id,
            mt,
            session_id,
            msg_id: None,
        })
    }

    async fn handle_out(self: Arc<Self>, m: OutMsg) {
        let src = format!("plugin:{}", m.plugin_name);
        match m.msg {
            PluginOut::Log { level, message } => self.logs.push(log_level(&level), &src, message),
            PluginOut::Send {
                event_id,
                session,
                chain,
            } => match self.target_for(event_id.as_deref(), session.as_deref()) {
                Some(t) => self.send_to_target(&t, &chain).await,
                None => self.logs.warn(&src, "无法确定回复目标，消息未发送"),
            },
            PluginOut::SendTo { session, chain } => match self.target_for(None, Some(&session)) {
                Some(t) => self.send_to_target(&t, &chain).await,
                None => self.logs.warn(&src, format!("无效的会话 {session}")),
            },
            PluginOut::Llm {
                req_id,
                prompt,
                system_prompt,
                contexts,
                image_urls,
                tools,
            } => {
                let (s, spec) = self.settings();
                let reply = match spec.filter(|l| !l.api_key.is_empty()) {
                    None => {
                        json!({"op": "llm_result", "req_id": req_id, "error": "未配置 LLM 密钥"})
                    }
                    Some(spec) => {
                        let mut messages = vec![];
                        if let Some(sp) = system_prompt.filter(|s| !s.is_empty()) {
                            messages.push(ChatMessage::new("system", sp));
                        }
                        messages.extend(llm::plugin_contexts(&spec, &contexts));
                        if prompt.is_some() || !image_urls.is_empty() || contexts.is_empty() {
                            messages.push(ChatMessage::new("user", prompt.unwrap_or_default()));
                        }
                        let model = if s.llm.model.is_empty() {
                            spec.model.clone()
                        } else {
                            s.llm.model.clone()
                        };
                        let req = ChatRequest {
                            spec: &spec,
                            model: &model,
                            messages,
                            image_urls,
                            temperature: s.llm.temperature,
                            max_tokens: s.llm.max_tokens.max(16),
                            tools: tools.iter().filter_map(tool_definition).collect(),
                            tool_choice_none: false,
                        };
                        match llm::chat_reply(&self.shared.http, req).await {
                            Ok(reply) => reply.plugin_result(&req_id),
                            Err(e) => {
                                self.logs.error("llm", format!("{src} 调用失败: {e:#}"));
                                json!({"op": "llm_result", "req_id": req_id, "error": format!("{e:#}")})
                            }
                        }
                    }
                };
                m.writer.send(&reply);
            }
            PluginOut::Result {
                req_id,
                event_id,
                session,
                chain,
                llm,
            } => {
                let sent = match self.target_for(event_id.as_deref(), session.as_deref()) {
                    Some(t) => {
                        let ev = event_id
                            .as_deref()
                            .and_then(|id| self.inflight_event(id).map(|(_, ev)| (id, ev)));
                        self.deliver(ev.as_ref().map(|(id, ev)| (*id, ev)), &t, chain, llm)
                            .await
                    }
                    None => {
                        self.logs.warn(&src, "无法确定回复目标，消息未发送");
                        false
                    }
                };
                m.writer
                    .send(&json!({"op": "result_done", "req_id": req_id, "sent": sent}));
            }
            PluginOut::Agent {
                req_id,
                event_id,
                session: _,
                prompt,
                system_prompt,
                contexts,
                image_urls,
            } => {
                let reply = self
                    .plugin_agent(&req_id, event_id, prompt, system_prompt, contexts, image_urls)
                    .await;
                m.writer.send(&reply);
            }
            PluginOut::PluginError {
                event_id,
                handler,
                error,
                traceback,
            } => {
                let Some((id, (_, ev))) = event_id.and_then(|id| self.inflight_event(&id).map(|e| (id, e))) else {
                    return;
                };
                let payload =
                    json!({"plugin": m.plugin_name, "handler": handler, "error": error, "traceback": traceback});
                self.run_hooks(
                    "plugin_error",
                    Some((&id, &ev)),
                    payload,
                    HOOK_WAIT,
                    true,
                    Some(&m.plugin_id),
                )
                .await;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn platforms_len(&self) -> usize {
        self.platforms.read().unwrap().len()
    }
}

/// The `plugin` payload of lifecycle hooks.
fn plugin_info(proc: &PluginProc, plugin_id: &str) -> Value {
    let name = if proc.hello.name.is_empty() {
        &proc.plugin_name
    } else {
        &proc.hello.name
    };
    json!({"id": plugin_id, "name": name, "version": proc.hello.version})
}

/// Images a tool may hand to the model in one call.
const MAX_TOOL_IMAGES: usize = 4;

/// What one LLM turn produced.
struct AgentReply {
    /// The user prompt after the `llm_request` hooks.
    prompt: Option<String>,
    /// The reply after the `llm_response` hooks.
    text: String,
    /// True when a tool already sent something to the chat.
    sent_by_tools: bool,
}

/// What a tool call hands back to the model.
struct ToolOutput {
    text: String,
    /// `data:` or `https:` URLs of images the tool returned.
    images: Vec<String>,
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        ToolOutput { text, images: vec![] }
    }
}

impl From<&str> for ToolOutput {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

/// The OpenAI style form of a message, as AstrBot's agent hooks see it.
fn message_json(m: &ChatMessage) -> Value {
    match &m.raw {
        Some(raw) if raw.is_object() => raw.clone(),
        Some(Value::Array(blocks)) => {
            let text: String = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            json!({"role": m.role, "content": text})
        }
        _ => match &m.tool_call_id {
            Some(id) => json!({"role": "tool", "tool_call_id": id, "content": m.content}),
            None => json!({"role": m.role, "content": m.content}),
        },
    }
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn tool_definition(value: &Value) -> Option<ToolDef> {
    let name = value.get("name")?.as_str()?;
    valid_tool_name(name).then(|| ToolDef {
        name: name.to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        parameters: value
            .get("parameters")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…（已截断）", &s[..i]),
        None => s.to_string(),
    }
}

/// True for an HTTP 4xx answer from the provider, which usually means the request shape was rejected.
/// A 4xx response that a request without tools may avoid.
fn is_client_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<llm::ApiError>()
        .and_then(|a| a.status)
        .is_some_and(|s| s.is_client_error())
        && !llm::trouble(e).is_some_and(|t| t.is_account_problem() || t == llm::Trouble::Busy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Sender;

    pub(crate) fn test_shared(dir: &std::path::Path) -> Arc<Shared> {
        let cfg = Config {
            node_id: "n".into(),
            secret: b"k".to_vec(),
            secret_text: "k".into(),
            console_url: None,
            listen: "127.0.0.1:0".into(),
            data_dir: dir.to_path_buf(),
            sidecar_python: "python3".into(),
            sidecar_dir: dir.to_path_buf(),
            uv: "uv".into(),
            dev: true,
            dev_services: false,
            plugin_uid: None,
            plugin_gid: None,
            plugin_idle_sleep: None,
        };
        let http = reqwest::Client::new();
        let sidecar = Arc::new(Sidecar::new(
            crate::plugin::SidecarCommand {
                program: "false".into(),
                args: vec![],
                envs: vec![],
                cwd: None,
                uid: None,
                gid: None,
            },
            dir.join("run"),
        ));
        Arc::new(Shared {
            installer: Installer {
                http: http.clone(),
                uv: "uv".into(),
                python: "python3".into(),
                python_version: String::new(),
                cache_dir: dir.join("uv"),
                shared_dir: dir.join("shared"),
                codeload_base: None,
                archive_proxy: None,
            },
            sidecar: sidecar.clone(),
            groups: Arc::new(Groups::new(sidecar)),
            db: Arc::new(NodeDb::open(&dir.join("node.db")).unwrap()),
            conv: Arc::new(ConvDb::open(&dir.join("c.db")).unwrap()),
            events: Events::disabled(),
            cgroups: None,
            install_sem: Semaphore::new(1),
            storage_by_user: Mutex::new(HashMap::new()),
            cfg,
            http,
        })
    }

    fn inbound(text: &str, mt: MessageType, is_at: bool) -> Inbound {
        Inbound {
            platform_id: "p1".into(),
            platform_kind: PLATFORM_QQ,
            self_id: "me".into(),
            message_type: mt,
            group_id: (mt == MessageType::Group).then(|| "g1".to_string()),
            session_id: if mt == MessageType::Group {
                "g1".into()
            } else {
                "u1".into()
            },
            sender: Sender {
                user_id: "u1".into(),
                nickname: "u1".into(),
            },
            text: text.into(),
            chain: vec![Component::plain(text)],
            is_at,
            msg_id: "m1".into(),
            raw: Value::Null,
        }
    }

    #[test]
    fn sid_answers_only_the_prefixed_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = DesiredBot {
            bot_id: "b".into(),
            user_id: "u".into(),
            ..Default::default()
        };
        d.settings.behavior.reply_in_group_without_at = false;
        let bot = Bot::new(test_shared(dir.path()), d.clone());
        let reply = |text: &str, mt: MessageType| {
            let ib = inbound(text, mt, false);
            let ev = bot.build_event(&ib, &d.settings).unwrap();
            sid_reply(&ev, &ib.text)
        };
        let group = reply("/sid", MessageType::Group).unwrap();
        assert!(group.contains("当前会话：p1:GroupMessage:g1"));
        assert!(group.contains("你的账号：u1"));
        assert!(group.contains("群号：g1"));
        assert!(reply("/SID", MessageType::Private).is_some());
        assert!(reply("sid", MessageType::Private).is_none());
        assert!(reply("sid", MessageType::Group).is_none());
        assert!(reply("/sid please", MessageType::Private).is_none());
        d.settings.behavior.wake_prefixes = vec!["小鱼".into()];
        let ib = inbound("小鱼 sid", MessageType::Group, false);
        let ev = bot.build_event(&ib, &d.settings).unwrap();
        assert!(sid_reply(&ev, &ib.text).is_some());
    }

    #[test]
    fn event_building_applies_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = DesiredBot {
            bot_id: "b".into(),
            user_id: "u".into(),
            ..Default::default()
        };
        d.settings.behavior.admins = vec!["qq:u1".into()];
        let bot = Bot::new(test_shared(dir.path()), d.clone());
        let ev = bot
            .build_event(&inbound("/help", MessageType::Group, false), &d.settings)
            .unwrap();
        assert!(ev.is_at_or_wake);
        assert!(ev.is_admin);
        assert_eq!(ev.admins, vec!["u1"]);
        assert_eq!(ev.message_str, "help");
        assert_eq!(ev.unified_msg_origin, "p1:GroupMessage:g1");
        let ev = bot
            .build_event(&inbound("hi", MessageType::Group, false), &d.settings)
            .unwrap();
        assert!(!ev.is_at_or_wake);
        d.settings.behavior.blocklist = vec!["qq:g1".into()];
        assert!(bot
            .build_event(&inbound("hi", MessageType::Group, true), &d.settings)
            .is_none());
    }

    #[tokio::test]
    async fn removed_and_cleared_plugin_files_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let shared = test_shared(dir.path());
        let mut d = DesiredBot {
            bot_id: "b".into(),
            user_id: "u".into(),
            running: false,
            ..Default::default()
        };
        d.plugins.push(PluginSpec {
            id: "kept".into(),
            name: "kept_plugin".into(),
            enabled: false,
            ..Default::default()
        });
        let kept = shared.cfg.plugin_dir("u", "b", "kept_plugin");
        let gone = shared.cfg.plugin_dir("u", "b", "gone_plugin");
        for p in [&kept, &gone] {
            std::fs::create_dir_all(p.join("data")).unwrap();
            std::fs::write(p.join("data").join("x"), b"1").unwrap();
        }
        let bot = Bot::new(shared, d.clone());
        bot.apply(d).await;
        assert!(!gone.exists());
        assert!(kept.join("data").join("x").exists());
        bot.clear_plugin_data("kept").await.unwrap();
        assert!(kept.exists());
        assert!(!kept.join("data").exists());
        assert!(bot.clear_plugin_data("missing").await.is_err());
    }

    #[tokio::test]
    async fn apply_and_stop_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = DesiredBot {
            bot_id: "b".into(),
            user_id: "u".into(),
            ..Default::default()
        };
        d.platforms.push(PlatformSpec {
            id: "p".into(),
            kind: "unknown".into(),
            enabled: true,
            ..Default::default()
        });
        d.platforms.push(PlatformSpec {
            id: "q".into(),
            kind: PLATFORM_QQ.into(),
            enabled: false,
            ..Default::default()
        });
        let bot = Bot::new(test_shared(dir.path()), d.clone());
        bot.apply(d.clone()).await;
        let s = bot.status_json();
        assert_eq!(s["status"], "running");
        assert_eq!(s["platforms"][0]["status"], "error");
        assert_eq!(s["platforms"][1]["status"], "disabled");
        assert_eq!(bot.platforms_len(), 0);
        d.running = false;
        bot.apply(d).await;
        assert_eq!(bot.status_json()["status"], "stopped");
    }
}
