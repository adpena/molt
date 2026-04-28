//! Redundant `CheckException` elimination pass.
//!
//! The frontend liberally emits `CHECK_EXCEPTION` after every statement
//! within a try block (and within functions that have a function-level
//! exception label).  Many of these checks are redundant because the
//! intervening ops cannot raise — pure arithmetic, constants, variable
//! load/store, comparisons on known types, etc.
//!
//! This pass walks each block linearly and removes any `CheckException`
//! op that follows only non-raising ops since the previous check (or
//! since block entry).  At block boundaries the analysis is
//! conservative: the first `CheckException` in each block is always
//! kept, since the predecessor block may have left an exception
//! pending.
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
//! intervening ops are all in the "cannot raise" set.  We delegate to
//! the same `is_potentially_throwing` predicate that DCE uses for
//! preserving potentially-raising ops inside try regions, ensuring the
//! two passes share a single source of truth for raising semantics.

use super::PassStats;
use super::dce::is_potentially_throwing;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};

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
            | "exception_pop"
            | "exception_push"
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
/// Wraps `dce::is_potentially_throwing` with an `_original_kind`
/// classifier for unmapped SimpleIR ops.  A `Copy` op carrying
/// `_original_kind` represents an op the SSA lift did not have a
/// dedicated `OpCode` for; whether it can raise depends on the
/// original SimpleIR kind, not on `OpCode::Copy` itself.  Without this
/// classifier the predicate would either over-approximate (treating
/// every unmapped op as raising and producing zero `check_exception`
/// elision in fast-int loops) or under-approximate (treating them all
/// as non-raising and dropping the safety guards that protect
/// `exception_new` / `exception_class` etc.).
fn op_may_raise(op: &TirOp) -> bool {
    if is_potentially_throwing(op.opcode) {
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

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "check_exception_elim",
        ..Default::default()
    };

    for block in func.blocks.values_mut() {
        // `pending_exception_possible` is true at block entry (the
        // predecessor may have left one) and after any
        // potentially-throwing op.  When false, a `CheckException`
        // can be elided — there is provably no exception state to
        // observe since the last clear.
        let mut pending_exception_possible = true;
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
                    if op_may_raise(&op) {
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

    #[test]
    fn first_check_kept() {
        let mut func = make_func_with_block(vec![
            make_const_int(1, ValueId(0)),
            make_check_exception(),
        ]);
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
}
