use crate::builtins::attr::clear_attr_tls_caches;
use crate::object::utf8_cache::{
    clear_utf8_count_tls, Utf8CacheStore, Utf8CountCacheStore, UTF8_CACHE_MAX_ENTRIES,
    UTF8_COUNT_CACHE_SHARDS,
};
use crate::PyToken;
use crate::{
    builtin_classes_shutdown, clear_exception_state, clear_exception_type_cache, dec_ref_bits,
    default_cancel_tokens, obj_from_bits, reset_ptr_registry, GilReleaseGuard, MoltObject,
    ACTIVE_EXCEPTION_FALLBACK, ACTIVE_EXCEPTION_STACK, BLOCK_ON_TASK, CONTEXT_STACK,
    CURRENT_TASK, CURRENT_TOKEN, DEFAULT_RECURSION_LIMIT, EXCEPTION_STACK, FRAME_STACK,
    GENERATOR_EXCEPTION_STACKS, GENERATOR_RAISE, GIL_DEPTH, NEXT_CANCEL_TOKEN_ID,
    OBJECT_POOL_BUCKETS, OBJECT_POOL_TLS, PARSE_ARENA, RECURSION_DEPTH, RECURSION_LIMIT,
    TASK_RAISE_ACTIVE,
};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::{cache::clear_atomic_slots, cache::clear_method_cache, RuntimeState};

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
        crate::with_gil_entry!(_py, {
            clear_thread_local_state(_py);
        });
        clear_object_pool_tls();
    }
}

pub(crate) fn touch_tls_guard() {
    GIL_DEPTH.with(|_| {});
    TLS_GUARD.with(|_| {});
}

pub(crate) fn runtime_teardown(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, true);
}

pub(crate) fn runtime_teardown_isolate(_py: &PyToken<'_>, state: &RuntimeState) {
    runtime_teardown_inner(_py, state, false);
}

