//! Plugin archive proxy. Nodes download GitHub archives through the Worker.
//!
//! `GET /api/internal/plugin-archive/{owner}/{repo}/zip/{ref...}` signed by a node
//! streams `https://codeload.github.com/{owner}/{repo}/zip/{ref...}`.

use super::internal::node_auth;
use crate::app::App;
use crate::error::{ApiError, ApiResult};
use futures_util::future::{select, Either};
use std::time::Duration;
use worker::{AbortController, Delay, Fetch, Headers, Method, Request, RequestInit, Response};

pub const PREFIX: &str = "/api/internal/plugin-archive/";
/// Same limit as the node installer.
pub const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const HEADERS_TIMEOUT_MS: u64 = 20_000;

fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.len() <= 100
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Maps the path after `PREFIX` to the codeload URL. Rejects anything that is not a repository archive.
pub fn upstream_url(rest: &str) -> Option<String> {
    let segs: Vec<&str> = rest.split('/').collect();
    let [owner, repo, "zip", reference @ ..] = segs.as_slice() else {
        return None;
    };
    if !safe_segment(owner) || !safe_segment(repo) || reference.is_empty() || reference.len() > 8 {
        return None;
    }
    if !reference.iter().all(|s| safe_segment(s)) {
        return None;
    }
    Some(format!(
        "https://codeload.github.com/{owner}/{repo}/zip/{}",
        reference.join("/")
    ))
}

fn error(e: ApiError) -> worker::Result<Response> {
    Ok(Response::from_json(&e.body())?.with_status(e.status))
}

pub async fn proxy(req: Request, app: &App) -> worker::Result<Response> {
    match run(req, app).await {
        Ok(resp) => Ok(resp),
        Err(e) => error(e),
    }
}

async fn run(req: Request, app: &App) -> ApiResult<Response> {
    if req.method() != Method::Get {
        return Err(ApiError::new(405, "method_not_allowed", "只支持 GET"));
    }
    node_auth(&req, app, "GET", b"").await?;
    let path = req.path();
    let url = path
        .strip_prefix(PREFIX)
        .and_then(upstream_url)
        .ok_or_else(|| ApiError::bad_request("不是有效的插件压缩包地址"))?;

    let headers = Headers::new();
    headers
        .set("user-agent", "fish-console")
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let upstream_req = Request::new_with_init(&url, &init).map_err(|e| ApiError::internal(e.to_string()))?;
    let ctrl = AbortController::default();
    let signal = ctrl.signal();
    let fetch = Fetch::Request(upstream_req);
    let send = fetch.send_with_signal(&signal);
    futures_util::pin_mut!(send);
    let upstream = match select(send, Delay::from(Duration::from_millis(HEADERS_TIMEOUT_MS))).await {
        Either::Left((Ok(r), _)) => r,
        Either::Left((Err(e), _)) => {
            return Err(ApiError::new(502, "upstream_failed", format!("GitHub 请求失败：{e}")))
        }
        Either::Right(_) => {
            ctrl.abort();
            return Err(ApiError::new(504, "upstream_timeout", "GitHub 响应超时"));
        }
    };
    let status = upstream.status_code();
    if !(200..300).contains(&status) {
        let code = if status == 404 { 404 } else { 502 };
        return Err(ApiError::new(code, "upstream_status", format!("GitHub 返回 {status}")));
    }
    let length = upstream
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok());
    if length.is_some_and(|l| l > MAX_ARCHIVE_BYTES) {
        return Err(ApiError::new(413, "archive_too_large", "插件压缩包过大"));
    }
    let out = Headers::new();
    let set = |k: &str, v: &str| out.set(k, v).map_err(|e| ApiError::internal(e.to_string()));
    set("content-type", "application/zip")?;
    set("cache-control", "no-store")?;
    if let Some(l) = length {
        set("content-length", &l.to_string())?;
    }
    Ok(upstream.with_headers(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_archive_paths() {
        assert_eq!(
            upstream_url("Soulter/helloworld/zip/refs/heads/master").as_deref(),
            Some("https://codeload.github.com/Soulter/helloworld/zip/refs/heads/master")
        );
        assert_eq!(
            upstream_url("a/b.c/zip/HEAD").as_deref(),
            Some("https://codeload.github.com/a/b.c/zip/HEAD")
        );
        assert_eq!(
            upstream_url("a/b/zip/v1.2.0").as_deref(),
            Some("https://codeload.github.com/a/b/zip/v1.2.0")
        );
        assert!(upstream_url("a/b/tar.gz/HEAD").is_none());
        assert!(upstream_url("a/b/zip").is_none());
        assert!(upstream_url("a/b/zip/").is_none());
        assert!(upstream_url("../b/zip/HEAD").is_none());
        assert!(upstream_url("a/b/zip/refs/../../x").is_none());
        assert!(upstream_url("a/b/zip/x%2Fy").is_none());
        assert!(upstream_url("a/b/zip/x?y").is_none());
    }
}
