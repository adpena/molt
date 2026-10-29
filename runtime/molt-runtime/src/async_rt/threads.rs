use super::wake_await_waiters;
use crate::BuiltinExc;
use crate::PyToken;
use crate::*;

#[cfg(not(target_arch = "wasm32"))]
use crate::concurrency::isolates::configured_thread_stack_size;
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, Sender, unbounded};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ThreadTaskState {
    future_ptr: *mut u8,
    done: AtomicBool,
    pub(crate) cancelled: AtomicBool,
    pub(crate) result: Mutex<Option<u64>>,
    pub(crate) exception: Mutex<Option<u64>>,
    wait_lock: Mutex<()>,
    pub(crate) condvar: Condvar,
}

// Raw pointers are managed via runtime locks; task state is safe to share across threads.
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for ThreadTaskState {}
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Sync for ThreadTaskState {}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadWorkItem {
    task: Arc<ThreadTaskState>,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
}

#[cfg(not(target_arch = "wasm32"))]
enum ThreadWork {
    Run(ThreadWorkItem),
    Shutdown,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ThreadPool {
    sender: Sender<ThreadWork>,
    receiver: Receiver<ThreadWork>,
    handles: crate::state::lifecycle::RuntimeServiceWorkers,
    running: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadPool {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = unbounded();
        Self {
            sender,
            receiver,
            handles: crate::state::lifecycle::RuntimeServiceWorkers::default(),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) fn start_workers(
        &self,
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    ) -> std::io::Result<()> {
        self.handles.start(origin, |handles| {
            let count = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .max(1);
            for index in 0..count {
                let rx = self.receiver.clone();
                let tx = self.sender.clone();
                let running = Arc::clone(&self.running);
                let mut builder = thread::Builder::new().name(format!("molt-thread-pool-{index}"));
                if let Some(stack_size) = configured_thread_stack_size() {
                    builder = builder.stack_size(stack_size);
                }
                handles.push(crate::state::lifecycle::RuntimeServiceWorker::spawn(
                    origin,
                    builder,
                    move || thread_worker(origin, rx, tx, running),
                )?);
            }
            Ok(())
        })
    }

    fn submit(&self, item: ThreadWorkItem) -> Result<(), ThreadWorkItem> {
        if !self.running.load(AtomicOrdering::Acquire) {
            return Err(item);
        }
        self.sender
            .send(ThreadWork::Run(item))
            .map_err(|error| match error.0 {
                ThreadWork::Run(item) => item,
                ThreadWork::Shutdown => unreachable!(),
            })
    }

    pub(crate) fn stop(&self) {
        if self.running.swap(false, AtomicOrdering::AcqRel) {
            for _ in 0..self.handles.count() {
                let _ = self.sender.send(ThreadWork::Shutdown);
            }
        }
    }

    pub(crate) fn join(&self) -> bool {
        self.handles.join()
    }

    pub(crate) fn clear_stopped_queue(&self, py: &PyToken<'_>) -> bool {
        assert!(!self.running.load(AtomicOrdering::Acquire));
        let mut changed = false;
        while let Ok(work) = self.receiver.try_recv() {
            if let ThreadWork::Run(item) = work {
                changed = true;
                item.task.cancelled.store(true, AtomicOrdering::Release);
                item.task.notify_done();
                release_thread_work(py, item);
            }
        }
        changed
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadTaskState {
    pub(crate) fn new(future_ptr: *mut u8) -> Self {
        Self {
            future_ptr,
            done: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            result: Mutex::new(None),
            exception: Mutex::new(None),
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    fn set_result(&self, bits: u64) {
        let mut guard = self.result.lock().unwrap();
        *guard = Some(bits);
    }

    fn set_exception(&self, bits: u64) {
        let mut guard = self.exception.lock().unwrap();
        *guard = Some(bits);
    }

    fn notify_done(&self) {
        self.done.store(true, AtomicOrdering::Release);
        self.condvar.notify_all();
    }

    pub(crate) fn wait_blocking(&self, timeout: Option<Duration>) {
        if self.done.load(AtomicOrdering::Acquire) {
            return;
        }
        let mut guard = self.wait_lock.lock().unwrap();
        loop {
            if self.done.load(AtomicOrdering::Acquire) {
                break;
            }
            match timeout {
                Some(wait) => {
                    let _ = self.condvar.wait_timeout(guard, wait).unwrap();
                    break;
                }
                None => {
                    guard = self.condvar.wait(guard).unwrap();
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn thread_worker(
    origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    rx: Receiver<ThreadWork>,
    tx: Sender<ThreadWork>,
    running: Arc<AtomicBool>,
) {
    while let Ok(work) = rx.recv() {
        match work {
            ThreadWork::Shutdown => break,
            ThreadWork::Run(item) => {
                let Some(execution) =
                    crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
                else {
                    // The published pool retains both channel ends until join.
                    // Return owned work without touching runtime reference counts.
                    assert!(tx.send(ThreadWork::Run(item)).is_ok());
                    break;
                };
                if !running.load(AtomicOrdering::Acquire) {
                    assert!(tx.send(ThreadWork::Run(item)).is_ok());
                    break;
                }
                let _py = execution.token();
                let _py = &_py;
                let ThreadWorkItem {
                    task,
                    callable_bits,
                    args_bits,
                    kwargs_bits,
                } = item;
                let result_bits = call_thread_callable(_py, callable_bits, args_bits, kwargs_bits);
                dec_ref_bits(_py, callable_bits);
                if !obj_from_bits(args_bits).is_none() {
                    dec_ref_bits(_py, args_bits);
                }
                if !obj_from_bits(kwargs_bits).is_none() {
                    dec_ref_bits(_py, kwargs_bits);
                }
                let cancelled = task.cancelled.load(AtomicOrdering::Acquire);
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last_pending();
                    molt_exception_clear();
                    if cancelled {
                        dec_ref_bits(_py, exc_bits);
                    } else {
                        task.set_exception(exc_bits);
                    }
                } else if cancelled {
                    if !obj_from_bits(result_bits).is_none() {
                        dec_ref_bits(_py, result_bits);
                    }
                } else {
                    task.set_result(result_bits);
                }
                task.notify_done();
                let _ = wake_await_waiters(_py, task.future_ptr);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn release_thread_work(py: &PyToken<'_>, item: ThreadWorkItem) {
    dec_ref_bits(py, item.callable_bits);
    if !obj_from_bits(item.args_bits).is_none() {
        dec_ref_bits(py, item.args_bits);
    }
    if !obj_from_bits(item.kwargs_bits).is_none() {
        dec_ref_bits(py, item.kwargs_bits);
    }
}

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
        return unsafe { call_callable0(_py, callable_bits) };
    }
    let builder_bits = molt_callargs_new(0, 0);
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    if has_args {
        let _ = unsafe { molt_callargs_expand_star(builder_bits, args_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    if has_kwargs {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return MoltObject::none().bits();
        }
    }
    molt_call_bind(callable_bits, builder_bits)
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `callable_bits`, `args_bits`, and `kwargs_bits` must be valid runtime objects.
/// The runtime must be initialized and the call must be allowed to enter the GIL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_thread_submit(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !runtime_state(_py).thread_borrow_admission_is_open() {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "runtime thread admission is closed",
            );
        }
        // Complete startup before publishing future or callable ownership.
        let pool = runtime_state(_py).thread_pool();
        let future_bits = molt_future_new(thread_poll_fn_addr(), 0);
        let Some(future_ptr) = resolve_obj_ptr(future_bits) else {
            return MoltObject::none().bits();
        };
        let state = Arc::new(ThreadTaskState::new(future_ptr));
        runtime_state(_py)
            .thread_tasks
            .lock()
            .unwrap()
            .insert(PtrSlot(future_ptr), Arc::clone(&state));
        inc_ref_bits(_py, callable_bits);
        if !obj_from_bits(args_bits).is_none() {
            inc_ref_bits(_py, args_bits);
        }
        if !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        if let Err(item) = pool.submit(ThreadWorkItem {
            task: state,
            callable_bits,
            args_bits,
            kwargs_bits,
        }) {
            runtime_state(_py)
                .thread_tasks
                .lock()
                .unwrap()
                .remove(&PtrSlot(future_ptr));
            release_thread_work(_py, item);
            dec_ref_bits(_py, future_bits);
            return raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "runtime thread admission is closed",
            );
        }
        future_bits
    })
}

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `callable_bits`, `args_bits`, and `kwargs_bits` must be valid runtime objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_thread_submit(
    _callable_bits: u64,
    _args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::RuntimeError,
            "thread submit unsupported on wasm",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `obj_bits` must be a valid thread wait future object from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_thread_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let Some(state) = runtime_state(_py)
            .thread_tasks
            .lock()
            .unwrap()
            .get(&PtrSlot(obj_ptr))
            .cloned()
        else {
            return raise_exception::<i64>(_py, BuiltinExc::RuntimeError, "thread task missing");
        };
        if state.done.load(AtomicOrdering::Acquire) {
            task_take_cancel_pending(obj_ptr);
        } else if task_cancel_pending(obj_ptr) {
            task_take_cancel_pending(obj_ptr);
            state.cancelled.store(true, AtomicOrdering::Release);
            return raise_cancelled_with_message::<i64>(_py, obj_ptr);
        }
        if !state.done.load(AtomicOrdering::Acquire) {
            return pending_bits_i64();
        }
        if let Some(exc_bits) = state.exception.lock().unwrap().as_ref().copied() {
            let res_bits = molt_raise(exc_bits);
            return res_bits as i64;
        }
        if let Some(result_bits) = state.result.lock().unwrap().as_ref().copied() {
            inc_ref_bits(_py, result_bits);
            return result_bits as i64;
        }
        MoltObject::none().bits() as i64
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `_obj_bits` is a valid thread object pointer or zero.
pub unsafe extern "C" fn molt_thread_poll(_obj_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, { pending_bits_i64() })
}

// ---------------------------------------------------------------------------
// Threading capability check – returns 1 if real threads are available, 0
// otherwise.  On WASM this always returns 0.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_available() -> u64 {
    MoltObject::from_bool(true).bits()
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_threading_available() -> u64 {
    MoltObject::from_bool(false).bits()
}

// ---------------------------------------------------------------------------
// WASM module gate – checks whether a threading-dependent module can be used
// on the current platform.  On non-WASM this is always a no-op.  On WASM it
// raises RuntimeError for modules that require real threads.
// ---------------------------------------------------------------------------

/// List of modules that require real OS threads and cannot function under the
/// single-threaded WASM runtime.
#[cfg(target_arch = "wasm32")]
const WASM_THREAD_BLOCKED_MODULES: &[&str] = &[
    "smtplib",
    "socketserver",
    "xmlrpc.server",
    "http.server",
    "concurrent.futures",
    "multiprocessing",
];

/// # Safety
/// `name_bits` must be a valid NaN-boxed string object.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_wasm_check_module_gate(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = crate::string_obj_to_owned(name_obj) else {
            return MoltObject::none().bits();
        };
        for blocked in WASM_THREAD_BLOCKED_MODULES {
            if name == *blocked || name.starts_with(&format!("{blocked}.")) {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::RuntimeError,
                    &format!(
                        "module '{name}' requires threading support which is unavailable in the \
                         WASM runtime. This is a known promotion blocker – see MOL-184."
                    ),
                );
            }
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// `name_bits` must be a valid NaN-boxed string object.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_wasm_check_module_gate(_name_bits: u64) -> u64 {
    MoltObject::none().bits()
}

// ---------------------------------------------------------------------------
// Explicit WASM gate for threading.Thread, threading.Lock, etc.
// On WASM, attempting to create a Thread raises RuntimeError.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_thread_start(_callable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::RuntimeError,
            "threading.Thread is not available in the WASM runtime: WASM is \
             single-threaded. Server-heavy workloads using threading (smtplib, \
             socketserver) are promotion blockers – see MOL-184.",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `callable_bits` must be a valid runtime callable object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_thread_start(callable_bits: u64) -> u64 {
    // On native, this is a simple alias for molt_thread_submit with no
    // args/kwargs.
    unsafe {
        molt_thread_submit(
            callable_bits,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        )
    }
}
