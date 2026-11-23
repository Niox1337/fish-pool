use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::bot::Bot;
use crate::model::Quota;
use crate::node::Node;
use crate::plugin::install::dir_size;
use crate::sandbox::{tenant_limit_bytes, BURST_FACTOR};
use crate::sign::now_ms;

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
pub const OVER_QUOTA_SAMPLES: u32 = 3;
/// How often kernel-limited tenants are checked for memory bursts.
pub const BURST_CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// How long a kernel-limited tenant may stay above its quota before its largest plugin stops.
pub const BURST_WINDOW: Duration = Duration::from_secs(180);
/// Node memory that bursts never use.
const NODE_RESERVE: u64 = 256 * MIB;
const MIB: u64 = 1024 * 1024;
const CPU_PLUGIN_ROWS: usize = 20;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Source {
    pub bot_id: String,
    pub kind: &'static str,
    pub plugin_id: Option<String>,
    pub label: String,
    pub memory_bytes: u64,
    /// Files the tenant owns, counted against the storage quota.
    pub storage_bytes: u64,
    /// Apparent size of the shared code and dependency install a plugin uses. Not counted against the quota.
    pub deps_bytes: u64,
    pub cpu_percent: f64,
}

#[derive(Serialize)]
struct PluginCpu {
    user_id: String,
    bot_id: String,
    bot_name: String,
    plugin_id: String,
    label: String,
    cpu_percent: f64,
}

#[derive(Serialize)]
struct PluginCpuSample {
    percent: f64,
    sampled_at: i64,
    plugins: Vec<PluginCpu>,
}

