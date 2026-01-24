use crate::PyToken;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Instant;

use molt_obj_model::MoltObject;

use crate::concurrency::GilGuard;
use crate::object::accessors::resolve_obj_ptr;
use crate::{
    alloc_exception, alloc_object, alloc_tuple, async_sleep_poll_fn_addr, async_trace_enabled,
    asyncgen_poll_fn_addr, asyncgen_registry, await_waiter_clear, await_waiter_register,
    await_waiters_take, call_poll_fn, class_name_for_error, clear_exception, current_task_ptr,
    dec_ref_bits, exception_context_align_depth, exception_context_fallback_pop,
    exception_context_fallback_push, exception_pending, exception_stack_depth,
    exception_stack_set_depth, fn_ptr_code_get, generator_exception_stack_store,
    generator_exception_stack_take, generator_raise_active, header_from_obj_ptr, inc_ref_bits,
    instant_from_monotonic_secs, io_wait_poll_fn_addr, is_block_on_task, maybe_ptr_from_bits,
    molt_anext, molt_exception_clear, molt_exception_kind, molt_exception_last,
    molt_float_from_obj, molt_raise, obj_from_bits, object_class_bits, object_mark_has_ptrs,
    object_type_id, pending_bits_i64, process_poll_fn_addr, process_task_state, ptr_from_bits,
    raise_cancelled_with_message, raise_exception, record_exception, register_task_token,
    resolve_task_ptr, runtime_state, set_generator_raise, string_obj_to_owned,
    task_cancel_message_clear, task_cancel_message_set, task_cancel_pending,
    task_exception_depth_drop, task_exception_stack_drop, task_has_token, task_last_exceptions,
    task_set_cancel_pending, task_take_cancel_pending, task_waiting_on, thread_poll_fn_addr,
    thread_task_state, to_f64, to_i64, token_id_from_bits, type_name, wake_task_ptr, MoltHeader,
    PtrSlot, ACTIVE_EXCEPTION_STACK, ASYNCGEN_CONTROL_SIZE, ASYNCGEN_GEN_OFFSET,
    ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT, ASYNCGEN_OP_ASEND, ASYNCGEN_OP_ATHROW,
    ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET, GEN_CLOSED_OFFSET, GEN_CONTROL_SIZE,
    GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET, HEADER_FLAG_GEN_RUNNING,
    HEADER_FLAG_GEN_STARTED, HEADER_FLAG_SPAWN_RETAIN, TASK_KIND_FUTURE, TASK_KIND_GENERATOR,
    TYPE_ID_ASYNC_GENERATOR, TYPE_ID_EXCEPTION, TYPE_ID_GENERATOR, TYPE_ID_OBJECT, TYPE_ID_TUPLE,
};

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
                let mut guard = asyncgen_registry().lock().unwrap();
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
            let len = asyncgen_registry().lock().unwrap().len();
            assert_eq!(len, 1);
            dec_ref_bits(_py, asyncgen_bits);
            let len_after = asyncgen_registry().lock().unwrap().len();
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

fn generator_unpack_pair(bits: u64) -> Option<(u64, bool)> {
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
        let done = obj_from_bits(elems[1]).as_bool().unwrap_or(false);
        Some((elems[0], done))
    }
}

