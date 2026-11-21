//! Signed calls from the console to a node agent.

use crate::crypto::sign_request;
use crate::http;
use crate::util::now_ms;
use serde_json::Value;
use worker::Method;

pub struct NodeClient {
    pub id: String,
    pub base_url: String,
    pub secret: [u8; 32],
}

#[derive(Debug)]
pub enum NodeError {
    /// Network failure or timeout.
    Unreachable(String),
    /// The node answered with a non 2xx status.
    Status(u16, String),
}

impl NodeError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, NodeError::Status(404, _))
    }
    /// Human readable reason for status_detail fields.
    pub fn detail(&self) -> String {
        match self {
            NodeError::Unreachable(_) => "运行节点暂时无法连接，设置已保存，节点恢复后会自动同步".into(),
            NodeError::Status(code, msg) => format!(
                "运行节点返回错误（{code}）：{}",
                msg.chars().take(200).collect::<String>()
            ),
        }
    }
}

/// Extracts the path and query of `url`, which is the part covered by the signature.
pub fn signed_path(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(match u.query() {
        Some(q) => format!("{}?{}", u.path(), q),
        None => u.path().to_string(),
    })
}

fn error_message(resp: &http::HttpResponse) -> String {
    resp.json()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| resp.text().chars().take(200).collect())
}

impl NodeClient {
    pub fn url(&self, path_with_query: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path_with_query)
    }

    pub async fn call(
        &self,
        method: Method,
        path_with_query: &str,
        body: Option<&Value>,
        timeout_ms: u64,
    ) -> Result<Value, NodeError> {
        let url = self.url(path_with_query);
        let signed = signed_path(&url).ok_or_else(|| NodeError::Unreachable(format!("bad node url {url}")))?;
        let bytes = body
            .map(|b| serde_json::to_vec(b).unwrap_or_default())
            .unwrap_or_default();
        let ts = now_ms();
        let sig = sign_request(&self.secret, ts, method.as_ref(), &signed, &bytes);
        let ts_s = ts.to_string();
        let headers = [
            ("content-type", "application/json"),
            ("user-agent", "fish-console"),
            ("x-fish-node", self.id.as_str()),
            ("x-fish-ts", ts_s.as_str()),
            ("x-fish-sig", sig.as_str()),
        ];
        let send_body = if body.is_some() { Some(bytes.as_slice()) } else { None };
        let resp = http::send(method, &url, &headers, send_body, timeout_ms)
            .await
            .map_err(NodeError::Unreachable)?;
        if !resp.ok() {
            return Err(NodeError::Status(resp.status, error_message(&resp)));
        }
        if resp.body.is_empty() {
            return Ok(Value::Null);
        }
        resp.json()
            .ok_or_else(|| NodeError::Status(resp.status, "节点返回的不是 JSON".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_path_includes_query_and_base_prefix() {
        assert_eq!(signed_path("https://n.example/v1/health").unwrap(), "/v1/health");
        assert_eq!(
            signed_path("https://n.example/v1/usage?user_id=u1").unwrap(),
            "/v1/usage?user_id=u1"
        );
        assert_eq!(
            signed_path("http://127.0.0.1:8701/prefix/v1/x?a=1&b=2").unwrap(),
            "/prefix/v1/x?a=1&b=2"
        );
        let c = NodeClient {
            id: "node-1".into(),
            base_url: "http://h:1/".into(),
            secret: [0; 32],
        };
        assert_eq!(c.url("/v1/health"), "http://h:1/v1/health");
    }
}