fn runtime_teardown_inner(_py: &PyToken<'_>, state: &RuntimeState, reset_ptrs: bool) {
    crate::gil_assert();
    let scheduler_started = state.scheduler_started.load(AtomicOrdering::Acquire);
    let sleep_queue_started = state.sleep_queue_started.load(AtomicOrdering::Acquire);
    let io_poller_started = state.io_poller_started.load(AtomicOrdering::Acquire);
    #[cfg(not(target_arch = "wasm32"))]
    let thread_pool_started = state.thread_pool_started.load(AtomicOrdering::Acquire);
    #[cfg(target_arch = "wasm32")]
    let thread_pool_started = false;

    if scheduler_started || sleep_queue_started || io_poller_started || thread_pool_started {
        let _release = GilReleaseGuard::new();
        if scheduler_started {
            state.scheduler().shutdown();
        }
        if sleep_queue_started {
            state.sleep_queue().shutdown(_py);
        }
        if io_poller_started {
            state.io_poller().shutdown();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if thread_pool_started {
            if let Some(pool) = state.thread_pool.get() {
                pool.shutdown();
            }
        }
    }
    clear_async_hang_probe(state);
    clear_task_state(_py, state);
    clear_exception_state(_py);
    clear_module_cache(_py, state);
    clear_exception_type_cache(_py, state);
    builtin_classes_shutdown(_py, state);
    clear_interned_names(_py, state);
    clear_method_cache(_py, state);
    clear_special_cache(_py, state);
    clear_utf8_caches(state);
    clear_code_slots(_py, state);
    clear_object_pool(state);
    clear_asyncgen_registry(state);
    clear_asyncgen_hooks(_py, state);
    clear_asyncgen_locals(_py, state);
    clear_fn_ptr_code_map(_py, state);
    if reset_ptrs {
        reset_ptr_registry();
    }
    clear_thread_local_state(_py);
}

pub(crate) fn runtime_reset_for_init(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    PARSE_ARENA.with(|arena| arena.borrow_mut().reset());
    reset_object_pool(state);
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

fn clear_fn_ptr_code_map(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let mut guard = state.fn_ptr_code.lock().unwrap();
    for (_key, bits) in guard.drain() {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

fn clear_async_hang_probe(state: &RuntimeState) {
    if let Some(Some(probe)) = state.async_hang_probe.get() {
        if let Ok(mut guard) = probe.pending_counts.lock() {
            guard.clear();
        }
    }
}

fn clear_thread_local_state(_py: &PyToken<'_>) {
    crate::gil_assert();
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
        }
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
    let _ = CURRENT_TOKEN.try_with(|cell| cell.set(1));
    let _ = PARSE_ARENA.try_with(|arena| arena.borrow_mut().clear());
    clear_attr_tls_caches(_py);
    clear_utf8_count_tls();
    let _ = GIL_DEPTH.try_with(|depth| depth.set(0));
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
    clear_object_pool_tls();
}

fn clear_task_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
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
    for ptr in pointers {
        let bits = MoltObject::from_ptr(ptr).bits();
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
        let mut guard = state.cancel_tokens.lock().unwrap();
        *guard = default_cancel_tokens();
    }
    NEXT_CANCEL_TOKEN_ID.store(2, AtomicOrdering::SeqCst);
}

fn clear_module_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let modules = {
        let mut guard = state.module_cache.lock().unwrap();
        let old = std::mem::take(&mut *guard);
        old.into_values().collect::<Vec<_>>()
    };
    for bits in modules {
        dec_ref_bits(_py, bits);
    }
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

fn clear_object_pool_tls() {
    let _ = OBJECT_POOL_TLS.try_with(|pool| {
        let mut pool = pool.borrow_mut();
        for (idx, bucket) in pool.iter_mut().enumerate() {
            let size = idx * 8;
            if size == 0 {
                bucket.clear();
                continue;
            }
            let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
            for slot in bucket.drain(..) {
                unsafe {
                    std::alloc::dealloc(slot.0, layout);
                }
            }
        }
        *pool = Vec::new();
    });
}

fn clear_object_pool(state: &RuntimeState) {
    let mut guard = state.object_pool.lock().unwrap();
    for (idx, bucket) in guard.iter_mut().enumerate() {
        let size = idx * 8;
        if size == 0 {
            bucket.clear();
            continue;
        }
        let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
        for slot in bucket.drain(..) {
            unsafe {
                std::alloc::dealloc(slot.0, layout);
            }
        }
    }
    clear_object_pool_tls();
    *guard = Vec::new();
}

fn reset_object_pool(state: &RuntimeState) {
    let mut guard = state.object_pool.lock().unwrap();
    if guard.len() != OBJECT_POOL_BUCKETS {
        *guard = vec![Vec::new(); OBJECT_POOL_BUCKETS];
    } else {
        for bucket in guard.iter_mut() {
            bucket.clear();
        }
    }
    OBJECT_POOL_TLS.with(|pool| {
        let mut pool = pool.borrow_mut();
        *pool = vec![Vec::new(); OBJECT_POOL_BUCKETS];
    });
}

fn clear_interned_names(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = interned_name_slots(state);
    clear_atomic_slots(_py, &slots);
}

fn interned_name_slots(state: &RuntimeState) -> Vec<&AtomicU64> {
    vec![
        &state.interned.bases_name,
        &state.interned.mro_name,
        &state.interned.get_name,
        &state.interned.set_name,
        &state.interned.delete_name,
        &state.interned.set_name_method,
        &state.interned.getattr_name,
        &state.interned.getattribute_name,
        &state.interned.call_name,
        &state.interned.await_name,
        &state.interned.init_name,
        &state.interned.init_subclass_name,
        &state.interned.new_name,
        &state.interned.instancecheck_name,
        &state.interned.subclasscheck_name,
        &state.interned.enter_name,
        &state.interned.exit_name,
        &state.interned.setattr_name,
        &state.interned.delattr_name,
        &state.interned.write_name,
        &state.interned.flush_name,
        &state.interned.sys_name,
        &state.interned.sys_version_info,
        &state.interned.sys_version,
        &state.interned.stdout_name,
        &state.interned.modules_name,
        &state.interned.all_name,
        &state.interned.fspath_name,
        &state.interned.dict_name,
        &state.interned.slots_name,
        &state.interned.weakref_name,
        &state.interned.molt_dict_data_name,
        &state.interned.class_name,
        &state.interned.annotations_name,
        &state.interned.annotate_name,
        &state.interned.field_offsets_name,
        &state.interned.molt_layout_size,
        &state.interned.float_name,
        &state.interned.index_name,
        &state.interned.int_name,
        &state.interned.round_name,
        &state.interned.trunc_name,
        &state.interned.repr_name,
        &state.interned.str_name,
        &state.interned.format_name,
        &state.interned.qualname_name,
        &state.interned.name_name,
        &state.interned.obj_name,
        &state.interned.f_lasti_name,
        &state.interned.f_code_name,
        &state.interned.f_lineno_name,
        &state.interned.tb_frame_name,
        &state.interned.tb_lineno_name,
        &state.interned.tb_next_name,
        &state.interned.molt_arg_names,
        &state.interned.molt_posonly,
        &state.interned.molt_kwonly_names,
        &state.interned.molt_vararg,
        &state.interned.molt_varkw,
        &state.interned.molt_closure_size,
        &state.interned.molt_is_coroutine,
        &state.interned.molt_is_generator,
        &state.interned.molt_bind_kind,
        &state.interned.defaults_name,
        &state.interned.kwdefaults_name,
        &state.interned.lt_name,
        &state.interned.le_name,
        &state.interned.gt_name,
        &state.interned.ge_name,
        &state.interned.eq_name,
        &state.interned.ne_name,
        &state.interned.add_name,
        &state.interned.radd_name,
        &state.interned.mul_name,
        &state.interned.rmul_name,
        &state.interned.sub_name,
        &state.interned.rsub_name,
        &state.interned.truediv_name,
        &state.interned.rtruediv_name,
        &state.interned.floordiv_name,
        &state.interned.rfloordiv_name,
        &state.interned.or_name,
        &state.interned.ror_name,
        &state.interned.and_name,
        &state.interned.rand_name,
        &state.interned.xor_name,
        &state.interned.rxor_name,
        &state.interned.iadd_name,
        &state.interned.isub_name,
        &state.interned.ior_name,
        &state.interned.iand_name,
        &state.interned.ixor_name,
    ]
}

fn clear_special_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = vec![
        &state.special_cache.open_default_mode,
        &state.special_cache.molt_missing,
        &state.special_cache.molt_not_implemented,
        &state.special_cache.molt_ellipsis,
        &state.special_cache.awaitable_await,
    ];
    clear_atomic_slots(_py, &slots);
}

#[cfg(test)]
mod tests {
    use super::clear_worker_thread_state;

    #[test]
    fn clear_worker_thread_state_keeps_gil_for_tls_cleanup() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            clear_worker_thread_state(_py);
        });
    }
}
