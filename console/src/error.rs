use serde_json::json;
use std::fmt;

/// API error rendered as `{"error": {"code", "message"}}` with an HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, "invalid_input", message)
    }
    pub fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "请先登录")
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(403, "forbidden", message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "not_found", message)
    }
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(409, code, message)
    }
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(429, "rate_limited", message)
    }
    pub fn quota(message: impl Into<String>) -> Self {
        Self::new(403, "quota_exceeded", message)
    }
    pub fn node_unreachable() -> Self {
        Self::new(502, "node_unreachable", "运行节点暂时无法连接，请稍后再试")
    }
    pub fn internal(detail: impl fmt::Display) -> Self {
        crate::util::log_error(&format!("internal error: {detail}"));
        Self::new(500, "internal", "服务器内部错误，请稍后再试")
    }

    pub fn body(&self) -> serde_json::Value {
        json!({"error": {"code": self.code, "message": self.message}})
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.message)
    }
}

impl From<worker::Error> for ApiError {
    fn from(e: worker::Error) -> Self {
        ApiError::internal(e)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(e)
    }
}
