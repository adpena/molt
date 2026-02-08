use crate::PyToken;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use molt_obj_model::MoltObject;

use crate::concurrency::GilGuard;
use crate::object::accessors::resolve_obj_ptr;
use crate::object::HEADER_FLAG_COROUTINE;
use crate::{
    alloc_dict_with_pairs, alloc_exception, alloc_list, alloc_object, alloc_tuple,
    async_sleep_poll_fn_addr, async_trace_enabled, asyncgen_poll_fn_addr, asyncgen_registry,
    asyncio_fd_watcher_poll_fn_addr, asyncio_gather_poll_fn_addr,
    asyncio_ready_runner_poll_fn_addr, asyncio_server_accept_loop_poll_fn_addr,
    asyncio_sock_accept_poll_fn_addr, asyncio_sock_connect_poll_fn_addr,
    asyncio_sock_recv_into_poll_fn_addr, asyncio_sock_recv_poll_fn_addr,
    asyncio_sock_recvfrom_into_poll_fn_addr, asyncio_sock_recvfrom_poll_fn_addr,
    asyncio_sock_sendall_poll_fn_addr, asyncio_sock_sendto_poll_fn_addr,
    asyncio_socket_reader_read_poll_fn_addr, asyncio_socket_reader_readline_poll_fn_addr,
    asyncio_stream_reader_read_poll_fn_addr, asyncio_stream_reader_readline_poll_fn_addr,
    asyncio_stream_send_all_poll_fn_addr, asyncio_timer_handle_poll_fn_addr,
    asyncio_wait_for_poll_fn_addr, asyncio_wait_poll_fn_addr, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes, await_waiter_clear, await_waiter_register, await_waiters,
    await_waiters_take, call_callable0, call_callable1, call_callable2, call_callable3,
    call_poll_fn, class_name_for_error, clear_exception, clear_exception_state, current_task_ptr,
    dec_ref_bits, exception_args_bits, exception_clear_reason_set, exception_context_align_depth,
    exception_context_fallback_pop, exception_context_fallback_push, exception_kind_bits,
    exception_pending, exception_stack_depth, exception_stack_set_depth, fn_ptr_code_get,
    generator_exception_stack_store, generator_exception_stack_take, generator_raise_active,
    header_from_obj_ptr, inc_ref_bits, instant_from_monotonic_secs, io_wait_poll_fn_addr,
    is_truthy, maybe_ptr_from_bits, missing_bits, molt_anext, molt_bytes_from_obj, molt_call_bind,
    molt_callargs_expand_star, molt_callargs_new, molt_exception_clear, molt_exception_kind,
    molt_exception_last, molt_exception_set_last, molt_float_from_obj, molt_getitem_method,
    molt_io_wait_new, molt_is_callable, molt_len, molt_raise, molt_set_add, molt_set_new,
    molt_slice_new, molt_socket_reader_read, molt_socket_reader_readline, molt_str_from_obj,
    molt_stream_reader_read, molt_stream_reader_readline, molt_stream_send_obj, obj_from_bits,
    object_class_bits, object_mark_has_ptrs, object_type_id, pending_bits_i64,
    process_poll_fn_addr, promise_poll_fn_addr, ptr_from_bits, raise_cancelled_with_message,
    raise_exception, raise_os_error_errno, register_task_token, resolve_task_ptr, runtime_state,
    seq_vec_ref, set_generator_raise, string_obj_to_owned, task_cancel_message_clear,
    task_cancel_message_set, task_cancel_pending, task_exception_baseline_drop,
    task_exception_depth_drop, task_exception_stack_drop, task_has_token, task_last_exceptions,
    task_mark_done, task_set_cancel_pending, task_take_cancel_pending, task_waiting_on,
    thread_poll_fn_addr, to_f64, to_i64, token_id_from_bits, tuple_from_iter_bits, type_name,
    wake_task_ptr, MoltHeader, PtrSlot, ACTIVE_EXCEPTION_STACK, ASYNCGEN_CONTROL_SIZE,
    ASYNCGEN_FIRSTITER_OFFSET, ASYNCGEN_GEN_OFFSET, ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT,
    ASYNCGEN_OP_ASEND, ASYNCGEN_OP_ATHROW, ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET,
    GEN_CLOSED_OFFSET, GEN_CONTROL_SIZE, GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET,
    GEN_YIELD_FROM_OFFSET, HEADER_FLAG_BLOCK_ON, HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED,
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
const ASYNCIO_WAIT_RETURN_ALL_COMPLETED: i64 = 0;
const ASYNCIO_WAIT_RETURN_FIRST_COMPLETED: i64 = 1;
const ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION: i64 = 2;
const ASYNCIO_WAIT_FLAG_HAS_TIMER: i64 = 1;
const ASYNCIO_WAIT_FLAG_TIMEOUT_READY: i64 = 2;
const ASYNCIO_GATHER_RESULT_OFFSET: usize = 4;
const ASYNCIO_SOCKET_IO_EVENT_READ: i64 = 1;
const ASYNCIO_SOCKET_IO_EVENT_WRITE: i64 = 2;
const ASYNCIO_STREAM_READER_READ_SLOT_READER: usize = 0;
const ASYNCIO_STREAM_READER_READ_SLOT_N: usize = 1;
const ASYNCIO_STREAM_READER_READ_SLOT_WAIT: usize = 2;
const ASYNCIO_STREAM_READER_READLINE_SLOT_READER: usize = 0;
const ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT: usize = 1;
const ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM: usize = 0;
const ASYNCIO_STREAM_SEND_ALL_SLOT_DATA: usize = 1;
const ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT: usize = 2;
const ASYNCIO_SOCKET_READER_READ_SLOT_READER: usize = 0;
const ASYNCIO_SOCKET_READER_READ_SLOT_N: usize = 1;
const ASYNCIO_SOCKET_READER_READ_SLOT_FD: usize = 2;
const ASYNCIO_SOCKET_READER_READ_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_READER: usize = 0;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_FD: usize = 1;
const ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT: usize = 2;
const ASYNCIO_SOCK_RECV_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECV_SLOT_SIZE: usize = 1;
const ASYNCIO_SOCK_RECV_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_RECV_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_CONNECT_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_CONNECT_SLOT_ADDR: usize = 1;
const ASYNCIO_SOCK_CONNECT_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_CONNECT_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_ACCEPT_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_ACCEPT_SLOT_FD: usize = 1;
const ASYNCIO_SOCK_ACCEPT_SLOT_WAIT: usize = 2;
const ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECV_INTO_SLOT_BUF: usize = 1;
const ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES: usize = 2;
const ASYNCIO_SOCK_RECV_INTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT: usize = 4;
const ASYNCIO_SOCK_SENDALL_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_SENDALL_SLOT_DATA: usize = 1;
const ASYNCIO_SOCK_SENDALL_SLOT_TOTAL: usize = 2;
const ASYNCIO_SOCK_SENDALL_SLOT_DLEN: usize = 3;
const ASYNCIO_SOCK_SENDALL_SLOT_FD: usize = 4;
const ASYNCIO_SOCK_SENDALL_SLOT_WAIT: usize = 5;
const ASYNCIO_SOCK_RECVFROM_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECVFROM_SLOT_SIZE: usize = 1;
const ASYNCIO_SOCK_RECVFROM_SLOT_FD: usize = 2;
const ASYNCIO_SOCK_RECVFROM_SLOT_WAIT: usize = 3;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF: usize = 1;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES: usize = 2;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT: usize = 4;
const ASYNCIO_SOCK_SENDTO_SLOT_SOCK: usize = 0;
const ASYNCIO_SOCK_SENDTO_SLOT_DATA: usize = 1;
const ASYNCIO_SOCK_SENDTO_SLOT_ADDR: usize = 2;
const ASYNCIO_SOCK_SENDTO_SLOT_FD: usize = 3;
const ASYNCIO_SOCK_SENDTO_SLOT_WAIT: usize = 4;
const ASYNCIO_TIMER_SLOT_HANDLE: usize = 0;
const ASYNCIO_TIMER_SLOT_DELAY: usize = 1;
const ASYNCIO_TIMER_SLOT_LOOP: usize = 2;
const ASYNCIO_TIMER_SLOT_SCHEDULED: usize = 3;
const ASYNCIO_TIMER_SLOT_READY_LOCK: usize = 4;
const ASYNCIO_TIMER_SLOT_READY: usize = 5;
const ASYNCIO_TIMER_SLOT_WAIT: usize = 6;
const ASYNCIO_FD_WATCHER_SLOT_REGISTRY: usize = 0;
const ASYNCIO_FD_WATCHER_SLOT_FILENO: usize = 1;
const ASYNCIO_FD_WATCHER_SLOT_CALLBACK: usize = 2;
const ASYNCIO_FD_WATCHER_SLOT_ARGS: usize = 3;
const ASYNCIO_FD_WATCHER_SLOT_EVENTS: usize = 4;
const ASYNCIO_FD_WATCHER_SLOT_WAIT: usize = 5;
const ASYNCIO_SERVER_ACCEPT_SLOT_SOCK: usize = 0;
const ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK: usize = 1;
const ASYNCIO_SERVER_ACCEPT_SLOT_LOOP: usize = 2;
const ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR: usize = 3;
const ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR: usize = 4;
const ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE: usize = 5;
const ASYNCIO_SERVER_ACCEPT_SLOT_FD: usize = 6;
const ASYNCIO_SERVER_ACCEPT_SLOT_WAIT: usize = 7;
const ASYNCIO_READY_RUNNER_SLOT_LOOP: usize = 0;
const ASYNCIO_READY_RUNNER_SLOT_READY_LOCK: usize = 1;
const ASYNCIO_READY_RUNNER_SLOT_READY: usize = 2;
const ASYNCIO_READY_RUNNER_SLOT_WAIT: usize = 3;
const WAIT_FOR_STATE_PENDING: i64 = 1;
const WAIT_FOR_STATE_CANCEL_WAIT: i64 = 2;
const WAIT_FOR_FLAG_HAS_TIMER: i64 = 1;
const WAIT_FOR_FLAG_FORCE_TIMEOUT: i64 = 2;

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

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_task_cancel_apply(future_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        if obj_from_bits(msg_bits).is_none() {
            cancel_future_task(_py, task_ptr, None);
        } else {
            cancel_future_task(_py, task_ptr, Some(msg_bits));
        }
        MoltObject::from_bool(true).bits()
    })
}

/// # Safety
/// - `tasks_bits` must be iterable and contain awaitables with `done()`/`cancel()`.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_cancel_pending(tasks_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "task collection must be awaitables");
        };
        let tasks = seq_vec_ref(task_tuple_ptr);
        let mut cancelled_count = 0i64;
        for &task_bits in tasks {
            let Some(done) = asyncio_method_truthy(_py, task_bits, b"done") else {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            };
            if done {
                continue;
            }
            let out_bits = asyncio_call_method0(_py, task_bits, b"cancel");
            if exception_pending(_py) {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            }
            let did_cancel = is_truthy(_py, obj_from_bits(out_bits));
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            if did_cancel {
                cancelled_count += 1;
            }
        }
        dec_ref_bits(_py, task_tuple_bits);
        MoltObject::from_int(cancelled_count).bits()
    })
}

unsafe fn asyncio_ready_batch_run_tuple(_py: &PyToken<'_>, handle_tuple_bits: u64) -> Option<i64> {
    let Some(handle_tuple_ptr) = obj_from_bits(handle_tuple_bits).as_ptr() else {
        let _ =
            raise_exception::<u64>(_py, "TypeError", "ready-handle collection must be iterable");
        return None;
    };
    let handles = seq_vec_ref(handle_tuple_ptr);
    let mut ran_count = 0i64;
    for &handle_bits in handles {
        let cancelled = asyncio_method_truthy(_py, handle_bits, b"cancelled")?;
        if cancelled {
            continue;
        }
        let run_bits = asyncio_call_method0(_py, handle_bits, b"_run");
        if exception_pending(_py) {
            return None;
        }
        if !obj_from_bits(run_bits).is_none() {
            dec_ref_bits(_py, run_bits);
        }
        ran_count += 1;
    }
    Some(ran_count)
}

/// # Safety
/// - `handles_bits` must be iterable and contain asyncio Handle-compatible objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_ready_batch_run(handles_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, handles_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(ran_count) = asyncio_ready_batch_run_tuple(_py, handle_tuple_bits) else {
            dec_ref_bits(_py, handle_tuple_bits);
            return MoltObject::none().bits();
        };
        dec_ref_bits(_py, handle_tuple_bits);
        MoltObject::from_int(ran_count).bits()
    })
}

unsafe fn asyncio_loop_enqueue_handle_inner(
    _py: &PyToken<'_>,
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
    handle_bits: u64,
) -> Option<()> {
    let acquire_bits = asyncio_call_method0(_py, ready_lock_bits, b"acquire");
    if exception_pending(_py) {
        return None;
    }
    let acquired = is_truthy(_py, obj_from_bits(acquire_bits));
    if !obj_from_bits(acquire_bits).is_none() {
        dec_ref_bits(_py, acquire_bits);
    }
    if !acquired {
        let _ = raise_exception::<u64>(_py, "RuntimeError", "ready queue lock acquire failed");
        return None;
    }

    let append_bits = asyncio_call_method1(_py, ready_bits, b"append", handle_bits);
    let append_failed = exception_pending(_py);
    if !obj_from_bits(append_bits).is_none() {
        dec_ref_bits(_py, append_bits);
    }

    let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
    let release_failed = exception_pending(_py);
    if !obj_from_bits(release_bits).is_none() {
        dec_ref_bits(_py, release_bits);
    }
    if append_failed || release_failed {
        return None;
    }

    let running = asyncio_method_truthy(_py, loop_bits, b"is_running")?;
    if running {
        let ensure_bits = asyncio_call_method0(_py, loop_bits, b"_ensure_ready_runner");
        if exception_pending(_py) {
            return None;
        }
        if !obj_from_bits(ensure_bits).is_none() {
            dec_ref_bits(_py, ensure_bits);
        }
    }
    Some(())
}

