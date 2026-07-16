use crate::PyToken;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::async_rt::sockets::socket_runtime_state_clear;
use crate::builtins::attr::clear_attr_tls_caches;
use crate::builtins::attributes::attributes_clear_runtime_state;
use crate::builtins::concurrent::concurrent_clear_runtime_state;
use crate::builtins::contextvars::contextvars_clear_state;
use crate::builtins::copy_mod::copy_memo_clear_state;
use crate::builtins::functions::python_builtin_functions_clear_runtime_state;
use crate::builtins::functools::functools_clear_runtime_state;
use crate::builtins::io::io_clear_runtime_state;
use crate::builtins::modules::modules_clear_runtime_state;
use crate::builtins::operator::operator_clear_runtime_state;
use crate::builtins::platform::platform_clear_runtime_state;
use crate::builtins::signal_ext::signal_clear_state;
use crate::builtins::sys_ext::sys_ext_clear_state;
use crate::builtins::types::types_clear_runtime_state;
use crate::c_api::c_api_module_clear_state;
use crate::call::bind::{clear_call_bind_ic_cache, clear_method_ic_cache, clear_super_ic_cache};
use crate::const_data_cache::clear_const_data_literal_caches;
use crate::object::builders::clear_builder_singletons;
use crate::object::dec_ref_ptr;
use crate::object::utf8_cache::{
    UTF8_CACHE_MAX_ENTRIES, UTF8_COUNT_CACHE_SHARDS, Utf8CacheStore, Utf8CountCacheStore,
    clear_utf8_count_tls,
};
use crate::{
    ACTIVE_EXCEPTION_FALLBACK, ACTIVE_EXCEPTION_STACK, BLOCK_ON_TASK, CONTEXT_STACK,
    CURRENT_EXCEPTION_PENDING, CURRENT_TASK, CURRENT_TOKEN, DEFAULT_RECURSION_LIMIT,
    EXCEPTION_STACK, FRAME_STACK, GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, GIL_DEPTH, GilGuard,
    GilReleaseGuard, MoltObject, NEXT_CANCEL_TOKEN_ID, PARSE_ARENA, RECURSION_DEPTH,
    RECURSION_LIMIT, TASK_RAISE_ACTIVE, TRACE_FRAME_PUSH_STACK, TYPE_ID_DICT, TYPE_ID_FILE_HANDLE,
    TYPE_ID_MODULE, alloc_string, builtin_classes_break_cycles, builtin_classes_shutdown,
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

#[cfg(feature = "stdlib_asyncio")]
use crate::asyncio_bridge::{asyncio_core_clear_state, asyncio_queue_clear_state};

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
        clear_thread_local_state(&gil.token());
        #[cfg(test)]
        trace_thread_local_drop("cleared");
    }
}

pub(crate) fn touch_tls_guard() {
    let _ = GIL_DEPTH.try_with(|_| {});
    let _ = PARSE_ARENA.try_with(|_| {});
    let _ = crate::REPR_SET.try_with(|_| {});
    let _ = TLS_GUARD.try_with(|_| {});
}

pub(crate) fn runtime_teardown(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, true);
}

