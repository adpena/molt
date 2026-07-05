use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use std::collections::HashMap;

use super::super::PassStats;
use super::run;

// -----------------------------------------------------------------------
// Helper: build a TirOp
// -----------------------------------------------------------------------
fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn op_with_attrs(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: AttrDict,
) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

fn int_attrs(v: i64) -> AttrDict {
    let mut m = AttrDict::new();
    m.insert("value".into(), AttrValue::Int(v));
    m
}

fn run_vectorize(func: &mut TirFunction) -> PassStats {
    let mut am = AnalysisManager::new();
    run(func, &mut am, &TargetInfo::native_release_fast())
}

// -----------------------------------------------------------------------
// Test 1: Simple array-sum loop → marked vectorizable with Sum reduction.
//
// CFG shape:
//   entry → loop_header(acc: I64) ──back──> loop_header
//                         └─exit──> exit_block
//
// loop_header body:
//   %elem = ConstInt 1          (simulates loading an element)
//   %acc2 = Add acc, %elem      (accumulator update — sum reduction)
//   ForIter …
// -----------------------------------------------------------------------
#[test]
fn simple_sum_loop_vectorizable() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let exit_id = BlockId(2);

    // Values
    let acc = ValueId(0); // loop block arg — accumulator
    let elem = ValueId(1); // loaded element
    let acc2 = ValueId(2); // updated accumulator
    let init = ValueId(3); // initial accumulator value

    let mut blocks = HashMap::new();

    // Entry: produce initial accumulator, branch to loop header.
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![op_with_attrs(
                OpCode::ConstInt,
                vec![],
                vec![init],
                int_attrs(0),
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![init],
            },
        },
    );

    // Loop header: acc is the block arg (accumulator).
    // The back-edge passes acc2, creating the reduction phi.
    blocks.insert(
        header_id,
        TirBlock {
            id: header_id,
            args: vec![TirValue {
                id: acc,
                ty: TirType::I64,
            }],
            ops: vec![
                // Simulate element load as a ConstInt.
                op_with_attrs(OpCode::ConstInt, vec![], vec![elem], int_attrs(1)),
                // acc2 = acc + elem  (sum reduction)
                op(OpCode::Add, vec![acc, elem], vec![acc2]),
                // ForIter marker.
                op(OpCode::ForIter, vec![], vec![]),
            ],
            // Conditional: continue loop (pass acc2 back) or exit.
            terminator: Terminator::CondBranch {
                cond: acc,
                then_block: header_id, // back-edge
                then_args: vec![acc2],
                else_block: exit_id,
                else_args: vec![acc2],
            },
        },
    );

    // Exit block.
    blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![TirValue {
                id: ValueId(4),
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(4)],
            },
        },
    );

    let mut func = TirFunction {
        name: "sum_loop".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 5,
        next_block: 3,
        attrs: crate::tir::ops::AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let stats = run_vectorize(&mut func);

    // Loop header should have been annotated.
    assert!(
        stats.values_changed > 0,
        "expected at least one loop annotated"
    );

    let header = &func.blocks[&header_id];
    let for_iter_op = header
        .ops
        .iter()
        .find(|o| o.opcode == OpCode::ForIter)
        .expect("ForIter op must exist");

    assert_eq!(
        for_iter_op.attrs.get("vectorize"),
        Some(&AttrValue::Bool(true)),
        "vectorize attr must be set"
    );
    assert_eq!(
        for_iter_op.attrs.get("reduction"),
        Some(&AttrValue::Str("sum".into())),
        "reduction attr must be 'sum'"
    );
}

// -----------------------------------------------------------------------
// Test 2: Loop with a function call → NOT marked vectorizable.
// -----------------------------------------------------------------------
#[test]
fn loop_with_call_not_vectorizable() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let exit_id = BlockId(2);

    let callee = ValueId(0);
    let result = ValueId(1);

    let mut blocks = HashMap::new();

    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![op_with_attrs(
                OpCode::ConstInt,
                vec![],
                vec![callee],
                int_attrs(0),
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        },
    );

    blocks.insert(
        header_id,
        TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![
                // Impure call inside loop — disqualifies vectorization.
                op(OpCode::Call, vec![callee], vec![result]),
                op(OpCode::ForIter, vec![], vec![]),
            ],
            terminator: Terminator::CondBranch {
                cond: callee,
                then_block: header_id,
                then_args: vec![],
                else_block: exit_id,
                else_args: vec![],
            },
        },
    );

    blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut func = TirFunction {
        name: "call_loop".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 2,
        next_block: 3,
        attrs: crate::tir::ops::AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    run_vectorize(&mut func);

    // The ForIter op must NOT have the "vectorize" attribute.
    let header = &func.blocks[&header_id];
    let for_iter_op = header
        .ops
        .iter()
        .find(|o| o.opcode == OpCode::ForIter)
        .expect("ForIter op must exist");

    assert!(
        !for_iter_op.attrs.contains_key("vectorize"),
        "loop with Call must NOT be marked vectorizable"
    );
}

