use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::DesiredBot;
use crate::sign::now_ms;

/// Node state database (`node.db`): desired bots, small key value state and usage samples.
pub struct NodeDb {
    conn: Mutex<Connection>,
}

fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -512)?;
    Ok(conn)
}

impl NodeDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bots(
                bot_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, desired_json TEXT NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS kv(k TEXT PRIMARY KEY, v TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS usage_samples(
                ts INTEGER NOT NULL, bot_id TEXT NOT NULL, user_id TEXT NOT NULL,
                memory_bytes INTEGER NOT NULL, storage_bytes INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS usage_user_ts ON usage_samples(user_id, ts);",
        )?;
        Ok(NodeDb { conn: Mutex::new(conn) })
    }

    pub fn save_bot(&self, d: &DesiredBot) -> Result<()> {
        let json = serde_json::to_string(d)?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO bots(bot_id, user_id, desired_json, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(bot_id) DO UPDATE SET user_id = excluded.user_id, desired_json = excluded.desired_json,
             updated_at = excluded.updated_at",
            params![d.bot_id, d.user_id, json, now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_bot(&self, bot_id: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM bots WHERE bot_id = ?1", params![bot_id])?;
        c.execute("DELETE FROM usage_samples WHERE bot_id = ?1", params![bot_id])?;
        c.execute("DELETE FROM kv WHERE k LIKE ?1", params![format!("bot:{bot_id}:%")])?;
        Ok(())
    }

    pub fn load_bots(&self) -> Result<Vec<DesiredBot>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT desired_json FROM bots ORDER BY bot_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = vec![];
        for row in rows {
            match serde_json::from_str::<DesiredBot>(&row?) {
                Ok(d) => out.push(d),
                Err(e) => tracing::warn!("skip corrupt desired bot row: {e}"),
            }
        }
        Ok(out)
    }

    pub fn kv_get(&self, k: &str) -> Option<String> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT v FROM kv WHERE k = ?1", params![k], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn kv_set(&self, k: &str, v: &str) {
        let r = self.conn.lock().unwrap().execute(
            "INSERT INTO kv(k, v) VALUES(?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![k, v],
        );
        if let Err(e) = r {
            tracing::warn!("kv_set {k}: {e}");
        }
    }

    pub fn kv_delete(&self, k: &str) {
        let _ = self
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM kv WHERE k = ?1", params![k]);
    }

    pub fn add_samples(&self, ts: i64, rows: &[(String, String, u64, u64)]) -> Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        for (bot_id, user_id, mem, sto) in rows {
            tx.execute(
                "INSERT INTO usage_samples(ts, bot_id, user_id, memory_bytes, storage_bytes) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![ts, bot_id, user_id, *mem as i64, *sto as i64],
            )?;
        }
        tx.execute(
            "DELETE FROM usage_samples WHERE ts < ?1",
            params![ts - 24 * 3600 * 1000],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Returns `(ts, memory_bytes, storage_bytes)` summed over all bots per sample round.
    pub fn node_history(&self, since: i64) -> Result<Vec<(i64, u64, u64)>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT ts, SUM(memory_bytes), SUM(storage_bytes) FROM usage_samples
             WHERE ts >= ?1 GROUP BY ts ORDER BY ts",
        )?;
        let rows = stmt.query_map(params![since], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Returns `(ts, memory_bytes, storage_bytes)` summed over a user's bots per sample round.
    pub fn history(&self, user_id: &str, since: i64) -> Result<Vec<(i64, u64, u64)>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT ts, SUM(memory_bytes), SUM(storage_bytes) FROM usage_samples
             WHERE user_id = ?1 AND ts >= ?2 GROUP BY ts ORDER BY ts",
        )?;
        let rows = stmt.query_map(params![user_id, since], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// Conversation history database (`conversations.db`).
pub struct ConvDb {
    conn: Mutex<Connection>,
}

/// Number of messages kept per session. Older rows are pruned on insert.
pub const KEEP_PER_SESSION: i64 = 200;
/// Number of recently seen sessions and senders kept per bot and kind.
pub const KEEP_SEEN: i64 = 100;

/// A session or sender that recently messaged a bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// `session` or `user`.
    pub kind: String,
    /// Unified origin of a session, or the sender's platform account id.
    pub id: String,
    /// Unified origin of the session the entry was last seen in.
    pub origin: String,
    pub ts: i64,
}

impl ConvDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY AUTOINCREMENT, bot_id TEXT NOT NULL, session TEXT NOT NULL,
                role TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS messages_bot_session ON messages(bot_id, session, id);
             CREATE TABLE IF NOT EXISTS seen(
                bot_id TEXT NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL,
                origin TEXT NOT NULL, ts INTEGER NOT NULL, seq INTEGER NOT NULL,
                PRIMARY KEY(bot_id, kind, id));",
        )?;
        Ok(ConvDb { conn: Mutex::new(conn) })
    }

    pub fn append(&self, bot_id: &str, session: &str, role: &str, content: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO messages(bot_id, session, role, content, ts) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![bot_id, session, role, content, now_ms()],
        )?;
        c.execute(
            "DELETE FROM messages WHERE bot_id = ?1 AND session = ?2 AND id <= (
                SELECT id FROM messages WHERE bot_id = ?1 AND session = ?2 ORDER BY id DESC LIMIT 1 OFFSET ?3)",
            params![bot_id, session, KEEP_PER_SESSION],
        )?;
        Ok(())
    }

    /// Returns the last `n` messages of a session in chronological order as `(role, content)`.
    pub fn recent(&self, bot_id: &str, session: &str, n: usize) -> Result<Vec<(String, String)>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT role, content FROM messages WHERE bot_id = ?1 AND session = ?2 ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![bot_id, session, n as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out: Vec<(String, String)> = rows.filter_map(|r| r.ok()).collect();
        out.reverse();
        Ok(out)
    }

    /// Approximate storage of a bot's rows: payload bytes plus a fixed per row overhead.
    pub fn bot_bytes(&self, bot_id: &str) -> u64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(CAST(content AS BLOB)) + LENGTH(session) + LENGTH(role) + 48), 0)
                 FROM messages WHERE bot_id = ?1",
                params![bot_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v as u64)
            .unwrap_or(0)
    }

    pub fn delete_bot(&self, bot_id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM messages WHERE bot_id = ?1", params![bot_id])?;
        Ok(())
    }

    /// Sessions of a bot as `(session, messages, bytes, last_ts)`, most recent first. Bytes use the
    /// same estimate as `bot_bytes`.
    pub fn sessions(&self, bot_id: &str) -> Result<Vec<(String, u64, u64, i64)>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT session, COUNT(*), SUM(LENGTH(CAST(content AS BLOB)) + LENGTH(session) + LENGTH(role) + 48),
                    MAX(ts)
             FROM messages WHERE bot_id = ?1 GROUP BY session ORDER BY MAX(ts) DESC",
        )?;
        let rows = stmt.query_map(params![bot_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)?,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Deletes one session of a bot and returns the number of removed messages.
    pub fn delete_session(&self, bot_id: &str, session: &str) -> Result<usize> {
        Ok(self.conn.lock().unwrap().execute(
            "DELETE FROM messages WHERE bot_id = ?1 AND session = ?2",
            params![bot_id, session],
        )?)
    }

    /// Records that a session or sender messaged a bot, keeping the newest `KEEP_SEEN` entries per kind.
    pub fn touch_seen(&self, bot_id: &str, kind: &str, id: &str, origin: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO seen(bot_id, kind, id, origin, ts, seq)
             VALUES(?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(seq), 0) + 1 FROM seen WHERE bot_id = ?1))
             ON CONFLICT(bot_id, kind, id) DO UPDATE SET origin = excluded.origin, ts = excluded.ts, seq = excluded.seq",
            params![bot_id, kind, id, origin, now_ms()],
        )?;
        c.execute(
            "DELETE FROM seen WHERE bot_id = ?1 AND kind = ?2 AND id NOT IN (
                SELECT id FROM seen WHERE bot_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT ?3)",
            params![bot_id, kind, KEEP_SEEN],
        )?;
        Ok(())
    }

    /// Recently seen sessions and senders of a bot, most recent first.
    pub fn seen(&self, bot_id: &str) -> Result<Vec<Seen>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT kind, id, origin, ts FROM seen WHERE bot_id = ?1 ORDER BY seq DESC")?;
        let rows = stmt.query_map(params![bot_id], |r| {
            Ok(Seen {
                kind: r.get(0)?,
                id: r.get(1)?,
                origin: r.get(2)?,
                ts: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn delete_seen(&self, bot_id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM seen WHERE bot_id = ?1", params![bot_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_db_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = NodeDb::open(&dir.path().join("node.db")).unwrap();
        let d = DesiredBot {
            bot_id: "b1".into(),
            user_id: "u1".into(),
            ..Default::default()
        };
        db.save_bot(&d).unwrap();
        db.save_bot(&d).unwrap();
        assert_eq!(db.load_bots().unwrap(), vec![d]);
        db.kv_set("bot:b1:x", "1");
        assert_eq!(db.kv_get("bot:b1:x").as_deref(), Some("1"));
        let now = now_ms();
        db.add_samples(
            now,
            &[("b1".into(), "u1".into(), 10, 20), ("b2".into(), "u1".into(), 1, 2)],
        )
        .unwrap();
        assert_eq!(db.history("u1", 0).unwrap(), vec![(now, 11, 22)]);
        db.add_samples(now, &[("b3".into(), "u2".into(), 100, 200)]).unwrap();
        assert_eq!(db.node_history(0).unwrap(), vec![(now, 111, 222)]);
        assert!(db.node_history(now + 1).unwrap().is_empty());
        db.delete_bot("b1").unwrap();
        assert!(db.load_bots().unwrap().is_empty());
        assert!(db.kv_get("bot:b1:x").is_none());
    }

    #[test]
    fn conv_prune_and_recent() {
        let dir = tempfile::tempdir().unwrap();
        let db = ConvDb::open(&dir.path().join("c.db")).unwrap();
        for i in 0..(KEEP_PER_SESSION + 10) {
            db.append("b", "s", "user", &format!("m{i}")).unwrap();
        }
        let r = db.recent("b", "s", 1000).unwrap();
        assert_eq!(r.len() as i64, KEEP_PER_SESSION);
        assert_eq!(r.last().unwrap().1, format!("m{}", KEEP_PER_SESSION + 9));
        assert!(db.bot_bytes("b") > 0);
        db.delete_bot("b").unwrap();
        assert_eq!(db.bot_bytes("b"), 0);
    }

    #[test]
    fn seen_keeps_the_newest_entries_per_kind() {
        let dir = tempfile::tempdir().unwrap();
        let db = ConvDb::open(&dir.path().join("c.db")).unwrap();
        for i in 0..(KEEP_SEEN + 5) {
            db.touch_seen(
                "b",
                "session",
                &format!("p:FriendMessage:{i}"),
                &format!("p:FriendMessage:{i}"),
            )
            .unwrap();
        }
        db.touch_seen("b", "user", "u1", "p:GroupMessage:g1").unwrap();
        db.touch_seen("b", "user", "u1", "p:FriendMessage:u1").unwrap();
        db.touch_seen("other", "user", "u2", "p:FriendMessage:u2").unwrap();
        let rows = db.seen("b").unwrap();
        let sessions: Vec<&Seen> = rows.iter().filter(|r| r.kind == "session").collect();
        assert_eq!(sessions.len() as i64, KEEP_SEEN);
        assert!(!sessions.iter().any(|r| r.id == "p:FriendMessage:0"));
        let users: Vec<&Seen> = rows.iter().filter(|r| r.kind == "user").collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].origin, "p:FriendMessage:u1");
        db.delete_seen("b").unwrap();
        assert!(db.seen("b").unwrap().is_empty());
        assert_eq!(db.seen("other").unwrap().len(), 1);
    }

    #[test]
    fn conv_sessions_and_delete_session() {
        let dir = tempfile::tempdir().unwrap();
        let db = ConvDb::open(&dir.path().join("c.db")).unwrap();
        db.append("b", "p:GroupMessage:g1", "user", "hi").unwrap();
        db.append("b", "p:GroupMessage:g1", "assistant", "hello").unwrap();
        db.append("b", "p:FriendMessage:u1", "user", "yo").unwrap();
        db.append("other", "p:GroupMessage:g1", "user", "keep").unwrap();
        let sessions = db.sessions("b").unwrap();
        assert_eq!(sessions.len(), 2);
        let g1 = sessions.iter().find(|s| s.0 == "p:GroupMessage:g1").unwrap();
        assert_eq!(g1.1, 2);
        assert_eq!(sessions.iter().map(|s| s.2).sum::<u64>(), db.bot_bytes("b"));
        assert_eq!(db.delete_session("b", "p:GroupMessage:g1").unwrap(), 2);
        assert_eq!(db.sessions("b").unwrap().len(), 1);
        assert_eq!(db.recent("other", "p:GroupMessage:g1", 10).unwrap().len(), 1);
    }
}
