use super::GuardedTargets;

fn run(func: &mut TirFunction) -> super::PassStats {
    super::run(func, &GuardedTargets::new())
}
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::repr::Repr;
use molt_ir::runtime_fn_generated::RuntimeFn;

/// `return a <op> b` over two parameters of the given types.
fn binary(opcode: OpCode, params: [TirType; 2]) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "f".into(),
        params.to_vec(),
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp::new(
        opcode,
        vec![ValueId(0), ValueId(1)],
        vec![result],
    ));
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    (func, result)
}

fn ops(func: &TirFunction, opcode: OpCode) -> Vec<(BlockId, TirOp)> {
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort();
    blocks
        .into_iter()
        .flat_map(|block| {
            func.blocks[&block]
                .ops
                .iter()
                .filter(move |op| op.opcode == opcode)
                .map(move |op| (block, op.clone()))
        })
        .collect()
}

fn assert_valid(func: &TirFunction) {
    crate::tir::verify::verify_function(func).expect("expanded function verifies");
    crate::tir::exception_regions::verify_exception_regions(func)
        .expect("expanded function keeps its exception regions");
}

/// The block holding a fast arm: the first block where `unbox` reads a value.
fn arm_block(func: &TirFunction, unbox: OpCode) -> BlockId {
    ops(func, unbox)[0].0
}

fn block_opcodes(func: &TirFunction, block: BlockId) -> Vec<OpCode> {
    func.blocks[&block].ops.iter().map(|op| op.opcode).collect()
}

#[test]
fn boxed_add_tests_int_then_float_words() {
    let (mut func, result) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 1);
    assert_valid(&func);
    // entry (int test a), int test b, int arm, float test a, float test b,
    // float arm, slow arm, join.
    assert_eq!(func.blocks.len(), 8);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2);
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 2);
    let fast = arm_block(&func, OpCode::UnboxInlineInt);
    assert_eq!(
        block_opcodes(&func, fast),
        [
            OpCode::UnboxInlineInt,
            OpCode::UnboxInlineInt,
            OpCode::Add,
            OpCode::BoxVal
        ]
    );
    let float = arm_block(&func, OpCode::UnboxFloatWord);
    assert_eq!(
        block_opcodes(&func, float),
        [
            OpCode::UnboxFloatWord,
            OpCode::UnboxFloatWord,
            OpCode::Add,
            OpCode::BoxVal
        ]
    );
    let (_, float_add) = ops(&func, OpCode::Add)
        .into_iter()
        .find(|(block, _)| *block == float)
        .unwrap();
    for operand in &float_add.operands {
        assert_eq!(func.value_types[operand], TirType::F64);
    }
    // A failed int test reaches the float tests, never the slow arm directly.
    let entry_end = &func.blocks[&func.entry_block].terminator;
    let Terminator::CondBranch { else_block, .. } = entry_end else {
        panic!("the entry tests the int tag");
    };
    assert_eq!(block_opcodes(&func, *else_block), [OpCode::IsFloatWord]);
    let calls = ops(&func, OpCode::CallRuntime);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.runtime, Some(RuntimeFn::Add));
    assert_eq!(calls[0].1.operands, vec![ValueId(0), ValueId(1)]);
    // The original result is the join's argument; the return still names it.
    let join = func
        .blocks
        .values()
        .find(|block| block.args.iter().any(|arg| arg.id == result))
        .expect("join binds the original result");
    assert!(matches!(&join.terminator, Terminator::Return { values } if values == &[result]));
    assert!(
        ops(&func, OpCode::Add)
            .iter()
            .all(|(block, _)| *block == fast || *block == float)
    );
}

