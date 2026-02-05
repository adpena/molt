use crate::PyToken;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use molt_obj_model::MoltObject;

use crate::concurrency::GilGuard;
use crate::object::accessors::resolve_obj_ptr;
use crate::object::HEADER_FLAG_COROUTINE;
use crate::{
    alloc_dict_with_pairs, alloc_exception, alloc_object, alloc_tuple, async_sleep_poll_fn_addr,
    async_trace_enabled, asyncgen_poll_fn_addr, asyncgen_registry, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes, await_waiter_clear, await_waiter_register, await_waiters,
    await_waiters_take, call_callable0, call_callable1, call_poll_fn, class_name_for_error,
    clear_exception, clear_exception_state, current_task_ptr, dec_ref_bits,
    exception_clear_reason_set, exception_context_align_depth, exception_context_fallback_pop,
    exception_context_fallback_push, exception_kind_bits, exception_pending, exception_stack_depth,
    exception_stack_set_depth, fn_ptr_code_get, generator_exception_stack_store,
    generator_exception_stack_take, generator_raise_active, header_from_obj_ptr, inc_ref_bits,
    instant_from_monotonic_secs, io_wait_poll_fn_addr, is_truthy, maybe_ptr_from_bits,
    missing_bits, molt_anext, molt_exception_clear, molt_exception_kind, molt_exception_last,
    molt_exception_set_last, molt_float_from_obj, molt_is_callable, molt_raise, molt_str_from_obj,
    obj_from_bits, object_class_bits, object_mark_has_ptrs, object_type_id, pending_bits_i64,
    process_poll_fn_addr, promise_poll_fn_addr, ptr_from_bits, raise_cancelled_with_message,
    raise_exception, register_task_token, resolve_task_ptr, runtime_state, seq_vec_ref,
    set_generator_raise, string_obj_to_owned, task_cancel_message_clear, task_cancel_message_set,
    task_cancel_pending, task_exception_baseline_drop, task_exception_depth_drop,
    task_exception_stack_drop, task_has_token, task_last_exceptions, task_mark_done,
    task_set_cancel_pending, task_take_cancel_pending, task_waiting_on, thread_poll_fn_addr,
    to_f64, to_i64, token_id_from_bits, type_name, wake_task_ptr, MoltHeader, PtrSlot,
    ACTIVE_EXCEPTION_STACK, ASYNCGEN_CONTROL_SIZE, ASYNCGEN_FIRSTITER_OFFSET, ASYNCGEN_GEN_OFFSET,
    ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT, ASYNCGEN_OP_ASEND, ASYNCGEN_OP_ATHROW,
    ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET, GEN_CLOSED_OFFSET, GEN_CONTROL_SIZE,
    GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET, GEN_YIELD_FROM_OFFSET,
    HEADER_FLAG_BLOCK_ON, HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED,
    HEADER_FLAG_SPAWN_RETAIN, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR,
    TYPE_ID_ASYNC_GENERATOR, TYPE_ID_EXCEPTION, TYPE_ID_GENERATOR, TYPE_ID_OBJECT, TYPE_ID_STRING,
    TYPE_ID_TUPLE,
};

use crate::state::runtime_state::{AsyncGenLocalsEntry, GenLocalsEntry};

#[cfg(not(target_arch = "wasm32"))]
use crate::{is_block_on_task, process_task_state, thread_task_state};

fn promise_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
            Some("1")
        )
    })
}

fn sleep_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| matches!(std::env::var("MOLT_TRACE_SLEEP").ok().as_deref(), Some("1")))
}

fn asyncgen_locals_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_ASYNCGEN_LOCALS_TRACE").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

#[inline]
fn debug_current_task() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1"))
}

const ASYNC_SLEEP_YIELD_SECS: f64 = 0.000_001;
const ASYNC_SLEEP_YIELD_SENTINEL: f64 = -1.0;

unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    ptr.add(offset) as *mut u64
}

#[cfg(test)]
mod tests {
    use super::{molt_asyncgen_new, molt_generator_new};
    use crate::{asyncgen_registry, dec_ref_bits, obj_from_bits, GEN_CONTROL_SIZE};

    #[test]
    fn asyncgen_registry_removes_on_drop() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            {
                let mut guard = asyncgen_registry(_py).lock().unwrap();
                guard.clear();
            }
            let gen_bits = molt_generator_new(0, GEN_CONTROL_SIZE as u64);
            assert!(
                !obj_from_bits(gen_bits).is_none(),
                "generator allocation failed"
            );
            let asyncgen_bits = molt_asyncgen_new(gen_bits);
            assert!(
                !obj_from_bits(asyncgen_bits).is_none(),
                "async generator allocation failed"
            );
            let len = asyncgen_registry(_py).lock().unwrap().len();
            assert_eq!(len, 1);
            dec_ref_bits(_py, asyncgen_bits);
            let len_after = asyncgen_registry(_py).lock().unwrap().len();
            assert_eq!(len_after, 0);
            dec_ref_bits(_py, gen_bits);
        });
    }
}

unsafe fn generator_set_slot(_py: &PyToken<'_>, ptr: *mut u8, offset: usize, bits: u64) {
    crate::gil_assert();
    let slot = generator_slot_ptr(ptr, offset);
    let old_bits = *slot;
    dec_ref_bits(_py, old_bits);
    inc_ref_bits(_py, bits);
    *slot = bits;
}

pub(crate) unsafe fn generator_closed(ptr: *mut u8) -> bool {
    let bits = *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET);
    obj_from_bits(bits).as_bool().unwrap_or(false)
}

unsafe fn generator_set_closed(_py: &PyToken<'_>, ptr: *mut u8, closed: bool) {
    crate::gil_assert();
    let bits = MoltObject::from_bool(closed).bits();
    generator_set_slot(_py, ptr, GEN_CLOSED_OFFSET, bits);
}

pub(crate) unsafe fn generator_running(ptr: *mut u8) -> bool {
    let header = header_from_obj_ptr(ptr);
    ((*header).flags & HEADER_FLAG_GEN_RUNNING) != 0
}

unsafe fn generator_set_running(_py: &PyToken<'_>, ptr: *mut u8, running: bool) {
    crate::gil_assert();
    let header = header_from_obj_ptr(ptr);
    if running {
        (*header).flags |= HEADER_FLAG_GEN_RUNNING;
    } else {
        (*header).flags &= !HEADER_FLAG_GEN_RUNNING;
    }
}

pub(crate) unsafe fn generator_started(ptr: *mut u8) -> bool {
    let header = header_from_obj_ptr(ptr);
    ((*header).flags & HEADER_FLAG_GEN_STARTED) != 0
}

pub(crate) unsafe fn generator_yieldfrom_bits(ptr: *mut u8) -> u64 {
    *generator_slot_ptr(ptr, GEN_YIELD_FROM_OFFSET)
}

unsafe fn generator_close_yieldfrom(_py: &PyToken<'_>, iter_bits: u64) -> bool {
    if obj_from_bits(iter_bits).is_none() {
        return true;
    }
    let Some(ptr) = obj_from_bits(iter_bits).as_ptr() else {
        return true;
    };
    if object_type_id(ptr) == TYPE_ID_GENERATOR {
        let res_bits = molt_generator_close(iter_bits);
        if exception_pending(_py) {
            return false;
        }
        if !obj_from_bits(res_bits).is_none() {
            dec_ref_bits(_py, res_bits);
        }
        return true;
    }
    let Some(close_name_bits) = attr_name_bits_from_bytes(_py, b"close") else {
        return true;
    };
    if let Some(close_bits) = attr_lookup_ptr_allow_missing(_py, ptr, close_name_bits) {
        let res_bits = call_callable0(_py, close_bits);
        dec_ref_bits(_py, close_bits);
        if exception_pending(_py) {
            if !obj_from_bits(res_bits).is_none() {
                dec_ref_bits(_py, res_bits);
            }
            return false;
        }
        if !obj_from_bits(res_bits).is_none() {
            dec_ref_bits(_py, res_bits);
        }
    }
    true
}

fn resolve_sleep_target(_py: &PyToken<'_>, future_ptr: *mut u8) -> *mut u8 {
    if future_ptr.is_null() {
        return future_ptr;
    }
    let mut cursor = future_ptr;
    for _ in 0..16 {
        let poll_fn = unsafe { (*header_from_obj_ptr(cursor)).poll_fn };
        if poll_fn == async_sleep_poll_fn_addr() || poll_fn == io_wait_poll_fn_addr() {
            return cursor;
        }
        let next = {
            let waiting_map = task_waiting_on(_py).lock().unwrap();
            waiting_map.get(&PtrSlot(cursor)).map(|val| val.0)
        };
        let Some(next_ptr) = next else {
            return future_ptr;
        };
        if next_ptr.is_null() || next_ptr == cursor {
            return future_ptr;
        }
        cursor = next_ptr;
    }
    future_ptr
}

unsafe fn generator_set_started(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    let header = header_from_obj_ptr(ptr);
    (*header).flags |= HEADER_FLAG_GEN_STARTED;
}

unsafe fn generator_pending_throw(ptr: *mut u8) -> bool {
    let bits = *generator_slot_ptr(ptr, GEN_THROW_OFFSET);
    !obj_from_bits(bits).is_none()
}

