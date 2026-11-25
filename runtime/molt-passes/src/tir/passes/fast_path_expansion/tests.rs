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
    // Four arms, each two tests and its block: int+int, float+float,
    // float+int, int+float; then the slow arm and the join.
    assert_eq!(func.blocks.len(), 14);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 4);
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 4);
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
    // Each mixed arm converts its int payload exactly, then adds floats.
    let converts = ops(&func, OpCode::IntToFloat);
    assert_eq!(converts.len(), 2);
    for (arm, convert) in &converts {
        assert_eq!(func.value_types[&convert.results[0]], TirType::F64);
        let (_, add) = ops(&func, OpCode::Add)
            .into_iter()
            .find(|(block, _)| block == arm)
            .expect("the mixed arm adds");
        assert!(add.operands.contains(&convert.results[0]));
        for operand in &add.operands {
            assert_eq!(func.value_types[operand], TirType::F64);
        }
    }
    assert_eq!(ops(&func, OpCode::Add).len(), 4, "one add per fast arm");
    assert!(
        ops(&func, OpCode::Add)
            .iter()
            .all(|(block, _)| *block != slow_block(&func))
    );
}

fn slow_block(func: &TirFunction) -> BlockId {
    ops(func, OpCode::CallRuntime)[0].0
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
        // One payload op per fast arm: int, float and the two mixed arms.
        assert_eq!(ops(&func, fast).len(), 4, "{opcode:?}");
        assert!(
            ops(&func, opcode).len() <= if opcode == fast { 4 } else { 0 },
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
    op.attrs = AttrDict::from_iter([("marker".into(), AttrValue::Int(7))]);
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
        attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(c))]),
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
fn identity_follows_a_forwarded_value_in_cold_and_hot_code() {
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    for name in ["f", "py.app.__molt_module_chunk_1"] {
        for (predicate, expected) in [(OpCode::Is, true), (OpCode::IsNot, false)] {
            let (mut func, _) = binary(predicate, [TirType::I64, TirType::I64]);
            func.name = name.into();
            assert!(!crate::tir::pass_manager::run_late_lowering(
                &mut func, &tti, None
            ));
            let alias = func.fresh_value();
            func.value_types.insert(alias, TirType::I64);
            let block = func.blocks.get_mut(&func.entry_block).unwrap();
            block.ops[0].operands[1] = alias;
            block
                .ops
                .insert(0, TirOp::new(OpCode::Copy, vec![ValueId(0)], vec![alias]));
            assert!(crate::tir::pass_manager::run_late_lowering(
                &mut func, &tti, None
            ));
            let folded = &ops(&func, OpCode::ConstBool)[0].1;
            assert_eq!(folded.attrs.get("value"), Some(&AttrValue::Bool(expected)));
            assert_valid(&func);
        }
    }
}

#[test]
fn a_function_without_eligible_ops_is_left_untouched() {
    let (mut func, _) = binary(OpCode::Pow, [TirType::DynBox, TirType::DynBox]);
    let before = crate::tir::printer::print_function(&func);
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    assert!(!crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti, None
    ));
    assert_eq!(crate::tir::printer::print_function(&func), before);
}

#[test]
fn a_cold_module_chunk_calls_the_runtime_instead_of_inlining_fast_paths() {
    // `CodeTemperature`: a module-body chunk runs once, so its Add keeps the
    // runtime call; the same body in an ordinary function expands.
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    let (mut hot, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    assert!(crate::tir::pass_manager::run_late_lowering(
        &mut hot, &tti, None
    ));
    let (mut cold, _) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    cold.name = "py.app.__molt_module_chunk_1".into();
    let before = crate::tir::printer::print_function(&cold);
    assert!(!crate::tir::pass_manager::run_late_lowering(
        &mut cold, &tti, None
    ));
    assert_eq!(crate::tir::printer::print_function(&cold), before);
}

#[test]
fn cold_call_legalization_honors_the_compiled_target_without_warming_the_block() {
    let (mut func, result) = guarded_call(false);
    func.name = "py.app.__molt_module_chunk_1".into();
    let entry = func.entry_block;
    let sum = func.fresh_value();
    let block = func.blocks.get_mut(&entry).unwrap();
    block
        .ops
        .push(TirOp::new(OpCode::Add, vec![result, ValueId(1)], vec![sum]));
    block.terminator = Terminator::Return { values: vec![sum] };

    let linkage = targets(1, false);
    assert_eq!(super::run(&mut func, &linkage).values_changed, 0);
    func.blocks.get_mut(&entry).unwrap().ops[0].set_proven_target(true);
    assert_eq!(super::run(&mut func, &linkage).values_changed, 1);
    assert_eq!(ops(&func, OpCode::DirectCall).len(), 1);
    assert_eq!(ops(&func, OpCode::Add).len(), 1);
    assert!(ops(&func, OpCode::Call).is_empty());
    assert_valid(&func);
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
            attrs: AttrDict::from_iter([(key.into(), value)]),
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
        assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 4);
        assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 4);
        // Only the fast arms compare in TIR; the slow arm calls the entry.
        // A mixed comparison compares the exact f64 of its int payload.
        let compares = ops(&func, opcode);
        assert_eq!(compares.len(), 4, "{opcode:?}");
        assert_eq!(ops(&func, OpCode::IntToFloat).len(), 2, "{opcode:?}");
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
                attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(c))]),
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
        for func in [&boxed, &literal] {
            let raise = crate::tir::passes::check_exception_elim::classify::RaiseFacts::of(func);
            for op in func.blocks.values().flat_map(|block| &block.ops) {
                if matches!(op.opcode, OpCode::CallRuntime | OpCode::ConstInt) {
                    continue;
                }
                assert!(
                    !raise.op_may_raise(op),
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
    call.callee = Some(molt_ir::FunctionId(0));
    call.origin = Some(molt_ir::OpKind::CallGuarded);
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
        molt_ir::FunctionId(0),
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
        assert_eq!(
            runtimes,
            [
                RuntimeFn::FunctionDirectCallEligible,
                RuntimeFn::RaiseRecursionError,
            ],
            "the recursion charge makes no runtime call"
        );
        assert_recursion_charge_brackets_the_direct_call(&func);
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
    let raise = crate::tir::passes::check_exception_elim::classify::RaiseFacts::of(&func);
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        let raises = matches!(op.opcode, OpCode::Call | OpCode::DirectCall)
            || op.runtime == Some(RuntimeFn::RaiseRecursionError);
        assert_eq!(
            raise.op_may_raise(op),
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
#[should_panic(expected = "outside its document")]
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
        attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(value))]),
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

fn plan(func: &TirFunction) -> molt_ir::deterministic_hash::HashMap<ValueId, Repr> {
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

    // The expansion's guard diamond sits inside the loop body. The counted-loop
    // recurrence holds on a branching body, so even without the proof marks
    // the variable keeps its raw carrier.
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            op.attrs.remove(crate::tir::ops::INLINE_INT_PROOF_ATTR);
        }
    }
    assert_eq!(plan(&func)[&next_i], Repr::RawI64Safe);
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
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 4);
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 4);
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
    assert_eq!(
        ops(&func, OpCode::Mul).len(),
        3,
        "the float arm's product and the two mixed arms'"
    );
}

