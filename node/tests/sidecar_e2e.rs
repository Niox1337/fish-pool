//! End to end tests against the real Python fork server in `../sidecar`.
//!
//! They need `../sidecar/.venv/bin/python` with the sidecar dependencies and skip with a message
//! when it is missing. No network access and no uv are used.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fish_node::config::Config;
use fish_node::model::{Behavior, Component, MessageEvent, MessageType, Sender};
use fish_node::node::Node;
use fish_node::pipeline::wake;
use fish_node::plugin::{launch, LaunchSpec, LoadError, OutMsg, PluginOut, Sidecar, SidecarCommand};
use serde_json::{json, Value};
use tokio::sync::mpsc;

fn sidecar_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sidecar")
        .canonicalize()
        .unwrap()
}

fn sidecar_python() -> Option<PathBuf> {
    let p = sidecar_dir().join(".venv/bin/python");
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping: {} not found", p.display());
        None
    }
}

fn command(python: &Path) -> SidecarCommand {
    let dir = sidecar_dir();
    SidecarCommand {
        program: python.display().to_string(),
        args: vec!["-m".into(), "fish_sidecar.server".into()],
        envs: vec![
            ("PYTHONPATH".into(), dir.display().to_string()),
            ("PYTHONUNBUFFERED".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("OBJC_DISABLE_INITIALIZE_FORK_SAFETY".into(), "YES".into()),
        ],
        cwd: Some(dir),
        uid: None,
        gid: None,
    }
}

fn sample_plugin(name: &str) -> PathBuf {
    sidecar_dir().join("tests/plugins").join(name)
}

fn event(platform: &str, origin: &str, text: &str) -> MessageEvent {
    MessageEvent {
        platform: platform.into(),
        bot_id: "b1".into(),
        self_id: "me".into(),
        message_type: MessageType::Private,
        group_id: None,
        session_id: "u1".into(),
        message_id: "m1".into(),
        unified_msg_origin: origin.into(),
        sender: Sender {
            user_id: "u1".into(),
            nickname: "小明".into(),
        },
        message_str: text.into(),
        chain: vec![Component::plain(text)],
        is_at_or_wake: true,
        is_admin: false,
        timestamp: 0,
        raw: None,
    }
}

async fn next_send(rx: &mut mpsc::Receiver<OutMsg>) -> (Option<String>, Vec<Component>) {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("plugin output")
            .expect("channel open");
        if let PluginOut::Send { session, chain, .. } = m.msg {
            return (session, chain);
        }
    }
}

fn spec(dir: &Path, proc_id: &str, plugin: &str) -> LaunchSpec {
    LaunchSpec {
        proc_id: proc_id.into(),
        plugin_id: format!("pl-{plugin}"),
        plugin_name: plugin.into(),
        bot_id: "b1".into(),
        plugin_dir: sample_plugin(plugin),
        venv_site_packages: None,
        data_dir: dir.join(proc_id),
        config: json!({}),
    }
}

#[tokio::test]
async fn real_sidecar_commands_wake_prefix_and_load_failure() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fe").tempdir_in("/tmp").unwrap();
    let sidecar = Sidecar::new(command(&python), dir.path().join("run"));
    let (out_tx, mut out_rx) = mpsc::channel(64);

    let (proc, _exit) = launch(&sidecar, spec(dir.path(), "p1", "cmd_plugin"), out_tx.clone())
        .await
        .expect("launch cmd_plugin");
    assert_eq!(proc.hello.name, "cmd_plugin");
    assert_eq!(proc.hello.version, "1.2.3");

    // the node strips the "/" wake prefix and the sidecar sees a plain command
    let w = wake(&Behavior::default(), MessageType::Private, false, "/echo hi");
    assert!(w.woke);
    assert_eq!(w.text, "echo hi");
    let (m, d) = proc.dispatch("e1", &event("qq_official", "p1:FriendMessage:u1", &w.text));
    assert!(tokio::time::timeout(Duration::from_secs(10), m).await.unwrap().unwrap());
    let (session, chain) = next_send(&mut out_rx).await;
    assert_eq!(session.as_deref(), Some("p1:FriendMessage:u1"));
    assert_eq!(chain, vec![Component::plain("echo:hi")]);
    let (_, call_llm) = tokio::time::timeout(Duration::from_secs(10), d).await.unwrap().unwrap();
    assert!(!call_llm);

    // weixin_ilink is shown to the plugin as weixin_oc while the session origin is passed through unchanged
    let (m, _d) = proc.dispatch("e2", &event("weixin_ilink", "wx9:FriendMessage:u1", "whoami"));
    assert!(tokio::time::timeout(Duration::from_secs(10), m).await.unwrap().unwrap());
    let (session, chain) = next_send(&mut out_rx).await;
    assert_eq!(session.as_deref(), Some("wx9:FriendMessage:u1"));
    let text = match &chain[..] {
        [Component::Plain { text }] => text.clone(),
        other => panic!("unexpected chain {other:?}"),
    };
    let parts: Vec<&str> = text.split('|').collect();
    assert_eq!(parts[2], "weixin_oc");
    assert_eq!(parts[5], "wx9:FriendMessage:u1");
    assert!(fish_node::model::parse_origin(parts[5]).is_some());

    proc.shutdown();

    // an import error exits with code 2 and surfaces as a LoadError holding the error log line
    let err = match launch(&sidecar, spec(dir.path(), "p2", "broken_plugin"), out_tx).await {
        Ok(_) => panic!("broken_plugin must not load"),
        Err(e) => e,
    };
    let load = err.downcast_ref::<LoadError>().expect("load error");
    assert!(load.0.contains("NotImplementedError"), "{}", load.0);
    assert!(load.0.contains("astrbot.core.pipeline"), "{}", load.0);

    sidecar.shutdown().await;
}

