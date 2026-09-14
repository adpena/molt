// === FILE: runtime/molt-runtime/src/builtins/concurrent.rs ===
//! `concurrent.futures` intrinsics for Molt.
//!
//! Provides ThreadPoolExecutor and Future handle management.
//! Work items are callable bits dispatched to OS threads via crossbeam-channel.
//! Future results are stored in shared Arc<Mutex<FutureState>> cells.
//!
//! ABI: NaN-boxed u64 in/out.  Handles are opaque i64 IDs stored in
//! runtime-owned, mutex-protected maps shared by executor and caller threads.

use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Future state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
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
    registered: bool,
    callbacks: Vec<u64>, // owned callable bits to fire when done
}

impl FutureState {
    fn new() -> Self {
        Self {
            outcome: FutureOutcome::Pending,
            running: false,
            registered: true,
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

fn release_future_owners(py: &PyToken<'_>, future: &SharedFuture, workers_stopped: bool) {
    let (result, callbacks) = {
        let mut state = future.lock().unwrap();
        state.registered = false;
        let result = if matches!(state.outcome, FutureOutcome::Done(_)) {
            match std::mem::replace(&mut state.outcome, FutureOutcome::Cancelled) {
                FutureOutcome::Done(bits) => Some(bits),
                _ => unreachable!(),
            }
        } else {
            None
        };
        // A live worker still owns pending callbacks even after the public
        // handle disappears. Teardown joins it before the final owner drain.
        let callbacks = if workers_stopped || state.is_done() {
            std::mem::take(&mut state.callbacks)
        } else {
            Vec::new()
        };
        (result, callbacks)
    };
    if let Some(bits) = result {
        dec_ref_bits(py, bits);
    }
    for bits in callbacks {
        dec_ref_bits(py, bits);
    }
}

// ── Work item dispatched to worker threads ─────────────────────────────────

struct WorkItem {
    future: SharedFuture,
    /// Owned callable and argument references retained through worker dispatch.
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

pub(crate) struct ConcurrentRuntimeState {
    next_pool_id: AtomicI64,
    next_future_id: AtomicI64,
    pools: Mutex<HashMap<i64, ThreadPoolState>>,
    futures: Mutex<HashMap<i64, SharedFuture>>,
}

impl ConcurrentRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            next_pool_id: AtomicI64::new(1),
            next_future_id: AtomicI64::new(1),
            pools: Mutex::new(HashMap::new()),
            futures: Mutex::new(HashMap::new()),
        }
    }
}

fn concurrent_state(_py: &PyToken<'_>) -> &'static ConcurrentRuntimeState {
    &crate::runtime_state(_py).concurrent
}

fn next_pool_id(_py: &PyToken<'_>) -> i64 {
    concurrent_state(_py)
        .next_pool_id
        .fetch_add(1, Ordering::Relaxed)
}

fn next_future_id(_py: &PyToken<'_>) -> i64 {
    concurrent_state(_py)
        .next_future_id
        .fetch_add(1, Ordering::Relaxed)
}

fn pool_registry(_py: &PyToken<'_>) -> &'static Mutex<HashMap<i64, ThreadPoolState>> {
    &concurrent_state(_py).pools
}

fn future_registry(_py: &PyToken<'_>) -> &'static Mutex<HashMap<i64, SharedFuture>> {
    &concurrent_state(_py).futures
}

pub(crate) fn concurrent_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let pools = {
        let mut pools = state.concurrent.pools.lock().unwrap();
        std::mem::take(&mut *pools)
    };
    let futures = {
        let mut futures = state.concurrent.futures.lock().unwrap();
        std::mem::take(&mut *futures)
    };
    // A drained registry is still reachable by finalizers and joined workers.
    // Keep its identity sequence: stale handles must never alias new owners.
    let changed = !pools.is_empty() || !futures.is_empty();
    let mut workers = Vec::new();
    for (_, mut pool) in pools {
        if !pool.shutdown {
            pool.shutdown = true;
            for _ in 0..pool.max_workers {
                let _ = pool.sender.send(None);
            }
        }
        workers.extend(pool._workers);
    }
    if !workers.is_empty() {
        let _release = GilReleaseGuard::suspend();
        for worker in workers {
            let _ = worker.join();
        }
    }
    for future in futures.into_values() {
        release_future_owners(_py, &future, true);
    }
    changed
}

// ── Worker thread loop ────────────────────────────────────────────────────
//
// Workers receive WorkItem packets.  They acquire the GIL to call the
// Python callable, then release and store the result.  This matches
// CPython's ThreadPoolExecutor model.

fn worker_loop(receiver: Receiver<Option<WorkItem>>) {
    crate::state::run_runtime_worker(|| worker_loop_inner(receiver));
}

fn worker_loop_inner(receiver: Receiver<Option<WorkItem>>) {
    while let Ok(Some(item)) = receiver.recv() {
        let gil = GilGuard::new();
        let token = gil.token();
        let py = &token;
        let cancelled = {
            let mut state = item.future.lock().unwrap();
            let cancelled = state.is_cancelled();
            if !cancelled {
                state.running = true;
            }
            cancelled
        };
        // No future mutex survives dispatch, result destruction, or callbacks.
        let result = if cancelled {
            FutureOutcome::Cancelled
        } else if obj_from_bits(item.fn_bits).is_none() {
            FutureOutcome::Exception("callable is None".to_string())
        } else {
            let bits = if obj_from_bits(item.args_bits).is_none() {
                unsafe { call_callable0(py, item.fn_bits) }
            } else {
                unsafe { call_callable1(py, item.fn_bits, item.args_bits) }
            };
            if exception_pending(py) {
                let exc_bits = exception_last_bits_noinc(py).unwrap_or(MoltObject::none().bits());
                let msg = format_obj_str(py, obj_from_bits(exc_bits));
                clear_exception(py);
                dec_ref_bits(py, bits);
                FutureOutcome::Exception(msg)
            } else {
                FutureOutcome::Done(bits)
            }
        };
        let (callbacks, discarded_result) = {
            let mut state = item.future.lock().unwrap();
            state.running = false;
            let discarded_result = match result {
                FutureOutcome::Done(bits) if !state.registered => {
                    state.outcome = FutureOutcome::Cancelled;
                    Some(bits)
                }
                outcome => {
                    state.outcome = outcome;
                    None
                }
            };
            (std::mem::take(&mut state.callbacks), discarded_result)
        };
        if let Some(bits) = discarded_result {
            dec_ref_bits(py, bits);
        }
        for cb_bits in callbacks {
            let out = unsafe { call_callable1(py, cb_bits, item.fn_bits) };
            if exception_pending(py) {
                clear_exception(py);
            }
            dec_ref_bits(py, out);
            dec_ref_bits(py, cb_bits);
        }
        dec_ref_bits(py, item.fn_bits);
        dec_ref_bits(py, item.args_bits);
    }
}

// ── ThreadPoolExecutor intrinsics ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_new(max_workers_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let max_workers = to_i64(obj_from_bits(max_workers_bits)).unwrap_or(0).max(1) as usize;
        let workers_capped = max_workers.min(512);

        let (sender, receiver) = unbounded::<Option<WorkItem>>();
        let mut handles = Vec::with_capacity(workers_capped);
        for _ in 0..workers_capped {
            let rx = receiver.clone();
            let h = thread::spawn(move || worker_loop(rx));
            handles.push(h);
        }

        let id = next_pool_id(_py);
        pool_registry(_py).lock().unwrap().insert(
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
    crate::with_gil_entry_nopanic!(_py, {
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };

        let future_shared = Arc::new(Mutex::new(FutureState::new()));
        let future_id = next_future_id(_py);

        inc_ref_bits(_py, fn_bits);
        inc_ref_bits(_py, args_bits);
        let sent = {
            let map = pool_registry(_py).lock().unwrap();
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
            dec_ref_bits(_py, fn_bits);
            dec_ref_bits(_py, args_bits);
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "cannot submit to a shut-down executor",
            );
        }

        // Store in this runtime's future registry keyed by future_id.
        future_registry(_py)
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
    crate::with_gil_entry_nopanic!(_py, {
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };
        let wait = is_truthy(_py, obj_from_bits(wait_bits));

        let pool = {
            let mut pools = pool_registry(_py).lock().unwrap();
            if let Some(pool) = pools.get_mut(&pool_id) {
                if !pool.shutdown {
                    pool.shutdown = true;
                    for _ in 0..pool.max_workers {
                        let _ = pool.sender.send(None);
                    }
                }
            }
            // Non-waiting shutdown closes admission, but runtime teardown still
            // owns the join handles until every already-submitted job finishes.
            if wait { pools.remove(&pool_id) } else { None }
        };
        if let Some(pool) = pool {
            let _release = GilReleaseGuard::suspend();
            for handle in pool._workers {
                let _ = handle.join();
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

fn get_future(_py: &PyToken<'_>, id: i64) -> Option<SharedFuture> {
    future_registry(_py).lock().unwrap().get(&id).cloned()
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        let future = match get_future(_py, id) {
            Some(f) => f,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
        };
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };
        let timed_out = {
            let _release = GilReleaseGuard::suspend();
            wait_for_future(&future, timeout).is_err()
        };
        if timed_out {
            return raise_exception::<u64>(
                _py,
                "concurrent.futures.TimeoutError",
                "future result timed out",
            );
        }
        let outcome = {
            let state = future.lock().unwrap();
            if let FutureOutcome::Done(bits) = state.outcome {
                inc_ref_bits(_py, bits);
            }
            state.outcome.clone()
        };
        match outcome {
            FutureOutcome::Done(bits) => bits,
            FutureOutcome::Exception(msg) => {
                raise_exception::<u64>(_py, "concurrent.futures.CancelledError", &msg)
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        let future = match get_future(_py, id) {
            Some(f) => f,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
        };
        let timeout = {
            let obj = obj_from_bits(timeout_bits);
            if obj.is_none() { None } else { to_f64(obj) }
        };
        let timed_out = {
            let _release = GilReleaseGuard::suspend();
            wait_for_future(&future, timeout).is_err()
        };
        if timed_out {
            return raise_exception::<u64>(
                _py,
                "concurrent.futures.TimeoutError",
                "future exception timed out",
            );
        }
        let outcome = future.lock().unwrap().outcome.clone();
        match outcome {
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(_py, id) {
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(_py, id) {
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(_py, id) {
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(_py, id) {
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
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "future handle must be int"),
        };
        match get_future(_py, id) {
            None => raise_exception::<u64>(_py, "ValueError", "invalid future handle"),
            Some(f) => {
                let mut state = f.lock().unwrap();
                if state.is_done() {
                    // Fire immediately.
                    drop(state);
                    let out = unsafe { call_callable1(_py, fn_bits, handle_bits) };
                    if exception_pending(_py) {
                        clear_exception(_py);
                    }
                    dec_ref_bits(_py, out);
                } else {
                    inc_ref_bits(_py, fn_bits);
                    state.callbacks.push(fn_bits);
                }
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            let future = future_registry(_py).lock().unwrap().remove(&id);
            if let Some(future) = future {
                release_future_owners(_py, &future, false);
            }
        }
        MoltObject::none().bits()
    })
}

// ── Module-level functions ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_as_completed(futures_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                crate::object::seq_access::with_borrowed(ptr, |elems| {
                    elems
                        .iter()
                        .filter_map(|&b| to_i64(obj_from_bits(b)))
                        .collect()
                })
            }
        };

        // Wait for each in turn and return them in completion order.
        // This is a simplified synchronous implementation; the Python layer
        // should wrap this as a generator for true lazy iteration.
        use std::time::Instant;
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));
        let futures = future_registry(_py);

        let mut completed_bits = Vec::with_capacity(future_ids.len());
        let mut pending: Vec<i64> = future_ids;

        {
            let _release = GilReleaseGuard::suspend();
            while !pending.is_empty() {
                if deadline.is_some_and(|dl| Instant::now() >= dl) {
                    break;
                }
                let mut still_pending = Vec::new();
                for id in pending {
                    let future = futures.lock().unwrap().get(&id).cloned();
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
    crate::with_gil_entry_nopanic!(_py, {
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
                crate::object::seq_access::with_borrowed(ptr, |elems| {
                    elems
                        .iter()
                        .filter_map(|&b| to_i64(obj_from_bits(b)))
                        .collect()
                })
            }
        };

        use std::time::Instant;
        let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t));
        let futures_registry = future_registry(_py);

        let done_ids: Vec<i64>;
        let not_done_ids: Vec<i64>;

        {
            let _release = GilReleaseGuard::suspend();
            loop {
                let all_done = future_ids.iter().all(|id| {
                    futures_registry
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(true)
                });
                let any_done = future_ids.iter().any(|id| {
                    futures_registry
                        .lock()
                        .unwrap()
                        .get(id)
                        .map(|f| f.lock().unwrap().is_done())
                        .unwrap_or(false)
                });
                let any_exception = future_ids.iter().any(|id| {
                    futures_registry
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
                    futures_registry
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
                    !futures_registry
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
    crate::with_gil_entry_nopanic!(_py, { return_str(_py, "FIRST_COMPLETED") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_first_exception() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { return_str(_py, "FIRST_EXCEPTION") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_all_completed() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { return_str(_py, "ALL_COMPLETED") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    static REENTRANT_FUTURE: AtomicI64 = AtomicI64::new(0);
    static REENTRANT_DISPATCH_OK: AtomicBool = AtomicBool::new(false);
    static REENTRANT_CALLBACK_OK: AtomicBool = AtomicBool::new(false);
    static CALLBACK_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn owner_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn native_callable(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    extern "C" fn retained_argument(bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, bits);
            bits
        })
    }

    extern "C" fn reentrant_argument(bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let id = REENTRANT_FUTURE.load(Ordering::Acquire);
            if let Some(future) = get_future(py, id) {
                // Fail observably instead of hanging the test on the old
                // dispatch-with-future-lock-held implementation.
                let unlocked = future.try_lock().is_ok();
                if unlocked {
                    let running = molt_concurrent_future_running(MoltObject::from_int(id).bits());
                    REENTRANT_DISPATCH_OK.store(
                        running == MoltObject::from_bool(true).bits(),
                        Ordering::Release,
                    );
                }
            }
            retained_argument(bits)
        })
    }

    extern "C" fn reentrant_done_callback(_callable_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let id = REENTRANT_FUTURE.load(Ordering::Acquire);
            if let Some(future) = get_future(py, id) {
                let unlocked = future.try_lock().is_ok();
                if unlocked {
                    let handle = MoltObject::from_int(id).bits();
                    let done = molt_concurrent_future_done(handle);
                    let result = molt_concurrent_future_result(handle, MoltObject::none().bits());
                    REENTRANT_CALLBACK_OK.store(
                        done == MoltObject::from_bool(true).bits() && !exception_pending(py),
                        Ordering::Release,
                    );
                    dec_ref_bits(py, result);
                }
            }
            CALLBACK_CALLS.fetch_add(1, Ordering::AcqRel);
            MoltObject::none().bits()
        })
    }

    extern "C" fn owned_callback_result(callable_bits: u64) -> u64 {
        CALLBACK_CALLS.fetch_add(1, Ordering::AcqRel);
        retained_argument(callable_bits)
    }

    #[test]
    fn future_result_returns_independent_owners_without_consuming_stored_result() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            concurrent_clear_runtime_state(py, runtime_state(py));
            let callable = native_callable(py, retained_argument as *const ());
            let payload = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let baseline = owner_count(payload);
            let pool = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
            let future = molt_concurrent_threadpool_submit(pool, callable, payload);
            molt_concurrent_threadpool_shutdown(
                pool,
                MoltObject::from_bool(true).bits(),
                MoltObject::none().bits(),
            );
            assert!(!exception_pending(py));
            assert_eq!(
                owner_count(payload),
                baseline + 1,
                "completed future owns its result"
            );
            let first = molt_concurrent_future_result(future, MoltObject::none().bits());
            let second = molt_concurrent_future_result(future, MoltObject::none().bits());
            assert_eq!((first, second), (payload, payload));
            assert_eq!(owner_count(payload), baseline + 3);
            dec_ref_bits(py, first);
            assert_eq!(owner_count(payload), baseline + 2);
            molt_concurrent_future_drop(future);
            assert_eq!(
                owner_count(payload),
                baseline + 1,
                "dropping future preserves returned owner"
            );
            dec_ref_bits(py, second);
            assert_eq!(owner_count(payload), baseline);
            dec_ref_bits(py, callable);
            dec_ref_bits(py, payload);
        });
    }

    #[test]
    fn worker_dispatch_and_done_callback_can_reenter_the_same_future() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            concurrent_clear_runtime_state(py, runtime_state(py));
            REENTRANT_DISPATCH_OK.store(false, Ordering::Release);
            REENTRANT_CALLBACK_OK.store(false, Ordering::Release);
            CALLBACK_CALLS.store(0, Ordering::Release);
            let callable = native_callable(py, reentrant_argument as *const ());
            let callback = native_callable(py, reentrant_done_callback as *const ());
            let payload = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let callable_base = owner_count(callable);
            let callback_base = owner_count(callback);
            let payload_base = owner_count(payload);
            let pool = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
            let future = molt_concurrent_threadpool_submit(pool, callable, payload);
            REENTRANT_FUTURE.store(to_i64(obj_from_bits(future)).unwrap(), Ordering::Release);
            molt_concurrent_future_add_done_callback(future, callback);
            assert_eq!(owner_count(callable), callable_base + 1);
            assert_eq!(owner_count(payload), payload_base + 1);
            assert_eq!(owner_count(callback), callback_base + 1);
            molt_concurrent_threadpool_shutdown(
                pool,
                MoltObject::from_bool(true).bits(),
                MoltObject::none().bits(),
            );
            assert!(REENTRANT_DISPATCH_OK.load(Ordering::Acquire));
            assert!(REENTRANT_CALLBACK_OK.load(Ordering::Acquire));
            assert_eq!(CALLBACK_CALLS.load(Ordering::Acquire), 1);
            assert_eq!(owner_count(callable), callable_base);
            assert_eq!(owner_count(callback), callback_base);
            assert_eq!(owner_count(payload), payload_base + 1);
            molt_concurrent_future_drop(future);
            assert_eq!(owner_count(payload), payload_base);
            for bits in [callable, callback, payload] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn nonwaiting_shutdown_keeps_worker_join_custody_and_clear_releases_all_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            concurrent_clear_runtime_state(py, state);
            CALLBACK_CALLS.store(0, Ordering::Release);
            let callable = native_callable(py, retained_argument as *const ());
            let callback = native_callable(py, owned_callback_result as *const ());
            let payload = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let baselines = [
                owner_count(callable),
                owner_count(callback),
                owner_count(payload),
            ];
            let pool = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
            let future = molt_concurrent_threadpool_submit(pool, callable, payload);
            molt_concurrent_future_add_done_callback(future, callback);
            molt_concurrent_threadpool_shutdown(
                pool,
                MoltObject::from_bool(false).bits(),
                MoltObject::none().bits(),
            );
            {
                let pools = state.concurrent.pools.lock().unwrap();
                let pool = pools
                    .get(&to_i64(obj_from_bits(pool)).unwrap())
                    .expect("runtime retains nonwaiting pool");
                assert!(pool.shutdown);
                assert_eq!(pool._workers.len(), 1);
            }
            assert!(concurrent_clear_runtime_state(py, state));
            assert_eq!(
                CALLBACK_CALLS.load(Ordering::Acquire),
                1,
                "clear joined the actual submitted callback"
            );
            assert_eq!(
                [
                    owner_count(callable),
                    owner_count(callback),
                    owner_count(payload)
                ],
                baselines
            );
            assert!(
                !concurrent_clear_runtime_state(py, state),
                "owner drain reaches fixed point"
            );
            for bits in [callable, callback, payload] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn concurrent_runtime_state_is_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            concurrent_clear_runtime_state(_py, state);

            let one_worker = MoltObject::from_int(1).bits();
            let first_pool = molt_concurrent_threadpool_new(one_worker);
            let second_pool = molt_concurrent_threadpool_new(one_worker);
            let first_pool_id = to_i64(obj_from_bits(first_pool)).unwrap();
            let second_pool_id = to_i64(obj_from_bits(second_pool)).unwrap();
            assert_eq!(second_pool_id, first_pool_id + 1);
            assert_eq!(state.concurrent.pools.lock().unwrap().len(), 2);

            let future_id = next_future_id(_py);
            state
                .concurrent
                .futures
                .lock()
                .unwrap()
                .insert(future_id, Arc::new(Mutex::new(FutureState::new())));
            assert_eq!(state.concurrent.futures.lock().unwrap().len(), 1);

            concurrent_clear_runtime_state(_py, state);
            assert!(state.concurrent.pools.lock().unwrap().is_empty());
            assert!(state.concurrent.futures.lock().unwrap().is_empty());
            assert_eq!(
                state.concurrent.next_pool_id.load(Ordering::Acquire),
                second_pool_id + 1
            );
            assert_eq!(
                state.concurrent.next_future_id.load(Ordering::Acquire),
                future_id + 1
            );

            let new_pool = molt_concurrent_threadpool_new(one_worker);
            assert_eq!(to_i64(obj_from_bits(new_pool)), Some(second_pool_id + 1));
            assert_eq!(next_future_id(_py), future_id + 1);
            assert!(get_future(_py, future_id).is_none());
            assert!(
                !state
                    .concurrent
                    .pools
                    .lock()
                    .unwrap()
                    .contains_key(&first_pool_id)
            );
            let true_bits = MoltObject::from_bool(true).bits();
            let false_bits = MoltObject::from_bool(false).bits();
            let _ = molt_concurrent_threadpool_shutdown(new_pool, true_bits, false_bits);
        });
    }
}