/// # Safety
/// - `loop_bits` must expose `is_running()` and `_ensure_ready_runner()`.
/// - `ready_lock_bits` must expose `acquire()`/`release()`.
/// - `ready_bits` must expose `append()`.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_loop_enqueue_handle(
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
    handle_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if asyncio_loop_enqueue_handle_inner(
            _py,
            loop_bits,
            ready_lock_bits,
            ready_bits,
            handle_bits,
        )
        .is_none()
        {
            return MoltObject::none().bits();
        }
        MoltObject::from_int(1).bits()
    })
}

/// # Safety
/// - `ready_lock_bits` must be a lock-like object with `acquire()`/`release()`.
/// - `ready_bits` must be a mutable ready-handle queue.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_ready_queue_drain(
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut total_ran = 0i64;
        loop {
            let acquire_bits = asyncio_call_method0(_py, ready_lock_bits, b"acquire");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let acquired = is_truthy(_py, obj_from_bits(acquire_bits));
            if !obj_from_bits(acquire_bits).is_none() {
                dec_ref_bits(_py, acquire_bits);
            }
            if !acquired {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "ready queue lock acquire failed",
                );
            }

            let len_bits = molt_len(ready_bits);
            if exception_pending(_py) {
                let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }
                return MoltObject::none().bits();
            }
            let Some(ready_len) = to_i64(obj_from_bits(len_bits)) else {
                if !obj_from_bits(len_bits).is_none() {
                    dec_ref_bits(_py, len_bits);
                }
                let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                if exception_pending(_py) {
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }
                return raise_exception::<u64>(_py, "TypeError", "ready queue must be sized");
            };
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            if ready_len <= 0 {
                let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                if exception_pending(_py) {
                    if !obj_from_bits(release_bits).is_none() {
                        dec_ref_bits(_py, release_bits);
                    }
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }
                break;
            }

            let Some(handle_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, ready_bits) }) else {
                let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }
                return MoltObject::none().bits();
            };
            let clear_bits = asyncio_call_method0(_py, ready_bits, b"clear");
            let release_bits = asyncio_call_method0(_py, ready_lock_bits, b"release");
            if exception_pending(_py) {
                if !obj_from_bits(clear_bits).is_none() {
                    dec_ref_bits(_py, clear_bits);
                }
                if !obj_from_bits(release_bits).is_none() {
                    dec_ref_bits(_py, release_bits);
                }
                dec_ref_bits(_py, handle_tuple_bits);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(clear_bits).is_none() {
                dec_ref_bits(_py, clear_bits);
            }
            if !obj_from_bits(release_bits).is_none() {
                dec_ref_bits(_py, release_bits);
            }

            let Some(batch_ran) = asyncio_ready_batch_run_tuple(_py, handle_tuple_bits) else {
                dec_ref_bits(_py, handle_tuple_bits);
                return MoltObject::none().bits();
            };
            dec_ref_bits(_py, handle_tuple_bits);
            total_ran += batch_ran;
        }
        MoltObject::from_int(total_ran).bits()
    })
}

/// # Safety
/// - `waiters_bits` must be a deque/list-like object supporting pop-front semantics.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_waiters_notify(
    waiters_bits: u64,
    count_bits: u64,
    result_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "waiter notify count must be int");
        };
        if count <= 0 {
            return MoltObject::from_int(0).bits();
        }
        let len_bits = molt_len(waiters_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return raise_exception::<u64>(_py, "TypeError", "waiter collection must be sized");
        };
        if !obj_from_bits(len_bits).is_none() {
            dec_ref_bits(_py, len_bits);
        }
        if waiters_len <= 0 {
            return MoltObject::from_int(0).bits();
        }
        count = count.min(waiters_len);
        let mut woken_count = 0i64;
        for _ in 0..count {
            let waiter_bits = asyncio_waiters_pop_front(_py, waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                if !obj_from_bits(waiter_bits).is_none() {
                    dec_ref_bits(_py, waiter_bits);
                }
                return MoltObject::none().bits();
            };
            if !done {
                let out_bits = asyncio_call_method1(_py, waiter_bits, b"set_result", result_bits);
                if exception_pending(_py) {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
            }
            if !obj_from_bits(waiter_bits).is_none() {
                dec_ref_bits(_py, waiter_bits);
            }
            woken_count += 1;
        }
        MoltObject::from_int(woken_count).bits()
    })
}

/// # Safety
/// - `waiters_bits` must be a deque/list-like object supporting pop-front semantics.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_waiters_notify_exception(
    waiters_bits: u64,
    count_bits: u64,
    exc_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "waiter notify-exception count must be int",
            );
        };
        if count <= 0 {
            return MoltObject::from_int(0).bits();
        }
        let len_bits = molt_len(waiters_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(waiters_len) = to_i64(obj_from_bits(len_bits)) else {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return raise_exception::<u64>(_py, "TypeError", "waiter collection must be sized");
        };
        if !obj_from_bits(len_bits).is_none() {
            dec_ref_bits(_py, len_bits);
        }
        if waiters_len <= 0 {
            return MoltObject::from_int(0).bits();
        }
        count = count.min(waiters_len);
        let mut woken_count = 0i64;
        for _ in 0..count {
            let waiter_bits = asyncio_waiters_pop_front(_py, waiters_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                if !obj_from_bits(waiter_bits).is_none() {
                    dec_ref_bits(_py, waiter_bits);
                }
                return MoltObject::none().bits();
            };
            if !done {
                let out_bits = asyncio_call_method1(_py, waiter_bits, b"set_exception", exc_bits);
                if exception_pending(_py) {
                    if !obj_from_bits(waiter_bits).is_none() {
                        dec_ref_bits(_py, waiter_bits);
                    }
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
            }
            if !obj_from_bits(waiter_bits).is_none() {
                dec_ref_bits(_py, waiter_bits);
            }
            woken_count += 1;
        }
        MoltObject::from_int(woken_count).bits()
    })
}

/// # Safety
/// - `waiters_bits` must support `remove(waiter)` semantics.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_waiters_remove(waiters_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let out_bits = asyncio_call_method1(_py, waiters_bits, b"remove", waiter_bits);
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        }
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        MoltObject::from_bool(true).bits()
    })
}

/// # Safety
/// - `condition_bits` must be an asyncio.Condition-like object.
/// - `predicate_bits` must be callable.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_condition_wait_for_step(
    condition_bits: u64,
    predicate_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let callable_bits = molt_is_callable(predicate_bits);
        let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
        if !obj_from_bits(callable_bits).is_none() {
            dec_ref_bits(_py, callable_bits);
        }
        if !is_callable {
            return raise_exception::<u64>(_py, "TypeError", "predicate must be callable");
        }

        let predicate_out = call_callable0(_py, predicate_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let done = is_truthy(_py, obj_from_bits(predicate_out));
        let done_bits = MoltObject::from_bool(done).bits();
        if done {
            let out_ptr = alloc_tuple(_py, &[done_bits, predicate_out]);
            if out_ptr.is_null() {
                if !obj_from_bits(predicate_out).is_none() {
                    dec_ref_bits(_py, predicate_out);
                }
                return MoltObject::none().bits();
            }
            if !obj_from_bits(predicate_out).is_none() {
                dec_ref_bits(_py, predicate_out);
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        if !obj_from_bits(predicate_out).is_none() {
            dec_ref_bits(_py, predicate_out);
        }

        let wait_bits = asyncio_call_method0(_py, condition_bits, b"wait");
        if exception_pending(_py) {
            return wait_bits;
        }
        let out_ptr = alloc_tuple(_py, &[done_bits, wait_bits]);
        if out_ptr.is_null() {
            if !obj_from_bits(wait_bits).is_none() {
                dec_ref_bits(_py, wait_bits);
            }
            return MoltObject::none().bits();
        }
        if !obj_from_bits(wait_bits).is_none() {
            dec_ref_bits(_py, wait_bits);
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

/// # Safety
/// - `waiters_bits` must be iterable and contain asyncio Future-compatible waiters.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_barrier_release(waiters_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(waiter_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, waiters_bits) }) else {
            return MoltObject::none().bits();
        };
        let clear_bits = asyncio_call_method0(_py, waiters_bits, b"clear");
        if exception_pending(_py) {
            dec_ref_bits(_py, waiter_tuple_bits);
            return MoltObject::none().bits();
        }
        if !obj_from_bits(clear_bits).is_none() {
            dec_ref_bits(_py, clear_bits);
        }
        let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, waiter_tuple_bits);
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "barrier waiter collection must be iterable",
            );
        };
        let waiters = seq_vec_ref(waiter_tuple_ptr);
        let mut released_count = 0i64;
        for (idx, &waiter_bits) in waiters.iter().enumerate() {
            let Some(done) = asyncio_method_truthy(_py, waiter_bits, b"done") else {
                dec_ref_bits(_py, waiter_tuple_bits);
                return MoltObject::none().bits();
            };
            if done {
                continue;
            }
            let out_bits = asyncio_call_method1(
                _py,
                waiter_bits,
                b"set_result",
                MoltObject::from_int(idx as i64).bits(),
            );
            if exception_pending(_py) {
                dec_ref_bits(_py, waiter_tuple_bits);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            released_count += 1;
        }
        dec_ref_bits(_py, waiter_tuple_bits);
        MoltObject::from_int(released_count).bits()
    })
}

unsafe fn asyncio_transfer_set_target_exception(
    _py: &PyToken<'_>,
    target_bits: u64,
    exc_bits: u64,
) {
    let out_bits = asyncio_call_method1(_py, target_bits, b"set_exception", exc_bits);
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
    if exception_pending(_py) {
        asyncio_clear_pending_exception(_py);
    }
}

/// # Safety
/// - `source_bits`/`target_bits` must be Future-compatible objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_future_transfer(source_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(target_done) = asyncio_method_truthy(_py, target_bits, b"done") else {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        };
        if target_done {
            return MoltObject::from_bool(false).bits();
        }

        let Some(source_cancelled) = asyncio_method_truthy(_py, source_bits, b"cancelled") else {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        };
        if source_cancelled {
            let cancel_msg_ref =
                asyncio_attr_lookup_allow_missing(_py, source_bits, b"_cancel_message");
            let cancel_msg_bits = cancel_msg_ref.unwrap_or_else(|| MoltObject::none().bits());
            let out_bits = asyncio_call_method1(_py, target_bits, b"cancel", cancel_msg_bits);
            if let Some(found_bits) = cancel_msg_ref {
                if !obj_from_bits(found_bits).is_none() {
                    dec_ref_bits(_py, found_bits);
                }
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            return MoltObject::from_bool(true).bits();
        }

        let source_exc_bits = asyncio_call_method0(_py, source_bits, b"exception");
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        }
        let source_has_exc = !obj_from_bits(source_exc_bits).is_none();
        if source_has_exc {
            asyncio_transfer_set_target_exception(_py, target_bits, source_exc_bits);
            dec_ref_bits(_py, source_exc_bits);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
                return MoltObject::from_bool(false).bits();
            }
            return MoltObject::from_bool(true).bits();
        }
        if !obj_from_bits(source_exc_bits).is_none() {
            dec_ref_bits(_py, source_exc_bits);
        }

        let result_bits = asyncio_call_method0(_py, source_bits, b"result");
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        }
        let out_bits = asyncio_call_method1(_py, target_bits, b"set_result", result_bits);
        if !obj_from_bits(result_bits).is_none() {
            dec_ref_bits(_py, result_bits);
        }
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
            return MoltObject::from_bool(false).bits();
        }
        MoltObject::from_bool(true).bits()
    })
}

/// # Safety
/// - `waiters_bits` must be iterable and contain Event waiter futures.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_event_waiters_cleanup(waiters_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(waiter_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, waiters_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, waiter_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "event waiters must be iterable");
        };
        let waiters = seq_vec_ref(waiter_tuple_ptr);
        let mut cleaned = 0i64;
        for &waiter_bits in waiters {
            let Some(owner_bits) =
                asyncio_attr_lookup_allow_missing(_py, waiter_bits, b"_molt_event_owner")
            else {
                continue;
            };
            if obj_from_bits(owner_bits).is_none() {
                dec_ref_bits(_py, owner_bits);
                continue;
            }
            let Some(owner_waiters_bits) =
                asyncio_attr_lookup_allow_missing(_py, owner_bits, b"_waiters")
            else {
                dec_ref_bits(_py, owner_bits);
                continue;
            };
            let out_bits = asyncio_call_method1(_py, owner_waiters_bits, b"remove", waiter_bits);
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            } else {
                cleaned += 1;
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            if !obj_from_bits(owner_waiters_bits).is_none() {
                dec_ref_bits(_py, owner_waiters_bits);
            }
            if !obj_from_bits(owner_bits).is_none() {
                dec_ref_bits(_py, owner_bits);
            }
        }
        dec_ref_bits(_py, waiter_tuple_bits);
        MoltObject::from_int(cleaned).bits()
    })
}

