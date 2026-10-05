//! One availability authority for every release DropInsertion places: a root is
//! named only where every normal and exceptional entry defines it, and a
//! conditional iterator result only where its not-done edge initialized it.

use std::collections::HashMap;

use molt_passes::tir::dominators::{CfgEdgePolicy, ProgramPointDominance, reachable_blocks_with};

use super::*;

fn check(label: i64) -> TirOp {
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check
}

fn function(name: &str) -> TirFunction {
    TirFunction::new(
        name.into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    )
}

fn returning_block(id: BlockId, ops: Vec<TirOp>) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops,
        terminator: Terminator::Return { values: vec![] },
    }
}

fn branching_block(id: BlockId, ops: Vec<TirOp>, target: BlockId) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops,
        terminator: Terminator::Branch {
            target,
            args: vec![],
        },
    }
}

/// Every RC operation names a value defined on every path to it, including a
/// path that enters its block through an exception observation.
pub(super) fn assert_rc_operands_available(func: &TirFunction) {
    let dominance = ProgramPointDominance::compute(func);
    let mut definitions = HashMap::new();
    for (&block_id, block) in &func.blocks {
        for arg in &block.args {
            definitions.insert(arg.id, (block_id, None));
        }
        for (index, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                definitions.insert(result, (block_id, Some(index)));
            }
        }
    }
    let reachable = reachable_blocks_with(func, CfgEdgePolicy::Full);
    for (&block_id, block) in &func.blocks {
        if !reachable.contains(&block_id) {
            continue;
        }
        for (index, op) in block.ops.iter().enumerate() {
            if !matches!(op.opcode, OpCode::DecRef | OpCode::IncRef) {
                continue;
            }
            for &operand in &op.operands {
                let &(def_block, def_op) = definitions
                    .get(&operand)
                    .unwrap_or_else(|| panic!("{operand:?} has no definition"));
                assert!(
                    dominance.definition_available(def_block, def_op, block_id, index),
                    "{:?} in {block_id:?} at {index} names {operand:?}, which an entry does not define",
                    op.opcode
                );
            }
        }
    }
}

/// Releases executed from `block`, entered with `incoming` bound to its
/// arguments, along unconditional branches to a Return. Landing parameters are
/// resolved to the values the path carried in.
fn releases_along(
    func: &TirFunction,
    mut block: BlockId,
    mut incoming: Vec<ValueId>,
) -> Vec<ValueId> {
    let mut released = Vec::new();
    for _ in 0..=func.blocks.len() {
        let body = &func.blocks[&block];
        let bound: HashMap<ValueId, ValueId> = body
            .args
            .iter()
            .map(|arg| arg.id)
            .zip(incoming.iter().copied())
            .collect();
        let resolve = |value: ValueId| bound.get(&value).copied().unwrap_or(value);
        for op in &body.ops {
            if op.opcode == OpCode::DecRef {
                released.extend(op.operands.iter().copied().map(resolve));
            }
        }
        match &body.terminator {
            Terminator::Branch { target, args } => {
                incoming = args.iter().copied().map(resolve).collect();
                block = *target;
            }
            Terminator::Return { .. } => return released,
            other => panic!("the followed path must be unconditional, found {other:?}"),
        }
    }
    panic!("the followed path must reach a Return")
}

/// Releases on the exceptional path of `observation`: its landing, if any, and
/// the handler path, with payload arguments resolved.
fn exceptional_releases(func: &TirFunction, observation: &TirOp) -> Vec<ValueId> {
    let labels: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&block, &label)| (label, BlockId(block)))
        .collect();
    let AttrValue::Int(label) = observation.attrs["value"] else {
        panic!("observation without an exceptional target")
    };
    releases_along(func, labels[&label], observation.operands.clone())
}

fn observations(func: &TirFunction, block: BlockId) -> Vec<TirOp> {
    func.blocks[&block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::CheckException)
        .cloned()
        .collect()
}

fn count(released: &[ValueId], value: ValueId) -> usize {
    released
        .iter()
        .filter(|&&released| released == value)
        .count()
}

fn decrefs_in(func: &TirFunction, block: BlockId, value: ValueId) -> usize {
    func.blocks[&block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef && op.operands.contains(&value))
        .count()
}

fn decrefs_of(func: &TirFunction, value: ValueId) -> usize {
    func.blocks
        .keys()
        .map(|&block| decrefs_in(func, block, value))
        .sum()
}

