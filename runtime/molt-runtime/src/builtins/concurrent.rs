// === FILE: runtime/molt-runtime/src/builtins/concurrent.rs ===
//! `concurrent.futures` intrinsics for Molt.
//!
//! Provides ThreadPoolExecutor and Future handle management.
//! Work items are callable bits dispatched to OS threads via crossbeam-channel.
//! Future results are stored in shared Arc<Mutex<FutureState>> cells.
//!
//! ABI: NaN-boxed u64 in/out.  Handles are opaque i64 IDs stored in
//! runtime-owned, mutex-protected maps shared by executor and caller threads.

use crate::audit::AuditArgs;
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

// ── Future state ──────────────────────────────────────────────────────────

#[derive(Debug)]
enum FutureOutcome {
    /// Task is pending / in flight.
    Pending,
    /// Task completed successfully; holds the result bits.
    Done(u64),
    /// Task raised an exception; owns the original Python exception object.
    Exception(u64),
    /// Task was cancelled before it started.
    Cancelled,
}

struct FutureState {
    outcome: FutureOutcome,
    running: bool,
    registered: bool,
    callbacks: Vec<u64>, // owned callable bits to fire when done
    completion: Arc<Condvar>,
}

impl FutureState {
    fn new() -> Self {
        Self {
            outcome: FutureOutcome::Pending,
            running: false,
            registered: true,
            callbacks: Vec::new(),
            completion: Arc::new(Condvar::new()),
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
        let result = if matches!(
            state.outcome,
            FutureOutcome::Done(_) | FutureOutcome::Exception(_)
        ) {
            match std::mem::replace(&mut state.outcome, FutureOutcome::Cancelled) {
                FutureOutcome::Done(bits) | FutureOutcome::Exception(bits) => Some(bits),
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
    future_id: i64,
    /// Owned callable and argument references retained through worker dispatch.
    fn_bits: u64,
    args_bits: u64,
}

// ── ThreadPool state ──────────────────────────────────────────────────────

struct ThreadPoolState {
    sender: Sender<Option<WorkItem>>, // None = shutdown sentinel
    receiver: Receiver<Option<WorkItem>>,
    workers: crate::state::lifecycle::RuntimeServiceWorkers,
    shutdown: AtomicBool,
    pending: Mutex<HashMap<i64, SharedFuture>>,
    limit: WorkerLimit,
    submission: Mutex<()>,
    // One obligation per accepted/reserved item, through callbacks and all
    // execution cleanup. This sizes demand; queue/owner locks publish objects.
    unfinished: AtomicUsize,
}

/// Native ceilings need no Python owner. Other admitted limits preserve their
/// original comparison protocol and exactly one pool-owned reference.
enum WorkerLimit {
    Native(usize),
    Dynamic(AtomicU64),
}

impl WorkerLimit {
    fn new(py: &PyToken<'_>, bits: u64) -> Result<Self, u64> {
        let class = type_of_bits(py, bits);
        let classes = builtin_classes(py);
        if class == classes.int || class == classes.bool {
            let payload = crate::builtins::numbers::index_integral_payload_bits(bits)
                .expect("exact builtin integer storage");
            let maximum = if let Some(value) = to_i64(obj_from_bits(payload)) {
                if value <= 0 {
                    return Err(raise_exception::<u64>(
                        py,
                        "ValueError",
                        "max_workers must be greater than 0",
                    ));
                }
                usize::try_from(value).unwrap_or(usize::MAX)
            } else {
                use num_traits::ToPrimitive;
                let ptr = crate::builtins::numbers::bigint_ptr_from_bits(payload)
                    .expect("exact bigint storage");
                let value = unsafe { crate::builtins::numbers::bigint_ref(ptr) };
                if value.sign() != num_bigint::Sign::Plus {
                    return Err(raise_exception::<u64>(
                        py,
                        "ValueError",
                        "max_workers must be greater than 0",
                    ));
                }
                value.to_usize().unwrap_or(usize::MAX)
            };
            return Ok(Self::Native(maximum));
        }
        if class == classes.float {
            let value =
                as_float_extended(obj_from_bits(bits)).expect("exact builtin float storage");
            if value <= 0.0 {
                return Err(raise_exception::<u64>(
                    py,
                    "ValueError",
                    "max_workers must be greater than 0",
                ));
            }
            // Integer count < positive real permits ceil(real) workers. Rust's
            // saturating cast maps +inf to the physical count maximum and NaN
            // to zero, matching their respective true/false count comparisons.
            return Ok(Self::Native(value.ceil() as usize));
        }
        // Python already performed the observable max_workers <= 0 check.
        // Do not repeat it or coerce custom/Decimal/Fraction protocols here.
        inc_ref_bits(py, bits);
        Ok(Self::Dynamic(AtomicU64::new(bits)))
    }

    fn release(&self, py: &PyToken<'_>) {
        if let Self::Dynamic(owner) = self {
            // Concurrent public shutdown and the reaper may both observe joined
            // empty state. Detach before callbacks so retirement remains once-only.
            let bits = owner.swap(MoltObject::none().bits(), Ordering::AcqRel);
            dec_ref_bits(py, bits);
        }
    }
}

// ── Handle-id counter ─────────────────────────────────────────────────────

pub(crate) struct ConcurrentRuntimeState {
    next_pool_id: AtomicI64,
    next_future_id: AtomicI64,
    pools: Mutex<HashMap<i64, Arc<ThreadPoolState>>>,
    futures: Mutex<HashMap<i64, SharedFuture>>,
    // IDs are scheduling hints only; pools remains the sole lifetime owner.
    stopped: Mutex<StoppedPools>,
    stopped_pending: AtomicBool,
}

#[derive(Default)]
struct StoppedPools {
    ids: VecDeque<i64>,
    #[cfg(not(target_arch = "wasm32"))]
    reaping: bool,
}

impl ConcurrentRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            next_pool_id: AtomicI64::new(1),
            next_future_id: AtomicI64::new(1),
            pools: Mutex::new(HashMap::new()),
            futures: Mutex::new(HashMap::new()),
            stopped: Mutex::new(StoppedPools::default()),
            stopped_pending: AtomicBool::new(false),
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

fn next_future_id(state: &ConcurrentRuntimeState) -> i64 {
    state.next_future_id.fetch_add(1, Ordering::Relaxed)
}

fn pool_registry(_py: &PyToken<'_>) -> &'static Mutex<HashMap<i64, Arc<ThreadPoolState>>> {
    &concurrent_state(_py).pools
}

fn future_registry(_py: &PyToken<'_>) -> &'static Mutex<HashMap<i64, SharedFuture>> {
    &concurrent_state(_py).futures
}

enum PoolSubmission {
    Accepted,
    Rejected,
    NoCapacity,
    AdmissionClosed,
    ComparisonFailed,
    SpawnFailed(crate::state::lifecycle::RuntimeServiceGrowthError),
}

impl ThreadPoolState {
    fn finish_work(&self) {
        let previous = self.unfinished.fetch_sub(1, Ordering::Relaxed);
        assert_ne!(previous, 0, "executor work obligation retired twice");
    }

    /// Only submit/stop use this callback-bearing gate. Shared worker handle
    /// locks stay short and callback-free; a contending Python caller must let
    /// the current comparator reacquire GIL after a nested blocking operation.
    fn submission_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.submission.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(error)) => {
                panic!("executor submission gate poisoned: {error}")
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                #[cfg(test)]
                tests::SUBMISSION_WAITING.with(|slot| {
                    if let Some(waiting) = slot.borrow_mut().take() {
                        waiting.send(()).unwrap();
                    }
                });
                let _release = GilReleaseGuard::suspend();
                self.submission.lock().unwrap()
            }
        }
    }

    fn stop(&self, cancel_pending: bool) -> (bool, HashMap<i64, SharedFuture>) {
        let _submission = self.submission_guard();
        let pending = if cancel_pending {
            std::mem::take(&mut *self.pending.lock().unwrap())
        } else {
            HashMap::new()
        };
        let started = !self.shutdown.swap(true, Ordering::AcqRel);
        if started {
            for _ in 0..self.workers.count() {
                let _ = self.sender.send(None);
            }
        }
        (started, pending)
    }
}

fn schedule_stopped_pool(state: &ConcurrentRuntimeState, id: i64) {
    let mut stopped = state.stopped.lock().unwrap();
    stopped.ids.push_back(id);
    state.stopped_pending.store(true, Ordering::Release);
}

/// A poll owns only one worklist ID. The map retains the actual pool until
/// joins and callback-bearing queued-owner release both succeed. Reentrant
/// polls do no work; unwind restores this ID for a later custody-held retry.
#[cfg(not(target_arch = "wasm32"))]
struct StoppedPoolPoll<'a> {
    state: &'a ConcurrentRuntimeState,
    id: i64,
    retry: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for StoppedPoolPoll<'_> {
    fn drop(&mut self) {
        let mut stopped = self.state.stopped.lock().unwrap();
        if self.retry {
            stopped.ids.push_back(self.id);
        }
        stopped.reaping = false;
        self.state
            .stopped_pending
            .store(!stopped.ids.is_empty(), Ordering::Release);
    }
}

/// A bounded FIFO retirement round. Hot safepoints use eight native-worker
/// records and eight queued messages; cold create/shutdown budgets scale with
/// worker count. Callback execution itself is not preempted. The caller holds
/// execution custody for this exact runtime across any callback reentry.
#[inline]
pub(crate) fn concurrent_reap_stopped_pools(
    py: &PyToken<'_>,
    state: &ConcurrentRuntimeState,
    budget: usize,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if state.stopped_pending.load(Ordering::Acquire) {
        reap_stopped_pools_pending(py, state, budget);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (py, state, budget);
}

// Keep the empty safepoint outside the callback/join frame. The same pending
// flag and queue remain authoritative; no auxiliary readiness state is needed.
#[cfg(not(target_arch = "wasm32"))]
#[cold]
#[inline(never)]
fn reap_stopped_pools_pending(py: &PyToken<'_>, state: &ConcurrentRuntimeState, mut budget: usize) {
    use crate::state::lifecycle::RuntimeServiceJoinStatus;
    while budget != 0 && state.stopped_pending.load(Ordering::Acquire) {
        let quantum = budget.min(8);
        let id = {
            let mut stopped = state.stopped.lock().unwrap();
            if stopped.reaping {
                return;
            }
            let Some(id) = stopped.ids.pop_front() else {
                state.stopped_pending.store(false, Ordering::Release);
                return;
            };
            stopped.reaping = true;
            id
        };
        let mut poll = StoppedPoolPoll {
            state,
            id,
            retry: true,
        };
        let pool = state.pools.lock().unwrap().get(&id).cloned();
        let Some(pool) = pool else {
            // Explicit waiting shutdown may have consumed this queued ID.
            poll.retry = false;
            budget -= 1;
            continue;
        };
        // The empty fast path above avoids even error-custody TLS work.
        // Actual queued-owner destruction must preserve an in-flight exception.
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            let (status, inspected) = pool.workers.reap_finished(quantum);
            // Every turn costs at least one unit, including a stale/empty ID.
            // Closed pools reserve the same quantum for queued-message cleanup.
            budget -= if status == RuntimeServiceJoinStatus::Closed {
                quantum
            } else {
                inspected.max(1)
            };
            match status {
                RuntimeServiceJoinStatus::Pending => {}
                RuntimeServiceJoinStatus::Failed => {
                    // Preserve the original map owner/disposition, with no further
                    // automatic attempts. Explicit shutdown still reports failure.
                    poll.retry = false;
                }
                RuntimeServiceJoinStatus::Closed => {
                    if release_stopped_pool_work_bounded(py, &pool, quantum) {
                        state.pools.lock().unwrap().remove(&id);
                        poll.retry = false;
                    }
                }
            }
        });
    }
}

pub(crate) fn concurrent_stop_runtime_workers(state: &crate::state::RuntimeState) -> bool {
    // Runtime teardown closes origin admission before taking this cohort.
    // Never retain the registry mutex while waiting for a pool's comparator.
    let pools: Vec<_> = state
        .concurrent
        .pools
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    for pool in &pools {
        pool.stop(false);
    }
    !pools.is_empty()
}

pub(crate) fn concurrent_join_runtime_workers(state: &crate::state::RuntimeState) -> bool {
    let pools: Vec<_> = state
        .concurrent
        .pools
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    let mut closed = true;
    for pool in pools {
        closed &= pool.workers.join();
    }
    closed
}

fn release_stopped_pool_work(py: &PyToken<'_>, pool: &ThreadPoolState) {
    while !release_stopped_pool_work_bounded(py, pool, 8) {}
}

fn release_stopped_pool_work_bounded(
    py: &PyToken<'_>,
    pool: &ThreadPoolState,
    budget: usize,
) -> bool {
    for _ in 0..budget {
        let Ok(message) = pool.receiver.try_recv() else {
            pool.limit.release(py);
            return true;
        };
        if let Some(item) = message {
            pool.pending.lock().unwrap().remove(&item.future_id);
            {
                let mut future = item.future.lock().unwrap();
                if matches!(future.outcome, FutureOutcome::Pending) {
                    future.outcome = FutureOutcome::Cancelled;
                }
                future.completion.notify_all();
            }
            release_future_owners(py, &item.future, true);
            dec_ref_bits(py, item.fn_bits);
            dec_ref_bits(py, item.args_bits);
            pool.finish_work();
        }
    }
    // All native workers joined before this drain. Submit refuses a stopped
    // pool, including finalizer reentry, so no live producer can refill it.
    let empty = pool.receiver.is_empty();
    if empty {
        pool.limit.release(py);
    }
    empty
}

pub(crate) fn concurrent_clear_runtime_state(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    // Preserve the exact owner cohort across GIL-releasing joins. Production
    // teardown has closed admission; ready-runtime test drains may run callbacks
    // that publish a later cohort, which must remain visible and owned.
    let pools = state.concurrent.pools.lock().unwrap().clone();
    let futures = state.concurrent.futures.lock().unwrap().clone();
    for pool in pools.values() {
        pool.stop(false);
    }
    let closed = {
        let _release = GilReleaseGuard::suspend();
        pools
            .values()
            .fold(true, |closed, pool| pool.workers.join() && closed)
    };
    assert!(closed, "runtime executor workers failed; origin retained");
    {
        let mut stopped = state.concurrent.stopped.lock().unwrap();
        stopped.ids.retain(|id| !pools.contains_key(id));
        state
            .concurrent
            .stopped_pending
            .store(!stopped.ids.is_empty(), Ordering::Release);
    }
    let changed = !pools.is_empty() || !futures.is_empty();
    for (id, pool) in pools {
        state.concurrent.pools.lock().unwrap().remove(&id);
        release_stopped_pool_work(py, &pool);
    }
    for (id, future) in futures {
        state.concurrent.futures.lock().unwrap().remove(&id);
        release_future_owners(py, &future, true);
    }
    changed
}

// ── Worker thread loop ────────────────────────────────────────────────────
//
// Workers receive WorkItem packets.  They acquire the GIL to call the
// Python callable, then release and store the result.  This matches
// CPython's ThreadPoolExecutor model.

#[cfg(not(target_arch = "wasm32"))]
fn worker_loop(
    origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    pool: Arc<ThreadPoolState>,
) {
    while let Ok(Some(item)) = pool.receiver.recv() {
        let Some(execution) =
            crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
        else {
            assert!(pool.sender.send(Some(item)).is_ok());
            break;
        };
        let token = execution.token();
        let py = &token;
        pool.pending.lock().unwrap().remove(&item.future_id);
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
        } else {
            let bits = if obj_from_bits(item.args_bits).is_none() {
                unsafe { call_callable0(py, item.fn_bits) }
            } else {
                unsafe { call_callable1(py, item.fn_bits, item.args_bits) }
            };
            if exception_pending(py) {
                let exc_bits = molt_exception_last_pending();
                clear_exception(py);
                dec_ref_bits(py, bits);
                FutureOutcome::Exception(exc_bits)
            } else {
                FutureOutcome::Done(bits)
            }
        };
        let (callbacks, discarded_result) = {
            let mut state = item.future.lock().unwrap();
            state.running = false;
            let discarded_result = match result {
                FutureOutcome::Done(bits) | FutureOutcome::Exception(bits) if !state.registered => {
                    state.outcome = FutureOutcome::Cancelled;
                    Some(bits)
                }
                outcome => {
                    state.outcome = outcome;
                    None
                }
            };
            state.completion.notify_all();
            (std::mem::take(&mut state.callbacks), discarded_result)
        };
        if let Some(bits) = discarded_result {
            dec_ref_bits(py, bits);
        }
        for cb_bits in callbacks {
            let out = unsafe { call_callable1(py, cb_bits, int_bits_from_i64(py, item.future_id)) };
            if exception_pending(py) {
                clear_exception(py);
            }
            dec_ref_bits(py, out);
            dec_ref_bits(py, cb_bits);
        }
        dec_ref_bits(py, item.fn_bits);
        dec_ref_bits(py, item.args_bits);
        // WorkerEnd cleanup may run finalizers that submit and wait. Keep this
        // worker counted as busy until that callback-bearing boundary closes.
        drop(execution);
        pool.finish_work();
    }
}

