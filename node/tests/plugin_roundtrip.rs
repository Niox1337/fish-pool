//! Plugin protocol round trip against a stub fork server.
//!
//! The stub is this test binary re-executed with `FISH_STUB_SIDECAR=1`, which makes the
//! `stub_fork_server` test act as the fork server and every spawned plugin.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fish_node::model::{Component, MessageEvent, MessageType, Sender};
use fish_node::plugin::{launch, LaunchSpec, LoadError, PluginOut, Sidecar, SidecarCommand};
use serde_json::{json, Value};
use tokio::sync::mpsc;

fn write_line(w: &Mutex<UnixStream>, v: &Value) {
    let mut g = w.lock().unwrap();
    let _ = g.write_all(format!("{v}\n").as_bytes());
}

fn fake_plugin(stream: UnixStream, proc_id: String, ctrl: Arc<Mutex<UnixStream>>) {
    let w = Mutex::new(stream.try_clone().unwrap());
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["op"], "config");
    let config = first["config"].clone();
    write_line(
        &w,
        &json!({"op": "log", "level": "info", "message": format!("loading with {config}")}),
    );
    match config["fail"].as_str() {
        Some("error_op") => {
            write_line(&w, &json!({"op": "error", "message": "bad plugin"}));
            let _ = w.lock().unwrap().shutdown(std::net::Shutdown::Both);
            write_line(
                &ctrl,
                &json!({"op": "exited", "proc_id": proc_id, "pid": std::process::id(), "code": 1}),
            );
            return;
        }
        Some("exit2") => {
            write_line(
                &w,
                &json!({"op": "log", "level": "error", "message": "plugin load failed: ImportError: x"}),
            );
            write_line(&w, &json!({"op": "log", "level": "debug", "message": "traceback"}));
            let _ = w.lock().unwrap().shutdown(std::net::Shutdown::Both);
            write_line(
                &ctrl,
                &json!({"op": "exited", "proc_id": proc_id, "pid": std::process::id(), "code": 2}),
            );
            return;
        }
        _ => {}
    }
    write_line(
        &w,
        &json!({"op": "hello", "proc_id": proc_id, "plugin": {"name": "stub", "version": "1.2.3", "handlers": [
            {"id": "h1", "kind": "command", "pattern": "ping", "priority": 0},
            {"id": "h2", "kind": "llm_request", "priority": 5}
        ]}}),
    );
    let mut code = 0;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let v: Value = serde_json::from_str(&line).unwrap();
        let event_id = v["event_id"].clone();
        match v["op"].as_str().unwrap_or("") {
            "event" => {
                let text = v["event"]["message_str"].as_str().unwrap_or("");
                match text {
                    "ping" => {
                        write_line(&w, &json!({"op": "matched", "event_id": event_id, "matched": true}));
                        write_line(
                            &w,
                            &json!({"op": "send", "event_id": event_id,
                            "session": v["event"]["unified_msg_origin"], "chain": [{"type": "plain", "text": "pong"}]}),
                        );
                        write_line(&w, &json!({"op": "llm", "req_id": "r1", "prompt": "hi"}));
                        write_line(
                            &w,
                            &json!({"op": "done", "event_id": event_id, "stop": true, "call_llm": false}),
                        );
                    }
                    "crash" => {
                        code = 3;
                        break;
                    }
                    _ => write_line(&w, &json!({"op": "matched", "event_id": event_id, "matched": false})),
                }
            }
            "hook" => {
                let sys = v["payload"]["system_prompt"].as_str().unwrap_or("");
                write_line(
                    &w,
                    &json!({"op": "plugin_error", "event_id": event_id, "handler": "h", "error": "e", "traceback": "tb"}),
                );
                write_line(
                    &w,
                    &json!({"op": "result", "req_id": "q1", "event_id": event_id, "chain": [{"type": "plain", "text": "r"}], "llm": true}),
                );
                write_line(
                    &w,
                    &json!({"op": "agent", "req_id": "q2", "event_id": event_id, "prompt": "p", "contexts": [{"role": "user", "content": "c"}], "image_urls": ["https://x/a.png"]}),
                );
                write_line(
                    &w,
                    &json!({"op": "hook_result", "hook_id": v["hook_id"], "stopped": v["kind"] == "llm_request" && event_id == "e3",
                        "payload": {"system_prompt": format!("{sys} [hooked]")}}),
                );
            }
            "llm_result" => {
                write_line(
                    &w,
                    &json!({"op": "log", "level": "warning", "message": format!("llm said {}", v)}),
                );
            }
            "config" => {
                write_line(
                    &w,
                    &json!({"op": "log", "level": "info", "message": format!("config now {}", v["config"])}),
                );
            }
            "shutdown" => break,
            _ => {}
        }
    }
    let _ = w.lock().unwrap().shutdown(std::net::Shutdown::Both);
    write_line(
        &ctrl,
        &json!({"op": "exited", "proc_id": proc_id, "pid": std::process::id(), "code": code}),
    );
}

