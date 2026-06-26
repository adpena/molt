use crate::{FunctionIR, OpIR};

fn is_lowered_block_arg_store(op: &OpIR) -> bool {
    op.kind == "store_var"
        && op
            .var
            .as_deref()
            .is_some_and(|var| var.starts_with("_bb") && var.contains("_arg"))
}

fn skip_lowered_block_arg_stores(ops: &[OpIR], mut idx: usize) -> usize {
    while idx < ops.len() && is_lowered_block_arg_store(&ops[idx]) {
        idx += 1;
    }
    idx
}

fn lowered_block_arg_store_start(ops: &[OpIR], mut idx: usize) -> usize {
    while idx > 0 && is_lowered_block_arg_store(&ops[idx - 1]) {
        idx -= 1;
    }
    idx
}

fn op_first_arg(op: &OpIR) -> Option<&str> {
    op.args
        .as_ref()
        .and_then(|args| args.first())
        .map(String::as_str)
}

fn remove_marked_ops(ops: &mut Vec<OpIR>, remove: Vec<bool>) -> bool {
    if !remove.iter().any(|remove_op| *remove_op) {
        return false;
    }
    *ops = ops
        .iter()
        .enumerate()
        .filter(|(idx, _)| !remove[*idx])
        .map(|(_, op)| op.clone())
        .collect();
    true
}

fn raise_flows_directly_to_target(ops: &[OpIR], raise_idx: usize, target: i64) -> bool {
    let mut idx = skip_lowered_block_arg_stores(ops, raise_idx + 1);
    if idx < ops.len() && ops[idx].kind == "check_exception" && ops[idx].value == Some(target) {
        idx = skip_lowered_block_arg_stores(ops, idx + 1);
    }
    idx < ops.len() && ops[idx].kind == "jump" && ops[idx].value == Some(target)
}

/// Canonicalize explicit raise exits after the TIR roundtrip.
///
/// Lowering materializes exception-handler block arguments at every
/// `check_exception` edge. An explicit `raise` followed by an unconditional
/// jump to the same handler already carries required state on that jump; the
/// extra `check_exception` edge only duplicates the same block-argument stores.
/// For canonical builtin exception construction, the constructor is also
/// non-throwing, so a pre-raise handler poll is redundant.
pub fn canonicalize_direct_raise_edges(func_ir: &mut FunctionIR) {
    if !func_ir.ops.iter().any(|op| op.kind == "raise") {
        return;
    }

    let mut remove = vec![false; func_ir.ops.len()];
    for raise_idx in 0..func_ir.ops.len() {
        if func_ir.ops[raise_idx].kind != "raise" {
            continue;
        }
        let check_idx = skip_lowered_block_arg_stores(&func_ir.ops, raise_idx + 1);
        if check_idx >= func_ir.ops.len() || func_ir.ops[check_idx].kind != "check_exception" {
            continue;
        }
        let Some(target) = func_ir.ops[check_idx].value else {
            continue;
        };
        let jump_idx = skip_lowered_block_arg_stores(&func_ir.ops, check_idx + 1);
        if jump_idx >= func_ir.ops.len()
            || func_ir.ops[jump_idx].kind != "jump"
            || func_ir.ops[jump_idx].value != Some(target)
        {
            continue;
        }
        for slot in remove.iter_mut().take(check_idx + 1).skip(raise_idx + 1) {
            *slot = true;
        }
    }
    remove_marked_ops(&mut func_ir.ops, remove);

    let mut remove = vec![false; func_ir.ops.len()];
    for check_idx in 0..func_ir.ops.len() {
        if func_ir.ops[check_idx].kind != "check_exception" {
            continue;
        }
        let Some(target) = func_ir.ops[check_idx].value else {
            continue;
        };
        let raise_idx = check_idx + 1;
        if raise_idx >= func_ir.ops.len() || func_ir.ops[raise_idx].kind != "raise" {
            continue;
        }
        if !raise_flows_directly_to_target(&func_ir.ops, raise_idx, target) {
            continue;
        }
        let Some(raise_arg) = op_first_arg(&func_ir.ops[raise_idx]) else {
            continue;
        };
        let store_start = lowered_block_arg_store_start(&func_ir.ops, check_idx);
        let Some(producer_idx) = store_start.checked_sub(1) else {
            continue;
        };
        let producer = &func_ir.ops[producer_idx];
        if !matches!(
            producer.kind.as_str(),
            "exception_new_builtin" | "exception_new_builtin_empty" | "exception_new_builtin_one"
        ) || producer.out.as_deref() != Some(raise_arg)
        {
            continue;
        }
        for slot in remove.iter_mut().take(check_idx + 1).skip(store_start) {
            *slot = true;
        }
    }
    remove_marked_ops(&mut func_ir.ops, remove);
}
