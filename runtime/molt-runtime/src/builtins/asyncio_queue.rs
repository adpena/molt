// === FILE: runtime/molt-runtime/src/builtins/asyncio_queue.rs ===
//
// Intrinsic implementations for asyncio.Queue, asyncio.PriorityQueue, and
// asyncio.LifoQueue.
//
// Handle model: runtime-owned `Mutex<HashMap<i64, QueueState>>` keyed by an
// atomically-issued per-runtime handle ID, returned to Python as a NaN-boxed
// integer. The registry is scoped to `RuntimeState`, so runtime teardown and
// isolate reset release all queued object references deterministically.
//
// Queue types:
//   0 = FIFO  (VecDeque, popleft)
//   1 = LIFO  (Vec, pop from end)
//   2 = Priority (BinaryHeap<Reverse<OrderedItem>>, min-heap by MoltObject numeric ordering)
//
// Refcount protocol:
// - Items pushed into the queue are inc_ref'd; items popped are returned without
//   extra inc_ref (caller takes ownership of the reference the queue held).
// - Future waiters are owned by the Python Queue object. Rust owns only the
//   data-structure state that must survive as an intrinsic-backed primitive.
//
// WASM compatibility: ALL intrinsics in this module are pure data structure
// operations with no I/O, no file descriptors, no platform-specific syscalls,
// and no std::time usage. They compile and run correctly on all targets
// including wasm32-wasi and wasm32-unknown-unknown — no `#[cfg]` gating required.

use crate::state::runtime_state::{RuntimeState, runtime_state};
use crate::*;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

// ─── Queue type discriminator ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueType {
    Fifo,
    Lifo,
    Priority,
}

// ─── Priority queue item wrapper ─────────────────────────────────────────────
//
// Wraps a u64 (NaN-boxed MoltObject bits) with comparison semantics that mirror
// CPython's heapq: numeric ordering via MoltObject's int/float extraction.
// Non-numeric items compare by their raw bits for a stable total order.

#[derive(Clone, Copy)]
struct OrderedItem(u64);

impl OrderedItem {
    /// Extract a comparable f64 from the NaN-boxed bits.
    /// Ints are promoted to f64; floats are used directly; everything else
    /// falls back to raw-bits comparison (stable, deterministic, but not
    /// semantically meaningful — matches CPython's TypeError on uncomparable
    /// types pushed to PriorityQueue, except we silently order them).
    fn sort_key(&self) -> (i8, f64, u64) {
        let obj = obj_from_bits(self.0);
        if let Some(i) = to_i64(obj) {
            // Numeric int — primary sort class 0.
            return (0, i as f64, self.0);
        }
        if let Some(f) = obj.as_float() {
            // Numeric float — primary sort class 0, tiebreak by bits for NaN
            // stability.
            return (0, f, self.0);
        }
        // Non-numeric: sort class 1, order by raw bits for determinism.
        (1, 0.0, self.0)
    }
}

impl PartialEq for OrderedItem {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for OrderedItem {}

impl PartialOrd for OrderedItem {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedItem {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        let (ac, af, ab) = self.sort_key();
        let (bc, bf, bb) = other.sort_key();
        ac.cmp(&bc)
            .then(af.partial_cmp(&bf).unwrap_or(CmpOrdering::Equal))
            .then(ab.cmp(&bb))
    }
}

/// Wrapper for min-heap: `BinaryHeap` is a max-heap by default, so we reverse.
#[derive(Clone, Eq, PartialEq)]
struct Reverse(OrderedItem);

impl PartialOrd for Reverse {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for Reverse {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other.0.cmp(&self.0)
    }
}

// ─── Queue state ─────────────────────────────────────────────────────────────

struct QueueState {
    /// FIFO items (used when queue_type == Fifo).
    fifo_items: VecDeque<u64>,
    /// LIFO items (used when queue_type == Lifo).
    lifo_items: Vec<u64>,
    /// Priority items (used when queue_type == Priority).
    priority_items: BinaryHeap<Reverse>,
    /// Maximum size (0 = unlimited).
    maxsize: usize,
    /// Queue variant.
    queue_type: QueueType,
    /// Number of unfinished tasks (incremented on put, decremented on task_done).
    unfinished_tasks: i64,
    /// Whether shutdown() has been called.
    shutdown: bool,
    /// Whether shutdown(immediate=True) was called.
    shutdown_immediate: bool,
}

impl QueueState {
    fn new(maxsize: usize, queue_type: QueueType) -> Self {
        Self {
            fifo_items: VecDeque::new(),
            lifo_items: Vec::new(),
            priority_items: BinaryHeap::new(),
            maxsize,
            queue_type,
            unfinished_tasks: 0,
            shutdown: false,
            shutdown_immediate: false,
        }
    }

