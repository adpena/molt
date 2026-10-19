use wasm_encoder::ValType;

pub(crate) use crate::wasm_abi_generated::{
    CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY, IMPORT_REGISTRY, POLL_TABLE_IMPORTS,
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS, RUNTIME_CALLABLE_IMPORTS,
    ReservedRuntimeCallableDispatch, RuntimeImportSpec, WasmRuntimeImport, poll_table_import_slot,
    runtime_callable_import, wasm_runtime_export_name, wasm_runtime_import,
};
pub(crate) use molt_codegen_abi::{
    GENERATOR_CONTROL_BYTES as GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE,
    TASK_KIND_GENERATOR,
};

pub(crate) const RUNTIME_IMPORT_MODULE: &str = "molt_runtime";
pub(crate) const NATIVE_CALLABLE_IMPORT_MODULE: &str = "molt_native";
/// Canonical linker namespace for user-function declarations.
///
/// FunctionIR externs are ordinary symbols supplied by another WASM object or
/// by the standalone host. Keeping them in `env` matches the wasm-ld undefined
/// symbol convention without conflating them with Molt runtime ABI imports.
pub(crate) const USER_FUNCTION_IMPORT_MODULE: &str = "env";

// ---------------------------------------------------------------------------
// WASM Exception Handling (WASM_OPTIMIZATION_PLAN.md Section 3.6)
//
// Native WASM exception handling replaces the host-imported exception
// mechanism (exception_push/exception_pending/exception_pop) with the
// standardized WASM exception handling instructions (try_table/throw/catch).
//
// The exception tag carries a single i64 payload: the exception object
// handle. Its (i64) -> () signature is interned in the module type space.
//
// Current host-call exception model:
//   try block entry:  call exception_push   (push handler frame)
//   after each call:  call exception_pending (poll for raised exception)
//                     br_if to handler      (branch if pending != 0)
//   try block exit:   call exception_pop    (pop handler frame)
//   raise:            call raise            (set pending + unwind)
//
// Native WASM EH model (target):
//   try block entry:  try_table with catch clause
//   after each call:  (eliminated -- WASM catches automatically)
//   try block exit:   end (implicit)
//   raise:            throw $molt_exception <handle>
//
// Estimated impact: 20-40% speedup for exception-heavy code; 5-10%
// binary size reduction from eliminating exception_pending checks.
//
// Enabled by default; set MOLT_WASM_NATIVE_EH=0 to disable.
// ---------------------------------------------------------------------------

/// Tag index for the molt exception tag (first and only tag in the module).
pub(crate) const TAG_EXCEPTION_INDEX: u32 = 0;

/// Generic direct calls transport i64 locals only. Dedicated emitters own
/// mixed-width runtime ABIs; return ownership never authorizes a width cast.
pub(crate) fn assert_runtime_i64_call(import: WasmRuntimeImport, arg_count: usize) {
    let (params, results) = import.machine_signature();
    assert!(
        params.len() == arg_count
            && params.iter().all(|param| *param == ValType::I64)
            && (results.is_empty() || results == [ValType::I64]),
        "generic direct runtime call {} supplies {arg_count} i64 operands; canonical ABI {:?} -> {:?} requires exact arity and i64-only operands with an i64 or void result",
        import.runtime_export_name(),
        params,
        results
    );
}

// Constant folding pass is now shared via crate::fold_constants in passes.rs.

#[cfg(test)]
mod tests {
    use super::{IMPORT_REGISTRY, WasmRuntimeImport};
    use crate::wasm_abi_generated::STATIC_FUNC_TYPES;
    use crate::wasm_abi_generated::{
        RUNTIME_CALLABLE_IMPORTS, op_loop_runtime_call, runtime_callable_import,
        wasm_runtime_import,
    };
    use wasm_encoder::ValType;

    #[test]
    fn generic_runtime_calls_validate_the_canonical_machine_signature() {
        for (import, arity) in [
            (WasmRuntimeImport::IntFromI64, 1),
            (WasmRuntimeImport::IntAsI64, 1),
            (WasmRuntimeImport::DictSet, 3),
            (WasmRuntimeImport::PrintNewline, 0),
            (WasmRuntimeImport::DecRefObj, 1),
        ] {
            super::assert_runtime_i64_call(import, arity);
            assert!(
                std::panic::catch_unwind(|| super::assert_runtime_i64_call(import, arity + 1))
                    .is_err()
            );
        }
        for (import, arity) in [
            (WasmRuntimeImport::ArenaNew, 0),
            (WasmRuntimeImport::ArenaAllocObject, 2),
            (WasmRuntimeImport::ArenaFree, 1),
            (WasmRuntimeImport::HandleResolve, 1),
            (WasmRuntimeImport::LoadMethod, 5),
        ] {
            assert!(
                std::panic::catch_unwind(|| super::assert_runtime_i64_call(import, arity)).is_err()
            );
        }
    }

