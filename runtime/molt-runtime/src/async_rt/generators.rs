use crate::PyToken;
#[cfg(test)]
use crate::missing_bits;
use std::sync::OnceLock;

use molt_obj_model::MoltObject;

use super::generators_async::molt_future_new;
use crate::object::accessors::resolve_obj_ptr;
use crate::object::{
    HEADER_FLAG_COROUTINE, ObjectAuxPreselection, object_init_poll_fn_unpublished,
    object_init_shape_unpublished, object_init_state_unpublished, object_state,
    task_shape_for_poll_fn,
};
use crate::{
    ACTIVE_EXCEPTION_STACK, ASYNCGEN_CONTROL_SIZE, ASYNCGEN_FINALIZER_OFFSET,
    ASYNCGEN_FIRSTITER_OFFSET, ASYNCGEN_GEN_OFFSET, ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT,
    ASYNCGEN_OP_ASEND, ASYNCGEN_OP_ATHROW, ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET,
    GEN_CLOSED_OFFSET, GEN_CONTROL_SIZE, GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET,
    GEN_YIELD_FROM_OFFSET, HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED, MoltHeader,
    TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR, TYPE_ID_ASYNC_GENERATOR,
    TYPE_ID_GENERATOR, TYPE_ID_OBJECT, TYPE_ID_TUPLE, alloc_exception, alloc_object,
    alloc_object_with_aux, alloc_tuple, asyncgen_poll_fn_addr, call_callable1, call_poll_fn,
    clear_exception, context_stack_store, context_stack_take, current_task_ptr, dec_ref_bits,
    exception_clear_reason_set, exception_context_align_depth, exception_context_fallback_pop,
    exception_context_fallback_push, exception_pending, exception_stack_depth,
    exception_stack_set_depth, exception_type_bits_from_name, generator_context_stack_store,
    generator_context_stack_take, generator_exception_stack_store, generator_exception_stack_take,
    generator_raise_active, header_from_obj_ptr, inc_ref_bits, is_truthy, maybe_ptr_from_bits,
    molt_exception_clear, molt_exception_last, molt_exception_set_last, molt_raise, obj_from_bits,
    object_mark_has_ptrs, object_type_id, pending_bits_i64, ptr_from_bits, raise_exception,
    register_task_execution, resolve_task_ptr, runtime_state, set_generator_raise, to_i64,
    token_id_from_bits, type_name,
};

pub(crate) fn promise_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
            Some("1")
        )
    })
}

pub(crate) fn sleep_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| matches!(std::env::var("MOLT_TRACE_SLEEP").ok().as_deref(), Some("1")))
}

pub(crate) fn asyncio_connect_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_TRACE_ASYNCIO_CONNECT").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

#[inline]
pub(crate) fn debug_current_task() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1"))
}

unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    unsafe { ptr.add(offset) as *mut u64 }
}

unsafe fn generator_set_slot(_py: &PyToken<'_>, ptr: *mut u8, offset: usize, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, offset, bits);
    }
}

pub(crate) unsafe fn generator_closed(ptr: *mut u8) -> bool {
    unsafe {
        let bits = *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET);
        obj_from_bits(bits).as_bool().unwrap_or(false)
    }
}

unsafe fn generator_set_closed(_py: &PyToken<'_>, ptr: *mut u8, closed: bool) {
    unsafe {
        crate::gil_assert();
        let was_closed = generator_closed(ptr);
        let bits = MoltObject::from_bool(closed).bits();
        generator_set_slot(_py, ptr, GEN_CLOSED_OFFSET, bits);
        if closed && !was_closed {
            // The terminal transition hands the generator frame's bindings to
            // a frame object that shares them, or releases them.
            crate::builtins::frames::activation_exit_bindings(_py, ptr);
        }
    }
}

pub(crate) unsafe fn generator_running(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        ((*header).load_synchronized_flags() & HEADER_FLAG_GEN_RUNNING) != 0
    }
}

struct GeneratorRunningGuard(*mut MoltHeader);

impl GeneratorRunningGuard {
    unsafe fn try_enter(ptr: *mut u8) -> Option<Self> {
        unsafe {
            let header = header_from_obj_ptr(ptr);
            (*header)
                .try_set_flags_unless(HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_RUNNING)
                .then_some(Self(header))
        }
    }
}

impl Drop for GeneratorRunningGuard {
    fn drop(&mut self) {
        unsafe {
            (*self.0).take_flags(HEADER_FLAG_GEN_RUNNING);
        }
    }
}

pub(crate) unsafe fn generator_started(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        ((*header).load_synchronized_flags() & HEADER_FLAG_GEN_STARTED) != 0
    }
}

pub(crate) unsafe fn generator_yieldfrom_bits(ptr: *mut u8) -> u64 {
    unsafe { *generator_slot_ptr(ptr, GEN_YIELD_FROM_OFFSET) }
}

pub(crate) fn resolve_sleep_target(py: &PyToken<'_>, future_ptr: *mut u8) -> *mut u8 {
    super::scheduler::await_chain_terminal(py, future_ptr).unwrap_or(std::ptr::null_mut())
}

unsafe fn generator_set_started(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        let header = header_from_obj_ptr(ptr);
        (*header).fetch_or_flags(HEADER_FLAG_GEN_STARTED);
    }
}

unsafe fn generator_pending_throw(ptr: *mut u8) -> bool {
    unsafe {
        let bits = *generator_slot_ptr(ptr, GEN_THROW_OFFSET);
        !obj_from_bits(bits).is_none()
    }
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
        let elems = crate::object::seq_access::snapshot(
            _py,
            ptr,
            "generator result snapshot allocation failed",
        )?;
        if elems.len() < 2 {
            return None;
        }
        let done = is_truthy(_py, obj_from_bits(elems[1]));
        Some((elems[0], done))
    }
}

pub(crate) unsafe fn raise_stop_iteration_from_value(py: &PyToken<'_>, value: u64) -> u64 {
    let values = if obj_from_bits(value).is_none() {
        &[][..]
    } else {
        std::slice::from_ref(&value)
    };
    let args = alloc_tuple(py, values);
    if args.is_null() {
        return MoltObject::none().bits();
    }
    let args = MoltObject::from_ptr(args).bits();
    let exception = crate::builtins::exceptions::molt_exception_new_from_class(
        exception_type_bits_from_name(py, "StopIteration"),
        args,
    );
    dec_ref_bits(py, args);
    if exception_pending(py) {
        dec_ref_bits(py, exception);
        return MoltObject::none().bits();
    }
    let raised = molt_raise(exception);
    dec_ref_bits(py, exception);
    raised
}

