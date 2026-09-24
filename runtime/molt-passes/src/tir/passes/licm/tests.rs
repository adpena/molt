use super::super::value_range::ValueRangeResult;
use super::run;
use super::safety::throw_condition_disproven;
use crate::ir::{FunctionIR, OpIR};
use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::lower_from_simple::lower_to_tir;
use crate::tir::numeric_facts::IntRange;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::pass_manager::{Mutates, PassManager, TirPass};
use crate::tir::passes::PassStats;
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

struct LicmOnlyPass;

impl TirPass for LicmOnlyPass {
    fn name(&self) -> &'static str {
        "licm"
    }

    fn mutation_class(&self) -> Mutates {
        Mutates::Cfg
    }

    fn run(
        &self,
        func: &mut TirFunction,
        am: &mut AnalysisManager,
        _tti: &TargetInfo,
    ) -> PassStats {
        super::run(func, am)
    }
}

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

struct SingleLoop {
    preheader: BlockId,
    header: BlockId,
    body: BlockId,
    loop_arg: ValueId,
}

fn build_single_loop(func: &mut TirFunction) -> SingleLoop {
    let preheader = func.fresh_block();
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let seed = func.fresh_value();
    let loop_arg = func.fresh_value();
    let cond = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(make_const_int(0, seed));
    entry.terminator = Terminator::Branch {
        target: preheader,
        args: vec![],
    };
    func.blocks.insert(
        preheader,
        TirBlock {
            id: preheader,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![seed],
            },
        },
    );
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: loop_arg,
                ty: TirType::I64,
            }],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstBool,
                operands: vec![],
                results: vec![cond],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::CondBranch {
                cond,
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
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![loop_arg],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_roles.insert(body, LoopRole::LoopEnd);
    func.loop_pairs.insert(header, body);

    SingleLoop {
        preheader,
        header,
        body,
        loop_arg,
    }
}

