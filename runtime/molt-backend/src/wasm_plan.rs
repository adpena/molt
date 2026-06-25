use crate::repr::{ContainerKind, ContainerStorageKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::{FunctionIR, OpIR};
use wasm_encoder::Instruction;

#[derive(Debug, Clone)]
pub(crate) struct WasmStageAuditShape {
    functions: usize,
    simple_ops: usize,
    tir_blocks: usize,
    tir_ops: usize,
    largest_function: String,
    largest_ops: usize,
}

pub(crate) fn is_shared_drop_fact_marker(kind: &str) -> bool {
    matches!(kind, "drop_inserted" | "exception_region_drops_inserted")
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

pub(crate) fn gpu_runtime_call_symbol(kind: &str) -> Option<&'static str> {
    match kind {
        "gpu_thread_id" => Some("molt_gpu_thread_id"),
        "gpu_block_id" => Some("molt_gpu_block_id"),
        "gpu_block_dim" => Some("molt_gpu_block_dim"),
        "gpu_grid_dim" => Some("molt_gpu_grid_dim"),
        "gpu_barrier" => Some("molt_gpu_barrier"),
        _ => None,
    }
}

pub(crate) fn wasm_scalar_integer_fast_path_for_op(
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        // `/=` shares `/`'s int-family fast-path gating: both produce a float on
        // int operands, so the lane is keyed on integer-family operands rather
        // than an integer result.
        "div" | "inplace_div" | "lt" | "le" | "gt" | "ge" | "eq" | "ne" => {
            plan.op_args_are_integer_family(op)
        }
        _ => plan.op_prefers_integer_runtime_lane(op),
    }
}

pub(crate) fn wasm_scalar_truthiness_fast_path_for_name(
    plan: &ScalarRepresentationPlan,
    name: &str,
) -> bool {
    plan.name_is_integer_family(name)
}

pub(crate) fn wasm_specialized_container_import(
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    kind: &str,
    op: &OpIR,
) -> Option<&'static str> {
    match kind {
        "index"
            if plan.op_has_container_storage(op_index, op, ContainerStorageKind::FlatListInt) =>
        {
            Some("list_int_getitem")
        }
        "store_index"
            if plan.op_has_container_storage(op_index, op, ContainerStorageKind::FlatListInt) =>
        {
            Some("list_int_setitem")
        }
        "contains" | "len" | "index" | "store_index" => {
            let container = op.args.as_ref()?.first()?;
            let container_kind = plan.name_container_kind(container)?;
            match kind {
                "contains" => match container_kind {
                    ContainerKind::Set => Some("set_contains"),
                    ContainerKind::Dict => Some("dict_contains"),
                    ContainerKind::List => Some("list_contains"),
                    ContainerKind::Str => Some("str_contains"),
                    ContainerKind::Tuple => None,
                },
                "len" => match container_kind {
                    ContainerKind::List => Some("len_list"),
                    ContainerKind::Str => Some("len_str"),
                    ContainerKind::Dict => Some("len_dict"),
                    ContainerKind::Tuple => Some("len_tuple"),
                    ContainerKind::Set => Some("len_set"),
                },
                "index" => match container_kind {
                    ContainerKind::Dict => Some("dict_getitem"),
                    ContainerKind::Tuple => Some("tuple_getitem"),
                    ContainerKind::List | ContainerKind::Set | ContainerKind::Str => None,
                },
                "store_index" => match container_kind {
                    ContainerKind::Dict => Some("dict_setitem"),
                    ContainerKind::List
                    | ContainerKind::Set
                    | ContainerKind::Tuple
                    | ContainerKind::Str => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) const DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES: &[&str] = &[
    "molt_gpu_broadcast_binary_contiguous",
    "molt_gpu_linear_contiguous",
    "molt_gpu_linear_split_last_dim_contiguous",
    "molt_gpu_linear_squared_relu_gate_interleaved_contiguous",
    "molt_gpu_matmul_contiguous",
    "molt_gpu_permute_contiguous",
    "molt_gpu_repeat_axis_contiguous",
    "molt_gpu_rms_norm_last_axis_contiguous",
    "molt_gpu_rope_apply_contiguous",
    "molt_gpu_softmax_last_axis_contiguous",
    "molt_gpu_squared_relu_gate_interleaved_contiguous",
    "molt_gpu_tensor_from_buffer",
    "molt_gpu_tensor_from_parts",
    "molt_gpu_tensor__tensor_concat_first_dim",
    "molt_gpu_tensor__tensor_scatter_rows",
    "molt_gpu_tensor__tensor_take_rows",
    "molt_gpu_tensor__zeros",
];

pub(crate) fn prepare_lir_wasm_fast_output(
    func_ir: &FunctionIR,
    tir_func: &crate::tir::function::TirFunction,
) -> Option<crate::tir::lower_to_wasm::WasmFunctionOutput> {
    // Drive the LIR carrier derivation from the PROVEN `repr_by_value` (the
    // single source of truth shared with LLVM), so `LirRepr::I64` — and the bare
    // `I64Add`/`I64*` ops gated on it — is assigned ONLY to proven `RawI64Safe`
    // integers. The proof comes from the value-range analysis computed on this
    // exact `tir_func` (the same source the LLVM `LlvmReprFacts::build` uses), so
    // WASM and LLVM agree per `ValueId`. An unproven `int` (`MaybeBigInt`) lowers
    // to `DynBox`; its arithmetic emits the boxed runtime `Call(0)`, which is
    // rejected below so the function falls back to the IntFastLane-guarded slow
    // path (correctness preserved; the unsound bare op is un-emittable here).
    let vr = crate::representation_plan::value_range_for(tir_func);
    let repr = crate::representation_plan::repr_by_value_for(func_ir, tir_func, Some(&vr));
    let output = crate::tir::lower_to_wasm::lower_tir_to_wasm_boxed_i64_abi_with_proof(
        tir_func,
        Some(&repr),
        Some(&vr),
    )?;
    let has_placeholder_call = output
        .instructions
        .iter()
        .any(|inst| matches!(inst, Instruction::Call(0)));
    if has_placeholder_call {
        None
    } else {
        Some(output)
    }
}

pub(crate) fn is_production_lir_wasm_fast_path_name(func_name: &str) -> bool {
    func_name.contains("____molt_globals_builtin__")
}