#[test]
fn inplace_ops_call_their_inplace_entry_and_share_the_fast_arm() {
    for (opcode, runtime, fast) in [
        (OpCode::InplaceAdd, RuntimeFn::InplaceAdd, OpCode::Add),
        (OpCode::InplaceSub, RuntimeFn::InplaceSub, OpCode::Sub),
        (OpCode::Sub, RuntimeFn::Sub, OpCode::Sub),
    ] {
        let (mut func, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        run(&mut func);
        assert_valid(&func);
        let calls = ops(&func, OpCode::CallRuntime);
        assert_eq!(calls[0].1.runtime, Some(runtime), "{opcode:?}");
        // One payload op in the int arm and one in the float arm.
        assert_eq!(ops(&func, fast).len(), 2, "{opcode:?}");
        assert!(
            ops(&func, opcode).len() <= if opcode == fast { 2 } else { 0 },
            "{opcode:?}"
        );
    }
}

#[test]
fn the_slow_arm_keeps_the_ops_attributes_and_span() {
    let (mut func, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let entry = func.entry_block;
    let op = &mut func.blocks.get_mut(&entry).unwrap().ops[0];
    op.source_span = Some((3, 9));
    op.attrs = AttrDict::from([("marker".into(), AttrValue::Int(7))]);
    run(&mut func);
    let call = &ops(&func, OpCode::CallRuntime)[0].1;
    assert_eq!(call.source_span, Some((3, 9)));
    assert_eq!(call.attrs.get("marker"), Some(&AttrValue::Int(7)));
    assert_eq!(call.origin, None);
}

/// `return x <op> c` over one boxed parameter and the constant `c`.
fn with_constant(opcode: OpCode, c: i64) -> (TirFunction, ValueId, ValueId) {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let constant = func.fresh_value();
    let result = func.fresh_value();
    func.value_types.insert(constant, TirType::I64);
    func.value_types.insert(result, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        attrs: AttrDict::from([("value".into(), AttrValue::Int(c))]),
        ..TirOp::new(OpCode::ConstInt, vec![], vec![constant])
    });
    block
        .ops
        .push(TirOp::new(opcode, vec![ValueId(0), constant], vec![result]));
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    (func, constant, result)
}

#[test]
fn a_boxed_plus_inline_safe_constant_tests_only_the_boxed_operand() {
    let (mut func, constant, _) = with_constant(OpCode::Add, 1);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    let tests = ops(&func, OpCode::IsInlineInt);
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].1.operands, [ValueId(0)]);
    let (_, sum) = &ops(&func, OpCode::Add)[0];
    assert_eq!(
        sum.operands[1], constant,
        "the raw constant feeds the fast arm"
    );
    assert_eq!(ops(&func, OpCode::UnboxInlineInt).len(), 1);
    // The runtime entry takes boxed words: the slow arm boxes the constant.
    let (slow, call) = &ops(&func, OpCode::CallRuntime)[0];
    let boxes: Vec<_> = func.blocks[slow]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::BoxInlineInt)
        .collect();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].operands, [constant]);
    assert_eq!(call.operands, [ValueId(0), boxes[0].results[0]]);
    let vr = crate::representation_facts::value_range_for(&func);
    let repr = crate::representation_facts::repr_by_value_for(&func, Some(&vr));
    assert_eq!(repr[&sum.results[0]], Repr::RawI64FullDeopt);
}

#[test]
fn a_constant_outside_the_inline_window_keeps_its_lowering() {
    // 2**46 is past the inline payload: the sum need not be exact in i64 terms
    // of two 47-bit values, so the op keeps its current lowering.
    let (mut func, _, _) = with_constant(OpCode::Add, 1 << 46);
    assert_eq!(run(&mut func).values_changed, 0);
    assert_eq!(func.blocks.len(), 1);
}

#[test]
fn every_eligible_op_in_a_block_expands() {
    let (mut func, first) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let second = func.fresh_value();
    func.value_types.insert(second, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp::new(
        OpCode::Sub,
        vec![first, ValueId(1)],
        vec![second],
    ));
    block.terminator = Terminator::Return {
        values: vec![second],
    };
    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 2);
    assert_valid(&func);
    let calls = ops(&func, OpCode::CallRuntime);
    let runtimes: Vec<_> = calls.iter().map(|(_, op)| op.runtime).collect();
    assert_eq!(runtimes, [Some(RuntimeFn::Add), Some(RuntimeFn::Sub)]);
}

#[test]
fn payloads_are_inline_safe_and_their_sum_is_exact_full_range() {
    let (mut func, result) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    run(&mut func);
    let vr = crate::representation_facts::value_range_for(&func);
    let repr = crate::representation_facts::repr_by_value_for(&func, Some(&vr));
    for (_, op) in ops(&func, OpCode::UnboxInlineInt) {
        assert_eq!(repr[&op.results[0]], Repr::RawI64Safe);
    }
    let (_, sum) = &ops(&func, OpCode::Add)[0];
    assert_eq!(repr[&sum.results[0]], Repr::RawI64FullDeopt);
    assert!(!repr[&result].is_raw_i64_carrier());
}

#[test]
fn cleanup_merges_a_values_repeated_tag_test() {
    // a + b, then a + c: after the stage, `a` is tag-tested once.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox; 3],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let first = func.fresh_value();
    let second = func.fresh_value();
    func.value_types.insert(first, TirType::DynBox);
    func.value_types.insert(second, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp::new(
        OpCode::Add,
        vec![ValueId(0), ValueId(1)],
        vec![first],
    ));
    block.ops.push(TirOp::new(
        OpCode::Add,
        vec![ValueId(0), ValueId(2)],
        vec![second],
    ));
    let tuple = func.fresh_value();
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp::new(
        OpCode::BuildTuple,
        vec![first, second],
        vec![tuple],
    ));
    block.terminator = Terminator::Return {
        values: vec![tuple],
    };
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    assert!(crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti, None
    ));
    assert_valid(&func);
    let tests_of_a = ops(&func, OpCode::IsInlineInt)
        .into_iter()
        .filter(|(_, op)| op.operands == [ValueId(0)])
        .count();
    assert_eq!(tests_of_a, 1);
}

#[test]
fn a_function_without_eligible_ops_is_left_untouched() {
    let (mut func, _) = binary(OpCode::FloorDiv, [TirType::DynBox, TirType::DynBox]);
    let before = crate::tir::printer::print_function(&func);
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    assert!(!crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti, None
    ));
    assert_eq!(crate::tir::printer::print_function(&func), before);
}