    #[inline]
    fn qsize(&self) -> usize {
        match self.queue_type {
            QueueType::Fifo => self.fifo_items.len(),
            QueueType::Lifo => self.lifo_items.len(),
            QueueType::Priority => self.priority_items.len(),
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.qsize() == 0
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.maxsize > 0 && self.qsize() >= self.maxsize
    }

    /// Push an item into the backing store (type-dispatched).
    fn put(&mut self, item_bits: u64) {
        match self.queue_type {
            QueueType::Fifo => self.fifo_items.push_back(item_bits),
            QueueType::Lifo => self.lifo_items.push(item_bits),
            QueueType::Priority => {
                self.priority_items.push(Reverse(OrderedItem(item_bits)));
            }
        }
    }

    /// Pop an item from the backing store (type-dispatched). Returns `None` if empty.
    fn get(&mut self) -> Option<u64> {
        match self.queue_type {
            QueueType::Fifo => self.fifo_items.pop_front(),
            QueueType::Lifo => self.lifo_items.pop(),
            QueueType::Priority => self.priority_items.pop().map(|r| r.0.0),
        }
    }

    /// Drain all items, returning their bits for refcount cleanup.
    fn drain_all(&mut self) -> Vec<u64> {
        match self.queue_type {
            QueueType::Fifo => self.fifo_items.drain(..).collect(),
            QueueType::Lifo => self.lifo_items.drain(..).collect(),
            QueueType::Priority => self.priority_items.drain().map(|r| r.0.0).collect(),
        }
    }

    fn clear_refs(&mut self, _py: &PyToken<'_>) {
        for item in self.drain_all() {
            dec_ref_bits(_py, item);
        }
    }
}

// ─── Runtime-owned registry ──────────────────────────────────────────────────

pub(crate) struct AsyncioQueueRuntimeState {
    next_queue_handle: AtomicI64,
    queues: Mutex<HashMap<i64, QueueState>>,
}

impl AsyncioQueueRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            next_queue_handle: AtomicI64::new(1),
            queues: Mutex::new(HashMap::new()),
        }
    }

    fn next_queue_handle(&self) -> i64 {
        self.next_queue_handle.fetch_add(1, Ordering::Relaxed)
    }

    fn reset_next_queue_handle(&self) {
        self.next_queue_handle.store(1, Ordering::Relaxed);
    }
}

fn queue_runtime_state(_py: &PyToken<'_>) -> &'static AsyncioQueueRuntimeState {
    &runtime_state(_py).asyncio_queues
}

pub(crate) fn asyncio_queue_clear_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let queues = {
        let mut queues = state
            .asyncio_queues
            .queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *queues)
    };
    state.asyncio_queues.reset_next_queue_handle();
    for (_, mut queue) in queues {
        queue.clear_refs(_py);
    }
}

