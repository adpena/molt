use crate::runtime_flags::{self, RuntimeFlag};
use crate::{GilGuard, MoltObject, PyToken, dec_ref_bits, runtime_state};
use std::cell::{Cell, RefCell};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

pub(super) fn debug_oom() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_OOM)
}

#[derive(Clone, Copy)]
pub(crate) struct ExceptionContextFallback {
    pub(crate) bits: u64,
    pub(crate) owned: bool,
}

thread_local! {
    /// CPython-compatible pending exception for the current native thread.
    /// Async task execution uses the task-keyed runtime map instead; the
    /// thread slot is the sole authority whenever no task is active. The slot
    /// has no destructor, so it stays accessible while other thread-locals are
    /// torn down; `THREAD_EXCEPTION_RELEASE` releases its strong edge.
    pub(crate) static THREAD_LAST_EXCEPTION: ThreadExceptionState = const { ThreadExceptionState::new() };
    /// Thread-exit owner of the pending exception's strong edge. Releasing it
    /// runs finalizer probes that preserve and restore the pending error, so
    /// they re-enter `THREAD_LAST_EXCEPTION`; a slot with its own destructor is
    /// inaccessible while that destructor runs and would abort the thread.
    static THREAD_EXCEPTION_RELEASE: ThreadExceptionRelease = const { ThreadExceptionRelease };
    pub(crate) static EXCEPTION_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    pub(crate) static EXCEPTION_STACK_BASELINE: Cell<usize> = const { Cell::new(0) };
    pub(crate) static ACTIVE_EXCEPTION_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static ACTIVE_EXCEPTION_FALLBACK: RefCell<Vec<ExceptionContextFallback>> = const { RefCell::new(Vec::new()) };
    pub(crate) static GENERATOR_RAISE: Cell<bool> = const { Cell::new(false) };
    pub(crate) static TASK_RAISE_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Inline fast byte for the active execution context, in this thread's
/// record, where compiled code tests it with one load. It mirrors the thread
/// slot outside async execution and the active task's suspended slot while a
/// task is installed on this native thread.
#[inline(always)]
pub(crate) fn exception_pending_flag() -> &'static Cell<bool> {
    &crate::state::execution_tls().exception_pending
}

pub(crate) struct ThreadExceptionState {
    slot: Cell<*mut u8>,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct ThreadExceptionDropTestGate {
    pub(super) owner: std::thread::ThreadId,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: Arc<std::sync::Barrier>,
    pub(super) completion: std::sync::mpsc::Sender<()>,
}

#[cfg(test)]
pub(super) static THREAD_EXCEPTION_DROP_TEST_GATE: Mutex<Option<ThreadExceptionDropTestGate>> =
    Mutex::new(None);

impl ThreadExceptionState {
    const fn new() -> Self {
        Self {
            slot: Cell::new(std::ptr::null_mut()),
        }
    }

    pub(crate) fn get(&self) -> *mut u8 {
        self.slot.get()
    }

    pub(crate) fn replace(&self, ptr: *mut u8) -> *mut u8 {
        arm_thread_exception_release(ptr);
        self.slot.replace(ptr)
    }
}

/// First access registers the thread-exit release. During that release the
/// sentinel is already being destroyed; an exception published by a finalizer
/// there is drained by the release loop itself.
fn arm_thread_exception_release(ptr: *mut u8) {
    if !ptr.is_null() {
        let _ = THREAD_EXCEPTION_RELEASE.try_with(|_| ());
    }
}

struct ThreadExceptionRelease;

impl Drop for ThreadExceptionRelease {
    fn drop(&mut self) {
        if THREAD_LAST_EXCEPTION.with(|state| state.get().is_null()) {
            return;
        }
        let gil = GilGuard::new();
        #[cfg(test)]
        let test_completion = {
            let gate = { THREAD_EXCEPTION_DROP_TEST_GATE.lock().unwrap().clone() };
            if let Some(gate) = gate
                && gate.owner == std::thread::current().id()
            {
                gate.entered.send(()).unwrap();
                gate.release.wait();
                Some(gate.completion)
            } else {
                None
            }
        };
        // Liveness must be validated only after acquiring the same GIL that
        // shutdown holds through unpublication and free. A pre-lock check can
        // race with shutdown and turn this DECREF into a use-after-free.
        if crate::state::runtime_state::runtime_state_for_gil().is_none() {
            return;
        }
        let py = gil.token();
        // Releasing an exception can run finalizers that publish another one;
        // drain until the slot stays empty.
        loop {
            let ptr = THREAD_LAST_EXCEPTION.with(|state| state.slot.replace(std::ptr::null_mut()));
            if ptr.is_null() {
                break;
            }
            dec_ref_bits(&py, MoltObject::from_ptr(ptr).bits());
        }
        #[cfg(test)]
        if let Some(completion) = test_completion {
            completion.send(()).unwrap();
        }
    }
}

const EXCEPTIONS_OBJECT_SLOT_COUNT: usize = 1;

pub(crate) struct ExceptionsRuntimeState {
    pub(super) memory_error: super::memory_error::MemoryErrorReserve,
    pub(super) unraisable_hook_args_class: AtomicU64,
}

impl ExceptionsRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            memory_error: super::memory_error::MemoryErrorReserve::new(),
            unraisable_hook_args_class: AtomicU64::new(0),
        }
    }

    pub(super) fn object_slots(&self) -> [&AtomicU64; EXCEPTIONS_OBJECT_SLOT_COUNT] {
        [&self.unraisable_hook_args_class]
    }
}

pub(super) fn exceptions_state(_py: &PyToken<'_>) -> &'static ExceptionsRuntimeState {
    &runtime_state(_py).exceptions
}

#[inline]
pub(super) fn debug_exception_flow() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTION_FLOW)
}

#[inline]
pub(super) fn debug_exception_clear() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTION_CLEAR)
}

#[inline]
pub(super) fn debug_exception_raise() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTION_RAISE)
}

#[inline]
pub(super) fn debug_exception_pending() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTION_PENDING)
}

#[inline]
pub(super) fn debug_exception_rc() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTION_RC)
}

#[inline]
pub(super) fn trace_exception_stack() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_EXCEPTION_STACK)
}

/// Cached `MOLT_DEBUG_EXCEPTIONS` flag. `record_exception_with_caller_frame`
/// runs on every exception raise, so reading the env var directly there takes
/// the libc environ lock and heap-allocates per raise — a measurable tax in
/// exception-heavy loops. Cache it like the sibling flags above.
#[inline]
pub(super) fn debug_exceptions() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_EXCEPTIONS)
}

thread_local! {
    static EXCEPTION_CLEAR_REASON: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

thread_local! {
    /// Stashed col_offset/end_col_offset from the frame stack at the point
    /// an exception was recorded.  The traceback formatter reads this to
    /// produce correct caret annotations.  (-1, -1) = unknown.
    pub(super) static LAST_EXCEPTION_COL: RefCell<(i64, i64)> = const { RefCell::new((-1, -1)) };
}

pub(crate) fn exception_clear_reason_set(reason: &'static str) {
    EXCEPTION_CLEAR_REASON.with(|cell| {
        *cell.borrow_mut() = Some(reason);
    });
}

pub(super) fn exception_clear_reason_take() -> Option<&'static str> {
    EXCEPTION_CLEAR_REASON.with(|cell| cell.borrow_mut().take())
}