#[test]
fn the_verifier_ties_runtime_entries_to_call_runtime() {
    let (mut func, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    run(&mut func);
    let (block, _) = ops(&func, OpCode::CallRuntime)[0];
    let call = func
        .blocks
        .get_mut(&block)
        .unwrap()
        .ops
        .first_mut()
        .unwrap();
    call.runtime = None;
    assert!(crate::tir::verify::verify_function(&func).is_err());

    let (mut func, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let entry = func.entry_block;
    func.blocks.get_mut(&entry).unwrap().ops[0].runtime = Some(RuntimeFn::Add);
    assert!(crate::tir::verify::verify_function(&func).is_err());
}

/// `return bool(x)` over one parameter of the given type.
fn truth_of(param: TirType) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "t".into(),
        vec![param],
        TirType::Bool,
        molt_ir::FunctionReturnAbi::Value,
    );
    let truth = func.fresh_value();
    func.value_types.insert(truth, TirType::Bool);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block
        .ops
        .push(TirOp::new(OpCode::Bool, vec![ValueId(0)], vec![truth]));
    block.terminator = Terminator::Return {
        values: vec![truth],
    };
    (func, truth)
}

#[test]
fn truthiness_of_a_boxed_word_tests_bool_then_int_tags() {
    let (mut func, truth) = truth_of(TirType::DynBox);
    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 1);
    assert_valid(&func);
    assert_eq!(func.blocks.len(), 6);
    assert_eq!(ops(&func, OpCode::IsBoolWord).len(), 1);
    assert_eq!(ops(&func, OpCode::UnboxBoolWord).len(), 1);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 1);
    let ne = ops(&func, OpCode::Ne);
    assert_eq!(ne.len(), 1);
    let int_block: Vec<OpCode> = func.blocks[&ne[0].0]
        .ops
        .iter()
        .map(|op| op.opcode)
        .collect();
    assert_eq!(
        int_block,
        [OpCode::UnboxInlineInt, OpCode::ConstInt, OpCode::Ne]
    );
    // The slow arm keeps the op itself; no runtime call is introduced.
    assert_eq!(ops(&func, OpCode::Bool).len(), 1);
    assert!(ops(&func, OpCode::CallRuntime).is_empty());
    let join = func
        .blocks
        .values()
        .find(|block| block.args.iter().any(|arg| arg.id == truth))
        .expect("join binds the original truth value");
    assert!(matches!(&join.terminator, Terminator::Return { values } if values == &[truth]));
    // Rerunning finds nothing: the slow arm's op is not reexpanded.
    let mut again = func.clone();
    assert_eq!(run(&mut again).values_changed, 1);
}

#[test]
fn truthiness_of_raw_scalars_keeps_its_lowering() {
    // Parameters travel as boxed words (the parameter ABI); values whose
    // producers make them raw carriers are not boxed words.
    for (opcode, ty, value) in [
        (OpCode::ConstBool, TirType::Bool, AttrValue::Bool(true)),
        (OpCode::ConstFloat, TirType::F64, AttrValue::Float(1.5)),
    ] {
        let mut func = TirFunction::new(
            "t".into(),
            vec![],
            TirType::Bool,
            molt_ir::FunctionReturnAbi::Value,
        );
        let scalar = func.fresh_value();
        let truth = func.fresh_value();
        func.value_types.insert(scalar, ty);
        func.value_types.insert(truth, TirType::Bool);
        let key = if opcode == OpCode::ConstFloat {
            "f_value"
        } else {
            "value"
        };
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            attrs: AttrDict::from([(key.into(), value)]),
            ..TirOp::new(opcode, vec![], vec![scalar])
        });
        block
            .ops
            .push(TirOp::new(OpCode::Bool, vec![scalar], vec![truth]));
        block.terminator = Terminator::Return {
            values: vec![truth],
        };
        assert_eq!(run(&mut func).values_changed, 0, "{opcode:?}");
        assert_eq!(func.blocks.len(), 1);
    }
}

#[test]
fn the_int_arm_compares_a_raw_payload_with_a_raw_zero() {
    let (mut func, _) = truth_of(TirType::DynBox);
    run(&mut func);
    let vr = crate::representation_facts::value_range_for(&func);
    let repr = crate::representation_facts::repr_by_value_for(&func, Some(&vr));
    let (_, ne) = &ops(&func, OpCode::Ne)[0];
    for operand in &ne.operands {
        assert_eq!(repr[operand], Repr::RawI64Safe);
    }
    assert_eq!(repr[&ne.results[0]], Repr::Bool);
}