/// # Safety
/// - `tasks_bits` must be a mutable task set.
/// - `errors_bits` must be an appendable error list.
/// - `task_bits` must be a task/future object.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_taskgroup_on_task_done(
    tasks_bits: u64,
    errors_bits: u64,
    task_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let contains_bits = asyncio_call_method1(_py, tasks_bits, b"__contains__", task_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let in_group = is_truthy(_py, obj_from_bits(contains_bits));
        if !obj_from_bits(contains_bits).is_none() {
            dec_ref_bits(_py, contains_bits);
        }
        if !in_group {
            return MoltObject::from_bool(false).bits();
        }

        let discard_bits = asyncio_call_method1(_py, tasks_bits, b"discard", task_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(discard_bits).is_none() {
            dec_ref_bits(_py, discard_bits);
        }

        let Some(mut should_cancel) =
            asyncio_taskgroup_collect_task_error(_py, errors_bits, task_bits)
        else {
            return MoltObject::none().bits();
        };
        if !should_cancel {
            return MoltObject::from_bool(false).bits();
        }

        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "task group must be iterable");
        };
        let tasks = seq_vec_ref(task_tuple_ptr);
        for &other_task_bits in tasks {
            let Some(done) = asyncio_method_truthy(_py, other_task_bits, b"done") else {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            };
            if !done {
                continue;
            }
            let discard_bits = asyncio_call_method1(_py, tasks_bits, b"discard", other_task_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(discard_bits).is_none() {
                dec_ref_bits(_py, discard_bits);
            }
            let Some(collected_error) =
                asyncio_taskgroup_collect_task_error(_py, errors_bits, other_task_bits)
            else {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            };
            should_cancel |= collected_error;
        }
        dec_ref_bits(_py, task_tuple_bits);
        MoltObject::from_bool(should_cancel).bits()
    })
}

/// # Safety
/// - `cancel_callback_bits` must be callable.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_taskgroup_request_cancel(
    loop_bits: u64,
    cancel_callback_bits: u64,
    cancel_handle_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !obj_from_bits(cancel_handle_bits).is_none() {
            return cancel_handle_bits;
        }
        if obj_from_bits(loop_bits).is_none() {
            let out_bits = call_callable0(_py, cancel_callback_bits);
            if exception_pending(_py) {
                return out_bits;
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            return MoltObject::none().bits();
        }
        let out_bits = asyncio_call_method1(_py, loop_bits, b"call_soon", cancel_callback_bits);
        if exception_pending(_py) {
            return out_bits;
        }
        out_bits
    })
}

/// # Safety
/// - `tasks_bits` must be iterable and contain Future-like objects.
/// - `callback_bits` must be callable.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_tasks_add_done_callback(
    tasks_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let callable_bits = molt_is_callable(callback_bits);
        let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
        if !obj_from_bits(callable_bits).is_none() {
            dec_ref_bits(_py, callable_bits);
        }
        if !is_callable {
            return raise_exception::<u64>(_py, "TypeError", "callback must be callable");
        }
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "tasks must be iterable");
        };
        let tasks = seq_vec_ref(task_tuple_ptr);
        let mut attached = 0i64;
        for &task_bits in tasks {
            let out_bits =
                asyncio_call_method1(_py, task_bits, b"add_done_callback", callback_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            attached += 1;
        }
        dec_ref_bits(_py, task_tuple_bits);
        MoltObject::from_int(attached).bits()
    })
}

/// # Safety
/// - `future_bits` must be a valid pointer to a Molt future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_task_uncancel_apply(future_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_ptr) = resolve_task_ptr(future_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not awaitable");
        };
        task_cancel_message_clear(_py, task_ptr);
        let _ = task_take_cancel_pending(task_ptr);
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `future_bits` must be a Future-like object exposing `_run_callback`.
/// - `callbacks_bits` must be iterable of `(callback, context)` pairs.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_future_invoke_callbacks(
    future_bits: u64,
    callbacks_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(callback_tuple_bits) = tuple_from_iter_bits(_py, callbacks_bits) else {
            return MoltObject::none().bits();
        };
        let Some(callback_tuple_ptr) = obj_from_bits(callback_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, callback_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "future callbacks must be iterable");
        };
        let callbacks = seq_vec_ref(callback_tuple_ptr);
        let idx0 = MoltObject::from_int(0).bits();
        let idx1 = MoltObject::from_int(1).bits();
        let mut called = 0i64;
        for &entry_bits in callbacks {
            let fn_bits = molt_getitem_method(entry_bits, idx0);
            if exception_pending(_py) {
                dec_ref_bits(_py, callback_tuple_bits);
                return MoltObject::none().bits();
            }
            let ctx_bits = molt_getitem_method(entry_bits, idx1);
            if exception_pending(_py) {
                if !obj_from_bits(fn_bits).is_none() {
                    dec_ref_bits(_py, fn_bits);
                }
                dec_ref_bits(_py, callback_tuple_bits);
                return MoltObject::none().bits();
            }
            let out_bits =
                asyncio_call_method2(_py, future_bits, b"_run_callback", fn_bits, ctx_bits);
            if !obj_from_bits(fn_bits).is_none() {
                dec_ref_bits(_py, fn_bits);
            }
            if !obj_from_bits(ctx_bits).is_none() {
                dec_ref_bits(_py, ctx_bits);
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, callback_tuple_bits);
                return out_bits;
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            called += 1;
        }
        dec_ref_bits(_py, callback_tuple_bits);
        MoltObject::from_int(called).bits()
    })
}

/// # Safety
/// - `waiters_bits` must be iterable of Event waiters.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_event_set_waiters(
    waiters_bits: u64,
    result_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(waiter_tuple_bits) = tuple_from_iter_bits(_py, waiters_bits) else {
            return MoltObject::none().bits();
        };
        let Some(waiter_tuple_ptr) = obj_from_bits(waiter_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, waiter_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "event waiters must be iterable");
        };
        let waiters = seq_vec_ref(waiter_tuple_ptr);
        let mut woke = 0i64;
        for &waiter_bits in waiters {
            if let Some(token_bits) =
                asyncio_attr_lookup_allow_missing(_py, waiter_bits, b"_molt_event_token_id")
            {
                if to_i64(obj_from_bits(token_bits)).is_some() {
                    let out = crate::molt_asyncio_event_waiters_unregister(token_bits, waiter_bits);
                    if !obj_from_bits(out).is_none() {
                        dec_ref_bits(_py, out);
                    }
                    if exception_pending(_py) {
                        dec_ref_bits(_py, token_bits);
                        dec_ref_bits(_py, waiter_tuple_bits);
                        return MoltObject::none().bits();
                    }
                }
                if !obj_from_bits(token_bits).is_none() {
                    dec_ref_bits(_py, token_bits);
                }
            }
            let out_bits = asyncio_call_method1(_py, waiter_bits, b"set_result", result_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, waiter_tuple_bits);
                return out_bits;
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            woke += 1;
        }
        dec_ref_bits(_py, waiter_tuple_bits);
        MoltObject::from_int(woke).bits()
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

unsafe fn asyncio_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    let bits = *payload_ptr.add(idx);
    if bits != 0 && !obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
    }
    *payload_ptr.add(idx) = MoltObject::none().bits();
}

pub(crate) unsafe fn asyncio_clear_pending_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last();
    dec_ref_bits(_py, exc_bits);
    molt_exception_clear();
}

unsafe fn asyncio_exception_kind_is(_py: &PyToken<'_>, exc_bits: u64, expected: &str) -> bool {
    let kind_bits = molt_exception_kind(exc_bits);
    if exception_pending(_py) {
        asyncio_clear_pending_exception(_py);
        return false;
    }
    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
    if !obj_from_bits(kind_bits).is_none() {
        dec_ref_bits(_py, kind_bits);
    }
    kind.as_deref() == Some(expected)
}

unsafe fn asyncio_exception_is_fatal_base(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    let kind_bits = molt_exception_kind(exc_bits);
    if exception_pending(_py) {
        asyncio_clear_pending_exception(_py);
        return false;
    }
    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
    if !obj_from_bits(kind_bits).is_none() {
        dec_ref_bits(_py, kind_bits);
    }
    matches!(
        kind.as_deref(),
        Some("KeyboardInterrupt")
            | Some("SystemExit")
            | Some("GeneratorExit")
            | Some("BaseExceptionGroup")
    )
}

pub(crate) unsafe fn asyncio_call_method0(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> u64 {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits) else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let out = call_callable0(_py, method_bits);
    dec_ref_bits(_py, method_bits);
    out
}

unsafe fn asyncio_call_method1(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg_bits: u64,
) -> u64 {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits) else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let out = call_callable1(_py, method_bits, arg_bits);
    dec_ref_bits(_py, method_bits);
    out
}

unsafe fn asyncio_call_method2(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits) else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let out = call_callable2(_py, method_bits, arg0_bits, arg1_bits);
    dec_ref_bits(_py, method_bits);
    out
}

unsafe fn asyncio_call_method3(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return MoltObject::none().bits();
    };
    let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits) else {
        return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
    };
    let out = call_callable3(_py, method_bits, arg0_bits, arg1_bits, arg2_bits);
    dec_ref_bits(_py, method_bits);
    out
}

unsafe fn asyncio_call_with_args(_py: &PyToken<'_>, callable_bits: u64, args_bits: u64) -> u64 {
    let builder_bits = molt_callargs_new(0, 0);
    if obj_from_bits(builder_bits).is_none() {
        return builder_bits;
    }
    let _ = molt_callargs_expand_star(builder_bits, args_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, builder_bits);
        return MoltObject::none().bits();
    }
    molt_call_bind(callable_bits, builder_bits)
}

