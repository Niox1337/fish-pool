use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::bot::Bot;
use crate::node::Node;
use crate::plugin::install::dir_size;
use crate::sign::now_ms;

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
pub const OVER_QUOTA_SAMPLES: u32 = 3;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Source {
    pub bot_id: String,
    pub kind: &'static str,
    pub plugin_id: Option<String>,
    pub label: String,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub cpu_percent: f64,
}

/// Sum of all `Pss:` lines of `/proc/<pid>/smaps_rollup`, in bytes.
pub fn parse_pss(text: &str) -> Option<u64> {
    let mut total = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Pss:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            total = Some(total.unwrap_or(0) + kb * 1024);
        }
    }
    total
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
    cpu_prev: Mutex<HashMap<u32, (u64, Instant)>>,
    over_count: Mutex<HashMap<String, u32>>,
}

impl Usage {
    /// Returns `(memory_bytes, cpu_percent)` of a process.
    pub async fn process(&self, pid: u32) -> (u64, f64) {
        if pid == 0 {
            return (0, 0.0);
        }
        if cfg!(target_os = "linux") {
            let mem = match tokio::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .await
                .ok()
                .and_then(|t| parse_pss(&t))
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
                cpu_percent: 0.0,
            },
            Source {
                bot_id: bot.id.clone(),
                kind: "conversations",
                plugin_id: None,
                label: "对话记录".into(),
                memory_bytes: 0,
                storage_bytes: bot.shared.conv.bot_bytes(&bot.id),
                cpu_percent: 0.0,
            },
        ];
        for spec in plugins {
            let dir = bot.shared.cfg.plugin_dir(&user, &bot.id, &spec.name);
            let storage = tokio::task::spawn_blocking(move || dir_size(&dir)).await.unwrap_or(0);
            let pid = bot.procs.read().unwrap().get(&spec.id).map(|p| p.pid).unwrap_or(0);
            let (mem, cpu) = self.process(pid).await;
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
        let mut per_user: HashMap<String, (u64, u64, u64)> = HashMap::new();
        let mut largest: HashMap<String, (u64, Arc<Bot>, String)> = HashMap::new();
        for bot in &bots {
            let sources = self.collect_bot(bot).await;
            let (user, quota) = {
                let d = bot.desired.read().unwrap();
                (d.user_id.clone(), d.quota.memory_mb)
            };
            let mem: u64 = sources.iter().map(|s| s.memory_bytes).sum();
            let sto: u64 = sources.iter().map(|s| s.storage_bytes).sum();
            rows.push((bot.id.clone(), user.clone(), mem, sto));
            let e = per_user.entry(user.clone()).or_insert((0, 0, 0));
            e.0 += mem;
            e.1 += sto;
            e.2 = e.2.max(quota);
            for s in sources.iter().filter(|s| s.kind == "plugin" && s.memory_bytes > 0) {
                if largest.get(&user).is_none_or(|(m, _, _)| s.memory_bytes > *m) {
                    largest.insert(
                        user.clone(),
                        (s.memory_bytes, bot.clone(), s.plugin_id.clone().unwrap_or_default()),
                    );
                }
            }
            self.snapshot.lock().unwrap().insert(bot.id.clone(), sources);
        }
        {
            let ids: Vec<&String> = bots.iter().map(|b| &b.id).collect();
            self.snapshot.lock().unwrap().retain(|k, _| ids.contains(&k));
        }
        if let Err(e) = node.shared.db.add_samples(ts, &rows) {
            tracing::warn!("store usage samples: {e}");
        }
        *node.shared.storage_by_user.lock().unwrap() = per_user.iter().map(|(u, (_, s, _))| (u.clone(), *s)).collect();

        let mut to_stop = vec![];
        {
            let mut counts = self.over_count.lock().unwrap();
            for (user, (mem, _, quota)) in &per_user {
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
        json!({
            "quota": {"memory_mb": if bots.is_empty() { 256 } else { qmem }, "storage_mb": if bots.is_empty() { 1024 } else { qsto }},
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
    fn proc_parsers() {
        let rollup = "00400000-7fff [rollup]\nRss:  2048 kB\nPss:  1024 kB\nPss_Anon: 512 kB\nPss_File: 512 kB\n";
        assert_eq!(parse_pss(rollup), Some(1024 * 1024));
        assert_eq!(parse_pss("Rss: 1 kB"), None);
        assert_eq!(
            parse_status_rss("Name:\tpython\nVmRSS:\t  3000 kB\n"),
            Some(3000 * 1024)
        );
        let stat = "1234 (python3 (x)) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 1 0 100 0";
        assert_eq!(parse_stat_ticks(stat), Some(300));
    }
}
