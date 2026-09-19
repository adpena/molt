//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

mod loop_structure;
mod pre_ssa;
mod type_inference;

use self::loop_structure::{detect_loop_cond_blocks, detect_loop_structure};
use self::pre_ssa::rewrite_cell_locals_to_store_load;
pub use self::pre_ssa::rewrite_loop_index_to_store_load;
use self::type_inference::param_string_to_tir_type;
#[cfg(test)]
use self::type_inference::string_to_tir_type;
use super::type_refine::infer_return_type;
use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, TirBlock};
use super::cfg::CFG;
use super::function::{TirFunction, TirModule};
use super::op_kinds_generated::{
    SimpleIrReturnShape, opcode_sets_exception_handling_table, simpleir_return_shape,
};
use super::ssa::{SsaOutput, convert_to_ssa_with_name_and_params};
use super::target_info::TargetInfo;
use super::types::TirType;

/// Lift every **non-extern** `FunctionIR` in `functions` to TIR and assemble a
/// [`TirModule`] for the whole-program module phase (the E1 inliner). Returns the
/// module plus an `idx_map` aligning each module position to its original index
/// in `functions` — externs are skipped (their bodies live in `stdlib_shared.o`
/// and carry no inlinable ops), so module positions are NOT equal to source
/// indices. The caller back-converts each post-inline `TirFunction` at module
/// position `p` into `functions[idx_map[p]]`.
///
/// Mirrors the extern filter the legacy `compute_leaf_functions_via_call_graph`
/// used (`.filter(|f| !f.is_extern)`), so the call graph the inliner builds over
/// this module sees exactly the local function bodies.
pub fn lower_functions_to_tir_module(functions: &[FunctionIR]) -> (TirModule, Vec<usize>) {
    let mut tir_functions = Vec::new();
    let mut idx_map = Vec::new();
    for (i, f) in functions.iter().enumerate() {
        if f.is_extern {
            continue;
        }
        tir_functions.push(lower_to_tir(f));
        idx_map.push(i);
    }
    (
        TirModule {
            name: "native_module".to_string(),
            functions: tir_functions,
        },
        idx_map,
    )
}

/// Convert a SimpleIR function into a fully-constructed TIR function.
///
/// Pipeline: SimpleIR ops → CFG extraction → SSA conversion → TIR construction.
///
/// TIR typing must come from structural sources only: explicit function
/// parameter types plus canonical propagation over the SSA graph. Transport
/// compatibility metadata on SimpleIR is intentionally ignored here.
pub fn lower_to_tir(ir: &FunctionIR) -> TirFunction {
    lower_to_tir_impl(ir, None)
}

/// Lower for an executable target, materializing target-required async-work
/// boundaries before SSA so payload-bearing exception edges receive the same
/// canonical environment mapping as frontend-authored transfers.
pub fn lower_to_tir_for_target(ir: &FunctionIR, target_info: &TargetInfo) -> TirFunction {
    lower_to_tir_impl(ir, Some(target_info))
}