/// A raw float operand takes only arms that read it as a float: `x + 0.5`
/// tests `x` as a float, then as an int whose exact f64 it adds. A raw float
/// boxes for the slow arm with BoxVal.
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
            attrs: AttrDict::from_iter([("f_value".into(), AttrValue::Float(0.5))]),
            ..TirOp::new(OpCode::ConstFloat, vec![], vec![float_const])
        },
    );
    block.ops[1].operands[1] = float_const;
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsFloatWord).len(), 1, "only x is tested");
    assert_eq!(
        ops(&func, OpCode::IsInlineInt).len(),
        1,
        "x as an int, once"
    );
    assert_eq!(ops(&func, OpCode::IntToFloat).len(), 1, "and converted");
    assert!(
        ops(&func, OpCode::UnboxInlineInt)
            .iter()
            .all(|(block, _)| ops(&func, OpCode::IntToFloat)[0].0 == *block),
        "no int arm adds two ints"
    );
    let slow = ops(&func, OpCode::CallRuntime)[0].0;
    assert_eq!(
        block_opcodes(&func, slow),
        [OpCode::BoxVal, OpCode::CallRuntime],
        "the raw float boxes for the runtime entry"
    );
}

#[test]
fn an_exact_str_operand_takes_no_numeric_arm() {
    // `"%d" % n` and `s + t`: a str word is never an inline int or a float,
    // so the op keeps its runtime entry with no tag test in front of it.
    for opcode in [OpCode::Mod, OpCode::Add] {
        let (mut func, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        let text = func.fresh_value();
        func.value_types.insert(text, TirType::Str);
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.insert(
            0,
            TirOp {
                attrs: AttrDict::from_iter([("s_value".into(), AttrValue::Str("%d".into()))]),
                ..TirOp::new(OpCode::ConstStr, vec![], vec![text])
            },
        );
        block.ops[1].operands[0] = text;
        let before = crate::tir::printer::print_function(&func);
        assert_eq!(run(&mut func).values_changed, 0, "{opcode:?}");
        assert_eq!(crate::tir::printer::print_function(&func), before);
    }
}

/// `x / y` on boxed words divides in F64 on every arm: two inline ints
/// convert exactly (CPython's quotient below 2**53), a mixed pair converts
/// its int, and F64 division raises on a zero divisor. The slow arm calls
/// the true-division entry.
#[test]
fn boxed_true_division_divides_floats_on_every_arm() {
    let (mut func, _) = binary(OpCode::Div, [TirType::DynBox, TirType::DynBox]);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    let divides = ops(&func, OpCode::Div);
    assert_eq!(divides.len(), 4, "one division per fast arm");
    for (_, divide) in &divides {
        for operand in &divide.operands {
            assert_eq!(func.value_types[operand], TirType::F64);
        }
    }
    assert_eq!(
        ops(&func, OpCode::IntToFloat).len(),
        4,
        "two in the int arm, one in each mixed arm"
    );
    let slow = ops(&func, OpCode::CallRuntime)[0].0;
    assert_eq!(
        ops(&func, OpCode::CallRuntime)[0].1.runtime,
        Some(RuntimeFn::Div)
    );
    // Two ints divided by zero raise the int message: the int arm tests its
    // divisor payload and leaves for the runtime on zero, before converting.
    let tests = ops(&func, OpCode::Ne);
    assert_eq!(tests.len(), 1, "only the int arm tests its divisor");
    let (int_arm, test) = &tests[0];
    assert_eq!(func.value_types[&test.operands[0]], TirType::I64);
    let Terminator::CondBranch { else_block, .. } = &func.blocks[int_arm].terminator else {
        panic!("the int arm branches on its divisor");
    };
    assert_eq!(*else_block, slow);
}

#[test]
fn boxed_floor_division_and_modulo_guard_a_zero_int_divisor() {
    // `//` has the int arm only; `%` has the int arm and the three float arms.
    for (opcode, runtime, arms) in [
        (OpCode::FloorDiv, RuntimeFn::Floordiv, 1),
        (OpCode::Mod, RuntimeFn::Mod, 4),
    ] {
        let (mut func, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        assert_eq!(run(&mut func).values_changed, 1, "{opcode:?}");
        assert_valid(&func);
        let raw = ops(&func, opcode);
        assert_eq!(raw.len(), arms, "{opcode:?}: one raw op per fast arm");
        let int_ops: Vec<_> = raw
            .iter()
            .filter(|(_, op)| func.value_types[&op.operands[0]] == TirType::I64)
            .collect();
        assert_eq!(int_ops.len(), 1, "{opcode:?}: one int arm");
        for (_, op) in &raw {
            let ty = &func.value_types[&op.operands[0]];
            assert_eq!(&func.value_types[&op.operands[1]], ty, "{opcode:?}");
        }
        assert_eq!(
            ops(&func, OpCode::IsFloatWord).is_empty(),
            opcode == OpCode::FloorDiv,
            "{opcode:?}: floor division has no float arm"
        );
        let calls = ops(&func, OpCode::CallRuntime);
        assert_eq!(calls.len(), 1, "{opcode:?}");
        assert_eq!(calls[0].1.runtime, Some(runtime), "{opcode:?}");
        // The int arm tests its divisor payload against zero and leaves for
        // the runtime, which raises the int message, before dividing.
        let tests = ops(&func, OpCode::Ne);
        assert_eq!(tests.len(), 1, "{opcode:?}: only the int arm tests");
        let (guard, test) = &tests[0];
        assert_eq!(func.value_types[&test.operands[0]], TirType::I64);
        let Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } = &func.blocks[guard].terminator
        else {
            panic!("{opcode:?}: the int arm branches on its divisor");
        };
        assert_eq!(*else_block, calls[0].0, "{opcode:?}: zero goes slow");
        assert_eq!(*then_block, int_ops[0].0, "{opcode:?}: nonzero divides");
    }
}

#[test]
fn boxed_bitwise_ops_take_only_the_inline_int_arm() {
    // 2026-11-13: `i & 63` on a boxed loop index called molt_bit_and on every
    // iteration (about 12% of an isinstance loop's samples, LLVM).
    for (opcode, runtime) in [
        (OpCode::BitAnd, RuntimeFn::BitAnd),
        (OpCode::BitOr, RuntimeFn::BitOr),
        (OpCode::BitXor, RuntimeFn::BitXor),
    ] {
        let (mut func, _) = binary(opcode, [TirType::DynBox, TirType::DynBox]);
        assert_eq!(run(&mut func).values_changed, 1, "{opcode:?}");
        assert_valid(&func);
        // One arm: int with int. A float operand raises TypeError, so no float
        // arm exists and every other word takes the runtime entry.
        assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2, "{opcode:?}");
        assert!(ops(&func, OpCode::IsFloatWord).is_empty(), "{opcode:?}");
        assert!(ops(&func, OpCode::IntToFloat).is_empty(), "{opcode:?}");
        let fast = arm_block(&func, OpCode::UnboxInlineInt);
        assert_eq!(
            block_opcodes(&func, fast),
            [
                OpCode::UnboxInlineInt,
                OpCode::UnboxInlineInt,
                opcode,
                OpCode::BoxVal
            ],
            "{opcode:?}"
        );
        let raw = ops(&func, opcode)
            .into_iter()
            .find(|(block, _)| *block == fast)
            .unwrap()
            .1;
        for operand in &raw.operands {
            assert_eq!(func.value_types[operand], TirType::I64, "{opcode:?}");
        }
        let calls = ops(&func, OpCode::CallRuntime);
        assert_eq!(calls.len(), 1, "{opcode:?}");
        assert_eq!(calls[0].1.runtime, Some(runtime), "{opcode:?}");
    }
}

