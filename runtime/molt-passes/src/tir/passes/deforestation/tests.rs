use super::fusion::{is_fusable_body, run};
use super::tuple_scalarize::run_tuple_scalarize;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_call_builtin(name: &str, operand: ValueId, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallBuiltin,
        operands: vec![operand],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("name".into(), AttrValue::Str(name.into()));
            m
        },
        source_span: None,
    }
}

/// Build a minimal function representing `sum(x for x in data)`:
///
///   bb0 (entry): data = param[0]
///     iter = GetIter(data)
///     → Branch bb1
///   bb1 (loop header):
///     elem = ForIter(iter)
///     → CondBranch(elem_valid, bb2, bb3)
///   bb2 (loop body): [pure ops on elem]
///     → Branch bb1
///   bb3 (exit):
///     result = CallBuiltin("sum", elem)
///     → Return(result)
fn build_iter_sum_function() -> TirFunction {
    let mut func = TirFunction::new("test_sum".into(), vec![TirType::DynBox], TirType::I64);

    // Values: 0=data(param), 1=iter, 2=elem, 3=elem_valid, 4=result
    let iter_val = func.fresh_value(); // 1
    let elem_val = func.fresh_value(); // 2
    let elem_valid = func.fresh_value(); // 3
    let result_val = func.fresh_value(); // 4

    let bb1 = func.fresh_block(); // loop header
    let bb2 = func.fresh_block(); // loop body
    let bb3 = func.fresh_block(); // exit

    // bb0 (entry): GetIter → Branch bb1
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::GetIter, vec![ValueId(0)], vec![iter_val]));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
    }

    // bb1 (loop header): ForIter → CondBranch
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ForIter,
                operands: vec![iter_val],
                results: vec![elem_val],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::CondBranch {
                cond: elem_valid,
                then_block: bb2,
                then_args: vec![],
                else_block: bb3,
                else_args: vec![],
            },
        },
    );

    // bb2 (loop body): pure — just branches back
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: bb1,
                args: vec![],
            },
        },
    );

    // bb3 (exit): CallBuiltin("sum", elem) → Return
    func.blocks.insert(
        bb3,
        TirBlock {
            id: bb3,
            args: vec![],
            ops: vec![make_call_builtin("sum", elem_val, result_val)],
            terminator: Terminator::Return {
                values: vec![result_val],
            },
        },
    );

    func
}

// -----------------------------------------------------------------------
// Test 1: sum(x for x in data) → fused accumulator loop
// -----------------------------------------------------------------------
#[test]
fn sum_genexpr_fused_to_accumulator() {
    let mut func = build_iter_sum_function();
    let stats = run(&mut func);

    assert!(
        stats.values_changed >= 1,
        "should have fused at least one chain"
    );
    assert!(stats.ops_added >= 2, "should have added init + add ops");

    // The CallBuiltin("sum") should have been replaced with a Copy.
    let bb3 = BlockId(3);
    let exit_ops = &func.blocks[&bb3].ops;
    assert_eq!(exit_ops.len(), 1);
    assert_eq!(exit_ops[0].opcode, OpCode::Copy);
    assert_eq!(
        exit_ops[0].attrs.get("fused"),
        Some(&AttrValue::Str("sum".into()))
    );
}

// -----------------------------------------------------------------------
// Test 2: any(x > 0 for x in data) → fused early-exit
// -----------------------------------------------------------------------
#[test]
fn any_genexpr_fused_to_early_exit() {
    let mut func = build_iter_sum_function();

    // Change the CallBuiltin from "sum" to "any".
    let bb3 = BlockId(3);
    func.blocks.get_mut(&bb3).unwrap().ops[0] = make_call_builtin(
        "any",
        ValueId(2), // elem
        ValueId(4), // result
    );

    let stats = run(&mut func);

    assert!(stats.values_changed >= 1);
    let exit_ops = &func.blocks[&bb3].ops;
    assert_eq!(exit_ops[0].opcode, OpCode::Copy);
    assert_eq!(
        exit_ops[0].attrs.get("fused"),
        Some(&AttrValue::Str("any".into()))
    );
}

