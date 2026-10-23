//! Exercise production finalization without supplying custody in the fixture.

use crate::concurrency::execution::{
    RuntimeExecutionGuard, current_thread_has_c_extension_execution_context,
    current_thread_holds_shutdown_drain_custody,
};
use crate::state::runtime_state::{
    active_runtime_execution_lease_count, current_thread_holds_runtime_execution_lease,
    molt_runtime_init, molt_runtime_shutdown,
};
use crate::{MoltObject, PyToken, dec_ref_bits, obj_from_bits};
use std::sync::Mutex;

static CALLBACKS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn observe_callback(stage: &'static str) {
    // Check BEFORE public entry: native raw-GIL nesting otherwise hides the
    // missing finalization capability that WASM correctly rejects.
    assert!(current_thread_has_c_extension_execution_context());
    let leases = active_runtime_execution_lease_count();
    let inherited = current_thread_holds_runtime_execution_lease();
    assert!(inherited || current_thread_holds_shutdown_drain_custody());
    assert!(
        obj_from_bits(crate::molt_getrecursionlimit())
            .as_int()
            .unwrap()
            > 0
    );
    assert_eq!(active_runtime_execution_lease_count(), leases);
    assert_eq!(current_thread_holds_runtime_execution_lease(), inherited);
    CALLBACKS.lock().unwrap().push(stage);
}

unsafe extern "C" fn pending_callback(_arg: *mut std::ffi::c_void) -> std::ffi::c_int {
    observe_callback("pending");
    0
}

extern "C" fn atexit_callback() -> u64 {
    observe_callback("atexit");
    MoltObject::none().bits()
}

extern "C" fn flush_callback(_self: u64) -> u64 {
    observe_callback("flush");
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(mode) = std::env::var_os("MOLT_SHUTDOWN_CUSTODY_TEST_CHILD") {
        assert_eq!(
            *CALLBACKS.lock().unwrap(),
            ["cycle", "pending", "atexit", "flush"]
        );
        println!(
            "shutdown callbacks verified before process exit: {}",
            mode.to_str().expect("ASCII child mode")
        );
    }
    MoltObject::none().bits()
}

extern "C" fn cycle_callback(_self: u64) -> u64 {
    observe_callback("cycle");
    MoltObject::none().bits()
}

extern "C" fn module_cycle_callback(_self: u64) -> u64 {
    observe_callback("module-cycle");
    crate::with_gil_entry_nopanic!(py, {
        // Cycles released by ordinary module retirement must finalize before
        // the real sys namespace advances to its own retirement phase.
        let sys = crate::runtime_state(py)
            .interpreter_sys
            .module(py)
            .expect("module-cycle finalizer lost the live sys namespace");
        let stdout = crate::attr_name_bits_from_bytes(py, b"stdout").unwrap();
        let stream = crate::molt_get_attr_name(sys, stdout);
        assert!(obj_from_bits(stream).as_ptr().is_some());
        dec_ref_bits(py, stream);
        dec_ref_bits(py, stdout);
        assert!(!crate::exception_pending(py));
        MoltObject::none().bits()
    })
}

fn detach_finalizer_field(py: &PyToken<'_>, receiver: u64) {
    // Resolve the published method exactly as object.__setattr__ in a Python
    // finalizer does. Calling molt_object_setattr directly would bypass the
    // namespace whose premature retirement this control must detect.
    let name = crate::attr_name_bits_from_bytes(py, b"__setattr__").unwrap();
    let setter = crate::molt_get_attr_name(crate::builtin_classes(py).object, name);
    dec_ref_bits(py, name);
    assert!(
        !crate::exception_pending(py),
        "object.__setattr__ retired before finalizers"
    );
    assert!(obj_from_bits(setter).as_ptr().is_some());
    let field = crate::attr_name_bits_from_bytes(py, b"handle").unwrap();
    let result =
        unsafe { crate::call_callable3(py, setter, receiver, field, MoltObject::none().bits()) };
    dec_ref_bits(py, setter);
    dec_ref_bits(py, result);
    assert!(!crate::exception_pending(py));
    let observed = crate::molt_get_attr_name(receiver, field);
    dec_ref_bits(py, field);
    assert!(!crate::exception_pending(py));
    assert!(obj_from_bits(observed).is_none());
    dec_ref_bits(py, observed);
}

