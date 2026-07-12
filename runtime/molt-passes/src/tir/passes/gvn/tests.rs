use super::run;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

fn make_const_int(value: i64, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(value));
            m
        },
        source_span: None,
    }
}

fn make_binop(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![lhs, rhs],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_const_bool(value: bool, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Bool(value));
            m
        },
        source_span: None,
    }
}

fn make_const_bytes(bytes: &[u8], result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBytes,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("bytes".into(), AttrValue::Bytes(bytes.to_vec()));
            m
        },
        source_span: None,
    }
}

fn make_type_guard(operand: ValueId, expected_type: &str, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TypeGuard,
        operands: vec![operand],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "expected_type".into(),
                AttrValue::Str(expected_type.to_string()),
            );
            m
        },
        source_span: None,
    }
}

#[test]
fn redundant_add_eliminated() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let sum1 = func.fresh_value();
    let sum2 = func.fresh_value(); // same computation as sum1

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_binop(OpCode::Add, p0, p1, sum1));
    entry.ops.push(make_binop(OpCode::Add, p0, p1, sum2));
    entry.terminator = Terminator::Return { values: vec![sum2] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert!(stats.values_changed > 0);

    // sum2's definition should now be a Copy from sum1.
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[1].opcode, OpCode::Copy);
    assert_eq!(ops[1].operands[0], sum1);
}

#[test]
fn duplicate_constants_not_folded_by_gvn() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
    let c1 = func.fresh_value();
    let c2 = func.fresh_value(); // same constant as c1

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_const_int(42, c1));
    entry.ops.push(make_const_int(42, c2));
    entry.terminator = Terminator::Return { values: vec![c2] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Constants are intentionally left as constants. Backends handle
    // safe constant pooling in backend-native form; GVN must not create
    // cross-control-flow Copy dependencies for constants.
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(stats.values_changed, 0);
    assert_eq!(ops[1].opcode, OpCode::ConstInt);
}

#[test]
fn different_constants_not_folded() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
    let c1 = func.fresh_value();
    let c2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_const_int(42, c1));
    entry.ops.push(make_const_int(99, c2));
    entry.terminator = Terminator::Return { values: vec![c2] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    // c2 should NOT be folded — different constant.
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[1].opcode, OpCode::ConstInt);
    let _ = stats;
}

#[test]
fn different_const_bytes_not_folded() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::Bytes);
    let c1 = func.fresh_value();
    let c2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_const_bytes(b"one,two", c1));
    entry.ops.push(make_const_bytes(b"two", c2));
    entry.terminator = Terminator::Return { values: vec![c2] };

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[1].opcode, OpCode::ConstBytes);
    assert_eq!(
        ops[1].attrs.get("bytes"),
        Some(&AttrValue::Bytes(b"two".to_vec()))
    );
}

#[test]
fn side_effecting_ops_preserved() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
    let p0 = ValueId(0);
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    // Two identical Call ops — both must be preserved (side effects).
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![p0],
        results: vec![r1],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![p0],
        results: vec![r2],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![r2] };

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Both calls must remain — not folded.
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[0].opcode, OpCode::Call);
    assert_eq!(ops[1].opcode, OpCode::Call);
}

#[test]
fn duplicate_type_guards_with_same_attr_are_deduped() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::Bool);
    let p0 = ValueId(0);
    let g1 = func.fresh_value();
    let g2 = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_type_guard(p0, "int", g1));
    entry.ops.push(make_type_guard(p0, "int", g2));
    entry.terminator = Terminator::Return { values: vec![g2] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.values_changed, 1);

    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[1].opcode, OpCode::Copy);
    assert_eq!(ops[1].operands[0], g1);
}

#[test]
fn type_guard_value_key_includes_expected_type_attr() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::Bool);
    let p0 = ValueId(0);
    let is_int = func.fresh_value();
    let is_str = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_type_guard(p0, "int", is_int));
    entry.ops.push(make_type_guard(p0, "str", is_str));
    entry.terminator = Terminator::Return {
        values: vec![is_str],
    };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.values_changed, 0);

    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[0].opcode, OpCode::TypeGuard);
    assert_eq!(ops[1].opcode, OpCode::TypeGuard);
    assert_eq!(
        ops[1].attrs.get("expected_type"),
        Some(&AttrValue::Str("str".to_string()))
    );
}

// ── Cross-block dominator-scoped GVN tests ──────────────────────────

/// entry: c1 = ConstInt 42; branch body
/// body:  c2 = ConstInt 42; return c2
/// → constants stay backend-native constants. GVN must not replace c2
///   with Copy(c1), even though entry strictly dominates body.
#[test]
fn cross_block_redundant_constant_not_folded() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
    let body = func.fresh_block();
    let c1 = func.fresh_value();
    let c2 = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(42, c1));
        entry.terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
    }

    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![make_const_int(42, c2)],
            terminator: Terminator::Return { values: vec![c2] },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.values_changed, 0);

    // c2 in `body` remains a backend-native constant.
    let body_ops = &func.blocks[&body].ops;
    assert_eq!(body_ops[0].opcode, OpCode::ConstInt);
    assert_eq!(body_ops[0].results[0], c2);
    let _ = c1;
}

