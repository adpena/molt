use crate::PyToken;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use crossbeam_deque::{Injector, Worker};

use crate::object::ops::string_obj_to_owned;
use crate::state::clear_worker_thread_state;
use crate::{
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, GIL_DEPTH, GilGuard, GilReleaseGuard,
    HEADER_FLAG_BLOCK_ON, HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAG_TASK_DONE, HEADER_FLAG_TASK_QUEUED,
    HEADER_FLAG_TASK_RUNNING, HEADER_FLAG_TASK_WAKE_PENDING, MoltHeader, MoltObject, PtrSlot,
    anext_default_poll_fn_addr, async_sleep_poll_fn_addr, asyncgen_poll_fn_addr, call_poll_fn,
    class_name_for_error, code_filename_bits, code_name_bits, context_stack_unwind, dec_ref_bits,
    exception_context_align_depth, exception_context_fallback_pop, exception_context_fallback_push,
    exception_handler_active, exception_kind_bits, exception_pending, exception_stack_baseline_get,
    exception_stack_baseline_set, exception_stack_depth, exception_stack_set_depth,
    format_exception_with_traceback, generator_raise_active, handle_system_exit,
    header_from_obj_ptr, inc_ref_bits, io_wait_poll_fn_addr, maybe_ptr_from_bits,
    molt_exception_last, obj_from_bits, object_class_bits, object_type_id, pending_bits_i64,
    process_poll_fn_addr, promise_poll_fn_addr, ptr_from_bits, raise_exception, record_exception,
    resolve_task_ptr, runtime_state, set_task_raise_active, task_exception_baseline_store,
    task_exception_baseline_take, task_exception_depth_store, task_exception_depth_take,
    task_exception_handler_stack_store, task_exception_handler_stack_take,
    task_exception_stack_store, task_exception_stack_take, task_last_exception_contains_valid,
    task_raise_active, thread_poll_fn_addr, with_gil,
};

