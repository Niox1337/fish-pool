use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::{safe_name, Config};
use crate::plugin::SidecarCommand;

fn running_as_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|s| s.trim() == "0")
}

// shortcut: one sandbox wraps the whole fork server so every plugin can write to all tenant dirs,
// upgrade by restricting each forked child to its own data dir inside the sidecar.
/// Bubblewrap arguments: read only root, private /tmp, node data hidden except tenants and run dirs,
/// shared plugin code read only.
pub fn bwrap_args(cfg: &Config) -> Vec<String> {
    let data = cfg.data_dir.display().to_string();
    let tenants = cfg.tenants_dir().display().to_string();
    let run = cfg.run_dir().display().to_string();
    let shared = cfg.shared_dir().display().to_string();
    let mut a: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        &data,
        "--ro-bind",
        &shared,
        &shared,
        "--bind",
        &tenants,
        &tenants,
        "--bind",
        &run,
        &run,
        "--unshare-ipc",
        "--unshare-uts",
        "--die-with-parent",
        "--new-session",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push("--".into());
    a
}

fn bwrap_works(cfg: &Config) -> bool {
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(bwrap_args(cfg))
        .arg("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let Some(g) = cfg.plugin_gid {
            cmd.gid(g);
        }
        if let Some(u) = cfg.plugin_uid {
            cmd.uid(u);
        }
    }
    cmd.status().is_ok_and(|s| s.success())
}

/// Builds the fork server command. Uses bwrap when it works, otherwise runs python directly.
pub fn sidecar_command(cfg: &Config) -> SidecarCommand {
    let own = format!(
        "{}:{}",
        cfg.sidecar_dir.display(),
        cfg.sidecar_dir.join("src").display()
    );
    let pythonpath = match std::env::var("PYTHONPATH") {
        Ok(p) if !p.is_empty() => format!("{own}:{p}"),
        _ => own,
    };
    let envs = vec![
        ("PYTHONPATH".to_string(), pythonpath),
        ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
        ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
        ("FISH_NODE_SECRET".to_string(), String::new()),
        ("MALLOC_ARENA_MAX".to_string(), "2".to_string()),
    ];
    let as_root = running_as_root();
    let (uid, gid) = if as_root {
        (cfg.plugin_uid, cfg.plugin_gid)
    } else {
        (None, None)
    };
    let base_args = vec!["-m".to_string(), "fish_sidecar.server".to_string()];
    if !cfg.dev && cfg!(target_os = "linux") {
        let probe_cfg = Config {
            plugin_uid: uid,
            plugin_gid: gid,
            ..cfg.clone()
        };
        if bwrap_works(&probe_cfg) {
            tracing::info!("plugin isolation: bwrap enabled");
            let mut args = bwrap_args(cfg);
            args.push(cfg.sidecar_python.clone());
            args.extend(base_args);
            return SidecarCommand {
                program: "bwrap".into(),
                args,
                envs,
                cwd: Some(cfg.sidecar_dir.clone()),
                uid,
                gid,
            };
        }
        tracing::warn!("plugin isolation: bwrap unavailable, running plugins without a sandbox");
    }
    SidecarCommand {
        program: cfg.sidecar_python.clone(),
        args: base_args,
        envs,
        cwd: Some(cfg.sidecar_dir.clone()),
        uid,
        gid,
    }
}

/// cgroup v2 controller for per tenant memory limits.
pub struct Cgroups {
    base: PathBuf,
}

fn write(path: &Path, value: &str) -> std::io::Result<()> {
    std::fs::write(path, value)
}

pub fn tenant_limit_bytes(memory_mb: u64) -> u64 {
    memory_mb.max(32) * 1024 * 1024
}

/// Upper bound of a tenant's `memory.max` as a multiple of its quota.
pub const BURST_FACTOR: u64 = 2;

