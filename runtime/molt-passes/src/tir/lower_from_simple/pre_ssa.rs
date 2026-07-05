//! Pre-SSA SimpleIR rewrites for TIR lowering.
//!
//! These transforms make loop carriers and cell-local mutations visible to
//! the shared SSA conversion before CFG assembly turns the stream into TIR.

use super::super::op_kinds_generated::simpleir_kind_is_pre_ssa_rewritten;

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

pub(super) fn rewrite_loop_index_to_store_load(ops: &[crate::ir::OpIR]) -> Vec<crate::ir::OpIR> {
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
pub(super) fn rewrite_cell_locals_to_store_load(ops: &mut [crate::ir::OpIR]) -> bool {
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
