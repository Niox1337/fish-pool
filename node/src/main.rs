use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("fish-node [--dev]\n\nEnvironment: FISH_NODE_ID FISH_NODE_SECRET FISH_CONSOLE_URL FISH_LISTEN FISH_DATA_DIR");
        println!("FISH_SIDECAR_PYTHON FISH_SIDECAR_DIR FISH_UV FISH_PLUGIN_UID FISH_PLUGIN_GID FISH_DEV RUST_LOG");
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();
    let cfg = fish_node::config::Config::from_env(&args)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .thread_stack_size(1024 * 1024)
        .enable_all()
        .build()?;
    rt.block_on(fish_node::run(cfg))
}
