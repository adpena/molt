#[cfg(feature = "native-backend")]
pub(crate) const DEFAULT_BACKEND_BATCH_SIZE: usize = 64;
#[cfg(feature = "native-backend")]
pub(crate) const DEFAULT_STDLIB_BATCH_SIZE: usize = 128;
#[cfg(feature = "native-backend")]
pub(crate) const DEFAULT_BACKEND_BATCH_OP_BUDGET: usize = 8_000;
pub(crate) const MIB: usize = 1024 * 1024;
pub(crate) const DEFAULT_DAEMON_REQUEST_LIMIT_BYTES: usize = 512 * MIB;
pub(crate) const DEFAULT_STDIN_REQUEST_LIMIT_BYTES: usize = DEFAULT_DAEMON_REQUEST_LIMIT_BYTES;

#[cfg(molt_backend_daemon)]
pub(crate) const DEFAULT_DAEMON_MAX_JOBS: usize = 512;
/// Seconds an idle daemon waits for a connection before it retires: sccache
/// waits 10 minutes, the Gradle daemon and Bazel's server 3 hours.
#[cfg(molt_backend_daemon)]
pub(crate) const DEFAULT_DAEMON_IDLE_TIMEOUT_SECS: usize = 1800;
