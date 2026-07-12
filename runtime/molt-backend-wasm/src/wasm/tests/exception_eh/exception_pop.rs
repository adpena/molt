use super::super::*;

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
