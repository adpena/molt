use super::*;

#[test]
#[should_panic(expected = "builtin runtime callable arity mismatch")]
fn builtin_callable_observed_arity_must_match_manifest() {
    let mut import_transaction = wasm_test_op("builtin_func", Some("fn"), vec![]);
    import_transaction.s_value = Some("molt_importlib_import_transaction".to_string());
    import_transaction.value = Some(4);
    let func = wasm_test_function(
        "stale_callable_arity",
        vec![],
        None,
        vec![import_transaction, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
}

#[test]
#[should_panic(expected = "direct runtime call missing WASM ABI manifest import")]
fn direct_molt_runtime_call_without_manifest_import_fails_closed() {
    let mut call = wasm_test_op("call", Some("out"), vec!["arg"]);
    call.s_value = Some("molt_unregistered_runtime_probe".to_string());
    let func = wasm_test_function(
        "unknown_direct_runtime_call",
        vec!["arg"],
        None,
        vec![call, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
}