pub(crate) fn generator_done_tuple(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    let done_bits = MoltObject::from_bool(true).bits();
    let tuple_ptr = alloc_tuple(_py, &[value_bits, done_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn generator_unpack_pair(_py: &PyToken<'_>, bits: u64) -> Option<(u64, bool)> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return None;
        }
        let elems = crate::seq_vec_ref(ptr);
        if elems.len() < 2 {
            return None;
        }
        let done = is_truthy(_py, obj_from_bits(elems[1]));
        Some((elems[0], done))
    }
}

unsafe fn raise_stop_iteration_from_value(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if obj_from_bits(value_bits).is_none() {
        return raise_exception::<_>(_py, "StopIteration", "");
    }
    let msg_bits = molt_str_from_obj(value_bits);
    let msg = string_obj_to_owned(obj_from_bits(msg_bits)).unwrap_or_default();
    dec_ref_bits(_py, msg_bits);
    raise_exception::<_>(_py, "StopIteration", &msg)
}

unsafe fn generator_method_result(_py: &PyToken<'_>, res_bits: u64) -> u64 {
    if let Some((val_bits, done)) = generator_unpack_pair(_py, res_bits) {
        if done {
            return raise_stop_iteration_from_value(_py, val_bits);
        }
        inc_ref_bits(_py, val_bits);
        return val_bits;
    }
    res_bits
}

#[no_mangle]
pub extern "C" fn molt_task_new(poll_fn_addr: u64, closure_size: u64, kind_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (type_id, is_coroutine) = match kind_bits {
            TASK_KIND_FUTURE => (TYPE_ID_OBJECT, false),
            TASK_KIND_COROUTINE => (TYPE_ID_OBJECT, true),
            TASK_KIND_GENERATOR => (TYPE_ID_GENERATOR, false),
            _ => {
                return raise_exception::<_>(_py, "TypeError", "unknown task kind");
            }
        };
        if type_id == TYPE_ID_GENERATOR && (closure_size as usize) < GEN_CONTROL_SIZE {
            return raise_exception::<_>(_py, "TypeError", "generator task closure too small");
        }
        let total_size = std::mem::size_of::<MoltHeader>() + closure_size as usize;
        let ptr = alloc_object(_py, total_size, type_id);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let slots = closure_size as usize / std::mem::size_of::<u64>();
            if slots > 0 {
                let payload_ptr = ptr as *mut u64;
                for idx in 0..slots {
                    *payload_ptr.add(idx) = MoltObject::none().bits();
                }
            }
            let header = header_from_obj_ptr(ptr);
            (*header).poll_fn = poll_fn_addr;
            (*header).state = 0;
            if is_coroutine {
                (*header).flags |= HEADER_FLAG_COROUTINE;
            }
            if type_id == TYPE_ID_GENERATOR && closure_size as usize >= GEN_CONTROL_SIZE {
                *generator_slot_ptr(ptr, GEN_SEND_OFFSET) = MoltObject::none().bits();
                *generator_slot_ptr(ptr, GEN_THROW_OFFSET) = MoltObject::none().bits();
                *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET) = MoltObject::from_bool(false).bits();
                *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET) = MoltObject::from_int(1).bits();
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_awaitable_await(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(self_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        unsafe {
            if (*header_from_obj_ptr(ptr)).poll_fn == 0 {
                return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
            }
        }
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_is_native_awaitable(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(val_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        let has_poll = unsafe { (*header_from_obj_ptr(ptr)).poll_fn != 0 };
        MoltObject::from_bool(has_poll).bits()
    })
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
/// - `token_bits` must be an integer cancel token id.
#[no_mangle]
pub unsafe extern "C" fn molt_task_register_token_owned(task_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        register_task_token(_py, task_ptr, id);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_task_new(poll_fn_addr, closure_size, TASK_KIND_GENERATOR)
    })
}

#[no_mangle]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let is_gen = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
        MoltObject::from_bool(is_gen).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_send(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            if generator_running(ptr) {
                return raise_exception::<_>(_py, "ValueError", "generator already executing");
            }
            if generator_closed(ptr) {
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            if !generator_started(ptr) && !obj_from_bits(send_bits).is_none() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "can't send non-None value to a just-started generator",
                );
            }
            generator_set_slot(_py, ptr, GEN_SEND_OFFSET, send_bits);
            generator_set_slot(_py, ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
            let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
            let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
            exception_stack_set_depth(_py, gen_depth);
            let prev_raise = generator_raise_active();
            set_generator_raise(true);
            generator_set_started(_py, ptr);
            generator_set_running(_py, ptr, true);
            let res = call_poll_fn(_py, poll_fn_addr, ptr);
            generator_set_running(_py, ptr, false);
            set_generator_raise(prev_raise);
            let pending = exception_pending(_py);
            let exc_bits = if pending {
                let bits = molt_exception_last();
                clear_exception(_py);
                bits
            } else {
                MoltObject::none().bits()
            };
            let new_depth = exception_stack_depth();
            generator_set_slot(
                _py,
                ptr,
                GEN_EXC_DEPTH_OFFSET,
                MoltObject::from_int(new_depth as i64).bits(),
            );
            exception_context_align_depth(_py, new_depth);
            let gen_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            generator_exception_stack_store(ptr, gen_active);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            exception_stack_set_depth(_py, caller_depth);
            exception_context_fallback_pop();
            if pending {
                return generator_raise_from_pending(_py, ptr, exc_bits);
            }
            let res_bits = res as u64;
            if let Some((_val, done)) = generator_unpack_pair(_py, res_bits) {
                if done {
                    generator_set_closed(_py, ptr, true);
                }
            }
            res_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_throw(gen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            if generator_running(ptr) {
                return raise_exception::<_>(_py, "ValueError", "generator already executing");
            }
            if generator_closed(ptr) {
                return molt_raise(exc_bits);
            }
            if !generator_started(ptr) {
                generator_set_closed(_py, ptr, true);
                return molt_raise(exc_bits);
            }
            generator_set_slot(_py, ptr, GEN_THROW_OFFSET, exc_bits);
            generator_set_slot(_py, ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
            let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
            let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
            exception_stack_set_depth(_py, gen_depth);
            let prev_raise = generator_raise_active();
            set_generator_raise(true);
            generator_set_started(_py, ptr);
            generator_set_running(_py, ptr, true);
            let res = call_poll_fn(_py, poll_fn_addr, ptr);
            generator_set_running(_py, ptr, false);
            set_generator_raise(prev_raise);
            let pending = exception_pending(_py);
            let exc_bits = if pending {
                let bits = molt_exception_last();
                clear_exception(_py);
                bits
            } else {
                MoltObject::none().bits()
            };
            let new_depth = exception_stack_depth();
            generator_set_slot(
                _py,
                ptr,
                GEN_EXC_DEPTH_OFFSET,
                MoltObject::from_int(new_depth as i64).bits(),
            );
            exception_context_align_depth(_py, new_depth);
            let gen_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            generator_exception_stack_store(ptr, gen_active);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            exception_stack_set_depth(_py, caller_depth);
            exception_context_fallback_pop();
            if pending {
                return generator_raise_from_pending(_py, ptr, exc_bits);
            }
            let res_bits = res as u64;
            if let Some((_val, done)) = generator_unpack_pair(_py, res_bits) {
                if done {
                    generator_set_closed(_py, ptr, true);
                }
            }
            res_bits
        }
    })
}

unsafe fn generator_resume_bits(_py: &PyToken<'_>, gen_bits: u64) -> u64 {
    let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
        return raise_exception::<_>(_py, "TypeError", "expected generator");
    };
    if object_type_id(ptr) != TYPE_ID_GENERATOR {
        return raise_exception::<_>(_py, "TypeError", "expected generator");
    }
    if generator_closed(ptr) {
        return generator_done_tuple(_py, MoltObject::none().bits());
    }
    let header = header_from_obj_ptr(ptr);
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 {
        generator_set_closed(_py, ptr, true);
        return generator_done_tuple(_py, MoltObject::none().bits());
    }
    let caller_depth = exception_stack_depth();
    let caller_active =
        ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_context = caller_active
        .last()
        .copied()
        .unwrap_or(MoltObject::none().bits());
    exception_context_fallback_push(caller_context);
    let gen_active = generator_exception_stack_take(ptr);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = gen_active;
    });
    let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
    let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
    let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
    exception_stack_set_depth(_py, gen_depth);
    let prev_raise = generator_raise_active();
    set_generator_raise(true);
    generator_set_started(_py, ptr);
    generator_set_running(_py, ptr, true);
    let res = call_poll_fn(_py, poll_fn_addr, ptr);
    generator_set_running(_py, ptr, false);
    set_generator_raise(prev_raise);
    let exc_pending = exception_pending(_py);
    let exc_bits = if exc_pending {
        let bits = molt_exception_last();
        clear_exception(_py);
        bits
    } else {
        MoltObject::none().bits()
    };
    let new_depth = exception_stack_depth();
    generator_set_slot(
        _py,
        ptr,
        GEN_EXC_DEPTH_OFFSET,
        MoltObject::from_int(new_depth as i64).bits(),
    );
    exception_context_align_depth(_py, new_depth);
    let gen_active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    generator_exception_stack_store(ptr, gen_active);
    ACTIVE_EXCEPTION_STACK.with(|stack| {
        *stack.borrow_mut() = caller_active;
    });
    exception_stack_set_depth(_py, caller_depth);
    exception_context_fallback_pop();
    if exc_pending {
        return generator_raise_from_pending(_py, ptr, exc_bits);
    }
    let res_bits = res as u64;
    if let Some((_val, done)) = generator_unpack_pair(_py, res_bits) {
        if done {
            generator_set_closed(_py, ptr, true);
        }
    }
    res_bits
}