#[test]
fn truthiness_after_an_add_reuses_the_int_tag_test() {
    // a + b, then bool(a): after cleanup, `a` is tag-tested once.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox; 2],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let sum = func.fresh_value();
    let truth = func.fresh_value();
    let tuple = func.fresh_value();
    func.value_types.insert(sum, TirType::DynBox);
    func.value_types.insert(truth, TirType::Bool);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp::new(
        OpCode::Add,
        vec![ValueId(0), ValueId(1)],
        vec![sum],
    ));
    block
        .ops
        .push(TirOp::new(OpCode::Bool, vec![ValueId(0)], vec![truth]));
    block.ops.push(TirOp::new(
        OpCode::BuildTuple,
        vec![sum, truth],
        vec![tuple],
    ));
    block.terminator = Terminator::Return {
        values: vec![tuple],
    };
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    assert!(crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti, None
    ));
    assert_valid(&func);
    let tests_of_a = ops(&func, OpCode::IsInlineInt)
        .into_iter()
        .filter(|(_, op)| op.operands == [ValueId(0)])
        .count();
    assert_eq!(tests_of_a, 1);
}

const COMPARISONS: [(OpCode, RuntimeFn); 6] = [
    (OpCode::Eq, RuntimeFn::Eq),
    (OpCode::Ne, RuntimeFn::Ne),
    (OpCode::Lt, RuntimeFn::Lt),
    (OpCode::Le, RuntimeFn::Le),
    (OpCode::Gt, RuntimeFn::Gt),
    (OpCode::Ge, RuntimeFn::Ge),
];

#[test]
fn a_boxed_comparison_compares_payloads_and_boxes_the_raw_bool() {
    for (opcode, runtime) in COMPARISONS {
        let (mut func, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        assert_eq!(run(&mut func).values_changed, 1, "{opcode:?}");
        assert_valid(&func);
        assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2);
        assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 2);
        // Only the fast arms compare in TIR; the slow arm calls the entry.
        let compares = ops(&func, opcode);
        assert_eq!(compares.len(), 2, "{opcode:?}");
        let int_arm = arm_block(&func, OpCode::UnboxInlineInt);
        let (fast, compare) = compares.iter().find(|(b, _)| *b == int_arm).unwrap();
        assert_eq!(func.value_types[&compare.results[0]], TirType::Bool);
        for operand in &compare.operands {
            assert_eq!(func.value_types[operand], TirType::I64);
        }
        let float_arm = arm_block(&func, OpCode::UnboxFloatWord);
        let (_, float_compare) = compares.iter().find(|(b, _)| *b == float_arm).unwrap();
        for operand in &float_compare.operands {
            assert_eq!(func.value_types[operand], TirType::F64);
        }
        let fast_ops: Vec<OpCode> = func.blocks[fast].ops.iter().map(|op| op.opcode).collect();
        assert_eq!(
            fast_ops,
            [
                OpCode::UnboxInlineInt,
                OpCode::UnboxInlineInt,
                opcode,
                OpCode::BoxVal
            ]
        );
        let calls = ops(&func, OpCode::CallRuntime);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1.runtime, Some(runtime));
        assert_eq!(calls[0].1.operands, [ValueId(0), ValueId(1)]);
    }
}

#[test]
fn a_comparison_with_a_literal_tests_only_the_boxed_operand() {
    // `n < 2`: the literal compares as it is and is boxed only for the entry.
    let (mut func, constant, _) = with_constant(OpCode::Lt, 2);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    let tests = ops(&func, OpCode::IsInlineInt);
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].1.operands, [ValueId(0)]);
    let (_, compare) = &ops(&func, OpCode::Lt)[0];
    assert_eq!(compare.operands[1], constant);
    let (_, call) = &ops(&func, OpCode::CallRuntime)[0];
    assert_eq!(call.runtime, Some(RuntimeFn::Lt));
    assert_eq!(call.operands[0], ValueId(0));
    assert_ne!(
        call.operands[1], constant,
        "the entry takes the boxed literal"
    );
}

#[test]
fn raw_operand_comparisons_keep_their_lowering() {
    // Parameters travel as boxed words; two raw producers are the backend's
    // raw compare, not a boxed fast path.
    for (opcode, _) in COMPARISONS {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let [lhs, rhs, result] = [func.fresh_value(), func.fresh_value(), func.fresh_value()];
        func.value_types.insert(lhs, TirType::I64);
        func.value_types.insert(rhs, TirType::I64);
        func.value_types.insert(result, TirType::DynBox);
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        for (value, c) in [(lhs, 3), (rhs, 4)] {
            block.ops.push(TirOp {
                attrs: AttrDict::from([("value".into(), AttrValue::Int(c))]),
                ..TirOp::new(OpCode::ConstInt, vec![], vec![value])
            });
        }
        block
            .ops
            .push(TirOp::new(opcode, vec![lhs, rhs], vec![result]));
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        assert_eq!(run(&mut func).values_changed, 0, "{opcode:?}");
        assert_eq!(func.blocks.len(), 1);
    }
}

