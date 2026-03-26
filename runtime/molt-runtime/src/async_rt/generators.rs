use crate::PyToken;
use std::sync::OnceLock;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::{Duration, Instant};

use molt_obj_model::MoltObject;

use crate::concurrency::GilGuard;
use crate::object::HEADER_FLAG_COROUTINE;
use crate::object::accessors::resolve_obj_ptr;
use crate::{
    ACTIVE_EXCEPTION_STACK, ASYNCGEN_CONTROL_SIZE, ASYNCGEN_FIRSTITER_OFFSET, ASYNCGEN_GEN_OFFSET,
    ASYNCGEN_OP_ACLOSE, ASYNCGEN_OP_ANEXT, ASYNCGEN_OP_ASEND, ASYNCGEN_OP_ATHROW,
    ASYNCGEN_PENDING_OFFSET, ASYNCGEN_RUNNING_OFFSET, GEN_CLOSED_OFFSET, GEN_CONTROL_SIZE,
    GEN_EXC_DEPTH_OFFSET, GEN_SEND_OFFSET, GEN_THROW_OFFSET, GEN_YIELD_FROM_OFFSET,
    HEADER_FLAG_BLOCK_ON, HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED,
    HEADER_FLAG_SPAWN_RETAIN, MoltHeader, PtrSlot, TASK_KIND_COROUTINE, TASK_KIND_FUTURE,
    TASK_KIND_GENERATOR, TYPE_ID_ASYNC_GENERATOR, TYPE_ID_EXCEPTION, TYPE_ID_GENERATOR,
    TYPE_ID_OBJECT, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_dict_with_pairs,
    alloc_exception, alloc_list, alloc_object, alloc_tuple, async_sleep_poll_fn_addr,
    async_trace_enabled, asyncgen_poll_fn_addr, asyncgen_registry, asyncio_fd_watcher_poll_fn_addr,
    asyncio_gather_poll_fn_addr, asyncio_ready_runner_poll_fn_addr,
    asyncio_server_accept_loop_poll_fn_addr, asyncio_sock_accept_poll_fn_addr,
    asyncio_sock_connect_poll_fn_addr, asyncio_sock_recv_into_poll_fn_addr,
    asyncio_sock_recv_poll_fn_addr, asyncio_sock_recvfrom_into_poll_fn_addr,
    asyncio_sock_recvfrom_poll_fn_addr, asyncio_sock_sendall_poll_fn_addr,
    asyncio_sock_sendto_poll_fn_addr, asyncio_socket_reader_read_poll_fn_addr,
    asyncio_socket_reader_readline_poll_fn_addr, asyncio_stream_reader_read_poll_fn_addr,
    asyncio_stream_reader_readline_poll_fn_addr, asyncio_stream_send_all_poll_fn_addr,
    asyncio_timer_handle_poll_fn_addr, asyncio_wait_for_poll_fn_addr, asyncio_wait_poll_fn_addr,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, await_waiter_clear,
    await_waiter_register, await_waiters, await_waiters_take, call_callable0, call_callable1,
    call_callable2, call_callable3, call_poll_fn, class_name_for_error, clear_exception,
    clear_exception_state, context_stack_store, context_stack_take, current_task_ptr, dec_ref_bits,
    exception_args_bits, exception_clear_reason_set, exception_context_align_depth,
    exception_context_fallback_pop, exception_context_fallback_push, exception_kind_bits,
    exception_pending, exception_stack_depth, exception_stack_set_depth,
    exception_type_bits_from_name, fn_ptr_code_get, generator_context_stack_store,
    generator_context_stack_take, generator_exception_stack_store, generator_exception_stack_take,
    generator_raise_active, header_from_obj_ptr, inc_ref_bits, instant_from_monotonic_secs,
    io_wait_poll_fn_addr, is_truthy, issubclass_bits, maybe_ptr_from_bits, missing_bits,
    molt_anext, molt_bytes_from_obj, molt_call_bind, molt_callargs_expand_star, molt_callargs_new,
    molt_exception_clear, molt_exception_kind, molt_exception_last, molt_exception_set_last,
    molt_float_from_obj, molt_getitem_method, molt_io_wait_new, molt_is_callable, molt_len,
    molt_raise, molt_set_add, molt_set_new, molt_slice_new, molt_socket_reader_read,
    molt_socket_reader_readline, molt_str_from_obj, molt_stream_reader_read,
    molt_stream_reader_readline, molt_stream_send_obj, obj_from_bits, object_class_bits,
    object_mark_has_ptrs, object_type_id, pending_bits_i64, process_poll_fn_addr,
    promise_poll_fn_addr, ptr_from_bits, raise_cancelled_with_message, raise_exception,
    raise_os_error_errno, register_task_token, resolve_task_ptr, runtime_state, seq_vec_ref,
    set_generator_raise, string_obj_to_owned, task_cancel_message_clear, task_cancel_message_set,
    task_cancel_pending, task_exception_baseline_drop, task_exception_depth_drop,
    task_exception_stack_drop, task_has_token, task_last_exceptions, task_mark_done,
    task_set_cancel_pending, task_take_cancel_pending, task_waiting_on, thread_poll_fn_addr,
    to_f64, to_i64, token_id_from_bits, tuple_from_iter_bits, type_name, wake_task_ptr,
};

