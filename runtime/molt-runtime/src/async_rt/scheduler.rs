use crate::PyToken;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_deque::{Injector, Stealer, Worker};

use crate::state::clear_worker_thread_state;
use crate::{
    call_poll_fn, exception_context_align_depth, exception_context_fallback_pop,
    exception_context_fallback_push, exception_stack_depth, exception_stack_set_depth,
    header_from_obj_ptr, inc_ref_bits, io_wait_poll_fn_addr, obj_from_bits, pending_bits_i64,
    process_poll_fn_addr, profile_hit, ptr_from_bits, raise_exception, resolve_task_ptr,
    runtime_state, set_task_raise_active, task_exception_depth_store, task_exception_depth_take,
    task_exception_handler_stack_store, task_exception_handler_stack_take,
    task_exception_stack_store, task_exception_stack_take, task_raise_active, thread_poll_fn_addr,
    to_i64, with_gil, GilGuard, MoltHeader, MoltObject, ProcessTaskState, PtrSlot, ThreadTaskState,
    ACTIVE_EXCEPTION_STACK, ASYNCGEN_REGISTRY, ASYNC_PENDING_COUNT, ASYNC_POLL_COUNT,
    ASYNC_SLEEP_REGISTER_COUNT, ASYNC_WAKEUP_COUNT, EXCEPTION_STACK, FN_PTR_CODE,
    HEADER_FLAG_SPAWN_RETAIN,
};

use super::cancellation::{
    cancel_tokens, clear_task_token, current_token_id, ensure_task_token,
    raise_cancelled_with_message, register_task_token, set_current_token, task_cancel_pending,
    task_take_cancel_pending,
};

// --- Scheduler ---

pub(crate) struct AsyncHangProbe {
    threshold: usize,
    pub(crate) pending_counts: Mutex<HashMap<usize, usize>>,
}

impl AsyncHangProbe {
    fn new(threshold: usize) -> Self {
        Self {
            threshold,
            pending_counts: Mutex::new(HashMap::new()),
        }
    }
}

pub(crate) fn async_hang_probe(_py: &PyToken<'_>) -> Option<&'static AsyncHangProbe> {
    runtime_state(_py)
        .async_hang_probe
        .get_or_init(|| {
            let value = std::env::var("MOLT_ASYNC_HANG_PROBE").ok()?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return None;
            }
            let threshold = match trimmed.parse::<usize>() {
                Ok(0) => return None,
                Ok(val) => val,
                Err(_) => 100_000,
            };
            Some(AsyncHangProbe::new(threshold))
        })
        .as_ref()
}

pub(crate) fn async_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_ASYNC_TRACE").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