pub(crate) unsafe fn generator_method_result(py: &PyToken<'_>, result: u64) -> u64 {
    if let Some((value, done)) = generator_unpack_pair(py, result) {
        inc_ref_bits(py, value);
        dec_ref_bits(py, result);
        if done {
            let raised = unsafe { raise_stop_iteration_from_value(py, value) };
            dec_ref_bits(py, value);
            raised
        } else {
            value
        }
    } else {
        result
    }
}
/// Allocate every task kind with explicitly selected code and namespace custody.
fn task_new_with_context(
    _py: &PyToken<'_>,
    poll_fn_addr: u64,
    closure_size: u64,
    kind_bits: u64,
    context: [u64; 3],
) -> u64 {
    let Some(closure_size) = crate::provenance::abi::address(closure_size) else {
        return raise_exception::<_>(
            _py,
            "MemoryError",
            "task closure size exceeds the active address space",
        );
    };
    let trace_alloc = matches!(
        std::env::var("MOLT_TRACE_GENERATOR_ALLOC").ok().as_deref(),
        Some("1")
    );
    if trace_alloc {
        eprintln!(
            "molt_task_new enter poll_fn=0x{:x} closure_size={} kind={}",
            poll_fn_addr, closure_size, kind_bits
        );
    }
    let (type_id, is_coroutine) = match kind_bits {
        TASK_KIND_FUTURE => (TYPE_ID_OBJECT, false),
        TASK_KIND_COROUTINE => (TYPE_ID_OBJECT, true),
        TASK_KIND_GENERATOR => (TYPE_ID_GENERATOR, false),
        _ => {
            return raise_exception::<_>(_py, "TypeError", "unknown task kind");
        }
    };
    if type_id == TYPE_ID_GENERATOR && closure_size < GEN_CONTROL_SIZE {
        return raise_exception::<_>(_py, "TypeError", "generator task closure too small");
    }
    let Some(total_size) = std::mem::size_of::<MoltHeader>().checked_add(closure_size) else {
        return raise_exception::<_>(_py, "MemoryError", "task allocation size overflow");
    };
    let ptr = if type_id == TYPE_ID_OBJECT {
        alloc_object_with_aux(_py, total_size, type_id, ObjectAuxPreselection::Sidecar)
    } else {
        // Generator kinds select a sidecar from their type authority.
        alloc_object(_py, total_size, type_id)
    };
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let slots = closure_size / std::mem::size_of::<u64>();
        if slots > 0 {
            let payload_ptr = ptr as *mut u64;
            for idx in 0..slots {
                *payload_ptr.add(idx) = MoltObject::none().bits();
            }
        }
        let header = header_from_obj_ptr(ptr);
        // Compiled task constructors populate tagged capture slots directly
        // after this call. Their layout permits heap owners even while the
        // current words are None; exclude non-owning field fast paths before
        // any backend publishes captures into the payload.
        crate::object::object_mark_has_ptrs(_py, ptr);
        if !object_init_poll_fn_unpublished(ptr, poll_fn_addr)
            || !object_init_shape_unpublished(ptr, task_shape_for_poll_fn(poll_fn_addr))
            || !object_init_state_unpublished(ptr, 0)
            || !crate::object::aux_header::object_init_frame_context_unpublished(
                _py, ptr, context[0], context[1], context[2],
            )
        {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            if !crate::exception_pending(_py) {
                return raise_exception::<_>(_py, "MemoryError", "task context allocation failed");
            }
            return MoltObject::none().bits();
        }
        if is_coroutine {
            (*header).fetch_or_flags(HEADER_FLAG_COROUTINE);
        }
        if type_id == TYPE_ID_GENERATOR && closure_size >= GEN_CONTROL_SIZE {
            *generator_slot_ptr(ptr, GEN_SEND_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_THROW_OFFSET) = MoltObject::none().bits();
            *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET) = MoltObject::from_bool(false).bits();
            *generator_slot_ptr(ptr, GEN_EXC_DEPTH_OFFSET) = MoltObject::from_int(1).bits();
        }
    }
    if trace_alloc {
        eprintln!(
            "molt_task_new ok ptr=0x{:x} bits=0x{:x} type_id={}",
            ptr as usize,
            MoltObject::from_ptr(ptr).bits(),
            type_id
        );
    }
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_task_new(poll_fn_addr: u64, closure_size: u64, kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pending =
            crate::builtins::frames::acquire_pending_invocation_context(_py, poll_fn_addr);
        // Generated callable trampolines carry the exact invocation context.
        // Runtime-native tasks have no Python code owner; ambient caller frames
        // are not a substitute for an actual callable handoff.
        let context = pending.unwrap_or([0; 3]);
        let result = task_new_with_context(_py, poll_fn_addr, closure_size, kind_bits, context);
        if let Some(context) = pending {
            for bits in context {
                dec_ref_bits(_py, bits);
            }
        }
        result
    })
}

/// Native coroutine identity is a runtime fact, never a public attribute.
pub(crate) fn is_native_coroutine_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && ((*header_from_obj_ptr(ptr)).load_metadata_flags() & HEADER_FLAG_COROUTINE) != 0
    })
}

/// Internal poll future adapter admission is a physical runtime invariant.
/// A generator's poll function never grants the __await__ protocol.
pub(crate) fn is_native_poll_future_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && ((*header_from_obj_ptr(ptr)).load_metadata_flags() & HEADER_FLAG_COROUTINE) == 0
            && crate::object_class_bits(ptr) == 0
            && crate::object::object_poll_fn(ptr) != 0
    })
}

/// types.coroutine publishes its protocol flag on the captured code object.
/// Reading gi_code on an arbitrary object admits spoofed attributes and hooks.
pub(crate) fn is_iterable_coroutine_bits(bits: u64) -> bool {
    maybe_ptr_from_bits(bits).is_some_and(|ptr| unsafe {
        if object_type_id(ptr) != TYPE_ID_GENERATOR {
            return false;
        }
        let code = crate::object::aux_header::object_frame_code_bits(ptr);
        maybe_ptr_from_bits(code).is_some_and(|code| {
            object_type_id(code) == crate::TYPE_ID_CODE
                && (crate::object::layout::code_protocol_flags(code)
                    & crate::object::layout::CO_ITERABLE_COROUTINE)
                    != 0
        })
    })
}

pub(crate) fn is_native_python_awaitable_bits(bits: u64) -> bool {
    is_native_coroutine_bits(bits) || is_iterable_coroutine_bits(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_native_awaitable(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(
            is_native_python_awaitable_bits(val_bits) || is_native_poll_future_bits(val_bits),
        )
        .bits()
    })
}

