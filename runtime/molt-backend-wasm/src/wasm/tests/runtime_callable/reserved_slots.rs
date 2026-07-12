use super::*;

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
