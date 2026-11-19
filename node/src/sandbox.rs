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
/// Bubblewrap arguments: read only root, private /tmp, node data hidden except tenants and run dirs.
pub fn bwrap_args(cfg: &Config) -> Vec<String> {
    let data = cfg.data_dir.display().to_string();
    let tenants = cfg.tenants_dir().display().to_string();
    let run = cfg.run_dir().display().to_string();
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

    fn tenant(&self, user_id: &str) -> PathBuf {
        self.base.join(format!("t-{}", safe_name(user_id)))
    }

    pub fn set_limit(&self, user_id: &str, memory_mb: u64) {
        let dir = self.tenant(user_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("cgroup mkdir {}: {e}", dir.display());
            return;
        }
        let bytes = memory_mb.max(32) * 1024 * 1024;
        if let Err(e) = write(&dir.join("memory.max"), &bytes.to_string()) {
            tracing::warn!("cgroup memory.max: {e}");
        }
        let _ = write(&dir.join("memory.swap.max"), "0");
    }

    pub fn add_pid(&self, user_id: &str, pid: u32) {
        let dir = self.tenant(user_id);
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = write(&dir.join("cgroup.procs"), &pid.to_string()) {
            tracing::warn!("cgroup add pid {pid}: {e}");
        }
    }

    pub fn remove_tenant(&self, user_id: &str) {
        let _ = std::fs::remove_dir(self.tenant(user_id));
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
