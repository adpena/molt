use crate::PyToken;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_deque::{Injector, Worker};

use crate::object::ops::string_obj_to_owned;
use crate::object::{dec_ref_ptr, inc_ref_ptr};
use crate::state::clear_worker_thread_state;
use crate::{
    alloc_list, alloc_string, alloc_tuple, anext_default_poll_fn_addr, async_sleep_poll_fn_addr,
    asyncgen_poll_fn_addr, bits_from_ptr, call_callable0, call_poll_fn, class_name_for_error,
    code_filename_bits, code_name_bits, context_stack_unwind, dec_ref_bits, dict_clear_in_place,
    dict_del_in_place, dict_get_in_place, dict_set_in_place, exception_context_align_depth,
    exception_context_fallback_pop, exception_context_fallback_push, exception_handler_active,
    exception_kind_bits, exception_pending, exception_stack_baseline_get,
    exception_stack_baseline_set, exception_stack_depth, exception_stack_set_depth,
    format_exception_with_traceback, generator_raise_active, handle_system_exit,
    header_from_obj_ptr, inc_ref_bits, io_wait_poll_fn_addr, is_missing_bits, is_truthy,
    maybe_ptr_from_bits, missing_bits, molt_exception_last, molt_getattr_builtin, molt_set_add,
    molt_set_new, obj_from_bits, object_class_bits, object_type_id, pending_bits_i64,
    process_poll_fn_addr, profile_hit, promise_poll_fn_addr, ptr_from_bits, raise_exception,
    record_exception, resolve_task_ptr, runtime_state, seq_vec_ref, set_task_raise_active,
    task_exception_baseline_store, task_exception_baseline_take, task_exception_depth_store,
    task_exception_depth_take, task_exception_handler_stack_store,
    task_exception_handler_stack_take, task_exception_stack_store, task_exception_stack_take,
    task_raise_active, thread_poll_fn_addr, to_i64, with_gil, GilGuard, GilReleaseGuard,
    MoltHeader, MoltObject, PtrSlot, ACTIVE_EXCEPTION_STACK, ASYNC_PENDING_COUNT, ASYNC_POLL_COUNT,
    ASYNC_SLEEP_REGISTER_COUNT, ASYNC_WAKEUP_COUNT, EXCEPTION_STACK, GIL_DEPTH,
    HEADER_FLAG_BLOCK_ON, HEADER_FLAG_SPAWN_RETAIN, HEADER_FLAG_TASK_DONE, HEADER_FLAG_TASK_QUEUED,
    HEADER_FLAG_TASK_RUNNING, HEADER_FLAG_TASK_WAKE_PENDING, TYPE_ID_DICT, TYPE_ID_LIST,
    TYPE_ID_TUPLE,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::IoPoller;
use crate::ProcessTaskState;
#[cfg(not(target_arch = "wasm32"))]
use crate::ThreadTaskState;

use super::cancellation::{
    cancel_tokens, clear_task_token, current_token_id, ensure_task_token,
    raise_cancelled_with_message, set_current_token, task_cancel_pending, task_take_cancel_pending,
};
use super::channels::has_capability;
use super::{spawned_task_count, spawned_task_inc};

// --- Scheduler ---

#[inline]
fn debug_current_task() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1"))
}

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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn async_worker_threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        let max_threads = num_cpus::get().max(1);
        let parsed = std::env::var("MOLT_ASYNC_THREADS")
            .ok()
            .and_then(|val| val.trim().parse::<usize>().ok());
        parsed.unwrap_or(0).min(max_threads)
    })
}

thread_local! {
    pub(crate) static CURRENT_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
    pub(crate) static BLOCK_ON_TASK: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
}

fn task_queue_lock() -> &'static Mutex<()> {
    static TASK_QUEUE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    TASK_QUEUE_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn task_exception_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<u64>>> {
    &runtime_state(_py).task_exception_stacks
}

pub(crate) fn task_exception_handler_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<usize>>> {
    &runtime_state(_py).task_exception_handler_stacks
}

pub(crate) fn await_waiters(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, Vec<PtrSlot>>> {
    &runtime_state(_py).await_waiters
}

pub(crate) fn task_waiting_on(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, PtrSlot>> {
    &runtime_state(_py).task_waiting_on
}

pub(crate) fn asyncgen_registry(_py: &PyToken<'_>) -> &'static Mutex<HashSet<PtrSlot>> {
    &runtime_state(_py).asyncgen_registry
}

pub(crate) fn fn_ptr_code_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).fn_ptr_code
}

pub(crate) fn asyncio_running_loop_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_running_loops
}

pub(crate) fn asyncio_event_loop_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_event_loops
}

pub(crate) fn asyncio_task_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<u64, u64>> {
    &runtime_state(_py).asyncio_tasks
}

pub(crate) fn asyncio_event_waiters_map(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<u64, Vec<u64>>> {
    &runtime_state(_py).asyncio_event_waiters
}

fn asyncio_parse_token_id(_py: &PyToken<'_>, token_bits: u64) -> Result<u64, u64> {
    let Some(token_id) = to_i64(obj_from_bits(token_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "token_id must be int",
        ));
    };
    if token_id < 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "token_id must be >= 0",
        ));
    }
    Ok(token_id as u64)
}

