use crate::FunctionIR;
use crate::wasm::WasmNumericLaneStats;

#[derive(Debug, Clone)]
pub(crate) struct WasmStageAuditShape {
    functions: usize,
    simple_ops: usize,
    tir_blocks: usize,
    tir_ops: usize,
    largest_function: String,
    largest_ops: usize,
}

/// One request-scoped decision, shared by every stage and emitted function.
/// Daemon requests may select different environments; no process-global cache.
#[derive(Clone, Copy)]
pub(crate) struct WasmStageAudit {
    enabled: bool,
}

impl WasmStageAudit {
    pub(crate) fn from_environment() -> Self {
        Self {
            enabled: std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1"),
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(enabled: bool) -> Self {
        Self { enabled }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn start(self) -> Option<std::time::Instant> {
        self.enabled.then(std::time::Instant::now)
    }
}

pub(crate) fn simple_ir_stage_shape(functions: &[FunctionIR]) -> WasmStageAuditShape {
    let mut simple_ops = 0usize;
    let mut largest_function = "<none>".to_string();
    let mut largest_ops = 0usize;
    for func in functions {
        let ops = func.ops.len();
        simple_ops = simple_ops.saturating_add(ops);
        if ops > largest_ops {
            largest_ops = ops;
            largest_function = func.name.clone();
        }
    }
    WasmStageAuditShape {
        functions: functions.len(),
        simple_ops,
        tir_blocks: 0,
        tir_ops: 0,
        largest_function,
        largest_ops,
    }
}

pub(crate) fn tir_module_stage_shape(
    module: &crate::tir::function::TirModule,
) -> WasmStageAuditShape {
    let mut tir_blocks = 0usize;
    let mut tir_ops = 0usize;
    let mut largest_function = "<none>".to_string();
    let mut largest_ops = 0usize;
    for func in &module.functions {
        let blocks = func.blocks.len();
        let ops = func
            .blocks
            .values()
            .fold(0usize, |total, block| total.saturating_add(block.ops.len()));
        tir_blocks = tir_blocks.saturating_add(blocks);
        tir_ops = tir_ops.saturating_add(ops);
        if ops > largest_ops {
            largest_ops = ops;
            largest_function = func.name.clone();
        }
    }
    WasmStageAuditShape {
        functions: module.functions.len(),
        simple_ops: 0,
        tir_blocks,
        tir_ops,
        largest_function,
        largest_ops,
    }
}

pub(crate) fn emit_wasm_stage_audit(
    audit: WasmStageAudit,
    stage: &str,
    shape: impl FnOnce() -> WasmStageAuditShape,
    bytes: Option<usize>,
    unused_imports: Option<usize>,
    changed_functions: Option<usize>,
    elapsed_ms: impl FnOnce() -> Option<u128>,
) {
    if !audit.enabled() {
        return;
    }
    // Shape walks, owned name projections, elapsed sampling and RSS lookup all
    // happen after this one audit gate, including calls from shared TIR stages.
    let shape = shape();
    let elapsed_ms = elapsed_ms();
    eprintln!(
        "[molt-wasm-stage-audit] stage={stage} functions={} simple_ops={} tir_blocks={} tir_ops={} largest_function={} largest_ops={} bytes={} unused_imports={} changed_functions={} elapsed_ms={} peak_rss_mib={}",
        shape.functions,
        shape.simple_ops,
        shape.tir_blocks,
        shape.tir_ops,
        shape.largest_function,
        shape.largest_ops,
        bytes
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        unused_imports
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        changed_functions
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}

pub(crate) fn emit_wasm_numeric_lane_audit(audit: WasmStageAudit, stats: WasmNumericLaneStats) {
    if !audit.enabled() {
        return;
    }
    eprintln!(
        "[molt-wasm-numeric-lane-audit] op_loop_additive_inline_int_raw_sites={} op_loop_additive_float_raw_sites={} op_loop_additive_guarded_int_sites={} op_loop_additive_boxed_runtime_sites={} op_loop_bitwise_inline_int_raw_sites={} op_loop_bitwise_guarded_int_sites={} op_loop_bitwise_boxed_runtime_sites={} op_loop_division_guarded_int_sites={} op_loop_division_boxed_runtime_sites={} op_loop_raw_sites_total={} op_loop_guarded_or_boxed_sites_total={}",
        stats.op_loop_additive_inline_int_raw_sites,
        stats.op_loop_additive_float_raw_sites,
        stats.op_loop_additive_guarded_int_sites,
        stats.op_loop_additive_boxed_runtime_sites,
        stats.op_loop_bitwise_inline_int_raw_sites,
        stats.op_loop_bitwise_guarded_int_sites,
        stats.op_loop_bitwise_boxed_runtime_sites,
        stats.op_loop_division_guarded_int_sites,
        stats.op_loop_division_boxed_runtime_sites,
        stats.raw_result_total(),
        stats.guarded_or_boxed_total(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_request_never_evaluates_observation_inputs() {
        let audit = WasmStageAudit { enabled: false };
        assert!(audit.start().is_none());
        for _ in 0..128 {
            emit_wasm_stage_audit(
                audit,
                "disabled",
                || panic!("disabled audit projected the IR"),
                None,
                None,
                None,
                || panic!("disabled audit sampled elapsed time"),
            );
        }
        assert!(WasmStageAudit { enabled: true }.start().is_some());
        assert!(!audit.enabled());
    }
}
