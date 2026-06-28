use crate::repr::{ContainerKind, ContainerStorageKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

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

// Multi-value return analysis (WASM_OPTIMIZATION_PLAN.md section 3.1).
//
// Scans every function in the IR and identifies call sites whose result is
// immediately destructured via a fixed number of `tuple_index` ops with
// constant indices 0..N-1. These are candidates for the multi-value return
// optimisation: the callee can push N i64 results directly, and the caller can
// consume them without a heap-allocated tuple.
//
// Returns a map: callee_name -> required_return_count (2 or 3). Only functions
// where every call site destructures to the same arity are included.
pub(crate) fn detect_multi_return_candidates(ir: &SimpleIR) -> BTreeMap<String, usize> {
    // callee -> Option<arity> (None means conflicting arities => ineligible)
    let mut candidate_arity: BTreeMap<String, Option<usize>> = BTreeMap::new();

    for func_ir in &ir.functions {
        let ops = &func_ir.ops;
        for (i, op) in ops.iter().enumerate() {
            // Only consider call_internal (user-defined functions we control).
            if op.kind != "call_internal" {
                continue;
            }
            let Some(callee) = op.s_value.as_ref() else {
                continue;
            };
            let Some(result_var) = op.out.as_ref() else {
                continue;
            };

            // Scan forward to find consecutive tuple_index ops on result_var.
            let mut unpack_count = 0usize;
            let mut seen_indices: BTreeSet<i64> = BTreeSet::new();
            for j in (i + 1)..ops.len() {
                let next_op = &ops[j];
                if next_op.kind != "tuple_index" {
                    break;
                }
                let Some(args) = next_op.args.as_ref() else {
                    break;
                };
                if args.len() < 2 || args[0] != *result_var {
                    break;
                }
                // The index argument should be a const-int; we check by
                // looking at the preceding ops, but for this analysis just
                // count the tuple_index ops.
                if let Some(idx_val) = next_op.value {
                    seen_indices.insert(idx_val);
                }
                unpack_count += 1;
            }

            // Only 2 or 3 element unpacks are worth multi-value. Mark callees
            // with non-destructuring call sites as ineligible.
            if !(2..=3).contains(&unpack_count) {
                candidate_arity.insert(callee.clone(), None);
                continue;
            }

            match candidate_arity.entry(callee.clone()) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(Some(unpack_count));
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    if *e.get() != Some(unpack_count) {
                        // Conflicting arities across call sites: not eligible.
                        *e.get_mut() = None;
                    }
                }
            }
        }
    }

    let call_site_candidates: BTreeMap<String, usize> = candidate_arity
        .into_iter()
        .filter_map(|(name, arity)| arity.map(|a| (name, a)))
        .collect();

    // Phase 2: Verify the callee function body: every `ret` must return a
    // variable that was produced by a `tuple_new` with the expected arity.
    let func_map: BTreeMap<&str, &FunctionIR> =
        ir.functions.iter().map(|f| (f.name.as_str(), f)).collect();

    call_site_candidates
        .into_iter()
        .filter(|(name, expected_arity)| {
            let Some(func_ir) = func_map.get(name.as_str()) else {
                return false;
            };
            let mut tuple_new_vars: BTreeSet<String> = BTreeSet::new();
            let mut has_any_ret = false;
            let mut all_rets_ok = true;

            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "tuple_new" => {
                        if let Some(args) = &op.args
                            && args.len() == *expected_arity
                            && let Some(out) = &op.out
                        {
                            tuple_new_vars.insert(out.clone());
                        }
                    }
                    "ret" => {
                        has_any_ret = true;
                        match &op.var {
                            Some(var) if tuple_new_vars.contains(var) => {}
                            _ => {
                                all_rets_ok = false;
                            }
                        }
                    }
                    _ => {}
                }
            }

            has_any_ret && all_rets_ok
        })
        .collect()
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

#[derive(Debug, Clone)]
pub(crate) enum WasmFunctionLoweringPlan {
    LirFast(crate::wasm::body::WasmBody),
    Generic {
        reason: crate::wasm::body::WasmLirFallbackReason,
    },
}