use crate::state::runtime_state::{AsyncGenLocalsEntry, GenLocalsEntry};

#[cfg(not(target_arch = "wasm32"))]
use crate::{is_block_on_task, process_task_state, thread_task_state};

pub(super) fn promise_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
            Some("1")
        )
    })
}

pub(super) fn sleep_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| matches!(std::env::var("MOLT_TRACE_SLEEP").ok().as_deref(), Some("1")))
}

pub(super) fn asyncgen_locals_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_ASYNCGEN_LOCALS_TRACE").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

pub(super) fn asyncio_connect_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_TRACE_ASYNCIO_CONNECT").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

#[inline]
pub(super) fn debug_current_task() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1"))
}

pub(super) const ASYNC_SLEEP_YIELD_SECS: f64 = 0.000_001;
pub(super) const ASYNC_SLEEP_YIELD_SENTINEL: f64 = -1.0;
pub(super) const ASYNCIO_WAIT_RETURN_ALL_COMPLETED: i64 = 0;
pub(super) const ASYNCIO_WAIT_RETURN_FIRST_COMPLETED: i64 = 1;
pub(super) const ASYNCIO_WAIT_RETURN_FIRST_EXCEPTION: i64 = 2;
pub(super) const ASYNCIO_WAIT_FLAG_HAS_TIMER: i64 = 1;
pub(super) const ASYNCIO_WAIT_FLAG_TIMEOUT_READY: i64 = 2;
pub(super) const ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED: i64 = 4;
pub(super) const ASYNCIO_WAIT_FLAG_TIMEOUT_DEFERRED_2: i64 = 8;
pub(super) const ASYNCIO_GATHER_RESULT_OFFSET: usize = 4;
pub(super) const ASYNCIO_SOCKET_IO_EVENT_READ: i64 = 1;
pub(super) const ASYNCIO_SOCKET_IO_EVENT_WRITE: i64 = 2;
pub(super) const ASYNCIO_STREAM_READER_READ_SLOT_READER: usize = 0;
pub(super) const ASYNCIO_STREAM_READER_READ_SLOT_N: usize = 1;
pub(super) const ASYNCIO_STREAM_READER_READ_SLOT_WAIT: usize = 2;
pub(super) const ASYNCIO_STREAM_READER_READLINE_SLOT_READER: usize = 0;
pub(super) const ASYNCIO_STREAM_READER_READLINE_SLOT_WAIT: usize = 1;
pub(super) const ASYNCIO_STREAM_SEND_ALL_SLOT_STREAM: usize = 0;
pub(super) const ASYNCIO_STREAM_SEND_ALL_SLOT_DATA: usize = 1;
pub(super) const ASYNCIO_STREAM_SEND_ALL_SLOT_WAIT: usize = 2;
pub(super) const ASYNCIO_SOCKET_READER_READ_SLOT_READER: usize = 0;
pub(super) const ASYNCIO_SOCKET_READER_READ_SLOT_N: usize = 1;
pub(super) const ASYNCIO_SOCKET_READER_READ_SLOT_FD: usize = 2;
pub(super) const ASYNCIO_SOCKET_READER_READ_SLOT_WAIT: usize = 3;
pub(super) const ASYNCIO_SOCKET_READER_READLINE_SLOT_READER: usize = 0;
pub(super) const ASYNCIO_SOCKET_READER_READLINE_SLOT_FD: usize = 1;
pub(super) const ASYNCIO_SOCKET_READER_READLINE_SLOT_WAIT: usize = 2;
pub(super) const ASYNCIO_SOCK_RECV_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_RECV_SLOT_SIZE: usize = 1;
pub(super) const ASYNCIO_SOCK_RECV_SLOT_FD: usize = 2;
pub(super) const ASYNCIO_SOCK_RECV_SLOT_WAIT: usize = 3;
pub(super) const ASYNCIO_SOCK_CONNECT_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_CONNECT_SLOT_ADDR: usize = 1;
pub(super) const ASYNCIO_SOCK_CONNECT_SLOT_FD: usize = 2;
pub(super) const ASYNCIO_SOCK_CONNECT_SLOT_WAIT: usize = 3;
pub(super) const ASYNCIO_SOCK_ACCEPT_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_ACCEPT_SLOT_FD: usize = 1;
pub(super) const ASYNCIO_SOCK_ACCEPT_SLOT_WAIT: usize = 2;
pub(super) const ASYNCIO_SOCK_RECV_INTO_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_RECV_INTO_SLOT_BUF: usize = 1;
pub(super) const ASYNCIO_SOCK_RECV_INTO_SLOT_NBYTES: usize = 2;
pub(super) const ASYNCIO_SOCK_RECV_INTO_SLOT_FD: usize = 3;
pub(super) const ASYNCIO_SOCK_RECV_INTO_SLOT_WAIT: usize = 4;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_DATA: usize = 1;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_TOTAL: usize = 2;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_DLEN: usize = 3;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_FD: usize = 4;
pub(super) const ASYNCIO_SOCK_SENDALL_SLOT_WAIT: usize = 5;
pub(super) const ASYNCIO_SOCK_RECVFROM_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_RECVFROM_SLOT_SIZE: usize = 1;
pub(super) const ASYNCIO_SOCK_RECVFROM_SLOT_FD: usize = 2;
pub(super) const ASYNCIO_SOCK_RECVFROM_SLOT_WAIT: usize = 3;
pub(super) const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_BUF: usize = 1;
pub(super) const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_NBYTES: usize = 2;
pub(super) const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_FD: usize = 3;
pub(super) const ASYNCIO_SOCK_RECVFROM_INTO_SLOT_WAIT: usize = 4;
pub(super) const ASYNCIO_SOCK_SENDTO_SLOT_SOCK: usize = 0;
pub(super) const ASYNCIO_SOCK_SENDTO_SLOT_DATA: usize = 1;
pub(super) const ASYNCIO_SOCK_SENDTO_SLOT_ADDR: usize = 2;
pub(super) const ASYNCIO_SOCK_SENDTO_SLOT_FD: usize = 3;
pub(super) const ASYNCIO_SOCK_SENDTO_SLOT_WAIT: usize = 4;
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

