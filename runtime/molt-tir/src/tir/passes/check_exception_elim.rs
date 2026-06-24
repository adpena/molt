//! Redundant `CheckException` elimination pass.
//!
//! The frontend liberally emits `CHECK_EXCEPTION` after every statement
//! within a try block (and within functions that have a function-level
//! exception label).  Many of these checks are redundant because the
//! intervening ops cannot raise — pure arithmetic, constants, variable
//! load/store, comparisons on known types, etc.
//!
//! This pass runs a small forward dataflow analysis and removes any
//! `CheckException` op that follows only non-raising ops since the
//! previous observed/cleared exception state, including across normal
//! CFG edges.  Exception-handler targets stay conservatively seeded as
//! pending-possible, so handler entry semantics are preserved while
//! normal fallthrough blocks do not pay an unconditional first-poll tax.
//!
//! Targets bench_exception_heavy and other try-block-bearing loops
//! where the per-iter check_exception count drives noticeable
//! per-instruction overhead.
//!
//! ## Safety
//!
//! `CheckException` is a side-effecting op (it branches to a handler
//! when the runtime exception flag is set).  Removing one is safe iff
//! no op since the previous check could have set the flag — i.e. the
//! intervening ops are all in the "cannot raise" set.  The base
//! classifier delegates to the same op-aware TIR effects oracle that DCE
//! uses, then tightens it with local TIR facts for operations whose only
//! remaining exceptional case has been statically excluded (for example
//! integer division-family ops by a proven nonzero const).

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::effects::op_may_throw;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    LiteralPayloadKind, opcode_literal_payload_kind_table,
    opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// SimpleIR op kinds that fall through to `OpCode::Copy` in the SSA lift
/// (so they carry `_original_kind`) but are nevertheless provably
/// non-throwing.  Hoisted into a const set here so the per-op check is
/// O(1).  Anything *not* on this list is treated as throwing — a
/// conservative choice consistent with DCE's safety policy.
fn original_kind_is_provably_nonthrowing(kind: &str) -> bool {
    matches!(
        kind,
        // Type-tag / layout guards: emit a typed branch on mismatch
        // but never raise; the slow path falls through to the
        // polymorphic op which the pass already sees separately.
        "guard_tag"
            | "guard_layout"
            | "guard_int"
            | "guard_float"
            | "guard_str"
            | "guard_bool"
            | "guard_none"
            // Field-offset stores/loads against a layout-guarded
            // object are pure memory ops by construction.
            | "store"
            | "load"
            // Exception-state queries that read or clear pending
            // state without raising.
            | "exception_clear"
            | "exception_last"
            | "exception_last_pending"
            | "exception_finally_pending_observer"
            | "exception_pop"
            | "exception_push"
            | "exception_new_builtin"
            | "exception_new_builtin_empty"
            | "exception_new_builtin_one"
            | "exception_match_builtin"
            | "exception_stack_enter"
            | "exception_stack_clear"
            | "exception_stack_depth"
            | "exception_context_set"
            // Try/with control-flow markers — the structured
            // try/except wraps potentially-raising body ops which
            // appear separately in the linear IR; the markers
            // themselves don't raise.
            | "try_start"
            | "try_end"
            | "context_depth"
            // Diagnostic / metadata markers.
            | "trace_enter_slot"
            | "trace_exit"
            | "line"
            | "code_slots_init"
            | "code_slot_set"
            | "code_new"
            // Comparison / boolean ops on already-typed values.
            // (Untyped variants land on the dedicated
            // OpCode::Lt/Eq/etc. paths and are gated by
            // is_potentially_throwing instead.)
            | "is"
            | "is_not"
            | "not"
            | "and"
            | "or"
            | "bool"
            // Loop bookkeeping markers — control flow without
            // exception semantics.
            | "loop_start"
            | "loop_end"
            | "loop_continue"
            | "loop_break"
            | "loop_break_if_false"
            | "loop_index_start"
            | "loop_index_next"
            // Identity helpers introduced by lowering.
            | "missing"
            | "phi"
            | "identity_alias"
            | "copy_var"
    )
}

