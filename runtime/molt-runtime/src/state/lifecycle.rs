use crate::PyToken;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::async_rt::sockets::socket_runtime_state_clear;
use crate::builtins::attr::clear_attr_tls_caches;
use crate::builtins::attributes::attributes_clear_runtime_state;
use crate::builtins::codecs_ext::codecs_clear_error_handlers;
use crate::builtins::concurrent::concurrent_clear_runtime_state;
use crate::builtins::contextvars::contextvars_clear_state;
use crate::builtins::copy_mod::copy_memo_clear_state;
use crate::builtins::exceptions::{
    canonical_exception_class_roots, drain_dynamic_exception_type_cache,
    exceptions_release_runtime_class_anchor, take_thread_exception_for_teardown,
};
use crate::builtins::functions::python_builtin_functions_clear_runtime_state;
use crate::builtins::functools::functools_clear_runtime_state;
use crate::builtins::io::io_clear_runtime_state;
use crate::builtins::modules::modules_clear_runtime_state;
use crate::builtins::operator::operator_clear_runtime_state;
use crate::builtins::platform::platform_clear_runtime_state;
use crate::builtins::signal_ext::signal_clear_state;
use crate::builtins::sys_ext::sys_ext_clear_state;
use crate::builtins::types::{
    types_clear_runtime_callbacks, types_clear_runtime_state, types_runtime_class_roots,
};
use crate::c_api::c_api_module_clear_state;
use crate::call::bind::{clear_call_bind_ic_cache, clear_method_ic_cache, clear_super_ic_cache};
use crate::const_data_cache::clear_const_data_literal_caches;
use crate::object::builders::clear_builder_singletons;
use crate::object::class_storage::RuntimeClassRetirement;
use crate::object::dec_ref_ptr;
use crate::object::utf8_cache::{
    UTF8_CACHE_MAX_ENTRIES, UTF8_COUNT_CACHE_SHARDS, Utf8CacheStore, Utf8CountCacheStore,
    clear_utf8_count_tls,
};
use crate::{
    ACTIVE_EXCEPTION_FALLBACK, ACTIVE_EXCEPTION_STACK, BLOCK_ON_TASK, CONTEXT_STACK,
    CURRENT_EXCEPTION_PENDING, CURRENT_TASK, CURRENT_TOKEN, DEFAULT_RECURSION_LIMIT,
    EXCEPTION_STACK, FRAME_STACK, GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, GilGuard,
    GilReleaseGuard, MoltObject, PARSE_ARENA, RECURSION_DEPTH, RECURSION_LIMIT, TASK_RAISE_ACTIVE,
    TRACE_FRAME_PUSH_STACK, TYPE_ID_DICT, TYPE_ID_FILE_HANDLE, TYPE_ID_MODULE, alloc_string,
    builtin_classes_retire_identities, builtin_classes_shutdown, call_callable0, clear_exception,
    clear_exception_type_cache, clear_thread_exception_for_teardown, dec_ref_bits,
    default_cancel_tokens, dict_clear_in_place_shutdown, dict_get_in_place, exception_pending,
    exceptions_clear_runtime_state, inc_ref_bits, intern_static_name, module_dict_bits,
    molt_file_flush, molt_get_attr_name, obj_from_bits, object_type_id, reset_ptr_registry,
    runtime_state,
};
use std::sync::OnceLock;
use std::sync::atomic::Ordering as AtomicOrdering;

#[cfg(test)]
pub(crate) static THREAD_LOCAL_DROP_TEST_TRACE: std::sync::Mutex<
    Option<(std::thread::ThreadId, std::sync::mpsc::Sender<&'static str>)>,
> = std::sync::Mutex::new(None);

#[cfg(test)]
fn trace_thread_local_drop(stage: &'static str) {
    let trace = { THREAD_LOCAL_DROP_TEST_TRACE.lock().unwrap().clone() };
    if let Some((owner, sender)) = trace
        && owner == std::thread::current().id()
    {
        let _ = sender.send(stage);
    }
}

use super::{
    RuntimeState, cache::clear_atomic_slots, cache::clear_method_cache,
    cache::clear_runtime_static_names, runtime_extension_states_clear_and_drop,
};

thread_local! {
    static TLS_GUARD: ThreadLocalGuard = ThreadLocalGuard::new();
}

struct ThreadLocalGuard;

impl ThreadLocalGuard {
    fn new() -> Self {
        Self
    }
}

impl Drop for ThreadLocalGuard {
    fn drop(&mut self) {
        #[cfg(test)]
        trace_thread_local_drop("enter");
        // After `molt_runtime_shutdown` the RuntimeState has been freed and
        // both the ready-state publication and `TLS_RUNTIME_STATE` are null.
        // Attempting GIL acquisition + cleanup here would either:
        //   (a) dereference a dangling TLS pointer (use-after-free), or
        //   (b) trigger `molt_runtime_init` to re-allocate a new RuntimeState
        //       just to tear it down again.
        // Both are incorrect.  The shutdown path already called
        // `clear_thread_local_state`, so there is nothing left to clean up.
        //
        // HOWEVER: we must still release heap-backed TLS caches NOW, while the
        // global allocator (mimalloc) is still alive. If we leave them for
        // Rust's TLS destructor phase, deallocation can race with mimalloc's
        // own thread-local cleanup (registered via pthread_key_create).
        let gil = GilGuard::new();
        #[cfg(test)]
        trace_thread_local_drop("gil_acquired");
        if crate::state::runtime_state::runtime_state_for_gil().is_none() {
            #[cfg(test)]
            trace_thread_local_drop("runtime_absent");
            drop(gil);
            #[cfg(test)]
            trace_thread_local_drop("gil_released");
            drain_heap_tls();
            #[cfg(test)]
            trace_thread_local_drop("drained");
            return;
        }
        clear_thread_local_state_without_ref_owning_ic(&gil.token());
        #[cfg(test)]
        trace_thread_local_drop("cleared");
    }
}

pub(crate) fn touch_tls_guard() {
    molt_cpython_abi::api::object::prepare_runtime_thread_state_lifetime();
    crate::state::runtime_state::touch_runtime_execution_lease_tls_lifetime();
    crate::concurrency::gil::touch_gil_tls_lifetime();
    crate::object::heap_lifecycle::touch_terminal_sink_tls_lifetime();
    let _ = PARSE_ARENA.try_with(|_| {});
    let _ = crate::REPR_SET.try_with(|_| {});
    let _ = TLS_GUARD.try_with(|_| {});
    // This sentinel is deliberately initialized last. Rust destroys TLS in
    // reverse initialization order, so it drains the CPython thread-state
    // record while the GIL, heap-lifecycle sink pools, and runtime guard TLS
    // above are all still reachable.
    molt_cpython_abi::api::object::arm_runtime_thread_state_lifetime();
}

pub(crate) fn runtime_teardown(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, RuntimeTeardownMode::Embedding);
}