/// A cgroup interface file holding one number. None for `max`, a missing file or other text.
fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// `memory.current` minus `inactive_file` from `memory.stat` of the group at `dir`.
fn working_set(dir: &Path) -> Option<u64> {
    let current = read_u64(&dir.join("memory.current"))?;
    let stat = std::fs::read_to_string(dir.join("memory.stat")).unwrap_or_default();
    let inactive = stat
        .lines()
        .find_map(|l| l.strip_prefix("inactive_file "))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Some(current.saturating_sub(inactive))
}

impl Cgroups {
    /// Sets up `<own cgroup>/node` as a leaf for existing processes and enables the memory controller
    /// for tenant groups. Returns `None` when cgroup v2 is missing or not writable.
    pub fn init(cfg: &Config) -> Option<Cgroups> {
        if cfg.dev || !cfg!(target_os = "linux") {
            return None;
        }
        let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let rel = own
            .lines()
            .find_map(|l| l.strip_prefix("0::"))?
            .trim()
            .trim_start_matches('/')
            .to_string();
        let base = Path::new("/sys/fs/cgroup").join(rel);
        let controllers = std::fs::read_to_string(base.join("cgroup.controllers")).ok()?;
        if !controllers.split_whitespace().any(|c| c == "memory") {
            tracing::warn!("cgroup: memory controller unavailable at {}", base.display());
            return None;
        }
        let leaf = base.join("node");
        let setup = || -> std::io::Result<()> {
            std::fs::create_dir_all(&leaf)?;
            let procs = std::fs::read_to_string(base.join("cgroup.procs"))?;
            for pid in procs.split_whitespace() {
                let _ = write(&leaf.join("cgroup.procs"), pid);
            }
            write(&base.join("cgroup.subtree_control"), "+memory")
        };
        match setup() {
            Ok(()) => {
                tracing::info!("cgroup: tenant memory limits enabled under {}", base.display());
                Some(Cgroups { base })
            }
            Err(e) => {
                tracing::warn!("cgroup: not writable ({e}), tenant memory limits disabled");
                None
            }
        }
    }

    /// The node's own cgroup directory, parent of the tenant groups.
    pub fn base(&self) -> &Path {
        &self.base
    }

    fn tenant(&self, user_id: &str) -> PathBuf {
        self.base.join(format!("t-{}", safe_name(user_id)))
    }

    /// Creates the tenant group and keeps its `memory.max` between the quota and `BURST_FACTOR` times the quota.
    /// A burst allowance granted earlier survives a desired state update when it still fits the quota.
    pub fn set_limit(&self, user_id: &str, memory_mb: u64) {
        let dir = self.tenant(user_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("cgroup mkdir {}: {e}", dir.display());
            return;
        }
        let quota = tenant_limit_bytes(memory_mb);
        let bytes = self.memory_max(user_id).map_or(quota, |cur| cur.clamp(quota, quota * BURST_FACTOR));
        if let Err(e) = write(&dir.join("memory.max"), &bytes.to_string()) {
            tracing::warn!("cgroup memory.max: {e}");
        }
        let _ = write(&dir.join("memory.swap.max"), "0");
    }

    /// The tenant's finite `memory.max` in bytes. None when the group is missing or unlimited.
    pub fn memory_max(&self, user_id: &str) -> Option<u64> {
        read_u64(&self.tenant(user_id).join("memory.max"))
    }

    /// Writes the tenant's `memory.max`, kept between the quota and `BURST_FACTOR` times the quota.
    pub fn set_memory_max(&self, user_id: &str, memory_mb: u64, bytes: u64) {
        let quota = tenant_limit_bytes(memory_mb);
        let bytes = bytes.clamp(quota, quota * BURST_FACTOR);
        if self.memory_max(user_id) == Some(bytes) {
            return;
        }
        if let Err(e) = write(&self.tenant(user_id).join("memory.max"), &bytes.to_string()) {
            tracing::warn!("cgroup memory.max for {user_id}: {e}");
        }
    }

    /// Memory the kernel charges to the tenant minus reclaimable inactive file cache, in bytes.
    pub fn tenant_working_set(&self, user_id: &str) -> Option<u64> {
        working_set(&self.tenant(user_id))
    }