unsafe fn generator_raise_from_pending(_py: &PyToken<'_>, ptr: *mut u8, exc_bits: u64) -> u64 {
    let mut raise_bits = exc_bits;
    let mut converted = false;
    if let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() {
        if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
            let kind_bits = exception_kind_bits(exc_ptr);
            if let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) {
                if kind == "StopIteration" {
                    let rt_ptr =
                        alloc_exception(_py, "RuntimeError", "generator raised StopIteration");
                    if !rt_ptr.is_null() {
                        let rt_bits = MoltObject::from_ptr(rt_ptr).bits();
                        let cause_slot = rt_ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_cause = *cause_slot;
                        if old_cause != exc_bits {
                            dec_ref_bits(_py, old_cause);
                            inc_ref_bits(_py, exc_bits);
                            *cause_slot = exc_bits;
                        }
                        let context_slot = rt_ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_context = *context_slot;
                        if old_context != exc_bits {
                            dec_ref_bits(_py, old_context);
                            inc_ref_bits(_py, exc_bits);
                            *context_slot = exc_bits;
                        }
                        let suppress_bits = MoltObject::from_bool(true).bits();
                        let suppress_slot = rt_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_suppress = *suppress_slot;
                        if old_suppress != suppress_bits {
                            dec_ref_bits(_py, old_suppress);
                            inc_ref_bits(_py, suppress_bits);
                            *suppress_slot = suppress_bits;
                        }
                        raise_bits = rt_bits;
                        converted = true;
                    }
                }
            }
        }
    }
    generator_set_closed(_py, ptr, true);
    let raised = molt_raise(raise_bits);
    if converted {
        dec_ref_bits(_py, raise_bits);
    }
    dec_ref_bits(_py, exc_bits);
    raised
}

#[no_mangle]
pub extern "C" fn molt_generator_close(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            if generator_running(ptr) {
                return raise_exception::<_>(_py, "ValueError", "generator already executing");
            }
            if generator_closed(ptr) {
                return MoltObject::none().bits();
            }
            let yieldfrom_bits = generator_yieldfrom_bits(ptr);
            if !obj_from_bits(yieldfrom_bits).is_none() {
                inc_ref_bits(_py, yieldfrom_bits);
                generator_set_slot(_py, ptr, GEN_YIELD_FROM_OFFSET, MoltObject::none().bits());
                let ok = generator_close_yieldfrom(_py, yieldfrom_bits);
                dec_ref_bits(_py, yieldfrom_bits);
                if !ok {
                    let exc_bits = molt_exception_last();
                    clear_exception(_py);
                    let res = molt_generator_throw(gen_bits, exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return res;
                }
            }
            if !generator_started(ptr) {
                generator_set_closed(_py, ptr, true);
                return MoltObject::none().bits();
            }
            let exc_ptr = alloc_exception(_py, "GeneratorExit", "");
            if exc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
            generator_set_slot(_py, ptr, GEN_THROW_OFFSET, exc_bits);
            dec_ref_bits(_py, exc_bits);
            generator_set_slot(_py, ptr, GEN_SEND_OFFSET, MoltObject::none().bits());
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return MoltObject::none().bits();
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            let gen_depth_bits = *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET);
            let gen_depth = to_i64(obj_from_bits(gen_depth_bits)).unwrap_or(0);
            let gen_depth = if gen_depth < 0 { 0 } else { gen_depth as usize };
            exception_stack_set_depth(_py, gen_depth);
            let prev_raise = generator_raise_active();
            set_generator_raise(true);
            generator_set_started(_py, ptr);
            generator_set_running(_py, ptr, true);
            let res = call_poll_fn(_py, poll_fn_addr, ptr) as u64;
            generator_set_running(_py, ptr, false);
            set_generator_raise(prev_raise);
            let pending = exception_pending(_py);
            let exc_bits = if pending {
                let bits = molt_exception_last();
                clear_exception(_py);
                bits
            } else {
                MoltObject::none().bits()
            };
            let new_depth = exception_stack_depth();
            generator_set_slot(
                _py,
                ptr,
                GEN_EXC_DEPTH_OFFSET,
                MoltObject::from_int(new_depth as i64).bits(),
            );
            exception_context_align_depth(_py, new_depth);
            let gen_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            generator_exception_stack_store(ptr, gen_active);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            exception_stack_set_depth(_py, caller_depth);
            exception_context_fallback_pop();
            if pending {
                let exc_obj = obj_from_bits(exc_bits);
                let is_exit = if let Some(exc_ptr) = exc_obj.as_ptr() {
                    if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                        let kind =
                            string_obj_to_owned(obj_from_bits(crate::exception_kind_bits(exc_ptr)))
                                .unwrap_or_default();
                        kind == "GeneratorExit"
                    } else {
                        false
                    }
                } else {
                    false
                };
                if is_exit {
                    dec_ref_bits(_py, exc_bits);
                    generator_set_closed(_py, ptr, true);
                    return MoltObject::none().bits();
                }
                generator_set_closed(_py, ptr, true);
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised;
            }
            if let Some((_val, done)) = generator_unpack_pair(_py, res) {
                if !done {
                    return raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "generator ignored GeneratorExit",
                    );
                }
            }
            generator_set_closed(_py, ptr, true);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_next_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_send(gen_bits, MoltObject::none().bits());
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_send_method(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_send(gen_bits, send_bits);
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_throw_method(gen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            if generator_closed(ptr) {
                return molt_raise(exc_bits);
            }
        }
        let res = molt_generator_throw(gen_bits, exc_bits);
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[no_mangle]
pub extern "C" fn molt_generator_close_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_close(gen_bits);
        if exception_pending(_py) {
            return res;
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_coroutine_close_method(coro_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(coro_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_future_task(_py, task_ptr, None);
        task_mark_done(task_ptr);
        MoltObject::none().bits()
    })
}

fn asyncgen_registry_insert(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry(_py).lock().unwrap();
    guard.insert(PtrSlot(ptr));
}

pub(crate) fn asyncgen_registry_remove(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry(_py).lock().unwrap();
    guard.remove(&PtrSlot(ptr));
}

fn asyncgen_registry_take(_py: &PyToken<'_>) -> Vec<u64> {
    crate::gil_assert();
    let mut guard = asyncgen_registry(_py).lock().unwrap();
    if guard.is_empty() {
        return Vec::new();
    }
    let mut gens = Vec::with_capacity(guard.len());
    for slot in guard.iter() {
        if slot.0.is_null() {
            continue;
        }
        let bits = MoltObject::from_ptr(slot.0).bits();
        gens.push(bits);
    }
    guard.clear();
    gens
}

unsafe fn asyncgen_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    ptr.add(offset) as *mut u64
}

pub(crate) unsafe fn asyncgen_gen_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_GEN_OFFSET)
}

pub(crate) unsafe fn asyncgen_running_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET)
}

pub(crate) unsafe fn asyncgen_pending_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET)
}

pub(crate) unsafe fn asyncgen_firstiter_bits(ptr: *mut u8) -> u64 {
    *asyncgen_slot_ptr(ptr, ASYNCGEN_FIRSTITER_OFFSET)
}

unsafe fn asyncgen_set_firstiter_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = asyncgen_slot_ptr(ptr, ASYNCGEN_FIRSTITER_OFFSET);
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
    }
}

unsafe fn asyncgen_firstiter_called(ptr: *mut u8) -> bool {
    obj_from_bits(asyncgen_firstiter_bits(ptr))
        .as_bool()
        .unwrap_or(false)
}

unsafe fn asyncgen_set_running_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET);
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
    }
}

unsafe fn asyncgen_set_pending_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET);
    let old_bits = *slot;
    if old_bits != bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
    }
}

unsafe fn asyncgen_clear_pending_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    asyncgen_set_pending_bits(_py, ptr, MoltObject::none().bits());
}

unsafe fn asyncgen_clear_running_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    asyncgen_set_running_bits(_py, ptr, MoltObject::none().bits());
}

pub(crate) unsafe fn asyncgen_running(ptr: *mut u8) -> bool {
    !obj_from_bits(asyncgen_running_bits(ptr)).is_none()
}

pub(crate) unsafe fn asyncgen_await_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let running_bits = asyncgen_running_bits(ptr);
    let Some(running_ptr) = maybe_ptr_from_bits(running_bits) else {
        return MoltObject::none().bits();
    };
    // Prefer the outer awaitable cached in the async generator's await slots.
    let gen_bits = asyncgen_gen_bits(ptr);
    if let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) {
        if object_type_id(gen_ptr) == TYPE_ID_GENERATOR {
            let poll_fn_addr = (*header_from_obj_ptr(gen_ptr)).poll_fn;
            let await_offsets: Vec<usize> = {
                let registry = runtime_state(_py).asyncgen_locals.lock().unwrap();
                match registry.get(&poll_fn_addr) {
                    Some(entry) => entry
                        .names
                        .iter()
                        .zip(&entry.offsets)
                        .filter_map(|(name_bits, offset)| {
                            let name = string_obj_to_owned(obj_from_bits(*name_bits))?;
                            if name.starts_with("__await_future_") {
                                Some(*offset)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    None => Vec::new(),
                }
            };
            let missing = missing_bits(_py);
            for offset in await_offsets {
                let bits = *generator_slot_ptr(gen_ptr, offset);
                let obj = obj_from_bits(bits);
                if bits == missing || obj.is_none() {
                    continue;
                }
                inc_ref_bits(_py, bits);
                return bits;
            }
        }
    }

    let awaited = {
        let map = task_waiting_on(_py).lock().unwrap();
        map.get(&PtrSlot(running_ptr)).copied()
    };
    let Some(PtrSlot(awaited_ptr)) = awaited else {
        return MoltObject::none().bits();
    };
    if awaited_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let bits = MoltObject::from_ptr(awaited_ptr).bits();
    inc_ref_bits(_py, bits);
    bits
}

pub(crate) unsafe fn asyncgen_code_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let gen_bits = asyncgen_gen_bits(ptr);
    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
        return MoltObject::none().bits();
    };
    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
        return MoltObject::none().bits();
    }
    let header = header_from_obj_ptr(gen_ptr);
    let poll_fn_addr = (*header).poll_fn;
    let code_bits = fn_ptr_code_get(_py, poll_fn_addr);
    if code_bits == 0 {
        return MoltObject::none().bits();
    }
    inc_ref_bits(_py, code_bits);
    code_bits
}