/// A proven guarded call (HF-846 slice 2) enters its target directly: no
/// admission call, no code guard, no slow call. The recursion check stays.
#[test]
fn a_proven_call_skips_admission_and_the_code_guard() {
    let (mut func, result) = guarded_call(true);
    let entry = func.entry_block;
    func.blocks.get_mut(&entry).unwrap().ops[0].set_proven_target(true);
    assert_eq!(super::run(&mut func, &targets(1, false)).values_changed, 1);
    assert_valid(&func);
    let runtimes: Vec<_> = ops(&func, OpCode::CallRuntime)
        .into_iter()
        .map(|(_, op)| op.runtime.unwrap())
        .collect();
    assert_eq!(runtimes, [RuntimeFn::RaiseRecursionError]);
    assert_recursion_charge_brackets_the_direct_call(&func);
    assert!(ops(&func, OpCode::CodeIs).is_empty(), "no code guard");
    assert!(
        ops(&func, OpCode::Call).is_empty(),
        "no slow call: the callee is proven"
    );
    let direct = ops(&func, OpCode::DirectCall);
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].1.operands, [ValueId(0), ValueId(1)]);
    let _ = result;
}

#[test]
fn recursion_refusal_does_not_feed_the_proven_call_result() {
    use crate::tir::op_kinds_generated::ExceptionEdgeKind;

    let (mut func, result) = guarded_call(true);
    let entry = func.entry_block;
    let handler = func.fresh_block();
    let label = 20;
    func.label_id_map.insert(handler.0, label);
    func.blocks.insert(
        handler,
        super::new_block(handler, vec![], Terminator::Return { values: vec![] }),
    );
    let body = func.blocks.get_mut(&entry).unwrap();
    body.ops[0].set_proven_target(true);
    assert!(body.ops[0].mark_exact_result_proof(&TirType::I64));
    let mut check = TirOp::new(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
    body.ops.push(check);

    assert_eq!(super::run(&mut func, &targets(1, false)).values_changed, 1);
    assert_valid(&func);
    let refusal = ops(&func, OpCode::CallRuntime)
        .into_iter()
        .find(|(_, op)| op.runtime == Some(RuntimeFn::RaiseRecursionError))
        .unwrap()
        .0;
    let failure = &func.blocks[&refusal];
    assert_eq!(failure.ops[1].opcode, OpCode::CheckException);
    assert!(matches!(failure.terminator, Terminator::Unwind));
    assert_eq!(
        crate::tir::type_refine::extract_exact_scalar_map(&func).get(&result),
        Some(&TirType::I64)
    );
}

/// A proven call of a closure target (design 78 5.10) passes the callee's own
/// closure tuple, read live from the function object, after the callable:
/// the place the target's entry takes its environment. An unproven closure
/// call keeps the runtime lane (`a_closure_target_or_an_arity_mismatch_...`).
#[test]
fn a_proven_closure_call_passes_its_function_s_closure_tuple() {
    use crate::ir::ParameterCustody::{Borrowed, Transferred};
    let (mut func, _) = guarded_call(true);
    let entry = func.entry_block;
    func.blocks.get_mut(&entry).unwrap().ops[0].set_proven_target(true);
    // Arity 2: the environment and the one Python argument.
    assert_eq!(super::run(&mut func, &targets(2, true)).values_changed, 1);
    assert_valid(&func);
    let direct = ops(&func, OpCode::DirectCall);
    assert_eq!(direct.len(), 1);
    let (block, call) = &direct[0];
    let body: Vec<OpCode> = func.blocks[block].ops.iter().map(|op| op.opcode).collect();
    assert_eq!(
        body,
        [
            OpCode::RecursionCount,
            OpCode::FunctionClosure,
            OpCode::DirectCall,
            OpCode::RecursionRelease
        ]
    );
    let closure = &func.blocks[block].ops[1];
    assert_eq!(closure.operands, [ValueId(0)], "the callee's own closure");
    assert_eq!(call.operands, [ValueId(0), closure.results[0], ValueId(1)]);
    assert_eq!(
        call.argument_custody().unwrap(),
        [Borrowed, Borrowed, Transferred],
        "the callable and its environment are borrowed; the argument keeps its custody"
    );
    // The tuple is the function's: nothing releases the view.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(&func);
    assert!(
        !crate::tir::passes::ownership_lattice_min::OwnershipRootFacts::result_holds_own_reference(
            closure,
            closure.results[0],
            &aliases
        )
    );
}

/// CPython's call protocol: the limit check guards the call (its false edge
/// raises RecursionError), and the admitted frame is counted before the call
/// and released after it in the same block, so no path leaks or skips a count.
fn assert_recursion_charge_brackets_the_direct_call(func: &TirFunction) {
    let admits = ops(func, OpCode::RecursionAdmits);
    assert_eq!(admits.len(), 1);
    let (check_block, check) = &admits[0];
    let Terminator::CondBranch {
        cond, then_block, ..
    } = &func.blocks[check_block].terminator
    else {
        panic!("the limit check ends its block in a branch");
    };
    assert_eq!(*cond, check.results[0]);
    let kinds: Vec<OpCode> = func.blocks[then_block]
        .ops
        .iter()
        .map(|op| op.opcode)
        .collect();
    assert_eq!(
        kinds,
        [
            OpCode::RecursionCount,
            OpCode::DirectCall,
            OpCode::RecursionRelease
        ]
    );
}

/// `x < 2**50`: the constant is an exact int past the inline window, so it
/// travels as a full-range raw i64 (as a range counter with an unknown bound
/// does). Before this case no stage and no backend lane owned the compare: it
/// boxed the raw side, called the runtime and asked the runtime for the truth
/// of the result, on every iteration of `for i in range(n)`.
#[test]
fn a_comparison_with_a_full_range_raw_int_compares_in_the_int_arm_only() {
    for (opcode, runtime) in COMPARISONS {
        let (mut func, constant, _) = with_constant(opcode, 1 << 50);
        assert_eq!(plan(&func)[&constant], Repr::RawI64FullDeopt);
        assert_eq!(run(&mut func).values_changed, 1, "{opcode:?}");
        assert_valid(&func);
        // One tag test, on the boxed operand; no arm reads the raw int as a
        // float, whose conversion is inexact past 2**53.
        let tests = ops(&func, OpCode::IsInlineInt);
        assert_eq!(tests.len(), 1, "{opcode:?}");
        assert_eq!(tests[0].1.operands, [ValueId(0)]);
        assert!(ops(&func, OpCode::IsFloatWord).is_empty(), "{opcode:?}");
        assert!(ops(&func, OpCode::IntToFloat).is_empty(), "{opcode:?}");
        let compares = ops(&func, opcode);
        assert_eq!(compares.len(), 1, "{opcode:?}");
        assert_eq!(compares[0].1.operands[1], constant);
        // The runtime entry takes words: the slow arm boxes the raw int
        // through the full-range path, never the inline-window box.
        let (slow, call) = &ops(&func, OpCode::CallRuntime)[0];
        assert_eq!(call.runtime, Some(runtime));
        assert!(ops(&func, OpCode::BoxInlineInt).is_empty(), "{opcode:?}");
        let boxes: Vec<_> = func.blocks[slow]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::BoxVal)
            .collect();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].operands, [constant]);
        assert_eq!(call.operands, [ValueId(0), boxes[0].results[0]]);
    }
}