pub(crate) fn runtime_teardown_isolate(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, RuntimeTeardownMode::Isolate);
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RuntimeTeardownMode {
    Embedding,
    ProcessExit,
    Isolate,
}

fn finish_pending_calls_for_teardown(_py: &PyToken<'_>) {
    crate::builtins::exceptions::run_unraisable(
        _py,
        MoltObject::none().bits(),
        Some("Exception ignored while finishing pending calls at shutdown"),
        molt_cpython_abi::api::pending_calls::finish_pending_calls_before_teardown,
    );
}

pub(crate) fn runtime_teardown_for_process_exit(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, RuntimeTeardownMode::ProcessExit);
}

fn shutdown_started_runtime_workers(_py: &PyToken<'_>, state: &RuntimeState) {
    let scheduler_started = state.scheduler_started.load(AtomicOrdering::Acquire);
    let sleep_queue_started = state.sleep_queue_started.load(AtomicOrdering::Acquire);
    let io_poller_started = state.io_poller_started.load(AtomicOrdering::Acquire);
    #[cfg(not(target_arch = "wasm32"))]
    let thread_pool_started = state.thread_pool_started.load(AtomicOrdering::Acquire);
    #[cfg(target_arch = "wasm32")]
    let thread_pool_started = false;

    if scheduler_started || sleep_queue_started || io_poller_started || thread_pool_started {
        trace_shutdown("workers_shutdown_start");
        let _release = GilReleaseGuard::suspend();
        if scheduler_started {
            trace_shutdown("scheduler_shutdown_start");
            state.scheduler().shutdown();
            trace_shutdown("scheduler_shutdown_done");
        }
        if sleep_queue_started {
            trace_shutdown("sleep_queue_shutdown_start");
            state.sleep_queue().shutdown(_py);
            trace_shutdown("sleep_queue_shutdown_done");
        }
        if io_poller_started {
            trace_shutdown("io_poller_shutdown_start");
            state.io_poller().shutdown();
            trace_shutdown("io_poller_shutdown_done");
        }
        #[cfg(not(target_arch = "wasm32"))]
        if thread_pool_started && let Some(pool) = state.thread_pool.get() {
            trace_shutdown("thread_pool_shutdown_start");
            pool.shutdown();
            trace_shutdown("thread_pool_shutdown_done");
        }
        trace_shutdown("workers_shutdown_done");
    }
}