fn run_stub(sock: &str) {
    let _ = std::fs::remove_file(sock);
    let listener = UnixListener::bind(sock).unwrap();
    let (ctrl, _) = listener.accept().unwrap();
    let writer = Arc::new(Mutex::new(ctrl.try_clone().unwrap()));
    let children: Arc<Mutex<HashMap<String, UnixStream>>> = Arc::default();
    for line in BufReader::new(ctrl).lines() {
        let Ok(line) = line else { break };
        let v: Value = serde_json::from_str(&line).unwrap();
        let proc_id = v["proc_id"].as_str().unwrap_or("").to_string();
        match v["op"].as_str().unwrap_or("") {
            "spawn" => {
                let stream = match UnixStream::connect(v["socket"].as_str().unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        write_line(
                            &writer,
                            &json!({"op": "error", "proc_id": proc_id, "message": e.to_string()}),
                        );
                        continue;
                    }
                };
                children
                    .lock()
                    .unwrap()
                    .insert(proc_id.clone(), stream.try_clone().unwrap());
                write_line(
                    &writer,
                    &json!({"op": "spawned", "proc_id": proc_id, "pid": std::process::id()}),
                );
                assert!(v.get("config").is_none(), "the config goes over the plugin socket");
                assert_eq!(v["config_on_socket"], true);
                let ctrl = writer.clone();
                std::thread::spawn(move || fake_plugin(stream, proc_id, ctrl));
            }
            "kill" => {
                if let Some(s) = children.lock().unwrap().remove(&proc_id) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn stub_fork_server() {
    if std::env::var("FISH_STUB_SIDECAR").is_err() {
        return;
    }
    let args: Vec<String> = std::env::args().collect();
    let sock = args
        .iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .expect("--socket");
    run_stub(sock);
    std::process::exit(0);
}

fn event(text: &str) -> MessageEvent {
    MessageEvent {
        platform: "qq_official".into(),
        bot_id: "b1".into(),
        self_id: "me".into(),
        message_type: MessageType::Private,
        group_id: None,
        session_id: "u1".into(),
        message_id: "m1".into(),
        unified_msg_origin: "p1:FriendMessage:u1".into(),
        sender: Sender {
            user_id: "u1".into(),
            nickname: "u1".into(),
        },
        message_str: text.into(),
        chain: vec![Component::plain(text)],
        is_at_or_wake: true,
        is_admin: false,
        timestamp: 0,
        raw: None,
    }
}

async fn next_out(rx: &mut mpsc::Receiver<fish_node::plugin::OutMsg>) -> fish_node::plugin::OutMsg {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("out message")
        .expect("channel open")
}

fn spec(dir: &std::path::Path, proc_id: &str) -> LaunchSpec {
    LaunchSpec {
        proc_id: proc_id.into(),
        plugin_id: "pl1".into(),
        plugin_name: "stub".into(),
        bot_id: "b1".into(),
        plugin_dir: dir.join("src"),
        venv_site_packages: None,
        data_dir: dir.join("data"),
        config: json!({"greeting": "hi"}),
        share_key: None,
    }
}

#[tokio::test]
async fn plugin_round_trip_through_sidecar_protocol() {
    let dir = tempfile::Builder::new().prefix("fx").tempdir_in("/tmp").unwrap();
    let exe = std::env::current_exe().unwrap();
    let cmd = SidecarCommand {
        program: exe.display().to_string(),
        args: ["--exact", "--nocapture", "--test-threads=1", "--", "stub_fork_server"]
            .map(String::from)
            .to_vec(),
        envs: vec![("FISH_STUB_SIDECAR".into(), "1".into())],
        cwd: None,
        uid: None,
        gid: None,
    };
    let sidecar = Sidecar::new(cmd, dir.path().join("run"));
    let (out_tx, mut out_rx) = mpsc::channel(16);

    let (proc, mut exit_rx) = launch(&sidecar, spec(dir.path(), "proc-a"), out_tx.clone(), None)
        .await
        .expect("launch");
    assert_eq!(proc.hello.name, "stub");
    assert_eq!(proc.hello.version, "1.2.3");
    assert!(proc.has_handler("llm_request"));
    assert_eq!(proc.handler_priority("llm_request"), 5);
    let first = next_out(&mut out_rx).await;
    assert!(matches!(&first.msg, PluginOut::Log { message, .. } if message.contains("greeting")));

    // matched event: send, llm request and done
    let (m, d) = proc.dispatch("e1", &event("ping"));
    assert!(tokio::time::timeout(Duration::from_secs(2), m).await.unwrap().unwrap());
    let send = next_out(&mut out_rx).await;
    assert_eq!(send.plugin_id, "pl1");
    assert_eq!(
        send.msg,
        PluginOut::Send {
            event_id: Some("e1".into()),
            session: Some("p1:FriendMessage:u1".into()),
            chain: vec![Component::plain("pong")]
        }
    );
    let llm = next_out(&mut out_rx).await;
    assert!(
        matches!(&llm.msg, PluginOut::Llm { req_id, prompt, .. } if req_id == "r1" && prompt.as_deref() == Some("hi"))
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), d).await.unwrap().unwrap(),
        (true, false)
    );

    // the node answers the llm request through the writer handed out with the message
    llm.writer
        .send(&json!({"op": "llm_result", "req_id": "r1", "text": "hello"}));
    let echoed = next_out(&mut out_rx).await;
    assert!(
        matches!(&echoed.msg, PluginOut::Log { level, message } if level == "warning" && message.contains("hello"))
    );

    // unmatched event
    let (m, _d) = proc.dispatch("e2", &event("something else"));
    assert!(!tokio::time::timeout(Duration::from_secs(2), m).await.unwrap().unwrap());

    // hook with the service ops a plugin may send while handling it
    let ev = event("hooked");
    let hooked = proc
        .hook(
            "llm_request",
            Some(("e3", &ev)),
            &json!({"prompt": "p", "system_prompt": "sys", "contexts": []}),
            Duration::from_secs(2),
        )
        .await
        .expect("hook result");
    assert_eq!(hooked["payload"]["system_prompt"], "sys [hooked]");
    assert_eq!(hooked["stopped"], true);
    let err = next_out(&mut out_rx).await;
    assert!(
        matches!(&err.msg, PluginOut::PluginError { event_id: Some(e), handler, error, traceback }
        if e == "e3" && handler == "h" && error == "e" && traceback == "tb")
    );
    let res = next_out(&mut out_rx).await;
    assert!(
        matches!(&res.msg, PluginOut::Result { req_id, event_id: Some(e), session: None, chain, llm: true }
        if req_id == "q1" && e == "e3" && chain.len() == 1)
    );
    let agent = next_out(&mut out_rx).await;
    assert!(
        matches!(&agent.msg, PluginOut::Agent { req_id, prompt: Some(p), system_prompt: None, contexts, image_urls, .. }
        if req_id == "q2" && p == "p" && contexts.len() == 1 && image_urls == &["https://x/a.png"])
    );
    let plain = proc
        .hook("plugin_loaded", None, &json!({}), Duration::from_secs(2))
        .await
        .expect("hook result");
    assert_eq!(plain["stopped"], false);
    for _ in 0..3 {
        next_out(&mut out_rx).await;
    }

    // config update
    proc.writer()
        .send(&json!({"op": "config", "config": {"greeting": "yo"}}));
    let cfg = next_out(&mut out_rx).await;
    assert!(matches!(&cfg.msg, PluginOut::Log { message, .. } if message.contains("yo")));

    // graceful shutdown reports exit code 0
    proc.shutdown();
    let code = tokio::time::timeout(Duration::from_secs(5), &mut exit_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(code, 0);

    // a crashing plugin reports its exit code and closes the socket
    let out_tx_fail = out_tx.clone();
    let (proc_b, exit_b) = launch(&sidecar, spec(dir.path(), "proc-b"), out_tx, None)
        .await
        .expect("launch b");
    let _ = next_out(&mut out_rx).await;
    let _ = proc_b.dispatch("e4", &event("crash"));
    let code = tokio::time::timeout(Duration::from_secs(5), exit_b)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(code, 3);
    tokio::time::timeout(Duration::from_secs(5), proc_b.closed.cancelled())
        .await
        .unwrap();

    // load failures: an error op before hello, and exit code 2 with an error log line
    for (fail, want) in [
        ("error_op", "bad plugin"),
        ("exit2", "plugin load failed: ImportError: x"),
    ] {
        let mut s = spec(dir.path(), &format!("proc-{fail}"));
        s.config = json!({"fail": fail});
        let err = match launch(&sidecar, s, out_tx_fail.clone(), None).await {
            Ok(_) => panic!("{fail} must fail"),
            Err(e) => e,
        };
        assert_eq!(
            err.downcast_ref::<LoadError>(),
            Some(&LoadError(want.into())),
            "{fail}: {err:#}"
        );
    }

    sidecar.shutdown().await;
}
