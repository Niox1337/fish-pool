use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};

use super::Inbound;
use crate::model::{Component, MessageType, Sender};

pub const ID: &str = "dev-loopback";
pub const KIND: &str = "dev_loopback";
const CAPACITY: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inject {
    pub text: String,
    pub session_id: String,
    pub message_id: String,
}

// shortcut: loopback accepts private text messages only, extend when local group or media tests are needed.
impl Inject {
    pub fn inbound(self) -> Inbound {
        Inbound {
            platform_id: ID.into(),
            platform_kind: KIND,
            self_id: ID.into(),
            message_type: MessageType::Private,
            group_id: None,
            sender: Sender {
                user_id: self.session_id.clone(),
                nickname: self.session_id.clone(),
            },
            session_id: self.session_id,
            chain: vec![Component::plain(&self.text)],
            text: self.text,
            is_at: false,
            msg_id: self.message_id,
            raw: Value::Null,
        }
    }
}

#[derive(Default)]
pub struct Loopback {
    outbox: Mutex<VecDeque<Value>>,
}

impl Loopback {
    pub fn send(&self, mt: MessageType, session_id: &str, chain: &[Component], reply_to: Option<&str>) {
        let mut outbox = self.outbox.lock().unwrap();
        if outbox.len() == CAPACITY {
            outbox.pop_front();
        }
        outbox.push_back(json!({
            "message_type": mt, "session_id": session_id, "chain": chain, "reply_to": reply_to,
        }));
    }

    pub fn outbox(&self) -> Vec<Value> {
        self.outbox.lock().unwrap().iter().cloned().collect()
    }

    pub fn approx_bytes(&self) -> usize {
        self.outbox.lock().unwrap().iter().map(|v| v.to_string().len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_inbound_and_bounded_outbox() {
        let inbound = Inject {
            text: "/echo hi".into(),
            session_id: "u1".into(),
            message_id: "m1".into(),
        }
        .inbound();
        assert_eq!(inbound.platform_kind, KIND);
        assert_eq!(inbound.message_type, MessageType::Private);
        assert_eq!(inbound.sender.user_id, "u1");
        let conn = Loopback::default();
        for i in 0..=CAPACITY {
            conn.send(
                MessageType::Private,
                "u1",
                &[Component::plain(i.to_string())],
                Some("m1"),
            );
        }
        let outbox = conn.outbox();
        assert_eq!(outbox.len(), CAPACITY);
        assert_eq!(outbox[0]["chain"][0]["text"], "1");
        assert_eq!(outbox[0]["reply_to"], "m1");
    }
}
