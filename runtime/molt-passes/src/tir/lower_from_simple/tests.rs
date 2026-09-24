//! Tests for SimpleIR to TIR lowering.

use super::*;
use crate::ir::{FunctionIR, OpIR};
use crate::tir::blocks::Terminator;
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;

/// Helper: build a FunctionIR with given name, params, and ops.
fn make_func(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: params.iter().map(|s| s.to_string()).collect(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }
}

#[test]
fn extern_declarations_keep_authored_return_contract_without_a_body() {
    for (return_abi, expected_type) in [
        (molt_ir::FunctionReturnAbi::Value, TirType::DynBox),
        (molt_ir::FunctionReturnAbi::Void, TirType::None),
    ] {
        let mut ir = make_func("extern_declaration", &["argument"], vec![]);
        ir.is_extern = true;
        ir.return_abi = return_abi;
        let tir = lower_to_tir(&ir);
        assert_eq!(tir.return_abi, return_abi);
        assert_eq!(tir.return_type, expected_type);
        assert_eq!(tir.param_names, ir.params);
        assert_eq!(tir.param_types, vec![TirType::DynBox]);
        assert!(
            tir.blocks.is_empty(),
            "declaration must not acquire a synthetic body"
        );
        assert!(tir.value_types.is_empty());
        assert!(ir.ops.is_empty());
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

    ops[6].source_op_idx = Some(41);
    ops[6].source_line = Some(12);
    ops[7].source_op_idx = Some(42);
    ops[7].source_line = Some(13);

    assert!(rewrite_cell_locals_to_store_load(&mut ops));
    assert_eq!(ops[6].kind, "store_var");
    assert_eq!(ops[6].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[7].kind, "load_var");
    assert_eq!(ops[7].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[6].source_op_idx, Some(41));
    assert_eq!(ops[6].source_line, Some(12));
    assert_eq!(ops[7].source_op_idx, Some(42));
    assert_eq!(ops[7].source_line, Some(13));
    assert_eq!(ops[9].kind, "store_var");
    assert_eq!(ops[9].var.as_deref(), Some("_cell_cell_b_0"));
    assert_eq!(ops[10].kind, "load_var");
    assert_eq!(ops[10].var.as_deref(), Some("_cell_cell_b_0"));
}

#[test]
fn loop_index_rewrite_preserves_source_identity_on_derived_ops() {
    let ops = vec![
        op_val_out("const", 0, "initial"),
        op("loop_start"),
        OpIR {
            kind: "loop_index_start".into(),
            args: Some(vec!["initial".into()]),
            out: Some("index".into()),
            source_op_idx: Some(73),
            source_line: Some(21),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_index_next".into(),
            args: Some(vec!["index".into()]),
            out: Some("index".into()),
            source_op_idx: Some(74),
            source_line: Some(22),
            ..OpIR::default()
        },
        op("loop_continue"),
        op("loop_end"),
    ];

    let rewritten = rewrite_loop_index_to_store_load(&ops);

    assert_eq!(rewritten[1].kind, "store_var");
    assert_eq!(rewritten[1].source_op_idx, Some(73));
    assert_eq!(rewritten[1].source_line, Some(21));
    assert_eq!(rewritten[3].kind, "load_var");
    assert_eq!(rewritten[3].source_op_idx, Some(73));
    assert_eq!(rewritten[3].source_line, Some(21));
    assert_eq!(rewritten[4].kind, "store_var");
    assert_eq!(rewritten[4].source_op_idx, Some(74));
    assert_eq!(rewritten[4].source_line, Some(22));
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
    // Only the authored ConstInt and Add; no unused SSA undef materialization.
    assert_eq!(
        entry.ops.len(),
        2,
        "entry should have only const and add ops"
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
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
        codegen_partition: false,
        execution_context: Default::default(),
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

#[test]
fn source_origins_survive_removed_markers_without_mutating_the_input() {
    for explicit in [None, Some(91)] {
        let mut constant = op_val_out("const", 7, "value");
        constant.source_op_idx = explicit;
        let func = make_func(
            "source_origin_before_marker_removal",
            &[],
            vec![op("drop_inserted"), constant, op_args("ret", &["value"])],
        );
        let tir = lower_to_tir(&func);
        let constant = tir
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find(|op| op.opcode == OpCode::ConstInt)
            .unwrap();
        assert_eq!(
            constant.source_op_index(),
            Some(explicit.unwrap_or(1) as usize)
        );
        assert_eq!(func.ops[1].source_op_idx, explicit);
        assert_eq!(func.ops[0].source_op_idx, None);
    }
}

#[test]
fn predicate_return_contracts_require_exact_not_annotated_operands() {
    for kind in ["eq", "ne", "lt", "le", "gt", "ge"] {
        let mut source = make_func(
            "annotated_predicate",
            &["left", "right"],
            vec![
                op_args_out(kind, &["left", "right"], "predicate"),
                op_args_out("copy", &["predicate"], "copied"),
                op_args("ret", &["copied"]),
            ],
        );
        source.param_types = Some(vec!["int".into(), "int".into()]);
        let mut tir = lower_to_tir(&source);
        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "{kind}: initial return ABI"
        );
        // Refinement must also repair stale caller-visible return metadata.
        tir.return_type = TirType::Bool;
        crate::tir::type_refine::refine_types(&mut tir);
        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "{kind}: refined return ABI"
        );

        let exact = make_func(
            "exact_predicate",
            &[],
            vec![
                op_val_out("const", 1, "left"),
                op_val_out("const", 2, "right"),
                op_args_out(kind, &["left", "right"], "predicate"),
                op_args_out("copy", &["predicate"], "copied"),
                op_args("ret", &["copied"]),
            ],
        );
        assert_eq!(lower_to_tir(&exact).return_type, TirType::Bool);
    }
}

#[test]
fn arithmetic_return_contracts_require_exact_not_annotated_operands() {
    for (kind, exact_result) in [
        ("add", TirType::I64),
        ("mul", TirType::I64),
        ("div", TirType::F64),
        ("pow", TirType::DynBox),
    ] {
        for annotation in ["int", "float"] {
            let mut source = make_func(
                "annotated_arithmetic",
                &["left", "right"],
                vec![
                    op_args_out(kind, &["left", "right"], "result"),
                    op_args_out("copy", &["result"], "copied"),
                    op_args("ret", &["copied"]),
                ],
            );
            source.param_types = Some(vec![annotation.into(), annotation.into()]);
            let mut tir = lower_to_tir(&source);
            assert_eq!(
                tir.return_type,
                TirType::DynBox,
                "{annotation} {kind}: initial ABI"
            );
            tir.return_type = TirType::F64;
            crate::tir::type_refine::refine_types(&mut tir);
            assert_eq!(
                tir.return_type,
                TirType::DynBox,
                "{annotation} {kind}: refined ABI"
            );
        }
        let source = make_func(
            "exact_arithmetic",
            &[],
            vec![
                op_val_out("const", 1, "left"),
                op_val_out("const", 2, "right"),
                op_args_out(kind, &["left", "right"], "result"),
                op_args_out("copy", &["result"], "copied"),
                op_args("ret", &["copied"]),
            ],
        );
        let mut tir = lower_to_tir(&source);
        assert_eq!(tir.return_type, exact_result, "{kind}: exact initial ABI");
        crate::tir::type_refine::refine_types(&mut tir);
        assert_eq!(tir.return_type, exact_result, "{kind}: exact refined ABI");
    }
}

#[test]
fn annotated_parameter_returns_do_not_promise_exact_scalar_abi() {
    for annotation in ["int", "float", "bool", "str", "bytes"] {
        let mut source = make_func(
            "annotated_identity",
            &["value"],
            vec![
                op_args_out("copy", &["value"], "copied"),
                op_args("ret", &["copied"]),
            ],
        );
        source.param_types = Some(vec![annotation.into()]);
        let mut tir = lower_to_tir(&source);
        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "{annotation}: initial ABI"
        );
        crate::tir::type_refine::refine_types(&mut tir);
        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "{annotation}: refined ABI"
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
    assert!(crate::tir::type_refine::extract_exact_scalar_map(&tir).is_empty());
    assert!(crate::tir::type_refine::extract_proven_map(&tir).is_empty());
}

#[test]
fn phase_only_functions_preserve_markers_without_phantom_exact_facts() {
    use crate::tir::passes::drop_insertion::{
        DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR,
    };
    for markers in [
        vec![DROP_INSERTED_ATTR],
        vec![EXCEPTION_REGION_DROPS_INSERTED_ATTR],
        vec![DROP_INSERTED_ATTR, EXCEPTION_REGION_DROPS_INSERTED_ATTR],
    ] {
        let func_ir = make_func(
            "phase_only",
            &[],
            markers.iter().map(|name| op(name)).collect(),
        );
        let tir = lower_to_tir(&func_ir);
        assert!(tir.blocks.is_empty());
        assert!(crate::tir::type_refine::extract_exact_scalar_map(&tir).is_empty());
        assert!(crate::tir::type_refine::extract_proven_map(&tir).is_empty());
        for marker in markers {
            assert_eq!(
                tir.attrs.get(marker),
                Some(&crate::tir::ops::AttrValue::Bool(true))
            );
        }
    }
}

// =======================================================================
// Test 5: Function with param_types annotation
// =======================================================================
#[test]
fn param_types_from_annotation() {
    let func_ir = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "typed_add".to_string(),
        params: vec!["a".to_string(), "b".to_string()],
        ops: vec![op_args_out("add", &["a", "b"], "c"), op_args("ret", &["c"])],
        param_types: Some(vec!["int".to_string(), "float".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
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
        Some(&TirType::DynBox),
        "annotations must not seed exact arithmetic result facts during lifting"
    );
    assert_eq!(tir.return_type, TirType::DynBox);
}

#[test]
fn compound_param_types_from_annotation() {
    let func_ir = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "typed_container".to_string(),
        params: vec!["items".to_string()],
        ops: vec![op_args("ret", &["items"])],
        param_types: Some(vec!["list[int]".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
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
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "boxed_carrier".to_string(),
        params: vec!["obj".to_string()],
        ops: vec![op_args("ret", &["obj"])],
        param_types: Some(vec!["i64".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
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
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "exception_marker_transport".to_string(),
        params: vec![],
        ops: vec![
            op(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            op("ret_void"),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
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

// Physical partitions survive the actual TIR artifact cache, without granting
// inherited Python-frame access or relying on reserved-looking function names.
#[test]
fn codegen_partition_survives_tir_artifact_roundtrip() {
    for partitioned in [false, true] {
        let mut input = make_func("__molt_chunk_v1_user", &[], vec![op("ret_void")]);
        input.codegen_partition = partitioned;
        let tir = lower_to_tir(&input);
        assert_eq!(tir.is_codegen_partition(), partitioned);
        assert_eq!(
            tir.execution_context,
            crate::ir::ExecutionContextPolicy::None
        );
        let bytes = crate::tir::serialize::serialize_tir_function(&tir).unwrap();
        let restored = crate::tir::serialize::deserialize_tir_function(&bytes).unwrap();
        assert_eq!(restored.is_codegen_partition(), partitioned);
        assert_eq!(
            restored.execution_context,
            crate::ir::ExecutionContextPolicy::None
        );
    }
}