#[test]
fn throw_proof_does_not_use_success_body_range_for_failed_guards() {
    for (opcode, start, stop) in [(OpCode::FloorDiv, 10, 0), (OpCode::Shr, 1, -1)] {
        let mut func = TirFunction::new(
            "guard_throw".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let region = build_single_loop(&mut func);
        let seed = func.blocks[&region.preheader]
            .terminator
            .first_edge_args_to(region.header)
            .unwrap()[0];
        let bound = func.fresh_value();
        let step = func.fresh_value();
        let one = func.fresh_value();
        let next = func.fresh_value();
        let result = func.fresh_value();
        let condition = func.blocks[&region.header].ops[0].results[0];
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops[0] = make_const_int(start, seed);
        entry.ops.extend([
            make_const_int(stop, bound),
            make_const_int(-1, step),
            make_const_int(1, one),
        ]);
        let probe = make_binop(opcode, one, region.loop_arg, result);
        func.blocks.get_mut(&region.header).unwrap().ops = vec![
            probe.clone(),
            make_binop(OpCode::Gt, region.loop_arg, bound, condition),
        ];
        let body = func.blocks.get_mut(&region.body).unwrap();
        body.ops = vec![make_binop(OpCode::Add, region.loop_arg, step, next)];
        body.terminator = Terminator::Branch {
            target: region.header,
            args: vec![next],
        };
        let vr = crate::representation_facts::value_range_for(&func);
        let types =
            std::collections::HashMap::from([(one, TirType::I64), (region.loop_arg, TirType::I64)]);
        assert!(
            !throw_condition_disproven(&probe, &vr, &types),
            "failed guard must retain division-zero/negative-shift exception"
        );
        let local = vr.range_at(region.body, region.loop_arg);
        if opcode == OpCode::FloorDiv {
            assert!(local.proves_nonzero());
        } else {
            assert!(local.proves_i64_shift_count());
        }
        run(&mut func, &mut AnalysisManager::new());
        assert!(
            func.blocks[&region.header]
                .ops
                .iter()
                .any(|op| op.results == [result])
        );
    }
}

/// Build:  entry -> preheader -> loop_header -> loop_body -> loop_header
///                                         \ exit
/// with a+b computed inside the loop body.
#[test]
fn invariant_add_hoisted_to_preheader() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let a = func.fresh_value();
    let b = func.fresh_value();

    let preheader = func.fresh_block();
    let loop_header = func.fresh_block();
    let loop_body = func.fresh_block();
    let exit = func.fresh_block();

    let loop_var = func.fresh_value();
    let sum_ab = func.fresh_value(); // a + b - loop invariant
    let result = func.fresh_value();
    let cond = func.fresh_value();

    // Entry -> preheader
    {
        let init = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(4, a));
        entry.ops.push(make_const_int(5, b));
        entry.ops.push(make_const_int(0, init));
        entry.terminator = Terminator::Branch {
            target: preheader,
            args: vec![],
        };
    }

    // Preheader -> loop_header
    func.blocks.insert(
        preheader,
        TirBlock {
            id: preheader,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![],
            },
        },
    );

    // Loop header: CondBranch -> body or exit
    func.blocks.insert(
        loop_header,
        TirBlock {
            id: loop_header,
            args: vec![TirValue {
                id: loop_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: loop_body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    // Loop body: compute a+b (invariant!), use it locally, and loop back.
    // sum_ab is NOT passed as a branch arg (phi value), so it can be hoisted.
    let use_val = func.fresh_value();
    func.blocks.insert(
        loop_body,
        TirBlock {
            id: loop_body,
            args: vec![],
            ops: vec![
                make_binop(OpCode::Add, a, b, sum_ab), // invariant - should be hoisted
                make_binop(OpCode::Add, sum_ab, loop_var, use_val), // uses sum_ab locally
            ],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![use_val],
            },
        },
    );

    // Exit
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![TirValue {
                id: result,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );

    // Mark loop_header as a loop header.
    func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // The invariant Add(a, b) -> sum_ab should have been hoisted.
    // The non-invariant Add(sum_ab, loop_var) -> use_val should remain.
    let body_ops = &func.blocks[&loop_body].ops;
    let invariant_add_remains = body_ops
        .iter()
        .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
    assert!(
        !invariant_add_remains,
        "Invariant Add(a, b) should have been hoisted out of the loop body"
    );

    let preheader_ops = &func.blocks[&preheader].ops;
    assert!(
        preheader_ops.iter().any(|op| op.opcode == OpCode::Add),
        "Add should appear in the preheader"
    );

    assert!(stats.ops_removed > 0 || stats.ops_added > 0);
}

#[test]
fn reachable_preheader_ignores_retained_unreachable_predecessor() {
    let mut func = TirFunction::new(
        "dead_predecessor".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let loop_shape = build_single_loop(&mut func);
    let invariant = func.fresh_value();
    func.blocks
        .get_mut(&loop_shape.body)
        .unwrap()
        .ops
        .push(make_const_int(41, invariant));

    let dead_latch = func.fresh_block();
    func.blocks.insert(
        dead_latch,
        TirBlock {
            id: dead_latch,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_shape.header,
                args: vec![loop_shape.loop_arg],
            },
        },
    );
    func.loop_roles.insert(dead_latch, LoopRole::LoopEnd);
    func.loop_pairs.insert(loop_shape.header, dead_latch);

    run(&mut func, &mut AnalysisManager::new());

    assert!(
        func.blocks[&loop_shape.preheader]
            .ops
            .iter()
            .any(|op| op.results == vec![invariant]),
        "an unreachable retained latch must not invalidate the executable preheader"
    );
}

#[test]
fn function_parameter_operand_can_hoist() {
    let mut func = TirFunction::new(
        "parameter_dominates_preheader".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let parameter = ValueId(0);
    let loop_shape = build_single_loop(&mut func);
    let copied = func.fresh_value();
    func.blocks
        .get_mut(&loop_shape.body)
        .unwrap()
        .ops
        .push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![parameter],
            results: vec![copied],
            attrs: AttrDict::new(),
            source_span: None,
        });

    run(&mut func, &mut AnalysisManager::new());

    assert!(
        func.blocks[&loop_shape.preheader]
            .ops
            .iter()
            .any(|op| op.results == vec![copied]),
        "entry-defined function parameters must be available at the preheader"
    );
}

#[test]
fn loop_block_argument_operand_cannot_hoist() {
    let mut func = TirFunction::new(
        "loop_argument".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let loop_shape = build_single_loop(&mut func);
    let copied = func.fresh_value();
    func.blocks
        .get_mut(&loop_shape.body)
        .unwrap()
        .ops
        .push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![loop_shape.loop_arg],
            results: vec![copied],
            attrs: AttrDict::new(),
            source_span: None,
        });

    run(&mut func, &mut AnalysisManager::new());

    assert!(
        func.blocks[&loop_shape.body]
            .ops
            .iter()
            .any(|op| op.results == vec![copied]),
        "a loop-carried block argument is defined inside the loop"
    );
}

#[test]
fn sibling_definition_that_does_not_dominate_preheader_cannot_hoist() {
    let mut func = TirFunction::new(
        "sibling_definition".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let loop_shape = build_single_loop(&mut func);
    let sibling = func.fresh_block();
    let entry_cond = func.fresh_value();
    let sibling_value = func.fresh_value();
    let copied = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![entry_cond],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::CondBranch {
            cond: entry_cond,
            then_block: loop_shape.preheader,
            then_args: vec![],
            else_block: sibling,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        sibling,
        TirBlock {
            id: sibling,
            args: vec![],
            ops: vec![make_const_int(7, sibling_value)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks
        .get_mut(&loop_shape.body)
        .unwrap()
        .ops
        .push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![sibling_value],
            results: vec![copied],
            attrs: AttrDict::new(),
            source_span: None,
        });

    run(&mut func, &mut AnalysisManager::new());

    assert!(
        func.blocks[&loop_shape.body]
            .ops
            .iter()
            .any(|op| op.results == vec![copied]),
        "being outside the loop is insufficient without dominance of the destination"
    );
}

#[test]
fn same_round_hoists_preserve_source_order() {
    let mut func = TirFunction::new(
        "stable_hoist_order".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let loop_shape = build_single_loop(&mut func);
    let first = func.fresh_value();
    let second = func.fresh_value();
    func.blocks
        .get_mut(&loop_shape.body)
        .unwrap()
        .ops
        .extend([make_const_int(11, first), make_const_int(22, second)]);

    run(&mut func, &mut AnalysisManager::new());

    let preheader_ops = &func.blocks[&loop_shape.preheader].ops;
    let first_pos = preheader_ops
        .iter()
        .position(|op| op.results == vec![first])
        .expect("first invariant must hoist");
    let second_pos = preheader_ops
        .iter()
        .position(|op| op.results == vec![second])
        .expect("second invariant must hoist");
    assert!(
        first_pos < second_pos,
        "back-to-front removal must not reverse destination order"
    );
}

#[test]
fn luau_loop_pending_observer_stays_inside_executable_loop_through_pass_manager() {
    let source = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "loop_pending_observer".into(),
        ops: vec![
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_exception".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("ok".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["ok".into()]),
                ..OpIR::default()
            },
        ],
        ..FunctionIR::default()
    };
    let mut func = lower_to_tir(&source);

    let pending_block_before = func
        .blocks
        .iter()
        .find_map(|(&bid, block)| {
            block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::ExceptionPending)
                .then_some(bid)
        })
        .expect("SimpleIR lift must materialize the loop pending observer");
    let mut am = AnalysisManager::new();
    let forest_before = am.get::<LoopForest>(&func);
    assert!(
        forest_before.headers.is_empty(),
        "the retained loop_end after an unconditional break is not an executable backedge"
    );

    let pipeline = PassManager::new(
        vec![Box::new(LicmOnlyPass)],
        TargetInfo::luau_release_fast(),
    );
    let stats = pipeline.run(&mut func);
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].name, "licm");

    let pending_block_after = func
        .blocks
        .iter()
        .find_map(|(&bid, block)| {
            block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::ExceptionPending)
                .then_some(bid)
        })
        .expect("LICM must retain the runtime pending observer");
    let mut am = AnalysisManager::new();
    let forest_after = am.get::<LoopForest>(&func);
    assert!(forest_after.headers.is_empty());
    assert_eq!(pending_block_after, pending_block_before);
}

