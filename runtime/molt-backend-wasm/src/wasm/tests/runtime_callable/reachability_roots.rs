use super::*;
use crate::wasm_abi_generated::{PYTHON_BUILTIN_CALLABLES, runtime_callable_import};

fn deferred_lookup_function(function_name: &str, name: Option<&str>, spelling: &str) -> FunctionIR {
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
    wasm_test_function(function_name, params, None, ops)
}

fn compile_lookup_functions(
    mut functions: Vec<FunctionIR>,
    reloc_enabled: bool,
    profile: WasmProfile,
) -> Vec<u8> {
    // The production pipeline eliminates unreachable functions. Make every
    // lookup helper escape through an actual function object, preserving its
    // dynamic parameters while establishing real callable reachability.
    let mut entry_ops = Vec::new();
    let mut callables = Vec::new();
    for (index, function) in functions.iter().enumerate() {
        let name = format!("callable_{index}");
        let mut object = wasm_test_op("func_new", Some(&name), vec![]);
        object.s_value = Some(function.name.clone());
        object.value = Some(function.params.len() as i64);
        entry_ops.push(object);
        callables.push(name);
    }
    entry_ops.push(wasm_test_op(
        "tuple_new",
        Some("callables"),
        callables.iter().map(String::as_str).collect(),
    ));
    entry_ops.push(wasm_test_op("ret", None, vec!["callables"]));
    functions.insert(0, wasm_test_function("molt_main", vec![], None, entry_ops));
    WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled,
        wasm_profile: profile,
        ..WasmCompileOptions::default()
    })
    .compile(SimpleIR {
        functions,
        profile: None,
    })
}

fn deferred_lookup_wasm(
    name: Option<&str>,
    spelling: &str,
    reloc_enabled: bool,
    profile: WasmProfile,
) -> Vec<u8> {
    compile_lookup_functions(
        vec![deferred_lookup_function("lookup", name, spelling)],
        reloc_enabled,
        profile,
    )
}

#[test]
fn deferred_global_lookup_roots_the_generated_builtin_family() {
    for spelling in ["module_get_global", "call"] {
        for reloc_enabled in [false, true] {
            // All signatures in one module per transport/relocation cell. The
            // planner unit test owns per-name minimality; avoid recompiling the
            // entire import/table surface once per generated builtin.
            let functions = PYTHON_BUILTIN_CALLABLES
                .iter()
                .enumerate()
                .map(|(index, spec)| {
                    deferred_lookup_function(
                        &format!("lookup_{index}"),
                        Some(spec.python_name),
                        spelling,
                    )
                })
                .collect();
            let wasm = compile_lookup_functions(functions, reloc_enabled, WasmProfile::Auto);
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let imports = wasm_function_import_names(&wasm);
            let data = wasm_data_segment_payloads(&wasm);
            for spec in PYTHON_BUILTIN_CALLABLES {
                let import = runtime_callable_import(spec.runtime_name).unwrap();
                assert!(
                    imports.iter().any(|name| name == import.name()),
                    "{spelling}: {spec:?}"
                );
                assert!(
                    data.iter()
                        .any(|bytes| bytes == spec.runtime_name.as_bytes()),
                    "missing executable app resolver entry for {spelling}: {spec:?}"
                );
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
