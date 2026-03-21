// === FILE: runtime/molt-runtime/src/builtins/concurrent.rs ===
//! `concurrent.futures` intrinsics for Molt.
//!
//! Provides ThreadPoolExecutor and Future handle management.
//! Work items are callable bits dispatched to OS threads via crossbeam-channel.
//! Future results are stored in shared Arc<Mutex<FutureState>> cells.
//!
//! ABI: NaN-boxed u64 in/out.  Handles are opaque i64 IDs stored in
//! thread-local maps to keep them off the GIL.

use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::Duration;

// ── Future state ──────────────────────────────────────────────────────────

#[derive(Debug)]
enum FutureOutcome {
    /// Task is pending / in flight.
    Pending,
    /// Task completed successfully; holds the result bits.
    Done(u64),
    /// Task raised an exception; holds exception message.
    Exception(String),
    /// Task was cancelled before it started.
    Cancelled,
}

struct FutureState {
    outcome: FutureOutcome,
    running: bool,
    callbacks: Vec<u64>, // callable bits to fire when done
}

impl FutureState {
    fn new() -> Self {
        Self {
            outcome: FutureOutcome::Pending,
            running: false,
            callbacks: Vec::new(),
        }
    }

    fn is_done(&self) -> bool {
        !matches!(self.outcome, FutureOutcome::Pending)
    }

    fn is_cancelled(&self) -> bool {
        matches!(self.outcome, FutureOutcome::Cancelled)
    }
}

type SharedFuture = Arc<Mutex<FutureState>>;

// ── Work item dispatched to worker threads ─────────────────────────────────

struct WorkItem {
    future: SharedFuture,
    /// The Python callable bits and args list bits.
    fn_bits: u64,
    args_bits: u64,
}

// ── ThreadPool state ──────────────────────────────────────────────────────

struct ThreadPoolState {
    sender: Sender<Option<WorkItem>>, // None = shutdown sentinel
    _workers: Vec<thread::JoinHandle<()>>,
    max_workers: usize,
    shutdown: bool,
}

// ── Handle-id counter ─────────────────────────────────────────────────────

static NEXT_POOL_ID: AtomicI64 = AtomicI64::new(1);
static NEXT_FUTURE_ID: AtomicI64 = AtomicI64::new(1);

