use molt_backend::{FunctionIR, OpIR, SimpleIR, validate_simple_ir};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn test_func(name: &str, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: Vec::new(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

fn operation_shape_fixture(
    shape: &molt_ir::tir::op_kinds_generated::SimpleIrOpShape,
    operands: usize,
    value: Option<i64>,
) -> SimpleIR {
    let params: Vec<_> = (0..operands).map(|index| format!("arg{index}")).collect();
    let mut function = test_func(
        "operation_shape_fixture",
        vec![OpIR {
            kind: shape.kind.into(),
            args: Some(params.clone()),
            value,
            ..OpIR::default()
        }],
    );
    function.params = params;
    function.return_abi = molt_ir::FunctionReturnAbi::Void;
    // Keep the subject at op#0 while satisfying its generated frame contract.
    // Frame consumers inherit their caller's context; only entry owns a local
    // lifecycle. Parameters retain ordinary borrowed custody, independent of
    // the generated frame-home storage adoption performed by ownership lowering.
    if shape.kind == "trace_enter_slot" {
        function.execution_context = molt_ir::ExecutionContextPolicy::Local;
        function.ops.push(op("trace_exit"));
    } else if function.ops[0].uses_execution_frame() {
        function.execution_context = molt_ir::ExecutionContextPolicy::Inherited;
    }
    function.ops.push(op("ret_void"));
    SimpleIR {
        functions: vec![function],
        profile: None,
    }
}

#[test]
fn generated_operation_shapes_agree_across_all_transport_boundaries() {
    use molt_ir::tir::op_kinds_generated::{SIMPLEIR_OP_SHAPES, SimpleIrOpValueRule};
    for shape in SIMPLEIR_OP_SHAPES {
        let value = (shape.value_rule == SimpleIrOpValueRule::NonNegative).then_some(0);
        let mut cases = vec![
            (shape.operands, value, None),
            (shape.operands + 1, value, Some("args")),
        ];
        if shape.operands > 0 {
            cases.push((shape.operands - 1, value, Some("args")));
        }
        if shape.value_rule == SimpleIrOpValueRule::NonNegative {
            cases.push((shape.operands, None, Some("value")));
            cases.push((shape.operands, Some(-1), Some("value")));
        }
        for (operands, value, invalid_field) in cases {
            let ir = operation_shape_fixture(shape, operands, value);
            let valid = invalid_field.is_none();
            let encoded = serde_json::to_string(&ir).unwrap();
            let mut function = serde_json::to_value(&ir.functions[0]).unwrap();
            function["kind"] = "function".into();
            let ndjson = format!(
                "{{\"kind\":\"ir_stream_start\"}}\n{function}\n{{\"kind\":\"ir_stream_end\"}}\n"
            );
            let results = [
                validate_simple_ir(&ir),
                SimpleIR::from_json_str(&encoded).map(|_| ()),
                serde_json::from_str::<SimpleIR>(&encoded)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                SimpleIR::from_ndjson_reader(std::io::Cursor::new(ndjson.as_bytes())).map(|_| ()),
            ];
            for result in results {
                assert_eq!(result.is_ok(), valid, "{}: {result:?}", shape.kind);
                if let Err(error) = result {
                    assert!(
                        error.contains(shape.kind)
                            && error.contains(invalid_field.expect("malformed fixture")),
                        "{error}"
                    );
                    assert!(error.contains("op#0"), "{error}");
                }
            }
            let fragment = molt_ir::ir_schema::validate_function_op_shapes(&ir.functions[0]);
            assert_eq!(fragment.is_ok(), valid);
        }
    }
}

#[test]
fn runtime_guard_transport_preserves_two_reads_without_weakening_unary_aliases() {
    // The independent operation contract must survive JSON and streamed input.
    // Ownership transparency says which object is returned, not how many
    // operands the validation effect reads.
    for (kind, expected) in [
        ("guard_tag", 2),
        ("guard_type", 2),
        ("copy", 1),
        ("identity_alias", 1),
        ("binding_alias", 1),
    ] {
        for count in 0..=3 {
            for var in [None, Some("source"), Some("tag")] {
                for out in [None, Some("none"), Some("checked")] {
                    let inputs = ["source", "tag", "extra"];
                    let mut function = test_func(
                        "guard_operand_transport",
                        vec![
                            OpIR {
                                kind: kind.into(),
                                args: Some(
                                    inputs[..count].iter().map(|name| (*name).into()).collect(),
                                ),
                                var: var.map(str::to_string),
                                out: out.map(str::to_string),
                                ..OpIR::default()
                            },
                            op("ret_void"),
                        ],
                    );
                    function.params = inputs.iter().map(|name| (*name).into()).collect();
                    let ir = SimpleIR {
                        functions: vec![function],
                        profile: None,
                    };
                    // Runtime guards require two args. Copy/owned aliases can
                    // use their existing sole semantic `var` read instead.
                    let valid = if expected == 2 {
                        count == 2 && var.is_none()
                    } else {
                        count + usize::from(var.is_some()) == 1
                    };
                    if expected == 2 {
                        assert_eq!(
                            molt_ir::ir_schema::validate_function_op_shapes(&ir.functions[0])
                                .is_ok(),
                            valid,
                            "isolated {kind} count={count} var={var:?}",
                        );
                    }
                    let encoded = serde_json::to_string(&ir).unwrap();
                    let mut streamed = serde_json::to_value(&ir.functions[0]).unwrap();
                    streamed["kind"] = "function".into();
                    let ndjson = format!(
                        "{{\"kind\":\"ir_stream_start\"}}\n{streamed}\n{{\"kind\":\"ir_stream_end\"}}\n"
                    );
                    for result in [
                        validate_simple_ir(&ir),
                        SimpleIR::from_json_str(&encoded).map(|_| ()),
                        SimpleIR::from_ndjson_reader(std::io::Cursor::new(ndjson.as_bytes()))
                            .map(|_| ()),
                    ] {
                        assert_eq!(
                            result.is_ok(),
                            valid,
                            "{kind} count={count} var={var:?} out={out:?}: {result:?}"
                        );
                    }
                }
            }
        }
        if expected == 2 {
            let mut function = test_func(
                "undefined_guard_tag",
                vec![
                    OpIR {
                        kind: kind.into(),
                        args: Some(vec!["source".into(), "tag".into()]),
                        ..OpIR::default()
                    },
                    op("ret_void"),
                ],
            );
            function.params = vec!["source".into()];
            let error = validate_simple_ir(&SimpleIR {
                functions: vec![function],
                profile: None,
            })
            .unwrap_err();
            assert!(error.contains("uses undefined value `tag`"), "{error}");
        }
    }
}

#[test]
fn representation_conversions_require_one_operand_independent_of_result_use() {
    // The wire schema specifies these four spellings independently of the
    // generated shape inventory: an omitted inventory row must fail this test.
    for kind in ["box", "box_from_raw_int", "unbox", "unbox_to_raw_int"] {
        for (args, var) in [
            (None, None),
            (Some(vec![]), None),
            (Some(vec!["source".into(), "extra".into()]), None),
            (Some(vec!["source".into()]), Some("extra")),
            (Some(vec!["source".into()]), Some("source")),
            (Some(vec!["source".into()]), Some("none")),
            (Some(vec!["source".into()]), Some("")),
        ] {
            let mut function = test_func(
                "representation_shape",
                vec![
                    OpIR {
                        kind: kind.into(),
                        args: args.clone(),
                        var: var.map(str::to_owned),
                        ..OpIR::default()
                    },
                    op("ret_void"),
                ],
            );
            function.params = vec!["source".into(), "extra".into()];
            let ir = SimpleIR {
                functions: vec![function],
                profile: None,
            };
            let violation = if var.is_some() {
                "forbids `var`; `args` is the sole input carrier".to_string()
            } else {
                format!(
                    "requires `args` length 1, found {}",
                    args.as_ref()
                        .map_or("none".into(), |args| args.len().to_string())
                )
            };
            let expected = format!(
                "function `representation_shape` op#0: [family=representation_conversion] `{kind}` {violation}"
            );
            let encoded = serde_json::to_string(&ir).unwrap();
            let mut function = serde_json::to_value(&ir.functions[0]).unwrap();
            function["kind"] = "function".into();
            let ndjson = format!(
                "{{\"kind\":\"ir_stream_start\"}}\n{function}\n{{\"kind\":\"ir_stream_end\"}}\n"
            );
            for result in [
                validate_simple_ir(&ir),
                SimpleIR::from_json_str(&encoded).map(|_| ()),
                serde_json::from_str::<SimpleIR>(&encoded)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                SimpleIR::from_ndjson_reader(std::io::Cursor::new(ndjson.as_bytes())).map(|_| ()),
            ] {
                let error = result.expect_err("malformed conversion must fail wire admission");
                assert!(error.contains(&expected), "{error}");
            }
            let isolated = std::panic::catch_unwind(|| {
                molt_backend::tir::lower_from_simple::lower_to_tir(&ir.functions[0])
            })
            .expect_err("malformed conversion must fail before isolated SSA lowering");
            let message = isolated
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| isolated.downcast_ref::<&str>().copied())
                .unwrap_or("");
            assert!(message.contains(&expected), "{message}");
            assert_checked_shape_rejection(&ir);
        }
        for out in [None, Some("none"), Some("result")] {
            let mut function = test_func(
                "representation_shape",
                vec![
                    OpIR {
                        kind: kind.into(),
                        args: Some(vec!["source".into()]),
                        out: out.map(str::to_owned),
                        ..OpIR::default()
                    },
                    op("ret_void"),
                ],
            );
            function.params = vec!["source".into()];
            let ir = SimpleIR {
                functions: vec![function],
                profile: None,
            };
            validate_simple_ir(&ir).expect("a discarded result does not change unary admission");
            let tir = molt_backend::tir::lower_from_simple::lower_to_tir(&ir.functions[0]);
            molt_backend::tir::verify::verify_operation_shapes(&tir).unwrap();
            let roundtrip = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir);
            let conversions: Vec<_> = roundtrip
                .iter()
                .filter(|op| matches!(op.kind.as_str(), "box" | "unbox"))
                .collect();
            assert_eq!(conversions.len(), 1, "{kind} {out:?}: {roundtrip:?}");
            assert_eq!(conversions[0].args.as_ref().map(Vec::len), Some(1));
            assert_eq!(
                molt_ir::tir::simple_def_use::simple_ir_out_result(conversions[0]).is_some(),
                out == Some("result")
            );
        }
    }
}

