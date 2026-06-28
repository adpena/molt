use crate::{FunctionIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

// Multi-value return analysis (WASM_OPTIMIZATION_PLAN.md section 3.1).
//
// Scans every function in the IR and identifies call sites whose result is
// immediately destructured via a fixed number of `tuple_index` ops with
// constant indices 0..N-1. These are candidates for the multi-value return
// optimisation: the callee can push N i64 results directly, and the caller can
// consume them without a heap-allocated tuple.
//
// Returns a map: callee_name -> required_return_count (2 or 3). Only functions
// where every call site destructures to the same arity are included.
pub(crate) fn detect_multi_return_candidates(ir: &SimpleIR) -> BTreeMap<String, usize> {
    // callee -> Option<arity> (None means conflicting arities => ineligible)
    let mut candidate_arity: BTreeMap<String, Option<usize>> = BTreeMap::new();

    for func_ir in &ir.functions {
        let ops = &func_ir.ops;
        for (i, op) in ops.iter().enumerate() {
            // Only consider call_internal (user-defined functions we control).
            if op.kind != "call_internal" {
                continue;
            }
            let Some(callee) = op.s_value.as_ref() else {
                continue;
            };
            let Some(result_var) = op.out.as_ref() else {
                continue;
            };

            // Scan forward to find consecutive tuple_index ops on result_var.
            let mut unpack_count = 0usize;
            let mut seen_indices: BTreeSet<i64> = BTreeSet::new();
            for j in (i + 1)..ops.len() {
                let next_op = &ops[j];
                if next_op.kind != "tuple_index" {
                    break;
                }
                let Some(args) = next_op.args.as_ref() else {
                    break;
                };
                if args.len() < 2 || args[0] != *result_var {
                    break;
                }
                // The index argument should be a const-int; we check by
                // looking at the preceding ops, but for this analysis just
                // count the tuple_index ops.
                if let Some(idx_val) = next_op.value {
                    seen_indices.insert(idx_val);
                }
                unpack_count += 1;
            }

            // Only 2 or 3 element unpacks are worth multi-value. Mark callees
            // with non-destructuring call sites as ineligible.
            if !(2..=3).contains(&unpack_count) {
                candidate_arity.insert(callee.clone(), None);
                continue;
            }

            match candidate_arity.entry(callee.clone()) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(Some(unpack_count));
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    if *e.get() != Some(unpack_count) {
                        // Conflicting arities across call sites: not eligible.
                        *e.get_mut() = None;
                    }
                }
            }
        }
    }

    let call_site_candidates: BTreeMap<String, usize> = candidate_arity
        .into_iter()
        .filter_map(|(name, arity)| arity.map(|a| (name, a)))
        .collect();

    // Phase 2: Verify the callee function body: every `ret` must return a
    // variable that was produced by a `tuple_new` with the expected arity.
    let func_map: BTreeMap<&str, &FunctionIR> =
        ir.functions.iter().map(|f| (f.name.as_str(), f)).collect();

    call_site_candidates
        .into_iter()
        .filter(|(name, expected_arity)| {
            let Some(func_ir) = func_map.get(name.as_str()) else {
                return false;
            };
            let mut tuple_new_vars: BTreeSet<String> = BTreeSet::new();
            let mut has_any_ret = false;
            let mut all_rets_ok = true;

            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "tuple_new" => {
                        if let Some(args) = &op.args
                            && args.len() == *expected_arity
                            && let Some(out) = &op.out
                        {
                            tuple_new_vars.insert(out.clone());
                        }
                    }
                    "ret" => {
                        has_any_ret = true;
                        match &op.var {
                            Some(var) if tuple_new_vars.contains(var) => {}
                            _ => {
                                all_rets_ok = false;
                            }
                        }
                    }
                    _ => {}
                }
            }

            has_any_ret && all_rets_ok
        })
        .collect()
}
