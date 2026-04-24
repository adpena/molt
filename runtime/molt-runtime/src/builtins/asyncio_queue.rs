// === FILE: runtime/molt-runtime/src/builtins/asyncio_queue.rs ===
//
// Intrinsic implementations for asyncio.Queue, asyncio.PriorityQueue, and
// asyncio.LifoQueue.
//
// Handle model: global `LazyLock<Mutex<HashMap<i64, QueueState>>>` keyed by an
// atomically-issued handle ID, returned to Python as a NaN-boxed integer.
// Uses a global registry (not thread-local) so handles are visible across all
// threads. The GIL serializes all Python-level access, so the Mutex is always
// uncontended.
//
// Queue types:
//   0 = FIFO  (VecDeque, popleft)
//   1 = LIFO  (Vec, pop from end)
//   2 = Priority (BinaryHeap<Reverse<OrderedItem>>, min-heap by MoltObject numeric ordering)
//
// Refcount protocol:
// - Items pushed into the queue are inc_ref'd; items popped are returned without
//   extra inc_ref (caller takes ownership of the reference the queue held).
// - Waiter bits (Future objects) are inc_ref'd on registration, dec_ref'd on
//   notification or cleanup.
//
// WASM compatibility: ALL intrinsics in this module are pure data structure
// operations with no I/O, no file descriptors, no platform-specific syscalls,
// and no std::time usage. They compile and run correctly on all targets
// including wasm32-wasi and wasm32-unknown-unknown — no `#[cfg]` gating required.

use crate::*;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ─── Handle counter ──────────────────────────────────────────────────────────

static NEXT_QUEUE_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_queue_handle() -> i64 {
    NEXT_QUEUE_HANDLE.fetch_add(1, Ordering::Relaxed)
}

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
    /// Waiting putter Future bits.
    putters: VecDeque<u64>,
    /// Waiting getter Future bits.
    getters: VecDeque<u64>,
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
            putters: VecDeque::new(),
            getters: VecDeque::new(),
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
}

// ─── Global registry ─────────────────────────────────────────────────────────

static QUEUE_REGISTRY: LazyLock<Mutex<HashMap<i64, QueueState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Execute a closure with mutable access to the queue state for the given handle.
/// Returns `None` if the handle is not found.
fn with_queue<F, R>(handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&mut QueueState) -> R,
{
    let mut map = QUEUE_REGISTRY.lock().unwrap();
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

        let handle = next_queue_handle();
        QUEUE_REGISTRY
            .lock()
            .unwrap()
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

        let result = with_queue(handle, |state| {
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

        let result = with_queue(handle, |state| {
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
        let Some(size) = with_queue(handle, |state| state.qsize() as i64) else {
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
        let Some(maxsize) = with_queue(handle, |state| state.maxsize as i64) else {
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
        let Some(empty) = with_queue(handle, |state| state.is_empty()) else {
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
        let Some(full) = with_queue(handle, |state| state.is_full()) else {
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

        let result = with_queue(handle, |state| {
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
        let Some(count) = with_queue(handle, |state| state.unfinished_tasks) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(count).bits()
    })
}

/// Return the number of waiting putters.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_putter_count(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(count) = with_queue(handle, |state| state.putters.len() as i64) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(count).bits()
    })
}

/// Return the number of waiting getters.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_getter_count(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(count) = with_queue(handle, |state| state.getters.len() as i64) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_int(count).bits()
    })
}

/// Register a putter waiter (Future bits). The waiter is inc_ref'd.
///
/// Returns `MoltObject::none()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_add_putter(handle_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        inc_ref_bits(_py, waiter_bits);

        let result = with_queue(handle, |state| {
            state.putters.push_back(waiter_bits);
        });

        if result.is_none() {
            dec_ref_bits(_py, waiter_bits);
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        }
        MoltObject::none().bits()
    })
}

/// Register a getter waiter (Future bits). The waiter is inc_ref'd.
///
/// Returns `MoltObject::none()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_add_getter(handle_bits: u64, waiter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        inc_ref_bits(_py, waiter_bits);

        let result = with_queue(handle, |state| {
            state.getters.push_back(waiter_bits);
        });

        if result.is_none() {
            dec_ref_bits(_py, waiter_bits);
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        }
        MoltObject::none().bits()
    })
}

/// Wake up to `count` putters by popping them from the front of the putters
/// deque. Each woken putter is dec_ref'd (the Python shim is responsible for
/// calling `Future.set_result` on them before they are released here).
///
/// Returns the number of putters actually notified (NaN-boxed int).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_notify_putters(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(1).max(0) as usize;

        let waiters: Vec<u64> = {
            let mut map = QUEUE_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&handle) else {
                return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
            };
            let n = count.min(state.putters.len());
            state.putters.drain(..n).collect()
        };

        let notified = waiters.len() as i64;
        // Dec-ref each waiter outside the lock.
        for w in waiters {
            dec_ref_bits(_py, w);
        }
        MoltObject::from_int(notified).bits()
    })
}

/// Wake up to `count` getters by popping them from the front of the getters
/// deque. Each woken getter is dec_ref'd.
///
/// Returns the number of getters actually notified (NaN-boxed int).
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_notify_getters(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let count = to_i64(obj_from_bits(count_bits)).unwrap_or(1).max(0) as usize;

        let waiters: Vec<u64> = {
            let mut map = QUEUE_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&handle) else {
                return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
            };
            let n = count.min(state.getters.len());
            state.getters.drain(..n).collect()
        };

        let notified = waiters.len() as i64;
        for w in waiters {
            dec_ref_bits(_py, w);
        }
        MoltObject::from_int(notified).bits()
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

        // Collect items and waiters to dec_ref outside the lock.
        let (items_to_free, putters_to_free, getters_to_free) = {
            let mut map = QUEUE_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&handle) else {
                return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
            };

            state.shutdown = true;
            state.shutdown_immediate = immediate;

            let items = if immediate {
                state.drain_all()
            } else {
                Vec::new()
            };

            // On shutdown, all waiting putters and getters should be woken
            // (the Python shim will set QueueShutDown on them).
            let putters: Vec<u64> = state.putters.drain(..).collect();
            let getters: Vec<u64> = state.getters.drain(..).collect();

            (items, putters, getters)
        };

        // Dec-ref drained items.
        for item in items_to_free {
            dec_ref_bits(_py, item);
        }
        // Dec-ref waiters.
        for w in putters_to_free {
            dec_ref_bits(_py, w);
        }
        for w in getters_to_free {
            dec_ref_bits(_py, w);
        }

        MoltObject::none().bits()
    })
}

/// Return True if the queue has been shut down.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_is_shutdown(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);
        let Some(shut) = with_queue(handle, |state| state.shutdown) else {
            return raise_exception::<u64>(_py, "RuntimeError", "asyncio queue not found");
        };
        MoltObject::from_bool(shut).bits()
    })
}

/// Drop and clean up a queue handle. All remaining items and waiters are dec_ref'd.
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_queue_drop(handle_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = handle_from_bits(handle_bits);

        let removed = QUEUE_REGISTRY.lock().unwrap().remove(&handle);

        if let Some(mut state) = removed {
            // Dec-ref all remaining items.
            let items = state.drain_all();
            for item in items {
                dec_ref_bits(_py, item);
            }
            // Dec-ref all waiting putters.
            for w in state.putters.drain(..) {
                dec_ref_bits(_py, w);
            }
            // Dec-ref all waiting getters.
            for w in state.getters.drain(..) {
                dec_ref_bits(_py, w);
            }
        }

        MoltObject::none().bits()
    });
}
