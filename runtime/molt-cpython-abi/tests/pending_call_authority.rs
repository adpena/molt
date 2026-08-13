use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("molt-cpython-abi must live under <repo>/runtime")
        .to_path_buf()
}

#[test]
fn headers_route_pending_calls_to_the_exported_abi_authority() {
    let root = repo_root();
    let source_header = fs::read_to_string(root.join("include/molt/Python.h")).unwrap();
    let linked_header =
        fs::read_to_string(root.join("runtime/molt-cpython-abi/include/Python.h")).unwrap();

    for header in [&source_header, &linked_header] {
        assert!(header.contains("int Py_AddPendingCall(int (*func)(void *), void *arg);"));
        assert!(header.contains("int Py_MakePendingCalls(void);"));
    }
    assert!(!source_header.contains("static inline int Py_AddPendingCall"));
    assert!(!source_header.contains("static inline int Py_MakePendingCalls"));
    assert!(!source_header.contains("call immediately"));
}

#[test]
fn runtime_registers_one_canonical_pending_call_authority() {
    let root = repo_root();
    let authority =
        fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/pending_calls.rs")).unwrap();
    let runtime_init =
        fs::read_to_string(root.join("runtime/molt-runtime/src/state/runtime_state.rs")).unwrap();

    for export in ["fn Py_AddPendingCall", "fn Py_MakePendingCalls"] {
        assert!(
            authority.contains(export),
            "missing real ABI export {export}"
        );
    }
    assert!(authority.contains("PendingCallQueue<PENDING_CALL_CAPACITY>"));
    assert!(authority.contains("compare_exchange_weak"));
    assert!(authority.contains(".store(Self::advance(pos, 1), Ordering::Release)"));
    assert!(authority.contains(".store(Self::advance(pos, N), Ordering::Release)"));
    assert!(!authority.contains("Mutex<VecDeque"));
    assert!(!authority.contains("VecDeque"));
    assert!(
        authority
            .contains("#[cfg(not(feature = \"runtime-test-support\"))]\nuse std::sync::OnceLock;")
    );
    assert!(
        authority.contains("#[cfg(feature = \"runtime-test-support\")]\nuse std::sync::Mutex;")
    );
    assert_eq!(
        authority.matches("static MAIN_THREAD:").count(),
        2,
        "production and runtime-test-support must each declare one cfg-exclusive owner store"
    );

    let lifecycle_win = runtime_init
        .find("RuntimeLifecyclePhase::Uninitialized =>")
        .expect("lifecycle winner arm");
    let main_registration = runtime_init
        .find("pending_calls::register_main_thread(owner)")
        .expect("main-thread registration");
    assert!(
        main_registration > lifecycle_win,
        "main identity must be published only by the winning init transaction"
    );
    assert!(authority.contains("AttachedMainRuntimeContext"));
    assert!(authority.contains("finish_pending_calls_before_teardown"));
    assert!(authority.contains("PendingCallErrorKind::RuntimeContextDetached"));
    assert!(authority.contains("transfer_runtime_pending_to_current"));
    assert!(authority.contains("finish_c_boundary"));
    assert!(authority.contains("finish_runtime_boundary"));
    assert!(authority.contains("make_pending_calls_at_runtime_safepoint"));
    assert!(authority.contains("PendingCallAdmission"));
    assert!(authority.contains("epoch: AtomicUsize::new(0)"));
    assert!(authority.contains("close_and_quiesce"));
    assert!(authority.contains("discard_pending_calls"));
    assert!(hooks_source_sets_typed_pending_error(&root));
    assert!(runtime_init.contains("attach_runtime_execution_thread()"));
    assert!(runtime_init.contains("detach_runtime_execution_thread()"));
}

fn hooks_source_sets_typed_pending_error(root: &Path) -> bool {
    let authority =
        fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/pending_calls.rs")).unwrap();
    let runtime_hooks =
        fs::read_to_string(root.join("runtime/molt-runtime/src/cpython_abi_hooks.rs")).unwrap();
    let lifecycle =
        fs::read_to_string(root.join("runtime/molt-runtime/src/state/lifecycle.rs")).unwrap();
    authority.contains("pending_call_error")
        && authority.contains("PyExc_SystemError")
        && runtime_hooks.contains("\"SystemError\"")
        && lifecycle.contains("run_unraisable")
        && lifecycle.contains("finish_pending_calls_before_teardown")
}