// -----------------------------------------------------------------------
// Test 3: Loop body with Call → NOT fused (impure)
// -----------------------------------------------------------------------
#[test]
fn impure_body_not_fused() {
    let mut func = build_iter_sum_function();

    // Add a Call op to the loop body (bb2) to make it impure.
    let bb2 = BlockId(2);
    let call_result = func.fresh_value();
    func.blocks.get_mut(&bb2).unwrap().ops.push(make_op(
        OpCode::Call,
        vec![ValueId(2)],
        vec![call_result],
    ));

    let stats = run(&mut func);

    // Should NOT have fused anything.
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_added, 0);

    // The CallBuiltin("sum") should remain unchanged.
    let bb3 = BlockId(3);
    let exit_ops = &func.blocks[&bb3].ops;
    assert_eq!(exit_ops[0].opcode, OpCode::CallBuiltin);
}

// -----------------------------------------------------------------------
// Test 4: No iterator patterns → no changes
// -----------------------------------------------------------------------
#[test]
fn no_iterator_patterns_no_changes() {
    let mut func = TirFunction::new("noop".into(), vec![TirType::I64], TirType::I64);
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::ConstInt, vec![], vec![ValueId(0)]));
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
    }

    let stats = run(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(stats.ops_removed, 0);
}

// -----------------------------------------------------------------------
// Test 5: Nested generators → only innermost fused (conservative)
// -----------------------------------------------------------------------
#[test]
fn nested_generators_conservative() {
    // Build a function with two nested ForIter loops but only one
    // CallBuiltin("sum") consuming the inner loop's element.
    // The pass should fuse at most the inner chain.
    let mut func = build_iter_sum_function();

    // Add a second GetIter → ForIter in a new block that wraps the existing
    // loop. The outer loop is NOT connected to the CallBuiltin, so the pass
    // should still fuse the inner one only.
    let stats = run(&mut func);

    // The inner sum chain should still fuse.
    assert!(stats.values_changed >= 1);
    // But at most one chain fused.
    assert_eq!(stats.values_changed, 1);
}

// -----------------------------------------------------------------------
// Test 6: all(genexpr) → fused early-exit with inverted logic
// -----------------------------------------------------------------------
#[test]
fn all_genexpr_fused() {
    let mut func = build_iter_sum_function();

    let bb3 = BlockId(3);
    func.blocks.get_mut(&bb3).unwrap().ops[0] = make_call_builtin("all", ValueId(2), ValueId(4));

    let stats = run(&mut func);

    assert!(stats.values_changed >= 1);
    let exit_ops = &func.blocks[&bb3].ops;
    assert_eq!(exit_ops[0].opcode, OpCode::Copy);
    assert_eq!(
        exit_ops[0].attrs.get("fused"),
        Some(&AttrValue::Str("all".into()))
    );
    // all → init is true, early-exit on false
    assert_eq!(
        exit_ops[0].attrs.get("early_exit_on"),
        Some(&AttrValue::Bool(false))
    );
}

// -----------------------------------------------------------------------
// Test 7: is_fusable_body unit tests
// -----------------------------------------------------------------------
#[test]
fn fusion_check_fusable_ops() {
    let ops = vec![
        make_op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]),
        make_op(OpCode::Mul, vec![ValueId(2), ValueId(0)], vec![ValueId(3)]),
        make_op(OpCode::Gt, vec![ValueId(3), ValueId(1)], vec![ValueId(4)]),
    ];
    assert!(is_fusable_body(&ops));
}

#[test]
fn fusion_check_barrier_call() {
    let ops = vec![make_op(OpCode::Call, vec![ValueId(0)], vec![ValueId(1)])];
    assert!(!is_fusable_body(&ops));
}

#[test]
fn fusion_check_barrier_store_attr() {
    let ops = vec![make_op(
        OpCode::StoreAttr,
        vec![ValueId(0), ValueId(1)],
        vec![],
    )];
    assert!(!is_fusable_body(&ops));
}

#[test]
fn fusion_check_barrier_yield() {
    let ops = vec![make_op(OpCode::Yield, vec![ValueId(0)], vec![ValueId(1)])];
    assert!(!is_fusable_body(&ops));
}

#[test]
fn fusion_check_empty_is_fusable() {
    assert!(is_fusable_body(&[]));
}

// ===================================================================
// Tuple Scalarization Tests
// ===================================================================

/// Helper: make an unpack_sequence op (Copy with _original_kind).
fn make_unpack_sequence(source: ValueId, results: Vec<ValueId>, count: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("unpack_sequence".into()),
    );
    attrs.insert("value".into(), AttrValue::Int(count));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![source],
        results,
        attrs,
        source_span: None,
    }
}

