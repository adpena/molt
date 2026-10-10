use crate::PyToken;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use crossbeam_deque::{Injector, Worker};

use crate::object::ops::string_obj_to_owned;
use crate::{
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, GIL_DEPTH, GilGuard, GilReleaseGuard,
    HEADER_FLAG_BLOCK_ON, HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAG_TASK_DONE, HEADER_FLAG_TASK_QUEUED,
    HEADER_FLAG_TASK_RUNNING, HEADER_FLAG_TASK_WAKE_PENDING, MoltHeader, MoltObject, PtrSlot,
    anext_default_poll_fn_addr, async_sleep_poll_fn_addr, asyncgen_poll_fn_addr,
    class_name_for_error, code_filename_bits, code_name_bits, context_stack_unwind, dec_ref_bits,
    exception_context_align_depth, exception_context_fallback_pop, exception_context_fallback_push,
    exception_handler_active, exception_pending, exception_stack_baseline_get,
    exception_stack_baseline_set, exception_stack_depth, exception_stack_set_depth,
    generator_raise_active, header_from_obj_ptr, inc_ref_bits, io_wait_poll_fn_addr,
    maybe_ptr_from_bits, molt_exception_last, obj_from_bits, object_class_bits, object_type_id,
    pending_bits_i64, process_poll_fn_addr, promise_poll_fn_addr, ptr_from_bits, raise_exception,
    record_exception, resolve_task_ptr, runtime_state, set_task_raise_active,
    task_exception_baseline_store, task_exception_baseline_take, task_exception_depth_store,
    task_exception_depth_take, task_exception_handler_stack_store,
    task_exception_handler_stack_take, task_exception_stack_store, task_exception_stack_take,
    task_last_exception_contains_valid, task_raise_active, thread_poll_fn_addr, with_gil,
};

use super::cancellation::{
    cancel_tokens, clear_task_token, current_token_id, ensure_task_token,
    raise_cancelled_with_message, set_current_token, task_cancel_pending, task_take_cancel_pending,
};
use super::poll::call_scheduled_poll_fn;
use super::{spawned_task_count, spawned_task_inc};

// --- Scheduler ---

mod diagnostics;
#[cfg(not(target_arch = "wasm32"))]
use diagnostics::async_worker_threads;
use diagnostics::debug_current_task;
pub(crate) use diagnostics::{
    AsyncHangProbe, async_trace_enabled, record_async_poll, trace_task_result,
};

mod block_wait;
pub(crate) use block_wait::block_on_wait_spec;
use block_wait::{BLOCK_ON_MAX_WAIT, BLOCK_ON_MIN_SLEEP, BlockOnWaitSpec, block_on_poll_timeout};

mod sleep_queue;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use sleep_queue::sleep_worker;
pub(crate) use sleep_queue::{
    SleepQueue, instant_from_monotonic_secs, monotonic_now_nanos, monotonic_now_secs,
};

mod task_state;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use task_state::thread_task_state;
pub(crate) use task_state::{
    AwaitWaiterIndex, await_chain_terminal, await_waiter_clear, await_waiter_register,
    await_waiters, process_task_state, task_detach_owned_edges, task_exception_depths,
    task_exception_handler_stacks, task_exception_stacks, task_last_exceptions,
    task_visit_owned_edges, task_waiting_on, task_waiting_on_event, task_waiting_on_future,
    wake_await_waiters,
};

mod asyncio_runtime;
pub(crate) use asyncio_runtime::{
    AsyncioEventWaiterIndex, molt_asyncio_child_watcher_add, molt_asyncio_child_watcher_clear,
    molt_asyncio_child_watcher_pop, molt_asyncio_child_watcher_remove, molt_asyncio_enter_task,
    molt_asyncio_event_loop_get, molt_asyncio_event_loop_get_current,
    molt_asyncio_event_loop_policy_get, molt_asyncio_event_loop_policy_set,
    molt_asyncio_event_loop_set, molt_asyncio_event_waiters_cleanup_token,
    molt_asyncio_event_waiters_register, molt_asyncio_event_waiters_unregister,
    molt_asyncio_leave_task, molt_asyncio_register_task,
    molt_asyncio_require_child_watcher_support, molt_asyncio_require_unix_socket_support,
    molt_asyncio_running_loop_get, molt_asyncio_running_loop_set,
    molt_asyncio_ssl_transport_orchestrate, molt_asyncio_task_last_exception_clear,
    molt_asyncio_task_registry_contains, molt_asyncio_task_registry_current,
    molt_asyncio_task_registry_current_for_loop, molt_asyncio_task_registry_get,
    molt_asyncio_task_registry_live_set, molt_asyncio_task_registry_move,
    molt_asyncio_task_registry_pop, molt_asyncio_task_registry_set,
    molt_asyncio_task_registry_values, molt_asyncio_unregister_task,
};

thread_local! {
    pub(crate) static CURRENT_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    pub(crate) static BLOCK_ON_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
}

fn task_queue_lock() -> &'static Mutex<()> {
    static TASK_QUEUE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    TASK_QUEUE_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn current_task_ptr() -> *mut u8 {
    CURRENT_TASK.with(|cell| cell.get())
}

/// Install one execution-context owner on the current native thread and keep
/// the exception fast byte synchronized with that exact owner.
pub(crate) fn replace_current_task(_py: &PyToken<'_>, task_ptr: *mut u8) -> *mut u8 {
    let previous = CURRENT_TASK.with(|cell| cell.replace(task_ptr));
    crate::sync_current_exception_pending(_py, task_ptr);
    previous
}

pub(crate) struct CurrentTaskScope {
    previous: *mut u8,
}

impl CurrentTaskScope {
    pub(crate) fn enter(py: &PyToken<'_>, task_ptr: *mut u8) -> Self {
        Self {
            previous: replace_current_task(py, task_ptr),
        }
    }

    pub(crate) fn previous(&self) -> *mut u8 {
        self.previous
    }
}

impl Drop for CurrentTaskScope {
    fn drop(&mut self) {
        with_gil(|py| {
            replace_current_task(&py, self.previous);
        });
    }
}

pub(crate) fn current_task_key() -> Option<PtrSlot> {
    // Use try_with to avoid panicking during TLS destruction (e.g.,
    // when exception_pending is called from ThreadLocalGuard::drop).
    CURRENT_TASK
        .try_with(|cell| {
            let value = cell.get();
            if value.is_null() {
                None
            } else {
                Some(PtrSlot(value))
            }
        })
        .unwrap_or(None)
}

/// One external queue/current-poll owner. Moves between scheduler transports
/// do not retain again; destruction must occur outside their registry locks.
pub struct MoltTask {
    pub(super) future_ptr: *mut u8,
}

unsafe impl Send for MoltTask {}

impl MoltTask {
    pub(super) fn new(py: &PyToken<'_>, future_ptr: *mut u8) -> Self {
        debug_assert!(!future_ptr.is_null());
        inc_ref_bits(py, MoltObject::from_ptr(future_ptr).bits());
        Self { future_ptr }
    }

    pub(super) fn into_owned_bits(self) -> u64 {
        let this = std::mem::ManuallyDrop::new(self);
        MoltObject::from_ptr(this.future_ptr).bits()
    }
}