unsafe fn asyncio_fd_ready_select_once(
    _py: &PyToken<'_>,
    fileno_bits: u64,
    events_bits: u64,
) -> Option<bool> {
    let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0);
    let empty_ptr = alloc_list(_py, &[]);
    if empty_ptr.is_null() {
        return None;
    }
    let empty_bits = MoltObject::from_ptr(empty_ptr).bits();
    let read_bits = if (events & ASYNCIO_SOCKET_IO_EVENT_READ) != 0 {
        let ptr = alloc_list(_py, &[fileno_bits]);
        if ptr.is_null() {
            dec_ref_bits(_py, empty_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        inc_ref_bits(_py, empty_bits);
        empty_bits
    };
    let write_bits = if (events & ASYNCIO_SOCKET_IO_EVENT_WRITE) != 0 {
        let ptr = alloc_list(_py, &[fileno_bits]);
        if ptr.is_null() {
            dec_ref_bits(_py, read_bits);
            dec_ref_bits(_py, empty_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        inc_ref_bits(_py, empty_bits);
        empty_bits
    };
    let timeout_bits = MoltObject::from_float(0.0).bits();
    let select_out_bits =
        crate::molt_select_select(read_bits, write_bits, empty_bits, timeout_bits);
    dec_ref_bits(_py, read_bits);
    dec_ref_bits(_py, write_bits);
    dec_ref_bits(_py, empty_bits);
    if exception_pending(_py) {
        return None;
    }
    let idx0 = MoltObject::from_int(0).bits();
    let idx1 = MoltObject::from_int(1).bits();
    let ready_read_bits = molt_getitem_method(select_out_bits, idx0);
    if exception_pending(_py) {
        if !obj_from_bits(select_out_bits).is_none() {
            dec_ref_bits(_py, select_out_bits);
        }
        return None;
    }
    let ready_write_bits = molt_getitem_method(select_out_bits, idx1);
    if exception_pending(_py) {
        if !obj_from_bits(ready_read_bits).is_none() {
            dec_ref_bits(_py, ready_read_bits);
        }
        if !obj_from_bits(select_out_bits).is_none() {
            dec_ref_bits(_py, select_out_bits);
        }
        return None;
    }
    let read_len_bits = molt_len(ready_read_bits);
    let write_len_bits = molt_len(ready_write_bits);
    let read_len = to_i64(obj_from_bits(read_len_bits)).unwrap_or(0);
    let write_len = to_i64(obj_from_bits(write_len_bits)).unwrap_or(0);
    if !obj_from_bits(read_len_bits).is_none() {
        dec_ref_bits(_py, read_len_bits);
    }
    if !obj_from_bits(write_len_bits).is_none() {
        dec_ref_bits(_py, write_len_bits);
    }
    if !obj_from_bits(ready_read_bits).is_none() {
        dec_ref_bits(_py, ready_read_bits);
    }
    if !obj_from_bits(ready_write_bits).is_none() {
        dec_ref_bits(_py, ready_write_bits);
    }
    if !obj_from_bits(select_out_bits).is_none() {
        dec_ref_bits(_py, select_out_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    Some(read_len > 0 || write_len > 0)
}

unsafe fn asyncio_call_method0_allow_missing(
    _py: &PyToken<'_>,
    obj_bits: u64,
    method: &[u8],
) -> Option<u64> {
    let obj_ptr = obj_from_bits(obj_bits).as_ptr()?;
    let method_name_bits = attr_name_bits_from_bytes(_py, method)?;
    let method_bits = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits)?;
    let out = call_callable0(_py, method_bits);
    dec_ref_bits(_py, method_bits);
    Some(out)
}

unsafe fn asyncio_attr_lookup_allow_missing(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Option<u64> {
    let obj_ptr = obj_from_bits(obj_bits).as_ptr()?;
    let name_bits = attr_name_bits_from_bytes(_py, name)?;
    attr_lookup_ptr_allow_missing(_py, obj_ptr, name_bits)
}

unsafe fn asyncio_take_pending_exception_bits(_py: &PyToken<'_>) -> u64 {
    let exc_bits = molt_exception_last();
    molt_exception_clear();
    exc_bits
}

unsafe fn asyncio_close_connection_best_effort(_py: &PyToken<'_>, conn_bits: u64) {
    let close_bits = asyncio_call_method0(_py, conn_bits, b"close");
    if exception_pending(_py) {
        let exc_bits = molt_exception_last();
        molt_exception_clear();
        dec_ref_bits(_py, exc_bits);
    }
    if !obj_from_bits(close_bits).is_none() {
        dec_ref_bits(_py, close_bits);
    }
}

unsafe fn asyncio_oserror_errno_from_exception(_py: &PyToken<'_>, exc_bits: u64) -> Option<i64> {
    let exc_ptr = obj_from_bits(exc_bits).as_ptr()?;
    if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
        return None;
    }
    let kind_bits = exception_kind_bits(exc_ptr);
    let kind = string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_default();
    dec_ref_bits(_py, kind_bits);
    if kind == "BlockingIOError" {
        return Some(libc::EWOULDBLOCK as i64);
    }
    if kind == "InterruptedError" {
        return Some(libc::EINTR as i64);
    }
    let args_bits = exception_args_bits(exc_ptr);
    let args_ptr = obj_from_bits(args_bits).as_ptr()?;
    if object_type_id(args_ptr) != TYPE_ID_TUPLE {
        return None;
    }
    let args = seq_vec_ref(args_ptr);
    args.first().and_then(|bits| to_i64(obj_from_bits(*bits)))
}

fn asyncio_retryable_socket_errno(errno: i64) -> bool {
    errno == libc::EWOULDBLOCK as i64
        || errno == libc::EAGAIN as i64
        || errno == libc::EINTR as i64
        || errno == libc::EALREADY as i64
        || errno == libc::EINPROGRESS as i64
}

unsafe fn asyncio_method_truthy(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> Option<bool> {
    let bits = asyncio_call_method0(_py, obj_bits, method);
    if exception_pending(_py) {
        return None;
    }
    let truthy = is_truthy(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Some(truthy)
}

unsafe fn asyncio_waiters_pop_front(_py: &PyToken<'_>, waiters_bits: u64) -> u64 {
    if let Some(bits) = asyncio_call_method0_allow_missing(_py, waiters_bits, b"popleft") {
        return bits;
    }
    asyncio_call_method1(_py, waiters_bits, b"pop", MoltObject::from_int(0).bits())
}

unsafe fn asyncio_taskgroup_append_error(
    _py: &PyToken<'_>,
    errors_bits: u64,
    err_bits: u64,
) -> Option<()> {
    let append_bits = asyncio_call_method1(_py, errors_bits, b"append", err_bits);
    if exception_pending(_py) {
        return None;
    }
    if !obj_from_bits(append_bits).is_none() {
        dec_ref_bits(_py, append_bits);
    }
    Some(())
}

unsafe fn asyncio_taskgroup_collect_task_error(
    _py: &PyToken<'_>,
    errors_bits: u64,
    task_bits: u64,
) -> Option<bool> {
    let exc_bits = asyncio_call_method0(_py, task_bits, b"exception");
    if exception_pending(_py) {
        let pending_exc_bits = asyncio_take_pending_exception_bits(_py);
        let cancelled = asyncio_exception_kind_is(_py, pending_exc_bits, "CancelledError");
        if cancelled {
            dec_ref_bits(_py, pending_exc_bits);
            return Some(false);
        }
        asyncio_taskgroup_append_error(_py, errors_bits, pending_exc_bits)?;
        dec_ref_bits(_py, pending_exc_bits);
        return Some(true);
    }
    if obj_from_bits(exc_bits).is_none() {
        return Some(false);
    }
    let cancelled = asyncio_exception_kind_is(_py, exc_bits, "CancelledError");
    if cancelled {
        dec_ref_bits(_py, exc_bits);
        return Some(false);
    }
    asyncio_taskgroup_append_error(_py, errors_bits, exc_bits)?;
    dec_ref_bits(_py, exc_bits);
    Some(true)
}

unsafe fn asyncio_wait_scan(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    return_when: i64,
) -> Option<(Vec<bool>, bool)> {
    let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
        let _ = raise_exception::<u64>(_py, "TypeError", "wait tasks must be awaitables");
        return None;
    };
    let tasks = seq_vec_ref(tasks_ptr);
    let mut done_flags = Vec::with_capacity(tasks.len());
    let mut pending_count = 0usize;
    let mut triggered = false;
    for &task_bits in tasks {
        let done = asyncio_method_truthy(_py, task_bits, b"done")?;
        done_flags.push(done);
        if !done {
            pending_count += 1;
            continue;
        }
        if return_when == ASYNCIO_WAIT_RETURN_FIRST_COMPLETED {
            triggered = true;
            continue;
        }
        if return_when == ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION {
            let cancelled = asyncio_method_truthy(_py, task_bits, b"cancelled")?;
            if cancelled {
                triggered = true;
                continue;
            }
            let exc_bits = asyncio_call_method0(_py, task_bits, b"exception");
            if exception_pending(_py) {
                return None;
            }
            let has_exc = !obj_from_bits(exc_bits).is_none();
            dec_ref_bits(_py, exc_bits);
            if has_exc {
                triggered = true;
            }
        }
    }
    if return_when == ASYNCIO_WAIT_RETURN_ALL_COMPLETED && pending_count == 0 {
        triggered = true;
    }
    Some((done_flags, triggered))
}

unsafe fn asyncio_wait_build_result(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    done_flags: &[bool],
) -> i64 {
    let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
        return raise_exception::<i64>(_py, "TypeError", "wait tasks must be awaitables");
    };
    let tasks = seq_vec_ref(tasks_ptr);
    if tasks.len() != done_flags.len() {
        return raise_exception::<i64>(_py, "RuntimeError", "invalid wait state");
    }
    let done_bits = molt_set_new(tasks.len() as u64);
    if obj_from_bits(done_bits).is_none() {
        return MoltObject::none().bits() as i64;
    }
    let pending_bits = molt_set_new(tasks.len() as u64);
    if obj_from_bits(pending_bits).is_none() {
        dec_ref_bits(_py, done_bits);
        return MoltObject::none().bits() as i64;
    }
    for (idx, &task_bits) in tasks.iter().enumerate() {
        let target_set = if done_flags[idx] {
            done_bits
        } else {
            pending_bits
        };
        let _ = molt_set_add(target_set, task_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, done_bits);
            dec_ref_bits(_py, pending_bits);
            return MoltObject::none().bits() as i64;
        }
    }
    let out_ptr = alloc_tuple(_py, &[done_bits, pending_bits]);
    dec_ref_bits(_py, done_bits);
    dec_ref_bits(_py, pending_bits);
    if out_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    MoltObject::from_ptr(out_ptr).bits() as i64
}

unsafe fn asyncio_cancel_task(_py: &PyToken<'_>, task_bits: u64) {
    let out_bits = asyncio_call_method0(_py, task_bits, b"cancel");
    if exception_pending(_py) {
        asyncio_clear_pending_exception(_py);
        return;
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
}

unsafe fn asyncio_gather_store_result(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    idx: usize,
    value_bits: u64,
) {
    let slot = payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx);
    let old_bits = *slot;
    if old_bits != 0 && !obj_from_bits(old_bits).is_none() {
        dec_ref_bits(_py, old_bits);
    }
    *slot = value_bits;
    inc_ref_bits(_py, value_bits);
}

unsafe fn asyncio_gather_cancel_pending(
    _py: &PyToken<'_>,
    tasks_bits: u64,
    payload_ptr: *mut u64,
    results_len: usize,
    missing: u64,
) {
    let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
        return;
    };
    let tasks = seq_vec_ref(tasks_ptr);
    let limit = results_len.min(tasks.len());
    for (idx, &task_bits) in tasks.iter().take(limit).enumerate() {
        if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) == missing {
            asyncio_cancel_task(_py, task_bits);
        }
    }
}

unsafe fn asyncio_gather_build_list(_py: &PyToken<'_>, payload_ptr: *mut u64, len: usize) -> u64 {
    let mut elems = Vec::with_capacity(len);
    for idx in 0..len {
        elems.push(*payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx));
    }
    let out_ptr = alloc_list(_py, elems.as_slice());
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

unsafe fn asyncio_pending_with_wait(
    _py: &PyToken<'_>,
    payload_ptr: *mut u64,
    slot_idx: usize,
    fd_bits: u64,
    io_events: i64,
) -> i64 {
    let mut waiter_bits = *payload_ptr.add(slot_idx);
    if obj_from_bits(waiter_bits).is_none() {
        let fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(-1);
        waiter_bits = if fd < 0 {
            molt_async_sleep_new(
                MoltObject::from_float(0.0).bits(),
                MoltObject::none().bits(),
            )
        } else {
            molt_io_wait_new(
                MoltObject::from_int(fd).bits(),
                MoltObject::from_int(io_events).bits(),
                MoltObject::none().bits(),
            )
        };
        if obj_from_bits(waiter_bits).is_none() {
            return waiter_bits as i64;
        }
        *payload_ptr.add(slot_idx) = waiter_bits;
    }
    let wait_res = molt_future_poll(waiter_bits);
    if wait_res == pending_bits_i64() {
        return pending_bits_i64();
    }
    if exception_pending(_py) {
        return wait_res;
    }
    asyncio_drop_slot_ref(_py, payload_ptr, slot_idx);
    pending_bits_i64()
}

fn asyncio_msg_dontwait() -> i64 {
    #[cfg(unix)]
    {
        libc::MSG_DONTWAIT as i64
    }
    #[cfg(not(unix))]
    {
        0
    }
}

unsafe fn asyncio_drop_payload_slots(_py: &PyToken<'_>, payload_ptr: *mut u64, slots: usize) {
    for idx in 0..slots {
        asyncio_drop_slot_ref(_py, payload_ptr, idx);
    }
}

/// # Safety
/// - `reader_bits` must be a valid stream-reader handle.
#[no_mangle]
pub extern "C" fn molt_asyncio_stream_reader_read_new(reader_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_reader_read_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_N) = n_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, n_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-reader read wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_stream_reader_read_poll(obj_bits: u64) -> i64 {
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
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid stream_reader_read payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let reader_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_READER);
        let n_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READ_SLOT_N);
        let out_bits = molt_stream_reader_read(reader_bits, n_bits);
        if out_bits as i64 != pending_bits_i64() {
            asyncio_drop_payload_slots(_py, payload_ptr, 3);
            return out_bits as i64;
        }
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_STREAM_READER_READ_SLOT_WAIT,
            MoltObject::none().bits(),
            ASYNCIO_SOCKET_IO_EVENT_READ,
        )
    })
}

/// # Safety
/// - `reader_bits` must be a valid stream-reader handle.
#[no_mangle]
pub extern "C" fn molt_asyncio_stream_reader_readline_new(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_reader_readline_poll_fn_addr(),
            (2 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-reader readline wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_stream_reader_readline_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 2 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid stream_reader_readline payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let reader_bits = *payload_ptr.add(ASYNCIO_STREAM_READER_READLINE_SLOT_READER);
        let out_bits = molt_stream_reader_readline(reader_bits);
        if out_bits as i64 != pending_bits_i64() {
            asyncio_drop_payload_slots(_py, payload_ptr, 2);
            return out_bits as i64;
        }
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT,
            MoltObject::none().bits(),
            ASYNCIO_SOCKET_IO_EVENT_READ,
        )
    })
}

/// # Safety
/// - `stream_bits` must be a valid stream handle.
#[no_mangle]
pub extern "C" fn molt_asyncio_stream_send_all_new(stream_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_stream_send_all_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM) = stream_bits;
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, stream_bits);
        inc_ref_bits(_py, data_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a stream-send wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_stream_send_all_poll(obj_bits: u64) -> i64 {
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
            return raise_exception::<i64>(_py, "RuntimeError", "invalid stream_send_all payload");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let stream_bits = *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM);
        let data_bits = *payload_ptr.add(ASYNCIO_STREAM_SEND_ALL_SLOT_DATA);
        let out_bits = molt_stream_send_obj(stream_bits, data_bits);
        if out_bits as i64 == pending_bits_i64() {
            return asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT,
                MoltObject::none().bits(),
                ASYNCIO_SOCKET_IO_EVENT_READ,
            );
        }
        if exception_pending(_py) {
            asyncio_drop_payload_slots(_py, payload_ptr, 3);
            return out_bits as i64;
        }
        let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
        if sent == 0 {
            asyncio_drop_payload_slots(_py, payload_ptr, 3);
            return MoltObject::none().bits() as i64;
        }
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT,
            MoltObject::none().bits(),
            ASYNCIO_SOCKET_IO_EVENT_READ,
        )
    })
}