    #[test]
    fn import_return_contracts_agree_with_machine_and_shared_boxed_abis() {
        use crate::wasm_abi_generated::WasmRuntimeReturn;
        use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};
        use wasm_encoder::ValType;

        for spec in IMPORT_REGISTRY {
            let (params, results) = spec.import.machine_signature();
            let contract = spec.import.return_contract();
            if results.is_empty() {
                assert_eq!(contract, WasmRuntimeReturn::Void, "{}", spec.name);
            } else if results != [ValType::I64] {
                assert_eq!(contract, WasmRuntimeReturn::RawBits, "{}", spec.name);
            } else {
                assert_ne!(contract, WasmRuntimeReturn::Void, "{}", spec.name);
            }
            if let Some(boxed) = runtime_boxed_abi(spec.import.runtime_export_name(), params.len())
            {
                let expected = match boxed.result {
                    RuntimeBoxedReturn::OwnedValue => WasmRuntimeReturn::OwnedObject,
                    RuntimeBoxedReturn::BorrowedValue => WasmRuntimeReturn::BorrowedObject,
                    RuntimeBoxedReturn::PollValue => WasmRuntimeReturn::PollResult,
                    RuntimeBoxedReturn::Void => WasmRuntimeReturn::Void,
                };
                assert_eq!(contract, expected, "{}", spec.name);
            }
        }
    }

    #[test]
    fn import_return_contract_preserves_provider_ownership_and_protocols() {
        use crate::wasm_abi_generated::WasmRuntimeReturn;
        for (import, expected) in [
            (
                WasmRuntimeImport::IntFromI64,
                WasmRuntimeReturn::OwnedObject,
            ),
            (WasmRuntimeImport::IntAsI64, WasmRuntimeReturn::RawBits),
            (
                WasmRuntimeImport::ExceptionPending,
                WasmRuntimeReturn::RawBits,
            ),
            (
                WasmRuntimeImport::AsyncWorkPollAndExceptionPending,
                WasmRuntimeReturn::RawBits,
            ),
            (
                WasmRuntimeImport::FrameInvocationEnter,
                WasmRuntimeReturn::RawBits,
            ),
            (
                WasmRuntimeImport::FrameInvocationExit,
                WasmRuntimeReturn::BorrowedObject,
            ),
            (
                WasmRuntimeImport::DictSet,
                WasmRuntimeReturn::BorrowedObject,
            ),
            (
                WasmRuntimeImport::StoreIndex,
                WasmRuntimeReturn::BorrowedObject,
            ),
            (
                WasmRuntimeImport::GuardType,
                WasmRuntimeReturn::BorrowedObject,
            ),
            (
                WasmRuntimeImport::AllocClass,
                WasmRuntimeReturn::UnpublishedObject,
            ),
            (
                WasmRuntimeImport::ScratchAlloc,
                WasmRuntimeReturn::ScratchAllocation,
            ),
            (
                WasmRuntimeImport::RuntimeExecutionEnter,
                WasmRuntimeReturn::ExecutionToken,
            ),
            (WasmRuntimeImport::FuturePoll, WasmRuntimeReturn::PollResult),
            (WasmRuntimeImport::IoWait, WasmRuntimeReturn::PollResult),
            (
                WasmRuntimeImport::ChanRecvBlocking,
                WasmRuntimeReturn::PollResult,
            ),
            (WasmRuntimeImport::ChanSend, WasmRuntimeReturn::PollResult),
            (
                WasmRuntimeImport::StreamReaderRead,
                WasmRuntimeReturn::PollResult,
            ),
            (
                WasmRuntimeImport::StreamReaderReadline,
                WasmRuntimeReturn::PollResult,
            ),
            (
                WasmRuntimeImport::StreamSendObj,
                WasmRuntimeReturn::PollResult,
            ),
            (WasmRuntimeImport::WsSendObj, WasmRuntimeReturn::PollResult),
            (
                WasmRuntimeImport::GpuThreadId,
                WasmRuntimeReturn::OwnedObject,
            ),
            (WasmRuntimeImport::PrintNewline, WasmRuntimeReturn::Void),
        ] {
            assert_eq!(import.return_contract(), expected, "{}", import.name());
        }
        for spec in super::POLL_TABLE_IMPORTS {
            assert_eq!(spec.import.return_contract(), WasmRuntimeReturn::PollResult);
        }
        assert!(op_loop_runtime_call("chan_drop", false).is_none());
        assert!(op_loop_runtime_call("alloc_class", false).is_none());
        let owned = op_loop_runtime_call("asyncgen_new", false).expect("owned async generator");
        assert!(
            !owned
                .required_imports
                .contains(&WasmRuntimeImport::DecRefObj)
        );
        for kind in ["guard_tag", "guard_type"] {
            let borrowed = op_loop_runtime_call(kind, false).expect("borrowed guard result");
            assert_eq!(borrowed.import, WasmRuntimeImport::GuardType);
            assert!(
                !borrowed
                    .required_imports
                    .contains(&WasmRuntimeImport::IncRefObj)
            );
        }
    }

    #[test]
    fn module_cache_del_is_registered_as_on_demand_wasm_import() {
        assert!(
            IMPORT_REGISTRY
                .iter()
                .any(|spec| spec.import == WasmRuntimeImport::ModuleCacheDel)
        );
        assert_eq!(
            WasmRuntimeImport::ModuleCacheDel.machine_signature(),
            (&[ValType::I64][..], &[ValType::I64][..]),
            "module_cache_del must use the unary i64 -> i64 host import ABI"
        );

        let op_call =
            op_loop_runtime_call("module_cache_del", false).expect("module_cache_del op-loop call");
        assert_eq!(op_call.import, WasmRuntimeImport::ModuleCacheDel);
        assert_eq!(
            op_call.required_imports,
            [WasmRuntimeImport::ModuleCacheDel],
            "module_cache_del codegen must request its runtime import explicitly"
        );
    }

    #[test]
    fn finally_pending_observer_selects_exactly_one_import_from_the_shared_marker() {
        let plain = op_loop_runtime_call("exception_finally_pending_observer", false)
            .expect("plain finally observer runtime call");
        assert_eq!(plain.import, WasmRuntimeImport::ExceptionLastPending);
        assert_eq!(
            plain.required_imports,
            [WasmRuntimeImport::ExceptionLastPending]
        );

        let marked = op_loop_runtime_call("exception_finally_pending_observer", true)
            .expect("marked finally observer runtime call");
        assert_eq!(
            marked.import,
            WasmRuntimeImport::AsyncWorkPollAndExceptionLastPending
        );
        assert_eq!(
            marked.required_imports,
            [WasmRuntimeImport::AsyncWorkPollAndExceptionLastPending]
        );
    }

    #[test]
    fn target_unavailable_sqlite_is_not_advertised_by_wasm_abi() {
        assert!(wasm_runtime_import("sqlite3_connect").is_none());
        assert!(runtime_callable_import("molt_sqlite3_connect").is_none());
        assert!(
            IMPORT_REGISTRY
                .iter()
                .all(|spec| !spec.name.starts_with("sqlite3_"))
        );
        assert!(
            RUNTIME_CALLABLE_IMPORTS
                .iter()
                .all(|spec| !spec.runtime_name.starts_with("molt_sqlite3_"))
        );
    }

    #[test]
    fn object_new_bound_declares_wasm_imports() {
        assert!(
            IMPORT_REGISTRY
                .iter()
                .any(|spec| spec.import == WasmRuntimeImport::ObjectNewBound)
        );
        assert_eq!(
            WasmRuntimeImport::ObjectNewBound.machine_signature(),
            (&[ValType::I64][..], &[ValType::I64][..]),
            "object_new_bound must use the unary i64 -> i64 host import ABI"
        );

        assert!(
            wasm_runtime_import("object_new_bound_sized").is_none(),
            "heap instance sizing must have one class-owned runtime authority"
        );
    }

    #[test]
    fn import_signature_catalog_is_unique_and_has_no_unproduced_entries() {
        let mut signatures = std::collections::HashSet::new();
        for signature in STATIC_FUNC_TYPES {
            assert!(
                signatures.insert((signature.params, signature.results)),
                "duplicate import signature"
            );
            assert!(
                signature.results.len() <= 1,
                "Python-callable ABI has at most one result"
            );
            assert!(
                IMPORT_REGISTRY
                    .iter()
                    .any(|spec| spec.import.machine_signature()
                        == (signature.params, signature.results)),
                "signature has no import producer"
            );
        }
    }

    #[test]
    fn shared_boxed_text_calls_project_canonical_abi_without_marked_variants() {
        use crate::wasm_abi_generated::{OpLoopRuntimeArgSpec, WasmRuntimeReturn};
        use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_op_abi};
        for receiver in ["string", "bytes", "bytearray"] {
            for method in ["find", "startswith", "endswith", "count"] {
                for suffix in ["", "_slice"] {
                    let kind = format!("{receiver}_{method}{suffix}");
                    let arity = if suffix.is_empty() { 2 } else { 6 };
                    let abi = runtime_boxed_op_abi(&kind).unwrap();
                    assert_eq!(abi.symbol, format!("molt_{kind}"));
                    assert_eq!(abi.arity, arity);
                    assert_eq!(abi.result, RuntimeBoxedReturn::OwnedValue);
                    let plain = op_loop_runtime_call(&kind, false).unwrap();
                    assert_eq!(plain, op_loop_runtime_call(&kind, true).unwrap());
                    assert_eq!(plain.import.runtime_export_name(), abi.symbol);
                    assert_eq!(
                        plain.import.return_contract(),
                        WasmRuntimeReturn::OwnedObject
                    );
                    super::assert_runtime_i64_call(plain.import, arity);
                    assert_eq!(
                        plain.args,
                        (0..arity)
                            .map(OpLoopRuntimeArgSpec::Local)
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(plain.required_imports, &[plain.import]);
                    assert!(!plain.discard_result);
                }
            }
        }
    }
}
