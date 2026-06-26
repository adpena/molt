use crate::FunctionIR;
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Escape analysis pass
//
// Scans the IR op stream for short-lived object allocations (tuple_new,
// list_new, dict_new) and determines whether the resulting object "escapes"
// the current function.  An allocation escapes if its result variable is:
//
//   - Returned from the function (ret)
//   - Passed to a function call (call, call_internal, call_method, etc.)
//   - Stored to a non-local / global / attribute / closure variable
//   - Stored into another object (store_index, dict_set, list_append, etc.)
//   - Used by yield / yield_from / await
//
// If an allocation does NOT escape, it is marked `stack_eligible = true`,
// signalling the native backend that it may use a stack slot instead of a
// heap allocation.  The primary beneficiary is the (value, done) tuple from
// `iter_next`, which is created on every loop iteration, immediately
// destructured via `index`, and never referenced again.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn escape_analysis(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_ESCAPE_ANALYSIS").is_ok() {
        return;
    }

    // Allocation op kinds eligible for stack promotion.
    let alloc_kinds = ["tuple_new", "list_new", "dict_new"];

    // Op kinds where any argument reference is a "safe" (non-escaping) use.
    // The object is consumed locally — read-only or iteration.
    let safe_use_kinds: BTreeSet<&str> = [
        "index",        // subscript / destructure
        "len",          // len() intrinsic
        "type",         // type() intrinsic
        "is",           // identity check
        "is_not",       // identity check
        "bool_test",    // truthiness test
        "iter",         // create an iterator (reads the container)
        "contains",     // `in` operator
        "not_contains", // `not in` operator
        "unpack",       // tuple unpacking (reads elements)
        "unpack_ex",    // star unpacking
        "compare",      // comparison
        "copy",         // local alias — tracked transitively below
    ]
    .iter()
    .copied()
    .collect();

    // Op kinds that definitely cause escape for any argument.
    let escaping_ops: BTreeSet<&str> = [
        "ret",
        "call",
        "call_internal",
        "call_method",
        "call_method_ic",
        "call_super_method_ic",
        "call_function_ex",
        "call_intrinsic",
        "store_global",
        "store_nonlocal",
        "store_attr",
        "store_index",
        "store_closure",
        "dict_set",
        "list_append",
        "list_extend",
        "set_add",
        "yield",
        "yield_from",
        "await",
        "raise",
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ]
    .iter()
    .copied()
    .collect();

    // Phase 1: Collect all allocation sites.
    // Map from output variable name (owned) → op index.
    let mut alloc_sites: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if alloc_kinds.contains(&op.kind.as_str())
            && let Some(ref out) = op.out
        {
            alloc_sites.insert(out.clone(), idx);
        }
    }

    if alloc_sites.is_empty() {
        return;
    }

    // Phase 2: Build a use-list for each allocation.
    // Track which alloc vars escape.
    let mut escaped: BTreeSet<String> = BTreeSet::new();
    // Track copy aliases: if `copy x -> y`, then y is an alias for x's alloc.
    // Maps alias name → root alloc name.
    let mut alias_to_alloc: BTreeMap<String, String> = BTreeMap::new();
    // Initialize: each alloc name maps to itself.
    for name in alloc_sites.keys() {
        alias_to_alloc.insert(name.clone(), name.clone());
    }

    // Forward scan: resolve copy aliases and check uses.
    for op in func_ir.ops.iter() {
        let kind = op.kind.as_str();

        // Handle copy aliases: if source is a tracked alloc, propagate.
        if kind == "copy" {
            if let (Some(args), Some(out)) = (&op.args, &op.out)
                && args.len() == 1
                && let Some(root) = alias_to_alloc.get(&args[0]).cloned()
            {
                alias_to_alloc.insert(out.clone(), root);
            }
            continue;
        }

        // Check arguments of this op.
        if let Some(ref args) = op.args {
            for arg in args {
                let root = match alias_to_alloc.get(arg).cloned() {
                    Some(r) => r,
                    None => continue,
                };
                if escaped.contains(&root) {
                    continue; // already known to escape
                }

                if safe_use_kinds.contains(kind) {
                    continue; // non-escaping use
                }

                if escaping_ops.contains(kind) {
                    escaped.insert(root);
                    continue;
                }

                // Conservative: unknown op → assume escape.
                escaped.insert(root);
            }
        }

        // Also check `var` field (used by ret and some other ops).
        if let Some(ref var) = op.var
            && let Some(root) = alias_to_alloc.get(var).cloned()
            && (kind == "ret" || escaping_ops.contains(kind))
        {
            escaped.insert(root);
        }
    }

    // Phase 3: Mark non-escaping allocations as stack-eligible.
    for (name, idx) in &alloc_sites {
        if !escaped.contains(name) {
            func_ir.ops[*idx].stack_eligible = Some(true);
        }
    }
}