#[test]
fn isolated_lowering_rejects_shapes_without_requiring_program_slot_closure() {
    use molt_ir::tir::op_kinds_generated::simpleir_op_shape;
    let shape = simpleir_op_shape("code_slot_set").unwrap();
    let fragment = operation_shape_fixture(shape, 2, Some(17));
    // No table-init or code-construction declaration: those are program/runtime
    // semantics, not the isolated function's operand transport contract.
    let tir = molt_backend::tir::lower_from_simple::lower_to_tir(&fragment.functions[0]);
    assert!(molt_backend::tir::verify::verify_operation_shapes(&tir).is_ok());
    let malformed = operation_shape_fixture(shape, 1, Some(17));
    let error = std::panic::catch_unwind(|| {
        molt_backend::tir::lower_from_simple::lower_to_tir(&malformed.functions[0])
    })
    .expect_err("direct function lowering must reject old one-operand slots");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("invalid SimpleIR operation shape before lowering"),
        "{message}"
    );
    assert!(message.contains("code_slot_set"), "{message}");
}

#[test]
fn direct_checked_backends_share_generated_shape_rejection() {
    use molt_ir::tir::op_kinds_generated::{SIMPLEIR_OP_SHAPES, SimpleIrOpValueRule};
    for shape in SIMPLEIR_OP_SHAPES {
        let value = (shape.value_rule == SimpleIrOpValueRule::NonNegative).then_some(0);
        let mut malformed = vec![operation_shape_fixture(shape, shape.operands + 1, value)];
        if shape.operands > 0 {
            malformed.push(operation_shape_fixture(shape, shape.operands - 1, value));
        }
        if shape.value_rule == SimpleIrOpValueRule::NonNegative {
            malformed.push(operation_shape_fixture(shape, shape.operands, None));
            malformed.push(operation_shape_fixture(shape, shape.operands, Some(-1)));
        }
        for ir in malformed {
            assert_checked_shape_rejection(&ir);
        }
    }
}

