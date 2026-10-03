pub(crate) const BACKEND_DAEMON_PROTOCOL_VERSION: u32 = 1;

pub(crate) static DAEMON_REQUEST_ENV_KEYS: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| molt_ir::backend_environment::daemon_request_env_keys().to_vec());