// ── ThreadPoolExecutor intrinsics ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_new(max_workers_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(error) =
            require_operation(_py, crate::OperationId::ThreadSpawnShared, AuditArgs::None)
        {
            return error;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = max_workers_bits;
            raise_exception::<u64>(_py, "RuntimeError", "thread pools unsupported on wasm")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let state = runtime_state(_py);
            if !state.thread_borrow_admission_is_open() {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "runtime thread admission is closed",
                );
            }
            let limit = match WorkerLimit::new(_py, max_workers_bits) {
                Ok(limit) => limit,
                Err(error) => return error,
            };
            // Creation has no native startup work. Actual submissions service
            // additional bounded retirement debt independently of capacity.
            concurrent_reap_stopped_pools(_py, &state.concurrent, 8);
            let (sender, receiver) = unbounded::<Option<WorkItem>>();
            let pool = Arc::new(ThreadPoolState {
                sender,
                receiver,
                workers: crate::state::lifecycle::RuntimeServiceWorkers::default(),
                shutdown: AtomicBool::new(false),
                pending: Mutex::new(HashMap::new()),
                limit,
                submission: Mutex::new(()),
                unfinished: AtomicUsize::new(0),
            });
            let id = next_pool_id(_py);
            let published = {
                let mut pools = pool_registry(_py).lock().unwrap();
                // Pair closure with registry publication: a delayed constructor
                // cannot appear after teardown's closed-admission snapshot.
                if state.thread_borrow_admission_is_open() {
                    pools.insert(id, Arc::clone(&pool));
                    true
                } else {
                    false
                }
            };
            if !published {
                pool.limit.release(_py);
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "runtime thread admission is closed",
                );
            }
            int_bits_from_i64(_py, id)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_submit(
    handle_bits: u64,
    fn_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let runtime = runtime_state(_py);
        #[cfg(not(target_arch = "wasm32"))]
        if !runtime.thread_borrow_admission_is_open() {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "runtime thread admission is closed",
            );
        }
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };

        let state = &runtime.concurrent;
        concurrent_reap_stopped_pools(_py, state, 2);
        let future_shared = Arc::new(Mutex::new(FutureState::new()));
        let future_id = next_future_id(state);

        inc_ref_bits(_py, fn_bits);
        inc_ref_bits(_py, args_bits);
        let submission = 'publish: {
            let pool = state.pools.lock().unwrap().get(&pool_id).cloned();
            let Some(pool) = pool else {
                break 'publish PoolSubmission::Rejected;
            };
            let _submission = pool.submission_guard();
            #[cfg(not(target_arch = "wasm32"))]
            if !runtime.thread_borrow_admission_is_open() {
                break 'publish PoolSubmission::AdmissionClosed;
            }
            if pool.shutdown.load(Ordering::Acquire) {
                break 'publish PoolSubmission::Rejected;
            }
            if pool
                .unfinished
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    count.checked_add(1)
                })
                .is_err()
            {
                break 'publish PoolSubmission::NoCapacity;
            }
            let item = WorkItem {
                future: Arc::clone(&future_shared),
                future_id,
                fn_bits,
                args_bits,
            };
            pool.pending
                .lock()
                .unwrap()
                .insert(future_id, Arc::clone(&future_shared));
            // A worker must be able to resolve its public future as soon as the
            // channel makes the job visible, including without a global GIL.
            // Transfer the initial owner; failure takes it back from the registry
            // unless a reentrant public drop has already retired that owner.
            state
                .futures
                .lock()
                .unwrap()
                .insert(future_id, future_shared);
            if pool.sender.send(Some(item)).is_err() {
                pool.pending.lock().unwrap().remove(&future_id);
                state.futures.lock().unwrap().remove(&future_id);
                pool.finish_work();
                break 'publish PoolSubmission::Rejected;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let maximum = match &pool.limit {
                    WorkerLimit::Native(maximum) => *maximum,
                    WorkerLimit::Dynamic(owner) => {
                        let count = pool.workers.count();
                        if count >= pool.unfinished.load(Ordering::Relaxed) {
                            break 'publish PoolSubmission::Accepted;
                        }
                        use crate::object::ops_compare::{
                            CompareBoolOutcome, CompareOp, rich_compare_op_bool,
                        };
                        let count = int_bits_from_i64(
                            _py,
                            i64::try_from(count).expect("addressable worker count"),
                        );
                        let compared = rich_compare_op_bool(
                            _py,
                            obj_from_bits(count),
                            obj_from_bits(owner.load(Ordering::Acquire)),
                            CompareOp::Lt,
                        );
                        dec_ref_bits(_py, count);
                        match compared {
                            CompareBoolOutcome::True => usize::MAX,
                            CompareBoolOutcome::False => break 'publish PoolSubmission::Accepted,
                            CompareBoolOutcome::Error => {
                                break 'publish PoolSubmission::ComparisonFailed;
                            }
                            CompareBoolOutcome::NotComparable => {
                                unreachable!("ordering authority supplies TypeError")
                            }
                        }
                    }
                };
                let origin = crate::state::runtime_state::RuntimeWorkerOrigin::new(runtime);
                let started =
                    pool.workers
                        .grow_for_demand(origin, &pool.unfinished, maximum, |index| {
                            let pool = Arc::clone(&pool);
                            crate::state::lifecycle::RuntimeServiceWorker::spawn(
                                origin,
                                thread::Builder::new().name(format!("molt-executor-{index}")),
                                move || worker_loop(origin, pool),
                            )
                        });
                if let Err(error) = started {
                    break 'publish PoolSubmission::SpawnFailed(error);
                }
            }
            PoolSubmission::Accepted
        };
        match submission {
            PoolSubmission::Accepted => {}
            PoolSubmission::ComparisonFailed => {
                let future = state.futures.lock().unwrap().remove(&future_id);
                if let Some(future) = future {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        release_future_owners(_py, &future, false)
                    });
                }
                return MoltObject::none().bits();
            }
            PoolSubmission::SpawnFailed(error) => {
                // Match queue-before-start semantics: the accepted job remains
                // executable and later submissions may retry native growth.
                // Only the never-returned public handle is unpublished. The
                // queued Arc retains result/argument retirement independently.
                let future = state.futures.lock().unwrap().remove(&future_id);
                if let Some(future) = future {
                    release_future_owners(_py, &future, false);
                }
                if matches!(
                    error,
                    crate::state::lifecycle::RuntimeServiceGrowthError::Capacity
                ) {
                    return raise_exception::<u64>(_py, "MemoryError", "");
                }
                return raise_exception::<u64>(_py, "RuntimeError", "can't start new thread");
            }
            failure @ (PoolSubmission::Rejected
            | PoolSubmission::NoCapacity
            | PoolSubmission::AdmissionClosed) => {
                dec_ref_bits(_py, fn_bits);
                dec_ref_bits(_py, args_bits);
                if matches!(failure, PoolSubmission::NoCapacity) {
                    return raise_exception::<u64>(_py, "MemoryError", "");
                }
                if matches!(failure, PoolSubmission::AdmissionClosed) {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "runtime thread admission is closed",
                    );
                }
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "cannot submit to a shut-down executor",
                );
            }
        }

        int_bits_from_i64(_py, future_id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_threadpool_shutdown(
    handle_bits: u64,
    wait_bits: u64,
    cancel_futures_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pool_id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "thread pool handle must be int");
            }
        };
        let wait = is_truthy(_py, obj_from_bits(wait_bits));
        let cancel_futures = is_truthy(_py, obj_from_bits(cancel_futures_bits));
        let state = concurrent_state(_py);
        let pool = state.pools.lock().unwrap().get(&pool_id).cloned();
        let pending = if let Some(pool) = &pool {
            let (started, pending) = pool.stop(cancel_futures);
            if started {
                schedule_stopped_pool(state, pool_id);
            }
            pending
        } else {
            HashMap::new()
        };
        let retirement_budget = pool.as_ref().map_or(8, |pool| pool.workers.count() * 2 + 8);
        for (id, future) in pending {
            cancel_future(_py, &future, int_bits_from_i64(_py, id));
        }
        if wait && let Some(pool) = pool {
            let closed = {
                let _release = GilReleaseGuard::suspend();
                pool.workers.join()
            };
            if !closed {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "executor worker did not close; owner retained",
                );
            }
            release_stopped_pool_work(_py, &pool);
            state.pools.lock().unwrap().remove(&pool_id);
        }
        concurrent_reap_stopped_pools(_py, state, retirement_budget);
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

