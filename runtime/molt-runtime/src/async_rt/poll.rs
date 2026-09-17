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

#[cfg(target_arch = "wasm32")]
use crate::builtins::functions::{
    WASM_POLL_SLOT_MAX_OFFSET, wasm_poll_table_slot_from_symbol_name,
};

#[cfg(target_arch = "wasm32")]
#[inline]
fn wasm_poll_slot(symbol_name: &str) -> u64 {
    let offset = wasm_poll_table_slot_from_symbol_name(symbol_name)
        .unwrap_or_else(|| panic!("missing generated wasm poll slot for {symbol_name}"));
    crate::wasm_table_base().saturating_add(offset)
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn normalize_wasm_poll_fn_addr(poll_fn_addr: u64) -> u64 {
    let table_base = crate::wasm_table_base();
    if poll_fn_addr >= table_base {
        return poll_fn_addr;
    }
    let legacy_base = crate::wasm_table_base_fallback();
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
        wasm_poll_slot("molt_async_sleep_poll")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_async_sleep_poll)
    }
}

#[inline]
pub(crate) fn anext_default_poll_fn_addr() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_poll_slot("molt_anext_default_poll")
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
        wasm_poll_slot("molt_asyncgen_poll")
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
        wasm_poll_slot("molt_promise_poll")
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
        wasm_poll_slot("molt_contextlib_asyncgen_enter_poll")
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
        wasm_poll_slot("molt_contextlib_asyncgen_exit_poll")
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
        wasm_poll_slot("molt_contextlib_async_exitstack_exit_poll")
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
        wasm_poll_slot("molt_contextlib_async_exitstack_enter_context_poll")
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
        wasm_poll_slot("molt_io_wait")
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
        wasm_poll_slot("molt_ws_wait")
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
        wasm_poll_slot("molt_thread_poll")
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
        wasm_poll_slot("molt_process_poll")
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
        wasm_poll_slot("molt_asyncio_wait_for_poll")
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
        wasm_poll_slot("molt_asyncio_wait_poll")
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
        wasm_poll_slot("molt_asyncio_gather_poll")
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
        wasm_poll_slot("molt_asyncio_timer_handle_poll")
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
        wasm_poll_slot("molt_asyncio_fd_watcher_poll")
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
        wasm_poll_slot("molt_asyncio_server_accept_loop_poll")
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
        wasm_poll_slot("molt_asyncio_ready_runner_poll")
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
        wasm_poll_slot("molt_asyncio_socket_reader_read_poll")
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
        wasm_poll_slot("molt_asyncio_socket_reader_readline_poll")
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
        wasm_poll_slot("molt_asyncio_stream_reader_read_poll")
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
        wasm_poll_slot("molt_asyncio_stream_reader_readline_poll")
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
        wasm_poll_slot("molt_asyncio_stream_send_all_poll")
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
        wasm_poll_slot("molt_asyncio_sock_recv_poll")
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
        wasm_poll_slot("molt_asyncio_sock_connect_poll")
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
        wasm_poll_slot("molt_asyncio_sock_accept_poll")
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
        wasm_poll_slot("molt_asyncio_sock_recv_into_poll")
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
        wasm_poll_slot("molt_asyncio_sock_sendall_poll")
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
        wasm_poll_slot("molt_asyncio_sock_recvfrom_poll")
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
        wasm_poll_slot("molt_asyncio_sock_recvfrom_into_poll")
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
        wasm_poll_slot("molt_asyncio_sock_sendto_poll")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fn_addr!(crate::molt_asyncio_sock_sendto_poll)
    }
}

