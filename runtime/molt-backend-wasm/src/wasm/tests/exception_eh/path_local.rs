use super::*;

#[test]
fn dispatch_keeps_handler_checks_and_runtime_frames_for_every_eh_request() {
    const PROBE: &str = "path_local_dispatch";
    let labelled = |kind: molt_ir::OpKind, target: i64| OpIR {
        value: Some(target),
        ..wasm_test_op(kind, None, vec![])
    };
    for (native_eh_enabled, reloc_enabled) in [(false, false), (true, false), (true, true)] {
        let func = wasm_test_function(
            PROBE,
            vec!["condition"],
            Some(vec!["bool"]),
            vec![
                wasm_test_op(molt_ir::OpKind::ExceptionPush, None, vec![]),
                labelled(molt_ir::OpKind::TryStart, 10),
                labelled(molt_ir::OpKind::CheckException, 10),
                wasm_test_op(molt_ir::OpKind::If, None, vec!["condition"]),
                labelled(molt_ir::OpKind::TryEnd, 10),
                wasm_test_op(molt_ir::OpKind::ExceptionPop, None, vec![]),
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
                wasm_test_op(molt_ir::OpKind::EndIf, None, vec![]),
                labelled(molt_ir::OpKind::TryEnd, 10),
                wasm_test_op(molt_ir::OpKind::ExceptionPop, None, vec![]),
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
                labelled(molt_ir::OpKind::Label, 10),
                wasm_test_op(molt_ir::OpKind::ExceptionClear, None, vec![]),
                labelled(molt_ir::OpKind::CheckException, 20),
                wasm_test_op(molt_ir::OpKind::ExceptionPop, None, vec![]),
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
                labelled(molt_ir::OpKind::Label, 20),
                wasm_test_op(molt_ir::OpKind::ExceptionPop, None, vec![]),
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
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
                    vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
                ),
            ],
            profile: None,
        };
        let analysis = super::super::super::trampoline_analysis::analyze_wasm_trampolines(&ir);
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled,
            reloc_enabled,
            ..WasmCompileOptions::default()
        })
        .emit_wasm_module(
            crate::wasm::final_functions::WasmFinalFunctions::from_codegen_fixtures(ir),
            analysis,
            crate::wasm_plan::WasmStageAudit::from_environment(),
        )
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
            !operators.iter().any(|op| op.starts_with("Throw {")),
            "{operators:?}"
        );
        assert_eq!(
            operators
                .iter()
                .filter(|op| op.starts_with("TryTable"))
                .count(),
            if native_eh_enabled && !reloc_enabled {
                2
            } else {
                0
            },
            "dispatch uses only the Python/foreign ABI boundaries, never authored-handler EH: {operators:?}"
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