#[test]
fn external_programs_reject_unknown_and_backend_private_kinds_before_preprocessing() {
    use molt_ir::ir_schema::OpShapeViolation;
    use molt_ir::tir::op_kinds_generated::SIMPLEIR_BACKEND_PRIVATE_KINDS;

    for (kind, owner) in std::iter::once(("future_unregistered_op", None)).chain(
        SIMPLEIR_BACKEND_PRIVATE_KINDS
            .iter()
            .map(|&(kind, owner)| (kind, Some(owner))),
    ) {
        for out in [None, Some("unused_result")] {
            // Even unreachable, unused and resultless operations must be rejected
            // before optimization can erase them at any checked backend boundary.
            let ir = SimpleIR {
                functions: vec![test_func(
                    "external_input",
                    vec![
                        op("ret_void"),
                        OpIR {
                            kind: kind.into(),
                            out: out.map(str::to_owned),
                            ..OpIR::default()
                        },
                    ],
                )],
                profile: None,
            };
            let error = molt_ir::ir_schema::validate_simple_ir_op_shapes(&ir).unwrap_err();
            assert_eq!(error.function, "external_input");
            assert_eq!(error.op_index, 1);
            assert_eq!(error.shape.kind, kind);
            assert_eq!(
                error.shape.violation,
                match owner {
                    Some(backend) => OpShapeViolation::BackendPrivate { backend },
                    None => OpShapeViolation::UnregisteredKind,
                }
            );
            let json = serde_json::to_string(&ir).unwrap();
            let wire_error = SimpleIR::from_json_str(&json).unwrap_err();
            assert!(
                wire_error.contains(kind)
                    && wire_error.contains("external_input")
                    && wire_error.contains("op#1"),
                "{wire_error}"
            );
            assert_checked_shape_rejection(&ir);
        }
    }
}

