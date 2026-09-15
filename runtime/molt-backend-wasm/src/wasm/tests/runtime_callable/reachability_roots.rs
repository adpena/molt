use super::*;
use crate::wasm_abi_generated::{PYTHON_BUILTIN_CALLABLES, runtime_callable_import};

fn deferred_lookup_wasm(
    name: Option<&str>,
    spelling: &str,
    reloc_enabled: bool,
    profile: WasmProfile,
) -> Vec<u8> {
    let mut ops = Vec::new();
    let mut params = vec!["module"];
    if let Some(name) = name {
        let mut constant = wasm_test_op("const_str", Some("name"), vec![]);
        constant.s_value = Some(name.to_string());
        ops.push(constant);
    } else {
        params.push("name");
    }
    let mut lookup = wasm_test_op(spelling, Some("value"), vec!["module", "name"]);
    match spelling {
        "call" => lookup.s_value = Some("molt_module_get_global".to_string()),
        "module_get_global" => {}
        _ => panic!("unknown fixture lookup spelling {spelling}"),
    }
    ops.extend([lookup, wasm_test_op("ret", None, vec!["value"])]);
    WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled,
        wasm_profile: profile,
        ..WasmCompileOptions::default()
    })
    .compile(SimpleIR {
        functions: vec![wasm_test_function("deferred_lookup", params, None, ops)],
        profile: None,
    })
}

#[test]
fn deferred_global_lookup_roots_the_generated_builtin_family() {
    for spelling in ["module_get_global", "call"] {
        for reloc_enabled in [false, true] {
            for spec in PYTHON_BUILTIN_CALLABLES {
                let wasm = deferred_lookup_wasm(
                    Some(spec.python_name),
                    spelling,
                    reloc_enabled,
                    WasmProfile::Auto,
                );
                wasmparser::Validator::new().validate_all(&wasm).unwrap();
                let imports = wasm_function_import_names(&wasm);
                let import = runtime_callable_import(spec.runtime_name).unwrap();
                assert!(
                    imports.iter().any(|name| name == import.name()),
                    "{spelling}: {spec:?}"
                );
                let data = wasm_data_segment_payloads(&wasm);
                assert!(
                    data.iter()
                        .any(|bytes| bytes == spec.runtime_name.as_bytes()),
                    "missing executable app resolver entry for {spelling}: {spec:?}"
                );
                // No unrelated family-wide roots for a known source name.
                for other in PYTHON_BUILTIN_CALLABLES {
                    if other.runtime_name != spec.runtime_name {
                        assert!(
                            !data
                                .iter()
                                .any(|bytes| bytes == other.runtime_name.as_bytes()),
                            "unreached resolver entry leaked: {other:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn computed_global_lookup_conservatively_roots_supported_python_builtins() {
    let wasm = deferred_lookup_wasm(None, "module_get_global", false, WasmProfile::Auto);
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    let data = wasm_data_segment_payloads(&wasm);
    for spec in PYTHON_BUILTIN_CALLABLES {
        assert!(
            data.iter()
                .any(|bytes| bytes == spec.runtime_name.as_bytes()),
            "{spec:?}"
        );
    }
}

#[test]
fn unrelated_global_names_do_not_root_python_builtins() {
    let wasm = deferred_lookup_wasm(
        Some("application_global"),
        "module_get_global",
        false,
        WasmProfile::Pure,
    );
    let data = wasm_data_segment_payloads(&wasm);
    for spec in PYTHON_BUILTIN_CALLABLES {
        assert!(
            !data
                .iter()
                .any(|bytes| bytes == spec.runtime_name.as_bytes()),
            "{spec:?}"
        );
    }
}

#[test]
fn supported_deferred_builtin_lookup_is_valid_in_pure_profile() {
    let wasm = deferred_lookup_wasm(Some("print"), "module_get_global", false, WasmProfile::Pure);
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    assert!(
        wasm_data_segment_payloads(&wasm)
            .iter()
            .any(|bytes| bytes == b"molt_print_builtin")
    );
}

#[test]
#[should_panic(expected = "WASM pure profile cannot admit reachable runtime import 'open_builtin'")]
fn unavailable_deferred_builtin_is_rejected_before_import_table_emission() {
    deferred_lookup_wasm(Some("open"), "module_get_global", false, WasmProfile::Pure);
}

#[test]
#[should_panic(expected = "WASM pure profile cannot admit reachable runtime import")]
fn unproven_dynamic_builtin_closure_is_rejected_for_pure_profile() {
    deferred_lookup_wasm(None, "module_get_global", false, WasmProfile::Pure);
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