#[test]
fn only_the_runtime_call_of_a_comparison_may_raise() {
    // The join's CheckException keeps the original op's declared source (a
    // comparison's is Unwind); any other op that could raise would mix a fresh
    // Origin into it, which the exception-region verifier rejects.
    for (opcode, _) in COMPARISONS {
        let (mut boxed, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        let (mut literal, _, _) = with_constant(opcode, 2);
        for func in [&mut boxed, &mut literal] {
            run(func);
            assert_valid(func);
        }
        let const_ints = std::collections::HashMap::new();
        for func in [&boxed, &literal] {
            for op in func.blocks.values().flat_map(|block| &block.ops) {
                if matches!(op.opcode, OpCode::CallRuntime | OpCode::ConstInt) {
                    continue;
                }
                assert!(
                    !crate::tir::passes::check_exception_elim::classify::op_may_raise(
                        &func.value_types,
                        &const_ints,
                        op
                    ),
                    "{opcode:?}: {:?} may raise",
                    op.opcode
                );
            }
        }
    }
}

#[test]
fn the_terminal_phase_keeps_the_expanded_results_type() {
    // `return x + 1` with x an int parameter (a boxed word, typed I64). The
    // terminal phase refines types, then expands; the join must keep I64, or
    // every use downstream (a loop-carried counter, `n == 0`) widens to DynBox.
    let (mut func, _, result) = with_constant(OpCode::Add, 1);
    func.param_types = vec![TirType::I64];
    func.value_types.insert(ValueId(0), TirType::I64);
    func.value_types.insert(result, TirType::I64);
    func.return_type = TirType::I64;
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    crate::tir::drop_phase::finalize_function_drops(&mut func, &tti, None);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 1, "the add expands");
    assert_eq!(func.value_types[&result], TirType::I64);
}

/// `return f(a)`: a guarded call of the compiled function `target` through the
/// callable in parameter 0, adopting its operands when `adopts`.
fn guarded_call(adopts: bool) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "caller".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let mut call = TirOp::new(OpCode::Call, vec![ValueId(0), ValueId(1)], vec![result]);
    call.origin = Some(molt_ir::OpKind::CallGuarded);
    call.attrs
        .insert("s_value".into(), AttrValue::Str("target".into()));
    if adopts {
        call.set_argument_custody(&[crate::ir::ParameterCustody::Transferred; 2]);
    }
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(call);
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    (func, result)
}

fn targets(arity: usize, has_closure: bool) -> GuardedTargets {
    GuardedTargets::from([(
        "target".to_string(),
        super::CalleeFacts { arity, has_closure },
    )])
}

#[test]
fn a_guarded_call_admits_then_calls_its_target_directly() {
    for adopts in [false, true] {
        let (mut func, _) = guarded_call(adopts);
        assert_eq!(super::run(&mut func, &targets(1, false)).values_changed, 1);
        assert_valid(&func);
        let runtimes: Vec<_> = ops(&func, OpCode::CallRuntime)
            .into_iter()
            .map(|(_, op)| op.runtime.unwrap())
            .collect();
        for expected in [
            RuntimeFn::FunctionDirectCallEligible,
            RuntimeFn::RecursionEnterFast,
            RuntimeFn::RecursionExitFast,
            RuntimeFn::RaiseRecursionError,
        ] {
            assert_eq!(
                runtimes.iter().filter(|&&r| r == expected).count(),
                1,
                "{expected:?}"
            );
        }
        // Admission passes the supplied count and the adoption bit as raw words.
        let (entry, eligible) = ops(&func, OpCode::CallRuntime)
            .into_iter()
            .find(|(_, op)| op.runtime == Some(RuntimeFn::FunctionDirectCallEligible))
            .unwrap();
        let constant = |value: ValueId| {
            func.blocks[&entry]
                .ops
                .iter()
                .find(|op| op.results == [value])
                .and_then(|op| match op.attrs.get("value") {
                    Some(AttrValue::Int(v)) => Some(*v),
                    _ => None,
                })
        };
        assert_eq!(eligible.operands[0], ValueId(0));
        assert_eq!(constant(eligible.operands[1]), Some(1));
        assert_eq!(
            constant(eligible.operands[2]),
            Some(if adopts { 0b10 } else { 0 })
        );
        // The direct call keeps the arguments' custody and borrows the callable,
        // so the callable outlives the call.
        let (_, direct) = &ops(&func, OpCode::DirectCall)[0];
        assert_eq!(direct.operands, [ValueId(0), ValueId(1)]);
        assert_eq!(
            direct.operand_custody(0),
            crate::ir::ParameterCustody::Borrowed
        );
        assert_eq!(
            direct.operand_custody(1),
            if adopts {
                crate::ir::ParameterCustody::Transferred
            } else {
                crate::ir::ParameterCustody::Borrowed
            }
        );
        // The slow arm is the original guarded call, unchanged.
        let slow = ops(&func, OpCode::Call);
        assert_eq!(slow.len(), 1);
        assert_eq!(slow[0].1.origin, Some(molt_ir::OpKind::CallGuarded));
        assert_eq!(
            slow[0].1.operand_custody(0) == crate::ir::ParameterCustody::Transferred,
            adopts
        );
        assert_eq!(ops(&func, OpCode::CodeIs).len(), 1);
    }
}

