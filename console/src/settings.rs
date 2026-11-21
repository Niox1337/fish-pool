//! BotSettings stored in `bots.settings_json`, with defaults, merge patching and validation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Persona {
    pub name: String,
    pub system_prompt: String,
    pub greeting: String,
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            name: "大肥鱼".into(),
            system_prompt: "你是大肥鱼，一个热情、幽默、乐于助人的聊天伙伴。回答简洁自然，使用中文。".into(),
            greeting: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    pub key_id: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub context_turns: u32,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            key_id: None,
            model: String::new(),
            temperature: 0.7,
            max_tokens: 1024,
            context_turns: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        Self {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BotSettings {
    pub persona: Persona,
    pub llm: LlmSettings,
    pub behavior: Behavior,
}

impl BotSettings {
    pub fn for_new_bot(name: &str, key: Option<(&str, &str)>) -> Self {
        let mut s = Self::default();
        s.persona.name = name.to_string();
        if let Some((id, model)) = key {
            s.llm.key_id = Some(id.to_string());
            s.llm.model = model.to_string();
        }
        s
    }

    /// Parses stored JSON, falling back to defaults for missing or broken fields.
    pub fn from_stored(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), String> {
        let p = &self.persona;
        if p.name.trim().is_empty() || p.name.chars().count() > 32 {
            return Err("人设名称长度应为 1 到 32 个字符".into());
        }
        if p.system_prompt.chars().count() > 8000 {
            return Err("人设提示词不能超过 8000 个字符".into());
        }
        if p.greeting.chars().count() > 500 {
            return Err("开场白不能超过 500 个字符".into());
        }
        let l = &self.llm;
        if !(0.0..=2.0).contains(&l.temperature) || l.temperature.is_nan() {
            return Err("temperature 应在 0 到 2 之间".into());
        }
        if l.max_tokens == 0 || l.max_tokens > 32768 {
            return Err("max_tokens 应在 1 到 32768 之间".into());
        }
        if l.context_turns > 100 {
            return Err("上下文轮数不能超过 100".into());
        }
        if l.model.chars().count() > 100 {
            return Err("模型名称过长".into());
        }
        let b = &self.behavior;
        let list_ok =
            |v: &Vec<String>, max: usize| v.len() <= max && v.iter().all(|s| !s.is_empty() && s.chars().count() <= 64);
        if !list_ok(&b.wake_prefixes, 10) {
            return Err("唤醒前缀最多 10 个，每个 1 到 64 个字符".into());
        }
        if !list_ok(&b.admins, 100) || !list_ok(&b.blocklist, 1000) {
            return Err("管理员或黑名单条目无效".into());
        }
        if b.rate_limit_per_minute == 0 || b.rate_limit_per_minute > 600 {
            return Err("每分钟回复上限应在 1 到 600 之间".into());
        }
        Ok(())
    }

    /// Applies a JSON merge patch (null removes a key, which then falls back to its default).
    pub fn apply_patch(&self, patch: &Value) -> Result<Self, String> {
        if !patch.is_object() {
            return Err("settings 必须是 JSON 对象".into());
        }
        let mut current = serde_json::to_value(self).map_err(|e| e.to_string())?;
        merge_patch(&mut current, patch);
        let next: Self = serde_json::from_value(current).map_err(|e| format!("settings 格式错误：{e}"))?;
        next.validate()?;
        Ok(next)
    }
}

pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_roundtrip() {
        let s = BotSettings::for_new_bot("小鱼", Some(("k1", "deepseek-chat")));
        assert_eq!(s.persona.name, "小鱼");
        assert_eq!(s.llm.key_id.as_deref(), Some("k1"));
        assert!(s.validate().is_ok());
        let stored = serde_json::to_string(&s).unwrap();
        assert_eq!(BotSettings::from_stored(&stored), s);
        assert_eq!(BotSettings::from_stored("{"), BotSettings::default());
        assert_eq!(
            BotSettings::from_stored(r#"{"persona":{"name":"x"}}"#).llm.max_tokens,
            1024
        );
    }

    #[test]
    fn patches() {
        let s = BotSettings::default();
        let n = s
            .apply_patch(&json!({"persona": {"greeting": "你好"}, "llm": {"temperature": 1.2}}))
            .unwrap();
        assert_eq!(n.persona.greeting, "你好");
        assert_eq!(n.persona.name, "大肥鱼");
        assert_eq!(n.llm.temperature, 1.2);
        let n2 = n.apply_patch(&json!({"persona": {"greeting": null}})).unwrap();
        assert_eq!(n2.persona.greeting, "");
        assert!(s.apply_patch(&json!({"llm": {"temperature": 3}})).is_err());
        assert!(s.apply_patch(&json!({"llm": {"max_tokens": "x"}})).is_err());
        assert!(s.apply_patch(&json!({"persona": {"name": ""}})).is_err());
        assert!(s.apply_patch(&json!([1])).is_err());
        assert!(s
            .apply_patch(&json!({"behavior": {"rate_limit_per_minute": 0}}))
            .is_err());
    }
}
