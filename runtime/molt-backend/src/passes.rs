use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
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
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
const PGO_HOT_INLINE_OP_LIMIT: usize = 80;

/// Call-count threshold above which a function is considered "hot" for
/// inlining purposes.  The profiler uses this to populate
/// `PgoProfileIR::hot_functions`; the constant is kept here for reference.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
const _PGO_HOT_CALL_THRESHOLD: u64 = 1000;

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
pub fn inline_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_INLINING").is_ok() {
        return;
    }
    let limit: usize = std::env::var("MOLT_INLINE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(INLINE_OP_LIMIT);

    let defined_functions: BTreeSet<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();

    // Build a set of PGO-hot function names for O(1) lookup.
    let pgo_hot: BTreeSet<&str> = ir
        .profile
        .as_ref()
        .map(|p| p.hot_functions.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let mut inlineable: BTreeMap<String, (Vec<String>, Vec<OpIR>)> = BTreeMap::new();
    for func in &ir.functions {
        // PGO-guided inlining: if the profile shows this function is called
        // frequently (>1000 times), allow a larger op budget so more of its
        // body can be inlined at call sites.
        let effective_limit = if pgo_hot.contains(func.name.as_str()) {
            limit.max(PGO_HOT_INLINE_OP_LIMIT)
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
                            ic_index: None,
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
pub fn apply_profile_order(ir: &mut SimpleIR) {
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
pub fn fold_constants(ops: &mut Vec<OpIR>) {
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
#[allow(dead_code)]
struct BranchSnapshot {
    /// Constants known at the point just before the `if` op.
    pre_ints: BTreeMap<String, i64>,
    pre_bools: BTreeMap<String, bool>,
    /// Constants accumulated in the *then* arm (captured when we hit `else`).
    then_ints: Option<BTreeMap<String, i64>>,
    then_bools: Option<BTreeMap<String, bool>>,
}

#[allow(dead_code)]
pub fn fold_constants_cross_block(ops: &mut Vec<OpIR>) {
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
// Loop-invariant type narrowing pass
//
// Scans for loop regions (loop_start..loop_end and loop_index_start..loop_end)
// and propagates `fast_int = true` onto arithmetic ops inside loop bodies
// whose operands are all known to be integers.
//
// A variable is "known-int" if it is produced by:
//   - `const` (integer literal)
//   - `const_bool` (booleans are int-compatible in Python arithmetic)
//   - `loop_index_start` / `loop_index_next` (range loop induction variable)
//   - An arithmetic op that already has `fast_int = true`
//   - Any op with `type_hint = "int"` or `type_hint = "bool"`
//
// The pass runs in two phases:
//   1. Pre-loop: collect known-int variables from ops before the loop.
//   2. Loop body (iterated to fixpoint): propagate known-int through the
//      loop body and set `fast_int = true` on eligible arithmetic ops.
//
// This eliminates N-1 redundant tag checks per loop for variables whose
// integer type is loop-invariant (e.g. `total += i` where both are ints).
// ---------------------------------------------------------------------------

/// Op kinds eligible for fast_int promotion when all operands are known-int.
/// Split into two categories: ops that produce ints and ops that produce bools.
#[allow(dead_code)]
const FAST_INT_ARITH_OPS: &[&str] = &[
    "add",
    "sub",
    "mul",
    "inplace_add",
    "inplace_sub",
    "inplace_mul",
    "floordiv",
    "inplace_floordiv",
    "mod",
    "inplace_mod",
    "bit_and",
    "bit_or",
    "bit_xor",
    "inplace_bit_and",
    "inplace_bit_or",
    "inplace_bit_xor",
    "lshift",
    "rshift",
    "lt",
    "le",
    "gt",
    "ge",
    "eq",
    "ne",
];

/// Comparison ops produce bool, not int; their outputs should not be treated
/// as known-int for downstream propagation (though they benefit from fast_int
/// for skipping tag checks on their *operands*).
#[allow(dead_code)]
const COMPARISON_OPS: &[&str] = &["lt", "le", "gt", "ge", "eq", "ne"];

/// Op kinds that produce a known-int output regardless of inputs.
#[allow(dead_code)]
const INT_PRODUCING_OPS: &[&str] = &[
    "const",
    "const_bool",
    "loop_index_start",
    "loop_index_next",
    "len",
];

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
#[allow(dead_code)]
pub fn propagate_loop_fast_int(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_LOOP_FAST_INT").is_ok() {
        return;
    }

    let ops = &func_ir.ops;
    let len = ops.len();

    // Phase 1: Find all loop regions (pairs of start_idx, end_idx).
    // Note: indexed loops emit LOOP_START + LOOP_INDEX_START as a pair,
    // but only one LOOP_END closes them. We skip LOOP_START when it is
    // immediately followed by LOOP_INDEX_START (the codegen does the same).
    let mut loop_regions: Vec<(usize, usize)> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                let next_is_index = ops
                    .get(idx + 1)
                    .is_some_and(|next| next.kind == "loop_index_start");
                if !next_is_index {
                    loop_start_stack.push(idx);
                }
                // If next is loop_index_start, let that one push instead.
            }
            "loop_index_start" => {
                loop_start_stack.push(idx);
            }
            "loop_end" => {
                if let Some(start) = loop_start_stack.pop() {
                    loop_regions.push((start, idx));
                }
            }
            _ => {}
        }
    }

    if loop_regions.is_empty() {
        return;
    }

    // Phase 2: Build the initial known-int set from all ops before loops.
    // We track which variable names are known to hold integer values.
    let mut known_int: BTreeSet<String> = BTreeSet::new();
    for op in ops.iter() {
        if let Some(ref out) = op.out {
            if INT_PRODUCING_OPS.contains(&op.kind.as_str()) {
                known_int.insert(out.clone());
            } else if matches!(op.type_hint.as_deref(), Some("int") | Some("bool")) {
                known_int.insert(out.clone());
            } else if op.fast_int.unwrap_or(false) {
                // An arithmetic op already marked fast_int produces an int.
                known_int.insert(out.clone());
            }
        }
    }

    // Phase 3: For each loop region, propagate fast_int in the loop body.
    // We iterate to fixpoint because setting fast_int on one op may make
    // its output known-int, which enables fast_int on downstream ops.
    let mut changed_any = false;
    for &(start, end) in &loop_regions {
        // Iterate to fixpoint over the loop body.
        let mut made_progress = true;
        while made_progress {
            made_progress = false;
            for idx in start..=end.min(len - 1) {
                let op = &func_ir.ops[idx];
                let kind = op.kind.as_str();

                // Check if this op is eligible for fast_int promotion.
                if !FAST_INT_ARITH_OPS.contains(&kind) {
                    continue;
                }
                let is_comparison = COMPARISON_OPS.contains(&kind);
                // Already has fast_int — just ensure output is tracked.
                if op.fast_int.unwrap_or(false) {
                    if !is_comparison {
                        if let Some(ref out) = op.out {
                            if known_int.insert(out.clone()) {
                                made_progress = true;
                            }
                        }
                    }
                    continue;
                }
                // Check if all operands are known-int.
                let all_int = op.args.as_ref().map_or(false, |args| {
                    args.len() >= 2 && args.iter().all(|a| known_int.contains(a))
                });
                if all_int {
                    // Promote to fast_int.
                    func_ir.ops[idx].fast_int = Some(true);
                    // Comparisons produce bool, not int — don't add to known_int.
                    if !is_comparison {
                        if let Some(ref out) = func_ir.ops[idx].out {
                            known_int.insert(out.clone());
                        }
                    }
                    made_progress = true;
                    changed_any = true;
                }
            }
        }
    }
    let _ = changed_any;
}

// ---------------------------------------------------------------------------
// Pre-built constant integer map for O(1) lookups during compilation.
//
// Scans all ops once and records the first `const` definition for each
// variable name. This replaces any backward scan pattern (O(n) per lookup)
// with a single O(n) build step + O(log n) BTreeMap lookups.
//
// Only the first definition is stored, which is correct for SSA-like
// variable naming where each name is defined exactly once.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn build_const_int_map(ops: &[OpIR]) -> BTreeMap<String, i64> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    fn make_const_int(out: &str, val: i64) -> OpIR {
        OpIR {
            kind: "const".to_string(),
            value: Some(val),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    fn make_arith(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    fn make_loop_index_start(arg: &str, out: &str) -> OpIR {
        OpIR {
            kind: "loop_index_start".to_string(),
            args: Some(vec![arg.to_string()]),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    fn make_loop_index_next(arg: &str, out: &str) -> OpIR {
        OpIR {
            kind: "loop_index_next".to_string(),
            args: Some(vec![arg.to_string()]),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_loop_fast_int_basic_range_loop() {
        // Simulates: total = 0; for i in range(n): total += i
        // The IR pattern for indexed loops is: loop_start, loop_index_start, ..., loop_end
        // The loop_start is a no-op when followed by loop_index_start.
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("total_init", 0),        // 0
                make_const_int("start", 0),              // 1
                make_op("loop_start"),                   // 2 (skipped for indexed loops)
                make_loop_index_start("start", "i"),     // 3
                make_arith("inplace_add", &["total_init", "i"], "total_next"), // 4
                make_const_int("step", 1),               // 5
                make_arith("add", &["i", "step"], "next_i"), // 6
                make_loop_index_next("next_i", "i"),     // 7
                make_op("loop_continue"),                // 8
                make_op("loop_end"),                     // 9
            ],
        };

        propagate_loop_fast_int(&mut func);

        // The inplace_add should now have fast_int=true
        assert_eq!(func.ops[4].fast_int, Some(true), "inplace_add should be fast_int");
        // The add (i + step) should also be fast_int
        assert_eq!(func.ops[6].fast_int, Some(true), "add should be fast_int");
    }

    #[test]
    fn test_loop_fast_int_already_set() {
        // If fast_int is already set, the pass should not change it.
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("a", 1),
                make_const_int("b", 2),
                make_op("loop_start"),
                {
                    let mut op = make_arith("add", &["a", "b"], "c");
                    op.fast_int = Some(true);
                    op
                },
                make_op("loop_end"),
            ],
        };

        propagate_loop_fast_int(&mut func);

        assert_eq!(func.ops[3].fast_int, Some(true));
    }

    #[test]
    fn test_loop_fast_int_no_loop() {
        // No loop — pass should be a no-op.
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("a", 1),
                make_const_int("b", 2),
                make_arith("add", &["a", "b"], "c"),
            ],
        };

        let original_fast_int = func.ops[2].fast_int;
        propagate_loop_fast_int(&mut func);

        // Outside a loop, the pass should not touch the op.
        assert_eq!(func.ops[2].fast_int, original_fast_int);
    }

    #[test]
    fn test_loop_fast_int_unknown_operand() {
        // If one operand is not known-int, fast_int should not be set.
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("a", 1),
                make_op("loop_start"),
                // "unknown" is not produced by any known-int op
                make_arith("add", &["a", "unknown"], "c"),
                make_op("loop_end"),
            ],
        };

        propagate_loop_fast_int(&mut func);

        assert_eq!(func.ops[2].fast_int, None, "should not set fast_int with unknown operand");
    }

    #[test]
    fn test_loop_fast_int_chained_ops() {
        // Chain: a + b -> c, then c + a -> d (fixpoint iteration needed)
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("a", 1),
                make_const_int("b", 2),
                make_op("loop_start"),
                make_arith("add", &["a", "b"], "c"),
                make_arith("add", &["c", "a"], "d"),
                make_op("loop_end"),
            ],
        };

        propagate_loop_fast_int(&mut func);

        assert_eq!(func.ops[3].fast_int, Some(true), "first add should be fast_int");
        assert_eq!(func.ops[4].fast_int, Some(true), "chained add should be fast_int via fixpoint");
    }

    #[test]
    fn test_loop_fast_int_type_hint_propagation() {
        // An op with type_hint="int" should make its output known-int.
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec![],
            param_types: None,
            ops: vec![
                make_const_int("a", 1),
                {
                    let mut op = make_op("index");
                    op.out = Some("x".to_string());
                    op.type_hint = Some("int".to_string());
                    op
                },
                make_op("loop_start"),
                make_arith("add", &["a", "x"], "c"),
                make_op("loop_end"),
            ],
        };

        propagate_loop_fast_int(&mut func);

        assert_eq!(func.ops[3].fast_int, Some(true), "add with type_hint int operand should be fast_int");
    }

    // --- RC coalescing tests ---

    fn make_ref_op(kind: &str, arg: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(vec![arg.to_string()]),
            ..Default::default()
        }
    }

    #[test]
    fn rc_coalescing_eliminates_adjacent_inc_dec_pair() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // Both inc_ref and dec_ref should be eliminated.
        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }

    #[test]
    fn rc_coalescing_preserves_pair_across_control_flow() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                make_op("if"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // The pair should NOT be eliminated because `if` is control flow.
        assert_eq!(func.ops.len(), 4);
    }

    #[test]
    fn rc_coalescing_handles_borrow_release_pair() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["y".to_string()],
            param_types: None,
            ops: vec![
                make_ref_op("borrow", "y"),
                make_ref_op("release", "y"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }

    #[test]
    fn rc_coalescing_preserves_pair_with_intervening_use() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                // An op that uses x as an argument — breaks the window.
                make_arith("add", &["x", "x"], "y"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // The pair should NOT be eliminated because of the intervening use.
        assert_eq!(func.ops.len(), 4);
    }

    #[test]
    fn rc_coalescing_eliminates_different_vars_independently() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: None,
            ops: vec![
                make_ref_op("inc_ref", "a"),
                make_ref_op("inc_ref", "b"),
                make_ref_op("dec_ref", "a"),
                make_ref_op("dec_ref", "b"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // inc_ref(a)/dec_ref(a) cannot be eliminated because inc_ref(b) intervenes
        // (it doesn't use 'a' though). Let's check what actually happens.
        // The scan finds inc_ref(a) at 0, then looks at 1 (inc_ref(b) — not a
        // dec_ref of a, and doesn't use a), then at 2 (dec_ref(a) — match!).
        // So indices 0,2 are eliminated. Then inc_ref(b) at 1, looks at 3
        // (dec_ref(b) — match!), indices 1,3 eliminated.
        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }
}

/// Identify pairs of `inc_ref`/`dec_ref` ops that cancel within a basic block.
/// Returns: (set of op indices to skip, set of variable names whose dec_ref to skip).
pub fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    const CONTROL_FLOW: &[&str] = &[
        "if", "else", "end_if", "jump", "br_if", "label",
        "check_exception", "state_transition",
        "state_yield", "state_switch", "state_label", "exception_push",
        "exception_pop", "chan_send_yield", "chan_recv_yield",
        "ret", "ret_void",
        "loop_start", "loop_index_start", "loop_end",
        "loop_break_if_true", "loop_break_if_false", "loop_continue",
    ];
    let cf_set: HashSet<&str> = CONTROL_FLOW.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) { continue; }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec { continue; }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) { break; }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release")
                    && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow")
                    && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b.args.as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var { break; }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) { continue; }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") { continue; }
        let out_name = match op.out.as_deref() {
            Some(name) if name != "none" => name,
            _ => continue,
        };
        // If the variable appears in last_use, check if its final use is at or
        // before this inc_ref — that means the inc_ref output is dead. If the
        // variable is completely absent from last_use (never used anywhere),
        // the inc_ref is also dead. We explicitly distinguish these cases to
        // avoid silently eliding an inc_ref due to variable name mismatches.
        let is_dead = match last_use.get(out_name) {
            Some(&last) => last <= idx,
            None => true, // Variable never used after definition — dead inc_ref.
        };
        if is_dead {
            skip_ops.insert(idx);
            skip_dec_ref.insert(out_name.to_string());
        }
    }

    (skip_ops, skip_dec_ref)
}

