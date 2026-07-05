pub(crate) fn resolved_batch_size_limit(default: usize) -> usize {
    resolve_zero_disables_limit("MOLT_BACKEND_BATCH_SIZE", default)
}

pub(crate) fn resolved_batch_op_budget_limit(default: usize) -> usize {
    resolve_zero_disables_limit("MOLT_BACKEND_BATCH_OP_BUDGET", default)
}

fn resolve_zero_disables_limit(var: &str, default: usize) -> usize {
    let raw = std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    if raw == 0 { usize::MAX } else { raw }
}