/// entry: s1 = p0 + p1; branch body
/// body:  s2 = p0 + p1; return s2
/// → s2 should become Copy(s1).
#[test]
fn cross_block_redundant_arithmetic() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let body = func.fresh_block();
    let s1 = func.fresh_value();
    let s2 = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_binop(OpCode::Add, p0, p1, s1));
        entry.terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
    }

    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
            terminator: Terminator::Return { values: vec![s2] },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert!(stats.values_changed > 0);

    let body_ops = &func.blocks[&body].ops;
    assert_eq!(body_ops[0].opcode, OpCode::Copy);
    assert_eq!(body_ops[0].operands[0], s1);
    assert_eq!(body_ops[0].results[0], s2);
}

/// Diamond:
///   entry: cond branch → then / else
///   then:  s1 = p0 + p1
///   else:  s2 = p0 + p1     ← NOT dominated by `then`, must NOT dedup
///   merge: return s_phi
/// → s2 must remain a real Add; only entry-defined values may flow into
///   sibling blocks, and `then` does not dominate `else`.
#[test]
fn non_dominating_no_dedup() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::I64, TirType::I64, TirType::Bool],
        TirType::I64,
    );
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let cond = ValueId(2);
    let then_b = func.fresh_block();
    let else_b = func.fresh_block();
    let merge_b = func.fresh_block();

    let s1 = func.fresh_value();
    let s2 = func.fresh_value();
    let merge_arg = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: then_b,
            then_args: vec![],
            else_block: else_b,
            else_args: vec![],
        };
    }

    func.blocks.insert(
        then_b,
        TirBlock {
            id: then_b,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, p0, p1, s1)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![s1],
            },
        },
    );

    func.blocks.insert(
        else_b,
        TirBlock {
            id: else_b,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![s2],
            },
        },
    );

    func.blocks.insert(
        merge_b,
        TirBlock {
            id: merge_b,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Both sibling adds must remain real Add ops. `then` does not
    // dominate `else` (and vice versa), so neither may be replaced
    // with a Copy of the other.
    assert_eq!(
        func.blocks[&then_b].ops[0].opcode,
        OpCode::Add,
        "then-block add must not be deduped"
    );
    assert_eq!(
        func.blocks[&else_b].ops[0].opcode,
        OpCode::Add,
        "else-block add must not be deduped (then does not dominate else)"
    );
}

/// entry  → then → merge
///       → else → merge
/// `entry` defines `e = p0 + p1`.  Both `then` and `else` recompute
/// `p0 + p1`.  Both must dedup against `e` (entry dominates both).
#[test]
fn dominator_value_propagates_to_both_branches() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::I64, TirType::I64, TirType::Bool],
        TirType::I64,
    );
    let p0 = ValueId(0);
    let p1 = ValueId(1);
    let cond = ValueId(2);
    let then_b = func.fresh_block();
    let else_b = func.fresh_block();
    let merge_b = func.fresh_block();
    let e = func.fresh_value();
    let s1 = func.fresh_value();
    let s2 = func.fresh_value();
    let merge_arg = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_binop(OpCode::Add, p0, p1, e));
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: then_b,
            then_args: vec![],
            else_block: else_b,
            else_args: vec![],
        };
    }

    func.blocks.insert(
        then_b,
        TirBlock {
            id: then_b,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, p0, p1, s1)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![s1],
            },
        },
    );

    func.blocks.insert(
        else_b,
        TirBlock {
            id: else_b,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, p0, p1, s2)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![s2],
            },
        },
    );

    func.blocks.insert(
        merge_b,
        TirBlock {
            id: merge_b,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert!(stats.values_changed >= 2);
    assert_eq!(func.blocks[&then_b].ops[0].opcode, OpCode::Copy);
    assert_eq!(func.blocks[&then_b].ops[0].operands[0], e);
    assert_eq!(func.blocks[&else_b].ops[0].opcode, OpCode::Copy);
    assert_eq!(func.blocks[&else_b].ops[0].operands[0], e);
}

/// Cross-block dedup must NOT escape the entry block when the
/// "supposedly redundant" computation lives in a block that
/// post-dominates entry but is itself a loop header — block args
/// (phi values) coming in from the back edge are NOT visible in
/// entry's value table, so dominator-scoped GVN naturally skips them.
/// This regression guards against accidentally numbering loop-carried
/// values as constants from the preheader.
#[test]
fn loop_header_back_edge_not_deduped() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
    let p0 = ValueId(0);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let header_arg = func.fresh_value();
    let bumped = func.fresh_value();
    let one = func.fresh_value();
    let one_in_body = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Branch to header, threading p0 as the loop-carried value.
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![p0],
        };
    }

    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: header_arg,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, one)],
            terminator: Terminator::CondBranch {
                cond: one,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            // body redefines `1` — same constant, but belongs to a different
            // dominator scope from entry's standpoint.  GVN should still
            // dedup against the header's `one` because header dominates body.
            ops: vec![
                make_const_int(1, one_in_body),
                make_binop(OpCode::Add, header_arg, one_in_body, bumped),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![bumped],
            },
        },
    );

    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![header_arg],
            },
        },
    );

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // body constants are not replaced with cross-block copies. The
    // loop-carried `bumped` must remain a real Add because its operand
    // `header_arg` is a phi.
    assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::ConstInt);
    assert_eq!(
        func.blocks[&body].ops[1].opcode,
        OpCode::Add,
        "phi-fed Add must not be folded"
    );
    let _ = one;
}