unsafe fn asyncgen_call_firstiter_if_needed(
    _py: &PyToken<'_>,
    asyncgen_bits: u64,
    asyncgen_ptr: *mut u8,
) -> Option<u64> {
    if asyncgen_firstiter_called(asyncgen_ptr) {
        return None;
    }
    asyncgen_set_firstiter_bits(_py, asyncgen_ptr, MoltObject::from_bool(true).bits());
    let hook_bits = {
        let hooks = runtime_state(_py).asyncgen_hooks.lock().unwrap();
        hooks.firstiter
    };
    if obj_from_bits(hook_bits).is_none() {
        return None;
    }
    inc_ref_bits(_py, hook_bits);
    let res_bits = call_callable1(_py, hook_bits, asyncgen_bits);
    dec_ref_bits(_py, hook_bits);
    if exception_pending(_py) {
        let exc_bits = molt_exception_last();
        clear_exception(_py);
        let raised = molt_raise(exc_bits);
        dec_ref_bits(_py, exc_bits);
        return Some(raised);
    }
    if res_bits != 0 {
        dec_ref_bits(_py, res_bits);
    }
    None
}

pub(crate) unsafe fn asyncgen_call_finalizer(_py: &PyToken<'_>, asyncgen_ptr: *mut u8) {
    let gen_bits = asyncgen_gen_bits(asyncgen_ptr);
    let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
        return;
    };
    if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
        return;
    }
    if generator_closed(gen_ptr) {
        return;
    }
    let hook_bits = {
        let hooks = runtime_state(_py).asyncgen_hooks.lock().unwrap();
        hooks.finalizer
    };
    if obj_from_bits(hook_bits).is_none() {
        return;
    }
    inc_ref_bits(_py, hook_bits);
    let asyncgen_bits = MoltObject::from_ptr(asyncgen_ptr).bits();
    let res_bits = call_callable1(_py, hook_bits, asyncgen_bits);
    dec_ref_bits(_py, hook_bits);
    if exception_pending(_py) {
        let exc_bits = molt_exception_last();
        clear_exception(_py);
        dec_ref_bits(_py, exc_bits);
        return;
    }
    if res_bits != 0 {
        dec_ref_bits(_py, res_bits);
    }
}

fn asyncgen_running_message(op: i64) -> &'static str {
    match op {
        ASYNCGEN_OP_ANEXT => "anext(): asynchronous generator is already running",
        ASYNCGEN_OP_ASEND => "asend(): asynchronous generator is already running",
        ASYNCGEN_OP_ATHROW => "athrow(): asynchronous generator is already running",
        ASYNCGEN_OP_ACLOSE => "aclose(): asynchronous generator is already running",
        _ => "asynchronous generator is already running",
    }
}

fn asyncgen_shutdown_trace_enabled() -> bool {
    matches!(
        std::env::var("MOLT_TRACE_ASYNCGEN_SHUTDOWN").as_deref(),
        Ok("1")
    )
}

fn asyncgen_close_trace_enabled() -> bool {
    matches!(
        std::env::var("MOLT_TRACE_ASYNCGEN_CLOSE").as_deref(),
        Ok("1")
    )
}

unsafe fn asyncgen_future_new(
    _py: &PyToken<'_>,
    asyncgen_bits: u64,
    op_kind: i64,
    arg_bits: u64,
) -> u64 {
    let payload = (3 * std::mem::size_of::<u64>()) as u64;
    let obj_bits = molt_future_new(asyncgen_poll_fn_addr(), payload);
    if obj_from_bits(obj_bits).is_none() {
        return obj_bits;
    }
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    let payload_ptr = obj_ptr as *mut u64;
    *payload_ptr = asyncgen_bits;
    *payload_ptr.add(1) = MoltObject::from_int(op_kind).bits();
    *payload_ptr.add(2) = arg_bits;
    inc_ref_bits(_py, asyncgen_bits);
    inc_ref_bits(_py, arg_bits);
    obj_bits
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_new(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            let total = std::mem::size_of::<MoltHeader>() + ASYNCGEN_CONTROL_SIZE;
            let ptr = alloc_object(_py, total, TYPE_ID_ASYNC_GENERATOR);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            let payload_ptr = ptr as *mut u64;
            *payload_ptr = gen_bits;
            inc_ref_bits(_py, gen_bits);
            *payload_ptr.add(1) = MoltObject::none().bits();
            *payload_ptr.add(2) = MoltObject::none().bits();
            *payload_ptr.add(3) = MoltObject::from_bool(false).bits();
            object_mark_has_ptrs(_py, ptr);
            asyncgen_registry_insert(_py, ptr);
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_hooks_get() -> u64 {
    crate::with_gil_entry!(_py, {
        let hooks = runtime_state(_py).asyncgen_hooks.lock().unwrap();
        let ptr = alloc_tuple(_py, &[hooks.firstiter, hooks.finalizer]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_hooks_set(firstiter_bits: u64, finalizer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let first_obj = obj_from_bits(firstiter_bits);
        if !first_obj.is_none() {
            let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(firstiter_bits)));
            if !callable_ok {
                let type_label = type_name(_py, first_obj);
                let msg = format!("callable firstiter expected, got {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let final_obj = obj_from_bits(finalizer_bits);
        if !final_obj.is_none() {
            let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(finalizer_bits)));
            if !callable_ok {
                let type_label = type_name(_py, final_obj);
                let msg = format!("callable finalizer expected, got {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let mut hooks = runtime_state(_py).asyncgen_hooks.lock().unwrap();
        if hooks.firstiter != 0 {
            dec_ref_bits(_py, hooks.firstiter);
        }
        if hooks.finalizer != 0 {
            dec_ref_bits(_py, hooks.finalizer);
        }
        hooks.firstiter = firstiter_bits;
        hooks.finalizer = finalizer_bits;
        if firstiter_bits != 0 {
            inc_ref_bits(_py, firstiter_bits);
        }
        if finalizer_bits != 0 {
            inc_ref_bits(_py, finalizer_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_locals_register(
    fn_ptr: u64,
    names_bits: u64,
    offsets_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if fn_ptr == 0 {
            return MoltObject::none().bits();
        }
        let Some(names_ptr) = obj_from_bits(names_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "asyncgen locals names must be tuple");
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "asyncgen locals offsets must be tuple");
        };
        unsafe {
            if object_type_id(names_ptr) != TYPE_ID_TUPLE
                || object_type_id(offsets_ptr) != TYPE_ID_TUPLE
            {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "asyncgen locals metadata must be tuples",
                );
            }
        }
        let names_vec = unsafe { seq_vec_ref(names_ptr) }.clone();
        let offsets_vec = unsafe { seq_vec_ref(offsets_ptr) };
        if names_vec.len() != offsets_vec.len() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "asyncgen locals names/offsets mismatch",
            );
        }
        let mut offsets = Vec::with_capacity(offsets_vec.len());
        for &bits in offsets_vec.iter() {
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "asyncgen locals offsets must be int",
                );
            };
            if val < 0 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "asyncgen locals offsets must be non-negative",
                );
            }
            offsets.push(val as usize);
        }
        for &bits in names_vec.iter() {
            let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "asyncgen locals names must be str");
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "asyncgen locals names must be str",
                    );
                }
            }
        }
        let entry = AsyncGenLocalsEntry {
            names: names_vec,
            offsets,
        };
        let mut guard = runtime_state(_py).asyncgen_locals.lock().unwrap();
        if let Some(old) = guard.insert(fn_ptr, entry.clone()) {
            for bits in old.names {
                if bits != 0 {
                    dec_ref_bits(_py, bits);
                }
            }
        }
        for &bits in entry.names.iter() {
            if bits != 0 {
                inc_ref_bits(_py, bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_locals(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let empty_dict = || {
            let ptr = alloc_dict_with_pairs(_py, &[]);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        };
        let Some(asyncgen_ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "object is not a Python async generator",
            );
        };
        unsafe {
            if object_type_id(asyncgen_ptr) != TYPE_ID_ASYNC_GENERATOR {
                let name = type_name(_py, obj_from_bits(asyncgen_bits));
                let msg = format!("{name} is not a Python async generator");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let gen_bits = asyncgen_gen_bits(asyncgen_ptr);
            let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                return empty_dict();
            };
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                return empty_dict();
            }
            if generator_closed(gen_ptr) {
                return empty_dict();
            }
            if !generator_started(gen_ptr) {
                return empty_dict();
            }
            let header = header_from_obj_ptr(gen_ptr);
            let poll_fn_addr = (*header).poll_fn;
            let entry = {
                let guard = runtime_state(_py).asyncgen_locals.lock().unwrap();
                guard.get(&poll_fn_addr).cloned()
            };
            let Some(entry) = entry else {
                return empty_dict();
            };
            if entry.names.is_empty() {
                return empty_dict();
            }
            let missing = missing_bits(_py);
            let mut pairs: Vec<u64> = Vec::with_capacity(entry.names.len() * 2);
            for (name_bits, offset) in entry.names.iter().zip(entry.offsets.iter()) {
                let val_bits = *(gen_ptr.add(*offset) as *const u64);
                if asyncgen_locals_trace_enabled() {
                    let name = string_obj_to_owned(obj_from_bits(*name_bits))
                        .unwrap_or_else(|| "<invalid>".to_string());
                    let is_missing = val_bits == missing;
                    let val_obj = obj_from_bits(val_bits);
                    let val_type = type_name(_py, val_obj);
                    eprintln!(
                        "molt async trace: asyncgen_locals name={} missing={} type={}",
                        name, is_missing, val_type
                    );
                }
                if val_bits == missing {
                    continue;
                }
                pairs.push(*name_bits);
                pairs.push(val_bits);
            }
            let ptr = alloc_dict_with_pairs(_py, &pairs);
            if ptr.is_null() {
                return empty_dict();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_gen_locals_register(fn_ptr: u64, names_bits: u64, offsets_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if fn_ptr == 0 {
            return MoltObject::none().bits();
        }
        let Some(names_ptr) = obj_from_bits(names_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "generator locals names must be tuple");
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "generator locals offsets must be tuple",
            );
        };
        unsafe {
            if object_type_id(names_ptr) != TYPE_ID_TUPLE
                || object_type_id(offsets_ptr) != TYPE_ID_TUPLE
            {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "generator locals metadata must be tuples",
                );
            }
        }
        let names_vec = unsafe { seq_vec_ref(names_ptr) }.clone();
        let offsets_vec = unsafe { seq_vec_ref(offsets_ptr) };
        if names_vec.len() != offsets_vec.len() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "generator locals names/offsets mismatch",
            );
        }
        let mut offsets = Vec::with_capacity(offsets_vec.len());
        for &bits in offsets_vec.iter() {
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "generator locals offsets must be int",
                );
            };
            if val < 0 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "generator locals offsets must be non-negative",
                );
            }
            offsets.push(val as usize);
        }
        for &bits in names_vec.iter() {
            let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "generator locals names must be str",
                );
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "generator locals names must be str",
                    );
                }
            }
        }
        let entry = GenLocalsEntry {
            names: names_vec,
            offsets,
        };
        let mut guard = runtime_state(_py).gen_locals.lock().unwrap();
        if let Some(old) = guard.insert(fn_ptr, entry.clone()) {
            for bits in old.names {
                if bits != 0 {
                    dec_ref_bits(_py, bits);
                }
            }
        }
        for &bits in entry.names.iter() {
            if bits != 0 {
                inc_ref_bits(_py, bits);
            }
        }
        MoltObject::none().bits()
    })
}