extern "C" fn tls_dictionary_callback(receiver: u64) -> u64 {
    observe_callback("tls-dictionary");
    crate::with_gil_entry_nopanic!(py, {
        detach_finalizer_field(py, receiver);
        MoltObject::none().bits()
    })
}

fn publish_tls_dictionary_callback(py: &PyToken<'_>) {
    use molt_cpython_abi::api::{mapping, refcount, sys};
    let value = instance_with_method(
        py,
        b"ShutdownThreadDictionaryOwner",
        b"__del__",
        tls_dictionary_callback as *const (),
    );
    unsafe {
        let view = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value),
        );
        assert!(!view.as_ptr().is_null());
        let dictionary = sys::PyThreadState_GetDict();
        assert!(!dictionary.is_null());
        assert_eq!(
            mapping::PyDict_SetItemString(dictionary, c"late_finalizer".as_ptr(), view.as_ptr()),
            0,
        );
    }
    dec_ref_bits(py, value);
    assert!(!crate::exception_pending(py));
}

extern "C" fn heap_namespace_callback(receiver: u64) -> u64 {
    observe_callback("heap-namespace");
    crate::with_gil_entry_nopanic!(py, {
        detach_finalizer_field(py, receiver);
        // The heap-namespace phase must repeat the existing two-domain drain,
        // not retire static methods immediately after clearing that namespace.
        publish_tls_dictionary_callback(py);
        MoltObject::none().bits()
    })
}

fn runtime_function(py: &PyToken<'_>, address: *const (), arity: u64) -> u64 {
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(address),
        arity,
    );
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn instance_with_method(py: &PyToken<'_>, name: &[u8], method: &[u8], address: *const ()) -> u64 {
    let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base(class, crate::builtin_classes(py).object);
    let class_ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
    let key = crate::attr_name_bits_from_bytes(py, method).unwrap();
    let function = runtime_function(py, address, 1);
    crate::molt_set_attr_name(class, key, function);
    dec_ref_bits(py, key);
    dec_ref_bits(py, function);
    let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
    assert!(obj_from_bits(instance).as_ptr().is_some());
    dec_ref_bits(py, class);
    instance
}

fn prepare_native_type_cycles(py: &PyToken<'_>) -> [usize; 2] {
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_DEFAULT, PyObject, PyType_Slot, PyType_Spec};
    use molt_cpython_abi::api::{refcount::OwnedPyObject, typeobj};

    unsafe {
        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: std::ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"shutdown.UnrootedType".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let unrooted = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&mut spec));
        assert!(!unrooted.as_ptr().is_null());
        let unrooted_address = unrooted.as_ptr().addr();
        drop(unrooted);
        // A completed heap type normally owns its MRO self-cycle. Merely
        // dropping its sole caller must leave that cycle for production GC.
        assert!(crate::object::gc::native_gc_is_enrolled(unrooted_address));

        spec.name = c"shutdown.ModuleRootedType".as_ptr();
        let rooted = OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&mut spec));
        assert!(!rooted.as_ptr().is_null());
        let rooted_address = rooted.as_ptr().addr();
        let rooted_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(rooted.as_ptr())
            .unwrap();
        let name = crate::attr_name_bits_from_bytes(py, b"shutdown_cycle_owner").unwrap();
        let owner_ptr = crate::alloc_module_obj(py, name);
        dec_ref_bits(py, name);
        assert!(!owner_ptr.is_null());
        let owner = MoltObject::from_ptr(owner_ptr).bits();
        let key = crate::attr_name_bits_from_bytes(py, b"native_type").unwrap();
        crate::builtins::modules::molt_module_set_attr(owner, key, rooted_bits);
        dec_ref_bits(py, key);
        dec_ref_bits(py, rooted_bits);
        drop(rooted);

        let cycle = instance_with_method(
            py,
            b"ModuleShutdownCycle",
            b"__del__",
            module_cycle_callback as *const (),
        );
        let edge = crate::attr_name_bits_from_bytes(py, b"edge").unwrap();
        crate::molt_set_attr_name(cycle, edge, cycle);
        dec_ref_bits(py, edge);
        let key = crate::attr_name_bits_from_bytes(py, b"cycle").unwrap();
        crate::builtins::modules::molt_module_set_attr(owner, key, cycle);
        dec_ref_bits(py, key);
        dec_ref_bits(py, cycle);
        // Transfer the actual module owner into the production cache. Clearing
        // its namespace is what makes both cycles unreachable during teardown.
        assert!(
            crate::runtime_state(py)
                .module_cache
                .lock()
                .unwrap()
                .insert("shutdown_cycle_owner".into(), owner)
                .is_none()
        );
        assert!(crate::object::gc::native_gc_is_enrolled(rooted_address));
        publish_tls_dictionary_callback(py);
        let heap_value = instance_with_method(
            py,
            b"ShutdownHeapNamespaceOwner",
            b"__del__",
            heap_namespace_callback as *const (),
        );
        let key = crate::attr_name_bits_from_bytes(py, b"late_finalizer").unwrap();
        // ExceptionGroup is a real mutable canonical heap type in CPython,
        // unlike an artificial test class injected into the runtime root list.
        crate::molt_set_attr_name(crate::builtin_classes(py).exception_group, key, heap_value);
        dec_ref_bits(py, key);
        dec_ref_bits(py, heap_value);
        assert!(!crate::exception_pending(py));
        [unrooted_address, rooted_address]
    }
}

