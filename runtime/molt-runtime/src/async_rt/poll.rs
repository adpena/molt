use molt_obj_model::MoltObject;

use crate::{
    exception_context_align_depth, exception_context_fallback_pop, exception_context_fallback_push,
    exception_stack_baseline_get, exception_stack_baseline_set, exception_stack_depth,
    exception_stack_set_depth, set_task_raise_active, task_exception_baseline_store,
    task_exception_baseline_take, task_exception_depth_store, task_exception_depth_take,
    task_exception_handler_stack_store, task_exception_handler_stack_take,
    task_exception_stack_store, task_exception_stack_take, task_raise_active, PyToken,
    ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK,
};

#[cfg(target_arch = "wasm32")]
use crate::{
    raise_exception, WASM_TABLE_BASE, WASM_TABLE_IDX_ANEXT_DEFAULT_POLL,
    WASM_TABLE_IDX_ASYNCGEN_POLL, WASM_TABLE_IDX_ASYNCIO_FD_WATCHER_POLL,
    WASM_TABLE_IDX_ASYNCIO_GATHER_POLL, WASM_TABLE_IDX_ASYNCIO_READY_RUNNER_POLL,
    WASM_TABLE_IDX_ASYNCIO_SERVER_ACCEPT_LOOP_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCKET_READER_READLINE_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCKET_READER_READ_POLL, WASM_TABLE_IDX_ASYNCIO_SOCK_ACCEPT_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCK_CONNECT_POLL, WASM_TABLE_IDX_ASYNCIO_SOCK_RECVFROM_INTO_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCK_RECVFROM_POLL, WASM_TABLE_IDX_ASYNCIO_SOCK_RECV_INTO_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCK_RECV_POLL, WASM_TABLE_IDX_ASYNCIO_SOCK_SENDALL_POLL,
    WASM_TABLE_IDX_ASYNCIO_SOCK_SENDTO_POLL, WASM_TABLE_IDX_ASYNCIO_STREAM_READER_READLINE_POLL,
    WASM_TABLE_IDX_ASYNCIO_STREAM_READER_READ_POLL, WASM_TABLE_IDX_ASYNCIO_STREAM_SEND_ALL_POLL,
    WASM_TABLE_IDX_ASYNCIO_TIMER_HANDLE_POLL, WASM_TABLE_IDX_ASYNCIO_WAIT_FOR_POLL,
    WASM_TABLE_IDX_ASYNCIO_WAIT_POLL, WASM_TABLE_IDX_ASYNC_SLEEP,
    WASM_TABLE_IDX_CONTEXTLIB_ASYNCGEN_ENTER_POLL, WASM_TABLE_IDX_CONTEXTLIB_ASYNCGEN_EXIT_POLL,
    WASM_TABLE_IDX_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT_POLL,
    WASM_TABLE_IDX_CONTEXTLIB_ASYNC_EXITSTACK_EXIT_POLL, WASM_TABLE_IDX_IO_WAIT,
    WASM_TABLE_IDX_PROCESS_POLL, WASM_TABLE_IDX_PROMISE_POLL, WASM_TABLE_IDX_THREAD_POLL,
    WASM_TABLE_IDX_WS_WAIT,
};

use super::scheduler::CURRENT_TASK;

#[inline]
pub(crate) fn async_sleep_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ASYNC_SLEEP
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
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ANEXT_DEFAULT_POLL
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
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_ASYNCGEN_POLL
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
        // Keep in sync with wasm table layout in runtime/molt-backend/src/wasm.rs.
        WASM_TABLE_IDX_PROMISE_POLL
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
        WASM_TABLE_IDX_CONTEXTLIB_ASYNCGEN_ENTER_POLL
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
        WASM_TABLE_IDX_CONTEXTLIB_ASYNCGEN_EXIT_POLL
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
        WASM_TABLE_IDX_CONTEXTLIB_ASYNC_EXITSTACK_EXIT_POLL
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
        WASM_TABLE_IDX_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT_POLL
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
        WASM_TABLE_IDX_IO_WAIT
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
        WASM_TABLE_IDX_WS_WAIT
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
        WASM_TABLE_IDX_THREAD_POLL
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
        WASM_TABLE_IDX_PROCESS_POLL
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
        WASM_TABLE_IDX_ASYNCIO_WAIT_FOR_POLL
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
        WASM_TABLE_IDX_ASYNCIO_WAIT_POLL
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
        WASM_TABLE_IDX_ASYNCIO_GATHER_POLL
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
        WASM_TABLE_IDX_ASYNCIO_TIMER_HANDLE_POLL
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
        WASM_TABLE_IDX_ASYNCIO_FD_WATCHER_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SERVER_ACCEPT_LOOP_POLL
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
        WASM_TABLE_IDX_ASYNCIO_READY_RUNNER_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCKET_READER_READ_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCKET_READER_READLINE_POLL
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
        WASM_TABLE_IDX_ASYNCIO_STREAM_READER_READ_POLL
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
        WASM_TABLE_IDX_ASYNCIO_STREAM_READER_READLINE_POLL
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
        WASM_TABLE_IDX_ASYNCIO_STREAM_SEND_ALL_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_RECV_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_CONNECT_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_ACCEPT_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_RECV_INTO_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_SENDALL_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_RECVFROM_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_RECVFROM_INTO_POLL
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
        WASM_TABLE_IDX_ASYNCIO_SOCK_SENDTO_POLL
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_sendto_poll)
    }
}

pub(crate) unsafe fn call_poll_fn(_py: &PyToken<'_>, poll_fn_addr: u64, task_ptr: *mut u8) -> i64 {
    let addr = task_ptr.expose_provenance() as u64;
    #[cfg(target_arch = "wasm32")]
    {
        if std::env::var("MOLT_WASM_POLL_DEBUG").as_deref() == Ok("1") {
            eprintln!("molt wasm poll: fn=0x{poll_fn_addr:x}");
        }
        if poll_fn_addr < WASM_TABLE_BASE {
            return raise_exception::<i64>(_py, "RuntimeError", "invalid wasm poll function");
        }
        return crate::molt_call_indirect1(poll_fn_addr, addr);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let poll_fn: extern "C" fn(u64) -> i64 = std::mem::transmute(poll_fn_addr as usize);
        poll_fn(addr)
    }
}

pub(crate) unsafe fn poll_future_with_task_stack(
    _py: &PyToken<'_>,
    task_ptr: *mut u8,
    poll_fn_addr: u64,
) -> i64 {
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
    let caller_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
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
    let task_active = ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
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