impl Drop for MoltTask {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
            crate::molt_dec_ref(self.future_ptr);
        });
    }
}

pub struct MoltScheduler {
    injector: Arc<Injector<MoltTask>>,
    running: Arc<AtomicBool>,
    deferred: Arc<Mutex<DeferredQueue>>,
    epoch: Arc<AtomicU64>,
    #[cfg(not(target_arch = "wasm32"))]
    worker_handles: Mutex<Vec<thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct DeferredQueue {
    // Reverse index and ordered owner follow the event-loop timer authority.
    // Sequence preserves FIFO; cancellation removes both coordinates directly.
    entries: HashMap<PtrSlot, (u64, u64)>,
    by_epoch: BTreeMap<(u64, u64), MoltTask>,
    next_sequence: u64,
}

impl DeferredQueue {
    fn insert(&mut self, task: MoltTask, target: u64) {
        let slot = PtrSlot(task.future_ptr);
        assert!(!self.entries.contains_key(&slot));
        let key = (target, self.next_sequence);
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .unwrap_or_else(|| std::process::abort());
        self.entries.insert(slot, key);
        // The unique sequence precludes replacement/drop under the mutex.
        self.by_epoch.insert(key, task);
    }

    fn remove(&mut self, slot: PtrSlot) -> Option<MoltTask> {
        let key = self.entries.remove(&slot)?;
        Some(self.by_epoch.remove(&key).expect("deferred owner"))
    }

    fn contains(&self, slot: PtrSlot) -> bool {
        self.entries.contains_key(&slot)
    }

    fn flush(&mut self, current: u64, injector: &Injector<MoltTask>) -> bool {
        let mut enqueued = false;
        while self
            .by_epoch
            .first_key_value()
            .is_some_and(|(&(epoch, _), _)| epoch <= current)
        {
            let (_, task) = self.by_epoch.pop_first().unwrap();
            self.entries.remove(&PtrSlot(task.future_ptr));
            injector.push(task);
            enqueued = true;
        }
        enqueued
    }
}

impl MoltScheduler {
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let num_threads = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let num_threads = async_worker_threads();
        let injector = Arc::new(Injector::new());
        let deferred = Arc::new(Mutex::new(DeferredQueue::default()));
        let epoch = Arc::new(AtomicU64::new(0));
        let mut workers: Vec<Worker<MoltTask>> = Vec::new();
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
                let deferred_clone = Arc::clone(&deferred);
                let epoch_clone = Arc::clone(&epoch);
                let stealers_clone = stealers.clone();
                let running_clone = Arc::clone(&running);

