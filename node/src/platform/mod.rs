pub mod loopback;
pub mod qq;
pub mod weixin;

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use crate::model::{Component, MessageType, Sender};

/// Normalized inbound message produced by a platform adapter.
#[derive(Clone, Debug)]
pub struct Inbound {
    pub platform_id: String,
    pub platform_kind: &'static str,
    pub self_id: String,
    pub message_type: MessageType,
    pub group_id: Option<String>,
    pub session_id: String,
    pub sender: Sender,
    pub text: String,
    pub chain: Vec<Component>,
    pub is_at: bool,
    pub msg_id: String,
    pub raw: Value,
}

/// Callback used by adapters to report `connected`, `connecting`, `error` or `disabled`.
pub type StatusSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Send handle of a running platform connection.
#[derive(Clone)]
pub enum PlatformConn {
    Loopback(Arc<loopback::Loopback>),
    Qq(Arc<qq::QqConn>),
    Weixin(Arc<weixin::WxConn>),
}

impl PlatformConn {
    pub async fn send(
        &self,
        mt: MessageType,
        session_id: &str,
        chain: &[Component],
        reply_to: Option<&str>,
    ) -> Result<()> {
        match self {
            PlatformConn::Loopback(c) => {
                c.send(mt, session_id, chain, reply_to);
                Ok(())
            }
            PlatformConn::Qq(c) => c.send(mt, session_id, chain, reply_to).await,
            PlatformConn::Weixin(c) => c.send(session_id, chain).await,
        }
    }

    pub fn approx_bytes(&self) -> usize {
        match self {
            PlatformConn::Loopback(c) => c.approx_bytes(),
            PlatformConn::Qq(c) => c.approx_bytes(),
            PlatformConn::Weixin(c) => c.approx_bytes(),
        }
    }
}

/// Exponential backoff helper capped at `max_s` seconds.
pub fn backoff_secs(attempt: u32, max_s: u64) -> u64 {
    let base = 1u64 << attempt.min(16);
    base.min(max_s)
}