pub(crate) fn runtime_teardown_isolate(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, false);
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
    crate::gil_assert();
    trace_shutdown("process_exit_start");
    trace_shutdown("process_exit_finish_pending_calls");
    finish_pending_calls_for_teardown(_py);
    shutdown_started_runtime_workers(_py, state);
    trace_shutdown("process_exit_drain_process_registry");
    state.process_registry.drain_for_teardown();
    trace_shutdown("process_exit_clear_concurrent_runtime_state");
    concurrent_clear_runtime_state(_py, state);
    #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
    {
        trace_shutdown("process_exit_clear_socket_state");
        socket_runtime_state_clear(state);
    }
    trace_shutdown("process_exit_clear_task_state");
    clear_task_state(_py, state);
    #[cfg(feature = "stdlib_asyncio")]
    {
        trace_shutdown("process_exit_clear_asyncio_queue_state");
        asyncio_queue_clear_state(_py);
    }
    trace_shutdown("process_exit_clear_thread_exception");
    clear_thread_exception_for_teardown(_py);
    trace_shutdown("process_exit_run_atexit_callbacks");
    crate::builtins::atexit::atexit_run_exitfuncs_teardown(_py);
    trace_shutdown("process_exit_clear_weakref_runtime_state");
    trace_shutdown("process_exit_clear_signal_state");
    signal_clear_state(_py, state);
    trace_shutdown("process_exit_clear_contextvars_state");
    contextvars_clear_state(_py, state);
    trace_shutdown("process_exit_clear_copy_memo_state");
    copy_memo_clear_state(_py, state);
    trace_shutdown("process_exit_clear_sys_ext_state");
    sys_ext_clear_state(_py, state);
    trace_shutdown("process_exit_flush_stdio");
    flush_stdio_handles(_py, state);
    trace_shutdown("process_exit_flush_stdio_post_finalizers");
    flush_stdio_handles(_py, state);
    trace_shutdown("process_exit_clear_io_runtime_state");
    io_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_modules_runtime_state");
    modules_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_platform_runtime_state");
    platform_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_c_api_module_state");
    c_api_module_clear_state(_py, state);
    trace_shutdown("process_exit_clear_runtime_extension_states");
    runtime_extension_states_clear_and_drop(state);
    trace_shutdown("process_exit_clear_functools_runtime_state");
    functools_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_operator_runtime_state");
    operator_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_python_builtin_function_cache");
    python_builtin_functions_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_attributes_runtime_state");
    attributes_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_types_runtime_state");
    types_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_exceptions_runtime_state");
    exceptions_clear_runtime_state(_py, state);
    trace_shutdown("process_exit_clear_resource_state");
    crate::resource::clear_resource_state();
    trace_shutdown("process_exit_done");
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

