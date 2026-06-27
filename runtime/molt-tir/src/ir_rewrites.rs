//! Backend-neutral IR rewrite/elision passes (moved verbatim from lib.rs).

use crate::ir::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

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
        match op.kind.as_str() {
            "if" => if_stack.push((idx, None)),
            "else" => {
                if let Some(top) = if_stack.last_mut() {
                    top.1 = Some(idx);
                }
            }
            "end_if" => {
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
        while scan < ops.len() && ops[scan].kind == "phi" {
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
            kind: "load_var".to_string(),
            var: Some(format!("_phi_{}", rewrite.phi_out)),
            out: Some(rewrite.phi_out.clone()),
            ..OpIR::default()
        };
    }

    // Now insert store_var ops. Collect all insertions, sort by index descending.
    let mut insertions: Vec<(usize, OpIR)> = Vec::new();
    for rewrite in &rewrites {
        // Store for else-path (insert before end_if).
        insertions.push((
            rewrite.else_insert_idx,
            OpIR {
                kind: "store_var".to_string(),
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.else_arg.clone()]),
                ..OpIR::default()
            },
        ));
        // Store for then-path (insert before else or end_if).
        insertions.push((
            rewrite.then_insert_idx,
            OpIR {
                kind: "store_var".to_string(),
                var: Some(format!("_phi_{}", rewrite.phi_out)),
                args: Some(vec![rewrite.then_arg.clone()]),
                ..OpIR::default()
            },
        ));
    }

    // Sort by insertion index descending to maintain correct positions.
    insertions.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));

    for (idx, op) in insertions {
        ops.insert(idx, op);
    }
}

/// Eliminate try/except wrappers whose body provably cannot raise.
///
/// The frontend emits a fixed structural pattern for `try: BODY except ...:
/// HANDLER`:
/// ```text
///   exception_push
///   try_start
///   <BODY>
///   try_end
///   jump <done_label>
///   label <handler_label>
///   label <done_label>
///   <handler dispatch sequence>
///   exception_pop
/// ```
/// Each `exception_push`/`exception_pop` pair carries a real per-iter cost
/// (two function calls into the runtime) plus the dead-but-still-emitted
/// handler dispatch.  When BODY contains no potentially-raising op, none
/// of that overhead is observable: the handler is unreachable, the pop
/// has no state to release.  This pass replaces the entire wrapped
/// region with just the BODY ops.
///
/// Targets `bench_exception_check.happy_path`'s `try: total += 1` loop
/// where `total += 1` is typed scalar arithmetic and provably non-throwing.
pub fn elide_useless_try_blocks(ops: &mut Vec<OpIR>) {
    elide_useless_try_blocks_inner(ops, None);
}

/// Elide try/except wrappers using the same typed representation authority as
/// the backend lowering path. Transport flags such as `fast_int` and
/// `fast_float` are not proof that Python dispatch cannot raise.
pub fn elide_useless_try_blocks_for_function(func: &mut FunctionIR) {
    let scalar_plan = crate::representation_plan::ScalarRepresentationPlan::for_function_ir(func);
    let scalar_facts =
        crate::passes::SimpleIrScalarPurityFacts::for_function(func, Some(&scalar_plan));
    elide_useless_try_blocks_inner(&mut func.ops, Some(&scalar_facts));
}

