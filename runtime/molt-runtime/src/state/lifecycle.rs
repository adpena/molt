mod module_retirement;
use module_retirement::ModuleRetirement;

use crate::PyToken;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::async_rt::sockets::socket_runtime_state_clear;
use crate::builtins::attr::clear_attr_tls_caches;
use crate::builtins::attributes::attributes_clear_runtime_state;
use crate::builtins::codecs_ext::codecs_clear_error_handlers;
use crate::builtins::concurrent::concurrent_clear_runtime_state;
use crate::builtins::contextvars::clear_thread_context;
use crate::builtins::copy_mod::copy_memo_clear_state;
use crate::builtins::exceptions::{
    canonical_exception_class_roots, drain_dynamic_exception_type_cache,
    exceptions_release_runtime_class_anchor, take_thread_exception_for_teardown,
};
use crate::builtins::functools::{
    functools_clear_runtime_callbacks, functools_clear_runtime_state, functools_runtime_class_roots,
};
use crate::builtins::io::io_clear_runtime_state;
use crate::builtins::modules::modules_clear_runtime_state;
use crate::builtins::operator::{
    operator_clear_runtime_callbacks, operator_clear_runtime_state, operator_runtime_class_roots,
};
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
    CURRENT_EXCEPTION_PENDING, CURRENT_TASK, CURRENT_TOKEN, EXCEPTION_STACK, FRAME_STACK,
    GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, GilGuard, GilReleaseGuard, MoltObject,
    PARSE_ARENA, TASK_RAISE_ACTIVE, TRACE_FRAME_PUSH_STACK, TYPE_ID_DICT, TYPE_ID_FILE_HANDLE,
    TYPE_ID_MODULE, alloc_string, builtin_classes_retire_identities, builtin_classes_shutdown,
    call_callable0, clear_exception, clear_exception_type_cache,
    clear_thread_exception_for_teardown, dec_ref_bits, default_cancel_tokens,
    dict_clear_in_place_shutdown, dict_get_in_place, exception_pending,
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
    // The guard's teardown exits every frame the thread still has, which
    // releases the bindings their homes own: the frame stack and the home
    // arena must outlive it.
    let _ = FRAME_STACK.try_with(|_| {});
    crate::builtins::frames::touch_frame_home_tls_lifetime();
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
    // Only inspect published owners: teardown must never initialize a service.
    let scheduler = state.scheduler.get();
    let timer = state.sleep_queue.get();
    let poller = state.io_poller.get();
    #[cfg(not(target_arch = "wasm32"))]
    let pool = state.thread_pool.get();
    if let Some(scheduler) = scheduler {
        scheduler.stop();
    }
    if let Some(timer) = timer {
        timer.stop();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(poller) = poller {
        poller.stop();
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(poller) = poller {
        poller.shutdown();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(pool) = pool {
        pool.stop();
    }
    let executors = crate::builtins::concurrent::concurrent_stop_runtime_workers(state);
    let any = scheduler.is_some() || timer.is_some() || poller.is_some() || executors;
    #[cfg(not(target_arch = "wasm32"))]
    let any = any || pool.is_some();
    if !any {
        return;
    }
    trace_shutdown("workers_shutdown_start");
    let closed = {
        let _release = GilReleaseGuard::suspend();
        let mut closed = true;
        if let Some(scheduler) = scheduler {
            closed &= scheduler.join();
        }
        if let Some(timer) = timer {
            closed &= timer.join();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(poller) = poller {
            closed &= poller.join();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(pool) = pool {
            closed &= pool.join();
        }
        closed &= crate::builtins::concurrent::concurrent_join_runtime_workers(state);
        closed
    };
    // Reacquire GIL before reporting failure. Every joined disposition and
    // unfinished handle remains in its published owner; no root was detached.
    assert!(
        closed,
        "runtime teardown refused: joined service workers failed; origin retained"
    );
    trace_shutdown("workers_shutdown_done");
}

fn clear_stopped_runtime_worker_queues(py: &PyToken<'_>, state: &RuntimeState) -> bool {
    let mut changed = false;
    if let Some(scheduler) = state.scheduler.get() {
        changed |= scheduler.clear_stopped_queue();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(pool) = state.thread_pool.get() {
        changed |= pool.clear_stopped_queue(py);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = py;
    changed
}

fn runtime_teardown_inner(_py: &PyToken<'_>, state: &RuntimeState, mode: RuntimeTeardownMode) {
    crate::gil_assert();
    // Close before any callback or root mutation. A delayed shared worker has
    // already reserved its origin heap and requires that whole owner to remain.
    #[cfg(not(target_arch = "wasm32"))]
    state.close_thread_borrow_admission();
    // Own callback execution from the first collector/pending-call callback
    // through the callback-free release tail. A public caller may already own
    // an ordinary lease; raw GIL or C-extension context alone is insufficient.
    let _custody = crate::concurrency::execution::ShutdownDrainExecutionCustody::enter_if_needed();
    // Isolate and WASM teardown can begin without an ordinary attached frame.
    // Establish the same queue/error owner before the first cleanup callback.
    let mut shutdown_thread_state =
        molt_cpython_abi::api::object::RuntimeShutdownThreadState::begin(state.identity());
    trace_shutdown("start");
    crate::builtins::exceptions::memory_error::begin_shutdown(_py);
    // Close process-service launch and prove every retained OS worker finished
    // before collection or callbacks can retire their task/stream owners.
    shutdown_started_runtime_workers(_py, state);
    #[cfg(not(target_arch = "wasm32"))]
    {
        state.assert_thread_borrows_drained();
        if crate::state::runtime_state::owns_process_cpython_state(state) {
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_attachment_count(),
                usize::from(molt_cpython_abi::api::object::runtime_execution_thread_is_attached()),
                "runtime teardown refused: foreign runtime execution attachments remain live"
            );
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                usize::from(
                    molt_cpython_abi::api::object::current_thread_has_retained_runtime_state()
                ),
                "runtime teardown refused: foreign retained PyThreadState owners remain live"
            );
        }
    }
    trace_shutdown("drain_process_registry");
    state.process_registry.drain_for_teardown();
    if mode == RuntimeTeardownMode::ProcessExit {
        crate::object::ops::profile_dump_with_gil(_py);
        // Pre-teardown peak-live canary, not the post-teardown true-leak gauge.
        crate::object::ops::assert_no_leak_at_exit(_py);
    }
    // All finalization modes retire ordinary cycles through the shared mixed
    // collector. Preserve the first collection before pending-call callbacks.
    // A failed/no-op collector cannot supply progress to the owner fixed point.
    let mut cycle_collection_available = true;
    collect_cycles_for_teardown(_py, &mut cycle_collection_available);
    // Pending-call admission and its ring are process-static. An isolate owns
    // its native thread state, not the primary runtime's pending callbacks.
    if crate::state::runtime_state::owns_process_cpython_state(state) {
        trace_shutdown("finish_pending_calls");
        finish_pending_calls_for_teardown(_py);
    }
    clear_stopped_runtime_worker_queues(_py, state);
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
    trace_shutdown("drain_runtime_class_callbacks");
    // C roots can run extension deallocators. Close their readiness while all
    // runtime lookup/class authority is still present, then use the same root
    // owner drain before and throughout canonical class retirement.
    state.cpython.retire_static_roots();
    let mut modules = ModuleRetirement::new(_py);
    clear_runtime_callback_roots(_py, state, &mut modules);
    let mut retirement = RuntimeClassRetirement::new();
    trace_shutdown("drain_shutdown_owner_thread_state");
    molt_cpython_abi::api::object::detach_runtime_execution_thread();
    loop {
        let mut drain = || {
            // Collect cycles released by the preceding runtime/C TLS pass.
            // Drain owners AFTER collection: even a zero-collected finalizer
            // can resurrect into a module, cache, or either thread-state domain.
            let mut changed = collect_cycles_for_teardown(_py, &mut cycle_collection_available);
            changed |= retirement.include(_py, runtime_class_roots(_py, state));
            changed |= clear_runtime_callback_roots(_py, state, &mut modules);
            changed |= retirement.clear_contents(_py);
            changed |= clear_thread_local_state(_py);
            changed |= clear_interned_names(_py, state);
            changed |= clear_runtime_static_names(_py, state);
            changed
        };
        // Keep one actual C record through the whole fixed point. Its queue is
        // needed by native decrefs after each managed-edge drain returns.
        let drained = shutdown_thread_state.drain(&mut drain);
        // Forced C-view retirement must not run inside the TLS cleanup callback:
        // the C error/context/dict owners are still live there, and error
        // preservation temporarily moves those owners off the visible record.
        // The completed drain has emptied both managed ownership domains.
        // Projection release can publish fresh owners, so repeat both domains
        // before advancing module retirement or destroying class metadata.
        if retirement.retire_projections(_py, &drained) {
            continue;
        }
        if !modules.advance() {
            break;
        }
    }
    shutdown_thread_state.finish();
    if mode == RuntimeTeardownMode::Embedding {
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0,
            "retained PyThreadState survived the last live-runtime callback drain"
        );
    }
    crate::builtins::exceptions::memory_error::finish_shutdown(_py);
    trace_shutdown("retire_runtime_class_identities");
    builtin_classes_retire_identities(_py, state, &retirement);
    trace_shutdown("clear_exception_type_cache");
    clear_exception_type_cache(_py, state);
    trace_shutdown("release_cached_runtime_class_anchors");
    types_clear_runtime_state(_py, state);
    functools_clear_runtime_state(_py, state);
    operator_clear_runtime_state(_py, state);
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

/// Keep shutdown collection inside the callback-capable owner transaction.
/// Only completed reclamation is progress; failure/no-op outcomes are reported
/// once and disable further attempts in this teardown. The normal retirement
/// assertions still reject live native allocations before registry reset.
fn collect_cycles_for_teardown(py: &PyToken<'_>, available: &mut bool) -> bool {
    if !*available {
        return false;
    }
    trace_shutdown("collect_cycles");
    let outcome = unsafe { crate::object::gc::collect_cycles(py) };
    use crate::object::gc::GcCollectStatus;
    match outcome.status {
        GcCollectStatus::Completed => outcome.retired != 0,
        GcCollectStatus::ReentrantNoop => {
            *available = false;
            eprintln!(
                "molt gc: shutdown collection skipped while another collection is active; no retry during teardown"
            );
            false
        }
        GcCollectStatus::UnsupportedConcurrency => {
            *available = false;
            eprintln!(
                "molt gc: shutdown collection unsupported without a free-threaded stop-the-world epoch"
            );
            false
        }
        failure @ (GcCollectStatus::ResourceError(_) | GcCollectStatus::CallbackError(_)) => {
            *available = false;
            eprintln!(
                "molt gc: shutdown collection failed closed: {failure:?}; no retry during teardown"
            );
            false
        }
    }
}

/// One ordered owner family, shared by initial root retirement and every
/// callback fixed-point pass. Never short-circuit: a later owner can repopulate
/// an earlier one. Each cleanup detaches its complete cohort before release.
fn clear_runtime_callback_roots(
    py: &PyToken<'_>,
    state: &RuntimeState,
    modules: &mut ModuleRetirement<'_, '_>,
) -> bool {
    let mut changed = clear_stopped_runtime_worker_queues(py, state);
    changed |= concurrent_clear_runtime_state(py, state);
    changed |= clear_task_state(py, state);
    changed |= signal_clear_state(py, state);
    changed |= clear_thread_context(py);
    changed |= copy_memo_clear_state(py, state);
    changed |= sys_ext_clear_state(py, state);
    changed |= c_api_module_clear_state(py, state);
    changed |= runtime_extension_states_clear_and_drop(state);
    changed |= modules.drain(state);
    changed |= modules_clear_runtime_state(py, state);
    changed |= crate::object::gc::gc_clear_api_roots(py);
    changed |= platform_clear_runtime_state(py, state);
    changed |= io_clear_runtime_state(py, state);
    changed |= codecs_clear_error_handlers(py, state);
    changed |= crate::builtins::exceptions::memory_error::clear_shutdown_fields(py);
    changed |= exceptions_clear_runtime_state(py, state);
    changed |= drain_dynamic_exception_type_cache(py, state);
    changed |= types_clear_runtime_callbacks(py, state);
    changed |= clear_stateful_locals(py, state);
    changed |= clear_method_cache(py, state);
    changed |= attributes_clear_runtime_state(py, state);
    changed |= clear_special_cache(py, state);
    changed |= clear_code_slots(py, state);
    changed |= clear_asyncgen_hooks(py, state);
    changed |= functools_clear_runtime_callbacks(py, state);
    changed |= operator_clear_runtime_callbacks(py, state);
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
    roots.extend(functools_runtime_class_roots(py, state));
    roots.extend(operator_runtime_class_roots(py, state));
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

fn release_asyncgen_hooks(
    py: &PyToken<'_>,
    hooks: impl IntoIterator<Item = crate::state::runtime_state::AsyncGenHooks>,
) -> bool {
    let mut changed = false;
    for hook in hooks {
        for bits in [hook.firstiter, hook.finalizer] {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                changed = true;
                dec_ref_bits(py, bits);
            }
        }
    }
    changed
}

fn clear_asyncgen_hooks(py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let hooks = std::mem::take(&mut *state.asyncgen_hooks.lock().unwrap());
    release_asyncgen_hooks(py, hooks.into_values())
}

fn clear_thread_asyncgen_hooks(py: &PyToken<'_>) -> bool {
    let hook = runtime_state(py)
        .asyncgen_hooks
        .lock()
        .unwrap()
        .remove(&std::thread::current().id());
    release_asyncgen_hooks(py, hook)
}

fn clear_stateful_locals(py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let layouts = std::mem::take(&mut *state.stateful_locals.lock().unwrap());
    let changed = !layouts.is_empty();
    for layout in layouts.into_values() {
        crate::state::runtime_state::release_stateful_locals_layout(py, layout);
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
    let mut changed =
        exception.is_some() | clear_thread_asyncgen_hooks(_py) | clear_thread_context(_py);
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    let contexts = CONTEXT_STACK
        .try_with(|stack| std::mem::take(&mut *stack.borrow_mut()))
        .unwrap_or_default();
    let frames = FRAME_STACK
        .try_with(|stack| std::mem::take(&mut *stack.borrow_mut()))
        .unwrap_or_default();
    let pending_namespaces = crate::builtins::frames::take_pending_namespaces_for_teardown();
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
    super::recursion::assert_thread_recursion_idle();
    let _ = GENERATOR_RAISE.try_with(|flag| flag.set(false));
    let _ = TASK_RAISE_ACTIVE.try_with(|flag| flag.set(false));
    let _ = BLOCK_ON_TASK.try_with(|cell| cell.set(std::ptr::null_mut()));
    let _ = CURRENT_TASK.try_with(|cell| cell.set(std::ptr::null_mut()));
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    let _ = CURRENT_TOKEN.try_with(|cell| cell.set(1));
    // Every TLS owner and execution marker is detached before Python can run.
    // In particular no RefCell borrow survives a decref that can repopulate the
    // same context/exception stack from a finalizer.
    changed |= !contexts.is_empty()
        || !frames.is_empty()
        || !active.is_empty()
        || !generators.is_empty()
        || !pending_namespaces.is_empty();
    if let Some(bits) = exception {
        dec_ref_bits(_py, bits);
    }
    for bits in contexts.into_iter().chain(active) {
        dec_ref_bits(_py, bits);
    }
    // Every abandoned entry exits as a return would, innermost first: a frame
    // object that shares its bindings takes them over, otherwise they are
    // released in the target's order, and its homes go back only after those
    // releases finish. The stack is already unlinked above.
    for frame in frames.into_iter().rev() {
        frame.exit(_py);
    }
    for bits in pending_namespaces {
        dec_ref_bits(_py, bits);
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
        let binding = slot.take(_py);
        if binding.code_bits != 0 || binding.globals_bits != 0 {
            detached.push(binding);
        }
    }
    let changed = !detached.is_empty();
    for binding in detached {
        binding.release(_py);
    }
    changed
}

pub(crate) fn clear_worker_thread_state(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    clear_thread_local_state(_py)
}

/// Existing service fields own this handle set. Publication precedes native
/// startup; one mutex serializes one-shot New -> Ready/Failed and demand-driven
/// append, so no caller can consume partial readiness. Joins release the mutex
/// and retain their failure payload. Pool stop excludes any subsequent append.
#[derive(Default)]
pub(crate) struct RuntimeServiceWorkers {
    inner: std::sync::Mutex<RuntimeServiceWorkerState>,
    joined: std::sync::Condvar,
}

#[derive(Default)]
struct RuntimeServiceWorkerState {
    start: RuntimeServiceStart,
    workers: Vec<RuntimeServiceWorker>,
    // Successfully joined prefix; bounded reaping never rescans old records.
    #[cfg(not(target_arch = "wasm32"))]
    reap_cursor: usize,
}

#[derive(Default)]
enum RuntimeServiceStart {
    #[default]
    New,
    Ready,
    Failed(String),
}

pub(crate) struct RuntimeServiceWorker {
    handle: Option<std::thread::JoinHandle<()>>,
    // Retain the target identity after taking its handle: a worker must still
    // refuse to wait for itself when a concurrent join owns that handle.
    joining: Option<std::thread::ThreadId>,
    panic: Option<Box<dyn std::any::Any + Send + 'static>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeServiceJoinStatus {
    Closed,
    Pending,
    Failed,
}

/// Distinguish handle-storage allocation from native thread-start refusal:
/// CPython exposes MemoryError for preparation and RuntimeError for OS startup.
pub(crate) enum RuntimeServiceGrowthError {
    Capacity,
    Start(std::io::Error),
}

impl From<std::io::Error> for RuntimeServiceGrowthError {
    fn from(error: std::io::Error) -> Self {
        Self::Start(error)
    }
}

impl std::fmt::Display for RuntimeServiceGrowthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capacity => formatter.write_str("worker handle allocation failed"),
            Self::Start(error) => error.fmt(formatter),
        }
    }
}

impl RuntimeServiceWorkers {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn start(
        &self,
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
        start: impl FnOnce(&mut Vec<RuntimeServiceWorker>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.start_transaction(origin, true, start)
    }

    /// Add at most one native worker for actual queued/in-flight demand. The
    /// caller serializes pool submission and stop; joins only follow stop.
    /// Ordinary spawn errors leave this operation retryable, unlike one-shot
    /// service startup. Every successful handle uses the same retained owner.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn grow_for_demand(
        &self,
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
        demand: &std::sync::atomic::AtomicUsize,
        maximum: usize,
        spawn: impl FnOnce(usize) -> std::io::Result<RuntimeServiceWorker>,
    ) -> Result<(), RuntimeServiceGrowthError> {
        self.start_transaction(origin, false, |workers| {
            let count = workers.len();
            if count < maximum && count < demand.load(std::sync::atomic::Ordering::Relaxed) {
                // Reserve handle storage before creating an OS thread: even a
                // failed allocation must never detach a successfully spawned child.
                workers
                    .try_reserve(1)
                    .map_err(|_| RuntimeServiceGrowthError::Capacity)?;
                workers.push(spawn(count)?);
            }
            Ok(())
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_transaction<E: From<std::io::Error> + std::fmt::Display>(
        &self,
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
        once: bool,
        start: impl FnOnce(&mut Vec<RuntimeServiceWorker>) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut inner = self.inner.lock().unwrap();
        match &inner.start {
            RuntimeServiceStart::Ready if once => return Ok(()),
            RuntimeServiceStart::Failed(reason) => {
                return Err(std::io::Error::other(reason.clone()).into());
            }
            RuntimeServiceStart::New | RuntimeServiceStart::Ready => {}
        }
        let outcome = if origin.state().thread_borrow_admission_is_open() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                start(&mut inner.workers)
            })) {
                Ok(outcome) => outcome,
                Err(panic) => {
                    inner.start =
                        RuntimeServiceStart::Failed("runtime service startup panicked".into());
                    // Publish failure before unwinding; preserve partially
                    // spawned handles without poisoning the retained owner.
                    drop(inner);
                    std::panic::resume_unwind(panic)
                }
            }
        } else {
            Err(std::io::Error::other("runtime service admission is closed").into())
        };
        match &outcome {
            Ok(()) => inner.start = RuntimeServiceStart::Ready,
            Err(error) if once => inner.start = RuntimeServiceStart::Failed(error.to_string()),
            Err(_) => {} // A failed dynamic append may be retried by later demand.
        }
        outcome
    }

    /// One take/join/record transaction for blocking shutdown and bounded
    /// reaping. Never hold either the owner mutex or GIL across native joining:
    /// even is_finished does not replace the actual TLS-completion join.
    fn join_worker(&self, index: usize, wait: bool) -> RuntimeServiceJoinStatus {
        let handle = loop {
            let mut inner = self.inner.lock().unwrap();
            let worker = &mut inner.workers[index];
            if worker.panic.is_some() {
                return RuntimeServiceJoinStatus::Failed;
            }
            if let Some(target) = worker.joining {
                if !wait || target == std::thread::current().id() {
                    return RuntimeServiceJoinStatus::Pending;
                }
                drop(inner);
                {
                    // Recheck after releasing custody to avoid a lost notify.
                    // Drop the owner mutex before restoring the caller's GIL.
                    let _release = crate::gil_held().then(GilReleaseGuard::suspend);
                    #[cfg(all(test, not(target_arch = "wasm32")))]
                    SERVICE_JOIN_WAITING.with(|slot| {
                        if let Some(ready) = slot.borrow_mut().take() {
                            ready.send(()).unwrap();
                        }
                    });
                    let inner = self.inner.lock().unwrap();
                    drop(
                        self.joined
                            .wait_while(inner, |inner| inner.workers[index].joining.is_some())
                            .unwrap(),
                    );
                }
                continue;
            }
            let Some(handle) = worker.handle.as_ref() else {
                return RuntimeServiceJoinStatus::Closed;
            };
            if handle.thread().id() == std::thread::current().id()
                || (!wait && !handle.is_finished())
            {
                return RuntimeServiceJoinStatus::Pending;
            }
            worker.joining = Some(handle.thread().id());
            break worker.handle.take().unwrap();
        };
        let outcome = {
            let _release = crate::gil_held().then(GilReleaseGuard::suspend);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(all(test, not(target_arch = "wasm32")))]
                SERVICE_JOIN_PAUSE.with(|slot| {
                    if let Some((ready, proceed)) = slot.borrow_mut().take() {
                        // A failed test rendezvous must still consume the real
                        // native join, never detach its extracted handle.
                        if ready.send(()).is_ok() {
                            let _ = proceed.recv_timeout(std::time::Duration::from_secs(5));
                        }
                    }
                });
                handle.join()
            }))
        };
        let mut inner = self.inner.lock().unwrap();
        let worker = &mut inner.workers[index];
        worker.panic = match outcome {
            Ok(result) => result.err(),
            Err(panic) => Some(panic),
        };
        worker.joining = None;
        self.joined.notify_all();
        if worker.panic.is_some() {
            RuntimeServiceJoinStatus::Failed
        } else {
            RuntimeServiceJoinStatus::Closed
        }
    }

    /// Stop/queue owners close work first. All dispositions stay published,
    /// including concurrent joins, self-join refusals and native panics.
    pub(crate) fn join(&self) -> bool {
        let count = {
            let inner = self.inner.lock().unwrap();
            let caller = std::thread::current().id();
            // Refuse before joining any sibling: two members concurrently
            // shutting down their owner must never join one another first.
            if inner.workers.iter().any(|worker| {
                worker.joining == Some(caller)
                    || worker
                        .handle
                        .as_ref()
                        .is_some_and(|handle| handle.thread().id() == caller)
            }) {
                return false;
            }
            inner.workers.len()
        };
        let mut closed = true;
        for index in 0..count {
            closed &= self.join_worker(index, true) == RuntimeServiceJoinStatus::Closed;
        }
        closed
    }

    /// Inspect at most `budget` records. A pending prefix retains the whole
    /// owner until a later poll; a failed prefix is terminal, never auto-retried.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn reap_finished(&self, budget: usize) -> (RuntimeServiceJoinStatus, usize) {
        let mut inspected = 0;
        for _ in 0..budget {
            let index = {
                let inner = self.inner.lock().unwrap();
                if inner.reap_cursor == inner.workers.len() {
                    return (RuntimeServiceJoinStatus::Closed, inspected);
                }
                inner.reap_cursor
            };
            inspected += 1;
            match self.join_worker(index, false) {
                RuntimeServiceJoinStatus::Closed => {
                    let mut inner = self.inner.lock().unwrap();
                    if inner.reap_cursor == index {
                        inner.reap_cursor += 1;
                    }
                }
                other => return (other, inspected),
            }
        }
        let inner = self.inner.lock().unwrap();
        let status = if inner.reap_cursor == inner.workers.len() {
            RuntimeServiceJoinStatus::Closed
        } else {
            RuntimeServiceJoinStatus::Pending
        };
        (status, inspected)
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn finished_for_test(&self) -> bool {
        self.inner.lock().unwrap().workers.iter().all(|worker| {
            worker.joining.is_none()
                && worker
                    .handle
                    .as_ref()
                    .is_none_or(std::thread::JoinHandle::is_finished)
        })
    }

    pub(crate) fn count(&self) -> usize {
        self.inner.lock().unwrap().workers.len()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
thread_local! {
    static SERVICE_SPAWN_FAILURE: std::cell::Cell<Option<std::io::ErrorKind>> = const { std::cell::Cell::new(None) };
    pub(crate) static SERVICE_JOIN_PAUSE: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>
    )>> = const { std::cell::RefCell::new(None) };
    pub(crate) static SERVICE_JOIN_WAITING: std::cell::RefCell<Option<
        std::sync::mpsc::Sender<()>
    >> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn with_service_spawn_failure_for_test<R>(
    kind: std::io::ErrorKind,
    action: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<std::io::ErrorKind>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SERVICE_SPAWN_FAILURE.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(SERVICE_SPAWN_FAILURE.with(|slot| slot.replace(Some(kind))));
    let result = action();
    assert!(
        SERVICE_SPAWN_FAILURE.with(|slot| slot.get()).is_none(),
        "native spawn boundary was not reached"
    );
    result
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeServiceWorker {
    pub(crate) fn spawn(
        origin: crate::state::runtime_state::RuntimeWorkerOrigin,
        builder: std::thread::Builder,
        worker: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<Self> {
        #[cfg(test)]
        if let Some(kind) = SERVICE_SPAWN_FAILURE.with(|slot| slot.take()) {
            return Err(std::io::Error::from(kind));
        }
        let handle = builder.spawn(move || run_runtime_worker(origin, worker))?;
        Ok(Self {
            handle: Some(handle),
            joining: None,
            panic: None,
        })
    }
}

/// A retained service join owner permits destruction, including Finalizing,
/// without admitting ordinary work or keeping a permanent execution lease.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_runtime_worker<R>(
    origin: crate::state::runtime_state::RuntimeWorkerOrigin,
    worker: impl FnOnce() -> R,
) -> R {
    origin.install();
    touch_tls_guard();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker));
    let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let gil = GilGuard::new();
        let _custody =
            crate::concurrency::execution::ShutdownDrainExecutionCustody::for_retained_worker(
                origin,
            );
        molt_cpython_abi::api::object::detach_runtime_execution_thread();
        molt_cpython_abi::api::object::clear_runtime_execution_thread_state(|| {
            clear_worker_thread_state(&gil.token())
        });
    }));
    match (outcome, cleanup) {
        (Ok(value), Ok(())) => {
            crate::state::clear_thread_runtime_state();
            value
        }
        (Err(panic), Ok(())) => {
            crate::state::clear_thread_runtime_state();
            std::panic::resume_unwind(panic)
        }
        (Ok(value), Err(panic)) => {
            std::mem::forget(value);
            std::panic::resume_unwind(panic)
        }
        (Err(original), Err(cleanup)) => {
            // Failure retains origin TLS and the enclosing handle owner. A
            // second panic payload destructor cannot replace the first cause.
            std::mem::forget(cleanup);
            std::panic::resume_unwind(original)
        }
    }
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
        guard
            .values_mut()
            .filter_map(Option::take)
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
    let execution_attachments = {
        let mut guard = state.task_tokens.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    changed |= !execution_attachments.is_empty();
    // Spawn references belong to scheduler execution, not to the task's GC
    // graph. Detach their flags before callback-bearing retirement releases.
    let spawned_roots = execution_attachments
        .keys()
        .filter_map(|slot| crate::async_rt::cancellation::take_task_spawn_root(slot.0))
        .collect::<Vec<_>>();
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
    for attachment in execution_attachments.into_values() {
        if let crate::async_rt::cancellation::TaskContextBinding::Owned(bits) = attachment.context {
            dec_ref_bits(_py, bits);
        }
    }
    for bits in spawned_roots {
        dec_ref_bits(_py, bits);
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

fn flush_stdio_handles(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let sys_bits = state.interpreter_sys.module(_py);
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
        let val_bits = match val_bits {
            Ok(Some(bits)) => bits,
            // Shutdown's existing flush policy reports its own failures and
            // has no later dictionary/default fallback.
            Ok(None) | Err(()) => return,
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
        &state.special_cache.awaitable_await,
        &state.special_cache.function_code_descriptor,
        &state.special_cache.function_globals_descriptor,
        &state.special_cache.weakref_callback_descriptor,
    ];
    clear_atomic_slots(_py, &slots)
}

#[cfg(test)]
pub(crate) mod shutdown_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        THREAD_LOCAL_DROP_TEST_TRACE, clear_interned_names, clear_special_cache,
        clear_worker_thread_state,
    };
    use crate::{MoltObject, alloc_string, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn service_member_refuses_before_joining_an_earlier_live_sibling() {
        use std::sync::{Arc, mpsc};
        use std::time::Duration;
        let owner = Arc::new(super::RuntimeServiceWorkers::default());
        let (release_tx, release_rx) = mpsc::channel();
        let earlier = std::thread::spawn(move || {
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let (start_tx, start_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let member_owner = Arc::clone(&owner);
        let member = std::thread::spawn(move || {
            start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            result_tx.send(member_owner.join()).unwrap();
        });
        {
            let mut inner = owner.inner.lock().unwrap();
            for handle in [earlier, member] {
                inner.workers.push(super::RuntimeServiceWorker {
                    handle: Some(handle),
                    joining: None,
                    panic: None,
                });
            }
        }
        start_tx.send(()).unwrap();
        let result_before_sibling_release = result_rx.recv_timeout(Duration::from_secs(2));
        // Always settle both real native handles, including the regression case.
        release_tx.send(()).unwrap();
        assert!(owner.join());
        assert_eq!(
            result_before_sibling_release.unwrap(),
            false,
            "service member joined an earlier sibling before refusing itself"
        );
    }

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

    static REENTRANT_CONTEXT_PUBLISH_C: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static REENTRANT_CONTEXT_C_PUBLICATIONS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    #[cfg(not(target_arch = "wasm32"))]
    static JOINED_SERVICE_ORIGIN: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn install_joined_service_control(
        py: &crate::PyToken<'_>,
        panic_worker: bool,
        partial_start: bool,
        cleanup_panic: bool,
    ) -> std::sync::Weak<crate::async_rt::scheduler::SleepQueue> {
        let state = runtime_state(py);
        let origin = crate::state::runtime_state::RuntimeWorkerOrigin::new(state);
        JOINED_SERVICE_ORIGIN.store(origin.identity(), Ordering::SeqCst);
        REENTRANT_CONTEXT_RELEASES.store(0, Ordering::SeqCst);
        REENTRANT_CONTEXT_C_PUBLICATIONS.store(0, Ordering::SeqCst);
        REENTRANT_CONTEXT_PUBLISH_C.store(true, Ordering::SeqCst);
        let queue = std::sync::Arc::new(crate::async_rt::scheduler::SleepQueue::new());
        assert!(state.sleep_queue.set(std::sync::Arc::clone(&queue)).is_ok());
        let worker_queue = std::sync::Arc::clone(&queue);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let outcome = queue.install_joined_test_worker(origin, partial_start, move || {
            {
                let work =
                    crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
                        .unwrap();
                assert_eq!(runtime_state(&work.token()).identity(), origin.identity());
                assert_eq!(origin.state().thread_borrow_count(), 1);
                assert!(
                    crate::state::runtime_state::current_thread_holds_runtime_execution_lease()
                );
            }
            assert_eq!(origin.state().thread_borrow_count(), 0);
            // Preserve real callback-bearing TLS for terminal worker cleanup.
            {
                let entry = crate::concurrency::RuntimeExecutionGuard::enter();
                let py = entry.token();
                let class = reentrant_context_class(&py);
                REENTRANT_CONTEXT_CLASS.store(class, Ordering::SeqCst);
                publish_context_binding(&py, class);
                crate::dec_ref_bits(&py, class);
                // Production per-work entry retires its C record. Preserve a
                // runtime TLS owner only, so the ordinary foreign-C-owner
                // shutdown refusal cannot mask terminal service cleanup.
                molt_cpython_abi::api::object::detach_runtime_execution_thread();
                molt_cpython_abi::api::object::clear_runtime_execution_thread_state(|| false);
            }
            ready_tx.send(()).unwrap();
            worker_queue.wait_for_test_stop();
            assert!(
                crate::concurrency::execution::RuntimeServiceExecutionGuard::try_enter(origin)
                    .is_none()
            );
            if cleanup_panic {
                assert!(
                    cfg!(panic = "unwind"),
                    "cleanup panic control requires unwind"
                );
                #[cfg(panic = "unwind")]
                crate::concurrency::execution::inject_shutdown_drain_drop_panic();
            }
            if panic_worker {
                panic!("injected joined service worker panic");
            }
        });
        assert_eq!(outcome.is_err(), partial_start);
        if partial_start {
            // Failed readiness must persist; no second creator may retry.
            assert!(queue.start_worker(origin).is_err());
        }
        {
            let _release = crate::GilReleaseGuard::suspend();
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        std::sync::Arc::downgrade(&queue)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn assert_joined_service_control_drained() {
        assert_eq!(REENTRANT_CONTEXT_RELEASES.load(Ordering::SeqCst), 2);
        assert_eq!(REENTRANT_CONTEXT_C_PUBLICATIONS.load(Ordering::SeqCst), 1);
        JOINED_SERVICE_ORIGIN.store(0, Ordering::SeqCst);
    }

    fn publish_context_binding(py: &crate::PyToken<'_>, class: u64) {
        let class_ptr = crate::obj_from_bits(class)
            .as_ptr()
            .expect("context default class");
        let value = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        assert!(crate::obj_from_bits(value).as_ptr().is_some());
        let name_ptr = crate::alloc_string(py, b"shutdown-reentrant-default");
        assert!(!name_ptr.is_null());
        let name = MoltObject::from_ptr(name_ptr).bits();
        let variable = crate::builtins::contextvars::new_variable(py, name, None).unwrap();
        let token = crate::builtins::contextvars::set_variable(py, variable, value).unwrap();
        crate::dec_ref_bits(py, token);
        crate::dec_ref_bits(py, variable);
        crate::dec_ref_bits(py, name);
        crate::dec_ref_bits(py, value);
    }

    extern "C" fn repopulate_context_binding(_self: u64) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        if JOINED_SERVICE_ORIGIN.load(Ordering::SeqCst) != 0 {
            assert!(crate::concurrency::execution::current_thread_holds_shutdown_drain_custody());
            assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
            assert_eq!(
                crate::state::runtime_state::runtime_state_for_gil()
                    .unwrap()
                    .identity(),
                JOINED_SERVICE_ORIGIN.load(Ordering::SeqCst)
            );
        }
        crate::with_gil_entry_nopanic!(py, {
            if REENTRANT_CONTEXT_RELEASES.fetch_add(1, Ordering::SeqCst) == 0 {
                if REENTRANT_CONTEXT_PUBLISH_C.load(Ordering::SeqCst) {
                    if !molt_cpython_abi::api::object::current_thread_has_retained_runtime_state() {
                        // The runtime callback deliberately enters the absent C
                        // domain through its existing attachment owner, then
                        // leaves it detached for the outer fixed-point drain.
                        assert!(
                            molt_cpython_abi::api::object::attach_runtime_execution_thread(
                                runtime_state(py).identity()
                            )
                        );
                        molt_cpython_abi::api::object::detach_runtime_execution_thread();
                    }
                    let dict = unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() };
                    assert!(!dict.is_null());
                    REENTRANT_CONTEXT_C_PUBLICATIONS.fetch_add(1, Ordering::SeqCst);
                }
                publish_context_binding(py, REENTRANT_CONTEXT_CLASS.load(Ordering::SeqCst));
            }
            MoltObject::none().bits()
        })
    }

    fn reentrant_context_class(py: &crate::PyToken<'_>) -> u64 {
        let name = crate::attr_name_bits_from_bytes(py, b"ReentrantContextDefault").unwrap();
        let class = crate::molt_class_new(name);
        crate::dec_ref_bits(py, name);
        crate::molt_class_set_base(class, crate::builtin_classes(py).object);
        let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
        unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
        let method = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "repopulate_context_binding",
                repopulate_context_binding as *const (),
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
        class
    }

    #[test]
    fn callback_root_drain_revisits_context_bindings_repopulated_by_real_finalizers() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let class = reentrant_context_class(py);
                REENTRANT_CONTEXT_CLASS.store(class, Ordering::SeqCst);
                REENTRANT_CONTEXT_RELEASES.store(0, Ordering::SeqCst);
                publish_context_binding(py, class);
                let state = runtime_state(py);
                let mut passes = 0;
                let mut modules = super::ModuleRetirement::new(py);
                loop {
                    passes += 1;
                    assert!(passes <= 8, "finite finalizer fixture did not quiesce");
                    let mut changed = super::clear_runtime_callback_roots(py, state, &mut modules);
                    changed |= super::clear_thread_local_state(py);
                    if !changed {
                        break;
                    }
                }
                // Both drains can release context bindings in one pass. The
                // contract is complete retirement and a stable fixed point.
                assert_eq!(REENTRANT_CONTEXT_RELEASES.load(Ordering::SeqCst), 2);
                assert!(!super::clear_runtime_callback_roots(
                    py,
                    state,
                    &mut modules
                ));
                assert!(!super::clear_thread_local_state(py));
                assert_eq!(REENTRANT_CONTEXT_RELEASES.load(Ordering::SeqCst), 2);
                assert!(!crate::builtins::contextvars::clear_thread_context(py));
                assert!(!crate::exception_pending(py));
                crate::dec_ref_bits(py, class);
                REENTRANT_CONTEXT_CLASS.store(0, Ordering::SeqCst);
            });
        });
    }

    #[test]
    fn worker_end_revisits_runtime_and_c_finalizer_publication() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            for (initial_c_record, publish_c) in
                [(true, false), (true, true), (false, false), (false, true)]
            {
                std::thread::spawn(move || {
                    let entry = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                    let py = entry.token();
                    let class = reentrant_context_class(&py);
                    REENTRANT_CONTEXT_CLASS.store(class, Ordering::SeqCst);
                    REENTRANT_CONTEXT_RELEASES.store(0, Ordering::SeqCst);
                    REENTRANT_CONTEXT_C_PUBLICATIONS.store(0, Ordering::SeqCst);
                    REENTRANT_CONTEXT_PUBLISH_C.store(publish_c, Ordering::SeqCst);
                    publish_context_binding(&py, class);
                    if !initial_c_record {
                        // Remove only the empty C domain. The real runtime
                        // finalizers below still require repeated TLS cleanup.
                        molt_cpython_abi::api::object::detach_runtime_execution_thread();
                        molt_cpython_abi::api::object::clear_runtime_execution_thread_state(|| false);
                        assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
                    }
                    drop(entry);
                    // Observe before native TLS destructors or another entry
                    // could hide an unfinished cleanup pass in either domain.
                    assert_eq!(REENTRANT_CONTEXT_RELEASES.load(Ordering::SeqCst), 2);
                    assert_eq!(REENTRANT_CONTEXT_C_PUBLICATIONS.load(Ordering::SeqCst), usize::from(publish_c));
                    assert!(!molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
                    assert!(crate::CONTEXT_STACK.with(|stack| stack.borrow().is_empty()));
                    let entry = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
                    let py = entry.token();
                    assert!(!crate::builtins::contextvars::clear_thread_context(&py));
                    crate::dec_ref_bits(&py, class);
                    REENTRANT_CONTEXT_CLASS.store(0, Ordering::SeqCst);
                    REENTRANT_CONTEXT_PUBLISH_C.store(false, Ordering::SeqCst);
                }).join().unwrap();
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn worker_cached_method(_self: u64) -> u64 {
        MoltObject::none().bits()
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct WorkerIcWitness {
        class: u64,
        method: u64,
        receiver: u64,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl WorkerIcWitness {
        fn new(py: &crate::PyToken<'_>) -> Self {
            let name = crate::attr_name_bits_from_bytes(py, b"WorkerIcOwner").unwrap();
            let class = crate::molt_class_new(name);
            crate::dec_ref_bits(py, name);
            crate::molt_class_set_base(class, crate::builtin_classes(py).object);
            let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
            unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
            let method = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(
                    "worker_cached_method",
                    worker_cached_method as *const (),
                ),
                1,
            );
            assert!(!method.is_null());
            let method = MoltObject::from_ptr(method).bits();
            let name = crate::attr_name_bits_from_bytes(py, b"cached").unwrap();
            crate::molt_set_attr_name(class, name, method);
            crate::dec_ref_bits(py, name);
            let receiver = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
            assert!(crate::obj_from_bits(receiver).as_ptr().is_some());
            assert!(!crate::exception_pending(py));
            Self {
                class,
                method,
                receiver,
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl Drop for WorkerIcWitness {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(py, {
                for bits in [self.receiver, self.class, self.method] {
                    crate::dec_ref_bits(py, bits);
                }
            });
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn worker_method_refs(method: u64) -> u32 {
        crate::gil_assert();
        let ptr = crate::obj_from_bits(method).as_ptr().unwrap();
        unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn traced_worker(should_panic: bool) -> (std::thread::Result<()>, Vec<&'static str>) {
        let (origin, witness, baseline) = crate::with_gil_entry_nopanic!(py, {
            let witness = WorkerIcWitness::new(py);
            let baseline = worker_method_refs(witness.method);
            (
                crate::state::runtime_state::RuntimeWorkerOrigin::new(crate::runtime_state(py)),
                witness,
                baseline,
            )
        });
        // Main-thread owners keep the observed object alive across worker exit;
        // the public method call must add a distinct cache-owned reference.
        let method = witness.method;
        let receiver = witness.receiver;
        let (id_tx, id_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let (stage_tx, stage_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            id_tx.send(std::thread::current().id()).unwrap();
            go_rx.recv().unwrap();
            let outcome = crate::test_support::catch_expected_unwind(|| {
                super::run_runtime_worker(origin, || {
                    crate::with_gil_entry_nopanic!(py, {
                        let result = crate::call::bind::molt_call_method_ic0(
                            MoltObject::from_int(819).bits(),
                            receiver,
                            b"cached".as_ptr(),
                            6,
                        );
                        assert_eq!(result, MoltObject::none().bits());
                        crate::dec_ref_bits(py, result);
                        assert!(!crate::exception_pending(py));
                        assert_eq!(
                            worker_method_refs(method),
                            baseline + 1,
                            "the public call must retain a real method-cache owner"
                        );
                        if should_panic {
                            let _: u64 = crate::builtins::exceptions::raise_exception(
                                py,
                                "RuntimeError",
                                "intentional worker unwind",
                            );
                            panic!("intentional worker unwind");
                        }
                    });
                });
            });
            {
                // A raw GIL guard observes before another execution entry or
                // native TLS destructor could conceal an unfinished drain.
                let _gil = crate::GilGuard::new();
                assert_eq!(
                    worker_method_refs(method),
                    baseline,
                    "worker cleanup must release exactly the cache-owned reference"
                );
                super::trace_thread_local_drop("ic_owner_released");
            }
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
        });
        let worker_id = id_rx.recv().unwrap();
        *THREAD_LOCAL_DROP_TEST_TRACE.lock().unwrap() = Some((worker_id, stage_tx));
        go_tx.send(()).unwrap();
        let result = worker.join();
        *THREAD_LOCAL_DROP_TEST_TRACE.lock().unwrap() = None;
        drop(witness);
        (result, stage_rx.try_iter().collect())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_runtime_worker_released_ic_tls(stages: &[&'static str]) {
        let clear = stages
            .iter()
            .position(|stage| *stage == "ic_caches_cleared")
            .unwrap();
        let released = stages
            .iter()
            .position(|stage| *stage == "ic_owner_released")
            .unwrap();
        let tls_drop = stages.iter().position(|stage| *stage == "enter").unwrap();
        assert!(
            clear < released && released < tls_drop,
            "the real cache owner must retire before TLS key destruction: {stages:?}"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn normal_worker_releases_ic_tls_before_key_destruction() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let (result, stages) = traced_worker(false);
        assert!(result.is_ok());
        assert_runtime_worker_released_ic_tls(&stages);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn panicking_worker_does_not_double_panic_during_tls_cleanup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let (result, stages) = traced_worker(true);

        let panic = result.expect_err("the primary worker panic must remain observable");
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"intentional worker unwind"),
            "a fixture or cleanup assertion must not masquerade as the expected panic"
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