pub(super) unsafe fn generator_slot_ptr(ptr: *mut u8, offset: usize) -> *mut u64 {
    unsafe { ptr.add(offset) as *mut u64 }
}

pub(super) unsafe fn generator_set_slot(_py: &PyToken<'_>, ptr: *mut u8, offset: usize, bits: u64) {
    unsafe {
        crate::gil_assert();
        let slot = generator_slot_ptr(ptr, offset);
        let old_bits = *slot;
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
    }
}

pub(crate) unsafe fn generator_closed(ptr: *mut u8) -> bool {
    unsafe {
        let bits = *generator_slot_ptr(ptr, GEN_CLOSED_OFFSET);
        obj_from_bits(bits).as_bool().unwrap_or(false)
    }
}

pub(super) unsafe fn generator_set_closed(_py: &PyToken<'_>, ptr: *mut u8, closed: bool) {
    unsafe {
        crate::gil_assert();
        let bits = MoltObject::from_bool(closed).bits();
        generator_set_slot(_py, ptr, GEN_CLOSED_OFFSET, bits);
    }
}

pub(crate) unsafe fn generator_running(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        ((*header).flags & HEADER_FLAG_GEN_RUNNING) != 0
    }
}

pub(super) unsafe fn generator_set_running(_py: &PyToken<'_>, ptr: *mut u8, running: bool) {
    unsafe {
        crate::gil_assert();
        let header = header_from_obj_ptr(ptr);
        if running {
            (*header).flags |= HEADER_FLAG_GEN_RUNNING;
        } else {
            (*header).flags &= !HEADER_FLAG_GEN_RUNNING;
        }
    }
}

pub(crate) unsafe fn generator_started(ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(ptr);
        ((*header).flags & HEADER_FLAG_GEN_STARTED) != 0
    }
}

pub(crate) unsafe fn generator_yieldfrom_bits(ptr: *mut u8) -> u64 {
    unsafe { *generator_slot_ptr(ptr, GEN_YIELD_FROM_OFFSET) }
}

pub(super) unsafe fn generator_close_yieldfrom(_py: &PyToken<'_>, iter_bits: u64) -> bool {
    unsafe {
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
}

pub(super) fn resolve_sleep_target(_py: &PyToken<'_>, future_ptr: *mut u8) -> *mut u8 {
    if future_ptr.is_null() {
        return future_ptr;
    }
    let mut cursor = future_ptr;
    for _ in 0..16 {
        let poll_fn = crate::object::object_poll_fn(cursor);
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

pub(super) unsafe fn generator_set_started(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        let header = header_from_obj_ptr(ptr);
        (*header).flags |= HEADER_FLAG_GEN_STARTED;
    }
}

pub(super) unsafe fn generator_pending_throw(ptr: *mut u8) -> bool {
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

pub(super) fn generator_unpack_pair(_py: &PyToken<'_>, bits: u64) -> Option<(u64, bool)> {
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

pub(super) unsafe fn raise_stop_iteration_from_value(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if obj_from_bits(value_bits).is_none() {
        return raise_exception::<_>(_py, "StopIteration", "");
    }
    let msg_bits = molt_str_from_obj(value_bits);
    let msg = string_obj_to_owned(obj_from_bits(msg_bits)).unwrap_or_default();
    dec_ref_bits(_py, msg_bits);
    raise_exception::<_>(_py, "StopIteration", &msg)
}

unsafe fn generator_method_result(_py: &PyToken<'_>, res_bits: u64) -> u64 {
    unsafe {
        if let Some((val_bits, done)) = generator_unpack_pair(_py, res_bits) {
            if done {
                return raise_stop_iteration_from_value(_py, val_bits);
            }
            inc_ref_bits(_py, val_bits);
            return val_bits;
        }
        res_bits
    }
}