/// `raise` jumps to the exit that every earlier observation also targets. The
/// local's frame boundary is that exit, but the first observation precedes the
/// local. The release belongs on the normal arc that carries the local, and on
/// the landings of observations that follow it.
#[test]
fn mixed_exit_releases_local_only_where_its_entry_defines_it() {
    let mut func = function("mixed_exit_local");
    let local = func.fresh_value();
    let flag = func.fresh_value();
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let flag_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let exception = func.fresh_value();
    for value in [local, exception] {
        func.value_types.insert(value, TirType::DynBox);
    }
    func.value_types.insert(flag, TirType::Bool);
    let raising = func.fresh_block();
    let returning = func.fresh_block();
    let exit = func.fresh_block();
    func.label_id_map.insert(exit.0, 2);
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops = vec![
            check(2),
            finalizer_object(local),
            original_copy_with_operands("store_var", vec![local], vec![]),
            check(2),
            op(OpCode::Copy, vec![flag_input], vec![flag]),
        ];
        block.terminator = Terminator::CondBranch {
            cond: flag,
            then_block: raising,
            then_args: vec![],
            else_block: returning,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        raising,
        branching_block(
            raising,
            vec![
                const_str(exception),
                op(OpCode::Raise, vec![exception], vec![]),
            ],
            exit,
        ),
    );
    func.blocks
        .insert(returning, returning_block(returning, vec![marker()]));
    func.blocks.insert(exit, returning_block(exit, vec![]));

    run(&mut func, &mut AnalysisManager::new());

    assert_rc_operands_available(&func);
    let checks = observations(&func, entry);
    assert_eq!(checks.len(), 2);
    assert_eq!(
        count(&exceptional_releases(&func, &checks[0]), local),
        0,
        "the first observation precedes the local"
    );
    assert_eq!(
        count(&exceptional_releases(&func, &checks[1]), local),
        1,
        "a later observation abandons the local exactly once"
    );
    assert_eq!(
        count(&releases_along(&func, raising, vec![]), local),
        1,
        "the raise path releases the local exactly once"
    );
    assert_eq!(
        count(&releases_along(&func, returning, vec![]), local),
        1,
        "the normal return releases the local exactly once"
    );
}

/// The same shape through a handler that re-raises. The local has an explicit
/// scope-exit release on the normal return. The handler is entered by the
/// raise and by observations on both sides of the local, so the release that
/// covers the raise path belongs before the handler, never inside it.
#[test]
fn mixed_handler_releases_explicit_local_only_where_its_entry_defines_it() {
    let mut func = function("mixed_handler_local");
    let local = func.fresh_value();
    let flag = func.fresh_value();
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let flag_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    let exception = func.fresh_value();
    for value in [local, exception] {
        func.value_types.insert(value, TirType::DynBox);
    }
    func.value_types.insert(flag, TirType::Bool);
    let raising = func.fresh_block();
    let returning = func.fresh_block();
    let handler = func.fresh_block();
    let exit = func.fresh_block();
    func.label_id_map.insert(handler.0, 90);
    func.label_id_map.insert(exit.0, 2);
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops = vec![
            check(90),
            finalizer_object(local),
            original_copy_with_operands("store_var", vec![local], vec![]),
            check(90),
            op(OpCode::Copy, vec![flag_input], vec![flag]),
        ];
        block.terminator = Terminator::CondBranch {
            cond: flag,
            then_block: raising,
            then_args: vec![],
            else_block: returning,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        raising,
        branching_block(
            raising,
            vec![
                const_str(exception),
                op(OpCode::Raise, vec![exception], vec![]),
            ],
            handler,
        ),
    );
    func.blocks.insert(
        returning,
        returning_block(
            returning,
            vec![marker(), op(OpCode::DelBoundary, vec![local], vec![])],
        ),
    );
    func.blocks
        .insert(handler, branching_block(handler, vec![marker()], exit));
    func.blocks.insert(exit, returning_block(exit, vec![]));

    run(&mut func, &mut AnalysisManager::new());

    assert_rc_operands_available(&func);
    let checks = observations(&func, entry);
    assert_eq!(checks.len(), 2);
    assert_eq!(
        count(&exceptional_releases(&func, &checks[0]), local),
        0,
        "the handler path of the first observation never had the local"
    );
    assert_eq!(
        count(&exceptional_releases(&func, &checks[1]), local),
        1,
        "a later observation abandons the local exactly once"
    );
    assert_eq!(
        count(&releases_along(&func, raising, vec![]), local),
        1,
        "the raise path releases the local exactly once"
    );
    assert_eq!(
        count(&releases_along(&func, returning, vec![]), local),
        1,
        "the explicit scope-exit boundary stays the normal release"
    );
}

