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
        share_key: Some(format!("{plugin}-test")),
    }
}

#[tokio::test]
async fn real_sidecar_commands_wake_prefix_and_load_failure() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fe").tempdir_in("/tmp").unwrap();
    let sidecar = Sidecar::new(command(&python), dir.path().join("run"));
    let (out_tx, mut out_rx) = mpsc::channel(64);

    let (proc, _exit) = launch(&sidecar, spec(dir.path(), "p1", "cmd_plugin"), out_tx.clone(), None)
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
    let err = match launch(&sidecar, spec(dir.path(), "p2", "broken_plugin"), out_tx, None).await {
        Ok(_) => panic!("broken_plugin must not load"),
        Err(e) => e,
    };
    let load = err.downcast_ref::<LoadError>().expect("load error");
    assert!(load.0.contains("ImportError") && load.0.contains("astrbot.core.pipeline"), "{}", load.0);
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
        plugin_idle_sleep: None,
        plugin_groups: false,
    }
}

/// Places a sample plugin where the installer would have put it so no download or uv run happens.
fn preinstall(cfg: &Config, plugin: &Value) {
    let name = plugin["name"].as_str().unwrap();
    let pdir = cfg.plugin_dir("u1", "b1", name);
    let key = format!("{name}-test");
    let entry = cfg.shared_plugins_dir().join(&key);
    let src = entry.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(entry.join("venv")).unwrap();
    std::fs::write(entry.join(".fish_shared.json"), "{}").unwrap();
    std::fs::create_dir_all(pdir.join("data")).unwrap();
    for entry in std::fs::read_dir(sample_plugin(name)).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), src.join(entry.file_name())).unwrap();
        }
    }
    let python = fish_node::plugin::install::python_version(&cfg.sidecar_python);
    let marker = json!({"repo_url": plugin["repo_url"], "version": plugin["version"], "name": name, "python": python, "shared": key});
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
                b["status_detail"].as_str().unwrap().contains("ImportError"),
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