thread_local! {
    pub(crate) static CURRENT_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    pub(crate) static BLOCK_ON_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn task_exception_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<u64>>> {
    &runtime_state(_py).task_exception_stacks
}

pub(crate) fn task_exception_handler_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<u8>>> {
    &runtime_state(_py).task_exception_handler_stacks
}

pub(crate) fn await_waiters(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, Vec<PtrSlot>>> {
    &runtime_state(_py).await_waiters
}

pub(crate) fn task_waiting_on(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, PtrSlot>> {
    &runtime_state(_py).task_waiting_on
}

pub(crate) fn asyncgen_registry() -> &'static Mutex<HashSet<PtrSlot>> {
    ASYNCGEN_REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn fn_ptr_code_map() -> &'static Mutex<HashMap<u64, u64>> {
    FN_PTR_CODE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn fn_ptr_code_set(_py: &PyToken<'_>, fn_ptr: u64, code_bits: u64) {
    crate::gil_assert();
    if fn_ptr == 0 {
        return;
    }
    let mut guard = fn_ptr_code_map().lock().unwrap();
    if code_bits == 0 {
        if let Some(old_bits) = guard.remove(&fn_ptr) {
            if old_bits != 0 {
                crate::dec_ref_bits(_py, old_bits);
            }
        }
        return;
    }
    let old_bits = guard.insert(fn_ptr, code_bits);
    if old_bits != Some(code_bits) {
        crate::inc_ref_bits(_py, code_bits);
        if let Some(old) = old_bits {
            if old != 0 {
                crate::dec_ref_bits(_py, old);
            }
        }
    }
}

pub(crate) fn fn_ptr_code_get(fn_ptr: u64) -> u64 {
    if fn_ptr == 0 {
        return 0;
    }
    let guard = fn_ptr_code_map().lock().unwrap();
    guard.get(&fn_ptr).copied().unwrap_or(0)
}

pub(crate) fn task_exception_depths(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, usize>> {
    &runtime_state(_py).task_exception_depths
}

pub(crate) fn task_last_exceptions(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, PtrSlot>> {
    &runtime_state(_py).task_last_exceptions
}

pub(crate) fn current_task_ptr() -> *mut u8 {
    CURRENT_TASK.with(|cell| cell.get())
}

pub(crate) fn current_task_key() -> Option<PtrSlot> {
    CURRENT_TASK.with(|cell| {
        let value = cell.get();
        if value.is_null() {
            None
        } else {
            Some(PtrSlot(value))
        }
    })
}

pub(crate) fn await_waiter_register(_py: &PyToken<'_>, waiter_ptr: *mut u8, awaited_ptr: *mut u8) {
    if waiter_ptr.is_null() || awaited_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: await_register waiter=0x{:x} awaited=0x{:x}",
            waiter_ptr as usize, awaited_ptr as usize
        );
    }
    let waiter_key = PtrSlot(waiter_ptr);
    let awaited_key = PtrSlot(awaited_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    if let Some(prev) = waiting_map.insert(waiter_key, awaited_key) {
        if prev != awaited_key {
            if let Some(waiters) = awaiters_map.get_mut(&prev) {
                if let Some(pos) = waiters.iter().position(|val| *val == waiter_key) {
                    waiters.swap_remove(pos);
                }
                if waiters.is_empty() {
                    awaiters_map.remove(&prev);
                }
            }
        }
    }
    let waiters = awaiters_map.entry(awaited_key).or_default();
    if !waiters.contains(&waiter_key) {
        waiters.push(waiter_key);
    }
}

pub(crate) fn await_waiter_clear(_py: &PyToken<'_>, waiter_ptr: *mut u8) {
    if waiter_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: await_clear waiter=0x{:x}",
            waiter_ptr as usize
        );
    }
    let waiter_key = PtrSlot(waiter_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let awaited_key = waiting_map.remove(&waiter_key);
    if awaited_key.is_none() {
        return;
    }
    let awaited_key = awaited_key.unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    if let Some(waiters) = awaiters_map.get_mut(&awaited_key) {
        if let Some(pos) = waiters.iter().position(|val| *val == waiter_key) {
            waiters.swap_remove(pos);
        }
        if waiters.is_empty() {
            awaiters_map.remove(&awaited_key);
        }
    }
}

pub(crate) fn await_waiters_take(_py: &PyToken<'_>, awaited_ptr: *mut u8) -> Vec<PtrSlot> {
    if awaited_ptr.is_null() {
        return Vec::new();
    }
    let awaited_key = PtrSlot(awaited_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    let waiters = awaiters_map.remove(&awaited_key).unwrap_or_default();
    for waiter in &waiters {
        waiting_map.remove(waiter);
    }
    waiters
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thread_task_state(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) -> Option<Arc<ThreadTaskState>> {
    if future_ptr.is_null() {
        return None;
    }
    runtime_state(_py)
        .thread_tasks
        .lock()
        .unwrap()
        .get(&PtrSlot(future_ptr))
        .cloned()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thread_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    crate::gil_assert();
    if future_ptr.is_null() {
        return;
    }
    let state = runtime_state(_py)
        .thread_tasks
        .lock()
        .unwrap()
        .remove(&PtrSlot(future_ptr));
    if let Some(state) = state {
        state.cancelled.store(true, AtomicOrdering::Release);
        if let Some(bits) = state.result.lock().unwrap().take() {
            crate::dec_ref_bits(_py, bits);
        }
        if let Some(bits) = state.exception.lock().unwrap().take() {
            crate::dec_ref_bits(_py, bits);
        }
        state.condvar.notify_all();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn process_task_state(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) -> Option<Arc<ProcessTaskState>> {
    if future_ptr.is_null() {
        return None;
    }
    runtime_state(_py)
        .process_tasks
        .lock()
        .unwrap()
        .get(&PtrSlot(future_ptr))
        .cloned()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn process_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    crate::gil_assert();
    if future_ptr.is_null() {
        return;
    }
    let state = runtime_state(_py)
        .process_tasks
        .lock()
        .unwrap()
        .remove(&PtrSlot(future_ptr));
    if let Some(state) = state {
        state.cancelled.store(true, AtomicOrdering::Release);
        let mut guard = state.process.wait_future.lock().unwrap();
        if guard.map(|val| val.0) == Some(future_ptr) {
            *guard = None;
        }
        state.process.condvar.notify_all();
    }
}

pub(crate) fn task_waiting_on_event(_py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    let waiting_map = task_waiting_on(_py).lock().unwrap();
    let awaited = match waiting_map.get(&PtrSlot(task_ptr)) {
        Some(val) => val.0,
        None => return false,
    };
    unsafe {
        let header = header_from_obj_ptr(awaited);
        let poll_fn = (*header).poll_fn;
        poll_fn == io_wait_poll_fn_addr()
            || poll_fn == thread_poll_fn_addr()
            || poll_fn == process_poll_fn_addr()
    }
}

pub(crate) fn task_waiting_on_future(_py: &PyToken<'_>, task_ptr: *mut u8) -> Option<*mut u8> {
    if task_ptr.is_null() {
        return None;
    }
    let waiting_map = task_waiting_on(_py).lock().unwrap();
    waiting_map.get(&PtrSlot(task_ptr)).map(|val| val.0)
}

pub(crate) fn block_on_wait_event(
    _py: &PyToken<'_>,
    awaited_ptr: *mut u8,
    deadline: Option<Instant>,
) -> bool {
    if awaited_ptr.is_null() {
        return false;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = deadline;
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        unsafe {
            let header = header_from_obj_ptr(awaited_ptr);
            let poll_fn = (*header).poll_fn;
            let timeout = deadline.and_then(|dl| {
                let now = Instant::now();
                if dl > now {
                    Some(dl - now)
                } else {
                    None
                }
            });
            if poll_fn == io_wait_poll_fn_addr() {
                let payload_bytes = (*header)
                    .size
                    .saturating_sub(std::mem::size_of::<MoltHeader>());
                if payload_bytes < 2 * std::mem::size_of::<u64>() {
                    return false;
                }
                let payload_ptr = awaited_ptr as *mut u64;
                let socket_bits = *payload_ptr;
                let events_bits = *payload_ptr.add(1);
                let socket_ptr = ptr_from_bits(socket_bits);
                if socket_ptr.is_null() {
                    return false;
                }
                let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
                if events == 0 {
                    return false;
                }
                let _ = runtime_state(_py)
                    .io_poller()
                    .wait_blocking(socket_ptr, events, timeout);
                return true;
            }
            if poll_fn == thread_poll_fn_addr() {
                if let Some(state) = thread_task_state(_py, awaited_ptr) {
                    state.wait_blocking(timeout);
                    return true;
                }
            }
            if poll_fn == process_poll_fn_addr() {
                if let Some(state) = process_task_state(_py, awaited_ptr) {
                    state.wait_blocking(timeout);
                    return true;
                }
            }
        }
        false
    }
}

pub(crate) fn record_async_poll(_py: &PyToken<'_>, task_ptr: *mut u8, pending: bool, site: &str) {
    profile_hit(_py, &ASYNC_POLL_COUNT);
    if pending {
        profile_hit(_py, &ASYNC_PENDING_COUNT);
    }
    let Some(probe) = async_hang_probe(_py) else {
        return;
    };
    if task_ptr.is_null() {
        return;
    }
    if !pending {
        probe
            .pending_counts
            .lock()
            .unwrap()
            .remove(&(task_ptr as usize));
        return;
    }
    let mut counts = probe.pending_counts.lock().unwrap();
    let count = counts.entry(task_ptr as usize).or_insert(0);
    *count += 1;
    if *count != probe.threshold && *count % probe.threshold != 0 {
        return;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        eprintln!(
            "Molt async hang probe: site={} polls={} ptr=0x{:x} type={} state={} poll=0x{:x}",
            site,
            count,
            task_ptr as usize,
            (*header).type_id,
            (*header).state,
            (*header).poll_fn
        );
    }
}

pub struct MoltTask {
    pub future_ptr: *mut u8,
}

#[derive(Copy, Clone)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SleepEntry {
    deadline: Instant,
    task_ptr: PtrSlot,
    gen: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl PartialEq for SleepEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.gen == other.gen && self.task_ptr == other.task_ptr
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Eq for SleepEntry {}

#[cfg(not(target_arch = "wasm32"))]
impl PartialOrd for SleepEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Ord for SleepEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.gen.cmp(&self.gen))
    }
}

pub(crate) struct SleepState {
    #[cfg(not(target_arch = "wasm32"))]
    heap: BinaryHeap<SleepEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    tasks: HashMap<PtrSlot, u64>,
    #[cfg(not(target_arch = "wasm32"))]
    next_gen: u64,
    blocking: HashMap<PtrSlot, Instant>,
    shutdown: bool,
}

pub(crate) struct SleepQueue {
    inner: Mutex<SleepState>,
    #[cfg(not(target_arch = "wasm32"))]
    cv: Condvar,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl SleepQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(SleepState {
                #[cfg(not(target_arch = "wasm32"))]
                heap: BinaryHeap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                tasks: HashMap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                next_gen: 0,
                blocking: HashMap::new(),
                shutdown: false,
            }),
            #[cfg(not(target_arch = "wasm32"))]
            cv: Condvar::new(),
            #[cfg(not(target_arch = "wasm32"))]
            worker: Mutex::new(None),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_worker_handle(&self, handle: thread::JoinHandle<()>) {
        let mut guard = self.worker.lock().unwrap();
        *guard = Some(handle);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn register_scheduler(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
        deadline: Instant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        let gen = guard.next_gen;
        guard.next_gen += 1;
        guard.tasks.insert(PtrSlot(task_ptr), gen);
        profile_hit(_py, &ASYNC_SLEEP_REGISTER_COUNT);
        guard.heap.push(SleepEntry {
            deadline,
            task_ptr: PtrSlot(task_ptr),
            gen,
        });
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register task=0x{:x} delay_ms={} gen={}",
                task_ptr as usize,
                delay.as_secs_f64() * 1000.0,
                gen
            );
        }
        self.cv.notify_one();
    }

    pub(crate) fn register_blocking(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
        deadline: Instant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        profile_hit(_py, &ASYNC_SLEEP_REGISTER_COUNT);
        guard.blocking.insert(PtrSlot(task_ptr), deadline);
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register_blocking task=0x{:x} delay_ms={}",
                task_ptr as usize,
                delay.as_secs_f64() * 1000.0
            );
        }
    }

    pub(crate) fn cancel_task(&self, _py: &PyToken<'_>, task_ptr: *mut u8) {
        let _ = _py;
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        guard.blocking.remove(&PtrSlot(task_ptr));
        #[cfg(not(target_arch = "wasm32"))]
        {
            guard.tasks.remove(&PtrSlot(task_ptr));
            self.cv.notify_one();
        }
    }

    pub(crate) fn take_blocking_deadline(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
    ) -> Option<Instant> {
        let _ = _py;
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return None;
        }
        guard.blocking.remove(&PtrSlot(task_ptr))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn is_scheduled(&self, _py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
        let _ = _py;
        let guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return false;
        }
        guard.tasks.contains_key(&PtrSlot(task_ptr))
    }

    pub(crate) fn shutdown(&self, _py: &PyToken<'_>) {
        let _ = _py;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.shutdown = true;
            guard.blocking.clear();
            #[cfg(not(target_arch = "wasm32"))]
            {
                guard.tasks.clear();
                guard.heap.clear();
                self.cv.notify_all();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(handle) = self.worker.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sleep_worker(queue: Arc<SleepQueue>) {
    loop {
        let task_ptr = {
            let mut guard = queue.inner.lock().unwrap();
            loop {
                if guard.shutdown {
                    return;
                }
                match guard.heap.peek() {
                    Some(entry) => {
                        let key = entry.task_ptr;
                        if guard.tasks.get(&key) != Some(&entry.gen) {
                            guard.heap.pop();
                            continue;
                        }
                        let now = Instant::now();
                        if entry.deadline <= now {
                            let entry = guard.heap.pop().unwrap();
                            guard.tasks.remove(&key);
                            break entry.task_ptr.0;
                        }
                        let wait = entry.deadline.saturating_duration_since(now);
                        let (next_guard, _) = queue.cv.wait_timeout(guard, wait).unwrap();
                        guard = next_guard;
                    }
                    None => {
                        guard = queue.cv.wait(guard).unwrap();
                    }
                }
            }
        };
        let gil = GilGuard::new();
        let py = gil.token();
        profile_hit(&py, &ASYNC_WAKEUP_COUNT);
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_wakeup task=0x{:x}",
                task_ptr as usize
            );
        }
        runtime_state(&py).scheduler().enqueue(MoltTask {
            future_ptr: task_ptr,
        });
    }
}

pub(crate) fn monotonic_now_secs(_py: &PyToken<'_>) -> f64 {
    runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_secs_f64()
}

pub(crate) fn monotonic_now_nanos(_py: &PyToken<'_>) -> u128 {
    runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
}

pub(crate) fn instant_from_monotonic_secs(_py: &PyToken<'_>, secs: f64) -> Instant {
    let start = runtime_state(_py).start_time.get_or_init(Instant::now);
    if !secs.is_finite() || secs <= 0.0 {
        return *start;
    }
    *start + Duration::from_secs_f64(secs)
}

unsafe impl Send for MoltTask {}

pub struct MoltScheduler {
    injector: Arc<Injector<MoltTask>>,
    stealers: Vec<Stealer<MoltTask>>,
    running: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    worker_handles: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl MoltScheduler {
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let num_threads = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let num_threads = num_cpus::get().max(1);
        let injector = Arc::new(Injector::new());
        let mut workers = Vec::new();
        let mut stealers = Vec::new();
        let running = Arc::new(AtomicBool::new(true));
        #[cfg(not(target_arch = "wasm32"))]
        let mut worker_handles = Vec::new();

        for _ in 0..num_threads {
            workers.push(Worker::new_fifo());
        }

        for w in &workers {
            stealers.push(w.stealer());
        }

        for (i, worker) in workers.into_iter().enumerate() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let injector_clone = Arc::clone(&injector);
                let stealers_clone = stealers.clone();
                let running_clone = Arc::clone(&running);

                let handle = thread::spawn(move || loop {
                    if !running_clone.load(AtomicOrdering::Relaxed) {
                        with_gil(|py| clear_worker_thread_state(&py));
                        break;
                    }

                    if let Some(task) = worker.pop() {
                        Self::execute_task(task, &injector_clone);
                        continue;
                    }

                    match injector_clone.steal_batch_and_pop(&worker) {
                        crossbeam_deque::Steal::Success(task) => {
                            Self::execute_task(task, &injector_clone);
                            continue;
                        }
                        crossbeam_deque::Steal::Retry => continue,
                        crossbeam_deque::Steal::Empty => {}
                    }

                    let mut stolen = false;
                    for (j, stealer) in stealers_clone.iter().enumerate() {
                        if i == j {
                            continue;
                        }
                        if let crossbeam_deque::Steal::Success(task) =
                            stealer.steal_batch_and_pop(&worker)
                        {
                            Self::execute_task(task, &injector_clone);
                            stolen = true;
                            break;
                        }
                    }

                    if !stolen {
                        thread::yield_now();
                    }
                });
                worker_handles.push(handle);
            }
        }

        Self {
            injector,
            stealers,
            running,
            #[cfg(not(target_arch = "wasm32"))]
            worker_handles: Mutex::new(worker_handles),
        }
    }

    pub fn enqueue(&self, task: MoltTask) {
        if !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        if self.stealers.is_empty() {
            Self::execute_task(task, &self.injector);
        } else {
            self.injector.push(task);
        }
    }

    pub fn shutdown(&self) {
        if !self.running.swap(false, AtomicOrdering::SeqCst) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handles = {
                let mut guard = self.worker_handles.lock().unwrap();
                std::mem::take(&mut *guard)
            };
            for handle in handles {
                let _ = handle.join();
            }
        }
    }

    fn execute_task(task: MoltTask, injector: &Injector<MoltTask>) {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = injector;
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = (*header).poll_fn;
                if poll_fn_addr != 0 {
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    let _py = &_py;
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr);
                        prev
                    });
                    let token = ensure_task_token(_py, task_ptr, current_token_id());
                    let prev_token = set_current_token(_py, token);
                    let caller_depth = exception_stack_depth();
                    let caller_handlers =
                        EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    let caller_active = ACTIVE_EXCEPTION_STACK
                        .with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    let caller_context = caller_active
                        .last()
                        .copied()
                        .unwrap_or(MoltObject::none().bits());
                    exception_context_fallback_push(caller_context);
                    let task_handlers = task_exception_handler_stack_take(_py, task_ptr);
                    EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = task_handlers;
                    });
                    let task_active = task_exception_stack_take(_py, task_ptr);
                    ACTIVE_EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = task_active;
                    });
                    let task_depth = task_exception_depth_take(_py, task_ptr);
                    exception_stack_set_depth(_py, task_depth);
                    let prev_raise = task_raise_active();
                    set_task_raise_active(true);
                    loop {
                        let mut res = call_poll_fn(_py, poll_fn_addr, task_ptr);
                        if task_cancel_pending(task_ptr) {
                            task_take_cancel_pending(task_ptr);
                            res = raise_cancelled_with_message::<i64>(_py, task_ptr);
                        }
                        let pending = res == pending_bits_i64();
                        record_async_poll(_py, task_ptr, pending, "scheduler");
                        if pending {
                            if let Some(deadline) = runtime_state(_py)
                                .sleep_queue()
                                .take_blocking_deadline(_py, task_ptr)
                            {
                                let now = Instant::now();
                                if deadline > now {
                                    std::thread::sleep(deadline - now);
                                }
                            } else {
                                std::thread::yield_now();
                            }
                            continue;
                        }
                        let new_depth = exception_stack_depth();
                        task_exception_depth_store(_py, task_ptr, new_depth);
                        exception_context_align_depth(_py, new_depth);
                        let task_handlers =
                            EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                        task_exception_handler_stack_store(_py, task_ptr, task_handlers);
                        let task_active = ACTIVE_EXCEPTION_STACK
                            .with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                        task_exception_stack_store(_py, task_ptr, task_active);
                        ACTIVE_EXCEPTION_STACK.with(|stack| {
                            *stack.borrow_mut() = caller_active;
                        });
                        EXCEPTION_STACK.with(|stack| {
                            *stack.borrow_mut() = caller_handlers;
                        });
                        exception_stack_set_depth(_py, caller_depth);
                        exception_context_fallback_pop();
                        clear_task_token(_py, task_ptr);
                        set_task_raise_active(prev_raise);
                        break;
                    }
                    set_current_token(_py, prev_token);
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
                }
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = (*header).poll_fn;
                if poll_fn_addr != 0 {
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    let _py = &_py;
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr);
                        prev
                    });
                    let token = ensure_task_token(_py, task_ptr, current_token_id());
                    let prev_token = set_current_token(_py, token);
                    let caller_depth = exception_stack_depth();
                    let caller_handlers =
                        EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    let caller_active = ACTIVE_EXCEPTION_STACK
                        .with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    let caller_context = caller_active
                        .last()
                        .copied()
                        .unwrap_or(MoltObject::none().bits());
                    exception_context_fallback_push(caller_context);
                    let task_handlers = task_exception_handler_stack_take(_py, task_ptr);
                    EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = task_handlers;
                    });
                    let task_active = task_exception_stack_take(_py, task_ptr);
                    ACTIVE_EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = task_active;
                    });
                    let task_depth = task_exception_depth_take(_py, task_ptr);
                    exception_stack_set_depth(_py, task_depth);
                    let prev_raise = task_raise_active();
                    set_task_raise_active(true);
                    let mut res = call_poll_fn(_py, poll_fn_addr, task_ptr);
                    if task_cancel_pending(task_ptr) {
                        task_take_cancel_pending(task_ptr);
                        res = raise_cancelled_with_message::<i64>(_py, task_ptr);
                    }
                    let pending = res == pending_bits_i64();
                    record_async_poll(_py, task_ptr, pending, "scheduler");
                    let new_depth = exception_stack_depth();
                    task_exception_depth_store(_py, task_ptr, new_depth);
                    exception_context_align_depth(_py, new_depth);
                    let task_handlers =
                        EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    task_exception_handler_stack_store(_py, task_ptr, task_handlers);
                    let task_active = ACTIVE_EXCEPTION_STACK
                        .with(|stack| std::mem::take(&mut *stack.borrow_mut()));
                    task_exception_stack_store(_py, task_ptr, task_active);
                    ACTIVE_EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = caller_active;
                    });
                    EXCEPTION_STACK.with(|stack| {
                        *stack.borrow_mut() = caller_handlers;
                    });
                    exception_stack_set_depth(_py, caller_depth);
                    exception_context_fallback_pop();
                    if pending {
                        let waiting_on_event = task_waiting_on_event(_py, task_ptr);
                        let scheduled =
                            runtime_state(_py).sleep_queue().is_scheduled(_py, task_ptr);
                        if async_trace_enabled() {
                            eprintln!(
                                "molt async trace: poll_pending task=0x{:x} waiting_on_event={} scheduled={}",
                                task_ptr as usize, waiting_on_event, scheduled
                            );
                        }
                        if !waiting_on_event && !scheduled {
                            injector.push(task);
                        }
                    } else {
                        clear_task_token(_py, task_ptr);
                    }
                    set_task_raise_active(prev_raise);
                    set_current_token(_py, prev_token);
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
                }
            }
        }
    }
}