/// # Safety
/// - `buffer_bits` must be bytes-like.
#[no_mangle]
pub extern "C" fn molt_asyncio_stream_buffer_snapshot(buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let out_bits = molt_bytes_from_obj(buffer_bits);
        if exception_pending(_py) {
            return out_bits;
        }
        out_bits
    })
}

/// # Safety
/// - `buffer_bits` must be a mutable bytearray-like object.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_stream_buffer_consume(
    buffer_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(mut count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "stream consume count must be int");
        };
        if count <= 0 {
            return MoltObject::from_int(0).bits();
        }
        let len_bits = molt_len(buffer_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(buf_len) = to_i64(obj_from_bits(len_bits)) else {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return raise_exception::<u64>(_py, "TypeError", "stream buffer must be sized");
        };
        if !obj_from_bits(len_bits).is_none() {
            dec_ref_bits(_py, len_bits);
        }
        if buf_len <= 0 {
            return MoltObject::from_int(0).bits();
        }
        count = count.min(buf_len);

        if count == buf_len {
            let clear_bits = asyncio_call_method0(_py, buffer_bits, b"clear");
            if exception_pending(_py) {
                return clear_bits;
            }
            if !obj_from_bits(clear_bits).is_none() {
                dec_ref_bits(_py, clear_bits);
            }
            return MoltObject::from_int(count).bits();
        }

        let slice_bits = molt_slice_new(
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(count).bits(),
            MoltObject::none().bits(),
        );
        if obj_from_bits(slice_bits).is_none() {
            return slice_bits;
        }
        let del_bits = asyncio_call_method1(_py, buffer_bits, b"__delitem__", slice_bits);
        if !obj_from_bits(slice_bits).is_none() {
            dec_ref_bits(_py, slice_bits);
        }
        if exception_pending(_py) {
            return del_bits;
        }
        if !obj_from_bits(del_bits).is_none() {
            dec_ref_bits(_py, del_bits);
        }
        MoltObject::from_int(count).bits()
    })
}

/// # Safety
/// - `reader_bits` must be a valid socket-reader handle.
#[no_mangle]
pub extern "C" fn molt_asyncio_socket_reader_read_new(
    reader_bits: u64,
    n_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_socket_reader_read_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_N) = n_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, n_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket-reader read wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_socket_reader_read_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid socket_reader_read payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let reader_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_READER);
        let n_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_N);
        let out_bits = molt_socket_reader_read(reader_bits, n_bits);
        if out_bits as i64 != pending_bits_i64() {
            asyncio_drop_payload_slots(_py, payload_ptr, 4);
            return out_bits as i64;
        }
        let fd_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READ_SLOT_FD);
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_SOCKET_READER_READ_SLOT_WAIT,
            fd_bits,
            ASYNCIO_SOCKET_IO_EVENT_READ,
        )
    })
}

/// # Safety
/// - `reader_bits` must be a valid socket-reader handle.
#[no_mangle]
pub extern "C" fn molt_asyncio_socket_reader_readline_new(reader_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_socket_reader_readline_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_READER) = reader_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, reader_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket-reader readline wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_socket_reader_readline_poll(obj_bits: u64) -> i64 {
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
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid socket_reader_readline payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let reader_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_READER);
        let out_bits = molt_socket_reader_readline(reader_bits);
        if out_bits as i64 != pending_bits_i64() {
            asyncio_drop_payload_slots(_py, payload_ptr, 3);
            return out_bits as i64;
        }
        let fd_bits = *payload_ptr.add(ASYNCIO_SOCKET_READER_READLINE_SLOT_FD);
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT,
            fd_bits,
            ASYNCIO_SOCKET_IO_EVENT_READ,
        )
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_recv_new(sock_bits: u64, size_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recv_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SIZE) = size_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, size_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recv wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_recv_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_recv payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SOCK);
        let size_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_SIZE);
        let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
        let out_bits = asyncio_call_method2(_py, sock_bits, b"recv", size_bits, flags_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_RECV_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 4);
        out_bits as i64
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_recv_into_new(
    sock_bits: u64,
    buf_bits: u64,
    nbytes_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recv_into_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_BUF) = buf_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES) = nbytes_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, buf_bits);
        inc_ref_bits(_py, nbytes_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recv_into wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_recv_into_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 5 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_recv_into payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK);
        let buf_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_BUF);
        let nbytes_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES);
        let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
        let out_bits = asyncio_call_method3(
            _py,
            sock_bits,
            b"recv_into",
            buf_bits,
            nbytes_bits,
            flags_bits,
        );
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECV_INTO_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 5);
        out_bits as i64
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_sendall_new(
    sock_bits: u64,
    data_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data_len_bits = molt_len(data_bits);
        if exception_pending(_py) {
            return data_len_bits;
        }
        let data_len = to_i64(obj_from_bits(data_len_bits)).unwrap_or(-1);
        if data_len < 0 {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return raise_exception::<u64>(_py, "TypeError", "invalid sendall payload");
        }
        let obj_bits = molt_future_new(
            asyncio_sock_sendall_poll_fn_addr(),
            (6 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            if !obj_from_bits(data_len_bits).is_none() {
                dec_ref_bits(_py, data_len_bits);
            }
            return MoltObject::none().bits();
        };
        let total_bits = MoltObject::from_int(0).bits();
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL) = total_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DLEN) = data_len_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, data_bits);
        inc_ref_bits(_py, total_bits);
        inc_ref_bits(_py, data_len_bits);
        inc_ref_bits(_py, fd_bits);
        dec_ref_bits(_py, data_len_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket sendall wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_sendall_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 6 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_sendall payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_SOCK);
        let data_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DATA);
        let data_len = to_i64(obj_from_bits(
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_DLEN),
        ))
        .unwrap_or(0);

        for _ in 0..8 {
            let total_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL);
            let total = to_i64(obj_from_bits(total_bits)).unwrap_or(0);
            if total >= data_len {
                asyncio_drop_payload_slots(_py, payload_ptr, 6);
                return MoltObject::none().bits() as i64;
            }

            let slice_bits = molt_slice_new(
                total_bits,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            if obj_from_bits(slice_bits).is_none() {
                return slice_bits as i64;
            }
            let tail_bits = molt_getitem_method(data_bits, slice_bits);
            dec_ref_bits(_py, slice_bits);
            if exception_pending(_py) {
                if !obj_from_bits(tail_bits).is_none() {
                    dec_ref_bits(_py, tail_bits);
                }
                return tail_bits as i64;
            }

            let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
            let out_bits = asyncio_call_method2(_py, sock_bits, b"send", tail_bits, flags_bits);
            dec_ref_bits(_py, tail_bits);
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
                if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                    dec_ref_bits(_py, exc_bits);
                    let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
                    return asyncio_pending_with_wait(
                        _py,
                        payload_ptr,
                        ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
                        fd_bits,
                        ASYNCIO_SOCKET_IO_EVENT_WRITE,
                    );
                }
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }

            let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
            dec_ref_bits(_py, out_bits);
            if sent <= 0 {
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_WRITE,
                );
            }

            let old_total_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL);
            let new_total = total.saturating_add(sent);
            let new_total_bits = MoltObject::from_int(new_total).bits();
            if old_total_bits != 0 && !obj_from_bits(old_total_bits).is_none() {
                dec_ref_bits(_py, old_total_bits);
            }
            *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_TOTAL) = new_total_bits;
            inc_ref_bits(_py, new_total_bits);
            if new_total >= data_len {
                asyncio_drop_payload_slots(_py, payload_ptr, 6);
                return MoltObject::none().bits() as i64;
            }
        }

        let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDALL_SLOT_FD);
        asyncio_pending_with_wait(
            _py,
            payload_ptr,
            ASYNCIO_SOCK_SENDALL_SLOT_WAIT,
            fd_bits,
            ASYNCIO_SOCKET_IO_EVENT_WRITE,
        )
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_recvfrom_new(
    sock_bits: u64,
    size_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recvfrom_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SIZE) = size_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, size_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recvfrom wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_recvfrom_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_recvfrom payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SOCK);
        let size_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_SIZE);
        let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
        let out_bits = asyncio_call_method2(_py, sock_bits, b"recvfrom", size_bits, flags_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_RECVFROM_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 4);
        out_bits as i64
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_recvfrom_into_new(
    sock_bits: u64,
    buf_bits: u64,
    nbytes_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_recvfrom_into_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF) = buf_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES) = nbytes_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, buf_bits);
        inc_ref_bits(_py, nbytes_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket recvfrom_into wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_recvfrom_into_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 5 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_recvfrom_into payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK);
        let buf_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF);
        let nbytes_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES);
        let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
        let out_bits = asyncio_call_method3(
            _py,
            sock_bits,
            b"recvfrom_into",
            buf_bits,
            nbytes_bits,
            flags_bits,
        );
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 5);
        out_bits as i64
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_sendto_new(
    sock_bits: u64,
    data_bits: u64,
    addr_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_sendto_poll_fn_addr(),
            (5 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_DATA) = data_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_ADDR) = addr_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, data_bits);
        inc_ref_bits(_py, addr_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket sendto wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_sendto_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 5 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_sendto payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_SOCK);
        let data_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_DATA);
        let addr_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_ADDR);
        let flags_bits = MoltObject::from_int(asyncio_msg_dontwait()).bits();
        let out_bits =
            asyncio_call_method3(_py, sock_bits, b"sendto", data_bits, flags_bits, addr_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_SENDTO_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_WRITE,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        let sent = to_i64(obj_from_bits(out_bits)).unwrap_or(-1);
        if sent <= 0 {
            dec_ref_bits(_py, out_bits);
            let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_SENDTO_SLOT_FD);
            return asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_SOCK_SENDTO_SLOT_WAIT,
                fd_bits,
                ASYNCIO_SOCKET_IO_EVENT_WRITE,
            );
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 5);
        out_bits as i64
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_connect_new(
    sock_bits: u64,
    addr_bits: u64,
    fd_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_connect_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_ADDR) = addr_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, addr_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket connect wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_connect_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_connect payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_SOCK);
        let addr_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_ADDR);
        let rc_bits = asyncio_call_method1(_py, sock_bits, b"connect_ex", addr_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_CONNECT_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_WRITE,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        let rc = to_i64(obj_from_bits(rc_bits)).unwrap_or(libc::EINVAL as i64);
        dec_ref_bits(_py, rc_bits);
        if rc == 0 {
            asyncio_drop_payload_slots(_py, payload_ptr, 4);
            return MoltObject::none().bits() as i64;
        }
        if asyncio_retryable_socket_errno(rc) {
            let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_CONNECT_SLOT_FD);
            return asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_SOCK_CONNECT_SLOT_WAIT,
                fd_bits,
                ASYNCIO_SOCKET_IO_EVENT_WRITE,
            );
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 4);
        raise_os_error_errno::<i64>(_py, rc, "connect")
    })
}