/// `VmRSS` of `/proc/<pid>/status`, in bytes.
pub fn parse_status_rss(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

/// utime + stime clock ticks from `/proc/<pid>/stat`.
pub fn parse_stat_ticks(text: &str) -> Option<u64> {
    let after = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

#[derive(Default)]
pub struct Usage {
    snapshot: Mutex<HashMap<String, Vec<Source>>>,
    plugin_cpu: Mutex<Option<PluginCpuSample>>,
    cpu_prev: Mutex<HashMap<u32, (u64, Instant)>>,
    over_count: Mutex<HashMap<String, u32>>,
    /// Kernel-limited tenants above their quota, with the time each went over.
    over_since: Mutex<HashMap<String, Instant>>,
}

/// `memory.max` for a tenant with `quota` bytes that uses `working_set` bytes while the node has `available` bytes left.
/// Half of the node memory above `reserve` is offered on top of the current use, within the quota and `BURST_FACTOR` times the quota.
pub fn burst_limit(quota: u64, working_set: u64, available: u64, reserve: u64) -> u64 {
    let spare = available.saturating_sub(reserve) / 2;
    working_set.saturating_add(spare).clamp(quota, quota * BURST_FACTOR)
}

impl Usage {
    /// Returns `(memory_bytes, cpu_percent)` of a process. Memory is private clean plus private dirty from `smaps_rollup`, or `VmRSS` when that file is unreadable.
    pub async fn process(&self, pid: u32) -> (u64, f64) {
        if pid == 0 {
            return (0, 0.0);
        }
        if cfg!(target_os = "linux") {
            let mem = match tokio::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .await
                .ok()
                .and_then(|t| crate::system::parse_rollup(&t))
                .and_then(|m| m.uss)
            {
                Some(m) => m,
                None => tokio::fs::read_to_string(format!("/proc/{pid}/status"))
                    .await
                    .ok()
                    .and_then(|t| parse_status_rss(&t))
                    .unwrap_or(0),
            };
            let cpu = match tokio::fs::read_to_string(format!("/proc/{pid}/stat"))
                .await
                .ok()
                .and_then(|t| parse_stat_ticks(&t))
            {
                Some(ticks) => {
                    let now = Instant::now();
                    let mut g = self.cpu_prev.lock().unwrap();
                    let pct = match g.get(&pid) {
                        Some((prev, at)) if ticks >= *prev => {
                            let secs = now.duration_since(*at).as_secs_f64().max(0.001);
                            (ticks - prev) as f64 / 100.0 / secs * 100.0
                        }
                        _ => 0.0,
                    };
                    g.insert(pid, (ticks, now));
                    pct
                }
                None => 0.0,
            };
            (mem, (cpu * 10.0).round() / 10.0)
        } else {
            let out = tokio::process::Command::new("ps")
                .args(["-o", "rss=,%cpu=", "-p", &pid.to_string()])
                .output()
                .await;
            let text = out
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            let mut it = text.split_whitespace();
            let rss = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024;
            let cpu = it.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            (rss, cpu)
        }
    }

    pub async fn collect_bot(&self, bot: &Arc<Bot>) -> Vec<Source> {
        let (user, plugins) = {
            let d = bot.desired.read().unwrap();
            (d.user_id.clone(), d.plugins.clone())
        };
        let mut out = vec![
            Source {
                bot_id: bot.id.clone(),
                kind: "core",
                plugin_id: None,
                label: "核心连接".into(),
                memory_bytes: bot.core_memory_bytes(),
                storage_bytes: 0,
                deps_bytes: 0,
                cpu_percent: 0.0,
            },
            Source {
                bot_id: bot.id.clone(),
                kind: "conversations",
                plugin_id: None,
                label: "对话记录".into(),
                memory_bytes: 0,
                storage_bytes: bot.shared.conv.bot_bytes(&bot.id),
                deps_bytes: 0,
                cpu_percent: 0.0,
            },
        ];
        // Plugins in one group process share its pid. The process is measured once and split evenly.
        let pids: Vec<u32> = {
            let procs = bot.procs.read().unwrap();
            plugins
                .iter()
                .map(|s| procs.get(&s.id).map(|p| p.pid).unwrap_or(0))
                .collect()
        };
        let mut sharers: HashMap<u32, u32> = HashMap::new();
        for pid in pids.iter().filter(|p| **p != 0) {
            *sharers.entry(*pid).or_insert(0) += 1;
        }
        let mut measured: HashMap<u32, (u64, f64)> = HashMap::new();
        for (spec, pid) in plugins.into_iter().zip(pids) {
            let dir = bot.shared.cfg.plugin_dir(&user, &bot.id, &spec.name);
            let shared = bot.shared.clone();
            // The bot's plugin directory holds its data. The shared install is reported separately.
            let (storage, deps) =
                tokio::task::spawn_blocking(move || (dir_size(&dir), shared.installer.shared_size(&dir)))
                    .await
                    .unwrap_or((0, 0));
            let (mem, cpu) = match measured.get(&pid) {
                Some(v) => *v,
                None => {
                    let v = self.process(pid).await;
                    measured.insert(pid, v);
                    v
                }
            };
            let n = sharers.get(&pid).copied().unwrap_or(1).max(1);
            let (mem, cpu) = (mem / n as u64, cpu / n as f64);
            out.push(Source {
                bot_id: bot.id.clone(),
                kind: "plugin",
                plugin_id: Some(spec.id.clone()),
                label: if spec.name.is_empty() {
                    spec.id.clone()
                } else {
                    spec.name.clone()
                },
                memory_bytes: mem,
                storage_bytes: storage,
                deps_bytes: deps,
                cpu_percent: cpu,
            });
        }
        out
    }

    /// Takes one sample of every bot, stores it and enforces memory quotas.
    pub async fn sample(&self, node: &Node) {
        let ts = now_ms();
        let bots = node.all();
        let mut rows = vec![];
        let mut snapshot = HashMap::new();
        let mut cpu = PluginCpuSample {
            percent: 0.0,
            sampled_at: ts,
            plugins: Vec::new(),
        };
        let mut per_user: HashMap<String, (u64, u64, u64)> = HashMap::new();
        let mut largest: HashMap<String, (u64, Arc<Bot>, String)> = HashMap::new();
        for bot in &bots {
            let sources = self.collect_bot(bot).await;
            let (user, bot_name, quota) = {
                let d = bot.desired.read().unwrap();
                (d.user_id.clone(), d.name.clone(), d.quota.memory_mb)
            };
            let mem: u64 = sources.iter().map(|s| s.memory_bytes).sum();
            let sto: u64 = sources.iter().map(|s| s.storage_bytes).sum();
            rows.push((bot.id.clone(), user.clone(), mem, sto));
            let e = per_user.entry(user.clone()).or_insert((0, 0, 0));
            e.0 += mem;
            e.1 += sto;
            e.2 = e.2.max(quota);
            for s in sources.iter().filter(|s| s.cpu_percent > 0.0) {
                if let Some(plugin_id) = &s.plugin_id {
                    cpu.percent += s.cpu_percent;
                    cpu.plugins.push(PluginCpu {
                        user_id: user.clone(),
                        bot_id: bot.id.clone(),
                        bot_name: bot_name.clone(),
                        plugin_id: plugin_id.clone(),
                        label: s.label.clone(),
                        cpu_percent: s.cpu_percent,
                    });
                }
            }
            for s in sources.iter().filter(|s| s.kind == "plugin" && s.memory_bytes > 0) {
                if largest.get(&user).is_none_or(|(m, _, _)| s.memory_bytes > *m) {
                    largest.insert(
                        user.clone(),
                        (s.memory_bytes, bot.clone(), s.plugin_id.clone().unwrap_or_default()),
                    );
                }
            }
            snapshot.insert(bot.id.clone(), sources);
        }
        cpu.plugins.sort_unstable_by(|a, b| {
            b.cpu_percent
                .total_cmp(&a.cpu_percent)
                .then_with(|| a.plugin_id.cmp(&b.plugin_id))
        });
        cpu.plugins.truncate(CPU_PLUGIN_ROWS);
        *self.snapshot.lock().unwrap() = snapshot;
        *self.plugin_cpu.lock().unwrap() = Some(cpu);
        if let Err(e) = node.shared.db.add_samples(ts, &rows) {
            tracing::warn!("store usage samples: {e}");
        }
        *node.shared.storage_by_user.lock().unwrap() = per_user.iter().map(|(u, (_, s, _))| (u.clone(), *s)).collect();

        let mut to_stop = vec![];
        {
            let mut counts = self.over_count.lock().unwrap();
            for (user, (mem, _, quota)) in &per_user {
                if node
                    .shared
                    .cgroups
                    .as_ref()
                    .is_some_and(|cg| cg.memory_max(user).is_some())
                {
                    // Kernel-limited tenants are handled by `enforce_bursts`.
                    counts.remove(user);
                    continue;
                }
                let c = counts.entry(user.clone()).or_insert(0);
                if *mem > quota * MIB {
                    *c += 1;
                } else {
                    *c = 0;
                }
                if *c >= OVER_QUOTA_SAMPLES {
                    *c = 0;
                    if let Some((_, bot, pid)) = largest.get(user) {
                        to_stop.push((bot.clone(), pid.clone()));
                    }
                }
            }
            counts.retain(|u, _| per_user.contains_key(u));
        }
        for (bot, plugin_id) in to_stop {
            tracing::warn!("memory quota exceeded, stopping plugin {plugin_id} of bot {}", bot.id);
            bot.stop_over_quota(&plugin_id).await;
        }
    }

    /// Raises or lowers each kernel-limited tenant's `memory.max` with the node's free memory and stops the
    /// tenant's largest plugin after it stayed above its quota for `BURST_WINDOW`.
    pub async fn enforce_bursts(&self, node: &Node) {
        let Some(cg) = node.shared.cgroups.as_ref() else { return };
        let bots = node.all();
        let mut quotas: HashMap<String, u64> = HashMap::new();
        for bot in &bots {
            let d = bot.desired.read().unwrap();
            let q = quotas.entry(d.user_id.clone()).or_insert(0);
            *q = (*q).max(d.quota.memory_mb);
        }
        let available = cg.node_available().unwrap_or(0);
        let now = Instant::now();
        let mut started = vec![];
        let mut expired = vec![];
        {
            let mut over = self.over_since.lock().unwrap();
            over.retain(|u, _| quotas.contains_key(u));
            for (user, mb) in &quotas {
                if cg.memory_max(user).is_none() {
                    continue;
                }
                let Some(ws) = cg.tenant_working_set(user) else {
                    continue;
                };
                let quota = tenant_limit_bytes(*mb);
                cg.set_memory_max(user, *mb, burst_limit(quota, ws, available, NODE_RESERVE));
                if ws <= quota {
                    over.remove(user);
                    continue;
                }
                match over.get(user) {
                    None => {
                        over.insert(user.clone(), now);
                        started.push(user.clone());
                    }
                    Some(since) if now.duration_since(*since) >= BURST_WINDOW => {
                        over.remove(user);
                        expired.push(user.clone());
                    }
                    Some(_) => {}
                }
            }
        }
        let minutes = BURST_WINDOW.as_secs() / 60;
        for user in started {
            for bot in bots.iter().filter(|b| b.user_id() == user) {
                bot.logs.warn(
                    "core",
                    format!("内存超出配额，正在临时使用节点余量。{minutes} 分钟内回落到配额以内就不会停止插件"),
                );
            }
        }
        for user in expired {
            let mut largest: Option<(u64, Arc<Bot>, String)> = None;
            for bot in bots.iter().filter(|b| b.user_id() == user) {
                for s in self.collect_bot(bot).await {
                    if s.kind == "plugin" && largest.as_ref().is_none_or(|(m, _, _)| s.memory_bytes > *m) {
                        largest = Some((s.memory_bytes, bot.clone(), s.plugin_id.clone().unwrap_or_default()));
                    }
                }
            }
            if let Some((_, bot, plugin_id)) = largest {
                tracing::warn!(
                    "memory burst of {user} lasted {minutes} min, stopping plugin {plugin_id} of bot {}",
                    bot.id
                );
                bot.stop_over_quota(&plugin_id).await;
            }
        }
    }

    /// Sources of one bot from the last sample with a fresh core estimate. Collects once when no sample exists.
    async fn bot_sources(&self, bot: &Arc<Bot>) -> Vec<Source> {
        let cached = self.snapshot.lock().unwrap().get(&bot.id).cloned();
        match cached {
            Some(mut s) => {
                if let Some(core) = s.iter_mut().find(|x| x.kind == "core") {
                    core.memory_bytes = bot.core_memory_bytes();
                }
                s
            }
            None => {
                let s = self.collect_bot(bot).await;
                self.snapshot.lock().unwrap().insert(bot.id.clone(), s.clone());
                s
            }
        }
    }

    /// Whole node usage: totals, per user totals sorted by user id and 24h of node totals per sample round.
    pub async fn summary(&self, node: &Node) -> Value {
        let mut per_user: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for bot in node.all() {
            let sources = self.bot_sources(&bot).await;
            let e = per_user.entry(bot.user_id()).or_insert((0, 0));
            e.0 += sources.iter().map(|s| s.memory_bytes).sum::<u64>();
            e.1 += sources.iter().map(|s| s.storage_bytes).sum::<u64>();
        }
        let history: Vec<Value> = node
            .shared
            .db
            .node_history(now_ms() - 24 * 3600 * 1000)
            .unwrap_or_default()
            .into_iter()
            .map(|(ts, m, s)| json!({"ts": ts, "memory_bytes": m, "storage_bytes": s}))
            .collect();
        let users: Vec<Value> = per_user
            .iter()
            .map(|(u, (m, s))| json!({"user_id": u, "memory_bytes": m, "storage_bytes": s}))
            .collect();
        json!({
            "memory_bytes": per_user.values().map(|v| v.0).sum::<u64>(),
            "storage_bytes": per_user.values().map(|v| v.1).sum::<u64>(),
            "users": users,
            "history": history,
            "plugin_cpu": &*self.plugin_cpu.lock().unwrap(),
        })
    }

    pub async fn user_usage(&self, node: &Node, user_id: &str) -> Value {
        let bots: Vec<Arc<Bot>> = node.all().into_iter().filter(|b| b.user_id() == user_id).collect();
        let mut sources = vec![];
        let (mut qmem, mut qsto) = (0u64, 0u64);
        for bot in &bots {
            {
                let d = bot.desired.read().unwrap();
                qmem = qmem.max(d.quota.memory_mb);
                qsto = qsto.max(d.quota.storage_mb);
            }
            sources.extend(self.bot_sources(bot).await);
        }
        let history: Vec<Value> = node
            .shared
            .db
            .history(user_id, now_ms() - 24 * 3600 * 1000)
            .unwrap_or_default()
            .into_iter()
            .map(|(ts, m, s)| json!({"ts": ts, "memory_bytes": m, "storage_bytes": s}))
            .collect();
        let quota = if bots.is_empty() {
            Quota::default()
        } else {
            Quota {
                memory_mb: qmem,
                storage_mb: qsto,
            }
        };
        json!({
            "quota": quota,
            "memory_bytes": sources.iter().map(|s| s.memory_bytes).sum::<u64>(),
            "storage_bytes": sources.iter().map(|s| s.storage_bytes).sum::<u64>(),
            "sources": sources,
            "history": history,
        })
    }
}

pub fn spawn_sampler(node: Arc<Node>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
        loop {
            tick.tick().await;
            node.usage.sample(&node).await;
        }
    });
}

