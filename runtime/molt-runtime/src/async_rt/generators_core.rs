//! Core generator protocol: task creation, generator send/throw/close, method wrappers.

use super::generators::*;
use crate::PyToken;
use crate::*;

use molt_obj_model::MoltObject;

use crate::object::HEADER_FLAG_COROUTINE;
use crate::object::accessors::resolve_obj_ptr;
use crate::state::runtime_state::GenLocalsEntry;


#[unsafe(no_mangle)]
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
            crate::object::object_set_poll_fn(ptr, poll_fn_addr);
            crate::object::object_set_state(ptr, 0);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_awaitable_await(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(self_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        if crate::object::object_poll_fn(ptr) == 0 {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        }
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_native_awaitable(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(val_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        let has_poll = crate::object::object_poll_fn(ptr) != 0;
        MoltObject::from_bool(has_poll).bits()
    })
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
/// - `token_bits` must be an integer cancel token id.
#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_new(poll_fn_addr: u64, closure_size: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_task_new(poll_fn_addr, closure_size, TASK_KIND_GENERATOR)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let is_gen = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
        MoltObject::from_bool(is_gen).bits()
    })
}

#[unsafe(no_mangle)]
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
            let _header = header_from_obj_ptr(ptr);
            let poll_fn_addr = crate::object::object_poll_fn(ptr);
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context_stack = context_stack_take();
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            let gen_context_stack = generator_context_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            context_stack_store(gen_context_stack);
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
            let gen_context_stack = context_stack_take();
            generator_context_stack_store(ptr, gen_context_stack);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            context_stack_store(caller_context_stack);
            exception_stack_set_depth(_py, caller_depth);
            exception_context_fallback_pop();
            if pending {
                return generator_raise_from_pending(_py, ptr, exc_bits);
            }
            let res_bits = res as u64;
            if let Some((_val, done)) = generator_unpack_pair(_py, res_bits)
                && done
            {
                generator_set_closed(_py, ptr, true);
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
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
            let _header = header_from_obj_ptr(ptr);
            let poll_fn_addr = crate::object::object_poll_fn(ptr);
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return generator_done_tuple(_py, MoltObject::none().bits());
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context_stack = context_stack_take();
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            let gen_context_stack = generator_context_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            context_stack_store(gen_context_stack);
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
            let gen_context_stack = context_stack_take();
            generator_context_stack_store(ptr, gen_context_stack);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            context_stack_store(caller_context_stack);
            exception_stack_set_depth(_py, caller_depth);
            exception_context_fallback_pop();
            if pending {
                return generator_raise_from_pending(_py, ptr, exc_bits);
            }
            let res_bits = res as u64;
            if let Some((_val, done)) = generator_unpack_pair(_py, res_bits)
                && done
            {
                generator_set_closed(_py, ptr, true);
            }
            res_bits
        }
    })
}

unsafe fn generator_resume_bits(_py: &PyToken<'_>, gen_bits: u64) -> u64 {
    unsafe {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        }
        if generator_closed(ptr) {
            return generator_done_tuple(_py, MoltObject::none().bits());
        }
        let _header = header_from_obj_ptr(ptr);
        let poll_fn_addr = crate::object::object_poll_fn(ptr);
        if poll_fn_addr == 0 {
            generator_set_closed(_py, ptr, true);
            return generator_done_tuple(_py, MoltObject::none().bits());
        }
        let caller_depth = exception_stack_depth();
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context_stack = context_stack_take();
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let gen_active = generator_exception_stack_take(ptr);
        let gen_context_stack = generator_context_stack_take(ptr);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = gen_active;
        });
        context_stack_store(gen_context_stack);
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
        let gen_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        generator_exception_stack_store(ptr, gen_active);
        let gen_context_stack = context_stack_take();
        generator_context_stack_store(ptr, gen_context_stack);
        ACTIVE_EXCEPTION_STACK.with(|stack| {
            *stack.borrow_mut() = caller_active;
        });
        context_stack_store(caller_context_stack);
        exception_stack_set_depth(_py, caller_depth);
        exception_context_fallback_pop();
        if exc_pending {
            return generator_raise_from_pending(_py, ptr, exc_bits);
        }
        let res_bits = res as u64;
        if let Some((_val, done)) = generator_unpack_pair(_py, res_bits)
            && done
        {
            generator_set_closed(_py, ptr, true);
        }
        res_bits
    }
}