#[test]
fn fallback_semantic_copy_is_not_hoisted() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let module = ValueId(0);
    let attr_name = ValueId(1);

    let preheader = func.fresh_block();
    let loop_header = func.fresh_block();
    let loop_body = func.fresh_block();
    let exit = func.fresh_block();

    let loop_var = func.fresh_value();
    let lookup = func.fresh_value();
    let cond = func.fresh_value();

    {
        let init = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(0, init));
        entry.terminator = Terminator::Branch {
            target: preheader,
            args: vec![],
        };
    }

    func.blocks.insert(
        preheader,
        TirBlock {
            id: preheader,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![],
            },
        },
    );

    func.blocks.insert(
        loop_header,
        TirBlock {
            id: loop_header,
            args: vec![TirValue {
                id: loop_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: loop_body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    func.blocks.insert(
        loop_body,
        TirBlock {
            id: loop_body,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![module, attr_name],
                results: vec![lookup],
                attrs: {
                    let mut attrs = AttrDict::new();
                    attrs.insert(
                        "_original_kind".into(),
                        AttrValue::Str("module_get_attr".into()),
                    );
                    attrs
                },
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![loop_var],
            },
        },
    );

    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    assert!(
        func.blocks[&loop_body].ops.iter().any(|op| {
            op.opcode == OpCode::Copy
                && matches!(
                    op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if kind == "module_get_attr"
                )
        }),
        "fallback semantic Copy ops must not be hoisted as pure copies"
    );
    assert!(
        func.blocks[&preheader]
            .ops
            .iter()
            .all(|op| { !(op.opcode == OpCode::Copy && op.attrs.contains_key("_original_kind")) }),
        "semantic fallback Copy must not move into the preheader"
    );
}

/// Layout of a canonical 2-level nested loop CFG:
/// ```text
/// entry -> outer_ph -> outer_h <-> outer_b -> inner_ph -> inner_h <-> inner_b
///                       v                                  \
///                    outer_exit <- inner_exit <- inner_h
/// ```
/// Back edges: inner_b -> inner_h (inner loop), inner_exit -> outer_h (outer loop).
/// outer_ph is the outer-loop preheader; inner_ph is the inner-loop preheader.
/// inner_ph lives *inside* the outer loop body, but *outside* the inner loop body.
struct NestedLoop {
    outer_ph: BlockId,
    outer_h: BlockId,
    outer_b: BlockId,
    inner_ph: BlockId,
    inner_h: BlockId,
    inner_b: BlockId,
    inner_exit: BlockId,
    outer_exit: BlockId,
    outer_var: ValueId,
    inner_var: ValueId,
    cond_outer: ValueId,
    cond_inner: ValueId,
    result: ValueId,
}

/// Build the canonical 2-level nested-loop CFG with empty bodies.
/// The caller fills in the inner_b ops and any extra preheader/header ops.
fn build_nested_loop(func: &mut TirFunction) -> NestedLoop {
    let outer_ph = func.fresh_block();
    let outer_h = func.fresh_block();
    let outer_b = func.fresh_block();
    let inner_ph = func.fresh_block();
    let inner_h = func.fresh_block();
    let inner_b = func.fresh_block();
    let inner_exit = func.fresh_block();
    let outer_exit = func.fresh_block();

    let outer_var = func.fresh_value();
    let inner_var = func.fresh_value();
    let cond_outer = func.fresh_value();
    let cond_inner = func.fresh_value();
    let result = func.fresh_value();
    let outer_init = func.fresh_value();
    let outer_ph_arg = func.fresh_value();
    let inner_init = func.fresh_value();
    let inner_ph_arg = func.fresh_value();
    let outer_next = func.fresh_value();
    let inner_next = func.fresh_value();

    // entry: -> outer_ph (with outer_init)
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(0, outer_init));
        entry.terminator = Terminator::Branch {
            target: outer_ph,
            args: vec![outer_init],
        };
    }

    // outer_ph: takes the outer-loop seed; -> outer_h
    func.blocks.insert(
        outer_ph,
        TirBlock {
            id: outer_ph,
            args: vec![TirValue {
                id: outer_ph_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: outer_h,
                args: vec![outer_ph_arg],
            },
        },
    );

    // outer_h: outer-loop header; CondBranch -> outer_b or outer_exit
    func.blocks.insert(
        outer_h,
        TirBlock {
            id: outer_h,
            args: vec![TirValue {
                id: outer_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond_outer)],
            terminator: Terminator::CondBranch {
                cond: cond_outer,
                then_block: outer_b,
                then_args: vec![],
                else_block: outer_exit,
                else_args: vec![outer_var],
            },
        },
    );

    // outer_b: -> inner_ph (no ops by default)
    func.blocks.insert(
        outer_b,
        TirBlock {
            id: outer_b,
            args: vec![],
            ops: vec![make_const_int(0, inner_init)],
            terminator: Terminator::Branch {
                target: inner_ph,
                args: vec![inner_init],
            },
        },
    );

    // inner_ph: takes the inner-loop seed; -> inner_h
    func.blocks.insert(
        inner_ph,
        TirBlock {
            id: inner_ph,
            args: vec![TirValue {
                id: inner_ph_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: inner_h,
                args: vec![inner_ph_arg],
            },
        },
    );

    // inner_h: inner-loop header; CondBranch -> inner_b or inner_exit
    func.blocks.insert(
        inner_h,
        TirBlock {
            id: inner_h,
            args: vec![TirValue {
                id: inner_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond_inner)],
            terminator: Terminator::CondBranch {
                cond: cond_inner,
                then_block: inner_b,
                then_args: vec![],
                else_block: inner_exit,
                else_args: vec![],
            },
        },
    );

    // inner_b: -> inner_h (back-edge). Caller fills in ops.
    func.blocks.insert(
        inner_b,
        TirBlock {
            id: inner_b,
            args: vec![],
            ops: vec![make_const_int(1, inner_next)],
            terminator: Terminator::Branch {
                target: inner_h,
                args: vec![inner_next],
            },
        },
    );

    // inner_exit: -> outer_h (outer back-edge), advancing outer_var.
    func.blocks.insert(
        inner_exit,
        TirBlock {
            id: inner_exit,
            args: vec![],
            ops: vec![make_const_int(1, outer_next)],
            terminator: Terminator::Branch {
                target: outer_h,
                args: vec![outer_next],
            },
        },
    );

    // outer_exit: Return.
    func.blocks.insert(
        outer_exit,
        TirBlock {
            id: outer_exit,
            args: vec![TirValue {
                id: result,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );

    // Mark both headers as loop headers.
    func.loop_roles.insert(outer_h, LoopRole::LoopHeader);
    func.loop_roles.insert(inner_h, LoopRole::LoopHeader);

    NestedLoop {
        outer_ph,
        outer_h,
        outer_b,
        inner_ph,
        inner_h,
        inner_b,
        inner_exit,
        outer_exit,
        outer_var,
        inner_var,
        cond_outer,
        cond_inner,
        result,
        // Suppress unused-field warnings; these are kept on the struct
        // for documentation and for richer assertions in future tests.
    }
}

/// `for i: for j: y = a + b` where `a`, `b` are exact values defined outside
/// both loops.
/// Both operands are free w.r.t. the inner loop, so the Add is hoisted
/// to the inner preheader on the inner pass; on the outer pass both
/// operands are still free w.r.t. the outer loop, so the Add is
/// hoisted again - into the outer preheader. End state: the Add lives
/// in the outer preheader; the inner preheader and inner body no
/// longer contain it.
#[test]
fn nested_loop_inner_invariant_hoisted_to_outer_preheader() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let a = func.fresh_value();
    let b = func.fresh_value();

    let nl = build_nested_loop(&mut func);
    let entry_ops = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops;
    entry_ops.insert(0, make_const_int(5, b));
    entry_ops.insert(0, make_const_int(4, a));

    // Place y = a + b inside the inner body.
    let y = func.fresh_value();
    {
        let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
        inner_b.ops.insert(0, make_binop(OpCode::Add, a, b, y));
    }
    // Suppress dead-code warnings on documentation-only struct fields.
    let _ = (
        nl.outer_var,
        nl.inner_var,
        nl.cond_outer,
        nl.cond_inner,
        nl.result,
        nl.outer_exit,
        nl.inner_exit,
        nl.outer_b,
        nl.inner_h,
        nl.outer_h,
    );

    let stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // The inner body must NOT still contain the Add(a, b).
    let inner_body_ops = &func.blocks[&nl.inner_b].ops;
    let in_inner_body = inner_body_ops
        .iter()
        .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
    assert!(
        !in_inner_body,
        "Add(a, b) must have been hoisted out of the inner body"
    );

    // The inner preheader must NOT still contain the Add (it should
    // have been hoisted further to the outer preheader).
    let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
    let in_inner_ph = inner_ph_ops
        .iter()
        .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
    assert!(
        !in_inner_ph,
        "Add(a, b) is outer-invariant - it should not stop in the inner preheader"
    );

    // The Add must end up in the outer preheader.
    let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
    let in_outer_ph = outer_ph_ops
        .iter()
        .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
    assert!(
        in_outer_ph,
        "Add(a, b) should end up in the outer preheader (multi-level hoist)"
    );

    // Multi-level hoist accounts for two move events on the same op.
    assert!(
        stats.ops_removed >= 2 && stats.ops_added >= 2,
        "expected at least 2 hoist events (inner->inner_ph, inner_ph->outer_ph), got removed={} added={}",
        stats.ops_removed,
        stats.ops_added,
    );
}

/// Force the multi-level hoist to be observable: an inner-body op
/// `t = a + b` followed by `y = t + a` chains through the inner
/// preheader on round 1 and again to the outer preheader on round 2,
/// once `t` has migrated outward. Verifies that both ops follow the
/// invariant transitively across both preheaders.
#[test]
fn nested_loop_outer_invariant_hoisted_via_inner() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let a = func.fresh_value();
    let b = func.fresh_value();

    let nl = build_nested_loop(&mut func);
    let entry_ops = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops;
    entry_ops.insert(0, make_const_int(5, b));
    entry_ops.insert(0, make_const_int(4, a));

    let t = func.fresh_value();
    let y = func.fresh_value();
    {
        let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
        // Two chained invariants. Both end up in outer preheader.
        inner_b.ops.insert(0, make_binop(OpCode::Add, a, b, t));
        inner_b.ops.insert(1, make_binop(OpCode::Mul, t, a, y));
    }
    let _ = nl.outer_exit; // doc-only field

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
    let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
    let inner_body_ops = &func.blocks[&nl.inner_b].ops;

    // Neither op should remain in the inner body.
    assert!(
        !inner_body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]),
        "Add(a, b) must leave the inner body"
    );
    assert!(
        !inner_body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]),
        "Mul(t, a) must leave the inner body once t becomes outer-invariant"
    );

    // Neither op should rest in the inner preheader - both are
    // outer-invariant after t is hoisted out.
    assert!(
        !inner_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]),
        "Add(a, b) must not stop in the inner preheader"
    );
    assert!(
        !inner_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]),
        "Mul(t, a) must not stop in the inner preheader"
    );

    // Both ops must reach the outer preheader, in dependency order.
    let add_pos = outer_ph_ops
        .iter()
        .position(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
    let mul_pos = outer_ph_ops
        .iter()
        .position(|op| op.opcode == OpCode::Mul && op.operands == vec![t, a]);
    assert!(add_pos.is_some(), "Add(a, b) must land in outer preheader");
    assert!(mul_pos.is_some(), "Mul(t, a) must land in outer preheader");
    assert!(
        add_pos.unwrap() < mul_pos.unwrap(),
        "Add(a, b) must precede Mul(t, a) in the outer preheader (dependency order)"
    );
}