#[test]
fn guarded_call_admission_cannot_raise() {
    // Only the direct call, the slow call and the RecursionError raise may
    // raise; the join's check keeps the call's own exception source.
    let (mut func, _) = guarded_call(true);
    super::run(&mut func, &targets(1, false));
    let const_ints = std::collections::HashMap::new();
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        let raises = matches!(op.opcode, OpCode::Call | OpCode::DirectCall)
            || op.runtime == Some(RuntimeFn::RaiseRecursionError);
        assert_eq!(
            crate::tir::passes::check_exception_elim::classify::op_may_raise(
                &func.value_types,
                &const_ints,
                op
            ),
            raises,
            "{:?} {:?}",
            op.opcode,
            op.runtime
        );
    }
}

#[test]
fn a_closure_target_or_an_arity_mismatch_keeps_the_runtime_lane() {
    for facts in [targets(1, true), targets(2, false), GuardedTargets::new()] {
        let (mut func, _) = guarded_call(true);
        assert_eq!(super::run(&mut func, &facts).values_changed, 0);
        assert_eq!(func.blocks.len(), 1);
    }
}

#[test]
#[should_panic(expected = "has no source signature")]
fn a_guarded_target_without_a_signature_is_a_compiler_error() {
    let (func, _) = guarded_call(true);
    let table = molt_ir::linkage_abi::FunctionSignatures::new();
    super::guarded_targets(&func, Some(&table));
}

/// `i = 0; while i < 100: total = total + i; i = i + 1; return total`, with
/// `total` a boxed parameter. Returns the function, the induction phi and its
/// update.
fn counted_loop_with_boxed_accumulator() -> (TirFunction, ValueId, ValueId) {
    counted_loop(false)
}

/// The same loop. With `exact_accumulator`, `total` starts at the literal 0,
/// so it is an exact int of unknown size (a boxed word that cannot raise in
/// `+`), and the statement after the sum forwards an existing exception: an
/// Unwind check, which admits no new source in front of it.
fn counted_loop(exact_accumulator: bool) -> (TirFunction, ValueId, ValueId) {
    use crate::tir::blocks::TirBlock;
    use crate::tir::values::TirValue;
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let int = |func: &mut TirFunction| {
        let value = func.fresh_value();
        func.value_types.insert(value, TirType::I64);
        value
    };
    let (zero, one, stop) = (int(&mut func), int(&mut func), int(&mut func));
    let (i, next_i) = (int(&mut func), int(&mut func));
    let total = func.fresh_value();
    let sum = func.fresh_value();
    let cond = func.fresh_value();
    let accumulator = if exact_accumulator {
        TirType::I64
    } else {
        TirType::DynBox
    };
    func.value_types.insert(total, accumulator.clone());
    func.value_types.insert(sum, accumulator.clone());
    func.value_types.insert(cond, TirType::Bool);
    let (header, guard, body, exit) = (
        func.fresh_block(),
        func.fresh_block(),
        func.fresh_block(),
        func.fresh_block(),
    );
    let constant = |value: i64, result: ValueId| TirOp {
        attrs: AttrDict::from([("value".into(), AttrValue::Int(value))]),
        ..TirOp::new(OpCode::ConstInt, vec![], vec![result])
    };
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops = vec![constant(0, zero), constant(1, one), constant(100, stop)];
    block.terminator = Terminator::Branch {
        target: header,
        args: vec![zero, if exact_accumulator { zero } else { ValueId(0) }],
    };
    let mut body_ops = vec![
        TirOp::new(OpCode::Add, vec![total, i], vec![sum]),
        TirOp::new(OpCode::Add, vec![i, one], vec![next_i]),
    ];
    if exact_accumulator {
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 9);
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let mut forward = TirOp::new(OpCode::CheckException, vec![], vec![]);
        forward.attrs.insert("value".into(), AttrValue::Int(9));
        forward.set_exception_edge_kind(crate::tir::op_kinds_generated::ExceptionEdgeKind::Unwind);
        body_ops.insert(1, forward);
    }
    let mut insert = |id: BlockId, args: Vec<TirValue>, ops: Vec<TirOp>, terminator| {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args,
                ops,
                terminator,
            },
        );
    };
    insert(
        header,
        vec![
            TirValue {
                id: i,
                ty: TirType::I64,
            },
            TirValue {
                id: total,
                ty: accumulator,
            },
        ],
        vec![],
        Terminator::Branch {
            target: guard,
            args: vec![],
        },
    );
    insert(
        guard,
        vec![],
        vec![TirOp::new(OpCode::Lt, vec![i, stop], vec![cond])],
        Terminator::CondBranch {
            cond,
            then_block: body,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        },
    );
    insert(
        body,
        vec![],
        body_ops,
        Terminator::Branch {
            target: header,
            args: vec![next_i, sum],
        },
    );
    insert(
        exit,
        vec![],
        vec![],
        Terminator::Return {
            values: vec![total],
        },
    );
    (func, i, next_i)
}

fn plan(func: &TirFunction) -> std::collections::HashMap<ValueId, Repr> {
    let vr = crate::representation_facts::value_range_for(func);
    crate::representation_facts::repr_by_value_for(func, Some(&vr))
}

