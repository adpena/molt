use super::support::*;

#[test]
fn call_indirect_exports_follow_manifest_imports() {
    let func = wasm_test_function(
        "call_indirect_exports",
        vec![],
        None,
        vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
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
}

#[test]
fn call_indirect_type_layout_and_sentinel_table_slot_are_pinned() {
    let func = wasm_test_function(
        "call_indirect_type_layout",
        vec![],
        None,
        vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
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
    let value_signatures = wasm_type_section_value_signatures(&wasm);
    assert_eq!(
        value_signatures.iter().collect::<BTreeSet<_>>().len(),
        value_signatures.len(),
        "runtime imports, users and indirect-call wrappers must share function types"
    );

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
            (spec.arity + 2, 1),
            "{} wrapper must accept table index plus {} args and return one value",
            spec.import_name,
            spec.arity
        );
    }
}

#[test]
fn reloc_objects_publish_no_numeric_callable_aliases_or_table_writers() {
    let func = wasm_test_function(
        "molt_main",
        vec![],
        None,
        vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
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
    assert!(
        exports
            .iter()
            .all(|name| !name.starts_with("__molt_table_ref_")),
        "reloc output must not publish numeric callable-table aliases"
    );
    assert!(!exports.contains("molt_table_init"));
    assert!(!wasm_has_table_set(&wasm));
}

#[test]
fn runtime_polls_are_keys_the_app_neither_imports_nor_tables() {
    // HF-699 K1: a runtime poll is its key on every target. The app imports
    // no poll function and its table holds none, whatever the bound
    // runtime's features (HF-487).
    let absent = [
        WasmRuntimeImport::AsyncioSockRecvPoll,
        WasmRuntimeImport::IoWait,
        WasmRuntimeImport::WsWait,
    ];
    let slot_of = |import: WasmRuntimeImport| {
        crate::wasm_abi::poll_table_import_slot(import)
            .unwrap_or_else(|| panic!("missing generated poll key offset for {}", import.name()))
    };
    let reduced = absent.iter().fold(
        crate::wasm_options::WasmRuntimePollSlots::all().mask(),
        |mask, &import| mask & !(1u64 << slot_of(import)),
    );
    for mask in [
        crate::wasm_options::WasmRuntimePollSlots::all().mask(),
        reduced,
    ] {
        let runtime_poll_slots = crate::wasm_options::WasmRuntimePollSlots::from_mask(mask)
            .expect("a subset of the generated poll key offsets");
        for reloc_enabled in [false, true] {
            let ir = SimpleIR {
                // A relocatable object needs the `molt_main` entry wrapper.
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec![],
                    None,
                    vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
                )],
                profile: None,
            };
            let wasm = WasmBackend::with_options(WasmCompileOptions {
                native_eh_enabled: false,
                reloc_enabled,
                runtime_poll_slots,
                ..WasmCompileOptions::default()
            })
            .compile(ir);
            wasmparser::Validator::new()
                .validate_all(&wasm)
                .expect("poll key module must be structurally valid WASM");
            let import_indices = wasm_function_import_indices(&wasm);
            for spec in POLL_TABLE_IMPORTS {
                assert!(
                    !import_indices.contains_key(spec.import.name()),
                    "reloc={reloc_enabled}: runtime poll {} is a key, not an app import",
                    spec.import.name()
                );
            }
        }
    }
}

#[test]
fn runtime_poll_slot_masks_outside_the_generated_table_are_refused() {
    use crate::wasm_options::WasmRuntimePollSlots;
    let all = WasmRuntimePollSlots::all().mask();
    assert_eq!(all & 1, 0, "key offset 0 names no runtime poll");
    assert!(WasmRuntimePollSlots::from_mask(all).is_ok());
    assert!(WasmRuntimePollSlots::from_mask(0).is_ok());
    let error = WasmRuntimePollSlots::from_mask(all | 1).expect_err("slot 0 is refused");
    assert!(
        error.contains("outside the generated poll table"),
        "{error}"
    );
}
