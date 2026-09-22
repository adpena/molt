use super::*;
use crate::tir::blocks::TirBlock;
use crate::tir::numeric_facts::python_range_len;
use crate::tir::ops::{Dialect, TirOp};
use crate::tir::types::TirType;

/// Helper: create a function with a single block, apply SCCP, return the block's ops.
fn run_sccp_on_ops(ops: Vec<TirOp>, next_value: u32) -> (Vec<TirOp>, Terminator) {
    let mut func = TirFunction::new(
        "test".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
    }
    func.next_value = next_value;
    run(&mut func);
    let entry = &func.blocks[&func.entry_block];
    (entry.ops.clone(), entry.terminator.clone())
}

fn make_const_int(result: u32, value: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

fn make_const_float(result: u32, value: f64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("f_value".into(), AttrValue::Float(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstFloat,
        operands: vec![],
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

fn make_const_bool(result: u32, value: bool) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Bool(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

fn make_binop(opcode: OpCode, result: u32, lhs: u32, rhs: u32) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![ValueId(lhs), ValueId(rhs)],
        results: vec![ValueId(result)],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_check_exception(target_label: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(target_label));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

#[test]
fn fold_int_addition() {
    // 1 + 2 => 3
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_binop(OpCode::Add, 2, 0, 1),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    // The Add op should be rewritten to ConstInt(3).
    assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn fold_comparison_gt() {
    // 5 > 3 => true
    let ops = vec![
        make_const_int(0, 5),
        make_const_int(1, 3),
        make_binop(OpCode::Gt, 2, 0, 1),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[2].attrs.get("value"),
        Some(&AttrValue::Bool(true))
    );
}

#[test]
fn fold_constant_cond_branch_true() {
    // if true: goto bb1, else: goto bb2 => Branch to bb1
    let mut func = TirFunction::new(
        "test".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let then_id = func.fresh_block();
    let else_id = func.fresh_block();

    let const_true = make_const_bool(0, true);
    func.next_value = 1;

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_true);
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };
    }

    // Add stub blocks so iteration doesn't miss them.
    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);
    let entry = &func.blocks[&func.entry_block];
    match &entry.terminator {
        Terminator::Branch { target, .. } => {
            assert_eq!(*target, then_id);
        }
        other => panic!("expected Branch, got {:?}", other),
    }
    assert!(stats.ops_removed > 0);
}

#[test]
fn branch_fold_keeps_check_exception_handler_block_reachable() {
    let mut func = TirFunction::new(
        "test".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.has_exception_handling = true;
    let active_id = func.fresh_block();
    let dead_id = func.fresh_block();
    let exit_id = func.fresh_block();
    let handler_id = func.fresh_block();
    func.label_id_map.insert(handler_id.0, 100);

    let const_true = make_const_bool(0, true);
    func.next_value = 1;

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_true);
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: active_id,
            then_args: vec![],
            else_block: dead_id,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        active_id,
        TirBlock {
            id: active_id,
            args: vec![],
            ops: vec![make_check_exception(100)],
            terminator: Terminator::Branch {
                target: exit_id,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        dead_id,
        TirBlock {
            id: dead_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler_id,
        TirBlock {
            id: handler_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let stats = run(&mut func);

    assert!(stats.ops_removed > 0);
    assert!(
        !func.blocks.contains_key(&dead_id),
        "constant branch fold should still remove the truly dead normal successor"
    );
    assert!(
        func.blocks.contains_key(&handler_id),
        "check_exception handler blocks must remain reachable after SCCP branch folding"
    );
    assert_eq!(func.label_id_map.get(&handler_id.0), Some(&100));
}

#[test]
fn no_fold_parameter_plus_const() {
    // x + 1 where x is a function parameter => no folding
    let mut func = TirFunction::new(
        "test".into(),
        vec![TirType::I64],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    // param is ValueId(0)
    let const_one = make_const_int(1, 1);
    let add = make_binop(OpCode::Add, 2, 0, 1);
    func.next_value = 3;
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_one);
        entry.ops.push(add);
        entry.terminator = Terminator::Return {
            values: vec![ValueId(2)],
        };
    }

    let stats = run(&mut func);
    let entry = &func.blocks[&func.entry_block];
    // The Add should remain an Add (not folded).
    assert_eq!(entry.ops[1].opcode, OpCode::Add);
    assert_eq!(stats.values_changed, 0);
}

#[test]
fn fold_float_multiplication() {
    // 1.0 * 2.0 => 2.0
    let ops = vec![
        make_const_float(0, 1.0),
        make_const_float(1, 2.0),
        make_binop(OpCode::Mul, 2, 0, 1),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstFloat);
    assert_eq!(
        result_ops[2].attrs.get("f_value"),
        Some(&AttrValue::Float(2.0))
    );
}

// --- Concrete eval tests for effects-driven constant folding ---

fn make_const_str(result: u32, value: &str) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str(value.into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

fn make_call_builtin(result: u32, name: &str, args: Vec<u32>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str(name.into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallBuiltin,
        operands: args.into_iter().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

fn make_call_method(result: u32, method: &str, args: Vec<u32>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("method".into(), AttrValue::Str(method.into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethod,
        operands: args.into_iter().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs,
        source_span: None,
    }
}

#[test]
fn defers_float_str_repr_constants_to_runtime_formatter() {
    // CPython 3.12 repr(f64::from_bits(0x4289368ec8725340)) is
    // "3465264303690.4062"; Rust Display rounds this exact value to
    // "...4063". SCCP must not rewrite either call unless it can use the same
    // CPython-compatible formatter as the runtime.
    let tricky = f64::from_bits(0x4289368ec8725340);
    let ops = vec![
        make_const_float(0, tricky),
        make_call_builtin(1, "str", vec![0]),
        make_call_builtin(2, "repr", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[1].opcode, OpCode::CallBuiltin);
    assert_eq!(result_ops[1].operands, vec![ValueId(0)]);
    assert_eq!(
        result_ops[1].attrs.get("name"),
        Some(&AttrValue::Str("str".into()))
    );
    assert_eq!(result_ops[2].opcode, OpCode::CallBuiltin);
    assert_eq!(result_ops[2].operands, vec![ValueId(0)]);
    assert_eq!(
        result_ops[2].attrs.get("name"),
        Some(&AttrValue::Str("repr".into()))
    );
}

#[test]
fn fold_len_of_constant_string() {
    // len("hello") => 5
    let ops = vec![
        make_const_str(0, "hello"),
        make_call_builtin(1, "len", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(5)));
}

#[test]
fn fold_abs_of_negative_int() {
    // abs(-42) => 42
    let ops = vec![make_const_int(0, -42), make_call_builtin(1, "abs", vec![0])];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(42)));
}

#[test]
fn math_sqrt_retains_target_runtime_evaluation() {
    // A builtin name and host libm result are not target-semantic admission.
    let ops = vec![
        make_const_float(0, 4.0),
        make_call_builtin(1, "math.sqrt", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::CallBuiltin);
    assert_eq!(result_ops[1].operands, vec![ValueId(0)]);
}

#[test]
fn fold_min_of_two_ints() {
    // min(5, 3) => 3
    let ops = vec![
        make_const_int(0, 5),
        make_const_int(1, 3),
        make_call_builtin(2, "min", vec![0, 1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn fold_chr_ord_roundtrip() {
    // chr(65) => "A"
    let ops = vec![make_const_int(0, 65), make_call_builtin(1, "chr", vec![0])];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[1].attrs.get("s_value"),
        Some(&AttrValue::Str("A".into()))
    );
}

#[test]
fn fold_hex_of_int() {
    // hex(255) => "0xff"
    let ops = vec![make_const_int(0, 255), make_call_builtin(1, "hex", vec![0])];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[1].attrs.get("s_value"),
        Some(&AttrValue::Str("0xff".into()))
    );
}

#[test]
fn no_fold_print_builtin() {
    // print("hello") should NOT be folded (I/O side effect)
    let ops = vec![
        make_const_str(0, "hello"),
        make_call_builtin(1, "print", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::CallBuiltin);
}

// --- Compound constant folding tests ---

fn make_build_list(result: u32, elements: Vec<u32>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: elements.into_iter().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_build_tuple(result: u32, elements: Vec<u32>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildTuple,
        operands: elements.into_iter().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_build_dict(result: u32, kv_pairs: Vec<u32>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildDict,
        operands: kv_pairs.into_iter().map(ValueId).collect(),
        results: vec![ValueId(result)],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

#[test]
fn fold_len_of_immutable_tuple() {
    // len((1, 2, 3)) => 3
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_const_int(2, 3),
        make_build_tuple(3, vec![0, 1, 2]),
        make_call_builtin(4, "len", vec![3]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 5);
    assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn mutable_dict_contents_are_not_value_constants() {
    let ops = vec![
        make_const_str(0, "a"),
        make_const_int(1, 1),
        make_const_str(2, "b"),
        make_const_int(3, 2),
        make_build_dict(4, vec![0, 1, 2, 3]),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[4].opcode, OpCode::BuildDict);
    assert_eq!(result_ops[5].opcode, OpCode::CallBuiltin);
}

#[test]
fn fold_string_concatenation() {
    // "hello" + " " + "world" => "hello world"
    let ops = vec![
        make_const_str(0, "hello"),
        make_const_str(1, " "),
        make_binop(OpCode::Add, 2, 0, 1),
        make_const_str(3, "world"),
        make_binop(OpCode::Add, 4, 2, 3),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 5);
    // The intermediate "hello " should fold, then "hello " + "world" => "hello world"
    assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[2].attrs.get("s_value"),
        Some(&AttrValue::Str("hello ".into()))
    );
    assert_eq!(result_ops[4].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[4].attrs.get("s_value"),
        Some(&AttrValue::Str("hello world".into()))
    );
}

#[test]
fn fold_string_repeat() {
    // "ab" * 3 => "ababab"
    let ops = vec![
        make_const_str(0, "ab"),
        make_const_int(1, 3),
        make_binop(OpCode::Mul, 2, 0, 1),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[2].attrs.get("s_value"),
        Some(&AttrValue::Str("ababab".into()))
    );
}

#[test]
fn fold_string_repeat_zero() {
    // "abc" * 0 => ""
    let ops = vec![
        make_const_str(0, "abc"),
        make_const_int(1, 0),
        make_binop(OpCode::Mul, 2, 0, 1),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[2].attrs.get("s_value"),
        Some(&AttrValue::Str("".into()))
    );
}

#[test]
fn fold_sum_of_immutable_tuple() {
    // sum((1, 2, 3, 4)) => 10
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_const_int(2, 3),
        make_const_int(3, 4),
        make_build_tuple(4, vec![0, 1, 2, 3]),
        make_call_builtin(5, "sum", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(10)));
}

#[test]
fn builtin_mutable_results_are_not_value_constants() {
    // sorted returns a mutable list even when its input is immutable.
    let ops = vec![
        make_const_int(0, 3),
        make_const_int(1, 1),
        make_const_int(2, 2),
        make_build_tuple(3, vec![0, 1, 2]),
        make_call_builtin(4, "sorted", vec![3]),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[4].opcode, OpCode::CallBuiltin);
    assert_eq!(result_ops[5].opcode, OpCode::CallBuiltin);
}

#[test]
fn fold_tuple_concat() {
    // len((1, 2) + (3, 4)) => 4
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_build_tuple(2, vec![0, 1]),
        make_const_int(3, 3),
        make_const_int(4, 4),
        make_build_tuple(5, vec![3, 4]),
        make_binop(OpCode::Add, 6, 2, 5),
        make_call_builtin(7, "len", vec![6]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 8);
    assert_eq!(result_ops[7].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[7].attrs.get("value"), Some(&AttrValue::Int(4)));
}

#[test]
fn fold_tuple_repeat() {
    // len((1, 2) * 3) => 6
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_build_tuple(2, vec![0, 1]),
        make_const_int(3, 3),
        make_binop(OpCode::Mul, 4, 2, 3),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(6)));
}

#[test]
fn no_fold_oversized_tuple() {
    // Building a tuple with > MAX_COMPOUND_ELEMENTS should not fold.
    // We test with 1001 elements (above the cap).
    let mut ops = Vec::new();
    for i in 0..1001u32 {
        ops.push(make_const_int(i, i as i64));
    }
    let elem_ids: Vec<u32> = (0..1001).collect();
    ops.push(make_build_tuple(1001, elem_ids));
    ops.push(make_call_builtin(1002, "len", vec![1001]));
    let (result_ops, _) = run_sccp_on_ops(ops, 1003);
    // The BuildTuple should NOT fold (too large), so len() can't fold either.
    let len_op = &result_ops[1002];
    assert_eq!(len_op.opcode, OpCode::CallBuiltin);
}

#[test]
fn python_range_len_uses_canonical_numeric_fact() {
    // Verify the canonical numeric fact matches Python semantics for edge cases.
    assert_eq!(python_range_len(0, 10, 1), Some(10));
    assert_eq!(python_range_len(0, 10, 2), Some(5));
    assert_eq!(python_range_len(0, 10, 3), Some(4));
    assert_eq!(python_range_len(0, 0, 1), Some(0));
    assert_eq!(python_range_len(5, 5, 1), Some(0));
    assert_eq!(python_range_len(10, 0, -1), Some(10));
    assert_eq!(python_range_len(10, 0, -2), Some(5));
    assert_eq!(python_range_len(10, 0, -3), Some(4));
    assert_eq!(python_range_len(0, -10, -1), Some(10));
    assert_eq!(python_range_len(0, 10, -1), Some(0)); // empty (step goes wrong way)
    assert_eq!(python_range_len(10, 0, 1), Some(0)); // empty (step goes wrong way)
    assert_eq!(python_range_len(0, 1, 1), Some(1));
    assert_eq!(python_range_len(-5, 5, 1), Some(10));
    assert_eq!(python_range_len(0, 1, 0), None);
}

#[test]
fn malformed_producers_never_seed_rewrite_or_fold_downstream_control() {
    for (opcode, attrs) in [
        (OpCode::ConstInt, make_const_int(20, 7).attrs),
        (OpCode::ConstFloat, make_const_float(20, 1.0).attrs),
        (OpCode::ConstBool, make_const_bool(20, true).attrs),
        (OpCode::ConstStr, make_const_str(20, "value").attrs),
        (OpCode::ConstNone, AttrDict::new()),
        (OpCode::Add, AttrDict::new()),
        (OpCode::Neg, AttrDict::new()),
        (OpCode::Not, AttrDict::new()),
        (OpCode::BuildTuple, AttrDict::new()),
        (
            OpCode::CallBuiltin,
            make_call_builtin(20, "len", vec![]).attrs,
        ),
        (
            OpCode::CallMethod,
            make_call_method(20, "upper", vec![]).attrs,
        ),
    ] {
        for operand_count in 0..=4 {
            for result_count in 0..=3 {
                let mut producer = make_const_int(20, 7);
                producer.opcode = opcode;
                producer.operands = vec![ValueId(0); operand_count];
                producer.results = (20..20 + result_count).map(ValueId).collect();
                producer.attrs = attrs.clone();
                if admits_constant_result(&producer) {
                    continue;
                }
                let mut func = TirFunction::new(
                    "invalid_producer".into(),
                    vec![],
                    TirType::None,
                    molt_ir::FunctionReturnAbi::Void,
                );
                let then_block = func.fresh_block();
                let else_block = func.fresh_block();
                for id in [then_block, else_block] {
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
                let entry = func.blocks.get_mut(&func.entry_block).unwrap();
                entry.ops = vec![
                    if opcode == OpCode::Not {
                        make_const_bool(0, true)
                    } else {
                        make_const_int(0, 1)
                    },
                    producer.clone(),
                    make_binop(OpCode::Add, 30, 20, 0),
                    make_call_builtin(31, "len", vec![20]),
                ];
                entry.terminator = Terminator::CondBranch {
                    cond: ValueId(20),
                    then_block,
                    then_args: vec![],
                    else_block,
                    else_args: vec![],
                };
                func.next_value = 32;
                run(&mut func);
                let entry = &func.blocks[&func.entry_block];
                assert_eq!(
                    entry.ops[1].opcode, producer.opcode,
                    "{opcode:?}/{operand_count}/{result_count}"
                );
                assert_eq!(entry.ops[1].operands, producer.operands);
                assert_eq!(entry.ops[1].results, producer.results);
                assert_eq!(entry.ops[1].attrs, producer.attrs);
                assert_eq!(entry.ops[2].opcode, OpCode::Add);
                assert_eq!(entry.ops[3].opcode, OpCode::CallBuiltin);
                assert!(matches!(entry.terminator, Terminator::CondBranch { .. }));
                assert_eq!(func.blocks.len(), 3);
            }
        }
    }
}

#[test]
fn call_folding_preserves_unsupported_arguments_and_result_siblings() {
    for (mut call, valid_operand_count) in [
        (make_call_builtin(20, "len", vec![0]), 1),
        (make_call_method(20, "upper", vec![0]), 1),
        (make_call_method(20, "find", vec![0, 1]), 2),
        (make_call_method(20, "replace", vec![0, 1, 2]), 3),
    ] {
        let opcode = call.opcode;
        let prefix = vec![
            make_const_str(0, "abc"),
            make_const_str(1, "a"),
            make_const_str(2, "x"),
            make_const_int(3, 1),
        ];
        for results in [0, 2, 3] {
            call.results = (20..20 + results).map(ValueId).collect();
            let mut ops = prefix.clone();
            ops.push(call.clone());
            let (after, _) = run_sccp_on_ops(ops, 24);
            assert_eq!(after[4].opcode, opcode);
            assert_eq!(after[4].results, call.results);
        }
        call.results = vec![ValueId(20)];
        call.operands.push(ValueId(3));
        let mut ops = prefix.clone();
        ops.push(call.clone());
        ops.push(make_call_builtin(21, "len", vec![20]));
        let (after, _) = run_sccp_on_ops(ops, 24);
        assert_eq!(after[4].opcode, opcode, "extra args {call:?}");
        assert_eq!(after[4].operands.len(), valid_operand_count + 1);
        assert_eq!(after[5].opcode, OpCode::CallBuiltin);
        call.operands.pop();
        let mut ops = prefix;
        ops.push(call);
        let (after, _) = run_sccp_on_ops(ops, 24);
        if opcode == OpCode::CallBuiltin {
            assert_ne!(
                after[4].opcode, opcode,
                "valid fixed builtin must still fold"
            );
        } else {
            assert_eq!(
                after[4].opcode, opcode,
                "operand zero is a callable, not its receiver"
            );
        }
    }
}

#[test]
fn unproved_float_calls_and_operators_retain_runtime_evaluation() {
    for (name, values) in [
        ("int", vec![f64::INFINITY]),
        ("math.floor", vec![f64::NAN]),
        ("math.ceil", vec![(1_u64 << 63) as f64]),
        ("math.trunc", vec![f64::NEG_INFINITY]),
        ("math.sqrt", vec![4.0]),
        ("math.sqrt", vec![-1.0]),
        ("math.log", vec![0.0]),
        ("math.exp", vec![1000.0]),
        ("math.pow", vec![0.0, -1.0]),
        ("math.hypot", vec![3.0, 4.0]),
    ] {
        let mut ops: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, &value)| make_const_float(index as u32, value))
            .collect();
        let call = make_call_builtin(10, name, (0..values.len() as u32).collect());
        ops.push(call.clone());
        ops.push(make_call_builtin(11, "bool", vec![10]));
        let (after, _) = run_sccp_on_ops(ops, 12);
        let retained = &after[values.len()];
        assert_eq!(retained.opcode, call.opcode, "{name}/{values:?}");
        assert_eq!(retained.operands, call.operands);
        assert_eq!(retained.attrs, call.attrs);
        assert_eq!(after[values.len() + 1].opcode, OpCode::CallBuiltin);
    }
    for opcode in [OpCode::Pow, OpCode::FloorDiv, OpCode::Mod] {
        for (left, right) in [(2.0, 3.0), (1.0, 0.1), (-0.0, 3.0), (0.0, -3.0), (1.0, 0.0)] {
            let operation = make_binop(opcode, 2, 0, 1);
            let (after, _) = run_sccp_on_ops(
                vec![
                    make_const_float(0, left),
                    make_const_float(1, right),
                    operation.clone(),
                    make_call_builtin(3, "bool", vec![2]),
                ],
                4,
            );
            assert_eq!(after[2].opcode, opcode);
            assert_eq!(after[2].operands, operation.operands);
            assert_eq!(after[3].opcode, OpCode::CallBuiltin);
        }
    }
    let (after, _) = run_sccp_on_ops(
        vec![
            make_const_int(0, 9_007_199_254_740_993),
            make_const_int(1, 3),
            make_binop(OpCode::Div, 2, 0, 1),
            make_call_builtin(3, "bool", vec![2]),
        ],
        4,
    );
    assert_eq!(after[2].opcode, OpCode::Div);
    assert_eq!(after[2].operands, vec![ValueId(0), ValueId(1)]);
    assert_eq!(after[3].opcode, OpCode::CallBuiltin);
}

#[test]
fn dedicated_range_constructor_has_exact_three_operand_semantics() {
    for (start, stop, step, expected) in [
        (0, 10, 1, Some(10)),
        (3, 10, 1, Some(7)),
        (0, 10, 3, Some(4)),
        (10, 0, 1, Some(0)),
        (10, 0, -2, Some(5)),
        (0, 10, 0, None),
        (i64::MIN, i64::MAX, 1, None),
    ] {
        let mut range = make_call_builtin(3, "range", vec![0, 1, 2]);
        range
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("range_new".into()));
        let (after, _) = run_sccp_on_ops(
            vec![
                make_const_int(0, start),
                make_const_int(1, stop),
                make_const_int(2, step),
                range,
                make_call_builtin(4, "len", vec![3]),
            ],
            5,
        );
        assert_eq!(
            after[3].opcode,
            OpCode::CallBuiltin,
            "do not materialize a range value"
        );
        match expected {
            Some(len) => {
                assert_eq!(after[4].opcode, OpCode::ConstInt);
                assert_eq!(after[4].attrs.get("value"), Some(&AttrValue::Int(len)));
            }
            None => assert_eq!(after[4].opcode, OpCode::CallBuiltin),
        }
    }
}

#[test]
fn callable_metadata_never_turns_a_receiver_into_a_bound_callable() {
    for method in [
        "upper",
        "lower",
        "title",
        "capitalize",
        "swapcase",
        "strip",
        "lstrip",
        "rstrip",
        "isalpha",
        "isdigit",
        "isalnum",
        "isspace",
        "isupper",
        "islower",
        "startswith",
        "endswith",
        "find",
        "rfind",
        "count",
        "replace",
        "removeprefix",
        "removesuffix",
        "zfill",
        "bit_length",
        "bit_count",
        "is_integer",
    ] {
        for spelling in [method.to_string(), format!("BoundMethod:str:{method}")] {
            let call = make_call_method(3, &spelling, vec![0, 1, 2]);
            let (after, _) = run_sccp_on_ops(
                vec![
                    make_const_str(0, "abc"),
                    make_const_str(1, "a"),
                    make_const_str(2, "b"),
                    call.clone(),
                    make_call_builtin(4, "len", vec![3]),
                ],
                5,
            );
            assert_eq!(after[3].opcode, call.opcode, "{spelling}");
            assert_eq!(after[3].operands, call.operands);
            assert_eq!(after[3].attrs, call.attrs);
            assert_eq!(after[4].opcode, OpCode::CallBuiltin);
        }
    }
}

#[test]
fn mutable_lookup_names_do_not_establish_builtin_identity() {
    for name in [
        "bool",
        "int",
        "float",
        "str",
        "range",
        "math.floor",
        "math.ceil",
        "math.trunc",
        "math.fabs",
        "math.isfinite",
        "math.isinf",
        "math.isnan",
        "math.copysign",
        "math.gcd",
        "math.lcm",
    ] {
        for count in 0..=3 {
            let call = make_call_builtin(3, name, (0..count).collect());
            let (after, _) = run_sccp_on_ops(
                vec![
                    make_const_int(0, 1),
                    make_const_int(1, 2),
                    make_const_int(2, 3),
                    call.clone(),
                    make_call_builtin(4, "len", vec![3]),
                ],
                5,
            );
            assert_eq!(after[3].opcode, call.opcode, "{name}/{count}");
            assert_eq!(after[3].operands, call.operands);
            assert_eq!(after[3].attrs, call.attrs);
            assert_eq!(after[4].opcode, OpCode::CallBuiltin);
        }
    }
}

#[test]
fn recursive_constant_payloads_and_long_literals_stop_before_copy_amplification() {
    let mut ops = vec![make_const_str(0, &"x".repeat(501))];
    ops.push(make_build_tuple(1, vec![0]));
    ops.push(make_build_tuple(2, vec![1, 1]));
    ops.push(make_call_builtin(3, "len", vec![2]));
    ops.push(make_const_str(4, &"x".repeat(1001)));
    ops.push(make_call_builtin(5, "repr", vec![4]));
    let (after, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(after[3].opcode, OpCode::CallBuiltin);
    assert_eq!(after[5].opcode, OpCode::CallBuiltin);
}

#[test]
fn builtin_dispatch_view_separates_name_and_arguments_and_rejects_conflicts() {
    for dynamic in [false, true] {
        let mut call = make_call_builtin(2, "len", vec![1]);
        if dynamic {
            call.attrs.clear();
            call.operands.insert(0, ValueId(0));
        }
        let (after, _) = run_sccp_on_ops(
            vec![make_const_str(0, "len"), make_const_str(1, "payload"), call],
            3,
        );
        assert_eq!(after[2].opcode, OpCode::ConstInt);
        assert_eq!(after[2].attrs.get("value"), Some(&AttrValue::Int(7)));
    }
    for (key, value) in [
        ("_original_kind", AttrValue::Str("print".into())),
        ("_original_kind", AttrValue::Str("unknown_builtin".into())),
        ("name", AttrValue::Int(1)),
        ("s_value", AttrValue::Str("abs".into())),
        ("callee", AttrValue::Str("molt_is_truthy".into())),
    ] {
        let mut call = make_call_builtin(1, "len", vec![0]);
        call.attrs.insert(key.into(), value);
        let (after, _) = run_sccp_on_ops(vec![make_const_str(0, "abc"), call.clone()], 2);
        assert_eq!(after[1].opcode, call.opcode, "{key}");
        assert_eq!(after[1].attrs, call.attrs);
    }
}

#[test]
fn boolean_primitive_folds_exact_immutable_values_in_the_real_pipeline() {
    for (literal, expected) in [(make_const_str(0, ""), false), (make_const_int(0, 2), true)] {
        let mut op = make_binop(OpCode::Bool, 1, 0, 0);
        op.operands.truncate(1);
        let (after, _) = run_sccp_on_ops(vec![literal, op], 2);
        assert_eq!(after[1].opcode, OpCode::ConstBool);
        assert_eq!(
            after[1].attrs.get("value"),
            Some(&AttrValue::Bool(expected))
        );
    }
}

#[test]
fn lattice_float_identity_preserves_all_bits() {
    let bits = [
        0x0000_0000_0000_0000_u64,
        0x8000_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x7ff8_0000_0000_0001,
        0x7ff8_0000_0000_0002,
        0xfff8_0000_0000_0001,
        0x7ff0_0000_0000_0001,
    ];
    for left_bits in bits {
        for right_bits in bits {
            let left = ConstVal::Float(f64::from_bits(left_bits));
            let right = ConstVal::Float(f64::from_bits(right_bits));
            let expected = left_bits == right_bits;
            assert_eq!(left == right, expected, "{left_bits:x} vs {right_bits:x}");
            assert_eq!(
                LatticeValue::Constant(left) == LatticeValue::Constant(right),
                expected,
                "lattice wrapper must retain exact float identity"
            );
        }
    }
}

#[test]
fn lattice_identity_preserves_variants_and_sentinels() {
    assert_ne!(ConstVal::Bool(true), ConstVal::Int(1));
    assert_ne!(ConstVal::Int(1), ConstVal::Float(1.0));
    assert_ne!(ConstVal::Bool(false), ConstVal::None);
    assert_ne!(LatticeValue::Top, LatticeValue::Bottom);
    assert_ne!(LatticeValue::Top, LatticeValue::Constant(ConstVal::None));
    assert_eq!(LatticeValue::Top, LatticeValue::Top);
    assert_eq!(LatticeValue::Bottom, LatticeValue::Bottom);
    for value in [
        ConstVal::Int(1),
        ConstVal::Bool(true),
        ConstVal::Str("literal".into()),
        ConstVal::None,
    ] {
        assert_eq!(value, value.clone());
    }
}

#[test]
fn lattice_immutable_tuple_identity_is_recursive_not_allocation_based() {
    fn nested(zero: f64, bits: u64) -> ConstVal {
        ConstVal::Tuple(
            vec![
                ConstVal::Tuple(
                    vec![ConstVal::Float(zero), ConstVal::Float(f64::from_bits(bits))].into(),
                ),
                ConstVal::Range {
                    start: 1,
                    stop: 9,
                    step: 2,
                },
            ]
            .into(),
        )
    }
    let left = nested(0.0, 0x7ff8_0000_0000_0001);
    let same = nested(0.0, 0x7ff8_0000_0000_0001);
    let (ConstVal::Tuple(left_storage), ConstVal::Tuple(same_storage)) = (&left, &same) else {
        unreachable!();
    };
    assert!(!Arc::ptr_eq(left_storage, same_storage));
    assert_eq!(left, same);
    assert_eq!(left, left.clone());
    assert_ne!(left, nested(-0.0, 0x7ff8_0000_0000_0001));
    assert_ne!(left, nested(0.0, 0x7ff8_0000_0000_0002));
    assert_ne!(
        ConstVal::Tuple(vec![ConstVal::Bool(true)].into()),
        ConstVal::Tuple(vec![ConstVal::Int(1)].into())
    );
    // Python range equality compares sequences, but exposed start/stop/step
    // attributes still differ; value facts cannot merge these empty ranges.
    assert_ne!(
        ConstVal::Range {
            start: 0,
            stop: 0,
            step: 1
        },
        ConstVal::Range {
            start: 1,
            stop: 1,
            step: 1
        }
    );
}

#[test]
fn lattice_identity_does_not_replace_python_comparison_semantics() {
    let positive_zero = ConstVal::Float(0.0);
    let negative_zero = ConstVal::Float(-0.0);
    assert_ne!(positive_zero, negative_zero);
    assert_eq!(
        evaluate_op(OpCode::Eq, &[Some(&positive_zero), Some(&negative_zero)]),
        Some(ConstVal::Bool(true))
    );
    assert_eq!(
        evaluate_op(OpCode::Ne, &[Some(&positive_zero), Some(&negative_zero)]),
        Some(ConstVal::Bool(false))
    );
    let left_nan = ConstVal::Float(f64::from_bits(0x7ff8_0000_0000_0001));
    let right_nan = ConstVal::Float(f64::from_bits(0x7ff8_0000_0000_0001));
    assert_eq!(left_nan, right_nan);
    assert_eq!(
        evaluate_op(OpCode::Eq, &[Some(&left_nan), Some(&right_nan)]),
        Some(ConstVal::Bool(false))
    );
    assert_eq!(
        evaluate_op(OpCode::Ne, &[Some(&left_nan), Some(&right_nan)]),
        Some(ConstVal::Bool(true))
    );
}

#[test]
fn mutable_list_observations_survive_direct_and_aliased_inplace_mutation() {
    for aliased in [false, true] {
        for observer in ["len", "bool", "sum"] {
            let mut alias = make_build_list(3, vec![2]);
            alias.opcode = OpCode::Copy;
            let ops = vec![
                make_const_int(0, 1),
                make_const_int(1, 2),
                make_build_list(2, vec![0]),
                alias,
                make_build_list(4, vec![1]),
                make_call_builtin(5, observer, vec![2]),
                make_binop(OpCode::InplaceAdd, 6, if aliased { 3 } else { 2 }, 4),
                make_call_builtin(7, observer, vec![2]),
            ];
            let expected = ops.clone();
            let (result_ops, _) = run_sccp_on_ops(ops, 8);
            for index in [2, 3, 4, 5, 6, 7] {
                assert_eq!(
                    result_ops[index].opcode, expected[index].opcode,
                    "{observer}, aliased={aliased}, op={index}"
                );
                assert_eq!(result_ops[index].operands, expected[index].operands);
            }
        }
    }
}

#[test]
fn callbacks_cannot_leave_mutable_or_nested_compound_value_facts() {
    for constructor in [OpCode::BuildList, OpCode::BuildDict, OpCode::BuildSet] {
        let mut container = make_build_list(2, vec![0, 1]);
        container.opcode = constructor;
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            container,
            make_build_tuple(3, vec![2]),
            make_build_tuple(4, vec![3]),
            // The callback can mutate captured state without receiving the
            // container as an explicit operand. Operand-local invalidation
            // would miss this ownership boundary.
            make_call_builtin(5, "mutate_captured_state", vec![]),
            make_call_builtin(6, "len", vec![2]),
            make_call_builtin(7, "bool", vec![2]),
            make_call_builtin(8, "len", vec![3]),
            make_call_builtin(9, "bool", vec![4]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 10);
        assert_eq!(result_ops[2].opcode, constructor);
        assert_eq!(result_ops[3].opcode, OpCode::BuildTuple);
        assert_eq!(result_ops[4].opcode, OpCode::BuildTuple);
        for op in &result_ops[5..] {
            assert_eq!(op.opcode, OpCode::CallBuiltin, "{constructor:?}");
        }
    }
}

#[test]
fn mutable_builtin_and_sequence_results_remain_runtime_values() {
    for builtin in ["sorted", "list", "dict", "set"] {
        let ops = vec![
            make_const_int(0, 1),
            make_build_tuple(1, vec![0]),
            make_call_builtin(2, builtin, if builtin == "dict" { vec![] } else { vec![1] }),
            make_call_builtin(3, "mutate_captured_state", vec![]),
            make_call_builtin(4, "len", vec![2]),
            make_build_tuple(5, vec![2]),
            make_call_builtin(6, "bool", vec![5]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 7);
        for index in [2, 3, 4, 6] {
            assert_eq!(result_ops[index].opcode, OpCode::CallBuiltin, "{builtin}");
        }
    }
    for opcode in [OpCode::Add, OpCode::Mul] {
        let ops = vec![
            make_const_int(0, 2),
            make_build_list(1, vec![0]),
            make_binop(opcode, 2, 1, if opcode == OpCode::Add { 1 } else { 0 }),
            make_call_builtin(3, "mutate_captured_state", vec![]),
            make_call_builtin(4, "len", vec![2]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 5);
        assert_eq!(result_ops[2].opcode, opcode);
        assert_eq!(result_ops[4].opcode, OpCode::CallBuiltin);
    }
}