/// # Safety
/// - `sock_bits` must be a valid socket object.
#[no_mangle]
pub extern "C" fn molt_asyncio_sock_accept_new(sock_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_sock_accept_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, fd_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a socket accept wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_sock_accept_poll(obj_bits: u64) -> i64 {
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
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio sock_accept payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let sock_bits = *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_SOCK);
        let out_bits = asyncio_call_method0(_py, sock_bits, b"accept");
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                dec_ref_bits(_py, exc_bits);
                let fd_bits = *payload_ptr.add(ASYNCIO_SOCK_ACCEPT_SLOT_FD);
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_SOCK_ACCEPT_SLOT_WAIT,
                    fd_bits,
                    ASYNCIO_SOCKET_IO_EVENT_READ,
                );
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 3);
        out_bits as i64
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub extern "C" fn molt_asyncio_timer_handle_new(
    handle_bits: u64,
    delay_bits: u64,
    loop_bits: u64,
    scheduled_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_timer_handle_poll_fn_addr(),
            (7 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_HANDLE) = handle_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_DELAY) = delay_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_SCHEDULED) = scheduled_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_READY_LOCK) = ready_lock_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_READY) = ready_bits;
            *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, handle_bits);
        inc_ref_bits(_py, delay_bits);
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, scheduled_bits);
        inc_ref_bits(_py, ready_lock_bits);
        inc_ref_bits(_py, ready_bits);
        obj_bits
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_timer_schedule(
    handle_bits: u64,
    delay_bits: u64,
    loop_bits: u64,
    scheduled_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let delay_obj = obj_from_bits(molt_float_from_obj(delay_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let delay = delay_obj.as_float().unwrap_or(0.0);
        if !delay.is_finite() || delay <= 0.0 {
            if asyncio_loop_enqueue_handle_inner(
                _py,
                loop_bits,
                ready_lock_bits,
                ready_bits,
                handle_bits,
            )
            .is_none()
            {
                return MoltObject::none().bits();
            }
            return MoltObject::none().bits();
        }

        let add_bits = asyncio_call_method1(_py, scheduled_bits, b"add", handle_bits);
        if exception_pending(_py) {
            return add_bits;
        }
        if !obj_from_bits(add_bits).is_none() {
            dec_ref_bits(_py, add_bits);
        }

        let timer_bits = molt_asyncio_timer_handle_new(
            handle_bits,
            delay_bits,
            loop_bits,
            scheduled_bits,
            ready_lock_bits,
            ready_bits,
        );
        if obj_from_bits(timer_bits).is_none() {
            return timer_bits;
        }
        let task_bits = asyncio_call_method1(_py, loop_bits, b"create_task", timer_bits);
        dec_ref_bits(_py, timer_bits);
        if exception_pending(_py) {
            return task_bits;
        }
        task_bits
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_timer_handle_cancel(
    scheduled_bits: u64,
    handle_bits: u64,
    timer_task_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !obj_from_bits(timer_task_bits).is_none() {
            let cancel_bits = asyncio_call_method0(_py, timer_task_bits, b"cancel");
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            } else if !obj_from_bits(cancel_bits).is_none() {
                dec_ref_bits(_py, cancel_bits);
            }
        }
        let discard_bits = asyncio_call_method1(_py, scheduled_bits, b"discard", handle_bits);
        if exception_pending(_py) {
            asyncio_clear_pending_exception(_py);
        } else if !obj_from_bits(discard_bits).is_none() {
            dec_ref_bits(_py, discard_bits);
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// - `obj_bits` must be a valid timer-handle wrapper future pointer.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_timer_handle_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 7 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid asyncio timer payload");
        }
        let payload_ptr = obj_ptr as *mut u64;
        if (*header).state == 0 {
            let delay_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_DELAY);
            let delay_obj = obj_from_bits(molt_float_from_obj(delay_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
            let delay = delay_obj.as_float().unwrap_or(0.0);
            if delay.is_finite() && delay > 0.0 {
                let waiter_bits = molt_async_sleep_new(
                    MoltObject::from_float(delay).bits(),
                    MoltObject::none().bits(),
                );
                if obj_from_bits(waiter_bits).is_none() {
                    return waiter_bits as i64;
                }
                *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT) = waiter_bits;
            }
            (*header).state = 1;
        }

        let wait_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_WAIT);
        if !obj_from_bits(wait_bits).is_none() {
            let wait_res = molt_future_poll(wait_bits);
            if wait_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            if exception_pending(_py) {
                return wait_res;
            }
            asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_TIMER_SLOT_WAIT);
        }

        let handle_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_HANDLE);
        let scheduled_bits = *payload_ptr.add(ASYNCIO_TIMER_SLOT_SCHEDULED);
        let discard_bits = asyncio_call_method1(_py, scheduled_bits, b"discard", handle_bits);
        if exception_pending(_py) {
            return discard_bits as i64;
        }
        if !obj_from_bits(discard_bits).is_none() {
            dec_ref_bits(_py, discard_bits);
        }
        let cancelled = match asyncio_method_truthy(_py, handle_bits, b"cancelled") {
            Some(flag) => flag,
            None => return MoltObject::none().bits() as i64,
        };
        if cancelled {
            asyncio_drop_payload_slots(_py, payload_ptr, 7);
            return MoltObject::none().bits() as i64;
        }
        let run_bits = asyncio_call_method0(_py, handle_bits, b"_run");
        if exception_pending(_py) {
            return run_bits as i64;
        }
        if !obj_from_bits(run_bits).is_none() {
            dec_ref_bits(_py, run_bits);
        }
        asyncio_drop_payload_slots(_py, payload_ptr, 7);
        MoltObject::none().bits() as i64
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub extern "C" fn molt_asyncio_fd_watcher_new(
    registry_bits: u64,
    fileno_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_fd_watcher_poll_fn_addr(),
            (6 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_REGISTRY) = registry_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_FILENO) = fileno_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_CALLBACK) = callback_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_ARGS) = args_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS) = events_bits;
            *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, registry_bits);
        inc_ref_bits(_py, fileno_bits);
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, events_bits);
        obj_bits
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_register(
    loop_bits: u64,
    registry_bits: u64,
    fileno_bits: u64,
    callback_bits: u64,
    args_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let old_entry_bits = asyncio_call_method2(
            _py,
            registry_bits,
            b"pop",
            fileno_bits,
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return old_entry_bits;
        }
        if !obj_from_bits(old_entry_bits).is_none() {
            let task_bits = molt_getitem_method(old_entry_bits, MoltObject::from_int(2).bits());
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            } else {
                let cancel_bits = asyncio_call_method0(_py, task_bits, b"cancel");
                if exception_pending(_py) {
                    asyncio_clear_pending_exception(_py);
                } else if !obj_from_bits(cancel_bits).is_none() {
                    dec_ref_bits(_py, cancel_bits);
                }
                if !obj_from_bits(task_bits).is_none() {
                    dec_ref_bits(_py, task_bits);
                }
            }
            dec_ref_bits(_py, old_entry_bits);
        }

        let pending_entry_ptr =
            alloc_tuple(_py, &[callback_bits, args_bits, MoltObject::none().bits()]);
        if pending_entry_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let pending_entry_bits = MoltObject::from_ptr(pending_entry_ptr).bits();
        let pending_set_bits = asyncio_call_method2(
            _py,
            registry_bits,
            b"__setitem__",
            fileno_bits,
            pending_entry_bits,
        );
        if !obj_from_bits(pending_entry_bits).is_none() {
            dec_ref_bits(_py, pending_entry_bits);
        }
        if exception_pending(_py) {
            return pending_set_bits;
        }
        if !obj_from_bits(pending_set_bits).is_none() {
            dec_ref_bits(_py, pending_set_bits);
        }

        let watcher_bits = molt_asyncio_fd_watcher_new(
            registry_bits,
            fileno_bits,
            callback_bits,
            args_bits,
            events_bits,
        );
        if obj_from_bits(watcher_bits).is_none() {
            let cleanup_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"pop",
                fileno_bits,
                MoltObject::none().bits(),
            );
            if !obj_from_bits(cleanup_bits).is_none() {
                dec_ref_bits(_py, cleanup_bits);
            }
            return watcher_bits;
        }
        let task_bits = asyncio_call_method1(_py, loop_bits, b"create_task", watcher_bits);
        dec_ref_bits(_py, watcher_bits);
        if exception_pending(_py) {
            let cleanup_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"pop",
                fileno_bits,
                MoltObject::none().bits(),
            );
            if !obj_from_bits(cleanup_bits).is_none() {
                dec_ref_bits(_py, cleanup_bits);
            }
            return task_bits;
        }
        let entry_ptr = alloc_tuple(_py, &[callback_bits, args_bits, task_bits]);
        if entry_ptr.is_null() {
            let cleanup_bits = asyncio_call_method2(
                _py,
                registry_bits,
                b"pop",
                fileno_bits,
                MoltObject::none().bits(),
            );
            if !obj_from_bits(cleanup_bits).is_none() {
                dec_ref_bits(_py, cleanup_bits);
            }
            if !obj_from_bits(task_bits).is_none() {
                dec_ref_bits(_py, task_bits);
            }
            return MoltObject::none().bits();
        }
        let entry_bits = MoltObject::from_ptr(entry_ptr).bits();
        let setitem_bits =
            asyncio_call_method2(_py, registry_bits, b"__setitem__", fileno_bits, entry_bits);
        if !obj_from_bits(entry_bits).is_none() {
            dec_ref_bits(_py, entry_bits);
        }
        if !obj_from_bits(task_bits).is_none() {
            dec_ref_bits(_py, task_bits);
        }
        if exception_pending(_py) {
            return setitem_bits;
        }
        if !obj_from_bits(setitem_bits).is_none() {
            dec_ref_bits(_py, setitem_bits);
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_unregister(
    registry_bits: u64,
    fileno_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let entry_bits = asyncio_call_method2(
            _py,
            registry_bits,
            b"pop",
            fileno_bits,
            MoltObject::none().bits(),
        );
        if exception_pending(_py) {
            return entry_bits;
        }
        if obj_from_bits(entry_bits).is_none() {
            return MoltObject::from_bool(false).bits();
        }

        let task_bits = molt_getitem_method(entry_bits, MoltObject::from_int(2).bits());
        if exception_pending(_py) {
            dec_ref_bits(_py, entry_bits);
            return task_bits;
        }
        if !obj_from_bits(task_bits).is_none() {
            let cancel_bits = asyncio_call_method0(_py, task_bits, b"cancel");
            if exception_pending(_py) {
                asyncio_clear_pending_exception(_py);
            } else if !obj_from_bits(cancel_bits).is_none() {
                dec_ref_bits(_py, cancel_bits);
            }
            dec_ref_bits(_py, task_bits);
        }
        dec_ref_bits(_py, entry_bits);
        MoltObject::from_bool(true).bits()
    })
}