/// Execute a closure with mutable access to the queue state for the given handle.
/// Returns `None` if the handle is not found.
fn with_queue<F, R>(_py: &PyToken<'_>, handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&mut QueueState) -> R,
{
    let mut map = queue_runtime_state(_py)
        .queues
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.get_mut(&handle).map(f)
}

/// Extract a handle i64 from NaN-boxed bits. Returns -1 on failure (which will
/// never match a valid handle since handles start at 1).
#[inline]
fn handle_from_bits(bits: u64) -> i64 {
    to_i64(obj_from_bits(bits)).unwrap_or(-1)
}

// ─── Intrinsics ──────────────────────────────────────────────────────────────

/// Create a new asyncio queue.
///
/// `maxsize`: NaN-boxed int, maximum queue size (0 = unlimited).
/// `queue_type`: NaN-boxed int, 0 = FIFO, 1 = LIFO, 2 = Priority.
///
/// Returns a NaN-boxed int handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_new(maxsize_bits: u64, queue_type_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsize_obj = obj_from_bits(maxsize_bits);
        let maxsize_raw = to_i64(maxsize_obj).unwrap_or(0);
        if maxsize_raw < 0 {
            return raise_exception::<u64>(_py, "ValueError", "maxsize must be >= 0");
        }
        let maxsize = maxsize_raw as usize;

        let qt_obj = obj_from_bits(queue_type_bits);
        let qt_raw = to_i64(qt_obj).unwrap_or(0);
        let queue_type = match qt_raw {
            0 => QueueType::Fifo,
            1 => QueueType::Lifo,
            2 => QueueType::Priority,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "queue_type must be 0 (FIFO), 1 (LIFO), or 2 (Priority)",
                );
            }
        };

        let state = queue_runtime_state(_py);
        let handle = state.next_queue_handle();
        state
            .queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(handle, QueueState::new(maxsize, queue_type));
        MoltObject::from_int(handle).bits()
    })
}

/// Put an item into the queue without blocking.
///
/// Increments `unfinished_tasks`. If the queue is full, raises `asyncio.QueueFull`.
/// If the queue is shut down, raises `asyncio.QueueShutDown`.
///
/// Returns `MoltObject::none()` on success.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_put_nowait(handle_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);

        // We need to inc_ref the item before storing it.  Do it speculatively;
        // if we end up not storing (error path), we dec_ref it back.
        inc_ref_bits(_py, item_bits);

        let result = with_queue(_py, handle, |state| {
            if state.shutdown {
                return Err("QueueShutDown");
            }
            if state.is_full() {
                return Err("QueueFull");
            }
            state.unfinished_tasks += 1;
            state.put(item_bits);
            Ok(())
        });

        match result {
            None => {
                dec_ref_bits(_py, item_bits);
                raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found")
            }
            Some(Err("QueueShutDown")) => {
                dec_ref_bits(_py, item_bits);
                raise_exception::<u64>(_py, "asyncio.QueueShutDown", "")
            }
            Some(Err(_)) => {
                // QueueFull
                dec_ref_bits(_py, item_bits);
                raise_exception::<u64>(_py, "asyncio.QueueFull", "")
            }
            Some(Ok(())) => MoltObject::none().bits(),
        }
    })
}

/// Get an item from the queue without blocking.
///
/// If the queue is empty and shutdown, raises `asyncio.QueueShutDown`.
/// If the queue is empty, raises `asyncio.QueueEmpty`.
///
/// Returns the item bits (caller takes ownership of the reference).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_get_nowait(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);

        let result = with_queue(_py, handle, |state| {
            if let Some(item) = state.get() {
                // Item was inc_ref'd when put; caller now owns that reference.
                Ok(item)
            } else if state.shutdown {
                Err("QueueShutDown")
            } else {
                Err("QueueEmpty")
            }
        });

        match result {
            None => raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found"),
            Some(Err("QueueShutDown")) => raise_exception::<u64>(_py, "asyncio.QueueShutDown", ""),
            Some(Err(_)) => {
                // QueueEmpty
                raise_exception::<u64>(_py, "asyncio.QueueEmpty", "")
            }
            Some(Ok(item_bits)) => item_bits,
        }
    })
}

/// Return the current number of items in the queue.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_qsize(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(size) = with_queue(_py, handle, |state| state.qsize() as i64) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(size).bits()
    })
}

/// Return the maxsize of the queue (0 = unlimited).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_maxsize(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(maxsize) = with_queue(_py, handle, |state| state.maxsize as i64) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(maxsize).bits()
    })
}