fn wait_for_future(future: &SharedFuture, timeout: Option<Duration>) -> Result<(), ()> {
    let state = future.lock().unwrap();
    let completion = Arc::clone(&state.completion);
    if let Some(timeout) = timeout {
        let (state, _) = completion
            .wait_timeout_while(state, timeout, |state| !state.is_done())
            .unwrap();
        if state.is_done() { Ok(()) } else { Err(()) }
    } else {
        drop(
            completion
                .wait_while(state, |state| !state.is_done())
                .unwrap(),
        );
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FutureReadKind {
    Result,
    Exception,
}

enum FutureReadOutcome {
    /// Each payload is an independent owner, acquired with the GIL held.
    Return(u64),
    Raise(u64),
    Cancelled,
    TimedOut,
}

fn snapshot_future_outcome(
    py: &PyToken<'_>,
    future: &SharedFuture,
    kind: FutureReadKind,
) -> Option<FutureReadOutcome> {
    let state = future.lock().unwrap();
    Some(match state.outcome {
        FutureOutcome::Done(bits) => match kind {
            FutureReadKind::Result => {
                inc_ref_bits(py, bits);
                FutureReadOutcome::Return(bits)
            }
            FutureReadKind::Exception => FutureReadOutcome::Return(MoltObject::none().bits()),
        },
        FutureOutcome::Exception(bits) => {
            inc_ref_bits(py, bits);
            match kind {
                FutureReadKind::Result => FutureReadOutcome::Raise(bits),
                FutureReadKind::Exception => FutureReadOutcome::Return(bits),
            }
        }
        FutureOutcome::Cancelled => FutureReadOutcome::Cancelled,
        FutureOutcome::Pending => return None,
    })
}

fn read_future_outcome(
    py: &PyToken<'_>,
    future: &SharedFuture,
    timeout_bits: u64,
    kind: FutureReadKind,
) -> Result<FutureReadOutcome, u64> {
    // Terminal reads never inspect timeout, including user comparison/index
    // methods. Snapshot/retain under the GIL, then release the mutex.
    if let Some(outcome) = snapshot_future_outcome(py, future, kind) {
        return Ok(outcome);
    }
    let timeout = crate::builtins::threading_helpers::parse_thread_timeout(
        py,
        timeout_bits,
        crate::builtins::threading_helpers::ThreadTimeoutPolicy::Condition,
    )?;
    {
        let _release = GilReleaseGuard::suspend();
        let _ = wait_for_future(future, timeout);
    }
    // Reacquire the GIL before retaining; a completion/cancel delivered while
    // reacquiring takes precedence over timeout, matching Condition.wait.
    Ok(snapshot_future_outcome(py, future, kind).unwrap_or(FutureReadOutcome::TimedOut))
}

fn raise_future_error(py: &PyToken<'_>, error_class: u64) -> u64 {
    // The Python module owns exception identity. Construct its class through
    // normal call dispatch, outside the future lock, with no dotted-name cache.
    let exception = unsafe { call_callable0(py, error_class) };
    if exception_pending(py) {
        dec_ref_bits(py, exception);
        return MoltObject::none().bits();
    }
    let result = molt_raise(exception);
    dec_ref_bits(py, exception);
    result
}

fn read_future(
    py: &PyToken<'_>,
    handle_bits: u64,
    timeout_bits: u64,
    cancelled_error: u64,
    timeout_error: u64,
    kind: FutureReadKind,
) -> u64 {
    let id = match to_i64(obj_from_bits(handle_bits)) {
        Some(v) => v,
        None => return raise_exception::<u64>(py, "TypeError", "future handle must be int"),
    };
    let future = match get_future(py, id) {
        Some(f) => f,
        None => return raise_exception::<u64>(py, "ValueError", "invalid future handle"),
    };
    let outcome = match read_future_outcome(py, &future, timeout_bits, kind) {
        Ok(outcome) => outcome,
        Err(bits) => return bits,
    };
    match outcome {
        FutureReadOutcome::Return(bits) => bits,
        FutureReadOutcome::Raise(bits) => {
            let result = molt_raise(bits);
            dec_ref_bits(py, bits);
            result
        }
        FutureReadOutcome::Cancelled => raise_future_error(py, cancelled_error),
        FutureReadOutcome::TimedOut => raise_future_error(py, timeout_error),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_result(
    handle_bits: u64,
    timeout_bits: u64,
    cancelled_error: u64,
    timeout_error: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        read_future(
            py,
            handle_bits,
            timeout_bits,
            cancelled_error,
            timeout_error,
            FutureReadKind::Result,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concurrent_future_exception(
    handle_bits: u64,
    timeout_bits: u64,
    cancelled_error: u64,
    timeout_error: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        read_future(
            py,
            handle_bits,
            timeout_bits,
            cancelled_error,
            timeout_error,
            FutureReadKind::Exception,
        )
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
            Some(f) => MoltObject::from_bool(cancel_future(_py, &f, handle_bits)).bits(),
        }
    })
}

fn cancel_future(py: &PyToken<'_>, future: &SharedFuture, handle: u64) -> bool {
    let callbacks = {
        let mut state = future.lock().unwrap();
        if state.is_cancelled() {
            return true;
        }
        if state.is_done() || state.running {
            return false;
        }
        state.outcome = FutureOutcome::Cancelled;
        state.completion.notify_all();
        std::mem::take(&mut state.callbacks)
    };
    for callback in callbacks {
        let result = unsafe { call_callable1(py, callback, handle) };
        if exception_pending(py) {
            clear_exception(py);
        }
        dec_ref_bits(py, result);
        dec_ref_bits(py, callback);
    }
    true
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
            return MoltObject::none().bits();
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
            return MoltObject::none().bits();
        }
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_ptr(done_set_ptr).bits(),
                MoltObject::from_ptr(not_done_set_ptr).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    static REENTRANT_FUTURE: AtomicI64 = AtomicI64::new(0);
    static REENTRANT_DISPATCH_OK: AtomicBool = AtomicBool::new(false);
    static REENTRANT_CALLBACK_OK: AtomicBool = AtomicBool::new(false);
    static CALLBACK_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DEMAND_RELEASE: AtomicBool = AtomicBool::new(false);
    static DEMAND_STARTED: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_POOL: AtomicI64 = AtomicI64::new(0);
    static NESTED_CALLBACK_OK: AtomicBool = AtomicBool::new(false);
    static LIMIT_BEHAVIOR: AtomicUsize = AtomicUsize::new(0);
    static LIMIT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static LIMIT_OTHER_POOL: AtomicI64 = AtomicI64::new(0);
    static LIMIT_CREATED_POOL: AtomicI64 = AtomicI64::new(0);
    static LIMIT_ERROR: AtomicU64 = AtomicU64::new(0);
    thread_local! {
        pub(super) static SUBMISSION_WAITING: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
        static LIMIT_ENTERED: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
    }

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

    extern "C" fn blocked_argument(bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            DEMAND_STARTED.fetch_add(1, Ordering::AcqRel);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !DEMAND_RELEASE.load(Ordering::Acquire) {
                if std::time::Instant::now() >= deadline {
                    return raise_exception::<u64>(
                        py,
                        "RuntimeError",
                        "test work release timed out",
                    );
                }
                let _release = GilReleaseGuard::suspend();
                thread::yield_now();
            }
            retained_argument(bits)
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn nested_submit_callback(_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let pool_id = CALLBACK_POOL.load(Ordering::Acquire);
            let callable = native_callable(py, retained_argument as *const ());
            let future = molt_concurrent_threadpool_submit(
                MoltObject::from_int(pool_id).bits(),
                callable,
                MoltObject::from_int(7).bits(),
            );
            let result = molt_concurrent_future_result(
                future,
                MoltObject::from_float(5.0).bits(),
                builtin_classes(py).exception,
                builtin_classes(py).exception,
            );
            NESTED_CALLBACK_OK.store(
                !exception_pending(py)
                    && to_i64(obj_from_bits(result)) == Some(7)
                    && state_pool_for_test(py, pool_id).workers.count() == 2,
                Ordering::Release,
            );
            if exception_pending(py) {
                clear_exception(py);
            }
            dec_ref_bits(py, result);
            molt_concurrent_future_drop(future);
            dec_ref_bits(py, callable);
            MoltObject::none().bits()
        })
    }

    extern "C" fn limit_greater_than(_limit: u64, count: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            LIMIT_CALLS.fetch_add(1, Ordering::AcqRel);
            match LIMIT_BEHAVIOR.load(Ordering::Acquire) {
                1 => {
                    let result = raise_exception::<u64>(
                        py,
                        "LookupError",
                        "original limit comparison failure",
                    );
                    LIMIT_ERROR.store(molt_exception_last_pending(), Ordering::Release);
                    return result;
                }
                2 => {
                    LIMIT_ENTERED.with(|slot| {
                        if let Some(entered) = slot.borrow_mut().take() {
                            entered.send(()).unwrap();
                        }
                    });
                    // This waits for another real pool and releases the GIL while
                    // retaining only this executor's callback-bearing gate.
                    molt_concurrent_threadpool_shutdown(
                        MoltObject::from_int(LIMIT_OTHER_POOL.load(Ordering::Acquire)).bits(),
                        MoltObject::from_bool(true).bits(),
                        MoltObject::none().bits(),
                    );
                    if exception_pending(py) {
                        return MoltObject::none().bits();
                    }
                }
                _ => {}
            }
            MoltObject::from_bool(to_i64(obj_from_bits(count)).is_some_and(|count| count < 4))
                .bits()
        })
    }

    extern "C" fn limit_destructor_creates_pool(_limit: u64) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            let handle = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
            LIMIT_CREATED_POOL.store(
                to_i64(obj_from_bits(handle)).unwrap_or(-1),
                Ordering::Release,
            );
            MoltObject::none().bits()
        })
    }

    fn dynamic_limit(py: &PyToken<'_>, create_on_drop: bool) -> (u64, u64) {
        let name = attr_name_bits_from_bytes(py, b"ExecutorLimitContract").unwrap();
        let class = molt_class_new(name);
        dec_ref_bits(py, name);
        molt_class_set_base(class, builtin_classes(py).object);
        let mut methods = vec![(b"__gt__".as_slice(), limit_greater_than as *const (), 2)];
        if create_on_drop {
            methods.push((
                b"__del__".as_slice(),
                limit_destructor_creates_pool as *const (),
                1,
            ));
        }
        for (name, address, arity) in methods {
            let name = attr_name_bits_from_bytes(py, name).unwrap();
            let function = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(address),
                arity,
            );
            assert!(!function.is_null());
            let function = MoltObject::from_ptr(function).bits();
            molt_set_attr_name(class, name, function);
            dec_ref_bits(py, name);
            dec_ref_bits(py, function);
        }
        unsafe {
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
        }
        let size = unsafe {
            crate::object::layout::class_cached_layout_size(obj_from_bits(class).as_ptr().unwrap())
                .unwrap()
        };
        let value = crate::object::builders::alloc_class_instance(py, size, class);
        unsafe {
            crate::object::gc::gc_publish_initialized(py, obj_from_bits(value).as_ptr().unwrap());
        }
        assert!(!exception_pending(py));
        (class, value)
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
            // A completed future callback is still running on its pool worker.
            // The canonical poll must preserve self-join refusal and custody.
            crate::molt_async_work_poll_and_exception_pending();
            let id = REENTRANT_FUTURE.load(Ordering::Acquire);
            if let Some(future) = get_future(py, id) {
                let unlocked = future.try_lock().is_ok();
                if unlocked {
                    let handle = MoltObject::from_int(id).bits();
                    let done = molt_concurrent_future_done(handle);
                    let result = molt_concurrent_future_result(
                        handle,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    );
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

    extern "C" fn worker_failure(_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<u64>(py, "ValueError", "original worker failure")
        })
    }

    #[test]
    fn worker_exception_preserves_identity_and_independent_owners() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                concurrent_clear_runtime_state(py, runtime_state(py));
                let callable = native_callable(py, worker_failure as *const ());
                let pool = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
                let future = molt_concurrent_threadpool_submit(
                    pool,
                    callable,
                    MoltObject::from_int(0).bits(),
                );
                molt_concurrent_threadpool_shutdown(
                    pool,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                let exception = molt_concurrent_future_exception(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert!(!exception_pending(py));
                let baseline = owner_count(exception);
                let second = molt_concurrent_future_exception(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert_eq!(second, exception);
                assert_eq!(owner_count(exception), baseline + 1);
                dec_ref_bits(py, second);
                molt_concurrent_future_result(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert!(exception_pending(py));
                let pending = molt_exception_last_pending();
                assert_eq!(pending, exception);
                clear_exception(py);
                dec_ref_bits(py, pending);
                molt_concurrent_future_drop(future);
                // The caller still owns a live exception after its future is gone.
                assert!(obj_from_bits(exception).as_ptr().is_some());
                dec_ref_bits(py, exception);
                dec_ref_bits(py, callable);
            });
        });
    }

    #[test]
    fn future_errors_use_supplied_classes_and_release_temporary_exception_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            concurrent_clear_runtime_state(py, runtime_state(py));
            let new_error_class = |name: &[u8]| {
                let name_bits = MoltObject::from_ptr(alloc_string(py, name)).bits();
                let class = molt_class_new(name_bits);
                dec_ref_bits(py, name_bits);
                molt_class_set_base(class, builtin_classes(py).exception);
                unsafe {
                    crate::object::class_finish_definition(
                        py,
                        obj_from_bits(class).as_ptr().expect("error class"),
                    )
                }
                .expect("seal error class");
                assert!(!exception_pending(py));
                class
            };
            let cancelled_error = new_error_class(b"ModuleCancelledError");
            let timeout_error = new_error_class(b"ModuleTimeoutError");
            let id = next_future_id(concurrent_state(py));
            let handle = MoltObject::from_int(id).bits();
            let future = Arc::new(Mutex::new(FutureState::new()));
            future_registry(py)
                .lock()
                .unwrap()
                .insert(id, future.clone());
            let zero_timeout = MoltObject::from_int(0).bits();
            let readers: [extern "C" fn(u64, u64, u64, u64) -> u64; 2] = [
                molt_concurrent_future_result,
                molt_concurrent_future_exception,
            ];
            for cancelled in [false, true] {
                if cancelled {
                    assert_eq!(
                        molt_concurrent_future_cancel(handle),
                        MoltObject::from_bool(true).bits()
                    );
                }
                for read in readers {
                    let result = read(handle, zero_timeout, cancelled_error, timeout_error);
                    assert_eq!(result, MoltObject::none().bits());
                    assert!(exception_pending(py));
                    let pending = molt_exception_last_pending();
                    let pending_ptr = obj_from_bits(pending).as_ptr().unwrap();
                    assert_eq!(
                        unsafe { object_class_bits(pending_ptr) },
                        if cancelled {
                            cancelled_error
                        } else {
                            timeout_error
                        }
                    );
                    clear_exception(py);
                    let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                    let is_unique = || {
                        bridge
                            .managed_handle_is_uniquely_referenced(pending)
                            .unwrap_or_else(|| unsafe {
                                (*header_from_obj_ptr(pending_ptr)).is_uniquely_owned()
                            })
                    };
                    assert!(is_unique(), "only the returned semantic owner remains");
                    inc_ref_bits(py, pending);
                    assert!(!is_unique(), "an extra runtime owner must be observable");
                    dec_ref_bits(py, pending);
                    dec_ref_bits(py, pending);
                    assert_eq!(
                        bridge.managed_handle_is_uniquely_referenced(pending),
                        None,
                        "last semantic release must retire any canonical C view"
                    );
                    assert_eq!(future.lock().unwrap().is_done(), cancelled);
                }
            }
            molt_concurrent_future_drop(handle);
            dec_ref_bits(py, cancelled_error);
            dec_ref_bits(py, timeout_error);
        });
    }

    #[test]
    fn cancellation_notifies_callbacks_once_before_worker_dispatch() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                concurrent_clear_runtime_state(py, runtime_state(py));
                CALLBACK_CALLS.store(0, Ordering::Release);
                let callable = native_callable(py, retained_argument as *const ());
                let callback = native_callable(py, owned_callback_result as *const ());
                let pool = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
                let future = molt_concurrent_threadpool_submit(
                    pool,
                    callable,
                    MoltObject::from_int(7).bits(),
                );
                molt_concurrent_future_add_done_callback(future, callback);
                assert_eq!(
                    molt_concurrent_future_cancel(future),
                    MoltObject::from_bool(true).bits()
                );
                assert_eq!(CALLBACK_CALLS.load(Ordering::Acquire), 1);
                assert_eq!(
                    molt_concurrent_future_cancel(future),
                    MoltObject::from_bool(true).bits()
                );
                molt_concurrent_threadpool_shutdown(
                    pool,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(true).bits(),
                );
                assert_eq!(CALLBACK_CALLS.load(Ordering::Acquire), 1);
                assert_eq!(
                    molt_concurrent_future_cancelled(future),
                    MoltObject::from_bool(true).bits()
                );
                molt_concurrent_future_drop(future);
                dec_ref_bits(py, callable);
                dec_ref_bits(py, callback);
            });
        });
    }

    #[test]
    fn future_result_returns_independent_owners_without_consuming_stored_result() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
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
                let exception = molt_concurrent_future_exception(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert_eq!(exception, MoltObject::none().bits());
                assert!(!exception_pending(py));
                assert_eq!(
                    owner_count(payload),
                    baseline + 1,
                    "exception() does not retain a successful result"
                );
                let first = molt_concurrent_future_result(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                let second = molt_concurrent_future_result(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
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
        });
    }

    #[test]
    fn worker_dispatch_and_done_callback_can_reenter_the_same_future() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
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
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll_until(py: &PyToken<'_>, done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "executor retirement did not finish"
            );
            {
                let _release = GilReleaseGuard::suspend();
                std::thread::yield_now();
            }
            // Exercise the actual generated-code safepoint consumer.
            assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
            assert!(!exception_pending(py));
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn generic_limit_comparison_failure_keeps_work_and_retires_owner_once() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                LIMIT_BEHAVIOR.store(1, Ordering::Release);
                LIMIT_CALLS.store(0, Ordering::Release);
                let (class, limit) = dynamic_limit(py, false);
                let baseline = owner_count(limit);
                let handle = molt_concurrent_threadpool_new(limit);
                let pool = state_pool_for_test(py, to_i64(obj_from_bits(handle)).unwrap());
                assert_eq!(
                    LIMIT_CALLS.load(Ordering::Acquire),
                    0,
                    "constructor repeated a user comparison"
                );
                assert_eq!(owner_count(limit), baseline + 1);
                let callable = native_callable(py, retained_argument as *const ());
                let failed_id = concurrent_state(py).next_future_id.load(Ordering::Relaxed);
                molt_concurrent_threadpool_submit(handle, callable, MoltObject::from_int(7).bits());
                assert!(exception_pending(py));
                let error = molt_exception_last_pending();
                let original = LIMIT_ERROR.swap(0, Ordering::AcqRel);
                assert_eq!(error, original);
                assert_eq!(class_name_for_error(type_of_bits(py, error)), "LookupError");
                clear_exception(py);
                dec_ref_bits(py, error);
                dec_ref_bits(py, original);
                assert!(get_future(py, failed_id).is_none());
                assert_eq!(pool.unfinished.load(Ordering::Relaxed), 1);
                assert_eq!(pool.workers.count(), 0);
                LIMIT_BEHAVIOR.store(0, Ordering::Release);
                let future = molt_concurrent_threadpool_submit(
                    handle,
                    callable,
                    MoltObject::from_int(9).bits(),
                );
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                assert!(!exception_pending(py));
                assert_eq!(pool.unfinished.load(Ordering::Relaxed), 0);
                assert!(pool.pending.lock().unwrap().is_empty());
                assert_eq!(owner_count(limit), baseline);
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                assert_eq!(owner_count(limit), baseline);
                let result = molt_concurrent_future_result(
                    future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert_eq!(to_i64(obj_from_bits(result)), Some(9));
                dec_ref_bits(py, result);
                molt_concurrent_future_drop(future);
                for bits in [callable, limit, class] {
                    dec_ref_bits(py, bits);
                }
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn limit_reentry_and_contended_submission_release_global_locks_and_gil() {
        use crate::state::lifecycle::run_runtime_worker;
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                LIMIT_BEHAVIOR.store(2, Ordering::Release);
                DEMAND_RELEASE.store(false, Ordering::Release);
                let other = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
                LIMIT_OTHER_POOL.store(to_i64(obj_from_bits(other)).unwrap(), Ordering::Release);
                let blocked = native_callable(py, blocked_argument as *const ());
                let other_future = molt_concurrent_threadpool_submit(
                    other,
                    blocked,
                    MoltObject::from_int(1).bits(),
                );
                let (class, limit) = dynamic_limit(py, false);
                let primary = molt_concurrent_threadpool_new(limit);
                let callable = native_callable(py, retained_argument as *const ());
                let origin =
                    crate::state::runtime_state::RuntimeWorkerOrigin::new(runtime_state(py));
                let (entered_tx, entered_rx) = std::sync::mpsc::channel();
                let first = thread::spawn(move || {
                    run_runtime_worker(origin, || {
                        crate::with_gil_entry_nopanic!(py, {
                            LIMIT_ENTERED.with(|slot| *slot.borrow_mut() = Some(entered_tx));
                            let future = molt_concurrent_threadpool_submit(
                                primary,
                                callable,
                                MoltObject::from_int(7).bits(),
                            );
                            assert!(!exception_pending(py));
                            future
                        })
                    })
                });
                {
                    let _release = GilReleaseGuard::suspend();
                    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
                let second = thread::spawn(move || {
                    run_runtime_worker(origin, || {
                        crate::with_gil_entry_nopanic!(py, {
                            SUBMISSION_WAITING.with(|slot| *slot.borrow_mut() = Some(waiting_tx));
                            let future = molt_concurrent_threadpool_submit(
                                primary,
                                callable,
                                MoltObject::from_int(7).bits(),
                            );
                            assert!(!exception_pending(py));
                            future
                        })
                    })
                });
                {
                    let _release = GilReleaseGuard::suspend();
                    waiting_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                // Reacquiring GIL here proves the actual contended submit did
                // not retain it. The first comparator still owns its pool gate.
                DEMAND_RELEASE.store(true, Ordering::Release);
                let futures = {
                    let _release = GilReleaseGuard::suspend();
                    [first.join().unwrap(), second.join().unwrap()]
                };
                molt_concurrent_threadpool_shutdown(
                    primary,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                assert!(!exception_pending(py));
                for future in futures {
                    let result = molt_concurrent_future_result(
                        future,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    );
                    assert_eq!(to_i64(obj_from_bits(result)), Some(7));
                    dec_ref_bits(py, result);
                    molt_concurrent_future_drop(future);
                }
                molt_concurrent_future_drop(other_future);
                for bits in [blocked, callable, limit, class] {
                    dec_ref_bits(py, bits);
                }
                assert!(pool_registry(py).lock().unwrap().is_empty());
                assert!(future_registry(py).lock().unwrap().is_empty());
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn manual_clear_preserves_limit_finalizer_replacement_cohort() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                LIMIT_CREATED_POOL.store(0, Ordering::Release);
                let (class, limit) = dynamic_limit(py, true);
                let old = molt_concurrent_threadpool_new(limit);
                dec_ref_bits(py, limit); // Pool is now the sole limit owner.
                assert!(concurrent_clear_runtime_state(py, runtime_state(py)));
                let replacement = LIMIT_CREATED_POOL.load(Ordering::Acquire);
                assert!(replacement > to_i64(obj_from_bits(old)).unwrap());
                assert_eq!(pool_registry(py).lock().unwrap().len(), 1);
                assert_eq!(state_pool_for_test(py, replacement).workers.count(), 0);
                assert!(concurrent_clear_runtime_state(py, runtime_state(py)));
                assert!(pool_registry(py).lock().unwrap().is_empty());
                assert!(!exception_pending(py));
                dec_ref_bits(py, class);
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn pool_workers_follow_demand_and_reuse_settled_capacity() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let huge = crate::builtins::numbers::int_bits_from_bigint(
                    py,
                    num_bigint::BigInt::from(1u8) << 100usize,
                );
                let nan = crate::object::ops::float_result_bits(py, f64::NAN);
                for (limit, expected) in [
                    (MoltObject::from_int(513).bits(), 513),
                    (huge, usize::MAX),
                    (MoltObject::from_bool(true).bits(), 1),
                    (MoltObject::from_float(0.5).bits(), 1),
                    (MoltObject::from_float(3.5).bits(), 4),
                    (MoltObject::from_float(f64::INFINITY).bits(), usize::MAX),
                    (nan, 0),
                ] {
                    let handle = molt_concurrent_threadpool_new(limit);
                    assert!(!exception_pending(py));
                    let pool = state_pool_for_test(py, to_i64(obj_from_bits(handle)).unwrap());
                    assert!(matches!(pool.limit, WorkerLimit::Native(value) if value == expected));
                    assert_eq!(
                        pool.workers.count(),
                        0,
                        "unused capacity started native workers"
                    );
                    molt_concurrent_threadpool_shutdown(
                        handle,
                        MoltObject::from_bool(true).bits(),
                        MoltObject::none().bits(),
                    );
                }
                let handle = molt_concurrent_threadpool_new(nan);
                let callable = native_callable(py, retained_argument as *const ());
                let future = molt_concurrent_threadpool_submit(
                    handle,
                    callable,
                    MoltObject::from_int(1).bits(),
                );
                assert_eq!(
                    molt_concurrent_future_done(future),
                    MoltObject::from_bool(false).bits()
                );
                assert_eq!(
                    state_pool_for_test(py, to_i64(obj_from_bits(handle)).unwrap())
                        .workers
                        .count(),
                    0
                );
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(true).bits(),
                );
                assert_eq!(
                    molt_concurrent_future_cancelled(future),
                    MoltObject::from_bool(true).bits()
                );
                molt_concurrent_future_drop(future);
                dec_ref_bits(py, callable);
                dec_ref_bits(py, nan);
                dec_ref_bits(py, huge);
                for value in [0, -1] {
                    molt_concurrent_threadpool_new(MoltObject::from_int(value).bits());
                    assert_pending_class(py, "ValueError");
                }
                let handle = molt_concurrent_threadpool_new(MoltObject::from_int(4).bits());
                let pool = state_pool_for_test(py, to_i64(obj_from_bits(handle)).unwrap());
                let callable = native_callable(py, retained_argument as *const ());
                for _ in 0..3 {
                    let future = molt_concurrent_threadpool_submit(
                        handle,
                        callable,
                        MoltObject::from_int(7).bits(),
                    );
                    let result = molt_concurrent_future_result(
                        future,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    );
                    assert_eq!(to_i64(obj_from_bits(result)), Some(7));
                    dec_ref_bits(py, result);
                    molt_concurrent_future_drop(future);
                    // Completion notification precedes callbacks/cleanup. Wait for
                    // the actual owner boundary before asserting idle reuse.
                    poll_until(py, || pool.unfinished.load(Ordering::Relaxed) == 0);
                    assert_eq!(pool.workers.count(), 1);
                }
                dec_ref_bits(py, callable);
                DEMAND_RELEASE.store(false, Ordering::Release);
                DEMAND_STARTED.store(0, Ordering::Release);
                let callable = native_callable(py, blocked_argument as *const ());
                let futures: Vec<_> = (0..8)
                    .map(|_| {
                        molt_concurrent_threadpool_submit(
                            handle,
                            callable,
                            MoltObject::from_int(9).bits(),
                        )
                    })
                    .collect();
                assert_eq!(
                    pool.workers.count(),
                    4,
                    "demand did not saturate declared capacity"
                );
                assert_eq!(pool.unfinished.load(Ordering::Relaxed), 8);
                poll_until(py, || DEMAND_STARTED.load(Ordering::Acquire) == 4);
                DEMAND_RELEASE.store(true, Ordering::Release);
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                assert!(!exception_pending(py));
                assert_eq!(DEMAND_STARTED.load(Ordering::Acquire), 8);
                assert_eq!(pool.unfinished.load(Ordering::Relaxed), 0);
                for future in futures {
                    let result = molt_concurrent_future_result(
                        future,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    );
                    assert_eq!(to_i64(obj_from_bits(result)), Some(9));
                    dec_ref_bits(py, result);
                    molt_concurrent_future_drop(future);
                }
                dec_ref_bits(py, callable);
            });
        });
    }

    fn assert_pending_class(py: &PyToken<'_>, name: &str) {
        assert!(exception_pending(py));
        let error = molt_exception_last_pending();
        assert_eq!(class_name_for_error(type_of_bits(py, error)), name);
        clear_exception(py);
        dec_ref_bits(py, error);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_spawn_refusal_preserves_queued_work_and_pool_usability() {
        use crate::state::lifecycle::with_service_spawn_failure_for_test;
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                // First-spawn refusal: retry or either shutdown disposition.
                // Later growth refusal: an existing worker and both accepted
                // jobs survive. OS ENOMEM is still native startup RuntimeError.
                for kind in [
                    std::io::ErrorKind::WouldBlock,
                    std::io::ErrorKind::OutOfMemory,
                ] {
                    for (existing, retry, wait, cancel) in [
                        (0, true, true, false),
                        (0, false, true, false),
                        (0, false, true, true),
                        (0, false, false, false),
                        (1, true, true, false),
                    ] {
                        DEMAND_RELEASE.store(false, Ordering::Release);
                        DEMAND_STARTED.store(0, Ordering::Release);
                        let handle = molt_concurrent_threadpool_new(MoltObject::from_int(2).bits());
                        let pool = state_pool_for_test(py, to_i64(obj_from_bits(handle)).unwrap());
                        let callable = native_callable(py, blocked_argument as *const ());
                        let payload =
                            MoltObject::from_ptr(alloc_string(py, b"accepted job owner")).bits();
                        let callable_base = owner_count(callable);
                        let payload_base = owner_count(payload);
                        let mut returned = Vec::new();
                        for _ in 0..existing {
                            returned
                                .push(molt_concurrent_threadpool_submit(handle, callable, payload));
                        }
                        let failed_id = concurrent_state(py).next_future_id.load(Ordering::Relaxed);
                        with_service_spawn_failure_for_test(kind, || {
                            molt_concurrent_threadpool_submit(handle, callable, payload)
                        });
                        assert_pending_class(py, "RuntimeError");
                        assert!(!pool.shutdown.load(Ordering::Acquire));
                        assert_eq!(pool.workers.count(), existing);
                        assert_eq!(pool.unfinished.load(Ordering::Relaxed), existing + 1);
                        assert!(
                            get_future(py, failed_id).is_none(),
                            "unreturned future leaked its registry owner"
                        );
                        assert_eq!(future_registry(py).lock().unwrap().len(), existing);
                        let failed =
                            Arc::downgrade(pool.pending.lock().unwrap().get(&failed_id).unwrap());
                        assert_eq!(owner_count(callable), callable_base + existing as u32 + 1);
                        assert_eq!(owner_count(payload), payload_base + existing as u32 + 1);
                        if retry {
                            returned
                                .push(molt_concurrent_threadpool_submit(handle, callable, payload));
                            assert!(
                                !exception_pending(py),
                                "transient failure poisoned later startup"
                            );
                            assert_eq!(pool.workers.count(), existing + 1);
                        }
                        DEMAND_RELEASE.store(true, Ordering::Release);
                        molt_concurrent_threadpool_shutdown(
                            handle,
                            MoltObject::from_bool(wait).bits(),
                            MoltObject::from_bool(cancel).bits(),
                        );
                        assert!(!exception_pending(py));
                        poll_until(py, || pool.unfinished.load(Ordering::Relaxed) == 0);
                        assert_eq!(
                            DEMAND_STARTED.load(Ordering::Acquire),
                            if retry { existing + 2 } else { 0 }
                        );
                        for future in returned {
                            let result = molt_concurrent_future_result(
                                future,
                                MoltObject::none().bits(),
                                builtin_classes(py).exception,
                                builtin_classes(py).exception,
                            );
                            assert_eq!(result, payload);
                            dec_ref_bits(py, result);
                            molt_concurrent_future_drop(future);
                        }
                        assert!(
                            failed.upgrade().is_none(),
                            "unreturned accepted job retained its future"
                        );
                        assert!(pool.pending.lock().unwrap().is_empty());
                        assert!(future_registry(py).lock().unwrap().is_empty());
                        assert_eq!(owner_count(callable), callable_base);
                        assert_eq!(owner_count(payload), payload_base);
                        dec_ref_bits(py, callable);
                        dec_ref_bits(py, payload);
                    }
                }
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn callback_submitted_work_grows_while_origin_worker_is_busy() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                NESTED_CALLBACK_OK.store(false, Ordering::Release);
                DEMAND_RELEASE.store(false, Ordering::Release);
                let handle = molt_concurrent_threadpool_new(MoltObject::from_int(2).bits());
                let id = to_i64(obj_from_bits(handle)).unwrap();
                CALLBACK_POOL.store(id, Ordering::Release);
                let pool = state_pool_for_test(py, id);
                let callable = native_callable(py, blocked_argument as *const ());
                let callback = native_callable(py, nested_submit_callback as *const ());
                let future = molt_concurrent_threadpool_submit(
                    handle,
                    callable,
                    MoltObject::from_int(3).bits(),
                );
                molt_concurrent_future_add_done_callback(future, callback);
                DEMAND_RELEASE.store(true, Ordering::Release);
                poll_until(py, || pool.unfinished.load(Ordering::Relaxed) == 0);
                assert!(NESTED_CALLBACK_OK.load(Ordering::Acquire));
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                molt_concurrent_future_drop(future);
                dec_ref_bits(py, callable);
                dec_ref_bits(py, callback);
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn repeated_nonwaiting_executor_drops_reclaim_completed_pool_owners() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let state = concurrent_state(py);
                let baseline = state.pools.lock().unwrap().len();
                let mut previous: Option<std::sync::Weak<ThreadPoolState>> = None;
                let callable = native_callable(py, blocked_argument as *const ());
                for cycle in 0..25 {
                    DEMAND_RELEASE.store(false, Ordering::Release);
                    // Actual demand creates 33 workers, exceeding the old two
                    // cold polls' 16-record budget. No capacity-only fixture.
                    let handle = molt_concurrent_threadpool_new(MoltObject::from_int(33).bits());
                    let id = to_i64(obj_from_bits(handle)).unwrap();
                    let pool = state_pool_for_test(py, id);
                    let futures: Vec<_> = (0..33)
                        .map(|_| {
                            molt_concurrent_threadpool_submit(
                                handle,
                                callable,
                                MoltObject::from_int(1).bits(),
                            )
                        })
                        .collect();
                    assert!(!exception_pending(py));
                    assert_eq!(pool.workers.count(), 33);
                    if let Some(previous) = previous.take() {
                        assert!(
                            previous.upgrade().is_none(),
                            "completed pool debt grew across public cycles"
                        );
                    }
                    assert_eq!(state.pools.lock().unwrap().len(), baseline + 1);
                    for future in futures {
                        molt_concurrent_future_drop(future);
                    }
                    DEMAND_RELEASE.store(true, Ordering::Release);
                    if cycle == 24 {
                        molt_concurrent_threadpool_shutdown(
                            handle,
                            MoltObject::from_bool(true).bits(),
                            MoltObject::none().bits(),
                        );
                        break;
                    }
                    previous = Some(Arc::downgrade(&pool));
                    molt_concurrent_threadpool_drop(handle);
                    // No cleanup polls: subsequent public creation/submission
                    // must repay the previous completed native-handle debt.
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    while !pool.workers.finished_for_test() {
                        assert!(std::time::Instant::now() < deadline);
                        let _release = GilReleaseGuard::suspend();
                        std::thread::yield_now();
                    }
                    assert_eq!(pool.unfinished.load(Ordering::Relaxed), 0);
                }
                dec_ref_bits(py, callable);
                assert!(!exception_pending(py));
                assert_eq!(state.pools.lock().unwrap().len(), baseline);
                assert!(state.stopped.lock().unwrap().ids.is_empty());
                assert!(!state.stopped_pending.load(Ordering::Acquire));
            });
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn controlled_pool(
        py: &PyToken<'_>,
        worker: impl FnOnce() + Send + 'static,
    ) -> (i64, Arc<ThreadPoolState>) {
        let (sender, receiver) = unbounded();
        let pool = Arc::new(ThreadPoolState {
            sender,
            receiver,
            workers: crate::state::lifecycle::RuntimeServiceWorkers::default(),
            shutdown: AtomicBool::new(false),
            pending: Mutex::new(HashMap::new()),
            limit: WorkerLimit::Native(1),
            submission: Mutex::new(()),
            unfinished: AtomicUsize::new(0),
        });
        let id = next_pool_id(py);
        pool_registry(py)
            .lock()
            .unwrap()
            .insert(id, Arc::clone(&pool));
        let origin = crate::state::runtime_state::RuntimeWorkerOrigin::new(runtime_state(py));
        pool.workers
            .start(origin, |handles| {
                handles.push(crate::state::lifecycle::RuntimeServiceWorker::spawn(
                    origin,
                    thread::Builder::new().name("molt-pool-reap-control".into()),
                    worker,
                )?);
                Ok(())
            })
            .unwrap();
        (id, pool)
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn blocking_shutdown_waits_for_canonical_reaper_join_disposition() {
        use crate::state::lifecycle::{
            SERVICE_JOIN_PAUSE, SERVICE_JOIN_WAITING, run_runtime_worker,
        };
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let handle = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
                let id = to_i64(obj_from_bits(handle)).unwrap();
                let pool = state_pool_for_test(py, id);
                let callable = native_callable(py, retained_argument as *const ());
                let future = molt_concurrent_threadpool_submit(
                    handle,
                    callable,
                    MoltObject::from_int(7).bits(),
                );
                assert_eq!(
                    pool.workers.count(),
                    1,
                    "join race requires a real native worker"
                );
                assert!(pool.stop(false).0);
                schedule_stopped_pool(concurrent_state(py), id);
                let origin =
                    crate::state::runtime_state::RuntimeWorkerOrigin::new(runtime_state(py));
                let (taken_tx, taken_rx) = std::sync::mpsc::channel();
                let (resume_tx, resume_rx) = std::sync::mpsc::channel();
                let reaper = thread::spawn(move || {
                    run_runtime_worker(origin, || {
                        SERVICE_JOIN_PAUSE
                            .with(|slot| *slot.borrow_mut() = Some((taken_tx, resume_rx)));
                        let deadline = std::time::Instant::now() + Duration::from_secs(5);
                        loop {
                            let done = crate::with_gil_entry_nopanic!(py, {
                                crate::molt_async_work_poll_and_exception_pending();
                                !pool_registry(py).lock().unwrap().contains_key(&id)
                            });
                            if done {
                                break;
                            }
                            assert!(std::time::Instant::now() < deadline);
                            thread::yield_now();
                        }
                    })
                });
                {
                    let _release = GilReleaseGuard::suspend();
                    taken_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
                SERVICE_JOIN_WAITING.with(|slot| *slot.borrow_mut() = Some(waiting_tx));
                let controller = thread::spawn(move || {
                    run_runtime_worker(origin, || {
                        let waiting = waiting_rx.recv_timeout(Duration::from_secs(5));
                        // This acquisition independently proves the waiting public
                        // shutdown has released GIL custody before disposition.
                        let gil = crate::GilGuard::new();
                        resume_tx.send(()).unwrap();
                        drop(gil);
                        waiting.is_ok()
                    })
                });
                molt_concurrent_threadpool_shutdown(
                    handle,
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                let success = !exception_pending(py);
                if !success {
                    clear_exception(py);
                }
                let saw_wait = {
                    let _release = GilReleaseGuard::suspend();
                    reaper.join().unwrap();
                    controller.join().unwrap()
                };
                assert!(
                    saw_wait,
                    "public shutdown returned before the in-progress join disposition"
                );
                assert!(success, "concurrent successful join became a RuntimeError");
                assert!(!pool_registry(py).lock().unwrap().contains_key(&id));
                assert!(pool.workers.join());
                assert_eq!(pool.unfinished.load(Ordering::Relaxed), 0);
                molt_concurrent_future_drop(future);
                dec_ref_bits(py, callable);
            });
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn state_pool_for_test(py: &PyToken<'_>, id: i64) -> Arc<ThreadPoolState> {
        pool_registry(py).lock().unwrap().get(&id).cloned().unwrap()
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn self_shutdown_refuses_even_when_another_joiner_owns_its_handle() {
        use crate::state::lifecycle::SERVICE_JOIN_PAUSE;
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let (pool_tx, pool_rx) = std::sync::mpsc::channel::<i64>();
                let (result_tx, result_rx) = std::sync::mpsc::channel();
                let (id, pool) = controlled_pool(py, move || {
                    let id = pool_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    crate::with_gil_entry_nopanic!(py, {
                        molt_concurrent_threadpool_shutdown(
                            MoltObject::from_int(id).bits(),
                            MoltObject::from_bool(true).bits(),
                            MoltObject::none().bits(),
                        );
                        let refused = exception_pending(py);
                        if refused {
                            clear_exception(py);
                        }
                        result_tx.send(refused).unwrap();
                    });
                });
                let join_pool = Arc::clone(&pool);
                let (taken_tx, taken_rx) = std::sync::mpsc::channel();
                let (resume_tx, resume_rx) = std::sync::mpsc::channel();
                let joiner = thread::spawn(move || {
                    SERVICE_JOIN_PAUSE
                        .with(|slot| *slot.borrow_mut() = Some((taken_tx, resume_rx)));
                    join_pool.workers.join()
                });
                taken_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                pool_tx.send(id).unwrap();
                let refused = {
                    let _release = GilReleaseGuard::suspend();
                    let refused = result_rx.recv_timeout(Duration::from_secs(5));
                    resume_tx.send(()).unwrap();
                    assert!(joiner.join().unwrap());
                    refused.unwrap()
                };
                assert!(refused, "worker waited for its own extracted join handle");
                molt_concurrent_threadpool_shutdown(
                    MoltObject::from_int(id).bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::none().bits(),
                );
                assert!(!exception_pending(py));
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn nonwaiting_reaper_retains_inflight_owner_without_blocking_or_detaching() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let (ready_tx, ready_rx) = std::sync::mpsc::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let (id, pool) = controlled_pool(py, move || {
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                });
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let weak = Arc::downgrade(&pool);
                drop(pool);
                molt_concurrent_threadpool_drop(MoltObject::from_int(id).bits());
                for _ in 0..16 {
                    assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
                }
                assert!(
                    weak.upgrade().is_some(),
                    "live native worker lost its registry owner"
                );
                assert_eq!(concurrent_state(py).stopped.lock().unwrap().ids.len(), 1);
                release_tx.send(()).unwrap();
                poll_until(py, || weak.upgrade().is_none());
            });
        });
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), panic = "unwind"))]
    fn nonwaiting_reaper_retains_failed_disposition_without_automatic_retries() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let (id, pool) = controlled_pool(py, || {
                    crate::test_support::with_expected_panic(|| {
                        panic!("executor reap failure control")
                    });
                });
                let weak = Arc::downgrade(&pool);
                drop(pool);
                molt_concurrent_threadpool_drop(MoltObject::from_int(id).bits());
                let state = concurrent_state(py);
                poll_until(py, || !state.stopped_pending.load(Ordering::Acquire));
                assert!(
                    weak.upgrade().is_some(),
                    "failed worker disposition was discarded"
                );
                assert!(state.stopped.lock().unwrap().ids.is_empty());
                for _ in 0..16 {
                    assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
                }
                let retained = state.pools.lock().unwrap().get(&id).cloned().unwrap();
                assert!(!retained.workers.join(), "failure was relabeled successful");
                assert!(state.stopped.lock().unwrap().ids.is_empty());
                drop(retained);
                // This control panics only in the body; real terminal cleanup
                // and native join have completed. Explicitly remove this test's
                // failed disposition so a separate runtime teardown can finish.
                state.pools.lock().unwrap().remove(&id);
                assert!(weak.upgrade().is_none());
            });
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    static RETIRE_FINALIZERS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(not(target_arch = "wasm32"))]
    static RETIRE_REENTRY_OK: AtomicBool = AtomicBool::new(false);

    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn retirement_finalizer(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let state = concurrent_state(py);
            let unlocked = state.pools.try_lock().is_ok() && state.stopped.try_lock().is_ok();
            if unlocked {
                let result = crate::molt_async_work_poll_and_exception_pending();
                RETIRE_REENTRY_OK.store(result == 0, Ordering::Release);
            }
            RETIRE_FINALIZERS.fetch_add(1, Ordering::AcqRel);
            MoltObject::none().bits()
        })
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn stopped_queue_reaping_is_bounded_and_finalizers_reenter_without_owner_locks() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                RETIRE_FINALIZERS.store(0, Ordering::Release);
                RETIRE_REENTRY_OK.store(false, Ordering::Release);
                let name = attr_name_bits_from_bytes(py, b"ExecutorRetireReentry").unwrap();
                let class = molt_class_new(name);
                dec_ref_bits(py, name);
                molt_class_set_base(class, builtin_classes(py).object);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
                let method = native_callable(py, retirement_finalizer as *const ());
                let key = attr_name_bits_from_bytes(py, b"__del__").unwrap();
                molt_set_attr_name(class, key, method);
                dec_ref_bits(py, key);
                dec_ref_bits(py, method);
                let payload = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
                assert!(obj_from_bits(payload).as_ptr().is_some());
                let callable = native_callable(py, retained_argument as *const ());
                let baseline = owner_count(callable);
                let handle = molt_concurrent_threadpool_new(MoltObject::from_int(1).bits());
                let id = to_i64(obj_from_bits(handle)).unwrap();
                let pool = pool_registry(py).lock().unwrap().get(&id).cloned().unwrap();
                let weak = Arc::downgrade(&pool);
                for _ in 0..20 {
                    let future = molt_concurrent_threadpool_submit(handle, callable, payload);
                    molt_concurrent_future_drop(future);
                }
                dec_ref_bits(py, payload);
                // Main still owns GIL, so no callable can have run. Closure
                // returns the claimed item to the pool, then joins normally.
                runtime_state(py).close_thread_borrow_admission();
                molt_concurrent_threadpool_drop(handle);
                assert!(pool.workers.join());
                drop(pool);
                assert_eq!(RETIRE_FINALIZERS.load(Ordering::Acquire), 0);
                assert_eq!(crate::molt_async_work_poll_and_exception_pending(), 0);
                assert_eq!(
                    RETIRE_FINALIZERS.load(Ordering::Acquire),
                    0,
                    "one poll drained an unbounded queued cohort"
                );
                assert!(weak.upgrade().is_some());
                poll_until(py, || weak.upgrade().is_none());
                assert_eq!(RETIRE_FINALIZERS.load(Ordering::Acquire), 1);
                assert!(RETIRE_REENTRY_OK.load(Ordering::Acquire));
                assert_eq!(owner_count(callable), baseline);
                dec_ref_bits(py, callable);
                dec_ref_bits(py, class);
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn nonwaiting_shutdown_reaps_queued_owners_and_preserves_callback_outcomes() {
        for close_origin in [false, true] {
            crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
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
                    let id = to_i64(obj_from_bits(pool)).unwrap();
                    let weak =
                        Arc::downgrade(state.concurrent.pools.lock().unwrap().get(&id).unwrap());
                    let future = molt_concurrent_threadpool_submit(pool, callable, payload);
                    molt_concurrent_future_add_done_callback(future, callback);
                    molt_concurrent_threadpool_drop(pool);
                    // Future drop leaves the worker's pending callback owners
                    // intact; completion/reaping must retire them exactly once.
                    molt_concurrent_future_drop(future);
                    if close_origin {
                        state.close_thread_borrow_admission();
                    }
                    poll_until(py, || weak.upgrade().is_none());
                    assert_eq!(
                        CALLBACK_CALLS.load(Ordering::Acquire),
                        usize::from(!close_origin)
                    );
                    assert_eq!(
                        [
                            owner_count(callable),
                            owner_count(callback),
                            owner_count(payload)
                        ],
                        baselines
                    );
                    assert!(!concurrent_clear_runtime_state(py, state));
                    for bits in [callable, callback, payload] {
                        dec_ref_bits(py, bits);
                    }
                });
            });
        }
    }

    #[test]
    fn concurrent_runtime_state_is_owned_and_clearable() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
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

                let future_id = next_future_id(&state.concurrent);
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
                assert_eq!(next_future_id(&state.concurrent), future_id + 1);
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
        });
    }

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    #[ignore = "real-runtime lifecycle cost probe; run exact retained image through cargo_test_binary_runner"]
    fn lifecycle_cost_probe() {
        use molt_cpython_abi::l7_attestation::{
            CALIBRATION_TARGET_NS, MINIMUM_SAMPLE_NS, SAMPLE_COUNT, calibrate_timed_iterations,
            enforce_current_thread_affinity, normalized_affinity_mask, summarize_samples,
        };
        use std::hint::black_box;
        use std::time::Instant;

        fn measure(name: &str, mut operation: impl FnMut()) -> serde_json::Value {
            for _ in 0..64 {
                operation();
            }
            let mut batch = |iterations: usize| {
                let started = Instant::now();
                for _ in 0..iterations {
                    operation();
                }
                started.elapsed().as_nanos()
            };
            let iterations = calibrate_timed_iterations(64, &mut batch);
            let elapsed: Vec<u128> = (0..SAMPLE_COUNT)
                .map(|_| {
                    let ns = batch(iterations);
                    assert!(
                        ns >= MINIMUM_SAMPLE_NS,
                        "lifecycle sample below timing floor"
                    );
                    ns
                })
                .collect();
            let samples: Vec<f64> = elapsed
                .iter()
                .map(|ns| *ns as f64 / iterations as f64)
                .collect();
            let summary = summarize_samples(&samples);
            serde_json::json!({
                "name": name,
                "iterations_per_sample": iterations,
                "sample_count": SAMPLE_COUNT,
                "calibration_target_ns": CALIBRATION_TARGET_NS,
                "minimum_sample_ns": MINIMUM_SAMPLE_NS,
                "elapsed_ns": elapsed,
                "ns_per_operation": samples,
                "summary": {
                    "median_ns_per_operation": summary.median,
                    "cv": summary.cv,
                    "robust_cv": summary.robust_cv,
                },
            })
        }

        assert!(
            !cfg!(debug_assertions),
            "lifecycle cost probe requires an optimized image"
        );
        let mask = std::env::var("MOLT_L7_AFFINITY_MASK")
            .expect("lifecycle cost probe requires MOLT_L7_AFFINITY_MASK");
        let affinity = enforce_current_thread_affinity(&mask);
        // One origin surrounds all warmup, calibration and samples. Per-cycle
        // runtime transactions would measure setup and hide retained owners.
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                assert!(!exception_pending(py));
                assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
                assert!(pool_registry(py).lock().unwrap().is_empty());
                let empty = measure("empty_public_async_safepoint", || {
                    assert_eq!(
                        black_box(crate::molt_async_work_poll_and_exception_pending()),
                        0
                    );
                });
                assert!(!exception_pending(py));
                assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
                let one = MoltObject::from_int(1).bits();
                let yes = MoltObject::from_bool(true).bits();
                let no = MoltObject::from_bool(false).bits();
                let pool = measure("one_worker_pool_create_and_waiting_shutdown", || {
                    let handle = black_box(molt_concurrent_threadpool_new(one));
                    assert!(to_i64(obj_from_bits(handle)).is_some_and(|id| id > 0));
                    assert!(!exception_pending(py));
                    assert_eq!(
                        black_box(molt_concurrent_threadpool_shutdown(handle, yes, no)),
                        MoltObject::none().bits(),
                    );
                    assert!(!exception_pending(py));
                });
                assert!(pool_registry(py).lock().unwrap().is_empty());
                assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
                let callable = native_callable(py, retained_argument as *const ());
                let callable_base = owner_count(callable);
                let active = measure("one_job_pool_create_submit_and_waiting_shutdown", || {
                    let handle = black_box(molt_concurrent_threadpool_new(one));
                    let future =
                        black_box(molt_concurrent_threadpool_submit(handle, callable, one));
                    assert!(!exception_pending(py));
                    black_box(molt_concurrent_threadpool_shutdown(handle, yes, no));
                    let result = black_box(molt_concurrent_future_result(
                        future,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    ));
                    assert_eq!(result, one);
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                    molt_concurrent_future_drop(future);
                });
                // Warm one actual worker once; this case measures the common
                // submission/result path, excluding startup and waiting shutdown.
                let steady_pool = molt_concurrent_threadpool_new(one);
                let warm_future = molt_concurrent_threadpool_submit(steady_pool, callable, one);
                let warm_result = molt_concurrent_future_result(
                    warm_future,
                    MoltObject::none().bits(),
                    builtin_classes(py).exception,
                    builtin_classes(py).exception,
                );
                assert_eq!(warm_result, one);
                dec_ref_bits(py, warm_result);
                molt_concurrent_future_drop(warm_future);
                let steady = measure("one_worker_steady_submit_result_and_future_drop", || {
                    let future = black_box(molt_concurrent_threadpool_submit(
                        steady_pool,
                        callable,
                        one,
                    ));
                    let result = black_box(molt_concurrent_future_result(
                        future,
                        MoltObject::none().bits(),
                        builtin_classes(py).exception,
                        builtin_classes(py).exception,
                    ));
                    assert_eq!(result, one);
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                    molt_concurrent_future_drop(future);
                });
                molt_concurrent_threadpool_shutdown(steady_pool, yes, no);
                assert!(!exception_pending(py));
                assert_eq!(owner_count(callable), callable_base);
                dec_ref_bits(py, callable);
                assert!(pool_registry(py).lock().unwrap().is_empty());
                assert!(future_registry(py).lock().unwrap().is_empty());
                println!(
                    "LIFECYCLE_COST_PROBE={}",
                    serde_json::json!({
                        "schema_version": 1,
                        "kind": "runtime_lifecycle_cost_probe",
                        "debug_assertions": cfg!(debug_assertions),
                        "host": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
                        "affinity_mask": normalized_affinity_mask(affinity),
                        "timing_scope": "loop inclusive, including semantic checks; one fresh runtime outside all samples",
                        "worker_cost_scope": "capacity-one empty case has no submitted jobs; active case includes one job, native startup and waiting shutdown; steady case reuses one worker with startup/shutdown outside timing",
                        "layout_bytes": {"pool_owner": std::mem::size_of::<ThreadPoolState>(), "concurrent_state": std::mem::size_of::<ConcurrentRuntimeState>()},
                        "allocation_metrics": false,
                        "performance_claim": false,
                        "cases": [empty, pool, active, steady],
                    })
                );
            });
        });
    }
}
