use super::support::*;

#[test]
fn object_new_bound_import_demand_uses_class_owned_layout_size() {
    let cases = [
        (
            "object_new_bound_unsized_selector",
            "object_new_bound",
            None,
            "object_new_bound",
            "object_new_bound_sized",
        ),
        (
            "object_new_bound_with_static_layout_hint",
            "object_new_bound",
            Some(24),
            "object_new_bound",
            "object_new_bound_sized",
        ),
    ];
    for (name, kind, payload_size, expected_import, rejected_import) in cases {
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_object_new_bound_ir(kind, payload_size));
        let reloc_imports = wasm_function_import_names(&reloc_wasm);
        assert!(
            reloc_imports.iter().any(|name| name == expected_import),
            "{expected_import} must be selected by Auto+reloc import demand; imports={reloc_imports:?}"
        );

        let direct_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_object_new_bound_ir(kind, payload_size));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        let operators = wasm_operator_debug_for_export(&direct_wasm, "molt_main");
        let expected_index = *import_indices
            .get(expected_import)
            .unwrap_or_else(|| panic!("{expected_import} import must exist"));
        assert!(
            call_indices.contains(&expected_index),
            "{name} must directly call {expected_import} from molt_main; calls={call_indices:?}"
        );
        let call = format!("Call {{ function_index: {expected_index} }}");
        let call_pos = operators
            .iter()
            .position(|operator| operator == &call)
            .unwrap_or_else(|| panic!("{name} missing {call}; operators={operators:?}"));
        assert!(
            call_pos > 0 && operators[call_pos - 1].starts_with("LocalGet {"),
            "{name} must pass only class bits to unary {expected_import}; operators={operators:?}"
        );
        if let Some(rejected_index) = import_indices.get(rejected_import) {
            assert!(
                !call_indices.contains(rejected_index),
                "{name} must not directly call {rejected_import} from molt_main; calls={call_indices:?}"
            );
        }
    }
}

#[test]
fn method_load_import_demand_and_codegen_select_the_call_arity() {
    const CALLS: [&str; 6] = [
        "call_loaded_method0",
        "call_loaded_method1",
        "call_loaded_method2",
        "call_loaded_method3",
        "call_loaded_method4",
        "call_loaded_method_spilled",
    ];
    for (load_kind, extra_arg_count, expected_call) in [
        ("load_method", 0, "call_loaded_method0"),
        ("load_method", 2, "call_loaded_method2"),
        ("load_method", 4, "call_loaded_method4"),
        ("load_method", 5, "call_loaded_method_spilled"),
        ("load_super_method", 0, "call_loaded_method0"),
        ("load_super_method", 2, "call_loaded_method2"),
        ("load_super_method", 5, "call_loaded_method_spilled"),
    ] {
        let case = format!("{load_kind}/{extra_arg_count}");
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_loaded_method_ir(load_kind, extra_arg_count));
        let reloc_imports = wasm_function_import_names(&reloc_wasm);
        for import_name in [load_kind, expected_call] {
            assert!(
                reloc_imports.iter().any(|name| name == import_name),
                "{case} must retain {import_name}; imports={reloc_imports:?}"
            );
        }
        for import_name in CALLS {
            if import_name != expected_call {
                assert!(
                    !reloc_imports.iter().any(|name| name == import_name),
                    "{case} must not retain unselected import {import_name}; imports={reloc_imports:?}"
                );
            }
        }

        let direct_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_loaded_method_ir(load_kind, extra_arg_count));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        for import_name in [load_kind, expected_call] {
            let index = *import_indices
                .get(import_name)
                .unwrap_or_else(|| panic!("{case}: {import_name} import must exist"));
            assert!(
                call_indices.contains(&index),
                "{case} must directly call {import_name}; calls={call_indices:?}"
            );
        }
        assert!(
            wasm_data_segment_payloads(&direct_wasm)
                .iter()
                .any(|payload| payload.as_slice() == b"selected_method"),
            "{case} must materialize the method name"
        );
    }
}