/// Returns `true` if this op may raise an exception.
///
/// Wraps the shared TIR effects oracle with an `_original_kind`
/// classifier for unmapped SimpleIR ops. A `Copy` op carrying
/// `_original_kind` represents an op the SSA lift did not have a
/// dedicated `OpCode` for; whether it can raise depends on the
/// original SimpleIR kind, not on `OpCode::Copy` itself.  Without this
/// classifier the predicate would either over-approximate (treating
/// every unmapped op as raising and producing zero `check_exception`
/// elision in fast-int loops) or under-approximate (treating them all
/// as non-raising and dropping the safety guards that protect
/// `exception_new` / `exception_class` etc.).
fn const_int_values(func: &TirFunction) -> HashMap<ValueId, i64> {
    let mut values = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            let value = match opcode_literal_payload_kind_table(op.opcode) {
                Some(LiteralPayloadKind::Int) => match op.attrs.get("value") {
                    Some(AttrValue::Int(value)) => Some(*value),
                    _ => None,
                },
                Some(LiteralPayloadKind::Bool) => match op.attrs.get("value") {
                    Some(AttrValue::Bool(value)) => Some(i64::from(*value)),
                    Some(AttrValue::Int(value)) => Some(i64::from(*value != 0)),
                    _ => None,
                },
                None => None,
            };
            if let Some(value) = value {
                for result in &op.results {
                    values.insert(*result, value);
                }
            }
        }
    }
    values
}

fn value_is_i64(value_types: &HashMap<ValueId, TirType>, value: ValueId) -> bool {
    matches!(value_types.get(&value), Some(TirType::I64))
}

fn proven_nonzero_i64_divisor(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    let [lhs, rhs] = op.operands.as_slice() else {
        return false;
    };
    value_is_i64(value_types, *lhs)
        && value_is_i64(value_types, *rhs)
        && const_ints.get(rhs).is_some_and(|value| *value != 0)
}

fn op_may_raise(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode)
        && proven_nonzero_i64_divisor(value_types, const_ints, op)
    {
        return false;
    }
    if op_may_throw(op) {
        return true;
    }
    if op.opcode == OpCode::Copy {
        if let Some(AttrValue::Str(orig)) = op.attrs.get("_original_kind") {
            return !original_kind_is_provably_nonthrowing(orig);
        }
        // No `_original_kind` → real Copy / store_var / load_var, all safe.
        return false;
    }
    false
}

fn op_clears_pending_exception(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    matches!(
        op.attrs.get("_original_kind"),
        Some(AttrValue::Str(orig)) if orig == "exception_clear"
    )
}

fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.push(*default);
            successors.extend(cases.iter().map(|(_, target, _)| *target));
            successors
        }
        Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
    }
}

fn exception_target_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid, &label)| (label, BlockId(bid)))
        .collect();
    let mut targets = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if dominators::is_exception_transfer_edge(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = label_to_block.get(label)
            {
                targets.insert(target);
            }
        }
    }
    targets
}

fn transfer_block_pending(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    block: &crate::tir::blocks::TirBlock,
    mut pending: bool,
) -> bool {
    for op in &block.ops {
        if op.opcode == OpCode::CheckException {
            if pending {
                pending = false;
            }
            continue;
        }
        if op_clears_pending_exception(op) {
            pending = false;
        } else if op_may_raise(value_types, const_ints, op) {
            pending = true;
        }
    }
    pending
}

