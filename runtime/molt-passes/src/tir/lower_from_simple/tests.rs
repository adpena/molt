//! Tests for SimpleIR to TIR lowering.

use super::*;
use crate::ir::{FunctionIR, OpIR};
use crate::tir::blocks::Terminator;
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;

/// Helper: build a FunctionIR with given name, params, and ops.
fn make_func(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.iter().map(|s| s.to_string()).collect(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
    }
}

#[test]
fn lower_functions_to_tir_module_skips_externs_and_aligns_idx() {
    // [non-extern "a", extern "ext", non-extern "b"] → module has {a, b}
    // (extern skipped), idx_map aligns module position → original index.
    let mut ext = make_func("ext", &[], vec![op("ret_void")]);
    ext.is_extern = true;
    let funcs = vec![
        make_func("a", &[], vec![op("ret_void")]),
        ext,
        make_func("b", &[], vec![op("ret_void")]),
    ];
    let (module, idx_map) = lower_functions_to_tir_module(&funcs);
    assert_eq!(module.functions.len(), 2, "externs are skipped");
    assert_eq!(idx_map, vec![0, 2], "module position maps to source index");
    assert_eq!(module.functions[0].name, "a");
    assert_eq!(module.functions[1].name, "b");
}

/// Helper to create an `OpIR` with just a `kind`.
fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

/// Helper to create an `OpIR` with `kind`, `value`, and `out`.
fn op_val_out(kind: &str, value: i64, out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        value: Some(value),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

/// Helper to create an `OpIR` with `kind`, `args`, and `out`.
fn op_args_out(kind: &str, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..OpIR::default()
    }
}

/// Helper to create an `OpIR` with `kind` and `args`.
fn op_args(kind: &str, args: &[&str]) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        ..OpIR::default()
    }
}

/// Helper: create an op with integer compatibility hint.
fn op_fast_int(kind: &str, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        fast_int: Some(true),
        ..OpIR::default()
    }
}

/// Helper: create an op with float compatibility hint.
fn op_fast_float(kind: &str, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        fast_float: Some(true),
        ..OpIR::default()
    }
}

#[test]
fn cell_rewrite_skips_cells_escaped_into_closure_tuple() {
    let mut ops = vec![
        op_args_out("missing", &[], "missing"),
        op_args_out("list_new", &["missing"], "cell"),
        op_val_out("const", 0, "zero"),
        op_val_out("const", 7, "value"),
        op_args("store_index", &["cell", "zero", "value"]),
        op_args_out("tuple_new", &["cell"], "closure"),
        op_args_out("index", &["cell", "zero"], "loaded"),
    ];

    assert!(!rewrite_cell_locals_to_store_load(&mut ops));
    assert_eq!(ops[4].kind, "store_index");
    assert_eq!(ops[6].kind, "index");
}

#[test]
fn cell_rewrite_handles_multiple_unescaped_cells_independently() {
    let mut ops = vec![
        op_args_out("missing", &[], "missing_a"),
        op_args_out("list_new", &["missing_a"], "cell_a"),
        op_args_out("missing", &[], "missing_b"),
        op_args_out("list_new", &["missing_b"], "cell_b"),
        op_val_out("const", 0, "zero"),
        op_val_out("const", 1, "value_a"),
        op_args("store_index", &["cell_a", "zero", "value_a"]),
        op_args_out("index", &["cell_a", "zero"], "loaded_a"),
        op_val_out("const", 2, "value_b"),
        op_args("store_index", &["cell_b", "zero", "value_b"]),
        op_args_out("index", &["cell_b", "zero"], "loaded_b"),
    ];

    assert!(rewrite_cell_locals_to_store_load(&mut ops));
    assert_eq!(ops[6].kind, "store_var");
    assert_eq!(ops[6].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[7].kind, "load_var");
    assert_eq!(ops[7].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[9].kind, "store_var");
    assert_eq!(ops[9].var.as_deref(), Some("_cell_cell_b_0"));
    assert_eq!(ops[10].kind, "load_var");
    assert_eq!(ops[10].var.as_deref(), Some("_cell_cell_b_0"));
}

// =======================================================================
// Test 1: Trivial function — const + add + ret
// =======================================================================
#[test]
fn trivial_function_lowering() {
    let func_ir = make_func(
        "test_add",
        &[],
        vec![
            op_val_out("const", 1, "x"),
            op_args_out("add", &["x"], "y"),
            op_args("ret", &["y"]),
        ],
    );

    let tir = lower_to_tir(&func_ir);

    assert_eq!(tir.name, "test_add");
    assert!(!tir.blocks.is_empty(), "should have at least one block");
    assert!(tir.blocks.contains_key(&tir.entry_block));

    // Should have exactly 1 block for straight-line code.
    assert_eq!(tir.blocks.len(), 1);

    // Entry block should have 2 ops (const + add; ret is structural).
    let entry = &tir.blocks[&tir.entry_block];
    // 3 ops: ConstNone (SSA undef sentinel) + ConstInt + Add; ret is structural.
    assert_eq!(
        entry.ops.len(),
        3,
        "entry should have undef sentinel, const, and add ops"
    );

    // Terminator should be Return.
    assert!(
        matches!(entry.terminator, Terminator::Return { .. }),
        "expected Return terminator, got {:?}",
        entry.terminator
    );
}

