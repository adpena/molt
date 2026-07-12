use super::*;

#[test]
fn import_transaction_callable_wrapper_matches_runtime_import_abi() {
    let mut import_transaction = wasm_test_op("builtin_func", Some("fn"), vec![]);
    import_transaction.s_value = Some("molt_importlib_import_transaction".to_string());
    import_transaction.value = Some(5);
    let func = wasm_test_function(
        "import_transaction_callable",
        vec![],
        None,
        vec![import_transaction, wasm_test_op("ret_void", None, vec![])],
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
        .expect("import transaction wrapper must be structurally valid WASM");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("importlib_import_transaction")
        .expect("import transaction runtime import must be registered");
    assert_eq!(
        sigs[import_type as usize],
        (5, 1),
        "importlib_import_transaction import ABI must consume the five values emitted by its callable wrapper"
    );
}

#[test]
fn void_runtime_callable_wrapper_uses_manifest_result_type() {
    let mut socket_drop = wasm_test_op("builtin_func", Some("fn"), vec![]);
    socket_drop.s_value = Some("molt_socket_drop".to_string());
    socket_drop.value = Some(1);
    let func = wasm_test_function(
        "socket_drop_callable",
        vec![],
        None,
        vec![socket_drop, wasm_test_op("ret_void", None, vec![])],
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
        .expect("void callable wrapper must synthesize None after the runtime import call");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("socket_drop")
        .expect("socket_drop runtime import must be registered");
    assert_eq!(
        sigs[import_type as usize],
        (1, 0),
        "socket_drop import ABI must be manifest void, not locally defaulted to i64"
    );
}

#[test]
fn intrinsic_runtime_callables_are_manifest_backed() {
    let mut load_intrinsic = wasm_test_op("builtin_func", Some("fn"), vec![]);
    load_intrinsic.s_value = Some("molt_load_intrinsic_runtime".to_string());
    load_intrinsic.value = Some(2);
    let func = wasm_test_function(
        "load_intrinsic_callable",
        vec![],
        None,
        vec![load_intrinsic, wasm_test_op("ret_void", None, vec![])],
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
        .expect("intrinsic resolver callable must compile through generated ABI metadata");

    let imports = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    let import_type = *imports
        .get("load_intrinsic_runtime")
        .expect("load_intrinsic_runtime import must be manifest-backed");
    assert_eq!(sigs[import_type as usize], (2, 1));
}

#[test]
fn gpu_context_runtime_ops_are_manifest_backed() {
    let mut ret = wasm_test_op("ret", None, vec!["tid"]);
    ret.var = Some("tid".to_string());
    let func = wasm_test_function(
        "gpu_context_runtime_ops",
        vec![],
        None,
        vec![
            wasm_test_op("gpu_thread_id", Some("tid"), vec![]),
            wasm_test_op("gpu_block_id", Some("bid"), vec![]),
            wasm_test_op("gpu_block_dim", Some("bdim"), vec![]),
            wasm_test_op("gpu_grid_dim", Some("gdim"), vec![]),
            wasm_test_op("gpu_barrier", Some("barrier"), vec![]),
            ret,
        ],
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
        .expect("GPU context runtime ops must compile through generated ABI metadata");

    let import_types = wasm_function_import_type_indices(&wasm);
    let import_indices = wasm_function_import_indices(&wasm);
    let call_indices = wasm_direct_call_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    for import_name in [
        "gpu_thread_id",
        "gpu_block_id",
        "gpu_block_dim",
        "gpu_grid_dim",
        "gpu_barrier",
    ] {
        let import_type = *import_types
            .get(import_name)
            .unwrap_or_else(|| panic!("{import_name} import must be manifest-backed"));
        assert_eq!(
            sigs[import_type as usize],
            (0, 1),
            "{import_name} must use the manifest [] -> i64 ABI"
        );
        let import_index = *import_indices
            .get(import_name)
            .unwrap_or_else(|| panic!("{import_name} import must stay live"));
        assert!(
            call_indices.contains(&import_index),
            "{import_name} must be emitted as a direct runtime call"
        );
    }
}