#[test]
fn backend_private_string_semantics_survive_internal_tir_roundtrip() {
    let mut function = test_func(
        "internal_luau",
        vec![
            OpIR {
                kind: "string_splitlines".into(),
                args: Some(vec!["text".into(), "keepends".into()]),
                out: Some("lines".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["lines".into()]),
                ..OpIR::default()
            },
        ],
    );
    function.return_abi = molt_ir::FunctionReturnAbi::Value;
    function.params = vec!["text".into(), "keepends".into()];
    let tir = molt_backend::tir::lower_from_simple::lower_to_tir(&function);
    let restored = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir);
    let split = restored
        .iter()
        .find(|op| op.kind == "string_splitlines")
        .expect("private string operation must retain its identity, not become a copy");
    assert_eq!(
        split.args.as_deref(),
        Some(["text".to_string(), "keepends".to_string()].as_slice())
    );
    let ret = restored.iter().find(|op| op.kind == "ret").unwrap();
    assert_eq!(
        ret.args.as_deref(),
        Some([split.out.clone().unwrap()].as_slice())
    );
}

fn assert_checked_shape_rejection(ir: &SimpleIR) {
    let expected = molt_ir::ir_schema::validate_simple_ir_op_shapes(ir).unwrap_err();
    #[cfg(feature = "native-backend")]
    {
        let Err(error) = molt_backend::SimpleBackend::new().compile_checked(ir.clone()) else {
            panic!("native checked admission");
        };
        assert_eq!(error, expected);
    }
    #[cfg(all(feature = "native-backend", feature = "llvm"))]
    {
        let Err(error) = molt_backend::SimpleBackend::new().compile_llvm_checked(ir.clone()) else {
            panic!("LLVM checked admission");
        };
        assert_eq!(error, expected);
    }
    #[cfg(feature = "wasm-backend")]
    {
        let Err(error) = molt_backend::WasmBackend::new().compile_checked(ir.clone()) else {
            panic!("WASM checked admission");
        };
        assert_eq!(error, expected);
    }
    #[cfg(feature = "rust-backend")]
    {
        let error = molt_backend::rust::RustBackend::new()
            .compile_checked(ir)
            .unwrap_err();
        assert!(error.contains(&expected.to_string()), "{error}");
    }
    #[cfg(feature = "luau-backend")]
    {
        let error = molt_backend::luau::LuauBackend::new()
            .compile_checked(ir)
            .unwrap_err();
        assert!(error.contains(&expected.to_string()), "{error}");
    }
    let _ = expected;
}

