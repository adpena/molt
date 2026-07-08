use super::*;
use crate::tir::blocks::TirBlock;
use crate::tir::numeric_facts::python_range_len;
use crate::tir::ops::{Dialect, TirOp};
use crate::tir::types::TirType;

/// Helper: create a function with a single block, apply SCCP, return the block's ops.
fn run_sccp_on_ops(ops: Vec<TirOp>, next_value: u32) -> (Vec<TirOp>, Terminator) {
    let mut func = TirFunction::new("test".into(), vec![], TirType::None);
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
    let mut func = TirFunction::new("test".into(), vec![], TirType::None);
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
    let mut func = TirFunction::new("test".into(), vec![], TirType::None);
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
    let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::I64);
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
fn fold_math_sqrt_constant() {
    // math.sqrt(4.0) => 2.0
    let ops = vec![
        make_const_float(0, 4.0),
        make_call_builtin(1, "math.sqrt", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstFloat);
    assert_eq!(
        result_ops[1].attrs.get("f_value"),
        Some(&AttrValue::Float(2.0))
    );
}

#[test]
fn fold_math_floor_constant() {
    // math.floor(3.7) => 3
    let ops = vec![
        make_const_float(0, 3.7),
        make_call_builtin(1, "math.floor", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn fold_str_upper_method() {
    // "hello".upper() => "HELLO"
    let ops = vec![
        make_const_str(0, "hello"),
        make_call_method(1, "upper", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[1].attrs.get("s_value"),
        Some(&AttrValue::Str("HELLO".into()))
    );
}

#[test]
fn fold_str_lower_method() {
    // "WORLD".lower() => "world"
    let ops = vec![
        make_const_str(0, "WORLD"),
        make_call_method(1, "lower", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[1].attrs.get("s_value"),
        Some(&AttrValue::Str("world".into()))
    );
}

#[test]
fn fold_str_strip_method() {
    // "  hi  ".strip() => "hi"
    let ops = vec![
        make_const_str(0, "  hi  "),
        make_call_method(1, "strip", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[1].attrs.get("s_value"),
        Some(&AttrValue::Str("hi".into()))
    );
}

#[test]
fn fold_str_startswith_method() {
    // "hello".startswith("hel") => True
    let ops = vec![
        make_const_str(0, "hello"),
        make_const_str(1, "hel"),
        make_call_method(2, "startswith", vec![0, 1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[2].attrs.get("value"),
        Some(&AttrValue::Bool(true))
    );
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

#[test]
fn fold_str_replace_method() {
    // "hello world".replace("world", "rust") => "hello rust"
    let ops = vec![
        make_const_str(0, "hello world"),
        make_const_str(1, "world"),
        make_const_str(2, "rust"),
        make_call_method(3, "replace", vec![0, 1, 2]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 4);
    assert_eq!(result_ops[3].opcode, OpCode::ConstStr);
    assert_eq!(
        result_ops[3].attrs.get("s_value"),
        Some(&AttrValue::Str("hello rust".into()))
    );
}

#[test]
fn fold_int_bit_length_method() {
    // (255).bit_length() => 8
    let ops = vec![
        make_const_int(0, 255),
        make_call_method(1, "bit_length", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(8)));
}

#[test]
fn fold_bool_builtin() {
    // bool(0) => False
    let ops = vec![make_const_int(0, 0), make_call_builtin(1, "bool", vec![0])];
    let (result_ops, _) = run_sccp_on_ops(ops, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[1].attrs.get("value"),
        Some(&AttrValue::Bool(false))
    );
}

#[test]
fn fold_math_gcd() {
    // math.gcd(12, 8) => 4
    let ops = vec![
        make_const_int(0, 12),
        make_const_int(1, 8),
        make_call_builtin(2, "math.gcd", vec![0, 1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(4)));
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
fn fold_len_of_constant_list() {
    // len([1, 2, 3]) => 3
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_const_int(2, 3),
        make_build_list(3, vec![0, 1, 2]),
        make_call_builtin(4, "len", vec![3]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 5);
    assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn fold_len_of_constant_dict() {
    // len({"a": 1, "b": 2}) => 2
    let ops = vec![
        make_const_str(0, "a"),
        make_const_int(1, 1),
        make_const_str(2, "b"),
        make_const_int(3, 2),
        make_build_dict(4, vec![0, 1, 2, 3]),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(2)));
}

#[test]
fn fold_len_of_range() {
    // len(range(10)) => 10
    let ops = vec![
        make_const_int(0, 10),
        make_call_builtin(1, "range", vec![0]),
        make_call_builtin(2, "len", vec![1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(10)));
}

#[test]
fn fold_len_of_range_with_start_stop() {
    // len(range(3, 10)) => 7
    let ops = vec![
        make_const_int(0, 3),
        make_const_int(1, 10),
        make_call_builtin(2, "range", vec![0, 1]),
        make_call_builtin(3, "len", vec![2]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 4);
    assert_eq!(result_ops[3].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[3].attrs.get("value"), Some(&AttrValue::Int(7)));
}

#[test]
fn fold_len_of_range_with_step() {
    // len(range(0, 10, 3)) => 4  (0, 3, 6, 9)
    let ops = vec![
        make_const_int(0, 0),
        make_const_int(1, 10),
        make_const_int(2, 3),
        make_call_builtin(3, "range", vec![0, 1, 2]),
        make_call_builtin(4, "len", vec![3]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 5);
    assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(4)));
}

#[test]
fn fold_len_of_empty_range() {
    // len(range(10, 0)) => 0
    let ops = vec![
        make_const_int(0, 10),
        make_const_int(1, 0),
        make_call_builtin(2, "range", vec![0, 1]),
        make_call_builtin(3, "len", vec![2]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 4);
    assert_eq!(result_ops[3].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[3].attrs.get("value"), Some(&AttrValue::Int(0)));
}

#[test]
fn fold_len_of_negative_step_range() {
    // len(range(10, 0, -2)) => 5  (10, 8, 6, 4, 2)
    let ops = vec![
        make_const_int(0, 10),
        make_const_int(1, 0),
        make_const_int(2, -2),
        make_call_builtin(3, "range", vec![0, 1, 2]),
        make_call_builtin(4, "len", vec![3]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 5);
    assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(5)));
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
fn fold_bool_of_constant_list() {
    // bool([]) => False, bool([1]) => True
    let ops_empty = vec![
        make_build_list(0, vec![]),
        make_call_builtin(1, "bool", vec![0]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops_empty, 2);
    assert_eq!(result_ops[1].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[1].attrs.get("value"),
        Some(&AttrValue::Bool(false))
    );

    let ops_nonempty = vec![
        make_const_int(0, 42),
        make_build_list(1, vec![0]),
        make_call_builtin(2, "bool", vec![1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops_nonempty, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[2].attrs.get("value"),
        Some(&AttrValue::Bool(true))
    );
}

#[test]
fn fold_sum_of_constant_list() {
    // sum([1, 2, 3, 4]) => 10
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_const_int(2, 3),
        make_const_int(3, 4),
        make_build_list(4, vec![0, 1, 2, 3]),
        make_call_builtin(5, "sum", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(10)));
}

#[test]
fn fold_sorted_of_constant_list() {
    // sorted([3, 1, 2]) => [1, 2, 3]
    // len(sorted([3, 1, 2])) => 3
    let ops = vec![
        make_const_int(0, 3),
        make_const_int(1, 1),
        make_const_int(2, 2),
        make_build_list(3, vec![0, 1, 2]),
        make_call_builtin(4, "sorted", vec![3]),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    // sorted result stays as BuildList (no ConstList opcode), but len propagates
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(3)));
}

#[test]
fn fold_list_concat() {
    // len([1, 2] + [3, 4]) => 4
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_build_list(2, vec![0, 1]),
        make_const_int(3, 3),
        make_const_int(4, 4),
        make_build_list(5, vec![3, 4]),
        make_binop(OpCode::Add, 6, 2, 5),
        make_call_builtin(7, "len", vec![6]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 8);
    assert_eq!(result_ops[7].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[7].attrs.get("value"), Some(&AttrValue::Int(4)));
}

#[test]
fn fold_list_repeat() {
    // len([1, 2] * 3) => 6
    let ops = vec![
        make_const_int(0, 1),
        make_const_int(1, 2),
        make_build_list(2, vec![0, 1]),
        make_const_int(3, 3),
        make_binop(OpCode::Mul, 4, 2, 3),
        make_call_builtin(5, "len", vec![4]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 6);
    assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
    assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(6)));
}

#[test]
fn fold_bool_of_range() {
    // bool(range(0)) => False
    let ops = vec![
        make_const_int(0, 0),
        make_call_builtin(1, "range", vec![0]),
        make_call_builtin(2, "bool", vec![1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[2].attrs.get("value"),
        Some(&AttrValue::Bool(false))
    );

    // bool(range(5)) => True
    let ops = vec![
        make_const_int(0, 5),
        make_call_builtin(1, "range", vec![0]),
        make_call_builtin(2, "bool", vec![1]),
    ];
    let (result_ops, _) = run_sccp_on_ops(ops, 3);
    assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
    assert_eq!(
        result_ops[2].attrs.get("value"),
        Some(&AttrValue::Bool(true))
    );
}

#[test]
fn no_fold_oversized_list() {
    // Building a list with > MAX_COMPOUND_ELEMENTS should not fold.
    // We test with 1001 elements (above the cap).
    let mut ops = Vec::new();
    for i in 0..1001u32 {
        ops.push(make_const_int(i, i as i64));
    }
    let elem_ids: Vec<u32> = (0..1001).collect();
    ops.push(make_build_list(1001, elem_ids));
    ops.push(make_call_builtin(1002, "len", vec![1001]));
    let (result_ops, _) = run_sccp_on_ops(ops, 1003);
    // The BuildList should NOT fold (too large), so len() can't fold either.
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
