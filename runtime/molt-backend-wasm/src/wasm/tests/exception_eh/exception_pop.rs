use super::*;

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
fn generic_wasm_del_boundary_lowers_through_shared_dec_ref_authority() {
    let mut owned = wasm_test_op("const_str", Some("v0"), vec![]);
    owned.s_value = Some("owned".to_string());
    let ir = SimpleIR {
        functions: vec![wasm_test_function(
            "del_boundary_drop",
            vec![],
            None,
            vec![
                owned,
                wasm_test_op("del_boundary", None, vec!["v0"]),
                wasm_test_op("ret_void", None, vec![]),
            ],
        )],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    let imports = wasm_function_import_names(&wasm);
    assert!(
        imports.iter().any(|name| name == "dec_ref_obj"),
        "generic WASM must lower DelBoundary through dec_ref_obj; imports={imports:?}"
    );
}

fn compile_local_alias_body(ops: Vec<OpIR>) -> (Vec<u32>, BTreeMap<String, u32>) {
    let ir = SimpleIR {
        functions: vec![wasm_test_function("molt_main", vec![], None, ops)],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    (
        wasm_direct_call_indices_for_export(&wasm, "molt_main"),
        wasm_function_import_indices(&wasm),
    )
}

#[test]
fn generic_wasm_local_alias_retain_policy_follows_function_rc_authority() {
    let mut load_one = wasm_test_op("load_var", Some("v1"), vec![]);
    load_one.var = Some("slot".to_string());
    let mut load_two = wasm_test_op("load_var", Some("v2"), vec![]);
    load_two.var = Some("slot".to_string());
    let mut owned = wasm_test_op("const_str", Some("owned"), vec![]);
    owned.s_value = Some("owned".to_string());
    let mut store = wasm_test_op("store_var", None, vec!["owned"]);
    store.var = Some("slot".to_string());
    let (drop_calls, drop_imports) = compile_local_alias_body(vec![
        owned,
        store,
        load_one.clone(),
        load_two,
        wasm_test_op("const_none", Some("none"), vec![]),
        wasm_test_op("is", Some("result"), vec!["v1", "none"]),
        wasm_test_op("del_boundary", None, vec!["v2"]),
        wasm_test_op("ret", None, vec!["result"]),
    ]);
    let dec_index = drop_imports["dec_ref_obj"];
    let inc_count = drop_imports.get("inc_ref_obj").map_or(0, |inc_index| {
        drop_calls
            .iter()
            .filter(|call_index| **call_index == *inc_index)
            .count()
    });
    let dec_count = drop_calls
        .iter()
        .filter(|call_index| **call_index == dec_index)
        .count();
    assert_eq!(
        (inc_count, dec_count),
        (0, 2),
        "transparent load aliases share the source ownership root; terminal drops release the temporary value and slot owner without backend-minted references: calls={drop_calls:?} imports={drop_imports:?}"
    );

    let (binding_calls, binding_imports) = compile_local_alias_body(vec![
        wasm_test_op("binding_alias", Some("owned"), vec!["slot"]),
        wasm_test_op("ret_void", None, vec![]),
    ]);
    let inc_index = binding_imports["inc_ref_obj"];
    assert_eq!(
        binding_calls
            .iter()
            .filter(|call_index| **call_index == inc_index)
            .count(),
        1,
        "binding_alias remains an explicitly owned alias: calls={binding_calls:?}"
    );
}