/// `c <op> x` over a constant and a boxed parameter.
fn constant_first(opcode: OpCode, c: i64) -> (TirFunction, ValueId) {
    let (mut func, constant, _) = with_constant(opcode, c);
    let entry = func.entry_block;
    let op = func.blocks.get_mut(&entry).unwrap().ops.last_mut().unwrap();
    op.operands = vec![constant, ValueId(0)];
    (func, constant)
}

/// `a <op> b` over two constants, with a boxed result.
fn two_constants(opcode: OpCode, a: i64, b: i64) -> (TirFunction, ValueId, ValueId) {
    let (mut func, lhs, _) = with_constant(opcode, a);
    let rhs = func.fresh_value();
    func.value_types.insert(rhs, TirType::I64);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.insert(
        1,
        TirOp {
            attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(b))]),
            ..TirOp::new(OpCode::ConstInt, vec![], vec![rhs])
        },
    );
    block.ops.last_mut().unwrap().operands = vec![lhs, rhs];
    (func, lhs, rhs)
}

/// `x + 2**50` and `x * 2**50`: the constant is a full-range raw int, so the
/// int arm computes with the checked op and leaves for the runtime on
/// overflow. Before this arm the op boxed the raw side and called the runtime.
#[test]
fn a_full_range_raw_int_adds_and_multiplies_through_a_checked_op() {
    for (opcode, checked, runtime) in [
        (OpCode::Add, OpCode::CheckedAdd, RuntimeFn::Add),
        (
            OpCode::InplaceAdd,
            OpCode::CheckedAdd,
            RuntimeFn::InplaceAdd,
        ),
        (OpCode::Mul, OpCode::CheckedMul, RuntimeFn::Mul),
    ] {
        let (mut func, constant, _) = with_constant(opcode, 1 << 50);
        assert_eq!(plan(&func)[&constant], Repr::RawI64FullDeopt);
        assert_eq!(run(&mut func).values_changed, 1, "{opcode:?}");
        assert_valid(&func);
        assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 1, "{opcode:?}");
        assert!(ops(&func, OpCode::IsFloatWord).is_empty(), "{opcode:?}");
        assert!(ops(&func, OpCode::IntToFloat).is_empty(), "{opcode:?}");
        let (arm, op) = &ops(&func, checked)[0];
        let payload = ops(&func, OpCode::UnboxInlineInt)[0].1.results[0];
        assert_eq!(op.operands, [payload, constant]);
        // The overflow flag sends the arm to the runtime call.
        let (slow, call) = &ops(&func, OpCode::CallRuntime)[0];
        assert_eq!(call.runtime, Some(runtime));
        match &func.blocks[arm].terminator {
            Terminator::CondBranch {
                cond, then_block, ..
            } => {
                assert_eq!(*cond, op.results[1]);
                assert_eq!(then_block, slow);
            }
            other => panic!("the checked op ends its block on the flag: {other:?}"),
        }
        let repr = plan(&func);
        assert_eq!(repr[&op.results[0]], Repr::RawI64FullDeopt);
    }
}

