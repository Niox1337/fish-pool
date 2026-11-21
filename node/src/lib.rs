//! Node agent that runs chat bots of all tenants on one host.

pub mod api;
pub mod bot;
pub mod config;
pub mod crypto;
pub mod events;
pub mod llm;
pub mod logs;
pub mod model;
pub mod node;
pub mod pipeline;
pub mod platform;
pub mod plugin;
pub mod qr;
pub mod sandbox;
pub mod sign;
pub mod store;
pub mod system;
pub mod usage;

use std::sync::Arc;

use anyhow::{Context, Result};

/// Runs the node until SIGINT or SIGTERM.
pub async fn run(cfg: config::Config) -> Result<()> {
    let listen = cfg.listen.clone();
    let dev = cfg.dev;
    let node = node::Node::new(cfg, None)?;
    let restored = node.restore().await?;
    tracing::info!(dev, restored, "fish-node {} listening on {listen}", node::VERSION);
    usage::spawn_sampler(node.clone());
    usage::spawn_burst_guard(node.clone());
    system::spawn_system_sampler(node.clone());
    let app = api::router(node.clone());
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shutting down");
    let n: Arc<node::Node> = node;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), n.shutdown()).await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}
