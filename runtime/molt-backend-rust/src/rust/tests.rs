use super::*;
use crate::{FunctionIR, OpIR, SimpleIR};

#[test]
fn compile_keeps_annotation_functions_when_referenced() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "__main____annotate__".to_string(),
                params: vec!["args".to_string()],
                ops: vec![OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn __main____annotate__("));
}

#[test]
fn compile_int_from_str_of_obj_preserves_base_operand() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![
                "value".to_string(),
                "base".to_string(),
                "has_base".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "int_from_str_of_obj".to_string(),
                    args: Some(vec![
                        "value".to_string(),
                        "base".to_string(),
                        "has_base".to_string(),
                    ]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("out".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("molt_bool(&has_base)"));
    assert!(source.contains("let __base = molt_int(&base);"));
    assert!(source.contains("i64::from_str_radix(__s.trim(), __base as u32)"));
}

#[test]
fn compile_numeric_equality_does_not_fall_back_for_non_numeric_values() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "cmp_eq".to_string(),
                args: Some(vec!["v0".to_string(), "v1".to_string()]),
                out: Some("v2".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_is_numeric(x: &MoltValue) -> bool"));
    assert!(source.contains("_ if molt_is_numeric(a) && molt_is_numeric(b) =>"));
    assert!(source.contains("_ => false,"));
}

#[test]
fn compile_rust_arithmetic_fast_path_ignores_transport_hints() {
    let mut backend = RustBackend::new();
    let mut add = OpIR {
        kind: "add".to_string(),
        args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
        out: Some("sum".to_string()),
        ..OpIR::default()
    };
    add.fast_int = Some(true);
    add.type_hint = Some("int".to_string());
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                add,
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("sum".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("let mut sum: MoltValue = molt_add(lhs.clone(), rhs.clone());"));
    assert!(!source.contains(
        "let mut sum: MoltValue = MoltValue::Int(molt_int(&lhs).wrapping_add(molt_int(&rhs)))"
    ));
}

#[test]
fn compile_rust_arithmetic_fast_path_uses_typed_operands_without_transport_hints() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("sum".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains(
        "let mut sum: MoltValue = MoltValue::Int(molt_int(&lhs).wrapping_add(molt_int(&rhs)))"
    ));
    assert!(!source.contains("let mut sum: MoltValue = molt_add(lhs.clone(), rhs.clone());"));
}

#[test]
fn compile_list_append_writes_back_indexed_aliases() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "helper".to_string(),
                params: vec!["v0".to_string(), "v1".to_string(), "v3".to_string()],
                ops: vec![
                    OpIR {
                        kind: "index".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_append".to_string(),
                        args: Some(vec!["v2".to_string(), "v3".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "return_none".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("let mut __alias_key_v2: MoltValue = v1.clone();"));
    assert!(source.contains("molt_list_append(&mut v2, v3.clone());"));
    assert!(source.contains("molt_set_item(&mut v0, __alias_key_v2.clone(), v2.clone());"));
}

#[test]
fn compile_call_method_uses_s_value_method_name() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["items".to_string(), "value".to_string()],
            ops: vec![
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("append".to_string()),
                    args: Some(vec!["items".to_string(), "value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend
        .compile_checked(&ir)
        .expect("call_method should lower from s_value without stub markers");
    assert!(source.contains("molt_list_append(&mut items, value.clone());"));
    assert!(!source.contains("MOLT_STUB: method"));
}

#[test]
fn compile_ord_at_emits_fused_helper() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "ord_at_unicode".to_string(),
            params: vec!["s".to_string(), "i".to_string()],
            ops: vec![
                OpIR {
                    kind: "ord_at".to_string(),
                    args: Some(vec!["s".to_string(), "i".to_string()]),
                    out: Some("code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("code".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["str".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend
        .compile_checked(&ir)
        .expect("ord_at should lower without stub markers");
    assert!(source.contains("fn molt_ord_at(obj: &MoltValue, key: &MoltValue)"));
    assert!(source.contains("fn molt_get_item(obj: &MoltValue, key: &MoltValue)"));
    assert!(source.contains("fn molt_ord(x: &MoltValue)"));
    assert!(source.contains("let mut code: MoltValue = molt_ord_at(&s, &i);"));
    assert!(!source.contains("MOLT_STUB"));
}

#[test]
fn compile_code_slots_contains_and_ref_markers_without_stubs() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![
                "filename".to_string(),
                "name".to_string(),
                "firstlineno".to_string(),
                "linetable".to_string(),
                "varnames".to_string(),
                "names".to_string(),
                "argcount".to_string(),
                "posonlyargcount".to_string(),
                "kwonlyargcount".to_string(),
                "container".to_string(),
                "needle".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "code_slots_init".to_string(),
                    value: Some(4),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "code_new".to_string(),
                    args: Some(vec![
                        "filename".to_string(),
                        "name".to_string(),
                        "firstlineno".to_string(),
                        "linetable".to_string(),
                        "varnames".to_string(),
                        "names".to_string(),
                        "argcount".to_string(),
                        "posonlyargcount".to_string(),
                        "kwonlyargcount".to_string(),
                    ]),
                    out: Some("code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "inc_ref".to_string(),
                    args: Some(vec!["code".to_string()]),
                    out: Some("owned_code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(2),
                    args: Some(vec!["owned_code".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "trace_enter_slot".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "frame_locals_set".to_string(),
                    args: Some(vec!["container".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_enter".to_string(),
                    out: Some("exc_base".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("exc_depth".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["exc_depth".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".to_string(),
                    args: Some(vec!["exc_base".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("last_exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last_pending".to_string(),
                    out: Some("pending_exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_clear".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "trace_exit".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dec_ref".to_string(),
                    args: Some(vec!["owned_code".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "contains".to_string(),
                    args: Some(vec!["container".to_string(), "needle".to_string()]),
                    out: Some("present".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("present".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend
        .compile_checked(&ir)
        .expect("Rust source should lower code metadata, contains, and ref markers");
    assert!(source.contains("fn molt_code_new("));
    assert!(source.contains("fn molt_code_slots_init("));
    assert!(source.contains("fn molt_code_slot_set("));
    assert!(source.contains("molt_code_slots_init(4);"));
    assert!(source.contains(
        "let mut code: MoltValue = molt_code_new(&filename, &name, &firstlineno, &linetable, &varnames, &names, &argcount, &posonlyargcount, &kwonlyargcount);"
    ));
    assert!(source.contains("let mut owned_code: MoltValue = code.clone();"));
    assert!(source.contains("molt_code_slot_set(2, &owned_code);"));
    assert!(source.contains("fn molt_exception_stack_enter() -> MoltValue"));
    assert!(source.contains("fn molt_trace_enter_slot(code_id: i64) -> MoltValue"));
    assert!(source.contains("let mut exc_base: MoltValue = molt_exception_stack_enter();"));
    assert!(source.contains("let mut exc_depth: MoltValue = molt_exception_stack_depth();"));
    assert!(source.contains("molt_exception_stack_set_depth(&exc_depth);"));
    assert!(source.contains("molt_exception_stack_exit(&exc_base);"));
    assert!(source.contains("let mut last_exc: MoltValue = molt_exception_last();"));
    assert!(source.contains("let mut pending_exc: MoltValue = molt_exception_last_pending();"));
    assert!(
        source.contains(
            "let mut present: MoltValue = MoltValue::Bool(molt_in(&needle, &container));"
        )
    );
    assert!(!source.contains("MOLT_STUB"));
}

#[test]
fn compile_checked_reports_stub_markers() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unsupported".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "matmul".to_string(),
                out: Some("value".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("unsupported ops should be rejected with marker details");
    // Fail-closed authority is the emit-time accumulator, which names the op kind
    // and backend and refuses to emit fail-open codegen.
    assert!(
        err.contains("refuses to emit fail-open codegen"),
        "error must come from the fail-closed accumulator, got: {err}"
    );
    assert!(err.contains("matmul"), "diagnostic must name the op kind, got: {err}");
}

#[test]
fn compile_boolean_short_circuit_omits_unused_if_parentheses() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "and".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "or".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v3".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("if !molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
    assert!(source.contains("if molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
    assert!(!source.contains("(if !molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
    assert!(!source.contains("(if molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
}

#[test]
fn compile_unpack_sequence_lowers_outputs_instead_of_stub() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["seq".to_string()],
            ops: vec![
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    args: Some(vec![
                        "seq".to_string(),
                        "left".to_string(),
                        "right".to_string(),
                    ]),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("pair".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("pair".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_unpack_sequence("));
    assert!(source.contains("fn molt_unpack_too_many_message("));
    assert!(source.contains("fn molt_runtime_target_at_least("));
    assert!(source.contains("cannot unpack non-iterable {} object"));
    assert!(!source.contains("cannot unpack non-sequence"));
    assert!(source.contains("let __unpack_seq"));
    assert!(source.contains("let mut left: MoltValue = __unpack_seq[0].clone();"));
    assert!(source.contains("let mut right: MoltValue = __unpack_seq[1].clone();"));
    assert!(!source.contains("MOLT_STUB: unpack_sequence"));
}

#[test]
fn compile_module_cache_ops_lower_to_runtime_cache() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("alpha".to_string()),
                    out: Some("name".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("miss".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_new".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("module".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_set".to_string(),
                    args: Some(vec!["name".to_string(), "module".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("hit".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_del".to_string(),
                    args: Some(vec!["name".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend
        .compile_checked(&ir)
        .expect("module cache ops should lower without stub markers");
    assert!(source.contains("fn molt_module_cache_get("));
    assert!(source.contains("fn molt_module_cache_set("));
    assert!(source.contains("fn molt_module_cache_del("));
    assert!(source.contains("let mut miss: MoltValue = molt_module_cache_get(&name);"));
    assert!(source.contains("molt_module_cache_set(&name, module.clone());"));
    assert!(source.contains("let mut hit: MoltValue = molt_module_cache_get(&name);"));
    assert!(source.contains("molt_module_cache_del(&name);"));
    assert!(!source.contains("let mut miss: MoltValue = MoltValue::Bool(true);"));
    assert!(!source.contains("let mut hit: MoltValue = MoltValue::Bool(true);"));
    assert!(!source.contains("MOLT_STUB: module_cache"));
}

#[test]
fn compile_const_bigint_lowers_exact_i64_literal() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some("2305843009213693951".to_string()),
                    out: Some("big".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend
        .compile_checked(&ir)
        .expect("i64-sized bigint literal should lower exactly");
    assert!(source.contains("let mut big: MoltValue = MoltValue::Int(2305843009213693951i64);"));
    assert!(!source.contains("MOLT_STUB: const_bigint"));
    assert!(
        !source.contains("MoltValue::Int(\"2305843009213693951\".parse::<i64>().unwrap_or(0))")
    );
}

#[test]
fn compile_checked_rejects_unrepresented_literal_values() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some("9223372036854775808".to_string()),
                    out: Some("too_big".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bytes".to_string(),
                    s_value: Some("payload".to_string()),
                    out: Some("bytes".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_ellipsis".to_string(),
                    out: Some("ellipsis".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("unsupported literal value representations must fail closed");
    // The fail-closed accumulator (authority) names each unsupported op kind and
    // carries its reason; it fires before the retained MOLT_STUB text scan.
    assert!(
        err.contains("refuses to emit fail-open codegen"),
        "error must come from the fail-closed accumulator, got: {err}"
    );
    assert!(err.contains("const_bigint"), "got: {err}");
    assert!(err.contains("bigint literal exceeds Rust backend i64 value representation"));
    assert!(err.contains("const_bytes"), "got: {err}");
    assert!(err.contains("bytes literals require a Rust backend bytes value representation"));
    assert!(err.contains("const_ellipsis"), "got: {err}");
}

#[test]
fn compile_store_var_and_load_var_use_named_local_storage() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("src".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("rows".to_string()),
                    args: Some(vec!["src".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("rows".to_string()),
                    out: Some("tmp".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("tmp".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("let mut rows: MoltValue = MoltValue::None;"));
    assert!(source.contains("rows = src.clone();"));
    assert!(source.contains("let mut tmp: MoltValue = rows.clone();"));
    assert!(!source.contains("MOLT_STUB: store_var"));
    assert!(!source.contains("MOLT_STUB: load_var"));
}

#[test]
fn jump_after_loop_does_not_capture_scoped_set_item_temps() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["frame".to_string()],
            ops: vec![
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("answer".to_string()),
                    out: Some("key".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(42),
                    out: Some("val".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_item".to_string(),
                    args: Some(vec![
                        "frame".to_string(),
                        "key".to_string(),
                        "val".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(!source.contains("return molt_get_item(&frame, &key);"));
    assert!(source.contains("return MoltValue::None; /* jump: no prior store */"));
}

#[test]
fn strip_dead_after_return_skips_jump_after_nested_return_until_else() {
    let ops = vec![
        OpIR {
            kind: "if".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "return_none".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "else".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            out: Some("v0".to_string()),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "end_if".to_string(),
            ..OpIR::default()
        },
    ];

    let lowered = strip_dead_after_return(&ops);
    let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["if", "return_none", "else", "const", "end_if"]);
}

#[test]
fn strip_dead_after_return_skips_top_level_jump_after_return() {
    let ops = vec![
        OpIR {
            kind: "return_none".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            out: Some("v0".to_string()),
            value: Some(1),
            ..OpIR::default()
        },
    ];

    let lowered = strip_dead_after_return(&ops);
    let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["return_none"]);
}

/// Fail-closed authority: an op kind that no dispatch arm claims must fail the
/// build through the `unsupported_ops` accumulator recorded at emit time — NOT
/// merely through a text scan for the stub-marker comment. A synthetic unknown
/// kind routes to `emit_op_other` → `emit_unsupported_op`, which records it.
#[test]
fn compile_checked_fails_closed_on_synthetically_unsupported_op() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "molt_synthetic_unsupported_op_probe".to_string(),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "return_none".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("a synthetically-unsupported op must fail the build closed");
    assert!(
        err.contains("refuses to emit fail-open codegen"),
        "error must come from the fail-closed accumulator, got: {err}"
    );
    assert!(
        err.contains("molt_synthetic_unsupported_op_probe"),
        "diagnostic must name the unsupported op kind, got: {err}"
    );
}

/// The fail-closed accumulator is the authority, independent of the emitted
/// text. Even when the catch-all's `out` is a non-assignable sink (so NO
/// `MoltValue::None` value line is emitted), the op is still recorded and the
/// build fails closed. This proves the gate does not rely on scanning for the
/// `/* MOLT_STUB: */ MoltValue::None` string in the output.
#[test]
fn compile_checked_fails_closed_without_emitted_value_marker() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                // No `out` → catch-all emits only a `/* ... */` comment, never a
                // `MoltValue::None` value line — yet the op is still recorded.
                kind: "molt_synthetic_unsupported_sink_probe".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("an unsupported op with no output must still fail closed");
    assert!(err.contains("refuses to emit fail-open codegen"), "got: {err}");
    assert!(err.contains("molt_synthetic_unsupported_sink_probe"), "got: {err}");
}