fn runtime_teardown_inner(_py: &PyToken<'_>, state: &RuntimeState, mode: RuntimeTeardownMode) {
    crate::gil_assert();
    trace_shutdown("start");
    // Pending-call admission and its ring are process-static. An isolate owns
    // its native thread state, not the primary runtime's pending callbacks.
    if crate::state::runtime_state::owns_process_cpython_state(state) {
        trace_shutdown("finish_pending_calls");
        finish_pending_calls_for_teardown(_py);
    }
    shutdown_started_runtime_workers(_py, state);
    trace_shutdown("drain_process_registry");
    state.process_registry.drain_for_teardown();
    trace_shutdown("clear_concurrent_runtime_state");
    concurrent_clear_runtime_state(_py, state);
    #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
    {
        trace_shutdown("clear_socket_state");
        socket_runtime_state_clear(state);
    }
    trace_shutdown("clear_async_hang_probe");
    clear_async_hang_probe(state);
    trace_shutdown("clear_task_state");
    clear_task_state(_py, state);
    trace_shutdown("clear_thread_exception");
    clear_thread_exception_for_teardown(_py);
    trace_shutdown("run_atexit_callbacks");
    crate::builtins::atexit::atexit_run_exitfuncs_teardown(_py);
    trace_shutdown("flush_stdio");
    flush_stdio_handles(_py, state);
    trace_shutdown("clear_utf8_caches");
    clear_utf8_caches(state);
    trace_shutdown("clear_asyncgen_registry");
    clear_asyncgen_registry(state);
    trace_shutdown("drain_runtime_class_callbacks");
    // Keep destruction custody through the callback-free tail as well: ordinary
    // bridge-view destruction checks this capability, and must never establish
    // a fresh public execution/thread-state boundary after quiescence.
    let _custody =
        (!crate::concurrency::execution::current_thread_has_c_extension_execution_context())
            .then(crate::concurrency::execution::ShutdownDrainExecutionCustody::enter);
    // C roots can run extension deallocators. Close their readiness while all
    // runtime lookup/class authority is still present, then use the same root
    // owner drain before and throughout canonical class retirement.
    state.cpython.retire_static_roots();
    clear_runtime_callback_roots(_py, state);
    let mut retirement = RuntimeClassRetirement::new();
    let mut drain = || {
        let mut changed = retirement.include(_py, runtime_class_roots(_py, state));
        changed |= clear_runtime_callback_roots(_py, state);
        changed |= retirement.clear_contents(_py);
        changed |= clear_thread_local_state(_py);
        changed |= clear_interned_names(_py, state);
        changed |= clear_runtime_static_names(_py, state);
        changed
    };
    // Every runtime owner must close both current-thread domains: either
    // domain's callbacks can repopulate the other. This includes native
    // isolates and WASM instances; no absent C record is fabricated by draining.
    trace_shutdown("drain_shutdown_owner_thread_state");
    molt_cpython_abi::api::object::detach_runtime_execution_thread();
    molt_cpython_abi::api::object::clear_current_thread_state_for_runtime_shutdown(&mut drain);
    if mode == RuntimeTeardownMode::Embedding {
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0,
            "retained PyThreadState survived the last live-runtime callback drain"
        );
    }
    trace_shutdown("retire_runtime_class_identities");
    builtin_classes_retire_identities(_py, state, &retirement);
    trace_shutdown("clear_exception_type_cache");
    clear_exception_type_cache(_py, state);
    trace_shutdown("release_types_runtime_class_anchors");
    types_clear_runtime_state(_py, state);
    exceptions_release_runtime_class_anchor(_py, state);
    trace_shutdown("retire_cpython_static_bindings");
    state.cpython.retire_static_bindings(_py);
    // Keep builtin classes alive until after cache + TLS teardown: releasing
    // them too early can trigger lock re-entry when later dec_ref paths perform
    // class attribute lookups during shutdown.
    trace_shutdown("builtin_classes_shutdown");
    builtin_classes_shutdown(_py, state);
    retirement.release_pins(_py);
    // Sealed class slot declarations and field maps still own canonical tuple
    // and name payloads after cycle breaking. Retire their final class anchors
    // before turning immortal pool entries into mortal shutdown allocations.
    trace_shutdown("clear_builder_singletons");
    clear_builder_singletons(_py, state);
    if mode == RuntimeTeardownMode::Embedding {
        trace_shutdown("reset_ptr_registry");
        reset_ptr_registry();
        trace_shutdown("reset_gc_registry");
        crate::object::gc::gc_reset_registry(state);
        trace_shutdown("reset_gc_workspace");
        crate::object::gc::gc_reset_workspace();
        trace_shutdown("reset_gc_control_state");
        state.gc.reset();
        trace_shutdown("reset_detached_sink_pool");
        crate::object::heap_lifecycle::reset_detached_sink_pool();
        trace_shutdown("clear_profile_epoch");
        crate::object::ops::profile_epoch_clear();
    }
    if mode == RuntimeTeardownMode::ProcessExit {
        crate::object::ops::profile_epoch_clear();
    }
    trace_shutdown("clear_resource_state");
    crate::resource::clear_resource_state();
    trace_shutdown("done");
}

/// One ordered owner family, shared by initial root retirement and every
/// callback fixed-point pass. Never short-circuit: a later owner can repopulate
/// an earlier one. Each cleanup detaches its complete cohort before release.
fn clear_runtime_callback_roots(py: &PyToken<'_>, state: &RuntimeState) -> bool {
    let mut changed = concurrent_clear_runtime_state(py, state);
    changed |= clear_task_state(py, state);
    changed |= signal_clear_state(py, state);
    changed |= contextvars_clear_state(py, state);
    changed |= copy_memo_clear_state(py, state);
    changed |= sys_ext_clear_state(py, state);
    changed |= c_api_module_clear_state(py, state);
    changed |= runtime_extension_states_clear_and_drop(state);
    changed |= clear_module_cache(py, state);
    changed |= modules_clear_runtime_state(py, state);
    changed |= crate::object::gc::gc_clear_api_roots(py);
    changed |= platform_clear_runtime_state(py, state);
    changed |= io_clear_runtime_state(py, state);
    changed |= codecs_clear_error_handlers(py, state);
    changed |= exceptions_clear_runtime_state(py, state);
    changed |= drain_dynamic_exception_type_cache(py, state);
    changed |= types_clear_runtime_callbacks(py, state);
    changed |= clear_gen_locals(py, state);
    changed |= clear_dict_subclass_storage(py, state);
    changed |= clear_method_cache(py, state);
    changed |= python_builtin_functions_clear_runtime_state(py, state);
    changed |= attributes_clear_runtime_state(py, state);
    changed |= clear_special_cache(py, state);
    changed |= clear_code_slots(py, state);
    changed |= clear_fn_ptr_code_map(py, state);
    changed |= clear_asyncgen_hooks(py, state);
    changed |= clear_asyncgen_locals(py, state);
    changed |= functools_clear_runtime_state(py, state);
    changed |= operator_clear_runtime_state(py, state);
    changed |= crate::builtins::atexit::atexit_clear_runtime_roots(py);
    changed
}