    /// Bytes the node's own group can still use before its limit. Uses `MemAvailable` when the group has no limit.
    pub fn node_available(&self) -> Option<u64> {
        match read_u64(&self.base.join("memory.max")) {
            Some(limit) => Some(limit.saturating_sub(working_set(&self.base)?)),
            None => {
                let text = std::fs::read_to_string("/proc/meminfo").ok()?;
                text.lines()
                    .find_map(|l| l.strip_prefix("MemAvailable:"))
                    .and_then(|r| r.split_whitespace().next())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
            }
        }
    }

    pub fn add_pid(&self, user_id: &str, pid: u32) -> std::io::Result<()> {
        let dir = self.tenant(user_id);
        if std::fs::read_to_string(dir.join("memory.max"))?.trim() == "max" {
            return Err(std::io::Error::other("tenant memory.max is unlimited"));
        }
        write(&dir.join("cgroup.procs"), &pid.to_string())
    }

    /// Creates the memory group for plugin template processes and returns its directory.
    pub fn templates_group(&self, memory_mb: u64) -> Option<PathBuf> {
        let dir = self.base.join("templates");
        let setup = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            write(&dir.join("memory.max"), &(memory_mb * 1024 * 1024).to_string())?;
            let _ = write(&dir.join("memory.swap.max"), "0");
            Ok(())
        };
        match setup() {
            Ok(()) => Some(dir),
            Err(e) => {
                tracing::warn!("cgroup templates group: {e}");
                None
            }
        }
    }

    pub fn remove_tenant(&self, user_id: &str) {
        let _ = std::fs::remove_dir(self.tenant(user_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_limit_keeps_a_burst_within_the_quota_range() {
        let root = tempfile::tempdir().unwrap();
        let groups = Cgroups { base: root.path().to_path_buf() };
        let mib = 1024 * 1024;
        assert_eq!(groups.memory_max("u1"), None);
        groups.set_limit("u1", 128);
        assert_eq!(groups.memory_max("u1"), Some(128 * mib));
        groups.set_memory_max("u1", 128, 1000 * mib);
        assert_eq!(groups.memory_max("u1"), Some(256 * mib));
        groups.set_memory_max("u1", 128, 200 * mib);
        groups.set_limit("u1", 128);
        assert_eq!(groups.memory_max("u1"), Some(200 * mib));
        groups.set_limit("u1", 64);
        assert_eq!(groups.memory_max("u1"), Some(128 * mib));
        groups.set_memory_max("u1", 64, 0);
        assert_eq!(groups.memory_max("u1"), Some(64 * mib));
        write(&groups.tenant("u1").join("memory.max"), "max\n").unwrap();
        assert_eq!(groups.memory_max("u1"), None);
    }

    #[test]
    fn working_set_excludes_inactive_file_cache() {
        let root = tempfile::tempdir().unwrap();
        let groups = Cgroups { base: root.path().to_path_buf() };
        let dir = groups.tenant("u1");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("memory.current"), "1000\n").unwrap();
        write(&dir.join("memory.stat"), "anon 600\nactive_file 100\ninactive_file 300\n").unwrap();
        assert_eq!(groups.tenant_working_set("u1"), Some(700));
        write(&root.path().join("memory.max"), "5000\n").unwrap();
        write(&root.path().join("memory.current"), "2000\n").unwrap();
        assert_eq!(groups.node_available(), Some(3000));
    }
}

/// Changes ownership of a directory tree to the plugin uid when configured and running as root.
pub fn chown_tree(cfg: &Config, path: &Path) {
    #[cfg(unix)]
    {
        let (Some(uid), gid) = (cfg.plugin_uid, cfg.plugin_gid) else {
            return;
        };
        let mut stack = vec![path.to_path_buf()];
        while let Some(p) = stack.pop() {
            let _ = std::os::unix::fs::lchown(&p, Some(uid), gid);
            if p.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    stack.extend(rd.flatten().map(|e| e.path()));
                }
            }
        }
    }
}