                let handle = thread::spawn(move || {
                    crate::state::run_runtime_worker(|| {
                        if async_trace_enabled() {
                            eprintln!("molt async trace: worker_start idx={}", i);
                        }
                        loop {
                            if !running_clone.load(AtomicOrdering::Relaxed) {
                                // Transfer local owners to shutdown's GIL-held
                                // drain; never abandon them with raw pointers.
                                while let Some(task) = worker.pop() {
                                    injector_clone.push(task);
                                }
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
                                let _ = epoch_clone.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                                if Self::flush_deferred_shared(
                                    &deferred_clone,
                                    &epoch_clone,
                                    &injector_clone,
                                ) {
                                    continue;
                                }
                                thread::yield_now();
                            }
                        }
                    })
                });
                worker_handles.push(handle);
            }
        }

        Self {
            injector,
            running,
            deferred,
            epoch,
            #[cfg(not(target_arch = "wasm32"))]
            worker_handles: Mutex::new(worker_handles),
        }
    }

    pub fn enqueue(&self, _py: &PyToken<'_>, task: MoltTask) {
        if !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: enqueue task=0x{:x}",
                task.future_ptr as usize
            );
        }
        if let Some(loop_handle) = super::cancellation::task_loop_handle(_py, task.future_ptr) {
            if let Err(rejected) = super::event_loop::enqueue_loop_task(_py, loop_handle, task) {
                task_clear_queue_flags(rejected.future_ptr);
                if let Some(bits) = super::cancellation::take_task_spawn_root(rejected.future_ptr) {
                    dec_ref_bits(_py, bits);
                }
                // Rejection keeps work-item custody until the final pointer use.
                drop(rejected);
            }
        } else {
            self.injector.push(task);
        }
    }

    pub(super) fn execute_loop_task(&self, task: MoltTask) {
        Self::execute_task(task, &self.injector);
    }

    fn advance_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, AtomicOrdering::SeqCst) + 1
    }

    pub(crate) fn defer_task_ptr(&self, _py: &PyToken<'_>, task_ptr: *mut u8) {
        if task_ptr.is_null() || !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        if super::cancellation::task_loop_handle(_py, task_ptr).is_some() {
            // During a poll this sets WAKE_PENDING. Its epilogue appends the
            // continuation after callbacks scheduled by the poll, for next turn.
            wake_task_ptr(_py, task_ptr);
            return;
        }
        let target = self.epoch.load(AtomicOrdering::Relaxed).saturating_add(1);
        let mut guard = self.deferred.lock().unwrap();
        if !guard.contains(PtrSlot(task_ptr)) {
            guard.insert(MoltTask::new(_py, task_ptr), target);
        }
    }

    pub(crate) fn clear_deferred(&self, task_ptr: *mut u8) {
        if task_ptr.is_null() {
            return;
        }
        let removed = self.deferred.lock().unwrap().remove(PtrSlot(task_ptr));
        drop(removed);
    }

    pub(crate) fn is_deferred(&self, task_ptr: *mut u8) -> bool {
        if task_ptr.is_null() {
            return false;
        }
        let guard = self.deferred.lock().unwrap();
        guard.contains(PtrSlot(task_ptr))
    }

    fn try_pop(&self) -> Option<MoltTask> {
        match self.injector.steal() {
            crossbeam_deque::Steal::Success(task) => Some(task),
            _ => None,
        }
    }

    fn flush_deferred(&self) -> bool {
        Self::flush_deferred_shared(&self.deferred, &self.epoch, &self.injector)
    }

    fn flush_deferred_shared(
        deferred: &Arc<Mutex<DeferredQueue>>,
        epoch: &Arc<AtomicU64>,
        injector: &Injector<MoltTask>,
    ) -> bool {
        let current = epoch.load(AtomicOrdering::Relaxed);
        let mut guard = deferred.lock().unwrap();
        guard.flush(current, injector)
    }

    pub(crate) fn drain_ready(&self) {
        self.advance_epoch();
        self.flush_deferred();
        #[cfg(target_arch = "wasm32")]
        {
            let gil = GilGuard::new();
            let py = gil.token();
            runtime_state(&py).io_poller().poll_host(&py);
        }
        while let Some(task) = self.try_pop() {
            Self::execute_task(task, &self.injector);
        }
    }

    pub fn shutdown(&self) {
        self.running.swap(false, AtomicOrdering::SeqCst);
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

    pub(crate) fn clear_stopped_queue(&self) {
        assert!(!self.running.load(AtomicOrdering::Acquire));
        let deferred = std::mem::take(&mut *self.deferred.lock().unwrap());
        drop(deferred);
        loop {
            match self.injector.steal() {
                crossbeam_deque::Steal::Success(task) => drop(task),
                crossbeam_deque::Steal::Retry => continue,
                crossbeam_deque::Steal::Empty => break,
            }
        }
    }

    fn execute_task(task: MoltTask, _injector: &Injector<MoltTask>) {
        let gil = GilGuard::new();
        let py = gil.token();
        // Local custody drops before the GIL and covers every exit, including
        // done/invalid-poll entries and all completion/finalizer callbacks.
        let task = task;
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = crate::object::object_poll_fn(task_ptr);
                let done = {
                    let _guard = task_queue_lock().lock().unwrap();
                    let done = ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0;
                    if done {
                        (*header).update_flags(
                            0,
                            HEADER_FLAG_TASK_QUEUED
                                | HEADER_FLAG_TASK_RUNNING
                                | HEADER_FLAG_TASK_WAKE_PENDING,
                        );
                    }
                    done
                };
                if done {
                    clear_task_token(&py, task_ptr);
                    return;
                }
                if poll_fn_addr != 0 {
                    if async_trace_enabled() {
                        eprintln!(
                            "molt async trace: poll_enter task=0x{:x} poll=0x{:x}",
                            task_ptr as usize, poll_fn_addr
                        );
                    }
                    let _py = &py;
                    let task_scope = CurrentTaskScope::enter(_py, task_ptr);
                    let prev_task = task_scope.previous();
                    {
                        let _guard = task_queue_lock().lock().unwrap();
                        let header = header_from_obj_ptr(task_ptr);
                        (*header).update_flags(HEADER_FLAG_TASK_RUNNING, HEADER_FLAG_TASK_QUEUED);
                    }
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
                    if async_trace_enabled() {
                        eprintln!(
                            "molt async trace: poll_start task=0x{:x} poll=0x{:x}",
                            task_ptr as usize, poll_fn_addr
                        );
                    }
                    let mut res = call_scheduled_poll_fn(_py, poll_fn_addr, task_ptr);
                    if task_cancel_pending(task_ptr) {
                        task_take_cancel_pending(task_ptr);
                        res = raise_cancelled_with_message::<i64>(_py, task_ptr);
                    }
                    let pending = res == pending_bits_i64();
                    let escaped = if !pending
                        && exception_pending(_py)
                        && super::cancellation::task_loop_handle(_py, task_ptr).is_some()
                    {
                        Some(molt_exception_last())
                    } else {
                        None
                    };
                    record_async_poll(_py, task_ptr, pending, "scheduler");
                    {
                        let _guard = task_queue_lock().lock().unwrap();
                        let header = header_from_obj_ptr(task_ptr);
                        (*header).take_flags(HEADER_FLAG_TASK_RUNNING);
                    }
                    let wake_pending = task_take_wake_pending(task_ptr);
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
                    exception_context_fallback_pop(_py);
                    if pending {
                        let waiting_on_event = task_waiting_on_event(_py, task_ptr);
                        let scheduled = task_sleep_scheduled(_py, task_ptr);
                        let deferred = runtime_state(_py).scheduler().is_deferred(task_ptr);
                        if async_trace_enabled() {
                            eprintln!(
                                "molt async trace: poll_pending task=0x{:x} waiting_on_event={} scheduled={} deferred={}",
                                task_ptr as usize, waiting_on_event, scheduled, deferred
                            );
                        }
                        if wake_pending || (!waiting_on_event && !scheduled && !deferred) {
                            enqueue_task_ptr(_py, task_ptr);
                        }
                    } else {
                        // Publish terminal state before dropping Context or
                        // exception edges that may reenter scheduler ingress.
                        task_mark_done(_py, task_ptr);
                        clear_task_token(_py, task_ptr);
                        let _ = task_take_wake_pending(task_ptr);
                        let _ = wake_await_waiters(_py, task_ptr);
                    }
                    set_task_raise_active(prev_raise);
                    set_current_token(_py, prev_token);
                    if debug_current_task() && prev_task.is_null() {
                        let current = CURRENT_TASK.with(|cell| cell.get());
                        if !current.is_null() {
                            eprintln!(
                                "molt task trace: scheduler restore null (ready) current=0x{:x} task=0x{:x}",
                                current as usize, task_ptr as usize
                            );
                        }
                    }
                    drop(task_scope);
                    if let Some(bits) = escaped {
                        if let Some(ptr) = maybe_ptr_from_bits(bits) {
                            record_exception(_py, ptr);
                        }
                        dec_ref_bits(_py, bits);
                    }
                }
                if poll_fn_addr == 0 {
                    task_mark_done(&py, task_ptr);
                    clear_task_token(&py, task_ptr);
                    if async_trace_enabled() {
                        eprintln!(
                            "molt async trace: poll_skip task=0x{:x} poll=0x0",
                            task_ptr as usize
                        );
                    }
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

fn task_take_wake_pending(task_ptr: *mut u8) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    let _guard = task_queue_lock().lock().unwrap();
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).take_flags(HEADER_FLAG_TASK_WAKE_PENDING) != 0
    }
}

fn task_clear_queue_flags(task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    let _guard = task_queue_lock().lock().unwrap();
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).update_flags(
            0,
            HEADER_FLAG_TASK_QUEUED | HEADER_FLAG_TASK_RUNNING | HEADER_FLAG_TASK_WAKE_PENDING,
        );
    }
}

pub(crate) fn task_mark_done(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    if trace_task_result() {
        eprintln!("molt task_result mark_done ptr=0x{:x}", task_ptr as usize);
    }
    if !task_last_exception_contains_valid(_py, task_ptr) && !exception_pending(_py) {
        crate::task_last_exception_drop(_py, task_ptr);
    }
    {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            (*header).update_flags(
                HEADER_FLAG_TASK_DONE,
                HEADER_FLAG_TASK_QUEUED | HEADER_FLAG_TASK_RUNNING | HEADER_FLAG_TASK_WAKE_PENDING,
            );
        }
    }
    if let Some(scheduler) = runtime_state(_py).scheduler.get() {
        scheduler.clear_deferred(task_ptr);
    }
    // Publish terminal state and release the queue lock before a displaced
    // continuation can run a destructor. Wakeup custody is independent.
    let awaited = unsafe { crate::object::aux_header::object_take_frame_awaited_bits(task_ptr) };
    if awaited != 0 {
        dec_ref_bits(_py, awaited);
    }
    // The terminal transition hands the coroutine frame's bindings to a frame
    // object that shares them, or releases them.
    unsafe { crate::builtins::frames::activation_exit_bindings(_py, task_ptr) };
}

pub(crate) fn task_result_get(_py: &PyToken<'_>, task_ptr: *mut u8) -> Option<u64> {
    if task_ptr.is_null() {
        return None;
    }
    let result = {
        let guard = runtime_state(_py).task_results.lock().unwrap();
        guard.get(&PtrSlot(task_ptr)).copied()
    }?;
    if trace_task_result() {
        eprintln!(
            "molt task_result get ptr=0x{:x} result=0x{:x}",
            task_ptr as usize, result
        );
    }
    inc_ref_bits(_py, result);
    Some(result)
}

