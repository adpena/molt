use super::run;
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

#[test]
fn boxed_add_becomes_the_inline_int_diamond() {
    let (mut func, result) = binary(OpCode::Add, [TirType::DynBox, TirType::DynBox]);
    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 1);
    assert_valid(&func);
    assert_eq!(func.blocks.len(), 5);
    assert_eq!(ops(&func, OpCode::IsInlineInt).len(), 2);
    let unboxed = ops(&func, OpCode::UnboxInlineInt);
    assert_eq!(unboxed.len(), 2);
    let fast = unboxed[0].0;
    let fast_ops: Vec<OpCode> = func.blocks[&fast].ops.iter().map(|op| op.opcode).collect();
    assert_eq!(
        fast_ops,
        [
            OpCode::UnboxInlineInt,
            OpCode::UnboxInlineInt,
            OpCode::Add,
            OpCode::BoxVal
        ]
    );
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
            .all(|(block, _)| *block == fast)
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
        assert_eq!(ops(&func, fast).len(), 1, "{opcode:?}");
        assert!(
            ops(&func, opcode).len() <= usize::from(opcode == fast),
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

#[test]
fn raw_or_mixed_operands_keep_their_current_lowering() {
    // A small constant is a raw inline-safe carrier, not a boxed word.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let one = func.fresh_value();
    let result = func.fresh_value();
    func.value_types.insert(one, TirType::I64);
    func.value_types.insert(result, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        attrs: AttrDict::from([("value".into(), AttrValue::Int(1))]),
        ..TirOp::new(OpCode::ConstInt, vec![], vec![one])
    });
    block
        .ops
        .push(TirOp::new(OpCode::Add, vec![ValueId(0), one], vec![result]));
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 0);
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
    assert!(crate::tir::pass_manager::run_late_lowering(&mut func, &tti));
    assert_valid(&func);
    let tests_of_a = ops(&func, OpCode::IsInlineInt)
        .into_iter()
        .filter(|(_, op)| op.operands == [ValueId(0)])
        .count();
    assert_eq!(tests_of_a, 1);
}

#[test]
fn a_function_without_eligible_ops_is_left_untouched() {
    let (mut func, _) = binary(OpCode::Mul, [TirType::DynBox, TirType::DynBox]);
    let before = crate::tir::printer::print_function(&func);
    let tti = crate::tir::target_info::TargetInfo::llvm_release_fast();
    assert!(!crate::tir::pass_manager::run_late_lowering(
        &mut func, &tti
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