pub(crate) unsafe fn generator_locals_dict(_py: &PyToken<'_>, gen_ptr: *mut u8) -> u64 {
    let empty_dict = || {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    };
    if generator_closed(gen_ptr) || !generator_started(gen_ptr) {
        return empty_dict();
    }
    let header = header_from_obj_ptr(gen_ptr);
    let poll_fn_addr = (*header).poll_fn;
    let entry = {
        let guard = runtime_state(_py).gen_locals.lock().unwrap();
        guard.get(&poll_fn_addr).cloned()
    };
    let Some(entry) = entry else {
        return empty_dict();
    };
    if entry.names.is_empty() {
        return empty_dict();
    }
    let missing = missing_bits(_py);
    let mut pairs: Vec<u64> = Vec::with_capacity(entry.names.len() * 2);
    for (name_bits, offset) in entry.names.iter().zip(entry.offsets.iter()) {
        let val_bits = *(gen_ptr.add(*offset) as *const u64);
        if val_bits == missing {
            continue;
        }
        pairs.push(*name_bits);
        pairs.push(val_bits);
    }
    let ptr = alloc_dict_with_pairs(_py, &pairs);
    if ptr.is_null() {
        return empty_dict();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_gen_locals(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not a Python generator");
        };
        unsafe {
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                let name = type_name(_py, obj_from_bits(gen_bits));
                let msg = format!("{name} is not a Python generator");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            generator_locals_dict(_py, gen_ptr)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_aiter(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected async generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            }
        }
        inc_ref_bits(_py, asyncgen_bits);
        asyncgen_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_anext(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(
                _py,
                asyncgen_bits,
                ASYNCGEN_OP_ANEXT,
                MoltObject::none().bits(),
            )
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_asend(asyncgen_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ASEND, val_bits)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_athrow(asyncgen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            };
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
            asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ATHROW, exc_bits)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_aclose(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected async generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_ASYNC_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected async generator");
            }
            if let Some(raised) = asyncgen_call_firstiter_if_needed(_py, asyncgen_bits, ptr) {
                return raised;
            }
        }
        let exc_ptr = alloc_exception(_py, "GeneratorExit", "");
        if exc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        let future_bits =
            unsafe { asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ACLOSE, exc_bits) };
        dec_ref_bits(_py, exc_bits);
        future_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_shutdown() -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = asyncgen_shutdown_trace_enabled();
        let prior_exc_bits = if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            exception_clear_reason_set("asyncgen_shutdown_prior");
            molt_exception_clear();
            Some(exc_bits)
        } else {
            None
        };
        let gens = asyncgen_registry_take(_py);
        if trace {
            eprintln!("asyncgen_shutdown count={}", gens.len());
        }
        for gen_bits in gens {
            if trace {
                eprintln!("asyncgen_shutdown gen_bits={gen_bits}");
            }
            let future_bits = molt_asyncgen_aclose(gen_bits);
            if trace {
                eprintln!("asyncgen_shutdown future_bits={future_bits}");
            }
            if !obj_from_bits(future_bits).is_none() {
                unsafe {
                    let _ = crate::molt_block_on(future_bits);
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    exception_clear_reason_set("asyncgen_shutdown_block_on");
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, future_bits);
            }
            dec_ref_bits(_py, gen_bits);
        }
        if let Some(exc_bits) = prior_exc_bits {
            let _ = molt_exception_set_last(exc_bits);
            dec_ref_bits(_py, exc_bits);
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// Caller must pass a valid async-generator awaitable object bits value.
/// The runtime must be initialized and the thread must be allowed to enter
/// the GIL-guarded runtime state.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncgen_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        struct PendingExceptionGuard<'a> {
            py: &'a PyToken<'a>,
            prior_bits: Option<u64>,
        }

        impl<'a> PendingExceptionGuard<'a> {
            fn new(py: &'a PyToken<'a>) -> Self {
                let prior_bits = if exception_pending(py) {
                    let bits = molt_exception_last();
                    exception_clear_reason_set("asyncgen_poll_guard_prior");
                    molt_exception_clear();
                    Some(bits)
                } else {
                    None
                };
                Self { py, prior_bits }
            }

            fn restore(&mut self) {
                let Some(prior_bits) = self.prior_bits.take() else {
                    return;
                };
                if exception_pending(self.py) {
                    let cur_bits = molt_exception_last();
                    exception_clear_reason_set("asyncgen_poll_guard_restore");
                    molt_exception_clear();
                    dec_ref_bits(self.py, cur_bits);
                }
                let _ = molt_exception_set_last(prior_bits);
                dec_ref_bits(self.py, prior_bits);
            }
        }

        impl Drop for PendingExceptionGuard<'_> {
            fn drop(&mut self) {
                self.restore();
            }
        }

        let _pending_guard = PendingExceptionGuard::new(_py);
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return MoltObject::none().bits() as i64;
        }
        let payload_ptr = obj_ptr as *mut u64;
        let asyncgen_bits = *payload_ptr;
        let op_bits = *payload_ptr.add(1);
        let arg_bits = *payload_ptr.add(2);
        let op = to_i64(obj_from_bits(op_bits)).unwrap_or(-1);
        let Some(asyncgen_ptr) = maybe_ptr_from_bits(asyncgen_bits) else {
            return raise_exception::<i64>(_py, "TypeError", "expected async generator");
        };
        if object_type_id(asyncgen_ptr) != TYPE_ID_ASYNC_GENERATOR {
            return raise_exception::<i64>(_py, "TypeError", "expected async generator");
        }
        let gen_bits = asyncgen_gen_bits(asyncgen_ptr);
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<i64>(_py, "TypeError", "expected generator");
        };
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<i64>(_py, "TypeError", "expected generator");
        }
        let task_ptr = current_task_ptr();
        let task_bits = if task_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(task_ptr).bits()
        };
        // Prefer the current task as the running marker so ag_running/ag_await
        // can resolve the awaited future via the task_waiting_on map.
        let running_marker_bits = if task_bits == MoltObject::none().bits() {
            obj_bits
        } else {
            task_bits
        };
        let running_bits = asyncgen_running_bits(asyncgen_ptr);
        let running_obj = obj_from_bits(running_bits);
        if !running_obj.is_none() && running_bits != running_marker_bits {
            return raise_exception::<i64>(_py, "RuntimeError", asyncgen_running_message(op));
        }
        if generator_running(gen_ptr) {
            return raise_exception::<i64>(_py, "RuntimeError", asyncgen_running_message(op));
        }
        let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
        if !obj_from_bits(pending_bits).is_none()
            && matches!(op, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND)
        {
            inc_ref_bits(_py, pending_bits);
            asyncgen_clear_pending_bits(_py, asyncgen_ptr);
            let raised = molt_raise(pending_bits);
            dec_ref_bits(_py, pending_bits);
            return raised as i64;
        }

        let res_bits = if (*header).state != 0 {
            generator_resume_bits(_py, gen_bits)
        } else {
            match op {
                ASYNCGEN_OP_ANEXT => {
                    if generator_closed(gen_ptr) {
                        if generator_pending_throw(gen_ptr) {
                            let throw_bits = *generator_slot_ptr(gen_ptr, GEN_THROW_OFFSET);
                            inc_ref_bits(_py, throw_bits);
                            generator_set_slot(
                                _py,
                                gen_ptr,
                                GEN_THROW_OFFSET,
                                MoltObject::none().bits(),
                            );
                            let raised = molt_raise(throw_bits);
                            dec_ref_bits(_py, throw_bits);
                            return raised as i64;
                        }
                        return raise_exception::<i64>(_py, "StopAsyncIteration", "");
                    }
                    if generator_pending_throw(gen_ptr) {
                        generator_resume_bits(_py, gen_bits)
                    } else {
                        molt_generator_send(gen_bits, MoltObject::none().bits())
                    }
                }
                ASYNCGEN_OP_ASEND => {
                    if generator_closed(gen_ptr) {
                        if generator_pending_throw(gen_ptr) {
                            return generator_resume_bits(_py, gen_bits) as i64;
                        }
                        return raise_exception::<i64>(_py, "StopAsyncIteration", "");
                    }
                    if !generator_started(gen_ptr) && !obj_from_bits(arg_bits).is_none() {
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "can't send non-None value to a just-started async generator",
                        );
                    }
                    if generator_pending_throw(gen_ptr) {
                        generator_resume_bits(_py, gen_bits)
                    } else {
                        molt_generator_send(gen_bits, arg_bits)
                    }
                }
                ASYNCGEN_OP_ATHROW => {
                    if generator_closed(gen_ptr) {
                        if generator_pending_throw(gen_ptr) {
                            return raise_exception::<i64>(_py, "StopAsyncIteration", "");
                        }
                        return MoltObject::none().bits() as i64;
                    }
                    molt_generator_throw(gen_bits, arg_bits)
                }
                ASYNCGEN_OP_ACLOSE => {
                    if asyncgen_close_trace_enabled() {
                        let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
                        eprintln!(
                            "asyncgen_aclose gen=0x{:x} started={} closed={} pending={}",
                            gen_ptr as usize,
                            generator_started(gen_ptr),
                            generator_closed(gen_ptr),
                            !obj_from_bits(pending_bits).is_none()
                        );
                    }
                    if generator_closed(gen_ptr) {
                        return MoltObject::none().bits() as i64;
                    }
                    if !generator_started(gen_ptr) {
                        generator_set_closed(_py, gen_ptr, true);
                        asyncgen_registry_remove(_py, asyncgen_ptr);
                        return MoltObject::none().bits() as i64;
                    }
                    molt_generator_throw(gen_bits, arg_bits)
                }
                _ => return raise_exception::<i64>(_py, "TypeError", "invalid async generator op"),
            }
        };

        if exception_pending(_py) {
            if running_bits == running_marker_bits {
                asyncgen_clear_running_bits(_py, asyncgen_ptr);
            }
            (*header).state = 0;
            if op == ASYNCGEN_OP_ACLOSE {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(_py, kind_bits);
                if matches!(
                    kind.as_deref(),
                    Some("GeneratorExit" | "StopAsyncIteration")
                ) {
                    exception_clear_reason_set("asyncgen_aclose_swallow");
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                    generator_set_closed(_py, gen_ptr, true);
                    asyncgen_registry_remove(_py, asyncgen_ptr);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, exc_bits);
            }
            return res_bits as i64;
        }

        if res_bits as i64 == pending_bits_i64() {
            asyncgen_set_running_bits(_py, asyncgen_ptr, running_marker_bits);
            (*header).state = 1;
            return res_bits as i64;
        }

        if running_bits == running_marker_bits {
            asyncgen_clear_running_bits(_py, asyncgen_ptr);
        }
        (*header).state = 0;

        if let Some((val_bits, done)) = generator_unpack_pair(_py, res_bits) {
            if !done {
                inc_ref_bits(_py, val_bits);
            }
            dec_ref_bits(_py, res_bits);
            if op == ASYNCGEN_OP_ACLOSE {
                if done {
                    generator_set_closed(_py, gen_ptr, true);
                    asyncgen_registry_remove(_py, asyncgen_ptr);
                    return MoltObject::none().bits() as i64;
                }
                if !obj_from_bits(arg_bits).is_none() {
                    asyncgen_set_pending_bits(_py, asyncgen_ptr, arg_bits);
                    generator_set_slot(_py, gen_ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
                }
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "async generator ignored GeneratorExit",
                );
            }
            if done {
                asyncgen_registry_remove(_py, asyncgen_ptr);
                match op {
                    ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND => {
                        return raise_exception::<i64>(_py, "StopAsyncIteration", "");
                    }
                    ASYNCGEN_OP_ATHROW => {
                        return MoltObject::none().bits() as i64;
                    }
                    _ => {
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            return val_bits as i64;
        }

        res_bits as i64
    })
}