fn runtime_teardown_inner(_py: &PyToken<'_>, state: &RuntimeState, reset_ptrs: bool) {
    crate::gil_assert();
    trace_shutdown("start");
    trace_shutdown("finish_pending_calls");
    finish_pending_calls_for_teardown(_py);
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
    #[cfg(feature = "stdlib_asyncio")]
    {
        trace_shutdown("clear_asyncio_queue_state");
        asyncio_queue_clear_state(_py);
    }
    trace_shutdown("clear_thread_exception");
    clear_thread_exception_for_teardown(_py);
    trace_shutdown("run_atexit_callbacks");
    crate::builtins::atexit::atexit_run_exitfuncs_teardown(_py);
    trace_shutdown("clear_weakref_runtime_state");
    trace_shutdown("clear_signal_state");
    signal_clear_state(_py, state);
    trace_shutdown("clear_contextvars_state");
    contextvars_clear_state(_py, state);
    trace_shutdown("clear_copy_memo_state");
    copy_memo_clear_state(_py, state);
    trace_shutdown("clear_sys_ext_state");
    sys_ext_clear_state(_py, state);
    trace_shutdown("flush_stdio");
    flush_stdio_handles(_py, state);
    trace_shutdown("flush_stdio_post_finalizers");
    flush_stdio_handles(_py, state);
    trace_shutdown("clear_c_api_module_state");
    c_api_module_clear_state(_py, state);
    trace_shutdown("clear_runtime_extension_states");
    runtime_extension_states_clear_and_drop(state);
    // Break class graph cycles while bases, MROs, dicts, interned names, and
    // type caches are still valid. The anchor refs remain live until the final
    // release phase after all consumers have drained.
    trace_shutdown("builtin_classes_break_cycles");
    builtin_classes_break_cycles(_py, state);
    trace_shutdown("clear_module_cache");
    clear_module_cache(_py, state);
    trace_shutdown("clear_modules_runtime_state");
    modules_clear_runtime_state(_py, state);
    trace_shutdown("clear_platform_runtime_state");
    platform_clear_runtime_state(_py, state);
    trace_shutdown("flush_stdio_post_modules");
    flush_stdio_handles(_py, state);
    trace_shutdown("clear_io_runtime_state");
    io_clear_runtime_state(_py, state);
    trace_shutdown("clear_exception_type_cache");
    clear_exception_type_cache(_py, state);
    trace_shutdown("clear_exceptions_runtime_state");
    exceptions_clear_runtime_state(_py, state);
    trace_shutdown("clear_gen_locals");
    clear_gen_locals(_py, state);
    trace_shutdown("clear_dict_subclass_storage");
    clear_dict_subclass_storage(_py, state);
    trace_shutdown("clear_interned_names");
    clear_interned_names(_py, state);
    trace_shutdown("clear_method_cache");
    clear_method_cache(_py, state);
    trace_shutdown("clear_runtime_static_names");
    clear_runtime_static_names(_py, state);
    trace_shutdown("clear_python_builtin_function_cache");
    python_builtin_functions_clear_runtime_state(_py, state);
    trace_shutdown("clear_call_bind_ic_cache");
    clear_call_bind_ic_cache();
    trace_shutdown("clear_method_ic_cache");
    clear_method_ic_cache(_py);
    trace_shutdown("clear_super_ic_cache");
    clear_super_ic_cache(_py);
    trace_shutdown("clear_attributes_runtime_state");
    attributes_clear_runtime_state(_py, state);
    trace_shutdown("clear_special_cache");
    clear_special_cache(_py, state);
    trace_shutdown("clear_utf8_caches");
    clear_utf8_caches(state);
    trace_shutdown("clear_code_slots");
    clear_code_slots(_py, state);
    trace_shutdown("clear_asyncgen_registry");
    clear_asyncgen_registry(state);
    trace_shutdown("clear_asyncgen_hooks");
    clear_asyncgen_hooks(_py, state);
    trace_shutdown("clear_asyncgen_locals");
    clear_asyncgen_locals(_py, state);
    trace_shutdown("clear_thread_local_state");
    clear_thread_local_state(_py);
    // Code objects in the fn-ptr map own co_filename/co_name/co_varnames/co_names
    // references that may point at interned/builder singletons. Release them
    // before clearing those singleton pools, or code teardown can walk freed
    // metadata during process shutdown.
    trace_shutdown("clear_fn_ptr_code_map");
    clear_fn_ptr_code_map(_py, state);
    trace_shutdown("clear_functools_runtime_state");
    functools_clear_runtime_state(_py, state);
    trace_shutdown("clear_operator_runtime_state");
    operator_clear_runtime_state(_py, state);
    trace_shutdown("clear_types_runtime_state");
    types_clear_runtime_state(_py, state);
    trace_shutdown("clear_builder_singletons");
    clear_builder_singletons(_py, state);
    // Keep builtin classes alive until after cache + TLS teardown: releasing
    // them too early can trigger lock re-entry when later dec_ref paths perform
    // class attribute lookups during shutdown.
    trace_shutdown("builtin_classes_shutdown");
    builtin_classes_shutdown(_py, state);
    if reset_ptrs {
        trace_shutdown("reset_ptr_registry");
        reset_ptr_registry();
        trace_shutdown("reset_gc_registry");
        crate::object::gc::gc_reset_registry();
    }
    trace_shutdown("clear_resource_state");
    crate::resource::clear_resource_state();
    trace_shutdown("done");
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

fn clear_asyncgen_hooks(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let mut guard = state.asyncgen_hooks.lock().unwrap();
    if guard.firstiter != 0 {
        dec_ref_bits(_py, guard.firstiter);
    }
    if guard.finalizer != 0 {
        dec_ref_bits(_py, guard.finalizer);
    }
    guard.firstiter = MoltObject::none().bits();
    guard.finalizer = MoltObject::none().bits();
}

fn clear_asyncgen_locals(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let mut guard = state.asyncgen_locals.lock().unwrap();
    for (_, entry) in guard.drain() {
        for bits in entry.names {
            if bits != 0 {
                dec_ref_bits(_py, bits);
            }
        }
    }
}

fn clear_gen_locals(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let mut guard = state.gen_locals.lock().unwrap();
    for (_, entry) in guard.drain() {
        for bits in entry.names {
            if bits != 0 {
                dec_ref_bits(_py, bits);
            }
        }
    }
}

fn clear_dict_subclass_storage(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let drained: Vec<u64> = {
        let mut guard = state.dict_subclass_storage.lock().unwrap();
        guard.drain().map(|(_, bits)| bits).collect()
    };
    for bits in drained {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

fn clear_fn_ptr_code_map(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let drained: Vec<u64> = {
        let mut guard = state.fn_ptr_code.lock().unwrap();
        guard.drain().map(|(_key, bits)| bits).collect()
    };
    for bits in drained {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

fn clear_async_hang_probe(state: &RuntimeState) {
    if let Some(Some(probe)) = state.async_hang_probe.get()
        && let Ok(mut guard) = probe.pending_counts.lock()
    {
        guard.clear();
    }
}

fn clear_thread_local_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    clear_thread_exception_for_teardown(_py);
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    let _ = CONTEXT_STACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let old = std::mem::take(&mut *stack);
        for bits in old {
            dec_ref_bits(_py, bits);
        }
    });
    let _ = FRAME_STACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let old = std::mem::take(&mut *stack);
        for entry in old {
            if entry.code_bits != 0 {
                dec_ref_bits(_py, entry.code_bits);
            }
            if entry.locals_bits != 0 && !obj_from_bits(entry.locals_bits).is_none() {
                dec_ref_bits(_py, entry.locals_bits);
            }
            if entry.globals_bits != 0 && !obj_from_bits(entry.globals_bits).is_none() {
                dec_ref_bits(_py, entry.globals_bits);
            }
            if entry.builtins_bits != 0 && !obj_from_bits(entry.builtins_bits).is_none() {
                dec_ref_bits(_py, entry.builtins_bits);
            }
        }
    });
    let _ = TRACE_FRAME_PUSH_STACK.try_with(|stack| {
        let _ = std::mem::take(&mut *stack.borrow_mut());
    });
    let _ = ACTIVE_EXCEPTION_STACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let old = std::mem::take(&mut *stack);
        for bits in old {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
    });
    let _ = ACTIVE_EXCEPTION_FALLBACK.try_with(|stack| {
        let mut stack = stack.borrow_mut();
        let _ = std::mem::take(&mut *stack);
    });
    let _ = GENERATOR_EXCEPTION_STACKS.try_with(|map| {
        let mut map = map.borrow_mut();
        let old = std::mem::take(&mut *map);
        for (_key, stack) in old {
            for bits in stack {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
        }
    });
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
    let _ = PARSE_ARENA.try_with(|arena| arena.borrow_mut().clear());
    clear_attr_tls_caches(_py);
    clear_const_data_literal_caches(_py);
    clear_utf8_count_tls();
}