async fn wait_status(node: &Arc<Node>, id: &str, wanted: &[&str], secs: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let s = plugin_status(node, id);
        if s["status"].as_str().is_some_and(|status| wanted.contains(&status)) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{id} did not reach {wanted:?}: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn say(node: &Arc<Node>, text: &str, n: usize) -> String {
    let bot = node.get("b1").unwrap();
    bot.inject_loopback(fish_node::platform::loopback::Inject {
        text: text.into(),
        session_id: "s1".into(),
        message_id: format!("m{n}"),
    })
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let outbox = bot.loopback_outbox();
        if outbox.len() >= n {
            return outbox[n - 1].to_string();
        }
        assert!(tokio::time::Instant::now() < deadline, "no reply to {text}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn real_sidecar_idle_plugins_sleep_and_wake_on_message() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fn").tempdir_in("/tmp").unwrap();
    let mut cfg = node_config(dir.path());
    cfg.dev_services = true;
    cfg.plugin_idle_sleep = Some(Duration::from_secs(2));
    let quiet = json!({"id": "pc", "name": "cmd_plugin", "repo_url": "https://github.com/x/cmd", "version": "1", "enabled": true, "config": {}});
    let bg = json!({"id": "pg", "name": "bg_plugin", "repo_url": "https://github.com/x/bg", "version": "1", "enabled": true, "config": {}});
    preinstall(&cfg, &quiet);
    preinstall(&cfg, &bg);
    let node = Node::new(cfg, Some(command(&python))).unwrap();
    let desired = serde_json::from_value(json!({
        "bot_id": "b1", "user_id": "u1", "name": "小鱼", "running": true,
        "platforms": [], "plugins": [quiet, bg]
    }))
    .unwrap();
    node.put(desired).await.unwrap();

    wait_status(&node, "pc", &["sleeping"], 30).await;
    wait_status(&node, "pg", &["sleeping"], 30).await;
    let bot = node.get("b1").unwrap();
    assert!(bot.procs.read().unwrap().is_empty());

    // a message wakes every sleeping plugin before it is dispatched
    let reply = say(&node, "echo hi", 1).await;
    assert!(reply.contains("echo:hi"), "{reply}");
    assert_eq!(plugin_status(&node, "pc")["status"], "running");
    let reply = say(&node, "bgstart", 2).await;
    assert!(reply.contains("started"), "{reply}");

    // the plugin with a background task keeps running while the other one sleeps again
    wait_status(&node, "pc", &["sleeping"], 15).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(plugin_status(&node, "pg")["status"], "running");
    let (items, _) = bot.logs.read_after(None, 1000);
    assert!(
        items
            .iter()
            .any(|i| i.source == "plugin:bg_plugin" && i.message.contains("暂不休眠")),
        "no busy log"
    );

    node.shutdown().await;
}

#[tokio::test]
async fn command_only_plugin_sleeps_during_other_traffic_and_wakes_for_alias() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fc").tempdir_in("/tmp").unwrap();
    let mut cfg = node_config(dir.path());
    cfg.dev_services = true;
    cfg.plugin_idle_sleep = Some(Duration::from_secs(2));
    let command_only = json!({"id": "cold", "name": "cold_cmd_plugin", "repo_url": "https://github.com/x/cold", "version": "1", "enabled": true, "config": {}});
    let generic = json!({"id": "echo", "name": "cmd_plugin", "repo_url": "https://github.com/x/cmd", "version": "1", "enabled": true, "config": {}});
    preinstall(&cfg, &command_only);
    preinstall(&cfg, &generic);
    let node = Node::new(cfg, Some(command(&python))).unwrap();
    let desired = serde_json::from_value(json!({
        "bot_id": "b1", "user_id": "u1", "name": "小鱼", "running": true,
        "platforms": [], "plugins": [command_only, generic]
    }))
    .unwrap();
    node.put(desired).await.unwrap();
    wait_status(&node, "cold", &["running", "sleeping"], 30).await;
    wait_status(&node, "echo", &["running", "sleeping"], 30).await;

    for n in 1..=8 {
        assert!(say(&node, "echo active", n).await.contains("echo:active"));
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(plugin_status(&node, "cold")["status"], "sleeping");
    assert_eq!(plugin_status(&node, "echo")["status"], "running");
    assert!(!node.get("b1").unwrap().procs.read().unwrap().contains_key("cold"));
    assert!(say(&node, "/鱼池\u{3000}有鱼", 9).await.contains("pond:有鱼"));
    assert_eq!(plugin_status(&node, "cold")["status"], "running");

    assert!(say(&node, "/bait", 10).await.contains("waiting for bait"));
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(plugin_status(&node, "cold")["status"], "running");
    assert!(say(&node, "shrimp", 11).await.contains("caught:shrimp"));

    wait_status(&node, "cold", &["sleeping"], 30).await;
    node.get("b1").unwrap().reload_plugin("cold").await.unwrap();
    wait_status(&node, "cold", &["running"], 30).await;

    node.shutdown().await;
}

#[tokio::test]
async fn light_plugins_share_a_group_process_and_others_run_alone() {
    let Some(python) = sidecar_python() else { return };
    let dir = tempfile::Builder::new().prefix("fg").tempdir_in("/tmp").unwrap();
    let mut cfg = node_config(dir.path());
    cfg.dev_services = true;
    cfg.plugin_groups = true;
    cfg.plugin_idle_sleep = Some(Duration::from_secs(3));
    let cmd = json!({"id": "pc", "name": "cmd_plugin", "repo_url": "https://github.com/x/cmd", "version": "1", "enabled": true, "config": {}});
    let conf = json!({"id": "pf", "name": "conf_plugin", "repo_url": "https://github.com/x/conf", "version": "1", "enabled": true, "config": {}});
    let bg = json!({"id": "pg", "name": "bg_plugin", "repo_url": "https://github.com/x/bg", "version": "1", "enabled": true, "config": {}});
    preinstall(&cfg, &cmd);
    preinstall(&cfg, &conf);
    preinstall(&cfg, &bg);
    let node = Node::new(cfg, Some(command(&python))).unwrap();
    let desired = serde_json::from_value(json!({
        "bot_id": "b1", "user_id": "u1", "name": "小鱼", "running": true,
        "platforms": [], "plugins": [cmd, conf, bg]
    }))
    .unwrap();
    node.put(desired).await.unwrap();

    for id in ["pc", "pf", "pg"] {
        wait_status(&node, id, &["running"], 30).await;
    }
    let bot = node.get("b1").unwrap();
    let pids: Vec<u32> = {
        let procs = bot.procs.read().unwrap();
        ["pc", "pf", "pg"].iter().map(|id| procs[*id].pid).collect()
    };
    assert_eq!(pids[0], pids[1]);
    assert_ne!(pids[0], pids[2]);
    assert!(plugin_status(&node, "pc")["status_detail"].as_str().unwrap().contains("共用"));
    assert_eq!(plugin_status(&node, "pg")["status_detail"], "");
    let (items, _) = bot.logs.read_after(None, 1000);
    assert!(
        items
            .iter()
            .any(|i| i.source == "plugin:bg_plugin" && i.message.contains("改为单独运行")),
        "no fallback log"
    );

    let reply = say(&node, "echo hi", 1).await;
    assert!(reply.contains("echo:hi"), "{reply}");
    let reply = say(&node, "kv a", 2).await;
    assert!(reply.contains("none->a|conf_plugin|True"), "{reply}");

    // the group process ends when its plugins sleep, and a message starts a new one for them
    wait_status(&node, "pc", &["sleeping"], 20).await;
    wait_status(&node, "pf", &["sleeping"], 20).await;
    let reply = say(&node, "kv b", 3).await;
    assert!(reply.contains("a->b|conf_plugin|True"), "{reply}");
    let procs = bot.procs.read().unwrap();
    assert_eq!(procs["pc"].pid, procs["pf"].pid);
    assert_ne!(procs["pc"].pid, pids[0]);
    drop(procs);

    node.shutdown().await;
}
