use super::run;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_type_guard(operand: ValueId, result: ValueId, ty: &str) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("ty".to_string(), AttrValue::Str(ty.to_string()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TypeGuard,
        operands: vec![operand],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

fn make_const_int(result: ValueId, value: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".to_string(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

// -----------------------------------------------------------------------
// Test 1: Simple TypeGuard elimination via versioning
//
// bb0 (entry): %x = ConstInt(42); branch to bb1
// bb1: %ok = TypeGuard(%x, INT); return %ok
//
// After SBBV: bb0 branches to bb1_spec (specialized), where the
// TypeGuard is replaced with ConstBool(true).
// -----------------------------------------------------------------------
#[test]
fn simple_type_guard_elimination() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let x = func.fresh_value(); // %0
    let ok = func.fresh_value(); // %1

    let bb1 = func.fresh_block(); // BlockId(1)

    // bb0: %x = ConstInt(42); branch to bb1 passing %x as arg
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(x, 42));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![x],
        };
    }

    // bb1(%x_arg): %ok = TypeGuard(%x_arg, INT); return %ok
    let x_arg = func.fresh_value(); // %2
    let block1 = TirBlock {
        id: bb1,
        args: vec![TirValue {
            id: x_arg,
            ty: TirType::DynBox,
        }],
        ops: vec![make_type_guard(x_arg, ok, "INT")],
        terminator: Terminator::Return { values: vec![ok] },
    };
    func.blocks.insert(bb1, block1);

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Should have created a specialized version.
    assert!(
        stats.values_changed >= 1,
        "expected at least one block versioned"
    );
    assert!(
        stats.ops_removed >= 1,
        "expected at least one TypeGuard removed"
    );

    // The entry block should now branch to the specialized block (not bb1).
    let entry_term = &func.blocks[&func.entry_block].terminator;
    match entry_term {
        Terminator::Branch { target, .. } => {
            assert_ne!(
                *target, bb1,
                "entry should branch to specialized block, not original"
            );
            // The specialized block should exist and have a ConstBool instead of TypeGuard.
            let spec_block = &func.blocks[target];
            assert!(
                spec_block
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::ConstBool),
                "specialized block should have ConstBool(true)"
            );
            assert!(
                !spec_block
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::TypeGuard),
                "specialized block should not have TypeGuard"
            );
        }
        other => panic!("expected Branch terminator, got {:?}", other),
    }
}

#[test]
fn operand_dependent_producers_do_not_specialize_type_guard() {
    for (opcode, guard_ty) in [
        (OpCode::Div, "INT"),
        (OpCode::Shl, "INT"),
        (OpCode::And, "BOOL"),
    ] {
        let mut func = TirFunction::new(format!("f_{opcode:?}"), vec![], TirType::None);

        let lhs = func.fresh_value();
        let rhs = func.fresh_value();
        let value = func.fresh_value();
        let arg = func.fresh_value();
        let ok = func.fresh_value();
        let guarded = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(lhs, 4));
            entry.ops.push(make_const_int(rhs, 2));
            entry.ops.push(make_op(opcode, vec![lhs, rhs], vec![value]));
            entry.terminator = Terminator::Branch {
                target: guarded,
                args: vec![value],
            };
        }

        func.blocks.insert(
            guarded,
            TirBlock {
                id: guarded,
                args: vec![TirValue {
                    id: arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![make_type_guard(arg, ok, guard_ty)],
                terminator: Terminator::Return { values: vec![ok] },
            },
        );

        let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

        assert_eq!(
            stats.values_changed, 0,
            "{opcode:?} must not prove {guard_ty} without operand-dependent type refinement"
        );
        assert_eq!(stats.ops_removed, 0);
        match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => assert_eq!(*target, guarded),
            other => panic!("expected Branch terminator, got {:?}", other),
        }
        assert!(
            func.blocks[&guarded]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::TypeGuard),
            "{opcode:?} path should keep the generic TypeGuard"
        );
    }
}