#[no_mangle]
pub extern "C" fn molt_task_new(poll_fn_addr: u64, closure_size: u64, kind_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_id = match kind_bits {
            TASK_KIND_FUTURE => TYPE_ID_OBJECT,
            TASK_KIND_GENERATOR => TYPE_ID_GENERATOR,
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
            if generator_closed(ptr) {
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            generator_set_slot(_py, ptr, GEN_SEND_OFFSET, send_bits);
            generator_set_slot(_py, ptr, GEN_THROW_OFFSET, MoltObject::none().bits());
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
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
                generator_set_closed(_py, ptr, true);
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised;
            }
            res as u64
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
            if generator_closed(ptr) {
                return generator_done_tuple(_py, MoltObject::none().bits());
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
                generator_set_closed(_py, ptr, true);
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised;
            }
            res as u64
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
        generator_set_closed(_py, ptr, true);
        let raised = molt_raise(exc_bits);
        dec_ref_bits(_py, exc_bits);
        return raised;
    }
    res as u64
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
            if generator_closed(ptr) {
                return MoltObject::none().bits();
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
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised;
            }
            if let Some((_val, done)) = generator_unpack_pair(res) {
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

fn asyncgen_registry_insert(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry().lock().unwrap();
    guard.insert(PtrSlot(ptr));
}

pub(crate) fn asyncgen_registry_remove(_py: &PyToken<'_>, ptr: *mut u8) {
    crate::gil_assert();
    if ptr.is_null() {
        return;
    }
    let mut guard = asyncgen_registry().lock().unwrap();
    guard.remove(&PtrSlot(ptr));
}

fn asyncgen_registry_take(_py: &PyToken<'_>) -> Vec<u64> {
    crate::gil_assert();
    let mut guard = asyncgen_registry().lock().unwrap();
    if guard.is_empty() {
        return Vec::new();
    }
    let mut gens = Vec::with_capacity(guard.len());
    for slot in guard.iter() {
        if slot.0.is_null() {
            continue;
        }
        let bits = MoltObject::from_ptr(slot.0).bits();
        inc_ref_bits(_py, bits);
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
    let code_bits = fn_ptr_code_get(poll_fn_addr);
    if code_bits == 0 {
        return MoltObject::none().bits();
    }
    inc_ref_bits(_py, code_bits);
    code_bits
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
            object_mark_has_ptrs(_py, ptr);
            asyncgen_registry_insert(_py, ptr);
            MoltObject::from_ptr(ptr).bits()
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
        unsafe { asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ASEND, val_bits) }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_athrow(asyncgen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe { asyncgen_future_new(_py, asyncgen_bits, ASYNCGEN_OP_ATHROW, exc_bits) }
    })
}

#[no_mangle]
pub extern "C" fn molt_asyncgen_aclose(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let gens = asyncgen_registry_take(_py);
        for gen_bits in gens {
            let future_bits = molt_asyncgen_aclose(gen_bits);
            if !obj_from_bits(future_bits).is_none() {
                unsafe {
                    let _ = crate::molt_block_on(future_bits);
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, future_bits);
            }
            dec_ref_bits(_py, gen_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub unsafe extern "C" fn molt_asyncgen_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
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
        let running_bits = asyncgen_running_bits(asyncgen_ptr);
        let running_obj = obj_from_bits(running_bits);
        if (*header).state == 0 {
            if !running_obj.is_none() && running_bits != obj_bits {
                return raise_exception::<i64>(_py, "RuntimeError", asyncgen_running_message(op));
            }
        } else if !running_obj.is_none() && running_bits != obj_bits {
            return raise_exception::<i64>(_py, "RuntimeError", asyncgen_running_message(op));
        }
        if generator_running(gen_ptr) {
            return raise_exception::<i64>(_py, "RuntimeError", asyncgen_running_message(op));
        }
        let pending_bits = asyncgen_pending_bits(asyncgen_ptr);
        if !obj_from_bits(pending_bits).is_none() {
            if matches!(op, ASYNCGEN_OP_ANEXT | ASYNCGEN_OP_ASEND) {
                inc_ref_bits(_py, pending_bits);
                asyncgen_clear_pending_bits(_py, asyncgen_ptr);
                let raised = molt_raise(pending_bits);
                dec_ref_bits(_py, pending_bits);
                return raised as i64;
            }
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
                    if generator_closed(gen_ptr) {
                        return MoltObject::none().bits() as i64;
                    }
                    if !generator_started(gen_ptr) {
                        generator_set_closed(_py, gen_ptr, true);
                        return MoltObject::none().bits() as i64;
                    }
                    molt_generator_throw(gen_bits, arg_bits)
                }
                _ => return raise_exception::<i64>(_py, "TypeError", "invalid async generator op"),
            }
        };

        if exception_pending(_py) {
            if running_bits == obj_bits {
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
                    molt_exception_clear();
                    dec_ref_bits(_py, exc_bits);
                    generator_set_closed(_py, gen_ptr, true);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, exc_bits);
            }
            return res_bits as i64;
        }

        if res_bits as i64 == pending_bits_i64() {
            asyncgen_set_running_bits(_py, asyncgen_ptr, obj_bits);
            (*header).state = 1;
            return res_bits as i64;
        }

        if running_bits == obj_bits {
            asyncgen_clear_running_bits(_py, asyncgen_ptr);
        }
        (*header).state = 0;

        if let Some((val_bits, done)) = generator_unpack_pair(res_bits) {
            if !done {
                inc_ref_bits(_py, val_bits);
            }
            dec_ref_bits(_py, res_bits);
            if op == ASYNCGEN_OP_ACLOSE {
                generator_set_closed(_py, gen_ptr, true);
                if done {
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
            raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
            return 0;
        };
        unsafe {
            let header = header_from_obj_ptr(ptr);
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr == 0 {
                raise_exception::<i64>(_py, "TypeError", "object is not awaitable");
                return 0;
            }
            let res = crate::poll_future_with_task_stack(_py, ptr, poll_fn_addr);
            let current_task = current_task_ptr();
            if res == pending_bits_i64() {
                if !current_task.is_null() && ptr != current_task {
                    await_waiter_register(_py, current_task, ptr);
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
            if res != pending_bits_i64() && !current_task.is_null() && ptr != current_task {
                let awaited_exception = {
                    let guard = task_last_exceptions(_py).lock().unwrap();
                    guard.get(&PtrSlot(ptr)).copied()
                };
                if let Some(exc_ptr) = awaited_exception {
                    record_exception(_py, exc_ptr.0);
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
                    crate::CURRENT_TASK.with(|cell| cell.set(prev_task));
                    if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                        record_exception(_py, exc_ptr);
                    }
                    if !obj_from_bits(exc_bits).is_none() {
                        dec_ref_bits(_py, exc_bits);
                    }
                }
            }
            if res != pending_bits_i64() && !task_has_token(_py, ptr) {
                task_exception_stack_drop(_py, ptr);
                task_exception_depth_drop(_py, ptr);
            }
            res
        }
    })
}

fn cancel_future_task(_py: &PyToken<'_>, task_ptr: *mut u8, msg_bits: Option<u64>) {
    if task_ptr.is_null() {
        return;
    }
    match msg_bits {
        Some(bits) => task_cancel_message_set(_py, task_ptr, bits),
        None => task_cancel_message_clear(_py, task_ptr),
    }
    task_set_cancel_pending(task_ptr);
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
        if ((*header).flags & HEADER_FLAG_SPAWN_RETAIN) != 0 {
            wake_task_ptr(_py, task_ptr);
        }
    }
    let waiters = await_waiters_take(_py, task_ptr);
    for waiter in waiters {
        wake_task_ptr(_py, waiter.0);
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
                    -1.0
                } else {
                    crate::monotonic_now_secs(_py) + delay_secs
                };
                *payload_ptr = MoltObject::from_float(deadline).bits();
            }
            (*header).state = 1;
            if async_trace_enabled() {
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
                if deadline.is_finite() && deadline > 0.0 {
                    if crate::monotonic_now_secs(_py) < deadline {
                        return pending_bits_i64();
                    }
                }
            }
        }

        let result_bits = if payload_len >= 2 {
            *payload_ptr.add(1)
        } else {
            MoltObject::none().bits()
        };
        inc_ref_bits(_py, result_bits);
        if async_trace_enabled() {
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
        let header = header_from_obj_ptr(future_ptr);
        let poll_fn = (*header).poll_fn;
        if poll_fn != async_sleep_poll_fn_addr() && poll_fn != io_wait_poll_fn_addr() {
            return 0;
        }
        if (*header).state == 0 {
            return 0;
        }
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        let payload_ptr = future_ptr as *mut u64;
        let deadline_obj = if poll_fn == async_sleep_poll_fn_addr() {
            if payload_bytes < std::mem::size_of::<u64>() {
                return 0;
            }
            obj_from_bits(*payload_ptr)
        } else {
            if payload_bytes < 3 * std::mem::size_of::<u64>() {
                return 0;
            }
            obj_from_bits(*payload_ptr.add(2))
        };
        let Some(deadline_secs) = to_f64(deadline_obj) else {
            return 0;
        };
        if !deadline_secs.is_finite() || deadline_secs <= 0.0 {
            return 0;
        }
        let deadline = instant_from_monotonic_secs(_py, deadline_secs);
        if deadline <= Instant::now() {
            return 0;
        }
        #[cfg(target_arch = "wasm32")]
        {
            runtime_state(_py)
                .sleep_queue()
                .register_blocking(_py, task_ptr, deadline);
            1
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
            1
        }
    })
}