/// # Safety
/// - `obj_bits` must be a valid fd watcher wrapper future pointer.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_fd_watcher_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 6 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio fd watcher payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let registry_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_REGISTRY);
        let fileno_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_FILENO);

        let contains_bits = asyncio_call_method1(_py, registry_bits, b"__contains__", fileno_bits);
        if exception_pending(_py) {
            return contains_bits as i64;
        }
        let still_registered = is_truthy(_py, obj_from_bits(contains_bits));
        if !obj_from_bits(contains_bits).is_none() {
            dec_ref_bits(_py, contains_bits);
        }
        if !still_registered {
            asyncio_drop_payload_slots(_py, payload_ptr, 6);
            return MoltObject::none().bits() as i64;
        }

        let mut waiter_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT);
        if obj_from_bits(waiter_bits).is_none() {
            let events_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS);
            waiter_bits = molt_io_wait_new(fileno_bits, events_bits, MoltObject::none().bits());
            if obj_from_bits(waiter_bits).is_none() {
                if exception_pending(_py) {
                    let exc_bits = asyncio_take_pending_exception_bits(_py);
                    let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                    if fatal {
                        let raised = molt_raise(exc_bits);
                        dec_ref_bits(_py, exc_bits);
                        return raised as i64;
                    }
                    dec_ref_bits(_py, exc_bits);
                }
                let Some(ready_now) = asyncio_fd_ready_select_once(_py, fileno_bits, events_bits)
                else {
                    return MoltObject::none().bits() as i64;
                };
                if !ready_now {
                    waiter_bits = molt_async_sleep_new(
                        MoltObject::from_float(0.001).bits(),
                        MoltObject::none().bits(),
                    );
                    if obj_from_bits(waiter_bits).is_none() {
                        return waiter_bits as i64;
                    }
                    *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = waiter_bits;
                }
            } else {
                *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_WAIT) = waiter_bits;
            }
        }

        if !obj_from_bits(waiter_bits).is_none() {
            let wait_res = molt_future_poll(waiter_bits);
            if wait_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            if exception_pending(_py) {
                let exc_bits = asyncio_take_pending_exception_bits(_py);
                let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
                if fatal {
                    let raised = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return raised as i64;
                }
                dec_ref_bits(_py, exc_bits);
                asyncio_drop_payload_slots(_py, payload_ptr, 6);
                return MoltObject::none().bits() as i64;
            }
            asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_FD_WATCHER_SLOT_WAIT);
        }

        let contains_bits = asyncio_call_method1(_py, registry_bits, b"__contains__", fileno_bits);
        if exception_pending(_py) {
            return contains_bits as i64;
        }
        let still_registered = is_truthy(_py, obj_from_bits(contains_bits));
        if !obj_from_bits(contains_bits).is_none() {
            dec_ref_bits(_py, contains_bits);
        }
        if !still_registered {
            asyncio_drop_payload_slots(_py, payload_ptr, 6);
            return MoltObject::none().bits() as i64;
        }

        let callback_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_CALLBACK);
        let args_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_ARGS);
        let callback_res = asyncio_call_with_args(_py, callback_bits, args_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            let errno = asyncio_oserror_errno_from_exception(_py, exc_bits).unwrap_or(i64::MIN);
            dec_ref_bits(_py, exc_bits);
            let events_bits = *payload_ptr.add(ASYNCIO_FD_WATCHER_SLOT_EVENTS);
            let default_events = ASYNCIO_SOCKET_IO_EVENT_READ | ASYNCIO_SOCKET_IO_EVENT_WRITE;
            let events = to_i64(obj_from_bits(events_bits)).unwrap_or(default_events);
            if errno != i64::MIN && asyncio_retryable_socket_errno(errno) {
                return asyncio_pending_with_wait(
                    _py,
                    payload_ptr,
                    ASYNCIO_FD_WATCHER_SLOT_WAIT,
                    fileno_bits,
                    events,
                );
            }
            // CPython event loops do not terminate reader/writer watchers on ordinary callback
            // exceptions; they route errors via loop exception handling and keep dispatch alive.
            // Re-arm the watcher to avoid silently dropping callbacks on non-fatal errors.
            return asyncio_pending_with_wait(
                _py,
                payload_ptr,
                ASYNCIO_FD_WATCHER_SLOT_WAIT,
                fileno_bits,
                events,
            );
        }
        if !obj_from_bits(callback_res).is_none() {
            dec_ref_bits(_py, callback_res);
        }
        pending_bits_i64()
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub extern "C" fn molt_asyncio_server_accept_loop_new(
    sock_bits: u64,
    callback_bits: u64,
    loop_bits: u64,
    reader_ctor_bits: u64,
    writer_ctor_bits: u64,
    closed_probe_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let fd_bits = unsafe { asyncio_call_method0(_py, sock_bits, b"fileno") };
        if exception_pending(_py) {
            return fd_bits;
        }
        let obj_bits = molt_future_new(
            asyncio_server_accept_loop_poll_fn_addr(),
            (8 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            if !obj_from_bits(fd_bits).is_none() {
                dec_ref_bits(_py, fd_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            if !obj_from_bits(fd_bits).is_none() {
                dec_ref_bits(_py, fd_bits);
            }
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_SOCK) = sock_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK) = callback_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR) = reader_ctor_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR) = writer_ctor_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE) = closed_probe_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_FD) = fd_bits;
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, sock_bits);
        inc_ref_bits(_py, callback_bits);
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, reader_ctor_bits);
        inc_ref_bits(_py, writer_ctor_bits);
        inc_ref_bits(_py, closed_probe_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid server-accept-loop wrapper future pointer.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_server_accept_loop_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 8 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio server accept loop payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let closed_probe_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CLOSED_PROBE);
        let closed_bits = call_callable0(_py, closed_probe_bits);
        if exception_pending(_py) {
            return closed_bits as i64;
        }
        let is_closed = is_truthy(_py, obj_from_bits(closed_bits));
        if !obj_from_bits(closed_bits).is_none() {
            dec_ref_bits(_py, closed_bits);
        }
        if is_closed {
            asyncio_drop_payload_slots(_py, payload_ptr, 8);
            return MoltObject::none().bits() as i64;
        }

        let mut waiter_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
        if obj_from_bits(waiter_bits).is_none() {
            let sock_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_SOCK);
            let fd_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_FD);
            waiter_bits = molt_asyncio_sock_accept_new(sock_bits, fd_bits);
            if obj_from_bits(waiter_bits).is_none() {
                return waiter_bits as i64;
            }
            *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WAIT) = waiter_bits;
        }

        let wait_res = molt_future_poll(waiter_bits);
        if wait_res == pending_bits_i64() {
            return pending_bits_i64();
        }
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            if asyncio_exception_kind_is(_py, exc_bits, "CancelledError") {
                dec_ref_bits(_py, exc_bits);
                asyncio_drop_payload_slots(_py, payload_ptr, 8);
                return MoltObject::none().bits() as i64;
            }
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
            return pending_bits_i64();
        }

        asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_SERVER_ACCEPT_SLOT_WAIT);
        let accepted_bits = wait_res as u64;
        let conn_bits = molt_getitem_method(accepted_bits, MoltObject::from_int(0).bits());
        if !obj_from_bits(accepted_bits).is_none() {
            dec_ref_bits(_py, accepted_bits);
        }
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            return pending_bits_i64();
        }

        let setblocking_bits = asyncio_call_method1(
            _py,
            conn_bits,
            b"setblocking",
            MoltObject::from_bool(false).bits(),
        );
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            if asyncio_exception_is_fatal_base(_py, exc_bits) {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                dec_ref_bits(_py, conn_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
        } else if !obj_from_bits(setblocking_bits).is_none() {
            dec_ref_bits(_py, setblocking_bits);
        }

        let reader_ctor_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_READER_CTOR);
        let reader_bits = call_callable1(_py, reader_ctor_bits, conn_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            asyncio_close_connection_best_effort(_py, conn_bits);
            dec_ref_bits(_py, conn_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            return pending_bits_i64();
        }

        let writer_ctor_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_WRITER_CTOR);
        let writer_bits = call_callable1(_py, writer_ctor_bits, conn_bits);
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            if !obj_from_bits(reader_bits).is_none() {
                dec_ref_bits(_py, reader_bits);
            }
            asyncio_close_connection_best_effort(_py, conn_bits);
            dec_ref_bits(_py, conn_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            return pending_bits_i64();
        }

        let callback_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_CALLBACK);
        let callback_res = call_callable2(_py, callback_bits, reader_bits, writer_bits);
        if !obj_from_bits(reader_bits).is_none() {
            dec_ref_bits(_py, reader_bits);
        }
        if !obj_from_bits(writer_bits).is_none() {
            dec_ref_bits(_py, writer_bits);
        }
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            asyncio_close_connection_best_effort(_py, conn_bits);
            dec_ref_bits(_py, conn_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            return pending_bits_i64();
        }

        let loop_bits = *payload_ptr.add(ASYNCIO_SERVER_ACCEPT_SLOT_LOOP);
        let spawn_bits = asyncio_call_method1(_py, loop_bits, b"create_task", callback_res);
        if !obj_from_bits(callback_res).is_none() {
            dec_ref_bits(_py, callback_res);
        }
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            let fatal = asyncio_exception_is_fatal_base(_py, exc_bits);
            asyncio_close_connection_best_effort(_py, conn_bits);
            dec_ref_bits(_py, conn_bits);
            if fatal {
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            dec_ref_bits(_py, exc_bits);
            return pending_bits_i64();
        }
        if !obj_from_bits(spawn_bits).is_none() {
            dec_ref_bits(_py, spawn_bits);
        }
        dec_ref_bits(_py, conn_bits);
        pending_bits_i64()
    })
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub extern "C" fn molt_asyncio_ready_runner_new(
    loop_bits: u64,
    ready_lock_bits: u64,
    ready_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_future_new(
            asyncio_ready_runner_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        let payload_ptr = obj_ptr as *mut u64;
        unsafe {
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_LOOP) = loop_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY_LOCK) = ready_lock_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY) = ready_bits;
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT) = MoltObject::none().bits();
        }
        inc_ref_bits(_py, loop_bits);
        inc_ref_bits(_py, ready_lock_bits);
        inc_ref_bits(_py, ready_bits);
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid ready-runner wrapper future pointer.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_ready_runner_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(
                _py,
                "RuntimeError",
                "invalid asyncio ready runner payload",
            );
        }
        let payload_ptr = obj_ptr as *mut u64;
        let loop_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_LOOP);
        let closed = match asyncio_method_truthy(_py, loop_bits, b"is_closed") {
            Some(flag) => flag,
            None => return MoltObject::none().bits() as i64,
        };
        if closed {
            asyncio_drop_payload_slots(_py, payload_ptr, 4);
            return MoltObject::none().bits() as i64;
        }

        let ready_lock_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY_LOCK);
        let ready_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_READY);
        let drained_bits = molt_asyncio_ready_queue_drain(ready_lock_bits, ready_bits);
        if exception_pending(_py) {
            return drained_bits as i64;
        }
        if !obj_from_bits(drained_bits).is_none() {
            dec_ref_bits(_py, drained_bits);
        }

        let mut waiter_bits = *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT);
        if obj_from_bits(waiter_bits).is_none() {
            waiter_bits = molt_async_sleep_new(
                MoltObject::from_float(0.0).bits(),
                MoltObject::none().bits(),
            );
            if obj_from_bits(waiter_bits).is_none() {
                return waiter_bits as i64;
            }
            *payload_ptr.add(ASYNCIO_READY_RUNNER_SLOT_WAIT) = waiter_bits;
        }
        let wait_res = molt_future_poll(waiter_bits);
        if wait_res == pending_bits_i64() {
            return pending_bits_i64();
        }
        if exception_pending(_py) {
            let exc_bits = asyncio_take_pending_exception_bits(_py);
            if asyncio_exception_kind_is(_py, exc_bits, "CancelledError") {
                dec_ref_bits(_py, exc_bits);
                asyncio_drop_payload_slots(_py, payload_ptr, 4);
                return MoltObject::none().bits() as i64;
            }
            let raised = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return raised as i64;
        }
        asyncio_drop_slot_ref(_py, payload_ptr, ASYNCIO_READY_RUNNER_SLOT_WAIT);
        pending_bits_i64()
    })
}

/// # Safety
/// - `tasks_bits` must be iterable; items must implement asyncio Future methods.
#[no_mangle]
pub extern "C" fn molt_asyncio_wait_new(
    tasks_bits: u64,
    timeout_bits: u64,
    return_when_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "wait tasks must be awaitables");
        };
        if unsafe { seq_vec_ref(task_tuple_ptr) }.is_empty() {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "asyncio.wait() requires at least one awaitable",
            );
        }
        let return_when =
            to_i64(obj_from_bits(return_when_bits)).unwrap_or(ASYNCIO_WAIT_RETURN_ALL_COMPLETED);
        if !matches!(
            return_when,
            ASYNCIO_WAIT_RETURN_ALL_COMPLETED
                | ASYNCIO_WAIT_RETURN_FIRST_COMPLETED
                | ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION
        ) {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "ValueError", "Invalid return_when value");
        }

        let mut wait_flags = 0i64;
        let mut timer_bits = MoltObject::none().bits();
        if !obj_from_bits(timeout_bits).is_none() {
            let timeout_obj = obj_from_bits(molt_float_from_obj(timeout_bits));
            if exception_pending(_py) {
                dec_ref_bits(_py, task_tuple_bits);
                return MoltObject::none().bits();
            }
            let timeout = timeout_obj.as_float().unwrap_or(0.0);
            if timeout.is_finite() && timeout > 0.0 {
                timer_bits = molt_async_sleep_new(
                    MoltObject::from_float(timeout).bits(),
                    MoltObject::none().bits(),
                );
                if obj_from_bits(timer_bits).is_none() {
                    dec_ref_bits(_py, task_tuple_bits);
                    return MoltObject::none().bits();
                }
                wait_flags |= ASYNCIO_WAIT_FLAG_HAS_TIMER;
            } else {
                wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_READY;
            }
        }

        let obj_bits = molt_future_new(
            asyncio_wait_poll_fn_addr(),
            (4 * std::mem::size_of::<u64>()) as u64,
        );
        if obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, task_tuple_bits);
            if !obj_from_bits(timer_bits).is_none() {
                dec_ref_bits(_py, timer_bits);
            }
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            dec_ref_bits(_py, task_tuple_bits);
            if !obj_from_bits(timer_bits).is_none() {
                dec_ref_bits(_py, timer_bits);
            }
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = task_tuple_bits;
            *payload_ptr.add(1) = timer_bits;
            *payload_ptr.add(2) = MoltObject::from_int(return_when).bits();
            *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to an asyncio wait wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_wait_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid wait payload");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let wrapper_ptr = current_task_ptr();
        if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr) {
            task_take_cancel_pending(wrapper_ptr);
            return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
        }
        let tasks_bits = *payload_ptr;
        let return_when =
            to_i64(obj_from_bits(*payload_ptr.add(2))).unwrap_or(ASYNCIO_WAIT_RETURN_ALL_COMPLETED);
        let Some((done_flags, mut triggered)) = asyncio_wait_scan(_py, tasks_bits, return_when)
        else {
            return MoltObject::none().bits() as i64;
        };
        let mut wait_flags = to_i64(obj_from_bits(*payload_ptr.add(3))).unwrap_or(0);
        if !triggered {
            if (wait_flags & ASYNCIO_WAIT_FLAG_TIMEOUT_READY) != 0 {
                triggered = true;
            } else if (wait_flags & ASYNCIO_WAIT_FLAG_HAS_TIMER) != 0 {
                let timer_bits = *payload_ptr.add(1);
                if !obj_from_bits(timer_bits).is_none() {
                    let timer_res = molt_future_poll(timer_bits);
                    if timer_res == pending_bits_i64() {
                        return pending_bits_i64();
                    }
                    if exception_pending(_py) {
                        return timer_res;
                    }
                }
                wait_flags |= ASYNCIO_WAIT_FLAG_TIMEOUT_READY;
                *payload_ptr.add(3) = MoltObject::from_int(wait_flags).bits();
                triggered = true;
            }
        }
        if !triggered {
            return pending_bits_i64();
        }
        let out = asyncio_wait_build_result(_py, tasks_bits, done_flags.as_slice());
        if exception_pending(_py) {
            return out;
        }
        asyncio_drop_slot_ref(_py, payload_ptr, 0);
        asyncio_drop_slot_ref(_py, payload_ptr, 1);
        asyncio_drop_slot_ref(_py, payload_ptr, 2);
        asyncio_drop_slot_ref(_py, payload_ptr, 3);
        out
    })
}