fn asyncio_running_loop_get_impl(_py: &PyToken<'_>) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let guard = asyncio_running_loop_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&tid).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_running_loop_set_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let mut guard = asyncio_running_loop_map(_py).lock().unwrap();
    if obj_from_bits(loop_bits).is_none() {
        if let Some(old_bits) = guard.remove(&tid) {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
        return MoltObject::none().bits();
    }

    let old_bits = guard.insert(tid, loop_bits);
    if old_bits != Some(loop_bits) {
        inc_ref_bits(_py, loop_bits);
        if let Some(old_bits) = old_bits {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
    }
    MoltObject::none().bits()
}

fn asyncio_event_loop_get_impl(_py: &PyToken<'_>) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let guard = asyncio_event_loop_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&tid).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_event_loop_get_current_impl(_py: &PyToken<'_>) -> u64 {
    let bits = asyncio_event_loop_get_impl(_py);
    if !obj_from_bits(bits).is_none() {
        return bits;
    }
    raise_exception(
        _py,
        "RuntimeError",
        "There is no current event loop in thread 'MainThread'.",
    )
}

fn asyncio_event_loop_set_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let tid = crate::concurrency::current_thread_id();
    let mut guard = asyncio_event_loop_map(_py).lock().unwrap();
    if obj_from_bits(loop_bits).is_none() {
        if let Some(old_bits) = guard.remove(&tid) {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
        return MoltObject::none().bits();
    }

    let old_bits = guard.insert(tid, loop_bits);
    if old_bits != Some(loop_bits) {
        inc_ref_bits(_py, loop_bits);
        if let Some(old_bits) = old_bits {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
    }
    MoltObject::none().bits()
}

fn asyncio_event_loop_policy_get_impl(_py: &PyToken<'_>) -> u64 {
    let bits = *runtime_state(_py).asyncio_event_loop_policy.lock().unwrap();
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_event_loop_policy_set_impl(_py: &PyToken<'_>, policy_bits: u64) -> u64 {
    let mut guard = runtime_state(_py).asyncio_event_loop_policy.lock().unwrap();
    let old_bits = *guard;
    *guard = policy_bits;
    if policy_bits != 0 && !obj_from_bits(policy_bits).is_none() {
        inc_ref_bits(_py, policy_bits);
    }
    if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
        dec_ref_bits(_py, old_bits);
    }
    MoltObject::none().bits()
}

fn asyncio_task_registry_set_impl(_py: &PyToken<'_>, token_bits: u64, task_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    if obj_from_bits(task_bits).is_none() {
        if let Some(old_bits) = guard.remove(&token_id) {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
        return MoltObject::none().bits();
    }
    let old_bits = guard.insert(token_id, task_bits);
    if old_bits != Some(task_bits) {
        inc_ref_bits(_py, task_bits);
        if let Some(old_bits) = old_bits {
            if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
                dec_ref_bits(_py, old_bits);
            }
        }
    }
    MoltObject::none().bits()
}

fn asyncio_task_registry_get_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let guard = asyncio_task_map(_py).lock().unwrap();
    let Some(bits) = guard.get(&token_id).copied() else {
        return MoltObject::none().bits();
    };
    if bits != 0 && !obj_from_bits(bits).is_none() {
        inc_ref_bits(_py, bits);
    }
    bits
}

fn asyncio_task_registry_contains_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let guard = asyncio_task_map(_py).lock().unwrap();
    MoltObject::from_bool(guard.contains_key(&token_id)).bits()
}

fn asyncio_task_registry_current_impl(_py: &PyToken<'_>) -> u64 {
    let token_id = current_token_id();
    {
        let guard = asyncio_task_map(_py).lock().unwrap();
        if let Some(bits) = guard.get(&token_id).copied() {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
    }
    let task_ptr = current_task_ptr();
    if task_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let task_bits = MoltObject::from_ptr(task_ptr).bits();
    inc_ref_bits(_py, task_bits);
    task_bits
}

fn asyncio_task_registry_current_for_loop_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let task_bits = asyncio_task_registry_current_impl(_py);
    if obj_from_bits(task_bits).is_none() || obj_from_bits(loop_bits).is_none() {
        return task_bits;
    }
    let loop_name_ptr = alloc_string(_py, b"_loop");
    if loop_name_ptr.is_null() {
        dec_ref_bits(_py, task_bits);
        return MoltObject::none().bits();
    }
    let loop_name_bits = MoltObject::from_ptr(loop_name_ptr).bits();
    let missing = missing_bits(_py);
    let loop_attr_bits = molt_getattr_builtin(task_bits, loop_name_bits, missing);
    dec_ref_bits(_py, loop_name_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, task_bits);
        return MoltObject::none().bits();
    }
    let matches = !is_missing_bits(_py, loop_attr_bits) && loop_attr_bits == loop_bits;
    if !obj_from_bits(loop_attr_bits).is_none() {
        dec_ref_bits(_py, loop_attr_bits);
    }
    if matches {
        task_bits
    } else {
        dec_ref_bits(_py, task_bits);
        MoltObject::none().bits()
    }
}

fn asyncio_task_registry_pop_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    guard
        .remove(&token_id)
        .unwrap_or_else(|| MoltObject::none().bits())
}

