use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::sign::{now_ms, SignedClient};

pub const EVENTS_PATH: &str = "/api/internal/node/events";

/// Best effort status change notifications to the console.
#[derive(Clone)]
pub struct Events {
    tx: Option<mpsc::UnboundedSender<Value>>,
}

impl Events {
    pub fn disabled() -> Self {
        Events { tx: None }
    }

    pub fn start(client: Option<SignedClient>) -> Self {
        let Some(client) = client else {
            return Events::disabled();
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            loop {
                let Some(first) = rx.recv().await else { return };
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut batch = vec![first];
                while let Ok(e) = rx.try_recv() {
                    if batch.len() >= 200 {
                        break;
                    }
                    batch.push(e);
                }
                let body = json!({"node_id": client.node_id, "events": batch});
                for attempt in 0..2 {
                    match client.post_json(EVENTS_PATH, &body).await {
                        Ok(r) if r.status().is_success() => break,
                        Ok(r) => tracing::debug!("console events returned {}", r.status()),
                        Err(e) => tracing::debug!("console events failed: {e}"),
                    }
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
        Events { tx: Some(tx) }
    }

    pub fn push(&self, bot_id: &str, kind: &str, id: &str, status: &str, detail: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(json!({
                "bot_id": bot_id, "kind": kind, "id": id, "status": status, "status_detail": detail, "ts": now_ms(),
            }));
        }
    }
}
