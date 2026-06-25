//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, LoopBreakKind, LoopRole, TirBlock};
use super::cfg::CFG;
use super::function::{TirFunction, TirModule};
use super::op_kinds_generated::simpleir_kind_is_pre_ssa_rewritten;
use super::ssa::{SsaOutput, convert_to_ssa_with_name_and_params};
use super::types::TirType;
use super::values::ValueId;

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

/// Rewrite `loop_index_start`/`loop_index_next` into `store_var`/`load_var`
/// patterns so the SSA conversion creates proper phi nodes at loop headers.
///
/// The original pattern:
/// ```text
///   ... (before loop_start)
///   loop_start
///   loop_index_start  out=V  args=INIT   // V = INIT on first iteration
///   ...loop body...
///   loop_index_next   out=V  args=UPDATED // V = UPDATED on subsequent iterations
///   loop_continue
///   loop_end
/// ```
///
/// The rewritten pattern:
/// ```text
///   ... (before loop_start)
///   store_var  var=V  args=INIT           // define V before the loop
///   loop_start
///   load_var   var=V  out=V               // read V (phi at loop header)
///   ...loop body...
///   store_var  var=V  args=UPDATED        // update V at end of loop body
///   loop_continue
///   loop_end
/// ```
///
/// Returns an empty Vec if no rewrites were needed (caller uses original ops).
fn is_pre_ssa_rewritten_kind(kind: &str) -> bool {
    simpleir_kind_is_pre_ssa_rewritten(kind)
}