/// # Safety
/// - `tasks_bits` must be iterable; items must implement asyncio Future methods.
#[no_mangle]
pub extern "C" fn molt_asyncio_gather_new(tasks_bits: u64, return_exceptions_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(task_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, tasks_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(task_tuple_ptr) = obj_from_bits(task_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, task_tuple_bits);
            return raise_exception::<u64>(_py, "TypeError", "gather tasks must be awaitables");
        };
        let results_len = unsafe { seq_vec_ref(task_tuple_ptr) }.len();
        let payload_slots = ASYNCIO_GATHER_RESULT_OFFSET + results_len;
        let payload_bytes = (payload_slots * std::mem::size_of::<u64>()) as u64;
        let obj_bits = molt_future_new(asyncio_gather_poll_fn_addr(), payload_bytes);
        if obj_from_bits(obj_bits).is_none() {
            dec_ref_bits(_py, task_tuple_bits);
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            dec_ref_bits(_py, task_tuple_bits);
            return MoltObject::none().bits();
        };
        let return_exceptions = is_truthy(_py, obj_from_bits(return_exceptions_bits));
        let missing = missing_bits(_py);
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = task_tuple_bits;
            *payload_ptr.add(1) = MoltObject::from_bool(return_exceptions).bits();
            *payload_ptr.add(2) = MoltObject::from_int(0).bits();
            *payload_ptr.add(3) = MoltObject::from_int(results_len as i64).bits();
            for idx in 0..results_len {
                let slot = payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx);
                *slot = missing;
                inc_ref_bits(_py, missing);
            }
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to an asyncio gather wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_gather_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < ASYNCIO_GATHER_RESULT_OFFSET * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid gather payload");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let payload_slots = payload_bytes / std::mem::size_of::<u64>();
        let results_len = payload_slots.saturating_sub(ASYNCIO_GATHER_RESULT_OFFSET);
        let tasks_bits = *payload_ptr;
        let Some(tasks_ptr) = obj_from_bits(tasks_bits).as_ptr() else {
            return raise_exception::<i64>(_py, "TypeError", "gather tasks must be awaitables");
        };
        let tasks = seq_vec_ref(tasks_ptr);
        if tasks.len() != results_len {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid gather payload state");
        }
        let wrapper_ptr = current_task_ptr();
        if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr) {
            let missing = missing_bits(_py);
            asyncio_gather_cancel_pending(_py, tasks_bits, payload_ptr, results_len, missing);
            task_take_cancel_pending(wrapper_ptr);
            return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
        }

        let missing = missing_bits(_py);
        let return_exceptions = is_truthy(_py, obj_from_bits(*payload_ptr.add(1)));
        for (idx, &task_bits) in tasks.iter().enumerate() {
            if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) != missing {
                continue;
            }
            let done = match asyncio_method_truthy(_py, task_bits, b"done") {
                Some(value) => value,
                None => return MoltObject::none().bits() as i64,
            };
            if !done {
                continue;
            }
            let result_bits = asyncio_call_method0(_py, task_bits, b"result");
            if exception_pending(_py) {
                if return_exceptions {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    asyncio_gather_store_result(_py, payload_ptr, idx, exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits() as i64;
                    }
                    continue;
                }
                let exc_bits = molt_exception_last();
                molt_exception_clear();
                asyncio_gather_cancel_pending(_py, tasks_bits, payload_ptr, results_len, missing);
                let raised = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return raised as i64;
            }
            asyncio_gather_store_result(_py, payload_ptr, idx, result_bits);
            dec_ref_bits(_py, result_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
        }

        for idx in 0..results_len {
            if *payload_ptr.add(ASYNCIO_GATHER_RESULT_OFFSET + idx) == missing {
                return pending_bits_i64();
            }
        }
        let out_bits = asyncio_gather_build_list(_py, payload_ptr, results_len);
        if obj_from_bits(out_bits).is_none() {
            return MoltObject::none().bits() as i64;
        }
        for idx in 0..payload_slots {
            asyncio_drop_slot_ref(_py, payload_ptr, idx);
        }
        out_bits as i64
    })
}

/// # Safety
/// - `future_ptr` must be a valid Molt wait future pointer.
pub(crate) unsafe fn asyncio_wait_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    for idx in 0..4usize {
        asyncio_drop_slot_ref(_py, payload_ptr, idx);
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt gather future pointer.
pub(crate) unsafe fn asyncio_gather_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_slots = payload_bytes / std::mem::size_of::<u64>();
    if payload_slots < ASYNCIO_GATHER_RESULT_OFFSET {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    for idx in 0..payload_slots {
        asyncio_drop_slot_ref(_py, payload_ptr, idx);
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt wait_for future pointer.
pub(crate) unsafe fn asyncio_wait_for_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    for idx in 0..3usize {
        let bits = *payload_ptr.add(idx);
        if bits != 0 && !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
            *payload_ptr.add(idx) = MoltObject::none().bits();
        }
    }
}

/// # Safety
/// - `future_ptr` must be a valid Molt stream-reader read future pointer.
pub(crate) unsafe fn asyncio_stream_reader_read_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 3);
}

/// # Safety
/// - `future_ptr` must be a valid Molt stream-reader readline future pointer.
pub(crate) unsafe fn asyncio_stream_reader_readline_task_drop(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 2 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 2);
}

/// # Safety
/// - `future_ptr` must be a valid Molt stream-send future pointer.
pub(crate) unsafe fn asyncio_stream_send_all_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 3);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket-reader read future pointer.
pub(crate) unsafe fn asyncio_socket_reader_read_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 4);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket-reader readline future pointer.
pub(crate) unsafe fn asyncio_socket_reader_readline_task_drop(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 3);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket recv future pointer.
pub(crate) unsafe fn asyncio_sock_recv_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 4);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket connect future pointer.
pub(crate) unsafe fn asyncio_sock_connect_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 4);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket accept future pointer.
pub(crate) unsafe fn asyncio_sock_accept_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 3 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 3);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket recv_into future pointer.
pub(crate) unsafe fn asyncio_sock_recv_into_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 5 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 5);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket sendall future pointer.
pub(crate) unsafe fn asyncio_sock_sendall_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 6 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 6);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket recvfrom future pointer.
pub(crate) unsafe fn asyncio_sock_recvfrom_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 4);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket recvfrom_into future pointer.
pub(crate) unsafe fn asyncio_sock_recvfrom_into_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 5 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 5);
}

/// # Safety
/// - `future_ptr` must be a valid Molt socket sendto future pointer.
pub(crate) unsafe fn asyncio_sock_sendto_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 5 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 5);
}

/// # Safety
/// - `future_ptr` must be a valid Molt timer-handle future pointer.
pub(crate) unsafe fn asyncio_timer_handle_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 7 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 7);
}

/// # Safety
/// - `future_ptr` must be a valid Molt fd-watcher future pointer.
pub(crate) unsafe fn asyncio_fd_watcher_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 6 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 6);
}

/// # Safety
/// - `future_ptr` must be a valid Molt server-accept-loop future pointer.
pub(crate) unsafe fn asyncio_server_accept_loop_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 8 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 8);
}

/// # Safety
/// - `future_ptr` must be a valid Molt ready-runner future pointer.
pub(crate) unsafe fn asyncio_ready_runner_task_drop(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = header_from_obj_ptr(future_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    if payload_bytes < 4 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    asyncio_drop_payload_slots(_py, payload_ptr, 4);
}

fn wait_for_raise_timeout(_py: &PyToken<'_>) -> i64 {
    raise_exception::<i64>(_py, "TimeoutError", "")
}

unsafe fn wait_for_flags(payload_ptr: *mut u64) -> i64 {
    to_i64(obj_from_bits(*payload_ptr.add(3))).unwrap_or(0)
}

unsafe fn wait_for_set_flags(payload_ptr: *mut u64, flags: i64) {
    *payload_ptr.add(3) = MoltObject::from_int(flags).bits();
}

unsafe fn wait_for_drop_slot_ref(_py: &PyToken<'_>, payload_ptr: *mut u64, idx: usize) {
    let bits = *payload_ptr.add(idx);
    if bits != 0 && !obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
    }
    *payload_ptr.add(idx) = MoltObject::none().bits();
}

unsafe fn wait_for_clear_pending_exception(_py: &PyToken<'_>) {
    if !exception_pending(_py) {
        return;
    }
    let exc_bits = molt_exception_last();
    dec_ref_bits(_py, exc_bits);
    molt_exception_clear();
}

unsafe fn wait_for_has_method(_py: &PyToken<'_>, obj_bits: u64, method: &[u8]) -> bool {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return false;
    };
    let Some(method_name_bits) = attr_name_bits_from_bytes(_py, method) else {
        return false;
    };
    let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, method_name_bits) else {
        return false;
    };
    dec_ref_bits(_py, method_bits);
    true
}

unsafe fn wait_for_is_supported_target(_py: &PyToken<'_>, target_bits: u64) -> bool {
    if resolve_task_ptr(target_bits).is_some() {
        return true;
    }
    wait_for_has_method(_py, target_bits, b"done")
        && wait_for_has_method(_py, target_bits, b"cancel")
        && wait_for_has_method(_py, target_bits, b"result")
}

unsafe fn wait_for_poll_target(_py: &PyToken<'_>, target_bits: u64) -> i64 {
    if resolve_task_ptr(target_bits).is_some() {
        return molt_future_poll(target_bits);
    }
    let Some(done) = asyncio_method_truthy(_py, target_bits, b"done") else {
        return MoltObject::none().bits() as i64;
    };
    if !done {
        return pending_bits_i64();
    }
    asyncio_call_method0(_py, target_bits, b"result") as i64
}

unsafe fn wait_for_cancel_target(_py: &PyToken<'_>, target_bits: u64) {
    if let Some(task_ptr) = resolve_task_ptr(target_bits) {
        cancel_future_task(_py, task_ptr, None);
        return;
    }
    asyncio_cancel_task(_py, target_bits);
}

/// # Safety
/// - `future_bits` must reference an awaitable future/task.
#[no_mangle]
pub extern "C" fn molt_asyncio_wait_for_new(future_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = unsafe { wait_for_is_supported_target(_py, future_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if !supported {
            return raise_exception::<u64>(_py, "TypeError", "object is not awaitable");
        }
        let payload = (4 * std::mem::size_of::<u64>()) as u64;
        let obj_bits = molt_future_new(asyncio_wait_for_poll_fn_addr(), payload);
        if obj_from_bits(obj_bits).is_none() {
            return obj_bits;
        }
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = future_bits;
            *payload_ptr.add(1) = timeout_bits;
            *payload_ptr.add(2) = MoltObject::none().bits();
            *payload_ptr.add(3) = MoltObject::from_int(0).bits();
            inc_ref_bits(_py, future_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        obj_bits
    })
}

/// # Safety
/// - `obj_bits` must be a valid pointer to a wait_for wrapper future.
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_wait_for_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>());
        if payload_bytes < 4 * std::mem::size_of::<u64>() {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid wait_for payload");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let target_bits = *payload_ptr;
        let wrapper_ptr = current_task_ptr();
        if !wrapper_ptr.is_null() && wrapper_ptr == obj_ptr && task_cancel_pending(wrapper_ptr) {
            let timer_bits = *payload_ptr.add(2);
            if let Some(timer_ptr) = resolve_task_ptr(timer_bits) {
                cancel_future_task(_py, timer_ptr, None);
            }
            wait_for_cancel_target(_py, target_bits);
            task_take_cancel_pending(wrapper_ptr);
            return raise_cancelled_with_message::<i64>(_py, wrapper_ptr);
        }

        if (*header).state == 0 {
            let mut flags = wait_for_flags(payload_ptr);
            let timeout_bits = *payload_ptr.add(1);
            if obj_from_bits(timeout_bits).is_none() {
                wait_for_drop_slot_ref(_py, payload_ptr, 1);
            } else {
                let timeout_obj = obj_from_bits(molt_float_from_obj(timeout_bits));
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
                let timeout = timeout_obj.as_float().unwrap_or(0.0);
                let immediate = !timeout.is_finite() || timeout <= 0.0;
                if immediate {
                    flags |= WAIT_FOR_FLAG_FORCE_TIMEOUT;
                    wait_for_set_flags(payload_ptr, flags);
                    wait_for_drop_slot_ref(_py, payload_ptr, 1);
                    let Some(done_now) = asyncio_method_truthy(_py, target_bits, b"done") else {
                        return MoltObject::none().bits() as i64;
                    };
                    if !done_now {
                        wait_for_cancel_target(_py, target_bits);
                        (*header).state = WAIT_FOR_STATE_CANCEL_WAIT;
                        return pending_bits_i64();
                    }
                } else {
                    let timer_bits = molt_async_sleep_new(
                        MoltObject::from_float(timeout).bits(),
                        MoltObject::none().bits(),
                    );
                    if obj_from_bits(timer_bits).is_none() {
                        return timer_bits as i64;
                    }
                    *payload_ptr.add(2) = timer_bits;
                    inc_ref_bits(_py, timer_bits);
                    flags |= WAIT_FOR_FLAG_HAS_TIMER;
                    wait_for_set_flags(payload_ptr, flags);
                    wait_for_drop_slot_ref(_py, payload_ptr, 1);
                }
            }
            (*header).state = WAIT_FOR_STATE_PENDING;
        }

        if (*header).state == WAIT_FOR_STATE_CANCEL_WAIT {
            let target_res = wait_for_poll_target(_py, target_bits);
            if target_res == pending_bits_i64() {
                return pending_bits_i64();
            }
            let flags = wait_for_flags(payload_ptr);
            if (flags & WAIT_FOR_FLAG_FORCE_TIMEOUT) != 0 {
                wait_for_clear_pending_exception(_py);
                return wait_for_raise_timeout(_py);
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                dec_ref_bits(_py, kind_bits);
                dec_ref_bits(_py, exc_bits);
                if kind.as_deref() == Some("CancelledError") {
                    molt_exception_clear();
                    return wait_for_raise_timeout(_py);
                }
            }
            return target_res;
        }

        let target_res = wait_for_poll_target(_py, target_bits);
        if target_res != pending_bits_i64() {
            wait_for_drop_slot_ref(_py, payload_ptr, 2);
            return target_res;
        }

        let flags = wait_for_flags(payload_ptr);
        if (flags & WAIT_FOR_FLAG_HAS_TIMER) == 0 {
            return pending_bits_i64();
        }
        let timer_bits = *payload_ptr.add(2);
        if obj_from_bits(timer_bits).is_none() {
            return pending_bits_i64();
        }
        let timer_res = molt_future_poll(timer_bits);
        if timer_res == pending_bits_i64() {
            return pending_bits_i64();
        }
        if exception_pending(_py) {
            return timer_res;
        }
        wait_for_cancel_target(_py, target_bits);
        wait_for_drop_slot_ref(_py, payload_ptr, 2);
        (*header).state = WAIT_FOR_STATE_CANCEL_WAIT;
        pending_bits_i64()
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