fn next_pool_id() -> i64 {
    NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_future_id() -> i64 {
    NEXT_FUTURE_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Process-wide handle storage ──────────────────────────────────────────
//
// These maps are process-wide so that handles created on the main thread
// are visible to worker threads (required for concurrent.futures).

static POOL_REGISTRY: LazyLock<Mutex<HashMap<i64, ThreadPoolState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static FUTURE_REGISTRY: LazyLock<Mutex<HashMap<i64, SharedFuture>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Worker thread loop ────────────────────────────────────────────────────
//
// Workers receive WorkItem packets.  They acquire the GIL to call the
// Python callable, then release and store the result.  This matches
// CPython's ThreadPoolExecutor model.

fn worker_loop(receiver: Receiver<Option<WorkItem>>) {
    struct _GilThreadGuard;
    impl Drop for _GilThreadGuard {
        fn drop(&mut self) {
            crate::concurrency::unregister_gil_thread();
        }
    }
    let _gtg = _GilThreadGuard;

    while let Ok(Some(item)) = receiver.recv() {
        // Mark running.
        {
            let mut state = item.future.lock().unwrap();
            if state.is_cancelled() {
                continue;
            }
            state.running = true;
        }

        // Call the Python function under the GIL.
        let result: Result<u64, String> = {
            let _gil = GilGuard::new();
            let _py_tok = _gil.token();
            let _py = &_py_tok;

            // Check cancellation once more under GIL.
            {
                let state = item.future.lock().unwrap();
                if state.is_cancelled() {
                    Err("cancelled".to_string())
                } else {
                    // Dispatch fn_bits(args_bits).
                    let fn_obj = obj_from_bits(item.fn_bits);
                    if fn_obj.is_none() {
                        Err("callable is None".to_string())
                    } else {
                        let args_obj = obj_from_bits(item.args_bits);
                        let result_bits = if args_obj.is_none() {
                            // Safety: GIL is held.
                            unsafe { call_callable0(_py, item.fn_bits) }
                        } else {
                            unsafe { call_callable1(_py, item.fn_bits, item.args_bits) }
                        };

                        if exception_pending(_py) {
                            let exc_bits =
                                exception_last_bits_noinc(_py).unwrap_or(MoltObject::none().bits());
                            let msg = format_obj_str(_py, obj_from_bits(exc_bits));
                            clear_exception(_py);
                            Err(msg)
                        } else {
                            Ok(result_bits)
                        }
                    }
                }
            }
        };

        // Store result and fire callbacks.
        let callbacks: Vec<u64> = {
            let mut state = item.future.lock().unwrap();
            state.running = false;
            match result {
                Ok(bits) => state.outcome = FutureOutcome::Done(bits),
                Err(msg) => {
                    if msg == "cancelled" {
                        state.outcome = FutureOutcome::Cancelled;
                    } else {
                        state.outcome = FutureOutcome::Exception(msg);
                    }
                }
            }
            std::mem::take(&mut state.callbacks)
        };

        // Fire done callbacks outside the future lock.
        if !callbacks.is_empty() {
            let _gil2 = GilGuard::new();
            let _py_cb = _gil2.token();
            let _py_cb = &_py_cb;
            for cb_bits in &callbacks {
                unsafe { call_callable1(_py_cb, *cb_bits, item.fn_bits) };
                if exception_pending(_py_cb) {
                    clear_exception(_py_cb);
                }
            }
        }
    }
}

// ── ThreadPoolExecutor intrinsics ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_new(max_workers_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let max_workers = to_i64(obj_from_bits(max_workers_bits)).unwrap_or(0).max(1) as usize;
        let workers_capped = max_workers.min(512);

        let (sender, receiver) = unbounded::<Option<WorkItem>>();
        let mut handles = Vec::with_capacity(workers_capped);
        for _ in 0..workers_capped {
            let rx = receiver.clone();
            crate::concurrency::register_gil_thread();
            let h = thread::spawn(move || worker_loop(rx));
            handles.push(h);
        }

        let id = next_pool_id();
        POOL_REGISTRY.lock().unwrap().insert(
            id,
            ThreadPoolState {
                sender,
                _workers: handles,
                max_workers: workers_capped,
                shutdown: false,
            },
        );
        int_bits_from_i64(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_submit(
    handle_bits: u64,
    fn_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };

        let future_shared = Arc::new(Mutex::new(FutureState::new()));
        let future_id = next_future_id();

        let sent = {
            let map = POOL_REGISTRY.lock().unwrap();
            if let Some(pool) = map.get(&pool_id) {
                if pool.shutdown {
                    false
                } else {
                    let item = WorkItem {
                        future: future_shared.clone(),
                        fn_bits,
                        args_bits,
                    };
                    pool.sender.send(Some(item)).is_ok()
                }
            } else {
                false
            }
        };

        if !sent {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "cannot submit to a shut-down executor",
            );
        }

        // Store in FUTURE_REGISTRY keyed by future_id.
        FUTURE_REGISTRY
            .lock()
            .unwrap()
            .insert(future_id, future_shared);
        int_bits_from_i64(_py, future_id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_shutdown(
    handle_bits: u64,
    wait_bits: u64,
    _cancel_futures_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };
        let wait = is_truthy(_py, obj_from_bits(wait_bits));

        let pool = POOL_REGISTRY.lock().unwrap().remove(&pool_id);
        if let Some(mut pool) = pool {
            pool.shutdown = true;
            // Send shutdown sentinels for each worker.
            for _ in 0..pool.max_workers {
                let _ = pool.sender.send(None);
            }
            if wait {
                // Join workers — release GIL while waiting.
                let _release = GilReleaseGuard::new();
                for handle in pool._workers {
                    let _ = handle.join();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_drop(handle_bits: u64) -> u64 {
    // Forward to shutdown(wait=False).
    let false_bits = MoltObject::from_bool(false).bits();
    molt_concurrent_threadpool_shutdown(handle_bits, false_bits, false_bits)
}

// ── Future intrinsics ─────────────────────────────────────────────────────

fn get_future(id: i64) -> Option<SharedFuture> {
    FUTURE_REGISTRY.lock().unwrap().get(&id).cloned()
}

fn wait_for_future(future: &SharedFuture, timeout_secs: Option<f64>) -> Result<(), ()> {
    use std::time::Instant;
    let deadline = timeout_secs.map(|t| Instant::now() + Duration::from_secs_f64(t));
    loop {
        {
            let state = future.lock().unwrap();
            if state.is_done() {
                return Ok(());
            }
        }
        if deadline.is_some_and(|dl| Instant::now() >= dl) {
            return Err(());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_result(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        let future = match get_future(id) {
            Some(f) => f,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
        };
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };
        {
            let _release = GilReleaseGuard::new();
            if wait_for_future(&future, timeout).is_err() {
                return raise_exception::<u64>(
                    _py,
                    "concurrent.futures.TimeoutError",
                    "future result timed out",
                );
            }
        }
        let state = future.lock().unwrap();
        match &state.outcome {
            FutureOutcome::Done(bits) => *bits,
            FutureOutcome::Exception(msg) => {
                raise_exception::<u64>(_py, "concurrent.futures.CancelledError", msg)
            }
            FutureOutcome::Cancelled => raise_exception::<u64>(
                _py,
                "concurrent.futures.CancelledError",
                "future was cancelled",
            ),
            FutureOutcome::Pending => {
                raise_exception::<u64>(_py, "RuntimeError", "future is still pending")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_exception(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        let future = match get_future(id) {
            Some(f) => f,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
        };
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };
        {
            let _release = GilReleaseGuard::new();
            if wait_for_future(&future, timeout).is_err() {
                return raise_exception::<u64>(
                    _py,
                    "concurrent.futures.TimeoutError",
                    "future exception timed out",
                );
            }
        }
        let state = future.lock().unwrap();
        match &state.outcome {
            FutureOutcome::Exception(msg) => {
                // Return a string representation — the Python layer wraps it.
                let ptr = alloc_string(_py, msg.as_bytes());
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            _ => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_done(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let state = f.lock().unwrap();
                MoltObject::from_bool(state.is_done()).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_cancelled(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let state = f.lock().unwrap();
                MoltObject::from_bool(state.is_cancelled()).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_cancel(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let mut state = f.lock().unwrap();
                if state.is_done() || state.running {
                    MoltObject::from_bool(false).bits()
                } else {
                    state.outcome = FutureOutcome::Cancelled;
                    MoltObject::from_bool(true).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_running(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let state = f.lock().unwrap();
                MoltObject::from_bool(state.running).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_add_done_callback(handle_bits: u64, fn_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let mut state = f.lock().unwrap();
                if state.is_done() {
                    // Fire immediately.
                    drop(state);
                    unsafe { call_callable1(_py, fn_bits, handle_bits) };
                    if exception_pending(_py) {
                        clear_exception(_py);
                    }
                } else {
                    state.callbacks.push(fn_bits);
                }
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            FUTURE_REGISTRY.lock().unwrap().remove(&id);
        }
        MoltObject::none().bits()
    })
}

// ── Module-level functions ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_as_completed(futures_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Collect future IDs from a list/tuple of handle bits.
        let futures_obj = obj_from_bits(futures_bits);
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };

        let future_ids: Vec<i64> = {
            let Some(ptr) = futures_obj.as_ptr() else {
                return raise_exception::<u64>(_py, "TypeError", "as_completed expects iterable");
            };
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "as_completed expects list or tuple",
                    );
                }
                let elems = seq_vec_ref(ptr);
                elems
                    .iter()
                    .filter_map(|&b| to_i64(obj_from_bits(b)))
                    .collect()
            }
        };

        // Wait for each in turn and return them in completion order.
        // This is a simplified synchronous implementation; the Python layer
        // should wrap this as a generator for true lazy iteration.
        use std::time::Instant;
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));

        let mut completed_bits = Vec::with_capacity(future_ids.len());
        let mut pending: Vec<i64> = future_ids;

        {
            let _release = GilReleaseGuard::new();
            while !pending.is_empty() {
                if deadline.is_some_and(|dl| Instant::now() >= dl) {
                    break;
                }
                let mut still_pending = Vec::new();
                for id in pending {
                    let future = FUTURE_REGISTRY.lock().unwrap().get(&id).cloned();
                    if let Some(f) = future {
                        let done = f.lock().unwrap().is_done();
                        if done {
                            completed_bits.push(int_bits_from_i64_raw(id));
                        } else {
                            still_pending.push(id);
                        }
                    }
                }
                pending = still_pending;
                if !pending.is_empty() {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }

        // Append remaining (not-done) futures at the end.
        for id in &pending {
            completed_bits.push(int_bits_from_i64_raw(*id));
        }

        let list_ptr = alloc_list(_py, &completed_bits);
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// Non-GIL int bits conversion helper used inside GIL-release sections.
fn int_bits_from_i64_raw(v: i64) -> u64 {
    // Inline the small-int fast path from numbers.rs.
    MoltObject::from_int(v).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_wait(
    futures_bits: u64,
    timeout_bits: u64,
    return_when_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let futures_obj = obj_from_bits(futures_bits);
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };
        let return_when = string_obj_to_owned(obj_from_bits(return_when_bits))
            .unwrap_or_else(|| "ALL_COMPLETED".to_string());

        let future_ids: Vec<i64> = {
            let Some(ptr) = futures_obj.as_ptr() else {
                return raise_exception::<u64>(_py, "TypeError", "wait expects iterable");
            };
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                    return raise_exception::<u64>(_py, "TypeError", "wait expects list or tuple");
                }
                let elems = seq_vec_ref(ptr);
                elems
                    .iter()
                    .filter_map(|&b| to_i64(obj_from_bits(b)))
                    .collect()
            }
        };

        use std::time::Instant;
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));

        let done_ids: Vec<i64>;
        let not_done_ids: Vec<i64>;

        {
            let _release = GilReleaseGuard::new();
            loop {
                let all_done = future_ids.iter().all(|id| {
                    FUTURE_REGISTRY
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(true)
                });
                let any_done = future_ids.iter().any(|id| {
                    FUTURE_REGISTRY
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(false)
                });
                let any_exception = future_ids.iter().any(|id| {
                    FUTURE_REGISTRY
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| matches!(f.lock().unwrap().outcome, FutureOutcome::Exception(_)))
                        .unwrap_or(false)
                });

                let should_stop = match return_when.as_str() {
                    "FIRST_COMPLETED" => any_done,
                    "FIRST_EXCEPTION" => any_exception || all_done,
                    _ => all_done, // ALL_COMPLETED
                };

                if should_stop {
                    break;
                }

                if deadline.is_some_and(|dl| Instant::now() >= dl) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }

            done_ids = future_ids
                .iter()
                .filter(|id| {
                    FUTURE_REGISTRY
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            not_done_ids = future_ids
                .iter()
                .filter(|id| {
                    !FUTURE_REGISTRY
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(true)
                })
                .copied()
                .collect();
        }

        let done_bits: Vec<u64> = done_ids
            .iter()
            .map(|&id| int_bits_from_i64(_py, id))
            .collect();
        let not_done_bits: Vec<u64> = not_done_ids
            .iter()
            .map(|&id| int_bits_from_i64(_py, id))
            .collect();

        let done_set_ptr = alloc_set_with_entries(_py, &done_bits);
        let not_done_set_ptr = alloc_set_with_entries(_py, &not_done_bits);
        if done_set_ptr.is_null() || not_done_set_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_ptr(done_set_ptr).bits(),
                MoltObject::from_ptr(not_done_set_ptr).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

// ── Constants ──────────────────────────────────────────────────────────────

fn return_str(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        raise_exception::<u64>(_py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_first_completed() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, "FIRST_COMPLETED") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_first_exception() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, "FIRST_EXCEPTION") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_all_completed() -> u64 {
    crate::with_gil_entry!(_py, { return_str(_py, "ALL_COMPLETED") })
}
