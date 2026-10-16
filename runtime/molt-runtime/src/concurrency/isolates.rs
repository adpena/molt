use crate::{MoltObject, PyToken, alloc_list, raise_exception};

#[cfg(not(target_arch = "wasm32"))]
use super::current_thread_id;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex, Weak};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::GilGuard;
#[cfg(not(target_arch = "wasm32"))]
use crate::audit::AuditArgs;
#[cfg(not(target_arch = "wasm32"))]
use crate::builtins::attr::attr_name_bits_from_bytes;
#[cfg(not(target_arch = "wasm32"))]
use crate::builtins::modules::molt_module_cache_get;
#[cfg(not(target_arch = "wasm32"))]
use crate::call::dispatch::call_callable1;
#[cfg(not(target_arch = "wasm32"))]
use crate::object::native_handle::{native_handle_arc, native_handle_new};
#[cfg(not(target_arch = "wasm32"))]
use crate::state::runtime_state::{RuntimeLifetimeLease, RuntimeThreadBorrow};
#[cfg(not(target_arch = "wasm32"))]
use crate::state::{
    RuntimeState, clear_thread_runtime_state, runtime_reset_for_init, runtime_teardown_isolate,
    set_thread_runtime_state, touch_tls_guard,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    TYPE_ID_BYTES, alloc_bytes, alloc_string, alloc_tuple, bytes_data, bytes_len, dec_ref_bits,
    exception_pending, format_exception_with_traceback, inc_ref_bits, is_truthy,
    molt_exception_clear, molt_exception_last_pending, molt_module_get_attr, obj_from_bits,
    object_type_id, ptr_from_bits, runtime_state, string_obj_to_owned, to_i64,
};

