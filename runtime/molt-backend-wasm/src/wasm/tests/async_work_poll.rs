use super::support::*;

fn poll_ir() -> SimpleIR {
    let poll = OpIR {
        kind: "async_work_poll".into(),
        value: Some(41),
        ..Default::default()
    };
    let fallthrough = wasm_test_op("const_none", Some("ok"), vec![]);
    let mut ret = wasm_test_op("ret", None, vec!["ok"]);
    ret.args = Some(vec!["ok".into()]);
    let exit = OpIR {
        kind: "label".into(),
        value: Some(41),
        ..Default::default()
    };
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec![],
            None,
            vec![
                poll,
                fallthrough,
                ret,
                exit,
                wasm_test_op("ret_void", None, vec![]),
            ],
        )],
        profile: None,
    }
}

fn compile_final_poll(native_eh_enabled: bool) -> Vec<u8> {
    let ir = poll_ir();
    let trampoline_analysis = super::super::trampoline_analysis::analyze_wasm_trampolines(&ir);
    WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .emit_wasm_module(ir, BTreeMap::new(), trampoline_analysis)
    .wasm
}

#[test]
fn jumpful_and_native_eh_dispatch_call_async_observer_then_branch() {
    for native_eh_enabled in [false, true] {
        let wasm = compile_final_poll(native_eh_enabled);
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        let imports = wasm_function_import_indices(&wasm);
        let async_poll = *imports
            .get("async_work_poll_and_exception_pending")
            .expect("async observer import must survive demand pruning");
        let calls = wasm_direct_call_indices_for_export(&wasm, "molt_main");
        assert!(calls.contains(&async_poll));
        if let Some(pure_exception_pending) = imports.get("exception_pending") {
            assert!(
                !calls.contains(pure_exception_pending),
                "async safepoints must not lower through the pure predicate"
            );
        }

        let ops = wasm_operator_debug_for_export(&wasm, "molt_main");
        let call = format!("Call {{ function_index: {async_poll} }}");
        let call_pos = ops
            .iter()
            .position(|op| op == &call)
            .expect("molt_main must call the async observer");
        assert!(ops[call_pos + 1].starts_with("I64Const { value: 0"));
        assert_eq!(ops[call_pos + 2], "I64Ne");
        assert!(
            ops[call_pos + 3].starts_with("If"),
            "dispatch must conditionally transfer to the function exception exit; ops={ops:?}"
        );
    }
}