/// Return True if the queue is empty.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_empty(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(empty) = with_queue(_py, handle, |state| state.is_empty()) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_bool(empty).bits()
    })
}

/// Return True if the queue is full.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_full(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(full) = with_queue(_py, handle, |state| state.is_full()) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_bool(full).bits()
    })
}

/// Decrement the `unfinished_tasks` counter.
///
/// Raises `ValueError` if called more times than items have been put.
/// Returns `MoltObject::none()` on success.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_task_done(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);

        let result = with_queue(_py, handle, |state| {
            if state.unfinished_tasks <= 0 {
                return Err(());
            }
            state.unfinished_tasks -= 1;
            Ok(state.unfinished_tasks)
        });

        match result {
            None => raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found"),
            Some(Err(())) => {
                raise_exception::<u64>(_py, "ValueError", "task_done() called too many times")
            }
            Some(Ok(_)) => MoltObject::none().bits(),
        }
    })
}

/// Return the current `unfinished_tasks` count.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_unfinished_tasks(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(count) = with_queue(_py, handle, |state| state.unfinished_tasks) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(count).bits()
    })
}

/// Shut down the queue.
///
/// `immediate`: NaN-boxed bool/int. If truthy, the queue is drained immediately
/// and all remaining items are dec_ref'd.
///
/// Returns `MoltObject::none()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_shutdown(handle_bits: u64, immediate_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let immediate = is_truthy(_py, obj_from_bits(immediate_bits));

        // Collect drained items to dec_ref outside the lock.
        let items_to_free = {
            let mut map = queue_runtime_state(_py)
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(state) = map.get_mut(&handle) else {
                return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
            };

            state.shutdown = true;
            state.shutdown_immediate = immediate;

            if immediate {
                state.drain_all()
            } else {
                Vec::new()
            }
        };

        for item in items_to_free {
            dec_ref_bits(_py, item);
        }

        MoltObject::none().bits()
    })
}

/// Return True if the queue has been shut down.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_is_shutdown(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(shut) = with_queue(_py, handle, |state| state.shutdown) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_bool(shut).bits()
    })
}

/// Drop and clean up a queue handle. All remaining items are dec_ref'd.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_drop(handle_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);

        let removed = queue_runtime_state(_py)
            .queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&handle);

        if let Some(mut state) = removed {
            state.clear_refs(_py);
        }

        MoltObject::none().bits()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering as AtomicOrdering;

    fn ref_count(ptr: *mut u8) -> u32 {
        unsafe {
            (*header_from_obj_ptr(ptr))
                .ref_count
                .load(AtomicOrdering::Relaxed)
        }
    }

    #[test]
    fn asyncio_queue_state_is_runtime_scoped_and_clearable() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            asyncio_queue_clear_state(_py, state);

            let handle_bits = molt_asyncio_queue_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
            );
            assert_eq!(to_i64(obj_from_bits(handle_bits)), Some(1));
            assert_eq!(state.asyncio_queues.queues.lock().unwrap().len(), 1);

            let item_ptr = alloc_string(_py, b"asyncio-queue-state-item");
            let item_bits = MoltObject::from_ptr(item_ptr).bits();
            let item_refs_initial = ref_count(item_ptr);
            assert!(obj_from_bits(molt_asyncio_queue_put_nowait(handle_bits, item_bits)).is_none());
            assert_eq!(ref_count(item_ptr), item_refs_initial + 1);

            asyncio_queue_clear_state(_py, state);

            assert!(state.asyncio_queues.queues.lock().unwrap().is_empty());
            assert_eq!(ref_count(item_ptr), item_refs_initial);

            let handle2_bits = molt_asyncio_queue_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
            );
            assert_eq!(to_i64(obj_from_bits(handle2_bits)), Some(1));
            asyncio_queue_clear_state(_py, state);

            dec_ref_bits(_py, item_bits);
            assert!(!exception_pending(_py));
        });
    }
}