fn runtime_class_roots(py: &PyToken<'_>, state: &RuntimeState) -> Vec<u64> {
    let mut roots = Vec::new();
    let builtins = state.builtin_classes.load(AtomicOrdering::Acquire);
    if !builtins.is_null() {
        roots.extend(unsafe { &*builtins }.anchors());
    }
    roots.extend(canonical_exception_class_roots(state));
    roots.extend(types_runtime_class_roots(py, state));
    roots.extend(state.cpython.static_class_roots());
    roots
}

fn trace_shutdown_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_SHUTDOWN").ok().as_deref(),
            Some("1")
        )
    })
}

fn trace_shutdown(step: &str) {
    if trace_shutdown_enabled() {
        eprintln!("molt shutdown: {step}");
    }
}

pub(crate) fn runtime_reset_for_init(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    PARSE_ARENA.with(|arena| arena.borrow_mut().reset());
    state
        .importlib_default_meta_path_bootstrapped
        .store(false, AtomicOrdering::Release);
}

fn clear_asyncgen_registry(state: &RuntimeState) {
    let mut guard = state.asyncgen_registry.lock().unwrap();
    guard.clear();
}

fn clear_asyncgen_hooks(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let roots = {
        let mut guard = state.asyncgen_hooks.lock().unwrap();
        [
            std::mem::replace(&mut guard.firstiter, MoltObject::none().bits()),
            std::mem::replace(&mut guard.finalizer, MoltObject::none().bits()),
        ]
    };
    let mut changed = false;
    for bits in roots {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            changed = true;
            dec_ref_bits(_py, bits);
        }
    }
    changed
}

fn clear_asyncgen_locals(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let locals = std::mem::take(&mut *state.asyncgen_locals.lock().unwrap());
    let changed = !locals.is_empty();
    for entry in locals.into_values() {
        for bits in entry.names {
            if bits != 0 {
                dec_ref_bits(_py, bits);
            }
        }
    }
    changed
}

fn clear_gen_locals(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let locals = std::mem::take(&mut *state.gen_locals.lock().unwrap());
    let changed = !locals.is_empty();
    for entry in locals.into_values() {
        for bits in entry.names {
            if bits != 0 {
                dec_ref_bits(_py, bits);
            }
        }
    }
    changed
}

fn clear_dict_subclass_storage(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let drained: Vec<u64> = {
        let mut guard = state.dict_subclass_storage.lock().unwrap();
        guard.drain().map(|(_, bits)| bits).collect()
    };
    let changed = !drained.is_empty();
    for bits in drained {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    changed
}

fn clear_fn_ptr_code_map(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let drained: Vec<u64> = {
        let mut guard = state.fn_ptr_code.lock().unwrap();
        guard.drain().map(|(_key, bits)| bits).collect()
    };
    let changed = !drained.is_empty();
    for bits in drained {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
    changed
}

fn clear_async_hang_probe(state: &RuntimeState) {
    if let Some(Some(probe)) = state.async_hang_probe.get()
        && let Ok(mut guard) = probe.pending_counts.lock()
    {
        guard.clear();
    }
}

fn clear_thread_local_state(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    let mut changed = clear_call_bind_ic_cache(_py);
    changed |= clear_method_ic_cache(_py);
    changed |= clear_super_ic_cache(_py);
    #[cfg(test)]
    trace_thread_local_drop("ic_caches_cleared");
    changed | clear_thread_local_state_without_ref_owning_ic(_py)
}

fn clear_thread_local_state_without_ref_owning_ic(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    let exception = take_thread_exception_for_teardown(_py);
    let mut changed = exception.is_some();
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    let contexts = CONTEXT_STACK
        .try_with(|stack| std::mem::take(&mut *stack.borrow_mut()))
        .unwrap_or_default();
    let frames = FRAME_STACK
        .try_with(|stack| std::mem::take(&mut *stack.borrow_mut()))
        .unwrap_or_default();
    let _ = TRACE_FRAME_PUSH_STACK.try_with(|stack| {
        let _ = std::mem::take(&mut *stack.borrow_mut());
    });
    let active = ACTIVE_EXCEPTION_STACK
        .try_with(|stack| std::mem::take(&mut *stack.borrow_mut()))
        .unwrap_or_default();
    let _ = ACTIVE_EXCEPTION_FALLBACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let _ = std::mem::take(&mut *stack);
    });
    let generators = GENERATOR_EXCEPTION_STACKS
        .try_with(|map| std::mem::take(&mut *map.borrow_mut()))
        .unwrap_or_default();
    let _ = EXCEPTION_STACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let _ = std::mem::take(&mut *stack);
    });
    let _ = RECURSION_DEPTH.try_with(|depth| depth.set(0));
    let _ = RECURSION_LIMIT.try_with(|limit| limit.set(DEFAULT_RECURSION_LIMIT));
    let _ = GENERATOR_RAISE.try_with(|flag| flag.set(false));
    let _ = TASK_RAISE_ACTIVE.try_with(|flag| flag.set(false));
    let _ = BLOCK_ON_TASK.try_with(|cell| cell.set(std::ptr::null_mut()));
    let _ = CURRENT_TASK.try_with(|cell| cell.set(std::ptr::null_mut()));
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    let _ = CURRENT_TOKEN.try_with(|cell| cell.set(1));
    // Every TLS owner and execution marker is detached before Python can run.
    // In particular no RefCell borrow survives a decref that can repopulate the
    // same context/exception stack from a finalizer.
    changed |=
        !contexts.is_empty() || !frames.is_empty() || !active.is_empty() || !generators.is_empty();
    if let Some(bits) = exception {
        dec_ref_bits(_py, bits);
    }
    for bits in contexts.into_iter().chain(active) {
        dec_ref_bits(_py, bits);
    }
    for frame in frames {
        frame.release(_py);
    }
    for stack in generators.into_values() {
        for bits in stack {
            dec_ref_bits(_py, bits);
        }
    }
    let _ = PARSE_ARENA.try_with(|arena| arena.borrow_mut().clear());
    changed |= clear_attr_tls_caches(_py);
    changed |= clear_const_data_literal_caches(_py);
    clear_utf8_count_tls();
    changed
}