unsafe fn generator_raise_from_pending(_py: &PyToken<'_>, ptr: *mut u8, exc_bits: u64) -> u64 {
    unsafe {
        let mut raise_bits = exc_bits;
        let mut converted = false;
        if let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr()
            && object_type_id(exc_ptr) == TYPE_ID_EXCEPTION
        {
            let kind_bits = exception_kind_bits(exc_ptr);
            if let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits))
                && kind == "StopIteration"
            {
                let rt_ptr = alloc_exception(_py, "RuntimeError", "generator raised StopIteration");
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
        generator_set_closed(_py, ptr, true);
        let raised = molt_raise(raise_bits);
        if converted {
            dec_ref_bits(_py, raise_bits);
        }
        dec_ref_bits(_py, exc_bits);
        raised
    }
}

#[unsafe(no_mangle)]
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
            let _header = header_from_obj_ptr(ptr);
            let poll_fn_addr = crate::object::object_poll_fn(ptr);
            if poll_fn_addr == 0 {
                generator_set_closed(_py, ptr, true);
                return MoltObject::none().bits();
            }
            let caller_depth = exception_stack_depth();
            let caller_active =
                ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
            let caller_context_stack = context_stack_take();
            let caller_context = caller_active
                .last()
                .copied()
                .unwrap_or(MoltObject::none().bits());
            exception_context_fallback_push(caller_context);
            let gen_active = generator_exception_stack_take(ptr);
            let gen_context_stack = generator_context_stack_take(ptr);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = gen_active;
            });
            context_stack_store(gen_context_stack);
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
            let gen_context_stack = context_stack_take();
            generator_context_stack_store(ptr, gen_context_stack);
            ACTIVE_EXCEPTION_STACK.with(|stack| {
                *stack.borrow_mut() = caller_active;
            });
            context_stack_store(caller_context_stack);
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
            if let Some((_val, done)) = generator_unpack_pair(_py, res)
                && !done
            {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "generator ignored GeneratorExit",
                );
            }
            generator_set_closed(_py, ptr, true);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_next_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_send(gen_bits, MoltObject::none().bits());
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_send_method(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_send(gen_bits, send_bits);
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[unsafe(no_mangle)]
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
        let is_generator_exit = unsafe { throw_arg_is_generator_exit(_py, exc_bits) };
        let unpacked = generator_unpack_pair(_py, res);
        if is_generator_exit {
            let done = unpacked.map(|(_, done)| done);
            let closed_now = unsafe { generator_closed(ptr) };
            if done == Some(true) || closed_now {
                return molt_raise(exc_bits);
            }
        }
        unsafe { generator_method_result(_py, res) }
    })
}

unsafe fn throw_arg_is_generator_exit(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    unsafe {
        let gen_exit_bits = exception_type_bits_from_name(_py, "GeneratorExit");
        if obj_from_bits(gen_exit_bits).is_none() {
            return false;
        }
        let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
            return false;
        };
        match object_type_id(exc_ptr) {
            TYPE_ID_EXCEPTION => {
                let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(exc_ptr)))
                    .unwrap_or_default();
                if kind == "GeneratorExit" {
                    return true;
                }
                let class_bits = crate::exception_class_bits(exc_ptr);
                class_bits == gen_exit_bits || issubclass_bits(class_bits, gen_exit_bits)
            }
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(exc_ptr);
                if items.is_empty() {
                    false
                } else {
                    throw_arg_is_generator_exit(_py, items[0])
                }
            }
            TYPE_ID_TYPE => exc_bits == gen_exit_bits || issubclass_bits(exc_bits, gen_exit_bits),
            _ => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_close_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_generator_close(gen_bits);
        if exception_pending(_py) {
            return res;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
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