fn asyncio_task_registry_move_impl(
    _py: &PyToken<'_>,
    old_token_bits: u64,
    new_token_bits: u64,
) -> u64 {
    let old_token = match asyncio_parse_token_id(_py, old_token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let new_token = match asyncio_parse_token_id(_py, new_token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    if old_token == new_token {
        return MoltObject::from_bool(false).bits();
    }
    let mut guard = asyncio_task_map(_py).lock().unwrap();
    let Some(old_bits) = guard.remove(&old_token) else {
        return MoltObject::from_bool(false).bits();
    };
    if let Some(replaced_bits) = guard.insert(new_token, old_bits) {
        if replaced_bits != 0 && !obj_from_bits(replaced_bits).is_none() {
            dec_ref_bits(_py, replaced_bits);
        }
    }
    MoltObject::from_bool(true).bits()
}

fn asyncio_task_registry_values_impl(_py: &PyToken<'_>) -> u64 {
    let guard = asyncio_task_map(_py).lock().unwrap();
    let values = guard.values().copied().collect::<Vec<_>>();
    drop(guard);
    let ptr = alloc_list(_py, values.as_slice());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    bits_from_ptr(ptr)
}

fn asyncio_event_waiters_register_impl(
    _py: &PyToken<'_>,
    token_bits: u64,
    waiter_bits: u64,
) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    if obj_from_bits(waiter_bits).is_none() {
        return MoltObject::none().bits();
    }
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let waiters = guard.entry(token_id).or_default();
    waiters.push(waiter_bits);
    inc_ref_bits(_py, waiter_bits);
    MoltObject::none().bits()
}

fn asyncio_event_waiters_unregister_impl(
    _py: &PyToken<'_>,
    token_bits: u64,
    waiter_bits: u64,
) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let Some(waiters) = guard.get_mut(&token_id) else {
        return MoltObject::from_bool(false).bits();
    };
    let Some(idx) = waiters.iter().position(|bits| *bits == waiter_bits) else {
        return MoltObject::from_bool(false).bits();
    };
    let removed = waiters.remove(idx);
    if removed != 0 && !obj_from_bits(removed).is_none() {
        dec_ref_bits(_py, removed);
    }
    if waiters.is_empty() {
        guard.remove(&token_id);
    }
    MoltObject::from_bool(true).bits()
}

fn asyncio_event_waiters_cleanup_token_impl(_py: &PyToken<'_>, token_bits: u64) -> u64 {
    let token_id = match asyncio_parse_token_id(_py, token_bits) {
        Ok(id) => id,
        Err(bits) => return bits,
    };
    let mut guard = asyncio_event_waiters_map(_py).lock().unwrap();
    let Some(waiters) = guard.remove(&token_id) else {
        return MoltObject::from_int(0).bits();
    };
    drop(guard);
    let list_ptr = alloc_list(_py, waiters.as_slice());
    if list_ptr.is_null() {
        for bits in waiters {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        return MoltObject::none().bits();
    }
    let list_bits = bits_from_ptr(list_ptr);
    let out_bits = unsafe { crate::molt_asyncio_event_waiters_cleanup(list_bits) };
    dec_ref_bits(_py, list_bits);
    for bits in waiters {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    out_bits
}

fn asyncio_child_watcher_dict_ptr(_py: &PyToken<'_>, callbacks_bits: u64) -> Result<*mut u8, u64> {
    let Some(ptr) = obj_from_bits(callbacks_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "child watcher callbacks must be dict",
        ));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "child watcher callbacks must be dict",
        ));
    }
    Ok(ptr)
}

fn asyncio_child_watcher_pid(_py: &PyToken<'_>, pid_bits: u64) -> Result<i64, u64> {
    let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
        return Err(raise_exception::<u64>(_py, "TypeError", "pid must be int"));
    };
    Ok(pid)
}

fn asyncio_child_watcher_args_tuple_bits(_py: &PyToken<'_>, args_bits: u64) -> Result<u64, u64> {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "args must be tuple or list",
        ));
    };
    let type_id = unsafe { object_type_id(args_ptr) };
    if type_id == TYPE_ID_TUPLE {
        inc_ref_bits(_py, args_bits);
        return Ok(args_bits);
    }
    if type_id == TYPE_ID_LIST {
        let elems = unsafe { seq_vec_ref(args_ptr) };
        let tuple_ptr = alloc_tuple(_py, elems.as_slice());
        if tuple_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        return Ok(bits_from_ptr(tuple_ptr));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "args must be tuple or list",
    ))
}

fn asyncio_child_watcher_add_impl(
    _py: &PyToken<'_>,
    callbacks_bits: u64,
    pid_bits: u64,
    callback_bits: u64,
    args_bits: u64,
) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let args_tuple_bits = match asyncio_child_watcher_args_tuple_bits(_py, args_bits) {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let entry_ptr = alloc_tuple(_py, &[callback_bits, args_tuple_bits]);
    if entry_ptr.is_null() {
        dec_ref_bits(_py, args_tuple_bits);
        return MoltObject::none().bits();
    }
    let entry_bits = bits_from_ptr(entry_ptr);
    unsafe {
        dict_set_in_place(_py, callbacks_ptr, pid_key_bits, entry_bits);
    }
    dec_ref_bits(_py, pid_key_bits);
    dec_ref_bits(_py, entry_bits);
    dec_ref_bits(_py, args_tuple_bits);
    MoltObject::none().bits()
}

fn asyncio_child_watcher_remove_impl(_py: &PyToken<'_>, callbacks_bits: u64, pid_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let removed = unsafe { dict_del_in_place(_py, callbacks_ptr, pid_key_bits) };
    dec_ref_bits(_py, pid_key_bits);
    MoltObject::from_bool(removed).bits()
}