fn prepare_callbacks(with_cycle: bool, with_native_cycles: bool) -> [usize; 2] {
    assert_eq!(molt_runtime_init(), 1);
    CALLBACKS.lock().unwrap().clear();
    let mut native_cycles = [0; 2];
    {
        let execution = RuntimeExecutionGuard::enter();
        let py = execution.token();
        crate::runtime_state(&py).gc.set_enabled(false);
        let callback = runtime_function(&py, atexit_callback as *const (), 0);
        // The registry owns its reference; the result carries none.
        assert!(
            obj_from_bits(crate::builtins::atexit::molt_atexit_register(
                callback,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            ))
            .is_none()
        );
        dec_ref_bits(&py, callback);

        let stream = instance_with_method(
            &py,
            b"ShutdownStream",
            b"flush",
            flush_callback as *const (),
        );
        let sys_name = crate::attr_name_bits_from_bytes(&py, b"sys").unwrap();
        let sys = crate::builtins::modules::molt_module_new(sys_name);
        crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
        let stdout = crate::attr_name_bits_from_bytes(&py, b"stdout").unwrap();
        crate::builtins::modules::molt_module_set_attr(sys, stdout, stream);
        // Keep the bootstrap file-handle sibling live: teardown must traverse
        // both direct molt_file_flush and custom flush lookup/call branches.
        let stderr_name = crate::attr_name_bits_from_bytes(&py, b"stderr").unwrap();
        let stderr = crate::molt_get_attr_name(sys, stderr_name);
        assert_eq!(
            unsafe { crate::object_type_id(obj_from_bits(stderr).as_ptr().unwrap()) },
            crate::TYPE_ID_FILE_HANDLE,
        );
        dec_ref_bits(&py, stderr);
        dec_ref_bits(&py, stderr_name);
        for bits in [stdout, stream, sys, sys_name] {
            dec_ref_bits(&py, bits);
        }
        if with_cycle {
            let cycle = instance_with_method(
                &py,
                b"ShutdownCycle",
                b"__del__",
                cycle_callback as *const (),
            );
            let edge = crate::attr_name_bits_from_bytes(&py, b"edge").unwrap();
            crate::molt_set_attr_name(cycle, edge, cycle);
            dec_ref_bits(&py, edge);
            dec_ref_bits(&py, cycle);
        }
        if with_native_cycles {
            native_cycles = prepare_native_type_cycles(&py);
        }
        assert!(!crate::exception_pending(&py));
    }
    assert!(!current_thread_holds_runtime_execution_lease());
    assert_eq!(active_runtime_execution_lease_count(), 0);
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                Some(pending_callback),
                std::ptr::null_mut(),
            )
        },
        0
    );
    assert!(CALLBACKS.lock().unwrap().is_empty());
    native_cycles
}

