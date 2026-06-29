use super::*;
use crate::runtime_import_abi::{MOLT_DEC_REF, NATIVE_RUNTIME_HELPER_IMPORTS};

#[test]
fn native_compiles_canonical_bare_get_attr() {
    let func = FunctionIR {
        name: "bare_get_attr_repr".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "get_attr".to_string(),
                args: Some(vec!["self".to_string()]),
                s_value: Some("optional".to_string()),
                out: Some("v0".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    // Must not panic at the dispatch's no-codegen catch-all; the canonical
    // `get_attr` lowers to the generic-by-name runtime attribute fetch.
    let clif = compile_function_to_clif_text(vec![func], "bare_get_attr_repr");
    assert!(
        clif.contains("call"),
        "canonical bare `get_attr` must lower to a runtime attribute-get call; \
         got CLIF:\n{clif}",
    );
}

#[test]
fn native_backend_skips_trace_imports_by_default() {
    let bytes = compile_trace_probe_object(None);

    assert!(
        !bytes
            .windows(b"molt_trace_enter_slot".len())
            .any(|window| window == b"molt_trace_enter_slot")
    );
    assert!(
        !bytes
            .windows(b"molt_trace_exit".len())
            .any(|window| window == b"molt_trace_exit")
    );
}

#[test]
fn native_backend_can_opt_in_trace_imports() {
    let bytes = compile_trace_probe_object(Some("1"));

    assert!(
        bytes
            .windows(b"molt_trace_enter_slot".len())
            .any(|window| window == b"molt_trace_enter_slot")
    );
    assert!(
        bytes
            .windows(b"molt_trace_exit".len())
            .any(|window| window == b"molt_trace_exit")
    );
}

#[test]
fn native_backend_import_ids_are_cached_by_symbol() {
    let mut backend = SimpleBackend::new();

    let first = SimpleBackend::import_runtime_func_id_split(
        &mut backend.module,
        &mut backend.import_ids,
        MOLT_DEC_REF,
    );
    let second = SimpleBackend::import_runtime_func_id_split(
        &mut backend.module,
        &mut backend.import_ids,
        MOLT_DEC_REF,
    );

    assert_eq!(first, second);
    assert_eq!(backend.import_ids.len(), 1);
}

#[test]
fn native_runtime_helper_import_descriptors_are_unique() {
    let names: BTreeSet<&str> = NATIVE_RUNTIME_HELPER_IMPORTS
        .iter()
        .map(|signature| signature.name)
        .collect();

    assert_eq!(names.len(), NATIVE_RUNTIME_HELPER_IMPORTS.len());
    assert!(names.contains("molt_inc_ref_obj"));
    assert!(names.contains("molt_dec_ref_obj"));
    assert!(names.contains("molt_task_new"));
    assert!(names.contains("molt_cancel_token_get_current"));
    assert!(names.contains("molt_task_register_token_owned"));
    assert!(names.contains("molt_asyncgen_new"));
}

#[test]
fn native_backend_skips_profile_store_imports_when_function_has_no_store_ops() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(
        !output
            .bytes
            .windows(b"molt_profile_struct_field_store".len())
            .any(|window| window == b"molt_profile_struct_field_store")
    );
    assert!(
        !output
            .bytes
            .windows(b"molt_profile_enabled".len())
            .any(|window| window == b"molt_profile_enabled")
    );
}

#[test]
fn native_backend_keeps_profile_store_imports_when_function_has_store_ops() {
    let _guard = acquire_backend_env_lock();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("obj".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("value".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "value".to_string()]),
                    value: Some(8),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(
        output
            .bytes
            .windows(b"molt_profile_struct_field_store".len())
            .any(|window| window == b"molt_profile_struct_field_store")
    );
    assert!(
        output
            .bytes
            .windows(b"molt_profile_enabled".len())
            .any(|window| window == b"molt_profile_enabled")
    );
}

fn compile_check_exception_target_shape(name: &str, target: Option<i64>) {
    compile_function_to_clif_text(
        vec![FunctionIR {
            name: name.to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("sentinel".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: target,
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        name,
    );
}

#[test]
#[should_panic(
    expected = "check_exception missing target label id in function `native_check_exception_missing_target` op 1"
)]
fn check_exception_missing_target_fails_closed_at_codegen() {
    compile_check_exception_target_shape("native_check_exception_missing_target", None);
}

#[test]
#[should_panic(
    expected = "check_exception target label 7 is not present in native label map for function `native_check_exception_orphan_target` op 1"
)]
fn check_exception_orphan_target_fails_closed_at_codegen() {
    compile_check_exception_target_shape("native_check_exception_orphan_target", Some(7));
}