/// A method load reads its scratch word before it releases a discarded
/// callable: the release can run `__del__`, whose compiled code may reuse the
/// word (HF-216).
#[test]
fn method_load_reads_its_out_word_before_releasing_a_discarded_callable() {
    let load = OpIR {
        kind: "load_method".to_string(),
        var: Some("unused_callable".to_string()),
        out: Some("unused_self".to_string()),
        args: Some(vec!["receiver".to_string()]),
        s_value: Some("selected_method".to_string()),
        source_op_idx: Some(0),
        ..OpIR::default()
    };
    let ir = SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec!["receiver"],
            None,
            vec![
                load,
                wasm_test_op("const_none", Some("nothing"), vec![]),
                wasm_test_op("ret", None, vec!["nothing"]),
            ],
        )],
        profile: None,
    };
    let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir, Default::default());
    wasmparser::Validator::new()
        .validate_all(&output.wasm)
        .unwrap();
    let imports = wasm_function_import_indices(&output.wasm);
    let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
    let call = |name: &str| format!("Call {{ function_index: {} }}", imports[name]);
    let load_position = operators
        .iter()
        .position(|operator| operator == &call("load_method"))
        .unwrap_or_else(|| panic!("missing the load call; operators={operators:?}"));
    let after_load = &operators[load_position + 1..];
    let word_read = after_load
        .iter()
        .position(|operator| operator.starts_with("I64Load {"))
        .unwrap_or_else(|| panic!("the scratch word is never read; operators={operators:?}"));
    let release = after_load
        .iter()
        .position(|operator| operator == &call("dec_ref_obj"))
        .unwrap_or_else(|| panic!("the discarded words are not released; operators={operators:?}"));
    assert!(
        word_read < release,
        "the scratch word must be read before the first release; operators={operators:?}"
    );
}

/// An unboxed iterator step is one `molt_iter_next_unboxed` call whose value
/// comes back through the scratch word (HF-217): no `(value, done)` tuple
/// from `iter_next` and no `index` calls to take it apart.
#[test]
fn iterator_step_calls_iter_next_unboxed_with_an_out_word() {
    let mut next = wasm_test_op("iter_next_unboxed", Some("done"), vec!["iterator"]);
    next.var = Some("value".into());
    let ir = SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec!["iterator"],
            None,
            vec![next, wasm_test_op("ret", None, vec!["value"])],
        )],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    let import_indices = wasm_function_import_indices(&wasm);
    let calls = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    let step = *import_indices
        .get("iter_next_unboxed")
        .expect("iter_next_unboxed import must exist");
    assert!(calls.contains(&step), "calls={calls:?}");
    for retired in ["iter_next", "index"] {
        if let Some(index) = import_indices.get(retired) {
            assert!(
                !calls.contains(index),
                "the step must not call {retired}; calls={calls:?}"
            );
        }
    }
    let operators = wasm_operator_debug_for_export(&wasm, "molt_main");
    let call = format!("Call {{ function_index: {step} }}");
    let call_pos = operators
        .iter()
        .position(|operator| operator == &call)
        .unwrap_or_else(|| panic!("missing {call}; operators={operators:?}"));
    assert!(
        operators[call_pos - 1].starts_with("I64Const {"),
        "the second argument is the scratch word's address; operators={operators:?}"
    );
    assert!(
        operators[call_pos + 1..]
            .iter()
            .any(|operator| operator.starts_with("I64Load {")),
        "the value is loaded from the scratch word; operators={operators:?}"
    );
}

#[test]
fn explicit_trace_ops_call_their_runtime_imports() {
    let op = |kind: &str| OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    };
    let mut enter = op("trace_enter_slot");
    enter.s_value = Some("molt_test_func".into());
    let bindings: molt_ir::code_slots::CodeSlotBindings =
        serde_json::from_value(serde_json::json!({"molt_test_func": 0})).unwrap();
    let wasm = WasmBackend::new()
        .with_code_slots(bindings)
        .compile(SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_test_func".into(),
                return_abi: molt_ir::FunctionReturnAbi::Void,
                execution_context: molt_ir::ExecutionContextPolicy::Local,
                ops: vec![enter, op("trace_exit"), op("ret_void")],
                ..FunctionIR::default()
            }],
            profile: None,
        });
    let imports = wasm_function_import_indices(&wasm);
    let calls = wasm_direct_call_indices(&wasm);
    for name in ["trace_enter_slot", "trace_exit"] {
        let index = imports
            .get(name)
            .unwrap_or_else(|| panic!("{name} must be imported; imports={imports:?}"));
        assert!(
            calls.contains(index),
            "the {name} op must call its runtime import; calls={calls:?}"
        );
    }
}
