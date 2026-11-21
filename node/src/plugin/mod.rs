pub mod group;
pub mod install;
pub mod proc;
pub mod sidecar;

pub use group::Groups;
pub use proc::{launch, Hello, LaunchSpec, LoadError, OutMsg, PluginOut, PluginProc, ProcWriter, Ungroupable, EXIT_LOAD_FAILED};
pub use sidecar::{Sidecar, SidecarCommand};

/// Restart delay for crashed plugins: 2s doubling up to 5 minutes.
pub fn restart_backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs((2u64 << attempt.min(8)).min(300))
}