#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" {
    fn molt_isolate_bootstrap() -> u64;
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct MoltThreadHandle {
    pending_call: Mutex<Option<SharedThreadCall>>,
    done: AtomicBool,
    ident: AtomicU64,
    native_id: AtomicU64,
    join_handle: Mutex<Option<thread::JoinHandle<()>>>,
    wait_lock: Mutex<()>,
    condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltThreadHandle {
    fn new() -> Self {
        Self {
            pending_call: Mutex::new(None),
            done: AtomicBool::new(false),
            ident: AtomicU64::new(0),
            native_id: AtomicU64::new(0),
            join_handle: Mutex::new(None),
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    fn publish_call(&self, py: &PyToken<'_>, callable_bits: u64, args_bits: u64, kwargs_bits: u64) {
        let mut pending = self.pending_call.lock().unwrap();
        assert!(
            pending.is_none(),
            "native thread call was already published"
        );
        inc_ref_bits(py, callable_bits);
        if !obj_from_bits(args_bits).is_none() {
            inc_ref_bits(py, args_bits);
        }
        if !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(py, kwargs_bits);
        }
        *pending = Some(SharedThreadCall {
            runtime_owner: runtime_state(py).identity(),
            callable_bits,
            args_bits,
            kwargs_bits,
        });
    }

    fn call_args(&self, py: &PyToken<'_>) -> (u64, u64, u64) {
        let pending = self.pending_call.lock().unwrap();
        let call = pending
            .as_ref()
            .expect("native worker lost its admitted call");
        assert_eq!(call.runtime_owner, runtime_state(py).identity());
        (call.callable_bits, call.args_bits, call.kwargs_bits)
    }

    fn release_call(&self, py: &PyToken<'_>) {
        let mut pending = self.pending_call.lock().unwrap();
        let Some(call) = pending.as_ref() else {
            return;
        };
        assert_eq!(call.runtime_owner, runtime_state(py).identity());
        let mut call = pending.take().unwrap();
        drop(pending);
        // A decref can invoke arbitrary finalizers. No native mutex spans it,
        // and consumed fields are cleared before callbacks so unwind cannot
        // release them twice. Failed cleanup republishes the remaining owner.
        let released = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for field in [
                &mut call.callable_bits,
                &mut call.args_bits,
                &mut call.kwargs_bits,
            ] {
                let bits = std::mem::replace(field, MoltObject::none().bits());
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        }));
        if let Err(panic) = released {
            let mut pending = self.pending_call.lock().unwrap();
            assert!(
                pending.is_none(),
                "native call republished during retirement"
            );
            *pending = Some(call);
            drop(pending);
            std::panic::resume_unwind(panic);
        }
    }

    fn set_join_handle(&self, handle: thread::JoinHandle<()>) {
        let mut guard = self.join_handle.lock().unwrap();
        *guard = Some(handle);
    }

    fn mark_started(&self, ident: u64, native_id: u64) {
        let _guard = self.wait_lock.lock().unwrap();
        self.ident.store(ident, AtomicOrdering::Release);
        self.native_id.store(native_id, AtomicOrdering::Release);
        self.condvar.notify_all();
    }

    fn mark_done(&self) {
        let _guard = self.wait_lock.lock().unwrap();
        self.done.store(true, AtomicOrdering::Release);
        self.condvar.notify_all();
    }

    fn wait_started(&self) -> bool {
        if self.ident.load(AtomicOrdering::Acquire) != 0 {
            return true;
        }
        let guard = self.wait_lock.lock().unwrap();
        let _guard = self
            .condvar
            .wait_while(guard, |_| {
                self.ident.load(AtomicOrdering::Acquire) == 0
                    && !self.done.load(AtomicOrdering::Acquire)
            })
            .unwrap();
        self.ident.load(AtomicOrdering::Acquire) != 0
    }

    fn wait(&self, timeout: Option<Duration>) -> bool {
        if self.done.load(AtomicOrdering::Acquire) {
            return true;
        }
        let guard = self.wait_lock.lock().unwrap();
        match timeout {
            Some(wait) => {
                let (_guard, _) = self
                    .condvar
                    .wait_timeout_while(guard, wait, |_| !self.done.load(AtomicOrdering::Acquire))
                    .unwrap();
            }
            None => {
                let _guard = self
                    .condvar
                    .wait_while(guard, |_| !self.done.load(AtomicOrdering::Acquire))
                    .unwrap();
            }
        }
        self.done.load(AtomicOrdering::Acquire)
    }

    fn join(&self) {
        let handle = {
            let mut guard = self.join_handle.lock().unwrap();
            guard.take()
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadRegistryEntry {
    handle: Weak<MoltThreadHandle>,
    name: String,
    daemon: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct ThreadRegistry {
    main_name: Option<String>,
    main_daemon: bool,
    main_ident: u64,
    entries: HashMap<usize, ThreadRegistryEntry>,
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry<'a>(py: &'a PyToken<'_>) -> &'a Mutex<ThreadRegistry> {
    runtime_state(py)
        .thread_registry
        .get_or_init(|| Mutex::new(ThreadRegistry::default()))
}

#[cfg(not(target_arch = "wasm32"))]
struct SharedThreadCall {
    runtime_owner: usize,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
}

#[cfg(not(target_arch = "wasm32"))]
static THREAD_STACK_SIZE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[cfg(not(target_arch = "wasm32"))]
const THREAD_STACK_SIZE_MIN: usize = 32 * 1024;

#[cfg(not(target_arch = "wasm32"))]
fn thread_handle_from_bits(bits: u64) -> Option<Arc<MoltThreadHandle>> {
    native_handle_arc(bits)
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_set_main(py: &PyToken<'_>, name: String, daemon: bool) {
    let ident = current_thread_id();
    let mut registry = thread_registry(py).lock().unwrap();
    registry.main_name = Some(name);
    registry.main_daemon = daemon;
    registry.main_ident = ident;
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_register(
    py: &PyToken<'_>,
    handle: &Arc<MoltThreadHandle>,
    name: String,
    daemon: bool,
) {
    let mut registry = thread_registry(py).lock().unwrap();
    registry.entries.insert(
        Arc::as_ptr(handle).expose_provenance(),
        ThreadRegistryEntry {
            handle: Arc::downgrade(handle),
            name,
            daemon,
        },
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_forget(py: &PyToken<'_>, handle: &Arc<MoltThreadHandle>) {
    thread_registry(py)
        .lock()
        .unwrap()
        .entries
        .remove(&Arc::as_ptr(handle).expose_provenance());
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_cleanup_locked(registry: &mut ThreadRegistry) {
    registry
        .entries
        .retain(|_, entry| entry.handle.upgrade().is_some());
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_record_tuple(
    _py: &PyToken<'_>,
    name: &str,
    daemon: bool,
    ident: u64,
    native_id: u64,
    alive: bool,
    is_main: bool,
) -> u64 {
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let daemon_bits = MoltObject::from_bool(daemon).bits();
    let ident_bits = if ident == 0 {
        MoltObject::none().bits()
    } else {
        MoltObject::from_int(ident as i64).bits()
    };
    let native_bits = if native_id == 0 {
        MoltObject::none().bits()
    } else {
        MoltObject::from_int(native_id as i64).bits()
    };
    let alive_bits = MoltObject::from_bool(alive).bits();
    let is_main_bits = MoltObject::from_bool(is_main).bits();
    let tuple_ptr = alloc_tuple(
        _py,
        &[
            name_bits,
            daemon_bits,
            ident_bits,
            native_bits,
            alive_bits,
            is_main_bits,
        ],
    );
    dec_ref_bits(_py, name_bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn call_thread_callable(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let args_obj = obj_from_bits(args_bits);
    let kwargs_obj = obj_from_bits(kwargs_bits);
    let has_args = !args_obj.is_none();
    let has_kwargs = !kwargs_obj.is_none();
    if !has_args && !has_kwargs {
        return unsafe { crate::call_callable0(_py, callable_bits) };
    }
    let builder_bits = crate::molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if has_args {
        let _ = unsafe { crate::molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    if has_kwargs {
        let _ = unsafe { crate::molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    crate::molt_call_bind(callable_bits, builder_bits)
}

#[cfg(not(target_arch = "wasm32"))]
fn payload_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, String> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("thread payload must be bytes".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_BYTES {
            return Err("thread payload must be bytes".to_string());
        }
        let len = bytes_len(ptr);
        let data = std::slice::from_raw_parts(bytes_data(ptr), len);
        Ok(data.to_vec())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn log_thread_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last_pending();
    if !obj_from_bits(exc_bits).is_none() {
        let exc_ptr = ptr_from_bits(exc_bits);
        if !exc_ptr.is_null() {
            let formatted = format_exception_with_traceback(_py, exc_ptr);
            eprintln!("molt thread exception: {formatted}");
        }
    }
    molt_exception_clear();
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_thread_payload(payload: Vec<u8>) {
    crate::with_gil_entry_nopanic!(_py, {
        let module_ptr = alloc_string(_py, b"threading");
        if module_ptr.is_null() {
            return;
        }
        let module_bits = MoltObject::from_ptr(module_ptr).bits();
        let mut loaded_bits = molt_module_cache_get(module_bits);
        if obj_from_bits(loaded_bits).is_none() {
            // Import bedrock: registry resolve → molt_module_ensure (the old
            // app-owned molt_isolate_import chain is deleted on native).
            loaded_bits = crate::builtins::module_table::isolate_import_dispatch(_py, "threading");
        }
        dec_ref_bits(_py, module_bits);
        if obj_from_bits(loaded_bits).is_none() {
            log_thread_exception(_py);
            return;
        }
        let Some(attr_bits) = attr_name_bits_from_bytes(_py, b"_molt_thread_run") else {
            dec_ref_bits(_py, loaded_bits);
            return;
        };
        let func_bits = molt_module_get_attr(loaded_bits, attr_bits);
        if obj_from_bits(func_bits).is_none() {
            dec_ref_bits(_py, loaded_bits);
            return;
        }
        let payload_ptr = alloc_bytes(_py, &payload);
        if payload_ptr.is_null() {
            dec_ref_bits(_py, func_bits);
            dec_ref_bits(_py, loaded_bits);
            return;
        }
        let payload_bits = MoltObject::from_ptr(payload_ptr).bits();
        let result_bits = unsafe { call_callable1(_py, func_bits, payload_bits) };
        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, payload_bits);
        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, loaded_bits);
        log_thread_exception(_py);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn run_thread_payload_shared(py: &PyToken<'_>, handle: &MoltThreadHandle) {
    let (callable, args, kwargs) = handle.call_args(py);
    let result_bits = call_thread_callable(py, callable, args, kwargs);
    if !obj_from_bits(result_bits).is_none() {
        dec_ref_bits(py, result_bits);
    }
    log_thread_exception(py);
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main(payload: Vec<u8>, handle: Arc<MoltThreadHandle>, lifetime: RuntimeLifetimeLease) {
    run_isolate_thread(
        lifetime,
        handle,
        || unsafe { molt_isolate_bootstrap() },
        || run_thread_payload(payload),
    );
}

/// Isolate teardowns that panicked in this test process. A failed teardown
/// retains its isolate and keeps process shutdown closed, yet the test that
/// launched it can still pass; the runtime test transaction reads this count
/// so that test fails instead.
#[cfg(all(test, not(target_arch = "wasm32")))]
static ISOLATE_TEARDOWN_FAILURES: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn isolate_teardown_failure_count() -> usize {
    ISOLATE_TEARDOWN_FAILURES.load(AtomicOrdering::SeqCst)
}

/// Own the complete fresh-isolate lifecycle. The final-image export remains the
/// sole application initializer; parameters make this lifecycle testable without
/// installing a second global provider or invoking libtest's Unavailable export.
#[cfg(not(target_arch = "wasm32"))]
fn run_isolate_thread(
    lifetime: RuntimeLifetimeLease,
    handle: Arc<MoltThreadHandle>,
    bootstrap: impl FnOnce() -> u64,
    payload: impl FnOnce(),
) {
    // Once execution starts, unexpected unwind must preserve primary custody
    // with any partially initialized isolate; cancellation before start drops
    // the ordinary lease captured by the native spawn closure.
    let lifetime = std::mem::ManuallyDrop::new(lifetime);
    let thread_id = current_thread_id();
    handle.mark_started(thread_id, thread_id);
    let state = Box::new(RuntimeState::new_isolate(&lifetime));
    let state_ptr = Box::into_raw(state);
    let gc_owner = crate::object::gc::IsolateGcOwner::new(unsafe { &*state_ptr }, &lifetime);
    set_thread_runtime_state(state_ptr);
    touch_tls_guard();
    let setup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        {
            let gil = GilGuard::new();
            let py = gil.token();
            runtime_reset_for_init(&py, unsafe { &*state_ptr });
            crate::builtins::exceptions::memory_error::initialize(&py);
        }
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                log_thread_exception(_py);
                return false;
            }
            let result = bootstrap();
            // The application ABI returns an owned Molt result, including on
            // exceptional paths. Discarding it must release that ownership.
            dec_ref_bits(_py, result);
            let initialized = !exception_pending(_py);
            // Reporting may clear pending state, but cannot change the captured
            // initialization outcome into permission to execute the payload.
            log_thread_exception(_py);
            initialized
        })
    }));
    match setup {
        Ok(true) => {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(payload)).is_err() {
                eprintln!("MOLT_ISOLATE_PAYLOAD_PANIC: payload unwound; tearing down isolate");
            }
        }
        Ok(false) => {}
        Err(_) => {
            eprintln!("MOLT_ISOLATE_BOOTSTRAP_PANIC: initialization unwound; payload skipped");
        }
    }
    let teardown = std::panic::catch_unwind(|| {
        let gil = GilGuard::new();
        let py = gil.token();
        runtime_teardown_isolate(&py, unsafe { &*state_ptr });
        gc_owner.retire();
    });
    if teardown.is_err() {
        #[cfg(test)]
        ISOLATE_TEARDOWN_FAILURES.fetch_add(1, AtomicOrdering::SeqCst);
        eprintln!(
            "MOLT_ISOLATE_TEARDOWN_PANIC: retaining failed isolate at {state_ptr:p}; process shutdown remains closed"
        );
    }
    if teardown.is_ok() {
        clear_thread_runtime_state();
        unsafe {
            drop(Box::from_raw(state_ptr));
        }
        drop(std::mem::ManuallyDrop::into_inner(lifetime));
    }
    // A failed owner remains bound through native TLS destruction. Clearing it
    // here would make retained isolate edges fall back to the primary runtime.
    handle.mark_done();
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod bootstrap_failure_tests {
    use super::*;
    use crate::runtime_flags::RuntimeFlag;
    use crate::{builtin_classes, runtime_state};
    use std::ffi::c_void;
    use std::os::raw::c_int;

    static THREAD_LIFETIME_TEARDOWN_CALLS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn thread_lifetime_teardown_probe() -> u64 {
        THREAD_LIFETIME_TEARDOWN_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        println!("native thread lifetime teardown callback ran");
        MoltObject::none().bits()
    }

    fn register_thread_lifetime_probe(py: &PyToken<'_>) {
        let callback = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "thread_lifetime_teardown_probe",
                thread_lifetime_teardown_probe as *const (),
            ),
            0,
        );
        assert!(!callback.is_null());
        let callback = MoltObject::from_ptr(callback).bits();
        // The registry owns its reference; the result carries none.
        assert!(
            obj_from_bits(crate::builtins::atexit::molt_atexit_register(
                callback,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            ))
            .is_none()
        );
        dec_ref_bits(py, callback);
    }

    /// Every scenario uses an actual native worker or the actual public spawn
    /// receiver. Terminal retained-owner cases belong in fresh child processes.
    #[test]
    #[cfg(panic = "unwind")]
    fn native_thread_admission_and_origin_retirement() {
        use crate::state::runtime_state::{
            active_runtime_execution_lease_count, molt_runtime_init, molt_runtime_shutdown,
        };
        const MODE: &str = "MOLT_NATIVE_THREAD_LIFETIME_TEST";
        const TEST: &str = "concurrency::isolates::bootstrap_failure_tests::native_thread_admission_and_origin_retirement";
        if let Ok(mode) = std::env::var(MODE) {
            assert_eq!(molt_runtime_init(), 1);
            let primary = crate::state::runtime_state::runtime_state_for_gil()
                .unwrap()
                .identity();
            match mode.as_str() {
                "delayed-isolate" | "delayed-shared" => {
                    let handle = Arc::new(MoltThreadHandle::new());
                    let worker_handle = Arc::clone(&handle);
                    let completed = Arc::new(AtomicBool::new(false));
                    let witness = Arc::clone(&completed);
                    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
                    let (release_tx, release_rx) = std::sync::mpsc::channel();
                    let worker = if mode == "delayed-isolate" {
                        let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
                        spawn_runtime_thread(move || {
                            entered_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                            run_isolate_thread(
                                lifetime,
                                worker_handle,
                                || MoltObject::none().bits(),
                                || {
                                    crate::with_gil_entry_nopanic!(py, {
                                        assert_ne!(runtime_state(py).identity(), primary);
                                        crate::async_rt::process::assert_process_origin_wake(py);
                                    });
                                    witness.store(true, AtomicOrdering::Release);
                                },
                            );
                        })
                        .unwrap()
                    } else {
                        let origin = {
                            let entry = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                            RuntimeThreadBorrow::try_acquire(&entry.token()).unwrap()
                        };
                        spawn_runtime_thread(move || {
                            entered_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                            run_shared_thread(worker_handle, origin, || {
                                crate::with_gil_entry_nopanic!(py, {
                                    assert_eq!(runtime_state(py).identity(), primary);
                                    witness.store(true, AtomicOrdering::Release);
                                });
                            });
                        })
                        .unwrap()
                    };
                    entered_rx.recv().unwrap();
                    assert_eq!(handle.ident.load(AtomicOrdering::Acquire), 0);
                    assert_eq!(active_runtime_execution_lease_count(), 1);
                    assert_eq!(
                        molt_runtime_shutdown(),
                        0,
                        "not-yet-started worker owns lifetime"
                    );
                    assert_eq!(
                        crate::state::runtime_state::runtime_state_for_gil()
                            .unwrap()
                            .identity(),
                        primary
                    );
                    release_tx.send(()).unwrap();
                    worker.join().unwrap();
                    assert!(
                        completed.load(AtomicOrdering::Acquire),
                        "caught payload assertions must not count as completion"
                    );
                    assert!(handle.done.load(AtomicOrdering::Acquire));
                    assert_eq!(active_runtime_execution_lease_count(), 0);
                    assert_eq!(molt_runtime_shutdown(), 1);
                    assert!(
                        RuntimeLifetimeLease::try_acquire().is_none(),
                        "closed process must reject before worker admission"
                    );
                }
                "cancel-before-start" => {
                    let origin = {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        RuntimeThreadBorrow::try_acquire(&entry.token()).unwrap()
                    };
                    let canceled: Box<dyn FnOnce() + Send> = Box::new(move || drop(origin));
                    assert_eq!(active_runtime_execution_lease_count(), 1);
                    drop(canceled);
                    assert_eq!(active_runtime_execution_lease_count(), 0);
                    let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
                    let canceled: Box<dyn FnOnce() + Send> = Box::new(move || drop(lifetime));
                    drop(canceled);
                    assert_eq!(active_runtime_execution_lease_count(), 0);
                    assert_eq!(molt_runtime_shutdown(), 1);
                }
                "spawn-failure-shared" | "spawn-failure-bytes" | "spawn-failure-isolate" => {
                    let entry =
                        crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                    let py = entry.token();
                    let baseline = active_runtime_execution_lease_count();
                    let state = runtime_state(&py);
                    if mode == "spawn-failure-shared" {
                        let callable = crate::builtins::functions::alloc_runtime_function_obj(
                            &py,
                            crate::builtins::functions::runtime_fn_addr(
                                "thread_lifetime_teardown_probe",
                                thread_lifetime_teardown_probe as *const (),
                            ),
                            0,
                        );
                        assert!(!callable.is_null());
                        let refs =
                            unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() };
                        NATIVE_THREAD_SPAWN_TEST_FAIL.with(|fail| fail.set(true));
                        unsafe {
                            molt_thread_spawn_shared(
                                MoltObject::from_ptr(callable).bits(),
                                MoltObject::none().bits(),
                                MoltObject::none().bits(),
                            )
                        };
                        assert!(
                            !NATIVE_THREAD_SPAWN_TEST_FAIL.with(|fail| fail.get()),
                            "actual native spawn producer was not reached"
                        );
                        assert_eq!(
                            unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() },
                            refs
                        );
                        dec_ref_bits(&py, MoltObject::from_ptr(callable).bits());
                    } else {
                        let payload = alloc_bytes(&py, b"not-executed-after-creation-failure");
                        assert!(!payload.is_null());
                        NATIVE_THREAD_SPAWN_TEST_FAIL.with(|fail| fail.set(true));
                        unsafe { molt_thread_spawn(MoltObject::from_ptr(payload).bits()) };
                        assert!(!NATIVE_THREAD_SPAWN_TEST_FAIL.with(|fail| fail.get()));
                        dec_ref_bits(&py, MoltObject::from_ptr(payload).bits());
                    }
                    let (error, message) = pending_runtime_error(&py);
                    assert!(
                        error && message.contains("injected native thread creation failure"),
                        "{message}"
                    );
                    molt_exception_clear();
                    assert_eq!(state.thread_borrow_count(), 0);
                    assert_eq!(active_runtime_execution_lease_count(), baseline);
                    drop(entry);
                    assert_eq!(molt_runtime_shutdown(), 1);
                }
                "shared-call-identity" => {
                    let entry =
                        crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                    let py = entry.token();
                    let primary = runtime_state(&py).identity();
                    thread_registry_set_main(&py, "primary-origin".into(), false);
                    let callable = crate::builtins::functions::alloc_runtime_function_obj(
                        &py,
                        crate::builtins::functions::runtime_fn_addr(
                            "thread_lifetime_teardown_probe",
                            thread_lifetime_teardown_probe as *const (),
                        ),
                        0,
                    );
                    assert!(!callable.is_null());
                    let callable_bits = MoltObject::from_ptr(callable).bits();
                    let refs =
                        unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() };
                    let handle = Arc::new(MoltThreadHandle::new());
                    handle.publish_call(
                        &py,
                        callable_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                    );
                    thread_registry_register(&py, &handle, "delayed-primary".into(), false);
                    let origin = RuntimeThreadBorrow::try_acquire(&py).unwrap();
                    let worker_handle = Arc::clone(&handle);
                    let (release_tx, release_rx) = std::sync::mpsc::channel();
                    let worker = spawn_runtime_thread(move || {
                        release_rx.recv().unwrap();
                        thread_main_shared_call(worker_handle, origin);
                    })
                    .unwrap();
                    // Dropping display metadata and the parent Arc must neither
                    // consume nor cancel the delayed native worker's payload.
                    thread_registry_forget(&py, &handle);
                    assert_eq!(handle.call_args(&py).0, callable_bits);
                    let weak = Arc::downgrade(&handle);
                    drop(handle);
                    assert!(weak.upgrade().is_some());
                    // A fresh origin starts with independent metadata and payload
                    // custody even while the first origin's worker is delayed.
                    let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
                    let isolate = spawn_runtime_thread(move || {
                        run_isolate_thread(
                            lifetime,
                            Arc::new(MoltThreadHandle::new()),
                            || MoltObject::none().bits(),
                            || {
                                crate::with_gil_entry_nopanic!(py, {
                                    assert_ne!(runtime_state(py).identity(), primary);
                                    assert!(
                                        thread_registry(py).lock().unwrap().main_name.is_none()
                                    );
                                    thread_registry_set_main(py, "isolate-origin".into(), true);
                                    let own = Arc::new(MoltThreadHandle::new());
                                    own.publish_call(
                                        py,
                                        MoltObject::from_int(73).bits(),
                                        MoltObject::none().bits(),
                                        MoltObject::none().bits(),
                                    );
                                    thread_registry_register(
                                        py,
                                        &own,
                                        "isolate-worker".into(),
                                        true,
                                    );
                                    assert_eq!(
                                        own.call_args(py).0,
                                        MoltObject::from_int(73).bits()
                                    );
                                    own.release_call(py);
                                    thread_registry_forget(py, &own);
                                });
                            },
                        );
                    })
                    .unwrap();
                    {
                        let _release = crate::concurrency::gil::GilReleaseGuard::suspend();
                        isolate.join().unwrap();
                    }
                    assert_eq!(
                        thread_registry(&py).lock().unwrap().main_name.as_deref(),
                        Some("primary-origin")
                    );
                    assert_eq!(
                        unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() },
                        refs + 1
                    );
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        0
                    );
                    release_tx.send(()).unwrap();
                    {
                        let _release = crate::concurrency::gil::GilReleaseGuard::suspend();
                        worker.join().unwrap();
                    }
                    assert!(weak.upgrade().is_none());
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        1
                    );
                    assert_eq!(
                        unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() },
                        refs
                    );
                    dec_ref_bits(&py, callable_bits);
                    drop(entry);
                    assert_eq!(molt_runtime_shutdown(), 1);
                }
                "nested-shared" => {
                    let (observed_tx, observed_rx) = std::sync::mpsc::channel();
                    let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
                    let isolate = spawn_runtime_thread(move || {
                        run_isolate_thread(
                            lifetime,
                            Arc::new(MoltThreadHandle::new()),
                            || MoltObject::none().bits(),
                            || {
                                crate::with_gil_entry_nopanic!(py, {
                                    register_thread_lifetime_probe(py);
                                    let state = runtime_state(py);
                                    let identity = state.identity();
                                    let class = builtin_classes(py).int;
                                    let queue = state.sleep_queue();
                                    let storage = Arc::downgrade(queue);
                                    {
                                        // Retire the service's Arc first: Weak now
                                        // observes RuntimeState's own storage only.
                                        let _release =
                                            crate::concurrency::gil::GilReleaseGuard::suspend();
                                        queue.shutdown(py);
                                    }
                                    let origin = RuntimeThreadBorrow::try_acquire(py).unwrap();
                                    let (release_tx, release_rx) = std::sync::mpsc::channel();
                                    let verified = Arc::new(AtomicBool::new(false));
                                    let witness = Arc::clone(&verified);
                                    let shared = spawn_runtime_thread(move || {
                                        release_rx.recv().unwrap();
                                        run_shared_thread(
                                            Arc::new(MoltThreadHandle::new()),
                                            origin,
                                            || {
                                                crate::with_gil_entry_nopanic!(py, {
                                                    assert_eq!(
                                                        runtime_state(py).identity(),
                                                        identity
                                                    );
                                                    assert_eq!(builtin_classes(py).int, class);
                                                    assert!(
                                                        RuntimeThreadBorrow::try_acquire(py)
                                                            .is_none(),
                                                        "retiring origin reopened shared admission"
                                                    );
                                                    let callable = crate::builtins::functions::alloc_runtime_function_obj(
                                                        py,
                                                        crate::builtins::functions::runtime_fn_addr(
                                                            "thread_lifetime_teardown_probe",
                                                            thread_lifetime_teardown_probe as *const (),
                                                        ),
                                                        0,
                                                    );
                                                    assert!(!callable.is_null());
                                                    let refs = unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() };
                                                    unsafe { molt_thread_spawn_shared(
                                                        MoltObject::from_ptr(callable).bits(),
                                                        MoltObject::none().bits(),
                                                        MoltObject::none().bits(),
                                                    ) };
                                                    let (error, message) = pending_runtime_error(py);
                                                    assert!(error, "{message}");
                                                    let exception = crate::builtins::exceptions::molt_exception_last_pending();
                                                    let exact = crate::builtins::exceptions::format_exception_message(py, obj_from_bits(exception).as_ptr().unwrap());
                                                    dec_ref_bits(py, exception);
                                                    assert_eq!(exact, "runtime is closing thread admission");
                                                    assert_eq!(unsafe { (*crate::header_from_obj_ptr(callable)).ref_count_snapshot() }, refs);
                                                    molt_exception_clear();
                                                    dec_ref_bits(py, MoltObject::from_ptr(callable).bits());
                                                    let value = alloc_string(
                                                        py,
                                                        b"origin still owns its classes",
                                                    );
                                                    assert!(!value.is_null());
                                                    dec_ref_bits(
                                                        py,
                                                        MoltObject::from_ptr(value).bits(),
                                                    );
                                                    witness.store(true, AtomicOrdering::Release);
                                                });
                                            },
                                        );
                                    })
                                    .unwrap();
                                    observed_tx
                                        .send((storage, shared, release_tx, verified))
                                        .unwrap();
                                });
                            },
                        );
                    })
                    .unwrap();
                    isolate.join().unwrap();
                    let (storage, shared, release, verified) = observed_rx.recv().unwrap();
                    assert!(
                        storage.upgrade().is_some(),
                        "live origin borrower lost its state"
                    );
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        0,
                        "borrow refusal must precede all teardown callbacks"
                    );
                    assert_eq!(molt_runtime_shutdown(), 0);
                    release.send(()).unwrap();
                    shared.join().unwrap();
                    assert!(verified.load(AtomicOrdering::Acquire));
                    assert!(
                        storage.upgrade().is_some(),
                        "failed isolate stays retained after borrower completion"
                    );
                    assert_eq!(active_runtime_execution_lease_count(), 1);
                    assert_eq!(molt_runtime_shutdown(), 0);
                }
                "shared-payload-panic" | "shared-cleanup-panic" => {
                    let cleanup_panic = mode == "shared-cleanup-panic";
                    let origin = {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        RuntimeThreadBorrow::try_acquire(&entry.token()).unwrap()
                    };
                    let handle = Arc::new(MoltThreadHandle::new());
                    let worker_handle = Arc::clone(&handle);
                    spawn_runtime_thread(move || {
                        run_shared_thread(worker_handle, origin, || {
                            if cleanup_panic {
                                assert!(!unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() }.is_null());
                                molt_cpython_abi::api::object::inject_thread_state_finalizer_panic_for_test();
                            } else {
                                panic!("intentional shared payload panic");
                            }
                        });
                    }).unwrap().join().unwrap();
                    assert!(handle.done.load(AtomicOrdering::Acquire));
                    assert_eq!(
                        active_runtime_execution_lease_count(),
                        usize::from(cleanup_panic)
                    );
                    assert_eq!(molt_runtime_shutdown(), u64::from(!cleanup_panic));
                }
                "joined-service-inflight" => {
                    crate::state::lifecycle::shutdown_tests::inflight_joined_service_finishes_under_retained_origin(false);
                }
                "joined-service-foreign-c" => {
                    {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        register_thread_lifetime_probe(&entry.token());
                    }
                    crate::state::lifecycle::shutdown_tests::inflight_joined_service_finishes_under_retained_origin(true);
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        0
                    );
                }
                "joined-service-exit" => {
                    {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        register_thread_lifetime_probe(&entry.token());
                    }
                    crate::state::lifecycle::shutdown_tests::joined_service_cannot_exit_own_runtime(
                    );
                }
                "joined-service-admitted" => {
                    crate::state::lifecycle::shutdown_tests::admitted_joined_service_closes_without_dispatch();
                }
                "joined-service-primary"
                | "joined-service-partial-start"
                | "joined-service-failed-join"
                | "joined-service-cleanup-panic" => {
                    let failed = matches!(
                        mode.as_str(),
                        "joined-service-failed-join" | "joined-service-cleanup-panic"
                    );
                    let storage = {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        register_thread_lifetime_probe(&entry.token());
                        crate::state::lifecycle::tests::install_joined_service_control(
                            &entry.token(),
                            mode == "joined-service-failed-join",
                            mode == "joined-service-partial-start",
                            mode == "joined-service-cleanup-panic",
                        )
                    };
                    assert_eq!(
                        active_runtime_execution_lease_count(),
                        0,
                        "idle service must not reserve process admission"
                    );
                    assert_eq!(molt_runtime_shutdown(), u64::from(!failed));
                    crate::state::lifecycle::tests::assert_joined_service_control_drained();
                    assert_eq!(storage.upgrade().is_some(), failed);
                    if failed {
                        assert_eq!(
                            THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                            0
                        );
                        assert_eq!(molt_runtime_init(), 0);
                        assert_eq!(molt_runtime_shutdown(), 0);
                    } else {
                        assert_eq!(
                            THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                            1
                        );
                    }
                }
                "joined-service-isolate" => {
                    let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
                    let (storage_tx, storage_rx) = std::sync::mpsc::channel();
                    let worker = spawn_runtime_thread(move || {
                        run_isolate_thread(lifetime, Arc::new(MoltThreadHandle::new()),
                            || MoltObject::none().bits(), || {
                                crate::with_gil_entry_nopanic!(py, {
                                    assert_ne!(runtime_state(py).identity(), primary);
                                    let storage = crate::state::lifecycle::tests::install_joined_service_control(py, false, false, false);
                                    storage_tx.send(storage).unwrap();
                                });
                            });
                    }).unwrap();
                    worker.join().unwrap();
                    assert!(storage_rx.recv().unwrap().upgrade().is_none());
                    crate::state::lifecycle::tests::assert_joined_service_control_drained();
                    assert_eq!(active_runtime_execution_lease_count(), 0);
                    assert_eq!(molt_runtime_shutdown(), 1);
                }
                "process-service-timeout" => {
                    let entry =
                        crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                    let py = entry.token();
                    register_thread_lifetime_probe(&py);
                    let queue = runtime_state(&py).sleep_queue();
                    let storage = Arc::downgrade(queue);
                    {
                        // Retire the service's Arc so Weak observes only the
                        // RuntimeState field before the terminal refusal.
                        let _release = crate::concurrency::gil::GilReleaseGuard::suspend();
                        queue.shutdown(&py);
                    }
                    let fixture =
                        crate::async_rt::process::install_process_service_timeout_fixture(&py);
                    drop(entry);
                    assert_eq!(molt_runtime_shutdown(), 0);
                    assert!(!crate::state::runtime_state::runtime_is_initialized());
                    assert!(!crate::state::runtime_state::runtime_is_ready());
                    assert!(
                        storage.upgrade().is_some(),
                        "failed process drain freed original RuntimeState storage"
                    );
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        0
                    );
                    fixture.assert_retained();
                    assert_eq!(molt_runtime_init(), 0, "Failed owner must not be replaced");
                    fixture.release_and_assert_closed();
                    assert!(storage.upgrade().is_some());
                    assert_eq!(active_runtime_execution_lease_count(), 0);
                    assert_eq!(
                        THREAD_LIFETIME_TEARDOWN_CALLS.load(AtomicOrdering::SeqCst),
                        0
                    );
                }
                "exit-with-pending-isolate" => {
                    {
                        let entry =
                            crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                        register_thread_lifetime_probe(&entry.token());
                    }
                    let _pending = RuntimeLifetimeLease::try_acquire().unwrap();
                    assert!(
                        !crate::state::runtime_state::current_thread_holds_runtime_execution_lease(
                        )
                    );
                    crate::state::runtime_state::molt_runtime_exit(0);
                    unreachable!("process exit returned");
                }
                _ => panic!("unknown native thread lifetime mode"),
            }
            println!("native thread lifetime verified: {mode}");
            return;
        }
        for mode in [
            "delayed-isolate",
            "delayed-shared",
            "cancel-before-start",
            "spawn-failure-shared",
            "spawn-failure-bytes",
            "spawn-failure-isolate",
            "nested-shared",
            "shared-call-identity",
            "shared-payload-panic",
            "shared-cleanup-panic",
            "exit-with-pending-isolate",
            "process-service-timeout",
            "joined-service-inflight",
            "joined-service-admitted",
            "joined-service-primary",
            "joined-service-isolate",
            "joined-service-partial-start",
            "joined-service-failed-join",
            "joined-service-cleanup-panic",
            "joined-service-foreign-c",
            "joined-service-exit",
        ] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                .env(MODE, mode)
                .env(
                    "MOLT_CAPABILITY_TIER",
                    molt_runtime_core::host_capabilities_generated::MAXIMUM_BUILTIN_CAPABILITY_TIER,
                )
                .env(
                    RuntimeFlag::MOLT_THREAD_ISOLATED.name(),
                    if mode == "spawn-failure-isolate" {
                        "1"
                    } else {
                        "0"
                    },
                );
            if mode == "process-service-timeout" {
                command.env("MOLT_PROCESS_TEARDOWN_JOIN_TIMEOUT_MS", "25");
            }
            let output = crate::test_support::captured_runtime_children::capture(
                &mut command,
                "native-thread-admission",
                mode,
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if mode == "joined-service-exit" {
                assert_eq!(output.status.code(), Some(1), "{stderr}");
                assert!(stderr.contains("molt runtime process exit refused: joined service callback cannot retire its own runtime"), "{stderr}");
                assert!(!stdout.contains("native thread lifetime teardown callback ran"));
            } else if mode == "exit-with-pending-isolate" {
                assert_eq!(output.status.code(), Some(1), "{stderr}");
                assert!(
                    stderr
                        .contains("process exit refused: 1 active execution lease(s) remain live"),
                    "{stderr}"
                );
                assert!(!stdout.contains("native thread lifetime teardown callback ran"));
            } else {
                if mode == "process-service-timeout" {
                    assert!(stderr.contains("molt runtime lifecycle failed: runtime teardown refused: process workers remain live or failed; origin retained"), "{stderr}");
                    assert!(!stdout.contains("native thread lifetime teardown callback ran"));
                }
                assert_eq!(output.status.code(), Some(0), "{mode}: {stderr}");
                assert!(
                    stdout.contains(&format!("native thread lifetime verified: {mode}")),
                    "{mode}: {stdout}\n{stderr}"
                );
            }
        }
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn completed_isolate_preserves_primary_resource_factory_for_new_workers() {
        use crate::resource::{OperationEstimate, ResourceError, with_tracker};
        use crate::state::runtime_state::{molt_runtime_init, molt_runtime_shutdown};
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            // An operation cap exercises real startup installation without
            // changing the host process's OS memory backstop.
            const KEY: &str = "MOLT_RESOURCE_MAX_REPEAT_RESULT";
            struct RestoreEnv(Option<std::ffi::OsString>);
            impl Drop for RestoreEnv {
                fn drop(&mut self) {
                    unsafe {
                        match &self.0 {
                            Some(value) => std::env::set_var(KEY, value),
                            None => std::env::remove_var(KEY),
                        }
                    }
                }
            }
            let _environment = RestoreEnv(std::env::var_os(KEY));
            unsafe {
                std::env::set_var(KEY, "1024");
            }
            assert_eq!(molt_runtime_init(), 1);
            fn assert_limited() {
                let result = with_tracker(|tracker| {
                    tracker.check_operation_size(&OperationEstimate::Repeat {
                        item_bytes: 1,
                        count: 2048,
                    })
                });
                assert!(
                    matches!(result, Err(ResourceError::OperationTooLarge { .. })),
                    "the startup cap must remain installed on this worker"
                );
            }
            assert_limited();
            let completed = Arc::new(AtomicBool::new(false));
            let verified = Arc::clone(&completed);
            let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
            let worker = spawn_runtime_thread(move || {
                run_isolate_thread(
                    lifetime,
                    Arc::new(MoltThreadHandle::new()),
                    || MoltObject::none().bits(),
                    || {
                        assert_limited();
                        verified.store(true, AtomicOrdering::Release);
                    },
                );
            })
            .unwrap();
            worker.join().unwrap();
            assert!(
                completed.load(AtomicOrdering::Acquire),
                "isolate payload did not complete"
            );
            // This is a fresh TLS tracker initialized after the actual isolate
            // teardown, not a tracker retained from before it.
            std::thread::spawn(assert_limited).join().unwrap();
            assert_limited();
            assert_eq!(molt_runtime_shutdown(), 1);
        });
    }

    #[test]
    fn isolate_inherits_startup_policy_after_process_environment_mutation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let expected = crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            (
                state.policy().stream_max_queued_bytes,
                state.policy().thread_isolated,
                state.capabilities.get().unwrap().clone(),
                *state.trusted.get().unwrap(),
                state.get_vfs().map(|vfs| vfs as *const _ as usize),
            )
        });
        let keys = [
            "MOLT_STREAM_MAX_QUEUED_BYTES",
            RuntimeFlag::MOLT_THREAD_ISOLATED.name(),
            "MOLT_CAPABILITY_TIER",
            "MOLT_CAPABILITIES",
            "MOLT_VFS_BUNDLE",
        ];
        let saved = keys.map(|key| (key, std::env::var_os(key)));
        unsafe {
            std::env::set_var(
                keys[0],
                if expected.0 == 65536 {
                    "1073741824"
                } else {
                    "65536"
                },
            );
            std::env::set_var(keys[1], if expected.1 { "0" } else { "1" });
            std::env::set_var(keys[2], if expected.3 { "none" } else { "full" });
            std::env::set_var(keys[3], "test.late-grant");
            std::env::set_var(keys[4], "late-untrusted-isolate-bundle");
        }
        let completed = Arc::new(AtomicBool::new(false));
        let verified = Arc::clone(&completed);
        let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
        let worker = spawn_runtime_thread(move || {
            run_isolate_thread(
                lifetime,
                Arc::new(MoltThreadHandle::new()),
                || MoltObject::none().bits(),
                || {
                    crate::with_gil_entry_nopanic!(py, {
                        let state = runtime_state(py);
                        assert_eq!(state.policy().stream_max_queued_bytes, expected.0);
                        assert_eq!(state.policy().thread_isolated, expected.1);
                        assert_eq!(state.capabilities.get().unwrap(), &expected.2);
                        assert_eq!(*state.trusted.get().unwrap(), expected.3);
                        assert_eq!(
                            state.get_vfs().map(|vfs| vfs as *const _ as usize),
                            expected.4
                        );
                        verified.store(true, AtomicOrdering::Release);
                    })
                },
            );
        })
        .unwrap();
        let joined = worker.join();
        for (key, value) in saved {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        joined.unwrap();
        assert!(
            completed.load(AtomicOrdering::Acquire),
            "isolate must execute with the inherited policy"
        );
    }

    static SHARED_DRAIN_ORIGIN: AtomicUsize = AtomicUsize::new(0);
    static SHARED_DRAIN_CLASS: AtomicU64 = AtomicU64::new(0);
    static SHARED_DRAIN_LATE_BITS: AtomicU64 = AtomicU64::new(0);
    static SHARED_DRAIN_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn shared_worker_c_finalizer_reentry() {
        crate::with_gil_entry_nopanic!(py, {
            assert_eq!(
                runtime_state(py).identity(),
                SHARED_DRAIN_ORIGIN.load(AtomicOrdering::Acquire)
            );
            assert_eq!(
                builtin_classes(py).int,
                SHARED_DRAIN_CLASS.load(AtomicOrdering::Acquire)
            );
            assert!(
                crate::concurrency::execution::current_thread_has_c_extension_execution_context()
            );
            let late = alloc_list(py, &[]);
            assert!(!late.is_null());
            let bits = MoltObject::from_ptr(late).bits();
            SHARED_DRAIN_LATE_BITS.store(bits, AtomicOrdering::Release);
            crate::CONTEXT_STACK.with(|stack| stack.borrow_mut().push(bits));
            // The old C dictionary is already detached. Republish both domains
            // from its finalizer coordinate, then require WorkerEnd to revisit.
            assert!(!unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() }.is_null());
            SHARED_DRAIN_CALLBACKS.fetch_add(1, AtomicOrdering::AcqRel);
        });
    }

    #[test]
    fn shared_worker_quiesces_c_to_runtime_reentry_under_exact_origin() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        SHARED_DRAIN_CALLBACKS.store(0, AtomicOrdering::Release);
        SHARED_DRAIN_LATE_BITS.store(0, AtomicOrdering::Release);
        let completed = Arc::new(AtomicBool::new(false));
        let verified = Arc::clone(&completed);
        let lifetime = RuntimeLifetimeLease::try_acquire().unwrap();
        let isolate = spawn_runtime_thread(move || {
            run_isolate_thread(
                lifetime,
                Arc::new(MoltThreadHandle::new()),
                || MoltObject::none().bits(),
                || {
                    crate::with_gil_entry_nopanic!(py, {
                        let state = runtime_state(py);
                        let identity = state.identity();
                        SHARED_DRAIN_ORIGIN.store(identity, AtomicOrdering::Release);
                        SHARED_DRAIN_CLASS.store(builtin_classes(py).int, AtomicOrdering::Release);
                        let c_records = molt_cpython_abi::api::object::runtime_retained_thread_state_count_for(identity);
                        let origin = RuntimeThreadBorrow::try_acquire(py).unwrap();
                        let worker = spawn_runtime_thread(move || {
                            run_shared_thread(Arc::new(MoltThreadHandle::new()), origin, || {
                                assert!(!unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() }.is_null());
                                molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(Some(shared_worker_c_finalizer_reentry));
                            });
                            // These observations precede every native TLS
                            // destructor and any new runtime entry boundary.
                            assert_eq!(SHARED_DRAIN_CALLBACKS.load(AtomicOrdering::Acquire), 1);
                            assert!(crate::CONTEXT_STACK.with(|stack| stack.borrow().is_empty()));
                            assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
                        }).unwrap();
                        {
                            let _release = crate::concurrency::gil::GilReleaseGuard::suspend();
                            worker.join().unwrap();
                        }
                        assert_eq!(state.thread_borrow_count(), 0);
                        assert_eq!(molt_cpython_abi::api::object::runtime_retained_thread_state_count_for(identity), c_records);
                        let late = SHARED_DRAIN_LATE_BITS.load(AtomicOrdering::Acquire);
                        assert_ne!(late, 0);
                        // Inspect live registry values; never dereference the
                        // retired pointer to manufacture the lifetime oracle.
                        let visible = crate::object::gc::get_objects(py, None).unwrap();
                        assert!(!unsafe { crate::object::seq_access::with_borrowed(visible, |items| items.contains(&late)) });
                        dec_ref_bits(py, MoltObject::from_ptr(visible).bits());
                        verified.store(true, AtomicOrdering::Release);
                    });
                },
            );
        }).unwrap();
        isolate.join().unwrap();
        assert!(
            completed.load(AtomicOrdering::Acquire),
            "caught payload assertion cannot count as completion"
        );
        molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(None);
    }

    static ISOLATE_DRAIN_ATEXIT_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    static ISOLATE_DRAIN_REENTRY_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    static ISOLATE_DRAIN_LATE_TLS_BITS: AtomicU64 = AtomicU64::new(0);
    static ISOLATE_PRESERVED_PENDING_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn isolate_shutdown_drain_reentry() {
        assert!(
            crate::concurrency::execution::current_thread_has_c_extension_execution_context(),
            "isolate shutdown reentry lost destruction execution custody"
        );
        let late_tls = ISOLATE_DRAIN_LATE_TLS_BITS.swap(0, AtomicOrdering::AcqRel);
        assert_ne!(
            late_tls, 0,
            "isolate shutdown reentry lost its late TLS owner"
        );
        crate::CONTEXT_STACK.with(|stack| stack.borrow_mut().push(late_tls));

        // The first dictionary was already detached from the record before this
        // hook ran. Publishing another one here forces the shutdown owner to
        // revisit both the Molt TLS and CPython managed-edge domains.
        let dict = unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() };
        assert!(
            !dict.is_null(),
            "isolate shutdown reentry failed to publish a fresh state dictionary"
        );
        ISOLATE_DRAIN_REENTRY_CALLBACKS.fetch_add(1, AtomicOrdering::AcqRel);
    }

    extern "C" fn publish_isolate_shutdown_edges() -> u64 {
        assert!(crate::concurrency::execution::current_thread_holds_shutdown_drain_custody());
        assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
        crate::with_gil_entry_nopanic!(py, {
            let dict = unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() };
            assert!(
                !dict.is_null(),
                "isolate atexit callback failed to publish its initial state dictionary"
            );
            let late_tls = alloc_string(py, b"isolate-shutdown-late-tls-owner");
            assert!(
                !late_tls.is_null(),
                "isolate late TLS owner allocation failed"
            );
            let previous = ISOLATE_DRAIN_LATE_TLS_BITS.swap(
                MoltObject::from_ptr(late_tls).bits(),
                AtomicOrdering::AcqRel,
            );
            assert_eq!(previous, 0, "isolate late TLS owner was already published");
            molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(Some(
                isolate_shutdown_drain_reentry,
            ));
            ISOLATE_DRAIN_ATEXIT_CALLBACKS.fetch_add(1, AtomicOrdering::AcqRel);
            MoltObject::none().bits()
        })
    }

    unsafe extern "C" fn count_isolate_preserved_pending_call(_arg: *mut c_void) -> c_int {
        ISOLATE_PRESERVED_PENDING_CALLBACKS.fetch_add(1, AtomicOrdering::AcqRel);
        0
    }

    #[cfg(not(feature = "free-threaded"))]
    static PRIMARY_GC_PHASES: AtomicUsize = AtomicUsize::new(0);
    #[cfg(not(feature = "free-threaded"))]
    static ISOLATE_GC_PHASES: AtomicUsize = AtomicUsize::new(0);

    #[cfg(not(feature = "free-threaded"))]
    extern "C" fn count_primary_gc_phase(_phase: u64, _info: u64) -> u64 {
        PRIMARY_GC_PHASES.fetch_add(1, AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    }

    #[cfg(not(feature = "free-threaded"))]
    extern "C" fn count_isolate_gc_phase(_phase: u64, _info: u64) -> u64 {
        ISOLATE_GC_PHASES.fetch_add(1, AtomicOrdering::Relaxed);
        MoltObject::none().bits()
    }

    #[cfg(not(feature = "free-threaded"))]
    fn install_gc_callback(py: &PyToken<'_>, callback: extern "C" fn(u64, u64) -> u64) {
        let callable = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(callback as *const ()),
            2,
        );
        assert!(!callable.is_null());
        let callable = MoltObject::from_ptr(callable).bits();
        let callbacks = runtime_state(py).gc.callbacks_bits(py);
        crate::molt_list_append(callbacks, callable);
        dec_ref_bits(py, callbacks);
        dec_ref_bits(py, callable);
        assert!(!exception_pending(py));
    }

    #[cfg(not(feature = "free-threaded"))]
    fn unreachable_list_cycle(py: &PyToken<'_>) -> usize {
        let pointer = crate::alloc_list(py, &[]);
        assert!(!pointer.is_null());
        let bits = MoltObject::from_ptr(pointer).bits();
        crate::molt_list_append(bits, bits);
        assert!(!exception_pending(py));
        dec_ref_bits(py, bits);
        pointer.expose_provenance()
    }

    #[test]
    #[cfg(not(feature = "free-threaded"))]
    fn isolate_gc_and_retirement_preserve_primary_heap_and_c_owners() {
        use crate::object::gc;
        use molt_cpython_abi::api::{memory, object, refcount::OwnedPyObject};
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        PRIMARY_GC_PHASES.store(0, AtomicOrdering::Relaxed);
        ISOLATE_GC_PHASES.store(0, AtomicOrdering::Relaxed);
        crate::with_gil_entry_nopanic!(py, {
            let primary = runtime_state(py);
            primary.gc.set_enabled(false);
            install_gc_callback(py, count_primary_gc_phase);
            let frozen = unreachable_list_cycle(py);
            gc::freeze_tracked_registry(py);
            let permanent_before = gc::permanent_generation_count(py);
            let ordinary = unreachable_list_cycle(py);
            let native = unsafe {
                OwnedPyObject::from_owned(memory::_PyObject_GC_New(
                    &raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError,
                ))
            };
            assert!(!native.as_ptr().is_null());
            let native_address = native.as_ptr().expose_provenance();
            let native_refs = unsafe { (*native.as_ptr()).ob_refcnt };
            let primary_c_states =
                object::runtime_retained_thread_state_count_for(primary.identity());
            assert_ne!(
                primary_c_states, 0,
                "hold a real primary PyThreadState during isolate retirement"
            );
            let counts = primary.gc.counts();
            let stats = primary.gc.generation_stats();
            let payload_verified = Arc::new(AtomicBool::new(false));
            let worker_verified = Arc::clone(&payload_verified);
            let handle = Arc::new(MoltThreadHandle::new());
            let worker_handle = Arc::clone(&handle);
            {
                let _release = crate::concurrency::gil::GilReleaseGuard::suspend();
                let lifetime =
                    RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
                std::thread::spawn(move || {
                    run_isolate_thread(
                        lifetime,
                        worker_handle,
                        || MoltObject::none().bits(),
                        || {
                            crate::with_gil_entry_nopanic!(py, {
                                runtime_state(py).gc.set_enabled(false);
                                install_gc_callback(py, count_isolate_gc_phase);
                                let allocation_count = runtime_state(py).gc.counts()[0];
                                let dictionary =
                                    crate::object::builders::alloc_dict_with_pairs(py, &[]);
                                assert!(!dictionary.is_null());
                                assert_eq!(runtime_state(py).gc.counts()[0], allocation_count + 1);
                                dec_ref_bits(py, MoltObject::from_ptr(dictionary).bits());
                                assert_eq!(runtime_state(py).gc.counts()[0], allocation_count);
                                let own = unreachable_list_cycle(py);
                                let visible =
                                    gc::get_objects(py, None).expect("isolated gc.get_objects");
                                let values = unsafe {
                                    crate::object::seq_access::with_borrowed(visible, |items| {
                                        items.to_vec()
                                    })
                                };
                                for foreign in [frozen, ordinary] {
                                    let bits = MoltObject::from_ptr(
                                        std::ptr::with_exposed_provenance_mut(foreign),
                                    )
                                    .bits();
                                    assert!(
                                        !values.contains(&bits),
                                        "gc.get_objects exposed another interpreter"
                                    );
                                }
                                let own_bits = MoltObject::from_ptr(
                                    std::ptr::with_exposed_provenance_mut(own),
                                )
                                .bits();
                                assert!(values.contains(&own_bits));
                                dec_ref_bits(py, MoltObject::from_ptr(visible).bits());
                                gc::freeze_tracked_registry(py);
                                let isolated_permanent = gc::permanent_generation_count(py);
                                assert!(isolated_permanent > 0);
                                // A second live isolate must not mutate this
                                // isolate's generation or collect its cycle.
                                let sibling_verified = Arc::new(AtomicBool::new(false));
                                let sibling_observer = Arc::clone(&sibling_verified);
                                {
                                    let _release =
                                        crate::concurrency::gil::GilReleaseGuard::suspend();
                                    let lifetime = RuntimeLifetimeLease::try_acquire()
                                        .expect("live process for isolate");
                                    std::thread::spawn(move || {
                                        run_isolate_thread(
                                            lifetime,
                                            Arc::new(MoltThreadHandle::new()),
                                            || MoltObject::none().bits(),
                                            || {
                                                crate::with_gil_entry_nopanic!(py, {
                                                    runtime_state(py).gc.set_enabled(false);
                                                    let sibling = unreachable_list_cycle(py);
                                                    gc::unfreeze_tracked_registry(py);
                                                    assert_eq!(
                                                        unsafe { gc::collect_cycles(py) }.status,
                                                        gc::GcCollectStatus::Completed
                                                    );
                                                    assert!(!unsafe {
                                                        gc::gc_is_tracked(
                                                            std::ptr::with_exposed_provenance_mut(
                                                                sibling,
                                                            ),
                                                        )
                                                    });
                                                    assert!(unsafe {
                                                        gc::gc_is_tracked(
                                                            std::ptr::with_exposed_provenance_mut(
                                                                own,
                                                            ),
                                                        )
                                                    });
                                                    sibling_observer
                                                        .store(true, AtomicOrdering::Release);
                                                });
                                            },
                                        );
                                    })
                                    .join()
                                    .unwrap();
                                }
                                assert!(sibling_verified.load(AtomicOrdering::Acquire));
                                assert_eq!(gc::permanent_generation_count(py), isolated_permanent);
                                gc::unfreeze_tracked_registry(py);
                                assert_eq!(gc::permanent_generation_count(py), 0);
                                let collected = unsafe { gc::collect_cycles(py) };
                                assert_eq!(collected.status, gc::GcCollectStatus::Completed);
                                assert!(!unsafe {
                                    gc::gc_is_tracked(std::ptr::with_exposed_provenance_mut(own))
                                });
                                assert_eq!(ISOLATE_GC_PHASES.load(AtomicOrdering::Relaxed), 2);
                                assert_eq!(PRIMARY_GC_PHASES.load(AtomicOrdering::Relaxed), 0);
                                worker_verified.store(true, AtomicOrdering::Release);
                            });
                        },
                    );
                })
                .join()
                .expect("isolate collection and retirement");
            }
            assert!(handle.done.load(AtomicOrdering::Acquire));
            assert!(
                payload_verified.load(AtomicOrdering::Acquire),
                "caught isolate payload panic cannot qualify the ownership assertions"
            );
            assert!(
                !gc::gc_has_live_isolate_owners(),
                "isolate must actually retire, not merely complete its handle after a caught panic"
            );
            assert_eq!(
                primary.gc.counts(),
                counts,
                "isolate collection charged the primary counter"
            );
            assert_eq!(primary.gc.generation_stats(), stats);
            assert_eq!(gc::permanent_generation_count(py), permanent_before);
            assert_eq!(PRIMARY_GC_PHASES.load(AtomicOrdering::Relaxed), 0);
            assert_eq!(
                object::runtime_retained_thread_state_count_for(primary.identity()),
                primary_c_states
            );
            assert!(gc::native_gc_is_enrolled(native_address));
            assert_eq!(unsafe { (*native.as_ptr()).ob_refcnt }, native_refs);
            for address in [frozen, ordinary] {
                assert!(unsafe {
                    gc::gc_is_tracked(std::ptr::with_exposed_provenance_mut(address))
                });
            }
            gc::unfreeze_tracked_registry(py);
            let collected = unsafe { gc::collect_cycles(py) };
            assert_eq!(collected.status, gc::GcCollectStatus::Completed);
            for address in [frozen, ordinary] {
                assert!(!unsafe {
                    gc::gc_is_tracked(std::ptr::with_exposed_provenance_mut(address))
                });
            }
            assert_eq!(PRIMARY_GC_PHASES.load(AtomicOrdering::Relaxed), 2);
            drop(native);
            assert!(!gc::native_gc_is_enrolled(native_address));
        });
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn failed_isolate_preserves_state_and_blocks_process_retirement() {
        const MODE: &str = "MOLT_ISOLATE_OWNER_LIFETIME_TEST";
        const TEST: &str = "concurrency::isolates::bootstrap_failure_tests::failed_isolate_preserves_state_and_blocks_process_retirement";
        if let Ok(mode) = std::env::var(MODE) {
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            let primary = crate::state::runtime_state::runtime_state_for_gil()
                .unwrap()
                .identity();
            let (send, receive) = std::sync::mpsc::channel();
            let fail = mode != "successful";
            let fail_thread_state = mode == "thread-state";
            let handle = Arc::new(MoltThreadHandle::new());
            let worker_handle = Arc::clone(&handle);
            let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
            std::thread::spawn(move || {
                let isolate_identity = std::cell::Cell::new(0);
                run_isolate_thread(
                    lifetime,
                    worker_handle,
                    || {
                        crate::with_gil_entry_nopanic!(py, {
                            isolate_identity.set(runtime_state(py).identity());
                            // Observe an existing Rust owner inside RuntimeState. Its
                            // worker is joined by normal teardown; only the state's
                            // Arc remains afterward, so Weak tests Box lifetime
                            // without dereferencing potentially retired memory.
                            send.send(Arc::downgrade(runtime_state(py).sleep_queue()))
                                .unwrap();
                            if fail && !fail_thread_state {
                                crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                            }
                            MoltObject::none().bits()
                        })
                    },
                    || {
                        if fail_thread_state {
                            // Keep a real C owner past payload execution, then
                            // fail its first shutdown drain. Thread exit must
                            // finish under the retained isolate, not primary TLS.
                            let gil = GilGuard::new();
                            let py = gil.token();
                            let owner = runtime_state(&py).identity();
                            molt_cpython_abi::api::object::attach_runtime_execution_thread(owner);
                            assert!(!unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() }.is_null());
                            molt_cpython_abi::api::object::detach_runtime_execution_thread();
                            molt_cpython_abi::api::object::inject_thread_state_finalizer_panic_for_test();
                        }
                    },
                );
                if fail {
                    let _gil = GilGuard::new();
                    assert_eq!(
                        crate::state::runtime_state::runtime_state_for_gil()
                            .unwrap()
                            .identity(),
                        isolate_identity.get(),
                        "failed isolate TLS must not rebind to the primary owner"
                    );
                }
            })
            .join()
            .unwrap();
            let retained = receive.recv().unwrap();
            assert!(handle.done.load(AtomicOrdering::Acquire));
            assert_eq!(
                retained.upgrade().is_some(),
                fail,
                "failed state storage must stay alive; successful state storage must retire"
            );
            assert_eq!(crate::object::gc::gc_has_live_isolate_owners(), fail);
            assert_eq!(
                crate::state::runtime_state::molt_runtime_shutdown(),
                u64::from(!fail)
            );
            if fail {
                assert!(retained.upgrade().is_some());
                let admitted = crate::state::runtime_state::active_runtime_execution_lease_count();
                assert!(
                    crate::test_support::catch_expected_unwind(
                        crate::state::runtime_state::molt_runtime_reset_for_testing
                    )
                    .is_err()
                );
                assert_eq!(
                    crate::state::runtime_state::active_runtime_execution_lease_count(),
                    admitted
                );
                assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
                assert_eq!(
                    crate::state::runtime_state::runtime_state_for_gil()
                        .unwrap()
                        .identity(),
                    primary,
                    "refused retirement must preserve the primary epoch"
                );
            }
            println!("isolate owner lifetime verified: {mode}");
            return;
        }
        for mode in ["successful", "failed", "thread-state"] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                .env(MODE, mode);
            let output = crate::test_support::captured_runtime_children::capture(
                &mut command,
                "isolate-owner-lifetime",
                mode,
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(0), "{mode}: {stderr}");
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("isolate owner lifetime verified: {mode}"))
            );
            if mode != "successful" {
                assert!(
                    stderr.contains("MOLT_ISOLATE_TEARDOWN_PANIC: retaining failed isolate at"),
                    "{stderr}"
                );
                assert!(
                    stderr.contains("active execution lease(s) remain live"),
                    "{stderr}"
                );
            } else {
                assert!(!stderr.contains("MOLT_ISOLATE_TEARDOWN_PANIC"), "{stderr}");
            }
        }
    }

    #[derive(Default)]
    struct IsolatedCpythonObservation {
        owns_process_state: bool,
        native_rejected: bool,
        native_runtime_error: bool,
        native_message: String,
        prepare_result: u64,
        prepare_runtime_error: bool,
        prepare_message: String,
        producer_result: u64,
        producer_runtime_error: bool,
        producer_message: String,
        retained_abi_state_after_teardown: bool,
    }

    fn pending_runtime_error(_py: &PyToken<'_>) -> (bool, String) {
        if !exception_pending(_py) {
            return (false, String::new());
        }
        let bits = crate::builtins::exceptions::molt_exception_last_pending();
        let matches = !obj_from_bits(bits).is_none()
            && crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                bits,
                "RuntimeError",
            );
        let message = obj_from_bits(bits)
            .as_ptr()
            .map(|ptr| format_exception_with_traceback(_py, ptr))
            .unwrap_or_default();
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        (matches, message)
    }

    unsafe extern "C" fn rejected_isolate_noargs(
        _self: *mut molt_cpython_abi::abi_types::PyObject,
        _args: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        std::ptr::null_mut()
    }

    fn primary_exception_bindings() -> [u64; 2] {
        let gil = GilGuard::new();
        let _py = gil.token();
        [
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError),
            (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError),
        ]
        .map(|pointer| {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(pointer.cast())
                .expect("primary static exception shell must remain bound")
                .bits()
        })
    }

    fn exercise_bootstrap(fail: bool) -> (usize, usize) {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let handle = Arc::new(MoltThreadHandle::new());
        let initializations = Arc::new(AtomicUsize::new(0));
        let payloads = Arc::new(AtomicUsize::new(0));
        let worker_handle = Arc::clone(&handle);
        let worker_initializations = Arc::clone(&initializations);
        let worker_payloads = Arc::clone(&payloads);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(
                lifetime,
                worker_handle,
                || {
                    worker_initializations.fetch_add(1, AtomicOrdering::SeqCst);
                    crate::with_gil_entry_nopanic!(_py, {
                        if fail {
                            raise_exception::<u64>(
                                _py,
                                "RuntimeError",
                                "bootstrap regression failure",
                            )
                        } else {
                            MoltObject::none().bits()
                        }
                    })
                },
                || {
                    worker_payloads.fetch_add(1, AtomicOrdering::SeqCst);
                },
            );
        })
        .join()
        .expect("bootstrap failure must not unwind past lifecycle cleanup");
        assert!(handle.done.load(AtomicOrdering::Acquire));
        assert!(handle.wait_started());
        assert!(handle.wait(Some(Duration::ZERO)));
        (
            initializations.load(AtomicOrdering::SeqCst),
            payloads.load(AtomicOrdering::SeqCst),
        )
    }

    #[test]
    fn isolate_object_and_type_metadata_use_runtime_declarations() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let handle = Arc::new(MoltThreadHandle::new());
        let worker_handle = Arc::clone(&handle);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(
                lifetime,
                worker_handle,
                || MoltObject::none().bits(),
                || {
                    crate::with_gil_entry_nopanic!(py, {
                        let roots = builtin_classes(py);
                        assert!(!crate::state::runtime_state::owns_process_cpython_state(
                            runtime_state(py)
                        ));
                        let mut owners = Vec::new();
                        let mut attribute = |value, name: &[u8]| {
                            let key = attr_name_bits_from_bytes(py, name).unwrap();
                            let result = crate::molt_get_attr_name(value, key);
                            dec_ref_bits(py, key);
                            assert!(!exception_pending(py), "isolate metadata lookup failed");
                            owners.push(result);
                            result
                        };
                        assert_eq!(
                            attribute(MoltObject::from_int(7).bits(), b"__class__"),
                            roots.int
                        );
                        assert_eq!(
                            string_obj_to_owned(obj_from_bits(attribute(roots.int, b"__name__")))
                                .as_deref(),
                            Some("int")
                        );
                        let mro = attribute(roots.int, b"__mro__");
                        assert_eq!(crate::molt_len(mro), MoltObject::from_int(2).bits());
                        let namespace = attribute(roots.type_obj, b"__dict__");
                        let get = attribute(namespace, b"get");
                        let key = attr_name_bits_from_bytes(py, b"__name__").unwrap();
                        let descriptor = unsafe { crate::call_callable1(py, get, key) };
                        assert!(!exception_pending(py));
                        let pointer = obj_from_bits(descriptor).as_ptr().unwrap();
                        assert_eq!(
                            unsafe { object_type_id(pointer) },
                            crate::TYPE_ID_NATIVE_DESCRIPTOR
                        );
                        assert_eq!(
                            unsafe { crate::object::layout::native_descriptor_flavor(pointer) },
                            Some(crate::object::layout::NativeDescriptorFlavor::RootMetadata)
                        );
                        dec_ref_bits(py, descriptor);
                        dec_ref_bits(py, key);
                        for owner in owners {
                            dec_ref_bits(py, owner);
                        }
                    });
                },
            );
        })
        .join()
        .expect("isolate metadata consumer must complete");
        assert!(handle.done.load(AtomicOrdering::Acquire));
    }

    #[test]
    fn failed_initializer_never_runs_payload_and_completes_handle() {
        assert_eq!(exercise_bootstrap(true), (1, 0));
    }

    #[test]
    fn successful_initializer_runs_payload_once_and_completes_handle() {
        assert_eq!(exercise_bootstrap(false), (1, 1));
    }

    #[test]
    fn isolate_teardown_drains_cpython_edges_published_after_the_early_worker_boundary() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        ISOLATE_DRAIN_ATEXIT_CALLBACKS.store(0, AtomicOrdering::Release);
        ISOLATE_DRAIN_REENTRY_CALLBACKS.store(0, AtomicOrdering::Release);
        ISOLATE_DRAIN_LATE_TLS_BITS.store(0, AtomicOrdering::Release);
        molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(None);
        let retained_before = molt_cpython_abi::api::object::runtime_retained_thread_state_count();

        let handle = Arc::new(MoltThreadHandle::new());
        let retained_after_teardown = Arc::new(AtomicBool::new(true));
        let worker_handle = Arc::clone(&handle);
        let worker_retained_after_teardown = Arc::clone(&retained_after_teardown);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(
                lifetime,
                worker_handle,
                || {
                    crate::with_gil_entry_nopanic!(py, {
                        let callback = crate::builtins::functions::alloc_runtime_function_obj(
                            py,
                            crate::provenance::abi::expose_function_address(
                                publish_isolate_shutdown_edges as *const (),
                            ),
                            0,
                        );
                        assert!(
                            !callback.is_null(),
                            "isolate atexit callback allocation failed"
                        );
                        let callback = MoltObject::from_ptr(callback).bits();
                        // The registry owns its reference; the result carries none.
                        assert!(
                            obj_from_bits(crate::builtins::atexit::molt_atexit_register(
                                callback,
                                MoltObject::none().bits(),
                                MoltObject::none().bits(),
                            ))
                            .is_none()
                        );
                        dec_ref_bits(py, callback);
                        assert!(!exception_pending(py));
                        MoltObject::none().bits()
                    })
                },
                || {},
            );
            worker_retained_after_teardown.store(
                molt_cpython_abi::api::object::current_thread_has_retained_runtime_state(),
                AtomicOrdering::Release,
            );
        })
        .join()
        .expect("late isolate shutdown publication must complete teardown");

        // Leave no process-global test hook behind even when an assertion below
        // reports a lifecycle regression.
        molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(None);
        assert!(handle.done.load(AtomicOrdering::Acquire));
        assert!(handle.wait_started());
        assert!(handle.wait(Some(Duration::ZERO)));
        assert_eq!(
            ISOLATE_DRAIN_ATEXIT_CALLBACKS.load(AtomicOrdering::Acquire),
            1,
            "the real isolate teardown must run the late-publication callback once"
        );
        assert_eq!(
            ISOLATE_DRAIN_REENTRY_CALLBACKS.load(AtomicOrdering::Acquire),
            1,
            "the shared shutdown drain must revisit the late CPython edge once"
        );
        assert_eq!(
            ISOLATE_DRAIN_LATE_TLS_BITS.load(AtomicOrdering::Acquire),
            0,
            "the shutdown drain never transferred the late Molt TLS owner"
        );
        assert!(
            !retained_after_teardown.load(AtomicOrdering::Acquire),
            "isolate teardown retained the native worker's late CPython state"
        );
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            retained_before,
            "late isolate shutdown publication changed the process owner count"
        );
    }

    #[test]
    fn isolate_teardown_preserves_primary_pending_call_queue_and_admission() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        ISOLATE_PRESERVED_PENDING_CALLBACKS.store(0, AtomicOrdering::Release);
        let retained_before = molt_cpython_abi::api::object::runtime_retained_thread_state_count();
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(count_isolate_preserved_pending_call),
                    std::ptr::null_mut(),
                )
            },
            0,
            "primary pending-call admission was not open before isolate teardown"
        );

        let handle = Arc::new(MoltThreadHandle::new());
        let worker_handle = Arc::clone(&handle);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(lifetime, worker_handle, || MoltObject::none().bits(), || {});
        })
        .join()
        .expect("isolate teardown must preserve primary pending-call custody");

        assert!(handle.done.load(AtomicOrdering::Acquire));
        assert_eq!(
            ISOLATE_PRESERVED_PENDING_CALLBACKS.load(AtomicOrdering::Acquire),
            0,
            "isolate teardown executed a primary-runtime pending callback"
        );
        assert!(
            molt_cpython_abi::api::pending_calls::has_pending_calls(),
            "isolate teardown discarded the primary pending-call queue"
        );
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            retained_before,
            "isolate teardown changed the primary runtime's retained-state count"
        );
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(count_isolate_preserved_pending_call),
                    std::ptr::null_mut(),
                )
            },
            0,
            "isolate teardown closed primary pending-call admission"
        );
        crate::with_gil_entry_nopanic!(_py, {
            assert_eq!(
                molt_cpython_abi::api::pending_calls::Py_MakePendingCalls(),
                0
            );
        });
        assert_eq!(
            ISOLATE_PRESERVED_PENDING_CALLBACKS.load(AtomicOrdering::Acquire),
            2,
            "the primary runtime did not execute both preserved pending callbacks"
        );
        assert!(!molt_cpython_abi::api::pending_calls::has_pending_calls());
    }

    #[test]
    fn isolated_exception_rendering_covers_every_declared_family_without_process_ownership() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let primary_bindings = primary_exception_bindings();
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        let handle = Arc::new(MoltThreadHandle::new());
        let worker_handle = Arc::clone(&handle);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(
                lifetime,
                worker_handle,
                || {
                    crate::with_gil_entry_nopanic!(py, {
                        assert!(!crate::state::runtime_state::owns_process_cpython_state(
                            runtime_state(py)
                        ));
                        crate::builtins::exceptions::assert_managed_rendering_contract(py);
                        assert!(!crate::state::runtime_state::owns_process_cpython_state(
                            runtime_state(py)
                        ));
                        MoltObject::none().bits()
                    })
                },
                || {
                    worker_completed.store(true, AtomicOrdering::Release);
                },
            );
        })
        .join()
        .expect("isolate rendering worker must finish");
        assert!(
            completed.load(AtomicOrdering::Acquire),
            "caught fixture failures cannot count as completion"
        );
        assert!(handle.done.load(AtomicOrdering::Acquire));
        assert_eq!(primary_exception_bindings(), primary_bindings);
    }

    #[test]
    fn isolated_runtime_rejects_process_static_cpython_ownership() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let primary_bindings = primary_exception_bindings();
        let retained_before = molt_cpython_abi::api::object::runtime_retained_thread_state_count();

        let handle = Arc::new(MoltThreadHandle::new());
        let payloads = Arc::new(AtomicUsize::new(0));
        let observation = Arc::new(Mutex::new(IsolatedCpythonObservation::default()));
        let worker_handle = Arc::clone(&handle);
        let worker_payloads = Arc::clone(&payloads);
        let worker_observation = Arc::clone(&observation);
        let lifetime = RuntimeLifetimeLease::try_acquire().expect("live process for isolate");
        std::thread::spawn(move || {
            run_isolate_thread(
                lifetime,
                worker_handle,
                || {
                    crate::with_gil_entry_nopanic!(_py, {
                        let owns_process_state =
                            crate::state::runtime_state::owns_process_cpython_state(
                                crate::runtime_state(_py),
                            );
                        // A real native GC constructor must reject this owner
                        // before registering any process-shell allocation.
                        let native = unsafe {
                            molt_cpython_abi::api::memory::_PyObject_GC_New(
                                &raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError,
                            )
                        };
                        let native_rejected = native.is_null();
                        let (native_runtime_error, native_message) = pending_runtime_error(_py);
                        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
                        let _ = molt_exception_clear();
                        if !native.is_null() {
                            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(native) };
                        }
                        // Admission must reject the isolate before touching a
                        // callback address or allocating initializer state.
                        let prepare_result = crate::molt_cpython_abi_run_static_extension_init(
                            0,
                            MoltObject::none().bits(),
                        );
                        let (prepare_runtime_error, prepare_message) = pending_runtime_error(_py);
                        let _ = molt_exception_clear();

                        let name = b"rejected_isolate_callable";
                        let producer_result = unsafe {
                            molt_cpython_abi::hooks::hooks_or_stubs().register_c_function(
                                rejected_isolate_noargs as *const () as usize as u64,
                                molt_cpython_abi::abi_types::METH_NOARGS,
                                MoltObject::none().bits(),
                                true,
                                MoltObject::none().bits(),
                                name.as_ptr(),
                                name.len(),
                            )
                        };
                        let (producer_runtime_error, producer_message) = pending_runtime_error(_py);
                        *worker_observation.lock().unwrap() = IsolatedCpythonObservation {
                            owns_process_state,
                            native_rejected,
                            native_runtime_error,
                            native_message,
                            prepare_result,
                            prepare_runtime_error,
                            prepare_message,
                            producer_result,
                            producer_runtime_error,
                            producer_message,
                            retained_abi_state_after_teardown: false,
                        };
                        // Leave the producer's RuntimeError pending. The real
                        // isolate lifecycle must classify bootstrap as failed,
                        // skip its payload, report the error, and still tear
                        // down the isolate completely.
                        MoltObject::none().bits()
                    })
                },
                || {
                    worker_payloads.fetch_add(1, AtomicOrdering::SeqCst);
                },
            );
            worker_observation
                .lock()
                .unwrap()
                .retained_abi_state_after_teardown =
                molt_cpython_abi::api::object::current_thread_has_retained_runtime_state();
        })
        .join()
        .expect("rejected isolate C-extension admission must complete teardown");

        assert!(handle.done.load(AtomicOrdering::Acquire));
        assert!(handle.wait_started());
        assert!(handle.wait(Some(Duration::ZERO)));
        assert_eq!(
            payloads.load(AtomicOrdering::SeqCst),
            0,
            "rejected C-extension bootstrap must never execute the isolate payload"
        );
        let observed = observation.lock().unwrap();
        assert!(
            observed.native_rejected,
            "isolate admitted a process-native GC allocation"
        );
        assert!(
            observed.native_runtime_error && observed.native_message.contains("isolated runtime"),
            "native admission lost its RuntimeError: {}",
            observed.native_message
        );
        assert!(!observed.owns_process_state);
        assert_eq!(observed.prepare_result, MoltObject::none().bits());
        assert!(observed.prepare_runtime_error);
        assert!(
            observed
                .prepare_message
                .contains("process-static C extensions")
                && observed.prepare_message.contains("isolated runtime"),
            "unexpected prepare rejection: {}",
            observed.prepare_message
        );
        assert_eq!(
            observed.producer_result, 0,
            "isolate must not publish executable C-callable ownership"
        );
        assert!(observed.producer_runtime_error);
        assert!(
            observed
                .producer_message
                .contains("process-static C extensions")
                && observed.producer_message.contains("isolated runtime"),
            "unexpected producer rejection: {}",
            observed.producer_message
        );
        assert!(
            !observed.retained_abi_state_after_teardown,
            "rejected isolate teardown retained its native thread's CPython state"
        );
        drop(observed);

        assert_eq!(
            primary_exception_bindings(),
            primary_bindings,
            "isolate teardown must not detach or replace primary static exception bindings"
        );
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            retained_before,
            "rejected isolate admission leaked retained CPython thread state"
        );
        assert!(
            crate::cpython_abi_hooks::register_cpython_hooks(),
            "primary C-extension admission must remain ready after isolate teardown"
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main_shared(
    payload: Vec<u8>,
    handle: Arc<MoltThreadHandle>,
    origin: RuntimeThreadBorrow,
) {
    run_shared_thread(handle, origin, || run_thread_payload(payload));
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main_shared_call(handle: Arc<MoltThreadHandle>, origin: RuntimeThreadBorrow) {
    let call_owner = Arc::clone(&handle);
    run_shared_thread(handle, origin, || {
        crate::with_gil_entry_nopanic!(py, {
            run_thread_payload_shared(py, &call_owner);
        });
    });
}

/// Shared bytes and handle-owned callable workers have the same origin lifetime and
/// cleanup owner. The payload panic is caught before dropping that owner, so a
/// successfully cleaned panic does not permanently retain its borrowed heap.
#[cfg(not(target_arch = "wasm32"))]
fn run_shared_thread(
    handle: Arc<MoltThreadHandle>,
    origin: RuntimeThreadBorrow,
    payload: impl FnOnce(),
) {
    let origin = std::mem::ManuallyDrop::new(origin);
    // A failed finalizer retains the same payload/handle together with its
    // origin reservation; no token table or later owner can steal that call.
    let handle = std::mem::ManuallyDrop::new(handle);
    let thread_id = current_thread_id();
    handle.mark_started(thread_id, thread_id);
    set_thread_runtime_state(origin.state_ptr());
    touch_tls_guard();
    let cleaned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let payload_failed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(payload)).is_err();
        handle.release_call(&execution.token());
        drop(execution);
        payload_failed
    }));
    match cleaned {
        Ok(payload_failed) => {
            clear_thread_runtime_state();
            drop(std::mem::ManuallyDrop::into_inner(origin));
            if payload_failed {
                eprintln!("MOLT_SHARED_THREAD_PAYLOAD_PANIC: payload unwound; owners retired");
            }
            handle.mark_done();
            drop(std::mem::ManuallyDrop::into_inner(handle));
        }
        Err(_) => {
            eprintln!(
                "MOLT_SHARED_THREAD_CLEANUP_PANIC: retaining origin runtime and process custody"
            );
            handle.mark_done();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_runtime_thread(
    run: impl FnOnce() + Send + 'static,
) -> std::io::Result<thread::JoinHandle<()>> {
    #[cfg(test)]
    if NATIVE_THREAD_SPAWN_TEST_FAIL.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other(
            "injected native thread creation failure",
        ));
    }
    let configured = THREAD_STACK_SIZE_BYTES.load(AtomicOrdering::Acquire);
    let builder = if configured == 0 {
        thread::Builder::new()
    } else {
        thread::Builder::new().stack_size(configured)
    };
    builder.spawn(run)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
thread_local! {
    static NATIVE_THREAD_SPAWN_TEST_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configured_thread_stack_size() -> Option<usize> {
    let configured = THREAD_STACK_SIZE_BYTES.load(AtomicOrdering::Acquire);
    if configured == 0 {
        None
    } else {
        Some(configured)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `payload_bits` must reference a valid thread payload tuple allocated by this runtime.
pub unsafe extern "C" fn molt_thread_spawn(payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let isolated_override = crate::runtime_state(_py).policy().thread_isolated;
        let operation = if isolated_override {
            crate::OperationId::ThreadSpawnIsolated
        } else {
            crate::OperationId::ThreadSpawn
        };
        if let Err(err) = crate::require_operation(_py, operation, AuditArgs::None) {
            return err;
        }
        // Shared runtime is the CPython-parity default. Isolated execution is
        // selected explicitly rather than emerging as a missing-grant fallback.
        let shared_runtime = !isolated_override;
        let payload = match payload_from_bits(_py, payload_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let handle = Arc::new(MoltThreadHandle::new());
        let thread_handle = handle.clone();
        let join = if shared_runtime {
            let Some(origin) = RuntimeThreadBorrow::try_acquire(_py) else {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "runtime is closing thread admission",
                );
            };
            match spawn_runtime_thread(move || thread_main_shared(payload, thread_handle, origin)) {
                Ok(handle) => handle,
                Err(err) => {
                    let msg = format!("thread spawn failed: {err}");
                    return raise_exception::<_>(_py, "RuntimeError", &msg);
                }
            }
        } else {
            let Some(lifetime) = RuntimeLifetimeLease::try_acquire() else {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "runtime is closing thread admission",
                );
            };
            match spawn_runtime_thread(move || thread_main(payload, thread_handle, lifetime)) {
                Ok(handle) => handle,
                Err(err) => {
                    let msg = format!("thread spawn failed: {err}");
                    return raise_exception::<_>(_py, "RuntimeError", &msg);
                }
            }
        };
        handle.set_join_handle(join);
        let handle_bits = native_handle_new(_py, handle);
        if handle_bits == 0 {
            return MoltObject::none().bits();
        }
        handle_bits
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `callable_bits`/`args_bits`/`kwargs_bits` must be valid Molt objects owned by
/// the current runtime. The returned native handle is the sole call identity.
pub unsafe extern "C" fn molt_thread_spawn_shared(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::ThreadSpawnShared, AuditArgs::None)
        {
            return err;
        }
        let isolated_override = crate::runtime_state(_py).policy().thread_isolated;
        if isolated_override {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "shared thread runtime unavailable in isolated mode",
            );
        }
        let Some(origin) = RuntimeThreadBorrow::try_acquire(_py) else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "runtime is closing thread admission",
            );
        };
        let handle = Arc::new(MoltThreadHandle::new());
        handle.publish_call(_py, callable_bits, args_bits, kwargs_bits);
        let thread_handle = handle.clone();
        let join =
            match spawn_runtime_thread(move || thread_main_shared_call(thread_handle, origin)) {
                Ok(handle) => handle,
                Err(err) => {
                    handle.release_call(_py);
                    let msg = format!("thread spawn failed: {err}");
                    return raise_exception::<_>(_py, "RuntimeError", &msg);
                }
            };
        handle.set_join_handle(join);
        let handle_bits = native_handle_new(_py, handle);
        if handle_bits == 0 {
            return MoltObject::none().bits();
        }
        handle_bits
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by `molt_thread_spawn` and `timeout_bits`
/// must be either `None` or a numeric timeout object.
pub unsafe extern "C" fn molt_thread_join(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        let timeout = match crate::builtins::threading_helpers::parse_thread_timeout(
            _py,
            timeout_bits,
            crate::builtins::threading_helpers::ThreadTimeoutPolicy::Join,
        ) {
            Ok(timeout) => timeout,
            Err(bits) => return bits,
        };
        let _release = crate::concurrency::GilReleaseGuard::suspend();
        if handle.wait(timeout) {
            handle.join();
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by this runtime.
pub unsafe extern "C" fn molt_thread_is_alive(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        let alive = !handle.done.load(AtomicOrdering::Acquire);
        MoltObject::from_bool(alive).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by this runtime.
pub unsafe extern "C" fn molt_thread_ident(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        if !handle.wait_started() {
            return MoltObject::none().bits();
        }
        let ident = handle.ident.load(AtomicOrdering::Acquire);
        if ident == 0 {
            MoltObject::none().bits()
        } else {
            MoltObject::from_int(ident as i64).bits()
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by this runtime.
pub unsafe extern "C" fn molt_thread_native_id(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        if !handle.wait_started() {
            return MoltObject::none().bits();
        }
        let ident = handle.native_id.load(AtomicOrdering::Acquire);
        if ident == 0 {
            MoltObject::none().bits()
        } else {
            MoltObject::from_int(ident as i64).bits()
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_current_ident() -> u64 {
    let ident = current_thread_id();
    MoltObject::from_int(ident as i64).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_current_native_id() -> u64 {
    let ident = current_thread_id();
    MoltObject::from_int(ident as i64).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// This function must be called through the runtime FFI entrypoint while runtime state is valid.
pub unsafe extern "C" fn molt_thread_stack_size_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let size = THREAD_STACK_SIZE_BYTES.load(AtomicOrdering::Acquire);
        MoltObject::from_int(size as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `size_bits` must be an integer object representing the requested stack size.
pub unsafe extern "C" fn molt_thread_stack_size_set(size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(size_i64) = to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "size must be 0 or a positive integer");
        };
        if size_i64 < 0 {
            return raise_exception::<_>(_py, "ValueError", "size must be 0 or a positive integer");
        }
        let size = size_i64 as usize;
        if size != 0 && size < THREAD_STACK_SIZE_MIN {
            let msg = format!("size not valid: {size} bytes");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let prev = THREAD_STACK_SIZE_BYTES.swap(size, AtomicOrdering::AcqRel);
        MoltObject::from_int(prev as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by this runtime and not already dropped.
pub unsafe extern "C" fn molt_thread_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if thread_handle_from_bits(handle_bits).is_none() {
            return raise_exception::<_>(_py, "TypeError", "invalid thread handle");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `name_bits` must be a string object and `daemon_bits` must be a truthy-capable object.
pub unsafe extern "C" fn molt_thread_registry_set_main(name_bits: u64, daemon_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread name must be str");
        };
        let daemon = is_truthy(_py, obj_from_bits(daemon_bits));
        thread_registry_set_main(_py, name, daemon);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle owned by this runtime and
/// `name_bits` a string object owned by this runtime.
pub unsafe extern "C" fn molt_thread_registry_register(
    handle_bits: u64,
    name_bits: u64,
    daemon_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid thread handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread name must be str");
        };
        let daemon = is_truthy(_py, obj_from_bits(daemon_bits));
        thread_registry_register(_py, &handle, name, daemon);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live native thread handle owned by this runtime.
/// Forgetting display metadata does not cancel an admitted worker or its call.
pub unsafe extern "C" fn molt_thread_registry_forget(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid thread handle");
        };
        thread_registry_forget(_py, &handle);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// This function must be called through the runtime FFI entrypoint while the thread registry is
/// initialized for the current runtime.
pub unsafe extern "C" fn molt_thread_registry_snapshot() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut tuple_bits = Vec::new();
        {
            let mut registry = thread_registry(_py).lock().unwrap();
            if registry.main_ident == 0 {
                registry.main_ident = current_thread_id();
            }
            if registry.main_name.is_none() {
                registry.main_name = Some("MainThread".to_string());
            }
            thread_registry_cleanup_locked(&mut registry);

            let main_name = registry.main_name.as_deref().unwrap_or("MainThread");
            let main_tuple = thread_record_tuple(
                _py,
                main_name,
                registry.main_daemon,
                registry.main_ident,
                registry.main_ident,
                true,
                true,
            );
            if !obj_from_bits(main_tuple).is_none() {
                tuple_bits.push(main_tuple);
            }

            for entry in registry.entries.values() {
                let Some(handle) = entry.handle.upgrade() else {
                    continue;
                };
                let alive = !handle.done.load(AtomicOrdering::Acquire);
                if !alive {
                    continue;
                }
                let ident = handle.ident.load(AtomicOrdering::Acquire);
                let native_id = handle.native_id.load(AtomicOrdering::Acquire);
                let record = thread_record_tuple(
                    _py,
                    entry.name.as_str(),
                    entry.daemon,
                    ident,
                    native_id,
                    true,
                    false,
                );
                if !obj_from_bits(record).is_none() {
                    tuple_bits.push(record);
                }
            }
        }
        let list_ptr = alloc_list(_py, tuple_bits.as_slice());
        for bits in tuple_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// This function must be called through the runtime FFI entrypoint while the thread registry is
/// initialized for the current runtime.
pub unsafe extern "C" fn molt_thread_registry_current() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let current_ident = current_thread_id();
        let mut fallback = (
            "MainThread".to_string(),
            false,
            current_ident,
            current_ident,
            true,
            true,
        );
        {
            let mut registry = thread_registry(_py).lock().unwrap();
            if registry.main_ident == 0 {
                registry.main_ident = current_ident;
            }
            if registry.main_name.is_none() {
                registry.main_name = Some("MainThread".to_string());
            }
            thread_registry_cleanup_locked(&mut registry);

            if current_ident == registry.main_ident {
                let name = registry.main_name.as_deref().unwrap_or("MainThread");
                fallback = (
                    name.to_string(),
                    registry.main_daemon,
                    current_ident,
                    current_ident,
                    true,
                    true,
                );
            } else {
                for entry in registry.entries.values() {
                    let Some(handle) = entry.handle.upgrade() else {
                        continue;
                    };
                    if handle.done.load(AtomicOrdering::Acquire) {
                        continue;
                    }
                    let ident = handle.ident.load(AtomicOrdering::Acquire);
                    if ident != current_ident {
                        continue;
                    }
                    let native_id = handle.native_id.load(AtomicOrdering::Acquire);
                    fallback = (
                        entry.name.clone(),
                        entry.daemon,
                        ident,
                        native_id,
                        true,
                        false,
                    );
                    break;
                }
            }
        }
        thread_record_tuple(
            _py,
            fallback.0.as_str(),
            fallback.1,
            fallback.2,
            fallback.3,
            fallback.4,
            fallback.5,
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// This function must be called through the runtime FFI entrypoint while the thread registry is
/// initialized for the current runtime.
pub unsafe extern "C" fn molt_thread_registry_active_count() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let current_ident = current_thread_id();
        let mut count: usize = 1;
        {
            let mut registry = thread_registry(_py).lock().unwrap();
            if registry.main_ident == 0 {
                registry.main_ident = current_ident;
            }
            if registry.main_name.is_none() {
                registry.main_name = Some("MainThread".to_string());
            }
            thread_registry_cleanup_locked(&mut registry);
            for entry in registry.entries.values() {
                let Some(handle) = entry.handle.upgrade() else {
                    continue;
                };
                if !handle.done.load(AtomicOrdering::Acquire) {
                    count += 1;
                }
            }
        }
        MoltObject::from_int(count as i64).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_set_main(_name_bits: u64, _daemon_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_register(
    _handle_bits: u64,
    _name_bits: u64,
    _daemon_bits: u64,
) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_forget(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_snapshot() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(_py, &[]);
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_current() -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
const WASM_MAIN_THREAD_IDENT: i64 = 1;

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_active_count() -> u64 {
    MoltObject::from_int(WASM_MAIN_THREAD_IDENT).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_spawn(_payload_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "threads are unavailable in wasm",
        )
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_spawn_shared(
    _callable_bits: u64,
    _args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "threads are unavailable in wasm",
        )
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_join(_handle_bits: u64, _timeout_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_is_alive(_handle_bits: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_ident(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_native_id(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_current_ident() -> u64 {
    MoltObject::from_int(WASM_MAIN_THREAD_IDENT).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_current_native_id() -> u64 {
    // WASM has no OS TID, but CPython's API contract is a stable non-zero int.
    // Use the same single-thread identity authority as get_ident().
    MoltObject::from_int(WASM_MAIN_THREAD_IDENT).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_stack_size_get() -> u64 {
    MoltObject::from_int(0).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_stack_size_set(_size_bits: u64) -> u64 {
    MoltObject::from_int(0).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_drop(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}