/// `2**50 - x` adds the negated payload, which is exact in i64; `x - 2**50`
/// would negate a full-range int, which is not (`i64::MIN`), and keeps the
/// runtime entry.
#[test]
fn a_full_range_subtraction_adds_the_negated_inline_payload() {
    let (mut func, constant) = constant_first(OpCode::Sub, 1 << 50);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    let payload = ops(&func, OpCode::UnboxInlineInt)[0].1.results[0];
    let (_, negate) = &ops(&func, OpCode::Sub)[0];
    assert_eq!(negate.operands[1], payload);
    let (_, sum) = &ops(&func, OpCode::CheckedAdd)[0];
    assert_eq!(sum.operands, [constant, negate.results[0]]);
    let repr = plan(&func);
    assert!(repr[&negate.results[0]].is_raw_i64_carrier());
    assert_eq!(
        ops(&func, OpCode::CallRuntime)[0].1.runtime,
        Some(RuntimeFn::Sub)
    );

    let (mut func, _, _) = with_constant(OpCode::Sub, 1 << 50);
    assert_eq!(run(&mut func).values_changed, 0);
    assert_eq!(func.blocks.len(), 1);
}

/// Two raw operands stay the backend's raw lane, whatever their result: such
/// an op with a boxed result is a checked loop's overflow exit, and expanding
/// it gave the loop's raw carriers refcount ops on every iteration (4x the
/// instructions of `for i in range(n): total += i`).
#[test]
fn two_raw_operands_keep_the_backend_lane() {
    for opcode in [
        OpCode::Add,
        OpCode::Sub,
        OpCode::Mul,
        OpCode::Shl,
        OpCode::Shr,
    ] {
        for (a, b) in [(1 << 50, 1), (5, 1)] {
            let (mut func, _, _) = two_constants(opcode, a, b);
            assert_eq!(run(&mut func).values_changed, 0, "{opcode:?} {a} {b}");
            assert_eq!(func.blocks.len(), 1);
        }
    }
}

fn guards(func: &TirFunction) -> Vec<(OpCode, i64)> {
    let constants: std::collections::BTreeMap<ValueId, i64> = ops(func, OpCode::ConstInt)
        .iter()
        .filter_map(|(_, op)| match op.attrs.get("value") {
            Some(AttrValue::Int(value)) => Some((op.results[0], *value)),
            _ => None,
        })
        .collect();
    let mut found = Vec::new();
    for opcode in [OpCode::Ge, OpCode::Le] {
        for (_, op) in ops(func, opcode) {
            found.push((opcode, constants[&op.operands[1]]));
        }
    }
    found
}