// =======================================================================
// Test 2: Function with if/else control flow
// =======================================================================
#[test]
fn if_else_control_flow() {
    let func_ir = make_func(
        "test_branch",
        &[],
        vec![
            op_val_out("const", 0, "c"), // 0 entry
            op_args("if", &["c"]),       // 1 ends entry
            op_val_out("const", 1, "x"), // 2 then
            op("else"),                  // 3 else
            op_val_out("const", 2, "x"), // 4 else body
            op("end_if"),                // 5 join
            op_args("ret", &["x"]),      // 6 return
        ],
    );

    let tir = lower_to_tir(&func_ir);

    assert_eq!(tir.name, "test_branch");
    assert!(
        tir.blocks.len() >= 3,
        "if/else should produce at least 3 blocks"
    );

    // Find the join block — it should have a block argument for `x`.
    let join_block = tir.blocks.values().find(|b| !b.args.is_empty());
    assert!(
        join_block.is_some(),
        "should have a join block with block arguments"
    );
    let join = join_block.unwrap();
    assert_eq!(
        join.args.len(),
        1,
        "join block should have 1 block arg (for x)"
    );

    // There should be a block with a CondBranch terminator (the block
    // containing the `if` op — which may or may not be the entry block,
    // depending on how the CFG splits).
    let has_cond_branch = tir
        .blocks
        .values()
        .any(|b| matches!(b.terminator, Terminator::CondBranch { .. }));
    assert!(
        has_cond_branch,
        "should have a block with CondBranch terminator"
    );
}

#[test]
fn module_import_preserves_operand_through_lower_to_tir() {
    let func_ir = make_func(
        "module_import_shape",
        &["__molt_module_obj__"],
        vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("builtins".to_string()),
                out: Some("v62".to_string()),
                ..OpIR::default()
            },
            op_args_out("module_import", &["v62"], "v63"),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(3),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("_builtins".to_string()),
                out: Some("v64".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".to_string(),
                args: Some(vec![
                    "__molt_module_obj__".to_string(),
                    "v64".to_string(),
                    "v63".to_string(),
                ]),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            op("ret_void"),
        ],
    );

    let tir = lower_to_tir(&func_ir);
    let import_op = tir
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == crate::tir::ops::OpCode::Import)
        .expect("expected import op");
    assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
}

#[test]
fn gpu_thread_id_lowers_to_runtime_backed_call_in_tir() {
    let func_ir = make_func(
        "gpu_tid",
        &[],
        vec![
            OpIR {
                kind: "gpu_thread_id".to_string(),
                out: Some("tid".to_string()),
                ..OpIR::default()
            },
            op_args("ret", &["tid"]),
        ],
    );

    let tir = lower_to_tir(&func_ir);
    let call_op = tir
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == crate::tir::ops::OpCode::Call)
        .expect("expected gpu_thread_id to lower to a call op");
    assert_eq!(
        call_op.attrs.get("s_value"),
        Some(&crate::tir::ops::AttrValue::Str(
            "molt_gpu_thread_id".to_string()
        ))
    );
    assert_eq!(
        call_op.attrs.get("_original_kind"),
        Some(&crate::tir::ops::AttrValue::Str(
            "gpu_thread_id".to_string()
        ))
    );
}

