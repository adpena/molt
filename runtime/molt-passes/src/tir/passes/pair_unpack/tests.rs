use super::{Refusal, analyze, run};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::default(),
        source_span: None,
    }
}

fn with_value(mut tir_op: TirOp, value: i64) -> TirOp {
    tir_op.attrs.insert("value".into(), AttrValue::Int(value));
    tir_op
}

fn location(value: i64) -> TirOp {
    with_value(op(OpCode::FrameSetLocation, vec![], vec![]), value)
}

fn check(label: i64) -> TirOp {
    with_value(op(OpCode::CheckException, vec![], vec![]), label)
}

const LABEL: i64 = 14;
const LOCATION: i64 = 6;

/// What a fixture changes from the shape every pair loop has today.
#[derive(Default, Clone, Copy)]
struct Shape {
    item_also_returned: bool,
    arity: Option<usize>,
    call_between: bool,
    other_location: bool,
    other_label: bool,
    step_unobserved: bool,
    body_second_entry: bool,
    operand_check_between: bool,
}

struct Fixture {
    func: TirFunction,
    item: ValueId,
    done: ValueId,
    targets: Vec<ValueId>,
    header: BlockId,
    body: BlockId,
}

/// The dump shape of `for k, v in d.items()`:
///
/// ```text
/// bb0: br bb1
/// bb1 (header): frame_set_location 6; %e, %done = iter_next_unboxed %it;
///               check_exception 14; br bb2
/// bb2: check_exception 14 (unwind); cond_br %done, bb5, bb3
/// bb3 (body):   frame_set_location 6; check_exception 14; br bb4
/// bb4:          %k, %v = unpack_sequence %e {2}; check_exception 14; br bb1
/// bb5: return;  bb6 (handler): return
/// ```
fn fixture(shape: Shape) -> Fixture {
    let mut func = TirFunction::new(
        "pair_loop".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let iterator = func.blocks[&func.entry_block].args[0].id;
    let item = func.fresh_value();
    let done = func.fresh_value();
    let arity = shape.arity.unwrap_or(2);
    let targets: Vec<ValueId> = (0..arity).map(|_| func.fresh_value()).collect();
    let header = func.fresh_block();
    let branch = func.fresh_block();
    let body = func.fresh_block();
    let unpack_block = func.fresh_block();
    let exit = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, LABEL);
    func.label_id_map.insert(exit.0, LABEL + 1);
    let entry = func.entry_block;
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: header,
        args: vec![],
    };
    let mut header_ops = vec![
        location(LOCATION),
        op(OpCode::IterNextUnboxed, vec![iterator], vec![item, done]),
    ];
    if !shape.step_unobserved {
        header_ops.push(check(LABEL));
    }
    // The loop structure's redundant observations, as the frontend emits them.
    let mut unwind = check(LABEL);
    unwind
        .attrs
        .insert("exception_edge".into(), AttrValue::Int(1));
    let mut body_ops = vec![
        location(if shape.other_location { 7 } else { LOCATION }),
        check(LABEL),
    ];
    if shape.call_between {
        body_ops.push(op(OpCode::Call, vec![], vec![]));
    }
    if shape.operand_check_between {
        let mut bound = check(LABEL);
        bound.operands.push(iterator);
        body_ops.push(bound);
    }
    let mut unpack = op(OpCode::UnpackSequence, vec![item], targets.clone());
    unpack
        .attrs
        .insert("value".into(), AttrValue::Int(arity as i64));
    let unpack_ops = vec![
        unpack,
        check(if shape.other_label { LABEL + 1 } else { LABEL }),
    ];
    let blocks = [
        (
            header,
            header_ops,
            Terminator::Branch {
                target: branch,
                args: vec![],
            },
        ),
        (
            branch,
            vec![unwind],
            Terminator::CondBranch {
                cond: done,
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        ),
        (
            body,
            body_ops,
            Terminator::Branch {
                target: unpack_block,
                args: vec![],
            },
        ),
        (
            unpack_block,
            unpack_ops,
            Terminator::Branch {
                target: header,
                args: vec![],
            },
        ),
        (
            exit,
            vec![],
            Terminator::Return {
                values: if shape.item_also_returned {
                    vec![item]
                } else {
                    vec![]
                },
            },
        ),
        (handler, vec![], Terminator::Return { values: vec![] }),
    ];
    for (id, ops, terminator) in blocks {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops,
                terminator,
            },
        );
    }
    if shape.body_second_entry {
        // The handler re-enters the body: the not-done edge is not its only entry.
        func.blocks.get_mut(&handler).unwrap().terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
    }
    for &target in &targets {
        func.value_types.insert(target, TirType::Str);
    }
    Fixture {
        func,
        item,
        done,
        targets,
        header,
        body,
    }
}

