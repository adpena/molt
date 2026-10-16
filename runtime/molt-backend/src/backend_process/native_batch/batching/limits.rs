use molt_backend::backend_environment::{self, BackendEnv};

pub(crate) fn resolved_batch_size_limit(default: usize) -> usize {
    resolve_zero_disables_limit(BackendEnv::MOLT_BACKEND_BATCH_SIZE, default)
}

pub(crate) fn resolved_batch_op_budget_limit(default: usize) -> usize {
    resolve_zero_disables_limit(BackendEnv::MOLT_BACKEND_BATCH_OP_BUDGET, default)
}

fn resolve_zero_disables_limit(key: BackendEnv, default: usize) -> usize {
    let raw = backend_environment::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}
