use super::*;

#[test]
fn shared_drop_fact_marker_set_is_explicit_for_wasm() {
    assert!(is_shared_drop_fact_marker("drop_inserted"));
    assert!(is_shared_drop_fact_marker(
        "exception_region_drops_inserted"
    ));
    assert!(!is_shared_drop_fact_marker("inc_ref"));
    assert!(!is_shared_drop_fact_marker("dec_ref"));
    assert!(!is_shared_drop_fact_marker("release"));
}

#[test]
fn generic_wasm_exception_pop_then_drop_keeps_dec_ref_import_across_eh_modes() {
    let mut owned = wasm_test_op("const_str", Some("v0"), vec![]);
    owned.s_value = Some("owned".to_string());
    let func = wasm_test_function(
        "exception_drop",
        vec![],
        None,
        vec![
            wasm_test_op("exception_region_drops_inserted", None, vec![]),
            owned,
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("dec_ref", None, vec!["v0"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    for (native_eh_enabled, expect_exception_pop) in [(true, false), (false, true)] {
        let options = WasmCompileOptions {
            native_eh_enabled,
            reloc_enabled: false,
            ..WasmCompileOptions::default()
        };
        let wasm = WasmBackend::with_options(options).compile(ir.clone());
        let imports = wasm_function_import_names(&wasm);
        assert_eq!(
            imports.iter().any(|name| name == "exception_pop"),
            expect_exception_pop,
            "generic WASM exception_pop import mismatch for native_eh_enabled={native_eh_enabled}; imports={imports:?}"
        );
        assert!(
            imports.iter().any(|name| name == "dec_ref_obj"),
            "generic WASM shared drops must keep dec_ref_obj import for native_eh_enabled={native_eh_enabled}; imports={imports:?}"
        );
    }
}

#[test]
fn wasm_compiles_exception_stack_depth_bookkeeping_family() {
    // Every function with try/with handlers — including the always-present
    // module-globals scaffold — emits the runtime exception-handler-stack depth
    // bookkeeping family (enter/depth/set_depth/exit). Before these handlers
    // existed, WASM codegen panicked in emit_control_op on the very first op
    // (`exception_stack_enter`) of `m____molt_globals_builtin__`, so the backend
    // could not compile ANY program. This compiles the full family and asserts
    // each op lowers to its `molt_exception_stack_*` runtime import with the ABI
    // signature shared with the native backend (no-arg enter/depth -> i64;
    // one-arg exit/set_depth -> i64).
    let func = wasm_test_function(
        "exc_stack_family",
        vec![],
        None,
        vec![
            wasm_test_op("exception_stack_enter", Some("prev"), vec![]),
            wasm_test_op("exception_stack_depth", Some("depth"), vec![]),
            wasm_test_op("exception_stack_set_depth", Some("none"), vec!["depth"]),
            wasm_test_op("exception_stack_exit", Some("none"), vec!["prev"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    // Structural validation catches both the historical codegen panic and any
    // operand-stack imbalance (e.g. a missing Drop on a void-returning op).
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("exception-stack bookkeeping family must compile to structurally valid WASM");

    let import_types = wasm_function_import_type_indices(&wasm);
    let sigs = wasm_type_section_signatures(&wasm);
    for (name, expected_sig) in [
        ("exception_stack_enter", (0usize, 1usize)),
        ("exception_stack_depth", (0, 1)),
        ("exception_stack_exit", (1, 1)),
        ("exception_stack_set_depth", (1, 1)),
    ] {
        let type_idx = *import_types.get(name).unwrap_or_else(|| {
            panic!("{name} runtime import must be registered; imports={import_types:?}")
        });
        assert_eq!(
            sigs[type_idx as usize], expected_sig,
            "{name} import ABI signature mismatch (params, results)"
        );
    }
}