/// # Safety
/// - `task_bits` must be a valid pointer to a Molt task with a valid header.
/// - `token_bits` must be an integer cancel token id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_task_register_execution(
    task_bits: u64,
    token_bits: u64,
    context_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(task_ptr) = resolve_task_ptr(task_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        let id = match token_id_from_bits(token_bits) {
            Some(id) => id,
            None => return raise_exception::<_>(_py, "TypeError", "cancel token id must be int"),
        };
        let context = if obj_from_bits(context_bits).is_none() {
            super::cancellation::TaskContextBinding::Inherited
        } else if crate::builtins::contextvars::is_context(context_bits) {
            super::cancellation::TaskContextBinding::Owned(context_bits)
        } else {
            return raise_exception::<_>(_py, "TypeError", "context must be a Context");
        };
        register_task_execution(_py, task_ptr, id, context);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_generator(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let is_gen = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_GENERATOR });
        MoltObject::from_bool(is_gen).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_send(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace = matches!(
            std::env::var("MOLT_TRACE_GENERATOR_STATE").ok().as_deref(),
            Some("1")
        );
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            let Some(_running_guard) = GeneratorRunningGuard::try_enter(ptr) else {
                return raise_exception::<_>(_py, "ValueError", "generator already executing");
            };
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
            let state_before = object_state(ptr);
            let res = call_poll_fn(_py, poll_fn_addr, ptr);
            let state_after = object_state(ptr);
            set_generator_raise(prev_raise);
            if trace {
                eprintln!(
                    "[molt generator_send] state_before={} state_after={} result_type={} result_bits=0x{:x}",
                    state_before,
                    state_after,
                    type_name(_py, obj_from_bits(res as u64)),
                    res
                );
            }
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
            exception_context_fallback_pop(_py);
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(_py, "TypeError", "expected generator");
            }
            // Normalize a leaf before activation: exception constructors and
            // subclass checks may legally resume or close this generator.
            // Re-read terminal state afterward. Delegated arguments stay raw
            // in the existing tuple carrier; an exception instance in the
            // suspension slot means direct injection into its continuation.
            let pending = if generator_closed(ptr)
                || !generator_started(ptr)
                || obj_from_bits(generator_yieldfrom_bits(ptr)).is_none()
            {
                let Some(exception) =
                    super::throw_protocol::normalize_throw_argument(_py, exc_bits)
                else {
                    return MoltObject::none().bits();
                };
                exception
            } else if maybe_ptr_from_bits(exc_bits)
                .is_some_and(|arguments| object_type_id(arguments) == TYPE_ID_TUPLE)
            {
                inc_ref_bits(_py, exc_bits);
                exc_bits
            } else {
                let arguments = alloc_tuple(_py, &[exc_bits]);
                if arguments.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(arguments).bits()
            };
            let Some(_running_guard) = GeneratorRunningGuard::try_enter(ptr) else {
                dec_ref_bits(_py, pending);
                return raise_exception::<_>(_py, "ValueError", "generator already executing");
            };
            if generator_closed(ptr) {
                // A closed activation owns no frame; the exception simply
                // propagates from the caller.
                crate::molt_exception_trace_prepend(pending);
                let result = molt_raise(pending);
                dec_ref_bits(_py, pending);
                return result;
            }
            if !generator_started(ptr) {
                // Raise at the created activation's first instruction: one
                // real target frame, entered and left without running the
                // body, under this call's execution custody.
                let frame = crate::builtins::frames::ActivationFrameScope::enter(_py, ptr);
                generator_set_closed(_py, ptr, true);
                let Ok(_frame) = frame else {
                    dec_ref_bits(_py, pending);
                    return MoltObject::none().bits();
                };
                crate::molt_exception_trace_prepend(pending);
                let result = molt_raise(pending);
                dec_ref_bits(_py, pending);
                return result;
            }
            generator_set_slot(_py, ptr, GEN_THROW_OFFSET, pending);
            dec_ref_bits(_py, pending);
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
            let res = call_poll_fn(_py, poll_fn_addr, ptr);
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
            exception_context_fallback_pop(_py);
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
        let Some(_running_guard) = GeneratorRunningGuard::try_enter(ptr) else {
            return raise_exception::<_>(_py, "ValueError", "generator already executing");
        };
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
        let res = call_poll_fn(_py, poll_fn_addr, ptr);
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
        exception_context_fallback_pop(_py);
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

unsafe fn generator_raise_from_pending(py: &PyToken<'_>, ptr: *mut u8, exception: u64) -> u64 {
    unsafe {
        generator_set_closed(py, ptr, true);
        super::throw_protocol::raise_body_exception(py, ptr, exception)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_close(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(py, "TypeError", "expected generator");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_GENERATOR {
                return raise_exception::<_>(py, "TypeError", "expected generator");
            }
            if generator_running(ptr) {
                return raise_exception::<_>(py, "ValueError", "generator already executing");
            }
            if generator_closed(ptr) || !generator_started(ptr) {
                generator_set_closed(py, ptr, true);
                return MoltObject::none().bits();
            }
            let exit = alloc_exception(py, "GeneratorExit", "");
            if exit.is_null() {
                return MoltObject::none().bits();
            }
            let exit = MoltObject::from_ptr(exit).bits();
            // Throw owns the activation, running guard and delegated close. Its
            // iterator protocol consumes delegated StopIteration as a return,
            // while the body boundary converts an escaping StopIteration.
            let result = molt_generator_throw(gen_bits, exit);
            dec_ref_bits(py, exit);
            if exception_pending(py) {
                dec_ref_bits(py, result);
                let exception = molt_exception_last();
                let is_exit = crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "GeneratorExit",
                );
                if is_exit {
                    clear_exception(py);
                    generator_set_closed(py, ptr, true);
                }
                dec_ref_bits(py, exception);
                return MoltObject::none().bits();
            }
            let outcome = generator_unpack_pair(py, result);
            if let Some((_, false)) = outcome {
                dec_ref_bits(py, result);
                return raise_exception::<_>(py, "RuntimeError", "generator ignored GeneratorExit");
            }
            let returned = if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                outcome
                    .map(|(value, _)| value)
                    .unwrap_or(MoltObject::none().bits())
            } else {
                MoltObject::none().bits()
            };
            inc_ref_bits(py, returned);
            generator_set_closed(py, ptr, true);
            dec_ref_bits(py, result);
            returned
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_next_method(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_generator_send(gen_bits, MoltObject::none().bits());
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_send_method(gen_bits: u64, send_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_generator_send(gen_bits, send_bits);
        if exception_pending(_py) {
            return res;
        }
        unsafe { generator_method_result(_py, res) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_throw_method(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((receiver, arguments)) = super::throw_protocol::parse_throw_call(py, args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        let result = molt_generator_throw(receiver, arguments);
        dec_ref_bits(py, arguments);
        if exception_pending(py) {
            result
        } else {
            unsafe { generator_method_result(py, result) }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generator_close_method(gen_bits: u64) -> u64 {
    molt_generator_close(gen_bits)
}

unsafe fn asyncgen_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    unsafe { ptr.add(offset) as *mut u64 }
}

pub(crate) unsafe fn asyncgen_gen_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_GEN_OFFSET) }
}

pub(crate) unsafe fn asyncgen_running_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_RUNNING_OFFSET) }
}

pub(crate) unsafe fn asyncgen_pending_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_PENDING_OFFSET) }
}

pub(crate) unsafe fn asyncgen_finalizer_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_FINALIZER_OFFSET) }
}