/// Build a minimal function representing `a, b = b, a + b`:
///
///   bb0 (entry):
///     %0 = param (b)
///     %1 = param (a_plus_b)
///     %2 = BuildTuple(%0, %1)
///     (%3, %4) = unpack_sequence(%2, 2)
///     → Return(%3, %4)
fn build_fib_swap_function() -> TirFunction {
    let mut func = TirFunction::new(
        "fib_swap".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );

    // params: ValueId(0)=b, ValueId(1)=a_plus_b
    let tuple_val = func.fresh_value(); // 2
    let new_a = func.fresh_value(); // 3
    let new_b = func.fresh_value(); // 4

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // BuildTuple(%0, %1) -> %2
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1)],
        vec![tuple_val],
    ));

    // (%3, %4) = unpack_sequence(%2)
    entry
        .ops
        .push(make_unpack_sequence(tuple_val, vec![new_a, new_b], 2));

    entry.terminator = Terminator::Return {
        values: vec![new_a, new_b],
    };

    func
}

// -----------------------------------------------------------------------
// Test: Basic fib swap scalarization
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_fib_swap() {
    let mut func = build_fib_swap_function();
    let stats = run_tuple_scalarize(&mut func);

    assert_eq!(stats.values_changed, 1, "should scalarize one tuple");
    assert_eq!(stats.ops_removed, 2, "should remove BuildTuple + unpack");
    assert_eq!(stats.ops_added, 2, "should add 2 Copy ops");

    // The entry block should now have exactly 2 Copy ops (no BuildTuple, no unpack).
    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 2);

    // First Copy: %3 = Copy(%0)  (new_a = b)
    assert_eq!(entry.ops[0].opcode, OpCode::Copy);
    assert_eq!(entry.ops[0].operands, vec![ValueId(0)]);
    assert_eq!(entry.ops[0].results, vec![ValueId(3)]);
    // Should NOT have _original_kind (it's a real Copy, not a passthrough).
    assert!(!entry.ops[0].attrs.contains_key("_original_kind"));

    // Second Copy: %4 = Copy(%1)  (new_b = a_plus_b)
    assert_eq!(entry.ops[1].opcode, OpCode::Copy);
    assert_eq!(entry.ops[1].operands, vec![ValueId(1)]);
    assert_eq!(entry.ops[1].results, vec![ValueId(4)]);
    assert!(!entry.ops[1].attrs.contains_key("_original_kind"));
}

// -----------------------------------------------------------------------
// Test: Tuple used elsewhere -> NOT scalarized (escapes)
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_escaping_tuple_not_eliminated() {
    let mut func = TirFunction::new(
        "escape".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );

    let tuple_val = func.fresh_value(); // 2
    let new_a = func.fresh_value(); // 3
    let new_b = func.fresh_value(); // 4
    let call_result = func.fresh_value(); // 5

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // BuildTuple
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1)],
        vec![tuple_val],
    ));

    // Unpack
    entry
        .ops
        .push(make_unpack_sequence(tuple_val, vec![new_a, new_b], 2));

    // Also pass the tuple to a function call (second use -> escapes)
    entry
        .ops
        .push(make_op(OpCode::Call, vec![tuple_val], vec![call_result]));

    entry.terminator = Terminator::Return {
        values: vec![new_a],
    };

    let stats = run_tuple_scalarize(&mut func);

    // Should NOT scalarize because tuple_val has 2 uses.
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
}

// -----------------------------------------------------------------------
// Test: Element count mismatch -> NOT scalarized
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_count_mismatch_not_eliminated() {
    let mut func = TirFunction::new(
        "mismatch".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );

    let tuple_val = func.fresh_value(); // 2
    let out_a = func.fresh_value(); // 3
    let out_b = func.fresh_value(); // 4
    let out_c = func.fresh_value(); // 5

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // BuildTuple with 2 elements
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1)],
        vec![tuple_val],
    ));

    // Unpack expecting 3 elements (mismatch!)
    entry.ops.push(make_unpack_sequence(
        tuple_val,
        vec![out_a, out_b, out_c],
        3,
    ));

    entry.terminator = Terminator::Return {
        values: vec![out_a],
    };

    let stats = run_tuple_scalarize(&mut func);

    // Should NOT scalarize due to element count mismatch.
    assert_eq!(stats.values_changed, 0);
}

// -----------------------------------------------------------------------
// Test: No BuildTuple in function -> no changes
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_no_tuples_no_changes() {
    let mut func = TirFunction::new("noop".into(), vec![TirType::I64], TirType::I64);
    let c = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![c]));
        entry.terminator = Terminator::Return { values: vec![c] };
    }

    let stats = run_tuple_scalarize(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
}