#[test]
fn native_backend_compiles_exception_label_guard_if_without_else() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "hello_regress____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "exception_stack_enter".to_string(),
                    out: Some("v74".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("v75".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v76".to_string()),
                    s_value: Some("hello_regress".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    out: Some("v77".to_string()),
                    args: Some(vec!["v76".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v78".to_string()),
                    s_value: Some("__dict__".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    out: Some("v79".to_string()),
                    args: Some(vec!["v77".to_string(), "v78".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v79".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["v75".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".to_string(),
                    args: Some(vec!["v74".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("v80".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v81".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    out: Some("v82".to_string()),
                    args: Some(vec!["v80".to_string(), "v81".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".to_string(),
                    out: Some("v83".to_string()),
                    args: Some(vec!["v82".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["v83".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["v80".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn native_backend_compiles_tir_roundtripped_exception_label_guard_if_without_else() {
    let func = FunctionIR {
        name: "hello_regress____molt_globals_builtin__".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "exception_stack_enter".to_string(),
                out: Some("v74".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_depth".to_string(),
                out: Some("v75".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("v76".to_string()),
                s_value: Some("hello_regress".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".to_string(),
                out: Some("v77".to_string()),
                args: Some(vec!["v76".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("v78".to_string()),
                s_value: Some("__dict__".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".to_string(),
                out: Some("v79".to_string()),
                args: Some(vec!["v77".to_string(), "v78".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("v79".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_set_depth".to_string(),
                args: Some(vec!["v75".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_exit".to_string(),
                args: Some(vec!["v74".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("v80".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("v81".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "is".to_string(),
                out: Some("v82".to_string()),
                args: Some(vec!["v80".to_string(), "v81".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "not".to_string(),
                out: Some("v83".to_string()),
                args: Some(vec!["v82".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".to_string(),
                args: Some(vec!["v83".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "raise".to_string(),
                args: Some(vec!["v80".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("v84".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("v84".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let roundtripped = roundtrip_function_through_tir(&func);
    let clif = compile_function_to_clif_text(
        vec![roundtripped],
        "hello_regress____molt_globals_builtin__",
    );

    assert!(
        clif.contains("return"),
        "TIR-roundtripped exception function must compile to CLIF:\n{clif}"
    );
}

#[cfg(feature = "llvm")]
#[test]
fn native_backend_compiles_tir_roundtripped_nested_loops() {
    let func = FunctionIR {
        name: "nested_loops".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("total".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(2),
                out: Some("outer_limit".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(2),
                out: Some("inner_limit".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("one".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "lt".into(),
                args: Some(vec!["i".into(), "outer_limit".into()]),
                out: Some("outer_cond".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_false".into(),
                args: Some(vec!["outer_cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("j".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "lt".into(),
                args: Some(vec!["j".into(), "inner_limit".into()]),
                out: Some("inner_cond".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_false".into(),
                args: Some(vec!["inner_cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["total".into(), "j".into()]),
                out: Some("total".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["j".into(), "one".into()]),
                out: Some("j".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["i".into(), "one".into()]),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("total".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let roundtripped = roundtrip_function_through_tir(&func);
    let clif = compile_function_to_clif_text(vec![roundtripped], "nested_loops");

    assert!(
        clif.contains("return"),
        "TIR-roundtripped nested-loop function must compile to CLIF:\n{clif}"
    );
}
