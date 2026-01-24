use crate::*;
use super::{await_waiters_take, wake_task_ptr};
use crate::PyToken;

#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{unbounded, Receiver, Sender};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
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
    handles: Mutex<Vec<thread::JoinHandle<()>>>,
    worker_count: AtomicUsize,
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadPool {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = unbounded();
        let pool = Self {
            sender,
            receiver,
            handles: Mutex::new(Vec::new()),
            worker_count: AtomicUsize::new(0),
        };
        pool.start_workers();
        pool
    }

    fn start_workers(&self) {
        let count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);
        let mut handles = self.handles.lock().unwrap();
        self.worker_count.store(count, AtomicOrdering::Release);
        for _ in 0..count {
            let rx = self.receiver.clone();
            let handle = thread::spawn(move || thread_worker(rx));
            handles.push(handle);
        }
    }

    fn submit(&self, item: ThreadWorkItem) {
        let _ = self.sender.send(ThreadWork::Run(item));
    }

    pub(crate) fn shutdown(&self) {
        let count = self.worker_count.load(AtomicOrdering::Acquire).max(1);
        for _ in 0..count {
            let _ = self.sender.send(ThreadWork::Shutdown);
        }
        let handles = {
            let mut guard = self.handles.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for handle in handles {
            let _ = handle.join();
        }
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
fn thread_worker(rx: Receiver<ThreadWork>) {
    loop {
        let work = match rx.recv() {
            Ok(work) => work,
            Err(_) => break,
        };
        match work {
            ThreadWork::Shutdown => break,
            ThreadWork::Run(item) => {
                let _gil = GilGuard::new();
                let _py = _gil.token();
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
                    let exc_bits = molt_exception_last();
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
                let waiters = await_waiters_take(_py, task.future_ptr);
                for waiter in waiters {
                    wake_task_ptr(_py, waiter.0);
                }
            }
        }
    }
    with_gil(|py| crate::state::clear_worker_thread_state(&py));
}

#[cfg(not(target_arch = "wasm32"))]
fn call_thread_callable(_py: &PyToken<'_>, callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
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
#[no_mangle]
pub unsafe extern "C" fn molt_thread_submit(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
    runtime_state(_py).thread_pool().submit(ThreadWorkItem {
        task: state,
        callable_bits,
        args_bits,
        kwargs_bits,
    });
    future_bits

    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_submit(
    _callable_bits: u64,
    _args_bits: u64,
    _kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
    raise_exception::<u64>(_py, "RuntimeError", "thread submit unsupported on wasm")

    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_thread_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
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
        return raise_exception::<i64>(_py, "RuntimeError", "thread task missing");
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
#[no_mangle]
pub unsafe extern "C" fn molt_thread_poll(_obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
    pending_bits_i64()

    })
}
