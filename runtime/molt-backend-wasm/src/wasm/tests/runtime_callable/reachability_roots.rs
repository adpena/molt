use super::*;

#[test]
fn unreachable_runtime_callables_are_not_imported() {
    let func = wasm_test_function(
        "no_runtime_callable_roots",
        vec![],
        None,
        vec![wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("tree-shaken runtime callable module must be valid WASM");

    let imports = wasm_function_import_names(&wasm);
    assert!(
        !imports.iter().any(|name| name == "abs_builtin"),
        "unreached builtin runtime callable import leaked into module: {imports:?}"
    );
    assert!(
        !imports.iter().any(|name| name == "gpu_tensor_from_buffer"),
        "unreached GPU intrinsic callable import leaked into module: {imports:?}"
    );
}

#[test]
fn poll_table_runtime_callables_remain_table_roots() {
    let func = wasm_test_function(
        "poll_table_roots",
        vec![],
        None,
        vec![wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("poll-table root module must be valid WASM");

    let imports: BTreeSet<String> = wasm_function_import_names(&wasm).into_iter().collect();
    for spec in POLL_TABLE_IMPORTS {
        let import_name = spec.import.name();
        assert!(
            imports.contains(import_name),
            "poll-table root import {import_name} must remain available for slot {}",
            spec.table_slot
        );
    }
}

#[test]
fn reachable_builtin_runtime_callable_is_imported() {
    let mut abs_builtin = wasm_test_op("builtin_func", Some("fn"), vec![]);
    abs_builtin.s_value = Some("molt_abs_builtin".to_string());
    abs_builtin.value = Some(1);
    let func = wasm_test_function(
        "reachable_builtin_callable",
        vec![],
        None,
        vec![abs_builtin, wasm_test_op("ret_void", None, vec![])],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("reachable runtime callable module must be valid WASM");

    let imports = wasm_function_import_names(&wasm);
    assert!(
        imports.iter().any(|name| name == "abs_builtin"),
        "reached builtin runtime callable import missing from module: {imports:?}"
    );
}
