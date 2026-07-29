use super::support::*;

fn unpack_ir(function_name: &str, expected: usize) -> SimpleIR {
    let mut args = vec!["seq".to_string()];
    for index in 0..expected {
        args.push(format!("out{index}"));
    }
    let unpack = OpIR {
        kind: "unpack_sequence".to_string(),
        args: Some(args),
        value: Some(expected as i64),
        ..OpIR::default()
    };
    let mut ret = wasm_test_op("ret", None, vec![]);
    if expected != 0 {
        ret.args = Some(vec!["out0".to_string()]);
    }
    SimpleIR {
        functions: vec![wasm_test_function(
            function_name,
            vec!["seq"],
            None,
            vec![unpack, ret],
        )],
        profile: None,
    }
}

fn assert_transactional_unpack_calls(wasm: &[u8], export: &str, expected: usize) {
    wasmparser::Validator::new()
        .validate_all(wasm)
        .expect("transactional unpack lowering must emit valid WASM");
    let imports = wasm_function_import_indices(wasm);
    let calls = wasm_direct_call_indices_for_export(wasm, export);
    let unpack = imports["unpack_sequence"];
    assert!(calls.contains(&unpack));
    if expected == 0 {
        assert!(!imports.contains_key("scratch_alloc"));
        assert!(!imports.contains_key("scratch_free"));
        return;
    }
    assert!(calls.contains(&imports["scratch_alloc"]));
    assert!(calls.contains(&imports["scratch_free"]));
    if let Some(index) = imports.get("index") {
        assert!(
            !calls.contains(index),
            "unpack must not regress to prefix indexing"
        );
    }
}

#[test]
fn generic_unpack_uses_transactional_runtime_for_nonzero_and_zero_arity() {
    for expected in [0, 2] {
        let name = format!("unpack_generic_{expected}");
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            ..WasmCompileOptions::default()
        })
        .compile(unpack_ir(&name, expected));
        assert_transactional_unpack_calls(&wasm, &name, expected);
    }
}

#[test]
fn lir_fast_unpack_uses_the_same_transactional_runtime_authority() {
    let name = "m____molt_globals_builtin__unpack_exact";
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(unpack_ir(name, 2));
    assert_transactional_unpack_calls(&wasm, name, 2);
}

#[test]
#[should_panic(expected = "UnpackSequence")]
fn malformed_simple_ir_unpack_is_rejected_before_codegen() {
    let mut ir = unpack_ir("malformed_unpack", 2);
    ir.functions[0].ops[0].value = Some(1);
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
}
