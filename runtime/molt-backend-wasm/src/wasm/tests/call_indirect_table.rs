use super::*;

#[test]
fn call_indirect_exports_follow_manifest_imports() {
    let func = wasm_test_function(
        "call_indirect_exports",
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
        .expect("call_indirect manifest export module must be structurally valid WASM");

    let exported_call_indirects: BTreeSet<String> = wasm_function_exports(&wasm)
        .into_iter()
        .filter(|name| name.starts_with("molt_call_indirect"))
        .collect();
    let manifest_call_indirects: BTreeSet<String> = CALL_INDIRECT_IMPORTS
        .iter()
        .map(|spec| spec.import_name.to_string())
        .collect();

    assert_eq!(exported_call_indirects, manifest_call_indirects);
    assert_eq!(
        CALL_INDIRECT_MAX_ARITY,
        CALL_INDIRECT_IMPORTS
            .last()
            .expect("generated call_indirect import family must be non-empty")
            .arity
    );
}

#[test]
fn call_indirect_type_layout_and_sentinel_table_slot_are_pinned() {
    let func = wasm_test_function(
        "call_indirect_type_layout",
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
        .expect("call_indirect type-layout module must be structurally valid WASM");

    let import_count = wasm_function_import_indices(&wasm).len() as u32;
    let function_type_indices = wasm_function_section_type_indices(&wasm);
    let export_indices = wasm_function_export_indices(&wasm);
    let signatures = wasm_type_section_signatures(&wasm);

    let first_call_indirect_import = CALL_INDIRECT_IMPORTS
        .first()
        .expect("generated call_indirect import family must be non-empty");
    let first_call_indirect_idx = *export_indices
        .get(first_call_indirect_import.import_name)
        .expect("first call_indirect export must exist");
    for (offset, spec) in CALL_INDIRECT_IMPORTS.iter().enumerate() {
        let func_idx = *export_indices
            .get(spec.import_name)
            .unwrap_or_else(|| panic!("{} export must exist", spec.import_name));
        assert_eq!(
            func_idx,
            first_call_indirect_idx + offset as u32,
            "{} export must stay in generated call_indirect order",
            spec.import_name
        );
        let type_idx = function_type_indices[(func_idx - import_count) as usize];
        assert_eq!(
            signatures[type_idx as usize],
            (spec.arity + 1, 1),
            "{} wrapper must accept table index plus {} args and return one value",
            spec.import_name,
            spec.arity
        );
    }

    let sentinel_func_idx = first_call_indirect_idx + CALL_INDIRECT_IMPORTS.len() as u32;
    let element_indices = wasm_element_function_indices(&wasm);
    let poll_table_prefix = POLL_TABLE_IMPORTS
        .iter()
        .map(|spec| spec.table_slot)
        .max()
        .unwrap_or(0) as usize
        + 1;
    let occupied_poll_slots: BTreeSet<usize> = POLL_TABLE_IMPORTS
        .iter()
        .map(|spec| spec.table_slot as usize)
        .collect();
    for slot in 0..poll_table_prefix {
        if !occupied_poll_slots.contains(&slot) {
            assert_eq!(
                element_indices[slot], sentinel_func_idx,
                "unassigned poll-table slot {slot} must point at the generated sentinel"
            );
        }
    }
}

#[test]
fn reloc_table_ref_exports_do_not_publish_reserved_runtime_sentinels() {
    let func = wasm_test_function(
        "reserved_table_ref_export_filter",
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
        reloc_enabled: true,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("reloc table-ref module must be structurally valid WASM");

    let exports = wasm_function_exports(&wasm);
    let table_ref_slots: Vec<u32> = exports
        .iter()
        .filter_map(|name| {
            name.strip_prefix("__molt_table_ref_")
                .and_then(|raw| raw.parse::<u32>().ok())
                .map(|table_index| {
                    table_index
                        .checked_sub(RELOC_TABLE_BASE_DEFAULT)
                        .unwrap_or_else(|| {
                            panic!(
                                "table-ref export {table_index} is below reloc table base {RELOC_TABLE_BASE_DEFAULT}"
                            )
                        })
                })
        })
        .collect();
    assert!(
        !table_ref_slots.is_empty(),
        "reloc output must still export concrete app table refs"
    );

    let poll_table_prefix = POLL_TABLE_IMPORTS
        .iter()
        .map(|spec| spec.table_slot)
        .max()
        .unwrap_or(0)
        + 1;
    let reserved_callable_start = poll_table_prefix;
    let reserved_trampoline_start = reserved_callable_start + RESERVED_RUNTIME_CALLABLE_COUNT;
    let reserved_trampoline_end = reserved_trampoline_start + RESERVED_RUNTIME_CALLABLE_COUNT;

    for slot in table_ref_slots {
        let in_reserved_callable = slot >= reserved_callable_start
            && slot < reserved_callable_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let in_reserved_trampoline =
            slot >= reserved_trampoline_start && slot < reserved_trampoline_end;
        assert!(
            !in_reserved_callable && !in_reserved_trampoline,
            "reserved runtime callable/trampoline slot {slot} must stay runtime-owned, not exported as an app table ref"
        );
    }
}

#[test]
fn poll_table_slots_follow_manifest_slot_numbers() {
    let func = wasm_test_function(
        "slot_layout",
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
        .expect("poll table layout test module must be structurally valid WASM");

    let import_indices = wasm_function_import_indices(&wasm);
    let element_indices = wasm_element_function_indices(&wasm);
    for import in [
        WasmRuntimeImport::AsyncSleepPoll,
        WasmRuntimeImport::PromisePoll,
        WasmRuntimeImport::ContextlibAsyncExitstackEnterContextPoll,
    ] {
        let import_name = import.name();
        let slot = crate::wasm_abi::poll_table_import_slot(import)
            .unwrap_or_else(|| panic!("missing generated poll slot for {import_name}"));
        let func_index = *import_indices
            .get(import_name)
            .unwrap_or_else(|| panic!("missing poll import {import_name}"));
        assert_eq!(
            element_indices[slot as usize], func_index,
            "poll import {import_name} must occupy manifest table slot {slot}"
        );
    }
}
