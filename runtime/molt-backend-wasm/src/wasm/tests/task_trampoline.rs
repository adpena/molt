use super::support::*;

fn task_marker_ir(marker: &str, target: &str) -> SimpleIR {
    let mut marker_value = wasm_test_op("const_bool", Some("is_task"), vec![]);
    marker_value.value = Some(1);
    let mut closure_size = wasm_test_op("const", Some("closure_size"), vec![]);
    closure_size.value = Some(64);
    let mut function_object = wasm_test_op("func_new", Some("function_object"), vec![]);
    function_object.s_value = Some(target.to_string());
    function_object.value = Some(0);
    let mut set_marker = wasm_test_op(
        "set_attr_generic_obj",
        Some("marker_result"),
        vec!["function_object", "is_task"],
    );
    set_marker.s_value = Some(marker.to_string());
    let mut set_closure_size = wasm_test_op(
        "set_attr_generic_obj",
        Some("closure_result"),
        vec!["function_object", "closure_size"],
    );
    set_closure_size.s_value = Some("__molt_closure_size__".to_string());
    let mut return_function = wasm_test_op("ret", None, vec!["function_object"]);
    return_function.args = Some(vec!["function_object".to_string()]);

    SimpleIR {
        functions: vec![
            wasm_test_function(
                "molt_main",
                vec![],
                None,
                vec![
                    marker_value,
                    closure_size,
                    function_object,
                    set_marker,
                    set_closure_size,
                    return_function,
                ],
            ),
            wasm_test_function(
                target,
                vec![],
                None,
                vec![wasm_test_op("ret_void", None, vec![])],
            ),
        ],
        profile: None,
    }
}

#[test]
fn task_marker_trampolines_emit_task_new_and_valid_wasm() {
    for (marker, target) in [
        ("__molt_is_generator__", "generator_body_poll"),
        ("__molt_is_coroutine__", "coroutine_body_poll"),
        ("__molt_is_async_generator__", "asyncgen_body_poll"),
    ] {
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(task_marker_ir(marker, target));

        wasmparser::Validator::new()
            .validate_all(&wasm)
            .unwrap_or_else(|error| {
                panic!("{marker} task trampoline emitted invalid WASM: {error}")
            });

        let imports = wasm_function_import_indices(&wasm);
        assert!(
            imports.contains_key("task_new"),
            "{marker} must emit a task trampoline that calls task_new; imports={imports:?}"
        );
    }
}