/// Build a last-use map: for each variable name, the index of the last op that
/// references it (via `var`, `args`, or `out`).
fn build_last_use_map(ops: &[OpIR]) -> BTreeMap<String, usize> {
    let mut last_use = BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let Some(var) = &op.var {
            if var != "none" {
                last_use.insert(var.clone(), i);
            }
        }
        if let Some(args) = &op.args {
            for name in args {
                if name != "none" {
                    last_use.insert(name.clone(), i);
                }
            }
        }
        if let Some(out) = &op.out {
            if out != "none" {
                last_use.insert(out.clone(), i);
            }
        }
    }
    last_use
}

/// RC coalescing pass: eliminate redundant `inc_ref`/`dec_ref` pairs within
/// basic blocks.  When an `inc_ref(x)` is followed by `dec_ref(x)` (or vice
/// versa) with no intervening store, call, control-flow, or other use that
/// could observe the refcount, the pair is removed.  Also removes trailing
/// `inc_ref` ops whose output is never used (the corresponding `dec_ref` at
/// function exit is skipped as well).
///
/// This is the IR-level counterpart of `compute_rc_coalesce_skips`, which is
/// applied at codegen time.  Running it as an early pass shrinks the op stream
/// for all downstream analyses and backends.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn rc_coalescing(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_RC_COALESCE").is_ok() {
        return;
    }

    let last_use = build_last_use_map(&func_ir.ops);
    let (skip_ops, skip_dec_ref) = compute_rc_coalesce_skips(&func_ir.ops, &last_use);

    if skip_ops.is_empty() && skip_dec_ref.is_empty() {
        return;
    }

    let mut new_ops = Vec::with_capacity(func_ir.ops.len());
    for (idx, op) in func_ir.ops.iter().enumerate() {
        // Skip ops identified as redundant inc_ref/dec_ref pairs by index.
        if skip_ops.contains(&idx) {
            continue;
        }
        // Skip dec_ref/release ops whose variable was flagged by the
        // dead-inc_ref analysis (the inc_ref was removed, so the dec_ref
        // must be removed too).
        if matches!(op.kind.as_str(), "dec_ref" | "release") {
            if let Some(arg) = op.args.as_ref().and_then(|a| a.first()) {
                if skip_dec_ref.contains(arg.as_str()) {
                    continue;
                }
            }
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}

// ---------------------------------------------------------------------------
// Loop-Invariant Code Motion (LICM)
// ---------------------------------------------------------------------------

const HOISTABLE_OPS: &[&str] = &[
    "const", "const_int", "const_float", "const_str",
    "const_bool", "const_none", "const_bytes", "list_new", "tuple_new",
];

fn is_hoistable(op: &OpIR) -> bool {
    let kind = op.kind.as_str();
    // list_new/tuple_new/dict_new allocate fresh heap objects.
    // Hoisting them out of loops causes ONE allocation to be shared across
    // all iterations, leading to aliasing corruption if the object is mutated.
    if matches!(kind, "list_new" | "tuple_new" | "dict_new" | "set_new") {
        return false;
    }
    HOISTABLE_OPS.contains(&kind)
}

/// Eliminate `check_exception` ops that follow operations known to never
/// raise exceptions. This reduces branch overhead in tight inner loops
/// (e.g., fib: 10 checks/call -> fewer).
///
/// Safe-to-elide predecessors: inc_ref, dec_ref, dec_ref_obj, const_int,
/// const_float, const_bool, const_none, nop, line.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_safe_exception_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_EXC_ELIDE").is_ok() {
        return;
    }
    /// Operations that are guaranteed to never set the exception flag.
    const NEVER_RAISES: &[&str] = &[
        "inc_ref",
        "dec_ref",
        "dec_ref_obj",
        "inc_ref_obj",
        "const_int",
        "const_float",
        "const_bool",
        "const_none",
        "const_string",
        "nop",
        "line",
        "label",
        "state_label",
    ];
    let ops = &func_ir.ops;
    let len = ops.len();
    if len < 2 {
        return;
    }
    let mut remove = vec![false; len];
    for i in 1..len {
        if ops[i].kind != "check_exception" {
            continue;
        }
        // Walk backwards skipping nops, labels, and other non-raising ops
        // to find the "real" predecessor.
        let mut pred_idx = i - 1;
        while pred_idx > 0
            && matches!(
                ops[pred_idx].kind.as_str(),
                "nop" | "line" | "label" | "state_label"
            )
        {
            pred_idx -= 1;
        }
        let pred_kind = ops[pred_idx].kind.as_str();
        if NEVER_RAISES.contains(&pred_kind) {
            remove[i] = true;
        }
    }
    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn hoist_loop_invariants(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_LICM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len == 0 {
        return;
    }
    let mut loop_regions: Vec<(usize, usize)> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                let next_is_index = ops
                    .get(idx + 1)
                    .is_some_and(|next| next.kind == "loop_index_start");
                if !next_is_index {
                    loop_start_stack.push(idx);
                }
            }
            "loop_index_start" => {
                loop_start_stack.push(idx);
            }
            "loop_end" => {
                if let Some(start) = loop_start_stack.pop() {
                    loop_regions.push((start, idx));
                }
            }
            _ => {}
        }
    }
    if loop_regions.is_empty() {
        return;
    }
    let mut hoist_before: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut hoisted_set: BTreeSet<usize> = BTreeSet::new();
    for &(start, end) in &loop_regions {
        let mut defined_in_loop: HashSet<String> = HashSet::new();
        for idx in (start + 1)..end {
            if let Some(out) = ops[idx].out.as_deref() {
                defined_in_loop.insert(out.to_string());
            }
        }
        let mut to_hoist: Vec<usize> = Vec::new();
        for idx in (start + 1)..end {
            let op = &ops[idx];
            if !is_hoistable(op) {
                continue;
            }
            let inputs_outside = op.args.as_ref().map_or(true, |args| {
                args.iter().all(|arg| !defined_in_loop.contains(arg))
            });
            if !inputs_outside {
                continue;
            }
            if let Some(out) = op.out.as_deref() {
                let mut write_count = 0;
                for j in (start + 1)..end {
                    if ops[j].out.as_deref() == Some(out) {
                        write_count += 1;
                    }
                }
                if write_count > 1 {
                    continue;
                }
            }
            to_hoist.push(idx);
        }
        if !to_hoist.is_empty() {
            for &idx in &to_hoist {
                hoisted_set.insert(idx);
            }
            hoist_before.entry(start).or_default().extend(to_hoist);
        }
    }
    if hoisted_set.is_empty() {
        return;
    }
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(len);
    for (idx, op) in ops.iter().enumerate() {
        if let Some(hoisted_indices) = hoist_before.get(&idx) {
            for &hi in hoisted_indices {
                new_ops.push(ops[hi].clone());
            }
        }
        if hoisted_set.contains(&idx) {
            continue;
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}

/// Dead-function elimination: remove functions that are never referenced from
/// any reachable function.  The entry function (first in the list, typically
/// `<module>`) is always retained; any function reachable from it through
/// `call_internal`, `func_new`, `func_new_closure`, `func_new_builtin`,
/// or `code_new` references is kept.
///
/// This pass runs after inlining — if a callee was fully inlined into all
/// call sites, it becomes unreachable and will be eliminated here.
/// Applies to both native and WASM backends.
pub fn eliminate_dead_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_FUNC_ELIM").is_ok() {
        return;
    }
    if ir.functions.is_empty() {
        return;
    }

    // ── Stub molt_isolate_import / molt_isolate_bootstrap ──
    //
    // These functions must exist as linker symbols (the runtime references
    // them via extern "C"), but their bodies dispatch to every module init
    // function, preventing dead-function elimination from stripping unused
    // stdlib modules.  Replace their bodies with trivial stubs:
    //   molt_isolate_import  → const_none + ret  (return None)
    //   molt_isolate_bootstrap → ret_void
    //
    // For programs that actually use isolates, the full bodies are
    // preserved by reachability from molt_main (which calls them).
    // When they are NOT reachable from molt_main (the common case for
    // simple programs), their bodies are inert stubs.
    {
        // Check if molt_main actually calls either isolate function.
        let main_calls_isolate = ir.functions.iter()
            .find(|f| f.name == "molt_main")
            .map_or(false, |main_fn| {
                main_fn.ops.iter().any(|op| {
                    op.s_value.as_deref()
                        .map_or(false, |s| s.starts_with("molt_isolate_"))
                })
            });
        if !main_calls_isolate {
            for func in &mut ir.functions {
                if func.name == "molt_isolate_import" {
                    func.ops.clear();
                    func.ops.push(OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    });
                    func.ops.push(OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    });
                } else if func.name == "molt_isolate_bootstrap" {
                    func.ops.clear();
                    func.ops.push(OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    });
                }
            }
        }
    }

    // Build the call graph: function name -> set of referenced function names.
    // Use owned Strings so that `ir.functions` is not borrowed when we call retain().
    let defined: BTreeSet<String> = ir.functions.iter().map(|f| f.name.clone()).collect();
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for func in &ir.functions {
        let mut refs: BTreeSet<String> = BTreeSet::new();
        for op in &func.ops {
            match op.kind.as_str() {
                "call" | "call_internal" | "func_new" | "func_new_closure"
                | "func_new_builtin" | "code_new" | "call_guarded" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                    }
                }
                "call_indirect" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                    }
                }
                // alloc_task's s_value is the poll function name directly
                // (e.g., "foo_poll"). generator_create/coro_create reference
                // a base function whose companion _poll must also be kept.
                "alloc_task" | "generator_create" | "coro_create" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                        // generator_create/coro_create reference the base
                        // function; the backends derive "{base}_poll" at
                        // compile time, so mark both.
                        if !name.ends_with("_poll") {
                            let poll_name = format!("{name}_poll");
                            if defined.contains(poll_name.as_str()) {
                                refs.insert(poll_name);
                            }
                        }
                    }
                }
                // Other op kinds that legitimately reference functions by name.
                "task_new" | "generator_send"
                | "spawn" | "call_func" | "call_method" | "import_from"
                | "import_name" | "class_def" | "make_function" | "decorator"
                | "super_call" | "yield_from" | "await" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        references.insert(func.name.clone(), refs);
    }

    // BFS from entry roots to find all reachable functions.
    // Roots: (1) the first function (entry), (2) well-known linker/runtime
    // entry points, (3) any function whose name matches a keep-pattern.
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    let seed = |name: String, r: &mut BTreeSet<String>, q: &mut std::collections::VecDeque<String>| {
        if r.insert(name.clone()) {
            q.push_back(name);
        }
    };

    // (1) First function is always the module entry.
    seed(ir.functions[0].name.clone(), &mut reachable, &mut queue);

    // (2) + (3) Scan all functions for keep-patterns.
    //
    // molt_init_* functions are NOT blanket-kept.  They are referenced by
    // static CALL ops in the IR (emitted by the frontend's _emit_module_load)
    // so the BFS discovers them naturally.
    //
    // molt_isolate_* functions MUST be kept because the runtime library
    // references them as extern "C" symbols.  However, to enable effective
    // tree shaking, we stub out molt_isolate_import's body to just return
    // None — the actual module dispatch is handled by the frontend's lazy
    // init pattern, not by this runtime hook for simple programs.
    for func in &ir.functions {
        let name = &func.name;
        let keep = name == "molt_main"
            || name == "_start"
            // Must exist for the runtime linker; body is stubbed below.
            || name.starts_with("molt_isolate_");
        if keep {
            seed(name.clone(), &mut reachable, &mut queue);
        }
    }

    while let Some(current) = queue.pop_front() {
        if let Some(refs) = references.get(&current) {
            for target in refs {
                if reachable.insert(target.clone()) {
                    queue.push_back(target.clone());
                }
            }
        }
    }

    let original_count = ir.functions.len();
    ir.functions.retain(|f| reachable.contains(&f.name));
    let eliminated = original_count - ir.functions.len();

    if eliminated > 0 {
        if std::env::var("MOLT_DEBUG_DEAD_FUNC_ELIM").is_ok() {
            eprintln!(
                "dead-func-elim: removed {eliminated} of {original_count} functions ({} retained)",
                ir.functions.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Megafunction splitting pass
//
// Cranelift's register allocator has O(n^2) behavior on very large functions.
// When a function exceeds max_ops (default 4000, env: MOLT_MAX_FUNCTION_OPS),
// this pass splits it at top-level statement boundaries (loop_depth=0,
// if_depth=0) into private __molt_chunk_{name}_{n} functions.  The original
// function is replaced with sequential call_internal ops to each chunk.
//
// Safety: never splits inside loops, if-blocks, or try-blocks.
// ---------------------------------------------------------------------------

/// Default maximum number of ops before a function is split into chunks.
const DEFAULT_MAX_FUNCTION_OPS: usize = 2000;

/// Split a single large function into multiple chunk functions.
///
/// Returns `Err(func)` (giving back the original) if the function is small
/// enough or no safe split points exist; otherwise returns `Ok((stub, chunks))`
/// where `stub` is the replacement parent function and `chunks` are the
/// extracted pieces.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn split_large_function(func: FunctionIR, max_ops: usize) -> Result<(FunctionIR, Vec<FunctionIR>), FunctionIR> {
    if func.ops.len() <= max_ops {
        return Err(func);
    }

    // Functions with exception handling CAN be split, but only at boundaries
    // outside check_exception → label spans. The forbidden_ranges logic below
    // ensures split points don't bisect exception handler control flow.

    // ---------------------------------------------------------------
    // 1. Find safe split points (indices where depth == 0).
    //    A split point is the index of the *first* op of a new chunk,
    //    i.e. the boundary falls just before that index.
    //
    //    Additionally, we must not split between a `check_exception`
    //    and its target `label`/`state_label`, since the function
    //    compiler expects both to be in the same chunk.
    // ---------------------------------------------------------------

    // Build forbidden ranges: for each label_id referenced by
    // check_exception, find the span [earliest_ref, label_def] (or
    // [label_def, latest_ref] if the label comes first) and forbid
    // splitting within that range.
    let mut label_positions: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    let mut check_exc_refs: std::collections::BTreeMap<i64, (usize, usize)> = std::collections::BTreeMap::new();
    for (idx, op) in func.ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    label_positions.insert(id, idx);
                }
            }
            "check_exception" => {
                if let Some(id) = op.value {
                    let entry = check_exc_refs.entry(id).or_insert((idx, idx));
                    entry.0 = entry.0.min(idx);
                    entry.1 = entry.1.max(idx);
                }
            }
            _ => {}
        }
    }
    // Compute forbidden ranges: a split point at index `sp` is forbidden
    // if it falls strictly between a check_exception and its target label.
    let mut forbidden_ranges: Vec<(usize, usize)> = Vec::new();
    for (label_id, (earliest_check, latest_check)) in &check_exc_refs {
        if let Some(&label_idx) = label_positions.get(label_id) {
            let range_start = (*earliest_check).min(label_idx);
            let range_end = (*latest_check).max(label_idx);
            forbidden_ranges.push((range_start, range_end));
        }
    }

    let is_forbidden = |sp: usize| -> bool {
        for &(start, end) in &forbidden_ranges {
            // sp is the first index of the new chunk; splitting here means
            // indices [0..sp) go to one chunk and [sp..) go to the next.
            // Forbidden if the range straddles the split point.
            if start < sp && sp <= end {
                return true;
            }
        }
        false
    };


    let mut split_candidates: Vec<usize> = Vec::new();
    let mut depth: i32 = 0;

    for (idx, op) in func.ops.iter().enumerate() {
        // At depth 0 before processing this op, this is a valid split point.
        if depth == 0 && idx > 0 && !is_forbidden(idx) {
            split_candidates.push(idx);
        }

        match op.kind.as_str() {
            // Openers -- increase nesting depth
            "if" | "loop_start" | "loop_index_start" | "for_iter_start"
            | "while_start" | "try_start" | "async_for_start" => {
                depth += 1;
            }
            // Closers -- decrease nesting depth
            "end_if" | "loop_end" | "loop_index_end" | "for_iter_end"
            | "while_end" | "try_end" | "async_for_end" => {
                depth -= 1;
            }
            _ => {}
        }
    }

    if split_candidates.is_empty() {
        return Err(func);
    }

    // ---------------------------------------------------------------
    // 2. Select split points to keep chunks roughly <= max_ops.
    // ---------------------------------------------------------------
    let mut selected: Vec<usize> = Vec::new();
    let mut last_split = 0usize;
    for &sp in &split_candidates {
        let chunk_len = sp - last_split;
        if chunk_len >= max_ops {
            selected.push(sp);
            last_split = sp;
        }
    }

    // If no selected splits, the function is too deeply nested to split.
    if selected.is_empty() {
        return Err(func);
    }

    // ---------------------------------------------------------------
    // 3. Partition ops into chunks at the selected split points.
    // ---------------------------------------------------------------
    let mut boundaries: Vec<usize> = Vec::new();
    boundaries.push(0);
    boundaries.extend_from_slice(&selected);
    boundaries.push(func.ops.len());

    // Validate: ensure no chunk exceeds max_ops. If any chunk is oversized,
    // the function has a deeply nested region that can't be split cleanly.
    for window in boundaries.windows(2) {
        let chunk_size = window[1] - window[0];
        if chunk_size > max_ops * 2 {
            // Allow up to 2x max_ops for the final chunk — beyond that,
            // return Err to fall back to single-module compilation.
            return Err(func);
        }
    }

    let sanitized_name = func
        .name
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_");

    let mut chunks: Vec<FunctionIR> = Vec::new();
    let all_ops = func.ops;

    for i in 0..boundaries.len() - 1 {
        let start = boundaries[i];
        let end = boundaries[i + 1];
        let mut chunk_ops: Vec<OpIR> = all_ops[start..end].to_vec();

        // Collect label IDs defined in THIS chunk.
        let chunk_labels: std::collections::BTreeSet<i64> = chunk_ops
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
            .filter_map(|op| op.value)
            .collect();

        // Strip check_exception ops that reference labels NOT in this chunk.
        // These are dead code — the target label is in another chunk.
        chunk_ops.retain(|op| {
            if op.kind == "check_exception" {
                if let Some(target_id) = op.value {
                    return chunk_labels.contains(&target_id);
                }
            }
            true
        });

        let chunk_name = format!("__molt_chunk_{sanitized_name}_{i}");
        chunks.push(FunctionIR {
            name: chunk_name,
            params: Vec::new(),
            ops: chunk_ops,
            param_types: None,
        });
    }

    // ---------------------------------------------------------------
    // 4. Build the stub parent function that calls each chunk.
    // ---------------------------------------------------------------
    let mut stub_ops: Vec<OpIR> = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        stub_ops.push(OpIR {
            kind: "call_internal".to_string(),
            s_value: Some(chunk.name.clone()),
            args: Some(Vec::new()),
            out: Some("none".to_string()),
            ..OpIR::default()
        });
    }

    let stub = FunctionIR {
        name: func.name,
        params: func.params,
        ops: stub_ops,
        param_types: func.param_types,
    };

    Ok((stub, chunks))
}

