use super::*;

#[test]
fn runtime_import_aliases_follow_manifest_runtime_names() {
    assert_eq!(
        wasm_runtime_import("importlib_import_transaction"),
        Some(WasmRuntimeImport::ImportlibImportTransaction)
    );
    assert_eq!(
        wasm_runtime_import("molt_importlib_import_transaction"),
        Some(WasmRuntimeImport::ImportlibImportTransaction)
    );
    assert_eq!(
        wasm_runtime_import("socket_drop"),
        Some(WasmRuntimeImport::SocketDrop)
    );
    assert_eq!(
        wasm_runtime_import("molt_socket_drop"),
        Some(WasmRuntimeImport::SocketDrop)
    );
    assert_eq!(
        wasm_runtime_import("runtime_init"),
        Some(WasmRuntimeImport::RuntimeInit)
    );
    assert_eq!(
        wasm_runtime_import("molt_runtime_init"),
        Some(WasmRuntimeImport::RuntimeInit)
    );
    assert_eq!(
        wasm_runtime_import("runtime_shutdown"),
        Some(WasmRuntimeImport::RuntimeShutdown)
    );
    assert_eq!(
        wasm_runtime_import("molt_runtime_shutdown"),
        Some(WasmRuntimeImport::RuntimeShutdown)
    );
    assert_eq!(
        WasmRuntimeImport::ImportlibImportTransaction.runtime_export_name(),
        "molt_importlib_import_transaction"
    );
    assert_eq!(
        WasmRuntimeImport::RuntimeInit.runtime_export_name(),
        "molt_runtime_init"
    );
    assert_eq!(
        wasm_runtime_export_name("importlib_import_transaction"),
        Some("molt_importlib_import_transaction")
    );
    assert_eq!(
        wasm_runtime_export_name("molt_importlib_import_transaction"),
        Some("molt_importlib_import_transaction")
    );
    assert_eq!(
        wasm_runtime_export_name("socket_drop"),
        Some("molt_socket_drop")
    );
    assert_eq!(
        wasm_runtime_export_name("molt_runtime_init"),
        Some("molt_runtime_init")
    );
}

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
fn reserved_runtime_callable_function_objects_own_linked_table_slots() {
    let mut import_transaction = wasm_test_op("builtin_func", Some("fn"), vec![]);
    import_transaction.s_value = Some("molt_importlib_import_transaction".to_string());
    import_transaction.value = Some(5);
    let func = wasm_test_function(
        "molt_main",
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
        reloc_enabled: true,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("reserved runtime callable table ownership must emit valid WASM");

    let import_transaction_spec = RESERVED_RUNTIME_CALLABLE_SPECS
        .iter()
        .find(|spec| spec.runtime_name == "molt_importlib_import_transaction")
        .expect("import transaction must stay in reserved runtime callable manifest");
    assert_eq!(
        import_transaction_spec.dispatch,
        ReservedRuntimeCallableDispatch::Trampoline
    );

    let first_call_indirect_import = CALL_INDIRECT_IMPORTS
        .first()
        .expect("generated call_indirect import family must be non-empty");
    let export_indices = wasm_function_export_indices(&wasm);
    let first_call_indirect_idx = *export_indices
        .get(first_call_indirect_import.import_name)
        .expect("first call_indirect export must exist");
    let sentinel_func_idx = first_call_indirect_idx + CALL_INDIRECT_IMPORTS.len() as u32;

    let poll_table_prefix = POLL_TABLE_IMPORTS
        .iter()
        .map(|spec| spec.table_slot)
        .max()
        .unwrap_or(0)
        + 1;
    let reserved_callable_start = poll_table_prefix;
    let reserved_trampoline_start = reserved_callable_start + RESERVED_RUNTIME_CALLABLE_COUNT;
    let direct_table_index =
        RELOC_TABLE_BASE_DEFAULT + reserved_callable_start + import_transaction_spec.index;
    let trampoline_table_index =
        RELOC_TABLE_BASE_DEFAULT + reserved_trampoline_start + import_transaction_spec.index;

    let table_refs = wasm_table_set_refs_for_export(&wasm, "molt_table_init");
    assert_eq!(
        table_refs.get(&(direct_table_index as i32)),
        Some(&sentinel_func_idx),
        "trampoline-only reserved direct slot must be reset to the sentinel, not left for native extension element segments"
    );
    let trampoline_func_idx = *table_refs
        .get(&(trampoline_table_index as i32))
        .expect("reachable reserved runtime trampoline slot must be initialized");
    assert_ne!(
        trampoline_func_idx, sentinel_func_idx,
        "reachable reserved runtime trampoline slot must not remain sentinel-backed"
    );

    let imports = wasm_function_import_indices(&wasm);
    let import_idx = *imports
        .get("importlib_import_transaction")
        .expect("import transaction runtime import must be registered");
    assert_ne!(
        trampoline_func_idx, import_idx,
        "reserved trampoline slot must point at a generated argv trampoline, not the raw five-arg import"
    );

    let import_count = imports.len() as u32;
    let function_type_indices = wasm_function_section_type_indices(&wasm);
    let signatures = wasm_type_section_signatures(&wasm);
    let trampoline_type_idx = function_type_indices[(trampoline_func_idx - import_count) as usize];
    assert_eq!(
        signatures[trampoline_type_idx as usize],
        (3, 1),
        "reserved runtime trampoline must consume closure, argv, argc and return one value"
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
