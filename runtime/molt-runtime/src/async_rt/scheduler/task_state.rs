use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use crate::ThreadTaskState;
use crate::object::heap_lifecycle::DetachedEdgeSink;
use crate::object::{dec_ref_ptr, inc_ref_ptr};
use crate::{
    HEADER_FLAG_SPAWN_RETAIN, MoltObject, ProcessTaskState, PtrSlot, PyToken, header_from_obj_ptr,
    io_wait_poll_fn_addr, process_poll_fn_addr, promise_poll_fn_addr, runtime_state,
    thread_poll_fn_addr,
};

use super::{async_trace_enabled, wake_task_ptr};

pub(crate) fn task_exception_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<u64>>> {
    &runtime_state(_py).task_exception_stacks
}

pub(crate) fn task_exception_handler_stacks(
    _py: &PyToken<'_>,
) -> &'static Mutex<HashMap<PtrSlot, Vec<usize>>> {
    &runtime_state(_py).task_exception_handler_stacks
}

pub(crate) fn await_waiters(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, Vec<PtrSlot>>> {
    &runtime_state(_py).await_waiters
}

pub(crate) fn task_waiting_on(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, PtrSlot>> {
    &runtime_state(_py).task_waiting_on
}

#[derive(Default)]
pub(crate) struct AwaitWaiterIndex {
    positions: HashMap<PtrSlot, usize>,
}

fn await_waiter_index_map(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, AwaitWaiterIndex>> {
    &runtime_state(_py).await_waiter_index
}

fn rebuild_unique_index<T: Copy + Eq + Hash>(values: &[T]) -> HashMap<T, usize> {
    let mut index = HashMap::with_capacity(values.len());
    for (idx, value) in values.iter().copied().enumerate() {
        index.insert(value, idx);
    }
    index
}

fn indexed_unique_vec_insert<T: Copy + Eq + Hash>(
    values: &mut Vec<T>,
    index: &mut HashMap<T, usize>,
    value: T,
) -> bool {
    if let Some(&idx) = index.get(&value)
        && idx < values.len()
        && values[idx] == value
    {
        return false;
    }
    if index.len() != values.len() {
        *index = rebuild_unique_index(values);
        if index.contains_key(&value) {
            return false;
        }
    }
    let next = values.len();
    values.push(value);
    index.insert(value, next);
    true
}

fn indexed_unique_vec_swap_remove<T: Copy + Eq + Hash>(
    values: &mut Vec<T>,
    index: &mut HashMap<T, usize>,
    value: T,
) -> bool {
    if index.len() != values.len() {
        *index = rebuild_unique_index(values);
    }
    let Some(idx) = index.remove(&value) else {
        return false;
    };
    let Some(last) = values.pop() else {
        return false;
    };
    if idx < values.len() {
        values[idx] = last;
        index.insert(last, idx);
    }
    true
}

pub(crate) fn task_exception_depths(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, usize>> {
    &runtime_state(_py).task_exception_depths
}

pub(crate) fn task_last_exceptions(_py: &PyToken<'_>) -> &'static Mutex<HashMap<PtrSlot, PtrSlot>> {
    &runtime_state(_py).task_last_exceptions
}

/// Side-effect-free projection of every Python edge owned on behalf of a task
/// outside its inline closure payload.
pub(crate) fn task_visit_owned_edges(
    _py: &PyToken<'_>,
    task_ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    let slot = PtrSlot(task_ptr);
    if unsafe { (*header_from_obj_ptr(task_ptr)).has_flag(HEADER_FLAG_SPAWN_RETAIN) } {
        visit(MoltObject::from_ptr(task_ptr).bits());
    }
    if let Some(stack) = task_exception_stacks(_py).lock().unwrap().get(&slot) {
        for &bits in stack {
            visit(bits);
        }
    }
    if let Some(exception) = task_last_exceptions(_py).lock().unwrap().get(&slot) {
        visit(MoltObject::from_ptr(exception.0).bits());
    }
    if let Some(&bits) = runtime_state(_py).task_results.lock().unwrap().get(&slot) {
        visit(bits);
    }
    if let Some(&bits) = crate::task_cancel_messages(_py).lock().unwrap().get(&slot) {
        visit(bits);
    }
    if let Some(awaited) = task_waiting_on(_py).lock().unwrap().get(&slot) {
        // The await graph retains both endpoints.
        visit(MoltObject::from_ptr(task_ptr).bits());
        visit(MoltObject::from_ptr(awaited.0).bits());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(state) = runtime_state(_py).thread_tasks.lock().unwrap().get(&slot) {
        if let Some(bits) = *state.result.lock().unwrap() {
            visit(bits);
        }
        if let Some(bits) = *state.exception.lock().unwrap() {
            visit(bits);
        }
    }
}

fn await_waiter_detach_owned_edge(
    _py: &PyToken<'_>,
    waiter_ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    let waiter = PtrSlot(waiter_ptr);
    let awaited = task_waiting_on(_py).lock().unwrap().remove(&waiter);
    let Some(awaited) = awaited else {
        return;
    };
    let mut awaiters = await_waiters(_py).lock().unwrap();
    let mut indices = await_waiter_index_map(_py).lock().unwrap();
    if let Some(waiters) = awaiters.get_mut(&awaited) {
        let index = indices.entry(awaited).or_default();
        if index.positions.len() != waiters.len() {
            index.positions = rebuild_unique_index(waiters.as_slice());
        }
        indexed_unique_vec_swap_remove(waiters, &mut index.positions, waiter);
        if waiters.is_empty() {
            awaiters.remove(&awaited);
            indices.remove(&awaited);
        }
    } else {
        indices.remove(&awaited);
    }
    drop(indices);
    drop(awaiters);
    sink.detach(MoltObject::from_ptr(waiter_ptr).bits());
    sink.detach(MoltObject::from_ptr(awaited.0).bits());
}

/// Detach every scheduler-owned Python edge for one task while publishing all
/// corresponding side tables empty. No Python destructor runs here.
pub(crate) fn task_detach_owned_edges(
    _py: &PyToken<'_>,
    task_ptr: *mut u8,
    sink: &mut DetachedEdgeSink,
) {
    let slot = PtrSlot(task_ptr);
    if let Some(bits) = runtime_state(_py)
        .task_results
        .lock()
        .unwrap()
        .remove(&slot)
    {
        sink.detach_if_heap(bits);
    }
    crate::task_cancellation_detach(_py, task_ptr, sink);
    crate::builtins::exceptions::task_exception_detach_owned_edges(_py, task_ptr, sink);
    await_waiter_detach_owned_edge(_py, task_ptr, sink);

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(state) = runtime_state(_py)
        .thread_tasks
        .lock()
        .unwrap()
        .remove(&slot)
    {
        state.cancelled.store(true, AtomicOrdering::Release);
        if let Some(bits) = state.result.lock().unwrap().take() {
            sink.detach_if_heap(bits);
        }
        if let Some(bits) = state.exception.lock().unwrap().take() {
            sink.detach_if_heap(bits);
        }
        state.condvar.notify_all();
    }

    if let Some(state) = runtime_state(_py)
        .process_tasks
        .lock()
        .unwrap()
        .remove(&slot)
    {
        state.cancel_wait();
    }
}

pub(crate) fn await_waiter_register(_py: &PyToken<'_>, waiter_ptr: *mut u8, awaited_ptr: *mut u8) {
    if waiter_ptr.is_null() || awaited_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: await_register waiter=0x{:x} awaited=0x{:x}",
            waiter_ptr as usize, awaited_ptr as usize
        );
    }
    if matches!(
        std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
        Some("1")
    ) {
        let poll_fn = crate::object::object_poll_fn(awaited_ptr);
        if poll_fn == promise_poll_fn_addr() {
            eprintln!(
                "molt async trace: await_register_promise waiter=0x{:x} awaited=0x{:x}",
                waiter_ptr as usize, awaited_ptr as usize
            );
        }
    }
    let waiter_key = PtrSlot(waiter_ptr);
    let awaited_key = PtrSlot(awaited_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    let mut awaiter_index_map = await_waiter_index_map(_py).lock().unwrap();
    let prev = waiting_map.insert(waiter_key, awaited_key);
    // Keep raw pointers alive while they live in the await graph.
    unsafe {
        if prev.is_none() {
            inc_ref_ptr(_py, waiter_ptr);
        }
        if prev != Some(awaited_key) {
            inc_ref_ptr(_py, awaited_ptr);
        }
    }
    if let Some(prev_key) = prev
        && prev_key != awaited_key
    {
        if let Some(waiters) = awaiters_map.get_mut(&prev_key) {
            let waiter_index = awaiter_index_map.entry(prev_key).or_default();
            if waiter_index.positions.len() != waiters.len() {
                waiter_index.positions = rebuild_unique_index(waiters.as_slice());
            }
            indexed_unique_vec_swap_remove(waiters, &mut waiter_index.positions, waiter_key);
            if waiters.is_empty() {
                awaiters_map.remove(&prev_key);
                awaiter_index_map.remove(&prev_key);
            }
        } else {
            awaiter_index_map.remove(&prev_key);
        }
        unsafe {
            dec_ref_ptr(_py, prev_key.0);
        }
    }
    let waiters = awaiters_map.entry(awaited_key).or_default();
    let waiter_index = awaiter_index_map.entry(awaited_key).or_default();
    if waiter_index.positions.len() != waiters.len() {
        waiter_index.positions = rebuild_unique_index(waiters.as_slice());
    }
    indexed_unique_vec_insert(waiters, &mut waiter_index.positions, waiter_key);
}

pub(crate) fn await_waiter_clear(_py: &PyToken<'_>, waiter_ptr: *mut u8) {
    if waiter_ptr.is_null() {
        return;
    }
    if async_trace_enabled() {
        eprintln!(
            "molt async trace: await_clear waiter=0x{:x}",
            waiter_ptr as usize
        );
    }
    let waiter_key = PtrSlot(waiter_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let awaited_key = waiting_map.remove(&waiter_key);
    if awaited_key.is_none() {
        return;
    }
    let awaited_key = awaited_key.unwrap();
    unsafe {
        dec_ref_ptr(_py, awaited_key.0);
        dec_ref_ptr(_py, waiter_ptr);
    }
    if matches!(
        std::env::var("MOLT_TRACE_PROMISE").ok().as_deref(),
        Some("1")
    ) {
        let poll_fn = crate::object::object_poll_fn(awaited_key.0);
        if poll_fn == promise_poll_fn_addr() {
            eprintln!(
                "molt async trace: await_clear_promise waiter=0x{:x} awaited=0x{:x}",
                waiter_ptr as usize, awaited_key.0 as usize
            );
        }
    }
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    let mut awaiter_index_map = await_waiter_index_map(_py).lock().unwrap();
    if let Some(waiters) = awaiters_map.get_mut(&awaited_key) {
        let waiter_index = awaiter_index_map.entry(awaited_key).or_default();
        if waiter_index.positions.len() != waiters.len() {
            waiter_index.positions = rebuild_unique_index(waiters.as_slice());
        }
        indexed_unique_vec_swap_remove(waiters, &mut waiter_index.positions, waiter_key);
        if waiters.is_empty() {
            awaiters_map.remove(&awaited_key);
            awaiter_index_map.remove(&awaited_key);
        }
    } else {
        awaiter_index_map.remove(&awaited_key);
    }
}

struct AwaitWaiterEdge {
    waiter: PtrSlot,
    awaited: PtrSlot,
}

fn await_waiter_edges_take(_py: &PyToken<'_>, awaited_ptr: *mut u8) -> Vec<AwaitWaiterEdge> {
    if awaited_ptr.is_null() {
        return Vec::new();
    }
    let awaited_key = PtrSlot(awaited_ptr);
    let mut waiting_map = task_waiting_on(_py).lock().unwrap();
    let mut awaiters_map = await_waiters(_py).lock().unwrap();
    let mut awaiter_index_map = await_waiter_index_map(_py).lock().unwrap();
    let waiters = awaiters_map.remove(&awaited_key).unwrap_or_default();
    awaiter_index_map.remove(&awaited_key);
    let mut edges = Vec::with_capacity(waiters.len());
    for waiter in waiters {
        match waiting_map.remove(&waiter) {
            Some(recorded_awaited) if recorded_awaited == awaited_key => {
                edges.push(AwaitWaiterEdge {
                    waiter,
                    awaited: recorded_awaited,
                });
            }
            Some(recorded_awaited) => {
                waiting_map.insert(waiter, recorded_awaited);
            }
            None => {}
        }
    }
    edges
}

pub(crate) fn wake_await_waiters(_py: &PyToken<'_>, awaited_ptr: *mut u8) -> usize {
    let edges = await_waiter_edges_take(_py, awaited_ptr);
    let count = edges.len();
    for edge in edges {
        wake_task_ptr(_py, edge.waiter.0);
        unsafe {
            dec_ref_ptr(_py, edge.awaited.0);
            dec_ref_ptr(_py, edge.waiter.0);
        }
    }
    count
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thread_task_state(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) -> Option<Arc<ThreadTaskState>> {
    if future_ptr.is_null() {
        return None;
    }
    runtime_state(_py)
        .thread_tasks
        .lock()
        .unwrap()
        .get(&PtrSlot(future_ptr))
        .cloned()
}

pub(crate) fn process_task_state(
    _py: &PyToken<'_>,
    future_ptr: *mut u8,
) -> Option<Arc<ProcessTaskState>> {
    if future_ptr.is_null() {
        return None;
    }
    runtime_state(_py)
        .process_tasks
        .lock()
        .unwrap()
        .get(&PtrSlot(future_ptr))
        .cloned()
}

/// Resolve the existing await graph once under its lock. Floyd cycle detection
/// avoids both arbitrary nesting limits and allocation on the poll hot path.
/// None denotes an await cycle (blocked until cancellation), not ready work.
pub(crate) fn await_chain_terminal(py: &PyToken<'_>, task: *mut u8) -> Option<*mut u8> {
    if task.is_null() {
        return None;
    }
    let links = task_waiting_on(py).lock().unwrap();
    let next = |ptr: *mut u8| {
        links
            .get(&PtrSlot(ptr))
            .map(|slot| slot.0)
            .filter(|ptr| !ptr.is_null())
    };
    let mut cursor = task;
    let mut fast = task;
    loop {
        let Some(target) = next(cursor) else {
            return Some(cursor);
        };
        cursor = target;
        fast = next(fast).and_then(next).unwrap_or(std::ptr::null_mut());
        if cursor == fast {
            return None;
        }
    }
}

pub(crate) fn task_waiting_on_event(py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
    let Some(awaited) = await_chain_terminal(py, task_ptr) else {
        return !task_ptr.is_null();
    };
    if awaited == task_ptr {
        return false;
    }
    unsafe {
        let header = header_from_obj_ptr(awaited);
        let poll_fn = crate::object::object_poll_fn(awaited);
        ((*header).load_synchronized_flags() & HEADER_FLAG_SPAWN_RETAIN) != 0
            || poll_fn == io_wait_poll_fn_addr()
            || poll_fn == thread_poll_fn_addr()
            || poll_fn == process_poll_fn_addr()
            || poll_fn == promise_poll_fn_addr()
            || super::task_sleep_scheduled(py, awaited)
    }
}

pub(crate) fn task_waiting_on_future(_py: &PyToken<'_>, task_ptr: *mut u8) -> Option<*mut u8> {
    if task_ptr.is_null() {
        return None;
    }
    let waiting_map = task_waiting_on(_py).lock().unwrap();
    waiting_map.get(&PtrSlot(task_ptr)).map(|val| val.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MoltObject, dec_ref_bits, header_from_obj_ptr, molt_future_new, ptr_from_bits};

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn wake_await_waiters_releases_graph_edge_refs() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let waiter_bits = molt_future_new(0, 0);
            let awaited_bits = molt_future_new(0, 0);
            let waiter_ptr = ptr_from_bits(waiter_bits);
            let awaited_ptr = ptr_from_bits(awaited_bits);
            assert_eq!(ref_count(waiter_ptr), 1);
            assert_eq!(ref_count(awaited_ptr), 1);

            await_waiter_register(_py, waiter_ptr, awaited_ptr);
            assert_eq!(ref_count(waiter_ptr), 2);
            assert_eq!(ref_count(awaited_ptr), 2);

            assert_eq!(wake_await_waiters(_py, awaited_ptr), 1);
            assert_eq!(ref_count(waiter_ptr), 1);
            assert_eq!(ref_count(awaited_ptr), 1);
            assert!(task_waiting_on(_py).lock().unwrap().is_empty());
            assert!(await_waiters(_py).lock().unwrap().is_empty());
            assert!(await_waiter_index_map(_py).lock().unwrap().is_empty());

            dec_ref_bits(_py, MoltObject::from_ptr(waiter_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(awaited_ptr).bits());
        });
    }
}

#[cfg(test)]
mod await_chain_tests {
    use super::*;

    #[test]
    fn deep_wait_chain_and_cycle_have_one_terminal_authority() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let nodes: Vec<u64> = (0..64).map(|_| crate::molt_future_new(0, 0)).collect();
            let terminal = crate::molt_promise_new();
            let mut chain: Vec<*mut u8> = nodes.iter().copied().map(crate::ptr_from_bits).collect();
            chain.push(crate::ptr_from_bits(terminal));
            for pair in chain.windows(2) {
                await_waiter_register(py, pair[0], pair[1]);
            }
            assert_eq!(await_chain_terminal(py, chain[0]), chain.last().copied());
            assert!(task_waiting_on_event(py, chain[0]));
            assert_eq!(
                crate::async_rt::generators::resolve_sleep_target(py, chain[0]),
                *chain.last().unwrap()
            );

            await_waiter_register(py, *chain.last().unwrap(), chain[0]);
            assert_eq!(await_chain_terminal(py, chain[0]), None);
            assert!(task_waiting_on_event(py, chain[0]));
            for &node in &chain {
                await_waiter_clear(py, node);
            }
            for node in nodes {
                crate::dec_ref_bits(py, node);
            }
            crate::dec_ref_bits(py, terminal);
        });
    }
}
