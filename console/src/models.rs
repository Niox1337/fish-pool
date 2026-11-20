//! D1 row types, browser facing views and the DesiredBot document pushed to nodes.

use crate::settings::BotSettings;
use crate::util::mask_secret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountQuota {
    pub memory_mb: i64,
    pub storage_mb: i64,
    pub bots: Option<i64>,
    pub plugins: Option<i64>,
}

// The existing integer columns use -1 for an unlimited count. Zero is a real limit.
pub const UNLIMITED_COUNT: i64 = -1;

impl Default for AccountQuota {
    fn default() -> Self {
        Self { memory_mb: 128, storage_mb: 128, bots: None, plugins: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub created_at: i64,
    pub quota_memory_mb: i64,
    pub quota_storage_mb: i64,
    pub quota_bots: i64,
    pub quota_plugins: i64,
    #[serde(default)]
    pub last_login_at: Option<i64>,
    #[serde(default)]
    pub service_until: Option<i64>,
}

impl UserRow {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// True for admins and for users whose service period ends after `now`.
    pub fn service_active_at(&self, now: i64) -> bool {
        self.is_admin() || self.service_until.is_some_and(|t| t > now)
    }

    pub fn view(&self) -> Value {
        self.view_at(crate::util::now_ms())
    }

    pub fn view_at(&self, now: i64) -> Value {
        json!({
            "id": self.id,
            "email": self.email,
            "role": self.role,
            "created_at": self.created_at,
            "quota": AccountQuota {
                memory_mb: self.quota_memory_mb,
                storage_mb: self.quota_storage_mb,
                bots: (self.quota_bots != UNLIMITED_COUNT).then_some(self.quota_bots),
                plugins: (self.quota_plugins != UNLIMITED_COUNT).then_some(self.quota_plugins),
            },
            "service_until": self.service_until,
            "service_active": self.service_active_at(now),
        })
    }
}

/// Per user counters used by the admin views.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UserCounts {
    pub bot_count: i64,
    pub plugin_count: i64,
    pub running_count: i64,
    pub error_count: i64,
}

/// SQL selecting [`UserCounts`] for the user aliased `u`.
pub const USER_COUNTS_SQL: &str = "\
(SELECT COUNT(*) FROM bots b WHERE b.user_id = u.id) AS bot_count, \
(SELECT COUNT(*) FROM plugins p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = u.id) AS plugin_count, \
(SELECT COUNT(*) FROM bots b WHERE b.user_id = u.id AND b.status = 'running') AS running_count, \
((SELECT COUNT(*) FROM bots b WHERE b.user_id = u.id AND b.status IN ('error', 'over_quota')) + \
 (SELECT COUNT(*) FROM plugins p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = u.id AND p.status IN ('error', 'over_quota')) + \
 (SELECT COUNT(*) FROM platforms p JOIN bots b ON b.id = p.bot_id WHERE b.user_id = u.id AND p.enabled = 1 AND p.status = 'error')) AS error_count";

/// AdminUser: the User view plus counters, usage (null when unknown) and last login time.
pub fn admin_user_view(user: &UserRow, counts: &UserCounts, usage: Option<(i64, i64)>) -> Value {
    let mut v = user.view();
    let o = v.as_object_mut().expect("user view is an object");
    o.insert("bots".into(), json!(counts.bot_count));
    o.insert("plugins".into(), json!(counts.plugin_count));
    o.insert("running".into(), json!(counts.running_count));
    o.insert("errors".into(), json!(counts.error_count));
    o.insert("memory_bytes".into(), json!(usage.map(|u| u.0)));
    o.insert("storage_bytes".into(), json!(usage.map(|u| u.1)));
    o.insert("last_login_at".into(), json!(user.last_login_at));
    v
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmKeyRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key_enc: String,
    pub created_at: i64,
}

impl LlmKeyRow {
    pub fn view(&self, plaintext: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "base_url": self.base_url,
            "model": self.model,
            "key_masked": plaintext.map(mask_secret).unwrap_or_else(|| "…".into()),
            "created_at": self.created_at,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeRow {
    pub id: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub node_id: String,
    pub status: String,
    pub status_detail: String,
    pub desired_running: i64,
    pub needs_push: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub settings_json: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformRow {
    pub id: String,
    pub bot_id: String,
    pub kind: String,
    pub display_name: String,
    pub app_id: String,
    pub secret_enc: String,
    pub extra_json: String,
    pub enabled: i64,
    pub status: String,
    pub status_detail: String,
    pub created_at: i64,
}

impl PlatformRow {
    pub fn view(&self, secret_plain: Option<&str>) -> Value {
        let enabled = self.enabled != 0;
        json!({
            "id": self.id,
            "kind": self.kind,
            "display_name": self.display_name,
            "app_id": self.app_id,
            "secret_masked": secret_plain.map(mask_secret).unwrap_or_else(|| "…".into()),
            "enabled": enabled,
            "status": if enabled { self.status.as_str() } else { "disabled" },
            "status_detail": if enabled { self.status_detail.as_str() } else { "" },
            "mode": self.qq_mode(),
        })
    }

    /// Connection mode of a QQ platform, `websocket` unless `extra_json.mode` is `webhook`.
    /// Other platform kinds have no mode.
    pub fn qq_mode(&self) -> Option<&'static str> {
        if self.kind != "qq_official" {
            return None;
        }
        let extra = parse_json_or(Some(&self.extra_json), json!({}));
        Some(if extra.get("mode").and_then(Value::as_str) == Some("webhook") { "webhook" } else { "websocket" })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginRow {
    pub id: String,
    pub bot_id: String,
    pub repo_url: String,
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: i64,
    pub config_json: String,
    pub schema_json: Option<String>,
    pub status: String,
    pub status_detail: String,
    pub install_rev: i64,
    /// Commit the plugin is pinned to. Empty installs the head of its branch.
    #[serde(default)]
    pub commit_sha: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn parse_json_or(s: Option<&str>, fallback: Value) -> Value {
    s.and_then(|s| serde_json::from_str(s).ok()).unwrap_or(fallback)
}

impl PluginRow {
    pub fn config(&self) -> Value {
        parse_json_or(Some(&self.config_json), json!({}))
    }
    pub fn schema(&self) -> Option<Value> {
        self.schema_json.as_deref().and_then(|s| serde_json::from_str(s).ok())
    }
    pub fn config_revision(&self) -> String {
        crate::crypto::sha256_hex(format!("{}\n{}", self.schema_json.as_deref().unwrap_or_default(), self.config_json).as_bytes())
    }
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "repo_url": self.repo_url,
            "name": self.name,
            "display_name": self.display_name,
            "version": self.version,
            "author": self.author,
            "description": self.description,
            "enabled": self.enabled != 0,
            "commit": self.commit_sha,
            "config": self.config(),
            "config_revision": self.config_revision(),
            "schema": self.schema(),
            "status": self.status,
            "status_detail": self.status_detail,
            "updated_at": self.updated_at,
        })
    }
}

/// Bot view. `platforms` must already be rendered with masked secrets.
pub fn bot_view(bot: &BotRow, platforms: Vec<Value>, plugins: &[PluginRow]) -> Value {
    json!({
        "id": bot.id,
        "name": bot.name,
        "avatar": bot.avatar,
        "status": bot.status,
        "status_detail": bot.status_detail,
        "created_at": bot.created_at,
        "updated_at": bot.updated_at,
        "settings": BotSettings::from_stored(&bot.settings_json),
        "platforms": platforms,
        "plugins": plugins.iter().map(PluginRow::view).collect::<Vec<_>>(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesiredQuota {
    pub memory_mb: i64,
    pub storage_mb: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesiredLlm {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// Base URL the node calls instead of `base_url`. Empty means `base_url`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    /// AI Gateway token sent with requests to `endpoint`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gateway_token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesiredPlatform {
    pub id: String,
    pub kind: String,
    pub enabled: bool,
    pub app_id: String,
    pub secret: String,
    pub extra: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesiredPlugin {
    pub id: String,
    pub name: String,
    pub repo_url: String,
    pub version: String,
    pub enabled: bool,
    pub config: Value,
    /// Changes when a reinstall is requested.
    pub install_rev: i64,
    /// Commit to install. Empty installs the head of the repository's branch.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DesiredBot {
    pub bot_id: String,
    pub user_id: String,
    pub name: String,
    pub running: bool,
    pub quota: DesiredQuota,
    pub settings: BotSettings,
    pub llm: Option<DesiredLlm>,
    pub platforms: Vec<DesiredPlatform>,
    pub plugins: Vec<DesiredPlugin>,
}

/// Builds the node document. `decrypt` turns stored ciphertext into plaintext.
/// Platforms whose secret cannot be decrypted are sent disabled with an empty secret.
pub fn build_desired(
    user: &UserRow,
    bot: &BotRow,
    platforms: &[PlatformRow],
    plugins: &[PluginRow],
    llm_key: Option<&LlmKeyRow>,
    decrypt: &dyn Fn(&str) -> Option<String>,
) -> DesiredBot {
    let settings = BotSettings::from_stored(&bot.settings_json);
    let llm = llm_key
        .filter(|k| k.user_id == user.id && settings.llm.key_id.as_deref() == Some(k.id.as_str()))
        .and_then(|k| {
            let api_key = decrypt(&k.api_key_enc)?;
            let model = if settings.llm.model.trim().is_empty() { k.model.clone() } else { settings.llm.model.clone() };
            Some(DesiredLlm { base_url: k.base_url.clone(), model, api_key, endpoint: String::new(), gateway_token: String::new() })
        });
    let platforms = platforms
        .iter()
        .filter(|p| p.bot_id == bot.id)
        .map(|p| {
            let secret = decrypt(&p.secret_enc);
            DesiredPlatform {
                id: p.id.clone(),
                kind: p.kind.clone(),
                enabled: p.enabled != 0 && secret.is_some(),
                app_id: p.app_id.clone(),
                secret: secret.unwrap_or_default(),
                extra: parse_json_or(Some(&p.extra_json), json!({})),
            }
        })
        .collect();
    let plugins = plugins
        .iter()
        .filter(|p| p.bot_id == bot.id)
        .map(|p| DesiredPlugin {
            id: p.id.clone(),
            name: p.name.clone(),
            repo_url: p.repo_url.clone(),
            version: p.version.clone(),
            enabled: p.enabled != 0,
            config: p.config(),
            install_rev: p.install_rev,
            commit: p.commit_sha.clone(),
        })
        .collect();
    DesiredBot {
        bot_id: bot.id.clone(),
        user_id: user.id.clone(),
        name: bot.name.clone(),
        running: bot.desired_running != 0,
        quota: DesiredQuota { memory_mb: user.quota_memory_mb, storage_mb: user.quota_storage_mb },
        settings,
        llm,
        platforms,
        plugins,
    }
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub fn user() -> UserRow {
        UserRow {
            id: "u1".into(),
            email: "a@b.co".into(),
            password_hash: "x".into(),
            role: "user".into(),
            created_at: 1,
            quota_memory_mb: 256,
            quota_storage_mb: 1024,
            quota_bots: 1,
            quota_plugins: 8,
            last_login_at: None,
            service_until: None,
        }
    }

    pub fn bot(settings: &BotSettings) -> BotRow {
        BotRow {
            id: "b1".into(),
            user_id: "u1".into(),
            name: "小鱼".into(),
            avatar: None,
            node_id: "node-1".into(),
            status: "starting".into(),
            status_detail: String::new(),
            desired_running: 1,
            needs_push: 0,
            created_at: 1,
            updated_at: 2,
            settings_json: serde_json::to_string(settings).unwrap(),
        }
    }

    pub fn key() -> LlmKeyRow {
        LlmKeyRow {
            id: "k1".into(),
            user_id: "u1".into(),
            name: "ds".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
            api_key_enc: "enc:sk-1234567890".into(),
            created_at: 1,
        }
    }

    pub fn platform(id: &str, enabled: i64, secret_enc: &str) -> PlatformRow {
        PlatformRow {
            id: id.into(),
            bot_id: "b1".into(),
            kind: "qq_official".into(),
            display_name: "QQ".into(),
            app_id: "1020".into(),
            secret_enc: secret_enc.into(),
            extra_json: "{}".into(),
            enabled,
            status: "connecting".into(),
            status_detail: String::new(),
            created_at: 1,
        }
    }

    pub fn plugin() -> PluginRow {
        PluginRow {
            id: "p1".into(),
            bot_id: "b1".into(),
            repo_url: "https://github.com/a/b".into(),
            name: "b".into(),
            display_name: "B".into(),
            version: "1.0".into(),
            author: "a".into(),
            description: String::new(),
            enabled: 1,
            config_json: r#"{"x":1}"#.into(),
            schema_json: Some(r#"{"x":{"type":"int"}}"#.into()),
            status: "installing".into(),
            status_detail: String::new(),
            install_rev: 5,
            commit_sha: "c".repeat(40),
            created_at: 1,
            updated_at: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn dec(s: &str) -> Option<String> {
        s.strip_prefix("enc:").map(str::to_string)
    }

    #[test]
    fn builds_desired_bot() {
        let settings = BotSettings::for_new_bot("小鱼", Some(("k1", "")));
        let bot = bot(&settings);
        let d = build_desired(
            &user(),
            &bot,
            &[platform("pl1", 1, "enc:s3cret"), platform("pl2", 1, "garbage"), platform("pl3", 0, "enc:x")],
            &[plugin()],
            Some(&key()),
            &dec,
        );
        assert!(d.running);
        assert_eq!(d.quota, DesiredQuota { memory_mb: 256, storage_mb: 1024 });
        let llm = d.llm.clone().unwrap();
        assert_eq!(llm.api_key, "sk-1234567890");
        assert_eq!(llm.model, "deepseek-chat");
        assert_eq!(d.platforms.len(), 3);
        assert!(d.platforms[0].enabled);
        assert_eq!(d.platforms[0].secret, "s3cret");
        assert!(!d.platforms[1].enabled);
        assert_eq!(d.platforms[1].secret, "");
        assert!(!d.platforms[2].enabled);
        assert_eq!(d.plugins[0].config, json!({"x": 1}));
        assert_eq!(d.plugins[0].install_rev, 5);
        assert_eq!(d.plugins[0].commit, "c".repeat(40));

        let v = serde_json::to_value(&d).unwrap();
        for k in ["bot_id", "user_id", "name", "running", "quota", "settings", "llm", "platforms", "plugins"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert_eq!(v["settings"]["llm"]["key_id"], "k1");
    }

    #[test]
    fn desired_llm_requires_matching_key() {
        let mut settings = BotSettings::for_new_bot("x", Some(("k1", "deepseek-reasoner")));
        let d = build_desired(&user(), &bot(&settings), &[], &[], Some(&key()), &dec);
        assert_eq!(d.llm.unwrap().model, "deepseek-reasoner");
        settings.llm.key_id = Some("other".into());
        let d = build_desired(&user(), &bot(&settings), &[], &[], Some(&key()), &dec);
        assert!(d.llm.is_none());
        let mut stopped = bot(&settings);
        stopped.desired_running = 0;
        assert!(!build_desired(&user(), &stopped, &[], &[], None, &dec).running);
        let v = serde_json::to_value(build_desired(&user(), &stopped, &[], &[], None, &dec)).unwrap();
        assert!(v["llm"].is_null());
    }

    #[test]
    fn admin_user_view_shape() {
        let mut u = user();
        u.last_login_at = Some(42);
        let counts = UserCounts { bot_count: 1, plugin_count: 2, running_count: 1, error_count: 3 };
        let v = admin_user_view(&u, &counts, Some((10, 20)));
        assert_eq!(v["email"], "a@b.co");
        assert_eq!(v["quota"]["plugins"], 8);
        assert_eq!((v["bots"].clone(), v["plugins"].clone(), v["running"].clone(), v["errors"].clone()), (json!(1), json!(2), json!(1), json!(3)));
        assert_eq!((v["memory_bytes"].clone(), v["storage_bytes"].clone()), (json!(10), json!(20)));
        assert_eq!(v["last_login_at"], 42);
        assert!(v.get("password_hash").is_none());
        let unknown = admin_user_view(&user(), &UserCounts::default(), None);
        assert!(unknown["memory_bytes"].is_null() && unknown["storage_bytes"].is_null() && unknown["last_login_at"].is_null());
    }

    #[test]
    fn service_active_rules() {
        let mut u = user();
        assert!(!u.service_active_at(100));
        assert_eq!(u.view_at(100)["service_until"], Value::Null);
        u.service_until = Some(200);
        assert!(u.service_active_at(100));
        assert!(!u.service_active_at(200));
        assert_eq!(u.view_at(300)["service_active"], false);
        u.role = "admin".into();
        assert!(u.service_active_at(300));
        assert_eq!(u.view_at(300)["service_active"], true);
    }

    #[test]
    fn views_hide_secrets() {
        let p = platform("pl1", 1, "enc:abcdefghijklmn");
        let v = p.view(Some("abcdefghijklmn"));
        assert_eq!(v["secret_masked"], "abc…klmn");
        assert!(v.get("secret_enc").is_none());
        let off = platform("pl2", 0, "x").view(None);
        assert_eq!(off["status"], "disabled");
        let k = key().view(Some("sk-1234567890"));
        assert_eq!(k["key_masked"], "sk-…7890");
        assert!(k.get("api_key_enc").is_none());
        let u = user().view();
        assert!(u.get("password_hash").is_none());
        assert_eq!(u["quota"]["bots"], 1);
    }
}