fn asyncio_child_watcher_clear_impl(_py: &PyToken<'_>, callbacks_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    unsafe {
        dict_clear_in_place(_py, callbacks_ptr);
    }
    MoltObject::none().bits()
}

fn asyncio_child_watcher_pop_impl(_py: &PyToken<'_>, callbacks_bits: u64, pid_bits: u64) -> u64 {
    let callbacks_ptr = match asyncio_child_watcher_dict_ptr(_py, callbacks_bits) {
        Ok(ptr) => ptr,
        Err(bits) => return bits,
    };
    let pid = match asyncio_child_watcher_pid(_py, pid_bits) {
        Ok(pid) => pid,
        Err(bits) => return bits,
    };
    let pid_key_bits = MoltObject::from_int(pid).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, callbacks_ptr, pid_key_bits) };
    let out_bits = if let Some(bits) = entry_bits {
        inc_ref_bits(_py, bits);
        unsafe {
            dict_del_in_place(_py, callbacks_ptr, pid_key_bits);
        }
        bits
    } else {
        MoltObject::none().bits()
    };
    dec_ref_bits(_py, pid_key_bits);
    out_bits
}

fn asyncio_task_registry_live_values_impl(
    _py: &PyToken<'_>,
    loop_bits: u64,
) -> Result<Vec<u64>, u64> {
    let target_loop = if obj_from_bits(loop_bits).is_none() {
        None
    } else {
        Some(loop_bits)
    };
    let values: Vec<u64> = {
        let guard = asyncio_task_map(_py).lock().unwrap();
        guard.values().copied().collect()
    };
    let done_name_ptr = alloc_string(_py, b"done");
    if done_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let done_name_bits = MoltObject::from_ptr(done_name_ptr).bits();
    let loop_name_ptr = alloc_string(_py, b"_loop");
    if loop_name_ptr.is_null() {
        dec_ref_bits(_py, done_name_bits);
        return Err(MoltObject::none().bits());
    }
    let loop_name_bits = MoltObject::from_ptr(loop_name_ptr).bits();
    let missing = missing_bits(_py);
    let mut out_bits: Vec<u64> = Vec::new();

    for task_bits in values {
        if task_bits == 0 || obj_from_bits(task_bits).is_none() {
            continue;
        }
        if let Some(loop_filter) = target_loop {
            let loop_attr_bits = molt_getattr_builtin(task_bits, loop_name_bits, missing);
            if exception_pending(_py) {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, done_name_bits);
                dec_ref_bits(_py, loop_name_bits);
                return Err(MoltObject::none().bits());
            }
            let matches = !is_missing_bits(_py, loop_attr_bits) && loop_attr_bits == loop_filter;
            if !obj_from_bits(loop_attr_bits).is_none() {
                dec_ref_bits(_py, loop_attr_bits);
            }
            if !matches {
                continue;
            }
        }
        let done_method_bits = molt_getattr_builtin(task_bits, done_name_bits, missing);
        if exception_pending(_py) {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, done_name_bits);
            dec_ref_bits(_py, loop_name_bits);
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, done_method_bits) {
            if !obj_from_bits(done_method_bits).is_none() {
                dec_ref_bits(_py, done_method_bits);
            }
            continue;
        }
        let done_bits = unsafe { call_callable0(_py, done_method_bits) };
        dec_ref_bits(_py, done_method_bits);
        if exception_pending(_py) {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, done_name_bits);
            dec_ref_bits(_py, loop_name_bits);
            return Err(MoltObject::none().bits());
        }
        let is_done = is_truthy(_py, obj_from_bits(done_bits));
        if !obj_from_bits(done_bits).is_none() {
            dec_ref_bits(_py, done_bits);
        }
        if !is_done {
            inc_ref_bits(_py, task_bits);
            out_bits.push(task_bits);
        }
    }

    dec_ref_bits(_py, done_name_bits);
    dec_ref_bits(_py, loop_name_bits);
    Ok(out_bits)
}

