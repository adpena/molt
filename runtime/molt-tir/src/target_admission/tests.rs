use super::*;

mod wire_domains;
use crate::tir::op_kinds_generated::{
    SIMPLEIR_RUNTIME_REQUIREMENT_CARRIER_KINDS, SIMPLEIR_RUNTIME_SYMBOL_CARRIER_KINDS,
    SimpleIrRuntimeRequirements,
};
use crate::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR};

fn function_ir(ops: Vec<OpIR>) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "f".to_string(),
            ops,
            ..FunctionIR::default()
        }],
        profile: None,
    }
}

#[test]
fn singleton_materialization_does_not_authorize_identity_or_unknown_spellings() {
    for kind in [
        molt_ir::OpKind::ConstEllipsis,
        molt_ir::OpKind::ConstNotImplemented,
        molt_ir::OpKind::Missing,
    ] {
        let literal = OpIR {
            kind,
            out: Some("singleton".into()),
            ..OpIR::new(kind)
        };
        let ir = function_ir(vec![
            literal.clone(),
            OpIR {
                kind: molt_ir::OpKind::Ret,
                args: Some(vec!["singleton".into()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
        ]);
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::wasm_release_fast(),
            TargetInfo::llvm_release_fast(),
            TargetInfo::luau_release_fast(),
            TargetInfo::rust_release_fast(),
            TargetInfo::mlir_release_fast(),
        ] {
            validate_runtime_target_contract(&ir, &target)
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
        }
        let observed = function_ir(vec![
            literal,
            OpIR {
                kind: molt_ir::OpKind::Is,
                args: Some(vec!["singleton".into(), "singleton".into()]),
                out: Some("observed".into()),
                ..OpIR::new(molt_ir::OpKind::Is)
            },
            OpIR {
                kind: molt_ir::OpKind::Ret,
                args: Some(vec!["observed".into()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
        ]);
        let restricted = target_with_runtime(
            SimpleIrRuntimeRequirements::ALL.difference(SimpleIrRuntimeRequirements::IDENTITY),
        );
        let error = validate_runtime_target_contract(&observed, &restricted)
            .expect_err("a literal does not grant object identity semantics");
        assert!(error.contains("Python object identity"), "{error}");
    }
}

#[test]
fn shared_graph_admission_precedes_target_representation_planning() {
    let labelled = |kind: molt_ir::OpKind, value: i64| OpIR {
        kind,
        value: Some(value),
        ..OpIR::new(kind)
    };
    let cases = [
        (
            vec![labelled(molt_ir::OpKind::Jump, 19)],
            "invalid-jump-target",
        ),
        (
            vec![OpIR {
                kind: molt_ir::OpKind::Jump,
                s_value: Some("alias".into()),
                ..OpIR::new(molt_ir::OpKind::Jump)
            }],
            "malformed-label-reference",
        ),
        (
            vec![OpIR {
                kind: molt_ir::OpKind::Label,
                s_value: Some("alias".into()),
                ..OpIR::new(molt_ir::OpKind::Label)
            }],
            "malformed-label-definition",
        ),
        (
            vec![
                labelled(molt_ir::OpKind::Label, 19),
                labelled(molt_ir::OpKind::Label, 19),
            ],
            "duplicate-label-definition",
        ),
        (
            vec![
                OpIR {
                    kind: molt_ir::OpKind::LoopStart,
                    ..OpIR::new(molt_ir::OpKind::LoopStart)
                },
                OpIR {
                    kind: molt_ir::OpKind::EndIf,
                    ..OpIR::new(molt_ir::OpKind::EndIf)
                },
            ],
            "unbalanced-control-flow",
        ),
        (
            vec![OpIR {
                kind: molt_ir::OpKind::LoopBreakIfException,
                ..OpIR::new(molt_ir::OpKind::LoopBreakIfException)
            }],
            "break-outside-loop",
        ),
    ];
    for target in [
        TargetInfo::native_release_fast(),
        TargetInfo::wasm_release_fast(),
        TargetInfo::llvm_release_fast(),
        TargetInfo::luau_release_fast(),
        TargetInfo::rust_release_fast(),
        TargetInfo::mlir_release_fast(),
    ] {
        for (ops, expected) in &cases {
            let mut planned = false;
            let error = validate_target_contract_with_representation_plan(
                &function_ir(ops.clone()),
                &target,
                |_, _| {
                    planned = true;
                    Ok(())
                },
            )
            .expect_err("malformed graph must not reach representation planning");
            assert!(!planned, "{}: {error}", target.target.as_str());
            assert!(
                error.contains(*expected),
                "{}: {error}",
                target.target.as_str()
            );
            assert!(error.contains("function `f` op#"), "{error}");
        }
    }
}

#[test]
fn representation_shape_admission_precedes_every_target_plan() {
    for target in [
        TargetInfo::native_release_fast(),
        TargetInfo::wasm_release_fast(),
        TargetInfo::llvm_release_fast(),
        TargetInfo::luau_release_fast(),
        TargetInfo::rust_release_fast(),
        TargetInfo::mlir_release_fast(),
    ] {
        for kind in [
            molt_ir::OpKind::Box,
            molt_ir::OpKind::BoxFromRawInt,
            molt_ir::OpKind::Unbox,
            molt_ir::OpKind::UnboxToRawInt,
        ] {
            for (args, var) in [
                (None, None),
                (Some(vec![]), None),
                (Some(vec!["source".into(), "extra".into()]), None),
                (Some(vec!["source".into()]), Some("extra")),
                (Some(vec!["source".into()]), Some("source")),
                (Some(vec!["source".into()]), Some("none")),
                (Some(vec!["source".into()]), Some("")),
            ] {
                let mut ir = function_ir(vec![
                    OpIR {
                        kind,
                        args,
                        var: var.map(str::to_owned),
                        ..OpIR::new(kind)
                    },
                    OpIR {
                        kind: molt_ir::OpKind::RetVoid,
                        ..OpIR::new(molt_ir::OpKind::RetVoid)
                    },
                ]);
                ir.functions[0].params = vec!["source".into(), "extra".into()];
                let mut planned = false;
                let error =
                    validate_target_contract_with_representation_plan(&ir, &target, |_, _| {
                        planned = true;
                        Ok(())
                    })
                    .expect_err("invalid conversion must not reach representation planning");
                assert!(!planned, "{}: {error}", target.target.as_str());
                let violation = if var.is_some() {
                    "forbids `var`"
                } else {
                    "requires `args` length 1"
                };
                assert!(error.contains(&format!("`{kind}` {violation}")), "{error}");
                assert!(error.contains("function `f` op#0"), "{error}");
            }
        }
    }
}

#[test]
fn lexical_cells_require_real_storage_and_capture_transport() {
    let requirement = SimpleIrRuntimeRequirements::LEXICAL_CELLS;
    let denied = target_with_runtime(SimpleIrRuntimeRequirements::ALL.difference(requirement));
    let admitted = target_with_runtime(SimpleIrRuntimeRequirements::ALL);
    let mut operations: Vec<OpIR> = [
        molt_ir::OpKind::FuncNewClosure,
        molt_ir::OpKind::FunctionClosureBits,
    ]
    .into_iter()
    .map(|kind| OpIR {
        kind,
        ..OpIR::new(kind)
    })
    .collect();
    for symbol in [
        "molt_cell_new",
        "molt_cell_get",
        "molt_cell_set",
        "molt_cell_eq",
        "molt_cell_ne",
        "molt_cell_lt",
        "molt_cell_le",
        "molt_cell_gt",
        "molt_cell_ge",
        "molt_types_cell_new",
        "molt_types_cell_contents_get",
        "molt_types_cell_contents_set",
        "molt_types_cell_contents_delete",
        "molt_func_new_closure",
        "molt_function_closure_bits",
    ] {
        for kind in [
            molt_ir::OpKind::Call,
            molt_ir::OpKind::CallInternal,
            molt_ir::OpKind::FuncNew,
            molt_ir::OpKind::FuncNewClosure,
            molt_ir::OpKind::BuiltinFunc,
        ] {
            operations.push(OpIR {
                kind,
                s_value: Some(symbol.into()),
                ..OpIR::new(kind)
            });
        }
        for kind in SIMPLEIR_RUNTIME_SYMBOL_CARRIER_KINDS {
            operations.push(OpIR {
                kind: (*kind),
                runtime_symbol: Some(symbol.into()),
                ..OpIR::new(*kind)
            });
        }
    }
    for op in operations {
        assert!(op.runtime_requirements().unwrap().contains(requirement));
        let ir = function_ir(vec![op]);
        let error = validate_runtime_target_contract(&ir, &denied).unwrap_err();
        assert!(error.contains("shared mutable lexical cells"), "{error}");
        validate_runtime_target_contract(&ir, &admitted).unwrap();
    }
}

#[test]
fn import_protocol_is_independent_of_object_storage_and_callable_transport() {
    let import = SimpleIrRuntimeRequirements::IMPORT_PROTOCOL;
    let unsupported = target_with_runtime(SimpleIrRuntimeRequirements::ALL.difference(import));
    let supported = target_with_runtime(SimpleIrRuntimeRequirements::ALL);
    let mut operations: Vec<OpIR> = [
        molt_ir::OpKind::Import,
        molt_ir::OpKind::ImportName,
        molt_ir::OpKind::ModuleImport,
        molt_ir::OpKind::ImportFrom,
        molt_ir::OpKind::ModuleImportFrom,
        molt_ir::OpKind::ModuleImportStar,
    ]
    .into_iter()
    .map(|kind| OpIR {
        kind,
        ..OpIR::new(kind)
    })
    .collect();
    for symbol in [
        "molt_module_import",
        "molt_module_ensure",
        "molt_module_import_from",
        "molt_module_import_star",
        "molt_importlib_import_transaction",
        "molt_importlib_import_module",
        "molt_importlib_import_optional",
        "molt_importlib_import_required",
        "molt_importlib_import_or_fallback",
        "molt_importlib_reload",
        "molt_importlib_load_module_from_spec",
        "molt_importlib_load_module_shim",
        "molt_importlib_sourcefileloader_exec_module",
        "molt_importlib_zip_source_loader_exec_module",
        "molt_importlib_extension_loader_exec_module",
        "molt_importlib_sourceless_loader_exec_module",
        "molt_runpy_run_module",
        "molt_runpy_run_path",
    ] {
        for kind in [
            molt_ir::OpKind::CallInternal,
            molt_ir::OpKind::Call,
            molt_ir::OpKind::FuncNew,
            molt_ir::OpKind::FuncNewClosure,
            molt_ir::OpKind::BuiltinFunc,
        ] {
            operations.push(OpIR {
                kind,
                s_value: Some(symbol.into()),
                ..OpIR::new(kind)
            });
        }
        operations.push(OpIR {
            kind: molt_ir::OpKind::ModuleGetAttr,
            runtime_symbol: Some(symbol.into()),
            ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
        });
    }
    operations.push(OpIR {
        kind: molt_ir::OpKind::GetAttrGenericObj,
        runtime_requirement_bits: import.bits(),
        ..OpIR::new(molt_ir::OpKind::GetAttrGenericObj)
    });
    for op in operations {
        let ir = function_ir(vec![op]);
        let error = validate_runtime_target_contract(&ir, &unsupported).expect_err(&format!(
            "object/cache support cannot authorize Python import transactions: {:?}",
            ir.functions[0].ops[0]
        ));
        assert!(error.contains("Python import dispatch"), "{error}");
        validate_runtime_target_contract(&ir, &supported).unwrap();
    }
    for symbol in [
        "molt_importlib_resolve_name",
        "molt_importlib_cache_from_source",
    ] {
        assert!(
            !crate::tir::op_kinds_generated::simpleir_runtime_symbol_requirements_table(symbol)
                .contains(import)
        );
    }
}

#[test]
fn raw_boxed_stack_allocation_is_rejected_on_every_target() {
    let ir = function_ir(vec![OpIR {
        kind: molt_ir::OpKind::StackAlloc,
        value: Some(16),
        out: Some("object".into()),
        ..OpIR::new(molt_ir::OpKind::StackAlloc)
    }]);
    for target in [
        crate::tir::TargetInfo::native_release_fast(),
        crate::tir::TargetInfo::wasm_release_fast(),
        crate::tir::TargetInfo::llvm_release_fast(),
        crate::tir::TargetInfo::luau_release_fast(),
        crate::tir::TargetInfo::rust_release_fast(),
        crate::tir::TargetInfo::mlir_release_fast(),
    ] {
        let error = validate_runtime_target_contract(&ir, &target).unwrap_err();
        assert!(error.contains(crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED));
        assert!(error.contains("f:op#0"));
    }
}

#[test]
fn retired_class_frame_operation_is_rejected_at_decode() {
    for payload in [None, Some(-1), Some(16), Some(i64::MAX)] {
        let error = serde_json::from_value::<OpIR>(serde_json::json!({
            "kind": "object_new_bound_stack", "args": ["class"],
            "value": payload, "out": "object",
        }))
        .expect_err("retired spellings cannot enter any target");
        assert!(error.to_string().contains("object_new_bound_stack"));
    }
}

fn binary(kind: molt_ir::OpKind, ty: &str) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "f".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![OpIR {
                kind,
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::new(kind)
            }],
            param_types: Some(vec![ty.to_string(), ty.to_string()]),
            ..FunctionIR::default()
        }],
        profile: None,
    }
}

fn target_with_runtime(
    supported_runtime_semantics: SimpleIrRuntimeRequirements,
) -> crate::tir::TargetInfo {
    let mut target = crate::tir::TargetInfo::rust_release_fast();
    target.supported_runtime_semantics = supported_runtime_semantics;
    target
}

fn runtime_without_frame_introspection() -> crate::tir::TargetInfo {
    target_with_runtime(
        SimpleIrRuntimeRequirements::ALL
            .difference(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION),
    )
}

#[test]
fn async_work_marker_uses_precise_pending_call_eval_breaker_capability() {
    let explicit_poll = OpIR {
        kind: molt_ir::OpKind::AsyncWorkPoll,
        value: Some(9),
        ..OpIR::new(molt_ir::OpKind::AsyncWorkPoll)
    };
    let marked_observer = OpIR {
        kind: molt_ir::OpKind::ExceptionFinallyPendingObserver,
        out: Some("pending".to_string()),
        async_work_poll: true,
        ..OpIR::new(molt_ir::OpKind::ExceptionFinallyPendingObserver)
    };
    let unmarked_observer = OpIR {
        async_work_poll: false,
        ..marked_observer.clone()
    };

    for op in [&explicit_poll, &marked_observer] {
        let requirements = op
            .runtime_requirements()
            .expect("every async-work carrier has generated runtime requirements");
        assert!(requirements.contains(SimpleIrRuntimeRequirements::PENDING_CALL_EVAL_BREAKER));
        assert!(requirements.contains(SimpleIrRuntimeRequirements::EXCEPTION));
        assert!(
            !requirements.contains(SimpleIrRuntimeRequirements::ASYNC_RUNTIME),
            "pending-call polling must not imply scheduling or suspension support"
        );
    }
    assert!(
        !unmarked_observer
            .runtime_requirements()
            .unwrap()
            .contains(SimpleIrRuntimeRequirements::PENDING_CALL_EVAL_BREAKER),
        "the ordinary pending observer must remain a non-polling exception read"
    );

    let capabilities = target_with_runtime(
        SimpleIrRuntimeRequirements::ALL
            .difference(SimpleIrRuntimeRequirements::PENDING_CALL_EVAL_BREAKER),
    );
    let error =
        validate_runtime_target_contract(&function_ir(vec![marked_observer]), &capabilities)
            .expect_err(
                "a target without the runtime boundary must reject before source generation",
            );
    assert!(error.contains("pending-call and eval-breaker polling boundary"));

    let error = validate_runtime_target_contract(
        &function_ir(vec![explicit_poll]),
        &target_with_runtime(
            SimpleIrRuntimeRequirements::ALL.difference(SimpleIrRuntimeRequirements::EXCEPTION),
        ),
    )
    .expect_err("the poll alias must retain the established exception requirement");
    assert!(error.contains("Python exception state"));
}

#[test]
fn extern_linkage_capability_is_one_shared_target_admission_gate() {
    let mut declaration = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "external_helper".to_string(),
        params: vec!["arg".to_string()],
        ops: vec![OpIR {
            kind: molt_ir::OpKind::RetVoid,
            ..OpIR::new(molt_ir::OpKind::RetVoid)
        }],
        ..FunctionIR::default()
    };
    declaration
        .externalize_with_signature()
        .expect("canonical extern declaration");
    let ir = SimpleIR {
        functions: vec![declaration],
        profile: None,
    };

    let error = validate_target_contract(&ir, &crate::tir::TargetInfo::rust_release_fast())
        .expect_err("targets without a provider ABI must reject extern declarations");
    assert!(error.contains("rust target has no extern provider/linkage ABI"));

    let mut linkable = crate::tir::TargetInfo::rust_release_fast();
    linkable.extern_function_linkage = true;
    validate_target_contract(&ir, &linkable)
        .expect("declaration-capable targets admit canonical extern signatures");
}

