use crate::{raise_exception, MoltObject, PyToken};

#[cfg(not(target_arch = "wasm32"))]
use super::current_thread_id;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::builtins::attr::attr_name_bits_from_bytes;
#[cfg(not(target_arch = "wasm32"))]
use crate::builtins::modules::molt_module_cache_get;
#[cfg(not(target_arch = "wasm32"))]
use crate::call::dispatch::call_callable1;
#[cfg(not(target_arch = "wasm32"))]
use crate::state::{
    clear_thread_runtime_state, runtime_reset_for_init, runtime_teardown_isolate,
    set_thread_runtime_state, touch_tls_guard, RuntimeState,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::GilGuard;
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    alloc_bytes, alloc_string, bits_from_ptr, bytes_data, bytes_len, dec_ref_bits,
    exception_pending, format_exception_with_traceback, has_capability, molt_exception_clear,
    molt_exception_last, molt_module_get_attr, obj_from_bits, object_type_id, ptr_from_bits,
    release_ptr, TYPE_ID_BYTES,
};

#[cfg(not(target_arch = "wasm32"))]
extern "C" {
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
#[no_mangle]
pub unsafe extern "C" fn molt_thread_spawn(payload_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "thread") && !has_capability(_py, "thread.spawn") {
            return raise_exception::<_>(_py, "PermissionError", "missing thread capability");
        }
        let shared_runtime = has_capability(_py, "thread.shared");
        let payload = match payload_from_bits(_py, payload_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let handle = Arc::new(MoltThreadHandle::new());
        let thread_handle = handle.clone();
        let join = if shared_runtime {
            let state_ptr =
                crate::state::runtime_state::runtime_state(_py) as *const RuntimeState as usize;
            thread::spawn(move || thread_main_shared(payload, thread_handle, state_ptr))
        } else {
            thread::spawn(move || thread_main(payload, thread_handle))
        };
        handle.set_join_handle(join);
        let raw = Arc::into_raw(handle) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
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
#[no_mangle]
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
#[no_mangle]
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
#[no_mangle]
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
#[no_mangle]
pub extern "C" fn molt_thread_current_ident() -> u64 {
    let ident = current_thread_id();
    MoltObject::from_int(ident as i64).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_thread_current_native_id() -> u64 {
    let ident = current_thread_id();
    MoltObject::from_int(ident as i64).bits()
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_drop(handle_bits: u64) -> u64 {
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

#[cfg(target_arch = "wasm32")]
#[no_mangle]
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
#[no_mangle]
pub extern "C" fn molt_thread_join(_handle_bits: u64, _timeout_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_is_alive(_handle_bits: u64) -> u64 {
    MoltObject::from_bool(false).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_ident(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_native_id(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_current_ident() -> u64 {
    MoltObject::from_int(0).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_current_native_id() -> u64 {
    MoltObject::from_int(0).bits()
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_thread_drop(_handle_bits: u64) -> u64 {
    MoltObject::none().bits()
}
