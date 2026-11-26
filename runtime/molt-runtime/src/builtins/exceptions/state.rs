use crate::runtime_flags::{self, RuntimeFlag};
use crate::{PyToken, runtime_state};
use std::cell::{Cell, RefCell};
use std::sync::atomic::AtomicU64;

pub(super) fn debug_oom() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_OOM)
}

#[derive(Clone, Copy)]
pub(crate) struct ExceptionContextFallback {
    pub(crate) bits: u64,
    pub(crate) owned: bool,
}

/// One field of this thread's record (`crate::state::ExecutionTls`), named as
/// the thread-local it replaces. `with` reaches the record through the
/// thread-local key (one `_tlv_get_addr`); `of` reaches it through the
/// execution context compiled code passes, with no thread-local access, and
/// is what a hot path uses.
pub(crate) struct RecordField<T: 'static>(fn(&'static crate::state::ExecutionTls) -> &'static T);

impl<T: 'static> RecordField<T> {
    #[inline(always)]
    pub(crate) fn with<R>(&'static self, f: impl FnOnce(&'static T) -> R) -> R {
        f((self.0)(crate::state::execution_tls()))
    }

    /// The record has no destructor, so access never fails; the result keeps
    /// the shape of `LocalKey::try_with` for paths that ran during teardown.
    #[inline(always)]
    pub(crate) fn try_with<R>(
        &'static self,
        f: impl FnOnce(&'static T) -> R,
    ) -> Result<R, std::convert::Infallible> {
        Ok(self.with(f))
    }

    #[inline(always)]
    pub(crate) fn of(&'static self, py: &PyToken<'_>) -> &'static T {
        (self.0)(py.execution_tls())
    }
}

/// CPython-compatible pending exception for the current native thread.
/// Async task execution uses the task-keyed runtime map instead; the thread
/// slot is the sole authority whenever no task is active. The record has no
/// destructor, so the slot stays accessible during the exact-origin,
/// two-domain thread-state drain that releases its strong edge.
pub(crate) static THREAD_LAST_EXCEPTION: RecordField<ThreadExceptionState> =
    RecordField(|tls| &tls.thread_last_exception);
pub(crate) static EXCEPTION_HANDLER_DEPTH: RecordField<Cell<usize>> =
    RecordField(|tls| &tls.exception_handler_depth);
pub(crate) static EXCEPTION_STACK_BASELINE: RecordField<Cell<usize>> =
    RecordField(|tls| &tls.exception_stack_baseline);
pub(crate) static ACTIVE_EXCEPTION_STACK: RecordField<std::mem::ManuallyDrop<RefCell<Vec<u64>>>> =
    RecordField(|tls| &tls.active_exception_stack);
pub(crate) static ACTIVE_EXCEPTION_FALLBACK: RecordField<
    std::mem::ManuallyDrop<RefCell<Vec<ExceptionContextFallback>>>,
> = RecordField(|tls| &tls.active_exception_fallback);
pub(crate) static GENERATOR_RAISE: RecordField<Cell<bool>> =
    RecordField(|tls| &tls.generator_raise);
pub(crate) static TASK_RAISE_ACTIVE: RecordField<Cell<bool>> =
    RecordField(|tls| &tls.task_raise_active);

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

impl ThreadExceptionState {
    pub(crate) const fn new() -> Self {
        Self {
            slot: Cell::new(std::ptr::null_mut()),
        }
    }

    pub(crate) fn get(&self) -> *mut u8 {
        self.slot.get()
    }

    pub(crate) fn replace(&self, ptr: *mut u8) -> *mut u8 {
        self.slot.replace(ptr)
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

pub(crate) fn exception_clear_reason_set(reason: &'static str) {
    EXCEPTION_CLEAR_REASON.with(|cell| {
        *cell.borrow_mut() = Some(reason);
    });
}

pub(super) fn exception_clear_reason_take() -> Option<&'static str> {
    EXCEPTION_CLEAR_REASON.with(|cell| cell.borrow_mut().take())
}