fn lower_to_tir_impl(ir: &FunctionIR, target_info: Option<&TargetInfo>) -> TirFunction {
    if std::env::var("MOLT_TRACE_SIMPLE_IMPORT").as_deref() == Ok("1") {
        for op in &ir.ops {
            if op.kind.contains("import") {
                eprintln!(
                    "Simple import trace: func={} kind={} args={:?} var={:?} out={:?} s_value={:?}",
                    ir.name, op.kind, op.args, op.var, op.out, op.s_value
                );
            }
        }
    }
    // 0. Memory SSA: rewrite cell-based local variables (store_index/index on
    //    the locals list) into store_var/load_var. This enables the SSA pass
    //    to track local variable mutations through loop iterations — the key
    //    enabler for type specialization and integer-lane optimization on loops.
    //
    //    The rewrite is safe because lower_to_simple_ir restores the original
    //    store_index/index patterns from the SSA output.
    // Rewrite loop_index_start/loop_index_next to store_var/load_var so the
    // SSA pass creates proper phi nodes at loop headers for induction variables.
    // Establish missing durable origins before any rewrite inserts/removes
    // operations. Existing transported origins are preserved, never reinterpreted
    // as positions in this current stream.
    let source_ops: Vec<_> = ir
        .ops
        .iter()
        .enumerate()
        .map(|(index, op)| {
            let mut source = op.clone();
            if source.source_op_idx.is_none() {
                source.source_op_idx = Some(
                    i64::try_from(index).expect("SimpleIR source operation index exceeds i64"),
                );
            }
            source
        })
        .collect();
    let rewritten_ops = rewrite_loop_index_to_store_load(&source_ops);
    let mut working_ops = if rewritten_ops.is_empty() {
        source_ops
    } else {
        rewritten_ops
    };
    // RC drop-insertion substrate (design 20): function-level attrs do not live
    // in FunctionIR, so drop facts round-trip as leading SimpleIR marker ops. The
    // full `drop_inserted` marker tells native to disable its legacy value-tracker
    // because TIR owns the whole function's RC. The narrower exception-region
    // marker only protects already-inserted CreationRef/MatchRef releases across
    // relifts and optimizer re-runs; native deliberately ignores it as an RC
    // suppression signal. Both markers carry no per-op TIR semantics, so strip
    // them before CFG/SSA construction and preserve them as function attrs.
    let had_drop_inserted_marker = working_ops
        .iter()
        .any(|op| op.kind == crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR);
    let had_exception_region_drops_marker = working_ops.iter().any(|op| {
        op.kind == crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    });
    working_ops.retain(|op| {
        op.kind != crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR
            && op.kind != crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    });
    // Memory SSA: rewrite cell-based locals (store_index/index on a 1-elem
    // list "cell") to store_var/load_var so SSA generates proper phi nodes
    // at loop headers for cell variables. Always-on; no env gate.
    let _cell_rewrite_applied = rewrite_cell_locals_to_store_load(&mut working_ops);

    let mut tmp_ir = crate::ir::FunctionIR {
        name: ir.name.clone(),
        ops: working_ops,
        params: ir.params.clone(),
        param_types: ir.param_types.clone(),
        source_file: ir.source_file.clone(),
        is_extern: false,
        codegen_partition: ir.codegen_partition,
        execution_context: ir.execution_context,
    };
    let mut tir_func =
        if target_info.is_some_and(TargetInfo::supports_pending_call_eval_breaker_poll) {
            lower_prepared_with_async_work_materialization(&mut tmp_ir)
        } else {
            lower_prepared_function(&tmp_ir)
        };
    // Preserve the RC drop-insertion marker across the round-trip (see above).
    if had_drop_inserted_marker {
        tir_func.attrs.insert(
            crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.to_string(),
            crate::tir::ops::AttrValue::Bool(true),
        );
    }
    if had_exception_region_drops_marker {
        tir_func.attrs.insert(
            crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
            crate::tir::ops::AttrValue::Bool(true),
        );
    }
    tir_func
}

fn lower_prepared_function(ir: &FunctionIR) -> TirFunction {
    // Build the exact CFG consumed by SSA, then convert once. No catch_unwind:
    // panics propagate cleanly through rayon, preserving unwind safety.
    let cfg = CFG::build(&ir.ops);
    let ssa = convert_to_ssa_with_name_and_params(&ir.name, &cfg, &ir.ops, &ir.params);
    assemble_function(ir, &cfg, ssa)
}

/// Lower a placement preview whose source indices are exact positions in the
/// prepared SimpleIR stream, materialize against that same stream, then restore
/// durable source provenance on the returned TIR. `OpIR::source_op_idx` is an
/// origin/inline-cache identity and must never be dereferenced as a current
/// array position after a TIR round trip.
fn lower_prepared_with_async_work_materialization(ir: &mut FunctionIR) -> TirFunction {
    let transported_source_indices: Vec<_> = ir
        .ops
        .iter_mut()
        .map(|op| op.source_op_idx.take())
        .collect();
    let mut preview = lower_prepared_function(ir);
    for (op, source_op_idx) in ir.ops.iter_mut().zip(transported_source_indices) {
        op.source_op_idx = source_op_idx;
    }

    let materialized = super::passes::async_work_poll::materialize_before_ssa(ir, &mut preview);
    if materialized.transfers_inserted != 0 {
        return lower_prepared_function(ir);
    }

    for op in preview
        .blocks
        .values_mut()
        .flat_map(|block| block.ops.iter_mut())
    {
        let Some(prepared_index) = op.source_op_index() else {
            continue;
        };
        let source = ir.ops.get(prepared_index).unwrap_or_else(|| {
            panic!(
                "prepared SimpleIR source index {prepared_index} exceeds {} operations",
                ir.ops.len()
            )
        });
        op.set_source_op_index(source.source_op_index_or(prepared_index));
    }
    preview
}