fn clear_code_slots(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let Some(slots) = state.code_slots.get() else {
        return;
    };
    for slot in slots {
        let bits = slot.swap(0, AtomicOrdering::AcqRel);
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

pub(crate) fn clear_worker_thread_state(_py: &PyToken<'_>) {
    crate::gil_assert();
    clear_thread_local_state(_py);
}

fn clear_task_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    state.event_loop_registry.clear(_py);
    state.pipe_transport_registry.clear();
    #[cfg(feature = "stdlib_asyncio")]
    asyncio_core_clear_state(_py);
    clear_await_graph_state(_py, state);
    clear_native_task_states(_py, state);
    let stacks = {
        let mut guard = state.task_exception_stacks.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for stack in stacks {
        for bits in stack {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
    }
    {
        let mut guard = state.task_exception_handler_stacks.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    {
        let mut guard = state.task_exception_depths.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    {
        let mut guard = state.task_exception_baselines.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    let pointers = {
        let mut guard = state.task_last_exceptions.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().map(|ptr| ptr.0).collect::<Vec<_>>()
    };
    let _ = CURRENT_EXCEPTION_PENDING.try_with(|pending| pending.set(false));
    for ptr in pointers {
        let bits = MoltObject::from_ptr(ptr).bits();
        dec_ref_bits(_py, bits);
    }
    let result_bits = {
        let mut guard = state.task_results.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in result_bits {
        dec_ref_bits(_py, bits);
    }
    let cancel_bits = {
        let mut guard = state.task_cancel_messages.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in cancel_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    {
        let mut guard = state.task_tokens.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    {
        let mut guard = state.task_tokens_by_id.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    {
        let mut guard = state.cancel_tokens.lock().unwrap();
        *guard = default_cancel_tokens();
    }
    let running_loop_bits = {
        let mut guard = state.asyncio_running_loops.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in running_loop_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    let event_loop_bits = {
        let mut guard = state.asyncio_event_loops.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in event_loop_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    let event_loop_policy_bits = {
        let mut guard = state.asyncio_event_loop_policy.lock().unwrap();
        let bits = *guard;
        *guard = MoltObject::none().bits();
        bits
    };
    if event_loop_policy_bits != 0 && !obj_from_bits(event_loop_policy_bits).is_none() {
        dec_ref_bits(_py, event_loop_policy_bits);
    }
    let task_bits = {
        let mut guard = state.asyncio_tasks.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in task_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    let current_task_bits = {
        let mut guard = state.asyncio_current_tasks.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in current_task_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    let event_waiter_bits = {
        let mut guard = state.asyncio_event_waiters.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        let mut bits = Vec::new();
        for waiters in old.into_values() {
            bits.extend(waiters);
        }
        bits
    };
    for bits in event_waiter_bits {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    {
        let mut guard = state.asyncio_event_waiter_index.lock().unwrap();
        let _ = std::mem::take(&mut *guard);
    }
    NEXT_CANCEL_TOKEN_ID.store(2, AtomicOrdering::SeqCst);
}

fn clear_await_graph_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let edges = {
        let mut waiting = state.task_waiting_on.lock().unwrap();
        let old = std::mem::take(&mut *waiting);
        state.await_waiters.lock().unwrap().clear();
        state.await_waiter_index.lock().unwrap().clear();
        old
    };
    for (waiter, awaited) in edges {
        unsafe {
            dec_ref_ptr(_py, awaited.0);
            dec_ref_ptr(_py, waiter.0);
        }
    }
}

fn clear_native_task_states(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let thread_tasks = {
            let mut guard = state.thread_tasks.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for task in thread_tasks.into_values() {
            task.cancelled.store(true, AtomicOrdering::Release);
            if let Some(bits) = task.result.lock().unwrap().take() {
                dec_ref_bits(_py, bits);
            }
            if let Some(bits) = task.exception.lock().unwrap().take() {
                dec_ref_bits(_py, bits);
            }
            task.condvar.notify_all();
        }
    }

    let process_tasks = {
        let mut guard = state.process_tasks.lock().unwrap();
        std::mem::take(&mut *guard)
    };
    for (future, task) in process_tasks {
        task.cancelled.store(true, AtomicOrdering::Release);
        let mut wait_future = task.process.wait_future.lock().unwrap();
        if wait_future.map(|slot| slot.0) == Some(future.0) {
            *wait_future = None;
        }
        drop(wait_future);
        #[cfg(not(target_arch = "wasm32"))]
        task.process.condvar.notify_all();
    }
}

fn clear_module_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let modules = {
        let mut guard = state.module_cache.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
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
        crate::object::release_shutdown_bits(_py, bits);
    }
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

/// Drain the heap-backed TLS anchors initialized before `TLS_GUARD`.
///
/// Every other TLS key is initialized after the guard and therefore has
/// already run its destructor when this function executes. Calling `try_with`
/// on those keys here would initialize fresh TLS during TLS destruction,
/// creating a second destructor wave and potentially stranding thread exit.
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

fn clear_interned_names(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.interned.slots();
    clear_atomic_slots(_py, &slots);
}

fn clear_special_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = vec![
        &state.special_cache.open_default_mode,
        &state.special_cache.molt_missing,
        &state.special_cache.molt_not_implemented,
        &state.special_cache.molt_ellipsis,
        &state.special_cache.awaitable_await,
        &state.special_cache.function_code_descriptor,
        &state.special_cache.function_globals_descriptor,
    ];
    clear_atomic_slots(_py, &slots);
}

#[cfg(test)]
mod tests {
    use super::{clear_interned_names, clear_special_cache, clear_worker_thread_state};
    use crate::{MoltObject, alloc_string, runtime_state};
    use std::sync::atomic::Ordering;

    #[test]
    fn clear_worker_thread_state_keeps_gil_for_tls_cleanup() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            clear_worker_thread_state(_py);
        });
    }

    #[test]
    fn clear_special_cache_releases_function_descriptor_slots() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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
