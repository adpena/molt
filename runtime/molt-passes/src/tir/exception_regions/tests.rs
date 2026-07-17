//! Tests for the exception-region ownership analysis and its path-state engine.

use std::collections::{BTreeMap, BTreeSet};

use super::path_state::{ExceptionPathState, compute_state_resume_stacks};
use super::*;
use crate::tir::analysis::AnalysisId;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn op(opcode: OpCode) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn original(kind: &str, results: Vec<ValueId>) -> TirOp {
    let mut op = op(OpCode::Copy);
    op.results = results;
    op.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    op
}

fn try_start(label: i64) -> TirOp {
    let mut op = op(OpCode::TryStart);
    op.attrs.insert("value".into(), AttrValue::Int(label));
    op
}

fn try_end(label: i64) -> TirOp {
    let mut op = op(OpCode::TryEnd);
    op.attrs.insert("value".into(), AttrValue::Int(label));
    op
}

fn check_exception(label: i64) -> TirOp {
    let mut op = op(OpCode::CheckException);
    op.attrs.insert("value".into(), AttrValue::Int(label));
    op
}

fn const_int(result: ValueId, value: i64) -> TirOp {
    let mut op = op(OpCode::ConstInt);
    op.results = vec![result];
    op.attrs.insert("value".into(), AttrValue::Int(value));
    op
}

fn state_yield(state: i64) -> TirOp {
    let mut op = op(OpCode::StateYield);
    op.attrs.insert("value".into(), AttrValue::Int(state));
    op
}

fn state_transition(awaitable: ValueId, slot: ValueId, pending_state: ValueId) -> TirOp {
    let mut op = op(OpCode::StateTransition);
    op.operands = vec![awaitable, slot, pending_state];
    op
}