/// The stage splits the loop body. The plan a backend builds afterwards must
/// still carry the induction variable raw: before this proof travelled on the
/// op, the split hid the loop from the range analysis and the variable fell
/// back to a boxed word (a runtime call for `i < n` and `i + 1` on LLVM).
#[test]
fn expansion_keeps_a_loop_induction_variable_raw() {
    let (mut func, i, next_i) = counted_loop_with_boxed_accumulator();
    let before = plan(&func);
    assert_eq!(before[&i], Repr::RawI64Safe, "the loop is recognized");
    assert_eq!(before[&next_i], Repr::RawI64Safe);

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 1, "`total + i` expands");
    assert_valid(&func);
    let after = plan(&func);
    assert_eq!(after[&i], Repr::RawI64Safe);
    assert_eq!(after[&next_i], Repr::RawI64Safe);
    for (value, repr) in &before {
        if repr.is_raw_i64_carrier() {
            assert_eq!(after.get(value), Some(repr), "{value:?} lost its carrier");
        }
    }

    // The proof on the op is what carries it: without the mark, the same
    // function plans the variable as a possible big int.
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            op.attrs.remove(crate::tir::ops::INLINE_INT_PROOF_ATTR);
        }
    }
    assert_eq!(plan(&func)[&next_i], Repr::MaybeBigInt);
}

#[test]
fn a_function_without_eligible_ops_carries_no_proof_marks() {
    let (mut func, _) = binary(OpCode::Add, [TirType::I64, TirType::I64]);
    run(&mut func);
    assert!(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .all(|op| !op.has_inline_int_proof())
    );
}

/// An add of two exact ints cannot raise, so no check follows it. The ops
/// that replace it box a sum and call the runtime; the op tables declare both
/// as raising. Unmarked, each is a new exception source in front of a check
/// that only forwards an existing one, which the region verifier rejects (the
/// LLVM build failure of a `total += i * j` loop).
#[test]
fn replacing_an_op_that_cannot_raise_adds_no_exception_source() {
    let (mut func, _, _) = counted_loop(true);
    crate::tir::exception_regions::verify_exception_regions(&func)
        .expect("the fixture is valid before the stage");
    assert_eq!(run(&mut func).values_changed, 1, "`total + i` expands");
    assert_valid(&func);
    let replaced: Vec<TirOp> = [OpCode::BoxVal, OpCode::CallRuntime]
        .into_iter()
        .flat_map(|opcode| ops(&func, opcode))
        .map(|(_, op)| op)
        .collect();
    assert_eq!(replaced.len(), 2);
    for op in &replaced {
        assert!(
            !crate::tir::passes::effects::op_may_throw_with_types(op, &func.value_types),
            "{:?} replaces an add that cannot raise",
            op.opcode
        );
    }

    // The marks are what satisfies the verifier.
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            op.attrs.remove(crate::tir::ops::NOTHROW_PROOF_ATTR);
        }
    }
    assert!(crate::tir::exception_regions::verify_exception_regions(&func).is_err());
}

#[test]
fn replacing_an_op_that_may_raise_keeps_its_exception_source() {
    let (mut func, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    run(&mut func);
    for opcode in [OpCode::BoxVal, OpCode::CallRuntime] {
        let (_, op) = &ops(&func, opcode)[0];
        assert!(!op.has_nothrow_proof());
        assert!(crate::tir::passes::effects::op_may_throw_with_types(
            op,
            &func.value_types
        ));
    }
}

/// `total < 100` where `total` is an exact int of unknown size (a boxed
/// word) and the result is planned as a raw bool. No backend lane owns that
/// shape, so before this case the op called the runtime and then asked the
/// runtime for the truth of its result, on every iteration.
#[test]
fn a_comparison_with_a_raw_bool_result_joins_two_raw_bools() {
    let (mut func, _, _) = counted_loop(true);
    let body = func
        .blocks
        .iter()
        .find(|(_, block)| block.ops.iter().any(|op| op.opcode == OpCode::Add))
        .map(|(id, _)| *id)
        .unwrap();
    let total = func.blocks[&body].ops[0].operands[0];
    let stop = func.blocks.values().find_map(|block| {
        block
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Lt)
            .map(|op| op.operands[1])
    });
    let less = func.fresh_value();
    func.value_types.insert(less, TirType::Bool);
    func.blocks.get_mut(&body).unwrap().ops.push(TirOp::new(
        OpCode::Lt,
        vec![total, stop.unwrap()],
        vec![less],
    ));
    let before = plan(&func);
    assert_eq!(before[&total], Repr::MaybeBigInt);
    assert_eq!(before[&less], Repr::Bool);

    assert_eq!(run(&mut func).values_changed, 2, "the add and the compare");
    assert_valid(&func);
    assert_eq!(plan(&func)[&less], Repr::Bool);
    // The compare's result is the argument of a join that both arms feed raw.
    let join = func
        .blocks
        .values()
        .find(|block| block.args.iter().any(|arg| arg.id == less))
        .expect("the join binds the compare's result");
    assert_eq!(join.args[0].ty, TirType::Bool);
    let feeders: Vec<&TirOp> = func
        .blocks
        .values()
        .filter(|block| {
            matches!(&block.terminator, Terminator::Branch { target, .. } if *target == join.id)
        })
        .map(|block| block.ops.last().unwrap())
        .collect();
    let mut opcodes: Vec<OpCode> = feeders.iter().map(|op| op.opcode).collect();
    opcodes.sort_by_key(|opcode| format!("{opcode:?}"));
    assert_eq!(opcodes, [OpCode::Lt, OpCode::UnboxBoolWord]);
    // The fast arm compares payloads; nothing boxes a bool.
    let fast = feeders.iter().find(|op| op.opcode == OpCode::Lt).unwrap();
    let plan_after = plan(&func);
    assert_eq!(plan_after[&fast.operands[0]], Repr::RawI64Safe);
    assert!(ops(&func, OpCode::BoxVal).len() == 1, "only the add boxes");
}