/// Apply megafunction splitting to all oversized functions in the IR.
///
/// Call this before the main compilation loop so that the chunk functions
/// are present in `ir.functions` and will be compiled normally.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn split_megafunctions(ir: &mut SimpleIR) {
    let max_ops: usize = std::env::var("MOLT_MAX_FUNCTION_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FUNCTION_OPS);

    let mut new_functions: Vec<FunctionIR> = Vec::new();
    let old_functions = std::mem::take(&mut ir.functions);

    for func in old_functions {
        let op_count = func.ops.len();
        match split_large_function(func, max_ops) {
            Ok((stub, chunks)) => {
                eprintln!(
                    "MOLT_BACKEND: split `{}` ({} ops) into {} chunks",
                    stub.name,
                    op_count,
                    chunks.len()
                );
                // Insert chunks first so they are defined before the stub calls them.
                new_functions.extend(chunks);
                new_functions.push(stub);
            }
            Err(original) => {
                new_functions.push(original);
            }
        }
    }

    ir.functions = new_functions;
}

/// Rewrite loops that contain `state_yield` in stateful (generator/async)
/// functions so the native backend can resume inside the loop body.
///
/// Problem: the native Cranelift backend tracks loop context on a runtime
/// `loop_stack`.  When a generator yields inside a loop and later resumes,
/// the `loop_start` that pushed the frame is skipped (the state machine
/// jumps directly to the resume block).  Any subsequent `loop_continue`
/// finds an empty `loop_stack` and falls through, ending the generator
/// after a single yield.
///
/// Fix: for every loop that encloses a `state_yield`, insert a
/// `state_label` at the loop body start and rewrite `loop_continue` ops
/// to `jump` ops targeting that label.  The native backend treats
/// `state_label` as a resume-eligible block and `jump` as an
/// unconditional branch — both work correctly across state-machine
/// boundaries.
pub fn rewrite_stateful_loops(func_ir: &mut FunctionIR) {
    // Only transform stateful functions (generators / async).
    let is_stateful = func_ir.ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "state_switch" | "state_transition" | "state_yield" | "chan_send_yield" | "chan_recv_yield"
        )
    });
    if !is_stateful {
        return;
    }

    // Find the maximum state ID already in use so we can allocate fresh IDs.
    let mut max_state_id: i64 = 0;
    for op in &func_ir.ops {
        if let Some(id) = op.value {
            if matches!(
                op.kind.as_str(),
                "state_yield"
                    | "state_transition"
                    | "state_label"
                    | "label"
                    | "chan_send_yield"
                    | "chan_recv_yield"
            ) {
                max_state_id = max_state_id.max(id);
            }
        }
    }
    let mut next_state_id = max_state_id + 100; // leave headroom

    // Build a stack of loop start indices and find which loops contain yields.
    struct LoopInfo {
        start_idx: usize,
        end_idx: usize,
        has_yield: bool,
        continues: Vec<usize>,
        breaks: Vec<usize>,
        break_if_trues: Vec<usize>,
        break_if_falses: Vec<usize>,
    }
    let mut loop_stack: Vec<LoopInfo> = Vec::new();
    let mut finished_loops: Vec<LoopInfo> = Vec::new();

    for (idx, op) in func_ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" | "loop_index_start" => {
                loop_stack.push(LoopInfo {
                    start_idx: idx,
                    end_idx: 0,
                    has_yield: false,
                    continues: Vec::new(),
                    breaks: Vec::new(),
                    break_if_trues: Vec::new(),
                    break_if_falses: Vec::new(),
                });
            }
            "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                for frame in loop_stack.iter_mut() {
                    frame.has_yield = true;
                }
            }
            "loop_continue" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.continues.push(idx);
                }
            }
            "loop_break" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.breaks.push(idx);
                }
            }
            "loop_break_if_true" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_trues.push(idx);
                }
            }
            "loop_break_if_false" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_falses.push(idx);
                }
            }
            "loop_end" => {
                if let Some(mut frame) = loop_stack.pop() {
                    frame.end_idx = idx;
                    finished_loops.push(frame);
                }
            }
            _ => {}
        }
    }

    let yield_loops: Vec<LoopInfo> = finished_loops
        .into_iter()
        .filter(|l| l.has_yield)
        .collect();

    if yield_loops.is_empty() {
        return;
    }

    // For each yield-containing loop, allocate TWO state labels:
    //   body_label  — at loop body start (continue / back-edge target)
    //   after_label — after loop_end (break target)
    // Replace ALL structured loop ops with labels and jumps so the
    // state machine works correctly on resume.
    let mut body_label_for_start: BTreeMap<usize, i64> = BTreeMap::new();
    let mut after_label_for_end: BTreeMap<usize, i64> = BTreeMap::new();
    let mut continue_target: BTreeMap<usize, i64> = BTreeMap::new();
    let mut break_target: BTreeMap<usize, i64> = BTreeMap::new();

    for info in &yield_loops {
        let body_label = next_state_id;
        next_state_id += 1;
        let after_label = next_state_id;
        next_state_id += 1;

        body_label_for_start.insert(info.start_idx, body_label);
        after_label_for_end.insert(info.end_idx, after_label);

        for &ci in &info.continues {
            continue_target.insert(ci, body_label);
        }
        for &bi in &info.breaks {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_trues {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_falses {
            break_target.insert(bi, after_label);
        }
    }

    // Rebuild the ops list, replacing structured loop ops with labels/jumps.
    let old_ops = std::mem::take(&mut func_ir.ops);
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(old_ops.len() + yield_loops.len() * 4);

    for (idx, op) in old_ops.into_iter().enumerate() {
        if let Some(&body_label) = body_label_for_start.get(&idx) {
            // Replace loop_start with state_label (loop body entry).
            new_ops.push(OpIR {
                kind: "state_label".to_string(),
                value: Some(body_label),
                ..OpIR::default()
            });
        } else if let Some(&after_label) = after_label_for_end.get(&idx) {
            // Replace loop_end with state_label (break target).
            new_ops.push(OpIR {
                kind: "state_label".to_string(),
                value: Some(after_label),
                ..OpIR::default()
            });
        } else if let Some(&target) = continue_target.get(&idx) {
            // Replace loop_continue with jump to body label.
            new_ops.push(OpIR {
                kind: "jump".to_string(),
                value: Some(target),
                ..OpIR::default()
            });
        } else if let Some(&target) = break_target.get(&idx) {
            match op.kind.as_str() {
                "loop_break" => {
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                }
                "loop_break_if_true" => {
                    // Expand: if(cond) { jump(after_label) } end_if
                    new_ops.push(OpIR {
                        kind: "if".to_string(),
                        args: op.args.clone(),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    });
                }
                "loop_break_if_false" => {
                    let cond = op.args.as_ref()
                        .and_then(|a| a.first().cloned())
                        .unwrap_or_default();
                    let not_var = format!("__slr_not_{idx}");
                    new_ops.push(OpIR {
                        kind: "not".to_string(),
                        args: Some(vec![cond]),
                        out: Some(not_var.clone()),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "if".to_string(),
                        args: Some(vec![not_var]),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    });
                }
                _ => new_ops.push(op),
            }
        } else {
            new_ops.push(op);
        }
    }

    func_ir.ops = new_ops;
}

