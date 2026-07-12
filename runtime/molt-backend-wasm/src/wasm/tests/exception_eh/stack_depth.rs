use super::super::*;

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