fn node_config(dir: &Path) -> Config {
    Config {
        node_id: "node-e2e".into(),
        secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        secret_text: "unused".into(),
        console_url: None,
        listen: "127.0.0.1:0".into(),
        data_dir: dir.to_path_buf(),
        sidecar_python: "python3".into(),
        sidecar_dir: sidecar_dir(),
        uv: "uv".into(),
        dev: true,
        dev_services: false,
        plugin_uid: None,
        plugin_gid: None,
    }
}

/// Places a sample plugin where the installer would have put it so no download or uv run happens.
fn preinstall(cfg: &Config, plugin: &Value) {
    let name = plugin["name"].as_str().unwrap();
    let pdir = cfg.plugin_dir("u1", "b1", name);
    let src = pdir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(pdir.join("venv")).unwrap();
    std::fs::create_dir_all(pdir.join("data")).unwrap();
    for entry in std::fs::read_dir(sample_plugin(name)).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), src.join(entry.file_name())).unwrap();
        }
    }
    let marker = json!({"repo_url": plugin["repo_url"], "version": plugin["version"], "name": name});
    std::fs::write(pdir.join(".fish_install.json"), marker.to_string()).unwrap();
}

fn plugin_status(node: &Arc<Node>, id: &str) -> Value {
    let status = node.get("b1").unwrap().status_json();
    status["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn real_sidecar_load_failure_sets_status_without_restart_loop() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fn").tempdir_in("/tmp").unwrap();
    let cfg = node_config(dir.path());
    let broken = json!({"id": "pb", "name": "broken_plugin", "repo_url": "https://github.com/x/broken", "version": "1", "enabled": true, "config": {}});
    let good = json!({"id": "pc", "name": "cmd_plugin", "repo_url": "https://github.com/x/cmd", "version": "1", "enabled": true, "config": {}});
    preinstall(&cfg, &broken);
    preinstall(&cfg, &good);
    let node = Node::new(cfg, Some(command(&python))).unwrap();
    let desired = serde_json::from_value(json!({
        "bot_id": "b1", "user_id": "u1", "name": "小鱼", "running": true,
        "platforms": [], "plugins": [broken, good]
    }))
    .unwrap();
    node.put(desired).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (b, g) = (plugin_status(&node, "pb"), plugin_status(&node, "pc"));
        if b["status"] == "error" && g["status"] == "running" {
            assert!(
                b["status_detail"].as_str().unwrap().contains("NotImplementedError"),
                "{b}"
            );
            assert_eq!(g["version"], "1.2.3");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "plugins did not settle: {b} {g}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // the failed plugin stays in error and is not relaunched
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(plugin_status(&node, "pb")["status"], "error");
    let (items, _) = node.get("b1").unwrap().logs.read_after(None, 1000);
    let launches = items
        .iter()
        .filter(|i| i.source == "plugin:broken_plugin" && i.message.contains("plugin load failed"))
        .count();
    assert_eq!(launches, 1);

    node.shutdown().await;
}