fn rewrite_loop_index_to_store_load(ops: &[crate::ir::OpIR]) -> Vec<crate::ir::OpIR> {
    use crate::ir::OpIR;

    // Quick scan: any loop-index op consumed by this pre-SSA rewrite?
    let has_loop_index = ops
        .iter()
        .any(|op| is_pre_ssa_rewritten_kind(op.kind.as_str()));
    if !has_loop_index {
        return Vec::new();
    }

    // Find the loop_start op that immediately precedes each loop_index_start.
    // We need to insert store_var BEFORE the loop_start.
    //
    // Also find every loop_index_start and loop_index_next to rewrite them.
    let mut result: Vec<OpIR> = Vec::with_capacity(ops.len() + 8);

    // First, find the positions of loop_start ops so we can insert store_var
    // before them. We process ops sequentially, buffering the loop_start and
    // inserting the store_var before it when we see loop_index_start.

    // Strategy: two-pass approach.
    // Pass 1: identify (loop_start_idx, var_name, init_arg) for each pattern.
    // Pass 2: emit rewritten ops.

    struct LoopIndexPattern {
        loop_start_idx: usize,
        var_name: String,
        init_arg: String,
    }

    let mut patterns: Vec<LoopIndexPattern> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                loop_start_stack.push(idx);
            }
            "loop_end" => {
                loop_start_stack.pop();
            }
            "loop_index_start" => {
                if let Some(&ls_idx) = loop_start_stack.last() {
                    let var_name = op.out.clone().unwrap_or_default();
                    let init_arg = op
                        .args
                        .as_ref()
                        .and_then(|a| a.first())
                        .cloned()
                        .unwrap_or_default();
                    if !var_name.is_empty() && var_name != "none" {
                        patterns.push(LoopIndexPattern {
                            loop_start_idx: ls_idx,
                            var_name,
                            init_arg,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if patterns.is_empty() {
        return Vec::new();
    }

    // Build sets for quick lookup.
    let insert_before: std::collections::HashMap<usize, Vec<&LoopIndexPattern>> = {
        let mut map: std::collections::HashMap<usize, Vec<&LoopIndexPattern>> =
            std::collections::HashMap::new();
        for pat in &patterns {
            map.entry(pat.loop_start_idx).or_default().push(pat);
        }
        map
    };
    let rewrite_vars: std::collections::HashSet<&str> =
        patterns.iter().map(|p| p.var_name.as_str()).collect();
    let loop_carrier_for_start: std::collections::HashMap<usize, String> = patterns
        .iter()
        .map(|pat| (pat.loop_start_idx, pat.var_name.clone()))
        .collect();

    let mut active_loop_carriers: Vec<Option<String>> = Vec::new();

    // Pass 2: emit rewritten ops.
    for (idx, op) in ops.iter().enumerate() {
        // Before a loop_start, insert store_var for each pattern.
        if let Some(pats) = insert_before.get(&idx) {
            for pat in pats {
                result.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(pat.var_name.clone()),
                    args: Some(vec![pat.init_arg.clone()]),
                    ..OpIR::default()
                });
            }
        }

        match op.kind.as_str() {
            "loop_start" => {
                active_loop_carriers.push(loop_carrier_for_start.get(&idx).cloned());
                result.push(op.clone());
            }
            "loop_index_start" => {
                let var_name = op.out.clone().unwrap_or_default();
                if rewrite_vars.contains(var_name.as_str()) {
                    // Rewrite to load_var: read V from the phi.
                    result.push(OpIR {
                        kind: "load_var".to_string(),
                        var: Some(var_name.clone()),
                        out: Some(var_name),
                        ..OpIR::default()
                    });
                } else {
                    result.push(op.clone());
                }
            }
            "loop_index_next" => {
                let carrier_name = active_loop_carriers
                    .iter()
                    .rev()
                    .find_map(|carrier| carrier.as_ref())
                    .cloned();
                if let Some(var_name) = carrier_name {
                    // Rewrite to store_var: update V.
                    let updated_arg = op
                        .args
                        .as_ref()
                        .and_then(|a| a.first())
                        .cloned()
                        .unwrap_or_default();
                    result.push(OpIR {
                        kind: "store_var".to_string(),
                        var: Some(var_name),
                        args: Some(vec![updated_arg]),
                        ..OpIR::default()
                    });
                } else {
                    result.push(op.clone());
                }
            }
            "loop_end" => {
                active_loop_carriers.pop();
                result.push(op.clone());
            }
            _ => {
                result.push(op.clone());
            }
        }
    }

    result
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
        block.ops.iter().any(|op| {
            matches!(
                op.opcode,
                super::ops::OpCode::TryStart
                    | super::ops::OpCode::TryEnd
                    | super::ops::OpCode::StateBlockStart
                    | super::ops::OpCode::StateBlockEnd
                    | super::ops::OpCode::CheckException
            )
        })
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
    let (loop_roles, loop_pairs, loop_break_kinds) = detect_loop_structure(ir, cfg, &block_map);
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

/// Scan the original SimpleIR ops and CFG to detect which TIR blocks correspond
/// to `loop_start` and `loop_end` structural markers, which loop-end pairs with
/// each header, and what the original loop-break polarity was.
fn detect_loop_structure(
    ir: &FunctionIR,
    cfg: &CFG,
    _block_map: &HashMap<BlockId, TirBlock>,
) -> (
    HashMap<BlockId, LoopRole>,
    HashMap<BlockId, BlockId>,
    HashMap<BlockId, LoopBreakKind>,
) {
    let mut roles = HashMap::new();
    let mut loop_pairs = HashMap::new();
    let mut loop_break_kinds = HashMap::new();
    let block_containing = |op_idx: usize| -> Option<BlockId> {
        cfg.blocks
            .iter()
            .position(|bb| bb.start_op <= op_idx && op_idx < bb.end_op)
            .map(|bid| BlockId(bid as u32))
    };
    for (bid, bb) in cfg.blocks.iter().enumerate() {
        if bb.start_op >= ir.ops.len() {
            continue;
        }
        let first_kind = ir.ops[bb.start_op].kind.as_str();
        match first_kind {
            "loop_start" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopHeader);
            }
            "loop_end" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopEnd);
            }
            _ => {}
        }
    }
    let mut loop_stack: Vec<(usize, BlockId)> = Vec::new();
    for (op_idx, op) in ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                if let Some(header_bid) = block_containing(op_idx) {
                    loop_stack.push((op_idx, header_bid));
                }
            }
            "loop_end" => {
                let Some((header_op_idx, header_bid)) = loop_stack.pop() else {
                    continue;
                };
                let Some(end_bid) = block_containing(op_idx) else {
                    continue;
                };
                loop_pairs.insert(header_bid, end_bid);

                let mut nested_depth = 0usize;
                for inner_idx in (header_op_idx + 1)..op_idx {
                    match ir.ops[inner_idx].kind.as_str() {
                        "loop_start" => nested_depth += 1,
                        "loop_end" => nested_depth = nested_depth.saturating_sub(1),
                        "loop_break_if_true" if nested_depth == 0 => {
                            loop_break_kinds.insert(header_bid, LoopBreakKind::BreakIfTrue);
                            break;
                        }
                        "loop_break_if_false" if nested_depth == 0 => {
                            loop_break_kinds.insert(header_bid, LoopBreakKind::BreakIfFalse);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (roles, loop_pairs, loop_break_kinds)
}

fn detect_loop_cond_blocks(ir: &FunctionIR, cfg: &CFG) -> HashMap<BlockId, BlockId> {
    let mut loop_cond_blocks = HashMap::new();
    let block_containing = |op_idx: usize| -> Option<BlockId> {
        cfg.blocks
            .iter()
            .position(|bb| bb.start_op <= op_idx && op_idx < bb.end_op)
            .map(|bid| BlockId(bid as u32))
    };
    let mut loop_stack: Vec<(usize, BlockId)> = Vec::new();
    for (op_idx, op) in ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                if let Some(header_bid) = block_containing(op_idx) {
                    loop_stack.push((op_idx, header_bid));
                }
            }
            "loop_end" => {
                loop_stack.pop();
            }
            "loop_break_if_true" | "loop_break_if_false" => {
                let Some((_, header_bid)) = loop_stack.last().copied() else {
                    continue;
                };
                let Some(cond_bid) = block_containing(op_idx) else {
                    continue;
                };
                loop_cond_blocks.entry(header_bid).or_insert(cond_bid);
            }
            _ => {}
        }
    }
    loop_cond_blocks
}

