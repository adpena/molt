use super::*;
use crate::tir::blocks::TirBlock;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, Dialect};
use crate::tir::values::TirValue;

fn operation(opcode: OpCode, operands: &[u32], result: u32) -> TirOp {
    TirOp {
        origin: None,
        dialect: Dialect::Molt,
        opcode,
        operands: operands.iter().copied().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn none_branch(annotation: TirType) -> TirFunction {
    let mut func = TirFunction::new(
        "callee".into(),
        vec![annotation],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.next_block = 3;
    func.next_value = 3;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        operation(OpCode::ConstNone, &[], 1),
        operation(OpCode::Is, &[0, 1], 2),
    ];
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(2),
        then_block: BlockId(1),
        then_args: vec![],
        else_block: BlockId(2),
        else_args: vec![],
    };
    for id in [BlockId(1), BlockId(2)] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    func
}

#[test]
fn annotation_counterfactual_never_seeds_the_product_solve_or_changes_ir() {
    for (annotation, expected) in [
        (TirType::I64, AnnotationEffect::DecidesFromNullness),
        (TirType::None, AnnotationEffect::DecidesFromNullness),
        (TirType::DynBox, AnnotationEffect::Unavailable),
        (
            TirType::Union(vec![TirType::I64, TirType::None]),
            AnnotationEffect::Unavailable,
        ),
    ] {
        let func = none_branch(annotation);
        let before = serde_json::to_value(&func).unwrap();
        let mut solver = Solver::new(&func);
        solver.solve();
        let report = capture(&solver);
        assert_eq!(report.undecided_branches, 1);
        assert_eq!(report.parameter_branches.len(), 1);
        assert_eq!(report.parameter_branches[0].annotation, expected);
        assert_eq!(*solver.state(ValueId(0)), Lattice::Overdefined);
        assert!(
            decided_edge(&func.blocks[&func.entry_block].terminator, &solver.lattice).is_none()
        );
        assert_eq!(serde_json::to_value(&func).unwrap(), before);
    }
}

#[test]
fn dependence_follows_forwarded_block_arguments_and_deduplicates_uses() {
    let mut func = none_branch(TirType::DynBox);
    // Move the predicate into an actual successor with a block argument. Its
    // value is the parameter regardless of both redundant copy operands.
    let mut body = func.blocks[&func.entry_block].clone();
    body.id = BlockId(3);
    body.args = vec![TirValue {
        id: ValueId(3),
        ty: TirType::DynBox,
    }];
    body.ops[1].operands[0] = ValueId(4);
    body.ops.insert(1, operation(OpCode::Copy, &[3, 3], 4));
    func.blocks.insert(body.id, body);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.clear();
    entry.terminator = Terminator::Branch {
        target: BlockId(3),
        args: vec![ValueId(0)],
    };
    func.next_block = 4;
    func.next_value = 5;
    let mut solver = Solver::new(&func);
    solver.solve();
    let report = capture(&solver);
    assert_eq!(report.undecided_branches, 1);
    assert_eq!(report.parameter_branches.len(), 1);
    assert_eq!(report.parameter_branches[0].parameter, ParamIndex(0));
    assert_eq!(report.parameter_branches[0].block, BlockId(3));
}

#[test]
fn guarded_and_builder_calls_do_not_become_static_edges_or_argument_facts() {
    // Exercise the REAL SimpleIR boundary, including a stale target hint and
    // a consumed builder. Counting [callee, builder] as two args is a defect.
    let ir: molt_ir::FunctionIR = serde_json::from_value(serde_json::json!({
        "name": "caller", "return_abi": "value", "params": ["callee"],
        "ops": [
            {"kind": "const_none", "out": "n"},
            {"kind": "call_guarded", "s_value": "candidate", "args": ["callee", "n"], "out": "r"},
            {"kind": "callargs_new", "out": "builder"},
            {"kind": "call_bind", "args": ["callee", "builder"], "out": "b"},
            {"kind": "ret", "args": ["b"]}
        ]
    }))
    .unwrap();
    let func = crate::tir::lower_from_simple::lower_to_tir(&ir);
    let mut solver = Solver::new(&func);
    solver.solve();
    let report = capture(&solver);
    assert_eq!(report.calls.len(), 2);
    assert_eq!(report.calls[0].kind, CallKind::Guarded);
    assert_eq!(report.calls[0].candidate.as_ref().unwrap().0, "candidate");
    assert_eq!(report.calls[0].positional, Some(vec![ArgumentFact::None]));
    assert_eq!(report.calls[1].kind, CallKind::Bind);
    assert!(report.calls[1].candidate.is_none());
    assert!(report.calls[1].positional.is_none());
    for block in func.blocks.values() {
        for op in &block.ops {
            if matches!(
                op.origin,
                Some(molt_ir::OpKind::CallGuarded | molt_ir::OpKind::CallBind)
            ) {
                assert!(direct_call_symbol_for_op(op).is_none());
            }
        }
    }
}

#[test]
fn argument_facts_are_at_the_site_and_preserve_constant_categories() {
    let func = none_branch(TirType::DynBox);
    let mut solver = Solver::new(&func);
    solver.solve();
    assert_eq!(
        argument_fact(&solver, BlockId(0), ValueId(0)),
        ArgumentFact::Unknown
    );
    assert_eq!(
        argument_fact(&solver, BlockId(1), ValueId(0)),
        ArgumentFact::None
    );
    assert_eq!(
        argument_fact(&solver, BlockId(2), ValueId(0)),
        ArgumentFact::NotNone
    );
    for (constant, expected) in [
        (ConstVal::Bool(true), ArgumentFact::True),
        (ConstVal::Bool(false), ArgumentFact::False),
        (ConstVal::Int(0), ArgumentFact::OtherConstant),
    ] {
        solver
            .lattice
            .insert(ValueId(7), Lattice::Constant(constant));
        assert_eq!(argument_fact(&solver, BlockId(0), ValueId(7)), expected);
    }
}