fn split_cleanup_function() -> TirFunction {
    let mut func = TirFunction::new("split_cleanup".into(), vec![], TirType::None);
    let clean = func.fresh_block();
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 4);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: clean,
        args: vec![],
    };
    func.blocks.insert(
        clean,
        TirBlock {
            id: clean,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func
}

fn ambiguous_depth_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("ambiguous_depth".into(), vec![], TirType::None);
    let before_try = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 7);
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: before_try,
        then_args: vec![],
        else_block: handler,
        else_args: vec![],
    };
    func.blocks.insert(
        before_try,
        TirBlock {
            id: before_try,
            args: vec![],
            ops: vec![try_start(7)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original("exception_last_pending", vec![exc]),
                original("exception_pop", vec![]),
            ],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn mixed_exception_exit_observer_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "mixed_exception_exit_observer".into(),
        vec![],
        TirType::None,
    );
    let before_try = func.fresh_block();
    let exit_cleanup = func.fresh_block();
    func.label_id_map.insert(exit_cleanup.0, 3);
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: before_try,
        then_args: vec![],
        else_block: exit_cleanup,
        else_args: vec![],
    };
    func.blocks.insert(
        before_try,
        TirBlock {
            id: before_try,
            args: vec![],
            ops: vec![try_start(3), check_exception(3)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        exit_cleanup,
        TirBlock {
            id: exit_cleanup,
            args: vec![],
            ops: vec![original("exception_last", vec![exc])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn mixed_pending_exit_observer_without_pop_function(
    source_kind: &str,
    name: &str,
) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(name.into(), vec![], TirType::None);
    let before_try = func.fresh_block();
    let exit_cleanup = func.fresh_block();
    func.label_id_map.insert(exit_cleanup.0, 3);
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: before_try,
        then_args: vec![],
        else_block: exit_cleanup,
        else_args: vec![],
    };
    func.blocks.insert(
        before_try,
        TirBlock {
            id: before_try,
            args: vec![],
            ops: vec![try_start(3), check_exception(3)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        exit_cleanup,
        TirBlock {
            id: exit_cleanup,
            args: vec![],
            ops: vec![original(source_kind, vec![exc])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn mixed_exception_pending_exit_observer_without_pop_function() -> (TirFunction, ValueId) {
    mixed_pending_exit_observer_without_pop_function(
        "exception_last_pending",
        "mixed_exception_pending_exit_observer_without_pop",
    )
}

fn mixed_finally_pending_exit_observer_without_pop_function() -> (TirFunction, ValueId) {
    mixed_pending_exit_observer_without_pop_function(
        "exception_finally_pending_observer",
        "mixed_finally_pending_exit_observer_without_pop",
    )
}

fn same_owner_with_different_outer_prefix_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "same_owner_with_different_outer_prefix".into(),
        vec![],
        TirType::None,
    );
    let direct_inner = func.fresh_block();
    let outer_then_inner = func.fresh_block();
    let handler_merge = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler_merge.0, 20);
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond,
        then_block: direct_inner,
        then_args: vec![],
        else_block: outer_then_inner,
        else_args: vec![],
    };
    func.blocks.insert(
        direct_inner,
        TirBlock {
            id: direct_inner,
            args: vec![],
            ops: vec![try_start(20), try_end(20)],
            terminator: Terminator::Branch {
                target: handler_merge,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        outer_then_inner,
        TirBlock {
            id: outer_then_inner,
            args: vec![],
            ops: vec![try_start(10), try_start(20), try_end(20)],
            terminator: Terminator::Branch {
                target: handler_merge,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_merge,
        TirBlock {
            id: handler_merge,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn exception_edge_unwinds_to_target_handler_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "exception_edge_unwinds_to_target_handler".into(),
        vec![],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 10);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops =
        vec![try_start(10), try_start(20), check_exception(10)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn forced_raise_check_exception_handler_branch_function() -> (TirFunction, ValueId, BlockId) {
    let mut func = TirFunction::new(
        "forced_raise_check_exception_handler_branch".into(),
        vec![],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 57);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops =
        vec![try_start(57), op(OpCode::Raise), check_exception(57)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: handler,
        args: vec![],
    };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc, handler_pop)
}

fn explicit_raise_branch_to_labeled_handler_function() -> (TirFunction, ValueId, BlockId) {
    let mut func = TirFunction::new(
        "explicit_raise_branch_to_labeled_handler".into(),
        vec![],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 61);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops =
        vec![try_start(61), check_exception(61), op(OpCode::Raise)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: handler,
        args: vec![],
    };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc, handler_pop)
}

fn inactive_check_exception_target_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "inactive_check_exception_target".into(),
        vec![],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 73);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![check_exception(73)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn body_close_to_normal_exit_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("body_close_to_normal_exit".into(), vec![], TirType::None);
    let normal_exit = func.fresh_block();
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 17);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(17), try_end(17)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: normal_exit,
        args: vec![],
    };
    func.blocks.insert(
        normal_exit,
        TirBlock {
            id: normal_exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn split_exit_pops_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("split_exit_pops".into(), vec![], TirType::None);
    let handler = func.fresh_block();
    let pop_a = func.fresh_block();
    let pop_b = func.fresh_block();
    func.label_id_map.insert(handler.0, 11);
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(11)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: pop_a,
                then_args: vec![],
                else_block: pop_b,
                else_args: vec![],
            },
        },
    );
    for block in [pop_a, pop_b] {
        func.blocks.insert(
            block,
            TirBlock {
                id: block,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    (func, exc)
}

fn finally_cleanup_join_function() -> (TirFunction, ValueId, BlockId) {
    finally_cleanup_join_function_with_source("exception_finally_pending_observer")
}

fn finally_cleanup_join_function_with_source(source_kind: &str) -> (TirFunction, ValueId, BlockId) {
    let mut func = TirFunction::new("finally_cleanup_join".into(), vec![], TirType::None);
    let normal = func.fresh_block();
    let cleanup = func.fresh_block();
    let pop = func.fresh_block();
    func.label_id_map.insert(cleanup.0, 20);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(20)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: normal,
        args: vec![],
    };
    func.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![try_end(20)],
            terminator: Terminator::Branch {
                target: cleanup,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![],
            ops: vec![original(source_kind, vec![exc])],
            terminator: Terminator::Branch {
                target: pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        pop,
        TirBlock {
            id: pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc, pop)
}

fn depth_zero_observer_after_pop_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "depth_zero_observer_after_pop".into(),
        vec![],
        TirType::None,
    );
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 12);
    let exc = func.fresh_value();
    let late_observer = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(12)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original("exception_last_pending", vec![exc]),
                original("exception_pop", vec![]),
                original("exception_last", vec![late_observer]),
            ],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn loop_reentry_after_try_end_and_exception_pop_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "loop_reentry_after_try_end_and_exception_pop".into(),
        vec![],
        TirType::None,
    );
    let loop_block = func.fresh_block();
    let normal = func.fresh_block();
    let cleanup = func.fresh_block();
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 50);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![op(OpCode::TryStart)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: loop_block,
        args: vec![],
    };
    func.blocks.insert(
        loop_block,
        TirBlock {
            id: loop_block,
            args: vec![],
            ops: vec![try_start(50)],
            terminator: Terminator::Branch {
                target: normal,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![try_end(50)],
            terminator: Terminator::Branch {
                target: cleanup,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Branch {
                target: loop_block,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![try_end(50), original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn state_resume_inside_try_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("state_resume_inside_try".into(), vec![], TirType::None);
    let initial = func.fresh_block();
    let resume = func.fresh_block();
    let handler = func.fresh_block();
    let match_block = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 94);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(197, resume, vec![])],
        default: initial,
        default_args: vec![],
    };
    func.blocks.insert(
        initial,
        TirBlock {
            id: initial,
            args: vec![],
            ops: vec![try_start(94), state_yield(197)],
            terminator: Terminator::Unreachable,
        },
    );
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![check_exception(94)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![try_end(94)],
            terminator: Terminator::Branch {
                target: match_block,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        match_block,
        TirBlock {
            id: match_block,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

fn repoll_state_resume_inside_try_function() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "repoll_state_resume_inside_try".into(),
        vec![],
        TirType::None,
    );
    let initial = func.fresh_block();
    let resume = func.fresh_block();
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 46);
    let awaitable = func.fresh_value();
    let slot = func.fresh_value();
    let pending = func.fresh_value();
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(47, resume, vec![])],
        default: initial,
        default_args: vec![],
    };
    func.blocks.insert(
        initial,
        TirBlock {
            id: initial,
            args: vec![],
            ops: vec![
                try_start(46),
                const_int(slot, 64),
                const_int(pending, 47),
                state_transition(awaitable, slot, pending),
            ],
            terminator: Terminator::Branch {
                target: resume,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![check_exception(46)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![try_end(46), original("exception_last", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    (func, exc)
}

#[test]
fn exception_region_pairs_match_ref_with_reachable_handler_pop() {
    let func = split_cleanup_function();
    let exc = ValueId(0);
    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }]
    );
    assert!(
        !facts.release_to_matches.contains_key(&ExceptionOpPosition {
            block: BlockId(1),
            op_index: 0,
        })
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }],
        vec![exc],
    );
    assert_eq!(
        facts.release_to_match_facts[&ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }],
        vec![ExceptionMatchReleaseFact {
            value: exc,
            owner: ExceptionRegionToken::Labeled(4),
            entry_predecessors: vec![BlockId(2)],
        }],
    );
}

#[test]
fn exception_region_reports_match_without_reachable_pop() {
    let mut func = TirFunction::new("missing_pop".into(), vec![], TirType::None);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 9);
    let exc = func.fresh_value();
    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(9)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original("exception_last_pending", vec![exc])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let facts = compute_exception_region_facts(&func);

    assert_eq!(
        facts.diagnostics[0].kind,
        ExceptionRegionDiagnosticKind::MatchWithoutReachablePop,
    );
    assert!(facts.match_refs[&exc].releases.is_empty());
    assert!(verify_exception_regions(&func).is_err());
}

#[test]
fn exception_region_ignores_depth_zero_exception_observer() {
    let mut func = TirFunction::new(
        "depth_zero_exception_observer".into(),
        vec![],
        TirType::None,
    );
    let exc = func.fresh_value();
    func.blocks.get_mut(&func.entry_block).unwrap().ops =
        vec![original("exception_last", vec![exc])];

    let facts = compute_exception_region_facts(&func);

    assert!(facts.match_refs.is_empty());
    assert!(facts.diagnostics.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_ignores_owned_exception_last_exit_observer_without_pop() {
    let mut func = TirFunction::new(
        "owned_exception_last_exit_observer".into(),
        vec![],
        TirType::None,
    );
    let cleanup = func.fresh_block();
    func.label_id_map.insert(cleanup.0, 3);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(3)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![],
            ops: vec![original("exception_last", vec![exc])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(!facts.match_refs.contains_key(&exc));
    assert!(facts.release_to_matches.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_reports_ambiguous_producer_depth_without_selecting_release() {
    let (func, exc) = ambiguous_depth_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.match_refs[&exc].releases.is_empty());
    assert!(facts.release_to_matches.is_empty());
    assert_eq!(
        facts
            .diagnostics
            .iter()
            .filter(|diag| diag.kind == ExceptionRegionDiagnosticKind::AmbiguousProducerDepth)
            .count(),
        1,
    );
    assert!(
        facts
            .diagnostics
            .iter()
            .all(|diag| diag.kind != ExceptionRegionDiagnosticKind::MatchWithoutReachablePop),
        "{:?}",
        facts.diagnostics
    );
    assert!(verify_exception_regions(&func).is_err());
}

#[test]
fn exception_region_ignores_mixed_exception_last_exit_observer() {
    let (func, exc) = mixed_exception_exit_observer_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(!facts.match_refs.contains_key(&exc));
    assert!(facts.release_to_matches.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_reports_overloaded_pending_observer_without_owner_pop() {
    let (func, exc) = mixed_exception_pending_exit_observer_without_pop_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.match_refs[&exc].releases.is_empty());
    assert!(facts.release_to_matches.is_empty());
    assert_eq!(
        facts
            .diagnostics
            .iter()
            .filter(|diag| diag.kind == ExceptionRegionDiagnosticKind::AmbiguousProducerDepth)
            .count(),
        1,
    );
    assert!(verify_exception_regions(&func).is_err());
}

#[test]
fn exception_region_ignores_mixed_finally_pending_exit_observer_without_owner_pop() {
    let (func, exc) = mixed_finally_pending_exit_observer_without_pop_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(!facts.match_refs.contains_key(&exc));
    assert!(facts.release_to_matches.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_uses_top_region_owner_not_outer_stack_depth() {
    let (func, exc) = same_owner_with_different_outer_prefix_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(4),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(4),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_exception_edge_unwinds_to_target_handler_owner() {
    let (func, exc) = exception_edge_unwinds_to_target_handler_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(2),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(2),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_forced_raise_check_exception_has_no_fallthrough_owner() {
    let (func, exc, pop) = forced_raise_check_exception_handler_branch_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: pop,
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_match_facts[&ExceptionOpPosition {
            block: pop,
            op_index: 0,
        }],
        vec![ExceptionMatchReleaseFact {
            value: exc,
            owner: ExceptionRegionToken::Labeled(57),
            entry_predecessors: vec![BlockId(1)],
        }],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_pending_raise_branch_enters_labeled_handler_owner() {
    let (func, exc, pop) = explicit_raise_branch_to_labeled_handler_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: pop,
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_match_facts[&ExceptionOpPosition {
            block: pop,
            op_index: 0,
        }],
        vec![ExceptionMatchReleaseFact {
            value: exc,
            owner: ExceptionRegionToken::Labeled(61),
            entry_predecessors: vec![BlockId(1)],
        }],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_inactive_check_exception_target_does_not_fabricate_owner() {
    let (func, exc) = inactive_check_exception_target_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(!facts.match_refs.contains_key(&exc));
    assert!(facts.release_to_matches.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_try_end_does_not_reenter_handler_at_depth_zero() {
    let (func, exc) = body_close_to_normal_exit_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_allows_path_alternative_exit_pops() {
    let (func, exc) = split_exit_pops_function();

    let facts = compute_exception_region_facts(&func);

    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![
            ExceptionOpPosition {
                block: BlockId(2),
                op_index: 0,
            },
            ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            },
        ]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(2),
            op_index: 0,
        }],
        vec![exc],
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(3),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_ignores_finally_pending_observer_cleanup_join() {
    let (func, exc, _pop) = finally_cleanup_join_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(!facts.match_refs.contains_key(&exc));
    assert!(facts.release_to_matches.is_empty());
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_allows_exception_last_finally_cleanup_join() {
    let (func, exc, pop) = finally_cleanup_join_function_with_source("exception_last");

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: pop,
            op_index: 0,
        }]
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_allows_depth_zero_observer_after_handler_pop() {
    let (func, exc) = depth_zero_observer_after_pop_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(1),
            op_index: 1,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(1),
            op_index: 1,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_loop_reentry_keeps_try_end_and_pop_as_single_close() {
    let (func, exc) = loop_reentry_after_try_end_and_exception_pop_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(5),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(5),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_state_resume_preserves_suspended_try_depth() {
    let (func, exc) = state_resume_inside_try_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(5),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(5),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_repoll_state_resume_uses_pending_state_depth() {
    let (func, exc) = repoll_state_resume_inside_try_function();

    let facts = compute_exception_region_facts(&func);

    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert_eq!(
        facts.match_refs[&exc].releases,
        vec![ExceptionOpPosition {
            block: BlockId(4),
            op_index: 0,
        }]
    );
    assert_eq!(
        facts.release_to_matches[&ExceptionOpPosition {
            block: BlockId(4),
            op_index: 0,
        }],
        vec![exc],
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_region_state_resume_stacks_are_bounded_by_lexical_try_token() {
    let mut func = TirFunction::new("state_resume_stack_cycle".into(), vec![], TirType::None);
    let initial = func.fresh_block();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(7, initial, vec![])],
        default: initial,
        default_args: vec![],
    };
    func.blocks.insert(
        initial,
        TirBlock {
            id: initial,
            args: vec![],
            ops: vec![try_start(99), state_yield(7)],
            terminator: Terminator::Branch {
                target: func.entry_block,
                args: vec![],
            },
        },
    );

    let label_to_block: BTreeMap<_, _> = dominators::exception_label_to_block(&func)
        .into_iter()
        .collect();
    let stacks = compute_state_resume_stacks(&func, &label_to_block);

    assert_eq!(
        stacks.get(&7).cloned().unwrap_or_default(),
        BTreeSet::from([ExceptionPathState {
            frames: vec![ExceptionRegionToken::Labeled(99)],
            owners: Vec::new(),
            normal_closures: Vec::new(),
            pending_must_transfer: false,
        }]),
        "state-dispatch cycles must not manufacture duplicate lexical exception frames"
    );
    assert!(verify_exception_regions(&func).is_ok());
}

#[test]
fn exception_regions_analysis_manager_caches_and_invalidates() {
    let func = split_cleanup_function();
    let mut am = crate::tir::analysis::AnalysisManager::new();

    assert!(!am.is_cached(AnalysisId::ExceptionRegions));
    assert_eq!(am.get::<ExceptionRegions>(&func).match_refs.len(), 1,);
    assert!(am.is_cached(AnalysisId::ExceptionRegions));
    am.invalidate_ops();
    assert!(!am.is_cached(AnalysisId::ExceptionRegions));
    assert_eq!(
        am.get::<ExceptionRegions>(&func).release_to_matches.len(),
        1,
    );
    am.invalidate_cfg();
    assert!(!am.is_cached(AnalysisId::ExceptionRegions));
}

#[test]
fn lexical_handler_query_is_total_and_fail_closed() {
    let position = ExceptionOpPosition {
        block: BlockId(7),
        op_index: 3,
    };
    let mut facts = ExceptionRegionFacts::default();
    assert_eq!(
        facts.lexical_handler_before(position),
        Ok(ExceptionBoundaryHandler::Unreachable)
    );

    facts
        .lexical_handlers_before
        .insert(position, BTreeSet::from([None]));
    assert_eq!(
        facts.lexical_handler_before(position),
        Ok(ExceptionBoundaryHandler::DepthZero)
    );

    facts.lexical_handlers_before.insert(
        position,
        BTreeSet::from([Some(ExceptionRegionToken::Labeled(41))]),
    );
    assert_eq!(
        facts.lexical_handler_before(position),
        Ok(ExceptionBoundaryHandler::Labeled(41))
    );

    let owner = ExceptionOpPosition {
        block: BlockId(2),
        op_index: 1,
    };
    facts.lexical_handlers_before.insert(
        position,
        BTreeSet::from([Some(ExceptionRegionToken::Anonymous(owner))]),
    );
    assert_eq!(
        facts.lexical_handler_before(position),
        Err(ExceptionBoundaryHandlerError::Anonymous { position, owner })
    );

    let states = BTreeSet::from([None, Some(ExceptionRegionToken::Labeled(41))]);
    facts
        .lexical_handlers_before
        .insert(position, states.clone());
    assert_eq!(
        facts.lexical_handler_before(position),
        Err(ExceptionBoundaryHandlerError::Ambiguous { position, states })
    );
}
