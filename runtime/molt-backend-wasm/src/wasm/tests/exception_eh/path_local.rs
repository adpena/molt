use super::*;

#[test]
fn dispatch_keeps_handler_checks_and_runtime_frames_for_every_eh_request() {
    const PROBE: &str = "path_local_dispatch";
    let labelled = |kind: &str, target: i64| OpIR {
        value: Some(target),
        ..wasm_test_op(kind, None, vec![])
    };
    let func = wasm_test_function(
        PROBE,
        vec!["condition"],
        Some(vec!["bool"]),
        vec![
            wasm_test_op("exception_push", None, vec![]),
            labelled("try_start", 10),
            labelled("check_exception", 10),
            wasm_test_op("if", None, vec!["condition"]),
            labelled("try_end", 10),
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
            wasm_test_op("end_if", None, vec![]),
            labelled("try_end", 10),
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
            labelled("label", 10),
            wasm_test_op("exception_clear", None, vec![]),
            labelled("check_exception", 20),
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
            labelled("label", 20),
            wasm_test_op("exception_pop", None, vec![]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );
    let ir = SimpleIR {
        functions: vec![
            func,
            // Relocatable modules wrap the parameterless application entry.
            // Keep the parameterized probe exported independently so assertions
            // inspect its body, not that table-initialization wrapper.
            wasm_test_function(
                "molt_main",
                vec![],
                None,
                vec![wasm_test_op("ret_void", None, vec![])],
            ),
        ],
        profile: None,
    };
    for (native_eh_enabled, reloc_enabled) in [(false, false), (true, false), (true, true)] {
        let analysis = super::super::super::trampoline_analysis::analyze_wasm_trampolines(&ir);
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled,
            reloc_enabled,
            ..WasmCompileOptions::default()
        })
        .emit_wasm_module(ir.clone(), BTreeMap::new(), analysis)
        .wasm;
        wasmparser::Validator::new()
            .validate_all(&wasm)
            .unwrap_or_else(|error| {
                panic!(
                    "path-local dispatch must remain valid WASM: native_eh={native_eh_enabled}, reloc={reloc_enabled}: {error}"
                )
            });
        let imports = wasm_function_import_indices(&wasm);
        let calls = wasm_direct_call_indices_for_export(&wasm, PROBE);
        let pending = imports["exception_pending"];
        assert_eq!(
            calls.iter().filter(|&&index| index == pending).count(),
            2,
            "an observer inside a handler is a live transfer: native_eh={native_eh_enabled}, reloc={reloc_enabled}, calls={calls:?}"
        );
        assert!(calls.contains(&imports["exception_push"]));
        assert!(calls.contains(&imports["exception_pop"]));
        let operators = wasm_operator_debug_for_export(&wasm, PROBE);
        assert!(
            !operators
                .iter()
                .any(|op| op.starts_with("Throw") || op.starts_with("TryTable")),
            "{operators:?}"
        );
    }
}

#[test]
fn only_plain_non_relocatable_frames_admit_structured_native_eh() {
    use crate::wasm::function_frame::WasmFrameControlMode;
    for mode in [
        WasmFrameControlMode::Plain,
        WasmFrameControlMode::Jumpful,
        WasmFrameControlMode::Stateful,
    ] {
        for requested in [false, true] {
            for relocatable in [false, true] {
                assert_eq!(
                    mode.native_eh_enabled(requested, relocatable),
                    mode == WasmFrameControlMode::Plain && requested && !relocatable
                );
            }
        }
    }
}
