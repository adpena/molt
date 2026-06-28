use crate::FunctionIR;

#[derive(Debug, Clone)]
pub(crate) struct WasmStageAuditShape {
    functions: usize,
    simple_ops: usize,
    tir_blocks: usize,
    tir_ops: usize,
    largest_function: String,
    largest_ops: usize,
}

fn wasm_stage_audit_enabled() -> bool {
    std::env::var("MOLT_WASM_STAGE_AUDIT").as_deref() == Ok("1")
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
    stage: &str,
    shape: WasmStageAuditShape,
    bytes: Option<usize>,
    unused_imports: Option<usize>,
    changed_functions: Option<usize>,
    elapsed_ms: Option<u128>,
) {
    if !wasm_stage_audit_enabled() {
        return;
    }
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
