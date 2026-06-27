use crate::tir::function::TirFunction;

fn drop_inner_stage_audit_enabled(func: &TirFunction) -> bool {
    let enabled = std::env::var("MOLT_DROP_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_MODULE_STAGE_AUDIT").as_deref() == Ok("1")
        || std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1");
    if !enabled {
        return false;
    }
    match std::env::var("MOLT_DROP_STAGE_AUDIT_FUNC") {
        Ok(filter) if !filter.trim().is_empty() => func.name.contains(filter.trim()),
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_drop_inner_stage_audit(
    func: &TirFunction,
    stage: &str,
    plans: Option<usize>,
    edge_splits: Option<usize>,
    roots: Option<usize>,
    blocks_seen: Option<usize>,
    elapsed_ms: u128,
) {
    if !drop_inner_stage_audit_enabled(func) {
        return;
    }
    let blocks = func.blocks.len();
    let ops = func
        .blocks
        .values()
        .fold(0usize, |count, block| count.saturating_add(block.ops.len()));
    eprintln!(
        "[molt-drop-inner-audit] stage={stage} function={} blocks={} ops={} plans={} edge_splits={} roots={} blocks_seen={} elapsed_ms={} peak_rss_mib={}",
        func.name,
        blocks,
        ops,
        plans
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        edge_splits
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        roots
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        blocks_seen
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms,
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}
