use super::*;
use crate::tir::op_kinds_generated::{
    SIMPLEIR_RUNTIME_REQUIREMENT_CARRIER_KINDS, SIMPLEIR_RUNTIME_SYMBOL_CARRIER_KINDS,
    SimpleIrRuntimeRequirements,
};
use crate::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR};

fn function_ir(ops: Vec<OpIR>) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "f".to_string(),
            ops,
            ..FunctionIR::default()
        }],
        profile: None,
    }
}

#[test]
fn raw_boxed_stack_allocation_is_rejected_on_every_target() {
    let ir = function_ir(vec![OpIR {
        kind: "stack_alloc".into(),
        value: Some(16),
        out: Some("object".into()),
        ..OpIR::default()
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
fn retired_class_frame_operation_is_rejected_on_every_target() {
    for payload in [None, Some(-1), Some(16), Some(i64::MAX)] {
        let ir = function_ir(vec![
            OpIR {
                kind: "object_new_bound_stack".into(),
                args: Some(vec!["class".into()]),
                value: payload,
                out: Some("object".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["object".into()]),
                ..OpIR::default()
            },
        ]);
        for target in [
            crate::tir::TargetInfo::native_release_fast(),
            crate::tir::TargetInfo::wasm_release_fast(),
            crate::tir::TargetInfo::llvm_release_fast(),
            crate::tir::TargetInfo::luau_release_fast(),
            crate::tir::TargetInfo::rust_release_fast(),
            crate::tir::TargetInfo::mlir_release_fast(),
        ] {
            let error = validate_runtime_target_contract(&ir, &target).unwrap_err();
            assert!(error.contains("object_new_bound_stack"));
            assert!(error.contains("unclassified"));
            assert!(error.contains("f:op#0"));
        }
    }
}

fn binary(kind: &str, ty: &str) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "f".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![OpIR {
                kind: kind.to_string(),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::default()
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
        kind: "async_work_poll".to_string(),
        value: Some(9),
        ..OpIR::default()
    };
    let marked_observer = OpIR {
        kind: "exception_finally_pending_observer".to_string(),
        out: Some("pending".to_string()),
        async_work_poll: true,
        ..OpIR::default()
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
        name: "external_helper".to_string(),
        params: vec!["arg".to_string()],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
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
        &binary("add", "float"),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect("float add is exact in the target policy");

    for kind in ["pow", "floor_div", "mod"] {
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
        &binary("add", "int"),
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
            kind: "const".to_string(),
            value: Some(42),
            out: Some("out".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_int".to_string(),
            value: Some(-(1_i64 << 53)),
            out: Some("out".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_bigint".to_string(),
            s_value: Some((1_u64 << 53).to_string()),
            out: Some("out".to_string()),
            ..OpIR::default()
        },
    ] {
        validate_numeric_target_contract(&function_ir(vec![op]), &target)
            .expect("exact concrete literal must be admitted");
    }
    for payload in ["9007199254740993", "-9007199254740993", "not-an-int"] {
        let error = validate_numeric_target_contract(
            &function_ir(vec![OpIR {
                kind: "const_bigint".to_string(),
                s_value: Some(payload.to_string()),
                out: Some("out".to_string()),
                ..OpIR::default()
            }]),
            &target,
        )
        .expect_err("unsafe or malformed bigint literal must reject");
        assert!(error.contains("exact concrete value authority"));
    }
}

#[test]
fn generic_const_non_integer_payload_stays_outside_integer_admission() {
    validate_numeric_target_contract(
        &function_ir(vec![OpIR {
            kind: "const".to_string(),
            f_value: Some(1.25),
            out: Some("out".to_string()),
            ..OpIR::default()
        }]),
        &crate::tir::TargetInfo::rust_release_fast(),
    )
    .expect("generic const float payload is not an integer literal");
}

#[test]
fn execution_frames_are_distinct_from_python_introspection() {
    for kind in ["frame_locals_set", "line", "trace_enter_slot", "trace_exit"] {
        let ir = function_ir(vec![OpIR {
            kind: kind.to_string(),
            args: (kind == "frame_locals_set").then(|| vec!["locals".to_string()]),
            value: matches!(kind, "line" | "trace_enter_slot").then_some(7),
            ..OpIR::default()
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
            kind: "getframe".to_string(),
            ..OpIR::default()
        }]),
        &target_with_runtime(SimpleIrRuntimeRequirements::EXECUTION_FRAME),
    )
    .expect_err("execution frames must not imply Python-visible frame objects");
    assert!(error.contains("exact Python-visible frame objects"));
}

#[test]
fn super_context_intrinsics_require_execution_frames_not_locals_introspection() {
    for symbol in ["molt_frame_context_set", "molt_super_from_frame"] {
        let carriers = [
            OpIR {
                kind: "call_internal".to_string(),
                s_value: Some(symbol.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "builtin_func".to_string(),
                s_value: Some(symbol.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".to_string(),
                runtime_symbol: Some(symbol.to_string()),
                ..OpIR::default()
            },
        ];
        for op in carriers {
            let requirements = op.runtime_requirements().unwrap();
            assert!(requirements.contains(SimpleIrRuntimeRequirements::EXECUTION_FRAME));
            assert!(!requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));
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
        kind: "module_get_attr".to_string(),
        runtime_symbol: Some("molt_getframe".to_string()),
        out: Some("frame_callable".to_string()),
        ..OpIR::default()
    };
    let requirements = acquisition
        .runtime_requirements()
        .expect("acquisition op must be classified");
    assert!(requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));

    let mut ir = function_ir(vec![
        acquisition,
        OpIR {
            kind: "call".to_string(),
            args: Some(vec!["frame_callable".to_string()]),
            out: Some("dynamic_result".to_string()),
            ..OpIR::default()
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
        kind: "module_get_attr".to_string(),
        runtime_requirement_bits: SimpleIrRuntimeRequirements::FRAME_INTROSPECTION.bits(),
        out: Some("maybe_frame_callable".to_string()),
        ..OpIR::default()
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
            r#"{{"functions":[{{"name":"f","params":[],"ops":[{{"kind":"{kind}","runtime_requirement_bits":{},"out":"value"}}]}}]}}"#,
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
            r#"{{"functions":[{{"name":"f","params":[],"ops":[{{"kind":"{kind}","runtime_symbol":"molt_getframe","out":"value"}}]}}]}}"#,
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
        for op in [
            OpIR {
                kind: "module_get_attr".to_string(),
                runtime_symbol: Some(symbol.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "builtin_func".to_string(),
                builtin_name: Some(symbol.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_internal".to_string(),
                s_value: Some(symbol.to_string()),
                ..OpIR::default()
            },
        ] {
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
