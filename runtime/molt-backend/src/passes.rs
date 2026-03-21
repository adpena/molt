use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_STRUCT_ELIDE").is_ok() {
        return;
    }
    let mut remove = vec![false; func_ir.ops.len()];
    let alloc_kinds = ["alloc_class", "alloc_class_trusted", "alloc_class_static"];
    let allowed_use_kinds = [
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ];

    let mut uses_by_name: BTreeMap<&str, Vec<(usize, usize, &str)>> = BTreeMap::new();
    for (use_idx, use_op) in func_ir.ops.iter().enumerate() {
        let Some(args) = use_op.args.as_ref() else {
            continue;
        };
        let kind = use_op.kind.as_str();
        for (pos, arg) in args.iter().enumerate() {
            uses_by_name
                .entry(arg.as_str())
                .or_default()
                .push((use_idx, pos, kind));
        }
    }

    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !alloc_kinds.contains(&op.kind.as_str()) {
            continue;
        }
        let Some(out_name) = op.out.as_deref() else {
            continue;
        };
        let Some(uses) = uses_by_name.get(out_name) else {
            remove[idx] = true;
            continue;
        };
        let mut allowed = true;
        for &(_, pos, use_kind) in uses {
            if pos != 0 || !allowed_use_kinds.contains(&use_kind) {
                allowed = false;
                break;
            }
        }
        if allowed {
            remove[idx] = true;
            for &(use_idx, _, _) in uses {
                remove[use_idx] = true;
            }
        }
    }

    if remove.iter().any(|&flag| flag) {
        let mut new_ops = Vec::with_capacity(func_ir.ops.len());
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if !remove[idx] {
                new_ops.push(op.clone());
            }
        }
        func_ir.ops = new_ops;
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
const INLINE_OP_LIMIT: usize = 30;

/// PGO-guided inline limit for hot functions (called >1000 times).
/// Hot callees get a larger budget so more of their body can be inlined.
const PGO_HOT_INLINE_OP_LIMIT: usize = 80;

