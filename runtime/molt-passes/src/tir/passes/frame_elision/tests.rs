//! Frame elision: a body keeps its record exactly while an observer can
//! reach it.

use super::*;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::ops::AttrValue;
use crate::tir::target_info::TargetInfo;

const EXIT_LABEL: i64 = 700;

fn copy_carrier(kind: OpKind, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut op = TirOp::new(OpCode::Copy, operands, results);
    op.origin = Some(kind);
    op
}

fn check() -> TirOp {
    let mut op = TirOp::new(OpCode::CheckException, vec![], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(EXIT_LABEL));
    op.attrs.insert(
        "exception_edge".into(),
        AttrValue::Int(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin as i64),
    );
    op
}

/// `def add(a, b): return a + b` as the lift frames it: the record's entry
/// and its check, a home per parameter, a location, the sum, the pop; and an
/// exception exit that pops. `exact` seeds both parameters as exact ints, as
/// a type-flow clone's are.
pub(crate) fn framed_add(name: &str, exact: bool) -> TirFunction {
    let mut f = TirFunction::new(
        name.into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    f.execution_context = ExecutionContextPolicy::Local;
    f.has_exception_handling = true;
    if exact {
        f.exact_param_types = vec![TirType::I64, TirType::I64];
    }
    let entry = f.entry_block;
    let a = f.blocks[&entry].args[0].id;
    let b = f.blocks[&entry].args[1].id;
    let home_a = f.fresh_value();
    let home_b = f.fresh_value();
    let sum = f.fresh_value();
    let exit = f.fresh_block();
    let mut enter = copy_carrier(OpKind::TraceEnterSlot, vec![], vec![]);
    enter
        .attrs
        .insert("s_value".into(), AttrValue::Str(name.into()));
    enter.attrs.insert("value".into(), AttrValue::Int(2));
    let home = |value, result, slot| {
        let mut op = copy_carrier(OpKind::FrameHomeStore, vec![value], vec![result]);
        op.attrs.insert("value".into(), AttrValue::Int(slot));
        op
    };
    let home_a_op = home(a, home_a, 0);
    let home_b_op = home(b, home_b, 1);
    let mut location = TirOp::new(OpCode::FrameSetLocation, vec![], vec![]);
    location.attrs.insert("value".into(), AttrValue::Int(1));
    let ops = vec![
        enter,
        check(),
        home_a_op,
        home_b_op,
        copy_carrier(OpKind::Line, vec![], vec![]),
        location,
        TirOp::new(OpCode::Add, vec![home_a, home_b], vec![sum]),
        check(),
        copy_carrier(OpKind::TraceExit, vec![], vec![]),
    ];
    let body = f.blocks.get_mut(&entry).unwrap();
    body.ops = ops;
    body.terminator = Terminator::Return { values: vec![sum] };
    f.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![copy_carrier(OpKind::TraceExit, vec![], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f.label_id_map.insert(exit.0, EXIT_LABEL);
    crate::tir::type_refine::refine_types(&mut f);
    f
}

fn has_frame_op(func: &TirFunction) -> bool {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .any(|op| frame_op(op).is_some())
}

fn count(func: &TirFunction, opcode: OpCode) -> usize {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == opcode)
        .count()
}

#[test]
fn a_leaf_over_exact_ints_links_no_record() {
    let mut f = framed_add("add__tf_ii", true);
    assert!(run(&mut f));
    assert_eq!(f.execution_context, ExecutionContextPolicy::None);
    assert!(!has_frame_op(&f), "record, homes and locations are gone");
    assert_eq!(count(&f, OpCode::CheckException), 0);
    assert_eq!(count(&f, OpCode::Add), 1, "the body itself stays");
    let _ = crate::tir::passes::reoptimize(&mut f, &TargetInfo::from_llvm_feature_string(""));
    crate::tir::verify::verify_function(&f).expect("an elided body verifies");
    assert_eq!(
        f.blocks.len(),
        1,
        "the exception exit lost its only edge and is deleted"
    );
}

#[test]
fn a_body_that_may_raise_keeps_its_record() {
    // `a + b` of unproven values may raise TypeError: the traceback reads
    // the record's location.
    let mut f = framed_add("add", false);
    let before = format!("{f:?}");
    assert!(!run(&mut f));
    assert_eq!(format!("{f:?}"), before, "a refused body is untouched");
}

#[test]
fn a_poll_keeps_its_record() {
    // A signal handler the poll runs may call `sys._getframe()`.
    let mut f = framed_add("add__tf_ii", true);
    let entry = f.entry_block;
    let poll = f.blocks.get_mut(&entry).unwrap().ops.get_mut(1).unwrap();
    assert!(poll.mark_async_work_poll());
    assert!(!run(&mut f));
    assert_eq!(f.execution_context, ExecutionContextPolicy::Local);
}

#[test]
fn a_home_of_an_unproven_value_keeps_its_record() {
    // Releasing the home's binding at the frame clear may run a finalizer.
    // `def f(a, b): return a + a` with only `a` proven: `b`'s home holds an
    // object of any class.
    let mut f = framed_add("f__tf_id", true);
    f.exact_param_types[1] = TirType::DynBox;
    let entry = f.entry_block;
    let body = f.blocks.get_mut(&entry).unwrap();
    let home_a = body.ops[2].results[0];
    let sum = body
        .ops
        .iter_mut()
        .find(|op| op.opcode == OpCode::Add)
        .unwrap();
    sum.operands = vec![home_a, home_a];
    assert!(!run(&mut f));
    // Proving `b` too removes the last observer.
    f.exact_param_types[1] = TirType::I64;
    assert!(run(&mut f));
}

#[test]
fn a_body_with_its_own_execution_context_policy_is_left_alone() {
    let mut f = framed_add("chunk", true);
    f.execution_context = ExecutionContextPolicy::Inherited;
    assert!(!run(&mut f));
}