// -----------------------------------------------------------------------
// Test: 3-element tuple scalarization
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_three_elements() {
    let mut func = TirFunction::new(
        "triple".into(),
        vec![TirType::I64, TirType::I64, TirType::I64],
        TirType::I64,
    );

    let tuple_val = func.fresh_value(); // 3
    let out_a = func.fresh_value(); // 4
    let out_b = func.fresh_value(); // 5
    let out_c = func.fresh_value(); // 6

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // BuildTuple(%0, %1, %2) -> %3
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![tuple_val],
    ));

    // (%4, %5, %6) = unpack_sequence(%3, 3)
    entry.ops.push(make_unpack_sequence(
        tuple_val,
        vec![out_a, out_b, out_c],
        3,
    ));

    entry.terminator = Terminator::Return {
        values: vec![out_a, out_b, out_c],
    };

    let stats = run_tuple_scalarize(&mut func);

    assert_eq!(stats.values_changed, 1);
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(stats.ops_added, 3, "should add 3 Copy ops for 3 elements");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 3);

    // Verify each Copy connects the right element to the right target.
    for i in 0..3 {
        assert_eq!(entry.ops[i].opcode, OpCode::Copy);
        assert_eq!(entry.ops[i].operands, vec![ValueId(i as u32)]);
        assert_eq!(entry.ops[i].results, vec![ValueId(4 + i as u32)]);
    }
}

// -----------------------------------------------------------------------
// Test: Multiple scalarizations in the same block
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_multiple_in_same_block() {
    let mut func = TirFunction::new(
        "multi".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );

    // First tuple: swap a,b
    let tuple1 = func.fresh_value(); // 2
    let out1_a = func.fresh_value(); // 3
    let out1_b = func.fresh_value(); // 4

    // Second tuple: swap again
    let tuple2 = func.fresh_value(); // 5
    let out2_a = func.fresh_value(); // 6
    let out2_b = func.fresh_value(); // 7

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // First swap
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1)],
        vec![tuple1],
    ));
    entry
        .ops
        .push(make_unpack_sequence(tuple1, vec![out1_a, out1_b], 2));

    // Second swap (using outputs of first)
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![out1_a, out1_b],
        vec![tuple2],
    ));
    entry
        .ops
        .push(make_unpack_sequence(tuple2, vec![out2_a, out2_b], 2));

    entry.terminator = Terminator::Return {
        values: vec![out2_a, out2_b],
    };

    let stats = run_tuple_scalarize(&mut func);

    assert_eq!(stats.values_changed, 2, "should scalarize 2 tuples");
    assert_eq!(
        stats.ops_removed, 4,
        "should remove 2 BuildTuple + 2 unpack"
    );
    assert_eq!(stats.ops_added, 4, "should add 4 Copy ops total");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 4);
    assert!(entry.ops.iter().all(|op| op.opcode == OpCode::Copy));
}

// -----------------------------------------------------------------------
// Test: Tuple used in terminator -> NOT scalarized
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_tuple_in_terminator_not_eliminated() {
    let mut func = TirFunction::new(
        "term_use".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );

    let tuple_val = func.fresh_value(); // 2

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    // BuildTuple
    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0), ValueId(1)],
        vec![tuple_val],
    ));

    // Return the tuple directly (use in terminator = use count > 0)
    // No unpack_sequence at all.
    entry.terminator = Terminator::Return {
        values: vec![tuple_val],
    };

    let stats = run_tuple_scalarize(&mut func);

    // No unpack_sequence found, so nothing to scalarize.
    assert_eq!(stats.values_changed, 0);
}

// -----------------------------------------------------------------------
// Test: Single-element tuple scalarization
// -----------------------------------------------------------------------
#[test]
fn tuple_scalarize_single_element() {
    let mut func = TirFunction::new("single".into(), vec![TirType::I64], TirType::I64);

    let tuple_val = func.fresh_value(); // 1
    let out_a = func.fresh_value(); // 2

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();

    entry.ops.push(make_op(
        OpCode::BuildTuple,
        vec![ValueId(0)],
        vec![tuple_val],
    ));

    entry
        .ops
        .push(make_unpack_sequence(tuple_val, vec![out_a], 1));

    entry.terminator = Terminator::Return {
        values: vec![out_a],
    };

    let stats = run_tuple_scalarize(&mut func);

    assert_eq!(stats.values_changed, 1);
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(stats.ops_added, 1);

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 1);
    assert_eq!(entry.ops[0].opcode, OpCode::Copy);
    assert_eq!(entry.ops[0].operands, vec![ValueId(0)]);
    assert_eq!(entry.ops[0].results, vec![ValueId(2)]);
}