/// A `for` loop whose body contains a fallible call before its last use of the
/// loop value.
fn loop_with_body(
    func: &mut TirFunction,
    body_ops: impl FnOnce(ValueId, ValueId) -> Vec<TirOp>,
) -> (BlockId, BlockId, BlockId, ValueId, ValueId) {
    let iterator = func.fresh_value();
    let value = func.fresh_value();
    let done = func.fresh_value();
    let binding = func.fresh_value();
    for id in [iterator, value, binding] {
        func.value_types.insert(id, TirType::DynBox);
    }
    func.value_types.insert(done, TirType::Bool);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let finished = func.fresh_block();
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops = vec![const_str(iterator)];
        block.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![op(
                OpCode::IterNextUnboxed,
                vec![iterator],
                vec![value, done],
            )],
            terminator: Terminator::CondBranch {
                cond: done,
                then_block: finished,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    let mut ops = vec![original_copy_with_operands(
        "store_var",
        vec![value],
        vec![binding],
    )];
    ops.extend(body_ops(binding, iterator));
    func.blocks.insert(body, branching_block(body, ops, header));
    func.blocks
        .insert(finished, returning_block(finished, vec![]));
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    (header, body, finished, iterator, value)
}

/// An exception raised in the body abandons the initialized loop value like any
/// other owner. The exhaustion edge must still never release it.
#[test]
fn loop_body_exception_releases_the_initialized_iterator_value() {
    let mut func = function("loop_body_exception");
    let exit = func.fresh_block();
    func.label_id_map.insert(exit.0, 2);
    func.blocks.insert(exit, returning_block(exit, vec![]));
    let (header, body, _, iterator, value) = loop_with_body(&mut func, |binding, _| {
        vec![
            named_call("fixture_external_call", vec![], vec![]),
            check(2),
            named_call("fixture_external_call", vec![binding], vec![]),
        ]
    });

    run(&mut func, &mut AnalysisManager::new());

    assert_rc_operands_available(&func);
    let body_checks = observations(&func, body);
    assert_eq!(body_checks.len(), 1);
    let abandoned = exceptional_releases(&func, &body_checks[0]);
    assert_eq!(
        count(&abandoned, value),
        1,
        "the body's exception path releases the initialized value once: {abandoned:?}"
    );
    assert_eq!(
        count(&abandoned, iterator),
        1,
        "and the iterator the next iteration would have used: {abandoned:?}"
    );
    let Terminator::CondBranch { then_block, .. } = &func.blocks[&header].terminator else {
        panic!("the header keeps its exhaustion test")
    };
    let exhausted = releases_along(&func, *then_block, vec![]);
    assert_eq!(
        count(&exhausted, value),
        0,
        "the exhaustion edge never releases the uninitialized value: {exhausted:?}"
    );
    assert_eq!(decrefs_in(&func, header, value), 0);
    assert_eq!(
        decrefs_in(&func, body, value),
        1,
        "the normal body path keeps its single last-use release"
    );
}

/// The loop value dies on the body's untaken branch. Validity is a point fact,
/// so the release belongs at that branch, which is still below the not-done
/// edge.
#[test]
fn loop_body_branch_releases_the_iterator_value_where_it_dies() {
    let mut func = function("loop_body_branch");
    let flag = func.fresh_value();
    // Keep both CFG paths executable; this fixture condition is not a literal.
    let flag_input = crate::fixture_support::append_parameter(&mut func, TirType::Bool);
    func.value_types.insert(flag, TirType::Bool);
    let used = func.fresh_block();
    let skipped = func.fresh_block();
    let latch = func.fresh_block();
    let (header, body, finished, _, value) = loop_with_body(&mut func, |_, _| {
        vec![op(OpCode::Copy, vec![flag_input], vec![flag])]
    });
    let binding = match func.blocks[&body].ops[0].results.as_slice() {
        [binding] => *binding,
        other => panic!("store_var binds one value, found {other:?}"),
    };
    func.blocks.get_mut(&body).unwrap().terminator = Terminator::CondBranch {
        cond: flag,
        then_block: used,
        then_args: vec![],
        else_block: skipped,
        else_args: vec![],
    };
    func.blocks.insert(
        used,
        branching_block(
            used,
            vec![named_call("fixture_external_call", vec![binding], vec![])],
            latch,
        ),
    );
    func.blocks
        .insert(skipped, branching_block(skipped, vec![], latch));
    func.blocks
        .insert(latch, branching_block(latch, vec![], header));

    run(&mut func, &mut AnalysisManager::new());

    assert_rc_operands_available(&func);
    assert_eq!(
        decrefs_in(&func, used, value),
        1,
        "the using branch releases after its last use"
    );
    assert_eq!(
        decrefs_in(&func, skipped, value),
        1,
        "the untaken branch releases the value it never used"
    );
    assert_eq!(
        decrefs_of(&func, value),
        2,
        "no other point releases the value; in particular not {header:?} or {finished:?}"
    );
}

/// A landing label must be fresh in the function's whole exception-label
/// namespace. That includes a region marker whose handler block no longer
/// exists.
#[test]
fn landing_label_is_fresh_across_region_labels() {
    let mut func = function("landing_label_domain");
    let owner = func.fresh_value();
    func.value_types.insert(owner, TirType::DynBox);
    let exit = func.fresh_block();
    func.label_id_map.insert(exit.0, 77);
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops = vec![
            try_start(78),
            named_call("fixture_external_call", vec![], vec![owner]),
            check(77),
            named_call("fixture_external_call", vec![owner], vec![]),
        ];
        block.terminator = Terminator::Return { values: vec![] };
    }
    func.blocks.insert(exit, returning_block(exit, vec![]));

    run(&mut func, &mut AnalysisManager::new());

    let checks = observations(&func, entry);
    assert_eq!(checks.len(), 1);
    let AttrValue::Int(label) = checks[0].attrs["value"] else {
        panic!("the observation lost its exceptional target")
    };
    assert!(
        label != 77 && label != 78,
        "the landing reused existing exception label {label}"
    );
    assert_eq!(exceptional_releases(&func, &checks[0]), vec![owner]);
}
