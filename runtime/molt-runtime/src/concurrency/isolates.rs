use crate::{MoltObject, PyToken, alloc_list, raise_exception};

#[cfg(not(target_arch = "wasm32"))]
use super::current_thread_id;
#[cfg(not(target_arch = "wasm32"))]
use once_cell::sync::Lazy;
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
use crate::builtins::attr::attr_name_bits_from_bytes;
#[cfg(not(target_arch = "wasm32"))]
use crate::builtins::modules::molt_module_cache_get;
#[cfg(not(target_arch = "wasm32"))]
use crate::call::dispatch::call_callable1;
#[cfg(not(target_arch = "wasm32"))]
use crate::state::{
    RuntimeState, clear_thread_runtime_state, runtime_reset_for_init, runtime_teardown_isolate,
    set_thread_runtime_state, touch_tls_guard,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    TYPE_ID_BYTES, alloc_bytes, alloc_string, alloc_tuple, bits_from_ptr, bytes_data, bytes_len,
    dec_ref_bits, exception_pending, format_exception_with_traceback, has_capability, inc_ref_bits,
    is_truthy, molt_exception_clear, molt_exception_last, molt_module_get_attr, obj_from_bits,
    object_type_id, ptr_from_bits, release_ptr, string_obj_to_owned, to_i64,
};

