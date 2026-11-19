use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DesiredBot {
    pub bot_id: String,
    pub user_id: String,
    pub name: String,
    pub running: bool,
    pub quota: Quota,
    pub settings: BotSettings,
    pub llm: Option<LlmSpec>,
    pub platforms: Vec<PlatformSpec>,
    pub plugins: Vec<PluginSpec>,
}

impl Default for DesiredBot {
    fn default() -> Self {
        DesiredBot {
            bot_id: String::new(),
            user_id: String::new(),
            name: String::new(),
            running: true,
            quota: Quota::default(),
            settings: BotSettings::default(),
            llm: None,
            platforms: vec![],
            plugins: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Quota {
    pub memory_mb: u64,
    pub storage_mb: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Quota {
            memory_mb: 256,
            storage_mb: 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct BotSettings {
    pub persona: Persona,
    pub llm: LlmSettings,
    pub behavior: Behavior,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Persona {
    pub name: String,
    pub system_prompt: String,
    pub greeting: String,
}

impl Default for Persona {
    fn default() -> Self {
        Persona {
            name: "大肥鱼".into(),
            system_prompt: String::new(),
            greeting: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmSettings {
    pub key_id: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub context_turns: u32,
}

impl Default for LlmSettings {
    fn default() -> Self {
        LlmSettings {
            key_id: None,
            model: String::new(),
            temperature: 0.7,
            max_tokens: 1024,
            context_turns: 12,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Behavior {
    pub wake_prefixes: Vec<String>,
    pub reply_when_at: bool,
    pub reply_in_private: bool,
    pub reply_in_group_without_at: bool,
    pub admins: Vec<String>,
    pub blocklist: Vec<String>,
    pub rate_limit_per_minute: u32,
}

impl Default for Behavior {
    fn default() -> Self {
        Behavior {
            wake_prefixes: vec!["/".into()],
            reply_when_at: true,
            reply_in_private: true,
            reply_in_group_without_at: false,
            admins: vec![],
            blocklist: vec![],
            rate_limit_per_minute: 20,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct LlmSpec {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PlatformSpec {
    pub id: String,
    pub kind: String,
    pub enabled: bool,
    pub app_id: String,
    pub secret: String,
    pub extra: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PluginSpec {
    pub id: String,
    pub name: String,
    pub repo_url: String,
    pub version: String,
    pub enabled: bool,
    pub config: Value,
}

impl PluginSpec {
    /// True when a change between two specs requires reinstalling the plugin code.
    pub fn needs_reinstall(&self, other: &PluginSpec) -> bool {
        self.repo_url != other.repo_url || self.version != other.version || self.name != other.name
    }
}

pub const PLATFORM_QQ: &str = "qq_official";
pub const PLATFORM_WEIXIN: &str = "weixin_ilink";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Component {
    Plain {
        text: String,
    },
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
    },
    At {
        user_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Reply {
        id: String,
    },
    Record {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Face {
        id: String,
    },
    #[serde(other)]
    Unknown,
}

impl Component {
    pub fn plain(text: impl Into<String>) -> Self {
        Component::Plain { text: text.into() }
    }

    pub fn image_url(url: impl Into<String>) -> Self {
        Component::Image {
            url: Some(url.into()),
            path: None,
            base64: None,
        }
    }
}

/// Concatenated plain text of a chain.
pub fn chain_text(chain: &[Component]) -> String {
    chain
        .iter()
        .filter_map(|c| match c {
            Component::Plain { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Group,
    Private,
}

impl MessageType {
    pub fn origin_tag(self) -> &'static str {
        match self {
            MessageType::Group => "GroupMessage",
            MessageType::Private => "FriendMessage",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Sender {
    pub user_id: String,
    pub nickname: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageEvent {
    pub platform: String,
    pub bot_id: String,
    pub self_id: String,
    pub message_type: MessageType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub message_id: String,
    pub unified_msg_origin: String,
    pub sender: Sender,
    pub message_str: String,
    pub chain: Vec<Component>,
    pub is_at_or_wake: bool,
    pub is_admin: bool,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

pub fn unified_origin(platform_id: &str, mt: MessageType, session_id: &str) -> String {
    format!("{platform_id}:{}:{session_id}", mt.origin_tag())
}

/// Parses `<platform_id>:<GroupMessage|FriendMessage>:<session_id>`.
pub fn parse_origin(origin: &str) -> Option<(String, MessageType, String)> {
    let mut parts = origin.splitn(3, ':');
    let pid = parts.next()?.to_string();
    let mt = match parts.next()? {
        "GroupMessage" => MessageType::Group,
        "FriendMessage" => MessageType::Private,
        _ => return None,
    };
    let sid = parts.next()?.to_string();
    if pid.is_empty() || sid.is_empty() {
        return None;
    }
    Some((pid, mt, sid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desired_bot_defaults() {
        let d: DesiredBot = serde_json::from_value(json!({"bot_id": "b", "user_id": "u"})).unwrap();
        assert!(d.running);
        assert_eq!(d.settings.behavior.wake_prefixes, vec!["/"]);
        assert_eq!(d.settings.llm.context_turns, 12);
        assert_eq!(d.quota.memory_mb, 256);
    }

    #[test]
    fn component_roundtrip() {
        let chain: Vec<Component> = serde_json::from_value(json!([
            {"type": "plain", "text": "hi"},
            {"type": "image", "url": "https://x/y.png"},
            {"type": "at", "user_id": "1"},
            {"type": "video", "url": "v"}
        ]))
        .unwrap();
        assert_eq!(chain[0], Component::plain("hi"));
        assert_eq!(chain[3], Component::Unknown);
        let v = serde_json::to_value(&chain[1]).unwrap();
        assert_eq!(v, json!({"type": "image", "url": "https://x/y.png"}));
    }

    #[test]
    fn origin_parse() {
        let o = unified_origin("p1", MessageType::Group, "g:1");
        assert_eq!(o, "p1:GroupMessage:g:1");
        assert_eq!(parse_origin(&o), Some(("p1".into(), MessageType::Group, "g:1".into())));
        assert_eq!(parse_origin("p1:Other:x"), None);
    }
}
