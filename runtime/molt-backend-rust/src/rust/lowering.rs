use super::rust_ident;
use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet};

// ── IR lowering passes (shared logic, simpler than Luau variants) ─────────────

/// Mark unreachable ops after return as nop so they don't emit dead code.
pub(super) fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None;
    for op in ops {
        let kind = op.kind.as_str();
        let is_open = matches!(
            kind,
            "if" | "if_not" | "loop_start" | "while_start" | "for_range" | "for_iter"
        );
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "while_end" | "end_for");

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            if let Some(d) = dead_at_depth
                && d > depth
            {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            dead_at_depth = None;
        }

        let is_terminator = matches!(
            kind,
            "ret"
                | "return"
                | "return_value"
                | "return_none"
                | "ret_none"
                | "ret_void"
                | "jump"
                | "raise"
                | "reraise"
        );
        result.push(op.clone());
        if is_terminator {
            dead_at_depth = Some(depth);
        }
    }
    result
}

/// Lower early returns (store+jump→ret pattern) — no-op for Rust since we emit `return`.
pub(super) fn lower_early_returns(ops: &[OpIR]) -> Vec<OpIR> {
    ops.to_vec()
}

/// Convert `call iter() + for_iter` patterns to plain for_iter if already present.
pub(super) fn lower_iter_to_for(ops: &[OpIR]) -> Vec<OpIR> {
    ops.to_vec()
}

// ── Phi hoisting helpers ──────────────────────────────────────────────────────

pub(super) fn collect_phi_assignments(
    ops: &[OpIR],
    hoisted_vars: &mut BTreeSet<String>,
) -> BTreeMap<usize, Vec<(String, Vec<String>)>> {
    let mut phi_assignments: BTreeMap<usize, Vec<(String, Vec<String>)>> = BTreeMap::new();
    let mut i = 0;
    while i < ops.len() {
        if ops[i].kind == "end_if" {
            let end_if_idx = i;
            let mut j = i + 1;
            while j < ops.len() && ops[j].kind == "phi" {
                if let Some(ref out_name) = ops[j].out {
                    let phi_var = rust_ident(out_name);
                    let args: Vec<String> = ops[j]
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| rust_ident(a))
                        .collect();
                    phi_assignments
                        .entry(end_if_idx)
                        .or_default()
                        .push((phi_var.clone(), args));
                    hoisted_vars.insert(phi_var);
                }
                j += 1;
            }
        }
        i += 1;
    }
    phi_assignments
}

pub(super) fn build_phi_injection_maps(
    ops: &[OpIR],
    phi_assignments: &BTreeMap<usize, Vec<(String, Vec<String>)>>,
) -> (
    BTreeMap<usize, Vec<(String, String)>>,
    BTreeMap<usize, Vec<(String, String)>>,
) {
    let mut before_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut before_end_if: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "if" | "if_not" => if_stack.push((idx, None)),
            "else" => {
                if let Some(last) = if_stack.last_mut() {
                    last.1 = Some(idx);
                }
            }
            "end_if" => {
                if let Some((_if_idx, else_idx)) = if_stack.pop()
                    && let Some(phis) = phi_assignments.get(&idx)
                {
                    for (phi_var, args) in phis {
                        if let Some(else_i) = else_idx {
                            let true_val = args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_else
                                .entry(else_i)
                                .or_default()
                                .push((phi_var.clone(), true_val));
                            let false_val = args
                                .get(1)
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_end_if
                                .entry(idx)
                                .or_default()
                                .push((phi_var.clone(), false_val));
                        } else {
                            let true_val = args
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "MoltValue::None".to_string());
                            before_end_if
                                .entry(idx)
                                .or_default()
                                .push((phi_var.clone(), true_val));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (before_else, before_end_if)
}

pub(super) fn collect_scope_escapes(
    ops: &[OpIR],
    func: &FunctionIR,
    hoisted_vars: &mut BTreeSet<String>,
) {
    let mut depth: i32 = 0;
    let mut decl_depth: BTreeMap<String, i32> = BTreeMap::new();
    let param_set: BTreeSet<String> = func.params.iter().map(|p| rust_ident(p)).collect();

    for op in ops {
        match op.kind.as_str() {
            "if" | "if_not" | "loop_start" | "while_start" | "for_range" | "for_iter" => depth += 1,
            "end_if" | "loop_end" | "while_end" | "end_for" => depth -= 1,
            _ => {}
        }
        if let Some(ref out_name) = op.out
            && out_name != "none"
            && !op.kind.starts_with("nop")
        {
            let var = rust_ident(out_name);
            decl_depth.entry(var).or_insert(depth);
        }
        let mut refs: Vec<String> = op
            .args
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|s| rust_ident(s))
            .collect();
        if let Some(v) = op.var.as_deref() {
            refs.push(rust_ident(v));
        }
        for r in refs {
            if param_set.contains(&r) {
                continue;
            }
            if let Some(&dd) = decl_depth.get(&r)
                && dd > depth
            {
                hoisted_vars.insert(r);
            }
        }
    }
}