#[test]
fn retired_operations_are_rejected_at_wire_isolated_and_checked_backend_boundaries() {
    for (kind, operands, reason) in [
        (
            "store_init",
            2,
            "fresh-slot initialization is derived from typed-slot ownership facts",
        ),
        (
            "guarded_field_init",
            2,
            "initialization cannot be asserted by wire spelling",
        ),
        (
            "object_new_bound_stack",
            1,
            "frame placement requires an owner-lifetime proof",
        ),
        (
            "list_repeat_range",
            2,
            "canonical list construction, multiplication or comprehension lowering",
        ),
        (
            "list_repeat_range",
            4,
            "canonical list construction, multiplication or comprehension lowering",
        ),
    ] {
        let params: Vec<String> = (0..operands).map(|index| format!("arg{index}")).collect();
        let mut function = test_func(
            "retired_operation",
            vec![
                OpIR {
                    kind: kind.into(),
                    args: Some(params.clone()),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
                op("ret_void"),
            ],
        );
        function.params = params;
        let ir = SimpleIR {
            functions: vec![function],
            profile: None,
        };
        let encoded = serde_json::to_string(&ir).unwrap();
        let mut function = serde_json::to_value(&ir.functions[0]).unwrap();
        function["kind"] = "function".into();
        let ndjson = format!(
            "{{\"kind\":\"ir_stream_start\"}}\n{function}\n{{\"kind\":\"ir_stream_end\"}}\n"
        );
        for result in [
            validate_simple_ir(&ir),
            SimpleIR::from_json_str(&encoded).map(|_| ()),
            serde_json::from_str::<SimpleIR>(&encoded)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            SimpleIR::from_ndjson_reader(std::io::Cursor::new(ndjson.as_bytes())).map(|_| ()),
        ] {
            let error = result.expect_err("retired spellings must not enter SimpleIR");
            assert!(
                error.contains(&format!("retired compiler operation `{kind}`")),
                "{error}"
            );
            assert!(error.contains(reason), "{error}");
        }
        let fragment_error =
            molt_ir::ir_schema::validate_function_op_shapes(&ir.functions[0]).unwrap_err();
        assert!(fragment_error.to_string().contains(reason));
        assert!(
            std::panic::catch_unwind(|| {
                molt_backend::tir::lower_from_simple::lower_to_tir(&ir.functions[0])
            })
            .is_err(),
            "{kind} must fail before isolated lowering"
        );
        assert_checked_shape_rejection(&ir);
    }
}

#[test]
fn bytearray_fill_range_preserves_four_operands_without_an_owned_result() {
    let shape =
        molt_ir::tir::op_kinds_generated::simpleir_op_shape("bytearray_fill_range").unwrap();
    for out in [None, Some("none".to_string())] {
        let mut ir = operation_shape_fixture(shape, 4, None);
        ir.functions[0].ops[0].out = out;
        assert!(validate_simple_ir(&ir).is_ok());
        assert!(SimpleIR::from_json_str(&serde_json::to_string(&ir).unwrap()).is_ok());
    }
}

#[test]
fn validate_simple_ir_rejects_retired_frame_allocation_before_lowering() {
    for payload in [None, Some(-1), Some(16), Some(i64::MAX)] {
        let allocation = OpIR {
            kind: "object_new_bound_stack".into(),
            args: Some(vec!["class".into()]),
            value: payload,
            out: Some("object".into()),
            ..OpIR::default()
        };
        let mut function = test_func(
            "retired_frame_escape",
            vec![
                allocation,
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["object".into()]),
                    ..OpIR::default()
                },
            ],
        );
        function.params.push("class".into());
        let ir = SimpleIR {
            functions: vec![function],
            profile: None,
        };
        let error = validate_simple_ir(&ir).expect_err("frame lifetime is unproved");
        assert!(error.contains("retired compiler operation"));
        assert!(error.contains("owner-lifetime proof"));
        let encoded = serde_json::to_string(&ir).unwrap();
        assert!(
            SimpleIR::from_json_str(&encoded)
                .unwrap_err()
                .contains("retired compiler operation")
        );
    }
}

