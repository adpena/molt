use crate::tir::op_kinds_generated::kind_to_opcode_table;
use crate::tir::ops::OpCode;
use crate::tir::simple_def_use::visit_simple_ir_defined_names;
use crate::{FunctionIR, OpIR};
use std::collections::BTreeMap;
use molt_ir::deterministic_hash::HashSet;

// ---------------------------------------------------------------------------
// Pre-built constant integer map for O(log n) lookups during compilation.
//
// Record only unambiguous generated ConstInt producers. Authored SimpleIR
// names are not necessarily SSA: parameters, rebinding and multi-result
// definitions must never acquire a function-wide constant from one producer.
// ---------------------------------------------------------------------------

pub fn build_const_int_map(function: &FunctionIR) -> BTreeMap<String, i64> {
    let mut map = BTreeMap::new();
    let mut defined: HashSet<&str> = function.params.iter().map(String::as_str).collect();
    for op in &function.ops {
        let constant = (kind_to_opcode_table(op.kind) == Some(OpCode::ConstInt))
            .then_some(op.value)
            .flatten();
        visit_simple_ir_defined_names(op, |name| {
            // The field-role visitor is not a name validator. Malformed empty
            // transports cannot establish a usable constant fact.
            if name.is_empty() {
                return;
            }
            if !defined.insert(name) {
                map.remove(name);
            } else if let Some(value) = constant {
                map.insert(name.to_string(), value);
            }
        });
    }
    map
}

/// Identify pairs of `inc_ref`/`dec_ref` ops that cancel within a basic block.
/// Returns: (set of op indices to skip, set of variable names whose dec_ref to skip).
pub fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    const CONTROL_FLOW: &[molt_ir::OpKind] = &[
        molt_ir::OpKind::If,
        molt_ir::OpKind::Else,
        molt_ir::OpKind::EndIf,
        molt_ir::OpKind::Jump,
        molt_ir::OpKind::BrIf,
        molt_ir::OpKind::Label,
        molt_ir::OpKind::CheckException,
        molt_ir::OpKind::StateTransition,
        molt_ir::OpKind::StateYield,
        molt_ir::OpKind::StateSwitch,
        molt_ir::OpKind::StateLabel,
        molt_ir::OpKind::ExceptionPush,
        molt_ir::OpKind::ExceptionPop,
        molt_ir::OpKind::Ret,
        molt_ir::OpKind::RetVoid,
        molt_ir::OpKind::LoopStart,
        molt_ir::OpKind::LoopIndexStart,
        molt_ir::OpKind::LoopEnd,
        molt_ir::OpKind::LoopBreakIfTrue,
        molt_ir::OpKind::LoopBreakIfFalse,
        // Value-less conditional break gated on the runtime exception flag.  It
        // is a real control-flow boundary (the loop may exit here on a pending
        // exception), so inc_ref/dec_ref coalescing MUST NOT scan across it —
        // otherwise a dec_ref placed after the break could be skipped on the
        // exception-exit path, leaking the referenced object.
        molt_ir::OpKind::LoopBreakIfException,
        molt_ir::OpKind::LoopContinue,
    ];
    let cf_set: HashSet<molt_ir::OpKind> = CONTROL_FLOW.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::default();
    let mut skip_dec_ref: HashSet<String> = HashSet::default();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) {
            continue;
        }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind, molt_ir::OpKind::IncRef | molt_ir::OpKind::Borrow);
        let a_is_dec = matches!(a.kind, molt_ir::OpKind::DecRef | molt_ir::OpKind::Release);
        if !a_is_inc && !a_is_dec {
            continue;
        }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(&b.kind) {
                break;
            }
            let b_kind = b.kind;
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, molt_ir::OpKind::DecRef | molt_ir::OpKind::Release)
                    && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, molt_ir::OpKind::IncRef | molt_ir::OpKind::Borrow)
                    && b_arg.map(String::as_str) == Some(&a_arg)
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
        if !matches!(op.kind, molt_ir::OpKind::IncRef | molt_ir::OpKind::Borrow) {
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
