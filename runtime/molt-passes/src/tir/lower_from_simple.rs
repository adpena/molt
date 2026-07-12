//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

mod loop_structure;
mod pre_ssa;
mod type_inference;

use self::loop_structure::{detect_loop_cond_blocks, detect_loop_structure};
use self::pre_ssa::{rewrite_cell_locals_to_store_load, rewrite_loop_index_to_store_load};
#[cfg(test)]
use self::type_inference::string_to_tir_type;
use self::type_inference::{
    infer_return_type, param_string_to_tir_type, propagate_arithmetic_types,
};
use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, TirBlock};
use super::cfg::CFG;
use super::function::{TirFunction, TirModule};
use super::op_kinds_generated::opcode_sets_exception_handling_table;
use super::ssa::{SsaOutput, convert_to_ssa_with_name_and_params};
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
    let rewritten_ops = rewrite_loop_index_to_store_load(&ir.ops);
    let mut working_ops = if rewritten_ops.is_empty() {
        ir.ops.clone()
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

    let tmp_ir = crate::ir::FunctionIR {
        name: ir.name.clone(),
        ops: working_ops.clone(),
        params: ir.params.clone(),
        param_types: ir.param_types.clone(),
        source_file: ir.source_file.clone(),
        is_extern: false,
    };
    let ir_ref = &tmp_ir;
    let ops = &working_ops[..];

    // 1. Build CFG from the rewritten op stream.
    let cfg = CFG::build(ops);

    // 2. Convert to SSA with block arguments (pass params for implicit entry defs).
    // No catch_unwind — panics propagate cleanly through rayon. Using
    // AssertUnwindSafe on borrowed state violates Rust's unwind safety contract.
    let ssa = convert_to_ssa_with_name_and_params(&ir.name, &cfg, ops, &ir.params);

    // 3. Assemble the TirFunction from the SSA output.
    let mut tir_func = assemble_function(ir_ref, &cfg, ssa);
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

    // Propagate parameter types to the entry block arguments in the types map.
    // This is critical for SCCP: without it, parameters default to DynBox and
    // the type inference can't prove that `n + 1` produces I64 even when
    // the function signature says `n: int`. Entry block args correspond 1:1
    // to function parameters.
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

    // Forward type propagation: when all operands of an Add/Sub/Mul/etc. are
    // known-typed from constants or parameter signatures, infer the result
    // type before deriving the function return contract.
    propagate_arithmetic_types(&tir_blocks, &mut types);

    // Infer a return type from the SSA output by inspecting return terminators.
    let return_type = infer_return_type(&tir_blocks, &types);

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

    TirFunction {
        name: ir.name.clone(),
        param_names: ir.params.clone(),
        param_types,
        return_type,
        blocks: block_map,
        entry_block,
        next_value,
        next_block,
        attrs: {
            let mut a = super::ops::AttrDict::new();
            if ir.ops.iter().any(|op| op.kind == "ret") {
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
    }
}

// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