#[test]
fn validate_simple_ir_accepts_well_formed_value_uses() {
    let mut c0 = op("const");
    c0.value = Some(1);
    c0.out = Some("v0".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["v0".to_string()]);

    let ir = SimpleIR {
        functions: vec![test_func("molt_test_validate_ok", vec![c0, ret])],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_rejects_missing_value_definition() {
    let mut idx = op("index");
    idx.args = Some(vec!["v0".to_string(), "v9999".to_string()]);
    idx.out = Some("v1".to_string());

    let ir = SimpleIR {
        functions: vec![test_func("molt_test_validate_missing", vec![idx])],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected undefined value rejection");
    assert!(err.contains("uses undefined value `v9999`"));
}

#[test]
fn validate_simple_ir_accepts_block_argument_local_slot_transport() {
    let mut incoming = op("const_int");
    incoming.value = Some(42);
    incoming.out = Some("incoming".to_string());

    let mut store = op("store_var");
    store.var = Some("_bb1_arg0".to_string());
    store.args = Some(vec!["incoming".to_string()]);

    let mut load = op("load_var");
    load.var = Some("_bb1_arg0".to_string());
    load.out = Some("joined".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["joined".to_string()]);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_block_arg_slot",
            vec![incoming, store, load, ret],
        )],
        profile: None,
    };
    validate_simple_ir(&ir).expect("store_var defines the block-argument local slot");
}

#[test]
fn validate_simple_ir_rejects_undefined_block_argument_store_source() {
    let mut store = op("store_var");
    store.var = Some("_bb1_arg0".to_string());
    store.args = Some(vec!["missing_incoming".to_string()]);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_missing_block_arg_source",
            vec![store],
        )],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("store source must still be defined");
    assert!(err.contains("function `molt_test_validate_missing_block_arg_source` op#0"));
    assert!(err.contains("uses undefined value `missing_incoming`"));
}

#[test]
fn validate_simple_ir_accepts_every_generated_multi_result_transport_shape() {
    let mut sequence = op("const");
    sequence.value = Some(0);
    sequence.out = Some("sequence".to_string());

    let mut unpack = op("unpack_sequence");
    unpack.args = Some(vec![
        "sequence".to_string(),
        "left".to_string(),
        "right".to_string(),
    ]);
    unpack.value = Some(2);

    let mut checked_add = op("checked_add");
    checked_add.args = Some(vec!["left".to_string(), "right".to_string()]);
    checked_add.var = Some("sum".to_string());
    checked_add.out = Some("add_overflow".to_string());

    let mut checked_mul = op("checked_mul");
    checked_mul.args = Some(vec!["sum".to_string(), "right".to_string()]);
    checked_mul.var = Some("product".to_string());
    checked_mul.out = Some("mul_overflow".to_string());

    let mut next = op("iter_next_unboxed");
    next.args = Some(vec!["iterator".to_string()]);
    next.var = Some("item".to_string());
    next.out = Some("done".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["product".to_string()]);

    let mut function = test_func(
        "molt_test_validate_multi_result_fields",
        vec![sequence, unpack, checked_add, checked_mul, next, ret],
    );
    function.params = vec!["iterator".to_string()];
    let ir = SimpleIR {
        functions: vec![function],
        profile: None,
    };
    validate_simple_ir(&ir).expect("generated field roles must define every multi-result output");
}

