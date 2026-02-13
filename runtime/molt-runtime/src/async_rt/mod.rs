pub(crate) mod cancellation;
pub(crate) mod channels;
pub(crate) mod generators;
pub(crate) mod io_poller;
pub(crate) mod poll;
pub(crate) mod process;
pub(crate) mod scheduler;
pub(crate) mod sockets;
pub(crate) mod task;
pub(crate) mod threads;

use std::sync::atomic::{AtomicUsize, Ordering};

static SPAWN_RETAIN_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn spawned_task_inc() {
    SPAWN_RETAIN_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn spawned_task_dec() {
    SPAWN_RETAIN_COUNT.fetch_sub(1, Ordering::Relaxed);
}

pub(crate) fn spawned_task_count() -> usize {
    SPAWN_RETAIN_COUNT.load(Ordering::Relaxed)
}

#[allow(unused_imports)]
pub(crate) use cancellation::{
    CURRENT_TOKEN, CancelTokenEntry, NEXT_CANCEL_TOKEN_ID, cancel_tokens, clear_task_token,
    current_token_id, default_cancel_tokens, ensure_task_token, raise_cancelled_with_message,
    register_task_token, release_token, retain_token, set_current_token, task_cancel_message_clear,
    task_cancel_message_set, task_cancel_pending, task_has_token, task_set_cancel_pending,
    task_take_cancel_pending, token_id_from_bits, token_is_cancelled,
    wake_tasks_for_cancelled_tokens,
};

#[allow(unused_imports)]
pub(crate) use scheduler::{
    AsyncHangProbe, CURRENT_TASK, MoltScheduler, MoltTask, SleepQueue, async_trace_enabled,
    asyncgen_registry, await_waiter_clear, await_waiter_register, await_waiters,
    await_waiters_take, block_on_wait_spec, current_task_key, current_task_ptr, fn_ptr_code_get,
    fn_ptr_code_set, instant_from_monotonic_secs, molt_asyncio_child_watcher_add,
    molt_asyncio_child_watcher_clear, molt_asyncio_child_watcher_pop,
    molt_asyncio_child_watcher_remove, molt_asyncio_event_loop_get,
    molt_asyncio_event_loop_get_current, molt_asyncio_event_loop_policy_get,
    molt_asyncio_event_loop_policy_set, molt_asyncio_event_loop_set,
    molt_asyncio_event_waiters_cleanup_token, molt_asyncio_event_waiters_register,
    molt_asyncio_event_waiters_unregister, molt_asyncio_require_child_watcher_support,
    molt_asyncio_require_ssl_transport_support, molt_asyncio_require_unix_socket_support,
    molt_asyncio_running_loop_get, molt_asyncio_running_loop_set,
    molt_asyncio_ssl_transport_orchestrate, molt_asyncio_task_registry_contains,
    molt_asyncio_task_registry_current, molt_asyncio_task_registry_current_for_loop,
    molt_asyncio_task_registry_get, molt_asyncio_task_registry_live,
    molt_asyncio_task_registry_live_set, molt_asyncio_task_registry_move,
    molt_asyncio_task_registry_pop, molt_asyncio_task_registry_set,
    molt_asyncio_task_registry_values, molt_block_on, molt_spawn, monotonic_now_nanos,
    monotonic_now_secs, record_async_poll, task_exception_depths, task_exception_handler_stacks,
    task_exception_stacks, task_last_exceptions, task_mark_done, task_waiting_on,
    task_waiting_on_future, wake_task_ptr,
};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use scheduler::{is_block_on_task, sleep_worker, thread_task_drop, thread_task_state};
pub(crate) use scheduler::{process_task_drop, process_task_state};

#[allow(unused_imports)]
pub(crate) use generators::*;

pub(crate) use poll::{
    anext_default_poll_fn_addr, async_sleep_poll_fn_addr, asyncgen_poll_fn_addr,
    asyncio_fd_watcher_poll_fn_addr, asyncio_gather_poll_fn_addr,
    asyncio_ready_runner_poll_fn_addr, asyncio_server_accept_loop_poll_fn_addr,
    asyncio_sock_accept_poll_fn_addr, asyncio_sock_connect_poll_fn_addr,
    asyncio_sock_recv_into_poll_fn_addr, asyncio_sock_recv_poll_fn_addr,
    asyncio_sock_recvfrom_into_poll_fn_addr, asyncio_sock_recvfrom_poll_fn_addr,
    asyncio_sock_sendall_poll_fn_addr, asyncio_sock_sendto_poll_fn_addr,
    asyncio_socket_reader_read_poll_fn_addr, asyncio_socket_reader_readline_poll_fn_addr,
    asyncio_stream_reader_read_poll_fn_addr, asyncio_stream_reader_readline_poll_fn_addr,
    asyncio_stream_send_all_poll_fn_addr, asyncio_timer_handle_poll_fn_addr,
    asyncio_wait_for_poll_fn_addr, asyncio_wait_poll_fn_addr, call_poll_fn,
    contextlib_async_exitstack_enter_context_poll_fn_addr,
    contextlib_async_exitstack_exit_poll_fn_addr, contextlib_asyncgen_enter_poll_fn_addr,
    contextlib_asyncgen_exit_poll_fn_addr, io_wait_poll_fn_addr, poll_future_with_task_stack,
    process_poll_fn_addr, promise_poll_fn_addr, thread_poll_fn_addr,
};

pub(crate) use task::resolve_task_ptr;