fn elide_useless_try_blocks_inner(
    ops: &mut Vec<OpIR>,
    scalar_facts: Option<&crate::passes::SimpleIrScalarPurityFacts<'_>>,
) {
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind != "exception_push" {
            i += 1;
            continue;
        }
        let push_idx = i;
        // Find matching exception_pop at depth 0.
        let mut depth = 1;
        let mut pop_idx: Option<usize> = None;
        let mut j = i + 1;
        while j < ops.len() {
            match ops[j].kind.as_str() {
                "exception_push" => depth += 1,
                "exception_pop" => {
                    depth -= 1;
                    if depth == 0 {
                        pop_idx = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let pop_idx = match pop_idx {
            Some(j) => j,
            None => {
                i += 1;
                continue;
            }
        };
        // Find try_start within (push_idx, pop_idx).  The frontend
        // always emits try_start as the first op after exception_push,
        // but tolerate intervening line/check_exception markers.
        let try_start_idx = (push_idx + 1..pop_idx).find(|&k| ops[k].kind == "try_start");
        let try_end_idx = match try_start_idx {
            Some(ts) => (ts + 1..pop_idx).find(|&k| ops[k].kind == "try_end"),
            None => None,
        };
        let (Some(ts_idx), Some(te_idx)) = (try_start_idx, try_end_idx) else {
            i = pop_idx + 1;
            continue;
        };
        let Some(handler_label) = ops[ts_idx].value else {
            i = pop_idx + 1;
            continue;
        };
        if !try_wrapper_has_except_dispatch(ops, ts_idx, te_idx, pop_idx, handler_label) {
            i = pop_idx + 1;
            continue;
        }
        // Body is ops[ts_idx + 1 .. te_idx].  All must be provably
        // non-throwing.  Encountering any nested exception_push in the
        // body is fine — it nests in our analysis but not at this
        // pair's depth, so it's already been considered (or will be on
        // the next outer iteration after we splice).  But conservatively
        // bail on nested try here to avoid doubly-modifying ranges.
        let body_range = ts_idx + 1..te_idx;
        let mut safe = true;
        for op in &ops[body_range.clone()] {
            // Don't recurse into nested try blocks at this pass — the
            // outer iteration will revisit the spliced ops afterwards.
            if op.kind == "exception_push" || op.kind == "try_start" {
                safe = false;
                break;
            }
            if !crate::passes::simple_ir_op_is_provably_nonthrowing_with_facts(scalar_facts, op) {
                safe = false;
                break;
            }
        }
        if !safe {
            i = pop_idx + 1;
            continue;
        }
        let removed_labels = removed_try_wrapper_labels(ops, push_idx, pop_idx, body_range.clone());
        if body_branches_to_removed_wrapper_label(ops, body_range.clone(), &removed_labels) {
            i = pop_idx + 1;
            continue;
        }
        // Replace the entire wrapper [push_idx, pop_idx] with just BODY.
        // Checks that target now-removed wrapper handlers are dead because
        // the body has been proven non-throwing; keeping them would leave
        // dangling handler labels after the wrapper is spliced away.
        let body: Vec<OpIR> = ops[body_range]
            .iter()
            .filter(|op| {
                !(op.kind == "check_exception"
                    && op
                        .value
                        .is_some_and(|label| removed_labels.contains(&label)))
            })
            .cloned()
            .collect();
        ops.splice(push_idx..=pop_idx, body);
        // Restart from push_idx to catch nested wrappers that may
        // have come into scope after splicing.
        i = push_idx;
    }
}

fn removed_try_wrapper_labels(
    ops: &[OpIR],
    push_idx: usize,
    pop_idx: usize,
    body_range: std::ops::Range<usize>,
) -> Vec<i64> {
    (push_idx..=pop_idx)
        .filter(|idx| !body_range.contains(idx))
        .filter_map(|idx| {
            let op = &ops[idx];
            matches!(op.kind.as_str(), "label" | "state_label")
                .then_some(op.value)
                .flatten()
        })
        .collect()
}

fn body_branches_to_removed_wrapper_label(
    ops: &[OpIR],
    body_range: std::ops::Range<usize>,
    removed_labels: &[i64],
) -> bool {
    ops[body_range].iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "jump" | "goto" | "br_if" | "loop_break_if_true" | "loop_break_if_false"
        ) && op
            .value
            .is_some_and(|label| removed_labels.contains(&label))
    })
}

fn try_wrapper_has_except_dispatch(
    ops: &[OpIR],
    _try_start_idx: usize,
    try_end_idx: usize,
    pop_idx: usize,
    handler_label: i64,
) -> bool {
    let mut cursor = try_end_idx + 1;
    while cursor < pop_idx && matches!(ops[cursor].kind.as_str(), "line" | "nop") {
        cursor += 1;
    }
    let Some(done_label) = ops.get(cursor).and_then(|op| {
        matches!(op.kind.as_str(), "jump" | "goto")
            .then_some(op.value)
            .flatten()
    }) else {
        return false;
    };
    cursor += 1;
    while cursor < pop_idx && matches!(ops[cursor].kind.as_str(), "line" | "nop") {
        cursor += 1;
    }
    if ops
        .get(cursor)
        .is_none_or(|op| op.kind != "label" || op.value != Some(handler_label))
    {
        return false;
    }
    cursor += 1;

    while cursor < pop_idx {
        let op = &ops[cursor];
        if op.kind == "label" && op.value == Some(done_label) {
            return false;
        }
        if op.kind == "exception_match_builtin" {
            return true;
        }
        cursor += 1;
    }
    false
}

fn alias_rewrite_var_field_is_storage_definition(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        "store_var"
            | "store_fast"
            | "store_var_slot"
            | "store_fast_slot"
            | "delete_var"
            | "iter_next_unboxed"
    )
}

fn alias_rewrite_var_field_is_value_read(op: &OpIR) -> bool {
    if matches!(op.kind.as_str(), "copy_var" | "load_var")
        && op.args.as_ref().is_some_and(|args| !args.is_empty())
    {
        return false;
    }
    !alias_rewrite_var_field_is_storage_definition(op)
}

fn alias_rewrite_mutable_storage_names(ops: &[OpIR]) -> BTreeSet<String> {
    ops.iter()
        .filter(|op| alias_rewrite_var_field_is_storage_definition(op))
        .filter_map(|op| op.var.as_deref().or(op.out.as_deref()))
        .filter(|name| *name != "none")
        .map(str::to_string)
        .collect()
}