// -----------------------------------------------------------------------
// Helper: build a single-block loop function whose header carries `args`
// and `ops`, with a self-back-edge passing `back_args` to the header and
// an exit edge to a Return.
//
// Centralising this scaffolding keeps the new mixed-type tests focused
// on the type contract rather than CFG plumbing, and matches the layout
// used by the original `loop_with_mixed_types_*` test.
// -----------------------------------------------------------------------
fn build_loop_func(
    name: &str,
    header_args: Vec<TirValue>,
    body_ops: Vec<TirOp>,
    cond: ValueId,
    back_args: Vec<ValueId>,
) -> TirFunction {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let exit_id = BlockId(2);

    let mut blocks = HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        },
    );
    blocks.insert(
        header_id,
        TirBlock {
            id: header_id,
            args: header_args,
            ops: body_ops,
            terminator: Terminator::CondBranch {
                cond,
                then_block: header_id,
                then_args: back_args,
                else_block: exit_id,
                else_args: vec![],
            },
        },
    );
    blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    // `next_value` / `next_block` only matter when the pass needs to mint
    // fresh ids; the vectorize pass is annotation-only, so a generous
    // upper bound is sufficient and keeps tests robust to future edits.
    TirFunction {
        name: name.into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 1024,
        next_block: 16,
        attrs: crate::tir::ops::AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    }
}

fn header_op(func: &TirFunction, header_id: BlockId) -> &TirOp {
    func.blocks[&header_id]
        .ops
        .iter()
        .find(|o| o.opcode == OpCode::ForIter)
        .expect("ForIter op must exist in loop header")
}

// -----------------------------------------------------------------------
// Test 3: Mixed I64 + F64 loop body → vectorized as F64 lanes with the
// `promoted` attr set, modelling `total = total + a[i]` where `a` is
// `list[int]` and `total` is `float`. This is the headline behaviour of
// the lift: previously this loop was bailed out of vectorization.
// -----------------------------------------------------------------------
#[test]
fn mixed_int_float_promotes_to_float_vector() {
    let int_val = ValueId(0); // simulates a[i] : int
    let float_acc = ValueId(1); // total : float (loop-carried)
    let int_as_float = ValueId(2); // result of mixed-type arithmetic
    let new_acc = ValueId(3); // updated accumulator (still float)

    let mut func = build_loop_func(
        "mixed_int_float_loop",
        vec![
            TirValue {
                id: int_val,
                ty: TirType::I64,
            },
            TirValue {
                id: float_acc,
                ty: TirType::F64,
            },
        ],
        vec![
            // First op references both an I64 operand and an F64 result —
            // the mixed-type pattern that previously bailed.
            op(OpCode::Add, vec![float_acc, int_val], vec![int_as_float]),
            op(OpCode::Add, vec![int_as_float, float_acc], vec![new_acc]),
            op(OpCode::ForIter, vec![], vec![]),
        ],
        int_val,
        vec![int_val, new_acc],
    );

    run_vectorize(&mut func);

    let for_iter_op = header_op(&func, BlockId(1));

    assert_eq!(
        for_iter_op.attrs.get("vectorize"),
        Some(&AttrValue::Bool(true)),
        "mixed-type loop must now be marked vectorizable"
    );
    assert_eq!(
        for_iter_op.attrs.get("element_type"),
        Some(&AttrValue::Str("f64".into())),
        "mixed-type loop must promote to f64 lanes"
    );
    assert_eq!(
        for_iter_op.attrs.get("simd_width"),
        Some(&AttrValue::Int(2)),
        "f64 lanes use the conservative 128-bit minimum width"
    );
    assert_eq!(
        for_iter_op.attrs.get("promoted"),
        Some(&AttrValue::Bool(true)),
        "promoted attr must signal lane-wise sitofp insertion"
    );
    // The Add-on-acc is still recognised as a Sum reduction even after
    // promotion — vectorized horizontal-add reductions on f64 are well-
    // defined on every targeted ISA.
    assert_eq!(
        for_iter_op.attrs.get("reduction"),
        Some(&AttrValue::Str("sum".into())),
        "sum reduction detection survives promotion"
    );
}