/// Call-count threshold above which a function is considered "hot" for
/// inlining purposes.
const PGO_HOT_CALL_THRESHOLD: u64 = 1000;

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
fn is_inlineable_with_limit(
    func: &FunctionIR,
    defined_functions: &BTreeSet<&str>,
    op_limit: usize,
) -> bool {
    if func.ops.len() > op_limit {
        return false;
    }
    for op in &func.ops {
        match op.kind.as_str() {
            "loop_index_start" | "loop_index_end" | "loop_start" | "loop_end"
            | "for_iter_start" | "for_iter_end" | "while_start" | "while_end" | "try_start"
            | "try_end" | "except" | "finally" | "yield" | "yield_from" | "await"
            | "async_for_start" | "ASYNCGEN_NEW" | "GENERATOR_NEW" | "COROUTINE_NEW" => {
                return false;
            }
            "call_internal" => {
                if let Some(target) = op.s_value.as_deref()
                    && defined_functions.contains(target)
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn inline_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_INLINING").is_ok() {
        return;
    }
    let limit: usize = std::env::var("MOLT_INLINE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(INLINE_OP_LIMIT);

    let defined_functions: BTreeSet<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();

    let mut inlineable: BTreeMap<String, (Vec<String>, Vec<OpIR>)> = BTreeMap::new();
    for func in &ir.functions {
        // PGO-guided inlining: if the profile shows this function is called
        // frequently (>1000 times), allow a larger op budget so more of its
        // body can be inlined at call sites.
        let effective_limit = if let Some(profile) = ir.profile.as_ref() {
            if let Some(calls) = profile.get_call_count(&func.name) {
                if calls >= PGO_HOT_CALL_THRESHOLD {
                    limit.max(PGO_HOT_INLINE_OP_LIMIT)
                } else {
                    limit
                }
            } else {
                limit
            }
        } else {
            limit
        };
        let func_copy = FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops: func.ops.clone(),
            param_types: func.param_types.clone(),
        };
        if is_inlineable_with_limit(&func_copy, &defined_functions, effective_limit) {
            inlineable.insert(
                func_copy.name.clone(),
                (func_copy.params.clone(), func_copy.ops),
            );
        }
    }

    if inlineable.is_empty() {
        return;
    }

    let mut inline_counter = 0u64;

    for func_ir in &mut ir.functions {
        let mut new_ops: Vec<OpIR> = Vec::with_capacity(func_ir.ops.len());
        let mut changed = false;

        for op in &func_ir.ops {
            if op.kind != "call_internal" {
                new_ops.push(op.clone());
                continue;
            }
            let target_name = match op.s_value.as_deref() {
                Some(name) => name,
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };
            let Some((callee_params, callee_ops)) = inlineable.get(target_name) else {
                new_ops.push(op.clone());
                continue;
            };
            let call_args = match op.args.as_ref() {
                Some(args) => args,
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };
            let call_out = match op.out.as_deref() {
                Some(out) => out.to_string(),
                None => {
                    new_ops.push(op.clone());
                    continue;
                }
            };

            inline_counter += 1;
            let prefix = format!(
                "_inl{}_{}_",
                inline_counter,
                target_name.replace(|c: char| !c.is_alphanumeric(), "_")
            );

            let mut rename_map: BTreeMap<String, String> = BTreeMap::new();
            for (i, param) in callee_params.iter().enumerate() {
                if i < call_args.len() {
                    rename_map.insert(param.clone(), call_args[i].clone());
                }
            }

            for callee_op in callee_ops {
                if callee_op.kind == "ret" || callee_op.kind == "ret_void" {
                    if callee_op.kind == "ret"
                        && let Some(ret_var) = callee_op.var.as_deref()
                    {
                        let renamed = rename_map
                            .get(ret_var)
                            .cloned()
                            .unwrap_or_else(|| format!("{prefix}{ret_var}"));
                        new_ops.push(OpIR {
                            kind: "copy".to_string(),
                            value: None,
                            f_value: None,
                            s_value: None,
                            bytes: None,
                            var: None,
                            args: Some(vec![renamed]),
                            out: Some(call_out.clone()),
                            fast_int: None,
                            fast_float: None,
                            raw_int: None,
                            stack_eligible: None,
                            task_kind: None,
                            container_type: None,
                            type_hint: None,
                        });
                    }
                    continue;
                }

                let mut inlined_op = callee_op.clone();

                if let Some(out) = inlined_op.out.clone() {
                    let renamed = rename_map
                        .get(&out)
                        .cloned()
                        .unwrap_or_else(|| format!("{prefix}{out}"));
                    inlined_op.out = Some(renamed.clone());
                    rename_map.entry(out).or_insert(renamed);
                }

                if let Some(ref args) = inlined_op.args {
                    inlined_op.args = Some(
                        args.iter()
                            .map(|a| {
                                rename_map
                                    .get(a)
                                    .cloned()
                                    .unwrap_or_else(|| format!("{prefix}{a}"))
                            })
                            .collect(),
                    );
                }

                if let Some(ref var) = inlined_op.var {
                    inlined_op.var = Some(
                        rename_map
                            .get(var)
                            .cloned()
                            .unwrap_or_else(|| format!("{prefix}{var}")),
                    );
                }

                new_ops.push(inlined_op);
            }

            changed = true;
        }

        if changed {
            func_ir.ops = new_ops;
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn apply_profile_order(ir: &mut SimpleIR) {
    let Some(profile) = ir.profile.as_ref() else {
        return;
    };
    if profile.hot_functions.is_empty() {
        return;
    }
    let mut ranks: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.clone()).or_insert(idx);
    }
    let mut original: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, func) in ir.functions.iter().enumerate() {
        original.entry(func.name.clone()).or_insert(idx);
    }
    ir.functions.sort_by(|left, right| {
        let left_rank = ranks.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.name).copied().unwrap_or(usize::MAX);
        if left_rank != right_rank {
            return left_rank.cmp(&right_rank);
        }
        let left_idx = original.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_idx = original.get(&right.name).copied().unwrap_or(usize::MAX);
        left_idx
            .cmp(&right_idx)
            .then_with(|| left.name.cmp(&right.name))
    });
}

// ---------------------------------------------------------------------------
// Constant folding pass (peephole, pre-emission)
//
// Scans IR ops in forward order, tracking which variables hold known constant
// values.  When an arithmetic op's inputs are all constants (and `fast_int` is
// set), the op is replaced with a `const` op holding the computed result.
// This eliminates redundant unbox-compute-box sequences in the emitted code,
// yielding a 3-5% binary size reduction on constant-heavy code.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn fold_constants(ops: &mut Vec<OpIR>) {
    // Map from variable name -> known constant integer value (raw, unboxed).
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();
    // Map from variable name -> known constant boolean value.
    let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();

    for op in ops.iter_mut() {
        match op.kind.as_str() {
            "const" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), val);
                }
            }
            "const_bool" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_bools.insert(out.clone(), val != 0);
                }
            }

            // Binary integer arithmetic: add, sub, mul, inplace_add, inplace_sub, inplace_mul
            "add" | "sub" | "mul" | "inplace_add" | "inplace_sub" | "inplace_mul"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "add" | "inplace_add" => a.wrapping_add(b),
                                "sub" | "inplace_sub" => a.wrapping_sub(b),
                                "mul" | "inplace_mul" => a.wrapping_mul(b),
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                // Output variable is no longer a known constant.
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Bitwise integer ops: bit_and, bit_or, bit_xor and inplace variants
            "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
            | "inplace_bit_xor"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "bit_and" | "inplace_bit_and" => a & b,
                                "bit_or" | "inplace_bit_or" => a | b,
                                "bit_xor" | "inplace_bit_xor" => a ^ b,
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Boolean not: `not` on a known bool constant.
            "not" => {
                if let Some(ref args) = op.args {
                    if args.len() == 1 {
                        if let Some(&val) = const_bools.get(&args[0]) {
                            let result = !val;
                            op.kind = "const_bool".to_string();
                            op.value = Some(if result { 1 } else { 0 });
                            op.args = None;
                            if let Some(ref out) = op.out {
                                const_bools.insert(out.clone(), result);
                                const_ints.remove(out);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Control flow boundaries: clear all tracked constants.
            "if" | "else" | "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end"
            | "jump" | "label" | "state_switch" => {
                const_ints.clear();
                const_bools.clear();
            }

            // Any other op that writes an output kills the constant for that variable.
            _ => {
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-block constant propagation pass
//
// Extends fold_constants with dominator-aware constant tracking across
// structured control flow (if / else / end_if).  Constants defined before
// a branch are available in both arms.  At merge points (end_if), only
// constants that agree in both arms survive.
//
// For unstructured control flow (loops, try/except, jumps, labels) we
// conservatively clear all tracked constants, same as the intra-block pass.
//
// This pass fully subsumes fold_constants — it performs the same peephole
// arithmetic folding AND propagates constants across basic block boundaries.
// ---------------------------------------------------------------------------

/// Saved constant state at a control-flow split point.
struct BranchSnapshot {
    /// Constants known at the point just before the `if` op.
    pre_ints: BTreeMap<String, i64>,
    pre_bools: BTreeMap<String, bool>,
    /// Constants accumulated in the *then* arm (captured when we hit `else`).
    then_ints: Option<BTreeMap<String, i64>>,
    then_bools: Option<BTreeMap<String, bool>>,
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn fold_constants_cross_block(ops: &mut Vec<OpIR>) {
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();
    let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();

    // Stack of snapshots for nested if/else/end_if.
    let mut branch_stack: Vec<BranchSnapshot> = Vec::new();

    for op in ops.iter_mut() {
        match op.kind.as_str() {
            // ----- constant definitions -----
            "const" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), val);
                }
            }
            "const_bool" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_bools.insert(out.clone(), val != 0);
                }
            }

            // ----- binary integer arithmetic -----
            "add" | "sub" | "mul" | "inplace_add" | "inplace_sub" | "inplace_mul"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "add" | "inplace_add" => a.wrapping_add(b),
                                "sub" | "inplace_sub" => a.wrapping_sub(b),
                                "mul" | "inplace_mul" => a.wrapping_mul(b),
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- bitwise integer ops -----
            "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
            | "inplace_bit_xor"
                if op.fast_int.unwrap_or(false) =>
            {
                if let Some(ref args) = op.args {
                    if args.len() == 2 {
                        let a_val = const_ints.get(&args[0]).copied();
                        let b_val = const_ints.get(&args[1]).copied();
                        if let (Some(a), Some(b)) = (a_val, b_val) {
                            let result = match op.kind.as_str() {
                                "bit_and" | "inplace_bit_and" => a & b,
                                "bit_or" | "inplace_bit_or" => a | b,
                                "bit_xor" | "inplace_bit_xor" => a ^ b,
                                _ => unreachable!(),
                            };
                            op.kind = "const".to_string();
                            op.value = Some(result);
                            op.args = None;
                            op.fast_int = None;
                            if let Some(ref out) = op.out {
                                const_ints.insert(out.clone(), result);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- boolean not -----
            "not" => {
                if let Some(ref args) = op.args {
                    if args.len() == 1 {
                        if let Some(&val) = const_bools.get(&args[0]) {
                            let result = !val;
                            op.kind = "const_bool".to_string();
                            op.value = Some(if result { 1 } else { 0 });
                            op.args = None;
                            if let Some(ref out) = op.out {
                                const_bools.insert(out.clone(), result);
                                const_ints.remove(out);
                            }
                            continue;
                        }
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- structured control flow: if / else / end_if -----
            "if" => {
                branch_stack.push(BranchSnapshot {
                    pre_ints: const_ints.clone(),
                    pre_bools: const_bools.clone(),
                    then_ints: None,
                    then_bools: None,
                });
            }
            "else" => {
                if let Some(snapshot) = branch_stack.last_mut() {
                    snapshot.then_ints = Some(const_ints.clone());
                    snapshot.then_bools = Some(const_bools.clone());
                    const_ints = snapshot.pre_ints.clone();
                    const_bools = snapshot.pre_bools.clone();
                } else {
                    const_ints.clear();
                    const_bools.clear();
                }
            }
            "end_if" => {
                if let Some(snapshot) = branch_stack.pop() {
                    if let (Some(then_ints), Some(then_bools)) =
                        (snapshot.then_ints, snapshot.then_bools)
                    {
                        let else_ints = const_ints;
                        let else_bools = const_bools;

                        let mut merged_ints = BTreeMap::new();
                        for (name, then_val) in &then_ints {
                            if let Some(&else_val) = else_ints.get(name) {
                                if then_val == &else_val {
                                    merged_ints.insert(name.clone(), *then_val);
                                }
                            }
                        }

                        let mut merged_bools = BTreeMap::new();
                        for (name, then_val) in &then_bools {
                            if let Some(&else_val) = else_bools.get(name) {
                                if then_val == &else_val {
                                    merged_bools.insert(name.clone(), *then_val);
                                }
                            }
                        }

                        const_ints = merged_ints;
                        const_bools = merged_bools;
                    } else {
                        let then_ints = const_ints;
                        let then_bools = const_bools;

                        let mut merged_ints = BTreeMap::new();
                        for (name, pre_val) in &snapshot.pre_ints {
                            if let Some(&then_val) = then_ints.get(name) {
                                if pre_val == &then_val {
                                    merged_ints.insert(name.clone(), *pre_val);
                                }
                            }
                        }

                        let mut merged_bools = BTreeMap::new();
                        for (name, pre_val) in &snapshot.pre_bools {
                            if let Some(&then_val) = then_bools.get(name) {
                                if pre_val == &then_val {
                                    merged_bools.insert(name.clone(), *pre_val);
                                }
                            }
                        }

                        const_ints = merged_ints;
                        const_bools = merged_bools;
                    }
                } else {
                    const_ints.clear();
                    const_bools.clear();
                }
            }

            // ----- unstructured / opaque control flow: conservative clear -----
            "loop_start" | "loop_end" | "try_start" | "try_end" | "jump" | "label"
            | "state_switch" => {
                const_ints.clear();
                const_bools.clear();
                branch_stack.clear();
            }

            // ----- default: kill the output variable -----
            _ => {
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }
        }
    }
}

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
pub(crate) fn escape_analysis(func_ir: &mut FunctionIR) {
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
        if alloc_kinds.contains(&op.kind.as_str()) {
            if let Some(ref out) = op.out {
                alloc_sites.insert(out.clone(), idx);
            }
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
            if let (Some(args), Some(out)) = (&op.args, &op.out) {
                if args.len() == 1 {
                    if let Some(root) = alias_to_alloc.get(&args[0]).cloned() {
                        alias_to_alloc.insert(out.clone(), root);
                    }
                }
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
        if let Some(ref var) = op.var {
            if let Some(root) = alias_to_alloc.get(var).cloned() {
                if kind == "ret" || escaping_ops.contains(kind) {
                    escaped.insert(root);
                }
            }
        }
    }

    // Phase 3: Mark non-escaping allocations as stack-eligible.
    for (name, idx) in &alloc_sites {
        if !escaped.contains(name) {
            func_ir.ops[*idx].stack_eligible = Some(true);
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-built constant integer map for O(1) lookups during compilation.
//
// Scans all ops once and records the first `const` definition for each
// variable name. This replaces any backward scan pattern (O(n) per lookup)
// with a single O(n) build step + O(1) HashMap lookups.
//
// Only the first definition is stored, which is correct for SSA-like
// variable naming where each name is defined exactly once.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) fn build_const_int_map(ops: &[OpIR]) -> BTreeMap<String, i64> {
    let mut map = BTreeMap::new();
    for op in ops {
        if op.kind == "const" {
            if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                // Only store the first definition (SSA correctness).
                map.entry(out.clone()).or_insert(val);
            }
        }
    }
    map
}