pub(crate) fn task_result_store(_py: &PyToken<'_>, task_ptr: *mut u8, result_bits: u64) {
    if task_ptr.is_null() {
        return;
    }
    if trace_task_result() {
        eprintln!(
            "molt task_result store ptr=0x{:x} result=0x{:x}",
            task_ptr as usize, result_bits
        );
    }
    inc_ref_bits(_py, result_bits);
    let old = {
        let mut guard = runtime_state(_py).task_results.lock().unwrap();
        guard.insert(PtrSlot(task_ptr), result_bits)
    };
    if let Some(old_bits) = old {
        dec_ref_bits(_py, old_bits);
    }
}

pub(crate) fn task_result_drop(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    if trace_task_result() {
        eprintln!("molt task_result drop ptr=0x{:x}", task_ptr as usize);
    }
    let old = {
        let mut guard = runtime_state(_py).task_results.lock().unwrap();
        guard.remove(&PtrSlot(task_ptr))
    };
    if let Some(old_bits) = old {
        dec_ref_bits(_py, old_bits);
    }
}

pub(super) fn enqueue_task_ptr(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    let mut should_enqueue = false;
    let mut should_return = false;
    {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            let flags = (*header).load_synchronized_flags();
            if (flags & HEADER_FLAG_TASK_DONE) != 0 {
                should_return = true;
            }
            if (flags & HEADER_FLAG_BLOCK_ON) != 0 {
                should_return = true;
            }
            if !should_return && (flags & HEADER_FLAG_TASK_RUNNING) != 0 {
                (*header).fetch_or_flags(HEADER_FLAG_TASK_WAKE_PENDING);
                should_return = true;
            }
            if !should_return && (flags & HEADER_FLAG_TASK_QUEUED) != 0 {
                should_return = true;
            }
            if !should_return {
                (*header).fetch_or_flags(HEADER_FLAG_TASK_QUEUED);
                should_enqueue = true;
            }
        }
    }
    if should_return {
        return;
    }
    if should_enqueue {
        runtime_state(_py)
            .scheduler()
            .enqueue(_py, MoltTask::new(_py, task_ptr));
    }
}

pub(crate) fn wake_task_ptr(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    runtime_state(_py).scheduler().clear_deferred(task_ptr);
    if current_task_key() == Some(PtrSlot(task_ptr)) {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            if ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0 {
                return;
            }
            if async_trace_enabled() {
                eprintln!(
                    "molt async trace: wake_task_self task=0x{:x}",
                    task_ptr as usize
                );
            }
            (*header).fetch_or_flags(HEADER_FLAG_TASK_WAKE_PENDING);
        }
        return;
    }
    cancel_task_sleep(_py, task_ptr);
    let mut should_enqueue = false;
    let mut should_return = false;
    let inline_only = {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            let flags = (*header).load_synchronized_flags();
            let done = (flags & HEADER_FLAG_TASK_DONE) != 0;
            let block_on = (flags & HEADER_FLAG_BLOCK_ON) != 0;
            let running = (flags & HEADER_FLAG_TASK_RUNNING) != 0;
            let queued = (flags & HEADER_FLAG_TASK_QUEUED) != 0;
            let spawned = (flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
            let inline_only = !spawned && !block_on;
            if async_trace_enabled() {
                eprintln!(
                    "molt async trace: wake_task task=0x{:x} done={} block_on={} running={} queued={}",
                    task_ptr as usize, done, block_on, running, queued
                );
            }
            if done {
                should_return = true;
            }
            if !should_return && block_on {
                (*header).fetch_or_flags(HEADER_FLAG_TASK_WAKE_PENDING);
                should_return = true;
            }
            if !should_return && running {
                (*header).fetch_or_flags(HEADER_FLAG_TASK_WAKE_PENDING);
                should_return = true;
            }
            if !should_return && queued {
                should_return = true;
            }
            if !should_return && !inline_only {
                (*header).fetch_or_flags(HEADER_FLAG_TASK_QUEUED);
                should_enqueue = true;
            }
            inline_only
        }
    };
    if should_return {
        return;
    }
    if inline_only {
        let waiters = await_waiters(_py)
            .lock()
            .unwrap()
            .get(&PtrSlot(task_ptr))
            .cloned()
            .unwrap_or_default();
        for waiter in waiters {
            wake_task_ptr(_py, waiter.0);
        }
        return;
    }
    if should_enqueue {
        runtime_state(_py)
            .scheduler()
            .enqueue(_py, MoltTask::new(_py, task_ptr));
    }
}

/// Route deadlines by execution ownership before constructing an unbound worker.
pub(crate) fn register_task_sleep(py: &PyToken<'_>, task: *mut u8, deadline: Instant) {
    if let Some(owner) = super::cancellation::task_loop_handle(py, task) {
        super::event_loop::register_loop_sleep(py, owner, task, deadline);
    } else {
        runtime_state(py)
            .sleep_queue()
            .register_scheduler(py, task, deadline);
    }
}

pub(crate) fn task_sleep_scheduled(py: &PyToken<'_>, task: *mut u8) -> bool {
    if let Some(owner) = super::cancellation::task_loop_handle(py, task) {
        super::event_loop::loop_task_sleep_scheduled(py, owner, task)
    } else {
        runtime_state(py)
            .sleep_queue
            .get()
            .is_some_and(|queue| queue.is_scheduled(py, task))
    }
}

/// Remove metadata first; the caller chooses immediate or deferred edge release.
pub(crate) fn take_task_sleep(py: &PyToken<'_>, task: *mut u8) -> Option<u64> {
    if let Some(owner) = super::cancellation::task_loop_handle(py, task) {
        super::event_loop::take_loop_sleep(py, owner, task)
    } else {
        if let Some(queue) = runtime_state(py).sleep_queue.get() {
            queue.cancel_task(py, task);
        }
        None
    }
}

