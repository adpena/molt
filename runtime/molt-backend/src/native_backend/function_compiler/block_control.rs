use super::*;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn collect_slot_backed_join_names(
    ops: &[OpIR],
    exception_label_ids: &BTreeSet<i64>,
    stateful: bool,
) -> BTreeSet<String> {
    let mut slot_backed_join_names: BTreeSet<String> = BTreeSet::new();

    // Join carriers that are explicitly materialized with store/load in the IR
    // are memory-backed transport by construction. Keep them on the stack-backed
    // path so later label materialization does not try to reinterpret them as
    // structured phi joins.
    for op in ops {
        if matches!(op.kind.as_str(), "store_var" | "delete_var")
            && let Some(name) = op.var.as_ref().or(op.out.as_ref())
            && is_join_slot_name(name)
        {
            slot_backed_join_names.insert(name.clone());
        }
    }

    // Stateful functions (generators / async / comprehension polls) carry their
    // SSA values across state_yield / state_label resume points the same way
    // exception-bearing functions carry values across check_exception splits.
    // The state machine generates many block edges that aren't eagerly sealed,
    // so phi resolution at seal_all_blocks() time can explode block-parameter
    // counts past regalloc2's u32-indexed entity tables (u32::MAX panic).
    //
    // Treat stateful functions like exception functions: route all store_var
    // targets through stack slots so the state machine carries memory values,
    // not SSA values, across resume edges.
    if exception_label_ids.is_empty() && !stateful {
        return slot_backed_join_names;
    }

    let mut exception_region_depth = 0i32;
    let mut first_seen_join_in_exception: BTreeMap<String, bool> = BTreeMap::new();
    let mut exception_written_locals: BTreeSet<String> = BTreeSet::new();

    // Collect ALL persistent local-slot mutation targets that appear anywhere
    // in a function with exception handling or stateful resume points. When the function defers
    // block sealing to seal_all_blocks(), Cranelift must resolve SSA phi
    // nodes for every variable that has definitions reaching from different
    // predecessors. Each check_exception or state_yield creates a new block
    // split, and variables carried across these splits become block
    // parameters. In functions with many such splits (e.g. try/except
    // bodies, generator/async poll state machines), the block parameter
    // count explodes and can overflow regalloc2's internal index tables
    // (u32::MAX index panic).
    //
    // By routing all persistent local storage through stack slots instead of SSA
    // variables, we eliminate the phi nodes entirely. Stack loads/stores
    // are slightly slower than register-to-register moves, but:
    // 1. Exception-handling and poll functions are already on the cold path
    // 2. The alternative is a hard backend compile failure
    // 3. regalloc2 phi resolution for many-predecessor blocks is O(n^2)
    //
    // This is the same strategy used by LLVM's mem2reg in the presence of
    // exception handling: keep values in memory across EH boundaries.
    let mut all_store_var_targets: BTreeSet<String> = BTreeSet::new();
    for op in ops {
        if matches!(op.kind.as_str(), "store_var" | "delete_var")
            && let Some(name) = op.var.as_ref().or(op.out.as_ref())
            && is_persistent_local_slot_name(name)
        {
            all_store_var_targets.insert(name.clone());
        }
    }
    // All persistent store_var targets in exception-bearing or stateful functions
    // use stack slots. Compiler SSA temps remain SSA values; they are not Python
    // local storage and widening them to stack slots can erase representation
    // facts at check_exception boundaries.
    slot_backed_join_names.extend(all_store_var_targets);

    for op in ops {
        match op.kind.as_str() {
            "try_start" => {
                exception_region_depth += 1;
            }
            "exception_pop" => {
                exception_region_depth = (exception_region_depth - 1).max(0);
            }
            "store_var" | "delete_var" if exception_region_depth > 0 => {
                if let Some(name) = op.var.as_ref().or(op.out.as_ref())
                    && is_persistent_local_slot_name(name)
                {
                    exception_written_locals.insert(name.clone());
                    if is_join_slot_name(name) {
                        first_seen_join_in_exception
                            .entry(name.clone())
                            .or_insert(true);
                    }
                }
            }
            "copy_var" | "load_var" if exception_region_depth > 0 => {
                let candidate = op
                    .var
                    .as_ref()
                    .or_else(|| op.args.as_ref().and_then(|args| args.first()));
                if let Some(name) = candidate
                    && is_join_slot_name(name)
                {
                    first_seen_join_in_exception
                        .entry(name.clone())
                        .or_insert(true);
                }
            }
            _ => {}
        }
    }
    for (name, in_exception) in first_seen_join_in_exception {
        if in_exception {
            slot_backed_join_names.insert(name);
        }
    }
    slot_backed_join_names.extend(exception_written_locals);
    slot_backed_join_names
}

#[cfg(feature = "native-backend")]
#[cfg(test)]
pub(in crate::native_backend::function_compiler) fn live_exception_rebind_vars_for_op(
    vars: &BTreeMap<String, Variable>,
    transport_last_use: &BTreeMap<String, usize>,
    first_defined_at: &BTreeMap<String, usize>,
    op_idx: usize,
) -> BTreeMap<String, Variable> {
    vars.iter()
        .filter_map(|(name, var)| {
            let last = transport_last_use.get(name).copied().unwrap_or(usize::MAX);
            let has_reaching_def = first_defined_at
                .get(name)
                .copied()
                .is_some_and(|first| first <= op_idx);
            (has_reaching_def && last > op_idx).then_some((name.clone(), *var))
        })
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn switch_to_block_with_rebind(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
    _has_exception_labels: bool,
) {
    crate::switch_to_block_tracking(builder, block, is_block_filled);
    // Do not synthesize implicit SSA transport here.
    //
    // Cranelift materializes missing `use_var` state at a block switch by
    // appending block params. That is only correct when the predecessor edges
    // explicitly transport those values. Merge payloads and exception
    // fallthrough state must therefore be modeled by real block params or
    // slot-backed joins at the call site, not by opportunistic rebinding here.
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn materialize_label_block(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    ensure_block_in_layout(builder, block);
    // If we're already inside `block` and it's still open, the label has
    // effectively materialised in place — do not emit a self-jump to itself,
    // which would (a) close the block, (b) wire it as its own predecessor,
    // and (c) generate an unreachable trailing instruction. The
    // `is_block_filled` guard alone is not sufficient because a fresh
    // resume block created by `state_yield` lowering also has
    // `is_block_filled == false` while already being the current block.
    let already_in_target = builder.current_block() == Some(block);
    if !already_in_target {
        if !*is_block_filled {
            jump_block(builder, block, &[]);
        }
        crate::switch_to_block_tracking(builder, block, is_block_filled);
    }
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn switch_to_block_materialized(
    builder: &mut FunctionBuilder,
    block: Block,
) {
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
}
