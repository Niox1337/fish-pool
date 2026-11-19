use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use crate::bot::{Bot, Shared};
use crate::config::Config;
use crate::events::Events;
use crate::model::DesiredBot;
use crate::plugin::install::Installer;
use crate::plugin::{Sidecar, SidecarCommand};
use crate::qr::QrTasks;
use crate::sandbox::{self, Cgroups};
use crate::sign::SignedClient;
use crate::store::{ConvDb, NodeDb};
use crate::usage::{system_memory, Usage};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Node {
    pub shared: Arc<Shared>,
    bots: Mutex<HashMap<String, Arc<Bot>>>,
    pub qr: Arc<QrTasks>,
    pub usage: Usage,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("fish-node/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(4)
        .build()
        .expect("http client")
}

impl Node {
    /// Builds the node from configuration. `sidecar_cmd` overrides the fork server command.
    pub fn new(cfg: Config, sidecar_cmd: Option<SidecarCommand>) -> Result<Arc<Node>> {
        std::fs::create_dir_all(cfg.tenants_dir())?;
        std::fs::create_dir_all(cfg.run_dir())?;
        let cache = cfg.data_dir.join("uv-cache");
        std::fs::create_dir_all(&cache)?;
        #[cfg(unix)]
        if cfg.plugin_uid.is_some() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(cfg.run_dir(), std::fs::Permissions::from_mode(0o1777));
            let _ = std::fs::set_permissions(&cfg.data_dir, std::fs::Permissions::from_mode(0o711));
        }
        let http = http_client();
        let events = Events::start(cfg.console_url.clone().map(|base_url| SignedClient {
            http: http.clone(),
            base_url,
            node_id: cfg.node_id.clone(),
            key: cfg.secret.clone(),
        }));
        let cmd = sidecar_cmd.unwrap_or_else(|| sandbox::sidecar_command(&cfg));
        let shared = Arc::new(Shared {
            db: Arc::new(NodeDb::open(&cfg.data_dir.join("node.db"))?),
            conv: Arc::new(ConvDb::open(&cfg.data_dir.join("conversations.db"))?),
            sidecar: Arc::new(Sidecar::new(cmd, cfg.run_dir())),
            events,
            cgroups: Cgroups::init(&cfg),
            installer: Installer {
                http: http.clone(),
                uv: cfg.uv.clone(),
                python: cfg.sidecar_python.clone(),
                cache_dir: cache,
                codeload_base: if cfg.dev_services {
                    std::env::var("FISH_CODELOAD_BASE").ok()
                } else {
                    None
                },
            },
            install_sem: Semaphore::new(1),
            storage_by_user: Mutex::new(HashMap::new()),
            http: http.clone(),
            cfg,
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in ["node.db", "conversations.db"] {
                let _ = std::fs::set_permissions(shared.cfg.data_dir.join(f), std::fs::Permissions::from_mode(0o600));
            }
        }
        Ok(Arc::new(Node {
            shared,
            bots: Mutex::new(HashMap::new()),
            qr: Arc::new(QrTasks::new(http)),
            usage: Usage::default(),
        }))
    }

    pub fn get(&self, id: &str) -> Option<Arc<Bot>> {
        self.bots.lock().unwrap().get(id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Bot>> {
        self.bots.lock().unwrap().values().cloned().collect()
    }

    pub async fn put(&self, d: DesiredBot) -> Result<()> {
        self.shared.db.save_bot(&d)?;
        let bot = {
            let mut g = self.bots.lock().unwrap();
            g.entry(d.bot_id.clone())
                .or_insert_with(|| Bot::new(self.shared.clone(), d.clone()))
                .clone()
        };
        bot.apply(d).await;
        Ok(())
    }

    pub async fn delete(&self, id: &str) {
        let bot = self.bots.lock().unwrap().remove(id);
        match bot {
            Some(b) => b.destroy().await,
            None => {
                let _ = self.shared.db.delete_bot(id);
                let _ = self.shared.conv.delete_bot(id);
            }
        }
    }

    /// Restores all bots saved in `node.db`.
    pub async fn restore(&self) -> Result<usize> {
        let saved = self.shared.db.load_bots()?;
        let n = saved.len();
        for d in saved {
            let bot = Bot::new(self.shared.clone(), d.clone());
            self.bots.lock().unwrap().insert(d.bot_id.clone(), bot.clone());
            tokio::spawn(async move { bot.apply(d).await });
        }
        Ok(n)
    }

    pub async fn shutdown(&self) {
        let bots = self.all();
        futures_util::future::join_all(bots.iter().map(|b| b.stop())).await;
        self.shared.sidecar.shutdown().await;
    }

    pub async fn health(&self) -> Value {
        let (total, used) = system_memory().await;
        json!({
            "node_id": self.shared.cfg.node_id,
            "version": VERSION,
            "memory_total_bytes": total,
            "memory_used_bytes": used,
            "bots": self.bots.lock().unwrap().len(),
        })
    }
}