#[test]
fn generated_eval_breaker_is_distinct_from_pure_exception_observation() {
    let root = repo_root();
    let exception_abi = fs::read_to_string(
        root.join("runtime/molt-runtime/src/builtins/exceptions/exception_state_abi.rs"),
    )
    .unwrap();
    let op_kinds = fs::read_to_string(root.join("runtime/molt-ir/src/tir/op_kinds.toml")).unwrap();
    let placement =
        fs::read_to_string(root.join("runtime/molt-passes/src/tir/passes/async_work_poll.rs"))
            .unwrap();
    let tir_ops = fs::read_to_string(root.join("runtime/molt-ir/src/tir/ops.rs")).unwrap();
    let target_info =
        fs::read_to_string(root.join("runtime/molt-ir/src/tir/target_info.rs")).unwrap();
    let pass_manager =
        fs::read_to_string(root.join("runtime/molt-passes/src/tir/pass_manager.rs")).unwrap();
    let native = fs::read_to_string(root.join(
        "runtime/molt-backend-native/src/native_backend/function_compiler/fc/exception_control.rs",
    ))
    .unwrap();
    let llvm = fs::read_to_string(
        root.join("runtime/molt-backend-native/src/llvm_backend/lowering/op_dispatch.rs"),
    )
    .unwrap();
    let luau = fs::read_to_string(root.join("runtime/molt-backend-luau/src/luau/op_exceptions.rs"))
        .unwrap();
    let rust =
        fs::read_to_string(root.join("runtime/molt-backend-rust/src/rust/op_emitter/gaps.rs"))
            .unwrap();
    let mlir =
        fs::read_to_string(root.join("runtime/molt-backend-mlir/src/tir_to_mlir/ops.rs")).unwrap();
    let wasm = fs::read_to_string(
        root.join("runtime/molt-backend-wasm/src/wasm/op_loop/control_ops/exceptions.rs"),
    )
    .unwrap();

    let pure_start = exception_abi.find("fn molt_exception_pending()").unwrap();
    let async_start = exception_abi
        .find("fn molt_async_work_poll_and_exception_pending()")
        .unwrap();
    assert!(pure_start < async_start);
    assert!(
        !exception_abi[pure_start..async_start].contains("make_pending_calls_at_runtime_safepoint"),
        "pure exception predicates must never re-enter callbacks"
    );
    assert!(exception_abi[async_start..].contains("make_pending_calls_at_runtime_safepoint"));
    assert!(exception_abi[async_start..].contains("drain_failed || exception_pending"));

    assert!(op_kinds.contains("async_work_poll_after_opcodes"));
    assert!(op_kinds.contains("aliases = [\"async_work_poll\"]"));
    assert!(placement.contains("opcode_requires_async_work_poll_after_table"));
    assert!(placement.contains("LoopForest"));
    assert!(placement.contains("op.mark_async_work_poll()"));
    assert!(tir_ops.contains("pub fn is_async_work_poll(&self)"));
    assert!(tir_ops.contains("pub fn mark_async_work_poll(&mut self)"));
    assert!(target_info.contains("supports_pending_call_eval_breaker_poll"));
    assert!(pass_manager.contains("tti.supports_pending_call_eval_breaker_poll()"));

    for backend in [&native, &llvm, &wasm] {
        assert!(
            backend.contains("molt_async_work_poll_and_exception_pending")
                || backend.contains("AsyncWorkPollAndExceptionPending")
        );
    }
    for unsupported_backend in [&luau, &rust, &mlir] {
        assert!(unsupported_backend.contains("async_work_poll"));
        assert!(
            unsupported_backend
                .contains("canonical pending-call/eval-breaker runtime boundary is unavailable")
        );
    }
    assert!(mlir.contains("op.is_async_work_poll()"));
}

#[test]
fn lifecycle_and_target_projection_form_one_attached_main_capability() {
    let root = repo_root();
    let authority =
        fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/pending_calls.rs")).unwrap();
    let hooks = fs::read_to_string(root.join("runtime/molt-cpython-abi/src/hooks.rs")).unwrap();
    let runtime_hooks =
        fs::read_to_string(root.join("runtime/molt-runtime/src/cpython_abi_hooks.rs")).unwrap();
    assert!(authority.contains("AttachedMainRuntimeContext::current()"));
    assert!(hooks.contains("NativeFreeThreaded"));
    assert!(hooks.contains("WasmSingleThread"));
    assert!(runtime_hooks.contains("_PyThreadState_UncheckedGet()"));
    assert!(!runtime_hooks.contains(
        "runtime_is_initialized() {\n        AttachedRuntimeContextKind::NativeFreeThreaded"
    ));
}