pub(crate) unsafe fn call_poll_fn(_py: &PyToken<'_>, poll_fn_addr: u64, task_ptr: *mut u8) -> i64 {
    unsafe {
        // Resumption transports creation-time code and namespace into the compiler's
        // real frame entry; it must not introduce a second visible Python frame.
        let [globals_bits, builtins_bits, code_bits] =
            crate::object::aux_header::object_frame_context_bits(task_ptr);
        let Some(_frame_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_suspended_namespace(
                _py,
                code_bits,
                globals_bits,
                builtins_bits,
            )
        else {
            return MoltObject::none().bits() as i64;
        };
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
            let res = crate::molt_call_indirect1(normalized_poll_fn_addr, addr);
            if matches!(
                std::env::var("MOLT_TRACE_POLL_RETURN").ok().as_deref(),
                Some("1")
            ) {
                let known_kind = if poll_fn_addr == async_sleep_poll_fn_addr() {
                    "async_sleep"
                } else if poll_fn_addr == promise_poll_fn_addr() {
                    "promise"
                } else if poll_fn_addr == asyncio_wait_for_poll_fn_addr() {
                    "asyncio_wait_for"
                } else if poll_fn_addr == asyncio_wait_poll_fn_addr() {
                    "asyncio_wait"
                } else if poll_fn_addr == asyncio_gather_poll_fn_addr() {
                    "asyncio_gather"
                } else if poll_fn_addr == asyncio_timer_handle_poll_fn_addr() {
                    "asyncio_timer_handle"
                } else if poll_fn_addr == asyncio_fd_watcher_poll_fn_addr() {
                    "asyncio_fd_watcher"
                } else if poll_fn_addr == asyncio_server_accept_loop_poll_fn_addr() {
                    "asyncio_server_accept_loop"
                } else if poll_fn_addr == asyncio_ready_runner_poll_fn_addr() {
                    "asyncio_ready_runner"
                } else if poll_fn_addr == asyncio_socket_reader_read_poll_fn_addr() {
                    "asyncio_socket_reader_read"
                } else if poll_fn_addr == asyncio_socket_reader_readline_poll_fn_addr() {
                    "asyncio_socket_reader_readline"
                } else if poll_fn_addr == asyncio_stream_reader_read_poll_fn_addr() {
                    "asyncio_stream_reader_read"
                } else if poll_fn_addr == asyncio_stream_reader_readline_poll_fn_addr() {
                    "asyncio_stream_reader_readline"
                } else if poll_fn_addr == asyncio_stream_send_all_poll_fn_addr() {
                    "asyncio_stream_send_all"
                } else if poll_fn_addr == asyncio_sock_recv_poll_fn_addr() {
                    "asyncio_sock_recv"
                } else if poll_fn_addr == asyncio_sock_connect_poll_fn_addr() {
                    "asyncio_sock_connect"
                } else if poll_fn_addr == asyncio_sock_accept_poll_fn_addr() {
                    "asyncio_sock_accept"
                } else if poll_fn_addr == asyncio_sock_recv_into_poll_fn_addr() {
                    "asyncio_sock_recv_into"
                } else if poll_fn_addr == asyncio_sock_sendall_poll_fn_addr() {
                    "asyncio_sock_sendall"
                } else if poll_fn_addr == asyncio_sock_recvfrom_poll_fn_addr() {
                    "asyncio_sock_recvfrom"
                } else if poll_fn_addr == asyncio_sock_recvfrom_into_poll_fn_addr() {
                    "asyncio_sock_recvfrom_into"
                } else if poll_fn_addr == asyncio_sock_sendto_poll_fn_addr() {
                    "asyncio_sock_sendto"
                } else if poll_fn_addr == io_wait_poll_fn_addr() {
                    "io_wait"
                } else if poll_fn_addr == thread_poll_fn_addr() {
                    "thread"
                } else if poll_fn_addr == process_poll_fn_addr() {
                    "process"
                } else if poll_fn_addr == asyncgen_poll_fn_addr() {
                    "asyncgen"
                } else if poll_fn_addr == anext_default_poll_fn_addr() {
                    "anext_default"
                } else if poll_fn_addr == ws_wait_poll_fn_addr() {
                    "ws_wait"
                } else {
                    "other"
                };
                let mut code_name = "<none>".to_string();
                let mut code_file = "<none>".to_string();
                if code_bits != 0
                    && let Some(code_ptr) = crate::maybe_ptr_from_bits(code_bits)
                {
                    let name_bits = crate::code_name_bits(code_ptr);
                    code_name = crate::string_obj_to_owned(crate::obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let file_bits = crate::code_filename_bits(code_ptr);
                    code_file = crate::string_obj_to_owned(crate::obj_from_bits(file_bits))
                        .unwrap_or_else(|| "<unknown>".to_string());
                }
                let kind = if crate::exception_pending(_py) {
                    let exc_bits = crate::molt_exception_last();
                    if let Some(exc_ptr) = crate::maybe_ptr_from_bits(exc_bits) {
                        let kind_bits = crate::exception_kind_bits(exc_ptr);
                        crate::string_obj_to_owned(crate::obj_from_bits(kind_bits))
                            .unwrap_or_else(|| "<exc>".to_string())
                    } else {
                        "<none>".to_string()
                    }
                } else {
                    "<none>".to_string()
                };
                eprintln!(
                    "molt poll return fn=0x{:x} normalized=0x{:x} kind={} code={} file={} res=0x{:x} pending={}",
                    poll_fn_addr,
                    normalized_poll_fn_addr,
                    known_kind,
                    code_name,
                    code_file,
                    res as u64,
                    kind
                );
            }
            res
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // SAFETY: `poll_fn_addr` is a valid extern "C" fn pointer stored in the task object
            // by the async runtime. The caller ensures it points to a 1-arg poll function. UB if null.
            let poll_target = if let Some(target) =
                crate::builtins::functions::runtime_callable_target_ptr(poll_fn_addr)
            {
                target
            } else {
                let Some(target) = crate::provenance::abi::function_ptr(poll_fn_addr) else {
                    return crate::raise_exception::<i64>(
                        _py,
                        "RuntimeError",
                        "async poll address exceeds the active address space",
                    );
                };
                target
            };
            let poll_fn: extern "C" fn(u64) -> i64 = std::mem::transmute(poll_target);
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
        let task_scope = crate::CurrentTaskScope::enter(_py, task_ptr);
        let prev_task = task_scope.previous();
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
        if res != crate::pending_bits_i64() && !crate::exception_pending(_py) {
            crate::task_last_exception_drop(_py, task_ptr);
        }
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
        exception_context_fallback_pop(_py);
        if debug_task && prev_task.is_null() {
            eprintln!(
                "molt task trace: restoring prev_task=null after task=0x{:x}",
                task_ptr as usize
            );
        }
        drop(task_scope);
        res
    }
}

#[cfg(test)]
mod tests {
    use super::async_sleep_poll_fn_addr;

    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn namespace_probe_poll(task_address: u64) -> i64 {
        crate::with_gil_entry_nopanic!(_py, {
            crate::molt_trace_enter_slot(0);
            let globals = crate::builtins::frames::frame_stack_active_globals_bits();
            let task = std::ptr::with_exposed_provenance_mut::<u8>(task_address as usize);
            assert_eq!(
                crate::builtins::frames::frame_stack_active_code_bits(),
                crate::object::aux_header::object_frame_code_bits(task),
                "resume must use the retained code object despite symbol rebinding",
            );
            assert_eq!(
                crate::builtins::frames::frame_stack_active_builtins_bits(),
                crate::object::aux_header::object_frame_builtins_bits(task),
                "resume must use captured builtins, not the mutated globals entry",
            );
            crate::molt_trace_exit();
            globals as i64
        })
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn suspended_tasks_keep_creation_namespace_across_callers_and_frame_views() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use crate::builtins::frames::FrameInvocationGuard;
            use crate::object::aux_header::{
                object_frame_builtins_bits, object_frame_globals_bits,
            };
            use crate::{MoltObject, alloc_dict_with_pairs, dec_ref_bits};

            let lexical = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let rebound = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let caller = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let builtins = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let replacement = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let builtins_key = crate::attr_name_bits_from_bytes(py, b"__builtins__").unwrap();
            let builtins_baseline = unsafe {
                (*crate::header_from_obj_ptr(crate::obj_from_bits(builtins).as_ptr().unwrap()))
                    .ref_count_snapshot()
            };
            let name =
                MoltObject::from_ptr(crate::alloc_string(py, b"<suspended-namespace>")).bits();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let code = crate::object::builders::alloc_code_obj(
                py,
                name,
                name,
                1,
                MoltObject::none().bits(),
                empty,
                empty,
                0,
                0,
                0,
            );
            let code_bits = MoltObject::from_ptr(code).bits();
            let replacement_code = crate::object::builders::alloc_code_obj(
                py,
                name,
                name,
                1,
                MoltObject::none().bits(),
                empty,
                empty,
                0,
                0,
                0,
            );
            let replacement_code_bits = MoltObject::from_ptr(replacement_code).bits();
            dec_ref_bits(py, name);
            dec_ref_bits(py, empty);
            crate::molt_code_slots_init(1);
            crate::molt_code_slot_set(0, code_bits, lexical);
            let poll = namespace_probe_poll as *const () as usize as u64;
            for bits in [code_bits, replacement_code_bits] {
                let function = crate::builtins::functions::alloc_runtime_function_obj(py, poll, 0);
                assert!(!function.is_null());
                assert!(unsafe {
                    crate::object::layout::function_set_code_bits(py, function, bits)
                });
                dec_ref_bits(py, MoltObject::from_ptr(function).bits());
            }
            let baseline = unsafe {
                (*crate::header_from_obj_ptr(crate::obj_from_bits(rebound).as_ptr().unwrap()))
                    .ref_count_snapshot()
            };

            for kind in [
                crate::TASK_KIND_GENERATOR,
                crate::TASK_KIND_COROUTINE,
                crate::TASK_KIND_FUTURE,
            ] {
                crate::molt_code_slot_set(0, code_bits, lexical);
                unsafe {
                    crate::dict_set_in_place(
                        py,
                        crate::obj_from_bits(rebound).as_ptr().unwrap(),
                        builtins_key,
                        builtins,
                    );
                }
                let creation = FrameInvocationGuard::for_namespace(py, code_bits, rebound).unwrap();
                let task = crate::molt_task_new(poll, crate::GEN_CONTROL_SIZE as u64, kind);
                drop(creation);
                let ptr = crate::obj_from_bits(task).as_ptr().unwrap();
                assert_eq!(object_frame_globals_bits(ptr), rebound);
                assert_eq!(object_frame_builtins_bits(ptr), builtins);
                assert_eq!(
                    crate::object::aux_header::object_frame_code_bits(ptr),
                    code_bits
                );
                crate::molt_code_slot_set(0, replacement_code_bits, lexical);
                unsafe {
                    crate::dict_set_in_place(
                        py,
                        crate::obj_from_bits(rebound).as_ptr().unwrap(),
                        builtins_key,
                        replacement,
                    );
                }
                let mut visited = Vec::new();
                unsafe {
                    crate::object::heap_lifecycle::visit_owned_values(py, ptr, &mut |bits| {
                        visited.push(bits)
                    });
                }
                assert!(
                    visited.contains(&rebound),
                    "captured globals must be visible to GC"
                );
                assert!(
                    visited.contains(&builtins),
                    "captured builtins must be visible to GC"
                );
                assert!(
                    visited.contains(&code_bits),
                    "retained code must be visible to GC"
                );

                let calling = FrameInvocationGuard::for_namespace(py, code_bits, caller).unwrap();
                crate::molt_trace_enter_slot(0);
                for _ in 0..2 {
                    assert_eq!(
                        unsafe { super::call_poll_fn(py, poll, ptr) } as u64,
                        rebound
                    );
                    assert_eq!(
                        crate::builtins::frames::frame_stack_active_globals_bits(),
                        caller
                    );
                }
                let mut views = Vec::new();
                if kind == crate::TASK_KIND_GENERATOR {
                    views.push((task, b"gi_frame".as_slice()));
                    views.push((crate::molt_asyncgen_new(task), b"ag_frame".as_slice()));
                } else if kind == crate::TASK_KIND_COROUTINE {
                    views.push((task, b"cr_frame".as_slice()));
                }
                for (owner, field) in views {
                    let code_field = match field {
                        b"gi_frame" => b"gi_code".as_slice(),
                        b"ag_frame" => b"ag_code".as_slice(),
                        b"cr_frame" => b"cr_code".as_slice(),
                        _ => unreachable!(),
                    };
                    let code_attr = crate::attr_name_bits_from_bytes(py, code_field).unwrap();
                    let viewed_code = unsafe {
                        crate::builtins::attributes::attr_lookup_ptr(
                            py,
                            crate::obj_from_bits(owner).as_ptr().unwrap(),
                            code_attr,
                        )
                    }
                    .unwrap();
                    assert_eq!(viewed_code, code_bits);
                    dec_ref_bits(py, viewed_code);
                    dec_ref_bits(py, code_attr);
                    let attr = crate::attr_name_bits_from_bytes(py, field).unwrap();
                    let frame = unsafe {
                        crate::builtins::attributes::attr_lookup_ptr(
                            py,
                            crate::obj_from_bits(owner).as_ptr().unwrap(),
                            attr,
                        )
                    }
                    .unwrap();
                    assert_eq!(
                        unsafe {
                            crate::object_class_bits(crate::obj_from_bits(frame).as_ptr().unwrap())
                        },
                        crate::builtin_classes(py).frame,
                        "every suspended view must use the canonical Python frame class"
                    );
                    for (name, expected) in [
                        (b"f_globals".as_slice(), rebound),
                        (b"f_builtins".as_slice(), builtins),
                        (b"f_code".as_slice(), code_bits),
                        (b"f_back".as_slice(), MoltObject::none().bits()),
                        (b"f_lineno".as_slice(), MoltObject::from_int(1).bits()),
                    ] {
                        let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
                        let value = unsafe {
                            crate::builtins::attributes::attr_lookup_ptr(
                                py,
                                crate::obj_from_bits(frame).as_ptr().unwrap(),
                                name,
                            )
                        }
                        .unwrap();
                        assert_eq!(value, expected);
                        dec_ref_bits(py, value);
                        dec_ref_bits(py, name);
                    }
                    dec_ref_bits(py, frame);
                    dec_ref_bits(py, attr);
                    if owner != task {
                        dec_ref_bits(py, owner);
                    }
                }
                crate::molt_trace_exit();
                drop(calling);
                dec_ref_bits(py, task);
                assert_eq!(
                    unsafe {
                        (*crate::header_from_obj_ptr(
                            crate::obj_from_bits(rebound).as_ptr().unwrap(),
                        ))
                        .ref_count_snapshot()
                    },
                    baseline,
                    "task and frame namespace owners must retire exactly once"
                );
                assert_eq!(
                    unsafe {
                        (*crate::header_from_obj_ptr(
                            crate::obj_from_bits(builtins).as_ptr().unwrap(),
                        ))
                        .ref_count_snapshot()
                    },
                    builtins_baseline,
                    "captured builtins must retire exactly once"
                );
            }
            let calling = FrameInvocationGuard::for_namespace(py, code_bits, caller).unwrap();
            crate::molt_trace_enter_slot(0);
            let native_task = crate::molt_task_new(
                poll,
                crate::GEN_CONTROL_SIZE as u64,
                crate::TASK_KIND_FUTURE,
            );
            let native_ptr = crate::obj_from_bits(native_task).as_ptr().unwrap();
            assert_eq!(object_frame_globals_bits(native_ptr), 0);
            assert_eq!(object_frame_builtins_bits(native_ptr), 0);
            assert_eq!(
                crate::object::aux_header::object_frame_code_bits(native_ptr),
                0
            );
            crate::molt_trace_exit();
            drop(calling);
            dec_ref_bits(py, native_task);
            dec_ref_bits(py, code_bits);
            dec_ref_bits(py, replacement_code_bits);
            dec_ref_bits(py, lexical);
            dec_ref_bits(py, rebound);
            dec_ref_bits(py, caller);
            dec_ref_bits(py, builtins);
            dec_ref_bits(py, replacement);
            dec_ref_bits(py, builtins_key);
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_async_sleep_poll_id_is_stable_not_raw_code_address() {
        let poll_id = async_sleep_poll_fn_addr();
        let raw_ptr = crate::molt_async_sleep_poll as *const () as usize as u64;

        assert_ne!(poll_id, raw_ptr);
        assert_eq!(
            crate::builtins::functions::runtime_callable_target_ptr(poll_id),
            Some(crate::molt_async_sleep_poll as *const ())
        );
        assert_eq!(
            crate::builtins::functions::canonicalize_runtime_callable_key(raw_ptr),
            raw_ptr
        );
    }
}