/// A generator's finalizer is captured at first iteration, independent of the
/// thread's later hook changes. The snapshot owns both references across calls.
fn asyncgen_hooks_snapshot(py: &PyToken<'_>) -> [u64; 2] {
    let hooks = runtime_state(py).asyncgen_hooks.lock().unwrap();
    let values = hooks
        .get(&std::thread::current().id())
        .map(|hooks| [hooks.firstiter, hooks.finalizer])
        .unwrap_or([MoltObject::none().bits(); 2]);
    for bits in values {
        inc_ref_bits(py, bits);
    }
    values
}

pub(crate) unsafe fn asyncgen_needs_finalizer(ptr: *mut u8) -> bool {
    unsafe {
        maybe_ptr_from_bits(asyncgen_gen_bits(ptr)).is_some_and(|generator| {
            object_type_id(generator) == TYPE_ID_GENERATOR && !generator_closed(generator)
        })
    }
}

pub(crate) unsafe fn asyncgen_firstiter_bits(ptr: *mut u8) -> u64 {
    unsafe { *asyncgen_slot_ptr(ptr, ASYNCGEN_FIRSTITER_OFFSET) }
}

pub(crate) unsafe fn asyncgen_visit_owned_edges(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    unsafe {
        visit(asyncgen_pending_bits(ptr));
        visit(asyncgen_running_bits(ptr));
        visit(asyncgen_gen_bits(ptr));
        visit(asyncgen_finalizer_bits(ptr));
    }
}

pub(crate) unsafe fn asyncgen_detach_owned_edges(
    ptr: *mut u8,
    sink: &mut crate::object::heap_lifecycle::DetachedEdgeSink,
) {
    unsafe {
        let pending = crate::object::payload_refs::take(ptr, ASYNCGEN_PENDING_OFFSET);
        let running = crate::object::payload_refs::take(ptr, ASYNCGEN_RUNNING_OFFSET);
        let generator_bits = crate::object::payload_refs::take(ptr, ASYNCGEN_GEN_OFFSET);
        let finalizer = crate::object::payload_refs::take(ptr, ASYNCGEN_FINALIZER_OFFSET);
        sink.detach_if_heap(pending);
        sink.detach_if_heap(running);
        sink.detach_if_heap(generator_bits);
        sink.detach_if_heap(finalizer);
    }
}

unsafe fn asyncgen_set_firstiter_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_FIRSTITER_OFFSET, bits);
    }
}

unsafe fn asyncgen_firstiter_called(ptr: *mut u8) -> bool {
    unsafe {
        obj_from_bits(asyncgen_firstiter_bits(ptr))
            .as_bool()
            .unwrap_or(false)
    }
}

unsafe fn asyncgen_set_running_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_RUNNING_OFFSET, bits);
    }
}

unsafe fn asyncgen_set_pending_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    unsafe {
        crate::object::payload_refs::store_borrowed(_py, ptr, ASYNCGEN_PENDING_OFFSET, bits);
    }
}

unsafe fn asyncgen_clear_pending_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        asyncgen_set_pending_bits(_py, ptr, MoltObject::none().bits());
    }
}

unsafe fn asyncgen_clear_running_bits(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        asyncgen_set_running_bits(_py, ptr, MoltObject::none().bits());
    }
}

pub(crate) unsafe fn asyncgen_running(ptr: *mut u8) -> bool {
    unsafe { !obj_from_bits(asyncgen_running_bits(ptr)).is_none() }
}

pub(crate) unsafe fn asyncgen_await_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let Some(running_ptr) = maybe_ptr_from_bits(asyncgen_running_bits(ptr)) else {
            return MoltObject::none().bits();
        };
        let Some(gen_ptr) = maybe_ptr_from_bits(asyncgen_gen_bits(ptr)) else {
            return MoltObject::none().bits();
        };
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR || generator_running(gen_ptr) {
            return MoltObject::none().bits();
        }
        // The asyncgen operation owns CURRENT_TASK while its generator body polls.
        // Its semantic await edge survives scheduler wakeups and retains the exact
        // acquired Python awaitable independently of public local variable names.
        let awaited = crate::object::aux_header::object_frame_awaited_bits(running_ptr);
        if awaited == 0 {
            return MoltObject::none().bits();
        }
        crate::async_rt::awaitable::python_awaited_bits(_py, awaited)
    }
}

pub(crate) unsafe fn asyncgen_code_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let gen_bits = asyncgen_gen_bits(ptr);
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return MoltObject::none().bits();
        };
        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
            return MoltObject::none().bits();
        }
        let code_bits = crate::object::aux_header::object_frame_code_bits(gen_ptr);
        if code_bits == 0 {
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, code_bits);
        code_bits
    }
}

unsafe fn asyncgen_call_firstiter_if_needed(
    _py: &PyToken<'_>,
    asyncgen_bits: u64,
    asyncgen_ptr: *mut u8,
) -> Option<u64> {
    unsafe {
        if asyncgen_firstiter_called(asyncgen_ptr) {
            return None;
        }
        asyncgen_set_firstiter_bits(_py, asyncgen_ptr, MoltObject::from_bool(true).bits());
        let [hook_bits, finalizer_bits] = asyncgen_hooks_snapshot(_py);
        crate::object::payload_refs::store_owned(
            _py,
            asyncgen_ptr,
            ASYNCGEN_FINALIZER_OFFSET,
            finalizer_bits,
        );
        if obj_from_bits(hook_bits).is_none() {
            return None;
        }
        let res_bits = call_callable1(_py, hook_bits, asyncgen_bits);
        dec_ref_bits(_py, hook_bits);
        if res_bits != 0 {
            dec_ref_bits(_py, res_bits);
        }
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            clear_exception(_py);
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return Some(raised);
        }
        None
    }
}

