//! Downloads a real plugin from GitHub and builds its venv with uv. Needs network and uv.
//! Run with `cargo test --test install_live -- --ignored`.

use std::sync::Arc;

use fish_node::logs::LogBuffer;
use fish_node::model::PluginSpec;
use fish_node::plugin::install::{is_installed, Installer};

#[tokio::test]
#[ignore]
async fn install_helloworld_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let installer = Installer {
        codeload_base: None,
        http: fish_node::node::http_client(),
        uv: std::env::var("FISH_UV").unwrap_or_else(|_| "uv".into()),
        python: std::env::var("FISH_SIDECAR_PYTHON").unwrap_or_else(|_| "python3".into()),
        cache_dir: dir.path().join("uv-cache"),
    };
    let spec = PluginSpec {
        id: "p".into(),
        name: "helloworld".into(),
        repo_url: "https://github.com/Soulter/helloworld".into(),
        version: "1".into(),
        enabled: true,
        config: serde_json::Value::Null,
    };
    let logs = Arc::new(LogBuffer::new("b", 500));
    let plugin_dir = dir.path().join("plugins").join("helloworld");
    let inst = installer
        .ensure(&spec, &plugin_dir, &logs, "plugin:helloworld")
        .await
        .unwrap();
    assert!(inst.src.join("main.py").is_file());
    assert!(inst.site_packages.is_some());
    assert!(is_installed(&spec, &plugin_dir));
}