fn clear_code_slots(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let Some(slots) = state.code_slots.get() else {
        return false;
    };
    let mut detached = Vec::with_capacity(slots.len());
    for slot in slots {
        let bits = slot.swap(0, AtomicOrdering::AcqRel);
        if bits != 0 {
            detached.push(bits);
        }
    }
    let changed = !detached.is_empty();
    for bits in detached {
        dec_ref_bits(_py, bits);
    }
    changed
}

pub(crate) fn clear_worker_thread_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    clear_thread_local_state(_py);
}

struct RuntimeWorkerCleanup;

impl Drop for RuntimeWorkerCleanup {
    fn drop(&mut self) {
        if crate::state::runtime_state::runtime_state_for_gil().is_none() {
            return;
        }
        let gil = GilGuard::new();
        clear_worker_thread_state(&gil.token());
    }
}

/// Run a worker that may enter Python with deterministic TLS cleanup on both
/// normal return and unwind. The stack guard is independent of TLS destructor
/// order and re-raises the original panic after cleanup.
pub(crate) fn run_runtime_worker<R>(worker: impl FnOnce() -> R) -> R {
    let _cleanup = RuntimeWorkerCleanup;
    worker()
}

fn clear_task_state(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let mut changed = false;

    // Detach the complete task/exception authority before any callback-bearing
    // reference is released. A finalizer can only publish into fresh state and
    // will therefore be observed by the next fixed-point pass.
    let stacks = {
        let mut guard = state.task_exception_stacks.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !stacks.is_empty();
    changed |= {
        let mut guard = state.task_exception_handler_stacks.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };
    changed |= {
        let mut guard = state.task_exception_depths.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };
    changed |= {
        let mut guard = state.task_exception_baselines.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };
    let pointers = {
        let mut guard = state.task_last_exceptions.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .map(|ptr| ptr.0)
            .collect::<Vec<_>>()
    };
    changed |= !pointers.is_empty();
    let result_bits = {
        let mut guard = state.task_results.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !result_bits.is_empty();
    let cancel_bits = {
        let mut guard = state.task_cancel_messages.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !cancel_bits.is_empty();
    changed |= {
        let mut guard = state.task_tokens.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };
    changed |= {
        let mut guard = state.task_tokens_by_id.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };
    changed |= {
        let mut guard = state.cancel_tokens.lock().unwrap();
        let token_state_changed = guard.len() != 1
            || match guard.get(&1) {
                Some(root) => root.parent != 0 || root.cancelled || root.refs != 1,
                None => true,
            };
        if token_state_changed {
            *guard = default_cancel_tokens();
        }
        token_state_changed
    };

    let running_loop_bits = {
        let mut guard = state.asyncio_running_loops.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !running_loop_bits.is_empty();
    let event_loop_bits = {
        let mut guard = state.asyncio_event_loops.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !event_loop_bits.is_empty();
    let event_loop_policy_bits = {
        let mut guard = state.asyncio_event_loop_policy.lock().unwrap();
        std::mem::replace(&mut *guard, MoltObject::none().bits())
    };
    changed |= event_loop_policy_bits != MoltObject::none().bits();
    let task_bits = {
        let mut guard = state.asyncio_tasks.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !task_bits.is_empty();
    let current_task_bits = {
        let mut guard = state.asyncio_current_tasks.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_values()
            .collect::<Vec<_>>()
    };
    changed |= !current_task_bits.is_empty();
    let event_waiter_bits = {
        let mut guard = state.asyncio_event_waiters.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        changed |= !old.is_empty();
        old.into_values().flatten().collect::<Vec<_>>()
    };
    changed |= {
        let mut guard = state.asyncio_event_waiter_index.lock().unwrap();
        !std::mem::take(&mut *guard).is_empty()
    };

    let edges = {
        let mut waiting = state.task_waiting_on.lock().unwrap();
        let edges = std::mem::take(&mut *waiting);
        let mut waiters = state.await_waiters.lock().unwrap();
        let detached_waiters = std::mem::take(&mut *waiters);
        let mut waiter_index = state.await_waiter_index.lock().unwrap();
        let detached_waiter_index = std::mem::take(&mut *waiter_index);
        changed |=
            !edges.is_empty() || !detached_waiters.is_empty() || !detached_waiter_index.is_empty();
        edges
    };

    #[cfg(not(target_arch = "wasm32"))]
    let thread_task_roots = {
        let thread_tasks = {
            let mut guard = state.thread_tasks.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        changed |= !thread_tasks.is_empty();
        thread_tasks
            .into_values()
            .map(|task| {
                task.cancelled.store(true, AtomicOrdering::Release);
                let result = task.result.lock().unwrap().take();
                let exception = task.exception.lock().unwrap().take();
                task.condvar.notify_all();
                (task, result, exception)
            })
            .collect::<Vec<_>>()
    };

    let process_tasks = {
        let mut guard = state.process_tasks.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    changed |= !process_tasks.is_empty();
    for task in process_tasks.values() {
        task.cancel_wait();
    }

    // Retire registrations, not identity sequences: callbacks can publish new
    // tokens/loops/transports while stale handles from this cohort still exist.
    changed |= state.pipe_transport_registry.clear();
    changed |= state.event_loop_registry.clear(_py);

    for stack in stacks {
        for bits in stack {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
    }
    for ptr in pointers {
        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
    }
    for bits in result_bits {
        dec_ref_bits(_py, bits);
    }
    for bits in cancel_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for bits in running_loop_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for bits in event_loop_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    if event_loop_policy_bits != 0 && !obj_from_bits(event_loop_policy_bits).is_none() {
        dec_ref_bits(_py, event_loop_policy_bits);
    }
    for bits in task_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for bits in current_task_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for bits in event_waiter_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    for (waiter, awaited) in edges {
        unsafe {
            dec_ref_ptr(_py, awaited.0);
            dec_ref_ptr(_py, waiter.0);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    for (_, result, exception) in thread_task_roots {
        if let Some(bits) = result {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = exception {
            dec_ref_bits(_py, bits);
        }
    }
    drop(process_tasks);
    changed
}

fn clear_module_cache(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let modules = {
        let mut guard = state.module_cache.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    let changed = !modules.is_empty();
    for bits in &modules {
        let Some(module_ptr) = obj_from_bits(*bits).as_ptr() else {
            continue;
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                continue;
            }
            let dict_bits = module_dict_bits(module_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_clear_in_place_shutdown(_py, dict_ptr);
            }
        }
    }
    for bits in modules {
        dec_ref_bits(_py, bits);
    }
    changed
}

fn flush_stdio_handles(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let sys_bits = {
        let guard = state.module_cache.lock().unwrap();
        guard.get("sys").copied()
    };
    let Some(sys_bits) = sys_bits else {
        return;
    };
    // Hold a ref while we inspect stdout/stderr.
    inc_ref_bits(_py, sys_bits);
    flush_module_attr(_py, sys_bits, "stdout");
    flush_module_attr(_py, sys_bits, "stderr");
    dec_ref_bits(_py, sys_bits);
}

fn flush_module_attr(_py: &PyToken<'_>, module_bits: u64, attr: &str) {
    let module_obj = obj_from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return;
        }
        let dict_bits = module_dict_bits(module_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
        let name_ptr = alloc_string(_py, attr.as_bytes());
        if name_ptr.is_null() {
            return;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let val_bits = dict_get_in_place(_py, dict_ptr, name_bits);
        dec_ref_bits(_py, name_bits);
        let Some(val_bits) = val_bits else {
            return;
        };
        if obj_from_bits(val_bits).is_none() {
            return;
        }
        inc_ref_bits(_py, val_bits);
        flush_stdio_target(_py, val_bits);
        dec_ref_bits(_py, val_bits);
    }
}

fn flush_stdio_target(_py: &PyToken<'_>, target_bits: u64) {
    let target_obj = obj_from_bits(target_bits);
    if let Some(ptr) = target_obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
                let _ = molt_file_flush(target_bits);
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                return;
            }
        }
    }
    let flush_name_bits = intern_static_name(_py, &state_interned(_py).flush_name, b"flush");
    let flush_bits = molt_get_attr_name(target_bits, flush_name_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return;
    }
    let res_bits = unsafe { call_callable0(_py, flush_bits) };
    dec_ref_bits(_py, flush_bits);
    dec_ref_bits(_py, res_bits);
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

fn state_interned(_py: &PyToken<'_>) -> &'static crate::state::cache::InternedNames {
    &runtime_state(_py).interned
}

fn clear_utf8_caches(state: &RuntimeState) {
    if let Ok(mut cache) = state.utf8_index_cache.lock() {
        *cache = Utf8CacheStore::new();
    }
    for shard in state.utf8_count_cache.iter() {
        if let Ok(mut store) = shard.lock() {
            let per_shard = (UTF8_CACHE_MAX_ENTRIES / UTF8_COUNT_CACHE_SHARDS).max(1);
            *store = Utf8CountCacheStore::new(per_shard);
        }
    }
}

/// Drain the allocation-only TLS anchors initialized before `TLS_GUARD`.
///
/// Ref-owning call-cache TLS is also initialized before the guard, but it is
/// cleared only on the runtime-present path because releasing Python objects
/// requires a live runtime and PyToken. Runtime teardown and explicit worker
/// cleanup must empty it before the runtime-absent destructor path is possible.
fn drain_heap_tls() {
    // Replace the parse arena with an empty state whose outer Vec has zero
    // capacity, so the TLS destructor has nothing to deallocate.
    let _ = PARSE_ARENA.try_with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.drain();
    });
    let _ = crate::REPR_SET.try_with(|s| {
        let _ = std::mem::take(&mut *s.borrow_mut());
    });
}

fn clear_interned_names(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots = state.interned.slots();
    clear_atomic_slots(_py, &slots)
}

fn clear_special_cache(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots = vec![
        &state.special_cache.open_default_mode,
        &state.special_cache.awaitable_await,
        &state.special_cache.function_code_descriptor,
        &state.special_cache.function_globals_descriptor,
        &state.special_cache.weakref_callback_descriptor,
    ];
    clear_atomic_slots(_py, &slots)
}

#[cfg(test)]
mod tests {
    use super::{
        THREAD_LOCAL_DROP_TEST_TRACE, clear_interned_names, clear_special_cache,
        clear_worker_thread_state,
    };
    use crate::{MoltObject, alloc_string, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    fn task_root_retirement_never_reuses_live_runtime_handle_identities() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            super::clear_task_state(py, state);
            #[cfg(not(target_arch = "wasm32"))]
            let new_pipe = || {
                // The identity contract needs an owned descriptor, not pipe
                // traffic. Use the native null device so cleanup closes a real
                // disposable fd without borrowing stdin or another test's fd.
                let device = if cfg!(windows) { c"NUL" } else { c"/dev/null" };
                let fd = unsafe { libc::open(device.as_ptr(), libc::O_RDONLY) };
                assert!(
                    fd >= 0,
                    "open test descriptor: {}",
                    std::io::Error::last_os_error()
                );
                crate::async_rt::event_loop::molt_pipe_transport_new(
                    MoltObject::from_int(i64::from(fd)).bits(),
                    MoltObject::from_bool(true).bits(),
                )
            };
            let token = unsafe {
                crate::async_rt::cancellation::molt_cancel_token_new(MoltObject::none().bits())
            };
            let event_loop = crate::async_rt::event_loop::molt_event_loop_new();
            #[cfg(not(target_arch = "wasm32"))]
            let pipe = new_pipe();
            assert!(!crate::exception_pending(py));
            assert!(super::clear_task_state(py, state));
            assert!(!super::clear_task_state(py, state));
            let next_token = unsafe {
                crate::async_rt::cancellation::molt_cancel_token_new(MoltObject::none().bits())
            };
            let next_loop = crate::async_rt::event_loop::molt_event_loop_new();
            assert!(
                crate::obj_from_bits(next_token).as_int().unwrap()
                    > crate::obj_from_bits(token).as_int().unwrap()
            );
            assert!(
                crate::obj_from_bits(next_loop).as_int().unwrap()
                    > crate::obj_from_bits(event_loop).as_int().unwrap()
            );
            #[cfg(not(target_arch = "wasm32"))]
            {
                let next_pipe = new_pipe();
                assert!(
                    crate::obj_from_bits(next_pipe).as_int().unwrap()
                        > crate::obj_from_bits(pipe).as_int().unwrap()
                );
            }
            assert!(!crate::exception_pending(py));
            assert!(super::clear_task_state(py, state));
            assert!(!super::clear_task_state(py, state));
        });
    }

    static REENTRANT_CONTEXT_CLASS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static REENTRANT_CONTEXT_RELEASES: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    fn publish_context_default(py: &crate::PyToken<'_>, class: u64) {
        let class_ptr = crate::obj_from_bits(class)
            .as_ptr()
            .expect("context default class");
        let value = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        assert!(crate::obj_from_bits(value).as_ptr().is_some());
        let name_ptr = crate::alloc_string(py, b"shutdown-reentrant-default");
        assert!(!name_ptr.is_null());
        let name = MoltObject::from_ptr(name_ptr).bits();
        let handle = crate::builtins::contextvars::molt_contextvars_new_var(name, value);
        assert!(crate::obj_from_bits(handle).as_int().is_some());
        crate::dec_ref_bits(py, name);
        crate::dec_ref_bits(py, value);
    }

    extern "C" fn repopulate_context_default(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if REENTRANT_CONTEXT_RELEASES.fetch_add(1, Ordering::SeqCst) == 0 {
                publish_context_default(py, REENTRANT_CONTEXT_CLASS.load(Ordering::SeqCst));
            }
            MoltObject::none().bits()
        })
    }

    #[test]
    fn callback_root_drain_revisits_context_defaults_repopulated_by_real_finalizers() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let name =
                    crate::attr_name_bits_from_bytes(py, b"ReentrantContextDefault").unwrap();
                let class = crate::molt_class_new(name);
                crate::dec_ref_bits(py, name);
                crate::molt_class_set_base(class, crate::builtin_classes(py).object);
                let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
                unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
                let method = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "repopulate_context_default",
                        repopulate_context_default as *const (),
                    ),
                    1,
                );
                assert!(!method.is_null());
                let key = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
                let method = MoltObject::from_ptr(method).bits();
                crate::molt_set_attr_name(class, key, method);
                crate::dec_ref_bits(py, key);
                crate::dec_ref_bits(py, method);
                assert!(!crate::exception_pending(py));
                REENTRANT_CONTEXT_CLASS.store(class, Ordering::SeqCst);
                REENTRANT_CONTEXT_RELEASES.store(0, Ordering::SeqCst);
                publish_context_default(py, class);
                let state = runtime_state(py);
                let mut passes = 0;
                loop {
                    passes += 1;
                    assert!(passes <= 8, "finite finalizer fixture did not quiesce");
                    let mut changed = super::clear_runtime_callback_roots(py, state);
                    changed |= super::clear_thread_local_state(py);
                    if !changed {
                        break;
                    }
                }
                assert!(
                    passes >= 3,
                    "runtime-only reentry requires another drain pass"
                );
                assert_eq!(REENTRANT_CONTEXT_RELEASES.load(Ordering::SeqCst), 2);
                assert!(state.contextvars.lock().unwrap().var_defaults.is_empty());
                assert!(!crate::exception_pending(py));
                crate::dec_ref_bits(py, class);
                REENTRANT_CONTEXT_CLASS.store(0, Ordering::SeqCst);
            });
        });
    }

    fn traced_worker(should_panic: bool) -> (std::thread::Result<()>, Vec<&'static str>) {
        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let (stage_tx, stage_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            id_tx.send(std::thread::current().id()).unwrap();
            go_rx.recv().unwrap();
            crate::test_support::with_expected_panic(|| {
                super::run_runtime_worker(|| {
                    crate::with_gil_entry_nopanic!(_py, {
                        let _ = crate::runtime_state(_py);
                        if should_panic {
                            let _: u64 = crate::builtins::exceptions::raise_exception(
                                _py,
                                "RuntimeError",
                                "intentional worker unwind",
                            );
                            panic!("intentional worker unwind");
                        }
                    });
                });
            });
        });
        let worker_id = id_rx.recv().unwrap();
        *THREAD_LOCAL_DROP_TEST_TRACE.lock().unwrap() = Some((worker_id, stage_tx));
        go_tx.send(()).unwrap();
        let result = worker.join();
        *THREAD_LOCAL_DROP_TEST_TRACE.lock().unwrap() = None;
        (result, stage_rx.try_iter().collect())
    }

    fn assert_runtime_worker_released_ic_tls(stages: &[&'static str]) {
        assert_eq!(
            stages
                .iter()
                .filter(|stage| **stage == "ic_caches_cleared")
                .count(),
            1,
            "the stack-owned runtime worker guard must drain IC refs exactly once"
        );
    }

    #[test]
    fn normal_worker_releases_ic_tls_before_key_destruction() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let (result, stages) = traced_worker(false);
        assert!(result.is_ok());
        assert_runtime_worker_released_ic_tls(&stages);
    }

    #[test]
    fn panicking_worker_does_not_double_panic_during_tls_cleanup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let (result, stages) = traced_worker(true);

        assert!(
            result.is_err(),
            "the primary worker panic must remain observable after TLS cleanup"
        );
        assert_runtime_worker_released_ic_tls(&stages);
    }

    #[test]
    fn clear_worker_thread_state_keeps_gil_for_tls_cleanup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            clear_worker_thread_state(_py);
        });
    }

    #[test]
    fn clear_special_cache_releases_function_descriptor_slots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_special_cache(_py, state);

            let code_ptr = alloc_string(_py, b"__code__ descriptor sentinel");
            assert!(!code_ptr.is_null());
            let globals_ptr = alloc_string(_py, b"__globals__ descriptor sentinel");
            assert!(!globals_ptr.is_null());
            state
                .special_cache
                .function_code_descriptor
                .store(MoltObject::from_ptr(code_ptr).bits(), Ordering::Release);
            state
                .special_cache
                .function_globals_descriptor
                .store(MoltObject::from_ptr(globals_ptr).bits(), Ordering::Release);

            clear_special_cache(_py, state);
            assert_eq!(
                state
                    .special_cache
                    .function_code_descriptor
                    .load(Ordering::Acquire),
                0
            );
            assert_eq!(
                state
                    .special_cache
                    .function_globals_descriptor
                    .load(Ordering::Acquire),
                0
            );
        });
    }

    #[test]
    fn clear_interned_names_releases_every_manifest_slot() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_interned_names(_py, state);

            let slots = state.interned.slots();
            for (index, slot) in slots.iter().enumerate() {
                let name = format!("interned-name-slot-{index}");
                let ptr = alloc_string(_py, name.as_bytes());
                assert!(!ptr.is_null());
                slot.store(MoltObject::from_ptr(ptr).bits(), Ordering::Release);
            }

            clear_interned_names(_py, state);

            for slot in slots {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }
}