/// Called only inside the shared object-finalization revival window.
/// The object's captured finalizer participates in ordinary GC edge traversal.
pub(crate) unsafe fn asyncgen_call_finalizer(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        if !asyncgen_needs_finalizer(ptr) {
            return;
        }
        let hook = asyncgen_finalizer_bits(ptr);
        inc_ref_bits(py, hook);
        let _hook_owner = obj_from_bits(hook).as_ptr().map(crate::PtrDropGuard::new);
        let generator = MoltObject::from_ptr(ptr).bits();
        crate::builtins::exceptions::run_unraisable_with_policy(
            py,
            || (generator, None),
            || {
                let _scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
                let result = if obj_from_bits(hook).is_none() {
                    molt_generator_close(asyncgen_gen_bits(ptr))
                } else {
                    call_callable1(py, hook, generator)
                };
                dec_ref_bits(py, result);
            },
        );
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
    unsafe {
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_new(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            *payload_ptr.add(4) = MoltObject::none().bits();
            object_mark_has_ptrs(_py, ptr);
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_hooks_get() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hooks = asyncgen_hooks_snapshot(_py);
        let ptr = alloc_tuple(_py, &hooks);
        for bits in hooks {
            dec_ref_bits(_py, bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[derive(Clone, Copy)]
enum AsyncGenHook {
    Firstiter,
    Finalizer,
}

fn validate_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook, bits: u64) -> Result<(), u64> {
    let name = match slot {
        AsyncGenHook::Firstiter => "firstiter",
        AsyncGenHook::Finalizer => "finalizer",
    };
    let object = obj_from_bits(bits);
    if !object.is_none() {
        let callable = crate::builtins::callable::is_callable_impl(py, bits);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if !callable {
            let label = type_name(py, object);
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                &format!("callable {name} expected, got {label}"),
            ));
        }
    }
    Ok(())
}

fn audit_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook) -> bool {
    let event = match slot {
        AsyncGenHook::Firstiter => "sys.set_asyncgen_hook_firstiter",
        AsyncGenHook::Finalizer => "sys.set_asyncgen_hook_finalizer",
    };
    crate::builtins::sys_ext::audit_event_noargs(py, event)
}

fn replace_asyncgen_hook(py: &PyToken<'_>, slot: AsyncGenHook, bits: u64) {
    // Audit callbacks observe the old value. Finalization observes the newly
    // published value, and may reenter or modify either per-thread hook.
    inc_ref_bits(py, bits);
    let old = {
        let mut threads = runtime_state(py).asyncgen_hooks.lock().unwrap();
        let hooks = threads
            .entry(std::thread::current().id())
            .or_insert_with(|| crate::state::runtime_state::AsyncGenHooks {
                firstiter: MoltObject::none().bits(),
                finalizer: MoltObject::none().bits(),
            });
        let target = match slot {
            AsyncGenHook::Firstiter => &mut hooks.firstiter,
            AsyncGenHook::Finalizer => &mut hooks.finalizer,
        };
        std::mem::replace(target, bits)
    };
    dec_ref_bits(py, old);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_hooks_set(
    firstiter_bits: u64,
    finalizer_bits: u64,
    omitted_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let transactional = crate::object::ops_sys::runtime_target_minor(py) >= 13;
        if transactional {
            // CPython 3.13+ validates both arguments before any audit/update.
            for (slot, bits) in [
                (AsyncGenHook::Finalizer, finalizer_bits),
                (AsyncGenHook::Firstiter, firstiter_bits),
            ] {
                if bits != omitted_bits
                    && let Err(raised) = validate_asyncgen_hook(py, slot, bits)
                {
                    return raised;
                }
            }
        }
        let previous = if transactional && firstiter_bits != omitted_bits {
            let bits = {
                let threads = runtime_state(py).asyncgen_hooks.lock().unwrap();
                threads
                    .get(&std::thread::current().id())
                    .map_or(MoltObject::none().bits(), |hooks| hooks.finalizer)
            };
            // This thread owns the hook slot, and registration cannot call
            // Python. Observe its lifetime without retaining it across audits.
            match crate::object::weakref::WeakBorrow::new(py, bits) {
                Ok(observation) => Some(observation),
                Err(raised) => return raised,
            }
        } else {
            None
        };
        for (slot, bits) in [
            (AsyncGenHook::Finalizer, finalizer_bits),
            (AsyncGenHook::Firstiter, firstiter_bits),
        ] {
            if bits == omitted_bits {
                continue;
            }
            if !transactional && let Err(raised) = validate_asyncgen_hook(py, slot, bits) {
                return raised;
            }
            if !audit_asyncgen_hook(py, slot) {
                if matches!(slot, AsyncGenHook::Firstiter)
                    && let Some(previous) = previous
                {
                    // Rollback is audited even when finalizer was omitted.
                    // Audit saves the firstiter failure, restoring it only
                    // on success; a rollback audit failure replaces it.
                    if audit_asyncgen_hook(py, AsyncGenHook::Finalizer)
                        && let Some(bits) = previous.upgrade_owned()
                    {
                        replace_asyncgen_hook(py, AsyncGenHook::Finalizer, bits);
                        dec_ref_bits(py, bits);
                    }
                    // CPython's borrowed rollback pointer may already
                    // be dead. Never dereference/revive such a pointer:
                    // leave the current hook and original failure intact.
                }
                return MoltObject::none().bits();
            }
            replace_asyncgen_hook(py, slot, bits);
        }
        MoltObject::none().bits()
    })
}

/// `inspect.getasyncgenlocals`: the async generator's activation is its inner
/// generator task, projected through the shared layout authority.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_locals(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            let activation = maybe_ptr_from_bits(asyncgen_gen_bits(asyncgen_ptr))
                .filter(|gen_ptr| object_type_id(*gen_ptr) == TYPE_ID_GENERATOR);
            let locals = match activation {
                Some(gen_ptr) => crate::builtins::frames::activation_locals_bits(_py, gen_ptr),
                None => crate::builtins::frames::empty_locals_bits(_py),
            };
            locals.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