#[test]
fn validate_simple_ir_allows_dict_receiver_merge_placeholders() {
    let mut k = op("const_str");
    k.s_value = Some("key".to_string());
    k.out = Some("v0".to_string());

    let mut v = op("const");
    v.value = Some(1);
    v.out = Some("v1".to_string());

    let mut dict_set = op("dict_set");
    dict_set.args = Some(vec![
        "v9999".to_string(),
        "v0".to_string(),
        "v1".to_string(),
    ]);
    dict_set.out = Some("none".to_string());

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_dict_receiver_placeholder",
            vec![k, v, dict_set],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_accepts_fast_int_flags_on_arithmetic_ops() {
    let mut lhs = op("const");
    lhs.value = Some(7);
    lhs.out = Some("v0".to_string());

    let mut rhs = op("const");
    rhs.value = Some(3);
    rhs.out = Some("v1".to_string());

    let mut add = op("add");
    add.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    add.out = Some("v2".to_string());
    add.fast_int = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_fast_int",
            vec![lhs, rhs, add],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_accepts_fast_int_flags_on_division_transport_ops() {
    let mut lhs = op("const");
    lhs.value = Some(7);
    lhs.out = Some("v0".to_string());

    let mut rhs = op("const");
    rhs.value = Some(3);
    rhs.out = Some("v1".to_string());

    let mut div = op("div");
    div.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    div.out = Some("v2".to_string());
    div.fast_int = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_fast_int_div",
            vec![lhs, rhs, div],
        )],
        profile: None,
    };
    assert!(validate_simple_ir(&ir).is_ok());
}

#[test]
fn validate_simple_ir_rejects_param_type_arity_mismatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_validate_param_types".to_string(),
            params: vec!["x".to_string()],
            ops: vec![op("ret_void")],
            param_types: Some(vec!["int".to_string(), "bool".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected param type arity rejection");
    assert!(err.contains("has 1 params but 2 param_types"));
}

#[test]
fn validate_simple_ir_rejects_conflicting_fast_scalar_flags() {
    let mut scalar = op("add");
    scalar.args = Some(vec!["lhs".to_string(), "rhs".to_string()]);
    scalar.out = Some("sum".to_string());
    scalar.fast_int = Some(true);
    scalar.fast_float = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_validate_conflicting_scalar_flags".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![scalar],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected scalar flag rejection");
    assert!(err.contains("cannot set both fast_int and fast_float"));
}

#[test]
fn validate_simple_ir_rejects_fast_int_on_non_scalar_owner() {
    let mut call = op("call");
    call.s_value = Some("opaque".to_string());
    call.out = Some("v0".to_string());
    call.fast_int = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func("molt_test_validate_fast_int_owner", vec![call])],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected fast_int owner rejection");
    assert!(err.contains("does not own fast_int scalar specialization"));
}

// floordiv/mod (and their inplace variants) own the fast_float scalar
// specialization: the frontend emits fast_float on them when both operands
// are float-typed, so the SimpleIR contract must accept it. A mutation that
// drops floordiv/mod from SCALAR_FAST_FLOAT_KINDS re-breaks the native build
// for `f // f` / `f % f` (tests/differential/basic/float_ops.py).
#[test]
fn validate_simple_ir_accepts_fast_float_floordiv_and_mod() {
    for kind in ["floordiv", "inplace_floordiv", "mod", "inplace_mod"] {
        let mut a = op("const_float");
        a.f_value = Some(7.0);
        a.out = Some("lhs".to_string());
        let mut b = op("const_float");
        b.f_value = Some(2.0);
        b.out = Some("rhs".to_string());

        let mut scalar = op(kind);
        scalar.args = Some(vec!["lhs".to_string(), "rhs".to_string()]);
        scalar.out = Some("quot".to_string());
        scalar.fast_float = Some(true);

        let ir = SimpleIR {
            functions: vec![test_func(
                "molt_test_validate_fast_float_owner",
                vec![a, b, scalar],
            )],
            profile: None,
        };
        assert!(
            validate_simple_ir(&ir).is_ok(),
            "op `{kind}` must own the fast_float scalar specialization it emits"
        );
    }
}

// Mutation teeth: an op that does NOT own the fast_float specialization must
// still be rejected, so the whitelist stays exact rather than fail-open.
#[test]
fn validate_simple_ir_rejects_fast_float_on_non_scalar_owner() {
    let mut call = op("call");
    call.s_value = Some("opaque".to_string());
    call.out = Some("v0".to_string());
    call.fast_float = Some(true);

    let ir = SimpleIR {
        functions: vec![test_func(
            "molt_test_validate_fast_float_owner_reject",
            vec![call],
        )],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected fast_float owner rejection");
    assert!(err.contains("does not own fast_float scalar specialization"));
}

#[test]
fn validate_simple_ir_rejects_unknown_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.container_type = Some("vectorish".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_validate_container_type".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected container type rejection");
    assert!(err.contains("unsupported container_type `vectorish`"));
}

