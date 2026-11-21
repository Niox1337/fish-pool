//! Signed HTTP API against a node bound to a local port.

use std::sync::Arc;

use fish_node::config::Config;
use fish_node::node::Node;
use fish_node::plugin::SidecarCommand;
use fish_node::sign::{now_ms, sign};
use serde_json::{json, Value};

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

fn config(dir: &std::path::Path) -> Config {
    Config {
        node_id: "node-t".into(),
        secret: KEY.to_vec(),
        secret_text: "unused".into(),
        console_url: None,
        listen: "127.0.0.1:0".into(),
        data_dir: dir.to_path_buf(),
        sidecar_python: "python3".into(),
        sidecar_dir: dir.to_path_buf(),
        uv: "uv".into(),
        dev: true,
        dev_services: false,
        plugin_uid: None,
        plugin_gid: None,
        plugin_idle_sleep: None,
    }
}

fn dummy_sidecar() -> SidecarCommand {
    SidecarCommand {
        program: "false".into(),
        args: vec![],
        envs: vec![],
        cwd: None,
        uid: None,
        gid: None,
    }
}

async fn serve(node: Arc<Node>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, fish_node::api::router(node)).await.unwrap() });
    format!("http://{addr}")
}

async fn call(base: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default();
    let ts = now_ms();
    let sig = sign(KEY, ts, method, path, &bytes);
    let resp = reqwest::Client::new()
        .request(method.parse().unwrap(), format!("{base}{path}"))
        .header("X-Fish-Node", "node-t")
        .header("X-Fish-Ts", ts.to_string())
        .header("X-Fish-Sig", sig)
        .header("content-type", "application/json")
        .body(bytes)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn signed_api_lifecycle() {
    let dir = tempfile::Builder::new().prefix("fa").tempdir_in("/tmp").unwrap();
    let node = Node::new(config(dir.path()), Some(dummy_sidecar())).unwrap();
    let base = serve(node.clone()).await;

    let unsigned = reqwest::get(format!("{base}/v1/health")).await.unwrap();
    assert_eq!(unsigned.status().as_u16(), 401);
    let body: Value = unsigned.json().await.unwrap();
    assert_eq!(body["error"]["code"], "signature_missing");

    let ts = now_ms() - 120_000;
    let stale = reqwest::Client::new()
        .get(format!("{base}/v1/health"))
        .header("X-Fish-Node", "node-t")
        .header("X-Fish-Ts", ts.to_string())
        .header("X-Fish-Sig", sign(KEY, ts, "GET", "/v1/health", b""))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status().as_u16(), 401);

    let (s, health) = call(&base, "GET", "/v1/health", None).await;
    assert_eq!(s, 200);
    assert_eq!(health["node_id"], "node-t");
    assert_eq!(health["bots"], 0);

    let desired = json!({
        "bot_id": "b1", "user_id": "u1", "name": "小鱼", "running": true,
        "quota": {"memory_mb": 128, "storage_mb": 512},
        "settings": {"persona": {"name": "小鱼", "system_prompt": "你好", "greeting": ""}},
        "llm": null, "platforms": [], "plugins": []
    });
    let (s, r) = call(&base, "PUT", "/v1/bots/b1", Some(desired.clone())).await;
    assert_eq!((s, r), (200, json!({"ok": true})));
    let (s, _) = call(&base, "PUT", "/v1/bots/b1", Some(desired.clone())).await;
    assert_eq!(s, 200);

    let (s, mismatch) = call(&base, "PUT", "/v1/bots/b2", Some(desired.clone())).await;
    assert_eq!(s, 400);
    assert_eq!(mismatch["error"]["code"], "invalid_body");

    let (_, status) = call(&base, "GET", "/v1/bots/b1/status", None).await;
    assert_eq!(status["status"], "running");
    assert_eq!(status["platforms"], json!([]));

    let (_, logs) = call(&base, "GET", "/v1/bots/b1/logs", None).await;
    let items = logs["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["source"] == "core"));
    let cursor = logs["cursor"].as_str().unwrap().to_string();
    let (_, later) = call(&base, "GET", &format!("/v1/bots/b1/logs?after={cursor}"), None).await;
    assert_eq!(later["items"], json!([]));

    let (_, usage) = call(&base, "GET", "/v1/usage?user_id=u1", None).await;
    assert_eq!(usage["quota"]["memory_mb"], 128);
    let kinds: Vec<&str> = usage["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["core", "conversations"]);
    assert!(usage["memory_bytes"].as_u64().unwrap() > 0);

    let mut other = desired.clone();
    other["bot_id"] = json!("b2");
    other["user_id"] = json!("u2");
    let (s, _) = call(&base, "PUT", "/v1/bots/b2", Some(other)).await;
    assert_eq!(s, 200);
    node.usage.sample(&node).await;
    let (s, summary) = call(&base, "GET", "/v1/usage/summary", None).await;
    assert_eq!(s, 200);
    let users = summary["users"].as_array().unwrap();
    let ids: Vec<&str> = users.iter().map(|u| u["user_id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["u1", "u2"]);
    let mem_sum: u64 = users.iter().map(|u| u["memory_bytes"].as_u64().unwrap()).sum();
    let sto_sum: u64 = users.iter().map(|u| u["storage_bytes"].as_u64().unwrap()).sum();
    assert!(mem_sum > 0);
    assert_eq!(summary["memory_bytes"].as_u64().unwrap(), mem_sum);
    assert_eq!(summary["storage_bytes"].as_u64().unwrap(), sto_sum);
    let history = summary["history"].as_array().unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0]["ts"].as_i64().unwrap() > 0);
    assert!(history[0]["memory_bytes"].as_u64().unwrap() > 0);
    let (s, _) = call(&base, "DELETE", "/v1/bots/b2", None).await;
    assert_eq!(s, 200);
    let (_, summary) = call(&base, "GET", "/v1/usage/summary", None).await;
    assert_eq!(summary["users"].as_array().unwrap().len(), 1);

    let mut stopped = desired.clone();
    stopped["running"] = json!(false);
    call(&base, "PUT", "/v1/bots/b1", Some(stopped)).await;
    let (_, status) = call(&base, "GET", "/v1/bots/b1/status", None).await;
    assert_eq!(status["status"], "stopped");

    let (s, _) = call(&base, "POST", "/v1/bots/b1/restart", None).await;
    assert_eq!(s, 200);

    let (s, qr) = call(&base, "GET", "/v1/qr/qq/unknown", None).await;
    assert_eq!(s, 404);
    assert_eq!(qr["error"]["code"], "qr_task_not_found");

    // a fresh node on the same data dir restores the saved desired state
    let node2 = Node::new(config(dir.path()), Some(dummy_sidecar())).unwrap();
    assert_eq!(node2.restore().await.unwrap(), 1);
    assert_eq!(node2.get("b1").unwrap().user_id(), "u1");

    let (s, _) = call(&base, "DELETE", "/v1/bots/b1", None).await;
    assert_eq!(s, 200);
    let (s, gone) = call(&base, "GET", "/v1/bots/b1/status", None).await;
    assert_eq!(s, 404);
    assert_eq!(gone["error"]["code"], "bot_not_found");
}

#[tokio::test]
async fn loopback_requires_explicit_fish_dev_and_running_bot() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    assert!(cfg.dev);
    assert!(!cfg.dev_services);
    let node = Node::new(cfg.clone(), Some(dummy_sidecar())).unwrap();
    let base = serve(node.clone()).await;
    let input = json!({"text": "hi", "session_id": "u", "message_id": "m"});
    for (method, suffix) in [("POST", "inject"), ("GET", "outbox")] {
        let (status, _) = call(&base, method, &format!("/v1/dev/bots/b/{suffix}"), None).await;
        assert_eq!(status, 404);
    }
    let desired = json!({"bot_id": "b", "user_id": "u", "running": true});
    node.put(serde_json::from_value(desired.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        node.get("b")
            .unwrap()
            .inject_loopback(serde_json::from_value(input.clone()).unwrap())
            .await,
        Err("FISH_DEV is required".into())
    );
    node.shutdown().await;

    let mut enabled = cfg;
    enabled.dev_services = true;
    let node = Node::new(enabled, Some(dummy_sidecar())).unwrap();
    let base = serve(node.clone()).await;
    node.put(serde_json::from_value(desired.clone()).unwrap())
        .await
        .unwrap();
    let path = "/v1/dev/bots/b/inject";
    let unsigned = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(unsigned.status().as_u16(), 401);
    let (status, _) = call(&base, "POST", path, Some(input.clone())).await;
    assert_eq!(status, 200);
    let (status, outbox) = call(&base, "GET", "/v1/dev/bots/b/outbox", None).await;
    assert_eq!(status, 200);
    let notice = fish_node::llm::Trouble::NoKey.notice();
    assert_eq!(outbox["items"].as_array().unwrap().len(), 1, "{outbox}");
    assert_eq!(outbox["items"][0]["chain"][0]["text"], notice, "{outbox}");
    let (status, _) = call(&base, "POST", path, Some(json!({"text": "hi again", "session_id": "u", "message_id": "m2"}))).await;
    assert_eq!(status, 200);
    let (_, outbox) = call(&base, "GET", "/v1/dev/bots/b/outbox", None).await;
    assert_eq!(outbox["items"].as_array().unwrap().len(), 1, "the notice repeats only after its cooldown: {outbox}");
    let (status, _) = call(
        &base,
        "POST",
        path,
        Some(json!({"text": "hi", "session_id": "", "message_id": "m"})),
    )
    .await;
    assert_eq!(status, 400);
    let mut stopped = desired;
    stopped["running"] = json!(false);
    node.put(serde_json::from_value(stopped).unwrap()).await.unwrap();
    let (status, _) = call(&base, "POST", path, Some(input)).await;
    assert_eq!(status, 400);
    node.shutdown().await;
}
