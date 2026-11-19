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

impl ConvDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY AUTOINCREMENT, bot_id TEXT NOT NULL, session TEXT NOT NULL,
                role TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS messages_bot_session ON messages(bot_id, session, id);",
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
}