#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" {
    fn molt_isolate_bootstrap() -> u64;
    fn molt_isolate_import(name_bits: u64) -> u64;
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct MoltThreadHandle {
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
            done: AtomicBool::new(false),
            ident: AtomicU64::new(0),
            native_id: AtomicU64::new(0),
            join_handle: Mutex::new(None),
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    fn set_join_handle(&self, handle: thread::JoinHandle<()>) {
        let mut guard = self.join_handle.lock().unwrap();
        *guard = Some(handle);
    }

    fn mark_started(&self, ident: u64, native_id: u64) {
        self.ident.store(ident, AtomicOrdering::Release);
        self.native_id.store(native_id, AtomicOrdering::Release);
    }

    fn mark_done(&self) {
        self.done.store(true, AtomicOrdering::Release);
        self.condvar.notify_all();
    }

    fn wait(&self, timeout: Option<Duration>) -> bool {
        if self.done.load(AtomicOrdering::Acquire) {
            return true;
        }
        let guard = self.wait_lock.lock().unwrap();
        if self.done.load(AtomicOrdering::Acquire) {
            return true;
        }
        match timeout {
            Some(wait) => {
                let (_guard, _) = self.condvar.wait_timeout(guard, wait).unwrap();
            }
            None => {
                let _guard = self.condvar.wait(guard).unwrap();
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
struct ThreadRegistry {
    main_name: Option<String>,
    main_daemon: bool,
    main_ident: u64,
    entries: HashMap<u64, ThreadRegistryEntry>,
}

#[cfg(not(target_arch = "wasm32"))]
static THREAD_REGISTRY: Lazy<Mutex<ThreadRegistry>> =
    Lazy::new(|| Mutex::new(ThreadRegistry::default()));

#[cfg(not(target_arch = "wasm32"))]
struct SharedThreadCall {
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
}

#[cfg(not(target_arch = "wasm32"))]
static SHARED_THREAD_CALLS: Lazy<Mutex<HashMap<u64, SharedThreadCall>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[cfg(not(target_arch = "wasm32"))]
static THREAD_STACK_SIZE_BYTES: AtomicUsize = AtomicUsize::new(0);

#[cfg(not(target_arch = "wasm32"))]
const THREAD_STACK_SIZE_MIN: usize = 32 * 1024;

#[cfg(not(target_arch = "wasm32"))]
fn thread_handle_from_bits(bits: u64) -> Option<Arc<MoltThreadHandle>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltThreadHandle);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_set_main(name: String, daemon: bool) {
    let ident = current_thread_id();
    let mut registry = THREAD_REGISTRY.lock().unwrap();
    registry.main_name = Some(name);
    registry.main_daemon = daemon;
    registry.main_ident = ident;
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_register(
    token: u64,
    handle: &Arc<MoltThreadHandle>,
    name: String,
    daemon: bool,
) {
    let mut registry = THREAD_REGISTRY.lock().unwrap();
    registry.entries.insert(
        token,
        ThreadRegistryEntry {
            handle: Arc::downgrade(handle),
            name,
            daemon,
        },
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_registry_forget(token: u64) {
    let mut registry = THREAD_REGISTRY.lock().unwrap();
    registry.entries.remove(&token);
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
fn shared_thread_call_replace(
    _py: &PyToken<'_>,
    token: u64,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) {
    inc_ref_bits(_py, callable_bits);
    if !obj_from_bits(args_bits).is_none() {
        inc_ref_bits(_py, args_bits);
    }
    if !obj_from_bits(kwargs_bits).is_none() {
        inc_ref_bits(_py, kwargs_bits);
    }
    let mut calls = SHARED_THREAD_CALLS.lock().unwrap();
    if let Some(old) = calls.insert(
        token,
        SharedThreadCall {
            callable_bits,
            args_bits,
            kwargs_bits,
        },
    ) {
        dec_ref_bits(_py, old.callable_bits);
        if !obj_from_bits(old.args_bits).is_none() {
            dec_ref_bits(_py, old.args_bits);
        }
        if !obj_from_bits(old.kwargs_bits).is_none() {
            dec_ref_bits(_py, old.kwargs_bits);
        }
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
    let exc_bits = molt_exception_last();
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
    crate::with_gil_entry!(_py, {
        let module_ptr = alloc_string(_py, b"threading");
        if module_ptr.is_null() {
            return;
        }
        let module_bits = MoltObject::from_ptr(module_ptr).bits();
        let mut loaded_bits = molt_module_cache_get(module_bits);
        if obj_from_bits(loaded_bits).is_none() {
            loaded_bits = unsafe { molt_isolate_import(module_bits) };
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
        let _ = unsafe { call_callable1(_py, func_bits, payload_bits) };
        dec_ref_bits(_py, payload_bits);
        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, loaded_bits);
        log_thread_exception(_py);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn run_thread_payload_shared(token: u64) {
    crate::with_gil_entry!(_py, {
        let entry = {
            let mut calls = SHARED_THREAD_CALLS.lock().unwrap();
            calls.remove(&token)
        };
        let Some(entry) = entry else {
            let msg = format!("missing shared thread payload for token {token}");
            let _ = raise_exception::<u64>(_py, "RuntimeError", &msg);
            log_thread_exception(_py);
            return;
        };
        let result_bits =
            call_thread_callable(_py, entry.callable_bits, entry.args_bits, entry.kwargs_bits);
        if !obj_from_bits(result_bits).is_none() {
            dec_ref_bits(_py, result_bits);
        }
        dec_ref_bits(_py, entry.callable_bits);
        if !obj_from_bits(entry.args_bits).is_none() {
            dec_ref_bits(_py, entry.args_bits);
        }
        if !obj_from_bits(entry.kwargs_bits).is_none() {
            dec_ref_bits(_py, entry.kwargs_bits);
        }
        log_thread_exception(_py);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main(payload: Vec<u8>, handle: Arc<MoltThreadHandle>) {
    let thread_id = current_thread_id();
    handle.mark_started(thread_id, thread_id);
    let state = Box::new(RuntimeState::new());
    let state_ptr = Box::into_raw(state);
    set_thread_runtime_state(state_ptr);
    touch_tls_guard();
    let setup = std::panic::catch_unwind(|| {
        let gil = GilGuard::new();
        let py = gil.token();
        runtime_reset_for_init(&py, unsafe { &*state_ptr });
    });
    if setup.is_ok() {
        crate::with_gil_entry!(_py, {
            unsafe {
                let _ = molt_isolate_bootstrap();
            }
            log_thread_exception(_py);
        });
        run_thread_payload(payload);
    }
    let _ = std::panic::catch_unwind(|| {
        let gil = GilGuard::new();
        let py = gil.token();
        runtime_teardown_isolate(&py, unsafe { &*state_ptr });
    });
    clear_thread_runtime_state();
    unsafe {
        drop(Box::from_raw(state_ptr));
    }
    handle.mark_done();
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main_shared(payload: Vec<u8>, handle: Arc<MoltThreadHandle>, state_ptr: usize) {
    let thread_id = current_thread_id();
    handle.mark_started(thread_id, thread_id);
    let state_ptr = state_ptr as *mut RuntimeState;
    set_thread_runtime_state(state_ptr);
    touch_tls_guard();
    run_thread_payload(payload);
    clear_thread_runtime_state();
    handle.mark_done();
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_main_shared_token(token: u64, handle: Arc<MoltThreadHandle>, state_ptr: usize) {
    let thread_id = current_thread_id();
    handle.mark_started(thread_id, thread_id);
    let state_ptr = state_ptr as *mut RuntimeState;
    set_thread_runtime_state(state_ptr);
    touch_tls_guard();
    run_thread_payload_shared(token);
    clear_thread_runtime_state();
    handle.mark_done();
}

#[cfg(not(target_arch = "wasm32"))]
fn configured_thread_builder() -> thread::Builder {
    let configured = THREAD_STACK_SIZE_BYTES.load(AtomicOrdering::Acquire);
    if configured == 0 {
        thread::Builder::new()
    } else {
        thread::Builder::new().stack_size(configured)
    }
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
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "thread") && !has_capability(_py, "thread.spawn") {
            return raise_exception::<_>(_py, "PermissionError", "missing thread capability");
        }
        let isolated_override = matches!(
            std::env::var("MOLT_THREAD_ISOLATED")
                .ok()
                .as_deref()
                .map(|value| value.to_ascii_lowercase()),
            Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on")
        );
        // Default to shared-runtime threads for CPython parity of thread-visible
        // global/module state; keep an escape hatch for isolate-only mode.
        let shared_runtime = !isolated_override
            && (has_capability(_py, "thread.shared")
                || has_capability(_py, "thread")
                || has_capability(_py, "thread.spawn"));
        let payload = match payload_from_bits(_py, payload_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let handle = Arc::new(MoltThreadHandle::new());
        let thread_handle = handle.clone();
        let join = if shared_runtime {
            let state_ptr =
                crate::state::runtime_state::runtime_state(_py) as *const RuntimeState as usize;
            match configured_thread_builder()
                .spawn(move || thread_main_shared(payload, thread_handle, state_ptr))
            {
                Ok(handle) => handle,
                Err(err) => {
                    let msg = format!("thread spawn failed: {err}");
                    return raise_exception::<_>(_py, "RuntimeError", &msg);
                }
            }
        } else {
            match configured_thread_builder().spawn(move || thread_main(payload, thread_handle)) {
                Ok(handle) => handle,
                Err(err) => {
                    let msg = format!("thread spawn failed: {err}");
                    return raise_exception::<_>(_py, "RuntimeError", &msg);
                }
            }
        };
        handle.set_join_handle(join);
        let raw = Arc::into_raw(handle) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `token_bits` must be an integer object and `callable_bits`/`args_bits`/`kwargs_bits` must be
/// valid Molt objects owned by the current runtime.
pub unsafe extern "C" fn molt_thread_spawn_shared(
    token_bits: u64,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "thread") && !has_capability(_py, "thread.spawn") {
            return raise_exception::<_>(_py, "PermissionError", "missing thread capability");
        }
        let isolated_override = matches!(
            std::env::var("MOLT_THREAD_ISOLATED")
                .ok()
                .as_deref()
                .map(|value| value.to_ascii_lowercase()),
            Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on")
        );
        if isolated_override {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "shared thread runtime unavailable in isolated mode",
            );
        }
        let Some(token) = crate::to_i64(obj_from_bits(token_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread token must be an integer");
        };
        if token < 0 {
            return raise_exception::<_>(_py, "ValueError", "thread token must be >= 0");
        }
        let token_u64 = token as u64;
        shared_thread_call_replace(_py, token_u64, callable_bits, args_bits, kwargs_bits);
        let handle = Arc::new(MoltThreadHandle::new());
        let thread_handle = handle.clone();
        let state_ptr =
            crate::state::runtime_state::runtime_state(_py) as *const RuntimeState as usize;
        let join = match configured_thread_builder()
            .spawn(move || thread_main_shared_token(token_u64, thread_handle, state_ptr))
        {
            Ok(handle) => handle,
            Err(err) => {
                let entry = {
                    let mut calls = SHARED_THREAD_CALLS.lock().unwrap();
                    calls.remove(&token_u64)
                };
                if let Some(entry) = entry {
                    dec_ref_bits(_py, entry.callable_bits);
                    if !obj_from_bits(entry.args_bits).is_none() {
                        dec_ref_bits(_py, entry.args_bits);
                    }
                    if !obj_from_bits(entry.kwargs_bits).is_none() {
                        dec_ref_bits(_py, entry.kwargs_bits);
                    }
                }
                let msg = format!("thread spawn failed: {err}");
                return raise_exception::<_>(_py, "RuntimeError", &msg);
            }
        };
        handle.set_join_handle(join);
        let raw = Arc::into_raw(handle) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle created by `molt_thread_spawn` and `timeout_bits`
/// must be either `None` or a numeric timeout object.
pub unsafe extern "C" fn molt_thread_join(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            match crate::to_f64(obj_from_bits(timeout_bits)) {
                Some(val) if val > 0.0 => Some(Duration::from_secs_f64(val)),
                _ => Some(Duration::from_secs(0)),
            }
        };
        let _release = crate::concurrency::GilReleaseGuard::new();
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return MoltObject::none().bits();
        };
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
    crate::with_gil_entry!(_py, {
        let size = THREAD_STACK_SIZE_BYTES.load(AtomicOrdering::Acquire);
        MoltObject::from_int(size as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `size_bits` must be an integer object representing the requested stack size.
pub unsafe extern "C" fn molt_thread_stack_size_set(size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    unsafe {
        crate::with_gil_entry!(_py, {
            let ptr = ptr_from_bits(handle_bits);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            release_ptr(ptr);
            let _ = Arc::from_raw(ptr as *const MoltThreadHandle);
            MoltObject::none().bits()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `name_bits` must be a string object and `daemon_bits` must be a truthy-capable object.
pub unsafe extern "C" fn molt_thread_registry_set_main(name_bits: u64, daemon_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread name must be str");
        };
        let daemon = is_truthy(_py, obj_from_bits(daemon_bits));
        thread_registry_set_main(name, daemon);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `handle_bits` must be a live thread handle, `token_bits` an integer object, and `name_bits`
/// a string object owned by this runtime.
pub unsafe extern "C" fn molt_thread_registry_register(
    handle_bits: u64,
    token_bits: u64,
    name_bits: u64,
    daemon_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = thread_handle_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid thread handle");
        };
        let Some(token_i64) = to_i64(obj_from_bits(token_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread token must be an integer");
        };
        if token_i64 < 0 {
            return raise_exception::<_>(_py, "ValueError", "thread token must be >= 0");
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread name must be str");
        };
        let daemon = is_truthy(_py, obj_from_bits(daemon_bits));
        thread_registry_register(token_i64 as u64, &handle, name, daemon);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// `token_bits` must be an integer object produced by this runtime.
pub unsafe extern "C" fn molt_thread_registry_forget(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(token_i64) = to_i64(obj_from_bits(token_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "thread token must be an integer");
        };
        if token_i64 >= 0 {
            let token = token_i64 as u64;
            thread_registry_forget(token);
            let entry = {
                let mut calls = SHARED_THREAD_CALLS.lock().unwrap();
                calls.remove(&token)
            };
            if let Some(entry) = entry {
                dec_ref_bits(_py, entry.callable_bits);
                if !obj_from_bits(entry.args_bits).is_none() {
                    dec_ref_bits(_py, entry.args_bits);
                }
                if !obj_from_bits(entry.kwargs_bits).is_none() {
                    dec_ref_bits(_py, entry.kwargs_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
/// # Safety
/// This function must be called through the runtime FFI entrypoint while the thread registry is
/// initialized for the current runtime.
pub unsafe extern "C" fn molt_thread_registry_snapshot() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut tuple_bits = Vec::new();
        {
            let mut registry = THREAD_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
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
            let mut registry = THREAD_REGISTRY.lock().unwrap();
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
    crate::with_gil_entry!(_py, {
        let current_ident = current_thread_id();
        let mut count: usize = 1;
        {
            let mut registry = THREAD_REGISTRY.lock().unwrap();
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
    _token_bits: u64,
    _name_bits: u64,
    _daemon_bits: u64,
) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_forget(_token_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_snapshot() -> u64 {
    crate::with_gil_entry!(_py, {
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
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_registry_active_count() -> u64 {
    MoltObject::from_int(1).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_spawn(_payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    _token_bits: u64,
    _callable_bits: u64,
    _args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
    MoltObject::from_int(0).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_current_native_id() -> u64 {
    MoltObject::from_int(0).bits()
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