/// `inspect.getgeneratorlocals`, projected through the shared layout authority.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gen_locals(gen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not a Python generator");
        };
        unsafe {
            if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                let name = type_name(_py, obj_from_bits(gen_bits));
                let msg = format!("{name} is not a Python generator");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            crate::builtins::frames::activation_locals_bits(_py, gen_ptr)
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_aiter(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_anext(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_asend(asyncgen_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_athrow(asyncgen_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncgen_aclose(asyncgen_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

/// # Safety
/// Caller must pass a valid async-generator awaitable object bits value.
/// The runtime must be initialized and the thread must be allowed to enter
/// the GIL-guarded runtime state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncgen_poll(obj_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
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
            let _header = header_from_obj_ptr(obj_ptr);
            let payload_bytes = crate::object::object_payload_size(obj_ptr);
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
            // The active operation owns the semantic await edge published by
            // its generator body; retain that owner for ag_running/ag_await.
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

            let res_bits = if crate::object::object_state(obj_ptr) != 0 {
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
                            return MoltObject::none().bits() as i64;
                        }
                        molt_generator_throw(gen_bits, arg_bits)
                    }
                    _ => {
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "invalid async generator op",
                        );
                    }
                }
            };

            if exception_pending(_py) {
                if running_bits == running_marker_bits {
                    asyncgen_clear_running_bits(_py, asyncgen_ptr);
                }
                crate::object::object_set_state(obj_ptr, 0);
                let exc_bits = molt_exception_last();
                if op == ASYNCGEN_OP_ACLOSE {
                    let terminal = obj_from_bits(exc_bits).as_ptr().is_some_and(|exc_ptr| {
                        crate::builtins::exceptions::exception_matches_builtin_name(
                            _py,
                            MoltObject::from_ptr(exc_ptr).bits(),
                            "GeneratorExit",
                        ) || crate::builtins::exceptions::exception_matches_builtin_name(
                            _py,
                            MoltObject::from_ptr(exc_ptr).bits(),
                            "StopAsyncIteration",
                        )
                    });
                    if terminal {
                        exception_clear_reason_set("asyncgen_aclose_swallow");
                        molt_exception_clear();
                        dec_ref_bits(_py, exc_bits);
                        generator_set_closed(_py, gen_ptr, true);
                        return MoltObject::none().bits() as i64;
                    }
                }
                dec_ref_bits(_py, exc_bits);
                return res_bits as i64;
            }

            if res_bits as i64 == pending_bits_i64() {
                asyncgen_set_running_bits(_py, asyncgen_ptr, running_marker_bits);
                crate::object::object_set_state(obj_ptr, 1);
                return res_bits as i64;
            }

            if running_bits == running_marker_bits {
                asyncgen_clear_running_bits(_py, asyncgen_ptr);
            }
            crate::object::object_set_state(obj_ptr, 0);

            if let Some((val_bits, done)) = generator_unpack_pair(_py, res_bits) {
                if !done {
                    inc_ref_bits(_py, val_bits);
                }
                dec_ref_bits(_py, res_bits);
                if op == ASYNCGEN_OP_ACLOSE {
                    if done {
                        generator_set_closed(_py, gen_ptr, true);
                        return MoltObject::none().bits() as i64;
                    }
                    if !obj_from_bits(arg_bits).is_none() {
                        asyncgen_set_pending_bits(_py, asyncgen_ptr, arg_bits);
                        generator_set_slot(
                            _py,
                            gen_ptr,
                            GEN_THROW_OFFSET,
                            MoltObject::none().bits(),
                        );
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
}

#[cfg(test)]
mod asyncgen_lifetime_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static RESURRECTED: AtomicU64 = AtomicU64::new(0);
    static ORIGINAL_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REPLACEMENT_CALLS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn original_finalizer(generator: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            ORIGINAL_CALLS.fetch_add(1, Ordering::Relaxed);
            // Models loop.call_soon_threadsafe retaining an aclose awaitable.
            // This must occur before committed death, in the shared revival window.
            inc_ref_bits(py, generator);
            assert_eq!(RESURRECTED.swap(generator, Ordering::Relaxed), 0);
            MoltObject::none().bits()
        })
    }

    extern "C" fn replacement_finalizer(_generator: u64) -> u64 {
        REPLACEMENT_CALLS.fetch_add(1, Ordering::Relaxed);
        MoltObject::none().bits()
    }

    fn callback(py: &PyToken<'_>, address: *const ()) -> u64 {
        let pointer = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!pointer.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(pointer, address) };
        MoltObject::from_ptr(pointer).bits()
    }

    static HOOK_AUDIT_MODE: AtomicUsize = AtomicUsize::new(0);
    static HOOK_AUDIT_EVENTS: std::sync::Mutex<Vec<&'static str>> =
        std::sync::Mutex::new(Vec::new());

    extern "C" fn hook_audit(event: u64, _args: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let mode = HOOK_AUDIT_MODE.load(Ordering::Relaxed);
            match crate::string_obj_to_owned(obj_from_bits(event)).as_deref() {
                Some("sys.set_asyncgen_hook_firstiter") => {
                    HOOK_AUDIT_EVENTS.lock().unwrap().push("firstiter");
                    if mode != 0 {
                        return raise_exception::<u64>(py, "RuntimeError", "firstiter veto");
                    }
                }
                Some("sys.set_asyncgen_hook_finalizer") => {
                    let rollback = {
                        let mut events = HOOK_AUDIT_EVENTS.lock().unwrap();
                        let rollback = events.contains(&"firstiter");
                        events.push("finalizer");
                        rollback
                    };
                    if mode == 2 && rollback {
                        return raise_exception::<u64>(py, "LookupError", "rollback veto");
                    }
                }
                _ => {}
            }
            MoltObject::none().bits()
        })
    }

    #[test]
    fn asyncgen_hook_audited_rollback_obeys_target_version_and_dead_identity_safety() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let omitted = missing_bits(py);
            let first = callback(py, original_finalizer as *const ());
            let old_finalizer = callback(py, replacement_finalizer as *const ());
            let replacement = callback(py, replacement_finalizer as *const ());
            let audit_ptr = crate::object::builders::alloc_function_obj(
                py,
                crate::provenance::abi::expose_function_address(hook_audit as *const ()),
                2,
            );
            assert!(!audit_ptr.is_null());
            unsafe {
                crate::object::layout::function_set_call_target_ptr(
                    audit_ptr,
                    hook_audit as *const (),
                );
            }
            let audit = MoltObject::from_ptr(audit_ptr).bits();
            crate::builtins::sys_ext::molt_sys_addaudithook(audit);
            let saved_version =
                crate::object::ops_sys::runtime_target_python_info(runtime_state(py));
            for minor in [12, 13, 14] {
                let mut version = saved_version.clone();
                version.minor = minor;
                *runtime_state(py).sys_version_info.lock().unwrap() = Some(version);
                HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                molt_asyncgen_hooks_set(first, old_finalizer, omitted);
                HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                molt_asyncgen_hooks_set(MoltObject::from_int(1).bits(), replacement, omitted);
                assert!(exception_pending(py));
                clear_exception(py);
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(
                    hooks,
                    [
                        first,
                        if minor >= 13 {
                            old_finalizer
                        } else {
                            replacement
                        }
                    ]
                );
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
                assert_eq!(
                    *HOOK_AUDIT_EVENTS.lock().unwrap(),
                    if minor >= 13 {
                        vec![]
                    } else {
                        vec!["finalizer"]
                    }
                );
                for (mode, expected_finalizer, expected_error) in [
                    (
                        1,
                        if minor >= 13 {
                            old_finalizer
                        } else {
                            replacement
                        },
                        "RuntimeError",
                    ),
                    (
                        2,
                        replacement,
                        if minor >= 13 {
                            "LookupError"
                        } else {
                            "RuntimeError"
                        },
                    ),
                ] {
                    HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                    molt_asyncgen_hooks_set(first, old_finalizer, omitted);
                    HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                    HOOK_AUDIT_MODE.store(mode, Ordering::Relaxed);
                    molt_asyncgen_hooks_set(replacement, replacement, omitted);
                    assert!(exception_pending(py));
                    let exception = crate::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        exception,
                        expected_error
                    ));
                    clear_exception(py);
                    dec_ref_bits(py, exception);
                    let hooks = asyncgen_hooks_snapshot(py);
                    assert_eq!(hooks, [first, expected_finalizer]);
                    for bits in hooks {
                        dec_ref_bits(py, bits);
                    }
                    assert_eq!(
                        *HOOK_AUDIT_EVENTS.lock().unwrap(),
                        if minor >= 13 {
                            vec!["finalizer", "firstiter", "finalizer"]
                        } else {
                            vec!["finalizer", "firstiter"]
                        }
                    );
                }
                // The old callable's only owner is the slot. Replacement must
                // really destroy it before firstiter audit, and rollback must
                // never upgrade that dead identity or leak its borrow entry.
                HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
                let ephemeral = callback(py, replacement_finalizer as *const ());
                molt_asyncgen_hooks_set(first, ephemeral, omitted);
                dec_ref_bits(py, ephemeral);
                let registrations = runtime_state(py)
                    .weakrefs
                    .lock()
                    .unwrap()
                    .borrowed_targets
                    .len();
                HOOK_AUDIT_EVENTS.lock().unwrap().clear();
                HOOK_AUDIT_MODE.store(1, Ordering::Relaxed);
                molt_asyncgen_hooks_set(replacement, replacement, omitted);
                assert!(exception_pending(py));
                clear_exception(py);
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(hooks, [first, replacement]);
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
                assert_eq!(
                    runtime_state(py)
                        .weakrefs
                        .lock()
                        .unwrap()
                        .borrowed_targets
                        .len(),
                    registrations
                );
            }
            HOOK_AUDIT_MODE.store(0, Ordering::Relaxed);
            molt_asyncgen_hooks_set(none, none, omitted);
            *runtime_state(py).sys_version_info.lock().unwrap() = Some(saved_version);
            crate::builtins::sys_ext::sys_ext_clear_state(py, runtime_state(py));
            for bits in [first, old_finalizer, replacement, audit] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn asyncgen_hook_setter_preserves_omission_and_ordered_failures() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let omitted = missing_bits(py);
            let first = callback(py, original_finalizer as *const ());
            let finalizer = callback(py, replacement_finalizer as *const ());
            let check = |expected| {
                let hooks = asyncgen_hooks_snapshot(py);
                assert_eq!(hooks, expected);
                for bits in hooks {
                    dec_ref_bits(py, bits);
                }
            };
            molt_asyncgen_hooks_set(first, finalizer, omitted);
            molt_asyncgen_hooks_set(omitted, omitted, omitted);
            check([first, finalizer]);
            let invalid = MoltObject::from_int(1).bits();
            molt_asyncgen_hooks_set(finalizer, invalid, omitted);
            assert!(exception_pending(py));
            clear_exception(py);
            check([first, finalizer]);
            molt_asyncgen_hooks_set(invalid, first, omitted);
            assert!(exception_pending(py));
            clear_exception(py);
            check([
                first,
                if crate::object::ops_sys::runtime_target_minor(py) >= 13 {
                    finalizer
                } else {
                    first
                },
            ]);
            molt_asyncgen_hooks_set(omitted, none, omitted);
            check([first, none]);
            molt_asyncgen_hooks_set(none, none, omitted);
            dec_ref_bits(py, first);
            dec_ref_bits(py, finalizer);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn asyncgen_captures_finalizer_and_allows_one_safe_resurrection() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            ORIGINAL_CALLS.store(0, Ordering::Relaxed);
            REPLACEMENT_CALLS.store(0, Ordering::Relaxed);
            RESURRECTED.store(0, Ordering::Relaxed);
            let original = callback(py, original_finalizer as *const ());
            let replacement = callback(py, replacement_finalizer as *const ());
            molt_asyncgen_hooks_set(none, original, missing_bits(py));
            let inner = molt_task_new(0, GEN_CONTROL_SIZE as u64, TASK_KIND_GENERATOR);
            assert!(!obj_from_bits(inner).is_none());
            let generator = molt_asyncgen_new(inner);
            assert!(!obj_from_bits(generator).is_none());
            dec_ref_bits(py, inner);
            let pointer = obj_from_bits(generator).as_ptr().unwrap();
            assert!(unsafe { asyncgen_call_firstiter_if_needed(py, generator, pointer) }.is_none());
            molt_asyncgen_hooks_set(none, replacement, missing_bits(py));
            dec_ref_bits(py, original);
            // Captured hook is a real GC-visible owner after hook replacement.
            let mut edges = Vec::new();
            unsafe { asyncgen_visit_owned_edges(pointer, |bits| edges.push(bits)) };
            assert!(edges.contains(&original));
            let observation = crate::object::weakref::WeakBorrow::new(py, generator).unwrap();
            dec_ref_bits(py, generator);
            assert_eq!(observation.upgrade_owned(), Some(generator));
            dec_ref_bits(py, generator);
            assert_eq!(ORIGINAL_CALLS.load(Ordering::Relaxed), 1);
            assert_eq!(REPLACEMENT_CALLS.load(Ordering::Relaxed), 0);
            assert_eq!(RESURRECTED.load(Ordering::Relaxed), generator);
            assert_eq!(unsafe { object_type_id(pointer) }, TYPE_ID_ASYNC_GENERATOR);
            assert!(!unsafe {
                (*header_from_obj_ptr(pointer)).has_flag(crate::object::HEADER_FLAG_DEALLOCATING)
            });
            molt_asyncgen_hooks_set(none, none, missing_bits(py));
            dec_ref_bits(py, replacement);
            dec_ref_bits(py, RESURRECTED.swap(0, Ordering::Relaxed));
            assert!(observation.upgrade_owned().is_none());
            assert_eq!(ORIGINAL_CALLS.load(Ordering::Relaxed), 1);
            assert!(!exception_pending(py));
        });
    }
}