// -----------------------------------------------------------------------
// Test 2: Multiple predecessors with different type contexts
//
// bb0 (entry): branch based on condition
//   then -> bb1: %x = ConstInt(1); branch to bb3
//   else -> bb2: %x = Call(...); branch to bb3
// bb3(%arg): %ok = TypeGuard(%arg, INT); return %ok
//
// After SBBV:
//   bb1 -> bb3_spec (guard removed, ConstBool(true))
//   bb2 -> bb3 (original, guard kept)
// -----------------------------------------------------------------------
#[test]
fn multiple_predecessors_different_contexts() {
    let mut func = TirFunction::new("f".into(), vec![TirType::Bool], TirType::Bool);

    let cond = ValueId(0); // entry param

    let bb1 = func.fresh_block(); // BlockId(1)
    let bb2 = func.fresh_block(); // BlockId(2)
    let bb3 = func.fresh_block(); // BlockId(3)

    // Entry: CondBranch to bb1/bb2
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };
    }

    // bb1: %int_val = ConstInt(1); branch to bb3(%int_val)
    let int_val = func.fresh_value();
    let block1 = TirBlock {
        id: bb1,
        args: vec![],
        ops: vec![make_const_int(int_val, 1)],
        terminator: Terminator::Branch {
            target: bb3,
            args: vec![int_val],
        },
    };
    func.blocks.insert(bb1, block1);

    // bb2: %dyn_val = Call(...); branch to bb3(%dyn_val)
    let dyn_val = func.fresh_value();
    let block2 = TirBlock {
        id: bb2,
        args: vec![],
        ops: vec![make_op(OpCode::Call, vec![], vec![dyn_val])],
        terminator: Terminator::Branch {
            target: bb3,
            args: vec![dyn_val],
        },
    };
    func.blocks.insert(bb2, block2);

    // bb3(%arg): %ok = TypeGuard(%arg, INT); return %ok
    let arg = func.fresh_value();
    let ok = func.fresh_value();
    let block3 = TirBlock {
        id: bb3,
        args: vec![TirValue {
            id: arg,
            ty: TirType::DynBox,
        }],
        ops: vec![make_type_guard(arg, ok, "INT")],
        terminator: Terminator::Return { values: vec![ok] },
    };
    func.blocks.insert(bb3, block3);

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // The pass traces through branch args to their definitions:
    // - bb1 passes int_val (from ConstInt) → proves INT → routed to bb3_spec
    // - bb2 passes dyn_val (from Call) → can't prove → stays on bb3 (generic)
    //
    // This validates the predecessor-tracing behavior: SBBV follows branch
    // args back to their producing ops to determine type proofs, enabling
    // versioning even when the block arg itself is typed DynBox.
    assert!(
        stats.values_changed >= 1,
        "should version bb3 — bb1's ConstInt proves INT via branch arg tracing"
    );

    // bb1 should now branch to the specialized block (not bb3).
    let bb1_term = &func.blocks[&bb1].terminator;
    match bb1_term {
        Terminator::Branch { target, .. } => {
            assert_ne!(*target, bb3, "bb1 should be rewired to specialized block");
        }
        other => panic!("expected Branch, got {:?}", other),
    }

    // bb2 should still branch to the original bb3 (generic).
    let bb2_term = &func.blocks[&bb2].terminator;
    match bb2_term {
        Terminator::Branch { target, .. } => {
            assert_eq!(*target, bb3, "bb2 should stay on generic block");
        }
        other => panic!("expected Branch, got {:?}", other),
    }

    // Original bb3 should still have TypeGuard (for the generic path).
    let bb3_ops = &func.blocks[&bb3].ops;
    assert!(
        bb3_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
        "generic bb3 should still have TypeGuard"
    );
}

