use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::model::{Behavior, MessageType, PLATFORM_QQ, PLATFORM_WEIXIN};

/// Short identity prefixes accepted in admin and block lists for each platform kind.
fn prefixes(platform_kind: &str) -> &'static [&'static str] {
    match platform_kind {
        PLATFORM_QQ => &["qq", "qq_official"],
        PLATFORM_WEIXIN => &["weixin", "wx", "wechat", "weixin_ilink"],
        _ => &[],
    }
}

/// True when `entry` names `id` either bare or as `<platform prefix>:<id>`.
pub fn identity_matches(entry: &str, platform_kind: &str, id: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() || id.is_empty() {
        return false;
    }
    if entry == id {
        return true;
    }
    match entry.split_once(':') {
        Some((p, rest)) => rest == id && prefixes(platform_kind).iter().any(|x| x.eq_ignore_ascii_case(p)),
        None => false,
    }
}

/// Account ids of the entries that apply to `platform_kind`: bare entries and entries with one of its prefixes.
pub fn listed_ids(list: &[String], platform_kind: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in list.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        let id = match entry.split_once(':') {
            Some((p, rest)) if prefixes(platform_kind).iter().any(|x| x.eq_ignore_ascii_case(p)) => rest,
            Some(_) => continue,
            None => entry,
        };
        if !id.is_empty() && !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    }
    out
}

pub fn is_listed(list: &[String], platform_kind: &str, ids: &[&str]) -> bool {
    list.iter()
        .any(|e| ids.iter().any(|id| identity_matches(e, platform_kind, id)))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Wake {
    pub woke: bool,
    /// Message text with a matched wake prefix removed.
    pub text: String,
}

/// Applies the wake rules of a bot to one inbound message.
pub fn wake(behavior: &Behavior, mt: MessageType, is_at: bool, text: &str) -> Wake {
    let trimmed = text.trim();
    for p in &behavior.wake_prefixes {
        if !p.is_empty() && trimmed.starts_with(p.as_str()) {
            return Wake {
                woke: true,
                text: trimmed[p.len()..].trim_start().to_string(),
            };
        }
    }
    let woke = match mt {
        MessageType::Private => behavior.reply_in_private,
        MessageType::Group => (is_at && behavior.reply_when_at) || behavior.reply_in_group_without_at,
    };
    Wake {
        woke,
        text: trimmed.to_string(),
    }
}

/// Sliding one minute window counter per key.
#[derive(Default)]
pub struct RateLimiter {
    windows: HashMap<String, VecDeque<Instant>>,
}

impl RateLimiter {
    /// Records a hit and returns false when the key already reached `limit` hits in the last minute.
    /// A limit of zero disables limiting.
    pub fn allow(&mut self, key: &str, limit: u32, now: Instant) -> bool {
        if limit == 0 {
            return true;
        }
        let window = Duration::from_secs(60);
        if self.windows.len() > 4096 {
            self.windows
                .retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < window));
        }
        let q = self.windows.entry(key.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= limit as usize {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn behavior() -> Behavior {
        Behavior::default()
    }

    #[test]
    fn listed_ids_keep_entries_of_the_platform() {
        let list: Vec<String> = ["qq:A1", "B2", "weixin:W3", "qq_official:A1", " ", "QQ:C4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(listed_ids(&list, PLATFORM_QQ), vec!["A1", "B2", "C4"]);
        assert_eq!(listed_ids(&list, PLATFORM_WEIXIN), vec!["B2", "W3"]);
    }

    #[test]
    fn private_wakes_by_default() {
        let w = wake(&behavior(), MessageType::Private, false, " 你好 ");
        assert_eq!(
            w,
            Wake {
                woke: true,
                text: "你好".into()
            }
        );
        let mut b = behavior();
        b.reply_in_private = false;
        assert!(!wake(&b, MessageType::Private, false, "你好").woke);
        assert!(wake(&b, MessageType::Private, false, "/help").woke);
    }

    #[test]
    fn group_needs_at_or_prefix() {
        let b = behavior();
        assert!(!wake(&b, MessageType::Group, false, "随便聊聊").woke);
        assert!(wake(&b, MessageType::Group, true, "随便聊聊").woke);
        let w = wake(&b, MessageType::Group, false, "/ help me");
        assert_eq!(
            w,
            Wake {
                woke: true,
                text: "help me".into()
            }
        );
        let mut b2 = behavior();
        b2.reply_when_at = false;
        assert!(!wake(&b2, MessageType::Group, true, "hi").woke);
        b2.reply_in_group_without_at = true;
        assert!(wake(&b2, MessageType::Group, false, "hi").woke);
    }

    #[test]
    fn multiple_prefixes_and_empty_prefix_ignored() {
        let mut b = behavior();
        b.wake_prefixes = vec!["".into(), "鱼鱼".into(), "#".into()];
        assert_eq!(wake(&b, MessageType::Group, false, "鱼鱼 讲个笑话").text, "讲个笑话");
        assert!(wake(&b, MessageType::Group, false, "#x").woke);
        assert!(!wake(&b, MessageType::Group, false, "x").woke);
    }

    #[test]
    fn identity_lists() {
        assert!(identity_matches("qq:123", PLATFORM_QQ, "123"));
        assert!(identity_matches("123", PLATFORM_WEIXIN, "123"));
        assert!(!identity_matches("qq:123", PLATFORM_WEIXIN, "123"));
        assert!(identity_matches("wx:abc@im.wechat", PLATFORM_WEIXIN, "abc@im.wechat"));
        assert!(!identity_matches("qq:1234", PLATFORM_QQ, "123"));
        assert!(is_listed(&["qq:g1".into()], PLATFORM_QQ, &["u1", "g1"]));
    }

    #[test]
    fn rate_limit_window() {
        let mut r = RateLimiter::default();
        let t0 = Instant::now();
        assert!(r.allow("a", 2, t0));
        assert!(r.allow("a", 2, t0));
        assert!(!r.allow("a", 2, t0 + Duration::from_secs(10)));
        assert!(r.allow("b", 2, t0));
        assert!(r.allow("a", 2, t0 + Duration::from_secs(61)));
        assert!(r.allow("a", 0, t0));
    }
}
