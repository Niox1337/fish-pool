use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::Engine;

/// Node configuration read from the environment and command line flags.
#[derive(Clone, Debug)]
pub struct Config {
    pub node_id: String,
    /// Raw HMAC key bytes decoded from `FISH_NODE_SECRET`.
    pub secret: Vec<u8>,
    /// The secret string exactly as configured, accepted as an alternate HMAC key on verify.
    pub secret_text: String,
    pub console_url: Option<String>,
    pub listen: String,
    pub data_dir: PathBuf,
    pub sidecar_python: String,
    pub sidecar_dir: PathBuf,
    pub uv: String,
    /// Dev mode skips cgroups, bwrap and uid switching.
    pub dev: bool,
    /// Local test services require FISH_DEV, independently of --dev.
    pub dev_services: bool,
    /// Unprivileged uid and gid for the plugin fork server when the node runs as root.
    pub plugin_uid: Option<u32>,
    pub plugin_gid: Option<u32>,
    /// A plugin process stops after its bot receives no message for this long and starts again
    /// on the next message. None disables it.
    pub plugin_idle_sleep: Option<Duration>,
    /// Runs light plugins of a bot in one shared process. Set by `FISH_PLUGIN_GROUPS`.
    pub plugin_groups: bool,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn decode_secret(text: &str) -> Vec<u8> {
    match base64::engine::general_purpose::STANDARD.decode(text.trim()) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => text.as_bytes().to_vec(),
    }
}

/// Default for `FISH_PLUGIN_IDLE_SECS`.
pub const DEFAULT_PLUGIN_IDLE_SECS: u64 = 1800;

/// Parses `FISH_PLUGIN_IDLE_SECS`. Missing or invalid values use the default, 0 disables.
fn idle_sleep(value: Option<&str>) -> Option<Duration> {
    let secs = value
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PLUGIN_IDLE_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn dev_services_enabled(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}

fn flag(value: Option<&str>) -> bool {
    matches!(value.map(str::to_ascii_lowercase).as_deref(), Some("1" | "true" | "yes" | "on"))
}

impl Config {
    pub fn from_env(args: &[String]) -> Result<Self> {
        let dev_services = dev_services_enabled(env("FISH_DEV").as_deref());
        let dev = args.iter().any(|a| a == "--dev") || dev_services;
        let node_id = env("FISH_NODE_ID").unwrap_or_else(|| "node-dev".to_string());
        let secret_text = match env("FISH_NODE_SECRET") {
            Some(s) => s,
            None if dev => "ZGV2LXNlY3JldC1kZXYtc2VjcmV0LWRldi1zZWNyZXQ=".to_string(),
            None => return Err(anyhow!("FISH_NODE_SECRET is required outside dev mode")),
        };
        let data_dir = PathBuf::from(env("FISH_DATA_DIR").unwrap_or_else(|| "data".to_string()));
        std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
        let data_dir = data_dir.canonicalize()?;
        let sidecar_dir = PathBuf::from(env("FISH_SIDECAR_DIR").unwrap_or_else(|| "../sidecar".to_string()));
        let sidecar_dir = sidecar_dir.canonicalize().unwrap_or(sidecar_dir);
        let parse_id = |name: &str| env(name).and_then(|v| v.parse::<u32>().ok());
        Ok(Config {
            node_id,
            secret: decode_secret(&secret_text),
            secret_text,
            console_url: env("FISH_CONSOLE_URL").map(|u| u.trim_end_matches('/').to_string()),
            listen: env("FISH_LISTEN").unwrap_or_else(|| "0.0.0.0:8700".to_string()),
            data_dir,
            sidecar_python: env("FISH_SIDECAR_PYTHON").unwrap_or_else(|| "python3".to_string()),
            sidecar_dir,
            uv: env("FISH_UV").unwrap_or_else(|| "uv".to_string()),
            dev,
            dev_services,
            plugin_uid: if dev { None } else { parse_id("FISH_PLUGIN_UID") },
            plugin_gid: if dev {
                None
            } else {
                parse_id("FISH_PLUGIN_GID").or(parse_id("FISH_PLUGIN_UID"))
            },
            plugin_idle_sleep: idle_sleep(env("FISH_PLUGIN_IDLE_SECS").as_deref()),
            plugin_groups: flag(env("FISH_PLUGIN_GROUPS").as_deref()),
        })
    }

    pub fn run_dir(&self) -> PathBuf {
        self.data_dir.join("run")
    }

    pub fn tenants_dir(&self) -> PathBuf {
        self.data_dir.join("tenants")
    }

    /// Plugin code and venvs shared by every bot that installed the same archive.
    pub fn shared_dir(&self) -> PathBuf {
        self.data_dir.join("shared")
    }

    pub fn shared_plugins_dir(&self) -> PathBuf {
        self.shared_dir().join("plugins")
    }

    pub fn bot_dir(&self, user_id: &str, bot_id: &str) -> PathBuf {
        self.tenants_dir().join(safe_name(user_id)).join(safe_name(bot_id))
    }

    pub fn plugin_dir(&self, user_id: &str, bot_id: &str, plugin_name: &str) -> PathBuf {
        self.bot_dir(user_id, bot_id)
            .join("plugins")
            .join(safe_name(plugin_name))
    }
}

/// Reduces a string to characters that are safe as a single path component.
pub fn safe_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_services_require_explicit_fish_dev() {
        assert!(!dev_services_enabled(None));
        assert!(!dev_services_enabled(Some("0")));
        assert!(!dev_services_enabled(Some("false")));
        assert!(dev_services_enabled(Some("1")));
        assert!(dev_services_enabled(Some("true")));
    }

    #[test]
    fn flag_parsing() {
        assert!(!flag(None));
        assert!(flag(Some("1")) && flag(Some("TRUE")) && flag(Some("on")));
        assert!(!flag(Some("0")) && !flag(Some("off")));
    }

    #[test]
    fn idle_sleep_parsing() {
        assert_eq!(idle_sleep(None), Some(Duration::from_secs(DEFAULT_PLUGIN_IDLE_SECS)));
        assert_eq!(idle_sleep(Some("90")), Some(Duration::from_secs(90)));
        assert_eq!(idle_sleep(Some("0")), None);
        assert_eq!(idle_sleep(Some("x")), Some(Duration::from_secs(DEFAULT_PLUGIN_IDLE_SECS)));
    }

    #[test]
    fn safe_name_blocks_traversal() {
        assert_eq!(safe_name("../etc"), "___etc");
        assert_eq!(safe_name("astrbot_plugin-x"), "astrbot_plugin-x");
        assert_eq!(safe_name(""), "_");
    }
}
