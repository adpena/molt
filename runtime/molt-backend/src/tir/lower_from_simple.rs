//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, LoopBreakKind, LoopRole, TirBlock};
use super::cfg::CFG;
use super::function::TirFunction;
use super::ssa::{SsaOutput, convert_to_ssa_with_name_and_params};
use super::types::TirType;
use super::values::ValueId;

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
    // Also rewrite cell-based locals (store_index/index on list) to
    // store_var/load_var for the same reason.
    // Memory SSA: rewrite cell-based locals to store_var/load_var so SSA
    // creates proper phi nodes at loop headers for cell variables.
    // Memory SSA default ON: rewrite cell locals to store_var/load_var.
    let _cell_rewrite_applied = if std::env::var("MOLT_TIR_CELL_SSA").is_ok() {
        rewrite_cell_locals_to_store_load(&mut working_ops)
    } else {
        false
    };

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
    assemble_function(ir_ref, &cfg, ssa)
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
fn rewrite_loop_index_to_store_load(ops: &[crate::ir::OpIR]) -> Vec<crate::ir::OpIR> {
    use crate::ir::OpIR;

    // Quick scan: any loop_index_start ops?
    let has_loop_index = ops.iter().any(|op| op.kind == "loop_index_start");
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
        blocks: tir_blocks,
        mut types,
        next_value,
    } = ssa;

    // Forward type propagation: when all operands of an arithmetic op are
    // known-typed (e.g., I64), infer the result type.
    propagate_arithmetic_types(&tir_blocks, &mut types);

    // Determine parameter types — default to DynBox, but honour param_types if
    // the frontend provided string annotations.
    let param_types: Vec<TirType> = if let Some(ref pt) = ir.param_types {
        pt.iter().map(|s| string_to_tir_type(s)).collect()
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
            a
        },
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

