use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;

use crate::sign::now_ms;

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    #[serde(skip)]
    pub seq: u64,
    pub ts: i64,
    pub level: &'static str,
    pub source: String,
    pub message: String,
}

struct Inner {
    entries: VecDeque<LogEntry>,
    next_seq: u64,
    bytes: usize,
}

/// Bounded per bot log ring buffer with monotonically increasing cursors.
pub struct LogBuffer {
    bot_id: String,
    cap: usize,
    inner: Mutex<Inner>,
}

pub const MAX_MESSAGE: usize = 2000;

impl LogBuffer {
    pub fn new(bot_id: &str, cap: usize) -> Self {
        LogBuffer {
            bot_id: bot_id.to_string(),
            cap,
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                next_seq: 1,
                bytes: 0,
            }),
        }
    }

    pub fn push(&self, level: &'static str, source: &str, message: impl Into<String>) {
        let mut message: String = message.into();
        if message.len() > MAX_MESSAGE {
            let mut cut = MAX_MESSAGE;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        match level {
            "error" => tracing::warn!(bot = %self.bot_id, source, "{message}"),
            _ => tracing::debug!(bot = %self.bot_id, source, level, "{message}"),
        }
        let mut g = self.inner.lock().unwrap();
        let seq = g.next_seq;
        g.next_seq += 1;
        g.bytes += message.len() + source.len() + 48;
        g.entries.push_back(LogEntry {
            seq,
            ts: now_ms(),
            level,
            source: source.to_string(),
            message,
        });
        while g.entries.len() > self.cap {
            if let Some(old) = g.entries.pop_front() {
                g.bytes = g.bytes.saturating_sub(old.message.len() + old.source.len() + 48);
            }
        }
    }

    pub fn info(&self, source: &str, message: impl Into<String>) {
        self.push("info", source, message)
    }

    pub fn warn(&self, source: &str, message: impl Into<String>) {
        self.push("warn", source, message)
    }

    pub fn error(&self, source: &str, message: impl Into<String>) {
        self.push("error", source, message)
    }

    /// Returns up to `limit` entries with a sequence greater than `after`, plus the new cursor.
    pub fn read_after(&self, after: Option<u64>, limit: usize) -> (Vec<LogEntry>, String) {
        let g = self.inner.lock().unwrap();
        let after = after.unwrap_or(0);
        let items: Vec<LogEntry> = g
            .entries
            .iter()
            .filter(|e| e.seq > after)
            .take(limit)
            .cloned()
            .collect();
        let cursor = items.last().map(|e| e.seq).unwrap_or(after);
        (items, cursor.to_string())
    }

    pub fn approx_bytes(&self) -> usize {
        self.inner.lock().unwrap().bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_and_cursor() {
        let b = LogBuffer::new("b", 3);
        for i in 0..5 {
            b.info("core", format!("m{i}"));
        }
        let (items, cur) = b.read_after(None, 10);
        assert_eq!(
            items.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(),
            vec!["m2", "m3", "m4"]
        );
        assert_eq!(cur, "5");
        let (items, cur2) = b.read_after(Some(5), 10);
        assert!(items.is_empty());
        assert_eq!(cur2, "5");
        b.warn("llm", "x");
        let (items, _) = b.read_after(Some(5), 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source, "llm");
    }
}
