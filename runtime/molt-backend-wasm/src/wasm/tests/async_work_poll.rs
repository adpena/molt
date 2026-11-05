use super::support::*;

#[test]
fn pure_poll_can_read_its_execution_record_without_admitting_native_thread_spawn() {
    for row in crate::wasm_abi_generated::IMPORT_REGISTRY {
        if row.name.starts_with("thread_") {
            assert!(
                !WasmProfile::Pure.allows_runtime_import(row.import),
                "native thread operations remain outside the Pure poll profile: {}",
                row.name,
            );
        }
    }
}

fn poll_ir() -> SimpleIR {
    let poll = OpIR {
        value: Some(41),
        ..OpIR::new(molt_ir::OpKind::AsyncWorkPoll)
    };
    let fallthrough = wasm_test_op(molt_ir::OpKind::ConstNone, Some("ok"), vec![]);
    let mut ret = wasm_test_op(molt_ir::OpKind::Ret, None, vec!["ok"]);
    ret.args = Some(vec!["ok".into()]);
    let exit = OpIR {
        value: Some(41),
        ..OpIR::new(molt_ir::OpKind::Label)
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
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
            ],
        )],
        profile: None,
    }
}

fn compile_ir(ir: SimpleIR, native_eh_enabled: bool) -> Vec<u8> {
    let trampoline_analysis = super::super::trampoline_analysis::analyze_wasm_trampolines(&ir);
    WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .emit_wasm_module(
        crate::wasm::final_functions::WasmFinalFunctions::from_codegen_fixtures(ir),
        trampoline_analysis,
        crate::wasm_plan::WasmStageAudit::from_environment(),
    )
    .wasm
}

fn compile_final_poll(native_eh_enabled: bool) -> Vec<u8> {
    compile_ir(poll_ir(), native_eh_enabled)
}

fn finally_pending_observer_ir(async_work_poll: bool) -> SimpleIR {
    let observer = OpIR {
        async_work_poll,
        out: Some("pending".into()),
        ..OpIR::new(molt_ir::OpKind::ExceptionFinallyPendingObserver)
    };
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec![],
            None,
            vec![
                observer,
                wasm_test_op(molt_ir::OpKind::Ret, None, vec!["pending"]),
            ],
        )],
        profile: None,
    }
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
        assert!(
            ops[call_pos - 1].starts_with("If"),
            "service must be conditional: {ops:?}"
        );
        assert_eq!(ops[call_pos + 1], "Else");
        assert!(ops[..call_pos].iter().any(|op| op.starts_with("I32Load")));
        assert!(
            ops[call_pos + 1..]
                .iter()
                .any(|op| op.starts_with("I64Load8U"))
        );
    }
}

#[test]
fn finally_pending_observer_services_only_the_notified_branch_without_exception_transfer() {
    for native_eh_enabled in [false, true] {
        for marked in [false, true] {
            let wasm = compile_ir(finally_pending_observer_ir(marked), native_eh_enabled);
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let imports = wasm_function_import_indices(&wasm);
            let passive = imports["exception_last_pending"];
            let calls = wasm_direct_call_indices_for_export(&wasm, "molt_main");
            assert!(calls.contains(&passive));
            let ops = wasm_operator_debug_for_export(&wasm, "molt_main");
            assert!(
                ops.iter().all(|op| !op.starts_with("BrIf")),
                "finally must reconcile the returned payload on its authored path: {ops:?}"
            );
            if marked {
                let service = imports["async_work_poll_and_exception_last_pending"];
                let index = ops
                    .iter()
                    .position(|op| op == &format!("Call {{ function_index: {service} }}"))
                    .unwrap();
                assert!(ops[index - 2].starts_with("If"));
                assert_eq!(ops[index + 1], "Else");
                let passive_index = ops
                    .iter()
                    .position(|op| op == &format!("Call {{ function_index: {passive} }}"))
                    .unwrap();
                assert!(
                    ops[passive_index - 2].starts_with("If"),
                    "empty exception state must also avoid runtime entry: {ops:?}"
                );
                assert_eq!(calls, [imports["refcount_admission"], service, passive]);
            } else {
                assert!(!imports.contains_key("async_work_poll_and_exception_last_pending"));
                assert_eq!(calls, [imports["refcount_admission"], passive]);
            }
        }
    }
}