use super::cancellation::{
    cancel_tokens, clear_task_token, current_token_id, ensure_task_token,
    raise_cancelled_with_message, set_current_token, task_cancel_pending, task_take_cancel_pending,
};
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
pub(crate) use task_state::{
    AwaitWaiterIndex, asyncgen_registry, await_waiter_clear, await_waiter_register, await_waiters,
    fn_ptr_code_get, fn_ptr_code_set, process_task_drop, process_task_state, task_exception_depths,
    task_exception_handler_stacks, task_exception_stacks, task_last_exceptions, task_waiting_on,
    task_waiting_on_blocked, task_waiting_on_event, task_waiting_on_future, wake_await_waiters,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use task_state::{thread_task_drop, thread_task_state};

mod asyncio_runtime;
pub(crate) use asyncio_runtime::{
    AsyncioEventWaiterIndex, molt_asyncio_child_watcher_add, molt_asyncio_child_watcher_clear,
    molt_asyncio_child_watcher_pop, molt_asyncio_child_watcher_remove, molt_asyncio_enter_task,
    molt_asyncio_event_loop_get, molt_asyncio_event_loop_get_current,
    molt_asyncio_event_loop_policy_get, molt_asyncio_event_loop_policy_set,
    molt_asyncio_event_loop_set, molt_asyncio_event_waiters_cleanup_token,
    molt_asyncio_event_waiters_register, molt_asyncio_event_waiters_unregister,
    molt_asyncio_leave_task, molt_asyncio_register_task,
    molt_asyncio_require_child_watcher_support, molt_asyncio_require_ssl_transport_support,
    molt_asyncio_require_unix_socket_support, molt_asyncio_running_loop_get,
    molt_asyncio_running_loop_set, molt_asyncio_ssl_transport_orchestrate,
    molt_asyncio_task_last_exception_clear, molt_asyncio_task_registry_contains,
    molt_asyncio_task_registry_current, molt_asyncio_task_registry_current_for_loop,
    molt_asyncio_task_registry_get, molt_asyncio_task_registry_live,
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

pub struct MoltTask {
    pub future_ptr: *mut u8,
}

unsafe impl Send for MoltTask {}

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
    entries: HashMap<PtrSlot, u64>,
    by_epoch: BTreeMap<u64, VecDeque<PtrSlot>>,
}

impl DeferredQueue {
    fn insert(&mut self, task_ptr: PtrSlot, target: u64) -> bool {
        if self.entries.contains_key(&task_ptr) {
            return false;
        }
        self.entries.insert(task_ptr, target);
        self.by_epoch.entry(target).or_default().push_back(task_ptr);
        true
    }

    fn remove(&mut self, task_ptr: PtrSlot) {
        self.entries.remove(&task_ptr);
    }

    fn contains(&self, task_ptr: PtrSlot) -> bool {
        self.entries.contains_key(&task_ptr)
    }

    fn flush(&mut self, current: u64, injector: &Injector<MoltTask>) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let mut enqueued = false;
        let mut ready_epochs = Vec::new();
        for (&epoch, queue) in self.by_epoch.iter_mut() {
            if epoch > current {
                break;
            }
            while let Some(task_ptr) = queue.pop_front() {
                if self.entries.remove(&task_ptr).is_some() {
                    injector.push(MoltTask {
                        future_ptr: task_ptr.0,
                    });
                    enqueued = true;
                }
            }
            ready_epochs.push(epoch);
        }
        for epoch in ready_epochs {
            self.by_epoch.remove(&epoch);
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
                    if async_trace_enabled() {
                        eprintln!("molt async trace: worker_start idx={}", i);
                    }
                    loop {
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

    pub fn enqueue(&self, task: MoltTask) {
        if !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: enqueue task=0x{:x}",
                task.future_ptr as usize
            );
        }
        self.injector.push(task);
    }

    fn advance_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, AtomicOrdering::SeqCst) + 1
    }

    pub(crate) fn defer_task_ptr(&self, task_ptr: *mut u8) {
        if task_ptr.is_null() || !self.running.load(AtomicOrdering::Relaxed) {
            return;
        }
        let target = self.epoch.load(AtomicOrdering::Relaxed).saturating_add(1);
        let mut guard = self.deferred.lock().unwrap();
        guard.insert(PtrSlot(task_ptr), target);
    }

    pub(crate) fn clear_deferred(&self, task_ptr: *mut u8) {
        if task_ptr.is_null() {
            return;
        }
        let mut guard = self.deferred.lock().unwrap();
        guard.remove(PtrSlot(task_ptr));
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

    fn execute_task(task: MoltTask, _injector: &Injector<MoltTask>) {
        #[cfg(target_arch = "wasm32")]
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = crate::object::object_poll_fn(task_ptr);
                {
                    let _guard = task_queue_lock().lock().unwrap();
                    if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                        (*header).flags &= !HEADER_FLAG_TASK_QUEUED;
                        (*header).flags &= !HEADER_FLAG_TASK_RUNNING;
                        (*header).flags &= !HEADER_FLAG_TASK_WAKE_PENDING;
                        if async_trace_enabled() {
                            eprintln!(
                                "molt async trace: poll_skip_done task=0x{:x}",
                                task_ptr as usize
                            );
                        }
                        return;
                    }
                }
                if poll_fn_addr != 0 {
                    if async_trace_enabled() {
                        eprintln!(
                            "molt async trace: poll_enter task=0x{:x} poll=0x{:x}",
                            task_ptr as usize, poll_fn_addr
                        );
                    }
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    let _py = &_py;
                    let task_scope = CurrentTaskScope::enter(_py, task_ptr);
                    let prev_task = task_scope.previous();
                    {
                        let _guard = task_queue_lock().lock().unwrap();
                        unsafe {
                            let header = header_from_obj_ptr(task_ptr);
                            (*header).flags &= !HEADER_FLAG_TASK_QUEUED;
                            (*header).flags |= HEADER_FLAG_TASK_RUNNING;
                        }
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
                    loop {
                        let mut res = call_poll_fn(_py, poll_fn_addr, task_ptr);
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
                        exception_context_fallback_pop(_py);
                        clear_task_token(_py, task_ptr);
                        task_mark_done(_py, task_ptr);
                        runtime_state(_py).sleep_queue().cancel_task(_py, task_ptr);
                        let _ = wake_await_waiters(_py, task_ptr);
                        set_task_raise_active(prev_raise);
                        break;
                    }
                    set_current_token(_py, prev_token);
                    if debug_current_task() && prev_task.is_null() {
                        let current = CURRENT_TASK.with(|cell| cell.get());
                        if !current.is_null() {
                            eprintln!(
                                "molt task trace: scheduler restore null (poll) current=0x{:x} task=0x{:x}",
                                current as usize, task_ptr as usize
                            );
                        }
                    }
                    drop(task_scope);
                }
                if poll_fn_addr == 0 && async_trace_enabled() {
                    eprintln!(
                        "molt async trace: poll_skip task=0x{:x} poll=0x0",
                        task_ptr as usize
                    );
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = crate::object::object_poll_fn(task_ptr);
                {
                    let _guard = task_queue_lock().lock().unwrap();
                    if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                        (*header).flags &= !HEADER_FLAG_TASK_QUEUED;
                        (*header).flags &= !HEADER_FLAG_TASK_RUNNING;
                        (*header).flags &= !HEADER_FLAG_TASK_WAKE_PENDING;
                        if async_trace_enabled() {
                            eprintln!(
                                "molt async trace: poll_skip_done task=0x{:x}",
                                task_ptr as usize
                            );
                        }
                        return;
                    }
                }
                if poll_fn_addr != 0 {
                    if async_trace_enabled() {
                        eprintln!(
                            "molt async trace: poll_enter task=0x{:x} poll=0x{:x}",
                            task_ptr as usize, poll_fn_addr
                        );
                    }
                    let _gil = GilGuard::new();
                    let _py = _gil.token();
                    let _py = &_py;
                    let task_scope = CurrentTaskScope::enter(_py, task_ptr);
                    let prev_task = task_scope.previous();
                    {
                        let _guard = task_queue_lock().lock().unwrap();
                        let header = header_from_obj_ptr(task_ptr);
                        (*header).flags &= !HEADER_FLAG_TASK_QUEUED;
                        (*header).flags |= HEADER_FLAG_TASK_RUNNING;
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
                    let mut res = call_poll_fn(_py, poll_fn_addr, task_ptr);
                    if task_cancel_pending(task_ptr) {
                        task_take_cancel_pending(task_ptr);
                        res = raise_cancelled_with_message::<i64>(_py, task_ptr);
                    }
                    let pending = res == pending_bits_i64();
                    record_async_poll(_py, task_ptr, pending, "scheduler");
                    {
                        let _guard = task_queue_lock().lock().unwrap();
                        let header = header_from_obj_ptr(task_ptr);
                        (*header).flags &= !HEADER_FLAG_TASK_RUNNING;
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
                        let scheduled =
                            runtime_state(_py).sleep_queue().is_scheduled(_py, task_ptr);
                        let deferred = runtime_state(_py).scheduler().is_deferred(task_ptr);
                        let waiting_on_blocked = task_waiting_on_blocked(_py, task_ptr);
                        if async_trace_enabled() {
                            eprintln!(
                                "molt async trace: poll_pending task=0x{:x} waiting_on_event={} scheduled={} deferred={} waiting_on_blocked={}",
                                task_ptr as usize,
                                waiting_on_event,
                                scheduled,
                                deferred,
                                waiting_on_blocked
                            );
                        }
                        if wake_pending
                            || (!waiting_on_event && !scheduled && !deferred && !waiting_on_blocked)
                        {
                            enqueue_task_ptr(_py, task_ptr);
                        }
                    } else {
                        clear_task_token(_py, task_ptr);
                        task_mark_done(_py, task_ptr);
                        runtime_state(_py).sleep_queue().cancel_task(_py, task_ptr);
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
                }
                if poll_fn_addr == 0 {
                    task_clear_queue_flags(task_ptr);
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
        let pending = ((*header).flags & HEADER_FLAG_TASK_WAKE_PENDING) != 0;
        if pending {
            (*header).flags &= !HEADER_FLAG_TASK_WAKE_PENDING;
        }
        pending
    }
}

fn task_clear_queue_flags(task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    let _guard = task_queue_lock().lock().unwrap();
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).flags &= !HEADER_FLAG_TASK_QUEUED;
        (*header).flags &= !HEADER_FLAG_TASK_RUNNING;
        (*header).flags &= !HEADER_FLAG_TASK_WAKE_PENDING;
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
    let _guard = task_queue_lock().lock().unwrap();
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).flags |= HEADER_FLAG_TASK_DONE;
    }
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

fn enqueue_task_ptr(_py: &PyToken<'_>, task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    let mut should_enqueue = false;
    let mut should_return = false;
    {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                should_return = true;
            }
            if ((*header).flags & HEADER_FLAG_BLOCK_ON) != 0 {
                should_return = true;
            }
            if !should_return && ((*header).flags & HEADER_FLAG_TASK_RUNNING) != 0 {
                (*header).flags |= HEADER_FLAG_TASK_WAKE_PENDING;
                should_return = true;
            }
            if !should_return && ((*header).flags & HEADER_FLAG_TASK_QUEUED) != 0 {
                should_return = true;
            }
            if !should_return {
                (*header).flags |= HEADER_FLAG_TASK_QUEUED;
                should_enqueue = true;
            }
        }
    }
    if should_return {
        return;
    }
    if should_enqueue {
        runtime_state(_py).scheduler().enqueue(MoltTask {
            future_ptr: task_ptr,
        });
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
            if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                return;
            }
            if async_trace_enabled() {
                eprintln!(
                    "molt async trace: wake_task_self task=0x{:x}",
                    task_ptr as usize
                );
            }
            (*header).flags |= HEADER_FLAG_TASK_WAKE_PENDING;
        }
        return;
    }
    let sleep_queue = runtime_state(_py).sleep_queue();
    sleep_queue.cancel_task(_py, task_ptr);
    let mut should_enqueue = false;
    let mut should_return = false;
    let inline_only = {
        let _guard = task_queue_lock().lock().unwrap();
        unsafe {
            let header = header_from_obj_ptr(task_ptr);
            let done = ((*header).flags & HEADER_FLAG_TASK_DONE) != 0;
            let block_on = ((*header).flags & HEADER_FLAG_BLOCK_ON) != 0;
            let running = ((*header).flags & HEADER_FLAG_TASK_RUNNING) != 0;
            let queued = ((*header).flags & HEADER_FLAG_TASK_QUEUED) != 0;
            let spawned = ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
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
                (*header).flags |= HEADER_FLAG_TASK_WAKE_PENDING;
                should_return = true;
            }
            if !should_return && running {
                (*header).flags |= HEADER_FLAG_TASK_WAKE_PENDING;
                should_return = true;
            }
            if !should_return && queued {
                should_return = true;
            }
            if !should_return && !inline_only {
                (*header).flags |= HEADER_FLAG_TASK_QUEUED;
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
        runtime_state(_py).scheduler().enqueue(MoltTask {
            future_ptr: task_ptr,
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_block_on_task(task_ptr: *mut u8) -> bool {
    BLOCK_ON_TASK.with(|cell| cell.get() == task_ptr)
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(task_ptr) = resolve_task_ptr(task_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            };
            if async_trace_enabled() {
                let poll_fn = crate::object::object_poll_fn(task_ptr);
                eprintln!(
                    "molt async trace: spawn task=0x{:x} poll=0x{:x}",
                    task_ptr as usize, poll_fn
                );
            }
            cancel_tokens(_py);
            // Respect the task's pre-registered cancellation/context token when present.
            let _ = ensure_task_token(_py, task_ptr, current_token_id());
            let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
            if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) == 0 {
                (*header).flags |= HEADER_FLAG_SPAWN_RETAIN;
                inc_ref_bits(_py, MoltObject::from_ptr(task_ptr).bits());
                spawned_task_inc();
            }
            enqueue_task_ptr(_py, task_ptr);
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
            (*header).flags |= HEADER_FLAG_BLOCK_ON;
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
                let mut res = call_poll_fn(_py, poll_fn_addr, task_ptr);
                if res != pending_bits_i64() && !exception_pending(_py) {
                    crate::task_last_exception_drop(_py, task_ptr);
                }
                if matches!(
                    std::env::var("MOLT_TRACE_BLOCK_ON_RESULT").ok().as_deref(),
                    Some("1")
                ) {
                    let pending_kind = if exception_pending(_py) {
                        let exc_bits = molt_exception_last();
                        if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                            let kind_bits = exception_kind_bits(exc_ptr);
                            string_obj_to_owned(obj_from_bits(kind_bits))
                                .unwrap_or_else(|| "<exc>".to_string())
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
                                let code_bits = fn_ptr_code_get(_py, poll_fn);
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
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let due = runtime_state(&_py).sleep_queue().take_due_scheduler_tasks();
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
                    let exc_bits = molt_exception_last();
                    if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                        let kind_bits = exception_kind_bits(exc_ptr);
                        string_obj_to_owned(obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string())
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
            (*header).flags &= !HEADER_FLAG_BLOCK_ON;
            trace_step("clear_block_on_flag");
            task_mark_done(_py, task_ptr);
            trace_step("task_mark_done");
            clear_task_token(_py, task_ptr);
            trace_step("clear_task_token");
            runtime_state(_py).sleep_queue().cancel_task(_py, task_ptr);
            trace_step("cancel_task_sleep");
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
                    let kind_bits = exception_kind_bits(exc_ptr);
                    if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref()
                        == Some("SystemExit")
                    {
                        handle_system_exit(_py, exc_ptr);
                    }
                    context_stack_unwind(_py, exc_bits);
                    eprintln!("{}", format_exception_with_traceback(_py, exc_ptr));
                    std::process::exit(1);
                }
                if !obj_from_bits(exc_bits).is_none() {
                    dec_ref_bits(_py, exc_bits);
                }
            }
        }
        result
    }
}