/// Forward type propagation for scalar return-relevant operation results.
///
/// Parameter signatures are seeded before this pass, so the same canonical
/// scalar result inference used by TIR refinement can derive return contracts
/// from typed parameters, constants, and prior propagation. Container and
/// aggregate types are deliberately left to the full refinement pipeline after
/// TIR construction so this pre-assembly pass cannot duplicate richer type
/// lattice behavior.
/// This runs iteratively until no new types are discovered.
fn propagate_arithmetic_types(blocks: &[TirBlock], types: &mut HashMap<ValueId, TirType>) {
    let mut changed = true;
    while changed {
        changed = false;
        for block in blocks {
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                let result_id = op.results[0];
                // Skip if already typed
                if types.get(&result_id).is_some_and(|t| *t != TirType::DynBox) {
                    continue;
                }

                let operand_types: Vec<TirType> = op
                    .operands
                    .iter()
                    .map(|id| types.get(id).cloned().unwrap_or(TirType::DynBox))
                    .collect();
                if let Some(inferred) = super::type_refine::infer_scalar_return_result_type(
                    op.opcode,
                    &operand_types,
                    Some(&op.attrs),
                ) {
                    types.insert(result_id, inferred);
                    changed = true;
                }
            }
        }
    }
}

/// Convert a string type annotation to a `TirType`.
fn string_to_tir_type(s: &str) -> TirType {
    match s {
        "int" | "i64" => TirType::I64,
        "float" | "f64" => TirType::F64,
        _ => match TirType::from_type_hint(s) {
            TirType::UserClass(_) => TirType::DynBox,
            ty => ty,
        },
    }
}

fn param_string_to_tir_type(s: &str) -> TirType {
    match s {
        "i64" => TirType::DynBox,
        _ => string_to_tir_type(s),
    }
}

/// Infer the function return type by examining all Return terminators.
/// Uses a lattice meet to combine multiple return types.
fn infer_return_type(blocks: &[TirBlock], types: &HashMap<ValueId, TirType>) -> TirType {
    use super::blocks::Terminator;

    let mut result_type: Option<TirType> = None;

    for block in blocks {
        if let Terminator::Return { values } = &block.terminator {
            let ret_ty = if values.is_empty() {
                TirType::None
            } else {
                // Use the type of the first return value.
                values
                    .first()
                    .and_then(|vid| types.get(vid))
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            };

            result_type = Some(match result_type {
                None => ret_ty,
                Some(existing) => existing.meet(&ret_ty),
            });
        }
    }

    result_type.unwrap_or(TirType::None)
}

// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

// ---------------------------------------------------------------------------
// Memory SSA: cell-based locals → store_var/load_var rewrite
// ---------------------------------------------------------------------------

/// Rewrite store_index/index on the function's locals cell list into
/// store_var/load_var ops. This is a form of Memory SSA that enables
/// the standard SSA algorithm to track local variable mutations through
/// loop iterations.
///
/// The Molt frontend stores ALL local variables in a cell list:
///   missing → v0; list_new(v0) → cell_list
///   store_index(cell_list, const_N, value)  // assign local[N] = value
///   index(cell_list, const_N) → v           // read local[N]
///
/// After rewrite:
///   store_var(_cell_N, value)  // SSA-visible assignment
///   load_var(_cell_N) → v     // SSA-visible read
///
/// For non-escaping cell lists, the store_index/index ops are replaced with
/// store_var/load_var because no runtime observer can see the heap cell.
/// Escaping cells (for example closure cells captured through a tuple_new
/// environment) must remain heap-backed so later function calls see the
/// mutated cell value.
/// Returns true if any rewrites were applied.
fn rewrite_cell_locals_to_store_load(ops: &mut [crate::ir::OpIR]) -> bool {
    use crate::ir::OpIR;
    use std::collections::{HashMap, HashSet};

    // Step 1: identify candidate cell list variables.
    // The pattern is: missing → X; list_new(X) → CELL_LIST
    // A cell list_new has exactly one argument that was produced by a `missing`
    // op.  User-created list literals (e.g. `out = []`) have zero arguments
    // and must NOT be mistaken for a cell variable.
    //
    // If the function already contains frontend-emitted store_var ops (the
    // frontend now emits store_var/load_var for non-boxed locals), skip the
    // cell rewrite entirely — the SSA pass already has explicit variable
    // tracking and the rewrite would misidentify user lists as cells.
    let has_frontend_store_var = ops.iter().any(|op| op.kind == "store_var");
    if has_frontend_store_var {
        return false;
    }
    let mut missing_outputs: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        if op.kind == "missing"
            && let Some(out) = &op.out
        {
            missing_outputs.insert(out.clone());
        }
    }
    let mut cell_vars: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        if op.kind == "list_new"
            && let Some(out) = &op.out
        {
            // A cell list_new has exactly one arg that is a missing sentinel.
            if let Some(args) = &op.args
                && args.len() == 1
                && missing_outputs.contains(&args[0])
            {
                cell_vars.insert(out.clone());
            }
        }
    }
    if cell_vars.is_empty() {
        return false; // No cell lists — nothing to rewrite.
    }

    // A cell escapes if it is used as anything other than the container operand
    // of index/store_index. Closure environments are the critical case:
    // tuple_new(cell) followed by func_new_closure must keep the physical cell
    // live, otherwise the closure will keep seeing the initial missing value.
    let mut escaped_cells: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        let Some(args) = &op.args else {
            continue;
        };
        for (arg_idx, arg) in args.iter().enumerate() {
            if !cell_vars.contains(arg) {
                continue;
            }
            let container_access = matches!(op.kind.as_str(), "index" | "store_index")
                && arg_idx == 0
                && args
                    .iter()
                    .enumerate()
                    .all(|(idx, candidate)| candidate != arg || idx == 0);
            if !container_access {
                escaped_cells.insert(arg.clone());
            }
        }
    }
    cell_vars.retain(|cell| !escaped_cells.contains(cell));
    if cell_vars.is_empty() {
        return false;
    }

    // Step 2: find all constant slots used with this cell list.
    // We need to map (cell_var, const_slot_value) → synthetic variable name.
    // The const_slot_value is in the `value` field of a `const` op whose
    // output is used as the second arg of store_index/index.
    //
    // Build a map: const_output_var → const_value (for slot indices).
    let mut const_values: HashMap<String, i64> = HashMap::new();
    for op in ops.iter() {
        if op.kind == "const"
            && let (Some(out), Some(val)) = (&op.out, op.value)
        {
            const_values.insert(out.clone(), val);
        }
    }

    // Step 2b: identify which slots hold non-scalar values (lists, dicts, etc.)
    // by checking what's stored at each slot. If a slot is assigned the output
    // of list_new, dict_new, etc., it holds a heap object and must NOT be
    // converted to a scalar store_var/load_var.
    let mut heap_slots: HashSet<(String, i64)> = HashSet::new();
    {
        // Map: var_name → producing op kind
        let mut var_producers: HashMap<String, String> = HashMap::new();
        for op in ops.iter() {
            if let Some(out) = &op.out {
                var_producers.insert(out.clone(), op.kind.clone());
            }
        }
        // Check each store_index: if the value arg was produced by a heap-allocating op,
        // mark that slot as heap.
        let heap_ops: HashSet<&str> = [
            "list_new",
            "dict_new",
            "set_new",
            "tuple_new",
            "call",
            "call_method",
            "call_function",
            "call_builtin",
            "CALL_BIND",
            "call_bind",
        ]
        .iter()
        .copied()
        .collect();
        for op in ops.iter() {
            if op.kind == "store_index"
                && let Some(args) = &op.args
                && args.len() == 3
                && cell_vars.contains(&args[0])
                && let Some(&slot_val) = const_values.get(&args[1])
            {
                let value_var = &args[2];
                if let Some(producer) = var_producers.get(value_var)
                    && heap_ops.contains(producer.as_str())
                {
                    heap_slots.insert((args[0].clone(), slot_val));
                }
            }
        }
    }

    // Step 3: scan for store_index and index ops on the cell list.
    // Only convert SCALAR slots (not heap slots) to store_var/load_var.
    let mut replacements: Vec<(usize, OpIR)> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        if let Some(args) = &op.args {
            if op.kind == "store_index" && args.len() == 3 && cell_vars.contains(&args[0]) {
                if let Some(&slot_val) = const_values.get(&args[1]) {
                    if heap_slots.contains(&(args[0].clone(), slot_val)) {
                        // Skip heap slots — lists, dicts, etc. must stay as cell ops.
                        continue;
                    }
                    let var_name = format!("_cell_{}_{}", args[0], slot_val);
                    replacements.push((
                        i,
                        OpIR {
                            kind: "store_var".to_string(),
                            var: Some(var_name),
                            args: Some(vec![args[2].clone()]),
                            ..OpIR::default()
                        },
                    ));
                }
            } else if op.kind == "index"
                && args.len() == 2
                && cell_vars.contains(&args[0])
                && let Some(&slot_val) = const_values.get(&args[1])
            {
                if heap_slots.contains(&(args[0].clone(), slot_val)) {
                    continue; // Skip heap slots.
                }
                if let Some(out) = &op.out {
                    let var_name = format!("_cell_{}_{}", args[0], slot_val);
                    replacements.push((
                        i,
                        OpIR {
                            kind: "load_var".to_string(),
                            var: Some(var_name),
                            out: Some(out.clone()),
                            ..OpIR::default()
                        },
                    ));
                }
            }
        }
    }

    if replacements.is_empty() {
        return false; // No cell locals to rewrite.
    }

    // Apply all replacements (store_index → store_var, index → load_var).
    for (idx, new_op) in &replacements {
        ops[*idx] = new_op.clone();
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::blocks::Terminator;
    use crate::tir::ops::OpCode;
    use crate::tir::types::TirType;

    /// Helper: build a FunctionIR with given name, params, and ops.
    fn make_func(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            params: params.iter().map(|s| s.to_string()).collect(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
        }
    }

    #[test]
    fn lower_functions_to_tir_module_skips_externs_and_aligns_idx() {
        // [non-extern "a", extern "ext", non-extern "b"] → module has {a, b}
        // (extern skipped), idx_map aligns module position → original index.
        let mut ext = make_func("ext", &[], vec![op("ret_void")]);
        ext.is_extern = true;
        let funcs = vec![
            make_func("a", &[], vec![op("ret_void")]),
            ext,
            make_func("b", &[], vec![op("ret_void")]),
        ];
        let (module, idx_map) = lower_functions_to_tir_module(&funcs);
        assert_eq!(module.functions.len(), 2, "externs are skipped");
        assert_eq!(idx_map, vec![0, 2], "module position maps to source index");
        assert_eq!(module.functions[0].name, "a");
        assert_eq!(module.functions[1].name, "b");
    }

    /// Helper to create an `OpIR` with just a `kind`.
    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `value`, and `out`.
    fn op_val_out(kind: &str, value: i64, out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `args`, and `out`.
    fn op_args_out(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `args`.
    fn op_args(kind: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    /// Helper: create an op with integer compatibility hint.
    fn op_fast_int(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_int: Some(true),
            ..OpIR::default()
        }
    }

    /// Helper: create an op with float compatibility hint.
    fn op_fast_float(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_float: Some(true),
            ..OpIR::default()
        }
    }

    #[test]
    fn cell_rewrite_skips_cells_escaped_into_closure_tuple() {
        let mut ops = vec![
            op_args_out("missing", &[], "missing"),
            op_args_out("list_new", &["missing"], "cell"),
            op_val_out("const", 0, "zero"),
            op_val_out("const", 7, "value"),
            op_args("store_index", &["cell", "zero", "value"]),
            op_args_out("tuple_new", &["cell"], "closure"),
            op_args_out("index", &["cell", "zero"], "loaded"),
        ];

        assert!(!rewrite_cell_locals_to_store_load(&mut ops));
        assert_eq!(ops[4].kind, "store_index");
        assert_eq!(ops[6].kind, "index");
    }

    #[test]
    fn cell_rewrite_handles_multiple_unescaped_cells_independently() {
        let mut ops = vec![
            op_args_out("missing", &[], "missing_a"),
            op_args_out("list_new", &["missing_a"], "cell_a"),
            op_args_out("missing", &[], "missing_b"),
            op_args_out("list_new", &["missing_b"], "cell_b"),
            op_val_out("const", 0, "zero"),
            op_val_out("const", 1, "value_a"),
            op_args("store_index", &["cell_a", "zero", "value_a"]),
            op_args_out("index", &["cell_a", "zero"], "loaded_a"),
            op_val_out("const", 2, "value_b"),
            op_args("store_index", &["cell_b", "zero", "value_b"]),
            op_args_out("index", &["cell_b", "zero"], "loaded_b"),
        ];

        assert!(rewrite_cell_locals_to_store_load(&mut ops));
        assert_eq!(ops[6].kind, "store_var");
        assert_eq!(ops[6].var.as_deref(), Some("_cell_cell_a_0"));
        assert_eq!(ops[7].kind, "load_var");
        assert_eq!(ops[7].var.as_deref(), Some("_cell_cell_a_0"));
        assert_eq!(ops[9].kind, "store_var");
        assert_eq!(ops[9].var.as_deref(), Some("_cell_cell_b_0"));
        assert_eq!(ops[10].kind, "load_var");
        assert_eq!(ops[10].var.as_deref(), Some("_cell_cell_b_0"));
    }

    // =======================================================================
    // Test 1: Trivial function — const + add + ret
    // =======================================================================
    #[test]
    fn trivial_function_lowering() {
        let func_ir = make_func(
            "test_add",
            &[],
            vec![
                op_val_out("const", 1, "x"),
                op_args_out("add", &["x"], "y"),
                op_args("ret", &["y"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "test_add");
        assert!(!tir.blocks.is_empty(), "should have at least one block");
        assert!(tir.blocks.contains_key(&tir.entry_block));

        // Should have exactly 1 block for straight-line code.
        assert_eq!(tir.blocks.len(), 1);

        // Entry block should have 2 ops (const + add; ret is structural).
        let entry = &tir.blocks[&tir.entry_block];
        // 3 ops: ConstNone (SSA undef sentinel) + ConstInt + Add; ret is structural.
        assert_eq!(
            entry.ops.len(),
            3,
            "entry should have undef sentinel, const, and add ops"
        );

        // Terminator should be Return.
        assert!(
            matches!(entry.terminator, Terminator::Return { .. }),
            "expected Return terminator, got {:?}",
            entry.terminator
        );
    }

    // =======================================================================
    // Test 2: Function with if/else control flow
    // =======================================================================
    #[test]
    fn if_else_control_flow() {
        let func_ir = make_func(
            "test_branch",
            &[],
            vec![
                op_val_out("const", 0, "c"), // 0 entry
                op_args("if", &["c"]),       // 1 ends entry
                op_val_out("const", 1, "x"), // 2 then
                op("else"),                  // 3 else
                op_val_out("const", 2, "x"), // 4 else body
                op("end_if"),                // 5 join
                op_args("ret", &["x"]),      // 6 return
            ],
        );

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "test_branch");
        assert!(
            tir.blocks.len() >= 3,
            "if/else should produce at least 3 blocks"
        );

        // Find the join block — it should have a block argument for `x`.
        let join_block = tir.blocks.values().find(|b| !b.args.is_empty());
        assert!(
            join_block.is_some(),
            "should have a join block with block arguments"
        );
        let join = join_block.unwrap();
        assert_eq!(
            join.args.len(),
            1,
            "join block should have 1 block arg (for x)"
        );

        // There should be a block with a CondBranch terminator (the block
        // containing the `if` op — which may or may not be the entry block,
        // depending on how the CFG splits).
        let has_cond_branch = tir
            .blocks
            .values()
            .any(|b| matches!(b.terminator, Terminator::CondBranch { .. }));
        assert!(
            has_cond_branch,
            "should have a block with CondBranch terminator"
        );
    }

    #[test]
    fn module_import_preserves_operand_through_lower_to_tir() {
        let func_ir = make_func(
            "module_import_shape",
            &["__molt_module_obj__"],
            vec![
                OpIR {
                    kind: "line".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("builtins".to_string()),
                    out: Some("v62".to_string()),
                    ..OpIR::default()
                },
                op_args_out("module_import", &["v62"], "v63"),
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("_builtins".to_string()),
                    out: Some("v64".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".to_string(),
                    args: Some(vec![
                        "__molt_module_obj__".to_string(),
                        "v64".to_string(),
                        "v63".to_string(),
                    ]),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                op("ret_void"),
            ],
        );

        let tir = lower_to_tir(&func_ir);
        let import_op = tir
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == crate::tir::ops::OpCode::Import)
            .expect("expected import op");
        assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
    }

    #[test]
    fn gpu_thread_id_lowers_to_runtime_backed_call_in_tir() {
        let func_ir = make_func(
            "gpu_tid",
            &[],
            vec![
                OpIR {
                    kind: "gpu_thread_id".to_string(),
                    out: Some("tid".to_string()),
                    ..OpIR::default()
                },
                op_args("ret", &["tid"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);
        let call_op = tir
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == crate::tir::ops::OpCode::Call)
            .expect("expected gpu_thread_id to lower to a call op");
        assert_eq!(
            call_op.attrs.get("s_value"),
            Some(&crate::tir::ops::AttrValue::Str(
                "molt_gpu_thread_id".to_string()
            ))
        );
        assert_eq!(
            call_op.attrs.get("_original_kind"),
            Some(&crate::tir::ops::AttrValue::Str(
                "gpu_thread_id".to_string()
            ))
        );
    }

    // =======================================================================
    // Test 3: transport hints do not seed canonical SSA types
    // =======================================================================
    #[test]
    fn transport_hints_do_not_seed_canonical_types() {
        let func_ir = FunctionIR {
            name: "hint_only_add".into(),
            params: vec!["a".into(), "b".into(), "fa".into(), "fb".into()],
            ops: vec![
                op_fast_int("add", &["a", "b"], "c"),
                op_fast_float("mul", &["fa", "fb"], "fc"),
                op_args("ret", &["c"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "transport-only hints must not seed canonical TIR types"
        );
        for op in tir.blocks.values().flat_map(|block| &block.ops) {
            assert!(
                !op.attrs.contains_key("_fast_int"),
                "SimpleIR fast_int metadata must not enter TIR attrs: {op:?}"
            );
            assert!(
                !op.attrs.contains_key("_fast_float"),
                "SimpleIR fast_float metadata must not enter TIR attrs: {op:?}"
            );
        }
    }

    // =======================================================================
    // Test 4: Empty function
    // =======================================================================
    #[test]
    fn empty_function() {
        let func_ir = make_func("empty", &[], vec![]);
        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "empty");
        // Empty ops → empty CFG → no blocks from SSA.
        assert!(tir.blocks.is_empty());
    }

    // =======================================================================
    // Test 5: Function with param_types annotation
    // =======================================================================
    #[test]
    fn param_types_from_annotation() {
        let func_ir = FunctionIR {
            name: "typed_add".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            ops: vec![op_args_out("add", &["a", "b"], "c"), op_args("ret", &["c"])],
            param_types: Some(vec!["int".to_string(), "float".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.param_types.len(), 2);
        assert_eq!(tir.param_types[0], TirType::I64);
        assert_eq!(tir.param_types[1], TirType::F64);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&TirType::I64),
            "entry param i64 fact must be present in the function-owned map"
        );
        assert_eq!(
            tir.value_types.get(&entry.args[1].id),
            Some(&TirType::F64),
            "entry param f64 fact must be present in the function-owned map"
        );
        let add_result = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Add)
            .and_then(|op| op.results.first())
            .copied()
            .expect("typed add result");
        assert_eq!(
            tir.value_types.get(&add_result),
            Some(&TirType::F64),
            "arithmetic propagation must persist op-result facts on TirFunction"
        );
    }

    #[test]
    fn compound_param_types_from_annotation() {
        let func_ir = FunctionIR {
            name: "typed_container".to_string(),
            params: vec!["items".to_string()],
            ops: vec![op_args("ret", &["items"])],
            param_types: Some(vec!["list[int]".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);
        let expected = TirType::List(Box::new(TirType::I64));

        assert_eq!(tir.param_types, vec![expected.clone()]);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&expected),
            "entry param compound type fact must be present in the function-owned map"
        );
        assert_eq!(
            entry.args[0].ty, expected,
            "entry param argument must carry the structured compound type"
        );
    }

    #[test]
    fn abi_i64_param_type_is_not_a_semantic_int_fact() {
        let func_ir = FunctionIR {
            name: "boxed_carrier".to_string(),
            params: vec!["obj".to_string()],
            ops: vec![op_args("ret", &["obj"])],
            param_types: Some(vec!["i64".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.param_types, vec![TirType::DynBox]);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&TirType::DynBox),
            "native ABI carrier `i64` must stay a boxed dynamic value, not semantic I64"
        );
    }

    #[test]
    fn exception_region_drop_marker_round_trips_without_full_drop_gate() {
        let func_ir = FunctionIR {
            name: "exception_marker_transport".to_string(),
            params: vec![],
            ops: vec![
                op(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
                op("ret_void"),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert!(matches!(
            tir.attrs
                .get(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            Some(crate::tir::ops::AttrValue::Bool(true))
        ));
        assert!(
            !tir.attrs
                .contains_key(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            "exception-only marker must not be promoted to the native full-RC gate"
        );
        assert!(
            tir.blocks[&tir.entry_block]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::Copy),
            "transport marker must be stripped before TIR op assembly"
        );
    }

    // =======================================================================
    // Test 6: string_to_tir_type coverage
    // =======================================================================
    #[test]
    fn string_type_conversion() {
        assert_eq!(string_to_tir_type("int"), TirType::I64);
        assert_eq!(string_to_tir_type("i64"), TirType::I64);
        assert_eq!(string_to_tir_type("float"), TirType::F64);
        assert_eq!(string_to_tir_type("f64"), TirType::F64);
        assert_eq!(string_to_tir_type("bool"), TirType::Bool);
        assert_eq!(string_to_tir_type("str"), TirType::Str);
        assert_eq!(string_to_tir_type("bytes"), TirType::Bytes);
        assert_eq!(string_to_tir_type("None"), TirType::None);
        assert_eq!(string_to_tir_type("none"), TirType::None);
        assert_eq!(
            string_to_tir_type("list[int]"),
            TirType::List(Box::new(TirType::I64))
        );
        assert_eq!(
            string_to_tir_type("dict[str, float]"),
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::F64))
        );
        assert_eq!(string_to_tir_type("unknown_type"), TirType::DynBox);
    }
}