#[no_mangle]
pub extern "C" fn molt_future_poll_fn(future_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(future_bits);
        let Some(ptr) = obj.as_ptr() else {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                eprintln!(
                    "Molt awaitable debug: bits=0x{:x} type={}",
                    future_bits,
                    type_name(_py, obj)
                );
            }
            raise_exception::<()>(_py, "TypeError", "object is not awaitable");
            return 0;
        };
        unsafe {
            let _gil = GilGuard::new();
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                    let mut class_name = None;
                    if object_type_id(ptr) == TYPE_ID_OBJECT {
                        let class_bits = object_class_bits(ptr);
                        if class_bits != 0 {
                            class_name = Some(class_name_for_error(class_bits));
                        }
                    }
                    eprintln!(
                    "Molt awaitable debug: bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                    future_bits,
                    type_name(_py, obj),
                    class_name.as_deref().unwrap_or("-"),
                    (*header).state,
                    (*header).size
                );
                }
                raise_exception::<()>(_py, "TypeError", "object is not awaitable");
                return 0;
            }
            poll_fn_addr
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_future_poll(future_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(future_bits);
        let Some(ptr) = obj.as_ptr() else {
            if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                eprintln!(
                    "Molt awaitable debug: poll bits=0x{:x} type={}",
                    future_bits,
                    type_name(_py, obj)
                );
            }
            raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
            return 0;
        };
        unsafe {
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
                    let mut class_name = None;
                    if object_type_id(ptr) == TYPE_ID_OBJECT {
                        let class_bits = object_class_bits(ptr);
                        if class_bits != 0 {
                            class_name = Some(class_name_for_error(class_bits));
                        }
                    }
                    eprintln!(
                        "Molt awaitable debug: poll bits=0x{:x} type={} class={} poll=0x0 state={} size={}",
                        future_bits,
                        type_name(_py, obj),
                        class_name.as_deref().unwrap_or("-"),
                        (*header).state,
                        (*header).size
                    );
                }
                raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
                return 0;
            }
            let res = crate::poll_future_with_task_stack(_py, ptr, poll_fn_addr);
            if promise_trace_enabled() && poll_fn_addr == promise_poll_fn_addr() {
                let state = (*header).state;
                eprintln!(
                    "molt async trace: promise_poll task=0x{:x} state={} res=0x{:x}",
                    ptr as usize, state, res as u64
                );
            }
            if task_cancel_pending(ptr) {
                task_take_cancel_pending(ptr);
                return raise_cancelled_with_message::<i64>(_py, ptr);
            }
            let current_task = current_task_ptr();
            if res == pending_bits_i64() {
                if !current_task.is_null() && ptr != current_task {
                    await_waiter_register(_py, current_task, ptr);
                    let current_header = header_from_obj_ptr(current_task);
                    let is_block_on = ((*current_header).flags & HEADER_FLAG_BLOCK_ON) != 0;
                    let is_spawned = ((*current_header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
                    if is_block_on || is_spawned {
                        let sleep_target = resolve_sleep_target(_py, ptr);
                        let _ = sleep_register_impl(_py, current_task, sleep_target);
                    }
                }
            } else if !current_task.is_null() {
                await_waiter_clear(_py, current_task);
            }
            if !current_task.is_null() {
                let current_cancelled = task_cancel_pending(current_task);
                if current_cancelled {
                    task_take_cancel_pending(current_task);
                    return raise_cancelled_with_message::<i64>(_py, current_task);
                }
            }
            if res != pending_bits_i64() {
                task_mark_done(ptr);
            }
            if res != pending_bits_i64() && !current_task.is_null() && ptr != current_task {
                let awaited_exception = {
                    let guard = task_last_exceptions(_py).lock().unwrap();
                    guard.get(&PtrSlot(ptr)).copied()
                };
                if let Some(exc_ptr) = awaited_exception {
                    let exc_bits = MoltObject::from_ptr(exc_ptr.0).bits();
                    inc_ref_bits(_py, exc_bits);
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                } else {
                    let prev_task = crate::CURRENT_TASK.with(|cell| {
                        let prev = cell.get();
                        cell.set(ptr);
                        prev
                    });
                    let exc_bits = if exception_pending(_py) {
                        molt_exception_last()
                    } else {
                        MoltObject::none().bits()
                    };
                    if debug_current_task() && prev_task.is_null() {
                        let current = crate::CURRENT_TASK.with(|cell| cell.get());
                        if !current.is_null() {
                            eprintln!(
                                "molt task trace: generators restore null current=0x{:x} task=0x{:x}",
                                current as usize, ptr as usize
                            );
                        }
                    }
                    crate::CURRENT_TASK.with(|cell| cell.set(prev_task));
                    if obj_from_bits(exc_bits).is_none() {
                        let global_exc = {
                            let guard = runtime_state(_py).last_exception.lock().unwrap();
                            guard.map(|ptr| ptr.0)
                        };
                        if let Some(exc_ptr) = global_exc {
                            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
                            inc_ref_bits(_py, exc_bits);
                            let raised = molt_raise(exc_bits);
                            dec_ref_bits(_py, exc_bits);
                            clear_exception_state(_py);
                            return raised as i64;
                        }
                    } else {
                        let raised = molt_raise(exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        return raised as i64;
                    }
                }
            }
            if res != pending_bits_i64() && !task_has_token(_py, ptr) {
                task_exception_stack_drop(_py, ptr);
                task_exception_depth_drop(_py, ptr);
                task_exception_baseline_drop(_py, ptr);
            }
            res
        }
    })
}

