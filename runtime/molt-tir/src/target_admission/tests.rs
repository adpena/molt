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

fn runtime_without_frame_introspection() -> RuntimeTargetCapabilities {
    RuntimeTargetCapabilities {
        extern_function_linkage: false,
        execution_frame_state: true,
        python_frame_introspection: false,
        python_identity: true,
        tuple_representation: true,
        exception_model: true,
        deterministic_lifetime: true,
        format_protocol: true,
        iterable_protocol: true,
        object_model: true,
        python_truthiness: true,
        python_comparison: true,
        structured_runtime_errors: true,
        async_runtime: true,
        unstructured_control_flow: true,
        host_capabilities: true,
    }
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

    let error = validate_target_contract(
        &ir,
        "source",
        NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        RuntimeTargetCapabilities::NONE,
    )
    .expect_err("targets without a provider ABI must reject extern declarations");
    assert!(error.contains("source target has no extern provider/linkage ABI"));

    validate_target_contract(
        &ir,
        "linkable",
        NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        RuntimeTargetCapabilities {
            extern_function_linkage: true,
            ..RuntimeTargetCapabilities::NONE
        },
    )
    .expect("declaration-capable targets admit canonical extern signatures");
}

#[test]
fn fixed_width_targets_admit_exact_float_basics_only() {
    validate_numeric_target_contract(
        &binary("add", "float"),
        "test",
        NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
    )
    .expect("float add is exact in the target policy");

    for kind in ["pow", "floor_div", "mod"] {
        let error = validate_numeric_target_contract(
            &binary(kind, "float"),
            "test",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect_err("non-exact float semantics must reject");
        assert!(error.contains("rejected before source generation"));
    }
}

#[test]
fn fixed_width_targets_reject_integer_arithmetic() {
    let error = validate_numeric_target_contract(
        &binary("add", "int"),
        "test",
        NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
    )
    .expect_err("i64 is not Python integer semantics");
    assert!(error.contains("arbitrary-precision"));
}

#[test]
fn exact_literal_capability_admits_only_complete_in_range_siblings() {
    let capabilities = NumericTargetCapabilities::LUAU_EXACT_INTEGER_LITERALS;
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
        validate_numeric_target_contract(&function_ir(vec![op]), "luau", capabilities)
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
            "luau",
            capabilities,
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
        "fixed",
        NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
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
            validate_runtime_target_contract(&ir, "no-frames", RuntimeTargetCapabilities::NONE)
                .expect_err("execution-frame operations must not degrade to target no-ops");
        assert!(error.contains("execution-frame stack and source-location"));

        validate_runtime_target_contract(
            &ir,
            "frames",
            RuntimeTargetCapabilities {
                execution_frame_state: true,
                ..RuntimeTargetCapabilities::NONE
            },
        )
        .expect("the execution-frame capability admits its generated sibling family");
    }

    let error = validate_runtime_target_contract(
        &function_ir(vec![OpIR {
            kind: "getframe".to_string(),
            ..OpIR::default()
        }]),
        "execution-only",
        RuntimeTargetCapabilities {
            execution_frame_state: true,
            ..RuntimeTargetCapabilities::NONE
        },
    )
    .expect_err("execution frames must not imply Python-visible frame objects");
    assert!(error.contains("exact Python-visible frame objects"));
}

#[test]
fn runtime_symbol_provenance_rejects_at_acquisition_not_at_transport_use() {
    let acquisition = OpIR {
        kind: "module_get_attr".to_string(),
        runtime_symbol: Some("molt_getframe".to_string()),
        out: Some("frame_callable".to_string()),
        ..OpIR::default()
    };
    let requirements =
        simpleir_op_runtime_requirements(&acquisition).expect("acquisition op must be classified");
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
    let error = validate_runtime_target_contract(
        &ir,
        "execution-only",
        runtime_without_frame_introspection(),
    )
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
    let requirements = simpleir_op_runtime_requirements(&acquisition)
        .expect("typed requirement bits must participate in target admission");
    assert!(requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION));

    let error = validate_runtime_target_contract(
        &function_ir(vec![acquisition]),
        "execution-only",
        runtime_without_frame_introspection(),
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
        let error = validate_runtime_target_contract(
            &ir,
            "execution-only",
            runtime_without_frame_introspection(),
        )
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
        let error = validate_runtime_target_contract(
            &ir,
            "execution-only",
            runtime_without_frame_introspection(),
        )
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
            let requirements =
                simpleir_op_runtime_requirements(&op).expect("runtime-call op must be classified");
            assert!(
                requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION),
                "{symbol} via {}",
                op.kind
            );
        }
    }
}