/// Forward type propagation for arithmetic and comparison ops.
///
/// When all operands of an Add/Sub/Mul/etc. are I64 (from param_types,
/// const hints, or prior propagation), the result is also I64.
/// This runs iteratively until no new types are discovered.
fn propagate_arithmetic_types(blocks: &[TirBlock], types: &mut HashMap<ValueId, TirType>) {
    use super::ops::OpCode;
    let arithmetic_ops = [
        OpCode::Add,
        OpCode::Sub,
        OpCode::Mul,
        OpCode::InplaceAdd,
        OpCode::InplaceSub,
        OpCode::InplaceMul,
    ];
    let comparison_ops = [
        OpCode::Lt,
        OpCode::Le,
        OpCode::Gt,
        OpCode::Ge,
        OpCode::Eq,
        OpCode::Ne,
    ];

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

                if arithmetic_ops.contains(&op.opcode) && op.operands.len() == 2 {
                    let lhs_ty = types.get(&op.operands[0]);
                    let rhs_ty = types.get(&op.operands[1]);
                    match (lhs_ty, rhs_ty) {
                        (Some(TirType::I64), Some(TirType::I64)) => {
                            types.insert(result_id, TirType::I64);
                            changed = true;
                        }
                        (Some(TirType::F64), _) | (_, Some(TirType::F64)) => {
                            types.insert(result_id, TirType::F64);
                            changed = true;
                        }
                        _ => {}
                    }
                } else if comparison_ops.contains(&op.opcode) && op.operands.len() == 2 {
                    // Comparison results are always Bool
                    let lhs_ty = types.get(&op.operands[0]);
                    let rhs_ty = types.get(&op.operands[1]);
                    if lhs_ty.is_some_and(|t| t.is_numeric())
                        && rhs_ty.is_some_and(|t| t.is_numeric())
                    {
                        types.insert(result_id, TirType::Bool);
                        changed = true;
                    }
                } else if op.opcode == OpCode::ConstInt {
                    types.insert(result_id, TirType::I64);
                    changed = true;
                } else if op.opcode == OpCode::ConstFloat {
                    types.insert(result_id, TirType::F64);
                    changed = true;
                } else if op.opcode == OpCode::ConstBool {
                    types.insert(result_id, TirType::Bool);
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
        "bool" => TirType::Bool,
        "str" => TirType::Str,
        "bytes" => TirType::Bytes,
        "None" | "none" => TirType::None,
        "list" => TirType::List(Box::new(TirType::DynBox)),
        "dict" => TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
        "set" => TirType::Set(Box::new(TirType::DynBox)),
        "tuple" => TirType::Tuple(vec![]),
        _ => TirType::DynBox,
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
/// The original store_index/index on the cell list are kept as-is (the
/// runtime still needs them for the actual cell storage), but ADDITIONAL
/// store_var/load_var ops are inserted so the SSA pass can track the
/// variable flow. The load_var output replaces subsequent uses of the
/// index output.
/// Returns true if any rewrites were applied.
fn rewrite_cell_locals_to_store_load(ops: &mut [crate::ir::OpIR]) -> bool {
    use crate::ir::OpIR;
    use std::collections::{HashMap, HashSet};

    // Step 1: identify the cell list variable.
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
    let mut cell_list_var: Option<String> = None;
    for op in ops.iter() {
        if op.kind == "list_new"
            && let Some(out) = &op.out
        {
            // A cell list_new has exactly one arg that is a missing sentinel.
            if let Some(args) = &op.args
                && args.len() == 1
                && missing_outputs.contains(&args[0])
            {
                cell_list_var = Some(out.clone());
                break;
            }
        }
    }
    let Some(cell_var) = cell_list_var else {
        return false; // No cell list — nothing to rewrite.
    };

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
    // by checking what's initially stored at each slot. If a slot is initialized
    // with the output of list_new, dict_new, etc., it holds a heap object and
    // must NOT be converted to a scalar store_var/load_var.
    let mut heap_slots: HashSet<i64> = HashSet::new();
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
                && args[0] == cell_var
                && let Some(&slot_val) = const_values.get(&args[1])
            {
                let value_var = &args[2];
                if let Some(producer) = var_producers.get(value_var)
                    && heap_ops.contains(producer.as_str())
                {
                    heap_slots.insert(slot_val);
                }
            }
        }
    }

    // Step 3: scan for store_index and index ops on the cell list.
    // Only convert SCALAR slots (not heap slots) to store_var/load_var.
    let mut replacements: Vec<(usize, OpIR)> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        if let Some(args) = &op.args {
            if op.kind == "store_index" && args.len() == 3 && args[0] == cell_var {
                if let Some(&slot_val) = const_values.get(&args[1]) {
                    if heap_slots.contains(&slot_val) {
                        // Skip heap slots — lists, dicts, etc. must stay as cell ops.
                        continue;
                    }
                    let var_name = format!("_cell_{}", slot_val);
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
                && args[0] == cell_var
                && let Some(&slot_val) = const_values.get(&args[1])
            {
                if heap_slots.contains(&slot_val) {
                    continue; // Skip heap slots.
                }
                if let Some(out) = &op.out {
                    let var_name = format!("_cell_{}", slot_val);
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
            Some(&crate::tir::ops::AttrValue::Str("molt_gpu_thread_id".to_string()))
        );
        assert_eq!(
            call_op.attrs.get("_original_kind"),
            Some(&crate::tir::ops::AttrValue::Str("gpu_thread_id".to_string()))
        );
    }

    // =======================================================================
    // Test 3: transport hints do not seed canonical SSA types
    // =======================================================================
    #[test]
    fn transport_hints_do_not_seed_canonical_types() {
        let func_ir = FunctionIR {
            name: "hint_only_add".into(),
            params: vec!["a".into(), "b".into()],
            ops: vec![op_fast_int("add", &["a", "b"], "c"), op_args("ret", &["c"])],
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
        assert_eq!(string_to_tir_type("unknown_type"), TirType::DynBox);
    }
}