impl WasmFunctionLoweringPlan {
    pub(crate) fn lir_fast_body(&self) -> Option<&crate::wasm::body::WasmBody> {
        match self {
            Self::LirFast(body) => Some(body),
            Self::Generic { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn generic_reason(&self) -> Option<crate::wasm::body::WasmLirFallbackReason> {
        match self {
            Self::LirFast(_) => None,
            Self::Generic { reason } => Some(*reason),
        }
    }
}

pub(crate) type WasmFunctionLoweringPlans = BTreeMap<String, WasmFunctionLoweringPlan>;

pub(crate) fn prepare_lir_wasm_fast_plan(
    tir_func: &crate::tir::function::TirFunction,
) -> WasmFunctionLoweringPlan {
    // Drive the LIR carrier derivation from the PROVEN `repr_by_value` (the
    // single source of truth shared with LLVM), so `LirRepr::I64` is assigned
    // only to proven raw-i64 carriers (`RawI64Safe` or `RawI64FullDeopt`).
    // Arithmetic still consults the inline-window proof before taking unchecked
    // machine ops. The proof comes from the value-range analysis computed on
    // this exact `tir_func` (the same source the LLVM `LlvmReprFacts::build`
    // uses), so WASM and LLVM agree per `ValueId`. An unproven `int`
    // (`MaybeBigInt`) lowers
    // to `DynBox`; its arithmetic emits a typed generic-path bail, which is
    // rejected below so the function falls back to the IntFastLane-guarded slow
    // path (correctness preserved; the unsound bare op is un-emittable here).
    let vr = crate::representation_plan::value_range_for(tir_func);
    let repr = crate::representation_plan::repr_by_value_for(tir_func, Some(&vr));
    let Some(output) =
        crate::wasm::lir_fast::lower_tir_to_wasm_boxed_i64_abi_with_proof(tir_func, &repr, &vr)
    else {
        return WasmFunctionLoweringPlan::Generic {
            reason: crate::wasm::body::WasmLirFallbackReason::BoxedI64AbiUnsupported,
        };
    };
    if let Some(reason) = output.bail_to_generic_reason() {
        WasmFunctionLoweringPlan::Generic { reason }
    } else {
        WasmFunctionLoweringPlan::LirFast(output)
    }
}

pub(crate) fn compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(
    ir: &SimpleIR,
    escaped_callable_targets: &BTreeSet<String>,
) -> WasmFunctionLoweringPlans {
    let mut plans = BTreeMap::new();
    for func_ir in &ir.functions {
        if func_ir.is_extern || !is_production_lir_wasm_fast_path_name(&func_ir.name) {
            continue;
        }
        if escaped_callable_targets.contains(&func_ir.name) {
            plans.insert(
                func_ir.name.clone(),
                WasmFunctionLoweringPlan::Generic {
                    reason: crate::wasm::body::WasmLirFallbackReason::EscapedCallableTarget,
                },
            );
            continue;
        }
        let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
        crate::tir::type_refine::refine_types(&mut tir_func);
        plans.insert(func_ir.name.clone(), prepare_lir_wasm_fast_plan(&tir_func));
    }
    plans
}

pub(crate) fn is_production_lir_wasm_fast_path_name(func_name: &str) -> bool {
    func_name.contains("____molt_globals_builtin__")
}

#[cfg(all(test, feature = "wasm-backend"))]
mod wasm_lir_fast_plan_tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use crate::wasm::body::WasmLirFallbackReason;

    fn const_i64_return_func(value: i64) -> TirFunction {
        let mut func = TirFunction::new("const_i64_return".into(), vec![], TirType::I64);
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        func
    }

    fn add_two_i64_params_func() -> TirFunction {
        let mut func = TirFunction::new(
            "add_two_i64_params".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        func
    }

    fn checked_mul_i64_consts_func() -> TirFunction {
        let mut func = TirFunction::new("checked_mul_i64_consts".into(), vec![], TirType::I64);
        let lhs = func.fresh_value();
        let rhs = func.fresh_value();
        let product = func.fresh_value();
        let overflow = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for (id, value) in [(lhs, 6), (rhs, 7)] {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(value));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![id],
                attrs,
                source_span: None,
            });
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckedMul,
            operands: vec![lhs, rhs],
            results: vec![product, overflow],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![product],
        };
        func
    }

    #[test]
    fn wasm_lir_fast_plan_accepts_proven_i64_body() {
        let plan = prepare_lir_wasm_fast_plan(&const_i64_return_func(42));
        let body = plan
            .lir_fast_body()
            .expect("proven i64 const return must keep the LIR fast plan");

        assert_eq!(plan.generic_reason(), None);
        assert_eq!(body.bail_to_generic_reason(), None);
    }

    #[test]
    fn wasm_lir_fast_plan_records_boxed_i64_abi_rejection() {
        let plan = prepare_lir_wasm_fast_plan(&add_two_i64_params_func());

        assert_eq!(
            plan.generic_reason(),
            Some(WasmLirFallbackReason::BoxedI64AbiUnsupported)
        );
    }

    #[test]
    fn wasm_lir_fast_plan_records_body_bail_reason() {
        let plan = prepare_lir_wasm_fast_plan(&checked_mul_i64_consts_func());

        assert_eq!(
            plan.generic_reason(),
            Some(WasmLirFallbackReason::BoxedCheckedArithmetic)
        );
    }

    #[test]
    fn wasm_lir_fast_plan_records_escaped_callable_reason() {
        let name = "pkg____molt_globals_builtin__escaped".to_string();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: name.clone(),
                params: vec![],
                ops: vec![],
                param_types: Some(vec![]),
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };
        let escaped = BTreeSet::from([name.clone()]);
        let plans = compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(&ir, &escaped);

        assert_eq!(
            plans
                .get(&name)
                .and_then(WasmFunctionLoweringPlan::generic_reason),
            Some(WasmLirFallbackReason::EscapedCallableTarget)
        );
    }
}
