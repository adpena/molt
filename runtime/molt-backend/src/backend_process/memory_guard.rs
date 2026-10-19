use molt_host_process::host_process_environment_generated::HostProcessEnv;
use molt_host_process::{
    default_process_max_rss_gb, install_process_memory_limit, process_max_rss_bytes_from,
};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

fn backend_max_rss_bytes() -> u64 {
    process_max_rss_bytes_from(
        backend_env::var(BackendEnv::MOLT_BACKEND_MAX_PROCESS_RSS_GB)
            .ok()
            .as_deref(),
        default_process_max_rss_gb(),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "startup diagnostic control uses the generated host-process catalog"
)]
fn install_limit(max_bytes: u64) {
    let debug = std::env::var(HostProcessEnv::MOLT_DEBUG_RLIMIT.name()).as_deref() == Ok("1");
    install_process_memory_limit(max_bytes, debug);
}

pub(crate) fn install_process_memory_guard() {
    install_limit(backend_max_rss_bytes());
}

/// A daemon widens only its process envelope before serving concurrent jobs;
/// each compile retains the admitted request budget through BackendEnv.
#[cfg(molt_backend_daemon)]
pub(crate) fn widen_process_memory_guard(compiles: usize) {
    let compiles = u64::try_from(compiles.max(1)).unwrap_or(u64::MAX);
    install_limit(backend_max_rss_bytes().saturating_mul(compiles));
}