#[test]
fn fixed_width_targets_admit_exact_float_basics_only() {
    validate_numeric_target_contract(
        &binary(molt_ir::OpKind::Add, "float"),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect("float add is exact in the target policy");

    for kind in [
        molt_ir::OpKind::Pow,
        molt_ir::OpKind::FloorDiv,
        molt_ir::OpKind::Mod,
    ] {
        let error = validate_numeric_target_contract(
            &binary(kind, "float"),
            &crate::tir::TargetInfo::rust_release_fast(),
        )
        .expect_err("non-exact float semantics must reject");
        assert!(error.contains("rejected before source generation"));
    }
}

#[test]
fn fixed_width_targets_reject_integer_arithmetic() {
    let error = validate_numeric_target_contract(
        &binary(molt_ir::OpKind::Add, "int"),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect_err("i64 is not Python integer semantics");
    assert!(error.contains("arbitrary-precision"));
}

#[test]
fn exact_literal_capability_admits_only_complete_in_range_siblings() {
    let target = crate::tir::TargetInfo::luau_release_fast();
    for op in [
        OpIR {
            kind: molt_ir::OpKind::Const,
            value: Some(42),
            out: Some("out".to_string()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            kind: molt_ir::OpKind::ConstInt,
            value: Some(-(1_i64 << 53)),
            out: Some("out".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstInt)
        },
        OpIR {
            kind: molt_ir::OpKind::ConstBigint,
            bytes: Some(molt_ir::literal_payload::integer_literal_bytes(
                1_i128 << 53,
            )),
            out: Some("out".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstBigint)
        },
    ] {
        validate_numeric_target_contract(&function_ir(vec![op]), &target)
            .expect("exact concrete literal must be admitted");
    }
    for payload in [
        molt_ir::literal_payload::integer_literal_bytes(9_007_199_254_740_993),
        molt_ir::literal_payload::integer_literal_bytes(-9_007_199_254_740_993),
        vec![0, 0],
    ] {
        let error = validate_numeric_target_contract(
            &function_ir(vec![OpIR {
                kind: molt_ir::OpKind::ConstBigint,
                bytes: Some(payload.clone()),
                out: Some("out".to_string()),
                ..OpIR::new(molt_ir::OpKind::ConstBigint)
            }]),
            &target,
        )
        .expect_err("unsafe or malformed bigint literal must reject");
        assert!(
            error.contains("exact concrete value authority"),
            "{payload:?}: {error}"
        );
    }
}

#[test]
fn float_literal_stays_outside_integer_admission() {
    validate_numeric_target_contract(
        &function_ir(vec![OpIR {
            kind: molt_ir::OpKind::ConstFloat,
            f_value: Some(1.25),
            out: Some("out".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstFloat)
        }]),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect("a float literal is not an integer literal");
}

#[test]
fn integer_literal_kind_without_integer_payload_is_rejected() {
    // `const` is the integer-literal kind; a float-only payload is malformed
    // wire and must not be admitted as an integer literal.
    let error = validate_numeric_target_contract(
        &function_ir(vec![OpIR {
            kind: molt_ir::OpKind::Const,
            f_value: Some(1.25),
            out: Some("out".to_string()),
            ..OpIR::new(molt_ir::OpKind::Const)
        }]),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect_err("an integer-literal kind without an integer payload must reject");
    assert!(error.contains("integer literal"), "{error}");
}

#[test]
fn execution_frames_are_distinct_from_python_introspection() {
    for kind in [
        molt_ir::OpKind::FrameContextSet,
        molt_ir::OpKind::FrameLocalsSet,
        molt_ir::OpKind::Line,
        molt_ir::OpKind::TraceEnterSlot,
        molt_ir::OpKind::TraceExit,
    ] {
        let ir = function_ir(vec![OpIR {
            kind,
            args: match kind {
                molt_ir::OpKind::FrameContextSet => {
                    Some(vec!["arg0".into(), "kind".into(), "class_cell".into()])
                }
                molt_ir::OpKind::FrameLocalsSet => Some(vec!["locals".into()]),
                _ => None,
            },
            value: (kind == molt_ir::OpKind::Line).then_some(7),
            s_value: (kind == molt_ir::OpKind::TraceEnterSlot).then(|| "f".into()),
            ..OpIR::new(kind)
        }]);
        let error =
            validate_runtime_target_contract(&ir, &crate::tir::TargetInfo::rust_release_fast())
                .expect_err("execution-frame operations must not degrade to target no-ops");
        assert!(error.contains("execution-frame stack and source-location"));

        validate_runtime_target_contract(
            &ir,
            &target_with_runtime(SimpleIrRuntimeRequirements::EXECUTION_FRAME),
        )
        .expect("the execution-frame capability admits its generated sibling family");
    }

    let error = validate_runtime_target_contract(
        &function_ir(vec![OpIR {
            kind: molt_ir::OpKind::Getframe,
            ..OpIR::new(molt_ir::OpKind::Getframe)
        }]),
        &target_with_runtime(SimpleIrRuntimeRequirements::EXECUTION_FRAME),
    )
    .expect_err("execution frames must not imply Python-visible frame objects");
    assert!(error.contains("exact Python-visible frame objects"));
}

#[test]
fn super_context_intrinsics_require_execution_frames_not_locals_introspection() {
    for symbol in ["molt_super_from_frame"] {
        let carriers = [
            molt_ir::OpKind::Call,
            molt_ir::OpKind::CallInternal,
            molt_ir::OpKind::BuiltinFunc,
            molt_ir::OpKind::FuncNew,
            molt_ir::OpKind::FuncNewClosure,
        ]
        .into_iter()
        .map(|kind| OpIR {
            kind,
            s_value: Some(symbol.to_string()),
            ..OpIR::new(kind)
        })
        .chain([OpIR {
            kind: molt_ir::OpKind::ModuleGetAttr,
            runtime_symbol: Some(symbol.to_string()),
            ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
        }]);
        for op in carriers {
            let requirements = op.runtime_requirements().unwrap();
            assert!(requirements.contains(SimpleIrRuntimeRequirements::EXECUTION_FRAME));
            assert!(!requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));
            assert_eq!(
                op.uses_execution_frame(),
                matches!(
                    op.kind,
                    molt_ir::OpKind::Call | molt_ir::OpKind::CallInternal
                ),
                "callable acquisition requires target support, not an already-entered frame: {op:?}"
            );
            let ir = function_ir(vec![op]);
            validate_runtime_target_contract(&ir, &runtime_without_frame_introspection())
                .expect("semantic frame context must not require a locals snapshot");
            let without_frames = target_with_runtime(
                SimpleIrRuntimeRequirements::ALL
                    .difference(SimpleIrRuntimeRequirements::EXECUTION_FRAME),
            );
            let error = validate_runtime_target_contract(&ir, &without_frames)
                .expect_err("frame context must not silently become a target no-op");
            assert!(error.contains("execution-frame stack and source-location"));
        }
    }
}

#[test]
fn runtime_symbol_provenance_rejects_at_acquisition_not_at_transport_use() {
    let acquisition = OpIR {
        kind: molt_ir::OpKind::ModuleGetAttr,
        runtime_symbol: Some("molt_getframe".to_string()),
        out: Some("frame_callable".to_string()),
        ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
    };
    let requirements = acquisition
        .runtime_requirements()
        .expect("acquisition op must be classified");
    assert!(requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));

    let mut ir = function_ir(vec![
        acquisition,
        OpIR {
            kind: molt_ir::OpKind::Call,
            args: Some(vec!["frame_callable".to_string()]),
            out: Some("dynamic_result".to_string()),
            ..OpIR::new(molt_ir::OpKind::Call)
        },
    ]);
    ir.functions[0].execution_context = ExecutionContextPolicy::None;
    let error = validate_runtime_target_contract(&ir, &runtime_without_frame_introspection())
        .expect_err("producer acquisition must reject before any dynamic call transport matters");
    assert!(error.contains("f:op#0 `module_get_attr`"), "{error}");
    assert!(
        error.contains("exact Python-visible frame objects"),
        "{error}"
    );
}

#[test]
fn typed_may_provenance_rejects_without_inventing_a_runtime_symbol() {
    let acquisition = OpIR {
        kind: molt_ir::OpKind::ModuleGetAttr,
        runtime_requirement_bits: SimpleIrRuntimeRequirements::FRAME_INTROSPECTION.bits(),
        out: Some("maybe_frame_callable".to_string()),
        ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
    };
    assert!(acquisition.runtime_symbol.is_none());
    let requirements = acquisition
        .runtime_requirements()
        .expect("typed requirement bits must participate in target admission");
    assert!(requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));

    let error = validate_runtime_target_contract(
        &function_ir(vec![acquisition]),
        &runtime_without_frame_introspection(),
    )
    .expect_err("may-provenance must reject on a target without frame introspection");
    assert!(error.contains("f:op#0 `module_get_attr`"), "{error}");
    assert!(
        error.contains("exact Python-visible frame objects"),
        "{error}"
    );
}

#[test]
fn every_generated_runtime_requirement_carrier_parses_and_reaches_admission() {
    for &kind in SIMPLEIR_RUNTIME_REQUIREMENT_CARRIER_KINDS {
        let source = format!(
            r#"{{"functions":[{{"return_abi": "void", "name":"f","params":[],"ops":[{{"kind":"{kind}","runtime_requirement_bits":{},"out":"value"}}]}}]}}"#,
            SimpleIrRuntimeRequirements::FRAME_INTROSPECTION.bits(),
        );
        let ir = SimpleIR::from_json_str(&source)
            .unwrap_or_else(|error| panic!("generated carrier {kind} must parse: {error}"));
        let error = validate_runtime_target_contract(&ir, &runtime_without_frame_introspection())
            .expect_err("every explicit carrier must reach target admission");
        assert!(error.contains(&format!("f:op#0 `{kind}`")), "{error}");
        assert!(
            error.contains("exact Python-visible frame objects"),
            "{error}"
        );
    }
}

#[test]
fn every_generated_runtime_symbol_carrier_parses_and_reaches_admission() {
    for &kind in SIMPLEIR_RUNTIME_SYMBOL_CARRIER_KINDS {
        let source = format!(
            r#"{{"functions":[{{"return_abi": "void", "name":"f","params":[],"ops":[{{"kind":"{kind}","runtime_symbol":"molt_getframe","out":"value"}}]}}]}}"#,
        );
        let ir = SimpleIR::from_json_str(&source)
            .unwrap_or_else(|error| panic!("generated symbol carrier {kind} must parse: {error}"));
        let error = validate_runtime_target_contract(&ir, &runtime_without_frame_introspection())
            .expect_err("every runtime-symbol carrier must reach target admission");
        assert!(error.contains(&format!("f:op#0 `{kind}`")), "{error}");
        assert!(
            error.contains("exact Python-visible frame objects"),
            "{error}"
        );
    }
}

#[test]
fn every_canonical_runtime_symbol_field_shares_frame_introspection_admission() {
    for symbol in [
        "molt_getframe",
        "molt_inspect_currentframe",
        "molt_sys_settrace",
        "molt_sys_gettrace",
        "molt_sys_setprofile",
        "molt_sys_getprofile",
    ] {
        let carriers = [
            OpIR {
                kind: molt_ir::OpKind::ModuleGetAttr,
                runtime_symbol: Some(symbol.to_string()),
                ..OpIR::new(molt_ir::OpKind::ModuleGetAttr)
            },
            OpIR {
                kind: molt_ir::OpKind::BuiltinFunc,
                builtin_name: Some(symbol.to_string()),
                ..OpIR::new(molt_ir::OpKind::BuiltinFunc)
            },
            OpIR {
                kind: molt_ir::OpKind::CallInternal,
                s_value: Some(symbol.to_string()),
                ..OpIR::new(molt_ir::OpKind::CallInternal)
            },
        ]
        .into_iter()
        .chain(
            [
                molt_ir::OpKind::Call,
                molt_ir::OpKind::BuiltinFunc,
                molt_ir::OpKind::FuncNew,
                molt_ir::OpKind::FuncNewClosure,
            ]
            .into_iter()
            .map(|kind| OpIR {
                kind,
                s_value: Some(symbol.to_string()),
                ..OpIR::new(kind)
            }),
        );
        for op in carriers {
            let requirements = op
                .runtime_requirements()
                .expect("runtime-call op must be classified");
            assert!(
                requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION),
                "{symbol} via {}",
                op.kind
            );
        }
    }
}

#[test]
fn runtime_symbol_text_is_not_callable_identity_on_opaque_or_literal_operations() {
    let restricted = target_with_runtime(
        SimpleIrRuntimeRequirements::ALL
            .difference(SimpleIrRuntimeRequirements::IMPORT_PROTOCOL)
            .difference(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION),
    );
    for symbol in ["molt_module_import", "molt_getframe"] {
        for kind in [
            molt_ir::OpKind::ConstStr,
            molt_ir::OpKind::CallFunc,
            molt_ir::OpKind::CallBind,
            molt_ir::OpKind::CallGuarded,
            molt_ir::OpKind::InvokeFfi,
        ] {
            let op = OpIR {
                kind,
                s_value: Some(symbol.to_string()),
                ..OpIR::new(kind)
            };
            validate_runtime_target_contract(&function_ir(vec![op]), &restricted)
                .expect("literal payloads and opaque call hints do not acquire or invoke the named runtime symbol");
        }
    }
}