fn asyncio_task_registry_live_impl(_py: &PyToken<'_>, loop_bits: u64) -> u64 {
    let out_bits = match asyncio_task_registry_live_values_impl(_py, loop_bits) {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let list_ptr = alloc_list(_py, out_bits.as_slice());
    for bits in out_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        bits_from_ptr(list_ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_asyncio_running_loop_get() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_running_loop_get_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_running_loop_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { asyncio_running_loop_set_impl(_py, loop_bits) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_loop_get() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_event_loop_get_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_loop_get_current() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_event_loop_get_current_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_loop_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { asyncio_event_loop_set_impl(_py, loop_bits) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_loop_policy_get() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_event_loop_policy_get_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_loop_policy_set(policy_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_event_loop_policy_set_impl(_py, policy_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_set(token_bits: u64, task_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_task_registry_set_impl(_py, token_bits, task_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_get(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { asyncio_task_registry_get_impl(_py, token_bits) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_contains(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_task_registry_contains_impl(_py, token_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_current() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_task_registry_current_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_current_for_loop(loop_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_task_registry_current_for_loop_impl(_py, loop_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_pop(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { asyncio_task_registry_pop_impl(_py, token_bits) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_move(old_token_bits: u64, new_token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_task_registry_move_impl(_py, old_token_bits, new_token_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_values() -> u64 {
    crate::with_gil_entry!(_py, { asyncio_task_registry_values_impl(_py) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_waiters_register(token_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_event_waiters_register_impl(_py, token_bits, waiter_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_waiters_unregister(token_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_event_waiters_unregister_impl(_py, token_bits, waiter_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_event_waiters_cleanup_token(token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_event_waiters_cleanup_token_impl(_py, token_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_child_watcher_add(
    callbacks_bits: u64,
    pid_bits: u64,
    callback_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_child_watcher_add_impl(_py, callbacks_bits, pid_bits, callback_bits, args_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_child_watcher_remove(callbacks_bits: u64, pid_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_child_watcher_remove_impl(_py, callbacks_bits, pid_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_child_watcher_clear(callbacks_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_child_watcher_clear_impl(_py, callbacks_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_child_watcher_pop(callbacks_bits: u64, pid_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        asyncio_child_watcher_pop_impl(_py, callbacks_bits, pid_bits)
    })
}

fn asyncio_has_any_net_capability(_py: &PyToken<'_>) -> bool {
    has_capability(_py, "net")
        || has_capability(_py, "net.connect")
        || has_capability(_py, "net.listen")
        || has_capability(_py, "net.bind")
}

fn asyncio_has_any_process_capability(_py: &PyToken<'_>) -> bool {
    has_capability(_py, "process") || has_capability(_py, "process.exec")
}

#[no_mangle]
pub extern "C" fn molt_asyncio_require_ssl_transport_support() -> u64 {
    crate::with_gil_entry!(_py, {
        if !asyncio_has_any_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio SSL transport",
            );
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_ssl_transport_orchestrate(
    operation_bits: u64,
    ssl_bits: u64,
    server_hostname_bits: u64,
    server_side_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(operation) = string_obj_to_owned(obj_from_bits(operation_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "asyncio SSL transport operation must be str",
            );
        };
        if operation.is_empty() {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "asyncio SSL transport operation cannot be empty",
            );
        }
        let is_client_operation = matches!(
            operation.as_str(),
            "open_connection"
                | "open_unix_connection"
                | "create_connection"
                | "create_unix_connection"
        );
        let is_server_operation =
            matches!(operation.as_str(), "create_server" | "create_unix_server");
        let is_tls_upgrade = matches!(operation.as_str(), "start_tls");
        if !(is_client_operation || is_server_operation || is_tls_upgrade) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "unsupported asyncio SSL transport operation",
            );
        }
        let bool_true_bits = MoltObject::from_bool(true).bits();
        let bool_false_bits = MoltObject::from_bool(false).bits();
        if obj_from_bits(ssl_bits).is_none() {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "ssl transport requires an ssl context or ssl=True",
            );
        }
        let Some(server_side_raw) = to_i64(obj_from_bits(server_side_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "server_side must be bool");
        };
        if server_side_raw != 0 && server_side_raw != 1 {
            return raise_exception::<u64>(_py, "TypeError", "server_side must be bool");
        }
        let server_side = server_side_raw == 1;
        if is_client_operation && server_side {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "client SSL operations require server_side=False",
            );
        }
        if is_server_operation && !server_side {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "server SSL operations require server_side=True",
            );
        }
        if !obj_from_bits(server_hostname_bits).is_none() {
            let Some(server_hostname) = string_obj_to_owned(obj_from_bits(server_hostname_bits))
            else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "server_hostname must be str or None",
                );
            };
            if server_hostname.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname cannot be an empty string",
                );
            }
            if server_side {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname is only meaningful for client connections",
                );
            }
        }
        if ssl_bits == bool_false_bits {
            if is_tls_upgrade {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "start_tls requires an SSL context",
                );
            }
            if !obj_from_bits(server_hostname_bits).is_none() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname requires an active SSL transport",
                );
            }
            return bool_false_bits;
        }
        if is_client_operation && obj_from_bits(server_hostname_bits).is_none() {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "you have to pass server_hostname when using ssl",
            );
        }
        if !asyncio_has_any_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio SSL transport",
            );
        }
        if is_client_operation {
            if matches!(operation.as_str(), "open_connection" | "create_connection") {
                return bool_true_bits;
            }
            if matches!(
                operation.as_str(),
                "open_unix_connection" | "create_unix_connection"
            ) {
                #[cfg(all(unix, not(target_arch = "wasm32")))]
                {
                    return bool_true_bits;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on wasm",
                    );
                }
                #[cfg(windows)]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on Windows",
                    );
                }
                #[cfg(all(not(unix), not(target_arch = "wasm32"), not(windows)))]
                {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "asyncio SSL unix transport is unavailable on this host",
                    );
                }
            }
            let msg = format!(
                "unsupported asyncio SSL transport operation '{}'",
                operation
            );
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
        if is_server_operation {
            return bool_true_bits;
        }
        if is_tls_upgrade {
            return bool_true_bits;
        }
        let msg = format!(
            "asyncio SSL transport operation '{}' is not yet available in this runtime",
            operation
        );
        raise_exception::<u64>(_py, "RuntimeError", &msg)
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_require_unix_socket_support() -> u64 {
    crate::with_gil_entry!(_py, {
        if !asyncio_has_any_net_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing net capability for asyncio unix sockets",
            );
        }
        #[cfg(target_arch = "wasm32")]
        {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on wasm",
            );
        }
        #[cfg(windows)]
        {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on Windows",
            );
        }
        #[cfg(not(any(unix, windows, target_arch = "wasm32")))]
        {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio unix sockets are unavailable on this host",
            );
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_require_child_watcher_support() -> u64 {
    crate::with_gil_entry!(_py, {
        if !asyncio_has_any_process_capability(_py) {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for asyncio child watchers",
            );
        }
        #[cfg(any(target_arch = "wasm32", windows))]
        {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "asyncio child watchers are unavailable on this host",
            );
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_live(loop_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { asyncio_task_registry_live_impl(_py, loop_bits) })
}

#[no_mangle]
pub extern "C" fn molt_asyncio_task_registry_live_set(loop_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let tasks = match asyncio_task_registry_live_values_impl(_py, loop_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let set_bits = molt_set_new(tasks.len() as u64);
        if obj_from_bits(set_bits).is_none() {
            for task_bits in tasks {
                dec_ref_bits(_py, task_bits);
            }
            return MoltObject::none().bits();
        }
        for &task_bits in &tasks {
            let _ = molt_set_add(set_bits, task_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                for live_task_bits in tasks {
                    dec_ref_bits(_py, live_task_bits);
                }
                return MoltObject::none().bits();
            }
        }
        for task_bits in tasks {
            dec_ref_bits(_py, task_bits);
        }
        set_bits
    })
}

pub(crate) fn fn_ptr_code_set(_py: &PyToken<'_>, fn_ptr: u64, code_bits: u64) {
    crate::gil_assert();
    if fn_ptr == 0 {
        return;
    }
    let mut guard = fn_ptr_code_map(_py).lock().unwrap();
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

#[no_mangle]
pub extern "C" fn molt_fn_ptr_code_set(fn_ptr: u64, code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        fn_ptr_code_set(_py, fn_ptr, code_bits);
        MoltObject::none().bits()
    })
}

pub(crate) fn fn_ptr_code_get(_py: &PyToken<'_>, fn_ptr: u64) -> u64 {
    if fn_ptr == 0 {
        return 0;
    }
    let guard = fn_ptr_code_map(_py).lock().unwrap();
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
    if matches!(
        std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
        Some("1")
    ) {
        let poll_fn = unsafe { (*header_from_obj_ptr(awaited_ptr)).poll_fn };
        if poll_fn == promise_poll_fn_addr() {
            eprintln!(
                "molt async trace: await_register_promise waiter=0x{:x} awaited=0x{:x}",
                waiter_ptr as usize, awaited_ptr as usize
            );
        }
    }
    let waiter_key = PtrSlot(waiter_ptr);
    let awaited_key = PtrSlot(awaited_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    let prev = waiting_map.insert(waiter_key, awaited_key);
    // Keep raw pointers alive while they live in the await graph.
    unsafe {
        if prev.is_none() {
            inc_ref_ptr(_py, waiter_ptr);
        }
        if prev != Some(awaited_key) {
            inc_ref_ptr(_py, awaited_ptr);
        }
    }
    if let Some(prev_key) = prev {
        if prev_key != awaited_key {
            if let Some(waiters) = awaiters_map.get_mut(&prev_key) {
                if let Some(pos) = waiters.iter().position(|val| *val == waiter_key) {
                    waiters.swap_remove(pos);
                }
                if waiters.is_empty() {
                    awaiters_map.remove(&prev_key);
                }
            }
            unsafe {
                dec_ref_ptr(_py, prev_key.0);
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
    unsafe {
        dec_ref_ptr(_py, awaited_key.0);
        dec_ref_ptr(_py, waiter_ptr);
    }
    if matches!(
        std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
        Some("1")
    ) {
        let poll_fn = unsafe { (*header_from_obj_ptr(awaited_key.0)).poll_fn };
        if poll_fn == promise_poll_fn_addr() {
            eprintln!(
                "molt async trace: await_clear_promise waiter=0x{:x} awaited=0x{:x}",
                waiter_ptr as usize, awaited_key.0 as usize
            );
        }
    }
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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
        if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0 {
            return true;
        }
        poll_fn == io_wait_poll_fn_addr()
            || poll_fn == thread_poll_fn_addr()
            || poll_fn == process_poll_fn_addr()
            || poll_fn == promise_poll_fn_addr()
    }
}

pub(crate) fn task_waiting_on_future(_py: &PyToken<'_>, task_ptr: *mut u8) -> Option<*mut u8> {
    if task_ptr.is_null() {
        return None;
    }
    let waiting_map = task_waiting_on(_py).lock().unwrap();
    waiting_map.get(&PtrSlot(task_ptr)).map(|val| val.0)
}

pub(crate) fn task_waiting_on_blocked(_py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    let mut cursor = task_ptr;
    for _ in 0..8 {
        let awaited_ptr = {
            let waiting_map = task_waiting_on(_py).lock().unwrap();
            match waiting_map.get(&PtrSlot(cursor)) {
                Some(val) => val.0,
                None => return false,
            }
        };
        if awaited_ptr.is_null() {
            return false;
        }
        if task_waiting_on_event(_py, awaited_ptr) {
            return true;
        }
        if runtime_state(_py)
            .sleep_queue()
            .is_scheduled(_py, awaited_ptr)
        {
            return true;
        }
        cursor = awaited_ptr;
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) enum BlockOnWaitSpec {
    Io {
        poller: Arc<IoPoller>,
        socket_ptr: *mut u8,
        events: u32,
        timeout: Option<Duration>,
    },
    Thread {
        state: Arc<ThreadTaskState>,
        timeout: Option<Duration>,
    },
    Process {
        state: Arc<ProcessTaskState>,
        timeout: Option<Duration>,
    },
}

const BLOCK_ON_MIN_SLEEP: Duration = Duration::from_micros(50);
const BLOCK_ON_MAX_WAIT: Duration = Duration::from_millis(5);

fn block_on_poll_timeout(timeout: Option<Duration>) -> Duration {
    match timeout {
        Some(val) => val.min(BLOCK_ON_MAX_WAIT),
        None => BLOCK_ON_MAX_WAIT,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) enum BlockOnWaitSpec {}

pub(crate) fn block_on_wait_spec(
    _py: &PyToken<'_>,
    awaited_ptr: *mut u8,
    deadline: Option<Instant>,
) -> Option<BlockOnWaitSpec> {
    if awaited_ptr.is_null() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = deadline;
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[inline]
        fn remaining_timeout(deadline: Option<Instant>) -> Option<Duration> {
            deadline.and_then(|dl| {
                let now = Instant::now();
                if dl > now {
                    Some(dl - now)
                } else {
                    None
                }
            })
        }
        #[inline]
        unsafe fn wait_spec_for_ptr(
            _py: &PyToken<'_>,
            cursor: *mut u8,
            timeout: Option<Duration>,
        ) -> Option<BlockOnWaitSpec> {
            let header = header_from_obj_ptr(cursor);
            let poll_fn = (*header).poll_fn;
            if poll_fn == io_wait_poll_fn_addr() {
                let payload_bytes = (*header)
                    .size
                    .saturating_sub(std::mem::size_of::<MoltHeader>());
                if payload_bytes < 2 * std::mem::size_of::<u64>() {
                    return None;
                }
                let payload_ptr = cursor as *mut u64;
                let socket_bits = *payload_ptr;
                let events_bits = *payload_ptr.add(1);
                let socket_ptr = ptr_from_bits(socket_bits);
                if socket_ptr.is_null() {
                    return None;
                }
                let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
                if events == 0 {
                    return None;
                }
                let poller = Arc::clone(runtime_state(_py).io_poller());
                return Some(BlockOnWaitSpec::Io {
                    poller,
                    socket_ptr,
                    events,
                    timeout,
                });
            }
            if poll_fn == thread_poll_fn_addr() {
                if let Some(state) = thread_task_state(_py, cursor) {
                    return Some(BlockOnWaitSpec::Thread { state, timeout });
                }
            }
            if poll_fn == process_poll_fn_addr() {
                if let Some(state) = process_task_state(_py, cursor) {
                    return Some(BlockOnWaitSpec::Process { state, timeout });
                }
            }
            None
        }
        unsafe {
            let mut cursor = awaited_ptr;
            for _ in 0..8 {
                let timeout = remaining_timeout(deadline);
                if let Some(spec) = wait_spec_for_ptr(_py, cursor, timeout) {
                    return Some(spec);
                }
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
        None
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
        if guard.tasks.contains_key(&PtrSlot(task_ptr)) {
            if async_trace_enabled() {
                eprintln!(
                    "molt async trace: sleep_register_skip task=0x{:x}",
                    task_ptr as usize
                );
            }
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
            let removed = guard.tasks.remove(&PtrSlot(task_ptr));
            if removed.is_some() && async_trace_enabled() {
                eprintln!(
                    "molt async trace: sleep_cancel task=0x{:x}",
                    task_ptr as usize
                );
            }
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

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn next_scheduler_deadline(&self) -> Option<Instant> {
        let _ = self;
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn next_scheduler_deadline(&self) -> Option<Instant> {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return None;
        }
        loop {
            let entry = guard.heap.peek()?;
            let key = entry.task_ptr;
            if guard.tasks.get(&key) != Some(&entry.gen) {
                guard.heap.pop();
                continue;
            }
            return Some(entry.deadline);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_due_scheduler_tasks(&self) -> Vec<*mut u8> {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return Vec::new();
        }
        let now = Instant::now();
        let mut due: Vec<*mut u8> = Vec::new();
        loop {
            let Some(entry) = guard.heap.peek() else {
                break;
            };
            let key = entry.task_ptr;
            if guard.tasks.get(&key) != Some(&entry.gen) {
                guard.heap.pop();
                continue;
            }
            if entry.deadline > now {
                break;
            }
            let entry = guard.heap.pop().expect("heap entry disappeared");
            guard.tasks.remove(&key);
            due.push(entry.task_ptr.0);
        }
        due
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

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn is_scheduled(&self, _py: &PyToken<'_>, _task_ptr: *mut u8) -> bool {
        let _ = _py;
        false
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
    if async_trace_enabled() {
        eprintln!("molt async trace: sleep_worker_start");
    }
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
        enqueue_task_ptr(&py, task_ptr);
    }
}

pub(crate) fn monotonic_now_secs(_py: &PyToken<'_>) -> f64 {
    let nanos = runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .max(1);
    nanos as f64 / 1_000_000_000.0
}

pub(crate) fn monotonic_now_nanos(_py: &PyToken<'_>) -> u128 {
    runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .max(1)
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

    fn execute_task(task: MoltTask, _injector: &Injector<MoltTask>) {
        #[cfg(target_arch = "wasm32")]
        {
            unsafe {
                let task_ptr = task.future_ptr;
                let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
                let poll_fn_addr = (*header).poll_fn;
                {
                    let _guard = task_queue_lock().lock().unwrap();
                    if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                        task_clear_queue_flags(task_ptr);
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
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr);
                        prev
                    });
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
                        exception_context_fallback_pop();
                        clear_task_token(_py, task_ptr);
                        task_mark_done(task_ptr);
                        runtime_state(_py).sleep_queue().cancel_task(_py, task_ptr);
                        let waiters = await_waiters_take(_py, task_ptr);
                        for waiter in waiters {
                            wake_task_ptr(_py, waiter.0);
                        }
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
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
                }
                if poll_fn_addr == 0 && async_trace_enabled() {
                    eprintln!(
                        "molt async trace: poll_skip task=0x{:x} poll=0x0",
                        task_ptr as usize
                    );
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
                {
                    let _guard = task_queue_lock().lock().unwrap();
                    if ((*header).flags & HEADER_FLAG_TASK_DONE) != 0 {
                        task_clear_queue_flags(task_ptr);
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
                    let prev_task = CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(task_ptr);
                        prev
                    });
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
                    exception_context_fallback_pop();
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
                        task_mark_done(task_ptr);
                        runtime_state(_py).sleep_queue().cancel_task(_py, task_ptr);
                        let _ = task_take_wake_pending(task_ptr);
                        let waiters = await_waiters_take(_py, task_ptr);
                        for waiter in waiters {
                            wake_task_ptr(_py, waiter.0);
                        }
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
                    CURRENT_TASK.with(|cell| cell.set(prev_task));
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

pub(crate) fn task_mark_done(task_ptr: *mut u8) {
    if task_ptr.is_null() {
        return;
    }
    let _guard = task_queue_lock().lock().unwrap();
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        (*header).flags |= HEADER_FLAG_TASK_DONE;
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
#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_bits: u64) {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        if async_trace_enabled() {
            let poll_fn =
                (*(task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader)).poll_fn;
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

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_bits: u64) -> i64 {
    let (
        task_ptr,
        poll_fn_addr,
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
                let scheduler_deadline = runtime_state(_py).sleep_queue().next_scheduler_deadline();
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
                        let poll_fn = (*header_from_obj_ptr(ptr)).poll_fn;
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
                                    let cursor_poll = (*header_from_obj_ptr(cursor)).poll_fn;
                                    let cursor_kind = poll_kind(cursor_poll);
                                    eprintln!(
                                        "molt async trace: block_on_chain depth={} ptr=0x{:x} poll=0x{:x} kind={}",
                                        depth,
                                        cursor as usize,
                                        cursor_poll,
                                        cursor_kind
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
                            task_ptr as usize,
                            ptr as usize,
                            poll_fn,
                            kind,
                            detail
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
                let _release = GilReleaseGuard::new();
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
                let _release = GilReleaseGuard::new();
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
                let _release = GilReleaseGuard::new();
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
            task_mark_done(task_ptr);
            let _ = task_take_cancel_pending(task_ptr);
            let _ = task_take_wake_pending(task_ptr);
        }
        break res;
    };

    {
        let _gil = GilGuard::new();
        let _py = _gil.token();
        let _py = &_py;
        let new_depth = exception_stack_depth();
        task_exception_depth_store(_py, task_ptr, new_depth);
        exception_context_align_depth(_py, new_depth);
        let new_baseline = exception_stack_baseline_get();
        task_exception_baseline_store(_py, task_ptr, new_baseline);
        exception_stack_baseline_set(caller_baseline);
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
        // Move any pending exception off the block_on task and onto the caller/global slot.
        let task_exc_slot = {
            let state = runtime_state(_py);
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            let slot = guard.remove(&PtrSlot(task_ptr));
            if guard.is_empty() {
                state
                    .task_last_exception_pending
                    .store(false, AtomicOrdering::Relaxed);
            }
            slot
        };
        let pending_bits = if let Some(exc_slot) = task_exc_slot {
            MoltObject::from_ptr(exc_slot.0).bits()
        } else if exception_pending(_py) {
            molt_exception_last()
        } else {
            MoltObject::none().bits()
        };
        if let Some(exc_ptr) = maybe_ptr_from_bits(pending_bits) {
            let restore_task = CURRENT_TASK.with(|cell| {
                let restore = cell.get();
                if debug_current_task() && prev_task.is_null() && !restore.is_null() {
                    eprintln!(
                        "molt task trace: block_on temp restore null current=0x{:x} task=0x{:x}",
                        restore as usize, task_ptr as usize
                    );
                }
                cell.set(prev_task);
                restore
            });
            record_exception(_py, exc_ptr);
            CURRENT_TASK.with(|cell| cell.set(restore_task));
        }
        if !obj_from_bits(pending_bits).is_none() {
            dec_ref_bits(_py, pending_bits);
        }
        let header = header_from_obj_ptr(task_ptr);
        (*header).flags &= !HEADER_FLAG_BLOCK_ON;
        task_mark_done(task_ptr);
        BLOCK_ON_TASK.with(|cell| cell.set(std::ptr::null_mut()));
        set_task_raise_active(prev_raise);
        set_current_token(_py, prev_token);
        if debug_current_task() && prev_task.is_null() {
            let current = CURRENT_TASK.with(|cell| cell.get());
            if !current.is_null() {
                eprintln!(
                    "molt task trace: block_on restore null current=0x{:x} task=0x{:x}",
                    current as usize, task_ptr as usize
                );
            }
        }
        CURRENT_TASK.with(|cell| cell.set(prev_task));
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
                if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("SystemExit") {
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