pub(crate) fn cancel_task_sleep(py: &PyToken<'_>, task: *mut u8) {
    if let Some(bits) = take_task_sleep(py, task) {
        dec_ref_bits(py, bits);
    }
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(task_ptr) = resolve_task_ptr(task_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            };
            // Repeated spawn of a terminal task cannot establish an external
            // reference for work that enqueue will necessarily reject.
            let header = header_from_obj_ptr(task_ptr);
            if (*header).has_flag(HEADER_FLAG_TASK_DONE) {
                return MoltObject::none().bits();
            }
            if async_trace_enabled() {
                let poll_fn = crate::object::object_poll_fn(task_ptr);
                eprintln!(
                    "molt async trace: spawn task=0x{:x} poll=0x{:x}",
                    task_ptr as usize, poll_fn
                );
            }
            cancel_tokens(_py);
            // Capture an independent execution Context on the submitting thread.
            let _ = ensure_task_token(_py, task_ptr, current_token_id());
            if !super::cancellation::ensure_scheduled_context(_py, task_ptr) {
                return MoltObject::none().bits();
            }
            if ((*header).fetch_or_flags(HEADER_FLAG_SPAWN_RETAIN) & HEADER_FLAG_SPAWN_RETAIN) == 0
            {
                inc_ref_bits(_py, MoltObject::from_ptr(task_ptr).bits());
                spawned_task_inc();
            }
            enqueue_task_ptr(_py, task_ptr);
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_block_on(task_bits: u64) -> i64 {
    unsafe {
        let (
            task_ptr,
            poll_fn_addr,
            task_scope,
            prev_task,
            prev_token,
            caller_depth,
            caller_baseline,
            caller_handlers,
            caller_active,
            prev_raise,
        ) = {
            let _gil = GilGuard::new();
            let _py = _gil.token();
            let _py = &_py;
            let Some(task_ptr) = resolve_task_ptr(task_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            };
            if async_trace_enabled() {
                eprintln!("molt async trace: block_on task=0x{:x}", task_ptr as usize);
            }
            cancel_tokens(_py);
            let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
            let poll_fn_addr = crate::object::object_poll_fn(task_ptr);
            if poll_fn_addr == 0 {
                return 0;
            }
            let task_scope = CurrentTaskScope::enter(_py, task_ptr);
            let prev_task = task_scope.previous();
            let token = ensure_task_token(_py, task_ptr, current_token_id());
            let prev_token = set_current_token(_py, token);
            let caller_depth = exception_stack_depth();
            let caller_baseline = exception_stack_baseline_get();
            let caller_handlers =
                EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let task_baseline = task_exception_baseline_take(_py, task_ptr);
            exception_stack_baseline_set(task_baseline);
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
            (*header).fetch_or_flags(HEADER_FLAG_BLOCK_ON);
            BLOCK_ON_TASK.with(|cell| cell.set(task_ptr));
            let prev_raise = task_raise_active();
            set_task_raise_active(true);
            (
                task_ptr,
                poll_fn_addr,
                task_scope,
                prev_task,
                prev_token,
                caller_depth,
                caller_baseline,
                caller_handlers,
                caller_active,
                prev_raise,
            )
        };
        if async_trace_enabled() {
            let depth = GIL_DEPTH.with(|depth| depth.get());
            eprintln!("molt async trace: block_on_gil_depth={}", depth);
        }

        let result = loop {
            {
                let _gil = GilGuard::new();
                let _py = _gil.token();
                // Consume any pending wake flag; we are about to poll the root task.
                let _ = task_take_wake_pending(task_ptr);
            }
            let (pending, wait_spec, deadline, res) = {
                let _gil = GilGuard::new();
                let _py = _gil.token();
                let _py = &_py;
                let mut res = call_scheduled_poll_fn(_py, poll_fn_addr, task_ptr);
                if res != pending_bits_i64() && !exception_pending(_py) {
                    crate::task_last_exception_drop(_py, task_ptr);
                }
                if matches!(
                    std::env::var("MOLT_TRACE_BLOCK_ON_RESULT").ok().as_deref(),
                    Some("1")
                ) {
                    let pending_kind = if exception_pending(_py) {
                        let exc_bits = crate::exception_last_bits_noinc(_py)
                            .unwrap_or_else(|| MoltObject::none().bits());
                        if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                            crate::builtins::exceptions::exception_diagnostic_name(exc_ptr)
                        } else {
                            "<none>".to_string()
                        }
                    } else {
                        "<none>".to_string()
                    };
                    eprintln!(
                        "molt block_on poll result=0x{:x} pending_kind={}",
                        res, pending_kind
                    );
                }
                if task_cancel_pending(task_ptr) {
                    if exception_pending(_py) {
                        let _ = task_take_cancel_pending(task_ptr);
                    } else if res == pending_bits_i64() {
                        let _ = task_take_cancel_pending(task_ptr);
                        res = raise_cancelled_with_message::<i64>(_py, task_ptr);
                    } else {
                        let _ = task_take_cancel_pending(task_ptr);
                    }
                }
                let pending = res == pending_bits_i64();
                record_async_poll(_py, task_ptr, pending, "block_on");
                if pending {
                    let blocking_deadline = runtime_state(_py)
                        .sleep_queue()
                        .take_blocking_deadline(_py, task_ptr);
                    let scheduler_deadline =
                        runtime_state(_py).sleep_queue().next_scheduler_deadline();
                    let deadline = match (blocking_deadline, scheduler_deadline) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (Some(a), None) => Some(a),
                        (None, Some(b)) => Some(b),
                        (None, None) => None,
                    };
                    let awaited_ptr = task_waiting_on_future(_py, task_ptr);
                    if matches!(
                        std::env::var("MOLT_TRACE_BLOCK_ON").ok().as_deref(),
                        Some("1")
                    ) {
                        if let Some(ptr) = awaited_ptr {
                            let poll_fn = crate::object::object_poll_fn(ptr);
                            let poll_kind = |addr: u64| -> &'static str {
                                if addr == async_sleep_poll_fn_addr() {
                                    "sleep"
                                } else if addr == promise_poll_fn_addr() {
                                    "promise"
                                } else if addr == io_wait_poll_fn_addr() {
                                    "io_wait"
                                } else if addr == thread_poll_fn_addr() {
                                    "thread"
                                } else if addr == process_poll_fn_addr() {
                                    "process"
                                } else if addr == asyncgen_poll_fn_addr() {
                                    "asyncgen"
                                } else if addr == anext_default_poll_fn_addr() {
                                    "anext_default"
                                } else {
                                    "other"
                                }
                            };
                            let kind = poll_kind(poll_fn);
                            let mut detail = String::new();
                            if kind == "other" {
                                let class_bits = object_class_bits(ptr);
                                let class_name = class_name_for_error(class_bits);
                                let type_id = object_type_id(ptr);
                                detail = format!(" type_id={} class={}", type_id, class_name);
                                let code_bits =
                                    crate::object::aux_header::object_frame_code_bits(ptr);
                                if code_bits != 0 {
                                    let code_ptr = ptr_from_bits(code_bits);
                                    if !code_ptr.is_null() {
                                        let name_bits = code_name_bits(code_ptr);
                                        let file_bits = code_filename_bits(code_ptr);
                                        let name = string_obj_to_owned(obj_from_bits(name_bits))
                                            .unwrap_or_default();
                                        let file = string_obj_to_owned(obj_from_bits(file_bits))
                                            .unwrap_or_default();
                                        if !name.is_empty() || !file.is_empty() {
                                            detail = format!(
                                                " type_id={} class={} code={} file={}",
                                                type_id, class_name, name, file
                                            );
                                        }
                                    }
                                }
                                if matches!(
                                    std::env::var("MOLT_TRACE_BLOCK_ON_CHAIN").ok().as_deref(),
                                    Some("1")
                                ) {
                                    let mut cursor = ptr;
                                    for depth in 0..8 {
                                        let cursor_poll = crate::object::object_poll_fn(cursor);
                                        let cursor_kind = poll_kind(cursor_poll);
                                        eprintln!(
                                            "molt async trace: block_on_chain depth={} ptr=0x{:x} poll=0x{:x} kind={}",
                                            depth, cursor as usize, cursor_poll, cursor_kind
                                        );
                                        let next = {
                                            let waiting_map = task_waiting_on(_py).lock().unwrap();
                                            waiting_map.get(&PtrSlot(cursor)).map(|val| val.0)
                                        };
                                        let Some(next_ptr) = next else {
                                            break;
                                        };
                                        if next_ptr.is_null() || next_ptr == cursor {
                                            break;
                                        }
                                        cursor = next_ptr;
                                    }
                                }
                            }
                            eprintln!(
                                "molt async trace: block_on_wait task=0x{:x} awaited=0x{:x} poll=0x{:x} kind={}{}",
                                task_ptr as usize, ptr as usize, poll_fn, kind, detail
                            );
                        } else {
                            eprintln!(
                                "molt async trace: block_on_wait task=0x{:x} awaited=none",
                                task_ptr as usize
                            );
                        }
                    }
                    let wait_spec = awaited_ptr
                        .and_then(|awaited_ptr| block_on_wait_spec(_py, awaited_ptr, deadline));
                    (pending, wait_spec, deadline, res)
                } else {
                    (pending, None, None, res)
                }
            };
            if pending {
                {
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    {
                        let due = runtime_state(&_py)
                            .sleep_queue()
                            .take_due_scheduler_tasks(&_py);
                        for due_task in due {
                            enqueue_task_ptr(&_py, due_task);
                        }
                    }
                    runtime_state(&_py).scheduler().drain_ready();
                }
                let wake_pending = {
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    task_take_wake_pending(task_ptr)
                };
                if wake_pending {
                    std::thread::sleep(BLOCK_ON_MIN_SLEEP);
                    continue;
                }
                if let Some(spec) = wait_spec {
                    let _release = GilReleaseGuard::suspend();
                    #[cfg(not(target_arch = "wasm32"))]
                    match spec {
                        BlockOnWaitSpec::Io {
                            poller,
                            socket_ptr,
                            events,
                            timeout,
                        } => {
                            let wait = block_on_poll_timeout(timeout);
                            let _ = poller.wait_blocking(socket_ptr, events, Some(wait));
                        }
                        BlockOnWaitSpec::Thread { state, timeout } => {
                            let wait = block_on_poll_timeout(timeout);
                            state.wait_blocking(Some(wait));
                        }
                        BlockOnWaitSpec::Process { state, timeout } => {
                            let wait = block_on_poll_timeout(timeout);
                            state.wait_blocking(Some(wait));
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let _ = spec;
                    }
                    continue;
                }
                let refreshed_deadline = {
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    let scheduler_deadline =
                        runtime_state(&_py).sleep_queue().next_scheduler_deadline();
                    match (deadline, scheduler_deadline) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (Some(a), None) => Some(a),
                        (None, Some(b)) => Some(b),
                        (None, None) => None,
                    }
                };
                let spawned = spawned_task_count();
                if let Some(deadline) = refreshed_deadline {
                    let _release = GilReleaseGuard::suspend();
                    let now = Instant::now();
                    if deadline > now {
                        // Cap block_on sleeps so external wakeups (io/thread/process/task) are
                        // observed promptly instead of stalling until long deadlines.
                        let mut wait = (deadline - now).min(BLOCK_ON_MAX_WAIT);
                        if spawned > 0 && wait < BLOCK_ON_MIN_SLEEP {
                            wait = BLOCK_ON_MIN_SLEEP;
                        }
                        std::thread::sleep(wait);
                    } else if spawned > 0 {
                        std::thread::sleep(BLOCK_ON_MIN_SLEEP);
                    } else {
                        std::thread::yield_now();
                    }
                } else {
                    let _release = GilReleaseGuard::suspend();
                    std::thread::sleep(BLOCK_ON_MIN_SLEEP);
                }
                continue;
            }
            // Even when the root task reports ready, CPython drains the ready queue
            // before fully stopping the loop. Run ready tasks and retry if they
            // scheduled a cancellation or wake-up for the root task.
            {
                let _gil = GilGuard::new();
                let _py = _gil.token();
                let _py = &_py;
                runtime_state(_py).scheduler().drain_ready();
                // Once the root task is ready, don't re-poll it; clear pending wake/cancel flags.
                task_mark_done(_py, task_ptr);
                let _ = task_take_cancel_pending(task_ptr);
                let _ = task_take_wake_pending(task_ptr);
            }
            break res;
        };

        {
            let _gil = GilGuard::new();
            let _py = _gil.token();
            let _py = &_py;
            let trace_epilogue = matches!(
                std::env::var("MOLT_TRACE_BLOCK_ON_EPILOGUE")
                    .ok()
                    .as_deref(),
                Some("1")
            );
            let trace_step = |label: &str| {
                if !trace_epilogue {
                    return;
                }
                let pending = exception_pending(_py);
                let kind = if pending {
                    let exc_bits = crate::exception_last_bits_noinc(_py)
                        .unwrap_or_else(|| MoltObject::none().bits());
                    if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                        crate::builtins::exceptions::exception_diagnostic_name(exc_ptr)
                    } else {
                        "<none>".to_string()
                    }
                } else {
                    "<none>".to_string()
                };
                eprintln!(
                    "molt block_on epilogue step={} pending={} kind={}",
                    label, pending, kind
                );
            };
            let new_depth = exception_stack_depth();
            trace_step("start");
            task_exception_depth_store(_py, task_ptr, new_depth);
            trace_step("task_exception_depth_store");
            exception_context_align_depth(_py, new_depth);
            trace_step("exception_context_align_depth");
            let new_baseline = exception_stack_baseline_get();
            task_exception_baseline_store(_py, task_ptr, new_baseline);
            trace_step("task_exception_baseline_store");
            exception_stack_baseline_set(caller_baseline);
            trace_step("exception_stack_baseline_set");
            let task_handlers =
                EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            task_exception_handler_stack_store(_py, task_ptr, task_handlers);
            trace_step("task_exception_handler_stack_store");
            let task_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            task_exception_stack_store(_py, task_ptr, task_active);
            trace_step("task_exception_stack_store");
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            trace_step("restore_active_exception_stack");
            EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_handlers;
            });
            trace_step("restore_exception_stack");
            exception_stack_set_depth(_py, caller_depth);
            trace_step("exception_stack_set_depth");
            exception_context_fallback_pop(_py);
            trace_step("exception_context_fallback_pop");
            // Move any pending exception off the block_on task and onto the caller/global slot.
            let task_exc_slot = task_last_exceptions(_py)
                .lock()
                .unwrap()
                .remove(&PtrSlot(task_ptr));
            crate::CURRENT_EXCEPTION_PENDING.with(|pending| pending.set(false));
            trace_step("task_exc_slot_taken");
            let pending_bits = if let Some(exc_slot) = task_exc_slot {
                MoltObject::from_ptr(exc_slot.0).bits()
            } else if exception_pending(_py) {
                molt_exception_last()
            } else {
                MoltObject::none().bits()
            };
            trace_step("pending_bits_selected");
            if let Some(exc_ptr) = maybe_ptr_from_bits(pending_bits) {
                let restore_task = current_task_ptr();
                if debug_current_task() && prev_task.is_null() && !restore_task.is_null() {
                    eprintln!(
                        "molt task trace: block_on temp restore null current=0x{:x} task=0x{:x}",
                        restore_task as usize, task_ptr as usize
                    );
                }
                let caller_scope = CurrentTaskScope::enter(_py, prev_task);
                record_exception(_py, exc_ptr);
                drop(caller_scope);
                debug_assert_eq!(current_task_ptr(), restore_task);
                trace_step("record_exception");
            }
            if !obj_from_bits(pending_bits).is_none() {
                dec_ref_bits(_py, pending_bits);
                trace_step("pending_bits_dec_ref");
            }
            let header = header_from_obj_ptr(task_ptr);
            (*header).fetch_and_flags(!HEADER_FLAG_BLOCK_ON);
            trace_step("clear_block_on_flag");
            task_mark_done(_py, task_ptr);
            trace_step("task_mark_done");
            clear_task_token(_py, task_ptr);
            trace_step("clear_task_token");
            let _ = task_take_wake_pending(task_ptr);
            trace_step("clear_wake_pending");
            let _ = wake_await_waiters(_py, task_ptr);
            trace_step("wake_await_waiters");
            BLOCK_ON_TASK.with(|cell| cell.set(std::ptr::null_mut()));
            trace_step("clear_block_on_task");
            set_task_raise_active(prev_raise);
            trace_step("set_task_raise_active");
            set_current_token(_py, prev_token);
            trace_step("set_current_token");
            if debug_current_task() && prev_task.is_null() {
                let current = CURRENT_TASK.with(|cell| cell.get());
                if !current.is_null() {
                    eprintln!(
                        "molt task trace: block_on restore null current=0x{:x} task=0x{:x}",
                        current as usize, task_ptr as usize
                    );
                }
            }
            drop(task_scope);
            trace_step("restore_current_task");
            let pending_after = exception_pending(_py);
            let handlers_active = exception_handler_active();
            let generator_raise = generator_raise_active();
            let task_raise = task_raise_active();
            let trace_block_on = matches!(
                std::env::var("MOLT_TRACE_BLOCK_ON").ok().as_deref(),
                Some("1")
            );
            if prev_task.is_null() && trace_block_on {
                eprintln!(
                    "molt async trace: block_on_exit pending={} handlers={} gen_raise={} task_raise={}",
                    pending_after, handlers_active, generator_raise, task_raise
                );
            }
            if prev_task.is_null()
                && pending_after
                && !handlers_active
                && !generator_raise
                && !task_raise
            {
                let exc_bits = molt_exception_last();
                if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                    context_stack_unwind(_py, MoltObject::from_ptr(exc_ptr).bits());
                }
                if !obj_from_bits(exc_bits).is_none() {
                    dec_ref_bits(_py, exc_bits);
                }
            }
        }
        result
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod context_queue_root_tests {
    use super::*;
    use crate::async_rt::cancellation::{TaskContextBinding, register_task_execution};
    use crate::builtins::contextvars as context;
    use crate::object::weakref::WeakBorrow;
    use std::sync::atomic::AtomicUsize;

    static POLLS: AtomicUsize = AtomicUsize::new(0);
    static OBSERVED: AtomicU64 = AtomicU64::new(0);
    static DEFER_FIRST: AtomicBool = AtomicBool::new(false);
    static CANCEL_AFTER_OBSERVE: AtomicBool = AtomicBool::new(false);

    extern "C" fn poll(raw: u64) -> i64 {
        with_gil(|py| unsafe {
            let ptr = std::ptr::with_exposed_provenance_mut::<u8>(raw as usize);
            let var = *ptr.add(crate::GEN_CONTROL_SIZE).cast::<u64>();
            let value = context::get_variable(&py, var, None).flatten().unwrap_or(0);
            OBSERVED.store(value, AtomicOrdering::SeqCst);
            if value != 0 {
                dec_ref_bits(&py, value);
            }
            if CANCEL_AFTER_OBSERVE.load(AtomicOrdering::SeqCst) {
                crate::async_rt::cancellation::task_set_cancel_pending(ptr);
            }
            let first = POLLS.fetch_add(1, AtomicOrdering::SeqCst) == 0;
            if first && DEFER_FIRST.load(AtomicOrdering::SeqCst) {
                runtime_state(&py).scheduler().defer_task_ptr(&py, ptr);
                pending_bits_i64()
            } else {
                MoltObject::none().bits() as i64
            }
        })
    }

    fn var(py: &PyToken<'_>, name: &[u8]) -> u64 {
        let name = MoltObject::from_ptr(crate::alloc_string(py, name)).bits();
        let var = context::new_variable(py, name, None).unwrap();
        dec_ref_bits(py, name);
        var
    }

    fn install_deterministic_scheduler(py: &PyToken<'_>) {
        // This is the production scheduler with no worker threads: a real
        // spawn remains in its injector until this test calls drain_ready.
        let scheduler = MoltScheduler {
            injector: Arc::new(Injector::new()),
            running: Arc::new(AtomicBool::new(true)),
            deferred: Arc::new(Mutex::new(DeferredQueue::default())),
            epoch: Arc::new(AtomicU64::new(0)),
            worker_handles: Mutex::new(Vec::new()),
        };
        assert!(runtime_state(py).scheduler.set(scheduler).is_ok());
    }

    #[test]
    fn real_spawn_and_deferred_poll_keep_task_context_alive_without_caller_owner() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            with_gil(|py| {
                install_deterministic_scheduler(&py);
                let scheduler = runtime_state(&py).scheduler();
                for kind in [crate::TASK_KIND_GENERATOR, crate::TASK_KIND_FUTURE] {
                    for deferred in [false, true] {
                        POLLS.store(0, AtomicOrdering::SeqCst);
                        OBSERVED.store(0, AtomicOrdering::SeqCst);
                        DEFER_FIRST.store(deferred, AtomicOrdering::SeqCst);
                        let value_var = var(&py, b"queued-value");
                        let cycle_var = var(&py, b"queued-cycle");
                        let selected = context::new_context(&py).unwrap();
                        let task = crate::molt_task_new(
                            poll as *const () as usize as u64,
                            (crate::GEN_CONTROL_SIZE + 8) as u64,
                            kind,
                        );
                        let ptr = obj_from_bits(task).as_ptr().unwrap();
                        unsafe {
                            crate::object::payload_refs::store_borrowed(
                                &py,
                                ptr,
                                crate::GEN_CONTROL_SIZE,
                                value_var,
                            );
                        }
                        {
                            let _entered = context::EnteredContext::enter(&py, selected).unwrap();
                            for (key, value) in [
                                (value_var, MoltObject::from_int(77).bits()),
                                (cycle_var, task),
                            ] {
                                let token = context::set_variable(&py, key, value).unwrap();
                                dec_ref_bits(&py, token);
                            }
                        }
                        register_task_execution(&py, ptr, 1, TaskContextBinding::Owned(selected));
                        let task_watch = WeakBorrow::new(&py, task).unwrap();
                        let context_watch = WeakBorrow::new(&py, selected).unwrap();
                        let spawn_count = spawned_task_count();
                        unsafe {
                            molt_spawn(task);
                        }
                        dec_ref_bits(&py, task);
                        dec_ref_bits(&py, selected);
                        unsafe {
                            crate::object::gc::collect_cycles(&py);
                        }
                        assert_eq!(POLLS.load(AtomicOrdering::SeqCst), 0);
                        for watch in [&task_watch, &context_watch] {
                            let live = watch
                                .upgrade_owned()
                                .expect("queued task and Context survive GC");
                            dec_ref_bits(&py, live);
                        }
                        // Separate the suspended execution root from queue
                        // custody. The external spawn root alone must survive GC.
                        drop(scheduler.try_pop().expect("real queued work"));
                        unsafe {
                            crate::object::gc::collect_cycles(&py);
                        }
                        let live = task_watch
                            .upgrade_owned()
                            .expect("spawn is an external GC root");
                        task_clear_queue_flags(ptr);
                        enqueue_task_ptr(&py, ptr);
                        dec_ref_bits(&py, live);
                        scheduler.drain_ready();
                        if deferred {
                            assert_eq!(POLLS.load(AtomicOrdering::SeqCst), 1);
                            unsafe {
                                crate::object::gc::collect_cycles(&py);
                            }
                            let live = task_watch
                                .upgrade_owned()
                                .expect("deferred owner survives GC");
                            dec_ref_bits(&py, live);
                            scheduler.drain_ready();
                        }
                        assert_eq!(
                            POLLS.load(AtomicOrdering::SeqCst),
                            if deferred { 2 } else { 1 }
                        );
                        assert_eq!(
                            OBSERVED.load(AtomicOrdering::SeqCst),
                            MoltObject::from_int(77).bits()
                        );
                        assert_eq!(spawned_task_count(), spawn_count);
                        unsafe {
                            crate::object::gc::collect_cycles(&py);
                        }
                        assert!(task_watch.upgrade_owned().is_none());
                        assert!(context_watch.upgrade_owned().is_none());
                        dec_ref_bits(&py, value_var);
                        dec_ref_bits(&py, cycle_var);
                    }
                }
            });
        });
    }

    #[test]
    fn unspawned_task_context_cycle_has_no_scheduler_root() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            with_gil(|py| {
                POLLS.store(0, AtomicOrdering::SeqCst);
                for kind in [crate::TASK_KIND_GENERATOR, crate::TASK_KIND_FUTURE] {
                    let variable = var(&py, b"unspawned-cycle");
                    let selected = context::new_context(&py).unwrap();
                    let task = crate::molt_task_new(
                        poll as *const () as usize as u64,
                        crate::GEN_CONTROL_SIZE as u64,
                        kind,
                    );
                    let ptr = obj_from_bits(task).as_ptr().unwrap();
                    {
                        let _entered = context::EnteredContext::enter(&py, selected).unwrap();
                        let token = context::set_variable(&py, variable, task).unwrap();
                        dec_ref_bits(&py, token);
                    }
                    register_task_execution(&py, ptr, 1, TaskContextBinding::Owned(selected));
                    let task_watch = WeakBorrow::new(&py, task).unwrap();
                    let context_watch = WeakBorrow::new(&py, selected).unwrap();
                    dec_ref_bits(&py, task);
                    dec_ref_bits(&py, selected);
                    unsafe {
                        crate::object::gc::collect_cycles(&py);
                    }
                    assert!(task_watch.upgrade_owned().is_none());
                    assert!(context_watch.upgrade_owned().is_none());
                    dec_ref_bits(&py, variable);
                }
                assert_eq!(POLLS.load(AtomicOrdering::SeqCst), 0);
            });
        });
    }
    #[test]
    fn event_loop_completion_cancellation_and_close_retire_all_external_roots() {
        use crate::async_rt::{cancellation, event_loop};
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            with_gil(|py| {
                install_deterministic_scheduler(&py);
                DEFER_FIRST.store(false, AtomicOrdering::SeqCst);
                for kind in [crate::TASK_KIND_GENERATOR, crate::TASK_KIND_FUTURE] {
                    // 0: complete, 1: cancellation during poll, 2: close with
                    // both a queued step and a far-future timer owned by loop.
                    for terminal in 0..3 {
                        POLLS.store(0, AtomicOrdering::SeqCst);
                        CANCEL_AFTER_OBSERVE.store(terminal == 1, AtomicOrdering::SeqCst);
                        let variable = var(&py, b"loop-value");
                        let selected = context::new_context(&py).unwrap();
                        {
                            let _entry = context::EnteredContext::enter(&py, selected).unwrap();
                            let token = context::set_variable(
                                &py,
                                variable,
                                MoltObject::from_int(77).bits(),
                            )
                            .unwrap();
                            dec_ref_bits(&py, token);
                        }
                        let task = crate::molt_task_new(
                            poll as *const () as usize as u64,
                            (crate::GEN_CONTROL_SIZE + 8) as u64,
                            kind,
                        );
                        let ptr = obj_from_bits(task).as_ptr().unwrap();
                        unsafe {
                            crate::object::payload_refs::store_borrowed(
                                &py,
                                ptr,
                                crate::GEN_CONTROL_SIZE,
                                variable,
                            );
                        }
                        let execution = unsafe {
                            cancellation::molt_cancel_token_new(MoltObject::from_int(-1).bits())
                        };
                        let id = obj_from_bits(execution).as_int().unwrap() as u64;
                        register_task_execution(&py, ptr, id, TaskContextBinding::Owned(selected));
                        unsafe {
                            cancellation::molt_cancel_token_drop(execution);
                        }
                        let task_watch = WeakBorrow::new(&py, task).unwrap();
                        let context_watch = WeakBorrow::new(&py, selected).unwrap();
                        let loop_handle = event_loop::molt_event_loop_new();
                        let spawn_count = spawned_task_count();
                        event_loop::molt_event_loop_spawn(loop_handle, task);
                        assert!(!exception_pending(&py));
                        if terminal == 2 {
                            register_task_sleep(
                                &py,
                                ptr,
                                Instant::now() + std::time::Duration::from_secs(3600),
                            );
                        }
                        dec_ref_bits(&py, task);
                        dec_ref_bits(&py, selected);
                        unsafe {
                            crate::object::gc::collect_cycles(&py);
                        }
                        let live = task_watch
                            .upgrade_owned()
                            .expect("loop queue owns live work");
                        dec_ref_bits(&py, live);
                        if terminal == 2 {
                            event_loop::molt_event_loop_close(loop_handle);
                        } else {
                            event_loop::molt_event_loop_run_once(loop_handle);
                            if terminal == 1 {
                                assert!(exception_pending(&py));
                                unsafe {
                                    molt_cpython_abi::api::errors::PyErr_Clear();
                                }
                            } else {
                                assert!(!exception_pending(&py));
                            }
                        }
                        assert_eq!(
                            POLLS.load(AtomicOrdering::SeqCst),
                            usize::from(terminal != 2)
                        );
                        assert_eq!(spawned_task_count(), spawn_count);
                        unsafe {
                            crate::object::gc::collect_cycles(&py);
                        }
                        assert!(task_watch.upgrade_owned().is_none());
                        assert!(context_watch.upgrade_owned().is_none());
                        event_loop::molt_event_loop_drop(loop_handle);
                        dec_ref_bits(&py, variable);
                    }
                }
                CANCEL_AFTER_OBSERVE.store(false, AtomicOrdering::SeqCst);
            });
        });
    }
}