fn compute_block_entry_pending(func: &TirFunction) -> HashMap<BlockId, bool> {
    let exception_targets = exception_target_blocks(func);
    let const_ints = const_int_values(func);
    let value_types = func.value_types.clone();
    let mut entry_pending: HashMap<BlockId, bool> = func
        .blocks
        .keys()
        .copied()
        .map(|bid| (bid, false))
        .collect();
    entry_pending.insert(func.entry_block, true);
    for target in &exception_targets {
        entry_pending.insert(*target, true);
    }

    loop {
        let mut next: HashMap<BlockId, bool> = func
            .blocks
            .keys()
            .copied()
            .map(|bid| (bid, false))
            .collect();
        next.insert(func.entry_block, true);
        for target in &exception_targets {
            next.insert(*target, true);
        }

        for (bid, block) in &func.blocks {
            let starts_pending = entry_pending.get(bid).copied().unwrap_or(false);
            let exits_pending =
                transfer_block_pending(&value_types, &const_ints, block, starts_pending);
            if !exits_pending {
                continue;
            }
            for succ in terminator_successors(&block.terminator) {
                if func.blocks.contains_key(&succ) {
                    next.insert(succ, true);
                }
            }
        }

        if next == entry_pending {
            return entry_pending;
        }
        entry_pending = next;
    }
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "check_exception_elim",
        ..Default::default()
    };

    let entry_pending = compute_block_entry_pending(func);
    let const_ints = const_int_values(func);
    let value_types = func.value_types.clone();

    for block in func.blocks.values_mut() {
        // `pending_exception_possible` is true when an entry edge may
        // carry pending exception state, after any potentially throwing
        // op, and false after an observed check or explicit clear.
        // When false, a `CheckException` can be elided.
        let mut pending_exception_possible = entry_pending.get(&block.id).copied().unwrap_or(false);
        let mut new_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op.opcode {
                OpCode::CheckException => {
                    if pending_exception_possible {
                        // Keep this check.  It clears the pending
                        // possibility for subsequent ops.
                        pending_exception_possible = false;
                        new_ops.push(op);
                    } else {
                        // Redundant — drop.
                        stats.ops_removed += 1;
                    }
                }
                _ => {
                    if op_clears_pending_exception(&op) {
                        pending_exception_possible = false;
                    } else if op_may_raise(&value_types, &const_ints, &op) {
                        pending_exception_possible = true;
                    }
                    new_ops.push(op);
                }
            }
        }
        block.ops = new_ops;
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::effects::EffectProof;
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use std::collections::HashMap;

    fn make_check_exception() -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(100));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    fn make_const_int(value: i64, out: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![out],
            attrs,
            source_span: None,
        }
    }

    fn make_call(callee: &str, out: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str(callee.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![out],
            attrs,
            source_span: None,
        }
    }

    fn make_module_get_attr(module: ValueId, attr_name: ValueId, out: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ModuleGetAttr,
            operands: vec![module, attr_name],
            results: vec![out],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_effect_proven_module_get_attr(
        module: ValueId,
        attr_name: ValueId,
        out: ValueId,
    ) -> TirOp {
        let mut op = make_module_get_attr(module, attr_name, out);
        op.attrs.insert(
            "effect_proof".into(),
            AttrValue::Str(EffectProof::StaticModuleClassBinding.name().into()),
        );
        op
    }

    fn make_binary(opcode: OpCode, lhs: ValueId, rhs: ValueId, out: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![out],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_original_kind(kind: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    fn make_func_with_block(ops: Vec<TirOp>) -> TirFunction {
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![],
            ops,
            terminator: Terminator::Return { values: vec![] },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        TirFunction {
            name: "test".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 100,
            next_block: 1,
            ..TirFunction::new("test".into(), vec![], TirType::None)
        }
    }

    fn make_two_block_func(entry_ops: Vec<TirOp>, successor_ops: Vec<TirOp>) -> TirFunction {
        let entry_id = BlockId(0);
        let successor_id = BlockId(1);
        let entry = TirBlock {
            id: entry_id,
            args: vec![],
            ops: entry_ops,
            terminator: Terminator::Branch {
                target: successor_id,
                args: vec![],
            },
        };
        let successor = TirBlock {
            id: successor_id,
            args: vec![],
            ops: successor_ops,
            terminator: Terminator::Return { values: vec![] },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);
        blocks.insert(successor_id, successor);
        TirFunction {
            name: "two_block_test".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 100,
            next_block: 2,
            ..TirFunction::new("two_block_test".into(), vec![], TirType::None)
        }
    }

    #[test]
    fn first_check_kept() {
        let mut func =
            make_func_with_block(vec![make_const_int(1, ValueId(0)), make_check_exception()]);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
    }

    #[test]
    fn redundant_check_after_pure_ops_dropped() {
        let mut func = make_func_with_block(vec![
            make_const_int(1, ValueId(0)),
            make_check_exception(), // first check, kept
            make_const_int(2, ValueId(1)),
            make_const_int(3, ValueId(2)),
            make_check_exception(), // redundant, dropped
        ]);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
    }

    #[test]
    fn check_after_call_kept() {
        let mut func = make_func_with_block(vec![
            make_const_int(1, ValueId(0)),
            make_check_exception(), // first check, kept
            make_call("foo", ValueId(1)),
            make_check_exception(), // after call (raising), kept
        ]);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
    }

    #[test]
    fn check_after_effect_proven_static_module_class_read_is_dropped() {
        let mut func = make_func_with_block(vec![
            make_check_exception(), // first check, kept
            make_effect_proven_module_get_attr(ValueId(0), ValueId(1), ValueId(2)),
            make_check_exception(), // certified read cannot set the exception flag
        ]);

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 1);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
    }

    #[test]
    fn check_after_unproven_module_get_attr_is_kept() {
        let mut func = make_func_with_block(vec![
            make_check_exception(), // first check, kept
            make_module_get_attr(ValueId(0), ValueId(1), ValueId(2)),
            make_check_exception(), // unproven module reads may raise
        ]);

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 3);
    }

    #[test]
    fn many_redundant_checks_collapsed() {
        let mut func = make_func_with_block(vec![
            make_check_exception(), // first, kept
            make_const_int(1, ValueId(0)),
            make_check_exception(), // redundant
            make_const_int(2, ValueId(1)),
            make_check_exception(), // redundant
            make_const_int(3, ValueId(2)),
            make_check_exception(), // redundant
            make_call("foo", ValueId(3)),
            make_check_exception(), // after call, kept
            make_check_exception(), // redundant after the kept one
        ]);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 4);
        // Original 10 ops, removed 4, leaves 6.
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 6);
    }

    #[test]
    fn first_check_in_normal_successor_dropped_after_checked_predecessor() {
        let mut func = make_two_block_func(
            vec![make_check_exception()],
            vec![make_const_int(2, ValueId(1)), make_check_exception()],
        );
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 1);
        assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
    }

    #[test]
    fn first_check_in_successor_kept_when_predecessor_may_raise() {
        let mut func = make_two_block_func(
            vec![make_call("foo", ValueId(1))],
            vec![make_check_exception()],
        );
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
    }

    #[test]
    fn exception_target_entry_remains_conservative() {
        let mut func = make_two_block_func(
            vec![make_check_exception()],
            vec![make_const_int(2, ValueId(1)), make_check_exception()],
        );
        func.label_id_map.insert(1, 100);
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(1)].ops.len(), 2);
    }

    #[test]
    fn explicit_exception_clear_feeds_successor_elision() {
        let mut func = make_two_block_func(
            vec![
                make_check_exception(),
                make_original_kind("exception_clear"),
            ],
            vec![make_check_exception()],
        );
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        assert_eq!(func.blocks[&BlockId(1)].ops.len(), 0);
    }

    #[test]
    fn check_after_i64_mod_by_nonzero_const_is_dropped() {
        let lhs = ValueId(0);
        let rhs = ValueId(1);
        let out = ValueId(2);
        let mut func = make_func_with_block(vec![
            make_const_int(9, lhs),
            make_const_int(3, rhs),
            make_check_exception(), // first check, kept
            make_binary(OpCode::Mod, lhs, rhs, out),
            make_check_exception(), // redundant: i64 modulo by nonzero const
        ]);
        func.value_types.insert(lhs, TirType::I64);
        func.value_types.insert(rhs, TirType::I64);
        func.value_types.insert(out, TirType::I64);

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 1);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
    }

    #[test]
    fn check_after_i64_mod_by_zero_const_is_kept() {
        let lhs = ValueId(0);
        let rhs = ValueId(1);
        let out = ValueId(2);
        let mut func = make_func_with_block(vec![
            make_const_int(9, lhs),
            make_const_int(0, rhs),
            make_check_exception(), // first check, kept
            make_binary(OpCode::Mod, lhs, rhs, out),
            make_check_exception(), // required: modulo by zero may raise
        ]);
        func.value_types.insert(lhs, TirType::I64);
        func.value_types.insert(rhs, TirType::I64);
        func.value_types.insert(out, TirType::I64);

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
    }

    #[test]
    fn check_after_i64_mod_by_dynamic_rhs_is_kept() {
        let lhs = ValueId(0);
        let rhs = ValueId(1);
        let out = ValueId(2);
        let mut func = make_func_with_block(vec![
            make_const_int(9, lhs),
            make_check_exception(), // first check, kept
            make_binary(OpCode::Mod, lhs, rhs, out),
            make_check_exception(), // required: typed but not proven nonzero
        ]);
        func.value_types.insert(lhs, TirType::I64);
        func.value_types.insert(rhs, TirType::I64);
        func.value_types.insert(out, TirType::I64);

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
    }
}
