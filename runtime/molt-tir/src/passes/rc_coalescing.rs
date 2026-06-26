use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, HashSet};

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
        if op.kind == "const"
            && let (Some(out), Some(val)) = (op.out.as_ref(), op.value)
        {
            // Only store the first definition (SSA correctness).
            map.entry(out.clone()).or_insert(val);
        }
    }
    map
}

/// Identify pairs of `inc_ref`/`dec_ref` ops that cancel within a basic block.
/// Returns: (set of op indices to skip, set of variable names whose dec_ref to skip).
pub fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    const CONTROL_FLOW: &[&str] = &[
        "if",
        "else",
        "end_if",
        "jump",
        "br_if",
        "label",
        "check_exception",
        "state_transition",
        "state_yield",
        "state_switch",
        "state_label",
        "exception_push",
        "exception_pop",
        "chan_send_yield",
        "chan_recv_yield",
        "ret",
        "ret_void",
        "loop_start",
        "loop_index_start",
        "loop_end",
        "loop_break_if_true",
        "loop_break_if_false",
        // Value-less conditional break gated on the runtime exception flag.  It
        // is a real control-flow boundary (the loop may exit here on a pending
        // exception), so inc_ref/dec_ref coalescing MUST NOT scan across it —
        // otherwise a dec_ref placed after the break could be skipped on the
        // exception-exit path, leaking the referenced object.
        "loop_break_if_exception",
        "loop_continue",
    ];
    let cf_set: HashSet<&str> = CONTROL_FLOW.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) {
            continue;
        }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec {
            continue;
        }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) {
                break;
            }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release") && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow") && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b
                .args
                .as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var {
                break;
            }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) {
            continue;
        }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") {
            continue;
        }
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
        if let Some(var) = &op.var
            && var != "none"
        {
            last_use.insert(var.clone(), i);
        }
        if let Some(args) = &op.args {
            for name in args {
                if name != "none" {
                    last_use.insert(name.clone(), i);
                }
            }
        }
        if let Some(out) = &op.out
            && out != "none"
        {
            last_use.insert(out.clone(), i);
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
        if matches!(op.kind.as_str(), "dec_ref" | "release")
            && let Some(arg) = op.args.as_ref().and_then(|a| a.first())
            && skip_dec_ref.contains(arg.as_str())
        {
            continue;
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}

// ---------------------------------------------------------------------------
// Loop-Invariant Code Motion (LICM)
// ---------------------------------------------------------------------------