/// Partially invariant: `for i: for j: y = i + a`. `a` is an exact value
/// defined before both loops. `i` is the outer-loop induction variable, invariant
/// w.r.t. the inner loop only. The Add must therefore land in the *inner*
/// preheader (not the outer preheader, since `i` changes per outer iteration).
#[test]
fn nested_loop_partially_invariant_hoists_to_inner_preheader() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let a = func.fresh_value();

    let nl = build_nested_loop(&mut func);
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .ops
        .insert(0, make_const_int(4, a));
    let i = nl.outer_var; // outer-loop induction variable

    let y = func.fresh_value();
    {
        let inner_b = func.blocks.get_mut(&nl.inner_b).unwrap();
        inner_b.ops.insert(0, make_binop(OpCode::Add, i, a, y));
    }
    let _ = nl.outer_exit;

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // Op must leave the inner body.
    let inner_body_ops = &func.blocks[&nl.inner_b].ops;
    assert!(
        !inner_body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
        "Add(i, a) must leave the inner body - invariant w.r.t. the inner loop"
    );

    // Op must land in the inner preheader.
    let inner_ph_ops = &func.blocks[&nl.inner_ph].ops;
    assert!(
        inner_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
        "Add(i, a) should land in the inner preheader (i changes per outer iter)"
    );

    // Op must NOT escape to the outer preheader - `i` is not outer-invariant.
    let outer_ph_ops = &func.blocks[&nl.outer_ph].ops;
    assert!(
        !outer_ph_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![i, a]),
        "Add(i, a) must NOT reach the outer preheader - i is the outer induction var"
    );
}