#[test]
fn validate_simple_ir_rejects_legacy_list_int_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.container_type = Some("list_int".to_string());

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_validate_legacy_list_int_container_type".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let err = validate_simple_ir(&ir).expect_err("expected list_int container type rejection");
    assert!(err.contains("unsupported container_type `list_int`"));
}

#[test]
fn validate_simple_ir_accepts_bce_safe_without_container_type() {
    let mut idx = op("index");
    idx.args = Some(vec!["seq".to_string(), "idx".to_string()]);
    idx.out = Some("item".to_string());
    idx.bce_safe = Some(true);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_validate_bce_container".to_string(),
            params: vec!["seq".to_string(), "idx".to_string()],
            ops: vec![idx],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    validate_simple_ir(&ir).expect("bce_safe is an independent bounds proof");
}

#[test]
fn validate_simple_ir_rejects_unproved_arena_placement_on_every_operation() {
    for kind in ["add", "alloc", "alloc_class", "object_new_bound"] {
        for requested in [false, true] {
            let mut allocation = op(kind);
            allocation.args = Some(match kind {
                "alloc" => vec![],
                "add" => vec!["lhs".to_string(), "rhs".to_string()],
                _ => vec!["lhs".to_string()],
            });
            allocation.out = Some("result".to_string());
            allocation.value = Some(16);
            allocation.arena_eligible = Some(requested);
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    return_abi: molt_ir::FunctionReturnAbi::Void,
                    name: "molt_test_validate_arena_owner".to_string(),
                    params: vec!["lhs".to_string(), "rhs".to_string()],
                    ops: vec![allocation],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    codegen_partition: false,
                    parameter_custody: Vec::new(),
                    execution_context: Default::default(),
                }],
                profile: None,
            };
            let err = validate_simple_ir(&ir).expect_err("expected arena owner rejection");
            assert!(err.contains("cannot carry arena_eligible"), "{kind}: {err}");
        }
    }
}

#[test]
fn tree_shake_luau_rewrites_main_and_drops_runtime_bootstrap_helpers() {
    let mut main_runtime_init = op("call");
    main_runtime_init.s_value = Some("molt_runtime_init".to_string());
    main_runtime_init.out = Some("v0".to_string());

    let mut main_init = op("call");
    main_init.s_value = Some("molt_init___main__".to_string());
    main_init.out = Some("v1".to_string());

    let main_ret = op("ret_void");

    let mut init_sys = op("call");
    init_sys.s_value = Some("molt_init_sys".to_string());
    init_sys.out = Some("v2".to_string());

    let mut user_call = op("call");
    user_call.s_value = Some("user_kernel".to_string());
    user_call.out = Some("v3".to_string());

    let init_ret = op("ret_void");

    let user_ret = op("ret_void");
    let helper_ret = op("ret_void");

    let mut ir = SimpleIR {
        functions: vec![
            test_func("molt_main", vec![main_runtime_init, main_init, main_ret]),
            test_func("molt_init___main__", vec![init_sys, user_call, init_ret]),
            test_func("molt_runtime_init", vec![helper_ret.clone()]),
            test_func("molt_init_sys", vec![helper_ret.clone()]),
            test_func("user_kernel", vec![user_ret]),
            test_func("unused_helper", vec![helper_ret]),
        ],
        profile: None,
    };

    ir.tree_shake_luau();

    let names = ir
        .functions
        .iter()
        .map(|func| func.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["molt_main", "molt_init___main__", "user_kernel"]
    );

    let main = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_main")
        .expect("molt_main should remain");
    assert_eq!(main.ops.len(), 2);
    assert_eq!(main.ops[0].kind, "call");
    assert_eq!(main.ops[0].s_value.as_deref(), Some("molt_init___main__"));
    assert_eq!(main.ops[1].kind, "ret_void");

    let init_main = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_init___main__")
        .expect("molt_init___main__ should remain");
    assert_eq!(init_main.ops[0].kind, "nop");
    assert_eq!(init_main.ops[1].kind, "call");
    assert_eq!(init_main.ops[1].s_value.as_deref(), Some("user_kernel"));
}