/// `x >> c`: one tag test per boxed operand, the count guarded to a machine
/// count, then a machine shift whose result the plan carries as an inline
/// int. A negative count (ValueError) and a count past 63 reach the runtime.
#[test]
fn a_right_shift_guards_its_count_and_shifts_the_inline_payload() {
    let (mut func, constant, _) = with_constant(OpCode::Shr, 3);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 1);
    assert!(ops(&func, OpCode::IsFloatWord).is_empty());
    assert_eq!(guards(&func), [(OpCode::Ge, 0), (OpCode::Le, 63)]);
    let payload = ops(&func, OpCode::UnboxInlineInt)[0].1.results[0];
    let (_, shift) = &ops(&func, OpCode::Shr)[0];
    assert_eq!(shift.operands, [payload, constant]);
    assert!(shift.has_nothrow_proof());
    assert_eq!(plan(&func)[&shift.results[0]], Repr::RawI64Safe);
    let (_, call) = &ops(&func, OpCode::CallRuntime)[0];
    assert_eq!(call.runtime, Some(RuntimeFn::Rshift));
    assert!(!call.has_nothrow_proof(), "a negative count raises");

    // Two boxed words: both tags are tested, and the count is the payload.
    let (mut func, _) = binary(OpCode::Shr, [TirType::DynBox, TirType::DynBox]);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2);
    assert_eq!(guards(&func), [(OpCode::Ge, 0), (OpCode::Le, 63)]);
}

/// A full-range value shifts right only inside the inline window, so the
/// machine shift's result is an inline int by construction.
#[test]
fn a_right_shift_of_a_full_range_int_guards_the_inline_window() {
    let (mut func, lhs) = constant_first(OpCode::Shr, 1 << 50);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 1);
    let rhs = ops(&func, OpCode::UnboxInlineInt)[0].1.results[0];
    assert_eq!(
        guards(&func),
        [
            (OpCode::Ge, 0),
            (OpCode::Ge, -(1 << 46)),
            (OpCode::Le, 63),
            (OpCode::Le, (1 << 46) - 1)
        ]
    );
    let (_, shift) = &ops(&func, OpCode::Shr)[0];
    assert_eq!(shift.operands, [lhs, rhs]);
    assert_eq!(plan(&func)[&shift.results[0]], Repr::RawI64Safe);
}

/// `x << c` is the checked product `x * (1 << c)` for a count in [0, 45]:
/// the power of two is an inline int, and a product past i64 reaches the
/// runtime, which computes the big int.
#[test]
fn a_left_shift_multiplies_by_an_inline_power_of_two_with_an_overflow_check() {
    for (mut func, tests) in [
        (with_constant(OpCode::Shl, 3).0, 1),
        (binary(OpCode::Shl, [TirType::DynBox, TirType::DynBox]).0, 2),
        (constant_first(OpCode::Shl, 1 << 50).0, 1),
    ] {
        assert_eq!(run(&mut func).values_changed, 1);
        assert_valid(&func);
        assert_eq!(ops(&func, OpCode::IsInlineInt).len(), tests);
        assert_eq!(guards(&func), [(OpCode::Ge, 0), (OpCode::Le, 45)]);
        let (_, power) = &ops(&func, OpCode::Shl)[0];
        assert!(power.has_nothrow_proof());
        let repr = plan(&func);
        assert_eq!(repr[&power.results[0]], Repr::RawI64Safe);
        let (_, product) = &ops(&func, OpCode::CheckedMul)[0];
        assert_eq!(product.operands[1], power.results[0]);
        assert_eq!(repr[&product.results[0]], Repr::RawI64FullDeopt);
        let (_, call) = &ops(&func, OpCode::CallRuntime)[0];
        assert_eq!(call.runtime, Some(RuntimeFn::Lshift));
    }
}

/// A full-range operand of a bitwise op or a true division keeps its
/// lowering: its result is no inline int, and it has no exact float.
#[test]
fn a_full_range_operand_of_a_bitwise_op_or_a_division_keeps_its_lowering() {
    for opcode in [OpCode::BitAnd, OpCode::BitOr, OpCode::BitXor, OpCode::Div] {
        let (mut func, _, _) = with_constant(opcode, 1 << 50);
        assert_eq!(run(&mut func).values_changed, 0, "{opcode:?}");
    }
}

/// A raw int that meets a boxed block argument on one edge of a conditional
/// branch reaches it through a forwarding block, where the backend boxes it;
/// the other edge is unchanged.
/// Before, the backend boxed it ahead of the branch, so a loop exit boxed its
/// raw carrier on every iteration.
#[test]
fn a_raw_int_meets_a_boxed_block_argument_on_its_own_edge() {
    use crate::tir::blocks::TirBlock;
    use crate::tir::values::TirValue;
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.entry_block;
    let wide = func.fresh_value();
    let flag = func.fresh_value();
    let joined = func.fresh_value();
    func.value_types.insert(wide, TirType::I64);
    func.value_types.insert(flag, TirType::Bool);
    func.value_types.insert(joined, TirType::DynBox);
    let exit = func.fresh_block();
    let other = func.fresh_block();
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(1 << 50))]),
        ..TirOp::new(OpCode::ConstInt, vec![], vec![wide])
    });
    block.ops.push(TirOp {
        attrs: AttrDict::from_iter([("value".into(), AttrValue::Bool(true))]),
        ..TirOp::new(OpCode::ConstBool, vec![], vec![flag])
    });
    block.terminator = Terminator::CondBranch {
        cond: flag,
        then_block: exit,
        then_args: vec![wide],
        else_block: other,
        else_args: vec![],
    };
    func.blocks.insert(
        other,
        TirBlock {
            id: other,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit,
                args: vec![ValueId(0)],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![TirValue {
                id: joined,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![joined],
            },
        },
    );
    assert_eq!(plan(&func)[&wide], Repr::RawI64FullDeopt);
    assert_eq!(super::box_raw_edge_args(&mut func), 1);
    assert_valid(&func);
    let Terminator::CondBranch {
        then_block,
        then_args,
        else_block,
        ..
    } = &func.blocks[&entry].terminator
    else {
        panic!("the entry keeps its conditional branch");
    };
    assert_eq!(*else_block, other);
    assert!(then_args.is_empty());
    // The edge block forwards the raw value: the backend boxes it there.
    let edge = &func.blocks[then_block];
    assert!(edge.ops.is_empty());
    assert!(matches!(
        &edge.terminator,
        Terminator::Branch { target, args } if *target == exit && *args == [wide]
    ));
    assert_eq!(plan(&func)[&wide], Repr::RawI64FullDeopt);
    // A second run finds nothing left to box.
    assert_eq!(super::box_raw_edge_args(&mut func), 0);
}