/// Collapse simple SSA alias-only copy ops (`copy`, `copy_var`,
/// `identity_alias`) by rewriting later uses to the original source name.
///
/// Python local storage names are not SSA values: a `store_var` target can be
/// overwritten between a copy site and a later use. Treating those names as
/// immutable aliases erases the load point and lets exception-split native
/// blocks reuse stale SSA values. Alias collapse is therefore restricted to
/// names that are not mutable storage targets anywhere in the function.
pub fn rewrite_copy_aliases(ops: &mut [OpIR]) {
    let mutable_storage_names = alias_rewrite_mutable_storage_names(ops);
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let resolve_alias = |name: &str, aliases: &BTreeMap<String, String>| -> String {
        let mut current = name;
        while let Some(next) = aliases.get(current) {
            current = next;
        }
        current.to_string()
    };

    for op in ops.iter_mut() {
        if alias_rewrite_var_field_is_value_read(op)
            && let Some(var) = op.var.as_mut()
        {
            *var = resolve_alias(var, &aliases);
        }
        if let Some(args) = op.args.as_mut() {
            for arg in args {
                *arg = resolve_alias(arg, &aliases);
            }
        }

        match op.kind.as_str() {
            "copy_var" if op.args.is_none() => {
                if let (Some(src), Some(out)) = (op.var.as_ref(), op.out.as_ref())
                    && out != "none"
                    && !mutable_storage_names.contains(out)
                    && !mutable_storage_names.contains(src)
                {
                    aliases.insert(out.clone(), src.clone());
                    op.kind = "nop".to_string();
                    op.var = None;
                    op.out = None;
                }
            }
            "copy" | "identity_alias" => {
                if let (Some(args), Some(out)) = (op.args.as_ref(), op.out.as_ref())
                    && let Some(src) = args.first()
                    && out != "none"
                    && !mutable_storage_names.contains(out)
                    && !mutable_storage_names.contains(src)
                {
                    aliases.insert(out.clone(), src.clone());
                    op.kind = "nop".to_string();
                    op.args = None;
                    op.out = None;
                }
            }
            _ => {}
        }
    }
}

/// Replace typing-only `__annotate__` stubs with a deterministic empty-dict
/// return so all backend entrypoints preserve matching callable signatures and
/// a usable `__annotations__` value.
pub fn rewrite_annotate_stubs(ir: &mut SimpleIR) {
    for func in ir.functions.iter_mut() {
        if func.name.contains("__annotate__") {
            func.ops.clear();
            func.ops.push(OpIR {
                kind: "dict_new".to_string(),
                out: Some("__ret".to_string()),
                ..OpIR::default()
            });
            func.ops.push(OpIR {
                kind: "ret".to_string(),
                var: Some("__ret".to_string()),
                ..OpIR::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> String {
        value.to_string()
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| name(value)).collect()
    }

    #[test]
    fn rewrite_copy_aliases_collapses_pure_ssa_copy() {
        let mut ops = vec![
            OpIR {
                kind: "copy".to_string(),
                out: Some(name("alias")),
                args: Some(args(&["source"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "len".to_string(),
                out: Some(name("length")),
                args: Some(args(&["alias"])),
                ..OpIR::default()
            },
        ];

        rewrite_copy_aliases(&mut ops);

        assert_eq!(ops[0].kind, "nop");
        assert_eq!(ops[1].args.as_ref(), Some(&args(&["source"])));
    }

    #[test]
    fn rewrite_copy_aliases_preserves_load_from_mutable_store_var() {
        let mut ops = vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some(name("initial")),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some(name("bag")),
                args: Some(args(&["initial"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "list_new".to_string(),
                out: Some(name("list_value")),
                args: Some(args(&["item"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some(name("bag")),
                args: Some(args(&["list_value"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy_var".to_string(),
                var: Some(name("bag")),
                out: Some(name("loaded_bag")),
                ..OpIR::default()
            },
            OpIR {
                kind: "len".to_string(),
                out: Some(name("length")),
                args: Some(args(&["loaded_bag"])),
                ..OpIR::default()
            },
        ];

        rewrite_copy_aliases(&mut ops);

        assert_eq!(ops[4].kind, "copy_var");
        assert_eq!(ops[4].var.as_deref(), Some("bag"));
        assert_eq!(ops[4].out.as_deref(), Some("loaded_bag"));
        assert_eq!(ops[5].args.as_ref(), Some(&args(&["loaded_bag"])));
    }

    #[test]
    fn rewrite_copy_aliases_does_not_rewrite_mutable_assignment_targets() {
        let mut ops = vec![
            OpIR {
                kind: "copy".to_string(),
                out: Some(name("slot")),
                args: Some(args(&["source"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some(name("slot")),
                args: Some(args(&["replacement"])),
                ..OpIR::default()
            },
            OpIR {
                kind: "len".to_string(),
                out: Some(name("length")),
                args: Some(args(&["slot"])),
                ..OpIR::default()
            },
        ];

        rewrite_copy_aliases(&mut ops);

        assert_eq!(ops[0].kind, "copy");
        assert_eq!(ops[1].var.as_deref(), Some("slot"));
        assert_eq!(ops[2].args.as_ref(), Some(&args(&["slot"])));
    }
}

#[cfg(test)]
#[path = "ir_rewrites/regression_tests.rs"]
mod regression_tests;
