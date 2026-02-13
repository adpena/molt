use molt_obj_model::MoltObject;

use crate::{
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK, PyToken, exception_context_align_depth,
    exception_context_fallback_pop, exception_context_fallback_push, exception_stack_baseline_get,
    exception_stack_baseline_set, exception_stack_depth, exception_stack_set_depth,
    set_task_raise_active, task_exception_baseline_store, task_exception_baseline_take,
    task_exception_depth_store, task_exception_depth_take, task_exception_handler_stack_store,
    task_exception_handler_stack_take, task_exception_stack_store, task_exception_stack_take,
    task_raise_active,
};

#[cfg(target_arch = "wasm32")]
use crate::raise_exception;

use super::scheduler::CURRENT_TASK;

#[cfg(target_arch = "wasm32")]
#[inline]
fn wasm_poll_slot(offset: u64) -> u64 {
    crate::wasm_table_base().saturating_add(offset)
}

#[cfg(target_arch = "wasm32")]
const WASM_POLL_SLOT_MAX_OFFSET: u64 = 32;

#[cfg(target_arch = "wasm32")]
#[inline]
fn normalize_wasm_poll_fn_addr(poll_fn_addr: u64) -> u64 {
    let table_base = crate::wasm_table_base();
    if poll_fn_addr >= table_base {
        return poll_fn_addr;
    }
    let legacy_base = crate::WASM_TABLE_BASE_FALLBACK;
    if table_base == legacy_base || poll_fn_addr < legacy_base {
        return poll_fn_addr;
    }
    let slot_offset = poll_fn_addr - legacy_base;
    if slot_offset <= WASM_POLL_SLOT_MAX_OFFSET {
        return table_base.saturating_add(slot_offset);
    }
    poll_fn_addr
}

#[inline]
pub(crate) fn async_sleep_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(1)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_async_sleep)
    }
}

#[inline]
pub(crate) fn anext_default_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(2)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_anext_default_poll)
    }
}

#[inline]
pub(crate) fn asyncgen_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(3)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncgen_poll)
    }
}

#[inline]
pub(crate) fn promise_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(4)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_promise_poll)
    }
}

#[inline]
pub(crate) fn contextlib_asyncgen_enter_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(29)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_contextlib_asyncgen_enter_poll)
    }
}

#[inline]
pub(crate) fn contextlib_asyncgen_exit_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(30)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_contextlib_asyncgen_exit_poll)
    }
}

#[inline]
pub(crate) fn contextlib_async_exitstack_exit_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(31)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_contextlib_async_exitstack_exit_poll)
    }
}

#[inline]
pub(crate) fn contextlib_async_exitstack_enter_context_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(32)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_contextlib_async_exitstack_enter_context_poll)
    }
}

#[inline]
pub(crate) fn io_wait_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(5)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_io_wait)
    }
}

#[inline]
pub(crate) fn ws_wait_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(8)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_ws_wait)
    }
}

#[inline]
pub(crate) fn thread_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(6)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_thread_poll)
    }
}

#[inline]
pub(crate) fn process_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(7)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_process_poll)
    }
}

#[inline]
pub(crate) fn asyncio_wait_for_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(9)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_wait_for_poll)
    }
}

#[inline]
pub(crate) fn asyncio_wait_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(10)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_wait_poll)
    }
}

#[inline]
pub(crate) fn asyncio_gather_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(11)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_gather_poll)
    }
}

#[inline]
pub(crate) fn asyncio_timer_handle_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(25)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_timer_handle_poll)
    }
}

#[inline]
pub(crate) fn asyncio_fd_watcher_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(26)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_fd_watcher_poll)
    }
}

#[inline]
pub(crate) fn asyncio_server_accept_loop_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(27)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_server_accept_loop_poll)
    }
}

#[inline]
pub(crate) fn asyncio_ready_runner_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(28)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_ready_runner_poll)
    }
}

#[inline]
pub(crate) fn asyncio_socket_reader_read_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(12)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_socket_reader_read_poll)
    }
}

#[inline]
pub(crate) fn asyncio_socket_reader_readline_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(13)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_socket_reader_readline_poll)
    }
}

#[inline]
pub(crate) fn asyncio_stream_reader_read_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(14)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_stream_reader_read_poll)
    }
}

#[inline]
pub(crate) fn asyncio_stream_reader_readline_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(15)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_stream_reader_readline_poll)
    }
}

#[inline]
pub(crate) fn asyncio_stream_send_all_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(16)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_stream_send_all_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_recv_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(17)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_recv_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_connect_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(18)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_connect_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_accept_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(19)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_accept_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_recv_into_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(20)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_recv_into_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_sendall_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(21)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_sendall_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_recvfrom_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(22)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_recvfrom_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_recvfrom_into_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(23)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_recvfrom_into_poll)
    }
}

#[inline]
pub(crate) fn asyncio_sock_sendto_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot(24)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_sendto_poll)
    }
}

pub(crate) unsafe fn call_poll_fn(_py: &PyToken<'_>, poll_fn_addr: u64, task_ptr: *mut u8) -> i64 {
    unsafe {
        let addr = task_ptr.expose_provenance() as u64;
        #[cfg(target_arch = "wasm32")]
        {
            let normalized_poll_fn_addr = normalize_wasm_poll_fn_addr(poll_fn_addr);
            if std::env::var("MOLT_WASM_POLL_DEBUG").as_deref() == Ok("1") {
                if normalized_poll_fn_addr == poll_fn_addr {
                    eprintln!("molt wasm poll: fn=0x{poll_fn_addr:x}");
                } else {
                    eprintln!(
                        "molt wasm poll: fn=0x{poll_fn_addr:x} normalized=0x{normalized_poll_fn_addr:x}"
                    );
                }
            }
            if normalized_poll_fn_addr < crate::wasm_table_base() {
                return raise_exception::<i64>(_py, "RuntimeError", "invalid wasm poll function");
            }
            return crate::molt_call_indirect1(normalized_poll_fn_addr, addr);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let poll_fn: extern "C" fn(u64) -> i64 = std::mem::transmute(poll_fn_addr as usize);
            poll_fn(addr)
        }
    }
}

pub(crate) unsafe fn poll_future_with_task_stack(
    _py: &PyToken<'_>,
    task_ptr: *mut u8,
    poll_fn_addr: u64,
) -> i64 {
    unsafe {
        let debug_task = std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1");
        let prev_task = CURRENT_TASK.with(|cell| {
            let prev = cell.get();
            cell.set(task_ptr);
            prev
        });
        if debug_task && prev_task.is_null() {
            eprintln!(
                "molt task trace: prev_task=null set task=0x{:x}",
                task_ptr as usize
            );
        }
        let caller_depth = exception_stack_depth();
        let caller_baseline = exception_stack_baseline_get();
        let caller_handlers =
            EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_active =
            ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
        let caller_context = caller_active
            .last()
            .copied()
            .unwrap_or(MoltObject::none().bits());
        exception_context_fallback_push(caller_context);
        let task_baseline = task_exception_baseline_take(_py, task_ptr);
        exception_stack_baseline_set(task_baseline);
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
        let res = call_poll_fn(_py, poll_fn_addr, task_ptr);
        set_task_raise_active(prev_raise);
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
        if debug_task && prev_task.is_null() {
            eprintln!(
                "molt task trace: restoring prev_task=null after task=0x{:x}",
                task_ptr as usize
            );
        }
        CURRENT_TASK.with(|cell| cell.set(prev_task));
        res
    }
}