fn cancel_future_task(_py: &PyToken<'_>, task_ptr: *mut u8, msg_bits: Option<u64>) {
    if task_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: cancel_future task=0x{:x}",
            task_ptr as usize
        );
    }
    match msg_bits {
        Some(bits) => task_cancel_message_set(_py, task_ptr, bits),
        None => task_cancel_message_clear(_py, task_ptr),
    }
    task_set_cancel_pending(task_ptr);
    let awaited_ptr = {
        let waiting_map = task_waiting_on(_py).lock().unwrap();
        waiting_map.get(&PtrSlot(task_ptr)).map(|val| val.0)
    };
    if let Some(awaited_ptr) = awaited_ptr {
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: cancel_future_waiting task=0x{:x} awaited=0x{:x}",
                task_ptr as usize, awaited_ptr as usize
            );
        }
        if !awaited_ptr.is_null() {
            let sleep_target = resolve_sleep_target(_py, awaited_ptr);
            if !sleep_target.is_null() {
                let poll_fn = unsafe { (*header_from_obj_ptr(sleep_target)).poll_fn };
                if poll_fn == io_wait_poll_fn_addr() {
                    #[cfg(not(target_arch = "wasm32"))]
                    runtime_state(_py).io_poller().cancel_waiter(sleep_target);
                }
            }
        }
    }
    await_waiter_clear(_py, task_ptr);
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        let poll_fn = (*header).poll_fn;
        if poll_fn == thread_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = thread_task_state(_py, task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.condvar.notify_all();
            }
        }
        if poll_fn == process_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(state) = process_task_state(_py, task_ptr) {
                state.cancelled.store(true, AtomicOrdering::Release);
                state.process.condvar.notify_all();
            }
        }
        if poll_fn == io_wait_poll_fn_addr() {
            #[cfg(not(target_arch = "wasm32"))]
            runtime_state(_py).io_poller().cancel_waiter(task_ptr);
        }
    }
    let waiters = await_waiters_take(_py, task_ptr);
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: cancel_future_waiters task=0x{:x} count={}",
            task_ptr as usize,
            waiters.len()
        );
        for waiter in &waiters {
            eprintln!(
                "molt async trace: cancel_future_wake waiter=0x{:x} task=0x{:x}",
                waiter.0 as usize, task_ptr as usize
            );
        }
    }
    for waiter in waiters {
        wake_task_ptr(_py, waiter.0);
    }
    wake_task_ptr(_py, task_ptr);
}