// -----------------------------------------------------------------------
// Test 4: Pure-int loop continues to vectorize as `i64` lanes with no
// `promoted` attribute. Guards against the lift accidentally promoting
// every loop to f64.
// -----------------------------------------------------------------------
#[test]
fn pure_int_remains_int_vector() {
    let acc = ValueId(0);
    let elem = ValueId(1);
    let acc2 = ValueId(2);

    let mut func = build_loop_func(
        "pure_int_loop",
        vec![TirValue {
            id: acc,
            ty: TirType::I64,
        }],
        vec![
            op_with_attrs(OpCode::ConstInt, vec![], vec![elem], int_attrs(7)),
            op(OpCode::Add, vec![acc, elem], vec![acc2]),
            op(OpCode::ForIter, vec![], vec![]),
        ],
        acc,
        vec![acc2],
    );

    run_vectorize(&mut func);

    let for_iter_op = header_op(&func, BlockId(1));

    assert_eq!(
        for_iter_op.attrs.get("vectorize"),
        Some(&AttrValue::Bool(true))
    );
    assert_eq!(
        for_iter_op.attrs.get("element_type"),
        Some(&AttrValue::Str("i64".into())),
        "pure-int loop must stay on i64 lanes"
    );
    assert!(
        !for_iter_op.attrs.contains_key("promoted"),
        "pure-int loop must NOT carry the promoted hint"
    );
    assert_eq!(
        for_iter_op.attrs.get("reduction"),
        Some(&AttrValue::Str("sum".into()))
    );
}

// -----------------------------------------------------------------------
// Test 5: Pure-float loop continues to vectorize as `f64` lanes with no
// `promoted` attribute (no integer to promote).
// -----------------------------------------------------------------------
#[test]
fn pure_float_remains_float_vector() {
    let acc = ValueId(0);
    let elem = ValueId(1);
    let acc2 = ValueId(2);

    let mut float_attrs = AttrDict::new();
    float_attrs.insert("value".into(), AttrValue::Float(1.5));

    let mut func = build_loop_func(
        "pure_float_loop",
        vec![TirValue {
            id: acc,
            ty: TirType::F64,
        }],
        vec![
            op_with_attrs(OpCode::ConstFloat, vec![], vec![elem], float_attrs),
            op(OpCode::Add, vec![acc, elem], vec![acc2]),
            op(OpCode::ForIter, vec![], vec![]),
        ],
        acc,
        vec![acc2],
    );

    run_vectorize(&mut func);

    let for_iter_op = header_op(&func, BlockId(1));

    assert_eq!(
        for_iter_op.attrs.get("vectorize"),
        Some(&AttrValue::Bool(true))
    );
    assert_eq!(
        for_iter_op.attrs.get("element_type"),
        Some(&AttrValue::Str("f64".into())),
        "pure-float loop must stay on f64 lanes"
    );
    assert!(
        !for_iter_op.attrs.contains_key("promoted"),
        "pure-float loop must NOT carry the promoted hint"
    );
    assert_eq!(
        for_iter_op.attrs.get("reduction"),
        Some(&AttrValue::Str("sum".into()))
    );
}

// -----------------------------------------------------------------------
// Test 6: Bool-mixed-with-Int arithmetic — Python's `True + 1 == 2`
// pattern. Bool operands collapse into the integer lane category, so the
// loop must vectorize as `i64` lanes without triggering the `promoted`
// hint. This guards against accidentally classifying Bool as a separate
// numeric category that would force unnecessary float promotion.
// -----------------------------------------------------------------------
#[test]
fn boolean_mixed_in_blocks_vectorization_correctness() {
    let acc = ValueId(0); // i64 accumulator
    let flag = ValueId(1); // bool predicate (e.g. element > 0)
    let acc2 = ValueId(2); // updated accumulator

    let mut func = build_loop_func(
        "bool_int_loop",
        vec![
            TirValue {
                id: acc,
                ty: TirType::I64,
            },
            TirValue {
                id: flag,
                ty: TirType::Bool,
            },
        ],
        vec![
            // Bool-promoted-to-int arithmetic: count += predicate.
            op(OpCode::Add, vec![acc, flag], vec![acc2]),
            op(OpCode::ForIter, vec![], vec![]),
        ],
        acc,
        vec![acc2, flag],
    );

    run_vectorize(&mut func);

    let for_iter_op = header_op(&func, BlockId(1));

    assert_eq!(
        for_iter_op.attrs.get("vectorize"),
        Some(&AttrValue::Bool(true)),
        "bool+int loop must vectorize"
    );
    assert_eq!(
        for_iter_op.attrs.get("element_type"),
        Some(&AttrValue::Str("i64".into())),
        "bool collapses into i64 lane category"
    );
    assert!(
        !for_iter_op.attrs.contains_key("promoted"),
        "bool+int does not require float promotion"
    );
    assert_eq!(
        for_iter_op.attrs.get("reduction"),
        Some(&AttrValue::Str("sum".into())),
        "predicate-counting reduction is still recognised as Sum"
    );
}

// -----------------------------------------------------------------------
// Test 7: Function with no loops → no changes, no panic.
// -----------------------------------------------------------------------
#[test]
fn no_loops_no_changes() {
    let mut func = TirFunction::new("no_loops".into(), vec![], TirType::None);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return { values: vec![] };

    let stats = run_vectorize(&mut func);

    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.ops_added, 0);
}
