//! Backend-neutral IR rewrite/elision passes (moved verbatim from lib.rs).

use crate::ir::OpIR;

/// Pre-process phi ops into explicit store_var/load_var pairs.
///
/// The frontend emits `phi(then_val, else_val) -> out` after `end_if` to merge
/// values from if/else branches. The TIR pipeline converts structured
/// `if`/`else`/`end_if` into linearized `jump`/`label` ops but leaves `phi`
/// intact. The Cranelift backend's `phi` handler was a no-op, causing the phi
/// output to keep its entry-block None initialization.
///
/// This pass rewrites the phi pattern to explicit variable stores:
/// - In the then-branch (before `else` or `end_if`), insert `store_var out = arg0`
/// - In the else-branch (before `end_if`), insert `store_var out = arg1`
/// - Replace the `phi` op with `load_var out`
///
/// After this rewrite, the TIR SSA phase handles the merge correctly via block
/// arguments, and `lower_to_simple` emits proper `store_var`/`load_var` pairs.
pub fn rewrite_phi_to_store_load(ops: &mut Vec<OpIR>) {
    // Phase 1: find all if/else/end_if/phi patterns and collect rewrite info.
    // Build if_stack to match if/else/end_if.
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new(); // (if_idx, else_idx)
    let mut if_to_end_if: std::collections::BTreeMap<usize, usize> =
        std::collections::BTreeMap::new();
    let mut if_to_else: std::collections::BTreeMap<usize, usize> =
        std::collections::BTreeMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.kind {
            molt_ir::OpKind::If => if_stack.push((idx, None)),
            molt_ir::OpKind::Else => {
                if let Some(top) = if_stack.last_mut() {
                    top.1 = Some(idx);
                }
            }
            molt_ir::OpKind::EndIf => {
                if let Some((if_idx, else_idx)) = if_stack.pop() {
                    if_to_end_if.insert(if_idx, idx);
                    if let Some(ei) = else_idx {
                        if_to_else.insert(if_idx, ei);
                    }
                }
            }
            _ => {}
        }
    }

    // Phase 2: for each end_if, scan for following phi ops and collect rewrites.
    // Rewrites: Vec<(insert_before_else_or_end_if_idx, store_var_name, store_var_arg,
    //               insert_before_end_if_idx, store_var_name2, store_var_arg2,
    //               phi_idx, phi_out)>
    struct PhiRewrite {
        then_insert_idx: usize, // index to insert store_var for then-path
        then_arg: String,       // value name for then-path
        else_insert_idx: usize, // index to insert store_var for else-path
        else_arg: String,       // value name for else-path
        phi_idx: usize,         // index of the phi op to replace
        phi_out: String,        // output variable name
    }

    let mut rewrites: Vec<PhiRewrite> = Vec::new();

    for (&if_idx, &end_if_idx) in &if_to_end_if {
        let mut scan = end_if_idx + 1;
        while scan < ops.len() && ops[scan].kind == molt_ir::OpKind::Phi {
            let phi_op = &ops[scan];
            if let (Some(out), Some(args)) = (&phi_op.out, &phi_op.args)
                && args.len() == 2
                && out != "none"
            {
                let has_else = if_to_else.contains_key(&if_idx);
                let then_insert;
                let else_insert;
                if has_else {
                    // Insert then-path store_var just before the `else` op.
                    then_insert = *if_to_else.get(&if_idx).unwrap();
                    // Insert else-path store_var just before end_if.
                    else_insert = end_if_idx;
                } else {
                    // No explicit else: the else-path is the fall-through
                    // from IF (condition was false). Store the else-path
                    // value BEFORE the IF so it's the default. Store the
                    // then-path value before END_IF (overrides on true).
                    else_insert = if_idx; // Before the IF
                    then_insert = end_if_idx; // Before END_IF (in then-branch)
                }
                rewrites.push(PhiRewrite {
                    then_insert_idx: then_insert,
                    then_arg: args[0].clone(),
                    else_insert_idx: else_insert,
                    else_arg: args[1].clone(),
                    phi_idx: scan,
                    phi_out: out.clone(),
                });
            }
            scan += 1;
        }
    }

    if rewrites.is_empty() {
        return;
    }

    // Phase 3: apply rewrites. Work from the end to preserve indices.
    // Sort rewrites by phi_idx descending to avoid index invalidation.
    rewrites.sort_by_key(|rewrite| std::cmp::Reverse(rewrite.phi_idx));

    for rewrite in &rewrites {
        // Replace phi with load_var.
        ops[rewrite.phi_idx] = OpIR {
            kind: molt_ir::OpKind::LoadVar,
            var: Some(format!("_phi_{}", rewrite.phi_out)),
            out: Some(rewrite.phi_out.clone()),
            ..OpIR::new(molt_ir::OpKind::LoadVar)
        };
    }

    // Now insert store_var ops. Collect all insertions, sort by index descending.
    let mut insertions: Vec<(usize, OpIR)> = Vec::new();
    for rewrite in &rewrites {
        // Store for else-path (insert before end_if).
        insertions.push((
            rewrite.else_insert_idx,
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.else_arg.clone()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
        ));
        // Store for then-path (insert before else or end_if).
        insertions.push((
            rewrite.then_insert_idx,
            OpIR {
                kind: molt_ir::OpKind::StoreVar,
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.then_arg.clone()]),
                ..OpIR::new(molt_ir::OpKind::StoreVar)
            },
        ));
    }

    // Sort by insertion index descending to maintain correct positions.
    insertions.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));

    for (idx, op) in insertions {
        ops.insert(idx, op);
    }
}

#[cfg(test)]
#[path = "ir_rewrites/regression_tests.rs"]
mod regression_tests;
