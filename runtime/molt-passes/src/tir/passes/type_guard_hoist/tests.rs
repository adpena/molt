use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::run;

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

fn make_type_guard(operand: ValueId, result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("ty".to_string(), AttrValue::Str("INT".to_string()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TypeGuard,
        operands: vec![operand],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

#[test]
fn typeguard_loop_invariant_hoisted() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let x = func.fresh_value();
    let ok = func.fresh_value();

    let loop_header_id = func.fresh_block();
    let loop_body_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
        entry.terminator = Terminator::Branch {
            target: loop_header_id,
            args: vec![],
        };
    }

    func.blocks.insert(
        loop_header_id,
        TirBlock {
            id: loop_header_id,
            args: vec![],
            ops: vec![make_type_guard(x, ok)],
            terminator: Terminator::Branch {
                target: loop_body_id,
                args: vec![],
            },
        },
    );

    func.blocks.insert(
        loop_body_id,
        TirBlock {
            id: loop_body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            },
        },
    );

    let stats = run(&mut func, &mut AnalysisManager::new());

    assert!(
        stats.ops_removed >= 1,
        "expected at least one op removed from loop block"
    );
    assert!(
        stats.ops_added >= 1,
        "expected at least one op added to preheader"
    );

    let entry_ops = &func.blocks[&func.entry_block].ops;
    assert!(
        entry_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
        "TypeGuard should be in preheader (bb0)"
    );
    let header_ops = &func.blocks[&loop_header_id].ops;
    assert!(
        !header_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
        "TypeGuard should NOT remain in loop header (bb1)"
    );
}

#[test]
fn typeguard_hoists_when_latch_id_precedes_header() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let x = func.fresh_value();
    let ok = func.fresh_value();
    let header = BlockId(20);
    let body = BlockId(5);
    func.next_block = 21;

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }

    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![make_type_guard(x, ok)],
            terminator: Terminator::Branch {
                target: body,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    let stats = run(&mut func, &mut AnalysisManager::new());

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(stats.ops_added, 1);
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::TypeGuard)
    );
    assert!(
        !func.blocks[&header]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::TypeGuard)
    );
}

#[test]
fn typeguard_loop_local_not_hoisted() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);

    let y = func.fresh_value();
    let ok = func.fresh_value();

    let loop_header_id = func.fresh_block();
    let loop_body_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: loop_header_id,
            args: vec![],
        };
    }

    func.blocks.insert(
        loop_header_id,
        TirBlock {
            id: loop_header_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_body_id,
                args: vec![],
            },
        },
    );

    func.blocks.insert(
        loop_body_id,
        TirBlock {
            id: loop_body_id,
            args: vec![],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![y]),
                make_type_guard(y, ok),
            ],
            terminator: Terminator::Branch {
                target: loop_header_id,
                args: vec![],
            },
        },
    );

    let stats = run(&mut func, &mut AnalysisManager::new());

    assert_eq!(
        stats.ops_removed, 0,
        "should not hoist TypeGuard on loop-local value"
    );
    let body_ops = &func.blocks[&loop_body_id].ops;
    assert!(
        body_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
        "TypeGuard should remain in loop body"
    );
}

#[test]
fn no_typeguard_no_changes() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let v = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![v]));
    entry.terminator = Terminator::Return { values: vec![v] };

    let stats = run(&mut func, &mut AnalysisManager::new());
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
}

#[test]
fn typeguard_outside_loop_unchanged() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let x = func.fresh_value();
    let ok = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![x]));
    entry.ops.push(make_type_guard(x, ok));
    entry.terminator = Terminator::Return { values: vec![ok] };

    let stats = run(&mut func, &mut AnalysisManager::new());
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::TypeGuard)
    );
}
