use super::*;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::verify::verify_function;

fn function() -> TirFunction {
    TirFunction::new(
        "ssa_update_fixture".into(),
        vec![TirType::Bool],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    )
}

fn int_value(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::I64);
    value
}

fn const_int(value: i64, result: ValueId) -> TirOp {
    let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    op
}

fn check_exception(label: i64) -> TirOp {
    let mut attrs = AttrDict::default();
    attrs.insert("value".into(), AttrValue::Int(label));
    let mut op = TirOp::new(OpCode::CheckException, vec![], vec![]);
    op.attrs = attrs;
    op
}

/// entry(cond) -> then: v1 = 1 | else: v2 = 2 -> join: return v1.
/// `v1` and `v2` define one variable, as a cloned definition does.
struct Diamond {
    func: TirFunction,
    then_block: BlockId,
    else_block: BlockId,
    join: BlockId,
    v1: ValueId,
    v2: ValueId,
}

fn diamond(else_defines: bool) -> Diamond {
    let mut func = function();
    let cond = func.blocks[&func.entry_block].args[0].id;
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let join = func.fresh_block();
    let v1 = int_value(&mut func);
    let v2 = int_value(&mut func);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block,
        then_args: vec![],
        else_block,
        else_args: vec![],
    };
    func.blocks.insert(
        then_block,
        TirBlock {
            id: then_block,
            args: vec![],
            ops: vec![const_int(1, v1)],
            terminator: Terminator::Branch {
                target: join,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![],
            ops: if else_defines {
                vec![const_int(2, v2)]
            } else {
                vec![]
            },
            terminator: Terminator::Branch {
                target: join,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![v1] },
        },
    );
    Diamond {
        func,
        then_block,
        else_block,
        join,
        v1,
        v2,
    }
}

fn edge_args(func: &TirFunction, from: BlockId) -> Vec<ValueId> {
    match &func.blocks[&from].terminator {
        Terminator::Branch { args, .. } => args.clone(),
        other => panic!("expected a branch, got {other:?}"),
    }
}

#[test]
fn two_definitions_merge_at_their_join() {
    let Diamond {
        mut func,
        then_block,
        else_block,
        join,
        v1,
        v2,
    } = diamond(true);
    let stats = repair(
        &mut func,
        &[Variable {
            names: vec![v1, v2],
            ty: TirType::I64,
        }],
    )
    .expect("repair succeeds");
    assert_eq!(stats.joins, 1);
    let merged = func.blocks[&join].args[0].id;
    assert_eq!(func.value_types.get(&merged), Some(&TirType::I64));
    assert_eq!(edge_args(&func, then_block), vec![v1]);
    assert_eq!(edge_args(&func, else_block), vec![v2]);
    match &func.blocks[&join].terminator {
        Terminator::Return { values } => assert_eq!(values, &vec![merged]),
        other => panic!("expected a return, got {other:?}"),
    }
    verify_function(&func).expect("repaired function verifies");
}

#[test]
fn an_unreachable_predecessor_of_a_join_keeps_its_arity() {
    // The lift keeps a vestigial loop-else block that no path reaches but
    // that still branches into the loop header.
    let Diamond {
        mut func,
        join,
        v1,
        v2,
        ..
    } = diamond(true);
    let vestige = func.fresh_block();
    func.blocks.insert(
        vestige,
        TirBlock {
            id: vestige,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join,
                args: vec![],
            },
        },
    );
    repair(
        &mut func,
        &[Variable {
            names: vec![v1, v2],
            ty: TirType::I64,
        }],
    )
    .expect("repair succeeds");
    assert_eq!(func.blocks[&join].args.len(), 1);
    let passed = edge_args(&func, vestige);
    assert_eq!(passed.len(), 1);
    assert_eq!(func.value_types.get(&passed[0]), Some(&TirType::None));
    verify_function(&func).expect("repaired function verifies");
}

#[test]
fn a_dominated_use_keeps_its_definition() {
    let Diamond {
        mut func,
        then_block,
        join,
        v1,
        v2,
        ..
    } = diamond(true);
    // The return moves into the then-block, which v1's definition dominates.
    func.blocks.get_mut(&then_block).unwrap().terminator = Terminator::Return { values: vec![v1] };
    func.blocks.get_mut(&join).unwrap().terminator = Terminator::Return { values: vec![] };
    let stats = repair(
        &mut func,
        &[Variable {
            names: vec![v1, v2],
            ty: TirType::I64,
        }],
    )
    .expect("repair succeeds");
    assert_eq!(stats, RepairStats::default());
}

#[test]
fn a_path_without_a_definition_is_refused() {
    let Diamond {
        mut func, v1, ..
    } = diamond(false);
    let refusal = repair(
        &mut func,
        &[Variable {
            names: vec![v1],
            ty: TirType::I64,
        }],
    )
    .expect_err("the else path defines nothing");
    assert_eq!(refusal, RepairRefusal::UndefinedUse);
}

#[test]
fn a_join_entered_by_an_exception_edge_is_refused() {
    let Diamond {
        mut func,
        then_block,
        else_block,
        join,
        v1,
        v2,
    } = diamond(true);
    // Both arms raise into a handler that reads the variable: the handler
    // would need a block argument that exception edges cannot carry.
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 7);
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![v1] },
        },
    );
    for block in [then_block, else_block] {
        func.blocks.get_mut(&block).unwrap().ops.push(check_exception(7));
    }
    func.blocks.get_mut(&join).unwrap().terminator = Terminator::Return { values: vec![] };
    let refusal = repair(
        &mut func,
        &[Variable {
            names: vec![v1, v2],
            ty: TirType::I64,
        }],
    )
    .expect_err("the handler cannot take a new argument");
    assert_eq!(refusal, RepairRefusal::JoinAtExceptionTarget);
}

#[test]
fn a_name_in_two_variables_is_refused() {
    let Diamond {
        mut func, v1, v2, ..
    } = diamond(true);
    let refusal = repair(
        &mut func,
        &[
            Variable {
                names: vec![v1, v2],
                ty: TirType::I64,
            },
            Variable {
                names: vec![v1],
                ty: TirType::I64,
            },
        ],
    )
    .expect_err("v1 is named twice");
    assert_eq!(refusal, RepairRefusal::MalformedName);
}