#[test]
fn a_static_call_only_enters_or_adopts_arguments_on_the_admitted_path() {
    for origin in [molt_ir::OpKind::Call, molt_ir::OpKind::CallInternal] {
        let (mut func, result) = guarded_call(true);
        let entry = func.entry_block;
        let target = molt_ir::FunctionId(9);
        let call = &mut func.blocks.get_mut(&entry).unwrap().ops[0];
        call.origin = Some(origin);
        call.callee = Some(target);
        let mut leaf = molt_ir::FunctionSet::default();
        leaf.insert(target);
        assert!(!super::charge_static_calls(&mut func, &leaf));
        assert!(super::charge_static_calls(&mut func, &Default::default()));
        assert_valid(&func);
        let Terminator::CondBranch {
            cond,
            then_block,
            else_block,
            ..
        } = &func.blocks[&entry].terminator
        else {
            panic!("a non-leaf static call must test the limit");
        };
        let check = &func.blocks[&entry].ops;
        assert_eq!(check.len(), 1);
        assert_eq!(
            (check[0].opcode, check[0].results.as_slice()),
            (OpCode::RecursionAdmits, std::slice::from_ref(cond))
        );
        let entered = &func.blocks[then_block];
        assert_eq!(
            entered.ops.iter().map(|op| op.opcode).collect::<Vec<_>>(),
            [
                OpCode::RecursionCount,
                OpCode::Call,
                OpCode::RecursionRelease
            ]
        );
        assert_eq!(entered.ops[1].callee, Some(target));
        assert_eq!(
            entered.ops[1].argument_custody().unwrap(),
            [crate::ir::ParameterCustody::Transferred; 2]
        );
        assert!(
            matches!(&entered.terminator, Terminator::Return { values } if values == &[result])
        );
        let refused = &func.blocks[else_block];
        assert_eq!(refused.ops.len(), 1);
        assert_eq!(refused.ops[0].runtime, Some(RuntimeFn::RaiseRecursionError));
        assert!(refused.ops[0].results.is_empty());
        assert!(matches!(refused.terminator, Terminator::Unwind));
    }
}

#[test]
fn direct_closure_call_takes_the_capture_on_its_reaching_edge() {
    use crate::tir::blocks::TirBlock;
    use molt_ir::ParameterCustody::{Borrowed, Transferred};
    let mut f = TirFunction::new(
        "outer".into(),
        vec![TirType::Bool, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = f.entry_block;
    let left = f.fresh_block();
    let right = f.fresh_block();
    let join = f.fresh_block();
    let target = molt_ir::FunctionId(0);
    f.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: left,
        then_args: vec![],
        else_block: right,
        else_args: vec![],
    };
    for (branch, capture) in [(left, ValueId(1)), (right, ValueId(2))] {
        let callable = super::fresh_typed(&mut f, TirType::DynBox);
        let mut constructor = TirOp::new(OpCode::Copy, vec![capture], vec![callable]);
        constructor.origin = Some(molt_ir::OpKind::FuncNewClosure);
        constructor.callee = Some(target);
        f.blocks.insert(
            branch,
            TirBlock {
                id: branch,
                args: vec![],
                ops: vec![constructor],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![],
                },
            },
        );
    }
    let result = super::fresh_typed(&mut f, TirType::DynBox);
    // The user argument always reads parameter 1, even when the environment
    // comes from parameter 2 on the other arm.
    let mut call = TirOp::new(OpCode::Call, vec![ValueId(1)], vec![result]);
    call.callee = Some(target);
    call.set_argument_custody(&[Transferred]);
    f.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![],
            ops: vec![call],
            terminator: Terminator::Return {
                values: vec![result],
            },
        },
    );
    let signature = crate::ir::ExternFunctionSignature {
        arity: 2,
        has_closure: true,
        returns_value: true,
        execution_context: Default::default(),
    };
    assert!(super::lower_static_call_arguments(
        &mut f,
        molt_ir::FunctionId(1),
        &[signature]
    ));
    assert_valid(&f);
    let merged = f.blocks[&join].args[0].id;
    assert_eq!(f.blocks[&join].ops[0].operands, [merged, ValueId(1)]);
    assert_eq!(
        f.blocks[&join].ops[0].argument_custody().unwrap(),
        [Borrowed, Transferred]
    );
    for (branch, capture) in [(left, ValueId(1)), (right, ValueId(2))] {
        let body = &f.blocks[&branch];
        let Terminator::Branch { target, args } = &body.terminator else {
            panic!("expected join edge")
        };
        assert_eq!(*target, join);
        let definition = body.ops.iter().find(|op| op.results == *args).unwrap();
        assert_eq!((definition.opcode, definition.origin), (OpCode::Copy, None));
        assert_eq!(definition.operands, [capture]);
    }
}