// =======================================================================
// Test 3: transport hints do not seed canonical SSA types
// =======================================================================
#[test]
fn transport_hints_do_not_seed_canonical_types() {
    let func_ir = FunctionIR {
        name: "hint_only_add".into(),
        params: vec!["a".into(), "b".into(), "fa".into(), "fb".into()],
        ops: vec![
            op_fast_int("add", &["a", "b"], "c"),
            op_fast_float("mul", &["fa", "fb"], "fc"),
            op_args("ret", &["c"]),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);

    assert_eq!(
        tir.return_type,
        TirType::DynBox,
        "transport-only hints must not seed canonical TIR types"
    );
    for op in tir.blocks.values().flat_map(|block| &block.ops) {
        assert!(
            !op.attrs.contains_key("_fast_int"),
            "SimpleIR fast_int metadata must not enter TIR attrs: {op:?}"
        );
        assert!(
            !op.attrs.contains_key("_fast_float"),
            "SimpleIR fast_float metadata must not enter TIR attrs: {op:?}"
        );
    }
}

// =======================================================================
// Test 4: Empty function
// =======================================================================
#[test]
fn empty_function() {
    let func_ir = make_func("empty", &[], vec![]);
    let tir = lower_to_tir(&func_ir);

    assert_eq!(tir.name, "empty");
    // Empty ops → empty CFG → no blocks from SSA.
    assert!(tir.blocks.is_empty());
}

// =======================================================================
// Test 5: Function with param_types annotation
// =======================================================================
#[test]
fn param_types_from_annotation() {
    let func_ir = FunctionIR {
        name: "typed_add".to_string(),
        params: vec!["a".to_string(), "b".to_string()],
        ops: vec![op_args_out("add", &["a", "b"], "c"), op_args("ret", &["c"])],
        param_types: Some(vec!["int".to_string(), "float".to_string()]),
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);

    assert_eq!(tir.param_types.len(), 2);
    assert_eq!(tir.param_types[0], TirType::I64);
    assert_eq!(tir.param_types[1], TirType::F64);
    let entry = &tir.blocks[&tir.entry_block];
    assert_eq!(
        tir.value_types.get(&entry.args[0].id),
        Some(&TirType::I64),
        "entry param i64 fact must be present in the function-owned map"
    );
    assert_eq!(
        tir.value_types.get(&entry.args[1].id),
        Some(&TirType::F64),
        "entry param f64 fact must be present in the function-owned map"
    );
    let add_result = entry
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Add)
        .and_then(|op| op.results.first())
        .copied()
        .expect("typed add result");
    assert_eq!(
        tir.value_types.get(&add_result),
        Some(&TirType::F64),
        "arithmetic propagation must persist op-result facts on TirFunction"
    );
}

#[test]
fn compound_param_types_from_annotation() {
    let func_ir = FunctionIR {
        name: "typed_container".to_string(),
        params: vec!["items".to_string()],
        ops: vec![op_args("ret", &["items"])],
        param_types: Some(vec!["list[int]".to_string()]),
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);
    let expected = TirType::List(Box::new(TirType::I64));

    assert_eq!(tir.param_types, vec![expected.clone()]);
    let entry = &tir.blocks[&tir.entry_block];
    assert_eq!(
        tir.value_types.get(&entry.args[0].id),
        Some(&expected),
        "entry param compound type fact must be present in the function-owned map"
    );
    assert_eq!(
        entry.args[0].ty, expected,
        "entry param argument must carry the structured compound type"
    );
}

#[test]
fn abi_i64_param_type_is_not_a_semantic_int_fact() {
    let func_ir = FunctionIR {
        name: "boxed_carrier".to_string(),
        params: vec!["obj".to_string()],
        ops: vec![op_args("ret", &["obj"])],
        param_types: Some(vec!["i64".to_string()]),
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);

    assert_eq!(tir.param_types, vec![TirType::DynBox]);
    let entry = &tir.blocks[&tir.entry_block];
    assert_eq!(
        tir.value_types.get(&entry.args[0].id),
        Some(&TirType::DynBox),
        "native ABI carrier `i64` must stay a boxed dynamic value, not semantic I64"
    );
}

#[test]
fn exception_region_drop_marker_round_trips_without_full_drop_gate() {
    let func_ir = FunctionIR {
        name: "exception_marker_transport".to_string(),
        params: vec![],
        ops: vec![
            op(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            op("ret_void"),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);

    assert!(matches!(
        tir.attrs
            .get(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(crate::tir::ops::AttrValue::Bool(true))
    ));
    assert!(
        !tir.attrs
            .contains_key(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
        "exception-only marker must not be promoted to the native full-RC gate"
    );
    assert!(
        tir.blocks[&tir.entry_block]
            .ops
            .iter()
            .all(|op| op.opcode != OpCode::Copy),
        "transport marker must be stripped before TIR op assembly"
    );
}

// =======================================================================
// Test 6: string_to_tir_type coverage
// =======================================================================
#[test]
fn string_type_conversion() {
    assert_eq!(string_to_tir_type("int"), TirType::I64);
    assert_eq!(string_to_tir_type("i64"), TirType::I64);
    assert_eq!(string_to_tir_type("float"), TirType::F64);
    assert_eq!(string_to_tir_type("f64"), TirType::F64);
    assert_eq!(string_to_tir_type("bool"), TirType::Bool);
    assert_eq!(string_to_tir_type("str"), TirType::Str);
    assert_eq!(string_to_tir_type("bytes"), TirType::Bytes);
    assert_eq!(string_to_tir_type("None"), TirType::None);
    assert_eq!(string_to_tir_type("none"), TirType::None);
    assert_eq!(
        string_to_tir_type("list[int]"),
        TirType::List(Box::new(TirType::I64))
    );
    assert_eq!(
        string_to_tir_type("dict[str, float]"),
        TirType::Dict(Box::new(TirType::Str), Box::new(TirType::F64))
    );
    assert_eq!(string_to_tir_type("unknown_type"), TirType::DynBox);
}
