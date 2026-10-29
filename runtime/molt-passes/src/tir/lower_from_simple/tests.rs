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
        parameter_custody: Vec::new(),
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
    let mut ext = make_func("ext", &[], vec![op(molt_ir::OpKind::RetVoid)]);
    ext.is_extern = true;
    let funcs = vec![
        make_func("a", &[], vec![op(molt_ir::OpKind::RetVoid)]),
        ext,
        make_func("b", &[], vec![op(molt_ir::OpKind::RetVoid)]),
    ];
    let (module, idx_map) = lower_functions_to_tir_module(&funcs);
    assert_eq!(module.functions.len(), 2, "externs are skipped");
    assert_eq!(idx_map, vec![0, 2], "module position maps to source index");
    assert_eq!(module.functions[0].name, "a");
    assert_eq!(module.functions[1].name, "b");
}

/// Helper to create an `OpIR` with just a `kind`.
fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR {
        kind,
        ..OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind`, `value`, and `out`.
fn op_val_out(kind: molt_ir::OpKind, value: i64, out: &str) -> OpIR {
    OpIR {
        kind,
        value: Some(value),
        out: Some(out.to_string()),
        ..OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind`, `args`, and `out`.
fn op_args_out(kind: molt_ir::OpKind, args: &[&str], out: &str) -> OpIR {
    OpIR {
        kind,
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        out: Some(out.to_string()),
        ..OpIR::new(kind)
    }
}

/// Helper to create an `OpIR` with `kind` and `args`.
fn op_args(kind: molt_ir::OpKind, args: &[&str]) -> OpIR {
    OpIR {
        kind,
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        ..OpIR::new(kind)
    }
}

#[test]
fn cell_rewrite_skips_cells_escaped_into_closure_tuple() {
    let mut ops = vec![
        op_args_out(molt_ir::OpKind::Missing, &[], "missing"),
        op_args_out(molt_ir::OpKind::ListNew, &["missing"], "cell"),
        op_val_out(molt_ir::OpKind::Const, 0, "zero"),
        op_val_out(molt_ir::OpKind::Const, 7, "value"),
        op_args(molt_ir::OpKind::StoreIndex, &["cell", "zero", "value"]),
        op_args_out(molt_ir::OpKind::TupleNew, &["cell"], "closure"),
        op_args_out(molt_ir::OpKind::Index, &["cell", "zero"], "loaded"),
    ];

    assert!(!rewrite_cell_locals_to_store_load(&mut ops));
    assert_eq!(ops[4].kind, molt_ir::OpKind::StoreIndex);
    assert_eq!(ops[6].kind, molt_ir::OpKind::Index);
}

#[test]
fn cell_rewrite_handles_multiple_unescaped_cells_independently() {
    let mut ops = vec![
        op_args_out(molt_ir::OpKind::Missing, &[], "missing_a"),
        op_args_out(molt_ir::OpKind::ListNew, &["missing_a"], "cell_a"),
        op_args_out(molt_ir::OpKind::Missing, &[], "missing_b"),
        op_args_out(molt_ir::OpKind::ListNew, &["missing_b"], "cell_b"),
        op_val_out(molt_ir::OpKind::Const, 0, "zero"),
        op_val_out(molt_ir::OpKind::Const, 1, "value_a"),
        op_args(molt_ir::OpKind::StoreIndex, &["cell_a", "zero", "value_a"]),
        op_args_out(molt_ir::OpKind::Index, &["cell_a", "zero"], "loaded_a"),
        op_val_out(molt_ir::OpKind::Const, 2, "value_b"),
        op_args(molt_ir::OpKind::StoreIndex, &["cell_b", "zero", "value_b"]),
        op_args_out(molt_ir::OpKind::Index, &["cell_b", "zero"], "loaded_b"),
    ];

    ops[6].source_op_idx = Some(41);
    ops[6].source_line = Some(12);
    ops[7].source_op_idx = Some(42);
    ops[7].source_line = Some(13);

    assert!(rewrite_cell_locals_to_store_load(&mut ops));
    assert_eq!(ops[6].kind, molt_ir::OpKind::StoreVar);
    assert_eq!(ops[6].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[7].kind, molt_ir::OpKind::LoadVar);
    assert_eq!(ops[7].var.as_deref(), Some("_cell_cell_a_0"));
    assert_eq!(ops[6].source_op_idx, Some(41));
    assert_eq!(ops[6].source_line, Some(12));
    assert_eq!(ops[7].source_op_idx, Some(42));
    assert_eq!(ops[7].source_line, Some(13));
    assert_eq!(ops[9].kind, molt_ir::OpKind::StoreVar);
    assert_eq!(ops[9].var.as_deref(), Some("_cell_cell_b_0"));
    assert_eq!(ops[10].kind, molt_ir::OpKind::LoadVar);
    assert_eq!(ops[10].var.as_deref(), Some("_cell_cell_b_0"));
}

#[test]
fn loop_index_rewrite_preserves_source_identity_on_derived_ops() {
    let ops = vec![
        op_val_out(molt_ir::OpKind::Const, 0, "initial"),
        op(molt_ir::OpKind::LoopStart),
        OpIR {
            kind: molt_ir::OpKind::LoopIndexStart,
            args: Some(vec!["initial".into()]),
            out: Some("index".into()),
            source_op_idx: Some(73),
            source_line: Some(21),
            ..OpIR::new(molt_ir::OpKind::LoopIndexStart)
        },
        OpIR {
            kind: molt_ir::OpKind::LoopIndexNext,
            args: Some(vec!["index".into()]),
            out: Some("index".into()),
            source_op_idx: Some(74),
            source_line: Some(22),
            ..OpIR::new(molt_ir::OpKind::LoopIndexNext)
        },
        op(molt_ir::OpKind::LoopContinue),
        op(molt_ir::OpKind::LoopEnd),
    ];

    let rewritten = normalize_loop_indices("source_identity", ops);

    assert_eq!(rewritten[1].kind, molt_ir::OpKind::StoreVar);
    assert_eq!(rewritten[1].source_op_idx, Some(73));
    assert_eq!(rewritten[1].source_line, Some(21));
    assert_eq!(rewritten[3].kind, molt_ir::OpKind::LoadVar);
    assert_eq!(rewritten[3].source_op_idx, Some(73));
    assert_eq!(rewritten[3].source_line, Some(21));
    assert_eq!(rewritten[4].kind, molt_ir::OpKind::StoreVar);
    assert_eq!(rewritten[4].source_op_idx, Some(74));
    assert_eq!(rewritten[4].source_line, Some(22));
}

#[test]
fn loop_normalization_reuses_unchanged_owned_allocation() {
    let mut ops = Vec::with_capacity(19);
    ops.extend([
        op_val_out(molt_ir::OpKind::Const, 42, "answer"),
        op_args(molt_ir::OpKind::Ret, &["answer"]),
    ]);
    let allocation = ops.as_ptr();
    let capacity = ops.capacity();
    let normalized = normalize_loop_indices("unchanged", ops);
    assert_eq!(
        normalized.as_ptr(),
        allocation,
        "no-op normalization must not clone the stream"
    );
    assert_eq!(normalized.capacity(), capacity);
    assert_eq!(normalized.len(), 2);
    let empty = Vec::with_capacity(7);
    let allocation = empty.as_ptr();
    let normalized = normalize_loop_indices("empty", empty);
    assert!(normalized.is_empty());
    assert_eq!(normalized.as_ptr(), allocation);
    assert_eq!(normalized.capacity(), 7);
}

#[test]
fn loop_normalization_keeps_nested_initializers_outside_their_own_latches() {
    let marked = |kind: molt_ir::OpKind, args: &[&str], out: Option<&str>, origin| OpIR {
        kind,
        args: Some(args.iter().map(|arg| (*arg).into()).collect()),
        out: out.map(str::to_string),
        source_op_idx: Some(origin),
        ..OpIR::new(kind)
    };
    let ops = normalize_loop_indices(
        "nested",
        vec![
            marked(molt_ir::OpKind::LoopStart, &[], None, 11),
            marked(
                molt_ir::OpKind::LoopIndexStart,
                &["outer_seed"],
                Some("outer"),
                12,
            ),
            marked(molt_ir::OpKind::LoopBreakIfFalse, &["outer_ok"], None, 13),
            marked(molt_ir::OpKind::LoopStart, &[], None, 21),
            marked(
                molt_ir::OpKind::LoopIndexStart,
                &["inner_seed"],
                Some("inner"),
                22,
            ),
            marked(molt_ir::OpKind::LoopBreakIfFalse, &["inner_ok"], None, 23),
            marked(
                molt_ir::OpKind::LoopIndexNext,
                &["inner_updated"],
                Some("inner"),
                24,
            ),
            marked(molt_ir::OpKind::LoopContinue, &[], None, 25),
            marked(molt_ir::OpKind::LoopEnd, &[], None, 26),
            marked(
                molt_ir::OpKind::LoopIndexNext,
                &["outer_updated"],
                Some("outer"),
                31,
            ),
            marked(molt_ir::OpKind::LoopContinue, &[], None, 32),
            marked(molt_ir::OpKind::LoopEnd, &[], None, 33),
            marked(molt_ir::OpKind::RetVoid, &[], None, 40),
        ],
    );
    // Independent graph authority establishes where recurrence executes. Do not
    // compare the normalizer's complete output with a second rewritten-op list.
    let flow = molt_ir::simple_verify::simple_ir_logical_flow(&ops);
    for (carrier, seed, updated, header_origin, init_origin, update_origin, latch_origin) in [
        ("outer", "outer_seed", "outer_updated", 11, 12, 31, 32),
        ("inner", "inner_seed", "inner_updated", 21, 22, 24, 25),
    ] {
        let find = |kind: molt_ir::OpKind, origin| {
            ops.iter()
                .position(|op| op.kind == kind && op.source_op_idx == Some(origin))
                .unwrap()
        };
        let initialization = find(molt_ir::OpKind::StoreVar, init_origin);
        let header = find(molt_ir::OpKind::LoopStart, header_origin);
        let read = find(molt_ir::OpKind::LoadVar, init_origin);
        let update = find(molt_ir::OpKind::StoreVar, update_origin);
        let latch = find(molt_ir::OpKind::LoopContinue, latch_origin);
        assert!(initialization < header && header < read && read < update && update < latch);
        assert_eq!(ops[initialization].var.as_deref(), Some(carrier));
        assert_eq!(
            ops[initialization].args.as_deref(),
            Some(&[seed.to_string()][..])
        );
        assert_eq!(ops[read].var.as_deref(), Some(carrier));
        assert_eq!(ops[update].var.as_deref(), Some(carrier));
        assert_eq!(
            ops[update].args.as_deref(),
            Some(&[updated.to_string()][..])
        );
        assert_eq!(flow.edges[latch].len(), 1);
        assert_eq!(
            flow.edges[latch][0].target, header,
            "latch must reload, never reinitialize"
        );
        assert_eq!(flow.edges[header][0].target, read);
    }
}

#[test]
fn loop_normalization_preserves_outer_update_and_explicit_outer_continue() {
    for inner_counted in [false, true] {
        let mut ops = vec![
            op_val_out(molt_ir::OpKind::Const, 7, "seed"),
            op_val_out(molt_ir::OpKind::Const, 1, "one"),
            op_val_out(molt_ir::OpKind::Const, 10, "limit"),
            op(molt_ir::OpKind::LoopStart),
            op_args_out(molt_ir::OpKind::LoopIndexStart, &["seed"], "outer"),
            op_args_out(molt_ir::OpKind::Lt, &["outer", "limit"], "condition"),
            op_args(molt_ir::OpKind::LoopBreakIfFalse, &["condition"]),
            op(molt_ir::OpKind::LoopStart),
        ];
        if inner_counted {
            ops.push(op_args_out(
                molt_ir::OpKind::LoopIndexStart,
                &["seed"],
                "inner",
            ));
        }
        ops.extend([
            op_args_out(molt_ir::OpKind::Add, &["outer", "one"], "advanced"),
            OpIR {
                source_op_idx: Some(73),
                ..op_args_out(molt_ir::OpKind::LoopIndexNext, &["advanced"], "outer")
            },
            OpIR {
                value: Some(99),
                ..op(molt_ir::OpKind::Jump)
            },
            op(molt_ir::OpKind::LoopEnd),
            OpIR {
                value: Some(99),
                ..op(molt_ir::OpKind::Label)
            },
            op(molt_ir::OpKind::LoopContinue),
            op(molt_ir::OpKind::LoopEnd),
            op_args(molt_ir::OpKind::Ret, &["outer"]),
        ]);
        let normalized = normalize_loop_indices("outer_transfer", ops);
        let flow = molt_ir::simple_verify::simple_ir_logical_flow(&normalized);
        // The authored recurrence is 7,8,9 then return 10. Its update is
        // physically inside a younger loop, but both the carried value and
        // explicit transfer belong to the outer loop. Inspect these independent
        // data/control relations rather than copying a rewritten operation list.
        let update = normalized
            .iter()
            .position(|op| op.source_op_idx == Some(73))
            .unwrap();
        assert_eq!(normalized[update].kind, molt_ir::OpKind::StoreVar);
        assert_eq!(normalized[update].var.as_deref(), Some("outer"));
        assert_eq!(
            normalized[update].args.as_deref(),
            Some(&["advanced".to_string()][..])
        );
        let increment = normalized
            .iter()
            .find(|op| op.out.as_deref() == Some("advanced"))
            .unwrap();
        assert_eq!(increment.kind, molt_ir::OpKind::Add);
        assert_eq!(
            increment.args.as_deref(),
            Some(&["outer".to_string(), "one".to_string()][..])
        );
        let jump = flow.edges[update][0].target;
        assert_eq!(normalized[jump].kind, molt_ir::OpKind::Jump);
        let target = flow.edges[jump][0].target;
        assert_eq!(normalized[target].kind, molt_ir::OpKind::Label);
        assert_eq!(normalized[target].value, Some(99));
        let outer_latch = flow.edges[target][0].target;
        assert_eq!(normalized[outer_latch].kind, molt_ir::OpKind::LoopContinue);
        let outer_header = flow.edges[outer_latch][0].target;
        assert_eq!(normalized[outer_header].kind, molt_ir::OpKind::LoopStart);
        let reload = flow.edges[outer_header][0].target;
        assert_eq!(normalized[reload].kind, molt_ir::OpKind::LoadVar);
        assert_eq!(normalized[reload].var.as_deref(), Some("outer"));
        assert!(outer_header < update);
        assert_eq!(flow.edges[update].len(), 1);
        assert_eq!(
            flow.edges[jump].len(),
            1,
            "outer continue cannot fall through the inner closer"
        );
        if inner_counted {
            assert_eq!(
                normalized
                    .iter()
                    .filter(|op| {
                        op.kind == molt_ir::OpKind::StoreVar && op.var.as_deref() == Some("inner")
                    })
                    .count(),
                1,
                "the outer update must not mutate the inner seed"
            );
        }
    }
}

#[test]
fn loop_normalization_rejects_orphan_and_malformed_carriers() {
    let carrier = |kind: molt_ir::OpKind, args: Option<Vec<&str>>, out: Option<&str>| OpIR {
        kind,
        args: args.map(|args| args.into_iter().map(str::to_string).collect()),
        out: out.map(str::to_string),
        source_op_idx: Some(73),
        ..OpIR::new(kind)
    };
    let start = || {
        carrier(
            molt_ir::OpKind::LoopIndexStart,
            Some(vec!["seed"]),
            Some("index"),
        )
    };
    let next = || {
        carrier(
            molt_ir::OpKind::LoopIndexNext,
            Some(vec!["updated"]),
            Some("index"),
        )
    };
    let mut cases = vec![
        vec![start()],
        vec![next()],
        vec![
            op(molt_ir::OpKind::LoopStart),
            next(),
            op(molt_ir::OpKind::LoopEnd),
        ],
        // A closed loop no longer owns a carrier, even if its spelling matches.
        vec![
            op(molt_ir::OpKind::LoopStart),
            start(),
            op(molt_ir::OpKind::LoopEnd),
            next(),
        ],
        // An orphan with a previously valid spelling has no lexical owner.
        vec![
            op(molt_ir::OpKind::LoopStart),
            start(),
            op(molt_ir::OpKind::LoopEnd),
            start(),
        ],
        vec![
            op(molt_ir::OpKind::LoopStart),
            start(),
            start(),
            op(molt_ir::OpKind::LoopEnd),
        ],
        vec![
            op(molt_ir::OpKind::LoopStart),
            start(),
            carrier(
                molt_ir::OpKind::LoopIndexNext,
                Some(vec!["updated"]),
                Some("other"),
            ),
            op(molt_ir::OpKind::LoopEnd),
        ],
    ];
    for malformed in [
        carrier(molt_ir::OpKind::LoopIndexStart, None, Some("index")),
        carrier(molt_ir::OpKind::LoopIndexStart, Some(vec![]), Some("index")),
        carrier(
            molt_ir::OpKind::LoopIndexStart,
            Some(vec!["seed", "extra"]),
            Some("index"),
        ),
        carrier(
            molt_ir::OpKind::LoopIndexStart,
            Some(vec![""]),
            Some("index"),
        ),
        carrier(molt_ir::OpKind::LoopIndexStart, Some(vec!["seed"]), None),
        carrier(
            molt_ir::OpKind::LoopIndexStart,
            Some(vec!["seed"]),
            Some("none"),
        ),
    ] {
        cases.push(vec![
            op(molt_ir::OpKind::LoopStart),
            malformed,
            op(molt_ir::OpKind::LoopEnd),
        ]);
    }
    for malformed in [
        carrier(molt_ir::OpKind::LoopIndexNext, None, Some("index")),
        carrier(molt_ir::OpKind::LoopIndexNext, Some(vec![]), Some("index")),
        carrier(
            molt_ir::OpKind::LoopIndexNext,
            Some(vec!["updated", "extra"]),
            Some("index"),
        ),
        carrier(
            molt_ir::OpKind::LoopIndexNext,
            Some(vec![""]),
            Some("index"),
        ),
        carrier(molt_ir::OpKind::LoopIndexNext, Some(vec!["updated"]), None),
    ] {
        cases.push(vec![
            op(molt_ir::OpKind::LoopStart),
            start(),
            malformed,
            op(molt_ir::OpKind::LoopEnd),
        ]);
    }
    for ops in cases {
        let error = std::panic::catch_unwind(|| normalize_loop_indices("malformed", ops))
            .expect_err("malformed carriers cannot reach SSA passthrough");
        let message = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .expect("text diagnostic");
        assert!(message.contains("function `malformed`"), "{message}");
        assert!(
            message.contains("loop_index_") && message.contains("source op#73"),
            "{message}"
        );
        assert!(message.contains("pre-SSA operation remains"), "{message}");
    }
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
            op_val_out(molt_ir::OpKind::Const, 1, "x"),
            op_args_out(molt_ir::OpKind::Add, &["x"], "y"),
            op_args(molt_ir::OpKind::Ret, &["y"]),
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
            op_val_out(molt_ir::OpKind::Const, 0, "c"), // 0 entry
            op_args(molt_ir::OpKind::If, &["c"]),       // 1 ends entry
            op_val_out(molt_ir::OpKind::Const, 1, "x"), // 2 then
            op(molt_ir::OpKind::Else),                  // 3 else
            op_val_out(molt_ir::OpKind::Const, 2, "x"), // 4 else body
            op(molt_ir::OpKind::EndIf),                 // 5 join
            op_args(molt_ir::OpKind::Ret, &["x"]),      // 6 return
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
                kind: molt_ir::OpKind::Line,
                value: Some(7),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("builtins".to_string()),
                out: Some("v62".to_string()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            op_args_out(molt_ir::OpKind::ModuleImport, &["v62"], "v63"),
            OpIR {
                kind: molt_ir::OpKind::CheckException,
                value: Some(3),
                ..OpIR::new(molt_ir::OpKind::CheckException)
            },
            OpIR {
                kind: molt_ir::OpKind::ConstStr,
                s_value: Some("_builtins".to_string()),
                out: Some("v64".to_string()),
                ..OpIR::new(molt_ir::OpKind::ConstStr)
            },
            OpIR {
                kind: molt_ir::OpKind::ModuleSetAttr,
                args: Some(vec![
                    "__molt_module_obj__".to_string(),
                    "v64".to_string(),
                    "v63".to_string(),
                ]),
                out: Some("none".to_string()),
                ..OpIR::new(molt_ir::OpKind::ModuleSetAttr)
            },
            op(molt_ir::OpKind::RetVoid),
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
                kind: molt_ir::OpKind::GpuThreadId,
                out: Some("tid".to_string()),
                ..OpIR::new(molt_ir::OpKind::GpuThreadId)
            },
            op_args(molt_ir::OpKind::Ret, &["tid"]),
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
    assert_eq!(call_op.origin, Some(molt_ir::OpKind::GpuThreadId));
}

#[test]
fn source_origins_survive_removed_markers_without_mutating_the_input() {
    for explicit in [None, Some(91)] {
        let mut constant = op_val_out(molt_ir::OpKind::Const, 7, "value");
        constant.source_op_idx = explicit;
        let func = make_func(
            "source_origin_before_marker_removal",
            &[],
            vec![
                op(molt_ir::OpKind::DropInserted),
                constant,
                op_args(molt_ir::OpKind::Ret, &["value"]),
            ],
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
    for kind in [
        molt_ir::OpKind::Eq,
        molt_ir::OpKind::Ne,
        molt_ir::OpKind::Lt,
        molt_ir::OpKind::Le,
        molt_ir::OpKind::Gt,
        molt_ir::OpKind::Ge,
    ] {
        let mut source = make_func(
            "annotated_predicate",
            &["left", "right"],
            vec![
                op_args_out(kind, &["left", "right"], "predicate"),
                op_args_out(molt_ir::OpKind::Copy, &["predicate"], "copied"),
                op_args(molt_ir::OpKind::Ret, &["copied"]),
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
                op_val_out(molt_ir::OpKind::Const, 1, "left"),
                op_val_out(molt_ir::OpKind::Const, 2, "right"),
                op_args_out(kind, &["left", "right"], "predicate"),
                op_args_out(molt_ir::OpKind::Copy, &["predicate"], "copied"),
                op_args(molt_ir::OpKind::Ret, &["copied"]),
            ],
        );
        assert_eq!(lower_to_tir(&exact).return_type, TirType::Bool);
    }
}

#[test]
fn arithmetic_return_contracts_require_exact_not_annotated_operands() {
    for (kind, exact_result) in [
        (molt_ir::OpKind::Add, TirType::I64),
        (molt_ir::OpKind::Mul, TirType::I64),
        (molt_ir::OpKind::Div, TirType::F64),
        (molt_ir::OpKind::Pow, TirType::DynBox),
    ] {
        for annotation in ["int", "float"] {
            let mut source = make_func(
                "annotated_arithmetic",
                &["left", "right"],
                vec![
                    op_args_out(kind, &["left", "right"], "result"),
                    op_args_out(molt_ir::OpKind::Copy, &["result"], "copied"),
                    op_args(molt_ir::OpKind::Ret, &["copied"]),
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
                op_val_out(molt_ir::OpKind::Const, 1, "left"),
                op_val_out(molt_ir::OpKind::Const, 2, "right"),
                op_args_out(kind, &["left", "right"], "result"),
                op_args_out(molt_ir::OpKind::Copy, &["result"], "copied"),
                op_args(molt_ir::OpKind::Ret, &["copied"]),
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
                op_args_out(molt_ir::OpKind::Copy, &["value"], "copied"),
                op_args(molt_ir::OpKind::Ret, &["copied"]),
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
        vec![(molt_ir::OpKind::DropInserted, DROP_INSERTED_ATTR)],
        vec![(
            molt_ir::OpKind::ExceptionRegionDropsInserted,
            EXCEPTION_REGION_DROPS_INSERTED_ATTR,
        )],
        vec![
            (molt_ir::OpKind::DropInserted, DROP_INSERTED_ATTR),
            (
                molt_ir::OpKind::ExceptionRegionDropsInserted,
                EXCEPTION_REGION_DROPS_INSERTED_ATTR,
            ),
        ],
    ] {
        let func_ir = make_func(
            "phase_only",
            &[],
            markers.iter().map(|(kind, _)| op(*kind)).collect(),
        );
        let tir = lower_to_tir(&func_ir);
        assert!(tir.blocks.is_empty());
        assert!(crate::tir::type_refine::extract_exact_scalar_map(&tir).is_empty());
        assert!(crate::tir::type_refine::extract_proven_map(&tir).is_empty());
        for (_, marker) in markers {
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
        ops: vec![
            op_args_out(molt_ir::OpKind::Add, &["a", "b"], "c"),
            op_args(molt_ir::OpKind::Ret, &["c"]),
        ],
        param_types: Some(vec!["int".to_string(), "float".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
        ops: vec![op_args(molt_ir::OpKind::Ret, &["items"])],
        param_types: Some(vec!["list[int]".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
        ops: vec![op_args(molt_ir::OpKind::Ret, &["obj"])],
        param_types: Some(vec!["i64".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
            op(molt_ir::OpKind::ExceptionRegionDropsInserted),
            op(molt_ir::OpKind::RetVoid),
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
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
        let mut input = make_func(
            "__molt_chunk_v1_user",
            &[],
            vec![op(molt_ir::OpKind::RetVoid)],
        );
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

// Entry custody is a typed wire fact. The lift projects it for the ownership
// passes, the TIR artifact cache keeps it, and a declaration with no body
// answers it from its signature.
#[test]
fn parameter_custody_survives_tir_artifact_roundtrip() {
    use crate::ir::ParameterCustody::{Borrowed, Transferred};
    let mut body = make_func(
        "owns_second",
        &["first", "second"],
        vec![op(molt_ir::OpKind::RetVoid)],
    );
    body.parameter_custody = vec![Borrowed, Transferred];
    let mut declaration = body.clone();
    declaration.ops.clear();
    declaration.is_extern = true;
    for source in [&body, &declaration] {
        let tir = lower_to_tir(source);
        assert_eq!(
            [tir.parameter_custody(0), tir.parameter_custody(1)],
            [Borrowed, Transferred],
            "{}",
            source.name
        );
        assert_eq!(
            tir.attrs.get(crate::tir::function::PARAMETER_CUSTODY_ATTR),
            Some(&crate::tir::ops::AttrValue::Bytes(vec![0, 1])),
            "custody uses its canonical borrowed/transferred byte encoding"
        );
        let bytes = crate::tir::serialize::serialize_tir_function(&tir).unwrap();
        let restored = crate::tir::serialize::deserialize_tir_function(&bytes);
        if source.is_extern {
            assert!(tir.blocks.is_empty(), "a declaration must remain bodyless");
            assert!(
                restored.is_none(),
                "the executable TIR cache must reject a declaration without an entry block"
            );
        } else {
            crate::tir::verify::verify_function(&tir).expect("the executable body must verify");
            let restored = restored.expect("the executable body must survive the cache");
            assert_eq!(
                [restored.parameter_custody(0), restored.parameter_custody(1)],
                [Borrowed, Transferred]
            );
            assert_eq!(
                crate::tir::serialize::serialize_tir_function(&restored).unwrap(),
                bytes,
                "the custody-bearing body must retain its canonical artifact"
            );
        }
    }
    let mut borrowed_source = make_func("borrows", &["only"], vec![op(molt_ir::OpKind::RetVoid)]);
    let borrowed = lower_to_tir(&borrowed_source);
    borrowed_source.parameter_custody = vec![Borrowed];
    let explicit_borrowed = lower_to_tir(&borrowed_source);
    assert_eq!(
        crate::tir::serialize::serialize_tir_function(&borrowed).unwrap(),
        crate::tir::serialize::serialize_tir_function(&explicit_borrowed).unwrap(),
        "implicit and explicit all-borrowed custody have one artifact encoding"
    );
    assert_eq!(borrowed.parameter_custody(0), Borrowed);
    assert!(
        !borrowed
            .attrs
            .contains_key(crate::tir::function::PARAMETER_CUSTODY_ATTR)
    );
}