#[cfg(test)]
mod suspension_reference_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static CALLBACK_OWNER: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_OFFSET: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_OBSERVED: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_EMPTY_PREFIX: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_PREFIX: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

    struct CallbackScope;

    impl CallbackScope {
        fn new() -> Self {
            let scope = Self;
            scope.reset();
            scope
        }

        fn reset(&self) {
            CALLBACK_OWNER.store(0, Ordering::Relaxed);
            CALLBACK_OFFSET.store(0, Ordering::Relaxed);
            CALLBACK_OBSERVED.store(0, Ordering::Relaxed);
            CALLBACK_EMPTY_PREFIX.store(0, Ordering::Relaxed);
            for slot in &CALLBACK_PREFIX {
                slot.store(0, Ordering::Relaxed);
            }
        }
    }

    impl Drop for CallbackScope {
        fn drop(&mut self) {
            self.reset();
        }
    }

    extern "C" fn payload(_value: u64) -> u64 {
        MoltObject::none().bits()
    }

    extern "C" fn on_release(_weak: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let bits = CALLBACK_OWNER.swap(0, Ordering::Relaxed);
            if bits == 0 {
                return MoltObject::none().bits();
            }
            let owner = ptr_from_bits(bits);
            let offset = CALLBACK_OFFSET.load(Ordering::Relaxed);
            unsafe {
                let words = CALLBACK_EMPTY_PREFIX.load(Ordering::Relaxed);
                for (index, observed) in CALLBACK_PREFIX.iter().take(words).enumerate() {
                    observed.store(*owner.cast::<u64>().add(index), Ordering::Relaxed);
                }
                CALLBACK_OBSERVED.store(*owner.add(offset).cast::<u64>(), Ordering::Relaxed);
                crate::object::payload_refs::store_borrowed(
                    py,
                    owner,
                    offset,
                    MoltObject::from_int(77).bits(),
                );
            }
            MoltObject::none().bits()
        })
    }

    fn callback(py: &PyToken<'_>, address: *const ()) -> u64 {
        let ptr = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            1,
        );
        assert!(!ptr.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(ptr, address) };
        MoltObject::from_ptr(ptr).bits()
    }

    fn watched(py: &PyToken<'_>, value: u64, callback: u64) -> u64 {
        let class = crate::molt_weakref_reference_type();
        let weak = crate::molt_weakref_new(class, value, callback);
        dec_ref_bits(py, class);
        assert!(!exception_pending(py));
        weak
    }

    #[derive(Clone, Copy)]
    enum Reference {
        Send,
        Throw,
        Pending,
        Running,
    }

    impl Reference {
        fn offset(self) -> usize {
            match self {
                Self::Send => GEN_SEND_OFFSET,
                Self::Throw => GEN_THROW_OFFSET,
                Self::Pending => ASYNCGEN_PENDING_OFFSET,
                Self::Running => ASYNCGEN_RUNNING_OFFSET,
            }
        }

        fn owner(self, py: &PyToken<'_>) -> u64 {
            let inner = molt_task_new(0, GEN_CONTROL_SIZE as u64, TASK_KIND_GENERATOR);
            match self {
                Self::Send | Self::Throw => inner,
                Self::Pending | Self::Running => {
                    let owner = molt_asyncgen_new(inner);
                    dec_ref_bits(py, inner);
                    owner
                }
            }
        }

        unsafe fn store(self, py: &PyToken<'_>, owner: *mut u8, value: u64) {
            unsafe {
                match self {
                    Self::Send | Self::Throw => generator_set_slot(py, owner, self.offset(), value),
                    Self::Pending => asyncgen_set_pending_bits(py, owner, value),
                    Self::Running => asyncgen_set_running_bits(py, owner, value),
                }
            }
        }
    }

    #[test]
    fn suspension_slots_preserve_sole_owner_and_owned_self_assignment() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            for field in [
                Reference::Send,
                Reference::Throw,
                Reference::Pending,
                Reference::Running,
            ] {
                let owner = field.owner(py);
                let ptr = ptr_from_bits(owner);
                let value = callback(py, payload as *const ());
                let weak = watched(py, value, none);
                unsafe { field.store(py, ptr, value) };
                dec_ref_bits(py, value);
                unsafe { field.store(py, ptr, value) };
                let retained = crate::molt_weakref_call(weak);
                assert_eq!(
                    retained, value,
                    "borrowed self assignment finalized the value"
                );
                // The acquired weakref result is a distinct incoming owned edge.
                unsafe {
                    crate::object::payload_refs::store_owned(py, ptr, field.offset(), retained)
                };
                unsafe { field.store(py, ptr, none) };
                assert!(
                    obj_from_bits(crate::molt_weakref_call(weak)).is_none(),
                    "owned self assignment leaked an edge"
                );
                dec_ref_bits(py, weak);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn suspension_prefix_is_fully_detached_before_reentrant_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            let owner = crate::molt_alloc(16);
            let ptr = ptr_from_bits(owner);
            let value = callback(py, payload as *const ());
            let hook = callback(py, on_release as *const ());
            let weak = watched(py, value, hook);
            let later = callback(py, payload as *const ());
            let later_weak = watched(py, later, MoltObject::none().bits());
            unsafe {
                crate::object::payload_refs::store_owned(py, ptr, 0, value);
                crate::object::payload_refs::store_owned(py, ptr, 8, later);
            }
            crate::molt_object_publish_initialized(owner);
            CALLBACK_OWNER.store(owner, Ordering::Relaxed);
            CALLBACK_OFFSET.store(8, Ordering::Relaxed);
            CALLBACK_EMPTY_PREFIX.store(2, Ordering::Relaxed);
            unsafe { crate::object::payload_refs::clear_prefix::<2>(py, ptr) };
            for observed in &CALLBACK_PREFIX {
                assert_eq!(
                    observed.load(Ordering::Relaxed),
                    MoltObject::none().bits(),
                    "callback observed a partially retired payload"
                );
            }
            assert!(obj_from_bits(CALLBACK_OBSERVED.load(Ordering::Relaxed)).is_none());
            assert_eq!(
                unsafe { *ptr.cast::<u64>().add(1) },
                MoltObject::from_int(77).bits()
            );
            assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
            assert!(obj_from_bits(crate::molt_weakref_call(later_weak)).is_none());
            dec_ref_bits(py, weak);
            dec_ref_bits(py, later_weak);
            dec_ref_bits(py, hook);
            dec_ref_bits(py, owner);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn suspension_slot_callback_observes_publication_and_keeps_reentrant_write() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope::new();
            for field in [
                Reference::Send,
                Reference::Throw,
                Reference::Pending,
                Reference::Running,
            ] {
                let owner = field.owner(py);
                let ptr = ptr_from_bits(owner);
                let value = callback(py, payload as *const ());
                let hook = callback(py, on_release as *const ());
                let weak = watched(py, value, hook);
                let incoming = MoltObject::from_ptr(crate::alloc_string(py, b"replacement")).bits();
                unsafe { field.store(py, ptr, value) };
                dec_ref_bits(py, value);
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_OFFSET.store(field.offset(), Ordering::Relaxed);
                CALLBACK_OBSERVED.store(0, Ordering::Relaxed);
                unsafe { field.store(py, ptr, incoming) };
                assert_eq!(CALLBACK_OBSERVED.load(Ordering::Relaxed), incoming);
                assert_eq!(
                    unsafe { *ptr.add(field.offset()).cast::<u64>() },
                    MoltObject::from_int(77).bits()
                );
                assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
                dec_ref_bits(py, incoming);
                dec_ref_bits(py, weak);
                dec_ref_bits(py, hook);
                dec_ref_bits(py, owner);
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn suspension_callback_records_bad_prefix_and_disarms_on_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::molt_alloc(16);
            let ptr = ptr_from_bits(owner);
            assert!(!ptr.is_null());
            let unexpected = MoltObject::from_int(73).bits();
            unsafe {
                ptr.cast::<u64>().write(unexpected);
                ptr.cast::<u64>().add(1).write(MoltObject::none().bits());
            }
            crate::molt_object_publish_initialized(owner);
            let failure = crate::test_support::catch_expected_unwind(|| {
                let _callbacks = CallbackScope::new();
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                CALLBACK_EMPTY_PREFIX.store(2, Ordering::Relaxed);
                let result = on_release(0);
                dec_ref_bits(py, result);
                assert_eq!(CALLBACK_PREFIX[0].load(Ordering::Relaxed), unexpected);
                assert_eq!(CALLBACK_OWNER.load(Ordering::Relaxed), 0);
                CALLBACK_OWNER.store(owner, Ordering::Relaxed);
                panic!("suspension callback rollback control");
            });
            assert_eq!(
                failure
                    .expect_err("rollback control must unwind")
                    .downcast_ref::<&str>(),
                Some(&"suspension callback rollback control")
            );
            assert_eq!(CALLBACK_OWNER.load(Ordering::Relaxed), 0);
            assert_eq!(CALLBACK_EMPTY_PREFIX.load(Ordering::Relaxed), 0);
            dec_ref_bits(py, owner);
            assert!(!exception_pending(py));
        });
    }
}
