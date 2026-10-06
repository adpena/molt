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
    if std::env::var_os("MOLT_SHUTDOWN_CUSTODY_TEST_CHILD").is_some() {
        assert_eq!(
            *CALLBACKS.lock().unwrap(),
            ["cycle", "pending", "atexit", "flush"]
        );
        println!("shutdown callbacks verified before process exit");
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
            flags: Py_TPFLAGS_DEFAULT,
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
        assert_eq!(
            crate::builtins::atexit::molt_atexit_register(
                callback,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            ),
            callback,
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
            ["cycle", "pending", "atexit", "flush", "module-cycle"]
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
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            prepare_callbacks(true, false);
            let _execution = (mode == "lease").then(RuntimeExecutionGuard::enter);
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
            String::from_utf8_lossy(&output.stdout)
                .contains("shutdown callbacks verified before process exit"),
            "{mode}: {output:?}",
        );
    }
}
