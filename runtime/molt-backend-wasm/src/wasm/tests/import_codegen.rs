use super::support::*;

#[test]
fn object_new_bound_import_demand_uses_class_owned_layout_size() {
    let cases = [
        (
            "object_new_bound_unsized_selector",
            None,
            "object_new_bound",
            "object_new_bound_sized",
        ),
        (
            "object_new_bound_with_static_stack_hint",
            Some(24),
            "object_new_bound",
            "object_new_bound_sized",
        ),
    ];
    for (name, payload_size, expected_import, rejected_import) in cases {
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_object_new_bound_ir(payload_size));
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
        .compile(wasm_object_new_bound_ir(payload_size));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        let expected_index = *import_indices
            .get(expected_import)
            .unwrap_or_else(|| panic!("{expected_import} import must exist"));
        assert!(
            call_indices.contains(&expected_index),
            "{name} must directly call {expected_import} from molt_main; calls={call_indices:?}"
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
fn method_ic_import_demand_and_codegen_follow_generated_arity_selector() {
    let cases = [
        (
            "call_method_ic",
            0,
            "call_method_ic0",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_method_ic",
            2,
            "call_method_ic2",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_method_ic",
            5,
            "call_method_ic4",
            [
                "call_method_ic0",
                "call_method_ic1",
                "call_method_ic2",
                "call_method_ic3",
                "call_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            0,
            "call_super_method_ic0",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            2,
            "call_super_method_ic2",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
        (
            "call_super_method_ic",
            5,
            "call_super_method_ic4",
            [
                "call_super_method_ic0",
                "call_super_method_ic1",
                "call_super_method_ic2",
                "call_super_method_ic3",
                "call_super_method_ic4",
            ],
        ),
    ];
    for (kind, extra_arg_count, expected_import, family_imports) in cases {
        let reloc_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: true,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_method_ic_ir(kind, extra_arg_count));
        let reloc_imports = wasm_function_import_names(&reloc_wasm);
        assert!(
            reloc_imports.iter().any(|name| name == expected_import),
            "{kind}/{extra_arg_count} must retain selected import {expected_import}; imports={reloc_imports:?}"
        );
        for import_name in family_imports {
            if import_name != expected_import {
                assert!(
                    !reloc_imports.iter().any(|name| name == import_name),
                    "{kind}/{extra_arg_count} must not retain unselected import {import_name}; imports={reloc_imports:?}"
                );
            }
        }

        let direct_wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(wasm_method_ic_ir(kind, extra_arg_count));
        let import_indices = wasm_function_import_indices(&direct_wasm);
        let call_indices = wasm_direct_call_indices_for_export(&direct_wasm, "molt_main");
        let expected_index = *import_indices
            .get(expected_import)
            .unwrap_or_else(|| panic!("{expected_import} import must exist"));
        assert!(
            call_indices.contains(&expected_index),
            "{kind}/{extra_arg_count} must directly call {expected_import}; calls={call_indices:?}"
        );
        for import_name in family_imports {
            if let Some(unselected_index) = import_indices.get(import_name)
                && import_name != expected_import
            {
                assert!(
                    !call_indices.contains(unselected_index),
                    "{kind}/{extra_arg_count} must not directly call {import_name}; calls={call_indices:?}"
                );
            }
        }
        assert!(
            wasm_data_segment_payloads(&direct_wasm)
                .iter()
                .any(|payload| payload.as_slice() == b"selected_method"),
            "{kind}/{extra_arg_count} must materialize the selected method name"
        );
    }
}
