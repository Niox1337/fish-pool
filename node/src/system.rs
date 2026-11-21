//! Host, container, process and tenant metrics for the admin system view.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::safe_name;
use crate::node::{Node, VERSION};
use crate::plugin::install::dir_size;
use crate::sign::now_ms;

pub const SYSTEM_INTERVAL: Duration = Duration::from_secs(15);
const HISTORY_POINTS: usize = 240;
const UV_CACHE_EVERY: u32 = 20;
const LOG_WINDOW_MS: i64 = 3600 * 1000;
const LOG_BOT_ROWS: usize = 10;

// ---------- parsers ----------

/// `(total, idle)` jiffies of the aggregate `cpu` line of `/proc/stat`. Idle includes iowait.
pub fn parse_cpu_line(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let total = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total, idle))
}

/// Boot time in unix seconds from the `btime` line of `/proc/stat`.
pub fn parse_btime(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())
}

/// Lines of the form `Key: value [kB]` or `key value`, values in their raw unit except `kB`, which is
/// converted to bytes.
pub fn parse_kv(text: &str) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(n) = v.parse::<u64>() else { continue };
        let n = if it.next() == Some("kB") { n * 1024 } else { n };
        out.insert(k.trim_end_matches(':').to_string(), n);
    }
    out
}

/// A cgroup limit file value. `max` and unparsable values are `None`.
pub fn parse_limit(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// CPU limit in cores from `cpu.max` (`<quota> <period>` or `max <period>`).
pub fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut it = text.split_whitespace();
    let quota: f64 = it.next()?.parse().ok()?;
    let period: f64 = it.next()?.parse().ok()?;
    (period > 0.0).then(|| quota / period)
}

#[derive(Debug, PartialEq)]
pub struct ProcStat {
    pub name: String,
    pub ppid: u32,
    pub ticks: u64,
    pub threads: u32,
    pub start_ticks: u64,
}

/// Fields of `/proc/<pid>/stat` used here.
pub fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_string();
    let f: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    Some(ProcStat {
        name,
        ppid: f.get(1)?.parse().ok()?,
        ticks: f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?,
        threads: f.get(17)?.parse().ok()?,
        start_ticks: f.get(19)?.parse().ok()?,
    })
}

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub struct ProcMem {
    pub rss: u64,
    pub pss: Option<u64>,
    pub uss: Option<u64>,
    pub swap: Option<u64>,
}

/// Memory of a process from `/proc/<pid>/smaps_rollup`. USS is private clean plus private dirty.
pub fn parse_rollup(text: &str) -> Option<ProcMem> {
    let kv = parse_kv(text);
    let rss = *kv.get("Rss")?;
    let uss = match (kv.get("Private_Clean"), kv.get("Private_Dirty")) {
        (Some(c), Some(d)) => Some(c + d),
        _ => None,
    };
    Some(ProcMem {
        rss,
        pss: kv.get("Pss").copied(),
        uss,
        swap: kv.get("Swap").copied(),
    })
}

// ---------- snapshot types ----------