#[test]
fn non_invariant_not_hoisted() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::I64],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let a = ValueId(0);

    let preheader = func.fresh_block();
    let loop_header = func.fresh_block();
    let loop_body = func.fresh_block();
    let exit = func.fresh_block();

    let loop_var = func.fresh_value();
    let sum = func.fresh_value(); // a + loop_var - NOT invariant
    let cond = func.fresh_value();
    let result = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: preheader,
            args: vec![],
        };
    }

    func.blocks.insert(
        preheader,
        TirBlock {
            id: preheader,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![],
            },
        },
    );

    func.blocks.insert(
        loop_header,
        TirBlock {
            id: loop_header,
            args: vec![TirValue {
                id: loop_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: loop_body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );

    // a + loop_var: loop_var is defined inside the loop -> NOT invariant.
    func.blocks.insert(
        loop_body,
        TirBlock {
            id: loop_body,
            args: vec![],
            ops: vec![make_binop(OpCode::Add, a, loop_var, sum)],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![sum],
            },
        },
    );

    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![TirValue {
                id: result,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );

    func.loop_roles.insert(loop_header, LoopRole::LoopHeader);

    let _stats = run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());

    // The Add uses loop_var (defined in loop header) - should NOT be hoisted.
    let body_ops = &func.blocks[&loop_body].ops;
    assert!(
        body_ops.iter().any(|op| op.opcode == OpCode::Add),
        "Add should remain in the loop body (not invariant)"
    );
}

// -- #49: proven-safe `pure_may_throw` hoist gate ------------------------

/// Direct unit coverage of the throw-disproof predicate against a
/// hand-built `ValueRangeResult`, exercising every per-opcode arm and the
/// `Pow` refusal - independent of the full pipeline.
#[test]
fn throw_condition_disproven_per_opcode() {
    let mut vr = ValueRangeResult::default();
    let x = ValueId(1);
    let count_ok = ValueId(2); // [0, 63]
    let count_neg = ValueId(3); // [-4, 10] (straddles negative)
    let count_big = ValueId(4); // [0, 200] (exceeds 63)
    let count_unknown = ValueId(5); // FULL
    let divisor_nz = ValueId(6); // [1, 9] (non-zero)
    let divisor_zero = ValueId(7); // [-2, 5] (straddles zero)
    let _ = x;
    vr.record_global_range(count_ok, IntRange::new(0, 63));
    vr.record_global_range(count_neg, IntRange::new(-4, 10));
    vr.record_global_range(count_big, IntRange::new(0, 200));
    vr.record_global_range(divisor_nz, IntRange::new(1, 9));
    vr.record_global_range(divisor_zero, IntRange::new(-2, 5));
    // count_unknown deliberately left absent (FULL).
    let exact_integers = std::collections::HashMap::from([
        (x, TirType::I64),
        (count_ok, TirType::I64),
        (count_neg, TirType::I64),
        (count_big, TirType::I64),
        (count_unknown, TirType::I64),
        (divisor_nz, TirType::I64),
        (divisor_zero, TirType::I64),
    ]);

    // Shl / Shr: disproven iff count in [0, 63].
    assert!(throw_condition_disproven(
        &make_binop(OpCode::Shl, x, count_ok, ValueId(20)),
        &vr,
        &exact_integers,
    ));
    assert!(throw_condition_disproven(
        &make_binop(OpCode::Shr, x, count_ok, ValueId(21)),
        &vr,
        &exact_integers,
    ));
    assert!(
        !throw_condition_disproven(
            &make_binop(OpCode::Shl, x, count_neg, ValueId(22)),
            &vr,
            &exact_integers,
        ),
        "a possibly-negative count can raise ValueError - must NOT be disproven"
    );
    assert!(
        !throw_condition_disproven(
            &make_binop(OpCode::Shl, x, count_big, ValueId(23)),
            &vr,
            &exact_integers,
        ),
        "a count > 63 is a wrong-value machine shift - must NOT be disproven"
    );
    assert!(
        !throw_condition_disproven(
            &make_binop(OpCode::Shl, x, count_unknown, ValueId(24)),
            &vr,
            &exact_integers,
        ),
        "an unknown count must NOT be disproven (fail-closed)"
    );

    // Exact-integer FloorDiv / Mod are discharged by a non-zero divisor.
    for opcode in [OpCode::FloorDiv, OpCode::Mod] {
        assert!(
            throw_condition_disproven(
                &make_binop(opcode, x, divisor_nz, ValueId(30)),
                &vr,
                &exact_integers,
            ),
            "{opcode:?} with a non-zero divisor must be disproven"
        );
        assert!(
            !throw_condition_disproven(
                &make_binop(opcode, x, divisor_zero, ValueId(31)),
                &vr,
                &exact_integers,
            ),
            "{opcode:?} with a possibly-zero divisor must NOT be disproven"
        );
    }

    // True division still converts the semantic integer result to float, which
    // may overflow even when the divisor is non-zero.
    assert!(!throw_condition_disproven(
        &make_binop(OpCode::Div, x, divisor_nz, ValueId(32)),
        &vr,
        &exact_integers,
    ));

    // Pow: REFUSED unconditionally (gnarly base/exponent coupling).
    assert!(
        !throw_condition_disproven(
            &make_binop(OpCode::Pow, x, divisor_nz, ValueId(40)),
            &vr,
            &exact_integers,
        ),
        "Pow's throw condition is not a single-operand range fact - always refused"
    );

    for non_integral in [
        std::collections::HashMap::from([(x, TirType::F64), (divisor_nz, TirType::F64)]),
        std::collections::HashMap::from([(x, TirType::I64), (divisor_nz, TirType::F64)]),
        std::collections::HashMap::from([
            (x, TirType::UserClass("Left".into())),
            (divisor_nz, TirType::I64),
        ]),
        // Annotation-only types are absent from the exact-scalar map.
        std::collections::HashMap::new(),
    ] {
        assert!(!throw_condition_disproven(
            &make_binop(OpCode::FloorDiv, x, divisor_nz, ValueId(41)),
            &vr,
            &non_integral,
        ));
    }

    let mut malformed_operands = make_binop(OpCode::FloorDiv, x, divisor_nz, ValueId(42));
    malformed_operands.operands.push(count_ok);
    assert!(!throw_condition_disproven(
        &malformed_operands,
        &vr,
        &exact_integers,
    ));
    let mut malformed_results = make_binop(OpCode::FloorDiv, x, divisor_nz, ValueId(43));
    malformed_results.results.clear();
    assert!(!throw_condition_disproven(
        &malformed_results,
        &vr,
        &exact_integers,
    ));
}

/// Build a loop whose body contains a loop-invariant `y = x << k` (both `x`
/// and `k` defined in the preheader), with `k` a `ConstInt` whose value
/// determines whether the shift's `ValueError` throw is range-disproven.
/// Returns the function plus the loop-body block id and the shift result id.
fn invariant_shift_loop(shift_count: i64) -> (TirFunction, BlockId, ValueId) {
    let mut func = TirFunction::new(
        "sh".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let x = func.fresh_value();
    let preheader = func.fresh_block();
    let loop_header = func.fresh_block();
    let loop_body = func.fresh_block();
    let exit = func.fresh_block();

    let k = func.fresh_value(); // ConstInt shift count (preheader -> invariant)
    let loop_var = func.fresh_value();
    let y = func.fresh_value(); // x << k - the hoist candidate
    let use_val = func.fresh_value();
    let cond = func.fresh_value();
    let result = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(7, x));
        entry.terminator = Terminator::Branch {
            target: preheader,
            args: vec![],
        };
    }
    // Preheader defines the invariant shift count `k`.
    func.blocks.insert(
        preheader,
        TirBlock {
            id: preheader,
            args: vec![],
            ops: vec![make_const_int(shift_count, k)],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        loop_header,
        TirBlock {
            id: loop_header,
            args: vec![TirValue {
                id: loop_var,
                ty: TirType::I64,
            }],
            ops: vec![make_const_int(1, cond)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: loop_body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    // Body: y = x << k (invariant!), then use it locally and loop back.
    func.blocks.insert(
        loop_body,
        TirBlock {
            id: loop_body,
            args: vec![],
            ops: vec![
                make_binop(OpCode::Shl, x, k, y),
                make_binop(OpCode::Add, y, loop_var, use_val),
            ],
            terminator: Terminator::Branch {
                target: loop_header,
                args: vec![use_val],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![TirValue {
                id: result,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );
    func.loop_roles.insert(loop_header, LoopRole::LoopHeader);
    (func, loop_body, y)
}

#[test]
fn invariant_shift_with_proven_count_is_hoisted() {
    // y = x << 12: count 12 is range-proven in [0, 63] -> ValueError throw
    // disproven -> the shift is provably nothrow at the hoist site -> hoisted.
    let (mut func, loop_body, _y) = invariant_shift_loop(12);
    run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    let body_has_shl = func.blocks[&loop_body]
        .ops
        .iter()
        .any(|op| op.opcode == OpCode::Shl);
    assert!(
        !body_has_shl,
        "loop-invariant `x << 12` (count proven [0,63]) must be hoisted out of the loop"
    );
}

#[test]
fn invariant_shift_with_unprovable_count_is_not_hoisted() {
    // y = x << 80: count 80 is OUTSIDE [0, 63] -> the raw machine shift is a
    // wrong value (and CPython produces a bigint, but the point is the throw/
    // well-definedness is NOT disproven) -> must NOT be hoisted above the
    // guard. It stays in the loop on the boxed lane (BigInt-correct).
    let (mut func, loop_body, _y) = invariant_shift_loop(80);
    run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    let body_has_shl = func.blocks[&loop_body]
        .ops
        .iter()
        .any(|op| op.opcode == OpCode::Shl);
    assert!(
        body_has_shl,
        "`x << 80` (count outside [0,63]) must NOT be hoisted - throw/well-definedness not disproven"
    );
}

#[test]
fn invariant_negative_shift_count_is_not_hoisted() {
    // y = x << -1: a negative count raises ValueError every iteration the
    // loop runs. Hoisting it would move that raise to the preheader, where it
    // fires even if the loop body would never execute (zero-trip). The throw
    // is NOT disproven (count range [-1,-1] has lo < 0) -> must NOT hoist.
    let (mut func, loop_body, _y) = invariant_shift_loop(-1);
    run(&mut func, &mut crate::tir::analysis::AnalysisManager::new());
    let body_has_shl = func.blocks[&loop_body]
        .ops
        .iter()
        .any(|op| op.opcode == OpCode::Shl);
    assert!(
        body_has_shl,
        "`x << -1` raises ValueError - hoisting would move the raise above the guard"
    );
}