pub fn spawn_burst_guard(node: Arc<Node>) {
    if node.shared.cgroups.is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BURST_CHECK_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            node.usage.enforce_bursts(&node).await;
        }
    });
}

/// Returns `(total, used)` system memory in bytes.
pub async fn system_memory() -> (u64, u64) {
    if cfg!(target_os = "linux") {
        let text = tokio::fs::read_to_string("/proc/meminfo").await.unwrap_or_default();
        let get = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(k))
                .and_then(|r| r.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                * 1024
        };
        let total = get("MemTotal:");
        (total, total.saturating_sub(get("MemAvailable:")))
    } else {
        let out = tokio::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .await;
        let total = out
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
            .unwrap_or(0);
        let own = tokio::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
            .unwrap_or(0);
        (total, own * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_limit_offers_half_of_the_spare_node_memory() {
        let q = 100 * MIB;
        assert_eq!(burst_limit(q, 50 * MIB, 1000 * MIB, 256 * MIB), 2 * q);
        assert_eq!(burst_limit(q, 50 * MIB, 356 * MIB, 256 * MIB), q);
        assert_eq!(burst_limit(q, 120 * MIB, 296 * MIB, 256 * MIB), 140 * MIB);
        assert_eq!(burst_limit(q, 150 * MIB, 100 * MIB, 256 * MIB), 150 * MIB);
        assert_eq!(burst_limit(q, 10 * MIB, 0, 256 * MIB), q);
    }

    #[test]
    fn proc_parsers() {
        assert_eq!(
            parse_status_rss("Name:\tpython\nVmRSS:\t  3000 kB\n"),
            Some(3000 * 1024)
        );
        let stat = "1234 (python3 (x)) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 1 0 100 0";
        assert_eq!(parse_stat_ticks(stat), Some(300));
    }
}