#[derive(Serialize, Clone, Default, Debug)]
pub struct Host {
    pub cpu_count: usize,
    /// 100 is one logical CPU.
    pub cpu_percent: Option<f64>,
    pub load: Option<[f64; 3]>,
    pub uptime_s: Option<u64>,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub swap_free_bytes: Option<u64>,
    pub kernel: Option<String>,
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct Container {
    pub memory_current_bytes: u64,
    pub memory_max_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub memory_anon_bytes: Option<u64>,
    pub memory_file_bytes: Option<u64>,
    pub oom_kills: u64,
    /// 100 is one logical CPU.
    pub cpu_percent: Option<f64>,
    pub cpu_limit_cores: Option<f64>,
    pub pids_current: Option<u64>,
    pub pids_max: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// `node`, `sidecar`, `plugin`, `task` (other children of the node) or `other`.
    pub role: &'static str,
    pub name: String,
    pub rss_bytes: u64,
    pub pss_bytes: Option<u64>,
    pub uss_bytes: Option<u64>,
    pub swap_bytes: Option<u64>,
    /// 100 is one logical CPU.
    pub cpu_percent: Option<f64>,
    pub threads: Option<u32>,
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginRef>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PluginRef {
    pub user_id: String,
    pub bot_id: String,
    pub bot_name: String,
    pub plugin_id: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Point {
    pub ts: i64,
    pub host_cpu: Option<f64>,
    pub container_cpu: Option<f64>,
    pub host_memory_used_bytes: Option<u64>,
    pub container_memory_bytes: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Snapshot {
    pub sampled_at: i64,
    pub host: Host,
    pub container: Option<Container>,
    pub processes: Vec<Proc>,
}

struct Raw {
    at: Instant,
    host_cpu: Option<(u64, u64)>,
    cgroup_usec: Option<u64>,
    procs: HashMap<u32, (u64, u64)>,
}

#[derive(Default)]
struct State {
    prev: Option<Raw>,
    latest: Option<Snapshot>,
    history: VecDeque<Point>,
    ticks: u32,
    uv_cache_bytes: Option<u64>,
}

/// Who a process is, as known by the node.
pub struct Owners {
    pub own_pid: u32,
    pub sidecar_pid: u32,
    pub plugins: HashMap<u32, PluginRef>,
}

/// Role of `pid` from its ancestry: plugin subtree, fork server subtree, the node itself, other node children.
pub fn classify(pid: u32, parents: &HashMap<u32, u32>, owners: &Owners) -> (&'static str, Option<PluginRef>) {
    let mut cur = pid;
    for _ in 0..64 {
        if let Some(p) = owners.plugins.get(&cur) {
            return ("plugin", Some(p.clone()));
        }
        if owners.sidecar_pid != 0 && cur == owners.sidecar_pid {
            return ("sidecar", None);
        }
        if cur == owners.own_pid {
            return (if cur == pid { "node" } else { "task" }, None);
        }
        match parents.get(&cur) {
            Some(&p) if p != 0 && p != cur => cur = p,
            _ => break,
        }
    }
    ("other", None)
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn clk_tck() -> f64 {
    #[cfg(unix)]
    {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 {
            return v as f64;
        }
    }
    100.0
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn loadavg() -> Option<[f64; 3]> {
    #[cfg(unix)]
    {
        let mut l = [0f64; 3];
        let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
        if n == 3 {
            return Some(l.map(|v| (v * 100.0).round() / 100.0));
        }
    }
    None
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Disk {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub avail_bytes: u64,
    pub inodes_total: u64,
    pub inodes_free: u64,
}

/// Filesystem usage of the volume holding `path`.
pub fn disk(path: &Path) -> Option<Disk> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let f = s.f_frsize as u64;
        Some(Disk {
            path: path.display().to_string(),
            total_bytes: s.f_blocks as u64 * f,
            free_bytes: s.f_bfree as u64 * f,
            avail_bytes: s.f_bavail as u64 * f,
            inodes_total: s.f_files as u64,
            inodes_free: s.f_ffree as u64,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn file_size(path: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|s| std::fs::metadata(format!("{}{s}", path.display())).ok())
        .map(|m| m.len())
        .sum()
}

// ---------- reading ----------

struct ProcRead {
    pid: u32,
    stat: ProcStat,
    mem: ProcMem,
    /// CPU reported directly by the source, used instead of a tick delta.
    cpu: Option<f64>,
}

struct Read {
    raw: Raw,
    btime: Option<u64>,
    host: Host,
    container: Option<Container>,
    procs: Vec<ProcRead>,
}

fn read_linux(cgroup: &Path) -> Read {
    let stat = read("/proc/stat").unwrap_or_default();
    let mem = parse_kv(&read("/proc/meminfo").unwrap_or_default());
    let cpu_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let host = Host {
        cpu_count,
        cpu_percent: None,
        load: loadavg(),
        uptime_s: read("/proc/uptime")
            .and_then(|t| t.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
            .map(|v| v as u64),
        memory_total_bytes: mem.get("MemTotal").copied().unwrap_or(0),
        memory_available_bytes: mem.get("MemAvailable").copied(),
        swap_total_bytes: mem.get("SwapTotal").copied(),
        swap_free_bytes: mem.get("SwapFree").copied(),
        kernel: read("/proc/sys/kernel/osrelease").map(|s| s.trim().to_string()),
    };
    let cpu_stat = parse_kv(&read(cgroup.join("cpu.stat")).unwrap_or_default());
    let container = read(cgroup.join("memory.current"))
        .and_then(|t| t.trim().parse::<u64>().ok())
        .map(|current| {
            let ms = parse_kv(&read(cgroup.join("memory.stat")).unwrap_or_default());
            let ev = parse_kv(&read(cgroup.join("memory.events")).unwrap_or_default());
            Container {
                memory_current_bytes: current,
                memory_max_bytes: read(cgroup.join("memory.max")).and_then(|t| parse_limit(&t)),
                memory_peak_bytes: read(cgroup.join("memory.peak")).and_then(|t| parse_limit(&t)),
                memory_anon_bytes: ms.get("anon").copied(),
                memory_file_bytes: ms.get("file").copied(),
                oom_kills: ev.get("oom_kill").copied().unwrap_or(0),
                cpu_percent: None,
                cpu_limit_cores: read(cgroup.join("cpu.max")).and_then(|t| parse_cpu_max(&t)),
                pids_current: read(cgroup.join("pids.current")).and_then(|t| parse_limit(&t)),
                pids_max: read(cgroup.join("pids.max")).and_then(|t| parse_limit(&t)),
            }
        });
    let mut procs = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ps) = read(format!("/proc/{pid}/stat")).and_then(|t| parse_proc_stat(&t)) else {
            continue;
        };
        let mem = read(format!("/proc/{pid}/smaps_rollup"))
            .and_then(|t| parse_rollup(&t))
            .or_else(|| {
                read(format!("/proc/{pid}/status"))
                    .and_then(|t| crate::usage::parse_status_rss(&t))
                    .map(|rss| ProcMem {
                        rss,
                        ..Default::default()
                    })
            })
            .unwrap_or_default();
        procs.push(ProcRead {
            pid,
            stat: ps,
            mem,
            cpu: None,
        });
    }
    let raw = Raw {
        at: Instant::now(),
        host_cpu: parse_cpu_line(&stat),
        cgroup_usec: cpu_stat.get("usage_usec").copied(),
        procs: procs
            .iter()
            .map(|p| (p.pid, (p.stat.start_ticks, p.stat.ticks)))
            .collect(),
    };
    Read {
        raw,
        btime: parse_btime(&stat),
        host,
        container,
        procs,
    }
}

/// Process tree below `own` from `ps`, for hosts without `/proc`.
fn read_ps(own: u32) -> Read {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss=,%cpu=,comm="])
        .output();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(rss), Some(cpu)) = (it.next(), it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(rss), Ok(cpu)) = (
            pid.parse::<u32>(),
            ppid.parse::<u32>(),
            rss.parse::<u64>(),
            cpu.parse::<f64>(),
        ) else {
            continue;
        };
        let name = it.collect::<Vec<_>>().join(" ");
        let name = name.rsplit('/').next().unwrap_or("").to_string();
        rows.push((pid, ppid, rss * 1024, cpu, name));
    }
    let parents: HashMap<u32, u32> = rows.iter().map(|r| (r.0, r.1)).collect();
    let below = |mut p: u32| {
        for _ in 0..64 {
            if p == own {
                return true;
            }
            match parents.get(&p) {
                Some(&n) if n != p && n != 0 => p = n,
                _ => return false,
            }
        }
        false
    };
    let procs = rows
        .into_iter()
        .filter(|r| below(r.0))
        .map(|(pid, ppid, rss, cpu, name)| ProcRead {
            pid,
            stat: ProcStat {
                name,
                ppid,
                ticks: 0,
                threads: 0,
                start_ticks: 0,
            },
            mem: ProcMem {
                rss,
                ..Default::default()
            },
            cpu: Some(round1(cpu)),
        })
        .collect();
    let total = {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output();
        out.ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let host = Host {
        cpu_count: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        load: loadavg(),
        memory_total_bytes: total,
        ..Default::default()
    };
    let raw = Raw {
        at: Instant::now(),
        host_cpu: None,
        cgroup_usec: None,
        procs: HashMap::new(),
    };
    Read {
        raw,
        btime: None,
        host,
        container: None,
        procs,
    }
}

/// Builds a snapshot from a fresh reading and the previous raw counters.
fn build(read: Read, prev: Option<&Raw>, owners: &Owners) -> (Snapshot, Raw) {
    let Read {
        raw,
        btime,
        mut host,
        mut container,
        procs,
    } = read;
    let secs = prev
        .map(|p| raw.at.duration_since(p.at).as_secs_f64())
        .filter(|s| *s > 0.5);
    if let (Some(p), Some((t1, i1))) = (prev.and_then(|p| p.host_cpu), raw.host_cpu) {
        let (t0, i0) = p;
        if t1 > t0 {
            let busy = (t1 - t0).saturating_sub(i1.saturating_sub(i0)) as f64 / (t1 - t0) as f64;
            host.cpu_percent = Some(round1(busy * host.cpu_count as f64 * 100.0));
        }
    }
    if let (Some(c), Some(s), Some(u0), Some(u1)) = (
        container.as_mut(),
        secs,
        prev.and_then(|p| p.cgroup_usec),
        raw.cgroup_usec,
    ) {
        if u1 >= u0 {
            c.cpu_percent = Some(round1((u1 - u0) as f64 / (s * 1e6) * 100.0));
        }
    }
    let tck = clk_tck();
    let parents: HashMap<u32, u32> = procs.iter().map(|p| (p.pid, p.stat.ppid)).collect();
    let mut out: Vec<Proc> = procs
        .into_iter()
        .map(
            |ProcRead {
                 pid,
                 stat: ps,
                 mem,
                 cpu,
             }| {
                let (role, plugin) = classify(pid, &parents, owners);
                let cpu = cpu.or_else(|| match (secs, prev.and_then(|p| p.procs.get(&pid))) {
                    (Some(s), Some(&(start, t0))) if start == ps.start_ticks && ps.ticks >= t0 => {
                        Some(round1((ps.ticks - t0) as f64 / tck / s * 100.0))
                    }
                    _ => None,
                });
                Proc {
                    pid,
                    ppid: ps.ppid,
                    role,
                    name: ps.name,
                    rss_bytes: mem.rss,
                    pss_bytes: mem.pss,
                    uss_bytes: mem.uss,
                    swap_bytes: mem.swap,
                    cpu_percent: cpu,
                    threads: (ps.threads > 0).then_some(ps.threads),
                    started_at: btime
                        .filter(|_| ps.start_ticks > 0)
                        .map(|b| (b as f64 * 1000.0 + ps.start_ticks as f64 / tck * 1000.0) as i64),
                    plugin,
                }
            },
        )
        .collect();
    out.sort_by(|a, b| {
        b.pss_bytes
            .unwrap_or(b.rss_bytes)
            .cmp(&a.pss_bytes.unwrap_or(a.rss_bytes))
            .then(a.pid.cmp(&b.pid))
    });
    (
        Snapshot {
            sampled_at: now_ms(),
            host,
            container,
            processes: out,
        },
        raw,
    )
}

// ---------- monitor ----------

pub struct System {
    pub started_at: i64,
    state: Mutex<State>,
}

impl Default for System {
    fn default() -> Self {
        System {
            started_at: now_ms(),
            state: Mutex::default(),
        }
    }
}

fn owners(node: &Node) -> Owners {
    let mut plugins = HashMap::new();
    for bot in node.all() {
        let (user_id, bot_name, specs) = {
            let d = bot.desired.read().unwrap();
            (d.user_id.clone(), d.name.clone(), d.plugins.clone())
        };
        for (plugin_id, proc) in bot.procs.read().unwrap().iter() {
            if proc.pid == 0 {
                continue;
            }
            let label = specs
                .iter()
                .find(|s| &s.id == plugin_id)
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty());
            plugins.insert(
                proc.pid,
                PluginRef {
                    user_id: user_id.clone(),
                    bot_id: bot.id.clone(),
                    bot_name: bot_name.clone(),
                    plugin_id: plugin_id.clone(),
                    label: label.unwrap_or_else(|| proc.plugin_name.clone()),
                },
            );
        }
    }
    Owners {
        own_pid: std::process::id(),
        sidecar_pid: node.shared.sidecar.pid(),
        plugins,
    }
}

/// Root of the node's own cgroup v2 hierarchy as seen inside its namespace.
fn cgroup_root(node: &Node) -> PathBuf {
    node.shared
        .cgroups
        .as_ref()
        .map(|c| c.base().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"))
}

impl System {
    /// Takes one sample and appends a history point.
    pub async fn sample(&self, node: &Node) {
        let owners = owners(node);
        let cgroup = cgroup_root(node);
        let linux = cfg!(target_os = "linux");
        let (refresh_uv, uv_dir) = {
            let mut s = self.state.lock().unwrap();
            s.ticks = s.ticks.wrapping_add(1);
            (s.ticks % UV_CACHE_EVERY == 1, node.shared.cfg.data_dir.join("uv-cache"))
        };
        let reading = tokio::task::spawn_blocking(move || {
            let r = if linux {
                read_linux(&cgroup)
            } else {
                read_ps(owners.own_pid)
            };
            let uv = refresh_uv.then(|| dir_size(&uv_dir));
            (r, owners, uv)
        })
        .await;
        let Ok((reading, owners, uv)) = reading else { return };
        let mut s = self.state.lock().unwrap();
        let (snap, raw) = build(reading, s.prev.as_ref(), &owners);
        s.prev = Some(raw);
        if uv.is_some() {
            s.uv_cache_bytes = uv;
        }
        s.history.push_back(Point {
            ts: snap.sampled_at,
            host_cpu: snap.host.cpu_percent,
            container_cpu: snap.container.as_ref().and_then(|c| c.cpu_percent),
            host_memory_used_bytes: snap
                .host
                .memory_available_bytes
                .map(|a| snap.host.memory_total_bytes.saturating_sub(a)),
            container_memory_bytes: snap.container.as_ref().map(|c| c.memory_current_bytes),
        });
        while s.history.len() > HISTORY_POINTS {
            s.history.pop_front();
        }
        s.latest = Some(snap);
    }

    /// The full system report served on `GET /v1/system`.
    pub async fn report(&self, node: &Node) -> Value {
        if self.state.lock().unwrap().latest.is_none() {
            self.sample(node).await;
        }
        let (snap, history, uv) = {
            let s = self.state.lock().unwrap();
            (
                s.latest.clone(),
                s.history.iter().cloned().collect::<Vec<_>>(),
                s.uv_cache_bytes,
            )
        };
        let cfg = &node.shared.cfg;
        let data_dir = cfg.data_dir.clone();
        let (disk, node_db, conv_db) = tokio::task::spawn_blocking(move || {
            (
                disk(&data_dir),
                file_size(&data_dir.join("node.db")),
                file_size(&data_dir.join("conversations.db")),
            )
        })
        .await
        .unwrap_or((None, 0, 0));
        let tenants_bytes: u64 = node.shared.storage_by_user.lock().unwrap().values().sum();
        let bots = node.all();
        let users: Vec<String> = {
            let mut u: Vec<String> = bots.iter().map(|b| b.user_id()).collect();
            u.sort();
            u.dedup();
            u
        };
        let tenants = match &node.shared.cgroups {
            Some(cg) => tenant_rows(cg.base(), &users),
            None => Vec::new(),
        };
        let live = live_status(&bots);
        let logs = log_stats(&bots, now_ms() - LOG_WINDOW_MS);
        json!({
            "node_id": cfg.node_id,
            "version": VERSION,
            "started_at": self.started_at,
            "interval_s": SYSTEM_INTERVAL.as_secs(),
            "sampled_at": snap.as_ref().map(|s| s.sampled_at),
            "host": snap.as_ref().map(|s| &s.host),
            "container": snap.as_ref().and_then(|s| s.container.as_ref()),
            "processes": snap.as_ref().map(|s| s.processes.as_slice()).unwrap_or(&[]),
            "history": history,
            "disk": disk,
            "storage": {
                "node_db_bytes": node_db, "conversations_db_bytes": conv_db,
                "tenants_bytes": tenants_bytes, "uv_cache_bytes": uv,
            },
            "cgroups_enabled": node.shared.cgroups.is_some(),
            "tenants": tenants,
            "bots": live.bots,
            "platforms": live.platforms,
            "plugins": live.plugins,
            "logs": logs,
        })
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct TenantRow {
    pub user_id: String,
    pub memory_current_bytes: u64,
    pub memory_max_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub oom_kills: u64,
    pub procs: usize,
}

/// Memory of every tenant cgroup `t-<user>` under `base`. Unknown directories keep their raw name.
pub fn tenant_rows(base: &Path, users: &[String]) -> Vec<TenantRow> {
    let names: HashMap<String, &String> = users.iter().map(|u| (format!("t-{}", safe_name(u)), u)).collect();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("t-") || !entry.path().is_dir() {
            continue;
        }
        let dir = entry.path();
        let Some(current) = read(dir.join("memory.current")).and_then(|t| parse_limit(&t)) else {
            continue;
        };
        let ev = parse_kv(&read(dir.join("memory.events")).unwrap_or_default());
        rows.push(TenantRow {
            user_id: names
                .get(&name)
                .map(|u| u.to_string())
                .unwrap_or_else(|| name[2..].to_string()),
            memory_current_bytes: current,
            memory_max_bytes: read(dir.join("memory.max")).and_then(|t| parse_limit(&t)),
            memory_peak_bytes: read(dir.join("memory.peak")).and_then(|t| parse_limit(&t)),
            oom_kills: ev.get("oom_kill").copied().unwrap_or(0),
            procs: read(dir.join("cgroup.procs"))
                .map(|t| t.split_whitespace().count())
                .unwrap_or(0),
        });
    }
    rows.sort_by(|a, b| {
        b.memory_current_bytes
            .cmp(&a.memory_current_bytes)
            .then_with(|| a.user_id.cmp(&b.user_id))
    });
    rows
}

struct Live {
    bots: Value,
    platforms: Value,
    plugins: Value,
}

fn bump(map: &mut serde_json::Map<String, Value>, key: &str) {
    let n = map.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
    map.insert(key.to_string(), json!(n));
}

/// Live status counts of bots, platforms per kind and plugins as the node sees them.
fn live_status(bots: &[Arc<crate::bot::Bot>]) -> Live {
    let mut b = serde_json::Map::new();
    let mut plugins = serde_json::Map::new();
    let mut kinds: std::collections::BTreeMap<String, serde_json::Map<String, Value>> = Default::default();
    b.insert("total".into(), json!(bots.len()));
    for bot in bots {
        let st = bot.status_json();
        bump(&mut b, st["status"].as_str().unwrap_or("unknown"));
        let kind_of: HashMap<String, String> = bot
            .desired
            .read()
            .unwrap()
            .platforms
            .iter()
            .map(|p| (p.id.clone(), p.kind.clone()))
            .collect();
        for p in st["platforms"].as_array().into_iter().flatten() {
            let id = p["id"].as_str().unwrap_or("");
            let kind = kind_of.get(id).cloned().unwrap_or_else(|| "unknown".into());
            let m = kinds.entry(kind).or_default();
            bump(m, "total");
            bump(m, p["status"].as_str().unwrap_or("unknown"));
        }
        for p in st["plugins"].as_array().into_iter().flatten() {
            bump(&mut plugins, "total");
            bump(&mut plugins, p["status"].as_str().unwrap_or("unknown"));
        }
    }
    let platforms: Vec<Value> = kinds
        .into_iter()
        .map(|(kind, mut m)| {
            m.insert("kind".into(), json!(kind));
            Value::Object(m)
        })
        .collect();
    Live {
        bots: Value::Object(b),
        platforms: json!(platforms),
        plugins: Value::Object(plugins),
    }
}

fn log_stats(bots: &[Arc<crate::bot::Bot>], since: i64) -> Value {
    let (mut errors, mut warns) = (0u64, 0u64);
    let mut rows = Vec::new();
    for bot in bots {
        let s = bot.logs.stats_since(since);
        errors += s.errors;
        warns += s.warns;
        if s.errors + s.warns > 0 {
            let (user_id, bot_name) = {
                let d = bot.desired.read().unwrap();
                (d.user_id.clone(), d.name.clone())
            };
            rows.push(json!({
                "bot_id": bot.id, "bot_name": bot_name, "user_id": user_id,
                "errors": s.errors, "warns": s.warns,
                "last_error_at": s.last_error.as_ref().map(|e| e.0),
                "last_error": s.last_error.map(|e| e.1),
            }));
        }
    }
    rows.sort_by(|a, b| {
        let k = |v: &Value| (v["errors"].as_u64().unwrap_or(0), v["warns"].as_u64().unwrap_or(0));
        k(b).cmp(&k(a))
    });
    rows.truncate(LOG_BOT_ROWS);
    json!({"window_s": LOG_WINDOW_MS / 1000, "errors": errors, "warns": warns, "bots": rows})
}

pub fn spawn_system_sampler(node: Arc<Node>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SYSTEM_INTERVAL);
        loop {
            tick.tick().await;
            node.system.sample(&node).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_and_boot_lines() {
        let stat = "cpu  100 5 50 800 20 0 5 0 0 0\ncpu0 1 2 3 4\nbtime 1700000000\n";
        assert_eq!(parse_cpu_line(stat), Some((980, 820)));
        assert_eq!(parse_btime(stat), Some(1_700_000_000));
        assert_eq!(parse_cpu_line("cpu0 1 2 3 4"), None);
    }

    #[test]
    fn kv_limits_and_cpu_max() {
        let kv = parse_kv("anon 4096\nfile 8192\nRss:   12 kB\nbogus x\n");
        assert_eq!(kv.get("anon"), Some(&4096));
        assert_eq!(kv.get("Rss"), Some(&(12 * 1024)));
        assert!(!kv.contains_key("bogus"));
        assert_eq!(parse_limit("max\n"), None);
        assert_eq!(parse_limit("1677721600\n"), Some(1_677_721_600));
        assert_eq!(parse_cpu_max("max 100000"), None);
        assert_eq!(parse_cpu_max("150000 100000"), Some(1.5));
    }

    #[test]
    fn proc_stat_and_rollup() {
        let stat = "42 (python3 (x)) S 7 42 42 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 3 0 900 0";
        assert_eq!(
            parse_proc_stat(stat),
            Some(ProcStat {
                name: "python3 (x)".into(),
                ppid: 7,
                ticks: 300,
                threads: 3,
                start_ticks: 900
            })
        );
        let rollup = "55 [rollup]\nRss: 2048 kB\nPss: 1024 kB\nShared_Clean: 512 kB\nPrivate_Clean: 128 kB\nPrivate_Dirty: 256 kB\nSwap: 0 kB\n";
        assert_eq!(
            parse_rollup(rollup),
            Some(ProcMem {
                rss: 2048 * 1024,
                pss: Some(1024 * 1024),
                uss: Some(384 * 1024),
                swap: Some(0)
            })
        );
        assert_eq!(parse_rollup("Pss: 1 kB"), None);
    }

    fn plugin(id: &str) -> PluginRef {
        PluginRef {
            user_id: "u".into(),
            bot_id: "b".into(),
            bot_name: "鱼".into(),
            plugin_id: id.into(),
            label: id.into(),
        }
    }

    #[test]
    fn classification_follows_ancestry() {
        let parents: HashMap<u32, u32> = [
            (1, 0),
            (10, 1),
            (20, 10),
            (21, 20),
            (30, 21),
            (31, 30),
            (40, 10),
            (50, 1),
        ]
        .into_iter()
        .collect();
        let owners = Owners {
            own_pid: 10,
            sidecar_pid: 20,
            plugins: [(30, plugin("p"))].into_iter().collect(),
        };
        assert_eq!(classify(10, &parents, &owners).0, "node");
        assert_eq!(classify(20, &parents, &owners).0, "sidecar");
        assert_eq!(classify(21, &parents, &owners).0, "sidecar");
        assert_eq!(classify(30, &parents, &owners), ("plugin", Some(plugin("p"))));
        assert_eq!(classify(31, &parents, &owners), ("plugin", Some(plugin("p"))));
        assert_eq!(classify(40, &parents, &owners).0, "task");
        assert_eq!(classify(50, &parents, &owners).0, "other");
        assert_eq!(classify(1, &parents, &owners).0, "other");
    }

    #[test]
    fn deltas_give_cpu_percent() {
        let owners = Owners {
            own_pid: 10,
            sidecar_pid: 0,
            plugins: HashMap::new(),
        };
        let t0 = Instant::now();
        let prev = Raw {
            at: t0,
            host_cpu: Some((1000, 800)),
            cgroup_usec: Some(1_000_000),
            procs: [(10, (5, 100)), (11, (6, 100))].into_iter().collect(),
        };
        let reading = Read {
            raw: Raw {
                at: t0 + Duration::from_secs(10),
                host_cpu: Some((3000, 2300)),
                cgroup_usec: Some(6_000_000),
                procs: HashMap::new(),
            },
            host: Host {
                cpu_count: 2,
                ..Default::default()
            },
            btime: Some(1_700_000_000),
            container: Some(Container::default()),
            procs: vec![
                ProcRead {
                    pid: 10,
                    stat: ProcStat {
                        name: "fish-node".into(),
                        ppid: 1,
                        ticks: 600,
                        threads: 4,
                        start_ticks: 5,
                    },
                    mem: ProcMem {
                        rss: 10,
                        pss: Some(8),
                        ..Default::default()
                    },
                    cpu: None,
                },
                ProcRead {
                    pid: 11,
                    stat: ProcStat {
                        name: "reused".into(),
                        ppid: 10,
                        ticks: 900,
                        threads: 1,
                        start_ticks: 99,
                    },
                    mem: ProcMem {
                        rss: 50,
                        ..Default::default()
                    },
                    cpu: None,
                },
            ],
        };
        let (snap, _) = build(reading, Some(&prev), &owners);
        assert_eq!(snap.host.cpu_percent, Some(50.0));
        assert_eq!(snap.container.unwrap().cpu_percent, Some(50.0));
        let node = snap.processes.iter().find(|p| p.pid == 10).unwrap();
        assert_eq!(node.role, "node");
        assert_eq!(node.cpu_percent, Some(round1(500.0 / clk_tck() / 10.0 * 100.0)));
        let reused = snap.processes.iter().find(|p| p.pid == 11).unwrap();
        assert_eq!(reused.role, "task");
        assert_eq!(reused.cpu_percent, None);
        assert_eq!(snap.processes[0].pid, 11, "sorted by PSS or RSS, largest first");
    }

    #[test]
    fn tenant_cgroups_are_read_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("t-u1");
        std::fs::create_dir(&t).unwrap();
        std::fs::write(t.join("memory.current"), "1048576\n").unwrap();
        std::fs::write(t.join("memory.max"), "134217728\n").unwrap();
        std::fs::write(t.join("memory.events"), "low 0\noom 2\noom_kill 1\n").unwrap();
        std::fs::write(t.join("cgroup.procs"), "12\n13\n").unwrap();
        let o = dir.path().join("t-gone");
        std::fs::create_dir(&o).unwrap();
        std::fs::write(o.join("memory.current"), "0\n").unwrap();
        std::fs::write(o.join("memory.max"), "max\n").unwrap();
        std::fs::create_dir(dir.path().join("node")).unwrap();
        let rows = tenant_rows(dir.path(), &["u1".to_string()]);
        assert_eq!(
            rows,
            vec![
                TenantRow {
                    user_id: "u1".into(),
                    memory_current_bytes: 1_048_576,
                    memory_max_bytes: Some(134_217_728),
                    memory_peak_bytes: None,
                    oom_kills: 1,
                    procs: 2
                },
                TenantRow {
                    user_id: "gone".into(),
                    memory_current_bytes: 0,
                    memory_max_bytes: None,
                    memory_peak_bytes: None,
                    oom_kills: 0,
                    procs: 0
                },
            ]
        );
    }

    #[test]
    fn disk_of_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path()).unwrap();
        assert!(d.total_bytes > 0 && d.avail_bytes <= d.total_bytes);
    }
}