#[test]
#[ignore = "mutates the process-global runtime lifecycle; run with serialized lifecycle shard"]
fn embedding_shutdown_covers_first_pending_callback_atexit_and_live_stdio() {
    crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        prepare_callbacks(false, false);
        assert_eq!(molt_runtime_shutdown(), 1);
        assert_eq!(*CALLBACKS.lock().unwrap(), ["pending", "atexit", "flush"]);
        assert_eq!(active_runtime_execution_lease_count(), 0);
        assert!(!current_thread_has_c_extension_execution_context());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0
        );
        assert_eq!(molt_runtime_init(), 0);
        assert_eq!(molt_runtime_shutdown(), 0);
    });
}

#[cfg(not(feature = "free-threaded"))]
#[test]
#[ignore = "mutates the process-global runtime lifecycle; run with serialized lifecycle shard"]
fn embedding_shutdown_collects_native_type_cycles_across_module_retirement() {
    crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        let native_cycles = prepare_callbacks(true, true);
        assert!(native_cycles.iter().all(|&address| address != 0));
        // No test-side clear/collection is allowed: production embedding
        // shutdown must retire both native allocations before registry reset.
        assert_eq!(molt_runtime_shutdown(), 1);
        assert_eq!(
            *CALLBACKS.lock().unwrap(),
            [
                "cycle",
                "pending",
                "atexit",
                "flush",
                "module-cycle",
                "tls-dictionary",
                "heap-namespace",
                "tls-dictionary",
            ]
        );
        for address in native_cycles {
            assert!(!crate::object::gc::native_gc_is_enrolled(address));
        }
        assert_eq!(active_runtime_execution_lease_count(), 0);
        assert!(!current_thread_has_c_extension_execution_context());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0
        );
        assert_eq!(molt_runtime_init(), 0);
        assert_eq!(molt_runtime_shutdown(), 0);
    });
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "free-threaded")))]
#[test]
#[ignore = "spawns serialized process-exit lifecycle children"]
fn process_exit_covers_collection_before_pending_callbacks_with_or_without_lease() {
    const CHILD: &str = "MOLT_SHUTDOWN_CUSTODY_TEST_CHILD";
    if let Some(mode) = std::env::var_os(CHILD) {
        assert!(mode == "lease" || mode == "no-lease", "unknown child mode");
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            prepare_callbacks(true, false);
            let _execution = (mode == "lease").then(RuntimeExecutionGuard::enter);
            assert_eq!(
                current_thread_holds_runtime_execution_lease(),
                mode == "lease"
            );
            crate::state::runtime_state::molt_runtime_exit(0);
        });
        panic!("process exit returned");
    }
    for mode in ["no-lease", "lease"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "state::lifecycle::shutdown_tests::process_exit_covers_collection_before_pending_callbacks_with_or_without_lease",
                "--ignored", "--nocapture", "--test-threads=1",
            ])
            .env(CHILD, mode);
        let output = crate::test_support::captured_runtime_children::capture(
            &mut command,
            "process-exit-callbacks",
            mode,
        );
        assert!(output.status.success(), "{mode}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&format!(
                "shutdown callbacks verified before process exit: {mode}\n"
            )),
            "{mode}: {output:?}",
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn joined_service_readiness_is_one_serialized_result() {
    use super::RuntimeServiceWorkers;
    use crate::state::runtime_state::RuntimeWorkerOrigin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let origin = RuntimeWorkerOrigin::new(crate::runtime_state(py));
        let owner = RuntimeServiceWorkers::default();
        let starts = AtomicUsize::new(0);
        let (inside_tx, inside_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let owner_ref = &owner;
            let starts_ref = &starts;
            let first = scope.spawn(move || {
                owner_ref.start(origin, |_| {
                    starts_ref.fetch_add(1, Ordering::SeqCst);
                    inside_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
            });
            inside_rx.recv().unwrap();
            let second = scope.spawn(|| {
                let outcome = owner.start(origin, |_| {
                    starts.fetch_add(1, Ordering::SeqCst);
                    Err(std::io::Error::other("second startup must not execute"))
                });
                returned_tx.send(outcome.is_ok()).unwrap();
            });
            assert!(
                returned_rx
                    .recv_timeout(std::time::Duration::from_millis(25))
                    .is_err()
            );
            release_tx.send(()).unwrap();
            assert!(first.join().unwrap().is_ok());
            second.join().unwrap();
            assert!(returned_rx.recv().unwrap());
        });
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(owner.join());
        let failed = RuntimeServiceWorkers::default();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            failed.start(origin, |_| panic!("injected startup panic"))
        }));
        assert!(panic.is_err());
        assert!(
            failed
                .start(origin, |_| panic!("failed startup retried"))
                .is_err()
        );
        assert!(
            failed.join(),
            "failed startup without native children has no live handles"
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn admitted_joined_service_closes_without_dispatch() {
    use crate::state::runtime_state::RuntimeWorkerOrigin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let gil = crate::GilGuard::new();
    let py = gil.token();
    let state = crate::runtime_state(&py);
    let origin = RuntimeWorkerOrigin::new(state);
    let queue = Arc::new(crate::async_rt::scheduler::SleepQueue::new());
    assert!(state.sleep_queue.set(Arc::clone(&queue)).is_ok());
    let storage = Arc::downgrade(&queue);
    let dispatched = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let witness = Arc::clone(&dispatched);
    let done = Arc::clone(&finished);
    queue
        .install_joined_test_worker(origin, false, move || {
            let entry =
                crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin);
            if entry.is_some() {
                witness.store(true, Ordering::SeqCst);
            }
            done.store(true, Ordering::SeqCst);
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // Retain GIL: the actual service admission succeeds, while its callback
    // cannot cross the execution boundary until shutdown releases GIL to join.
    while state.thread_borrow_count() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "service did not reach admission"
        );
        std::thread::yield_now();
    }
    assert_eq!(active_runtime_execution_lease_count(), 1);
    drop(queue);
    assert_eq!(molt_runtime_shutdown(), 1);
    assert!(finished.load(Ordering::SeqCst));
    assert!(
        !dispatched.load(Ordering::SeqCst),
        "ordinary callback began after admission closed"
    );
    assert!(storage.upgrade().is_none());
    assert_eq!(active_runtime_execution_lease_count(), 0);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn inflight_joined_service_finishes_under_retained_origin(foreign_c: bool) {
    use crate::state::runtime_state::RuntimeWorkerOrigin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    // A separate unjoined C owner must survive the service's genuine cleanup.
    // The terminal child owns this parked thread until process reclamation.
    if foreign_c {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let foreign = std::thread::spawn(move || {
            {
                let _entry = RuntimeExecutionGuard::enter();
            }
            assert!(molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
            ready_tx.send(()).unwrap();
            loop {
                std::thread::park();
            }
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let _retained = std::mem::ManuallyDrop::new(foreign);
    }
    let finished = Arc::new(AtomicBool::new(false));
    let storage = {
        let entry = RuntimeExecutionGuard::enter_with_worker_cleanup();
        let py = entry.token();
        let state = crate::runtime_state(&py);
        let origin = RuntimeWorkerOrigin::new(state);
        let queue = Arc::new(crate::async_rt::scheduler::SleepQueue::new());
        assert!(state.sleep_queue.set(Arc::clone(&queue)).is_ok());
        let worker_queue = Arc::clone(&queue);
        let done = Arc::clone(&finished);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        queue
            .install_joined_test_worker(origin, false, move || {
                let work =
                    crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
                        .unwrap();
                // Nested direct acquisition inherits the existing service role.
                let baseline = active_runtime_execution_lease_count();
                assert_eq!(
                    crate::state::runtime_state::try_acquire_runtime_execution_lease(true),
                    Some(true)
                );
                assert_eq!(active_runtime_execution_lease_count(), baseline + 1);
                crate::state::runtime_state::release_runtime_execution_lease();
                let retained = unsafe { molt_cpython_abi::api::object::PyThreadState_Get() };
                assert!(!retained.is_null());
                {
                    let _release = crate::GilReleaseGuard::suspend();
                    ready_tx.send(()).unwrap();
                    worker_queue.wait_for_test_stop();
                    // First C Ensure on this fresh worker, while the outer GIL
                    // release guard remains alive. The real owner is Finalizing
                    // and joining us: reacquiring by dropping _release first would
                    // hide the lifecycle-init/admission deadlock regression.
                    assert!(crate::state::runtime_state::runtime_is_initialized());
                    assert!(!crate::state::runtime_state::runtime_is_ready());
                    assert!(!crate::gil_held());
                    assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
                    let count = active_runtime_execution_lease_count();
                    let c_state = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                    assert!(crate::gil_held());
                    assert_eq!(
                        crate::state::runtime_state::runtime_state_for_gil()
                            .unwrap()
                            .identity(),
                        origin.identity()
                    );
                    assert_eq!(
                        unsafe { molt_cpython_abi::api::object::PyThreadState_Get() },
                        retained
                    );
                    assert!(
                        crate::state::execution_tls().lease_depth.get()
                            & crate::state::runtime_state::RUNTIME_EXECUTION_SERVICE_ROLE
                            != 0
                    );
                    assert_eq!(origin.state().thread_borrow_count(), 1);
                    assert!(
                        obj_from_bits(crate::molt_getrecursionlimit())
                            .as_int()
                            .unwrap()
                            > 0
                    );
                    assert_eq!(
                        crate::runtime_state(&work.token()).identity(),
                        origin.identity()
                    );
                    assert_eq!(active_runtime_execution_lease_count(), count);
                    unsafe { molt_cpython_abi::api::object::PyGILState_Release(c_state) };
                    assert!(!crate::gil_held());
                    assert!(molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
                    assert!(
                        molt_cpython_abi::api::object::current_thread_has_retained_runtime_state()
                    );
                    assert_eq!(active_runtime_execution_lease_count(), count);
                    // Reacquire solely for observing identity; the matching guard
                    // drops before _release restores the original execution lane.
                    {
                        let _observe = crate::GilGuard::new();
                        assert_eq!(
                            unsafe { molt_cpython_abi::api::object::PyThreadState_Get() },
                            retained
                        );
                    }
                }
                assert!(!crate::state::runtime_state::runtime_is_ready());
                assert_eq!(
                    crate::runtime_state(&work.token()).identity(),
                    origin.identity()
                );
                assert!(current_thread_holds_runtime_execution_lease());
                let count = active_runtime_execution_lease_count();
                assert!(
                    obj_from_bits(crate::molt_getrecursionlimit())
                        .as_int()
                        .unwrap()
                        > 0
                );
                assert_eq!(active_runtime_execution_lease_count(), count);
                done.store(true, Ordering::SeqCst);
            })
            .unwrap();
        {
            let _release = crate::GilReleaseGuard::suspend();
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        Arc::downgrade(&queue)
    };
    assert_eq!(active_runtime_execution_lease_count(), 1);
    assert_eq!(molt_runtime_shutdown(), u64::from(!foreign_c));
    assert!(
        finished.load(Ordering::SeqCst),
        "joined cleanup must precede external-C refusal"
    );
    assert_eq!(storage.upgrade().is_some(), foreign_c);
    assert_eq!(active_runtime_execution_lease_count(), 0);
    if foreign_c {
        assert_eq!(molt_runtime_init(), 0);
        assert_eq!(molt_runtime_shutdown(), 0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn joined_service_cannot_exit_own_runtime() {
    use crate::state::runtime_state::RuntimeWorkerOrigin;
    use std::sync::Arc;
    let gil = crate::GilGuard::new();
    let py = gil.token();
    let state = crate::runtime_state(&py);
    let origin = RuntimeWorkerOrigin::new(state);
    let queue = Arc::new(crate::async_rt::scheduler::SleepQueue::new());
    assert!(state.sleep_queue.set(Arc::clone(&queue)).is_ok());
    queue
        .install_joined_test_worker(origin, false, move || {
            let _work =
                crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
                    .unwrap();
            // Exercise recursive ordinary entry as well: service role must survive.
            let _nested = RuntimeExecutionGuard::enter();
            crate::state::runtime_state::molt_runtime_exit(0);
            panic!("service process exit returned");
        })
        .unwrap();
    drop(gil);
    // The child must terminate through ProcessExit; parent custody bounds hangs.
    loop {
        std::thread::park();
    }
}