/// Assemble a `TirFunction` from a `FunctionIR`, its `CFG`, and the `SsaOutput`.
fn assemble_function(ir: &FunctionIR, cfg: &CFG, ssa: SsaOutput) -> TirFunction {
    let SsaOutput {
        blocks: mut tir_blocks,
        mut types,
        next_value,
    } = ssa;

    // Determine semantic parameter types. `param_types` also carries the
    // native ABI carrier marker `i64` for boxed Molt object words; that marker
    // is not a Python `int` proof and must remain DynBox in TIR.
    let param_types: Vec<TirType> = if let Some(ref pt) = ir.param_types {
        pt.iter().map(|s| param_string_to_tir_type(s)).collect()
    } else {
        ir.params.iter().map(|_| TirType::DynBox).collect()
    };

    // Preserve annotation metadata on entry block arguments. Entry arguments
    // correspond 1:1 to function parameters, but their annotations admit
    // overriding subclasses and cannot seed exact scalar return contracts.
    if let Some(entry) = tir_blocks.first() {
        for (arg_val, param_ty) in entry.args.iter().zip(param_types.iter()) {
            if *param_ty != TirType::DynBox {
                types.insert(arg_val.id, param_ty.clone());
            }
        }
    }
    if let Some(entry) = tir_blocks.first_mut() {
        for (arg_val, param_ty) in entry.args.iter_mut().zip(param_types.iter()) {
            if *param_ty != TirType::DynBox {
                arg_val.ty = param_ty.clone();
            }
        }
    }

    // Build the block map keyed by BlockId.
    let mut block_map: HashMap<BlockId, TirBlock> = HashMap::with_capacity(tir_blocks.len());
    for block in tir_blocks {
        block_map.insert(block.id, block);
    }

    let entry_block = if cfg.blocks.is_empty() {
        BlockId(0)
    } else {
        BlockId(cfg.entry as u32)
    };

    let next_block = block_map.len() as u32;

    // Detect whether the function contains exception-handling ops.
    let has_exception_handling = block_map.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| opcode_sets_exception_handling_table(op.opcode))
    });

    // Build label_id_map: for each CFG block that starts with a label/state_label,
    // record the original label value so the back-conversion can emit labels
    // with matching IDs for check_exception / jump / br_if targets.
    let mut label_id_map = HashMap::new();
    for (bid, bb) in cfg.blocks.iter().enumerate() {
        // Scan the ops in this block for a leading label/state_label.
        for op_idx in bb.start_op..bb.end_op {
            let op = &ir.ops[op_idx];
            if matches!(op.kind.as_str(), "label" | "state_label") {
                if let Some(label_val) = op.value {
                    label_id_map.insert(bid as u32, label_val);
                }
                break; // Only care about the first label in the block.
            }
            // If we hit a non-structural op before finding a label, stop.
            if !is_structural(&op.kind) {
                break;
            }
        }
    }

    // Detect loop structural roles from the original SimpleIR ops.
    let (loop_roles, loop_pairs, loop_break_kinds) = detect_loop_structure(ir, cfg);
    let loop_cond_blocks = detect_loop_cond_blocks(ir, cfg);

    let mut function = TirFunction {
        name: ir.name.clone(),
        execution_context: ir.execution_context,
        param_names: ir.params.clone(),
        param_types,
        return_type: TirType::DynBox,
        blocks: block_map,
        entry_block,
        next_value,
        next_block,
        attrs: {
            let mut a = super::ops::AttrDict::new();
            if ir.codegen_partition {
                a.insert(
                    super::function::CODEGEN_PARTITION_ATTR.into(),
                    super::ops::AttrValue::Bool(true),
                );
            }
            if ir
                .ops
                .iter()
                .any(|op| simpleir_return_shape(op.kind.as_str()) == SimpleIrReturnShape::Value)
            {
                a.insert(
                    "_original_has_ret".into(),
                    super::ops::AttrValue::Bool(true),
                );
            }
            if let Some(source_file) = &ir.source_file
                && !source_file.is_empty()
            {
                a.insert(
                    super::ops::SOURCE_FILE_ATTR.into(),
                    super::ops::AttrValue::Str(source_file.clone()),
                );
            }
            a
        },
        value_types: types,
        has_exception_handling,
        label_id_map,
        loop_roles,
        loop_pairs,
        loop_break_kinds,
        loop_cond_blocks,
    };
    let exact = super::type_refine::extract_exact_scalar_map(&function);
    function
        .value_types
        .extend(exact.iter().map(|(&value, ty)| (value, ty.clone())));
    function.return_type = infer_return_type(function.blocks.values(), &exact);
    function
}

// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
