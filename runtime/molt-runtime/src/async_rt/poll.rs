use molt_obj_model::MoltObject;

use crate::{
    exception_context_align_depth, exception_context_fallback_pop, exception_context_fallback_push,
    exception_stack_depth, exception_stack_set_depth, set_task_raise_active,
    task_exception_depth_store, task_exception_depth_take, task_exception_handler_stack_store,
    task_exception_handler_stack_take, task_exception_stack_store, task_exception_stack_take,
    task_raise_active, PyToken, ACTIVE_EXCEPTION_STACK, EXCEPTION_STACK,
};

#[cfg(target_arch = "wasm32")]
use crate::{
    raise_exception, WASM_TABLE_BASE, WASM_TABLE_IDX_ANEXT_DEFAULT_POLL,
    WASM_TABLE_IDX_ASYNCGEN_POLL, WASM_TABLE_IDX_ASYNC_SLEEP, WASM_TABLE_IDX_IO_WAIT,
    WASM_TABLE_IDX_PROCESS_POLL, WASM_TABLE_IDX_PROMISE_POLL, WASM_TABLE_IDX_THREAD_POLL,
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
    let prev_task = CURRENT_TASK.with(|cell| {
        let prev = cell.get();
        cell.set(task_ptr);
        prev
    });
    let caller_depth = exception_stack_depth();
    let caller_handlers = EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_active =
        ACTIVE_EXCEPTION_STACK.with(|stack| std::mem::take(&mut *stack.borrow_mut()));
    let caller_context = caller_active
        .last()
        .copied()
        .unwrap_or(MoltObject::none().bits());
    exception_context_fallback_push(caller_context);
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
    CURRENT_TASK.with(|cell| cell.set(prev_task));
    res
}