/// The frontend lifts direct intrinsics as Call with a typed runtime field;
/// exercise that actual ingest boundary rather than a hand-made cold opcode.
#[test]
fn exact_float_sqrt_uses_the_frontend_call_shape_and_keeps_domain_continuation() {
    use molt_ir::{FunctionIR, OpIR, OpKind};
    let source = FunctionIR {
        name: "sqrt_exact".into(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops: vec![
            OpIR {
                f_value: Some(-4.0),
                out: Some("x".into()),
                ..OpIR::new(OpKind::ConstFloat)
            },
            OpIR {
                s_value: Some(RuntimeFn::MathSqrt.symbol().into()),
                args: Some(vec!["x".into()]),
                out: Some("root".into()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["root".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..FunctionIR::default()
    };
    let mut func = crate::tir::lower_from_simple::lower_to_tir(&source);
    let original = ops(&func, OpCode::Call)[0].1.clone();
    assert_eq!(original.runtime, Some(RuntimeFn::MathSqrt));
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::FloatSqrt).len(), 1);
    let cold = ops(&func, OpCode::CallRuntime);
    assert_eq!(cold.len(), 1);
    assert_eq!(cold[0].1.runtime, Some(RuntimeFn::MathSqrt));
    assert_eq!(cold[0].1.attrs, original.attrs);
    assert_eq!(cold[0].1.source_span, original.source_span);
    assert_eq!(
        run(&mut func).values_changed,
        0,
        "cold arm cannot re-expand"
    );
}

#[test]
fn cold_module_expands_typed_sqrt_only_in_repeated_blocks() {
    let mut func = TirFunction::new(
        "py.app.__molt_module_chunk_1".into(),
        vec![TirType::Bool],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.entry_block;
    let loop_body = func.fresh_block();
    let exit = func.fresh_block();
    let input = func.fresh_value();
    func.value_types.insert(input, TirType::F64);
    let prefix = func.fresh_value();
    let repeated = func.fresh_value();
    let result = func.fresh_value();
    for value in [prefix, repeated, result] {
        func.value_types.insert(value, TirType::DynBox);
    }
    let sqrt = |output| {
        let mut op = TirOp::new(OpCode::Call, vec![input], vec![output]);
        op.runtime = Some(RuntimeFn::MathSqrt);
        op.attrs.insert(
            "s_value".into(),
            AttrValue::Str(RuntimeFn::MathSqrt.symbol().into()),
        );
        op
    };
    let entry_block = func.blocks.get_mut(&entry).unwrap();
    let mut value = TirOp::new(OpCode::ConstFloat, Vec::new(), vec![input]);
    value.attrs.insert("f_value".into(), AttrValue::Float(9.0));
    entry_block.ops.extend([value, sqrt(prefix)]);
    entry_block.terminator = super::branch_to(loop_body);
    func.blocks.insert(
        loop_body,
        super::new_block(
            loop_body,
            vec![sqrt(repeated)],
            super::cond_branch(ValueId(0), loop_body, exit),
        ),
    );
    func.blocks.insert(
        exit,
        super::new_block(
            exit,
            vec![sqrt(result)],
            Terminator::Return {
                values: vec![result],
            },
        ),
    );

    assert_valid(&func);
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::FloatSqrt).len(), 1);
    assert_eq!(
        ops(&func, OpCode::Call)
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [entry, exit],
    );
    assert_eq!(
        ops(&func, OpCode::CallRuntime)[0].1.runtime,
        Some(RuntimeFn::MathSqrt)
    );
}

#[test]
fn boxed_float_conversion_proves_the_sqrt_operand_without_reboxing_it() {
    use molt_ir::{FunctionIR, OpIR, OpKind};

    let source = FunctionIR {
        name: "convert_then_sqrt".into(),
        params: vec!["arg".into()],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops: vec![
            OpIR {
                args: Some(vec!["arg".into()]),
                out: Some("converted".into()),
                ..OpIR::new(OpKind::FloatFromObj)
            },
            OpIR {
                s_value: Some(RuntimeFn::MathSqrt.symbol().into()),
                args: Some(vec!["converted".into()]),
                out: Some("root".into()),
                ..OpIR::new(OpKind::Call)
            },
            OpIR {
                args: Some(vec!["root".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ],
        ..FunctionIR::default()
    };
    let mut func = crate::tir::lower_from_simple::lower_to_tir(&source);
    let input = ops(&func, OpCode::Call)[0].1.operands[0];
    assert_eq!(
        crate::tir::type_refine::extract_exact_scalar_map(&func)[&input],
        TirType::Box(Box::new(TirType::F64)),
    );
    assert_eq!(
        crate::representation_facts::analyze_representations(&func)[&input],
        Repr::DynBox,
    );
    assert_eq!(run(&mut func).values_changed, 1);
    assert_valid(&func);
    assert_eq!(ops(&func, OpCode::IsFloatWord)[0].1.operands, [input]);
    assert_eq!(ops(&func, OpCode::FloatSqrt).len(), 1);
    assert_eq!(ops(&func, OpCode::CallRuntime)[0].1.operands, [input]);
}

#[test]
fn sqrt_does_not_admit_unknown_or_integer_arguments() {
    for ty in [TirType::DynBox, TirType::I64, TirType::Bool] {
        let mut func = TirFunction::new(
            "sqrt_generic".into(),
            vec![ty],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        func.value_types.insert(result, TirType::DynBox);
        let mut call = TirOp::new(OpCode::Call, vec![ValueId(0)], vec![result]);
        call.runtime = Some(RuntimeFn::MathSqrt);
        call.attrs.insert(
            "s_value".into(),
            AttrValue::Str(RuntimeFn::MathSqrt.symbol().into()),
        );
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block.ops.push(call);
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        assert_eq!(run(&mut func).values_changed, 0);
        assert!(ops(&func, OpCode::FloatSqrt).is_empty());
    }
}