/// An add of two exact ints is an exact int. After expansion the result is a
/// join of a boxed sum and a runtime call, whose contract says "an object".
/// The call carries the op's exact type, so the join and the loop phi it
/// feeds stay exact; without it every later fact about the accumulator (a
/// raw bool from `total < n`, a non-raising add) is lost.
#[test]
fn expansion_keeps_the_exact_type_of_the_result_it_joins() {
    let (mut func, _, _) = counted_loop(true);
    let exact = |func: &TirFunction| crate::tir::type_refine::extract_exact_scalar_map(func);
    let sum = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::Add && op.results.len() == 1)
        .map(|op| op.results[0])
        .unwrap();
    let before = exact(&func);
    let sum_type = before[&sum].clone();
    assert!(matches!(sum_type, TirType::I64 | TirType::BigInt));

    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    let after = exact(&func);
    for (value, ty) in &before {
        if matches!(
            ty,
            TirType::I64 | TirType::BigInt | TirType::Bool | TirType::F64
        ) {
            assert_eq!(
                after.get(value),
                Some(ty),
                "{value:?} lost its exact type (sum is {sum:?}): {}",
                crate::tir::printer::print_function(&func)
            );
        }
    }

    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            op.attrs.remove(crate::tir::ops::EXACT_RESULT_PROOF_ATTR);
        }
    }
    assert_eq!(
        exact(&func).get(&sum),
        None,
        "unmarked, the join is no exact scalar"
    );
}

/// `x * y` on boxed words: the int arm multiplies the payloads with an
/// overflow test and leaves for the runtime on overflow; the float arm
/// multiplies the floats. The product of two inline payloads is planned as a
/// full-range raw int and boxes through the overflow-safe path.
#[test]
fn boxed_mul_checks_overflow_in_its_int_arm() {
    let (mut func, _) = binary(OpCode::Mul, [TirType::DynBox, TirType::DynBox]);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2);
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 2);
    let checked = ops(&func, OpCode::CheckedMul);
    assert_eq!(checked.len(), 1);
    let (int_arm, mul) = &checked[0];
    let [product, overflow] = mul.results[..] else {
        panic!("CheckedMul yields a value and a flag");
    };
    let slow = ops(&func, OpCode::CallRuntime)[0].0;
    assert_eq!(
        ops(&func, OpCode::CallRuntime)[0].1.runtime,
        Some(RuntimeFn::Mul)
    );
    let Terminator::CondBranch {
        cond,
        then_block,
        else_block,
        ..
    } = &func.blocks[int_arm].terminator
    else {
        panic!("the int arm branches on overflow");
    };
    assert_eq!((*cond, *then_block), (overflow, slow));
    assert_eq!(block_opcodes(&func, *else_block), [OpCode::BoxVal]);
    assert_eq!(plan(&func)[&product], Repr::RawI64FullDeopt);
    assert_eq!(ops(&func, OpCode::Mul).len(), 1, "the float arm's product");
}

/// An operand proven an exact int takes no float arm, and one proven an exact
/// float takes no int arm; a raw float boxes for the slow arm with BoxVal.
#[test]
fn exact_operand_kinds_select_the_arms() {
    let (mut func, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let float_const = func.fresh_value();
    func.value_types.insert(float_const, TirType::F64);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.insert(
        0,
        TirOp {
            attrs: AttrDict::from([("f_value".into(), AttrValue::Float(0.5))]),
            ..TirOp::new(OpCode::ConstFloat, vec![], vec![float_const])
        },
    );
    block.ops[1].operands[1] = float_const;
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert!(
        ops(&func, OpCode::IsInlineInt).is_empty(),
        "x + 0.5 has no int arm"
    );
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 1, "only x is tested");
    let slow = ops(&func, OpCode::CallRuntime)[0].0;
    assert_eq!(
        block_opcodes(&func, slow),
        [OpCode::BoxVal, OpCode::CallRuntime],
        "the raw float boxes for the runtime entry"
    );
}
