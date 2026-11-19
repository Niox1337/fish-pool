//! Outbound HTTP with timeouts.

use futures_util::future::{select, Either};
use std::time::Duration;
use wasm_bindgen::JsValue;
use worker::{AbortController, Delay, Fetch, Headers, Method, Request, RequestInit};

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Sends a request and reads the whole body, failing with `"timeout"` after `timeout_ms`.
pub async fn send(
    method: Method,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout_ms: u64,
) -> Result<HttpResponse, String> {
    let h = Headers::new();
    for (k, v) in headers {
        h.set(k, v).map_err(|e| e.to_string())?;
    }
    let mut init = RequestInit::new();
    init.with_method(method).with_headers(h);
    if let Some(b) = body {
        init.with_body(Some(JsValue::from(js_sys::Uint8Array::from(b))));
    }
    let req = Request::new_with_init(url, &init).map_err(|e| e.to_string())?;
    let ctrl = AbortController::default();
    let signal = ctrl.signal();
    let work = async move {
        let mut resp = Fetch::Request(req).send_with_signal(&signal).await.map_err(|e| e.to_string())?;
        let status = resp.status_code();
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(HttpResponse { status, body })
    };
    let delay = Delay::from(Duration::from_millis(timeout_ms));
    futures_util::pin_mut!(work);
    match select(work, delay).await {
        Either::Left((r, _)) => r,
        Either::Right(_) => {
            ctrl.abort();
            Err("timeout".into())
        }
    }
}