/// Mirrors the `bench_struct` body-block pattern: the loop-carried
/// induction variable `i: I64` participates in two structurally
/// identical `i + 1` computations in the same block (one for `p.y =
/// i + 1`, one for the `i += 1` increment). GVN must collapse the
/// second into a Copy of the first — within the same block, two
/// typed Adds with identical operands are equivalent regardless of
/// whether the operand is a phi-fed loop-carried value.
///
/// Locks in the contract that drove the dead-store-elim landing:
/// `bench_struct` performance hinges on this dedup firing.
///
/// The header's branch condition is a `ConstBool` instead of a
/// `ConstInt(1)` to keep the dom-tree leader table from
/// inadvertently aliasing the body's `1` literals against the
/// branch cond — the assertion targets `i + 1` dedup, not constant
/// folding across blocks.
#[test]
fn redundant_add_in_loop_body_dedups() {
    let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
    let p0 = ValueId(0);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let i = func.fresh_value();
    let cond = func.fresh_value();
    let one_a = func.fresh_value();
    let one_b = func.fresh_value();
    let plus_a = func.fresh_value();
    let plus_b = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![p0],
        };
    }

    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: i,
                ty: TirType::I64,
            }],
            ops: vec![make_const_bool(true, cond)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    // Body computes `i + 1` twice. A real bench_struct lowering has
    // a fresh ConstInt SSA for each literal `1`. GVN must not replace
    // the second ConstInt with a Copy, but its block-local constant
    // value number lets the second `Add(i, 1)` dedup against the first.
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                make_const_int(1, one_a),
                make_binop(OpCode::Add, i, one_a, plus_a),
                make_const_int(1, one_b),
                make_binop(OpCode::Add, i, one_b, plus_b),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![plus_b],
            },
        },
    );

    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![i] },
        },
    );

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    let body_ops = &func.blocks[&body].ops;
    assert_eq!(
        body_ops[0].opcode,
        OpCode::ConstInt,
        "first `1` literal stays as a const"
    );
    assert_eq!(
        body_ops[1].opcode,
        OpCode::Add,
        "first `i + 1` becomes the leader"
    );
    assert_eq!(
        body_ops[2].opcode,
        OpCode::ConstInt,
        "second ConstInt(1) stays backend-native"
    );
    assert_eq!(
        body_ops[3].opcode,
        OpCode::Copy,
        "second `i + 1` collapses to the first Add"
    );
    assert_eq!(body_ops[3].operands[0], plus_a);
}

/// Sibling blocks that each define the same constant must NOT see each
/// other's leaders.  After the dom-tree walk pops the first sibling, the
/// second sibling enters with a clean (parent-scope) leader table.
#[test]
fn scope_pops_after_sibling() {
    let mut func = TirFunction::new("f".into(), vec![TirType::Bool], TirType::I64);
    let cond = ValueId(0);
    let then_b = func.fresh_block();
    let else_b = func.fresh_block();
    let merge_b = func.fresh_block();
    let c_then = func.fresh_value();
    let c_else = func.fresh_value();
    let merge_arg = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: then_b,
            then_args: vec![],
            else_block: else_b,
            else_args: vec![],
        };
    }

    func.blocks.insert(
        then_b,
        TirBlock {
            id: then_b,
            args: vec![],
            ops: vec![make_const_int(7, c_then)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![c_then],
            },
        },
    );

    func.blocks.insert(
        else_b,
        TirBlock {
            id: else_b,
            args: vec![],
            ops: vec![make_const_int(7, c_else)],
            terminator: Terminator::Branch {
                target: merge_b,
                args: vec![c_else],
            },
        },
    );

    func.blocks.insert(
        merge_b,
        TirBlock {
            id: merge_b,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Neither sibling block dominates the other, so each ConstInt 7
    // must remain a ConstInt (not a Copy of the other).
    assert_eq!(func.blocks[&then_b].ops[0].opcode, OpCode::ConstInt);
    assert_eq!(func.blocks[&else_b].ops[0].opcode, OpCode::ConstInt);
}

/// `make_const_bool` is exercised here to ensure that constants are not
/// replaced across blocks. Bool-vs-int discrimination is still represented
/// by the constant opcode and attributes, not by a cross-block Copy.
#[test]
fn cross_block_const_bool_not_folded() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::Bool);
    let body = func.fresh_block();
    let b1 = func.fresh_value();
    let b2 = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_bool(false, b1));
        entry.terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
    }

    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![make_const_bool(false, b2)],
            terminator: Terminator::Return { values: vec![b2] },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks[&body].ops[0].opcode, OpCode::ConstBool);
    let _ = b1;
}
