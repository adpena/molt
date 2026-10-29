//! Pre-SSA SimpleIR rewrites for TIR lowering.
//!
//! These transforms make loop carriers and cell-local mutations visible to
//! the shared SSA conversion before CFG assembly turns the stream into TIR.

fn with_inherited_source_site(
    mut replacement: crate::ir::OpIR,
    source: &crate::ir::OpIR,
) -> crate::ir::OpIR {
    replacement.inherit_source_site_from(source);
    replacement
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
/// Consumes authored transport and returns normalized operations. An unchanged
/// stream retains its allocation. Malformed or orphaned carriers fail the
/// shared normalized-input contract rather than surviving into SSA passthrough.
pub fn normalize_loop_indices(
    function_name: &str,
    ops: Vec<crate::ir::OpIR>,
) -> Vec<crate::ir::OpIR> {
    use crate::ir::OpIR;

    // Quick scan: any loop-index op consumed by this pre-SSA rewrite?
    let has_loop_index = ops.iter().any(|op| op.kind.is_pre_ssa_rewritten());
    if !has_loop_index {
        return ops;
    }

    // Find carrier initializers and their lexical loop owner before emitting
    // stores. Key replacements by operation identity, not by a global spelling:
    // another loop may reuse a carrier name without owning an orphaned start.
    struct LoopIndexPattern {
        loop_start_idx: usize,
        source_op_idx: usize,
        var_name: String,
        init_arg: String,
    }

    let mut patterns: Vec<LoopIndexPattern> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();
    let mut seen_carriers = std::collections::HashSet::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind {
            molt_ir::OpKind::LoopStart => {
                loop_start_stack.push(idx);
            }
            molt_ir::OpKind::LoopEnd => {
                loop_start_stack.pop();
            }
            molt_ir::OpKind::LoopIndexStart => {
                if let Some(&ls_idx) = loop_start_stack.last() {
                    let var_name = op.out.clone().unwrap_or_default();
                    let init_arg = op.args.as_deref().and_then(|args| match args {
                        [initial] if !initial.is_empty() => Some(initial.clone()),
                        _ => None,
                    });
                    if !var_name.is_empty()
                        && var_name != "none"
                        && let Some(init_arg) = init_arg
                        && seen_carriers.insert((ls_idx, var_name.clone()))
                    {
                        patterns.push(LoopIndexPattern {
                            loop_start_idx: ls_idx,
                            source_op_idx: idx,
                            var_name,
                            init_arg,
                        });
                    }
                }
            }
            _ => {}
        }
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
    let rewrite_starts: std::collections::HashSet<usize> = patterns
        .iter()
        .map(|pattern| pattern.source_op_idx)
        .collect();
    let loop_carriers: std::collections::HashMap<usize, std::collections::HashSet<String>> =
        insert_before
            .iter()
            .map(|(&start, patterns)| {
                (
                    start,
                    patterns
                        .iter()
                        .map(|pattern| pattern.var_name.clone())
                        .collect(),
                )
            })
            .collect();

    let mut active_loop_carriers: Vec<usize> = Vec::new();
    let mut result: Vec<OpIR> = Vec::with_capacity(ops.len() + patterns.len());

    // Pass 2: emit rewritten ops.
    for (idx, op) in ops.iter().enumerate() {
        // Before a loop_start, insert store_var for each pattern.
        if let Some(pats) = insert_before.get(&idx) {
            for pat in pats {
                result.push(with_inherited_source_site(
                    OpIR {
                        kind: molt_ir::OpKind::StoreVar,
                        var: Some(pat.var_name.clone()),
                        args: Some(vec![pat.init_arg.clone()]),
                        ..OpIR::new(molt_ir::OpKind::StoreVar)
                    },
                    &ops[pat.source_op_idx],
                ));
            }
        }

        match op.kind {
            molt_ir::OpKind::LoopStart => {
                active_loop_carriers.push(idx);
                result.push(op.clone());
            }
            molt_ir::OpKind::LoopIndexStart => {
                let var_name = op.out.clone().unwrap_or_default();
                if rewrite_starts.contains(&idx) {
                    // Rewrite to load_var: read V from the phi.
                    result.push(with_inherited_source_site(
                        OpIR {
                            kind: molt_ir::OpKind::LoadVar,
                            var: Some(var_name.clone()),
                            out: Some(var_name),
                            ..OpIR::new(molt_ir::OpKind::LoadVar)
                        },
                        op,
                    ));
                } else {
                    result.push(op.clone());
                }
            }
            molt_ir::OpKind::LoopIndexNext => {
                // Physical nesting does not select the recurrence: inlined
                // cleanup may update an outer loop's explicitly named carrier.
                // Resolve that identity in the nearest declaring active scope.
                let carrier_name = op.out.as_ref().and_then(|name| {
                    active_loop_carriers
                        .iter()
                        .rev()
                        .find_map(|start| loop_carriers.get(start)?.get(name.as_str()))
                });
                let updated_arg = op.args.as_deref().and_then(|args| match args {
                    [updated] if !updated.is_empty() => Some(updated),
                    _ => None,
                });
                if let (Some(var_name), Some(updated_arg)) = (carrier_name, updated_arg) {
                    // Rewrite to store_var: update V.
                    result.push(with_inherited_source_site(
                        OpIR {
                            kind: molt_ir::OpKind::StoreVar,
                            var: Some(var_name.clone()),
                            args: Some(vec![updated_arg.clone()]),
                            ..OpIR::new(molt_ir::OpKind::StoreVar)
                        },
                        op,
                    ));
                } else {
                    result.push(op.clone());
                }
            }
            molt_ir::OpKind::LoopEnd => {
                active_loop_carriers.pop();
                result.push(op.clone());
            }
            _ => {
                result.push(op.clone());
            }
        }
    }

    molt_ir::ir_schema::validate_normalized_simple_ir_ops(function_name, &result)
        .unwrap_or_else(|error| panic!("counted-loop normalization failed: {error}"));
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
    let has_frontend_store_var = ops.iter().any(|op| op.kind == molt_ir::OpKind::StoreVar);
    if has_frontend_store_var {
        return false;
    }
    let mut missing_outputs: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        if op.kind == molt_ir::OpKind::Missing
            && let Some(out) = &op.out
        {
            missing_outputs.insert(out.clone());
        }
    }
    let mut cell_vars: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        if op.kind == molt_ir::OpKind::ListNew
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
            let container_access = matches!(
                op.kind,
                molt_ir::OpKind::Index | molt_ir::OpKind::StoreIndex
            ) && arg_idx == 0
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
        if op.kind == molt_ir::OpKind::Const
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
        let mut var_producers: HashMap<String, molt_ir::OpKind> = HashMap::new();
        for op in ops.iter() {
            if let Some(out) = &op.out {
                var_producers.insert(out.clone(), op.kind);
            }
        }
        // Check each store_index: if the value arg was produced by a heap-allocating op,
        // mark that slot as heap.
        let heap_ops: HashSet<molt_ir::OpKind> = [
            molt_ir::OpKind::ListNew,
            molt_ir::OpKind::DictNew,
            molt_ir::OpKind::SetNew,
            molt_ir::OpKind::TupleNew,
            molt_ir::OpKind::Call,
            molt_ir::OpKind::CallMethod,
            molt_ir::OpKind::CallFunction,
            molt_ir::OpKind::CallBuiltin,
            molt_ir::OpKind::CallBind,
        ]
        .iter()
        .copied()
        .collect();
        for op in ops.iter() {
            if op.kind == molt_ir::OpKind::StoreIndex
                && let Some(args) = &op.args
                && args.len() == 3
                && cell_vars.contains(&args[0])
                && let Some(&slot_val) = const_values.get(&args[1])
            {
                let value_var = &args[2];
                if let Some(producer) = var_producers.get(value_var)
                    && heap_ops.contains(producer)
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
            if op.kind == molt_ir::OpKind::StoreIndex
                && args.len() == 3
                && cell_vars.contains(&args[0])
            {
                if let Some(&slot_val) = const_values.get(&args[1]) {
                    if heap_slots.contains(&(args[0].clone(), slot_val)) {
                        // Skip heap slots — lists, dicts, etc. must stay as cell ops.
                        continue;
                    }
                    let var_name = format!("_cell_{}_{}", args[0], slot_val);
                    replacements.push((
                        i,
                        with_inherited_source_site(
                            OpIR {
                                kind: molt_ir::OpKind::StoreVar,
                                var: Some(var_name),
                                args: Some(vec![args[2].clone()]),
                                ..OpIR::new(molt_ir::OpKind::StoreVar)
                            },
                            op,
                        ),
                    ));
                }
            } else if op.kind == molt_ir::OpKind::Index
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
                        with_inherited_source_site(
                            OpIR {
                                kind: molt_ir::OpKind::LoadVar,
                                var: Some(var_name),
                                out: Some(out.clone()),
                                ..OpIR::new(molt_ir::OpKind::LoadVar)
                            },
                            op,
                        ),
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
