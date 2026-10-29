use crate::FunctionIR;

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
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

pub fn escape_analysis(func_ir: &mut FunctionIR) {
    if backend_env::var(BackendEnv::MOLT_DISABLE_ESCAPE_ANALYSIS).is_ok() {
        return;
    }

    // Allocation op kinds eligible for stack promotion.
    let alloc_kinds = [
        molt_ir::OpKind::TupleNew,
        molt_ir::OpKind::ListNew,
        molt_ir::OpKind::DictNew,
    ];

    // Op kinds where any argument reference is a "safe" (non-escaping) use.
    // The object is consumed locally — read-only or iteration.
    let safe_use_kinds: BTreeSet<molt_ir::OpKind> = [
        molt_ir::OpKind::Index,
        molt_ir::OpKind::Len,
        molt_ir::OpKind::Is,
        molt_ir::OpKind::IsNot,
        molt_ir::OpKind::Iter,
        molt_ir::OpKind::Contains,
        molt_ir::OpKind::Compare,
        molt_ir::OpKind::Copy,
    ]
    .iter()
    .copied()
    .collect();

    // Op kinds that definitely cause escape for any argument.
    let escaping_ops: BTreeSet<molt_ir::OpKind> = [
        molt_ir::OpKind::Call,
        molt_ir::OpKind::CallInternal,
        molt_ir::OpKind::CallMethod,
        molt_ir::OpKind::LoadMethod,
        molt_ir::OpKind::LoadSuperMethod,
        molt_ir::OpKind::CallLoadedMethod,
        molt_ir::OpKind::StoreAttr,
        molt_ir::OpKind::StoreIndex,
        molt_ir::OpKind::DictSet,
        molt_ir::OpKind::ListAppend,
        molt_ir::OpKind::ListExtend,
        molt_ir::OpKind::SetAdd,
        molt_ir::OpKind::Yield,
        molt_ir::OpKind::YieldFrom,
        molt_ir::OpKind::Raise,
        molt_ir::OpKind::Store,
        molt_ir::OpKind::GuardedFieldSet,
    ]
    .iter()
    .copied()
    .collect();

    // Phase 1: Collect all allocation sites.
    // Map from output variable name (owned) → op index.
    let mut alloc_sites: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if alloc_kinds.contains(&op.kind)
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
        let kind = op.kind;

        // Handle copy aliases: if source is a tracked alloc, propagate.
        if kind == molt_ir::OpKind::Copy {
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

                if safe_use_kinds.contains(&kind) {
                    continue; // non-escaping use
                }

                if escaping_ops.contains(&kind) || kind.is_return_terminator() {
                    escaped.insert(root);
                    continue;
                }

                // Conservative: unknown op → assume escape.
                escaped.insert(root);
            }
        }

        // Also check `var` for the remaining operations whose generated field
        // role permits a source value.
        if let Some(ref var) = op.var
            && let Some(root) = alias_to_alloc.get(var).cloned()
            && (escaping_ops.contains(&kind) || kind.is_return_terminator())
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