fn sleep_register_impl(_py: &PyToken<'_>, task_ptr: *mut u8, future_ptr: *mut u8) -> bool {
    if async_trace_enabled() || sleep_trace_enabled() {
        eprintln!(
            "molt async trace: sleep_register_impl_enter task=0x{:x} future=0x{:x}",
            task_ptr as usize, future_ptr as usize
        );
    }
    if future_ptr.is_null() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail future_null");
        }
        return false;
    }
    let mut resolved_task = task_ptr;
    if resolved_task.is_null() {
        resolved_task = await_waiters(_py)
            .lock()
            .unwrap()
            .get(&PtrSlot(future_ptr))
            .and_then(|list| list.first().copied())
            .map(|waiter| waiter.0)
            .unwrap_or(std::ptr::null_mut());
    }
    if resolved_task.is_null() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail task_null task=0x{:x} future=0x{:x}",
                task_ptr as usize, future_ptr as usize
            );
        }
        return false;
    }
    let task_ptr = resolved_task;
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let poll_fn = unsafe { (*header).poll_fn };
    if poll_fn != async_sleep_poll_fn_addr() && poll_fn != io_wait_poll_fn_addr() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail poll_fn=0x{:x}",
                poll_fn
            );
        }
        return false;
    }
    if unsafe { (*header).state == 0 } {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail state=0");
        }
        return false;
    }
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>())
    };
    let payload_ptr = future_ptr as *mut u64;
    let deadline_obj = if poll_fn == async_sleep_poll_fn_addr() {
        if payload_bytes < std::mem::size_of::<u64>() {
            return false;
        }
        obj_from_bits(unsafe { *payload_ptr })
    } else {
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return false;
        }
        obj_from_bits(unsafe { *payload_ptr.add(2) })
    };
    if poll_fn == io_wait_poll_fn_addr() && deadline_obj.is_none() {
        // I/O waits without a timeout rely on the poller to wake the task.
        return true;
    }
    let Some(deadline_secs) = to_f64(deadline_obj) else {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!("molt async trace: sleep_register_impl_fail deadline_nan");
        }
        return false;
    };
    if !deadline_secs.is_finite() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_impl_fail deadline_secs={}",
                deadline_secs
            );
        }
        return false;
    }
    if poll_fn == async_sleep_poll_fn_addr() && deadline_secs < 0.0 {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_yield task=0x{:x}",
                task_ptr as usize
            );
        }
        let task_header = unsafe { header_from_obj_ptr(task_ptr) };
        if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
            return true;
        }
        runtime_state(_py).scheduler().defer_task_ptr(task_ptr);
        return true;
    }
    if deadline_secs <= 0.0 {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_immediate task=0x{:x} deadline_secs={}",
                task_ptr as usize, deadline_secs
            );
        }
        if poll_fn == async_sleep_poll_fn_addr() {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            let task_header = unsafe { header_from_obj_ptr(task_ptr) };
            if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(target_arch = "wasm32")]
            {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if is_block_on_task(task_ptr) {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_blocking(_py, task_ptr, deadline);
                } else {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_scheduler(_py, task_ptr, deadline);
                }
                return true;
            }
        }
        wake_task_ptr(_py, task_ptr);
        return true;
    }
    let deadline = instant_from_monotonic_secs(_py, deadline_secs);
    if deadline <= Instant::now() {
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_register_immediate_elapsed task=0x{:x}",
                task_ptr as usize
            );
        }
        if poll_fn == async_sleep_poll_fn_addr() {
            let deadline =
                Instant::now() + Duration::from_secs_f64(ASYNC_SLEEP_YIELD_SECS.max(0.0));
            let task_header = unsafe { header_from_obj_ptr(task_ptr) };
            if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(target_arch = "wasm32")]
            {
                runtime_state(_py)
                    .sleep_queue()
                    .register_blocking(_py, task_ptr, deadline);
                return true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if is_block_on_task(task_ptr) {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_blocking(_py, task_ptr, deadline);
                } else {
                    runtime_state(_py)
                        .sleep_queue()
                        .register_scheduler(_py, task_ptr, deadline);
                }
                return true;
            }
        }
        wake_task_ptr(_py, task_ptr);
        return true;
    }
    let task_header = unsafe { header_from_obj_ptr(task_ptr) };
    if unsafe { ((*task_header).flags & HEADER_FLAG_BLOCK_ON) != 0 } {
        runtime_state(_py)
            .sleep_queue()
            .register_blocking(_py, task_ptr, deadline);
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        runtime_state(_py)
            .sleep_queue()
            .register_blocking(_py, task_ptr, deadline);
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_block_on_task(task_ptr) {
            runtime_state(_py)
                .sleep_queue()
                .register_blocking(_py, task_ptr, deadline);
        } else {
            runtime_state(_py)
                .sleep_queue()
                .register_scheduler(_py, task_ptr, deadline);
        }
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register_request task=0x{:x} deadline_secs={} delay_ms={}",
                task_ptr as usize,
                deadline_secs,
                delay.as_secs_f64() * 1000.0
            );
        }
        true
    }
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel(future_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_future_task(_py, task_ptr, None);
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel_msg(future_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        cancel_future_task(_py, task_ptr, Some(msg_bits));
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_future_cancel_clear(future_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        task_cancel_message_clear(_py, task_ptr);
        let _ = task_take_cancel_pending(task_ptr);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_future_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_task_new(poll_fn_addr, closure_size, TASK_KIND_FUTURE);
        if std::env::var("MOLT_DEBUG_AWAITABLE").is_ok() {
            if let Some(obj_ptr) = resolve_obj_ptr(obj_bits) {
                unsafe {
                    let header = header_from_obj_ptr(obj_ptr);
                    eprintln!(
                        "Molt future init debug: bits=0x{:x} poll=0x{:x} size={}",
                        obj_bits,
                        poll_fn_addr,
                        (*header).size
                    );
                }
            }
        }
        obj_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_promise_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(promise_poll_fn_addr(), std::mem::size_of::<u64>() as u64);
        if promise_trace_enabled() {
            eprintln!("molt async trace: promise_new bits=0x{:x}", obj_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt promise future.
#[no_mangle]
pub unsafe extern "C" fn molt_promise_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(obj_bits);
        if ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(ptr);
        if async_trace_enabled() || promise_trace_enabled() {
            let current = current_task_ptr();
            eprintln!(
                "molt async trace: promise_poll task=0x{:x} state={} current=0x{:x}",
                ptr as usize,
                (*header).state,
                current as usize
            );
        }
        match (*header).state {
            0 => pending_bits_i64(),
            1 => {
                let payload_ptr = ptr as *mut u64;
                let res_bits = *payload_ptr;
                inc_ref_bits(_py, res_bits);
                res_bits as i64
            }
            2 => {
                let payload_ptr = ptr as *mut u64;
                let exc_bits = *payload_ptr;
                let _ = molt_raise(exc_bits);
                MoltObject::none().bits() as i64
            }
            _ => MoltObject::none().bits() as i64,
        }
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt promise future.
#[no_mangle]
pub unsafe extern "C" fn molt_promise_set_result(future_bits: u64, result_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if async_trace_enabled() || promise_trace_enabled() {
            eprintln!(
                "molt async trace: promise_set_result_enter bits=0x{:x}",
                future_bits
            );
        }
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!("molt async trace: promise_set_result_fail reason=resolve");
            }
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        let header = header_from_obj_ptr(task_ptr);
        if (*header).poll_fn != promise_poll_fn_addr() {
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_set_result_fail reason=poll_fn poll=0x{:x}",
                    (*header).poll_fn
                );
            }
            return raise_exception::<_>(_py, "TypeError", "object is not a promise");
        }
        if (*header).state != 0 {
            if async_trace_enabled() || promise_trace_enabled() {
                eprintln!(
                    "molt async trace: promise_set_result_skip state={}",
                    (*header).state
                );
            }
            return MoltObject::none().bits();
        }
        let payload_ptr = task_ptr as *mut u64;
        *payload_ptr = result_bits;
        inc_ref_bits(_py, result_bits);
        (*header).state = 1;
        if async_trace_enabled() || promise_trace_enabled() {
            eprintln!(
                "molt async trace: promise_set_result task=0x{:x}",
                task_ptr as usize
            );
        }
        let waiters = await_waiters_take(_py, task_ptr);
        if async_trace_enabled() || promise_trace_enabled() {
            eprintln!(
                "molt async trace: promise_wake task=0x{:x} waiters={}",
                task_ptr as usize,
                waiters.len()
            );
        }
        for waiter in waiters {
            wake_task_ptr(_py, waiter.0);
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt promise future.
#[no_mangle]
pub unsafe extern "C" fn molt_promise_set_exception(future_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        let header = header_from_obj_ptr(task_ptr);
        if (*header).poll_fn != promise_poll_fn_addr() {
            return raise_exception::<_>(_py, "TypeError", "object is not a promise");
        }
        if (*header).state != 0 {
            return MoltObject::none().bits();
        }
        let payload_ptr = task_ptr as *mut u64;
        *payload_ptr = exc_bits;
        inc_ref_bits(_py, exc_bits);
        (*header).state = 2;
        if async_trace_enabled() || promise_trace_enabled() {
            eprintln!(
                "molt async trace: promise_set_exception task=0x{:x}",
                task_ptr as usize
            );
        }
        let waiters = await_waiters_take(_py, task_ptr);
        if async_trace_enabled() || promise_trace_enabled() {
            eprintln!(
                "molt async trace: promise_wake task=0x{:x} waiters={}",
                task_ptr as usize,
                waiters.len()
            );
        }
        for waiter in waiters {
            wake_task_ptr(_py, waiter.0);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_async_sleep_new(delay_bits: u64, result_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            async_sleep_poll_fn_addr(),
            (2 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = delay_bits;
            *payload_ptr.add(1) = result_bits;
            inc_ref_bits(_py, delay_bits);
            inc_ref_bits(_py, result_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer if the runtime associates a future with it.
#[no_mangle]
pub unsafe extern "C" fn molt_async_sleep(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let _obj_ptr = ptr_from_bits(obj_bits);
        if _obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let task_ptr = current_task_ptr();
        if !task_ptr.is_null() && task_cancel_pending(task_ptr) {
            task_take_cancel_pending(task_ptr);
            return raise_cancelled_with_message::<i64>(_py, task_ptr);
        }
        let header = header_from_obj_ptr(_obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        let payload_len = payload_bytes / std::mem::size_of::<u64>();
        let payload_ptr = _obj_ptr as *mut u64;
        if (*header).state == 0 {
            let delay_secs = if payload_len >= 1 {
                let delay_bits = *payload_ptr;
                let float_bits = molt_float_from_obj(delay_bits);
                let delay_obj = obj_from_bits(float_bits);
                delay_obj.as_float().unwrap_or(0.0)
            } else {
                0.0
            };
            let delay_secs = if delay_secs.is_finite() && delay_secs > 0.0 {
                delay_secs
            } else {
                0.0
            };
            let immediate = delay_secs <= 0.0;
            if payload_len >= 1 {
                let deadline = if immediate {
                    ASYNC_SLEEP_YIELD_SENTINEL
                } else {
                    crate::monotonic_now_secs(_py) + delay_secs
                };
                *payload_ptr = MoltObject::from_float(deadline).bits();
            }
            (*header).state = 1;
            if async_trace_enabled() || sleep_trace_enabled() {
                eprintln!(
                    "molt async trace: async_sleep_init task=0x{:x} delay={} immediate={}",
                    task_ptr as usize, delay_secs, immediate
                );
            }
            return pending_bits_i64();
        }

        if payload_len >= 1 {
            let deadline_obj = obj_from_bits(*payload_ptr);
            if let Some(deadline) = to_f64(deadline_obj) {
                if deadline.is_finite()
                    && deadline > 0.0
                    && crate::monotonic_now_secs(_py) < deadline
                {
                    return pending_bits_i64();
                }
            }
        }

        let result_bits = if payload_len >= 2 {
            *payload_ptr.add(1)
        } else {
            MoltObject::none().bits()
        };
        inc_ref_bits(_py, result_bits);
        if async_trace_enabled() || sleep_trace_enabled() {
            eprintln!(
                "molt async trace: async_sleep_ready task=0x{:x}",
                task_ptr as usize
            );
        }
        result_bits as i64
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a Molt future allocated with payload slots.
#[no_mangle]
pub unsafe extern "C" fn molt_anext_default_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let _obj_ptr = ptr_from_bits(obj_bits);
        if _obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(_obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 3 * std::mem::size_of::<u64>() {
            return MoltObject::none().bits() as i64;
        }
        let payload_ptr = _obj_ptr as *mut u64;
        let iter_bits = *payload_ptr;
        let default_bits = *payload_ptr.add(1);
        if (*header).state == 0 {
            let await_bits = molt_anext(iter_bits);
            inc_ref_bits(_py, await_bits);
            *payload_ptr.add(2) = await_bits;
            (*header).state = 1;
        }
        let await_bits = *payload_ptr.add(2);
        let Some(await_ptr) = maybe_ptr_from_bits(await_bits) else {
            return MoltObject::none().bits() as i64;
        };
        let await_header = header_from_obj_ptr(await_ptr);
        let poll_fn_addr = (*await_header).poll_fn;
        if poll_fn_addr == 0 {
            return MoltObject::none().bits() as i64;
        }
        let res = molt_future_poll(await_bits);
        if res == pending_bits_i64() {
            return res;
        }
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            let kind_bits = molt_exception_kind(exc_bits);
            let kind = string_obj_to_owned(obj_from_bits(kind_bits));
            dec_ref_bits(_py, kind_bits);
            if kind.as_deref() == Some("StopAsyncIteration") {
                exception_clear_reason_set("anext_default_stopasync");
                molt_exception_clear();
                dec_ref_bits(_py, exc_bits);
                inc_ref_bits(_py, default_bits);
                return default_bits as i64;
            }
            dec_ref_bits(_py, exc_bits);
        }
        res
    })
}

/// # Safety
/// - `task_ptr` must be a valid Molt task pointer.
/// - `future_ptr` must point to a valid Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_sleep_register(task_ptr: *mut u8, future_ptr: *mut u8) -> u64 {
    crate::with_gil_entry!(_py, {
        if task_ptr.is_null() || future_ptr.is_null() {
            return 0;
        }
        let header = header_from_obj_ptr(task_ptr);
        let flags = (*header).flags;
        let is_block_on = (flags & HEADER_FLAG_BLOCK_ON) != 0;
        let is_spawned = (flags & HEADER_FLAG_SPAWN_RETAIN) != 0;
        if !is_block_on && !is_spawned {
            return 0;
        }
        let sleep_target = resolve_sleep_target(_py, future_ptr);
        if sleep_register_impl(_py, task_ptr, sleep_target) {
            1
        } else {
            0
        }
    })
}