// -----------------------------------------------------------------------
// Test 3: Version limit (k=2) enforcement
//
// Even with many predecessors, at most 2 versions are created.
// Since we only create 1 specialized + 1 generic = 2 total, this is
// inherently bounded. Verify that a block with a TypeGuard only gets
// versioned once (not per-predecessor).
// -----------------------------------------------------------------------
#[test]
fn version_limit_enforced() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let bb1 = func.fresh_block(); // BlockId(1)
    let bb2 = func.fresh_block(); // BlockId(2)
    let bb3 = func.fresh_block(); // BlockId(3)
    let merge = func.fresh_block(); // BlockId(4)

    // Entry -> bb1
    let x0 = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(x0, 0));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
    }

    // bb1: %x1 = ConstInt(1); branch to merge
    let x1 = func.fresh_value();
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![make_const_int(x1, 1)],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![],
            },
        },
    );

    // bb2: %x2 = ConstInt(2); branch to merge
    let x2 = func.fresh_value();
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![make_const_int(x2, 2)],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![],
            },
        },
    );

    // bb3: %x3 = ConstInt(3); branch to merge
    let x3 = func.fresh_value();
    func.blocks.insert(
        bb3,
        TirBlock {
            id: bb3,
            args: vec![],
            ops: vec![make_const_int(x3, 3)],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![],
            },
        },
    );

    // merge: %val = ConstInt(99); %ok = TypeGuard(%val, INT); return %ok
    let val = func.fresh_value();
    let ok = func.fresh_value();
    func.blocks.insert(
        merge,
        TirBlock {
            id: merge,
            args: vec![],
            ops: vec![make_const_int(val, 99), make_type_guard(val, ok, "INT")],
            terminator: Terminator::Return { values: vec![ok] },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // At most 1 specialized version should be created (k=2 total including original).
    assert!(
        stats.values_changed <= 1,
        "at most 1 specialized version should be created (k=2 limit)"
    );

    // Count total blocks: original blocks + at most 1 specialized.
    // Original: bb0, bb1, bb2, bb3, merge = 5 blocks.
    // After versioning: 5 + at most 1 = 6.
    assert!(
        func.blocks.len() <= 6,
        "total blocks should be at most 6 (5 original + 1 specialized), got {}",
        func.blocks.len()
    );
}

// -----------------------------------------------------------------------
// Test 4: Loop header not versioned from back-edge
//
// bb0 (entry): branch to bb1
// bb1 (loop header): %ok = TypeGuard(%x, INT); branch to bb2
// bb2 (loop body): back-edge to bb1
//
// bb1 is a loop header (has back-edge from bb2). SBBV must not version it.
// -----------------------------------------------------------------------
#[test]
fn loop_header_not_versioned() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let x = func.fresh_value(); // %0
    let ok = func.fresh_value(); // %1

    let bb1 = func.fresh_block(); // BlockId(1) — loop header
    let bb2 = func.fresh_block(); // BlockId(2) — loop body

    // Entry: define %x, branch to bb1
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(x, 42));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
    }

    // bb1 (loop header): TypeGuard(%x, INT) -> %ok; branch to bb2
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![make_type_guard(x, ok, "INT")],
            terminator: Terminator::Branch {
                target: bb2,
                args: vec![],
            },
        },
    );

    // bb2 (loop body): back-edge to bb1
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: bb1,
                args: vec![],
            },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // bb1 is a loop header — it must NOT be versioned.
    assert_eq!(
        stats.values_changed, 0,
        "loop header should not be versioned"
    );
    assert_eq!(
        stats.ops_removed, 0,
        "no TypeGuard should be removed from loop header"
    );

    // The TypeGuard should still be in bb1.
    let bb1_ops = &func.blocks[&bb1].ops;
    assert!(
        bb1_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
        "TypeGuard should remain in loop header bb1"
    );

    // No new blocks should have been created.
    assert_eq!(
        func.blocks.len(),
        3,
        "no new blocks should be created for loop headers"
    );
}

// -----------------------------------------------------------------------
// Test 5: No TypeGuard ops — pass is a no-op
// -----------------------------------------------------------------------
#[test]
fn no_type_guards_no_changes() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let v = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_const_int(v, 0));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
}

// -----------------------------------------------------------------------
// Test 6: TypeGuard with unparseable type attr — not versioned
// -----------------------------------------------------------------------
#[test]
fn unparseable_guard_type_skipped() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let x = func.fresh_value();
    let ok = func.fresh_value();
    let bb1 = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(x, 1));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
    }

    // TypeGuard with unknown type "CUSTOM_CLASS"
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![make_type_guard(x, ok, "CUSTOM_CLASS")],
            terminator: Terminator::Return { values: vec![ok] },
        },
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    assert_eq!(
        stats.values_changed, 0,
        "unknown type should not be versioned"
    );
}