impl Default for MoltScheduler {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn wake_task_ptr(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    if current_task_key() == Some(PtrSlot(task_ptr)) {
        return;
    }
    let sleep_queue = runtime_state(_py).sleep_queue();
    sleep_queue.cancel_task(_py, task_ptr);
    runtime_state(_py).scheduler().enqueue(MoltTask {
        future_ptr: task_ptr,
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_block_on_task(task_ptr: *mut u8) -> bool {
    BLOCK_ON_TASK.with(|cell| cell.get() == task_ptr)
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_tokens(_py);
        let token = current_token_id();
        register_task_token(_py, task_ptr, token);
        let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) == 0 {
            (*header).flags |= HEADER_FLAG_SPAWN_RETAIN;
            inc_ref_bits(_py, MoltObject::from_ptr(task_ptr).bits());
        }
        runtime_state(_py).scheduler().enqueue(MoltTask {
            future_ptr: task_ptr,
        });
    })
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_tokens(_py);
        let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
        let poll_fn_addr = (*header).poll_fn;
        if poll_fn_addr == 0 {
            return 0;
        }
        let prev_task = CURRENT_TASK.with(|cell| {
            let prev = cell.get();
            cell.set(task_ptr);
            prev
        });
        let token = ensure_task_token(_py, task_ptr, current_token_id());
        let prev_token = set_current_token(_py, token);
        let caller_depth = exception_stack_depth();
        let caller_handlers =
            EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let task_handlers = task_exception_handler_stack_take(_py, task_ptr);
        EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = task_handlers;
        });
        let task_active = task_exception_stack_take(_py, task_ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = task_active;
        });
        let task_depth = task_exception_depth_take(_py, task_ptr);
        exception_stack_set_depth(_py, task_depth);
        BLOCK_ON_TASK.with(|cell| cell.set(task_ptr));
        let prev_raise = task_raise_active();
        set_task_raise_active(true);
        let result = loop {
            let mut res = {
                let _gil = GilGuard::new();
                call_poll_fn(_py, poll_fn_addr, task_ptr)
            };
            if task_cancel_pending(task_ptr) {
                task_take_cancel_pending(task_ptr);
                res = raise_cancelled_with_message::<i64>(_py, task_ptr);
            }
            let pending = res == pending_bits_i64();
            record_async_poll(_py, task_ptr, pending, "block_on");
            if pending {
                let deadline = runtime_state(_py)
                    .sleep_queue()
                    .take_blocking_deadline(_py, task_ptr);
                if let Some(awaited_ptr) = task_waiting_on_future(_py, task_ptr) {
                    if block_on_wait_event(_py, awaited_ptr, deadline) {
                        continue;
                    }
                }
                if let Some(deadline) = deadline {
                    let now = Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                } else {
                    std::thread::sleep(Duration::from_micros(50));
                }
                continue;
            }
            break res;
        };
        let new_depth = exception_stack_depth();
        task_exception_depth_store(_py, task_ptr, new_depth);
        exception_context_align_depth(_py, new_depth);
        let task_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        task_exception_handler_stack_store(_py, task_ptr, task_handlers);
        let task_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        task_exception_stack_store(_py, task_ptr, task_active);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_handlers;
        });
        exception_stack_set_depth(_py, caller_depth);
        exception_context_fallback_pop();
        BLOCK_ON_TASK.with(|cell| cell.set(std::ptr::null_mut()));
        set_task_raise_active(prev_raise);
        set_current_token(_py, prev_token);
        CURRENT_TASK.with(|cell| cell.set(prev_task));
        result
    })
}
