#[cfg(feature = "native-backend")]
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

#[cfg(feature = "native-backend")]
pub(crate) struct TraceOpsConfig {
    pub(crate) stride: usize,
}

#[cfg(feature = "native-backend")]
pub(crate) fn should_trace_ops(func_name: &str) -> Option<TraceOpsConfig> {
    // Read per function, not once per process: a daemon serves builds with
    // different settings.
    let raw = backend_env::var(BackendEnv::MOLT_TRACE_OP_PROGRESS)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let (filter_part, stride_part) = match raw.split_once(':') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (raw.as_str(), None),
    };
    let stride = stride_part
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5_000);
    let matches = filter_part == "1"
        || filter_part.eq_ignore_ascii_case("all")
        || func_name == filter_part
        || func_name.contains(filter_part);
    if matches {
        Some(TraceOpsConfig { stride })
    } else {
        None
    }
}