fn refusal(shape: Shape) -> Option<Refusal> {
    let fixture = fixture(shape);
    let (plans, outcomes) = analyze(&fixture.func);
    assert_eq!(outcomes.len(), 1, "one step reads into one unpack");
    assert_eq!(plans.len(), usize::from(outcomes[0].result.is_ok()));
    outcomes[0].result.err()
}

fn count(func: &TirFunction, opcode: OpCode) -> usize {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == opcode)
        .count()
}

#[test]
fn a_sole_use_pair_unpack_becomes_one_step_with_the_unpack_results() {
    let Fixture {
        mut func,
        item,
        done,
        targets,
        header,
        ..
    } = fixture(Shape::default());
    let stats = run(&mut func);
    assert_eq!((stats.ops_removed, stats.values_changed), (2, 1));
    assert_eq!(count(&func, OpCode::UnpackSequence), 0);
    assert_eq!(count(&func, OpCode::IterNextUnboxed), 0);
    // The unpack's observation is gone; the step's and the loop's stay.
    assert_eq!(count(&func, OpCode::CheckException), 3);
    let step = func.blocks[&header]
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::IterNextUnpack2)
        .expect("fused step");
    assert_eq!(step.results, vec![targets[0], targets[1], done]);
    // The targets keep their identities and type facts; the item is gone.
    assert_eq!(func.value_types.get(&targets[0]), Some(&TirType::Str));
    assert!(!func.value_types.contains_key(&item));
    assert!(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .all(|op| !op.operands.contains(&item))
    );
}

#[test]
fn a_held_pair_keeps_its_tuple() {
    let shape = Shape {
        item_also_returned: true,
        ..Shape::default()
    };
    assert_eq!(refusal(shape), Some(Refusal::ItemHasOtherUses));
    let mut func = fixture(shape).func;
    assert_eq!(run(&mut func).total_changes(), 0);
    assert_eq!(count(&func, OpCode::IterNextUnboxed), 1);
    assert_eq!(count(&func, OpCode::UnpackSequence), 1);
}

#[test]
fn each_unproven_fact_refuses_with_its_reason() {
    assert_eq!(refusal(Shape::default()), None);
    for (shape, expected) in [
        (
            Shape {
                arity: Some(3),
                ..Shape::default()
            },
            Refusal::UnpackArity(3),
        ),
        (
            Shape {
                call_between: true,
                ..Shape::default()
            },
            Refusal::ObservableOpBetween(OpCode::Call),
        ),
        (
            Shape {
                other_location: true,
                ..Shape::default()
            },
            Refusal::LocationDiffers,
        ),
        (
            Shape {
                other_label: true,
                ..Shape::default()
            },
            Refusal::HandlerDiffers,
        ),
        (
            Shape {
                step_unobserved: true,
                ..Shape::default()
            },
            Refusal::HandlerDiffers,
        ),
        (
            Shape {
                body_second_entry: true,
                ..Shape::default()
            },
            Refusal::UnpackNotOnContinuation,
        ),
        (
            Shape {
                operand_check_between: true,
                ..Shape::default()
            },
            Refusal::ObservableOpBetween(OpCode::CheckException),
        ),
    ] {
        assert_eq!(refusal(shape), Some(expected));
    }
}

#[test]
fn fused_targets_are_initialized_only_on_the_not_done_edge() {
    use crate::tir::passes::alias_analysis::build_alias_union_find;
    use crate::tir::passes::ownership_lattice_min::OwnershipRootFacts;
    let Fixture {
        mut func,
        done,
        targets,
        header,
        body,
        ..
    } = fixture(Shape::default());
    run(&mut func);
    // The region rule reads the CondBranch in the producer's block; merge the
    // header with its branch block as the pipeline's CFG cleanup does.
    let branch = func.blocks.remove(&BlockId(header.0 + 1)).unwrap();
    func.blocks.get_mut(&header).unwrap().terminator = branch.terminator;
    let aliases = build_alias_union_find(&func);
    let facts = OwnershipRootFacts::compute(&func, &aliases);
    for target in targets {
        assert!(facts.is_conditionally_valid_result_root(aliases.root(target)));
        assert_eq!(
            facts.conditionally_valid_region(aliases.root(target)),
            Some(body)
        );
    }
    assert!(!facts.is_conditionally_valid_result_root(aliases.root(done)));
}
